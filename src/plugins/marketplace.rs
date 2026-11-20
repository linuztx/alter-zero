//! The marketplace manifest: a header and one entry per plugin, read a record at a time.
//!
//! A marketplace is a repository or a folder holding one of
//! [`MARKETPLACE_FILES`] — Claude Code's `.claude-plugin/marketplace.json`,
//! Codex's `.agents/plugins/marketplace.json`, or Cursor's. The header names
//! the marketplace; each entry names a plugin and says where its files are
//! ([`PluginSource`]). See `docs/plugins.md`.
//!
//! Claude's official manifest is ~190 KB of JSON for 317 plugins, so it is
//! **never held whole and never built as a `serde_json::Value` tree**
//! (`docs/memory.md` *The rule*): [`read_marketplace`] streams it through
//! `serde_json::from_reader` into typed records that keep only what the
//! catalog shows, every key it does not read skipped as it goes and every
//! description capped as it is read. A tree of the document would cost ten
//! times its size and, glibc keeping the pages, keep costing it.
//!
//! The reader is tolerant the way the two real marketplaces need: a source
//! is a string or an object, an author a string or an object, any known
//! field of the wrong type is dropped, and an entry that cannot be listed (no
//! name, an invalid name, no source, not an object) is skipped and counted
//! rather than failing the file. Only the top level is strict: it must be an
//! object with a valid `name` and a `plugins` array.

use std::fmt;
use std::io::Read;

use serde::de::{
    self, Deserialize, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde_json::value::RawValue;

use super::id::{plugin_name_rule, validate_marketplace_name};
use super::source::{
    KeyName, MaybeString, PluginSource, RawSource, ignore_map, ignore_scalars, ignore_seq,
    ignore_str, safe_relative,
};

/// Where a marketplace's manifest may be, relative to its root, in the order
/// they are tried. Codex's `api_marketplace.json` (the API-key users' list)
/// is not read.
pub const MARKETPLACE_FILES: [&str; 3] = [
    ".claude-plugin/marketplace.json",
    ".agents/plugins/marketplace.json",
    ".cursor-plugin/marketplace.json",
];

/// How many characters of an entry's description are kept. Claude's longest
/// is 1,221; a catalog row and a detail page need a few lines at most, and
/// the plugin's own manifest has the rest.
pub const DESCRIPTION_CAP: usize = 320;

/// The `policy.installation` value that withdraws a plugin (Codex).
const NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// How many entries the list grows by at a time. Doubling would round
/// Claude's 317 entries up to a 512-slot block of ~140 KB, past glibc's
/// 128 KiB mmap threshold: freeing that mapped block raises the threshold,
/// and the next read's block then lands on the heap and stays resident
/// (`docs/memory.md` *The rule*). Fixed steps keep the block the size of
/// the list.
const ENTRY_STEP: usize = 64;

/// How many skipped entries are given a sentence each; a manifest with more
/// gets one more sentence counting the rest.
pub const SKIPPED_REASONS_MAX: usize = 20;

/// How many characters of a skipped entry's name its sentence quotes.
const SKIPPED_NAME_CHARS: usize = 64;

/// How many `renames` hops [`renamed`] follows before it stops.
const RENAME_HOPS: usize = 8;

/// A marketplace's own fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MarketplaceHeader {
    /// The validated `name` — what plugin ids are written against.
    pub name: String,
    /// `interface.displayName` (Codex).
    pub display_name: Option<String>,
    /// `description`, else `metadata.description` (capped).
    pub description: Option<String>,
    /// `owner.name` (Claude).
    pub owner: Option<String>,
    /// `metadata.pluginRoot`: where a bare-named `source` lives.
    pub plugin_root: Option<String>,
    /// `renames` (`old → new`), in file order; a rename to `null` (a removed
    /// plugin) is left out.
    pub renames: Vec<(String, String)>,
    /// One sentence per entry skipped because it could not be listed — no
    /// name or an invalid one, no source, or not an object — at most
    /// [`SKIPPED_REASONS_MAX`], then one saying how many more there were.
    pub skipped: Vec<String>,
}

/// One plugin as the marketplace lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceEntry {
    pub name: String,
    /// `displayName`, else `interface.displayName`.
    pub display_name: Option<String>,
    /// Trimmed and capped at [`DESCRIPTION_CAP`] characters, on a char
    /// boundary, `…` appended when cut.
    pub description: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    /// A string `author`, or an object's `name`.
    pub author: Option<String>,
    pub homepage: Option<String>,
    /// Resolved against the header's `plugin_root`.
    pub source: PluginSource,
    /// The source is one this build fetches **and** the entry's
    /// `policy.installation` is not `NOT_AVAILABLE`.
    pub installable: bool,
}

/// The whole manifest, read through `serde_json::from_reader` into typed
/// records — never a `Value` tree, never the body in one buffer. Hand it a
/// `BufReader` over the file.
///
/// # Errors
/// When the text is not JSON, or its top level is not an object with a valid
/// `name` and a `plugins` array. A bad entry is skipped instead
/// ([`MarketplaceHeader::skipped`]).
pub fn read_marketplace<R: Read>(
    reader: R,
) -> Result<(MarketplaceHeader, Vec<MarketplaceEntry>), String> {
    let top: Top = serde_json::from_reader(reader).map_err(json_error)?;
    let Some(name) = top.name else {
        return Err("the marketplace manifest has no `name`".into());
    };
    validate_marketplace_name(&name)?;
    let Some(mut entries) = top.plugins else {
        return Err("the marketplace manifest has no `plugins` list".into());
    };
    let header = MarketplaceHeader {
        name,
        display_name: top.display_name,
        description: top.description.or(top.metadata_description),
        owner: top.owner,
        plugin_root: top.plugin_root,
        renames: top.renames,
        skipped: top.skipped,
    };
    if let Some(root) = header
        .plugin_root
        .as_deref()
        .and_then(safe_relative)
        .filter(|root| !root.is_empty())
    {
        for &index in &top.bare {
            if let Some(MarketplaceEntry {
                source: PluginSource::Local { path },
                ..
            }) = entries.get_mut(index)
            {
                *path = format!("{root}/{path}");
            }
        }
    }
    Ok((header, entries))
}

/// One entry's complete JSON object (all its keys), for the installer and the
/// detail page. A second streaming pass: each entry is held only as its raw
/// text while its name is checked, and only the match is built as a `Value`.
///
/// # Errors
/// When the text is not JSON, or its `plugins` is not an array.
pub fn read_entry<R: Read>(reader: R, name: &str) -> Result<Option<serde_json::Value>, String> {
    let mut stream = serde_json::Deserializer::from_reader(reader);
    let found = EntrySearch { name }
        .deserialize(&mut stream)
        .map_err(json_error)?;
    stream.end().map_err(json_error)?;
    found.map_or(Ok(None), |raw| {
        serde_json::from_str(raw.get())
            .map(Some)
            .map_err(json_error)
    })
}

/// The current name for `name`: `renames` followed, at most eight
/// hops (a cycle stops wherever the hops run out).
#[must_use]
pub fn renamed<'a>(header: &'a MarketplaceHeader, name: &'a str) -> &'a str {
    let mut current = name;
    for _ in 0..RENAME_HOPS {
        match header.renames.iter().find(|(old, _)| old == current) {
            Some((_, new)) => current = new,
            None => break,
        }
    }
    current
}

/// A `serde_json` failure as a sentence about the manifest.
fn json_error(error: serde_json::Error) -> String {
    if error.is_data() {
        format!("the marketplace manifest is not a marketplace: {error}")
    } else {
        format!("the marketplace manifest is not valid JSON: {error}")
    }
}

/// `text` trimmed and cut to [`DESCRIPTION_CAP`] characters, `…` appended
/// when cut; `None` when blank. Only the kept prefix is ever copied.
fn capped(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match text.char_indices().nth(DESCRIPTION_CAP) {
        None => Some(text.to_string()),
        Some((cut, _)) => {
            let mut kept = text[..cut].trim_end().to_string();
            kept.push('…');
            Some(kept)
        }
    }
}

/// A non-blank string, or `None`.
fn non_blank(value: MaybeString) -> Option<String> {
    value.0.filter(|s| !s.trim().is_empty())
}

// ===== the top level =====

/// What the header pass collects. Entries are built in their final form as
/// they stream past, a bare-named source (one that lands under
/// `metadata.pluginRoot`) resolved without the root and its index noted in
/// `bare`: the root may come after `plugins`, and patching those few paths
/// afterwards keeps the read to one list of entries.
#[derive(Default)]
struct Top {
    name: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    metadata_description: Option<String>,
    owner: Option<String>,
    plugin_root: Option<String>,
    renames: Vec<(String, String)>,
    plugins: Option<Vec<MarketplaceEntry>>,
    bare: Vec<usize>,
    skipped: Vec<String>,
}

impl<'de> Deserialize<'de> for Top {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TopVisitor;

        impl<'de> Visitor<'de> for TopVisitor {
            type Value = Top;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Top, A::Error> {
                let mut top = Top::default();
                while let Some(key) = map.next_key::<KeyName>()? {
                    match key.0.as_deref() {
                        Some("name") => top.name = non_blank(map.next_value()?),
                        Some("description") => top.description = map.next_value::<Capped>()?.0,
                        Some("owner") => top.owner = map.next_value::<Author>()?.0,
                        Some("interface") => {
                            top.display_name = map.next_value::<DisplayName>()?.0;
                        }
                        Some("metadata") => {
                            let metadata = map.next_value::<Metadata>()?;
                            top.metadata_description = metadata.description;
                            top.plugin_root = metadata.plugin_root;
                        }
                        Some("renames") => top.renames = map.next_value::<Renames>()?.0,
                        Some("plugins") => {
                            let list = map.next_value::<PluginList>()?;
                            top.skipped = list.skipped;
                            top.bare = list.bare;
                            top.plugins = list.entries;
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(top)
            }
        }

        deserializer.deserialize_map(TopVisitor)
    }
}

/// `plugins`: the entries that can be listed, which of them have bare-named
/// sources, and how many could not be listed. `entries` is `None` when
/// `plugins` is not an array.
struct PluginList {
    entries: Option<Vec<MarketplaceEntry>>,
    bare: Vec<usize>,
    skipped: Vec<String>,
}

impl<'de> Deserialize<'de> for PluginList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ListVisitor;

        const NOT_A_LIST: PluginList = PluginList {
            entries: None,
            bare: Vec::new(),
            skipped: Vec::new(),
        };

        impl<'de> Visitor<'de> for ListVisitor {
            type Value = PluginList;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of plugins")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PluginList, A::Error> {
                let mut entries = Vec::new();
                let mut bare = Vec::new();
                let mut skipped = Vec::new();
                let mut unsaid = 0_usize;
                let mut position = 0_usize;
                while let Some(entry) = seq.next_element::<MaybeEntry>()? {
                    position += 1;
                    match entry.0 {
                        Ok((entry, is_bare)) => {
                            if is_bare {
                                bare.push(entries.len());
                            }
                            if entries.len() == entries.capacity() {
                                entries.reserve_exact(ENTRY_STEP);
                            }
                            entries.push(entry);
                        }
                        Err(_) if skipped.len() == SKIPPED_REASONS_MAX => unsaid += 1,
                        Err(skip) => skipped.push(skip.sentence(position)),
                    }
                }
                if unsaid > 0 {
                    skipped.push(format!(
                        "{unsaid} more {} could not be read",
                        if unsaid == 1 { "entry" } else { "entries" }
                    ));
                }
                Ok(PluginList {
                    entries: Some(entries),
                    bare,
                    skipped,
                })
            }

            ignore_scalars!(NOT_A_LIST);
            ignore_str!(NOT_A_LIST);
            ignore_map!(NOT_A_LIST);
        }

        deserializer.deserialize_any(ListVisitor)
    }
}

/// An entry and whether its source is a bare name, or why it cannot be
/// listed.
struct MaybeEntry(Result<(MarketplaceEntry, bool), Skip>);

/// Why an entry cannot be listed.
enum Skip {
    NotAnObject,
    NoName,
    /// The name, and the rule it breaks.
    BadName(String, &'static str),
    NoSource(String),
}

impl Skip {
    /// One sentence, `position` counting entries from 1.
    fn sentence(&self, position: usize) -> String {
        let quoted = |name: &str| -> String {
            let mut shown: String = name.chars().take(SKIPPED_NAME_CHARS).collect();
            if shown.len() < name.len() {
                shown.push('…');
            }
            shown
        };
        match self {
            Self::NotAnObject => format!("entry {position} is not an object"),
            Self::NoName => format!("entry {position} has no name"),
            Self::BadName(name, rule) => format!("{} — {rule}", quoted(name)),
            Self::NoSource(name) => format!("{} — it names no source", quoted(name)),
        }
    }
}

impl<'de> Deserialize<'de> for MaybeEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = MaybeEntry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a plugin entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<MaybeEntry, A::Error> {
                let mut name = None;
                let mut display_name = None;
                let mut interface_name = None;
                let mut description = None;
                let mut version = None;
                let mut category = None;
                let mut author = None;
                let mut homepage = None;
                let mut source = None;
                let mut withdrawn = false;
                while let Some(key) = map.next_key::<KeyName>()? {
                    match key.0.as_deref() {
                        Some("name") => name = non_blank(map.next_value()?),
                        Some("displayName") => display_name = non_blank(map.next_value()?),
                        Some("interface") => interface_name = map.next_value::<DisplayName>()?.0,
                        Some("description") => description = map.next_value::<Capped>()?.0,
                        Some("version") => version = non_blank(map.next_value()?),
                        Some("category") => category = non_blank(map.next_value()?),
                        Some("author") => author = map.next_value::<Author>()?.0,
                        Some("homepage") => homepage = non_blank(map.next_value()?),
                        Some("source") => source = Some(map.next_value::<RawSource>()?),
                        Some("policy") => withdrawn = map.next_value::<Policy>()?.withdrawn,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                let entry = match (name, source) {
                    (None, _) => Err(Skip::NoName),
                    (Some(name), _) if plugin_name_rule(&name).is_some() => {
                        let rule = plugin_name_rule(&name).unwrap_or_default();
                        Err(Skip::BadName(name, rule))
                    }
                    (Some(name), None) => Err(Skip::NoSource(name)),
                    (Some(name), Some(raw)) => {
                        let is_bare = raw.is_bare();
                        let source = raw.resolve(None);
                        Ok((
                            MarketplaceEntry {
                                installable: source.is_installable() && !withdrawn,
                                name,
                                display_name: display_name.or(interface_name),
                                description,
                                version,
                                category,
                                author,
                                homepage,
                                source,
                            },
                            is_bare,
                        ))
                    }
                };
                Ok(MaybeEntry(entry))
            }

            ignore_scalars!(MaybeEntry(Err(Skip::NotAnObject)));
            ignore_str!(MaybeEntry(Err(Skip::NotAnObject)));
            ignore_seq!(MaybeEntry(Err(Skip::NotAnObject)));
        }

        deserializer.deserialize_any(EntryVisitor)
    }
}

// ===== small field readers =====

/// A description, capped as it is read.
struct Capped(Option<String>);

impl<'de> Deserialize<'de> for Capped {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CappedVisitor;

        impl<'de> Visitor<'de> for CappedVisitor {
            type Value = Capped;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a description")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Capped, E> {
                Ok(Capped(capped(text)))
            }

            ignore_scalars!(Capped(None));
            ignore_seq!(Capped(None));
            ignore_map!(Capped(None));
        }

        deserializer.deserialize_any(CappedVisitor)
    }
}

/// `"Jane"` or `{"name": "Jane", …}`.
struct Author(Option<String>);

impl<'de> Deserialize<'de> for Author {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AuthorVisitor;

        impl<'de> Visitor<'de> for AuthorVisitor {
            type Value = Author;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an author")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Author, E> {
                Ok(Author(non_blank(MaybeString(Some(text.to_string())))))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Author, A::Error> {
                Ok(Author(field_of(map, "name")?))
            }

            ignore_scalars!(Author(None));
            ignore_seq!(Author(None));
        }

        deserializer.deserialize_any(AuthorVisitor)
    }
}

/// `{"displayName": …}` — an `interface` object's one field read here.
struct DisplayName(Option<String>);

impl<'de> Deserialize<'de> for DisplayName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct InterfaceVisitor;

        impl<'de> Visitor<'de> for InterfaceVisitor {
            type Value = DisplayName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an interface object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<DisplayName, A::Error> {
                Ok(DisplayName(field_of(map, "displayName")?))
            }

            ignore_scalars!(DisplayName(None));
            ignore_str!(DisplayName(None));
            ignore_seq!(DisplayName(None));
        }

        deserializer.deserialize_any(InterfaceVisitor)
    }
}

/// The one non-blank string field `wanted` of an object; every other key is
/// skipped.
fn field_of<'de, A: MapAccess<'de>>(mut map: A, wanted: &str) -> Result<Option<String>, A::Error> {
    let mut found = None;
    while let Some(key) = map.next_key::<KeyName>()? {
        if key.0.as_deref() == Some(wanted) {
            found = non_blank(map.next_value()?);
        } else {
            map.next_value::<IgnoredAny>()?;
        }
    }
    Ok(found)
}

/// `metadata`: Claude's alternate description and the plugin root.
#[derive(Default)]
struct Metadata {
    description: Option<String>,
    plugin_root: Option<String>,
}

impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MetadataVisitor;

        impl<'de> Visitor<'de> for MetadataVisitor {
            type Value = Metadata;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a metadata object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Metadata, A::Error> {
                let mut metadata = Metadata::default();
                while let Some(key) = map.next_key::<KeyName>()? {
                    match key.0.as_deref() {
                        Some("description") => {
                            metadata.description = map.next_value::<Capped>()?.0;
                        }
                        Some("pluginRoot") => metadata.plugin_root = non_blank(map.next_value()?),
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(metadata)
            }

            ignore_scalars!(Metadata::default());
            ignore_str!(Metadata::default());
            ignore_seq!(Metadata::default());
        }

        deserializer.deserialize_any(MetadataVisitor)
    }
}

/// `renames`: `{old: new}`, a `null` (or any non-string) new name left out.
struct Renames(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Renames {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RenamesVisitor;

        impl<'de> Visitor<'de> for RenamesVisitor {
            type Value = Renames;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a renames object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Renames, A::Error> {
                let mut renames = Vec::new();
                while let Some((old, new)) = map.next_entry::<String, MaybeString>()? {
                    if let Some(new) = non_blank(new) {
                        renames.push((old, new));
                    }
                }
                Ok(Renames(renames))
            }

            ignore_scalars!(Renames(Vec::new()));
            ignore_str!(Renames(Vec::new()));
            ignore_seq!(Renames(Vec::new()));
        }

        deserializer.deserialize_any(RenamesVisitor)
    }
}

/// `policy`: whether `installation` withdraws the plugin.
struct Policy {
    withdrawn: bool,
}

impl<'de> Deserialize<'de> for Policy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PolicyVisitor;

        const OPEN: Policy = Policy { withdrawn: false };

        impl<'de> Visitor<'de> for PolicyVisitor {
            type Value = Policy;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a policy object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Policy, A::Error> {
                let installation = field_of(map, "installation")?;
                Ok(Policy {
                    withdrawn: installation
                        .is_some_and(|value| value.trim().eq_ignore_ascii_case(NOT_AVAILABLE)),
                })
            }

            ignore_scalars!(OPEN);
            ignore_str!(OPEN);
            ignore_seq!(OPEN);
        }

        deserializer.deserialize_any(PolicyVisitor)
    }
}

// ===== one entry =====

/// The top level, searched for the first entry named `name`; everything but
/// `plugins` is skipped.
struct EntrySearch<'n> {
    name: &'n str,
}

impl<'de> DeserializeSeed<'de> for EntrySearch<'_> {
    type Value = Option<Box<RawValue>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EntrySearch<'_> {
    type Value = Option<Box<RawValue>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = None;
        while let Some(key) = map.next_key::<KeyName>()? {
            if key.0.as_deref() == Some("plugins") {
                found = map.next_value_seed(EntryScan { name: self.name })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// `plugins`, each element held as its raw text only while its name is
/// checked; the first match is kept.
struct EntryScan<'n> {
    name: &'n str,
}

impl<'de> DeserializeSeed<'de> for EntryScan<'_> {
    type Value = Option<Box<RawValue>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for EntryScan<'_> {
    type Value = Option<Box<RawValue>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of plugins")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut found = None;
        while let Some(raw) = seq.next_element::<Box<RawValue>>()? {
            if found.is_none()
                && serde_json::from_str::<EntryName>(raw.get())
                    .ok()
                    .and_then(|entry| entry.0)
                    .as_deref()
                    == Some(self.name)
            {
                found = Some(raw);
            }
        }
        Ok(found)
    }
}

/// An entry's `name`, nothing else read.
struct EntryName(Option<String>);

impl<'de> Deserialize<'de> for EntryName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl<'de> Visitor<'de> for NameVisitor {
            type Value = EntryName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a plugin entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<EntryName, A::Error> {
                Ok(EntryName(field_of(map, "name")?))
            }

            ignore_scalars!(EntryName(None));
            ignore_str!(EntryName(None));
            ignore_seq!(EntryName(None));
        }

        deserializer.deserialize_any(NameVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A Claude-shaped marketplace: `$schema`, owner, `renames`, and entries
    /// of every source shape the official one carries — a `./` string, a
    /// pinned `url`, a `url` with a `path`, a `git-subdir`, a `command`, a
    /// `strict:false` entry with `lspServers`, and a skill bundle.
    const CLAUDE: &str = r#"{
  "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
  "name": "claude-plugins-official",
  "description": "Directory of popular Claude Code extensions",
  "owner": {"name": "Anthropic", "email": "support@anthropic.com"},
  "renames": {"adlc": "agentforce-adlc", "wordpress.com": "wordpress", "gone": null},
  "plugins": [
    {"name": "agent-sdk-dev", "description": "Development kit for working with the Claude Agent SDK",
     "author": {"name": "Anthropic", "email": "support@anthropic.com"},
     "source": "./plugins/agent-sdk-dev", "category": "development",
     "homepage": "https://github.com/anthropics/claude-plugins-public/tree/main/plugins/agent-sdk-dev"},
    {"name": "agentforce-adlc", "description": "Agentforce agent development life cycle",
     "source": {"source": "url", "url": "https://github.com/SalesforceAIResearch/agentforce-adlc.git",
                "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}},
    {"name": "atomic-agents", "description": "Atomic agents",
     "source": {"source": "url", "url": "https://github.com/BrainBlend-AI/atomic-agents.git",
                "path": "claude-plugin/atomic-agents", "sha": "33d2ec94f42d4a65393a336e35edf21ee5700303"}},
    {"name": "42crunch-api-security-testing", "description": "API security testing", "version": "1.5.5",
     "source": {"source": "git-subdir", "url": "https://github.com/42Crunch-AI/claude-plugins.git",
                "path": "plugins/api-security-testing", "ref": "v1.5.5",
                "sha": "faf5305385de8afed9468904e8639be737aff39e"}},
    {"name": "xcode", "displayName": "Xcode", "description": "Work with Xcode projects",
     "author": {"name": "Apple Inc."}, "category": "development",
     "source": {"source": "command", "command": "xcrun agent plugin path --plugin-format claude", "mode": "link"},
     "homepage": "https://developer.apple.com/xcode/"},
    {"name": "clangd-lsp", "description": "C/C++ language server (clangd) for code intelligence",
     "version": "1.0.0", "author": {"name": "Anthropic"}, "source": "./plugins/clangd-lsp",
     "category": "development", "strict": false,
     "lspServers": {"clangd": {"command": "clangd", "args": ["--background-index"],
                               "extensionToLanguage": {".c": "c", ".h": "c"}}}},
    {"name": "learn-with-coursera", "description": "Turn any learning intent into a personalized Coursera experience.",
     "author": {"name": "Coursera"}, "category": "learning",
     "source": {"source": "git-subdir", "url": "https://github.com/coursera/skills.git", "path": "skills",
                "ref": "main", "sha": "ac28fd6ebf8584e3ee196159bd6d4514fa07de0f"},
     "strict": false, "skills": ["./learn-with-coursera"], "homepage": "https://github.com/coursera/skills"}
  ]
}"#;

    /// An OpenAI-shaped marketplace: `interface.displayName`, no owner, no
    /// descriptions, `policy` on every entry.
    const OPENAI: &str = r#"{
  "name": "openai-curated",
  "interface": {"displayName": "Codex official"},
  "plugins": [
    {"name": "linear", "source": {"source": "local", "path": "./plugins/linear"},
     "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}, "category": "Productivity"},
    {"name": "qodo", "source": {"source": "git-subdir", "url": "https://github.com/qodo-ai/qodo-skills.git",
     "path": "codex-packages/qodo"}, "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
     "category": "Developer Tools", "interface": {"displayName": "Qodo"}},
    {"name": "withdrawn", "source": {"source": "local", "path": "./plugins/withdrawn"},
     "policy": {"installation": "NOT_AVAILABLE"}, "category": "Productivity"}
  ]
}"#;

    fn read(text: &str) -> (MarketplaceHeader, Vec<MarketplaceEntry>) {
        read_marketplace(text.as_bytes()).expect("reads")
    }

    fn entry<'a>(entries: &'a [MarketplaceEntry], name: &str) -> &'a MarketplaceEntry {
        entries.iter().find(|e| e.name == name).expect(name)
    }

    // ===== the header =====

    #[test]
    fn the_claude_header_reads_its_name_description_owner_and_renames() {
        let (header, entries) = read(CLAUDE);
        assert_eq!(header.name, "claude-plugins-official");
        assert_eq!(
            header.description.as_deref(),
            Some("Directory of popular Claude Code extensions")
        );
        assert_eq!(header.owner.as_deref(), Some("Anthropic"));
        assert_eq!(header.display_name, None);
        assert_eq!(
            header.renames,
            vec![
                ("adlc".to_string(), "agentforce-adlc".to_string()),
                ("wordpress.com".to_string(), "wordpress".to_string()),
            ],
            "a rename to null is a removal, not a rename"
        );
        assert!(header.skipped.is_empty(), "{:?}", header.skipped);
        assert_eq!(entries.len(), 7);
    }

    #[test]
    fn the_openai_header_reads_its_display_name() {
        let (header, entries) = read(OPENAI);
        assert_eq!(header.name, "openai-curated");
        assert_eq!(header.display_name.as_deref(), Some("Codex official"));
        assert_eq!(header.owner, None);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn header_fields_after_the_plugins_still_apply() {
        // `metadata.pluginRoot` arriving after `plugins` must still place the
        // bare-named entries, and `renames` must still be read.
        let (header, entries) = read(
            r#"{"plugins": [{"name": "fmt", "source": "fmt"}, {"name": "lint", "source": "./lint"}],
                "name": "team", "renames": {"old-fmt": "fmt"},
                "metadata": {"description": "Team tools", "pluginRoot": "./plugins"}}"#,
        );
        assert_eq!(header.name, "team");
        assert_eq!(header.plugin_root.as_deref(), Some("./plugins"));
        assert_eq!(header.description.as_deref(), Some("Team tools"));
        assert_eq!(
            header.renames,
            vec![("old-fmt".to_string(), "fmt".to_string())]
        );
        assert_eq!(
            entry(&entries, "fmt").source,
            PluginSource::Local {
                path: "plugins/fmt".into()
            }
        );
        assert_eq!(
            entry(&entries, "lint").source,
            PluginSource::Local {
                path: "lint".into()
            }
        );
    }

    #[test]
    fn a_top_level_description_wins_over_the_metadata_one() {
        let (header, _) = read(
            r#"{"name": "t", "metadata": {"description": "meta"}, "description": "top", "plugins": []}"#,
        );
        assert_eq!(header.description.as_deref(), Some("top"));
    }

    #[test]
    fn a_malformed_top_level_fails_the_file() {
        for (text, needle) in [
            ("[1, 2]", "object"),
            (r#"{"plugins": []}"#, "name"),
            (r#"{"name": 5, "plugins": []}"#, "name"),
            (r#"{"name": "bad name", "plugins": []}"#, "bad name"),
            (r#"{"name": "t"}"#, "plugins"),
            (r#"{"name": "t", "plugins": {}}"#, "plugins"),
            (r#"{"name": "t", "plugins": [}"#, "JSON"),
            ("", "JSON"),
        ] {
            let err = read_marketplace(text.as_bytes()).unwrap_err();
            assert!(err.contains(needle), "{text}: {err}");
        }
    }

    // ===== entries =====

    #[test]
    fn every_source_shape_resolves() {
        let (_, entries) = read(CLAUDE);
        assert_eq!(
            entry(&entries, "agent-sdk-dev").source,
            PluginSource::Local {
                path: "plugins/agent-sdk-dev".into()
            }
        );
        assert!(matches!(
            entry(&entries, "agentforce-adlc").source,
            PluginSource::Git { path: None, .. }
        ));
        assert!(matches!(
            &entry(&entries, "atomic-agents").source,
            PluginSource::Git { path: Some(p), .. } if p == "claude-plugin/atomic-agents"
        ));
        assert!(matches!(
            &entry(&entries, "42crunch-api-security-testing").source,
            PluginSource::Git { reference: Some(r), .. } if r == "v1.5.5"
        ));
        assert_eq!(
            entry(&entries, "xcode").source,
            PluginSource::Unsupported {
                kind: "command".into()
            }
        );
        assert!(!entry(&entries, "xcode").installable);
        assert!(entry(&entries, "agent-sdk-dev").installable);
    }

    #[test]
    fn an_entry_reads_its_display_fields() {
        let (_, entries) = read(CLAUDE);
        let xcode = entry(&entries, "xcode");
        assert_eq!(xcode.display_name.as_deref(), Some("Xcode"));
        assert_eq!(xcode.author.as_deref(), Some("Apple Inc."));
        assert_eq!(xcode.category.as_deref(), Some("development"));
        assert_eq!(
            xcode.homepage.as_deref(),
            Some("https://developer.apple.com/xcode/")
        );
        assert_eq!(
            entry(&entries, "clangd-lsp").version.as_deref(),
            Some("1.0.0")
        );
    }

    #[test]
    fn an_openai_entry_with_policy_and_no_description_reads() {
        let (_, entries) = read(OPENAI);
        let linear = entry(&entries, "linear");
        assert_eq!(linear.description, None);
        assert_eq!(linear.category.as_deref(), Some("Productivity"));
        assert_eq!(
            linear.source,
            PluginSource::Local {
                path: "plugins/linear".into()
            }
        );
        assert!(linear.installable);
        assert_eq!(
            entry(&entries, "qodo").display_name.as_deref(),
            Some("Qodo")
        );
    }

    #[test]
    fn a_not_available_policy_makes_an_entry_not_installable() {
        let (_, entries) = read(OPENAI);
        assert!(!entry(&entries, "withdrawn").installable);
    }

    #[test]
    fn an_author_is_a_string_or_an_object_with_a_name() {
        let (_, entries) = read(
            r#"{"name": "t", "plugins": [
                {"name": "a", "source": "./a", "author": "Jane"},
                {"name": "b", "source": "./b", "author": {"email": "x@y"}},
                {"name": "c", "source": "./c", "author": 7}]}"#,
        );
        assert_eq!(entry(&entries, "a").author.as_deref(), Some("Jane"));
        assert_eq!(entry(&entries, "b").author, None);
        assert_eq!(entry(&entries, "c").author, None);
    }

    #[test]
    fn an_entry_that_cannot_be_listed_is_skipped_with_a_reason() {
        let (header, entries) = read(
            r#"{"name": "t", "plugins": [
                {"name": "ok", "source": "./ok"},
                {"name": "no-source", "description": "x"},
                {"source": "./no-name"},
                {"name": "日本語", "source": "./x"},
                {"name": "rocket-🚀-plugin", "source": "./x"},
                {"name": "-x", "source": "./x"},
                "not an object",
                {"name": "ok2", "source": "./ok2"}]}"#,
        );
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["ok", "ok2"]);
        assert_eq!(
            header.skipped,
            [
                "no-source — it names no source",
                "entry 3 has no name",
                "日本語 — a plugin name is ASCII letters, digits, ., _ and - only",
                "rocket-🚀-plugin — a plugin name is ASCII letters, digits, ., _ and - only",
                "-x — a plugin name starts and ends with an ASCII letter or digit",
                "entry 7 is not an object",
            ]
        );
    }

    #[test]
    fn the_reasons_kept_are_bounded() {
        let entries: Vec<String> = (0..500).map(|_| "1".to_string()).collect();
        let text = format!(r#"{{"name": "t", "plugins": [{}]}}"#, entries.join(","));
        let (header, _) = read(&text);
        assert_eq!(header.skipped.len(), SKIPPED_REASONS_MAX + 1);
        assert_eq!(
            header.skipped.last().map(String::as_str),
            Some("480 more entries could not be read")
        );
    }

    #[test]
    fn a_wrong_typed_field_is_dropped_not_fatal() {
        let (_, entries) = read(
            r#"{"name": "t", "plugins": [{"name": "a", "source": "./a", "description": ["x"],
                "version": 2, "category": {"k": 1}, "policy": "weird", "displayName": null}]}"#,
        );
        let a = entry(&entries, "a");
        assert_eq!(
            (
                a.description.as_ref(),
                a.version.as_ref(),
                a.category.as_ref()
            ),
            (None, None, None)
        );
        assert!(a.installable);
    }

    #[test]
    fn a_long_description_is_capped_on_a_char_boundary() {
        let long = "é".repeat(DESCRIPTION_CAP + 50);
        let text =
            json!({"name": "t", "plugins": [{"name": "a", "source": "./a", "description": long}]})
                .to_string();
        let (_, entries) = read(&text);
        let description = entries[0].description.as_deref().unwrap();
        assert_eq!(description.chars().count(), DESCRIPTION_CAP + 1);
        assert!(description.ends_with('…'));
        let short = json!({"name": "t", "plugins": [{"name": "a", "source": "./a", "description": "  Short.  "}]})
            .to_string();
        assert_eq!(read(&short).1[0].description.as_deref(), Some("Short."));
    }

    // ===== one entry =====

    #[test]
    fn read_entry_returns_the_whole_matching_object() {
        let value = read_entry(CLAUDE.as_bytes(), "clangd-lsp")
            .unwrap()
            .expect("found");
        assert_eq!(value["strict"], json!(false));
        assert_eq!(value["lspServers"]["clangd"]["command"], json!("clangd"));
        let value = read_entry(CLAUDE.as_bytes(), "learn-with-coursera")
            .unwrap()
            .unwrap();
        assert_eq!(value["skills"], json!(["./learn-with-coursera"]));
    }

    #[test]
    fn read_entry_is_none_for_a_name_that_is_not_listed() {
        assert_eq!(read_entry(CLAUDE.as_bytes(), "nope").unwrap(), None);
    }

    #[test]
    fn read_entry_finds_an_entry_whose_name_comes_late() {
        let text =
            r#"{"plugins": [{"source": "./a", "x": {"deep": [1, 2]}, "name": "a"}], "name": "t"}"#;
        assert_eq!(
            read_entry(text.as_bytes(), "a").unwrap().unwrap()["x"]["deep"],
            json!([1, 2])
        );
    }

    #[test]
    fn read_entry_fails_on_broken_json() {
        assert!(read_entry(r#"{"name": "t", "plugins": [{"#.as_bytes(), "a").is_err());
    }

    // ===== renames =====

    #[test]
    fn renamed_follows_the_chain() {
        let header = MarketplaceHeader {
            renames: vec![("a".into(), "b".into()), ("b".into(), "c".into())],
            ..MarketplaceHeader::default()
        };
        assert_eq!(renamed(&header, "a"), "c");
        assert_eq!(renamed(&header, "c"), "c");
        assert_eq!(renamed(&header, "z"), "z");
    }

    #[test]
    fn renamed_stops_on_a_cycle() {
        let header = MarketplaceHeader {
            renames: vec![("a".into(), "b".into()), ("b".into(), "a".into())],
            ..MarketplaceHeader::default()
        };
        // Eight hops from `a` lands back on `a`; what matters is that it ends.
        assert!(["a", "b"].contains(&renamed(&header, "a")));
    }
}

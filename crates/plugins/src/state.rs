//! `plugins.json` — which marketplaces are added, which plugins are
//! installed, and which of them each directory has turned off
//! (`docs/plugins.md` *On disk*) — plus the plugin id grammar, the name rule
//! every path segment obeys, and the marketplaces offered out of the box.
//!
//! ```json
//! { "marketplaces": { "team": { "source": "/srv/plugins" } },
//!   "plugins": { "lint@team": { "version": "1.2.0", "installed": 1763380000 } },
//!   "projects": { "/home/me/work/api": { "disabled": ["lint@team"] } } }
//! ```
//!
//! A plugin is on in every directory until one turns it off — `skills.json`'s
//! shape (`docs/per-directory-state.md`). The format is pure: a document
//! that will not parse is an **error** the caller must not write over
//! ([`PluginsFile::parse`]) — a registry silently reset is a set of plugins
//! silently gone — each edit is a method, and [`PluginsFile::to_json`]
//! renders it back with every key this build does not know carried through.
//! The read is typed and bounded: each value is kept as the text of its
//! field until a known field decodes it, never as a `Value` tree
//! (`docs/memory.md` *The rule*).

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

/// The largest `plugins.json` read: past this the file is not ours to parse.
pub const PLUGINS_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The environment variable that replaces the default marketplaces for a run.
pub const MARKETPLACES_ENV: &str = "ALTER_ZERO_PLUGIN_MARKETPLACES";

/// The marketplaces offered out of the box: `(name, source)`.
pub const DEFAULT_MARKETPLACES: [(&str, &str); 2] = [
    (
        "claude-plugins-official",
        "anthropics/claude-plugins-official",
    ),
    ("openai-curated", "openai/plugins"),
];

/// The longest plugin or marketplace name — the Agent Plugins bound (§5.5),
/// applied to every format.
pub const MAX_NAME_LEN: usize = 64;

/// Is `name` usable as a plugin's or a marketplace's name — and so as one
/// path segment under the plugins tree and the prefix of what a plugin
/// adds? ASCII letters, digits, `.`, `_`, `-`, opening on a letter or digit,
/// at most [`MAX_NAME_LEN`], no `..`.
///
/// # Errors
/// Why the name is refused.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("it is empty".into());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("it is longer than {MAX_NAME_LEN} characters"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("only letters, digits, '.', '_' and '-' are allowed".into());
    }
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return Err("it must start with a letter or a digit".into());
    }
    if name.contains("..") {
        return Err("it may not contain \"..\"".into());
    }
    Ok(())
}

/// [`validate_name`] as a predicate.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    validate_name(name).is_ok()
}

/// A plugin's id, `name@marketplace`: the key in `plugins.json` and the two
/// directory names under `installed/`. Two marketplaces may list a plugin of
/// one name, so the pair is the identity. The default is the empty id, a
/// placeholder that names nothing: no record has it, so no operation finds
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId {
    /// The marketplace entry's name.
    pub name: String,
    /// The marketplace's name.
    pub marketplace: String,
}

impl PluginId {
    /// An id from its two halves, both checked with [`validate_name`].
    ///
    /// # Errors
    /// The half that is refused, and why.
    pub fn new(name: &str, marketplace: &str) -> Result<Self, String> {
        validate_name(name).map_err(|why| format!("\"{name}\" is not a plugin name: {why}"))?;
        validate_name(marketplace)
            .map_err(|why| format!("\"{marketplace}\" is not a marketplace name: {why}"))?;
        Ok(Self {
            name: name.to_string(),
            marketplace: marketplace.to_string(),
        })
    }

    /// Parse `name@marketplace`, split on the **last** `@`.
    ///
    /// # Errors
    /// No `@`, or a half [`PluginId::new`] refuses.
    pub fn parse(id: &str) -> Result<Self, String> {
        let Some((name, marketplace)) = id.trim().rsplit_once('@') else {
            return Err(format!(
                "\"{id}\" is not a plugin id (expected name@marketplace)"
            ));
        };
        Self::new(name, marketplace)
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.marketplace)
    }
}

/// What the user typed to name a plugin: `name` or `name@marketplace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    /// The plugin's name.
    pub name: String,
    /// The marketplace, when one was named.
    pub marketplace: Option<String>,
}

impl PluginSpec {
    /// Parse `plugin[@marketplace]`.
    ///
    /// # Errors
    /// An invalid name, as a sentence.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.contains('@') {
            let id = PluginId::parse(spec)?;
            return Ok(Self {
                name: id.name,
                marketplace: Some(id.marketplace),
            });
        }
        validate_name(spec).map_err(|why| format!("\"{spec}\" is not a plugin name: {why}"))?;
        Ok(Self {
            name: spec.to_string(),
            marketplace: None,
        })
    }
}

/// One marketplace offered without being added: its name when known before
/// fetching, and the source to add it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultMarketplace {
    /// The marketplace's name, when known in advance.
    pub name: Option<String>,
    /// What to add it from, in the `plugin marketplace add` grammar.
    pub source: String,
}

/// The marketplaces to offer: [`DEFAULT_MARKETPLACES`], or — when the
/// [`MARKETPLACES_ENV`] value is set — its comma-separated sources (empty
/// offers none), each optionally `name=source` so an install naming the
/// marketplace can add it on the way.
#[must_use]
pub fn default_marketplaces(env: Option<&str>) -> Vec<DefaultMarketplace> {
    let Some(value) = env else {
        return DEFAULT_MARKETPLACES
            .iter()
            .map(|(name, source)| DefaultMarketplace {
                name: Some((*name).to_string()),
                source: (*source).to_string(),
            })
            .collect();
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| match item.split_once('=') {
            Some((name, source)) if is_valid_name(name.trim()) && !source.trim().is_empty() => {
                DefaultMarketplace {
                    name: Some(name.trim().to_string()),
                    source: source.trim().to_string(),
                }
            }
            _ => DefaultMarketplace {
                name: None,
                source: item.to_string(),
            },
        })
        .collect()
}

/// Keys this build does not know, kept as the JSON text they were written as.
type Extra = BTreeMap<String, String>;
type Fields = BTreeMap<String, Box<RawValue>>;

/// One added marketplace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceRecord {
    /// Where it was added from: `owner/repo`, a git URL, or an absolute
    /// folder (read in place).
    pub source: String,
    /// The branch or tag a git source follows.
    pub git_ref: Option<String>,
    /// The commit its checkout is at (a git source).
    pub revision: Option<String>,
    /// When it was added or last updated, in Unix seconds.
    pub updated: Option<u64>,
    extra: Extra,
}

impl MarketplaceRecord {
    /// A record for a marketplace added from `source`.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Self::default()
        }
    }
}

/// One installed plugin: facts about its copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginRecord {
    /// The manifest's version, else the marketplace entry's.
    pub version: Option<String>,
    /// What the copy was made from: the tree of a plugin kept in its git
    /// marketplace, the commit of one from a repository of its own.
    pub revision: Option<String>,
    /// The version its marketplace entry offered at install — what a later
    /// entry is compared with, for a plugin from a repository of its own.
    pub offered: Option<String>,
    /// The folder it loads from **in place** (a plugin of a marketplace kept
    /// in a directory) rather than a copy under `installed/`. An uninstall
    /// never touches it: it is the user's.
    pub root: Option<String>,
    /// When it was installed or last updated, in Unix seconds.
    pub installed: Option<u64>,
    extra: Extra,
}

/// One directory's turned-off plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPlugins {
    /// The ids turned off there, sorted.
    pub disabled: Vec<String>,
    extra: Extra,
}

/// Why `plugins.json` will not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateParseError(pub String);

impl fmt::Display for StateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StateParseError {}

/// `plugins.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginsFile {
    /// Added marketplaces by name.
    pub marketplaces: BTreeMap<String, MarketplaceRecord>,
    /// Installed plugins by id (`name@marketplace`).
    pub plugins: BTreeMap<String, PluginRecord>,
    /// Per-directory turned-off sets, by absolute directory.
    pub projects: BTreeMap<String, ProjectPlugins>,
    extra: Extra,
    /// Entries kept as written because their key is no usable name — never
    /// followed as a path, never dropped.
    ignored_marketplaces: Extra,
}

fn decode<T: for<'de> Deserialize<'de>>(raw: &RawValue) -> Option<T> {
    serde_json::from_str(raw.get()).ok()
}

fn keep(fields: Fields) -> Extra {
    fields
        .into_iter()
        .map(|(k, v)| {
            // Unknown keys are rare and small: kept compact, so a value
            // reads back the same however the file was indented.
            let compact = serde_json::from_str::<Value>(v.get())
                .map_or_else(|_| v.get().to_string(), |value| value.to_string());
            (k, compact)
        })
        .collect()
}

impl PluginsFile {
    /// Parse a document: an empty one is empty, one that is not a JSON
    /// object (or whose sections are not objects) is an error. A record's
    /// field of the wrong type is kept as written rather than read.
    ///
    /// # Errors
    /// The document is not a registry. The caller runs with no plugins, says
    /// so, and never writes over it.
    pub fn parse(text: &str) -> Result<Self, StateParseError> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let bad = |what: &str| StateParseError(format!("plugins.json will not parse: {what}"));
        let mut fields: Fields =
            serde_json::from_str(text).map_err(|e| bad(&format!("not a JSON object ({e})")))?;
        let mut out = Self::default();
        let section =
            |fields: &mut Fields, key: &str| -> Result<BTreeMap<String, Fields>, StateParseError> {
                match fields.remove(key) {
                    None => Ok(BTreeMap::new()),
                    Some(raw) => decode(&raw)
                        .ok_or_else(|| bad(&format!("\"{key}\" is not an object of objects"))),
                }
            };
        for (name, record) in section(&mut fields, "marketplaces")? {
            if is_valid_name(&name) {
                out.marketplaces.insert(name, marketplace_record(record));
            } else {
                let text = serde_json::to_string(&keep(record)).unwrap_or_default();
                out.ignored_marketplaces.insert(name, text);
            }
        }
        for (id, record) in section(&mut fields, "plugins")? {
            out.plugins.insert(id, plugin_record(record));
        }
        for (dir, record) in section(&mut fields, "projects")? {
            let mut record = record;
            let disabled = record
                .remove("disabled")
                .and_then(|raw| decode::<Vec<String>>(&raw))
                .unwrap_or_default();
            out.projects.insert(
                dir,
                ProjectPlugins {
                    disabled,
                    extra: keep(record),
                },
            );
        }
        out.extra = keep(fields);
        Ok(out)
    }

    /// Read a document off `reader`, bounded at [`PLUGINS_FILE_MAX_BYTES`].
    ///
    /// # Errors
    /// As [`PluginsFile::parse`], or the read failed.
    pub fn read_from(reader: impl Read) -> Result<Self, StateParseError> {
        let mut text = String::new();
        reader
            .take(PLUGINS_FILE_MAX_BYTES)
            .read_to_string(&mut text)
            .map_err(|e| StateParseError(format!("plugins.json could not be read: {e}")))?;
        Self::parse(&text)
    }

    /// The document, pretty-printed, unknown keys included.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut root = extra_object(&self.extra);
        let mut marketplaces = serde_json::Map::new();
        for (name, text) in &self.ignored_marketplaces {
            let object: Extra = serde_json::from_str(text).unwrap_or_default();
            marketplaces.insert(name.clone(), Value::Object(extra_object(&object)));
        }
        for (name, record) in &self.marketplaces {
            let mut obj = extra_object(&record.extra);
            obj.insert("source".into(), Value::String(record.source.clone()));
            put(&mut obj, "ref", record.git_ref.as_ref());
            put(&mut obj, "revision", record.revision.as_ref());
            if let Some(t) = record.updated {
                obj.insert("updated".into(), Value::from(t));
            }
            marketplaces.insert(name.clone(), Value::Object(obj));
        }
        root.insert("marketplaces".into(), Value::Object(marketplaces));
        let mut plugins = serde_json::Map::new();
        for (id, record) in &self.plugins {
            let mut obj = extra_object(&record.extra);
            put(&mut obj, "version", record.version.as_ref());
            put(&mut obj, "revision", record.revision.as_ref());
            put(&mut obj, "offered", record.offered.as_ref());
            put(&mut obj, "root", record.root.as_ref());
            if let Some(t) = record.installed {
                obj.insert("installed".into(), Value::from(t));
            }
            plugins.insert(id.clone(), Value::Object(obj));
        }
        root.insert("plugins".into(), Value::Object(plugins));
        if !self.projects.is_empty() {
            let mut projects = serde_json::Map::new();
            for (dir, record) in &self.projects {
                let mut obj = extra_object(&record.extra);
                obj.insert("disabled".into(), Value::from(record.disabled.clone()));
                projects.insert(dir.clone(), Value::Object(obj));
            }
            root.insert("projects".into(), Value::Object(projects));
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Record a marketplace, keeping the unknown keys an old record carried.
    pub fn set_marketplace(&mut self, name: &str, mut record: MarketplaceRecord) {
        if let Some(old) = self.marketplaces.remove(name)
            && record.extra.is_empty()
        {
            record.extra = old.extra;
        }
        self.marketplaces.insert(name.to_string(), record);
    }

    /// Forget a marketplace (its plugins are the caller's to remove first);
    /// whether it was there.
    pub fn remove_marketplace(&mut self, name: &str) -> bool {
        self.marketplaces.remove(name).is_some()
    }

    /// Record an installed plugin, keeping the unknown keys an old record
    /// carried.
    pub fn set_plugin(&mut self, id: &PluginId, mut record: PluginRecord) {
        let key = id.to_string();
        if let Some(old) = self.plugins.remove(&key)
            && record.extra.is_empty()
        {
            record.extra = old.extra;
        }
        self.plugins.insert(key, record);
    }

    /// Forget a plugin and every directory's word about it; whether it was
    /// installed.
    pub fn remove_plugin(&mut self, id: &PluginId) -> bool {
        let key = id.to_string();
        let was = self.plugins.remove(&key).is_some();
        self.projects.retain(|_, entry| {
            entry.disabled.retain(|off| *off != key);
            !entry.disabled.is_empty() || !entry.extra.is_empty()
        });
        was
    }

    /// One plugin's record.
    #[must_use]
    pub fn plugin(&self, id: &PluginId) -> Option<&PluginRecord> {
        self.plugins.get(&id.to_string())
    }

    /// Every installed plugin whose key is an id, in id order.
    #[must_use]
    pub fn installed(&self) -> Vec<PluginId> {
        self.plugins
            .keys()
            .filter_map(|key| PluginId::parse(key).ok())
            .collect()
    }

    /// The installed plugins of one marketplace.
    #[must_use]
    pub fn plugins_of(&self, marketplace: &str) -> Vec<PluginId> {
        self.installed()
            .into_iter()
            .filter(|id| id.marketplace == marketplace)
            .collect()
    }

    /// Is `id` on in the directory `project`?
    #[must_use]
    pub fn is_enabled(&self, project: &str, id: &PluginId) -> bool {
        let key = id.to_string();
        !self
            .projects
            .get(project)
            .is_some_and(|entry| entry.disabled.contains(&key))
    }

    /// Turn `id` on or off in `project`. An emptied set drops the
    /// directory's entry, so the file stays a diff from "everything on".
    pub fn set_enabled(&mut self, project: &str, id: &PluginId, enabled: bool) {
        let key = id.to_string();
        if enabled {
            if let Some(entry) = self.projects.get_mut(project) {
                entry.disabled.retain(|off| *off != key);
                if entry.disabled.is_empty() && entry.extra.is_empty() {
                    self.projects.remove(project);
                }
            }
            return;
        }
        let entry = self.projects.entry(project.to_string()).or_default();
        if !entry.disabled.contains(&key) {
            entry.disabled.push(key);
            entry.disabled.sort();
        }
    }
}

fn put(obj: &mut serde_json::Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        obj.insert(key.into(), Value::String(value.clone()));
    }
}

fn extra_object(extra: &Extra) -> serde_json::Map<String, Value> {
    extra
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::from_str(v).unwrap_or(Value::Null)))
        .collect()
}

fn take_string(fields: &mut Fields, key: &str) -> Option<String> {
    let raw = fields.remove(key)?;
    match decode::<String>(&raw) {
        Some(s) if !s.trim().is_empty() => Some(s),
        Some(_) => None,
        None => {
            fields.insert(key.to_string(), raw);
            None
        }
    }
}

fn take_u64(fields: &mut Fields, key: &str) -> Option<u64> {
    let raw = fields.remove(key)?;
    decode::<u64>(&raw).or_else(|| {
        fields.insert(key.to_string(), raw);
        None
    })
}

fn marketplace_record(mut fields: Fields) -> MarketplaceRecord {
    MarketplaceRecord {
        source: take_string(&mut fields, "source").unwrap_or_default(),
        git_ref: take_string(&mut fields, "ref"),
        revision: take_string(&mut fields, "revision"),
        updated: take_u64(&mut fields, "updated"),
        extra: keep(fields),
    }
}

fn plugin_record(mut fields: Fields) -> PluginRecord {
    PluginRecord {
        version: take_string(&mut fields, "version"),
        revision: take_string(&mut fields, "revision"),
        offered: take_string(&mut fields, "offered"),
        root: take_string(&mut fields, "root"),
        installed: take_u64(&mut fields, "installed"),
        extra: keep(fields),
    }
}

/// Apply `change` to the registry text `existing` — the file as it is on
/// disk at that moment — and return the text to write back.
///
/// # Errors
/// `existing` will not parse: nothing is to be written.
pub fn update_state(
    existing: &str,
    change: impl FnOnce(&mut PluginsFile),
) -> Result<String, StateParseError> {
    let mut file = PluginsFile::parse(existing)?;
    change(&mut file);
    Ok(file.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{
  "marketplaces": {
    "claude-plugins-official": {
      "source": "anthropics/claude-plugins-official",
      "revision": "d7d3279",
      "updated": 1795180000
    },
    "team": { "source": "/home/me/dev/team-plugins" }
  },
  "plugins": {
    "frontend-design@claude-plugins-official": {
      "version": "1.0.0",
      "revision": "9c1e0b4",
      "installed": 1795180100
    },
    "lint@team": { "root": "/home/me/dev/team-plugins/lint" }
  },
  "projects": { "/home/me/work/api": { "disabled": ["lint@team"] } }
}"#;

    fn id(text: &str) -> PluginId {
        PluginId::parse(text).unwrap()
    }

    #[test]
    fn the_documented_example_reads_back() {
        let file = PluginsFile::parse(EXAMPLE).unwrap();
        let official = &file.marketplaces["claude-plugins-official"];
        assert_eq!(official.source, "anthropics/claude-plugins-official");
        assert_eq!(official.updated, Some(1_795_180_000));
        let record = file
            .plugin(&id("frontend-design@claude-plugins-official"))
            .unwrap();
        assert_eq!(record.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            file.plugin(&id("lint@team")).unwrap().root.as_deref(),
            Some("/home/me/dev/team-plugins/lint")
        );
        assert!(!file.is_enabled("/home/me/work/api", &id("lint@team")));
        assert!(file.is_enabled("/elsewhere", &id("lint@team")));
        assert_eq!(PluginsFile::parse(&file.to_json()).unwrap(), file);
    }

    #[test]
    fn a_round_trip_keeps_unknown_keys_at_every_level() {
        let text = r#"{"future": [1, 2],
            "marketplaces": {"m": {"source": "o/r", "autoUpdate": true}},
            "plugins": {"p@m": {"pinned": "x"}},
            "projects": {"/d": {"disabled": ["p@m"], "note": 1}}}"#;
        let file = PluginsFile::parse(text).unwrap();
        let json = file.to_json();
        for key in [
            "\"future\"",
            "\"autoUpdate\": true",
            "\"pinned\": \"x\"",
            "\"note\": 1",
        ] {
            assert!(json.contains(key), "{key}: {json}");
        }
        assert_eq!(PluginsFile::parse(&json).unwrap(), file);
    }

    #[test]
    fn a_replaced_record_keeps_the_unknown_keys_of_the_old_one() {
        let mut file = PluginsFile::parse(r#"{"plugins": {"p@m": {"pinned": "x"}}}"#).unwrap();
        file.set_plugin(&id("p@m"), PluginRecord::default());
        assert!(file.to_json().contains("\"pinned\""));
    }

    #[test]
    fn a_registry_that_will_not_parse_is_an_error_and_never_rewritten() {
        for bad in [
            "{oops",
            "[]",
            r#"{"plugins": []}"#,
            r#"{"marketplaces": {"m": 3}}"#,
        ] {
            assert!(PluginsFile::parse(bad).is_err(), "{bad}");
            assert!(update_state(bad, |_| {}).is_err(), "{bad}");
        }
        assert_eq!(PluginsFile::parse("  ").unwrap(), PluginsFile::default());
    }

    #[test]
    fn a_marketplace_key_that_is_no_plain_name_is_kept_but_never_followed() {
        let file = PluginsFile::parse(
            r#"{"marketplaces": {"../../.ssh": {"source": "x"}, "ok": {"source": "y"}}}"#,
        )
        .unwrap();
        assert_eq!(file.marketplaces.keys().collect::<Vec<_>>(), ["ok"]);
        assert!(file.to_json().contains("../../.ssh"));
    }

    #[test]
    fn wrongly_typed_fields_are_kept_as_written_rather_than_read() {
        let file = PluginsFile::parse(r#"{"plugins": {"p@m": {"version": 3}}}"#).unwrap();
        assert_eq!(file.plugin(&id("p@m")).unwrap().version, None);
        assert!(file.to_json().contains("\"version\": 3"));
    }

    #[test]
    fn on_and_off_is_per_directory_and_an_emptied_set_drops_its_entry() {
        let mut file = PluginsFile::default();
        let lint = id("lint@team");
        file.set_enabled("/a", &lint, false);
        file.set_enabled("/a", &lint, false);
        assert_eq!(file.projects["/a"].disabled, ["lint@team"]);
        assert!(!file.is_enabled("/a", &lint));
        assert!(file.is_enabled("/b", &lint));
        file.set_enabled("/a", &lint, true);
        assert!(file.projects.is_empty());
        assert!(!file.to_json().contains("projects"));
    }

    #[test]
    fn removing_a_plugin_forgets_every_directorys_word_about_it() {
        let mut file = PluginsFile::default();
        let lint = id("lint@team");
        file.set_plugin(&lint, PluginRecord::default());
        file.set_enabled("/a", &lint, false);
        assert!(file.remove_plugin(&lint));
        assert!(file.projects.is_empty());
        assert!(!file.remove_plugin(&lint));
    }

    #[test]
    fn the_plugins_of_a_marketplace_are_listed() {
        let mut file = PluginsFile::default();
        for key in ["a@m", "b@m", "c@other"] {
            file.set_plugin(&id(key), PluginRecord::default());
        }
        assert_eq!(file.plugins_of("m"), [id("a@m"), id("b@m")]);
    }

    #[test]
    fn ids_split_on_the_last_at() {
        let parsed = id("a.b@market");
        assert_eq!(
            (parsed.name.as_str(), parsed.marketplace.as_str()),
            ("a.b", "market")
        );
        assert_eq!(parsed.to_string(), "a.b@market");
        for bad in ["x@y@z", "plain", "@m", "p@", "../x@m"] {
            assert!(PluginId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_spec_names_a_plugin_and_maybe_its_marketplace() {
        let bare = PluginSpec::parse("figma").unwrap();
        assert_eq!((bare.name.as_str(), bare.marketplace), ("figma", None));
        let full = PluginSpec::parse("figma@openai-curated").unwrap();
        assert_eq!(full.marketplace.as_deref(), Some("openai-curated"));
        assert!(PluginSpec::parse("../x").is_err());
        assert!(PluginSpec::parse("").is_err());
    }

    #[test]
    fn names_are_one_safe_path_segment() {
        for ok in ["a", "Frontend_Design", "acme.tools", "x-1", "9lives"] {
            assert!(is_valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".x",
            "-x",
            "_x",
            "a..b",
            "a/b",
            "a\\b",
            "a b",
            "a@b",
            "a:b",
            "é",
            &"x".repeat(65),
        ] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_defaults_are_the_two_official_marketplaces_unless_the_environment_says_otherwise() {
        let defaults = default_marketplaces(None);
        assert_eq!(defaults.len(), 2);
        assert_eq!(defaults[0].name.as_deref(), Some("claude-plugins-official"));
        assert_eq!(defaults[1].source, "openai/plugins");
        assert!(default_marketplaces(Some("")).is_empty());
        assert_eq!(
            default_marketplaces(Some(" /srv/a , team=file:///srv/b.git ,")),
            vec![
                DefaultMarketplace {
                    name: None,
                    source: "/srv/a".into()
                },
                DefaultMarketplace {
                    name: Some("team".into()),
                    source: "file:///srv/b.git".into()
                },
            ]
        );
    }
}

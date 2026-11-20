//! Marketplace catalogs (`docs/plugins.md` *Marketplaces*): the
//! `marketplace.json` both references write, each entry's `source`, the git
//! URL spellings a source may use, and the names reserved for the official
//! catalogs.
//!
//! A catalog can list hundreds of plugins (Anthropic's lists over three
//! hundred in 192 KB), so it is **streamed**: [`read_catalog`] runs
//! `serde_json::from_reader` over a [`Read::take`] cap into typed records
//! naming only the fields `/plugins` shows — every field read leniently, so
//! a wrong type costs that field, not the catalog — and the `plugins` array
//! is taken one entry at a time, each entry's text held only while it is
//! decoded. No `Value` tree of the document is ever built, and no buffer of
//! the whole body (`docs/memory.md` *The rule*).
//!
//! A catalog's `ref`, `sha`, `url` and subdirectory reach `git` as
//! arguments, so each is read as what it must be — an option-shaped one is
//! refused — and the entry that fails is skipped with the reason.

use std::fmt;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

use super::manifest::PluginMetadata;
use super::source::{github_shorthand, github_url, is_plain_url_path, url_path};
use super::state::validate_name;
use super::text::{cleaned, cleaned_opt, cleaned_paragraphs};

/// Where a marketplace root's catalog is looked for, first found wins.
pub const MARKETPLACE_MANIFESTS: [&str; 3] = [
    ".agents/plugins/marketplace.json",
    ".claude-plugin/marketplace.json",
    ".cursor-plugin/marketplace.json",
];
/// The most of a catalog file read.
pub const CATALOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// The most entries of one catalog read; the rest are one notice. The
/// official catalogs list a few hundred.
pub const CATALOG_MAX_ENTRIES: usize = 5_000;
/// The most notices one catalog keeps; the rest are counted in one more.
pub const CATALOG_MAX_NOTICES: usize = 100;
/// Where an entry's description is cut.
pub const DESCRIPTION_MAX_CHARS: usize = 2_000;

/// The marketplace names Claude Code reserves for Anthropic's catalogs: only
/// `github.com/anthropics/` may serve one.
pub const ANTHROPIC_RESERVED: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "anthropic-agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
    "claude-for-legal",
    "claude-for-financial-services",
    "financial-services-plugins",
    "first-party-plugins",
    "claude-tag-plugins",
    "claude-community",
    "claude-plugins-community",
    "healthcare",
    "anthropic-plugin-directory",
    "claude-plugin-directory",
];

/// The marketplace names codex reserves for OpenAI's catalogs: only
/// `github.com/openai/` may serve one.
pub const OPENAI_RESERVED: &[&str] = &[
    "openai-curated",
    "openai-api-curated",
    "openai-bundled",
    "openai-bundled-alpha",
    "openai-primary-runtime",
];

/// The component keys of an entry that stands in for a manifest, and the
/// ones this client names as not used.
const MANIFEST_KEYS: [&str; 9] = [
    "skills",
    "commands",
    "agents",
    "hooks",
    "mcpServers",
    "lspServers",
    "outputStyles",
    "userConfig",
    "channels",
];

/// Where an entry's plugin comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A directory of the marketplace, `/`-joined and clean (`""` is the
    /// marketplace root itself).
    Local {
        /// The path under the marketplace root.
        path: String,
    },
    /// `{"source": "github", "repo": "owner/repo"}`.
    GitHub {
        /// `owner`.
        owner: String,
        /// `repo`.
        repo: String,
        /// The branch or tag.
        git_ref: Option<String>,
        /// The pinned commit.
        sha: Option<String>,
    },
    /// `url` (optionally a subdirectory) or `git-subdir`.
    Git {
        /// The repository.
        url: String,
        /// A subdirectory of it.
        path: Option<String>,
        /// The branch or tag.
        git_ref: Option<String>,
        /// The pinned commit.
        sha: Option<String>,
    },
}

impl PluginSource {
    /// The clone URL of a git source (`None` for a local one), GitHub repos
    /// resolved against `github_base`.
    #[must_use]
    pub fn git_url(&self, github_base: &str) -> Option<String> {
        match self {
            Self::Local { .. } => None,
            Self::GitHub { owner, repo, .. } => Some(github_url(github_base, owner, repo)),
            Self::Git { url, .. } => Some(url.clone()),
        }
    }

    /// The pinned commit, when there is one.
    #[must_use]
    pub fn sha(&self) -> Option<&str> {
        match self {
            Self::Local { .. } => None,
            Self::GitHub { sha, .. } | Self::Git { sha, .. } => sha.as_deref(),
        }
    }

    /// The source in one line.
    #[must_use]
    pub fn label(&self) -> String {
        let pin = |git_ref: &Option<String>, sha: &Option<String>| match (git_ref, sha) {
            (_, Some(sha)) => format!(" @ {}", &sha[..sha.len().min(12)]),
            (Some(r), None) => format!(" @ {r}"),
            (None, None) => String::new(),
        };
        match self {
            Self::Local { path } if path.is_empty() => "the marketplace itself".into(),
            Self::Local { path } => format!("./{path}"),
            Self::GitHub {
                owner,
                repo,
                git_ref,
                sha,
            } => format!("{owner}/{repo}{}", pin(git_ref, sha)),
            Self::Git {
                url,
                path,
                git_ref,
                sha,
            } => {
                let sub = path.as_ref().map(|p| format!(" · {p}")).unwrap_or_default();
                format!("{url}{sub}{}", pin(git_ref, sha))
            }
        }
    }
}

/// One plugin a catalog lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The entry's name — the first half of the plugin's id.
    pub name: String,
    /// Where it comes from; `None` when it cannot be installed here.
    pub source: Option<PluginSource>,
    /// Why it cannot be installed here (an unsupported source, a policy).
    pub unavailable: Option<String>,
    /// `displayName` / `interface.displayName`.
    pub display_name: Option<String>,
    /// `description` / `interface.shortDescription`, cut at
    /// [`DESCRIPTION_MAX_CHARS`].
    pub description: Option<String>,
    /// `version`.
    pub version: Option<String>,
    /// `author.name` (or `author` as a string).
    pub author: Option<String>,
    /// `category`.
    pub category: Option<String>,
    /// `homepage`.
    pub homepage: Option<String>,
    /// `keywords` and `tags`.
    pub keywords: Vec<String>,
    /// The entry's component keys as one JSON object's text — the manifest
    /// of a plugin directory that has none. Kept only when present.
    pub manifest_fields: Option<String>,
}

impl CatalogEntry {
    /// The entry as a manifest document — its display fields and its
    /// component keys — what is saved beside an installed copy.
    #[must_use]
    pub fn manifest_text(&self) -> String {
        let mut object = match &self.manifest_fields {
            Some(text) => serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text)
                .unwrap_or_default(),
            None => serde_json::Map::new(),
        };
        object.insert("name".into(), self.name.clone().into());
        for (key, value) in [
            ("displayName", &self.display_name),
            ("description", &self.description),
            ("version", &self.version),
            ("category", &self.category),
            ("homepage", &self.homepage),
            ("author", &self.author),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), value.clone().into());
            }
        }
        if !self.keywords.is_empty() {
            object.insert("keywords".into(), self.keywords.clone().into());
        }
        serde_json::Value::Object(object).to_string()
    }

    /// Does the entry declare components of its own?
    #[must_use]
    pub fn declares_components(&self) -> bool {
        self.manifest_fields.is_some()
    }
}

/// A catalog, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// The marketplace's own name — its identity, never the URL's.
    pub name: String,
    /// `interface.displayName`.
    pub display_name: Option<String>,
    /// `description` / `metadata.description`.
    pub description: Option<String>,
    /// `owner.name`.
    pub owner: Option<String>,
    /// The entries that could be read, in catalog order.
    pub entries: Vec<CatalogEntry>,
    /// The entries skipped, each with its reason.
    pub notices: Vec<String>,
}

impl Catalog {
    /// The entry named `name`.
    #[must_use]
    pub fn entry(&self, name: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

/// Why a catalog cannot be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogError {}

// ===== lenient field types =====

/// A string, or nothing when the field holds anything else.
#[derive(Debug, Default)]
struct Text(Option<String>);

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
                let v = v.trim();
                Ok(Text((!v.is_empty()).then(|| v.to_string())))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Text, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// A list of strings (other items skipped), or nothing.
#[derive(Debug, Default)]
struct Texts(Vec<String>);

impl<'de> Deserialize<'de> for Texts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Texts;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Texts, A::Error> {
                let mut out = Vec::new();
                while let Some(item) = seq.next_element::<Text>()? {
                    out.extend(item.0);
                }
                Ok(Texts(out))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Texts, E> {
                Ok(Texts(Vec::new()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Texts, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Texts(Vec::new()))
            }
        }
        d.deserialize_any(V)
    }
}

/// An object's `name` string (or the value itself when it is a string), or
/// nothing — `owner`, `author`.
#[derive(Debug, Default)]
struct Person(Option<String>);

impl<'de> Deserialize<'de> for Person {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Person;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Person, A::Error> {
                let mut name = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "name" {
                        name = map.next_value::<Text>()?.0;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Person(name))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Person, E> {
                Ok(Person(Text::visit(v)))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Person, E> {
                Ok(Person(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Person, E> {
                Ok(Person(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Person, E> {
                Ok(Person(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Person, E> {
                Ok(Person(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Person, E> {
                Ok(Person(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Person, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Person(None))
            }
        }
        d.deserialize_any(V)
    }
}

impl Text {
    fn visit(v: &str) -> Option<String> {
        let v = v.trim();
        (!v.is_empty()).then(|| v.to_string())
    }
}

/// The sub-object fields a catalog's top level is read for.
#[derive(Debug, Default)]
struct Nested {
    display_name: Option<String>,
    description: Option<String>,
    plugin_root: Option<String>,
    installation: Option<String>,
}

impl<'de> Deserialize<'de> for Nested {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Nested;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Nested, A::Error> {
                let mut out = Nested::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "displayName" => out.display_name = map.next_value::<Text>()?.0,
                        "description" | "shortDescription" => {
                            let text = map.next_value::<Text>()?.0;
                            if out.description.is_none() {
                                out.description = text;
                            }
                        }
                        "pluginRoot" => out.plugin_root = map.next_value::<Text>()?.0,
                        "installation" => out.installation = map.next_value::<Text>()?.0,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(out)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_unit<E: de::Error>(self) -> Result<Nested, E> {
                Ok(Nested::default())
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Nested, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Nested::default())
            }
        }
        d.deserialize_any(V)
    }
}

/// One entry as the catalog wrote it, its source not yet resolved (the
/// marketplace's `pluginRoot` may come after `plugins`).
#[derive(Debug)]
struct PendingEntry {
    entry: CatalogEntry,
    source: Result<RawSource, String>,
}

#[derive(Debug)]
enum RawSource {
    Path(String),
    Object {
        kind: String,
        path: Option<String>,
        url: Option<String>,
        repo: Option<String>,
        git_ref: Option<String>,
        sha: Option<String>,
    },
}

#[derive(Deserialize)]
struct RawEntry<'a> {
    #[serde(default)]
    name: Text,
    #[serde(borrow, default)]
    source: Option<&'a RawValue>,
    #[serde(default)]
    description: Text,
    #[serde(default)]
    version: Text,
    #[serde(default)]
    category: Text,
    #[serde(default)]
    homepage: Text,
    #[serde(default, rename = "displayName")]
    display_name: Text,
    #[serde(default)]
    author: Person,
    #[serde(default)]
    keywords: Texts,
    #[serde(default)]
    tags: Texts,
    #[serde(default)]
    interface: Nested,
    #[serde(default)]
    policy: Nested,
}

#[derive(Deserialize)]
struct RawSourceObject {
    #[serde(default)]
    source: Text,
    #[serde(default)]
    path: Text,
    #[serde(default)]
    url: Text,
    #[serde(default)]
    repo: Text,
    #[serde(default, rename = "ref")]
    git_ref: Text,
    #[serde(default)]
    sha: Text,
}

fn cut(text: Option<String>) -> Option<String> {
    text.map(|t| match t.char_indices().nth(DESCRIPTION_MAX_CHARS) {
        Some((at, _)) => format!("{}…", &t[..at]),
        None => t,
    })
}

/// Decode one entry's text; `Err` is the reason it is skipped.
fn pending_entry(text: &str) -> Result<PendingEntry, String> {
    let raw: RawEntry<'_> =
        serde_json::from_str(text).map_err(|_| "it is not an object".to_string())?;
    let name = raw.name.0.ok_or("it has no \"name\"")?;
    validate_name(&name).map_err(|why| format!("\"{name}\" is not a usable plugin name: {why}"))?;
    let source = match raw.source {
        None => Err(format!("\"{name}\" has no \"source\"")),
        Some(source) => raw_source(source).map_err(|why| format!("\"{name}\": {why}")),
    };
    let fields: Vec<(String, &RawValue)> =
        super::agent_plugins::ordered_members(text).unwrap_or_default();
    let declared: Vec<String> = fields
        .iter()
        .filter(|(key, value)| MANIFEST_KEYS.contains(&key.as_str()) && value.get() != "null")
        .map(|(key, value)| {
            format!(
                "{}:{}",
                serde_json::to_string(key).unwrap_or_default(),
                value.get()
            )
        })
        .collect();
    let mut keywords = raw.keywords.0;
    keywords.extend(raw.tags.0);
    let keywords = keywords
        .into_iter()
        .map(cleaned)
        .filter(|k| !k.trim().is_empty())
        .collect();
    let unavailable = (raw.policy.installation.as_deref() == Some("NOT_AVAILABLE"))
        .then(|| "the marketplace marks it not available".to_string());
    Ok(PendingEntry {
        entry: CatalogEntry {
            name,
            source: None,
            unavailable,
            display_name: cleaned_opt(raw.display_name.0.or(raw.interface.display_name)),
            description: cut(cleaned_paragraphs(
                raw.description.0.or(raw.interface.description),
            )),
            version: cleaned_opt(raw.version.0),
            author: cleaned_opt(raw.author.0),
            category: cleaned_opt(raw.category.0),
            homepage: cleaned_opt(raw.homepage.0),
            keywords,
            manifest_fields: (!declared.is_empty()).then(|| format!("{{{}}}", declared.join(","))),
        },
        source,
    })
}

fn raw_source(raw: &RawValue) -> Result<RawSource, String> {
    if let Ok(path) = serde_json::from_str::<String>(raw.get()) {
        return Ok(RawSource::Path(path));
    }
    let object: RawSourceObject = serde_json::from_str(raw.get())
        .map_err(|_| "its \"source\" must be a path or an object".to_string())?;
    if !raw.get().trim_start().starts_with('{') {
        return Err("its \"source\" must be a path or an object".into());
    }
    Ok(RawSource::Object {
        kind: object.source.0.ok_or("its \"source\" names no type")?,
        path: object.path.0,
        url: object.url.0,
        repo: object.repo.0,
        git_ref: object.git_ref.0,
        sha: object.sha.0,
    })
}

/// A relative path as clean `/`-joined components.
///
/// # Errors
/// A backslash, an absolute path, or a `..`.
pub fn clean_relative(path: &str) -> Result<String, String> {
    if path.contains('\\') {
        return Err("a path must use forward slashes".into());
    }
    if path.starts_with('/') {
        return Err("a path must be relative".into());
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("a path must not contain \"..\"".into()),
            part if part.starts_with('-') => {
                return Err(format!(
                    "a path segment may not start with '-' (\"{part}\")"
                ));
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

/// A `ref` as git reads it: a branch or tag name, never an option.
///
/// # Errors
/// Anything that is not a plain ref name.
pub fn checked_ref(name: Option<String>) -> Result<Option<String>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '@' | '+'));
    if !plain || name.starts_with(['-', '+', '/']) || name.contains("..") || name.contains("@{") {
        return Err(format!("\"ref\" \"{name}\" is not a branch or tag name"));
    }
    Ok(Some(name))
}

/// A pinned `sha`: the whole 40-digit commit id, lowercased. A prefix is
/// refused — the plugin repository's owner can push a second commit that
/// starts with it.
///
/// # Errors
/// Anything else.
pub fn checked_sha(sha: Option<String>) -> Result<Option<String>, String> {
    let Some(sha) = sha else {
        return Ok(None);
    };
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("\"sha\" \"{sha}\" is not a commit id"));
    }
    Ok(Some(sha.to_ascii_lowercase()))
}

/// A catalog's git URL as git may be handed it: `https`, `http`, `ssh`,
/// `git` or `file` URLs, `user@host:path`, an absolute path, or `owner/repo`
/// (GitHub). Never an option, never a transport helper (`ext::…`).
///
/// # Errors
/// Anything else.
pub fn normalize_git_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let invalid = || format!("\"{url}\" is not a git repository address");
    if url.is_empty()
        || url.starts_with('-')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    if (url.contains("://") || url.contains(':') || url.starts_with('/'))
        && !is_plain_url_path(url_path(url).unwrap_or(url))
    {
        return Err(format!(
            "\"{url}\" names its repository with a \".\", \"..\" or empty path segment, or a %-escape"
        ));
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git" | "file") || rest.is_empty() {
            return Err(invalid());
        }
        return Ok(url.trim_end_matches('/').to_string());
    }
    if let Some((host, path)) = url.split_once(':') {
        if host.contains('@') && !host.contains('/') && !path.is_empty() && !path.starts_with(':') {
            return Ok(url.to_string());
        }
        return Err(invalid());
    }
    if url.starts_with('/') {
        return Ok(url.to_string());
    }
    github_shorthand(url)
        .map(|(owner, repo)| {
            github_url(
                super::source::GITHUB_BASE,
                owner,
                repo.trim_end_matches(".git"),
            )
        })
        .ok_or_else(invalid)
}

fn local_source(path: &str, plugin_root: Option<&str>) -> Result<PluginSource, String> {
    let path = path.trim();
    let clean = match path {
        "" => return Err("its source path is empty".into()),
        "." | "./" => String::new(),
        path if path.starts_with("./") => clean_relative(&path[2..])?,
        path if !path.contains(['/', '\\']) && path != ".." => match plugin_root {
            Some("") => clean_relative(path)?,
            Some(root) => clean_relative(&format!("{root}/{path}"))?,
            None => {
                return Err(format!(
                    "source path \"{path}\" must start with ./ (a bare name needs metadata.pluginRoot)"
                ));
            }
        },
        path => return Err(format!("source path \"{path}\" must start with ./")),
    };
    Ok(PluginSource::Local { path: clean })
}

/// Resolve an entry's source; `Ok(Err(reason))` is an entry that is listed
/// but cannot be installed here, `Err` one that is skipped.
fn resolve_source(
    raw: RawSource,
    plugin_root: Option<&str>,
) -> Result<Result<PluginSource, String>, String> {
    let (kind, path, url, repo, git_ref, sha) = match raw {
        RawSource::Path(path) => return local_source(&path, plugin_root).map(Ok),
        RawSource::Object {
            kind,
            path,
            url,
            repo,
            git_ref,
            sha,
        } => (kind, path, url, repo, git_ref, sha),
    };
    if let Some(pin) = &sha
        && pin.len() < 40
        && pin.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Ok(Err(format!(
            "its pinned \"sha\" \"{pin}\" is a prefix; only a full 40-digit commit id is trusted"
        )));
    }
    let subdir = |required: bool| -> Result<Option<String>, String> {
        match &path {
            Some(path) => {
                let clean = clean_relative(path.strip_prefix("./").unwrap_or(path))?;
                if clean.is_empty() {
                    return Err("its \"path\" is empty".into());
                }
                Ok(Some(clean))
            }
            None if required => Err("a git-subdir source needs a \"path\"".into()),
            None => Ok(None),
        }
    };
    Ok(Ok(match kind.as_str() {
        "local" => {
            return local_source(
                path.as_deref().ok_or("a local source needs a \"path\"")?,
                plugin_root,
            )
            .map(Ok);
        }
        "github" => {
            let repo = repo.ok_or("a github source needs a \"repo\"")?;
            let (owner, name) = github_shorthand(&repo)
                .filter(|(owner, _)| !owner.starts_with('-'))
                .ok_or_else(|| format!("\"repo\" \"{repo}\" is not owner/repo"))?;
            PluginSource::GitHub {
                owner: owner.to_string(),
                repo: name.trim_end_matches(".git").to_string(),
                git_ref: checked_ref(git_ref)?,
                sha: checked_sha(sha)?,
            }
        }
        "url" | "git-subdir" => PluginSource::Git {
            url: normalize_git_url(
                url.as_deref()
                    .ok_or_else(|| format!("a {kind} source needs a \"url\""))?,
            )?,
            path: subdir(kind == "git-subdir")?,
            git_ref: checked_ref(git_ref)?,
            sha: checked_sha(sha)?,
        },
        "npm" => return Ok(Err("npm sources are not supported".into())),
        "archive" => return Ok(Err("archive sources are not supported".into())),
        "command" => {
            return Ok(Err(
                "a command source would run a shell command to find the plugin, which this client never does"
                    .into(),
            ));
        }
        other => return Ok(Err(format!("source type \"{other}\" is not supported"))),
    }))
}

/// What a catalog says about itself — everything but its entries, which
/// [`stream_catalog`] hands over one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogHeader {
    /// The marketplace's own name.
    pub name: String,
    /// `interface.displayName`.
    pub display_name: Option<String>,
    /// `description` / `metadata.description`.
    pub description: Option<String>,
    /// `owner.name`.
    pub owner: Option<String>,
    /// The entries skipped, each with its reason.
    pub notices: Vec<String>,
}

/// A source path that only `metadata.pluginRoot` can resolve — a bare name.
fn needs_plugin_root(source: &Result<RawSource, String>) -> bool {
    let path = match source {
        Ok(RawSource::Path(path)) => path,
        Ok(RawSource::Object {
            kind,
            path: Some(path),
            ..
        }) if kind == "local" => path,
        _ => return false,
    };
    let path = path.trim();
    !path.is_empty()
        && !matches!(path, "." | "./" | "..")
        && !path.starts_with("./")
        && !path.contains(['/', '\\'])
}

/// The state one pass over a catalog keeps: its top-level fields as they
/// are met, the names handed over (for duplicates), and the few entries
/// that wait for a `pluginRoot` named after the list.
///
/// Bounded whatever the catalog says: [`CATALOG_MAX_ENTRIES`] elements
/// read, [`CATALOG_MAX_NOTICES`] notices kept, and a duplicate found in
/// constant time — a hash of each case-folded name, confirmed against the
/// names buffer only on a hit — since a catalog of a hundred thousand
/// entries is a few megabytes and the page reads catalogs on the UI loop.
struct Stream<'f> {
    on_entry: &'f mut dyn FnMut(usize, CatalogEntry),
    name: Option<String>,
    description: Option<String>,
    owner: Option<String>,
    interface: Nested,
    metadata: Option<Nested>,
    plugins_seen: bool,
    /// Every name taken, `\n`-separated — one buffer, not a set of strings.
    names: String,
    /// The case-folded hash of every name in `names`.
    hashes: std::collections::HashSet<u64>,
    waiting: Vec<(usize, PendingEntry)>,
    notices: Vec<String>,
    /// Notices past [`CATALOG_MAX_NOTICES`], counted.
    notices_dropped: usize,
    /// Elements past [`CATALOG_MAX_ENTRIES`], counted and never decoded.
    entries_dropped: usize,
}

/// A name's hash with ASCII case folded (names are ASCII): `Lint` and
/// `lint` are one directory on a case-insensitive filesystem.
fn folded_hash(name: &str) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for b in name.bytes() {
        hasher.write_u8(b.to_ascii_lowercase());
    }
    hasher.finish()
}

impl Stream<'_> {
    fn plugin_root(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.plugin_root.as_deref())
    }

    /// The name already handed over that `name` is a spelling of, if any.
    fn taken(&self, name: &str) -> Option<&str> {
        if !self.hashes.contains(&folded_hash(name)) {
            return None;
        }
        self.names
            .split('\n')
            .find(|taken| taken.eq_ignore_ascii_case(name))
    }

    /// Keep a notice, or count it once [`CATALOG_MAX_NOTICES`] are kept.
    fn notice(&mut self, make: impl FnOnce() -> String) {
        if self.notices.len() < CATALOG_MAX_NOTICES {
            self.notices.push(make());
        } else {
            self.notices_dropped += 1;
        }
    }

    /// The counts of what was not kept, one notice each.
    fn close_notices(&mut self) {
        if self.notices_dropped > 0 {
            self.notices
                .push(format!("… and {} more", self.notices_dropped));
        }
        if self.entries_dropped > 0 {
            self.notices.push(format!(
                "{} more entries were not read: a catalog lists at most {CATALOG_MAX_ENTRIES}",
                self.entries_dropped
            ));
        }
    }

    /// One element of `plugins`, at `index`: handed over, skipped with its
    /// reason, or — a bare source with no `pluginRoot` met yet — kept until
    /// the document ends.
    fn element(&mut self, index: usize, text: &str) {
        let pending = match pending_entry(text) {
            Ok(pending) => pending,
            Err(why) => {
                self.notice(|| format!("entry {} skipped: {why}", index + 1));
                return;
            }
        };
        if let Some(first) = self.taken(&pending.entry.name) {
            let message = if first == pending.entry.name {
                format!("plugin \"{first}\" is listed twice; the first entry is used",)
            } else {
                format!(
                    "plugin \"{}\" is listed again as \"{first}\" in another case; the first entry is used",
                    pending.entry.name
                )
            };
            self.notice(|| message);
            return;
        }
        if !self.names.is_empty() {
            self.names.push('\n');
        }
        self.names.push_str(&pending.entry.name);
        self.hashes.insert(folded_hash(&pending.entry.name));
        if self.metadata.is_none() && needs_plugin_root(&pending.source) {
            self.waiting.push((index, pending));
        } else {
            self.resolve(index, pending);
        }
    }

    /// Resolve an entry's source against the `pluginRoot` known now, and
    /// hand it over — or skip it, saying why.
    fn resolve(&mut self, index: usize, pending: PendingEntry) {
        let PendingEntry { mut entry, source } = pending;
        let root = self.plugin_root().map(str::to_string);
        match source.and_then(|raw| resolve_source(raw, root.as_deref())) {
            Ok(Ok(source)) => entry.source = Some(source),
            Ok(Err(reason)) => entry.unavailable = Some(reason),
            Err(why) => {
                self.notice(|| format!("entry \"{}\" skipped: {why}", entry.name));
                return;
            }
        }
        (self.on_entry)(index, entry);
    }
}

/// The document's top level, read into a [`Stream`].
struct Top<'s, 'f>(&'s mut Stream<'f>);

impl<'de> de::DeserializeSeed<'de> for Top<'_, '_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Top<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a marketplace catalog object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let stream = self.0;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => stream.name = map.next_value::<Text>()?.0,
                "description" => stream.description = map.next_value::<Text>()?.0,
                "owner" => stream.owner = map.next_value::<Person>()?.0,
                "interface" => stream.interface = map.next_value()?,
                "metadata" => stream.metadata = Some(map.next_value()?),
                "plugins" if stream.plugins_seen => {
                    return Err(de::Error::duplicate_field("plugins"));
                }
                "plugins" => map.next_value_seed(Plugins(&mut *stream))?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// The `plugins` list, each element handed to the [`Stream`] as it is read.
struct Plugins<'s, 'f>(&'s mut Stream<'f>);

impl<'de> de::DeserializeSeed<'de> for Plugins<'_, '_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Plugins<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of plugins")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let stream = self.0;
        stream.plugins_seen = true;
        let mut index = 0;
        while index < CATALOG_MAX_ENTRIES {
            let Some(raw) = seq.next_element::<Box<RawValue>>()? else {
                return Ok(());
            };
            stream.element(index, raw.get());
            index += 1;
        }
        // Past the cap the rest is skipped unread, only counted.
        while seq.next_element::<IgnoredAny>()?.is_some() {
            stream.entries_dropped += 1;
        }
        Ok(())
    }
}

/// Read a catalog off `reader`, bounded at [`CATALOG_MAX_BYTES`], handing
/// each usable entry to `on_entry` — with its index in the `plugins` list —
/// **as it is read**, and returning the catalog's header. Each entry is
/// decoded and validated by [`read_catalog`]'s rules (which is this, its
/// entries collected), so no more of the catalog than one entry is held at
/// a time; the exception is an entry whose bare source names a directory
/// under a `metadata.pluginRoot` the document has not reached yet, handed
/// over once the document ends.
///
/// On an error, whatever was handed over belongs to a catalog that is not
/// valid: the caller drops it.
///
/// # Errors
/// Not JSON, no usable `name`, no `plugins` list, or a read past the cap.
pub fn stream_catalog(
    reader: impl Read,
    on_entry: &mut dyn FnMut(usize, CatalogEntry),
) -> Result<CatalogHeader, CatalogError> {
    let mut stream = Stream {
        on_entry,
        name: None,
        description: None,
        owner: None,
        interface: Nested::default(),
        metadata: None,
        plugins_seen: false,
        names: String::new(),
        hashes: std::collections::HashSet::new(),
        waiting: Vec::new(),
        notices: Vec::new(),
        notices_dropped: 0,
        entries_dropped: 0,
    };
    let bounded = BufReader::new(reader.take(CATALOG_MAX_BYTES));
    let mut de = serde_json::Deserializer::from_reader(bounded);
    de::DeserializeSeed::deserialize(Top(&mut stream), &mut de)
        .and_then(|()| de.end())
        .map_err(|e| CatalogError(format!("the catalog is not valid: {e}")))?;
    let name = stream
        .name
        .take()
        .ok_or_else(|| CatalogError("the catalog has no \"name\"".into()))?;
    validate_name(&name).map_err(|why| {
        CatalogError(format!(
            "the catalog's name \"{name}\" is not usable: {why}"
        ))
    })?;
    if !stream.plugins_seen {
        return Err(CatalogError("the catalog has no \"plugins\" list".into()));
    }
    for (index, pending) in std::mem::take(&mut stream.waiting) {
        stream.resolve(index, pending);
    }
    let metadata = stream.metadata.take().unwrap_or_default();
    stream.close_notices();
    Ok(CatalogHeader {
        name,
        display_name: cleaned_opt(stream.interface.display_name.take()),
        description: cleaned_paragraphs(stream.description.take().or(metadata.description)),
        owner: cleaned_opt(stream.owner.take()),
        notices: std::mem::take(&mut stream.notices),
    })
}

/// [`stream_catalog`] over a file, the file named in the error.
///
/// # Errors
/// The file cannot be opened, or [`stream_catalog`]'s.
pub fn stream_catalog_file(
    path: &Path,
    on_entry: &mut dyn FnMut(usize, CatalogEntry),
) -> Result<CatalogHeader, CatalogError> {
    let file = std::fs::File::open(path)
        .map_err(|e| CatalogError(format!("{} could not be read: {e}", path.display())))?;
    stream_catalog(file, on_entry).map_err(|e| CatalogError(format!("{}: {e}", path.display())))
}

/// Read a catalog off `reader`, bounded at [`CATALOG_MAX_BYTES`]:
/// [`stream_catalog`], its entries collected in catalog order.
///
/// # Errors
/// [`stream_catalog`]'s.
pub fn read_catalog(reader: impl Read) -> Result<Catalog, CatalogError> {
    let mut entries: Vec<(usize, CatalogEntry)> = Vec::new();
    let header = stream_catalog(reader, &mut |index, entry| entries.push((index, entry)))?;
    entries.sort_by_key(|(index, _)| *index);
    Ok(Catalog {
        name: header.name,
        display_name: header.display_name,
        description: header.description,
        owner: header.owner,
        entries: entries.into_iter().map(|(_, entry)| entry).collect(),
        notices: header.notices,
    })
}

/// The first catalog file under a marketplace root.
#[must_use]
pub fn find_catalog(root: &Path) -> Option<PathBuf> {
    MARKETPLACE_MANIFESTS
        .iter()
        .map(|rel| root.join(rel))
        .find(|path| path.is_file())
}

/// [`read_catalog`] over a file, the file named in the error.
///
/// # Errors
/// The file cannot be opened, or [`read_catalog`]'s.
pub fn read_catalog_file(path: &Path) -> Result<Catalog, CatalogError> {
    let file = std::fs::File::open(path)
        .map_err(|e| CatalogError(format!("{} could not be read: {e}", path.display())))?;
    read_catalog(file).map_err(|e| CatalogError(format!("{}: {e}", path.display())))
}

/// A marketplace of one: a source with no catalog but a plugin manifest at
/// its root is a marketplace named after its plugin, else after its
/// repository (`fallback`).
///
/// # Errors
/// Neither name is usable — a manifest naming itself `../../.ssh` is
/// refused before any path is made of it.
pub fn single_plugin_catalog(
    metadata: &PluginMetadata,
    fallback: &str,
) -> Result<Catalog, CatalogError> {
    let name = if metadata.name.is_empty() {
        fallback.to_string()
    } else {
        metadata.name.clone()
    };
    validate_name(&name).map_err(|why| {
        CatalogError(format!(
            "the plugin's name \"{name}\" cannot name a marketplace: {why}"
        ))
    })?;
    Ok(Catalog {
        name: name.clone(),
        display_name: metadata.display_name.clone(),
        description: metadata.description.clone(),
        owner: metadata.author.clone(),
        entries: vec![CatalogEntry {
            name,
            source: Some(PluginSource::Local {
                path: String::new(),
            }),
            unavailable: None,
            display_name: metadata.display_name.clone(),
            description: cut(metadata.description.clone()),
            version: metadata.version.clone(),
            author: metadata.author.clone(),
            category: metadata.category.clone(),
            homepage: metadata.homepage.clone(),
            keywords: metadata.keywords.clone(),
            manifest_fields: None,
        }],
        notices: Vec::new(),
    })
}

/// A repository's name as a marketplace name: `…/my-tool.git` → `my-tool`;
/// `marketplace` when nothing usable is left.
#[must_use]
pub fn repository_name(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .next()
        .unwrap_or_default();
    let stem = last.strip_suffix(".git").unwrap_or(last);
    let plain: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let plain = plain.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if validate_name(plain).is_ok() {
        plain.to_string()
    } else {
        "marketplace".to_string()
    }
}

fn reserved_spelling(name: &str) -> String {
    name.trim_end_matches('.')
        .chars()
        .map(|c| {
            if c == '.' || c == '_' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// The GitHub account a git URL is on, lowercased.
///
/// Only over a transport that authenticates GitHub — `https://`, `ssh://`
/// or `git@github.com:` — and only for a path git fetches as written
/// ([`is_plain_url_path`]): plain `http://` and `git://` can be answered by
/// anyone on the way, and a `..` makes the owner read here and the owner
/// fetched from two different accounts.
#[must_use]
pub fn github_owner(url: &str) -> Option<String> {
    let rest = ["https://github.com/", "ssh://git@github.com/"]
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    if !is_plain_url_path(rest) {
        return None;
    }
    let owner = rest.split('/').next()?;
    (!owner.is_empty()).then(|| owner.to_ascii_lowercase())
}

/// Refuse a reserved marketplace name (or another spelling of one — case,
/// `.` or `_` for `-`, a trailing `.`) unless the catalog comes from its
/// owner's GitHub organisation. `git_url` is `None` for a local folder.
///
/// # Errors
/// The name is reserved and the source is not its owner's.
pub fn check_reserved(name: &str, git_url: Option<&str>) -> Result<(), String> {
    let spelled = reserved_spelling(name);
    let (reserved, owner) = if let Some(r) = ANTHROPIC_RESERVED.iter().find(|r| **r == spelled) {
        (*r, "anthropics")
    } else if let Some(r) = OPENAI_RESERVED.iter().find(|r| **r == spelled) {
        (*r, "openai")
    } else {
        return Ok(());
    };
    if git_url.and_then(github_owner).as_deref() == Some(owner) {
        return Ok(());
    }
    let respelled = if reserved == name {
        String::new()
    } else {
        format!(" (another spelling of \"{reserved}\")")
    };
    Err(format!(
        "\"{name}\"{respelled} is a reserved marketplace name; it can only come from github.com/{owner}/"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(text: &str) -> Catalog {
        read_catalog(text.as_bytes()).unwrap()
    }

    /// The shape of `anthropics/claude-plugins-official`: `$schema`,
    /// `owner`, `renames`, string and object sources of every kind.
    const CLAUDE_SHAPED: &str = r#"{
  "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
  "name": "claude-plugins-official",
  "description": "Directory of popular Claude Code extensions",
  "owner": {"name": "Anthropic", "email": "support@anthropic.com"},
  "renames": {"old": "new"},
  "plugins": [
    {"name": "agent-sdk-dev", "description": "Development kit", "author": {"name": "Anthropic"},
     "source": "./plugins/agent-sdk-dev", "category": "development", "homepage": "https://h"},
    {"name": "42crunch", "description": "Audit APIs", "category": "security",
     "source": {"source": "git-subdir", "url": "https://github.com/42Crunch-AI/claude-plugins.git",
                "path": "plugins/api-security", "ref": "main", "sha": "09BF1539d41f9ff355ba3eb5d05d4a75813423bb"}},
    {"name": "atomic-agents", "source": {"source": "url", "url": "https://github.com/x/atomic.git",
     "path": "claude-plugin", "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}},
    {"name": "adlc", "source": {"source": "url", "url": "https://github.com/S/adlc.git",
     "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}},
    {"name": "rust-analyzer-lsp", "strict": false, "source": "./plugins/rust-analyzer-lsp",
     "lspServers": {"rust": {"command": "rust-analyzer"}}},
    {"name": "learn-with-coursera", "strict": false, "skills": ["./skills/a"],
     "source": {"source": "git-subdir", "url": "https://github.com/c/c.git", "path": "p", "sha": "abcdef1"}},
    {"name": "xcode", "displayName": "Xcode", "source": {"source": "command", "command": "xcrun x", "mode": "link"}}
  ]
}"#;

    /// The shape of `openai/plugins`: `interface`, `policy`, `local`
    /// objects, unpinned remote sources, no descriptions.
    const OPENAI_SHAPED: &str = r#"{
  "name": "openai-curated",
  "interface": {"displayName": "Codex official"},
  "plugins": [
    {"name": "linear", "source": {"source": "local", "path": "./plugins/linear"},
     "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}, "category": "Productivity"},
    {"name": "hidden", "source": {"source": "local", "path": "./plugins/hidden"},
     "policy": {"installation": "NOT_AVAILABLE"}, "category": "Productivity"},
    {"name": "crowdstrike", "source": {"source": "url", "url": "https://github.com/CrowdStrike/foundry-skills.git"},
     "policy": {"installation": "AVAILABLE", "products": ["CODEX"]}, "category": "Security",
     "interface": {"displayName": "CrowdStrike"}},
    {"name": "qodo", "source": {"source": "git-subdir", "url": "https://github.com/qodo-ai/qodo-skills.git",
     "path": "codex-packages/qodo"}, "category": "Developer Tools"}
  ]
}"#;

    #[test]
    fn the_claude_shaped_catalog_reads_every_source_form() {
        let c = catalog(CLAUDE_SHAPED);
        assert_eq!(c.name, "claude-plugins-official");
        assert_eq!(c.owner.as_deref(), Some("Anthropic"));
        assert_eq!(c.entries.len(), 7, "{:?}", c.notices);
        assert_eq!(
            c.entry("agent-sdk-dev").unwrap().source,
            Some(PluginSource::Local {
                path: "plugins/agent-sdk-dev".into()
            })
        );
        assert_eq!(
            c.entry("agent-sdk-dev").unwrap().author.as_deref(),
            Some("Anthropic")
        );
        match c.entry("42crunch").unwrap().source.as_ref().unwrap() {
            PluginSource::Git {
                url,
                path,
                git_ref,
                sha,
            } => {
                assert_eq!(url, "https://github.com/42Crunch-AI/claude-plugins.git");
                assert_eq!(path.as_deref(), Some("plugins/api-security"));
                assert_eq!(git_ref.as_deref(), Some("main"));
                assert_eq!(
                    sha.as_deref(),
                    Some("09bf1539d41f9ff355ba3eb5d05d4a75813423bb")
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            c.entry("atomic-agents").unwrap().source,
            Some(PluginSource::Git { path: Some(_), .. })
        ));
        let lsp = c.entry("rust-analyzer-lsp").unwrap();
        assert!(
            lsp.manifest_fields
                .as_deref()
                .unwrap()
                .contains("lspServers")
        );
        let coursera = c.entry("learn-with-coursera").unwrap();
        assert!(coursera.declares_components());
        // Its pin is a 7-digit prefix: listed, not installable.
        assert!(coursera.source.is_none());
        assert!(coursera.unavailable.as_deref().unwrap().contains("prefix"));
        let xcode = c.entry("xcode").unwrap();
        assert!(xcode.source.is_none());
        assert!(xcode.unavailable.as_deref().unwrap().contains("command"));
        assert_eq!(xcode.display_name.as_deref(), Some("Xcode"));
    }

    #[test]
    fn the_openai_shaped_catalog_reads_local_objects_policies_and_interfaces() {
        let c = catalog(OPENAI_SHAPED);
        assert_eq!(c.name, "openai-curated");
        assert_eq!(c.display_name.as_deref(), Some("Codex official"));
        assert_eq!(c.entries.len(), 4);
        assert_eq!(
            c.entry("linear").unwrap().category.as_deref(),
            Some("Productivity")
        );
        assert!(c.entry("linear").unwrap().unavailable.is_none());
        assert!(c.entry("hidden").unwrap().unavailable.is_some());
        assert_eq!(
            c.entry("crowdstrike").unwrap().display_name.as_deref(),
            Some("CrowdStrike")
        );
        assert!(matches!(
            c.entry("qodo").unwrap().source,
            Some(PluginSource::Git { path: Some(ref p), sha: None, .. }) if p == "codex-packages/qodo"
        ));
    }

    #[test]
    fn a_github_source_and_a_plugin_root_are_read() {
        let c = catalog(
            r#"{"name": "m", "plugins": [
                {"name": "a", "source": {"source": "github", "repo": "o/r", "ref": "v1"}},
                {"name": "b", "source": "bare"}],
                "metadata": {"pluginRoot": "./plugins"}}"#,
        );
        assert_eq!(
            c.entry("a")
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .git_url("file:///gh/"),
            Some("file:///gh/o/r.git".into())
        );
        assert_eq!(
            c.entry("b").unwrap().source,
            Some(PluginSource::Local {
                path: "plugins/bare".into()
            })
        );
    }

    #[test]
    fn a_bad_entry_is_skipped_with_its_reason_and_the_rest_load() {
        let c = catalog(
            r#"{"name": "m", "plugins": [
                3,
                {"source": "./x"},
                {"name": "../../.ssh", "source": "./x"},
                {"name": "noslash", "source": "x"},
                {"name": "climb", "source": "./../x"},
                {"name": "opt", "source": {"source": "url", "url": "https://h/r.git", "ref": "--upload-pack=evil"}},
                {"name": "badsha", "source": {"source": "url", "url": "https://h/r.git", "sha": "xyz"}},
                {"name": "ext", "source": {"source": "url", "url": "ext::sh -c touch% /tmp/x"}},
                {"name": "dash", "source": {"source": "url", "url": "-oProxyCommand=x"}},
                {"name": "subopt", "source": {"source": "git-subdir", "url": "https://h/r.git", "path": "--x"}},
                {"name": "ok", "source": "./ok", "description": 7, "keywords": ["k", 1]},
                {"name": "ok", "source": "./dup"}
            ]}"#,
        );
        assert_eq!(c.entries.len(), 1, "{:?}", c.notices);
        assert_eq!(c.entries[0].keywords, ["k"]);
        assert_eq!(c.entries[0].description, None);
        assert_eq!(c.notices.len(), 11, "{:?}", c.notices);
        assert!(
            c.notices.iter().any(|n| n.contains("--upload-pack")),
            "{:?}",
            c.notices
        );
        assert!(c.notices.iter().any(|n| n.contains("twice")));
    }

    /// A catalog is somebody else's text: an escape sequence in what it
    /// says about a plugin never reaches the terminal.
    #[test]
    fn display_text_is_cleaned_of_control_characters_as_it_is_read() {
        let c = catalog(
            r#"{"name": "m", "description": "M\u001b[2J", "owner": "O\u0007",
                "plugins": [{"name": "p", "source": "./p",
                "description": "evil\u001b]52;c;AAAA\u0007 line\ntwo",
                "displayName": "D\u202e", "version": "1\u009b", "author": "A\u001b",
                "category": "c\u0000", "homepage": "https://h\u001b]8;;x\u0007",
                "keywords": ["k\u001b"]}]}"#,
        );
        let e = &c.entries[0];
        assert_eq!(e.description.as_deref(), Some("evil]52;c;AAAA line\ntwo"));
        assert_eq!(e.display_name.as_deref(), Some("D"));
        assert_eq!(e.version.as_deref(), Some("1"));
        assert_eq!(e.author.as_deref(), Some("A"));
        assert_eq!(e.category.as_deref(), Some("c"));
        assert_eq!(e.homepage.as_deref(), Some("https://h]8;;x"));
        assert_eq!(e.keywords, ["k"]);
        assert_eq!(c.description.as_deref(), Some("M[2J"));
        assert_eq!(c.owner.as_deref(), Some("O"));
    }

    /// Names that differ only by case are one directory on a
    /// case-insensitive filesystem: the second is a duplicate, and the first
    /// keeps its spelling.
    #[test]
    fn a_name_listed_twice_in_another_case_is_a_duplicate() {
        let c = catalog(
            r#"{"name": "m", "plugins": [
                {"name": "lint", "source": "./a"},
                {"name": "Lint", "source": "./b"},
                {"name": "LINT", "source": "./c"},
                {"name": "other", "source": "./d"}]}"#,
        );
        let names: Vec<&str> = c.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["lint", "other"]);
        assert_eq!(c.notices.len(), 2, "{:?}", c.notices);
        assert!(c.notices[0].contains("\"Lint\""), "{:?}", c.notices);
    }

    /// A catalog of tens of thousands of entries is read in linear time and
    /// bounded: past [`CATALOG_MAX_ENTRIES`] the rest is one notice, and the
    /// notices themselves stop at [`CATALOG_MAX_NOTICES`] and one more.
    #[test]
    fn a_huge_catalog_is_capped_in_entries_and_in_notices() {
        let mut text = String::from(r#"{"name": "m", "plugins": ["#);
        let total = CATALOG_MAX_ENTRIES + 700;
        for i in 0..total {
            if i > 0 {
                text.push(',');
            }
            if i % 10 == 0 {
                text.push('3');
            } else {
                text.push_str(&format!(r#"{{"name": "p{i}", "source": "./p"}}"#));
            }
        }
        text.push_str("]}");
        let c = catalog(&text);
        let bad = CATALOG_MAX_ENTRIES / 10;
        assert_eq!(c.entries.len(), CATALOG_MAX_ENTRIES - bad);
        assert_eq!(
            c.notices.len(),
            CATALOG_MAX_NOTICES + 2,
            "{:?}",
            c.notices.last()
        );
        assert!(
            c.notices[CATALOG_MAX_NOTICES].contains(&format!("{} more", bad - CATALOG_MAX_NOTICES)),
            "{:?}",
            &c.notices[CATALOG_MAX_NOTICES..]
        );
        assert!(
            c.notices
                .last()
                .unwrap()
                .contains(&format!("{} more entries", total - CATALOG_MAX_ENTRIES)),
            "{:?}",
            c.notices.last()
        );
    }

    /// A pinned commit is the whole id: a prefix is something the plugin
    /// repository's owner could make a second commit match.
    #[test]
    fn a_pinned_sha_must_be_the_full_commit_id() {
        let full = "09bf1539d41f9ff355ba3eb5d05d4a75813423bb";
        assert_eq!(
            checked_sha(Some(full.to_uppercase())).unwrap().as_deref(),
            Some(full)
        );
        assert!(checked_sha(Some("abcdef1".into())).is_err());
        let c = catalog(
            r#"{"name": "m", "plugins": [{"name": "short", "source":
                {"source": "url", "url": "https://h/r.git", "sha": "abcdef1"}}]}"#,
        );
        let short = c.entry("short").expect("listed, not skipped");
        assert!(short.source.is_none());
        assert!(
            short.unavailable.as_deref().unwrap().contains("40"),
            "{:?}",
            short.unavailable
        );
    }

    /// A dot segment or an escape in a URL's path is refused before git (or
    /// the reserved-name check) reads it: libcurl collapses `/../`, so the
    /// text and the fetch would name different repositories.
    #[test]
    fn a_git_url_with_a_dot_segment_or_an_escape_in_its_path_is_refused() {
        for bad in [
            "https://github.com/anthropics/../evil/claude-plugins-official",
            "https://github.com/anthropics/./x",
            "https://github.com/anthropics//x",
            "https://github.com/anthropics/%2e%2e/evil/x",
            "https://github.com/anthr%6fpics/x",
            "git@github.com:anthropics/../evil/x",
            "ssh://git@github.com/anthropics/../evil/x",
        ] {
            assert!(normalize_git_url(bad).is_err(), "{bad}");
        }
        assert!(normalize_git_url("https://github.com/anthropics/x.git").is_ok());
        assert!(normalize_git_url("https://h/r.git/").is_ok());
    }

    /// Only an authenticated transport names the official owner: plain
    /// `http://` and `git://` can be answered by anyone on the path.
    #[test]
    fn the_reserved_names_need_an_authenticated_url_on_the_owners_account() {
        let name = "claude-plugins-official";
        for good in [
            "https://github.com/anthropics/claude-plugins-official.git",
            "ssh://git@github.com/anthropics/claude-plugins-official.git",
            "git@github.com:anthropics/claude-plugins-official.git",
        ] {
            assert!(check_reserved(name, Some(good)).is_ok(), "{good}");
        }
        for bad in [
            "http://github.com/anthropics/claude-plugins-official.git",
            "git://github.com/anthropics/claude-plugins-official.git",
            "https://github.com/anthropics/../evil/claude-plugins-official",
            "https://github.com/anthropics/%2e%2e/evil/x",
            "git@github.com:anthropics/../evil/x",
        ] {
            assert!(check_reserved(name, Some(bad)).is_err(), "{bad}");
            assert_eq!(github_owner(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_catalog_without_a_usable_name_or_plugin_list_is_refused() {
        for bad in [
            "{",
            "[]",
            r#"{"plugins": []}"#,
            r#"{"name": "../x", "plugins": []}"#,
            r#"{"name": "m"}"#,
            r#"{"name": "m", "plugins": {}}"#,
        ] {
            assert!(read_catalog(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    /// Every entry a stream hands over, with its catalog index.
    fn streamed(
        text: &str,
    ) -> (
        Result<CatalogHeader, CatalogError>,
        Vec<(usize, CatalogEntry)>,
    ) {
        let mut got = Vec::new();
        let header = stream_catalog(text.as_bytes(), &mut |i, entry| got.push((i, entry)));
        (header, got)
    }

    #[test]
    fn a_stream_hands_over_what_read_catalog_returns_entry_for_entry() {
        for text in [CLAUDE_SHAPED, OPENAI_SHAPED] {
            let whole = catalog(text);
            let (header, got) = streamed(text);
            let header = header.unwrap();
            assert_eq!(header.name, whole.name);
            assert_eq!(header.display_name, whole.display_name);
            assert_eq!(header.description, whole.description);
            assert_eq!(header.owner, whole.owner);
            assert_eq!(header.notices, whole.notices);
            let entries: Vec<CatalogEntry> = got.iter().map(|(_, e)| e.clone()).collect();
            assert_eq!(entries, whole.entries);
            // In catalog order, each with its index in the `plugins` list.
            let indexes: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
            assert!(indexes.windows(2).all(|w| w[0] < w[1]), "{indexes:?}");
        }
    }

    #[test]
    fn a_stream_with_a_broken_entry_still_yields_the_others_with_a_notice() {
        let (header, got) = streamed(
            r#"{"name": "m", "plugins": [
                {"name": "a", "source": "./a"},
                {"name": "../../.ssh", "source": "./x"},
                7,
                {"name": "b", "source": {"source": "npm", "package": "x"}},
                {"name": "c", "source": "./c"}]}"#,
        );
        let header = header.unwrap();
        let names: Vec<(usize, &str)> = got.iter().map(|(i, e)| (*i, e.name.as_str())).collect();
        assert_eq!(names, [(0, "a"), (3, "b"), (4, "c")]);
        assert!(
            got[1].1.unavailable.is_some(),
            "an unsupported source is listed, unavailable"
        );
        assert_eq!(header.notices.len(), 2, "{:?}", header.notices);
        assert!(
            header.notices[0].starts_with("entry 2 skipped"),
            "{:?}",
            header.notices
        );
        assert!(
            header.notices[1].starts_with("entry 3 skipped"),
            "{:?}",
            header.notices
        );
    }

    #[test]
    fn a_stream_hands_each_entry_over_as_it_is_read() {
        // The document breaks off after two entries: both were already
        // handed over when the error is found.
        let (header, got) = streamed(
            r#"{"name": "m", "plugins": [{"name": "a", "source": "./a"}, {"name": "b", "source": "./b"}, {"#,
        );
        assert!(header.is_err());
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_bare_source_waits_for_a_plugin_root_named_after_the_list() {
        let (header, got) = streamed(
            r#"{"name": "m", "plugins": [
                {"name": "b", "source": "bare"},
                {"name": "a", "source": "./a"}],
                "metadata": {"pluginRoot": "./plugins"}}"#,
        );
        assert!(header.unwrap().notices.is_empty());
        let got: Vec<(usize, &str, Option<PluginSource>)> = got
            .iter()
            .map(|(i, e)| (*i, e.name.as_str(), e.source.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (1, "a", Some(PluginSource::Local { path: "a".into() })),
                (
                    0,
                    "b",
                    Some(PluginSource::Local {
                        path: "plugins/bare".into()
                    })
                ),
            ]
        );
        // With none named at all, it is skipped, saying why.
        let (header, got) =
            streamed(r#"{"name": "m", "plugins": [{"name": "b", "source": "bare"}]}"#);
        assert!(got.is_empty());
        assert!(header.unwrap().notices[0].contains("pluginRoot"));
        // A root named before the list resolves it at once.
        let (_, got) = streamed(
            r#"{"name": "m", "metadata": {"pluginRoot": "p"}, "plugins": [{"name": "b", "source": "bare"}]}"#,
        );
        assert_eq!(
            got[0].1.source,
            Some(PluginSource::Local {
                path: "p/bare".into()
            })
        );
    }

    #[test]
    fn a_stream_refuses_what_read_catalog_refuses() {
        for bad in [
            "{",
            "[]",
            r#"{"plugins": []}"#,
            r#"{"name": "../x", "plugins": []}"#,
            r#"{"name": "m"}"#,
            r#"{"name": "m", "plugins": null}"#,
            r#"{"name": "m", "plugins": {}}"#,
            r#"{"name": "m", "plugins": []} trailing"#,
        ] {
            assert!(streamed(bad).0.is_err(), "{bad}");
            assert!(read_catalog(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_long_description_is_cut() {
        let long = "x".repeat(DESCRIPTION_MAX_CHARS + 50);
        let c = catalog(&format!(
            r#"{{"name": "m", "plugins": [{{"name": "a", "source": "./a", "description": "{long}"}}]}}"#
        ));
        assert_eq!(
            c.entries[0].description.as_ref().unwrap().chars().count(),
            DESCRIPTION_MAX_CHARS + 1
        );
    }

    #[test]
    fn the_read_stops_at_the_cap() {
        let huge = std::io::repeat(b' ').take(CATALOG_MAX_BYTES + 10);
        let err = read_catalog(huge).unwrap_err();
        assert!(err.to_string().contains("not valid"), "{err}");
    }

    #[test]
    fn an_entry_becomes_a_manifest_document() {
        let c = catalog(
            r#"{"name": "m", "plugins": [{"name": "a", "source": "./a", "version": "2",
                "author": {"name": "Ann"}, "skills": ["./s"], "hooks": {"Stop": []}}]}"#,
        );
        let text = c.entries[0].manifest_text();
        let manifest = crate::manifest::parse_legacy(&text, "entry").unwrap();
        assert_eq!(manifest.metadata.name, "a");
        assert_eq!(manifest.metadata.version.as_deref(), Some("2"));
        assert_eq!(manifest.metadata.author.as_deref(), Some("Ann"));
        assert_eq!(manifest.components.skills, ["./s"]);
        assert_eq!(manifest.components.hooks.len(), 1);
    }

    #[test]
    fn git_urls_are_normalized_and_options_refused() {
        assert_eq!(
            normalize_git_url("o/r").unwrap(),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            normalize_git_url("https://x.org/r.git/").unwrap(),
            "https://x.org/r.git"
        );
        assert_eq!(normalize_git_url("git@h:o/r.git").unwrap(), "git@h:o/r.git");
        assert_eq!(
            normalize_git_url("file:///srv/r.git").unwrap(),
            "file:///srv/r.git"
        );
        for bad in ["", "-u", "ext::sh", "fd::3", "ftp://x/r", "a b", "o/r/x"] {
            assert!(normalize_git_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refs_and_shas_are_read_as_what_they_must_be() {
        assert_eq!(
            checked_ref(Some("release/v1.2".into())).unwrap().as_deref(),
            Some("release/v1.2")
        );
        for bad in ["-x", "+x", "a..b", "a@{1}", "a b", "/x"] {
            assert!(checked_ref(Some(bad.into())).is_err(), "{bad}");
        }
        assert_eq!(
            checked_sha(Some("ABCDEF1".repeat(5) + "ABCDE"))
                .unwrap()
                .as_deref(),
            Some("abcdef1abcdef1abcdef1abcdef1abcdef1abcde")
        );
        for bad in ["abc", "abcdef1", "g123456", &"a".repeat(41)] {
            assert!(checked_sha(Some(bad.into())).is_err(), "{bad}");
        }
    }

    #[test]
    fn reserved_names_come_only_from_their_owners() {
        let anthropic = "https://github.com/anthropics/claude-plugins-official.git";
        assert!(check_reserved("claude-plugins-official", Some(anthropic)).is_ok());
        assert!(
            check_reserved(
                "claude-plugins-official",
                Some("https://github.com/evil/x.git")
            )
            .is_err()
        );
        assert!(check_reserved("claude-plugins-official", None).is_err());
        let respelled =
            check_reserved("Claude.Plugins_Official.", Some("git@github.com:evil/x")).unwrap_err();
        assert!(respelled.contains("another spelling"), "{respelled}");
        assert!(
            check_reserved("openai-curated", Some("git@github.com:openai/plugins.git")).is_ok()
        );
        assert!(
            check_reserved(
                "openai-curated",
                Some("https://gitlab.com/openai/plugins.git")
            )
            .is_err()
        );
        assert!(check_reserved("team", None).is_ok());
    }

    #[test]
    fn a_repository_names_a_nameless_marketplace() {
        assert_eq!(
            repository_name("https://github.com/o/my-tool.git"),
            "my-tool"
        );
        assert_eq!(repository_name("git@h:o/x_y"), "x_y");
        assert_eq!(repository_name("/srv/"), "srv");
        assert_eq!(repository_name("https://h/..."), "marketplace");
    }

    #[test]
    fn a_marketplace_of_one_is_named_after_its_plugin() {
        let meta = PluginMetadata {
            name: "solo".into(),
            version: Some("1".into()),
            ..PluginMetadata::default()
        };
        let c = single_plugin_catalog(&meta, "repo").unwrap();
        assert_eq!(c.name, "solo");
        assert_eq!(
            c.entries[0].source,
            Some(PluginSource::Local {
                path: String::new()
            })
        );
        let nameless = single_plugin_catalog(&PluginMetadata::default(), "repo").unwrap();
        assert_eq!(nameless.name, "repo");
        let hostile = PluginMetadata {
            name: "../../../.ssh".into(),
            ..PluginMetadata::default()
        };
        assert!(single_plugin_catalog(&hostile, "repo").is_err());
    }
}

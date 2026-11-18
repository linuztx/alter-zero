//! Marketplace catalogs (`docs/plugins.md` *Marketplaces*): the
//! `marketplace.json` both references write, each entry's `source`, and the
//! git URL spellings a source may use.
//!
//! A catalog can list hundreds of plugins (Anthropic's lists over three
//! hundred), and `/plugins` reads every catalog each time it opens, so the
//! body is never built as one tree: the `plugins` array is borrowed as raw
//! slices and each entry is decoded, validated and dropped on its own
//! (`docs/memory.md`). An entry keeps a copy of its own text only when it
//! carries component keys — the few that are a manifest for a directory
//! that has none.

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use super::id::validate_name;
use super::manifest::{PluginFormat, PluginManifest, legacy_manifest};

/// Where a marketplace's catalog is, relative to its root, in precedence
/// order: codex's layout, then Claude Code's.
pub const MARKETPLACE_MANIFESTS: &[&str] = &[
    ".agents/plugins/marketplace.json",
    ".claude-plugin/marketplace.json",
];

/// The most of a catalog file the boundary reads.
pub const MARKETPLACE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The entry keys that make an entry a manifest worth keeping whole: the
/// component keys, and the ones the plugin's page lists as unused.
const MANIFEST_KEYS: &[&str] = &[
    "skills",
    "commands",
    "agents",
    "hooks",
    "mcpServers",
    "lspServers",
    "outputStyles",
    "userConfig",
    "channels",
    "dependencies",
    "interface",
];

/// Where a marketplace entry says its plugin is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A directory of the marketplace itself: its path from the marketplace
    /// root, `/`-joined with no leading `./` — empty for the root.
    Local { path: String },
    /// A git repository: where, which directory of it (the whole of it when
    /// `None`), and which commit — `sha` when pinned, else `ref_name`, else
    /// the default branch.
    Git {
        url: String,
        path: Option<String>,
        ref_name: Option<String>,
        sha: Option<String>,
    },
    /// A source this app does not fetch: listed, never installed.
    Unsupported { kind: String, reason: String },
}

impl PluginSource {
    /// The source as the plugin's page shows it.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Local { path } if path.is_empty() => "the marketplace itself".to_string(),
            Self::Local { path } => format!("./{path}"),
            Self::Git {
                url,
                path,
                ref_name,
                sha,
            } => {
                let mut label = url.clone();
                if let Some(path) = path {
                    label.push_str(" · ");
                    label.push_str(path);
                }
                if let Some(at) = ref_name
                    .as_deref()
                    .or_else(|| sha.as_deref().map(|sha| &sha[..sha.len().min(12)]))
                {
                    label.push_str(" @ ");
                    label.push_str(at);
                }
                label
            }
            Self::Unsupported { kind, .. } => format!("{kind} (not supported)"),
        }
    }

    /// Can a plugin with this source be installed?
    ///
    /// # Errors
    /// Why it cannot.
    pub fn installable(&self) -> Result<(), String> {
        match self {
            Self::Unsupported { reason, .. } => Err(reason.clone()),
            Self::Local { .. } | Self::Git { .. } => Ok(()),
        }
    }
}

/// One plugin a marketplace lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceEntry {
    /// The install key's first half — what the user types before `@`.
    pub name: String,
    pub source: PluginSource,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    /// The author's name.
    pub author: Option<String>,
    pub category: Option<String>,
    pub homepage: Option<String>,
    /// The entry's `tags` and `keywords`, in that order.
    pub keywords: Vec<String>,
    /// The entry's own JSON, kept only when it declares components.
    pub manifest_fields: Option<String>,
}

impl MarketplaceEntry {
    /// The entry as a manifest — what a plugin directory with no manifest
    /// of its own is read by (Claude Code's rule, and codex's fallback).
    #[must_use]
    pub fn manifest(&self) -> PluginManifest {
        let declared = self
            .manifest_fields
            .as_deref()
            .and_then(|text| serde_json::from_str::<Map<String, Value>>(text).ok());
        let mut manifest = match declared {
            Some(object) => legacy_manifest(&object, PluginFormat::ClaudeCode, &self.name),
            None => PluginManifest::named(PluginFormat::ClaudeCode, self.name.clone()),
        };
        // The entry's name is the plugin's when it stands in for a manifest,
        // and what the catalog already read is not read twice.
        manifest.name.clone_from(&self.name);
        manifest.display_name.clone_from(&self.display_name);
        manifest.description.clone_from(&self.description);
        manifest.version.clone_from(&self.version);
        manifest.author.clone_from(&self.author);
        manifest.category.clone_from(&self.category);
        manifest.homepage.clone_from(&self.homepage);
        manifest.keywords.clone_from(&self.keywords);
        manifest
    }
}

/// A parsed catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marketplace {
    /// The marketplace's own name: the second half of every plugin id from
    /// it, and what it is registered under.
    pub name: String,
    pub description: Option<String>,
    /// The maintainer's name.
    pub owner: Option<String>,
    pub plugins: Vec<MarketplaceEntry>,
    /// The entries that were skipped, and why.
    pub notes: Vec<String>,
}

/// The catalog's top level, with the `plugins` array left as raw text.
#[derive(Deserialize)]
struct RawMarketplace<'a> {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    owner: Option<Value>,
    #[serde(default)]
    metadata: Option<Value>,
    #[serde(borrow, default)]
    plugins: Option<&'a RawValue>,
}

/// A trimmed, non-empty copy of a string value.
fn text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A name out of `"Someone"` or `{"name": "Someone"}` — an `author` or an
/// `owner`.
fn person(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Object(object) => text(object.get("name")),
        other => text(Some(other)),
    }
}

/// Read a `marketplace.json`.
///
/// # Errors
/// A document that is not a JSON object, has no usable `name`, or has no
/// `plugins` array. An entry that cannot be used is a note on a catalog
/// that still loads.
pub fn parse_marketplace(contents: &str) -> Result<Marketplace, String> {
    let raw: RawMarketplace<'_> = serde_json::from_str(contents)
        .map_err(|err| format!("the marketplace file is not a valid catalog: {err}"))?;
    let name = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "the marketplace file has no `name`".to_string())?
        .to_string();
    validate_name(&name)
        .map_err(|reason| format!("invalid marketplace name `{name}`: {reason}"))?;
    let entries: Vec<&RawValue> = raw
        .plugins
        .ok_or_else(|| "the marketplace file has no `plugins` list".to_string())
        .and_then(|plugins| {
            serde_json::from_str(plugins.get())
                .map_err(|_| "the marketplace's `plugins` must be a list".to_string())
        })?;

    let metadata = raw.metadata.as_ref().and_then(Value::as_object);
    let plugin_root = metadata
        .and_then(|metadata| text(metadata.get("pluginRoot")))
        .and_then(|root| clean_relative(root.strip_prefix("./").unwrap_or(&root)).ok());

    let mut market = Marketplace {
        name,
        description: raw
            .description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .or_else(|| metadata.and_then(|metadata| text(metadata.get("description")))),
        owner: person(raw.owner.as_ref()),
        plugins: Vec::with_capacity(entries.len()),
        notes: Vec::new(),
    };
    for (index, raw) in entries.iter().enumerate() {
        match parse_entry(raw.get(), plugin_root.as_deref()) {
            Ok(entry) if market.plugins.iter().any(|seen| seen.name == entry.name) => {
                market.notes.push(format!(
                    "plugin `{}` is listed twice — the first entry is used",
                    entry.name
                ));
            }
            Ok(entry) => market.plugins.push(entry),
            Err(reason) => market
                .notes
                .push(format!("entry {} skipped: {reason}", index + 1)),
        }
    }
    Ok(market)
}

/// Decode one entry of the `plugins` array.
fn parse_entry(raw: &str, plugin_root: Option<&str>) -> Result<MarketplaceEntry, String> {
    let object: Map<String, Value> =
        serde_json::from_str(raw).map_err(|_| "it is not an object".to_string())?;
    let name = text(object.get("name")).ok_or_else(|| "it has no `name`".to_string())?;
    validate_name(&name).map_err(|reason| format!("invalid plugin name `{name}`: {reason}"))?;
    let source = object
        .get("source")
        .ok_or_else(|| format!("`{name}` has no `source`"))
        .and_then(|source| {
            parse_source(source, plugin_root).map_err(|reason| format!("`{name}`: {reason}"))
        })?;
    let strings = |key: &str| -> Vec<String> {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| text(Some(item))).collect())
            .unwrap_or_default()
    };
    let mut keywords = strings("tags");
    keywords.extend(strings("keywords"));
    let interface = object.get("interface").and_then(Value::as_object);
    let declares_components = MANIFEST_KEYS
        .iter()
        .any(|key| object.get(*key).is_some_and(|value| !value.is_null()));
    Ok(MarketplaceEntry {
        source,
        display_name: text(object.get("displayName"))
            .or_else(|| interface.and_then(|interface| text(interface.get("displayName")))),
        description: text(object.get("description"))
            .or_else(|| interface.and_then(|interface| text(interface.get("shortDescription")))),
        version: text(object.get("version")),
        author: person(object.get("author")),
        category: text(object.get("category")),
        homepage: text(object.get("homepage")),
        keywords,
        manifest_fields: declares_components.then(|| raw.to_string()),
        name,
    })
}

/// A plugin directory with no catalog, as the marketplace it is on its own:
/// one entry, named after the plugin, whose source is the directory itself.
/// How a plugin's own repository — the only way an Agent Plugin is
/// published — is something a user can add.
#[must_use]
pub fn single_plugin_marketplace(manifest: &PluginManifest) -> Marketplace {
    Marketplace {
        name: manifest.name.clone(),
        description: manifest.description.clone(),
        owner: manifest.author.clone(),
        plugins: vec![MarketplaceEntry {
            name: manifest.name.clone(),
            source: PluginSource::Local {
                path: String::new(),
            },
            display_name: manifest.display_name.clone(),
            description: manifest.description.clone(),
            version: manifest.version.clone(),
            author: manifest.author.clone(),
            category: manifest.category.clone(),
            homepage: manifest.homepage.clone(),
            keywords: manifest.keywords.clone(),
            manifest_fields: None,
        }],
        notes: Vec::new(),
    }
}

/// A relative path as clean `/`-joined components — no `.`, no empty part —
/// or why it cannot be used: a `..`, a backslash, an absolute path.
fn clean_relative(path: &str) -> Result<String, String> {
    if path.contains('\\') {
        return Err("a path must use forward slashes".to_string());
    }
    if path.starts_with('/') {
        return Err("a path must be relative".to_string());
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("a path must not contain `..`".to_string()),
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

/// A relative-path source: `./dir`, `.` for the marketplace root, or a bare
/// name under `metadata.pluginRoot`.
fn local_source(path: &str, plugin_root: Option<&str>) -> Result<PluginSource, String> {
    let path = path.trim();
    let clean = match path {
        "" => return Err("its source path is empty".to_string()),
        "." | "./" => String::new(),
        path if path.starts_with("./") => clean_relative(&path[2..])?,
        path if !path.contains('/') && !path.contains('\\') && path != ".." => match plugin_root {
            Some("") => path.to_string(),
            Some(root) => format!("{root}/{path}"),
            None => {
                return Err(format!(
                    "source path `{path}` must start with `./` (a bare name needs \
                     `metadata.pluginRoot`)"
                ));
            }
        },
        path => return Err(format!("source path `{path}` must start with `./`")),
    };
    Ok(PluginSource::Local { path: clean })
}

/// A branch or tag name safe to hand `git` as an argument: plain ref
/// characters, never an option.
pub(super) fn checked_ref(name: Option<String>) -> Result<Option<String>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '@' | '+'));
    if !plain || name.starts_with(['-', '+']) || name.contains("..") || name.contains("@{") {
        return Err(format!("`ref` `{name}` is not a branch or tag name"));
    }
    Ok(Some(name))
}

/// A commit id safe to hand `git` as an argument: 7 to 40 hex digits.
fn checked_sha(sha: Option<String>) -> Result<Option<String>, String> {
    let Some(sha) = sha else {
        return Ok(None);
    };
    if !(7..=40).contains(&sha.len()) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("`sha` `{sha}` is not a commit id"));
    }
    Ok(Some(sha.to_ascii_lowercase()))
}

/// Read an entry's `source`.
fn parse_source(source: &Value, plugin_root: Option<&str>) -> Result<PluginSource, String> {
    let object = match source {
        Value::String(path) => return local_source(path, plugin_root),
        Value::Object(object) => object,
        _ => return Err("its `source` must be a path or an object".to_string()),
    };
    let field = |key: &str| text(object.get(key));
    let kind = field("source").ok_or_else(|| "its `source` names no type".to_string())?;
    let subdir = |required: bool| -> Result<Option<String>, String> {
        match field("path") {
            Some(path) => {
                let clean = clean_relative(path.strip_prefix("./").unwrap_or(&path))?;
                if clean.is_empty() {
                    return Err("its `path` is empty".to_string());
                }
                Ok(Some(clean))
            }
            None if required => Err("a `git-subdir` source needs a `path`".to_string()),
            None => Ok(None),
        }
    };
    let git = |url: String, path: Option<String>| -> Result<PluginSource, String> {
        Ok(PluginSource::Git {
            url,
            path,
            ref_name: checked_ref(field("ref"))?,
            sha: checked_sha(field("sha"))?,
        })
    };
    let unsupported = |reason: &str| {
        Ok(PluginSource::Unsupported {
            kind: kind.clone(),
            reason: reason.to_string(),
        })
    };
    match kind.as_str() {
        "local" => local_source(
            &field("path").ok_or_else(|| "a `local` source needs a `path`".to_string())?,
            plugin_root,
        ),
        "github" => {
            let repo =
                field("repo").ok_or_else(|| "a `github` source needs a `repo`".to_string())?;
            let url = github_shorthand(&repo)
                .ok_or_else(|| format!("`repo` `{repo}` is not `owner/repo`"))?;
            git(url, None)
        }
        "url" => {
            let url = field("url").ok_or_else(|| "a `url` source needs a `url`".to_string())?;
            git(normalize_git_url(&url)?, subdir(false)?)
        }
        "git-subdir" => {
            let url =
                field("url").ok_or_else(|| "a `git-subdir` source needs a `url`".to_string())?;
            git(normalize_git_url(&url)?, subdir(true)?)
        }
        "npm" => unsupported("npm sources are not supported"),
        "archive" => unsupported("archive sources are not supported"),
        "command" => unsupported("a marketplace may not run a command to locate a plugin"),
        other => unsupported(&format!("unknown source type `{other}`")),
    }
}

/// `owner/repo` (or `owner/repo.git`) as GitHub's clone URL, or `None` for
/// anything else.
fn github_shorthand(text: &str) -> Option<String> {
    let (owner, repo) = text.split_once('/')?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let segment = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (segment(owner) && segment(repo) && !owner.starts_with('-'))
        .then(|| format!("https://github.com/{owner}/{repo}.git"))
}

/// A git repository's address in the one spelling `git` is handed: an
/// `https://`, `http://`, `ssh://`, `git://` or `file://` URL, a
/// `user@host:path` address, an absolute path, or GitHub's `owner/repo`
/// shorthand as its clone URL.
///
/// What comes out is passed to `git` as an argument, so nothing that could
/// be read as an option or as a transport helper (`ext::…`) gets through.
///
/// # Errors
/// An address in none of those forms.
pub fn normalize_git_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let invalid = || format!("`{url}` is not a git repository address");
    if url.is_empty()
        || url.starts_with('-')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git" | "file") || rest.is_empty() {
            return Err(invalid());
        }
        return Ok(url.trim_end_matches('/').to_string());
    }
    // `user@host:path` — scp's form. The colon comes before any slash, and
    // a single one: `ext::…` names a transport helper, not a host.
    if let Some((host, path)) = url.split_once(':')
        && host.contains('@')
        && !host.contains('/')
        && !path.is_empty()
        && !path.starts_with(':')
    {
        return Ok(url.to_string());
    }
    if url.starts_with('/') {
        return Ok(url.to_string());
    }
    github_shorthand(url).ok_or_else(invalid)
}

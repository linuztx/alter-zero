//! `plugins.json`: the marketplaces the user has and the plugins they installed.
//!
//! The state file lives in the config home beside the plugins tree
//! ([`PLUGINS_DIR`]), and is everything startup needs: each installed
//! plugin's record carries the component paths found at install, so a
//! session never parses a manifest to know what a plugin adds
//! ([`enabled_plugins`]). See `docs/plugins.md` for the layout.
//!
//! Reading is lenient — every field but the record maps' shape is optional,
//! a field of the wrong type reads as absent, unknown keys are ignored — so a
//! file written by an older or newer build still loads. Only text that is not
//! a JSON object is an error, and [`update_state`] then refuses to write: a
//! file the user broke by hand still holds what they installed, and it waits
//! for the fix rather than being replaced by an empty one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::id::PluginId;
use super::source::{MarketplaceSource, safe_relative};

/// The state file's name in the config home.
pub const STATE_FILE: &str = "plugins.json";
/// The plugins tree's name in the config home.
pub const PLUGINS_DIR: &str = "plugins";
/// The folder inside an install that holds the files generated for it.
pub const GENERATED_DIR: &str = ".alter-zero-plugin";
/// The generated hooks file, in the form `hooks::HooksFile` reads.
pub const GENERATED_HOOKS: &str = "hooks.json";
/// The generated MCP file, in the form `mcp::parse_mcp_file` reads.
pub const GENERATED_MCP: &str = "mcp.json";

/// The shape version this build writes.
const STATE_VERSION: u32 = 1;

/// A marketplace the user has without adding it: known from the built-in
/// list (or `ALTER_ZERO_PLUGIN_MARKETPLACES`) and fetched the first time it
/// is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultMarketplace {
    /// The name its manifest is expected to carry, shown before it is
    /// fetched.
    pub name: Option<String>,
    pub source: MarketplaceSource,
}

/// `anthropics/claude-plugins-official` (`claude-plugins-official`) and
/// `openai/plugins` (`openai-curated`), in that order. Built on each call
/// rather than held in a static table: two small values, nothing resident.
#[must_use]
pub fn builtin_defaults() -> Vec<DefaultMarketplace> {
    [
        (
            "claude-plugins-official",
            "anthropics/claude-plugins-official",
        ),
        ("openai-curated", "openai/plugins"),
    ]
    .into_iter()
    .map(|(name, owner_repo)| DefaultMarketplace {
        name: Some(name.to_string()),
        source: MarketplaceSource::Git {
            url: format!("https://github.com/{owner_repo}.git"),
            reference: None,
        },
    })
    .collect()
}

/// The defaults as `ALTER_ZERO_PLUGIN_MARKETPLACES` sets them: unset → the
/// built-ins; `none`, `off`, `0` or blank → no defaults; else
/// whitespace-separated sources replacing them (the `*_DIR` convention), each
/// one a built-in names keeping that name, one that does not parse left out.
/// Pure: the caller passes the variable's value.
#[must_use]
pub fn defaults_from_env(
    value: Option<&str>,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<DefaultMarketplace> {
    let Some(value) = value else {
        return builtin_defaults();
    };
    let trimmed = value.trim();
    if trimmed.is_empty()
        || ["none", "off", "0"]
            .iter()
            .any(|off| trimmed.eq_ignore_ascii_case(off))
    {
        return Vec::new();
    }
    let builtins = builtin_defaults();
    trimmed
        .split_whitespace()
        .filter_map(|text| MarketplaceSource::parse(text, cwd, home).ok())
        .map(|source| {
            let canonical = source.canonical();
            let name = builtins
                .iter()
                .find(|builtin| builtin.source.canonical() == canonical)
                .and_then(|builtin| builtin.name.clone());
            DefaultMarketplace { name, source }
        })
        .collect()
}

/// A fetched marketplace, under its manifest's name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketplaceRecord {
    /// [`MarketplaceSource::canonical`].
    #[serde(deserialize_with = "lenient")]
    pub source: String,
    /// The branch or tag asked for; absent = the remote's default.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// The commit last fetched.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// When it was fetched, in unix seconds.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub updated: Option<u64>,
}

/// One installed plugin, under its `name@marketplace` id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginRecord {
    /// Read as `true` when absent: a record with no answer is an install,
    /// and an install starts enabled.
    #[serde(deserialize_with = "lenient_enabled", default = "enabled_by_default")]
    pub enabled: bool,
    /// The manifest's or entry's version, else the first 12 characters of
    /// `revision`.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The commit (or an in-repo plugin's folder tree id) the copy was made
    /// from.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// When it was installed, in unix seconds.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub installed: Option<u64>,
    /// At most 200 characters, for `plugin list` without a catalog.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// [`super::manifest::ManifestFormat::as_str`].
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// For an `@local` install only: the directory it was copied from.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Skill paths relative to the install root: a skill directory or a
    /// directory of them (`""` is the root).
    #[serde(deserialize_with = "strings", skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Command paths: a directory walked for `*.md`, or one `.md` file.
    #[serde(deserialize_with = "strings", skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Agent paths: a directory walked for `*.md`, or one `.md` file.
    #[serde(deserialize_with = "strings", skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// The generated hooks file exists.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "is_false")]
    pub hooks: bool,
    /// The generated MCP file exists.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "is_false")]
    pub mcp: bool,
}

/// The whole state file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsFile {
    /// Always written as this build's version; read and otherwise ignored.
    #[serde(deserialize_with = "lenient", serialize_with = "write_version")]
    pub version: u32,
    /// Fetched marketplaces by name.
    #[serde(
        deserialize_with = "records",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub marketplaces: BTreeMap<String, MarketplaceRecord>,
    /// The canonical source of each default marketplace the user removed.
    #[serde(deserialize_with = "strings", skip_serializing_if = "Vec::is_empty")]
    pub removed_defaults: Vec<String>,
    /// Installed plugins, keyed by `name@marketplace`.
    #[serde(
        deserialize_with = "records",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub plugins: BTreeMap<String, PluginRecord>,
}

impl PluginsFile {
    /// The file's text as state. Empty or blank text is an empty file.
    ///
    /// # Errors
    /// When the text is not JSON, or not a JSON object.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_str(text).map_err(|e| {
            format!("{STATE_FILE} is not valid JSON ({e}) — fix it or move it aside")
        })?;
        if !value.is_object() {
            return Err(format!(
                "{STATE_FILE} is not a JSON object — fix it or move it aside"
            ));
        }
        Self::deserialize(value).map_err(|e| format!("{STATE_FILE} could not be read: {e}"))
    }

    /// The file's text: pretty, `"version": 1`, everything empty or absent
    /// left out, a trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// The record for `id`, if it is installed.
    #[must_use]
    pub fn plugin(&self, id: &PluginId) -> Option<&PluginRecord> {
        self.plugins.get(&id.to_string())
    }

    /// Nothing fetched, removed or installed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marketplaces.is_empty() && self.removed_defaults.is_empty() && self.plugins.is_empty()
    }
}

/// A marketplace the user has, whether or not it was ever fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownMarketplace {
    /// The manifest's name once fetched; a default's expected name before;
    /// else `None`.
    pub name: Option<String>,
    pub source: MarketplaceSource,
    pub reference: Option<String>,
    pub revision: Option<String>,
    pub updated: Option<u64>,
    /// One of the defaults (matched by canonical source).
    pub is_default: bool,
    /// Has a record in the file.
    pub fetched: bool,
}

/// The defaults that are neither recorded (matched by canonical source) nor
/// removed, then the recorded marketplaces in name order. A record whose
/// source no longer parses is left out.
#[must_use]
pub fn known_marketplaces(
    file: &PluginsFile,
    defaults: &[DefaultMarketplace],
) -> Vec<KnownMarketplace> {
    let root = Path::new("/");
    let recorded: Vec<(&String, &MarketplaceRecord, MarketplaceSource)> = file
        .marketplaces
        .iter()
        .filter_map(|(name, record)| {
            MarketplaceSource::parse(&record.source, root, None)
                .ok()
                .map(|source| (name, record, source))
        })
        .collect();
    let default_sources: Vec<String> = defaults.iter().map(|d| d.source.canonical()).collect();
    let mut known: Vec<KnownMarketplace> = defaults
        .iter()
        .zip(&default_sources)
        .filter(|(_, canonical)| {
            !file.removed_defaults.contains(canonical)
                && !recorded
                    .iter()
                    .any(|(_, _, source)| source.canonical() == **canonical)
        })
        .map(|(default, _)| KnownMarketplace {
            name: default.name.clone(),
            source: default.source.clone(),
            reference: None,
            revision: None,
            updated: None,
            is_default: true,
            fetched: false,
        })
        .collect();
    known.extend(
        recorded
            .into_iter()
            .map(|(name, record, source)| KnownMarketplace {
                name: Some(name.clone()),
                is_default: default_sources.contains(&source.canonical()),
                source,
                reference: record.r#ref.clone(),
                revision: record.revision.clone(),
                updated: record.updated,
                fetched: true,
            }),
    );
    known
}

/// The read-modify-write core: `existing` is the file's text as read **now**
/// (`""` for a missing file), `change` edits it, and the new text comes back
/// for the caller to write atomically.
///
/// # Errors
/// When `existing` will not parse — the caller refuses the write, and
/// `change` is never called.
pub fn update_state(
    existing: &str,
    change: impl FnOnce(&mut PluginsFile),
) -> Result<String, String> {
    let mut file = PluginsFile::parse(existing)?;
    change(&mut file);
    Ok(file.to_json())
}

/// `{config_home}/plugins.json`.
#[must_use]
pub fn state_path(config_home: &Path) -> PathBuf {
    config_home.join(STATE_FILE)
}

/// `{config_home}/plugins`.
#[must_use]
pub fn plugins_root(config_home: &Path) -> PathBuf {
    config_home.join(PLUGINS_DIR)
}

/// `{plugins_root}/marketplaces/<marketplace>` — a marketplace's checkout.
#[must_use]
pub fn marketplace_dir(plugins_root: &Path, marketplace: &str) -> PathBuf {
    plugins_root.join("marketplaces").join(marketplace)
}

/// `{plugins_root}/cache/<marketplace>/<plugin>` — an install. No version
/// segment: hook trust is keyed on the hooks file's path, so a stable path
/// re-asks after an update only for hooks that changed.
#[must_use]
pub fn install_dir(plugins_root: &Path, id: &PluginId) -> PathBuf {
    plugins_root
        .join("cache")
        .join(&id.marketplace)
        .join(&id.name)
}

/// `{plugins_root}/data/<plugin>-<marketplace>` — the plugin's data
/// directory, kept across updates.
#[must_use]
pub fn data_dir(plugins_root: &Path, id: &PluginId) -> PathBuf {
    plugins_root
        .join("data")
        .join(format!("{}-{}", id.name, id.marketplace))
}

/// `{plugins_root}/tmp` — staging for clones and atomic installs.
#[must_use]
pub fn staging_dir(plugins_root: &Path) -> PathBuf {
    plugins_root.join("tmp")
}

/// One enabled plugin as the session uses it: absolute paths, nothing
/// parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledPlugin {
    pub id: PluginId,
    pub root: PathBuf,
    pub data: PathBuf,
    pub skills: Vec<PathBuf>,
    pub commands: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
    /// `root/.alter-zero-plugin/hooks.json` when the record says `hooks`.
    pub hooks_file: Option<PathBuf>,
    /// `root/.alter-zero-plugin/mcp.json` when the record says `mcp`.
    pub mcp_file: Option<PathBuf>,
}

/// The enabled plugins in id order. A record whose id will not parse, or one
/// of whose recorded paths is not [`safe_relative`] (`""`, the root, is
/// fine), is skipped whole.
#[must_use]
pub fn enabled_plugins(file: &PluginsFile, plugins_root: &Path) -> Vec<EnabledPlugin> {
    file.plugins
        .iter()
        .filter(|(_, record)| record.enabled)
        .filter_map(|(key, record)| {
            let id = PluginId::parse(key).ok()?;
            let root = install_dir(plugins_root, &id);
            let join = |paths: &[String]| -> Option<Vec<PathBuf>> {
                paths
                    .iter()
                    .map(|path| {
                        if path.is_empty() {
                            Some(root.clone())
                        } else {
                            safe_relative(path).map(|path| root.join(path))
                        }
                    })
                    .collect()
            };
            let generated = root.join(GENERATED_DIR);
            Some(EnabledPlugin {
                skills: join(&record.skills)?,
                commands: join(&record.commands)?,
                agents: join(&record.agents)?,
                hooks_file: record.hooks.then(|| generated.join(GENERATED_HOOKS)),
                mcp_file: record.mcp.then(|| generated.join(GENERATED_MCP)),
                data: data_dir(plugins_root, &id),
                root,
                id,
            })
        })
        .collect()
}

// ===== lenient reading =====

/// Any value: the field's type when it is one, else its default.
fn lenient<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// `enabled`: a boolean, else `true`.
fn lenient_enabled<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Value::deserialize(d)?.as_bool().unwrap_or(true))
}

fn enabled_by_default() -> bool {
    true
}

/// The strings of an array; anything else is empty.
fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// An object of records, each that is an object kept; anything else empty.
fn records<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    d: D,
) -> Result<BTreeMap<String, T>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Object(entries) => entries
            .into_iter()
            .filter(|(_, value)| value.is_object())
            .filter_map(|(key, value)| T::deserialize(value).ok().map(|record| (key, record)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

fn write_version<S: Serializer>(_: &u32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u32(STATE_VERSION)
}

/// `skip_serializing_if` hands the field over by reference.
fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const SAMPLE: &str = r#"{
  "version": 1,
  "marketplaces": {
    "claude-plugins-official": {
      "source": "anthropics/claude-plugins-official",
      "ref": null,
      "revision": "d7d32797e7e02708d0bb00ffe8b8f2397dc1aea6",
      "updated": 1763643000
    },
    "team": { "source": "/home/me/work/team-plugins" }
  },
  "removed_defaults": ["openai/plugins"],
  "plugins": {
    "code-review@claude-plugins-official": {
      "enabled": true,
      "version": "1.0.0",
      "revision": "9fceb02d0ae598e95dc970b74767f19372d61af8",
      "installed": 1763643000,
      "description": "Automated code review for pull requests",
      "format": "claude",
      "skills": ["skills"],
      "commands": ["commands"],
      "agents": ["agents"],
      "hooks": true,
      "mcp": true
    },
    "notes@local": { "enabled": true, "source": "/home/me/dev/notes", "skills": ["skills"] }
  }
}"#;

    fn id(text: &str) -> PluginId {
        PluginId::parse(text).unwrap()
    }

    // ===== defaults =====

    #[test]
    fn the_built_in_defaults_are_claudes_then_openais() {
        let defaults = builtin_defaults();
        let pairs: Vec<_> = defaults
            .iter()
            .map(|d| (d.name.as_deref().unwrap(), d.source.canonical()))
            .collect();
        assert_eq!(
            pairs,
            [
                (
                    "claude-plugins-official",
                    "anthropics/claude-plugins-official".to_string()
                ),
                ("openai-curated", "openai/plugins".to_string()),
            ]
        );
    }

    #[test]
    fn the_environment_can_turn_the_defaults_off_or_replace_them() {
        let cwd = Path::new("/w");
        let home = Some(Path::new("/home/me"));
        assert_eq!(defaults_from_env(None, cwd, home), builtin_defaults());
        for off in ["", "  ", "none", "OFF", "0"] {
            assert!(
                defaults_from_env(Some(off), cwd, home).is_empty(),
                "{off:?}"
            );
        }
        let replaced = defaults_from_env(Some(" team/plugins  ~/mine  not-a-source "), cwd, home);
        let sources: Vec<_> = replaced.iter().map(|d| d.source.canonical()).collect();
        assert_eq!(sources, ["team/plugins", "/home/me/mine"]);
        assert!(replaced.iter().all(|d| d.name.is_none()));
        // A built-in named there keeps its expected name.
        let named = defaults_from_env(Some("openai/plugins"), cwd, home);
        assert_eq!(named[0].name.as_deref(), Some("openai-curated"));
    }

    // ===== the file =====

    #[test]
    fn the_sample_file_parses() {
        let file = PluginsFile::parse(SAMPLE).unwrap();
        assert_eq!(file.version, 1);
        assert_eq!(
            file.marketplaces["team"].source,
            "/home/me/work/team-plugins"
        );
        assert_eq!(
            file.marketplaces["claude-plugins-official"].updated,
            Some(1_763_643_000)
        );
        assert_eq!(file.removed_defaults, ["openai/plugins"]);
        let record = file
            .plugin(&id("code-review@claude-plugins-official"))
            .unwrap();
        assert!(record.enabled && record.hooks && record.mcp);
        assert_eq!(record.format.as_deref(), Some("claude"));
        assert_eq!(
            file.plugin(&id("notes@local")).unwrap().source.as_deref(),
            Some("/home/me/dev/notes")
        );
        assert!(file.plugin(&id("nope@local")).is_none());
    }

    #[test]
    fn empty_text_is_an_empty_file() {
        for text in ["", "  \n"] {
            let file = PluginsFile::parse(text).unwrap();
            assert!(file.is_empty());
        }
        assert!(!PluginsFile::parse(SAMPLE).unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_naming_the_file() {
        let err = PluginsFile::parse("{\"plugins\": ").unwrap_err();
        assert!(err.contains(STATE_FILE), "{err}");
        assert!(PluginsFile::parse("[1]").is_err());
    }

    #[test]
    fn an_older_or_newer_shape_reads_leniently() {
        let file = PluginsFile::parse(
            r#"{"version": 7, "future": {"x": 1},
                "marketplaces": {"m": {"source": "o/r", "ref": 5, "extra": true}, "bad": 3},
                "removed_defaults": "openai/plugins",
                "plugins": {"a@m": {"skills": ["s", 2], "hooks": "yes"}, "b@m": "broken"}}"#,
        )
        .unwrap();
        assert_eq!(file.marketplaces["m"].source, "o/r");
        assert_eq!(file.marketplaces["m"].r#ref, None);
        assert!(!file.marketplaces.contains_key("bad"));
        assert!(file.removed_defaults.is_empty());
        let a = &file.plugins["a@m"];
        assert!(
            a.enabled,
            "a record with no `enabled` is an enabled install"
        );
        assert_eq!(a.skills, ["s"]);
        assert!(!a.hooks);
        assert!(!file.plugins.contains_key("b@m"));
    }

    #[test]
    fn to_json_is_pretty_versioned_and_omits_what_is_empty() {
        let mut file = PluginsFile::default();
        file.marketplaces.insert(
            "team".into(),
            MarketplaceRecord {
                source: "/t".into(),
                ..Default::default()
            },
        );
        file.plugins.insert(
            "notes@local".into(),
            PluginRecord {
                enabled: false,
                skills: vec!["skills".into()],
                ..Default::default()
            },
        );
        let text = file.to_json();
        assert!(text.ends_with("}\n"), "{text}");
        assert!(text.contains("\n  \"version\": 1"), "{text}");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            json!({"version": 1, "marketplaces": {"team": {"source": "/t"}},
                   "plugins": {"notes@local": {"enabled": false, "skills": ["skills"]}}})
        );
        assert_eq!(PluginsFile::default().to_json(), "{\n  \"version\": 1\n}\n");
    }

    #[test]
    fn the_sample_round_trips() {
        let file = PluginsFile::parse(SAMPLE).unwrap();
        assert_eq!(PluginsFile::parse(&file.to_json()).unwrap(), file);
    }

    #[test]
    fn update_state_is_a_read_modify_write_over_the_text() {
        let text = update_state("", |file| {
            file.removed_defaults.push("openai/plugins".into());
        })
        .unwrap();
        let text = update_state(&text, |file| {
            file.plugins.insert(
                "x@m".into(),
                PluginRecord {
                    enabled: true,
                    ..Default::default()
                },
            );
        })
        .unwrap();
        let file = PluginsFile::parse(&text).unwrap();
        assert_eq!(file.removed_defaults, ["openai/plugins"]);
        assert!(file.plugins["x@m"].enabled);
    }

    #[test]
    fn update_state_refuses_a_file_that_will_not_parse() {
        let mut called = false;
        assert!(update_state("{nope", |_| called = true).is_err());
        assert!(!called);
    }

    // ===== known marketplaces =====

    #[test]
    fn known_marketplaces_lists_unfetched_defaults_then_the_recorded_ones() {
        let mut file = PluginsFile::default();
        file.marketplaces.insert(
            "team".into(),
            MarketplaceRecord {
                source: "/srv/team".into(),
                r#ref: Some("dev".into()),
                ..Default::default()
            },
        );
        let known = known_marketplaces(&file, &builtin_defaults());
        let rows: Vec<_> = known
            .iter()
            .map(|k| {
                (
                    k.name.as_deref(),
                    k.source.canonical(),
                    k.is_default,
                    k.fetched,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    Some("claude-plugins-official"),
                    "anthropics/claude-plugins-official".into(),
                    true,
                    false
                ),
                (Some("openai-curated"), "openai/plugins".into(), true, false),
                (Some("team"), "/srv/team".into(), false, true),
            ]
        );
        assert_eq!(known[2].reference.as_deref(), Some("dev"));
    }

    #[test]
    fn a_fetched_default_is_listed_once_as_recorded_and_a_removed_one_not_at_all() {
        let file = PluginsFile::parse(SAMPLE).unwrap();
        let known = known_marketplaces(&file, &builtin_defaults());
        let names: Vec<_> = known.iter().map(|k| k.name.clone().unwrap()).collect();
        assert_eq!(names, ["claude-plugins-official", "team"]);
        assert!(known[0].is_default && known[0].fetched);
        assert_eq!(
            known[0].revision.as_deref(),
            Some("d7d32797e7e02708d0bb00ffe8b8f2397dc1aea6")
        );
        assert!(!known[1].is_default);
    }

    // ===== paths =====

    #[test]
    fn the_layout_paths() {
        let home = Path::new("/home/me/.alter-zero");
        let root = plugins_root(home);
        let id = id("code-review@claude-plugins-official");
        assert_eq!(
            state_path(home),
            Path::new("/home/me/.alter-zero/plugins.json")
        );
        assert_eq!(root, Path::new("/home/me/.alter-zero/plugins"));
        assert_eq!(
            marketplace_dir(&root, "team"),
            root.join("marketplaces/team")
        );
        assert_eq!(
            install_dir(&root, &id),
            root.join("cache/claude-plugins-official/code-review")
        );
        assert_eq!(
            data_dir(&root, &id),
            root.join("data/code-review-claude-plugins-official")
        );
        assert_eq!(staging_dir(&root), root.join("tmp"));
    }

    // ===== enabled plugins =====

    #[test]
    fn enabled_plugins_are_absolute_and_in_id_order() {
        let file = PluginsFile::parse(SAMPLE).unwrap();
        let root = Path::new("/c/plugins");
        let enabled = enabled_plugins(&file, root);
        let ids: Vec<_> = enabled.iter().map(|p| p.id.to_string()).collect();
        assert_eq!(ids, ["code-review@claude-plugins-official", "notes@local"]);
        let review = &enabled[0];
        let install = root.join("cache/claude-plugins-official/code-review");
        assert_eq!(review.root, install);
        assert_eq!(
            review.data,
            root.join("data/code-review-claude-plugins-official")
        );
        assert_eq!(review.skills, [install.join("skills")]);
        assert_eq!(review.agents, [install.join("agents")]);
        assert_eq!(
            review.hooks_file,
            Some(install.join(".alter-zero-plugin/hooks.json"))
        );
        assert_eq!(
            review.mcp_file,
            Some(install.join(".alter-zero-plugin/mcp.json"))
        );
        assert_eq!(enabled[1].hooks_file, None);
    }

    #[test]
    fn a_disabled_or_unreadable_record_is_skipped() {
        let file = PluginsFile::parse(
            r#"{"plugins": {
                "off@m": {"enabled": false},
                "bad id": {"enabled": true},
                "escape@m": {"enabled": true, "skills": ["../../etc"]},
                "root@m": {"enabled": true, "skills": [""]},
                "ok@m": {"enabled": true}}}"#,
        )
        .unwrap();
        let enabled = enabled_plugins(&file, Path::new("/p"));
        let ids: Vec<_> = enabled.iter().map(|p| p.id.to_string()).collect();
        assert_eq!(ids, ["ok@m", "root@m"]);
        assert_eq!(enabled[1].skills, [PathBuf::from("/p/cache/m/root")]);
    }
}

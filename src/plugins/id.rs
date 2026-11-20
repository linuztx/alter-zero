//! Plugin and marketplace names, the `name@marketplace` id, and the namespaced component names.
//!
//! See `docs/plugins.md`.

use std::fmt;
use std::path::{Path, PathBuf};

/// The synthetic marketplace a plugin installed straight from a directory
/// belongs to (`alter-zero plugin install ./my-plugin` → `my-plugin@local`).
/// No real marketplace may take the name.
pub const LOCAL_MARKETPLACE: &str = "local";

/// The longest a plugin or marketplace name may be.
pub const MAX_NAME_LEN: usize = 64;

/// Checks a plugin name: `[A-Za-z0-9_.-]`, 1..=64 characters, first and last
/// alphanumeric, no `..`.
///
/// A name is a path segment under the config home (`plugins/cache/<m>/<p>`)
/// and the namespace every component is listed under, so the rule is the
/// narrowest one the two real marketplaces and Codex's plugin ids all fit:
/// no separators, no spaces, nothing a shell or a path would read twice.
///
/// # Errors
/// A sentence naming the name and what is wrong with it.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    validate_name("plugin", name)
}

/// Checks a marketplace name by the same rules as a plugin name — it is the
/// other half of an id and a path segment too.
///
/// # Errors
/// A sentence naming the name and what is wrong with it.
pub fn validate_marketplace_name(name: &str) -> Result<(), String> {
    validate_name("marketplace", name)
}

/// The rule a plugin name breaks, as a clause for a sentence naming it —
/// `None` for a valid name. A marketplace's unreadable entries are reported
/// with it.
#[must_use]
pub(crate) fn plugin_name_rule(name: &str) -> Option<&'static str> {
    validate_plugin_name(name).err()?;
    Some(if name.is_empty() {
        "a plugin name is not empty"
    } else if name
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        "a plugin name is ASCII letters, digits, ., _ and - only"
    } else if name.chars().count() > MAX_NAME_LEN {
        "a plugin name is 64 characters at most"
    } else if name.contains("..")
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
    {
        "a plugin name may not hold .."
    } else {
        "a plugin name starts and ends with an ASCII letter or digit"
    })
}

/// The shared check, `kind` being the noun the error leads with.
fn validate_name(kind: &str, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{kind} name is empty"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        let what = if bad.is_whitespace() {
            "a space".to_string()
        } else {
            format!("`{bad}`")
        };
        return Err(format!(
            "{kind} name `{name}` has {what} in it — ASCII letters, digits, `.`, `_` and `-` only"
        ));
    }
    let len = name.chars().count();
    if len > MAX_NAME_LEN {
        return Err(format!(
            "{kind} name `{name}` is {len} characters long — {MAX_NAME_LEN} at most"
        ));
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return Err(format!(
            "{kind} name `{name}` must start and end with a letter or a digit"
        ));
    }
    if name.contains("..") {
        return Err(format!("{kind} name `{name}` has `..` in it"));
    }
    Ok(())
}

/// An installed plugin's identity: its name in its marketplace, written
/// `name@marketplace` the way Claude Code and Codex both write it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId {
    pub name: String,
    pub marketplace: String,
}

impl PluginId {
    /// An id from its two halves, both validated.
    ///
    /// # Errors
    /// The first half that is not a valid name.
    pub fn new(name: &str, marketplace: &str) -> Result<Self, String> {
        validate_plugin_name(name)?;
        validate_marketplace_name(marketplace)?;
        Ok(Self {
            name: name.to_string(),
            marketplace: marketplace.to_string(),
        })
    }

    /// `name@marketplace`, split on the **last** `@` (Codex's rule), both
    /// halves validated.
    ///
    /// # Errors
    /// When there is no `@`, or either half is not a valid name.
    pub fn parse(text: &str) -> Result<Self, String> {
        let Some((name, marketplace)) = text.rsplit_once('@') else {
            return Err(format!(
                "`{text}` is not a plugin id — write it as `plugin@marketplace`"
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

/// What the CLI takes where it wants a plugin: a bare name (looked up across
/// the marketplaces), a full id, or a directory to install from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSelector {
    Id(PluginId),
    Name(String),
    Directory(PathBuf),
}

impl PluginSelector {
    /// `./x`, `../x`, `/x`, `~/x`, `.` → [`PluginSelector::Directory`],
    /// resolved against `cwd` / `home` and not checked for existence; text
    /// holding `@` → [`PluginSelector::Id`]; anything else →
    /// [`PluginSelector::Name`]. Ids and names are validated.
    ///
    /// # Errors
    /// Blank text, `~/…` with no home, or an invalid name.
    pub fn parse(text: &str, cwd: &Path, home: Option<&Path>) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("no plugin given — name one, as `plugin` or `plugin@marketplace`".into());
        }
        if let Some(path) = local_path(text, cwd, home) {
            return path.map(Self::Directory);
        }
        if text.contains('@') {
            return PluginId::parse(text).map(Self::Id);
        }
        validate_plugin_name(text)?;
        Ok(Self::Name(text.to_string()))
    }
}

/// `text` as an absolute path when it is written as one — `.`, `..`, `~`,
/// or starting `./`, `../`, `/` or `~/` — lexically normalised (no `.`
/// segments, each `..` taking one off); `None` when it does not look like a
/// path at all. Shared with [`super::source`], whose marketplace sources take
/// a folder the same way.
pub(crate) fn local_path(
    text: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Option<Result<PathBuf, String>> {
    let looks_local = matches!(text, "." | ".." | "~")
        || ["./", "../", "/", "~/"].iter().any(|p| text.starts_with(p));
    if !looks_local {
        return None;
    }
    let (base, rest) = if text == "~" || text.starts_with("~/") {
        let Some(home) = home else {
            return Some(Err(format!(
                "`{text}` starts with `~` but there is no home directory to resolve it against"
            )));
        };
        (home.to_path_buf(), text.get(2..).unwrap_or(""))
    } else if text.starts_with('/') {
        (PathBuf::from("/"), text)
    } else {
        (cwd.to_path_buf(), text)
    };
    let mut path = base;
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            part => path.push(part),
        }
    }
    Some(Ok(path))
}

/// Every character outside `[A-Za-z0-9_-]` becomes `_` — the rule Claude
/// Code applies to the segments of an MCP server's wire name.
#[must_use]
pub fn sanitize_segment(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `plugin_<plugin>_<server>`, each half through [`sanitize_segment`] —
/// Claude Code's wire shape, so a hook matcher written for
/// `mcp__plugin_<p>_<s>__<tool>` matches here unchanged.
#[must_use]
pub fn mcp_server_name(plugin: &str, server: &str) -> String {
    format!(
        "plugin_{}_{}",
        sanitize_segment(plugin),
        sanitize_segment(server)
    )
}

/// What two plugin names must not share to be installed side by side: the
/// name in ASCII lowercase with `.` folded to `_`. Components are listed as
/// `plugin:component` and MCP servers as `plugin_<plugin>_<server>` (with
/// `.` folded), and a filesystem may ignore case — so `Dup`, `dup`, `a.b`
/// and `a_b` would shadow each other.
#[must_use]
pub fn namespace_key(name: &str) -> String {
    name.to_ascii_lowercase().replace('.', "_")
}

/// A version as it is shown: `v1.2.0` for a version, the text itself for
/// anything else. An install that names no version is recorded under the
/// first characters of its commit, and `v2de00883bd2b` reads as neither a
/// version nor a commit; one already written `v1.2.0` gets no second `v`.
#[must_use]
pub fn version_label(version: &str) -> String {
    let commit = version.len() >= 7 && version.bytes().all(|b| b.is_ascii_hexdigit());
    if version.starts_with(|c: char| c.is_ascii_digit()) && !commit {
        format!("v{version}")
    } else {
        version.to_string()
    }
}

/// `plugin:component` — how a plugin's skills, agents and commands are
/// named, so two plugins' `review` never collide.
#[must_use]
pub fn namespaced(plugin: &str, component: &str) -> String {
    format!("{plugin}:{component}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_shown_with_a_v_and_a_commit_without() {
        assert_eq!(version_label("1.0.1"), "v1.0.1");
        assert_eq!(version_label("7"), "v7");
        assert_eq!(version_label("2025.11"), "v2025.11");
        // Already written with one: never a second.
        assert_eq!(version_label("v2.0.0"), "v2.0.0");
        // An install that names no version is recorded under the first
        // characters of its commit, which is no version.
        assert_eq!(version_label("2de00883bd2b"), "2de00883bd2b");
        assert_eq!(version_label("0f487c9"), "0f487c9");
        assert_eq!(version_label("main"), "main");
    }

    // ===== names =====

    #[test]
    fn a_plugin_name_takes_letters_digits_dots_underscores_and_dashes() {
        for name in [
            "code-review",
            "a",
            "Box",
            "wordpress.com",
            "my_plugin2",
            "x9",
        ] {
            assert_eq!(validate_plugin_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn a_plugin_name_with_a_space_or_a_separator_is_refused_by_name() {
        let err = validate_plugin_name("my plugin").unwrap_err();
        assert!(err.contains("`my plugin`"), "{err}");
        assert!(err.contains("space"), "{err}");
        for name in ["a/b", "a\\b", "a@b", "a:b", "ü"] {
            assert!(validate_plugin_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn a_non_ascii_letter_is_refused_as_not_an_ascii_letter() {
        let err = validate_plugin_name("日本語").unwrap_err();
        assert!(err.contains("ASCII letters"), "{err}");
    }

    #[test]
    fn names_differing_by_case_or_by_dot_and_underscore_share_a_namespace() {
        assert_eq!(namespace_key("Dup"), namespace_key("dup"));
        assert_eq!(namespace_key("a.b"), namespace_key("a_b"));
        assert_ne!(namespace_key("a-b"), namespace_key("a_b"));
    }

    #[test]
    fn a_plugin_name_must_start_and_end_alphanumeric() {
        for name in ["-x", "x-", ".x", "x.", "_x", "x_", "."] {
            assert!(validate_plugin_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn a_plugin_name_may_not_hold_two_dots_in_a_row() {
        assert!(validate_plugin_name("a..b").is_err());
        assert_eq!(validate_plugin_name("a.b.c"), Ok(()));
    }

    #[test]
    fn a_plugin_name_is_one_to_sixty_four_characters() {
        assert!(validate_plugin_name("").is_err());
        assert_eq!(validate_plugin_name(&"a".repeat(MAX_NAME_LEN)), Ok(()));
        let err = validate_plugin_name(&"a".repeat(MAX_NAME_LEN + 1)).unwrap_err();
        assert!(err.contains("64"), "{err}");
    }

    #[test]
    fn a_marketplace_name_follows_the_same_rules_and_says_marketplace() {
        assert_eq!(validate_marketplace_name("claude-plugins-official"), Ok(()));
        assert_eq!(validate_marketplace_name(LOCAL_MARKETPLACE), Ok(()));
        let err = validate_marketplace_name("team plugins").unwrap_err();
        assert!(err.starts_with("marketplace name"), "{err}");
        assert!(validate_plugin_name("x").is_ok());
        assert!(
            validate_plugin_name("team plugins")
                .unwrap_err()
                .starts_with("plugin name")
        );
    }

    // ===== ids =====

    #[test]
    fn an_id_displays_as_name_at_marketplace() {
        let id = PluginId::new("code-review", "claude-plugins-official").unwrap();
        assert_eq!(id.to_string(), "code-review@claude-plugins-official");
    }

    #[test]
    fn an_id_validates_both_halves() {
        assert!(PluginId::new("bad name", "m").is_err());
        assert!(PluginId::new("p", "bad name").is_err());
    }

    #[test]
    fn an_id_splits_on_the_last_at() {
        let id = PluginId::parse("notes@local").unwrap();
        assert_eq!(id, PluginId::new("notes", "local").unwrap());
        // A name cannot hold `@`, so the split being on the last one only
        // matters for the error: the left half is what is refused.
        let err = PluginId::parse("a@b@c").unwrap_err();
        assert!(err.contains("`a@b`"), "{err}");
    }

    #[test]
    fn an_id_without_an_at_or_with_an_empty_half_is_refused() {
        assert!(PluginId::parse("notes").is_err());
        assert!(PluginId::parse("@local").is_err());
        assert!(PluginId::parse("notes@").is_err());
    }

    // ===== selectors =====

    #[test]
    fn a_selector_that_looks_like_a_path_is_a_directory() {
        let cwd = Path::new("/work/repo");
        let home = Some(Path::new("/home/me"));
        let dir = |text: &str| match PluginSelector::parse(text, cwd, home).unwrap() {
            PluginSelector::Directory(path) => path,
            other => panic!("{text}: {other:?}"),
        };
        assert_eq!(dir("./my-plugin"), PathBuf::from("/work/repo/my-plugin"));
        assert_eq!(dir("../other"), PathBuf::from("/work/other"));
        assert_eq!(dir("/abs/p"), PathBuf::from("/abs/p"));
        assert_eq!(dir("~/dev/notes"), PathBuf::from("/home/me/dev/notes"));
        assert_eq!(dir("."), PathBuf::from("/work/repo"));
    }

    #[test]
    fn a_home_relative_selector_needs_a_home() {
        assert!(PluginSelector::parse("~/x", Path::new("/w"), None).is_err());
    }

    #[test]
    fn a_selector_with_an_at_is_an_id_and_otherwise_a_name() {
        let cwd = Path::new("/w");
        assert_eq!(
            PluginSelector::parse("code-review@claude-plugins-official", cwd, None).unwrap(),
            PluginSelector::Id(PluginId::new("code-review", "claude-plugins-official").unwrap())
        );
        assert_eq!(
            PluginSelector::parse("code-review", cwd, None).unwrap(),
            PluginSelector::Name("code-review".into())
        );
        assert!(PluginSelector::parse("bad name", cwd, None).is_err());
        assert!(PluginSelector::parse("  ", cwd, None).is_err());
    }

    // ===== component names =====

    #[test]
    fn a_segment_keeps_only_letters_digits_underscores_and_dashes() {
        assert_eq!(sanitize_segment("my-plugin_2"), "my-plugin_2");
        assert_eq!(sanitize_segment("wordpress.com"), "wordpress_com");
        assert_eq!(sanitize_segment("a b:c/é"), "a_b_c__");
    }

    #[test]
    fn an_mcp_server_name_is_claude_codes_wire_shape() {
        assert_eq!(mcp_server_name("github", "api"), "plugin_github_api");
        assert_eq!(
            mcp_server_name("wordpress.com", "my server"),
            "plugin_wordpress_com_my_server"
        );
    }

    #[test]
    fn a_namespaced_component_is_plugin_colon_component() {
        assert_eq!(
            namespaced("code-review", "review-pr"),
            "code-review:review-pr"
        );
    }
}

//! Plugin and marketplace names, and the `plugin@marketplace` id a plugin is
//! installed, enabled and recorded under (`docs/plugins.md`).

use std::fmt;

/// The longest a plugin's or a marketplace's name may be — the Agent Plugins
/// bound (§5.5), applied to every format.
pub const MAX_NAME_LEN: usize = 64;

/// What separates a plugin's name from its marketplace's in an id.
pub const ID_SEPARATOR: char = '@';

/// What separates a plugin's name from one of its components' — the skill
/// `hookify:writing-rules`, the agent `feature-dev:code-explorer`.
pub const NAMESPACE_SEPARATOR: char = ':';

/// Is `name` usable as one half of a plugin id — a plugin's name or a
/// marketplace's? ASCII letters, digits, `.`, `_` and `-`, opening on a
/// letter or a digit, at most [`MAX_NAME_LEN`] long, with no `..`.
///
/// Both halves become directory names under the plugins root and one half
/// becomes the prefix of every skill and agent the plugin adds, so the rule
/// is the intersection of what a path component, a `$` mention and both
/// references' own checks accept.
///
/// # Errors
/// The reason the name was refused.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!("longer than {MAX_NAME_LEN} characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("only letters, digits, `.`, `_` and `-` are allowed".to_string());
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err("must start with a letter or a digit".to_string());
    }
    if name.contains("..") {
        return Err("must not contain `..`".to_string());
    }
    Ok(())
}

/// One installable plugin: its marketplace entry's name and the marketplace
/// listing it. Two marketplaces may list a plugin of one name, so the pair
/// is the identity — the key in `plugins.json`, what `alter-zero plugin
/// install` takes, and the two directory names under `plugins/installed/`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId {
    /// The marketplace entry's name.
    pub name: String,
    /// The marketplace's own name.
    pub marketplace: String,
}

impl PluginId {
    /// The id of `name` in `marketplace`.
    ///
    /// # Errors
    /// Which half was refused, and why ([`validate_name`]).
    pub fn new(name: &str, marketplace: &str) -> Result<Self, String> {
        validate_name(name).map_err(|reason| format!("invalid plugin name `{name}`: {reason}"))?;
        validate_name(marketplace)
            .map_err(|reason| format!("invalid marketplace name `{marketplace}`: {reason}"))?;
        Ok(Self {
            name: name.to_string(),
            marketplace: marketplace.to_string(),
        })
    }

    /// Read a `plugin@marketplace` key.
    ///
    /// # Errors
    /// A key with no `@`, or with a half [`validate_name`] refuses.
    pub fn parse(key: &str) -> Result<Self, String> {
        let Some((name, marketplace)) = key.rsplit_once(ID_SEPARATOR) else {
            return Err(format!(
                "invalid plugin id `{key}`: expected <plugin>@<marketplace>"
            ));
        };
        Self::new(name, marketplace)
    }

    /// The `plugin@marketplace` key.
    #[must_use]
    pub fn key(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{ID_SEPARATOR}{}", self.name, self.marketplace)
    }
}

/// A component's name under its plugin's: `hookify` + `writing-rules` is
/// `hookify:writing-rules`. `segments` is the component's own path — one
/// name for a skill, several for a command or an agent in a subdirectory
/// (`db`, `migrate`).
#[must_use]
pub fn namespaced(plugin: &str, segments: &[&str]) -> String {
    let mut name = plugin.to_string();
    for segment in segments {
        name.push(NAMESPACE_SEPARATOR);
        name.push_str(segment);
    }
    name
}

/// Is `segment` usable as one segment of a component's name — letters,
/// digits, `_` and `-`, non-empty, at most [`MAX_NAME_LEN`] long? Looser than
/// a user skill's own rule (`skills::validate_skill_name`) by the upper
/// case: a command file is named by its author's habit, and the lookup folds
/// case anyway.
#[must_use]
pub fn is_component_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.chars().count() <= MAX_NAME_LEN
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_letters_digits_dots_underscores_and_hyphens() {
        for name in [
            "hookify",
            "acme.tools",
            "lint3r",
            "a",
            "my_plugin",
            "Team-Tools",
            "9lives",
        ] {
            assert_eq!(validate_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn a_name_that_is_not_a_safe_path_component_is_refused() {
        for name in [
            "",
            "has space",
            "a/b",
            "a\\b",
            "plugin@market",
            "plugin:skill",
            ".hidden",
            "-flag",
            "_under",
            "up..dir",
            "..",
            "é",
        ] {
            assert!(validate_name(name).is_err(), "{name:?} must be refused");
        }
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN)).is_ok());
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn an_id_is_the_plugin_at_the_marketplace() {
        let id = PluginId::parse("hookify@claude-plugins-official").unwrap();
        assert_eq!(id.name, "hookify");
        assert_eq!(id.marketplace, "claude-plugins-official");
        assert_eq!(id.key(), "hookify@claude-plugins-official");
        assert_eq!(id.to_string(), "hookify@claude-plugins-official");
    }

    #[test]
    fn an_id_needs_both_halves_and_both_valid() {
        assert!(
            PluginId::parse("hookify")
                .unwrap_err()
                .contains("<plugin>@<marketplace>")
        );
        assert!(
            PluginId::parse("@market")
                .unwrap_err()
                .contains("plugin name")
        );
        assert!(
            PluginId::parse("plugin@")
                .unwrap_err()
                .contains("marketplace name")
        );
        assert!(PluginId::parse("a b@market").is_err());
        assert!(PluginId::new("ok", "../up").is_err());
    }

    #[test]
    fn a_component_is_named_under_its_plugin() {
        assert_eq!(
            namespaced("hookify", &["writing-rules"]),
            "hookify:writing-rules"
        );
        assert_eq!(namespaced("tools", &["db", "migrate"]), "tools:db:migrate");
        assert_eq!(namespaced("solo", &[]), "solo");
    }

    #[test]
    fn a_component_segment_is_one_plain_word() {
        for segment in ["commit", "Commit-Push-PR", "code_review", "v2"] {
            assert!(is_component_segment(segment), "{segment}");
        }
        for segment in ["", "has space", "a.b", "a:b", "a/b", "é"] {
            assert!(!is_component_segment(segment), "{segment:?}");
        }
        assert!(!is_component_segment(&"a".repeat(MAX_NAME_LEN + 1)));
    }
}

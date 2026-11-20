//! The plugin-root and plugin-data variables a plugin's files may name.
//!
//! Claude Code spells them `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}`,
//! agent-plugins.org and Codex `${PLUGIN_ROOT}` / `${PLUGIN_DATA}`; an
//! installed plugin may use either, so both are answered. Expansion is one
//! textual pass that never re-reads what it wrote, and every other `${…}` is
//! left exactly as written — `${GITHUB_TOKEN}` and `${VAR:-default}` belong
//! to the MCP loader's own expansion, which runs later against the session's
//! environment. See `docs/plugins.md`.

use std::path::Path;

/// Where an installed plugin lives and where it keeps its data — the two
/// values the variables stand for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginPaths<'a> {
    /// The install root (`plugins/cache/<marketplace>/<plugin>`).
    pub root: &'a Path,
    /// The data directory kept across updates (`plugins/data/<plugin>-<marketplace>`).
    pub data: &'a Path,
}

/// `${CLAUDE_PLUGIN_ROOT}`, `${PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` and
/// `${PLUGIN_DATA}` replaced by the plugin's paths — a single pass,
/// non-recursive, every other placeholder left literal. Used where code
/// runs: hook commands and MCP server configs.
#[must_use]
pub fn expand(text: &str, paths: &PluginPaths<'_>) -> String {
    let root = paths.root.to_string_lossy();
    let data = paths.data.to_string_lossy();
    replace_names(
        text,
        &[
            ("CLAUDE_PLUGIN_ROOT", &root),
            ("PLUGIN_ROOT", &root),
            ("CLAUDE_PLUGIN_DATA", &data),
            ("PLUGIN_DATA", &data),
        ],
    )
}

/// Only the `CLAUDE_*` pair — what a skill, command or agent body may name.
/// The bare `${PLUGIN_ROOT}` is left alone there: in prose it is as likely
/// to be an example of the variable as a use of it, and Claude Code expands
/// only its own spelling in bodies.
#[must_use]
pub fn expand_claude(text: &str, paths: &PluginPaths<'_>) -> String {
    let root = paths.root.to_string_lossy();
    let data = paths.data.to_string_lossy();
    replace_names(
        text,
        &[("CLAUDE_PLUGIN_ROOT", &root), ("CLAUDE_PLUGIN_DATA", &data)],
    )
}

/// The environment a plugin's processes are given, in the order they are
/// set — agent-plugins.org requires `PLUGIN_ROOT`/`PLUGIN_DATA` in every
/// subprocess, and Claude Code's names ride along for scripts written
/// against them.
#[must_use]
pub fn env_pairs(paths: &PluginPaths<'_>) -> [(&'static str, String); 4] {
    let root = paths.root.to_string_lossy().into_owned();
    let data = paths.data.to_string_lossy().into_owned();
    [
        ("PLUGIN_ROOT", root.clone()),
        ("CLAUDE_PLUGIN_ROOT", root),
        ("PLUGIN_DATA", data.clone()),
        ("CLAUDE_PLUGIN_DATA", data),
    ]
}

/// `text` single-quoted for `sh` (`'` written as `'\''`), so a path with a
/// space or a quote in it reaches the program as one word.
#[must_use]
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// One left-to-right pass replacing `${NAME}` for each listed name. Text the
/// pass has written is never looked at again, so a path that itself contains
/// `${PLUGIN_ROOT}` stays as it is.
fn replace_names(text: &str, names: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let known = after.find('}').and_then(|close| {
            let name = &after[..close];
            names
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, value)| (close, *value))
        });
        match known {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> PluginPaths<'static> {
        PluginPaths {
            root: Path::new("/p/root"),
            data: Path::new("/p/data"),
        }
    }

    #[test]
    fn expand_answers_both_spellings_of_both_variables() {
        assert_eq!(
            expand(
                "${CLAUDE_PLUGIN_ROOT} ${PLUGIN_ROOT} ${CLAUDE_PLUGIN_DATA} ${PLUGIN_DATA}",
                &paths()
            ),
            "/p/root /p/root /p/data /p/data"
        );
    }

    #[test]
    fn expand_leaves_every_other_placeholder_literal() {
        let text =
            "Bearer ${GITHUB_TOKEN} ${TELEGRAM_STATE_DIR:-/tmp/x} $PLUGIN_ROOT ${PLUGIN_ROOT";
        assert_eq!(expand(text, &paths()), text);
    }

    #[test]
    fn expand_is_a_single_non_recursive_pass() {
        let p = PluginPaths {
            root: Path::new("/odd/${PLUGIN_DATA}"),
            data: Path::new("/d"),
        };
        assert_eq!(expand("${PLUGIN_ROOT}/x", &p), "/odd/${PLUGIN_DATA}/x");
        // A dollar in front of a placeholder stays a dollar.
        assert_eq!(expand("$${PLUGIN_DATA}", &p), "$/d");
    }

    #[test]
    fn expand_claude_leaves_the_bare_spelling() {
        assert_eq!(
            expand_claude(
                "${CLAUDE_PLUGIN_ROOT} ${PLUGIN_ROOT} ${CLAUDE_PLUGIN_DATA}",
                &paths()
            ),
            "/p/root ${PLUGIN_ROOT} /p/data"
        );
    }

    #[test]
    fn env_pairs_are_the_four_names_in_order() {
        assert_eq!(
            env_pairs(&paths()),
            [
                ("PLUGIN_ROOT", "/p/root".to_string()),
                ("CLAUDE_PLUGIN_ROOT", "/p/root".to_string()),
                ("PLUGIN_DATA", "/p/data".to_string()),
                ("CLAUDE_PLUGIN_DATA", "/p/data".to_string()),
            ]
        );
    }

    #[test]
    fn sh_quote_survives_spaces_and_quotes() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[cfg(unix)]
    #[test]
    fn sh_quote_round_trips_through_sh() {
        let text = "it's a \"path\" with $HOME and `ticks`";
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", sh_quote(text)))
            .output()
            .expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout), text);
    }
}

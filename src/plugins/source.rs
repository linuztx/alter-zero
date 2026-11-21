//! What a user types for a marketplace (`docs/plugins.md` *Adding one*):
//! GitHub's `owner/repo` shorthand, a git URL, or a path to a directory —
//! codex's grammar, with Claude Code's `@ref` on the shorthand.

use std::path::{Component, Path, PathBuf};

use super::marketplace::{checked_ref, normalize_git_url};
use super::registry::MarketplaceSource;

/// What the page and the CLI say a source may be.
pub const SOURCE_FORMS: &str = "owner/repo, a git URL, or a path to a directory";

/// Does `text` read as a path on this machine rather than a repository?
/// `./x`, `../x`, `/x`, `~/x`, and the bare `.`, `..` and `~`.
fn is_local_path(text: &str) -> bool {
    matches!(text, "." | ".." | "~")
        || text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with('/')
        || text.starts_with("~/")
}

/// `path` with its `.` and `..` components resolved by name alone — no
/// filesystem read, so a path that does not exist yet still has a spelling.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Read what the user typed for a marketplace. A relative path is taken
/// from `cwd`, and `~` from `home`.
///
/// # Errors
/// Text in none of the accepted forms, a `#ref` on a directory, or a ref
/// that is not a branch or tag name.
pub fn parse_marketplace_source(
    text: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<MarketplaceSource, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("a marketplace source is {SOURCE_FORMS}"));
    }

    if is_local_path(text) {
        if text.contains('#') {
            return Err("a `#ref` only applies to a git source, not a directory".to_string());
        }
        let path = if text == "~" || text.starts_with("~/") {
            let home =
                home.ok_or_else(|| "there is no home directory to read `~` from".to_string())?;
            home.join(text.trim_start_matches('~').trim_start_matches('/'))
        } else {
            cwd.join(text)
        };
        return Ok(MarketplaceSource::Local {
            path: lexical(&path).display().to_string(),
        });
    }

    // `#ref` pins any git source; `@ref` only the shorthand, where an `@`
    // cannot be a user name.
    let (base, ref_name) = match text.rsplit_once('#') {
        Some((base, ref_name)) => (base, Some(ref_name)),
        None if !text.contains(':') => match text.rsplit_once('@') {
            Some((base, ref_name)) => (base, Some(ref_name)),
            None => (text, None),
        },
        None => (text, None),
    };
    let url = normalize_git_url(base)
        .map_err(|_| format!("`{text}` is not a marketplace source: use {SOURCE_FORMS}"))?;
    let ref_name = checked_ref(
        ref_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
    )?;
    Ok(MarketplaceSource::Git { url, ref_name })
}

/// A marketplace `/plugins` offers on its add tab, so nobody types its
/// source to get it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceSuggestion {
    /// What adding it fetches — the text the add field takes.
    pub source: &'static str,
    /// The name its catalog gives, which tells whether it is added.
    pub name: &'static str,
    /// What it is, in a few words.
    pub description: &'static str,
}

/// The official catalogs of the two clients whose plugin formats this app
/// reads: Claude Code's and codex's.
pub const SUGGESTED_MARKETPLACES: [MarketplaceSuggestion; 2] = [
    MarketplaceSuggestion {
        source: "anthropics/claude-plugins-official",
        name: "claude-plugins-official",
        description: "Claude Code's official plugin directory",
    },
    MarketplaceSuggestion {
        source: "openai/plugins",
        name: "openai-curated",
        description: "Codex's official plugin directory",
    },
];

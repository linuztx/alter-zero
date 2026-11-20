//! What the user types to add a marketplace (`docs/plugins.md`
//! *Marketplaces*): `owner/repo`, a git URL (`https://`, `http://`,
//! `ssh://`, `file://`, `git@host:path`) or a local folder (`/abs`,
//! `./rel`, `../rel`, `~/…`), with an optional `@ref` / `#ref` on a git
//! source. Pure over an injected working directory and home, so a typo is an
//! error before anything is fetched.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Where GitHub `owner/repo` shorthand is cloned from.
pub const GITHUB_BASE: &str = "https://github.com/";

/// Where a marketplace comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceSource {
    /// `owner/repo` on GitHub.
    GitHub {
        /// The account or organisation.
        owner: String,
        /// The repository.
        repo: String,
    },
    /// Any other git remote, as typed.
    Git {
        /// The URL git is handed.
        url: String,
    },
    /// A folder on this machine, used in place.
    Local {
        /// The absolute, lexically normalised folder.
        path: PathBuf,
    },
}

/// A parsed marketplace source and the ref a git source follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSpec {
    /// Where it comes from.
    pub source: MarketplaceSource,
    /// The branch or tag to follow (git only).
    pub git_ref: Option<String>,
}

/// Why a source would not parse — a sentence to show as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

fn is_repo_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `owner/repo` split in two, when `text` is exactly that.
#[must_use]
pub fn github_shorthand(text: &str) -> Option<(&str, &str)> {
    let (owner, repo) = text.split_once('/')?;
    (is_repo_segment(owner) && is_repo_segment(repo)).then_some((owner, repo))
}

fn is_local(text: &str) -> bool {
    text.starts_with('/')
        || text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with("~/")
        || matches!(text, "." | ".." | "~")
}

fn is_url(text: &str) -> bool {
    ["https://", "http://", "ssh://", "file://"]
        .iter()
        .any(|scheme| text.starts_with(scheme))
}

/// `user@host:path` — git's scp-like ssh syntax.
fn is_scp_like(text: &str) -> bool {
    let Some((user_host, path)) = text.split_once(':') else {
        return false;
    };
    let Some((user, host)) = user_host.split_once('@') else {
        return false;
    };
    !user.is_empty() && !host.is_empty() && !host.contains('/') && !path.is_empty()
}

/// `path` with `.` and `..` folded away, without touching the filesystem.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Parse what the user typed. `explicit_ref` (`--ref`) wins over an `@ref`
/// / `#ref` suffix; a ref on a local folder is refused.
///
/// # Errors
/// An empty or unrecognised source, a ref on a folder, or `~` with no home.
pub fn parse_marketplace_source(
    input: &str,
    explicit_ref: Option<&str>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<MarketplaceSpec, SourceError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(SourceError("the marketplace source is empty".into()));
    }
    let explicit_ref = explicit_ref.map(str::trim).filter(|r| !r.is_empty());
    let (body, suffix_ref) = split_ref(text);
    let git_ref = explicit_ref
        .map(str::to_string)
        .or(suffix_ref.map(str::to_string));
    if let Some(r) = &git_ref
        && (r.starts_with('-') || r.contains(char::is_whitespace) || r.contains(".."))
    {
        return Err(SourceError(format!("\"{r}\" is not a usable git ref")));
    }
    if is_local(body) {
        if git_ref.is_some() {
            return Err(SourceError(
                "a ref is only for a git marketplace, not a local folder".into(),
            ));
        }
        let path = if body == "~" || body.starts_with("~/") {
            let Some(home) = home else {
                return Err(SourceError(format!(
                    "\"{body}\" names the home directory, which is not known here"
                )));
            };
            home.join(body.trim_start_matches('~').trim_start_matches('/'))
        } else {
            cwd.join(body)
        };
        return Ok(MarketplaceSpec {
            source: MarketplaceSource::Local {
                path: normalize(&path),
            },
            git_ref: None,
        });
    }
    let source = if is_url(body) || is_scp_like(body) {
        MarketplaceSource::Git {
            url: body.trim_end_matches('/').to_string(),
        }
    } else if let Some((owner, repo)) = github_shorthand(body) {
        MarketplaceSource::GitHub {
            owner: owner.to_string(),
            repo: repo.trim_end_matches(".git").to_string(),
        }
    } else {
        return Err(SourceError(format!(
            "\"{text}\" is not a marketplace source — expected owner/repo, a git URL, \
             or a local folder (/abs, ./rel, ~/…)"
        )));
    };
    Ok(MarketplaceSpec { source, git_ref })
}

/// `text` without its `#ref` (always) or `@ref` (only after `owner/repo`
/// shorthand), and the ref.
fn split_ref(text: &str) -> (&str, Option<&str>) {
    if let Some((body, r)) = text.rsplit_once('#')
        && !r.is_empty()
        && !body.is_empty()
    {
        return (body, Some(r));
    }
    if !is_local(text)
        && !is_url(text)
        && !is_scp_like(text)
        && let Some((body, r)) = text.rsplit_once('@')
        && !r.is_empty()
        && github_shorthand(body).is_some()
    {
        return (body, Some(r));
    }
    (text, None)
}

impl MarketplaceSource {
    /// The URL git clones, GitHub shorthand resolved against `github_base`
    /// (normally [`GITHUB_BASE`]). `None` for a local folder.
    #[must_use]
    pub fn clone_url(&self, github_base: &str) -> Option<String> {
        match self {
            Self::GitHub { owner, repo } => Some(github_url(github_base, owner, repo)),
            Self::Git { url } => Some(url.clone()),
            Self::Local { .. } => None,
        }
    }

    /// What `plugins.json` records: `owner/repo`, the URL, or the absolute
    /// folder — each of which parses back to this source.
    #[must_use]
    pub fn record(&self) -> String {
        match self {
            Self::GitHub { owner, repo } => format!("{owner}/{repo}"),
            Self::Git { url } => url.clone(),
            Self::Local { path } => path.to_string_lossy().into_owned(),
        }
    }

    /// The short form a list shows: `owner/repo`, a URL without its scheme
    /// or `.git`, a folder `~`-relative under `home`.
    #[must_use]
    pub fn display(&self, home: Option<&Path>) -> String {
        match self {
            Self::GitHub { owner, repo } => format!("{owner}/{repo}"),
            Self::Git { url } => {
                let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
                let rest = rest.trim_end_matches(".git");
                let rest = rest.strip_prefix("github.com/").unwrap_or(rest);
                rest.to_string()
            }
            Self::Local { path } => match home.and_then(|home| path.strip_prefix(home).ok()) {
                Some(rel) if rel.as_os_str().is_empty() => "~".to_string(),
                Some(rel) => format!("~/{}", rel.display()),
                None => path.display().to_string(),
            },
        }
    }

    /// Is this a folder used in place (no clone)?
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

/// `{base}{owner}/{repo}.git`, `base` given a trailing `/` when it lacks one.
#[must_use]
pub fn github_url(base: &str, owner: &str, repo: &str) -> String {
    let sep = if base.ends_with('/') { "" } else { "/" };
    format!("{base}{sep}{owner}/{repo}.git")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<MarketplaceSpec, SourceError> {
        parse_marketplace_source(
            text,
            None,
            Path::new("/work/dir"),
            Some(Path::new("/home/me")),
        )
    }

    fn git(url: &str, r: Option<&str>) -> MarketplaceSpec {
        MarketplaceSpec {
            source: MarketplaceSource::Git { url: url.into() },
            git_ref: r.map(str::to_string),
        }
    }

    #[test]
    fn owner_repo_is_github_with_an_optional_ref() {
        let spec = parse("anthropics/claude-plugins-official").unwrap();
        assert_eq!(
            spec.source,
            MarketplaceSource::GitHub {
                owner: "anthropics".into(),
                repo: "claude-plugins-official".into()
            }
        );
        assert_eq!(spec.git_ref, None);
        assert_eq!(
            parse("openai/plugins@main").unwrap().git_ref.as_deref(),
            Some("main")
        );
        assert_eq!(
            parse("openai/plugins#v1.2").unwrap().git_ref.as_deref(),
            Some("v1.2")
        );
        assert_eq!(
            spec.source.clone_url(GITHUB_BASE).as_deref(),
            Some("https://github.com/anthropics/claude-plugins-official.git")
        );
        assert_eq!(
            spec.source.clone_url("file:///srv/gh").as_deref(),
            Some("file:///srv/gh/anthropics/claude-plugins-official.git")
        );
    }

    #[test]
    fn urls_and_scp_paths_are_git_and_only_hash_names_their_ref() {
        assert_eq!(
            parse("https://github.com/o/r.git").unwrap(),
            git("https://github.com/o/r.git", None)
        );
        assert_eq!(
            parse("https://example.com/o/r/#dev").unwrap(),
            git("https://example.com/o/r", Some("dev"))
        );
        assert_eq!(
            parse("git@github.com:o/r.git").unwrap(),
            git("git@github.com:o/r.git", None)
        );
        assert_eq!(
            parse("ssh://git@host/x.git").unwrap(),
            git("ssh://git@host/x.git", None)
        );
        assert_eq!(
            parse("file:///srv/m.git").unwrap(),
            git("file:///srv/m.git", None)
        );
    }

    #[test]
    fn folders_resolve_against_the_working_directory_and_home() {
        let local = |text: &str| match parse(text).unwrap().source {
            MarketplaceSource::Local { path } => path,
            other => panic!("{other:?}"),
        };
        assert_eq!(local("/abs/m"), PathBuf::from("/abs/m"));
        assert_eq!(local("./m"), PathBuf::from("/work/dir/m"));
        assert_eq!(local("../m"), PathBuf::from("/work/m"));
        assert_eq!(local("~/dev/m"), PathBuf::from("/home/me/dev/m"));
        assert_eq!(local("."), PathBuf::from("/work/dir"));
    }

    #[test]
    fn an_explicit_ref_wins_and_a_folder_takes_none() {
        let spec = parse_marketplace_source("o/r@a", Some("b"), Path::new("/"), None).unwrap();
        assert_eq!(spec.git_ref.as_deref(), Some("b"));
        let err = parse_marketplace_source("./m", Some("b"), Path::new("/"), None).unwrap_err();
        assert!(
            err.to_string().contains("only for a git marketplace"),
            "{err}"
        );
    }

    #[test]
    fn typos_are_errors_before_anything_is_fetched() {
        for bad in ["", "  ", "just-a-name", "a/b/c", "o/r@-x", "x://y"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(
            parse_marketplace_source("~/m", None, Path::new("/"), None).is_err(),
            "no home"
        );
    }

    #[test]
    fn the_display_form_is_short() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(parse("o/r").unwrap().source.display(home), "o/r");
        assert_eq!(
            parse("https://github.com/o/r.git")
                .unwrap()
                .source
                .display(home),
            "o/r"
        );
        assert_eq!(
            parse("https://gitlab.com/o/r.git")
                .unwrap()
                .source
                .display(home),
            "gitlab.com/o/r"
        );
        assert_eq!(parse("~/dev/m").unwrap().source.display(home), "~/dev/m");
        assert_eq!(parse("/srv/m").unwrap().source.display(home), "/srv/m");
    }

    #[test]
    fn the_recorded_form_parses_back_to_the_same_source() {
        for text in ["o/r", "https://x.org/a.git", "/srv/m", "git@h:a/b"] {
            let spec = parse(text).unwrap();
            assert_eq!(
                parse(&spec.source.record()).unwrap().source,
                spec.source,
                "{text}"
            );
        }
    }
}

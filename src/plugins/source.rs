//! Where a marketplace lives and where a plugin's files come from.
//!
//! A marketplace is a git repository or a local folder
//! ([`MarketplaceSource`], parsed from what the user types); a plugin is a
//! folder inside the marketplace, a git repository (or one folder of one), or
//! a shape this build does not fetch ([`PluginSource`], read from the
//! marketplace entry's `source`). Both are plain data: nothing here touches
//! the network or the disk. See `docs/plugins.md`.
//!
//! The entry's `source` is read straight off the marketplace stream as a
//! `RawSource` — a string or a handful of optional strings, never a JSON
//! tree — so the marketplace reader keeps nothing it does not need
//! (`docs/memory.md` *The rule*).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};

use super::id::local_path;

/// Visitor methods for the JSON scalars a lenient reader does not want:
/// each becomes `$none`. With [`ignore_seq`] and [`ignore_map`] this is how
/// every reader here takes a value of the wrong type without failing the
/// entry, or the whole stream, that holds it.
macro_rules! ignore_scalars {
    ($none:expr) => {
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok($none)
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok($none)
        }
    };
}
pub(crate) use ignore_scalars;

/// A `visit_str` that takes any string as `$none` — for readers that want
/// only objects or arrays.
macro_rules! ignore_str {
    ($none:expr) => {
        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok($none)
        }
    };
}
pub(crate) use ignore_str;

/// A `visit_seq` that consumes an array element by element, keeping nothing.
macro_rules! ignore_seq {
    ($none:expr) => {
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok($none)
        }
    };
}
pub(crate) use ignore_seq;

/// A `visit_map` that consumes an object entry by entry, keeping nothing.
macro_rules! ignore_map {
    ($none:expr) => {
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            while map
                .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                .is_some()
            {}
            Ok($none)
        }
    };
}
pub(crate) use ignore_map;

/// Forges whose ssh remotes have a browsable https page at the same path.
const WEB_FORGES: [&str; 4] = ["github.com", "gitlab.com", "bitbucket.org", "codeberg.org"];

/// Where a marketplace lives: a git remote (with the branch or tag asked for)
/// or a folder read in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceSource {
    Git {
        url: String,
        reference: Option<String>,
    },
    Local {
        path: PathBuf,
    },
}

impl MarketplaceSource {
    /// What the user typed, as a source:
    ///
    /// - `owner/repo`, `owner/repo@ref`, `owner/repo#ref` → GitHub over https
    ///   (`https://github.com/owner/repo.git`);
    /// - `http(s)://…`, `ssh://…`, `git://…`, `file://…` and
    ///   `user@host:path`, each with an optional `#ref` → git;
    /// - `./x`, `../x`, `/x`, `~/x`, `.` → a local folder, made absolute
    ///   against `cwd` / `home` (whether it exists is the caller's check).
    ///
    /// An http(s) URL ending in `.json` is refused: Claude Code reads one as
    /// a bare marketplace file, but its plugins' relative sources then have
    /// nothing to resolve against.
    ///
    /// # Errors
    /// A sentence naming the input and the shapes that are accepted.
    pub fn parse(input: &str, cwd: &Path, home: Option<&Path>) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err(
                "no marketplace given — name one as `owner/repo`, a git URL or a folder".into(),
            );
        }
        if let Some(path) = local_path(input, cwd, home) {
            return path.map(|path| Self::Local { path });
        }
        let (location, reference) = split_reference(input, &['#']);
        if is_git_url(location) {
            let lower = location.to_ascii_lowercase();
            if (lower.starts_with("https://") || lower.starts_with("http://"))
                && lower.ends_with(".json")
            {
                return Err(format!(
                    "`{input}` is a JSON file — a marketplace is a repository or a folder; \
                     give the repository's URL instead"
                ));
            }
            return Ok(Self::Git {
                url: location.to_string(),
                reference,
            });
        }
        let (location, reference) = split_reference(input, &['#', '@']);
        if let Some(url) = github_shorthand(location) {
            return Ok(Self::Git { url, reference });
        }
        Err(format!(
            "`{input}` is not a marketplace source — give `owner/repo`, a git URL or a \
             folder such as `./plugins`"
        ))
    }

    /// What `plugins.json` stores and `removed_defaults` compares: `owner/repo`
    /// for a github.com https URL (with or without `.git`), else the URL, else
    /// the absolute path. The ref is not part of it — the state file keeps it
    /// beside the source — so `parse(canonical())` gives back the same
    /// location with no ref.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Git { url, .. } => github_slug(url).unwrap_or_else(|| url.clone()),
            Self::Local { path } => path.display().to_string(),
        }
    }

    /// For display: [`MarketplaceSource::canonical`], with a local folder
    /// under `home` shown as `~/…`.
    #[must_use]
    pub fn display(&self, home: Option<&Path>) -> String {
        if let (Self::Local { path }, Some(home)) = (self, home)
            && let Ok(rest) = path.strip_prefix(home)
        {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
        self.canonical()
    }

    /// A browsable https URL when there is one: any http(s) remote, and the
    /// ssh remotes of the well-known forges.
    #[must_use]
    pub fn web_url(&self) -> Option<String> {
        match self {
            Self::Git { url, .. } => forge_page(url),
            Self::Local { .. } => None,
        }
    }
}

impl fmt::Display for MarketplaceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Where one plugin's files come from, read from its marketplace entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A folder inside the marketplace root (already [`safe_relative`];
    /// `""` is the root itself).
    Local { path: String },
    /// A git repository, or one folder of it, at a ref or a pinned commit.
    Git {
        url: String,
        path: Option<String>,
        reference: Option<String>,
        sha: Option<String>,
    },
    /// A shape this build never fetches — npm, a command, an archive, an
    /// unknown kind, or a known kind with a field it cannot use. Listed, shown
    /// as not installable, never run.
    Unsupported { kind: String },
}

impl PluginSource {
    /// Every `source` shape a marketplace entry carries (`docs/plugins.md`):
    /// a `./` path, a bare name under `metadata.pluginRoot` (`plugin_root`),
    /// `{"source":"local"}`, `"url"`, `"git-subdir"` and `"github"` objects.
    /// Never fails: a shape it cannot read is [`PluginSource::Unsupported`].
    #[must_use]
    pub fn from_json(value: &serde_json::Value, plugin_root: Option<&str>) -> Self {
        RawSource::deserialize(value)
            .unwrap_or(RawSource::Other)
            .resolve(plugin_root)
    }

    /// Whether Install can be offered: a folder or a git repository.
    #[must_use]
    pub fn is_installable(&self) -> bool {
        !matches!(self, Self::Unsupported { .. })
    }

    /// One line for a detail page or `plugin info`: `plugins/code-review`,
    /// `github.com/o/r · plugins/x @ v1.2 (9fceb02)`, `npm (not supported)`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Local { path } if path.is_empty() => "the marketplace root".to_string(),
            Self::Local { path } => path.clone(),
            Self::Git {
                url,
                path,
                reference,
                sha,
            } => {
                let mut label = host_path(url);
                if let Some(path) = path {
                    label.push_str(" · ");
                    label.push_str(path);
                }
                if let Some(reference) = reference {
                    label.push_str(" @ ");
                    label.push_str(reference);
                }
                if let Some(sha) = sha {
                    let short: String = sha.chars().take(7).collect();
                    label.push_str(&format!(" ({short})"));
                }
                label
            }
            Self::Unsupported { kind } => format!("{kind} (not supported)"),
        }
    }

    /// The plugin's page on its forge, into the folder when it is one folder
    /// of the repository (GitHub's `tree/<rev>/<path>`).
    #[must_use]
    pub fn web_url(&self) -> Option<String> {
        let Self::Git {
            url,
            path,
            reference,
            sha,
        } = self
        else {
            return None;
        };
        let page = forge_page(url)?;
        let Some(path) = path.as_deref().filter(|p| !p.is_empty()) else {
            return Some(page);
        };
        if !page.starts_with("https://github.com/") {
            return Some(page);
        }
        let revision = sha.as_deref().or(reference.as_deref()).unwrap_or("HEAD");
        Some(format!("{page}/tree/{revision}/{path}"))
    }
}

/// A plugin-relative path made safe to join onto a root: leading `./`
/// stripped, empty and `.` segments and a trailing `/` dropped; `None` for an
/// absolute path, any `..` segment, a backslash or a NUL. `"."` and `"./"`
/// are `Some("")`, the root itself; `""` is `None`.
#[must_use]
pub fn safe_relative(path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') || path.contains(['\\', '\0']) {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// `input` split at the first of `marks` into the location and a non-empty
/// ref after it.
fn split_reference<'a>(input: &'a str, marks: &[char]) -> (&'a str, Option<String>) {
    match input.split_once(marks) {
        Some((location, reference)) => (
            location,
            Some(reference.trim())
                .filter(|r| !r.is_empty())
                .map(str::to_string),
        ),
        None => (input, None),
    }
}

/// Is `url` a remote git can be pointed at without reading it as an option
/// or a helper transport: one of the plain schemes, or scp-like
/// `user@host:path`.
fn is_git_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len())
    {
        return !url.contains(char::is_whitespace);
    }
    scp_parts(url).is_some()
}

/// `user@host:path` → `(host, path)`.
fn scp_parts(url: &str) -> Option<(&str, &str)> {
    if url.contains("://") || url.starts_with('-') || url.contains(char::is_whitespace) {
        return None;
    }
    let (left, path) = url.split_once(':')?;
    let (user, host) = left.split_once('@')?;
    (!user.is_empty() && !host.is_empty() && !host.contains('/') && !path.is_empty())
        .then_some((host, path))
}

/// `owner/repo` (an optional `.git` dropped) → its GitHub https clone URL.
fn github_shorthand(text: &str) -> Option<String> {
    let (owner, repo) = text.split_once('/')?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let segment = |s: &str| {
        !s.is_empty()
            && !matches!(s, "." | "..")
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (segment(owner) && segment(repo)).then(|| format!("https://github.com/{owner}/{repo}.git"))
}

/// `https://github.com/o/r[.git][/]` → `o/r`.
fn github_slug(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let slug = rest.to_string();
    github_shorthand(&slug).map(|_| slug)
}

/// A remote's browsable page: http(s) remotes as they are (minus `.git`),
/// ssh remotes of a [`WEB_FORGES`] host as https.
fn forge_page(url: &str) -> Option<String> {
    let trim = |path: &str| {
        let path = path.trim_end_matches('/');
        path.strip_suffix(".git").unwrap_or(path).to_string()
    };
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(trim(url));
    }
    let (host, path) = if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host.split_once(':').map_or(host, |(h, _)| h);
        (host, path)
    } else {
        scp_parts(url)?
    };
    WEB_FORGES
        .contains(&host)
        .then(|| format!("https://{host}/{}", trim(path.trim_start_matches('/'))))
}

/// A remote as `host/path` with no scheme, user or `.git`: `github.com/o/r`.
fn host_path(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = match scp_parts(url) {
        Some((host, path)) => format!("{host}/{path}"),
        None => match rest.split_once('/') {
            Some((authority, path)) => {
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                format!("{host}/{path}")
            }
            None => rest.to_string(),
        },
    };
    let rest = rest.trim_end_matches('/');
    rest.strip_suffix(".git").unwrap_or(rest).to_string()
}

/// A ref or commit that can be handed to git: no leading `-`, no whitespace.
fn plain_revision(text: &str) -> bool {
    !text.is_empty() && !text.starts_with('-') && !text.contains(char::is_whitespace)
}

/// A marketplace entry's `source` as it was written, before
/// [`RawSource::resolve`] turns it into a [`PluginSource`]. It is read off
/// the stream with no JSON tree: a string, a few optional strings, or
/// nothing usable. The marketplace reader keeps it until the header's
/// `metadata.pluginRoot` is known, which may come after `plugins`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum RawSource {
    Path(String),
    Object(SourceFields),
    #[default]
    Other,
}

/// The fields of an object `source` this build reads; any other key is
/// skipped, and a known key holding something other than a string is absent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SourceFields {
    kind: Option<String>,
    path: Option<String>,
    url: Option<String>,
    repo: Option<String>,
    reference: Option<String>,
    sha: Option<String>,
}

impl RawSource {
    /// Whether this names a folder by a bare name (no `./`), which lands
    /// under the marketplace's plugin root.
    pub(crate) fn is_bare(&self) -> bool {
        match self {
            Self::Path(path) => !is_root_relative(path),
            Self::Object(fields) => {
                fields.kind.as_deref() == Some("local")
                    && fields.path.as_deref().is_some_and(|p| !is_root_relative(p))
            }
            Self::Other => false,
        }
    }

    /// The [`PluginSource`] this shape means, bare names joined onto
    /// `plugin_root`.
    pub(crate) fn resolve(self, plugin_root: Option<&str>) -> PluginSource {
        match self {
            Self::Path(path) => local_source(&path, plugin_root).map_or_else(
                || PluginSource::Unsupported {
                    kind: "path".into(),
                },
                |path| PluginSource::Local { path },
            ),
            Self::Object(fields) => fields.resolve(plugin_root),
            Self::Other => unsupported("unknown"),
        }
    }
}

impl SourceFields {
    fn resolve(self, plugin_root: Option<&str>) -> PluginSource {
        let Some(kind) = self.kind else {
            return unsupported("unknown");
        };
        let refuse = || unsupported(&kind);
        match kind.as_str() {
            "local" => self
                .path
                .as_deref()
                .and_then(|path| local_source(path, plugin_root))
                .map_or_else(refuse, |path| PluginSource::Local { path }),
            "url" | "git-subdir" | "github" => {
                let url = if kind == "github" {
                    self.repo.as_deref().and_then(github_shorthand)
                } else {
                    self.url.as_deref().and_then(|url| {
                        github_shorthand(url).or_else(|| is_git_url(url).then(|| url.to_string()))
                    })
                };
                let Some(url) = url else { return refuse() };
                let path = match self.path.as_deref() {
                    None if kind == "git-subdir" => return refuse(),
                    None => None,
                    Some(path) => match safe_relative(path) {
                        None => return refuse(),
                        Some(path) => Some(path).filter(|p| !p.is_empty()),
                    },
                };
                if !self.reference.as_deref().is_none_or(plain_revision) {
                    return refuse();
                }
                let sha = self.sha.map(|sha| sha.to_ascii_lowercase());
                if !sha
                    .as_deref()
                    .is_none_or(|sha| sha.len() >= 4 && sha.chars().all(|c| c.is_ascii_hexdigit()))
                {
                    return refuse();
                }
                PluginSource::Git {
                    url,
                    path,
                    reference: self.reference,
                    sha,
                }
            }
            _ => refuse(),
        }
    }
}

fn unsupported(kind: &str) -> PluginSource {
    PluginSource::Unsupported {
        kind: kind.to_string(),
    }
}

/// `./x` and `.` are relative to the marketplace root; anything else is a
/// bare name for the plugin root.
fn is_root_relative(path: &str) -> bool {
    path == "." || path.starts_with("./")
}

/// A local plugin folder, relative to the marketplace root.
fn local_source(path: &str, plugin_root: Option<&str>) -> Option<String> {
    let relative = safe_relative(path)?;
    if is_root_relative(path) {
        return Some(relative);
    }
    match plugin_root.and_then(safe_relative) {
        Some(root) if !root.is_empty() => Some(format!("{root}/{relative}")),
        _ => Some(relative),
    }
}

impl<'de> Deserialize<'de> for RawSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;

        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = RawSource;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a plugin source")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<RawSource, E> {
                Ok(RawSource::Path(text.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawSource, A::Error> {
                let mut fields = SourceFields::default();
                while let Some(key) = map.next_key::<KeyName>()? {
                    let slot = match key.0.as_deref() {
                        Some("source") => &mut fields.kind,
                        Some("path") => &mut fields.path,
                        Some("url") => &mut fields.url,
                        Some("repo") => &mut fields.repo,
                        Some("ref") => &mut fields.reference,
                        Some("sha") => &mut fields.sha,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                            continue;
                        }
                    };
                    *slot = map.next_value::<MaybeString>()?.0;
                }
                Ok(RawSource::Object(fields))
            }

            ignore_scalars!(RawSource::Other);
            ignore_seq!(RawSource::Other);
        }

        deserializer.deserialize_any(SourceVisitor)
    }
}

/// A string, or `None` for a value of any other type — a known field written
/// with the wrong type is dropped rather than failing what holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MaybeString(pub(crate) Option<String>);

impl<'de> Deserialize<'de> for MaybeString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringVisitor;

        impl<'de> Visitor<'de> for StringVisitor {
            type Value = MaybeString;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<MaybeString, E> {
                Ok(MaybeString(Some(text.to_string())))
            }

            fn visit_string<E: de::Error>(self, text: String) -> Result<MaybeString, E> {
                Ok(MaybeString(Some(text)))
            }

            ignore_scalars!(MaybeString(None));
            ignore_seq!(MaybeString(None));
            ignore_map!(MaybeString(None));
        }

        deserializer.deserialize_any(StringVisitor)
    }
}

/// An object key, kept only when it is short enough to be one this build
/// reads — so a pathological key is never copied whole.
pub(crate) struct KeyName(pub(crate) Option<String>);

/// Longer than any key the readers match on.
const KEY_NAME_MAX: usize = 32;

impl<'de> Deserialize<'de> for KeyName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl Visitor<'_> for KeyVisitor {
            type Value = KeyName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object key")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<KeyName, E> {
                Ok(KeyName(
                    (text.len() <= KEY_NAME_MAX).then(|| text.to_string()),
                ))
            }
        }

        deserializer.deserialize_str(KeyVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(input: &str) -> Result<MarketplaceSource, String> {
        MarketplaceSource::parse(input, Path::new("/work/repo"), Some(Path::new("/home/me")))
    }

    fn git(url: &str, reference: Option<&str>) -> MarketplaceSource {
        MarketplaceSource::Git {
            url: url.into(),
            reference: reference.map(Into::into),
        }
    }

    // ===== marketplace sources =====

    #[test]
    fn owner_slash_repo_is_github_with_an_optional_ref() {
        let url = "https://github.com/anthropics/claude-plugins-official.git";
        assert_eq!(
            parse("anthropics/claude-plugins-official"),
            Ok(git(url, None))
        );
        assert_eq!(
            parse("anthropics/claude-plugins-official@v2"),
            Ok(git(url, Some("v2")))
        );
        assert_eq!(
            parse("anthropics/claude-plugins-official#main"),
            Ok(git(url, Some("main")))
        );
        assert_eq!(
            parse("openai/plugins.git"),
            Ok(git("https://github.com/openai/plugins.git", None))
        );
    }

    #[test]
    fn a_git_url_is_git_with_an_optional_fragment_ref() {
        assert_eq!(
            parse("https://gitlab.com/team/plugins.git#release"),
            Ok(git("https://gitlab.com/team/plugins.git", Some("release")))
        );
        assert_eq!(
            parse("ssh://git@host/x.git"),
            Ok(git("ssh://git@host/x.git", None))
        );
        assert_eq!(parse("git://host/x"), Ok(git("git://host/x", None)));
        assert_eq!(parse("file:///srv/x"), Ok(git("file:///srv/x", None)));
        assert_eq!(
            parse("git@github.com:o/r.git#dev"),
            Ok(git("git@github.com:o/r.git", Some("dev")))
        );
    }

    #[test]
    fn a_path_is_a_local_folder_made_absolute() {
        let local = |p: &str| MarketplaceSource::Local {
            path: PathBuf::from(p),
        };
        assert_eq!(parse("./team"), Ok(local("/work/repo/team")));
        assert_eq!(parse("../team"), Ok(local("/work/team")));
        assert_eq!(parse("/srv/team/"), Ok(local("/srv/team")));
        assert_eq!(parse("~/team"), Ok(local("/home/me/team")));
        assert_eq!(parse("."), Ok(local("/work/repo")));
    }

    #[test]
    fn a_json_url_is_refused_with_a_hint() {
        let err = parse("https://example.com/marketplace.json").unwrap_err();
        assert!(err.contains("repository or a folder"), "{err}");
    }

    #[test]
    fn something_that_is_no_source_is_refused() {
        assert!(parse("").is_err());
        assert!(parse("just-a-name").is_err());
        assert!(parse("a/b/c").is_err());
        assert!(parse("ftp://x/y").is_err());
    }

    #[test]
    fn canonical_is_owner_slash_repo_for_github_and_round_trips() {
        for (input, canonical) in [
            (
                "anthropics/claude-plugins-official",
                "anthropics/claude-plugins-official",
            ),
            ("https://github.com/openai/plugins", "openai/plugins"),
            ("https://github.com/openai/plugins.git", "openai/plugins"),
            ("https://gitlab.com/t/p.git", "https://gitlab.com/t/p.git"),
            ("git@github.com:o/r.git", "git@github.com:o/r.git"),
            ("./team", "/work/repo/team"),
        ] {
            let source = parse(input).unwrap();
            assert_eq!(source.canonical(), canonical, "{input}");
            assert_eq!(
                parse(&source.canonical()).unwrap().canonical(),
                canonical,
                "{input}"
            );
        }
        // The ref is not part of the canonical form — it lives beside it.
        assert_eq!(
            parse("openai/plugins#dev").unwrap().canonical(),
            "openai/plugins"
        );
    }

    #[test]
    fn display_shows_home_as_a_tilde() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(parse("~/team").unwrap().display(home), "~/team");
        assert_eq!(parse("/srv/team").unwrap().display(home), "/srv/team");
        assert_eq!(
            parse("openai/plugins").unwrap().display(home),
            "openai/plugins"
        );
    }

    #[test]
    fn a_web_url_exists_for_forge_remotes() {
        assert_eq!(
            parse("openai/plugins").unwrap().web_url().as_deref(),
            Some("https://github.com/openai/plugins")
        );
        assert_eq!(
            parse("git@gitlab.com:t/p.git")
                .unwrap()
                .web_url()
                .as_deref(),
            Some("https://gitlab.com/t/p")
        );
        assert_eq!(
            parse("https://git.example.com/t/p.git")
                .unwrap()
                .web_url()
                .as_deref(),
            Some("https://git.example.com/t/p")
        );
        assert_eq!(parse("./team").unwrap().web_url(), None);
        assert_eq!(parse("file:///srv/x").unwrap().web_url(), None);
    }

    // ===== safe_relative =====

    #[test]
    fn safe_relative_strips_the_dot_slash_and_refuses_an_escape() {
        assert_eq!(safe_relative("./plugins/x").as_deref(), Some("plugins/x"));
        assert_eq!(safe_relative("plugins/x/").as_deref(), Some("plugins/x"));
        assert_eq!(safe_relative(".").as_deref(), Some(""));
        assert_eq!(safe_relative("./").as_deref(), Some(""));
        for bad in ["/etc", "../x", "a/../../b", "a\\b", "a\0b", "", "a/.."] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }

    // ===== plugin sources: every shape in the marketplaces =====

    fn from(value: serde_json::Value) -> PluginSource {
        PluginSource::from_json(&value, None)
    }

    #[test]
    fn a_dot_slash_string_is_a_local_folder() {
        assert_eq!(
            from(json!("./plugins/agent-sdk-dev")),
            PluginSource::Local {
                path: "plugins/agent-sdk-dev".into()
            }
        );
    }

    #[test]
    fn a_bare_name_lands_under_the_plugin_root() {
        assert_eq!(
            PluginSource::from_json(&json!("formatter"), Some("./plugins")),
            PluginSource::Local {
                path: "plugins/formatter".into()
            }
        );
        // `./` strings are marketplace-root relative whatever the plugin root.
        assert_eq!(
            PluginSource::from_json(&json!("./x"), Some("./plugins")),
            PluginSource::Local { path: "x".into() }
        );
    }

    #[test]
    fn a_local_object_is_a_local_folder() {
        assert_eq!(
            from(json!({"source": "local", "path": "./plugins/linear"})),
            PluginSource::Local {
                path: "plugins/linear".into()
            }
        );
    }

    #[test]
    fn an_escaping_path_is_not_installable() {
        for value in [
            json!("../x"),
            json!("/etc"),
            json!({"source": "local", "path": "../x"}),
        ] {
            assert!(!from(value.clone()).is_installable(), "{value}");
        }
    }

    #[test]
    fn a_url_source_is_git_with_its_optional_path_ref_and_sha() {
        assert_eq!(
            from(json!({"source": "url",
                "url": "https://github.com/SalesforceAIResearch/agentforce-adlc.git",
                "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"})),
            PluginSource::Git {
                url: "https://github.com/SalesforceAIResearch/agentforce-adlc.git".into(),
                path: None,
                reference: None,
                sha: Some("09bf1539d41f9ff355ba3eb5d05d4a75813423bb".into()),
            }
        );
        assert_eq!(
            from(
                json!({"source": "url", "url": "https://github.com/BrainBlend-AI/atomic-agents.git",
                "path": "claude-plugin/atomic-agents", "ref": "main"})
            ),
            PluginSource::Git {
                url: "https://github.com/BrainBlend-AI/atomic-agents.git".into(),
                path: Some("claude-plugin/atomic-agents".into()),
                reference: Some("main".into()),
                sha: None,
            }
        );
    }

    #[test]
    fn a_git_subdir_source_is_git_and_takes_owner_slash_repo() {
        assert_eq!(
            from(
                json!({"source": "git-subdir", "url": "https://github.com/42Crunch-AI/claude-plugins.git",
                "path": "plugins/api-security-testing", "ref": "v1.5.5",
                "sha": "faf5305385de8afed9468904e8639be737aff39e"})
            ),
            PluginSource::Git {
                url: "https://github.com/42Crunch-AI/claude-plugins.git".into(),
                path: Some("plugins/api-security-testing".into()),
                reference: Some("v1.5.5".into()),
                sha: Some("faf5305385de8afed9468904e8639be737aff39e".into()),
            }
        );
        assert_eq!(
            from(json!({"source": "git-subdir", "url": "o/r", "path": "./tools/x"})),
            PluginSource::Git {
                url: "https://github.com/o/r.git".into(),
                path: Some("tools/x".into()),
                reference: None,
                sha: None,
            }
        );
    }

    #[test]
    fn a_github_source_is_git_on_github() {
        assert_eq!(
            from(json!({"source": "github", "repo": "your-org/formatter", "ref": "v2.0.0"})),
            PluginSource::Git {
                url: "https://github.com/your-org/formatter.git".into(),
                path: None,
                reference: Some("v2.0.0".into()),
                sha: None,
            }
        );
    }

    #[test]
    fn npm_command_archive_and_anything_else_are_unsupported() {
        for (value, kind) in [
            (json!({"source": "npm", "package": "@o/p"}), "npm"),
            (
                json!({"source": "command", "command": "xcrun agent plugin path", "mode": "link"}),
                "command",
            ),
            (
                json!({"source": "archive", "url": "https://x/p.zip"}),
                "archive",
            ),
            (json!({"source": "pip"}), "pip"),
            (json!({"path": "./x"}), "unknown"),
            (json!(42), "unknown"),
            (json!({"source": "url"}), "url"),
            (
                json!({"source": "git-subdir", "url": "https://x/y.git"}),
                "git-subdir",
            ),
            (
                json!({"source": "url", "url": "https://x/y.git", "path": "../up"}),
                "url",
            ),
        ] {
            let source = from(value.clone());
            assert_eq!(
                source,
                PluginSource::Unsupported { kind: kind.into() },
                "{value}"
            );
            assert!(!source.is_installable());
        }
    }

    #[test]
    fn local_and_git_sources_are_installable() {
        assert!(from(json!("./x")).is_installable());
        assert!(from(json!({"source": "url", "url": "https://h/x.git"})).is_installable());
    }

    #[test]
    fn a_label_is_one_line_for_a_detail_page() {
        assert_eq!(
            from(json!("./plugins/code-review")).label(),
            "plugins/code-review"
        );
        assert_eq!(
            from(json!({"source": "git-subdir", "url": "https://github.com/o/r.git",
                "path": "plugins/x", "ref": "v1.2", "sha": "9fceb02d0ae598e95dc970b74767f19372d61af8"}))
            .label(),
            "github.com/o/r · plugins/x @ v1.2 (9fceb02)"
        );
        assert_eq!(
            from(json!({"source": "url", "url": "https://gitlab.com/t/p.git"})).label(),
            "gitlab.com/t/p"
        );
        assert_eq!(
            from(json!({"source": "npm"})).label(),
            "npm (not supported)"
        );
    }

    #[test]
    fn a_plugin_web_url_points_into_the_repository() {
        assert_eq!(
            from(
                json!({"source": "git-subdir", "url": "https://github.com/o/r.git",
                "path": "plugins/x", "ref": "v1.2"})
            )
            .web_url()
            .as_deref(),
            Some("https://github.com/o/r/tree/v1.2/plugins/x")
        );
        assert_eq!(
            from(json!({"source": "url", "url": "https://github.com/o/r.git"}))
                .web_url()
                .as_deref(),
            Some("https://github.com/o/r")
        );
        assert_eq!(from(json!("./x")).web_url(), None);
    }
}

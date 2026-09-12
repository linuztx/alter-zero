//! Update check — **once a day, is there a newer release?** (`docs/update.md`)
//!
//! Pure half: the `update.json` format ([`UpdateFile`]), the version grammar
//! and ordering ([`Version`], [`is_newer`]), the tag read off GitHub's
//! `/releases/latest` redirect ([`version_from_release_url`]), the once-a-day
//! decisions ([`should_check`], [`notice_due`]), the URLs built off the
//! repository ([`latest_url`], [`release_page_url`], [`installer_url`]), the
//! environment predicate ([`enabled_by_env`]), the card's text ([`notice`])
//! and the one guard `alter-zero update` needs ([`is_cargo_build_dir`]). No
//! clock, no filesystem, no network: the boundary (`tui::update`) reads the
//! UTC date, does the read-modify-write over the file and spawns the request
//! — the one impure function here, [`fetch_latest`], is what that worker
//! thread calls.

use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::APP_NAME;

/// The file's name under the config home: `{config_home}/update.json` — its
/// own file, per user, like `telemetry.json` (`docs/per-directory-state.md`).
pub const UPDATE_FILE_NAME: &str = "update.json";

/// The app's own on/off switch for a run (`0`/`false`/`no`/`off` = off, the
/// grammar every `ALTER_ZERO_*` flag uses). Seeds the `/settings` row; never
/// written back.
pub const UPDATE_ENV: &str = "ALTER_ZERO_UPDATE_CHECK";

/// Overrides [`DEFAULT_REPO_URL`] for a run — a fork, or the smoke suite's
/// stand-in server (`scripts/release/release_server.py`).
pub const REPO_URL_ENV: &str = "ALTER_ZERO_UPDATE_URL";

/// The repository whose releases are checked: Cargo.toml's own `repository`,
/// so a fork that edits the manifest checks its own releases.
pub const DEFAULT_REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// The one-line installer's name at the repository root, and the branch it
/// is served from ([`installer_url`]).
pub const INSTALLER_FILE: &str = "install.sh";
pub const INSTALLER_BRANCH: &str = "main";

/// What `update.json` holds: the switch, the last day a check ran, the
/// newest version that check saw, and the last day the card was shown.
///
/// Every field is written on every save — a status file the user may open —
/// and every field is optional on the way in ([`Self::parse`] is lenient), so
/// a partial, hand-edited or corrupt file reads as the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateFile {
    /// The user's standing choice — `true` until turned off (`/settings` →
    /// Update check). The environment can override it for a run
    /// ([`enabled_by_env`]) without moving it.
    pub enabled: bool,
    /// The UTC date (`YYYY-MM-DD`) of the last check that ran, delivered or
    /// not — one request per day per install is the promise, so a failed
    /// check is not retried until tomorrow.
    pub last_check_day: Option<String>,
    /// The newest release the last successful check saw, as a bare version
    /// (`0.2.0`, no `v`). Kept across failed checks: what was known stays
    /// known.
    pub latest: Option<String>,
    /// The UTC date the card was last committed under the banner, so a
    /// pending update is mentioned once a day — not once per launch, and not
    /// once ever.
    pub notice_day: Option<String>,
}

impl Default for UpdateFile {
    fn default() -> Self {
        Self {
            enabled: true,
            last_check_day: None,
            latest: None,
            notice_day: None,
        }
    }
}

impl UpdateFile {
    /// Parse an `update.json` body, best-effort: malformed, empty or
    /// wrongly-typed JSON yields the defaults, so a bad file costs at most
    /// one extra check.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back — every field, always.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Note that a check ran on `day`, and what it found when it succeeded.
    /// A failed check (`None`) still stamps the day — the once-a-day bound is
    /// on attempts — and leaves the last known version alone.
    pub fn record_check(&mut self, day: &str, latest: Option<&str>) {
        self.last_check_day = Some(day.to_string());
        if let Some(latest) = latest {
            self.latest = Some(latest.to_string());
        }
    }

    /// Note that the card was committed on `day`.
    pub fn record_notice(&mut self, day: &str) {
        self.notice_day = Some(day.to_string());
    }
}

/// A release version: `major.minor.patch` with an optional pre-release
/// (`0.2.0-rc.1`). Orders as semver 2.0 does — a pre-release sorts *before*
/// its release, identifiers compare numerically when both are numbers and
/// as text otherwise, a numeric identifier before an alphanumeric one, and
/// a longer identifier list after a shorter equal prefix. Build metadata is
/// dropped at the parse; it never orders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => compare_pre_release(a, b),
            })
    }
}

/// Semver's pre-release ordering over two dot-separated identifier lists.
fn compare_pre_release(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// A numeric identifier: digits only, no leading zero unless it is `0`.
fn numeric_part(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if part.len() > 1 && part.starts_with('0') {
        return None;
    }
    part.parse().ok()
}

/// Parse `X.Y.Z`, `vX.Y.Z`, with an optional `-pre` and an ignored `+build`
/// — the shapes a release tag and Cargo.toml's `version` come in. Anything
/// else is `None`, and `None` is never newer than anything ([`is_newer`]).
#[must_use]
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let text = text.split_once('+').map_or(text, |(core, _build)| core);
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    if let Some(pre) = pre
        && (pre.is_empty()
            || !pre.split('.').all(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
    {
        return None;
    }
    let mut parts = core.split('.');
    let major = numeric_part(parts.next()?)?;
    let minor = numeric_part(parts.next()?)?;
    let patch = numeric_part(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
        pre: pre.map(str::to_string),
    })
}

/// Whether `latest` is a release newer than `current` — both parsed as
/// versions, so `0.10.0` beats `0.9.0` and a build ahead of the last release
/// (a dev checkout) is never told to update. Unparseable input on either
/// side is `false`: a check that cannot be read must not nag.
#[must_use]
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// The version a release page's URL names — GitHub answers
/// `/releases/latest` with a redirect to `/releases/tag/vX.Y.Z`, and this
/// reads the tag off wherever that redirect ended, ignoring a query string
/// or fragment. `None` for a URL that is not a versioned release page.
#[must_use]
pub fn version_from_release_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let tag = path.rsplit_once("/releases/tag/")?.1.trim_end_matches('/');
    parse_version(tag)?;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Whether a check is due: none has run on `today` (the boundary's UTC
/// date, `YYYY-MM-DD`). Attempts, not successes — see [`UpdateFile::record_check`].
#[must_use]
pub fn should_check(file: &UpdateFile, today: &str) -> bool {
    file.last_check_day.as_deref() != Some(today)
}

/// The version the card should announce, if any: the newest known release
/// is newer than `current` and the card has not been shown `today`.
#[must_use]
pub fn notice_due(file: &UpdateFile, current: &str, today: &str) -> Option<String> {
    let latest = file.latest.as_deref()?;
    if file.notice_day.as_deref() == Some(today) || !is_newer(current, latest) {
        return None;
    }
    Some(latest.to_string())
}

fn trimmed(repo: &str) -> &str {
    repo.trim_end_matches('/')
}

/// The URL whose redirect names the newest release: `{repo}/releases/latest`.
#[must_use]
pub fn latest_url(repo: &str) -> String {
    format!("{}/releases/latest", trimmed(repo))
}

/// A release's own page: `{repo}/releases/tag/vX.Y.Z`.
#[must_use]
pub fn release_page_url(repo: &str, version: &str) -> String {
    let version = version.strip_prefix('v').unwrap_or(version);
    format!("{}/releases/tag/v{version}", trimmed(repo))
}

/// Where the one-line installer is fetched from: GitHub serves a
/// repository's files from `raw.githubusercontent.com`, so a `github.com`
/// repository maps there (on [`INSTALLER_BRANCH`]); any other base — a
/// stand-in server — serves it beside its releases at `{base}/install.sh`.
#[must_use]
pub fn installer_url(repo: &str) -> String {
    let repo = trimmed(repo);
    match repo.strip_prefix("https://github.com/") {
        Some(slug) => {
            format!("https://raw.githubusercontent.com/{slug}/{INSTALLER_BRANCH}/{INSTALLER_FILE}")
        }
        None => format!("{repo}/{INSTALLER_FILE}"),
    }
}

/// What the environment says about the check for this run, given the raw
/// value of [`UPDATE_ENV`] (`None` when unset): `0`/`false`/`no`/`off` is
/// `Some(false)`, any other set value `Some(true)`, unset `None` — defer to
/// the file's `enabled`. Seeds the `/settings` row; never written back.
#[must_use]
pub fn enabled_by_env(value: Option<&str>) -> Option<bool> {
    value.map(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// The card's text (`ui::update_notice_lines` frames and wraps it): both
/// versions, the release page, the command that updates, and the opt-out.
/// Inline markdown marks the labels and commands; the blank line separates
/// the news from the opt-out, the telemetry card's shape.
#[must_use]
pub fn notice(current: &str, latest: &str, repo: &str) -> String {
    let current = current.strip_prefix('v').unwrap_or(current);
    let latest = latest.strip_prefix('v').unwrap_or(latest);
    let page = release_page_url(repo, latest);
    let bin = env!("CARGO_PKG_NAME");
    format!(
        "{APP_NAME} **v{latest}** is out — you are running v{current}.\n\
         **What's new** {page}\n\
         **Update** run `{bin} update`, then restart.\n\n\
         **Turn off** `/settings → Update check` or `{UPDATE_ENV}=0`"
    )
}

/// Whether an executable sits in a `cargo` build directory —
/// `target/{debug,release}/` or `target/{triple}/{debug,release}/` — which
/// is a checkout, not an install: `alter-zero update` refuses to overwrite
/// one with a release binary and points at `git pull && cargo build`.
#[must_use]
pub fn is_cargo_build_dir(exe: &Path) -> bool {
    let is =
        |dir: Option<&Path>, name: &str| dir.and_then(Path::file_name).is_some_and(|n| n == name);
    let profile = exe.parent();
    if !is(profile, "debug") && !is(profile, "release") {
        return false;
    }
    let above = profile.and_then(Path::parent);
    is(above, "target") || is(above.and_then(Path::parent), "target")
}

/// Whether a fetched body is the one-line installer rather than whatever a
/// captive portal or an error page served with a `200`: the shebang, the
/// closing `main "$@"` call, and the variable `alter-zero update` hands it.
#[must_use]
pub fn looks_like_installer(text: &str) -> bool {
    text.starts_with("#!/bin/sh")
        && text.contains("\nmain \"$@\"")
        && text.contains("ALTER_ZERO_INSTALL_DIR")
}

/// Fetch the one-line installer's text from [`installer_url`] — the
/// boundary function `alter-zero update` calls before running it from a
/// file. A body that does not [`looks_like_installer`] is refused: `sh`
/// would run an HTML page as a script and exit 0.
///
/// # Errors
/// The transport error, a non-`2xx` answer, or a body that is not the
/// installer, as text.
pub fn fetch_installer(repo: &str, current_version: &str) -> Result<String, String> {
    let url = installer_url(repo);
    let client =
        crate::llm::http_client(crate::llm::openai::NET_OP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .get(&url)
        .header("user-agent", crate::telemetry::user_agent(current_version))
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let text = response.text().map_err(|e| e.to_string())?;
    if !looks_like_installer(&text) {
        return Err(format!("{url} did not serve the installer"));
    }
    Ok(text)
}

/// Ask the repository for its newest release — the boundary function the
/// worker thread (and `alter-zero update`) calls. One `HEAD` request to
/// [`latest_url`], following GitHub's redirect to the release's page and
/// reading the tag off where it landed: no API (so no rate limit), no body,
/// no identifier beyond the `alter-zero/{version}` user agent every request
/// carries. Rides the shared cached client (`llm::http_client`), so the
/// check adds no second connection pool to a process that idles all day.
///
/// # Errors
/// The transport error, a non-`2xx` answer (a repository with no release
/// yet answers 404), or a final URL that names no version, as text.
pub fn fetch_latest(repo: &str, current_version: &str) -> Result<String, String> {
    let url = latest_url(repo);
    let client =
        crate::llm::http_client(crate::llm::openai::NET_OP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .head(&url)
        .header("user-agent", crate::telemetry::user_agent(current_version))
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let landed = response.url().as_str();
    version_from_release_url(landed).ok_or_else(|| format!("no release tag in {landed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn versions_parse_with_or_without_the_v_and_ignore_build_metadata() {
        let v = parse_version("0.1.0").expect("plain");
        assert_eq!((v.major, v.minor, v.patch), (0, 1, 0));
        assert!(v.pre.is_none());
        assert_eq!(
            parse_version("v0.1.0"),
            parse_version("0.1.0"),
            "the tag's v is noise"
        );
        assert_eq!(
            parse_version("1.2.3+build.7"),
            parse_version("1.2.3"),
            "build metadata never orders"
        );
        let pre = parse_version("0.2.0-rc.1").expect("pre-release");
        assert_eq!(pre.pre.as_deref(), Some("rc.1"));
        for bad in [
            "", "0.1", "0.1.0.0", "a.b.c", "1.2.3-", "01.0.0", "v", "0.1.0 ",
        ] {
            assert!(parse_version(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn a_pre_release_is_older_than_its_release_and_orders_by_identifier() {
        let v = |s: &str| parse_version(s).unwrap();
        assert!(v("0.2.0-rc.1") < v("0.2.0"));
        assert!(v("0.2.0-alpha") < v("0.2.0-beta"));
        assert!(
            v("0.2.0-rc.9") < v("0.2.0-rc.10"),
            "numeric identifiers compare as numbers"
        );
        assert!(
            v("0.2.0-rc.1") < v("0.2.0-rc.1.1"),
            "a longer identifier list is later"
        );
        assert!(
            v("0.2.0-1") < v("0.2.0-a"),
            "numeric identifiers sort before alphanumeric"
        );
        assert!(
            v("0.1.9") < v("0.2.0-rc.1"),
            "a pre-release of a later version is still later"
        );
    }

    #[test]
    fn is_newer_compares_semver_not_strings() {
        assert!(
            is_newer("0.9.0", "0.10.0"),
            "0.10 > 0.9 as numbers, not as text"
        );
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(
            !is_newer("0.2.0", "0.1.9"),
            "a dev build ahead of the last release"
        );
        assert!(!is_newer("0.2.0-rc.1", "0.1.9"));
        assert!(
            is_newer("0.2.0-rc.1", "0.2.0"),
            "the release supersedes its candidate"
        );
        assert!(!is_newer("0.1.0", "latest"), "garbage is never newer");
        assert!(!is_newer("garbage", "0.2.0"), "…and never a baseline");
    }

    #[test]
    fn the_release_tag_is_read_off_the_redirects_final_url() {
        assert_eq!(
            version_from_release_url("https://github.com/linuztx/alter-zero/releases/tag/v0.2.0"),
            Some("0.2.0".to_string())
        );
        assert_eq!(
            version_from_release_url("http://127.0.0.1:8000/releases/tag/v9.9.9?x=1"),
            Some("9.9.9".to_string()),
            "a query string is not part of the tag"
        );
        assert_eq!(
            version_from_release_url(
                "https://github.com/linuztx/alter-zero/releases/tag/v0.2.0-rc.1"
            ),
            Some("0.2.0-rc.1".to_string())
        );
        for url in [
            "https://github.com/linuztx/alter-zero/releases",
            "https://github.com/linuztx/alter-zero/releases/latest",
            "https://github.com/linuztx/alter-zero/releases/tag/nightly",
            "https://github.com/linuztx/alter-zero/releases/tag/",
            "",
        ] {
            assert_eq!(version_from_release_url(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_check_is_due_once_per_utc_day() {
        let mut file = UpdateFile::default();
        assert!(should_check(&file, "2026-09-12"), "never checked");
        file.record_check("2026-09-12", Some("0.2.0"));
        assert!(!should_check(&file, "2026-09-12"), "checked today");
        assert!(should_check(&file, "2026-09-13"), "a new day");
        assert_eq!(file.latest.as_deref(), Some("0.2.0"));
        file.record_check("2026-09-13", None);
        assert_eq!(file.last_check_day.as_deref(), Some("2026-09-13"));
        assert_eq!(
            file.latest.as_deref(),
            Some("0.2.0"),
            "a failed check keeps what it knew"
        );
    }

    #[test]
    fn the_notice_is_due_when_a_newer_release_is_known_and_not_shown_today() {
        let mut file = UpdateFile::default();
        assert_eq!(
            notice_due(&file, "0.1.0", "2026-09-12"),
            None,
            "nothing known yet"
        );
        file.record_check("2026-09-12", Some("0.1.0"));
        assert_eq!(notice_due(&file, "0.1.0", "2026-09-12"), None, "up to date");
        file.record_check("2026-09-12", Some("0.2.0"));
        assert_eq!(
            notice_due(&file, "0.1.0", "2026-09-12"),
            Some("0.2.0".to_string()),
            "newer and never shown"
        );
        file.record_notice("2026-09-12");
        assert_eq!(
            notice_due(&file, "0.1.0", "2026-09-12"),
            None,
            "shown today already"
        );
        assert_eq!(
            notice_due(&file, "0.1.0", "2026-09-13"),
            Some("0.2.0".to_string()),
            "once a day, not once ever — the user still has not updated"
        );
        assert_eq!(
            notice_due(&file, "0.2.0", "2026-09-13"),
            None,
            "updated: quiet again"
        );
        assert_eq!(
            notice_due(&file, "0.3.0-dev", "2026-09-13"),
            None,
            "a build ahead of the release is never nagged"
        );
    }

    #[test]
    fn update_json_round_trips_and_reads_a_bad_file_as_defaults() {
        let mut file = UpdateFile::default();
        assert!(file.enabled, "opt-out: on by default");
        file.enabled = false;
        file.record_check("2026-09-12", Some("0.2.0"));
        file.record_notice("2026-09-12");
        let back = UpdateFile::parse(&file.to_json());
        assert_eq!(back, file);
        let json = file.to_json();
        for key in ["enabled", "last_check_day", "latest", "notice_day"] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "every field written: {json}"
            );
        }
        assert_eq!(UpdateFile::parse(""), UpdateFile::default());
        assert_eq!(UpdateFile::parse("{not json"), UpdateFile::default());
        assert_eq!(
            UpdateFile::parse(r#"{"enabled": "yes"}"#),
            UpdateFile::default(),
            "wrong type"
        );
        let partial = UpdateFile::parse(r#"{"latest": "0.2.0"}"#);
        assert!(partial.enabled && partial.latest.as_deref() == Some("0.2.0"));
    }

    #[test]
    fn urls_are_built_off_the_repository() {
        let repo = "https://github.com/linuztx/alter-zero";
        assert_eq!(
            latest_url(repo),
            "https://github.com/linuztx/alter-zero/releases/latest"
        );
        assert_eq!(
            release_page_url(repo, "0.2.0"),
            "https://github.com/linuztx/alter-zero/releases/tag/v0.2.0"
        );
        assert_eq!(
            installer_url(repo),
            "https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh",
            "GitHub serves raw files from another host"
        );
        assert_eq!(
            installer_url("https://github.com/linuztx/alter-zero/"),
            "https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh",
            "a trailing slash is tolerated"
        );
        assert_eq!(
            installer_url("http://127.0.0.1:8000"),
            "http://127.0.0.1:8000/install.sh",
            "a stand-in server serves it beside the releases"
        );
        assert_eq!(
            latest_url("http://127.0.0.1:8000/"),
            "http://127.0.0.1:8000/releases/latest"
        );
        assert_eq!(DEFAULT_REPO_URL, repo, "the manifest's repository");
    }

    #[test]
    fn enabled_by_env_uses_the_apps_on_off_grammar() {
        assert_eq!(enabled_by_env(None), None, "unset: the file decides");
        for off in ["0", "false", "no", "off", " OFF "] {
            assert_eq!(enabled_by_env(Some(off)), Some(false), "{off:?}");
        }
        for on in ["1", "true", "yes", "", "anything"] {
            assert_eq!(enabled_by_env(Some(on)), Some(true), "{on:?}");
        }
    }

    #[test]
    fn the_notice_names_both_versions_the_release_page_the_command_and_the_opt_out() {
        let text = notice("0.1.0", "0.2.0", DEFAULT_REPO_URL);
        assert!(
            text.starts_with(crate::APP_NAME),
            "speaks the app's name first: {text}"
        );
        assert!(text.contains("**v0.2.0**"), "{text}");
        assert!(text.contains("v0.1.0"), "{text}");
        assert!(
            text.contains("https://github.com/linuztx/alter-zero/releases/tag/v0.2.0"),
            "{text}"
        );
        assert!(text.contains("`alter-zero update`"), "{text}");
        assert!(text.contains("`/settings → Update check`"), "{text}");
        assert!(text.contains(&format!("`{UPDATE_ENV}=0`")), "{text}");
        assert_eq!(
            text.matches("\n\n").count(),
            1,
            "one blank line separates the opt-out"
        );
    }

    #[test]
    fn a_fetched_installer_must_look_like_the_installer() {
        let real = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
            .expect("the checkout's install.sh");
        assert!(looks_like_installer(&real), "the real script passes");
        for fake in [
            "",
            "<!DOCTYPE html><html>captive portal</html>",
            "#!/bin/sh\necho hi\n",
            "404: Not Found",
        ] {
            assert!(!looks_like_installer(fake), "{fake:?}");
        }
    }

    #[test]
    fn a_cargo_build_dir_is_recognised() {
        for dev in [
            "/home/u/src/alter-zero/target/debug/alter-zero",
            "/home/u/src/alter-zero/target/release/alter-zero",
            "/home/u/src/alter-zero/target/x86_64-unknown-linux-gnu/release/alter-zero",
            "target/debug/alter-zero",
        ] {
            assert!(is_cargo_build_dir(Path::new(dev)), "{dev}");
        }
        for installed in [
            "/home/u/.local/bin/alter-zero",
            "/usr/local/bin/alter-zero",
            "/home/u/.cargo/bin/alter-zero",
            "/home/u/target-practice/alter-zero",
            "/home/u/target/alter-zero",
        ] {
            assert!(!is_cargo_build_dir(Path::new(installed)), "{installed}");
        }
    }
}

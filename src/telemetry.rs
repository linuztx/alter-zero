//! Anonymous usage telemetry — **one ping a day per install**, so the project
//! can count how many people ran it and where they are, and nothing else
//! (`docs/telemetry.md`).
//!
//! This module is the pure half: the `telemetry.json` format
//! ([`TelemetryFile`]), the install id it keeps, the wire payload ([`Ping`] —
//! the *whole* body, pinned field by field by a test), the once-a-day decision
//! ([`should_ping`]) and the environment predicate ([`enabled_by_env`]). No
//! clock, no entropy, no filesystem, no network: the boundary
//! (`tui::telemetry`) reads the UTC date, draws the random bytes, does the
//! read-modify-write over the file, and spawns the send — the one impure
//! function here, [`send_ping`], is what that worker thread calls.
//!
//! The country is deliberately **not** in the payload: the collector reads it
//! off the connection at the edge and stores the two-letter code alone, so
//! the client never has to know its own address, let alone send it.

use serde::{Deserialize, Serialize};

use crate::APP_NAME;

/// The payload shape's version, sent as `v`, so a future shape can be told
/// from this one by the collector. `2` is `1` plus the optional [`Ping::distro`];
/// the collector still accepts `1`, because bumping this must not stop
/// counting everyone who has not updated yet.
pub const PAYLOAD_VERSION: u32 = 2;

/// Where the ping goes unless [`ENDPOINT_ENV`] says otherwise: the collector
/// in `telemetry/`, deployed as the Cloudflare Worker `alter-zero-telemetry`
/// on its account's `workers.dev` subdomain (`docs/telemetry.md` *Deploying
/// it*). A deployment that prints a different URL changes this constant.
pub const DEFAULT_ENDPOINT: &str = "https://alter-zero-telemetry.linuztx.workers.dev/v1/ping";

/// The file's name under the config home: `{config_home}/telemetry.json`.
pub const TELEMETRY_FILE_NAME: &str = "telemetry.json";

/// How many random bytes an install id is made of — 128 bits, hex-encoded to
/// 32 characters. Random rather than derived from the machine, so it
/// identifies an *install* and can never be reversed into a person.
pub const INSTALL_ID_BYTES: usize = 16;

/// The app's own on/off switch for a run (`0`/`false`/`no`/`off` = off, the
/// grammar every `ALTER_ZERO_*` flag uses). Seeds the `/settings` row; never
/// written back.
pub const TELEMETRY_ENV: &str = "ALTER_ZERO_TELEMETRY";

/// Overrides [`DEFAULT_ENDPOINT`] for a run — a fork's own collector, or the
/// smoke suite's local stub.
pub const ENDPOINT_ENV: &str = "ALTER_ZERO_TELEMETRY_URL";

/// The cross-tool opt-out (consoledonottrack.com): set to `1` it turns
/// telemetry off for the run, outranking even an explicit
/// `ALTER_ZERO_TELEMETRY=1`.
pub const DNT_ENV: &str = "DO_NOT_TRACK";

/// Where the Linux distribution's `ID` is read from, in systemd's own search
/// order. The boundary does the reading; this is here so the parser and the
/// paths it parses stay together.
pub const OS_RELEASE_PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

/// What a Linux system with no distribution `ID` reports — the os-release
/// spec's own default, and the honest answer for a minimal container.
pub const DEFAULT_DISTRO: &str = "linux";

/// The longest distribution id the wire carries; the collector refuses more.
pub const DISTRO_MAX_LEN: usize = 32;

/// Where macOS keeps its own version. Read directly rather than through
/// `sw_vers`: a subprocess at startup costs more than a file read, and this
/// file is the one `sw_vers` itself reports from.
pub const MACOS_VERSION_PLIST: &str = "/System/Library/CoreServices/SystemVersion.plist";

/// The longest platform version the wire carries.
pub const OS_VERSION_MAX_LEN: usize = 16;

/// What `telemetry.json` holds: the switch, the install id, the last day a
/// ping was delivered, and whether the one-time notice has been shown.
///
/// Every field is written on every save — this is a status file the user may
/// open, and `"enabled": true` on the page says more than its absence would —
/// and every field is optional on the way in ([`Self::parse`] is lenient), so
/// an old, partial, hand-edited or corrupt file reads as the defaults rather
/// than failing startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryFile {
    /// The user's standing choice — `true` until turned off (`/settings` →
    /// Telemetry). The environment can override it for a run
    /// ([`enabled_by_env`]) without moving it.
    pub enabled: bool,
    /// The install id: [`INSTALL_ID_BYTES`] random bytes as lowercase hex,
    /// minted once by [`Self::install_id_or_mint`] and kept for as long as
    /// the file lives. `None` until the first launch that needs one.
    pub install_id: Option<String>,
    /// The UTC date (`YYYY-MM-DD`) of the last ping the collector accepted —
    /// recorded only on a `2xx`, so an offline launch is retried by the next
    /// launch that day rather than lost.
    pub last_ping_day: Option<String>,
    /// Whether the one-time disclosure has been committed under the banner.
    pub notice_shown: bool,
}

impl Default for TelemetryFile {
    fn default() -> Self {
        Self {
            enabled: true,
            install_id: None,
            last_ping_day: None,
            notice_shown: false,
        }
    }
}

impl TelemetryFile {
    /// Parse a `telemetry.json` body, best-effort: malformed, empty or
    /// wrongly-typed JSON yields the defaults, so a bad file costs at most a
    /// fresh id and a repeated notice (`SessionSettings::parse`'s posture).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back — every field, always.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Whether the stored id is one this install can be counted under.
    #[must_use]
    pub fn has_valid_install_id(&self) -> bool {
        self.install_id.as_deref().is_some_and(is_valid_install_id)
    }

    /// The install id — the stored one when it is valid, else a new one
    /// minted from `random` (the boundary draws the bytes; the pure core
    /// never touches the entropy source, which is also what lets a test mint
    /// a known id). Minting replaces an invalid id rather than sending it:
    /// the collector would refuse it anyway, and a refused ping counts no one.
    pub fn install_id_or_mint(&mut self, random: [u8; INSTALL_ID_BYTES]) -> &str {
        if !self.has_valid_install_id() {
            self.install_id = Some(hex_id(&random));
        }
        self.install_id.as_deref().unwrap_or_default()
    }

    /// Note that the collector accepted today's ping.
    pub fn record_ping(&mut self, day: &str) {
        self.last_ping_day = Some(day.to_string());
    }
}

/// Exactly 32 lowercase hex characters — the shape the collector validates
/// against too, so the two can never disagree about what an id looks like.
#[must_use]
pub fn is_valid_install_id(id: &str) -> bool {
    id.len() == INSTALL_ID_BYTES * 2
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Lowercase hex of the id's bytes.
fn hex_id(bytes: &[u8; INSTALL_ID_BYTES]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The shape a distribution id must have to ride the wire: the os-release
/// spec's own charset for `ID` (lowercase letters, digits, `.`, `_`, `-`),
/// capped at [`DISTRO_MAX_LEN`]. The collector validates the same thing, so
/// the two can never disagree about what a distribution looks like.
#[must_use]
pub fn is_valid_distro(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= DISTRO_MAX_LEN
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// The shape a platform version must have to ride the wire: the version
/// number as its file writes it (`24.04`, `39`, `15.3.1`), lowercased, opening
/// on a letter or digit and capped at [`OS_VERSION_MAX_LEN`].
#[must_use]
pub fn is_valid_os_version(version: &str) -> bool {
    let mut bytes = version.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && version.len() <= OS_VERSION_MAX_LEN
        && bytes.all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// The distribution `ID` in an `os-release` file's contents — the pure half
/// of the read the boundary does over [`OS_RELEASE_PATHS`].
///
/// It takes the machine-readable `ID` and nothing else: not `PRETTY_NAME`,
/// which is free text, and not the version, which
/// [`os_version_from_os_release`] reads separately.
///
/// `Some(DEFAULT_DISTRO)` when the file names no `ID` (the spec's own
/// default), and `None` when it names one this wire cannot carry — a bad
/// field would cost the whole ping a `400`, and a refused ping counts nobody.
#[must_use]
pub fn distro_from_os_release(contents: &str) -> Option<String> {
    let Some(id) = os_release_value(contents, "ID") else {
        return Some(DEFAULT_DISTRO.to_string());
    };
    is_valid_distro(&id).then_some(id)
}

/// The distribution's own `VERSION_ID` — `24.04`, `39`, `12`.
///
/// `None` for a rolling release, which names none: `arch` with a made-up
/// number would say less than `arch` does. Deliberately **not** `VERSION`,
/// which is the free-text `24.04.1 LTS (Noble Numbat)`.
#[must_use]
pub fn os_version_from_os_release(contents: &str) -> Option<String> {
    let version = os_release_value(contents, "VERSION_ID")?;
    is_valid_os_version(&version).then_some(version)
}

/// macOS's own `ProductVersion` from [`MACOS_VERSION_PLIST`]'s contents —
/// `15.3.1`. Read as text rather than parsed as a plist: one key out of a
/// nine-line file does not need a parser, let alone a dependency.
///
/// The `<key>` is matched **with its tags**, so neither
/// `ProductBuildVersion` (`24D70`, and it comes first in the file) nor
/// `ProductUserVisibleVersion` (which contains this key's whole name) can be
/// picked up in its place.
#[must_use]
pub fn macos_version_from_plist(contents: &str) -> Option<String> {
    let after = contents.split_once("<key>ProductVersion</key>")?.1;
    let value = after.split_once("<string>")?.1.split_once("</string>")?.0;
    let value = value.trim().to_ascii_lowercase();
    is_valid_os_version(&value).then_some(value)
}

/// One `KEY=value` from an os-release file: unquoted, trimmed, lowercased,
/// and `None` when the key is absent or its value empty.
///
/// The key is matched with `split_once('=')` rather than a prefix, because
/// `ID_LIKE=debian` is Ubuntu's ancestry and not its identity.
fn os_release_value(contents: &str, key: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((found, value)) = line.split_once('=') else {
            continue;
        };
        if found.trim() != key {
            continue;
        }
        let value = unquote(value.trim()).trim().to_ascii_lowercase();
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// Strip one matching pair of shell quotes. `ID`'s charset excludes every
/// character an escape would protect, so there is nothing else to unescape.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// The wire body — **all** of it. Seven fields: the shape version, the
/// install id, the app version, the OS, the CPU architecture, on Linux the
/// distribution's `ID`, and the version of whichever of those two names the
/// platform. No prompt, path, model, key, hostname or user name is in reach
/// of this struct, and `the_payload_carries_exactly_the_seven_fields` pins
/// the key set so adding one is a deliberate act (`docs/telemetry.md` *What
/// is sent*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ping {
    /// [`PAYLOAD_VERSION`].
    pub v: u32,
    /// The install id ([`TelemetryFile::install_id_or_mint`]).
    pub id: String,
    /// `CARGO_PKG_VERSION`.
    pub version: String,
    /// `std::env::consts::OS`.
    pub os: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
    /// The Linux distribution's os-release `ID` — `ubuntu`, `arch`, `nixos`
    /// — and **never** its version or pretty name
    /// ([`distro_from_os_release`]). Omitted from the body entirely off
    /// Linux, where there is no such thing and a placeholder would only be a
    /// bucket the dashboard has to explain away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distro: Option<String>,
    /// The version of the platform this install runs on: on Linux the
    /// distribution's own `VERSION_ID` (`24.04`), on macOS the system's
    /// `ProductVersion` (`15.3.1`). Omitted when the platform names none —
    /// a rolling release, or a system this build cannot ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

impl Ping {
    /// A ping for this install, in the current payload shape. A `distro` or
    /// `os_version` that does not fit the wire shape is dropped rather than
    /// carried: one bad field is a `400`, and a refused ping counts nobody.
    #[must_use]
    pub fn new(
        id: &str,
        version: &str,
        os: &str,
        arch: &str,
        distro: Option<&str>,
        os_version: Option<&str>,
    ) -> Self {
        Self {
            v: PAYLOAD_VERSION,
            id: id.to_string(),
            version: version.to_string(),
            os: os.to_string(),
            arch: arch.to_string(),
            distro: distro
                .filter(|d| is_valid_distro(d))
                .map(std::string::ToString::to_string),
            os_version: os_version
                .filter(|v| is_valid_os_version(v))
                .map(std::string::ToString::to_string),
        }
    }

    /// The request body: compact JSON, a few dozen bytes.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Whether a ping is due: none has been delivered on `today` (the boundary's
/// UTC date, `YYYY-MM-DD`). The collector dedups on `(day, id)` as well, so
/// a second ping on one day — two machines on one config home, a clock jump
/// — still counts once.
#[must_use]
pub fn should_ping(file: &TelemetryFile, today: &str) -> bool {
    file.last_ping_day.as_deref() != Some(today)
}

/// What the environment says about telemetry for this run, given the raw
/// values of [`TELEMETRY_ENV`] and [`DNT_ENV`] (each `None` when unset):
///
/// - `DO_NOT_TRACK` set to a truthy value (`1`/`true`/`yes`/`on`) is
///   `Some(false)` whatever else says — a blanket statement outranks an app
///   default; a `DO_NOT_TRACK` that says nothing (`0`, empty) is unset;
/// - otherwise an explicit `ALTER_ZERO_TELEMETRY` answers in the app's own
///   grammar: `0`/`false`/`no`/`off` is `Some(false)`, anything else
///   `Some(true)`;
/// - neither set: `None` — defer to the file's `enabled`.
///
/// The verdict seeds the `/settings` row for the run and is never written
/// back (`config::apply_setting_overrides`, `docs/settings.md`).
#[must_use]
pub fn enabled_by_env(telemetry: Option<&str>, dnt: Option<&str>) -> Option<bool> {
    let truthy = |v: &str| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    };
    let falsy = |v: &str| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    };
    if dnt.is_some_and(truthy) {
        return Some(false);
    }
    telemetry.map(|v| !falsy(v))
}

/// The one-time disclosure committed under the banner on the first launch
/// that will ping (`ui::telemetry_notice_lines` frames and wraps it): what is
/// sent, what never is, and both ways to turn it off. Inline markdown marks
/// the labels and commands; newlines separate the details from the opt-out.
/// Built rather than a `const` so the app's name comes from [`APP_NAME`] like every other place the app
/// speaks its own name.
#[must_use]
pub fn notice() -> String {
    format!(
        "{APP_NAME} sends one anonymous ping a day to count active users.\n\
         **Shares** App version, OS and connection country.\n\
         **Never** Your prompts, files, keys or IP address.\n\n\
         **Opt out** `/settings → Telemetry` or `{TELEMETRY_ENV}=0`"
    )
}

/// The `User-Agent` a ping carries: `alter-zero/{version}`.
#[must_use]
pub fn user_agent(version: &str) -> String {
    format!("alter-zero/{version}")
}

/// Deliver one ping — the boundary function the worker thread calls. `Ok`
/// means the collector answered `2xx` and the day may be recorded; `Err`
/// carries the reason, which the caller drops (a failed ping is silent by
/// design — nothing the user can act on, and the next launch retries).
///
/// Rides the same cached client every provider request uses
/// (`llm::http_client` at the chat's own timeout) so the ping adds no second
/// connection pool and no second runtime thread to a process that idles in a
/// terminal all day (`docs/memory.md`); the shared builder's 10 s connect
/// timeout bounds a dead collector, and the thread is detached either way.
///
/// # Errors
/// The transport error, or a non-`2xx` status, as text.
pub fn send_ping(endpoint: &str, ping: &Ping) -> Result<(), String> {
    let client =
        crate::llm::http_client(crate::llm::openai::NET_OP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("user-agent", user_agent(&ping.version))
        .body(ping.to_json())
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("collector answered {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES_A: [u8; INSTALL_ID_BYTES] = [
        0x6f, 0x1c, 0x2a, 0x4d, 0x9e, 0x0b, 0x7c, 0x3a, 0x5f, 0x8e, 0x1d, 0x2c, 0x4b, 0x6a, 0x79,
        0x80,
    ];
    const BYTES_B: [u8; INSTALL_ID_BYTES] = [0xff; INSTALL_ID_BYTES];

    #[test]
    fn a_missing_or_corrupt_file_reads_as_the_defaults() {
        // A bad file must cost at most a new id and a repeated notice, never
        // the session — the `settings.json` posture.
        for text in ["", "{", "[]", "null", "{\"enabled\": \"maybe\"}"] {
            let file = TelemetryFile::parse(text);
            assert_eq!(file, TelemetryFile::default(), "{text:?}");
        }
        let d = TelemetryFile::default();
        assert!(d.enabled, "opt-out: on until turned off");
        assert!(d.install_id.is_none());
        assert!(d.last_ping_day.is_none());
        assert!(!d.notice_shown);
    }

    #[test]
    fn the_file_round_trips_with_every_field_written() {
        // A status file the user may open: every field is written, defaults
        // included, so `enabled: true` is visible rather than implied.
        let mut file = TelemetryFile::default();
        file.install_id_or_mint(BYTES_A);
        file.record_ping("2026-09-06");
        file.notice_shown = true;
        file.enabled = false;
        let json = file.to_json();
        for key in ["enabled", "install_id", "last_ping_day", "notice_shown"] {
            assert!(json.contains(&format!("\"{key}\"")), "{key}: {json}");
        }
        assert_eq!(TelemetryFile::parse(&json), file);
        let fresh = TelemetryFile::default().to_json();
        assert!(fresh.contains("\"enabled\": true"), "{fresh}");
    }

    #[test]
    fn an_install_id_is_minted_once_and_kept() {
        let mut file = TelemetryFile::default();
        assert!(!file.has_valid_install_id());
        let minted = file.install_id_or_mint(BYTES_A).to_string();
        assert_eq!(minted, "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980");
        assert!(file.has_valid_install_id());
        // A second mint with different bytes changes nothing: the id is the
        // install's identity for as long as the file lives.
        assert_eq!(file.install_id_or_mint(BYTES_B), minted);
        // Anything that is not a valid id is replaced rather than sent.
        for bad in [
            "",
            "short",
            "6F1C2A4D9E0B7C3A5F8E1D2C4B6A7980",
            "zz1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
        ] {
            file.install_id = Some(bad.to_string());
            assert!(!file.has_valid_install_id(), "{bad:?}");
            assert_eq!(
                file.install_id_or_mint(BYTES_B),
                "ff".repeat(INSTALL_ID_BYTES)
            );
        }
    }

    #[test]
    fn is_valid_install_id_accepts_exactly_32_lowercase_hex() {
        assert!(is_valid_install_id(&"a".repeat(32)));
        assert!(is_valid_install_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_valid_install_id(&"a".repeat(31)));
        assert!(!is_valid_install_id(&"a".repeat(33)));
        assert!(!is_valid_install_id(&"A".repeat(32)));
        assert!(!is_valid_install_id(&"g".repeat(32)));
        assert!(!is_valid_install_id(""));
    }

    #[test]
    fn the_distro_is_the_os_release_id_and_nothing_else() {
        // The machine-readable `ID`, never the pretty name and never the
        // version: "which distributions do we build for" is the question, and
        // `ubuntu 22.04.3 LTS (Jammy Jellyfish)` answers a narrower one about
        // one machine (`docs/telemetry.md` *The Linux distribution*).
        let ubuntu = "NAME=\"Ubuntu\"\nVERSION=\"22.04.3 LTS (Jammy Jellyfish)\"\nID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(distro_from_os_release(ubuntu).as_deref(), Some("ubuntu"));

        for (release, want) in [
            ("NAME=\"Arch Linux\"\nID=arch\n", "arch"),
            (
                "ID=\"opensuse-leap\"\nVERSION_ID=\"15.5\"\n",
                "opensuse-leap",
            ),
            ("ID='fedora'\nVERSION_ID=39\n", "fedora"),
            ("NAME=NixOS\nID=nixos\n", "nixos"),
            // Whitespace, comments, CRLF, and a key that merely starts with ID.
            (
                "# a comment\r\nID_LIKE=rhel\r\n  ID = centos \r\n",
                "centos",
            ),
            ("ID=ALPINE\n", "alpine"),
        ] {
            assert_eq!(
                distro_from_os_release(release).as_deref(),
                Some(want),
                "{release:?}"
            );
        }
    }

    #[test]
    fn an_os_release_without_a_usable_id_reports_plain_linux() {
        // systemd's own default when `ID` is unset — and the honest answer
        // for a minimal container that has no os-release at all.
        for release in ["", "NAME=\"Something\"\n", "ID=\n", "ID=\"\"\n"] {
            assert_eq!(
                distro_from_os_release(release).as_deref(),
                Some(DEFAULT_DISTRO),
                "{release:?}"
            );
        }
        assert_eq!(DEFAULT_DISTRO, "linux");
    }

    #[test]
    fn the_platform_version_is_the_distributions_own_version_id() {
        let ubuntu = "NAME=\"Ubuntu\"\nVERSION=\"24.04.1 LTS (Noble Numbat)\"\nID=ubuntu\nVERSION_ID=\"24.04\"\n";
        assert_eq!(os_version_from_os_release(ubuntu).as_deref(), Some("24.04"));

        for (release, want) in [
            ("ID=fedora\nVERSION_ID=39\n", "39"),
            ("ID=debian\nVERSION_ID=\"12\"\n", "12"),
            ("ID=nixos\nVERSION_ID=\"24.11\"\n", "24.11"),
            ("ID=rhel\nVERSION_ID=\"9.4\"\n", "9.4"),
            ("ID=alpine\nVERSION_ID=3.20.3\n", "3.20.3"),
        ] {
            assert_eq!(
                os_version_from_os_release(release).as_deref(),
                Some(want),
                "{release:?}"
            );
        }

        // A rolling release has no version, and saying so is the honest
        // answer — `arch` with a made-up number would be worse than `arch`.
        for rolling in [
            "ID=arch\n",
            "NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n",
        ] {
            assert_eq!(os_version_from_os_release(rolling), None, "{rolling:?}");
        }
        // And the same rule the distro has: unusable rather than refused.
        assert_eq!(os_version_from_os_release("VERSION_ID=\"a b\"\n"), None);
        assert_eq!(os_version_from_os_release("VERSION_ID=\"\"\n"), None);
    }

    #[test]
    fn the_macos_version_is_the_product_version_from_the_system_plist() {
        // The real file's shape, keys and all — including the two other keys
        // whose names *contain* the one we want.
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>ProductBuildVersion</key>
	<string>24D70</string>
	<key>ProductCopyright</key>
	<string>1983-2025 Apple Inc.</string>
	<key>ProductName</key>
	<string>macOS</string>
	<key>ProductUserVisibleVersion</key>
	<string>15.3.1</string>
	<key>ProductVersion</key>
	<string>15.3.1</string>
	<key>iOSSupportVersion</key>
	<string>18.3</string>
</dict>
</plist>
"#;
        assert_eq!(macos_version_from_plist(plist).as_deref(), Some("15.3.1"));
        // `ProductBuildVersion` sits *before* it and `ProductUserVisibleVersion`
        // contains its name: neither may be picked up instead.
        assert_ne!(macos_version_from_plist(plist).as_deref(), Some("24D70"));

        assert_eq!(
            macos_version_from_plist("<key>ProductVersion</key><string>14.7</string>").as_deref(),
            Some("14.7")
        );
        // Nothing usable in it, and nothing that looks like it.
        for junk in [
            "",
            "<plist><dict></dict></plist>",
            "<key>ProductBuildVersion</key><string>24D70</string>",
            "<key>ProductVersion</key><string></string>",
            "<key>ProductVersion</key><string>not a version</string>",
            "<key>ProductVersion</key>",
        ] {
            assert_eq!(macos_version_from_plist(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn a_version_the_collector_would_refuse_is_dropped_rather_than_sent() {
        assert!(is_valid_os_version("24.04"));
        assert!(is_valid_os_version("39"));
        assert!(is_valid_os_version("15.3.1"));
        assert!(is_valid_os_version("3.20.3"));
        assert!(is_valid_os_version("24.11pre"));
        assert!(!is_valid_os_version(""));
        assert!(!is_valid_os_version("24 04"));
        assert!(
            !is_valid_os_version(".24"),
            "a version starts with a digit or letter"
        );
        assert!(!is_valid_os_version(&"9".repeat(17)));
    }

    #[test]
    fn a_distro_the_collector_would_refuse_is_dropped_rather_than_sent() {
        // A ping carrying one bad field is a 400, and a 400 counts nobody —
        // so an id that does not fit the wire shape costs the field, never
        // the ping.
        for release in [
            "ID=a-very-long-distribution-identifier-well-past-the-cap\n",
            "ID=\"deb ian\"\n",
            "ID=\"<script>\"\n",
        ] {
            assert_eq!(distro_from_os_release(release), None, "{release:?}");
        }
        assert!(is_valid_distro("ubuntu"));
        assert!(is_valid_distro("opensuse-leap"));
        assert!(is_valid_distro("sles_sap"));
        assert!(is_valid_distro("centos.stream"));
        assert!(!is_valid_distro(""));
        assert!(!is_valid_distro("Ubuntu"));
        assert!(!is_valid_distro("deb ian"));
        assert!(!is_valid_distro(&"x".repeat(33)));
    }

    #[test]
    fn the_payload_carries_exactly_the_seven_fields() {
        // The whole wire body, pinned field by field — adding one here is a
        // deliberate act the doc must describe (`docs/telemetry.md`).
        let ping = Ping::new(
            "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
            "0.1.0",
            "linux",
            "x86_64",
            Some("ubuntu"),
            Some("24.04"),
        );
        let value: serde_json::Value = serde_json::from_str(&ping.to_json()).unwrap();
        let object = value.as_object().expect("an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["arch", "distro", "id", "os", "os_version", "v", "version"]
        );
        assert_eq!(object["v"], PAYLOAD_VERSION);
        assert_eq!(object["id"], "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980");
        assert_eq!(object["version"], "0.1.0");
        assert_eq!(object["os"], "linux");
        assert_eq!(object["arch"], "x86_64");
        assert_eq!(object["distro"], "ubuntu");
        assert_eq!(object["os_version"], "24.04");
        assert!(
            ping.to_json().len() < 200,
            "a few dozen bytes, not a document"
        );
    }

    #[test]
    fn a_machine_with_no_distribution_sends_no_distro_key_at_all() {
        // macOS and Windows have no distribution, and a placeholder in the
        // column would be a bucket the dashboard has to explain away — but a
        // Mac still names its own version.
        let mac = Ping::new(
            "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
            "0.1.0",
            "macos",
            "aarch64",
            None,
            Some("15.3.1"),
        );
        let value: serde_json::Value = serde_json::from_str(&mac.to_json()).unwrap();
        let object = value.as_object().expect("an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["arch", "id", "os", "os_version", "v", "version"]);
        assert_eq!(object["os_version"], "15.3.1");

        // A rolling distribution, or a platform that names neither: back to
        // the v1 shape exactly.
        let bare = Ping::new(
            "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
            "0.1.0",
            "windows",
            "x86_64",
            None,
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&bare.to_json()).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["arch", "id", "os", "v", "version"]);
    }

    #[test]
    fn a_ping_goes_once_per_utc_day() {
        let mut file = TelemetryFile::default();
        assert!(should_ping(&file, "2026-09-06"), "never pinged");
        file.record_ping("2026-09-06");
        assert!(!should_ping(&file, "2026-09-06"), "already today");
        assert!(should_ping(&file, "2026-09-07"), "a new day");
        assert_eq!(file.last_ping_day.as_deref(), Some("2026-09-06"));
    }

    #[test]
    fn the_environment_seeds_the_row_and_do_not_track_outranks_it() {
        // Unset: defer to the file.
        assert_eq!(enabled_by_env(None, None), None);
        // The app's own flag, in the app's own on/off grammar.
        assert_eq!(enabled_by_env(Some("0"), None), Some(false));
        assert_eq!(enabled_by_env(Some("off"), None), Some(false));
        assert_eq!(enabled_by_env(Some(" No "), None), Some(false));
        assert_eq!(enabled_by_env(Some("1"), None), Some(true));
        assert_eq!(enabled_by_env(Some(" TRUE "), None), Some(true));
        assert_eq!(enabled_by_env(Some("anything"), None), Some(true));
        // DO_NOT_TRACK=1 is a blanket statement: it wins even over an explicit
        // ALTER_ZERO_TELEMETRY=1.
        assert_eq!(enabled_by_env(None, Some("1")), Some(false));
        assert_eq!(enabled_by_env(Some("1"), Some("1")), Some(false));
        assert_eq!(enabled_by_env(Some("1"), Some("true")), Some(false));
        // A DO_NOT_TRACK that says nothing (0, empty) is not set.
        assert_eq!(enabled_by_env(None, Some("0")), None);
        assert_eq!(enabled_by_env(None, Some("")), None);
        assert_eq!(enabled_by_env(Some("1"), Some("0")), Some(true));
    }

    #[test]
    fn the_notice_names_the_app_and_both_off_switches() {
        let text = notice();
        assert!(text.starts_with(crate::APP_NAME), "{text}");
        for needle in [
            "one anonymous ping a day",
            "country",
            "**Never** Your prompts, files, keys",
            "IP address",
            "/settings",
            "Telemetry",
            "ALTER_ZERO_TELEMETRY=0",
        ] {
            assert!(text.contains(needle), "{needle:?} in {text}");
        }
    }

    #[test]
    fn the_user_agent_names_the_app_and_version() {
        assert_eq!(user_agent("0.1.0"), "alter-zero/0.1.0");
    }

    #[test]
    fn the_default_endpoint_is_the_collector_route_over_https() {
        assert!(
            DEFAULT_ENDPOINT.starts_with("https://"),
            "{DEFAULT_ENDPOINT}"
        );
        assert!(DEFAULT_ENDPOINT.ends_with("/v1/ping"), "{DEFAULT_ENDPOINT}");
    }
}

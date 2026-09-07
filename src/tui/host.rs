//! Host facts gathered at the I/O boundary — the impurities the pure library
//! never sees.
//!
//! `App` and `ui` are pure: they take a timestamp, a date, an OS string as
//! *values* (the `App::set_clock` pattern), so every read of the real world
//! funnels through here. Nothing in this module renders or decides anything;
//! each function answers one question about the machine we happen to be
//! running on.
//!
//! - the wall clock: [`local_timestamp`] (the Ctrl+O transcript's stamp,
//!   `docs/timestamps.md`), [`utc_stamp`] (rollout write stamps,
//!   `docs/resume.md`), [`unix_secs`] (input-history `ts`,
//!   `docs/history-persistence.md`),
//! - the agent's environment context: [`local_date`] and [`os_context`]
//!   (`docs/environment.md`),
//! - process identity: [`process_uid`] (the background tasks root,
//!   `docs/background.md`) and [`session_id`],
//! - telemetry (`docs/telemetry.md`): [`utc_day`] (the once-a-day key) and
//!   [`random_bytes`] (the install id's entropy).

use alter_zero::llm;

/// Local wall-clock stamp for recorded items: 12-hour time, no seconds, e.g.
/// `03:20 AM`. Injected via [`App::set_clock`](alter_zero::app::App::set_clock) and shown **only** under the
/// user's message in the Ctrl+O transcript — the one impurity kept out of the
/// pure library.
pub(crate) fn local_timestamp() -> String {
    chrono::Local::now().format("%I:%M %p").to_string()
}

/// Local date for the agent's environment context — weekday plus ISO date,
/// e.g. `Sunday 2026-07-19`. Gathered here at the boundary and folded into the
/// system prompt by [`augment_with_environment`] so the agent knows the day
/// (see `docs/environment.md`).
///
/// [`augment_with_environment`]: alter_zero::llm::backend::augment_with_environment
pub(crate) fn local_date() -> String {
    chrono::Local::now().format("%A %Y-%m-%d").to_string()
}

/// The OS string for the agent's environment context: the platform
/// (`std::env::consts::OS`) enriched, on Linux, with the distro from
/// `/etc/os-release` — e.g. `linux (Ubuntu 24.04.4 LTS)`. Boundary code (reads
/// the file); the parse is the pure `backend::os_release_name`. Falls back to
/// the bare platform when the file is missing/unreadable or off Linux (see
/// `docs/environment.md`).
pub(crate) fn os_context() -> String {
    let os = std::env::consts::OS;
    if os == "linux" {
        let distro = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
            .ok()
            .and_then(|contents| llm::backend::os_release_name(&contents));
        if let Some(distro) = distro {
            return format!("{os} ({distro})");
        }
    }
    os.to_string()
}

/// UTC write-time stamp for rollout lines — codex's
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` shape.
pub(crate) fn utc_stamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A unique-enough session id: nanos since the epoch plus the pid, in hex.
/// No uuid dependency — the id is never parsed back (resume goes by path);
/// it only has to keep concurrent instances off each other's files.
pub(crate) fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{nanos:x}-{:x}", std::process::id())
}

/// Today's UTC date, `YYYY-MM-DD` — the key the daily telemetry ping is
/// throttled on (`telemetry::should_ping`). UTC rather than local so the
/// client's day and the collector's agree except around midnight, where a
/// second ping is harmless (the collector dedups on the day too).
pub(crate) fn utc_day() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// `N` bytes from the OS entropy source — the telemetry install id's
/// material (`TelemetryFile::install_id_or_mint`). Falls back to the clock
/// and pid when the source fails, which should never happen on a real
/// system, so that a failure still yields an id unlikely to collide rather
/// than a shared all-zero one.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = nanos ^ u128::from(std::process::id()).rotate_left(64);
        for (slot, byte) in bytes.iter_mut().zip(seed.to_le_bytes().iter().cycle()) {
            *slot = *byte;
        }
    }
    bytes
}

/// Seconds since the Unix epoch — the `ts` stamped into each history line.
/// Impurity kept at the boundary (the `utc_stamp`/`session_id` pattern).
pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// This process's uid — the stable per-user segment of the background tasks
/// root (Claude Code's `claude-{uid}` pattern, `background::tasks_dir`). Read
/// from `/proc/self`'s owner: this crate forbids `unsafe`, so no `libc`
/// getuid. Falls back to 0 where `/proc` is absent (macOS) — `temp_dir()` is
/// already per-user there.
#[cfg(unix)]
pub(crate) fn process_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map_or(0, |meta| meta.uid())
}

/// Non-unix fallback: no uid concept to read — 0 keeps the path shape.
#[cfg(not(unix))]
pub(crate) fn process_uid() -> u32 {
    0
}

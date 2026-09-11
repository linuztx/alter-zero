//! The anonymous daily usage ping at the boundary (`docs/telemetry.md`): the
//! install id's mint, the one-time notice, the send, and the delivered day's
//! record — everything `telemetry` (the pure half) leaves to the side that
//! can read a clock, draw entropy, touch the file and spawn a thread.
//!
//! Four rules hold the whole thing to its promise:
//!
//! - **Nothing happens unless telemetry is active.** No config home, an
//!   environment that forbids it (`ALTER_ZERO_TELEMETRY=0`,
//!   `DO_NOT_TRACK=1`), or the user's own saved `false` all resolve to
//!   `telemetry_active() == false` before [`Session::telemetry_tick`] mints
//!   an id, shows a notice or spawns anything. A forbidding environment
//!   additionally makes the `/settings` row *unavailable*
//!   (`config::telemetry_forbidden_by_env`), so it cannot be cycled back on
//!   from inside the app — an opt-out that a keystroke could undo would not
//!   be one.
//! - **No ping before the disclosure.** Every path that can send goes
//!   through [`Session::telemetry_tick`], which commits the notice first
//!   when it has never been shown. Having the two call sites each remember
//!   to do that is how the `/settings` path came to send a ping with
//!   `notice_shown: false` still in the file.
//! - **After the first frame, never before it.** `Session::bootstrap` calls
//!   [`Session::start_telemetry`] once `paint_first_frame` has queued the
//!   banner, and the send runs on a detached worker
//!   (`workers::spawn_telemetry_ping`), so a slow or dead collector costs
//!   the user nothing.
//! - **The loop writes the file; the worker only reports.** The worker sends
//!   back the day it delivered and [`Session::on_telemetry_result`] records
//!   it, so `telemetry.json` has one writer thread and the `/settings`
//!   toggle can never race it. Every write is
//!   `config::update_telemetry_file`'s read-modify-write, so a day recorded
//!   while the user was turning telemetry off cannot resurrect their `true`.

use ratatui::text::Line;

use alter_zero::telemetry::{self, Ping, TelemetryFile};
use alter_zero::ui;

use super::workers::spawn_telemetry_ping;
use super::{Session, config, host};

impl Session<'_> {
    /// Start the day's telemetry, once per launch, after the first frame is
    /// queued.
    pub(crate) fn start_telemetry(&mut self) {
        self.telemetry_tick();
    }

    /// A turn start's day rollover: a session kept open across midnight
    /// counts on the new day too. This app idles in a terminal all day, so a
    /// launch-only ping would undercount exactly its heaviest users.
    ///
    /// Cheap on the common path — the day this session already attempted is
    /// held in memory, so a turn that is not the first of a new day costs a
    /// date read and a string compare and touches no file. That memory is
    /// also the bound on a *failing* collector: one attempt per UTC day per
    /// session rather than one per turn, with the file's `last_ping_day`
    /// (written only on a `2xx`) still retrying at the next launch.
    pub(crate) fn telemetry_day_check(&mut self) {
        if !self.app.settings().telemetry_active() {
            return;
        }
        if self.telemetry_attempted.as_deref() == Some(host::utc_day().as_str()) {
            return;
        }
        self.telemetry_tick();
    }

    /// The `/settings` **Telemetry** row moved: record the choice in
    /// `telemetry.json` (its own file — a user's preference, not a
    /// directory's, `docs/per-directory-state.md`) and, if it was just turned
    /// on, take today's step. Turning it off sends nothing more; a result
    /// already in flight still records its day, which the read-modify-write
    /// keeps from resurrecting the `true` the user just cleared.
    pub(crate) fn apply_telemetry_setting(&mut self) {
        let enabled = self.app.settings().telemetry;
        config::update_telemetry_file(config::telemetry_json_path().as_deref(), |file| {
            file.enabled = enabled;
        });
        self.telemetry_tick();
    }

    /// The worker delivered today's ping: record the day, so the next launch
    /// today sends nothing. The file's one writer is this loop thread.
    pub(crate) fn on_telemetry_result(&mut self, day: &str) {
        config::update_telemetry_file(config::telemetry_json_path().as_deref(), |file| {
            file.record_ping(day);
        });
    }

    /// The one path to a ping: mint the id if this install has none, show the
    /// disclosure if it has never been shown, then send when today's has not
    /// gone yet. Every caller — the launch, a turn's day rollover, the
    /// `/settings` row — goes through here, which is what makes "no ping
    /// before the notice" a property of the code rather than a habit.
    ///
    /// Silent and side-effect-free when telemetry is not active, so the
    /// callers need no guard of their own.
    fn telemetry_tick(&mut self) {
        if !self.app.settings().telemetry_active() {
            return;
        }
        let Some(path) = config::telemetry_json_path() else {
            return;
        };
        let file = config::update_telemetry_file(Some(&path), |file| {
            file.install_id_or_mint(host::random_bytes());
        });
        // Said once, and always before the first ping leaves the machine.
        if !file.notice_shown {
            self.commit_telemetry_notice();
            config::update_telemetry_file(Some(&path), |file| file.notice_shown = true);
        }
        self.send_ping_if_due(&file);
    }

    /// Spawn the send when `telemetry::should_ping` says today's has not been
    /// delivered, and remember that this session tried. The payload is the
    /// five fields and nothing else (`telemetry::Ping`): the id the file
    /// holds, the crate version, the OS and the architecture.
    fn send_ping_if_due(&mut self, file: &TelemetryFile) {
        let today = host::utc_day();
        if !telemetry::should_ping(file, &today) {
            return;
        }
        let Some(id) = file.install_id.as_deref().filter(|id| {
            // A file whose id will not validate is one the collector would
            // refuse anyway, and a refused ping counts nobody.
            telemetry::is_valid_install_id(id)
        }) else {
            return;
        };
        let (distro, os_version) = platform();
        let ping = Ping::new(
            id,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            distro.as_deref(),
            os_version.as_deref(),
        );
        // Marked before the spawn, not after the answer: this is "we tried
        // today", the bound on a collector that never answers.
        self.telemetry_attempted = Some(today.clone());
        spawn_telemetry_ping(
            config::telemetry_endpoint(),
            ping,
            today,
            self.telemetry_tx.clone(),
        );
    }

    /// Commit the one-time disclosure as scrollback chrome under the banner:
    /// framed and wrapped to the terminal (`ui::telemetry_notice_lines`) over a blank
    /// spacer, through the same queue the banner rides, so the next draw
    /// flushes both in one frame. Chrome, never `history`; a purge rebuild
    /// does not re-emit it.
    fn commit_telemetry_notice(&mut self) {
        let width = self.term.screen().width;
        self.term.insert_before(ui::telemetry_notice_lines(width));
        self.term.insert_before(vec![Line::default()]);
    }
}

/// The platform this install runs on, read at the boundary — the file reads
/// the pure parsers in [`telemetry`] cannot do:
///
/// - **Linux**: the distribution's `ID` and its `VERSION_ID`, from the first
///   of [`telemetry::OS_RELEASE_PATHS`] that opens (systemd's own search
///   order). One read serves both, since they are two keys of one file.
///   Neither file readable is [`telemetry::DEFAULT_DISTRO`] with no version —
///   the spec's own default, and the honest answer for a minimal container.
/// - **macOS**: the system's `ProductVersion`, and no distribution: macOS has
///   no such thing, and a placeholder would only be a bucket the dashboard
///   has to explain away.
/// - **Anything else**: neither. Reading Windows' version needs a registry
///   crate or a subprocess, and neither is worth a startup cost here yet.
fn platform() -> (Option<String>, Option<String>) {
    // Runtime `cfg!` rather than `#[cfg]` blocks, so every branch is compiled
    // — and type-checked — on every platform.
    if cfg!(target_os = "linux") {
        for path in telemetry::OS_RELEASE_PATHS {
            if let Ok(contents) = std::fs::read_to_string(path) {
                return (
                    telemetry::distro_from_os_release(&contents),
                    telemetry::os_version_from_os_release(&contents),
                );
            }
        }
        return (Some(telemetry::DEFAULT_DISTRO.to_string()), None);
    }
    if cfg!(target_os = "macos") {
        let version = std::fs::read_to_string(telemetry::MACOS_VERSION_PLIST)
            .ok()
            .and_then(|contents| telemetry::macos_version_from_plist(&contents));
        return (None, version);
    }
    (None, None)
}

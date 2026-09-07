//! The anonymous daily usage ping at the boundary (`docs/telemetry.md`): the
//! install id's mint, the one-time notice, the send, and the delivered day's
//! record — everything `telemetry` (the pure half) leaves to the side that
//! can read a clock, draw entropy, touch the file and spawn a thread.
//!
//! Three rules hold the whole thing to its promise:
//!
//! - **After the first frame, never before it.** `Session::bootstrap` calls
//!   [`Session::start_telemetry`] once `paint_first_frame` has queued the
//!   banner, and the send runs on a detached worker
//!   (`workers::spawn_telemetry_ping`), so a slow or dead collector costs the
//!   user nothing.
//! - **The loop writes the file; the worker only reports.** The worker sends
//!   back the day it delivered and [`Session::on_telemetry_result`] records
//!   it, so `telemetry.json` has one writer thread and the `/settings`
//!   toggle can never race it. Every write is `config::update_telemetry_file`'s
//!   read-modify-write.
//! - **Off means nothing happens.** No config home, the environment saying
//!   off (`ALTER_ZERO_TELEMETRY=0`, `DO_NOT_TRACK=1`) or the file saying off
//!   all resolve to `telemetry_active() == false` before this module mints an
//!   id, shows a notice or spawns anything.

use ratatui::text::Line;

use alter_zero::telemetry::{self, Ping, TelemetryFile};
use alter_zero::ui;

use super::workers::spawn_telemetry_ping;
use super::{Session, config, host};

impl Session<'_> {
    /// Start the day's telemetry, once per launch, after the first frame is
    /// queued: mint the install id if this is the first launch, commit the
    /// one-time disclosure under the banner, and — when no ping has been
    /// delivered today — hand the send to a worker. Off (by environment,
    /// file or a missing config home) does none of it.
    pub(crate) fn start_telemetry(&mut self) {
        if !self.app.settings().telemetry_active() {
            return;
        }
        let Some(path) = config::telemetry_json_path() else {
            return;
        };
        let file = config::update_telemetry_file(Some(&path), |file| {
            file.install_id_or_mint(host::random_bytes());
        });
        // Said once, before the first ping ever goes — and only on a launch
        // that will actually send, since a run the environment turned off
        // has nothing to disclose.
        if !file.notice_shown {
            self.commit_telemetry_notice();
            config::update_telemetry_file(Some(&path), |file| file.notice_shown = true);
        }
        self.send_ping_if_due(&file);
    }

    /// The `/settings` **Telemetry** row moved: record the choice in
    /// `telemetry.json` (its own file — a user's preference, not a
    /// directory's, `docs/per-directory-state.md`) and, if it was just turned
    /// on, send today's ping when none has gone. Turning it off sends nothing
    /// more; a result already in flight still records its day, harmlessly.
    pub(crate) fn apply_telemetry_setting(&mut self) {
        let enabled = self.app.settings().telemetry;
        let path = config::telemetry_json_path();
        config::update_telemetry_file(path.as_deref(), |file| file.enabled = enabled);
        if !self.app.settings().telemetry_active() {
            return;
        }
        let file = config::update_telemetry_file(path.as_deref(), |file| {
            file.install_id_or_mint(host::random_bytes());
        });
        self.send_ping_if_due(&file);
    }

    /// The worker delivered today's ping: record the day, so the next launch
    /// today sends nothing. The file's one writer is this loop thread.
    pub(crate) fn on_telemetry_result(&mut self, day: &str) {
        config::update_telemetry_file(config::telemetry_json_path().as_deref(), |file| {
            file.record_ping(day);
        });
    }

    /// Spawn the send when `telemetry::should_ping` says today's has not been
    /// delivered. The payload is the five fields and nothing else
    /// (`telemetry::Ping`): the id the file holds, the crate version, the OS
    /// and the architecture.
    fn send_ping_if_due(&self, file: &TelemetryFile) {
        let today = host::utc_day();
        if !telemetry::should_ping(file, &today) || !file.has_valid_install_id() {
            return;
        }
        let Some(id) = file.install_id.as_deref() else {
            return;
        };
        let ping = Ping::new(
            id,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        spawn_telemetry_ping(
            config::telemetry_endpoint(),
            ping,
            today,
            self.telemetry_tx.clone(),
        );
    }

    /// Commit the one-time disclosure as scrollback chrome under the banner:
    /// wrapped to the terminal (`ui::startup_paragraph_lines`) over a blank
    /// spacer, through the same queue the banner rides, so the next draw
    /// flushes both in one frame. Chrome, never `history`; a purge rebuild
    /// does not re-emit it.
    fn commit_telemetry_notice(&mut self) {
        let width = self.term.screen().width;
        self.term
            .insert_before(ui::startup_paragraph_lines(&telemetry::notice(), width));
        self.term.insert_before(vec![Line::default()]);
    }
}

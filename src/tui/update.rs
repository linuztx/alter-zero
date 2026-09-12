//! The once-a-day update check at the boundary (`docs/update.md`): the
//! request, the card under the banner, the recorded day — everything
//! `update` (the pure half) leaves to the side that can read a clock, touch
//! the file and spawn a thread. `tui::telemetry`'s twin, and it keeps the
//! same four rules:
//!
//! - **Nothing happens unless the check is active.** No config home, an
//!   environment that forbids it (`ALTER_ZERO_UPDATE_CHECK=0`), or the
//!   user's own saved `false` all resolve to `update_check_active() ==
//!   false` before [`Session::update_tick`] reads the file or spawns
//!   anything, and a forbidding environment makes the `/settings` row
//!   *unavailable* (`config::update_check_forbidden_by_env`).
//! - **One request a day, whatever happens to it.** The attempt's day is
//!   recorded at the spawn, so a dead network or a repository with no
//!   release yet costs one request per day per install and nothing the
//!   user sees.
//! - **After the first frame, never before it** — and **never inside a
//!   reply**: a result that lands while a turn streams waits in
//!   `update_notice_pending` for the first idle loop bottom
//!   ([`Session::flush_pending_update_notice`]), so the card can never
//!   interleave with the text a model is producing.
//! - **The loop writes the file; the worker only reports.** The worker
//!   sends back the version it found and [`Session::on_update_result`]
//!   records it, so `update.json` has one writer thread.

use ratatui::text::Line;

use alter_zero::ui;
use alter_zero::update;

use super::workers::spawn_update_check;
use super::{Session, config, host};

impl Session<'_> {
    /// Start the day's check, once per launch, after the first frame is
    /// queued.
    pub(crate) fn start_update_check(&mut self) {
        self.update_tick();
    }

    /// A turn start's day rollover — `telemetry_day_check`'s twin: cheap on
    /// the common path (a date read and a string compare), and the bound on
    /// a failing request within one session.
    pub(crate) fn update_day_check(&mut self) {
        if !self.app.settings().update_check_active() {
            return;
        }
        if self.update_attempted.as_deref() == Some(host::utc_day().as_str()) {
            return;
        }
        self.update_tick();
    }

    /// The `/settings` **Update check** row moved: record the choice in
    /// `update.json` and, if it was just turned on, take today's step.
    pub(crate) fn apply_update_setting(&mut self) {
        let enabled = self.app.settings().update_check;
        config::update_update_file(config::update_json_path().as_deref(), |file| {
            file.enabled = enabled;
        });
        self.update_tick();
    }

    /// The worker found the newest release: record it, and announce it if it
    /// is newer than this build and the card has not been shown today —
    /// now when the session is idle, else at the next idle loop bottom.
    pub(crate) fn on_update_result(&mut self, latest: &str) {
        let today = host::utc_day();
        let file = config::update_update_file(config::update_json_path().as_deref(), |file| {
            file.record_check(&today, Some(latest));
        });
        if let Some(latest) = update::notice_due(&file, env!("CARGO_PKG_VERSION"), &today) {
            self.update_notice_pending = Some(latest);
            self.flush_pending_update_notice();
        }
    }

    /// Commit a pending card once nothing is streaming and commits are
    /// allowed (invariant 4) — called from `on_update_result` and from the
    /// loop bottom, so a card deferred by a running turn lands right after it.
    pub(crate) fn flush_pending_update_notice(&mut self) {
        if self.update_notice_pending.is_none()
            || self.inflight.is_some()
            || self.app.is_streaming()
            || !self.commits_allowed()
        {
            return;
        }
        let Some(latest) = self.update_notice_pending.take() else {
            return;
        };
        let today = host::utc_day();
        // Re-check against the file: the day may have rolled, or the card
        // may have been shown by a launch since this result was queued.
        let file = config::load_update_file(config::update_json_path().as_deref());
        if update::notice_due(&file, env!("CARGO_PKG_VERSION"), &today).as_deref() != Some(&*latest)
        {
            return;
        }
        self.commit_update_notice(&latest);
        config::update_update_file(config::update_json_path().as_deref(), |file| {
            file.record_notice(&today);
        });
        self.frame.schedule_frame();
    }

    /// The one path to a check: announce a newer release the file already
    /// knows of (once a day), then spawn today's request when none has run.
    /// Every caller — the launch, a turn's day rollover, the `/settings`
    /// row — goes through here. Silent and side-effect-free when the check
    /// is not active, so the callers need no guard of their own.
    fn update_tick(&mut self) {
        if !self.app.settings().update_check_active() {
            return;
        }
        let Some(path) = config::update_json_path() else {
            return;
        };
        let today = host::utc_day();
        let file = config::load_update_file(Some(&path));
        if let Some(latest) = update::notice_due(&file, env!("CARGO_PKG_VERSION"), &today) {
            self.update_notice_pending = Some(latest);
            self.flush_pending_update_notice();
        }
        if !update::should_check(&file, &today) {
            return;
        }
        // Marked before the spawn, not after the answer: "we tried today" is
        // the bound on a request that never answers.
        config::update_update_file(Some(&path), |file| file.record_check(&today, None));
        self.update_attempted = Some(today);
        spawn_update_check(
            config::update_repo_url(),
            env!("CARGO_PKG_VERSION").to_string(),
            self.update_tx.clone(),
        );
    }

    /// Commit the card as scrollback chrome: framed and wrapped to the
    /// terminal (`ui::update_notice_lines`) over a blank spacer, through the
    /// same queue the banner rides. Chrome, never `history`; a purge rebuild
    /// does not re-emit it.
    fn commit_update_notice(&mut self, latest: &str) {
        let width = self.term.screen().width;
        self.term.insert_before(ui::update_notice_lines(
            width,
            env!("CARGO_PKG_VERSION"),
            latest,
            &config::update_repo_url(),
        ));
        self.term.insert_before(vec![Line::default()]);
    }
}

//! The `/usage` page at the boundary: everything it cannot know on its own
//! (`docs/usage-limits.md`).
//!
//! The pure side owns the page and the session's store of every
//! subscription's limits. What the boundary adds is what lies outside the
//! process: which subscriptions are signed in (handed over when the page
//! opens), the fetches (one worker per subscription, on the open, on `r`,
//! and once a minute while the page stays open), each answer as it lands,
//! and the wall clock before every draw.

use std::time::Duration;

use alter_zero::app::{ToastKind, USAGE_LIMITS_EMPTY_NOTICE};
use alter_zero::clipboard;
use alter_zero::llm::usage_limits::PlanLimits;
use alter_zero::stream::CancelToken;

use super::workers::spawn_limits_fetch;
use super::{Session, host};

/// The page's own frame chain. Its countdowns and its freshness move by the
/// minute, so a frame a second is more than they need — the `/login` device
/// page's cadence, never the status animation's thirty a second.
pub(crate) const USAGE_LIMITS_TICK: Duration = Duration::from_secs(1);

/// The wall clock, as the page's Unix seconds.
fn now() -> i64 {
    i64::try_from(host::unix_secs()).unwrap_or(i64::MAX)
}

impl Session<'_> {
    /// `/usage`: open the page over the signed-in subscriptions and fetch
    /// each one's limits — or, with none signed in, say so in a toast and
    /// open nothing (the `/export` empty rule).
    pub(crate) fn open_usage_limits(&mut self) {
        let entries = self.models.subscription_entries();
        if entries.is_empty() {
            self.toast(USAGE_LIMITS_EMPTY_NOTICE, ToastKind::Info);
            return;
        }
        self.app.open_usage_limits(entries);
        self.app.set_usage_limits_now(now());
        self.refresh_usage_limits();
    }

    /// Fetch every listed subscription's limits again, in parallel: the
    /// fetch before it (if any is still out) is cancelled, and the new
    /// refresh's generation rides every worker so `App` drops a late answer
    /// to the old one.
    pub(crate) fn refresh_usage_limits(&mut self) {
        self.cancel_usage_limits();
        let generation = self.app.begin_usage_limits_refresh(now());
        let cancel = CancelToken::new();
        self.limits_cancel = Some(cancel.clone());
        let ids: Vec<String> = self
            .app
            .subscription_limits()
            .iter()
            .map(|subscription| subscription.entry.id.clone())
            .collect();
        for id in ids {
            let cfg = self.models.subscription_config(&id);
            spawn_limits_fetch(id, generation, cfg, cancel.clone(), self.limits_tx.clone());
        }
        self.frame.schedule_frame();
    }

    /// Cancel the fetches still out — the page closed, or the app is
    /// quitting. Their detached workers send nothing once it trips.
    pub(crate) fn cancel_usage_limits(&mut self) {
        if let Some(cancel) = self.limits_cancel.take() {
            cancel.cancel();
        }
    }

    /// One subscription's answer landed: into the store, stamped with the
    /// wall clock, and painted.
    pub(crate) fn on_limits_result(
        &mut self,
        id: &str,
        generation: u64,
        result: Result<PlanLimits, String>,
    ) {
        self.app.set_plan_limits(id, generation, result, now());
        self.frame.schedule_frame();
    }

    /// Before a draw while the page is open: hand it the wall clock, and
    /// refresh once a minute has passed since the last refresh began
    /// (`App::usage_limits_refresh_due` decides).
    pub(crate) fn tick_usage_limits(&mut self) {
        if self.app.usage_limits.is_none() {
            return;
        }
        let now = now();
        self.app.set_usage_limits_now(now);
        if self.app.usage_limits_refresh_due(now) {
            self.refresh_usage_limits();
        }
    }

    /// `c` on the page: the highlighted subscription's usage link to the
    /// clipboard, through `/copy`'s path, the page staying open.
    pub(crate) fn copy_usage_link(&mut self, url: &str) {
        match clipboard::copy_to_clipboard(url) {
            Ok(lease) => {
                self.clipboard_lease = lease;
                self.toast("Copied the usage link to clipboard", ToastKind::Info);
            }
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }
}

//! The `/usage` page at the boundary: reading every signed-in subscription
//! off its provider, and keeping the open page live (`docs/usage.md`).
//!
//! The pure side owns the page and its rounds ([`App::begin_usage_round`],
//! [`App::finish_usage_fetch`]). What the boundary adds is what `App` cannot
//! know or do: which subscriptions are signed in (`ModelSession::
//! usage_targets`), the reads themselves — one worker each, so a slow
//! provider never holds the others back — the wall clock the countdowns run
//! on, and the minute refresh.
//!
//! [`App::begin_usage_round`]: alter_zero::app::App::begin_usage_round
//! [`App::finish_usage_fetch`]: alter_zero::app::App::finish_usage_fetch

use std::time::Duration;

use alter_zero::llm::usage::UsageReport;
use alter_zero::stream::CancelToken;

use super::Session;
use super::host;
use super::workers::spawn_usage_fetch;

/// A finished read: the round it was asked for, the provider it read, and
/// the report or a one-line reason.
pub(crate) type UsageResult = (u64, String, Result<UsageReport, String>);

/// How often an open page is redrawn with no other event — its countdowns
/// run in minutes, so a frame every few seconds keeps them true without the
/// status chain's thirty a second (a kitty cursor trail would shimmer over a
/// still page at that rate, the `/login` device page's lesson).
const USAGE_TICK_INTERVAL: Duration = Duration::from_secs(5);

impl Session<'_> {
    /// Start a round of reads for the open page: say which subscriptions are
    /// signed in, then read each one on a worker of its own. Whatever an
    /// earlier round still has in flight is cancelled — a result it sends
    /// anyway names a round the page no longer keeps, and is dropped there.
    pub(crate) fn fetch_usage(&mut self) {
        self.cancel_usage_fetch();
        let targets = self.models.usage_targets();
        let Some(round) = self
            .app
            .begin_usage_round(targets.iter().map(|(target, _)| target.clone()).collect())
        else {
            return;
        };
        let cancel = CancelToken::new();
        self.usage_cancel = Some(cancel.clone());
        for (target, cfg) in targets {
            spawn_usage_fetch(
                round,
                target.provider,
                cfg,
                cancel.clone(),
                self.usage_tx.clone(),
            );
        }
        self.frame.schedule_frame();
    }

    /// Cancel the reads in flight — the page closed, a newer round replaced
    /// them, or the app is quitting. Each detached worker exits on the cancel
    /// without sending.
    pub(crate) fn cancel_usage_fetch(&mut self) {
        if let Some(cancel) = self.usage_cancel.take() {
            cancel.cancel();
        }
    }

    /// A read landed: hand it to the page, which drops it unless it names the
    /// page's own round, and repaint.
    pub(crate) fn on_usage_result(&mut self, (round, provider, result): UsageResult) {
        self.app
            .finish_usage_fetch(round, &provider, result, host::unix_secs_signed());
        self.frame.schedule_frame();
    }

    /// The open page's clock and its minute refresh, run every draw: a
    /// round due is started, and a slow frame stays scheduled so the
    /// countdowns move with no other event. Nothing at all while the page is
    /// closed.
    pub(crate) fn tick_usage_page(&mut self) {
        if self.app.usage_page.is_none() {
            return;
        }
        if self.app.usage_refresh_due() {
            self.fetch_usage();
        }
        self.frame.schedule_frame_in(USAGE_TICK_INTERVAL);
    }
}

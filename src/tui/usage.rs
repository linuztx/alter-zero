//! The `/usage` page at the boundary: reading every subscription off its
//! provider, and keeping the open page live (`docs/usage.md`).
//!
//! The pure side owns the page and its rounds ([`App::open_usage_page`],
//! [`App::begin_usage_round`], [`App::finish_usage_fetch`]). What the
//! boundary adds is what `App` cannot know or do: which subscriptions are
//! signed in (`ModelSession::usage_targets`), the reads themselves — one
//! worker each, so a slow provider never holds the others back — the wall
//! clock the countdowns run on, the minute refresh, and the clipboard `c`
//! copies a usage page to.
//!
//! [`App::open_usage_page`]: alter_zero::app::App::open_usage_page
//! [`App::begin_usage_round`]: alter_zero::app::App::begin_usage_round
//! [`App::finish_usage_fetch`]: alter_zero::app::App::finish_usage_fetch

use std::time::Duration;

use alter_zero::app::{ToastKind, USAGE_EMPTY_NOTICE, UsageTarget};
use alter_zero::clipboard;
use alter_zero::llm::ModelConfig;
use alter_zero::llm::usage::UsageReport;
use alter_zero::stream::CancelToken;

use super::Session;
use super::host;
use super::workers::spawn_usage_fetch;

/// A finished read: the round it was asked for, the provider it read, and
/// the report or a one-line reason.
pub(crate) type UsageResult = (u64, String, Result<UsageReport, String>);

/// How often an open page is redrawn with no other event — its countdowns
/// and its freshness run in minutes, so a frame every few seconds keeps them
/// true without the status chain's thirty a second (a kitty cursor trail
/// would shimmer over a still page at that rate).
const USAGE_TICK_INTERVAL: Duration = Duration::from_secs(5);

impl Session<'_> {
    /// `/usage`: open the page over the signed-in subscriptions and read
    /// each one — or, with none signed in, say so in a toast and open
    /// nothing (the `/export` empty rule).
    pub(crate) fn open_usage_page(&mut self) {
        self.cancel_usage_fetch();
        let (targets, configs) = split(self.models.usage_targets());
        let Some(round) = self.app.open_usage_page(targets) else {
            self.toast(USAGE_EMPTY_NOTICE, ToastKind::Info);
            return;
        };
        self.app.set_usage_now(host::unix_secs_signed());
        self.spawn_usage_reads(round, configs);
    }

    /// Read every signed-in subscription again on the open page — `r`, or
    /// the minute refresh: a subscription signed in or out since is added
    /// or dropped, and whatever an earlier round still has in flight is
    /// cancelled (a result it sends anyway names a round the page no longer
    /// keeps, and is dropped there).
    pub(crate) fn refresh_usage_page(&mut self) {
        self.cancel_usage_fetch();
        let (targets, configs) = split(self.models.usage_targets());
        if let Some(round) = self.app.begin_usage_round(targets) {
            self.spawn_usage_reads(round, configs);
        }
    }

    /// One worker per subscription, all on one cancel token.
    fn spawn_usage_reads(&mut self, round: u64, configs: Vec<(String, ModelConfig)>) {
        let cancel = CancelToken::new();
        self.usage_cancel = Some(cancel.clone());
        for (provider, cfg) in configs {
            spawn_usage_fetch(round, provider, cfg, cancel.clone(), self.usage_tx.clone());
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

    /// The open page's minute refresh, run every draw: a round due is
    /// started, and — while the page is on screen, never under an overlay,
    /// whose return re-seeds it — a slow frame stays scheduled so the
    /// countdowns and the freshness move with no other event. Nothing at all
    /// while the page is closed.
    pub(crate) fn tick_usage_page(&mut self) {
        if self.app.usage_page.is_none() {
            return;
        }
        if self.app.usage_refresh_due() {
            self.refresh_usage_page();
        }
        if !self.app.view.is_overlay() {
            self.frame.schedule_frame_in(USAGE_TICK_INTERVAL);
        }
    }

    /// `c` on the page: the highlighted subscription's usage page to the
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

/// The targets the page lists, and the config each one's read resolves its
/// credential through, by provider.
fn split(found: Vec<(UsageTarget, ModelConfig)>) -> (Vec<UsageTarget>, Vec<(String, ModelConfig)>) {
    found
        .into_iter()
        .map(|(target, cfg)| {
            let provider = target.provider.clone();
            (target, (provider, cfg))
        })
        .unzip()
}

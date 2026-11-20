//! The `/plugins` page at the boundary: opening it, running what it asks on
//! a worker thread, and folding the answers back in (`docs/plugins.md`).
//!
//! The pure side owns the page, its rounds and every rule about what an
//! answer does to it ([`App::open_plugins_menu`],
//! [`App::apply_plugin_event`]). What the boundary adds is what `App` cannot
//! do: the work itself — every [`PluginOp`] but `Cancel` on a thread of its
//! own, so the loop never waits on `git` or the disk — the cancel tokens
//! that stop it, the page clock the spinners and elapsed times run on, and
//! the session's reload once a plugin changed.
//!
//! The work is one function at the bottom of this file, [`run_plugin_op`],
//! behind a [`PluginWorkerControl`]: what it reports goes out through the
//! control as [`PluginEvent`]s, and the event that closes the operation is
//! its return value.
//!
//! [`App::open_plugins_menu`]: alter_zero::app::App::open_plugins_menu
//! [`App::apply_plugin_event`]: alter_zero::app::App::apply_plugin_event

use std::sync::Mutex;
use std::time::Instant;

use alter_zero::app::{PluginEvent, PluginOp};
use alter_zero::plugins::catalog::PluginCatalog;
use alter_zero::stream::CancelToken;

use super::Session;
use super::config;
use super::host;
use super::workers::spawn_plugin_op;

/// What the page's worker sends: the round it answers, and the event.
pub(crate) type PluginMessage = (u64, PluginEvent);

impl Session<'_> {
    /// `/plugins`: open the page — a fresh round — and start reading the
    /// catalog, fetching what was never fetched or is stale.
    pub(crate) fn open_plugins_menu(&mut self) {
        self.cancel_plugin_work();
        self.app.open_plugins_menu();
        self.plugins_opened = Some(Instant::now());
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        self.app.set_plugins_home(
            config::config_home().map(|path| alter_zero::ui::display_cwd(&path, home.as_deref())),
        );
        self.update_plugins_clock();
        self.start_plugin_op(PluginOp::Refresh { force: false });
    }

    /// One operation the page asked for. `Cancel` stops the running job;
    /// everything else runs on a worker thread of its own, on its own cancel
    /// token — a job's kept apart so Esc stops it alone, every token kept so
    /// closing the page stops them all.
    pub(crate) fn start_plugin_op(&mut self, op: PluginOp) {
        if op == PluginOp::Cancel {
            if let Some(cancel) = self.plugins_job.take() {
                cancel.cancel();
            }
            return;
        }
        let Some(round) = self.app.plugins_menu.as_ref().map(|menu| menu.round) else {
            return;
        };
        let cancel = CancelToken::new();
        if is_job(&op)
            && let Some(previous) = self.plugins_job.replace(cancel.clone())
        {
            previous.cancel();
        }
        self.plugins_work.push(cancel.clone());
        spawn_plugin_op(round, op, cancel, self.plugins_tx.clone());
        self.frame.schedule_frame();
    }

    /// Stop everything the page has in flight — it closed, or the app is
    /// quitting. Each worker sees its token and sends nothing more; what it
    /// had already sent names a round no open page has.
    pub(crate) fn cancel_plugin_work(&mut self) {
        for cancel in self.plugins_work.drain(..) {
            cancel.cancel();
        }
        self.plugins_job = None;
        self.plugins_opened = None;
    }

    /// An answer from the page's worker: folded into the page (which drops
    /// it unless it names the page's own round), and a plugin that changed
    /// reloads the session's snapshot of them.
    pub(crate) fn on_plugin_event(&mut self, (round, event): PluginMessage) {
        let changed = matches!(
            &event,
            PluginEvent::JobFinished { result: Ok(_) }
                | PluginEvent::EnabledSet { result: Ok(_), .. }
        );
        if matches!(event, PluginEvent::JobFinished { .. }) {
            self.plugins_job = None;
        }
        self.app.apply_plugin_event(round, event);
        if changed {
            self.plugins_changed();
        }
        self.frame.schedule_frame();
    }

    /// The page clock, before a draw: how long the page has been open, and
    /// the wall clock its ages count from. Nothing while it is closed.
    pub(crate) fn update_plugins_clock(&mut self) {
        if self.app.plugins_menu.is_none() {
            return;
        }
        if let Some(opened) = self.plugins_opened {
            self.app.set_plugins_clock(opened.elapsed());
        }
        self.app.set_plugins_now(host::unix_secs_signed());
    }

    /// A plugin was installed, updated, removed, enabled or disabled, or a
    /// marketplace changed. This is where the session reloads its snapshot
    /// of the enabled plugins, so their skills, agents and commands are
    /// offered from the very next message (and their hooks and MCP servers
    /// queue for the startup review at the next launch); with no plugin
    /// store in this build there is nothing to reload yet.
    pub(crate) fn plugins_changed(&mut self) {}
}

/// Whether `op` is the page's foreground job — what Esc cancels — rather
/// than a read or a fetch, which run beside it.
fn is_job(op: &PluginOp) -> bool {
    matches!(
        op,
        PluginOp::Install { .. }
            | PluginOp::Update { .. }
            | PluginOp::Uninstall { .. }
            | PluginOp::AddMarketplace { .. }
            | PluginOp::UpdateMarketplace { .. }
            | PluginOp::RemoveMarketplace { .. }
            | PluginOp::RestoreDefault { .. }
    )
}

/// The worker's side of one operation: the round it answers, the token
/// that stops it, and the channel it reports on — the shape the plugin
/// store's progress callbacks are adapted to.
///
/// It also answers for the operation if the worker unwinds: a dropped
/// control on a panicking thread sends the failure that closes what it was
/// doing ([`closing_failure`]), so a page never waits forever on a worker
/// that died — the `/model` fetch's `AnswerOnPanic` rule.
pub(crate) struct PluginWorkerControl {
    round: u64,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<PluginMessage>,
    op: PluginOp,
    /// The marketplace a fetch has been started for and not yet finished.
    open_fetch: Mutex<Option<String>>,
}

impl PluginWorkerControl {
    pub(crate) fn new(
        round: u64,
        op: PluginOp,
        cancel: CancelToken,
        tx: tokio::sync::mpsc::UnboundedSender<PluginMessage>,
    ) -> Self {
        Self {
            round,
            cancel,
            tx,
            op,
            open_fetch: Mutex::new(None),
        }
    }

    /// Whether the page stopped this operation (Esc, or the page closed).
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Send one event for this round — nothing once cancelled.
    pub(crate) fn send(&self, event: PluginEvent) {
        if self.cancelled() {
            return;
        }
        if let PluginEvent::FetchFinished { marketplace, .. } = &event {
            let mut open = self.open_fetch.lock().unwrap_or_else(|e| e.into_inner());
            if open.as_deref() == Some(marketplace) {
                *open = None;
            }
        }
        let _ = self.tx.send((self.round, event));
    }

    /// The job's progress: the step under way and git's percentage.
    pub(crate) fn progress(&self, step: Option<String>, percent: Option<u8>) {
        self.send(PluginEvent::JobProgress { step, percent });
    }

    /// A marketplace fetch's progress — the first report for a marketplace
    /// also announces the fetch, so the page shows it from its first byte.
    pub(crate) fn fetch_progress(
        &self,
        marketplace: &str,
        phase: Option<String>,
        percent: Option<u8>,
    ) {
        let started = {
            let mut open = self.open_fetch.lock().unwrap_or_else(|e| e.into_inner());
            let started = open.as_deref() != Some(marketplace);
            *open = Some(marketplace.to_string());
            started
        };
        if started {
            self.send(PluginEvent::FetchStarted {
                marketplace: marketplace.to_string(),
            });
        }
        if phase.is_some() || percent.is_some() {
            self.send(PluginEvent::FetchProgress {
                marketplace: marketplace.to_string(),
                phase,
                percent,
            });
        }
    }
}

impl Drop for PluginWorkerControl {
    fn drop(&mut self) {
        if !std::thread::panicking() || self.cancelled() {
            return;
        }
        let open = self
            .open_fetch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(marketplace) = open {
            let _ = self.tx.send((
                self.round,
                PluginEvent::FetchFinished {
                    marketplace,
                    result: Err(PLUGIN_WORKER_CRASHED.to_string()),
                },
            ));
        }
        let _ = self
            .tx
            .send((self.round, closing_failure(&self.op, PLUGIN_WORKER_CRASHED)));
    }
}

/// What a worker that stopped unexpectedly answers.
pub(crate) const PLUGIN_WORKER_CRASHED: &str = "the plugin worker stopped unexpectedly";

/// The event that closes `op` with a failure, `reason` — what the page
/// waits for after each kind of operation.
pub(crate) fn closing_failure(op: &PluginOp, reason: &str) -> PluginEvent {
    let reason = reason.to_string();
    match op {
        PluginOp::Refresh { .. } => PluginEvent::CatalogFailed(reason),
        PluginOp::LoadDetail { id } => PluginEvent::Detail {
            id: id.clone(),
            result: Err(reason),
        },
        PluginOp::SetEnabled { id, enabled } => PluginEvent::EnabledSet {
            id: id.clone(),
            enabled: *enabled,
            result: Err(reason),
        },
        PluginOp::FetchMarketplace { marketplace } => PluginEvent::FetchFinished {
            marketplace: marketplace.clone(),
            result: Err(reason),
        },
        PluginOp::Install { .. }
        | PluginOp::Update { .. }
        | PluginOp::Uninstall { .. }
        | PluginOp::AddMarketplace { .. }
        | PluginOp::UpdateMarketplace { .. }
        | PluginOp::RemoveMarketplace { .. }
        | PluginOp::RestoreDefault { .. }
        | PluginOp::Cancel => PluginEvent::JobFinished {
            result: Err(reason),
        },
    }
}

// The store goes here. ---------------------------------------------------------

/// What this build says to every operation: the page is wired end to end,
/// the store that does the work is not in it yet.
const NOT_CONNECTED: &str = "plugins are not connected in this build yet";

/// Run one operation and return the event that closes it; anything along
/// the way (fetch progress, a job's steps) goes out through `ctl`. A refresh
/// closes on the catalog it read; a detail on the detail; a fetch on its
/// finish; a toggle on its result; everything else on the job's result.
///
/// This is the seam the plugin store fills: each arm becomes a call to
/// `alter_zero::plugins::store::PluginStore` with `ctl` adapted to its
/// `Control` trait.
pub(crate) fn run_plugin_op(op: &PluginOp, ctl: &PluginWorkerControl) -> PluginEvent {
    let _ = ctl.cancelled();
    match op {
        PluginOp::Refresh { .. } => PluginEvent::Catalog(PluginCatalog::default()),
        PluginOp::FetchMarketplace { marketplace } => {
            ctl.fetch_progress(marketplace, None, None);
            closing_failure(op, NOT_CONNECTED)
        }
        PluginOp::LoadDetail { .. } | PluginOp::SetEnabled { .. } | PluginOp::Cancel => {
            closing_failure(op, NOT_CONNECTED)
        }
        _ => {
            ctl.progress(None, None);
            closing_failure(op, NOT_CONNECTED)
        }
    }
}

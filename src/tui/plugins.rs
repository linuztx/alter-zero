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
//! The work is [`run_plugin_op`] over the plugin store, behind a
//! [`PluginWorkerControl`] that is the store's `Control`: everything it
//! reports — a fetch's progress, a job's steps, the result that closes the
//! operation — goes out through the control as [`PluginEvent`]s. What can
//! be decided without the disk — which marketplaces a refresh fetches, the
//! order of its events, the sentences a result is stated in — is in pure
//! functions beside it, tested here.
//!
//! [`App::open_plugins_menu`]: alter_zero::app::App::open_plugins_menu
//! [`App::apply_plugin_event`]: alter_zero::app::App::apply_plugin_event

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use std::cell::Cell;

use alter_zero::app::{JobDone, JobGoto, PluginEvent, PluginOp};
use alter_zero::plugins::{
    Control, GitControl, GitError, GitProgress, InstallOutcome, KnownMarketplace,
    LOCAL_MARKETPLACE, MarketplaceOutcome, MarketplaceSource, PluginId, PluginStore, git_available,
    version_label,
};
use alter_zero::plugins::{PluginDetail, PluginStatus};
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
        let tx = self
            .plugins_channel
            .get_or_insert_with(tokio::sync::mpsc::unbounded_channel)
            .0
            .clone();
        spawn_plugin_op(round, op, cancel, tx);
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
                | PluginEvent::CancelledJobEnded { changed: true }
        );
        if matches!(event, PluginEvent::JobFinished { .. }) {
            self.plugins_job = None;
        }
        let review = match &event {
            PluginEvent::Detail {
                id,
                result: Ok(detail),
            } => Some((id.clone(), self.plugin_review(detail))),
            _ => None,
        };
        self.app.apply_plugin_event(round, event);
        if let Some((id, (hooks, servers, next_launch))) = review {
            self.app.set_plugin_review(&id, hooks, servers, next_launch);
        }
        if changed {
            self.plugins_changed();
        }
        self.frame.schedule_frame();
    }

    /// How many of an enabled plugin's hooks and MCP servers wait for the
    /// user — `(hooks, servers, next_launch)`. For a plugin this session
    /// loaded, its hooks not trusted and its servers not enabled, read off
    /// the review's own record; for one installed or enabled since launch,
    /// every hook and server it carries, since the startup review asks
    /// about them at the next launch.
    fn plugin_review(&self, detail: &PluginDetail) -> (usize, usize, bool) {
        if detail.status != PluginStatus::Enabled {
            return (0, 0, false);
        }
        let loaded = self
            .trust
            .plugins
            .iter()
            .flat_map(|layer| layer.files.iter())
            .find(|files| files.id == detail.id);
        let Some(files) = loaded else {
            return detail.components.as_ref().map_or((0, 0, false), |c| {
                (c.hooks.len(), c.mcp_servers.len(), true)
            });
        };
        let hooks = files
            .hooks
            .as_ref()
            .and_then(|file| {
                let source = file.path.display().to_string();
                let parsed = file.parsed.as_ref().ok()?;
                Some(
                    alter_zero::trust::runnable_hooks(parsed)
                        .iter()
                        .filter(|hook| !self.trust.is_hook_trusted(&source, hook))
                        .count(),
                )
            })
            .unwrap_or(0);
        let servers = files.mcp.as_ref().map_or(0, |file| {
            let source = file.path.display().to_string();
            self.trust
                .server_reviews()
                .iter()
                .filter(|review| review.source == source && !review.trust.approved())
                .count()
        });
        (hooks, servers, false)
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
    /// queue for the startup review at the next launch). Called once per
    /// operation that worked — [`Session::reload_plugins`].
    pub(crate) fn plugins_changed(&mut self) {
        self.reload_plugins();
    }
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
/// that stops it, and the channel it reports on — and, as the store's
/// [`Control`], where git's progress and the store's steps go: onto the
/// fetch under way when there is one, else onto the page's job.
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
    live: Mutex<Live>,
}

/// What a control knows of the work under way.
#[derive(Default)]
struct Live {
    /// The marketplace a fetch has been started for and not yet finished.
    fetch: Option<String>,
    /// The job's step, kept so git's percentages can ride beside it.
    step: Option<String>,
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
            live: Mutex::new(Live::default()),
        }
    }

    /// Whether the page stopped this operation (Esc, or the page closed).
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send one event for this round — nothing once cancelled. A fetch's
    /// start and finish also say where progress goes meanwhile.
    pub(crate) fn send(&self, event: PluginEvent) {
        match &event {
            PluginEvent::FetchStarted { marketplace } => {
                self.live().fetch = Some(marketplace.clone());
            }
            PluginEvent::FetchFinished { marketplace, .. } => {
                let mut live = self.live();
                if live.fetch.as_deref() == Some(marketplace) {
                    live.fetch = None;
                }
            }
            _ => {}
        }
        if self.cancelled() {
            return;
        }
        let _ = self.tx.send((self.round, event));
    }

    /// Send one event even though the operation was cancelled — what a
    /// cancelled job reads of the disk once it has ended, so the page shows
    /// what is true.
    fn send_settled(&self, event: PluginEvent) {
        let _ = self.tx.send((self.round, event));
    }

    /// Progress for whatever is under way: `phase` is git's (`Receiving
    /// objects`) or the store's step (`copying files`).
    fn report(&self, phase: Option<String>, percent: Option<u8>, is_step: bool) {
        let (fetch, step) = {
            let mut live = self.live();
            if is_step && live.fetch.is_none() {
                live.step.clone_from(&phase);
            }
            (live.fetch.clone(), live.step.clone())
        };
        match fetch {
            Some(marketplace) => self.send(PluginEvent::FetchProgress {
                marketplace,
                phase,
                percent,
            }),
            None => self.send(PluginEvent::JobProgress { step, percent }),
        }
    }
}

impl GitControl for PluginWorkerControl {
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn progress(&self, progress: GitProgress) {
        self.report(Some(progress.phase), progress.percent, false);
    }
}

impl Control for PluginWorkerControl {
    fn step(&self, text: &str) {
        self.report(Some(text.to_string()), None, true);
    }
}

impl Drop for PluginWorkerControl {
    fn drop(&mut self) {
        if !std::thread::panicking() || self.cancelled() {
            return;
        }
        let open = self.live().fetch.take();
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

/// What every operation answers with no config home to keep plugins in.
pub(crate) const NO_CONFIG_HOME: &str = "no config directory — set HOME or ALTER_ZERO_CONFIG_DIR";

/// How long a fetched git marketplace stays fresh: a page opened after this
/// fetches it again (`docs/plugins.md`).
pub(crate) const STALE_AFTER_SECS: u64 = 86_400;

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

// --- what a refresh fetches -----------------------------------------------------

/// How a marketplace is brought in: fetched for the first time, or updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchKind {
    Ensure,
    Update,
}

/// Whether a refresh fetches `known` at `now` (Unix seconds): one never
/// fetched always; a fetched repository once it is a day old, or when the
/// user asked (`force`); a folder only when asked, since reading it is all
/// an update does and the page reads it at every open anyway.
pub(crate) fn needs_fetch(known: &KnownMarketplace, now: u64, force: bool) -> Option<FetchKind> {
    if !known.fetched {
        return Some(FetchKind::Ensure);
    }
    let is_git = matches!(known.source, MarketplaceSource::Git { .. });
    let stale = known
        .updated
        .is_none_or(|at| now.saturating_sub(at) > STALE_AFTER_SECS);
    (force || (is_git && stale)).then_some(FetchKind::Update)
}

/// One marketplace a refresh fetches: its place in the known list, the
/// name the page shows it by (its manifest's once fetched, a default's
/// expected name or its source before), how, and whether that takes git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFetch {
    pub(crate) index: usize,
    pub(crate) shown: String,
    pub(crate) kind: FetchKind,
    pub(crate) needs_git: bool,
}

/// What a refresh fetches, in the known list's order.
pub(crate) fn refresh_plan(
    known: &[KnownMarketplace],
    now: u64,
    force: bool,
    home: Option<&Path>,
) -> Vec<PlannedFetch> {
    known
        .iter()
        .enumerate()
        .filter_map(|(index, market)| {
            let kind = needs_fetch(market, now, force)?;
            Some(PlannedFetch {
                index,
                shown: shown_name(market, home),
                kind,
                needs_git: matches!(market.source, MarketplaceSource::Git { .. }),
            })
        })
        .collect()
}

/// The name the catalog lists a known marketplace by.
fn shown_name(market: &KnownMarketplace, home: Option<&Path>) -> String {
    market
        .name
        .clone()
        .unwrap_or_else(|| market.source.display(home))
}

/// Run a refresh's fetches one after another — two clones at once buy
/// little and double the peak — each a fetch on the page from its start to
/// its finish, a fresh snapshot of the catalog after each that worked so
/// rows appear marketplace by marketplace, and nothing more once cancelled.
/// A repository with no `git` to fetch it fails with git's own sentence.
pub(crate) fn run_refresh(
    plan: &[PlannedFetch],
    git: bool,
    fetch: &mut dyn FnMut(&PlannedFetch) -> Result<(), String>,
    snapshot: &mut dyn FnMut() -> Vec<PluginEvent>,
    emit: &mut dyn FnMut(PluginEvent),
    cancelled: &dyn Fn() -> bool,
) {
    for planned in plan {
        if cancelled() {
            return;
        }
        emit(PluginEvent::FetchStarted {
            marketplace: planned.shown.clone(),
        });
        let result = if planned.needs_git && !git {
            Err(GitError::Missing.to_string())
        } else {
            fetch(planned)
        };
        let worked = result.is_ok();
        emit(PluginEvent::FetchFinished {
            marketplace: planned.shown.clone(),
            result,
        });
        if worked {
            for event in snapshot() {
                emit(event);
            }
        }
    }
}

// --- what the page says when a job is done ---------------------------------------

/// The dim row under a disable or an uninstall of a plugin that runs code.
const RUNS_CODE_NOTE: &str = "Its hooks and MCP servers stop at the next launch.";

fn done(text: String, detail: Option<String>, goto: Option<JobGoto>) -> JobDone {
    JobDone { text, detail, goto }
}

/// `Installed {name} — 2 skills … are ready for your next message.`, over
/// what waits for the startup review.
pub(crate) fn installed_done(name: &str, outcome: &InstallOutcome) -> JobDone {
    let text = match outcome.live_summary() {
        Some(live) => format!("Installed {name} — {live}"),
        None => format!("Installed {name}"),
    };
    done(text, outcome.review_summary(), None)
}

/// `Updated {name} to v{x}`, or that it already was. A folder's plugin has
/// no revision to compare, so it is always copied again; at the version it
/// had (`before`), it says so rather than claim an update.
pub(crate) fn updated_done(name: &str, before: Option<&str>, outcome: &InstallOutcome) -> JobDone {
    let text = match (outcome.changed, outcome.version.as_deref()) {
        (false, _) => format!("{name} is already up to date"),
        (true, Some(version)) if before == Some(version) => {
            format!(
                "{name} is at {} — copied again from its marketplace",
                version_label(version)
            )
        }
        (true, Some(version)) => format!("Updated {name} to {}", version_label(version)),
        (true, None) => format!("Updated {name}"),
    };
    done(text, None, None)
}

/// `Uninstalled {name}`, back on Browse.
pub(crate) fn uninstalled_done(name: &str, runs_code: bool) -> JobDone {
    done(
        format!("Uninstalled {name}"),
        runs_code.then(|| RUNS_CODE_NOTE.to_string()),
        Some(JobGoto::Browse),
    )
}

/// What turning a plugin on or off does, and when.
pub(crate) fn toggled_done(name: &str, enabled: bool, runs_code: bool) -> JobDone {
    if enabled {
        done(
            format!("Enabled {name} — it loads from your next message"),
            None,
            None,
        )
    } else {
        done(
            format!("Disabled {name} — it stops loading from your next message"),
            runs_code.then(|| RUNS_CODE_NOTE.to_string()),
            None,
        )
    }
}

/// `{n} plugin(s)`.
fn plugins_count(n: usize) -> String {
    format!("{n} plugin{}", if n == 1 { "" } else { "s" })
}

/// `Added {name} — {n} plugins. Pick one and press Enter.`, on its tab.
pub(crate) fn added_done(outcome: &MarketplaceOutcome) -> JobDone {
    let name = &outcome.name;
    let count = plugins_count(outcome.plugins);
    let text = if outcome.changed {
        format!("Added {name} — {count}. Pick one and press Enter.")
    } else {
        format!("{name} is already added — {count}")
    };
    done(text, None, Some(JobGoto::MarketplaceTab(name.clone())))
}

/// `Updated {name} — {n} plugins`, or that it was up to date.
pub(crate) fn market_updated_done(outcome: &MarketplaceOutcome) -> JobDone {
    let text = if outcome.changed {
        format!(
            "Updated {} — {}",
            outcome.name,
            plugins_count(outcome.plugins)
        )
    } else {
        format!("{} is up to date", outcome.name)
    };
    done(text, None, None)
}

/// `Removed {name}`, and what of it stays.
pub(crate) fn removed_done(name: &str, kept: usize) -> JobDone {
    let detail = match kept {
        0 => None,
        1 => Some("1 installed plugin from it stays.".to_string()),
        n => Some(format!("{n} installed plugins from it stay.")),
    };
    done(format!("Removed {name}"), detail, Some(JobGoto::Browse))
}

/// `Restored {name} — {n} plugins`.
pub(crate) fn restored_done(name: &str, plugins: usize) -> JobDone {
    done(
        format!("Restored {name} — {}", plugins_count(plugins)),
        None,
        None,
    )
}

// --- the work ------------------------------------------------------------------------

/// Run one operation against the plugin store, reporting everything —
/// fetches, steps, the result that closes it — through `ctl`. Every worker
/// builds its own store, holds the catalog only as long as it takes to send
/// it, and after any change on disk sends a fresh catalog (and a fresh
/// detail of the plugin it touched) ahead of its result.
pub(crate) fn run_plugin_op(op: &PluginOp, ctl: &PluginWorkerControl) {
    let Some(store) = config::plugin_store(false) else {
        ctl.send(closing_failure(op, NO_CONFIG_HOME));
        return;
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let work = Work {
        store: &store,
        ctl,
        home: home.as_deref(),
        changed: Cell::new(false),
    };
    work.run(op);
    if is_job(op) && ctl.cancelled() {
        work.settle_cancelled(op);
    }
}

/// The plugin a job is about, when it is about one.
fn job_subject(op: &PluginOp) -> Option<&PluginId> {
    match op {
        PluginOp::Install { id } | PluginOp::Update { id } | PluginOp::Uninstall { id } => Some(id),
        _ => None,
    }
}

/// One operation's working set.
struct Work<'a> {
    store: &'a PluginStore,
    ctl: &'a PluginWorkerControl,
    home: Option<&'a Path>,
    /// Whether it changed anything on disk.
    changed: Cell<bool>,
}

impl Work<'_> {
    fn run(&self, op: &PluginOp) {
        let ctl = self.ctl;
        let store = self.store;
        match op {
            PluginOp::Cancel => {}
            PluginOp::Refresh { force } => self.refresh(*force),
            PluginOp::LoadDetail { id } => ctl.send(PluginEvent::Detail {
                id: id.clone(),
                result: store.detail(id).map(Box::new),
            }),
            PluginOp::Install { id } => {
                let result = store.install(id, ctl).map(|outcome| {
                    self.changed(Some(id));
                    installed_done(&outcome.id.name, &outcome)
                });
                self.finish(result);
            }
            PluginOp::Update { id } => {
                let before = store
                    .state()
                    .ok()
                    .and_then(|file| file.plugin(id).and_then(|record| record.version.clone()));
                let result = (|| {
                    if id.marketplace != LOCAL_MARKETPLACE {
                        store.update_marketplace(&id.marketplace, ctl)?;
                    }
                    store.update_plugin(id, ctl)
                })()
                .map(|outcome| {
                    self.changed(Some(id));
                    updated_done(&id.name, before.as_deref(), &outcome)
                });
                self.finish(result);
            }
            PluginOp::Uninstall { id } => {
                let runs_code = self.runs_code(id);
                let result = store.uninstall(id).map(|()| {
                    self.changed(None);
                    uninstalled_done(&id.name, runs_code)
                });
                self.finish(result);
            }
            PluginOp::SetEnabled { id, enabled } => {
                let runs_code = self.runs_code(id);
                let result = store.set_enabled(id, *enabled).map(|()| {
                    self.changed(Some(id));
                    toggled_done(&id.name, *enabled, runs_code)
                });
                ctl.send(PluginEvent::EnabledSet {
                    id: id.clone(),
                    enabled: *enabled,
                    result,
                });
            }
            PluginOp::AddMarketplace { source } => {
                let result = self.parse_source(source).and_then(|source| {
                    let outcome = store.add_marketplace(&source, ctl)?;
                    self.changed(None);
                    Ok(added_done(&outcome))
                });
                self.finish(result);
            }
            PluginOp::FetchMarketplace { marketplace } => {
                ctl.send(PluginEvent::FetchStarted {
                    marketplace: marketplace.clone(),
                });
                let result = self.fetch_by_name(marketplace);
                let worked = result.is_ok();
                ctl.send(PluginEvent::FetchFinished {
                    marketplace: marketplace.clone(),
                    result,
                });
                if worked {
                    self.changed(None);
                }
            }
            PluginOp::UpdateMarketplace { name } => {
                let result = store.update_marketplace(name, ctl).map(|outcome| {
                    self.changed(None);
                    market_updated_done(&outcome)
                });
                self.finish(result);
            }
            PluginOp::RemoveMarketplace { name } => {
                let kept = store.state().map_or(0, |file| {
                    file.plugins
                        .keys()
                        .filter_map(|key| PluginId::parse(key).ok())
                        .filter(|id| &id.marketplace == name)
                        .count()
                });
                let result = store.remove_marketplace(name).map(|()| {
                    self.changed(None);
                    removed_done(name, kept)
                });
                self.finish(result);
            }
            PluginOp::RestoreDefault { source } => {
                let result = self.restore(source);
                self.finish(result);
            }
        }
    }

    /// Send a job's result.
    fn finish(&self, result: Result<JobDone, String>) {
        self.ctl.send(PluginEvent::JobFinished { result });
    }

    /// The catalog and the removed defaults as they are on disk now, as the
    /// events that carry them to the page.
    fn snapshot(&self) -> Vec<PluginEvent> {
        let mut events = vec![match self.store.catalog() {
            Ok(catalog) => PluginEvent::Catalog(catalog),
            Err(reason) => PluginEvent::CatalogFailed(reason),
        }];
        if let Ok(file) = self.store.state() {
            events.push(PluginEvent::RemovedDefaults(file.removed_defaults));
        }
        events
    }

    /// Something changed on disk: the page gets the catalog again, and the
    /// plugin it touched (`subject`) its detail.
    fn changed(&self, subject: Option<&PluginId>) {
        self.changed.set(true);
        for event in self.snapshot() {
            self.ctl.send(event);
        }
        if let Some(id) = subject {
            self.ctl.send(PluginEvent::Detail {
                id: id.clone(),
                result: self.store.detail(id).map(Box::new),
            });
        }
    }

    /// A job the user cancelled has ended — its store call returned, whether
    /// it stopped in time or had already done its work. Every event it sent
    /// since the cancel was dropped, so the page gets the catalog (and the
    /// plugin's detail) as they are on disk now, then whether anything
    /// changed, which also reloads the session's plugins.
    fn settle_cancelled(&self, op: &PluginOp) {
        for event in self.snapshot() {
            self.ctl.send_settled(event);
        }
        if let Some(id) = job_subject(op) {
            self.ctl.send_settled(PluginEvent::Detail {
                id: id.clone(),
                result: self.store.detail(id).map(Box::new),
            });
        }
        self.ctl.send_settled(PluginEvent::CancelledJobEnded {
            changed: self.changed.get(),
        });
    }

    /// Whether an installed plugin carries hooks or MCP servers — what its
    /// disable or uninstall stops at the next launch.
    fn runs_code(&self, id: &PluginId) -> bool {
        self.store
            .state()
            .ok()
            .and_then(|file| file.plugin(id).map(|record| record.hooks || record.mcp))
            .unwrap_or(false)
    }

    /// The page as it is on disk at once, then every marketplace that needs
    /// it fetched, one at a time.
    fn refresh(&self, force: bool) {
        let first = self.snapshot();
        let failed = matches!(first.first(), Some(PluginEvent::CatalogFailed(_)));
        for event in first {
            self.ctl.send(event);
        }
        if failed {
            return;
        }
        let Ok(known) = self.store.known_marketplaces() else {
            return;
        };
        let plan = refresh_plan(&known, unix_now(), force, self.home);
        run_refresh(
            &plan,
            git_available(),
            &mut |planned| match planned.kind {
                FetchKind::Ensure => self
                    .store
                    .ensure_marketplace(&known[planned.index], self.ctl)
                    .map(drop),
                FetchKind::Update => self
                    .store
                    .update_marketplace(&planned.shown, self.ctl)
                    .map(drop),
            },
            &mut || self.snapshot(),
            &mut |event| self.ctl.send(event),
            &|| self.ctl.cancelled(),
        );
    }

    /// Fetch the known marketplace the page names `name`.
    fn fetch_by_name(&self, name: &str) -> Result<(), String> {
        let known = self.store.known_marketplaces()?;
        let market = known
            .iter()
            .find(|market| shown_name(market, self.home) == name)
            .ok_or_else(|| format!("there is no marketplace named {name}"))?;
        if matches!(market.source, MarketplaceSource::Git { .. }) && !git_available() {
            return Err(GitError::Missing.to_string());
        }
        self.store.ensure_marketplace(market, self.ctl).map(drop)
    }

    /// What the Add field holds, as a source — a folder that must be there.
    fn parse_source(&self, text: &str) -> Result<MarketplaceSource, String> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let source = MarketplaceSource::parse(text.trim(), &cwd, self.home)?;
        if let MarketplaceSource::Local { path } = &source {
            if !path.exists() {
                return Err(format!("{} does not exist", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("{} is not a folder", path.display()));
            }
        }
        Ok(source)
    }

    /// Put a removed default back and fetch it.
    fn restore(&self, source: &str) -> Result<JobDone, String> {
        self.store.restore_default(source)?;
        let known = self.store.known_marketplaces()?;
        let fetched = known
            .iter()
            .find(|market| market.source.canonical() == source)
            .map(|market| self.store.ensure_marketplace(market, self.ctl))
            .transpose();
        let catalog = self.store.catalog();
        let name = match fetched {
            Ok(Some(name)) => name,
            Ok(None) => source.to_string(),
            Err(reason) => {
                // Restored, though not fetched: the page shows it unfetched.
                self.changed(None);
                return Err(reason);
            }
        };
        let plugins = catalog.as_ref().map_or(0, |catalog| {
            catalog
                .marketplaces
                .iter()
                .position(|market| market.name == name)
                .map_or(0, |index| catalog.count_in(index))
        });
        drop(catalog);
        self.changed(None);
        Ok(restored_done(&name, plugins))
    }
}

/// The wall clock, in Unix seconds — what a marketplace's age is measured
/// against.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alter_zero::plugins::Components;

    fn known(
        name: Option<&str>,
        source: MarketplaceSource,
        fetched: bool,
        updated: Option<u64>,
    ) -> KnownMarketplace {
        KnownMarketplace {
            name: name.map(str::to_string),
            source,
            reference: None,
            revision: None,
            updated,
            is_default: true,
            fetched,
        }
    }

    fn git(url: &str) -> MarketplaceSource {
        MarketplaceSource::Git {
            url: url.to_string(),
            reference: None,
        }
    }

    fn folder(path: &str) -> MarketplaceSource {
        MarketplaceSource::Local {
            path: PathBuf::from(path),
        }
    }

    const NOW: u64 = 1_763_650_000;

    #[test]
    fn a_refresh_fetches_the_unfetched_the_stale_and_on_force_everything() {
        let never = known(
            Some("official"),
            git("https://github.com/a/b.git"),
            false,
            None,
        );
        let fresh = known(
            Some("fresh"),
            git("https://github.com/c/d.git"),
            true,
            Some(NOW - 60),
        );
        let old = known(
            Some("old"),
            git("https://github.com/e/f.git"),
            true,
            Some(NOW - STALE_AFTER_SECS - 1),
        );
        let local = known(
            Some("team"),
            folder("/work/team"),
            true,
            Some(NOW - 10 * STALE_AFTER_SECS),
        );
        assert_eq!(needs_fetch(&never, NOW, false), Some(FetchKind::Ensure));
        assert_eq!(needs_fetch(&fresh, NOW, false), None);
        assert_eq!(needs_fetch(&old, NOW, false), Some(FetchKind::Update));
        assert_eq!(
            needs_fetch(&local, NOW, false),
            None,
            "a folder only on force"
        );
        assert_eq!(needs_fetch(&fresh, NOW, true), Some(FetchKind::Update));
        assert_eq!(needs_fetch(&local, NOW, true), Some(FetchKind::Update));
        let unnamed = known(None, folder("/work/x"), false, None);
        let plan = refresh_plan(&[fresh, unnamed, old], NOW, false, None);
        assert_eq!(
            plan,
            vec![
                PlannedFetch {
                    index: 1,
                    shown: "/work/x".to_string(),
                    kind: FetchKind::Ensure,
                    needs_git: false,
                },
                PlannedFetch {
                    index: 2,
                    shown: "old".to_string(),
                    kind: FetchKind::Update,
                    needs_git: true,
                },
            ]
        );
    }

    fn planned(shown: &str, needs_git: bool) -> PlannedFetch {
        PlannedFetch {
            index: 0,
            shown: shown.to_string(),
            kind: FetchKind::Ensure,
            needs_git,
        }
    }

    /// The events a refresh emits for `plan`, the fetch of `failing` failing.
    fn refresh_events(plan: &[PlannedFetch], git: bool, failing: &str) -> Vec<PluginEvent> {
        let mut events = Vec::new();
        run_refresh(
            plan,
            git,
            &mut |p| {
                if p.shown == failing {
                    Err("fatal: repository not found".to_string())
                } else {
                    Ok(())
                }
            },
            &mut || vec![PluginEvent::RemovedDefaults(Vec::new())],
            &mut |event| events.push(event),
            &|| false,
        );
        events
    }

    #[test]
    fn a_refresh_fetches_one_marketplace_after_another_and_snapshots_after_each_that_worked() {
        let events = refresh_events(&[planned("a", true), planned("b", true)], true, "b");
        assert_eq!(
            events,
            vec![
                PluginEvent::FetchStarted {
                    marketplace: "a".to_string()
                },
                PluginEvent::FetchFinished {
                    marketplace: "a".to_string(),
                    result: Ok(())
                },
                PluginEvent::RemovedDefaults(Vec::new()),
                PluginEvent::FetchStarted {
                    marketplace: "b".to_string()
                },
                PluginEvent::FetchFinished {
                    marketplace: "b".to_string(),
                    result: Err("fatal: repository not found".to_string()),
                },
            ]
        );
    }

    #[test]
    fn with_no_git_a_repository_fails_with_git_s_own_sentence_and_a_folder_still_reads() {
        let events = refresh_events(&[planned("repo", true), planned("dir", false)], false, "");
        assert_eq!(
            events[1],
            PluginEvent::FetchFinished {
                marketplace: "repo".to_string(),
                result: Err(GitError::Missing.to_string()),
            }
        );
        assert_eq!(
            events[3],
            PluginEvent::FetchFinished {
                marketplace: "dir".to_string(),
                result: Ok(())
            }
        );
    }

    #[test]
    fn a_cancelled_refresh_stops_between_marketplaces() {
        let mut events = Vec::new();
        let mut fetched = 0;
        run_refresh(
            &[planned("a", false), planned("b", false)],
            true,
            &mut |_| {
                fetched += 1;
                Ok(())
            },
            &mut Vec::new,
            &mut |event| events.push(event),
            &|| true,
        );
        assert_eq!(fetched, 0);
        assert!(events.is_empty());
    }

    fn outcome(changed: bool, components: Components) -> InstallOutcome {
        InstallOutcome {
            id: PluginId::new("code-review", "acme").unwrap(),
            version: Some("1.2.0".to_string()),
            changed,
            components,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_results_say_what_happened_and_when_it_takes_effect() {
        let components = Components {
            skills: vec![
                "code-review:lint".to_string(),
                "code-review:triage".to_string(),
            ],
            commands: vec!["/code-review:review".to_string()],
            hooks: vec!["PreToolUse · Bash".to_string()],
            ..Components::default()
        };
        let installed = installed_done("code-review", &outcome(true, components.clone()));
        assert_eq!(
            installed.text,
            "Installed code-review — 2 skills and 1 command are ready for your next message."
        );
        assert_eq!(
            installed.detail.as_deref(),
            Some("1 hook waits for your review at the next launch.")
        );
        assert_eq!(
            installed_done("bare", &outcome(true, Components::default())).text,
            "Installed bare"
        );
        assert_eq!(
            updated_done(
                "code-review",
                Some("1.1.0"),
                &outcome(true, components.clone())
            )
            .text,
            "Updated code-review to v1.2.0"
        );
        assert_eq!(
            updated_done(
                "code-review",
                Some("1.2.0"),
                &outcome(true, components.clone())
            )
            .text,
            "code-review is at v1.2.0 — copied again from its marketplace"
        );
        assert_eq!(
            updated_done("code-review", Some("1.2.0"), &outcome(false, components)).text,
            "code-review is already up to date"
        );
        let gone = uninstalled_done("guard", true);
        assert_eq!(gone.text, "Uninstalled guard");
        assert_eq!(gone.detail.as_deref(), Some(RUNS_CODE_NOTE));
        assert_eq!(gone.goto, Some(JobGoto::Browse));
        assert_eq!(uninstalled_done("formatter", false).detail, None);
        let off = toggled_done("guard", false, true);
        assert_eq!(
            off.text,
            "Disabled guard — it stops loading from your next message"
        );
        assert_eq!(off.detail.as_deref(), Some(RUNS_CODE_NOTE));
        assert_eq!(
            toggled_done("guard", true, true).text,
            "Enabled guard — it loads from your next message"
        );
        assert_eq!(toggled_done("guard", true, true).detail, None);
    }

    fn market(changed: bool, plugins: usize) -> MarketplaceOutcome {
        MarketplaceOutcome {
            name: "acme".to_string(),
            plugins,
            changed,
            revision: None,
        }
    }

    #[test]
    fn the_marketplace_results_name_it_and_count_its_plugins() {
        let added = added_done(&market(true, 3));
        assert_eq!(
            added.text,
            "Added acme — 3 plugins. Pick one and press Enter."
        );
        assert_eq!(
            added.goto,
            Some(JobGoto::MarketplaceTab("acme".to_string()))
        );
        assert_eq!(
            added_done(&market(false, 1)).text,
            "acme is already added — 1 plugin"
        );
        assert_eq!(
            market_updated_done(&market(false, 3)).text,
            "acme is up to date"
        );
        assert_eq!(
            market_updated_done(&market(true, 4)).text,
            "Updated acme — 4 plugins"
        );
        assert_eq!(removed_done("acme", 0).detail, None);
        assert_eq!(
            removed_done("acme", 2).detail.as_deref(),
            Some("2 installed plugins from it stay.")
        );
        assert_eq!(removed_done("acme", 2).goto, Some(JobGoto::Browse));
        assert_eq!(
            restored_done("openai-curated", 65).text,
            "Restored openai-curated — 65 plugins"
        );
    }
}

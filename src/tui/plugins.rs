//! The `/plugins` page's boundary (`docs/plugins-menu.md` *The boundary*):
//! the store the page works on, the thread each operation runs on, the
//! token Esc stops it with, and the session reload after anything that
//! changes what is installed or on.
//!
//! The pure page (`alter_zero_plugins::page`) shows a snapshot and answers
//! keys with operations; the plain functions over the store
//! (`page::{snapshot, contents, run_op}`) do the work. What is left here is
//! what neither can do:
//!
//! - **Open** ([`Session::open_plugins_menu`]): take another process's
//!   changes into the session first ([`Session::reload_plugins`]), build the
//!   store, read the snapshot *now* — files only, no `git`, a few
//!   milliseconds — and open the page on it. The store lives while the page
//!   is open and is dropped with it.
//! - **Run** ([`Session::run_plugin_op`]): one operation at a time on a
//!   thread of its own, its progress lines and its answer — with the
//!   snapshot read after it — sent back on one channel. A worker that
//!   panics still answers. [`PluginOp::Inspect`] runs the same way (a git
//!   marketplace's preview is a `git ls-tree`) and is answered with
//!   `App::supply_plugin_contents`.
//! - **Answer** ([`Session::on_plugin_event`]): the page takes the answer
//!   while it is still its job; one it does not take is a toast only once
//!   the page has closed (a toast over the open page would move it). Then,
//!   when the session's plugins may have changed, the reload — trust
//!   forgotten under what was installed, updated or removed, and the review
//!   opened on what is new.
//! - **Stop** ([`Session::cancel_plugin_job`]): Esc on a running job, the
//!   page closing or the app quitting cancels it, which kills the git child's
//!   process group; nothing waits on the thread.
//!
//! An idle session holds the channel and nothing else: everything below is
//! reached only with the page.

use std::path::PathBuf;
use std::time::Duration;

use alter_zero::app::ToastKind;
use alter_zero::plugins::{CancelToken, PLUGINS_ENV, PluginStore, plugins_enabled};
use alter_zero_plugins::page::{
    self, PLUGINS_JOB_DELAY, PluginContents, PluginDone, PluginId, PluginOp, PluginsSnapshot,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::Session;
use super::plugin_cli::plugin_store;

/// What the page's workers report — boxed on the channel, so the channel an
/// idle session holds stays a few pointers wide.
#[derive(Debug)]
pub(crate) enum PluginEvent {
    /// A job's word on where it is (`Cloning …`).
    Step { round: u64, step: String },
    /// A job is done: what it did. (The snapshot after it is read on the
    /// loop as this lands — never on the worker, whose thread arena would
    /// keep what the read once held.)
    Done {
        round: u64,
        outcome: Result<PluginDone, String>,
        forget: Vec<PathBuf>,
        reload: bool,
    },
    /// What a plugin contains, for its page.
    Contents {
        id: PluginId,
        contents: PluginContents,
    },
}

/// The page's side of the session: its channel, and — only while the page
/// is open — the store.
pub(crate) struct PluginsBoundary {
    pub(crate) tx: UnboundedSender<Box<PluginEvent>>,
    pub(crate) rx: UnboundedReceiver<Box<PluginEvent>>,
    open: Option<Box<Open>>,
    /// The job running, by round — kept past the page's closing only until
    /// the close cancels it.
    job: Option<(u64, CancelToken)>,
}

/// What exists while the page is open.
struct Open {
    store: PluginStore,
    /// Cancels the page's content reads when it closes.
    reads: CancelToken,
}

impl PluginsBoundary {
    pub(crate) fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            tx,
            rx,
            open: None,
            job: None,
        }
    }
}

/// What every answer says when there is nowhere to keep plugins.
const NO_CONFIG_HOME: &str =
    "There is no config home to keep plugins in — set HOME or ALTER_ZERO_CONFIG_DIR";

/// What a worker that panicked answers.
const WORKER_CRASHED: &str = "The plugin operation stopped unexpectedly";

/// How often the page is redrawn while a job runs, once its line shows: its
/// elapsed counts in seconds.
const PLUGINS_JOB_TICK: Duration = Duration::from_secs(1);

/// Whether this run loads plugins (`ALTER_ZERO_PLUGINS`).
fn session_enabled() -> bool {
    plugins_enabled(std::env::var(PLUGINS_ENV).ok().as_deref())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Sends a worker's answer if it panics before it could.
struct AnswerOnPanic {
    tx: UnboundedSender<Box<PluginEvent>>,
    answer: Option<PluginEvent>,
}

impl Drop for AnswerOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(answer) = self.answer.take()
        {
            let _ = self.tx.send(Box::new(answer));
        }
    }
}

impl Session<'_> {
    /// `/plugins`: take what another process changed into the session, then
    /// open the page over a snapshot read now.
    #[cold]
    #[inline(never)]
    pub(crate) fn open_plugins_menu(&mut self) {
        self.close_plugins_page();
        self.reload_plugins(&[]);
        let enabled = session_enabled();
        let (snapshot, open) = match plugin_store(&self.cwd) {
            Some(store) => {
                let snapshot = page::snapshot(&store, &self.cwd, home().as_deref(), enabled);
                let open = Box::new(Open {
                    store,
                    reads: CancelToken::new(),
                });
                (snapshot, Some(open))
            }
            None => (
                PluginsSnapshot {
                    error: Some(NO_CONFIG_HOME.to_string()),
                    session_enabled: enabled,
                    ..PluginsSnapshot::default()
                },
                None,
            ),
        };
        self.plugins_page.open = open;
        self.app.open_plugins_menu(snapshot);
        self.frame.schedule_frame();
    }

    /// The snapshot over the open page's store, read now — `None` with no
    /// page open (or no store).
    fn read_plugins_snapshot(&self) -> Option<PluginsSnapshot> {
        self.app.plugins_menu.as_ref()?;
        let open = self.plugins_page.open.as_ref()?;
        Some(page::snapshot(
            &open.store,
            &self.cwd,
            home().as_deref(),
            session_enabled(),
        ))
    }

    /// The page closed (or is about to reopen): stop its job and its reads,
    /// and drop the store.
    pub(crate) fn close_plugins_page(&mut self) {
        self.cancel_plugin_job();
        if let Some(open) = self.plugins_page.open.take() {
            open.reads.cancel();
        }
    }

    /// Run `op`, already marked running on the page under `round` — or, for
    /// [`PluginOp::Inspect`] (round `0`), read the plugin's contents.
    #[cold]
    #[inline(never)]
    pub(crate) fn run_plugin_op(&mut self, op: PluginOp, round: u64) {
        let tx = self.plugins_page.tx.clone();
        let Some(open) = self.plugins_page.open.as_ref() else {
            let error = NO_CONFIG_HOME.to_string();
            match op {
                PluginOp::Inspect(id) => {
                    self.app
                        .supply_plugin_contents(&id, PluginContents::Unreadable(error));
                }
                _ => {
                    self.app.finish_plugin_job(round, None, &Err(error));
                }
            }
            return;
        };
        let store = open.store.clone();
        let project = self.cwd.clone();
        if let PluginOp::Inspect(id) = op {
            let cancel = open.reads.clone();
            std::thread::spawn(move || {
                let _answer = AnswerOnPanic {
                    tx: tx.clone(),
                    answer: Some(PluginEvent::Contents {
                        id: id.clone(),
                        contents: PluginContents::Unreadable(WORKER_CRASHED.to_string()),
                    }),
                };
                let contents = page::contents(&store, &id, &project, &cancel);
                if !cancel.is_cancelled() {
                    let _ = tx.send(Box::new(PluginEvent::Contents { id, contents }));
                }
            });
            return;
        }
        self.cancel_plugin_job();
        let cancel = CancelToken::new();
        self.plugins_page.job = Some((round, cancel.clone()));
        std::thread::spawn(move || {
            // Read before anything runs: a worker that dies part way may
            // already have replaced files, and what it could have touched
            // is forgotten then — over-forgetting is the safe direction.
            let at_risk = page::may_replace(&store, &op);
            let _answer = AnswerOnPanic {
                tx: tx.clone(),
                answer: Some(PluginEvent::Done {
                    round,
                    outcome: Err(WORKER_CRASHED.to_string()),
                    forget: at_risk,
                    // It may have changed something before it stopped.
                    reload: true,
                }),
            };
            let steps = tx.clone();
            let mut progress = |step: &str| {
                let _ = steps.send(Box::new(PluginEvent::Step {
                    round,
                    step: step.to_string(),
                }));
            };
            let result = page::run_op(&store, &project, &op, &mut progress, &cancel);
            let _ = tx.send(Box::new(PluginEvent::Done {
                round,
                outcome: result.outcome,
                forget: result.forget,
                reload: result.reload,
            }));
        });
        self.frame.schedule_frame();
    }

    /// Stop the job running — Esc on it, the page closing, the app quitting.
    /// A clone or a fetch is killed; the worker still answers.
    pub(crate) fn cancel_plugin_job(&mut self) {
        if let Some((_, cancel)) = self.plugins_page.job.take() {
            cancel.cancel();
        }
    }

    /// A worker reported.
    #[cold]
    #[inline(never)]
    pub(crate) fn on_plugin_event(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Step { round, step } => {
                self.app.report_plugin_step(round, step);
            }
            PluginEvent::Contents { id, contents } => {
                self.app.supply_plugin_contents(&id, contents);
            }
            PluginEvent::Done {
                round,
                outcome,
                forget,
                reload,
            } => {
                if self
                    .plugins_page
                    .job
                    .as_ref()
                    .is_some_and(|(job, _)| *job == round)
                {
                    self.plugins_page.job = None;
                }
                // What the operation left on disk, cancelled or not — read
                // here, on the loop, and only for a page still open.
                let snapshot = self.read_plugins_snapshot();
                if self.app.plugins_job_round() == Some(round) {
                    self.app.finish_plugin_job(round, snapshot, &outcome);
                } else if let Some(snapshot) = snapshot {
                    // Esc let go of it and it ran to its end anyway: the
                    // rows show what is on disk now — never a toast over the
                    // page.
                    self.app.update_plugins_snapshot(snapshot);
                } else {
                    match &outcome {
                        Ok(done) if !done.message.is_empty() => {
                            self.toast(done.message.clone(), ToastKind::Info);
                        }
                        Err(error) => self.toast(error.clone(), ToastKind::Error),
                        Ok(_) => {}
                    }
                }
                if reload {
                    self.reload_plugins(&forget);
                }
                self.inspect_pending_plugin();
            }
        }
        self.frame.schedule_frame();
    }

    /// The open plugin page whose contents a job dropped (an install, an
    /// update): read them again.
    fn inspect_pending_plugin(&mut self) {
        if let Some(id) = self.app.pending_plugin_inspect() {
            if let Some(page) = self.app.plugins_menu.as_mut() {
                page.inspecting = Some(id.clone());
            }
            self.run_plugin_op(PluginOp::Inspect(id), 0);
        }
    }

    /// The page's frames while a job runs, every draw: one the moment the
    /// job has run long enough to be said ([`PLUGINS_JOB_DELAY`]), then one
    /// a second so its count moves with no other event. Nothing while the
    /// page is closed, no job runs, or an overlay covers it.
    pub(crate) fn tick_plugins_page(&mut self) {
        if !self.app.plugins_menu_ticks() {
            return;
        }
        let elapsed = self
            .app
            .plugins_menu
            .as_ref()
            .map_or(Duration::ZERO, |page| page.job_elapsed());
        let next = PLUGINS_JOB_DELAY
            .checked_sub(elapsed)
            .filter(|wait| !wait.is_zero())
            .unwrap_or(PLUGINS_JOB_TICK);
        self.frame.schedule_frame_in(next);
    }
}

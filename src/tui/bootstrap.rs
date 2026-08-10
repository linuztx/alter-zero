//! Assembling a [`Session`] before the first frame, and taking it apart at the
//! end.
//!
//! [`Session::bootstrap`] is one long sequence on purpose: the order is part of
//! the behaviour. The permission gate is seeded before the backend is built (so
//! the backend attaches a gate that already knows this project's rules), the
//! banner is committed before the startup directive's transcript, and the
//! `/resume` picker opens last so it covers a screen that already has its first
//! frame scheduled.
//!
//! Two things it deliberately does **not** do: create the `EventStream` before
//! the viewport's cursor query (invariant 1 — `main.rs` has already run `init`,
//! and this is still the only reader), and fail. Every store here degrades
//! instead: no config home means no persistence, a checkpoint store that won't
//! initialize yields no checkpoints, a session that can't be recorded is simply
//! not recorded. The TUI always starts.
//!
//! [`Session::shutdown`] is the mirror image, and its rule is *bounded*: stop
//! the backend without joining it (the interrupt-lag freeze), give a `!` shell
//! runner a short window to reap its child so a reparented process can't outlive
//! the TUI, kill every background shell and subagent, and hand back the session
//! id for the exit hint.

use std::collections::HashMap;
use std::io;

use ratatui::crossterm::event::EventStream;
use ratatui::text::Line;

use alter_zero::agents::AgentRegistry;
use alter_zero::app::{App, CHECKPOINT_RESTORED_NOTICE, ToastKind};
use alter_zero::background::{self, BackgroundRegistry, BgEvent};
use alter_zero::checkpoint::{self, CheckpointRefusal, CheckpointStore};
use alter_zero::frame;
use alter_zero::paste::PasteBurst;
use alter_zero::project_doc;
use alter_zero::session;
use alter_zero::stream::{CancelToken, StreamEvent};
use alter_zero::term::InlineViewport;
use alter_zero::ui;

use super::history_store::InputHistoryStore;
use super::models::{HookSetup, ModelSession};
use super::permission::PermissionStore;
use super::recorder::SessionRecorder;
use super::shell::{SHELL_POLL_INTERVAL, SHELL_QUIT_KILL_WINDOW};
use super::startup::{LoadedSession, Startup};
use super::view::RESIZE_REFLOW_MAX_ROWS;
use super::workers::{ModelFetch, spawn_file_search_worker, spawn_model_fetch};
use super::{Session, StatusClocks, config, host};

impl<'t> Session<'t> {
    /// Build the session — both ends of every channel included — then paint the
    /// first frame.
    ///
    /// `startup` is the CLI's `--continue`/`--resume` directive (`docs/cli.md`),
    /// applied before that frame: a `Load` restores the code state, installs the
    /// transcript and adopts the file for further recording; a `Picker` boots
    /// straight into the `/resume` overlay.
    pub(crate) fn bootstrap(
        term: &'t mut InlineViewport,
        startup: Option<Startup>,
    ) -> io::Result<Self> {
        // Backend → loop (the streamed reply). A tokio channel so the loop can
        // `select!` on it; the backend thread sends without touching the runtime.
        let (tx, reply_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        // The frame requester + its scheduler task, and the scheduler's draw-tick
        // channel (scheduler → loop).
        let (frame, frame_rx) = frame::channel();
        let (draw_tx, draw_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        tokio::spawn(frame::run_scheduler(frame_rx, draw_tx));

        let mut app = App::new();
        // Inject the wall-clock here at the I/O boundary so the pure library never
        // sees a clock. Recorded items are stamped with the local time; the stamp
        // is shown only in the Ctrl+O transcript (see docs/timestamps.md).
        app.set_clock(host::local_timestamp);
        // The persistent input history (docs/history-persistence.md): load the
        // JSONL file and seed `App::input_history` before the first paint, so ↑/↓
        // recall and Ctrl+R search span past sessions. New submissions are
        // appended per loop iteration (`take_unpersisted_inputs`).
        let hist_store = InputHistoryStore::new();
        app.seed_input_history(hist_store.load());

        let cwd = std::env::current_dir().unwrap_or_default();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);

        // The background-shell registry (docs/background.md): processes launched
        // by the model's `run_in_background` bash calls (or moved back with
        // Ctrl+B) report on their own channel — a dedicated `select!` source,
        // because they outlive turns and the reply channel is swapped on every
        // interrupt/`/clear`. Interim output tees to per-task files under the temp
        // dir so the model can `read` progress mid-run.
        let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel::<BgEvent>();
        // Claude Code's tasks layout: a stable per-user root, the cwd as one
        // dashed segment, and a per-session dir —
        // `{tmp}/alter-zero-{uid}/-home-user-proj/{session}/tasks/{id}.output`
        // (the pure `background::tasks_dir`; the uid/cwd/session injected here at
        // the boundary).
        //
        // Every shell child (model `bash`, `run_in_background`, the `!` shell)
        // spawns detached from the controlling terminal (`subprocess::tiers` — the
        // `setsid` binary, else our own binary re-execed in the mode `main`
        // installed), so a `sudo` password prompt errors at once instead of
        // writing over the TUI. The helper path is resolved ONCE here — it stays
        // valid even if a `cargo build` replaces the file mid-session (and the
        // setsid tier doesn't need it at all); a failed `current_exe` (None) just
        // shortens the chain. The registry carries it to all three spawn sites.
        let registry = BackgroundRegistry::new(
            bg_tx,
            background::tasks_dir(
                &std::env::temp_dir(),
                host::process_uid(),
                &cwd,
                &host::session_id(),
            ),
        )
        .with_detach_helper(std::env::current_exe().ok());

        // The subagent registry (docs/agent-tool.md): the model's `agent` tool
        // launches run their own loops on their own threads, reporting on a
        // dedicated channel — because agents outlive turns exactly like
        // background shells.
        let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent_registry = AgentRegistry::new(agent_tx);

        // The tool-permission gate (docs/permissions.md), seeded with this
        // project's saved rules BEFORE the backend is built below — every
        // `write`/`edit`/`bash` call, the main turn's and its subagents', raises
        // the inline prompt and blocks its own thread on the answer.
        let permissions = PermissionStore::open(&cwd);

        // The ask gate (docs/ask.md): the `AskUserQuestion` modal's answers
        // post here, waking the blocked tool thread. Always built — asking is
        // not a permission, so it doesn't follow ALTER_ZERO_PERMISSIONS.
        let ask = alter_zero::ask::AskGate::new();

        // The shared task list (docs/task-tools.md): the executor mutates it
        // on the tool thread, the loop reads and — on a rewind — replaces it.
        let task_registry = alter_zero::tasks::TaskRegistry::new();

        // The `/settings` knobs (docs/settings.md): the saved `settings.json`
        // with each `ALTER_ZERO_*` override applied on top. Resolved BEFORE the
        // backend, which is built around three of them (tools, retries,
        // temperature).
        let settings_path = config::settings_json_path();
        let saved_settings = config::load_saved_settings(settings_path.as_deref());
        let settings = config::apply_setting_overrides(saved_settings);

        // The user's lifecycle hooks (docs/hooks.md): `~/.alter-zero/hooks.json`
        // (or `ALTER_ZERO_HOOKS_FILE`), read once here and re-attached to every
        // backend rebuild through `HookSetup`. A malformed file is deliberately
        // loud — it becomes the startup toast below rather than a silent "no
        // hooks", which is how a user comes to trust a guard that isn't there.
        let hooks_path = alter_zero::llm::hooks::hooks_file_path(config::config_home().as_deref());
        let (hooks_file, hooks_error) = config::load_hooks(hooks_path.as_deref());
        let hook_setup = (!hooks_file.is_empty()).then(|| HookSetup {
            file: std::sync::Arc::new(hooks_file),
            session_id: host::session_id(),
            cwd: cwd.clone(),
            // The same tty-detach helper every other shell child gets.
            detach_helper: std::env::current_exe().ok(),
            enabled: config::hooks_enabled() && settings.hooks,
        });

        // The reply backend and everything that selects it (docs/llm.md).
        let mut models = ModelSession::resolve(
            &cwd,
            home.as_deref(),
            &registry,
            &agent_registry,
            permissions.gate(),
            &ask,
            &task_registry,
            &settings,
            hook_setup,
        );

        // The `@` file-search pipeline (docs/file-search.md): a background worker
        // walks the cwd and ranks it per query off the UI thread. The loop sends
        // queries on a std channel and receives results on a tokio channel it can
        // `select!` on. The worker only *sends* — it is not a stdin reader
        // (invariant 1).
        let (file_req_tx, file_req_rx) = std::sync::mpsc::channel::<String>();
        let (file_res_tx, file_rx) = tokio::sync::mpsc::unbounded_channel();
        let file_worker = spawn_file_search_worker(cwd.clone(), file_req_rx, file_res_tx);
        // The Ctrl+V image-paste pipeline (docs/image-paste.md) and the `/model`
        // picker's list fetch (docs/llm.md): each runs on its own short-lived
        // worker thread and reports here.
        let (img_tx, img_rx) = tokio::sync::mpsc::unbounded_channel();
        let (model_tx, model_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
        // The capability probe's own channel, so a concurrently-open `/model`
        // picker can't confuse the results (docs/reasoning.md).
        let (probe_tx, probe_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
        if let Some((provider, cfg)) = models.take_probe() {
            spawn_model_fetch(provider, cfg, CancelToken::new(), probe_tx);
        }

        // The /resume session recorder (docs/resume.md): mirrors App::history to a
        // rollout file, lazily created on the first recorded item so empty
        // sessions never touch disk.
        let recorder = SessionRecorder::new(&models.model_name(), &cwd);
        // The filesystem checkpoint store (docs/checkpoint.md): an isolated git
        // object store — never the user's real .git — that snapshots the whole cwd
        // per turn so a /resume or Esc-Esc backtrack can reset the code, not just
        // the transcript. Whether this host *can* snapshot at all is a `git`
        // binary being present and the cwd being a project in the first place
        // (`cwd_scope` — never a filesystem root, the home dir or an ancestor of
        // it, alter-zero's own state dir, a pseudo-filesystem, or a shared
        // scratch parent like `/tmp`): the session-start snapshot below runs before the
        // first frame paints, and `git add -A` is O(bytes), so an oversized cwd
        // blocks the raw-mode terminal for seconds while duplicating itself into
        // the store. `seed_checkpoints` then puts a cost *ceiling* on whatever
        // survives the scope check. Whether it *does* snapshot is the
        // `/settings` **Checkpoints** knob, which `ALTER_ZERO_CHECKPOINTS` seeds
        // (docs/settings.md) — applied right after, and flippable mid-session.
        let store_root = config::checkpoints_root();
        let state_dir = config::config_home();
        let tmp_dir = config::tmp_dir();
        let refusal = checkpoint::cwd_scope(
            &cwd,
            &checkpoint::CheckpointEnv {
                home: home.as_deref(),
                state_dir: state_dir.as_deref(),
                tmp_dir: tmp_dir.as_deref(),
            },
        )
        .err();
        let checkpoints_capable = refusal.is_none() && checkpoint::git_available();
        let mut checkpoints =
            CheckpointStore::new(store_root.as_deref(), &cwd, checkpoints_capable);
        checkpoints.set_enabled(settings.checkpoints_active());
        // Whether the *user* asked for checkpoints, kept before `settings` moves
        // into `seed_app` — a refusal is only worth a toast when it took
        // something away.
        let checkpoints_wanted = settings.checkpoints;

        let cwd_display = ui::display_cwd(&cwd, home.as_deref());
        let mut session = Self {
            term,
            app,
            models,
            render: ui::StreamRender::new(),
            agent_render: ui::StreamRender::new(),
            transcript: ui::TranscriptCache::new(),
            burst: PasteBurst::new(),
            clocks: StatusClocks::started_now(),
            toast_deadline: None,
            bg_clocks: HashMap::new(),
            agent_clocks: HashMap::new(),
            agent_expiry: HashMap::new(),
            // Invariant 1: the `EventStream` is created HERE — after
            // `InlineViewport::init` (in `main`) queried the cursor position over
            // stdin, synchronously — so it is the sole stdin reader from now on.
            events: EventStream::new(),
            frame,
            draw_rx,
            tx,
            reply_rx,
            file_req_tx,
            file_rx,
            last_file_query: None,
            img_tx,
            img_rx,
            model_tx,
            model_rx,
            probe_rx,
            bg_rx,
            agent_rx,
            _file_worker: file_worker,
            registry,
            agent_registry,
            permissions,
            ask,
            task_registry,
            recorder,
            hist_store,
            checkpoints,
            settings_path,
            saved_settings,
            inflight: None,
            reaping: Vec::new(),
            clipboard_lease: None,
            overlay_resized: false,
            cwd,
            cwd_display,
        };
        session.seed_app(settings);
        session.seed_checkpoints(refusal, checkpoints_wanted);
        // After the checkpoint toast, so a session with both problems ends up
        // showing the hooks one — the actionable typo beats the size refusal.
        session.report_hooks_error(hooks_error);
        let picker = session.apply_startup(startup);
        session.paint_first_frame(picker)?;

        Ok(session)
    }

    /// The injections the pure `App` needs from the boundary before the first
    /// frame: the footer's session context and permission mode, the context-window
    /// gauge, the system prompts the Ctrl+D view shows, and the project's
    /// AGENTS.md instructions.
    fn seed_app(&mut self, settings: alter_zero::settings::SessionSettings) {
        // The `/settings` knobs (docs/settings.md), plus what this host can
        // actually run — so an unavailable row says so from the first frame.
        *self.app.settings_mut() = settings;
        self.sync_setting_availability();
        // The footer's model name + cwd, formatted here at the boundary (the
        // set_clock pattern: the pure core never reads the environment) —
        // docs/footer.md — together with the system prompts and the gauge.
        self.sync_backend_info();
        // The thinking state `config.json` recorded for this exact selection, so
        // the Shift+Tab cycle starts where it left off (docs/reasoning.md).
        let thinking = self.models.take_thinking_seed();
        self.app.set_thinking(thinking);
        // The footer's right-edge permission segment (docs/permissions.md):
        // seeded from the gate (which just loaded this project's saved mode), or
        // hidden entirely when permissions are disabled — nothing asks, so a mode
        // would be a lie.
        let mode = self.permissions.mode();
        self.app.set_permission_mode(mode);
        // The project's AGENTS.md instructions (codex's project doc,
        // docs/project-doc.md): discovered root→cwd and injected like the system
        // prompt — the context derivation prepends them, the Ctrl+D view and the
        // token estimate carry them from the first frame. Refreshed at every turn
        // start, so this seed mostly serves the pre-first-turn Ctrl+D.
        // The **Project docs** knob can withhold them entirely
        // (docs/settings.md).
        let instructions = self
            .app
            .settings()
            .project_docs
            .then(|| project_doc::load_user_instructions(&self.cwd))
            .flatten();
        self.app.set_user_instructions(instructions);
    }

    /// Snapshot the pristine tree (history length 0) so a backtrack to the very
    /// first message restores it. A store that won't initialize (odd perms, disk
    /// full) simply yields no checkpoints for the session rather than killing the
    /// TUI — like recording.
    ///
    /// This snapshot runs **before the first frame paints**, so it is also the
    /// last chance to decide it shouldn't run at all. `scoped_out` carries the
    /// categorical verdict (`checkpoint::cwd_scope`, already applied to the
    /// store); on top of it a **pre-flight probe** measures what the snapshot
    /// would actually cost — enumerating with `git ls-files` instead of hashing,
    /// ~800× cheaper — and retires the store when a tree is past the budget.
    /// Either way the reason is toasted once, so the feature never goes quiet
    /// without saying why. See `docs/checkpoint.md`.
    fn seed_checkpoints(&mut self, scoped_out: Option<CheckpointRefusal>, wanted: bool) {
        if self.checkpoints.init().is_err() {
            return;
        }
        let refusal = scoped_out.or_else(|| {
            self.checkpoints
                .probe(&config::checkpoint_budget())
                .refusal()
                .inspect(|_| self.checkpoints.disable())
        });
        if let Some(reason) = refusal {
            // The `/settings` **Checkpoints** row was seeded from the store's
            // capability a moment ago; a probe refusal changes that answer.
            self.sync_setting_availability();
            // Silent when the user had already turned checkpoints off — a
            // refusal is only news when it took something away.
            if wanted {
                self.toast(reason.to_string(), ToastKind::Info);
            }
            return;
        }
        if let Some(commit) = self.checkpoints.snapshot("session start") {
            self.recorder
                .record_checkpoint(checkpoint::Checkpoint { after: 0, commit });
        }
    }

    /// Say once, at startup, that the `hooks.json` could not be read — the
    /// checkpoint refusal's rule: a feature that switches itself off must
    /// never do it quietly, because "my hook stopped firing" and "I typo'd the
    /// config" otherwise read as two unrelated problems (`docs/hooks.md`).
    fn report_hooks_error(&mut self, error: Option<String>) {
        if let Some(error) = error {
            self.toast(error, ToastKind::Error);
        }
    }

    /// Apply the CLI's `--continue`/`--resume` directive (`docs/cli.md`).
    ///
    /// A `Load` is the picker's `ResumeSession` arm run at startup — `main`
    /// already read + parsed the file (fail-fast), so this path cannot fail:
    /// restore the code state to the session's final checkpoint (backup snapshot
    /// first; an unknown commit or no recorded checkpoints leave the tree
    /// untouched — docs/checkpoint.md), install the transcript, and adopt the file
    /// so further turns append to it (the torn-tail repair and the file's own
    /// checkpoint chain included).
    ///
    /// Returns whether to boot into the `/resume` picker.
    fn apply_startup(&mut self, startup: Option<Startup>) -> bool {
        match startup {
            Some(Startup::Load(loaded)) => {
                let LoadedSession {
                    path,
                    text,
                    meta,
                    items,
                } = *loaded;
                let session_checkpoints = session::parse_checkpoints(&text);
                let restored = self.restore_final_checkpoint(&session_checkpoints);
                let count = items.len();
                self.app.load_session(items);
                // The checklist came back with the conversation — the shared
                // registry follows, exactly like the `/resume` picker's load
                // (docs/task-tools.md).
                self.sync_task_registry();
                let torn = !text.is_empty() && !text.ends_with('\n');
                self.recorder
                    .adopt(path, meta, count, torn, session_checkpoints);
                if restored {
                    self.toast(CHECKPOINT_RESTORED_NOTICE, ToastKind::Info);
                }
                false
            }
            Some(Startup::Picker) => true,
            None => false,
        }
    }

    /// Commit the startup banner (and a `--continue`/`--resume` load's
    /// transcript) and schedule the first paint — then, for a bare `--resume`,
    /// open the session picker over it.
    fn paint_first_frame(&mut self, picker: bool) -> io::Result<()> {
        // The startup header banner (docs/header.md): the ASCII wordmark +
        // version + cwd, committed to scrollback once here and re-emitted atop
        // every full repaint (resize, `/clear`) by the repaint. Pure chrome — it
        // never enters `history`, so it reaches neither the model nor the
        // `/resume` rollout. It flows in through the normal flicker-free pipeline
        // (the next draw writes it above the box in one synchronized frame).
        let width = self.term.screen().width;
        self.term.insert_before(ui::header_lines(&self.app, width));
        self.term.insert_before(vec![Line::default()]);
        // A --continue/--resume load commits the loaded conversation under the
        // banner through the same pipeline (docs/cli.md) — insert_before, never a
        // Purge: a fresh launch must not wipe the user's terminal scrollback (the
        // picker's mid-session purge exists to drop the *previous* conversation's
        // rows; at startup there are none). Capped like every full rebuild.
        if !self.app.history.is_empty() {
            let lines: Vec<Line<'static>> =
                ui::repaint_lines(&self.app.history, width, RESIZE_REFLOW_MAX_ROWS);
            self.term.insert_before(lines);
        }
        self.frame.schedule_frame(); // first paint
        // Bare --resume boots into the /resume picker (docs/cli.md): the
        // `OpenResumePicker` arm run before the first event. The header lines
        // queued above stay pending under the overlay and are dropped by the
        // return's reflow, which re-emits the banner itself (`ui::banner_tail`) —
        // whether the picker resumes a session, is dismissed, or quits outright.
        if picker {
            self.open_resume_picker()?;
        }
        Ok(())
    }

    /// Tear the session down on the way out, returning the active session's id
    /// when the run recorded a conversation — the exit hint `main` prints after
    /// the terminal is restored (`docs/cli.md`).
    pub(crate) fn shutdown(mut self) -> Option<String> {
        // The quit arms break before the loop-bottom sync — catch the last
        // change.
        self.recorder.sync(&self.app.history);
        let inputs = self.app.take_unpersisted_inputs();
        self.hist_store.append(&inputs);
        // Stop any in-flight reply on the way out, but don't `join()` it: joining
        // would couple the terminal restore to the backend's worst case (the
        // interrupt-lag freeze, on the quit path). The process exits right after
        // `term.restore()`, reaping any detached thread — the backend's and its
        // transport thread alike.
        if let Some((cancel, handle)) = self.inflight.take() {
            cancel.cancel();
            // A `!` shell turn's child is a separate PROCESS: the runner thread
            // dies with this process before its 20ms cancel poll can run, so the
            // reparented `sh -c` child would outlive the TUI (Esc and /clear kill
            // it only because the app stays alive long enough for the poll). Give
            // the runner a bounded window to observe the cancel and kill/reap the
            // child. A backend network thread is still never joined — the wait
            // applies only to the local shell runner, and it is bounded so a
            // wedged kill can't stall the quit.
            if self.app.status().is_some_and(|status| status.shell) {
                let deadline = std::time::Instant::now() + SHELL_QUIT_KILL_WINDOW;
                while !handle.is_finished() && std::time::Instant::now() < deadline {
                    std::thread::sleep(SHELL_POLL_INTERVAL / 2);
                }
            }
        }
        // Cancel a dangling /model fetch (its detached worker exits on the
        // cancel).
        self.models.cancel_model_fetch();
        // Kill every background shell on the way out — the sweep is synchronous
        // (direct process-group kills), so quitting can't orphan a `ping`
        // (docs/background.md) — and cancel every subagent (their threads observe
        // the token and die with the process either way; the cancel stops their
        // in-flight requests promptly — docs/agent-tool.md).
        self.registry.kill_all();
        self.agent_registry.kill_all();
        // The exit hint's handle (docs/cli.md): the active rollout's id, only when
        // this session holds a conversation — an empty session has no file and no
        // id (deferred create), and a `/clear`ed-then-idle one no history, so
        // neither prints a hint. Read after the final sync above, which is what
        // materializes the file for a quit that raced the loop-bottom sync.
        (!self.app.history.is_empty())
            .then(|| self.recorder.session_id().map(str::to_string))
            .flatten()
    }

    /// The loop-bottom bookkeeping, run after every event: the auto-compact check,
    /// the abandoned-permission release, the two on-disk mirrors, the transcript
    /// pre-render, and the detached-thread sweep.
    pub(crate) fn after_iteration(&mut self) {
        // Auto-compact (docs/compact.md): past codex's 90%-of-window threshold,
        // start the summarization turn on our own at this idle boundary — the loop
        // bottom sees every turn end and gauge change. `should_auto_compact`
        // pre-checks are cheap (the context derivation runs only once every gate
        // has passed), it can't fire mid-turn, and one attempt per user turn means
        // an Esc'd or failed compaction never loops. The StallAi test backend opts
        // out (it ignores the script).
        if self.inflight.is_none()
            && !self.models.is_stalled_test_backend()
            && self.app.should_auto_compact()
        {
            self.start_compact_turn(/*auto=*/ true);
            self.frame.schedule_frame();
        }
        // Release any permission request dropped without an answer (Esc,
        // `/clear`): the tool thread parked on it would otherwise wait for a
        // decision that is never coming — a cancelled turn's reaps itself, but a
        // background agent's has nothing to cancel it (docs/permissions.md).
        self.permissions.release_abandoned(&mut self.app);
        // …and any ask request dropped the same way, resolved as a decline so
        // the blocked thread wakes with the stop-and-wait result rather than
        // parking forever (docs/ask.md).
        for id in self.app.take_abandoned_asks() {
            self.ask
                .resolve(&id, alter_zero::ask::AskDecision::Declined);
        }
        // Mirror the finished history to the session file (docs/resume.md):
        // append what this iteration added, rewrite on a backtrack truncation,
        // nothing when unchanged — so streaming chunks (which never touch history)
        // cost no I/O.
        self.recorder.sync(&self.app.history);
        // Flush any inputs recorded this iteration to the persistent history file
        // (docs/history-persistence.md) — the drain is empty on iterations that
        // recorded nothing, so streaming ticks cost no I/O.
        let inputs = self.app.take_unpersisted_inputs();
        self.hist_store.append(&inputs);
        // Pre-render whatever this iteration committed into the Ctrl+O transcript
        // cache (docs/tool-view-performance.md) — a few integer compares when
        // nothing did, one grammar-highlight per new item when something did, the
        // whole loaded history on the iteration a `/resume` swapped it in. Paying
        // it here, at the boundary, is what makes the Ctrl+O keypress itself
        // O(live tail): the overlay never opens cold.
        let width = self.term.screen().width;
        self.transcript.warm(&self.app, width);
        // Reap detached backend threads (interrupt / `/clear` abandonments) that
        // have finished. `is_finished()` never blocks, so this can't stall the
        // loop; a thread still parked in its final network read is left until it
        // exits on its own. See `Session::abandon_inflight` / docs/interrupt.md.
        self.reaping.retain(|handle| !handle.is_finished());
    }
}

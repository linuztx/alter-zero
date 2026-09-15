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
use alter_zero::app::{App, CHECKPOINT_RESTORED_NOTICE, Mascot, Spinner, ToastKind};
use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::checkpoint::{self, CheckpointRefusal, CheckpointStore};
use alter_zero::frame;
use alter_zero::paste::PasteBurst;
use alter_zero::project_doc;
use alter_zero::scratchpad;
use alter_zero::session;
use alter_zero::stream::{CancelToken, StreamEvent};
use alter_zero::term::InlineViewport;
use alter_zero::ui;

use super::history_store::InputHistoryStore;
use super::models::{HookSetup, ModelSession};
use super::permission::PermissionStore;
use super::recorder::SessionRecorder;
use super::shell::{SHELL_POLL_INTERVAL, SHELL_QUIT_KILL_WINDOW};
use super::startup::{LoadedSession, Startup, StartupSession};
use super::view::RESIZE_REFLOW_MAX_ROWS;
use super::workers::{ModelFetch, spawn_file_search_worker, spawn_model_fetch};
use super::{Session, StatusClocks, config, host};

impl<'t> Session<'t> {
    /// Build the session — both ends of every channel included — then paint the
    /// first frame.
    ///
    /// `startup` is the CLI's `--continue`/`--resume` directive (`docs/cli.md`),
    /// applied before that frame — a `Load` restores the code state, installs
    /// the transcript and adopts the file for further recording; a `Picker`
    /// boots straight into the `/resume` overlay — plus the `[PROMPT]`, which
    /// is submitted as the first turn once the frame is scheduled.
    pub(crate) fn bootstrap(term: &'t mut InlineViewport, startup: Startup) -> io::Result<Self> {
        let Startup {
            session: startup,
            prompt,
        } = startup;
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
        // The working directory keys every per-directory file below
        // (docs/per-directory-state.md) — the two looks here, the `/model`
        // selection and `/settings` knobs further down, the permission rules.
        let cwd = std::env::current_dir().unwrap_or_default();
        let project = cwd.display().to_string();
        // The banner mascot (docs/mascot.md): this directory's saved `/mascot`
        // choice — or, launched in for the first time, the last choice made
        // anywhere, pinned as this directory's own right here — seeded before
        // the first frame commits the header so the banner draws it from
        // launch. An absent or corrupt file keeps the default.
        if let Some(mascot) =
            config::adopt_look::<Mascot>(config::mascot_json_path().as_deref(), &project)
        {
            app.set_mascot(mascot);
        }
        // The status spinner style (docs/spinner.md): the directory's saved
        // `/spinner` choice, seeded the same way so the first turn's status
        // line wears it.
        if let Some(spinner) =
            config::adopt_look::<Spinner>(config::spinner_json_path().as_deref(), &project)
        {
            app.set_spinner(spinner);
        }
        // The colour theme (docs/theme.md): the saved `/theme` choice, seeded
        // — and made the palette every renderer reads — before the banner is
        // built, so the first frame already wears it. An absent or corrupt
        // file keeps the default.
        if let Some(theme) = config::load_theme(config::theme_json_path().as_deref()) {
            app.set_theme(theme);
        }
        alter_zero::ui::activate_theme(app.theme());

        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        // How a `Read`/`Write`/`Edit` cell names its file (`docs/tools.md`
        // *Path display*): relative under this cwd, `~`-relative under home,
        // absolute elsewhere — injected once, like the clock, so the pure
        // renderers never read the environment. An unreadable cwd (the
        // `unwrap_or_default` above) leaves the verbatim policy.
        app.set_path_display(alter_zero::app::PathDisplay::new(cwd.clone(), home.clone()));

        // The session id, minted ONCE (it is nanos-derived — a second call is a
        // different id) and shared by everything that needs one: the temp tree
        // below and the lifecycle hooks' payloads, so a hook can find this
        // session's scratchpad and task output by the id it was handed.
        let session_id = host::session_id();
        // This session's own temp tree (docs/scratchpad.md):
        // `{tmp}/alter-zero-{uid}/{session}/` holding `scratchpad/` — where the
        // system prompt sends every temporary file — beside the `tasks/` dir
        // the background shells tee into. The scratchpad is created here,
        // before the prompt is assembled: `None` back means the agent is told
        // about no scratchpad and its writes there get no exemption.
        let session_tmp = config::session_tmp_root(&session_id);
        let scratchpad_dir = config::prepare_scratchpad(&session_tmp);
        // Where Ctrl+V pastes land (docs/image-paste.md): the config home,
        // not /tmp, so a resumed session still finds its pictures.
        let paste_dir = config::paste_store_dir(&session_id);
        // The store is durable, so it needs a bound: the oldest sessions'
        // folders go once it outgrows its cap (docs/image-paste.md).
        config::sweep_image_cache(&paste_dir);
        // Beside it, `images/` (docs/images.md "Memory"): the backend re-sends
        // every attached picture on every later turn, so the shrunk copy it
        // builds is kept here and read back instead of being decoded from the
        // original each time.
        alter_zero::images::set_payload_cache_dir(config::prepare_image_cache(&session_tmp));

        // The background-shell registry (docs/background.md): processes launched
        // by the model's `run_in_background` bash calls (or moved back with
        // Ctrl+B) report on their own channel — a dedicated `select!` source,
        // because they outlive turns and the reply channel is swapped on every
        // interrupt/`/clear`. Interim output tees to per-task files under the temp
        // dir so the model can `read` progress mid-run.
        let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel::<BgEvent>();
        // The interim files sit in the session's own temp tree —
        // `{tmp}/alter-zero-{uid}/{session}/tasks/{id}.output`, beside the
        // agent's `scratchpad/` (the pure `scratchpad::tasks_dir` over the
        // root resolved above; the uid/session injected at the boundary).
        // Short deliberately: the model reads these paths back out of every
        // background launch text.
        //
        // Every shell child (model `bash`, `run_in_background`, the `!` shell)
        // spawns detached from the controlling terminal (`subprocess::tiers` — the
        // `setsid` binary, else our own binary re-execed in the mode `main`
        // installed), so a `sudo` password prompt errors at once instead of
        // writing over the TUI. The helper path is resolved ONCE here — it stays
        // valid even if a `cargo build` replaces the file mid-session (and the
        // setsid tier doesn't need it at all); a failed `current_exe` (None) just
        // shortens the chain. The registry carries it to all three spawn sites.
        let registry = BackgroundRegistry::new(bg_tx, scratchpad::tasks_dir(&session_tmp))
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
        // …and pointed at the scratchpad, so a `write`/`edit` inside the
        // session's own temp directory resolves without a prompt
        // (`docs/scratchpad.md`). Nothing to point at = no exemption.
        if let Some(gate) = permissions.gate() {
            gate.set_scratchpad(scratchpad_dir.clone());
        }

        // The ask gate (docs/ask.md): the `AskUserQuestion` modal's answers
        // post here, waking the blocked tool thread. Always built — asking is
        // not a permission, so it doesn't follow ALTER_ZERO_PERMISSIONS.
        let ask = alter_zero::ask::AskGate::new();

        // The shared task list (docs/task-tools.md): the executor mutates it
        // on the tool thread, the loop reads and — on a rewind — replaces it.
        let task_registry = alter_zero::tasks::TaskRegistry::new();

        // The skills on disk (docs/skills.md): a `<root>/<name>/SKILL.md` walk
        // over the project's and the user's roots, done once here so the
        // listing and the tool set are settled before the backend is built.
        // A `SKILL.md` that will not parse is collected, not thrown — one bad
        // skill must not cost a session the rest — and becomes the startup
        // toast below.
        // The built-in `skill-creator` is seeded into the personal root first,
        // so this very walk finds it — a skill that appeared only on the
        // *second* launch would be missing from exactly the session that just
        // installed the app. Never clobbers an edited copy, and skipped
        // entirely when ALTER_ZERO_SKILLS_DIR replaced the roots.
        let mut skill_errors = match alter_zero::llm::skill::resolved_builtin_skills_dir(
            config::config_home().as_deref(),
        ) {
            Some(root) => alter_zero::llm::skill::seed_builtin_skills(&root),
            None => Vec::new(),
        };
        let (found_skills, walk_errors) =
            alter_zero::llm::skill::load_skills(&cwd, config::config_home().as_deref());
        skill_errors.extend(walk_errors);
        let skill_registry = alter_zero::skills::SkillRegistry::new(found_skills);
        // …and this project's saved on/off choices from `skills.json`, applied
        // BEFORE the backend is built so a session that starts with its last
        // skill turned off is never offered the tool (`docs/skills.md`).
        skill_registry.set_disabled(
            config::load_skills_file(config::skills_json_path().as_deref())
                .disabled_for(&cwd.display().to_string()),
        );

        // The subagent definitions on disk (docs/subagents.md): the built-in
        // `general-purpose`/`explore` seeded into the user root the first
        // time — so the defaults are editable files rather than a `match` in
        // the binary — then the walk over the project's and the user's roots.
        // Done here, before the backend, so a launch and the `<system-
        // reminder>` listing agree from the first turn. A file that will not
        // parse is collected, not thrown, and becomes the startup toast.
        let mut agent_errors =
            match alter_zero::llm::subagent::user_agents_dir(config::config_home().as_deref()) {
                Some(dir) => alter_zero::llm::subagent::seed_default_agents(&dir),
                None => Vec::new(),
            };
        let (found_agents, walk_errors) =
            alter_zero::llm::subagent::load_agents(&cwd, config::config_home().as_deref());
        agent_errors.extend(walk_errors);
        let subagents = alter_zero::subagents::SubagentRegistry::new(found_agents);

        // The project's `.alter-zero` config layer (docs/project-config.md):
        // each file read once, fingerprinted, and checked against trust.json
        // — the snapshot /trust reviews. Loaded BEFORE the MCP manager and
        // the hooks merge, both of which it feeds. Nothing untrusted runs.
        let (project_layer, trust_error) = super::trust::load_project_layer(&cwd);

        // The MCP servers (docs/mcp.md): the config files parsed and merged
        // (the project layer's, trust-aware), the manager built over them,
        // and every enabled server's connect kicked off on worker threads —
        // never blocking the first frame. The event channel is a select!
        // source like the background shells'.
        // The telemetry ping's report channel (docs/telemetry.md) — its own
        // `select!` source, because the worker outlives nothing but must
        // never write the file itself.
        let (telemetry_tx, telemetry_rx) =
            tokio::sync::mpsc::unbounded_channel::<alter_zero::telemetry::Delivery>();
        // The update check's report channel (docs/update.md), the same shape.
        let (update_tx, update_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (mcp_tx, mcp_rx) = tokio::sync::mpsc::unbounded_channel();
        let mcp_manager = config::mcp_enabled().then(|| {
            let manager = alter_zero::llm::mcp::McpManager::new(
                mcp_tx,
                super::mcp::load_mcp_sources(&cwd, project_layer.as_ref()),
            );
            manager.start_connections();
            manager
        });

        // The `/settings` knobs (docs/settings.md), per working directory
        // (docs/per-directory-state.md): this directory's saved entry — else
        // the file's seed — with each `ALTER_ZERO_*` override applied on top.
        // Resolved BEFORE the backend, which is built around three of them
        // (tools, retries, temperature).
        let settings_path = config::settings_json_path();
        let mut settings = config::load_settings_file(settings_path.as_deref())
            .settings_for(&cwd.display().to_string());
        // The Telemetry row's standing value is `telemetry.json`'s, not this
        // directory's entry — a user preference, not a project's
        // (`docs/telemetry.md`); the environment then overrides it for the
        // run like every other knob.
        settings.telemetry =
            config::load_telemetry_file(config::telemetry_json_path().as_deref()).enabled;
        // Likewise the Update check row's: update.json's (docs/update.md).
        settings.update_check =
            config::load_update_file(config::update_json_path().as_deref()).enabled;
        let settings = config::apply_setting_overrides(settings);

        // The user's lifecycle hooks (docs/hooks.md): `~/.alter-zero/hooks.json`
        // (or `ALTER_ZERO_HOOKS_FILE`), read once here and re-attached to every
        // backend rebuild through `HookSetup`. A malformed file is deliberately
        // loud — it becomes the startup toast below rather than a silent "no
        // hooks", which is how a user comes to trust a guard that isn't there.
        let hooks_path = alter_zero::llm::hooks::hooks_file_path(config::config_home().as_deref());
        let (hooks_file, hooks_error) = config::load_hooks(hooks_path.as_deref());
        // The trusted project layer unions in after the user's file
        // (docs/project-config.md — one more entry from the loader; an
        // untrusted or unparseable project file contributes nothing). The
        // user layer is kept apart on the session so a /trust decision can
        // rebuild this merge live.
        let user_hooks_file = hooks_file.clone();
        let hooks_file = match project_layer
            .as_ref()
            .and_then(super::trust::ProjectLayer::trusted_hooks)
        {
            Some(project_hooks) => user_hooks_file.merged(project_hooks),
            None => hooks_file,
        };
        // The rollout path is created lazily on the first recorded item, so
        // the payloads' `transcript_path` rides a shared cell the recorder
        // publishes into (below) and the sink reads at dispatch time. The
        // rest of the live handles ride beside it: the gate (a Shift+Tab cycle
        // reaches the very next payload), the SessionStart source queue —
        // seeded with `startup`, drained codex-style at the first turn's top
        // so nothing blocks the first paint — and the synthetic-turn mark.
        let hook_handles = alter_zero::llm::hooks::HookHandles {
            gate: permissions.gate().cloned(),
            ..alter_zero::llm::hooks::HookHandles::default()
        };
        if let Ok(mut sources) = hook_handles.sources.lock() {
            sources.push("startup".to_string());
        }
        let hook_transcript = hook_handles.transcript.clone();
        // Constructed even over an empty merge (docs/project-config.md): a
        // /trust approval mid-session swaps its file in place, and the live
        // handles must be THESE — a second set would split the transcript
        // path and the SessionStart queue. `hooks_available()` reads the
        // file's emptiness, so the /settings row reports exactly as before;
        // an empty file's sink is `None` either way.
        let hook_setup = Some(HookSetup {
            file: std::sync::Arc::new(hooks_file),
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            // The same tty-detach helper every other shell child gets.
            detach_helper: std::env::current_exe().ok(),
            // The **Hooks** row — off until a directory turns it on, with
            // `ALTER_ZERO_HOOKS` already merged over it for this run.
            enabled: settings.hooks,
            handles: hook_handles,
        });

        // The mid-turn message queue (docs/queue.md): what the user types
        // while a turn is running. The loop pushes onto it and every backend
        // build re-attaches the same handle, so a `/model` switch keeps the
        // queue the running turn's successor will drain.
        let steer = alter_zero::steer::SteerQueue::new();

        // The reply backend and everything that selects it (docs/llm.md).
        let mut models = ModelSession::resolve(
            &cwd,
            home.as_deref(),
            scratchpad_dir.as_deref(),
            &registry,
            &agent_registry,
            &steer,
            permissions.gate(),
            &ask,
            &task_registry,
            &skill_registry,
            &subagents,
            mcp_manager.as_ref(),
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
        let (device_tx, device_rx) = tokio::sync::mpsc::unbounded_channel();
        let (model_tx, model_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
        // The capability probe's own channel, so a concurrently-open `/model`
        // picker can't confuse the results (docs/reasoning.md).
        let (probe_tx, probe_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
        if let Some((provider, cfg)) = models.take_probe() {
            spawn_model_fetch(provider, cfg, CancelToken::new(), probe_tx);
        }

        // The /resume session recorder (docs/resume.md): mirrors App::history to a
        // rollout file, lazily created on the first recorded item so empty
        // sessions never touch disk. It publishes the rollout path into the
        // hooks' transcript cell whenever the active file changes
        // (docs/hooks.md).
        let recorder =
            SessionRecorder::new(&models.model_name(), &cwd).with_transcript(hook_transcript);
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
            context: ui::ContextCache::new(),
            burst: PasteBurst::new(),
            clocks: StatusClocks::started_now(),
            toast_deadline: None,
            bg_clocks: HashMap::new(),
            agent_clocks: HashMap::new(),
            agent_thinking_clocks: HashMap::new(),
            agent_command_clocks: HashMap::new(),
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
            paste_dir,
            device_tx,
            device_rx,
            device_cancel: None,
            device_expires: None,
            model_tx,
            model_rx,
            probe_rx,
            bg_rx,
            agent_rx,
            mcp_rx,
            telemetry_tx,
            telemetry_rx,
            telemetry_attempted: None,
            update_tx,
            update_rx,
            update_attempted: None,
            update_notice_pending: None,
            _file_worker: file_worker,
            registry,
            agent_registry,
            steer,
            permissions,
            ask,
            task_registry,
            skill_registry,
            subagents,
            mcp: mcp_manager,
            project_layer,
            user_hooks_file,
            // Seeded with the startup walk's failures, so the first turn's
            // rescan doesn't re-toast what the banner already said.
            reported_skill_errors: skill_errors
                .iter()
                .map(|error| error.path.clone())
                .collect(),
            reported_agent_errors: agent_errors
                .iter()
                .map(|error| error.path.clone())
                .collect(),
            recorder,
            hist_store,
            checkpoints,
            settings_path,
            inflight: None,
            reaping: Vec::new(),
            clipboard_lease: None,
            overlay_resized: false,
            flowed_view: None,
            cwd,
            cwd_display,
        };
        session.seed_app(settings);
        session.seed_checkpoints(refusal, checkpoints_wanted)?;
        // After the checkpoint toast, so a session with both problems ends up
        // showing the hooks one — the actionable typo beats the size refusal.
        session.report_hooks_error(hooks_error);
        session.report_skill_errors(&skill_errors);
        session.report_agent_errors(&agent_errors);
        session.report_mcp_errors();
        session.report_trust_state(trust_error);
        let picker = session.apply_startup(startup);
        session.paint_first_frame(picker)?;
        // The day's anonymous usage ping (docs/telemetry.md), AFTER the first
        // frame is queued so it can never delay it: the install id is minted
        // if this is the first launch, the one-time notice is committed under
        // the banner (before a [PROMPT]'s bubble below), and the send goes to
        // a detached thread. Nothing here can fail the boot.
        session.start_telemetry();
        // The day's update check (docs/update.md), the same posture: after
        // the first frame, a known newer release announced under the banner
        // at once, the request itself on a detached thread.
        session.start_update_check();
        // The [PROMPT] shortcut (docs/cli.md): the message given on the
        // command line becomes the first turn — after the loaded transcript
        // (if any) and the banner are queued, so its bubble lands under
        // them, exactly where a fast Enter would have put it. The grammar
        // already refused the one pairing with no sound meaning (a prompt
        // behind the interactive picker).
        if let Some(prompt) = prompt {
            session.submit_startup_prompt(prompt);
        }

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
        // Publish the image policy before the first line is built: the row
        // reservation is pure and reads it, so a picture drawn on the very
        // first frame (a `/resume`d conversation's image read) is already the
        // right size (`docs/images.md`).
        self.sync_image_policy();
        // The footer's model name + cwd, formatted here at the boundary (the
        // set_clock pattern: the pure core never reads the environment) —
        // docs/footer.md — together with the system prompts and the gauge.
        self.sync_backend_info();
        // The thinking state `config.json` recorded for this exact selection, so
        // the Ctrl+T cycle starts where it left off (docs/reasoning.md).
        let thinking = self.models.take_thinking_seed();
        self.app.set_thinking(thinking);
        // The service tiers `config.json` recorded for this exact selection,
        // so `/fast` and the footer marker are live from the first frame
        // rather than waiting on the capability probe (docs/fast-mode.md).
        let tiers = self.models.take_service_tier_seed();
        self.app.set_service_tier(tiers);
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
    /// without saying why. And a snapshot that *does* run announces itself
    /// first — the `Snapshotting …` row committed above the banner — because
    /// hashing is O(bytes) and a cold store's first snapshot can hold the
    /// first frame for seconds. See `docs/checkpoint.md`.
    fn seed_checkpoints(
        &mut self,
        scoped_out: Option<CheckpointRefusal>,
        wanted: bool,
    ) -> io::Result<()> {
        if self.checkpoints.init().is_err() {
            return Ok(());
        }
        // Bind the probe's cost instead of feeding it straight into
        // `.refusal()`: the measurement that decides whether to snapshot also
        // says what the snapshot will hash. A scoped-out cwd still never
        // spawns git.
        let cost = if scoped_out.is_some() {
            checkpoint::SnapshotCost::default()
        } else {
            self.checkpoints.probe(&config::checkpoint_budget())
        };
        let refusal = scoped_out.or_else(|| cost.refusal().inspect(|_| self.checkpoints.disable()));
        if let Some(reason) = refusal {
            // The `/settings` **Checkpoints** row was seeded from the store's
            // capability a moment ago; a probe refusal changes that answer.
            self.sync_setting_availability();
            // Silent when the user had already turned checkpoints off — a
            // refusal is only news when it took something away.
            if wanted {
                self.toast(reason.to_string(), ToastKind::Info);
            }
            return Ok(());
        }
        // Say what the coming seconds are for *before* paying them. The
        // notice needs its own forced frame: `insert_before` only queues, and
        // the loop's first draw tick sits on the far side of the snapshot.
        // Committed ahead of `paint_first_frame`'s banner it lands above it
        // in scrollback — chrome like the banner, never in `history`, and (a
        // one-time startup fact) not re-emitted by a purge rebuild. A warm
        // store with nothing new probes as zero and stays quiet, so ordinary
        // relaunches cost no extra frame.
        if let Some(notice) = checkpoint::snapshot_notice(&cost) {
            let width = self.term.screen().width;
            self.term
                .insert_before(ui::startup_notice_lines(&notice, width));
            self.draw_conversation()?;
        }
        if let Some(commit) = self.checkpoints.snapshot("session start") {
            self.recorder
                .record_checkpoint(checkpoint::Checkpoint { after: 0, commit });
        }
        Ok(())
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

    /// Say that some `SKILL.md` could not be loaded — the hooks toast's rule
    /// (`docs/skills.md`): a skill that silently never appears in the listing
    /// makes "the model ignores my skill" and "I typo'd the frontmatter" read
    /// as two unrelated problems. One row names the count and the first
    /// offender; the rest are the same shape.
    ///
    /// Shared with the per-turn rescan (`Session::rescan_skills`), which hands
    /// it only the files it has not already reported.
    pub(crate) fn report_skill_errors(&mut self, errors: &[alter_zero::skills::SkillError]) {
        let Some(first) = errors.first() else {
            return;
        };
        let more = errors.len() - 1;
        let tail = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        self.toast(
            format!("Skill {}: {}{tail}", first.path.display(), first.message),
            ToastKind::Error,
        );
    }

    /// Raise the first agent-definition failure as a red toast — the
    /// `report_skill_errors` rule one feature over (`docs/subagents.md`): an
    /// `agents/*.md` that silently never becomes a type makes "the model says
    /// my agent type is unknown" and "I typo'd the frontmatter" read as two
    /// unrelated problems.
    ///
    /// Shared with the per-turn rescan ([`Session::rescan_agents`]), which
    /// hands it only the failures it hasn't already raised.
    pub(crate) fn report_agent_errors(&mut self, errors: &[alter_zero::subagents::AgentFileError]) {
        let Some(first) = errors.first() else {
            return;
        };
        let more = errors.len() - 1;
        let tail = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        self.toast(
            format!("Agent {}: {}{tail}", first.path.display(), first.message),
            ToastKind::Error,
        );
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
    fn apply_startup(&mut self, startup: Option<StartupSession>) -> bool {
        match startup {
            Some(StartupSession::Load(loaded)) => {
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
                self.remember_loaded_image_sizes();
                // The checklist came back with the conversation — the shared
                // registry follows, exactly like the `/resume` picker's load
                // (docs/task-tools.md).
                self.sync_task_registry();
                let torn = !text.is_empty() && !text.ends_with('\n');
                self.recorder.adopt(
                    path,
                    meta,
                    count,
                    torn,
                    session_checkpoints,
                    self.app.history_generation(),
                );
                // A --continue/--resume boot is a *resume* boundary, not a
                // startup one: swap the seeded source so the SessionStart
                // hooks hear what actually happened (docs/hooks.md).
                self.models.set_session_source("resume");
                if restored {
                    self.toast(CHECKPOINT_RESTORED_NOTICE, ToastKind::Info);
                }
                false
            }
            Some(StartupSession::Picker) => true,
            None => false,
        }
    }

    /// Commit the startup banner (and a `--continue`/`--resume` load's
    /// transcript) and schedule the first paint — then, for a bare `--resume`,
    /// open the session picker over it.
    fn paint_first_frame(&mut self, picker: bool) -> io::Result<()> {
        // The startup header banner (docs/header.md): the gradient mascot +
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
            let lines: Vec<Line<'static>> = ui::repaint_lines(
                &self.app.history,
                width,
                RESIZE_REFLOW_MAX_ROWS,
                self.app.path_display(),
            );
            self.term.insert_before(lines);
        }
        self.frame.schedule_frame(); // first paint
        // Bare --resume boots into the /resume picker (docs/cli.md): the
        // `OpenResumePicker` arm run before the first event. The header lines
        // queued above stay pending under the overlay: a dismissal's return
        // flushes them (the banner appears once), while resuming a session
        // purge-rebuilds — the reflow drops the queue and re-emits the banner
        // itself (`ui::banner_tail`).
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
        self.recorder
            .sync(&self.app.history, self.app.history_generation());
        // SessionEnd (docs/hooks.md): fired before the teardown below, under
        // the sink's own 2 s budget, so a quit never hangs on a hook. Claude
        // Code's closest reason for an interactive quit is
        // `prompt_input_exit`.
        self.models.fire_session_end("prompt_input_exit");
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
        // Tear down the MCP connections (docs/mcp.md): drops every transport
        // — killing the stdio children synchronously — and cancels a running
        // auth flow, so quitting can't orphan a server process.
        if let Some(mcp) = &self.mcp {
            mcp.shutdown();
        }
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
        // A newer release the check found mid-turn is announced here, at the
        // first idle loop bottom, never inside a streaming reply (docs/update.md).
        self.flush_pending_update_notice();
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
        self.recorder
            .sync(&self.app.history, self.app.history_generation());
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

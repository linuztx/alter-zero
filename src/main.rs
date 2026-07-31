//! Thin terminal shell around the [`alter_zero`] library.
//!
//! This file is the one place that drives real terminal I/O, so it is
//! intentionally tiny and free of logic worth unit-testing — all of that lives
//! in `app`, `ui`, `stream`, `frame`, `paste`, and the geometry helpers `term`
//! consumes. Its job is only to:
//!
//! 1. open the custom inline viewport ([`term::InlineViewport`] — inline, real
//!    scrollback preserved, **dynamic** content-anchored height; an
//!    alternate-screen overlay is used *only* for the Ctrl+O tool-output view),
//! 2. run a codex-style **async** event loop ([`tokio`]): a `select!` over
//!    terminal input (an [`EventStream`]), streamed reply events (a tokio
//!    channel), coalesced draw ticks from the [`frame`] scheduler, `@`
//!    file-search results (`docs/file-search.md`), and finished Ctrl+V
//!    clipboard reads (a fifth channel — `docs/image-paste.md`),
//! 3. translate the [`App`]'s decisions into `insert_before` / `draw` calls.
//!
//! **Invariant 1 (stdin):** [`InlineViewport::init`] queries the cursor position
//! over stdin *once, synchronously*, before the [`EventStream`] exists — so the
//! `EventStream` is then the **sole** stdin reader. The reply backend runs on a
//! background thread that only *sends* on its channel, never reading stdin. A
//! second stdin reader would steal the cursor-position (DSR) reply — the source
//! of the "cursor position could not be read" error.
//!
//! Rendering is **tick-driven**: every state change calls
//! [`FrameRequester::schedule_frame`]; the scheduler coalesces a burst of those
//! into one rate-limited (120 fps) draw. A paste / fast-type run is detected by
//! [`PasteBurst`] so its characters request relaxed (non-immediate) frames,
//! the rate limiter coalescing the run into a few paints. `insert_before`
//! only **queues** its lines (codex's pending-history pattern): the draw tick
//! writes them and repaints the live region in one synchronized frame, so
//! scrollback growth never flashes a missing box (see `docs/flicker.md`).
//!
//! On **any width change** (and on returning from the tool-output overlay) the
//! visible conversation needs re-wrapping, so `App` retains a `history` and we
//! repaint from it — see [`repaint_conversation`].

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio_stream::StreamExt;

use alter_zero::agents::{AGENT_LINGER, AgentEvent, AgentRegistry};
use alter_zero::app::{
    Action, App, CHECKPOINT_RESTORED_NOTICE, CHECKPOINT_REWOUND_NOTICE, COPY_EMPTY_NOTICE,
    COPY_OK_NOTICE, HistoryItem, InterruptedTurn, ProviderChoice, QueuedTurn, Role, ToastKind,
    View,
};
use alter_zero::background::{BackgroundRegistry, BgEvent, PendingNotice};
use alter_zero::checkpoint;
use alter_zero::clipboard;
use alter_zero::context;
use alter_zero::file_search::{FileMatch, rank_files};
use alter_zero::frame::{self, FrameRequester};
use alter_zero::history;
use alter_zero::llm::{
    self, EnvFile, LlmBackend, ModelConfig, ModelEntry, ProvidersFile, ReasoningSupport, Selection,
    Settings, ThinkingMode, ThinkingSettings, backend::DEFAULT_SYSTEM_PROMPT,
};
use alter_zero::paste::{self, PasteBurst};
use alter_zero::permission::{PermissionDecision, PermissionGate};
use alter_zero::project_doc;
use alter_zero::session::{self, SessionMeta, SessionSummary};
use alter_zero::stream::{self, CancelToken, DummyAi, ReplySource, StreamEvent};
use alter_zero::term::{InlineViewport, ReflowClear};
use alter_zero::ui;

/// A finished `/model` fetch from one provider: its human label (for a failure
/// note) and either the provider's models or a one-line error. The picker
/// fetches every configured provider in parallel and merges these as they land.
/// Carried on the model-fetch worker's channel. See `docs/llm.md`.
type ModelFetch = (String, Result<Vec<ModelEntry>, String>);

fn main() -> io::Result<()> {
    // The detached-exec helper hook FIRST (crate::subprocess, docs/tools.md):
    // when this process was spawned as `{exe} __alter-zero-detached-exec
    // {cmd}` it is a shell runner's child, not a TUI — the hook `setsid()`s
    // away from the controlling terminal (so a `/dev/tty` password prompt
    // like `sudo`'s fails fast instead of hijacking the screen) and becomes
    // `sh -c {cmd}` in place, never returning. It must precede anything that
    // touches the terminal or spawns threads — the tokio runtime and
    // invariant 1's DSR cursor query included.
    alter_zero::subprocess::run_detached_exec_if_requested();
    tui_main()
}

#[tokio::main(flavor = "current_thread")]
async fn tui_main() -> io::Result<()> {
    // Build the tiktoken tokenizer (~125 ms of one-time rank parsing) off the
    // interactive path, concurrently with terminal init, so the first turn's
    // `count_tokens` doesn't freeze the loop. Detached; it never touches stdin
    // or the terminal (invariant 1 safe), and `tokenizer::warm` is idempotent.
    std::thread::spawn(alter_zero::tokenizer::warm);
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;
    let result = run(&mut term).await;
    // Always restore the terminal (raw mode off, cursor below the box), even if
    // the loop bailed out with an I/O error — then surface the first error.
    let restored = term.restore();
    result.and(restored)
}

/// The async event loop. A `select!` fans five sources onto one thread:
/// terminal input, the streamed reply, coalesced draw ticks, `@` file-search
/// results, and finished Ctrl+V clipboard reads. `select!` polls its branches
/// in randomized order, so input and draws can't starve each other — the
/// round-robin fairness codex builds explicitly.
async fn run(term: &mut InlineViewport) -> io::Result<()> {
    // Backend → loop (the streamed reply). A tokio channel so the loop can
    // `select!` on it; the backend thread sends without touching the runtime.
    // `mut` because an interrupt / `/clear` swaps in a fresh channel to isolate
    // a detached backend thread from the next turn (see `abandon_inflight`).
    let (mut tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    // The frame requester + its scheduler task, and the scheduler's draw-tick
    // channel (scheduler → loop).
    let (frame, frame_rx) = frame::channel();
    let (draw_tx, mut draw_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    tokio::spawn(frame::run_scheduler(frame_rx, draw_tx));

    let mut app = App::new();
    // Inject the wall-clock here at the I/O boundary so the pure library never
    // sees a clock. Recorded items are stamped with the local time; the stamp is
    // shown only in the Ctrl+O transcript (see docs/timestamps.md).
    app.set_clock(local_timestamp);
    // The persistent input history (docs/history-persistence.md): load the
    // JSONL file and seed `App::input_history` before the first paint, so ↑/↓
    // recall and Ctrl+R search span past sessions. New submissions are appended
    // per loop iteration below (`take_unpersisted_inputs` beside `recorder.sync`).
    let hist_store = InputHistoryStore::new();
    app.seed_input_history(hist_store.load());
    // The dummy's pre-stream pause so the status indicator shows first; the
    // pause is `STARTUP_DELAY` unless `ALTER_ZERO_STARTUP_DELAY_MS` overrides it
    // (the smoke test runs with a short delay; one phase uses a longer one). A
    // real backend's own first-token latency replaces it.
    let startup_delay = std::env::var("ALTER_ZERO_STARTUP_DELAY_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map_or(stream::STARTUP_DELAY, Duration::from_millis);
    // The background-shell registry (docs/background.md): processes launched
    // by the model's `run_in_background` bash calls (or moved back with
    // Ctrl+B) report on their own channel — a dedicated `select!` source,
    // because they outlive turns and the reply channel is swapped on every
    // interrupt/`/clear`. Interim output tees to per-task files under the
    // temp dir so the model can `read` progress mid-run. The started clocks
    // live here at the boundary (the `set_status_times` pattern); the pure
    // `App` sees only computed runtimes.
    let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel::<BgEvent>();
    // Claude Code's tasks layout: a stable per-user root, the cwd as one
    // dashed segment, and a per-session dir —
    // `{tmp}/alter-zero-{uid}/-home-user-proj/{session}/tasks/{id}.output`
    // (the pure `background::tasks_dir`; the uid/cwd/session injected here at
    // the boundary).
    let cwd = std::env::current_dir().unwrap_or_default();
    // Every shell child (model `bash`, `run_in_background`, the `!` shell)
    // spawns detached from the controlling terminal (`subprocess::tiers` —
    // the `setsid` binary, else our own binary re-execed in the mode `main`
    // installed above), so a `sudo` password prompt errors at once instead of
    // writing over the TUI. The helper path is resolved ONCE here — it stays
    // valid even if a `cargo build` replaces the file mid-session (and the
    // setsid tier doesn't need it at all); a failed `current_exe` (None) just
    // shortens the chain. The registry carries it to all three spawn sites.
    let registry = BackgroundRegistry::new(
        bg_tx,
        alter_zero::background::tasks_dir(
            &std::env::temp_dir(),
            process_uid(),
            &cwd,
            &session_id(),
        ),
    )
    .with_detach_helper(std::env::current_exe().ok());
    let mut bg_clocks: HashMap<String, Instant> = HashMap::new();
    // The subagent registry (docs/agent-tool.md): the model's `agent` tool
    // launches run their own loops on their own threads, reporting on a
    // dedicated channel — a seventh `select!` source, because agents outlive
    // turns exactly like background shells. The started clocks and the
    // finished agents' linger deadlines live here at the boundary (the
    // toast-deadline pattern).
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    let agent_registry = AgentRegistry::new(agent_tx);
    let mut agent_clocks: HashMap<String, Instant> = HashMap::new();
    let mut agent_expiry: HashMap<String, Instant> = HashMap::new();
    // The tool-permission gate (docs/permissions.md): every `write`/`edit`/
    // `bash` call — the main turn's and its subagents' — raises the inline
    // prompt and blocks its own thread on the answer. One gate for the whole
    // session, so "allow all edits" / "don't ask again for X" stick across
    // turns and across a `/model` rebuild. `ALTER_ZERO_PERMISSIONS=0` starts
    // without one and every tool runs unasked, as before the feature.
    let permissions: Option<PermissionGate> = permissions_enabled().then(PermissionGate::new);
    // The reply backend. The dummy is the default (and the fallback) so the app
    // always runs offline; a real OpenAI-compatible model activates only when a
    // provider, a model, and an API key all resolve and `ALTER_ZERO_DUMMY` isn't
    // forcing the dummy (see `build_backend` / docs/llm.md). `/model` rebuilds
    // it live, so it — plus the config it needs — is kept around.
    let providers = load_providers();
    // The persistent API-key store: `.env` in the cwd (or `ALTER_ZERO_ENV_FILE`),
    // written by the `/login` flow and consulted during key resolution (a real
    // process env var still wins). `set_var` is `unsafe` (forbidden here), so the
    // loaded keys live in this in-memory map rather than the process env; the
    // path is kept so `/login` can rewrite it. See `docs/llm.md`.
    let env_file_path = env_file_path();
    let mut env_file = load_env_file(&env_file_path);
    // The persisted `/model` selection (`~/.alter-zero/config.json`): the
    // provider + model chosen last run, so it survives a restart. Written on
    // each successful switch; real env vars still win over it. See `docs/llm.md`.
    let settings_path = settings_file_path();
    let saved = load_settings(settings_path.as_deref());
    // The (provider, model) pair config.json currently records — updated when
    // a /model switch rewrites it. Thinking-state writes (the probe, a
    // Shift+Tab cycle) attach to THIS pair only: an env-overridden selection
    // is never written back (env always wins, never sticks), so persisting
    // its thinking would hijack the saved default. See docs/reasoning.md.
    let mut persisted_selection: Option<(String, String)> =
        saved.provider.clone().zip(saved.model.clone());
    let temperature = std::env::var("ALTER_ZERO_TEMPERATURE")
        .ok()
        .and_then(|t| t.trim().parse::<f32>().ok());
    // The real backend's system prompt: the "Alter Zero" persona
    // (`prompts/alter_zero.md`) unless `ALTER_ZERO_SYSTEM_PROMPT` overrides it
    // (an empty value sends no system prompt at all — `with_system_prompt`
    // drops blanks). Either way we fold in the runtime environment — date, os,
    // cwd — so the agent has context awareness (docs/environment.md); the
    // values are gathered here at the boundary (the set_clock pattern), the
    // assembly is the pure `backend::augment_with_environment`. Folding once
    // here means every backend the loop rebuilds (`/model` switches) inherits
    // it via `system_prompt.clone()`.
    let system_prompt = std::env::var("ALTER_ZERO_SYSTEM_PROMPT")
        .ok()
        .or_else(|| Some(DEFAULT_SYSTEM_PROMPT.to_string()))
        .map(|base| {
            llm::backend::augment_with_environment(
                &base,
                &local_date(),
                &os_context(),
                &cwd.display().to_string(),
            )
        });
    // The provider the /model picker lists from and switches within: env, else
    // the saved selection, else the file's default. The active model starts from
    // env, then the saved selection, then tracks what the backend actually
    // answers as (so a dummy fallback shows `dummy_model_name`).
    let mut active_provider = std::env::var("ALTER_ZERO_PROVIDER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| saved.provider.clone())
        .or_else(|| providers.default_provider());
    // The saved provider/model are ONE selection: pairing the saved model with
    // a *different* (env-overridden) provider would ask that provider for a
    // model it may not serve, so the saved model applies only when the
    // resolved provider is the one it was saved with.
    let saved_model = saved
        .model
        .clone()
        .filter(|_| active_provider == saved.provider);
    let env_model = std::env::var("ALTER_ZERO_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .or(saved_model);
    // `ALTER_ZERO_STALL_MS` selects a test-only backend that ignores the cancel
    // for N ms — modelling a real network backend wedged in a blocking read
    // during the pre-first-token pause — so `scripts/smoke.sh` can prove an Esc
    // interrupt stays responsive even then. Never used in normal operation (it
    // preempts the real/dummy backend only when the env var is set). See
    // `docs/interrupt.md`.
    let stall_ms = std::env::var("ALTER_ZERO_STALL_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok());
    // The saved thinking blob describes the saved (provider, model) pairing —
    // like saved_model it applies only when that exact selection resolved.
    // Outer None = support unknown (the probe below finds out); Some(None) =
    // known non-reasoner; Some(Some(state)) = seed the Shift+Tab cycle.
    // See docs/reasoning.md.
    let saved_thinking: Option<Option<(ReasoningSupport, ThinkingMode)>> = saved
        .thinking
        .as_ref()
        .filter(|_| {
            active_provider == saved.provider && env_model.is_some() && env_model == saved.model
        })
        .map(ThinkingSettings::to_seed);
    let startup_thinking = saved_thinking.clone().flatten();
    // The saved model's image-input support — like saved_thinking it applies
    // only when that exact selection resolved; `None` = unknown (the probe
    // below finds out). Gates attachments on the backend and the Ctrl+V
    // paste warning toast. See docs/tools.md.
    let saved_vision: Option<bool> = saved.vision.filter(|_| {
        active_provider == saved.provider && env_model.is_some() && env_model == saved.model
    });
    let mut backend: Box<dyn ReplySource> = if let Some(ms) = stall_ms {
        Box::new(stream::StallAi::new(Duration::from_millis(ms)))
    } else {
        build_backend(
            &providers,
            &env_file,
            active_provider.as_deref(),
            env_model.as_deref(),
            temperature,
            startup_thinking.as_ref().map(|(_, mode)| *mode),
            saved_vision,
            system_prompt.clone(),
            startup_delay,
            &registry,
            &agent_registry,
            permissions.as_ref(),
        )
    };
    let mut active_model = backend.model_name();
    // Whether the *real* backend activated (vs the dummy fallback) — the
    // thinking state and its probe only make sense against a live provider.
    let real_backend = stall_ms.is_none()
        && !dummy_forced()
        && active_provider
            .as_deref()
            .zip(env_model.as_deref())
            .is_some_and(|(p, m)| {
                model_config_for(&providers, &env_file, p, m, temperature, None, None)
                    .is_some_and(|cfg| cfg.is_usable())
            });
    if real_backend {
        app.set_thinking(startup_thinking.clone());
    }
    // The active model's image-input support, tracked beside active_model:
    // `Some(false)` warns a Ctrl+V paste with a toast and rides every rebuilt
    // backend so attachments degrade gracefully (docs/tools.md). Meaningful
    // only against a real provider (the dummy sees no wire).
    let mut active_vision: Option<bool> = if real_backend { saved_vision } else { None };
    // The active model's context window (docs/compact.md), tracked beside
    // active_vision: drives the footer's `{used}/{window} ({pct}%)` gauge and the
    // auto-compact trigger. `ALTER_ZERO_CONTEXT_WINDOW` overrides whatever the
    // provider reports (and is the only way to get a gauge on the dummy);
    // otherwise the saved selection seeds it and the probe/{`/model`} keep it
    // fresh.
    let env_context_window: Option<u64> = std::env::var("ALTER_ZERO_CONTEXT_WINDOW")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&w| w > 0);
    let saved_context: Option<u64> = saved.context.filter(|_| {
        active_provider == saved.provider && env_model.is_some() && env_model == saved.model
    });
    let mut active_context: Option<u64> = if real_backend { saved_context } else { None };
    // Session context for the footer under the box — the backend's model name
    // and the cwd (shared with the tasks-dir derivation above) — formatted
    // here at the boundary (the set_clock pattern: the pure core never reads
    // the environment). See docs/footer.md.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd_display = ui::display_cwd(&cwd, home.as_deref());
    // The `~`-relative `.env` path shown in the `/login` provider-step hint, so
    // it names the real file even under an `ALTER_ZERO_ENV_FILE` override.
    let env_path_display = ui::display_cwd(&env_file_path, home.as_deref());
    app.set_session_info(backend.model_name(), cwd_display.clone());
    // The footer gauge + auto-compact window: the env override wins, else the
    // active model's known window (docs/compact.md).
    app.set_context_window(env_context_window.or(active_context));
    // The backend's system prompt rides into App so the Ctrl+D view shows the
    // whole context window (docs/context.md). None for the dummy.
    app.set_system_prompt(backend.system_prompt());
    // The project's AGENTS.md instructions (codex's project doc,
    // docs/project-doc.md): discovered root→cwd and injected like the system
    // prompt — the context derivation prepends them, the Ctrl+D view and the
    // token estimate carry them from the first frame. Refreshed at every
    // `start_turn`, so this seed mostly serves the pre-first-turn Ctrl+D.
    app.set_user_instructions(project_doc::load_user_instructions(&cwd));
    // The /resume session recorder (docs/resume.md): mirrors App::history to a
    // rollout file, lazily created on the first recorded item so empty
    // sessions never touch disk. `sync` runs once per loop iteration below.
    let mut recorder = SessionRecorder::new(&backend.model_name(), &cwd);
    // The filesystem checkpoint store (docs/checkpoint.md): an isolated git
    // object store — never the user's real .git — that snapshots the whole cwd
    // per turn so a /resume or Esc-Esc backtrack can reset the code, not just
    // the transcript. Keyed by cwd (checkpoints outlive a session), gated by
    // `ALTER_ZERO_CHECKPOINTS`, a `git` binary being present, and the cwd
    // being project-scoped (`cwd_allows_checkpoints` — never the home dir
    // itself, an ancestor of it, or a filesystem root: the session-start
    // snapshot below runs before the first frame, and a `git add -A` over a
    // whole home directory blocks the raw-mode terminal for minutes while
    // duplicating it into the store — the "hangs in `~`" bug). The initial
    // snapshot below captures the pristine tree (history length 0) so a
    // backtrack to the very first message restores it. All boundary I/O; the
    // pure mapping lives in `checkpoint`.
    let checkpoints_root = checkpoints_root();
    let checkpoints_enabled =
        checkpoint::enabled_by_env(std::env::var("ALTER_ZERO_CHECKPOINTS").ok().as_deref())
            && checkpoint::cwd_allows_checkpoints(&cwd, home.as_deref())
            && checkpoint::git_available();
    let checkpoints =
        checkpoint::CheckpointStore::new(checkpoints_root.as_deref(), &cwd, checkpoints_enabled);
    // A store that won't initialize (odd perms, disk full) simply yields no
    // checkpoints for the session rather than killing the TUI — like recording.
    if checkpoints.init().is_ok()
        && let Some(commit) = checkpoints.snapshot("session start")
    {
        recorder.record_checkpoint(checkpoint::Checkpoint { after: 0, commit });
    }
    // The `@` file-search pipeline (docs/file-search.md): a background worker
    // walks the cwd once and ranks it per query off the UI thread. The loop sends
    // queries on a std channel and receives results on a tokio channel it can
    // `select!` on; `last_file_query` (boundary state, like `committed`) dedupes
    // dispatches. The worker only *sends* — it is not a stdin reader (invariant 1).
    let (file_req_tx, file_req_rx) = std::sync::mpsc::channel::<String>();
    let (file_res_tx, mut file_rx) = tokio::sync::mpsc::unbounded_channel::<FileSearchResult>();
    let _file_worker = spawn_file_search_worker(cwd.clone(), file_req_rx, file_res_tx);
    let mut last_file_query: Option<String> = None;
    // The Ctrl+V image-paste pipeline (docs/image-paste.md): each paste runs
    // the clipboard read + decode + encode on its own short-lived worker
    // thread — a large screenshot's PNG encode takes real time, and doing it
    // inline would freeze the status animations and the composer. Results
    // come back on a tokio channel the loop `select!`s on; the worker only
    // *sends* — it is not a stdin reader (invariant 1).
    let (img_tx, mut img_rx) = tokio::sync::mpsc::unbounded_channel::<Result<PathBuf, String>>();
    // The `/model` picker's model-list fetch (docs/llm.md): opening the picker
    // spawns a worker that GETs the provider's `/models` off the UI thread and
    // sends the parsed list back here. Its `CancelToken` is held so closing the
    // picker (or selecting) cancels an in-flight fetch. The worker only *sends*
    // — not a stdin reader (invariant 1).
    let (model_tx, mut model_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
    let mut model_fetch_cancel: Option<CancelToken> = None;
    // The thinking-support probe (docs/reasoning.md): when the real backend is
    // active but the saved settings don't say whether its model reasons (an
    // env-selected model, or the first run since the upgrade that added the
    // blob), fetch the provider's `/models` once in the background and read
    // the active model's record out of it. A dedicated channel so a
    // concurrently-open `/model` picker can't confuse the results; the worker
    // only *sends* (invariant 1). `pending` gates the select! branch and is
    // cleared by the first result — or by a `/model` switch, which learns the
    // support first-hand from the picked entry.
    let (probe_tx, mut probe_rx) = tokio::sync::mpsc::unbounded_channel::<ModelFetch>();
    let mut thinking_probe_pending = false;
    if real_backend
        && (saved_thinking.is_none() || saved_vision.is_none())
        && let Some((p, m)) = active_provider.as_deref().zip(env_model.as_deref())
    {
        let cfg = model_config_for(&providers, &env_file, p, m, temperature, None, None);
        thinking_probe_pending = true;
        spawn_model_fetch(p.to_string(), cfg, CancelToken::new(), probe_tx);
    }
    // The in-flight reply's cancel token + thread handle, so a quit mid-stream
    // can stop it cleanly. `None` whenever no reply is streaming.
    let mut inflight: Option<(CancelToken, JoinHandle<()>)> = None;
    // Backend threads whose turn was interrupted or `/clear`ed: signalled to
    // cancel and *detached* to finish on their own. We never `join()` them on
    // the event loop — joining couples the UI to the thread's worst case, and
    // did freeze it for up to a network op-timeout back when the backend read
    // the socket itself (the interrupt-lag bug; the real backend now parks its
    // blocking network ops on a further detached transport thread and observes
    // the cancel within ~50 ms — see docs/llm.md). Finished ones are swept off
    // each loop iteration with the non-blocking `is_finished()`. See
    // docs/interrupt.md.
    let mut reaping: Vec<JoinHandle<()>> = Vec::new();
    // Keeps the last `/copy`'s native clipboard selection alive (Linux/arboard
    // serves it from a thread tied to the Clipboard's lifetime); replaced on each
    // copy, dropped at exit. Held only for its Drop — never read — hence the `_`
    // prefix. `None` until the first successful native copy (the OSC 52 fallback
    // needs no lease). See docs/copy.md.
    let mut _clipboard_lease: Option<clipboard::ClipboardLease> = None;
    // The incremental renderer that commits the in-progress reply to scrollback
    // as it streams — O(reply) over the whole stream, not O(reply²). It also
    // renders the strip's cheap preview line. See `docs/markdown.md`.
    let mut render = ui::StreamRender::new();
    // The agent session view's own incremental renderer (docs/agent-tool.md):
    // reset when a view opens, it commits the viewed agent's reply lines to
    // scrollback exactly like `render` does the main turn's.
    let mut agent_render = ui::StreamRender::new();
    // The Ctrl+O overlay's incrementally-built transcript: each committed item
    // is rendered once (the loop-bottom `transcript.warm`) and retained across
    // overlay closes, so opening the overlay — even right after a big `/resume`
    // load — assembles instead of re-highlighting all of history. See
    // docs/tool-view-performance.md.
    let mut transcript = ui::TranscriptCache::new();
    // Detects a paste / fast-type burst so its redraw can be coalesced.
    let mut burst = PasteBurst::new();
    // The live status indicator's clocks (impurity kept here, at the boundary):
    // when the turn was submitted, and when the current thinking phase began
    // (`None` when not in one). The pure `App` only ever sees the *computed*
    // durations, via `set_status_times`. See docs/status-indicator.md.
    let mut clocks = StatusClocks::started_now();
    // When the transient toast above the box should self-clear (`None` when
    // none is live). The impurity kept here, at the boundary — the timestamp
    // pattern, like `clocks`: `App` holds only the toast text, the draw tick
    // clears it when due and re-arms a frame while it lingers. See docs/toast.md.
    let mut toast_deadline: Option<Instant> = None;
    // Whether the terminal was resized while an overlay (Ctrl+O / `/resume`)
    // covered the inline view. The overlay just redraws at the new size, but
    // the main screen underneath was reflowed by the emulator — so the return
    // repaint must Purge-rebuild (like any resize, invariant 3) instead of the
    // usual in-place overwrite, or the emulator's own re-wrapped copy of the
    // old rows survives behind the repaint (the duplication `ReflowClear::Purge`
    // exists to clear). Consumed by the first overlay-exit repaint.
    let mut overlay_resized = false;

    // Init already queried the cursor over stdin; the EventStream is now the sole
    // stdin reader (see the module-level invariant note).
    let mut events = EventStream::new();

    // The startup header banner (docs/header.md): the ASCII wordmark + version +
    // cwd, committed to scrollback once here and re-emitted atop every full
    // repaint (resize, `/clear`) by `repaint_conversation`. Pure chrome — it
    // never enters `history`, so it reaches neither the model nor the `/resume`
    // rollout. It flows in through the normal flicker-free pipeline (the next
    // draw writes it above the box in one synchronized frame).
    term.insert_before(ui::header_lines(&app, term.screen().width));
    term.insert_before(vec![Line::default()]);

    frame.schedule_frame(); // first paint

    loop {
        tokio::select! {
            // 1. Terminal input. The events branch always matches (it binds the
            //    Option), so `select!` can never run out of armed branches.
            maybe_read = events.next() => {
                // Stdin closing (or `read?` bailing below) can leave the loop
                // with the Ctrl+O overlay still up — no exit_overlay runs on
                // these paths, so main's term.restore() leaves the alternate
                // screen itself (term::OVERLAY_ACTIVE, the panic hook's net).
                let Some(read) = maybe_read else { break }; // stdin closed
                match read? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match app.on_key(key) {
                            Action::Quit => {
                                // Drop back to the main screen before the loop exits
                                // if an overlay (the Ctrl+O transcript or the /resume
                                // picker) is up, so restore() lands on the chat.
                                // A turn may have finished while the overlay was
                                // showing — its scrollback commits were deferred
                                // (invariant 4) — so repaint the inline view from
                                // history (the reflow paints the box in the same
                                // frame) the same way a normal Ctrl+O return does;
                                // otherwise restore() lands on the stale live status
                                // strip ("Working… (… tokens)") instead of the
                                // committed "Done for Ns" summary.
                                if app.view != View::Conversation {
                                    term.exit_overlay()?;
                                    // Like every overlay return: a resize that
                                    // landed under the overlay upgrades the
                                    // repaint to Purge (the emulator reflowed
                                    // the main screen underneath — invariant 3).
                                    repaint_active_view(
                                        term,
                                        &mut app,
                                        &mut render,
                                        &mut agent_render,
                                        overlay_return_clear(&mut overlay_resized),
                                    )?;
                                }
                                break;
                            }
                            Action::None => {}
                            Action::Submit(text) => {
                                // Drain any Ctrl+V-attached images staged by the
                                // submit (docs/image-paste.md) to deliver them on
                                // the turn's typed image channel.
                                let images = app.take_submission_images();
                                inflight = Some(start_turn(
                                    term, &mut app, &tx, backend.as_ref(),
                                    TurnInput { texts: vec![text], images },
                                    &mut render, &mut clocks,
                                )?);
                            }
                            Action::PasteImage => {
                                // Ctrl+V: the clipboard I/O happens at the boundary
                                // (on_key stayed pure) but on a worker thread, not
                                // here — the read + decode + PNG encode of a large
                                // screenshot takes long enough to freeze the status
                                // animations and swallow keystrokes if run inline.
                                // The result arrives on the image channel (branch 5),
                                // which attaches it or commits the red notice.
                                spawn_image_paste(img_tx.clone());
                            }
                            Action::RunShell(command) => {
                                // `!command` from an idle composer: echo it, then
                                // run it locally as a turn (docs/shell-command.md).
                                // Reuses the streamed-reply channel + inflight
                                // handle, so the status strip, Esc-interrupt, and
                                // resize repaint all work exactly like an AI turn.
                                inflight = Some(run_shell(
                                    term, &mut app, &tx, command, &registry,
                                    &mut render, &mut clocks,
                                )?);
                            }
                            Action::KillBackground(id) => {
                                // `x` in the ↓ manager: stop the task. The
                                // registry's Exited event then removes the row
                                // and commits the stopped notice
                                // (docs/background.md).
                                registry.kill(&id);
                            }
                            Action::StopAgent(id) => {
                                // `x` on a roster row: cancel the subagent
                                // thread and settle the entry as interrupted —
                                // the row leaves the footer at once
                                // (docs/agent-tool.md). A background agent's
                                // stopped notice settles like a completion (a
                                // foreground one resolves with its group when
                                // the backend's wait loop sees the kill).
                                let _ = agent_registry.kill(&id);
                                agent_clocks.remove(&id);
                                agent_expiry.remove(&id);
                                if let Some(notice) = app.stop_agent(&id).flatten() {
                                    registry.post_notice(notice.context_text(), true);
                                    app.defer_agent_notice(notice);
                                    // The stopped background agent is done for
                                    // good — drop it from the roster now (the
                                    // user's `x` removes the row right away).
                                    app.remove_agent(&id);
                                    agent_registry.remove(&id);
                                    if !app.turn_active() {
                                        inflight = dispatch_after_turn(
                                            term, &mut app, &tx, backend.as_ref(), &registry,
                                            &mut render, &mut clocks,
                                            &checkpoints, &mut recorder,
                                        )?;
                                    }
                                }
                                frame.schedule_frame();
                            }
                            Action::ViewAgent(_) => {
                                // Enter on a roster row: swap the screen to
                                // that agent's own inline session — purge +
                                // rebuild from its transcript, the /clear
                                // shape (docs/agent-tool.md).
                                repaint_agent_view(term, &mut app, &mut agent_render)?;
                            }
                            Action::LeaveAgentView => {
                                // Esc (or Enter on `● main`): back to the main
                                // conversation — purge + rebuild from history,
                                // the in-flight partial included. The viewed
                                // agent's linger re-arms via the sweep (its
                                // deadline was pushed while viewed).
                                repaint_conversation(
                                    term, &mut app, &mut render, ReflowClear::Purge,
                                )?;
                            }
                            Action::AgentChat { id, text } => {
                                // Enter inside an agent session: deliver the
                                // draft to the agent — queued into its running
                                // loop, or a continuation run when idle
                                // (docs/agent-tool.md). The transcript already
                                // recorded it; commit the bubble in place.
                                if backend.spawn_agent_chat(&id, &text) {
                                    let width = term.screen().width;
                                    term.insert_before(ui::message_lines(
                                        Role::User, &text, width,
                                    ));
                                    term.insert_before(vec![Line::default()]);
                                    agent_clocks.entry(id.clone()).or_insert_with(Instant::now);
                                    agent_expiry.remove(&id);
                                } else {
                                    present_toast(
                                        &mut app,
                                        &mut toast_deadline,
                                        &frame,
                                        "Agent chat is not available with this backend"
                                            .to_string(),
                                        ToastKind::Error,
                                    );
                                }
                            }
                            Action::MoveToBackground => {
                                // Ctrl+B on a running command: raise the latch
                                // the runner's poll loop consumes to hand its
                                // child off (docs/background.md).
                                registry.request_background();
                            }
                            Action::ToggleToolView => {
                                // on_key already flipped app.view; sync the overlay.
                                if app.view == View::ToolOutput {
                                    term.enter_overlay()?;
                                    // Paint the overlay now rather than on the next
                                    // tick — the freshly-cleared alt screen would
                                    // show as a black flash for a frame otherwise.
                                    draw_tool_view(term, &mut app, &mut transcript)?;
                                } else {
                                    term.exit_overlay()?;
                                    // The transcript cache is deliberately RETAINED
                                    // across the close (docs/tool-view-performance.md):
                                    // its frozen prefix is what makes the next Ctrl+O
                                    // instant, and the loop-bottom `transcript.warm`
                                    // keeps appending to it as items commit.
                                    // Catch the inline view up on whatever streamed
                                    // — or was dispatched off the queue — while the
                                    // overlay was showing (the reflow regenerates
                                    // any pending user bubbles from history). An
                                    // in-place repaint keeps the terminal's own
                                    // scrollback (invariant 4 / Phase 7) — unless a
                                    // resize landed under the overlay, which forces
                                    // the purge-rebuild every resize gets. An open
                                    // agent session view repaints itself instead
                                    // (docs/agent-tool.md).
                                    repaint_active_view(
                                        term, &mut app, &mut render, &mut agent_render,
                                        overlay_return_clear(&mut overlay_resized),
                                    )?;
                                }
                            }
                            Action::ConfirmBacktrack => {
                                // Esc-Esc backtrack confirmed: history is already
                                // truncated to the chosen message and the composer
                                // prefilled. Reset the code to that point's
                                // checkpoint (docs/checkpoint.md) before returning
                                // to the inline view. The restore target is the
                                // latest checkpoint at or before the new history
                                // length; back up the current tree first
                                // (recoverable via the store's reflog). The
                                // loop-bottom recorder.sync sees history shrink and
                                // rewrites the file, dropping the rewound-away
                                // checkpoints in lockstep.
                                let restored = match checkpoint::restore_target(
                                    recorder.checkpoints(),
                                    app.history.len(),
                                ) {
                                    Some(commit) => {
                                        let commit = commit.to_string();
                                        let _ = checkpoints.snapshot("before backtrack restore");
                                        checkpoints.restore(&commit).unwrap_or(false)
                                    }
                                    None => false,
                                };
                                // Consume the overlay-resized flag (a resize under
                                // the overlay must not leak to the next return); we
                                // purge unconditionally below anyway.
                                let _ = overlay_return_clear(&mut overlay_resized);
                                term.exit_overlay()?;
                                // (The transcript cache needs no explicit clear: the
                                // truncation bumped the history generation, so the
                                // loop-bottom warm rebuilds the kept prefix.)
                                // Backtrack TRUNCATES history, so an in-place
                                // overwrite leaves the dropped exchange stale in
                                // scrollback (and on screen when it overflowed) —
                                // it only cleared on the next resize. Purge-rebuild
                                // like /resume and resize: the truncated
                                // conversation replaces the screen AND scrollback
                                // cleanly (invariant 3).
                                repaint_conversation(
                                    term, &mut app, &mut render, ReflowClear::Purge,
                                )?;
                                if restored {
                                    present_toast(
                                        &mut app, &mut toast_deadline, &frame,
                                        CHECKPOINT_REWOUND_NOTICE, ToastKind::Info,
                                    );
                                }
                            }
                            Action::ToggleContextDebug => {
                                // The Ctrl+D raw-context view — the same overlay
                                // dance as Ctrl+O (docs/context.md).
                                if app.view == View::ContextDebug {
                                    term.enter_overlay()?;
                                    draw_context_view(term, &mut app)?;
                                } else {
                                    term.exit_overlay()?;
                                    repaint_active_view(
                                        term, &mut app, &mut render, &mut agent_render,
                                        overlay_return_clear(&mut overlay_resized),
                                    )?;
                                }
                            }
                            Action::Notice(text) => {
                                // A slash command's one-off system notice. The helper
                                // finalises any mid-flight reply segment first (same
                                // ordering trick as a tool call) so the notice slots
                                // after it in scrollback and history alike.
                                commit_system_notice(term, &mut app, &mut render, &text);
                            }
                            Action::Copy(maybe_text) => {
                                // `/copy`: do the clipboard I/O here at the boundary
                                // (run_selected_command stayed pure). The pure core
                                // already decided *what* to copy — Some(text) or, for
                                // an empty conversation, None. The result surfaces as
                                // a transient toast, not a scrollback bullet. See
                                // docs/copy.md / docs/toast.md.
                                match maybe_text {
                                    None => present_toast(
                                        &mut app, &mut toast_deadline, &frame,
                                        COPY_EMPTY_NOTICE, ToastKind::Error,
                                    ),
                                    Some(text) => match clipboard::copy_to_clipboard(&text) {
                                        Ok(lease) => {
                                            // Hold the native selection alive for the
                                            // app's lifetime (Linux); None over OSC 52.
                                            _clipboard_lease = lease;
                                            present_toast(
                                                &mut app, &mut toast_deadline, &frame,
                                                COPY_OK_NOTICE, ToastKind::Info,
                                            );
                                        }
                                        Err(reason) => present_toast(
                                            &mut app, &mut toast_deadline, &frame,
                                            format!("Copy failed: {reason}"), ToastKind::Error,
                                        ),
                                    },
                                }
                            }
                            Action::Clear => {
                                // `/clear` already wiped the app state (history,
                                // streaming buffer, running tool, status, queued
                                // backlog). Mid-turn it is also a kill — the user
                                // asked for a fresh slate, not a finished turn — so
                                // stop the backend and isolate it from the blank
                                // screen. `abandon_inflight` cancels + detaches it
                                // and hands back a fresh channel (never join() on
                                // the loop — the interrupt-lag freeze): any stale
                                // chunk or ToolStart the dying thread still emits
                                // lands on the dropped receiver, so it can't
                                // repopulate the cleared state. See docs/interrupt.md.
                                let (new_tx, new_rx) =
                                    abandon_inflight(inflight.take(), &mut reaping);
                                tx = new_tx;
                                reply_rx = new_rx;
                                clocks.turn_start = None;
                                clocks.thinking_start = None;
                                render.reset();
                                // A fresh slate kills the background shells too
                                // (clear_conversation already forgot them, so
                                // their Exited events find nothing and owe no
                                // notice — docs/background.md). Notes already
                                // posted for the wiped conversation are dropped
                                // with it, else the next turn end would start a
                                // phantom follow-up turn about them.
                                registry.kill_all();
                                let _ = registry.take_pending_notices();
                                // …and the subagents (docs/agent-tool.md): the
                                // wiped roster drops their late events.
                                agent_registry.kill_all();
                                agent_clocks.clear();
                                agent_expiry.clear();
                                // …and the permission gate: `clear_conversation`
                                // dropped the prompt, the cancelled threads reap
                                // themselves, so no unclaimed decision may linger
                                // for the next turn (docs/permissions.md).
                                if let Some(gate) = permissions.as_ref() {
                                    gate.clear();
                                }
                                // A cleared conversation starts a fresh session
                                // file (codex's /new); the old one keeps what it
                                // had (docs/resume.md). Re-seed the checkpoint
                                // chain with a pristine snapshot of the current
                                // tree so a backtrack in the new session can
                                // restore its starting point (docs/checkpoint.md).
                                recorder.start_new();
                                if let Some(commit) = checkpoints.snapshot("session start") {
                                    recorder.record_checkpoint(checkpoint::Checkpoint {
                                        after: 0,
                                        commit,
                                    });
                                }
                                // Purge scrollback + clear the screen (codex's
                                // /clear), not just blank the visible screen —
                                // the old conversation must be gone from
                                // scrollback too, so scrolling up shows nothing.
                                repaint_conversation(
                                    term, &mut app, &mut render, ReflowClear::Purge,
                                )?;
                            }
                            Action::ResolvePermission { request, decision } => {
                                // The user answered the inline prompt: post the
                                // decision on the gate, waking the tool thread
                                // parked on it. The prompt is already closed and
                                // the composer draft restored (the pure core did
                                // that); a queued second request has already
                                // opened. See docs/permissions.md.
                                if let Some(gate) = permissions.as_ref() {
                                    // Remember the scope BEFORE posting, so the
                                    // standing rule is in force when the sweep
                                    // below asks what it now covers (the tool
                                    // thread would only remember once it woke).
                                    if decision == PermissionDecision::ApproveAlways {
                                        gate.remember(&request);
                                    }
                                    gate.resolve(&request.id, decision);
                                    // Parallel agents ask before any of them is
                                    // answered, so "don't ask again" has to reach
                                    // the requests already queued behind this one
                                    // — else three agents running the same command
                                    // ask three times after you said not to.
                                    for id in
                                        app.drain_covered_permissions(&|r| gate.allows(r))
                                    {
                                        gate.resolve(&id, PermissionDecision::Approve);
                                    }
                                }
                                frame.schedule_frame();
                            }
                            Action::Interrupt => {
                                // Esc mid-generation (codex-style): stop the backend
                                // but NEVER join it on the loop. A real backend can
                                // be parked in a blocking network read (the
                                // pre-first-token pause) for up to one op-timeout
                                // before it observes the cancel; join()ing there
                                // froze the whole UI — spinner, timer, composer —
                                // for that long (the interrupt-lag bug). Instead
                                // `abandon_inflight` cancels + detaches the thread
                                // and swaps in a fresh reply channel, so any stale
                                // event it still emits (a final chunk, or a
                                // ToolStart that would otherwise wedge a phantom
                                // running tool) lands on the dropped receiver and
                                // can't reach the next turn — replacing the old
                                // cancel+join+drain. See docs/interrupt.md.
                                let (new_tx, new_rx) =
                                    abandon_inflight(inflight.take(), &mut reaping);
                                tx = new_tx;
                                reply_rx = new_rx;
                                clocks.turn_start = None;
                                clocks.thinking_start = None;
                                // Interrupt only arises in the conversation view
                                // (overlay Esc returns instead), so nothing here
                                // touches the alternate screen.
                                match app.interrupt_turn() {
                                    Some(InterruptedTurn::Undone) => {
                                        // Nothing had streamed and nothing was
                                        // queued: the submission is undone —
                                        // interrupt_turn put the message back in
                                        // the composer and dropped it from
                                        // history. Repaint scrollback without it
                                        // (a purge rebuild from the truncated
                                        // history, like /clear — it resets `render`
                                        // itself). No notice, and no queue flush —
                                        // the empty queue is the undo's
                                        // precondition — but background
                                        // completions held during the turn still
                                        // settle (docs/background.md).
                                        repaint_conversation(
                                            term, &mut app, &mut render, ReflowClear::Purge,
                                        )?;
                                        inflight = dispatch_after_turn(
                                            term, &mut app, &tx, backend.as_ref(), &registry,
                                            &mut render, &mut clocks,
                                            &checkpoints, &mut recorder,
                                        )?;
                                    }
                                    Some(InterruptedTurn::Kept { partial, tool, notice, agents }) => {
                                        // Something streamed — keep it, commit the
                                        // notice (None for a `!` shell turn, whose
                                        // `⎿ Interrupted by user` cell already says
                                        // it — req 2). A live agent group resolved
                                        // as interrupted: stop its subagent threads
                                        // (the abandoned backend's own kill sweep
                                        // may still be parked in a network read)
                                        // and commit its red tree cell
                                        // (docs/agent-tool.md).
                                        // `commit_turn_failure`'s
                                        // `render.finish(partial)` needs the
                                        // committed-lines cache intact (it flushes
                                        // only the not-yet-committed tail), so
                                        // reset `render` AFTER it, never before.
                                        if let Some(group) = &agents {
                                            for entry in &group.agents {
                                                let _ = agent_registry.kill(&entry.id);
                                                agent_clocks.remove(&entry.id);
                                            }
                                        }
                                        commit_turn_failure(
                                            term, &app, &mut render, partial, tool, agents, notice,
                                        );
                                        render.reset();
                                        // The user interrupted to send their queued
                                        // follow-ups right away (their spec; codex's
                                        // submit-pending-steers-after-interrupt). The
                                        // front entry — the first queue — goes out now
                                        // (a text batch to the model, or a `!` command
                                        // run locally); any later batches iterate at
                                        // the following turn-ends. Held background
                                        // completions settle first, like every
                                        // turn end (docs/background.md).
                                        inflight = dispatch_after_turn(
                                            term, &mut app, &tx, backend.as_ref(), &registry,
                                            &mut render, &mut clocks,
                                            &checkpoints, &mut recorder,
                                        )?;
                                    }
                                    None => render.reset(),
                                }
                            }
                            Action::Compact => {
                                // /compact (docs/compact.md): run codex's
                                // summarization turn — the whole current context
                                // plus the fixed handoff prompt — with the reply
                                // diverted into the compact buffer (never
                                // rendered); `finish_compact` appends the marker
                                // at StreamDone and the derivation compacts the
                                // model's context from there on. The turn rides
                                // the normal `inflight` slot so Esc, /clear, and
                                // quit reap it like any other turn.
                                let compact_backend = if stall_ms.is_some() {
                                    None
                                } else {
                                    build_compact_backend(
                                        &providers, &env_file,
                                        active_provider.as_deref(), &active_model,
                                        temperature,
                                        app.thinking.as_ref().map(|t| t.mode),
                                        active_vision, system_prompt.clone(),
                                    )
                                };
                                inflight = Some(start_compact_turn(
                                    &mut app, &tx, backend.as_ref(),
                                    compact_backend.as_ref(),
                                    &mut render, &mut clocks, /*auto=*/ false,
                                ));
                            }
                            Action::OpenResumePicker => {
                                // /resume from an idle composer (docs/resume.md):
                                // scan the sessions dir here at the boundary —
                                // excluding the file being written — then swap to
                                // the picker on the alternate screen, painted at
                                // once (the Ctrl+O no-black-flash pattern).
                                let sessions =
                                    list_sessions(recorder.root(), recorder.active_path());
                                // The picker's cwd seeds its default Cwd filter —
                                // the same display formatting the recorder writes
                                // into each session's meta line.
                                app.open_resume_picker(sessions, cwd.display().to_string());
                                term.enter_overlay()?;
                                draw_resume_picker(term, &app)?;
                            }
                            Action::CloseResumePicker => {
                                // Esc/Ctrl+C dismissed the picker: the view is
                                // already back on the conversation — leave the
                                // overlay and repaint, the Ctrl+O return.
                                term.exit_overlay()?;
                                repaint_active_view(
                                    term, &mut app, &mut render, &mut agent_render,
                                    overlay_return_clear(&mut overlay_resized),
                                )?;
                            }
                            Action::ResumeSession(path) => {
                                // Enter on a picker row: read + parse the rollout
                                // here (the I/O). Success swaps the conversation
                                // and adopts the file for further recording;
                                // failure leaves the current conversation unharmed
                                // under a red notice (codex). Either way the
                                // overlay closes and the inline view repaints
                                // from the (new or unchanged) history.
                                let loaded =
                                    std::fs::read_to_string(&path).ok().and_then(|text| {
                                        session::parse_session(&text)
                                            .map(|(meta, items)| (text, meta, items))
                                    });
                                let clear = overlay_return_clear(&mut overlay_resized);
                                match loaded {
                                    Some((text, meta, items)) => {
                                        let count = items.len();
                                        // The code-reset side of resume
                                        // (docs/checkpoint.md): restore the cwd
                                        // to the session's *final* checkpoint so
                                        // the files match the transcript being
                                        // loaded. Back up the current tree first
                                        // (recoverable via the store's reflog);
                                        // an unknown commit (a session from a
                                        // different cwd) or no checkpoints leave
                                        // the code untouched.
                                        let session_checkpoints =
                                            session::parse_checkpoints(&text);
                                        let restored = match checkpoint::restore_target(
                                            &session_checkpoints,
                                            usize::MAX,
                                        ) {
                                            Some(commit) => {
                                                let commit = commit.to_string();
                                                let _ = checkpoints
                                                    .snapshot("before resume restore");
                                                checkpoints.restore(&commit).unwrap_or(false)
                                            }
                                            None => false,
                                        };
                                        app.load_session(items);
                                        // A file whose last line lost its newline
                                        // (a torn write) must not have the next
                                        // append glued onto it — the recorder
                                        // prefixes the repair. The parsed
                                        // checkpoints are adopted too so later
                                        // turns extend the same chain and a
                                        // backtrack restores against them.
                                        let torn = !text.is_empty() && !text.ends_with('\n');
                                        recorder.adopt(
                                            path,
                                            meta,
                                            count,
                                            torn,
                                            session_checkpoints,
                                        );
                                        term.exit_overlay()?;
                                        // A resumed session REPLACES the whole
                                        // conversation: purge-rebuild (like
                                        // /clear) so the loaded history fills
                                        // scrollback — an in-place repaint left
                                        // the old chat above it and put only the
                                        // last screenful of the resumed one on
                                        // record (its earlier turns were never
                                        // scrollback-committed in this run). An
                                        // open agent session view closes: the
                                        // user picked a conversation, so the
                                        // main screen is what they land on
                                        // (the roster keeps its agents).
                                        app.close_agent_view();
                                        repaint_conversation(
                                            term, &mut app, &mut render, ReflowClear::Purge,
                                        )?;
                                        if restored {
                                            present_toast(
                                                &mut app, &mut toast_deadline, &frame,
                                                CHECKPOINT_RESTORED_NOTICE, ToastKind::Info,
                                            );
                                        }
                                    }
                                    None => {
                                        app.close_resume_picker();
                                        term.exit_overlay()?;
                                        repaint_active_view(
                                            term, &mut app, &mut render, &mut agent_render, clear,
                                        )?;
                                        commit_error_notice(
                                            term, &mut app, &mut render,
                                            &format!(
                                                "Failed to load session: {}",
                                                path.display()
                                            ),
                                        );
                                    }
                                }
                            }
                            Action::Toast(text) => {
                                // A slash command's transient info toast — a soft
                                // rejection (/resume, /help run mid-turn) that
                                // self-clears above the box instead of landing in
                                // scrollback. See docs/toast.md.
                                present_toast(
                                    &mut app, &mut toast_deadline, &frame, text,
                                    ToastKind::Info,
                                );
                            }
                            Action::OpenModelPicker => {
                                // /model from an idle composer (docs/llm.md): open
                                // the inline picker (it replaces the composer — no
                                // alternate screen). Fetch **every** configured
                                // provider's list off-thread in parallel; the picker
                                // shows each as it lands and merges them into one
                                // list (branch 6). With none configured, skip the
                                // fetch and point the user at /login instead.
                                app.open_model_picker(active_model.clone());
                                if let Some(p) = active_provider.as_deref() {
                                    app.set_active_provider(p);
                                }
                                if let Some(c) = model_fetch_cancel.take() {
                                    c.cancel();
                                }
                                let configured: Vec<ProviderChoice> =
                                    provider_choices(&providers, &env_file)
                                        .into_iter()
                                        .filter(|c| c.configured)
                                        .collect();
                                if configured.is_empty() {
                                    app.set_models_needs_login();
                                } else {
                                    let cancel = CancelToken::new();
                                    model_fetch_cancel = Some(cancel.clone());
                                    app.begin_model_load(configured.len());
                                    for choice in &configured {
                                        let cfg = model_config_for(
                                            &providers, &env_file, &choice.id, &active_model,
                                            temperature, None, None,
                                        );
                                        spawn_model_fetch(
                                            choice.name.clone(),
                                            cfg,
                                            cancel.clone(),
                                            model_tx.clone(),
                                        );
                                    }
                                }
                            }
                            Action::CloseModelPicker => {
                                // Esc/Ctrl+C dismissed the inline picker: cancel a
                                // pending fetch and let the region collapse back to
                                // the composer on the next draw.
                                if let Some(c) = model_fetch_cancel.take() {
                                    c.cancel();
                                }
                            }
                            Action::SelectModel { provider, id, reasoning, vision, context } => {
                                // Enter on a picker row: rebuild the backend for the
                                // chosen provider/model (docs/llm.md). The picker is
                                // already closed (on_key did it); cancel any pending
                                // fetch, then switch if the config is usable (has a
                                // key), else keep the current backend. The outcome is
                                // a transient toast — a mid-turn switch must not split
                                // the streaming reply in scrollback. See docs/toast.md.
                                if let Some(c) = model_fetch_cancel.take() {
                                    c.cancel();
                                }
                                // The picked entry's reasoning support seeds the
                                // Shift+Tab cycle at its default mode (medium
                                // where offered) — docs/reasoning.md.
                                let thinking = reasoning.map(|support| {
                                    let mode = support.default_mode();
                                    (support, mode)
                                });
                                match model_config_for(
                                    &providers, &env_file, &provider, &id, temperature,
                                    thinking.as_ref().map(|(_, mode)| *mode), vision,
                                ) {
                                    Some(cfg) if cfg.is_usable() => {
                                        backend = Box::new(
                                            LlmBackend::with_system_prompt(
                                                cfg,
                                                system_prompt.clone(),
                                            )
                                            .with_background(registry.clone())
                                            .with_agents(agent_registry.clone()),
                                        );
                                        active_provider = Some(provider.clone());
                                        active_model = id.clone();
                                        app.set_session_info(
                                            backend.model_name(),
                                            cwd_display.clone(),
                                        );
                                        app.set_system_prompt(backend.system_prompt());
                                        // The switch knows its support first-hand —
                                        // a still-in-flight startup probe is stale.
                                        thinking_probe_pending = false;
                                        app.set_thinking(thinking.clone());
                                        active_vision = vision;
                                        // The picked entry's context window seeds
                                        // the footer gauge + auto-compact (the
                                        // env override still wins) —
                                        // docs/compact.md.
                                        active_context = context;
                                        app.set_context_window(
                                            env_context_window.or(active_context),
                                        );
                                        // Persist the choice (and its reasoning +
                                        // vision + context-window state) so it's
                                        // the default next run (docs/llm.md,
                                        // docs/reasoning.md, docs/tools.md,
                                        // docs/compact.md).
                                        persisted_selection =
                                            Some((provider.clone(), id.clone()));
                                        save_settings(
                                            settings_path.as_deref(),
                                            &provider,
                                            &id,
                                            Some(thinking_settings_of(thinking.as_ref())),
                                            vision,
                                            context,
                                        );
                                        present_toast(
                                            &mut app,
                                            &mut toast_deadline,
                                            &frame,
                                            format!("Switched model to {id}"),
                                            ToastKind::Info,
                                        );
                                    }
                                    _ => {
                                        let env = key_env_name(&providers, &provider);
                                        present_toast(
                                            &mut app,
                                            &mut toast_deadline,
                                            &frame,
                                            format!(
                                                "Can't switch to {id}: run /login to set {env}"
                                            ),
                                            ToastKind::Error,
                                        );
                                    }
                                }
                            }
                            Action::SetThinking(mode) => {
                                // Shift+Tab advanced the thinking mode (the pure
                                // state already moved — docs/reasoning.md). Rebind
                                // the *next* turn's backend so the mode rides its
                                // request (the running turn streams on its own
                                // thread, untouched — the /model pattern), persist
                                // the choice beside the model selection, and
                                // confirm with a transient toast.
                                if let Some(provider) = active_provider.as_deref()
                                    && let Some(cfg) = model_config_for(
                                        &providers, &env_file, provider, &active_model,
                                        temperature, Some(mode), active_vision,
                                    )
                                    && cfg.is_usable()
                                {
                                    backend = Box::new(
                                        LlmBackend::with_system_prompt(
                                            cfg,
                                            system_prompt.clone(),
                                        )
                                        .with_background(registry.clone()),
                                    );
                                }
                                if let Some(provider) = active_provider.as_deref()
                                    && persisted_selection.as_ref().is_some_and(|(p, m)| {
                                        p == provider && *m == active_model
                                    })
                                {
                                    let thinking = app
                                        .thinking
                                        .as_ref()
                                        .map(|t| (t.support.clone(), t.mode));
                                    save_settings(
                                        settings_path.as_deref(),
                                        provider,
                                        &active_model,
                                        Some(thinking_settings_of(thinking.as_ref())),
                                        active_vision,
                                        active_context,
                                    );
                                }
                                present_toast(
                                    &mut app,
                                    &mut toast_deadline,
                                    &frame,
                                    format!("Thinking: {}", mode.label()),
                                    ToastKind::Info,
                                );
                            }
                            Action::OpenKeyOnboarding => {
                                // /login from an idle composer (docs/llm.md): open
                                // the inline onboarding, its provider choices built
                                // from the file with the ✓ reflecting real env /
                                // .env key resolution. The hint names the real .env
                                // path (env_path_display).
                                app.open_key_onboarding(
                                    provider_choices(&providers, &env_file),
                                    env_path_display.clone(),
                                );
                            }
                            Action::CloseKeyOnboarding => {
                                // Esc/Ctrl+C dismissed the flow: nothing to reap; the
                                // region collapses back to the composer on next draw.
                            }
                            Action::SaveApiKey {
                                provider,
                                env_var,
                                key,
                            } => {
                                // Persist the key to the .env store, refresh the
                                // in-memory copy (so the next /model fetch/switch
                                // resolves it immediately), and confirm. `set_var`
                                // is forbidden here, so the value never enters the
                                // process env — only the map. See docs/llm.md.
                                let current = std::fs::read_to_string(&env_file_path)
                                    .unwrap_or_default();
                                let updated = EnvFile::upsert(&current, &env_var, &key);
                                // The config home (`~/.alter-zero`) may not exist
                                // yet — create it before the first write.
                                if let Some(parent) = env_file_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                match write_key_store(&env_file_path, &updated) {
                                    Ok(()) => {
                                        env_file = EnvFile::parse(&updated);
                                        present_toast(
                                            &mut app,
                                            &mut toast_deadline,
                                            &frame,
                                            format!(
                                                "Saved {env_var} — run /model to use {provider}"
                                            ),
                                            ToastKind::Info,
                                        );
                                    }
                                    Err(e) => {
                                        present_toast(
                                            &mut app,
                                            &mut toast_deadline,
                                            &frame,
                                            format!(
                                                "Couldn't write {}: {e}",
                                                env_file_path.display()
                                            ),
                                            ToastKind::Error,
                                        );
                                    }
                                }
                            }
                        }
                        schedule_for_key(&frame, &mut burst, &key);
                        // The edit may have changed the active `@token`; kick off
                        // a (coalesced) file search if so (docs/file-search.md).
                        dispatch_file_search(&app, &file_req_tx, &mut last_file_query);
                        // Remove the temp PNGs of any attachments this key
                        // discarded (an atomic placeholder delete, a Ctrl+C
                        // clear, a /clear'd queue) — the pure core records the
                        // drops, the file I/O lives here (docs/image-paste.md).
                        for path in app.take_discarded_images() {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    Event::Resize(width, height) => {
                        let size_changed = term.resized(width, height);
                        // Repaint from history on ANY dimension change (codex
                        // redraws from source on every resize): a width change
                        // stales the wrapping, and a height change moves the
                        // screen contents out from under the tracked viewport
                        // row — repainting at a stale row leaves phantom input
                        // boxes behind. Purge scrollback + clear the screen
                        // first (codex's resize replay), rebuilding the whole
                        // conversation from history — the in-place overwrite
                        // otherwise left the emulator's own reflowed copy of the
                        // old content on screen, duplicating the TUI text. Only
                        // the inline view reflows; the overlay just redraws at
                        // the new size (reflowing would write the alternate
                        // screen).
                        if size_changed && app.view == View::Conversation {
                            repaint_active_view(
                                term, &mut app, &mut render, &mut agent_render,
                                ReflowClear::Purge,
                            )?;
                        } else if size_changed {
                            // Under an overlay the inline view can't reflow
                            // (it would write the alternate screen) — remember
                            // to purge-rebuild on return instead of the usual
                            // in-place overwrite (see `overlay_resized`).
                            overlay_resized = true;
                        }
                        burst.reset();
                        frame.schedule_frame();
                    }
                    // A real bracketed paste (term::init enables it). A large
                    // paste collapses to a `[Pasted Content N chars]` placeholder
                    // in the composer, expanded back on send — docs/paste.md.
                    // The /resume picker's type-to-search accepts pastes too
                    // (codex normalizes them into the query); only the Ctrl+O
                    // overlay ignores them, like typing there.
                    Event::Paste(pasted) => {
                        match app.view {
                            // The `/login` flow takes pastes (an API key is always
                            // pasted); the `/model` picker's filter is typed, so a
                            // paste there is swallowed rather than editing the
                            // hidden composer draft underneath it.
                            View::Conversation if app.key_onboarding.is_some() => {
                                app.paste_into_key_onboarding(&pasted);
                            }
                            View::Conversation if app.model_picker.is_some() => {}
                            View::Conversation => {
                                app.on_paste(&pasted);
                                // The paste may have changed the active `@token`.
                                dispatch_file_search(&app, &file_req_tx, &mut last_file_query);
                            }
                            View::ResumePicker => app.paste_into_resume_search(&pasted),
                            View::ToolOutput | View::ContextDebug => {}
                        }
                        burst.reset();
                        frame.schedule_frame();
                    }
                    _ => {}
                }
            }

            // 2. A streamed reply event. App state is always updated; lines are
            //    only committed to scrollback in the conversation view (in the
            //    overlay we hold off and repaint on return).
            Some(stream_event) = reply_rx.recv() => {
                // A launched agent group: start each member's runtime clock
                // (the boundary owns the clocks — docs/agent-tool.md).
                if let StreamEvent::AgentBatch { agents, .. } = &stream_event {
                    for spec in agents {
                        agent_clocks.insert(spec.id.clone(), Instant::now());
                    }
                }
                // A resolving agent group: the subagents' terminal events were
                // enqueued on the agent channel *before* the backend sent this
                // resolution, so drain that channel first — the roster
                // snapshots the recorded group entries are built from are then
                // final (docs/agent-tool.md).
                let group_done_ids: Option<Vec<String>> =
                    if let StreamEvent::AgentGroupDone { agents, background } = &stream_event {
                        (!background).then(|| agents.iter().map(|a| a.id.clone()).collect())
                    } else {
                        None
                    };
                if matches!(&stream_event, StreamEvent::AgentGroupDone { .. }) {
                    while let Ok(AgentEvent::Stream { id, event }) = agent_rx.try_recv() {
                        on_agent_event(
                            term, &mut app, &mut agent_render, &registry, &agent_registry,
                            &mut agent_clocks, &mut agent_expiry, &id, event,
                        )?;
                    }
                }
                let resolved = on_stream_event(
                    term, &mut app, &mut render, &mut clocks, &agent_registry, stream_event,
                )?;
                // A foreground group's members are settled now — arm their
                // linger sweeps (a background group's keep running).
                if let Some(ids) = group_done_ids {
                    for id in ids {
                        agent_clocks.remove(&id);
                        if app.agent(&id).is_some_and(|run| run.status.is_final()) {
                            agent_expiry.insert(id, Instant::now() + AGENT_LINGER);
                        }
                    }
                }
                if resolved {
                    // The stream ended. Settle any background completions
                    // still held (most settle earlier, at a tool boundary —
                    // this catches ones that landed during the final text),
                    // then send the next queued batch as the following turn
                    // (`None` when nothing is queued) — Enter messages batch
                    // into one turn, while Tab-opened follow-up batches each
                    // flush at their own turn-end, so they iterate in order;
                    // with nothing queued, a model-launched completion the
                    // agent never heard about (its note untaken on the board)
                    // dispatches the automatic follow-up turn instead
                    // (docs/background.md). This runs under the Ctrl+O overlay
                    // too (codex's queue drains at turn end regardless of its
                    // Ctrl+T view, the transcript following along): dispatching
                    // only records history and *queues* the user bubbles —
                    // `term` never flushes pending lines into the alternate
                    // screen, and the return's reflow drops + regenerates them
                    // from history — so invariant 4 holds.
                    inflight = dispatch_after_turn(
                        term, &mut app, &tx, backend.as_ref(), &registry,
                        &mut render, &mut clocks,
                        &checkpoints, &mut recorder,
                    )?;
                }
                frame.schedule_frame();
            }

            // 3. A coalesced draw tick: refresh the status times, then paint.
            //    While a turn is in flight, re-arm the next animation frame
            //    (codex's status-widget pattern): each draw schedules another
            //    ~30 fps tick, so the verb's shimmer sweeps and the timer
            //    advances even when no reply event arrives (a tool run, a
            //    thinking pause). The chain seeds from the Submit keypress and
            //    stops by itself on the first draw after the turn ends.
            Some(()) = draw_rx.recv() => {
                update_status_times(&mut app, &clocks);
                // Inject each background shell's runtime (the boundary owns
                // the started clocks — docs/background.md), so the manager's
                // details view ticks.
                for (id, started) in &bg_clocks {
                    app.set_background_runtime(id, started.elapsed());
                }
                // …and each running agent's, so the roster's elapsed ticks
                // (docs/agent-tool.md).
                for (id, started) in &agent_clocks {
                    app.set_agent_runtime(id, started.elapsed());
                }
                // Arm a linger deadline for every settled roster entry that
                // lacks one — self-healing over every settle path (a group
                // resolution, an Esc interrupt, a backend error, an `x`), so
                // no path can strand a finished row.
                let now = Instant::now();
                let settled: Vec<String> = app
                    .agents()
                    .iter()
                    .filter(|run| run.status.is_final())
                    .map(|run| run.id.clone())
                    .collect();
                for id in settled {
                    agent_expiry.entry(id).or_insert(now + AGENT_LINGER);
                }
                // Sweep finished agents whose linger expired — deferred while
                // the user is inside that agent's session view (the deadline
                // pushes forward, so leaving restarts the full linger), and a
                // timer whose agent reopened (a chat continuation) is dropped.
                agent_expiry.retain(|id, deadline| {
                    if app.agent(id).is_none_or(|run| !run.status.is_final()) {
                        return false;
                    }
                    if app.agent_view.as_deref() == Some(id.as_str()) {
                        *deadline = now + AGENT_LINGER;
                        return true;
                    }
                    if now >= *deadline {
                        app.remove_agent(id);
                        agent_registry.remove(id);
                        return false;
                    }
                    true
                });
                // Expire the transient toast when its deadline passes (so this
                // very frame paints without it); while it still lingers, keep a
                // frame pending for the eventual clear — a coalesced keystroke
                // frame can consume the one `present_toast` scheduled, so
                // re-arming here guarantees the clear fires. See docs/toast.md.
                if let Some(deadline) = toast_deadline {
                    let now = Instant::now();
                    if now >= deadline {
                        app.clear_toast();
                        toast_deadline = None;
                    } else {
                        frame.schedule_frame_in(deadline - now);
                    }
                }
                match app.view {
                    View::Conversation => {
                        // Compute the strip preview cheaply once per frame (O(one
                        // line); a forming table re-renders just its own block),
                        // so the status animation never re-renders the whole
                        // reply — and inject its row count so the layout reserves
                        // it. See `docs/markdown.md`, `docs/table-streaming.md`.
                        let preview = stream_preview_lines(&mut app, &mut render, term.screen());
                        draw(term, &app, preview.as_deref())?;
                    }
                    View::ToolOutput => draw_tool_view(term, &mut app, &mut transcript)?,
                    View::ResumePicker => draw_resume_picker(term, &app)?,
                    View::ContextDebug => draw_context_view(term, &mut app)?,
                }
                // The ↓ manager band re-arms frames like an active turn: its
                // details view's Runtime ticks with no events otherwise — and
                // so does a non-empty agent roster (its elapsed counters tick,
                // and the linger sweep above needs the frames to fire).
                if app.turn_active()
                    || app.background_view.is_some()
                    || !app.agents().is_empty()
                {
                    frame.schedule_frame_in(STATUS_FRAME_INTERVAL);
                }
            }

            // 4. A file-search result from the worker. Feed it into the open `@`
            //    picker (stale results — the token moved on — are dropped by
            //    `set_file_matches`), then repaint. See docs/file-search.md.
            Some(result) = file_rx.recv() => {
                app.set_file_matches(&result.query, result.matches);
                frame.schedule_frame();
            }

            // 5. A finished Ctrl+V clipboard read from its worker thread:
            //    attach the temp image to the composer, or surface the red
            //    failure notice. Committing is view-gated (invariant 4) — the
            //    result may arrive with the Ctrl+O overlay up, where the
            //    notice is recorded only and the return repaints it from
            //    history. See docs/image-paste.md.
            Some(result) = img_rx.recv() => {
                match result {
                    Ok(path) => {
                        app.attach_image(path);
                        // A known non-vision model can't see the paste: warn
                        // at once with a toast — the attachment still rides
                        // the request as a text note the model reads, so it
                        // can tell the user too (docs/tools.md).
                        if active_vision == Some(false) {
                            present_toast(
                                &mut app,
                                &mut toast_deadline,
                                &frame,
                                format!("{active_model} does not support image input"),
                                ToastKind::Error,
                            );
                        }
                    }
                    Err(reason) => {
                        let text = format!("Failed to paste image: {reason}");
                        if app.view == View::Conversation {
                            commit_error_notice(term, &mut app, &mut render, &text);
                        } else {
                            app.record_error_message(&text);
                        }
                    }
                }
                // The inserted placeholder may sit in an `@token` — re-derive
                // the picker like any other composer edit.
                dispatch_file_search(&app, &file_req_tx, &mut last_file_query);
                frame.schedule_frame();
            }

            // 6. A finished `/model` fetch from one provider's worker thread:
            //    merge its models into the open picker, or record the failure
            //    beside the providers that did load. Both are no-ops if the
            //    picker was already dismissed. See docs/llm.md.
            Some((label, result)) = model_rx.recv() => {
                match result {
                    Ok(models) => app.add_models(models),
                    Err(reason) => app.add_model_error(label, reason),
                }
                frame.schedule_frame();
            }

            // 6b. The startup capability probe answered (docs/reasoning.md,
            //     docs/tools.md): find the active model's record, seed the
            //     Shift+Tab cycle at its default mode AND the image-input
            //     gate, rebind the next turn's backend so both ride its
            //     requests, and persist — so later startups seed from the
            //     file instead of probing. Guarded so a `/model` switch that
            //     raced the probe (and already knows its support first-hand)
            //     wins; a failed fetch just leaves the support unknown (no
            //     toast — this is background bookkeeping the user never
            //     asked for).
            Some((provider, result)) = probe_rx.recv(), if thinking_probe_pending => {
                thinking_probe_pending = false;
                if let Ok(models) = result
                    && active_provider.as_deref() == Some(provider.as_str())
                {
                    let entry = models.iter().find(|m| m.id == active_model);
                    let thinking = entry
                        .and_then(|entry| entry.reasoning.clone())
                        .map(|support| {
                            let mode = support.default_mode();
                            (support, mode)
                        });
                    let vision = entry.and_then(|entry| entry.vision);
                    // Rebind when something actually changes a request: a
                    // thinking mode to ride it, or a known-blind model whose
                    // attachments must degrade (Some(true)/None both attach —
                    // nothing to rebind for).
                    if (thinking.is_some() || vision == Some(false))
                        && let Some(cfg) = model_config_for(
                            &providers, &env_file, &provider, &active_model, temperature,
                            thinking.as_ref().map(|(_, mode)| *mode), vision,
                        )
                        && cfg.is_usable()
                    {
                        backend = Box::new(
                            LlmBackend::with_system_prompt(cfg, system_prompt.clone())
                                .with_background(registry.clone()),
                        );
                    }
                    app.set_thinking(thinking.clone());
                    active_vision = vision;
                    // The probed record's context window seeds the footer
                    // gauge + auto-compact (the env override still wins) —
                    // docs/compact.md.
                    active_context = entry.and_then(|entry| entry.context);
                    app.set_context_window(env_context_window.or(active_context));
                    // Persist only onto the recorded selection: an
                    // env-overridden model never writes config.json (env
                    // always wins, never sticks), so that combination just
                    // re-probes next run.
                    if persisted_selection.as_ref()
                        == Some(&(provider.clone(), active_model.clone()))
                    {
                        save_settings(
                            settings_path.as_deref(),
                            &provider,
                            &active_model,
                            Some(thinking_settings_of(thinking.as_ref())),
                            vision,
                            active_context,
                        );
                    }
                    frame.schedule_frame();
                }
            }

            // 7. A background-shell event from the registry's monitors
            //    (docs/background.md). Its channel is never swapped (unlike
            //    the reply channel), so shells survive interrupts and /clear
            //    kills them explicitly. An exit posts its model-facing note
            //    onto the registry's board at once (the in-flight agent takes
            //    it before its next round, so a mid-turn kill is known to the
            //    model within the same turn) and defers the TUI notice to the
            //    next safe boundary — a tool resolution mid-turn, or here and
            //    now while idle, where a note still on the board (no agent
            //    read it) starts the automatic follow-up turn for a
            //    model-launched shell.
            Some(bg_event) = bg_rx.recv() => {
                match bg_event {
                    BgEvent::Started { id, command, description, from_model } => {
                        bg_clocks.insert(id.clone(), Instant::now());
                        app.bg_started(&id, &command, description, from_model);
                    }
                    BgEvent::Output { id, chunk } => app.bg_output(&id, &chunk),
                    BgEvent::Exited { id, code, killed } => {
                        bg_clocks.remove(&id);
                        if let Some(completion) = app.bg_exited(&id, code, killed) {
                            registry.post_notice(
                                completion.context_text(),
                                completion.from_model,
                            );
                            app.defer_bg_completion(completion);
                            if !app.turn_active() {
                                inflight = dispatch_after_turn(
                                    term, &mut app, &tx, backend.as_ref(), &registry,
                                    &mut render, &mut clocks,
                                    &checkpoints, &mut recorder,
                                )?;
                            }
                        }
                    }
                }
                frame.schedule_frame();
            }

            // 8. A subagent event (docs/agent-tool.md): fold it into the
            //    roster entry — the footer list, the live group cell, and the
            //    Ctrl+O cells all render from there — committing incrementally
            //    when the user is inside that agent's session view. A settled
            //    **background** agent posts its model-facing note on the
            //    board (the in-flight main turn hears it at its next round)
            //    and defers its notice cell; with nothing in flight the
            //    boundary dispatch settles it at once and starts the
            //    automatic follow-up turn (the background-shell pattern).
            Some(AgentEvent::Stream { id, event }) = agent_rx.recv() => {
                on_agent_event(
                    term, &mut app, &mut agent_render, &registry, &agent_registry,
                    &mut agent_clocks, &mut agent_expiry, &id, event,
                )?;
                if !app.turn_active() && app.has_pending_agent_notices() {
                    inflight = dispatch_after_turn(
                        term, &mut app, &tx, backend.as_ref(), &registry,
                        &mut render, &mut clocks,
                        &checkpoints, &mut recorder,
                    )?;
                }
                frame.schedule_frame();
            }
        }

        // Auto-compact (docs/compact.md): past codex's 90%-of-window
        // threshold, start the summarization turn on our own at this idle
        // boundary — the loop bottom sees every turn end and gauge change.
        // `should_auto_compact` pre-checks are cheap (the context derivation
        // runs only once every gate has passed), it can't fire mid-turn, and
        // one attempt per user turn means an Esc'd or failed compaction never
        // loops. The StallAi test backend opts out (it ignores the script).
        if inflight.is_none() && stall_ms.is_none() && app.should_auto_compact() {
            let compact_backend = build_compact_backend(
                &providers,
                &env_file,
                active_provider.as_deref(),
                &active_model,
                temperature,
                app.thinking.as_ref().map(|t| t.mode),
                active_vision,
                system_prompt.clone(),
            );
            inflight = Some(start_compact_turn(
                &mut app,
                &tx,
                backend.as_ref(),
                compact_backend.as_ref(),
                &mut render,
                &mut clocks,
                /*auto=*/ true,
            ));
            frame.schedule_frame();
        }
        // Release any permission request dropped without an answer (Esc,
        // `/clear`): the tool thread parked on it would otherwise wait for a
        // decision that is never coming — a cancelled turn's reaps itself, but
        // a background agent's has nothing to cancel it (docs/permissions.md).
        if let Some(gate) = permissions.as_ref() {
            for id in app.take_abandoned_permissions() {
                gate.resolve(&id, PermissionDecision::Deny(None));
            }
        }
        // Mirror the finished history to the session file (docs/resume.md):
        // append what this iteration added, rewrite on a backtrack truncation,
        // nothing when unchanged — so streaming chunks (which never touch
        // history) cost no I/O.
        recorder.sync(&app.history);
        // Flush any inputs recorded this iteration to the persistent history
        // file (docs/history-persistence.md) — the drain is empty on iterations
        // that recorded nothing, so streaming ticks cost no I/O.
        hist_store.append(&app.take_unpersisted_inputs());
        // Pre-render whatever this iteration committed into the Ctrl+O
        // transcript cache (docs/tool-view-performance.md) — a few integer
        // compares when nothing did, one grammar-highlight per new item when
        // something did, the whole loaded history on the iteration a `/resume`
        // swapped it in. Paying it here, at the boundary, is what makes the
        // Ctrl+O keypress itself O(live tail): the overlay never opens cold.
        transcript.warm(&app, term.screen().width);
        // Reap detached backend threads (interrupt / `/clear` abandonments) that
        // have finished. `is_finished()` never blocks, so this can't stall the
        // loop; a thread still parked in its final network read is left until it
        // exits on its own. See `abandon_inflight` / docs/interrupt.md.
        reaping.retain(|handle| !handle.is_finished());
    }

    // The quit arms break before the loop-bottom sync — catch the last change.
    recorder.sync(&app.history);
    hist_store.append(&app.take_unpersisted_inputs());
    // Stop any in-flight reply on the way out, but don't `join()` it: joining
    // would couple the terminal restore to the backend's worst case (the
    // interrupt-lag freeze, on the quit path). The process exits right after
    // `term.restore()`, reaping any detached thread — the backend's and its
    // transport thread alike.
    if let Some((cancel, handle)) = inflight.take() {
        cancel.cancel();
        // A `!` shell turn's child is a separate PROCESS: the runner thread
        // dies with this process before its 20ms cancel poll can run, so the
        // reparented `sh -c` child would outlive the TUI (Esc and /clear kill
        // it only because the app stays alive long enough for the poll). Give
        // the runner a bounded window to observe the cancel and kill/reap the
        // child. A backend network thread is still never joined — the wait
        // applies only to the local shell runner, and it is bounded so a
        // wedged kill can't stall the quit.
        if app.status().is_some_and(|status| status.shell) {
            let deadline = Instant::now() + SHELL_QUIT_KILL_WINDOW;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(SHELL_POLL_INTERVAL / 2);
            }
        }
    }
    // Cancel a dangling /model fetch (its detached worker exits on the cancel).
    if let Some(cancel) = model_fetch_cancel.take() {
        cancel.cancel();
    }
    // Kill every background shell on the way out — the sweep is synchronous
    // (direct process-group kills), so quitting can't orphan a `ping`
    // (docs/background.md) — and cancel every subagent (their threads observe
    // the token and die with the process either way; the cancel stops their
    // in-flight requests promptly — docs/agent-tool.md).
    registry.kill_all();
    agent_registry.kill_all();
    Ok(())
}

/// Is the built-in dummy backend forced on? (`ALTER_ZERO_DUMMY` set to a truthy
/// value). Keeps `smoke.sh` — which sets nothing — on the dummy, and lets a
/// developer force it even with a key configured. See `docs/llm.md`.
fn dummy_forced() -> bool {
    std::env::var("ALTER_ZERO_DUMMY").ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Load the provider table: `ALTER_ZERO_PROVIDERS_FILE`, then `./providers.toml`,
/// then `~/.alter-zero/providers.toml`, else the built-in default. The first that
/// reads and parses wins; a malformed file falls through to the next. Boundary
/// code — env + filesystem. See `docs/llm.md`.
fn load_providers() -> ProvidersFile {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("ALTER_ZERO_PROVIDERS_FILE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from("providers.toml"));
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".alter-zero/providers.toml"));
    }
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(file) = ProvidersFile::parse(&text)
        {
            return file;
        }
    }
    ProvidersFile::builtin()
}

/// The environment variable a provider's API key is read from (its `api_key_env`
/// or the sanitized `<ID>_API_KEY` default), for both the key lookup and the
/// "set X" hint.
fn key_env_name(providers: &ProvidersFile, provider: &str) -> String {
    providers.get(provider).map_or_else(
        || alter_zero::llm::config::default_key_env(provider),
        |p| p.key_env(provider),
    )
}

/// A value from the real process environment (which wins, dotenv-style) or the
/// loaded `.env` store, ignoring empty values.
fn resolve_env(env_file: &EnvFile, name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            env_file
                .get(name)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
}

/// Resolve a provider's API key: its own env var (process env then `.env`), else
/// the generic `ALTER_ZERO_API_KEY`. Empty values count as unset.
fn resolve_api_key(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
) -> Option<String> {
    resolve_env(env_file, &key_env_name(providers, provider))
        .or_else(|| resolve_env(env_file, "ALTER_ZERO_API_KEY"))
}

/// Build the resolved [`ModelConfig`] for a provider/model (with the resolved
/// key + temperature + thinking mode + vision support merged in), or `None`
/// when the provider isn't in the file. `thinking` is the mode riding the
/// request payload — `None` for a model with no reasoning (or a fetch that
/// doesn't care, like the `/models` listing). See `docs/reasoning.md`.
/// `vision` is the model's known image-input support — `Some(false)` makes
/// the backend degrade attachments instead of letting the provider fail the
/// turn; `None` = unknown, attach optimistically. See `docs/tools.md`.
fn model_config_for(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
    model: &str,
    temperature: Option<f32>,
    thinking: Option<ThinkingMode>,
    vision: Option<bool>,
) -> Option<ModelConfig> {
    let sel = Selection {
        provider_id: provider.to_string(),
        model: model.to_string(),
        api_key: resolve_api_key(providers, env_file, provider),
        temperature,
        thinking,
        vision,
        cache_key: Some(session_cache_key().to_string()),
    };
    providers.model_config(&sel)
}

/// The per-session cache-affinity key every backend build shares, minted once
/// per process (pid + startup time — unique enough for a routing hint whose
/// caches live minutes). Sent as `prompt_cache_key` (and OpenRouter's
/// `session_id`) so this session's requests keep landing on the same
/// provider/server and hitting its warm prompt cache; a fresh key on the next
/// run just means one cold request. Boundary code — the time read stays out
/// of the pure core (see `docs/prompt-caching.md`).
fn session_cache_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("alter-zero-{}-{now}", std::process::id())
    })
}

/// The provider rows the `/login` flow shows: every provider in the file, tagged
/// with its key env var and whether a key already resolves (the ✓). See
/// `docs/llm.md`.
fn provider_choices(providers: &ProvidersFile, env_file: &EnvFile) -> Vec<ProviderChoice> {
    providers
        .ids()
        .into_iter()
        .map(|id| {
            let name = providers
                .get(&id)
                .map_or_else(|| id.clone(), |p| p.name.clone());
            let env_var = key_env_name(providers, &id);
            let configured = resolve_api_key(providers, env_file, &id).is_some();
            ProviderChoice {
                id,
                name,
                env_var,
                configured,
            }
        })
        .collect()
}

/// The app's config home — where the `.env` key store and `config.json` live:
/// `ALTER_ZERO_CONFIG_DIR`, else `~/.alter-zero`, else `None` (no HOME and no
/// override, so file persistence is disabled). Matches where `providers.toml`
/// and the sessions dir already resolve. See `docs/llm.md`.
fn config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALTER_ZERO_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".alter-zero"))
}

/// The `.env` key store path: `ALTER_ZERO_ENV_FILE`, else `{config_home}/.env`,
/// else `./.env` when there's no config home. Written by the `/login` flow.
/// Write the `.env` key store **owner-only**: the file holds plaintext API
/// keys, so it is created `0o600` — and a pre-existing file's mode is
/// tightened, since `mode()` only applies at creation — matching the
/// credential-file convention of gh/codex/Claude Code. On non-unix the plain
/// write applies.
fn write_key_store(path: &std::path::Path, contents: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

fn env_file_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ALTER_ZERO_ENV_FILE") {
        return PathBuf::from(path);
    }
    config_home().map_or_else(|| PathBuf::from(".env"), |dir| dir.join(".env"))
}

/// Load the `.env` key store; an absent or unreadable file yields an empty one.
fn load_env_file(path: &Path) -> EnvFile {
    std::fs::read_to_string(path)
        .map(|text| EnvFile::parse(&text))
        .unwrap_or_default()
}

/// The persisted-settings path (`{config_home}/config.json`), or `None` when
/// there's no config home — persistence is then disabled. See `docs/llm.md`.
fn settings_file_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("config.json"))
}

/// The checkpoints root — `ALTER_ZERO_CHECKPOINTS_DIR` (the smoke test points
/// it at a temp dir, the `ALTER_ZERO_SESSIONS_DIR` pattern), else
/// `~/.alter-zero/checkpoints`. `None` (no HOME and no override) disables
/// checkpoints. Each working directory gets one isolated store under this root
/// ([`checkpoint::store_git_dir`]). See `docs/checkpoint.md`.
fn checkpoints_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALTER_ZERO_CHECKPOINTS_DIR") {
        return Some(PathBuf::from(dir));
    }
    config_home().map(|dir| dir.join("checkpoints"))
}

/// Load the persisted `/model` selection; an absent, unreadable, or corrupt
/// file yields the default (all-unset) settings.
fn load_settings(path: Option<&Path>) -> Settings {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

/// Persist the chosen provider/model — plus the model's reasoning state, so
/// the Shift+Tab cycle needs no refetch next run (`docs/reasoning.md`), and
/// its image-input support, so the attachment gate needs no re-probe
/// (`docs/tools.md`) — to `config.json`, creating the config home first.
/// Best-effort — a write failure is swallowed (like the session recorder) so
/// it can never kill the TUI; a `None` path (no config home) no-ops. See
/// `docs/llm.md`.
fn save_settings(
    path: Option<&Path>,
    provider: &str,
    model: &str,
    thinking: Option<ThinkingSettings>,
    vision: Option<bool>,
    context: Option<u64>,
) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        Settings::for_selection(provider, model)
            .with_thinking(thinking)
            .with_vision(vision)
            .with_context(context)
            .to_json(),
    );
}

/// The [`ThinkingSettings`] blob recording a *definitively known* reasoning
/// state — a real state, or the "no thinking" marker for a model whose record
/// said so (so startup doesn't re-probe it). Use only when the support is
/// known; an unknown (probe pending/failed) should persist `None` instead.
fn thinking_settings_of(thinking: Option<&(ReasoningSupport, ThinkingMode)>) -> ThinkingSettings {
    match thinking {
        Some((support, mode)) => ThinkingSettings::from_state(support, *mode),
        None => ThinkingSettings::unsupported(),
    }
}

/// Build the one-off **tools-free** backend a `/compact` turn runs on (codex
/// sends the summarize request with no tools): the same persona + environment
/// prompt via `LlmBackend::configure(cfg, prompt, false)`, no
/// `.with_background` (no notice injection into the summary request). `None`
/// when no real backend is configured — the caller falls back to the session
/// backend (the dummy scripts a text-only canned summary). See
/// `docs/compact.md`.
#[allow(clippy::too_many_arguments)] // the same flat knob list as build_backend
fn build_compact_backend(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: Option<&str>,
    model: &str,
    temperature: Option<f32>,
    thinking: Option<ThinkingMode>,
    vision: Option<bool>,
    system_prompt: Option<String>,
) -> Option<LlmBackend> {
    if dummy_forced() {
        return None;
    }
    provider
        .and_then(|provider| {
            model_config_for(
                providers,
                env_file,
                provider,
                model,
                temperature,
                thinking,
                vision,
            )
        })
        .filter(llm::ModelConfig::is_usable)
        .map(|cfg| LlmBackend::configure(cfg, system_prompt, /*tools_enabled=*/ false))
}

/// Start a `/compact` summarization turn (docs/compact.md) — manual
/// (`Action::Compact`) or auto-triggered (the loop bottom's
/// `should_auto_compact`). Like [`start_background_turn`], no user message is
/// recorded: the context is derived as-is and codex's summarization prompt
/// rides as its final user entry (the real backend ignores the bare `prompt`
/// whenever the context is non-empty; the prompt argument still serves the
/// dummy, which scripts a text-only canned summary for it). Spawns on the
/// tools-free one-off backend when one resolved, else the session backend.
fn start_compact_turn(
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    fallback: &dyn ReplySource,
    compact_backend: Option<&LlmBackend>,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
    auto: bool,
) -> (CancelToken, JoinHandle<()>) {
    app.begin_compact(auto);
    app.count_user_input(context::SUMMARIZATION_PROMPT);
    render.reset();
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    clocks.command_start = None;
    let cancel = CancelToken::new();
    // The summarizer reads the same window the model does — the AGENTS.md
    // instructions in front (codex's compact request keeps its initial
    // context too). See docs/project-doc.md.
    let mut compact_context =
        context::context_messages_with(app.user_instructions.as_deref(), &app.history);
    compact_context.push(context::ContextMessage::new(
        context::ContextRole::User,
        context::SUMMARIZATION_PROMPT,
    ));
    let spawn_on: &dyn ReplySource = match compact_backend {
        Some(one_off) => one_off,
        None => fallback,
    };
    let handle = spawn_on.spawn(
        context::SUMMARIZATION_PROMPT.to_string(),
        Vec::new(),
        compact_context,
        tx.clone(),
        cancel.clone(),
    );
    (cancel, handle)
}

/// Pick the reply backend: the dummy unless a real provider/model/key all
/// resolve (and `ALTER_ZERO_DUMMY` isn't forcing the dummy). The dummy is the
/// safe fallback so the app always runs offline. See `docs/llm.md`.
#[allow(clippy::too_many_arguments)] // a flat list of independent config knobs
/// Does this session ask before a `write`/`edit`/`bash` runs? On by default;
/// disabled by a falsy `ALTER_ZERO_PERMISSIONS` (`0`/`false`/`no`/`off`), which
/// starts the session with no gate so every tool runs unasked — the pre-feature
/// behaviour. See `docs/permissions.md`.
fn permissions_enabled() -> bool {
    match std::env::var("ALTER_ZERO_PERMISSIONS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

// The full set of knobs a backend needs; splitting them into a struct would
// only add a shape the call sites have to build (the `live_height` precedent).
#[allow(clippy::too_many_arguments)]
fn build_backend(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: Option<&str>,
    model: Option<&str>,
    temperature: Option<f32>,
    thinking: Option<ThinkingMode>,
    vision: Option<bool>,
    system_prompt: Option<String>,
    startup_delay: Duration,
    registry: &BackgroundRegistry,
    agents: &AgentRegistry,
    permissions: Option<&PermissionGate>,
) -> Box<dyn ReplySource> {
    if !dummy_forced()
        && let (Some(provider), Some(model)) = (provider, model)
        && let Some(cfg) = model_config_for(
            providers,
            env_file,
            provider,
            model,
            temperature,
            thinking,
            vision,
        )
        && cfg.is_usable()
    {
        let mut backend = LlmBackend::with_system_prompt(cfg, system_prompt)
            .with_background(registry.clone())
            .with_agents(agents.clone());
        // The tool-permission gate (docs/permissions.md) — absent when
        // `ALTER_ZERO_PERMISSIONS` is falsy, and every tool then runs unasked.
        if let Some(gate) = permissions {
            backend = backend.with_permissions(gate.clone());
        }
        return Box::new(backend);
    }
    let dummy = DummyAi::with_startup_delay(startup_delay);
    Box::new(match permissions {
        Some(gate) => dummy.with_permissions(gate.clone()),
        None => dummy,
    })
}

/// Fetch one provider's `/models` list on a background thread (the image-paste
/// pattern), sending the labelled result to the loop. One of these is spawned
/// per configured provider so they fetch in parallel; a cancelled fetch (the
/// picker closed) is dropped. See `docs/llm.md`.
fn spawn_model_fetch(
    label: String,
    cfg: Option<ModelConfig>,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<ModelFetch>,
) {
    std::thread::spawn(move || {
        let result: Result<Vec<ModelEntry>, String> = match cfg {
            Some(cfg) => llm::models::fetch_models(&cfg, &cancel).map_err(|e| e.to_string()),
            None => Err("No provider configured — set providers.toml / ALTER_ZERO_PROVIDER".into()),
        };
        // Don't deliver a result the picker no longer wants.
        if !cancel.is_cancelled() {
            let _ = tx.send((label, result));
        }
    });
}

/// The live status indicator's clocks, bundled (the impurity kept at the
/// boundary): when the turn was submitted — driving the elapsed timer and the
/// verb's shimmer phase — and when the current thinking phase began (`None`
/// outside one). Reset at each turn start; the pure `App` only ever sees the
/// *computed* durations, via `set_status_times`. See docs/status-indicator.md.
struct StatusClocks {
    turn_start: Option<Instant>,
    thinking_start: Option<Instant>,
    /// When the **current running command** (a model `bash` call, set at its
    /// `ToolStart`; or a `!` shell run, set in `run_shell`) began — cleared
    /// when it resolves (`ToolEnd`/`ToolBackgrounded`) and at each turn start.
    /// Drives the delayed `(ctrl+b to run in background)` hint via
    /// `App::set_command_elapsed` (docs/background.md).
    command_start: Option<Instant>,
    /// When the event loop started — the epoch of the **animation phase** every
    /// pulsing bullet breathes against (`App::set_pulse`, docs/tool-pulse.md).
    /// Unlike the others this is never cleared: one monotonic clock, so a
    /// round's tool cells and its agent tree stay in step, and a background
    /// agent's live cell keeps animating between turns.
    loop_start: Instant,
}

impl StatusClocks {
    /// Idle clocks whose animation epoch is **now** — built once, at the top of
    /// the loop. No `Default`: `loop_start` is a real reading, and stamping it
    /// implicitly would let a stray `default()` silently restart every pulse.
    fn started_now() -> Self {
        Self {
            turn_start: None,
            thinking_start: None,
            command_start: None,
            loop_start: Instant::now(),
        }
    }
}

/// Stop tracking the in-flight turn **without blocking the event loop**, and
/// return a fresh reply channel to replace the old one.
///
/// The backend observes cancellation *cooperatively*, but a real network
/// backend can be parked in a blocking read (waiting for the response headers
/// or the first SSE byte) for up to one op-timeout before it notices — so
/// `join`ing the thread here would freeze the whole UI (spinner, timer,
/// composer) for that long: the interrupt-lag bug. Instead we **detach** the
/// thread (it exits on its own once its read returns and it re-checks the
/// token) and mint a **fresh** channel. Any last event the dying thread emits
/// goes to its old sender, whose receiver the caller is about to drop, so it
/// can never leak into the next turn (which spawns on the new sender). This
/// replaces the old `cancel + join + drain` teardown wholesale — the channel
/// swap is both the "thread stopped sending" guarantee *and* the drain. The
/// detached handle is parked in `reaping`, swept when finished (invariant: a
/// cancelled `LlmBackend` / `DummyAi` / `StallAi` / shell runner streams
/// nothing further and returns within one op-timeout). See `docs/interrupt.md`.
fn abandon_inflight(
    inflight: Option<(CancelToken, JoinHandle<()>)>,
    reaping: &mut Vec<JoinHandle<()>>,
) -> (
    tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
) {
    if let Some((cancel, handle)) = inflight {
        cancel.cancel();
        reaping.push(handle);
    }
    tokio::sync::mpsc::unbounded_channel()
}

/// One turn's user input handed to [`start_turn`]: the text message(s) — a
/// Submit is a batch of one, a queue flush a batch — plus any Ctrl+V-attached
/// images as their `(placeholder, path)` pairs (codex's `UserMessage`'s
/// `text` + `local_images`): the names let recording match each path to the
/// message whose text carries its placeholder. See `docs/image-paste.md`.
struct TurnInput {
    texts: Vec<String>,
    images: Vec<(String, PathBuf)>,
}

/// Start one turn for `texts` (a Submit is a batch of one; a queue flush sends
/// one batch — the Enter messages sharing that turn — as a single turn, the
/// Tab-opened batches flushing across later turns): record + commit each user
/// bullet to scrollback, open the stream, reset the per-turn clocks and commit
/// counter, and spawn the backend on the joined prompt. Returns the in-flight
/// cancel token + thread handle. Shared by the `Submit` key arm *and* every
/// queue flush (`StreamDone`/`Error` — under the Ctrl+O overlay too — and an
/// Esc interrupt), so the paths can never drift. Empty batches are the
/// caller's job to skip. See `docs/queue.md`.
fn start_turn(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    backend: &dyn ReplySource,
    input: TurnInput,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
) -> io::Result<(CancelToken, JoinHandle<()>)> {
    let TurnInput { texts, images } = input;
    let width = term.screen().width;
    // Each path is recorded onto the message whose text carries its
    // placeholder, in text-occurrence order — a merged batch's duplicate
    // `[Image #1]`s resolve to their own drafts' paths, and the
    // undo/backtrack re-key zips occurrences straight over the recorded
    // order (docs/image-paste.md).
    let image_count = images.len();
    let paths: Vec<PathBuf> = images.iter().map(|(_, path)| path.clone()).collect();
    let mut per_text = paste::distribute_images(&texts, images);
    // Committing is suppressed while an agent session view covers the screen
    // (a queued main turn can dispatch there) — the return's purge-rebuild
    // regenerates the bubbles from history (docs/agent-tool.md). Under the
    // Ctrl+O overlay the inserts merely queue and the return's reflow drops +
    // regenerates them, as before.
    let committing = app.agent_view.is_none();
    for (text, attached) in texts.iter().zip(&mut per_text) {
        app.record_user_message_with_images(text, std::mem::take(attached));
        if committing {
            term.insert_before(ui::message_lines(Role::User, text, width));
            term.insert_before(vec![Line::default()]);
        }
    }
    app.begin_stream();
    // Count the user's uploaded input into the tally (arrow ↑) so the status
    // shows `↑ N tokens` during the backend's pre-stream pause, before its
    // first chunk flips the arrow back to ↓. Any Ctrl+V-attached images add to
    // the same ↑ tally (docs/image-paste.md).
    let prompt = texts.join("\n");
    app.count_user_input(&prompt);
    app.count_input_images(image_count);
    render.reset();
    // Start the turn clock; the draw branch keeps the status animated from here.
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    // No command runs yet — a model `bash` call starts its own hint clock at
    // its ToolStart (docs/background.md).
    clocks.command_start = None;
    let cancel = CancelToken::new();
    // Refresh the project's AGENTS.md instructions right before the context
    // derives (docs/project-doc.md): the turn that just ended may have
    // written the guide (`/init`'s whole point), and this turn must already
    // carry it. The TUI process never chdirs, so this is run()'s cwd; on the
    // odd read failure the startup seed stands.
    if let Ok(cwd) = std::env::current_dir() {
        app.set_user_instructions(project_doc::load_user_instructions(&cwd));
    }
    // The whole conversation — the just-recorded user message included — rides
    // the request so a real model keeps its context across turns (the
    // AGENTS.md instructions in front); the image paths also travel the
    // original typed channel (codex's `UserInput::LocalImage`). See
    // docs/context.md.
    let context = context::context_messages_with(app.user_instructions.as_deref(), &app.history);
    let handle = backend.spawn(prompt, paths, context, tx.clone(), cancel.clone());
    Ok((cancel, handle))
}

/// Commit a one-off notice to scrollback + history, finalising any in-flight
/// streamed segment first so the notice slots in order — the same flush trick
/// a tool call uses. The shared body of [`commit_error_notice`] and
/// [`commit_system_notice`] (they differ only in role and recorder). Only
/// reached in the conversation view, so it never writes the alternate screen.
fn commit_notice(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    role: Role,
    record: fn(&mut App, &str),
    text: &str,
) {
    let width = term.screen().width;
    let committing = app.agent_view.is_none();
    if let Some(segment) = app.flush_streaming_segment()
        && committing
    {
        term.insert_before(render.finish(&segment, width));
        term.insert_before(vec![Line::default()]);
        render.reset();
    }
    record(app, text);
    if committing {
        term.insert_before(ui::message_lines(role, text, width));
        term.insert_before(vec![Line::default()]);
    }
}

/// Commit a red error notice (a Ctrl+V clipboard failure, a `/copy` error) —
/// [`commit_notice`] as [`Role::Error`]. See `docs/image-paste.md`.
fn commit_error_notice(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    text: &str,
) {
    commit_notice(
        term,
        app,
        render,
        Role::Error,
        App::record_error_message,
        text,
    );
}

/// Commit a cyan system notice — `/help`'s command list, `/copy`'s
/// confirmation — [`commit_notice`] as [`Role::System`].
fn commit_system_notice(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    text: &str,
) {
    commit_notice(
        term,
        app,
        render,
        Role::System,
        App::record_system_message,
        text,
    );
}

/// Commit a dead turn's remains to scrollback — the kept partial reply, the
/// tool the death resolved as failed, then the red notice, each with a
/// trailing blank spacer — after reseating the viewport to its idle height
/// (the StreamDone dance, so the box stays flush at the bottom as the
/// streaming strip clears). The shape shared by the Esc interrupt
/// ([`App::interrupt_turn`]) and a backend [`StreamEvent::Error`]
/// ([`App::fail_stream`]); the caller guarantees the conversation view.
///
/// `notice` is `None` for a `!` shell interrupt, whose `⎿ Interrupted by user`
/// cell (the `tool`) already says it — committing a second `Conversation
/// interrupted` line would be redundant (docs/interrupt.md, req 2). A backend
/// error always passes `Some` (the error text is its terminal notice).
fn commit_turn_failure(
    term: &mut InlineViewport,
    app: &App,
    render: &mut ui::StreamRender,
    partial: Option<String>,
    tool: Option<alter_zero::app::ToolCall>,
    agents: Option<alter_zero::app::AgentGroup>,
    notice: Option<&str>,
) {
    let width = term.screen().width;
    term.set_view_height(live_region_height(app, term.screen()));
    if let Some(partial) = partial {
        term.insert_before(render.finish(&partial, width));
        term.insert_before(vec![Line::default()]);
    }
    if let Some(tool) = tool {
        term.insert_before(ui::tool_lines(&tool, width));
        term.insert_before(vec![Line::default()]);
    }
    // The agent group the death resolved (its members marked interrupted) —
    // the red tree cell, before the notice (docs/agent-tool.md).
    if let Some(group) = agents {
        term.insert_before(ui::agent_group_lines(&group, width));
        term.insert_before(vec![Line::default()]);
    }
    if let Some(notice) = notice {
        term.insert_before(ui::message_lines(Role::Error, notice, width));
        term.insert_before(vec![Line::default()]);
    }
}

/// Run a `!command` locally as a turn (the [`Action::RunShell`] arm; see
/// `docs/shell-command.md`). Echoes `❯ !command` to scrollback + history,
/// calls [`App::begin_shell`] (which sets up the status strip with the command
/// as its running tool), then spawns [`spawn_shell_command`] on the same
/// streamed-reply channel — so the existing `ToolEnd`/`StreamDone` arms commit
/// the cell with **no** `Ran for Ns` summary ([`App::end_turn`] deliberately
/// returns `None` for a shell turn — the cell is the record, codex parity; see
/// `docs/shell-command.md`), and Esc routes through the normal interrupt path.
/// Returns the in-flight cancel token + thread handle, like [`start_turn`].
fn run_shell(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    command: String,
    registry: &BackgroundRegistry,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
) -> io::Result<(CancelToken, JoinHandle<()>)> {
    let width = term.screen().width;
    // begin_shell records the cell's `! command` header (Role::Shell) in
    // history; commit it with NO trailing blank — the `⎿ Running…` preview
    // (and later the committed `⎿` output) sits flush below it, forming the
    // codex-style exec cell (docs/shell-command.md). Suppressed under an
    // agent session view like every main commit (docs/agent-tool.md).
    app.begin_shell(&command);
    if app.agent_view.is_none() {
        term.insert_before(ui::message_lines(Role::Shell, &command, width));
    }
    render.reset();
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    // The `!` shell run IS the command (no separate ToolStart), so start its
    // Ctrl+B-hint clock here — a quick `!` command never flashes the hint.
    clocks.command_start = Some(Instant::now());
    let cancel = CancelToken::new();
    let handle = spawn_shell_command(command, tx.clone(), cancel.clone(), registry.clone());
    Ok((cancel, handle))
}

/// Settle the background completions held so far (empty when none landed):
/// record + commit each [`BackgroundNotice`] in arrival order — commits are
/// view-gated (invariant 4: history always records; an overlay return
/// repaints from it). Runs at every **safe boundary** — each tool
/// resolution / segment flush mid-turn, every turn end, and the idle
/// arrival — points where the streaming buffer is empty, so a notice cell
/// can never split a committed reply (invariants 2/3). See
/// `docs/background.md`.
fn settle_bg_completions(term: &mut InlineViewport, app: &mut App) {
    let committing = app.view == View::Conversation && app.agent_view.is_none();
    for completion in app.take_pending_bg_completions() {
        let notice = app.record_background_notice(&completion);
        if committing {
            let width = term.screen().width;
            // A completion can settle right after a turn whose strip just
            // collapsed — reseat the viewport like every post-stream commit
            // so the notice replaces the strip's rows in place (invariant 3).
            // Mid-turn this re-asserts the current strip-aware height (a
            // no-op sync; `paint_live` re-syncs before any pending flush).
            term.set_view_height(live_region_height(app, term.screen()));
            term.insert_before(ui::background_notice_lines(&notice, width));
            term.insert_before(vec![Line::default()]);
        }
    }
    // Background-agent completions settle at the same boundaries — the green
    // `● Agent "…" finished` / red stopped cell — and update the recorded
    // group entry so the Ctrl+O cell shows the final response
    // (docs/agent-tool.md).
    for notice in app.take_pending_agent_notices() {
        app.record_agent_notice(&notice);
        app.settle_agent_completion(&notice);
        if committing {
            let width = term.screen().width;
            term.set_view_height(live_region_height(app, term.screen()));
            term.insert_before(ui::agent_notice_lines(&notice, width));
            term.insert_before(vec![Line::default()]);
        }
    }
}

/// The every-turn-end dispatch (docs/background.md, docs/queue.md): settle
/// held background completions, then send the next queued entry — or, with
/// nothing queued and a model-launched completion **the in-flight agent
/// never heard about** (its note still on the registry's board — an agent
/// that read it mid-turn owes no follow-up), start the **automatic
/// follow-up turn** that tells the model its command finished (the notices
/// are already in history, so they ride the derived context either way — a
/// queued user batch simply carries them along with no extra request).
/// Returns the new in-flight handle, or `None` when nothing dispatched.
/// Shared by every turn-end site (`StreamDone`, `Error`, both Esc-interrupt
/// outcomes) *and* the idle completion arrival, so the paths can never
/// drift.
/// Snapshot the working directory at a turn boundary and record it against the
/// current conversation length (`docs/checkpoint.md`). Called before the next
/// queued turn dispatches, so it captures the *just-ended* turn's code state —
/// which is exactly what a later backtrack to the next message, or a resume,
/// restores. A disabled store or a failed git step is a silent no-op (the TUI
/// never dies for a checkpoint). The recorded line flushes with the loop's
/// next `recorder.sync`.
fn checkpoint_turn_end(
    checkpoints: &checkpoint::CheckpointStore,
    recorder: &mut SessionRecorder,
    app: &App,
) {
    if !checkpoints.is_enabled() {
        return;
    }
    let after = app.history.len();
    if let Some(commit) = checkpoints.snapshot(&format!("checkpoint after {after} items")) {
        recorder.record_checkpoint(checkpoint::Checkpoint { after, commit });
    }
}

#[allow(clippy::too_many_arguments)] // the turn-end plumbing, plus the checkpoint store + recorder
fn dispatch_after_turn(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    backend: &dyn ReplySource,
    registry: &BackgroundRegistry,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
    checkpoints: &checkpoint::CheckpointStore,
    recorder: &mut SessionRecorder,
) -> io::Result<Option<(CancelToken, JoinHandle<()>)>> {
    // The turn just ended and its history is settled — snapshot the code state
    // before any queued follow-up starts editing again (docs/checkpoint.md).
    checkpoint_turn_end(checkpoints, recorder, app);
    settle_bg_completions(term, app);
    let unheard = registry.take_pending_notices();
    let mut next = flush_next_queued(term, app, tx, backend, registry, render, clocks)?;
    if next.is_none() && unheard.iter().any(|note| note.from_model) {
        next = Some(start_background_turn(
            app, tx, backend, &unheard, render, clocks,
        ));
    }
    Ok(next)
}

/// Start the automatic follow-up turn for background completions the model
/// has not heard about: like [`start_turn`] but with **no new user
/// message** — the just-settled notices are the turn's cause and already
/// sit in history, so the derived context carries them (the prompt text is
/// their context form, for the empty-context fallback / the dummy). The
/// model then reports the result, exactly like the user's example
/// transcript. See `docs/background.md`.
fn start_background_turn(
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    backend: &dyn ReplySource,
    notices: &[PendingNotice],
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
) -> (CancelToken, JoinHandle<()>) {
    app.begin_stream();
    let prompt = notices
        .iter()
        .map(|note| note.context.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    app.count_user_input(&prompt);
    render.reset();
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    // A follow-up turn about background completions — text, no command yet.
    clocks.command_start = None;
    let cancel = CancelToken::new();
    let context = context::context_messages_with(app.user_instructions.as_deref(), &app.history);
    let handle = backend.spawn(prompt, Vec::new(), context, tx.clone(), cancel.clone());
    (cancel, handle)
}

/// Flush the next queued turn, if any, dispatching by its kind: a text batch
/// goes to the model ([`start_turn`]) and a `!` command runs locally
/// ([`run_shell`]) — codex's action-tagged drain (`maybe_send_next_queued_input`).
/// Returns the new in-flight handle, or `None` when the queue is empty. Shared
/// by every turn-end drain site (`StreamDone`/`Error` — in the overlay too —
/// and an Esc interrupt) so they can't drift. See `docs/queue.md`.
fn flush_next_queued(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    backend: &dyn ReplySource,
    registry: &BackgroundRegistry,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
) -> io::Result<Option<(CancelToken, JoinHandle<()>)>> {
    match app.drain_next_batch() {
        // The batch's Ctrl+V attachments dispatch with it — the whole
        // (placeholder, path) pairs, so start_turn can record each path on
        // the message carrying its placeholder (docs/image-paste.md).
        Some(QueuedTurn::Messages { texts, images }) => Ok(Some(start_turn(
            term,
            app,
            tx,
            backend,
            TurnInput { texts, images },
            render,
            clocks,
        )?)),
        Some(QueuedTurn::Shell(command)) => Ok(Some(run_shell(
            term, app, tx, command, registry, render, clocks,
        )?)),
        None => Ok(None),
    }
}

/// Run `command` under `sh -c` on a background thread, streaming the result back
/// on the reply channel as a `ToolEnd`/`StreamDone` pair (the command was
/// already shown as the running tool by [`App::begin_shell`]). Reader threads
/// drain stdout/stderr so a chatty command can't deadlock on a full pipe,
/// forwarding raw chunks the wait loop merges **in arrival order** into a
/// capped buffer (`read_capped`'s memory bound, the `llm::exec` merge shape);
/// the loop polls `cancel` so an Esc interrupt kills the child and reaps it,
/// and the registry's Ctrl+B latch so a running command can be **adopted into
/// the background** mid-run — resolving as `ToolBackgrounded` instead
/// (docs/background.md). The child leads its own process group (like the
/// model-bash executor), so kills reap backgrounded grandchildren too. A
/// non-zero exit appends `[exit status: N]` and resolves the cell red. The
/// I/O boundary — verified by `scripts/smoke.sh` (Phase 19), not unit tests.
/// See `docs/shell-command.md`.
fn spawn_shell_command(
    command: String,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancel: CancelToken,
    registry: BackgroundRegistry,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // A Ctrl+B pressed before this command started belongs to nothing.
        registry.clear_background_request();
        // Group + terminal membership (and stdio) come from
        // `subprocess::spawn_detached_shell`: its own process group so a kill
        // (Esc, quit, the registry) reaps the whole tree, and no controlling
        // terminal, so a password prompt (`! sudo …`) fails fast instead of
        // writing over the TUI (the `llm::exec` pattern; see
        // `subprocess`, docs/shell-command.md).
        let mut child = match alter_zero::subprocess::spawn_detached_shell(
            registry.detach_helper().as_deref(),
            &command,
        ) {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output: format!("failed to run command: {err}"),
                    ok: false,
                    truncated: false,
                });
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
        };

        // Drain both pipes on their own threads so a command that writes more
        // than the pipe buffer can't block (and thus never exit) while we
        // wait; the loop below *caps* what it retains so a command with huge
        // output (e.g. `tree ~/`) can't spike memory.
        let cap = SHELL_OUTPUT_MAX_BYTES;
        let (chunk_tx, chunk_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        if let Some(pipe) = child.stdout.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_shell_pipe(pipe, &tx));
        }
        if let Some(pipe) = child.stderr.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_shell_pipe(pipe, &tx));
        }
        drop(chunk_tx);

        let mut combined: Vec<u8> = Vec::new();
        let mut truncated = false;
        // Wait for the child, polling so an Esc interrupt (cancel) or a Ctrl+B
        // background request acts promptly.
        let status = loop {
            while let Ok(chunk) = chunk_rx.try_recv() {
                append_capped(&mut combined, &chunk, cap, &mut truncated);
            }
            if cancel.is_cancelled() {
                // The interrupt path (App::interrupt_turn) owns the UI from
                // here — resolve the tool failed, commit the notice. Kill the
                // whole group (a reparented grandchild would otherwise hold
                // the pipe open), send nothing, and return at once so the
                // quit path's bounded wait unblocks promptly. The detached
                // readers finish at EOF on their own.
                kill_shell_group(&mut child);
                return;
            }
            // Ctrl+B: hand the run to the background registry mid-flight —
            // it replays what we read so far and keeps streaming from our
            // pipe channel. The cell resolves as backgrounded; the shell turn
            // ends with no summary as usual (docs/background.md).
            if registry.take_background_request() {
                let task = registry.adopt(&command, None, false, child, chunk_rx, combined);
                let _ = tx.send(StreamEvent::ToolBackgrounded {
                    id: task.id.clone(),
                    output: format!(
                        "[moved to background as task {}; final output will follow when it completes]",
                        task.id
                    ),
                });
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => match chunk_rx.recv_timeout(SHELL_POLL_INTERVAL) {
                    Ok(chunk) => append_capped(&mut combined, &chunk, cap, &mut truncated),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(SHELL_POLL_INTERVAL);
                    }
                },
                Err(err) => {
                    kill_shell_group(&mut child);
                    let _ = tx.send(StreamEvent::ToolEnd {
                        output: format!("error waiting on command: {err}"),
                        ok: false,
                        truncated: false,
                    });
                    let _ = tx.send(StreamEvent::StreamDone);
                    return;
                }
            }
        };
        // Even on a clean exit, reap any process the command backgrounded —
        // it holds the pipes open, which used to delay the cell until the
        // straggler died; then absorb whatever the readers still buffered
        // (they hit EOF once the group is gone).
        kill_shell_group(&mut child);
        while let Ok(chunk) = chunk_rx.recv_timeout(SHELL_POLL_INTERVAL) {
            append_capped(&mut combined, &chunk, cap, &mut truncated);
        }

        // Lossy UTF-8: a command may emit non-UTF-8 bytes; the cap may also
        // cut a multi-byte char (→ one U+FFFD).
        let mut output = String::from_utf8_lossy(&combined).into_owned();
        let ok = status.success();
        if !ok {
            let code = status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("[exit status: {code}]"));
        }
        let _ = tx.send(StreamEvent::ToolEnd {
            output,
            ok,
            truncated,
        });
        let _ = tx.send(StreamEvent::StreamDone);
    })
}

/// Read `pipe` to EOF, forwarding raw chunks for the wait loop to merge (the
/// `llm::exec` drain shape). Stops early if the receiver hung up.
fn drain_shell_pipe(mut pipe: impl io::Read, tx: &std::sync::mpsc::Sender<Vec<u8>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Append `chunk` to the capped `buf`, marking `truncated` when the cap bites —
/// bounds peak memory regardless of how much a command emits (codex's
/// `append_capped` pattern; see `docs/shell-command.md`).
fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize, truncated: &mut bool) {
    if buf.len() < cap {
        let take = (cap - buf.len()).min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            *truncated = true;
        }
    } else if !chunk.is_empty() {
        *truncated = true;
    }
}

/// Kill a `!` command's whole process group and reap the direct child — the
/// `llm::exec::kill_process_group` pattern (the crate forbids `unsafe`, so the
/// shell's POSIX `kill` handles the negative-pgid form). Best-effort.
fn kill_shell_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id();
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// How often [`spawn_shell_command`] polls a running child for completion while
/// watching for an interrupt — short enough that Esc kills it promptly.
const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long the quit path waits for the shell runner to observe the cancel
/// and kill/reap its child before the process exits. Bounded — a wedged kill
/// can't stall the quit — and comfortably above [`SHELL_POLL_INTERVAL`] plus
/// the kill/wait syscalls.
const SHELL_QUIT_KILL_WINDOW: Duration = Duration::from_millis(250);

/// A `!` shell command retains at most this many bytes of output in memory; the
/// rest is drained and dropped (the cell appends a `…` marker). This caps peak
/// memory so a command with huge output (`tree ~/`) can't spike RSS — the
/// previous "save the full output to a file" approach still read everything into
/// memory first, which is what we're avoiding (codex caps in memory too). See
/// `docs/shell-command.md`.
const SHELL_OUTPUT_MAX_BYTES: usize = 100_000;

/// Apply one streamed reply event to `app`, committing finished lines to
/// scrollback in the conversation view. Returns whether the stream just **ended**
/// (`StreamDone`/`Error`), so the caller can clear its in-flight handle. The
/// commit work mirrors how a resize repaints the same items from history.
///
/// `clocks` holds the live-status timers: thinking events flip
/// `clocks.thinking_start`, and the turn-ending events read `clocks.turn_start`
/// for the `"Done for Ns"` summary, then clear both.
fn on_stream_event(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    clocks: &mut StatusClocks,
    agent_registry: &AgentRegistry,
    event: StreamEvent,
) -> io::Result<bool> {
    let width = term.screen().width;
    // Lines are only committed to scrollback in the conversation view; in the
    // overlay — and while an agent session view covers the screen — we hold
    // off and repaint the inline view on return, so the stream keeps
    // advancing without touching what the user is looking at.
    let committing = app.view == View::Conversation && app.agent_view.is_none();
    match event {
        StreamEvent::AgentBatch { background, agents } => {
            // The model launched a group of subagents: finalise the text
            // before them (the ToolBatch dance), settle held completions at
            // this safe boundary, then seed the roster + the live group cell.
            // No scrollback commit — the cell is live until the group
            // resolves. See docs/agent-tool.md.
            if let Some(segment) = app.flush_streaming_segment()
                && committing
            {
                term.insert_before(render.finish(&segment, width));
                term.insert_before(vec![Line::default()]);
            }
            render.reset();
            settle_bg_completions(term, app);
            app.start_agent_group(background, &agents);
            // The delayed Ctrl+B hint clock — a foreground group can be moved
            // to the background like a running command (docs/background.md).
            if !background {
                clocks.command_start = Some(Instant::now());
            }
            Ok(false)
        }
        StreamEvent::AgentGroupDone { background, agents } => {
            // The group resolved: record the tree cell and commit it — a tool
            // resolution boundary like ToolEnd (the caller drained the agent
            // channel first, so the roster snapshots are final). See
            // docs/agent-tool.md.
            let group = app.finish_agent_group(background, &agents);
            if committing {
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::agent_group_lines(&group, width));
                term.insert_before(vec![Line::default()]);
            }
            settle_bg_completions(term, app);
            clocks.command_start = None;
            Ok(false)
        }
        StreamEvent::Chunk(chunk) => {
            app.push_chunk(&chunk);
            if committing && let Some(text) = app.streaming_text() {
                term.insert_before(render.commit(text, width));
            }
            Ok(false)
        }
        StreamEvent::ToolBatch(items) => {
            // The model requested a batch of tool calls. Finalise the assistant
            // text before them (so they slot after it in scrollback), then
            // register the whole batch as `Waiting` in the live region — every
            // call shows at once, the ones not yet running as `⎿ Waiting…`. No
            // scrollback commit: the batch is live-only until each call ends. The
            // subsequent per-call ToolStart flips its front `Waiting` to
            // `Running`. See `docs/parallel-tools.md`.
            if let Some(segment) = app.flush_streaming_segment()
                && committing
            {
                term.insert_before(render.finish(&segment, width));
                term.insert_before(vec![Line::default()]);
            }
            render.reset();
            // The flush left the streaming buffer empty — a safe boundary for
            // background completions that landed while the text streamed
            // (docs/background.md), committed before the batch goes live.
            settle_bg_completions(term, app);
            app.start_tool_batch(&items);
            Ok(false)
        }
        StreamEvent::ToolStart { name, args, .. } => {
            // Finalise the current run of assistant text so the tool slots after
            // it in scrollback, then show the tool running (blue) in the live
            // region until its ToolEnd arrives. The flush always runs (it records
            // history); only the commit is view-gated. For a batched call the
            // flush already ran on ToolBatch (the buffer is empty, so this is a
            // no-op) and `start_tool` flips the front `Waiting` to `Running`; a
            // lone call (dummy/shell, no batch) pushes a fresh running call.
            if let Some(segment) = app.flush_streaming_segment()
                && committing
            {
                term.insert_before(render.finish(&segment, width));
                term.insert_before(vec![Line::default()]);
            }
            render.reset();
            // Same safe boundary as ToolBatch: the buffer is empty, so any
            // held completions commit ahead of the tool (docs/background.md).
            settle_bg_completions(term, app);
            app.start_tool(&name, &args);
            // Start this command's own clock — the delayed Ctrl+B hint waits
            // on it, so a fast command never flashes the hint (a model tool
            // that starts deep into a turn can't inherit the turn's elapsed).
            clocks.command_start = Some(Instant::now());
            Ok(false)
        }
        StreamEvent::ToolEnd {
            output,
            ok,
            truncated,
        } => {
            // A `!` output that overflowed the in-memory cap was cut by the
            // runner; mark the running tool so its expanded cell appends a `…`
            // marker (before end_tool takes it). `output` is the retained head.
            if truncated {
                app.set_tool_truncated();
            }
            // Commit the finished tool *collapsed* (green/red) to scrollback; its
            // full (retained) output lives in the Ctrl+O view.
            if let Some(tool) = app.end_tool(&output, ok)
                && committing
            {
                // A `!` shell turn's strip (preview + gap; it has no status
                // line) collapses the moment end_tool clears the running
                // tool — reseat the viewport before queueing the cell, like
                // StreamDone does, so a draw tick racing in ahead of the
                // back-to-back StreamDone can't flush against the stale
                // strip-inflated height and over-scroll the box off the
                // bottom (invariant 3).
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::tool_lines(&tool, width));
                term.insert_before(vec![Line::default()]);
            }
            // A tool resolution is a settle point: completions that landed
            // while the call ran (often a `kill` this very command issued)
            // commit right after its cell — the user's example order — not
            // at the turn's distant end (docs/background.md).
            settle_bg_completions(term, app);
            // The command resolved — stop its Ctrl+B-hint clock (the turn may
            // continue with more text/tools).
            clocks.command_start = None;
            Ok(false)
        }
        StreamEvent::ToolRejected { display, result } => {
            // The user refused the call at the permission prompt: commit the
            // red cell with the short `display` (Tab's instructions on its
            // second line) while `result` — the longer text the model read —
            // rides the recorded call so the derived context replays it on
            // every later turn (docs/permissions.md). Mirrors the ToolEnd
            // commit dance; nothing ran, so there is no truncation to mark.
            if let Some(tool) = app.reject_tool(&display, &result)
                && committing
            {
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::tool_lines(&tool, width));
                term.insert_before(vec![Line::default()]);
            }
            // A resolution boundary like ToolEnd (docs/background.md).
            settle_bg_completions(term, app);
            clocks.command_start = None;
            Ok(false)
        }
        StreamEvent::ToolBackgrounded { id: _, output } => {
            // The call resolved by moving to the background: commit its cell
            // with the fixed `⎿ Running in the background (↓ to manage)` row
            // (the stored output is the model-facing launch text). The process
            // itself now reports through the background channel. Mirrors the
            // ToolEnd commit dance (docs/background.md).
            if let Some(tool) = app.background_tool(&output)
                && committing
            {
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::tool_lines(&tool, width));
                term.insert_before(vec![Line::default()]);
            }
            // A resolution boundary like ToolEnd — completions held during
            // the launch settle here (docs/background.md).
            settle_bg_completions(term, app);
            // The command moved to the background — stop its hint clock.
            clocks.command_start = None;
            Ok(false)
        }
        StreamEvent::ToolOutput(chunk) => {
            // Live tool output: append it to the running call so the live cell
            // tails it (docs/tool-streaming.md). No scrollback commit — the tail
            // is live-only until the ToolEnd commits the finished cell; the next
            // draw tick repaints the preview with the grown output.
            app.push_tool_output(&chunk);
            Ok(false)
        }
        StreamEvent::Permission(request) => {
            // A tool is waiting on the user: raise the inline prompt (which
            // stashes the composer draft) — the backend thread is parked on the
            // gate until an `Action::ResolvePermission` answers it. Nothing is
            // committed: the prompt is live-only, and no call has started.
            // See docs/permissions.md.
            app.open_permission(request);
            Ok(false)
        }
        StreamEvent::ThinkingStart => {
            // Phase boundary: start the thinking clock so the status line
            // shows `Thinking for Ns`. No scrollback commit (thinking is live-only).
            clocks.thinking_start = Some(Instant::now());
            Ok(false)
        }
        StreamEvent::ThinkingChunk(chunk) => {
            // Reasoning delta: opaque text, counted into the token tally only —
            // never rendered, never committed.
            app.push_thinking(&chunk);
            Ok(false)
        }
        StreamEvent::ToolCallDelta(chunk) => {
            // The model is generating a tool call: opaque JSON, counted into the
            // token tally only (like reasoning) so the status ticks while it
            // generates — never rendered, never committed.
            app.push_tool_call_progress(&chunk);
            Ok(false)
        }
        StreamEvent::ThinkingEnd => {
            clocks.thinking_start = None;
            Ok(false)
        }
        StreamEvent::StreamDone => {
            // A /compact turn's end (docs/compact.md): the marker is appended
            // here (an append — the loop-bottom recorder sync writes it, the
            // checkpoint keys stay valid) and nothing streamed to the
            // transcript, so there is no final text and no Done-for-Ns
            // summary — the `● Context compacted` cell is the record.
            // dispatch_after_turn then snapshots + drains the queue like any
            // turn end, so a batch queued mid-compact goes out over the
            // freshly compacted context.
            if let Some(compaction) = app.finish_compact() {
                if committing {
                    term.set_view_height(live_region_height(app, term.screen()));
                    term.insert_before(ui::compaction_lines(&compaction, width));
                    term.insert_before(vec![Line::default()]); // blank spacer
                }
                settle_bg_completions(term, app);
                render.reset();
                clocks.turn_start = None;
                clocks.thinking_start = None;
                return Ok(true);
            }
            let final_text = app.finish_stream();
            let elapsed = clocks
                .turn_start
                .map_or(0, |start| start.elapsed().as_secs());
            // Clear the status and BUILD the summary, but don't record it yet:
            // a background completion still pending at turn end (a shell that
            // finished during this final text, with no tool call after it to
            // settle at) must land its notice ABOVE the "Done for Ns" summary
            // — the same placement a mid-turn tool boundary gives it — in both
            // history and scrollback (invariant 3). So the order is: reseat to
            // idle (the status is now cleared), commit the final reply, settle
            // the held completions, THEN record + commit the summary. The
            // common case (nothing pending) is unchanged — `settle_bg_completions`
            // is then a no-op. See `docs/background.md`.
            let summary = app.take_turn_summary(elapsed);
            if committing {
                // The reply just ended, so the streaming strip (preview + gap +
                // status, drawn *above* the box) is gone. Reseat the viewport to
                // its idle height *before* committing so the final reply line and
                // the "Done for Ns" summary replace the strip's rows in place and
                // the box stays flush at the bottom (instead of rising and leaving
                // blank rows beneath it).
                term.set_view_height(live_region_height(app, term.screen()));
                if let Some(text) = final_text {
                    term.insert_before(render.finish(&text, width));
                    term.insert_before(vec![Line::default()]); // blank spacer
                }
            }
            // Settle before recording the summary — history-gated commit inside
            // (invariant 4), so the notice sits above the summary either way.
            settle_bg_completions(term, app);
            if let Some(summary) = summary {
                app.record_turn_summary(summary.clone());
                if committing {
                    term.insert_before(ui::summary_lines(&summary, width));
                    term.insert_before(vec![Line::default()]); // blank spacer
                }
            }
            render.reset();
            clocks.turn_start = None;
            clocks.thinking_start = None;
            Ok(true)
        }
        StreamEvent::Retrying { attempt, max } => {
            // A failed request is being retried (the connection/send failed
            // before any content streamed). Show it live in the status line;
            // nothing commits to scrollback — the turn is still in flight.
            app.set_retry(attempt, max);
            Ok(false)
        }
        StreamEvent::Usage(usage) => {
            // The round's real usage frame: snap the live tally from the
            // app-side estimate to the provider's own accounting (cache
            // detail included). Live-only — the turn summary commits the
            // total at StreamDone (docs/prompt-caching.md).
            app.apply_usage(&usage);
            Ok(false)
        }
        StreamEvent::Error(message) => {
            if let Some(failure) = app.fail_stream(&message) {
                // A live agent group died with the turn: its subagent threads
                // keep running unless killed here (the backend thread that
                // owned the wait loop is gone). Idempotent for agents the
                // backend already resolved. See docs/agent-tool.md.
                if let Some(group) = &failure.agents {
                    for entry in &group.agents {
                        let _ = agent_registry.kill(&entry.id);
                    }
                }
                if committing {
                    // Flush whatever streamed before the failure, then the tool
                    // the error killed mid-run (resolved red), then the red
                    // error notice — the Esc interrupt's exact commit shape.
                    commit_turn_failure(
                        term,
                        app,
                        render,
                        failure.partial,
                        failure.tool,
                        failure.agents,
                        Some(&failure.error),
                    );
                }
            }
            render.reset();
            clocks.turn_start = None;
            clocks.thinking_start = None;
            Ok(true)
        }
    }
}

/// Fold one subagent event into its roster entry (`docs/agent-tool.md`) —
/// and, while the user is **inside that agent's session view**, commit it to
/// the screen incrementally through `agent_render`, mirroring
/// [`on_stream_event`]'s commit shape over the agent's own state. A settled
/// **background** agent posts its model-facing note on the shared board (the
/// in-flight main turn hears it at its next round — the background-shell
/// pattern) and defers its notice cell to the next safe boundary; any settle
/// arms the entry's linger sweep and stops its runtime clock.
#[allow(clippy::too_many_arguments)] // the loop's agent plumbing
fn on_agent_event(
    term: &mut InlineViewport,
    app: &mut App,
    agent_render: &mut ui::StreamRender,
    registry: &BackgroundRegistry,
    agent_registry: &AgentRegistry,
    agent_clocks: &mut HashMap<String, Instant>,
    agent_expiry: &mut HashMap<String, Instant>,
    id: &str,
    event: StreamEvent,
) -> io::Result<()> {
    let width = term.screen().width;
    // A subagent's permission request is the *user's* business, not the
    // roster's: raise the same shared prompt the main turn does, and stop —
    // the agent's own state is untouched while it waits
    // (docs/permissions.md).
    if let StreamEvent::Permission(request) = event {
        app.open_permission(request);
        return Ok(());
    }
    let viewing = app.view == View::Conversation && app.agent_view.as_deref() == Some(id);
    // Freeze the entry's runtime at its live value before a settling event
    // (the per-frame injection stops once the status is final).
    if let Some(started) = agent_clocks.get(id) {
        app.set_agent_runtime(id, started.elapsed());
    }
    // The view's segment boundaries: the agent's streamed text finalises
    // before a tool cell / the end of the run, exactly like the main loop's
    // flush points. Committed BEFORE the fold (the fold consumes the buffer).
    if viewing
        && matches!(
            event,
            StreamEvent::ToolBatch(_)
                | StreamEvent::ToolStart { .. }
                | StreamEvent::StreamDone
                | StreamEvent::Error(_)
        )
        && let Some(text) = app
            .viewed_agent()
            .and_then(|run| run.streaming.clone())
            .filter(|text| !text.is_empty())
    {
        term.insert_before(agent_render.finish(&text, width));
        term.insert_before(vec![Line::default()]);
        agent_render.reset();
    }
    let settled = app.apply_agent_event(id, &event);
    if viewing {
        match &event {
            StreamEvent::Chunk(_) => {
                if let Some(text) = app.viewed_agent().and_then(|run| run.streaming.as_deref()) {
                    let lines = agent_render.commit(text, width);
                    term.insert_before(lines);
                }
            }
            StreamEvent::ToolEnd { .. }
            | StreamEvent::ToolRejected { .. }
            | StreamEvent::ToolBackgrounded { .. } => {
                // The resolved call was pushed onto the agent's transcript —
                // commit its collapsed cell (the main ToolEnd dance).
                let tool = app.viewed_agent().and_then(|run| match run.history.last() {
                    Some(HistoryItem::Tool(tool)) => Some(tool.clone()),
                    _ => None,
                });
                if let Some(tool) = tool {
                    term.set_view_height(live_region_height(app, term.screen()));
                    term.insert_before(ui::tool_lines(&tool, width));
                    term.insert_before(vec![Line::default()]);
                }
            }
            StreamEvent::Error(message) => {
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::message_lines(Role::Error, message, width));
                term.insert_before(vec![Line::default()]);
            }
            StreamEvent::StreamDone => {
                // The strip collapses (the agent's status clears) — reseat so
                // the box stays flush (invariant 3). No summary cell: the
                // agent session keeps codex's quiet end.
                term.set_view_height(live_region_height(app, term.screen()));
            }
            _ => {}
        }
    }
    if let Some(notice) = settled {
        // A background agent completed on its own: the model-facing note
        // goes on the shared board (from_model — its untaken presence at a
        // turn boundary starts the automatic follow-up turn), the notice
        // cell defers to the next safe boundary (docs/agent-tool.md).
        registry.post_notice(notice.context_text(), true);
        app.defer_agent_notice(notice);
    }
    if app.agent(id).is_some_and(|run| run.status.is_final()) {
        agent_clocks.remove(id);
        agent_expiry.insert(id.to_string(), Instant::now() + AGENT_LINGER);
    }
    let _ = agent_registry; // the kill paths live in the action arms
    Ok(())
}

/// Rebuild the screen as an **agent session view** (`docs/agent-tool.md`):
/// purge scrollback + screen (the `/clear` shape — the main conversation
/// returns the same way), then the banner over the agent's own transcript,
/// with the live region (the agent's strip + the labelled composer + footer +
/// roster) painted below in the same synchronized frame. The in-flight
/// partial commits through `agent_render` so later chunks append seamlessly.
fn repaint_agent_view(
    term: &mut InlineViewport,
    app: &mut App,
    agent_render: &mut ui::StreamRender,
) -> io::Result<()> {
    let screen = term.screen();
    agent_render.reset();
    let Some(run) = app.viewed_agent() else {
        return Ok(());
    };
    let history = run.history.clone();
    let streaming = run.streaming.clone().filter(|text| !text.is_empty());
    let app: &App = app;
    let height = live_region_height(app, screen);
    let mut tail = ui::conversation_lines(&history, screen.width);
    if let Some(text) = &streaming {
        tail.extend(agent_render.committed_rows(text, screen.width));
    }
    let tail = ui::banner_tail(ui::header_lines(app, screen.width), tail, usize::MAX);
    term.reflow(
        tail,
        height,
        ReflowClear::Purge,
        |area, buf| ui::render_live_with_preview(area, buf, app, None),
        app,
    )?;
    Ok(())
}

/// Schedule the redraw for a just-handled key. A plain typed character that
/// lands in a [`PasteBurst`] asks for a *relaxed* frame a beat out instead of
/// an immediate one; every other key (and the first characters of a run)
/// paints at once. The scheduler keeps the soonest pending deadline and its
/// rate limiter caps everything at 120 fps — that floor, not the burst branch,
/// is what coalesces a paste run into a few paints (see `crate::paste`).
fn schedule_for_key(frame: &FrameRequester, burst: &mut PasteBurst, key: &KeyEvent) {
    let plain_char =
        matches!(key.code, KeyCode::Char(_)) && !key.modifiers.contains(KeyModifiers::CONTROL);
    if !plain_char {
        burst.reset(); // a navigation/submit key ends any burst
    }
    if plain_char && burst.note_char(Instant::now()) {
        frame.schedule_frame_in(paste::BURST_CHAR_INTERVAL);
    } else {
        frame.schedule_frame();
    }
}

/// How often the live status re-arms its next animation frame while a turn is in
/// flight (~30 fps — codex's status-widget cadence): drives the verb's shimmer
/// sweep and keeps the timer advancing through event-less pauses.
const STATUS_FRAME_INTERVAL: Duration = Duration::from_millis(32);

/// How long a transient toast stays above the box before it self-clears. See
/// docs/toast.md.
const TOAST_TTL: Duration = Duration::from_secs(4);

/// Raise a transient [`App`] toast and arm its expiry: set the text, stamp the
/// deadline `TOAST_TTL` out, and ask the frame scheduler for a draw then — so
/// it shows now (the caller schedules that frame) and self-clears later even
/// with no turn active. The draw tick clears it when due and re-arms while it
/// lingers. See docs/toast.md.
fn present_toast(
    app: &mut App,
    toast_deadline: &mut Option<Instant>,
    frame: &FrameRequester,
    text: impl Into<String>,
    kind: ToastKind,
) {
    app.show_toast(text, kind);
    *toast_deadline = Some(Instant::now() + TOAST_TTL);
    frame.schedule_frame_in(TOAST_TTL);
}

/// Write the live status's times onto `app` before a draw: how long the turn has
/// run (whole seconds for display; sub-second for the shimmer phase) and the
/// current thinking-phase duration (`Some` while thinking). Time is impure, so
/// this is the boundary's job — the pure `App`/`ui` only ever see the
/// already-computed values. No-op when no turn is in flight.
fn update_status_times(app: &mut App, clocks: &StatusClocks) {
    let elapsed = clocks
        .turn_start
        .map_or(Duration::ZERO, |start| start.elapsed());
    let thinking = clocks.thinking_start.map(|start| start.elapsed());
    app.set_status_times(elapsed, thinking);
    // The current running command's own elapsed (None when none is running),
    // gating the delayed Ctrl+B hint (docs/background.md).
    app.set_command_elapsed(clocks.command_start.map(|start| start.elapsed()));
    // The animation phase for the live region's pulsing bullets — a phase, not
    // a measurement: nothing displays it (docs/tool-pulse.md).
    app.set_pulse(clocks.loop_start.elapsed());
}

/// Local wall-clock stamp for recorded items: 12-hour time, no seconds, e.g.
/// `03:20 AM`. Injected via [`App::set_clock`] and shown **only** under the
/// user's message in the Ctrl+O transcript — the one impurity kept out of the
/// pure library.
fn local_timestamp() -> String {
    chrono::Local::now().format("%I:%M %p").to_string()
}

/// Local date for the agent's environment context — weekday plus ISO date,
/// e.g. `Sunday 2026-07-19`. Gathered here at the boundary and folded into the
/// system prompt by [`augment_with_environment`] so the agent knows the day
/// (see `docs/environment.md`).
///
/// [`augment_with_environment`]: alter_zero::llm::backend::augment_with_environment
fn local_date() -> String {
    chrono::Local::now().format("%A %Y-%m-%d").to_string()
}

/// The OS string for the agent's environment context: the platform
/// (`std::env::consts::OS`) enriched, on Linux, with the distro from
/// `/etc/os-release` — e.g. `linux (Ubuntu 24.04.4 LTS)`. Boundary code (reads
/// the file); the parse is the pure `backend::os_release_name`. Falls back to
/// the bare platform when the file is missing/unreadable or off Linux (see
/// `docs/environment.md`).
fn os_context() -> String {
    let os = std::env::consts::OS;
    if os == "linux" {
        let distro = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
            .ok()
            .and_then(|contents| llm::backend::os_release_name(&contents));
        if let Some(distro) = distro {
            return format!("{os} ({distro})");
        }
    }
    os.to_string()
}

/// The live region's height for `app` at the current screen size — exactly what
/// the next [`draw`] will use. Shared so a post-stream commit can reserve that same
/// idle height before flushing the final lines (see [`InlineViewport::set_view_height`]).
fn live_region_height(app: &App, screen: Rect) -> u16 {
    // A pending tool-permission request replaces the whole region — the
    // streaming strip included, since the turn is blocked on the answer. It is
    // modal, so it is checked first (docs/permissions.md).
    if let Some(height) = ui::permission_height(app, screen.width, screen.height) {
        return height;
    }
    // The inline `/model` picker, `/login` flow, and ↓ background manager each
    // replace the whole region with their own framed body (see docs/llm.md /
    // docs/background.md); the open one's height stands in for the composer's.
    if let Some(height) = ui::model_picker_height(app, screen.height) {
        return height;
    }
    if let Some(height) = ui::key_onboarding_height(app, screen.height) {
        return height;
    }
    if let Some(height) = ui::background_view_height(app, screen.height) {
        return height;
    }
    let band = ui::band_rows(app);
    ui::live_height(
        &app.input,
        screen.width,
        screen.height,
        ui::strip_has_status(app),
        ui::preview_rows(app, screen.width),
        ui::queued_rows(app, screen.width),
        ui::toast_rows(app),
        band,
        ui::footer_rows(app, band),
        ui::agent_list_rows(app),
    )
}

/// Upper bound on the rows a [`ReflowClear::Purge`] repaint (`/clear`, resize)
/// re-renders into scrollback. A purge drops the terminal's own scrollback, so
/// the whole conversation is rebuilt from history — capped here so a
/// pathologically long conversation can't turn one resize into an unbounded
/// write. codex bounds its resize reflow the same way (per-terminal 1k–10k rows);
/// 10k is effectively "everything" for any real conversation.
const RESIZE_REFLOW_MAX_ROWS: usize = 10_000;

/// Repaint the inline conversation from `App`'s retained history, re-wrapped to
/// the current width — tail and live region in one synchronized frame
/// ([`InlineViewport::reflow`]). Used after a resize, on `/clear`, and when
/// returning from the tool-output / `/resume` overlay (which kept the stream
/// advancing without committing).
///
/// `clear` selects how the screen is prepared (see [`ReflowClear`]). A
/// [`Purge`] rebuilds scrollback from scratch, so it repaints the **whole**
/// history (bounded by [`RESIZE_REFLOW_MAX_ROWS`]) — otherwise older turns would
/// be lost from the purged scrollback. An [`InPlace`] repaint keeps the
/// terminal's scrollback and only repaints the on-screen tail.
///
/// A mid-stream repaint must not lose the in-flight partial reply — it lives
/// in `App`'s streaming buffer, not `history`, so the tail carries the rows the
/// stream had **already committed** to scrollback ([`ui::repaint_tail`]) and
/// the rows that arrived *since* (chunks drained under the overlay, or the
/// whole partial after a purge dropped its committed copies) are queued right
/// after via [`ui::StreamRender::commit`] — the standard `insert_before`
/// pipeline, landing in the next draw's synchronized update. Repainting from
/// history alone blanked the partial until its next chunk (the Ctrl+O
/// disappear-then-flicker bug), and the old reset-then-recommit-from-scratch
/// duplicated the already-scrolled rows in the terminal's kept scrollback.
///
/// [`Purge`]: ReflowClear::Purge
/// [`InPlace`]: ReflowClear::InPlace
fn repaint_conversation(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    clear: ReflowClear,
) -> io::Result<()> {
    let screen = term.screen();
    if clear == ReflowClear::Purge {
        // The purge drops every committed row (screen and scrollback alike),
        // so nothing is "already committed" any more: reset, and let the
        // catch-up below re-commit the whole partial at the current width.
        render.reset();
    }
    // The preview comes FIRST: it injects the strip's preview row count
    // (`set_stream_preview_rows` — a forming table previews multi-row) that
    // `live_region_height` below must reserve (docs/table-streaming.md).
    let preview = stream_preview_lines(app, render, screen);
    let app: &App = app;
    let height = live_region_height(app, screen);
    let budget = match clear {
        ReflowClear::Purge => RESIZE_REFLOW_MAX_ROWS,
        ReflowClear::InPlace => ui::repaint_budget(screen.height, height),
    };
    let tail = ui::repaint_tail(
        &app.history,
        app.streaming_text(),
        render,
        screen.width,
        budget,
    );
    // Re-emit the header banner atop the rebuilt tail — it lives outside
    // `history` and would otherwise be lost (docs/header.md). A `Purge`
    // rebuilt scrollback from scratch (resize, `/clear`), so the banner tops
    // the full rebuild uncapped. An `InPlace` overwrite (the Ctrl+O /
    // `/resume` return) rewrites the on-screen window — where a short
    // conversation still *shows* the banner, which the overwrite used to wipe
    // — so the banner joins that tail too, re-capped to the window budget:
    // exactly as much of it as the window held comes back, and one that
    // scrolled wholly into the terminal's kept scrollback is not duplicated.
    let tail = ui::banner_tail(
        ui::header_lines(app, screen.width),
        tail,
        match clear {
            ReflowClear::Purge => usize::MAX,
            ReflowClear::InPlace => budget,
        },
    );
    term.reflow(
        tail,
        height,
        clear,
        |area, buf| ui::render_live_with_preview(area, buf, app, preview.as_deref()),
        app,
    )?;
    // Catch scrollback up on what streamed while the overlay was showing (or,
    // after a purge, on the whole partial): exactly the rows live streaming
    // would have committed, queued for the next draw. `reflow` cleared the
    // pending queue, so these can never double up with pre-repaint leftovers.
    if let Some(text) = app.streaming_text().filter(|text| !text.is_empty()) {
        term.insert_before(render.commit(text, screen.width));
    }
    Ok(())
}

/// How an overlay-return repaint prepares the screen: the usual spill-safe
/// in-place overwrite — unless a resize landed while the overlay covered the
/// inline view, which forces the purge-rebuild every resize gets (invariant 3;
/// the emulator reflowed the main screen's rows under the overlay, and an
/// in-place overwrite would leave its re-wrapped copies behind). Consumes the
/// flag, so only the first return purges.
/// Repaint whatever the inline screen is showing — the **agent session
/// view** when one is open (always a purge rebuild of the agent's
/// transcript), else the main conversation with the caller's clear mode.
/// The shared overlay-return / resize repaint (`docs/agent-tool.md`).
fn repaint_active_view(
    term: &mut InlineViewport,
    app: &mut App,
    render: &mut ui::StreamRender,
    agent_render: &mut ui::StreamRender,
    clear: ReflowClear,
) -> io::Result<()> {
    if app.agent_view.is_some() {
        repaint_agent_view(term, app, agent_render)
    } else {
        repaint_conversation(term, app, render, clear)
    }
}

fn overlay_return_clear(overlay_resized: &mut bool) -> ReflowClear {
    if std::mem::take(overlay_resized) {
        ReflowClear::Purge
    } else {
        ReflowClear::InPlace
    }
}

/// The strip's streaming preview: the reply's last rendered line — or, while a
/// table is forming, the whole forming block (capped to
/// [`ui::stream_preview_max_rows`] so its frontier tail-follows on a small
/// screen) — computed cheaply by [`ui::StreamRender::preview`]; `None` when
/// idle or while a tool runs (the tool's own header previews instead). Called
/// before every conversation-view draw so the status animation never pays to
/// re-render the whole reply, and **injects the row count into `App`**
/// ([`App::set_stream_preview_rows`]) so `ui::preview_rows` — and with it
/// `live_region_height`, the strip layout, and the cursor seat — reserve
/// exactly the rows the strip draws. See `docs/markdown.md`,
/// `docs/table-streaming.md`.
fn stream_preview_lines(
    app: &mut App,
    render: &mut ui::StreamRender,
    screen: Rect,
) -> Option<Vec<Line<'static>>> {
    let preview = match app.streaming_text() {
        Some(text) if !text.is_empty() && app.current_tool().is_none() => Some(render.preview(
            text,
            screen.width,
            ui::stream_preview_max_rows(screen.height),
        )),
        _ => None,
    };
    app.set_stream_preview_rows(
        preview
            .as_ref()
            .map_or(1, |p| u16::try_from(p.len()).unwrap_or(u16::MAX)),
    );
    preview
}

/// Render the live region at its current grown height and place the cursor.
/// The composer keeps its cursor even while a reply streams (codex-style —
/// typing mid-turn edits the draft, Enter queues it); only the Ctrl+O overlay
/// hides it (`enter_overlay`). `preview` is the streaming strip's precomputed
/// line(s) (see [`stream_preview_lines`], which also injected their count so
/// `live_region_height` here reserves what the strip draws).
fn draw(term: &mut InlineViewport, app: &App, preview: Option<&[Line<'static>]>) -> io::Result<()> {
    let height = live_region_height(app, term.screen());
    // `term` places the cursor from the final (content-anchored) viewport via
    // `ui::cursor_position`, which mirrors render_live's layout exactly.
    term.draw(
        height,
        |area, buf| ui::render_live_with_preview(area, buf, app, preview),
        app,
    )
}

/// Render the full-screen tool-output overlay. Clamps the scroll to the current
/// screen first (so the last line can reach the bottom but not scroll past it),
/// then paints the view onto the alternate screen.
fn draw_tool_view(
    term: &mut InlineViewport,
    app: &mut App,
    transcript: &mut ui::TranscriptCache,
) -> io::Result<()> {
    let screen = term.screen();
    // An agent session view's Ctrl+O shows the *viewed agent's* transcript —
    // a fresh, bounded build (docs/agent-tool.md); the main cache below is
    // untouched, so the ordinary open stays warm.
    if let Some(lines) = ui::agent_transcript_lines(app, screen.width) {
        let max = ui::tool_view_max_scroll_for(lines.len(), screen.height);
        app.settle_tool_scroll(max);
        return term.draw_overlay(|area, buf| ui::render_tool_view(area, buf, app, &lines));
    }
    // A backtrack preview open/step requested a scroll to its highlighted
    // message (docs/backtrack.md): apply the pure decision once — consumed,
    // so it never fights the user's own scrolling — before the normal clamp.
    if app.take_backtrack_scroll() {
        let selection = transcript.selection(app, screen.width);
        if let Some(scroll) = ui::backtrack_scroll_for(selection, app.tool_scroll, screen.height) {
            app.apply_backtrack_scroll(scroll);
        }
    }
    // Build the transcript at most once here (the cache skips even that while the
    // user only scrolls): the same cached lines feed the clamp and the render, so
    // a scroll keypress no longer re-highlights all of history (twice).
    let max = ui::tool_view_max_scroll_for(transcript.line_count(app, screen.width), screen.height);
    app.settle_tool_scroll(max);
    let lines = transcript.lines(app, screen.width);
    term.draw_overlay(|area, buf| ui::render_tool_view(area, buf, app, lines))
}

/// Render the full-screen `/resume` session picker onto the alternate screen
/// (the transcript overlay's twin). See `docs/resume.md`.
fn draw_resume_picker(term: &mut InlineViewport, app: &App) -> io::Result<()> {
    term.draw_overlay(|area, buf| ui::render_resume_picker(area, buf, app))
}

/// Render the Ctrl+D context-debug view onto the alternate screen — the
/// transcript pager's raw-context sibling: settle the scroll against the
/// current screen, then paint. See `docs/context.md`.
fn draw_context_view(term: &mut InlineViewport, app: &mut App) -> io::Result<()> {
    let screen = term.screen();
    let max = ui::context_view_max_scroll(app, screen.width, screen.height);
    app.settle_debug_scroll(max);
    term.draw_overlay(|area, buf| ui::render_context_view(area, buf, app))
}

// ===== /resume session recording + listing boundary (docs/resume.md) =====

/// How many candidate paths the `/resume` walk will collect before stopping —
/// codex's `MAX_SCAN_FILES` runaway bound (readdir only; cheap).
const RESUME_WALK_CAP: usize = 10_000;

/// How many of the newest-modified candidates get their heads read per
/// `/resume` open — the expensive per-file work (codex pages at 25 with the
/// same 10k scan bound; ours loads one capped page).
const RESUME_SCAN_CAP: usize = 200;

/// How many lines of a rollout file's head the scan reads while hunting for
/// the meta line + first-user-message preview — codex's 10-line head extended
/// by a 200-line user-message hunt.
const RESUME_HEAD_LINES: usize = 210;

/// A byte ceiling on the head read so a pathological no-newline file stays
/// bounded — generous, so a large pasted first message (expanded back to its
/// full text on send) still yields its preview. A line cut at the ceiling
/// fails to parse and is skipped — safe by construction.
const RESUME_HEAD_BYTES: u64 = 2 * 1024 * 1024;

/// Records the conversation to a rollout file as it happens — codex's
/// `RolloutRecorder`, sized down to this loop (see `docs/resume.md`).
///
/// The pure format lives in [`session`]; this owns the impurities: the root
/// dir from the environment, the local clock for the dated path, the UTC
/// write stamps, and the file appends. `recorded` is the on-disk watermark
/// (the scrollback `committed` pattern): [`sync`] appends history growth,
/// rewrites on a truncation (the Esc-Esc backtrack rewind), and does nothing
/// when the length is unchanged. Every failure is swallowed — recording must
/// never kill the TUI (codex logs and carries on the same way).
///
/// [`sync`]: SessionRecorder::sync
struct SessionRecorder {
    /// The sessions root (`~/.alter-zero/sessions`, or
    /// `ALTER_ZERO_SESSIONS_DIR` — the smoke test points it at a temp dir);
    /// `None` disables recording (no HOME and no override).
    root: Option<PathBuf>,
    /// The active session's file + meta, once anything was recorded — created
    /// lazily on the first item so empty sessions never touch disk (codex's
    /// deferred create). The meta is kept for rewrites.
    active: Option<(PathBuf, SessionMeta)>,
    /// How many history items are already on disk.
    recorded: usize,
    /// An adopted file's last line lost its newline (a torn write): the next
    /// append prefixes one so the first new item isn't glued onto it.
    repair_newline: bool,
    /// The filesystem checkpoints recorded in this session (`docs/checkpoint.md`)
    /// — an in-memory mirror of the `checkpoint` lines, kept so a truncation
    /// rewrite can re-emit the survivors and a resume can adopt the file's own.
    /// Appended interleaved with the item lines.
    checkpoints: Vec<checkpoint::Checkpoint>,
    /// How many of [`checkpoints`](Self::checkpoints) are already on disk — the
    /// checkpoint twin of `recorded`, so an append only writes the new ones.
    checkpoints_written: usize,
    /// The meta context of a *new* session file, captured once at startup.
    cwd: String,
    model: String,
}

impl SessionRecorder {
    fn new(model: &str, cwd: &Path) -> Self {
        let root = std::env::var_os("ALTER_ZERO_SESSIONS_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".alter-zero").join("sessions"))
            });
        Self {
            root,
            active: None,
            recorded: 0,
            repair_newline: false,
            checkpoints: Vec::new(),
            checkpoints_written: 0,
            cwd: cwd.display().to_string(),
            model: model.to_string(),
        }
    }

    /// Record a filesystem checkpoint against the current conversation length
    /// (`docs/checkpoint.md`): held in memory now, flushed to the file by the
    /// next [`sync`](Self::sync) (a turn always grows history, so the flush
    /// rides that append — and a session that never grows history never
    /// materializes a file, keeping codex's deferred create).
    fn record_checkpoint(&mut self, checkpoint: checkpoint::Checkpoint) {
        self.checkpoints.push(checkpoint);
    }

    /// The checkpoints known this session — the loop reads these to pick the
    /// restore target for an Esc-Esc backtrack (`docs/backtrack.md`).
    fn checkpoints(&self) -> &[checkpoint::Checkpoint] {
        &self.checkpoints
    }

    /// The sessions root, for the `/resume` scan.
    fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The file currently being written, if any — excluded from the picker
    /// (the session you're in is not something to "return" to).
    fn active_path(&self) -> Option<&Path> {
        self.active.as_ref().map(|(path, _)| path.as_path())
    }

    /// Mirror the file to `history`: append newly finished items (creating
    /// the file + meta line first on the very first one) plus any pending
    /// checkpoint lines (`docs/checkpoint.md`), rewrite the whole file when
    /// history shrank (the backtrack rewind), no-op when unchanged. History is
    /// append-or-truncate only, so the watermark compare is sound. A checkpoint
    /// alone never creates a file (deferred create).
    fn sync(&mut self, history: &[HistoryItem]) {
        if history.len() < self.recorded {
            self.rewrite(history);
            return;
        }
        let items_grew = history.len() > self.recorded;
        let checkpoints_pending = self.checkpoints.len() > self.checkpoints_written;
        // Nothing new to write — neither items nor checkpoints.
        if !items_grew && !checkpoints_pending {
            return;
        }
        // A checkpoint with no file yet (the startup/`/clear` pristine snapshot)
        // must NOT materialize a rollout file — codex's deferred create: an
        // empty session leaves no file. Hold it in memory until a history item
        // creates the file, at which point `append` flushes it alongside. A
        // pending checkpoint with a file that already exists does flush now.
        if !items_grew && self.active.is_none() {
            return;
        }
        let fresh = &history[self.recorded..];
        // Advance the watermark whether or not the write lands: a failed
        // append drops those lines (a later rewrite restores them, since it
        // writes the full history) instead of retrying every event.
        self.recorded = history.len();
        self.append(fresh);
    }

    /// `/clear` starts a fresh session (codex's `/new`): the next recorded
    /// item creates a new file; the old file keeps what it had.
    fn start_new(&mut self) {
        self.active = None;
        self.recorded = 0;
        self.repair_newline = false;
        self.checkpoints.clear();
        self.checkpoints_written = 0;
    }

    /// Adopt a resumed session's file: further items append there (codex's
    /// resume-mode open), and a rewrite re-serializes its own meta. `torn`
    /// flags a file whose last line lost its newline — the next append
    /// repairs it first. `checkpoints` are the file's own recorded snapshots
    /// (`docs/checkpoint.md`), taken over as already-written so later turns
    /// extend the same chain and a backtrack after resuming restores against
    /// them.
    fn adopt(
        &mut self,
        path: PathBuf,
        meta: SessionMeta,
        recorded: usize,
        torn: bool,
        checkpoints: Vec<checkpoint::Checkpoint>,
    ) {
        self.active = Some((path, meta));
        self.recorded = recorded;
        self.repair_newline = torn;
        self.checkpoints_written = checkpoints.len();
        self.checkpoints = checkpoints;
    }

    /// Append `items` as rollout lines, materializing the file (date dirs +
    /// meta line) on the first-ever append. Failures are dropped.
    fn append(&mut self, items: &[HistoryItem]) {
        use std::io::Write;
        if self.active.is_none() {
            self.active = self.create_session();
        }
        let Some((path, _)) = self.active.as_ref() else {
            return; // recording disabled, or the create failed
        };
        let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) else {
            return;
        };
        let stamp = utc_stamp();
        let mut text = String::new();
        // Terminate an adopted file's torn last line so the first new item
        // starts a line of its own (the junk stays, skipped by the reader).
        if std::mem::take(&mut self.repair_newline) {
            text.push('\n');
        }
        for item in items {
            text.push_str(&session::item_line(item, &stamp));
            text.push('\n');
        }
        // Flush any checkpoints recorded since the last append (docs/checkpoint.md)
        // — interleaved with the item lines; parsed back by their own reader.
        for checkpoint in &self.checkpoints[self.checkpoints_written..] {
            text.push_str(&session::checkpoint_line(checkpoint, &stamp));
            text.push('\n');
        }
        self.checkpoints_written = self.checkpoints.len();
        let _ = file.write_all(text.as_bytes());
    }

    /// Create the session file lazily: derive the dated path from the local
    /// clock (codex's layout), create the date dirs, and write the meta line.
    /// `None` when recording is disabled or any I/O fails.
    fn create_session(&self) -> Option<(PathBuf, SessionMeta)> {
        use chrono::{Datelike, Timelike};
        let root = self.root.as_ref()?;
        let now = chrono::Local::now();
        let id = session_id();
        let rel = session::rollout_rel_path(
            (now.year(), now.month(), now.day()),
            (now.hour(), now.minute(), now.second()),
            &id,
        );
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent()?).ok()?;
        let meta = SessionMeta {
            id,
            timestamp: utc_stamp(),
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            originator: "alter-zero".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let first_line = format!("{}\n", session::meta_line(&meta, &meta.timestamp));
        std::fs::write(&path, first_line).ok()?;
        Some((path, meta))
    }

    /// Rewrite the whole file (meta + every item) — the truncation path: the
    /// Esc-Esc backtrack rewound history, and the file must follow. (This
    /// normalizes the file to what this build parsed — lines a future build
    /// wrote and this one skipped are dropped; see `docs/resume.md`.)
    fn rewrite(&mut self, history: &[HistoryItem]) {
        self.recorded = history.len();
        self.repair_newline = false;
        // A truncation (backtrack) drops the checkpoints describing the
        // rewound-away future, so the file mirrors the survivors
        // (docs/checkpoint.md), in lockstep with the in-memory list.
        checkpoint::retain_surviving(&mut self.checkpoints, history.len());
        self.checkpoints_written = self.checkpoints.len();
        let Some((path, meta)) = self.active.as_ref() else {
            return;
        };
        let stamp = utc_stamp();
        let mut text = format!("{}\n", session::meta_line(meta, &meta.timestamp));
        for item in history {
            text.push_str(&session::item_line(item, &stamp));
            text.push('\n');
        }
        for checkpoint in &self.checkpoints {
            text.push_str(&session::checkpoint_line(checkpoint, &stamp));
            text.push('\n');
        }
        let _ = std::fs::write(path, text);
    }
}

/// UTC write-time stamp for rollout lines — codex's
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` shape.
fn utc_stamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// This process's uid — the stable per-user segment of the background tasks
/// root (Claude Code's `claude-{uid}` pattern, `background::tasks_dir`). Read
/// from `/proc/self`'s owner: this crate forbids `unsafe`, so no `libc`
/// getuid. Falls back to 0 where `/proc` is absent (macOS) — `temp_dir()` is
/// already per-user there.
#[cfg(unix)]
fn process_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map_or(0, |meta| meta.uid())
}

/// Non-unix fallback: no uid concept to read — 0 keeps the path shape.
#[cfg(not(unix))]
fn process_uid() -> u32 {
    0
}

/// A unique-enough session id: nanos since the epoch plus the pid, in hex.
/// No uuid dependency — the id is never parsed back (resume goes by path);
/// it only has to keep concurrent instances off each other's files.
fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{nanos:x}-{:x}", std::process::id())
}

/// Seconds since the Unix epoch — the `ts` stamped into each history line.
/// Impurity kept at the boundary (the `utc_stamp`/`session_id` pattern).
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Entries kept in memory (and the file's target after compaction). Recall and
/// Ctrl+R search are O(entries), so this bounds their cost no matter how big
/// the file grew — a shell-style HISTSIZE. See `docs/history-persistence.md`.
const HISTORY_MAX_ENTRIES: usize = 10_000;

/// Compact the file only once it exceeds this many entries (hard cap), trimming
/// back to [`HISTORY_MAX_ENTRIES`] (soft cap) — codex's hard-/soft-cap idea, so
/// we don't rewrite on every startup while hovering at the cap.
const HISTORY_COMPACT_AT: usize = HISTORY_MAX_ENTRIES + HISTORY_MAX_ENTRIES / 4;

/// Don't persist an input longer than this many bytes. A large paste is
/// **expanded** in the composer (`take_input` splices the payload back), so its
/// full text would otherwise be written — and the entry-*count* cap alone
/// wouldn't bound the file's byte size. A giant blob isn't a useful
/// reverse-search target anyway; it still recalls **this** session (it lives in
/// `InputHistory::entries`), it just isn't written to disk. codex bounds its
/// file by `max_bytes` for the same reason. See `docs/history-persistence.md`.
const HISTORY_MAX_ENTRY_BYTES: usize = 100 * 1024;

/// Persists the composer's input history across sessions (codex's
/// `history.jsonl`, sized down to this loop — see `docs/history-persistence.md`).
///
/// The pure format lives in [`history`]; this owns the impurities: the path
/// from the environment, the clock for `ts`, and the file reads/appends/
/// compaction. Best-effort — every failure is swallowed (persistence must never
/// kill the TUI, like [`SessionRecorder`] and `save_settings`).
struct InputHistoryStore {
    /// The history file (`ALTER_ZERO_HISTORY_FILE`, else
    /// `{config_home}/history.jsonl` — beside `config.json`/`.env`, which
    /// `ALTER_ZERO_CONFIG_DIR` already redirects). `None` disables persistence
    /// (no HOME and no override).
    path: Option<PathBuf>,
    /// This process's id, stamped into each record's `session_id` field. The
    /// app never reads it back — it only labels who wrote the line.
    session_id: String,
}

impl InputHistoryStore {
    fn new() -> Self {
        let path = std::env::var_os("ALTER_ZERO_HISTORY_FILE")
            .map(PathBuf::from)
            .or_else(|| config_home().map(|dir| dir.join("history.jsonl")));
        Self {
            path,
            session_id: session_id(),
        }
    }

    /// Load the persisted entries (oldest first), capped to the last
    /// [`HISTORY_MAX_ENTRIES`]. When the file has grown past the hard cap,
    /// rewrite it down to the soft cap first (best-effort). A missing or
    /// unreadable file yields no entries — a first run just starts empty.
    fn load(&self) -> Vec<String> {
        let Some(path) = &self.path else {
            return Vec::new();
        };
        // Read bytes + lossy-decode (not `read_to_string`): a torn/interleaved
        // append can leave invalid UTF-8 in the file, and `read_to_string`
        // would error on it and discard the WHOLE history. Lossy decoding turns
        // only the bad bytes into U+FFFD, so `parse_history` skips just that one
        // line (and the compaction below can then repair the file). This mirrors
        // the shell reader's lossy decode. See `docs/history-persistence.md`.
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let contents = String::from_utf8_lossy(&bytes);
        let mut texts = history::parse_history(&contents);
        let over_hard_cap = texts.len() > HISTORY_COMPACT_AT;
        if texts.len() > HISTORY_MAX_ENTRIES {
            texts.drain(..texts.len() - HISTORY_MAX_ENTRIES);
        }
        if over_hard_cap {
            self.compact(path, &texts);
        }
        texts
    }

    /// Append newly-recorded inputs as JSONL lines in a **single** `O_APPEND`
    /// `write_all` (atomic up to `PIPE_BUF`, so concurrent instances don't
    /// interleave), materializing the parent dir + a `0o600` file on the first
    /// write. Failures are dropped.
    fn append(&self, texts: &[String]) {
        if texts.is_empty() {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        let ts = unix_secs();
        let mut buf = String::new();
        for text in texts {
            // Skip a giant (expanded large-paste) input — bounds the file's
            // bytes, which the entry-count cap can't. It still recalls this
            // session from `entries`.
            if text.len() > HISTORY_MAX_ENTRY_BYTES {
                continue;
            }
            buf.push_str(&history::history_line(&self.session_id, ts, text));
            buf.push('\n');
        }
        if buf.is_empty() {
            return; // every entry was skipped — nothing to write, don't touch the fs
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = append_history_bytes(path, buf.as_bytes());
    }

    /// Rewrite the file to just `texts` (the compaction tail), re-stamped with
    /// this session (the `ts`/`session_id` fields are unused by the app — only
    /// `text` is read back). Best-effort: a failure leaves the oversized file
    /// in place, to be retried next startup.
    fn compact(&self, path: &Path, texts: &[String]) {
        let ts = unix_secs();
        let mut buf = String::new();
        for text in texts {
            buf.push_str(&history::history_line(&self.session_id, ts, text));
            buf.push('\n');
        }
        let _ = write_history_bytes(path, buf.as_bytes());
    }
}

/// Append `bytes` to the history file in one `O_APPEND` write, creating a
/// `0o600` file (the input may hold whatever the user typed — codex writes
/// `history.jsonl` owner-only too).
fn append_history_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

/// Truncate-and-write the history file owner-only (`0o600` on unix) — the
/// compaction rewrite. Tightens the mode on a pre-existing file, since
/// `mode()` only applies at creation (like `write_key_store`).
fn write_history_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(bytes)
}

/// Scan the sessions root for resumable rollout files, newest-modified first
/// (codex's Updated sort): walk the `YYYY/MM/DD` date dirs newest-first,
/// head-read each `rollout-*.jsonl` for its meta line and first-user-message
/// preview — files without both never list (codex's eligibility) — and stop
/// considering files past [`RESUME_SCAN_CAP`]. `exclude` is the recorder's
/// active file. Ages are humanized here and frozen (codex freezes its
/// reference when the picker opens).
fn list_sessions(root: Option<&Path>, exclude: Option<&Path>) -> Vec<SessionSummary> {
    let Some(root) = root else {
        return Vec::new();
    };
    // Collect candidate paths newest-first by the date layout (year desc /
    // month desc / day desc / filename desc — the name embeds the stamp), so
    // the runaway walk bound keeps the newest files if it ever bites.
    let mut files = Vec::new();
    'walk: for year in numeric_dirs_desc(root) {
        for month in numeric_dirs_desc(&year) {
            for day in numeric_dirs_desc(&month) {
                let mut names: Vec<PathBuf> = std::fs::read_dir(&day)
                    .map(|entries| {
                        entries
                            .flatten()
                            .map(|entry| entry.path())
                            .filter(|path| {
                                path.file_name().and_then(|name| name.to_str()).is_some_and(
                                    |name| name.starts_with("rollout-") && name.ends_with(".jsonl"),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort();
                names.reverse();
                files.extend(names);
                if files.len() >= RESUME_WALK_CAP {
                    files.truncate(RESUME_WALK_CAP);
                    break 'walk;
                }
            }
        }
    }

    // Order by mtime (newest-modified first — a resumed old session floats
    // back to the top, codex's Updated sort) BEFORE capping the expensive
    // head reads, so the cap can't cut a recently-touched old file.
    let mut stamped: Vec<(std::time::SystemTime, PathBuf)> = files
        .into_iter()
        .filter(|path| exclude.is_none_or(|active| active != path))
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, path)
        })
        .collect();
    stamped.sort_by(|a, b| b.0.cmp(&a.0));
    stamped.truncate(RESUME_SCAN_CAP);

    let now = std::time::SystemTime::now();
    let mut sessions = Vec::new();
    for (modified, path) in stamped {
        let Some(head) = read_head(&path, RESUME_HEAD_LINES, RESUME_HEAD_BYTES) else {
            continue;
        };
        // Eligibility (codex's): a parseable meta line AND a user message to
        // preview, both within the head window.
        let Some((meta, items)) = session::parse_session(&head) else {
            continue;
        };
        let Some(preview) = session::preview_of(&items) else {
            continue;
        };
        let updated_secs = now.duration_since(modified).map_or(0, |age| age.as_secs());
        // The Created sort key comes from the meta line's session-start
        // stamp; a foreign/unparseable stamp falls back to the mtime so the
        // row still sorts sanely under either key.
        let created_secs = chrono::DateTime::parse_from_rfc3339(&meta.timestamp)
            .ok()
            .map_or(updated_secs, |created| {
                let age = chrono::Utc::now() - created.with_timezone(&chrono::Utc);
                u64::try_from(age.num_seconds()).unwrap_or(0)
            });
        sessions.push(SessionSummary {
            path,
            updated_secs,
            created_secs,
            cwd: meta.cwd,
            preview,
        });
    }
    sessions
}

/// The numerically-named subdirectories of `dir`, sorted descending — the
/// `YYYY`/`MM`/`DD` walk visits newest first (codex's listing walk).
fn numeric_dirs_desc(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let number: u32 = entry.file_name().to_str()?.parse().ok()?;
                    entry
                        .file_type()
                        .ok()?
                        .is_dir()
                        .then(|| (number, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.into_iter().map(|(_, path)| path).collect()
}

/// Read up to `max_lines` lines of `path`'s head, stopping past the `cap`
/// byte ceiling (so a pathological no-newline file stays bounded). Line-based
/// like codex's head scan, so a long line — a large pasted first message —
/// is read whole; a line cut by the ceiling fails to parse and is skipped by
/// the caller. `None` when the file can't be opened or isn't UTF-8.
fn read_head(path: &Path, max_lines: usize, cap: u64) -> Option<String> {
    use std::io::{BufRead, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file).take(cap);
    let mut head = String::new();
    for _ in 0..max_lines {
        match reader.read_line(&mut head) {
            Ok(0) => break, // EOF (or the ceiling exhausted)
            Ok(_) => {}
            // A ceiling cut inside a multi-byte character makes read_line
            // report InvalidData — keep the (valid) lines already read
            // instead of discarding the whole head, so a session whose
            // preview was already in the buffer still lists.
            Err(_) => break,
        }
    }
    Some(head)
}

// ===== `@` file-search boundary (docs/file-search.md) =====

/// Max files the `@` picker's worker indexes — bounds the walk's memory/time
/// (codex's nucleo walk is similarly capped). Captured once per worker lifetime.
const FILE_INDEX_CAP: usize = 10_000;
/// Max ranked matches returned per query (the picker shows up to this many).
const FILE_MENU_LIMIT: usize = 8;
/// Directory names the walk skips, on top of every hidden (dotfile) entry.
const FILE_WALK_DENYLIST: &[&str] = &["target", "node_modules"];

/// A file-search result for the `@` picker: the `query` it answers (for the
/// staleness guard in `App::set_file_matches`) and its ranked `matches`.
struct FileSearchResult {
    query: String,
    matches: Vec<FileMatch>,
}

/// Walk `root` breadth-first collecting up to `cap` relative paths, skipping
/// hidden entries (dotfiles — so `.git` too) and [`FILE_WALK_DENYLIST`]
/// directories; directories are listed with a trailing `/`. Dependency-free (the
/// agreed design — no `.gitignore` parsing). See `docs/file-search.md`.
fn walk_files(root: &Path, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());
    while let Some(dir) = queue.pop_front() {
        if out.len() >= cap {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || FILE_WALK_DENYLIST.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(format!("{rel}/"));
                queue.push_back(path);
            } else {
                out.push(rel);
            }
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}

/// Spawn the `@` file-search worker: a background thread that walks `root` once
/// (caching the file list), then for each query — coalescing any that queued
/// while it worked (the debounce) — ranks the cache ([`rank_files`]) and sends a
/// [`FileSearchResult`] back. Exits when the request channel closes (app exit).
/// It only *sends* on the tokio channel; it never reads stdin (invariant 1).
/// Run one Ctrl+V clipboard read ([`clipboard::read_clipboard_image`]) on a
/// short-lived background thread, delivering the result on the loop's image
/// channel (`select!` branch 5). Detached: at quit a straggler finishes
/// writing a temp file harmlessly (like the shell pipe readers); it only
/// *sends* — never a stdin reader (invariant 1). Each Ctrl+V spawns its own
/// worker, so a double-press attaches two placeholders in completion order —
/// what codex's synchronous handler does too, minus the UI freeze.
fn spawn_image_paste(tx: tokio::sync::mpsc::UnboundedSender<Result<PathBuf, String>>) {
    std::thread::spawn(move || {
        // The receiver only closes at shutdown — a failed send just means
        // there is nothing left to attach to.
        let _ = tx.send(clipboard::read_clipboard_image());
    });
}

fn spawn_file_search_worker(
    root: PathBuf,
    req_rx: std::sync::mpsc::Receiver<String>,
    res_tx: tokio::sync::mpsc::UnboundedSender<FileSearchResult>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut cache: Option<Vec<String>> = None;
        while let Ok(mut query) = req_rx.recv() {
            // Coalesce: if the user kept typing, only serve the newest query.
            while let Ok(newer) = req_rx.try_recv() {
                query = newer;
            }
            let files = cache.get_or_insert_with(|| walk_files(&root, FILE_INDEX_CAP));
            let matches = rank_files(&query, files, FILE_MENU_LIMIT);
            if res_tx.send(FileSearchResult { query, matches }).is_err() {
                break; // the loop is gone
            }
        }
    })
}

/// Dispatch a file search when the active `@token` query changes — codex's
/// `StartFileSearch` on every token change. `last` is the boundary's record of
/// the query last sent, so an unchanged query (or a non-edit key) sends nothing.
fn dispatch_file_search(
    app: &App,
    req_tx: &std::sync::mpsc::Sender<String>,
    last: &mut Option<String>,
) {
    let query = app.file_search_query();
    if query.as_deref() != last.as_deref() {
        if let Some(q) = &query {
            let _ = req_tx.send(q.clone());
        }
        *last = query;
    }
}

// `main.rs` is the terminal I/O boundary (smoke-covered, not unit-tested) —
// except the odd pure helper with no terminal in it, like `term.rs`'s
// `keyboard_enhancement_disabled`. The `!` runner's drain/cap pair is that
// here: `append_capped` (pure cap logic) and `drain_shell_pipe`
// (reader-generic chunk forwarding), tested with in-memory readers.
#[cfg(test)]
mod tests {
    use std::io;

    use super::{append_capped, drain_shell_pipe};

    /// Serves its chunks one per `read` call (each far smaller than the
    /// drain's 64 KiB buffer, so every chunk arrives whole), then EOF —
    /// letting a test control exactly how the input splits across reads.
    struct ChunkedReader {
        chunks: Vec<Vec<u8>>,
        served: usize,
    }

    impl ChunkedReader {
        fn new(chunks: &[&[u8]]) -> Self {
            Self {
                chunks: chunks.iter().map(|c| c.to_vec()).collect(),
                served: 0,
            }
        }
    }

    impl io::Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(chunk) = self.chunks.get(self.served) else {
                return Ok(0); // past the last chunk: EOF
            };
            self.served += 1;
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            Ok(n)
        }
    }

    /// Fails with `ErrorKind::Interrupted` on the first read (a signal landed
    /// mid-`read`), serves its data on the second, then EOF.
    struct InterruptedOnce {
        data: Vec<u8>,
        calls: usize,
    }

    impl io::Read for InterruptedOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            match self.calls {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    let n = self.data.len().min(buf.len());
                    buf[..n].copy_from_slice(&self.data[..n]);
                    Ok(n)
                }
                _ => Ok(0),
            }
        }
    }

    /// Run `reader` through the drain + cap pair the runner's wait loop uses.
    fn drain_capped(reader: impl io::Read, cap: usize) -> (Vec<u8>, bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        drain_shell_pipe(reader, &tx);
        drop(tx);
        let mut buf = Vec::new();
        let mut truncated = false;
        while let Ok(chunk) = rx.try_recv() {
            append_capped(&mut buf, &chunk, cap, &mut truncated);
        }
        (buf, truncated)
    }

    #[test]
    fn empty_input_reads_nothing_and_is_not_truncated() {
        let (out, truncated) = drain_capped(io::Cursor::new(Vec::<u8>::new()), 10);
        assert!(out.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn output_landing_exactly_at_the_cap_is_not_truncated() {
        // Nothing was dropped, so the cell must not gain a `…` marker.
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'a'; 10]), 10);
        assert_eq!(out, vec![b'a'; 10]);
        assert!(!truncated);
    }

    #[test]
    fn one_byte_over_the_cap_truncates_to_exactly_the_cap() {
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'a'; 11]), 10);
        assert_eq!(out.len(), 10);
        assert!(truncated);
    }

    #[test]
    fn a_mid_chunk_cut_keeps_the_head_and_marks_truncation() {
        // One read serves 8 bytes but only 5 fit under the cap: the head is
        // retained byte-for-byte and the cut inside the chunk flags truncated.
        let (out, truncated) = drain_capped(io::Cursor::new(b"abcdefgh".to_vec()), 5);
        assert_eq!(out, b"abcde");
        assert!(truncated);
    }

    #[test]
    fn chunks_past_the_cap_are_drained_but_retain_nothing() {
        // The first chunk lands exactly at the cap (no mid-chunk cut), so only
        // the keep-draining loop can flag the later chunks as dropped.
        let mut reader = ChunkedReader::new(&[b"abcd", b"efgh", b"ijkl"]);
        let (out, truncated) = drain_capped(&mut reader, 4);
        assert_eq!(out, b"abcd");
        assert!(truncated);
        // … and the reader really was drained to EOF (so the child can't block
        // on a full pipe), not abandoned once the cap filled.
        assert_eq!(reader.served, 3);
    }

    #[test]
    fn input_larger_than_the_read_buffer_drains_across_reads() {
        // Bigger than the drain's 64 KiB chunk, so it spans several real
        // reads; only the first `cap` bytes are retained.
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'x'; 200_000]), 100);
        assert_eq!(out, vec![b'x'; 100]);
        assert!(truncated);
    }

    #[test]
    fn an_interrupted_read_is_retried_not_treated_as_eof() {
        let reader = InterruptedOnce {
            data: b"abc".to_vec(),
            calls: 0,
        };
        let (out, truncated) = drain_capped(reader, 10);
        assert_eq!(out, b"abc");
        assert!(!truncated);
    }
}

//! The terminal shell around the [`alter_zero`] library: the async event loop
//! and every piece of real I/O it drives.
//!
//! `main.rs` opens the viewport and calls [`event_loop::run`]; everything else
//! about running the app lives in here, split one module per area the way
//! `app/` and `ui/` are (see `docs/module-layout.md`). The library stays pure —
//! `App` decides, `ui` renders, and this tree is the only place that reads a
//! clock, touches the filesystem, spawns a thread, or writes to a terminal.
//!
//! # The shape
//!
//! [`Session`] holds the loop's state and every handler is a method on it. That
//! is deliberate, and it is the same trick `app/mod.rs` plays with `App`: a
//! struct's private fields are visible to the module that defines it **and all
//! its descendants**, so each area module below can `impl Session` and reach
//! the state it needs directly. Before this, the same state was ~50 locals in
//! one 2,000-line function, threaded into helpers nine arguments at a time.
//!
//! Both ends of every channel live on it too — the senders *and* the receivers
//! the `select!` polls. That works because `select!` scopes its futures: the
//! nine it builds borrow nine distinct fields, and they are dropped before the
//! winning branch's body runs, so that body can take `&mut self`. Keeping the
//! receivers here is what lets `Session::abandon_inflight` swap the reply
//! channel in place instead of the loop threading a `&mut` receiver down
//! through three call layers.
//!
//! # Where things live
//!
//! | Module | Holds |
//! |--------|-------|
//! | `mod.rs` | The [`Session`] struct and [`StatusClocks`] — the shared state. |
//! | [`event_loop`] | [`event_loop::run`]: the `select!` over the eleven sources, the loop-bottom work, the teardown. |
//! | [`actions`] | The [`alter_zero::app::Action`] dispatch — one method per key-press outcome. |
//! | [`turn`] | Starting and ending turns: user, queued, `!` shell, background follow-up, `/compact`. |
//! | [`stream`] | Folding one streamed reply event into `App` + scrollback. |
//! | [`agent`] | Subagent events, the roster's clocks, the linger sweep (`docs/agent-tool.md`). |
//! | [`background`] | Background-shell events and settling their notices (`docs/background.md`). |
//! | [`permission`] | The permission gate's session rules and the prompt's answers (`docs/permissions.md`). |
//! | [`view`] | Drawing: the inline region, the overlays, the repaints, the status clocks. |
//! | [`commit`] | Writing finished lines to scrollback — the one place invariant 4 is enforced. |
//! | [`models`] | The reply backend and everything that selects it (`docs/llm.md`). |
//! | [`config`] | Reading the environment: providers, keys, settings, permission rules. |
//! | [`bootstrap`] | Assembling a [`Session`] before the first frame. |
//! | [`startup`] | The `--continue`/`--resume` argument resolution (`docs/cli.md`). |
//! | [`recorder`] | Mirroring history to a rollout file (`docs/resume.md`). |
//! | [`resume`] | Finding recorded sessions on disk. |
//! | [`settings`] | Applying a `/settings` knob the menu cycled (`docs/settings.md`). |
//! | [`telemetry`] | The once-a-day anonymous usage ping: the install id, the notice, the send, the recorded day (`docs/telemetry.md`). |
//! | [`update`] | The once-a-day update check: the request, the card under the banner, the recorded day (`docs/update.md`). |
//! | [`update_cli`] | The `alter-zero update` subcommand: the check, then the one-line installer over this binary (`docs/update.md`). |
//! | [`history_store`] | The cross-session input history (`docs/history-persistence.md`). |
//! | [`shell`] | The `!` command runner (`docs/shell-command.md`). |
//! | [`workers`] | The off-thread file-search / clipboard / model-list jobs. |
//! | [`host`] | Clocks, dates, the OS string, ids — the raw impurities. |

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;

use ratatui::crossterm::event::EventStream;

use alter_zero::agents::{AgentEvent, AgentRegistry};
use alter_zero::app::App;
use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::checkpoint::CheckpointStore;
use alter_zero::clipboard::ClipboardLease;
use alter_zero::frame::FrameRequester;
use alter_zero::paste::PasteBurst;
use alter_zero::stream::{CancelToken, StreamEvent};
use alter_zero::term::InlineViewport;
use alter_zero::ui;

use self::history_store::InputHistoryStore;
use self::models::ModelSession;
use self::permission::PermissionStore;
use self::recorder::SessionRecorder;
use self::workers::{DeviceEvent, FileSearchResult, ModelFetch};

pub(crate) mod actions;
pub(crate) mod agent;
pub(crate) mod background;
pub(crate) mod bootstrap;
pub(crate) mod commit;
pub(crate) mod config;
pub(crate) mod donate;
pub(crate) mod event_loop;
pub(crate) mod history_store;
pub(crate) mod host;
pub(crate) mod login;
pub(crate) mod mascot;
pub(crate) mod mcp;
pub(crate) mod mcp_cli;
pub(crate) mod models;
pub(crate) mod permission;
pub(crate) mod recorder;
pub(crate) mod resume;
pub(crate) mod settings;
pub(crate) mod shell;
pub(crate) mod spinner;
pub(crate) mod startup;
pub(crate) mod stream;
pub(crate) mod telemetry;
pub(crate) mod theme;
pub(crate) mod trust;
pub(crate) mod turn;
pub(crate) mod update;
pub(crate) mod update_cli;
pub(crate) mod view;
pub(crate) mod workers;

/// Everything the running app owns outside the pure library: the terminal, the
/// conversation state, the renderers, the boundary's clocks, the registries and
/// stores, and the in-flight turn.
///
/// The fields are private and every area module reaches them directly through
/// its own `impl Session` block (see the module doc). The borrow of the viewport
/// is what makes this a `'t` — `main.rs` keeps ownership so it can restore the
/// terminal even if the loop returns an error.
pub(crate) struct Session<'t> {
    /// The custom inline viewport: scrollback commits, the live region, the
    /// alternate-screen overlays.
    term: &'t mut InlineViewport,
    /// The pure conversation state — the only thing that *decides* anything.
    app: App,
    /// Who replies, and how (`docs/llm.md`).
    models: ModelSession,

    // ----- renderers and caches (all incremental; see docs/markdown.md) -----
    /// Commits the in-flight reply to scrollback as it streams — O(reply) over
    /// the whole stream, and the strip's cheap preview line.
    render: ui::StreamRender,
    /// The same, for an open agent session view's own transcript
    /// (`docs/agent-tool.md`).
    agent_render: ui::StreamRender,
    /// The Ctrl+O overlay's incrementally-built transcript, retained across
    /// closes and warmed at the loop bottom so it never opens cold
    /// (`docs/tool-view-performance.md`).
    transcript: ui::TranscriptCache,
    /// The Ctrl+D overlay's built context window, rebuilt only when its
    /// signature changes — per frame the O(conversation) derivation starved
    /// the scroll keys on a big context (`docs/context.md`).
    context: ui::ContextCache,
    /// Detects a paste / fast-type burst so its redraws coalesce.
    burst: PasteBurst,

    // ----- the boundary's clocks and deadlines (the set_status_times pattern) -----
    /// The live status indicator's clocks.
    clocks: StatusClocks,
    /// When the transient toast above the box should self-clear (`docs/toast.md`).
    toast_deadline: Option<Instant>,
    /// When each background shell started, so the manager's Runtime ticks
    /// (`docs/background.md`).
    bg_clocks: HashMap<String, Instant>,
    /// When each running subagent started, so the roster's elapsed ticks.
    agent_clocks: HashMap<String, Instant>,
    /// When each subagent's **open thinking phase** started — the per-agent
    /// sibling of `StatusClocks::thinking_start`, so its session view's
    /// status line shows `Thinking for Ns` and the settle knows the phase's
    /// wall-clock. An entry exists only while that agent is thinking
    /// (`docs/agent-view-streaming.md`).
    agent_thinking_clocks: HashMap<String, Instant>,
    /// When each subagent's **current running command** started — the
    /// per-agent sibling of `StatusClocks::command_start`, injected each
    /// frame as `AgentRun::command_elapsed` so its session view's `bash`
    /// tail counts its `+N lines (Ns)` footer from the call's own
    /// `ToolStart`, never from the agent's runtime. Inserted at that
    /// `ToolStart`; **pruned by the roster tick** from the run's own queue —
    /// an agent without a running front call loses its entry — rather than
    /// removed at each resolution path (`docs/agent-view-streaming.md`).
    agent_command_clocks: HashMap<String, Instant>,
    /// When each finished subagent's row should sweep off the roster
    /// (`docs/agent-tool.md`).
    agent_expiry: HashMap<String, Instant>,

    // ----- the loop's own channels: what wakes it, and what it sends on -----
    //
    // Both ends live here. `select!` creates all nine futures up front, each
    // borrowing a *distinct field* of this struct, and scopes them so the branch
    // body that wins can still take `&mut self` — which is what lets a handler
    // both read a receiver and mutate everything else. `recv`/`try_recv` hand
    // back owned values, so no borrow outlives the call itself.
    /// Terminal input — the **sole** stdin reader (invariant 1). Created after
    /// the viewport's synchronous cursor query, in [`Session::bootstrap`].
    events: EventStream,
    /// Asks the frame scheduler for a redraw, and its coalesced, rate-limited
    /// draw ticks coming back.
    frame: FrameRequester,
    draw_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    /// The reply channel a turn streams on. Both ends are swapped wholesale by
    /// [`Session::abandon_inflight`] so a detached backend can't reach the next
    /// turn (`docs/interrupt.md`).
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    reply_rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    /// Queries for the `@` file-search worker and its ranked matches, plus the
    /// last query sent — the dedupe that keeps an unchanged token from
    /// re-walking the tree (`docs/file-search.md`).
    file_req_tx: std::sync::mpsc::Sender<String>,
    file_rx: tokio::sync::mpsc::UnboundedReceiver<FileSearchResult>,
    last_file_query: Option<String>,
    /// Where this session's pasted images are saved —
    /// `{config_home}/image-cache/{session}`, numbered in paste order
    /// (`docs/image-paste.md`). Handed to each paste worker, which creates it.
    paste_dir: PathBuf,
    /// A Ctrl+V clipboard read's result channel (`docs/image-paste.md`).
    img_tx: tokio::sync::mpsc::UnboundedSender<Result<PathBuf, String>>,
    img_rx: tokio::sync::mpsc::UnboundedReceiver<Result<PathBuf, String>>,
    /// The `/login` device-flow worker's channel (`docs/copilot.md`), the
    /// `CancelToken` Esc reaps it with, and when the shown code expires — the
    /// countdown the page ticks, injected per draw like every other clock.
    device_tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
    device_rx: tokio::sync::mpsc::UnboundedReceiver<DeviceEvent>,
    device_cancel: Option<CancelToken>,
    device_expires: Option<Instant>,
    /// The `/model` picker's fetch results (`docs/llm.md`), and the startup
    /// capability probe's own channel — separate so a concurrently-open picker
    /// can't confuse the results (`docs/reasoning.md`).
    model_tx: tokio::sync::mpsc::UnboundedSender<ModelFetch>,
    model_rx: tokio::sync::mpsc::UnboundedReceiver<ModelFetch>,
    probe_rx: tokio::sync::mpsc::UnboundedReceiver<ModelFetch>,
    /// Background shells (`docs/background.md`) and subagents
    /// (`docs/agent-tool.md`) — never swapped, because both outlive turns.
    bg_rx: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    agent_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    /// MCP server state changes (`docs/mcp.md`) — never swapped either;
    /// connections outlive turns like background shells.
    mcp_rx: tokio::sync::mpsc::UnboundedReceiver<alter_zero::llm::mcp::McpEvent>,
    /// The telemetry ping worker's report (`docs/telemetry.md`): the UTC day
    /// it delivered, which the **loop** then records in `telemetry.json` —
    /// the worker never writes the file, so it can't race the `/settings`
    /// toggle's write. Silent on failure.
    telemetry_tx: tokio::sync::mpsc::UnboundedSender<String>,
    telemetry_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    /// The UTC day this **session** last spawned a ping for, successful or
    /// not (`docs/telemetry.md`). Two jobs: the turn-start rollover check
    /// compares against it and returns without touching the file on the
    /// common path, and it bounds a failing collector to one attempt per day
    /// per session rather than one per turn — the file's `last_ping_day`,
    /// written only on a `2xx`, is still what retries the next launch.
    telemetry_attempted: Option<String>,
    /// The update check worker's report (`docs/update.md`): the newest
    /// release it found, which the **loop** then records in `update.json`
    /// and announces — the telemetry channel's twin.
    update_tx: tokio::sync::mpsc::UnboundedSender<String>,
    update_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    /// The UTC day this **session** last spawned a check for, so the
    /// turn-start rollover costs a string compare on the common path.
    update_attempted: Option<String>,
    /// A newer release learned of while a turn was streaming: its card must
    /// not land in the middle of a reply, so it waits here for the loop
    /// bottom to find the session idle (`Session::flush_pending_update_notice`).
    update_notice_pending: Option<String>,
    /// The `@` file-search worker's handle, kept so the thread's lifetime is
    /// tied to the session's. Never joined.
    _file_worker: JoinHandle<()>,

    // ----- registries, the gate, the stores -----
    /// Background shells: the `run_in_background` launches, Ctrl+B hand-offs,
    /// and the shared notice board (`docs/background.md`).
    registry: BackgroundRegistry,
    /// Subagents: the `agent` tool's roster and its threads
    /// (`docs/agent-tool.md`).
    agent_registry: AgentRegistry,
    /// The **mid-turn message queue** (`docs/queue.md`): what the user
    /// submitted while a turn was already running, waiting for that turn's
    /// next round boundary. This side pushes; the backend thread drains.
    /// `App::steered` is the mirror the strip renders — and the one that
    /// survives a turn that ended without reading them, which is why the
    /// reclaim at every turn end goes through both.
    steer: alter_zero::steer::SteerQueue,
    /// The tool-permission gate and this project's saved rules
    /// (`docs/permissions.md`).
    permissions: PermissionStore,
    /// The `AskUserQuestion` gate the modal's answers post on
    /// (`docs/ask.md`) — always present (asking is not a permission), shared
    /// by every backend build.
    ask: alter_zero::ask::AskGate,
    /// The shared task list the task tools operate on
    /// (`docs/task-tools.md`) — the loop syncs it to `App::tasks` after every
    /// history rewind so the model's next `tasklist` agrees with the strip.
    task_registry: alter_zero::tasks::TaskRegistry,
    /// The skills on disk (`docs/skills.md`) — the set every backend build
    /// offers the `skill` tool over, and the source of the
    /// `<system-reminder>` listing the derived context leads with. Re-walked
    /// at every turn start ([`Session::rescan_skills`]), so a skill added
    /// mid-session — or written by the agent itself — is live on the next
    /// turn instead of waiting for a restart.
    skill_registry: alter_zero::skills::SkillRegistry,
    /// The subagent definitions on disk (`docs/subagents.md`) — the
    /// `agents/*.md` types a launch resolves `subagent_type` against, and the
    /// source of the agent half of the `<system-reminder>`. Re-walked at
    /// every turn start ([`Session::rescan_agents`]) beside the skills, so a
    /// type added mid-session — or written by the agent itself — is
    /// launchable on the next turn.
    subagents: alter_zero::subagents::SubagentRegistry,
    /// The MCP servers (`docs/mcp.md`): every declared server's live
    /// connection state, the tool specs the backend folds in, and the OAuth
    /// flows. `None` when `ALTER_ZERO_MCP` turned the feature off.
    mcp: Option<alter_zero::llm::mcp::McpManager>,
    /// The project's `.alter-zero` config layer as loaded at bootstrap — the
    /// snapshot `/trust` reviews and its approval records/activates. `None`
    /// when `ALTER_ZERO_PROJECT_CONFIG` turned the layer off
    /// (`docs/project-config.md`).
    project_layer: Option<trust::ProjectLayer>,
    /// The **user** layer of the parsed hooks config, kept apart from the
    /// merge riding `HookSetup` so a `/trust` approval or revoke can rebuild
    /// the merge live (`docs/project-config.md`).
    user_hooks_file: alter_zero::hooks::HooksFile,
    /// The `SKILL.md` files whose parse failure has already been raised as a
    /// toast, so the rescan doesn't repeat itself every turn. Re-seeded from
    /// each walk's errors, so a file that is fixed and broken again reports
    /// again (`alter_zero::skills::unreported_errors`).
    reported_skill_errors: std::collections::BTreeSet<std::path::PathBuf>,
    /// The agent definition files whose parse failure has already been raised
    /// as a toast — [`reported_skill_errors`](Self::reported_skill_errors)'s
    /// twin, one feature over (`alter_zero::subagents::unreported_errors`).
    reported_agent_errors: std::collections::BTreeSet<std::path::PathBuf>,
    /// Mirrors history to the `/resume` rollout file (`docs/resume.md`).
    recorder: SessionRecorder,
    /// The cross-session input history (`docs/history-persistence.md`).
    hist_store: InputHistoryStore,
    /// Per-turn working-directory snapshots (`docs/checkpoint.md`).
    checkpoints: CheckpointStore,
    /// Where the `/settings` knobs persist (`settings.json`, this directory's
    /// entry — `docs/per-directory-state.md`); `None` disables persistence,
    /// like `config.json`'s path (`docs/settings.md`). The live values
    /// themselves live on `App` — one copy, read where they are used; a save
    /// is a read-modify-write over the file itself, moving across only the
    /// key the user cycled, so an `ALTER_ZERO_*` override merged in at startup
    /// never becomes the saved default.
    settings_path: Option<PathBuf>,

    // ----- the in-flight turn -----
    /// The streaming reply's cancel token + thread handle; `None` when idle.
    inflight: Option<(CancelToken, JoinHandle<()>)>,
    /// Backend threads whose turn was interrupted or `/clear`ed: cancelled and
    /// **detached**, swept here when finished. Never joined on the loop — that
    /// was the interrupt-lag freeze (`docs/interrupt.md`).
    reaping: Vec<JoinHandle<()>>,

    // ----- odds and ends the boundary has to remember -----
    /// Keeps the last `/copy`'s native clipboard selection alive (Linux/arboard
    /// serves it from a thread tied to the `Clipboard`'s lifetime). Held only
    /// for its `Drop` — never read (`docs/copy.md`).
    clipboard_lease: Option<ClipboardLease>,
    /// Whether the terminal was resized while an overlay covered the inline
    /// view, which upgrades that return's repaint to a purge rebuild
    /// (invariant 3).
    overlay_resized: bool,
    /// The signature of the framed-view rows currently **flowed** into
    /// scrollback — a screen-tall `/mcp`/`/hooks`/`/trust` page's top,
    /// committed above the live region so the terminal's own scrolling reads
    /// the whole page (`ui::view_flow`, `docs/view-flow.md`). `None` when
    /// nothing is flowed. The draw tick compares it against the current
    /// state's signature and answers any mismatch — a navigation, a resize,
    /// the close — with the standard purge rebuild, which re-establishes (or
    /// clears) the flow; while it is `Some`, scrollback commits pause
    /// ([`Session::commits_allowed`]) so nothing tears the flowed page.
    flowed_view: Option<u64>,
    /// The working directory: the tasks root, the permissions project key, the
    /// checkpoint store, the `/resume` filter, and the footer's `~`-relative
    /// form all key off it.
    cwd: PathBuf,
    cwd_display: String,
}

/// The live status indicator's clocks, bundled (the impurity kept at the
/// boundary): the pure `App` only ever sees the *computed* durations, via
/// `set_status_times`. See `docs/status-indicator.md`.
pub(crate) struct StatusClocks {
    /// When the turn was submitted — the elapsed timer and the verb's shimmer
    /// phase. `None` when no turn is in flight.
    turn_start: Option<Instant>,
    /// When the current thinking phase began (`None` outside one).
    thinking_start: Option<Instant>,
    /// When the **current running command** (a model `bash` call, set at its
    /// `ToolStart`; or a `!` shell run, set in `Session::run_shell`) began —
    /// cleared when it resolves (`ToolEnd`/`ToolBackgrounded`) and at each turn
    /// start. Drives the delayed `(ctrl+b to run in background)` hint via
    /// `App::set_command_elapsed` (`docs/background.md`).
    command_start: Option<Instant>,
    /// When the event loop started — the epoch of the **animation phase** every
    /// pulsing bullet breathes against (`App::set_pulse`, `docs/tool-pulse.md`).
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

    /// Start a turn's clocks: the elapsed timer runs, no thinking phase is
    /// open, and no command is running yet (a model `bash` call starts its own
    /// hint clock at its `ToolStart`).
    fn start_turn(&mut self) {
        self.turn_start = Some(Instant::now());
        self.thinking_start = None;
        self.command_start = None;
    }

    /// Stop tracking a finished (or abandoned) turn. `command_start` is left
    /// alone: each resolution path clears it explicitly.
    fn end_turn(&mut self) {
        self.turn_start = None;
        self.thinking_start = None;
    }
}

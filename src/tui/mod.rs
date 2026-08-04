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
//! The receivers deliberately stay *outside* `Session`, in
//! [`event_loop::Sources`]: `select!` borrows several of them at once, which
//! only type-checks while they are separate places. `Session` holds the
//! matching senders, so a handler can start a turn or ask for a frame.
//!
//! # Where things live
//!
//! | Module | Holds |
//! |--------|-------|
//! | `mod.rs` | The [`Session`] struct and [`StatusClocks`] — the shared state. |
//! | [`event_loop`] | [`event_loop::run`]: the `select!` over the eight sources, the loop-bottom work, the teardown. |
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
//! | [`history_store`] | The cross-session input history (`docs/history-persistence.md`). |
//! | [`shell`] | The `!` command runner (`docs/shell-command.md`). |
//! | [`workers`] | The off-thread file-search / clipboard / model-list jobs. |
//! | [`host`] | Clocks, dates, the OS string, ids — the raw impurities. |

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;

use alter_zero::agents::AgentRegistry;
use alter_zero::app::App;
use alter_zero::background::BackgroundRegistry;
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
use self::workers::ModelFetch;

pub(crate) mod actions;
pub(crate) mod agent;
pub(crate) mod background;
pub(crate) mod bootstrap;
pub(crate) mod commit;
pub(crate) mod config;
pub(crate) mod event_loop;
pub(crate) mod history_store;
pub(crate) mod host;
pub(crate) mod models;
pub(crate) mod permission;
pub(crate) mod recorder;
pub(crate) mod resume;
pub(crate) mod shell;
pub(crate) mod startup;
pub(crate) mod stream;
pub(crate) mod turn;
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
    /// When each finished subagent's row should sweep off the roster
    /// (`docs/agent-tool.md`).
    agent_expiry: HashMap<String, Instant>,

    // ----- senders into the loop's own channels (the receivers are Sources) -----
    /// Asks the frame scheduler for a redraw.
    frame: FrameRequester,
    /// The reply channel a turn streams on. Swapped wholesale by
    /// [`Session::abandon_inflight`] so a detached backend can't reach the next
    /// turn (`docs/interrupt.md`).
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    /// Queries for the `@` file-search worker, and the last one sent — the
    /// dedupe that keeps an unchanged token from re-walking the tree.
    file_req_tx: std::sync::mpsc::Sender<String>,
    last_file_query: Option<String>,
    /// A Ctrl+V clipboard read's result channel (`docs/image-paste.md`).
    img_tx: tokio::sync::mpsc::UnboundedSender<Result<PathBuf, String>>,
    /// The `/model` picker's fetch results (`docs/llm.md`).
    model_tx: tokio::sync::mpsc::UnboundedSender<ModelFetch>,

    // ----- registries, the gate, the stores -----
    /// Background shells: the `run_in_background` launches, Ctrl+B hand-offs,
    /// and the shared notice board (`docs/background.md`).
    registry: BackgroundRegistry,
    /// Subagents: the `agent` tool's roster and its threads
    /// (`docs/agent-tool.md`).
    agent_registry: AgentRegistry,
    /// The tool-permission gate and this project's saved rules
    /// (`docs/permissions.md`).
    permissions: PermissionStore,
    /// Mirrors history to the `/resume` rollout file (`docs/resume.md`).
    recorder: SessionRecorder,
    /// The cross-session input history (`docs/history-persistence.md`).
    hist_store: InputHistoryStore,
    /// Per-turn working-directory snapshots (`docs/checkpoint.md`).
    checkpoints: CheckpointStore,

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

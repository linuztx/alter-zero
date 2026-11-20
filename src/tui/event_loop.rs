//! The async event loop: a `select!` over every source that can wake the app.
//!
//! Sixteen sources fan onto one thread — terminal input, coalesced draw
//! ticks, the streamed reply, `@` file-search results, finished Ctrl+V
//! clipboard reads, the git review's workers, the `/login` sign-in worker,
//! `/usage` reads, `/model` list fetches, the startup capability probe,
//! background-shell events, subagent events, MCP server events, the telemetry
//! ping's report, the update check's, and the rows the transcript worker has
//! rendered.
//!
//! **Input and a due frame come first, in that order** (`biased`), and every
//! other source is chosen fairly among the rest ([`next_wake`]'s own
//! unbiased `select!`). A key is what the user is waiting on and a frame is
//! what shows it, and neither can crowd the others out: keys arrive at a
//! person's pace and frames at the scheduler's floor. The reverse was not
//! true. With every branch drawn at random a due frame won one poll in
//! fourteen against a reply channel that always had an event ready, so a fast
//! stream held each keystroke's paint behind a dozen more of its own events
//! (`docs/input-latency.md`).
//!
//! Every branch is one call on the [`Session`] that owns both ends of all the
//! channels, then [`Session::after_iteration`] for the loop-bottom bookkeeping.
//! That works — receiver and handler on the same struct — because `select!`
//! scopes its futures: each one it builds borrows a *distinct field*
//! ([`Sources`] names the data channels' fields one by one for that reason),
//! and all of them are dropped before the winning branch's body runs, so that
//! body is free to take `&mut session`. A handler that needs to drain a second
//! channel reads it the same way (`try_recv` returns an owned value, so no
//! borrow outlives the call).
//!
//! **Invariant 1:** the `EventStream` is the sole stdin reader.
//! `InlineViewport::init` already queried the cursor position over stdin,
//! synchronously, before that stream existed — a second reader would steal the
//! reply. Nothing here (and nothing in `workers`) may read stdin. The keys that
//! read took in on the way (typed while it waited) are replayed before the
//! first `select!`, through the same handler.
//!
//! ```text
//! keyboard / resize ──► EventStream ─┐  first
//! frame scheduler ───► draw-tick ────┤  second
//! reply backend ─────► tokio mpsc ───┤
//! file / image / model workers ──────┼─► select! ─► Session::on_* ─► schedule_frame
//! background shells + subagents ─────┘  fairly, among themselves
//!                                    └─ turn active? re-arm a frame in 32ms
//! ```

use std::io;
use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::StreamExt;

use alter_zero::agents::AgentEvent;
use alter_zero::background::BgEvent;
use alter_zero::llm::mcp::McpEvent;
use alter_zero::stream::StreamEvent;
use alter_zero::telemetry::Delivery;
use alter_zero::term::InlineViewport;
use alter_zero::ui;

use super::Session;
use super::actions::Flow;
use super::diff::DiffResult;
use super::plugins::PluginEvent;
use super::startup::Startup;
use super::usage::UsageResult;
use super::workers::{DeviceEvent, FileSearchResult, ModelFetch, ProbeFetch};

/// Run the app to completion.
///
/// `startup` is the CLI's `--continue`/`--resume` directive plus the optional
/// `[PROMPT]`, applied before the first frame (`docs/cli.md`). Returns the
/// active session's id when the run recorded a conversation — the exit hint
/// `main` prints after the terminal is restored — else `None`.
pub(crate) async fn run(term: &mut InlineViewport, startup: Startup) -> io::Result<Option<String>> {
    let mut session = Session::bootstrap(term, startup)?;
    // Keys typed while `InlineViewport::init`'s startup read waited on the
    // terminal: that read took them in, so they go first — through the same
    // handler, and ahead of anything the EventStream reads (`term::query`).
    for event in session.term.take_typeahead() {
        if session.on_terminal_event(event)? == Flow::Quit {
            return Ok(session.shutdown());
        }
        session.after_iteration();
    }
    loop {
        tokio::select! {
            biased;

            // 1. Terminal input, ahead of everything: a key waits behind
            //    nothing. The branch always matches (it binds the Option), so
            //    `select!` can never run out of armed branches.
            maybe_read = next_event(&mut session.events) => {
                // Stdin closing (or `read?` bailing below) can leave the loop
                // with an overlay still up — no exit_overlay runs on these
                // paths, so main's term.restore() leaves the alternate screen
                // itself (term::OVERLAY_ACTIVE, the panic hook's net).
                let Some(read) = maybe_read else { break }; // stdin closed
                if session.on_terminal_event(read?)? == Flow::Quit {
                    break;
                }
            }

            // 2. A coalesced draw tick, ahead of whatever it would show:
            //    refresh the injected times, then paint. The scheduler emits
            //    at most one per frame floor, so painting first costs the
            //    sources below nothing they would notice.
            Some(()) = session.draw_rx.recv() => session.on_draw_tick()?,

            // 3. Every other source, fairly among themselves.
            wake = next_wake(Sources {
                reply: &mut session.reply_rx,
                file: &mut session.file_rx,
                image: &mut session.img_rx,
                diff: &mut session.diff_rx,
                device: &mut session.device_rx,
                usage: &mut session.usage_rx,
                plugins: &mut session.plugins_page.rx,
                model: &mut session.model_rx,
                probe: &mut session.probe_rx,
                probe_pending: session.models.probe_pending(),
                background: &mut session.bg_rx,
                agent: &mut session.agent_rx,
                mcp: &mut session.mcp_rx,
                telemetry: &mut session.telemetry_rx,
                update: &mut session.update_rx,
                transcript: &mut session.transcript_rx,
                transcript_open: session.transcript_worker.is_some(),
            }) => session.on_wake(wake)?,
        }
        session.after_iteration();
    }
    Ok(session.shutdown())
}

/// What woke the loop besides a key or a frame — one event off one of the
/// data channels ([`next_wake`]).
enum Wake {
    /// A streamed reply event. App state is always updated; lines are only
    /// committed to scrollback in the conversation view (in an overlay we
    /// hold off and repaint on return).
    Reply(StreamEvent),
    /// A file-search result for the open `@` picker.
    File(FileSearchResult),
    /// A finished Ctrl+V clipboard read.
    Image(Result<PathBuf, String>),
    /// A git review worker's update.
    Diff(DiffResult),
    /// The `/login` device flow's worker: the code to show, then the
    /// sign-in's verdict (`docs/copilot.md`).
    Device(DeviceEvent),
    /// One subscription's `/usage` read (`docs/usage.md`).
    Usage(UsageResult),
    /// A `/plugins` worker's step, answer or contents read
    /// (`docs/plugins-menu.md`).
    Plugins(Box<PluginEvent>),
    /// A finished `/model` fetch from one provider's worker thread.
    Model(ModelFetch),
    /// The startup capability probe answered.
    Probe(ProbeFetch),
    /// A background-shell event from the registry's monitors.
    Background(BgEvent),
    /// A subagent event — its own stream, its settle, or the lead's stop or
    /// resume of it (`docs/agent-tools.md`).
    Agent(AgentEvent),
    /// An MCP server's state changed (a connect resolved, an auth flow
    /// progressed) — `docs/mcp.md`.
    Mcp(McpEvent),
    /// The day's telemetry ping was delivered (`docs/telemetry.md`).
    Telemetry(Delivery),
    /// The update check answered (`docs/update.md`).
    Update(String),
    /// A history item's rows for the Ctrl+O transcript, rendered off this
    /// thread — or `None`: the worker is gone, which only a render that
    /// panicked does (`docs/input-latency.md`).
    Transcript(Option<ui::RenderedTranscriptItem>),
}

/// The data channels [`next_wake`] waits on — each receiver borrowed by its
/// own field, so the loop's `select!` can hold them beside the terminal's
/// events and the draw ticks (the module docs).
struct Sources<'a> {
    reply: &'a mut UnboundedReceiver<StreamEvent>,
    file: &'a mut UnboundedReceiver<FileSearchResult>,
    image: &'a mut UnboundedReceiver<Result<PathBuf, String>>,
    diff: &'a mut UnboundedReceiver<DiffResult>,
    device: &'a mut UnboundedReceiver<DeviceEvent>,
    usage: &'a mut UnboundedReceiver<UsageResult>,
    plugins: &'a mut UnboundedReceiver<Box<PluginEvent>>,
    model: &'a mut UnboundedReceiver<ModelFetch>,
    probe: &'a mut UnboundedReceiver<ProbeFetch>,
    /// Whether the startup probe's answer is still wanted — a `/model`
    /// switch that raced it (and already knows its support first-hand) wins,
    /// so its channel is only read while this holds.
    probe_pending: bool,
    background: &'a mut UnboundedReceiver<BgEvent>,
    agent: &'a mut UnboundedReceiver<AgentEvent>,
    mcp: &'a mut UnboundedReceiver<McpEvent>,
    telemetry: &'a mut UnboundedReceiver<Delivery>,
    update: &'a mut UnboundedReceiver<String>,
    transcript: &'a mut UnboundedReceiver<ui::RenderedTranscriptItem>,
    /// Whether a transcript worker was started at all — a headless session
    /// has none, and its closed channel is nobody's news.
    transcript_open: bool,
}

/// The next event off any data channel. `select!` polls these branches in
/// randomized order, so none of them can starve another — the round-robin
/// fairness codex builds explicitly, kept among the sources that can each
/// arrive in a flood (a reply, a subagent's own stream, a shell's output).
async fn next_wake(sources: Sources<'_>) -> Wake {
    tokio::select! {
        Some(event) = sources.reply.recv() => Wake::Reply(event),
        Some(result) = sources.file.recv() => Wake::File(result),
        Some(result) = sources.image.recv() => Wake::Image(result),
        Some(result) = sources.diff.recv() => Wake::Diff(result),
        Some(event) = sources.device.recv() => Wake::Device(event),
        Some(result) = sources.usage.recv() => Wake::Usage(result),
        Some(event) = sources.plugins.recv() => Wake::Plugins(event),
        Some(fetch) = sources.model.recv() => Wake::Model(fetch),
        Some(answer) = sources.probe.recv(), if sources.probe_pending => Wake::Probe(answer),
        Some(event) = sources.background.recv() => Wake::Background(event),
        Some(event) = sources.agent.recv() => Wake::Agent(event),
        Some(event) = sources.mcp.recv() => Wake::Mcp(event),
        Some(delivered) = sources.telemetry.recv() => Wake::Telemetry(delivered),
        Some(latest) = sources.update.recv() => Wake::Update(latest),
        // No `Some(…)` pattern: this channel closing is itself the event.
        done = sources.transcript.recv(), if sources.transcript_open => Wake::Transcript(done),
        // Every channel closed (the session holds a sender of each, so this
        // is the teardown's own business): nothing more will arrive, and the
        // loop goes on serving keys and frames.
        else => std::future::pending().await,
    }
}

impl Session<'_> {
    /// Handle one data-channel event — the routing table from a [`Wake`] to
    /// the method of the area that owns it.
    fn on_wake(&mut self, wake: Wake) -> io::Result<()> {
        match wake {
            Wake::Reply(event) => self.on_reply_event(event),
            Wake::File(result) => self.on_file_matches(result),
            Wake::Image(result) => self.on_image_paste(result),
            Wake::Diff(result) => self.on_diff_result(result)?,
            Wake::Device(event) => self.on_device_event(event),
            Wake::Usage(result) => self.on_usage_result(result),
            Wake::Plugins(event) => self.on_plugin_event(*event),
            Wake::Model(fetch) => self.on_model_fetch(fetch),
            Wake::Probe((provider, result)) => self.apply_capability_probe(&provider, result),
            Wake::Background(event) => self.on_bg_event(event),
            Wake::Agent(event) => self.on_agent_message(event),
            Wake::Mcp(event) => self.on_mcp_event(event),
            Wake::Telemetry(delivered) => self.on_telemetry_result(&delivered),
            Wake::Update(latest) => self.on_update_result(&latest),
            Wake::Transcript(Some(done)) => self.on_transcript_item(done),
            // The worker's thread ended, which it does only by panicking in
            // a render: the hook has printed why and put the terminal back,
            // and the session ends here as it did when this thread rendered
            // the item itself.
            Wake::Transcript(None) => {
                return Err(io::Error::other("the transcript worker stopped"));
            }
        }
        Ok(())
    }
}

/// The next terminal event — or never, for a session with no terminal (a
/// headless one never comes through this loop; the arm stays armed either
/// way, so `select!` can never run out of branches).
async fn next_event(
    events: &mut Option<ratatui::crossterm::event::EventStream>,
) -> Option<io::Result<ratatui::crossterm::event::Event>> {
    match events {
        Some(events) => events.next().await,
        None => std::future::pending().await,
    }
}

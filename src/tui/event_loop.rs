//! The async event loop: a `select!` over every source that can wake the app.
//!
//! Eight sources fan onto one thread — terminal input, the streamed reply,
//! coalesced draw ticks, `@` file-search results, finished Ctrl+V clipboard
//! reads, `/model` list fetches, the startup capability probe, background-shell
//! events and subagent events. `select!` polls its branches in randomized order,
//! so input and draws can't starve each other — the round-robin fairness codex
//! builds explicitly.
//!
//! The receivers live in [`Sources`] rather than on `Session` because `select!`
//! borrows several of them at once, which only type-checks while they are
//! separate places. Two of them are also passed *into* handlers: the reply
//! receiver, because an interrupt or `/clear` replaces the whole channel
//! ([`Session::abandon_inflight`]), and the subagent receiver, because a group's
//! resolution has to drain its members' events first.
//!
//! **Invariant 1:** the `EventStream` is the sole stdin reader. `InlineViewport::init`
//! already queried the cursor position over stdin, synchronously, before this
//! stream existed — a second reader would steal that reply. Nothing here (and
//! nothing in `workers`) may read stdin.
//!
//! Every branch ends the same way: one handler call, then
//! [`Session::after_iteration`] for the loop-bottom bookkeeping.
//!
//! ```text
//! keyboard / resize ──► EventStream ─┐
//! reply backend ─────► tokio mpsc ───┼─► select! ─► Session::on_* ─► schedule_frame
//! frame scheduler ───► draw-tick ────┤
//! file / image / model workers ──────┤
//! background shells + subagents ─────┘
//!                                    └─ turn active? re-arm a frame in 32ms
//! ```

use std::io;
use std::path::PathBuf;
use std::thread::JoinHandle;

use ratatui::crossterm::event::EventStream;
use tokio_stream::StreamExt;

use alter_zero::agents::AgentEvent;
use alter_zero::background::BgEvent;
use alter_zero::stream::StreamEvent;
use alter_zero::term::InlineViewport;

use super::Session;
use super::actions::Flow;
use super::startup::Startup;
use super::workers::{FileSearchResult, ModelFetch};

/// Everything the loop receives on. Held apart from [`Session`] so `select!` can
/// borrow the branches it polls independently (see the module doc).
///
/// Built by `Session::bootstrap`, which is why the fields are `pub(super)`: the
/// `EventStream` must be created **after** the viewport's synchronous cursor
/// query, and bootstrap is where that ordering is visible (invariant 1).
pub(crate) struct Sources {
    /// Terminal input — the sole stdin reader (invariant 1).
    pub(super) events: EventStream,
    /// The streamed reply. Replaced wholesale by an interrupt or `/clear`, which
    /// is how a detached backend thread is isolated from the next turn.
    pub(super) reply_rx: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
    /// Coalesced, rate-limited draw ticks from the frame scheduler.
    pub(super) draw_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    /// The `@` picker's ranked matches (`docs/file-search.md`).
    pub(super) file_rx: tokio::sync::mpsc::UnboundedReceiver<FileSearchResult>,
    /// A finished Ctrl+V clipboard read (`docs/image-paste.md`).
    pub(super) img_rx: tokio::sync::mpsc::UnboundedReceiver<Result<PathBuf, String>>,
    /// One provider's `/model` list (`docs/llm.md`).
    pub(super) model_rx: tokio::sync::mpsc::UnboundedReceiver<ModelFetch>,
    /// The startup capability probe's own channel, so a concurrently-open
    /// `/model` picker can't confuse the results (`docs/reasoning.md`).
    pub(super) probe_rx: tokio::sync::mpsc::UnboundedReceiver<ModelFetch>,
    /// Background shells (`docs/background.md`) — never swapped, so shells
    /// survive interrupts.
    pub(super) bg_rx: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    /// Subagents (`docs/agent-tool.md`) — likewise, because agents outlive turns.
    pub(super) agent_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    /// The `@` file-search worker's handle, kept so the thread's lifetime is
    /// tied to the loop's. Never joined.
    pub(super) _file_worker: JoinHandle<()>,
}

/// Run the app to completion.
///
/// `startup` is the CLI's `--continue`/`--resume` directive, applied before the
/// first frame (`docs/cli.md`). Returns the active session's id when the run
/// recorded a conversation — the exit hint `main` prints after the terminal is
/// restored — else `None`.
pub(crate) async fn run(
    term: &mut InlineViewport,
    startup: Option<Startup>,
) -> io::Result<Option<String>> {
    let (mut session, mut sources) = Session::bootstrap(term, startup)?;
    loop {
        tokio::select! {
            // 1. Terminal input. The events branch always matches (it binds the
            //    Option), so `select!` can never run out of armed branches.
            maybe_read = sources.events.next() => {
                // Stdin closing (or `read?` bailing below) can leave the loop
                // with an overlay still up — no exit_overlay runs on these
                // paths, so main's term.restore() leaves the alternate screen
                // itself (term::OVERLAY_ACTIVE, the panic hook's net).
                let Some(read) = maybe_read else { break }; // stdin closed
                if session.on_terminal_event(read?, &mut sources.reply_rx)? == Flow::Quit {
                    break;
                }
            }

            // 2. A streamed reply event. App state is always updated; lines are
            //    only committed to scrollback in the conversation view (in an
            //    overlay we hold off and repaint on return).
            Some(event) = sources.reply_rx.recv() => {
                session.on_reply_event(event, &mut sources.agent_rx);
            }

            // 3. A coalesced draw tick: refresh the injected times, then paint.
            Some(()) = sources.draw_rx.recv() => session.on_draw_tick()?,

            // 4. A file-search result for the open `@` picker.
            Some(result) = sources.file_rx.recv() => session.on_file_matches(result),

            // 5. A finished Ctrl+V clipboard read.
            Some(result) = sources.img_rx.recv() => session.on_image_paste(result),

            // 6. A finished `/model` fetch from one provider's worker thread.
            Some((label, result)) = sources.model_rx.recv() => {
                session.on_model_fetch(label, result);
            }

            // 7. The startup capability probe answered. Guarded so a `/model`
            //    switch that raced it (and already knows its support first-hand)
            //    wins.
            Some((provider, result)) = sources.probe_rx.recv(),
                if session.models.probe_pending() =>
            {
                session.apply_capability_probe(&provider, result);
            }

            // 8. A background-shell event from the registry's monitors.
            Some(event) = sources.bg_rx.recv() => session.on_bg_event(event),

            // 9. A subagent event.
            Some(AgentEvent::Stream { id, event }) = sources.agent_rx.recv() => {
                session.on_agent_stream(&id, event);
            }
        }
        session.after_iteration();
    }
    Ok(session.shutdown())
}

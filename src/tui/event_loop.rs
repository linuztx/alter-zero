//! The async event loop: a `select!` over every source that can wake the app.
//!
//! Nine sources fan onto one thread — terminal input, the streamed reply,
//! coalesced draw ticks, `@` file-search results, finished Ctrl+V clipboard
//! reads, `/model` list fetches, the startup capability probe, background-shell
//! events and subagent events. `select!` polls its branches in randomized order,
//! so input and draws can't starve each other — the round-robin fairness codex
//! builds explicitly.
//!
//! Every branch is one call on the [`Session`] that owns both ends of all nine
//! channels, then [`Session::after_iteration`] for the loop-bottom bookkeeping.
//! That works — receiver and handler on the same struct — because `select!`
//! scopes its futures: the nine it builds borrow nine *distinct fields*, and all
//! of them are dropped before the winning branch's body runs, so that body is
//! free to take `&mut session`. A handler that needs to drain a second channel
//! reads it the same way (`try_recv` returns an owned value, so no borrow
//! outlives the call).
//!
//! **Invariant 1:** the `EventStream` is the sole stdin reader.
//! `InlineViewport::init` already queried the cursor position over stdin,
//! synchronously, before that stream existed — a second reader would steal the
//! reply. Nothing here (and nothing in `workers`) may read stdin.
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

use tokio_stream::StreamExt;

use alter_zero::agents::AgentEvent;
use alter_zero::term::InlineViewport;

use super::Session;
use super::actions::Flow;
use super::startup::Startup;

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
    let mut session = Session::bootstrap(term, startup)?;
    loop {
        tokio::select! {
            // 1. Terminal input. The events branch always matches (it binds the
            //    Option), so `select!` can never run out of armed branches.
            maybe_read = session.events.next() => {
                // Stdin closing (or `read?` bailing below) can leave the loop
                // with an overlay still up — no exit_overlay runs on these
                // paths, so main's term.restore() leaves the alternate screen
                // itself (term::OVERLAY_ACTIVE, the panic hook's net).
                let Some(read) = maybe_read else { break }; // stdin closed
                if session.on_terminal_event(read?)? == Flow::Quit {
                    break;
                }
            }

            // 2. A streamed reply event. App state is always updated; lines are
            //    only committed to scrollback in the conversation view (in an
            //    overlay we hold off and repaint on return).
            Some(event) = session.reply_rx.recv() => session.on_reply_event(event),

            // 3. A coalesced draw tick: refresh the injected times, then paint.
            Some(()) = session.draw_rx.recv() => session.on_draw_tick()?,

            // 4. A file-search result for the open `@` picker.
            Some(result) = session.file_rx.recv() => session.on_file_matches(result),

            // 5. A finished Ctrl+V clipboard read.
            Some(result) = session.img_rx.recv() => session.on_image_paste(result),

            // 5b. The `/login` device flow's worker: the code to show, then
            //     the sign-in's verdict (`docs/copilot.md`).
            Some(event) = session.device_rx.recv() => session.on_device_event(event),

            // 6. A finished `/model` fetch from one provider's worker thread.
            Some((label, result)) = session.model_rx.recv() => {
                session.on_model_fetch(label, result);
            }

            // 7. The startup capability probe answered. Guarded so a `/model`
            //    switch that raced it (and already knows its support first-hand)
            //    wins.
            Some((provider, result)) = session.probe_rx.recv(),
                if session.models.probe_pending() =>
            {
                session.apply_capability_probe(&provider, result);
            }

            // 8. A background-shell event from the registry's monitors.
            Some(event) = session.bg_rx.recv() => session.on_bg_event(event),

            // 9. A subagent event.
            Some(AgentEvent::Stream { id, event }) = session.agent_rx.recv() => {
                session.on_agent_stream(&id, event);
            }

            // 10. An MCP server's state changed (a connect resolved, an auth
            //     flow progressed) — docs/mcp.md.
            Some(event) = session.mcp_rx.recv() => session.on_mcp_event(event),
        }
        session.after_iteration();
    }
    Ok(session.shutdown())
}

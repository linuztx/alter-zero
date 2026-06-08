//! Thin terminal shell around the [`inline_tui`] library.
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
//!    channel), and coalesced draw ticks from the [`frame`] scheduler,
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
//! [`PasteBurst`] so its redraw defers to the burst's tail. `insert_before`
//! stays inline (it mutates scrollback immediately); only the live-region paint
//! waits for a tick.
//!
//! On **any width change** (and on returning from the tool-output overlay) the
//! visible conversation needs re-wrapping, so `App` retains a `history` and we
//! repaint from it — see [`repaint_conversation`].

use std::io;
use std::thread::JoinHandle;
use std::time::Instant;

use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio_stream::StreamExt;

use inline_tui::app::{Action, App, Role, View};
use inline_tui::frame::{self, FrameRequester};
use inline_tui::paste::{self, PasteBurst};
use inline_tui::stream::{CancelToken, DummyAi, ReplySource, StreamEvent};
use inline_tui::term::InlineViewport;
use inline_tui::ui;

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;
    let result = run(&mut term).await;
    // Always restore the terminal (raw mode off, cursor below the box), even if
    // the loop bailed out with an I/O error — then surface the first error.
    let restored = term.restore();
    result.and(restored)
}

/// The async event loop. A `select!` fans three sources onto one thread: terminal
/// input, the streamed reply, and coalesced draw ticks. `select!` polls its
/// branches in randomized order, so input and draws can't starve each other —
/// the round-robin fairness codex builds explicitly.
async fn run(term: &mut InlineViewport) -> io::Result<()> {
    // Backend → loop (the streamed reply). A tokio channel so the loop can
    // `select!` on it; the backend thread sends without touching the runtime.
    let (tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    // The frame requester + its scheduler task, and the scheduler's draw-tick
    // channel (scheduler → loop).
    let (frame, frame_rx) = frame::channel();
    let (draw_tx, mut draw_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    tokio::spawn(frame::run_scheduler(frame_rx, draw_tx));

    let mut app = App::new();
    // The reply backend. Swap this single line for a real model (any
    // `ReplySource`) and nothing else in the loop has to change.
    let backend = DummyAi;
    // The in-flight reply's cancel token + thread handle, so a quit mid-stream
    // can stop and reap it cleanly. `None` whenever no reply is streaming.
    let mut inflight: Option<(CancelToken, JoinHandle<()>)> = None;
    // How many lines of the in-progress reply have been flushed to scrollback.
    let mut committed = 0usize;
    // Detects a paste / fast-type burst so its redraw can be coalesced.
    let mut burst = PasteBurst::new();

    // Init already queried the cursor over stdin; the EventStream is now the sole
    // stdin reader (see the module-level invariant note).
    let mut events = EventStream::new();

    frame.schedule_frame(); // first paint

    loop {
        tokio::select! {
            // 1. Terminal input. The events branch always matches (it binds the
            //    Option), so `select!` can never run out of armed branches.
            maybe_read = events.next() => {
                let Some(read) = maybe_read else { break }; // stdin closed
                match read? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match app.on_key(key) {
                            Action::Quit => {
                                // Drop back to the main screen before the loop exits
                                // if the overlay is up, so restore() lands on the chat.
                                if app.view == View::ToolOutput {
                                    term.exit_overlay()?;
                                }
                                break;
                            }
                            Action::None => {}
                            Action::Submit(text) => {
                                let width = term.screen().width;
                                app.record_user_message(&text);
                                term.insert_before(ui::message_lines(Role::User, &text, width))?;
                                term.insert_before(vec![Line::default()])?;
                                app.begin_stream();
                                committed = 0;
                                let cancel = CancelToken::new();
                                let handle = backend.spawn(text, tx.clone(), cancel.clone());
                                inflight = Some((cancel, handle));
                            }
                            Action::ToggleToolView => {
                                // on_key already flipped app.view; sync the overlay.
                                if app.view == View::ToolOutput {
                                    term.enter_overlay()?;
                                } else {
                                    term.exit_overlay()?;
                                    // Catch the inline view up on whatever streamed
                                    // while the overlay was showing.
                                    repaint_conversation(term, &app, &mut committed)?;
                                }
                            }
                            Action::Notice(text) => {
                                // A slash command's one-off system notice. If a reply
                                // is mid-flight, finalise its current segment first
                                // (same ordering trick as a tool call) so the notice
                                // slots after it in scrollback and history alike.
                                let width = term.screen().width;
                                if let Some(segment) = app.flush_streaming_segment() {
                                    term.insert_before(ui::final_commit(&segment, width, committed))?;
                                    term.insert_before(vec![Line::default()])?;
                                    committed = 0;
                                }
                                app.record_system_message(&text);
                                term.insert_before(ui::message_lines(Role::System, &text, width))?;
                                term.insert_before(vec![Line::default()])?;
                            }
                            Action::Clear => {
                                // `/clear` already emptied app.history; repaint the
                                // now-blank inline view.
                                committed = 0;
                                repaint_conversation(term, &app, &mut committed)?;
                            }
                        }
                        schedule_for_key(&frame, &mut burst, &key);
                    }
                    Event::Resize(width, height) => {
                        let width_changed = term.resized(width, height);
                        // Only the inline view reflows; the overlay just redraws at
                        // the new size (reflowing would write the alternate screen).
                        if width_changed && app.view == View::Conversation {
                            repaint_conversation(term, &app, &mut committed)?;
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
                if on_stream_event(term, &mut app, &mut committed, stream_event)? {
                    inflight = None; // the stream ended
                }
                frame.schedule_frame();
            }

            // 3. A coalesced draw tick: paint the current view.
            Some(()) = draw_rx.recv() => {
                match app.view {
                    View::Conversation => draw(term, &app)?,
                    View::ToolOutput => draw_tool_view(term, &mut app)?,
                }
            }
        }
    }

    // Stop any in-flight reply promptly and reap its thread on the way out.
    if let Some((cancel, handle)) = inflight.take() {
        cancel.cancel();
        let _ = handle.join();
    }
    Ok(())
}

/// Apply one streamed reply event to `app`, committing finished lines to
/// scrollback in the conversation view. Returns whether the stream just **ended**
/// (`StreamDone`/`Error`), so the caller can clear its in-flight handle. The
/// commit work mirrors how a resize repaints the same items from history.
fn on_stream_event(
    term: &mut InlineViewport,
    app: &mut App,
    committed: &mut usize,
    event: StreamEvent,
) -> io::Result<bool> {
    let width = term.screen().width;
    // Lines are only committed to scrollback in the conversation view; in the
    // overlay we hold off and repaint the inline view on return, so the stream
    // keeps advancing without touching the alt screen.
    let committing = app.view == View::Conversation;
    match event {
        StreamEvent::Chunk(chunk) => {
            app.push_chunk(&chunk);
            if committing && let Some(text) = app.streaming_text() {
                let (lines, new_committed) = ui::stable_commit(text, width, *committed);
                term.insert_before(lines)?;
                *committed = new_committed;
            }
            Ok(false)
        }
        StreamEvent::ToolStart { name, args } => {
            // Finalise the current run of assistant text so the tool slots after
            // it in scrollback, then show the tool running (blue) in the live
            // region until its ToolEnd arrives. The flush always runs (it records
            // history); only the commit is view-gated.
            if let Some(segment) = app.flush_streaming_segment()
                && committing
            {
                term.insert_before(ui::final_commit(&segment, width, *committed))?;
                term.insert_before(vec![Line::default()])?;
            }
            *committed = 0;
            app.start_tool(&name, &args);
            Ok(false)
        }
        StreamEvent::ToolEnd { output, ok } => {
            // Commit the finished tool *collapsed* (green/red) to scrollback; its
            // full output lives in the Ctrl+O view.
            if let Some(tool) = app.end_tool(&output, ok)
                && committing
            {
                term.insert_before(ui::tool_lines(&tool, width))?;
                term.insert_before(vec![Line::default()])?;
            }
            Ok(false)
        }
        StreamEvent::StreamDone => {
            if let Some(text) = app.finish_stream()
                && committing
            {
                // The reply just ended, so the streaming strip (preview + gap,
                // drawn *above* the box) is gone. Reseat the viewport to its idle
                // height *before* the final commit so those lines replace the
                // strip's rows in place and the box stays flush at the bottom
                // instead of rising (which would leave blank rows beneath it).
                term.set_view_height(live_region_height(app, term.screen()));
                term.insert_before(ui::final_commit(&text, width, *committed))?;
                term.insert_before(vec![Line::default()])?; // blank spacer
            }
            *committed = 0;
            Ok(true)
        }
        StreamEvent::Error(message) => {
            if let Some(failure) = app.fail_stream(&message) {
                // Flush whatever streamed before the failure, then the red error
                // notice — each with a trailing blank spacer, mirroring how a
                // resize repaints them from history.
                if committing {
                    // The stream ended, so collapse the streaming strip into the
                    // idle box height before committing (same reason as StreamDone)
                    // so the box does not rise off the bottom.
                    term.set_view_height(live_region_height(app, term.screen()));
                    if let Some(partial) = failure.partial {
                        term.insert_before(ui::final_commit(&partial, width, *committed))?;
                        term.insert_before(vec![Line::default()])?;
                    }
                    term.insert_before(ui::message_lines(Role::Error, &failure.error, width))?;
                    term.insert_before(vec![Line::default()])?;
                }
            }
            *committed = 0;
            Ok(true)
        }
    }
}

/// Schedule the redraw for a just-handled key. A plain typed character that lands
/// in a [`PasteBurst`] defers its paint to the burst's tail — one coalesced
/// redraw for the run instead of one per key; every other key (and the first
/// characters of a run) paints at once. The frame scheduler still rate-limits the
/// result to 120 fps regardless.
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

/// The live region's height for `app` at the current screen size — exactly what
/// the next [`draw`] will use. Shared so a post-stream commit can reserve that same
/// idle height before flushing the final lines (see [`InlineViewport::set_view_height`]).
fn live_region_height(app: &App, screen: Rect) -> u16 {
    ui::live_height(
        &app.input,
        screen.width,
        screen.height,
        app.is_streaming(),
        ui::menu_rows(app),
    )
}

/// Repaint the inline conversation from `App`'s retained history, re-wrapped to
/// the current width. Used both after a resize *and* when returning from the
/// tool-output overlay (which kept the stream advancing without committing).
///
/// We repaint the tail that fits above the live region; resetting `committed`
/// lets any in-progress reply re-commit itself from scratch on its next chunk,
/// so a mid-stream resize or overlay round-trip recovers too.
fn repaint_conversation(
    term: &mut InlineViewport,
    app: &App,
    committed: &mut usize,
) -> io::Result<()> {
    let screen = term.screen();
    let height = live_region_height(app, screen);
    let budget = ui::repaint_budget(screen.height, height);
    let tail = ui::repaint_lines(&app.history, screen.width, budget);
    term.reflow(tail, height)?;
    *committed = 0;
    Ok(())
}

/// Render the live region at its current grown height and place the cursor (only
/// while editing — it's hidden while a reply streams).
fn draw(term: &mut InlineViewport, app: &App) -> io::Result<()> {
    let height = live_region_height(app, term.screen());
    // The cursor sits at the end of the input; `term` places it from the final
    // (content-anchored) viewport via `ui::cursor_position`, so we just hand it the
    // app while editing (and `None` while streaming hides the cursor).
    let cursor = (!app.is_streaming()).then_some(app);
    term.draw(height, |area, buf| ui::render_live(area, buf, app), cursor)
}

/// Render the full-screen tool-output overlay. Clamps the scroll to the current
/// screen first (so the last line can reach the bottom but not scroll past it),
/// then paints the view onto the alternate screen.
fn draw_tool_view(term: &mut InlineViewport, app: &mut App) -> io::Result<()> {
    let screen = term.screen();
    let max = ui::tool_view_max_scroll(app, screen.width, screen.height);
    app.settle_tool_scroll(max);
    term.draw_overlay(|area, buf| ui::render_tool_view(area, buf, app))
}

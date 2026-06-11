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
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio_stream::StreamExt;

use inline_tui::app::{Action, App, INTERRUPT_NOTICE, Role, View};
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
    // Inject the wall-clock here at the I/O boundary so the pure library never
    // sees a clock. Recorded items are stamped with the local time; the stamp is
    // shown only in the Ctrl+O transcript (see docs/timestamps.md).
    app.set_clock(local_timestamp);
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
    // The live status indicator's clocks (impurity kept here, at the boundary):
    // when the turn was submitted, and when the current thinking phase began
    // (`None` when not in one). The pure `App` only ever sees the *computed*
    // durations, via `set_status_times`. See docs/status-indicator.md.
    let mut turn_start: Option<Instant> = None;
    let mut thinking_start: Option<Instant> = None;

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
                                // A turn may have finished while the overlay was
                                // showing — its scrollback commits were deferred
                                // (invariant 4) — so repaint the inline view from
                                // history (and draw the box) the same way a normal
                                // Ctrl+O return does; otherwise restore() lands on the
                                // stale live status strip ("Working… (… tokens)")
                                // instead of the committed "Done for Ns" summary.
                                if app.view == View::ToolOutput {
                                    term.exit_overlay()?;
                                    repaint_conversation(term, &app, &mut committed)?;
                                    draw(term, &app)?;
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
                                // Start the turn clock; the draw branch keeps the
                                // status animated from here (see its re-arm note).
                                turn_start = Some(Instant::now());
                                thinking_start = None;
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
                            Action::Interrupt => {
                                // Esc mid-generation (codex-style): stop the backend
                                // promptly and reap it, then drop anything it sent
                                // before observing the cancel — a stale ToolStart
                                // processed after the interrupt would wedge a phantom
                                // "running" tool whose ToolEnd never comes. The thread
                                // is joined, so after the drain the channel stays empty.
                                if let Some((cancel, handle)) = inflight.take() {
                                    cancel.cancel();
                                    let _ = handle.join();
                                }
                                while reply_rx.try_recv().is_ok() {}
                                if let Some(interrupted) = app.interrupt_turn() {
                                    // The turn is over, so the streaming strip is gone:
                                    // reseat the viewport to its idle height before
                                    // committing (same dance as StreamDone) so the box
                                    // stays flush at the bottom. Interrupt only arises
                                    // in the conversation view (overlay Esc returns
                                    // instead), so committing here never touches the
                                    // alternate screen.
                                    let width = term.screen().width;
                                    term.set_view_height(live_region_height(&app, term.screen()));
                                    if let Some(partial) = interrupted.partial {
                                        term.insert_before(ui::final_commit(&partial, width, committed))?;
                                        term.insert_before(vec![Line::default()])?;
                                    }
                                    if let Some(tool) = interrupted.tool {
                                        term.insert_before(ui::tool_lines(&tool, width))?;
                                        term.insert_before(vec![Line::default()])?;
                                    }
                                    term.insert_before(ui::message_lines(
                                        Role::Error,
                                        INTERRUPT_NOTICE,
                                        width,
                                    ))?;
                                    term.insert_before(vec![Line::default()])?;
                                }
                                committed = 0;
                                turn_start = None;
                                thinking_start = None;
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
                if on_stream_event(
                    term, &mut app, &mut committed,
                    &mut turn_start, &mut thinking_start, stream_event,
                )? {
                    inflight = None; // the stream ended
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
                update_status_times(&mut app, turn_start, thinking_start);
                match app.view {
                    View::Conversation => draw(term, &app)?,
                    View::ToolOutput => draw_tool_view(term, &mut app)?,
                }
                if app.turn_active() {
                    frame.schedule_frame_in(STATUS_FRAME_INTERVAL);
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
///
/// `turn_start`/`thinking_start` are the live-status clocks: thinking events flip
/// `thinking_start`, and the turn-ending events read `turn_start` for the
/// `"Done for Ns"` summary, then clear both.
fn on_stream_event(
    term: &mut InlineViewport,
    app: &mut App,
    committed: &mut usize,
    turn_start: &mut Option<Instant>,
    thinking_start: &mut Option<Instant>,
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
        StreamEvent::ThinkingStart => {
            // Opaque phase boundary: start the thinking clock so the status line
            // shows `Thinking for Ns`. No scrollback commit (thinking is live-only).
            *thinking_start = Some(Instant::now());
            Ok(false)
        }
        StreamEvent::ThinkingEnd => {
            *thinking_start = None;
            Ok(false)
        }
        StreamEvent::StreamDone => {
            let final_text = app.finish_stream();
            let elapsed = turn_start.map_or(0, |start| start.elapsed().as_secs());
            let summary = app.end_turn(elapsed);
            if committing {
                // The reply just ended, so the streaming strip (preview + gap +
                // status, drawn *above* the box) is gone. Reseat the viewport to
                // its idle height *before* committing so the final reply line and
                // the "Done for Ns" summary replace the strip's rows in place and
                // the box stays flush at the bottom (instead of rising and leaving
                // blank rows beneath it).
                term.set_view_height(live_region_height(app, term.screen()));
                if let Some(text) = final_text {
                    term.insert_before(ui::final_commit(&text, width, *committed))?;
                    term.insert_before(vec![Line::default()])?; // blank spacer
                }
                if let Some(summary) = summary {
                    term.insert_before(ui::summary_lines(&summary, width))?;
                    term.insert_before(vec![Line::default()])?; // blank spacer
                }
            }
            *committed = 0;
            *turn_start = None;
            *thinking_start = None;
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
            *turn_start = None;
            *thinking_start = None;
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

/// How often the live status re-arms its next animation frame while a turn is in
/// flight (~30 fps — codex's status-widget cadence): drives the verb's shimmer
/// sweep and keeps the timer advancing through event-less pauses.
const STATUS_FRAME_INTERVAL: Duration = Duration::from_millis(32);

/// Write the live status's times onto `app` before a draw: how long the turn has
/// run (whole seconds for display; sub-second for the shimmer phase) and the
/// current thinking-phase duration (`Some` while thinking). Time is impure, so
/// this is the boundary's job — the pure `App`/`ui` only ever see the
/// already-computed values. No-op when no turn is in flight.
fn update_status_times(
    app: &mut App,
    turn_start: Option<Instant>,
    thinking_start: Option<Instant>,
) {
    let elapsed = turn_start.map_or(Duration::ZERO, |start| start.elapsed());
    let thinking = thinking_start.map(|start| start.elapsed());
    app.set_status_times(elapsed, thinking);
}

/// Local wall-clock stamp for recorded items: 12-hour time, no seconds, e.g.
/// `03:20 AM`. Injected via [`App::set_clock`] and shown **only** under the
/// user's message in the Ctrl+O transcript — the one impurity kept out of the
/// pure library.
fn local_timestamp() -> String {
    chrono::Local::now().format("%I:%M %p").to_string()
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
        ui::menu_rows(app) + ui::shortcuts_rows(app),
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

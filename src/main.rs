//! Thin terminal shell around the [`inline_tui`] library.
//!
//! This file is the one place that drives real terminal I/O, so it is
//! intentionally tiny and free of logic worth unit-testing — all of that lives
//! in `app`, `ui`, `stream`, and the geometry helpers `term` consumes. Its job is
//! only to:
//!
//! 1. open the custom inline viewport ([`term::InlineViewport`] — no alternate
//!    screen, real scrollback preserved, **dynamic** content-anchored height),
//! 2. read keyboard input and drain streamed chunks in one loop,
//! 3. translate the [`App`]'s decisions into `insert_before` / `draw` calls.
//!
//! Input is read on the **main thread** with `event::poll`; only the streaming
//! reply runs on a background thread (it just *sends* on a channel, never reads
//! stdin). That matters: `insert_before` and viewport init query the cursor
//! position over stdin, and a second thread reading stdin would steal the reply
//! to that query — the source of the "cursor position could not be read" error.
//!
//! On **any width change** the visible conversation needs re-wrapping, so `App`
//! retains a `history` of finished messages and we repaint from it — see
//! [`reflow_after_resize`].

use std::io;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::text::Line;

use inline_tui::app::{Action, App, Role};
use inline_tui::stream::{CancelToken, DummyAi, ReplySource, StreamEvent};
use inline_tui::term::InlineViewport;
use inline_tui::ui;

/// How long to wait for a keypress before checking for streamed chunks.
/// Short while a reply streams (so it's snappy), longer when idle (less spin).
const POLL_STREAMING: Duration = Duration::from_millis(20);
const POLL_IDLE: Duration = Duration::from_millis(200);

fn main() -> io::Result<()> {
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;
    let result = run(&mut term);
    // Always restore the terminal (raw mode off, cursor below the box), even if
    // the loop bailed out with an I/O error — then surface the first error.
    let restored = term.restore();
    result.and(restored)
}

/// The event loop. Reads keys on this thread; the reply streams in on a channel.
fn run(term: &mut InlineViewport) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let mut app = App::new();
    // The reply backend. Swap this single line for a real model (any
    // `ReplySource`) and nothing else in the loop has to change.
    let backend = DummyAi;
    // The in-flight reply's cancel token + thread handle, so a quit mid-stream
    // can stop and reap it cleanly. `None` whenever no reply is streaming.
    let mut inflight: Option<(CancelToken, JoinHandle<()>)> = None;
    // How many lines of the in-progress reply have been flushed to scrollback.
    let mut committed = 0usize;
    let mut dirty = true; // redraw only when something changed

    loop {
        if dirty {
            draw(term, &app)?;
            dirty = false;
        }

        // 1. Keyboard / resize. The wait is short while streaming so chunks stay
        //    snappy, longer when idle so we don't spin.
        let timeout = if app.is_streaming() {
            POLL_STREAMING
        } else {
            POLL_IDLE
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match app.on_key(key) {
                        Action::Quit => break,
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
                    }
                    dirty = true;
                }
                Event::Resize(width, height) => {
                    if term.resized(width, height) {
                        reflow_after_resize(term, &app, &mut committed)?;
                    }
                    dirty = true;
                }
                _ => {}
            }
        }

        // 2. Drain any reply chunks that have arrived, committing stable lines.
        while let Ok(stream_event) = rx.try_recv() {
            let width = term.screen().width;
            match stream_event {
                StreamEvent::Chunk(chunk) => {
                    app.push_chunk(&chunk);
                    if let Some(text) = app.streaming_text() {
                        let (lines, new_committed) = ui::stable_commit(text, width, committed);
                        term.insert_before(lines)?;
                        committed = new_committed;
                    }
                }
                StreamEvent::StreamDone => {
                    if let Some(text) = app.finish_stream() {
                        term.insert_before(ui::final_commit(&text, width, committed))?;
                        term.insert_before(vec![Line::default()])?; // blank spacer
                    }
                    committed = 0;
                    inflight = None;
                }
                StreamEvent::Error(message) => {
                    if let Some(failure) = app.fail_stream(&message) {
                        // Flush whatever streamed before the failure, then the red
                        // error notice — each with a trailing blank spacer,
                        // mirroring how a resize repaints them from history.
                        if let Some(partial) = failure.partial {
                            term.insert_before(ui::final_commit(&partial, width, committed))?;
                            term.insert_before(vec![Line::default()])?;
                        }
                        term.insert_before(ui::message_lines(Role::Error, &failure.error, width))?;
                        term.insert_before(vec![Line::default()])?;
                    }
                    committed = 0;
                    inflight = None;
                }
            }
            dirty = true;
        }
    }

    // Stop any in-flight reply promptly and reap its thread on the way out.
    if let Some((cancel, handle)) = inflight.take() {
        cancel.cancel();
        let _ = handle.join();
    }
    Ok(())
}

/// Repaint the conversation, re-wrapped to the new width, after a resize.
///
/// The width change makes every wrapped line stale, so we repaint the tail that
/// fits above the live region from `App`'s retained history. Resetting
/// `committed` lets any in-progress reply re-commit itself from scratch on its
/// next chunk, so a mid-stream resize recovers too.
fn reflow_after_resize(
    term: &mut InlineViewport,
    app: &App,
    committed: &mut usize,
) -> io::Result<()> {
    let screen = term.screen();
    let height = ui::live_height(&app.input, screen.width, screen.height, app.is_streaming());
    let budget = ui::repaint_budget(screen.height, height);
    let tail = ui::repaint_lines(&app.history, screen.width, budget);
    term.reflow(tail, height)?;
    *committed = 0;
    Ok(())
}

/// Render the live region at its current grown height and place the cursor (only
/// while editing — it's hidden while a reply streams).
fn draw(term: &mut InlineViewport, app: &App) -> io::Result<()> {
    let screen = term.screen();
    let height = ui::live_height(&app.input, screen.width, screen.height, app.is_streaming());
    // The cursor sits at the end of the input; `term` places it from the final
    // (content-anchored) viewport, so we just say whether we're editing.
    let cursor = (!app.is_streaming()).then_some(app.input.as_str());
    term.draw(height, |area, buf| ui::render_live(area, buf, app), cursor)
}

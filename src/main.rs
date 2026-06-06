//! Thin terminal shell around the [`inline_tui`] library.
//!
//! This file is the one place that touches the real terminal, so it is
//! intentionally tiny and free of logic worth unit-testing — all of that lives
//! in `app`, `ui`, and `stream`. Its job is only to:
//!
//! 1. start an inline viewport (no alternate screen, scrollback preserved),
//! 2. read keyboard input and drain streamed chunks in one loop,
//! 3. translate the [`App`]'s decisions into `insert_before` / `draw` calls.
//!
//! Input is read on the **main thread** with `event::poll`; only the streaming
//! reply runs on a background thread (it just *sends* on a channel, never reads
//! stdin). That matters: `insert_before` and terminal init query the cursor
//! position over stdin, and a second thread reading stdin would steal the reply
//! to that query — the source of the "cursor position could not be read" error.
//!
//! On **any width change** the visible conversation needs re-wrapping: ratatui
//! clears the screen on a width *shrink* (to avoid stale wrapping) but not on a
//! *grow*, and either way the on-screen lines keep their old wrapping until we
//! redraw them. We repaint from `App`'s retained history — see
//! [`repaint_after_resize`].

use std::io;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::TerminalOptions;
use ratatui::Viewport;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::terminal;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget};

use inline_tui::app::{Action, App, Role};
use inline_tui::stream::{CancelToken, DummyAi, ReplySource, StreamEvent};
use inline_tui::ui;

/// How long to wait for a keypress before checking for streamed chunks.
/// Short while a reply streams (so it's snappy), longer when idle (less spin).
const POLL_STREAMING: Duration = Duration::from_millis(20);
const POLL_IDLE: Duration = Duration::from_millis(200);

fn main() -> io::Result<()> {
    // Size the inline viewport up front, clamped to the current terminal.
    let rows = terminal::size().map(|(_, h)| h).unwrap_or(24).max(1);
    let height = ui::LIVE_HEIGHT.min(rows);
    let mut term = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(height),
    });

    let result = run(&mut term);

    ratatui::restore();
    println!(); // drop the shell prompt below the leftover input box
    result
}

/// The event loop. Reads keys on this thread; the reply streams in on a channel.
fn run(term: &mut DefaultTerminal) -> io::Result<()> {
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
    // Last terminal width, so we only repaint when the width actually changes
    // (a height-only resize doesn't re-wrap anything).
    let mut last_width = term_width(term)?;
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
                            app.record_user_message(&text);
                            flush_to_scrollback(
                                term,
                                ui::message_lines(Role::User, &text, term_width(term)?),
                            )?;
                            flush_to_scrollback(term, vec![Line::default()])?;
                            app.begin_stream();
                            committed = 0;
                            let cancel = CancelToken::new();
                            let handle = backend.spawn(text, tx.clone(), cancel.clone());
                            inflight = Some((cancel, handle));
                        }
                    }
                    dirty = true;
                }
                Event::Resize(..) => {
                    let new_width = term_width(term)?;
                    if new_width != last_width {
                        repaint_after_resize(term, &app, &mut committed)?;
                    }
                    last_width = new_width;
                    dirty = true;
                }
                _ => {}
            }
        }

        // 2. Drain any reply chunks that have arrived, committing stable lines.
        while let Ok(stream_event) = rx.try_recv() {
            match stream_event {
                StreamEvent::Chunk(chunk) => {
                    app.push_chunk(&chunk);
                    if let Some(text) = app.streaming_text() {
                        let (lines, new_committed) =
                            ui::stable_commit(text, term_width(term)?, committed);
                        flush_to_scrollback(term, lines)?;
                        committed = new_committed;
                    }
                }
                StreamEvent::StreamDone => {
                    if let Some(text) = app.finish_stream() {
                        flush_to_scrollback(
                            term,
                            ui::final_commit(&text, term_width(term)?, committed),
                        )?;
                        flush_to_scrollback(term, vec![Line::default()])?; // blank spacer
                    }
                    committed = 0;
                    inflight = None;
                }
                StreamEvent::Error(message) => {
                    if let Some(failure) = app.fail_stream(&message) {
                        let width = term_width(term)?;
                        // Flush whatever streamed before the failure, then the
                        // red error notice — each with a trailing blank spacer,
                        // mirroring how a resize repaints them from history.
                        if let Some(partial) = failure.partial {
                            flush_to_scrollback(
                                term,
                                ui::final_commit(&partial, width, committed),
                            )?;
                            flush_to_scrollback(term, vec![Line::default()])?;
                        }
                        flush_to_scrollback(
                            term,
                            ui::message_lines(Role::Error, &failure.error, width),
                        )?;
                        flush_to_scrollback(term, vec![Line::default()])?;
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
/// On a width *shrink* ratatui clears the screen (stale wrapping); on a *grow* it
/// doesn't — but in both cases the visible conversation keeps its old wrapping
/// until we redraw it. We force a clean repaint that works either way:
///
/// 1. Park the cursor at row 0. ratatui anchors the inline viewport to the
///    cursor row, so the following `resize` seats the viewport at the top of the
///    screen and clears the whole visible area from there down.
/// 2. Re-commit the tail of the history that fits above the live region. With
///    the viewport at the top, `insert_before` fills the screen *without*
///    scrolling — the tmux-safe path — re-wrapped to the new width. Older lines
///    survive in the terminal's own scrollback.
///
/// Resetting `committed` lets any in-progress reply re-commit itself from scratch
/// on its next chunk, so a mid-stream resize recovers too.
fn repaint_after_resize(
    term: &mut DefaultTerminal,
    app: &App,
    committed: &mut usize,
) -> io::Result<()> {
    let size = term.size()?;
    term.set_cursor_position(Position::new(0, 0))?;
    term.resize(Rect::new(0, 0, size.width, size.height))?;
    let max_rows = ui::repaint_budget(size.height);
    flush_to_scrollback(term, ui::repaint_lines(&app.history, size.width, max_rows))?;
    *committed = 0;
    Ok(())
}

/// Render the live region and place the cursor (only while editing).
fn draw(term: &mut DefaultTerminal, app: &App) -> io::Result<()> {
    term.draw(|frame| {
        let area = frame.area();
        ui::render_live(area, frame.buffer_mut(), app);
        if !app.is_streaming() {
            let (x, y) = ui::cursor_position(area, &app.input);
            frame.set_cursor_position(Position::new(x, y));
        }
    })?;
    Ok(())
}

/// Push pre-wrapped lines into the terminal's scrollback above the viewport.
fn flush_to_scrollback(term: &mut DefaultTerminal, lines: Vec<Line<'static>>) -> io::Result<()> {
    let height = lines.len() as u16;
    if height == 0 {
        return Ok(());
    }
    term.insert_before(height, |buf: &mut Buffer| {
        Paragraph::new(Text::from(lines)).render(buf.area, buf);
    })
}

/// Current terminal width (used to wrap messages responsively).
fn term_width(term: &DefaultTerminal) -> io::Result<u16> {
    Ok(term.size()?.width)
}

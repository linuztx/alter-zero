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
//! only **queues** its lines (codex's pending-history pattern): the draw tick
//! writes them and repaints the live region in one synchronized frame, so
//! scrollback growth never flashes a missing box (see `docs/flicker.md`).
//!
//! On **any width change** (and on returning from the tool-output overlay) the
//! visible conversation needs re-wrapping, so `App` retains a `history` and we
//! repaint from it — see [`repaint_conversation`].

use std::io;
use std::path::PathBuf;
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
use inline_tui::stream::{self, CancelToken, DummyAi, ReplySource, StreamEvent};
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
    // `ReplySource`) and nothing else in the loop has to change. The dummy
    // pauses before streaming so the status indicator shows first; the pause is
    // `STARTUP_DELAY` unless `INLINE_TUI_STARTUP_DELAY_MS` overrides it (the
    // smoke test runs with a short delay; one phase uses a longer one).
    let startup_delay = std::env::var("INLINE_TUI_STARTUP_DELAY_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map_or(stream::STARTUP_DELAY, Duration::from_millis);
    let backend = DummyAi::with_startup_delay(startup_delay);
    // Session context for the footer under the box — the backend's model name
    // and the cwd — formatted here at the boundary (the set_clock pattern: the
    // pure core never reads the environment). See docs/footer.md.
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    app.set_session_info(backend.model_name(), ui::display_cwd(&cwd, home.as_deref()));
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
    let mut clocks = StatusClocks::default();

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
                                // history (the reflow paints the box in the same
                                // frame) the same way a normal Ctrl+O return does;
                                // otherwise restore() lands on the stale live status
                                // strip ("Working… (… tokens)") instead of the
                                // committed "Done for Ns" summary.
                                if app.view == View::ToolOutput {
                                    term.exit_overlay()?;
                                    repaint_conversation(term, &app, &mut committed)?;
                                }
                                break;
                            }
                            Action::None => {}
                            Action::Submit(text) => {
                                inflight = Some(start_turn(
                                    term, &mut app, &tx, &backend, vec![text],
                                    &mut committed, &mut clocks,
                                )?);
                            }
                            Action::RunShell(command) => {
                                // `!command` from an idle composer: echo it, then
                                // run it locally as a turn (docs/shell-command.md).
                                // Reuses the streamed-reply channel + inflight
                                // handle, so the status strip, Esc-interrupt, and
                                // resize repaint all work exactly like an AI turn.
                                inflight = Some(run_shell(
                                    term, &mut app, &tx, command,
                                    &mut committed, &mut clocks,
                                )?);
                            }
                            Action::ToggleToolView => {
                                // on_key already flipped app.view; sync the overlay.
                                if app.view == View::ToolOutput {
                                    term.enter_overlay()?;
                                    // Paint the overlay now rather than on the next
                                    // tick — the freshly-cleared alt screen would
                                    // show as a black flash for a frame otherwise.
                                    draw_tool_view(term, &mut app)?;
                                } else {
                                    term.exit_overlay()?;
                                    // Catch the inline view up on whatever streamed
                                    // while the overlay was showing.
                                    repaint_conversation(term, &app, &mut committed)?;
                                    // A turn may have ended while the overlay was up
                                    // (its queue flush was deferred — invariant 4);
                                    // now that we commit inline again, send the next
                                    // queued batch (later batches flush at each
                                    // following turn-end, see the StreamDone arm).
                                    if inflight.is_none() {
                                        let batch = app.drain_next_batch();
                                        if !batch.is_empty() {
                                            inflight = Some(start_turn(
                                                term, &mut app, &tx, &backend, batch,
                                                &mut committed, &mut clocks,
                                            )?);
                                        }
                                    }
                                }
                            }
                            Action::Notice(text) => {
                                // A slash command's one-off system notice. If a reply
                                // is mid-flight, finalise its current segment first
                                // (same ordering trick as a tool call) so the notice
                                // slots after it in scrollback and history alike.
                                let width = term.screen().width;
                                if let Some(segment) = app.flush_streaming_segment() {
                                    term.insert_before(ui::final_commit(&segment, width, committed));
                                    term.insert_before(vec![Line::default()]);
                                    committed = 0;
                                }
                                app.record_system_message(&text);
                                term.insert_before(ui::message_lines(Role::System, &text, width));
                                term.insert_before(vec![Line::default()]);
                            }
                            Action::Clear => {
                                // `/clear` already wiped the app state (history,
                                // streaming buffer, running tool, status, queued
                                // backlog). Mid-turn it is also a kill — the user
                                // asked for a fresh slate, not a finished turn — so
                                // stop + reap the backend and drain the channel
                                // (the Esc-interrupt dance, minus the commits:
                                // a stale chunk or ToolStart processed after the
                                // wipe would repopulate the cleared state and
                                // stream into the blank screen).
                                if let Some((cancel, handle)) = inflight.take() {
                                    cancel.cancel();
                                    let _ = handle.join();
                                }
                                while reply_rx.try_recv().is_ok() {}
                                clocks.turn_start = None;
                                clocks.thinking_start = None;
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
                                        term.insert_before(ui::final_commit(&partial, width, committed));
                                        term.insert_before(vec![Line::default()]);
                                    }
                                    if let Some(tool) = interrupted.tool {
                                        term.insert_before(ui::tool_lines(&tool, width));
                                        term.insert_before(vec![Line::default()]);
                                    }
                                    term.insert_before(ui::message_lines(
                                        Role::Error,
                                        INTERRUPT_NOTICE,
                                        width,
                                    ));
                                    term.insert_before(vec![Line::default()]);
                                }
                                committed = 0;
                                clocks.turn_start = None;
                                clocks.thinking_start = None;
                                // The user interrupted to send their queued
                                // follow-ups right away (their spec; codex's
                                // submit-pending-steers-after-interrupt). The
                                // front batch — the first queue — goes out now;
                                // any Tab-opened follow-up batches iterate at the
                                // following turn-ends. Interrupt only arises in the
                                // conversation view, so committing here is safe.
                                let batch = app.drain_next_batch();
                                if !batch.is_empty() {
                                    inflight = Some(start_turn(
                                        term, &mut app, &tx, &backend, batch,
                                        &mut committed, &mut clocks,
                                    )?);
                                }
                            }
                        }
                        schedule_for_key(&frame, &mut burst, &key);
                    }
                    Event::Resize(width, height) => {
                        let size_changed = term.resized(width, height);
                        // Repaint from history on ANY dimension change (codex
                        // redraws from source on every resize): a width change
                        // stales the wrapping, and a height change moves the
                        // screen contents out from under the tracked viewport
                        // row — repainting at a stale row leaves phantom input
                        // boxes behind. Only the inline view reflows; the
                        // overlay just redraws at the new size (reflowing would
                        // write the alternate screen).
                        if size_changed && app.view == View::Conversation {
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
                    term, &mut app, &mut committed, &mut clocks, stream_event,
                )? {
                    inflight = None; // the stream ended
                    // Send the next queued batch as the following turn — Enter
                    // messages batch into one turn, while Tab-opened follow-up
                    // batches each flush at their own turn-end, so they iterate in
                    // order. Only in the conversation view; committing under the
                    // Ctrl+O overlay would violate the no-scrollback-in-overlay
                    // invariant. A turn that ends in the overlay flushes on return
                    // instead (see ToggleToolView).
                    if app.view == View::Conversation {
                        let batch = app.drain_next_batch();
                        if !batch.is_empty() {
                            inflight = Some(start_turn(
                                term, &mut app, &tx, &backend, batch,
                                &mut committed, &mut clocks,
                            )?);
                        }
                    }
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

/// The live status indicator's clocks, bundled (the impurity kept at the
/// boundary): when the turn was submitted — driving the elapsed timer and the
/// verb's shimmer phase — and when the current thinking phase began (`None`
/// outside one). Reset at each turn start; the pure `App` only ever sees the
/// *computed* durations, via `set_status_times`. See docs/status-indicator.md.
#[derive(Default)]
struct StatusClocks {
    turn_start: Option<Instant>,
    thinking_start: Option<Instant>,
}

/// Start one turn for `texts` (a Submit is a batch of one; a queue flush sends
/// one batch — the Enter messages sharing that turn — as a single turn, the
/// Tab-opened batches flushing across later turns): record + commit each user
/// bullet to scrollback, open the stream, reset the per-turn clocks and commit
/// counter, and spawn the backend on the joined prompt. Returns the in-flight
/// cancel token + thread handle. Shared by the `Submit` key arm *and* every
/// queue flush (`StreamDone`/`Error`, an Esc interrupt, or a Ctrl+O return), so
/// the paths can never drift. Empty batches are the caller's job to
/// skip. See `docs/queue.md`.
fn start_turn(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    backend: &impl ReplySource,
    texts: Vec<String>,
    committed: &mut usize,
    clocks: &mut StatusClocks,
) -> io::Result<(CancelToken, JoinHandle<()>)> {
    let width = term.screen().width;
    for text in &texts {
        app.record_user_message(text);
        term.insert_before(ui::message_lines(Role::User, text, width));
        term.insert_before(vec![Line::default()]);
    }
    app.begin_stream();
    // Count the user's uploaded input into the tally (arrow ↑) so the status
    // shows `↑ N tokens` during the backend's pre-stream pause, before its
    // first chunk flips the arrow back to ↓.
    let prompt = texts.join("\n");
    app.count_user_input(&prompt);
    *committed = 0;
    // Start the turn clock; the draw branch keeps the status animated from here.
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    let cancel = CancelToken::new();
    let handle = backend.spawn(prompt, tx.clone(), cancel.clone());
    Ok((cancel, handle))
}

/// Run a `!command` locally as a turn (the [`Action::RunShell`] arm; see
/// `docs/shell-command.md`). Echoes `❯ !command` to scrollback + history,
/// calls [`App::begin_shell`] (which sets up the status strip with the command
/// as its running tool), then spawns [`spawn_shell_command`] on the same
/// streamed-reply channel — so the existing `ToolEnd`/`StreamDone` arms commit
/// the cell + the `Ran for Ns` summary, and Esc routes through the normal
/// interrupt path. Returns the in-flight cancel token + thread handle, like
/// [`start_turn`].
fn run_shell(
    term: &mut InlineViewport,
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    command: String,
    committed: &mut usize,
    clocks: &mut StatusClocks,
) -> io::Result<(CancelToken, JoinHandle<()>)> {
    let width = term.screen().width;
    // begin_shell records the cell's `! command` header (Role::Shell) in
    // history; commit it with NO trailing blank — the `⎿ Running…` preview
    // (and later the committed `⎿` output) sits flush below it, forming the
    // codex-style exec cell (docs/shell-command.md).
    app.begin_shell(&command);
    term.insert_before(ui::message_lines(Role::Shell, &command, width));
    *committed = 0;
    clocks.turn_start = Some(Instant::now());
    clocks.thinking_start = None;
    let cancel = CancelToken::new();
    let handle = spawn_shell_command(command, tx.clone(), cancel.clone());
    Ok((cancel, handle))
}

/// Run `command` under `sh -c` on a background thread, streaming the result back
/// on the reply channel as a `ToolEnd`/`StreamDone` pair (the command was
/// already shown as the running tool by [`App::begin_shell`]). Reader threads
/// drain stdout/stderr so a chatty command can't deadlock on a full pipe; the
/// loop polls `cancel` so an Esc interrupt kills the child and reaps it. Output
/// is stdout then stderr; a non-zero exit appends `[exit status: N]` and
/// resolves the cell red. The I/O boundary — verified by `scripts/smoke.sh`
/// (Phase 19), not unit tests. See `docs/shell-command.md`.
fn spawn_shell_command(
    command: String,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancel: CancelToken,
) -> JoinHandle<()> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    std::thread::spawn(move || {
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output: format!("failed to run command: {err}"),
                    ok: false,
                    saved: None,
                });
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
        };

        // Drain both pipes on their own threads so a command that writes more
        // than the pipe buffer can't block (and thus never exit) while we wait.
        let read_pipe = |pipe: Option<std::process::ChildStdout>| {
            std::thread::spawn(move || {
                let mut buf = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_string(&mut buf);
                }
                buf
            })
        };
        let out_reader = read_pipe(child.stdout.take());
        // ChildStderr → ChildStdout type mismatch, so read stderr inline-typed.
        let mut err_pipe = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(pipe) = err_pipe.as_mut() {
                let _ = pipe.read_to_string(&mut buf);
            }
            buf
        });

        // Wait for the child, polling so an Esc interrupt (cancel) kills it.
        let status = loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                // The interrupt path (App::interrupt_turn) owns the UI from
                // here — resolve the tool failed, commit the notice. Send
                // nothing and return at once so the loop's `handle.join()`
                // unblocks promptly. We deliberately do **not** join the reader
                // threads: killing `sh` can leave a reparented grandchild (e.g.
                // `sleep`) holding the pipe's write end, so `read_to_string`
                // would block until *it* dies. Detached, the readers finish
                // harmlessly when that happens (they only drop a buffer).
                return;
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(SHELL_POLL_INTERVAL),
                Err(err) => {
                    let _ = out_reader.join();
                    let _ = err_reader.join();
                    let _ = tx.send(StreamEvent::ToolEnd {
                        output: format!("error waiting on command: {err}"),
                        ok: false,
                        saved: None,
                    });
                    let _ = tx.send(StreamEvent::StreamDone);
                    return;
                }
            }
        };

        let mut output = out_reader.join().unwrap_or_default();
        let stderr = err_reader.join().unwrap_or_default();
        if !stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&stderr);
        }
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
        // Output too large to keep? Write it to a file and stream only a
        // preview, so the cell shows an `Output too large …` block instead of
        // dumping megabytes into scrollback / memory (Claude-Code's behaviour).
        let (output, saved) = save_if_too_large(output);
        let _ = tx.send(StreamEvent::ToolEnd { output, ok, saved });
        let _ = tx.send(StreamEvent::StreamDone);
    })
}

/// How often [`spawn_shell_command`] polls a running child for completion while
/// watching for an interrupt — short enough that Esc kills it promptly.
const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Above this many bytes a `!` shell command's output is **saved to a file**
/// rather than kept: only a preview (`ui::SHELL_PREVIEW_BYTES`) is streamed
/// back, and the cell renders an `Output too large …` block. See
/// `docs/shell-command.md`.
const SHELL_OUTPUT_MAX_BYTES: usize = 100_000;

/// If `output` exceeds [`SHELL_OUTPUT_MAX_BYTES`], write the **full** output to
/// a unique temp file and return `(preview, Some((path, total_bytes)))` — the
/// preview is its first `ui::SHELL_PREVIEW_BYTES` (cut on a char boundary).
/// Otherwise (or if the write fails) returns `(output, None)` unchanged. The
/// I/O boundary — smoke-covered (Phase 22), not unit-tested.
fn save_if_too_large(output: String) -> (String, Option<(String, u64)>) {
    if output.len() <= SHELL_OUTPUT_MAX_BYTES {
        return (output, None);
    }
    let total_bytes = output.len() as u64;
    let mut cut = ui::SHELL_PREVIEW_BYTES.min(output.len());
    while cut > 0 && !output.is_char_boundary(cut) {
        cut -= 1;
    }
    let preview = output[..cut].to_string();
    // A unique name so concurrent / repeated saves don't collide.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "inline-tui-shell-{}-{nanos}.txt",
        std::process::id()
    ));
    match std::fs::write(&path, &output) {
        Ok(()) => (
            preview,
            Some((path.to_string_lossy().into_owned(), total_bytes)),
        ),
        // Couldn't save — fall back to showing the full output unsaved.
        Err(_) => (output, None),
    }
}

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
    committed: &mut usize,
    clocks: &mut StatusClocks,
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
                term.insert_before(lines);
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
                term.insert_before(ui::final_commit(&segment, width, *committed));
                term.insert_before(vec![Line::default()]);
            }
            *committed = 0;
            app.start_tool(&name, &args);
            Ok(false)
        }
        StreamEvent::ToolEnd { output, ok, saved } => {
            // A too-large `!` output was saved to a file (the runner did the
            // I/O); mark the running tool so its cell renders the `Output too
            // large …` block (before end_tool takes it). `output` is the preview.
            if let Some((path, total_bytes)) = saved {
                app.set_tool_saved(path, total_bytes);
            }
            // Commit the finished tool *collapsed* (green/red) to scrollback; its
            // full output lives in the Ctrl+O view (or the saved file).
            if let Some(tool) = app.end_tool(&output, ok)
                && committing
            {
                term.insert_before(ui::tool_lines(&tool, width));
                term.insert_before(vec![Line::default()]);
            }
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
        StreamEvent::ThinkingEnd => {
            clocks.thinking_start = None;
            Ok(false)
        }
        StreamEvent::StreamDone => {
            let final_text = app.finish_stream();
            let elapsed = clocks
                .turn_start
                .map_or(0, |start| start.elapsed().as_secs());
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
                    term.insert_before(ui::final_commit(&text, width, *committed));
                    term.insert_before(vec![Line::default()]); // blank spacer
                }
                if let Some(summary) = summary {
                    term.insert_before(ui::summary_lines(&summary, width));
                    term.insert_before(vec![Line::default()]); // blank spacer
                }
            }
            *committed = 0;
            clocks.turn_start = None;
            clocks.thinking_start = None;
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
                        term.insert_before(ui::final_commit(&partial, width, *committed));
                        term.insert_before(vec![Line::default()]);
                    }
                    term.insert_before(ui::message_lines(Role::Error, &failure.error, width));
                    term.insert_before(vec![Line::default()]);
                }
            }
            *committed = 0;
            clocks.turn_start = None;
            clocks.thinking_start = None;
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
fn update_status_times(app: &mut App, clocks: &StatusClocks) {
    let elapsed = clocks
        .turn_start
        .map_or(Duration::ZERO, |start| start.elapsed());
    let thinking = clocks.thinking_start.map(|start| start.elapsed());
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
    let band = ui::menu_rows(app) + ui::shortcuts_rows(app);
    ui::live_height(
        &app.input,
        screen.width,
        screen.height,
        app.is_streaming(),
        ui::strip_has_preview(app),
        ui::queued_rows(app, screen.width),
        band,
        ui::footer_rows(app, band),
    )
}

/// Repaint the inline conversation from `App`'s retained history, re-wrapped to
/// the current width — tail and live region in one synchronized frame
/// ([`InlineViewport::reflow`]). Used both after a resize *and* when returning
/// from the tool-output overlay (which kept the stream advancing without
/// committing).
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
    term.reflow(
        tail,
        height,
        |area, buf| ui::render_live(area, buf, app),
        app,
    )?;
    *committed = 0;
    Ok(())
}

/// Render the live region at its current grown height and place the cursor.
/// The composer keeps its cursor even while a reply streams (codex-style —
/// typing mid-turn edits the draft, Enter queues it); only the Ctrl+O overlay
/// hides it (`enter_overlay`).
fn draw(term: &mut InlineViewport, app: &App) -> io::Result<()> {
    let height = live_region_height(app, term.screen());
    // `term` places the cursor from the final (content-anchored) viewport via
    // `ui::cursor_position`, which mirrors render_live's layout exactly.
    term.draw(height, |area, buf| ui::render_live(area, buf, app), app)
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

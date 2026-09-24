//! What an interactive session's threads share (`docs/interactive-shell.md`):
//! the two views of its output, when output last arrived, whether it has
//! exited — and the handshake that decides **who reports the exit**.
//!
//! Three parties touch a session. Its **monitor** (a thread in
//! [`crate::background`]) feeds every chunk of output in and records the
//! exit. A **waiting call** — `bash` with `tty`, or `bash_session` — blocks
//! until the session settles ([`super::settle`]) and then takes a **look**:
//! the model-facing report of what happened since the previous one. And the
//! **event loop** hears of the session through the registry's events.
//!
//! The exit is the delicate part. When the model *saw* the exit — a call's
//! report said `Exit code: N` — a `[background] … completed` notice after it
//! would be noise at best and an automatic follow-up turn at worst. So the
//! party that finishes last **finalizes**: a monitor that finishes with no
//! call waiting finalizes at once, unobserved ([`Finish::Now`]); one that
//! finishes while a call waits leaves it to that call ([`Finish::Waiter`]),
//! which finalizes on its way out ([`SessionIo::end_wait`]) — observed
//! exactly when the report it composed covered the exit.
//!
//! No process here: the tests drive it from threads standing in for the
//! monitor, the way the agent loop is tested with fake rounds.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::probe::Probe;
use super::report::{self, Status, View};
use super::screen::Screen;
use super::settle::{self, Observation, Settle, WaitKind};
use super::transcript::Transcript;

/// How often a waiting call wakes, with nothing new, to re-ask its cancel and
/// hand-off predicates — short enough that Esc and Ctrl+B act at once.
const WAIT_POLL: Duration = Duration::from_millis(20);

/// The longest a **burst** of output lasts ([`Transcript::new_burst`]): the
/// writes a program makes for one frame (an erase, a move, the text) land
/// inside it, and a program redrawing a line — however fast it animates —
/// changes it again in a later one.
const BURST_SPAN: Duration = Duration::from_millis(100);

/// How often a waiting call streams the session to its running cell, at
/// most — a redrawn progress bar reaches the screen at a steady pace instead
/// of once per write (`docs/interactive-shell.md`).
const STREAM_INTERVAL: Duration = Duration::from_millis(50);

/// How long a terminal a call waits on must be quiet — no output, no input —
/// before the monitor probes what its program is blocked in
/// ([`SessionIo::wants_probe`]): soon enough for the answer to be in by
/// [`settle::PROMPT_QUIET`], late enough that a program mid-answer is not
/// caught between its reads.
pub const PROBE_QUIET: Duration = Duration::from_millis(200);

/// The most settled text one waiting call streams to its running cell; past
/// it only the live rows keep moving. The cell is replaced by the report
/// when the call ends, and the report keeps its own tail.
const STREAM_MAX_BYTES: usize = 64 * 1024;

/// See the module docs.
pub struct SessionIo {
    state: Mutex<IoState>,
    changed: Condvar,
}

impl std::fmt::Debug for SessionIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionIo").finish_non_exhaustive()
    }
}

struct IoState {
    /// When the session was created — the origin a launch waits from.
    created: Instant,
    transcript: Transcript,
    /// The screen — a TTY session's alone; a pipe has no screen to show.
    screen: Option<Screen>,
    /// Output chunks absorbed so far — what a [`Mark`] compares against.
    seq: u64,
    last_output: Option<Instant>,
    /// The monitor reaped the process and drained its output.
    finished: bool,
    code: Option<i32>,
    /// Calls blocked in [`SessionIo::wait`] (and not yet through `end_wait`).
    waiters: usize,
    /// A report composed since the exit said so.
    exit_reported: bool,
    /// The exit has been finalized (by the monitor or a waiter).
    finalized: bool,
    /// The event loop has been told about the session.
    announced: bool,
    /// The program reads its terminal key by key (raw mode) — a menu, an
    /// editor, a readline prompt — so it waits on keys wherever its cursor
    /// sits ([`IoState::awaiting_keys`]).
    reading_keys: bool,
    /// When the current burst of output began ([`BURST_SPAN`]) — `None`
    /// before any, and after input, whose answer starts a burst of its own.
    burst_began: Option<Instant>,
    /// What the monitor's probe last saw the program blocked in
    /// ([`super::probe`]) — [`Probe::Unknown`] again the moment output or
    /// input moves the session on.
    probe: Probe,
    /// When input was last written — quiet counts from it too, for the probe.
    last_input: Option<Instant>,
    /// The terminal reads a whole line with echo off — a password prompt
    /// (`pty::spawn::LineMode::hides_input`, read by the monitor).
    hidden_input: bool,
    /// [`IoState::seq`] when input was last written: output past it is the
    /// program's answer ([`IoState::password_prompt`]).
    input_seq: u64,
}

/// A point in a session's output history — taken before a call writes its
/// input (or at launch) so the output that answers it is counted from there,
/// even the echo that arrives before the call starts waiting.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    at: Instant,
    seq: u64,
}

/// Why a waiting call stopped waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitEnd {
    /// The session settled.
    Settled(Settle),
    /// The call's own cancel fired (Esc).
    Cancelled,
    /// The hand-off predicate fired (Ctrl+B on the running command).
    Handoff,
}

/// Who finalizes an exit the monitor has just recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// Nobody is waiting: the monitor finalizes it now, unobserved.
    Now,
    /// A call is waiting; it finalizes on its way out.
    Waiter,
}

impl SessionIo {
    /// A session's shared state — with a screen for a TTY session.
    #[must_use]
    pub fn new(tty: bool) -> Self {
        Self::with_waiters(tty, 0)
    }

    /// [`new`](Self::new), with the launching call already registered as a
    /// waiter — so a command that exits before the call gets to wait still
    /// leaves its exit for that call to report.
    #[must_use]
    pub fn waited(tty: bool) -> Self {
        Self::with_waiters(tty, 1)
    }

    fn with_waiters(tty: bool, waiters: usize) -> Self {
        Self {
            state: Mutex::new(IoState {
                created: Instant::now(),
                transcript: Transcript::new(),
                screen: tty.then(Screen::default),
                seq: 0,
                last_output: None,
                finished: false,
                code: None,
                waiters,
                exit_reported: false,
                finalized: false,
                announced: false,
                reading_keys: false,
                burst_began: None,
                probe: Probe::Unknown,
                last_input: None,
                hidden_input: false,
                input_seq: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, IoState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Does the session have a terminal (and so a screen, and input)?
    #[must_use]
    pub fn is_tty(&self) -> bool {
        self.lock().screen.is_some()
    }

    /// Now, in the session's output history (see [`Mark`]).
    #[must_use]
    pub fn mark(&self) -> Mark {
        Mark {
            at: Instant::now(),
            seq: self.lock().seq,
        }
    }

    /// Register a call as a waiter — **before** it acts on the session — and
    /// return the [`Mark`] its wait counts from. Every `begin_wait` must be
    /// followed by [`end_wait`](Self::end_wait).
    pub fn begin_wait(&self) -> Mark {
        let mut state = self.lock();
        state.waiters += 1;
        Mark {
            at: Instant::now(),
            seq: state.seq,
        }
    }

    /// The session's very beginning, in its output history — what a launch
    /// waits from, so output that arrived before the launching call began to
    /// wait still answers it.
    #[must_use]
    pub fn origin_mark(&self) -> Mark {
        Mark {
            at: self.lock().created,
            seq: 0,
        }
    }

    /// The monitor: fold a chunk of output into both views. Returns the query
    /// replies the terminal owes the program, and the lines the output
    /// finished (for the interim-output file).
    pub fn absorb(&self, bytes: &[u8]) -> (Vec<u8>, String) {
        let mut state = self.lock();
        let replies = state
            .screen
            .as_mut()
            .map(|screen| screen.feed(bytes))
            .unwrap_or_default();
        let now = Instant::now();
        if state
            .burst_began
            .is_none_or(|began| now.saturating_duration_since(began) >= BURST_SPAN)
        {
            state.transcript.new_burst();
            state.burst_began = Some(now);
        }
        state.transcript.feed(bytes);
        let committed = state.transcript.take_committed();
        state.seq += 1;
        state.last_output = Some(Instant::now());
        // Whatever the probe saw, the program has moved on since.
        state.probe = Probe::Unknown;
        drop(state);
        self.changed.notify_all();
        (replies, committed)
    }

    /// The monitor: the process exited (`code`, `None` for a signal) and its
    /// output is drained. Returns the transcript's unfinished tail (for the
    /// interim-output file) and who finalizes the exit.
    pub fn finish(&self, code: Option<i32>) -> (String, Finish) {
        let mut state = self.lock();
        state.finished = true;
        state.code = code;
        let rest = state.transcript_rest();
        let finish = if state.waiters == 0 {
            state.finalized = true;
            Finish::Now
        } else {
            Finish::Waiter
        };
        drop(state);
        self.changed.notify_all();
        (rest, finish)
    }

    /// The exit code once the process has finished — `Some(None)` for a
    /// signal, `None` while it runs.
    #[must_use]
    pub fn exit(&self) -> Option<Option<i32>> {
        let state = self.lock();
        state.finished.then_some(state.code)
    }

    /// Mark the session announced to the event loop; `true` when this call
    /// is the one that did (so it sends the event), `false` if it already was.
    pub fn announce(&self) -> bool {
        !std::mem::replace(&mut self.lock().announced, true)
    }

    /// Has the event loop been told about the session?
    #[must_use]
    pub fn announced(&self) -> bool {
        self.lock().announced
    }

    /// The screen as text, for the ↓ manager — `None` for a pipe session.
    #[must_use]
    pub fn screen_text(&self) -> Option<String> {
        self.lock()
            .screen
            .as_ref()
            .map(|screen| screen.snapshot().rows.join("\n"))
    }

    /// Is the session sitting at a prompt **now** — its screen awaiting keys
    /// and quiet for as long as a call of `kind` that began at `since` needs
    /// ([`settle::prompt_quiet`]), the test a wait settles on? What a report
    /// says even when the wait that preceded it timed out: a poll that saw
    /// nothing new ends on its timeout, but the program is no less at its
    /// prompt for that. Never for a pipe, or once it has exited.
    #[must_use]
    pub fn waiting(&self, kind: WaitKind, since: Mark) -> bool {
        let state = self.lock();
        let quiet = state.last_output.unwrap_or(state.created).elapsed();
        // The probe's read and the terminal's hidden line are exact: no
        // longer quiet than any prompt.
        let exact = state.probe == Probe::Reading || state.password_prompt();
        let needed = if exact {
            settle::PROMPT_QUIET
        } else {
            settle::prompt_quiet(kind, state.seq > since.seq)
        };
        !state.finished && quiet >= needed && (exact || state.awaiting_keys())
    }

    /// Should the monitor ask the kernel what the program is blocked in
    /// ([`super::probe`])? For a terminal a call is waiting on, once it has
    /// been quiet [`PROBE_QUIET`] since the last output or input — never for
    /// a pipe, or once it has exited.
    #[must_use]
    pub fn wants_probe(&self) -> bool {
        let state = self.lock();
        let last = [state.last_output, state.last_input]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(state.created);
        state.screen.is_some()
            && state.waiters > 0
            && !state.finished
            && last.elapsed() >= PROBE_QUIET
    }

    /// Record what the probe saw ([`super::probe::probe`]), waking a waiting
    /// call when it changed.
    pub fn set_probe(&self, probe: Probe) {
        let changed = std::mem::replace(&mut self.lock().probe, probe) != probe;
        if changed {
            self.changed.notify_all();
        }
    }

    /// The program is about to be typed into: what it draws next answers
    /// the keys — a menu moving its highlight, a line editor echoing — so
    /// the redraws it made on its own until now stop counting towards an
    /// animation ([`Transcript::new_input`]). `submits` says whether the keys
    /// reach a program reading whole lines ([`super::keys::reaches_line_reader`]).
    pub fn note_input(&self, submits: bool) {
        let mut state = self.lock();
        state.transcript.new_input();
        state.burst_began = None;
        // The read the probe saw is about to take these keys.
        state.probe = Probe::Unknown;
        state.last_input = Some(Instant::now());
        // Keys that reach a line reader answer its prompt; text still on
        // the line being edited has not, so a password prompt stands.
        if submits {
            state.input_seq = state.seq;
        }
    }

    /// Record whether the terminal reads a line with echo off — a password
    /// prompt (`pty::spawn::LineMode::hides_input`); the monitor reads it off
    /// the terminal as output arrives and on every idle poll.
    pub fn set_hidden_input(&self, hidden: bool) {
        let changed = std::mem::replace(&mut self.lock().hidden_input, hidden) != hidden;
        if changed {
            self.changed.notify_all();
        }
    }

    /// Is the session at a **password prompt** — its terminal reading a line
    /// with echo off, put up since it was last typed into, over no tree the
    /// probe sees at work (`IoState::password_prompt`)? What the report's
    /// frame names (`pty::report::Waiting::Password`).
    #[must_use]
    pub fn password_prompt(&self) -> bool {
        self.lock().password_prompt()
    }

    /// Record whether the program reads its terminal **key by key** (raw
    /// mode, not canonical) — the monitor reads it off the terminal as output
    /// arrives (`pty::spawn::line_mode`).
    pub fn set_reading_keys(&self, reading_keys: bool) {
        let changed = {
            let mut state = self.lock();
            std::mem::replace(&mut state.reading_keys, reading_keys) != reading_keys
        };
        if changed {
            self.changed.notify_all();
        }
    }

    /// Does the line under the cursor end with `typed` — text the call typed
    /// and a line editor echoed, still waiting for its Enter? Never on the
    /// alternate screen, where a full-screen program inserts what it is
    /// typed rather than holding a line.
    #[must_use]
    pub fn holds_typed(&self, typed: &str) -> bool {
        self.lock().screen.as_ref().is_some_and(|screen| {
            !screen.alternate() && !typed.is_empty() && screen.cursor_line().ends_with(typed)
        })
    }

    /// The program's cursor-key mode — how typed arrows are encoded.
    #[must_use]
    pub fn application_cursor(&self) -> bool {
        self.lock()
            .screen
            .as_ref()
            .is_some_and(Screen::application_cursor)
    }

    /// Block until the session settles (see [`super::settle`]) — or the
    /// call's `cancelled` or `handoff` predicate fires — streaming what the
    /// look will report to `stream(settled, live)` as it builds up
    /// ([`Transcript::take_stream`]): `settled` text to append, `live` rows
    /// that replace the ones streamed last. The call must already be a
    /// waiter ([`begin_wait`](Self::begin_wait), or a launch's
    /// [`waited`](Self::waited) session).
    pub fn wait(
        &self,
        kind: WaitKind,
        since: Mark,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
        handoff: &dyn Fn() -> bool,
        stream: &mut dyn FnMut(&str, &str),
    ) -> WaitEnd {
        let mut streamer = Streamer::default();
        loop {
            let (update, seen) = {
                let mut state = self.lock();
                let now = Instant::now();
                let update = streamer.due(now).then(|| state.transcript.take_stream());
                let output = state.seq > since.seq;
                let last = if output {
                    state.last_output.unwrap_or(since.at)
                } else {
                    since.at
                };
                let seen = Observation {
                    elapsed: now.saturating_duration_since(since.at),
                    quiet: now.saturating_duration_since(last),
                    output,
                    awaiting_keys: state.awaiting_keys(),
                    reading: state.probe == Probe::Reading,
                    secret: state.password_prompt(),
                    exited: state.finished,
                };
                (update, seen)
            };
            if let Some(update) = update {
                streamer.send(update, stream);
            }
            if let Some(settled) = settle::settle(kind, timeout, &seen) {
                // One last look, past the pace, so the cell ends where the
                // report begins.
                let update = self.lock().transcript.take_stream();
                streamer.send(update, stream);
                return WaitEnd::Settled(settled);
            }
            if cancelled() {
                return WaitEnd::Cancelled;
            }
            if handoff() {
                return WaitEnd::Handoff;
            }
            // Sleep only when there is nothing to act on — no exit that
            // landed after the observation above.
            let state = self.lock();
            if seen.exited || !state.finished {
                let _ = self
                    .changed
                    .wait_timeout(state, WAIT_POLL)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
    }

    /// The model's look: the report of everything since the previous look
    /// (see [`super::report`]), for a session in `status`. A report that
    /// covers the exit ([`Status::Exited`], [`Status::Stopped`]) is what makes
    /// the exit *observed*.
    pub fn look(&self, session: &str, status: Status) -> String {
        let mut state = self.lock();
        let addressed = state.transcript.screen_addressed();
        let update = state.transcript.take_update();
        let view = match &state.screen {
            // A full-screen program: its screen, under whatever the main
            // screen printed before it took over (`git commit`'s hints before
            // the editor) — the main screen's lines are not on the alternate
            // one, so nothing is shown twice.
            Some(screen) if screen.alternate() => View::Screen {
                before: update.text,
                snapshot: screen.snapshot(),
            },
            // Absolute addressing on the main screen (`clear`, a `watch`-style
            // redraw): the screen already holds the recent lines.
            Some(screen) if addressed => View::Screen {
                before: String::new(),
                snapshot: screen.snapshot(),
            },
            _ => View::Lines {
                text: update.text,
                omitted: update.omitted_lines,
                at: state
                    .screen
                    .as_ref()
                    .map(|screen| screen.cursor_line())
                    .unwrap_or_default(),
            },
        };
        if matches!(status, Status::Exited(_) | Status::Stopped) {
            state.exit_reported = true;
        }
        report::report(session, status, &view)
    }

    /// A waiting call is done (its look taken). Returns `Some(observed)` when
    /// this call must finalize the exit — the monitor left it to the calls,
    /// and this is the last one out — `observed` saying whether a report
    /// covered it.
    pub fn end_wait(&self) -> Option<bool> {
        let mut state = self.lock();
        state.waiters = state.waiters.saturating_sub(1);
        if state.finished && !state.finalized && state.waiters == 0 {
            state.finalized = true;
            return Some(state.exit_reported);
        }
        None
    }
}

/// A waiting call's stream to its running cell ([`SessionIo::wait`]): paced
/// at [`STREAM_INTERVAL`], sent only when something changed, the settled
/// text capped at [`STREAM_MAX_BYTES`].
#[derive(Default)]
struct Streamer {
    last: Option<Instant>,
    /// The live rows sent last.
    shown: String,
    /// Settled bytes sent so far.
    settled: usize,
}

impl Streamer {
    /// Is a stream update due at `now`? Starts the next interval if so.
    fn due(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= STREAM_INTERVAL);
        if due {
            self.last = Some(now);
        }
        due
    }

    /// Hand `update` to `stream` — unless it changes nothing on screen.
    fn send(&mut self, update: super::transcript::Stream, stream: &mut dyn FnMut(&str, &str)) {
        let settled = if self.settled < STREAM_MAX_BYTES {
            self.settled += update.settled.len();
            update.settled
        } else {
            String::new()
        };
        if settled.is_empty() && update.live == self.shown {
            return;
        }
        stream(&settled, &update.live);
        self.shown = update.live;
    }
}

impl IoState {
    /// Is the program waiting for keys? The probe's word first
    /// ([`super::probe`]): a thread reading the terminal is waiting wherever
    /// the cursor sits, and a tree that is all at work is busy whatever the
    /// screen shows. Without it, the screen: a full-screen program always
    /// looks it; otherwise a prompt by the cursor ([`Screen::awaiting_keys`])
    /// or a program reading key by key, wherever its cursor is — unless the
    /// line under the cursor is **animated**
    /// ([`Transcript::cursor_line_animated`]): a progress bar or a spinner
    /// leaves the cursor exactly where a prompt would, and pauses. Never a
    /// pipe.
    fn awaiting_keys(&self) -> bool {
        self.screen.as_ref().is_some_and(|screen| match self.probe {
            Probe::Reading => true,
            Probe::Idle => false,
            Probe::Unknown => {
                screen.alternate()
                    || (!self.transcript.cursor_line_animated()
                        && (self.reading_keys || screen.awaiting_keys()))
            }
        })
    }

    /// Is the terminal reading a line with echo off — a password prompt —
    /// that the program put up since it was last typed into? Keys just typed
    /// at a prompt reach a program still in its mode: until it answers, that
    /// is the answer on its way, not a second prompt. And the probe's word
    /// comes first, as for [`Self::awaiting_keys`]: a tree seen all at work
    /// left echo off for its own reasons (swallowing type-ahead) and asks
    /// nothing. Never a pipe.
    fn password_prompt(&self) -> bool {
        self.screen.is_some()
            && self.hidden_input
            && self.seq > self.input_seq
            && self.probe != Probe::Idle
    }

    /// The transcript's unfinished tail — the last line, never newline-ended.
    fn transcript_rest(&mut self) -> String {
        self.transcript.take_rest()
    }
}

#[cfg(test)]
mod tests {
    use super::report::Waiting;
    use super::*;
    use std::sync::Arc;

    const LONG: Duration = Duration::from_secs(10);

    fn never() -> bool {
        false
    }

    /// Feed `chunks` from a stand-in monitor thread after `delay`.
    fn feed_later(io: &Arc<SessionIo>, delay: Duration, chunks: &'static [&'static [u8]]) {
        let io = Arc::clone(io);
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            for chunk in chunks {
                io.absorb(chunk);
            }
        });
    }

    #[test]
    fn a_tty_session_has_a_screen_and_a_pipe_does_not() {
        assert!(SessionIo::new(true).is_tty());
        assert!(!SessionIo::new(false).is_tty());
        assert!(SessionIo::new(false).screen_text().is_none());
    }

    #[test]
    fn a_prompt_settles_the_wait_and_the_look_reports_it() {
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        feed_later(&io, Duration::from_millis(30), &[b"Python 3\r\n", b">>> "]);
        let end = io.wait(
            WaitKind::Launch,
            since,
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert_eq!(
            io.look(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                }
            ),
            "Running (session s1, waiting for input)\nPython 3\n>>>"
        );
        assert_eq!(io.end_wait(), None, "nothing to finalize while it runs");
    }

    #[test]
    fn the_echo_that_beats_the_wait_still_counts_as_the_answer() {
        // The mark is taken before the input is written: output that lands
        // between the write and the wait still settles the call as a prompt.
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        io.absorb(b">>> 1+1\r\n2\r\n>>> ");
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_launch_waits_from_the_sessions_beginning() {
        // Output that arrived before the launching call began to wait still
        // answers it — the prompt is not mistaken for silence.
        let io = SessionIo::waited(true);
        io.absorb(b"Name? ");
        let started = Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "not the line-quiet fallback"
        );
    }

    #[test]
    fn a_waiting_call_streams_what_the_look_will_report() {
        // The running cell shows, as it happens, exactly what the look will
        // hand the model: lines out of the cursor's reach appended once, the
        // rows in reach replaced as they redraw (docs/interactive-shell.md).
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        feed_paced(
            &io,
            &[
                b"Downloading\r\n",
                b"  10%",
                b"\r  50%",
                b"\r 100%\r\ndone\r\n$ ",
            ],
        );
        let mut view = String::new();
        let mut live_len = 0;
        let mut frames = Vec::new();
        let end = io.wait(
            WaitKind::Launch,
            since,
            LONG,
            &never,
            &never,
            &mut |settled, live| {
                live_len = crate::app::apply_tool_screen(&mut view, live_len, settled, live);
                frames.push(view.clone());
            },
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert_eq!(view, "Downloading\n 100%\ndone\n$");
        assert!(
            frames.iter().any(|frame| frame == "Downloading\n  50%"),
            "it streamed as the bar moved: {frames:?}"
        );
        assert!(
            frames.iter().all(|frame| frame.matches('%').count() <= 1),
            "a redrawn frame never lingers beside the next: {frames:?}"
        );
        assert_eq!(
            io.look(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                }
            ),
            format!("Running (session s1, waiting for input)\n{view}")
        );
    }

    #[test]
    fn lines_out_of_reach_settle_into_the_stream_once() {
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        let lines: String = (1..=60).map(|n| format!("line {n}\r\n")).collect();
        io.absorb(lines.as_bytes());
        io.absorb(b"$ ");
        let mut view = String::new();
        let mut live_len = 0;
        let mut settled_all = String::new();
        let _ = io.wait(
            WaitKind::Launch,
            since,
            LONG,
            &never,
            &never,
            &mut |settled, live| {
                settled_all.push_str(settled);
                live_len = crate::app::apply_tool_screen(&mut view, live_len, settled, live);
            },
        );
        assert!(
            settled_all.starts_with("line 1\nline 2\n"),
            "{settled_all:?}"
        );
        assert!(
            !settled_all.contains("line 60"),
            "the tail is still in reach"
        );
        let report = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert_eq!(
            report,
            format!("Running (session s1, waiting for input)\n{view}")
        );
    }

    #[test]
    fn an_exit_with_nobody_waiting_is_finalized_by_the_monitor() {
        let io = SessionIo::new(true);
        io.absorb(b"bye\r\n");
        let (rest, finish) = io.finish(Some(0));
        assert_eq!(finish, Finish::Now);
        assert_eq!(rest, "", "the finished line went to the stream already");
        assert_eq!(io.exit(), Some(Some(0)));
    }

    #[test]
    fn an_exit_a_waiting_call_reports_is_observed() {
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        let monitor = {
            let io = Arc::clone(&io);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                io.absorb(b"done\r\n");
                io.finish(Some(0)).1
            })
        };
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        assert_eq!(monitor.join().unwrap(), Finish::Waiter);
        assert_eq!(io.look("s1", Status::Exited(Some(0))), "Exit code: 0\ndone");
        assert_eq!(io.end_wait(), Some(true), "the report covered the exit");
        assert_eq!(io.end_wait(), None, "finalized once");
    }

    #[test]
    fn an_exit_the_report_missed_is_finalized_unobserved() {
        // The monitor finishes while the call is between its settle and its
        // look: the call reported the session running, so the exit still owes
        // the model a notice.
        let io = SessionIo::new(true);
        let _ = io.begin_wait();
        assert_eq!(io.finish(Some(0)).1, Finish::Waiter);
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::No,
            },
        );
        assert_eq!(io.end_wait(), Some(false));
    }

    #[test]
    fn a_waiter_registered_before_it_acts_keeps_an_exit_it_caused() {
        // `kill` ends it, `input` may type `exit` — the exit can land before
        // the call reaches its wait. Registered first, the call still owns it.
        let io = SessionIo::new(true);
        let since = io.begin_wait();
        assert_eq!(io.finish(None).1, Finish::Waiter, "left to the call");
        let end = io.wait(WaitKind::Wait, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        let _ = io.look("s1", Status::Stopped);
        assert_eq!(io.end_wait(), Some(true), "observed");
        // A launch is a waiter from birth.
        let launched = SessionIo::waited(true);
        assert_eq!(launched.finish(Some(0)).1, Finish::Waiter);
    }

    #[test]
    fn cancel_and_handoff_end_a_wait() {
        let io = SessionIo::new(true);
        let since = io.begin_wait();
        assert_eq!(
            io.wait(
                WaitKind::Wait,
                since,
                LONG,
                &|| true,
                &never,
                &mut |_, _| {}
            ),
            WaitEnd::Cancelled
        );
        let _ = io.end_wait();
        let since = io.begin_wait();
        assert_eq!(
            io.wait(
                WaitKind::Wait,
                since,
                LONG,
                &never,
                &|| true,
                &mut |_, _| {}
            ),
            WaitEnd::Handoff
        );
        let _ = io.end_wait();
    }

    #[test]
    fn a_wait_times_out() {
        let io = SessionIo::new(true);
        let since = io.begin_wait();
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(60),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
    }

    #[test]
    fn a_full_screen_program_is_looked_at_as_a_screen() {
        let io = SessionIo::new(true);
        io.absorb(b"$ vim\r\n\x1b[?1049h\x1b[H\x1b[2J~\r\n~");
        let look = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert_eq!(
            look,
            "Running (session s1, waiting for input)\n$ vim\n\
             Screen (40x120, cursor at line 2, column 2):\n~\n~",
            "the line typed before the program took over, then its screen"
        );
        // Back on the main screen, the next look is lines again.
        io.absorb(b"\x1b[?1049l$ ");
        let look = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert_eq!(look, "Running (session s1, waiting for input)\n$");
    }

    #[test]
    fn a_pipe_session_has_no_screen_and_never_waits_for_keys() {
        let io = Arc::new(SessionIo::new(false));
        let since = io.begin_wait();
        feed_later(&io, Duration::from_millis(10), &[b"prompt? "]);
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(900),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout), "stdin is /dev/null");
        assert_eq!(
            io.look(
                "s1",
                Status::Running {
                    waiting: Waiting::No
                }
            ),
            "Running (session s1)\nprompt?"
        );
    }

    #[test]
    fn a_session_still_at_its_prompt_is_waiting_even_when_nothing_new_came() {
        // A poll that saw no new output timed out rather than settling at a
        // prompt — but the program is still sitting at one, and the report
        // must keep saying so (a model told only "Running" polls on).
        let io = SessionIo::new(true);
        io.absorb(b"Full name: ");
        assert!(
            !io.waiting(WaitKind::Input, io.origin_mark()),
            "not while the prompt is still arriving"
        );
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(io.waiting(WaitKind::Input, io.origin_mark()));
        assert!(
            !io.waiting(WaitKind::Wait, io.origin_mark()),
            "a wait that saw the prompt arrive gives it longer"
        );
        let since = io.begin_wait();
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(60),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        assert!(
            io.waiting(WaitKind::Wait, since),
            "a wait that saw nothing new: it sat at its prompt throughout"
        );
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert!(
            io.waiting(WaitKind::Wait, since),
            "a look does not change where it stands"
        );
        assert_eq!(
            io.look(
                "s1",
                Status::Running {
                    waiting: report::Waiting::of(
                        io.waiting(WaitKind::Wait, since),
                        io.password_prompt()
                    )
                }
            ),
            "Running (session s1, waiting for input)\n\
             (no new output — still at: Full name:)"
        );
    }

    #[test]
    fn a_program_reading_key_by_key_waits_wherever_its_cursor_is() {
        // A menu drawn on the main screen leaves the cursor at the start of
        // a fresh line — no prompt by the cursor rule — but it has put the
        // terminal in raw mode to read single keys: it is waiting on them.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"? Pick a fruit\r\n> Apple\r\n  Banana\r\n");
        io.set_reading_keys(true);
        let started = Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < settle::LINE_QUIET,
            "settled as a prompt, not by the line-quiet fallback"
        );
        assert!(io.waiting(WaitKind::Input, io.origin_mark()));
        // Back to reading lines: the cursor rule decides again.
        io.set_reading_keys(false);
        assert!(!io.waiting(WaitKind::Input, io.origin_mark()));
    }

    /// Feed `chunks` from a stand-in monitor thread, a burst apart.
    fn feed_paced(io: &Arc<SessionIo>, chunks: &'static [&'static [u8]]) {
        let io = Arc::clone(io);
        std::thread::spawn(move || {
            for chunk in chunks {
                std::thread::sleep(BURST_SPAN + Duration::from_millis(50));
                io.absorb(chunk);
            }
        });
    }

    #[test]
    fn a_line_redrawn_in_place_is_no_prompt() {
        // A progress bar redrawn after a `\r` leaves the cursor after it, as
        // a prompt does — but a line changed in place by burst after burst
        // is an animation, however long it pauses.
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        feed_paced(
            &io,
            &[b" 10% [#---] ", b"\r 20% [##--] ", b"\r 30% [###-] "],
        );
        let end = io.wait(
            WaitKind::Input,
            since,
            Duration::from_millis(1500),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        assert!(!io.waiting(WaitKind::Input, since));
    }

    #[test]
    fn a_question_after_a_line_redrawn_in_place_is_a_prompt() {
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        feed_paced(
            &io,
            &[b" 10%", b"\r 20%", b"\r 30%", b"\r\nContinue? [Y/n] "],
        );
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(started.elapsed() < settle::LINE_QUIET);
    }

    #[test]
    fn a_redraw_answering_the_keys_just_typed_is_still_a_prompt() {
        // A menu moves its highlight once per key; what it drew before the
        // model typed does not make that one redraw an animation.
        let io = SessionIo::new(true);
        for frame in [
            &b"? Pick: Apple"[..],
            b"\r? Pick: Banana",
            b"\r? Pick: Cherry",
        ] {
            io.absorb(frame);
            std::thread::sleep(BURST_SPAN + Duration::from_millis(50));
        }
        std::thread::sleep(settle::PROMPT_QUIET);
        assert!(
            !io.waiting(WaitKind::Input, io.origin_mark()),
            "redrawn by nobody's keys"
        );
        let since = io.begin_wait();
        io.note_input(true);
        io.absorb(b"\r? Pick: Durian");
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(started.elapsed() < settle::LINE_QUIET);
    }

    #[test]
    fn typed_text_held_at_the_prompt_is_seen_at_the_cursor() {
        // A line editor echoes what it was typed at its prompt and holds it
        // until Enter; the cursor sits right after it.
        let io = SessionIo::new(true);
        io.absorb(b">>> print(1)");
        assert!(io.holds_typed("print(1)"));
        assert!(!io.holds_typed("print(2)"));
        // A full-screen program inserts what is typed — an editor's buffer
        // line is not a line waiting for Enter.
        let vim = SessionIo::new(true);
        vim.absorb(b"\x1b[?1049h\x1b[Hhello world");
        assert!(!vim.holds_typed("world"));
        assert!(
            !SessionIo::new(false).holds_typed("x"),
            "no screen, no prompt"
        );
    }

    #[test]
    fn a_program_seen_reading_its_terminal_waits_with_no_prompt_at_all() {
        // `read x`, `cat`: nothing on screen asks, but the probe saw the read
        // (`pty::probe`) — the launch settles as waiting, briskly.
        let io = Arc::new(SessionIo::waited(true));
        io.set_probe(Probe::Reading);
        let started = Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < settle::LINE_QUIET,
            "not the quiet fallback"
        );
        assert!(io.waiting(WaitKind::Launch, io.origin_mark()));
    }

    #[test]
    fn a_program_seen_at_work_is_busy_whatever_its_screen_shows() {
        // `Working... ` left open over a sleeping command has a prompt's
        // shape, but every process is at work: no prompt (`pty::probe`).
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        io.absorb(b"Working... ");
        io.set_probe(Probe::Idle);
        let end = io.wait(
            WaitKind::Input,
            since,
            Duration::from_millis(1200),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        assert!(!io.waiting(WaitKind::Input, since));
        // New output makes the verdict stale: the screen decides again.
        io.absorb(b"\r\nContinue? ");
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(io.waiting(WaitKind::Input, since));
    }

    #[test]
    fn input_makes_the_verdict_stale_too() {
        let io = SessionIo::new(true);
        io.set_probe(Probe::Reading);
        io.note_input(true);
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(
            !io.waiting(WaitKind::Input, io.origin_mark()),
            "the read the probe saw has taken the input"
        );
    }

    #[test]
    fn a_password_prompt_ends_a_wait_that_began_after_it() {
        // sudo's retry: `Sorry, try again.` and a fresh prompt, printed
        // between two calls, the terminal reading a hidden line. A pure wait
        // begun after all of it saw no output of its own — it still ends,
        // as a password prompt (docs/interactive-shell.md).
        let io = Arc::new(SessionIo::new(true));
        io.note_input(true);
        io.absorb(b"\r\nSorry, try again.\r\n[sudo] password for u: ");
        io.set_hidden_input(true);
        let since = io.begin_wait();
        let started = Instant::now();
        let end = io.wait(WaitKind::Wait, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(io.waiting(WaitKind::Wait, since));
        assert!(io.password_prompt());
    }

    #[test]
    fn a_hidden_line_read_is_a_password_prompt_wherever_the_cursor_is() {
        // Asked on a line of its own: no prompt by the cursor, but the
        // terminal's own mode says what it waits for.
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"Enter the passphrase on the next line\r\n");
        io.set_hidden_input(true);
        let started = Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < settle::LINE_QUIET,
            "not the quiet fallback"
        );
        assert!(io.password_prompt());
        io.set_hidden_input(false);
        assert!(!io.password_prompt(), "echo back on: the read is over");
    }

    #[test]
    fn keys_the_program_has_not_answered_are_no_second_password_prompt() {
        // Just typed: until the program answers, a terminal still in the
        // prompt's mode is the answer on its way, not a fresh prompt.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"Password: ");
        io.set_hidden_input(true);
        let since = io.begin_wait();
        io.note_input(true);
        assert!(!io.password_prompt());
        let end = io.wait(
            WaitKind::Input,
            since,
            Duration::from_millis(1000),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
    }

    #[test]
    fn text_typed_without_its_enter_leaves_the_password_prompt_standing() {
        // The terminal holds a line until Enter: nothing typed before it has
        // reached the program, which is still asking — and says so at once.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"[sudo] password for u: ");
        io.set_hidden_input(true);
        let since = io.begin_wait();
        io.note_input(false);
        assert!(io.password_prompt());
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < settle::LINE_QUIET,
            "not the quiet fallback"
        );
        io.note_input(true);
        assert!(!io.password_prompt(), "the Enter: the answer is on its way");
    }

    #[test]
    fn a_hidden_line_nobody_reads_is_no_password_prompt() {
        // A script that turns echo off to swallow type-ahead while it works
        // leaves the terminal in a password prompt's mode — but the kernel
        // sees every process at work: busy, not asking (`pty::probe`).
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        io.absorb(b"Installing... ");
        io.set_hidden_input(true);
        io.set_probe(Probe::Idle);
        assert!(!io.password_prompt());
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(1000),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        // Blind again — output made the verdict stale: the mode decides.
        io.absorb(b"\r\nPassword: ");
        assert!(io.password_prompt());
    }

    #[test]
    fn the_monitor_probes_a_quiet_terminal_a_call_waits_on() {
        let io = SessionIo::new(true);
        std::thread::sleep(PROBE_QUIET + Duration::from_millis(20));
        assert!(!io.wants_probe(), "nobody is waiting");
        let _since = io.begin_wait();
        assert!(io.wants_probe());
        io.absorb(b"x");
        assert!(!io.wants_probe(), "output just arrived");
        std::thread::sleep(PROBE_QUIET + Duration::from_millis(20));
        assert!(io.wants_probe());
        let pipe = SessionIo::waited(false);
        std::thread::sleep(PROBE_QUIET + Duration::from_millis(20));
        assert!(!pipe.wants_probe(), "a pipe has no terminal to read");
    }

    #[test]
    fn a_busy_or_finished_or_pipe_session_is_not_waiting() {
        let busy = SessionIo::new(true);
        busy.absorb(b"compiling\r\n");
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(
            !busy.waiting(WaitKind::Input, busy.origin_mark()),
            "a fresh line is no prompt"
        );
        let pipe = SessionIo::new(false);
        pipe.absorb(b"prompt? ");
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(
            !pipe.waiting(WaitKind::Input, pipe.origin_mark()),
            "nothing can type into a pipe"
        );
        let gone = SessionIo::new(true);
        gone.absorb(b"bye? ");
        let _ = gone.finish(Some(0));
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(!gone.waiting(WaitKind::Input, gone.origin_mark()));
    }

    #[test]
    fn a_terminal_query_is_answered_through_absorb() {
        let io = SessionIo::new(true);
        let (replies, _) = io.absorb(b"ab\x1b[6n");
        assert_eq!(replies, b"\x1b[1;3R");
        let (replies, _) = SessionIo::new(false).absorb(b"ab\x1b[6n");
        assert!(replies.is_empty(), "a pipe has no terminal to answer for");
    }

    #[test]
    fn announcing_happens_once() {
        let io = SessionIo::new(true);
        assert!(!io.announced());
        assert!(io.announce());
        assert!(!io.announce(), "the second announce sends nothing");
        assert!(io.announced());
    }
}

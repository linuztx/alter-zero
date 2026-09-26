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

use super::fold::{Fold, HeadTail};
use super::probe::Probe;
use super::report::{self, Status, View};
use super::screen::Screen;
use super::settle::{self, Observation, Settle, WaitKind};
use super::spawn::LineMode;
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

/// The most line feeds fed to the transcript before the lines it finished
/// are taken for the stream ([`SessionIo::absorb`]) — well inside its
/// retention cap, which a line feed applies.
const STREAM_SLICE_LINES: usize = super::transcript::MAX_RETAINED_LINES / 2;

/// `bytes` cut after every `lines`-th line feed (a newline, a vertical tab
/// or a form feed — each moves a terminal's cursor down a row), the last
/// piece holding whatever follows. The transcript's parser carries a
/// sequence split between two pieces over.
fn line_slices(bytes: &[u8], lines: usize) -> impl Iterator<Item = &[u8]> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let cut = rest
            .iter()
            .enumerate()
            .filter(|(_, byte)| matches!(byte, b'\n' | 0x0b | 0x0c))
            .nth(lines.max(1) - 1)
            .map_or(rest.len(), |(at, _)| at + 1);
        let (piece, tail) = rest.split_at(cut);
        rest = tail;
        Some(piece)
    })
}

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
    /// The terminal reads whole lines (canonical mode) — `None` until the
    /// monitor has read its mode ([`SessionIo::set_line_mode`]). No single
    /// key reaches a program in line mode, so a screen it draws is a display
    /// rather than one it takes keys on ([`IoState::drawing_screen`]).
    canonical: Option<bool>,
    /// When the current burst of output began ([`BURST_SPAN`]) — `None`
    /// before any, and after input, whose answer starts a burst of its own.
    burst_began: Option<Instant>,
    /// What the monitor's probe last saw the program blocked in
    /// ([`super::probe`]) — [`Probe::Unknown`] again the moment output or
    /// input moves the session on.
    probe: Probe,
    /// When input was last written — quiet counts from it too, for the probe.
    last_input: Option<Instant>,
    /// When the keys last sent will all have been typed — they go a moment
    /// apart (`pty::keys`); a waiting call looks only after, and counts the
    /// program's quiet from then. The most they could take until the writer
    /// says they are out ([`SessionIo::typed`]).
    typing_until: Option<Instant>,
    /// Calls whose keys the writer has not yet said are out.
    typing_calls: usize,
    /// The last look showed the screen, not lines — what a program drawing
    /// on the main screen keeps showing while it edits in place
    /// ([`IoState::screen_view_now`]).
    screen_view: bool,
    /// The program switched to the alternate screen and has drawn nothing on
    /// it yet ([`Screen::undrawn`]).
    awaiting_frame: bool,
    /// When the alternate screen last switched to was first drawn on: a
    /// screen that never stops drawing is watched from its first frame as
    /// from a key ([`IoState::screen_answer_began`]).
    drawn_at: Option<Instant>,
    /// The terminal reads a whole line with echo off — a password prompt
    /// (`pty::spawn::LineMode::hides_input`, read by the monitor).
    hidden_input: bool,
    /// [`IoState::seq`] when input was last written: output past it is the
    /// program's answer ([`IoState::password_prompt`]).
    input_seq: u64,
    /// [`IoState::seq`] at the model's last look ([`SessionIo::look`]):
    /// output past it is news to the model ([`IoState::printed_since`]).
    looked_seq: u64,
    /// The last input submitted a line at a password prompt: until the
    /// program prints visible text ([`Transcript::answered`]) it is checking
    /// the password, and its silence ends no call.
    awaiting_answer: bool,
    /// The output folded a second time as **data** — tabs and trailing
    /// spaces kept, both ends kept past the cap (`docs/bash-tools.md`) —
    /// for the report of a command that exits inside its own launch. A TTY
    /// session's alone: a pipe's transcript already is its data.
    data: Option<DataView>,
    /// The file the session's whole output is teed to, which a report that
    /// cut it names ([`HeadTail::render`]).
    log: Option<std::path::PathBuf>,
    /// The model has looked at the session — the data view is for a
    /// launch's one report of everything, never a report of what is new.
    looked: bool,
    /// The program switched to the alternate screen at some point: what it
    /// drew there is a screen, never lines of data.
    alternate_used: bool,
    /// The command exited leaving a process of its own running (`server &
    /// …`), which stopped with it — what the report tells the model how to
    /// avoid (`llm::tools::REAPED_NOTE`).
    stranded: bool,
    /// [`IoState::seq`] when the model was last told this session sits at a
    /// prompt nobody is waiting on ([`SessionIo::take_unseen_prompt`]); past
    /// [`IoState::looked_seq`], the model has not looked since, and is not
    /// told again.
    prompt_told_seq: u64,
}

/// The output as data: the fold a plain command's pipe always had, and what
/// of it is kept for the model.
#[derive(Default)]
struct DataView {
    fold: Fold,
    kept: HeadTail,
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
                transcript: if tty {
                    Transcript::new()
                } else {
                    Transcript::for_pipe()
                },
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
                canonical: None,
                burst_began: None,
                probe: Probe::Unknown,
                last_input: None,
                typing_until: None,
                typing_calls: 0,
                screen_view: false,
                awaiting_frame: false,
                drawn_at: None,
                hidden_input: false,
                input_seq: 0,
                looked_seq: 0,
                awaiting_answer: false,
                data: tty.then(DataView::default),
                log: None,
                looked: false,
                alternate_used: false,
                stranded: false,
                prompt_told_seq: 0,
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
        let was_alternate = state.screen.as_ref().is_some_and(Screen::alternate);
        let replies = state
            .screen
            .as_mut()
            .map(|screen| screen.feed(bytes))
            .unwrap_or_default();
        let now = Instant::now();
        // A screen switched to: its first frame is watched for.
        if let Some(alternate) = state.screen.as_ref().map(Screen::alternate) {
            state.alternate_used |= alternate || was_alternate;
            if alternate && !was_alternate {
                state.awaiting_frame = true;
            }
            if state.awaiting_frame
                && (!alternate || !state.screen.as_ref().is_some_and(Screen::undrawn))
            {
                state.awaiting_frame = false;
                if alternate {
                    state.drawn_at = Some(now);
                }
            }
        }
        if state
            .burst_began
            .is_none_or(|began| now.saturating_duration_since(began) >= BURST_SPAN)
        {
            state.transcript.new_burst();
            state.burst_began = Some(now);
        }
        // A slice at a time: a line feed trims the transcript to its
        // retention cap, so a chunk with more lines than that would drop
        // lines the stream — the session's log — had not taken yet.
        let mut committed = String::new();
        for slice in line_slices(bytes, STREAM_SLICE_LINES) {
            state.transcript.feed(slice);
            committed.push_str(&state.transcript.take_committed());
        }
        if let Some(data) = state.data.as_mut() {
            data.fold.feed(bytes);
            let lines = data.fold.take_settled();
            data.kept.push(&lines);
        }
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
        if let Some(data) = state.data.as_mut() {
            let rest = std::mem::take(&mut data.fold).finish();
            data.kept.push(&rest);
        }
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

    /// The file the session's whole output is teed to — what a report that
    /// had to cut the output names, so the model can read the rest.
    pub fn set_log(&self, path: std::path::PathBuf) {
        self.lock().log = Some(path);
    }

    /// The monitor: the command exited leaving a process of its own running,
    /// which stops with it (see [`stranded`](Self::stranded)).
    pub fn set_stranded(&self) {
        self.lock().stranded = true;
    }

    /// Did the command exit leaving a process of its own running — a
    /// `server &` it started — which stopped with it?
    #[must_use]
    pub fn stranded(&self) -> bool {
        self.lock().stranded
    }

    /// The monitor: has this session — announced, running, nobody waiting on
    /// it — stopped to ask for input since the model last looked, quiet for
    /// `min_quiet`? `true` once: a dev server asking `Use another port?
    /// (Y/n)` in the background would otherwise wait unseen until someone
    /// looked, and a session told of is not told again until the model has
    /// looked at it, so a program that keeps asking cannot start turn after
    /// turn nobody reads (`docs/bash-tools.md`). Only a line shaped like a
    /// prompt counts (`IoState::unseen_prompt_shape`); the probe, when it
    /// can see, rules out a program at work.
    pub fn take_unseen_prompt(&self, min_quiet: std::time::Duration) -> bool {
        let mut state = self.lock();
        let quiet = state.last_output.unwrap_or(state.created).elapsed();
        if !state.unseen_prompt_shape()
            || quiet < min_quiet
            || !(state.password_prompt() || state.awaiting_keys())
        {
            return false;
        }
        state.prompt_told_seq = state.seq;
        true
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
    /// ([`settle::prompt_quiet`]), or a full-screen program that has drawn on
    /// for [`settle::SCREEN_BUSY`] since the call's keys — the test a wait
    /// settles on? What a report
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
            settle::prompt_quiet(kind, state.printed_since(kind, since))
        };
        // A full-screen program that never stops drawing takes keys all the
        // while (`settle::SCREEN_BUSY`).
        let drawing = state.drawing_screen()
            && state.printed_since(kind, since)
            && state.screen_answer_began(since).elapsed() >= settle::SCREEN_BUSY;
        !state.finished
            && ((quiet >= needed && (exact || state.awaiting_keys()))
                || (drawing && state.awaiting_keys()))
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
            && !state.finished
            && last.elapsed() >= PROBE_QUIET
            && (state.waiters > 0 || state.unseen_prompt_shape())
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
    /// animation ([`Transcript::new_input`]). `typed` is what was written:
    /// whether it reaches a program reading whole lines
    /// ([`super::keys::reaches_line_reader`]) decides if a password prompt
    /// stands.
    pub fn note_input(&self, typed: &[u8]) {
        let mut state = self.lock();
        // A password handed over: the program checks it before it says a
        // word (sudo takes seconds), so the call waits for its answer.
        state.awaiting_answer = super::keys::submits_line(typed) && state.password_prompt();
        state.transcript.new_input();
        state.burst_began = None;
        // The read the probe saw is about to take these keys.
        state.probe = Probe::Unknown;
        state.last_input = Some(Instant::now());
        // Keys that reach a line reader answer its prompt; text still on
        // the line being edited has not, so a password prompt stands.
        if super::keys::reaches_line_reader(typed) {
            state.input_seq = state.seq;
        }
    }

    /// The keys just sent may take up to `bound` to type — every wait the
    /// writer may make between them ([`super::keys::typing_bound`]) — and
    /// are typing until the writer says they are out ([`typed`](Self::typed)):
    /// the last of them is the input the program's quiet counts from, for
    /// the probe too.
    pub fn typing_for(&self, bound: Duration) {
        let mut state = self.lock();
        let until = Instant::now() + bound;
        state.typing_until = Some(state.typing_until.map_or(until, |at| at.max(until)));
        state.last_input = state.typing_until;
        state.typing_calls += 1;
    }

    /// One call's keys are all out (the session's writer): once every
    /// call's are, typing ended now.
    pub fn typed(&self) {
        let mut state = self.lock();
        state.typing_calls = state.typing_calls.saturating_sub(1);
        if state.typing_calls == 0 {
            let now = Instant::now();
            state.typing_until = Some(now);
            state.last_input = Some(now);
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Are keys still being typed?
    #[cfg(test)]
    fn typing(&self) -> bool {
        self.lock()
            .typing_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Record whether the terminal reads a line with echo off — a password
    /// prompt (`pty::spawn::LineMode::hides_input`) — alone: the tests' seam
    /// for one part of what [`set_line_mode`](Self::set_line_mode) records.
    #[cfg(test)]
    fn set_hidden_input(&self, hidden: bool) {
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
    /// mode, not canonical) alone: the tests' seam for one part of what
    /// [`set_line_mode`](Self::set_line_mode) records.
    #[cfg(test)]
    fn set_reading_keys(&self, reading_keys: bool) {
        let changed = {
            let mut state = self.lock();
            std::mem::replace(&mut state.reading_keys, reading_keys) != reading_keys
        };
        if changed {
            self.changed.notify_all();
        }
    }

    /// Record how the program reads its terminal — the monitor reads the
    /// mode off it as output arrives and on every idle poll
    /// (`pty::spawn::line_mode`): key by key ([`LineMode::reads_keys`]), a
    /// line with echo off ([`LineMode::hides_input`]), or whole lines at all
    /// (canonical mode, which no single key gets through). All at once, so a
    /// waiting call never judges the screen by half a mode.
    pub fn set_line_mode(&self, mode: LineMode) {
        let changed = {
            let mut state = self.lock();
            let before = (state.reading_keys, state.hidden_input, state.canonical);
            state.reading_keys = mode.reads_keys();
            state.hidden_input = mode.hides_input();
            state.canonical = Some(mode.canonical);
            before != (state.reading_keys, state.hidden_input, state.canonical)
        };
        if changed {
            self.changed.notify_all();
        }
    }

    /// Does the line under the cursor end with `typed`, the cursor right
    /// after it — text the call typed and a line editor echoed, still
    /// waiting for its Enter ([`Screen::holds_at_cursor`])? Never on the
    /// alternate screen, where a full-screen program inserts what it is
    /// typed rather than holding a line.
    #[must_use]
    pub fn holds_typed(&self, typed: &str) -> bool {
        self.lock()
            .screen
            .as_ref()
            .is_some_and(|screen| !screen.alternate() && screen.holds_at_cursor(typed))
    }

    /// The modes the program set that change what its keys look like —
    /// cursor-key mode, bracketed paste ([`super::keys::encode`]). A pipe
    /// has none.
    #[must_use]
    pub fn modes(&self) -> super::keys::Modes {
        self.lock()
            .screen
            .as_ref()
            .map(Screen::modes)
            .unwrap_or_default()
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
                let output = state.printed_since(kind, since);
                let last = if output {
                    state.last_output.unwrap_or(since.at)
                } else {
                    since.at
                };
                // The call's own keys: typed a moment apart, the last is the
                // one the screen must have answered.
                let typing_until = state.typing_until.filter(|&until| until > since.at);
                let last = typing_until.map_or(last, |until| last.max(until));
                let seen = Observation {
                    elapsed: now.saturating_duration_since(since.at),
                    quiet: now.saturating_duration_since(last),
                    output,
                    awaiting_keys: state.awaiting_keys(),
                    reading: state.probe == Probe::Reading,
                    secret: state.password_prompt(),
                    answer_pending: state.awaiting_answer && !state.transcript.answered(),
                    typing: typing_until.is_some_and(|until| now < until),
                    full_screen: state.drawing_screen(),
                    undrawn: state.screen.as_ref().is_some_and(Screen::undrawn),
                    since_keys: now.saturating_duration_since(state.screen_answer_began(since)),
                    exited: state.finished,
                    busy: state.probe == Probe::Idle,
                    key_by_key: state.reading_keys,
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
        let screen_view = state.screen_view_now();
        state.screen_view = screen_view;
        // A command that exited inside its launch, having only written lines:
        // its output as data, both ends kept (docs/bash-tools.md). Read
        // before the transcript's look resets what it noticed.
        let data = (matches!(status, Status::Exited(_))
            && !state.looked
            && !state.alternate_used
            && !state.transcript.screen_addressed()
            && !state.transcript.edited_in_place())
        .then(|| {
            state
                .data
                .as_ref()
                .map(|data| data.kept.render(state.log.as_deref()))
        })
        .flatten();
        let update = state.transcript.take_update();
        state.looked_seq = state.seq;
        state.looked = true;
        let view = match &state.screen {
            _ if data.is_some() => View::Data {
                text: data.unwrap_or_default(),
            },
            // A full-screen program: its screen, under whatever the main
            // screen printed before it took over (`git commit`'s hints before
            // the editor) — the main screen's lines are not on the alternate
            // one, so nothing is shown twice.
            Some(screen) if screen.alternate() => View::Screen {
                before: update.text,
                snapshot: screen.snapshot(),
            },
            // A program drawing on the main screen (`clear`, a `watch`-style
            // redraw, dialog — see `IoState::screen_view_now`): the screen
            // already holds the recent lines.
            Some(screen) if screen_view => View::Screen {
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
    /// screen shows. Without it, the screen: a program drawing a screen it
    /// takes keys on ([`Self::drawing_screen`] — not a display left reading
    /// whole lines) always looks it; otherwise a prompt by the cursor
    /// ([`Screen::cursor_mid_line`]) or a program reading key by key,
    /// wherever its cursor is — unless the line under the cursor is
    /// **animated** ([`Transcript::cursor_line_animated`]): a progress bar or
    /// a spinner leaves the cursor exactly where a prompt would, and pauses.
    /// Never a pipe.
    fn awaiting_keys(&self) -> bool {
        match self.probe {
            Probe::Reading => self.screen.is_some(),
            Probe::Idle | Probe::Elsewhere => false,
            Probe::Polling | Probe::Unknown => self.screen_awaits_keys(),
        }
    }

    /// Does the **screen** say the program waits for keys — the shape
    /// [`Self::awaiting_keys`] falls back on where the probe leaves the
    /// answer open?
    fn screen_awaits_keys(&self) -> bool {
        // A screen switched to and not drawn on has nothing to answer.
        self.screen.as_ref().is_some_and(|screen| {
            !screen.undrawn()
                && (self.drawing_screen()
                    || (!self.transcript.cursor_line_animated()
                        && (self.reading_keys || screen.cursor_mid_line())))
        })
    }

    /// Might this session be asking something nobody has seen — announced,
    /// running, no call waiting on it, output since the model last looked
    /// and no notice since then either — by the shape of its screen alone:
    /// a prompt by the cursor, a program reading key by key, or a line read
    /// with echo off? The monitor probes such a session
    /// ([`SessionIo::wants_probe`]) so that [`SessionIo::take_unseen_prompt`]
    /// can tell a question from a program at work; a line of log output,
    /// which ends at the start of a line, is never one.
    fn unseen_prompt_shape(&self) -> bool {
        self.announced
            && self.waiters == 0
            && !self.finished
            && self.seq > self.looked_seq
            && self.prompt_told_seq <= self.looked_seq
            && ((self.screen.is_some() && self.hidden_input && self.seq > self.input_seq)
                || self.screen_awaits_keys())
    }

    /// Does the look show the **screen** of a program drawing on the main
    /// one? Absolute addressing since the last look turns it on; output that
    /// only edits in place — a backspace, a relative or column move, a
    /// character inserted or deleted (`Transcript::edited_in_place`) — keeps
    /// it, since dialog, once drawn, moves its focus with those alone; and
    /// output of plain lines turns it off, so `clear` then an ordinary
    /// command reads as lines again.
    fn screen_view_now(&self) -> bool {
        if self.transcript.screen_addressed() {
            true
        } else if self.seq > self.looked_seq && !self.transcript.edited_in_place() {
            false
        } else {
            self.screen_view
        }
    }

    /// Where the screen's answer to a call that began at `since` begins:
    /// its last key typed, or — later — the first frame of a screen switched
    /// to since ([`IoState::drawn_at`]): a program that took seconds to draw
    /// anything is watched from its first frame, not reported half-drawn.
    fn screen_answer_began(&self, since: Mark) -> Instant {
        let last_key = self
            .typing_until
            .filter(|&until| until > since.at)
            .unwrap_or(since.at);
        self.drawn_at
            .filter(|&drawn| drawn > last_key)
            .unwrap_or(last_key)
    }

    /// Is a program **drawing a screen** it takes keys on — a full-screen
    /// one on the alternate screen, or one that repaints the main screen by
    /// cursor addressing while the terminal reads key by key (`top`)? Such a
    /// program may never go quiet ([`settle::SCREEN_BUSY`]); a command that
    /// addresses the screen for a progress bar (apt's scroll region) reads
    /// whole lines, and is not one — nor is a **display** on the alternate
    /// screen that leaves the terminal reading whole lines (`gh run watch`,
    /// a `tput smcup` loop), since every program that takes keys there turns
    /// line mode off first. A relay holds the terminal raw, so what it
    /// carries still counts; a mode not read yet counts too.
    fn drawing_screen(&self) -> bool {
        self.screen.as_ref().is_some_and(|screen| {
            !screen.undrawn()
                && ((screen.alternate() && !self.reads_lines())
                    || (self.transcript.screen_addressed() && self.reading_keys))
        })
    }

    /// Does the terminal read whole lines — canonical mode, as the monitor
    /// last read it? `false` while the mode is unknown.
    fn reads_lines(&self) -> bool {
        self.canonical == Some(true)
    }

    /// Is the terminal reading a line with echo off — a password prompt —
    /// that the program put up since it was last typed into? Keys just typed
    /// at a prompt reach a program still in its mode: until it answers, that
    /// is the answer on its way, not a second prompt. And the probe's word
    /// comes first, as for [`Self::awaiting_keys`]: a tree seen all at work,
    /// or waiting on nothing that is the terminal, left echo off for its own
    /// reasons (swallowing type-ahead) and asks nothing. Never a pipe.
    fn password_prompt(&self) -> bool {
        self.screen.is_some()
            && self.hidden_input
            && self.seq > self.input_seq
            && !matches!(self.probe, Probe::Idle | Probe::Elsewhere)
    }

    /// Has the program printed anything new to a call of `kind` that began
    /// at `since`? For a launch or an input, anything since the call began —
    /// the answer to what it did. For a pure wait, anything since the model
    /// last looked: a question asked while the model was deciding to wait is
    /// as new to it as one asked during the wait.
    fn printed_since(&self, kind: WaitKind, since: Mark) -> bool {
        let from = if kind == WaitKind::Wait {
            since.seq.min(self.looked_seq)
        } else {
            since.seq
        };
        self.seq > from
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
    fn a_screen_switched_to_is_waited_on_until_something_is_drawn_on_it() {
        // btop 1.4 switches to the alternate screen, then probes its GPU
        // before it draws a thing: the launch shows the first frame, not
        // the blank a program never seen drawing reads as after two seconds.
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"\x1b[?1049h\x1b[?25l");
        let since = io.origin_mark();
        assert!(
            !io.waiting(WaitKind::Launch, since),
            "nothing to answer yet"
        );
        feed_later(&io, Duration::from_millis(2_500), &[b"\x1b[1;1fCPU  5%"]);
        let began = Instant::now();
        let end = io.wait(
            WaitKind::Launch,
            since,
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(began.elapsed() >= Duration::from_millis(2_500));
        assert!(
            io.look(
                "s",
                Status::Running {
                    waiting: Waiting::Input
                }
            )
            .contains("CPU  5%")
        );
    }

    #[test]
    fn a_first_frame_drawn_late_is_reported_whole() {
        // A frame written in pieces a moment apart, seconds after the switch:
        // two seconds had passed since the call began — the most a screen
        // that never stops drawing is watched — so the first piece settled
        // the call, and the look showed a quarter of the frame. The first
        // frame is watched as long as a first key's answer is.
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"\x1b[?1049h\x1b[?25l");
        let since = io.origin_mark();
        feed_later(&io, Duration::from_millis(2_200), &[b"\x1b[1;1Htop half"]);
        feed_later(
            &io,
            Duration::from_millis(2_260),
            &[b"\x1b[20;1Hbottom half"],
        );
        let end = io.wait(
            WaitKind::Launch,
            since,
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        let look = io.look(
            "s",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert!(look.contains("bottom half"), "{look}");
    }

    #[test]
    fn a_screen_drawn_on_the_main_screen_stays_a_screen_while_edited_in_place() {
        // dialog draws its menu on the main screen with absolute addressing,
        // then moves its focus with column moves and colours alone: the
        // next look said "no new output", and the model never learned its
        // Tab had moved the focus to Cancel.
        let io = SessionIo::new(true);
        io.absorb(b"\x1b[H\x1b[2J\x1b[3;5H< \x1b[7mOK\x1b[m >  < Cancel >");
        let running = Status::Running {
            waiting: Waiting::Input,
        };
        assert!(io.look("s", running).contains("Screen ("));
        io.absorb(b"\r\x1b[6C\x1b[mOK\x1b[6C\x1b[7mCancel\x1b[m");
        let look = io.look("s", running);
        assert!(look.contains("Screen ("), "{look}");
        assert!(look.contains("\"Cancel\""), "the focus, named: {look}");
        // Plain lines after it — the program gone, a shell printing — are
        // lines again.
        io.absorb(b"\r\n$ ls\r\nfile.txt\r\n$ ");
        let look = io.look("s", running);
        assert!(!look.contains("Screen ("), "{look}");
    }

    #[test]
    fn keys_typed_sooner_than_their_bound_end_the_typing_then() {
        // The writer paces keys by the program's reads — far sooner than the
        // most they could take: its word that they are out is where the
        // call counts the program's quiet from.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"\x1b[?1049h> apple\r\n  banana");
        let since = io.begin_wait();
        io.note_input(b"\x1b[B");
        io.typing_for(Duration::from_secs(30));
        io.absorb(b"\x1b[H  apple\r\n> banana");
        io.typed();
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        let took = started.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn typing_is_over_only_once_every_calls_keys_are_out() {
        // A call that ended mid-typing, and the next call's keys behind it:
        // the first batch done is not the second.
        let io = SessionIo::new(true);
        io.typing_for(Duration::from_secs(30));
        io.typing_for(Duration::from_secs(30));
        io.typed();
        assert!(io.typing(), "the second call's keys are still going");
        io.typed();
        assert!(!io.typing());
    }

    #[test]
    fn a_call_waits_for_its_keys_to_be_typed() {
        // Keys go a moment apart (`pty::keys`): a menu that answered the
        // first and went quiet has not answered the rest, so the call looks
        // once the last is typed and the screen has been quiet since.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"\x1b[?1049h> apple\r\n  banana");
        let since = io.begin_wait();
        io.note_input(b"\x1b[B");
        io.typing_for(Duration::from_millis(900));
        io.absorb(b"\x1b[H  apple\r\n> banana");
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(1_300), "{took:?}");
    }

    #[test]
    fn a_screen_that_keeps_redrawing_is_answered_within_seconds() {
        // `watch -n 0.1`, a clock, a meter: never quiet, and still a program
        // that takes keys — the call returns its screen after SCREEN_BUSY.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"\x1b[?1049h\x1b[H00.0");
        let since = io.begin_wait();
        let drawer = {
            let io = Arc::clone(&io);
            std::thread::spawn(move || {
                for n in 1..=40 {
                    std::thread::sleep(Duration::from_millis(100));
                    io.absorb(format!("\x1b[H{:02}.{}", n / 10, n % 10).as_bytes());
                }
            })
        };
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        let took = started.elapsed();
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(took >= settle::SCREEN_BUSY, "{took:?}");
        assert!(
            took < settle::SCREEN_BUSY + Duration::from_millis(500),
            "{took:?}"
        );
        assert!(
            io.waiting(WaitKind::Input, since),
            "the report says it takes keys"
        );
        drawer.join().unwrap();
    }

    #[test]
    fn a_main_screen_redrawn_for_keys_is_answered_like_a_full_screen() {
        // `top` repaints the main screen by cursor addressing, reading keys
        // one at a time: the same program as a full-screen one. A command
        // that addresses the screen for a progress bar (apt's scroll region)
        // reads no keys, and is waited out as before.
        for reading_keys in [true, false] {
            let io = Arc::new(SessionIo::new(true));
            io.set_reading_keys(reading_keys);
            io.absorb(b"\x1b[H\x1b[2Jtop - 00.0");
            let since = io.begin_wait();
            let drawer = {
                let io = Arc::clone(&io);
                std::thread::spawn(move || {
                    for n in 1..=30 {
                        std::thread::sleep(Duration::from_millis(100));
                        io.absorb(format!("\x1b[Htop - {:02}.{}", n / 10, n % 10).as_bytes());
                    }
                })
            };
            let end = io.wait(
                WaitKind::Input,
                since,
                Duration::from_millis(2_600),
                &never,
                &never,
                &mut |_, _| {},
            );
            let expected = if reading_keys {
                Settle::Prompt
            } else {
                Settle::Timeout
            };
            assert_eq!(
                end,
                WaitEnd::Settled(expected),
                "reading keys: {reading_keys}"
            );
            drawer.join().unwrap();
        }
    }

    /// The terminal's modes as the monitor reads them off it
    /// (`pty::spawn::line_mode`): a program reading whole lines, one reading
    /// key by key, a relay holding the terminal raw, a password prompt.
    const LINE_MODE: LineMode = LineMode {
        canonical: true,
        echo: true,
        processed_output: true,
    };
    const KEY_BY_KEY: LineMode = LineMode {
        canonical: false,
        echo: false,
        processed_output: true,
    };
    const RELAY: LineMode = LineMode {
        canonical: false,
        echo: false,
        processed_output: false,
    };
    const HIDDEN_LINE: LineMode = LineMode {
        canonical: true,
        echo: false,
        processed_output: true,
    };

    /// `gh run watch`'s frame as it draws it: the alternate screen cleared
    /// from home, the status block, the cursor at the start of the line under
    /// it — and nothing that reads a key.
    const GH_FRAME: &[u8] = b"\x1b[?1049h\x1b[0;0H\x1b[JRefreshing run status every 30 seconds. \
        Press Ctrl+C to quit.\r\n\r\n* v0.7.0 Release \xc2\xb7 36209800650\r\n\r\nJOBS\r\n\
        * build (ID 108313779167)\r\n";

    #[test]
    fn a_display_on_the_alternate_screen_that_reads_whole_lines_asks_nothing() {
        // `gh run watch`: the alternate screen, redrawn every few seconds,
        // and the terminal left in line mode, where no single key reaches the
        // program. A full-screen program that takes keys turns line mode off
        // first; one that leaves it on is a display, and its quiet screen is
        // no prompt, whichever call looks.
        let io = SessionIo::waited(true);
        io.set_line_mode(LINE_MODE);
        io.absorb(GH_FRAME);
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert!(!io.waiting(kind, io.origin_mark()), "{kind:?}");
        }
    }

    #[test]
    fn a_wait_on_a_display_that_reads_whole_lines_rides_it_out() {
        // The reported case: `gh run watch` started in the background, then
        // waited on. Its first frame — never looked at, quiet between
        // redraws — ended the wait within seconds as "waiting for input", and
        // the model killed a command doing exactly what it had asked of it.
        let io = Arc::new(SessionIo::new(true));
        io.set_line_mode(LINE_MODE);
        io.absorb(GH_FRAME);
        std::thread::sleep(settle::WAIT_PROMPT_QUIET);
        let since = io.begin_wait();
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(1_500),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        let waiting = report::Waiting::of(io.waiting(WaitKind::Wait, since), io.password_prompt());
        let look = io.look("s1", Status::Running { waiting });
        assert!(look.starts_with("Running (session s1)\nScreen ("), "{look}");
    }

    #[test]
    fn a_display_that_reads_whole_lines_is_waited_out_like_a_batch_command() {
        // A full-screen program that never stops drawing takes keys all the
        // while, so a launch returns its screen after SCREEN_BUSY; a display
        // in line mode takes none, and runs on like any command printing as
        // it works. The line mode alone tells the two apart: a key reader, a
        // relay holding the terminal raw, or a mode not read yet all keep the
        // screen answer.
        for (mode, expected) in [
            (Some(LINE_MODE), Settle::Timeout),
            (Some(KEY_BY_KEY), Settle::Prompt),
            (Some(RELAY), Settle::Prompt),
            (None, Settle::Prompt),
        ] {
            let io = Arc::new(SessionIo::waited(true));
            if let Some(mode) = mode {
                io.set_line_mode(mode);
            }
            io.absorb(b"\x1b[?1049h\x1b[H00.0");
            let drawer = {
                let io = Arc::clone(&io);
                std::thread::spawn(move || {
                    for n in 1..=30 {
                        std::thread::sleep(Duration::from_millis(100));
                        io.absorb(format!("\x1b[H{:02}.{}", n / 10, n % 10).as_bytes());
                    }
                })
            };
            let end = io.wait(
                WaitKind::Launch,
                io.origin_mark(),
                Duration::from_millis(2_600),
                &never,
                &never,
                &mut |_, _| {},
            );
            assert_eq!(end, WaitEnd::Settled(expected), "{mode:?}");
            drawer.join().unwrap();
        }
    }

    #[test]
    fn the_alternate_screen_still_asks_whenever_a_key_could_reach_it() {
        // Line mode rules out only a key: a program reading key by key, a
        // relay holding the terminal raw for a program behind it, or a mode
        // not read yet still wait on keys wherever the cursor is; and in line
        // mode a prompt by the cursor, a read the kernel saw, or a line read
        // with echo off are questions still.
        let quiet = |mode: Option<LineMode>, text: &[u8], probe: Option<Probe>| {
            let io = SessionIo::new(true);
            if let Some(mode) = mode {
                io.set_line_mode(mode);
            }
            io.absorb(text);
            if let Some(probe) = probe {
                io.set_probe(probe);
            }
            std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
            io
        };
        for mode in [Some(KEY_BY_KEY), Some(RELAY), None] {
            let io = quiet(mode, GH_FRAME, None);
            assert!(io.waiting(WaitKind::Launch, io.origin_mark()), "{mode:?}");
        }
        let prompt = quiet(Some(LINE_MODE), b"\x1b[?1049h\x1b[HName: ", None);
        assert!(
            prompt.waiting(WaitKind::Launch, prompt.origin_mark()),
            "a prompt by the cursor"
        );
        let read = quiet(Some(LINE_MODE), GH_FRAME, Some(Probe::Reading));
        assert!(
            read.waiting(WaitKind::Launch, read.origin_mark()),
            "a read the kernel saw"
        );
        let secret = quiet(Some(HIDDEN_LINE), GH_FRAME, None);
        assert!(secret.password_prompt(), "a line read with echo off");
        assert!(secret.waiting(WaitKind::Launch, secret.origin_mark()));
    }

    #[test]
    fn a_background_display_that_reads_whole_lines_asks_nothing() {
        // Nobody waits on it, and its screen changed since the model looked:
        // the monitor tells the model of a question it has not seen — but a
        // display in line mode poses none, and is not even probed for one.
        let io = SessionIo::new(true);
        io.announce();
        io.set_line_mode(LINE_MODE);
        io.absorb(GH_FRAME);
        std::thread::sleep(PROBE_QUIET + Duration::from_millis(20));
        assert!(!io.wants_probe(), "nothing shaped like a question");
        assert!(!io.take_unseen_prompt(Duration::ZERO));
        // The same screen over a key reader is still one to tell of.
        let menu = SessionIo::new(true);
        menu.announce();
        menu.set_line_mode(KEY_BY_KEY);
        menu.absorb(GH_FRAME);
        assert!(menu.take_unseen_prompt(Duration::ZERO));
    }

    #[test]
    fn a_program_waiting_elsewhere_asks_nothing_and_is_not_at_work_either() {
        // An event loop between network calls with `Fetching... ` left open:
        // the kernel sees it wait on a socket, not the terminal
        // (`pty::probe::Probe::Elsewhere`). No prompt — but no busy tree
        // either, so the quiet line still hands an input's call back.
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        io.absorb(b"Fetching... ");
        io.set_probe(Probe::Elsewhere);
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Quiet));
        assert!(!io.waiting(WaitKind::Input, since));
    }

    #[test]
    fn a_program_waiting_elsewhere_poses_no_question_at_all() {
        // Echo left off, a question-shaped line in the background, a screen
        // over a terminal in raw mode: none of it is asked by a program the
        // kernel sees waiting on something other than the terminal.
        let hidden = SessionIo::new(true);
        hidden.absorb(b"Password: ");
        hidden.set_hidden_input(true);
        hidden.set_probe(Probe::Elsewhere);
        assert!(!hidden.password_prompt());
        let background = SessionIo::new(true);
        background.announce();
        background.absorb(b"Continue? ");
        background.set_probe(Probe::Elsewhere);
        assert!(!background.take_unseen_prompt(Duration::ZERO));
        let display = SessionIo::new(true);
        display.set_line_mode(KEY_BY_KEY);
        display.absorb(GH_FRAME);
        display.set_probe(Probe::Elsewhere);
        std::thread::sleep(settle::PROMPT_QUIET + Duration::from_millis(50));
        assert!(!display.waiting(WaitKind::Launch, display.origin_mark()));
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
        // Never looked at before: the whole output, as data.
        assert_eq!(
            io.look("s1", Status::Exited(Some(0))),
            "Exit code: 0\ndone\n"
        );
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
             Screen (40x120, cursor at line 2, column 2 \u{2014} \"~\u{2038}\"):\n~\n~",
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
    fn a_pipe_sessions_lines_each_start_at_the_left_edge() {
        // A server's log in the background: read as a terminal's line feed,
        // a pipe's bare `\n` kept the column, so each line began where the
        // one before it ended — and once that passed MAX_LINE_CHARS the
        // newest lines were dropped whole.
        let io = SessionIo::new(false);
        let log: String = (1..=400)
            .map(|n| format!("127.0.0.1 - - \"GET /{n} HTTP/1.1\" 200 -\n"))
            .collect();
        io.absorb(log.as_bytes());
        let look = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::No,
            },
        );
        let mut lines = look.lines();
        assert_eq!(lines.next(), Some("Running (session s1)"));
        assert_eq!(
            lines.next(),
            Some("127.0.0.1 - - \"GET /1 HTTP/1.1\" 200 -")
        );
        assert_eq!(
            lines.last(),
            Some("127.0.0.1 - - \"GET /400 HTTP/1.1\" 200 -")
        );
        assert!(look.lines().all(|line| !line.starts_with(' ')), "{look}");
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
        // The call that met the prompt reported it; the model polls again.
        look_at(&io);
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
        io.note_input(b"\x1b[B");
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
    fn a_row_that_merely_ends_like_the_key_holds_nothing() {
        // `1` typed into top: its last row happens to end in `1`, but the
        // cursor is parked at the screen's edge, nowhere near it — the key
        // went to the program, not onto a line waiting for Enter.
        let io = SessionIo::new(true);
        io.absorb(b"   21 root   rt   0 S  0.0  0:00.27 migration/1\x1b[1;120H");
        assert!(!io.holds_typed("1"));
        let io = SessionIo::new(true);
        io.absorb(b">>> x = 1");
        assert!(io.holds_typed("x = 1"), "right after it, it is held");
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
    fn a_command_that_exits_in_its_launch_reports_its_output_as_data() {
        // Tabs and trailing spaces are data a model copies back
        // (docs/bash-tools.md); the transcript is what a person reads, and
        // expands and trims them.
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"col1\tcol2   \r\n");
        let _ = io.finish(Some(0));
        let end = io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            LONG,
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Exited));
        assert_eq!(
            io.look("s1", Status::Exited(Some(0))),
            "Exit code: 0\ncol1\tcol2   \n"
        );
    }

    #[test]
    fn a_long_launch_output_keeps_both_ends_and_names_its_log() {
        let io = Arc::new(SessionIo::waited(true));
        io.set_log(std::path::PathBuf::from("/tmp/t/b1.output"));
        for n in 1..=20_000 {
            io.absorb(format!("{n}\r\n").as_bytes());
        }
        let _ = io.finish(Some(0));
        let report = io.look("s1", Status::Exited(Some(0)));
        assert!(report.starts_with("Exit code: 0\n1\n2\n3\n"), "{report}");
        assert!(
            report.contains("lines omitted — the full output is in /tmp/t/b1.output]\n"),
            "{report}"
        );
        assert!(report.ends_with("\n19999\n20000\n"), "{report}");
    }

    #[test]
    fn output_that_moves_back_over_its_lines_keeps_the_transcript() {
        // A multi-line progress display redrawn with cursor-up: the fold,
        // one line at a time, would repeat every redraw — the transcript
        // follows it (docs/interactive-shell.md).
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"a\r\nb\r\n\x1b[2Aa!\r\n");
        let _ = io.finish(Some(0));
        assert_eq!(
            io.look("s1", Status::Exited(Some(0))),
            "Exit code: 0\na!\nb"
        );
    }

    #[test]
    fn an_exit_after_a_look_reports_only_what_is_new() {
        // The data view is a launch's one report of everything; a session
        // the model has looked at reports what came since.
        let io = Arc::new(SessionIo::waited(true));
        io.absorb(b"first\r\n");
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::No,
            },
        );
        io.absorb(b"second\r\n");
        let _ = io.finish(Some(0));
        assert_eq!(
            io.look("s1", Status::Exited(Some(0))),
            "Exit code: 0\nsecond"
        );
    }

    #[test]
    fn a_chunk_longer_than_the_transcript_keeps_every_line_in_the_stream() {
        // The stream is the session's log, which a long output's cut names
        // (docs/bash-tools.md): a burst read in one chunk with more lines
        // than the transcript retains lost lines from the middle of it.
        let io = SessionIo::new(true);
        let burst: String = (1..=5000).map(|n| format!("{n}\r\n")).collect();
        let (_, committed) = io.absorb(burst.as_bytes());
        let lines: Vec<&str> = committed.lines().collect();
        assert_eq!(lines.len(), 5000);
        assert_eq!(lines.first(), Some(&"1"));
        assert_eq!(lines.last(), Some(&"5000"));
    }

    #[test]
    fn a_chunk_is_cut_after_every_so_many_line_feeds() {
        let pieces: Vec<&[u8]> = line_slices(b"a\nb\x0bc\x0cd\ne", 2).collect();
        assert_eq!(pieces, [&b"a\nb\x0b"[..], b"c\x0cd\n", b"e"]);
        assert_eq!(line_slices(b"", 2).count(), 0);
        assert_eq!(line_slices(b"no feed", 2).collect::<Vec<_>>(), [b"no feed"]);
    }

    #[test]
    fn a_background_prompt_nobody_saw_is_told_once() {
        let io = SessionIo::new(true);
        io.absorb(b"Port 3000 is in use. Use another? (Y/n) ");
        assert!(
            !io.take_unseen_prompt(Duration::ZERO),
            "a session never announced is its call's to report"
        );
        io.announce();
        assert!(
            !io.take_unseen_prompt(Duration::from_secs(60)),
            "not quiet long enough yet"
        );
        assert!(io.take_unseen_prompt(Duration::ZERO));
        assert!(!io.take_unseen_prompt(Duration::ZERO), "once per prompt");
        // Told and not yet looked at: a program that keeps asking is not
        // told of again until the model has looked.
        io.absorb(b"\r\nUse another? (Y/n) ");
        assert!(!io.take_unseen_prompt(Duration::ZERO));
        // A prompt the model looked at is not news; the next one is.
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert!(!io.take_unseen_prompt(Duration::ZERO));
        io.absorb(b"\r\nready> ");
        assert!(io.take_unseen_prompt(Duration::ZERO));
    }

    #[test]
    fn a_background_line_of_output_is_not_a_prompt() {
        // A server's log ends at the start of a line: nothing to answer, and
        // nothing the monitor probes for.
        let log = SessionIo::new(true);
        log.announce();
        log.absorb(b"listening on :3000\r\n");
        assert!(!log.take_unseen_prompt(Duration::ZERO));
        // Nor is a prompt-shaped line the kernel sees a program at work
        // behind: `Compiling... ` left open while the compiler runs.
        let busy = SessionIo::new(true);
        busy.announce();
        busy.absorb(b"Compiling... ");
        busy.set_probe(Probe::Idle);
        assert!(!busy.take_unseen_prompt(Duration::ZERO));
        // A pipe has no prompt to tell of.
        let pipe = SessionIo::new(false);
        pipe.announce();
        pipe.absorb(b"Continue? ");
        assert!(!pipe.take_unseen_prompt(Duration::ZERO));
    }

    #[test]
    fn a_quiet_background_prompt_is_probed_until_it_is_told() {
        // The probe runs while a call waits; with none waiting it runs only
        // for a session that may be asking something unseen, so an idle
        // server printing logs is never walked.
        let io = SessionIo::new(true);
        io.announce();
        io.absorb(b"Overwrite? [y/N] ");
        std::thread::sleep(PROBE_QUIET);
        assert!(io.wants_probe());
        assert!(io.take_unseen_prompt(Duration::ZERO));
        assert!(!io.wants_probe(), "told: nothing left to find out");
        let log = SessionIo::new(true);
        log.announce();
        log.absorb(b"GET / 200\r\n");
        std::thread::sleep(PROBE_QUIET);
        assert!(!log.wants_probe());
    }

    #[test]
    fn a_program_seen_at_work_outlasts_the_quiet_line() {
        // A command that answered with whole lines and went quiet used to
        // hand its call back after LINE_QUIET whatever it was doing; one the
        // kernel sees at work is working (docs/bash-tools.md), and runs on to
        // the call's timeout.
        let io = Arc::new(SessionIo::new(true));
        let since = io.begin_wait();
        io.absorb(b"compiling\r\n");
        io.set_probe(Probe::Idle);
        let started = Instant::now();
        let end = io.wait(
            WaitKind::Input,
            since,
            settle::LINE_QUIET + Duration::from_millis(600),
            &never,
            &never,
            &mut |_, _| {},
        );
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
        assert!(started.elapsed() > settle::LINE_QUIET);
    }

    #[test]
    fn a_wait_on_a_key_reader_whose_prompt_was_seen_returns_at_once() {
        // A REPL at the prompt the model's last look already showed: the wait
        // ends in a moment, not at its budget (docs/bash-tools.md).
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b">>> ");
        io.set_reading_keys(true);
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        let since = io.begin_wait();
        let started = Instant::now();
        let end = io.wait(WaitKind::Wait, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn input_makes_the_verdict_stale_too() {
        let io = SessionIo::new(true);
        io.set_probe(Probe::Reading);
        io.note_input(b"x\r");
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
        io.note_input(b"hunter2\r");
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
        io.note_input(b"hunter2\r");
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

    /// The model looks at the session, as a call's report does.
    fn look_at(io: &SessionIo) {
        let _ = io.look(
            "s1",
            Status::Running {
                waiting: Waiting::No,
            },
        );
    }

    #[test]
    fn a_question_the_model_has_not_seen_ends_a_wait_begun_after_it() {
        // Asked after the model's last look and before its wait began — while
        // it was thinking: new to the model, so the wait ends on it as if it
        // had watched it arrive.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"working\r\n");
        look_at(&io);
        io.absorb(b"Proceed? [Y/n] ");
        std::thread::sleep(settle::WAIT_PROMPT_QUIET);
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
    }

    #[test]
    fn a_submitted_password_is_waited_on_until_the_program_answers() {
        // sudo checks a password for seconds before it says a word — longer
        // than LINE_QUIET. The call waits for the verdict, not the silence.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"[sudo] password for u: ");
        io.set_hidden_input(true);
        let since = io.begin_wait();
        io.note_input(b"letmein\r");
        // It read the line, turned echo back on, and broke the line.
        io.set_hidden_input(false);
        io.absorb(b"\r\n");
        let verdict = {
            let io = Arc::clone(&io);
            std::thread::spawn(move || {
                std::thread::sleep(settle::LINE_QUIET + Duration::from_millis(500));
                io.set_hidden_input(true);
                io.absorb(b"Sorry, try again.\r\n[sudo] password for u: ");
            })
        };
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        verdict.join().expect("verdict");
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(io.password_prompt());
    }

    #[test]
    fn once_a_password_is_answered_silence_ends_the_call_again() {
        // Accepted: the command's first words are the answer, and a pause
        // after them is the usual quiet line.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"[sudo] password for u: ");
        io.set_hidden_input(true);
        let since = io.begin_wait();
        io.note_input(b"hunter2\r");
        io.set_hidden_input(false);
        io.absorb(b"\r\nresolving dependencies...\r\n");
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Quiet));
        assert!(started.elapsed() < settle::LINE_QUIET + Duration::from_millis(500));
    }

    #[test]
    fn a_wait_on_unseen_output_hears_the_probe_first() {
        // A prompt-shaped line printed before the wait, over a busy command:
        // settling on the wait's first look would beat the probe, which —
        // asked once the wait begins — sees every process at work.
        let io = Arc::new(SessionIo::new(true));
        look_at(&io);
        io.absorb(b"Working... ");
        std::thread::sleep(settle::WAIT_PROMPT_QUIET);
        let since = io.begin_wait();
        let prober = {
            let io = Arc::clone(&io);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                io.set_probe(Probe::Idle);
            })
        };
        let end = io.wait(
            WaitKind::Wait,
            since,
            Duration::from_millis(1500),
            &never,
            &never,
            &mut |_, _| {},
        );
        prober.join().expect("prober");
        assert_eq!(end, WaitEnd::Settled(Settle::Timeout));
    }

    #[test]
    fn a_question_the_model_has_seen_leaves_a_wait_to_its_timeout() {
        // It was in the last report: the model chose to wait anyway.
        let io = Arc::new(SessionIo::new(true));
        io.absorb(b"Proceed? [Y/n] ");
        look_at(&io);
        let since = io.begin_wait();
        let end = io.wait(
            WaitKind::Wait,
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
        io.note_input(b"hunter2");
        assert!(io.password_prompt());
        let started = Instant::now();
        let end = io.wait(WaitKind::Input, since, LONG, &never, &never, &mut |_, _| {});
        assert_eq!(end, WaitEnd::Settled(Settle::Prompt));
        assert!(
            started.elapsed() < settle::LINE_QUIET,
            "not the quiet fallback"
        );
        io.note_input(b"\r");
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
    fn the_probe_waits_for_the_keys_to_be_typed() {
        // A program between two of the call's keys is blocked reading them:
        // asked then, the kernel would say it waits for input when what it
        // does with the last key is still to come.
        let io = SessionIo::new(true);
        let _since = io.begin_wait();
        io.note_input(b"\x1b[B");
        io.typing_for(Duration::from_millis(400));
        std::thread::sleep(PROBE_QUIET + Duration::from_millis(20));
        assert!(!io.wants_probe(), "keys are still being typed");
        std::thread::sleep(Duration::from_millis(400));
        assert!(io.wants_probe(), "quiet since the last key");
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

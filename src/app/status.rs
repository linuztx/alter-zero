//! The turn's live accounting: the [`TurnStatus`] the spinner line renders, the
//! token tally it counts into, and the [`TurnSummary`] committed when the turn
//! ends. See `docs/status-indicator.md`.

use super::*;

/// Which way the live token tally is moving, selecting the arrow glyph in the
/// status line: [`Down`] (`↓`) while the reply streams (output tokens), flipping
/// to [`Up`] (`↑`) right after a tool result (its output "uploaded" back). The
/// tally itself is cumulative and never resets when the arrow flips.
///
/// [`Down`]: TokenArrow::Down
/// [`Up`]: TokenArrow::Up
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenArrow {
    /// `↓` — output tokens, while the reply text streams.
    Down,
    /// `↑` — after a tool result is folded back in.
    Up,
}

/// The live status of the turn in flight, shown as a codex-style status line in
/// the strip above the input box (`● {verb}… ({elapsed}s · {arrow} {n} tokens ·
/// Thinking for {m}s)`). `Some` on [`App`] from [`App::begin_stream`] until the
/// turn ends; see `docs/status-indicator.md`.
///
/// `verb`/`done_verb`, `tokens`, and `arrow` are pure turn state. `elapsed` and
/// `thinking` are **written by the I/O boundary each frame**
/// ([`App::set_status_times`]) — time is impure, so it never reaches the pure
/// core except as these already-computed values (mirrors the timestamp clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStatus {
    /// The whimsical working verb shown live (e.g. `Working`), fixed for the turn.
    pub verb: &'static str,
    /// The matching done verb for the committed summary (e.g. `Done`).
    pub done_verb: &'static str,
    /// Cumulative token estimate for the whole turn (text **and** tool output);
    /// never reset mid-turn. Shown only when > 0.
    pub tokens: usize,
    /// Which arrow the tally shows (`↓` streaming / `↑` after a tool).
    pub arrow: TokenArrow,
    /// Time since the turn was submitted — set by the boundary each frame. The
    /// renderer derives both the displayed whole seconds and the verb's shimmer
    /// phase from it (sub-second resolution drives the wave).
    pub elapsed: Duration,
    /// How long the *current* thinking phase has run, or `None` when not thinking
    /// — set by the boundary each frame. `Some` renders the `Thinking for Ns`
    /// suffix; cleared the moment thinking ends.
    pub thinking: Option<Duration>,
    /// Whether this is a `!` shell-command turn ([`App::begin_shell`]). A shell
    /// turn ends **without** a `"{verb} for Ns"` summary — the committed cell
    /// is its own record ([`App::end_turn`] returns `None`). See
    /// `docs/shell-command.md`.
    pub shell: bool,
    /// Set while a failed request is being retried — the connection/send failed
    /// (or a transient status came back) before any content streamed, so the
    /// backend is reconnecting. Rendered as a `retrying {attempt}/{max}` clause
    /// in the status line and cleared the moment content arrives ([`App::push_chunk`]
    /// / [`App::push_thinking`]). Set by [`App::set_retry`] from the backend's
    /// [`crate::stream::StreamEvent::Retrying`]. See `docs/llm.md`.
    pub retry: Option<RetryInfo>,
}

/// A live retry indicator for the status line: the 1-based retry number and the
/// ceiling (`retrying {attempt}/{max}`). See [`TurnStatus::retry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryInfo {
    /// Which retry this is (1-based — the first retry is `1`).
    pub attempt: u32,
    /// The maximum number of retries ([`crate::llm::retry::MAX_RETRIES`]).
    pub max: u32,
}

/// A finished turn's summary, committed to scrollback as a dim `"{verb} for
/// {secs}s"` line and kept in [`App::history`] so it survives a resize and lists
/// in the Ctrl+O transcript (with a timestamp, like every other item).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummary {
    /// The done verb chosen for the turn (e.g. `Done`, `Finished`).
    pub verb: &'static str,
    /// The turn's total wall-clock duration in whole seconds.
    pub secs: u64,
    /// Wall-clock stamp of when the turn finished. Recorded but not currently
    /// displayed — only user-message stamps show. See `docs/timestamps.md`.
    pub timestamp: String,
    /// How many background shells were still running when the turn ended —
    /// rendered as a `· {n} shells still running` suffix when non-zero
    /// (`Done for 22s · 3 shells still running`). Snapshotted at
    /// [`App::end_turn`]; not persisted to the session file (a resumed
    /// session's shells are gone). See `docs/background.md`.
    pub shells: usize,
    /// The turn's **real** billed tokens (input + output summed over the
    /// turn's request rounds), from the provider's usage frames
    /// ([`App::apply_usage`]) — rendered as a `· {n} tokens` suffix. `0` when
    /// the backend reported none (the dummy, a `!` shell), which hides the
    /// clause. Persisted with the summary. See `docs/prompt-caching.md`.
    pub tokens: usize,
    /// How many of those input tokens were served from the provider's prompt
    /// cache — the `({n} cached)` suffix beside `tokens` when non-zero, the
    /// visible proof caching is working. See `docs/prompt-caching.md`.
    pub cached: usize,
    /// How many input tokens this turn **wrote** to the provider's prompt
    /// cache (explicit caching bills a write at a premium; an implicit one
    /// reports it or not) — the `({n} written)` half of the receipt's
    /// parenthetical when non-zero, so the turn that *primed* the cache
    /// shows it did. See `docs/prompt-caching.md`.
    pub cache_write: usize,
}

/// The active model's reasoning state: what the `/v1/models` record said it
/// supports and the mode the user has cycled to (Ctrl+T). Lives on
/// [`App::thinking`]; the footer shows `mode.label()` beside the model name
/// and the boundary threads the mode into each request. See
/// `docs/reasoning.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingState {
    /// The capability parsed from the provider's model record.
    pub support: ReasoningSupport,
    /// The currently selected mode (defaults to medium where offered).
    pub mode: ThinkingMode,
}

impl App {
    /// Count a just-submitted user message into the live tally as **uploaded
    /// input** — the tokens grow and the arrow points `↑` (like a tool result
    /// folded back in, the reverse of streaming output). Called right after
    /// [`begin_stream`] with the turn's prompt, so the status shows
    /// `↑ N tokens` while the model spins up before its first chunk (the first
    /// [`push_chunk`] flips the arrow back to `↓`). The tally is **added to**,
    /// never reset (see `docs/status-indicator.md`). No-op when no turn is in
    /// flight.
    ///
    /// [`begin_stream`]: App::begin_stream
    /// [`push_chunk`]: App::push_chunk
    pub fn count_user_input(&mut self, text: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(text);
            status.arrow = TokenArrow::Up;
        }
    }
    /// Count a streamed reasoning delta into the live token tally (arrow down —
    /// it is model output, streaming) **without** touching the reply buffer.
    /// This is what keeps the count ticking while the status line shows
    /// `Thinking for Ns`. No-op when no turn is in flight.
    ///
    /// When a thinking phase is **open** ([`begin_reasoning`]) the delta also
    /// lands in its buffer, so the strip's live block shows the model thinking
    /// and the phase can settle into a `Thought for …` cell. With the
    /// display off no phase is ever opened, so this only counts — the
    /// pre-feature behaviour, unchanged. See `docs/thinking-stream.md`.
    ///
    /// [`begin_reasoning`]: App::begin_reasoning
    pub fn push_thinking(&mut self, chunk: &str) {
        if let Some(buf) = self.reasoning.as_mut() {
            buf.push_str(chunk);
        }
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Reasoning is content too — the retrying request recovered.
            status.retry = None;
        }
    }
    /// Count a streamed tool-call generation delta (the model emitting a tool
    /// call's `name`/`arguments` fragments) into the live token tally (arrow
    /// down — it is model output, streaming) **without** touching the reply
    /// buffer: the fragment is opaque JSON, never rendered. This keeps the
    /// status count ticking while the model *generates* a tool call, exactly
    /// like reasoning ticks it while the model thinks (see
    /// `docs/status-indicator.md`). No-op when no turn is in flight.
    pub fn push_tool_call_progress(&mut self, chunk: &str) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count_tokens(chunk);
            status.arrow = TokenArrow::Down;
            // Generating the call is content too — a retrying request recovered.
            status.retry = None;
        }
    }
    /// Fold one provider usage report (the round's final `usage` frame,
    /// [`crate::stream::StreamEvent::Usage`]) into the turn: accumulate the
    /// billed total and its cached share, and **snap the live tally to the
    /// accumulated real number** — replacing the tiktoken estimate ticked so
    /// far, which never saw the system prompt or the re-sent context. Later
    /// estimates (the next round's stream) tick on top of the snapped base,
    /// and that round's own usage frame snaps again — so the tally is always
    /// "everything billed so far, plus the current round's live estimate".
    /// No-op when no turn is in flight. See `docs/prompt-caching.md`.
    pub fn apply_usage(&mut self, usage: &crate::stream::TokenUsage) {
        if self.status.is_none() {
            return;
        }
        // One frame ends one round: this is where the round's `Thought for
        // …` cells trade their tokenizer estimate for the provider's own
        // `reasoning_tokens` (docs/thinking-stream.md).
        self.snap_round_reasoning(usage.reasoning);
        self.turn_usage_tokens += usize::try_from(usage.total()).unwrap_or(usize::MAX);
        self.turn_usage_cached += usize::try_from(usage.cached).unwrap_or(usize::MAX);
        self.turn_usage_cache_write += usize::try_from(usage.cache_write).unwrap_or(usize::MAX);
        if let Some(status) = self.status.as_mut() {
            status.tokens = self.turn_usage_tokens;
        }
        // The round's `input` is the whole re-sent context and its `output`
        // joins the next round's — their sum is the live context gauge
        // (docs/compact.md), authoritative until a history mutation stales it.
        self.context_used = usage.input.saturating_add(usage.output);
    }
    /// Record that a failed request is being retried, so the status line shows
    /// `retrying {attempt}/{max}` while the backend reconnects. The 1-based
    /// `attempt` and the ceiling `max` come straight from the backend's
    /// [`crate::stream::StreamEvent::Retrying`]. Cleared by the next
    /// [`push_chunk`](App::push_chunk)/[`push_thinking`](App::push_thinking) once
    /// content arrives. No-op when no turn is in flight. See `docs/llm.md`.
    pub fn set_retry(&mut self, attempt: u32, max: u32) {
        if let Some(status) = self.status.as_mut() {
            status.retry = Some(RetryInfo { attempt, max });
        }
    }
    /// The live turn status, if a turn is in flight.
    #[must_use]
    pub const fn status(&self) -> Option<&TurnStatus> {
        self.status.as_ref()
    }
    /// Is a turn in flight (its status line should show)? True from
    /// [`begin_stream`] until the turn ends — drives the loop's per-second tick.
    ///
    /// [`begin_stream`]: App::begin_stream
    #[must_use]
    pub const fn turn_active(&self) -> bool {
        self.status.is_some()
    }
    /// Write the boundary-computed times onto the live status before a draw: how
    /// long the turn has run (`elapsed` — it also drives the verb's shimmer
    /// phase), and the current thinking-phase duration (`Some` while thinking,
    /// `None` otherwise). No-op when no turn is in flight. Time is impure, so it
    /// only ever reaches the status this way.
    pub fn set_status_times(&mut self, elapsed: Duration, thinking: Option<Duration>) {
        if let Some(status) = self.status.as_mut() {
            status.elapsed = elapsed;
            status.thinking = thinking;
        }
    }
    /// Inject the current running command's elapsed each frame (the
    /// [`set_status_times`](App::set_status_times) pattern — the clock lives at
    /// the boundary, started at the call's `ToolStart` / the `!` shell's
    /// launch). `None` when no command is running. Two readers: the running
    /// cell **displays** it — the `bash` cell's `+N lines (Ns · timeout …)`
    /// clock row, the `!` shell's `⎿ Running… (Ns)` row
    /// ([`command_elapsed`](App::command_elapsed),
    /// `docs/tool-streaming.md`) — and the preview delays the `(ctrl+b to run
    /// in background)` hint on it until a command has run a few seconds, so a
    /// fast command never flashes it
    /// ([`background_hint_elapsed`](App::background_hint_elapsed),
    /// `docs/background.md`).
    pub fn set_command_elapsed(&mut self, elapsed: Option<Duration>) {
        self.command_elapsed = elapsed;
    }

    /// Inject the **animation frame clock** before a draw (the
    /// [`set_status_times`](App::set_status_times) pattern): time since the loop
    /// started, monotonically increasing. It is a *phase*, not a measurement —
    /// nothing displays it — so its epoch is arbitrary; what matters is that it
    /// advances with the loop's 32 ms re-arm.
    ///
    /// One clock for every blinking bullet, so a round's tool cells and its agent
    /// tree blink in unison rather than each on its own timer. Unlike the
    /// turn's `elapsed` it keeps running between turns, so a background agent's
    /// live cell animates too. See `docs/tool-pulse.md`.
    pub fn set_pulse(&mut self, pulse: Duration) {
        self.pulse = pulse;
    }

    /// The current animation phase — see [`set_pulse`](App::set_pulse). Zero
    /// until the boundary injects one (the unit-test default), which simply
    /// renders every blinking bullet shown — the top of its cycle.
    #[must_use]
    pub const fn pulse(&self) -> Duration {
        self.pulse
    }
    /// The running command's elapsed **as the Ctrl+B hint's gate** — the
    /// injected [`command_elapsed`](App::command_elapsed), or `None` wherever
    /// the `(ctrl+b to run in background)` hint it delays must not show.
    /// Named for that one job: the plain getter answers what the setter set,
    /// and only this one is masked.
    ///
    /// `None` while a tool-permission prompt is open: the prompt owns
    /// every key, so Ctrl+B does nothing there — and the hint this gates would
    /// be advertising it (`docs/permissions.md`). Nothing is really *running*
    /// while a call waits on the user, either. The ↓ manager band and **every**
    /// composer-replacing picker (`/model`, `/login`, `/settings`, `/hooks`,
    /// `/skills`, `/mascot`, `/spinner`, `/theme`, `/donate`, `/export`) own
    /// every key the same way, so the running cell they keep visible above
    /// themselves stays hintless while one is open (`docs/background.md`,
    /// `docs/llm.md`, `docs/skills.md`, `docs/mascot.md`, `docs/spinner.md`,
    /// `docs/theme.md`, `docs/donate.md`, `docs/export.md`).
    #[must_use]
    pub fn background_hint_elapsed(&self) -> Option<Duration> {
        if self.permission.is_some()
            || self.background_view.is_some()
            || self.model_picker.is_some()
            || self.key_onboarding.is_some()
            || self.settings_picker.is_some()
            || self.hooks_menu.is_some()
            || self.skills_menu.is_some()
            || self.mascot_picker.is_some()
            || self.spinner_picker.is_some()
            || self.theme_picker.is_some()
            || self.donate_picker.is_some()
            || self.export_picker.is_some()
        {
            return None;
        }
        self.command_elapsed
    }
    /// How long the current running command has executed — exactly what
    /// [`set_command_elapsed`](App::set_command_elapsed) injected, `None` when
    /// no command is running (the boundary injects `None` at every
    /// resolution). This is what the running cell *displays*: the `bash`
    /// cell's `+N lines (Ns · timeout …)` clock row — on its `⎿ Running…`
    /// row before any output — and the `!` shell's `⎿ Running… (Ns)`
    /// row (`ui::running_command_lines`, `docs/tool-streaming.md`). It is the
    /// command's own clock, never the turn's — a `bash` call that starts a
    /// minute into a turn opens on `(0s)`, not on the status indicator's
    /// `60s` — and it is never masked: a picker or the ↓ band swallows
    /// Ctrl+B and so blanks the hint's gate
    /// ([`background_hint_elapsed`](App::background_hint_elapsed)), but keeps
    /// the running cell on screen above itself, still counting.
    #[must_use]
    pub const fn command_elapsed(&self) -> Option<Duration> {
        self.command_elapsed
    }
    /// Inject the streaming strip preview's row count before a draw (the
    /// [`set_status_times`] pattern): only the boundary's `ui::StreamRender`
    /// knows how many rows the preview renders — one for a normal reply, the
    /// whole forming block while a table streams — and `ui::preview_rows` /
    /// `ui::live_height` / `ui::cursor_position` must all reserve exactly what
    /// the strip draws. See `docs/table-streaming.md`.
    ///
    /// [`set_status_times`]: App::set_status_times
    pub fn set_stream_preview_rows(&mut self, rows: u16) {
        self.stream_preview_rows = rows;
    }
    /// The boundary-injected streaming-preview row count (see
    /// [`set_stream_preview_rows`]; 0 until a draw injects one — consumers
    /// floor at 1, the single-row preview).
    ///
    /// [`set_stream_preview_rows`]: App::set_stream_preview_rows
    #[must_use]
    pub const fn stream_preview_rows(&self) -> u16 {
        self.stream_preview_rows
    }
}

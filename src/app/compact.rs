//! `/compact` and auto-compaction: the summarization turn's buffer, the
//! [`Compaction`](super::Compaction) marker it appends, and the context gauge
//! that decides when to run it. See `docs/compact.md`.

use super::types::IMAGE_INPUT_TOKENS;
use super::*;

/// The fixed live-status verb for a `/compact` turn (the [`SHELL_VERB`]
/// pattern — not cycled): the status line reads `Compacting…`. It doubles as
/// the never-rendered done verb — a compact turn ends without a summary, the
/// `● Context compacted` cell being its record. See `docs/compact.md`.
pub const COMPACT_VERB: &str = "Compacting";

/// The auto-compact trigger's numerator/denominator: codex's threshold is
/// **90% of the context window** (`(context_window * 9) / 10`,
/// `ModelInfo::auto_compact_token_limit`). Past it, the loop starts a compact
/// turn on its own at the next idle boundary. See `docs/compact.md`.
const AUTO_COMPACT_NUMERATOR: u64 = 9;

const AUTO_COMPACT_DENOMINATOR: u64 = 10;

impl App {
    /// Begin a `/compact` turn (`docs/compact.md`), reusing the AI turn
    /// machinery like [`begin_shell`](App::begin_shell) so the summarization
    /// request gets the live status strip, `esc to interrupt`, and the
    /// mid-turn queueing for free:
    ///
    /// - an **empty** streaming buffer — [`is_streaming`](App::is_streaming)
    ///   is true (the strip shows, Enter queues, [`fail_stream`] works) but
    ///   the visible reply stays empty: chunks divert into the summary buffer
    ///   via [`push_chunk`](App::push_chunk), so the summary is never rendered
    ///   (codex parity) while the tally ticks;
    /// - a fixed-verb status ([`COMPACT_VERB`] → `Compacting…`) that does
    ///   **not** advance the cycled per-turn verbs (`turn_count` untouched).
    ///
    /// The boundary spawns the summarize request; [`finish_compact`] lands the
    /// marker at `StreamDone`, while an interrupt/error/`/clear` drops the
    /// buffer — the old context stands (codex swaps only at the very end).
    ///
    /// [`fail_stream`]: App::fail_stream
    /// [`finish_compact`]: App::finish_compact
    pub fn begin_compact(&mut self, auto: bool) {
        self.streaming = Some(String::new());
        self.compact_buffer = Some(String::new());
        self.compact_auto = auto;
        self.compact_before = self.context_used;
        self.stream_preview_rows = 1;
        // Per-turn usage accumulators reset with every turn machinery start.
        self.turn_usage_tokens = 0;
        self.turn_usage_cached = 0;
        // The summarizer's own thinking is never shown (the turn is invisible
        // by design) — `begin_reasoning` no-ops while the buffer below is
        // open; this only clears anything a previous turn abandoned.
        self.drop_reasoning();
        self.status = Some(TurnStatus {
            verb: COMPACT_VERB,
            // Never rendered: a compact turn ends without a summary (the
            // `● Context compacted` cell is its record).
            done_verb: COMPACT_VERB,
            tokens: 0,
            arrow: TokenArrow::Down,
            elapsed: Duration::ZERO,
            thinking: None,
            shell: false,
            retry: None,
        });
    }

    /// Whether a `/compact` turn is in flight (the summary buffer is open).
    #[must_use]
    pub const fn is_compacting(&self) -> bool {
        self.compact_buffer.is_some()
    }

    /// End a `/compact` turn: take the streamed summary, append the
    /// [`HistoryItem::Compaction`] marker (an *append* — the transcript, the
    /// recorder watermark, and the checkpoint keys all stay valid), and clear
    /// the turn state with **no** `Done for Ns` summary. Returns the appended
    /// marker for the boundary to commit the `● Context compacted` cell, or
    /// `None` when no compact turn was in flight. `elapsed_secs` is the
    /// summarization turn's wall-clock from the boundary's clock (the
    /// [`take_turn_summary`](App::take_turn_summary) shape) — recorded on the
    /// marker so the cell shows how long the compaction worked. The context
    /// derivation ([`crate::context::context_messages`]) applies the
    /// compacted shape from the marker on. See `docs/compact.md`.
    pub fn finish_compact(&mut self, elapsed_secs: u64) -> Option<Compaction> {
        let summary = self.compact_buffer.take()?;
        self.streaming = None;
        self.status = None;
        let mut compaction = Compaction {
            // A model reply often ends with a trailing newline; the bridge
            // adds its own separators, so store the summary trimmed.
            summary: summary.trim().to_string(),
            timestamp: self.now_stamp(),
            before: self.compact_before,
            after: 0,
            auto: self.compact_auto,
            secs: elapsed_secs,
        };
        self.history
            .push(HistoryItem::Compaction(compaction.clone()));
        // The gauge drops to the compacted derivation's estimate (codex's
        // recompute_token_usage) — the marker records the shrink.
        self.refresh_context_used();
        compaction.after = self.context_used;
        if let Some(HistoryItem::Compaction(recorded)) = self.history.last_mut() {
            recorded.after = self.context_used;
        }
        // One attempt per user turn: no immediate auto re-trigger.
        self.auto_compact_blocked = true;
        Some(compaction)
    }

    /// Inject the active model's context window in tokens (the boundary's
    /// `/v1/models` `context_length`, the saved settings, or the
    /// `ALTER_ZERO_CONTEXT_WINDOW` override) — `None` (or a meaningless 0)
    /// hides the footer gauge and disables auto-compact. See `docs/compact.md`.
    pub fn set_context_window(&mut self, window: Option<u64>) {
        self.context_window = window.filter(|&w| w > 0);
    }

    /// The active model's context window in tokens, when known.
    #[must_use]
    pub const fn context_window(&self) -> Option<u64> {
        self.context_window
    }

    /// The current context size in tokens (see the field doc) — the footer
    /// gauge's numerator.
    #[must_use]
    pub const fn context_used(&self) -> u64 {
        self.context_used
    }

    /// Estimate the current context size with the tokenizer: the system
    /// prompt, plus every derived context message's text (the AGENTS.md
    /// instructions among them — they ride every request), tool calls, and a
    /// flat [`IMAGE_INPUT_TOKENS`] per attachment. The stand-in between real
    /// usage frames — and the only measure right after a history mutation
    /// (compaction, `/clear`, backtrack, `/resume`) made the last frame stale.
    ///
    /// **An empty conversation estimates zero.** The system prompt and the
    /// standing instructions do ride the next request, but a *booted* session
    /// carries those same constants and the gauge reads 0 there (nothing
    /// seeds it until a turn), so counting them after a `/clear` showed the
    /// same empty state as two different numbers — and the leftover-looking
    /// one (`139/1M` under a blank screen) read as conversation that hadn't
    /// really gone. The gauge measures the conversation; with none, it is 0,
    /// and `/clear` lands exactly where a fresh session starts. The predicate
    /// is the one `/compact` uses for `Nothing to compact`, so the two never
    /// disagree about whether anything is there.
    #[must_use]
    fn estimate_context_tokens(&self) -> u64 {
        if !crate::context::derives_conversation(&self.history) {
            return 0;
        }
        let total = self.system_prompt.as_deref().map_or(0, count_tokens)
            + estimate_messages_tokens(&crate::context::context_messages_full(
                self.user_instructions.as_deref(),
                self.system_reminder.as_deref(),
                &self.history,
            ));
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    /// Re-seat the gauge on the tokenizer estimate (the last usage frame no
    /// longer describes the derived context).
    pub(super) fn refresh_context_used(&mut self) {
        self.context_used = self.estimate_context_tokens();
    }

    /// Whether the loop should start an **auto-compact** turn at the next idle
    /// boundary: the `/settings` **Auto compact** knob is on, the window is
    /// known, the gauge is past codex's 90% threshold, no turn is in flight,
    /// the last compact attempt isn't still blocking (one per user turn), and
    /// the conversation derives a non-empty context to summarize. See
    /// `docs/compact.md` / `docs/settings.md`.
    #[must_use]
    pub fn should_auto_compact(&self) -> bool {
        // The `/settings` knob (docs/settings.md): off means the loop never
        // starts one on its own — `/compact` by hand still works.
        if !self.settings.auto_compact {
            return false;
        }
        let Some(window) = self.context_window else {
            return false;
        };
        if self.auto_compact_blocked || self.turn_active() || self.is_streaming() {
            return false;
        }
        let threshold = window.saturating_mul(AUTO_COMPACT_NUMERATOR) / AUTO_COMPACT_DENOMINATOR;
        if self.context_used <= threshold {
            return false;
        }
        crate::context::derives_conversation(&self.history)
    }
}

/// The tokenizer estimate of a derived context: every message's text, a flat
/// `IMAGE_INPUT_TOKENS` per attachment, and each tool call's name and
/// arguments. The one counting rule behind both context gauges — the main
/// session's (`App::estimate_context_tokens`, which adds the system prompt
/// and hands in the fragments-led derivation) and a subagent's
/// ([`crate::agents::AgentRun`], over its own transcript) — so the two can
/// never disagree about what a message costs (`docs/agent-context-gauge.md`).
#[must_use]
pub(crate) fn estimate_messages_tokens(messages: &[crate::context::ContextMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            count_tokens(&message.text)
                + message.images.len() * IMAGE_INPUT_TOKENS
                + message
                    .tool_calls
                    .iter()
                    .map(|call| count_tokens(&call.name) + count_tokens(&call.arguments))
                    .sum::<usize>()
        })
        .sum()
}

/// A `/compact` marker's payload: the model-written handoff summary the
/// context derivation bridges into every later request, codex's local
/// compaction (`docs/compact.md`). Renders as the `● Context compacted` cell
/// (the summary body expands in the Ctrl+O transcript only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    /// The compact turn's full streamed reply — the handoff summary (may be
    /// empty when the model streamed nothing; the derivation substitutes
    /// codex's "(no summary available)").
    pub summary: String,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
    /// The context gauge when the compaction began (tokens) — the cell shows
    /// `· {before} → {after} tokens`. 0 = unknown (an old rollout), hiding
    /// the clause.
    pub before: u64,
    /// The re-estimated context size right after the compaction (tokens).
    pub after: u64,
    /// Whether this compaction was **auto-triggered** by the context gauge
    /// crossing the threshold (the cell appends `· auto`), vs the manual
    /// `/compact` command. See `docs/compact.md`.
    pub auto: bool,
    /// How long the summarization turn ran, in whole seconds — the cell's
    /// ` · {elapsed}` clause ([`format_elapsed`]-humanized). 0 = unknown (an
    /// old rollout), hiding the clause.
    pub secs: u64,
}

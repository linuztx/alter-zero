//! The thinking stream: the live reasoning buffer, the [`Reasoning`] cell it
//! settles into, and the provider's reasoning-token snap.
//! See `docs/thinking-stream.md`.
//!
//! The chain-of-thought a reasoning model streams used to be counted and
//! thrown away. It is now kept while the phase runs — previewed in the strip,
//! never committed, because scrollback cannot be taken back — and collapsed at
//! the phase's end into one `Thought for {elapsed} · {n} tokens` line whose
//! full text expands in the Ctrl+O transcript.
//!
//! The feature's on/off gate lives entirely at the I/O boundary
//! (`ALTER_ZERO_SHOW_THINKING`): with it off nothing ever calls
//! [`App::begin_reasoning`], so no buffer opens, [`App::push_thinking`] only
//! counts (exactly as before), and no cell is ever recorded.

use super::*;

/// One settled thinking phase: the model's whole chain-of-thought for it, how
/// long it ran, and what it cost.
///
/// Recorded as [`HistoryItem::Reasoning`] the moment the phase ends — before
/// the round's reply text or tool calls exist — so it slots ahead of them in
/// history and in scrollback alike. Rendered collapsed inline
/// ([`crate::ui::reasoning_lines`]) and expanded in the Ctrl+O transcript, the
/// same contract a tool call has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reasoning {
    /// The whole streamed chain-of-thought. Shown only in the live block while
    /// it streams and in the Ctrl+O transcript afterwards — never inline, and
    /// never replayed to the model ([`crate::context::context_messages`] skips
    /// it).
    pub text: String,
    /// How long the phase ran, in whole seconds (the boundary's thinking
    /// clock) — the cell's `Thought for {…}` clause, humanized by
    /// [`format_elapsed`].
    pub secs: u64,
    /// What the phase cost: the token estimate at first, **snapped** to
    /// the provider's `completion_tokens_details.reasoning_tokens` when the
    /// round's usage frame arrives ([`App::apply_usage`]). 0 hides the clause.
    pub tokens: usize,
    /// Wall-clock stamp (recorded like every item's; never displayed).
    pub timestamp: String,
}

/// An open thinking phase's chain-of-thought as it streams: the text, which
/// only ever grows ([`push`](LiveThought::push)), and the live block's tail
/// kept up beside it ([`tail_rows`](LiveThought::tail_rows)).
///
/// The block shows the thought's last few wrapped rows, and it is built
/// several times a frame. Wrapping the newest paragraph whole each time made
/// a frame cost all of it — a model that thinks in one long paragraph, which
/// many do, slowed every frame and every typed key's echo the longer it
/// thought (`docs/thinking-stream.md` *A long thought costs what it adds*).
/// So the rows a later delta cannot change are kept, and a frame wraps only
/// what arrived since, from the first row it may still rewrite.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveThought {
    text: String,
    tail: ThoughtTail,
}

impl From<&str> for LiveThought {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            tail: ThoughtTail::default(),
        }
    }
}

impl LiveThought {
    /// The chain-of-thought so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Append a streamed delta.
    pub fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    /// The whole text, the phase over.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }

    /// The last `keep` display rows of the thought, oldest first: every line
    /// that is not blank, each wrapped by `wrap`, in order — the rows the
    /// live block shows. `wrap` breaks a line, from one of its row starts,
    /// into row ranges, and says how many leading rows nothing appended to
    /// the line can change; the renderer's own wrapper answers both
    /// (`ui::wrap`'s `output_tail`), and it must keep to its `width`, which
    /// the kept rows are tied to.
    ///
    /// Brought up to date on the way out: a line that closed since the last
    /// call is wrapped from where its kept rows end, the line still arriving
    /// from its first row that may change, and nothing before — so a call
    /// costs what arrived since the last one, plus a row.
    pub fn tail_rows(
        &self,
        keep: usize,
        width: u16,
        wrap: impl Fn(&str) -> (Vec<std::ops::Range<usize>>, usize),
    ) -> Vec<String> {
        let text = self.text.as_str();
        let mut tail = self.tail.0.borrow_mut();
        if tail.width != width || tail.keep != keep || tail.searched > text.len() {
            *tail = TailState {
                width,
                keep,
                ..TailState::default()
            };
        }
        // Every line that has closed since: its rows are final now.
        let mut from = tail.line.max(tail.searched);
        while let Some(newline) = text[from..].find('\n') {
            let end = from + newline;
            if !text[tail.line..end].trim().is_empty() {
                let anchor = tail.anchor;
                let (rows, _) = wrap(&text[anchor..end]);
                for row in rows {
                    tail.keep_row(&text[anchor + row.start..anchor + row.end]);
                }
            }
            tail.line = end + 1;
            tail.anchor = tail.line;
            from = tail.line;
        }
        tail.searched = text.len();
        // The line still arriving: the rows before the first one a later
        // delta may change are final too.
        let mut open = Vec::new();
        if !text[tail.line..].trim().is_empty() {
            let anchor = tail.anchor;
            let (rows, settled) = wrap(&text[anchor..]);
            for (i, row) in rows.iter().enumerate() {
                let row = &text[anchor + row.start..anchor + row.end];
                if i < settled {
                    tail.keep_row(row);
                } else {
                    open.push(row.to_string());
                }
            }
            if let Some(next) = rows.get(settled) {
                tail.anchor = anchor + next.start;
            }
        }
        let mut window: Vec<String> = tail.kept.iter().cloned().collect();
        window.extend(open);
        let skip = window.len().saturating_sub(keep);
        window.split_off(skip)
    }
}

/// [`LiveThought`]'s kept rows behind a `RefCell` — a render-path cache in
/// the textarea's wrap-cache pattern, so it is no part of the thought's
/// identity: thoughts with the same text are equal whatever has been
/// wrapped, and a clone starts cold.
#[derive(Default)]
struct ThoughtTail(std::cell::RefCell<TailState>);

/// The state inside [`ThoughtTail`].
#[derive(Default)]
struct TailState {
    /// The width the rows were wrapped at.
    width: u16,
    /// How many rows are kept.
    keep: usize,
    /// How far the text has been searched for line breaks.
    searched: usize,
    /// Where the line still arriving starts.
    line: usize,
    /// A row start in that line: every row before it is final and kept.
    anchor: usize,
    /// The last `keep` final rows, oldest first.
    kept: std::collections::VecDeque<String>,
}

impl TailState {
    /// Keep a final row, dropping the oldest past `keep`.
    fn keep_row(&mut self, row: &str) {
        self.kept.push_back(row.to_string());
        if self.kept.len() > self.keep {
            self.kept.pop_front();
        }
    }
}

impl Clone for ThoughtTail {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for ThoughtTail {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ThoughtTail {}

impl std::fmt::Debug for ThoughtTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ThoughtTail(..)")
    }
}

/// Split `total` over `weights` proportionally, the last part taking the
/// remainder so the parts sum to exactly `total`.
///
/// This is how one round's `reasoning_tokens` is shared out when the model
/// thought more than once in it. With a single weight — the overwhelmingly
/// common case — it is just `[total]`; with all-zero weights the whole lot
/// goes to the last part rather than vanishing.
fn distribute(total: usize, weights: &[usize]) -> Vec<usize> {
    let mut parts = vec![0usize; weights.len()];
    let Some((last, head)) = parts.split_last_mut() else {
        return parts;
    };
    let sum: usize = weights.iter().sum();
    let mut given = 0usize;
    if sum > 0 {
        for (part, weight) in head.iter_mut().zip(weights) {
            // `total * weight` can be large but both are token counts — a
            // saturating product keeps a pathological report from wrapping.
            *part = total.saturating_mul(*weight) / sum;
            given += *part;
        }
    }
    *last = total.saturating_sub(given);
    parts
}

impl App {
    /// Open a thinking phase ([`crate::stream::StreamEvent::ThinkingStart`]),
    /// so the strip previews the reasoning as it streams and the phase settles
    /// into a cell at its end.
    ///
    /// A no-op during a `/compact` turn: that turn is invisible by design
    /// (`docs/compact.md`), and its summarizer's thinking is not conversation.
    pub fn begin_reasoning(&mut self) {
        if self.compact_buffer.is_some() {
            return;
        }
        self.reasoning = Some(LiveThought::default());
    }

    /// The chain-of-thought streamed so far in the open phase, or `None` when
    /// none is open — what the strip's live block previews. `Some("")` right
    /// after [`begin_reasoning`](App::begin_reasoning): the phase is real
    /// before its first delta, and the block should say so.
    #[must_use]
    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_ref().map(LiveThought::text)
    }

    /// The open phase's thought with its live tail — what the strip's block
    /// is built from ([`LiveThought::tail_rows`]); `None` when no phase is
    /// open.
    #[must_use]
    pub fn live_thought(&self) -> Option<&LiveThought> {
        self.reasoning.as_ref()
    }

    /// Close the open thinking phase, recording it as a [`Reasoning`] history
    /// item and returning it for the boundary to commit as the collapsed
    /// `Thought for …` cell. `secs` is the phase's wall-clock from the
    /// boundary's thinking clock (time never reaches the pure core except
    /// already computed — the [`set_status_times`](App::set_status_times)
    /// pattern).
    ///
    /// Returns `None` — recording nothing — when no phase was open, or when
    /// the phase produced no text: some providers open and close one without a
    /// single delta, and a `Thought for 0s` cell for that is noise.
    pub fn finish_reasoning(&mut self, secs: u64) -> Option<Reasoning> {
        let text = self.reasoning.take()?.into_text();
        if text.trim().is_empty() {
            return None;
        }
        let reasoning = Reasoning {
            tokens: count_tokens(&text),
            text,
            secs,
            timestamp: self.now_stamp(),
        };
        // Remember where it landed so this round's usage frame can snap its
        // estimate to the provider's own count.
        self.round_reasoning.push(self.history.len());
        self.history.push(HistoryItem::Reasoning(reasoning.clone()));
        Some(reasoning)
    }

    /// Drop an open thinking phase without recording it — the `/clear` wipe
    /// and the fresh-turn reset. The rounds' snap targets go with it.
    pub(super) fn drop_reasoning(&mut self) {
        self.reasoning = None;
        self.round_reasoning.clear();
    }

    /// Replace this round's reasoning cells' token estimates with the
    /// provider's own `completion_tokens_details.reasoning_tokens`, split over
    /// them by weight ([`distribute`]). Called from
    /// [`apply_usage`](App::apply_usage) — one usage frame ends one round, so
    /// the targets are cleared after.
    ///
    /// Defensive about its indices: a history mutation between the recording
    /// and the frame (a `/clear`, a backtrack) simply leaves nothing to snap.
    pub(super) fn snap_round_reasoning(&mut self, reasoning_tokens: u64) {
        let targets = std::mem::take(&mut self.round_reasoning);
        snap_reasoning_tokens(&mut self.history, &targets, reasoning_tokens);
    }
}

/// Replace the token estimates of the [`HistoryItem::Reasoning`] cells at
/// `targets` with the provider's own `reasoning_tokens`, split over them by
/// weight ([`distribute`]).
///
/// The pure core of [`App::snap_round_reasoning`], shared with
/// [`crate::agents::AgentRun`]'s own usage arm — a subagent's thinking cells
/// live on *its* transcript and need the identical snap
/// (`docs/agent-view-streaming.md`), and one round's accounting rule copied
/// into two places is one that drifts.
///
/// Defensive about its indices: a history mutation between the recording and
/// the frame (a `/clear`, a backtrack) simply leaves nothing to snap.
pub(crate) fn snap_reasoning_tokens(
    history: &mut [HistoryItem],
    targets: &[usize],
    reasoning_tokens: u64,
) {
    if reasoning_tokens == 0 || targets.is_empty() {
        return;
    }
    let weights: Vec<usize> = targets
        .iter()
        .map(|&i| match history.get(i) {
            Some(HistoryItem::Reasoning(r)) => r.tokens,
            _ => 0,
        })
        .collect();
    let parts = distribute(
        usize::try_from(reasoning_tokens).unwrap_or(usize::MAX),
        &weights,
    );
    for (&i, tokens) in targets.iter().zip(parts) {
        if let Some(HistoryItem::Reasoning(r)) = history.get_mut(i) {
            r.tokens = tokens;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_phase_takes_the_whole_round() {
        assert_eq!(distribute(169, &[12]), [169]);
    }

    #[test]
    fn two_phases_split_by_weight_and_sum_exactly() {
        assert_eq!(distribute(40, &[10, 30]), [10, 30]);
        // A remainder that doesn't divide evenly lands on the last part, so
        // the parts still add up to the round's real count.
        let parts = distribute(10, &[1, 1, 1]);
        assert_eq!(parts.iter().sum::<usize>(), 10);
        assert_eq!(parts, [3, 3, 4]);
    }

    #[test]
    fn all_zero_weights_still_place_the_whole_count() {
        assert_eq!(distribute(7, &[0, 0]), [0, 7]);
    }

    #[test]
    fn no_phases_places_nothing() {
        assert!(distribute(7, &[]).is_empty());
    }
}

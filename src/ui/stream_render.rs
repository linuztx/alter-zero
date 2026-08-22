//! [`StreamRender`] — the incremental renderer that flushes completed lines of
//! a streaming reply into scrollback.
//!
//! It caches the rendered rows of already-complete source lines and re-renders
//! only the newly-arrived tail, so streaming a reply is O(reply), not O(reply²).
//! See `docs/markdown.md` and `docs/flicker.md`.

use super::assistant::{AssistantRenderer, empty_assistant_row, row_is_blank};
use super::theme::*;
use super::*;

/// The **incremental**, stateful renderer that commits an assistant reply to
/// scrollback as it streams — the boundary's replacement for the old
/// re-render-the-whole-reply `stable_commit`/`final_commit` pair (which cost
/// O(reply) *per chunk*, so a long code reply was O(reply²) and starved the
/// status animation — see `docs/markdown.md`).
///
/// It drives one `AssistantRenderer`, caching the rendered rows of every
/// **complete** source line (`frozen`) and advancing over only the newly-arrived
/// lines on each call — so [`commit`](Self::commit)/[`preview`](Self::preview)
/// cost O(new text), and streaming a whole reply is O(reply).
///
/// Prefix-stability (CLAUDE.md invariant 2) holds by construction: a completed
/// source line's rows are frozen (markdown fence + highlight carry are threaded
/// left-to-right, wrapping is prefix-stable), so a row committed to scrollback
/// never changes. The still-growing trailing line is withheld from
/// [`commit`](Self::commit) and only peeked for the [`preview`](Self::preview),
/// then flushed by [`finish`](Self::finish) when the reply ends.
///
/// A width change (a resize) invalidates the cached rows; the next call rebuilds
/// from scratch (the boundary also [`reset`](Self::reset)s and re-commits the
/// reply, matching the old `committed = 0` behaviour).
pub struct StreamRender {
    /// The wrapping width the cache was built at; a change triggers a rebuild.
    pub(super) width: u16,
    /// Renderer state (fence + highlight) entering the trailing partial line.
    renderer: AssistantRenderer,
    /// Rendered rows of every complete (newline-terminated) source line so far.
    frozen: Vec<Line<'static>>,
    /// Byte offset of the start of the trailing partial line (end of the last
    /// complete line consumed into `frozen`).
    consumed: usize,
    /// Rows already returned to scrollback — an index into `frozen ++ tail`.
    pub(super) committed: usize,
    /// Bytes of the buffer already scanned for `'\n'` with none found past
    /// `consumed` — so [`advance`](Self::advance) rescans only newly-arrived
    /// bytes instead of the whole trailing line on every chunk (a 100 KB+
    /// single-line reply made that rescan O(line) *per chunk*).
    nl_scanned: usize,
    /// The `(consumed, tail length)` the last full [`commit`](Self::commit)
    /// evaluation ran at — the state behind the huge-single-line amortizer
    /// (see [`commit`](Self::commit)).
    last_eval: (usize, usize),
    /// Memo of the last [`preview`](Self::preview): the [`PreviewKey`] it was
    /// computed for and its rows. The strip redraws every animation frame,
    /// most of which arrive with no new chunk — this makes those frames O(1)
    /// instead of re-rendering the trailing line (for a huge line, O(line) at
    /// ~30 fps starved the loop).
    preview_memo: Option<(PreviewKey, Vec<Line<'static>>)>,
}

/// What pins a memoized preview: `(consumed, tail length, committed, width,
/// max_rows)`. The buffer is append-only within a turn, so an unchanged
/// `(consumed, tail length)` pins the same text; `committed` rides along
/// because the open-table path renders exactly the uncommitted rows.
type PreviewKey = (usize, usize, usize, u16, usize);

/// The trailing-line length past which [`StreamRender::commit`] amortizes its
/// per-chunk evaluation (see there) — generous enough that every humanly
/// readable line evaluates on every chunk, small enough that a machine-dump
/// line can't turn commits quadratic.
const TAIL_EVAL_MIN: usize = 4096;

impl Default for StreamRender {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamRender {
    /// A fresh renderer (before any width is known). The first call adopts the
    /// caller's width.
    #[must_use]
    pub fn new() -> Self {
        Self {
            width: 0,
            renderer: AssistantRenderer::new(0, AI_BULLET, AI_COLOR),
            frozen: Vec::new(),
            consumed: 0,
            committed: 0,
            nl_scanned: 0,
            last_eval: (0, 0),
            preview_memo: None,
        }
    }

    /// Discard all cached state — used at every turn boundary (turn end, tool
    /// split, interrupt, `/clear`, resize) so the next reply starts clean.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Fold every source line that has become **complete** (is newline-terminated)
    /// since the last call into `frozen`, advancing the renderer. Cheap: only the
    /// lines past `consumed`. A width change rebuilds from scratch first.
    fn advance(&mut self, text: &str, width: u16) {
        if width != self.width {
            self.width = width;
            self.renderer = AssistantRenderer::new(width, AI_BULLET, AI_COLOR);
            self.frozen.clear();
            self.consumed = 0;
            self.committed = 0;
            self.nl_scanned = 0;
            self.last_eval = (0, 0);
            self.preview_memo = None;
        }
        // The last '\n' at or after `consumed` terminates the last complete line;
        // everything up to it is now frozen. `consumed` always lands right after a
        // '\n' (or 0), so the slice is on char boundaries. Only the bytes past
        // `nl_scanned` can hold a new one — everything before it was scanned on
        // an earlier call and found newline-free — so the scan is O(new bytes),
        // not O(trailing line), per call. The buffer is append-only within a
        // turn (a boundary `reset` starts the next one), so the clamp is pure
        // insurance against a shrunk misuse panicking the render.
        let from = self.nl_scanned.max(self.consumed).min(text.len());
        if let Some(rel) = text[from..].rfind('\n') {
            let end = from + rel;
            for line in text[self.consumed..end].split('\n') {
                self.frozen.extend(self.renderer.feed_line(line));
            }
            self.consumed = end + 1;
        }
        self.nl_scanned = text.len();
    }

    /// The rows of the still-growing trailing partial line, rendered without
    /// disturbing the renderer's state (a cheap clone peek — O(one line)). An
    /// **empty** trailing line inside an open table is NOT fed: doing so would
    /// close the table on the clone and pull its rendered block into `commit`'s
    /// stable range mid-stream. The block only becomes real when a non-table
    /// line completes, or at `finish` (docs/table-streaming.md).
    fn tail_rows(&self, text: &str) -> Vec<Line<'static>> {
        let tail_src = &text[self.consumed..];
        let mut clone = self.renderer.clone();
        if tail_src.is_empty() && clone.in_table() {
            return Vec::new();
        }
        clone.feed_line(tail_src)
    }

    /// Take rows `[committed, stable)` of the virtual `frozen ++ tail`
    /// concatenation, advancing `committed`. `committed` never regresses.
    fn take_rows(&mut self, tail: &[Line<'static>], stable: usize) -> Vec<Line<'static>> {
        let start = self.committed.min(stable);
        let frozen_len = self.frozen.len();
        let out = (start..stable)
            .map(|i| {
                if i < frozen_len {
                    self.frozen[i].clone()
                } else {
                    tail[i - frozen_len].clone()
                }
            })
            .collect();
        self.committed = stable.max(self.committed);
        out
    }

    /// The stable rows to append to scrollback for the current buffer `text` —
    /// every rendered row except the still-growing last one — since the previous
    /// call. Replaces `stable_commit`; O(text appended since the last call).
    #[must_use]
    pub fn commit(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        // A huge single source line (a minified dump — no newline ever
        // arrives) makes every step below O(trailing line): the withhold
        // predicates scan it and `tail_rows` re-renders it, per chunk — a
        // 130 KB one-line reply cost ~40 s of event-loop time. Amortize:
        // once the tail is past `TAIL_EVAL_MIN`, re-evaluate only when it has
        // grown by an eighth since the last full evaluation (or a newline
        // finally arrived — `consumed` moved). Geometric re-evaluation keeps
        // the total work O(line); between evaluations nothing new commits,
        // which only *delays* rows (the differential tests hold — committed
        // rows are still a stable prefix), and the strip's preview keeps
        // showing the frontier. `finish` never consults the gate, so the
        // reply always completes exactly.
        let tail_len = text.len() - self.consumed;
        if tail_len > TAIL_EVAL_MIN
            && self.consumed == self.last_eval.0
            && tail_len < self.last_eval.1 + (self.last_eval.1 / 8).max(TAIL_EVAL_MIN / 4)
        {
            return Vec::new();
        }
        self.last_eval = (self.consumed, tail_len);
        // Withhold the whole trailing line when its rows aren't final yet:
        //  - **inside a fenced block**: a code line's colour isn't settled until the
        //    whole line is seen (a call's `(`, a `//` comment, a closing `*/`), and
        //    a long code line wraps into several rows — committing an early row
        //    could recolour it once the lookahead char arrives; and
        //  - a **partial fence marker** (`` ` ``/`` `` ``): a third marker would
        //    flip it from prose to a one-row label, so its wrapped prose rows must
        //    not reach scrollback; and
        //  - a **partial thematic-break run** (`-`/`*`/`_`, 1–2 markers): a third
        //    marker would collapse its wrapped prose rows into a single `———`
        //    rule; and
        //  - a **bare `#` run** (1–6 hashes): its heading *level* — and so its
        //    style — isn't settled (another `#` deepens it, a 7th flips it to
        //    prose), so at a width narrower than the run its wrapped rows must
        //    not reach scrollback yet; and
        //  - an **open table** (`in_table`): the block is buffered and renders
        //    whole only when it closes (its widths need every row), so nothing
        //    of it exists to commit — this commits only the settled pre-table
        //    rows, and the strip previews the forming block. A trailing
        //    **header candidate** (`is_table_header_candidate` — a leading
        //    `|`) is withheld the same way before the renderer has consumed
        //    it (docs/table-streaming.md); and
        //  - a trailing line with an **open inline marker** (`has_open_inline` —
        //    an unclosed `**`/`*`/`~~`/`` ` ``/`[`): its closer could still restyle
        //    an already-wrapped row, so the whole line is withheld until it settles
        //    (inline emphasis is line-local, so a *complete* line is always final).
        // Otherwise it's settled prose — only its still-growing *last* row is held
        // back. `tail_rows` (an O(one line) render) is computed only in that case,
        // never in the withhold path where it would be discarded.
        let tail_src = &text[self.consumed..];
        if self.renderer.in_code()
            || self.renderer.in_table()
            || markdown::is_table_header_candidate(tail_src)
            || markdown::is_partial_fence(tail_src)
            || markdown::is_partial_thematic_break(tail_src)
            || markdown::is_partial_heading(tail_src)
            || markdown::is_partial_list_marker(tail_src)
            || markdown::has_open_inline(tail_src)
            // A URL still forming at the line's end (`…see ht`, `http://e.`):
            // autolink detection flips on retroactively — the next chars
            // restyle the whole word blue+underlined — so its wrapped rows
            // must not reach scrollback as plain prose (docs/links.md).
            || crate::links::has_forming_url(tail_src)
        {
            let stable = self.frozen.len();
            // Hold back a trailing blank run — inside a fence too. A prose
            // blank is a paragraph break before the in-progress line; a
            // fence's blank lines ARE content, but whether the message keeps
            // them depends on what hasn't streamed yet: followed by more code
            // they commit (no longer trailing), while a closing fence with
            // nothing after turns them into the message's trailing blanks,
            // which the batch render trims (`assistant_lines`) — committing
            // them early left blank rows in scrollback a repaint drops.
            // `finish` settles the reply-ends-here case exactly (kept only
            // when the fence is still open).
            let stable = self.without_trailing_blanks(&[], stable);
            self.take_rows(&[], stable)
        } else {
            let tail = self.tail_rows(text);
            let total = self.frozen.len() + tail.len();
            // Withhold the **last non-blank row** (and any trailing blanks): it is
            // the row the strip previews, so committing it the moment its line ends
            // with a newline would show it twice — once in scrollback, once in the
            // preview — until the next chunk (the slow-stream duplicate-line bug).
            // It commits on a later call once newer content supersedes it, or at
            // `finish`. During active streaming the last row is the still-growing
            // trailing line, so this matches the old "withhold the last row".
            let stable = self.stable_keeping_preview_row(&tail, total);
            self.take_rows(&tail, stable)
        }
    }

    /// The commit boundary that **keeps the last non-blank row for the preview**:
    /// the index of the last non-blank row of the virtual `frozen ++ tail` (never
    /// below `committed`). Committing `[committed, boundary)` flushes every row
    /// *above* the one the strip previews; that row and any trailing blanks stay
    /// withheld. This is [`without_trailing_blanks`] backed off one further row, so
    /// a completed line is never both in scrollback and the preview at once.
    fn stable_keeping_preview_row(&self, tail: &[Line<'static>], total: usize) -> usize {
        let frozen_len = self.frozen.len();
        let mut s = total;
        while s > self.committed {
            let row = if s - 1 < frozen_len {
                &self.frozen[s - 1]
            } else {
                &tail[s - 1 - frozen_len]
            };
            if row_is_blank(row) {
                s -= 1; // withhold trailing blanks
            } else {
                return s - 1; // withhold this last non-blank row too (it previews)
            }
        }
        self.committed
    }

    /// Back off `stable` over trailing blank rows of the virtual `frozen ++ tail`
    /// sequence (never below `committed`), so a message-trailing blank run is
    /// **withheld** rather than committed on top of the next item's spacer (the
    /// 3-newline bug). A blank followed by real content is no longer trailing on
    /// the next call, so it commits then. Gated by the caller to skip code.
    fn without_trailing_blanks(&self, tail: &[Line<'static>], stable: usize) -> usize {
        let frozen_len = self.frozen.len();
        let mut s = stable;
        while s > self.committed {
            let row = if s - 1 < frozen_len {
                &self.frozen[s - 1]
            } else {
                &tail[s - 1 - frozen_len]
            };
            if row_is_blank(row) {
                s -= 1;
            } else {
                break;
            }
        }
        s
    }

    /// The remaining rows once the reply is complete: the withheld last row plus
    /// the whole trailing partial line, now rendered as a final complete line.
    /// Replaces `final_commit`.
    #[must_use]
    pub fn finish(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        let mut tail = self.renderer.feed_line(&text[self.consumed..]);
        // Flush a table the reply ended on (its rows were buffered pending a close
        // that never came), matching `assistant_lines`'s trailing `flush`.
        tail.extend(self.renderer.flush());
        self.consumed = text.len();
        self.frozen.extend(tail);
        if self.frozen.is_empty() {
            // The whole reply rendered to zero rows (only a code fence) — commit
            // the bullet home once, matching `assistant_lines`.
            self.frozen.push(empty_assistant_row(
                &self.renderer.bullet,
                self.renderer.color,
            ));
        }
        // Trim trailing blank rows (a model's `…\n\n` before a tool call, or at
        // the reply's end) — the caller adds exactly one spacer — unless the
        // reply ended inside an open fence (blank lines there are content). The
        // bullet-home fallback above is never blank, so an empty reply still
        // commits its bullet. Matches `assistant_lines` so the two agree.
        let mut total = self.frozen.len();
        if !self.renderer.in_code() {
            while total > self.committed && row_is_blank(&self.frozen[total - 1]) {
                total -= 1;
            }
        }
        self.take_rows(&[], total)
    }

    /// The rows this render has already handed to scrollback for `text` — the
    /// first `committed` rows of the virtual `frozen ++ tail` concatenation —
    /// re-rendered for a repaint (the Ctrl+O overlay return; see
    /// [`repaint_tail`]). Advances the line cache over `text` first, so rows
    /// committed from a since-completed trailing line are found in `frozen`;
    /// `committed` itself is untouched, so a follow-up [`commit`](Self::commit)
    /// still emits exactly the not-yet-committed rows. After a width change
    /// there *are* no already-committed rows at the new width (the cache
    /// rebuilt), so this returns nothing and the follow-up commit re-emits the
    /// whole reply.
    #[must_use]
    pub fn committed_rows(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        let mut rows: Vec<Line<'static>> =
            self.frozen.iter().take(self.committed).cloned().collect();
        if self.committed > self.frozen.len() {
            let tail = self.tail_rows(text);
            rows.extend(tail.into_iter().take(self.committed - self.frozen.len()));
        }
        rows
    }

    /// The strip's streaming preview for the current buffer: **every rendered
    /// row [`commit`](Self::commit) has not handed to scrollback** — the
    /// uncommitted tail of the same virtual `frozen ++ tail` sequence the
    /// commit frontier indexes.
    ///
    /// Sharing that one frontier is what makes `committed ++ preview` the whole
    /// reply at every instant — the strip shows exactly what scrollback is
    /// still missing. Both halves of that were reachable when the preview
    /// chose a row of its own: `commit` withholds by **source line** (a fenced
    /// code line, whose colour isn't final until it ends; a line with an open
    /// `**`/`` ` ``/`[`; a forming table), so a withheld line wrapping to N
    /// rows previewed only its last and the other N-1 were simply absent from
    /// the screen until the line settled (28 rows of a wide `vec![…]` literal
    /// at width 40), while a withheld line rendering to **no** rows — a
    /// closing ``` — fell back to the last frozen row, which was already
    /// committed, drawing the block's last code line twice.
    ///
    /// A **forming table** needs no special case any more, only its flush: its
    /// rows are buffered on the clone rather than in `frozen`, and none of them
    /// has committed, so they are uncommitted tail like everything else — the
    /// grid still streams row-by-row with its columns re-fitting as wider cells
    /// arrive (docs/table-streaming.md).
    ///
    /// Capped to the **newest** `max_rows` rows so a tail taller than the strip
    /// (a big table, a very long withheld code line) tail-follows its frontier.
    /// O(new complete lines since the last call + the trailing line + the open
    /// table): the uncommitted range is bounded by what `commit` withholds — a
    /// paragraph break, one source line, the open table, or (on a line past
    /// `TAIL_EVAL_MIN`) the rows its amortized evaluation has not caught up
    /// to yet — so redrawing it every animation frame stays cheap. That last
    /// case is also *why* the amortizer is safe: the rows it defers stay on
    /// screen here instead of vanishing until the next evaluation
    /// (`the_commit_amortizer_never_hides_rows_from_the_screen`). Repeat calls
    /// with no new chunk — the common animation frame — are served from the
    /// memo.
    #[must_use]
    pub fn preview(&mut self, text: &str, width: u16, max_rows: usize) -> Vec<Line<'static>> {
        self.advance(text, width);
        // The strip redraws every animation frame (~30/s), most with no new
        // chunk: serve those from the memo instead of re-rendering the
        // trailing line each time (O(line) per frame starved the loop on a
        // huge single-line reply — and rendering the whole uncommitted tail
        // costs strictly more than the one row the old preview picked). The
        // buffer is append-only within a turn, so an unchanged `(consumed,
        // tail length)` pins the same text and `frozen`, and `committed`
        // pins where the uncommitted range starts; a resize or `reset` clears
        // the memo with the rest of the cache.
        let key = (
            self.consumed,
            text.len() - self.consumed,
            self.committed,
            width,
            max_rows,
        );
        if let Some((memo_key, rows)) = &self.preview_memo
            && *memo_key == key
        {
            return rows.clone();
        }
        let rows = self.preview_uncached(text, max_rows);
        self.preview_memo = Some((key, rows.clone()));
        rows
    }

    /// The un-memoized [`preview`](Self::preview) body — one full render of
    /// the frontier for the current buffer (the caller's
    /// [`advance`](Self::advance) has already run).
    fn preview_uncached(&mut self, text: &str, max_rows: usize) -> Vec<Line<'static>> {
        // Feed the trailing line on a clone (not disturbing the resumable state)
        // and keep the clone so its *post-tail* fence state decides trimming —
        // the same state `finish`/`assistant_lines` see once the whole prefix is
        // rendered (a trailing `` ``` `` opens a fence, so a blank before it is
        // kept, not trimmed).
        let mut clone = self.renderer.clone();
        // An empty trailing line (a chunk boundary that ended right after a
        // newline) is *not* fed: feeding it would close an open table on the
        // clone early — the flush below renders the buffered block either way
        // (`tail_rows` carries the same guard, so commit and preview agree on
        // the frontier).
        let tail_src = &text[self.consumed..];
        let mut tail = if tail_src.is_empty() && clone.in_table() {
            Vec::new()
        } else {
            clone.feed_line(tail_src)
        };
        // A table is open — entering the trailing line (`self.renderer`), or
        // opened/kept open by it (`clone`): flush the buffered block off the
        // clone so the forming grid renders whole. Nothing of it has committed,
        // so it all falls inside the uncommitted range below.
        if self.renderer.in_table() || clone.in_table() {
            tail.extend(clone.flush());
        }
        // The uncommitted rows of the virtual `frozen ++ tail`.
        let skip = self.committed.min(self.frozen.len());
        let mut rows: Vec<Line<'static>> = self.frozen[skip..].to_vec();
        rows.extend(
            tail.into_iter()
                .skip(self.committed.saturating_sub(self.frozen.len())),
        );
        // Trailing blank rows are a paragraph break `commit` withholds too —
        // the strip has no reason to show them. Inside a fence blank lines are
        // content, so keep them there.
        if !clone.in_code() {
            while rows.last().is_some_and(row_is_blank) {
                rows.pop();
            }
        }
        // The reply so far renders to zero rows and nothing has committed (only
        // a code fence, or only whitespace): batch `assistant_lines` still
        // emits the bullet home, so the preview must match it or the strip
        // would diverge from a repaint. With rows already committed an empty
        // tail means exactly that — everything is in scrollback and the strip
        // has nothing to add.
        if rows.is_empty() && self.committed == 0 && self.frozen.is_empty() {
            return vec![empty_assistant_row(
                &self.renderer.bullet,
                self.renderer.color,
            )];
        }
        // Tail-follow: keep the newest rows when the tail outgrows the cap (the
        // top of a big table scrolls out of the strip and reappears when the
        // closed block commits whole).
        if rows.len() > max_rows {
            rows.drain(..rows.len() - max_rows);
        }
        rows
    }
}

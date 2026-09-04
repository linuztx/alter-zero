//! A codex-style editable text input: a cursor you can move *anywhere*, with
//! insert/delete at the cursor and movement across wrapped visual rows.
//!
//! This is a focused port of the editing core of openai/codex's
//! `bottom_pane/textarea.rs` — the same four load-bearing fields (`text`,
//! `cursor`, a width-keyed `wrap_cache`, and a `preferred_col` for vertical
//! motion) — plus the readline set on top: word-wise motion and the kill-key
//! *targets* (`prev_word_boundary`/`prev_unix_word_boundary`/
//! `next_word_boundary`/`cursor_line_start`/`cursor_kill_end` — the composer
//! deletes the spans itself so a kill can stay placeholder-atomic,
//! `App::kill_span`). Deliberately *without* codex's vim mode, atomic
//! `@mention` elements, emacs kill-ring/yank, masking, highlights or
//! keymap-config. See `docs/textarea.md`.
//!
//! Everything here is pure (no terminal I/O), so it is unit-tested directly: the
//! cursor is a byte offset kept on a grapheme boundary, wrapping produces byte
//! **ranges** (so the cursor maps to a visual row/column and back), and vertical
//! motion reads the wrap cache the render path fills — which is what lets
//! `App::on_key` stay width-agnostic.

use std::cell::RefCell;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

/// Display width of `s` in terminal columns (CJK count as two, combining marks
/// as zero) — all width math goes through this, never `chars().count()`.
fn cols(s: &str) -> usize {
    s.width()
}

/// The editable composer buffer: raw text plus a movable cursor, a width-keyed
/// cache of wrapped row ranges, and the remembered column for vertical motion.
/// `Clone` snapshots the whole draft — text *and* cursor — which is how the
/// Ctrl+R history search restores it on cancel (codex's `ComposerDraft`
/// snapshot/restore; see `docs/history-search.md`).
#[derive(Debug, Default, Clone)]
pub struct TextArea {
    /// The raw UTF-8 draft.
    text: String,
    /// Cursor byte offset into [`text`]; always kept on a grapheme boundary.
    ///
    /// [`text`]: TextArea::text
    cursor: usize,
    /// Wrapped row ranges, recomputed only when the width changes (codex
    /// technique #5). Cleared on every edit; filled lazily by [`wrapped_rows`].
    ///
    /// [`wrapped_rows`]: TextArea::wrapped_rows
    wrap_cache: RefCell<Option<WrapCache>>,
    /// The column a vertical move *wants*, so stepping up/down through lines of
    /// differing length doesn't snap left. Cleared by any horizontal move or edit.
    preferred_col: Option<usize>,
}

#[derive(Debug, Clone)]
struct WrapCache {
    width: u16,
    rows: Vec<Range<usize>>,
}

impl TextArea {
    /// A fresh, empty editor with the cursor at the start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An editor pre-filled with `text`, cursor at the **end** (as if just typed).
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
            wrap_cache: RefCell::new(None),
            preferred_col: None,
        }
    }

    /// The current draft text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Is the draft empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The cursor's byte offset (mostly for tests/assertions).
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace the whole draft, putting the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.cursor = self.text.len();
        self.dirty();
    }

    /// Replace the whole draft, seating the cursor at byte `cursor` — clamped
    /// to the text's end and snapped back to a grapheme boundary if it lands
    /// inside a cluster. For rewrites that must not teleport the cursor to the
    /// end like [`set_text`] does (e.g. `App::sync_shell_mode` absorbing a
    /// leading `!`: the text shrinks by one byte, the cursor stays put).
    ///
    /// [`set_text`]: TextArea::set_text
    pub fn set_text_with_cursor(&mut self, text: &str, cursor: usize) {
        self.text.clear();
        self.text.push_str(text);
        let mut pos = cursor.min(self.text.len());
        while pos > 0 && !self.text.is_char_boundary(pos) {
            pos -= 1;
        }
        let mut gc = GraphemeCursor::new(pos, self.text.len(), true);
        if !matches!(gc.is_boundary(&self.text, 0), Ok(true)) {
            pos = prev_grapheme(&self.text, pos);
        }
        self.cursor = pos;
        self.dirty();
    }

    /// Empty the draft (e.g. after a slash command consumes the input).
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.dirty();
    }

    /// Take the draft out, resetting the editor to empty — used on submit.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.dirty();
        text
    }

    /// Mark derived state stale after a mutation: drop the wrap cache and the
    /// remembered vertical column.
    fn dirty(&mut self) {
        self.wrap_cache.replace(None);
        self.preferred_col = None;
    }

    // ===== editing (at the cursor) =====

    /// Insert `s` at the cursor, advancing the cursor past it.
    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.dirty();
    }

    /// Replace the byte `range` with `with`, leaving the cursor **after** the
    /// inserted text. Used to swap a typed `@token` for a chosen file path
    /// (`docs/file-search.md`); unlike [`insert_str`] (which inserts at the
    /// cursor), this targets an arbitrary span. `range` must fall on char
    /// boundaries (the caller passes [`crate::file_search::at_token`]'s range).
    ///
    /// [`insert_str`]: TextArea::insert_str
    pub fn replace_range(&mut self, range: Range<usize>, with: &str) {
        let start = range.start;
        self.text.replace_range(range, with);
        self.cursor = start + with.len();
        self.dirty();
    }

    /// Insert a single character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut buf));
    }

    /// Insert a newline at the cursor (Ctrl+J / Alt+Enter / Shift+Enter — grows
    /// the box; see `docs/shift-enter.md`).
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Delete the grapheme **before** the cursor (Backspace). No-op at the start.
    pub fn delete_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = prev_grapheme(&self.text, self.cursor);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        self.dirty();
    }

    /// Delete the grapheme **at** the cursor (Delete). No-op at the end.
    pub fn delete_forward(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_grapheme(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
        self.dirty();
    }

    // ===== horizontal movement =====

    /// Move left one grapheme.
    pub fn move_left(&mut self) {
        self.cursor = prev_grapheme(&self.text, self.cursor);
        self.preferred_col = None;
    }

    /// Move right one grapheme.
    pub fn move_right(&mut self) {
        self.cursor = next_grapheme(&self.text, self.cursor);
        self.preferred_col = None;
    }

    /// Move to the start of the current **logical** line (after the previous
    /// `'\n'`, or the very start). Matches codex's Home.
    pub fn move_home(&mut self) {
        self.cursor = self.line_start(self.cursor);
        self.preferred_col = None;
    }

    /// Move to the end of the current **logical** line (before the next `'\n'`,
    /// or the very end). Matches codex's End.
    pub fn move_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
        self.preferred_col = None;
    }

    /// Move to the start of the previous **word** ([`prev_word_boundary`]) —
    /// Alt+B / Ctrl+←.
    ///
    /// [`prev_word_boundary`]: TextArea::prev_word_boundary
    pub fn move_word_left(&mut self) {
        self.cursor = self.prev_word_boundary();
        self.preferred_col = None;
    }

    /// Move to the end of the next **word** ([`next_word_boundary`]) —
    /// Alt+F / Ctrl+→.
    ///
    /// [`next_word_boundary`]: TextArea::next_word_boundary
    pub fn move_word_right(&mut self) {
        self.cursor = self.next_word_boundary();
        self.preferred_col = None;
    }

    // ===== word/line boundary queries (the kill targets) =====
    //
    // The kill keys (Ctrl+W/U/K, Alt+D, Alt+Backspace) delete *spans*, and the
    // composer must widen a span over any pasted placeholder it would cut in
    // half (`App::kill_span` — docs/paste.md) before deleting, so the targets
    // are exposed as queries rather than performed here.

    /// Where a word-wise left motion or backward word kill reaches from the
    /// cursor: the start of the previous readline word — a run of
    /// alphanumerics, so `foo/bar.txt` is three words. Whitespace and
    /// punctuation before it are crossed. (Deliberately readline's alnum
    /// words, not Unicode word segmentation, whose `.`-joins-letters rule
    /// would make `bar.txt` a single stop — terminal muscle memory wins.)
    #[must_use]
    pub fn prev_word_boundary(&self) -> usize {
        let mut pos = self.cursor;
        while pos > 0 {
            let prev = prev_grapheme(&self.text, pos);
            if is_word_grapheme(&self.text[prev..pos]) {
                break;
            }
            pos = prev;
        }
        while pos > 0 {
            let prev = prev_grapheme(&self.text, pos);
            if !is_word_grapheme(&self.text[prev..pos]) {
                break;
            }
            pos = prev;
        }
        pos
    }

    /// Where a word-wise right motion or forward word kill reaches from the
    /// cursor: the end of the current-or-next readline word (Emacs'
    /// `forward-word` end), or the very end when only non-words remain.
    #[must_use]
    pub fn next_word_boundary(&self) -> usize {
        let mut pos = self.cursor;
        while pos < self.text.len() {
            let next = next_grapheme(&self.text, pos);
            if is_word_grapheme(&self.text[pos..next]) {
                break;
            }
            pos = next;
        }
        while pos < self.text.len() {
            let next = next_grapheme(&self.text, pos);
            if !is_word_grapheme(&self.text[pos..next]) {
                break;
            }
            pos = next;
        }
        pos
    }

    /// Where Ctrl+W's unix-word-rubout reaches from the cursor: back over
    /// whitespace (a newline included), then back over the non-whitespace run —
    /// the shell's coarser, whitespace-delimited word, so `bar.txt` goes whole
    /// where [`prev_word_boundary`] stops at the dot.
    ///
    /// [`prev_word_boundary`]: TextArea::prev_word_boundary
    #[must_use]
    pub fn prev_unix_word_boundary(&self) -> usize {
        let mut pos = self.cursor;
        while pos > 0 {
            let prev = prev_grapheme(&self.text, pos);
            if self.text[prev..pos].chars().all(char::is_whitespace) {
                pos = prev;
            } else {
                break;
            }
        }
        while pos > 0 {
            let prev = prev_grapheme(&self.text, pos);
            if self.text[prev..pos].chars().all(char::is_whitespace) {
                break;
            }
            pos = prev;
        }
        pos
    }

    /// The start of the cursor's logical line — Ctrl+U's kill target (what
    /// [`move_home`] moves to).
    ///
    /// [`move_home`]: TextArea::move_home
    #[must_use]
    pub fn cursor_line_start(&self) -> usize {
        self.line_start(self.cursor)
    }

    /// Ctrl+K's kill target: the end of the cursor's logical line — except
    /// *at* that end, where the kill takes the `'\n'` itself (Emacs' `kill-line`
    /// joins the lines rather than dying as a no-op). At the very end of the
    /// text there is nothing to take.
    #[must_use]
    pub fn cursor_kill_end(&self) -> usize {
        let end = self.line_end(self.cursor);
        if self.cursor == end && end < self.text.len() {
            end + 1 // the '\n' after the line
        } else {
            end
        }
    }

    // ===== vertical movement (across wrapped rows; logical-line fallback) =====

    /// Move up one **visual** row (using the wrap cache the render path filled),
    /// preserving the preferred column. Falls back to logical-line motion when the
    /// cache is cold; from the first row, snaps to the very start.
    pub fn move_up(&mut self) {
        if let Some(rows) = self.cached_rows() {
            let row = row_of(&rows, self.cursor);
            let target = self.target_col(rows[row].start);
            if row == 0 {
                self.cursor = 0;
                self.preferred_col = None;
                return;
            }
            self.preferred_col.get_or_insert(target);
            let prev = &rows[row - 1];
            let pos = self.pos_at_col(prev.start, prev.end, target);
            // Byte-abutting rows (a hard-broken word's chunks) share their
            // boundary byte with this row's start; clamping to `prev.end`
            // would leave the cursor exactly where it was — and with
            // `preferred_col` now pinned, every further ↑ would repeat the
            // no-op. Step strictly into the previous row.
            self.cursor = if pos == self.cursor {
                prev_grapheme(&self.text, pos)
            } else {
                pos
            };
            return;
        }
        // Cache cold: logical-line fallback.
        if let Some(prev_nl) = self.text[..self.cursor].rfind('\n') {
            let target = self.target_col(self.line_start(self.cursor));
            self.preferred_col.get_or_insert(target);
            let start = self.text[..prev_nl].rfind('\n').map_or(0, |i| i + 1);
            self.cursor = self.pos_at_col(start, prev_nl, target);
        } else {
            self.cursor = 0;
            self.preferred_col = None;
        }
    }

    /// Move down one **visual** row (see [`move_up`]); from the last row, snaps to
    /// the very end.
    ///
    /// [`move_up`]: TextArea::move_up
    pub fn move_down(&mut self) {
        if let Some(rows) = self.cached_rows() {
            let row = row_of(&rows, self.cursor);
            let target = self.target_col(rows[row].start);
            if row + 1 >= rows.len() {
                self.cursor = self.text.len();
                self.preferred_col = None;
                return;
            }
            self.preferred_col.get_or_insert(target);
            let next = &rows[row + 1];
            self.cursor = self.pos_at_col(next.start, next.end, target);
            return;
        }
        // Cache cold: logical-line fallback.
        if let Some(next_nl) = self.text[self.cursor..].find('\n').map(|i| i + self.cursor) {
            let target = self.target_col(self.line_start(self.cursor));
            self.preferred_col.get_or_insert(target);
            let start = next_nl + 1;
            let end = self.text[start..]
                .find('\n')
                .map_or(self.text.len(), |i| i + start);
            self.cursor = self.pos_at_col(start, end, target);
        } else {
            self.cursor = self.text.len();
            self.preferred_col = None;
        }
    }

    /// The column a vertical move should aim for from a row starting at `line_start`:
    /// the remembered [`preferred_col`] if set, else the cursor's current column.
    ///
    /// [`preferred_col`]: TextArea::preferred_col
    fn target_col(&self, line_start: usize) -> usize {
        self.preferred_col
            .unwrap_or_else(|| cols(&self.text[line_start..self.cursor]))
    }

    /// The byte position on `[start, end)` whose display column first exceeds
    /// `target_col`, or `end` if the row is shorter than that.
    fn pos_at_col(&self, start: usize, end: usize, target_col: usize) -> usize {
        let mut w = 0;
        for (gidx, g) in self.text[start..end].grapheme_indices(true) {
            let gw = cols(g);
            if w + gw > target_col {
                return start + gidx;
            }
            w += gw;
        }
        end
    }

    fn line_start(&self, pos: usize) -> usize {
        self.text[..pos].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, pos: usize) -> usize {
        self.text[pos..]
            .find('\n')
            .map_or(self.text.len(), |i| i + pos)
    }

    // ===== wrapping / rendering geometry (width-parameterised) =====

    /// The wrapped row ranges at `width`, recomputing the cache only when the
    /// width changes. Each range is the displayed byte slice of one visual row.
    #[must_use]
    pub fn wrapped_rows(&self, width: u16) -> Vec<Range<usize>> {
        {
            let mut cache = self.wrap_cache.borrow_mut();
            let stale = cache.as_ref().is_none_or(|c| c.width != width);
            if stale {
                *cache = Some(WrapCache {
                    width,
                    rows: wrap_rows(&self.text, width),
                });
            }
        }
        self.wrap_cache.borrow().as_ref().unwrap().rows.clone()
    }

    /// The cached wrapped rows *without* recomputing — `None` when the cache is
    /// cold (between an edit and the next render). Vertical motion uses this so it
    /// never needs to know the terminal width.
    fn cached_rows(&self) -> Option<Vec<Range<usize>>> {
        self.wrap_cache.borrow().as_ref().map(|c| c.rows.clone())
    }

    /// The visible text of each wrapped row at `width` (for rendering).
    #[must_use]
    pub fn display_rows(&self, width: u16) -> Vec<String> {
        self.wrapped_rows(width)
            .iter()
            .map(|r| self.text[r.clone()].to_string())
            .collect()
    }

    /// How many visual rows the input occupies at `width` (always at least one).
    #[must_use]
    pub fn row_count(&self, width: u16) -> usize {
        self.wrapped_rows(width).len().max(1)
    }

    /// The cursor's `(visual row, display column)` at `width`. The row is the last
    /// wrapped row starting at or before the cursor (so a cursor at a wrap point
    /// shows at the next row's start), and the column is the display width up
    /// to it — `width` itself at the end of a row the wrap left exactly full.
    /// That column is the cell every caller keeps past its text width for the
    /// caret (`ui::layout::text_field_width`, `docs/textarea.md`), so the
    /// cursor always has a cell on its own row and never needs an empty row
    /// below it.
    #[must_use]
    pub fn cursor_row_col(&self, width: u16) -> (usize, usize) {
        let rows = self.wrapped_rows(width);
        let row = row_of(&rows, self.cursor);
        let r = &rows[row];
        let end = self.cursor.min(r.end);
        (row, cols(&self.text[r.start..end]))
    }
}

/// The index of the last row whose `start <= pos` (0 if none / empty).
fn row_of(rows: &[Range<usize>], pos: usize) -> usize {
    rows.iter().rposition(|r| r.start <= pos).unwrap_or(0)
}

/// Is this grapheme part of a readline word (contains an alphanumeric) rather
/// than whitespace/punctuation? The word class the Alt/Ctrl word motions and
/// word kills jump between.
fn is_word_grapheme(g: &str) -> bool {
    g.chars().any(char::is_alphanumeric)
}

/// The grapheme boundary strictly before `pos` (or 0).
fn prev_grapheme(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut gc = GraphemeCursor::new(pos, text.len(), true);
    match gc.prev_boundary(text, 0) {
        Ok(Some(b)) => b,
        _ => pos.saturating_sub(1),
    }
}

/// The grapheme boundary strictly after `pos` (or `text.len()`).
fn next_grapheme(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut gc = GraphemeCursor::new(pos, text.len(), true);
    match gc.next_boundary(text, 0) {
        Ok(Some(b)) => b,
        _ => (pos + 1).min(text.len()),
    }
}

/// Greedy word-wrap `text` to `width` columns, returning the **byte range** of the
/// displayed slice of each visual row. Honours `'\n'` (a blank logical line is an
/// empty range), preserves the user's exact characters (spaces included), consumes
/// the run of spaces at a soft break, and hard-breaks an over-long word — or a
/// leading/trailing space run — on grapheme boundaries, so no row grows wider
/// than `width` (short of a single grapheme wider than the whole field).
/// `width == 0` disables wrapping (split on `'\n'` only).
///
/// Always returns at least one range (the empty draft is one empty row). A row
/// exactly `width` wide gets **no** empty row after it: the cursor at its end
/// sits at column `width`, the cell every caller keeps past its text width for
/// the caret (`ui::layout::text_field_width`) — which is also why a word that
/// would land in the field's last column wraps to the next row instead,
/// Claude Code's rule. codex's `wrap_ranges` seats that cursor on an empty
/// sentinel row instead, and so did this one: the text flush against the
/// terminal's edge with the caret alone on the row below read as a newline
/// the user never typed (`docs/textarea.md`).
fn wrap_rows(text: &str, width: u16) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    loop {
        let rel_nl = text[line_start..].find('\n');
        let line_end = rel_nl.map_or(text.len(), |i| line_start + i);
        wrap_logical_line(text, line_start..line_end, width, &mut rows);
        match rel_nl {
            Some(i) => line_start += i + 1, // step past the '\n'
            None => break,
        }
    }
    rows
}

/// Wrap one newline-free logical line `seg`, appending its visual rows to `rows`.
fn wrap_logical_line(text: &str, seg: Range<usize>, width: u16, rows: &mut Vec<Range<usize>>) {
    if width == 0 || seg.start == seg.end {
        rows.push(seg.start..seg.end);
        return;
    }
    let width = width as usize;
    let mut row_start = seg.start;
    let mut cur_end = seg.start; // display end of the current row
    let mut col = 0usize;
    let mut at_head = true; // nothing placed on the current row yet
    let mut first = true; // first token of the whole logical line
    let mut pending: Option<Range<usize>> = None; // an inter-word space run

    for (range, is_space) in tokenize(text, seg.clone()) {
        if is_space {
            if at_head && first {
                // Leading indentation on the first row — shown, wrapping like a
                // word (spaces are 1-col graphemes) so it never overflows a row.
                place_word(
                    text,
                    range,
                    width,
                    &mut row_start,
                    &mut cur_end,
                    &mut col,
                    &mut at_head,
                    rows,
                );
            } else if at_head {
                // A break-space at the head of a wrapped row — consume (don't show).
                row_start = range.end;
                cur_end = range.end;
            } else {
                pending = Some(range);
            }
            first = false;
            continue;
        }
        first = false;
        let tw = cols(&text[range.clone()]);
        let sp_w = pending.as_ref().map_or(0, |r| cols(&text[r.clone()]));
        if !at_head && col + sp_w + tw > width {
            // Break before this word; the pending space is consumed at the break.
            rows.push(row_start..cur_end);
            row_start = range.start;
            cur_end = range.start;
            col = 0;
            at_head = true;
            pending = None;
        } else if let Some(sp) = pending.take() {
            // The space fits — place it before the word.
            cur_end = sp.end;
            col += sp_w;
        }
        place_word(
            text,
            range,
            width,
            &mut row_start,
            &mut cur_end,
            &mut col,
            &mut at_head,
            rows,
        );
    }
    if let Some(sp) = pending.take() {
        // Trailing spaces: show them (so the cursor after a trailing space is
        // visible), wrapped like a word so the cursor column stays in the field.
        place_word(
            text,
            sp,
            width,
            &mut row_start,
            &mut cur_end,
            &mut col,
            &mut at_head,
            rows,
        );
    }
    rows.push(row_start..cur_end);
}

/// Place a word run on the current row, hard-breaking it across rows (on grapheme
/// boundaries) when it is wider than `width`.
#[allow(clippy::too_many_arguments)]
fn place_word(
    text: &str,
    range: Range<usize>,
    width: usize,
    row_start: &mut usize,
    cur_end: &mut usize,
    col: &mut usize,
    at_head: &mut bool,
    rows: &mut Vec<Range<usize>>,
) {
    let word_w = cols(&text[range.clone()]);
    if *col + word_w <= width {
        *cur_end = range.end;
        *col += word_w;
        *at_head = false;
        return;
    }
    // Too wide to fit even on a fresh row — hard-break by graphemes.
    for (gidx, g) in text[range.clone()].grapheme_indices(true) {
        let gpos = range.start + gidx;
        let gw = cols(g);
        if !*at_head && *col + gw > width {
            rows.push(*row_start..*cur_end);
            *row_start = gpos;
            *cur_end = gpos;
            *col = 0;
            *at_head = true;
        }
        *cur_end = gpos + g.len();
        *col += gw;
        *at_head = false;
    }
}

/// Split `seg` into maximal runs of ASCII spaces / non-spaces, as `(range, is_space)`.
fn tokenize(text: &str, seg: Range<usize>) -> Vec<(Range<usize>, bool)> {
    let mut toks = Vec::new();
    let s = &text[seg.clone()];
    let mut idx = 0;
    while idx < s.len() {
        let is_space = s.as_bytes()[idx] == b' ';
        let run = if is_space {
            s[idx..].bytes().take_while(|b| *b == b' ').count()
        } else {
            s[idx..].find(' ').unwrap_or(s.len() - idx)
        };
        toks.push((seg.start + idx..seg.start + idx + run, is_space));
        idx += run;
    }
    toks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The displayed text of each wrapped row at `width`.
    fn rows(ta: &TextArea, width: u16) -> Vec<String> {
        ta.display_rows(width)
    }

    /// An editor with `text` and the cursor explicitly at byte `cursor`.
    fn at(text: &str, cursor: usize) -> TextArea {
        let mut ta = TextArea::from_text(text);
        ta.cursor = cursor;
        ta
    }

    // ===== editing at the cursor =====

    #[test]
    fn insert_char_inserts_at_the_cursor_not_the_end() {
        let mut ta = at("ac", 1); // cursor between a and c
        ta.insert_char('b');
        assert_eq!(ta.text(), "abc");
        assert_eq!(ta.cursor(), 2, "cursor advances past the inserted char");
    }

    #[test]
    fn set_text_with_cursor_seats_the_cursor_where_asked() {
        let mut ta = TextArea::from_text("old");
        ta.set_text_with_cursor("hello", 2);
        assert_eq!(ta.text(), "hello");
        assert_eq!(ta.cursor(), 2);
    }

    #[test]
    fn set_text_with_cursor_clamps_past_the_end() {
        let mut ta = TextArea::new();
        ta.set_text_with_cursor("hi", 99);
        assert_eq!(ta.cursor(), 2);
    }

    #[test]
    fn set_text_with_cursor_snaps_inside_a_grapheme_to_its_start() {
        // Byte 1 is inside the 4-byte emoji — the cursor must land on a
        // grapheme boundary, so it snaps back to the cluster's start.
        let mut ta = TextArea::new();
        ta.set_text_with_cursor("🦀ab", 1);
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn insert_str_advances_the_cursor() {
        let mut ta = at("()", 1);
        ta.insert_str("xy");
        assert_eq!(ta.text(), "(xy)");
        assert_eq!(ta.cursor(), 3);
    }

    #[test]
    fn replace_range_splices_and_seats_the_cursor_after() {
        let mut ta = at("see @al here", 7); // cursor just after "@al"
        ta.replace_range(4..7, "alpha.txt"); // swap the "@al" token for the path
        assert_eq!(ta.text(), "see alpha.txt here");
        assert_eq!(ta.cursor(), 4 + "alpha.txt".len());
    }

    #[test]
    fn backspace_deletes_the_grapheme_before_the_cursor() {
        let mut ta = at("abc", 2); // between b and c
        ta.delete_backward();
        assert_eq!(ta.text(), "ac");
        assert_eq!(ta.cursor(), 1);
    }

    #[test]
    fn backspace_at_the_start_is_a_no_op() {
        let mut ta = at("abc", 0);
        ta.delete_backward();
        assert_eq!(ta.text(), "abc");
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn delete_forward_removes_the_grapheme_at_the_cursor() {
        let mut ta = at("abc", 1); // between a and b
        ta.delete_forward();
        assert_eq!(ta.text(), "ac");
        assert_eq!(ta.cursor(), 1, "cursor stays put on a forward delete");
    }

    #[test]
    fn delete_forward_at_the_end_is_a_no_op() {
        let mut ta = at("abc", 3);
        ta.delete_forward();
        assert_eq!(ta.text(), "abc");
    }

    #[test]
    fn backspace_deletes_a_whole_grapheme_cluster() {
        // "é" as e + combining acute (2 bytes after the e) is one grapheme.
        let mut ta = TextArea::from_text("e\u{0301}");
        ta.delete_backward();
        assert_eq!(ta.text(), "", "the combining mark and its base go together");
    }

    #[test]
    fn take_returns_the_text_and_resets() {
        let mut ta = at("hello", 5);
        assert_eq!(ta.take(), "hello");
        assert!(ta.is_empty());
        assert_eq!(ta.cursor(), 0);
    }

    // ===== horizontal movement =====

    #[test]
    fn left_and_right_move_by_grapheme() {
        let mut ta = at("abc", 1);
        ta.move_right();
        assert_eq!(ta.cursor(), 2);
        ta.move_left();
        ta.move_left();
        assert_eq!(ta.cursor(), 0);
        ta.move_left(); // saturates at the start
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn right_moves_over_a_wide_grapheme_in_one_step() {
        // A CJK char is 3 bytes; Right should land past all 3, not mid-character.
        let mut ta = at("世x", 0);
        ta.move_right();
        assert_eq!(ta.cursor(), 3, "moved over the whole wide grapheme");
    }

    #[test]
    fn home_and_end_go_to_logical_line_bounds() {
        let mut ta = at("ab\ncd", 4); // on the second line, between c and d
        ta.move_home();
        assert_eq!(ta.cursor(), 3, "start of the 'cd' line");
        ta.move_end();
        assert_eq!(ta.cursor(), 5, "end of the 'cd' line");
    }

    // ===== wrapping to byte ranges =====

    #[test]
    fn wrap_breaks_on_word_boundaries_and_consumes_the_space() {
        let ta = TextArea::from_text("hello world");
        assert_eq!(rows(&ta, 5), vec!["hello", "world"]);
        // The space (byte 5) is consumed at the break: rows are [0,5) and
        // [6,11) — and nothing after a full last row: the cursor's cell is the
        // column the caller keeps past `width` (docs/textarea.md).
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..11]);
    }

    #[test]
    fn wrap_keeps_text_that_fits_on_one_row() {
        let ta = TextArea::from_text("hello world");
        assert_eq!(rows(&ta, 12), vec!["hello world"]);
    }

    #[test]
    fn wrap_hard_breaks_a_word_longer_than_the_width() {
        let ta = TextArea::from_text("abcdefg");
        assert_eq!(rows(&ta, 3), vec!["abc", "def", "g"]);
    }

    #[test]
    fn wrap_honours_newlines_and_blank_lines() {
        let ta = TextArea::from_text("a\n\nb");
        assert_eq!(rows(&ta, 10), vec!["a", "", "b"]);
        assert_eq!(ta.wrapped_rows(10), vec![0..1, 2..2, 3..4]);
    }

    #[test]
    fn wrap_of_empty_text_is_one_empty_row() {
        let ta = TextArea::new();
        assert_eq!(ta.wrapped_rows(10), vec![0..0]);
        assert_eq!(ta.row_count(10), 1);
    }

    #[test]
    fn wrap_preserves_runs_of_spaces_unlike_message_wrapping() {
        // The editor must not collapse whitespace — what you type is what shows.
        let ta = TextArea::from_text("a  b");
        assert_eq!(rows(&ta, 10), vec!["a  b"]);
    }

    #[test]
    fn wrap_measures_wide_chars_as_two_columns() {
        let ta = TextArea::from_text("你好世界");
        assert_eq!(rows(&ta, 4), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_with_zero_width_only_splits_on_newlines() {
        let ta = TextArea::from_text("a b\nc");
        assert_eq!(rows(&ta, 0), vec!["a b", "c"]);
    }

    #[test]
    fn trailing_spaces_wrap_instead_of_overflowing_the_row() {
        // "hi" + 5 trailing spaces at width 5: the run must wrap onto a
        // continuation row, not pin the cursor past the field edge.
        let ta = TextArea::from_text("hi     ");
        assert_eq!(rows(&ta, 5), vec!["hi   ", "  "]);
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 5..7], "every byte covered");
        assert_eq!(ta.cursor_row_col(5), (1, 2), "col stays within the width");
    }

    #[test]
    fn leading_indentation_wider_than_the_width_hard_breaks() {
        let ta = TextArea::from_text("    a");
        assert_eq!(rows(&ta, 2), vec!["  ", "  ", "a"]);
    }

    #[test]
    fn trailing_spaces_after_wide_chars_wrap_too() {
        // "世界" fills all 4 columns, so the 2 trailing spaces wrap.
        let ta = TextArea::from_text("世界  ");
        assert_eq!(rows(&ta, 4), vec!["世界", "  "]);
        assert_eq!(ta.cursor_row_col(4), (1, 2));
    }

    // ===== cursor <-> (row, col) =====

    #[test]
    fn cursor_row_col_maps_through_a_soft_wrap() {
        let ta = at("hello world", 6); // just before "world"
        assert_eq!(ta.cursor_row_col(5), (1, 0), "start of the wrapped 2nd row");
        // Just after "hello", before the space the wrap consumed: the end of
        // a full row is column 5 — the cell past `width` every caller keeps
        // for the caret (docs/textarea.md), so it stays on its own row.
        let ta = at("hello world", 5);
        assert_eq!(ta.cursor_row_col(5), (0, 5), "the reserved column");
    }

    #[test]
    fn cursor_row_col_at_the_end_of_a_short_row() {
        // After "abc" (three of five columns) the cursor sits at the end of
        // its row, before the consumed space.
        let ta = at("abc def", 3);
        assert_eq!(ta.wrapped_rows(5), vec![0..3, 4..7]);
        assert_eq!(ta.cursor_row_col(5), (0, 3));
    }

    #[test]
    fn the_end_of_a_full_line_before_a_newline_keeps_its_row() {
        // A hard `\n` really ends the row: the cursor before it stays on
        // that row (at its end), not at the head of the next logical line.
        let ta = at("abcde\nfg", 5);
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..8]);
        assert_eq!(ta.cursor_row_col(5), (0, 5));
    }

    #[test]
    fn vertical_motion_from_the_reserved_column_aims_for_that_column() {
        // From the reserved column after the full "abcde", ↓ aims for column
        // 5 on the short "fgh" (clamped to its end), then finds it again on
        // "ijklm"; ↑ retraces the same seats.
        let mut ta = at("abcde fgh ijklm", 5);
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..9, 10..15]);
        assert_eq!(ta.cursor_row_col(5), (0, 5));
        ta.move_down();
        assert_eq!(
            ta.cursor_row_col(5),
            (1, 3),
            "clamped to the short row's end"
        );
        ta.move_down();
        assert_eq!(
            ta.cursor_row_col(5),
            (2, 5),
            "the preferred column restored"
        );
        ta.move_up();
        assert_eq!(ta.cursor_row_col(5), (1, 3));
        ta.move_up();
        assert_eq!(ta.cursor_row_col(5), (0, 5));
    }

    #[test]
    fn cursor_row_col_at_the_very_end() {
        // "world" fills its row exactly: the end-of-text cursor sits in the
        // reserved column after it — on its row, never on an extra one.
        let ta = at("hello world", 11);
        assert_eq!(ta.cursor_row_col(5), (1, 5));
    }

    #[test]
    fn a_full_last_row_keeps_the_cursor_on_it_in_the_reserved_column() {
        // A row exactly `width` wide is the last one: no empty row follows it
        // for the cursor, whose cell is the column the caller keeps past
        // `width`. An empty trailing row here is what read as a newline the
        // user never typed (docs/textarea.md).
        let ta = TextArea::from_text("abcde");
        assert_eq!(ta.wrapped_rows(5), vec![0..5]);
        assert_eq!(ta.row_count(5), 1, "the box grows for text, not the caret");
        assert_eq!(ta.cursor_row_col(5), (0, 5));
    }

    #[test]
    fn a_trailing_space_run_ending_exactly_at_the_width_stays_one_row() {
        let ta = TextArea::from_text("hi   ");
        assert_eq!(ta.wrapped_rows(5), vec![0..5]);
        assert_eq!(ta.cursor_row_col(5), (0, 5));
    }

    #[test]
    fn text_ending_in_a_newline_has_its_empty_last_row() {
        // A full row followed by more text gets nothing extra…
        let ta = TextArea::from_text("abcde\nab");
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..8]);
        // …and text ending in '\n' has its empty last row: that one is real.
        let ta = TextArea::from_text("abcde\n");
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..6]);
    }

    #[test]
    fn cursor_row_col_at_the_end_of_a_full_last_line_of_multi_line_text() {
        let ta = at("ab\nabcde", 8);
        assert_eq!(ta.wrapped_rows(5), vec![0..2, 3..8]);
        assert_eq!(ta.cursor_row_col(5), (1, 5));
    }

    // ===== vertical movement across wrapped rows (warm cache) =====

    #[test]
    fn down_and_up_cross_wrapped_visual_rows() {
        let mut ta = at("hello world", 2); // row 0, col 2 (between "he" and "llo")
        let _ = ta.wrapped_rows(5); // warm the cache (as a render would)
        ta.move_down();
        // Column 2 preserved onto the 2nd visual row → between "wo" and "rld".
        assert_eq!(ta.cursor_row_col(5), (1, 2));
        ta.move_up();
        assert_eq!(ta.cursor_row_col(5), (0, 2), "preferred column restored");
    }

    #[test]
    fn down_preserves_the_preferred_column_across_a_short_row() {
        // Three visual rows; the middle is short. Down from a long row to the short
        // row clamps to its end, but Down again restores the original column.
        let mut ta = at("aaaaa\nb\nccccc", 3); // row 0 col 3
        let _ = ta.wrapped_rows(5);
        ta.move_down(); // onto "b" (col clamps to 1)
        assert_eq!(ta.cursor_row_col(5), (1, 1));
        ta.move_down(); // onto "ccccc" — preferred col 3 restored
        assert_eq!(ta.cursor_row_col(5), (2, 3));
    }

    #[test]
    fn up_from_the_first_row_snaps_to_the_start() {
        let mut ta = at("hello world", 2);
        let _ = ta.wrapped_rows(5);
        ta.move_up();
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn down_from_the_last_row_snaps_to_the_end() {
        let mut ta = at("hello world", 8);
        let _ = ta.wrapped_rows(5);
        ta.move_down();
        assert_eq!(ta.cursor(), 11);
    }

    #[test]
    fn down_from_a_full_last_row_lands_in_its_reserved_column() {
        let mut ta = at("abcdefghij", 7); // row 1 ("fghij"), col 2
        let _ = ta.wrapped_rows(5); // warm the cache (as a render would)
        ta.move_down(); // the last row: snaps to the very end
        assert_eq!(ta.cursor(), 10);
        assert_eq!(ta.cursor_row_col(5), (1, 5), "the reserved column");
    }

    #[test]
    fn a_horizontal_move_resets_the_preferred_column() {
        let mut ta = at("aaaaa\nbb\nccccc", 3); // row 0, col 3
        let _ = ta.wrapped_rows(5);
        ta.move_down(); // onto "bb", clamped to its end (col 2)
        assert_eq!(ta.cursor_row_col(5), (1, 2));
        ta.move_left(); // now at col 1, and preferred_col is cleared
        ta.move_down(); // aims for the *current* column (1), not the old 3
        assert_eq!(ta.cursor_row_col(5), (2, 1));
    }

    // ===== vertical movement with a cold cache (logical-line fallback) =====

    #[test]
    fn vertical_motion_falls_back_to_logical_lines_when_cold() {
        // No render happened, so the wrap cache is cold: Up/Down use '\n' lines.
        let mut ta = at("abc\nxyz", 6); // 2nd line, col 2 (between y and z)
        ta.move_up();
        assert_eq!(ta.cursor(), 2, "up to col 2 of the first logical line");
        ta.move_down();
        assert_eq!(ta.cursor(), 6, "back down to col 2 of the second line");
    }

    #[test]
    fn an_edit_invalidates_the_wrap_cache() {
        let mut ta = TextArea::from_text("hello world");
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..11]);
        ta.set_text("hi");
        assert_eq!(ta.wrapped_rows(5), vec![0..2], "re-wrapped after the edit");
    }

    // ===== word motion (alt+b/f, ctrl+←/→ — docs/textarea.md) =====

    #[test]
    fn word_left_goes_to_the_start_of_the_previous_word() {
        let mut ta = at("foo bar", 7);
        ta.move_word_left();
        assert_eq!(ta.cursor(), 4, "start of \"bar\"");
        ta.move_word_left();
        assert_eq!(ta.cursor(), 0, "start of \"foo\"");
        ta.move_word_left(); // saturates at the start
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn word_left_from_inside_a_word_goes_to_its_start() {
        let mut ta = at("hello", 3);
        ta.move_word_left();
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn word_left_stops_at_punctuation_separated_words() {
        // readline's alnum words: `foo/bar.txt` is three stops, not one.
        let mut ta = at("foo/bar.txt", 11);
        ta.move_word_left();
        assert_eq!(ta.cursor(), 8, "start of \"txt\"");
        ta.move_word_left();
        assert_eq!(ta.cursor(), 4, "start of \"bar\"");
        ta.move_word_left();
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn word_right_goes_to_the_end_of_the_next_word() {
        let mut ta = at("foo bar", 0);
        ta.move_word_right();
        assert_eq!(ta.cursor(), 3, "end of \"foo\"");
        ta.move_word_right();
        assert_eq!(ta.cursor(), 7, "end of \"bar\"");
        ta.move_word_right(); // saturates at the end
        assert_eq!(ta.cursor(), 7);
    }

    #[test]
    fn word_right_from_inside_a_word_goes_to_its_end() {
        let mut ta = at("hello world", 2);
        ta.move_word_right();
        assert_eq!(ta.cursor(), 5);
    }

    #[test]
    fn word_right_with_only_punctuation_ahead_goes_to_the_end() {
        let mut ta = at("foo //", 3);
        ta.move_word_right();
        assert_eq!(ta.cursor(), 6, "no more words: land at the very end");
    }

    #[test]
    fn word_motion_crosses_newlines() {
        let mut ta = at("ab\ncd", 5);
        ta.move_word_left();
        assert_eq!(ta.cursor(), 3, "start of \"cd\"");
        ta.move_word_left();
        assert_eq!(ta.cursor(), 0);
        ta.move_word_right();
        assert_eq!(ta.cursor(), 2, "end of \"ab\"");
    }

    #[test]
    fn word_motion_resets_the_preferred_column() {
        let mut ta = at("aaaaa\nbb\nccccc", 3); // row 0, col 3
        let _ = ta.wrapped_rows(5);
        ta.move_down(); // onto "bb", clamped to col 2, preferred_col pinned at 3
        ta.move_word_left(); // a horizontal move — the pin must clear
        ta.move_down();
        assert_eq!(ta.cursor_row_col(5), (2, 0), "aims for the current column");
    }

    // ===== word/line boundary queries (the kill targets — docs/textarea.md) =====

    #[test]
    fn prev_word_boundary_is_where_word_left_would_go() {
        let ta = at("foo bar ", 8);
        assert_eq!(ta.prev_word_boundary(), 4, "the trailing space and \"bar\"");
    }

    #[test]
    fn next_word_boundary_is_where_word_right_would_go() {
        let ta = at("foo bar", 3);
        assert_eq!(ta.next_word_boundary(), 7, "\" bar\" — to the word's end");
    }

    #[test]
    fn prev_unix_word_boundary_is_whitespace_delimited() {
        // ctrl+w's unix-word-rubout: `bar.txt` goes as one word, where
        // `prev_word_boundary` (alt+backspace) stops at the dot.
        let ta = at("foo bar.txt ", 12);
        assert_eq!(ta.prev_unix_word_boundary(), 4);
        assert_eq!(ta.prev_word_boundary(), 8);
    }

    #[test]
    fn prev_unix_word_boundary_crosses_a_newline_like_whitespace() {
        let ta = at("ab\ncd", 3);
        assert_eq!(ta.prev_unix_word_boundary(), 0, "the newline is whitespace");
    }

    #[test]
    fn cursor_line_start_is_the_logical_line_start() {
        let ta = at("ab\ncd", 4);
        assert_eq!(ta.cursor_line_start(), 3);
        let ta = at("hello", 3);
        assert_eq!(ta.cursor_line_start(), 0);
    }

    #[test]
    fn cursor_kill_end_is_the_logical_line_end() {
        let ta = at("ab\ncd", 1);
        assert_eq!(ta.cursor_kill_end(), 2, "up to the newline, not past it");
    }

    #[test]
    fn cursor_kill_end_at_a_line_end_takes_the_newline() {
        // Emacs' ctrl+k: at the end of a line the kill eats the '\n' (joining
        // the lines) instead of being a dead no-op.
        let ta = at("ab\ncd", 2);
        assert_eq!(ta.cursor_kill_end(), 3);
        let ta = at("ab\ncd", 5); // at the very end there is nothing to take
        assert_eq!(ta.cursor_kill_end(), 5);
    }

    #[test]
    fn move_up_from_an_abutting_row_boundary_still_moves() {
        // A hard-broken word's chunks abut byte-exactly: "abcdefghijklmno"
        // at width 5 wraps to [0..5, 5..10, 10..15]. A cursor parked on a
        // shared boundary byte with a pinned preferred_col at (or past) the
        // previous row's width used to compute a "previous-row" position
        // equal to itself, so every further ↑ was a permanent no-op.
        let mut ta = at("abcdefghijklmno", 15); // row 2, the reserved column
        let _ = ta.wrapped_rows(5); // warm the cache at width 5
        ta.move_up(); // pins preferred_col 5: lands on byte 10, the boundary
        assert_eq!(ta.cursor(), 10);
        ta.move_up();
        assert_ne!(ta.cursor(), 10, "↑ must move off the boundary");
        assert!(ta.cursor() < 10, "and keeps climbing");
    }
}

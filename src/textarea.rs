//! A codex-style editable text input: a cursor you can move *anywhere*, with
//! insert/delete at the cursor and movement across wrapped visual rows.
//!
//! This is a focused port of the editing core of openai/codex's
//! `bottom_pane/textarea.rs` — the same four load-bearing fields (`text`,
//! `cursor`, a width-keyed `wrap_cache`, and a `preferred_col` for vertical
//! motion) — deliberately *without* codex's vim mode, atomic `@mention`
//! elements, emacs kill-buffer/yank, masking, highlights or keymap-config. See
//! `docs/textarea.md`.
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
            self.cursor = self.pos_at_col(prev.start, prev.end, target);
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
    /// shows at the next row's start), and the column is the display width up to it.
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
/// the run of spaces at a soft break, and hard-breaks an over-long word on
/// grapheme boundaries. `width == 0` disables wrapping (split on `'\n'` only).
///
/// Always returns at least one range (the empty draft is one empty row). The clean
/// re-derivation of codex's `wrap_ranges` on top of our own greedy algorithm.
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
    let mut pending: Option<(usize, usize)> = None; // an inter-word space run (end, width)

    for (range, is_space) in tokenize(text, seg.clone()) {
        let tw = cols(&text[range.clone()]);
        if is_space {
            if at_head && first {
                // Leading indentation on the first row — show it.
                cur_end = range.end;
                col += tw;
                at_head = false;
            } else if at_head {
                // A break-space at the head of a wrapped row — consume (don't show).
                row_start = range.end;
                cur_end = range.end;
            } else {
                pending = Some((range.end, tw));
            }
            first = false;
            continue;
        }
        first = false;
        let sp_w = pending.map_or(0, |(_, w)| w);
        if !at_head && col + sp_w + tw > width {
            // Break before this word; the pending space is consumed at the break.
            rows.push(row_start..cur_end);
            row_start = range.start;
            cur_end = range.start;
            col = 0;
            at_head = true;
            pending = None;
        } else if let Some((sp_end, sp_w)) = pending.take() {
            // The space fits — place it before the word.
            cur_end = sp_end;
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
    if let Some((sp_end, sp_w)) = pending.take() {
        // Trailing spaces: show them so the cursor after a trailing space is visible.
        cur_end = sp_end;
        col += sp_w;
        let _ = col;
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
        // The space (byte 5) is consumed at the break: rows are [0,5) and [6,11).
        assert_eq!(ta.wrapped_rows(5), vec![0..5, 6..11]);
    }

    #[test]
    fn wrap_keeps_text_that_fits_on_one_row() {
        let ta = TextArea::from_text("hello world");
        assert_eq!(rows(&ta, 11), vec!["hello world"]);
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

    // ===== cursor <-> (row, col) =====

    #[test]
    fn cursor_row_col_maps_through_a_soft_wrap() {
        let ta = at("hello world", 6); // just before "world"
        assert_eq!(ta.cursor_row_col(5), (1, 0), "start of the wrapped 2nd row");
        let ta = at("hello world", 5); // just after "hello"
        assert_eq!(ta.cursor_row_col(5), (0, 5), "end of the 1st row");
    }

    #[test]
    fn cursor_row_col_at_the_very_end() {
        let ta = at("hello world", 11);
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
}

//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`input_view`], [`input_cursor_x`]), build
//! ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Message, Role};

/// Display width of `s` in terminal columns.
///
/// All width math in this module goes through this instead of `chars().count()`:
/// CJK and many emoji are two columns wide and combining marks are zero, so a
/// raw char count would wrap and pad non-ASCII text incorrectly.
fn cols(s: &str) -> usize {
    s.width()
}

/// Display width of a single `char` in terminal columns (control chars → 0).
fn char_cols(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

// --- Claude-Code-ish styling. Centralised so it's trivial to retheme. ---

/// Prompt shown at the start of the input field.
const PROMPT: &str = "❯ ";
/// Bullet prefixing a user message.
const USER_BULLET: &str = "❯ ";
/// Bullet prefixing an assistant message.
const AI_BULLET: &str = "● ";
/// Bullet prefixing a backend-error notice — same glyph as the assistant, but
/// coloured red (see [`ERROR_COLOR`]) so a failure reads as a red bullet point.
const ERROR_BULLET: &str = "● ";
/// Indent for wrapped continuation lines (matches a bullet's width).
const INDENT: &str = "  ";
/// Columns a bullet/indent occupies, subtracted from the content width.
const BULLET_WIDTH: u16 = 2;

const USER_COLOR: Color = Color::Rgb(0x6E, 0x6E, 0x6E);
const USER_BG_COLOR: Color = Color::Rgb(0x2D, 0x2D, 0x2D);
const AI_COLOR: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const ERROR_COLOR: Color = Color::Rgb(0xE0, 0x6C, 0x75);
const PROMPT_COLOR: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const BORDER_COLOR: Color = Color::Rgb(0xAA, 0xAA, 0xAA);

// --- Live-region geometry. The single source of truth for the bottom region's
// row split; both `render_live` and `cursor_position` derive from `live_layout`,
// and `main.rs` sizes its viewport and repaint budget from `LIVE_HEIGHT`. ---

/// Rows in the streaming-preview strip above the input box.
const PREVIEW_ROWS: u16 = 1;
/// Rows in the rule-framed input box: top rule + text row + bottom rule.
const INPUT_ROWS: u16 = 3;
/// Total height of the bottom live region — the one number `main.rs` shares with
/// this module. Defined as the sum of the row constants so it can never drift.
pub const LIVE_HEIGHT: u16 = PREVIEW_ROWS + INPUT_ROWS;

/// Split `area` into the live region's two stacked sub-areas `[preview, input]`.
/// This is the only place the preview/input row split is expressed.
fn live_layout(area: Rect) -> [Rect; 2] {
    Layout::vertical([
        Constraint::Length(PREVIEW_ROWS),
        Constraint::Length(INPUT_ROWS),
    ])
    .areas(area)
}

/// Greedy word-wrap `text` to `width` columns.
///
/// - Existing `'\n'`s are honoured (and blank lines preserved).
/// - Words longer than `width` are hard-broken across lines.
/// - `width == 0` disables wrapping (text is only split on `'\n'`).
///
/// Crucially this is *prefix-stable*: appending more text only ever changes the
/// last produced line, which is what lets streaming commit completed lines to
/// scrollback (see `main.rs`).
#[must_use]
pub fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(str::to_string).collect();
    }
    let width = width as usize;
    let mut out = Vec::new();
    for segment in text.split('\n') {
        if segment.split_whitespace().next().is_none() {
            out.push(String::new()); // blank line
            continue;
        }
        out.extend(wrap_segment(segment, width));
    }
    out
}

/// Greedy-wrap a single newline-free segment that has at least one word.
///
/// All length comparisons are in display columns (see [`cols`]), so wide CJK
/// glyphs count as two and zero-width marks as zero.
fn wrap_segment(segment: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0; // display width of `cur` in columns

    for word in segment.split_whitespace() {
        let word_w = cols(word);

        if word_w > width {
            // Hard-break a word that can't fit on any line, splitting on column
            // boundaries (a single wide char that overflows a 1-column line is
            // placed alone — a char can't be split further).
            if cur_w > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for ch in word.chars() {
                let ch_w = char_cols(ch);
                if cur_w > 0 && cur_w + ch_w > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += ch_w;
            }
            continue;
        }

        let needed = if cur_w == 0 {
            word_w
        } else {
            cur_w + 1 + word_w
        };
        if needed > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = word_w;
        } else {
            if cur_w > 0 {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(word);
            cur_w += word_w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Build the styled, wrapped lines for one message.
///
/// The first line carries a coloured role bullet; continuation lines are
/// indented to align under it. Returned lines are `'static` (owned), so the
/// caller can hand them to `insert_before` without lifetime juggling.
#[must_use]
pub fn message_lines(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    let (bullet, color) = match role {
        Role::User => (USER_BULLET, USER_COLOR),
        Role::Assistant => (AI_BULLET, AI_COLOR),
        Role::Error => (ERROR_BULLET, ERROR_COLOR),
    };
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);

    let bg = if role == Role::User {
        Style::new().bg(USER_BG_COLOR)
    } else {
        Style::default()
    };
    let cw = content_width as usize;
    wrap_text(text, content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            // Pad to content_width *columns* (not chars) so the background fills
            // the full terminal row even when the line holds wide CJK/emoji.
            let padded = if role == Role::User {
                let pad = " ".repeat(cw.saturating_sub(cols(&line)));
                format!("{line}{pad}")
            } else {
                line
            };
            if i == 0 {
                Line::from(vec![
                    Span::styled(bullet.to_string(), bullet_style),
                    Span::raw(padded),
                ])
                .style(bg)
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(padded)]).style(bg)
            }
        })
        .collect()
}

/// The slice of `input` that should be visible in a field `width` columns wide,
/// anchored to the end so the cursor (at the end of input) stays in view.
#[must_use]
pub fn input_view(input: &str, width: u16) -> String {
    let width = width as usize;
    if cols(input) <= width {
        return input.to_string();
    }
    // Keep the longest whole-char suffix whose display width fits `width`, so a
    // trailing wide char is dropped as a unit rather than sliced mid-glyph.
    let mut used = 0;
    let mut start = input.len();
    for (idx, ch) in input.char_indices().rev() {
        let ch_w = char_cols(ch);
        if used + ch_w > width {
            break;
        }
        used += ch_w;
        start = idx;
    }
    input[start..].to_string()
}

/// Column (relative to the field) where the cursor sits: just after the visible
/// text, clamped to the field width.
#[must_use]
pub fn input_cursor_x(input: &str, width: u16) -> u16 {
    // The cursor sits right after the visible text, so its column is exactly the
    // display width of the (possibly scrolled) view — never past the field edge.
    cols(&input_view(input, width)) as u16
}

/// Render the bottom live region into `buf`: a one-row preview (the streaming
/// line, or blank when idle) above a rule-framed input field.
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App) {
    let [preview_area, input_area] = live_layout(area);

    // Preview row: the in-progress line while streaming, else blank.
    let preview = match app.streaming_text() {
        Some(text) => message_lines(Role::Assistant, text, preview_area.width)
            .pop()
            .unwrap_or_default(),
        None => Line::default(),
    };
    Paragraph::new(preview).render(preview_area, buf);

    // Input field framed by a top and bottom rule (no side borders), with a
    // coloured prompt and the (scrolled) input text.
    let block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_COLOR));
    let inner = block.inner(input_area);
    block.render(input_area, buf);

    let field_width = inner.width.saturating_sub(cols(PROMPT) as u16);
    let line = Line::from(vec![
        Span::styled(PROMPT, Style::new().fg(PROMPT_COLOR)),
        Span::raw(input_view(&app.input, field_width)),
    ]);
    Paragraph::new(line).render(inner, buf);
}

/// Decide which assistant lines are now safe to flush to scrollback as a reply
/// streams in.
///
/// Greedy word-wrap is *prefix-stable* — only the final wrapped line can still
/// change as more text arrives — so we commit everything up to it. Given how
/// many lines were `committed` already, returns the new lines to commit and the
/// updated committed count. `committed` is clamped so a mid-stream resize (which
/// re-wraps to a different line count) can't panic.
#[must_use]
pub fn stable_commit(text: &str, width: u16, committed: usize) -> (Vec<Line<'static>>, usize) {
    let lines = message_lines(Role::Assistant, text, width);
    let stable = lines.len().saturating_sub(1);
    let start = committed.min(stable);
    (lines[start..stable].to_vec(), stable.max(committed))
}

/// The remaining (last) lines to flush once the reply is complete.
#[must_use]
pub fn final_commit(text: &str, width: u16, committed: usize) -> Vec<Line<'static>> {
    let lines = message_lines(Role::Assistant, text, width);
    let start = committed.min(lines.len());
    lines[start..].to_vec()
}

/// Build the whole conversation as styled lines, mirroring how it was streamed
/// to scrollback: each message's wrapped lines, with a blank spacer after every
/// message. Used to repaint after a resize clears the screen.
#[must_use]
pub fn conversation_lines(history: &[Message], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in history {
        lines.extend(message_lines(message.role, &message.text, width));
        lines.push(Line::default()); // blank spacer after every message
    }
    lines
}

/// The last `max_rows` lines of the conversation — i.e. the tail that fits on
/// screen above the live region. After a width shrink ratatui clears the visible
/// screen (older lines survive in the terminal's own scrollback), so we only
/// need to repaint what was on screen; capping at `max_rows` also avoids
/// re-scrolling content the terminal already kept.
#[must_use]
pub fn repaint_lines(history: &[Message], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let mut lines = conversation_lines(history, width);
    if lines.len() > max_rows {
        lines = lines.split_off(lines.len() - max_rows);
    }
    lines
}

/// How many history rows fit above the live region on a `term_height`-row screen
/// — the number of lines to repaint after a resize. Saturates at 0 so a terminal
/// shorter than the live region can never underflow. The pure counterpart of the
/// terminal calls in `main.rs::repaint_after_resize`.
#[must_use]
pub fn repaint_budget(term_height: u16) -> usize {
    term_height.saturating_sub(LIVE_HEIGHT) as usize
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input. Mirrors [`render_live`]'s layout so the cursor lands exactly
/// after the visible input text.
#[must_use]
pub fn cursor_position(area: Rect, input: &str) -> (u16, u16) {
    let [_, input_area] = live_layout(area);
    // Mirror render_live's layout: a top/bottom rule means the content sits one
    // row down and flush-left (no side border to offset by).
    let inner = input_area.inner(Margin::new(0, 1));
    let prompt_width = cols(PROMPT) as u16;
    let field_width = inner.width.saturating_sub(prompt_width);
    let x = inner.x + prompt_width + input_cursor_x(input, field_width);
    (x, inner.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenate a line's span contents into its plain text.
    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Read row `y` of a buffer back as a string.
    fn row(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| buf[(x, y)].symbol()).collect()
    }

    // --- wrap_text ---

    #[test]
    fn wrap_text_breaks_on_word_boundaries() {
        assert_eq!(wrap_text("hello world", 5), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_text_keeps_text_that_fits_on_one_line() {
        assert_eq!(wrap_text("hello world", 11), vec!["hello world"]);
    }

    #[test]
    fn wrap_text_hard_breaks_words_longer_than_width() {
        assert_eq!(wrap_text("aaaaaa", 3), vec!["aaa", "aaa"]);
        assert_eq!(wrap_text("abcdefg", 3), vec!["abc", "def", "g"]);
    }

    #[test]
    fn wrap_text_preserves_blank_lines() {
        assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_text_of_empty_string_is_a_single_blank_line() {
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn wrap_text_with_zero_width_does_not_wrap() {
        assert_eq!(wrap_text("a b", 0), vec!["a b"]);
    }

    #[test]
    fn wrap_text_hard_breaks_a_word_that_is_an_exact_multiple_of_width() {
        // A word whose length is an exact multiple of the width must not leave a
        // phantom trailing blank line or drop the final full chunk.
        assert_eq!(wrap_text("aaaaaaaaa", 3), vec!["aaa", "aaa", "aaa"]);
    }

    #[test]
    fn wrap_text_packs_a_following_word_onto_a_hard_break_remainder() {
        // "abcdefg" (7) hard-breaks at width 5 into "abcde" + the remainder "fg".
        // The next token "h" was whitespace-separated in the input, so the space
        // in "fg h" is real — this is ordinary greedy packing (like `fold`), not
        // an invented word boundary. Locks the behaviour against regression.
        assert_eq!(wrap_text("abcdefg h", 5), vec!["abcde", "fg h"]);
    }

    #[test]
    fn wrap_text_exact_multiple_remainder_does_not_absorb_the_next_word() {
        // "aaaaaa" is an exact multiple of 3 → two full lines, nothing buffered,
        // so the following word "bb" correctly starts its own line.
        assert_eq!(wrap_text("aaaaaa bb", 3), vec!["aaa", "aaa", "bb"]);
    }

    #[test]
    fn wrap_text_measures_wide_chars_as_two_columns() {
        // CJK glyphs occupy 2 terminal columns each, so only two fit in width 4
        // — not four, as a naive char count would allow.
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_text_treats_a_zero_width_combining_mark_as_zero_columns() {
        // "e" + combining acute is one column wide, so it fits a width-1 line
        // instead of being hard-broken onto two lines like a 2-char count implies.
        assert_eq!(wrap_text("e\u{0301}", 1), vec!["e\u{0301}"]);
    }

    // --- message_lines ---

    #[test]
    fn message_lines_prefixes_assistant_bullet() {
        let lines = message_lines(Role::Assistant, "hello", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(plain(&lines[0]), "● hello");
    }

    #[test]
    fn message_lines_prefixes_user_bullet() {
        let lines = message_lines(Role::User, "hello", 80);
        assert_eq!(plain(&lines[0]).trim_end(), "❯ hello");
    }

    #[test]
    fn message_lines_indents_wrapped_continuation_lines() {
        // width 8 → content width 6 → "hello"/"world" on separate lines.
        let lines = message_lines(Role::Assistant, "hello world", 8);
        assert!(lines.len() >= 2);
        assert_eq!(plain(&lines[0]), "● hello");
        assert_eq!(plain(&lines[1]), "  world");
    }

    #[test]
    fn message_lines_colours_the_bullet() {
        let lines = message_lines(Role::Assistant, "hi", 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(AI_COLOR));
    }

    #[test]
    fn message_lines_renders_errors_with_a_red_bullet() {
        let lines = message_lines(Role::Error, "stream failed", 80);
        assert_eq!(lines.len(), 1);
        assert!(plain(&lines[0]).contains("stream failed"));
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(ERROR_COLOR),
            "the error bullet is red, not white"
        );
        assert_ne!(ERROR_COLOR, AI_COLOR, "error colour differs from assistant");
    }

    #[test]
    fn message_lines_applies_background_to_user_lines() {
        let lines = message_lines(Role::User, "hi there long enough to wrap", 10);
        for line in &lines {
            assert_eq!(
                line.style.bg,
                Some(USER_BG_COLOR),
                "every user line has the background"
            );
        }
    }

    #[test]
    fn message_lines_user_spans_fill_the_full_width() {
        let width = 20u16;
        let lines = message_lines(Role::User, "hi", width);
        for line in &lines {
            let span_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(
                span_chars as u16, width,
                "spans cover full width so background extends edge-to-edge"
            );
        }
    }

    #[test]
    fn message_lines_pads_user_lines_to_full_display_width() {
        // Padding must count terminal columns, not chars: a CJK user line still
        // fills the row edge-to-edge so its dark background does not fall short.
        let width = 20u16;
        let lines = message_lines(Role::User, "你好", width);
        for line in &lines {
            let total: usize = line.spans.iter().map(|s| cols(s.content.as_ref())).sum();
            assert_eq!(total as u16, width, "user line fills full display width");
        }
    }

    // --- input view / cursor ---

    #[test]
    fn input_view_shows_the_tail_when_too_long() {
        assert_eq!(input_view("abcdef", 3), "def");
    }

    #[test]
    fn input_view_shows_everything_when_it_fits() {
        assert_eq!(input_view("ab", 5), "ab");
    }

    #[test]
    fn input_cursor_stops_at_the_right_edge() {
        assert_eq!(input_cursor_x("abcdef", 3), 3);
        assert_eq!(input_cursor_x("ab", 5), 2);
    }

    #[test]
    fn input_view_keeps_the_tail_by_display_columns() {
        // Field width 4 holds two 2-column CJK chars, so only the last two show.
        assert_eq!(input_view("你好世界", 4), "世界");
    }

    #[test]
    fn input_cursor_x_counts_display_columns_not_chars() {
        // Two 2-column chars put the cursor at column 4, not 2.
        assert_eq!(input_cursor_x("你好", 10), 4);
    }

    // --- render_live (against a plain Buffer) ---

    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn render_live_shows_blank_preview_when_idle() {
        let mut app = App::new();
        app.input = "hello".to_string();
        let mut buf = buffer(40, 4);
        render_live(buf.area, &mut buf, &app);

        assert!(
            row(&buf, 0, 40).trim().is_empty(),
            "preview row is blank when idle"
        );
        assert_eq!(buf[(0, 1)].symbol(), "─", "top rule above the input");
        assert!(
            row(&buf, 2, 40).contains("❯ hello"),
            "input row shows prompt + text"
        );
        assert_eq!(buf[(0, 3)].symbol(), "─", "bottom rule below the input");
    }

    // --- streaming commit bookkeeping ---

    #[test]
    fn incremental_commits_reconstruct_the_whole_reply() {
        // Stream a reply word-by-word, committing stable lines as we go, and
        // confirm the committed lines (plus the final flush) exactly equal the
        // fully-rendered message — no gaps, no duplicates, no reordering.
        let full = "the quick brown fox jumps over the lazy dog and then \
                    some extra words to force several wrapped lines here";
        let width = 20;
        let expected: Vec<String> = message_lines(Role::Assistant, full, width)
            .iter()
            .map(plain)
            .collect();

        let mut committed = 0;
        let mut got: Vec<String> = Vec::new();
        let mut acc = String::new();
        for chunk in crate::stream::chunks(full) {
            acc.push_str(&chunk);
            let (lines, new_committed) = stable_commit(&acc, width, committed);
            got.extend(lines.iter().map(plain));
            committed = new_committed;
        }
        got.extend(final_commit(&acc, width, committed).iter().map(plain));

        assert_eq!(got, expected);
    }

    #[test]
    fn stable_commit_withholds_the_last_line() {
        // "hi there" fits one line → nothing is stable yet.
        let (lines, committed) = stable_commit("hi there", 80, 0);
        assert!(lines.is_empty());
        assert_eq!(committed, 0);
    }

    #[test]
    fn stable_commit_clamps_when_a_resize_shrinks_the_line_count() {
        // A mid-stream width *grow* re-wraps the same reply to fewer lines, so
        // the previously-committed count can exceed the new stable count. The
        // clamps must absorb that: no slice panic, an empty new batch, and a
        // committed counter that never regresses. (Raw indexing here would
        // panic with `start > end`.)
        let text = "the quick brown fox jumps over the lazy dog";
        let (_, committed_narrow) = stable_commit(text, 6, 0);
        assert!(committed_narrow >= 1, "a narrow wrap commits several lines");

        let (lines, committed_after) = stable_commit(text, 80, committed_narrow);
        assert!(lines.is_empty(), "re-wrapped wider, nothing new is stable");
        assert_eq!(
            committed_after, committed_narrow,
            "committed never regresses"
        );
    }

    #[test]
    fn final_commit_clamps_an_over_large_committed_count() {
        // If `committed` outruns the re-wrapped line count (e.g. after a resize),
        // final_commit returns nothing rather than panicking on `lines[start..]`.
        assert!(final_commit("a short reply", 80, 999).is_empty());
    }

    #[test]
    fn cursor_sits_after_the_prompt_and_input() {
        // area 40x4 → input rows 1..4 framed by a top/bottom rule, so content
        // sits at row 2 flush-left (no side border). Empty input → cursor right
        // after "❯ ".
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), ""), (2, 2));
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), "hi"), (4, 2));
    }

    // --- live-region layout: single source of truth ---

    #[test]
    fn live_height_equals_the_sum_of_the_live_layout_rows() {
        let [preview, input] = live_layout(Rect::new(0, 0, 40, LIVE_HEIGHT));
        assert_eq!(
            preview.height + input.height,
            LIVE_HEIGHT,
            "LIVE_HEIGHT must stay in lockstep with the row constants"
        );
    }

    #[test]
    fn cursor_row_sits_on_the_rendered_prompt_row() {
        // render_live and cursor_position both derive their geometry from
        // live_layout, so the hardware cursor lands on exactly the row where the
        // prompt is drawn — they cannot drift apart.
        let area = Rect::new(0, 0, 40, LIVE_HEIGHT);
        let mut app = App::new();
        app.input = "x".to_string();
        let mut buf = Buffer::empty(area);
        render_live(area, &mut buf, &app);
        let (_, cy) = cursor_position(area, &app.input);
        let rendered: String = (0..area.width).map(|x| buf[(x, cy)].symbol()).collect();
        assert!(
            rendered.contains('❯'),
            "cursor row carries the prompt glyph"
        );
    }

    #[test]
    fn repaint_budget_is_the_screen_minus_the_live_region() {
        assert_eq!(repaint_budget(10), 10 - LIVE_HEIGHT as usize);
        assert_eq!(repaint_budget(LIVE_HEIGHT), 0);
        assert_eq!(repaint_budget(LIVE_HEIGHT - 1), 0, "saturates, never wraps");
    }

    #[test]
    fn bullet_prefixes_all_occupy_bullet_width_columns() {
        let bw = BULLET_WIDTH as usize;
        assert_eq!(cols(PROMPT), bw, "prompt width matches BULLET_WIDTH");
        assert_eq!(cols(USER_BULLET), bw, "user bullet matches BULLET_WIDTH");
        assert_eq!(cols(AI_BULLET), bw, "assistant bullet matches BULLET_WIDTH");
        assert_eq!(cols(ERROR_BULLET), bw, "error bullet matches BULLET_WIDTH");
        assert_eq!(cols(INDENT), bw, "continuation indent matches BULLET_WIDTH");
    }

    // --- conversation repaint (after a resize) ---

    fn msg(role: Role, text: &str) -> Message {
        Message {
            role,
            text: text.to_string(),
        }
    }

    #[test]
    fn conversation_lines_lays_out_a_turn_with_a_trailing_blank() {
        let history = [msg(Role::User, "hi"), msg(Role::Assistant, "hello")];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // User line, blank, assistant line, blank spacer after the reply.
        assert_eq!(texts, vec!["❯ hi", "", "● hello", ""]);
    }

    #[test]
    fn repaint_lines_keeps_only_the_last_max_rows() {
        // Lines are: "❯ one", "● two", "" → keep the last 2.
        let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
        let texts: Vec<String> = repaint_lines(&history, 80, 2).iter().map(plain).collect();
        assert_eq!(texts, vec!["● two", ""]);
    }

    #[test]
    fn repaint_lines_returns_everything_when_it_fits() {
        let history = [msg(Role::User, "hi")];
        assert_eq!(repaint_lines(&history, 80, 100).len(), 2); // message + blank
    }

    #[test]
    fn repaint_lines_of_empty_history_is_empty() {
        assert!(repaint_lines(&[], 80, 10).is_empty());
    }

    #[test]
    fn render_live_shows_streaming_text_in_preview_row() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("Hi there");
        let mut buf = buffer(40, 4);
        render_live(buf.area, &mut buf, &app);

        let preview = row(&buf, 0, 40);
        assert!(preview.contains("●"), "preview shows assistant bullet");
        assert!(preview.contains("Hi there"), "preview shows streamed text");
    }
}

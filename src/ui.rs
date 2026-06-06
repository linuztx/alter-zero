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
use ratatui::widgets::{Block, BorderType, Paragraph, Widget};

use crate::app::{App, Message, Role};

// --- Claude-Code-ish styling. Centralised so it's trivial to retheme. ---

/// Prompt shown at the start of the input box.
const PROMPT: &str = "> ";
/// Bullet prefixing a user message.
const USER_BULLET: &str = "› ";
/// Bullet prefixing an assistant message.
const AI_BULLET: &str = "● ";
/// Indent for wrapped continuation lines (matches a bullet's width).
const INDENT: &str = "  ";
/// Columns a bullet/indent occupies, subtracted from the content width.
const BULLET_WIDTH: u16 = 2;

const USER_COLOR: Color = Color::Cyan;
const AI_COLOR: Color = Color::Rgb(0xD7, 0x77, 0x57); // warm terracotta accent
const PROMPT_COLOR: Color = Color::Rgb(0xD7, 0x77, 0x57);
const HINT_COLOR: Color = Color::DarkGray;
const BORDER_COLOR: Color = Color::DarkGray;

/// Greedy word-wrap `text` to `width` columns.
///
/// - Existing `'\n'`s are honoured (and blank lines preserved).
/// - Words longer than `width` are hard-broken across lines.
/// - `width == 0` disables wrapping (text is only split on `'\n'`).
///
/// Crucially this is *prefix-stable*: appending more text only ever changes the
/// last produced line, which is what lets streaming commit completed lines to
/// scrollback (see `main.rs`).
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
fn wrap_segment(segment: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0; // length of `cur` in chars

    for word in segment.split_whitespace() {
        let word_len = word.chars().count();

        if word_len > width {
            // Hard-break a word that can't fit on any line.
            if cur_len > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            let chars: Vec<char> = word.chars().collect();
            for piece in chars.chunks(width) {
                if piece.len() == width {
                    lines.push(piece.iter().collect());
                } else {
                    cur = piece.iter().collect();
                    cur_len = piece.len();
                }
            }
            continue;
        }

        let needed = if cur_len == 0 {
            word_len
        } else {
            cur_len + 1 + word_len
        };
        if needed > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_len = word_len;
        } else {
            if cur_len > 0 {
                cur.push(' ');
                cur_len += 1;
            }
            cur.push_str(word);
            cur_len += word_len;
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
pub fn message_lines(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    let (bullet, color) = match role {
        Role::User => (USER_BULLET, USER_COLOR),
        Role::Assistant => (AI_BULLET, AI_COLOR),
    };
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);

    wrap_text(text, content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(bullet.to_string(), bullet_style),
                    Span::raw(line),
                ])
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(line)])
            }
        })
        .collect()
}

/// The slice of `input` that should be visible in a field `width` columns wide,
/// anchored to the end so the cursor (at the end of input) stays in view.
pub fn input_view(input: &str, width: u16) -> String {
    let width = width as usize;
    let count = input.chars().count();
    if count <= width {
        input.to_string()
    } else {
        input.chars().skip(count - width).collect()
    }
}

/// Column (relative to the field) where the cursor sits: just after the visible
/// text, clamped to the field width.
pub fn input_cursor_x(input: &str, width: u16) -> u16 {
    (input.chars().count() as u16).min(width)
}

/// Render the bottom live region into `buf`: a one-row preview (the streaming
/// line, or a dim hint when idle) above a rounded input box.
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App, hint: &str) {
    let [preview_area, input_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(area);

    // Preview row: the in-progress line while streaming, else the hint.
    let preview = match app.streaming_text() {
        Some(text) => message_lines(Role::Assistant, text, preview_area.width)
            .pop()
            .unwrap_or_default(),
        None => Line::from(Span::styled(hint.to_string(), Style::new().fg(HINT_COLOR))),
    };
    Paragraph::new(preview).render(preview_area, buf);

    // Rounded input box with a coloured prompt and the (scrolled) input text.
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER_COLOR));
    let inner = block.inner(input_area);
    block.render(input_area, buf);

    let field_width = inner.width.saturating_sub(PROMPT.len() as u16);
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
pub fn stable_commit(text: &str, width: u16, committed: usize) -> (Vec<Line<'static>>, usize) {
    let lines = message_lines(Role::Assistant, text, width);
    let stable = lines.len().saturating_sub(1);
    let start = committed.min(stable);
    (lines[start..stable].to_vec(), stable.max(committed))
}

/// The remaining (last) lines to flush once the reply is complete.
pub fn final_commit(text: &str, width: u16, committed: usize) -> Vec<Line<'static>> {
    let lines = message_lines(Role::Assistant, text, width);
    let start = committed.min(lines.len());
    lines[start..].to_vec()
}

/// Build the whole conversation as styled lines, mirroring how it was streamed
/// to scrollback: each message's wrapped lines, with a blank spacer after every
/// assistant reply. Used to repaint after a resize clears the screen.
pub fn conversation_lines(history: &[Message], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in history {
        lines.extend(message_lines(message.role, &message.text, width));
        if message.role == Role::Assistant {
            lines.push(Line::default()); // blank spacer after each reply
        }
    }
    lines
}

/// The last `max_rows` lines of the conversation — i.e. the tail that fits on
/// screen above the live region. After a width shrink ratatui clears the visible
/// screen (older lines survive in the terminal's own scrollback), so we only
/// need to repaint what was on screen; capping at `max_rows` also avoids
/// re-scrolling content the terminal already kept.
pub fn repaint_lines(history: &[Message], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let mut lines = conversation_lines(history, width);
    if lines.len() > max_rows {
        lines = lines.split_off(lines.len() - max_rows);
    }
    lines
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input. Mirrors [`render_live`]'s layout so the cursor lands exactly
/// after the visible input text.
pub fn cursor_position(area: Rect, input: &str) -> (u16, u16) {
    let [_, input_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(3)]).areas(area);
    let inner = input_area.inner(Margin::new(1, 1));
    let field_width = inner.width.saturating_sub(PROMPT.len() as u16);
    let x = inner.x + PROMPT.len() as u16 + input_cursor_x(input, field_width);
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
        assert_eq!(plain(&lines[0]), "› hello");
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

    // --- render_live (against a plain Buffer) ---

    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn render_live_draws_hint_and_input_when_idle() {
        let mut app = App::new();
        app.input = "hello".to_string();
        let mut buf = buffer(40, 4);
        render_live(buf.area, &mut buf, &app, "type here");

        assert!(
            row(&buf, 0, 40).contains("type here"),
            "preview row shows hint"
        );
        assert_eq!(buf[(0, 1)].symbol(), "╭", "rounded input box");
        assert!(
            row(&buf, 2, 40).contains("> hello"),
            "input row shows prompt + text"
        );
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
    fn cursor_sits_after_the_prompt_and_input() {
        // area 40x4 → input box rows 1..4, inner content at (1,2), width 38,
        // field width 36. Empty input → cursor right after "> ".
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), ""), (3, 2));
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), "hi"), (5, 2));
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
        let texts: Vec<String> = conversation_lines(&history, 80).iter().map(plain).collect();
        // User line, assistant line, then a blank spacer after the reply.
        assert_eq!(texts, vec!["› hi", "● hello", ""]);
    }

    #[test]
    fn repaint_lines_keeps_only_the_last_max_rows() {
        // Lines are: "› one", "● two", "" → keep the last 2.
        let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
        let texts: Vec<String> = repaint_lines(&history, 80, 2).iter().map(plain).collect();
        assert_eq!(texts, vec!["● two", ""]);
    }

    #[test]
    fn repaint_lines_returns_everything_when_it_fits() {
        let history = [msg(Role::User, "hi")];
        assert_eq!(repaint_lines(&history, 80, 100).len(), 1);
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
        render_live(buf.area, &mut buf, &app, "unused hint");

        let preview = row(&buf, 0, 40);
        assert!(preview.contains("●"), "preview shows assistant bullet");
        assert!(preview.contains("Hi there"), "preview shows streamed text");
    }
}

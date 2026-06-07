//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`live_height`], [`repin`], [`cursor_position`]),
//! build ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, HistoryItem, Role, ToolCall, ToolStatus};

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

// --- Tool-call styling. A tool renders as a coloured bullet header
// `● name(args)` plus a collapsed `⎿` peek of its output; the bullet colour is
// the tool's lifecycle (blue running, green ok, red fail). The full output is
// only shown in the Ctrl+O tool-output view, never inline. ---

/// Bullet prefixing a tool call (same glyph as the assistant, recoloured by
/// status — see [`tool_status_color`]).
const TOOL_BULLET: &str = "● ";
/// Prefix for the collapsed result peek line: indent + a turnstile glyph.
const TOOL_RESULT_PREFIX: &str = "  ⎿ ";
/// Prefix for the "+N lines" hint line under a collapsed peek.
const TOOL_MORE_PREFIX: &str = "    … ";
/// Hint telling the user how to see the full output.
const EXPAND_HINT: &str = " (ctrl+o to expand)";

/// Blue — a tool that is still executing.
const TOOL_RUNNING_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// Green — a tool that finished successfully.
const TOOL_OK_COLOR: Color = Color::Rgb(0x98, 0xC3, 0x79);
/// Red — a tool that failed (shares the backend-error red).
const TOOL_FAIL_COLOR: Color = ERROR_COLOR;
/// White — the tool's name.
const TOOL_NAME_COLOR: Color = AI_COLOR;
/// Dim grey — a tool's argument summary and its collapsed peek/hint.
const TOOL_DIM_COLOR: Color = Color::Rgb(0x8A, 0x8A, 0x8A);

// --- Tool-output view (the Ctrl+O full-screen overlay). A one-row title above a
// scrolling body that lists every tool call's *full* output. ---

/// Title shown at the top of the tool-output view.
const TOOL_VIEW_TITLE: &str = "Tool output";
/// Key hint shown beside the title.
const TOOL_VIEW_HINT: &str = "  ↑/↓ PgUp/PgDn scroll · ctrl+o / esc return";
/// Rows of chrome above the scrolling body (just the title row).
const TOOL_VIEW_TITLE_ROWS: u16 = 1;

// --- Live-region geometry. The bottom region's height is dynamic: it grows with
// the wrapped input (see `live_height`). `render_live` and `cursor_position` both
// derive their layout from `input_box` so the drawn text and cursor never drift;
// `main.rs`/`term.rs` size the viewport from `live_height`/`LIVE_MIN_HEIGHT`. ---

/// Rows in the streaming-preview line shown above the input box (the in-progress
/// reply's last, not-yet-committed line). Shown only while a reply streams.
const PREVIEW_ROWS: u16 = 1;
/// A blank gap row between the streaming preview and the box, so the live reply
/// never butts up against the box's top rule. Present only while streaming.
const GAP_ROWS: u16 = 1;
/// The input box's non-text rows: a top rule and a bottom rule.
const INPUT_CHROME_ROWS: u16 = 2;
/// The smallest the live region ever gets: a one-text-row box framed by two
/// rules (idle has no preview strip). `main.rs` sizes the initial viewport from this.
pub const LIVE_MIN_HEIGHT: u16 = INPUT_CHROME_ROWS + 1;

/// Rows of the streaming strip above the box — a preview line plus a blank gap —
/// shown only while a reply streams. Idle, the box sits directly under the chat
/// (separated by the committed blank spacer after the last message), so the strip
/// collapses to nothing and there is exactly one blank line above the box.
const fn strip_rows(streaming: bool) -> u16 {
    if streaming {
        PREVIEW_ROWS + GAP_ROWS
    } else {
        0
    }
}

/// Columns the input field's text occupies: the box spans the full width (no side
/// borders) minus the prompt/indent that prefixes every text row.
fn field_width(width: u16) -> u16 {
    width.saturating_sub(BULLET_WIDTH).max(1)
}

/// Height of the bottom live region for the current `input` at this terminal
/// size: the streaming strip (only while `streaming`), two framing rules, and one
/// row per wrapped input line — so the box **grows** as the message wraps —
/// clamped to the terminal height (after which the box scrolls internally; see
/// [`render_live`]).
#[must_use]
pub fn live_height(input: &str, width: u16, term_height: u16, streaming: bool) -> u16 {
    let rows = wrap_text(input, field_width(width)).len().max(1) as u16;
    (strip_rows(streaming) + INPUT_CHROME_ROWS + rows).min(term_height.max(1))
}

/// How to re-pin the live region when its height changes between draws, keeping
/// it **content-anchored** — its top fixed, like Claude Code / codex. The box
/// grows *downward* in place; the screen only scrolls up when the box would run
/// past the bottom (i.e. it has reached the bottom), and a shrink vacates rows
/// just below it. The pure decision behind `term::InlineViewport::draw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repin {
    /// Scroll the whole screen up this many rows first (0 unless the grown box
    /// overflows the bottom — only then does the chat scroll into scrollback).
    pub scroll_up: u16,
    /// The live region's new top row (unchanged unless it overflowed the bottom).
    pub top: u16,
    /// Rows to blank just below the new region (a shrink vacates them).
    pub clear_below: u16,
}

/// Decide how to re-pin a live region currently at `top` with `old_height` to
/// `new_height` on a `screen_height`-row screen, keeping its top anchored.
#[must_use]
pub fn repin(top: u16, old_height: u16, new_height: u16, screen_height: u16) -> Repin {
    let bottom = u32::from(top) + u32::from(new_height);
    let scroll_up = bottom.saturating_sub(u32::from(screen_height)) as u16;
    let new_top = top.saturating_sub(scroll_up);
    let clear_below = top
        .saturating_add(old_height)
        .saturating_sub(new_top.saturating_add(new_height));
    Repin {
        scroll_up,
        top: new_top,
        clear_below,
    }
}

/// Split `area` into the live region's two stacked sub-areas `[strip, input]`.
/// The strip holds the streaming preview + gap (height 0 when idle); the input
/// box takes whatever rows remain below it, so it **grows** as `area` grows (see
/// [`live_height`]). The only place the split is expressed.
fn live_layout(area: Rect, streaming: bool) -> [Rect; 2] {
    Layout::vertical([
        Constraint::Length(strip_rows(streaming)),
        Constraint::Min(0),
    ])
    .areas(area)
}

/// The geometry shared by [`render_live`] and [`cursor_position`] so the drawn
/// text and the hardware cursor can never drift apart: where the input text rows
/// live, the input wrapped to the field width, and how far it's scrolled so the
/// end (where the cursor always sits) stays visible when the box is full.
struct InputBox {
    /// The rule-framed box area (below the preview); borders are drawn here.
    frame: Rect,
    /// The inner area that holds the text rows (the box minus its two rules).
    text: Rect,
    /// Every wrapped input line (always at least one, possibly empty).
    wrapped: Vec<String>,
    /// Index of the first wrapped line shown — the tail is kept in view.
    scroll: usize,
}

fn input_box(area: Rect, input: &str, streaming: bool) -> InputBox {
    let [_, frame] = live_layout(area, streaming);
    let text = frame.inner(Margin::new(0, 1)); // inset past the top & bottom rules
    let wrapped = wrap_text(input, field_width(area.width));
    let scroll = wrapped.len().saturating_sub(text.height as usize);
    InputBox {
        frame,
        text,
        wrapped,
        scroll,
    }
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

/// Render the bottom live region into `buf`. While a reply streams, the strip's
/// top row previews the in-progress line and the row below it is a blank gap, so
/// the reply never touches the rule-framed, **growing** input box; idle, the
/// strip collapses and the box sits at the top of the region. The input wraps
/// across as many rows as `area` allows; the prompt marks its first line and
/// continuation lines are indented to align under it. When the input is taller
/// than the box, the tail is kept in view (the cursor is always at the end).
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App) {
    let streaming = app.is_streaming();
    let [strip, _] = live_layout(area, streaming);

    // Strip preview (top row; the rest of the strip is the blank gap). A running
    // tool takes precedence — its coloured header (blue) shows what's executing;
    // otherwise the in-progress reply's last line previews. Nothing when idle.
    let preview = if let Some(tool) = app.current_tool() {
        tool_lines(tool, strip.width).into_iter().next()
    } else {
        app.streaming_text().map(|text| {
            message_lines(Role::Assistant, text, strip.width)
                .pop()
                .unwrap_or_default()
        })
    };
    if let Some(preview) = preview {
        let preview_area = Rect {
            height: PREVIEW_ROWS.min(strip.height),
            ..strip
        };
        Paragraph::new(preview).render(preview_area, buf);
    }

    // The input box: a top/bottom rule framing the wrapped input rows.
    let bx = input_box(area, &app.input, streaming);
    let block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_COLOR));
    block.render(bx.frame, buf);

    let lines: Vec<Line> = bx
        .wrapped
        .iter()
        .enumerate()
        .skip(bx.scroll)
        .take(bx.text.height as usize)
        .map(|(i, line)| {
            // The prompt prefixes the real first line; wrapped/continuation lines
            // get a matching-width indent so the text stays aligned under it.
            let (prefix, style) = if i == 0 {
                (PROMPT, Style::new().fg(PROMPT_COLOR))
            } else {
                (INDENT, Style::default())
            };
            Line::from(vec![Span::styled(prefix, style), Span::raw(line.clone())])
        })
        .collect();
    Paragraph::new(lines).render(bx.text, buf);
}

/// The bullet colour for a tool's lifecycle: blue running, green ok, red fail.
const fn tool_status_color(status: ToolStatus) -> Color {
    match status {
        ToolStatus::Running => TOOL_RUNNING_COLOR,
        ToolStatus::Ok => TOOL_OK_COLOR,
        ToolStatus::Failed => TOOL_FAIL_COLOR,
    }
}

/// Truncate `s` to at most `max` display columns (column-aware, so wide glyphs
/// count as two), returning the kept prefix.
fn truncate_cols(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = char_cols(ch);
        if w + cw > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// The coloured bullet header line for a tool: `● name(args)`, the bullet
/// recoloured by lifecycle (blue/green/red). Shared by the inline collapsed view
/// ([`tool_lines`]) and the full-screen view ([`tool_view_lines`]).
fn tool_header(tool: &ToolCall) -> Line<'static> {
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status))
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(TOOL_BULLET.to_string(), bullet_style),
        Span::styled(
            tool.name.clone(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({})", tool.args), Style::new().fg(TOOL_DIM_COLOR)),
    ])
}

/// Build the styled lines for one tool call as shown **inline**: a coloured
/// bullet header `● name(args)`, then a collapsed one-line peek of its output
/// with a `(ctrl+o to expand)` hint when more is hidden. The full output is only
/// rendered in the separate tool-output view, never here.
#[must_use]
pub fn tool_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    let header = tool_header(tool);

    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);

    // Output split into lines (ignoring a single trailing blank from a final
    // newline), so the hidden-line count is accurate.
    let mut out_lines: Vec<&str> = if tool.output.is_empty() {
        Vec::new()
    } else {
        tool.output.split('\n').collect()
    };
    if out_lines.last() == Some(&"") {
        out_lines.pop();
    }

    let peek = match tool.status {
        ToolStatus::Running => "running…".to_string(),
        _ if out_lines.is_empty() => "(no output)".to_string(),
        _ => truncate_cols(out_lines[0], peek_width),
    };
    let mut lines = vec![
        header,
        Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(peek, dim),
        ]),
    ];

    // A hint line whenever output beyond the first peeked line is hidden.
    let hidden = out_lines.len().saturating_sub(1);
    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
            Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
        ]));
    }
    lines
}

/// Build the full content of the tool-output view: every tool call's coloured
/// header followed by its **complete** output (wrapped and indented), with a
/// blank line between tools. This is the expanded counterpart of [`tool_lines`]
/// — nothing is collapsed here. An empty list yields a single placeholder line.
#[must_use]
pub fn tool_view_lines(tools: &[&ToolCall], width: u16) -> Vec<Line<'static>> {
    if tools.is_empty() {
        return vec![Line::from(Span::styled(
            "No tool calls yet.".to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        ))];
    }
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut lines = Vec::new();
    for tool in tools {
        lines.push(tool_header(tool));
        let body = match (tool.status, tool.output.is_empty()) {
            (ToolStatus::Running, true) => vec!["running…".to_string()],
            (_, true) => vec!["(no output)".to_string()],
            _ => wrap_text(&tool.output, content_width),
        };
        for out in body {
            lines.push(Line::from(vec![
                Span::raw(INDENT.to_string()),
                Span::styled(out, dim),
            ]));
        }
        lines.push(Line::default()); // blank line between tools
    }
    lines
}

/// The largest the tool-output scroll offset can be on a `screen_height`-row
/// screen — the total content height minus the scrolling body — so the last
/// line can reach the bottom but not scroll past it. The loop clamps
/// `App::tool_scroll` to this each draw.
#[must_use]
pub fn tool_view_max_scroll(tools: &[&ToolCall], width: u16, screen_height: u16) -> usize {
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS) as usize;
    tool_view_lines(tools, width).len().saturating_sub(body)
}

/// Render the full-screen tool-output view: a title row, then the scrolling body
/// of every tool call's full output, windowed by `App::tool_scroll` (clamped so
/// it can't run past the end). Pure — `term.rs` paints this onto the overlay.
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App) {
    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(TOOL_VIEW_TITLE_ROWS), Constraint::Min(0)])
            .areas(area);

    let tools = app.tool_calls();
    let title = Line::from(vec![
        Span::styled(
            format!("{TOOL_VIEW_TITLE} ({})", tools.len()),
            Style::new().fg(AI_COLOR).add_modifier(Modifier::BOLD),
        ),
        Span::styled(TOOL_VIEW_HINT.to_string(), Style::new().fg(TOOL_DIM_COLOR)),
    ]);
    Paragraph::new(title).render(title_area, buf);

    let lines = tool_view_lines(&tools, body_area.width);
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    Paragraph::new(visible).render(body_area, buf);
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
/// to scrollback: each message's wrapped lines (or each tool call's collapsed
/// peek), with a blank spacer after every item. Used to repaint after a resize
/// clears the screen, or when returning from the tool-output view.
#[must_use]
pub fn conversation_lines(history: &[HistoryItem], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in history {
        match item {
            HistoryItem::Message(m) => lines.extend(message_lines(m.role, &m.text, width)),
            HistoryItem::Tool(t) => lines.extend(tool_lines(t, width)),
        }
        lines.push(Line::default()); // blank spacer after every item
    }
    lines
}

/// The last `max_rows` lines of the conversation — i.e. the tail that fits on
/// screen above the live region. After a width shrink ratatui clears the visible
/// screen (older lines survive in the terminal's own scrollback), so we only
/// need to repaint what was on screen; capping at `max_rows` also avoids
/// re-scrolling content the terminal already kept.
#[must_use]
pub fn repaint_lines(history: &[HistoryItem], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let mut lines = conversation_lines(history, width);
    if lines.len() > max_rows {
        lines = lines.split_off(lines.len() - max_rows);
    }
    lines
}

/// How many history rows fit above a `live_height`-row live region on a
/// `term_height`-row screen — the number of lines to repaint after a resize.
/// Saturates at 0 so a live region taller than the screen can never underflow.
/// The pure counterpart of the terminal calls in `main.rs::repaint_after_resize`.
#[must_use]
pub fn repaint_budget(term_height: u16, live_height: u16) -> usize {
    term_height.saturating_sub(live_height) as usize
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input. Shares [`input_box`] with [`render_live`] so the cursor lands
/// exactly after the visible input text — on the last wrapped row, at its end.
#[must_use]
pub fn cursor_position(area: Rect, input: &str) -> (u16, u16) {
    // The cursor is only ever placed while idle (it is hidden during streaming),
    // so the box is laid out without the streaming strip.
    let bx = input_box(area, input, false);
    // The cursor follows the end of the input: its wrapped row, less the scroll.
    let cursor_line = bx.wrapped.len().saturating_sub(1);
    let row = cursor_line.saturating_sub(bx.scroll) as u16;
    let col = cols(bx.wrapped.last().map_or("", String::as_str)) as u16;
    (bx.text.x + BULLET_WIDTH + col, bx.text.y + row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Message;

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

    // --- tool_lines (collapsed, colour-by-status) ---

    #[test]
    fn tool_lines_header_shows_name_and_args() {
        let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Ok, "a\nb\nc"), 80);
        assert_eq!(plain(&lines[0]), "● Bash(cargo test)");
    }

    #[test]
    fn tool_lines_colours_the_bullet_by_status() {
        for (status, color) in [
            (ToolStatus::Running, TOOL_RUNNING_COLOR),
            (ToolStatus::Ok, TOOL_OK_COLOR),
            (ToolStatus::Failed, TOOL_FAIL_COLOR),
        ] {
            let lines = tool_lines(&tool("X", "y", status, "out"), 80);
            assert_eq!(
                lines[0].spans[0].style.fg,
                Some(color),
                "bullet colour tracks status {status:?}"
            );
        }
    }

    #[test]
    fn tool_lines_collapses_multiline_output_to_a_peek_plus_expand_hint() {
        let lines = tool_lines(&tool("Read", "f", ToolStatus::Ok, "one\ntwo\nthree"), 80);
        assert_eq!(lines.len(), 3, "header + peek + hint");
        let peek = plain(&lines[1]);
        assert!(
            peek.contains("one"),
            "peek shows the first output line: {peek:?}"
        );
        assert!(!peek.contains("two"), "the rest is hidden inline: {peek:?}");
        let hint = plain(&lines[2]);
        assert!(
            hint.contains("+2 lines"),
            "hint counts hidden lines: {hint:?}"
        );
        assert!(
            hint.contains("ctrl+o to expand"),
            "hint mentions ctrl+o: {hint:?}"
        );
    }

    #[test]
    fn tool_lines_single_line_output_has_no_expand_hint() {
        let lines = tool_lines(&tool("Bash", "echo hi", ToolStatus::Ok, "hi"), 80);
        assert_eq!(lines.len(), 2, "header + peek only, nothing hidden");
        assert!(plain(&lines[1]).contains("hi"));
    }

    #[test]
    fn tool_lines_running_shows_a_running_peek() {
        let lines = tool_lines(&tool("Bash", "sleep 1", ToolStatus::Running, ""), 80);
        assert_eq!(lines.len(), 2);
        assert!(
            plain(&lines[1]).to_lowercase().contains("running"),
            "a running tool peeks as running: {:?}",
            plain(&lines[1])
        );
    }

    #[test]
    fn tool_lines_truncates_a_long_peek_to_the_width() {
        // A peek line never overflows the terminal width (column-aware).
        let long = "x".repeat(200);
        let lines = tool_lines(&tool("Bash", "y", ToolStatus::Ok, &long), 30);
        for line in &lines {
            assert!(cols(&plain(line)) <= 30, "no line exceeds the width");
        }
    }

    // --- tool-output view (the Ctrl+O full-screen overlay) ---

    #[test]
    fn tool_view_lines_shows_each_tools_full_output() {
        let t1 = tool("Read", "f", ToolStatus::Ok, "l1\nl2\nl3");
        let t2 = tool("Bash", "b", ToolStatus::Failed, "err");
        let texts: Vec<String> = tool_view_lines(&[&t1, &t2], 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t == "● Read(f)"), "{texts:?}");
        // The *full* output is present — every line, not collapsed.
        for needle in ["l1", "l2", "l3"] {
            assert!(
                texts.iter().any(|t| t.contains(needle)),
                "missing {needle:?} in {texts:?}"
            );
        }
        assert!(texts.iter().any(|t| t == "● Bash(b)"));
        assert!(texts.iter().any(|t| t.contains("err")));
    }

    #[test]
    fn tool_view_lines_colours_the_header_by_status() {
        let t = tool("Read", "f", ToolStatus::Ok, "x");
        let lines = tool_view_lines(&[&t], 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_OK_COLOR));
    }

    #[test]
    fn tool_view_lines_of_no_tools_is_a_placeholder() {
        let lines = tool_view_lines(&[], 80);
        assert!(
            plain(&lines[0]).to_lowercase().contains("no tool"),
            "{:?}",
            plain(&lines[0])
        );
    }

    #[test]
    fn render_tool_view_shows_a_title_and_the_full_output() {
        let mut app = App::new();
        app.start_tool("Read", "src/main.rs");
        app.end_tool("alpha\nbeta", true);
        app.view = crate::app::View::ToolOutput;
        let mut buf = buffer(40, 10);
        render_tool_view(buf.area, &mut buf, &app);
        let all: String = (0..10)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Tool output"), "title present: {all:?}");
        assert!(all.contains("Read(src/main.rs)"), "{all:?}");
        assert!(all.contains("alpha") && all.contains("beta"), "{all:?}");
    }

    #[test]
    fn render_tool_view_scrolls_past_the_top() {
        let mut app = App::new();
        app.start_tool("Read", "f");
        let output = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.end_tool(&output, true);
        app.view = crate::app::View::ToolOutput;
        app.tool_scroll = 8;
        let mut buf = buffer(40, 8);
        render_tool_view(buf.area, &mut buf, &app);
        let all: String = (0..8)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !all.contains("line0"),
            "scrolled past the first line: {all:?}"
        );
        assert!(all.contains("line"), "still shows some output: {all:?}");
    }

    #[test]
    fn tool_view_max_scroll_is_total_lines_minus_the_body() {
        let output = (0..20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tool("Read", "f", ToolStatus::Ok, &output);
        let total = tool_view_lines(&[&t], 40).len();
        let screen_h = 10u16;
        let body = (screen_h - TOOL_VIEW_TITLE_ROWS) as usize;
        assert_eq!(
            tool_view_max_scroll(&[&t], 40, screen_h),
            total.saturating_sub(body)
        );
    }

    // --- render_live (against a plain Buffer) ---

    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn render_live_grows_the_box_and_wraps_input_across_rows() {
        let mut app = App::new();
        app.input = "first\nsecond".to_string();
        let h = live_height(&app.input, 20, 24, false);
        assert_eq!(h, 4, "two rules + two input rows (no strip when idle)");
        let mut buf = buffer(20, h);
        render_live(buf.area, &mut buf, &app);

        assert_eq!(buf[(0, 0)].symbol(), "─", "top rule");
        assert!(row(&buf, 1, 20).contains("❯ first"), "prompt on first line");
        assert!(
            row(&buf, 2, 20).contains("second") && !row(&buf, 2, 20).contains("❯"),
            "continuation line is indented, no prompt"
        );
        assert_eq!(
            buf[(0, 3)].symbol(),
            "─",
            "bottom rule moved down as box grew"
        );
    }

    #[test]
    fn render_live_scrolls_input_to_keep_the_end_visible() {
        // Six input lines but a terminal that only fits four text rows: the box
        // shows the tail (so the cursor's line stays visible), not the head.
        let mut app = App::new();
        app.input = (0..6)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let term_h = 6; // live clamps to 6 → text rows = 6 - 2 = 4
        assert_eq!(live_height(&app.input, 20, term_h, false), 6);
        let mut buf = buffer(20, 6);
        render_live(buf.area, &mut buf, &app);

        let text: String = (1..5).map(|y| row(&buf, y, 20)).collect();
        assert!(text.contains("line5"), "last line is visible: {text:?}");
        assert!(!text.contains("line0"), "first line scrolled off: {text:?}");
    }

    #[test]
    fn render_live_separates_the_streaming_preview_from_the_box_with_a_blank_gap() {
        // While streaming, the preview line must not butt up against the box: a
        // blank gap row sits between the previewed reply and the top rule.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("streaming reply");
        let mut buf = buffer(40, 5); // preview + gap + (two rules + one input row)
        render_live(buf.area, &mut buf, &app);

        assert!(
            row(&buf, 0, 40).contains("streaming reply"),
            "preview on row 0"
        );
        assert!(
            row(&buf, 1, 40).trim().is_empty(),
            "blank gap row below the preview"
        );
        assert_eq!(
            buf[(0, 2)].symbol(),
            "─",
            "top rule below the gap, not touching the preview"
        );
    }

    #[test]
    fn render_live_has_no_strip_when_idle() {
        // Idle, the box sits directly under the chat — no preview/gap strip — so
        // the only separation is the committed blank after the last message.
        let mut app = App::new();
        app.input = "hello".to_string();
        let mut buf = buffer(40, 3); // just the box: two rules + one input row
        render_live(buf.area, &mut buf, &app);

        assert_eq!(
            buf[(0, 0)].symbol(),
            "─",
            "top rule on row 0 — no preview strip above it"
        );
        assert!(row(&buf, 1, 40).contains("❯ hello"), "input on row 1");
        assert_eq!(buf[(0, 2)].symbol(), "─", "bottom rule on row 2");
    }

    #[test]
    fn cursor_sits_on_the_last_wrapped_input_row() {
        // "ab\ncd" → two rows; the cursor follows the end onto the second text
        // row (y = 2) just after the indented "cd" (x = 2 + 2).
        let area = Rect::new(0, 0, 20, live_height("ab\ncd", 20, 24, false));
        assert_eq!(cursor_position(area, "ab\ncd"), (4, 2));
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
        // Idle (the only time the cursor shows): the box fills the area, so on a
        // 40x4 area the text row sits at row 1, flush-left (no side border).
        // Empty input → cursor right after "❯ ".
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), ""), (2, 1));
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), "hi"), (4, 1));
    }

    // --- growing input box: height + re-pin geometry ---

    #[test]
    fn live_height_is_minimal_for_short_input() {
        // Idle, empty or one-line input → a one-row box framed by two rules
        // (no preview strip) = LIVE_MIN_HEIGHT (3).
        assert_eq!(LIVE_MIN_HEIGHT, 3);
        assert_eq!(live_height("", 40, 24, false), LIVE_MIN_HEIGHT);
        assert_eq!(live_height("hi", 40, 24, false), LIVE_MIN_HEIGHT);
    }

    #[test]
    fn live_height_adds_the_streaming_strip_above_the_box() {
        // While streaming, the live region gains a preview row + a blank gap row
        // (PREVIEW_ROWS + GAP_ROWS = 2) above whatever the idle box would be.
        for input in ["", "hi", "a\nb\nc"] {
            assert_eq!(
                live_height(input, 40, 24, true),
                live_height(input, 40, 24, false) + 2,
                "streaming adds exactly the preview + gap rows for {input:?}"
            );
        }
    }

    #[test]
    fn live_height_grows_one_row_per_wrapped_input_line() {
        // Idle, three explicit lines → the box has three text rows, so the live
        // region is 2 (two rules) + 3 = 5 rows tall.
        assert_eq!(live_height("a\nb\nc", 40, 24, false), 5);
    }

    #[test]
    fn live_height_grows_when_a_long_line_soft_wraps() {
        // No explicit newline: a line longer than the field width wraps and the
        // box still grows. field width = 10 - 2 = 8, so 16 columns → 2 rows → 4.
        assert_eq!(live_height("abcdefghijklmnop", 10, 24, false), 4);
    }

    #[test]
    fn live_height_is_clamped_to_the_terminal_height() {
        let many = "a\n".repeat(50);
        assert_eq!(
            live_height(&many, 40, 10, false),
            10,
            "never taller than the screen"
        );
    }

    #[test]
    fn repin_keeps_the_box_top_anchored_growing_downward() {
        // Room below: grow in place, top fixed, no scroll, nothing to clear.
        assert_eq!(
            repin(3, 4, 6, 24),
            Repin {
                scroll_up: 0,
                top: 3,
                clear_below: 0
            }
        );
        // Unchanged height that already fits is a no-op.
        assert_eq!(
            repin(10, 5, 5, 24),
            Repin {
                scroll_up: 0,
                top: 10,
                clear_below: 0
            }
        );
    }

    #[test]
    fn repin_clears_below_on_a_shrink_and_leaves_the_top_put() {
        // Shrinking pulls the bottom up; the two vacated rows below get blanked.
        assert_eq!(
            repin(3, 6, 4, 24),
            Repin {
                scroll_up: 0,
                top: 3,
                clear_below: 2
            }
        );
    }

    #[test]
    fn repin_scrolls_up_only_when_the_box_overflows_the_bottom() {
        // At the bottom (top 20 + new height 6 = 26 > 24): scroll up 2 and pin.
        assert_eq!(
            repin(20, 4, 6, 24),
            Repin {
                scroll_up: 2,
                top: 18,
                clear_below: 0
            }
        );
    }

    // --- live-region layout: single source of truth ---

    #[test]
    fn live_layout_splits_the_area_into_the_strip_plus_the_rest() {
        // The strip takes its rows (preview + gap while streaming, none when
        // idle); the input box takes everything left, so the two always tile the
        // whole area — at the minimum height and beyond, streaming or not.
        for streaming in [false, true] {
            for h in [LIVE_MIN_HEIGHT, 9, 20] {
                let [strip, input] = live_layout(Rect::new(0, 0, 40, h), streaming);
                assert_eq!(strip.height + input.height, h, "sub-areas tile the area");
                assert_eq!(strip.height, strip_rows(streaming));
            }
        }
    }

    #[test]
    fn cursor_row_sits_on_the_rendered_prompt_row() {
        // render_live and cursor_position both derive their geometry from
        // input_box, so the hardware cursor lands on exactly the row where the
        // prompt is drawn — they cannot drift apart.
        let area = Rect::new(0, 0, 40, LIVE_MIN_HEIGHT);
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
        assert_eq!(
            repaint_budget(10, LIVE_MIN_HEIGHT),
            10 - LIVE_MIN_HEIGHT as usize
        );
        assert_eq!(
            repaint_budget(10, 9),
            1,
            "a taller live region leaves fewer rows"
        );
        assert_eq!(repaint_budget(LIVE_MIN_HEIGHT, LIVE_MIN_HEIGHT), 0);
        assert_eq!(repaint_budget(4, 9), 0, "saturates, never wraps");
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

    fn msg(role: Role, text: &str) -> HistoryItem {
        HistoryItem::Message(Message {
            role,
            text: text.to_string(),
        })
    }

    fn tool(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status,
            output: output.to_string(),
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
    fn conversation_lines_renders_a_tool_call_between_messages() {
        let history = [
            msg(Role::User, "hi"),
            HistoryItem::Tool(tool("Bash", "ls", ToolStatus::Ok, "a\nb")),
            msg(Role::Assistant, "done"),
        ];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // user, blank, tool header, tool peek, tool hint, blank, assistant, blank
        assert_eq!(texts.first().map(String::as_str), Some("❯ hi"));
        assert!(
            texts.iter().any(|t| t == "● Bash(ls)"),
            "tool header is present in order: {texts:?}"
        );
        assert!(texts.iter().any(|t| t == "● done"));
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

    #[test]
    fn render_live_previews_a_running_tool_in_blue() {
        // While a tool runs, the strip's preview row shows its coloured header
        // (blue) instead of the assistant text, so the user sees what's executing.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Read", "src/main.rs");
        let mut buf = buffer(40, 5);
        render_live(buf.area, &mut buf, &app);

        let preview = row(&buf, 0, 40);
        assert!(
            preview.contains("Read(src/main.rs)"),
            "preview shows the running tool header: {preview:?}"
        );
        assert_eq!(
            buf[(0, 0)].fg,
            TOOL_RUNNING_COLOR,
            "the running tool's bullet is blue"
        );
    }
}

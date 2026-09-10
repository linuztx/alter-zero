//! The Ctrl+D view's **classifier page** (Tab): the bounded task context
//! auto mode's classifier reads before every command and MCP call, under
//! the rubric it judges by. Its sibling page answers *what does the model
//! see*; this one answers *what does the reviewer see*. Same chrome, the
//! same `system prompt:` / `user:` tags over the same verbatim text — only
//! the lines differ, so this module is just the body builder and
//! [`super::render_context_view`] paints either page. See
//! `docs/permissions.md`.

use super::theme::*;
use super::wrap::{cols, ellipsize, wrap_verbatim};
use super::*;
use crate::context::ContextRole;
use crate::llm::classifier::ACTION_TO_REVIEW_HEADER;
use crate::permission::PermissionMode;

/// The dim note above the block: whether the log the view is showing is
/// actually being consulted. It is recorded in **every** mode (the boundary
/// feeds it per call, not per verdict), so a silent view would read as "the
/// classifier is deciding this" in modes where the user is.
const fn mode_note(mode: Option<PermissionMode>) -> &'static str {
    match mode {
        Some(PermissionMode::Auto) => CLASSIFIER_VIEW_NOTE_AUTO,
        Some(_) => CLASSIFIER_VIEW_NOTE_INACTIVE,
        None => CLASSIFIER_VIEW_NOTE_OFF,
    }
}

/// One row of a prompt abridged by [`abridge_prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PromptRow {
    /// A blank separator — one before each heading after the first row, so
    /// the sections read apart the way they do in the file.
    Blank,
    /// A line of the prompt: a heading, or a line of a section's opening
    /// paragraph — cut at the budget and closed with `…` when it overflowed.
    Text(String),
    /// How many non-blank lines of the section fold away here, painted as
    /// the dim `… +N lines` row.
    Elided(usize),
}

/// A markdown heading line — any `#`-run followed by a space (`#hashtag` is
/// not one).
fn is_heading(line: &str) -> bool {
    let rest = line.trim_start_matches('#');
    rest.len() < line.len() && rest.starts_with(' ')
}

/// Abridge a markdown prompt to its **structure**. The classifier's system
/// prompt is 3.6 KB of rubric — shown whole it is sixty rows above the live
/// block the page exists to show — but what the reviewer is *told* is half
/// of what it sees, so the page shows the prompt's shape instead: every
/// heading whole (the sections are the structure), each section's opening
/// paragraph kept up to `peek` display columns — whole lines while they fit,
/// the overflowing line cut with `…` — and the rest of the section folded
/// into one counted [`PromptRow::Elided`] row, so nothing vanishes silently.
/// Blank lines are separators, not content: they end the opening paragraph
/// and are neither shown nor counted. Text before the first heading is a
/// section of its own; a prompt with no headings is one section. Pure —
/// unit-tested.
pub(super) fn abridge_prompt(prompt: &str, peek: usize) -> Vec<PromptRow> {
    // Sections: the preamble (no heading), then one per heading line.
    let mut sections: Vec<Vec<&str>> = vec![Vec::new()];
    for line in prompt.lines() {
        if is_heading(line) {
            sections.push(Vec::new());
        }
        sections.last_mut().expect("seeded").push(line);
    }
    let mut rows = Vec::new();
    for section in sections {
        let mut lines = section.into_iter().peekable();
        let heading = lines.next_if(|line| is_heading(line));
        let mut budget = peek;
        let mut in_opening = true;
        let mut shown = Vec::new();
        let mut hidden = 0;
        for line in lines.skip_while(|line| line.trim().is_empty()) {
            if line.trim().is_empty() {
                in_opening = false;
                continue;
            }
            if in_opening && budget > 0 {
                let width = cols(line);
                if width <= budget {
                    shown.push(PromptRow::Text(line.to_string()));
                    budget -= width;
                } else {
                    shown.push(PromptRow::Text(ellipsize(line, budget)));
                    budget = 0;
                }
            } else {
                hidden += 1;
            }
        }
        // An empty preamble (a prompt that opens on its first heading) is
        // not a section.
        if heading.is_none() && shown.is_empty() && hidden == 0 {
            continue;
        }
        if !rows.is_empty() {
            rows.push(PromptRow::Blank);
        }
        if let Some(heading) = heading {
            rows.push(PromptRow::Text(heading.to_string()));
        }
        rows.extend(shown);
        if hidden > 0 {
            rows.push(PromptRow::Elided(hidden));
        }
    }
    rows
}

/// A coloured `role:` tag — wrapped on narrow screens like its body.
fn tag_lines(tag: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    wrap_verbatim(tag, width)
        .into_iter()
        .map(|row| Line::from(Span::styled(row, Style::new().fg(color))))
        .collect()
}

/// The classifier page's body: the mode note, then the request the next
/// verdict is judged in — the classifier's **system prompt** under the
/// sibling page's amber tag, abridged to its structure (`abridge_prompt`:
/// headings whole, opening paragraphs cut at `CLASSIFIER_PROMPT_PEEK_COLS`,
/// the rest counted into dim `… +N lines` rows), then the rendered task
/// context under a `user:` tag exactly as
/// [`crate::llm::classifier::ClassifierContext::render`] built it — wrapped
/// **verbatim** (never the markdown renderer: the `##` headers and the `> `
/// quoting are the block's own structure, and the whole point is showing the
/// unformatted text the classifier is sent) — closed by the
/// `## Action to review` header over a dim placeholder, since the action is
/// the one part of the message that is only known when a verdict is asked.
/// No prompt section when none was injected (the dummy keeps no
/// classifier), and a dim placeholder in the block's place when nothing has
/// been recorded — or when the backend keeps no log at all (the dummy, whose
/// offline auto-mode demo answers from a pure heuristic instead).
#[must_use]
pub fn classifier_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let dim = Style::new().fg(tool_dim_color());
    // Keep room for a wide glyph; an indent must never consume the entire
    // width, since wrap_verbatim treats zero as "do not wrap".
    let indent = if usize::from(width) >= cols(CONTEXT_INDENT) + 2 {
        CONTEXT_INDENT
    } else {
        ""
    };
    let text_width = width - cols(indent) as u16;
    let indented = |row: String| Line::from(format!("{indent}{row}"));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in wrap_verbatim(mode_note(app.permission_mode()), width) {
        lines.push(Line::from(Span::styled(row, dim)));
    }
    lines.push(Line::default());
    if let Some(prompt) = app
        .classifier_system_prompt()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        lines.extend(tag_lines(
            CONTEXT_SYSTEM_PROMPT_TAG,
            context_system_color(),
            width,
        ));
        for row in abridge_prompt(prompt, CLASSIFIER_PROMPT_PEEK_COLS) {
            match row {
                PromptRow::Blank => lines.push(Line::default()),
                PromptRow::Text(text) => {
                    lines.extend(wrap_verbatim(&text, text_width).into_iter().map(indented));
                }
                PromptRow::Elided(hidden) => {
                    let marker = format!("{CLASSIFIER_PROMPT_MORE_PREFIX}+{hidden} lines");
                    for row in wrap_verbatim(&marker, text_width) {
                        lines.push(Line::from(Span::styled(format!("{indent}{row}"), dim)));
                    }
                }
            }
        }
        lines.push(Line::default());
    }
    let block = app.classifier_context().map(str::trim).unwrap_or_default();
    if block.is_empty() {
        for row in wrap_verbatim(CLASSIFIER_VIEW_EMPTY, width) {
            lines.push(Line::from(Span::styled(row, dim)));
        }
        return lines;
    }
    lines.extend(tag_lines(
        &format!("{}:", ContextRole::User.wire_name()),
        context_user_color(),
        width,
    ));
    lines.extend(wrap_verbatim(block, text_width).into_iter().map(indented));
    lines.push(Line::default());
    lines.extend(
        wrap_verbatim(ACTION_TO_REVIEW_HEADER, text_width)
            .into_iter()
            .map(indented),
    );
    for row in wrap_verbatim(CLASSIFIER_ACTION_PLACEHOLDER, text_width) {
        lines.push(Line::from(Span::styled(format!("{indent}{row}"), dim)));
    }
    lines
}

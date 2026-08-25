//! The Ctrl+G classifier-context overlay: the bounded task context auto
//! mode's classifier reads before every command and MCP call. The Ctrl+D
//! view's sibling — same pager chrome, same verbatim body — over a different
//! question: not *what does the model see*, but *what does the reviewer
//! see*. See `docs/permissions.md`.

use super::theme::*;
use super::transcript::{overlay_header, tool_view_separator};
use super::wrap::wrap_verbatim;
use super::*;
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

/// The Ctrl+G body: the mode note, then the rendered turn context exactly as
/// [`crate::llm::classifier::ClassifierContext::render`] built it — wrapped
/// **verbatim** (never the markdown renderer: the `##` headers and the `> `
/// quoting are the block's own structure, and the whole point is showing the
/// unformatted text the classifier is sent). A dim placeholder when nothing
/// has been recorded — or when the backend keeps no log at all (the dummy,
/// whose offline auto-mode demo answers from a pure heuristic instead).
#[must_use]
pub fn classifier_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in wrap_verbatim(mode_note(app.permission_mode()), width) {
        lines.push(Line::from(Span::styled(row, dim)));
    }
    lines.push(Line::default());
    let block = app.classifier_context().map(str::trim).unwrap_or_default();
    if block.is_empty() {
        lines.push(Line::from(Span::styled(CLASSIFIER_VIEW_EMPTY, dim)));
        return lines;
    }
    for row in wrap_verbatim(block, width) {
        lines.push(Line::from(row));
    }
    lines
}

/// Render the full-screen Ctrl+G view — the transcript pager's chrome over
/// `lines` (built by [`classifier_lines`]), windowed by
/// `App::classifier_scroll` (clamped) with `~` filler past the end. Pure —
/// `term.rs` paints this onto the overlay.
pub fn render_classifier_view(area: Rect, buf: &mut Buffer, app: &App, lines: &[Line<'static>]) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(overlay_header(CLASSIFIER_VIEW_TITLE, area.width)).render(title_area, buf);

    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.classifier_scroll.min(max);
    let mut visible: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .cloned()
        .collect();
    while (visible.len() as u16) < body_area.height {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(tool_view_separator(area.width, scroll, max)).render(sep_area, buf);

    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(CLASSIFIER_VIEW_HINT_QUIT.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

//! The Ctrl+D view's **classifier page** (Tab): the bounded task context
//! auto mode's classifier reads before every command and MCP call. Its
//! sibling page answers *what does the model see*; this one answers *what
//! does the reviewer see*. Same chrome, same verbatim body — only the lines
//! differ, so this module is just the body builder and
//! [`super::render_context_view`] paints either page. See
//! `docs/permissions.md`.

use super::theme::*;
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

/// The classifier page's body: the mode note, then the rendered task context exactly as
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

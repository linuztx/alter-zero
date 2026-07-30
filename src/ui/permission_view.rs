//! The inline tool-permission prompt: a modal that replaces the whole live
//! region while a `write`/`edit`/`bash` call waits on the user.
//! See `docs/permissions.md`.
//!
//! One builder ([`permission_lines`]) produces the rows; the renderer paints
//! them and [`permission_height`](super::layout::permission_height) reserves
//! exactly that many, so the two can never drift.

use crate::permission::{
    PermissionKind, PermissionRequest, command_scope, hints, options, question, title,
};

use super::file_cell::numbered_body_lines;
use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;

/// A full-width rule in the input box's border colour — the prompt's frame.
fn rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_RULE.repeat(width as usize),
        Style::new().fg(BORDER_COLOR),
    ))
}

/// The dim dashed rule that frames a file change's numbered body.
fn body_rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_BODY_RULE.repeat(width as usize),
        Style::new().fg(PERMISSION_BODY_RULE_COLOR),
    ))
}

/// A one-space-inset row of `text` in `color`, truncated to the width.
fn text_row(text: &str, color: Color, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(PERMISSION_INDENT))
        .max(1);
    Line::from(vec![
        Span::raw(PERMISSION_INDENT),
        Span::styled(truncate_cols(text, room), Style::new().fg(color)),
    ])
}

/// The title row: the coloured action, plus a dim ` · from the {type} agent`
/// when a subagent raised the request.
fn title_row(request: &PermissionRequest, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::raw(PERMISSION_INDENT),
        Span::styled(
            title(request.kind).to_string(),
            Style::new()
                .fg(PERMISSION_TITLE_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(agent) = &request.agent {
        let text = format!("{PERMISSION_AGENT_SEPARATOR}{agent}{PERMISSION_AGENT_SUFFIX}");
        let room = (width as usize)
            .saturating_sub(cols(PERMISSION_INDENT) + cols(title(request.kind)))
            .max(1);
        spans.push(Span::styled(
            truncate_cols(&text, room),
            Style::new().fg(PERMISSION_AGENT_COLOR),
        ));
    }
    Line::from(spans)
}

/// One option row: `❯ 1. Yes` when highlighted (the whole row in the accent
/// colour), `  1. Yes` otherwise.
fn option_row(index: usize, label: &str, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected {
        PERMISSION_MARKER.to_string()
    } else {
        " ".repeat(cols(PERMISSION_MARKER))
    };
    let text = format!("{}. {label}", index + 1);
    let room = (width as usize)
        .saturating_sub(cols(PERMISSION_INDENT) + cols(&marker))
        .max(1);
    let style = if selected {
        Style::new().fg(PERMISSION_SELECTED_COLOR)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::raw(PERMISSION_INDENT),
        Span::styled(marker, style),
        Span::styled(truncate_cols(&text, room), style),
    ])
}

/// The hint row under the options — `{key}{label}` pairs joined by ` · `, keys
/// in the accent colour.
fn hint_row(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(PERMISSION_INDENT)];
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                PERMISSION_HINT_SEPARATOR,
                Style::new().fg(PERMISSION_HINT_TEXT_COLOR),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(PERMISSION_HINT_KEY_COLOR),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(PERMISSION_HINT_TEXT_COLOR),
        ));
    }
    Line::from(spans)
}

/// The width available to the amend field's text — the region minus the
/// inset and the `❯ ` prompt. Shared with [`cursor_position`].
pub(super) fn amend_field_width(width: u16) -> u16 {
    width
        .saturating_sub((cols(PERMISSION_INDENT) + cols(PERMISSION_MARKER)) as u16)
        .max(1)
}

/// Tab's amend field in place of the option rows: the composer's wrapped rows
/// behind a cyan `❯ ` prompt (continuations indented under it).
fn amend_rows(input: &TextArea, width: u16) -> Vec<Line<'static>> {
    let field = amend_field_width(width);
    input
        .display_rows(field)
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let prefix = if i == 0 {
                PERMISSION_MARKER.to_string()
            } else {
                " ".repeat(cols(PERMISSION_MARKER))
            };
            Line::from(vec![
                Span::raw(PERMISSION_INDENT),
                Span::styled(prefix, Style::new().fg(PERMISSION_SELECTED_COLOR)),
                Span::raw(text),
            ])
        })
        .collect()
}

/// The `bash` prompt's body: the command (word-wrapped, spaces preserved) over
/// the model's own description, both extra-indented.
fn command_rows(
    request: &PermissionRequest,
    width: u16,
    budget: usize,
) -> (Vec<Line<'static>>, usize) {
    let indent = PERMISSION_COMMAND_INDENT;
    let room = width.saturating_sub(cols(indent) as u16).max(1);
    let mut source: Vec<(String, Color)> = wrap_output(&request.target, room)
        .into_iter()
        .map(|row| (row, PERMISSION_TARGET_COLOR))
        .collect();
    if let Some(detail) = request
        .detail
        .as_ref()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
    {
        source.extend(
            wrap_output(detail, room)
                .into_iter()
                .map(|row| (row, PERMISSION_DETAIL_COLOR)),
        );
    }
    let hidden = source.len().saturating_sub(budget.max(1));
    let shown = source.len() - hidden;
    let lines = source
        .into_iter()
        .take(shown)
        .map(|(text, color)| {
            Line::from(vec![
                Span::raw(indent),
                Span::styled(text, Style::new().fg(color)),
            ])
        })
        .collect();
    (lines, hidden)
}

/// The dim `… +N lines` tail appended when the body did not fit the terminal.
fn more_row(hidden: usize, width: u16) -> Line<'static> {
    text_row(
        &format!("… +{hidden} line{}", if hidden == 1 { "" } else { "s" }),
        TOOL_DIM_COLOR,
        width,
    )
}

/// Every row of the open permission prompt, top rule to bottom rule, at
/// `width` on a `term_height`-row terminal.
///
/// The body — a `write`'s numbered contents, an `edit`'s numbered diff hunks,
/// a `bash` command and its description — is shown **whole**: the point of the
/// prompt is that you read what you are approving. It is capped only when the
/// prompt would not otherwise fit, and then by exactly enough that the
/// question, the options, and the hint row stay on screen — the budget is
/// whatever the terminal has left once the rows *around* the body are built,
/// so it stays right as those rows change (Tab's amend field is taller than
/// three options) — with a `… +N lines` tail saying what was left out.
///
/// A capped prompt is padded to fill `term_height` exactly. That is what keeps
/// [`permission_height`](super::layout::permission_height) — which sizes the
/// region from the *terminal* — and [`render_permission`] — which only ever
/// sees the sized *region* — in agreement: at both heights the builder is a
/// fixpoint, so the rows reserved are the rows painted.
///
/// Returns an empty vec when no prompt is open, so the callers can treat "no
/// prompt" and "no rows" alike.
#[must_use]
pub fn permission_lines(app: &App, width: u16, term_height: u16) -> Vec<Line<'static>> {
    let Some(prompt) = app.permission() else {
        return Vec::new();
    };
    let request = &prompt.request;
    let file_change = request.kind != PermissionKind::Bash;

    // Everything above the body: the frame, the title, and (for a file change)
    // the path it targets — a `bash` prompt gaps instead, its command being the
    // body.
    let mut out = vec![rule(width), Line::default(), title_row(request, width)];
    if file_change {
        out.push(text_row(&request.target, PERMISSION_TARGET_COLOR, width));
    } else {
        out.push(Line::default());
    }

    // Everything below it, built now so its height is known: the standing
    // notice (commands only), the question, the options — or Tab's amend field,
    // which is as tall as the feedback typed — the hint row, and the closing
    // frame.
    let mut below = Vec::new();
    if !file_change {
        below.push(Line::default());
        below.push(text_row(PERMISSION_NOTICE, PERMISSION_NOTICE_COLOR, width));
        below.push(Line::default());
    }
    below.push(text_row(&question(request), Color::Reset, width));
    if prompt.amend {
        below.extend(amend_rows(&app.input, width));
        below.push(Line::default());
        below.push(hint_row(PERMISSION_AMEND_HINTS));
    } else {
        for (i, label) in options(request).iter().enumerate() {
            below.push(option_row(i, label, i == prompt.selected, width));
        }
        below.push(Line::default());
        below.push(hint_row(&hints(request)));
    }

    // The body's row budget: whatever the terminal has left once those rows
    // (and, for a file change, its two dashed rules, plus the trailing gap and
    // rule) are accounted for. Zero means the terminal is too short for a
    // preview at all — the body and its rules drop out entirely rather than
    // pushing the options off screen.
    let framing = if file_change { 2 } else { 0 };
    let budget = usize::from(term_height).saturating_sub(
        out.len() + below.len() + framing + 2, /* the gap + rule */
    );

    let mut capped = false;
    if budget > 0 {
        let (body, hidden) = if file_change {
            numbered_body_lines(
                &request.body,
                file_lang(&request.target),
                request.kind == PermissionKind::Edit,
                cols(PERMISSION_INDENT),
                width,
                if body_fits(&request.body, budget) {
                    budget
                } else {
                    budget.saturating_sub(1) // room for the `… +N lines` tail
                },
            )
        } else {
            command_rows(request, width, budget)
        };
        capped = hidden > 0;
        if file_change {
            out.push(body_rule(width));
        }
        out.extend(body);
        if hidden > 0 {
            out.push(more_row(hidden, width));
        }
        if file_change {
            out.push(body_rule(width));
        }
    }

    out.extend(below);
    // A capped body means the prompt is meant to fill the terminal: pad the
    // shortfall (whole source rows can leave a row or two unused) so the count
    // is exactly `term_height` and the builder is a fixpoint — see above.
    if capped {
        let target = usize::from(term_height).saturating_sub(2); // the blank + rule below
        while out.len() < target {
            out.push(Line::default());
        }
    }
    out.push(Line::default());
    out.push(rule(width));
    out
}

/// A cheap "does the body fit" check so a body that exactly fills the budget
/// isn't shortened to make room for a `… +0 lines` tail that isn't needed. It
/// counts *source* rows, an under-estimate of display rows for wrapped
/// content — which only ever errs toward reserving the tail row.
fn body_fits(body: &str, budget: usize) -> bool {
    body.lines().count() <= budget
}

/// The highlight language for the preview — the target path's extension.
fn file_lang(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
}

/// Paint the open permission prompt over the whole live region. Pure —
/// [`render_live`] calls this in place of the composer.
pub fn render_permission(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(permission_lines(app, area.width, area.height)).render(area, buf);
}

/// The `bash` "don't ask again" label, re-exported for the boundary's toast
/// after an [`crate::permission::PermissionDecision::ApproveAlways`].
#[must_use]
pub fn permission_remember_label(request: &PermissionRequest) -> String {
    match request.kind {
        PermissionKind::Bash => command_scope(&request.target).label().to_string(),
        _ => "all edits".to_string(),
    }
}

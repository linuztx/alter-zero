//! The `/trust` review menu's inline view (`docs/project-config.md`) — the
//! hooks menu's sibling: a content-driven framed body ([`trust_view_lines`]
//! builds it line by line, so the height falls out as `lines.len()` and
//! [`trust_menu_height`] / [`render_trust_menu`] can never disagree), showing
//! the project root, each config file with **verbatim** what approving it
//! would let run, and the option rows Enter applies.

use super::model_view::model_rule;
use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_text};
use super::*;

use crate::app::TrustMenu;
use crate::trust::{TrustAction, TrustFileReview};

/// A `MODEL_INDENT`-inset single line in `style`, truncated to the width.
fn line(text: &str, style: Style, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(truncate_cols(text, room), style),
    ])
}

fn dim_line(text: &str, width: u16) -> Line<'static> {
    line(text, Style::new().fg(MODEL_META_COLOR), width)
}

/// `text` word-wrapped to inset dim rows (the info banner, an error).
fn wrapped(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    text.split('\n')
        .flat_map(|part| wrap_text(part, room))
        .map(|row| Line::from(vec![Span::raw(MODEL_INDENT), Span::styled(row, style)]))
        .collect()
}

/// One file's section: `{label} — {path}  {badge}` over its item lines (or
/// its parse error), the items capped at [`TRUST_MENU_MAX_ITEMS`] with a
/// `… +N more` fold.
fn file_lines(file: &TrustFileReview, width: u16) -> Vec<Line<'static>> {
    let (badge, badge_color) = if file.error.is_some() {
        (TRUST_BADGE_ERROR, ERROR_COLOR)
    } else if file.pending {
        (TRUST_BADGE_PENDING, ASK_WARNING_COLOR)
    } else {
        (TRUST_BADGE_TRUSTED, TOOL_OK_COLOR)
    };
    let head = format!("{} — {}", file.label, file.path);
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + 2 + cols(badge))
        .max(1);
    let mut lines = vec![Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            truncate_cols(&head, room),
            Style::new().fg(MODEL_ID_COLOR).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(badge.to_string(), Style::new().fg(badge_color)),
    ])];
    if let Some(error) = &file.error {
        lines.extend(wrapped(error, Style::new().fg(ERROR_COLOR), width));
        return lines;
    }
    let shown = file.items.iter().take(TRUST_MENU_MAX_ITEMS);
    let item_style = Style::new().fg(MODEL_META_COLOR);
    for item in shown {
        lines.push(line(&format!("  {item}"), item_style, width));
    }
    let hidden = file.items.len().saturating_sub(TRUST_MENU_MAX_ITEMS);
    if hidden > 0 {
        lines.push(line(&format!("  … +{hidden} more"), item_style, width));
    }
    lines
}

/// An option's row label.
const fn action_label(action: TrustAction) -> &'static str {
    match action {
        TrustAction::Approve => TRUST_APPROVE_LABEL,
        TrustAction::Revoke => TRUST_REVOKE_LABEL,
    }
}

/// The whole framed body — empty when the menu is closed. What
/// [`render_trust_menu`] paints and [`trust_menu_height`] counts.
#[must_use]
pub fn trust_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.trust_menu else {
        return Vec::new();
    };
    let TrustMenu { review, selected } = menu;
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        line(
            &format!("{TRUST_TITLE} — {}", review.root),
            Style::new().fg(AI_COLOR).add_modifier(Modifier::BOLD),
            width,
        ),
    ];
    let (status, status_color) = if review.trusted {
        (TRUST_STATUS_TRUSTED, TOOL_OK_COLOR)
    } else {
        (TRUST_STATUS_UNTRUSTED, ASK_WARNING_COLOR)
    };
    lines.push(Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled("Status: ".to_string(), Style::new().fg(MODEL_META_COLOR)),
        Span::styled(status.to_string(), Style::new().fg(status_color)),
    ]));
    lines.push(Line::default());
    lines.extend(wrapped(
        TRUST_INFO,
        Style::new().fg(MODEL_META_COLOR),
        width,
    ));
    lines.push(Line::default());
    if review.files.is_empty() {
        lines.push(dim_line(TRUST_EMPTY, width));
        lines.push(dim_line(
            &format!("  {}/.alter-zero/hooks.json", review.root),
            width,
        ));
        lines.push(dim_line(
            &format!("  {}/.alter-zero/mcp.json", review.root),
            width,
        ));
        lines.push(dim_line(&format!("  {}/.mcp.json", review.root), width));
    } else {
        for (i, file) in review.files.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.extend(file_lines(file, width));
        }
    }
    let options = review.options();
    if !options.is_empty() {
        lines.push(Line::default());
        let selected = (*selected).min(options.len() - 1);
        for (i, action) in options.iter().enumerate() {
            let (marker, style) = if i == selected {
                (
                    HOOKS_MARKER,
                    Style::new()
                        .fg(MODEL_SELECTED_COLOR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("  ", Style::new().fg(MODEL_ID_COLOR))
            };
            let marker_style = if i == selected {
                Style::new().fg(MODEL_SELECTED_COLOR)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::raw(MODEL_INDENT),
                Span::styled(marker.to_string(), marker_style),
                Span::styled(format!("{}. {}", i + 1, action_label(*action)), style),
            ]));
        }
    }
    let hint = if options.is_empty() {
        TRUST_CLOSE_HINT
    } else {
        TRUST_HINT
    };
    lines.extend([
        Line::default(),
        dim_line(hint, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The live-region height while the `/trust` menu is open, `None` when it
/// isn't — the hooks menu's rule: the streaming strip keeps its rows above
/// the menu, the body is the built line count, clamped to the terminal.
#[must_use]
pub fn trust_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.trust_menu.as_ref()?;
    let body = u16::try_from(trust_view_lines(app, width).len()).unwrap_or(u16::MAX);
    Some(super::layout::view_height(app, width, body, term_height))
}

/// Render the **inline** `/trust` menu into the live region, in place of the
/// composer. Pure — `render_live` paints this.
pub fn render_trust_menu(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(trust_view_lines(app, area.width)).render(area, buf);
}

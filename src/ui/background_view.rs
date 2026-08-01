//! The inline background-shell manager band reached with ↓.
//! See `docs/background.md`.

use super::model_view::model_rule;
use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;

/// A dim, `BG_INDENT`-inset single line for the ↓ manager band, truncated to
/// the width.
fn bg_dim_line(text: &str, width: u16) -> Line<'static> {
    bg_line(text, Style::new().fg(BG_DIM_COLOR), width)
}

/// A `BG_INDENT`-inset single line in `style`, truncated to the width.
fn bg_line(text: &str, style: Style, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(BG_INDENT)).max(1);
    Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(truncate_cols(text, room), style),
    ])
}

/// One row of the manager's shell list: `❯ {command} (running)` — the
/// selected row lights up in the palette accent (marker and text alike), the
/// others are dim, mirroring the slash-command palette's colour-only
/// selection.
fn bg_list_row(shell: &BackgroundShell, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { BG_MARKER } else { "  " };
    let style = if selected {
        Style::new().fg(BG_SELECTED_COLOR)
    } else {
        Style::new().fg(BG_DIM_COLOR)
    };
    let room = (width as usize)
        .saturating_sub(cols(BG_INDENT) + cols(BG_MARKER) + cols(BG_ROW_SUFFIX))
        .max(1);
    Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(marker.to_string(), style),
        Span::styled(truncate_cols(&shell.command, room), style),
        Span::styled(BG_ROW_SUFFIX.to_string(), style),
    ])
}

/// The manager's **list** page (or its empty state): title, `{n} active
/// shells`, the windowed selectable rows, and the key hints — all framed by
/// the picker rules. See `docs/background.md`.
fn bg_list_lines(app: &App, selected: usize, width: u16) -> Vec<Line<'static>> {
    let shells = app.background();
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        bg_line(BG_TITLE, Style::new().fg(AI_COLOR), width),
    ];
    if shells.is_empty() {
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_EMPTY, width));
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_EMPTY_HINTS, width));
    } else {
        let plural = if shells.len() == 1 { "" } else { "s" };
        lines.push(bg_dim_line(
            &format!("{} active shell{plural}", shells.len()),
            width,
        ));
        lines.push(Line::default());
        let selected = selected.min(shells.len() - 1);
        let offset = menu_window(shells.len(), selected, BG_MENU_MAX_ROWS);
        for (i, shell) in shells
            .iter()
            .enumerate()
            .skip(offset)
            .take(BG_MENU_MAX_ROWS)
        {
            lines.push(bg_list_row(shell, i == selected, width));
        }
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_LIST_HINTS, width));
    }
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The manager's **details** page for one shell: the status/runtime (the
/// humanized [`format_elapsed`], so a long-lived shell reads `6m 2s`) /
/// origin/command fields — the command **word-wraps** across rows (spaces
/// preserved, continuations aligned under the value column) so a long
/// command line is never truncated away, and a subagent-launched shell shows
/// a `From: {type} agent` field — the rounded output box tailing the last
/// [`BG_OUTPUT_ROWS`] lines of the live output (streaming in as the shell
/// runs), a `Showing N lines` caption, and the key hints. See
/// `docs/background.md`.
fn bg_details_lines(shell: &BackgroundShell, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(BG_DIM_COLOR);
    let value = Style::new().fg(AI_COLOR);
    let field = |label: &str, text: &str| {
        let room = (width as usize)
            .saturating_sub(cols(BG_INDENT) + cols(label))
            .max(1);
        Line::from(vec![
            Span::raw(BG_INDENT),
            Span::styled(label.to_string(), dim),
            Span::styled(truncate_cols(text, room), value),
        ])
    };
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        bg_line(BG_DETAILS_TITLE, Style::new().fg(AI_COLOR), width),
        Line::default(),
        field(BG_FIELD_STATUS, BG_STATUS_RUNNING),
        field(BG_FIELD_RUNTIME, &format_elapsed(shell.runtime.as_secs())),
    ];
    if let Some(origin) = &shell.origin {
        lines.push(field(
            BG_FIELD_FROM,
            &format!("{} agent", origin.agent_type),
        ));
    }
    // The command wraps instead of truncating (the user-requested fix): the
    // label leads the first row and continuations align under the value
    // column, `wrap_output` keeping the command's own spacing.
    let cmd_room = (width as usize)
        .saturating_sub(cols(BG_INDENT) + cols(BG_FIELD_COMMAND))
        .max(1);
    let rows = wrap_output(&shell.command, u16::try_from(cmd_room).unwrap_or(u16::MAX));
    if rows.is_empty() {
        lines.push(field(BG_FIELD_COMMAND, ""));
    }
    for (i, row) in rows.into_iter().enumerate() {
        if i == 0 {
            lines.push(Line::from(vec![
                Span::raw(BG_INDENT),
                Span::styled(BG_FIELD_COMMAND.to_string(), dim),
                Span::styled(row, value),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw(format!("{BG_INDENT}{}", " ".repeat(cols(BG_FIELD_COMMAND)))),
                Span::styled(row, value),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(bg_dim_line(BG_OUTPUT_LABEL, width));
    // The output box: rounded corners, one space of padding, the last
    // BG_OUTPUT_ROWS lines top-aligned over blank padding rows.
    let box_width = (width as usize).saturating_sub(cols(BG_INDENT) + 2).max(6);
    let inner = box_width - 2; // less the │ borders
    let text_room = inner.saturating_sub(2).max(1); // less one space each side
    let horizontal = "─".repeat(inner);
    lines.push(Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(format!("╭{horizontal}╮"), dim),
    ]));
    let output = shell.output.trim_end_matches('\n');
    let all: Vec<&str> = if output.is_empty() {
        Vec::new()
    } else {
        output.split('\n').collect()
    };
    let shown = all.len().min(BG_OUTPUT_ROWS);
    let tail = &all[all.len() - shown..];
    for row in 0..BG_OUTPUT_ROWS {
        let text = tail.get(row).copied().unwrap_or("");
        let clipped = truncate_cols(text, text_room);
        let pad = " ".repeat(text_room.saturating_sub(cols(&clipped)));
        lines.push(Line::from(vec![
            Span::raw(BG_INDENT),
            Span::styled("│ ".to_string(), dim),
            Span::styled(clipped, Style::new().fg(TOOL_OUTPUT_COLOR)),
            Span::raw(pad),
            Span::styled(" │".to_string(), dim),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(format!("╰{horizontal}╯"), dim),
    ]));
    let plural = if shown == 1 { "" } else { "s" };
    lines.push(bg_dim_line(&format!("Showing {shown} line{plural}"), width));
    lines.push(Line::default());
    lines.push(bg_dim_line(BG_DETAILS_HINTS, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// Every line of the open ↓ manager band, top rule to bottom rule — the
/// single source [`render_background_view`] paints and
/// [`background_view_height`] counts (no row ever wraps, so the count is
/// width-independent). Empty when the band is closed. See
/// `docs/background.md`.
#[must_use]
pub fn background_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    match &app.background_view {
        None => Vec::new(),
        Some(BackgroundView::List { selected }) => bg_list_lines(app, *selected, width),
        Some(BackgroundView::Details { id }) => match app.background_shell(id) {
            Some(shell) => bg_details_lines(shell, width),
            // The watched shell is gone (bg_exited retargets the view, so
            // this is a defensive fallback): show the list.
            None => bg_list_lines(app, 0, width),
        },
    }
}

/// Render the **inline** ↓ background manager band into the live region, in
/// place of the composer — the `/model` picker pattern (see
/// `docs/background.md`). Pure — `render_live` paints this.
pub fn render_background_view(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(background_view_lines(app, area.width)).render(area, buf);
}

//! The read-only `/export` page, and the plain-text export it produces.
//! See `docs/export.md`.
//!
//! The `/donate` page's frame — the family's rules and two-column inset,
//! the `❯` marker on the highlighted row, the dim hint — over the two
//! export targets, with the highlighted row's description under the list
//! (the `/settings` shape: the rows stay one glance wide, and what each one
//! does is said once, for the one that is about to be chosen).
//! [`export_view_lines`] builds the whole framed body line by line, so the
//! height is content-driven and falls out as `lines.len()`.
//!
//! [`export_text`] is the other half: the transcript **on screen** — the
//! Ctrl+O view's own rows, built by the same walk — as plain text, at the
//! width it is asked for. The styles are dropped and nothing else is
//! touched, so a file reads exactly as the terminal did.

use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::transcript::{agent_transcript_lines, transcript_lines};
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

use crate::app::ExportTarget;

/// The title row: `Export conversation`, bold in the `/login` pages' title
/// colour. Clamped with a trailing `…` at a width that can't seat it.
fn title_line(width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT));
    clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(
                ellipsize(EXPORT_TITLE, room),
                Style::new()
                    .fg(login_title_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ],
        width as usize,
    )
}

/// One target's row: `{marker}{n}. {label}` — the `❯` marker, number and
/// label in the accent on the highlighted row (the palette's whole-row
/// rule), the muted ink and a blank marker on the other. Nothing follows the
/// label: what the row does is the description under the list.
fn target_line(index: usize, target: ExportTarget, selected: bool, width: u16) -> Line<'static> {
    let accent = Style::new().fg(model_selected_color());
    let (marker, marker_style, number_style, label_style) = if selected {
        (
            HOOKS_MARKER,
            accent,
            accent,
            accent.add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "  ",
            Style::default(),
            Style::new().fg(model_meta_color()),
            Style::new().fg(model_id_color()),
        )
    };
    let number = format!("{}. ", index + 1);
    let used = cols(MODEL_INDENT) + cols(marker) + cols(&number);
    let room = (width as usize).saturating_sub(used);
    clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(marker.to_string(), marker_style),
            Span::styled(number, number_style),
            Span::styled(ellipsize(target.label(), room), label_style),
        ],
        width as usize,
    )
}

/// The highlighted target's description: where the clipboard row sends the
/// text, or the file's shape and the directory it lands in — the session's
/// cwd as the footer shows it, so the user knows where to look before the
/// file exists ([`EXPORT_FILE_DESC_DIR_FALLBACK`] until the boundary has
/// injected one).
fn description(app: &App, target: ExportTarget) -> String {
    match target {
        ExportTarget::Clipboard => EXPORT_CLIPBOARD_DESC.to_string(),
        ExportTarget::File => {
            let dir = app
                .session
                .as_ref()
                .map_or(EXPORT_FILE_DESC_DIR_FALLBACK, |s| s.cwd.as_str());
            format!("{EXPORT_FILE_DESC_PREFIX}{dir}.")
        }
    }
}

/// The whole framed page as lines: a top rule, the title, the dim blurb,
/// the two target rows, the highlighted row's description, the key hint,
/// and a bottom rule — built as **blocks** joined by exactly one blank row
/// (the `/login` page rule), so no two blank rows ever stack. What
/// [`render_export_picker`] paints (bottom-anchored) and `export_menu_rows`
/// counts, so the reserved height and the painted rows can never disagree
/// (`docs/view-flow.md`). Empty when the page is closed.
#[must_use]
pub fn export_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.export_picker.as_ref() else {
        return Vec::new();
    };
    let selected = picker.selected;
    let rows: Vec<Line<'static>> = ExportTarget::ALL
        .iter()
        .enumerate()
        .map(|(i, target)| target_line(i, *target, i == selected, width))
        .collect();
    let highlighted = ExportTarget::ALL
        .get(selected)
        .copied()
        .unwrap_or(ExportTarget::Clipboard);
    let blocks: Vec<Vec<Line<'static>>> = vec![
        vec![title_line(width)],
        model_wrapped_rows(EXPORT_BLURB, model_meta_color(), width),
        rows,
        model_wrapped_rows(&description(app, highlighted), model_meta_color(), width),
        vec![model_placeholder_row(
            EXPORT_HINT,
            model_meta_color(),
            width,
        )],
    ];

    let mut lines = vec![model_rule(width), Line::default()];
    for (i, block) in blocks.into_iter().filter(|b| !b.is_empty()).enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(block);
    }
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the page's own frame occupies — the built page's line count
/// ([`export_view_lines`]). What [`export_picker_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_export_picker`].
pub(super) fn export_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(export_view_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/export` page is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/export` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn export_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.export_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        export_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/export` page into the live region, in place of
/// the composer: the title, the blurb, the two rows, the description, the
/// hint, between the family's rules — bottom-anchored, so a squeezed area
/// keeps the rows, the hint and the closing rule on screen while the
/// skipped top flows into scrollback (`docs/view-flow.md`). Pure —
/// `render_live` paints this. See `docs/export.md`.
pub fn render_export_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, export_view_lines(app, area.width));
}

/// The conversation **on screen** as plain text, at `width` columns: the
/// Ctrl+O page's own rows, top to bottom — the startup banner it opens
/// with (the mascot beside `Alter Zero (v…)`, the cwd, the
/// `/login   /model   /resume` hint), then every message and every tool
/// call's full output, in order, plus the live tail mid-turn — inside a
/// subagent's session view that agent's transcript, else the main one
/// (the `/copy` rule). The styles are dropped; each row loses the padding
/// a cell carries on screen (a full-width user bubble, a right-aligned
/// stamp, the banner's padded rows); trailing blank rows go and the text
/// closes on exactly one newline. The command never exports an empty
/// conversation, so the pager's `Nothing here yet.` placeholder is never
/// reached.
#[must_use]
pub fn export_text(app: &App, width: u16) -> String {
    let lines = agent_transcript_lines(app, width).unwrap_or_else(|| transcript_lines(app, width));
    let mut rows: Vec<String> = lines
        .iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            text.trim_end().to_string()
        })
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    let mut text = rows.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

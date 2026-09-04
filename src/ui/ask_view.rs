//! The inline `AskUserQuestion` modal: the chip strip of question tabs, the
//! numbered options, the side-by-side preview panel, and the Submit review
//! page. See `docs/ask.md`.
//!
//! The permission prompt's sibling ([`super::permission_view`]): one builder
//! ([`ask_build`]) produces every row **and** the hardware cursor's seat, and
//! [`ask_height`] reserves those rows clamped to the terminal, so the
//! reserved height, the painted rows, and the cursor can never drift apart.
//! A page taller than the terminal paints bottom-anchored and flows its
//! skipped top into real scrollback like every framed view
//! (`docs/view-flow.md`).

use crate::app::{AskAnswerState, AskInput, AskPrompt, AskRow, ask_row_number, ask_rows};
use crate::ask::{AskOption, AskQuestion};

use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;

/// A full-width rule in the input box's border colour — the modal's frame
/// (the permission prompt's).
fn rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_RULE.repeat(width as usize),
        Style::new().fg(BORDER_COLOR),
    ))
}

/// A one-space-inset row of plain `text`, wrapped to the width.
fn text_rows(text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(ASK_INDENT)).max(1);
    wrap_output(text, room as u16)
        .into_iter()
        .map(|row| {
            Line::from(vec![
                Span::raw(ASK_INDENT),
                Span::styled(row, Style::new().fg(color)),
            ])
        })
        .collect()
}

/// The hint row under the body — `{key}{label}` pairs joined by ` · `, keys in
/// the accent colour (the permission prompt's shape).
fn hint_row(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(ASK_INDENT)];
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

/// The chip strip: `←  ☒ Coffee style  ☒ Demo topics  ✔ Submit  →`, the
/// current chip lit on the cyan selection background (the user-requested
/// highlight), answered questions checked `☒`, unanswered `☐`, the Submit tab
/// `✔`. A lone question shows just its own chip, no arrows.
fn chip_strip(prompt: &AskPrompt, width: u16) -> Line<'static> {
    let dim = Style::new().fg(ASK_CHIP_COLOR);
    let current = Style::new().fg(ASK_CHIP_CURRENT_FG).bg(ASK_CHIP_CURRENT_BG);
    let mut spans = vec![Span::raw(ASK_INDENT)];
    if prompt.has_submit_tab() {
        spans.push(Span::styled(ASK_ARROW_LEFT, dim));
        spans.push(Span::raw(ASK_CHIP_GAP));
    }
    for (i, question) in prompt.request.questions.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(ASK_CHIP_GAP));
        }
        let mark = if prompt.answers[i].answered() {
            ASK_CHIP_ANSWERED
        } else {
            ASK_CHIP_UNANSWERED
        };
        // The header names the tab; an empty one falls back to `Question N`.
        let header = Some(question.header.trim())
            .filter(|h| !h.is_empty())
            .map_or_else(|| format!("Question {}", i + 1), str::to_string);
        let text = format!(" {mark} {header} ");
        spans.push(Span::styled(
            text,
            if prompt.tab == i { current } else { dim },
        ));
    }
    if prompt.has_submit_tab() {
        spans.push(Span::raw(ASK_CHIP_GAP));
        let text = format!(" {ASK_CHIP_SUBMIT} Submit ");
        spans.push(Span::styled(
            text,
            if prompt.on_submit_tab() { current } else { dim },
        ));
        spans.push(Span::raw(ASK_CHIP_GAP));
        spans.push(Span::styled(ASK_ARROW_RIGHT, dim));
    }
    // Clip to the width so a narrow terminal never wraps the strip.
    let mut line = Line::from(spans);
    let mut used = 0usize;
    line.spans.retain_mut(|span| {
        let w = cols(&span.content);
        if used + w <= width as usize {
            used += w;
            true
        } else {
            let room = (width as usize).saturating_sub(used);
            used = width as usize;
            if room == 0 {
                return false;
            }
            span.content = truncate_cols(&span.content, room).into();
            true
        }
    });
    line
}

/// The row prefix columns: the inset + the `❯ ` marker slot + the `N. `
/// number slot (three columns wide so single digits line up).
fn row_prefix(selected: bool, number: Option<usize>) -> Vec<Span<'static>> {
    let marker = if selected {
        PERMISSION_MARKER.to_string()
    } else {
        " ".repeat(cols(PERMISSION_MARKER))
    };
    let number = match number {
        Some(n) => format!("{n}. "),
        None => "   ".to_string(),
    };
    let style = if selected {
        Style::new().fg(PERMISSION_SELECTED_COLOR)
    } else {
        Style::default()
    };
    vec![
        Span::raw(ASK_INDENT),
        Span::styled(format!("{marker}{number}"), style),
    ]
}

/// The columns a row's content starts at — what the description rows and the
/// cursor seat align under.
fn content_col() -> usize {
    cols(ASK_INDENT) + cols(PERMISSION_MARKER) + cols("1. ")
}

/// One option's label spans: the multi-select checkbox, the label, and the
/// single-select `✔` on the recorded answer.
fn option_label_spans(
    question: &AskQuestion,
    option: &AskOption,
    picked: bool,
    selected: bool,
) -> Vec<Span<'static>> {
    let style = if selected {
        Style::new().fg(PERMISSION_SELECTED_COLOR)
    } else {
        Style::default()
    };
    let mut spans = Vec::new();
    if question.multi_select {
        let (mark, mark_style) = if picked {
            (ASK_CHECKED, Style::new().fg(ASK_PICKED_COLOR))
        } else {
            (ASK_UNCHECKED, style)
        };
        spans.push(Span::styled(mark.to_string(), mark_style));
    }
    spans.push(Span::styled(option.label.clone(), style));
    if !question.multi_select && picked {
        spans.push(Span::styled(
            ASK_PICKED_MARK.to_string(),
            Style::new().fg(ASK_PICKED_COLOR),
        ));
    }
    spans
}

/// What the modal builds: every row of the region; while an entry field is
/// live, the cursor's `(row, col)` inside those rows; and the highlighted
/// `❯` row's index (`marker`), the hidden cursor's resting seat.
struct AskBuild {
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    marker: Option<usize>,
}

/// Build the whole modal at `width`: rule → chip strip → the current page →
/// hints → rule. Pure and total — an app without an open modal builds empty.
fn ask_build(app: &App, width: u16) -> AskBuild {
    let Some(prompt) = app.ask() else {
        return AskBuild {
            lines: Vec::new(),
            cursor: None,
            marker: None,
        };
    };
    let mut lines = vec![rule(width), Line::default()];
    lines.push(chip_strip(prompt, width));
    lines.push(Line::default());
    let mut cursor = None;
    let mut marker = None;
    if prompt.on_submit_tab() {
        build_review_page(prompt, width, &mut lines, &mut marker);
    } else if let Some(question) = prompt.current_question() {
        lines.extend(text_rows(&question.question, Color::Reset, width));
        lines.push(Line::default());
        if question.has_previews() {
            build_preview_page(
                app,
                prompt,
                question,
                width,
                &mut lines,
                &mut cursor,
                &mut marker,
            );
        } else {
            build_list_page(
                app,
                prompt,
                question,
                width,
                &mut lines,
                &mut cursor,
                &mut marker,
            );
        }
    }
    lines.push(Line::default());
    lines.push(hint_row(&hints(prompt)));
    lines.push(Line::default());
    lines.push(rule(width));
    AskBuild {
        lines,
        cursor,
        marker,
    }
}

/// The hint pairs for the modal's current state. The entry fields name the
/// newline key (Shift+Enter — Ctrl+J is the universal fallback, as in the
/// composer, `docs/shift-enter.md`).
fn hints(prompt: &AskPrompt) -> Vec<(&'static str, &'static str)> {
    match prompt.input_mode {
        AskInput::Other => vec![
            ("Enter", " to accept"),
            ("Shift+Enter", " for newline"),
            ("Esc", " to go back"),
        ],
        AskInput::Notes => vec![
            ("Enter/Esc", " to save and go back"),
            ("Shift+Enter", " for newline"),
        ],
        AskInput::Select => {
            let mut out = vec![("Enter", " to select")];
            if prompt.has_submit_tab() {
                out.push(("Tab/Arrow keys", " to navigate"));
            } else {
                out.push(("↑/↓", " to navigate"));
            }
            if prompt
                .current_question()
                .is_some_and(AskQuestion::has_previews)
            {
                out.push(("n", " to add notes"));
            }
            out.push(("Esc", " to cancel"));
            out
        }
    }
}

/// A question page in the plain (no-preview) layout: every row full-width,
/// each option's dim description under its label.
fn build_list_page(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    cursor: &mut Option<(usize, usize)>,
    marker: &mut Option<usize>,
) {
    let state = &prompt.answers[prompt.tab];
    let desc_indent = " ".repeat(
        content_col()
            + if question.multi_select {
                cols(ASK_CHECKED)
            } else {
                0
            },
    );
    let room = (width as usize).saturating_sub(content_col()).max(1);
    for (at, row) in ask_rows(question).iter().enumerate() {
        let selected = prompt.row == at;
        if selected {
            // The highlighted row's first line — where the `❯` paints, and
            // where the hidden cursor rests.
            *marker = Some(lines.len());
        }
        let number = ask_row_number(question, *row);
        match *row {
            AskRow::Option(i) => {
                let picked = state.selected.contains(&i);
                let mut spans = row_prefix(selected, number);
                spans.extend(option_label_spans(
                    question,
                    &question.options[i],
                    picked,
                    selected,
                ));
                lines.push(Line::from(spans));
                let description = question.options[i].description.trim();
                if !description.is_empty() {
                    let desc_room = (width as usize).saturating_sub(cols(&desc_indent)).max(1);
                    for text in wrap_output(description, desc_room as u16) {
                        lines.push(Line::from(vec![
                            Span::raw(desc_indent.clone()),
                            Span::styled(text, Style::new().fg(ASK_DESC_COLOR)),
                        ]));
                    }
                }
            }
            AskRow::Other => {
                let at_line = lines.len();
                lines.extend(other_rows(
                    app, prompt, question, state, selected, room, at_line, cursor,
                ));
            }
            AskRow::Confirm => {
                let mut spans = row_prefix(selected, None);
                spans.push(Span::styled(
                    ASK_CONFIRM_LABEL.to_string(),
                    if selected {
                        Style::new().fg(PERMISSION_SELECTED_COLOR)
                    } else {
                        Style::default()
                    },
                ));
                lines.push(Line::from(spans));
            }
            AskRow::Chat => {
                let mut spans = row_prefix(selected, number);
                spans.push(Span::styled(
                    ASK_CHAT_LABEL.to_string(),
                    if selected {
                        Style::new().fg(PERMISSION_SELECTED_COLOR)
                    } else {
                        Style::default()
                    },
                ));
                lines.push(Line::from(spans));
            }
        }
    }
}

/// A multi-line text flattened for a one-row display (an accepted Other
/// answer, the saved notes): the entry fields take Shift+Enter newlines, and
/// a raw `\n` inside a span paints as nothing, gluing the lines together.
fn flat(text: &str) -> String {
    text.trim().replace('\n', " ")
}

/// The free-text "Other" row in whatever state it is in: the **entry field**
/// while it is edited — the composer's wrapped rows behind the `❯ N. ` lead,
/// continuations aligned under the text, the cursor seat recorded (Tab's
/// amend-field shape, so Shift+Enter newlines and `[Pasted Content N chars]`
/// placeholders render exactly as in the composer) — else the accepted text
/// with its pick mark, or the `Type something.` label.
#[allow(clippy::too_many_arguments)] // one row, many facts about it
fn other_rows(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    state: &crate::app::AskAnswerState,
    selected: bool,
    room: usize,
    at_line: usize,
    cursor: &mut Option<(usize, usize)>,
) -> Vec<Line<'static>> {
    let number = ask_row_number(question, AskRow::Other);
    let style = if selected {
        Style::new().fg(PERMISSION_SELECTED_COLOR)
    } else {
        Style::default()
    };
    if prompt.input_mode == AskInput::Other {
        let field = super::layout::text_field_width(u16::try_from(room).unwrap_or(u16::MAX));
        let (crow, ccol) = app.input.cursor_row_col(field);
        *cursor = Some((at_line + crow, content_col() + ccol));
        let continuation = " ".repeat(content_col());
        return app
            .input
            .display_rows(field)
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let mut spans = if i == 0 {
                    row_prefix(selected, number)
                } else {
                    vec![Span::raw(continuation.clone())]
                };
                spans.push(Span::styled(text, style));
                Line::from(spans)
            })
            .collect();
    }
    let mut spans = row_prefix(selected, number);
    if question.multi_select {
        let (mark, mark_style) = if state.other_chosen {
            (ASK_CHECKED, Style::new().fg(ASK_PICKED_COLOR))
        } else {
            (ASK_UNCHECKED, style)
        };
        spans.push(Span::styled(mark.to_string(), mark_style));
    }
    let text = flat(&state.other);
    if text.is_empty() {
        spans.push(Span::styled(ASK_OTHER_LABEL.to_string(), style));
    } else {
        spans.push(Span::styled(truncate_cols(&text, room).to_string(), style));
        if !question.multi_select && state.other_chosen {
            spans.push(Span::styled(
                ASK_PICKED_MARK.to_string(),
                Style::new().fg(ASK_PICKED_COLOR),
            ));
        }
    }
    vec![Line::from(spans)]
}

/// A question page in the **side-by-side** layout (any option carries a
/// preview): the option rows on the left, the focused option's preview in a
/// bordered panel on the right, the notes line beneath the panel, and the
/// `Chat about this` row below.
fn build_preview_page(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    cursor: &mut Option<(usize, usize)>,
    marker: &mut Option<usize>,
) {
    let state = &prompt.answers[prompt.tab];
    let rows = ask_rows(question);
    // The left column: every row but Chat (which renders full-width below).
    let left_rows: Vec<AskRow> = rows
        .iter()
        .copied()
        .filter(|row| *row != AskRow::Chat)
        .collect();
    // Sized to the widest left row, capped at the share that keeps a useful
    // panel.
    let left_max = ((f32::from(width) * ASK_LEFT_MAX_SHARE) as usize).max(content_col() + 8);
    let left_width = left_rows
        .iter()
        .map(|row| {
            content_col()
                + match *row {
                    AskRow::Option(i) => cols(&question.options[i].label),
                    AskRow::Other => cols(ASK_OTHER_LABEL).max(cols(state.other.trim())),
                    AskRow::Confirm => cols(ASK_CONFIRM_LABEL),
                    AskRow::Chat => 0,
                }
                + cols(ASK_PICKED_MARK)
        })
        .max()
        .unwrap_or(content_col())
        .min(left_max);
    let panel_x = left_width + 2;
    let panel_width = (width as usize).saturating_sub(panel_x + 1);
    // The panel shows the focused option's preview; a non-option focus falls
    // back to the picked option, then the first with a preview.
    let focused = match rows.get(prompt.row) {
        Some(AskRow::Option(i)) => Some(*i),
        _ => None,
    };
    let shown = focused
        .filter(|i| {
            question
                .options
                .get(*i)
                .is_some_and(|o| o.preview.is_some())
        })
        .or_else(|| state.selected.iter().next().copied())
        .filter(|i| {
            question
                .options
                .get(*i)
                .is_some_and(|o| o.preview.is_some())
        })
        .or_else(|| question.options.iter().position(|o| o.preview.is_some()));
    let panel = shown
        .and_then(|i| question.options[i].preview.as_deref())
        .map(|preview| panel_lines(preview, panel_width))
        .unwrap_or_default();
    // The left column, one or more **visual** rows per logical row: the Other
    // entry wraps to the column while it is edited (Shift+Enter newlines, a
    // pasted placeholder), everything else stays a single row. The cursor is
    // remembered as a (visual row, col) pending seat and made absolute once
    // the zip below fixes where the columns start.
    let entry_room = left_width.saturating_sub(content_col()).max(1);
    let mut left: Vec<Vec<Span<'static>>> = Vec::new();
    let mut pending_cursor: Option<(usize, usize)> = None;
    let mut pending_marker: Option<usize> = None;
    for (at, row) in left_rows.iter().enumerate() {
        let selected = prompt.row == at;
        if selected {
            // The highlighted left row's first visual row — made absolute
            // once the zip below fixes where the columns start.
            pending_marker = Some(left.len());
        }
        match *row {
            AskRow::Option(idx) => {
                let picked = state.selected.contains(&idx);
                let mut spans = row_prefix(selected, ask_row_number(question, *row));
                spans.extend(option_label_spans(
                    question,
                    &question.options[idx],
                    picked,
                    selected,
                ));
                left.push(spans);
            }
            AskRow::Other => {
                let at_row = left.len();
                let mut sub = None;
                for line in other_rows(
                    app, prompt, question, state, selected, entry_room, at_row, &mut sub,
                ) {
                    left.push(line.spans);
                }
                if let Some((vrow, col)) = sub {
                    pending_cursor = Some((vrow, col));
                }
            }
            AskRow::Confirm => {
                let mut spans = row_prefix(selected, None);
                spans.push(Span::styled(
                    ASK_CONFIRM_LABEL.to_string(),
                    if selected {
                        Style::new().fg(PERMISSION_SELECTED_COLOR)
                    } else {
                        Style::default()
                    },
                ));
                left.push(spans);
            }
            AskRow::Chat => {}
        }
    }
    let zip_start = lines.len();
    if let Some((vrow, col)) = pending_cursor {
        *cursor = Some((zip_start + vrow, col));
    }
    if let Some(vrow) = pending_marker {
        *marker = Some(zip_start + vrow);
    }
    let total = left.len().max(panel.len());
    for i in 0..total {
        let mut spans: Vec<Span<'static>> = left.get(i).cloned().unwrap_or_default();
        let mut used: usize = spans.iter().map(|s| cols(&s.content)).sum();
        // A left row wider than its column clips so the panel stays on its
        // grid.
        if used > left_width {
            clip_spans(&mut spans, left_width);
            used = left_width;
        }
        if let Some(panel_row) = panel.get(i) {
            spans.push(Span::raw(" ".repeat(panel_x.saturating_sub(used))));
            spans.extend(panel_row.iter().cloned());
        }
        lines.push(Line::from(spans));
    }
    // The notes block under the panel, at the panel's column (the reference
    // layout): the typed notes, the live entry field — the composer's wrapped
    // rows, continuations aligned under the text — or the dim hint.
    lines.push(Line::default());
    let notes_pad = " ".repeat(panel_x);
    let notes_room = (width as usize)
        .saturating_sub(panel_x + cols(ASK_NOTES_LABEL))
        .max(1);
    let label = || Span::styled(ASK_NOTES_LABEL.to_string(), Style::new().fg(ASK_DESC_COLOR));
    if prompt.input_mode == AskInput::Notes {
        let field = super::layout::text_field_width(u16::try_from(notes_room).unwrap_or(u16::MAX));
        let (crow, ccol) = app.input.cursor_row_col(field);
        *cursor = Some((lines.len() + crow, panel_x + cols(ASK_NOTES_LABEL) + ccol));
        let continuation = " ".repeat(panel_x + cols(ASK_NOTES_LABEL));
        for (i, text) in app.input.display_rows(field).into_iter().enumerate() {
            let mut spans = if i == 0 {
                vec![Span::raw(notes_pad.clone()), label()]
            } else {
                vec![Span::raw(continuation.clone())]
            };
            spans.push(Span::raw(text));
            lines.push(Line::from(spans));
        }
    } else if state.notes.trim().is_empty() {
        lines.push(Line::from(vec![
            Span::raw(notes_pad),
            label(),
            Span::styled(
                ASK_NOTES_PLACEHOLDER.to_string(),
                Style::new().fg(ASK_DESC_COLOR),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw(notes_pad),
            label(),
            Span::raw(truncate_cols(&flat(&state.notes), notes_room).to_string()),
        ]));
    }
    // The Chat row, full-width below.
    lines.push(Line::default());
    if let Some(at) = rows.iter().position(|row| *row == AskRow::Chat) {
        let selected = prompt.row == at;
        if selected {
            *marker = Some(lines.len());
        }
        let mut spans = row_prefix(selected, ask_row_number(question, AskRow::Chat));
        spans.push(Span::styled(
            ASK_CHAT_LABEL.to_string(),
            if selected {
                Style::new().fg(PERMISSION_SELECTED_COLOR)
            } else {
                Style::default()
            },
        ));
        lines.push(Line::from(spans));
    }
}

/// Clip a span run to `max` display columns.
fn clip_spans(spans: &mut Vec<Span<'static>>, max: usize) {
    let mut used = 0usize;
    spans.retain_mut(|span| {
        let w = cols(&span.content);
        if used + w <= max {
            used += w;
            true
        } else {
            let room = max.saturating_sub(used);
            used = max;
            if room == 0 {
                return false;
            }
            span.content = truncate_cols(&span.content, room).into();
            true
        }
    });
}

/// The preview panel's bordered rows at `width` columns (border included):
/// each row is a span run — dim `│ ` edges around the content — ready to be
/// appended after a left-column row.
fn panel_lines(preview: &str, width: usize) -> Vec<Vec<Span<'static>>> {
    let dim = Style::new().fg(ASK_PREVIEW_COLOR);
    if width < 6 {
        return Vec::new();
    }
    let inner = width - 4; // `│ ` + ` │`
    let mut rows: Vec<Vec<Span>> = Vec::new();
    rows.push(vec![Span::styled(
        format!("┌{}┐", "─".repeat(width - 2)),
        dim,
    )]);
    for line in preview.split('\n').take(ASK_PREVIEW_MAX_ROWS) {
        let text = truncate_cols(&super::assistant::expand_code_tabs(line), inner).to_string();
        let pad = inner.saturating_sub(cols(&text));
        rows.push(vec![
            Span::styled("│ ".to_string(), dim),
            Span::styled(
                format!("{text}{}", " ".repeat(pad)),
                Style::new().fg(TOOL_OUTPUT_COLOR),
            ),
            Span::styled(" │".to_string(), dim),
        ]);
    }
    rows.push(vec![Span::styled(
        format!("└{}┘", "─".repeat(width - 2)),
        dim,
    )]);
    rows
}

/// The Submit page: a `⚠` warning when any question is still unanswered,
/// the review list of the **answered** questions only (`● question` over the
/// green `→ answer` — an unanswered one is omitted, its ☐ chip and the
/// warning already say so), the closing question, and the `Submit answers` /
/// `Cancel` options.
fn build_review_page(
    prompt: &AskPrompt,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    marker: &mut Option<usize>,
) {
    lines.extend(text_rows(ASK_REVIEW_TITLE, Color::Reset, width));
    lines.push(Line::default());
    // The warning leads the page whenever the submission would be partial —
    // Submit answers sends only what is answered (and with nothing answered
    // it walks back to the first open question instead).
    if !prompt.answers.iter().all(AskAnswerState::answered) {
        lines.extend(text_rows(ASK_WARNING, ASK_WARNING_COLOR, width));
        lines.push(Line::default());
    }
    let bullet_indent = cols(ASK_INDENT) + cols(ASK_REVIEW_BULLET);
    let q_room = (width as usize).saturating_sub(bullet_indent).max(1);
    let mut listed = false;
    for (question, state) in prompt.request.questions.iter().zip(&prompt.answers) {
        // Only the answered questions are reviewed — an open one shows
        // nothing here (the warning above and its ☐ chip already say so).
        if !state.answered() {
            continue;
        }
        listed = true;
        for (i, row) in wrap_output(&question.question, q_room as u16)
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 {
                Span::styled(ASK_REVIEW_BULLET.to_string(), Style::new().fg(SYSTEM_COLOR))
            } else {
                Span::raw(" ".repeat(cols(ASK_REVIEW_BULLET)))
            };
            lines.push(Line::from(vec![
                Span::raw(ASK_INDENT),
                lead,
                Span::raw(row),
            ]));
        }
        let answer_indent = " ".repeat(bullet_indent);
        let a_room = (width as usize)
            .saturating_sub(bullet_indent + cols(ASK_ANSWER_ARROW))
            .max(1);
        let mut labels: Vec<String> = state
            .selected
            .iter()
            .filter_map(|&i| question.options.get(i).map(|o| o.label.clone()))
            .collect();
        if state.other_chosen {
            labels.push(flat(&state.other));
        }
        let answer = labels.join(", ");
        // A huge answer (an expanded paste) previews capped — the page must
        // stay a review, not a pager; the full text rides the submission.
        let mut rows = wrap_output(&answer, a_room as u16);
        let capped = rows.len() > ASK_REVIEW_ANSWER_MAX_ROWS;
        if capped {
            rows.truncate(ASK_REVIEW_ANSWER_MAX_ROWS);
        }
        for (i, row) in rows.into_iter().enumerate() {
            let lead = if i == 0 {
                Span::styled(
                    ASK_ANSWER_ARROW.to_string(),
                    Style::new().fg(ASK_DESC_COLOR),
                )
            } else {
                Span::raw(" ".repeat(cols(ASK_ANSWER_ARROW)))
            };
            lines.push(Line::from(vec![
                Span::raw(answer_indent.clone()),
                lead,
                // Green, so the recorded answer is the row that carries the
                // eye (the user-requested emphasis).
                Span::styled(row, Style::new().fg(ASK_ANSWER_COLOR)),
            ]));
        }
        if capped {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(bullet_indent + cols(ASK_ANSWER_ARROW))),
                Span::styled("…".to_string(), Style::new().fg(ASK_DESC_COLOR)),
            ]));
        }
    }
    if listed {
        lines.push(Line::default());
    }
    lines.extend(text_rows(ASK_REVIEW_QUESTION, Color::Reset, width));
    lines.push(Line::default());
    for (i, label) in [ASK_SUBMIT_LABEL, ASK_CANCEL_LABEL].iter().enumerate() {
        let selected = prompt.row == i;
        if selected {
            *marker = Some(lines.len());
        }
        let mut spans = row_prefix(selected, Some(i + 1));
        spans.push(Span::styled(
            (*label).to_string(),
            if selected {
                Style::new().fg(PERMISSION_SELECTED_COLOR)
            } else {
                Style::default()
            },
        ));
        lines.push(Line::from(spans));
    }
}

/// Every row of the open ask modal at `width` — the **whole** page, top rule
/// to bottom rule; empty when no modal is open. The page is never cut here:
/// the paint bottom-anchors it into the region ([`render_ask`]) so the
/// interactive tail (options, hints, closing rule) stays on screen, and a
/// page taller than the terminal **flows** its skipped top into the
/// terminal's real scrollback like every framed view (`view_flow`,
/// `docs/view-flow.md`) — the retired top-drop clamp put the chip strip, the
/// question and the first options in no buffer at all on a short terminal.
#[must_use]
pub fn ask_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    ask_build(app, width).lines
}

/// The inline live-region height when the ask modal is open, or `None` when
/// it isn't (the caller falls back through the picker chain). The rows
/// [`ask_lines`] builds, clamped to the terminal — the permission prompt's
/// `permission_height` shape.
#[must_use]
pub fn ask_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.ask()?;
    let rows = ask_lines(app, width).len() as u16;
    Some(rows.min(term_height.max(1)))
}

/// The hardware cursor's `(x, y)` inside the modal's region: the live entry
/// field's caret (the Other row, the notes line) when one is open, else the
/// highlighted `❯` row at the option text's column — the permission prompt's
/// seat, so a terminal's cursor animation lands on the row being chosen
/// (`❯ 1. Black`) rather than the far end of the bottom rule, while
/// [`cursor_visible`](super::cursor_visible) still hides it over the menu.
/// `None` only with no open modal, or when the seat's row was clamped off the
/// page (the caller's far-corner fallback, the menus' rule).
#[must_use]
pub(super) fn ask_cursor(app: &App, width: u16, term_height: u16) -> Option<(u16, u16)> {
    let build = ask_build(app, width);
    let marker_col = cols(ASK_INDENT) + cols(PERMISSION_MARKER);
    let (row, col) = build.cursor.or(build.marker.map(|row| (row, marker_col)))?;
    // The paint bottom-anchors the page (`docs/view-flow.md`), so the seat
    // shifts by the same skipped top — and a seat inside the skip (a row
    // that flowed into scrollback) has no on-screen row.
    let dropped = super::view_flow::view_body_skip(build.lines.len(), term_height.max(1));
    let row = row.checked_sub(dropped)?;
    Some((
        col.min(usize::from(width.saturating_sub(1))) as u16,
        row.min(usize::from(term_height.saturating_sub(1))) as u16,
    ))
}

/// Paint the open ask modal over the whole live region — bottom-anchored, so
/// a page taller than the region keeps its tail (the options, the hints, the
/// closing rule) on screen while the skipped top flows into scrollback
/// (`docs/view-flow.md`). Pure — [`render_live`] calls this in place of the
/// composer, and the region is sized from the same builder ([`ask_height`]).
pub fn render_ask(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, ask_lines(app, area.width));
}

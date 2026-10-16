//! The inline `AskUserQuestion` modal: the chip strip of question tabs, the
//! numbered options, the side-by-side preview panel, and the Submit review
//! page. See `docs/ask.md`.
//!
//! The permission prompt's sibling ([`super::permission_view`]): one builder
//! ([`ask_build`]) produces every row **and** the hardware cursor's seat, and
//! [`ask_height`] reserves those rows clamped to the terminal, so the
//! reserved height, the painted rows, and the cursor can never drift apart.
//! On a terminal too short for the whole page the builder sheds what the
//! choice doesn't need first — the descriptions, then the rows outside a
//! window around the selection, then the chip strip — and only a page
//! taller than even that paints bottom-anchored and flows its skipped top
//! into real scrollback like every framed view (`docs/view-flow.md`).

use crate::app::{AskAnswerState, AskInput, AskPrompt, AskRow, ask_row_number, ask_rows};
use crate::ask::{AskOption, AskQuestion};

use super::model_view::list_window;
use super::theme::*;
use super::wrap::{cols, ellipsize, truncate_cols, wrap_output};
use super::*;

/// A full-width rule in the input box's border colour — the modal's frame
/// (the permission prompt's).
fn rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_RULE.repeat(width as usize),
        Style::new().fg(border_color()),
    ))
}

/// The closing rule: plain, or — while the idle clock runs — carrying the
/// countdown right-aligned as a lit chip with one bare rule cell after it
/// (`── Timeout: 9:41 ─`, `docs/ask.md`), the agent session view's label
/// dress: the theme's accent under its on-accent ink, the padding inside the
/// fill. The count rounds **up** to the whole second, so a fresh clock reads
/// `10:00` and the last second `0:01` rather than `0:00` over a question that
/// is still open; in its last minute the chip lights the caution amber. Too
/// narrow to keep [`ASK_TIMEOUT_MIN_LEAD`] rule cells in front of it, the
/// rule stays plain — the clock still runs, and the resolved cell says what
/// happened.
fn closing_rule(width: u16, remaining: Option<std::time::Duration>) -> Line<'static> {
    let Some(left) = remaining else {
        return rule(width);
    };
    let secs = u64::try_from(left.as_millis().div_ceil(1000)).unwrap_or(u64::MAX);
    let count = super::login_view::countdown(std::time::Duration::from_secs(secs));
    let label = format!(" {ASK_TIMEOUT_PREFIX}{count} ");
    let Some(lead) = (width as usize)
        .checked_sub(cols(&label) + 1)
        .filter(|lead| *lead >= ASK_TIMEOUT_MIN_LEAD)
    else {
        return rule(width);
    };
    let ground = if secs < ASK_TIMEOUT_WARN.as_secs() {
        ask_timeout_warn_bg()
    } else {
        ask_timeout_bg()
    };
    let border = Style::new().fg(border_color());
    Line::from(vec![
        Span::styled(PERMISSION_RULE.repeat(lead), border),
        Span::styled(label, Style::new().fg(ask_timeout_fg()).bg(ground)),
        Span::styled(PERMISSION_RULE, border),
    ])
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
                Style::new().fg(permission_hint_text_color()),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(permission_hint_key_color()),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(permission_hint_text_color()),
        ));
    }
    Line::from(spans)
}

/// The chip strip: `←  ☒ Coffee style  ☒ Demo topics  ✔ Submit  →`, the
/// current chip lit on the cyan selection background (the user-requested
/// highlight), answered questions checked `☒`, unanswered `☐`, the Submit tab
/// `✔`. A lone question shows just its own chip, no arrows.
fn chip_strip(prompt: &AskPrompt, width: u16) -> Line<'static> {
    let dim = Style::new().fg(ask_chip_color());
    let current = Style::new()
        .fg(ask_chip_current_fg())
        .bg(ask_chip_current_bg());
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
        Style::new().fg(permission_selected_color())
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
        Style::new().fg(permission_selected_color())
    } else {
        Style::default()
    };
    let mut spans = Vec::new();
    if question.multi_select {
        let (mark, mark_style) = if picked {
            (ASK_CHECKED, Style::new().fg(ask_picked_color()))
        } else {
            (ASK_UNCHECKED, style)
        };
        spans.push(Span::styled(mark.to_string(), mark_style));
    }
    spans.push(Span::styled(option.label.clone(), style));
    if !question.multi_select && picked {
        spans.push(Span::styled(
            ASK_PICKED_MARK.to_string(),
            Style::new().fg(ask_picked_color()),
        ));
    }
    spans
}

/// What the modal builds: every row of the region; while an entry field is
/// live, the cursor's `(row, col)` inside those rows; and the highlighted
/// `❯` row's index (`marker`), the hidden cursor's resting seat.
#[derive(Default)]
struct AskBuild {
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    marker: Option<usize>,
}

/// How a question page lays out its options' descriptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Descriptions {
    /// Wrapped whole — the whole page's layout.
    Wrapped,
    /// Cut to one `…`-ended row each, so the page is one height whatever
    /// the descriptions say.
    OneRow,
    /// Left out.
    Hidden,
}

/// What a page keeps — one rung of the ladder a terminal too short for the
/// whole page walks down (`docs/view-flow.md` *A page that sheds rows before
/// it flows*, `docs/ask.md`).
#[derive(Clone, Copy, Debug)]
struct Fit {
    /// Whether the chip strip of question tabs shows.
    chips: bool,
    /// The plain layout's descriptions.
    descriptions: Descriptions,
    /// The plain layout's window: at most this many rows between the ↑/↓
    /// overflow slots, or every row.
    window: Option<u16>,
    /// The side-by-side layout's dim `Notes: press n to add notes` line —
    /// typed notes and the live field show whatever this says.
    notes_hint: bool,
    /// Whether the side-by-side layout's panel is cut to the option
    /// column's height.
    panel_capped: bool,
}

impl Fit {
    /// The whole page.
    const WHOLE: Self = Self {
        chips: true,
        descriptions: Descriptions::Wrapped,
        window: None,
        notes_hint: true,
        panel_capped: false,
    };
}

/// A plain question page's rungs: the descriptions cut to one row, then
/// left out; then the rows windowed around the selection — the compact
/// pages' window — and last the chip strip. codex's order: what the choice
/// doesn't need yields first, the tabs and at least one row stay longest.
const LIST_FITS: [Fit; 5] = [
    Fit::WHOLE,
    Fit {
        descriptions: Descriptions::OneRow,
        ..Fit::WHOLE
    },
    Fit {
        descriptions: Descriptions::Hidden,
        ..Fit::WHOLE
    },
    Fit {
        descriptions: Descriptions::Hidden,
        window: Some(COMPACT_LIST_ROWS),
        ..Fit::WHOLE
    },
    Fit {
        chips: false,
        descriptions: Descriptions::Hidden,
        window: Some(COMPACT_LIST_ROWS),
        ..Fit::WHOLE
    },
];

/// A side-by-side question page's rungs: the dim notes line first (the hint
/// still offers `n`), then the panel cut to the option column, then the
/// chip strip. The options are at most a handful of rows, so they never
/// window here.
const PREVIEW_FITS: [Fit; 4] = [
    Fit::WHOLE,
    Fit {
        notes_hint: false,
        ..Fit::WHOLE
    },
    Fit {
        notes_hint: false,
        panel_capped: true,
        ..Fit::WHOLE
    },
    Fit {
        chips: false,
        notes_hint: false,
        panel_capped: true,
        ..Fit::WHOLE
    },
];

/// Build the modal at `width` — the first rung of its page's ladder whose
/// rows fit `height` ([`LIST_FITS`], [`PREVIEW_FITS`]; the review page has
/// only the whole page, its choices already at the bottom where the anchor
/// keeps them), else the last rung, which the paint bottom-anchors. `height`
/// is the terminal's, or the region already reserved for the modal, which
/// builds the same page. Pure and total — an app without an open modal
/// builds empty.
fn ask_build(app: &App, width: u16, height: u16) -> AskBuild {
    let Some(prompt) = app.ask() else {
        return AskBuild::default();
    };
    let fits: &[Fit] = if prompt.on_submit_tab() {
        &[Fit::WHOLE]
    } else if prompt
        .current_question()
        .is_some_and(AskQuestion::has_previews)
    {
        &PREVIEW_FITS
    } else {
        &LIST_FITS
    };
    let mut build = AskBuild::default();
    for fit in fits {
        build = ask_page(app, prompt, width, *fit);
        if build.lines.len() <= usize::from(height) {
            break;
        }
    }
    build
}

/// One page of the modal at `width`, keeping what `fit` keeps: rule → chip
/// strip → the current page → hints → rule.
fn ask_page(app: &App, prompt: &AskPrompt, width: u16, fit: Fit) -> AskBuild {
    let mut lines = vec![rule(width), Line::default()];
    if fit.chips {
        lines.push(chip_strip(prompt, width));
        lines.push(Line::default());
    }
    let mut cursor = None;
    let mut marker = None;
    if prompt.on_submit_tab() {
        build_review_page(prompt, width, &mut lines, &mut marker);
        lines.push(Line::default());
    } else if let Some(question) = prompt.current_question() {
        lines.extend(text_rows(&question.question, Color::Reset, width));
        if question.has_previews() {
            lines.push(Line::default());
            build_preview_page(
                app,
                prompt,
                question,
                width,
                fit,
                &mut lines,
                &mut cursor,
                &mut marker,
            );
            lines.push(Line::default());
        } else {
            // The list brings its own gaps: blank rows, or the overflow
            // slots standing in for them.
            build_list_page(
                app,
                prompt,
                question,
                width,
                fit,
                &mut lines,
                &mut cursor,
                &mut marker,
            );
        }
    } else {
        lines.push(Line::default());
    }
    lines.push(hint_row(&hints(prompt)));
    lines.push(Line::default());
    lines.push(closing_rule(width, prompt.remaining));
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

/// One row of a plain question page: its lines, the caret's `(row, col)`
/// inside them while it is the live entry field, and whether it is the
/// highlighted row (its first line wears the `❯`).
struct RowBlock {
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    selected: bool,
}

/// A compact page's reserved overflow slot at the modal's inset: `↑ N more
/// above` / `↓ N more below` in the dim description ink when `hidden` rows
/// lie that way, else the blank row the page wants there anyway — so the
/// page is one height at every selection.
fn overflow_slot(arrow: &str, hidden: usize, direction: &str) -> Line<'static> {
    if hidden == 0 {
        return Line::default();
    }
    Line::from(vec![
        Span::raw(ASK_INDENT),
        Span::styled(
            format!("{arrow}{hidden} more {direction}"),
            Style::new().fg(ask_desc_color()),
        ),
    ])
}

/// A question page in the plain (no-preview) layout: every row full-width,
/// each option's dim description under its label as `fit` lays it out,
/// between two gaps — or, when `fit` windows the rows, the window around the
/// selection between the overflow slots standing in for those gaps.
#[allow(clippy::too_many_arguments)] // one page, the builder's whole state
fn build_list_page(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    width: u16,
    fit: Fit,
    lines: &mut Vec<Line<'static>>,
    cursor: &mut Option<(usize, usize)>,
    marker: &mut Option<usize>,
) {
    let blocks = list_row_blocks(app, prompt, question, width, fit.descriptions);
    let selected = prompt.row.min(blocks.len().saturating_sub(1));
    let (above, range, below) = match fit.window {
        Some(max) => list_window(blocks.len(), selected, usize::from(max)),
        None => (0, 0..blocks.len(), 0),
    };
    lines.push(overflow_slot(HOOKS_UP_MARKER, above, "above"));
    for block in &blocks[range] {
        if block.selected {
            *marker = Some(lines.len());
        }
        if let Some((row, col)) = block.cursor {
            *cursor = Some((lines.len() + row, col));
        }
        lines.extend(block.lines.iter().cloned());
    }
    lines.push(overflow_slot(HOOKS_DOWN_MARKER, below, "below"));
}

/// Every row of a plain question page, one [`RowBlock`] each, the options'
/// descriptions laid out by `descriptions`.
fn list_row_blocks(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    width: u16,
    descriptions: Descriptions,
) -> Vec<RowBlock> {
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
    let mut blocks = Vec::new();
    for (at, row) in ask_rows(question).iter().enumerate() {
        let selected = prompt.row == at;
        let number = ask_row_number(question, *row);
        let mut block = RowBlock {
            lines: Vec::new(),
            cursor: None,
            selected,
        };
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
                block.lines.push(Line::from(spans));
                let description = question.options[i].description.trim();
                if !description.is_empty() {
                    let desc_room = (width as usize).saturating_sub(cols(&desc_indent)).max(1);
                    let rows = match descriptions {
                        Descriptions::Wrapped => wrap_output(description, desc_room as u16),
                        Descriptions::OneRow => vec![ellipsize(
                            &description.split_whitespace().collect::<Vec<_>>().join(" "),
                            desc_room,
                        )],
                        Descriptions::Hidden => Vec::new(),
                    };
                    for text in rows {
                        block.lines.push(Line::from(vec![
                            Span::raw(desc_indent.clone()),
                            Span::styled(text, Style::new().fg(ask_desc_color())),
                        ]));
                    }
                }
            }
            AskRow::Other => {
                block.lines = other_rows(
                    app,
                    prompt,
                    question,
                    state,
                    selected,
                    room,
                    0,
                    &mut block.cursor,
                );
            }
            AskRow::Confirm => {
                let mut spans = row_prefix(selected, None);
                spans.push(Span::styled(
                    ASK_CONFIRM_LABEL.to_string(),
                    if selected {
                        Style::new().fg(permission_selected_color())
                    } else {
                        Style::default()
                    },
                ));
                block.lines.push(Line::from(spans));
            }
            AskRow::Chat => {
                let mut spans = row_prefix(selected, number);
                spans.push(Span::styled(
                    ASK_CHAT_LABEL.to_string(),
                    if selected {
                        Style::new().fg(permission_selected_color())
                    } else {
                        Style::default()
                    },
                ));
                block.lines.push(Line::from(spans));
            }
        }
        blocks.push(block);
    }
    blocks
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
        Style::new().fg(permission_selected_color())
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
            (ASK_CHECKED, Style::new().fg(ask_picked_color()))
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
                Style::new().fg(ask_picked_color()),
            ));
        }
    }
    vec![Line::from(spans)]
}

/// A question page in the **side-by-side** layout (any option carries a
/// preview): the option rows on the left, the focused option's preview in a
/// bordered panel on the right — cut to the option column's height when
/// `fit` caps it — the notes line beneath the panel (the dim hint only when
/// `fit` keeps it), and the `Chat about this` row below.
#[allow(clippy::too_many_arguments)] // one page, the builder's whole state
fn build_preview_page(
    app: &App,
    prompt: &AskPrompt,
    question: &AskQuestion,
    width: u16,
    fit: Fit,
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
                        Style::new().fg(permission_selected_color())
                    } else {
                        Style::default()
                    },
                ));
                left.push(spans);
            }
            AskRow::Chat => {}
        }
    }
    // The panel's rows: every preview row it may show, or — capped — no
    // taller than the option column (never under one content row).
    let max_rows = if fit.panel_capped {
        left.len().max(3) - 2
    } else {
        ASK_PREVIEW_MAX_ROWS
    };
    let panel = shown
        .and_then(|i| question.options[i].preview.as_deref())
        .map(|preview| panel_lines(preview, panel_width, max_rows))
        .unwrap_or_default();
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
    // rows, continuations aligned under the text — or the dim hint, which a
    // short page leaves to the key hint that offers `n` too.
    let notes_typed = prompt.input_mode == AskInput::Notes || !state.notes.trim().is_empty();
    if notes_typed || fit.notes_hint {
        lines.push(Line::default());
        notes_rows(app, prompt, state, width, panel_x, lines, cursor);
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
                Style::new().fg(permission_selected_color())
            } else {
                Style::default()
            },
        ));
        lines.push(Line::from(spans));
    }
}

/// The side-by-side page's notes rows at the panel's column `panel_x`: the
/// live entry field (the caret seated in it), the typed notes, or the dim
/// hint.
fn notes_rows(
    app: &App,
    prompt: &AskPrompt,
    state: &AskAnswerState,
    width: u16,
    panel_x: usize,
    lines: &mut Vec<Line<'static>>,
    cursor: &mut Option<(usize, usize)>,
) {
    let notes_pad = " ".repeat(panel_x);
    let notes_room = (width as usize)
        .saturating_sub(panel_x + cols(ASK_NOTES_LABEL))
        .max(1);
    let label = || {
        Span::styled(
            ASK_NOTES_LABEL.to_string(),
            Style::new().fg(ask_desc_color()),
        )
    };
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
                Style::new().fg(ask_desc_color()),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw(notes_pad),
            label(),
            Span::raw(truncate_cols(&flat(&state.notes), notes_room).to_string()),
        ]));
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
fn panel_lines(preview: &str, width: usize, max_rows: usize) -> Vec<Vec<Span<'static>>> {
    let dim = Style::new().fg(ask_preview_color());
    if width < 6 {
        return Vec::new();
    }
    let inner = width - 4; // `│ ` + ` │`
    let mut rows: Vec<Vec<Span>> = Vec::new();
    rows.push(vec![Span::styled(
        format!("┌{}┐", "─".repeat(width - 2)),
        dim,
    )]);
    for line in preview.split('\n').take(max_rows.min(ASK_PREVIEW_MAX_ROWS)) {
        let text = truncate_cols(&super::assistant::expand_code_tabs(line), inner).to_string();
        let pad = inner.saturating_sub(cols(&text));
        rows.push(vec![
            Span::styled("│ ".to_string(), dim),
            Span::styled(
                format!("{text}{}", " ".repeat(pad)),
                Style::new().fg(tool_output_color()),
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
        lines.extend(text_rows(ASK_WARNING, ask_warning_color(), width));
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
                Span::styled(
                    ASK_REVIEW_BULLET.to_string(),
                    Style::new().fg(system_color()),
                )
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
                    Style::new().fg(ask_desc_color()),
                )
            } else {
                Span::raw(" ".repeat(cols(ASK_ANSWER_ARROW)))
            };
            lines.push(Line::from(vec![
                Span::raw(answer_indent.clone()),
                lead,
                // Green, so the recorded answer is the row that carries the
                // eye (the user-requested emphasis).
                Span::styled(row, Style::new().fg(ask_answer_color())),
            ]));
        }
        if capped {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(bullet_indent + cols(ASK_ANSWER_ARROW))),
                Span::styled("…".to_string(), Style::new().fg(ask_desc_color())),
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
                Style::new().fg(permission_selected_color())
            } else {
                Style::default()
            },
        ));
        lines.push(Line::from(spans));
    }
}

/// Every row of the open ask modal at `width`, top rule to bottom rule — the
/// first page that fits `height` rows, most complete first (`docs/ask.md`):
/// a short terminal sheds the options' descriptions, then the rows outside
/// a window around the selection, then the chip strip, so the question, the
/// highlighted row and the hint stay on screen. Empty when no modal is open.
/// The page is never cut here: the paint bottom-anchors it into the region
/// ([`render_ask`]), and a page taller than the terminal **flows** its
/// skipped top into the terminal's real scrollback like every framed view
/// (`view_flow`, `docs/view-flow.md`) — the retired top-drop clamp put the
/// chip strip, the question and the first options in no buffer at all.
#[must_use]
pub fn ask_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    ask_build(app, width, height).lines
}

/// The inline live-region height when the ask modal is open, or `None` when
/// it isn't (the caller falls back through the picker chain). The rows
/// [`ask_lines`] builds, clamped to the terminal — the permission prompt's
/// `permission_height` shape.
#[must_use]
pub fn ask_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.ask()?;
    let rows = u16::try_from(ask_lines(app, width, term_height).len()).unwrap_or(u16::MAX);
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
    let build = ask_build(app, width, term_height);
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
    super::view_flow::render_framed_tail(area, buf, ask_lines(app, area.width, area.height));
}

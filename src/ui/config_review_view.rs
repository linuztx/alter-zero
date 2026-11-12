//! The startup review's inline page (`docs/state/config-review.md`) — the hooks
//! menu's sibling: a framed body [`config_review_lines`] builds line by line,
//! so the height falls out as `lines.len()` and [`config_review_height`] /
//! [`render_config_review`] can never disagree.
//!
//! A page is a checkbox list of what would run, grouped by the file it came
//! from, each item showing **verbatim** what runs — a hook's command, a
//! server's command line or URL and its environment — over the two action
//! rows and the key hint. The review opens on the first frame, so the page
//! must never flow into scrollback (a flowed page purge-rebuilds the screen
//! when it closes, which at startup would wipe the terminal's own
//! scrollback): on a terminal too short for it the page sheds rows instead,
//! the compact pickers' way (`docs/ui/view-flow.md`) — the explanation first,
//! then every item but the highlighted one folds to a row, then the list
//! windows around the highlight, then the highlighted item's detail is cut,
//! then the title goes. The actions and the hint stay on every rung.

use super::model_view::{list_window, model_compact_list, model_rule};
use super::theme::*;
use super::wrap::{cols, ellipsize, wrap_output};
use super::*;

use crate::app::{ItemChange, ReviewChoice, ReviewItem, ReviewKind, ReviewPage, ReviewSection};

/// What one rung of the ladder keeps.
#[derive(Debug, Clone, Copy)]
struct Shape {
    /// The explanation under the title.
    info: bool,
    /// Every item but the highlighted one folded to its title row.
    fold: bool,
    /// The list windowed to this many items around the highlight, the file
    /// headings dropped.
    window: Option<usize>,
    /// The highlighted item's detail cut to this many rows.
    cap: Option<usize>,
    /// The title (and, on the servers page, the question under it).
    title: bool,
    /// The key hint wrapped whole — off only on the last rung, which cuts it
    /// to one row.
    full_hint: bool,
}

/// The rungs, most complete first, for a page of `items` items.
fn ladder(items: usize) -> Vec<Shape> {
    let whole = Shape {
        info: true,
        fold: false,
        window: None,
        cap: None,
        title: true,
        full_hint: true,
    };
    let mut out = vec![
        whole,
        Shape {
            info: false,
            ..whole
        },
        Shape {
            info: false,
            fold: true,
            ..whole
        },
    ];
    for shown in (1..items).rev() {
        out.push(Shape {
            info: false,
            fold: true,
            window: Some(shown),
            ..whole
        });
    }
    let lone = Shape {
        info: false,
        fold: true,
        window: Some(1),
        cap: Some(REVIEW_CAPPED_DETAIL_ROWS),
        title: true,
        full_hint: true,
    };
    out.push(lone);
    out.push(Shape {
        cap: Some(1),
        ..lone
    });
    let bare = Shape {
        cap: Some(1),
        title: false,
        ..lone
    };
    out.push(bare);
    out.push(Shape {
        full_hint: false,
        ..bare
    });
    out
}

/// `count` and `noun`, pluralised by an `s`.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The page's headline: what it asks about and how much of it is new.
fn headline(page: &ReviewPage) -> String {
    let (new, changed) = page.counts();
    let all = new + changed;
    match (page.kind, new, changed) {
        (ReviewKind::Hooks, _, 0) => format!("{} found", counted(all, "new hook")),
        (ReviewKind::Hooks, 0, _) => format!("{} changed", counted(all, "hook")),
        (ReviewKind::Hooks, ..) => {
            format!("{all} hooks to review · {new} new, {changed} changed")
        }
        (ReviewKind::McpServers, _, 0) => {
            format!("{} found in this project", counted(all, "new MCP server"))
        }
        (ReviewKind::McpServers, 0, _) => {
            format!("{} changed in this project", counted(all, "MCP server"))
        }
        (ReviewKind::McpServers, ..) => {
            format!("{all} MCP servers to review in this project · {new} new, {changed} changed")
        }
    }
}

/// The title row: the headline in the pages' title dress, then — when the
/// review has more than one page — a dim `1/2`.
fn title_line(page: &ReviewPage, position: Option<(usize, usize)>, width: u16) -> Line<'static> {
    let counter = position.map(|(at, of)| format!("{REVIEW_COUNTER_GAP}{at}/{of}"));
    let room = usize::from(width)
        .saturating_sub(cols(MODEL_INDENT) + counter.as_deref().map_or(0, cols))
        .max(1);
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(super::header::page_title_spans(&ellipsize(
        &headline(page),
        room,
    )));
    if let Some(counter) = counter {
        spans.push(Span::styled(counter, Style::new().fg(model_meta_color())));
    }
    Line::from(spans)
}

/// `text` word-wrapped to inset rows in `style`.
fn wrapped(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    super::wrap::wrap_text(text, room)
        .into_iter()
        .map(|row| Line::from(vec![Span::raw(MODEL_INDENT), Span::styled(row, style)]))
        .collect()
}

/// A file's heading: `User · ~/.alter-zero/hooks.json`, or the path alone.
fn heading_spans(section: &ReviewSection) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    match &section.scope {
        Some(scope) => vec![
            Span::styled(
                scope.clone(),
                Style::new()
                    .fg(model_id_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{REVIEW_SCOPE_SEPARATOR}{}", section.path), dim),
        ],
        None => vec![Span::styled(section.path.clone(), dim)],
    }
}

/// A file's heading, wrapped whole: which file a hook or a server comes
/// from is half of what the page says, and a path's tail is its file name,
/// so it never loses its end to a `…`. The scope word keeps its weight on
/// the first row.
fn heading_lines(section: &ReviewSection, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let scope = section
        .scope
        .as_ref()
        .map(|scope| format!("{scope}{REVIEW_SCOPE_SEPARATOR}"));
    let text = format!("{}{}", scope.as_deref().unwrap_or_default(), section.path);
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    wrap_output(&text, room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            match (&section.scope, i) {
                (Some(word), 0) if row.starts_with(word.as_str()) => {
                    spans.push(Span::styled(
                        word.clone(),
                        Style::new()
                            .fg(model_id_color())
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(row[word.len()..].to_string(), dim));
                }
                _ => spans.push(Span::styled(row, dim)),
            }
            Line::from(spans)
        })
        .collect()
}

/// `spans` cut to the row's room left after `used` columns, the last one
/// `…`-ended when anything was cut.
fn clip_spans(spans: Vec<Span<'static>>, width: u16, used: usize) -> Vec<Span<'static>> {
    let mut room = usize::from(width).saturating_sub(used);
    let mut out = Vec::new();
    for span in spans {
        if room == 0 {
            break;
        }
        let span_cols = cols(&span.content);
        if span_cols <= room {
            room -= span_cols;
            out.push(span);
        } else {
            out.push(Span::styled(ellipsize(&span.content, room), span.style));
            break;
        }
    }
    out
}

/// The columns an item's detail rows start at: under its title, past the
/// marker's and the checkbox's columns.
fn detail_inset() -> usize {
    cols(MODEL_INDENT) + cols(HOOKS_MARKER) + cols(ASK_CHECKED)
}

/// An item's title row: the marker on the highlighted one, the checkbox, the
/// title, and a changed item's badge — then, folded, the first row of what
/// it runs, `…`-cut to what is left of the row.
fn item_head(item: &ReviewItem, highlighted: bool, folded: bool, width: u16) -> Line<'static> {
    let (marker, marker_style, title_style) = if highlighted {
        (
            HOOKS_MARKER.to_string(),
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ".repeat(cols(HOOKS_MARKER)),
            Style::default(),
            Style::new().fg(model_id_color()),
        )
    };
    let (checkbox, checkbox_style) = if item.checked {
        (ASK_CHECKED, Style::new().fg(ask_picked_color()))
    } else {
        (ASK_UNCHECKED, Style::new().fg(model_meta_color()))
    };
    let mut tail = vec![Span::styled(item.title.clone(), title_style)];
    if item.change.is_some() {
        tail.push(Span::styled(
            format!("{REVIEW_BADGE_GAP}{REVIEW_CHANGED_BADGE}"),
            Style::new().fg(ask_warning_color()),
        ));
    }
    if folded && let Some(first) = item.detail.first() {
        tail.push(Span::styled(
            format!("{REVIEW_BADGE_GAP}{first}"),
            Style::new().fg(model_meta_color()),
        ));
    }
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, marker_style),
        Span::styled(checkbox.to_string(), checkbox_style),
    ];
    spans.extend(clip_spans(tail, width, detail_inset()));
    Line::from(spans)
}

/// An item's detail rows — every line of what it runs, word-wrapped whole
/// with its spaces kept (the tail of a command is the part worth reading),
/// then a changed item's `was:` or `changed:` line — each under the title's
/// column.
fn detail_rows(item: &ReviewItem, width: u16) -> Vec<Line<'static>> {
    let inset = " ".repeat(detail_inset());
    let room = usize::from(width).saturating_sub(detail_inset()).max(1) as u16;
    let mut out = Vec::new();
    let push = |text: &str, style: Style, out: &mut Vec<Line<'static>>| {
        for row in wrap_output(text, room) {
            out.push(Line::from(vec![
                Span::raw(inset.clone()),
                Span::styled(row, style),
            ]));
        }
    };
    for (i, line) in item.detail.iter().enumerate() {
        // What runs reads plainly; what rides along with it (a server's
        // `env`) is dim.
        let style = if i == 0 {
            Style::default()
        } else {
            Style::new().fg(model_meta_color())
        };
        push(line, style, &mut out);
    }
    let change = match &item.change {
        Some(ItemChange::Was(previous)) => Some(format!("{REVIEW_WAS_PREFIX}{previous}")),
        Some(ItemChange::Parts(parts)) if !parts.is_empty() => {
            Some(format!("{REVIEW_CHANGED_PREFIX}{}", parts.join(", ")))
        }
        _ => None,
    };
    if let Some(change) = change {
        push(&change, Style::new().fg(model_meta_color()), &mut out);
    }
    out
}

/// `rows` cut to `cap`, the last kept row `…`-ended when anything was cut.
fn capped(mut rows: Vec<Line<'static>>, cap: usize, width: u16) -> Vec<Line<'static>> {
    if rows.len() <= cap {
        return rows;
    }
    rows.truncate(cap.max(1));
    if let Some(last) = rows.last_mut() {
        let text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
        let style = last.spans.last().map(|s| s.style).unwrap_or_default();
        let inset = detail_inset();
        let body = text.get(inset..).unwrap_or_default().trim_end().to_string();
        let room = usize::from(width).saturating_sub(inset).max(1);
        let cut = if cols(&body) < room {
            format!("{body}…")
        } else {
            ellipsize(&format!("{body}…"), room)
        };
        *last = Line::from(vec![Span::raw(" ".repeat(inset)), Span::styled(cut, style)]);
    }
    rows
}

/// The two action rows, the highlighted one marked.
fn action_rows(page: &ReviewPage, width: u16) -> Vec<Line<'static>> {
    let (confirm, decline) = match page.kind {
        ReviewKind::Hooks => (REVIEW_TRUST_LABEL, REVIEW_SKIP_LABEL),
        ReviewKind::McpServers => (REVIEW_ENABLE_LABEL, REVIEW_DISABLE_LABEL),
    };
    [
        (ReviewChoice::Confirm, confirm),
        (ReviewChoice::Decline, decline),
    ]
    .into_iter()
    .map(|(choice, label)| {
        let highlighted = page.action_at(page.selected) == Some(choice);
        let (marker, marker_style, style) = if highlighted {
            (
                HOOKS_MARKER.to_string(),
                Style::new().fg(model_selected_color()),
                Style::new()
                    .fg(model_selected_color())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                " ".repeat(cols(HOOKS_MARKER)),
                Style::default(),
                Style::new().fg(model_id_color()),
            )
        };
        let room = usize::from(width)
            .saturating_sub(cols(MODEL_INDENT) + cols(HOOKS_MARKER))
            .max(1);
        Line::from(vec![
            Span::raw(MODEL_INDENT),
            Span::styled(marker, marker_style),
            Span::styled(ellipsize(label, room), style),
        ])
    })
    .collect()
}

/// The key hint, wrapped rather than cut — it is where the page says what
/// Esc does, which differs between the two pages — except on the last rung,
/// which has nothing else left to shed.
fn hint_lines(page: &ReviewPage, whole: bool, width: u16) -> Vec<Line<'static>> {
    let hint = match page.kind {
        ReviewKind::Hooks => REVIEW_HOOKS_HINT,
        ReviewKind::McpServers => REVIEW_MCP_HINT,
    };
    let style = Style::new().fg(model_meta_color());
    if whole {
        return wrapped(hint, style, width);
    }
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT)).max(1);
    vec![Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(ellipsize(hint, room), style),
    ])]
}

/// One rung's page.
fn build(
    page: &ReviewPage,
    position: Option<(usize, usize)>,
    shape: Shape,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let mut lines = vec![model_rule(width)];
    if shape.title {
        lines.push(title_line(page, position, width));
        if page.kind == ReviewKind::McpServers {
            lines.extend(wrapped(
                REVIEW_MCP_QUESTION,
                Style::new().fg(model_id_color()),
                width,
            ));
        }
        if shape.info {
            let info = match page.kind {
                ReviewKind::Hooks => REVIEW_HOOKS_INFO,
                ReviewKind::McpServers => REVIEW_MCP_INFO,
            };
            lines.extend(wrapped(info, dim, width));
        }
    }
    let items: Vec<(&ReviewSection, &ReviewItem)> = page
        .sections
        .iter()
        .flat_map(|section| section.items.iter().map(move |item| (section, item)))
        .collect();
    match shape.window {
        None => {
            if shape.title {
                lines.push(Line::default());
            }
            let mut index = 0;
            for (s, section) in page.sections.iter().enumerate() {
                if s > 0 {
                    lines.push(Line::default());
                }
                lines.extend(heading_lines(section, width));
                for item in &section.items {
                    let highlighted = index == page.selected;
                    let folded = shape.fold && !highlighted;
                    lines.push(item_head(item, highlighted, folded, width));
                    if !folded {
                        lines.extend(detail_rows(item, width));
                    }
                    index += 1;
                }
            }
            lines.push(Line::default());
        }
        Some(shown) => {
            // An action row keeps the last items in view.
            let focus = page.selected.min(items.len().saturating_sub(1));
            let (above, range, below) = list_window(items.len(), focus, shown);
            let mut window = Vec::new();
            for i in range {
                let (section, item) = items[i];
                let highlighted = i == page.selected;
                window.push(item_head(item, highlighted, !highlighted, width));
                if highlighted {
                    let mut rows = detail_rows(item, width);
                    match shape.cap {
                        Some(cap) => rows = capped(rows, cap, width),
                        None => {
                            // The headings are gone, so the item names its file.
                            let mut from = vec![
                                Span::raw(" ".repeat(detail_inset())),
                                Span::styled(REVIEW_FROM_PREFIX.to_string(), dim),
                            ];
                            from.extend(clip_spans(
                                heading_spans(section),
                                width,
                                detail_inset() + cols(REVIEW_FROM_PREFIX),
                            ));
                            rows.push(Line::from(from));
                        }
                    }
                    window.extend(rows);
                }
            }
            // The reserved overflow pair, under the title — never against a
            // rule (`docs/ui/view-flow.md` *The frame hugs its content*).
            if shape.title && shown < items.len() {
                lines.extend(model_compact_list(above, window, below, width));
            } else {
                if shape.title {
                    lines.push(Line::default());
                }
                lines.extend(window);
                lines.push(Line::default());
            }
        }
    }
    lines.extend(action_rows(page, width));
    lines.push(Line::default());
    lines.extend(hint_lines(page, shape.full_hint, width));
    lines.push(model_rule(width));
    lines
}

/// The whole framed page for a terminal `height` rows tall — the first rung
/// of the ladder that fits it (`model_view::first_page_that_fits`'s
/// rule), empty when no review is open.
#[must_use]
pub fn config_review_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(review) = &app.config_review else {
        return Vec::new();
    };
    let Some(page) = review.pages.first() else {
        return Vec::new();
    };
    let position = (review.total > 1).then(|| (review.position(), review.total));
    let mut lines = Vec::new();
    for shape in ladder(page.item_count()) {
        lines = build(page, position, shape, width);
        if lines.len() <= usize::from(height) {
            break;
        }
    }
    lines
}

/// The live-region height while the review is open, `None` when it isn't —
/// the hooks menu's rule: the streaming strip keeps its rows above the page,
/// the body is the built line count, clamped to the terminal.
#[must_use]
pub fn config_review_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.config_review.as_ref()?;
    let body =
        u16::try_from(config_review_lines(app, width, term_height).len()).unwrap_or(u16::MAX);
    Some(super::layout::view_height(app, width, body, term_height))
}

/// Render the review into the live region in place of the composer —
/// bottom-anchored like every framed view (`docs/ui/view-flow.md`), though its
/// ladder keeps it from ever needing to be. Pure — `render_live` paints this.
pub fn render_config_review(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        config_review_lines(app, area.width, area.height),
    );
}

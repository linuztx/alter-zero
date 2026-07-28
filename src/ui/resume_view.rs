//! The full-screen `/resume` session picker. See `docs/resume.md`.

use super::theme::*;
use super::transcript::{overlay_header, rule_with_label};
use super::wrap::{cols, spans_cols, truncate_cols};
use super::*;

/// One dense session row: the `❯ ` marker (spaces when unselected), the age
/// padded to [`RESUME_AGE_WIDTH`] columns, and the preview truncated to the
/// rest of the width — the whole row lit in the palette's selected colour or
/// dimmed (the selection-by-colour convention). Codex's dense picker row.
fn resume_row(
    session: &crate::session::SessionSummary,
    sort: ResumeSort,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let marker = if selected {
        RESUME_MARKER
    } else {
        RESUME_INDENT
    };
    let secs = match sort {
        ResumeSort::Updated => session.updated_secs,
        ResumeSort::Created => session.created_secs,
    };
    let mut age = truncate_cols(&crate::session::relative_age(secs), RESUME_AGE_WIDTH);
    while cols(&age) < RESUME_AGE_WIDTH {
        age.push(' ');
    }
    let room = (width as usize).saturating_sub(cols(marker) + RESUME_AGE_WIDTH);
    let preview = truncate_cols(&session.preview, room);
    let mut text = format!("{marker}{age}{preview}");
    let style = if selected {
        // Pad to the full width in *columns* so the tint spans the row even
        // with wide CJK/emoji in the preview.
        let pad = (width as usize).saturating_sub(cols(&text));
        text.push_str(&" ".repeat(pad));
        Style::new().fg(MENU_SELECTED_COLOR).bg(RESUME_SELECTED_BG)
    } else {
        Style::new().fg(MENU_DIM_COLOR)
    };
    Line::from(Span::styled(text, style))
}

/// The toolbar's tab label for a filter mode.
const fn resume_filter_label(filter: ResumeFilter) -> &'static str {
    match filter {
        ResumeFilter::Cwd => "Cwd",
        ResumeFilter::All => "All",
    }
}

/// The toolbar's tab label for a sort key.
const fn resume_sort_label(sort: ResumeSort) -> &'static str {
    match sort {
        ResumeSort::Updated => "Updated",
        ResumeSort::Created => "Created",
    }
}

/// One toolbar tab value — codex's `toolbar_value`: the active one bracketed
/// (`[Cwd]`, magenta when its control holds the Tab focus, plain otherwise),
/// an inactive one space-padded and dim.
fn resume_toolbar_value(label: &'static str, active: bool, focused: bool) -> Span<'static> {
    if active {
        let text = format!("[{label}]");
        if focused {
            Span::styled(text, Style::new().fg(RESUME_FOCUS_COLOR))
        } else {
            Span::from(text)
        }
    } else {
        Span::styled(format!(" {label} "), Style::new().fg(MENU_DIM_COLOR))
    }
}

/// The Filter/Sort toolbar spans — codex's `toolbar_line`: dim `Filter:` /
/// `Sort:` labels with their tab pairs (`[Cwd] All`, `[Updated] Created`),
/// or — `compact` — each label with just its active value (`Filter:[Cwd]`).
fn resume_toolbar_spans(picker: &ResumePicker, compact: bool) -> Vec<Span<'static>> {
    let dim = Style::new().fg(MENU_DIM_COLOR);
    let filter_focused = picker.focus == ResumeControl::Filter;
    let sort_focused = picker.focus == ResumeControl::Sort;
    if compact {
        return vec![
            Span::styled("Filter:", dim),
            resume_toolbar_value(resume_filter_label(picker.filter), true, filter_focused),
            Span::styled(RESUME_TOOLBAR_GAP, dim),
            Span::styled("Sort:", dim),
            resume_toolbar_value(resume_sort_label(picker.sort), true, sort_focused),
        ];
    }
    vec![
        Span::styled("Filter: ", dim),
        resume_toolbar_value(
            resume_filter_label(ResumeFilter::Cwd),
            picker.filter == ResumeFilter::Cwd,
            filter_focused,
        ),
        resume_toolbar_value(
            resume_filter_label(ResumeFilter::All),
            picker.filter == ResumeFilter::All,
            filter_focused,
        ),
        Span::styled(RESUME_TOOLBAR_GAP, dim),
        Span::styled("Sort: ", dim),
        resume_toolbar_value(
            resume_sort_label(ResumeSort::Updated),
            picker.sort == ResumeSort::Updated,
            sort_focused,
        ),
        resume_toolbar_value(
            resume_sort_label(ResumeSort::Created),
            picker.sort == ResumeSort::Created,
            sort_focused,
        ),
    ]
}

/// Render the full-screen `/resume` session picker — codex's resume picker,
/// sized down (docs/resume.md): the slash-tiled title, the type-to-search
/// line, the dense session rows (windowed to keep the selection visible, the
/// palette's [`menu_window`]), and the bottom rule carrying
/// `{selected+1}/{total}` over the dim key hints. Pure — `term.rs` paints
/// this onto the alternate screen, like the transcript pager.
pub fn render_resume_picker(area: Rect, buf: &mut Buffer, app: &App) {
    let [
        title_area,
        _,
        search_area,
        _,
        body_area,
        sep_area,
        hint_area,
        _,
    ] = Layout::vertical([
        Constraint::Length(1), // slash-tiled title
        Constraint::Length(1), // gap
        Constraint::Length(1), // search line
        Constraint::Length(1), // gap
        Constraint::Min(0),    // session rows
        Constraint::Length(1), // ─ rule + count
        Constraint::Length(1), // key hints
        Constraint::Length(1), // final blank
    ])
    .areas(area);

    Paragraph::new(overlay_header(RESUME_TITLE, area.width)).render(title_area, buf);

    let picker = app.resume_picker.as_ref();
    let query = picker.map_or("", |p| p.query.as_str());
    let mut search_spans = if query.is_empty() {
        vec![Span::styled(
            format!("{RESUME_INDENT}{RESUME_SEARCH_PLACEHOLDER}"),
            Style::new().fg(MENU_DIM_COLOR),
        )]
    } else {
        vec![
            Span::styled(
                format!("{RESUME_INDENT}{RESUME_SEARCH_PROMPT}"),
                Style::new().fg(MENU_DIM_COLOR),
            ),
            Span::styled(query.to_string(), Style::new().fg(SEARCH_QUERY_COLOR)),
        ]
    };
    // The Filter/Sort toolbar rides the search row's right edge (codex's):
    // the full tab pairs when they fit, the compact active-value form next,
    // dropped entirely on the narrowest screens.
    if let Some(picker) = picker {
        let left = spans_cols(&search_spans);
        let width = area.width as usize;
        let toolbar = [false, true].into_iter().find_map(|compact| {
            let spans = resume_toolbar_spans(picker, compact);
            let cols = spans_cols(&spans);
            (left + RESUME_TOOLBAR_MIN_GAP + cols <= width).then_some((spans, cols))
        });
        if let Some((spans, cols)) = toolbar {
            search_spans.push(Span::from(" ".repeat(width - left - cols)));
            search_spans.extend(spans);
        }
    }
    Paragraph::new(Line::from(search_spans)).render(search_area, buf);

    let matches = picker.map_or_else(Vec::new, |p| p.matches());
    let sort = picker.map_or(ResumeSort::Updated, |p| p.sort);
    let selected = picker
        .map_or(0, |p| p.selected)
        .min(matches.len().saturating_sub(1));
    let rows: Vec<Line> = if matches.is_empty() {
        // Two empty states (codex's): never saved anything, vs a query (or
        // the Cwd filter) leaving nothing to show.
        let placeholder = if picker.is_none_or(|p| p.sessions.is_empty()) {
            RESUME_NO_SESSIONS
        } else {
            RESUME_NO_MATCH
        };
        vec![Line::from(Span::styled(
            format!("{RESUME_INDENT}{placeholder}"),
            Style::new().fg(MENU_DIM_COLOR),
        ))]
    } else {
        let height = (body_area.height as usize).max(1);
        let start = menu_window(matches.len(), selected, height);
        matches
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(index, session)| resume_row(session, sort, index == selected, area.width))
            .collect()
    };
    Paragraph::new(rows).render(body_area, buf);

    // An empty list has no selection to count — the rule stays bare.
    let label = if matches.is_empty() {
        String::new()
    } else {
        format!(" {}/{} ", selected + 1, matches.len())
    };
    Paragraph::new(rule_with_label(area.width, &label)).render(sep_area, buf);
    Paragraph::new(Line::from(Span::styled(
        RESUME_HINTS.to_string(),
        Style::new().fg(TOOL_DIM_COLOR),
    )))
    .render(hint_area, buf);
}

//! A read-only Git workbench: a file navigator beside a numbered unified patch.
//! Rendering only visits the selected file's viewport; horizontal movement is
//! measured in terminal columns and never cuts UTF-8 or a grapheme cluster.

use super::theme::*;
use super::wrap::{cols, ellipsize, spans_cols};
use super::*;
use crate::app::{DiffFilter, DiffFocus, DiffReview};
use crate::git_diff::{DiffFile, DiffLine, DiffLineKind, DiffSection};
use ratatui::layout::Alignment;
use ratatui::widgets::BorderType;

/// Shared geometry keeps keyboard paging aligned with the visible patch rows.
pub fn diff_body_rows(area: Rect) -> usize {
    let (_, body, _) = regions(area);
    body.height.saturating_sub(2 + DIFF_PANE_HEADER_ROWS) as usize
}

fn regions(area: Rect) -> (Rect, Rect, Rect) {
    let inner = area.inner(Margin::new(DIFF_MARGIN, DIFF_MARGIN));
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(DIFF_HEADER_ROWS),
        Constraint::Min(0),
        Constraint::Length(DIFF_FOOTER_ROWS),
    ])
    .areas(inner);
    (header, body, footer)
}

/// Paint the full-screen `/diff` view without reading Git or touching a terminal.
pub fn render_diff_view(app: &App, area: Rect, buf: &mut Buffer) {
    if let Some(review) = app.diff_review() {
        render_review(review, area, buf);
    }
}

fn render_review(review: &DiffReview, area: Rect, buf: &mut Buffer) {
    buf.set_style(area, Style::new().fg(ai_color()));
    let (header, body, footer) = regions(area);
    render_header(review, header, buf);
    let files = review.visible_files();
    if review.loading && review.snapshot.is_none() {
        render_message(
            body,
            buf,
            "Reading Git changes…",
            "Inspecting the working tree and index.",
            false,
        );
    } else if let Some(error) = &review.error {
        render_message(
            body,
            buf,
            "Unable to load Git changes",
            &safe_text(error),
            true,
        );
    } else if review.snapshot.as_ref().is_some_and(|s| s.files.is_empty()) {
        render_message(
            body,
            buf,
            "Working tree clean",
            "No staged, unstaged, or untracked changes.",
            false,
        );
    } else if files.is_empty() {
        render_message(
            body,
            buf,
            "No matching files",
            "Change the filter or clear your search to see more changes.",
            false,
        );
    } else if body.width < DIFF_SPLIT_MIN_WIDTH {
        match review.focus {
            DiffFocus::Files => render_files(review, &files, body, buf),
            DiffFocus::Patch => render_patch(review, body, buf),
        }
    } else {
        let sidebar = (body.width / 3).clamp(DIFF_SIDEBAR_MIN, DIFF_SIDEBAR_MAX);
        let [left, _, right] = Layout::horizontal([
            Constraint::Length(sidebar),
            Constraint::Length(DIFF_PANE_GAP),
            Constraint::Min(0),
        ])
        .areas(body);
        render_files(review, &files, left, buf);
        render_patch(review, right, buf);
    }
    render_footer(review, files.len(), footer, diff_body_rows(area), buf);
}

fn paint_line(spans: Vec<Span<'_>>, area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn line_rect(area: Rect, offset: u16) -> Rect {
    Rect::new(
        area.x,
        area.y.saturating_add(offset),
        area.width,
        u16::from(offset < area.height),
    )
}

fn joined_row(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: u16,
) -> Vec<Span<'static>> {
    let used = spans_cols(&left) + spans_cols(&right);
    if used < width as usize {
        left.push(Span::raw(" ".repeat(width as usize - used)));
        left.extend(right);
    }
    left
}

fn change_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "change" } else { "changes" })
}

fn render_header(review: &DiffReview, area: Rect, buf: &mut Buffer) {
    let accent = Style::new().fg(menu_selected_color());
    let dim = Style::new().fg(tool_dim_color());
    let snapshot = review.snapshot.as_ref();
    let all_files = snapshot.map_or(&[][..], |s| s.files.as_slice());
    let additions: usize = all_files.iter().map(|f| f.additions).sum();
    let deletions: usize = all_files.iter().map(|f| f.deletions).sum();
    let title = vec![
        Span::styled(
            DIFF_BADGE,
            accent.bg(diff_pane_bg()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  Git changes",
            Style::new().fg(ai_color()).add_modifier(Modifier::BOLD),
        ),
    ];
    let mut totals = vec![
        Span::styled(change_count(all_files.len()), dim),
        Span::raw("  "),
        Span::styled(
            format!("+{additions}"),
            Style::new().fg(tool_diff_add_color()),
        ),
        Span::raw("  "),
        Span::styled(
            format!("−{deletions}"),
            Style::new().fg(tool_diff_del_color()),
        ),
    ];
    if all_files.iter().any(|file| file.truncated) {
        totals.push(Span::styled(
            "  preview totals",
            Style::new().fg(status_retry_color()),
        ));
    }
    paint_line(
        joined_row(title, totals, area.width),
        line_rect(area, 0),
        buf,
    );

    let context = if let Some(snapshot) = snapshot {
        let branch = safe_text(&snapshot.branch);
        let branch_width = cols(&branch).min(area.width as usize / 3);
        let branch = ellipsize(&branch, branch_width);
        let available = (area.width as usize).saturating_sub(cols(&branch) + cols(DIFF_SEPARATOR));
        vec![
            Span::styled(
                ellipsize(&safe_text(&snapshot.root.to_string_lossy()), available),
                dim,
            ),
            Span::styled(DIFF_SEPARATOR, dim),
            Span::styled(branch, accent),
        ]
    } else {
        vec![Span::styled("Working tree and index", dim)]
    };
    paint_line(context, line_rect(area, 1), buf);

    let filters = [
        DiffFilter::All,
        DiffFilter::Unstaged,
        DiffFilter::Staged,
        DiffFilter::Untracked,
    ];
    let mut chips = Vec::new();
    for (index, filter) in filters.into_iter().enumerate() {
        if area.width < DIFF_COMPACT_FILTER_WIDTH && filter != review.filter {
            continue;
        }
        let count = all_files
            .iter()
            .filter(|f| match filter {
                DiffFilter::All => true,
                DiffFilter::Unstaged => f.section == DiffSection::Unstaged,
                DiffFilter::Staged => f.section == DiffSection::Staged,
                DiffFilter::Untracked => f.section == DiffSection::Untracked,
            })
            .count();
        let style = if filter == review.filter {
            Style::new()
                .fg(footer_focus_fg())
                .bg(menu_selected_color())
                .add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        chips.push(Span::styled(
            format!(" {} {} {} ", index + 1, filter.label(), count),
            style,
        ));
        chips.push(Span::raw(" "));
    }
    if area.width < DIFF_COMPACT_FILTER_WIDTH {
        chips.push(Span::styled(" 1–4 filters", dim));
    }
    paint_line(chips, line_rect(area, 2), buf);

    let mut search = vec![Span::styled(
        " / ",
        if review.searching { accent } else { dim },
    )];
    if review.query.is_empty() && !review.searching {
        search.push(Span::styled("Filter files…   / to search", dim));
    } else {
        // Keep the end of a long query visible while typing.
        let query = safe_text(&review.query);
        let room = (area.width as usize).saturating_sub(5);
        let start = cols(&query).saturating_sub(room);
        search.push(Span::styled(column_slice(&query, start, room), accent));
        if review.searching {
            search.push(Span::styled(DIFF_CARET, accent));
        }
    }
    if review.loading {
        search = joined_row(
            search,
            vec![Span::styled("Refreshing… ", accent)],
            area.width,
        );
    }
    paint_line(search, line_rect(area, 3), buf);
}

fn pane(area: Rect, buf: &mut Buffer, title: &str, focused: bool) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(diff_border_color(focused)))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::new()
                .fg(if focused {
                    menu_selected_color()
                } else {
                    ai_color()
                })
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

fn render_files(review: &DiffReview, files: &[&DiffFile], area: Rect, buf: &mut Buffer) {
    let inner = pane(area, buf, "Files", review.focus == DiffFocus::Files);
    let dim = Style::new().fg(tool_dim_color());
    paint_line(
        vec![Span::styled(format!(" {}", change_count(files.len())), dim)],
        line_rect(inner, 0),
        buf,
    );
    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(DIFF_PANE_HEADER_ROWS),
        inner.width,
        inner.height.saturating_sub(DIFF_PANE_HEADER_ROWS),
    );
    let capacity = (body.height as usize / DIFF_FILE_ROWS).max(1);
    let selected = review.selected.min(files.len().saturating_sub(1));
    let start = centered_window(files.len(), selected, capacity);
    for (slot, (index, file)) in files
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let offset = (slot * DIFF_FILE_ROWS) as u16;
        if offset >= body.height {
            break;
        }
        let selected = index == selected;
        let style = Style::new().fg(if selected {
            ai_color()
        } else {
            tool_output_color()
        });
        if selected {
            buf.set_style(
                Rect::new(
                    body.x,
                    body.y + offset,
                    body.width,
                    (body.height - offset).min(DIFF_FILE_ROWS as u16),
                ),
                Style::new().bg(resume_selected_bg()),
            );
        }
        let status = file.status.as_str();
        let color = match status {
            "A" | "?" => tool_diff_add_color(),
            "D" => tool_diff_del_color(),
            "U" => error_color(),
            "!" => status_retry_color(),
            _ => menu_selected_color(),
        };
        let marker = if selected { DIFF_SELECTED } else { " " };
        let name = file
            .path
            .file_name()
            .unwrap_or(file.path.as_os_str())
            .to_string_lossy();
        let counts = format!(" +{} −{} ", file.additions, file.deletions);
        let reserve = if body.width as usize > cols(&counts) + 14 {
            cols(&counts)
        } else {
            0
        };
        let room = (body.width as usize).saturating_sub(5 + reserve);
        let mut row = vec![
            Span::styled(marker, Style::new().fg(menu_selected_color())),
            Span::styled(format!(" {} ", safe_text(status)), Style::new().fg(color)),
            Span::styled(
                ellipsize(&safe_text(&name), room),
                style.add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ];
        if reserve > 0 {
            row = joined_row(
                row,
                vec![
                    Span::styled(
                        format!(" +{}", file.additions),
                        Style::new().fg(tool_diff_add_color()),
                    ),
                    Span::styled(
                        format!(" −{} ", file.deletions),
                        Style::new().fg(tool_diff_del_color()),
                    ),
                ],
                body.width,
            );
        }
        paint_line(row, line_rect(body, offset), buf);

        let parent = file
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| safe_text(&p.to_string_lossy()));
        let detail = parent.map_or_else(
            || file.section.label().to_string(),
            |p| format!("{}{DIFF_SEPARATOR}{p}", file.section.label()),
        );
        paint_line(
            vec![
                Span::styled(marker, Style::new().fg(menu_selected_color())),
                Span::styled(
                    format!(
                        "    {}",
                        ellipsize(&detail, (body.width as usize).saturating_sub(5))
                    ),
                    dim,
                ),
            ],
            line_rect(body, offset + 1),
            buf,
        );
    }
    render_scrollbar(area, start, files.len(), capacity, buf);
}

fn render_patch(review: &DiffReview, area: Rect, buf: &mut Buffer) {
    let inner = pane(area, buf, "Patch", review.focus == DiffFocus::Patch);
    let Some(file) = review.selected_file() else {
        return;
    };
    let dim = Style::new().fg(tool_dim_color());
    let title = if let Some(old) = &file.old_path {
        format!(
            "{} → {}",
            safe_text(&old.to_string_lossy()),
            safe_text(&file.path.to_string_lossy())
        )
    } else {
        safe_text(&file.path.to_string_lossy())
    };
    let mut metadata = vec![Span::styled(
        format!(
            " {}",
            ellipsize(&title, (inner.width as usize).saturating_sub(2))
        ),
        Style::new().fg(ai_color()).add_modifier(Modifier::BOLD),
    )];
    metadata = joined_row(
        metadata,
        vec![Span::styled(format!(" {} ", file.section.label()), dim)],
        inner.width,
    );
    paint_line(metadata, line_rect(inner, 0), buf);
    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(DIFF_PANE_HEADER_ROWS),
        inner.width,
        inner.height.saturating_sub(DIFF_PANE_HEADER_ROWS),
    );
    if file.lines.is_empty() {
        render_message(
            body,
            buf,
            "No textual changes",
            "This change has no text patch to display.",
            false,
        );
        return;
    }
    let start = review
        .scroll
        .min(file.lines.len().saturating_sub(body.height as usize));
    let digits = file
        .lines
        .iter()
        .flat_map(|l| l.old_line.into_iter().chain(l.new_line))
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(DIFF_MIN_GUTTER);
    for (offset, line) in file
        .lines
        .iter()
        .skip(start)
        .take(body.height as usize)
        .enumerate()
    {
        render_patch_line(
            line,
            digits,
            review.horizontal,
            line_rect(body, offset as u16),
            buf,
        );
    }
    render_scrollbar(area, start, file.lines.len(), body.height as usize, buf);
}

fn render_patch_line(
    line: &DiffLine,
    digits: usize,
    horizontal: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let style = match line.kind {
        DiffLineKind::Addition => Style::new()
            .fg(tool_diff_add_color())
            .bg(tool_diff_add_bg()),
        DiffLineKind::Deletion => Style::new()
            .fg(tool_diff_del_color())
            .bg(tool_diff_del_bg()),
        DiffLineKind::Hunk => Style::new().fg(menu_selected_color()).bg(diff_pane_bg()),
        DiffLineKind::Header => Style::new().fg(tool_dim_color()),
        DiffLineKind::Notice => Style::new().fg(status_retry_color()),
        DiffLineKind::Context => Style::new().fg(tool_output_color()),
    };
    buf.set_style(area, style);
    if matches!(
        line.kind,
        DiffLineKind::Hunk | DiffLineKind::Header | DiffLineKind::Notice
    ) {
        paint_line(
            vec![Span::styled(
                format!(
                    " {}",
                    column_slice(
                        &safe_text(&line.text),
                        horizontal,
                        (area.width as usize).saturating_sub(1)
                    )
                ),
                style,
            )],
            area,
            buf,
        );
        return;
    }
    let old = line.old_line.map_or_else(String::new, |n| n.to_string());
    let new = line.new_line.map_or_else(String::new, |n| n.to_string());
    let mark = match line.kind {
        DiffLineKind::Addition => "+",
        DiffLineKind::Deletion => "−",
        _ => " ",
    };
    let gutter = format!("{old:>digits$} {new:>digits$} {DIFF_GUTTER_SEPARATOR} {mark} ");
    let room = (area.width as usize).saturating_sub(cols(&gutter));
    let text = column_slice(&safe_text(&line.text), horizontal, room);
    paint_line(
        vec![
            Span::styled(
                gutter,
                style.fg(if matches!(line.kind, DiffLineKind::Context) {
                    tool_dim_color()
                } else {
                    style.fg.unwrap_or_else(ai_color)
                }),
            ),
            Span::styled(text, style),
        ],
        area,
        buf,
    );
}

fn render_scrollbar(area: Rect, start: usize, total: usize, visible: usize, buf: &mut Buffer) {
    let height = area.height.saturating_sub(2) as usize;
    if total <= visible || height == 0 || area.width == 0 {
        return;
    }
    let thumb = (height * visible / total).max(1);
    let top = (height - thumb) * start / total.saturating_sub(visible).max(1);
    for offset in 0..height {
        let active = (top..top + thumb).contains(&offset);
        buf.set_string(
            area.right() - 1,
            area.y + 1 + offset as u16,
            if active {
                DIFF_SCROLL_THUMB
            } else {
                DIFF_SCROLL_TRACK
            },
            Style::new().fg(if active {
                menu_selected_color()
            } else {
                tool_dim_color()
            }),
        );
    }
}

fn render_message(area: Rect, buf: &mut Buffer, title: &str, detail: &str, error: bool) {
    let inner = area.inner(Margin::new(1, 0));
    let y = inner.height.saturating_sub(3) / 2;
    Paragraph::new(title)
        .alignment(Alignment::Center)
        .style(
            Style::new()
                .fg(if error {
                    error_color()
                } else {
                    menu_selected_color()
                })
                .add_modifier(Modifier::BOLD),
        )
        .render(line_rect(inner, y), buf);
    Paragraph::new(ellipsize(detail, inner.width as usize))
        .alignment(Alignment::Center)
        .style(Style::new().fg(tool_dim_color()))
        .render(line_rect(inner, y.saturating_add(2)), buf);
}

fn render_footer(review: &DiffReview, count: usize, area: Rect, rows: usize, buf: &mut Buffer) {
    let dim = Style::new().fg(tool_dim_color());
    let focus = match review.focus {
        DiffFocus::Files => "Files focus",
        DiffFocus::Patch => "Patch focus",
    };
    let mut left = vec![
        Span::styled(format!(" {focus}"), Style::new().fg(menu_selected_color())),
        Span::styled(DIFF_SEPARATOR, dim),
        Span::styled("read-only", dim),
    ];
    if count > 0 {
        left.push(Span::styled(
            format!(
                "{DIFF_SEPARATOR}{}/{}",
                review.selected.min(count - 1) + 1,
                count
            ),
            dim,
        ));
    }
    let mut right = Vec::new();
    if let Some(file) = review.selected_file() {
        if file.truncated {
            right.push(Span::styled(
                "Preview limited  ",
                Style::new().fg(status_retry_color()),
            ));
        }
        let start = review.scroll.min(file.lines.len().saturating_sub(rows));
        let end = (start + rows).min(file.lines.len());
        if !file.lines.is_empty() {
            right.push(Span::styled(
                format!("{}–{end}/{} ", start + 1, file.lines.len()),
                dim,
            ));
        }
    }
    paint_line(joined_row(left, right, area.width), line_rect(area, 0), buf);
    // Choose by actual display width, preserving a complete close hint even
    // when a resize makes the richer key legend stop fitting.
    let hint_sets: &[&[(&str, &str)]] = if review.searching {
        &[
            &[
                ("Type", " filter files"),
                ("Enter", " apply"),
                ("Esc", " cancel"),
            ],
            &[("Enter", " apply"), ("Esc", " cancel")],
            &[("Esc", " cancel")],
        ]
    } else {
        &[
            &[
                ("Tab", " pane"),
                ("↑↓/jk", " move"),
                ("←→", " pan"),
                ("[ ]", " file"),
                ("n/N", " hunk"),
                ("/", " search"),
                ("1–4", " filter"),
                ("r", " refresh"),
                ("q/Esc", " close"),
            ],
            &[
                ("Tab", " pane"),
                ("↑↓", " move"),
                ("n/N", " hunk"),
                ("/", " search"),
                ("r", " refresh"),
                ("Esc", " close"),
            ],
            &[
                ("Tab", " pane"),
                ("↑↓", " move"),
                ("/", " search"),
                ("r", " refresh"),
                ("Esc", " close"),
            ],
            &[("Tab", " pane"), ("/", " search"), ("Esc", " close")],
            &[("Tab", " pane"), ("Esc", " close")],
            &[("Esc", " close")],
        ]
    };
    let hints = hint_sets
        .iter()
        .copied()
        .find(|hints| {
            1 + hints
                .iter()
                .map(|(key, label)| cols(key) + cols(label))
                .sum::<usize>()
                + hints.len().saturating_sub(1) * cols(DIFF_SEPARATOR)
                <= area.width as usize
        })
        .unwrap_or(&[("Esc", "")]);
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(DIFF_SEPARATOR, dim));
        }
        spans.push(Span::styled(*key, Style::new().fg(ai_color())));
        spans.push(Span::styled(*label, dim));
    }
    paint_line(spans, line_rect(area, 1), buf);
}

/// Show control characters literally, never interpreting repository content as
/// terminal commands. Tabs preserve indentation as four display spaces.
fn safe_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => safe.push_str(&" ".repeat(CODE_TAB_WIDTH)),
            '\n' => safe.push_str("\\n"),
            '\r' => safe.push_str("\\r"),
            ch if ch.is_control() => safe.extend(ch.escape_default()),
            ch => safe.push(ch),
        }
    }
    safe
}

/// A left-clipped wide grapheme occupies blank cells for its remaining width.
/// This preserves the column grid while avoiding a broken half of a glyph.
fn column_slice(text: &str, start: usize, width: usize) -> String {
    let mut result = String::new();
    let mut position = 0;
    let end = start.saturating_add(width);
    for grapheme in text.graphemes(true) {
        let next = position + cols(grapheme);
        if position >= end {
            break;
        }
        if position >= start && next <= end {
            result.push_str(grapheme);
        } else if next > start && position < end {
            result.push_str(&" ".repeat(next.min(end) - position.max(start)));
        }
        position = next;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::DiffSnapshot;

    fn review_fixture() -> DiffReview {
        let mut review = DiffReview::default();
        review.snapshot.replace(DiffSnapshot {
            root: "/workspace/engine".into(),
            branch: "feature/review".to_string(),
            files: vec![DiffFile {
                path: "src/worker.rs".into(),
                old_path: None,
                section: DiffSection::Unstaged,
                status: "M".to_string(),
                additions: 1,
                deletions: 1,
                lines: crate::git_diff::parse_patch(
                    "@@ -1,2 +1,2 @@\n fn main() {\n-old_call();\n+new_call();\n",
                ),
                truncated: false,
            }],
        });
        review
    }

    fn screen_text(buf: &Buffer) -> String {
        (buf.area.y..buf.area.bottom())
            .map(|row| row_text(buf, row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row_text(buf: &Buffer, row: u16) -> String {
        let mut text = String::new();
        let mut x = buf.area.x;
        while x < buf.area.right() {
            let symbol = buf[(x, row)].symbol();
            text.push_str(symbol);
            x += cols(symbol).max(1) as u16;
        }
        text
    }

    #[test]
    fn horizontal_pan_preserves_wide_and_combining_graphemes() {
        assert_eq!(column_slice("a界e\u{301}xyz", 2, 4), " e\u{301}xy");
        assert_eq!(column_slice("a👩‍💻b", 1, 2), "👩‍💻");
        assert_eq!(column_slice("a界b", 0, 2), "a ");
        assert_eq!(column_slice("anything", usize::MAX, 10), "");
    }

    #[test]
    fn repository_control_characters_are_visible_instead_of_terminal_sequences() {
        assert_eq!(
            safe_text("\u{1b}[31m\tfile\nname\r"),
            "\\u{1b}[31m    file\\nname\\r"
        );
    }

    #[test]
    fn patch_rows_keep_gutters_fixed_and_tint_the_whole_row() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let line = DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(42),
            text: "prefix界end".to_string(),
        };
        render_patch_line(&line, 3, 6, area, &mut buf);
        assert!(row_text(&buf, 0).contains("42 │ + 界end"));
        assert_eq!(buf[(39, 0)].bg, tool_diff_add_bg());
        assert_eq!(buf[(0, 0)].bg, tool_diff_add_bg());
    }

    #[test]
    fn complete_view_shows_repository_filters_file_context_and_patch() {
        let review = review_fixture();
        let area = Rect::new(0, 0, 120, 28);
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        let text = screen_text(&buf);
        for expected in [
            "Git changes",
            "feature/review",
            "1 All 1",
            "2 Unstaged 1",
            "3 Staged 0",
            "4 Untracked 0",
            "Files",
            "Patch",
            "worker.rs",
            "src/worker.rs",
            "Unstaged · src",
            "old_call();",
            "new_call();",
            "Files focus",
            "read-only",
            "n/N",
            "refresh",
        ] {
            assert!(text.contains(expected), "missing {expected:?}:\n{text}");
        }
    }

    #[test]
    fn compact_view_gives_the_focused_pane_the_full_width() {
        let mut review = review_fixture();
        let area = Rect::new(0, 0, 56, 20);
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        let files = screen_text(&buf);
        assert!(files.contains("Files"));
        assert!(!files.contains("old_call"));
        review.focus = DiffFocus::Patch;
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        let patch = screen_text(&buf);
        assert!(patch.contains("Patch focus"));
        assert!(patch.contains("old_call"));
        assert!(!patch.contains("Files focus"));
    }

    #[test]
    fn resizing_preserves_a_complete_close_hint() {
        let review = review_fixture();
        for width in [14, 32, 48, 56, 64, 80, 120] {
            let area = Rect::new(0, 0, width, 20);
            let mut buf = Buffer::empty(area);
            render_review(&review, area, &mut buf);
            assert!(row_text(&buf, 18).contains("Esc close"), "width {width}");
        }
    }

    #[test]
    fn empty_clean_loading_and_failed_states_are_distinct() {
        let area = Rect::new(0, 0, 90, 24);
        let mut review = review_fixture();
        review.query = "does-not-exist".to_string();
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        assert!(screen_text(&buf).contains("No matching files"));

        review.snapshot.as_mut().unwrap().files.clear();
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        assert!(screen_text(&buf).contains("Working tree clean"));

        review.snapshot = None;
        review.loading = true;
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        assert!(screen_text(&buf).contains("Reading Git changes…"));

        review.loading = false;
        review.error = Some("Git is unavailable".to_string());
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        assert!(screen_text(&buf).contains("Unable to load Git changes"));
        assert!(screen_text(&buf).contains("Git is unavailable"));
    }

    #[test]
    fn scrolling_renders_only_the_patch_viewport() {
        let mut review = review_fixture();
        let file = &mut review.snapshot.as_mut().unwrap().files[0];
        file.lines = (0..100)
            .map(|n| DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(n + 1),
                new_line: Some(n + 1),
                text: format!("source_row_{n:03}"),
            })
            .collect();
        review.scroll = 50;
        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        let text = screen_text(&buf);
        assert!(text.contains("source_row_050"));
        let last = 50 + diff_body_rows(area) - 1;
        assert!(text.contains(&format!("source_row_{last:03}")));
        assert!(!text.contains("source_row_049"));
        assert!(!text.contains(&format!("source_row_{:03}", last + 1)));
    }

    #[test]
    fn large_old_line_numbers_keep_additions_and_deletions_aligned() {
        let mut review = review_fixture();
        review.snapshot.as_mut().unwrap().files[0].lines = vec![
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(10_000),
                new_line: None,
                text: "old_source".to_string(),
            },
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(1),
                text: "new_source".to_string(),
            },
        ];
        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        render_review(&review, area, &mut buf);
        let text = screen_text(&buf);
        let old = text.lines().find_map(|row| row.find("old_source")).unwrap();
        let new = text.lines().find_map(|row| row.find("new_source")).unwrap();
        // Compare terminal columns, not UTF-8 byte positions (the deletion
        // marker is the three-byte minus glyph while addition is ASCII).
        let old_row = text.lines().find(|row| row.contains("old_source")).unwrap();
        let new_row = text.lines().find(|row| row.contains("new_source")).unwrap();
        assert_eq!(cols(&old_row[..old]), cols(&new_row[..new]));
    }

    #[test]
    fn tiny_patch_and_empty_states_fit_without_panicking() {
        let mut review = review_fixture();
        for width in 0..20 {
            for height in 0..12 {
                let area = Rect::new(0, 0, width, height);
                let mut buf = Buffer::empty(area);
                render_message(area, &mut buf, "Working tree clean", "No changes", false);
                let (_, body, _) = regions(area);
                assert_eq!(diff_body_rows(area), body.height.saturating_sub(3) as usize);
                render_review(&review, area, &mut buf);
                review.focus = DiffFocus::Patch;
                render_review(&review, area, &mut buf);
            }
        }
    }
}

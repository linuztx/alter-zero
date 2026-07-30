//! The `read`/`write`/`edit` file cell: codex's `diff_render` look in the `⎿`
//! gutter — dim line numbers, green/red signs, syntax-highlighted content, and
//! added/removed rows on background tints. See `docs/tools.md`.

use super::assistant::code_content_rows;
use super::theme::*;
use super::tool::{more_hint_line, tool_output_lines};
use super::wrap::{cols, truncate_cols};
use super::*;

/// A `⎿` gutter row with an explicit content colour (`None` → dim): the
/// **first** row (index 0) opens with the [`TOOL_RESULT_PREFIX`] corner,
/// continuation rows indent by its display width so the text aligns under it
/// (Claude-Code's exec-cell output style). The shared basis for the dim
/// [`result_row`] and the diff-coloured rows.
pub(super) fn gutter_row(index: usize, text: String, color: Option<Color>) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let prefix = if index == 0 {
        TOOL_RESULT_PREFIX.to_string()
    } else {
        " ".repeat(cols(TOOL_RESULT_PREFIX))
    };
    let content_style = color.map_or(dim, |c| Style::new().fg(c));
    Line::from(vec![
        Span::styled(prefix, dim),
        Span::styled(text, content_style),
    ])
}

/// Is this a model tool whose output is a diff — so its `⎿` rows get `+`/`-`
/// diff colouring ([`diff_result_row`])? See [`DIFF_TOOL_NAMES`].
pub(super) fn is_diff_tool(tool: &ToolCall) -> bool {
    !tool.shell && DIFF_TOOL_NAMES.contains(&tool.name.as_str())
}

/// The diff colour for a source line by its leading marker — `+` green, `-`
/// red, everything else (context, the summary header) dim (`None`).
pub(super) fn diff_line_color(line: &str) -> Option<Color> {
    match line.chars().next() {
        Some('+') => Some(TOOL_DIFF_ADD_COLOR),
        Some('-') => Some(TOOL_DIFF_DEL_COLOR),
        _ => None,
    }
}

/// One parsed row of a numbered `Created …`/`Updated …` body — the
/// `llm::tools` gutter format (`{n:>W} {text}` / `{n:>W} {sign}{text}`) a
/// `write`/`edit` cell restyles ([`file_cell_lines`]).
enum FileRow {
    /// A numbered content line: the raw right-aligned number `gutter`, the
    /// diff `sign` (`None` in a `Created` body, which has no sign column),
    /// and the content `text`.
    Numbered {
        gutter: String,
        sign: Option<char>,
        text: String,
    },
    /// The `⋮` gap row between diff hunks (kept raw for display).
    Gap(String),
    /// A note row (the `… N more lines` cap tail) — rendered dim.
    Note(String),
}

/// Parse one body row of a numbered file cell; `signed` follows the head line
/// (`Updated` bodies carry a `+`/`-`/space sign column, `Created` bodies
/// don't). `None` means the row isn't in the format — the whole cell then
/// keeps the legacy first-char diff colouring (old sessions, error bodies).
fn parse_file_row(line: &str, signed: bool) -> Option<FileRow> {
    let trimmed = line.trim_start_matches(' ');
    if trimmed.starts_with('…') {
        return Some(FileRow::Note(line.to_string()));
    }
    if trimmed == "⋮" {
        return Some(FileRow::Gap(line.to_string()));
    }
    let indent = line.len() - trimmed.len();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let gutter = line[..indent + digits].to_string();
    let rest = &line[indent + digits..];
    if rest.is_empty() {
        // An empty content row whose trailing spaces something stripped.
        return Some(FileRow::Numbered {
            gutter,
            sign: signed.then_some(' '),
            text: String::new(),
        });
    }
    let rest = rest.strip_prefix(' ')?;
    if signed {
        let sign = rest.chars().next().unwrap_or(' ');
        if !matches!(sign, '+' | '-' | ' ') {
            return None;
        }
        Some(FileRow::Numbered {
            gutter,
            sign: Some(sign),
            text: rest.get(1..).unwrap_or("").to_string(),
        })
    } else {
        Some(FileRow::Numbered {
            gutter,
            sign: None,
            text: rest.to_string(),
        })
    }
}

/// Parse a `read`/`write`/`edit` cell's output as a numbered file change: the
/// summary head line for the `⎿` corner and the parsed body rows. A
/// `write`/`edit` cell carries its head in the output (`Created …`/`Updated
/// …`) over a signed (edit) or unsigned (created) body; a `read` cell has
/// **no** head — its whole output is unsigned numbered content, so the
/// `Read N lines` summary is synthesized here. `None` when the cell isn't a
/// finished file tool or the output isn't in the `llm::tools` gutter format (a
/// placeholder like `(file is empty)`, an old rollout, an error body) — the
/// caller keeps the legacy rendering.
fn parse_file_cell(tool: &ToolCall) -> Option<(String, Vec<FileRow>)> {
    if tool.shell || tool.status == ToolStatus::Running {
        return None;
    }
    let lines = tool_output_lines(tool);
    match tool.name.as_str() {
        "Read" => {
            if lines.is_empty() {
                return None;
            }
            // Every line must parse as a numbered content row; a placeholder
            // (`(file is empty)`, offset-past-end) doesn't, and falls back.
            let rows: Vec<FileRow> = lines
                .iter()
                .map(|l| parse_file_row(l, false))
                .collect::<Option<_>>()?;
            if !rows.iter().all(|r| matches!(r, FileRow::Numbered { .. })) {
                return None;
            }
            let n = lines.len();
            let head = format!("Read {n} line{}", if n == 1 { "" } else { "s" });
            Some((head, rows))
        }
        "Write" | "Edit" => {
            let (head, body) = lines.split_first()?;
            let signed = if head.starts_with("Updated ") {
                true
            } else if head.starts_with("Created ") {
                false
            } else {
                return None;
            };
            let rows: Vec<FileRow> = body
                .iter()
                .map(|l| parse_file_row(l, signed))
                .collect::<Option<_>>()?;
            Some((head.clone(), rows))
        }
        _ => None,
    }
}

/// The highlight language for a file cell — the extension of the path in the
/// cell's args (`index.html` → `html`); `None` (plain text) without one.
fn file_cell_lang(args: &str) -> Option<&str> {
    let name = args.rsplit(['/', '\\']).next().unwrap_or(args);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
}

/// The summary head of a file cell (`Created …`/`Updated …`/`Read N lines`) in
/// the white output colour ([`TOOL_OUTPUT_COLOR`]) so it's as noticeable as the
/// output, with its `(+A -D)` counts coloured green/red (codex's header counts);
/// all-white when there are no counts.
fn file_summary_spans(head: &str) -> Vec<Span<'static>> {
    let text = Style::new().fg(TOOL_OUTPUT_COLOR);
    if let Some(open) = head.rfind("(+") {
        let counts = head[open..]
            .strip_prefix("(+")
            .and_then(|t| t.strip_suffix(')'))
            .and_then(|t| t.split_once(" -"));
        if let Some((a, d)) = counts
            && !a.is_empty()
            && !d.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && d.chars().all(|c| c.is_ascii_digit())
        {
            return vec![
                Span::styled(format!("{}(", &head[..open]), text),
                Span::styled(format!("+{a}"), Style::new().fg(TOOL_DIFF_ADD_COLOR)),
                Span::styled(" ".to_string(), text),
                Span::styled(format!("-{d}"), Style::new().fg(TOOL_DIFF_DEL_COLOR)),
                Span::styled(")".to_string(), text),
            ];
        }
    }
    vec![Span::styled(head.to_string(), text)]
}

/// The left indent (display columns) of a numbered file cell's body — the
/// line-number gutter sits **one column past** the `⎿` corner content
/// ([`TOOL_RESULT_PREFIX`]), matching Claude-Code's file-change look (the
/// numbers land just inside the corner). The `⋮` hunk gaps and `…` notes align
/// here too. See `docs/tools.md`.
fn file_body_indent() -> usize {
    cols(TOOL_RESULT_PREFIX) + 1
}

/// Build the display rows for one numbered source row: a dim right-aligned
/// line number, the `+`/`-` sign in the diff colour, and the content
/// syntax-highlighted — added rows on the dark-green tint, removed rows
/// (their text dimmed) on the dark-red one, both padded to the full width.
/// Long content wraps ([`code_content_rows`]); continuations indent under the
/// content column and keep the tint.
fn numbered_row_lines(
    gutter: &str,
    sign: Option<char>,
    segs: &[highlight::Seg],
    indent_cols: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(indent_cols);
    let (bg, dim_content) = match sign {
        Some('+') => (Some(TOOL_DIFF_ADD_BG), false),
        Some('-') => (Some(TOOL_DIFF_DEL_BG), true),
        _ => (None, false),
    };
    let sign_style = match sign {
        Some('+') => Style::new().fg(TOOL_DIFF_ADD_COLOR),
        Some('-') => Style::new().fg(TOOL_DIFF_DEL_COLOR),
        _ => dim,
    };
    let with_bg = |style: Style| bg.map_or(style, |b| style.bg(b));

    let number = format!("{gutter} ");
    let gutter_cols = cols(&number) + usize::from(sign.is_some());
    let content_width = (width as usize)
        .saturating_sub(indent_cols + gutter_cols)
        .max(1);

    let segments: Vec<(String, Style)> = segs
        .iter()
        .map(|seg| (seg.text.clone(), seg.style))
        .collect();
    code_content_rows(&segments, content_width as u16)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![Span::raw(indent.clone())];
            if i == 0 {
                spans.push(Span::styled(number.clone(), with_bg(dim)));
                if let Some(s) = sign {
                    spans.push(Span::styled(s.to_string(), with_bg(sign_style)));
                }
            } else {
                spans.push(Span::styled(" ".repeat(gutter_cols), with_bg(Style::new())));
            }
            let mut row_cols = 0usize;
            for span in row {
                row_cols += cols(&span.content);
                let mut style = with_bg(span.style);
                if dim_content {
                    style = style.add_modifier(Modifier::DIM);
                }
                spans.push(Span::styled(span.content.into_owned(), style));
            }
            let pad = content_width.saturating_sub(row_cols);
            if bg.is_some() && pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), with_bg(Style::new())));
            }
            Line::from(spans)
        })
        .collect()
}

/// Render a raw numbered body — the `{n:>W} {text}` /
/// `{n:>W} {sign}{text}` gutter format
/// ([`crate::llm::tools::render_numbered_content`] /
/// [`crate::llm::tools::render_numbered_diff`]) — as styled rows at
/// `indent_cols`, capped at `budget` **display** rows (whole source rows only).
/// Returns the rows plus how many source rows were left out, so the caller can
/// append whichever "more" tail it uses.
///
/// The shared core of the finished `write`/`edit` cell ([`file_cell_lines`],
/// which caps at a peek and hints Ctrl+O) and the tool-permission prompt's
/// preview (`ui::permission_view`, which shows the whole change and caps only
/// to fit the terminal — `docs/permissions.md`). `signed` follows the body's
/// shape: a diff carries a `+`/`-`/space sign column, brand-new content does
/// not. A row that isn't in the format renders dim and verbatim, so an odd
/// line never swallows the rest.
pub(super) fn numbered_body_lines(
    body: &str,
    lang: Option<&str>,
    signed: bool,
    indent_cols: usize,
    width: u16,
    budget: usize,
) -> (Vec<Line<'static>>, usize) {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(indent_cols);
    let note_width = (width as usize).saturating_sub(indent_cols).max(1);
    let source: Vec<&str> = body.lines().collect();
    let mut hl = highlight::Highlighter::new(lang);
    let mut out = Vec::new();
    let mut used = 0usize;
    for (i, raw) in source.iter().enumerate() {
        let display = match parse_file_row(raw, signed) {
            Some(FileRow::Numbered { gutter, sign, text }) => {
                let segs = hl.line(&text);
                numbered_row_lines(&gutter, sign, &segs, indent_cols, width)
            }
            Some(FileRow::Gap(raw) | FileRow::Note(raw)) => {
                // Hunks re-synchronize at the gap; the lexer state resets too.
                hl = highlight::Highlighter::new(lang);
                vec![Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(truncate_cols(&raw, note_width), dim),
                ])]
            }
            None => vec![Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(truncate_cols(raw, note_width), dim),
            ])],
        };
        if used + display.len() > budget && used > 0 {
            return (out, source.len() - i);
        }
        used += display.len();
        out.extend(display);
    }
    (out, 0)
}

/// Build the styled `⎿` block for a `read`/`write`/`edit` cell whose output is
/// the numbered `llm::tools` format — codex's file look in the existing gutter:
/// the white summary head (its `(+A -D)` counts coloured) on the corner row,
/// then every body row via [`numbered_row_lines`], the `⋮` hunk gaps and `…`
/// notes dim. `peek` caps the body at [`FILE_PEEK_LINES`] display rows (whole
/// source rows only) and appends the `… +N lines (ctrl+o to expand)` hint.
/// `None` when the output isn't in the format — the caller falls back to the
/// legacy rendering. See `docs/tools.md`.
pub(super) fn file_cell_lines(
    tool: &ToolCall,
    width: u16,
    peek: bool,
) -> Option<Vec<Line<'static>>> {
    let (head, rows) = parse_file_cell(tool)?;
    let lang = file_cell_lang(&tool.args);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(file_body_indent());
    let note_width = (width as usize).saturating_sub(indent.len()).max(1);

    let mut summary = vec![Span::styled(TOOL_RESULT_PREFIX.to_string(), dim)];
    summary.extend(file_summary_spans(&head));
    let mut out = vec![Line::from(summary)];

    let budget = if peek { FILE_PEEK_LINES } else { usize::MAX };
    let mut used = 0usize;
    let mut hidden = 0usize;
    let mut hl = highlight::Highlighter::new(lang);
    for (i, row) in rows.iter().enumerate() {
        let display = match row {
            FileRow::Gap(raw) => {
                // Hunks re-synchronize at the gap; the lexer state resets too.
                hl = highlight::Highlighter::new(lang);
                vec![Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(truncate_cols(raw, note_width), dim),
                ])]
            }
            FileRow::Note(raw) => vec![Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(truncate_cols(raw, note_width), dim),
            ])],
            FileRow::Numbered { gutter, sign, text } => {
                let segs = hl.line(text);
                numbered_row_lines(gutter, *sign, &segs, file_body_indent(), width)
            }
        };
        if used + display.len() > budget && used > 0 {
            hidden = rows[i..]
                .iter()
                .filter(|r| matches!(r, FileRow::Numbered { .. }))
                .count();
            break;
        }
        used += display.len();
        out.extend(display);
    }
    if hidden > 0 {
        out.push(more_hint_line(hidden));
    }
    Some(out)
}

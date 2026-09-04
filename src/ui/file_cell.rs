//! The `read`/`write`/`edit` file cell: codex's `diff_render` look in the `⎿`
//! gutter — dim line numbers, green/red signs, syntax-highlighted content, and
//! added/removed rows on background tints. See `docs/tools.md`.

use super::assistant::code_content_rows;
use super::inline::wrap_inline_hanging;
use super::inline_diff::{RefineRow, refine_rows};
use super::theme::*;
use super::tool::{more_hint_line, tool_output_lines};
use super::wrap::{cols, segments_cols, truncate_cols};
use super::*;

/// A `⎿` gutter row with an explicit content colour (`None` → dim): the
/// **first** row (index 0) opens with the [`TOOL_RESULT_PREFIX`] corner,
/// continuation rows indent by its display width so the text aligns under it
/// (Claude-Code's exec-cell output style). The shared basis for the dim
/// [`result_row`] and the diff-coloured rows.
pub(super) fn gutter_row(index: usize, text: String, color: Option<Color>) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    gutter_row_styled(index, text, color.map_or(dim, |c| Style::new().fg(c)))
}

/// [`gutter_row`] with the content's whole [`Style`], not just its colour — for
/// a body that also carries a modifier, like the thinking stream's italic
/// chain-of-thought (`docs/thinking-stream.md`). The corner/indent stays dim
/// either way; this is the one place that geometry lives.
pub(super) fn gutter_row_styled(index: usize, text: String, style: Style) -> Line<'static> {
    let prefix = if index == 0 {
        TOOL_RESULT_PREFIX.to_string()
    } else {
        " ".repeat(cols(TOOL_RESULT_PREFIX))
    };
    Line::from(vec![
        Span::styled(prefix, Style::new().fg(TOOL_DIM_COLOR)),
        Span::styled(text, style),
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

/// One parsed row of a numbered `Wrote …`/`Updated …` body — the
/// `llm::tools` gutter format (`{n:>W} {text}` / `{n:>W} {sign}{text}`) a
/// `write`/`edit` cell restyles ([`file_cell_lines`]).
enum FileRow {
    /// A numbered content line: the raw right-aligned number `gutter`, the
    /// diff `sign` (`None` in a `Wrote` body, which has no sign column),
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
/// (`Updated` bodies carry a `+`/`-`/space sign column, `Wrote`/`Created`
/// bodies don't). `None` means the row isn't in the format — the whole cell
/// then keeps the legacy first-char diff colouring (old sessions, error
/// bodies).
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
/// `write`/`edit` cell carries its head in the output (`Wrote …`/`Updated …`,
/// plus the legacy `Created …` old rollouts still hold) over a signed (diff)
/// or unsigned (new-content) body; a `read` cell has **no** head — its whole
/// output is unsigned numbered content, so the `Read N lines` summary is
/// synthesized here. `None` when the cell isn't a finished file tool or the
/// output isn't in the `llm::tools` gutter format (a placeholder like `(file
/// is empty)`, an old rollout, an error body) — the caller keeps the legacy
/// rendering.
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
            } else if head.starts_with("Wrote ") || head.starts_with("Created ") {
                // `Wrote {N} lines to {path}` is the live head
                // (`llm::tools::write_report`); `Created {path} ({N} lines)`
                // is the pre-rename spelling old rollouts still carry.
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

/// The summary head with its path shown by the session's [`PathDisplay`]
/// rule (`docs/tools.md` *Path display*): `Wrote {N} lines to {path}`,
/// `Updated {path} (+A -D)` and the legacy `Created {path} ({N} lines)` each
/// name the file the way the `● Write({path})` header above them does —
/// relative under the cwd, `~`-relative under home, absolute elsewhere. A
/// head in any other shape (the synthesized `Read N lines`, which carries no
/// path) passes through untouched. The record keeps the executor's verbatim
/// path; only the painted row changes, which is also what lets a rollout
/// recorded with the old `../` climb read by the same rule.
fn display_file_head(head: &str, paths: &PathDisplay) -> String {
    if let Some(rest) = head.strip_prefix("Wrote ")
        && let Some((count, path)) = rest.split_once(" to ")
        && is_line_count(count)
    {
        return format!("Wrote {count} to {}", paths.display(path));
    }
    if let Some(rest) = head.strip_prefix("Updated ")
        && let Some(open) = rest.rfind(" (+")
        && is_diff_counts(&rest[open + 1..])
    {
        let (path, counts) = rest.split_at(open);
        return format!("Updated {}{counts}", paths.display(path));
    }
    if let Some(rest) = head.strip_prefix("Created ")
        && let Some(open) = rest.rfind(" (")
        && let Some(count) = rest[open + 2..].strip_suffix(')')
        && is_line_count(count)
    {
        let (path, tail) = rest.split_at(open);
        return format!("Created {}{tail}", paths.display(path));
    }
    head.to_string()
}

/// Is `s` a head's `{N} line`/`{N} lines` clause?
fn is_line_count(s: &str) -> bool {
    s.split_once(' ').is_some_and(|(n, unit)| {
        !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && matches!(unit, "line" | "lines")
    })
}

/// Is `s` an `Updated` head's `(+A -D)` clause?
fn is_diff_counts(s: &str) -> bool {
    s.strip_prefix("(+")
        .and_then(|t| t.strip_suffix(')'))
        .and_then(|t| t.split_once(" -"))
        .is_some_and(|(a, d)| {
            !a.is_empty()
                && !d.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && d.chars().all(|c| c.is_ascii_digit())
        })
}

/// The summary head of a file cell (`Wrote …`/`Updated …`/`Read N lines`) in
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

/// The `⎿` corner row(s) for a file cell's summary head — the
/// [`file_summary_spans`] dress, **word-wrapped** to the width
/// ([`wrap_inline_hanging`], the tool header's wrapper — a path is one word,
/// so an over-long one hard-breaks) with continuation rows indented under the
/// corner, instead of the clipped single row a long
/// `Wrote {N} lines to {path}` head used to lose its tail to. The `(+A -D)`
/// count colouring survives the wrap (the spans ride through as styled
/// segments).
fn summary_head_lines(head: &str, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let head_room = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let head_width = u16::try_from(head_room).unwrap_or(u16::MAX);
    let segments: Vec<(String, Style)> = file_summary_spans(head)
        .into_iter()
        .map(|span| (span.content.into_owned(), span.style))
        .collect();
    wrap_inline_hanging(&segments, head_width, head_width)
        .into_iter()
        .enumerate()
        .map(|(i, mut spans)| {
            let prefix = if i == 0 {
                TOOL_RESULT_PREFIX.to_string()
            } else {
                " ".repeat(cols(TOOL_RESULT_PREFIX))
            };
            let mut row = vec![Span::styled(prefix, dim)];
            row.append(&mut spans);
            Line::from(row)
        })
        .collect()
}

/// The left indent (display columns) of a numbered file cell's body — the
/// line-number gutter sits **one column past** the `⎿` corner content
/// ([`TOOL_RESULT_PREFIX`]), matching Claude-Code's file-change look (the
/// numbers land just inside the corner). The `⋮` hunk gaps and `…` notes align
/// here too. See `docs/tools.md`.
fn file_body_indent() -> usize {
    cols(TOOL_RESULT_PREFIX) + 1
}

/// One numbered row's already-styled content ([`row_segments`]), clipped to
/// `max_rows` display rows of `width` columns when the caller sets a budget —
/// the numbered file cell's half of the per-line row budget
/// (`docs/long-lines.md`). [`code_content_rows`]
/// hard-breaks at exactly `width`, so `max_rows * width` columns **is**
/// `max_rows` rows; the cut is marked with a dim [`TOOL_LINE_ELLIPSIS`] in the
/// last column, which keeps the row's `bg` tint. A row that fits comes back
/// unclipped and unmarked.
fn clip_segments(
    segments: Vec<(String, Style)>,
    width: usize,
    max_rows: Option<usize>,
    bg: Option<Color>,
) -> Vec<(String, Style)> {
    let Some(max) = max_rows else {
        return segments;
    };
    let budget = width.saturating_mul(max);
    if segments_cols(&segments) <= budget {
        return segments;
    }
    let room = budget.saturating_sub(cols(TOOL_LINE_ELLIPSIS));
    let mut kept: Vec<(String, Style)> = Vec::new();
    let mut used = 0usize;
    for (text, style) in segments {
        let w = cols(&text);
        if used + w <= room {
            used += w;
            kept.push((text, style));
        } else {
            let cut = truncate_cols(&text, room - used);
            if !cut.is_empty() {
                kept.push((cut, style));
            }
            break;
        }
    }
    let mark = Style::new().fg(TOOL_DIM_COLOR);
    kept.push((
        TOOL_LINE_ELLIPSIS.to_string(),
        bg.map_or(mark, |b| mark.bg(b)),
    ));
    kept
}

/// The largest index `<= at` that is a char boundary of `s`. Defensive: the
/// changed ranges are token boundaries of the very line these segments came
/// from, so this is a no-op in practice — but slicing a `&str` off one would
/// panic, and a syntax highlighter is not something to bet a redraw on.
fn floor_boundary(s: &str, at: usize) -> usize {
    let mut i = at.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Style one numbered row's syntax segments for display, splitting them at the
/// `changed` byte ranges — the character-level refinement
/// (`docs/inline-diff.md`).
///
/// The row's own `bg` tint sits under everything; the changed runs are lifted
/// onto the brighter `mark_bg` and bolded, so the muted row tint answers *this
/// line changed* while the bright one answers *here* — and only ever over
/// characters that genuinely differ, never over text the edit left alone.
/// On a removed row the
/// unchanged text dims (codex's look) and the changed text deliberately does
/// **not**: dimming the one run the eye is meant to find would defeat marking
/// it at all.
///
/// With no `changed` ranges this is exactly the old uniform styling, which is
/// what keeps an unrefined pair — and every `read`/`write` body — rendering
/// byte-for-byte as it did before.
fn row_segments(
    segs: &[highlight::Seg],
    changed: &[Range<usize>],
    bg: Option<Color>,
    mark_bg: Option<Color>,
    dim_content: bool,
) -> Vec<(String, Style)> {
    let plain = |style: Style| {
        let style = bg.map_or(style, |b| style.bg(b));
        if dim_content {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    };
    if changed.is_empty() {
        return segs
            .iter()
            .map(|seg| (seg.text.clone(), plain(seg.style)))
            .collect();
    }
    let marked = |style: Style| {
        mark_bg
            .map_or(style, |b| style.bg(b))
            .add_modifier(Modifier::BOLD)
    };
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut at = 0usize;
    for seg in segs {
        let end = at + seg.text.len();
        let mut cur = at;
        while cur < end {
            // The run from `cur` is either inside a changed range (up to its
            // end) or outside one (up to the next range's start).
            let hit = changed.iter().find(|r| r.start <= cur && cur < r.end);
            let stop = hit.map_or_else(
                || {
                    changed
                        .iter()
                        .map(|r| r.start)
                        .filter(|&start| start > cur)
                        .min()
                        .unwrap_or(end)
                },
                |r| r.end,
            );
            let stop = stop.min(end);
            let (lo, hi) = (
                floor_boundary(&seg.text, cur - at),
                floor_boundary(&seg.text, stop - at),
            );
            if hi > lo {
                let style = if hit.is_some() {
                    marked(seg.style)
                } else {
                    plain(seg.style)
                };
                out.push((seg.text[lo..hi].to_string(), style));
            }
            cur = stop;
        }
        at = end;
    }
    out
}

/// Build the display rows for one numbered source row: a dim right-aligned
/// line number, the `+`/`-` sign in the diff colour, and the content
/// syntax-highlighted — added rows on the dark-green tint, removed rows
/// (their text dimmed) on the dark-red one, both padded to the full width.
/// Long content wraps ([`code_content_rows`]); continuations indent under the
/// content column and keep the tint. `changed` are the row's character-level
/// refinement ranges ([`row_segments`], `docs/inline-diff.md`) — empty for an
/// unrefined row, which renders exactly as it always did.
///
/// `max_rows` is the collapsed cell's per-line budget ([`clip_segments`],
/// `docs/long-lines.md`): a minified `.json` line is cut to that many rows and
/// marked, instead of painting dozens of rows inline. `None` — the Ctrl+O
/// expansion and the permission prompt's preview — renders it whole.
fn numbered_row_lines(
    gutter: &str,
    sign: Option<char>,
    segs: &[highlight::Seg],
    changed: &[Range<usize>],
    indent_cols: usize,
    width: u16,
    max_rows: Option<usize>,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(indent_cols);
    let (bg, mark_bg, dim_content) = match sign {
        Some('+') => (Some(TOOL_DIFF_ADD_BG), Some(TOOL_DIFF_ADD_MARK_BG), false),
        Some('-') => (Some(TOOL_DIFF_DEL_BG), Some(TOOL_DIFF_DEL_MARK_BG), true),
        _ => (None, None, false),
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

    // The content spans arrive fully styled — tint, dim and word highlight
    // baked in before the wrap, so a changed word split across two display
    // rows keeps its mark on both.
    let styled = row_segments(segs, changed, bg, mark_bg, dim_content);
    let segments = clip_segments(styled, content_width, max_rows, bg);
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
                spans.push(span);
            }
            // The pad takes the plain row tint, never the word one, so the
            // bright block ends where the changed text ends instead of
            // bleeding to the terminal's edge.
            let pad = content_width.saturating_sub(row_cols);
            if bg.is_some() && pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), with_bg(Style::new())));
            }
            Line::from(spans)
        })
        .collect()
}

/// One parsed row as the character-level refinement sees it: a numbered row is its
/// sign plus its content, everything else breaks the run (`docs/inline-diff.md`).
fn refine_row(row: &FileRow) -> RefineRow<'_> {
    match row {
        FileRow::Numbered { sign, text, .. } => RefineRow::Line(sign.unwrap_or(' '), text),
        FileRow::Gap(_) | FileRow::Note(_) => RefineRow::Break,
    }
}

/// [`refine_row`] over a body whose rows all parsed ([`parse_file_cell`]).
fn refine_input_rows(rows: &[FileRow]) -> Vec<RefineRow<'_>> {
    rows.iter().map(refine_row).collect()
}

/// [`refine_row`] over a body parsed row by row ([`numbered_body_lines`]),
/// where a line that didn't parse renders verbatim and breaks the run.
fn refine_input(rows: &[Option<FileRow>]) -> Vec<RefineRow<'_>> {
    rows.iter()
        .map(|row| row.as_ref().map_or(RefineRow::Break, refine_row))
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
    // Parse the whole body first: the refinement pairs a `-` run with the `+`
    // run following it, which no per-row pass can see.
    let parsed: Vec<Option<FileRow>> = source
        .iter()
        .map(|raw| parse_file_row(raw, signed))
        .collect();
    let changed = refine_rows(&refine_input(&parsed));
    let mut hl = highlight::Highlighter::new(lang);
    let mut out = Vec::new();
    let mut used = 0usize;
    for (i, raw) in source.iter().enumerate() {
        let display = match &parsed[i] {
            Some(FileRow::Numbered { gutter, sign, text }) => {
                let segs = hl.line(text);
                numbered_row_lines(gutter, *sign, &segs, &changed[i], indent_cols, width, None)
            }
            Some(FileRow::Gap(raw) | FileRow::Note(raw)) => {
                // Hunks re-synchronize at the gap; the lexer state resets too.
                hl = highlight::Highlighter::new(lang);
                vec![Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(truncate_cols(raw, note_width), dim),
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
/// word-wrapped under it when long ([`summary_head_lines`]),
/// then every body row via [`numbered_row_lines`], the `⋮` hunk gaps and `…`
/// notes dim. `peek` caps the body at [`FILE_PEEK_LINES`] display rows (whole
/// source rows only) and appends the `… +N lines (ctrl+o to expand)` hint.
/// `None` when the output isn't in the format — the caller falls back to the
/// legacy rendering. See `docs/tools.md`.
pub(super) fn file_cell_lines(
    tool: &ToolCall,
    width: u16,
    peek: bool,
    paths: &PathDisplay,
) -> Option<Vec<Line<'static>>> {
    let (head, rows) = parse_file_cell(tool)?;
    let head = display_file_head(&head, paths);
    let lang = file_cell_lang(&tool.args);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(file_body_indent());
    let note_width = (width as usize).saturating_sub(indent.len()).max(1);
    let mut out = summary_head_lines(&head, width);

    let budget = if peek { FILE_PEEK_LINES } else { usize::MAX };
    // The collapsed cell also bounds ONE source line (`docs/long-lines.md`);
    // the Ctrl+O expansion is where the whole line lives, so it passes `None`.
    let line_rows = peek.then_some(TOOL_LINE_MAX_ROWS);
    let changed = refine_rows(&refine_input_rows(&rows));
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
                numbered_row_lines(
                    gutter,
                    *sign,
                    &segs,
                    &changed[i],
                    file_body_indent(),
                    width,
                    line_rows,
                )
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

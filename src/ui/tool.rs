//! Tool cells: the `● name(args)` header, the `⎿` output block, the numbered
//! file-change rendering, and a running command's live tail.
//! See `docs/tools.md` and `docs/tool-streaming.md`.

use super::assistant::{code_content_rows, expand_code_tabs, wrap_inline};
use super::theme::*;
use super::wrap::{cols, truncate_cols, truncate_spans, wrap_output, wrap_verbatim};
use super::*;

/// The bullet colour for a tool's lifecycle: dim waiting, blue running, green
/// ok, red fail — and green for a call that resolved by moving to the
/// background (the launch succeeded; see `docs/background.md`).
const fn tool_status_color(status: ToolStatus) -> Color {
    match status {
        ToolStatus::Waiting => TOOL_WAITING_COLOR,
        ToolStatus::Running => TOOL_RUNNING_COLOR,
        ToolStatus::Ok | ToolStatus::Backgrounded => TOOL_OK_COLOR,
        ToolStatus::Failed => TOOL_FAIL_COLOR,
    }
}

/// The coloured bullet header row(s) for a backend tool call: `● {name}({args})`,
/// the bullet recoloured by lifecycle (blue/green/red) and the args made bold +
/// the normal reply white ([`TOOL_ARGS_COLOR`]) so a `bash` command reads
/// clearly, the framing `(`/`)` left a dim [`TOOL_DIM_COLOR`] delimiter. Shared
/// by the inline collapsed view ([`tool_lines`]) and the full-screen transcript
/// ([`tool_full_lines`]).
///
/// When the header overflows `width` the args **word-wrap** across continuation
/// rows, each indented to align **under the opening `(`** (the width of
/// `● {name}`), so a long command reads clean and is never clipped at the
/// terminal edge — Claude-Code's wrapped `Bash(…)` header. `max_rows` caps how
/// many rows are shown: `Some(n)` (the inline peek and the live preview) keeps
/// the first `n` and splices [`TOOL_HEADER_ELLIPSIS`] + `)` onto the last so a
/// huge command doesn't flood the cell; `None` (the Ctrl+O transcript) renders
/// the whole thing. A `!` shell command is a tool with no args (name = the
/// command), so it stays a bare single `● {command}` line.
fn tool_header_lines(tool: &ToolCall, width: u16, max_rows: Option<usize>) -> Vec<Line<'static>> {
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status))
        .add_modifier(Modifier::BOLD);
    let name_style = Style::new()
        .fg(TOOL_NAME_COLOR)
        .add_modifier(Modifier::BOLD);
    let bullet = || Span::styled(TOOL_BULLET.to_string(), bullet_style);
    let name = || Span::styled(tool.name.clone(), name_style);
    if tool.args.is_empty() {
        return vec![Line::from(vec![bullet(), name()])];
    }
    let args_style = Style::new()
        .fg(TOOL_ARGS_COLOR)
        .add_modifier(Modifier::BOLD);
    // Continuation rows indent to align under the opening `(`, which sits right
    // after `● {name}`; wrapping `(args)` as one run — parens and args alike bold
    // white (a uniform, noticeable header body) — keeps every row (the first
    // included) the same body width, so the wrapped rows land exactly beneath the
    // `(`.
    let indent_cols = cols(TOOL_BULLET) + cols(&tool.name);
    let body_width = (width as usize).saturating_sub(indent_cols).max(1);
    let mut rows = wrap_inline(
        &[(format!("({})", tool.args), args_style)],
        body_width as u16,
    );
    // Cap a very long header: keep the first `max` rows and replace the tail with
    // `…)` (fitted within the body width, the same bold white) — the whole command
    // is still in Ctrl+O.
    if let Some(max) = max_rows
        && rows.len() > max.max(1)
    {
        rows.truncate(max.max(1));
        if let Some(last) = rows.last_mut() {
            let keep = body_width.saturating_sub(cols(TOOL_HEADER_ELLIPSIS) + cols(")"));
            *last = truncate_spans(last, keep);
            last.push(Span::styled(format!("{TOOL_HEADER_ELLIPSIS})"), args_style));
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, mut body_spans)| {
            let mut spans = if i == 0 {
                vec![bullet(), name()]
            } else {
                vec![Span::raw(" ".repeat(indent_cols))]
            };
            spans.append(&mut body_spans);
            Line::from(spans)
        })
        .collect()
}

/// A `⎿` gutter row with an explicit content colour (`None` → dim): the
/// **first** row (index 0) opens with the [`TOOL_RESULT_PREFIX`] corner,
/// continuation rows indent by its display width so the text aligns under it
/// (Claude-Code's exec-cell output style). The shared basis for the dim
/// [`result_row`] and the diff-coloured rows.
fn gutter_row(index: usize, text: String, color: Option<Color>) -> Line<'static> {
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

/// A dim `⎿` result row — for the `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `+N lines (Ns)` footer (meta, not output).
pub(super) fn result_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, None)
}

/// A `⎿` row for a finished tool's **output** — a dim corner over white content
/// ([`TOOL_OUTPUT_COLOR`]), so command/shell output reads like a normal reply.
/// The placeholders keep the dim [`result_row`].
fn output_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, Some(TOOL_OUTPUT_COLOR))
}

/// Is this a model tool whose output is a diff — so its `⎿` rows get `+`/`-`
/// diff colouring ([`diff_result_row`])? See [`DIFF_TOOL_NAMES`].
fn is_diff_tool(tool: &ToolCall) -> bool {
    !tool.shell && DIFF_TOOL_NAMES.contains(&tool.name.as_str())
}

/// Is this a model **command tool** (`bash`) — rendered like the `!` shell cell
/// (a multi-line `⎿` peek, its `Exit code: N` frame stripped for display) and
/// **tailed** live while running? Other generic backend tools keep the single
/// collapsed peek line. See [`COMMAND_TOOL_NAMES`] and `docs/tool-streaming.md`.
pub(super) fn is_command_tool(tool: &ToolCall) -> bool {
    !tool.shell && COMMAND_TOOL_NAMES.contains(&tool.name.as_str())
}

/// The diff colour for a source line by its leading marker — `+` green, `-`
/// red, everything else (context, the summary header) dim (`None`).
fn diff_line_color(line: &str) -> Option<Color> {
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
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent_cols = file_body_indent();
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

/// Build the styled `⎿` block for a `read`/`write`/`edit` cell whose output is
/// the numbered `llm::tools` format — codex's file look in the existing gutter:
/// the white summary head (its `(+A -D)` counts coloured) on the corner row,
/// then every body row via [`numbered_row_lines`], the `⋮` hunk gaps and `…`
/// notes dim. `peek` caps the body at [`FILE_PEEK_LINES`] display rows (whole
/// source rows only) and appends the `… +N lines (ctrl+o to expand)` hint.
/// `None` when the output isn't in the format — the caller falls back to the
/// legacy rendering. See `docs/tools.md`.
fn file_cell_lines(tool: &ToolCall, width: u16, peek: bool) -> Option<Vec<Line<'static>>> {
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
                numbered_row_lines(gutter, *sign, &segs, width)
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

/// The dim `… +N lines (ctrl+o to expand)` hint under a capped peek.
fn more_hint_line(hidden: usize) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Line::from(vec![
        Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
        Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
    ])
}

/// The live preview row for a running `!` shell command: `⎿ Running… (Ns)`. A
/// shell turn hides the spinner status line entirely (see [`strip_has_status`]),
/// so its running elapsed lives here instead — the boundary-supplied
/// `elapsed` (whole seconds), like the status line's timer. Only shown live in
/// [`render_live`]; the committed cell renders its output, not `Running…`. See
/// `docs/shell-command.md`.
pub(super) fn shell_running_line(elapsed: Duration) -> Line<'static> {
    result_row(0, format!("{TOOL_RUNNING} ({}s)", elapsed.as_secs()))
}

/// The output of `tool` split into display lines (a single trailing blank from a
/// final newline dropped, so a hidden-line count is accurate). Tabs are
/// expanded for display ([`expand_code_tabs`]) — a `'\t'` grapheme paints as
/// zero cells (ratatui filters control chars), gluing tab-separated fields
/// together — while the stored output stays byte-exact, like the code-block
/// render path.
fn tool_output_lines(tool: &ToolCall) -> Vec<String> {
    split_display_lines(&tool.output)
}

/// Split display `text` into lines: tabs expanded ([`expand_code_tabs`] — a
/// `'\t'` paints as zero cells otherwise), with a single trailing blank from a
/// final newline dropped so a hidden-line count stays accurate.
fn split_display_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = text
        .split('\n')
        .map(|line| expand_code_tabs(line).into_owned())
        .collect();
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// `output` reframed for **display**: on **success** the `Exit code: 0` line
/// is dropped so the cell reads like the real command output (the mock,
/// `docs/tool-streaming.md`); on **failure** it is rewritten to an
/// `Error: Exit code N` (or `Error: killed by signal`) line kept above the
/// body, so a red cell says *why* it failed even when the body is empty. The
/// raw frame stays in `tool.output` for the model / context replay
/// (`context::context_messages`) — this is display-only. Only fires when the
/// frame is present, so non-`bash` tools and old rollouts are untouched.
fn command_display_output(output: &str) -> std::borrow::Cow<'_, str> {
    let Some(rest) = output.strip_prefix("Exit code: ") else {
        return std::borrow::Cow::Borrowed(output);
    };
    // The frame is `Exit code: {code}\n{body}` (the body may be absent).
    let (code, body) = match rest.find('\n') {
        Some(nl) => (&rest[..nl], &rest[nl + 1..]),
        None => (rest, ""),
    };
    if code == "0" {
        // Success: the frame is noise — show just the body.
        return std::borrow::Cow::Borrowed(body);
    }
    // Failure: surface the exit code as an `Error: …` header above the body.
    let head = if code == "killed by signal" {
        "Error: killed by signal".to_string()
    } else {
        format!("Error: Exit code {code}")
    };
    std::borrow::Cow::Owned(if body.is_empty() {
        head
    } else {
        format!("{head}\n{body}")
    })
}

/// A command-style tool's output as display lines — [`tool_output_lines`] with
/// the `Exit code: N` frame reframed for display ([`command_display_output`]).
pub(super) fn command_display_lines(tool: &ToolCall) -> Vec<String> {
    split_display_lines(&command_display_output(&tool.output))
}

/// The live preview for a **running** command-style backend tool (`bash`): the
/// coloured `● name(args)` header, the **last** [`TOOL_PEEK_LINES`] display
/// **rows** of its output under the `⎿` gutter (the *tail* — what just
/// streamed), then a `+{hidden} lines ({secs}s)` footer when any source lines
/// are fully hidden above it. This is Claude-Code's running-command look (the
/// mock; `docs/tool-streaming.md`) — the asymmetric twin of the finished head
/// peek in [`tool_lines`]. The `elapsed` is boundary-supplied (like the shell
/// running row and the status timer), so this is drawn from
/// [`preview_tool_lines`] where `App` is in hand.
///
/// Long lines **word-wrap, spaces preserved** ([`wrap_output`] — the same
/// wrapper the finished peek and the Ctrl+O view use, so alignment survives
/// and prose breaks at words) instead of clipping at the width; the window is
/// counted in wrapped rows so a single long line tail-follows its own newest
/// rows without growing the strip past its budget. Walking the source lines
/// newest-first wraps only what the window can show — never the whole
/// retained buffer — per animation frame.
pub(super) fn running_command_lines(
    tool: &ToolCall,
    elapsed: Duration,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    let display = command_display_lines(tool);
    // The tail window: the last TOOL_PEEK_LINES wrapped rows, each remembering
    // its source line index so the footer can count what's *fully* hidden.
    let mut window: VecDeque<(usize, String)> = VecDeque::new();
    for (idx, line) in display.iter().enumerate().rev() {
        for row in wrap_output(line, wrap_width).into_iter().rev() {
            window.push_front((idx, row));
        }
        if window.len() >= TOOL_PEEK_LINES {
            break;
        }
    }
    while window.len() > TOOL_PEEK_LINES {
        window.pop_front();
    }
    // Source lines wholly above the window. A partially shown wrapped line is
    // on screen, not hidden — its index is the count of the lines above it.
    let hidden = window.front().map_or(0, |(idx, _)| *idx);
    let shown = window.len();
    for (i, (_, row)) in window.into_iter().enumerate() {
        lines.push(output_row(i, row));
    }
    if hidden > 0 {
        // A continuation row (index ≥ 1) so it indents under the content column;
        // the `+N lines (Ns)` footer is meta, so it stays the dim `result_row`.
        lines.push(result_row(
            shown,
            format!("+{hidden} lines ({}s)", elapsed.as_secs()),
        ));
    }
    lines
}

/// Build the styled lines for one tool call as shown **inline**.
///
/// A `!` shell command is **headerless** — its `Role::Shell` header (`! pwd`)
/// sits flush above (docs/shell-command.md) — and shows up to
/// `TOOL_PEEK_LINES` of its output as a `⎿` block (each line aligned under
/// the corner), then a `… +N lines (ctrl+o to expand)` hint when more is
/// hidden (Claude-Code's exec cell). A backend tool keeps its coloured
/// `● name(args)` header and a single collapsed peek line. The full output is
/// only rendered in the separate tool-output view, never here.
#[must_use]
pub fn tool_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let out_lines = tool_output_lines(tool);

    // A call resolved by moving to the background shows the fixed
    // `⎿ Running in the background (↓ to manage)` row — its stored output is
    // the model-facing launch text, never displayed (docs/background.md). A
    // `!` shell cell stays headerless like its other states.
    if tool.status == ToolStatus::Backgrounded {
        let row = result_row(0, TOOL_BACKGROUNDED.to_string());
        if tool.shell {
            return vec![row];
        }
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.push(row);
        return lines;
    }

    if tool.shell {
        // The running/empty single-row states; else up to TOOL_PEEK_LINES rows.
        // (Truncation of an over-cap output is marked only in the expanded view;
        // inline, the `… +N lines (ctrl+o to expand)` hint already signals more.)
        return match tool.status {
            // A shell command is never batched, so it is never `Waiting`; the
            // arm is here only to keep the match total and correct if it ever is.
            ToolStatus::Waiting => vec![result_row(0, TOOL_WAITING.to_string())],
            ToolStatus::Running => vec![result_row(0, TOOL_RUNNING.to_string())],
            _ if out_lines.is_empty() => vec![result_row(0, TOOL_NO_OUTPUT.to_string())],
            _ => result_peek_block(&out_lines, peek_width, wrap_output, |i, text, _| {
                output_row(i, text)
            }),
        };
    }

    // A `write`/`edit` cell in the numbered `llm::tools` format renders
    // codex-style — numbers, hunk gaps, tints, syntax colour
    // ([`file_cell_lines`]); output that doesn't parse (old sessions, error
    // bodies) falls through to the legacy first-char colouring below.
    if let Some(body) = file_cell_lines(tool, width, true) {
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.extend(body);
        return lines;
    }

    // An `edit`/`write` diff tool: coloured header + a multi-line `⎿` peek whose
    // `+`/`-` rows are diff-coloured (the codex trick shows inline, not just in
    // the Ctrl+O view). Other backend tools keep the single collapsed peek line.
    if is_diff_tool(tool) && tool.status != ToolStatus::Running && !out_lines.is_empty() {
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        // Wrap verbatim (a diff body is code, never reflowed at spaces) and
        // colour every wrapped row by the SOURCE line's `+`/`-` marker, so a
        // continuation row keeps its tint — the Ctrl+O view colours the same
        // way (docs/tools.md).
        lines.extend(result_peek_block(
            &out_lines,
            peek_width,
            wrap_verbatim,
            |i, text, src| gutter_row(i, text, diff_line_color(src)),
        ));
        return lines;
    }

    // A backend **command tool** (`bash`): coloured header (wrapped when long)
    // over a multi-line `⎿` peek — the *head*, up to TOOL_PEEK_LINES lines, then
    // `… +N lines (ctrl+o to expand)`, like the `!` shell cell (the mock's
    // finished state). The `Exit code: N` frame is stripped for display
    // (docs/tool-streaming.md); no output yet → the `⎿ Running…`/`Waiting…` row.
    // The running *tail* (last lines + elapsed) is a separate live-only render
    // (`running_command_lines`), used by the preview.
    if is_command_tool(tool) {
        let display = command_display_lines(tool);
        let peek = match tool.status {
            ToolStatus::Waiting => vec![result_row(0, TOOL_WAITING.to_string())],
            _ if display.is_empty() => vec![result_row(
                0,
                if tool.status == ToolStatus::Running {
                    TOOL_RUNNING.to_string()
                } else {
                    TOOL_NO_OUTPUT.to_string()
                },
            )],
            _ => result_peek_block(&display, peek_width, wrap_output, |i, text, _| {
                output_row(i, text)
            }),
        };
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.extend(peek);
        return lines;
    }

    // Any other backend tool (a `read`/`write`/`edit` cell whose output didn't
    // parse as the numbered/diff format, or an unknown tool): coloured header
    // (wrapped when long) + a single collapsed peek line — white output content,
    // dim placeholder — the rest behind the `… +N lines` hint.
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
    lines.push(match tool.status {
        ToolStatus::Waiting => result_row(0, TOOL_WAITING.to_string()),
        ToolStatus::Running => result_row(0, TOOL_RUNNING.to_string()),
        _ if out_lines.is_empty() => result_row(0, TOOL_NO_OUTPUT.to_string()),
        _ => output_row(0, truncate_cols(&out_lines[0], peek_width)),
    });
    let hidden = out_lines.len().saturating_sub(1);
    if hidden > 0 {
        lines.push(more_hint_line(hidden));
    }
    lines
}

/// The head peek of `out_lines`: the first [`TOOL_PEEK_LINES`] **source
/// lines**, each **fully wrapped** to `peek_width` — "the first 4 lines of
/// output", so a long first line never pushes its siblings out of the peek —
/// built with `row` (which also receives the **source** line, so a diff cell
/// can colour a wrapped continuation by the source's `+`/`-` marker). The
/// wrapper is `wrap`: [`wrap_output`] for command/shell output (word
/// boundaries, spaces preserved, like the Ctrl+O view) or [`wrap_verbatim`]
/// for diff bodies (code — hard-break, never reflowed at spaces) — either
/// way a long line's tail no longer disappears past the terminal edge. A
/// `… +N lines` hint follows when any source line isn't fully shown.
///
/// [`TOOL_PEEK_MAX_ROWS`] is the safety ceiling in display rows: one
/// pathological line (a minified bundle) can't balloon a committed cell into
/// hundreds of rows. `hidden` counts **source lines** not fully shown (a
/// line the ceiling cut mid-wrap counts as hidden), so the hint appears
/// whenever any content is cut — even within a single line. The wrap stops
/// once a budget is spent, so this is O(peek), not O(output).
fn result_peek_block(
    out_lines: &[String],
    peek_width: usize,
    wrap: fn(&str, u16) -> Vec<String>,
    row: impl Fn(usize, String, &str) -> Line<'static>,
) -> Vec<Line<'static>> {
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    let mut lines: Vec<Line> = Vec::new();
    let mut fully_shown = 0usize; // source lines whose every wrapped row fits
    for line in out_lines.iter().take(TOOL_PEEK_LINES) {
        if lines.len() >= TOOL_PEEK_MAX_ROWS {
            break;
        }
        let wrapped = wrap(line, wrap_width);
        let total = wrapped.len();
        let room = TOOL_PEEK_MAX_ROWS - lines.len();
        let take = total.min(room);
        for text in wrapped.into_iter().take(take) {
            // The very first display row of the block gets the `⎿` corner
            // ([`gutter_row`]'s index 0); every later row — a wrapped
            // continuation or the next source line — indents under the content
            // column, exactly like the uncapped Ctrl+O block.
            let i = lines.len();
            lines.push(row(i, text, line));
        }
        if take < total {
            break; // the ceiling cut this line mid-wrap: only partially shown
        }
        fully_shown += 1;
    }
    let hidden = out_lines.len() - fully_shown;
    if hidden > 0 {
        lines.push(more_hint_line(hidden));
    }
    lines
}

/// One tool call's full lines for the transcript view: its **complete** output
/// (wrapped **verbatim** — [`wrap_verbatim`], so `ls -l`/`tree` alignment and
/// indentation survive), or `running…` / `(no output)` when there is none yet.
/// The expanded counterpart of [`tool_lines`]. The output renders as a `⎿`
/// gutter block — each row aligned under the corner ([`result_row`]), the same
/// gutter as the inline peek and a shell cell — uncapped. A `!` shell command
/// stays **headerless** (the `! pwd` dark header is the `Role::Shell` message
/// above it); a backend tool keeps its coloured `● name(args)` header over the
/// gutter. An over-cap shell output ([`ToolCall::truncated`]) appends a dim
/// [`TOOL_TRUNCATED_MARKER`] line to show the rest was dropped.
pub(super) fn tool_full_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    // A backgrounded call shows its fixed row in the transcript too — the
    // live output belongs to the ↓ manager, and the final output arrives as
    // the completion notice (docs/background.md).
    if tool.status == ToolStatus::Backgrounded {
        let row = result_row(0, TOOL_BACKGROUNDED.to_string());
        if tool.shell {
            return vec![row];
        }
        let mut lines = tool_header_lines(tool, width, None);
        lines.push(row);
        return lines;
    }
    // A numbered `write`/`edit` cell renders wholesale (numbers, tints,
    // syntax colour — [`file_cell_lines`], uncapped here); everything else
    // goes through the plain row pipeline below.
    if let Some(body) = file_cell_lines(tool, width, false) {
        // The Ctrl+O transcript view never truncates the header (`None`).
        let mut lines = tool_header_lines(tool, width, None);
        lines.extend(body);
        if tool.truncated {
            lines.push(gutter_row(1, TOOL_TRUNCATED_MARKER.to_string(), None));
        }
        return lines;
    }
    // The body hangs under the `⎿` gutter, so it wraps to the width left of it
    // (like [`result_row`]'s continuation indent) — for a backend tool too, so
    // its expanded output aligns under the corner just like its inline peek.
    let body_width = width.saturating_sub(cols(TOOL_RESULT_PREFIX) as u16).max(1);
    // Each body row carries its content colour (`None` → dim). For an
    // `edit`/`write` diff cell the colour comes from the **source** line, then
    // the line is wrapped — so a long `+`/`-` line's continuation rows keep the
    // added/removed colour instead of being mis-coloured by their own
    // (marker-less) first char. Every other tool's output is dim.
    let mut rows: Vec<(String, Option<Color>)> = match (tool.status, tool.output.is_empty()) {
        (ToolStatus::Waiting, _) => vec![(TOOL_WAITING.to_string(), None)],
        (ToolStatus::Running, true) => vec![(TOOL_RUNNING.to_string(), None)],
        (_, true) => vec![(TOOL_NO_OUTPUT.to_string(), None)],
        _ if is_diff_tool(tool) => tool
            .output
            .split('\n')
            .flat_map(|src| {
                let color = diff_line_color(src);
                wrap_verbatim(&expand_code_tabs(src), body_width)
                    .into_iter()
                    .map(move |piece| (piece, color))
            })
            .collect(),
        // Tabs expanded for display (they paint as zero cells otherwise —
        // see `tool_output_lines`); the stored output stays byte-exact. A
        // backend command tool's `Exit code: N` frame is stripped for display
        // (a `!` shell command's output is raw — never framed, never stripped;
        // docs/tool-streaming.md).
        _ => {
            let body: std::borrow::Cow<'_, str> = if tool.shell {
                std::borrow::Cow::Borrowed(tool.output.as_str())
            } else {
                command_display_output(&tool.output)
            };
            wrap_output(&expand_code_tabs(&body), body_width)
                .into_iter()
                .map(|line| (line, Some(TOOL_OUTPUT_COLOR)))
                .collect()
        }
    };
    // The output was cut at the in-memory cap — mark the end so the user knows
    // more was dropped (it is not recoverable; nothing to expand to). Only the
    // `!` shell runner caps, so a backend tool never sets this.
    if tool.truncated {
        rows.push((TOOL_TRUNCATED_MARKER.to_string(), None));
    }
    let result = rows
        .into_iter()
        .enumerate()
        .map(|(i, (text, color))| gutter_row(i, text, color));
    // A `!` shell command is headerless (its `Role::Shell` header sits above);
    // a backend tool keeps its coloured `● name(args)` header (wrapped when long)
    // over the gutter.
    if tool.shell {
        result.collect()
    } else {
        // The Ctrl+O transcript view shows the whole command (`None`).
        let mut lines = tool_header_lines(tool, width, None);
        lines.extend(result);
        lines
    }
}

//! Tool cells: the `● name(args)` header, the `⎿` output block, the numbered
//! file-change rendering, and a running command's live tail.
//! See `docs/tools.md` and `docs/tool-streaming.md`.

use super::assistant::expand_code_tabs;
use super::file_cell::{diff_line_color, file_cell_lines, gutter_row, is_diff_tool};
use super::inline::wrap_inline;
use super::theme::*;
use super::wrap::{blend, cols, truncate_cols, truncate_spans, wrap_output, wrap_verbatim};
use super::*;

/// The bullet colour for a tool's lifecycle: dim waiting, grey running, green
/// ok, red fail — and green for a call that resolved by moving to the
/// background (the launch succeeded; see `docs/background.md`).
///
/// `pulse` is the live region's frame clock (`App::pulse`): `Some` animates a
/// **running** bullet through [`tool_pulse_color`], `None` renders it at rest.
/// Only `Running` ever animates — a queued sibling and a resolved call mean the
/// same thing on every frame, so making them move would say nothing. See
/// `docs/tool-pulse.md`.
fn tool_status_color(status: ToolStatus, pulse: Option<Duration>) -> Color {
    match status {
        ToolStatus::Waiting => TOOL_WAITING_COLOR,
        ToolStatus::Running => pulse.map_or(TOOL_RUNNING_COLOR, tool_pulse_color),
        ToolStatus::Ok | ToolStatus::Backgrounded => TOOL_OK_COLOR,
        ToolStatus::Failed => TOOL_FAIL_COLOR,
    }
}

/// A running bullet's colour for the frame at `elapsed` — the breath
/// Claude-Code's running dot has: a raised cosine easing
/// [`TOOL_PULSE_DIM`] → [`TOOL_PULSE_BRIGHT`] → [`TOOL_PULSE_DIM`] once per
/// [`TOOL_PULSE_PERIOD`], so it swells and fades rather than flicking on and
/// off.
///
/// Pure, like [`shimmer_spans`](super::status) and the comet spinner: the phase
/// derives entirely from the boundary-supplied `elapsed`
/// ([`App::set_pulse`](crate::app::App::set_pulse)), and the loop's 32 ms
/// animation re-arm is what makes it move. See `docs/tool-pulse.md`.
pub(super) fn tool_pulse_color(elapsed: Duration) -> Color {
    let period = TOOL_PULSE_PERIOD.as_secs_f32();
    // `phase` is 0…1 through one breath; the cosine turns it into 0 → 1 → 0.
    let phase = (elapsed.as_secs_f32() % period) / period;
    let t = 0.5 * (1.0 - (std::f32::consts::TAU * phase).cos());
    let (r, g, b) = blend(TOOL_PULSE_BRIGHT, TOOL_PULSE_DIM, t);
    Color::Rgb(r, g, b)
}

/// The coloured bullet header row(s) for a backend tool call: `● {name}({args})`,
/// the bullet recoloured by lifecycle (grey running/green/red) and the args made bold +
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
pub(super) fn tool_header_lines(
    tool: &ToolCall,
    width: u16,
    max_rows: Option<usize>,
    pulse: Option<Duration>,
) -> Vec<Line<'static>> {
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status, pulse))
        .add_modifier(Modifier::BOLD);
    let name_style = Style::new()
        .fg(TOOL_NAME_COLOR)
        .add_modifier(Modifier::BOLD);
    let bullet = || Span::styled(TOOL_BULLET.to_string(), bullet_style);
    let name = || Span::styled(tool.name.clone(), name_style);
    // An MCP call stores its **raw arguments JSON** (what makes the record
    // replayable — `docs/mcp.md`); the header derives the pretty
    // `key: "value"` form at render time instead.
    let args = if crate::mcp::is_mcp_display_name(&tool.name) {
        crate::mcp::pretty_args(&tool.args)
    } else {
        tool.args.clone()
    };
    if args.is_empty() {
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
    let mut rows = wrap_inline(&[(format!("({args})"), args_style)], body_width as u16);
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

/// Is this a model **command tool** (`bash`) — rendered like the `!` shell cell
/// (a multi-line `⎿` peek, its `Exit code: N` frame stripped for display) and
/// **tailed** live while running? Other generic backend tools keep the single
/// collapsed peek line. See [`COMMAND_TOOL_NAMES`] and `docs/tool-streaming.md`.
pub(super) fn is_command_tool(tool: &ToolCall) -> bool {
    !tool.shell && COMMAND_TOOL_NAMES.contains(&tool.name.as_str())
}

/// The dim `… +N lines (ctrl+o to expand)` hint under a capped peek.
pub(super) fn more_hint_line(hidden: usize) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Line::from(vec![
        Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
        Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
    ])
}

/// The live preview row for a running `!` shell command: `⎿ Running…
/// ({elapsed})`. A shell turn hides the spinner status line entirely (see
/// [`strip_has_status`]), so its running elapsed lives here instead — the
/// boundary-supplied `elapsed`, [`format_elapsed`]-humanized like the status
/// line's timer (`1m 5s`, never a bare `65s`). Only shown live in
/// [`render_live`]; the committed cell renders its output, not `Running…`.
/// See `docs/shell-command.md`.
pub(super) fn shell_running_line(elapsed: Duration) -> Line<'static> {
    result_row(
        0,
        format!("{TOOL_RUNNING} ({})", format_elapsed(elapsed.as_secs())),
    )
}

/// The output of `tool` split into display lines (a single trailing blank from a
/// final newline dropped, so a hidden-line count is accurate). Tabs are
/// expanded for display ([`expand_code_tabs`]) — a `'\t'` grapheme paints as
/// zero cells (ratatui filters control chars), gluing tab-separated fields
/// together — while the stored output stays byte-exact, like the code-block
/// render path.
pub(super) fn tool_output_lines(tool: &ToolCall) -> Vec<String> {
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
///
/// Two clocks ride in: `elapsed` is how long the command has run (the `(Ns)`
/// footer), `pulse` is the frame phase its bullet breathes at. Live-only by
/// construction — only the strip calls this — so the pulse is unconditional
/// here (`docs/tool-pulse.md`).
pub(super) fn running_command_lines(
    tool: &ToolCall,
    elapsed: Duration,
    pulse: Duration,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), Some(pulse));
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
        // the `+N lines ({elapsed})` footer is meta, so it stays the dim
        // `result_row` — the elapsed humanized past a minute like every
        // runtime display.
        lines.push(result_row(
            shown,
            format!("+{hidden} lines ({})", format_elapsed(elapsed.as_secs())),
        ));
    }
    lines
}

/// The auto mode classifier's provenance row (`docs/permissions.md`): a
/// fresh dim `⎿ Allowed by auto mode classifier` corner appended under a
/// **resolved** cell that carries the note — the reference transcript's last
/// line. A waiting/running cell keeps its live look (the note is recorded
/// the moment the call starts, but only a finished command shows it).
fn approval_note_row(tool: &ToolCall) -> Option<Line<'static>> {
    if matches!(tool.status, ToolStatus::Waiting | ToolStatus::Running) {
        return None;
    }
    tool.approval_note
        .as_ref()
        .map(|note| result_row(0, note.clone()))
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
///
/// A **running** bullet renders at rest (the flat grey) — this is the renderer
/// that feeds scrollback commits and the frozen transcript, where a colour
/// lasts forever, so it must never capture a frame of the pulse.
/// `live_tool_lines` is the animated one. See `docs/tool-pulse.md`.
#[must_use]
pub fn tool_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    tool_cell_lines(tool, width, None)
}

/// [`tool_lines`] for the **live region**: identical, except a running bullet
/// breathes at the frame `pulse` ([`tool_pulse_color`]). Only the live strip
/// calls this — its rows are redrawn every animation frame and never committed,
/// so the moving colour can't be frozen into scrollback. See
/// `docs/tool-pulse.md`.
#[must_use]
pub(super) fn live_tool_lines(tool: &ToolCall, width: u16, pulse: Duration) -> Vec<Line<'static>> {
    tool_cell_lines(tool, width, Some(pulse))
}

/// The shared body of [`tool_lines`] / [`live_tool_lines`] — `pulse` is `Some`
/// only on a live frame.
fn tool_cell_lines(tool: &ToolCall, width: u16, pulse: Option<Duration>) -> Vec<Line<'static>> {
    let mut lines = tool_cell_body(tool, width, pulse);
    lines.extend(approval_note_row(tool));
    lines
}

/// A resolved `AskUserQuestion` cell (`docs/ask.md`): the output's first line
/// **is** the headline (`User answered Claude's questions:` /
/// `User declined…` / `User wants to chat…`), promoted to the `●` header —
/// green or red by outcome — with the `· Q → A` rows in the `⎿` gutter.
/// `None` while the call runs (the generic header stands) or for any other
/// tool, so the ordinary branches are untouched. `cap` bounds the gutter rows
/// (the inline peek); `None` renders them all (the Ctrl+O transcript).
fn ask_cell_lines(
    tool: &ToolCall,
    width: u16,
    pulse: Option<Duration>,
    cap: bool,
) -> Option<Vec<Line<'static>>> {
    if tool.name != crate::llm::tools::ASK_TOOL_DISPLAY
        || matches!(tool.status, ToolStatus::Waiting | ToolStatus::Running)
    {
        return None;
    }
    let out_lines = tool_output_lines(tool);
    let (headline, rest) = out_lines.split_first()?;
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status, pulse))
        .add_modifier(Modifier::BOLD);
    let head_room = (width as usize).saturating_sub(cols(TOOL_BULLET)).max(1);
    let mut lines = vec![Line::from(vec![
        Span::styled(TOOL_BULLET.to_string(), bullet_style),
        Span::styled(
            truncate_cols(headline, head_room).to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    if rest.is_empty() {
        return Some(lines);
    }
    if cap {
        lines.extend(result_peek_block(
            rest,
            peek_width,
            wrap_output,
            |i, text, _| output_row(i, text),
        ));
    } else {
        let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
        let mut i = 0usize;
        for line in rest {
            for row in wrap_output(line, wrap_width) {
                lines.push(output_row(i, row));
                i += 1;
            }
        }
    }
    Some(lines)
}

/// The **collapsed** MCP cell (`docs/mcp.md`) — inline it is deliberately
/// quiet, the full `{server} - {tool} (MCP)({args})` story living in Ctrl+O:
///
/// - **Running/Waiting**: `● Calling {server}… (ctrl+o to expand)` — the
///   bullet coloured (and, live, breathing) by status — over a one-row peek
///   of the call's primary string argument (`⎿ "query"`), or the dim
///   `⎿ Waiting…` for a batch sibling.
/// - **Resolved ok**: the bullet-less dim
///   `Called {server} (ctrl+o to expand)` line — the settled thinking line's
///   exact shape ([`REASONING_LABEL_COLOR`]), because what is left is a fact
///   about the turn; the result text never reaches inline scrollback.
/// - **Failed / everything else**: `None` — the loud generic red cell (a
///   failure must not whisper).
fn mcp_cell_lines(
    tool: &ToolCall,
    width: u16,
    pulse: Option<Duration>,
) -> Option<Vec<Line<'static>>> {
    let server = crate::mcp::display_server(&tool.name)?.to_string();
    match tool.status {
        ToolStatus::Waiting | ToolStatus::Running => {
            let mut lines = vec![mcp_calling_header(
                &format!("{MCP_CALLING_PREFIX}{server}{MCP_CALLING_SUFFIX}"),
                tool.status,
                pulse,
                width,
            )];
            let peek_width = (width as usize)
                .saturating_sub(cols(TOOL_RESULT_PREFIX))
                .max(1);
            if tool.status == ToolStatus::Waiting {
                lines.push(result_row(0, TOOL_WAITING.to_string()));
            } else if let Some(peek) = crate::mcp::primary_arg(&tool.args) {
                lines.push(output_row(0, truncate_cols(&peek, peek_width)));
            }
            Some(lines)
        }
        ToolStatus::Ok => Some(vec![Line::from(Span::styled(
            format!("{MCP_CALLED_PREFIX}{server}{EXPAND_HINT}"),
            Style::new().fg(REASONING_LABEL_COLOR),
        ))]),
        ToolStatus::Failed | ToolStatus::Backgrounded => None,
    }
}

/// The `● Calling {label}… (ctrl+o to expand)` header row a collapsed MCP
/// cell (or the aggregated all-MCP batch strip) wears — truncated to the
/// width, the hint dropped first when the terminal is too narrow for both.
fn mcp_calling_header(
    label: &str,
    status: ToolStatus,
    pulse: Option<Duration>,
    width: u16,
) -> Line<'static> {
    let bullet_style = Style::new()
        .fg(tool_status_color(status, pulse))
        .add_modifier(Modifier::BOLD);
    let label_room = (width as usize).saturating_sub(cols(TOOL_BULLET)).max(1);
    let mut spans = vec![
        Span::styled(TOOL_BULLET.to_string(), bullet_style),
        Span::styled(
            truncate_cols(label, label_room).to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if cols(TOOL_BULLET) + cols(label) + cols(EXPAND_HINT) <= width as usize {
        spans.push(Span::styled(
            EXPAND_HINT.to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        ));
    }
    Line::from(spans)
}

/// The one aggregated live-strip cell for a parallel batch whose calls are
/// **all** MCP (`docs/mcp.md`): `● Calling deepwiki, context7 3 times…
/// (ctrl+o to expand)` — the servers in call order — over the running call's
/// primary-argument peek. `None` when any call isn't MCP (the ordinary
/// per-cell strip stands).
pub(super) fn mcp_batch_strip_lines(
    queue: &VecDeque<ToolCall>,
    pulse: Duration,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    if queue.len() < 2 {
        return None;
    }
    let servers: Vec<&str> = queue
        .iter()
        .map(|tool| crate::mcp::display_server(&tool.name))
        .collect::<Option<_>>()?;
    let label = format!(
        "{MCP_CALLING_PREFIX}{}{MCP_CALLING_SUFFIX}",
        crate::mcp::batch_label(&servers)
    );
    let mut lines = vec![mcp_calling_header(
        &label,
        ToolStatus::Running,
        Some(pulse),
        width,
    )];
    if let Some(peek) = queue
        .iter()
        .find(|tool| tool.status == ToolStatus::Running)
        .and_then(|tool| crate::mcp::primary_arg(&tool.args))
    {
        let peek_width = (width as usize)
            .saturating_sub(cols(TOOL_RESULT_PREFIX))
            .max(1);
        lines.push(output_row(0, truncate_cols(&peek, peek_width)));
    }
    Some(lines)
}

/// [`tool_cell_lines`] minus the trailing provenance note, so every branch's
/// early return stays as it was and the note lands exactly once.
fn tool_cell_body(tool: &ToolCall, width: u16, pulse: Option<Duration>) -> Vec<Line<'static>> {
    // The resolved ask cell replaces the whole header with its outcome
    // headline (`docs/ask.md`).
    if let Some(lines) = ask_cell_lines(tool, width, pulse, /*cap=*/ true) {
        return lines;
    }
    // The collapsed MCP cell (`docs/mcp.md`) — its failed state falls
    // through to the loud generic path below.
    if let Some(lines) = mcp_cell_lines(tool, width, pulse) {
        return lines;
    }
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
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), pulse);
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
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), pulse);
        lines.extend(body);
        return lines;
    }

    // An `edit`/`write` diff tool: coloured header + a multi-line `⎿` peek whose
    // `+`/`-` rows are diff-coloured (the codex trick shows inline, not just in
    // the Ctrl+O view). Other backend tools keep the single collapsed peek line.
    if is_diff_tool(tool) && tool.status != ToolStatus::Running && !out_lines.is_empty() {
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), pulse);
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
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), pulse);
        lines.extend(peek);
        return lines;
    }

    // Any other backend tool (a `read`/`write`/`edit` cell whose output didn't
    // parse as the numbered/diff format, or an unknown tool): coloured header
    // (wrapped when long) + a single collapsed peek line — white output content,
    // dim placeholder — the rest behind the `… +N lines` hint.
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS), pulse);
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
    let mut lines = tool_full_body(tool, width);
    // The classifier's provenance row closes the expanded cell too
    // (docs/permissions.md).
    lines.extend(approval_note_row(tool));
    lines
}

/// [`tool_full_lines`] minus the trailing provenance note (the
/// [`tool_cell_body`] split).
fn tool_full_body(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    // At rest: the transcript is a pager over a cached, incrementally-built
    // row list (`docs/tool-view-performance.md`) whose refresh short-circuits
    // on a signature that has no clock in it. Animating here would either not
    // move or cost a full-tail re-render every 32 ms, for a bullet nobody is
    // watching breathe. See `docs/tool-pulse.md`.
    let pulse: Option<Duration> = None;
    // The resolved ask cell keeps its headline header in the transcript too,
    // with the answer rows uncapped (`docs/ask.md`).
    if let Some(lines) = ask_cell_lines(tool, width, pulse, /*cap=*/ false) {
        return lines;
    }
    // A backgrounded call shows its fixed row in the transcript too — the
    // live output belongs to the ↓ manager, and the final output arrives as
    // the completion notice (docs/background.md).
    if tool.status == ToolStatus::Backgrounded {
        let row = result_row(0, TOOL_BACKGROUNDED.to_string());
        if tool.shell {
            return vec![row];
        }
        let mut lines = tool_header_lines(tool, width, None, pulse);
        lines.push(row);
        return lines;
    }
    // A numbered `write`/`edit` cell renders wholesale (numbers, tints,
    // syntax colour — [`file_cell_lines`], uncapped here); everything else
    // goes through the plain row pipeline below.
    if let Some(body) = file_cell_lines(tool, width, false) {
        // The Ctrl+O transcript view never truncates the header (`None`).
        let mut lines = tool_header_lines(tool, width, None, pulse);
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
        let mut lines = tool_header_lines(tool, width, None, pulse);
        lines.extend(result);
        lines
    }
}

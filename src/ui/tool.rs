//! Tool cells: the `● name(args)` header, the `⎿` output block, the numbered
//! file-change rendering, and a running command's live tail.
//! See `docs/tools.md` and `docs/tool-streaming.md`.

use super::assistant::expand_code_tabs;
use super::file_cell::{diff_line_color, file_cell_lines, gutter_row, is_diff_tool};
use super::theme::*;
use super::wrap::{WrapMode, cols, truncate_cols, wrap_output, wrap_output_hanging, wrap_verbatim};
use super::*;

/// The bullet colour for a tool's lifecycle: dim waiting, grey running, green
/// ok, red fail — and green for a call that resolved by moving to the
/// background (the launch succeeded; see `docs/background.md`).
///
/// One colour per state, on every frame: a running bullet's animation is the
/// **blink** ([`bullet_span`]), never a second colour, so this is all the
/// colour a cell ever wears. See `docs/tool-pulse.md`.
fn tool_status_color(status: ToolStatus) -> Color {
    match status {
        ToolStatus::Waiting => tool_waiting_color(),
        ToolStatus::Running => tool_running_color(),
        ToolStatus::Ok | ToolStatus::Backgrounded => tool_ok_color(),
        ToolStatus::Failed => tool_fail_color(),
    }
}

/// Whether a blinking bullet is **shown** on the frame at `elapsed`: on for
/// the first half of every [`TOOL_PULSE_PERIOD`], off for the second —
/// Claude-Code's running dot, which flicks between there and not there
/// rather than easing between two greys. Inside each half it holds; there is
/// no in-between frame.
///
/// Pure, like [`shimmer_spans`](super::status) and the comet spinner: the phase
/// derives entirely from the boundary-supplied `elapsed`
/// ([`App::set_pulse`](crate::app::App::set_pulse)), and the loop's 32 ms
/// animation re-arm is what makes it move. Whole milliseconds, so the edge
/// lands on the same frame however the clock is read. See `docs/tool-pulse.md`.
pub(super) fn tool_pulse_visible(elapsed: Duration) -> bool {
    let period = TOOL_PULSE_PERIOD.as_millis().max(1);
    (elapsed.as_millis() % period) * 2 < period
}

/// The `●` a cell header opens with, in `color` — or, on a frame the blink
/// hides it, **blanks of the same width** in the same style, so the header
/// text beside it keeps its column and the row's width never changes
/// (`docs/tool-pulse.md`). `blink` is the live region's frame clock
/// (`App::pulse`) for a bullet that is animating — a **running** call's, and
/// only in the strip; `None` renders it at rest, which is what every
/// scrollback commit and the Ctrl+O transcript do, so a hidden frame can never
/// be frozen into a buffer nobody redraws.
pub(super) fn bullet_span(color: Color, blink: Option<Duration>) -> Span<'static> {
    let style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    match blink {
        Some(at) if !tool_pulse_visible(at) => Span::styled(" ".repeat(cols(TOOL_BULLET)), style),
        _ => Span::styled(TOOL_BULLET.to_string(), style),
    }
}

/// The blink clock for a bullet of `status`: the frame `pulse` while the call
/// is **running**, `None` otherwise. Only `Running` ever animates — a queued
/// sibling and a resolved call mean the same thing on every frame, so making
/// them move would say nothing.
fn running_blink(status: ToolStatus, pulse: Option<Duration>) -> Option<Duration> {
    pulse.filter(|_| status == ToolStatus::Running)
}

/// The coloured bullet header row(s) for a backend tool call: `● {name}({args})`,
/// the bullet recoloured by lifecycle (grey running/green/red) and the args made bold +
/// the normal reply white ([`tool_args_color`]) so a `bash` command reads
/// clearly, the framing `(`/`)` left a dim [`tool_dim_color`] delimiter. Shared
/// by the inline collapsed view ([`tool_lines`]) and the full-screen transcript
/// ([`tool_full_lines`]).
///
/// When the header overflows `width` the args **wrap** across continuation
/// rows, each indented to align **under the opening `(`** (the width of
/// `● {name}`), so a long command reads clean and is never clipped at the
/// terminal edge — Claude Code's wrapped `Bash(…)` header, row for row: words
/// move down whole, and a token that fits on **no** row — a long URL — fills
/// the row it stands on before it is broken (`wrap_output_hanging`, the
/// hanging-indent form of [`wrap_output`]), so every row runs to the edge
/// instead of the first stopping short of the token. The args' own
/// whitespace survives the wrap (a run of spaces inside quotes stays, a
/// newline takes a row of its own, like the permission prompt that asked
/// about the same command), and an argument too wide for the name's row
/// spills to the next one whole. `collapsed` — the inline peek and the live
/// preview — **cuts** the command Claude Code's way ([`header_cut`]): its
/// first [`TOOL_HEADER_MAX_LINES`] lines and [`TOOL_HEADER_MAX_COLS`]
/// columns, then [`TOOL_HEADER_ELLIPSIS`] + `)`, so a huge command doesn't
/// flood the cell; the Ctrl+O transcript passes `false` and renders the
/// whole thing. A `bash` command's `"$(cat <<'EOF' … EOF)"` message is shown
/// as the quoted message itself on both surfaces ([`collapse_heredoc`]). A
/// `!` shell command is a tool with no args (name = the command), so it
/// stays a bare single `● {command}` line.
pub(super) fn tool_header_lines(
    tool: &ToolCall,
    width: u16,
    collapsed: bool,
    pulse: Option<Duration>,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let name_style = Style::new()
        .fg(tool_name_color())
        .add_modifier(Modifier::BOLD);
    let bullet = || {
        bullet_span(
            tool_status_color(tool.status),
            running_blink(tool.status, pulse),
        )
    };
    // An MCP header wears its server capitalized (`Deepwiki - ask_question
    // (MCP)`) while the record keeps the configured spelling — the context
    // replay inverts `tool.name`, so the capitalization lives only at this
    // render seam (`docs/mcp.md`). The same string feeds the width math
    // below: `to_uppercase` can widen a non-ASCII first char, and the
    // painted row and the reserved indent must agree.
    let display = if crate::mcp::is_mcp_display_name(&tool.name) {
        crate::mcp::capitalize_display(&tool.name)
    } else {
        tool.name.clone()
    };
    let name = || Span::styled(display.clone(), name_style);
    // An MCP call stores its **raw arguments JSON** (what makes the record
    // replayable — `docs/mcp.md`); the header derives the pretty
    // `key: "value"` form at render time instead.
    let args = if crate::mcp::is_mcp_display_name(&tool.name) {
        crate::mcp::pretty_args(&tool.args)
    } else if is_command_tool(tool) {
        // The commit idiom's scaffolding says nothing about the command —
        // the header shows the message it carries (render-time only; the
        // record, the prompt and the replay keep the command verbatim).
        collapse_heredoc(&tool.args).unwrap_or_else(|| tool.args.clone())
    } else {
        // A file tool's summary **is** its path, and the header shows it the
        // way the session reads paths — relative under the cwd, `~`-relative
        // under home, absolute elsewhere (`docs/tools.md` *Path display*).
        // The record keeps the model's absolute argument: this, like the MCP
        // capitalization above, lives only at the render seam.
        shown_args(&tool.name, &tool.args, paths)
    };
    if args.is_empty() {
        return vec![Line::from(vec![bullet(), name()])];
    }
    let args_style = Style::new()
        .fg(tool_args_color())
        .add_modifier(Modifier::BOLD);
    // A file tool's path is also a **link to the file** (`docs/links.md`
    // *The file tool header*): the shown text carries the file's absolute
    // `file://` target in the link carrier, so clicking any fragment of it —
    // however the rows below wrap or cut it — opens the whole file. Only the
    // path: the `(`/`)` framing it and the corner rows beneath stay plain,
    // and a path the policy cannot place (a relative argument under the
    // verbatim policy) paints exactly as before.
    let file_link = if crate::llm::tools::is_file_tool(&tool.name) {
        paths.file_url(&tool.args)
    } else {
        None
    };
    let path_style = file_link
        .as_deref()
        .map_or(args_style, |url| crate::links::linked(args_style, url));
    // Continuation rows indent to align under the opening `(`, which sits right
    // after `● {name}` — parens and args alike bold white (a uniform, noticeable
    // header body) — so the wrapped rows land exactly beneath it.
    //
    // That reads best while the name is short. A long one — an MCP call's
    // `deepwiki - ask_question (MCP)` is 31 columns — would spend a third of a
    // terminal on indent and squeeze the arguments into a ragged column, so a
    // header wider than [`TOOL_HEADER_ALIGN_SHARE`] of the width falls back to
    // the bullet's own two columns and the args get the whole row
    // (`docs/mcp.md`). The two rows then have **different** budgets — the first
    // is what is left beside `● {name}(`, the rest what is left beside the
    // indent — which is exactly [`wrap_output_hanging`].
    let aligned = cols(TOOL_BULLET) + cols(&display);
    let indent_cols = if aligned > (width as usize) / TOOL_HEADER_ALIGN_SHARE {
        cols(TOOL_BULLET)
    } else {
        aligned
    };
    // The `(` rides the name's row rather than the first argument word, so an
    // argument too wide for what is left beside the name spills to the
    // continuation row **whole** (`Name(` over `repoName: …`) instead of being
    // hard-broken across the two (`(repoName` / `: …`).
    let first_width = (width as usize)
        .saturating_sub(aligned + cols(TOOL_HEADER_OPEN))
        .max(1);
    let body_width = (width as usize).saturating_sub(indent_cols).max(1);
    // The collapsed cut is a budget on the **text** — the same command is cut
    // at the same character in every width — made before the wrap, so the
    // `…` lands where the text ends and the `)` rides after it, whatever row
    // that falls on. The cut text is trimmed, so the marker attaches to the
    // last kept word: `word …)` would read as a cut after a *missing* word.
    let (shown, cut) = if collapsed {
        header_cut(&args)
    } else {
        (std::borrow::Cow::Borrowed(args.as_str()), false)
    };
    let marker = if cut { TOOL_HEADER_ELLIPSIS } else { "" };
    // The arguments keep their own whitespace — a run of spaces inside quotes
    // is data, a newline a statement boundary that takes a row of its own —
    // wrapped at word boundaries like the output rows under them, with tabs
    // expanded for display the same way (the record stays byte-exact). The
    // closing `)` rides the last line.
    let body = format!("{}{marker}{TOOL_HEADER_CLOSE}", expand_code_tabs(&shown));
    // A wrapped row keeps the space its break fell on at its end; it paints
    // as nothing, so it is dropped here — the rows read the same and the
    // transcript's text stays clean.
    let rows: Vec<String> = wrap_output_hanging(&body, first_width as u16, body_width as u16)
        .into_iter()
        .map(|row| row.trim_end().to_string())
        .collect();
    let last_row = rows.len().saturating_sub(1);
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = if i == 0 {
                vec![
                    bullet(),
                    name(),
                    Span::styled(TOOL_HEADER_OPEN.to_string(), args_style),
                ]
            } else {
                vec![Span::raw(" ".repeat(indent_cols))]
            };
            if row.is_empty() {
                return Line::from(spans);
            }
            if file_link.is_none() {
                spans.push(Span::styled(row, args_style));
                return Line::from(spans);
            }
            // A linked path: its fragments carry the target, while the
            // closing `)` — the last row's final character, appended above —
            // is framing and rides its own plain span.
            let close = i == last_row && row.ends_with(TOOL_HEADER_CLOSE);
            let body = if close {
                row[..row.len() - TOOL_HEADER_CLOSE.len()].to_string()
            } else {
                row
            };
            if !body.is_empty() {
                spans.push(Span::styled(body, path_style));
            }
            if close {
                spans.push(Span::styled(TOOL_HEADER_CLOSE.to_string(), args_style));
            }
            Line::from(spans)
        })
        .collect()
}

/// The collapsed header's cut of a call's `args` — Claude Code's
/// `Bash(…)` rule, ported whole: the first [`TOOL_HEADER_MAX_LINES`] lines,
/// then at most [`TOOL_HEADER_MAX_COLS`] display columns, trimmed at both
/// ends when anything was cut (so the `…` the caller appends never follows a
/// space or a blank line). Returns the text to show and whether it was cut;
/// text within both budgets comes back untouched.
///
/// A budget on the text rather than on rows is what makes the header
/// predictable: the same command shows the same characters in a 40-column
/// terminal and a 200-column one, and a multi-line script shows its first
/// two statements rather than however many the width happened to fit.
pub(super) fn header_cut(args: &str) -> (std::borrow::Cow<'_, str>, bool) {
    let mut kept = args;
    let mut cut = false;
    if let Some((at, _)) = args.match_indices('\n').nth(TOOL_HEADER_MAX_LINES - 1) {
        kept = &args[..at];
        cut = true;
    }
    if cols(kept) > TOOL_HEADER_MAX_COLS {
        let head = truncate_cols(kept, TOOL_HEADER_MAX_COLS);
        return (std::borrow::Cow::Owned(head.trim().to_string()), true);
    }
    if cut {
        (std::borrow::Cow::Borrowed(kept.trim()), true)
    } else {
        (std::borrow::Cow::Borrowed(args), false)
    }
}

/// A `bash` command's `"$(cat <<'EOF'\n{body}\nEOF\n)"` message collapsed into
/// the quoted message itself — `git commit -m "$(cat <<'EOF'\nAdd X\nEOF\n)"`
/// shows as `git commit -m "Add X"` — Claude Code's header rule for the
/// idiom every commit with a body uses: the `cat` scaffolding is how a
/// multi-line string reaches the shell, not what the command does. `None`
/// when the command is not that shape, which leaves it shown verbatim. The
/// shape is Claude Code's own (its regex, ported): the opener must sit on the
/// command's first line, the body runs to the first `EOF` line that is
/// followed by the closing `)"`, and whatever trails the closer must stay on
/// its line — a command that goes on after it is not the idiom. Display
/// only: the record keeps the command byte-exact.
pub(super) fn collapse_heredoc(cmd: &str) -> Option<String> {
    const OPENER: &str = "$(cat <<'EOF'";
    if !cmd.contains("\"$(cat <<'EOF'") {
        return None;
    }
    let first_line_end = cmd.find('\n')?;
    let first_line = &cmd[..first_line_end];
    let at = first_line.find(OPENER)?;
    // The opener must close its line: the body starts on the next one.
    if at + OPENER.len() != first_line_end {
        return None;
    }
    let head = cmd[..at].strip_suffix('"').unwrap_or(&cmd[..at]);
    let after = &cmd[first_line_end + 1..];
    // The body ends at the first newline followed by an `EOF` line and the
    // `)"` closer (whitespace, newlines included, allowed between them).
    let mut search = 0usize;
    while let Some(rel) = after[search..].find('\n') {
        let nl = search + rel;
        let rest = after[nl + 1..].trim_start();
        if let Some(rest) = rest.strip_prefix("EOF\n")
            && let Some(tail) = rest.trim_start().strip_prefix(")\"")
            && !tail.contains('\n')
        {
            let body = &after[..nl];
            return Some(format!(
                "{} \"{}\"{}",
                head.trim(),
                body.trim(),
                tail.trim()
            ));
        }
        search = nl + 1;
    }
    None
}

/// A dim `⎿` result row — for the `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `+N lines (Ns)` footer (meta, not output).
pub(super) fn result_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, None)
}

/// A `⎿` row for a finished tool's **output** — a dim corner over white content
/// ([`tool_output_color`]), so command/shell output reads like a normal reply.
/// The placeholders keep the dim [`result_row`].
fn output_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, Some(tool_output_color()))
}

/// Is this a model **command tool** (`bash`) — rendered like the `!` shell cell
/// (a multi-line `⎿` peek, its `Exit code: N` frame stripped for display) and
/// **tailed** live while running? Other generic backend tools keep the single
/// collapsed peek line. See [`COMMAND_TOOL_NAMES`] and `docs/tool-streaming.md`.
pub(super) fn is_command_tool(tool: &ToolCall) -> bool {
    !tool.shell && COMMAND_TOOL_NAMES.contains(&tool.name.as_str())
}

/// A call's `args` summary as the screen shows it: a file tool's path
/// through the session's [`PathDisplay`] rule (`docs/tools.md` *Path
/// display*), every other tool's — a `bash` command, an agent's prose, an
/// MCP call's JSON — verbatim. The one place that says "a file tool's
/// summary is a path", shared by the cell header, the agent tree's sticky
/// activity row and the Ctrl+O agent cell's nested headers. `name` is the
/// display name (`crate::llm::tools::is_file_tool`); a `!` shell cell's
/// empty summary passes through empty.
pub(super) fn shown_args(name: &str, args: &str, paths: &PathDisplay) -> String {
    if crate::llm::tools::is_file_tool(name) {
        paths.display(args)
    } else {
        args.to_string()
    }
}

/// The dim `… +N lines (ctrl+o to expand)` hint under a capped peek.
pub(super) fn more_hint_line(hidden: usize) -> Line<'static> {
    let dim = Style::new().fg(tool_dim_color());
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
    // A session still alive (`docs/interactive-shell.md`): its frame line is
    // the model's; the cell shows the body, and says where the session stands
    // on its own dim row ([`session_state_row`]).
    if let Some((_, body)) = session_frame(output) {
        return std::borrow::Cow::Borrowed(body);
    }
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

/// The session frame opening a command cell's output — `Running (session
/// …)` or `Stopped (session …)` (`pty::report`) — and the body under it.
/// `None` for every other output: an exited session reports `Exit code: N`
/// like a plain `bash` call.
fn session_frame(output: &str) -> Option<(crate::pty::report::Frame<'_>, &str)> {
    let (first, body) = output.split_once('\n').unwrap_or((output, ""));
    crate::pty::report::parse_frame(first).map(|frame| (frame, body))
}

/// The dim `⎿ Waiting for input · session {id}` corner closing a resolved
/// command cell whose session is still alive — `Waiting for a password` at a
/// prompt that hides what it reads, or `Stopped · session {id}` for one the
/// call ended (`docs/interactive-shell.md`). A fresh corner, like
/// the classifier's provenance row: it is about the call, not output.
fn session_state_row(tool: &ToolCall) -> Option<Line<'static>> {
    use crate::pty::report::{Frame, Waiting};
    if !is_command_tool(tool) || matches!(tool.status, ToolStatus::Waiting | ToolStatus::Running) {
        return None;
    }
    let (state, session) = match session_frame(&tool.output)?.0 {
        Frame::Running {
            session,
            waiting: Waiting::Input,
        } => (SESSION_WAITING_ROW, session),
        Frame::Running {
            session,
            waiting: Waiting::Password,
        } => (SESSION_PASSWORD_ROW, session),
        Frame::Running {
            session,
            waiting: Waiting::No,
        } => (SESSION_RUNNING_ROW, session),
        Frame::Stopped { session } => (SESSION_STOPPED_ROW, session),
    };
    Some(result_row(0, format!("{state}{SESSION_ROW_ID}{session}")))
}

/// A command-style tool's output as display lines — [`tool_output_lines`] with
/// the `Exit code: N` frame reframed for display ([`command_display_output`]).
pub(super) fn command_display_lines(tool: &ToolCall) -> Vec<String> {
    split_display_lines(&command_display_output(&tool.output))
}

/// The lines a **settled** exec cell shows of its output — the `bash` tool
/// and the `!` shell command, one cell shape (`docs/shell-command.md`) — on
/// the inline peek and in the Ctrl+O expansion alike, so the peek's `+N
/// lines` hint counts exactly the rows the expansion adds. Display only, the
/// record byte-exact: the `Exit code: N` frame reframed
/// ([`command_display_output`]; a shell command's output is never framed),
/// each line that is a JSON document reshaped with two-space indentation
/// ([`prettify_json_lines`] — Claude Code's tool result does the same, which
/// is why its `curl` of an API reads `{` / `"batchcomplete": "",` where a
/// compact line read as a wall of braces), tabs expanded, and trailing blank
/// lines dropped (`docs/long-lines.md`) — a formatter's closing newlines are
/// nothing to show and nothing to hide. A generic backend cell whose output
/// is not the numbered file format shares it (the same result surface in
/// Claude Code). The running tail ([`running_command_lines`]) deliberately
/// does not: what is still streaming is shown as it was printed.
pub(super) fn exec_display_lines(tool: &ToolCall) -> Vec<String> {
    let body: std::borrow::Cow<'_, str> = if tool.shell {
        std::borrow::Cow::Borrowed(tool.output.as_str())
    } else {
        command_display_output(&tool.output)
    };
    let mut lines = split_display_lines(&prettify_json_lines(&body));
    while lines.last().is_some_and(|line| is_blank_row(line)) {
        lines.pop();
    }
    lines
}

/// `text` with every line that is a whole JSON document reshaped for display
/// — Claude Code's rule, ported: an output longer than
/// [`TOOL_JSON_PRETTY_MAX_BYTES`] is left alone whole (the bound on what a
/// render may parse), and a line is reshaped only when it **round-trips**
/// ([`pretty_json_line`]). Borrowed back unchanged when nothing qualifies,
/// which is every output that is not JSON.
fn prettify_json_lines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.len() > TOOL_JSON_PRETTY_MAX_BYTES {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut changed = false;
    let lines: Vec<std::borrow::Cow<'_, str>> = text
        .split('\n')
        .map(|line| match pretty_json_line(line) {
            Some(pretty) => {
                changed = true;
                std::borrow::Cow::Owned(pretty)
            }
            None => std::borrow::Cow::Borrowed(line),
        })
        .collect();
    if changed {
        std::borrow::Cow::Owned(lines.join("\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// One output line as a two-space-indented JSON document, when it is one:
/// it parses as a JSON object or array **and** re-serializes to the same
/// text once whitespace is ignored — Claude Code's guard, so a document the
/// command never printed is never shown: duplicate keys (the last wins in the
/// parse), a number the serializer would spell differently, an escape it
/// would resolve, all stay verbatim. Prose, scalars and partial JSON are
/// `None`. The parse is bounded by the caller's size cap and transient — a
/// render's `Value`, dropped with the rows (`docs/memory.md`'s rule is about
/// trees built to read a few fields of a body that is kept).
fn pretty_json_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let compact = serde_json::to_string(&value).ok()?;
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if squash(trimmed) != squash(&compact) {
        return None;
    }
    serde_json::to_string_pretty(&value).ok()
}

/// The clock clause every shape of the running command cell carries —
/// `(22s · wait 1m 50s)`: the command's own `elapsed`
/// ([`format_elapsed`]-humanized, ticking) beside how long its call waits for
/// it (`wait_ms`, [`format_timeout`]-humanized as a whole limit) — a wait, not
/// a deadline: the command goes on as a session when it passes
/// (`docs/bash-tools.md`, `docs/tool-streaming.md` *The clock row is always
/// there*).
fn command_clock_clause(elapsed: Duration, wait_ms: u64) -> String {
    format!(
        "({}{TOOL_CLOCK_SEPARATOR}{TOOL_TIMEOUT_LABEL}{})",
        format_elapsed(elapsed.as_secs()),
        format_timeout(wait_ms)
    )
}

/// The live preview for a **running** command-style backend tool (`bash`): the
/// coloured `● name(args)` header, the **last** [`TOOL_PEEK_ROWS`] display
/// **rows** of its output under the `⎿` gutter (the *tail* — what just
/// streamed), then the **clock row** — `+{hidden} lines ({elapsed} · wait
/// {limit})` when any display rows are fully hidden above the window, the
/// bare `({elapsed} · wait {limit})` when none are, and for a command that
/// has printed nothing yet the clause rides the corner row itself:
/// `⎿ Running… ({elapsed} · wait {limit})`. The clause is on the cell in
/// **every** shape, so a silent `sleep 100` no longer sits on a bare
/// `Running…` for as long as it takes, and the limit is the model's own
/// `wait` read off the call's verbatim arguments
/// ([`bash_wait_ms`](crate::llm::tools::bash_wait_ms) — the executor's
/// default-and-clamp rule, so the cell names exactly what the call waits).
/// This is Claude-Code's running-command look (the mock;
/// `docs/tool-streaming.md`) with the wait beside the clock — the asymmetric twin of the finished
/// head peek in [`tool_lines`]. The `elapsed` is boundary-supplied (like the
/// shell running row and the status timer), so this is drawn from
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
/// Two clocks ride in: `elapsed` is how long the command has run (the clock
/// row's first number), `pulse` is the frame phase its bullet blinks at.
/// Live-only by construction — only the strip calls this — so the blink is
/// unconditional here (`docs/tool-pulse.md`).
pub(super) fn running_command_lines(
    tool: &ToolCall,
    elapsed: Duration,
    pulse: Duration,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, Some(pulse), paths);
    // The wait the call runs under — a session call's own, not `bash`'s
    // (`docs/bash-tools.md`). A companion's display name lowercases to its
    // wire name; the legacy tool's has an underscore to put back.
    let arguments = tool.arguments.as_deref();
    let wait_ms = match tool.name.as_str() {
        "Bash" => crate::llm::tools::bash_wait_ms(arguments),
        crate::llm::tools::BASH_SESSION_TOOL_DISPLAY => {
            crate::llm::tools::session_wait_ms(crate::llm::tools::BASH_SESSION_TOOL_NAME, arguments)
        }
        other => crate::llm::tools::session_wait_ms(&other.to_ascii_lowercase(), arguments),
    };
    let clock = command_clock_clause(elapsed, wait_ms);
    let display = command_display_lines(tool);
    if display.is_empty() {
        // Nothing printed yet: the clause rides the `⎿ Running…` row, so a
        // silent command still shows it is alive and how long it may be.
        lines.push(result_row(0, format!("{TOOL_RUNNING} {clock}")));
        return lines;
    }
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    // The tail window: the last TOOL_PEEK_ROWS wrapped rows, each remembering
    // its source line index *and* how many of that line's rows it dropped, so
    // the footer can count what scrolled off in display rows.
    let mut window: VecDeque<(usize, String)> = VecDeque::new();
    let mut cut = 0usize; // rows dropped off the top of the oldest shown line
    for (idx, line) in display.iter().enumerate().rev() {
        let rows = wrap_output(line, wrap_width);
        let over = (window.len() + rows.len()).saturating_sub(TOOL_PEEK_ROWS);
        cut = over.min(rows.len());
        for row in rows.into_iter().rev() {
            window.push_front((idx, row));
        }
        if window.len() >= TOOL_PEEK_ROWS {
            break;
        }
    }
    while window.len() > TOOL_PEEK_ROWS {
        window.pop_front();
    }
    // Display **rows** above the window — the lines wholly above it plus the
    // rows the oldest shown line lost off its own top. Counting source lines
    // called a 2 KB line that scrolled past "1 line" (`docs/long-lines.md`);
    // `WrapMode::rows` builds nothing, so this stays cheap per animation frame.
    let hidden = window.front().map_or(0, |(idx, _)| {
        cut + display[..*idx]
            .iter()
            .map(|line| WrapMode::Output.rows(line, wrap_width))
            .sum::<usize>()
    });
    let shown = window.len();
    for (i, (_, row)) in window.into_iter().enumerate() {
        lines.push(output_row(i, row));
    }
    // The clock row closes the cell whatever is hidden: `+N lines` in front
    // of the clause when rows scrolled off the window, the clause alone when
    // the output fits. A continuation row (index ≥ 1) so it indents under
    // the content column; it is meta, so it stays the dim `result_row`.
    let footer = if hidden > 0 {
        format!("+{hidden} lines {clock}")
    } else {
        clock
    };
    lines.push(result_row(shown, footer));
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
/// sits flush above (docs/shell-command.md) — and shows its **whole** output
/// as a `⎿` block (each line aligned under the corner, word-wrapped), never
/// folded: the user ran it to read the output, so nothing waits behind a
/// hint, and the dim `…` marker closes a cell the in-memory cap cut. A
/// backend tool keeps its coloured `● name(args)` header over a **folded**
/// peek — `TOOL_FOLD_ROWS` display rows, then `… +N lines (ctrl+o to
/// expand)` when more is hidden (Claude-Code's exec cell) — with the full
/// output only in the separate tool-output view.
///
/// A **running** bullet renders at rest (the flat grey) — this is the renderer
/// that feeds scrollback commits and the frozen transcript, where a colour
/// lasts forever, so it must never capture a frame of the pulse.
/// `live_tool_lines` is the animated one. See `docs/tool-pulse.md`.
#[must_use]
pub fn tool_lines(tool: &ToolCall, width: u16, paths: &PathDisplay) -> Vec<Line<'static>> {
    tool_cell_lines(tool, width, None, paths)
}

/// [`tool_lines`] for the **live region**: identical, except a running bullet
/// blinks at the frame `pulse` ([`bullet_span`]). Only the live strip
/// calls this — its rows are redrawn every animation frame and never committed,
/// so a hidden frame can't be frozen into scrollback. See
/// `docs/tool-pulse.md`.
#[must_use]
pub(super) fn live_tool_lines(
    tool: &ToolCall,
    width: u16,
    pulse: Duration,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    tool_cell_lines(tool, width, Some(pulse), paths)
}

/// The shared body of [`tool_lines`] / [`live_tool_lines`] — `pulse` is `Some`
/// only on a live frame.
///
/// The **quiet resolved MCP cell** skips the classifier's provenance note: its
/// whole inline presence is the one dim `Called {server}` line (`docs/mcp.md`
/// — a parallel run's aggregated line never carried the note either, and in
/// auto mode every server call resolves noted, so the row doubled each cell
/// into pure noise). The record survives where the full story lives — the
/// Ctrl+O transcript ([`tool_full_lines`]), the rollout, a `/resume` — and a
/// *failed* MCP call keeps the note on its loud generic cell, where it still
/// explains why the call ran at all.
fn tool_cell_lines(
    tool: &ToolCall,
    width: u16,
    pulse: Option<Duration>,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = tool_cell_body(tool, width, pulse, paths);
    lines.extend(session_state_row(tool));
    let quiet_mcp =
        tool.status == ToolStatus::Ok && crate::mcp::display_server(&tool.name).is_some();
    if !quiet_mcp {
        lines.extend(approval_note_row(tool));
    }
    lines
}

/// A resolved `AskUserQuestion` cell (`docs/ask.md`): the output's first line
/// **is** the headline (`User answered Alter Zero's questions:` /
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
    let head_room = (width as usize).saturating_sub(cols(TOOL_BULLET)).max(1);
    let mut lines = vec![Line::from(vec![
        bullet_span(
            tool_status_color(tool.status),
            running_blink(tool.status, pulse),
        ),
        Span::styled(
            truncate_cols(headline, head_room).to_string(),
            Style::new()
                .fg(tool_name_color())
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
            WrapMode::Output,
            BlankPolicy::Keep,
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
/// - **Running**: `● Calling {server}… (ctrl+o to expand)` — the bullet
///   coloured by status (and, live, blinking) — and nothing else: echoing a
///   peek of the question under it says what the header already does, at the
///   cost of a row and a wrapped fragment of the argument.
/// - **Waiting**: the same header over the dim `⎿ Waiting…` its non-MCP
///   batch siblings show (only a *mixed* batch renders these — an all-MCP
///   batch collapses to [`mcp_batch_lines`]).
/// - **Resolved ok**: the bullet-less dim
///   `Called {server} (ctrl+o to expand)` line — the settled thinking line's
///   exact shape ([`reasoning_label_color`]), because what is left is a fact
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
            // The lone-call header bypasses `batch_label`, so it capitalizes
            // its one server itself (the aggregated forms get it there).
            let mut lines = vec![mcp_calling_header(
                &crate::mcp::capitalize_server(&server),
                tool.status,
                pulse,
                width,
            )];
            if tool.status == ToolStatus::Waiting {
                lines.push(result_row(0, TOOL_WAITING.to_string()));
            }
            Some(lines)
        }
        ToolStatus::Ok => Some(vec![mcp_called_line(&[&server])]),
        ToolStatus::Failed | ToolStatus::Backgrounded => None,
    }
}

/// The bullet-less dim `Called deepwiki 2 times (ctrl+o to expand)` line a
/// resolved MCP call — or a whole **parallel run** of them ([`mcp_run`]) —
/// leaves in scrollback: the settled thinking line's shape, because what is
/// left is a fact about the turn (`docs/mcp.md`).
pub(super) fn mcp_called_line(servers: &[&str]) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{MCP_CALLED_PREFIX}{}{EXPAND_HINT}",
            crate::mcp::batch_label(servers)
        ),
        Style::new().fg(reasoning_label_color()),
    ))
}

/// The batch and server of a **collapsible** MCP cell: an MCP call that
/// resolved `Ok` inside an announced parallel batch. `None` for anything else
/// — a failure (which stays loud and separate), a lone call, a non-MCP tool.
fn mcp_run_member(item: &HistoryItem) -> Option<(u64, &str)> {
    let HistoryItem::Tool(tool) = item else {
        return None;
    };
    let batch = tool.batch?;
    (tool.status == ToolStatus::Ok)
        .then(|| crate::mcp::display_server(&tool.name))
        .flatten()
        .map(|server| (batch, server))
}

/// The **parallel MCP run** leading `items`: the consecutive collapsible cells
/// sharing the first one's batch id, as `(len, servers-in-call-order)`.
///
/// `None` unless at least two line up — one call's cell is already the same
/// `Called {server}` line, and requiring the shared batch id is what keeps two
/// *sequential* calls that merely landed next to each other in history from
/// claiming they ran in parallel. See `docs/mcp.md`.
pub(super) fn mcp_run(items: &[HistoryItem]) -> Option<(usize, Vec<&str>)> {
    let (batch, first) = mcp_run_member(items.first()?)?;
    let mut servers = vec![first];
    for item in &items[1..] {
        match mcp_run_member(item) {
            Some((id, server)) if id == batch => servers.push(server),
            _ => break,
        }
    }
    (servers.len() > 1).then_some((servers.len(), servers))
}

/// How many trailing cells of `history` are collapsible members of `batch` —
/// the run walk both the commit and the hold are built on.
fn trailing_run_len(history: &[HistoryItem], batch: Option<u64>) -> usize {
    history
        .iter()
        .rev()
        .take_while(|item| mcp_run_member(item).is_some_and(|(id, _)| Some(id) == batch))
        .count()
}

/// The trailing `history` cells of a **parallel MCP run that is still in
/// flight** — the ones [`tool_commit_lines`] is holding because the next call
/// of their batch is about to start (`docs/mcp.md`).
///
/// They are *recorded* but not yet in scrollback, and until the run ends the
/// live strip's aggregated `● Calling deepwiki 2 times…` cell speaks for the
/// whole batch. So a rebuild that happens in that gap — a permission prompt
/// closing between two calls of the batch, a resize — must skip exactly these
/// (`conversation_lines_live`), or it paints a `Called deepwiki` line the
/// commit path is about to replace with the run's own.
#[must_use]
pub fn held_run_len(history: &[HistoryItem], queue: &VecDeque<ToolCall>) -> usize {
    let Some(HistoryItem::Tool(last)) = history.last() else {
        return 0;
    };
    let continues = mcp_run_member(history.last().expect("just matched")).is_some()
        && queue.front().is_some_and(|next| {
            next.batch == last.batch && crate::mcp::display_server(&next.name).is_some()
        });
    if continues {
        trailing_run_len(history, last.batch)
    } else {
        0
    }
}

/// The scrollback lines a **just-resolved** tool call commits — the last item
/// of `history`, with `queue` holding what is left of its batch.
///
/// Normally that is simply its own collapsed cell. But an MCP call inside a
/// parallel batch **holds** its line (`None`) while the next call of the same
/// batch is about to run, so the run commits *once*, as the aggregated
/// `Called deepwiki 2 times (ctrl+o to expand)` line — the very line the
/// repaint builds from the same history ([`conversation_lines`]), so
/// scrollback and a resize can never disagree. Whatever ends the run — its
/// last call, or a failure/rejection in the middle of it — flushes the held
/// cells with it. See `docs/mcp.md`.
#[must_use]
pub fn tool_commit_lines(
    history: &[HistoryItem],
    queue: &VecDeque<ToolCall>,
    width: u16,
    paths: &PathDisplay,
) -> Option<Vec<Line<'static>>> {
    let HistoryItem::Tool(last) = history.last()? else {
        return None;
    };
    // Still mid-run: the next call of this batch is about to start, and it
    // will render with this one.
    if held_run_len(history, queue) > 0 {
        return None;
    }
    // The run this call ends: the cells **held** before it plus this call
    // itself, whatever it resolved as. Held is not merely "trailing member
    // of the batch": a cell was held exactly when the call that ran next —
    // this one, `last` — is another collapsible member of its batch *by
    // name* (an MCP call, whatever it resolved as), the same predicate
    // [`held_run_len`] applied at that cell's own resolution. A resolved
    // MCP cell whose batch continued with a NON-MCP sibling was never held
    // (a mixed batch keeps per-cell rendering): it committed its own
    // `Called {server}` line at its own ToolEnd, and counting it into this
    // flush printed it twice — the reported duplicate.
    let before = if crate::mcp::display_server(&last.name).is_some() {
        trailing_run_len(&history[..history.len() - 1], last.batch)
    } else {
        0
    };
    let items = &history[history.len() - 1 - before..];
    let mut lines = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if !lines.is_empty() {
            lines.push(Line::default()); // the spacer between committed cells
        }
        if let Some((len, servers)) = mcp_run(&items[i..]) {
            lines.push(mcp_called_line(&servers));
            i += len;
        } else {
            match &items[i] {
                HistoryItem::Tool(tool) => lines.extend(tool_lines(tool, width, paths)),
                // Unreachable: the walk above only ever crosses tool cells.
                _ => break,
            }
            // An image `read` commits its picture with its cell, so the live
            // commit and the rebuild ([`conversation_lines`]) write the same
            // rows (`docs/images.md`).
            lines.extend(super::image::image_block_lines(&items[i], width));
            i += 1;
        }
    }
    Some(lines)
}

/// The scrollback lines for the calls a **settle resolved all at once** — the
/// last `count` tool records of `history`. An Esc or a backend error resolves
/// the running call and every `⎿ Waiting…` sibling behind it together
/// (`docs/interrupt.md`), then records its red notice behind them, so the
/// history no longer *ends* at a cell and [`tool_commit_lines`] — built for
/// the one call that just resolved — would find nothing to commit.
///
/// Each record commits exactly as its own resolution would have, through
/// [`tool_commit_lines`] on the history up to it: the first flushes a
/// parallel MCP run it was holding (`docs/mcp.md`), and a blank row
/// separates the cells — the rows a rebuild paints from the same history
/// ([`conversation_lines`]). Nothing is left in the queue to hold for, so
/// nothing is held back. Empty when `count` is zero.
#[must_use]
pub fn resolved_tools_commit_lines(
    history: &[HistoryItem],
    count: usize,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut ends: Vec<usize> = history
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, item)| matches!(item, HistoryItem::Tool(_)))
        .map(|(index, _)| index + 1)
        .take(count)
        .collect();
    ends.reverse();
    let mut lines = Vec::new();
    for end in ends {
        let Some(cell) = tool_commit_lines(&history[..end], &VecDeque::new(), width, paths) else {
            continue;
        };
        if cell.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default()); // the spacer between committed cells
        }
        lines.extend(cell);
    }
    lines
}

/// The `● Calling {label}… (ctrl+o to expand)` header row a collapsed MCP
/// cell (or the aggregated all-MCP batch strip) wears — truncated to the
/// width, the hint dropped first when the terminal is too narrow for both.
/// `label` is the bare server list; the `Calling …` framing is added here so
/// every caller wears it identically.
fn mcp_calling_header(
    label: &str,
    status: ToolStatus,
    pulse: Option<Duration>,
    width: u16,
) -> Line<'static> {
    let label = &format!("{MCP_CALLING_PREFIX}{label}{MCP_CALLING_SUFFIX}");
    let label_room = (width as usize).saturating_sub(cols(TOOL_BULLET)).max(1);
    let mut spans = vec![
        bullet_span(tool_status_color(status), running_blink(status, pulse)),
        Span::styled(
            truncate_cols(label, label_room).to_string(),
            Style::new()
                .fg(tool_name_color())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if cols(TOOL_BULLET) + cols(label) + cols(EXPAND_HINT) <= width as usize {
        spans.push(Span::styled(
            EXPAND_HINT.to_string(),
            Style::new().fg(tool_dim_color()),
        ));
    }
    Line::from(spans)
}

/// The one aggregated live cell for an in-flight batch whose calls are
/// **all** MCP (`docs/mcp.md`): `● Calling deepwiki, context7 3 times…
/// (ctrl+o to expand)` — the servers in call order, and nothing else. A
/// parallel batch is one act, so it reads as one line whether it is drawn in
/// the streaming strip ([`preview_tool_lines`](super::live::preview_tool_lines),
/// with a `pulse`) or above a permission prompt asking about one of its calls
/// (at rest, `pulse: None`).
///
/// The count is the **batch's**, not the queue's: the siblings that already
/// resolved are read back off the history, so a running batch's label doesn't
/// count itself down. `None` when the queue is empty or holds anything that
/// isn't an MCP call — a mixed batch keeps the ordinary per-cell rendering.
pub(super) fn mcp_batch_lines(
    app: &App,
    pulse: Option<Duration>,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let queue = app.tool_queue();
    if queue.is_empty() {
        return None;
    }
    let mut servers: Vec<&str> = queue
        .iter()
        .map(|tool| crate::mcp::display_server(&tool.name))
        .collect::<Option<_>>()?;
    // The batch's already-resolved calls, in call order, ahead of the queued
    // ones: the trailing history cells stamped with this batch's id.
    if let Some(batch) = queue.front().and_then(|tool| tool.batch) {
        let mut done: Vec<&str> = Vec::new();
        for item in app.history.iter().rev() {
            match item {
                HistoryItem::Tool(tool) if tool.batch == Some(batch) => {
                    match crate::mcp::display_server(&tool.name) {
                        Some(server) => done.push(server),
                        None => break,
                    }
                }
                _ => break,
            }
        }
        done.reverse();
        servers.splice(0..0, done);
    }
    Some(vec![mcp_calling_header(
        &crate::mcp::batch_label(&servers),
        ToolStatus::Running,
        pulse,
        width,
    )])
}

/// [`tool_cell_lines`] minus the trailing provenance note, so every branch's
/// early return stays as it was and the note lands exactly once.
fn tool_cell_body(
    tool: &ToolCall,
    width: u16,
    pulse: Option<Duration>,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
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
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, pulse, paths);
        lines.push(row);
        return lines;
    }

    if tool.shell {
        // The `!` shell cell never folds (docs/shell-command.md): the user
        // ran the command to read its output, so every display line shows
        // inline — the rows the Ctrl+O view paints, blanks kept — and the
        // dim `…` marker closes a cell the in-memory cap cut, since the
        // `… +N lines` hint that used to say more followed is gone. The
        // running/empty single-row states stay.
        let display = exec_display_lines(tool);
        let mut lines = match tool.status {
            // A shell command is never batched, so it is never `Waiting`; the
            // arm is here only to keep the match total and correct if it ever is.
            ToolStatus::Waiting => return vec![result_row(0, TOOL_WAITING.to_string())],
            ToolStatus::Running => return vec![result_row(0, TOOL_RUNNING.to_string())],
            _ if display.is_empty() => vec![result_row(0, TOOL_NO_OUTPUT.to_string())],
            _ => result_full_block(&display, peek_width),
        };
        if tool.truncated {
            lines.push(result_row(lines.len(), TOOL_TRUNCATED_MARKER.to_string()));
        }
        return lines;
    }

    // A `write`/`edit` cell in the numbered `llm::tools` format renders
    // codex-style — numbers, hunk gaps, tints, syntax colour
    // ([`file_cell_lines`]); output that doesn't parse (old sessions, error
    // bodies) falls through to the legacy first-char colouring below.
    if let Some(body) = file_cell_lines(tool, width, true) {
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, pulse, paths);
        lines.extend(body);
        return lines;
    }

    // An `edit`/`write` diff tool: coloured header + a multi-line `⎿` peek whose
    // `+`/`-` rows are diff-coloured (the codex trick shows inline, not just in
    // the Ctrl+O view). Other backend tools keep the single collapsed peek line.
    if is_diff_tool(tool) && tool.status != ToolStatus::Running && !out_lines.is_empty() {
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, pulse, paths);
        // Wrap verbatim (a diff body is code, never reflowed at spaces) and
        // colour every wrapped row by the SOURCE line's `+`/`-` marker, so a
        // continuation row keeps its tint — the Ctrl+O view colours the same
        // way (docs/tools.md).
        lines.extend(result_peek_block(
            &out_lines,
            peek_width,
            WrapMode::Verbatim,
            BlankPolicy::Keep,
            |i, text, src| gutter_row(i, text, diff_line_color(src)),
        ));
        return lines;
    }

    // A backend **command tool** (`bash`): coloured header (wrapped when long)
    // over a multi-line `⎿` peek — the *head*, folded at TOOL_FOLD_ROWS display
    // rows behind `… +N lines (ctrl+o to expand)`, like the `!` shell cell
    // (the mock's finished state). The `Exit code: N` frame is stripped and a
    // JSON line reshaped for display (`exec_display_lines`,
    // docs/tool-streaming.md); no output yet → the `⎿ Running…`/`Waiting…` row.
    // The running *tail* (last lines + elapsed) is a separate live-only render
    // (`running_command_lines`), used by the preview.
    if is_command_tool(tool) {
        let display = exec_display_lines(tool);
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
            _ => result_peek_block(
                &display,
                peek_width,
                WrapMode::Output,
                BlankPolicy::FirstBlock,
                |i, text, _| output_row(i, text),
            ),
        };
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, pulse, paths);
        lines.extend(peek);
        return lines;
    }

    // Any other backend tool (a `read`/`write`/`edit` cell whose output didn't
    // parse as the numbered/diff format — an image read's fact line, a
    // placeholder, an error body — or an unknown tool): coloured header
    // (wrapped when long) + the same folded peek the exec cells show — white
    // output content, dim placeholder — the rest behind the `… +N lines`
    // hint, so it word-wraps to the width and hides its remainder behind an
    // honest row count (`docs/long-lines.md`).
    let mut lines = tool_header_lines(tool, width, /*collapsed=*/ true, pulse, paths);
    let display = exec_display_lines(tool);
    match tool.status {
        ToolStatus::Waiting => lines.push(result_row(0, TOOL_WAITING.to_string())),
        ToolStatus::Running => lines.push(result_row(0, TOOL_RUNNING.to_string())),
        _ if display.is_empty() => lines.push(result_row(0, TOOL_NO_OUTPUT.to_string())),
        _ => lines.extend(result_peek_block(
            &display,
            peek_width,
            WrapMode::Output,
            BlankPolicy::Keep,
            |i, text, _| output_row(i, text),
        )),
    }
    lines
}

/// How a peek block treats a **blank** source line — a row with nothing on it
/// but spaces (`docs/long-lines.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlankPolicy {
    /// Every line as it comes. A diff body's spacing is content, and an ask
    /// cell's `· Q → A` rows have no blanks to skip.
    Keep,
    /// Command output: the peek is the output's **first block** — any leading
    /// blank lines are skipped, and the first blank line after them closes it.
    /// A four-row cell cannot afford to spend rows on nothing, and a peek that
    /// hopped the gap would read as one run of lines that isn't one.
    FirstBlock,
}

impl BlankPolicy {
    /// The slice of `out_lines` this policy shows. [`Self::Keep`] is the whole
    /// slice — and so is [`Self::FirstBlock`] over an output with no non-blank
    /// line anywhere: there is no first block to prefer, and an empty window
    /// would leave the hint with no `⎿` corner to hang from.
    fn window(self, out_lines: &[String]) -> std::ops::Range<usize> {
        let all = 0..out_lines.len();
        if self != Self::FirstBlock {
            return all;
        }
        let Some(start) = out_lines.iter().position(|line| !is_blank_row(line)) else {
            return all;
        };
        let end = out_lines[start..]
            .iter()
            .position(|line| is_blank_row(line))
            .map_or(out_lines.len(), |n| start + n);
        start..end
    }
}

/// Whether a display line paints as an empty row. Tabs are already expanded by
/// [`split_display_lines`], so trimming spaces is the whole test.
fn is_blank_row(line: &str) -> bool {
    line.trim().is_empty()
}

/// Every display row of `out_lines`, **unfolded**: each line wrapped to
/// `peek_width` ([`WrapMode::Output`] — word boundaries, spaces preserved,
/// the Ctrl+O view's wrapper), the first row under the `⎿` corner and every
/// later one aligned beneath it ([`output_row`]). The `!` shell cell's block
/// (`docs/shell-command.md`): the same rows [`tool_full_body`] paints for the
/// transcript, so the inline cell and Ctrl+O agree row for row.
fn result_full_block(out_lines: &[String], peek_width: usize) -> Vec<Line<'static>> {
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    out_lines
        .iter()
        .flat_map(|line| wrap_output(line, wrap_width))
        .enumerate()
        .map(|(i, text)| output_row(i, text))
        .collect()
}

/// The head peek of `out_lines`, folded Claude Code's way: its first
/// [`TOOL_FOLD_ROWS`] display **rows** — each line wrapped to `peek_width`
/// with `mode` ([`WrapMode::Output`] for command/shell output: word
/// boundaries, spaces preserved, like the Ctrl+O view; [`WrapMode::Verbatim`]
/// for diff bodies — code, hard-break, never reflowed at spaces) — then the
/// `… +N lines (ctrl+o to expand)` hint, built with `row` (which also
/// receives the **source** line, so a diff cell can colour a wrapped
/// continuation by the source's `+`/`-` marker). An output of exactly
/// `TOOL_FOLD_ROWS + 1` rows is shown **whole**: a hint hiding one row would
/// cost the very row it hides, so the block is at most [`TOOL_PEEK_ROWS`]
/// rows either way, hint included (`docs/long-lines.md`).
///
/// `blanks` picks the window the fold is then spent inside
/// ([`BlankPolicy`]): command output shows its **first block**, so a leading
/// `\n` never costs the cell its first row and a blank line closes the peek
/// rather than being painted as one; everything else keeps every line.
///
/// The hint counts **display rows** not shown — the rows pressing Ctrl+O
/// actually adds, counted with the same `mode` the expansion wraps with —
/// instead of source lines, which is how 1.8 KB of hidden JSON used to
/// report itself as `+1 lines`. That count stays exact under a window: the
/// blanks skipped above the block and the one that closed it are rows the
/// expansion adds, so they are counted like any other hidden row. Counting
/// allocates nothing ([`WrapMode::rows`]), so this stays cheap even on a
/// 64 KiB output.
fn result_peek_block(
    out_lines: &[String],
    peek_width: usize,
    mode: WrapMode,
    blanks: BlankPolicy,
    row: impl Fn(usize, String, &str) -> Line<'static>,
) -> Vec<Line<'static>> {
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    let rows_of = |lines: &[String]| {
        lines
            .iter()
            .map(|line| mode.rows(line, wrap_width))
            .sum::<usize>()
    };
    let mut lines: Vec<Line> = Vec::new();
    let push_rows = |lines: &mut Vec<Line>, line: &str, take: usize| {
        for text in mode.wrap(line, wrap_width).into_iter().take(take) {
            // The very first display row of the block gets the `⎿` corner
            // ([`gutter_row`]'s index 0); every later row — a wrapped
            // continuation or the next source line — indents under the content
            // column, exactly like the uncapped Ctrl+O block.
            let at = lines.len();
            lines.push(row(at, text, line));
        }
    };
    // Nothing to fold: at most one row past the fold, and a hint would cost
    // exactly that row — so every row shows, blank lines included.
    if rows_of(out_lines) <= TOOL_FOLD_ROWS + 1 {
        for line in out_lines {
            push_rows(&mut lines, line, usize::MAX);
        }
        return lines;
    }
    let window = blanks.window(out_lines);
    let block = &out_lines[window.clone()];
    // Everything outside the window is hidden whole: the blanks skipped above
    // the block, and the blank that closed it with all that follows.
    let mut hidden = rows_of(&out_lines[..window.start]) + rows_of(&out_lines[window.end..]);
    for (i, line) in block.iter().enumerate() {
        let room = TOOL_FOLD_ROWS.saturating_sub(lines.len());
        if room == 0 {
            // Past the fold: every remaining line is hidden whole.
            hidden += rows_of(&block[i..]);
            break;
        }
        let rows = mode.rows(line, wrap_width);
        hidden += rows.saturating_sub(room);
        push_rows(&mut lines, line, room);
    }
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
pub(super) fn tool_full_lines(
    tool: &ToolCall,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = tool_full_body(tool, width, paths);
    lines.extend(session_state_row(tool));
    // The classifier's provenance row closes the expanded cell too
    // (docs/permissions.md).
    lines.extend(approval_note_row(tool));
    lines
}

/// [`tool_full_lines`] minus the trailing provenance note (the
/// [`tool_cell_body`] split).
fn tool_full_body(tool: &ToolCall, width: u16, paths: &PathDisplay) -> Vec<Line<'static>> {
    // At rest: the transcript is a pager over a cached, incrementally-built
    // row list (`docs/tool-view-performance.md`) whose refresh short-circuits
    // on a signature that has no clock in it. Animating here would either not
    // move or cost a full-tail re-render every 32 ms, for a bullet nobody is
    // watching blink. See `docs/tool-pulse.md`.
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
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ false, pulse, paths);
        lines.push(row);
        return lines;
    }
    // A numbered `write`/`edit` cell renders wholesale (numbers, tints,
    // syntax colour — [`file_cell_lines`], uncapped here); everything else
    // goes through the plain row pipeline below.
    if let Some(body) = file_cell_lines(tool, width, false) {
        // The Ctrl+O transcript view never truncates the header (`None`).
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ false, pulse, paths);
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
        // The same display lines the inline peek folds (`exec_display_lines`:
        // the `Exit code: N` frame stripped, JSON reshaped, tabs expanded,
        // trailing blanks dropped — the stored output stays byte-exact), every
        // row of them, so the peek's `+N lines` is exactly what shows here. An
        // output that is blank lines only reads `(no output)` on both.
        _ => {
            let display = exec_display_lines(tool);
            if display.is_empty() {
                vec![(TOOL_NO_OUTPUT.to_string(), None)]
            } else {
                display
                    .iter()
                    .flat_map(|line| wrap_output(line, body_width))
                    .map(|line| (line, Some(tool_output_color())))
                    .collect()
            }
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
        let mut lines = tool_header_lines(tool, width, /*collapsed=*/ false, pulse, paths);
        lines.extend(result);
        lines
    }
}

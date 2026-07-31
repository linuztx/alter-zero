//! Painting the live region: the streaming strip above the input box, the box
//! itself, and the band below it.
//!
//! Every commit clears and repaints this region inside one synchronized frame,
//! so no flushed state ever lacks the box (`docs/flicker.md`). The strip's
//! preview shows a running tool's whole cell — the parallel batch in
//! `docs/parallel-tools.md`, a streaming command's tail in
//! `docs/tool-streaming.md`.

use super::agent::agent_view_preview_lines;
use super::layout::{input_box, live_layout};
use super::theme::*;
use super::tool::{
    command_display_lines, is_command_tool, live_tool_lines, result_row, running_command_lines,
    shell_running_line,
};
use super::*;

/// Split one wrapped input row into spans, styling the parts inside
/// `highlights` with [`SEARCH_HIGHLIGHT`] — the Ctrl+R match preview
/// (codex highlights its textarea the same way). `row` is the text of the
/// row whose byte range into the whole input is `range`; `highlights` are
/// sorted, non-overlapping byte ranges into that same text
/// ([`App::search_highlight_ranges`]). With no highlights the row comes back
/// as one plain span.
fn highlight_row_spans(
    row: &str,
    range: &Range<usize>,
    highlights: &[Range<usize>],
) -> Vec<Span<'static>> {
    if highlights.is_empty() {
        return vec![Span::raw(row.to_string())];
    }
    let style = Style::new().add_modifier(SEARCH_HIGHLIGHT);
    let mut spans = Vec::new();
    let mut pos = range.start;
    for h in highlights {
        // This row's slice of the highlight (a match can span wrapped rows).
        let start = h.start.clamp(pos, range.end);
        let end = h.end.clamp(pos, range.end);
        if start >= end {
            continue;
        }
        if start > pos {
            spans.push(Span::raw(
                row[pos - range.start..start - range.start].to_string(),
            ));
        }
        spans.push(Span::styled(
            row[start - range.start..end - range.start].to_string(),
            style,
        ));
        pos = end;
    }
    if pos < range.end {
        spans.push(Span::raw(row[pos - range.start..].to_string()));
    }
    spans
}

/// Render the bottom live region into `buf`. While a reply streams, the strip's
/// top row previews the in-progress line and the row below it is a blank gap, so
/// the reply never touches the rule-framed, **growing** input box; idle, the
/// strip collapses and the box sits at the top of the region. The input wraps
/// across as many rows as `area` allows; the prompt marks its first line and
/// continuation lines are indented to align under it. When the input is taller
/// than the box, it scrolls internally to keep the **cursor's wrapped row** in
/// view (`input_scroll` follows the cursor wherever the user has moved it).
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App) {
    render_live_with_preview(area, buf, app, None);
}

/// The streaming strip's preview line(s) for `app` at `width` — exactly what the
/// preview slot draws, and whose count is [`preview_rows`] (they must agree, so
/// the box and cursor sit right). A running backend tool previews its **whole**
/// collapsed cell (the wrapped `● name(args)` header + its `⎿ Running…` row) so a
/// long command isn't clipped and the running state shows (req 2); a running `!`
/// shell command previews one `⎿ Running… (Ns)` row (its elapsed rides here since
/// the status line is hidden); a streaming reply previews its last line — or the
/// whole forming table (docs/table-streaming.md) — `stream_preview` is the
/// boundary's cheap render of it ([`StreamRender::preview`], falling back to
/// re-rendering the last line from the buffer when absent, for unit tests).
/// Empty when there is nothing to preview (the pre-stream pause / idle).
fn preview_lines(
    app: &App,
    width: u16,
    stream_preview: Option<&[Line<'static>]>,
) -> Vec<Line<'static>> {
    if let Some(run) = app.viewed_agent() {
        agent_view_preview_lines(run, app.pulse(), width)
    } else if app.agent_group().is_some() || !app.tool_queue().is_empty() {
        preview_tool_lines(app, width)
    } else if let Some(lines) = stream_preview {
        lines.to_vec()
    } else {
        app.streaming_text()
            .filter(|t| !t.is_empty())
            .map(|text| {
                message_lines(Role::Assistant, text, width)
                    .pop()
                    .unwrap_or_default()
            })
            .into_iter()
            .collect()
    }
}

/// The live tool queue rendered as preview rows: each call's collapsed cell,
/// blank-line-separated so a **parallel batch** reads like the committed
/// scrollback (the running call live, each not-yet-started sibling a dim
/// `⎿ Waiting…` cell — `docs/parallel-tools.md`). A running backend `bash` cell
/// that has streamed output **tails** it — the header + last lines + a
/// `+N lines (Ns)` footer (`running_command_lines`; the mock,
/// `docs/tool-streaming.md`) — before any output arrives it is the plain
/// `⎿ Running…` peek. A lone `!` shell run collapses to its single
/// `⎿ Running… (Ns)` row (the elapsed rides the preview since a shell turn hides
/// the status line); the shell is never batched, so it is always the only call.
/// Shared by [`preview_lines`] (drawn) and [`preview_rows`] (sized) so the two
/// agree by construction (the strip's `debug_assert`).
pub(super) fn preview_tool_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let elapsed = app.status().map_or(Duration::ZERO, |s| s.elapsed);
    // The shared animation phase every running bullet in this strip breathes
    // against (`docs/tool-pulse.md`) — distinct from `elapsed`, which is the
    // *turn's* runtime and is displayed.
    let pulse = app.pulse();
    let mut lines = Vec::new();
    // The round's live agent group leads the strip — its blue tree cell over
    // any ordinary tool cells of a mixed round (docs/agent-tool.md).
    lines.extend(live_agent_group_lines(app, width));
    for (i, tool) in app.tool_queue().iter().enumerate() {
        if i > 0 || !lines.is_empty() {
            lines.push(Line::default()); // blank row between batch cells
        }
        if tool.shell && tool.status == ToolStatus::Running {
            lines.push(shell_running_line(elapsed));
        } else if is_command_tool(tool)
            && tool.status == ToolStatus::Running
            && !command_display_lines(tool).is_empty()
        {
            // A running backend command tool (bash) with streamed output tails it
            // live; other running tools fall to their plain `⎿ Running…` peek.
            lines.extend(running_command_lines(tool, elapsed, pulse, width));
        } else {
            // The live cell: a running bullet pulses here and only here
            // (`docs/tool-pulse.md`).
            lines.extend(live_tool_lines(tool, width, pulse));
        }
        // A running command (a model `bash` call or the `!` shell) can be
        // moved to the background with Ctrl+B — hint it under the live cell,
        // but only once the command has been running a few seconds
        // (`TOOL_BACKGROUND_HINT_DELAY`), Claude-Code-style: a command that
        // finishes right away never flashes the hint (Ctrl+B still works the
        // whole time — only the hint waits). The command's own elapsed is
        // boundary-injected each frame (`App::command_elapsed`). Live-only by
        // construction: this renderer never feeds scrollback commits, so the
        // hint is never committed (docs/background.md).
        if tool.status == ToolStatus::Running
            && (tool.shell || is_command_tool(tool))
            && app
                .command_elapsed()
                .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
        {
            lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
        }
    }
    lines
}

/// [`render_live`], but with the streaming strip's assistant-preview line(s)
/// supplied by the caller (the boundary's cheap [`StreamRender::preview`] —
/// O(one line), or the forming table's rows) instead of re-rendering the whole
/// reply here (which was O(reply) *every animation frame* and starved the
/// status spinner — `docs/markdown.md`). A `None` `stream_preview` falls back
/// to rendering the last line from the buffer, so unit tests (which don't
/// thread a `StreamRender`) keep their old behaviour; production always passes
/// `Some`, its row count injected via [`App::set_stream_preview_rows`] so
/// [`preview_rows`] sizes the same rows this draws (docs/table-streaming.md).
pub fn render_live_with_preview(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    stream_preview: Option<&[Line<'static>]>,
) {
    // A pending tool-permission request replaces the whole live region — the
    // streaming strip included, since the turn is blocked on the answer. It
    // wins over every other inline view (it is modal). See
    // `docs/permissions.md`.
    if app.permission().is_some() {
        render_permission(area, buf, app);
        return;
    }
    // The inline `/model` picker replaces the whole live region — the composer,
    // strip, band, and footer all give way to its own framed body. See
    // `docs/llm.md`.
    if let Some(picker) = &app.model_picker {
        render_model_picker(area, buf, picker);
        return;
    }
    // The inline `/login` onboarding flow likewise replaces the whole region.
    if let Some(onboarding) = &app.key_onboarding {
        render_key_onboarding(area, buf, onboarding);
        return;
    }
    // The ↓ background manager band likewise replaces the whole region. See
    // `docs/background.md`.
    if app.background_view.is_some() {
        render_background_view(area, buf, app);
        return;
    }
    // The band below the box holds the palette, the shortcuts overview, *or* the
    // `@` file picker (band_rows — mutually exclusive). Queued messages render
    // in the strip *above* the box instead; the session-context footer takes
    // the very last row unless a band displaces it, and the agent roster's
    // rows sit below it (docs/agent-tool.md).
    let band = band_rows(app);
    let queued = queued_rows(app, area.width);
    let toast = toast_rows(app);
    let footer = footer_rows(app, band);
    let agent_rows = agent_list_rows(app);
    // The preview row + its gap are only reserved when there is something to
    // preview; the pre-stream pause shows status-only (no stray blank line).
    // The status row + its gap are reserved unless this is a `!` shell turn,
    // which hides the spinner status and shows its elapsed in the preview.
    // The preview line(s): a running backend tool's whole cell (wrapped header +
    // `⎿ Running…`), a shell run's `⎿ Running… (Ns)`, or the reply's last line —
    // empty during the pre-stream pause / idle. `preview_rows` is the **single
    // source of truth** the box + cursor geometry size by (`cursor_position`,
    // `main.rs`); the strip layout here uses it too, and the drawn `preview_lines`
    // must match it exactly — same state, same width, so they agree by
    // construction. The `debug_assert` catches any future drift (a desync would
    // reserve one height but paint another, unseating the box/cursor).
    let preview = preview_lines(app, area.width, stream_preview);
    let preview_n = preview_rows(app, area.width);
    debug_assert_eq!(
        usize::from(preview_n),
        preview.len(),
        "preview_rows() must equal the drawn preview_lines()"
    );
    let has_status = strip_has_status(app);
    let [strip, _, band_area, footer_area, agent_area] = live_layout(
        area, has_status, preview_n, queued, toast, band, footer, agent_rows,
    );
    // Rows the preview slot (content + its trailing gap) / status each occupy at
    // the strip's top (0 when absent).
    let preview_slot = if preview_n > 0 {
        preview_n + GAP_ROWS
    } else {
        0
    };
    let status_rows = if has_status {
        STATUS_ROWS + STATUS_GAP_ROWS
    } else {
        0
    };

    // Strip preview at the top; the row below the last preview line is the blank
    // gap. A running tool takes precedence (its coloured cell shows what's
    // executing); otherwise the reply's last line previews. Nothing during the
    // pre-stream pause / idle (an empty `preview` reserves no rows, no stray
    // bullet — codex parity).
    if preview_n > 0 {
        let preview_area = Rect {
            height: preview_n.min(strip.height),
            ..strip
        };
        Paragraph::new(preview).render(preview_area, buf);
    }

    // The live status line, pinned below the preview (or at the strip top during
    // the pause), just above the box, while a turn is in flight — suppressed for
    // a `!` shell turn (has_status false), whose elapsed rides the preview above.
    // An agent session view shows the *viewed agent's* synthesized status
    // instead of the main turn's (docs/agent-tool.md).
    if has_status {
        let line = if let Some(run) = app.viewed_agent() {
            Some(status_line(&agent_view_status(run)))
        } else {
            app.status().map(status_line)
        };
        if let Some(line) = line {
            let status_y = strip.y + preview_slot;
            if status_y < strip.y + strip.height {
                let status_area = Rect {
                    x: strip.x,
                    y: status_y,
                    width: strip.width,
                    height: STATUS_ROWS,
                };
                Paragraph::new(line).render(status_area, buf);
            }
        }
    }

    // The queued messages, styled like sent user messages (❯ bullet, dark
    // background, wrapped), stacked below the status's gap and just above the
    // box's top rule — only while a turn streams (the only time the queue is
    // non-empty). codex's pending-input preview, in our user-message style. The
    // status slot is 0 rows for a shell turn (status_rows), so the queue sits
    // flush under the preview's gap then.
    if queued > 0 {
        let q_y = strip.y + preview_slot + status_rows;
        let strip_bottom = strip.y + strip.height;
        if q_y < strip_bottom {
            let q_area = Rect {
                x: strip.x,
                y: q_y,
                width: strip.width,
                height: queued.min(strip_bottom - q_y),
            };
            Paragraph::new(queued_lines(app, q_area.width)).render(q_area, buf);
        }
    }

    // The transient toast, on the strip's very last row — directly above the
    // box's top rule, below the status/queue when a turn streams and directly
    // above the box when idle. Self-clears after a few seconds (the expiry is
    // timed at the boundary). See docs/toast.md.
    if toast > 0 {
        let strip_bottom = strip.y + strip.height;
        let t_y = strip_bottom.saturating_sub(toast);
        if t_y < strip_bottom {
            let t_area = Rect {
                x: strip.x,
                y: t_y,
                width: strip.width,
                height: toast.min(strip_bottom - t_y),
            };
            Paragraph::new(toast_line(app, t_area.width)).render(t_area, buf);
        }
    }

    // The input box: a top/bottom rule framing the wrapped input rows. An
    // agent session view carries the agent's description as a right-aligned
    // label on the top rule (docs/agent-tool.md).
    let bx = input_box(
        area, &app.input, has_status, preview_n, queued, toast, band, footer, agent_rows,
    );
    let mut block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_COLOR));
    if let Some(run) = app.viewed_agent() {
        block = block.title_top(
            Line::from(Span::styled(
                format!(" {} ", run.description),
                Style::new().fg(TOOL_DIM_COLOR),
            ))
            .right_aligned(),
        );
    }
    block.render(bx.frame, buf);

    // While a Ctrl+R search previews a match, the query's occurrences in it
    // light up reversed+bold (codex's textarea highlight); otherwise the rows
    // render as single plain spans.
    let highlights = app.search_highlight_ranges();
    let lines: Vec<Line> = bx
        .rows
        .iter()
        .zip(&bx.row_ranges)
        .enumerate()
        .skip(bx.scroll)
        .take(bx.text.height as usize)
        .map(|(i, (line, range))| {
            // The prompt prefixes the real first line; wrapped/continuation lines
            // get a matching-width indent so the text stays aligned under it. In
            // shell mode the absorbed `!` renders back as a red prompt (`! pwd`
            // instead of `❯ pwd` — docs/shell-command.md).
            let (prefix, style) = if i != 0 {
                (INDENT, Style::default())
            } else if app.shell_mode {
                (SHELL_BULLET, Style::new().fg(SHELL_MODE_COLOR))
            } else {
                (PROMPT, Style::new().fg(PROMPT_COLOR))
            };
            let mut spans = vec![Span::styled(prefix, style)];
            spans.extend(highlight_row_spans(line, range, &highlights));
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).render(bx.text, buf);

    // The palette, the shortcuts overview, or the file picker, pinned in the
    // band below the box (at most one is open — band_rows).
    if menu_rows(app) > 0 {
        Paragraph::new(command_menu_lines(app, band_area.width)).render(band_area, buf);
    } else if shortcuts_rows(app) > 0 {
        Paragraph::new(shortcuts_lines(
            app.turn_active(),
            app.has_backtrack_target(),
        ))
        .render(band_area, buf);
    } else if file_menu_rows(app) > 0 {
        Paragraph::new(file_menu_lines(app, band_area.width)).render(band_area, buf);
    }

    // The session-context footer on the region's last row — only when no band
    // is open (the band takes its place; see docs/footer.md). An open Ctrl+R
    // search (docs/history-search.md), a `!command` shell mode
    // (docs/shell-command.md), a primed backtrack (docs/backtrack.md), or the
    // roster selection's hints (docs/agent-tool.md) take the same slot with
    // their own line.
    if footer > 0 {
        let line = if let Some(search) = app.history_search.as_ref() {
            search_line(search)
        } else if app.shell_mode {
            shell_mode_line()
        } else if app.backtrack.primed {
            backtrack_hint_line()
        } else if app.agent_selection().is_some() {
            agent_hint_line(app)
        } else {
            footer_line(app, footer_area.width)
        };
        Paragraph::new(line).render(footer_area, buf);
    }

    // The agent roster below the footer — the persistent `● main` + `◯ …`
    // list while agents exist (docs/agent-tool.md).
    if agent_rows > 0 {
        Paragraph::new(agent_list_lines(app, agent_area.width)).render(agent_area, buf);
    }
}

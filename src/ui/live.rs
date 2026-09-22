//! Painting the live region: the streaming strip above the input box, the box
//! itself, and the band below it.
//!
//! Every commit clears and repaints this region inside one synchronized frame,
//! so no flushed state ever lacks the box (`docs/flicker.md`). The strip's
//! preview shows a running tool's whole cell — the parallel batch in
//! `docs/parallel-tools.md`, a streaming command's tail in
//! `docs/tool-streaming.md`.

use super::agent::{agent_view_preview_lines, agent_view_rule_label};
use super::layout::{
    fit_preview_rows, input_box, key_onboarding_rows, live_layout, model_picker_rows,
    strip_other_rows, view_split,
};
use super::reasoning::live_reasoning_lines;
use super::theme::*;
use super::tool::{
    is_command_tool, live_tool_lines, result_row, running_command_lines, shell_running_line,
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
/// An **open thinking phase** previews its live block — the blinking
/// `● Thinking…` header over the tail of the chain-of-thought
/// (`docs/thinking-stream.md`) — after the agent/tool branches (what is
/// genuinely executing is what the user waits on) and before the reply's,
/// which cannot be streaming while the model is still thinking.
/// Empty when there is nothing to preview (the pre-stream pause / idle).
///
/// `rows` is the slot the caller reserved ([`fitted_preview_rows`], or the
/// unclamped [`preview_rows`] where nothing squeezes it): the walk builds what
/// the content wants and this **trims to it**, so the drawn rows can never
/// outrun the reserved ones. Which end survives is what the preview is *of* —
/// a reply's frontier tail-follows (its newest rows are the ones still
/// arriving, `StreamRender::preview`'s own rule), while a queue of live cells
/// or a thinking block keeps its head (the running call, the `● Thinking…`
/// header — the rows that say what is happening).
pub(super) fn preview_lines(
    app: &App,
    width: u16,
    stream_preview: Option<&[Line<'static>]>,
    rows: u16,
) -> Vec<Line<'static>> {
    let rows = usize::from(rows);
    if let Some(run) = app.viewed_agent() {
        // The viewed agent's own strip — fed the same boundary-built frontier
        // the main branch below gets, since its commits go through the same
        // kind of `StreamRender` (`docs/agent-view-streaming.md`).
        let streaming = run.tool_queue.is_empty() && run.reasoning().is_none();
        trim_preview(
            agent_view_preview_lines(run, app.pulse(), width, stream_preview, app.path_display()),
            rows,
            streaming,
        )
    } else if app.agent_group().is_some() || !app.tool_queue().is_empty() {
        trim_preview(preview_tool_lines(app, width), rows, false)
    } else if let Some(text) = app.reasoning() {
        trim_preview(live_reasoning_lines(text, app.pulse(), width), rows, false)
    } else if let Some(lines) = stream_preview {
        trim_preview(lines.to_vec(), rows, true)
    } else {
        trim_preview(
            app.streaming_text()
                .filter(|t| !t.is_empty())
                .map(|text| {
                    message_lines(Role::Assistant, text, width)
                        .pop()
                        .unwrap_or_default()
                })
                .into_iter()
                .collect(),
            rows,
            true,
        )
    }
}

/// Trim built preview rows to the `rows` the strip reserved — from the front
/// when the newest rows are the point (`tail`), off the end otherwise. A no-op
/// while the slot is big enough, which is every terminal that isn't cramped.
fn trim_preview(mut lines: Vec<Line<'static>>, rows: usize, tail: bool) -> Vec<Line<'static>> {
    if lines.len() > rows {
        if tail {
            lines.drain(..lines.len() - rows);
        } else {
            lines.truncate(rows);
        }
    }
    lines
}

/// One live call's strip rows: a `!` shell run's single `⎿ Running… (Ns)`
/// row, a running backend command tool (`bash`) **tailing its streamed
/// output** (`running_command_lines` — the header + last lines + a
/// `+N lines (Ns · timeout …)` clock row, or `⎿ Running… (Ns · timeout …)`
/// before any output, `docs/tool-streaming.md`), else the plain live
/// cell whose running bullet pulses (`docs/tool-pulse.md`).
///
/// The one renderer for a live cell, shared by the main strip
/// ([`preview_tool_lines`]) and the **agent session view's**
/// ([`super::agent::agent_view_preview_lines`]) — a subagent's running
/// command tails its output exactly like the main turn's, which a second
/// thinner copy of this walk did not (`docs/agent-view-streaming.md`).
/// `elapsed` is the **running command's own** runtime — what every `(Ns)`
/// here shows: the main strip's boundary-injected [`App::command_elapsed`],
/// the agent view's [`AgentRun::command_elapsed`](crate::agents::AgentRun::command_elapsed)
/// — never the turn's or the agent's elapsed, which the status line counts.
pub(super) fn live_call_lines(
    tool: &ToolCall,
    elapsed: Duration,
    pulse: Duration,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    if tool.shell && tool.status == ToolStatus::Running {
        return vec![shell_running_line(elapsed)];
    }
    if is_command_tool(tool) && tool.status == ToolStatus::Running {
        return running_command_lines(tool, elapsed, pulse, width, paths);
    }
    live_tool_lines(tool, width, pulse, paths)
}

/// The live tool queue rendered as preview rows: each call's collapsed cell,
/// blank-line-separated so a **parallel batch** reads like the committed
/// scrollback (the running call live, each not-yet-started sibling a dim
/// `⎿ Waiting…` cell — `docs/parallel-tools.md`). A running backend `bash` cell
/// that has streamed output **tails** it — the header + last lines + a
/// `+N lines (Ns · timeout …)` clock row (`running_command_lines`; the mock,
/// `docs/tool-streaming.md`) — before any output arrives it is the
/// `⎿ Running… (Ns · timeout …)` row, the same clock on the corner row. A
/// lone `!` shell run collapses to its single
/// `⎿ Running… (Ns)` row (the elapsed rides the preview since a shell turn hides
/// the status line); the shell is never batched, so it is always the only call.
/// Shared by [`preview_lines`] (drawn) and [`preview_rows`] (sized) so the two
/// agree by construction (the strip's `debug_assert`).
pub(super) fn preview_tool_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    // The `(Ns)` a running command shows is the command's **own** runtime —
    // the boundary's per-command clock (`App::command_elapsed`, started at
    // the call's `ToolStart` / the `!` shell's launch), never the turn's
    // `elapsed` the status line counts: a `bash` call that started a minute
    // into a turn used to open on `+N lines (60s)`, the status indicator's
    // number copied under a cell that had just begun
    // (docs/tool-streaming.md). Never masked — `background_hint_elapsed` is
    // the Ctrl+B hint's *gate*, blanked under a picker that keeps this strip
    // on screen. Zero until the first injection (the boundary injects before
    // every draw; a unit test may not).
    let elapsed = app.command_elapsed().unwrap_or(Duration::ZERO);
    // The shared animation phase every running bullet in this strip blinks
    // against (`docs/tool-pulse.md`) — distinct from `elapsed`, which is a
    // measurement and is displayed.
    let pulse = app.pulse();
    let mut lines = Vec::new();
    // The round's live agent group leads the strip — its blue tree cell over
    // any ordinary tool cells of a mixed round (docs/agent-tool.md).
    lines.extend(live_agent_group_lines(app, width));
    // A batch whose calls are ALL MCP collapses to one aggregated
    // `● Calling {servers} {n} times…` cell (`docs/mcp.md`); a mixed batch
    // keeps the ordinary per-cell strip below. `preview_rows` sizes from this
    // same walk, so the count and the paint agree.
    if lines.is_empty()
        && let Some(batch) = super::tool::mcp_batch_lines(app, Some(pulse), width)
    {
        return batch;
    }
    for (i, tool) in app.tool_queue().iter().enumerate() {
        if i > 0 || !lines.is_empty() {
            lines.push(Line::default()); // blank row between batch cells
        }
        lines.extend(live_call_lines(
            tool,
            elapsed,
            pulse,
            width,
            app.path_display(),
        ));
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
                .background_hint_elapsed()
                .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
        {
            lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
        }
    }
    lines
}

/// Every row of the streaming strip for `app` at `width`, in paint order:
/// the preview slot and its trailing gap, the status line with the task
/// checklist hanging off it and its trailing gap, the queued messages, and
/// the toast on the last row. Exactly `strip_rows(has_status, preview_n,
/// task_rows) + queued_rows + toast_rows` lines, so the rows `live_height` /
/// `live_layout` reserve and the rows this draws agree by construction.
///
/// `preview_n` is the preview slot's size — `render_live` passes the content's
/// **full ask**, so a squeezed region's strip is taller than its rect and the
/// bottom anchor drops the head (which `ui::view_flow`'s strip branch
/// commits to scrollback instead — `docs/strip-flow.md`); `render_strip_above` passes the
/// rows its own split left, where nothing flows and the list fits exactly.
pub(super) fn strip_lines(
    app: &App,
    width: u16,
    stream_preview: Option<&[Line<'static>]>,
    preview_n: u16,
) -> Vec<Line<'static>> {
    let mut lines = preview_lines(app, width, stream_preview, preview_n);
    if !lines.is_empty() {
        // The blank gap under the preview, so the reply/cell never touches
        // the status line below it (GAP_ROWS).
        lines.push(Line::default());
    }
    // The task checklist directly under the status line (inside the status
    // slot, above its trailing gap — docs/task-tools.md). Built from the same
    // (app, width) as `task_rows`, so the reserved rows and the painted ones
    // agree by construction.
    let tasks = super::tasks::task_lines(app, width);
    let has_status = strip_has_status(app);
    if has_status {
        // The live status line, just above the box while a turn is in flight
        // — suppressed for a `!` shell turn (has_status false), whose elapsed
        // rides the preview above. An agent session view shows the *viewed
        // agent's* synthesized status instead of the main turn's
        // (docs/agent-tool.md). Either opens with the session's chosen
        // spinner style (`/spinner`, docs/spinner.md). A blank row stands
        // in if the status somehow went while `has_status` said otherwise,
        // so the count still matches `strip_rows`' STATUS_ROWS.
        let line = if let Some(run) = app.viewed_agent() {
            Some(styled_status_line(
                &agent_view_status(run),
                None,
                app.spinner(),
                width,
            ))
        } else {
            // While some task is in progress the spinner wears its
            // activeForm instead of the turn's verb (docs/task-tools.md).
            app.status()
                .map(|status| styled_status_line(status, app.task_verb(), app.spinner(), width))
        };
        lines.push(line.unwrap_or_default());
        // The checklist's `⎿` rows hang directly off the status line —
        // Claude Code's live task list (docs/task-tools.md).
        lines.extend(tasks);
        lines.push(Line::default()); // STATUS_GAP_ROWS
    } else if !tasks.is_empty() {
        // No status line to hang from (the turn is over, or a `!` shell run
        // owns the strip): the standalone block — its count line over the
        // rows — sits at the strip top instead, so a plan with work left
        // stays visible while the user reads and types (docs/task-tools.md).
        lines.extend(tasks);
        lines.push(Line::default());
    }
    // The queued messages, styled like sent user messages (❯ bullet, dark
    // background, wrapped), stacked below the status's gap and just above the
    // box's top rule — only while a turn streams (the only time the queue is
    // non-empty). codex's pending-input preview, in our user-message style.
    lines.extend(queued_lines(app, width));
    // The transient toast, on the strip's very last row — directly above the
    // box's top rule, below the status/queue when a turn streams and directly
    // above the box when idle. Self-clears after a few seconds (the expiry is
    // timed at the boundary). See docs/toast.md.
    if toast_rows(app) > 0 {
        lines.push(toast_line(app, width));
    }
    lines
}

/// Paint the streaming strip into `strip`, **bottom-anchored**: a strip whose
/// content outgrew its rows keeps its *last* ones — the newest streamed rows,
/// the `+N lines` footer, the status line — and its head goes to scrollback
/// through `ui::view_flow`'s strip branch rather than nowhere at all
/// (`docs/strip-flow.md`). A no-op reshuffle while everything fits, which is
/// every terminal with room for the turn.
fn render_strip(
    strip: Rect,
    buf: &mut Buffer,
    app: &App,
    stream_preview: Option<&[Line<'static>]>,
    preview_n: u16,
) {
    super::view_flow::render_framed_tail(
        strip,
        buf,
        strip_lines(app, strip.width, stream_preview, preview_n),
    );
}

/// The preview rows the strip's **content** is built at in the conversation
/// view — the content's full ask ([`preview_rows`]) for the live cells that
/// reach no other buffer until they resolve (a running tool queue, an agent
/// group, a thinking block), so the rows the region cannot paint can flow
/// into scrollback instead of being trimmed away.
///
/// A streaming **reply's** frontier is the exception, and takes the reserved
/// count ([`fitted_preview_rows`]) so it contributes nothing to the flow:
/// `StreamRender` commits every completed line to scrollback as it lands, so
/// the frontier's dropped rows are not lost — they are the rows *about to be*
/// committed — and flowing a frontier that grows on every chunk would re-sign
/// the flow, and purge-rebuild the screen, once per chunk
/// (`docs/strip-flow.md`).
pub(super) fn strip_content_preview_rows(app: &App, width: u16, term_height: u16) -> u16 {
    if strip_preview_is_live_cells(app) {
        preview_rows(app, width)
    } else {
        fitted_preview_rows(app, width, term_height)
    }
}

/// Whether the preview slot is showing **live cells** — a running/queued tool
/// call, a live agent group, or an open thinking phase — rather than a
/// streaming reply's frontier. What [`strip_content_preview_rows`] branches
/// on; mirrors [`preview_rows`]' own branch order.
fn strip_preview_is_live_cells(app: &App) -> bool {
    if let Some(run) = app.viewed_agent() {
        return !run.tool_queue.is_empty() || run.reasoning().is_some();
    }
    app.agent_group().is_some() || !app.tool_queue().is_empty() || app.reasoning().is_some()
}

/// The rows the strip's content occupies at `preview_n` preview rows —
/// [`strip_lines`]' length, without building it. The cheap early-out the flow
/// check runs on every draw tick: a strip that fits its slice cannot flow, and
/// wrapping every queued message to find that out would cost a build per frame
/// (`docs/strip-flow.md`).
///
/// An over-estimate at worst, never an under-estimate: for a streaming reply's
/// frontier the caller passes the *reserved* count and the fallback build can
/// come back shorter, so "fits" here always means "fits".
pub(super) fn strip_content_rows(app: &App, width: u16, preview_n: u16) -> u16 {
    super::layout::strip_rows(
        strip_has_status(app),
        preview_n,
        super::tasks::task_rows(app, width),
    )
    .saturating_add(queued_rows(app, width))
    .saturating_add(toast_rows(app))
}

/// The key the strip's scrollback flow is **signed** on
/// (`ui::view_flow`'s `FlowSign::Frozen`, `docs/strip-flow.md`): what the
/// strip is *of*, with every ticking part left out.
///
/// The strip moves on its own at the turn's 32 ms animation cadence — output
/// streams into the running cell, the elapsed and the token tally advance,
/// the bullet blinks — so signing its rows would purge-rebuild the screen
/// thirty times a second. This hashes only what a **structural** change moves:
/// which conversation is on screen, whether a turn's status line is up, each
/// queued call's name/arguments/status, the live agent group's members, and
/// whether a thinking phase is open. A new call, a
/// resolution, a new round or the turn ending re-signs it; a streamed line
/// does not, so the flowed rows freeze where they were committed. (The width
/// and the flowed row count are hashed by the caller, so a resize — and any
/// change to how much overflows — re-signs too.)
pub(super) fn strip_flow_key(app: &App) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    app.agent_view.hash(&mut hasher);
    strip_has_status(app).hash(&mut hasher);
    app.reasoning().is_some().hash(&mut hasher);
    app.streaming_text().is_some().hash(&mut hasher);
    let queue = app
        .viewed_agent()
        .map_or_else(|| app.tool_queue(), |run| &run.tool_queue);
    queue.len().hash(&mut hasher);
    for call in queue {
        call.name.hash(&mut hasher);
        call.args.hash(&mut hasher);
        // `ToolStatus` is a plain enum with no payload — its debug name is a
        // stable discriminant and needs no `Hash` derive on a public type.
        format!("{:?}", call.status).hash(&mut hasher);
    }
    if let Some(group) = app.agent_group() {
        group.background.hash(&mut hasher);
        group.ids.hash(&mut hasher);
    }
    hasher.finish()
}

/// Paint the streaming strip above a **composer-replacing** inline view — the
/// `/model` picker, the `/login` flow, the `/settings` menu, the ↓ background
/// manager band. They stand in for the composer only, so what is executing
/// stays on screen above them: the running tool's live cell, the spinner
/// status line, the queued follow-ups, the toast (`docs/llm.md`,
/// `docs/background.md`). `strip` is the top half of [`view_split`], whose
/// bottom half is the view's own frame; `layout::strip_above_rows` reserves
/// exactly these rows.
fn render_strip_above(
    strip: Rect,
    buf: &mut Buffer,
    app: &App,
    stream_preview: Option<&[Line<'static>]>,
) {
    // The strip here is `view_split`'s leftover, so the fit is against the
    // rows it actually got: the view's own frame is pinned and the preview
    // gives way, which is the same order `preview_budget` enforces in the
    // composer's region. Without it a tall forming table clipped the status
    // line off the bottom of its own strip.
    let preview_n = fit_preview_rows(
        strip.height,
        preview_rows(app, strip.width),
        strip_other_rows(app, strip.width),
    );
    debug_assert_eq!(
        usize::from(preview_n),
        preview_lines(app, strip.width, stream_preview, preview_n).len(),
        "preview_rows() must equal the drawn preview_lines()"
    );
    render_strip(strip, buf, app, stream_preview, preview_n);
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
    // The `AskUserQuestion` modal replaces the whole live region — the
    // streaming strip included, since the turn is blocked on the answers.
    // Checked first: the two modals queue behind each other, so at most one
    // is ever open. See `docs/ask.md`.
    if app.ask().is_some() {
        render_ask(area, buf, app);
        return;
    }
    // A pending tool-permission request replaces the whole live region — the
    // streaming strip included, since the turn is blocked on the answer. It
    // wins over every other inline view (it is modal). See
    // `docs/permissions.md`.
    if app.permission().is_some() {
        render_permission(area, buf, app);
        return;
    }
    // The inline `/model` picker replaces the **composer** — its own framed
    // body takes the composer's rows plus the band's and the footer's — but
    // never the streaming strip: a running tool's live cell, the status line,
    // the queued messages and the toast keep their rows above it, so opening
    // `/model` mid-turn never hides the turn it was opened beside (the
    // reported bug; the ↓ manager band's rule — `docs/llm.md`,
    // `docs/background.md`). `model_picker_height` reserves the same sum.
    if let Some(picker) = &app.model_picker {
        let [strip, body] = view_split(area, model_picker_rows(picker, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_model_picker(body, buf, picker);
        return;
    }
    // The inline `/login` onboarding flow sits under the same strip.
    if let Some(onboarding) = &app.key_onboarding {
        let [strip, body] = view_split(area, key_onboarding_rows(onboarding, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_key_onboarding(body, buf, onboarding);
        return;
    }
    // …and so does the inline `/settings` menu. See `docs/settings.md`.
    if app.settings_picker.is_some() {
        let [strip, body] = view_split(area, super::settings_view::settings_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_settings(body, buf, app);
        return;
    }
    // …and the inline `/mascot` picker, the `/settings` menu's twin. See
    // `docs/mascot.md`.
    if app.mascot_picker.is_some() {
        let [strip, body] = view_split(area, super::mascot_view::mascot_menu_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_mascot_picker(body, buf, app);
        return;
    }
    // …and the inline `/spinner` picker, the `/mascot` picker's twin — the
    // one picker whose subject is the strip it keeps above itself. See
    // `docs/spinner.md`.
    if app.spinner_picker.is_some() {
        let [strip, body] = view_split(
            area,
            super::spinner_view::spinner_menu_rows(app, area.width),
        );
        render_strip_above(strip, buf, app, stream_preview);
        render_spinner_picker(body, buf, app);
        return;
    }
    // …and the inline `/theme` picker, the `/spinner` picker's twin. See
    // `docs/theme.md`.
    if app.theme_picker.is_some() {
        let [strip, body] = view_split(area, super::theme_view::theme_menu_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_theme_picker(body, buf, app);
        return;
    }
    // …and the inline `/skills` menu, the `/settings` menu's twin. See
    // `docs/skills.md`.
    if app.skills_menu.is_some() {
        let [strip, body] = view_split(area, super::skills_view::skills_menu_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_skills_menu(body, buf, app);
        return;
    }
    // …and the read-only `/hooks` menu — its body is the built line count,
    // the ↓ manager band's rule. See `docs/hooks-menu.md`.
    if app.hooks_menu.is_some() {
        let body_h = u16::try_from(super::hooks_view::hooks_view_lines(app, area.width).len())
            .unwrap_or(u16::MAX);
        let [strip, body] = view_split(area, body_h);
        render_strip_above(strip, buf, app, stream_preview);
        render_hooks_menu(body, buf, app);
        return;
    }
    // …and the read-only `/donate` page, its sibling — the same built-line
    // body. See `docs/donate.md`.
    if app.donate_picker.is_some() {
        let [strip, body] = view_split(area, super::donate_view::donate_menu_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        render_donate_picker(body, buf, app);
        return;
    }
    // …and the read-only `/export` page, the `/donate` page's sibling. See
    // `docs/export.md`.
    if app.export_picker.is_some() {
        let [strip, body] = view_split(area, super::export_view::export_menu_rows(app, area.width));
        render_strip_above(strip, buf, app, stream_preview);
        super::export_view::render_export_picker(body, buf, app);
        return;
    }
    // …and the `/trust` review menu, its sibling. See
    // `docs/project-config.md`.
    if app.trust_menu.is_some() {
        let body_h = u16::try_from(super::trust_view::trust_view_lines(app, area.width).len())
            .unwrap_or(u16::MAX);
        let [strip, body] = view_split(area, body_h);
        render_strip_above(strip, buf, app, stream_preview);
        render_trust_menu(body, buf, app);
        return;
    }
    // …and the `/mcp` manager, the hooks menu's twin. See `docs/mcp.md`.
    if app.mcp_menu.is_some() {
        let body_h = u16::try_from(super::mcp_view::mcp_view_lines(app, area.width).len())
            .unwrap_or(u16::MAX);
        let [strip, body] = view_split(area, body_h);
        render_strip_above(strip, buf, app, stream_preview);
        render_mcp_menu(body, buf, app);
        return;
    }
    // The ↓ background manager band replaces the composer (and the band/footer
    // slots below it) — but **not** the streaming strip: a running tool's live
    // cell, the status line, the queued messages and the toast keep their rows
    // above it, so opening the manager mid-turn never hides what is executing
    // (the user-reported fix). The band is pinned at the bottom with its full
    // height (`Length` wins when the clamped region can't fit both, squeezing
    // the strip first); `background_view_height` reserves the same sum. See
    // `docs/background.md`.
    if app.background_view.is_some() {
        let band_h =
            u16::try_from(background_view_lines(app, area.width).len()).unwrap_or(u16::MAX);
        let [strip, band] = view_split(area, band_h);
        render_strip_above(strip, buf, app, stream_preview);
        render_background_view(band, buf, app);
        return;
    }
    // The band below the box holds the palette, the shortcuts overview, *or* the
    // `@` file picker (band_rows — mutually exclusive). Queued messages render
    // in the strip *above* the box instead; the session-context footer takes
    // the very last row unless a band displaces it, and the agent roster's
    // rows sit below it (docs/agent-tool.md).
    let band = band_rows(app, area.width);
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
    let preview_n = fitted_preview_rows(app, area.width, area.height);
    debug_assert_eq!(
        usize::from(preview_n),
        preview_lines(app, area.width, stream_preview, preview_n).len(),
        "fitted_preview_rows() must equal the drawn preview_lines()"
    );
    let has_status = strip_has_status(app);
    let tasks_n = super::tasks::task_rows(app, area.width);
    let [strip, _, band_area, footer_area, agent_area] = live_layout(
        area, has_status, preview_n, tasks_n, queued, toast, band, footer, agent_rows,
    );
    // The strip's *content* is built at the preview's full ask, not the rows
    // the region reserved: a squeezed strip then bottom-anchors, keeping its
    // newest rows on screen while its head flows into scrollback frozen
    // (`ui::view_flow`'s strip branch, `docs/strip-flow.md`). They are the
    // same list when
    // everything fits, which is the ordinary terminal.
    render_strip(
        strip,
        buf,
        app,
        stream_preview,
        strip_content_preview_rows(app, area.width, area.height),
    );

    // The input box: a top/bottom rule framing the wrapped input rows. An
    // agent session view carries the agent's description as a right-aligned
    // label on the top rule (docs/agent-tool.md).
    let bx = input_box(
        area, &app.input, has_status, preview_n, tasks_n, queued, toast, band, footer, agent_rows,
    );
    let mut block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(border_color()));
    // A description is the model's own sentence, so the label is clipped to
    // half the rule before it is right-aligned into it: ratatui cuts an
    // over-wide right-aligned title off its LEFT end, which ate the whole
    // frame and lost the head of the description with it.
    if let Some(label) = app
        .viewed_agent()
        .and_then(|run| agent_view_rule_label(&run.description, bx.frame.width))
    {
        // The label as a lit chip — the theme's accent under the on-accent
        // ink, its padding spaces inside the fill — with a bare border cell
        // after it, so the rule resumes for one glyph past the text
        // (`── {description} ─`) and the chip reads as set into the frame
        // rather than dangling off its right end.
        block = block.title_top(
            Line::from(vec![
                Span::styled(
                    label,
                    Style::new()
                        .fg(agent_view_label_fg())
                        .bg(agent_view_label_bg()),
                ),
                Span::styled(
                    AGENT_VIEW_RULE_TAIL.to_string(),
                    Style::new().fg(border_color()),
                ),
            ])
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
                (SHELL_BULLET, Style::new().fg(shell_mode_color()))
            } else {
                (PROMPT, Style::new().fg(prompt_color()))
            };
            let mut spans = vec![Span::styled(prefix, style)];
            spans.extend(highlight_row_spans(line, range, &highlights));
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).render(bx.text, buf);

    // The palette, the shortcuts overview, the file picker, or the skill
    // picker, pinned in the band below the box (at most one is open —
    // band_rows).
    if menu_rows(app, area.width) > 0 {
        Paragraph::new(command_menu_lines(app, band_area.width)).render(band_area, buf);
    } else if shortcuts_rows(app) > 0 {
        Paragraph::new(shortcuts_lines(
            app.turn_active(),
            app.has_backtrack_target(),
        ))
        .render(band_area, buf);
    } else if file_menu_rows(app) > 0 {
        Paragraph::new(file_menu_lines(app, band_area.width)).render(band_area, buf);
    } else if skill_menu_rows(app) > 0 {
        Paragraph::new(skill_menu_lines(app, band_area.width)).render(band_area, buf);
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

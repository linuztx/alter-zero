//! Subagent rendering: the live `● Running {n} agents…` tree, the recorded
//! group cells, the footer roster, and the inline session view.
//! See `docs/agent-tool.md`.

use super::theme::*;
use super::tool::{bullet_span, result_row, shown_args};
use super::wrap::{cols, truncate_cols};
use super::*;

/// The group header's noun phrase: `2 agents` / `1 agent`.
fn agent_count_phrase(count: usize) -> String {
    if count == 1 {
        "1 agent".to_string()
    } else {
        format!("{count} agents")
    }
}

/// ` · {n} tool use[s] · {tokens} tokens` — a tree row's counters clause
/// (omitted while both are zero, and on a background-launch cell).
fn agent_counters_clause(tool_uses: usize, tokens: u64) -> String {
    let mut clause = String::new();
    if tool_uses > 0 || tokens > 0 {
        let plural = if tool_uses == 1 { "" } else { "s" };
        clause.push_str(&format!(" · {tool_uses} tool use{plural}"));
        clause.push_str(&format!(
            " · {} tokens",
            format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX))
        ));
    }
    clause
}

/// Clip `text` to `max` display columns, marking a cut with
/// [`TOOL_HEADER_ELLIPSIS`].
///
/// Every agent activity row **clips, never wraps**: what it shows can be a
/// whole `bash` command or an MCP argument blob, and a wrapped one would grow
/// the tree (or the lone agent's cell) taller under counters that tick every
/// frame.
fn clip_cols(text: &str, max: usize) -> String {
    if cols(text) <= max {
        return text.to_string();
    }
    let mut clipped = truncate_cols(text, max.saturating_sub(cols(TOOL_HEADER_ELLIPSIS)));
    clipped.push_str(TOOL_HEADER_ELLIPSIS);
    clipped
}

/// The agent session view's composer label — ` {description} ` — clipped to
/// what the top rule can spare, or `None` when it can spare nothing.
///
/// The label is right-aligned *inside* the rule, and ratatui skids an
/// over-wide right-aligned line off its **left** end rather than cutting its
/// right: a wordy description therefore ate the whole frame and lost its own
/// head, reading as a stray sentence with a `─` after it. So the budget is
/// spent here instead — at most `width / AGENT_VIEW_LABEL_DIVISOR` columns
/// for the label, its two padding spaces and the [`AGENT_VIEW_RULE_TAIL`]
/// included, the description cut with [`TOOL_HEADER_ELLIPSIS`] — which keeps
/// the *head* (what the agent is for) and always leaves the rest reading as a
/// rule (`docs/agent-tool.md`).
///
/// `None` once the budget cannot hold the mark **and** one real character
/// beside it: a lone ` … ─` names nothing, so a rule that narrow keeps its
/// frame rather than spending it on punctuation. A description that *fits*
/// still rides there — the mark is what needs the room, and nothing is cut.
pub(super) fn agent_view_rule_label(description: &str, width: u16) -> Option<String> {
    let chrome = cols(" ") * 2 + cols(AGENT_VIEW_RULE_TAIL);
    let budget = (usize::from(width) / AGENT_VIEW_LABEL_DIVISOR).saturating_sub(chrome);
    if cols(description) <= budget {
        return (budget > 0).then(|| format!(" {description} "));
    }
    (budget > cols(TOOL_HEADER_ELLIPSIS)).then(|| format!(" {} ", clip_cols(description, budget)))
}

/// One `⎿  {activity}` row — the dim gutter (`prefix`: the tree's own
/// indent + rail + corner, or the lone cell's [`TOOL_RESULT_PREFIX`]) and the
/// clipped activity in `color`. The one row every agent's state is drawn as,
/// so the tree and the lone cell can never drift apart.
fn agent_activity_row(prefix: &str, activity: &str, color: Color, width: u16) -> Line<'static> {
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::new().fg(tool_dim_color())),
        Span::styled(
            clip_cols(
                activity,
                (width as usize).saturating_sub(cols(prefix)).max(1),
            ),
            Style::new().fg(color),
        ),
    ])
}

/// The activity row's colour: red once an agent has failed or been stopped,
/// dim everywhere else (a status word is not news while it is working).
fn agent_status_color(status: crate::agents::AgentStatus) -> Color {
    match status {
        crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
            tool_fail_color()
        }
        _ => tool_dim_color(),
    }
}

/// One agent's two tree rows: the connector + description + dim counters,
/// then the rail + `⎿  {status}`. Rows truncate at the width (Claude Code's
/// truncate-end), so the tree never wraps.
fn agent_tree_rows(
    is_last: bool,
    description: &str,
    counters: &str,
    status: Option<(&str, Color)>,
    width: u16,
) -> Vec<Line<'static>> {
    let budget = (width as usize)
        .saturating_sub(cols(AGENT_TREE_INDENT) + cols(AGENT_TREE_MID))
        .max(1);
    let connector = if is_last {
        AGENT_TREE_LAST
    } else {
        AGENT_TREE_MID
    };
    let dim = Style::new().fg(tool_dim_color());
    let mut description = description.to_string();
    let counters_cols = cols(counters);
    if cols(&description) + counters_cols > budget {
        description = truncate_cols(&description, budget.saturating_sub(counters_cols + 1));
        description.push('…');
    }
    let mut lines = vec![Line::from(vec![
        Span::styled(AGENT_TREE_INDENT.to_string(), dim),
        Span::styled(connector.to_string(), dim),
        Span::styled(description, Style::new().fg(tool_output_color())),
        Span::styled(counters.to_string(), dim),
    ])];
    if let Some((status, color)) = status {
        let rail = if is_last {
            AGENT_TREE_BLANK
        } else {
            AGENT_TREE_PIPE
        };
        lines.push(agent_activity_row(
            &format!("{AGENT_TREE_INDENT}{rail}{AGENT_TREE_CORNER}"),
            status,
            color,
            width,
        ));
    }
    lines
}

/// The group cell's `● {header}` row: the coloured bullet, the white header
/// text, and a dim trailing hint. `blink` is the live tree's frame clock —
/// the running tool bullet's own blink ([`bullet_span`]) — and `None` for a
/// recorded group, which sits at rest.
fn agent_group_header(
    color: Color,
    blink: Option<Duration>,
    text: String,
    hint: &str,
) -> Line<'static> {
    Line::from(vec![
        bullet_span(color, blink),
        Span::styled(text, Style::new().fg(tool_name_color())),
        Span::styled(hint.to_string(), Style::new().fg(tool_dim_color())),
    ])
}

/// A **single** agent's `● Agent({description})` cell header — the tool-cell
/// look a lone launch keeps instead of the group tree (`docs/agent-tool.md`).
/// `blink` as in [`agent_group_header`].
fn agent_cell_header(color: Color, blink: Option<Duration>, description: &str) -> Line<'static> {
    Line::from(vec![
        bullet_span(color, blink),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(tool_name_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({description})"),
            Style::new().fg(tool_args_color()),
        ),
    ])
}

/// The `Done ({n} tool uses · {tokens} tokens · {elapsed})` settle clause
/// shared by the Ctrl+O cell footer and the single-agent committed cell. The
/// runtime humanizes past a minute (`6m 2s`, never a bare `362s`) — the
/// [`format_elapsed`] contract every runtime display shares.
fn agent_done_clause(tool_uses: usize, tokens: u64, secs: u64) -> String {
    format!(
        "Done ({tool_uses} tool use{} · {} tokens · {})",
        if tool_uses == 1 { "" } else { "s" },
        format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX)),
        format_elapsed(secs),
    )
}

/// The **live** cell of a lone agent — `● Agent({description})` over its
/// sticky `⎿ {activity}` line (`Initializing…` before any event, then the
/// newest call's `{Name}: {detail}` / `{Name}({args})`), instead of a
/// one-row tree (`docs/agent-tool.md`).
///
/// The activity row is the *tree row's* row: **dim and clipped**, whatever
/// the agent is doing. A running call used to break that shape here alone —
/// its white `Bash(…)` header wrapped over several rows above a `Running…`
/// line — which read as the
/// main turn's own running cell and grew the strip under a live counter.
fn single_live_agent_lines(
    app: &App,
    run: &crate::agents::AgentRun,
    background: bool,
    width: u16,
) -> Vec<Line<'static>> {
    // The same blinking grey bullet a running tool cell has — this *is* the
    // round's running cell (`docs/tool-pulse.md`).
    let mut lines = vec![agent_cell_header(
        tool_running_color(),
        Some(app.pulse()),
        &run.description,
    )];
    lines.push(agent_activity_row(
        TOOL_RESULT_PREFIX,
        &run.activity_shown(|name, args| shown_args(name, args, app.path_display())),
        agent_status_color(run.status),
        width,
    ));
    if !background
        && app
            .background_hint_elapsed()
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A **committed** agent group's tree cell (`docs/agent-tool.md`):
/// `● {n} background agents launched (↓ to manage · ctrl+o to expand)` over
/// description-only rows for a background launch, else `● {n} agents
/// finished (ctrl+o to expand)` over counter rows with a `⎿ Done` /
/// `⎿ Interrupted` / `⎿ Failed` status row per agent — green bullet when
/// every agent finished cleanly, red otherwise. A **lone** agent keeps the
/// tool-cell look instead: `● Agent({description})` over `⎿ Done ({n} tool
/// uses · {tokens} tokens · {elapsed})` and a dim `(ctrl+o to expand)` line
/// — or `⎿ Running in the background (↓ to manage · ctrl+o to expand)` for
/// a lone background launch.
#[must_use]
pub fn agent_group_lines(group: &crate::app::AgentGroup, width: u16) -> Vec<Line<'static>> {
    let color = if group.ok() {
        tool_ok_color()
    } else {
        tool_fail_color()
    };
    if let [entry] = group.agents.as_slice() {
        let dim = Style::new().fg(tool_dim_color());
        let mut lines = vec![agent_cell_header(color, None, &entry.description)];
        let (settle, settle_color) = if group.background {
            (AGENT_BACKGROUNDED.to_string(), tool_dim_color())
        } else {
            match entry.status {
                crate::agents::AgentStatus::Done => (
                    agent_done_clause(entry.tool_uses, entry.tokens, entry.secs),
                    tool_dim_color(),
                ),
                status if status.is_final() => (status.label().to_string(), tool_fail_color()),
                status => (status.label().to_string(), tool_dim_color()),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(settle, Style::new().fg(settle_color)),
        ]));
        if !group.background {
            lines.push(Line::from(vec![
                Span::raw(INDENT.to_string()),
                Span::styled(EXPAND_HINT.trim_start().to_string(), dim),
            ]));
        }
        return lines;
    }
    let mut lines = if group.background {
        vec![agent_group_header(
            color,
            None,
            format!(
                "{} background {} launched",
                group.agents.len(),
                if group.agents.len() == 1 {
                    "agent"
                } else {
                    "agents"
                }
            ),
            AGENT_MANAGE_HINT,
        )]
    } else {
        vec![agent_group_header(
            color,
            None,
            format!("{} finished", agent_count_phrase(group.agents.len())),
            EXPAND_HINT,
        )]
    };
    let count = group.agents.len();
    for (i, entry) in group.agents.iter().enumerate() {
        let is_last = i + 1 == count;
        if group.background {
            lines.extend(agent_tree_rows(
                is_last,
                &entry.description,
                "",
                None,
                width,
            ));
        } else {
            lines.extend(agent_tree_rows(
                is_last,
                &entry.description,
                &agent_counters_clause(entry.tool_uses, entry.tokens),
                Some((entry.status.label(), agent_status_color(entry.status))),
                width,
            ));
        }
    }
    lines
}

/// The **live** agent group's tree cell — the strip preview while the round's
/// agents run: a blinking-grey `● Running {n} agents… (ctrl+o to expand)`
/// header (the running tool cell's blink, `docs/tool-pulse.md`) over
/// live tree rows (counters ticking, the status row showing each agent's
/// current activity). Rendered from the roster entries the live group names;
/// an id already swept renders nothing (it settled long ago).
#[must_use]
pub fn live_agent_group_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(live) = app.agent_group() else {
        return Vec::new();
    };
    let runs: Vec<&crate::agents::AgentRun> =
        live.ids.iter().filter_map(|id| app.agent(id)).collect();
    if runs.is_empty() {
        return Vec::new();
    }
    // A lone agent keeps the tool-cell look — `● Agent({description})` over
    // its live state — instead of a one-row tree (docs/agent-tool.md).
    if let [run] = runs.as_slice() {
        return single_live_agent_lines(app, run, live.background, width);
    }
    let mut lines = vec![agent_group_header(
        tool_running_color(),
        Some(app.pulse()),
        format!("Running {}…", agent_count_phrase(runs.len())),
        EXPAND_HINT,
    )];
    let count = runs.len();
    for (i, run) in runs.iter().enumerate() {
        let activity = run.activity_shown(|name, args| shown_args(name, args, app.path_display()));
        lines.extend(agent_tree_rows(
            i + 1 == count,
            &run.description,
            &agent_counters_clause(run.tool_uses, run.tokens),
            Some((activity.as_str(), agent_status_color(run.status))),
            width,
        ));
    }
    // The whole group can be moved to the background with Ctrl+B — the
    // delayed discoverability hint, exactly like a running bash cell's
    // (docs/background.md). Live-only by construction.
    if !live.background
        && app
            .background_hint_elapsed()
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A background agent's completion notice cell: the coloured `●` — green for
/// a clean finish, red for a stop/failure — over the one-line headline
/// (`Agent "{description}" finished · 35s`). The final response the notice
/// carries is context-only, never rendered. See `docs/agent-tool.md`.
#[must_use]
pub fn agent_notice_lines(notice: &crate::app::AgentNotice, width: u16) -> Vec<Line<'static>> {
    let color = if notice.ok() {
        bg_notice_ok_color()
    } else {
        bg_notice_fail_color()
    };
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    wrap_text(&notice.headline(), content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(AI_BULLET.to_string(), bullet_style),
                    Span::raw(line),
                ])
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(line)])
            }
        })
        .collect()
}

/// What one Ctrl+O agent cell renders — bridged from either a **recorded**
/// [`crate::app::AgentGroupEntry`] or a **live** roster
/// [`crate::agents::AgentRun`], so the two views share one renderer.
pub(super) struct AgentCellView {
    pub(super) description: String,
    pub(super) status: crate::agents::AgentStatus,
    pub(super) background: bool,
    pub(super) prompt: String,
    pub(super) tool_headers: Vec<String>,
    /// The live activity row (`Running…`) — live cells only.
    activity: Option<String>,
    pub(super) result: String,
    pub(super) tool_uses: usize,
    pub(super) tokens: u64,
    pub(super) secs: u64,
}

impl AgentCellView {
    pub(super) fn of_entry(entry: &crate::app::AgentGroupEntry, background: bool) -> Self {
        Self {
            description: entry.description.clone(),
            status: entry.status,
            background,
            prompt: entry.prompt.clone(),
            tool_headers: entry.tool_headers.clone(),
            activity: None,
            result: entry.result.clone(),
            tool_uses: entry.tool_uses,
            tokens: entry.tokens,
            secs: entry.secs,
        }
    }

    pub(super) fn of_run(run: &crate::agents::AgentRun) -> Self {
        let mut tool_headers: Vec<String> = run
            .history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Tool(tool) => {
                    Some(crate::app::tool_header_text(&tool.name, &tool.args))
                }
                _ => None,
            })
            .collect();
        // The **running** call joins them — what the agent is doing right now
        // is part of what it has done. Its not-yet-started `⎿ Waiting…`
        // siblings do not: this cell lists headers with no status of their
        // own, so a queued call would read as one the agent ran
        // (`docs/parallel-tools.md`). Once one resolves it is on the history
        // walked above — an interrupt records each as `Interrupted by user`
        // (`docs/interrupt.md`). The main transcript can show them live
        // because `tool_full_lines` carries each call's status row.
        if let Some(running) = run
            .tool_queue
            .front()
            .filter(|tool| tool.status == ToolStatus::Running)
        {
            tool_headers.push(crate::app::tool_header_text(&running.name, &running.args));
        }
        Self {
            description: run.description.clone(),
            status: run.status,
            background: run.background,
            prompt: run.prompt.clone(),
            tool_headers,
            activity: (!run.status.is_final()).then(|| match run.status {
                crate::agents::AgentStatus::Pending => "Initializing…".to_string(),
                _ => "Running…".to_string(),
            }),
            result: run.result.clone().unwrap_or_default(),
            tool_uses: run.tool_uses,
            tokens: run.tokens,
            secs: run.runtime.as_secs(),
        }
    }
}

/// One agent's expanded Ctrl+O cell: the `● Agent({description})` header
/// (bullet coloured by status), the `⎿ Prompt:` block, the nested tool-call
/// headers it ran, the `⎿ Response:` block once a final response exists, and
/// the `⎿ Done ({n} tool uses · {tokens} tokens · {s}s)` /
/// `⎿ Interrupted` / `⎿ Failed` footer. See `docs/agent-tool.md`.
pub(super) fn agent_cell_lines(
    cell: &AgentCellView,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let bullet_color = match cell.status {
        crate::agents::AgentStatus::Done => tool_ok_color(),
        crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
            tool_fail_color()
        }
        _ => tool_running_color(),
    };
    let dim = Style::new().fg(tool_dim_color());
    let white = Style::new().fg(tool_output_color());
    let section = Style::new()
        .fg(agent_section_color())
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(bullet_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(tool_name_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({})", cell.description),
            Style::new().fg(tool_args_color()),
        ),
    ])];
    // ⎿  Prompt: over the indented prompt body.
    lines.push(Line::from(vec![
        Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
        Span::styled(AGENT_PROMPT_LABEL.to_string(), section),
    ]));
    let body_width = width.saturating_sub(cols(AGENT_BODY_INDENT) as u16).max(1);
    for row in wrap_text(&cell.prompt, body_width) {
        lines.push(Line::from(vec![
            Span::raw(AGENT_BODY_INDENT.to_string()),
            Span::styled(row, white),
        ]));
    }
    // The nested tool calls it ran (headers only — the agent session view has
    // the full cells), then the live activity row.
    if !cell.tool_headers.is_empty() || cell.activity.is_some() {
        lines.push(Line::default());
        let nested_width = width
            .saturating_sub(cols(AGENT_NESTED_INDENT) as u16)
            .max(1);
        for header in &cell.tool_headers {
            // A recorded `Name(args)` one-liner — a file tool's shows its
            // path by the session's rule (`docs/tools.md` *Path display*),
            // every other as recorded; `app::file_tool_header` is the
            // formatter's own inverse, so the grammar lives in one place.
            let header = match crate::app::file_tool_header(header) {
                Some((name, path)) => {
                    crate::app::tool_header_text(name, &shown_args(name, path, paths))
                }
                None => header.clone(),
            };
            for (i, row) in wrap_text(&header, nested_width).into_iter().enumerate() {
                let indent = if i == 0 {
                    AGENT_NESTED_INDENT.to_string()
                } else {
                    format!("{AGENT_NESTED_INDENT}  ")
                };
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(row, white),
                ]));
            }
        }
        if let Some(activity) = &cell.activity {
            lines.push(Line::from(vec![
                Span::raw(AGENT_NESTED_INDENT.to_string()),
                Span::styled(activity.clone(), dim),
            ]));
        }
    }
    // ⎿  Response: once a final response exists.
    if !cell.result.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(AGENT_RESPONSE_LABEL.to_string(), section),
        ]));
        for row in wrap_text(&cell.result, body_width) {
            lines.push(Line::from(vec![
                Span::raw(AGENT_BODY_INDENT.to_string()),
                Span::styled(row, white),
            ]));
        }
    }
    // The settle footer.
    let footer: Option<(String, Color)> = match cell.status {
        crate::agents::AgentStatus::Done => Some((
            agent_done_clause(cell.tool_uses, cell.tokens, cell.secs),
            tool_dim_color(),
        )),
        crate::agents::AgentStatus::Interrupted => {
            Some(("Interrupted".to_string(), tool_fail_color()))
        }
        crate::agents::AgentStatus::Failed => Some(("Failed".to_string(), tool_fail_color())),
        _ if cell.background => Some((TOOL_BACKGROUNDED.to_string(), tool_dim_color())),
        _ => None,
    };
    if let Some((text, color)) = footer {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(text, Style::new().fg(color)),
        ]));
    }
    lines
}

/// A committed [`crate::app::AgentGroup`]'s Ctrl+O expansion: one
/// [`agent_cell_lines`] cell per entry, blank-separated.
pub(super) fn agent_group_full_lines(
    group: &crate::app::AgentGroup,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, entry) in group.agents.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(agent_cell_lines(
            &AgentCellView::of_entry(entry, group.background),
            width,
            paths,
        ));
    }
    lines
}

/// How many rows the footer's agent roster occupies: 0 with no visible
/// agents, else a blank spacer + the `● main` row + one row per agent.
/// [`live_height`] adds this below the footer; [`render_live`] paints exactly
/// these rows.
#[must_use]
pub fn agent_list_rows(app: &App) -> u16 {
    let agents = app.visible_agents().len();
    if agents == 0 {
        return 0;
    }
    u16::try_from(2 + agents).unwrap_or(u16::MAX)
}

/// The footer roster: a blank spacer, the main row, then one
/// `{type}  {description} {elapsed} · ↓ {tokens} tokens` row per visible
/// agent. The **filled `●` bullet + bright bold text mark the session in
/// view** — `● main` normally, or the viewed agent's row inside its session
/// view (main demoting to the dim `◯` ring) — while the `❯` marker belongs
/// to the active ↑/↓ selection alone and leaves with it when the focus
/// returns to the composer. Finished agents' bullets colour green/red for
/// their linger. See `docs/agent-tool.md`.
#[must_use]
pub fn agent_list_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let agents = app.visible_agents();
    if agents.is_empty() {
        return Vec::new();
    }
    let dim = Style::new().fg(tool_dim_color());
    let selection = app.agent_selection();
    let mut lines = vec![Line::default()];
    // The main row: the filled bullet only while the MAIN session is the one
    // in view — an open agent view moves the highlight to that agent's row.
    let main_selected = selection == Some(0);
    let main_viewed = app.agent_view.is_none();
    let marker = if main_selected {
        AGENT_LIST_MARKER
    } else {
        AGENT_LIST_INDENT
    };
    let bullet = if main_viewed {
        AGENT_MAIN_BULLET
    } else {
        AGENT_ROW_BULLET
    };
    let mut main_style = if main_selected {
        Style::new().fg(menu_selected_color())
    } else if main_viewed {
        Style::new().fg(tool_output_color())
    } else {
        dim
    };
    if main_viewed {
        main_style = main_style.add_modifier(Modifier::BOLD);
    }
    lines.push(Line::from(vec![
        Span::styled(marker.to_string(), Style::new().fg(menu_selected_color())),
        Span::styled(bullet.to_string(), main_style),
        Span::styled(AGENT_MAIN_LABEL.to_string(), main_style),
    ]));
    for (i, run) in agents.iter().enumerate() {
        let selected = selection == Some(i + 1);
        let viewed = app.agent_view.as_deref() == Some(run.id.as_str());
        // The `❯` marks the explicit ↑/↓ selection only — once Enter/Esc
        // hand the keys back to the composer it leaves; the viewed session
        // is marked by its filled bullet + bright row instead.
        let marker = if selected {
            AGENT_LIST_MARKER
        } else {
            AGENT_LIST_INDENT
        };
        let bullet = if viewed {
            AGENT_MAIN_BULLET
        } else {
            AGENT_ROW_BULLET
        };
        let bullet_style = match run.status {
            s if s.is_final() && s.ok() => Style::new().fg(tool_ok_color()),
            s if s.is_final() => Style::new().fg(tool_fail_color()),
            _ if selected => Style::new().fg(menu_selected_color()),
            _ if viewed => Style::new()
                .fg(tool_output_color())
                .add_modifier(Modifier::BOLD),
            _ => dim,
        };
        let mut text_style = if selected {
            Style::new().fg(menu_selected_color())
        } else if viewed {
            Style::new().fg(tool_output_color())
        } else {
            dim
        };
        if viewed {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        // `{type}  {description}` truncated so the ` {elapsed} · ↓ {n} tokens`
        // suffix always fits.
        let mut suffix = format!(" {}", format_elapsed(run.runtime.as_secs()));
        if run.tokens > 0 {
            suffix.push_str(&format!(
                " · {} {} tokens",
                STATUS_ARROW_DOWN,
                format_token_count(usize::try_from(run.tokens).unwrap_or(usize::MAX))
            ));
        }
        let lead = format!("{marker}{bullet}");
        let budget = (width as usize)
            .saturating_sub(cols(&lead) + cols(&suffix))
            .max(1);
        let mut name = format!("{}  {}", run.agent_type, run.description);
        if cols(&name) > budget {
            name = truncate_cols(&name, budget.saturating_sub(1));
            name.push('…');
        }
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::new().fg(menu_selected_color())),
            Span::styled(bullet.to_string(), bullet_style),
            Span::styled(name, text_style),
            Span::styled(suffix, dim),
        ]));
    }
    lines
}

/// The roster selection's footer hint line — `↑/↓ to select · Enter to view`
/// on the `● main` row, `Enter to view · x to stop` on a running agent, and
/// `x to clear` once that agent has settled (a stop leaves the red row on the
/// roster; the same key is what takes it off) — taking the footer's slot
/// while the selection is active (the shell-mode-line pattern).
#[must_use]
pub fn agent_hint_line(app: &App) -> Line<'static> {
    let entries = match app.agent_selection() {
        None | Some(0) => AGENT_HINT_MAIN,
        Some(row) => {
            let settled = app
                .visible_agents()
                .get(row - 1)
                .is_some_and(|run| run.status.is_final());
            if settled {
                AGENT_HINT_AGENT_DONE
            } else {
                AGENT_HINT_AGENT
            }
        }
    };
    let mut spans = vec![Span::raw(FOOTER_INDENT)];
    for (i, (key, label)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                FOOTER_SEPARATOR.to_string(),
                Style::new().fg(footer_color()),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(shortcuts_key_color()),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(footer_color()),
        ));
    }
    Line::from(spans)
}

/// The synthesized status for an **agent session view**'s strip — the viewed
/// agent's own spinner line (`Working… (elapsed · ↓ tokens · esc to
/// interrupt)` shape, without the interrupt hint's meaning changing: Esc
/// leaves the view). Built per draw from the roster entry, the agent's open
/// thinking phase included (`Thinking for Ns`, boundary-injected like the
/// runtime — `docs/agent-view-streaming.md`), its verb the one the agent
/// walked to on its own clock
/// ([`AgentRun::status_verb`](crate::agents::AgentRun::status_verb)) — the
/// main line's rotation, whose past tense the agent's turn summary records
/// (`docs/status-indicator.md`).
#[must_use]
pub fn agent_view_status(run: &crate::agents::AgentRun) -> crate::app::TurnStatus {
    // The verb the run walked to on its own clock (`AgentRun::rotate_verb`);
    // this per-draw copy is never ticked, so it carries no rotation of its own.
    let verb = run.status_verb();
    crate::app::TurnStatus {
        verb: verb.working,
        done_verb: verb.done,
        rotates_from: None,
        tokens: usize::try_from(run.tokens).unwrap_or(usize::MAX),
        arrow: crate::app::TokenArrow::Down,
        elapsed: run.runtime,
        thinking: run.thinking,
        shell: false,
        retry: run.retry,
        // A subagent's line hangs no tip (docs/tips.md).
        tip: None,
    }
}

/// The agent session view's strip preview — the main strip's branches over
/// the viewed agent's own state, in the same order
/// ([`super::live::preview_lines`]): its live tool cells (the batch queue,
/// blank-separated, a running command tailing its output), else its open
/// thinking block, else its streaming reply's frontier. Empty when idle.
///
/// `stream_preview` is the boundary's [`super::StreamRender::preview`] over
/// the agent's buffer — **the rows its own commits have withheld**, which for
/// a forming table or a fenced code line is the whole block, not one row. The
/// fallback below (`None`: a unit test, or any caller without a render)
/// re-renders the last line from the buffer, exactly as the main branch's
/// does. Rendering that fallback while the commits withheld a whole block was
/// the reported "streaming disappears" bug — `committed ++ preview` must be
/// the reply here as much as in the main view (CLAUDE.md invariant 2,
/// `docs/agent-view-streaming.md`).
///
/// The [`preview_lines`]/[`preview_rows`] pair calls this for a viewed agent
/// so the two agree.
pub(super) fn agent_view_preview_lines(
    run: &crate::agents::AgentRun,
    pulse: Duration,
    width: u16,
    stream_preview: Option<&[Line<'static>]>,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    if !run.tool_queue.is_empty() {
        let mut lines = Vec::new();
        for (i, tool) in run.tool_queue.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            // A live strip like the main one — the agent's running call
            // blinks here too (`docs/tool-pulse.md`) and a running command
            // tails its streamed output under the `(Ns · wait …)` clock
            // row, on its `⎿ Running…` row before any (`docs/tool-streaming.md`). The
            // elapsed is the **command's** own — boundary-injected per agent
            // like the thinking phase's, `AgentRun::command_elapsed` — never
            // the agent's whole `runtime`, which its status line shows: a
            // call started a minute in used to open on `+N lines (60s)`.
            lines.extend(super::live::live_call_lines(
                tool,
                run.command_elapsed.unwrap_or(Duration::ZERO),
                pulse,
                width,
                paths,
            ));
        }
        return lines;
    }
    // An open thinking phase previews its live block, after the tool branch
    // and before the reply's — what is genuinely executing is what the user
    // waits on, and the model cannot be streaming a reply while it thinks
    // (the main strip's order, `docs/thinking-stream.md`).
    if let Some(text) = run.reasoning() {
        return super::reasoning::live_reasoning_lines(text, pulse, width);
    }
    if let Some(lines) = stream_preview {
        return lines.to_vec();
    }
    run.streaming
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|text| {
            message_lines(Role::Assistant, text, width)
                .pop()
                .unwrap_or_default()
        })
        .into_iter()
        .collect()
}

/// The strip's preview row count for a viewed agent — [`preview_rows`]' agent
/// branch, mirroring its main one: a live tool queue or an open thinking
/// phase size from the same walk the strip draws, while a **streaming reply**
/// reports the boundary-injected frontier height
/// ([`crate::app::App::stream_preview_rows`]), because only the boundary's
/// `StreamRender` knows how many rows it withheld
/// (`docs/agent-view-streaming.md`, `docs/table-streaming.md`).
pub(super) fn agent_preview_rows(app: &App, run: &crate::agents::AgentRun, width: u16) -> u16 {
    if run.tool_queue.is_empty()
        && run.reasoning().is_none()
        && run.streaming.as_deref().is_some_and(|t| !t.is_empty())
    {
        return app.stream_preview_rows();
    }
    u16::try_from(agent_view_preview_lines(run, app.pulse(), width, None, app.path_display()).len())
        .unwrap_or(u16::MAX)
}

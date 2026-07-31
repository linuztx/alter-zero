//! Subagent rendering: the live `● Running {n} agents…` tree, the recorded
//! group cells, the footer roster, and the inline session view.
//! See `docs/agent-tool.md`.

use super::theme::*;
use super::tool::{live_tool_lines, result_row, tool_pulse_color};
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
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut description = description.to_string();
    let counters_cols = cols(counters);
    if cols(&description) + counters_cols > budget {
        description = truncate_cols(&description, budget.saturating_sub(counters_cols + 1));
        description.push('…');
    }
    let mut lines = vec![Line::from(vec![
        Span::styled(AGENT_TREE_INDENT.to_string(), dim),
        Span::styled(connector.to_string(), dim),
        Span::styled(description, Style::new().fg(TOOL_OUTPUT_COLOR)),
        Span::styled(counters.to_string(), dim),
    ])];
    if let Some((status, color)) = status {
        let rail = if is_last {
            AGENT_TREE_BLANK
        } else {
            AGENT_TREE_PIPE
        };
        lines.push(Line::from(vec![
            Span::styled(AGENT_TREE_INDENT.to_string(), dim),
            Span::styled(rail.to_string(), dim),
            Span::styled(AGENT_TREE_CORNER.to_string(), dim),
            Span::styled(
                truncate_cols(status, budget.saturating_sub(cols(AGENT_TREE_CORNER))),
                Style::new().fg(color),
            ),
        ]));
    }
    lines
}

/// The group cell's `● {header}` row: the coloured bullet, the white header
/// text, and a dim trailing hint.
fn agent_group_header(color: Color, text: String, hint: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(text, Style::new().fg(TOOL_NAME_COLOR)),
        Span::styled(hint.to_string(), Style::new().fg(TOOL_DIM_COLOR)),
    ])
}

/// A **single** agent's `● Agent({description})` cell header — the tool-cell
/// look a lone launch keeps instead of the group tree (`docs/agent-tool.md`).
fn agent_cell_header(color: Color, description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({description})"), Style::new().fg(TOOL_ARGS_COLOR)),
    ])
}

/// The `Done ({n} tool uses · {tokens} tokens · {s}s)` settle clause shared by
/// the Ctrl+O cell footer and the single-agent committed cell.
fn agent_done_clause(tool_uses: usize, tokens: u64, secs: u64) -> String {
    format!(
        "Done ({tool_uses} tool use{} · {} tokens · {secs}s)",
        if tool_uses == 1 { "" } else { "s" },
        format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX)),
    )
}

/// A running tool's header **inside a `⎿` corner** — the single-agent live
/// cell's `⎿  Bash(sleep 10 && curl -s "…` shape: the corner row leads,
/// continuations char-wrap aligned under the opening `(`, capped at
/// [`TOOL_HEADER_MAX_ROWS`] rows with a fitted `…)`.
fn corner_tool_header_lines(name: &str, args: &str, width: u16) -> Vec<Line<'static>> {
    let corner_cols = cols(TOOL_RESULT_PREFIX);
    let indent = " ".repeat(corner_cols + cols(name) + 1); // under the `(`
    let text = format!("{name}({args})");
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let white = Style::new().fg(TOOL_ARGS_COLOR);
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut budget = (width as usize).saturating_sub(corner_cols).max(1);
    for ch in text.chars() {
        let w = cols(&ch.to_string());
        if cols(&current) + w > budget {
            rows.push(std::mem::take(&mut current));
            budget = (width as usize).saturating_sub(cols(&indent)).max(1);
        }
        current.push(ch);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.len() > TOOL_HEADER_MAX_ROWS {
        rows.truncate(TOOL_HEADER_MAX_ROWS);
        if let Some(last) = rows.last_mut() {
            *last = truncate_cols(
                last,
                (width as usize)
                    .saturating_sub(cols(&indent) + cols(TOOL_HEADER_ELLIPSIS) + 1)
                    .max(1),
            );
            last.push_str(TOOL_HEADER_ELLIPSIS);
            last.push(')');
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
                    Span::styled(row, white),
                ])
            } else {
                Line::from(vec![Span::raw(indent.clone()), Span::styled(row, white)])
            }
        })
        .collect()
}

/// The **live** cell of a lone agent — `● Agent({description})` over its
/// current state instead of a one-row tree (`docs/agent-tool.md`): the
/// running tool's wrapped header + a dim `Running…`, or the sticky
/// `⎿ {activity}` line (`Initializing…` before any event, the last
/// `{Name}: {detail}` between calls).
fn single_live_agent_lines(
    app: &App,
    run: &crate::agents::AgentRun,
    background: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    // The same breathing grey a running tool cell has — this *is* the round's
    // running cell (`docs/tool-pulse.md`).
    let mut lines = vec![agent_cell_header(
        tool_pulse_color(app.pulse()),
        &run.description,
    )];
    let running_tool = run
        .tool_queue
        .front()
        .filter(|tool| tool.status == ToolStatus::Running);
    if let Some(tool) = running_tool {
        lines.extend(corner_tool_header_lines(&tool.name, &tool.args, width));
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(cols(TOOL_RESULT_PREFIX))),
            Span::styled(TOOL_RUNNING.to_string(), dim),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(
                truncate_cols(
                    &run.activity(),
                    (width as usize)
                        .saturating_sub(cols(TOOL_RESULT_PREFIX))
                        .max(1),
                ),
                dim,
            ),
        ]));
    }
    if !background
        && app
            .command_elapsed()
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A **committed** agent group's tree cell (`docs/agent-tool.md`):
/// `● {n} background agents launched (↓ to manage)` over description-only
/// rows for a background launch, else `● {n} agents finished (ctrl+o to
/// expand)` over counter rows with a `⎿ Done` / `⎿ Interrupted` / `⎿ Failed`
/// status row per agent — green bullet when every agent finished cleanly,
/// red otherwise. A **lone** agent keeps the tool-cell look instead:
/// `● Agent({description})` over `⎿ Done ({n} tool uses · {tokens} tokens ·
/// {s}s)` and a dim `(ctrl+o to expand)` line — or
/// `⎿ Running in the background (↓ to manage)` for a lone background launch.
#[must_use]
pub fn agent_group_lines(group: &crate::app::AgentGroup, width: u16) -> Vec<Line<'static>> {
    let color = if group.ok() {
        TOOL_OK_COLOR
    } else {
        TOOL_FAIL_COLOR
    };
    if let [entry] = group.agents.as_slice() {
        let dim = Style::new().fg(TOOL_DIM_COLOR);
        let mut lines = vec![agent_cell_header(color, &entry.description)];
        let (settle, settle_color) = if group.background {
            (TOOL_BACKGROUNDED.to_string(), TOOL_DIM_COLOR)
        } else {
            match entry.status {
                crate::agents::AgentStatus::Done => (
                    agent_done_clause(entry.tool_uses, entry.tokens, entry.secs),
                    TOOL_DIM_COLOR,
                ),
                status if status.is_final() => (status.label().to_string(), TOOL_FAIL_COLOR),
                status => (status.label().to_string(), TOOL_DIM_COLOR),
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
            let status_color = match entry.status {
                s if s.ok() => TOOL_DIM_COLOR,
                crate::agents::AgentStatus::Running | crate::agents::AgentStatus::Pending => {
                    TOOL_DIM_COLOR
                }
                _ => TOOL_FAIL_COLOR,
            };
            lines.extend(agent_tree_rows(
                is_last,
                &entry.description,
                &agent_counters_clause(entry.tool_uses, entry.tokens),
                Some((entry.status.label(), status_color)),
                width,
            ));
        }
    }
    lines
}

/// The **live** agent group's tree cell — the strip preview while the round's
/// agents run: a breathing-grey `● Running {n} agents… (ctrl+o to expand)`
/// header (the running tool cell's pulse, `docs/tool-pulse.md`) over
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
        tool_pulse_color(app.pulse()),
        format!("Running {}…", agent_count_phrase(runs.len())),
        EXPAND_HINT,
    )];
    let count = runs.len();
    for (i, run) in runs.iter().enumerate() {
        let activity = run.activity();
        let status_color = match run.status {
            crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
                TOOL_FAIL_COLOR
            }
            _ => TOOL_DIM_COLOR,
        };
        lines.extend(agent_tree_rows(
            i + 1 == count,
            &run.description,
            &agent_counters_clause(run.tool_uses, run.tokens),
            Some((activity.as_str(), status_color)),
            width,
        ));
    }
    // The whole group can be moved to the background with Ctrl+B — the
    // delayed discoverability hint, exactly like a running bash cell's
    // (docs/background.md). Live-only by construction.
    if !live.background
        && app
            .command_elapsed()
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
        BG_NOTICE_OK_COLOR
    } else {
        BG_NOTICE_FAIL_COLOR
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
    fn of_entry(entry: &crate::app::AgentGroupEntry, background: bool) -> Self {
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
                HistoryItem::Tool(tool) => Some(format!("{}({})", tool.name, tool.args)),
                _ => None,
            })
            .collect();
        for tool in &run.tool_queue {
            tool_headers.push(format!("{}({})", tool.name, tool.args));
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
pub(super) fn agent_cell_lines(cell: &AgentCellView, width: u16) -> Vec<Line<'static>> {
    let bullet_color = match cell.status {
        crate::agents::AgentStatus::Done => TOOL_OK_COLOR,
        crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
            TOOL_FAIL_COLOR
        }
        _ => TOOL_RUNNING_COLOR,
    };
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let white = Style::new().fg(TOOL_OUTPUT_COLOR);
    let section = Style::new()
        .fg(AGENT_SECTION_COLOR)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(bullet_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({})", cell.description),
            Style::new().fg(TOOL_ARGS_COLOR),
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
            for (i, row) in wrap_text(header, nested_width).into_iter().enumerate() {
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
            TOOL_DIM_COLOR,
        )),
        crate::agents::AgentStatus::Interrupted => {
            Some(("Interrupted".to_string(), TOOL_FAIL_COLOR))
        }
        crate::agents::AgentStatus::Failed => Some(("Failed".to_string(), TOOL_FAIL_COLOR)),
        _ if cell.background => Some((TOOL_BACKGROUNDED.to_string(), TOOL_DIM_COLOR)),
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
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, entry) in group.agents.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(agent_cell_lines(
            &AgentCellView::of_entry(entry, group.background),
            width,
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

/// The footer roster: a blank spacer, the `● main` row, then one
/// `◯ {type}  {description} {elapsed} · ↓ {tokens} tokens` row per visible
/// agent — the `❯` selection marker on the active row, the viewed session
/// bold, finished agents' `◯` coloured green/red for their linger. See
/// `docs/agent-tool.md`.
#[must_use]
pub fn agent_list_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let agents = app.visible_agents();
    if agents.is_empty() {
        return Vec::new();
    }
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let selection = app.agent_selection();
    let mut lines = vec![Line::default()];
    // The `● main` row.
    let main_selected = selection == Some(0);
    let main_viewed = app.agent_view.is_none();
    let marker = if main_selected {
        AGENT_LIST_MARKER
    } else {
        AGENT_LIST_INDENT
    };
    let mut main_style = if main_selected {
        Style::new().fg(MENU_SELECTED_COLOR)
    } else if main_viewed {
        Style::new().fg(TOOL_OUTPUT_COLOR)
    } else {
        dim
    };
    if main_viewed {
        main_style = main_style.add_modifier(Modifier::BOLD);
    }
    lines.push(Line::from(vec![
        Span::styled(marker.to_string(), Style::new().fg(MENU_SELECTED_COLOR)),
        Span::styled(AGENT_MAIN_BULLET.to_string(), main_style),
        Span::styled(AGENT_MAIN_LABEL.to_string(), main_style),
    ]));
    for (i, run) in agents.iter().enumerate() {
        let selected = selection == Some(i + 1);
        let viewed = app.agent_view.as_deref() == Some(run.id.as_str());
        // The `❯` marks the explicit selection — or, with none active, the
        // agent whose session view is open (the user's reference look).
        let marker = if selected || (selection.is_none() && viewed) {
            AGENT_LIST_MARKER
        } else {
            AGENT_LIST_INDENT
        };
        let bullet_style = match run.status {
            s if s.is_final() && s.ok() => Style::new().fg(TOOL_OK_COLOR),
            s if s.is_final() => Style::new().fg(TOOL_FAIL_COLOR),
            _ if selected => Style::new().fg(MENU_SELECTED_COLOR),
            _ => dim,
        };
        let mut text_style = if selected {
            Style::new().fg(MENU_SELECTED_COLOR)
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
        let lead = format!("{}{}", marker, AGENT_ROW_BULLET);
        let budget = (width as usize)
            .saturating_sub(cols(&lead) + cols(&suffix))
            .max(1);
        let mut name = format!("{}  {}", run.agent_type, run.description);
        if cols(&name) > budget {
            name = truncate_cols(&name, budget.saturating_sub(1));
            name.push('…');
        }
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::new().fg(MENU_SELECTED_COLOR)),
            Span::styled(AGENT_ROW_BULLET.to_string(), bullet_style),
            Span::styled(name, text_style),
            Span::styled(suffix, dim),
        ]));
    }
    lines
}

/// The roster selection's footer hint line — `↑/↓ to select · Enter to view`
/// on the `● main` row, `Enter to view · x to stop` on an agent row — taking
/// the footer's slot while the selection is active (the shell-mode-line
/// pattern).
#[must_use]
pub fn agent_hint_line(app: &App) -> Line<'static> {
    let entries = if app.agent_selection() == Some(0) {
        AGENT_HINT_MAIN
    } else {
        AGENT_HINT_AGENT
    };
    let mut spans = vec![Span::raw(FOOTER_INDENT)];
    for (i, (key, label)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                FOOTER_SEPARATOR.to_string(),
                Style::new().fg(FOOTER_COLOR),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(SHORTCUTS_KEY_COLOR),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(FOOTER_COLOR),
        ));
    }
    Line::from(spans)
}

/// The synthesized status for an **agent session view**'s strip — the viewed
/// agent's own spinner line (`Working… (elapsed · ↓ tokens · esc to
/// interrupt)` shape, without the interrupt hint's meaning changing: Esc
/// leaves the view). Built per draw from the roster entry.
#[must_use]
pub fn agent_view_status(run: &crate::agents::AgentRun) -> crate::app::TurnStatus {
    crate::app::TurnStatus {
        verb: "Working",
        done_verb: "Done",
        tokens: usize::try_from(run.tokens).unwrap_or(usize::MAX),
        arrow: crate::app::TokenArrow::Down,
        elapsed: run.runtime,
        thinking: None,
        shell: false,
        retry: None,
    }
}

/// The agent session view's strip preview: the viewed agent's live tool
/// cells (the batch queue, blank-separated) or its streaming reply's last
/// row. Empty when idle. The [`preview_lines`]/[`preview_rows`] pair calls
/// this for a viewed agent so the two agree.
pub(super) fn agent_view_preview_lines(
    run: &crate::agents::AgentRun,
    pulse: Duration,
    width: u16,
) -> Vec<Line<'static>> {
    if !run.tool_queue.is_empty() {
        let mut lines = Vec::new();
        for (i, tool) in run.tool_queue.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            // A live strip like the main one — the agent's running call
            // breathes here too (`docs/tool-pulse.md`).
            lines.extend(live_tool_lines(tool, width, pulse));
        }
        return lines;
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

//! The Ctrl+D context-debug overlay: the raw LLM context window a real
//! backend sends each turn. See `docs/context.md`.

use super::assistant::expand_code_tabs;
use super::theme::*;
use super::transcript::{overlay_header, tool_view_separator};
use super::wrap::{cols, wrap_verbatim};
use super::*;

/// The role-tag colour of one context entry (see the `CONTEXT_*` consts).
const fn context_role_color(role: crate::context::ContextRole) -> Color {
    match role {
        crate::context::ContextRole::User => CONTEXT_USER_COLOR,
        crate::context::ContextRole::Assistant => CONTEXT_ASSISTANT_COLOR,
        crate::context::ContextRole::System => CONTEXT_SYSTEM_COLOR,
        crate::context::ContextRole::Tool => CONTEXT_TOOL_COLOR,
    }
}

/// One context entry's rows: the coloured `role:` tag, the raw text wrapped
/// **verbatim** (never the markdown renderer — the whole point is showing the
/// unformatted wire content), any native tool calls as `→ name(arguments)`
/// rows, any attachment paths dim beneath, and a blank spacer. An empty text
/// (an assistant entry that only called tools) contributes no text row.
fn context_entry_lines(
    lines: &mut Vec<Line<'static>>,
    tag: &str,
    color: Color,
    text: &str,
    tool_calls: &[crate::context::ContextToolCall],
    images: &[std::path::PathBuf],
    width: u16,
) {
    lines.push(Line::from(Span::styled(
        tag.to_string(),
        Style::new().fg(color),
    )));
    let text_width = width.saturating_sub(cols(CONTEXT_INDENT) as u16);
    if !text.is_empty() {
        // Tool results ride this path too — expand their tabs for display
        // (they paint as zero cells otherwise; see `tool_output_lines`).
        for row in wrap_verbatim(&expand_code_tabs(text), text_width) {
            lines.push(Line::from(format!("{CONTEXT_INDENT}{row}")));
        }
    }
    for call in tool_calls {
        // The native tool request in its raw wire form: `→ name(arguments)`.
        let rendered = format!(
            "{CONTEXT_TOOL_CALL_PREFIX}{}({})",
            call.name, call.arguments
        );
        for row in wrap_verbatim(&rendered, text_width) {
            lines.push(Line::from(Span::styled(
                format!("{CONTEXT_INDENT}{row}"),
                Style::new().fg(CONTEXT_TOOL_COLOR),
            )));
        }
    }
    for path in images {
        // Wrapped like the text — a long temp path must not clip off-screen.
        let label = format!("{CONTEXT_IMAGE_LABEL}{}", path.display());
        for row in wrap_verbatim(&label, text_width) {
            lines.push(Line::from(Span::styled(
                format!("{CONTEXT_INDENT}{row}"),
                Style::new().fg(TOOL_DIM_COLOR),
            )));
        }
    }
    lines.push(Line::default());
}

/// The Ctrl+D body: the raw context window, oldest first — the system prompt
/// (when the backend sends one), then every message
/// [`crate::context::context_messages`] derives from the history. What you
/// read here is what [`crate::llm::backend::build_messages`] sends (the
/// attachments as their paths rather than encoded bytes). A dim placeholder
/// when there is nothing yet. See `docs/context.md`.
#[must_use]
pub fn context_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    // An agent session view debugs the *viewed agent's* context: its own
    // transcript derived through the same mapping, under the prompt a
    // subagent is actually sent — the main prompt with the subagent note
    // appended (`App::agent_system_prompt`, from
    // `ReplySource::agent_system_prompt`) — with no AGENTS.md fragment
    // (subagents get none). See `docs/agent-tool.md`.
    // A viewed subagent's own window has neither leading fragment: its
    // instructions ride the system prompt shown above, and its skills — which
    // it does carry (`docs/skills.md`) — are the session's, listed on the
    // main view.
    let (history, instructions, skills, system_prompt) = match app.viewed_agent() {
        Some(run) => (
            run.history.as_slice(),
            None,
            None,
            app.agent_system_prompt.as_ref(),
        ),
        None => (
            app.history.as_slice(),
            app.user_instructions.as_deref(),
            app.skill_listing.as_deref(),
            app.system_prompt.as_ref(),
        ),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(prompt) = system_prompt {
        context_entry_lines(
            &mut lines,
            CONTEXT_SYSTEM_PROMPT_TAG,
            CONTEXT_SYSTEM_COLOR,
            prompt,
            &[],
            &[],
            width,
        );
    }
    for message in crate::context::context_messages_full(instructions, skills, history) {
        context_entry_lines(
            &mut lines,
            &format!("{}:", message.role.wire_name()),
            context_role_color(message.role),
            &message.text,
            &message.tool_calls,
            &message.images,
            width,
        );
    }
    if lines.is_empty() {
        return vec![Line::from(Span::styled(
            CONTEXT_VIEW_EMPTY,
            Style::new().fg(TOOL_DIM_COLOR),
        ))];
    }
    lines
}

/// The largest scroll offset the context view can take on this screen —
/// [`tool_view_max_scroll`]'s sibling (the chrome rows are shared).
#[must_use]
pub fn context_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize;
    context_lines(app, width).len().saturating_sub(body)
}

/// Render the full-screen Ctrl+D context-debug view — the transcript pager's
/// chrome over the raw context window, windowed by `App::debug_scroll`
/// (clamped) with `~` filler past the end. Pure — `term.rs` paints this onto
/// the overlay. See `docs/context.md`.
pub fn render_context_view(area: Rect, buf: &mut Buffer, app: &App) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(overlay_header(CONTEXT_VIEW_TITLE, area.width)).render(title_area, buf);

    let lines = context_lines(app, body_area.width);
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.debug_scroll.min(max);
    let mut visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    while (visible.len() as u16) < body_area.height {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(tool_view_separator(area.width, scroll, max)).render(sep_area, buf);

    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(CONTEXT_VIEW_HINT_QUIT.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

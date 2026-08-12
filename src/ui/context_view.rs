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

/// Caches the Ctrl+D view's built window so redrawing it costs O(viewport).
///
/// The derived-context walk + verbatim wrap above is O(conversation), and the
/// view redraws on every animation frame while a turn runs (the 32 ms status
/// re-arm) — rebuilt per frame it pegged the loop on a big context (a long
/// skill's rendered body in the window) and starved the scroll keys: the
/// reported "Ctrl+D freezes" bug. [`TranscriptCache`]'s little sibling: owned
/// by the event loop, consulted once per draw, the lines rebuilt only when
/// the signature below changes — so a scroll key or a status tick is a cache
/// hit. It needs no incremental prefix: the window only changes at history
/// boundaries (an item lands, a rewind, a turn start's fragment refresh),
/// never per streamed chunk. See `docs/context.md`.
///
/// [`TranscriptCache`]: super::TranscriptCache
#[derive(Default)]
pub struct ContextCache {
    sig: Option<ContextSig>,
    lines: Vec<Line<'static>>,
    /// Test-only: how many accesses did a rebuild, so a test can prove an
    /// unchanged conversation is a cache hit, not a rebuild.
    #[cfg(test)]
    pub(super) builds: usize,
}

/// What pins a cached window. History is append-only between generation bumps
/// (the transcript cache's premise), so `(generation, len)` pins the
/// conversation half; the leading fragments are pinned by their lengths —
/// cheap, and they otherwise only change beside a turn-start history append
/// (the direct edits — a `/settings` toggle dropping the instructions, a
/// `/model` switch swapping the prompt — all move the length). An open agent
/// session view swaps the whole window to that agent's derivation
/// ([`context_lines`]'s first branch), so the viewed agent — and how far its
/// own transcript has grown — keys too.
#[derive(PartialEq, Eq)]
struct ContextSig {
    generation: u64,
    history_len: usize,
    width: u16,
    agent: Option<(String, usize)>,
    prompt_len: Option<usize>,
    agent_prompt_len: Option<usize>,
    instructions_len: Option<usize>,
    skills_len: Option<usize>,
}

impl ContextSig {
    fn of(app: &App, width: u16) -> Self {
        Self {
            generation: app.history_generation(),
            history_len: app.history.len(),
            width,
            agent: app
                .agent_view
                .clone()
                .map(|id| (id, app.viewed_agent().map_or(0, |run| run.history.len()))),
            prompt_len: app.system_prompt.as_ref().map(String::len),
            agent_prompt_len: app.agent_system_prompt.as_ref().map(String::len),
            instructions_len: app.user_instructions.as_ref().map(String::len),
            skills_len: app.skill_listing.as_ref().map(String::len),
        }
    }
}

impl ContextCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The window's current line count, rebuilding first when it is stale —
    /// what the draw's scroll clamp feeds [`tool_view_max_scroll_for`].
    ///
    /// [`tool_view_max_scroll_for`]: super::tool_view_max_scroll_for
    pub fn line_count(&mut self, app: &App, width: u16) -> usize {
        self.refresh(app, width);
        self.lines.len()
    }

    /// The rendered window, rebuilt only when the signature changed.
    pub fn lines(&mut self, app: &App, width: u16) -> &[Line<'static>] {
        self.refresh(app, width);
        &self.lines
    }

    fn refresh(&mut self, app: &App, width: u16) {
        let sig = ContextSig::of(app, width);
        if self.sig.as_ref() == Some(&sig) {
            return;
        }
        self.lines = context_lines(app, width);
        self.sig = Some(sig);
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }
}

/// Render the full-screen Ctrl+D context-debug view — the transcript pager's
/// chrome over the raw context window `lines` (built by [`context_lines`],
/// served through the loop's [`ContextCache`]), windowed by
/// `App::debug_scroll` (clamped) with `~` filler past the end. Pure —
/// `term.rs` paints this onto the overlay. See `docs/context.md`.
pub fn render_context_view(area: Rect, buf: &mut Buffer, app: &App, lines: &[Line<'static>]) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(overlay_header(CONTEXT_VIEW_TITLE, area.width)).render(title_area, buf);

    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.debug_scroll.min(max);
    let mut visible: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .cloned()
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

//! The Ctrl+D context-debug overlay: the raw LLM context window a real
//! backend sends each turn. See `docs/context.md`.

use super::assistant::expand_code_tabs;
use super::theme::*;
use super::transcript::{overlay_header, tool_view_separator};
use super::wrap::{cols, wrap_verbatim};
use super::*;

/// The role-tag colour of one context entry (the `context_*_color` roles).
fn context_role_color(role: crate::context::ContextRole) -> Color {
    match role {
        crate::context::ContextRole::User => context_user_color(),
        crate::context::ContextRole::Assistant => context_assistant_color(),
        crate::context::ContextRole::System => context_system_color(),
        crate::context::ContextRole::Tool => context_tool_color(),
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
                Style::new().fg(context_tool_color()),
            )));
        }
    }
    for path in images {
        // Wrapped like the text — a long temp path must not clip off-screen.
        let label = format!("{CONTEXT_IMAGE_LABEL}{}", path.display());
        for row in wrap_verbatim(&label, text_width) {
            lines.push(Line::from(Span::styled(
                format!("{CONTEXT_INDENT}{row}"),
                Style::new().fg(tool_dim_color()),
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
    // subagent is actually sent — its type's definition, else the main
    // prompt, plus the subagent note (`App::agent_system_prompt`, from
    // `ReplySource::agent_system_prompt`) — with no AGENTS.md fragment
    // (subagents get none). See `docs/agent-tool.md`.
    // A viewed subagent's window has no AGENTS.md fragment (subagents get
    // none — its instructions ride the system prompt shown above), but it
    // does lead with a reminder of its own: the skills roster its fresh
    // context was briefed with, ahead of the task, which is the order the
    // agent read them in (`docs/subagents.md`). No agent types in it —
    // subagents cannot launch agents.
    let (history, instructions, reminder, system_prompt) = match app.viewed_agent() {
        Some(run) => (
            run.history.as_slice(),
            None,
            app.agent_system_reminder.as_deref(),
            app.agent_system_prompt.as_ref(),
        ),
        None => (
            app.history.as_slice(),
            app.user_instructions.as_deref(),
            app.system_reminder.as_deref(),
            app.system_prompt.as_ref(),
        ),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(prompt) = system_prompt {
        context_entry_lines(
            &mut lines,
            CONTEXT_SYSTEM_PROMPT_TAG,
            context_system_color(),
            prompt,
            &[],
            &[],
            width,
        );
    }
    for message in crate::context::context_messages_full(instructions, reminder, history) {
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
            Style::new().fg(tool_dim_color()),
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
    /// The active theme — the role tags' colours depend on it
    /// (`docs/theme.md`).
    theme: crate::app::Theme,
    history_len: usize,
    width: u16,
    agent: Option<(String, usize)>,
    prompt_len: Option<usize>,
    agent_prompt_len: Option<usize>,
    instructions_len: Option<usize>,
    reminder_len: Option<usize>,
    agent_reminder_len: Option<usize>,
}

impl ContextSig {
    fn of(app: &App, width: u16) -> Self {
        Self {
            generation: app.history_generation(),
            theme: super::palette::active_theme(),
            history_len: app.history.len(),
            width,
            agent: app
                .agent_view
                .clone()
                .map(|id| (id, app.viewed_agent().map_or(0, |run| run.history.len()))),
            prompt_len: app.system_prompt.as_ref().map(String::len),
            agent_prompt_len: app.agent_system_prompt.as_ref().map(String::len),
            instructions_len: app.user_instructions.as_ref().map(String::len),
            reminder_len: app.system_reminder.as_ref().map(String::len),
            agent_reminder_len: app.agent_system_reminder.as_ref().map(String::len),
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

    /// Drop the rendered window (allocation included). The cache exists to
    /// make redraws O(viewport) *while the view is up* — retained past the
    /// close it is a second full rendered copy of the conversation sitting
    /// resident for the rest of the session after one Ctrl+D peek, so the
    /// close releases it and the next open rebuilds once.
    pub fn release(&mut self) {
        self.sig = None;
        self.lines = Vec::new();
    }

    /// Test-only: how many rendered rows the cache is holding right now —
    /// what [`ContextCache::release`] promises to return to zero.
    #[cfg(test)]
    #[must_use]
    pub(super) fn retained_rows(&self) -> usize {
        self.lines.len()
    }
}

/// Render the full-screen Ctrl+D view — the transcript pager's chrome over
/// whichever page is showing, windowed by that page's own scroll offset
/// (clamped) with `~` filler past the end.
///
/// Both pages come through here: `lines` is [`context_lines`]' raw context
/// window (served through the loop's [`ContextCache`]) or
/// [`classifier_lines`]' task context, and the page decides only the title,
/// the scroll offset, and which way the Tab hint points. Sharing the chrome
/// is the point — two windows onto "what is this turn actually sending",
/// one key apart. Pure — `term.rs` paints this onto the overlay. See
/// `docs/context.md` and `docs/permissions.md`.
///
/// [`classifier_lines`]: super::classifier_lines
pub fn render_context_view(area: Rect, buf: &mut Buffer, app: &App, lines: &[Line<'static>]) {
    let classifier = app.debug_page == crate::app::DebugPage::Classifier;
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    let title = if classifier {
        CLASSIFIER_VIEW_TITLE
    } else {
        CONTEXT_VIEW_TITLE
    };
    Paragraph::new(overlay_header(title, area.width)).render(title_area, buf);

    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = if classifier {
        app.classifier_scroll.min(max)
    } else {
        app.debug_scroll.min(max)
    };
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

    let dim = Style::new().fg(tool_dim_color());
    // The quit row carries the page-flip hint: Tab is the other page's only
    // discovery affordance, so it names the page it would show.
    let tab_hint = if classifier {
        CONTEXT_VIEW_HINT_TAB_LLM
    } else {
        CONTEXT_VIEW_HINT_TAB_CLASSIFIER
    };
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(
            format!("{CONTEXT_VIEW_HINT_QUIT}{tab_hint}"),
            dim,
        )),
    ])
    .render(hints_area, buf);
}

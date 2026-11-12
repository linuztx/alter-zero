//! The Ctrl+D context-debug overlay: the raw LLM context window a real
//! backend sends each turn. See `docs/architecture/context.md`.

use std::collections::HashMap;

use super::assistant::expand_code_tabs;
use super::file_cell::gutter_row_spans;
use super::theme::*;
use super::transcript::{labeled_separator, overlay_header};
use super::wrap::{clamp_spans, cols, spans_cols, wrap_output, wrap_output_hanging, wrap_verbatim};
use super::*;
use crate::context::{ContextRole, ContextToolCall};
use crate::context_usage::CountMemo;
use crate::reminder::{REMINDER_CLOSE, REMINDER_OPEN};

/// One built page of the Ctrl+D view — [`context_body`]'s context window or
/// [`classifier_body`](super::classifier_body)'s task context: its rows, the
/// row each entry opens on, and the summary its bottom rule carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebugBody {
    /// The page's rows, top to bottom.
    pub lines: Vec<Line<'static>>,
    /// The row each entry's header — its role chip — sits on, in order:
    /// what ←/→ step between (`App::set_debug_headers`) and where the
    /// header pinned over an entry the window opens inside is read from.
    pub headers: Vec<usize>,
    /// The lead of the bottom rule, `5 messages · ≈1.2k tokens`, when the
    /// page has one.
    pub summary: Option<String>,
}

/// An entry's role chip label.
const fn role_label(role: ContextRole) -> &'static str {
    match role {
        ContextRole::System => CONTEXT_ROLE_SYSTEM,
        ContextRole::User => CONTEXT_ROLE_USER,
        ContextRole::Assistant => CONTEXT_ROLE_ASSISTANT,
        ContextRole::Tool => CONTEXT_ROLE_TOOL,
    }
}

/// An entry's role chip fill (the `context_*_color` roles).
fn role_color(role: ContextRole) -> Color {
    match role {
        ContextRole::User => context_user_color(),
        ContextRole::Assistant => context_assistant_color(),
        ContextRole::System => context_system_color(),
        ContextRole::Tool => context_tool_color(),
    }
}

/// An entry's header row: its **role chip** — the label upper-case on a
/// fill of the role's hue under the on-accent ink, bold, the row the eye
/// hunts for down a page of raw text — then `facts` dim beside it, the whole
/// row cut with `…` at the width rather than wrapped, so a header is always
/// one row (the pinned header is a copy of it).
pub(super) fn entry_header(role: ContextRole, facts: &[String], width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{CONTEXT_CHIP_PAD}{}{CONTEXT_CHIP_PAD}", role_label(role)),
        Style::new()
            .fg(context_chip_fg())
            .bg(role_color(role))
            .add_modifier(Modifier::BOLD),
    )];
    if !facts.is_empty() {
        spans.push(Span::styled(
            format!(
                "{CONTEXT_HEADER_GAP}{}",
                facts.join(CONTEXT_DETAIL_SEPARATOR)
            ),
            Style::new().fg(tool_dim_color()),
        ));
    }
    clamp_spans(spans, usize::from(width))
}

/// Whether `width` leaves room for the [`TOOL_RESULT_PREFIX`] gutter beside
/// an entry's text.
fn gutter_fits(width: u16) -> bool {
    usize::from(width) >= cols(TOOL_RESULT_PREFIX) + CONTEXT_BODY_MIN_COLS
}

/// The columns an entry's text wraps to — the width less its gutter, or the
/// whole width once the terminal is too narrow to spend the gutter's columns.
pub(super) fn body_width(width: u16) -> u16 {
    if gutter_fits(width) {
        width - cols(TOOL_RESULT_PREFIX) as u16
    } else {
        width
    }
}

/// Append one entry to `body`: its header row (recorded in
/// [`DebugBody::headers`]), its body `rows` hung on the tool cell's `⎿`
/// gutter — the corner on the first row, the rest aligned under it — or
/// [`CONTEXT_EMPTY_BODY`] when it has none, and a blank spacer.
pub(super) fn push_entry(
    body: &mut DebugBody,
    role: ContextRole,
    facts: &[String],
    rows: Vec<Vec<Span<'static>>>,
    width: u16,
) {
    body.headers.push(body.lines.len());
    body.lines.push(entry_header(role, facts, width));
    let rows = if rows.is_empty() {
        vec![vec![Span::styled(
            CONTEXT_EMPTY_BODY,
            Style::new().fg(tool_dim_color()),
        )]]
    } else {
        rows
    };
    let gutter = gutter_fits(width);
    for (index, spans) in rows.into_iter().enumerate() {
        body.lines.push(if gutter {
            gutter_row_spans(index, spans)
        } else {
            Line::from(spans)
        });
    }
    body.lines.push(Line::default());
}

/// Whether `line` is one of the `<system-reminder>` block's own markers.
fn is_reminder_marker(line: &str) -> bool {
    let line = line.trim();
    line == REMINDER_OPEN || line == REMINDER_CLOSE
}

/// A text's body rows at `width`: **word**-wrapped with its own spacing kept
/// ([`wrap_output`] — the hard mid-word break it replaced made every long
/// message a puzzle), tabs expanded (they paint as zero cells otherwise —
/// see `tool_output_lines`), trailing blank lines dropped (a model's `…\n\n`
/// before its tool call stacked empty rows between the text and the call it
/// introduces — invisible on the wire, so nothing is lost), and the
/// `<system-reminder>` markers dimmed so the block reads apart from what it
/// wraps. Never the markdown renderer: the whole point is the unformatted
/// wire content. No rows for a text with nothing in it.
pub(super) fn text_rows(text: &str, width: u16) -> Vec<Vec<Span<'static>>> {
    let text = text.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let ink = Style::new().fg(context_text_color());
    let dim = Style::new().fg(tool_dim_color());
    let mut rows = Vec::new();
    for line in expand_code_tabs(text).split('\n') {
        let style = if is_reminder_marker(line) { dim } else { ink };
        for row in wrap_output(line, width) {
            rows.push(vec![Span::styled(row, style)]);
        }
    }
    rows
}

/// Dim prose rows: `text` word-wrapped at `width` — a placeholder, a fold
/// count, a note: sentences, which a mid-word break made a puzzle.
pub(super) fn dim_rows(text: &str, width: u16) -> Vec<Vec<Span<'static>>> {
    let dim = Style::new().fg(tool_dim_color());
    wrap_output(text, width)
        .into_iter()
        .map(|row| vec![Span::styled(row, dim)])
        .collect()
}

/// An attachment's dim row(s): `image: {path}`, wrapped **verbatim** like
/// the text above it rather than by word — a path has no spaces worth
/// breaking at, and a long temp path must never clip off-screen.
fn image_rows(path: &std::path::Path, width: u16) -> Vec<Vec<Span<'static>>> {
    let dim = Style::new().fg(tool_dim_color());
    wrap_verbatim(&format!("{CONTEXT_IMAGE_LABEL}{}", path.display()), width)
        .into_iter()
        .map(|row| vec![Span::styled(row, dim)])
        .collect()
}

/// A native tool call's rows: the raw `→ name(arguments)` wire form, the
/// arrow and the name lifted in the tool hue, bold, over the arguments in
/// the body's ink, closed by the call's dim id ([`with_call_id`]). The
/// arguments word-wrap like the text above them — they carry a command or a
/// file's contents as often as not — **hanging** under the `→ name` lead
/// ([`wrap_output_hanging`], the tool header's rule: a first word too wide
/// for the lead's row but not for a whole one opens the next row, the lead
/// standing alone). A terminal too narrow for the lead itself wraps the
/// whole call verbatim instead.
fn call_rows(call: &ContextToolCall, width: u16) -> Vec<Vec<Span<'static>>> {
    with_call_id(request_rows(call, width), &call.id, width)
}

/// `rows` closed by the call's `id`, dim — the id its `TOOL` entry names
/// too, so a parallel batch's results pair with their calls by eye: beside
/// the last row where the two fit, else on a row (or rows) of its own.
fn with_call_id(
    mut rows: Vec<Vec<Span<'static>>>,
    id: &str,
    width: u16,
) -> Vec<Vec<Span<'static>>> {
    if id.is_empty() {
        return rows;
    }
    let dim = Style::new().fg(tool_dim_color());
    let beside = format!("{CONTEXT_CALL_ID_GAP}{id}");
    let fits = |row: &Vec<Span<'static>>| {
        width == 0 || spans_cols(row) + cols(&beside) <= usize::from(width)
    };
    match rows.last_mut() {
        Some(last) if fits(last) => last.push(Span::styled(beside, dim)),
        _ => rows.extend(dim_rows(id, width)),
    }
    rows
}

/// The `→ name(arguments)` request's own rows, before the id closes them.
fn request_rows(call: &ContextToolCall, width: u16) -> Vec<Vec<Span<'static>>> {
    let head = format!("{CONTEXT_TOOL_CALL_PREFIX}{}", call.name);
    let name = Style::new()
        .fg(context_tool_color())
        .add_modifier(Modifier::BOLD);
    let arguments = Style::new().fg(context_text_color());
    let tail = format!("({})", call.arguments);
    let lead = cols(&head);
    if width == 0 || lead < usize::from(width) {
        let first = width.saturating_sub(lead as u16);
        return wrap_output_hanging(&tail, first, width)
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let mut spans = Vec::new();
                if index == 0 {
                    spans.push(Span::styled(head.clone(), name));
                }
                if !row.is_empty() {
                    spans.push(Span::styled(row, arguments));
                }
                spans
            })
            .collect();
    }
    let whole = format!("{head}{tail}");
    let mut rows = Vec::new();
    let mut start = 0;
    for row in wrap_verbatim(&whole, width) {
        // A verbatim row is the next slice of its line, and the head lies
        // whole in the first line, so where it ends inside a row is known.
        let split = head.len().saturating_sub(start).min(row.len());
        start += row.len();
        let mut spans = Vec::new();
        if split > 0 {
            spans.push(Span::styled(row[..split].to_string(), name));
        }
        if split < row.len() {
            spans.push(Span::styled(row[split..].to_string(), arguments));
        }
        rows.push(spans);
    }
    rows
}

/// `n` of `noun`, its plural a trailing `s` — every noun the view counts
/// (message, tool call, image, token) is regular.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A token estimate as a header fact: `≈1.2k tokens` — the app's estimate
/// in the provider's tokens, marked the way the `/context` page marks its own.
fn token_estimate(tokens: usize) -> String {
    let noun = if tokens == 1 { "token" } else { "tokens" };
    format!(
        "{CONTEXT_USAGE_ESTIMATE_MARK}{} {noun}",
        format_token_count(tokens)
    )
}

/// The Ctrl+D body — the raw context window, oldest first: the system
/// prompt (when the backend sends one), then every message
/// [`crate::context::context_messages`] derives from the history. What you
/// read here is what [`crate::llm::backend::build_messages`] sends (the
/// attachments as their paths rather than encoded bytes).
///
/// Each entry opens on its role chip over the facts a raw window is opened
/// to find — its place in the request, the tool a result answers, a
/// token estimate of its size, its calls and pictures — then its text
/// hung on the `⎿` gutter, any native tool calls as `→ name(arguments)` rows
/// and attachment paths dim beneath. The bottom rule's summary counts the
/// messages and their estimate. A dim placeholder when there is nothing yet.
/// See `docs/architecture/context.md`.
#[must_use]
pub fn context_body(app: &App, width: u16) -> DebugBody {
    let calibration = app.context_calibration();
    build_context_body(app, width, &mut |text| {
        calibration.correct_count(crate::app::count_tokens(text))
    })
}

/// [`context_body`]'s rows alone.
#[must_use]
pub fn context_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    context_body(app, width).lines
}

/// [`context_body`] with each text's token count from `count` — the cache
/// hands in its memo ([`ContextCache`]).
fn build_context_body(app: &App, width: u16, count: &mut dyn FnMut(&str) -> usize) -> DebugBody {
    // An agent session view debugs the *viewed agent's* context: its own
    // transcript derived through the same mapping, under the prompt a
    // subagent is actually sent — its type's definition, else the main
    // prompt, plus the subagent note (`App::agent_system_prompt`, from
    // `ReplySource::agent_system_prompt`) — with no AGENTS.md section
    // (subagents get none). See `docs/agents/agent-tool.md`.
    // A viewed subagent's window has no AGENTS.md section (subagents get
    // none — its instructions ride the system prompt shown above), but it
    // does lead with a `<system-reminder>` of its own: the skills roster its
    // fresh context was briefed with, ahead of the task, which is the order
    // the agent read them in (`docs/agents/subagents.md`) — taken whole, as the
    // launch wrapped it, where the main window composes its block from the
    // two sections `App` holds (`context::context_messages_full`). No agent
    // types in it — subagents cannot launch agents.
    let (messages, system_prompt) = match app.viewed_agent() {
        Some(run) => (
            crate::context::context_messages_behind(
                app.agent_system_reminder.as_deref(),
                &run.history,
            ),
            app.agent_system_prompt.as_ref(),
        ),
        None => (
            crate::context::context_messages_full(
                app.user_instructions.as_deref(),
                app.listings.as_deref(),
                &app.history,
            ),
            app.system_prompt.as_ref(),
        ),
    };
    // Empty texts count nothing, so a memo is never asked about one.
    let mut tokens_of = |text: &str| if text.is_empty() { 0 } else { count(text) };
    let text_width = body_width(width);
    let mut body = DebugBody::default();
    let mut total = 0;
    let mut index = 0;
    // A result names the call it answers by id; the calls ride the
    // assistant entries ahead of it.
    let mut call_names: HashMap<&str, &str> = HashMap::new();
    if let Some(prompt) = system_prompt {
        index += 1;
        let tokens = tokens_of(prompt);
        total += tokens;
        let mut facts = vec![format!("{CONTEXT_INDEX_MARK}{index}")];
        if tokens > 0 {
            facts.push(token_estimate(tokens));
        }
        push_entry(
            &mut body,
            ContextRole::System,
            &facts,
            text_rows(prompt, text_width),
            width,
        );
    }
    for message in &messages {
        index += 1;
        // The footer gauge's own rule for one message — its text, a flat
        // charge per picture, each call's name and arguments — so the
        // entries add up to what the gauge reads.
        let tokens = crate::app::message_tokens(message, &mut tokens_of);
        let mut rows = text_rows(&message.text, text_width);
        for call in &message.tool_calls {
            call_names.insert(call.id.as_str(), call.name.as_str());
            rows.extend(call_rows(call, text_width));
        }
        for path in &message.images {
            rows.extend(image_rows(path, text_width));
        }
        total += tokens;
        let mut facts = vec![format!("{CONTEXT_INDEX_MARK}{index}")];
        if let Some(id) = message.tool_call_id.as_deref() {
            // The call it answers: the tool, then the id the call row ends on.
            if let Some(tool) = call_names.get(id) {
                facts.push((*tool).to_string());
            }
            facts.push(id.to_string());
        }
        if tokens > 0 {
            facts.push(token_estimate(tokens));
        }
        if !message.tool_calls.is_empty() {
            facts.push(counted(message.tool_calls.len(), "tool call"));
        }
        if !message.images.is_empty() {
            facts.push(counted(message.images.len(), "image"));
        }
        push_entry(&mut body, message.role, &facts, rows, width);
    }
    if index == 0 {
        body.lines.extend(
            dim_rows(CONTEXT_VIEW_EMPTY, width)
                .into_iter()
                .map(Line::from),
        );
        return body;
    }
    body.summary = Some(format!(
        "{}{CONTEXT_DETAIL_SEPARATOR}{}",
        counted(index, "message"),
        token_estimate(total)
    ));
    body
}

/// Caches the Ctrl+D view's built window so redrawing it costs O(viewport).
///
/// The derived-context walk + wrap above is O(conversation), and the view
/// redraws on every animation frame while a turn runs (the 32 ms status
/// re-arm) — rebuilt per frame it pegged the loop on a big context (a long
/// skill's rendered body in the window) and starved the scroll keys: the
/// reported "Ctrl+D freezes" bug. [`TranscriptCache`]'s little sibling: owned
/// by the event loop, consulted once per draw, the window rebuilt only when
/// the signature below changes — so a scroll key or a status tick is a cache
/// hit. It needs no incremental prefix: the window only changes at history
/// boundaries (an item lands, a rewind, a turn start's fragment refresh),
/// never per streamed chunk. The headers' token estimates come through a
/// [`CountMemo`] kept across rebuilds, so a rebuild tokenizes only the
/// entries that are new. See `docs/architecture/context.md`.
///
/// [`TranscriptCache`]: super::TranscriptCache
#[derive(Default)]
pub struct ContextCache {
    sig: Option<ContextSig>,
    body: DebugBody,
    /// Each text's token count, by length and hash — kept across rebuilds
    /// and across a close, being counts and never text.
    memo: CountMemo,
    /// Test-only: how many accesses did a rebuild, so a test can prove an
    /// unchanged conversation is a cache hit, not a rebuild.
    #[cfg(test)]
    pub(super) builds: usize,
    /// Test-only: how many texts went through the estimator, so a test can
    /// prove a rebuild counts only what is new.
    #[cfg(test)]
    pub(super) counted: usize,
}

/// What pins a cached window. History is append-only between generation bumps
/// (the transcript cache's premise), so `(generation, len)` pins the
/// conversation half; the leading fragments are pinned by their lengths —
/// cheap, and they otherwise only change beside a turn-start history append
/// (the direct edits — a `/settings` toggle dropping the instructions, a
/// `/model` switch swapping the prompt — all move the length). An open agent
/// session view swaps the whole window to that agent's derivation
/// ([`context_body`]'s first branch), so the viewed agent — and how far its
/// own transcript has grown — keys too.
#[derive(PartialEq, Eq)]
struct ContextSig {
    generation: u64,
    /// The active theme — the role chips' colours depend on it
    /// (`docs/ui/theme.md`).
    theme: crate::app::Theme,
    history_len: usize,
    width: u16,
    agent: Option<(String, usize)>,
    prompt_len: Option<usize>,
    agent_prompt_len: Option<usize>,
    instructions_len: Option<usize>,
    listings_len: Option<usize>,
    agent_reminder_len: Option<usize>,
    /// The ratio the estimates are corrected by — a new usage report's
    /// lesson recounts the window (`docs/architecture/token-estimate.md`).
    calibration: u64,
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
            listings_len: app.listings.as_ref().map(String::len),
            agent_reminder_len: app.agent_system_reminder.as_ref().map(String::len),
            calibration: app.context_calibration().ratio_permille(),
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
        self.body.lines.len()
    }

    /// The built window, rebuilt only when the signature changed.
    pub fn body(&mut self, app: &App, width: u16) -> &DebugBody {
        self.refresh(app, width);
        &self.body
    }

    fn refresh(&mut self, app: &App, width: u16) {
        let sig = ContextSig::of(app, width);
        if self.sig.as_ref() == Some(&sig) {
            return;
        }
        #[cfg(test)]
        let misses = std::cell::Cell::new(0usize);
        let tokenize = |text: &str| {
            #[cfg(test)]
            misses.set(misses.get() + 1);
            crate::app::count_tokens(text)
        };
        // In the provider's tokens, outside the memo of the estimate's own
        // counts (docs/architecture/token-estimate.md).
        let calibration = app.context_calibration();
        let memo = &mut self.memo;
        self.body = build_context_body(app, width, &mut |text| {
            calibration.correct_count(memo.count(text, &tokenize))
        });
        memo.finish_pass();
        self.sig = Some(sig);
        #[cfg(test)]
        {
            self.builds += 1;
            self.counted += misses.get();
        }
    }

    /// Drop the rendered window (allocation included). The cache exists to
    /// make redraws O(viewport) *while the view is up* — retained past the
    /// close it is a second full rendered copy of the conversation sitting
    /// resident for the rest of the session after one Ctrl+D peek, so the
    /// close releases it and the next open rebuilds once — against the token
    /// counts the memo kept, which are a few words an entry.
    pub fn release(&mut self) {
        self.sig = None;
        self.body = DebugBody::default();
    }

    /// Test-only: how many rendered rows the cache is holding right now —
    /// what [`ContextCache::release`] promises to return to zero.
    #[cfg(test)]
    #[must_use]
    pub(super) fn retained_rows(&self) -> usize {
        self.body.lines.len()
    }
}

/// The entry header to **pin** over the top row of a window opening at
/// `scroll`, `rows` tall, onto a page of `total` rows whose entries open on
/// `headers`: the entry the window opens inside, while its own header is
/// scrolled off above — so deep in a long system prompt or tool result the
/// top row still says whose text it is. Nothing on the header row itself
/// (already on top), nor once the entry has no body row left under the pin
/// (the pin would sit over its spacer, a header stacked on the next one), nor
/// in a window too short to give a row up ([`CONTEXT_PIN_MIN_ROWS`]).
fn pinned_header(headers: &[usize], total: usize, scroll: usize, rows: usize) -> Option<usize> {
    if rows < CONTEXT_PIN_MIN_ROWS {
        return None;
    }
    let next = headers.partition_point(|&row| row <= scroll);
    let current = headers[..next].last().copied()?;
    // The row before the next entry (or the page's end) is this one's spacer.
    let end = headers.get(next).copied().unwrap_or(total);
    (current < scroll && scroll + 2 < end).then_some(current)
}

/// The title row's text: the page's spaced caps, then — inside an agent
/// session view — whose window it is, the way the roster names an agent
/// (`explore agent · Fetch the weather`). Nothing else on the page says it,
/// and the lead's window and an agent's look alike.
fn view_title(app: &App, title: &str) -> String {
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    match app.viewed_agent() {
        Some(run) => format!(
            "{title}{CONTEXT_VIEW_AGENT_SEPARATOR}{}{CONTEXT_VIEW_AGENT_SUFFIX}\
             {CONTEXT_VIEW_AGENT_SEPARATOR}{}",
            flat(&run.agent_type),
            flat(&run.description),
        ),
        None => title.to_string(),
    }
}

/// Render the full-screen Ctrl+D view — the transcript pager's chrome over
/// whichever page is showing, windowed by that page's own scroll offset
/// (clamped) with `~` filler past the end, the entry the window opens inside
/// pinned over its top row (`pinned_header`) and the page's summary set
/// into the bottom rule beside the scroll percentage.
///
/// Both pages come through here: `body` is [`context_body`]'s raw context
/// window (served through the loop's [`ContextCache`]) or
/// [`classifier_body`]'s task context, and the page decides only the title,
/// the scroll offset, and which way the Tab hint points. Sharing the chrome
/// is the point — two windows onto "what is this turn actually sending",
/// one key apart. Pure — `term.rs` paints this onto the overlay. See
/// `docs/architecture/context.md` and `docs/state/permissions.md`.
///
/// [`classifier_body`]: super::classifier_body
pub fn render_context_view(area: Rect, buf: &mut Buffer, app: &App, body: &DebugBody) {
    let classifier = app.debug_page == crate::app::DebugPage::Classifier;
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    let title = view_title(
        app,
        if classifier {
            CLASSIFIER_VIEW_TITLE
        } else {
            CONTEXT_VIEW_TITLE
        },
    );
    Paragraph::new(overlay_header(&title, area.width)).render(title_area, buf);

    let lines = &body.lines;
    let rows = body_area.height as usize;
    let max = lines.len().saturating_sub(rows);
    let scroll = if classifier {
        app.classifier_scroll.min(max)
    } else {
        app.debug_scroll.min(max)
    };
    let mut visible: Vec<Line> = lines.iter().skip(scroll).take(rows).cloned().collect();
    if let (Some(at), Some(top)) = (
        pinned_header(&body.headers, lines.len(), scroll, rows),
        visible.first_mut(),
    ) {
        *top = lines[at].clone();
    }
    while visible.len() < rows {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(labeled_separator(
        area.width,
        scroll,
        max,
        body.summary.as_deref(),
    ))
    .render(sep_area, buf);

    let dim = Style::new().fg(tool_dim_color());
    // The quit row carries the page-flip hint: Tab is the other page's only
    // discovery affordance, so it names the page it would show.
    let tab_hint = if classifier {
        CONTEXT_VIEW_HINT_TAB_LLM
    } else {
        CONTEXT_VIEW_HINT_TAB_CLASSIFIER
    };
    Paragraph::new(vec![
        Line::from(Span::styled(CONTEXT_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(
            format!("{CONTEXT_VIEW_HINT_QUIT}{tab_hint}"),
            dim,
        )),
    ])
    .render(hints_area, buf);
}

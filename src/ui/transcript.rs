//! The Ctrl+O transcript overlay and the incremental cache behind it.
//!
//! [`TranscriptCache`] keeps a frozen prefix of per-item rendered rows so
//! opening the overlay on a long session is O(viewport), not O(history).
//! See `docs/tool-view-performance.md`.

use super::agent::{AgentCellView, agent_cell_lines, agent_group_full_lines};
use super::conversation::is_shell_header;
use super::message::{compaction_full_lines, hook_note_lines, user_stamp_lines};
use super::reasoning::{reasoning_full_lines, reasoning_live_full_lines};
use super::theme::*;
use super::tool::tool_full_lines;
use super::wrap::cols;
use super::*;

/// Build the full conversation transcript shown in the tool-output view: the
/// startup header banner (docs/header.md — the overlay mirrors the inline
/// scrollback, which opens with it), then every user/assistant/error message
/// **and** every tool call's complete output, interleaved in the exact order
/// they happened (straight from `App::history`), followed by the live tail —
/// the in-progress reply and/or the running tool. A blank line separates
/// items. Tools are shown *expanded* here (the inline view collapses them).
/// Empty → the banner over a single placeholder line.
///
/// Only the **user** message shows its wall-clock `timestamp`: dim,
/// right-aligned on its own line below the message (`user_stamp_lines`) — the
/// **only** stamp displayed anywhere (AI replies, tools, and turn summaries
/// record one but never show it; the inline view never shows any; see
/// `docs/timestamps.md`).
#[must_use]
pub fn transcript_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    transcript_build(app, width).0
}

/// The line range the backtrack preview's highlighted user message occupies
/// in [`transcript_lines`]'s output — the scroll-into-view target
/// ([`backtrack_scroll`]); `None` when no preview is active. Computed by the
/// same walk that styles the highlight (`transcript_build`), so the two can
/// never drift. See `docs/backtrack.md`.
#[must_use]
pub fn transcript_selection(app: &App, width: u16) -> Option<Range<usize>> {
    transcript_build(app, width).1
}

/// The single transcript walk behind [`transcript_lines`] and
/// [`transcript_selection`]: a fresh [`TranscriptCache`] refreshed once — the
/// incremental build *is* the only transcript implementation (one code path,
/// so the cached and from-scratch renders can never drift apart; the draw loop
/// reuses a long-lived cache instead of paying this full build).
fn transcript_build(app: &App, width: u16) -> (Vec<Line<'static>>, Option<Range<usize>>) {
    let mut cache = TranscriptCache::new();
    cache.refresh(app, width);
    (std::mem::take(&mut cache.lines), cache.selection)
}

/// One history item's transcript rows — the message (or the tool's *expanded*
/// output, or a summary/background notice) plus its trailing blank spacer,
/// self-contained so [`TranscriptCache`] can render each committed item
/// exactly once and only ever append. The spacer is skipped after a shell
/// command's header ([`is_shell_header`]): its tool's `⎿` output sits flush
/// below it, whether that tool is already in history or still the running
/// live tail, so the overlay renders the same exec cell as the inline view.
///
/// The second value is `Some(message-row count)` for a **user** message — the
/// span the Esc-Esc backtrack preview reverses (its timestamp line below stays
/// normal; see `docs/backtrack.md`) — `None` for everything else.
fn transcript_item_lines(item: &HistoryItem, width: u16) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut user_rows = None;
    match item {
        HistoryItem::Message(m) => {
            let message = message_lines(m.role, &m.text, width);
            if m.role == Role::User {
                user_rows = Some(message.len());
            }
            lines.extend(message);
            if m.role == Role::User {
                lines.extend(user_stamp_lines(&m.timestamp, width));
            }
        }
        HistoryItem::Tool(t) => lines.extend(tool_full_lines(t, width)),
        // A task call is invisible inline but the transcript is the full
        // record: expand it as an ordinary tool cell —
        // `● TaskCreate(subject)` over its `⎿` result (docs/task-tools.md).
        HistoryItem::TaskCall(t) => lines.extend(tool_full_lines(&t.as_tool_call(), width)),
        HistoryItem::Summary(s) => lines.extend(summary_lines(s, width)),
        HistoryItem::Background(n) => lines.extend(background_notice_lines(n, width)),
        // The transcript expands the group into one `● Agent({description})`
        // cell per subagent — prompt, nested tool headers, response, and the
        // Done/Interrupted footer (docs/agent-tool.md); the inline view shows
        // the collapsed tree cell.
        HistoryItem::AgentGroup(g) => lines.extend(agent_group_full_lines(g, width)),
        HistoryItem::AgentNotice(n) => lines.extend(agent_notice_lines(n, width)),
        // The transcript expands the marker with its summary body — the
        // inline view keeps it collapsed (docs/compact.md).
        HistoryItem::Compaction(c) => lines.extend(compaction_full_lines(c, width)),
        // The transcript is where a thought's whole chain-of-thought lives —
        // inline it is the one collapsed line (docs/thinking-stream.md).
        HistoryItem::Reasoning(r) => lines.extend(reasoning_full_lines(r, width)),
        // Hook-injected conversation text (docs/hooks.md): invisible inline,
        // the transcript is its record.
        HistoryItem::HookNote(n) => lines.extend(hook_note_lines(n, width)),
    }
    if !is_shell_header(item) {
        lines.push(Line::default());
    }
    (lines, user_rows)
}

/// The Ctrl+O transcript of the **viewed agent's** session
/// (`docs/agent-tool.md`), or `None` when no agent view is up (the caller
/// falls back to the main [`TranscriptCache`]). The banner over the agent's
/// items (tools expanded, exactly like the main walk) and its live tail —
/// the in-progress reply and the live tool queue. Built fresh per draw: an
/// agent transcript is bounded by one task's work, so the incremental cache
/// isn't warranted.
#[must_use]
pub fn agent_transcript_lines(app: &App, width: u16) -> Option<Vec<Line<'static>>> {
    let run = app.viewed_agent()?;
    let mut lines = header_lines(app, width);
    lines.push(Line::default());
    let chrome_rows = lines.len();
    for item in &run.history {
        let (rows, _) = transcript_item_lines(item, width);
        lines.extend(rows);
    }
    if let Some(text) = run.streaming.as_deref().filter(|text| !text.is_empty()) {
        lines.extend(message_lines(Role::Assistant, text, width));
        lines.push(Line::default());
    }
    // The agent's open thinking phase, whole — the main tail's order and its
    // reason: the pager has no row budget, unlike the strip's windowed block
    // (`docs/thinking-stream.md`, `docs/agent-view-streaming.md`).
    if let Some(text) = run.reasoning() {
        lines.extend(reasoning_live_full_lines(text, width));
        lines.push(Line::default());
    }
    for tool in &run.tool_queue {
        lines.extend(tool_full_lines(tool, width));
        lines.push(Line::default());
    }
    // …and what the user typed into this agent while it works, waiting for its
    // next round boundary — the main tail's rule (`docs/queue.md`): the Ctrl+O
    // view never hides a pending message. `queued_lines` reads the viewed
    // agent's own queue here, so this is that agent's backlog, not the
    // session's.
    if !run.queued.is_empty() {
        lines.extend(queued_lines(app, width));
        lines.push(Line::default());
    }
    if lines.len() == chrome_rows {
        lines.push(Line::from(Span::styled(
            TOOL_VIEW_EMPTY.to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        )));
    }
    Some(lines)
}

/// The pager's scrolling-body height: the screen less the title + footer chrome.
#[must_use]
pub fn tool_view_body_rows(screen_height: u16) -> usize {
    screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize
}

/// The largest scroll offset for a transcript of `line_count` rows — the content
/// height minus the body window, so the last line can reach the bottom but not
/// past it. Pure over the count so the caller can reuse a cached line build (see
/// [`TranscriptCache`]) instead of rebuilding just to clamp.
#[must_use]
pub fn tool_view_max_scroll_for(line_count: usize, screen_height: u16) -> usize {
    line_count.saturating_sub(tool_view_body_rows(screen_height))
}

/// The largest the transcript scroll offset can be on a `screen_height`-row
/// screen. Convenience over [`tool_view_max_scroll_for`] that builds the
/// transcript itself (the draw loop instead reuses [`TranscriptCache`]).
#[must_use]
pub fn tool_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    tool_view_max_scroll_for(transcript_lines(app, width).len(), screen_height)
}

/// Caches the Ctrl+O overlay's built transcript **incrementally**, so opening
/// the overlay, scrolling it, and following a live stream under it all avoid
/// re-rendering history — that walk is O(history) and, with real grammar
/// highlighting over every expanded tool cell, cost hundreds of ms on a big
/// resumed session (the old open re-highlighted everything on a blank alt
/// screen). Owned by the event loop like [`StreamRender`]; it is **retained
/// across overlay closes** (reopening is O(live tail)) and pre-warmed at the
/// boundary ([`warm`]) so the open itself renders nothing.
///
/// Committed history items are immutable and history only ever grows — every
/// other mutation (a `/clear`, a `/resume` load, a backtrack truncation, an
/// interrupt-undo pop) bumps [`App::history_generation`]. That makes
/// `(generation, width)` pin the **frozen prefix** exactly: `lines[..frozen_rows]`
/// holds the banner chrome plus every committed item, rendered once
/// (`transcript_item_lines`); each refresh truncates the volatile live tail
/// (in-progress reply, live tool queue, queued backlog) off the end and
/// re-renders just that. The Esc-Esc backtrack highlight — REVERSED rows
/// *inside* the frozen prefix — is applied as an in-place style diff
/// (`Self::restyle_selection`), never a re-render. A cheap `TranscriptSig`
/// short-circuits the refresh entirely while nothing changed, so a scroll
/// keypress is O(viewport).
///
/// [`warm`]: Self::warm
#[derive(Default)]
pub struct TranscriptCache {
    /// Pins the frozen prefix: `(history generation, width, session cwd)` —
    /// any mismatch invalidates every rendered item (the cwd feeds the banner
    /// chrome above them). `None` until the first build.
    pub(super) key: Option<FrozenKey>,
    sig: Option<TranscriptSig>,
    /// The full transcript: the frozen prefix (`..frozen_rows`) + the live tail.
    pub(super) lines: Vec<Line<'static>>,
    /// Rows of banner chrome at the top of `lines` (the empty-transcript
    /// placeholder keys on "nothing beyond the banner").
    chrome_rows: usize,
    /// End of the frozen prefix in `lines`; the live tail is rebuilt above it.
    frozen_rows: usize,
    /// Per rendered history item: its row count (for selection offsets) and,
    /// for a user message, the reversible message-row span of the backtrack
    /// preview ([`transcript_item_lines`]).
    items: Vec<RenderedItem>,
    /// The frozen rows currently carrying the backtrack preview's REVERSED
    /// styling, so a selection step can undo exactly what it applied.
    reversed: Option<Range<usize>>,
    pub(super) selection: Option<Range<usize>>,
    /// Test-only: how many refreshes did any rebuild work — so a test can
    /// prove a scroll (unchanged signature) is a cache hit, not a rebuild.
    #[cfg(test)]
    pub(super) builds: usize,
    /// Test-only: how many history items were rendered, ever — so a test can
    /// prove the frozen prefix is reused, not re-rendered.
    #[cfg(test)]
    pub(super) item_renders: usize,
}

/// What pins [`TranscriptCache`]'s frozen prefix — see the struct docs.
#[derive(PartialEq, Eq)]
pub(super) struct FrozenKey {
    generation: u64,
    pub(super) width: u16,
    pub(super) cwd: Option<String>,
}

/// One rendered history item's shape inside the frozen prefix.
struct RenderedItem {
    /// Rows this item occupies (message + stamp + spacer / expanded tool cell).
    pub(super) rows: usize,
    /// `Some(message-row count)` for a user message — the span the backtrack
    /// preview reverses; `None` otherwise.
    user_rows: Option<usize>,
}

/// The cheap fingerprint of every input to a [`TranscriptCache`] refresh — see
/// the struct docs for why lengths suffice (append-only history, plus the
/// generation catching every non-append mutation; a same-length replace after
/// an interrupt-undo pop would fool lengths alone).
#[derive(PartialEq, Eq)]
struct TranscriptSig {
    generation: u64,
    pub(super) width: u16,
    history_len: usize,
    /// Display length of the session cwd shown in the banner chrome (`None`
    /// before the boundary injects it) — set once at startup, but cheap to
    /// fingerprint, so a late injection can't leave a stale banner.
    cwd_len: Option<usize>,
    /// Live in-progress reply length (`None` when not streaming).
    streaming_len: Option<usize>,
    /// The open thinking phase's length (`None` outside one) — so a thought
    /// streaming under the overlay invalidates the tail exactly like a
    /// streaming tool's output, and Ctrl+O mid-think follows it rather than
    /// showing a frozen snapshot (`docs/thinking-stream.md`).
    reasoning_len: Option<usize>,
    /// The live tool queue's shape: `(number of live calls, front call status,
    /// front output length)`, `None` when none run. A **parallel batch** shrinks
    /// as each call commits (also bumping `history_len`) and the front flips
    /// `Waiting`→`Running` when it starts. The **front output length** changes as
    /// a running `bash` call **streams** its output
    /// ([`crate::stream::StreamEvent::ToolOutput`]) — so the Ctrl+O overlay
    /// rebuilds and tails the live output rather than showing a frozen snapshot
    /// (`docs/tool-streaming.md`); without it a single streaming call leaves the
    /// queue length and status unchanged and the overlay would go static. See
    /// `docs/parallel-tools.md`.
    pub(super) tool_queue: Option<(usize, ToolStatus, usize)>,
    /// How many messages are waiting above the box — the follow-up entries
    /// **and** the ones handed to the running turn. Both, because pushing onto
    /// `App::steered` changes no history length and no tool queue, so a
    /// signature blind to it would leave the overlay frozen on a page that no
    /// longer matches the strip (`docs/queue.md`).
    queued_len: usize,
    backtrack_selected: Option<usize>,
    /// The subagent roster's mutation counter — a live agent streaming (its
    /// tree counters, its Ctrl+O cell) invalidates the tail exactly like a
    /// streaming tool (docs/agent-tool.md).
    agents_generation: u64,
}

impl TranscriptSig {
    fn of(app: &App, width: u16) -> Self {
        let queue = app.tool_queue();
        Self {
            generation: app.history_generation(),
            width,
            history_len: app.history.len(),
            cwd_len: app.session.as_ref().map(|s| s.cwd.len()),
            streaming_len: app.streaming_text().map(str::len),
            reasoning_len: app.reasoning().map(str::len),
            tool_queue: queue
                .front()
                .map(|t| (queue.len(), t.status, t.output.len())),
            queued_len: app.queued.len() + app.steered.len(),
            backtrack_selected: app.backtrack.selected,
            agents_generation: app.agents_generation(),
        }
    }
}

impl TranscriptCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop the whole cache — the rendered transcript with it. Not part of the
    /// overlay lifecycle (the cache is deliberately retained across closes so
    /// reopening stays O(live tail)); for a caller that wants the memory back.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Pre-render the frozen prefix at the boundary, ahead of any overlay
    /// draw — after a `/resume` load, after each committed item — so pressing
    /// Ctrl+O finds every history item already rendered and pays only the
    /// live tail. When nothing changed this is a few integer compares.
    ///
    /// A **width-only** mismatch is deliberately skipped: resize events arrive
    /// in bursts (a drag delivers dozens), and a full O(history) re-render per
    /// event would freeze the loop — the first overlay draw at the new width
    /// pays the rebuild instead (behind the still-painted inline screen).
    pub fn warm(&mut self, app: &App, width: u16) {
        let width_only_miss = self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
                && k.width != width
        });
        if width_only_miss {
            return;
        }
        if self.ensure_frozen(app, width) {
            // The tail was truncated off (or the prefix reset): the next
            // refresh must rebuild it even if the volatile signature matches.
            self.sig = None;
        }
    }

    /// Bring the frozen prefix up to date with `app.history` at `width`:
    /// reset it when the [`FrozenKey`] mismatches, then render and append any
    /// items not yet cached. Returns whether anything changed; when it did,
    /// `lines` holds **only** chrome + frozen items (the live tail was
    /// truncated off) and the caller must rebuild the tail.
    fn ensure_frozen(&mut self, app: &App, width: u16) -> bool {
        let key_matches = self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.width == width
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
        });
        if !key_matches {
            self.key = Some(FrozenKey {
                generation: app.history_generation(),
                width,
                cwd: app.session.as_ref().map(|s| s.cwd.clone()),
            });
            // The header banner tops the transcript exactly as it tops the
            // inline conversation (docs/header.md) — the overlay mirrors the
            // real scrollback, so Ctrl+O never hides it.
            self.lines = header_lines(app, width);
            self.lines.push(Line::default());
            self.chrome_rows = self.lines.len();
            self.frozen_rows = self.chrome_rows;
            self.items.clear();
            self.reversed = None;
        } else if self.items.len() == app.history.len() {
            return false;
        } else {
            self.lines.truncate(self.frozen_rows);
        }
        // Append the not-yet-rendered items (all of them after a reset). The
        // generation guarantees the cached prefix is a prefix of `history`.
        for item in app.history.get(self.items.len()..).unwrap_or(&[]) {
            let (rows, user_rows) = transcript_item_lines(item, width);
            self.items.push(RenderedItem {
                rows: rows.len(),
                user_rows,
            });
            self.frozen_rows += rows.len();
            self.lines.extend(rows);
            #[cfg(test)]
            {
                self.item_renders += 1;
            }
        }
        true
    }

    /// Apply the backtrack preview's REVERSED highlight to the selected user
    /// message's rows — an in-place style diff on the frozen prefix (undo the
    /// old span, style the new), exactly mirroring the styling a from-scratch
    /// build applies, so stepping the selection never re-renders anything.
    fn restyle_selection(&mut self, app: &App) {
        let target = app.backtrack.selected.and_then(|ordinal| {
            let mut seen = 0usize;
            let mut row = self.chrome_rows;
            for item in &self.items {
                if let Some(user_rows) = item.user_rows {
                    if seen == ordinal {
                        return Some(row..row + user_rows);
                    }
                    seen += 1;
                }
                row += item.rows;
            }
            None
        });
        if self.reversed != target {
            if let Some(old) = self.reversed.take() {
                for line in &mut self.lines[old] {
                    line.style.add_modifier.remove(Modifier::REVERSED);
                }
            }
            if let Some(new) = target.clone() {
                for line in &mut self.lines[new] {
                    line.style.add_modifier.insert(Modifier::REVERSED);
                }
            }
            self.reversed = target.clone();
        }
        self.selection = target;
    }

    /// Rebuild the live tail above the frozen prefix: the in-progress
    /// assistant text, then every live tool call — the running one followed by
    /// any `⎿ Waiting…` siblings of a parallel batch, in order, so the overlay
    /// shows the full live picture and a waiting call is never hidden under
    /// Ctrl+O (docs/parallel-tools.md) — then the still-queued backlog
    /// ([`queued_lines`]' inset rows, docs/queue.md), and the placeholder when
    /// nothing at all follows the banner.
    fn build_tail(&mut self, app: &App, width: u16) {
        if let Some(text) = app.streaming_text()
            && !text.is_empty()
        {
            self.lines
                .extend(message_lines(Role::Assistant, text, width));
            self.lines.push(Line::default());
        }
        // An open thinking phase, whole (the pager has no row budget, unlike
        // the strip's windowed block) — docs/thinking-stream.md.
        if let Some(text) = app.reasoning() {
            self.lines.extend(reasoning_live_full_lines(text, width));
            self.lines.push(Line::default());
        }
        // The live agent group's members expand as their own `● Agent(…)`
        // cells — activity live — before the tool queue, mirroring the strip's
        // order (docs/agent-tool.md). Committed groups render from history.
        if let Some(live) = app.agent_group() {
            for id in &live.ids {
                if let Some(run) = app.agent(id) {
                    self.lines
                        .extend(agent_cell_lines(&AgentCellView::of_run(run), width));
                    self.lines.push(Line::default());
                }
            }
        }
        for tool in app.tool_queue() {
            self.lines.extend(tool_full_lines(tool, width));
            self.lines.push(Line::default());
        }
        if !app.queued.is_empty() || !app.steered.is_empty() {
            self.lines.extend(queued_lines(app, width));
            self.lines.push(Line::default());
        }
        if self.lines.len() == self.chrome_rows {
            self.lines.push(Line::from(Span::styled(
                TOOL_VIEW_EMPTY.to_string(),
                Style::new().fg(TOOL_DIM_COLOR),
            )));
        }
    }

    /// Rebuild what changed since the last call — nothing when the signature
    /// matches, otherwise the frozen-prefix append + selection restyle + live
    /// tail (see the struct docs).
    fn refresh(&mut self, app: &App, width: u16) {
        let sig = TranscriptSig::of(app, width);
        if self.sig.as_ref() == Some(&sig) {
            return;
        }
        if !self.ensure_frozen(app, width) {
            // Only the volatile tail changed: drop it, keep the frozen prefix.
            self.lines.truncate(self.frozen_rows);
        }
        self.restyle_selection(app);
        self.build_tail(app, width);
        self.sig = Some(sig);
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }

    /// The cached transcript rows, rebuilding first only if stale.
    pub fn lines(&mut self, app: &App, width: u16) -> &[Line<'static>] {
        self.refresh(app, width);
        &self.lines
    }

    /// The cached row count (for the scroll clamp) — no borrow held.
    pub fn line_count(&mut self, app: &App, width: u16) -> usize {
        self.refresh(app, width);
        self.lines.len()
    }

    /// The cached backtrack-preview selection range, rebuilding first if stale.
    pub fn selection(&mut self, app: &App, width: u16) -> Option<Range<usize>> {
        self.refresh(app, width);
        self.selection.clone()
    }
}

/// Where the transcript scroll must sit to show `target` in a `viewport`-row
/// window: up to its top when it is above, down just enough when it is below,
/// unmoved when already visible — and its top when it is taller than the
/// window (codex's `scroll_chunk_into_view`).
pub(super) fn scroll_into_view(current: usize, target: &Range<usize>, viewport: usize) -> usize {
    if target.start < current {
        target.start
    } else if target.end > current + viewport {
        target.end.saturating_sub(viewport).min(target.start)
    } else {
        current
    }
}

/// The `tool_scroll` that brings a backtrack preview's `selection` range into a
/// `screen_height`-row pager, given the `current` scroll — or `None` when there
/// is no selection. Pure over the range so the draw loop can pass a cached
/// selection ([`TranscriptCache::selection`]) instead of rebuilding.
#[must_use]
pub fn backtrack_scroll_for(
    selection: Option<Range<usize>>,
    current: usize,
    screen_height: u16,
) -> Option<usize> {
    let range = selection?;
    Some(scroll_into_view(
        current,
        &range,
        tool_view_body_rows(screen_height),
    ))
}

/// The overlay draw's scroll decision while a backtrack preview is active:
/// the `tool_scroll` that brings the highlighted user message into the
/// pager's body window, or `None` with no selection. Convenience over
/// [`backtrack_scroll_for`] that builds the selection itself (the draw loop
/// reuses [`TranscriptCache`]). See `docs/backtrack.md`.
#[must_use]
pub fn backtrack_scroll(app: &App, width: u16, screen_height: u16) -> Option<usize> {
    backtrack_scroll_for(
        transcript_selection(app, width),
        app.tool_scroll,
        screen_height,
    )
}

/// The pager's title row: `/ ` tiled across the width (a `/` on every even
/// column) with the spaced-caps `/ T R A N S C R I P T` overlaid from the left
/// edge, all dim — codex's transcript overlay header.
fn tool_view_header(width: u16) -> Line<'static> {
    overlay_header(TOOL_VIEW_TITLE, width)
}

/// A full-screen overlay's title row: the slash tiling with the spaced-caps
/// `title` overlaid from the left edge, all dim — shared by the transcript
/// pager and the `/resume` picker.
pub(super) fn overlay_header(title: &str, width: u16) -> Line<'static> {
    let title = format!("/ {title}");
    let mut text: String = title.chars().take(width as usize).collect();
    for col in cols(&text)..width as usize {
        text.push(if col.is_multiple_of(2) { '/' } else { ' ' });
    }
    Line::from(Span::styled(text, Style::new().fg(TOOL_DIM_COLOR)))
}

/// The pager's bottom rule: a dim `─` separator carrying the scroll position
/// as a right-aligned ` {pct}% ` one dash in from the right edge — 0% at the
/// top, 100% at the bottom (or whenever everything fits) — codex's transcript
/// overlay bottom bar.
pub(super) fn tool_view_separator(width: u16, scroll: usize, max: usize) -> Line<'static> {
    let pct = if max == 0 {
        100
    } else {
        (scroll.min(max) * 100 + max / 2) / max
    };
    rule_with_label(width, &format!(" {pct}% "))
}

/// A dim full-width `─` rule with `label` embedded right-aligned one dash in
/// from the edge — the bottom bar shared by the transcript pager (its scroll
/// percentage) and the `/resume` picker (its selection count).
pub(super) fn rule_with_label(width: u16, label: &str) -> Line<'static> {
    let width = width as usize;
    let mut rule = vec!['─'; width];
    let start = width.saturating_sub(cols(label) + 1);
    for (i, ch) in label.chars().enumerate() {
        if let Some(cell) = rule.get_mut(start + i) {
            *cell = ch;
        }
    }
    Line::from(Span::styled(
        rule.into_iter().collect::<String>(),
        Style::new().fg(TOOL_DIM_COLOR),
    ))
}

/// Render the full-screen tool-output view — codex's Ctrl+T transcript pager:
/// the slash-tiled title row, then the scrolling conversation transcript
/// (messages + every tool call's full output), windowed by `App::tool_scroll`
/// (clamped so it can't run past the end) with `~` filler on the body rows
/// past its end, then the percentage separator and the dim key-hint rows.
/// Pure — `term.rs` paints this onto the overlay.
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App, lines: &[Line<'static>]) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(tool_view_header(area.width)).render(title_area, buf);

    // `lines` is prebuilt (by the caller's `TranscriptCache`) at `area.width` —
    // the vertical split above keeps the full width, so `body_area.width` matches.
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
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
    // While a backtrack preview highlights a message, the close-hint row
    // shows the preview's keys instead (codex's highlighted-pager footer —
    // docs/backtrack.md); the scroll keys above keep working either way.
    // Outside a preview the row tells the truth about Esc: when it would
    // *begin* the preview (idle with a previous user message — the shared
    // `App::overlay_esc_backtracks`, the very predicate the key arm guards
    // on) the hint says `esc to edit prev` and drops Esc from the quit keys;
    // only when Esc genuinely closes the overlay does the classic
    // `q/esc/ctrl+o to quit` stand.
    let closing = if app.backtrack.selected.is_some() {
        TOOL_VIEW_HINT_BACKTRACK
    } else if app.overlay_esc_backtracks() {
        TOOL_VIEW_HINT_QUIT_EDIT
    } else {
        TOOL_VIEW_HINT_QUIT
    };
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(closing.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

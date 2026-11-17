//! The Ctrl+O transcript overlay and the incremental cache behind it.
//!
//! [`TranscriptCache`] keeps a frozen prefix of per-item rendered rows so
//! opening the overlay on a long session is O(viewport), not O(history).
//! See `docs/tool-view-performance.md`.
//!
//! Every build also records the page's [`TranscriptEntry`]s — where each
//! cell opens and where its header ends — which is what ←/→ step between
//! and what the pager pins over a window that opens inside a cell, so deep
//! in a long output the top row still names the command that printed it
//! (`docs/transcript-navigation.md`).

use super::agent::{
    AGENT_CELL_HEADER_ROWS, AgentCellView, agent_cell_lines, agent_group_full_cells,
};
use super::cached_lines::CachedLines;
use super::conversation::is_shell_header;
use super::file_cell::draft_cell;
use super::hook_cell::hook_report_full_cell;
use super::message::{
    assistant_message_rows, compaction_full_lines, hook_note_lines, user_stamp_lines,
};
use super::reasoning::{reasoning_full_lines, reasoning_live_full_lines};
use super::stream_render::StreamRender;
use super::theme::*;
use super::tool::tool_full_cell;
use super::wrap::{cols, truncate_cols};
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
    let cache = transcript_build(app, width);
    cache.rows.owned_range(0..cache.rows.len())
}

/// One entry of the Ctrl+O transcript — a message, a tool call, a `!`
/// command with its output, one agent of a group, a thought — as the pager
/// navigates it: ←/→ open the previous or next entry at the window's top,
/// and while the window opens inside an entry whose header has scrolled off,
/// that header is pinned over the top (`pinned_rows`). A turn's receipt
/// (`Done for 3s`) is no entry: it is a fact about the turn, not a cell to
/// step to. See `docs/transcript-navigation.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptEntry {
    /// The row the entry opens on — its header's first row, or its first row
    /// when it has no header.
    pub start: usize,
    /// The first row past its header — `start` for an entry with none (a
    /// message), which never pins.
    pub body: usize,
    /// One past its last row; the blank spacer after it is not part of it.
    pub end: usize,
}

/// A built transcript: its rows and the [`TranscriptEntry`]s on them — what
/// [`render_tool_view`] pages through.
#[derive(Debug, Clone, Default)]
pub struct TranscriptPage {
    /// The rows, top to bottom.
    pub lines: Vec<Line<'static>>,
    /// The entries, in order, each starting below the last.
    pub entries: Vec<TranscriptEntry>,
}

/// [`transcript_lines`] with its entries — the main transcript as one page,
/// built from scratch (the draw loop pages through its long-lived
/// [`TranscriptCache`] instead).
#[must_use]
pub fn transcript_page(app: &App, width: u16) -> TranscriptPage {
    let cache = transcript_build(app, width);
    TranscriptPage {
        lines: cache.rows.owned_range(0..cache.rows.len()),
        entries: cache.entries,
    }
}

/// The header rows pinned over the top of the pager's window
/// ([`pinned_rows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pin {
    /// The page's rows pinned — the first rows of the entry's header.
    pub(super) header: Range<usize>,
    /// Whether the header goes on past them, which the last pinned row then
    /// says with [`TOOL_HEADER_ELLIPSIS`] ([`ellipsized`]).
    pub(super) cut: bool,
}

/// The header to **pin** over the top of a window opening at `scroll`,
/// `rows` tall, onto a page whose entries are `entries`: the entry the window
/// opens inside, once its header has scrolled off above — so deep in a long
/// output the top row still names the command that printed it. Ctrl+D's
/// rule (`docs/context.md`), for headers that can wrap:
///
/// - the pin is the header's first rows — at most [`TRANSCRIPT_PIN_MAX_ROWS`],
///   the inline cell's own ceiling, and never more than leaves two rows of
///   the window for what it names — `cut` when the header goes on past them;
/// - nothing while the window starts on the header's first row (it is
///   already on top), nor while the header's own later rows would still show
///   under the pin (the header twice, with a gap in it);
/// - fewer rows as the entry's last row comes up, and none once a pinned row
///   would cover it — past that, the pin would stack on the next cell's
///   header;
/// - never for an entry with no header (a message), and never in a window
///   shorter than [`CONTEXT_PIN_MIN_ROWS`].
pub(super) fn pinned_rows(entries: &[TranscriptEntry], scroll: usize, rows: usize) -> Option<Pin> {
    if rows < CONTEXT_PIN_MIN_ROWS {
        return None;
    }
    let inside = entries.partition_point(|entry| entry.start <= scroll);
    let entry = entries[..inside].last()?;
    let header = entry.body.saturating_sub(entry.start);
    if header == 0 || entry.start >= scroll || scroll >= entry.end {
        return None;
    }
    let shown = header
        .min(TRANSCRIPT_PIN_MAX_ROWS)
        .min(rows + 1 - CONTEXT_PIN_MIN_ROWS)
        .min(entry.end - scroll - 1);
    (shown > 0 && entry.body <= scroll + shown).then(|| Pin {
        header: entry.start..entry.start + shown,
        cut: shown < header,
    })
}

/// `line` closed by [`TOOL_HEADER_ELLIPSIS`] — the last row of a pinned
/// header that goes on past the pin. The mark sits right after the row's
/// text: a padded row (a `!` command's dark header) keeps its width, the
/// mark taking a column of the padding, and a row too wide for it is cut
/// first by display columns. The mark wears the text's colour and weight,
/// never its link carrier — it is not part of the path it follows.
pub(super) fn ellipsized(line: Line<'_>, width: u16) -> Line<'_> {
    let width = usize::from(width);
    let mark = cols(TOOL_HEADER_ELLIPSIS);
    let total: usize = line.spans.iter().map(|span| cols(&span.content)).sum();
    let blank = trailing_blank_cols(&line.spans);
    let keep = (total - blank).min(width.saturating_sub(mark));
    let ink = line
        .spans
        .iter()
        .rev()
        .find(|span| !span.content.trim().is_empty())
        .map_or_else(Style::default, |span| Style {
            underline_color: None,
            ..span.style
        });
    let pad_style = line.spans.last().map(|span| span.style);
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    let mut used = 0;
    for span in line.spans {
        let w = cols(&span.content);
        if used + w <= keep {
            used += w;
            spans.push(span);
            continue;
        }
        let cut = truncate_cols(&span.content, keep - used);
        if !cut.is_empty() {
            used += cols(&cut);
            spans.push(Span::styled(cut, span.style));
        }
        break;
    }
    spans.push(Span::styled(TOOL_HEADER_ELLIPSIS, ink));
    // Only a padded row keeps its padding; a row cut for width just ends.
    let pad = if blank > 0 {
        total.min(width).saturating_sub(used + mark)
    } else {
        0
    };
    if let (true, Some(style)) = (pad > 0, pad_style) {
        spans.push(Span::styled(" ".repeat(pad), style));
    }
    Line { spans, ..line }
}

/// The display columns of blank text closing `spans` — a padded row's
/// padding.
fn trailing_blank_cols(spans: &[Span<'_>]) -> usize {
    let mut blank = 0;
    for span in spans.iter().rev() {
        let trimmed = span.content.trim_end();
        blank += cols(&span.content) - cols(trimmed);
        if !trimmed.is_empty() {
            break;
        }
    }
    blank
}

/// `visible` — a window's rows — with `pinned`, the header rows of the
/// entry the window opens inside, over its top ([`pinned_rows`]): the last
/// one [`ellipsized`] when the header goes on past them.
fn pin_over<'a>(visible: &mut [Line<'a>], pinned: Vec<Line<'a>>, cut: bool, width: u16) {
    let last = pinned.len().saturating_sub(1);
    for ((at, row), slot) in pinned.into_iter().enumerate().zip(visible.iter_mut()) {
        *slot = if cut && at == last {
            ellipsized(row, width)
        } else {
            row
        };
    }
}

/// The line range the backtrack preview's highlighted user message occupies
/// in [`transcript_lines`]'s output — the scroll-into-view target
/// ([`backtrack_scroll`]); `None` when no preview is active. Computed by the
/// same walk that styles the highlight (`transcript_build`), so the two can
/// never drift. See `docs/backtrack.md`.
#[must_use]
pub fn transcript_selection(app: &App, width: u16) -> Option<Range<usize>> {
    transcript_build(app, width).selection
}

/// The single transcript walk behind [`transcript_lines`],
/// [`transcript_page`] and [`transcript_selection`]: a fresh
/// [`TranscriptCache`] refreshed once — the incremental build *is* the only
/// transcript implementation (one code path, so the cached and from-scratch
/// renders can never drift apart; the draw loop reuses a long-lived cache
/// instead of paying this full build).
fn transcript_build(app: &App, width: u16) -> TranscriptCache {
    let mut cache = TranscriptCache::new();
    cache.refresh(app, width);
    cache
}

/// What one history item put on a page ([`transcript_item_into`]): its
/// backtrack span and what its rows hold as entries.
struct ItemShape {
    /// How many rows it put down, the trailing spacer included.
    rows: usize,
    /// `Some(message-row count)` for a **user** message — the span the Esc-Esc
    /// backtrack preview reverses (its timestamp line below stays normal; see
    /// `docs/backtrack.md`) — `None` for everything else.
    user_rows: Option<usize>,
    /// The rows before the spacer.
    content: usize,
    /// The entries on the rows.
    cells: ItemCells,
}

/// What a history item's rows hold, as [`TranscriptEntry`]s.
enum ItemCells {
    /// No entry — a turn's receipt, a fact about the turn rather than a cell.
    None,
    /// One cell, every row but the spacer, its first `header` rows its
    /// header (0 for a message, which has none).
    One { header: usize },
    /// A `!` command's header: one cell with the output under it
    /// ([`EntryWalk::shell_header`]).
    ShellHeader,
    /// A `!` command's output: the rest of that cell.
    ShellOutput,
    /// A group's agents, one cell each under its one-row header, as
    /// `(first row, rows)` from the item's top.
    Agents(Vec<(usize, usize)>),
}

/// Put one history item's transcript rows on `page` — the message (or the
/// tool's *expanded* output, or a summary/background notice) plus its
/// trailing blank spacer, self-contained so [`TranscriptCache`] can render
/// each committed item exactly once and only ever append. The spacer is
/// skipped after a shell command's header ([`is_shell_header`]): its tool's
/// `⎿` output sits flush below it, whether that tool is already in history or
/// still the running live tail, so the overlay renders the same exec cell as
/// the inline view.
///
/// The rows go **straight onto the page**, whatever form it keeps them in
/// ([`RowSink`]) — an assistant reply's one at a time as its lines render —
/// so the cache's compact rows are never a second copy of an item first
/// built whole as lines: for a long code reply those lines are megabytes of
/// strings made to be copied and freed, and a freed block that size is one
/// the allocator keeps (`docs/memory.md`).
fn transcript_item_into<R: RowSink>(
    page: &mut R,
    item: &HistoryItem,
    width: u16,
    paths: &PathDisplay,
) -> ItemShape {
    let top = page.row_count();
    let mut user_rows = None;
    let cells = match item {
        HistoryItem::Message(m) if m.role == Role::Assistant => {
            assistant_message_rows(&m.text, width, |row| page.push_row(row));
            ItemCells::One { header: 0 }
        }
        HistoryItem::Message(m) => {
            let message = message_lines(m.role, &m.text, width);
            if m.role == Role::User {
                user_rows = Some(message.len());
            }
            page.push_rows(message);
            if m.role == Role::User {
                page.push_rows(user_stamp_lines(&m.timestamp, width));
            }
            if m.role == Role::Shell {
                ItemCells::ShellHeader
            } else {
                ItemCells::One { header: 0 }
            }
        }
        HistoryItem::Tool(t) => {
            let (cell, header) = tool_full_cell(t, width, paths);
            page.push_rows(cell);
            if t.shell {
                ItemCells::ShellOutput
            } else {
                ItemCells::One { header }
            }
        }
        // A task call is invisible inline but the transcript is the full
        // record: expand it as an ordinary tool cell —
        // `● TaskCreate(subject)` over its `⎿` result (docs/task-tools.md).
        HistoryItem::TaskCall(t) => {
            let (cell, header) = tool_full_cell(&t.as_tool_call(), width, paths);
            page.push_rows(cell);
            ItemCells::One { header }
        }
        HistoryItem::Summary(s) => {
            page.push_rows(summary_lines(s, width));
            ItemCells::None
        }
        HistoryItem::Background(n) => {
            page.push_rows(background_notice_lines(n, width));
            ItemCells::One { header: 0 }
        }
        // The transcript expands the group into one `● Agent({description})`
        // cell per subagent — prompt, nested tool headers, response, and the
        // Done/Interrupted footer (docs/agent-tool.md); the inline view shows
        // the collapsed tree cell. A blank row divides the agents.
        HistoryItem::AgentGroup(g) => {
            let mut agents = Vec::new();
            for (i, cell) in agent_group_full_cells(g, width, paths)
                .into_iter()
                .enumerate()
            {
                if i > 0 {
                    page.push_row(Line::default());
                }
                agents.push((page.row_count() - top, cell.len()));
                page.push_rows(cell);
            }
            ItemCells::Agents(agents)
        }
        HistoryItem::AgentNotice(n) => {
            page.push_rows(agent_notice_lines(n, width));
            ItemCells::One { header: 0 }
        }
        // The transcript expands the marker with its summary body — the
        // inline view keeps it collapsed (docs/compact.md).
        HistoryItem::Compaction(c) => {
            page.push_rows(compaction_full_lines(c, width));
            ItemCells::One { header: 1 }
        }
        // The transcript is where a thought's whole chain-of-thought lives —
        // inline it is the one collapsed line (docs/thinking-stream.md).
        HistoryItem::Reasoning(r) => {
            page.push_rows(reasoning_full_lines(r, width));
            ItemCells::One { header: 1 }
        }
        // What the hooks said, expanded — each hook that ran and what it
        // said (docs/hooks.md *The hook-run cell*); a note without a report
        // keeps its label over the text the model read.
        HistoryItem::HookNote(n) => match &n.report {
            Some(report) => {
                let (cell, header) = hook_report_full_cell(report, width);
                page.push_rows(cell);
                ItemCells::One { header }
            }
            None => {
                page.push_rows(hook_note_lines(n, width));
                ItemCells::One { header: 1 }
            }
        },
    };
    // The transcript draws the same pictures the inline view does — the
    // reserved rows are ordinary lines, so the overlay's paint stamps them
    // exactly as scrollback's does (`docs/images.md`).
    page.push_rows(super::image::image_block_lines(item, width));
    let content = page.row_count() - top;
    if !is_shell_header(item) {
        page.push_row(Line::default());
    }
    ItemShape {
        rows: page.row_count() - top,
        user_rows,
        content,
        cells,
    }
}

/// Records a page's [`TranscriptEntry`]s as its cells are appended — the
/// one walk the cache's frozen prefix, its live tail and an agent's page all
/// go through, so the three can never disagree about where a cell starts.
///
/// A `!` command is two things on a page — its dark `! cmd` header, a history
/// item of its own, and its `⎿` output flush under it, the next item or the
/// live tail — but one exec cell, whose header is the command. So the walk
/// holds the header back until it sees what follows: the output joins it,
/// and anything else leaves it an entry of its own.
#[derive(Debug, Clone, Default)]
struct EntryWalk {
    /// A `!` command header's rows, held for the output under it.
    held: Option<Range<usize>>,
}

impl EntryWalk {
    /// Record an item's entries: `content` rows before its spacer, starting
    /// at `at`, holding `cells`.
    fn item(
        &mut self,
        entries: &mut Vec<TranscriptEntry>,
        at: usize,
        content: usize,
        cells: &ItemCells,
    ) {
        match cells {
            ItemCells::None => {}
            ItemCells::One { header } => self.cell(entries, at, *header, content),
            ItemCells::ShellHeader => self.shell_header(entries, at, content),
            ItemCells::ShellOutput => self.shell_output(entries, at, content),
            ItemCells::Agents(agents) => {
                for &(first, rows) in agents {
                    self.cell(entries, at + first, AGENT_CELL_HEADER_ROWS, rows);
                }
            }
        }
    }

    /// A cell of `rows` rows at `at`, its first `header` rows its header.
    fn cell(&mut self, entries: &mut Vec<TranscriptEntry>, at: usize, header: usize, rows: usize) {
        self.flush(entries);
        entries.push(TranscriptEntry {
            start: at,
            body: at + header.min(rows),
            end: at + rows,
        });
    }

    /// A `!` command's header, `rows` rows at `at`: held for its output.
    fn shell_header(&mut self, entries: &mut Vec<TranscriptEntry>, at: usize, rows: usize) {
        self.flush(entries);
        self.held = Some(at..at + rows);
    }

    /// A `!` command's output, `rows` rows at `at`: one entry with the header
    /// held right above it, else a headerless cell of its own.
    fn shell_output(&mut self, entries: &mut Vec<TranscriptEntry>, at: usize, rows: usize) {
        match self.held.take() {
            Some(header) if header.end == at => entries.push(TranscriptEntry {
                start: header.start,
                body: at,
                end: at + rows,
            }),
            held => {
                self.held = held;
                self.cell(entries, at, 0, rows);
            }
        }
    }

    /// Record a header still held — the page ends, or what came next was not
    /// its output — as an entry of its own, a message's: nothing to pin.
    fn flush(&mut self, entries: &mut Vec<TranscriptEntry>) {
        if let Some(header) = self.held.take() {
            entries.push(TranscriptEntry {
                start: header.start,
                body: header.start,
                end: header.end,
            });
        }
    }
}

/// Where a transcript's rows go — the cache's compact rows or an agent
/// page's plain ones — so [`CellWriter`] serves both.
trait RowSink {
    /// How many rows are down so far.
    fn row_count(&self) -> usize;
    /// Append `lines`.
    fn push_rows(&mut self, lines: Vec<Line<'static>>);
    /// Append one row.
    fn push_row(&mut self, line: Line<'static>);
}

impl RowSink for CachedLines {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn push_rows(&mut self, lines: Vec<Line<'static>>) {
        self.extend(lines);
    }

    fn push_row(&mut self, line: Line<'static>) {
        self.extend([line]);
    }
}

impl RowSink for Vec<Line<'static>> {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn push_rows(&mut self, lines: Vec<Line<'static>>) {
        self.extend(lines);
    }

    fn push_row(&mut self, line: Line<'static>) {
        self.push(line);
    }
}

/// Appends a live tail's cells — each one's rows, the blank row after it,
/// and its entry through an [`EntryWalk`] — to a page.
struct CellWriter<'a, R: RowSink> {
    rows: &'a mut R,
    entries: &'a mut Vec<TranscriptEntry>,
    walk: EntryWalk,
}

impl<R: RowSink> CellWriter<'_, R> {
    /// A cell, its first `header` rows its header (0: none).
    fn cell(&mut self, lines: Vec<Line<'static>>, header: usize) {
        self.cell_after(0, lines, header);
    }

    /// A cell whose first `kept` rows are the page's last ones already:
    /// `lines` are the rows after them.
    fn cell_after(&mut self, kept: usize, lines: Vec<Line<'static>>, header: usize) {
        let at = self.rows.row_count() - kept;
        let rows = kept + lines.len();
        self.rows.push_rows(lines);
        self.walk.cell(self.entries, at, header, rows);
        self.rows.push_rows(vec![Line::default()]);
    }

    /// A `!` command's live output, joining the header committed above it.
    fn shell_output(&mut self, lines: Vec<Line<'static>>) {
        let (at, rows) = (self.rows.row_count(), lines.len());
        self.rows.push_rows(lines);
        self.walk.shell_output(self.entries, at, rows);
        self.rows.push_rows(vec![Line::default()]);
    }

    /// Done appending: a header still held is an entry of its own.
    fn finish(mut self) {
        self.walk.flush(self.entries);
    }
}

/// The Ctrl+O transcript of the **viewed agent's** session
/// (`docs/agent-tool.md`) with its entries, or `None` when no agent view is
/// up (the caller falls back to the main [`TranscriptCache`]). The banner
/// over the agent's items (tools expanded, exactly like the main walk) and
/// its live tail — the in-progress reply and the live tool queue. Built fresh
/// per draw: an agent transcript is bounded by one task's work, so the
/// incremental cache isn't warranted.
#[must_use]
pub fn agent_transcript(app: &App, width: u16) -> Option<TranscriptPage> {
    let run = app.viewed_agent()?;
    let mut page = TranscriptPage {
        lines: header_lines(app, width),
        entries: Vec::new(),
    };
    page.lines.push(Line::default());
    let chrome_rows = page.lines.len();
    let mut walk = EntryWalk::default();
    for item in &run.history {
        let at = page.lines.len();
        let item = transcript_item_into(&mut page.lines, item, width, app.path_display());
        walk.item(&mut page.entries, at, item.content, &item.cells);
    }
    let mut out = CellWriter {
        rows: &mut page.lines,
        entries: &mut page.entries,
        walk,
    };
    if let Some(text) = run.streaming.as_deref().filter(|text| !text.is_empty()) {
        out.cell(message_lines(Role::Assistant, text, width), 0);
    }
    // The agent's open thinking phase, whole — the main tail's order and its
    // reason: the pager has no row budget, unlike the strip's windowed block
    // (`docs/thinking-stream.md`, `docs/agent-view-streaming.md`).
    if let Some(text) = run.reasoning() {
        out.cell(reasoning_live_full_lines(text, width), 1);
    }
    write_live_calls(
        &mut out,
        &run.tool_queue,
        run.drafts(),
        width,
        app.path_display(),
    );
    // …and what the user typed into this agent while it works — the messages
    // waiting for its next round boundary **and** the Tab follow-up turns
    // waiting for it to settle. The main tail's rule (`docs/queue.md`): the
    // Ctrl+O view never hides a pending message, so both sets gate it or a
    // follow-up queued on its own would be invisible here. `queued_lines`
    // reads the viewed agent's own sets, so this is that agent's backlog, not
    // the session's.
    if !run.queued.is_empty() || !run.followups.is_empty() {
        out.cell(queued_lines(app, width), 0);
    }
    out.finish();
    if page.lines.len() == chrome_rows {
        page.lines.push(Line::from(Span::styled(
            TOOL_VIEW_EMPTY.to_string(),
            Style::new().fg(tool_dim_color()),
        )));
    }
    Some(page)
}

/// [`agent_transcript`]'s rows alone — what `/export` writes inside an agent
/// session view (`docs/export.md`).
#[must_use]
pub fn agent_transcript_lines(app: &App, width: u16) -> Option<Vec<Line<'static>>> {
    agent_transcript(app, width).map(|page| page.lines)
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
/// `(generation, width)` pin the **frozen prefix** exactly: the first `frozen_rows`
/// holds the banner chrome plus every committed item, rendered once
/// (`transcript_item_into`); each refresh truncates the volatile live tail
/// (in-progress reply, live tool queue, queued backlog) off the end and
/// re-renders just that. The Esc-Esc backtrack highlight — REVERSED rows
/// *inside* the frozen prefix — is applied as an in-place style diff
/// (`Self::restyle_selection`), never a re-render. A cheap `TranscriptSig`
/// short-circuits the refresh entirely while nothing changed, so a scroll
/// keypress is O(viewport). Rows live in compact text/style storage; only the
/// visible viewport expands into ratatui lines when the pager paints.
///
/// [`warm`]: Self::warm
#[derive(Default)]
pub struct TranscriptCache {
    /// Pins the frozen prefix: `(history generation, width, session cwd)` —
    /// any mismatch invalidates every rendered item (the cwd feeds the banner
    /// chrome above them). `None` until the first build.
    pub(super) key: Option<FrozenKey>,
    sig: Option<TranscriptSig>,
    /// The full transcript, compacted after rendering each item. The pager
    /// expands only its visible window; text and styles are stored once.
    rows: CachedLines,
    /// Materialized only by the explicit whole-transcript `lines` accessor.
    /// The interactive pager never fills this compatibility view.
    pub(super) expanded: Option<Vec<Line<'static>>>,
    /// Rows of banner chrome at the top of `rows` (the empty-transcript
    /// placeholder keys on "nothing beyond the banner").
    chrome_rows: usize,
    /// End of the frozen prefix in `rows`; the live tail is rebuilt above it.
    frozen_rows: usize,
    /// Per rendered history item: its row count (for selection offsets) and,
    /// for a user message, the reversible message-row span of the backtrack
    /// preview ([`transcript_item_into`]).
    items: Vec<RenderedItem>,
    /// The frozen rows currently carrying the backtrack preview's REVERSED
    /// styling, so a selection step can undo exactly what it applied.
    reversed: Option<Range<usize>>,
    pub(super) selection: Option<Range<usize>>,
    /// Every entry on `rows` — the frozen prefix's, then the live tail's —
    /// what ←/→ step between and the pin is read from
    /// (`docs/transcript-navigation.md`).
    entries: Vec<TranscriptEntry>,
    /// How many of `entries` belong to the frozen prefix; the tail's are
    /// rebuilt above them with it.
    frozen_entries: usize,
    /// The frozen walk's state: a `!` command's header held at the end of
    /// the prefix for an output that is still the live tail. The tail walks
    /// on from a copy, so every refresh sees it again.
    walk: EntryWalk,
    /// Counts the frozen prefix's resets ([`Self::epoch`]).
    epoch: u64,
    /// The next history item to hand out as a job ([`Self::warm_jobs`]):
    /// everything below it is in the prefix or out with a worker.
    requested: usize,
    /// The reply still streaming, as the pager last showed it
    /// ([`LiveReply`]) — `None` until a pager draws one.
    live: Option<LiveReply>,
    /// How many of that reply's rows — the ones right after the frozen
    /// prefix — are settled and stay on the page through the tail's next
    /// rebuild. Zero whenever the page is cut back to the prefix.
    live_kept: usize,
    /// Test-only: how many refreshes did any rebuild work — so a test can
    /// prove a scroll (unchanged signature) is a cache hit, not a rebuild.
    #[cfg(test)]
    pub(super) builds: usize,
    /// Test-only: how many history items were rendered, ever — so a test can
    /// prove the frozen prefix is reused, not re-rendered.
    #[cfg(test)]
    pub(super) item_renders: usize,
}

/// The reply still streaming, for the pager's live tail: its rows, each
/// source line rendered **once**.
///
/// The tail is rebuilt on every refresh, and rendering the reply whole there
/// put the grammar over every line so far on every frame — a scroll key in
/// the pager waited ~150 ms behind a 60 KB reply (`docs/input-latency.md`).
/// A [`StreamRender`] of the cache's own keeps the finished lines' rows, so
/// a refresh renders the lines that completed since the last.
///
/// It is only as good as the claim that the buffer it is shown is the one
/// it rendered, grown. That does not hold across a turn — a tool call
/// finalises the segment and another starts, a failed attempt is withdrawn
/// and made again — so the text the rows came from is kept, and a buffer
/// that does not start with it starts the render over: one comparison of
/// bytes, where a length or a counter could be fooled by a longer buffer.
struct LiveReply {
    /// The buffer the rows were rendered from, as last seen.
    text: String,
    /// The width they were rendered at.
    width: u16,
    render: StreamRender,
}

/// What [`LiveReply::rows`] answers a rebuild of the page's tail with.
struct LiveRows {
    /// How many of the reply's rows already on the page stay there.
    keep: usize,
    /// The rows that follow them.
    rows: Vec<Line<'static>>,
    /// How many of the reply's rows — the kept and the new together — are
    /// settled, and so may stay through the next rebuild.
    settled: usize,
}

impl LiveReply {
    fn new(width: u16) -> Self {
        Self {
            text: String::new(),
            width,
            render: StreamRender::new(),
        }
    }

    /// The reply's rows for the buffer as it is now — the batch render's
    /// ([`message_lines`]), by [`StreamRender::whole`]'s contract — past the
    /// `held` settled ones the page still has from the last rebuild. A buffer
    /// that is not the last one grown, or another width, keeps none of them.
    fn rows(&mut self, text: &str, width: u16, held: usize) -> LiveRows {
        let grown = width == self.width && text.starts_with(self.text.as_str());
        if !grown {
            *self = Self::new(width);
        }
        let keep = if grown { held } else { 0 };
        let (rows, settled) = self.render.whole_after(text, width, keep);
        // `self.text` is a prefix of `text`, so this is a char boundary.
        self.text.push_str(&text[self.text.len()..]);
        LiveRows {
            keep,
            rows,
            settled,
        }
    }
}

/// What pins [`TranscriptCache`]'s frozen prefix — see the struct docs.
#[derive(PartialEq, Eq)]
pub(super) struct FrozenKey {
    generation: u64,
    pub(super) width: u16,
    pub(super) cwd: Option<String>,
    /// The theme the rows were painted in — a `/theme` switch re-renders
    /// every frozen item, since their colours are baked into the rows
    /// (`docs/theme.md`).
    theme: crate::app::Theme,
}

/// One rendered history item's shape inside the frozen prefix.
struct RenderedItem {
    /// Rows this item occupies (message + stamp + spacer / expanded tool cell).
    pub(super) rows: usize,
    /// `Some(message-row count)` for a user message — the span the backtrack
    /// preview reverses; `None` otherwise.
    user_rows: Option<usize>,
}

/// How many history items may be out with a worker at once
/// ([`TranscriptCache::warm_jobs`]): enough that it never waits on the loop
/// for its next one, few enough that a resumed session's whole history is
/// not copied in one go.
pub const TRANSCRIPT_WARM_WINDOW: usize = 8;

/// One history item to render for the transcript **off the loop thread**
/// ([`TranscriptCache::warm_jobs`], `docs/input-latency.md`): the item, and
/// everything its rows depend on that a thread other than the loop's does
/// not have — the theme and the way a labelled link shows its target are
/// per-thread cells, so the job carries the loop's and renders under them.
pub struct TranscriptJob {
    epoch: u64,
    index: usize,
    item: HistoryItem,
    width: u16,
    paths: PathDisplay,
    theme: crate::app::Theme,
    links: crate::links::TargetDisplay,
}

impl TranscriptJob {
    /// The cache epoch this job was handed out in — a worker skips a job
    /// from an epoch already over ([`TranscriptCache::epoch`]).
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Render the item — the rows [`TranscriptCache`] would have rendered on
    /// the loop thread, already compacted.
    #[must_use]
    pub fn run(self) -> RenderedTranscriptItem {
        // Compact from the start, on the thread that renders them: the loop
        // is left a copy of some bytes, and the small strings a rendered row
        // is made of are freed by the thread that allocated them.
        let mut rows = CachedLines::default();
        let item = super::palette::with_theme(self.theme, || {
            crate::links::with_target_display(self.links, || {
                transcript_item_into(&mut rows, &self.item, self.width, &self.paths)
            })
        });
        RenderedTranscriptItem {
            epoch: self.epoch,
            index: self.index,
            rows,
            user_rows: item.user_rows,
            content: item.content,
            cells: item.cells,
        }
    }
}

/// A [`TranscriptJob`]'s result, for [`TranscriptCache::accept`].
pub struct RenderedTranscriptItem {
    epoch: u64,
    index: usize,
    /// The item's rows, spacer included, compacted.
    rows: CachedLines,
    user_rows: Option<usize>,
    content: usize,
    cells: ItemCells,
}

/// The cheap fingerprint of every input to a [`TranscriptCache`] refresh — see
/// the struct docs for why lengths suffice (append-only history, plus the
/// generation catching every non-append mutation; a same-length replace after
/// an interrupt-undo pop would fool lengths alone).
#[derive(PartialEq, Eq)]
struct TranscriptSig {
    generation: u64,
    /// The active theme — the rows' colours depend on it (`docs/theme.md`).
    theme: crate::app::Theme,
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
    /// The running call's revision ([`App::tool_revision`]): a terminal
    /// session's same-length redraw (`45%` → `46%`) or a refined header
    /// changes no length above (`docs/interactive-shell.md`).
    tool_revision: u64,
    /// The round's drafts' revision: a `write` streaming into its cell moves
    /// no length above, and the overlay follows it like a streaming tool's
    /// output (`docs/write-streaming.md`).
    drafts_revision: u64,
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
            theme: super::palette::active_theme(),
            width,
            history_len: app.history.len(),
            cwd_len: app.session.as_ref().map(|s| s.cwd.len()),
            streaming_len: app.streaming_text().map(str::len),
            reasoning_len: app.reasoning().map(str::len),
            tool_queue: queue
                .front()
                .map(|t| (queue.len(), t.status, t.output.len())),
            tool_revision: app.tool_revision(),
            drafts_revision: app.tool_drafts().revision(),
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

    /// Pre-render the frozen prefix **on this thread**, ahead of any overlay
    /// draw, so the pager finds every history item already rendered and
    /// pays only the live tail. When nothing changed this is a few integer
    /// compares. The loop itself hands the rendering to a worker instead
    /// ([`warm_jobs`](Self::warm_jobs)); this is for a caller with no worker
    /// to hand it to.
    ///
    /// A **width-only** mismatch is deliberately skipped: resize events arrive
    /// in bursts (a drag delivers dozens), and a full O(history) re-render per
    /// event would freeze the caller — the first overlay draw at the new
    /// width pays the rebuild instead.
    pub fn warm(&mut self, app: &App, width: u16) {
        let width_only_miss = self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
                && k.theme == super::palette::active_theme()
                && k.width != width
        });
        if width_only_miss {
            return;
        }
        if self.ensure_frozen(app, width) {
            self.expanded = None;
            // The tail was truncated off (or the prefix reset): the next
            // refresh must rebuild it even if the volatile signature matches.
            self.sig = None;
        }
    }

    /// [`warm`](Self::warm) without the rendering: the history items not yet
    /// in the frozen prefix, handed out as jobs for a worker thread, each
    /// once, at most [`TRANSCRIPT_WARM_WINDOW`] out at a time. The loop calls
    /// this where it used to call `warm` and gives every result to
    /// [`accept`](Self::accept), so a committed item's rows are rendered —
    /// grammar highlighting and all — while the loop goes on reading keys
    /// (`docs/input-latency.md`). Unlike `warm` it follows a width change at
    /// once: re-keying is a few rows of banner, and the old width's jobs are
    /// refused when they come back.
    pub fn warm_jobs(&mut self, app: &App, width: u16) -> Vec<TranscriptJob> {
        if !self.key_matches(app, width) {
            self.reset_frozen(app, width);
            self.expanded = None;
            self.sig = None;
        }
        // A reply that has ended leaves no rows to keep for the pager.
        if app.streaming_text().is_none_or(str::is_empty) && self.live.take().is_some() {
            self.cut_to_frozen();
            self.sig = None;
        }
        let start = self.requested.max(self.items.len());
        let end = app
            .history
            .len()
            .min(self.items.len() + TRANSCRIPT_WARM_WINDOW);
        self.requested = start.max(end);
        let (theme, links) = (
            super::palette::active_theme(),
            crate::links::target_display(),
        );
        (start..end)
            .filter_map(|index| {
                Some(TranscriptJob {
                    epoch: self.epoch,
                    index,
                    item: app.history.get(index)?.clone(),
                    width,
                    paths: app.path_display().clone(),
                    theme,
                    links,
                })
            })
            .collect()
    }

    /// Splice a [`TranscriptJob`]'s rows onto the frozen prefix. Refused —
    /// `false`, nothing changed — unless it is the very next item of the
    /// prefix as it stands: a result from before a reset (another width, a
    /// rewound history, another theme), or for an item a draw has rendered
    /// itself meanwhile, is dropped.
    pub fn accept(&mut self, done: RenderedTranscriptItem) -> bool {
        if done.epoch != self.epoch || done.index != self.items.len() {
            // Out of turn in this epoch: whatever was handed out past the
            // prefix's end will be refused the same way, so hand it out
            // again rather than wait on it for ever.
            if done.epoch == self.epoch && done.index > self.items.len() {
                self.requested = self.items.len();
            }
            return false;
        }
        // The live tail goes, as it does when a draw appends an item.
        self.cut_to_frozen();
        let rows = done.rows.len();
        self.walk.item(
            &mut self.entries,
            self.frozen_rows,
            done.content,
            &done.cells,
        );
        self.items.push(RenderedItem {
            rows,
            user_rows: done.user_rows,
        });
        self.frozen_rows += rows;
        self.rows.append(done.rows);
        self.frozen_entries = self.entries.len();
        self.expanded = None;
        self.sig = None;
        true
    }

    /// Test-only: how many rows were ever put down on the page — so a test
    /// can prove a refresh appended what changed, not the reply again.
    #[cfg(test)]
    pub(super) fn rows_appended(&self) -> usize {
        self.rows.appended
    }

    /// Counts the frozen prefix's resets. A job is of the epoch it was
    /// handed out in, and only the current epoch's results are accepted; a
    /// worker reads it to skip the jobs of an epoch already over.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether the frozen prefix was rendered for `app`'s history as it is,
    /// at `width`, in the active theme.
    fn key_matches(&self, app: &App, width: u16) -> bool {
        self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.width == width
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
                && k.theme == super::palette::active_theme()
        })
    }

    /// Start the frozen prefix over for `app` at `width`: the banner chrome
    /// and no item — a new epoch, so nothing rendered for the old one can be
    /// spliced into it.
    fn reset_frozen(&mut self, app: &App, width: u16) {
        self.key = Some(FrozenKey {
            generation: app.history_generation(),
            width,
            cwd: app.session.as_ref().map(|s| s.cwd.clone()),
            theme: super::palette::active_theme(),
        });
        // The header banner tops the transcript exactly as it tops the
        // inline conversation (docs/header.md) — the overlay mirrors the
        // real scrollback, so Ctrl+O never hides it.
        self.rows.clear();
        self.rows.extend(header_lines(app, width));
        self.rows.extend([Line::default()]);
        self.chrome_rows = self.rows.len();
        self.frozen_rows = self.chrome_rows;
        self.items.clear();
        self.reversed = None;
        self.entries.clear();
        self.frozen_entries = 0;
        self.walk = EntryWalk::default();
        self.epoch += 1;
        self.requested = 0;
        // The live reply's rows were another width's, or another theme's.
        self.live = None;
        self.live_kept = 0;
    }

    /// Cut the page back to the frozen prefix: the live tail goes, the
    /// streaming reply's kept rows with it.
    fn cut_to_frozen(&mut self) {
        self.rows.truncate(self.frozen_rows);
        self.entries.truncate(self.frozen_entries);
        self.live_kept = 0;
    }

    /// Bring the frozen prefix up to date with `app.history` at `width`:
    /// reset it when the [`FrozenKey`] mismatches, then render and append any
    /// items not yet cached. Returns whether anything changed; when it did,
    /// `rows` holds **only** chrome + frozen items (the live tail was
    /// truncated off) and the caller must rebuild the tail.
    fn ensure_frozen(&mut self, app: &App, width: u16) -> bool {
        if !self.key_matches(app, width) {
            self.reset_frozen(app, width);
        } else if self.items.len() == app.history.len() {
            return false;
        } else {
            self.cut_to_frozen();
        }
        // Append the not-yet-rendered items (all of them after a reset). The
        // generation guarantees the cached prefix is a prefix of `history`.
        for item in app.history.get(self.items.len()..).unwrap_or(&[]) {
            let item = transcript_item_into(&mut self.rows, item, width, app.path_display());
            self.walk.item(
                &mut self.entries,
                self.frozen_rows,
                item.content,
                &item.cells,
            );
            self.items.push(RenderedItem {
                rows: item.rows,
                user_rows: item.user_rows,
            });
            self.frozen_rows += item.rows;
            #[cfg(test)]
            {
                self.item_renders += 1;
            }
        }
        self.frozen_entries = self.entries.len();
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
                self.rows.set_reversed(old, false);
            }
            if let Some(new) = target.clone() {
                self.rows.set_reversed(new, true);
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
    /// nothing at all follows the banner. Each records its entry, the walk
    /// going on from the frozen prefix's — so a `!` command's running output
    /// joins the header committed above it.
    fn build_tail(&mut self, app: &App, width: u16) {
        // The reply so far: each of its lines rendered once, and the rows of
        // the finished ones left where the last rebuild put them
        // ([`LiveReply`]) — the page is cut back to those, not to the prefix.
        let reply = match app.streaming_text().filter(|text| !text.is_empty()) {
            Some(text) => {
                let live = self.live.get_or_insert_with(|| LiveReply::new(width));
                Some(live.rows(text, width, self.live_kept))
            }
            None => {
                self.live = None;
                None
            }
        };
        self.live_kept = reply.as_ref().map_or(0, |reply| reply.keep);
        self.rows.truncate(self.frozen_rows + self.live_kept);
        self.entries.truncate(self.frozen_entries);
        let mut out = CellWriter {
            rows: &mut self.rows,
            entries: &mut self.entries,
            walk: self.walk.clone(),
        };
        if let Some(reply) = reply {
            out.cell_after(reply.keep, reply.rows, 0);
            self.live_kept = reply.settled;
        }
        // An open thinking phase, whole (the pager has no row budget, unlike
        // the strip's windowed block) — docs/thinking-stream.md.
        if let Some(text) = app.reasoning() {
            out.cell(reasoning_live_full_lines(text, width), 1);
        }
        // The live agent group's members expand as their own `● Agent(…)`
        // cells — activity live — before the tool queue, mirroring the strip's
        // order (docs/agent-tool.md). Committed groups render from history.
        if let Some(live) = app.agent_group() {
            for id in &live.ids {
                if let Some(run) = app.agent(id) {
                    out.cell(
                        agent_cell_lines(&AgentCellView::of_run(run), width, app.path_display()),
                        AGENT_CELL_HEADER_ROWS,
                    );
                }
            }
        }
        write_live_calls(
            &mut out,
            app.tool_queue(),
            app.tool_drafts(),
            width,
            app.path_display(),
        );
        if !app.queued.is_empty() || !app.steered.is_empty() {
            out.cell(queued_lines(app, width), 0);
        }
        out.finish();
        if self.rows.len() == self.chrome_rows {
            self.rows.extend([Line::from(Span::styled(
                TOOL_VIEW_EMPTY.to_string(),
                Style::new().fg(tool_dim_color()),
            ))]);
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
        self.expanded = None;
        // The prefix first; the tail's rebuild then cuts the page back to
        // what it keeps (`build_tail`).
        self.ensure_frozen(app, width);
        self.restyle_selection(app);
        self.build_tail(app, width);
        self.sig = Some(sig);
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }

    /// The complete, owned transcript rows, rebuilding first only if stale.
    /// Call [`Self::render`] to paint a pager without materializing every row.
    pub fn lines(&mut self, app: &App, width: u16) -> &[Line<'static>] {
        self.refresh(app, width);
        self.expanded
            .get_or_insert_with(|| self.rows.owned_range(0..self.rows.len()))
    }

    /// Paint the pager from compact rows, expanding only its visible window.
    /// Warming and scrolling retain the full transcript without holding a
    /// `String` and a complete `Style` for each highlighted token.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, app: &App) {
        self.refresh(app, area.width);
        // A caller may previously have requested all owned rows with `lines`.
        // The pager needs only a window, so release that compatibility view.
        self.expanded = None;
        let count = self.rows.len();
        let body_rows = tool_view_body_rows(area.height);
        let scroll = app.tool_scroll.min(count.saturating_sub(body_rows));
        let end = scroll.saturating_add(body_rows).min(count);
        let mut visible = self.rows.range(scroll..end);
        if let Some(pin) = pinned_rows(&self.entries, scroll, body_rows) {
            pin_over(
                &mut visible,
                self.rows.range(pin.header),
                pin.cut,
                area.width,
            );
        }
        render_tool_view_window(area, buf, app, count, visible);
    }

    /// The cached row count (for the scroll clamp) — no borrow held.
    pub fn line_count(&mut self, app: &App, width: u16) -> usize {
        self.refresh(app, width);
        self.rows.len()
    }

    /// The cached transcript's entries, rebuilding first only if stale — what
    /// the draw resolves a ←/→ step against (`docs/transcript-navigation.md`).
    pub fn entries(&mut self, app: &App, width: u16) -> &[TranscriptEntry] {
        self.refresh(app, width);
        &self.entries
    }

    /// The cached backtrack-preview selection range, rebuilding first if stale.
    pub fn selection(&mut self, app: &App, width: u16) -> Option<Range<usize>> {
        self.refresh(app, width);
        self.selection.clone()
    }
}

/// Every live call of a conversation, expanded for the pager, each over a
/// blank row and recorded as an entry under its header: the queue's cells in
/// order — a queued previewed call with the whole file it streamed under its
/// `Waiting…`/`Running…` corner, a `!` command's output joining the header
/// committed above it — then each call still being generated, every line so
/// far (`docs/write-streaming.md`). The one walk the main transcript's tail
/// and an agent's share, so the two pagers show a streaming file alike.
fn write_live_calls<R: RowSink>(
    out: &mut CellWriter<'_, R>,
    queue: &VecDeque<ToolCall>,
    drafts: &crate::app::ToolDrafts,
    width: u16,
    paths: &PathDisplay,
) {
    let mut adopted = drafts.queued();
    for tool in queue {
        let draft = crate::app::previewed_display(&tool.name)
            .then(|| adopted.next())
            .flatten();
        match draft {
            Some(draft) if matches!(tool.status, ToolStatus::Waiting | ToolStatus::Running) => {
                let (lines, header) = draft_cell(draft, Some(tool), None, width, paths, true);
                out.cell(lines, header);
            }
            _ if tool.shell => out.shell_output(tool_full_cell(tool, width, paths).0),
            _ => {
                let (lines, header) = tool_full_cell(tool, width, paths);
                out.cell(lines, header);
            }
        }
    }
    for draft in drafts.previews() {
        let (lines, header) = draft_cell(draft, None, None, width, paths, true);
        out.cell(lines, header);
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

/// An overlay's title-row text: the page's spaced caps, then — inside an
/// agent session view — whose page it is, the way the roster names an agent
/// (`explore agent · Fetch the weather`). Nothing else on the page says it,
/// and the lead's page and an agent's look alike: the transcript pager and
/// both Ctrl+D pages wear it (`docs/context.md`,
/// `docs/transcript-navigation.md`).
pub(super) fn view_title(app: &App, title: &str) -> String {
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

/// A full-screen overlay's title row: `/ ` tiled across the width (a `/` on
/// every even column) with the spaced-caps `title` overlaid from the left
/// edge, all dim — codex's transcript overlay header, shared by the
/// transcript pager, the `/resume` picker and both Ctrl+D pages.
pub(super) fn overlay_header(title: &str, width: u16) -> Line<'static> {
    // Cut by display columns, not characters: the Ctrl+D title can carry an
    // agent's description, whatever the model wrote — CJK included — and a
    // cut at `width` characters is up to twice the row wide.
    let mut text = super::wrap::truncate_cols(&format!("/ {title}"), width as usize);
    // The tiling opens with a blank after the title, so it never glues a `/`
    // onto the title's last character — an agent's description can end on
    // either parity, where every spaced-caps title ends on the same one.
    let start = cols(&text);
    for col in start..width as usize {
        text.push(if (col - start).is_multiple_of(2) {
            ' '
        } else {
            '/'
        });
    }
    Line::from(Span::styled(text, Style::new().fg(tool_dim_color())))
}

/// The pager's bottom rule: a dim `─` separator carrying the scroll position
/// as a right-aligned ` {pct}% ` one dash in from the right edge — 0% at the
/// top, 100% at the bottom (or whenever everything fits) — codex's transcript
/// overlay bottom bar.
pub(super) fn tool_view_separator(width: u16, scroll: usize, max: usize) -> Line<'static> {
    labeled_separator(width, scroll, max, None)
}

/// [`tool_view_separator`] with a `lead` set into the rule's left end, one
/// dash in — the Ctrl+D view's `5 messages · ≈1.2k tokens` summary
/// (`docs/context.md`). The percentage keeps its place; a lead the rule
/// cannot hold beside it, a dash between them, is left out whole rather
/// than cut.
pub(super) fn labeled_separator(
    width: u16,
    scroll: usize,
    max: usize,
    lead: Option<&str>,
) -> Line<'static> {
    let pct = if max == 0 {
        100
    } else {
        (scroll.min(max) * 100 + max / 2) / max
    };
    rule_with_labels(width, lead, &format!(" {pct}% "))
}

/// A dim full-width `─` rule with `label` embedded right-aligned one dash in
/// from the edge — the bottom bar shared by the transcript pager (its scroll
/// percentage) and the `/resume` picker (its selection count).
pub(super) fn rule_with_label(width: u16, label: &str) -> Line<'static> {
    rule_with_labels(width, None, label)
}

/// [`rule_with_label`] with an optional `lead` label set in from the left
/// edge the way `label` is from the right ([`labeled_separator`]). Both are
/// placed a character a cell, so they are one column per character.
fn rule_with_labels(width: u16, lead: Option<&str>, label: &str) -> Line<'static> {
    let width = width as usize;
    let mut rule = vec!['─'; width];
    let start = width.saturating_sub(cols(label) + 1);
    for (i, ch) in label.chars().enumerate() {
        if let Some(cell) = rule.get_mut(start + i) {
            *cell = ch;
        }
    }
    if let Some(lead) = lead {
        let lead = format!(" {lead} ");
        // A dash before the lead, and at least one between it and `label`.
        if cols(&lead) + 2 <= start {
            for (i, ch) in lead.chars().enumerate() {
                if let Some(cell) = rule.get_mut(1 + i) {
                    *cell = ch;
                }
            }
        }
    }
    Line::from(Span::styled(
        rule.into_iter().collect::<String>(),
        Style::new().fg(tool_dim_color()),
    ))
}

/// Render the full-screen tool-output view — codex's Ctrl+T transcript pager:
/// the slash-tiled title row, then the scrolling conversation transcript
/// (messages + every tool call's full output), windowed by `App::tool_scroll`
/// (clamped so it can't run past the end) with `~` filler on the body rows
/// past its end, the header of the cell the window opens inside pinned over
/// its top rows (`pinned_rows`), then the percentage separator and the dim
/// key-hint rows. Pure — `term.rs` paints this onto the overlay.
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App, page: &TranscriptPage) {
    let lines = &page.lines;
    let body_rows = tool_view_body_rows(area.height);
    let scroll = app.tool_scroll.min(lines.len().saturating_sub(body_rows));
    let mut visible: Vec<Line<'_>> = lines.iter().skip(scroll).take(body_rows).cloned().collect();
    if let Some(pin) = pinned_rows(&page.entries, scroll, body_rows) {
        pin_over(
            &mut visible,
            lines[pin.header].to_vec(),
            pin.cut,
            area.width,
        );
    }
    render_tool_view_window(area, buf, app, lines.len(), visible);
}

/// The shared pager chrome and body painter. The compact cache supplies a
/// borrowed viewport; the standalone renderer supplies owned rows — each
/// with any header already pinned over it.
fn render_tool_view_window(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    line_count: usize,
    mut visible: Vec<Line<'_>>,
) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    // Inside an agent session view the title says whose transcript this is,
    // Ctrl+D's way: the lead's and an agent's look alike.
    Paragraph::new(overlay_header(
        &view_title(app, TOOL_VIEW_TITLE),
        area.width,
    ))
    .render(title_area, buf);

    // The caller builds rows at the full width, which this split preserves.
    let max = line_count.saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
    while (visible.len() as u16) < body_area.height {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(tool_view_separator(area.width, scroll, max)).render(sep_area, buf);

    let dim = Style::new().fg(tool_dim_color());
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
    // ←/→ step between entries — except in a preview, where they step the
    // highlighted message and the closing row says so.
    let keys = if app.backtrack.selected.is_some() {
        TOOL_VIEW_HINT_SCROLL_KEYS
    } else {
        TOOL_VIEW_HINT_KEYS
    };
    Paragraph::new(vec![
        Line::from(Span::styled(keys.to_string(), dim)),
        Line::from(Span::styled(closing.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

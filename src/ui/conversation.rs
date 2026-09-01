//! Whole-conversation rendering: the inline scrollback walk over `history` and
//! the tail a resize or an overlay return repaints from it.
//!
//! The repaint is content-anchored and reflows both directions (CLAUDE.md
//! invariant 3); the tail restores the startup banner over a short conversation
//! (`docs/header.md`).

use super::*;

/// Whether `item` is a `!` shell command's header message (`Role::Shell`).
/// Such an item gets **no** blank spacer after it: its tool's `⎿` output (or
/// the live `⎿ Running…` preview) sits flush below, forming one exec cell
/// (docs/shell-command.md). Shared by [`conversation_lines`] and
/// [`transcript_lines`] so the inline view and the Ctrl+O overlay agree.
pub(super) fn is_shell_header(item: &HistoryItem) -> bool {
    matches!(item, HistoryItem::Message(m) if m.role == Role::Shell)
}

/// The slice of `history` that has actually reached scrollback: everything
/// except the trailing cells of a **parallel MCP run still in flight**, whose
/// lines the commit path is holding until the run ends ([`held_run_len`],
/// `docs/mcp.md`) — the live strip speaks for them meanwhile. Every rebuild
/// renders *this*, so a repaint landing mid-run can't paint a line the commit
/// is about to write.
#[must_use]
pub fn committed_history<'a>(
    history: &'a [HistoryItem],
    queue: &VecDeque<ToolCall>,
) -> &'a [HistoryItem] {
    &history[..history.len() - super::tool::held_run_len(history, queue)]
}

/// Build the whole conversation as styled lines, mirroring how it was streamed
/// to scrollback: each message's wrapped lines (or each tool call's collapsed
/// peek), with a blank spacer after every item. Used to repaint after a resize
/// clears the screen, or when returning from the tool-output view.
#[must_use]
pub fn conversation_lines(history: &[HistoryItem], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut index = 0;
    while index < history.len() {
        // A **parallel MCP run** — consecutive cells of one announced batch,
        // all resolved ok — is one act, so it collapses to the single
        // `Called deepwiki 2 times (ctrl+o to expand)` line the commit path
        // writes ([`tool_commit_lines`], `docs/mcp.md`), spacer included.
        if let Some((len, servers)) = super::tool::mcp_run(&history[index..]) {
            lines.push(super::tool::mcp_called_line(&servers));
            lines.push(Line::default());
            index += len;
            continue;
        }
        let item = &history[index];
        index += 1;
        match item {
            // A task tool call renders NOTHING inline — no cell, no spacer:
            // the live checklist is its display, and the record expands only
            // in the Ctrl+O transcript (docs/task-tools.md).
            HistoryItem::TaskCall(_) => continue,
            // Hook-injected conversation text is cell-less inline too — the
            // Ctrl+O transcript is its record (docs/hooks.md).
            HistoryItem::HookNote(_) => continue,
            HistoryItem::Message(m) => lines.extend(message_lines(m.role, &m.text, width)),
            HistoryItem::Tool(t) => lines.extend(tool_lines(t, width)),
            HistoryItem::Summary(s) => lines.extend(summary_lines(s, width)),
            HistoryItem::Background(n) => lines.extend(background_notice_lines(n, width)),
            HistoryItem::AgentGroup(g) => lines.extend(agent_group_lines(g, width)),
            HistoryItem::AgentNotice(n) => lines.extend(agent_notice_lines(n, width)),
            HistoryItem::Compaction(c) => lines.extend(compaction_lines(c, width)),
            // Collapsed: only the `Thought for …` line reaches scrollback —
            // the chain-of-thought expands in Ctrl+O (docs/thinking-stream.md).
            HistoryItem::Reasoning(r) => lines.extend(reasoning_lines(r, width)),
        }
        // A picture the item carries — an image `read`'s result, a message's
        // Ctrl+V attachments — hangs below the cell, one blank row apart and
        // flush at the left margin (`docs/images.md`). Empty for everything
        // else, and for every session where images are off.
        lines.extend(super::image::image_block_lines(item, width));
        // Blank spacer after every item — except a shell command's header:
        // its cell stays flush ([`is_shell_header`]).
        if !is_shell_header(item) {
            lines.push(Line::default());
        }
    }
    lines
}

/// The last `max_rows` lines of the conversation — i.e. the tail that fits on
/// screen above the live region. After a width shrink ratatui clears the visible
/// screen (older lines survive in the terminal's own scrollback), so we only
/// need to repaint what was on screen; capping at `max_rows` also avoids
/// re-scrolling content the terminal already kept.
#[must_use]
pub fn repaint_lines(history: &[HistoryItem], width: u16, max_rows: usize) -> Vec<Line<'static>> {
    keep_last_rows(conversation_lines(history, width), max_rows)
}

/// The repaint tail for a mid-stream conversation rebuild
/// (`main.rs::repaint_conversation`): the finished history plus the rows of
/// the in-flight partial reply that were **already committed** to scrollback
/// ([`StreamRender::committed_rows`]), re-rendered in place. Repainting from
/// history alone blanks the partial until its next chunk arrives (the Ctrl+O
/// disappear-then-flicker bug). The rows still to come are deliberately *not*
/// included — the caller queues them right after via [`StreamRender::commit`]
/// (the standard `insert_before` pipeline), so rows that streamed while the
/// overlay was up reach scrollback exactly once, however many there are.
#[must_use]
pub fn repaint_tail(
    history: &[HistoryItem],
    streaming: Option<&str>,
    render: &mut StreamRender,
    width: u16,
    max_rows: usize,
) -> Vec<Line<'static>> {
    let mut lines = conversation_lines(history, width);
    if let Some(text) = streaming.filter(|text| !text.is_empty()) {
        lines.extend(render.committed_rows(text, width));
    }
    keep_last_rows(lines, max_rows)
}

/// The last `max_rows` of `lines` — the shared cap of [`repaint_lines`] and
/// [`repaint_tail`] (applied *after* the partial's rows join the tail, so the
/// budget always keeps the newest rows, like a screen would).
fn keep_last_rows(mut lines: Vec<Line<'static>>, max_rows: usize) -> Vec<Line<'static>> {
    if lines.len() > max_rows {
        lines = lines.split_off(lines.len() - max_rows);
    }
    lines
}

/// A rebuilt repaint tail with the header banner (docs/header.md) restored
/// above it: `banner`, a blank spacer, then `tail`. Every purge rebuild goes
/// through this — the purge dropped scrollback whole, so the banner
/// unconditionally tops the reconstructed conversation. (An ordinary overlay
/// return never rebuilds: the on-screen banner survives untouched and the
/// queued commits flush beneath it.)
#[must_use]
pub fn banner_tail(mut banner: Vec<Line<'static>>, tail: Vec<Line<'static>>) -> Vec<Line<'static>> {
    banner.push(Line::default());
    banner.extend(tail);
    banner
}

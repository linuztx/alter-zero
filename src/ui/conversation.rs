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

/// Build the whole conversation as styled lines, mirroring how it was streamed
/// to scrollback: each message's wrapped lines (or each tool call's collapsed
/// peek), with a blank spacer after every item. Used to repaint after a resize
/// clears the screen, or when returning from the tool-output view.
#[must_use]
pub fn conversation_lines(history: &[HistoryItem], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in history {
        match item {
            HistoryItem::Message(m) => lines.extend(message_lines(m.role, &m.text, width)),
            HistoryItem::Tool(t) => lines.extend(tool_lines(t, width)),
            HistoryItem::Summary(s) => lines.extend(summary_lines(s, width)),
            HistoryItem::Background(n) => lines.extend(background_notice_lines(n, width)),
            HistoryItem::AgentGroup(g) => lines.extend(agent_group_lines(g, width)),
            HistoryItem::AgentNotice(n) => lines.extend(agent_notice_lines(n, width)),
            HistoryItem::Compaction(c) => lines.extend(compaction_lines(c, width)),
        }
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
/// above it: `banner`, a blank spacer, then `tail`, re-capped to the last
/// `budget` rows. Both of `main.rs::repaint_conversation`'s rebuild modes go
/// through this. A `Purge` rebuild (resize, `/clear`) passes `usize::MAX` —
/// the banner unconditionally tops the freshly-purged scrollback. An
/// `InPlace` overlay return (Ctrl+O, `/resume`) passes the on-screen window
/// budget, so the rebuild reproduces the window exactly: the banner comes
/// back fully when the conversation is short (the bug this fixes — the
/// overwrite used to wipe it), only its bottom rows when it had partly
/// scrolled, and not at all once it scrolled wholly into the terminal's kept
/// scrollback (re-adding it there would duplicate it). Prepend-then-recap is
/// exact because `keep_last_rows` keeps suffixes:
/// `keep(banner + keep(x, n), n) == keep(banner + x, n)`.
#[must_use]
pub fn banner_tail(
    mut banner: Vec<Line<'static>>,
    tail: Vec<Line<'static>>,
    budget: usize,
) -> Vec<Line<'static>> {
    banner.push(Line::default());
    banner.extend(tail);
    keep_last_rows(banner, budget)
}

/// How many history rows fit above a `live_height`-row live region on a
/// `term_height`-row screen — the number of lines to repaint after a resize.
/// Saturates at 0 so a live region taller than the screen can never underflow.
/// The pure counterpart of the terminal calls in `main.rs::repaint_after_resize`.
#[must_use]
pub fn repaint_budget(term_height: u16, live_height: u16) -> usize {
    term_height.saturating_sub(live_height) as usize
}

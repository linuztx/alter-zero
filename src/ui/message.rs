//! Committed conversation lines: user/assistant/system/error messages, the
//! `/compact` marker cell, and the repaint helpers a resize drives.

use super::assistant::assistant_lines;
use super::theme::*;
use super::wrap::cols;
use super::*;

/// Build the styled, wrapped lines for one message.
///
/// The first line carries a coloured role bullet; continuation lines are
/// indented to align under it. Returned lines are `'static` (owned), so the
/// caller can hand them to `insert_before` without lifetime juggling.
#[must_use]
pub fn message_lines(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    let (bullet, color) = match role {
        Role::User => (USER_BULLET, USER_COLOR),
        Role::Assistant => (AI_BULLET, AI_COLOR),
        Role::Error => (ERROR_BULLET, ERROR_COLOR),
        Role::System => (SYSTEM_BULLET, SYSTEM_COLOR),
        Role::Shell => (SHELL_BULLET, SHELL_MODE_COLOR),
    };
    // Only the assistant's replies are markdown; user/shell/notice text stays
    // literal (a user pasting ``` must not be code-blocked, and the dark-bg
    // padding math below assumes plain wrapped lines). See `docs/markdown.md`.
    if role == Role::Assistant {
        return assistant_lines(text, width, bullet, color);
    }
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);

    // User messages get the dark full-width block; a shell command's header
    // (`! pwd`) shares it — the mock's "dark line, like a user message".
    let dark = matches!(role, Role::User | Role::Shell);
    let bg = if dark {
        Style::new().bg(USER_BG_COLOR)
    } else {
        Style::default()
    };
    let cw = content_width as usize;
    wrap_text(text, content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            // Pad to content_width *columns* (not chars) so the background fills
            // the full terminal row even when the line holds wide CJK/emoji.
            let padded = if dark {
                let pad = " ".repeat(cw.saturating_sub(cols(&line)));
                format!("{line}{pad}")
            } else {
                line
            };
            if i == 0 {
                Line::from(vec![
                    Span::styled(bullet.to_string(), bullet_style),
                    Span::raw(padded),
                ])
                .style(bg)
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(padded)]).style(bg)
            }
        })
        .collect()
}

/// The inline `/compact` marker cell: the one-line `● Context compacted`
/// notice in the system-notice dress (cyan bullet, literal text), carrying a
/// dim ` · {before} → {after} tokens` shrink clause when the marker recorded
/// the gauge (0/0 — an old rollout — hides it) and a dim ` · auto` tag for an
/// auto-triggered compaction. A single unwrapped line, the [`summary_lines`]
/// precedent. The summary body never shows inline — it expands in the Ctrl+O
/// transcript only ([`compaction_full_lines`]). See `docs/compact.md`.
#[must_use]
pub fn compaction_lines(compaction: &crate::app::Compaction, width: u16) -> Vec<Line<'static>> {
    let _ = width; // one unwrapped line, like summary_lines
    let bullet_style = Style::new().fg(SYSTEM_COLOR).add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut spans = vec![
        Span::styled(SYSTEM_BULLET.to_string(), bullet_style),
        Span::raw(COMPACTED_NOTICE.to_string()),
    ];
    if compaction.before > 0 || compaction.after > 0 {
        spans.push(Span::styled(
            format!(
                " · {} → {} tokens",
                format_token_count(usize::try_from(compaction.before).unwrap_or(usize::MAX)),
                format_token_count(usize::try_from(compaction.after).unwrap_or(usize::MAX)),
            ),
            dim,
        ));
    }
    if compaction.auto {
        spans.push(Span::styled(" · auto".to_string(), dim));
    }
    vec![Line::from(spans)]
}

/// The Ctrl+O transcript's expanded `/compact` cell: the marker line with the
/// model-written handoff summary wrapped dim + indented below it — what the
/// bridge will replay to the model, readable in place. An empty summary shows
/// just the marker. See `docs/compact.md`.
pub(super) fn compaction_full_lines(
    compaction: &crate::app::Compaction,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = compaction_lines(compaction, width);
    if compaction.summary.is_empty() {
        return lines;
    }
    let body_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    for row in wrap_text(&compaction.summary, body_width) {
        lines.push(Line::from(vec![
            Span::raw(INDENT.to_string()),
            Span::styled(row, dim),
        ]));
    }
    lines
}

/// The stamp footer under a **user** message in the transcript: a blank row,
/// then the dim timestamp right-aligned flush to `width`. Empty for an empty
/// timestamp (no clock injected). Only user messages get this — the other item
/// kinds record a stamp too but never display it. Called **only** from the
/// transcript builder — the inline view stays stamp-free.
pub(super) fn user_stamp_lines(timestamp: &str, width: u16) -> Vec<Line<'static>> {
    if timestamp.is_empty() {
        return Vec::new();
    }
    let pad = (width as usize).saturating_sub(cols(timestamp));
    vec![
        Line::default(),
        Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(timestamp.to_string(), Style::new().fg(TIMESTAMP_COLOR)),
        ]),
    ]
}

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
/// exact because [`keep_last_rows`] keeps suffixes:
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

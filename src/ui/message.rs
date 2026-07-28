//! Rendering one committed message: the role-bulleted user/assistant/system/
//! error lines, and the `/compact` marker cell (`docs/compact.md`).
//!
//! The walk over the whole history lives in
//! [`conversation`](super::conversation).

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
/// transcript only (`compaction_full_lines`). See `docs/compact.md`.
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

//! The live region's last row: the session footer and everything that takes
//! its slot (the Ctrl+R search line, the `!` shell hint, the backtrack hint),
//! plus the toast above the box and the queued-message rows.
//! See `docs/footer.md`, `docs/toast.md`, `docs/queue.md`.

use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

/// How many rows the queued messages occupy in the strip at `width`: the total
/// wrapped height of every queued message (each styled like a user message),
/// uncapped — the whole backlog shows, codex-style; 0 when the queue is empty.
/// [`live_height`] reserves this and [`render_live`] paints exactly this many —
/// the two must agree (both go through [`queued_lines`], so they can't drift).
/// `live_height`'s terminal-height clamp still bounds the region as a whole.
#[must_use]
pub fn queued_rows(app: &App, width: u16) -> u16 {
    // An agent session view shows the agent's world — the main session's
    // queued follow-ups stay off it (they re-appear on return).
    if app.agent_view.is_some() {
        return 0;
    }
    // Saturating: the queue is uncapped, and a plain `as` cast would silently
    // wrap a >65,535-row backlog into a tiny (wrong) height.
    queued_lines(app, width).len().min(usize::from(u16::MAX)) as u16
}

/// The styled lines for the queued follow-up messages: each rendered like a sent
/// user message ([`message_lines`] — the `❯ ` bullet, dark background, wrapped to
/// `width` minus the `QUEUED_INDENT` every row is inset by), concatenated —
/// every queued message shows (no display cap). A **blank row divides each
/// turn-batch** from the next, so Tab-opened follow-ups read as separate turns
/// from the first queue (`docs/queue.md`). Empty when the queue is empty.
#[must_use]
pub fn queued_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cols(QUEUED_INDENT) as u16);
    let mut lines = Vec::new();
    for (i, entry) in app.queued.iter().enumerate() {
        // A blank row divides each queued entry from the next, so Tab-opened
        // follow-ups (and standalone shell commands) read as separate turns.
        if i > 0 {
            lines.push(Line::default());
        }
        match entry {
            QueuedTurn::Messages { texts, .. } => {
                for msg in texts {
                    lines.extend(
                        message_lines(Role::User, msg, inner)
                            .into_iter()
                            .map(indent_queued_line),
                    );
                }
            }
            // A queued `!` command renders like the exec cell it becomes: the
            // red `! command` Role::Shell header (docs/shell-command.md).
            QueuedTurn::Shell(cmd) => lines.extend(
                message_lines(Role::Shell, cmd, inner)
                    .into_iter()
                    .map(indent_queued_line),
            ),
        }
    }
    lines
}

/// Prefix one queued-message row with the [`QUEUED_INDENT`], keeping the indent
/// *outside* the message's styling (the dark user-message block starts after it):
/// the line-level style is folded into each span so the rebuilt line — and with
/// it the indent — can stay unstyled.
fn indent_queued_line(line: Line<'static>) -> Line<'static> {
    let base = line.style;
    let mut spans = vec![Span::raw(QUEUED_INDENT)];
    spans.extend(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, base.patch(span.style))),
    );
    Line::from(spans)
}

/// Rows the session-context footer occupies under the box: one once `main.rs`
/// has injected the session info ([`crate::app::App::set_session_info`]) and
/// no band is open — the palette / `?` shortcuts band **displaces** the footer
/// (codex's popups and shortcut overlay take its row the same way) — else
/// zero. [`live_height`] adds this; [`render_live`] paints exactly this many —
/// the two must agree, like [`menu_rows`].
#[must_use]
pub fn footer_rows(app: &App, band_rows: u16) -> u16 {
    // The Ctrl+R search line takes the slot whenever a search is open — even
    // with no session info injected, unlike the ambient footer (no band can be
    // open during a search; see docs/history-search.md).
    if app.history_search.is_some() {
        return 1;
    }
    // The `!` shell-mode hint takes the slot the same way (no band can be open
    // in shell mode either; see docs/shell-command.md).
    if app.shell_mode {
        return 1;
    }
    // A primed backtrack's "esc again…" hint likewise (priming requires an
    // empty composer, so no band/search/shell can be open with it; see
    // docs/backtrack.md).
    if app.backtrack.primed {
        return 1;
    }
    // The roster selection's hint line likewise (its gate requires an empty
    // composer with no palette/picker open; see docs/agent-tool.md).
    if app.agent_selection().is_some() {
        return 1;
    }
    u16::from(app.session.is_some() && band_rows == 0)
}

/// The primed-backtrack footer line (codex's `esc_backtrack_hint`): the
/// `FOOTER_INDENT`, the `esc` key bold-cyan like the search-line hint keys,
/// then the dim ` again to edit previous message` label. Takes the footer
/// slot while [`crate::app::Backtrack::primed`]. See `docs/backtrack.md`.
#[must_use]
pub fn backtrack_hint_line() -> Line<'static> {
    Line::from(vec![
        Span::raw(FOOTER_INDENT),
        Span::styled(
            BACKTRACK_HINT_KEY,
            Style::new()
                .fg(SEARCH_QUERY_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(BACKTRACK_HINT_LABEL, Style::new().fg(FOOTER_COLOR)),
    ])
}

/// The footer's single line: the `FOOTER_INDENT`, then `{model} · {cwd}` —
/// every segment dim (codex's no-theme-colours status line, the separator dim
/// like its ` · `) — cut with a trailing `…` when it overflows `width`
/// (codex's `truncate_line_with_ellipsis_if_overflow`). A reasoning-capable
/// model carries its thinking mode right beside the name (`{model} {mode}`,
/// Ctrl+T cycles it — `docs/reasoning.md`). Empty when no session info has
/// been injected.
#[must_use]
pub fn footer_line(app: &App, width: u16) -> Line<'static> {
    let Some(session) = &app.session else {
        return Line::default();
    };
    let dim = Style::new().fg(FOOTER_COLOR);
    // The permission mode is pinned flush at the row's RIGHT edge — its own
    // zone, like the transcript separator's right-aligned percentage, not
    // another ` · ` segment — so however long the model/cwd/gauge chain
    // grows, the ellipsis truncation below eats the left content and never
    // the one segment with a safety meaning (docs/permissions.md). Its
    // columns (plus a gap) come off the chain's budget up front. Absent
    // while permissions are disabled: nothing asks, and a mode would be a
    // lie.
    let mode = app.permission_mode().map(|mode| mode.label());
    let reserved = mode.map_or(0, |label| cols(label) + FOOTER_MODE_GAP);
    let mut segments = vec![Span::styled(session.model.clone(), dim)];
    if let Some(thinking) = &app.thinking {
        segments.push(Span::styled(format!(" {}", thinking.mode.label()), dim));
    }
    segments.extend([
        Span::styled(FOOTER_SEPARATOR.to_string(), dim),
        Span::styled(session.cwd.clone(), dim),
    ]);
    // The context gauge — `{used}/{window} ({pct}%)` (e.g. `1.3k/160k
    // (0.8%)`) — whenever the active model's window is known, so the user
    // sees both the raw context size and auto-compact approaching
    // (docs/compact.md). Both counts are humanized by the status line's token
    // formatter; the share keeps one decimal.
    if let Some(window) = app.context_window() {
        let used = app.context_used();
        #[allow(clippy::cast_precision_loss)] // display only — one decimal
        let pct = used as f64 * 100.0 / window as f64;
        segments.push(Span::styled(FOOTER_SEPARATOR.to_string(), dim));
        segments.push(Span::styled(
            format!(
                "{}/{} ({pct:.1}%)",
                format_token_count(usize::try_from(used).unwrap_or(usize::MAX)),
                format_token_count(usize::try_from(window).unwrap_or(usize::MAX))
            ),
            dim,
        ));
    }
    // Running background shells append a `· {n} shell(s)` count — the ↓
    // manager's ambient reminder (docs/background.md). ↓ *focuses* that
    // segment: it lights up on cyan and waits for the Enter that opens the
    // manager band, while every other segment keeps its dim styling, so the
    // row loses none of its context.
    let shells = app.background().len();
    if shells > 0 {
        let plural = if shells == 1 { "" } else { "s" };
        let style = if app.background_focused() {
            Style::new().fg(FOOTER_FOCUS_FG).bg(FOOTER_FOCUS_BG)
        } else {
            dim
        };
        segments.push(Span::styled(FOOTER_SEPARATOR.to_string(), dim));
        segments.push(Span::styled(format!("{shells} shell{plural}"), style));
    }
    let mut budget = (width as usize).saturating_sub(cols(FOOTER_INDENT) + reserved);
    let mut spans = vec![Span::raw(FOOTER_INDENT)];
    if segments.iter().map(|s| cols(&s.content)).sum::<usize>() <= budget {
        spans.extend(segments);
    } else {
        // Overflow: keep whole leading segments while they fit, cut the first
        // that doesn't, and close with the ellipsis.
        budget = budget.saturating_sub(cols(STATUS_ELLIPSIS));
        for segment in segments {
            let w = cols(&segment.content);
            if w <= budget {
                budget -= w;
                spans.push(segment);
            } else {
                let cut = truncate_cols(&segment.content, budget);
                if !cut.is_empty() {
                    spans.push(Span::styled(cut, segment.style));
                }
                break;
            }
        }
        spans.push(Span::styled(STATUS_ELLIPSIS.to_string(), dim));
    }
    // Seat the mode flush against the right edge: pad the gap the reservation
    // held back (at least [`FOOTER_MODE_GAP`]), then the label, dim like the
    // rest of the row.
    if let Some(label) = mode {
        let used: usize = spans.iter().map(|s| cols(&s.content)).sum();
        let pad = (width as usize).saturating_sub(used + cols(label));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(label.to_string(), dim));
    }
    Line::from(spans)
}

/// How many rows the transient toast reserves above the box: 0 when none is
/// live, else 1 (a single row, truncated to the width). Added to the strip by
/// [`live_height`]/`live_layout`, between the queued messages and the box's
/// top rule. See `docs/toast.md`.
#[must_use]
pub fn toast_rows(app: &App) -> u16 {
    u16::from(app.toast().is_some())
}

/// The transient toast's single line: the `TOAST_INDENT` then the message,
/// dim for an info toast (`TOAST_COLOR`) or red for a failure
/// (`TOAST_ERROR_COLOR`), cut with a trailing `…` when it overflows `width`
/// (like the footer). Empty when no toast is live. See `docs/toast.md`.
#[must_use]
pub fn toast_line(app: &App, width: u16) -> Line<'static> {
    let Some(toast) = app.toast() else {
        return Line::default();
    };
    let color = match toast.kind {
        ToastKind::Info => TOAST_COLOR,
        ToastKind::Error => TOAST_ERROR_COLOR,
    };
    let style = Style::new().fg(color);
    let budget = (width as usize).saturating_sub(cols(TOAST_INDENT));
    let text = if cols(&toast.text) <= budget {
        toast.text.clone()
    } else {
        let mut cut = truncate_cols(&toast.text, budget.saturating_sub(cols(STATUS_ELLIPSIS)));
        cut.push_str(STATUS_ELLIPSIS);
        cut
    };
    Line::from(vec![Span::raw(TOAST_INDENT), Span::styled(text, style)])
}

/// The `!` shell-mode footer line: the `FOOTER_INDENT` then `Shell mode` in
/// red (`SHELL_MODE_COLOR`) — codex's `shell_mode_footer_line`. Shown in the
/// footer slot whenever the composer holds a `!command` (see
/// `docs/shell-command.md`).
#[must_use]
pub fn shell_mode_line() -> Line<'static> {
    Line::from(vec![
        Span::raw(FOOTER_INDENT),
        Span::styled(SHELL_MODE_LABEL, Style::new().fg(SHELL_MODE_COLOR)),
    ])
}

/// The Ctrl+R search's footer-slot line — codex's
/// `history_search_footer_line`: the dim `reverse-i-search: ` prompt behind
/// the `FOOTER_INDENT`, the query cyan, then per state the accept/cancel
/// hints (keys cyan **bold**, labels dim) or the red no-match notice. The
/// hardware cursor sits at the end of the query ([`cursor_position`]).
#[must_use]
pub fn search_line(search: &HistorySearch) -> Line<'static> {
    let dim = Style::new().fg(FOOTER_COLOR);
    let key = Style::new()
        .fg(SEARCH_QUERY_COLOR)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::raw(FOOTER_INDENT),
        Span::styled(SEARCH_PROMPT, dim),
        Span::styled(search.query.clone(), Style::new().fg(SEARCH_QUERY_COLOR)),
    ];
    match search.state {
        SearchState::Idle => {}
        SearchState::Match { .. } => {
            spans.push(Span::styled("  ", dim));
            spans.push(Span::styled("enter", key));
            spans.push(Span::styled(" accept", dim));
            spans.push(Span::styled(" · ", dim));
            spans.push(Span::styled("esc", key));
            spans.push(Span::styled(" cancel", dim));
        }
        SearchState::NoMatch => {
            spans.push(Span::styled(SEARCH_NO_MATCH, Style::new().fg(ERROR_COLOR)));
        }
    }
    Line::from(spans)
}

/// Format a working directory for the footer, codex-style
/// (`format_directory_display`): `~` for the home directory itself, `~/rel`
/// for paths under it (component-wise, so `/home/userx` is *not* under
/// `/home/user`), and the absolute path as-is otherwise — or when no home is
/// known. Pure: `main.rs` reads the environment and passes both paths in.
#[must_use]
pub fn display_cwd(cwd: &Path, home: Option<&Path>) -> String {
    if let Some(rel) = home.and_then(|home| cwd.strip_prefix(home).ok()) {
        if rel.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display());
    }
    cwd.display().to_string()
}

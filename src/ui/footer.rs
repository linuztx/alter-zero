//! The live region's last row: the session footer and everything that takes
//! its slot (the Ctrl+R search line, the `!` shell hint, the backtrack hint),
//! plus the toast above the box and the queued-message rows.
//! See `docs/footer.md`, `docs/toast.md`, `docs/queue.md`.

use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// How many rows the pending messages occupy in the strip at `width`: the total
/// wrapped height of every one of them (each styled like a user message),
/// uncapped — the whole backlog shows, codex-style; 0 when nothing is pending.
/// [`live_height`] reserves this and [`render_live`] paints exactly this many —
/// the two must agree (both go through [`queued_lines`], so they can't drift).
/// `live_height`'s terminal-height clamp still bounds the region as a whole.
#[must_use]
pub fn queued_rows(app: &App, width: u16) -> u16 {
    // Saturating: the queue is uncapped, and a plain `as` cast would silently
    // wrap a >65,535-row backlog into a tiny (wrong) height.
    with_queued_lines(app, width, |lines| {
        lines.len().min(usize::from(u16::MAX)) as u16
    })
}

/// The frame's memoized pending rows: `queued_lines`'s build, kept until
/// something it reads changes.
struct QueuedMemo {
    /// A fingerprint of every input [`build_queued_lines`] reads — the width,
    /// which conversation is on screen, and the exact text of both its pending
    /// sets. Content-derived rather than a mutation counter on purpose: the
    /// queues are plain fields several modules (and the whole test tree) write
    /// directly, so a counter would be one forgotten bump away from painting
    /// rows that are no longer there.
    signature: u64,
    lines: Vec<Line<'static>>,
}

thread_local! {
    /// One memo per thread: the loop draws on one, and each test thread gets
    /// its own (a fingerprint match means the *inputs* match, so a shared
    /// entry could only ever serve identical rows anyway).
    static QUEUED_MEMO: RefCell<Option<QueuedMemo>> = const { RefCell::new(None) };
    /// How many times the rows have actually been **built** on this thread —
    /// the memo's own test hook (see [`queued_builds`]).
    static QUEUED_BUILDS: Cell<u64> = const { Cell::new(0) };
}

/// How many times [`queued_lines`] has built its rows on this thread, as
/// opposed to answering from the memo. The property the memo exists for is a
/// *cost*, and a cost is only pinned by counting it.
#[cfg(test)]
#[must_use]
pub(crate) fn queued_builds() -> u64 {
    QUEUED_BUILDS.with(Cell::get)
}

/// Run `f` over the pending rows, building them only when the memo's
/// fingerprint has moved.
///
/// **Why this is memoized at all.** `queued_lines` is reached six or seven
/// times per draw — [`live_height`] and [`preview_budget`]'s row sums,
/// [`cursor_position`]'s seat, `render_live`'s layout, and its paint — and
/// each build word-wraps and styles the *whole* backlog. While a turn runs the
/// draw tick re-arms every 32 ms, so a growing queue re-rendered rows that had
/// not changed since the last keystroke about two hundred times a second.
/// Measured on a stalled turn with fifty ~530-character messages queued: **75
/// CPU ticks per five idle seconds against a 6-tick empty-queue baseline** —
/// 15% of a core spent on a screen holding still, which is what "pressing Tab
/// with a message again and again lags the TUI" was. One build now serves the
/// whole frame: **13 ticks** on the same workload, the queue's own cost down
/// from 69 to 7.
fn with_queued_lines<T>(app: &App, width: u16, f: impl FnOnce(&[Line<'static>]) -> T) -> T {
    let signature = queued_signature(app, width);
    let stale = QUEUED_MEMO.with(|memo| {
        memo.borrow()
            .as_ref()
            .is_none_or(|entry| entry.signature != signature)
    });
    if stale {
        // Built outside the cell's borrow: nothing in the builder may re-enter
        // it, and not holding it is how that stays true rather than a promise.
        let lines = build_queued_lines(app, width);
        QUEUED_BUILDS.with(|builds| builds.set(builds.get().saturating_add(1)));
        QUEUED_MEMO.with(|memo| *memo.borrow_mut() = Some(QueuedMemo { signature, lines }));
    }
    QUEUED_MEMO.with(|memo| {
        f(memo
            .borrow()
            .as_ref()
            .map_or(&[][..], |entry| entry.lines.as_slice()))
    })
}

/// The fingerprint the memo turns on: the width, the active theme, **which**
/// conversation's rows these are, and the exact texts of that conversation's
/// two pending sets — precisely what [`build_queued_lines`] reads and nothing
/// else.
fn queued_signature(app: &App, width: u16) -> u64 {
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    // The rows' colours come from the active theme, so a `/theme` switch
    // must rebuild them too (`docs/theme.md`).
    super::palette::active_theme().hash(&mut hasher);
    if let Some(agent) = app.viewed_agent() {
        // The branch itself is part of the key: the same texts render as a
        // different page depending on whose session is on screen.
        0u8.hash(&mut hasher);
        agent.queued.hash(&mut hasher);
        agent.followups.hash(&mut hasher);
    } else {
        1u8.hash(&mut hasher);
        app.steered.hash(&mut hasher);
        app.queued.hash(&mut hasher);
    }
    hasher.finish()
}

/// The styled lines for the messages waiting above the box: each rendered like
/// a sent user message ([`message_lines`] — the `❯ ` bullet, dark background,
/// wrapped to `width` minus the `QUEUED_INDENT` every row is inset by),
/// concatenated — every pending message shows (no display cap). A **blank row
/// divides each turn** from the next, so Tab-opened follow-ups read as separate
/// turns from what is going into the one running (`docs/queue.md`).
///
/// Whose messages depends on which conversation is on screen — the same rows
/// either way, because a pending message is a pending message:
///
/// - the **main** view shows what the running turn is about to read
///   ([`App::steered`], first — it happens next) over the follow-up turns
///   ([`App::queued`]);
/// - an **agent session view** shows that agent's own two sets — what its
///   running loop is about to read
///   ([`AgentRun::queued`](crate::agents::AgentRun::queued)) over its Tab
///   follow-up turns ([`AgentRun::followups`](crate::agents::AgentRun::followups))
///   — and nothing else: the view is the agent's world, and the main
///   session's rows re-appear on return.
///
/// Empty when nothing is pending.
#[must_use]
pub fn queued_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    with_queued_lines(app, width, <[Line<'static>]>::to_vec)
}

/// [`queued_lines`]' actual build — the one the memo caches.
fn build_queued_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cols(QUEUED_INDENT) as u16);
    let mut lines = Vec::new();
    // A blank row divides each pending turn from the next, so Tab-opened
    // follow-ups (and standalone shell commands) read as separate turns.
    let divide = |lines: &mut Vec<Line<'static>>| {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
    };
    let user_rows = |text: &str| {
        message_lines(Role::User, text, inner)
            .into_iter()
            .map(indent_queued_line)
    };
    if let Some(agent) = app.viewed_agent() {
        // The main branch's shape exactly: what the running loop reads next
        // leads as one undivided block, its Tab follow-up turns come after,
        // a blank row dividing each of them (`docs/queue.md`).
        for text in &agent.queued {
            lines.extend(user_rows(text));
        }
        for text in &agent.followups {
            divide(&mut lines);
            lines.extend(user_rows(text));
        }
        return lines;
    }
    // What the *running* turn is about to read comes first — it is what
    // happens next. They share one undivided block: the model reads them
    // together at its next round boundary.
    for text in &app.steered {
        lines.extend(user_rows(text));
    }
    for entry in &app.queued {
        divide(&mut lines);
        match entry {
            QueuedTurn::Messages { texts, .. } => {
                for msg in texts {
                    lines.extend(user_rows(msg));
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
                .fg(search_query_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(BACKTRACK_HINT_LABEL, Style::new().fg(footer_color())),
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
    let dim = Style::new().fg(footer_color());
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
    // The model segment names the conversation on screen: inside a subagent's
    // session view, a type pinned to another model shows that model — and no
    // thinking mode, which the launch dropped along with the session's model
    // (`docs/subagents.md`) — while an inheriting type keeps the session's
    // pair (`docs/agent-context-gauge.md`).
    let mut segments = match app.viewed_agent_model() {
        Some(pinned) => vec![Span::styled(pinned.to_string(), dim)],
        None => {
            let mut model = vec![Span::styled(session.model.clone(), dim)];
            if let Some(thinking) = &app.thinking {
                model.push(Span::styled(format!(" {}", thinking.mode.label()), dim));
            }
            model
        }
    };
    segments.extend([
        Span::styled(FOOTER_SEPARATOR.to_string(), dim),
        Span::styled(session.cwd.clone(), dim),
    ]);
    // The context gauge — `{used}/{window} ({pct}%)` (e.g. `1.3k/160k
    // (0.8%)`) — whenever the window of the conversation ON SCREEN is known,
    // so the user sees both the raw context size and auto-compact approaching
    // (docs/compact.md): the main session's pair, or — inside a subagent's
    // session view — that agent's own context against its model's window
    // (`App::context_gauge`, docs/agent-context-gauge.md; reading the lead's
    // pair there was the reported bug). Both counts are humanized by the
    // status line's token formatter; the share keeps one decimal.
    if let Some((used, window)) = app.context_gauge() {
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
            Style::new().fg(footer_focus_fg()).bg(footer_focus_bg())
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
/// dim for an info toast (`toast_color()`) or red for a failure
/// (`toast_error_color()`), cut with a trailing `…` when it overflows `width`
/// (like the footer). Empty when no toast is live. See `docs/toast.md`.
#[must_use]
pub fn toast_line(app: &App, width: u16) -> Line<'static> {
    let Some(toast) = app.toast() else {
        return Line::default();
    };
    let color = match toast.kind {
        ToastKind::Info => toast_color(),
        ToastKind::Error => toast_error_color(),
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
/// red (`shell_mode_color()`) — codex's `shell_mode_footer_line`. Shown in the
/// footer slot whenever the composer holds a `!command` (see
/// `docs/shell-command.md`).
#[must_use]
pub fn shell_mode_line() -> Line<'static> {
    Line::from(vec![
        Span::raw(FOOTER_INDENT),
        Span::styled(SHELL_MODE_LABEL, Style::new().fg(shell_mode_color())),
    ])
}

/// The Ctrl+R search's footer-slot line — codex's
/// `history_search_footer_line`: the dim `reverse-i-search: ` prompt behind
/// the `FOOTER_INDENT`, the query cyan, then per state the accept/cancel
/// hints (keys cyan **bold**, labels dim) or the red no-match notice. The
/// hardware cursor sits at the end of the query ([`cursor_position`]).
#[must_use]
pub fn search_line(search: &HistorySearch) -> Line<'static> {
    let dim = Style::new().fg(footer_color());
    let key = Style::new()
        .fg(search_query_color())
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::raw(FOOTER_INDENT),
        Span::styled(SEARCH_PROMPT, dim),
        Span::styled(search.query.clone(), Style::new().fg(search_query_color())),
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
            spans.push(Span::styled(
                SEARCH_NO_MATCH,
                Style::new().fg(error_color()),
            ));
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

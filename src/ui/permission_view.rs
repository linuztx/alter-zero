//! The inline tool-permission prompt: a modal that replaces the whole live
//! region while a `write`/`edit`/`bash` call waits on the user.
//! See `docs/permissions.md`.
//!
//! One builder ([`permission_lines`]) produces the rows; the renderer paints
//! them and [`permission_height`](super::layout::permission_height) reserves
//! exactly that many, so the two can never drift.

use crate::permission::{
    PermissionKind, PermissionRequest, command_scope, hints, options, question, title,
};

use super::agent::live_agent_group_lines;
use super::file_cell::numbered_body_lines;
use super::theme::*;
use super::wrap::{cols, truncate_cols, wrap_output};
use super::*;

/// A full-width rule in the input box's border colour — the prompt's frame.
fn rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_RULE.repeat(width as usize),
        Style::new().fg(BORDER_COLOR),
    ))
}

/// The dim dashed rule that frames a file change's numbered body.
fn body_rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        PERMISSION_BODY_RULE.repeat(width as usize),
        Style::new().fg(PERMISSION_BODY_RULE_COLOR),
    ))
}

/// A one-space-inset row of `text` in `color`, truncated to the width.
fn text_row(text: &str, color: Color, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(PERMISSION_INDENT))
        .max(1);
    Line::from(vec![
        Span::raw(PERMISSION_INDENT),
        Span::styled(truncate_cols(text, room), Style::new().fg(color)),
    ])
}

/// The title row: the coloured action, plus a dim ` · from the {type} agent`
/// when a subagent raised the request.
fn title_row(request: &PermissionRequest, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::raw(PERMISSION_INDENT),
        Span::styled(
            title(request.kind).to_string(),
            Style::new()
                .fg(PERMISSION_TITLE_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(agent) = &request.agent {
        let text = format!("{PERMISSION_AGENT_SEPARATOR}{agent}{PERMISSION_AGENT_SUFFIX}");
        let room = (width as usize)
            .saturating_sub(cols(PERMISSION_INDENT) + cols(title(request.kind)))
            .max(1);
        spans.push(Span::styled(
            truncate_cols(&text, room),
            Style::new().fg(PERMISSION_AGENT_COLOR),
        ));
    }
    Line::from(spans)
}

/// One option's rows: `❯ 1. Yes` when highlighted (the whole block in the
/// accent colour), `  1. Yes` otherwise — the label **word-wrapped** to the
/// width. A long remember rule (an exact command's whole text) used to be cut
/// with a `…`, hiding exactly the text being approved; it now wraps like the
/// command body (`wrap_output`, spaces preserved), the continuation rows
/// aligned under the label past the marker and number, only the first row
/// carrying the `❯`.
fn option_rows(index: usize, label: &str, selected: bool, width: u16) -> Vec<Line<'static>> {
    let marker = if selected {
        PERMISSION_MARKER.to_string()
    } else {
        " ".repeat(cols(PERMISSION_MARKER))
    };
    let number = format!("{}. ", index + 1);
    let room = (width as usize)
        .saturating_sub(cols(PERMISSION_INDENT) + cols(PERMISSION_MARKER) + cols(&number))
        .max(1);
    let style = if selected {
        Style::new().fg(PERMISSION_SELECTED_COLOR)
    } else {
        Style::default()
    };
    let mut rows = wrap_output(label, room as u16);
    if rows.is_empty() {
        rows.push(String::new());
    }
    // A pathological label (a kilobytes-long exact command) caps with the
    // familiar `…` instead of stacking the option block off the screen bottom
    // — the region clamps to the terminal and paints top-down, so an
    // unbounded wrap would push `3. No` and the hints out of reach.
    if rows.len() > PERMISSION_OPTION_MAX_ROWS {
        rows.truncate(PERMISSION_OPTION_MAX_ROWS);
        let last = rows.last_mut().expect("the cap is nonzero");
        *last = format!("{}…", truncate_cols(last, room.saturating_sub(1)));
    }
    let continuation = " ".repeat(cols(PERMISSION_MARKER) + cols(&number));
    rows.into_iter()
        .enumerate()
        .map(|(i, text)| {
            let lead = if i == 0 {
                format!("{marker}{number}")
            } else {
                continuation.clone()
            };
            Line::from(vec![
                Span::raw(PERMISSION_INDENT),
                Span::styled(lead, style),
                Span::styled(text, style),
            ])
        })
        .collect()
}

/// Per-option wrapped heights at `width` — the seat's map: shared with
/// [`cursor_position`](super::layout::cursor_position) so the `❯` row the
/// renderer paints and the row the cursor rests on can never drift. The
/// selection changes an option's colour, never its height. `project` is the
/// directory an MCP rule's row names ([`options`]).
pub(super) fn option_heights(
    request: &PermissionRequest,
    project: Option<&str>,
    width: u16,
) -> Vec<usize> {
    options(request, project)
        .iter()
        .enumerate()
        .map(|(i, label)| option_rows(i, label, false, width).len())
        .collect()
}

/// The hint row under the options — `{key}{label}` pairs joined by ` · `, keys
/// in the accent colour.
fn hint_row(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(PERMISSION_INDENT)];
    for (i, (key, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                PERMISSION_HINT_SEPARATOR,
                Style::new().fg(PERMISSION_HINT_TEXT_COLOR),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(PERMISSION_HINT_KEY_COLOR),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(PERMISSION_HINT_TEXT_COLOR),
        ));
    }
    Line::from(spans)
}

/// The width available to the amend field's text — the region minus the
/// inset and the `❯ ` prompt. Shared with [`cursor_position`].
pub(super) fn amend_field_width(width: u16) -> u16 {
    width
        .saturating_sub((cols(PERMISSION_INDENT) + cols(PERMISSION_MARKER)) as u16)
        .max(1)
}

/// Tab's amend field in place of the option rows: the composer's wrapped rows
/// behind a cyan `❯ ` prompt (continuations indented under it).
fn amend_rows(input: &TextArea, width: u16) -> Vec<Line<'static>> {
    let field = amend_field_width(width);
    input
        .display_rows(field)
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let prefix = if i == 0 {
                PERMISSION_MARKER.to_string()
            } else {
                " ".repeat(cols(PERMISSION_MARKER))
            };
            Line::from(vec![
                Span::raw(PERMISSION_INDENT),
                Span::styled(prefix, Style::new().fg(PERMISSION_SELECTED_COLOR)),
                Span::raw(text),
            ])
        })
        .collect()
}

/// The `bash` prompt's body: the command (word-wrapped, spaces preserved) over
/// the model's own description, both extra-indented.
fn command_rows(
    request: &PermissionRequest,
    width: u16,
    budget: usize,
) -> (Vec<Line<'static>>, usize) {
    let indent = PERMISSION_COMMAND_INDENT;
    let room = width.saturating_sub(cols(indent) as u16).max(1);
    let mut source: Vec<(String, Color)> = wrap_output(&request.target, room)
        .into_iter()
        .map(|row| (row, PERMISSION_TARGET_COLOR))
        .collect();
    source.extend(detail_rows(request, room));
    let hidden = source.len().saturating_sub(budget.max(1));
    let shown = source.len() - hidden;
    let lines = source
        .into_iter()
        .take(shown)
        .map(|(text, color)| {
            Line::from(vec![
                Span::raw(indent),
                Span::styled(text, Style::new().fg(color)),
            ])
        })
        .collect();
    (lines, hidden)
}

/// The dim one-line description under a `bash` command / an MCP call, wrapped
/// to `room` — empty when the request carries none.
fn detail_rows(request: &PermissionRequest, room: u16) -> Vec<(String, Color)> {
    request
        .detail
        .as_ref()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .map(|detail| {
            wrap_output(detail, room)
                .into_iter()
                .map(|row| (row, PERMISSION_DETAIL_COLOR))
                .collect()
        })
        .unwrap_or_default()
}

/// An MCP prompt's body (`docs/mcp.md`) — the `bash` body's shape, because it
/// answers the same question (*what exactly is about to run?*):
///
/// ```text
///   deepwiki - read_wiki_structure(repoName: "linuztx/flaredantic") (MCP)
///   Get a list of documentation topics for a GitHub repository.
/// ```
///
/// The call reads as the cell header will — the display label with the
/// arguments in the `key: "value"` form, in the model's own key order — and
/// the ` (MCP)` marker closes it dim, riding the call's last row when it fits
/// so the eye lands on the tool and not on the badge. The server's own
/// description follows, dim. (The framed pretty-printed JSON this replaced
/// spent five rows re-punctuating two arguments.)
fn mcp_rows(request: &PermissionRequest, width: u16, budget: usize) -> (Vec<Line<'static>>, usize) {
    let indent = PERMISSION_COMMAND_INDENT;
    let room = width.saturating_sub(cols(indent) as u16).max(1);
    let label = crate::permission::mcp_label(&request.target);
    let args = request.body.trim();
    let call = if args.is_empty() {
        label
    } else {
        format!("{label}({args})")
    };
    let dim = Style::new().fg(PERMISSION_DETAIL_COLOR);
    let mut rows: Vec<Vec<Span<'static>>> = wrap_output(&call, room)
        .into_iter()
        .map(|row| vec![Span::styled(row, Style::new().fg(PERMISSION_TARGET_COLOR))])
        .collect();
    let suffix = crate::mcp::MCP_DISPLAY_SUFFIX;
    let fits = rows.last().is_some_and(|last| {
        last.iter().map(|s| cols(&s.content)).sum::<usize>() + cols(suffix) <= room as usize
    });
    match rows.last_mut() {
        Some(last) if fits => last.push(Span::styled(suffix.to_string(), dim)),
        _ => rows.push(vec![Span::styled(suffix.trim_start().to_string(), dim)]),
    }
    rows.extend(
        detail_rows(request, room)
            .into_iter()
            .map(|(text, color)| vec![Span::styled(text, Style::new().fg(color))]),
    );
    let hidden = rows.len().saturating_sub(budget.max(1));
    let shown = rows.len() - hidden;
    let lines = rows
        .into_iter()
        .take(shown)
        .map(|spans| {
            let mut row = vec![Span::raw(indent)];
            row.extend(spans);
            Line::from(row)
        })
        .collect();
    (lines, hidden)
}

/// The context cells above the prompt as separable chunks, in priority order:
/// the round's live agent group (when a subagent asked), then one chunk per
/// queued call — the asked-about front call first, its batch siblings behind
/// it. [`context_lines`] joins them blank-separated and caps the tail.
fn context_chunks(app: &App, width: u16) -> Vec<Vec<Line<'static>>> {
    let mut chunks = Vec::new();
    let agents = live_agent_group_lines(app, width);
    if !agents.is_empty() {
        chunks.push(agents);
    }
    // An all-MCP batch is one act and reads as one line here too — the strip's
    // aggregated cell, drawn at rest (`docs/mcp.md`). A prompt raised while
    // three `⎿ Waiting…` copies of it stacked up said the same thing three
    // times, in the rows the question needed.
    if chunks.is_empty()
        && let Some(batch) = super::tool::mcp_batch_lines(app, None, width)
    {
        return vec![batch];
    }
    chunks.extend(app.tool_queue().iter().map(|tool| tool_lines(tool, width)));
    chunks
}

/// The dim summary row standing in for the sibling cells the cap collapsed —
/// each of them a `⎿ Waiting…` call, so the row wears their colour.
fn waiting_summary_row(hidden: usize) -> Line<'static> {
    Line::from(Span::styled(
        format!("… +{hidden} more waiting"),
        Style::new().fg(TOOL_WAITING_COLOR),
    ))
}

/// The live cells that sit **above** the prompt: the call being asked about
/// (and any batch siblings behind it), led by the round's live agent group
/// when there is one — at most `budget` rows.
///
/// A permission prompt must never be a box out of nowhere — it is a question
/// *about something on screen*, so the strip's context survives the modal even
/// though the rest of the live region (the status line, the composer, the
/// bands, the footer) gives way to it. Every queued call renders its ordinary
/// collapsed cell — Claude Code's look: the one under the prompt shows the
/// same dim `⎿ Waiting…` its batch siblings do (it genuinely *is* waiting —
/// the approve seam runs before its `ToolStart`, so nothing has started), and
/// a call that is truly executing (the main turn's, under a subagent's
/// request) keeps its running row. No pulse: the prompt is a still frame
/// ([`tool_lines`] renders a running bullet at rest, `docs/tool-pulse.md`).
///
/// The context must not crowd out the prompt itself, though: a model's big
/// parallel batch queues a screenful of `⎿ Waiting…` siblings, which used to
/// eat the whole terminal — the body's budget saturated to zero (the prompt
/// showed no content at all) and the options ran off the screen bottom. So
/// the cells are kept **whole, in queue order, while they fit the budget**,
/// and the excess collapses into the dim [`waiting_summary_row`]. The first
/// chunk — the agent tree that asked, else the asked-about call itself — is
/// never dropped: the prompt stays a question about something on screen.
///
/// Empty when nothing raised the prompt on screen (a resumed session, the
/// dummy's scripted turn), so the prompt simply opens with its own rule.
fn context_lines(app: &App, width: u16, budget: usize) -> Vec<Line<'static>> {
    let chunks = context_chunks(app, width);
    if chunks.is_empty() {
        return Vec::new();
    }
    // Rows the first `keep` chunks occupy: their cells, the blank separators
    // between them, and — when anything is left over — the blank + summary
    // row standing in for the rest.
    let rows_for = |keep: usize| -> usize {
        let cells: usize = chunks[..keep].iter().map(Vec::len).sum();
        let summary = if keep < chunks.len() { 2 } else { 0 };
        cells + keep.saturating_sub(1) + summary
    };
    let mut keep = chunks.len();
    while keep > 1 && rows_for(keep) > budget {
        keep -= 1;
    }
    let hidden = chunks.len() - keep;
    let mut out = Vec::new();
    for chunk in chunks.into_iter().take(keep) {
        if !out.is_empty() {
            out.push(Line::default()); // blank row between cells
        }
        out.extend(chunk);
    }
    if hidden > 0 {
        out.push(Line::default());
        out.push(waiting_summary_row(hidden));
    }
    out
}

/// The dim `… +N lines` tail appended when the body did not fit the terminal.
fn more_row(hidden: usize, width: u16) -> Line<'static> {
    text_row(
        &format!("… +{hidden} line{}", if hidden == 1 { "" } else { "s" }),
        TOOL_DIM_COLOR,
        width,
    )
}

/// Every row of the open permission prompt, top rule to bottom rule, at
/// `width` on a `term_height`-row terminal.
///
/// The body — a `write`'s numbered contents, an `edit`'s numbered diff hunks,
/// a `bash` command and its description — is shown **whole**: the point of
/// the prompt is that you read what you are approving, all of it. The prompt
/// is a framed view like the menus and pickers (`docs/view-flow.md`): a page
/// taller than the terminal is painted bottom-anchored — the question, the
/// options and the hints close the page, so the anchor keeps them on screen
/// — and the skipped top flows into the terminal's real scrollback, where
/// the terminal's own scrolling reads the whole diff. (The retired
/// cap-and-pad layout hid the body's middle behind a `… +N lines` tail; the
/// tail survives only past `PERMISSION_BODY_MAX_ROWS`, the safety ceiling
/// for a pathological body rebuilt every draw tick.)
///
/// The context cells above the frame — the asked-about call, a live agent
/// tree — are kept only while the **whole page fits** the terminal: a live
/// tree's counters tick, and a flowed row is frozen in scrollback, so
/// ticking content must never flow. When even the collapsed context cannot
/// make the page fit, it drops whole and only the static frame flows.
///
/// The builder is a **fixpoint at its own height**:
/// [`permission_height`](super::layout::permission_height) sizes the region
/// from the terminal, [`render_permission`] rebuilds from the sized region,
/// and both produce the same rows — the keep-context decision re-derives
/// identically at either height (pinned by
/// `the_reserved_height_equals_the_painted_rows`).
///
/// Returns an empty vec when no prompt is open, so the callers can treat "no
/// prompt" and "no rows" alike.
#[must_use]
pub fn permission_lines(app: &App, width: u16, term_height: u16) -> Vec<Line<'static>> {
    let Some(prompt) = app.permission() else {
        return Vec::new();
    };
    let request = &prompt.request;
    let file_change = matches!(request.kind, PermissionKind::Write | PermissionKind::Edit);

    // Everything below the body, built FIRST so the context budget can see
    // its height: the standing notice (commands only), the question, the
    // options — or Tab's amend field, which is as tall as the feedback typed
    // — the hint row, and the closing frame.
    let mut below = Vec::new();
    if request.kind == PermissionKind::Bash {
        below.push(Line::default());
        below.push(text_row(PERMISSION_NOTICE, PERMISSION_NOTICE_COLOR, width));
        below.push(Line::default());
    }
    if request.kind == PermissionKind::Mcp {
        below.push(Line::default());
    }
    below.push(text_row(&question(request), Color::Reset, width));
    if prompt.amend {
        below.extend(amend_rows(&app.input, width));
        below.push(Line::default());
        below.push(hint_row(PERMISSION_AMEND_HINTS));
    } else {
        for (i, label) in options(request, app.project_dir()).iter().enumerate() {
            below.extend(option_rows(i, label, i == prompt.selected, width));
        }
        below.push(Line::default());
        below.push(hint_row(&hints(request)));
    }
    let below_len = below.len();

    // The prompt's own frame, rule to rule, with the body whole (up to the
    // `PERMISSION_BODY_MAX_ROWS` ceiling). A framed body (a file change's
    // numbered rows) carries its two dashed rules; a command's — and an MCP
    // call's, which reads like one — does not.
    let framed = file_change;
    let framing = if framed { 2 } else { 0 };
    let mut frame = vec![rule(width), Line::default(), title_row(request, width)];
    if file_change {
        frame.push(text_row(&request.target, PERMISSION_TARGET_COLOR, width));
    } else {
        // A command — and an MCP call, whose body names the tool the same way
        // (`docs/mcp.md`) — gaps here instead: its target *is* the body.
        frame.push(Line::default());
    }
    let (body, hidden) = if file_change {
        numbered_body_lines(
            &request.body,
            file_lang(&request.target),
            request.kind == PermissionKind::Edit,
            cols(PERMISSION_INDENT),
            width,
            PERMISSION_BODY_MAX_ROWS,
        )
    } else if request.kind == PermissionKind::Mcp {
        mcp_rows(request, width, PERMISSION_BODY_MAX_ROWS)
    } else {
        command_rows(request, width, PERMISSION_BODY_MAX_ROWS)
    };
    if framed {
        frame.push(body_rule(width));
    }
    frame.extend(body);
    if hidden > 0 {
        frame.push(more_row(hidden, width));
    }
    if framed {
        frame.push(body_rule(width));
    }
    frame.extend(below);
    frame.push(Line::default());
    frame.push(rule(width));

    // The live cells that raised this — kept visible above the modal so the
    // question reads as being *about* something on screen. Their budget is
    // whatever the terminal has left once the prompt's fixed rows AND a floor
    // for the body are set aside — the head is the 5 rows the frame opens
    // with (the blank after the cells, the rule, its gap, the title, the
    // target/gap row) — so a big parallel batch collapses its excess
    // `⎿ Waiting…` siblings into the summary row instead of squeezing the
    // body out (see [`context_lines`]).
    let context_budget = usize::from(term_height).saturating_sub(
        5 + below_len + framing + 2 /* the gap + rule */ + body_reserve(request, file_change),
    );
    let budgeted = context_lines(app, width, context_budget);
    let context = if budgeted.is_empty() {
        Vec::new()
    } else if budgeted.len() + 1 + frame.len() <= usize::from(term_height) {
        // The page fits with the (possibly collapsed) context above it.
        budgeted
    } else if context_is_stable(app) {
        // The page flows. A queued cell is static text while the prompt
        // blocks — nothing has started, the approve seam runs *before*
        // `ToolStart` — so it rides the flow into scrollback safely, and
        // dropping it instead is pure loss: the prompt becomes a box out of
        // nowhere (the reported bug). It rides **whole**, uncollapsed: there
        // is no screenful left to compete for once the page flows, so hiding
        // siblings behind a summary row would lose them for nothing. Building
        // it without the budget also keeps the page independent of
        // `term_height` here, which is what keeps the builder a fixpoint at
        // the region's own height (`docs/view-flow.md`).
        context_lines(app, width, usize::MAX)
    } else {
        // …unless it **ticks**: a live agent group's bullet breathes at the
        // frame pulse and its counters advance as the agents work. A flowed
        // row is frozen in scrollback, so ticking content would either go
        // stale there or re-sign the flow into a purge rebuild per tick. It
        // gives way, and only the static frame flows.
        Vec::new()
    };
    if context.is_empty() {
        return frame;
    }
    let mut out = context;
    out.push(Line::default());
    out.extend(frame);
    out
}

/// Whether the context cells above the prompt are **static** for as long as
/// it is open — the test for whether they may ride a flowing page into the
/// terminal's frozen scrollback (`docs/view-flow.md`).
///
/// A queued call is: the approve seam runs before its `ToolStart`, so a
/// `⎿ Waiting…` cell cannot change while the answer is pending. Two things
/// are not, and both are about something still *moving*: a **live agent
/// group** (its bullet breathes at the frame pulse, its `{n} tool uses ·
/// {tokens} tokens` counters advance) and a **running call** — the main
/// turn's own, under a subagent's request — whose streamed output grows the
/// cell's peek. Either one is enough to hold the whole context back.
fn context_is_stable(app: &App) -> bool {
    app.agent_group().is_none()
        && app
            .tool_queue()
            .iter()
            .all(|tool| tool.status != ToolStatus::Running)
}

/// The body rows the context cap must leave free: the body's own natural
/// height when it is short, else the [`PERMISSION_MIN_BODY_ROWS`] floor — so
/// a big batch's siblings can take everything a short body doesn't need, but
/// can never squeeze a tall body below its guaranteed peek. Counts *source*
/// rows like [`body_fits`] (wrapping only ever makes the shown body cap
/// earlier, never pushes the options off).
fn body_reserve(request: &PermissionRequest, file_change: bool) -> usize {
    let natural = if file_change {
        request.body.lines().count()
    } else {
        // A command's target is its body; an MCP call's is its one call row
        // (plus the ` (MCP)` marker's, when it doesn't fit beside it).
        request.target.lines().count()
            + request
                .detail
                .as_ref()
                .map_or(0, |d| d.trim().lines().count())
    };
    natural.min(PERMISSION_MIN_BODY_ROWS)
}

/// The highlight language for the preview — the target path's extension.
fn file_lang(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
}

/// Paint the open permission prompt over the whole live region —
/// bottom-anchored, so a page taller than the region keeps its tail (the
/// question, the options, the hints, the closing rule) on screen while the
/// skipped top flows into scrollback (`docs/view-flow.md`). Pure —
/// [`render_live`] calls this in place of the composer. The region is sized
/// from the same builder
/// ([`permission_height`](super::layout::permission_height), the fixpoint),
/// and the conversation above a fitting prompt is the *real* screen — the
/// prompt grew by scrolling like any other region, so what the user just
/// read stays put (`docs/permissions.md`).
pub fn render_permission(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, permission_lines(app, area.width, area.height));
}

/// The `bash` "don't ask again" label — the stored rule as the option showed
/// it (`{prefix} *`, or the exact command) — for the boundary's toast after an
/// [`crate::permission::PermissionDecision::ApproveAlways`].
#[must_use]
pub fn permission_remember_label(request: &PermissionRequest) -> String {
    match request.kind {
        PermissionKind::Bash => command_scope(&request.target).display(),
        // The MCP rule is the exact wire name (`docs/mcp.md`).
        PermissionKind::Mcp => request.target.clone(),
        _ => "all edits".to_string(),
    }
}

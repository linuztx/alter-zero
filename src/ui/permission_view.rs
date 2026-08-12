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
/// selection changes an option's colour, never its height.
pub(super) fn option_heights(request: &PermissionRequest, width: u16) -> Vec<usize> {
    options(request)
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
    if let Some(detail) = request
        .detail
        .as_ref()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
    {
        source.extend(
            wrap_output(detail, room)
                .into_iter()
                .map(|row| (row, PERMISSION_DETAIL_COLOR)),
        );
    }
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
/// a `bash` command and its description — is shown **whole**: the point of the
/// prompt is that you read what you are approving. It is capped only when the
/// prompt would not otherwise fit, and then by exactly enough that the
/// question, the options, and the hint row stay on screen — the budget is
/// whatever the terminal has left once the rows *around* the body are built,
/// so it stays right as those rows change (Tab's amend field is taller than
/// three options) — with a `… +N lines` tail saying what was left out.
///
/// A capped prompt is padded to fill `term_height` exactly. That is what keeps
/// [`permission_height`](super::layout::permission_height) — which sizes the
/// region from the *terminal* — and [`render_permission`] — which only ever
/// sees the sized *region* — in agreement: at both heights the builder is a
/// fixpoint, so the rows reserved are the rows painted. The padding goes
/// **above** the question, so the question/options/hint block keeps a fixed
/// seat against the bottom rule however tall the body is — which is what lets
/// [`cursor_position`](super::layout::cursor_position) find the highlighted
/// option (and the amend field) from that edge, `PERMISSION_TAIL_ROWS` up,
/// without re-deriving the body.
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

    // Everything below the body, built FIRST so both budgets can see its
    // height: the standing notice (commands only), the question, the options —
    // or Tab's amend field, which is as tall as the feedback typed — the hint
    // row, and the closing frame.
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
        for (i, label) in options(request).iter().enumerate() {
            below.extend(option_rows(i, label, i == prompt.selected, width));
        }
        below.push(Line::default());
        below.push(hint_row(&hints(request)));
    }

    // The live cells that raised this — kept visible above the modal so the
    // question reads as being *about* something on screen — then the frame, the
    // title, and (for a file change) the path it targets; a `bash` prompt gaps
    // instead, its command being the body. The cells get whatever the terminal
    // has left once the prompt's fixed rows AND a floor for the body are set
    // aside — the head is the 5 rows this function adds around them (the blank
    // after the cells, the rule, its gap, the title, the target/gap row) — so
    // a big parallel batch collapses its excess `⎿ Waiting…` siblings into the
    // summary row instead of squeezing the body out (see [`context_lines`]).
    // A framed body (a file change's numbered rows, an MCP call's arguments
    // JSON) carries its two dashed rules.
    let framed = file_change || request.kind == PermissionKind::Mcp;
    let framing = if framed { 2 } else { 0 };
    let context_budget = usize::from(term_height).saturating_sub(
        5 + below.len() + framing + 2 /* the gap + rule */ + body_reserve(request, file_change),
    );
    let mut out = context_lines(app, width, context_budget);
    if !out.is_empty() {
        out.push(Line::default());
    }
    out.extend([rule(width), Line::default(), title_row(request, width)]);
    if file_change {
        out.push(text_row(&request.target, PERMISSION_TARGET_COLOR, width));
    } else if request.kind == PermissionKind::Mcp {
        // The display form under the title — what the user knows the tool
        // as; the wire name is in option 2's rule (`docs/mcp.md`).
        out.push(text_row(
            &crate::mcp::display_from_wire(&request.target)
                .unwrap_or_else(|| request.target.clone()),
            PERMISSION_TARGET_COLOR,
            width,
        ));
    } else {
        out.push(Line::default());
    }

    // The body's row budget: whatever the terminal has left once those rows
    // (and, for a file change, its two dashed rules, plus the trailing gap and
    // rule) are accounted for — at least the reserve the context cap held
    // back, and everything the (possibly shorter) context did not use. Too
    // small for even one numbered row beside the `… +N lines` tail means the
    // terminal is too short for a preview at all — the body and its rules
    // drop out entirely rather than pushing the options off screen (the
    // numbered builder always emits its first row, so handing it a zero
    // budget would overflow the terminal by the tail's row).
    let budget = usize::from(term_height).saturating_sub(
        out.len() + below.len() + framing + 2, /* the gap + rule */
    );

    let mut capped = false;
    let body_rows = if budget == 0 {
        None
    } else if file_change {
        let fits = body_fits(&request.body, budget);
        if !fits && budget < 2 {
            None
        } else {
            Some(numbered_body_lines(
                &request.body,
                file_lang(&request.target),
                request.kind == PermissionKind::Edit,
                cols(PERMISSION_INDENT),
                width,
                if fits {
                    budget
                } else {
                    budget - 1 // room for the `… +N lines` tail
                },
            ))
        }
    } else if request.kind == PermissionKind::Mcp {
        // The arguments JSON, framed like a file body (the `╌` rules) — the
        // exact payload the server receives is what is being approved.
        (!request.body.trim().is_empty()).then(|| mcp_body_rows(&request.body, width, budget))
    } else {
        Some(command_rows(request, width, budget))
    };
    if let Some((body, hidden)) = body_rows {
        capped = hidden > 0;
        if framed {
            out.push(body_rule(width));
        }
        out.extend(body);
        if hidden > 0 {
            out.push(more_row(hidden, width));
        }
        if framed {
            out.push(body_rule(width));
        }
    }

    // A capped body means the prompt is meant to fill the terminal: pad the
    // shortfall (whole source rows can leave a row or two unused) so the count
    // is exactly `term_height` and the builder is a fixpoint — see above. The
    // padding goes here, above the question, so `below` stays flush against the
    // closing rule and the cursor can be seated from that edge.
    if capped {
        let target = usize::from(term_height).saturating_sub(below.len() + 2); // + blank + rule
        while out.len() < target {
            out.push(Line::default());
        }
    }
    out.extend(below);
    out.push(Line::default());
    out.push(rule(width));
    out
}

/// A cheap "does the body fit" check so a body that exactly fills the budget
/// isn't shortened to make room for a `… +0 lines` tail that isn't needed. It
/// counts *source* rows, an under-estimate of display rows for wrapped
/// content — which only ever errs toward reserving the tail row.
fn body_fits(body: &str, budget: usize) -> bool {
    body.lines().count() <= budget
}

/// The body rows the context cap must leave free: the body's own natural
/// height when it is short, else the [`PERMISSION_MIN_BODY_ROWS`] floor — so
/// a big batch's siblings can take everything a short body doesn't need, but
/// can never squeeze a tall body below its guaranteed peek. Counts *source*
/// rows like [`body_fits`] (wrapping only ever makes the shown body cap
/// earlier, never pushes the options off).
fn body_reserve(request: &PermissionRequest, file_change: bool) -> usize {
    let natural = if file_change || request.kind == PermissionKind::Mcp {
        request.body.lines().count()
    } else {
        request.target.lines().count()
            + request
                .detail
                .as_ref()
                .map_or(0, |d| d.trim().lines().count())
    };
    natural.min(PERMISSION_MIN_BODY_ROWS)
}

/// An MCP prompt's body: the arguments JSON, word-wrapped and indented like
/// the command body — at most `budget` rows, the hidden count returned for
/// the `… +N lines` tail.
fn mcp_body_rows(body: &str, width: u16, budget: usize) -> (Vec<Line<'static>>, usize) {
    let indent = PERMISSION_COMMAND_INDENT;
    let room = width.saturating_sub(cols(indent) as u16).max(1);
    let source: Vec<String> = body
        .lines()
        .flat_map(|line| wrap_output(line, room))
        .collect();
    let hidden = source.len().saturating_sub(budget.max(1));
    let shown = source.len() - hidden;
    let lines = source
        .into_iter()
        .take(shown)
        .map(|text| {
            Line::from(vec![
                Span::raw(indent),
                Span::styled(text, Style::new().fg(PERMISSION_TARGET_COLOR)),
            ])
        })
        .collect();
    (lines, hidden)
}

/// The highlight language for the preview — the target path's extension.
fn file_lang(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
}

/// Paint the open permission prompt over the whole live region. Pure —
/// [`render_live`] calls this in place of the composer. The region is sized
/// to exactly these rows ([`permission_height`](super::layout::permission_height)),
/// and the conversation above it is the *real* screen — the prompt grew by
/// scrolling like any other region, so what the user just read stays put
/// (and stays reachable in the terminal's scrollback — `docs/permissions.md`).
pub fn render_permission(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(permission_lines(app, area.width, area.height)).render(area, buf);
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

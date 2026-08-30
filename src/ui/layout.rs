//! The live region's geometry: how tall it is, where it re-pins on a resize,
//! and where the cursor sits inside it.
//!
//! Pure policy — `term.rs` calls these and does the terminal I/O itself
//! (`docs/design.md`). The rows it sizes are the streaming strip
//! (`docs/status-indicator.md`), the bands (`docs/shortcuts.md`,
//! `docs/file-search.md`) and the footer (`docs/footer.md`).

use super::agent::agent_preview_rows;
use super::live::preview_tool_lines;
use super::theme::*;
use super::wrap::cols;
use super::*;

/// Rows of the streaming strip above the box. The strip has two independent
/// slots, each with a trailing gap:
///
/// - the **status line** (`has_status`: a turn is active *and* it is not a `!`
///   shell turn — a shell run hides the spinner status entirely, showing its
///   elapsed in the `⎿ Running… (Ns)` preview instead, see [`strip_has_status`]
///   and `docs/shell-command.md`);
/// - the **preview** (`preview_rows` content rows: a reply whose buffer is
///   non-empty previews one row; a running backend tool previews its *whole*
///   collapsed cell — the wrapped `● name(args)` header **plus** its `⎿ Running…`
///   row, so a long command isn't clipped and the running state is visible; a
///   running `!` shell command previews one `⎿ Running… (Ns)` row — see
///   [`preview_rows`]).
///
/// So a normal streaming turn is preview + gap + status + gap; the pre-stream
/// pause is status + gap only (no empty preview line, codex parity); a shell run
/// is one preview row + gap only (no status); and idle it collapses to nothing.
/// The **task checklist** (`task_rows` — `docs/task-tools.md`) sits inside the
/// status slot, directly under the status line and above its trailing gap, so
/// its `⎿` rows visually hang off the spinner the way tool output hangs off
/// its header (0 when no turn is active or the list is empty — [`task_rows`]
/// gates on the same `has_status`). The **queued messages** (`queued_rows`)
/// stack below all of this, between the strip and the box's top rule — added
/// separately by [`live_height`]/[`live_layout`] since their height depends on
/// the queue.
pub(super) const fn strip_rows(has_status: bool, preview_rows: u16, task_rows: u16) -> u16 {
    let preview = if preview_rows > 0 {
        preview_rows + GAP_ROWS
    } else {
        0
    };
    // The status slot carries the checklist under the spinner; with no
    // status line the same slot holds the **idle** block alone (its count
    // line + rows), keeping its trailing gap so the box never sits flush
    // against it. Both collapse to nothing when there is neither.
    let status = if has_status {
        STATUS_ROWS + task_rows + STATUS_GAP_ROWS
    } else if task_rows > 0 {
        task_rows + STATUS_GAP_ROWS
    } else {
        0
    };
    preview + status
}

/// The number of **preview** content rows the streaming strip's content
/// *wants* for `app` at `width` (0 during the pre-stream pause / when idle, so
/// the strip drops the preview slot and its gap rather than leaving a stray
/// blank — codex's behaviour). A running backend tool previews its *whole*
/// collapsed cell (the wrapped header + `⎿ Running…`) so a long command isn't
/// clipped live and the running state shows; a running `!` shell command and a
/// streaming reply preview a single row.
///
/// This is what the content asks for, which on a small terminal is more than
/// the region can give: the conversation view sizes itself from
/// [`fitted_preview_rows`] instead, which is this clamped to
/// [`preview_budget`]. Must match exactly what `preview_lines` builds before
/// its own trim (they share this walk, so they agree by construction).
#[must_use]
pub fn preview_rows(app: &App, width: u16) -> u16 {
    // An agent session view previews the *viewed agent's* stream — its live
    // tool cells, its thinking block, or its reply's uncommitted frontier
    // (docs/agent-tool.md, docs/agent-view-streaming.md). The branch mirrors
    // the main one below, so `agent_preview_rows` owns it.
    if let Some(run) = app.viewed_agent() {
        return agent_preview_rows(app, run, width);
    }
    // A live agent group previews its whole tree cell (over the tool queue's
    // cells when a mixed round runs both — docs/agent-tool.md); a live tool
    // queue previews every call's cell (a **parallel batch** shows the
    // running one + each `⎿ Waiting…` sibling, blank-separated); a `!` shell
    // run its single `⎿ Running… (Ns)` row. Sized from the same walk the strip
    // draws ([`preview_tool_lines`]) so the count and the paint agree.
    if app.agent_group().is_some() || !app.tool_queue().is_empty() {
        return u16::try_from(preview_tool_lines(app, width).len()).unwrap_or(u16::MAX);
    }
    // An open thinking phase previews its live block — the `● Thinking…`
    // header plus the tail it shows (docs/thinking-stream.md). Sized from the
    // same walk the strip draws, so the two agree by construction.
    if let Some(text) = app.reasoning() {
        return u16::try_from(
            super::reasoning::live_reasoning_lines(text, app.pulse(), width).len(),
        )
        .unwrap_or(u16::MAX);
    }
    // A streaming reply previews the rows scrollback does not hold yet — its
    // last row for ordinary prose, every wrapped row of a withheld source
    // line, the whole forming table, or **none at all** when the frontier just
    // committed clean (a closing ``` renders no rows of its own). Only the
    // boundary's `StreamRender` knows that height, so it injects the count each
    // frame via [`App::set_stream_preview_rows`] (the `set_status_times`
    // pattern; docs/table-streaming.md) and this reports it verbatim — a floor
    // here would reserve a row [`preview_lines`] does not draw, tripping its
    // `debug_assert`. 1 until a draw injects one (`App::begin_stream`'s
    // default), which is also what the `None` render fallback draws. The
    // pre-stream pause / idle reserve none.
    if app.streaming_text().is_some_and(|t| !t.is_empty()) {
        app.stream_preview_rows()
    } else {
        0
    }
}

/// Rows the streaming strip's **preview slot** may take inside a live region
/// of `height` — the one budget the whole pipeline spends
/// (docs/table-streaming.md *The preview slot is budgeted*).
///
/// The preview is the region's only elastic row: everything else it holds is
/// decided by state the frame cannot negotiate with — the status line and the
/// task checklist, the queued messages, the toast, the composer's two rules
/// and its wrapped rows, the band below it, the footer, the agent roster. So
/// the budget is what is left once all of that is paid, minus the slot's own
/// trailing gap; 0 when nothing is left, which drops the slot whole.
///
/// A **fixed** allowance was the reported bug: the cap counted the box, the
/// status line and a footer and spent the region's entire slack on the forming
/// table, so opening the `/` palette mid-table asked for rows the terminal did
/// not have and the composer was squeezed off the screen until the turn ended.
///
/// `height` is the rows the live region may occupy — the screen height at the
/// boundary, the region's own `area.height` inside a render, which agree by
/// construction: an unclamped region is exactly the sum of its parts, so the
/// budget comes back as the content's full ask; a clamped one is the terminal.
///
/// A composer-replacing view (`/model`, `/settings`, the ↓ manager band …)
/// pays for the composer's chrome here rather than its own frame's rows — it
/// keeps that frame by `view_split`, which pins the body and squeezes the
/// strip, so the box-eviction this budget prevents cannot arise there.
#[must_use]
pub fn preview_budget(app: &App, width: u16, height: u16) -> u16 {
    slot_budget(height, non_preview_rows(app, width))
}

/// Every row the conversation view's live region owes **besides** the preview
/// slot: the status line and its checklist with their gap, the queued
/// messages, the toast, the composer's two rules and its wrapped rows, the
/// band below it, the footer, and the agent roster. What [`live_height`] adds
/// the preview slot to, spelled once so the budget cannot drift from the sum
/// it is subtracted from.
///
/// Saturating throughout: the queue and the input are both uncapped (a
/// recalled multi-megabyte paste wraps to tens of thousands of rows), and a
/// saturated total simply means no budget at all.
///
/// It re-derives rows its callers often have in hand rather than taking ten
/// arguments, because a second spelling of this sum is a sum that can drift.
/// Measured: 53 ns on an ordinary frame, and 13 µs on one whose palette is
/// open over a four-message queue (`queued_lines` renders each message) —
/// against a 8.3 ms frame budget, and beside the three calls the draw already
/// makes to the same helpers. If it ever matters, memoize `queued_lines`,
/// which every one of those call sites would gain from too.
fn non_preview_rows(app: &App, width: u16) -> u16 {
    let band = band_rows(app, width);
    strip_other_rows(app, width)
        .saturating_add(INPUT_CHROME_ROWS)
        .saturating_add(u16::try_from(app.input.row_count(field_width(width))).unwrap_or(u16::MAX))
        .saturating_add(band)
        .saturating_add(footer_rows(app, band))
        .saturating_add(agent_list_rows(app))
}

/// The strip's own rows **besides** the preview slot: the status line and its
/// task checklist with their trailing gap, the queued messages, and the toast.
/// [`strip_above_rows`] is this plus the preview slot; `render_strip_above`
/// subtracts it from the rows [`view_split`] left the strip.
pub(super) fn strip_other_rows(app: &App, width: u16) -> u16 {
    strip_rows(
        strip_has_status(app),
        0,
        super::tasks::task_rows(app, width),
    )
    .saturating_add(queued_rows(app, width))
    .saturating_add(toast_rows(app))
}

/// Rows left for the preview slot inside `height` once `other` — every row
/// beside it — is paid, the slot's own trailing gap included.
const fn slot_budget(height: u16, other: u16) -> u16 {
    height.saturating_sub(other.saturating_add(GAP_ROWS))
}

/// `want` clamped to [`slot_budget`]: the preview's fit inside a region whose
/// other rows are already spoken for. [`fitted_preview_rows`] is this over the
/// conversation view's chrome; `render_strip_above` is this over the rows
/// [`view_split`] left a composer-replacing view's strip.
pub(super) const fn fit_preview_rows(height: u16, want: u16, other: u16) -> u16 {
    let room = slot_budget(height, other);
    if want < room { want } else { room }
}

/// [`preview_rows`] clamped to [`preview_budget`]: the rows the strip actually
/// reserves *and* draws in a live region of `height`. The single source of
/// truth the conversation view's geometry sizes by — [`live_height`],
/// `live_layout`, `input_box`, [`cursor_position`] and the strip's own
/// paint all read it, so a squeezed region trims the preview instead of the
/// composer.
#[must_use]
pub fn fitted_preview_rows(app: &App, width: u16, height: u16) -> u16 {
    fit_preview_rows(
        height,
        preview_rows(app, width),
        non_preview_rows(app, width),
    )
}

/// The cap the boundary passes to [`StreamRender::preview`]: how many strip
/// rows a multi-row (forming-table) preview may take at this terminal height
/// before it tail-follows its newest rows — [`preview_budget`] exactly, so the
/// renderer never spends a row the strip cannot reserve (docs/table-streaming.md).
#[must_use]
pub fn stream_preview_max_rows(app: &App, width: u16, term_height: u16) -> usize {
    usize::from(preview_budget(app, width, term_height))
}

/// Whether the streaming strip shows the **status line** (the spinner + timer +
/// `esc to interrupt`): true while a turn is active, **except a `!` shell
/// turn**, which suppresses the whole status row and shows its elapsed in the
/// `⎿ Running… (Ns)` preview instead (docs/shell-command.md). Idle → false
/// (no turn). Used by [`render_live`]/[`cursor_position`]/`main.rs` to feed
/// `strip_rows` (and to gate the status render in [`render_live`]).
#[must_use]
pub fn strip_has_status(app: &App) -> bool {
    // An agent session view shows the *viewed agent's* status while it runs
    // — the main turn's spinner belongs to the main screen
    // (docs/agent-tool.md).
    if let Some(run) = app.viewed_agent() {
        return !run.status.is_final();
    }
    app.status().is_some_and(|status| !status.shell)
}

/// Columns the input field's text occupies: the box spans the full width (no side
/// borders) minus the prompt/indent that prefixes every text row.
fn field_width(width: u16) -> u16 {
    width.saturating_sub(BULLET_WIDTH).max(1)
}

/// Height of the bottom live region for the current `input` at this terminal
/// size: the streaming strip (the status and/or preview slots — see
/// `strip_rows` for how `has_status`/`has_preview` size it) plus the
/// `queued_rows` queued-message lines stacked under it (the strip's
/// [`queued_rows`]), the `toast_rows` transient toast row just above the box
/// ([`toast_rows`], 0 or 1), two framing rules, one row per wrapped input line —
/// so the box **grows** as the message wraps — the band below it (`band_rows`:
/// the command palette's [`menu_rows`] plus the shortcuts band's
/// [`shortcuts_rows`], 0 when both are closed), and the session-context footer
/// under that (`footer_rows`: [`footer_rows`], 0 when a band displaces it) —
/// clamped to the terminal height (after which the box scrolls internally; see
/// [`render_live`]).
// A flat list of irreducible geometry measurements — bundling them into a
// struct would only obscure the positional layout the tests assert directly.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn live_height(
    input: &TextArea,
    width: u16,
    term_height: u16,
    has_status: bool,
    preview_rows: u16,
    task_rows: u16,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
    agent_rows: u16,
) -> u16 {
    // Summed in usize: `rows` and `queued_rows` are both unbounded (a recalled
    // multi-megabyte paste wraps to tens of thousands of rows, and the queue
    // is deliberately uncapped), so a u16 sum can overflow-panic long before
    // the clamp. The result is ≤ term_height, so the final cast is exact.
    let rows = input.row_count(field_width(width));
    (usize::from(strip_rows(has_status, preview_rows, task_rows))
        + usize::from(queued_rows)
        + usize::from(toast_rows)
        + usize::from(INPUT_CHROME_ROWS)
        + rows
        + usize::from(band_rows)
        + usize::from(footer_rows)
        + usize::from(agent_rows))
    .min(usize::from(term_height.max(1))) as u16
}

/// The rows a **composer-replacing inline view** keeps *above* itself: the
/// streaming strip ([`strip_rows`] — the preview, the status line and the task
/// checklist with their gaps), the queued messages, and the toast row.
///
/// The `/model` picker, the `/login` flow, the `/settings` menu and the ↓
/// background manager band all replace the **composer** — never the running
/// turn (they open mid-turn precisely because the turn streams on its own
/// thread, `docs/llm.md`). So what is executing stays on screen above them:
/// the running tool's live cell, the spinner status line, the queued
/// follow-ups, the toast. Taking the whole region instead hid exactly the turn
/// the user opened the view beside — the reported bug, first for the band
/// (`docs/background.md`) and then for the pickers.
///
/// Saturating: the queue is uncapped, and the callers clamp the sum to the
/// terminal height anyway.
pub(super) fn strip_above_rows(app: &App, width: u16) -> u16 {
    strip_rows(
        strip_has_status(app),
        preview_rows(app, width),
        super::tasks::task_rows(app, width),
    )
    .saturating_add(queued_rows(app, width))
    .saturating_add(toast_rows(app))
}

/// A composer-replacing view's live-region height: its own `body` rows under
/// the [`strip_above_rows`] strip, clamped to the terminal. Shared by the four
/// `*_height` helpers so they agree by construction.
pub(super) fn view_height(app: &App, width: u16, body: u16, term_height: u16) -> u16 {
    strip_above_rows(app, width)
        .saturating_add(body)
        .min(term_height.max(1))
}

/// Split the live region into `[strip, body]` — the streaming strip above a
/// composer-replacing view's own `body_rows`-tall frame below it (the sum is
/// what [`view_height`] reserved). The body is a `Length`, so a region the
/// terminal squeezed keeps the view whole and drops strip rows first; the
/// paint ([`render_live_with_preview`]) and the cursor seat
/// ([`cursor_position`]) share this split, so they can never drift.
pub(super) fn view_split(area: Rect, body_rows: u16) -> [Rect; 2] {
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(body_rows.min(area.height)),
    ])
    .areas(area)
}

/// Seat the hardware cursor `row` rows into a composer-replacing view's own
/// frame, clamped inside the live region (a short terminal can squeeze the
/// view's top rows off, and a collapsed body has no rows at all).
fn view_cursor_y(area: Rect, body: Rect, row: u16) -> u16 {
    body.y
        .saturating_add(row)
        .min(area.y + area.height.saturating_sub(1))
}

/// A framed view page row's **on-screen** row under the bottom anchor: a page
/// taller than its body paints from `view_body_skip` down, so every page row
/// shifts up by the skip — clamped at the body top when the row itself is
/// scrolled off (the flow holds it in scrollback then, where a hardware
/// cursor cannot sit — `docs/view-flow.md`).
fn anchored_view_row(page_rows: u16, body: Rect, row: u16) -> u16 {
    let skip = super::view_flow::view_body_skip(usize::from(page_rows), body.height);
    row.saturating_sub(u16::try_from(skip).unwrap_or(u16::MAX))
}

/// Whether the picker shows its counter + model-name detail rows below the
/// list — only when a real model is listed (Ready with at least one match).
/// The placeholder states have a blank counter and name, so those rows collapse
/// to a single trailing gap.
pub(super) fn model_has_detail(picker: &ModelPicker) -> bool {
    picker.status == ModelLoad::Ready && !picker.matches().is_empty()
}

/// The rows the `/model` picker's own frame occupies — the built page's line
/// count (`model_view_lines`), so the height and the paint agree by
/// construction (`docs/view-flow.md`). What [`model_picker_height`] reserves
/// under the strip, and what [`render_live`] hands [`render_model_picker`].
pub(super) fn model_picker_rows(picker: &ModelPicker, width: u16) -> u16 {
    u16::try_from(super::model_view::model_view_lines(picker, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/model` picker is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). The picker
/// **replaces** the composer — and *only* the composer: the streaming strip
/// keeps its rows above it (`strip_above_rows`), so opening `/model`
/// mid-turn never hides the running turn's status line or its live tool cell.
/// Clamped to the terminal height. Shared by `tui::view`'s
/// `live_region_height`, [`render_live`], and [`cursor_position`] so all three
/// agree.
#[must_use]
pub fn model_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    let picker = app.model_picker.as_ref()?;
    Some(view_height(
        app,
        width,
        model_picker_rows(picker, width),
        term_height,
    ))
}

/// The inline live-region height when a tool-permission prompt is open, or
/// `None` when none is (the caller falls back to [`live_height`]). Like the
/// `/model` picker it **replaces** the composer — and the streaming strip with
/// it, since the turn is blocked on the answer. Exactly the rows
/// [`permission_lines`] builds, clamped to the terminal. See
/// `docs/permissions.md`.
#[must_use]
pub fn permission_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.permission()?;
    let rows = permission_lines(app, width, term_height).len() as u16;
    Some(rows.min(term_height.max(1)))
}

/// The inline live-region height when the ↓ background manager band is open,
/// or `None` when it isn't (the caller falls back to [`live_height`]). Like
/// the `/model` picker it **replaces** the composer — but *only* the
/// composer: the streaming strip (a running tool's live cell, the status
/// line, the queued messages, the toast) keeps its rows **above** the band,
/// so opening the manager mid-turn never hides what is executing (the
/// user-reported fix, `docs/background.md`). The band's own height is the
/// built line count ([`background_view_lines`]) **at the terminal's real
/// width** — the details page's Command field wraps, so the count is
/// width-dependent — the sum clamped to the terminal.
#[must_use]
pub fn background_view_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.background_view.as_ref()?;
    let band = u16::try_from(background_view_lines(app, width).len()).unwrap_or(u16::MAX);
    Some(view_height(app, width, band, term_height))
}

/// The rows the `/login` flow's own frame occupies — the built page's line
/// count (`key_onboarding_lines`): the provider step grows with its list, the
/// key step is a fixed height. What [`key_onboarding_height`] reserves under
/// the strip, and what [`render_live`] hands [`render_key_onboarding`].
pub(super) fn key_onboarding_rows(onboarding: &KeyOnboarding, width: u16) -> u16 {
    u16::try_from(super::login_view::key_onboarding_lines(onboarding, width).len())
        .unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/login` flow is open, or `None` when
/// it isn't (the caller falls back to [`live_height`]). Like the `/model`
/// picker it **replaces** the composer only, the streaming strip keeping its
/// rows above it (`strip_above_rows`). Shared by `tui::view`'s
/// `live_region_height`, [`render_live`], and [`cursor_position`].
#[must_use]
pub fn key_onboarding_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    let onboarding = app.key_onboarding.as_ref()?;
    Some(view_height(
        app,
        width,
        key_onboarding_rows(onboarding, width),
        term_height,
    ))
}

/// How to re-pin the live region when its height changes between draws, keeping
/// it **content-anchored** — its top fixed, like Claude Code / codex. The box
/// grows *downward* in place; the screen only scrolls up when the box would run
/// past the bottom (i.e. it has reached the bottom), and a shrink vacates rows
/// just below it. The pure decision behind `term::InlineViewport::draw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repin {
    /// Scroll the whole screen up this many rows first (0 unless the grown box
    /// overflows the bottom — only then does the chat scroll into scrollback).
    pub scroll_up: u16,
    /// The live region's new top row (unchanged unless it overflowed the bottom).
    pub top: u16,
    /// Rows to blank just below the new region (a shrink vacates them).
    pub clear_below: u16,
}

/// The terminal row the cursor should land on when the app exits: the row just
/// **below** the live region, so the shell prompt resumes directly under it.
///
/// Returns `None` when the box already occupies the last screen row (no room
/// below — the caller scrolls up one line and lands at the bottom instead).
/// Landing here, rather than at the screen bottom, is what avoids the big blank
/// gap on exit when the box is content-anchored near the top.
#[must_use]
pub fn restore_cursor_row(view_top: u16, view_height: u16, screen_height: u16) -> Option<u16> {
    let below = view_top.saturating_add(view_height);
    (below < screen_height).then_some(below)
}

/// Whether the live region is an **inline modal** — the tool-permission
/// prompt or the `AskUserQuestion` modal, the inline views that can be as
/// tall as the whole terminal and whose close needs special care
/// (`docs/permissions.md`, `docs/ask.md`).
///
/// The prompt grows through the ordinary [`repin`] like every other region —
/// the chat above it scrolls into the terminal's *real* scrollback, so the
/// user can scroll up and read while it asks. What makes it modal is the
/// collapse: the growth's scroll is one-way, so a plain shrink back to the
/// composer would strand the box above the rows it vacates. `term.rs` reads
/// this predicate to note any one-way move under an open prompt
/// (`InlineViewport::modal_scrolled`), and the loop answers the prompt's
/// close with a purge rebuild.
///
/// The sibling of [`strip_has_status`]: a pure predicate over `App`, keeping
/// the policy here and the I/O there. It is `ui`'s name for
/// [`App::modal_open`], which the key routing and the backtrack guard read
/// too — one definition, so the three cannot drift.
#[must_use]
pub fn region_is_modal(app: &App) -> bool {
    app.modal_open()
}

/// Whether this draw must **purge-rebuild** the conversation instead of
/// painting the live region in place — the pure decision behind the loop's
/// rebuild arm, the sibling of [`region_is_modal`] (`docs/permissions.md`).
/// Both cases answer the same problem: the modal prompt's scrolls are
/// one-way, so a plain shrink strands the region above rows it cannot refill.
///
/// - **The prompt just closed** (`modal` false): rebuild iff a one-way move
///   was noted while it was open (`modal_scrolled`,
///   `term::InlineViewport::modal_scrolled`) — the behaviour the close has
///   always had; the geometry is irrelevant, the note stands for what already
///   moved.
/// - **The prompt is open** (`modal` true), the last painted frame reached
///   the screen bottom (`painted_bottom`), **and this frame would seat the
///   region short of it**: rebuild *now*. The coming frame's bottom is what
///   the paint will produce — the flush seats the region below the
///   `pending_rows` queued lines and the repin never moves the top down, so
///   it ends at `view_top + pending_rows + new_height` (capped by the screen,
///   which the paint reaches by scrolling). A batch's back-to-back prompts
///   differ in height (a tall body-capped prompt answered, a short one
///   opening as the resolved cell commits out of the live region — or a
///   subagent's tree-topped prompt giving way to a main-turn one), and every
///   short-of-the-bottom seat leaves a blank band under the open prompt for
///   as long as it asks: [`repin`] blanks the vacated rows in place, and the
///   flush's trailing clear blanks everything below the shorter seat. Neither
///   can refill the bottom — only the rebuild reseats the prompt flush there.
///   The comparison is against the **painted** bottom, not the tracked
///   height: `set_view_height` re-syncs the tracked height between paints
///   (the turn-end flush plan), and the one-way note is deliberately not
///   consulted — the separate-frame shrink is a pure repin (no flush, no
///   scroll), which never set it. A region floating above the bottom shrinks
///   over rows that are already blank — no visible gap, and skipping the
///   rebuild keeps the user's own terminal scrollback unpurged.
#[must_use]
pub fn modal_needs_rebuild(
    modal: bool,
    modal_scrolled: bool,
    painted_bottom: u16,
    view_top: u16,
    pending_rows: usize,
    new_height: u16,
    screen_height: u16,
) -> bool {
    if modal {
        let planned = usize::from(view_top) + pending_rows + usize::from(new_height);
        painted_bottom >= screen_height && planned < usize::from(screen_height)
    } else {
        modal_scrolled
    }
}

/// Decide how to re-pin a live region currently at `top` with `old_height` to
/// `new_height` on a `screen_height`-row screen, keeping its top anchored.
#[must_use]
pub fn repin(top: u16, old_height: u16, new_height: u16, screen_height: u16) -> Repin {
    let bottom = u32::from(top) + u32::from(new_height);
    let scroll_up = bottom.saturating_sub(u32::from(screen_height)) as u16;
    let new_top = top.saturating_sub(scroll_up);
    let clear_below = top
        .saturating_add(old_height)
        .saturating_sub(new_top.saturating_add(new_height));
    Repin {
        scroll_up,
        top: new_top,
        clear_below,
    }
}

/// Split `area` into the live region's four stacked sub-areas
/// `[strip, input, band, footer]`. The strip holds the streaming preview, gap,
/// status, gap, **the `queued_rows` queued-message lines below them, and the
/// `toast_rows` transient toast row at its very bottom (just above the box)**
/// (height 0 when idle and no toast); the band — the command palette *or* the
/// `?` shortcuts overview — takes its fixed `band_rows` below the box (0 when
/// closed); the session-context footer sits on the very last `footer_rows` (0
/// when unset or displaced by the band); the input box takes whatever rows
/// remain in between, so it **grows** as `area` grows (see [`live_height`]).
/// Reserving the band and footer below rather than between keeps the box's top —
/// and the cursor — put when they appear. The only place the split is expressed.
#[allow(clippy::too_many_arguments)]
pub(super) fn live_layout(
    area: Rect,
    has_status: bool,
    preview_rows: u16,
    task_rows: u16,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
    agent_rows: u16,
) -> [Rect; 5] {
    // Hand-split rather than solved, because when the asks overflow the area
    // *which* slot gives way is the whole point. The box's floor is held back
    // first: a region that has `LIVE_MIN_HEIGHT` rows at all keeps a composer
    // you can see and type into, whatever the strip and the band asked for
    // (a constraint solver spent the rows in constraint order instead and
    // returned `Min(0)` == 0 — the textarea gone from the screen, the reported
    // bug's last mile). The rest take their ask in priority order from what is
    // left — the band the user just opened and is typing into, then the
    // bottom-pinned footer and agent roster, then the strip, whose preview is
    // the one row-count that is *supposed* to yield (`preview_budget`
    // normally spends it before we get here) — and every row nobody claimed
    // grows the box, so a region that fits lands exactly where the solver put
    // it. (The band and the footer never actually compete: `footer_rows`
    // reports 0 while a band is open, `docs/footer.md`.)
    let mut spare = area.height;
    let floor = LIVE_MIN_HEIGHT.min(spare);
    spare -= floor;
    let mut take = |ask: u16| {
        let got = ask.min(spare);
        spare -= got;
        got
    };
    let band = take(band_rows);
    let footer = take(footer_rows);
    let agent = take(agent_rows);
    // Saturating: `queued_rows` is uncapped, and `take` is what bounds it.
    let strip = take(
        strip_rows(has_status, preview_rows, task_rows)
            .saturating_add(queued_rows)
            .saturating_add(toast_rows),
    );
    let input = floor + spare;
    let mut y = area.y;
    let mut slice = |height: u16| {
        let rect = Rect { y, height, ..area };
        y += height;
        rect
    };
    [
        slice(strip),
        slice(input),
        slice(band),
        slice(footer),
        slice(agent),
    ]
}

/// The geometry shared by [`render_live`] and [`cursor_position`] so the drawn
/// text and the hardware cursor can never drift apart: where the input text rows
/// live, the input wrapped to the field width, the cursor's wrapped row/column,
/// and how far it's scrolled so the **cursor** stays visible when the box is full.
pub(super) struct InputBox {
    /// The rule-framed box area (below the preview); borders are drawn here.
    pub(super) frame: Rect,
    /// The inner area that holds the text rows (the box minus its two rules).
    pub(super) text: Rect,
    /// Every wrapped input row's displayed text (always at least one, possibly empty).
    pub(super) rows: Vec<String>,
    /// Each row's byte range into the input text (parallel to `rows`) — what
    /// maps the Ctrl+R search-highlight byte ranges onto row columns.
    pub(super) row_ranges: Vec<Range<usize>>,
    /// The cursor's wrapped-row index and display column (from the [`TextArea`]).
    cursor_row: usize,
    cursor_col: usize,
    /// Index of the first wrapped row shown — the window follows the cursor.
    pub(super) scroll: usize,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn input_box(
    area: Rect,
    input: &TextArea,
    has_status: bool,
    preview_rows: u16,
    task_rows: u16,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
    agent_rows: u16,
) -> InputBox {
    let [_, frame, _, _, _] = live_layout(
        area,
        has_status,
        preview_rows,
        task_rows,
        queued_rows,
        toast_rows,
        band_rows,
        footer_rows,
        agent_rows,
    );
    let text = frame.inner(Margin::new(0, 1)); // inset past the top & bottom rules
    let field = field_width(area.width);
    let rows = input.display_rows(field);
    let row_ranges = input.wrapped_rows(field);
    let (cursor_row, cursor_col) = input.cursor_row_col(field);
    let scroll = input_scroll(rows.len(), cursor_row, text.height as usize);
    InputBox {
        frame,
        text,
        rows,
        row_ranges,
        cursor_row,
        cursor_col,
        scroll,
    }
}

/// First visible input row so the cursor stays on screen: 0 while the rows fit,
/// otherwise the window shifts down just enough to keep `cursor_row` visible
/// (codex's `effective_scroll`, recomputed from the cursor each draw rather than
/// persisted). Clamped so the last window sits flush with the end.
fn input_scroll(total: usize, cursor_row: usize, height: usize) -> usize {
    if height == 0 || total <= height {
        return 0;
    }
    let max = total - height;
    (cursor_row + 1).saturating_sub(height).min(max)
}

/// Whether this frame shows a hardware cursor at all.
///
/// Almost always yes — the composer, the pickers and the bands all take typing,
/// and codex keeps the cursor on the prompt row even mid-turn. The exception is
/// a permission prompt's **option list**: it is a menu, not a text field, so
/// there is nothing for a cursor to point at, and the terminal cursor is the
/// one thing on screen that moves by itself — a terminal with a cursor-trail
/// animation (kitty) draws every jump it makes, which turned opening a prompt
/// and stepping through its options into so much flying punctuation. Tab's
/// amend field *is* typed into, so the caret comes back with it.
///
/// The `term.rs` sibling of [`strip_has_status`]: pure policy the boundary
/// acts on, by skipping the frame-closing `Show` (the frame already opened with
/// a `Hide`, so the cursor simply never reappears). [`cursor_position`] still
/// seats it either way, so a terminal that ignores the hide — and the cursor's
/// return when the prompt closes — starts from somewhere meaningful.
///
/// The ask modal follows the same rule (`docs/ask.md`): its option pages are
/// menus (no cursor), and the caret comes back for the free-text Other entry
/// and the notes field.
#[must_use]
pub fn cursor_visible(app: &App) -> bool {
    if let Some(ask) = app.ask() {
        return ask.editing();
    }
    if let Some(prompt) = app.permission() {
        return prompt.amend;
    }
    // The no-text-entry pickers (`/hooks`, `/trust`, `/mcp`) and the ↓
    // manager band are menus too — same rule (`docs/project-config.md`,
    // `docs/mcp.md`). The `/mcp` Auth page's `URL >` field *is* typed into,
    // so its caret stays, the amend-field exception again.
    if app.hooks_menu.is_some() || app.trust_menu.is_some() || app.background_view.is_some() {
        return false;
    }
    // The `/login` device page is a wait, not a field — the same menu rule: a
    // kitty cursor trail would streak across it on every countdown tick.
    if let Some(onboarding) = &app.key_onboarding
        && onboarding.step == KeyStep::Device
    {
        return false;
    }
    if let Some(menu) = &app.mcp_menu {
        return menu.page == crate::app::McpPage::Auth;
    }
    true
}

/// The hidden cursor's seat after a full-screen overlay paint (the Ctrl+O
/// transcript, the Ctrl+D context view, the `/resume` picker): the cell just
/// past the frame's **last non-blank glyph** — the end of the closing
/// `q/esc/… to quit` hint — clamped inside the frame, or the origin of an
/// empty one.
///
/// The overlay never *shows* the cursor (`term.rs` re-asserts the hide on
/// every overlay frame), but the seat still matters: a terminal with a
/// cursor-move animation (kitty's trail and kin) animates toward wherever
/// the cursor sits, and a full-screen cell paint used to leave it at the
/// blank bottom-right corner — the reported "the cursor animation jumps to
/// nowhere" on Ctrl+O/Ctrl+D. Seating it after the last text keeps the jump
/// on something readable — the same hidden-but-seated rule
/// [`cursor_visible`] documents for the inline menus. A wide glyph's shadow
/// cells are blank, so the scan lands on the glyph itself and steps past its
/// full width.
#[must_use]
pub fn overlay_cursor_seat(buf: &Buffer) -> (u16, u16) {
    let area = buf.area;
    for y in (area.top()..area.bottom()).rev() {
        for x in (area.left()..area.right()).rev() {
            let Some(cell) = buf.cell((x, y)) else {
                continue;
            };
            let symbol = cell.symbol();
            if symbol.trim().is_empty() {
                continue;
            }
            let after = x.saturating_add(cols(symbol).max(1) as u16);
            return (after.min(area.right().saturating_sub(1)), y);
        }
    }
    (area.left(), area.top())
}

/// The hidden cursor's seat inside a no-text-entry menu: the highlighted
/// `❯` row's marker column, found by scanning the built lines for the
/// selection marker span — the views are content-driven, so the row is
/// wherever the content put it. The [`view_split`] the paint uses seats the
/// body under any streaming strip, and a body taller than its area is
/// painted **bottom-anchored** ([`view_body_skip`], `docs/view-flow.md`), so
/// the marker's page row is shifted by the same skipped top rows; a
/// marker-less page (a detail view) and a marker whose row the anchor
/// scrolled off fall back to the far corner, where the seat reads as chrome
/// (the manager band's old rule).
fn menu_marker_seat(lines: &[ratatui::text::Line<'_>], area: Rect) -> (u16, u16) {
    let corner = (
        area.x + area.width.saturating_sub(1),
        area.y + area.height.saturating_sub(1),
    );
    let body_h = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let [_, body] = view_split(area, body_h);
    let skip = super::view_flow::view_body_skip(lines.len(), body.height);
    for (i, line) in lines.iter().enumerate() {
        let mut before = 0usize;
        for span in &line.spans {
            if span.content.as_ref() == HOOKS_MARKER {
                // The bottom anchor paints `lines[skip..]`: a marker above the
                // skip has no on-screen row.
                let Some(row) = i.checked_sub(skip) else {
                    return corner;
                };
                let Ok(row) = u16::try_from(row) else {
                    return corner;
                };
                if row >= body.height {
                    return corner;
                }
                let x = area.x + (before.min(usize::from(area.width.saturating_sub(1))) as u16);
                return (x, body.y + row);
            }
            before += cols(&span.content);
        }
    }
    corner
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input — its resting seat, which [`cursor_visible`] decides whether
/// to actually show. Shares `input_box` with [`render_live`] so the cursor lands
/// exactly where the editor's cursor is — on its wrapped row, at its column —
/// wherever the user has moved it, not just at the end.
#[must_use]
pub fn cursor_position(area: Rect, app: &App) -> (u16, u16) {
    // The ask modal seats the cursor in whichever entry field is live (the
    // Other row, the notes line), else on the highlighted `❯` row — the
    // permission prompt's rule, so a terminal's cursor animation lands on
    // the option being chosen while `cursor_visible` hides the cursor over
    // the menu. The builder computed the seat with the rows, so the two can
    // never drift (`docs/ask.md`); the far corner is only the fallback for
    // a seat the height clamp dropped off the page.
    if app.ask().is_some() {
        if let Some((x, y)) = super::ask_view::ask_cursor(app, area.width, area.height) {
            return (area.x + x, area.y + y);
        }
        let x = area.x + area.width.saturating_sub(1);
        let y = area.y + area.height.saturating_sub(1);
        return (x, y);
    }
    // A permission prompt seats the cursor on the row it is asking about: the
    // **highlighted option**, or Tab's amend field when that has replaced the
    // options. Both blocks sit a fixed [`PERMISSION_TAIL_ROWS`] above the
    // region's bottom (gap, hint, gap, rule) — `permission_lines` pads a capped
    // prompt above the question to keep them there — so the cursor is found
    // from that edge without re-deriving the body. The column is the same for
    // both (past the inset and the `❯ ` marker), so ↑/↓ walk the cursor
    // **straight** down the options and Tab never slides it sideways: the
    // terminal cursor is the one thing on screen that moves by itself, and a
    // kitty cursor-trail animation draws every jump it makes.
    if let Some(prompt) = app.permission() {
        let (row, ccol) = if prompt.amend {
            let field = super::permission_view::amend_field_width(area.width);
            let (crow, ccol) = app.input.cursor_row_col(field);
            let rows = app.input.row_count(field) as u16;
            (
                area.height
                    .saturating_sub(PERMISSION_TAIL_ROWS.saturating_add(rows))
                    + crow as u16,
                ccol,
            )
        } else {
            // Options wrap (a long remember rule spans rows), so the block's
            // height and the highlighted option's first row come from the
            // renderer's own per-option heights — the seat lands on the `❯`
            // row however tall the labels are.
            let heights = super::permission_view::option_heights(
                &prompt.request,
                app.project_dir(),
                area.width,
            );
            let selected = prompt.selected.min(heights.len().saturating_sub(1));
            let total: usize = heights.iter().sum();
            let before: usize = heights[..selected].iter().sum();
            (
                area.height
                    .saturating_sub(PERMISSION_TAIL_ROWS.saturating_add(total as u16))
                    + before as u16,
                0,
            )
        };
        let y = area.y + row.min(area.height.saturating_sub(1));
        let x = area.x
            + ((cols(PERMISSION_INDENT) + cols(PERMISSION_MARKER) + ccol)
                .min(usize::from(area.width.saturating_sub(1))) as u16);
        return (x, y);
    }
    // The inline `/model` picker parks the cursor at the end of its `>` search
    // line (see `model_view_lines`' stack: top rule, gap, search) — inside the
    // picker's own frame, which the streaming strip above it has pushed down
    // ([`view_split`], the same split the paint uses) and the bottom anchor
    // may have shifted up ([`anchored_view_row`]).
    if let Some(picker) = &app.model_picker {
        let rows = model_picker_rows(picker, area.width);
        let [_, body] = view_split(area, rows);
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&picker.query);
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let row = anchored_view_row(rows, body, MODEL_SEARCH_ROW);
        return (x, view_cursor_y(area, body, row));
    }
    // The inline `/settings` menu parks the cursor at the end of its `❯`
    // search line, exactly like the `/model` picker (docs/settings.md).
    if let Some(picker) = &app.settings_picker {
        let rows = super::settings_view::settings_rows(app, area.width);
        let [_, body] = view_split(area, rows);
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&picker.query);
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let row = anchored_view_row(rows, body, SETTINGS_SEARCH_ROW);
        return (x, view_cursor_y(area, body, row));
    }
    // The inline `/mascot` picker parks the cursor at the end of its `❯`
    // search line, exactly like the `/settings` menu (docs/mascot.md).
    if let Some(picker) = &app.mascot_picker {
        let rows = super::mascot_view::mascot_menu_rows(app, area.width);
        let [_, body] = view_split(area, rows);
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&picker.query);
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let row = anchored_view_row(rows, body, MASCOT_SEARCH_ROW);
        return (x, view_cursor_y(area, body, row));
    }
    // The inline `/skills` menu parks the cursor at the end of its `❯` search
    // line, exactly like the `/settings` menu (docs/skills.md).
    if let Some(menu) = &app.skills_menu {
        let rows = super::skills_view::skills_menu_rows(app, area.width);
        let [_, body] = view_split(area, rows);
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&menu.query);
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let row = anchored_view_row(rows, body, SKILLS_SEARCH_ROW);
        return (x, view_cursor_y(area, body, row));
    }
    // The read-only `/hooks` menu has no text entry at all — the permission
    // prompt's rule: [`cursor_visible`] shows no hardware cursor over a menu
    // (a kitty cursor animation blinks at whatever seat one picks), while
    // the *seat* tracks the highlighted `❯` row, so the cursor's return
    // when the menu closes starts somewhere sensible. A marker-less page
    // (the hook detail) falls back to the far corner. The `/trust` review
    // menu is its sibling and seats the same way.
    if app.hooks_menu.is_some() {
        let lines = super::hooks_view::hooks_view_lines(app, area.width);
        return menu_marker_seat(&lines, area);
    }
    if app.trust_menu.is_some() {
        let lines = super::trust_view::trust_view_lines(app, area.width);
        return menu_marker_seat(&lines, area);
    }
    // The `/mcp` manager seats on its `❯` too — except its auth page,
    // whose `URL >` paste field seats the caret at the typed text's end
    // (found from the region's bottom: the field sits a fixed 4 rows up —
    // its row, the blank, the return note, the blank, the rule — plus the
    // `Checking…` row while a submission runs). See `docs/mcp.md`.
    if let Some(menu) = &app.mcp_menu {
        if menu.page == crate::app::McpPage::Auth {
            let extra = if menu.auth.submitted { 1 } else { 0 };
            let x =
                cols(MODEL_INDENT)
                    + cols(super::theme::MCP_AUTH_PROMPT)
                    + cols(&menu.auth.input).min(usize::from(area.width).saturating_sub(
                        cols(MODEL_INDENT) + cols(super::theme::MCP_AUTH_PROMPT) + 1,
                    ));
            let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
            let y = area.y + area.height.saturating_sub(5 + extra);
            return (x, y);
        }
        let lines = super::mcp_view::mcp_view_lines(app, area.width);
        return menu_marker_seat(&lines, area);
    }
    // The ↓ background manager band has no text entry at all — park the
    // (shown-once-per-frame) cursor in the band's far corner where it reads
    // as chrome, not input.
    if app.background_view.is_some() {
        let x = area.x + area.width.saturating_sub(1);
        let y = area.y + area.height.saturating_sub(1);
        return (x, y);
    }
    // The inline `/login` flow parks the cursor at the end of its active `>`
    // line: the provider filter (step 1) or the masked key field (step 2) —
    // again inside its own frame, below the strip, shifted by the anchor.
    if let Some(onboarding) = &app.key_onboarding {
        let (query_cols, row) = match onboarding.step {
            // The method step is the root and carries no title, so its filter
            // sits two rows higher than the titled lists below it.
            KeyStep::Method => (cols(&onboarding.query), LOGIN_METHOD_SEARCH_ROW),
            KeyStep::Subscription | KeyStep::Provider => {
                (cols(&onboarding.query), LOGIN_SEARCH_ROW)
            }
            // The device page has no field; park the (hidden — see
            // `cursor_visible`) cursor on its code box rather than the corner.
            KeyStep::Device => (0, DEVICE_CODE_ROW),
            // One mask glyph per key character sits after the prompt.
            KeyStep::Key => (onboarding.key_input.chars().count(), LOGIN_KEY_INPUT_ROW),
        };
        let rows = key_onboarding_rows(onboarding, area.width);
        let [_, body] = view_split(area, rows);
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + query_cols;
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let row = anchored_view_row(rows, body, row);
        return (x, view_cursor_y(area, body, row));
    }
    // Laid out exactly as render_live lays the box out — the streaming strip
    // and queued rows above, the band and footer below — so the cursor sits on
    // the prompt row even mid-turn (codex keeps the composer focused while a
    // task runs: typing edits the draft, Enter queues it).
    let band = band_rows(app, area.width);
    let footer = footer_rows(app, band);
    // The same fit `render_live_with_preview` lays the box out with, off the
    // same region rect — the two must agree or the caret leaves the prompt.
    let preview = fitted_preview_rows(app, area.width, area.height);
    let has_status = strip_has_status(app);
    let toast = toast_rows(app);
    let tasks = super::tasks::task_rows(app, area.width);
    // While a Ctrl+R search is open the hardware cursor tracks the end of the
    // *footer query*, not the textarea preview — the shell reverse-i-search
    // feel (codex's history_search_cursor_pos), clamped inside the row.
    let agent_rows = agent_list_rows(app);
    if let Some(search) = &app.history_search {
        let [_, _, _, footer_area, _] = live_layout(
            area,
            has_status,
            preview,
            tasks,
            queued_rows(app, area.width),
            toast,
            band,
            footer,
            agent_rows,
        );
        if footer_area.height > 0 && footer_area.width > 0 {
            let x = (cols(FOOTER_INDENT) + cols(SEARCH_PROMPT) + cols(&search.query))
                .min(usize::from(footer_area.width.saturating_sub(1))) as u16;
            return (footer_area.x.saturating_add(x), footer_area.y);
        }
    }
    let bx = input_box(
        area,
        &app.input,
        has_status,
        preview,
        tasks,
        queued_rows(app, area.width),
        toast,
        band,
        footer,
        agent_rows,
    );
    // The frame can collapse below its two rules (a tall strip/queue on a
    // short terminal): `Rect::inner` then returns `Rect::ZERO`, whose origin
    // says nothing about where the box is. Park the cursor on the region's
    // last row instead of teleporting it to the screen's top-left, over the
    // scrollback.
    if bx.text.height == 0 || bx.text.width == 0 {
        let x = area.x + BULLET_WIDTH.min(area.width.saturating_sub(1));
        let y = area.y + area.height.saturating_sub(1);
        return (x, y);
    }
    let row = bx.cursor_row.saturating_sub(bx.scroll) as u16;
    let col = bx.cursor_col as u16;
    (bx.text.x + BULLET_WIDTH + col, bx.text.y + row)
}

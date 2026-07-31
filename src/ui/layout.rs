//! The live region's geometry: how tall it is, where it re-pins on a resize,
//! and where the cursor sits inside it.
//!
//! Pure policy — `term.rs` calls these and does the terminal I/O itself
//! (`docs/design.md`). The rows it sizes are the streaming strip
//! (`docs/status-indicator.md`), the bands (`docs/shortcuts.md`,
//! `docs/file-search.md`) and the footer (`docs/footer.md`).

use crate::permission::OPTION_COUNT;

use super::agent::agent_view_preview_lines;
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
/// The **queued messages** (`queued_rows`) stack below this, between the strip
/// and the box's top rule — added separately by [`live_height`]/[`live_layout`]
/// since their height depends on the queue.
pub(super) const fn strip_rows(has_status: bool, preview_rows: u16) -> u16 {
    let preview = if preview_rows > 0 {
        preview_rows + GAP_ROWS
    } else {
        0
    };
    let status = if has_status {
        STATUS_ROWS + STATUS_GAP_ROWS
    } else {
        0
    };
    preview + status
}

/// The number of **preview** content rows the streaming strip reserves for `app`
/// at `width` (0 during the pre-stream pause / when idle, so the strip drops the
/// preview slot and its gap rather than leaving a stray blank — codex's
/// behaviour). A running backend tool previews its *whole* collapsed cell (the
/// wrapped header + `⎿ Running…`) so a long command isn't clipped live and the
/// running state shows; a running `!` shell command and a streaming reply preview
/// a single row. Must match exactly what [`render_live_with_preview`] draws (it
/// sizes the layout from the same `preview_lines`). Used by
/// [`render_live`]/[`cursor_position`]/`main.rs` to feed `strip_rows`.
#[must_use]
pub fn preview_rows(app: &App, width: u16) -> u16 {
    // An agent session view previews the *viewed agent's* stream — its live
    // tool cells or its reply's last row (docs/agent-tool.md).
    if let Some(run) = app.viewed_agent() {
        return u16::try_from(agent_view_preview_lines(run, app.pulse(), width).len())
            .unwrap_or(u16::MAX);
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
    // A streaming reply previews its last row — or, while a table is forming,
    // the whole forming block: only the boundary's `StreamRender` knows that
    // height, so it injects the count each frame via
    // [`App::set_stream_preview_rows`] (the `set_status_times` pattern;
    // docs/table-streaming.md). 1 when nothing was injected — the single-row
    // preview every non-table reply (and the render fallback) uses. The
    // pre-stream pause / idle reserve none.
    if app.streaming_text().is_some_and(|t| !t.is_empty()) {
        app.stream_preview_rows().max(1)
    } else {
        0
    }
}

/// The cap the boundary passes to [`StreamRender::preview`]: how many strip
/// rows a multi-row (forming-table) preview may take at this terminal height
/// before it tail-follows its newest rows — the screen minus the live-region
/// chrome (`STREAM_PREVIEW_RESERVED_ROWS`), floored at
/// `STREAM_PREVIEW_MIN_ROWS` (docs/table-streaming.md).
#[must_use]
pub fn stream_preview_max_rows(term_height: u16) -> usize {
    usize::from(term_height.saturating_sub(STREAM_PREVIEW_RESERVED_ROWS))
        .max(STREAM_PREVIEW_MIN_ROWS)
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
    (usize::from(strip_rows(has_status, preview_rows))
        + usize::from(queued_rows)
        + usize::from(toast_rows)
        + usize::from(INPUT_CHROME_ROWS)
        + rows
        + usize::from(band_rows)
        + usize::from(footer_rows)
        + usize::from(agent_rows))
    .min(usize::from(term_height.max(1))) as u16
}

/// Whether the picker shows its counter + model-name detail rows below the
/// list — only when a real model is listed (Ready with at least one match).
/// The placeholder states have a blank counter and name, so those rows collapse
/// to a single trailing gap ([`MODEL_CHROME_ROWS_COLLAPSED`]).
pub(super) fn model_has_detail(picker: &ModelPicker) -> bool {
    picker.status == ModelLoad::Ready && !picker.matches().is_empty()
}

/// The fixed framing rows for the picker's current state — full when a model is
/// highlighted, collapsed for a placeholder. Mirrors [`render_model_picker`]'s
/// two layouts so [`model_picker_height`] reserves exactly what's painted.
fn model_chrome_rows(picker: &ModelPicker) -> u16 {
    if model_has_detail(picker) {
        MODEL_CHROME_ROWS
    } else {
        MODEL_CHROME_ROWS_COLLAPSED
    }
}

/// How many rows the inline `/model` picker's **list** occupies: one placeholder
/// row while loading / errored / empty, else the match count capped at
/// [`MODEL_MENU_MAX_ROWS`]. Must equal `model_list_lines(..).len()` so the
/// reserved height and the painted rows agree.
fn model_list_rows(picker: &ModelPicker) -> u16 {
    match &picker.status {
        ModelLoad::Ready => {
            let n = picker.matches().len();
            if n == 0 {
                1
            } else {
                (n as u16).min(MODEL_MENU_MAX_ROWS)
            }
        }
        // All providers failed → one row per failed provider (else a single
        // placeholder for the legacy single-message error).
        ModelLoad::Error(_) => (picker.errors.len() as u16).max(1),
        // Loading / NeedsLogin → a single placeholder row.
        _ => 1,
    }
}

/// The inline live-region height when the `/model` picker is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). The picker
/// **replaces** the composer, so this is the whole region — the chrome plus the
/// (possibly scrolled) list — clamped to the terminal height. Shared by
/// `main.rs`'s `live_region_height`, [`render_live`], and [`cursor_position`]
/// so all three agree.
#[must_use]
pub fn model_picker_height(app: &App, term_height: u16) -> Option<u16> {
    let picker = app.model_picker.as_ref()?;
    Some((model_chrome_rows(picker) + model_list_rows(picker)).min(term_height.max(1)))
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
/// the `/model` picker it **replaces** the composer. The band's rows never
/// wrap (every line is truncated to the width), so the height is
/// width-independent — it is simply the built line count
/// ([`background_view_lines`]), clamped to the terminal. See
/// `docs/background.md`.
#[must_use]
pub fn background_view_height(app: &App, term_height: u16) -> Option<u16> {
    app.background_view.as_ref()?;
    let rows = background_view_lines(app, 80).len() as u16;
    Some(rows.min(term_height.max(1)))
}

/// How many rows the `/login` provider list occupies: the match count capped at
/// [`LOGIN_MENU_MAX_ROWS`], or a single placeholder row when nothing matches.
/// Must equal `login_provider_list_lines(..).len()` so the reserved height and
/// the painted rows agree.
fn login_provider_list_rows(onboarding: &KeyOnboarding) -> u16 {
    let n = onboarding.matches().len();
    if n == 0 {
        1
    } else {
        (n as u16).min(LOGIN_MENU_MAX_ROWS)
    }
}

/// The inline live-region height when the `/login` flow is open, or `None` when
/// it isn't (the caller falls back to [`live_height`]). Like the `/model`
/// picker it **replaces** the composer; the provider step grows with its list,
/// the key step is a fixed height. Shared by `main.rs`'s `live_region_height`,
/// [`render_live`], and [`cursor_position`].
#[must_use]
pub fn key_onboarding_height(app: &App, term_height: u16) -> Option<u16> {
    let onboarding = app.key_onboarding.as_ref()?;
    let rows = match onboarding.step {
        KeyStep::Provider => LOGIN_PROVIDER_CHROME_ROWS + login_provider_list_rows(onboarding),
        KeyStep::Key => LOGIN_KEY_ROWS,
    };
    Some(rows.min(term_height.max(1)))
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

/// Whether the live region is an **inline modal**: a view that covers the
/// conversation rather than growing the region past it.
///
/// Only the tool-permission prompt is one (`docs/permissions.md`). It is the
/// single inline view that can be as tall as the whole terminal, and the one
/// the ordinary [`repin`] serves badly: growing to full height from a
/// bottom-seated composer scrolls a screenful of chat into scrollback, where
/// the collapse back to the composer can never get it back — leaving the box
/// stranded mid-screen with a band of blank rows beneath it. Every other band
/// and picker is short enough that the ordinary growth is right.
///
/// The sibling of [`strip_has_status`]: a pure predicate over `App` that
/// `term.rs` reads to pick its geometry ([`repin_modal`] instead of
/// [`repin`]), keeping the policy here and the I/O there.
#[must_use]
pub fn region_is_modal(app: &App) -> bool {
    app.permission().is_some()
}

/// The screen height an open inline modal's region takes, given the rows the
/// prompt itself needs (`prompt_rows`, the [`permission_height`] reading) and
/// how many committed conversation rows sit painted above the region
/// (`view_top`, the boundary's `InlineViewport::view_top`).
///
/// While the prompt fits between those rows and the screen bottom it keeps its
/// own height — it covers nothing, and the conversation above stays exactly
/// where it is. The moment it would need even one conversation row, the region
/// takes the **whole screen**: the render then replays the conversation tail
/// above the prompt ([`render_permission_with_context`]), so the newest
/// messages stay in view — visually the conversation "scrolls up" to make
/// room, Claude-Code style — while underneath it is still the reversible
/// covering ([`repin_modal`] seats the region at row 0 and counts every row,
/// and the close hands them all back). A partial cover can't do this: the
/// replay and the rows still painted above it would have to meet mid-screen,
/// and any shift between them tears the conversation. See
/// `docs/permissions.md`.
#[must_use]
pub fn modal_region_height(prompt_rows: u16, view_top: u16, screen_height: u16) -> u16 {
    let screen = screen_height.max(1);
    let prompt = prompt_rows.min(screen);
    if u32::from(view_top) + u32::from(prompt) > u32::from(screen) {
        screen
    } else {
        prompt
    }
}

/// How an **inline modal** ([`region_is_modal`]) re-pins: it takes the free
/// rows below the region first and then grows *upward*, covering the
/// conversation — and it **never scrolls**.
///
/// That is the whole point. A scroll is one-way: the rows it pushes into the
/// terminal's scrollback are gone from the screen, so when the prompt closes
/// the region shrinks with nothing to fill the rows it vacates and the box is
/// left floating above a band of blank ones. Covering is reversible — every
/// row a modal hides is still in `App`'s history, so closing it repaints them
/// in place and the box lands exactly where it started (`docs/permissions.md`).
///
/// A **shrink** behaves exactly like [`repin`] (top put, vacated rows below
/// blanked): the top only ever moves *up*, so no row above the region is ever
/// left stale.
#[must_use]
pub fn repin_modal(top: u16, old_height: u16, new_height: u16, screen_height: u16) -> Repin {
    let bottom = top.saturating_add(old_height);
    // Grow downward while there is room (the ordinary content-anchored growth,
    // invariant 3), then upward for whatever is left over.
    let up = new_height
        .saturating_sub(old_height)
        .saturating_sub(screen_height.saturating_sub(bottom));
    let new_top = top.saturating_sub(up);
    let new_bottom = new_top.saturating_add(new_height).min(screen_height);
    Repin {
        scroll_up: 0,
        top: new_top,
        clear_below: bottom.saturating_sub(new_bottom),
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
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
    agent_rows: u16,
) -> [Rect; 5] {
    Layout::vertical([
        // Saturating: `queued_rows` is uncapped, and the layout clamp below
        // (not this sum) is what bounds it to the area.
        Constraint::Length(
            strip_rows(has_status, preview_rows)
                .saturating_add(queued_rows)
                .saturating_add(toast_rows),
        ),
        Constraint::Min(0),
        Constraint::Length(band_rows),
        Constraint::Length(footer_rows),
        Constraint::Length(agent_rows),
    ])
    .areas(area)
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
#[must_use]
pub fn cursor_visible(app: &App) -> bool {
    app.permission().is_none_or(|prompt| prompt.amend)
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input — its resting seat, which [`cursor_visible`] decides whether
/// to actually show. Shares `input_box` with [`render_live`] so the cursor lands
/// exactly where the editor's cursor is — on its wrapped row, at its column —
/// wherever the user has moved it, not just at the end.
#[must_use]
pub fn cursor_position(area: Rect, app: &App) -> (u16, u16) {
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
            let options = OPTION_COUNT as u16;
            (
                area.height
                    .saturating_sub(PERMISSION_TAIL_ROWS.saturating_add(options))
                    + prompt.selected.min(OPTION_COUNT - 1) as u16,
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
    // line (see `render_model_picker`'s layout: top rule, header, gap, search).
    if let Some(picker) = &app.model_picker {
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&picker.query);
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let y = area.y + MODEL_SEARCH_ROW.min(area.height.saturating_sub(1));
        return (x, y);
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
    // line: the provider filter (step 1) or the masked key field (step 2).
    if let Some(onboarding) = &app.key_onboarding {
        let (query_cols, row) = match onboarding.step {
            KeyStep::Provider => (cols(&onboarding.query), LOGIN_SEARCH_ROW),
            // One mask glyph per key character sits after the prompt.
            KeyStep::Key => (onboarding.key_input.chars().count(), LOGIN_KEY_INPUT_ROW),
        };
        let x = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + query_cols;
        let x = area.x + (x.min(usize::from(area.width.saturating_sub(1))) as u16);
        let y = area.y + row.min(area.height.saturating_sub(1));
        return (x, y);
    }
    // Laid out exactly as render_live lays the box out — the streaming strip
    // and queued rows above, the band and footer below — so the cursor sits on
    // the prompt row even mid-turn (codex keeps the composer focused while a
    // task runs: typing edits the draft, Enter queues it).
    let band = band_rows(app);
    let footer = footer_rows(app, band);
    let preview = preview_rows(app, area.width);
    let has_status = strip_has_status(app);
    let toast = toast_rows(app);
    // While a Ctrl+R search is open the hardware cursor tracks the end of the
    // *footer query*, not the textarea preview — the shell reverse-i-search
    // feel (codex's history_search_cursor_pos), clamped inside the row.
    let agent_rows = agent_list_rows(app);
    if let Some(search) = &app.history_search {
        let [_, _, _, footer_area, _] = live_layout(
            area,
            has_status,
            preview,
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

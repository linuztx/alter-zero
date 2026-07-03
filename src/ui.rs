//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`live_height`], [`repin`], [`cursor_position`]),
//! build ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.

use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    App, HistoryItem, HistorySearch, QueuedTurn, Role, SearchState, SlashCommand, TokenArrow,
    ToolCall, ToolStatus, TurnStatus, TurnSummary, command_query, matching_commands,
};
use crate::file_search::FileMatch;
use crate::textarea::TextArea;

/// Display width of `s` in terminal columns.
///
/// All width math in this module goes through this instead of `chars().count()`:
/// CJK and many emoji are two columns wide and combining marks are zero, so a
/// raw char count would wrap and pad non-ASCII text incorrectly.
fn cols(s: &str) -> usize {
    s.width()
}

/// Display width of a single `char` in terminal columns (control chars → 0).
fn char_cols(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

// --- Claude-Code-ish styling. Centralised so it's trivial to retheme. ---

/// Prompt shown at the start of the input field.
const PROMPT: &str = "❯ ";
/// Bullet prefixing a user message.
const USER_BULLET: &str = "❯ ";
/// Bullet prefixing an assistant message.
const AI_BULLET: &str = "● ";
/// Bullet prefixing a backend-error notice — same glyph as the assistant, but
/// coloured red (see [`ERROR_COLOR`]) so a failure reads as a red bullet point.
const ERROR_BULLET: &str = "● ";
/// Bullet prefixing a system notice (slash-command output) — same glyph, coloured
/// cyan (see [`SYSTEM_COLOR`]) so it reads as meta rather than an AI reply.
const SYSTEM_BULLET: &str = "● ";
/// Indent for wrapped continuation lines (matches a bullet's width).
const INDENT: &str = "  ";
/// Columns a bullet/indent occupies, subtracted from the content width.
const BULLET_WIDTH: u16 = 2;

const USER_COLOR: Color = Color::Rgb(0x6E, 0x6E, 0x6E);
const USER_BG_COLOR: Color = Color::Rgb(0x2D, 0x2D, 0x2D);
const AI_COLOR: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const ERROR_COLOR: Color = Color::Rgb(0xE0, 0x6C, 0x75);
/// Cyan — a system notice's bullet (slash-command output).
const SYSTEM_COLOR: Color = Color::Rgb(0x56, 0xB6, 0xC2);
const PROMPT_COLOR: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const BORDER_COLOR: Color = Color::Rgb(0xAA, 0xAA, 0xAA);

// --- Tool-call styling. A tool renders as a coloured bullet header
// `● name(args)` plus a collapsed `⎿` peek of its output; the bullet colour is
// the tool's lifecycle (blue running, green ok, red fail). The full output is
// only shown in the Ctrl+O tool-output view, never inline. ---

/// Bullet prefixing a tool call (same glyph as the assistant, recoloured by
/// status — see [`tool_status_color`]).
const TOOL_BULLET: &str = "● ";
/// Prefix for the first result line: indent + a turnstile glyph. Continuation
/// lines are indented by its display width so a multi-line result aligns under
/// it (see [`result_row`]).
const TOOL_RESULT_PREFIX: &str = "  ⎿ ";
/// Prefix for the "+N lines" hint line under a capped peek.
const TOOL_MORE_PREFIX: &str = "    … ";
/// Hint telling the user how to see the full output.
const EXPAND_HINT: &str = " (ctrl+o to expand)";
/// How many output lines a `!` shell command shows inline before collapsing the
/// rest behind a `… +N lines (ctrl+o to expand)` hint (Claude-Code's exec-cell
/// preview). The full output is always in the Ctrl+O view.
const TOOL_PEEK_LINES: usize = 4;
/// Dim marker appended at the end of a `!` shell command's **expanded** output
/// (`tool_full_lines`) when it was cut at the in-memory cap (`tool.truncated`),
/// to show that more output was dropped. See `docs/shell-command.md`.
const TOOL_TRUNCATED_MARKER: &str = "…";
/// Placeholder body for a finished tool that produced no output.
const TOOL_NO_OUTPUT: &str = "(no output)";
/// Placeholder body for a still-executing `!` shell command — sentence case,
/// since the `⎿ Running…` row *opens* the headerless cell.
const TOOL_RUNNING: &str = "Running…";
/// Placeholder peek for a still-executing backend tool — lowercase, sitting
/// *under* its `● name(args)` header.
const TOOL_RUNNING_LOWER: &str = "running…";

/// Blue — a tool that is still executing.
const TOOL_RUNNING_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// Green — a tool that finished successfully.
const TOOL_OK_COLOR: Color = Color::Rgb(0x98, 0xC3, 0x79);
/// Red — a tool that failed (shares the backend-error red).
const TOOL_FAIL_COLOR: Color = ERROR_COLOR;
/// White — the tool's name.
const TOOL_NAME_COLOR: Color = AI_COLOR;
/// Dim grey — a tool's argument summary and its collapsed peek/hint.
const TOOL_DIM_COLOR: Color = Color::Rgb(0x8A, 0x8A, 0x8A);

/// The running placeholder for a tool — the shell-vs-backend casing rule
/// ([`TOOL_RUNNING`] / [`TOOL_RUNNING_LOWER`]) in one place, so `tool_lines`
/// and `tool_full_lines` can never drift apart.
fn tool_running_marker(shell: bool) -> &'static str {
    if shell {
        TOOL_RUNNING
    } else {
        TOOL_RUNNING_LOWER
    }
}

// --- Tool-output view (the Ctrl+O full-screen overlay). A one-row title above a
// scrolling body: the full conversation transcript — every message plus every
// tool call's *complete* (expanded) output. ---

/// Title shown at the top of the tool-output view.
const TOOL_VIEW_TITLE: &str = "Conversation";
/// Key hint shown beside the title.
const TOOL_VIEW_HINT: &str = "  ↑/↓ PgUp/PgDn scroll · ctrl+o / esc return";
/// Rows of chrome above the scrolling body (just the title row).
const TOOL_VIEW_TITLE_ROWS: u16 = 1;
/// The dim placeholder shown when the transcript has nothing to list yet.
const TOOL_VIEW_EMPTY: &str = "Nothing here yet.";

// --- Transcript timestamps (Ctrl+O view only). Only the *user* message shows
// its wall-clock stamp: dim, right-aligned on its own line below the message
// (`hh:mm AM/PM`). AI replies, tools, and turn summaries record a stamp too but
// never display it; the inline view never shows any. See docs/timestamps.md. ---

/// Dim grey — the user message's right-aligned timestamp in the Ctrl+O transcript.
const TIMESTAMP_COLOR: Color = TOOL_DIM_COLOR;

// --- Live status indicator (codex / Claude-Code style). While a turn is in
// flight a status line sits in the strip above the box (with a blank gap row
// between it and the box's top rule):
// `(●•·   ) {verb}… ({elapsed}s · {↓|↑} {n} tokens · Thinking for {m}s)`. The
// line opens with a **comet spinner** (a Larson-scanner sweep: a white head
// dragging a fading grey tail back and forth between dim walls, one frame per
// `SPINNER_INTERVAL` — see [`spinner_spans`]); the working verb is picked
// per-turn (in `App`) and its white text carries a codex-style **shimmer**: a
// bright-white band sweeps across the white-grey text (see [`shimmer_spans`],
// ported from openai/codex `tui/src/shimmer.rs`). On finish a dim, bullet-less
// `{done verb} for {n}s` summary commits to scrollback (a
// `HistoryItem::Summary`). See docs/status-indicator.md. ---

/// White — the comet's head (matches the codex/Claude-Code white status text).
const STATUS_COLOR: Color = AI_COLOR;
/// The comet-spinner animation frames (a Larson-scanner sweep, ten frames):
/// the bright head (`●`) drags a two-cell fading tail (`•` then `·`) out to
/// the right wall and back across to the **left wall** (flush against `(` —
/// no wasted leading cell), the tail whipping around behind it at each
/// bounce (a tail cell the head overlaps is hidden under it). Every frame is
/// the same width, so the verb after it never jitters.
const SPINNER_FRAMES: &[&str] = &[
    "(●•·   )",
    "(•●    )",
    "(·•●   )",
    "( ·•●  )",
    "(  ·•● )",
    "(   ·•●)",
    "(    ●•)",
    "(   ●•·)",
    "(  ●•· )",
    "( ●•·  )",
];
/// How long each spinner frame shows (the classic cli-spinners 80 ms cadence —
/// well under the loop's ~30 fps animation re-arm, so no frame is skipped).
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);
/// The comet's bright bold head in a [`SPINNER_FRAMES`] frame.
const SPINNER_HEAD: char = '●';
/// The tail cell right behind the head; the `·` end (and everything else in
/// the frame — walls, empty track) fades to [`STATUS_DETAIL_COLOR`].
const SPINNER_TAIL_MID: char = '•';
/// Mid grey — the `•` tail cell, between the white head and the dim tail end.
const SPINNER_TAIL_COLOR: Color = Color::Rgb(0xC8, 0xC8, 0xC8);
/// How many spans [`spinner_spans`] emits (one per frame cell: the left wall,
/// six track cells, the right wall) — the verb's per-char spans start at this
/// index in the status line.
const SPINNER_SPAN_COUNT: usize = 8;
/// Dim grey — the parenthesised metrics (`elapsed · tokens · thinking`).
const STATUS_DETAIL_COLOR: Color = TOOL_DIM_COLOR;
/// Trailing ellipsis after the working verb (`Working…`).
const STATUS_ELLIPSIS: &str = "…";
/// Arrow for output tokens while the reply streams.
const STATUS_ARROW_DOWN: &str = "↓";
/// Arrow once a tool result is folded back in.
const STATUS_ARROW_UP: &str = "↑";
/// The interrupt hint, the detail's final clause while a turn is in flight —
/// codex's `Esc to interrupt` discoverability hint, lowercased to match this
/// codebase's hint convention (`(ctrl+o to expand)`, `esc return`).
const STATUS_INTERRUPT_HINT: &str = "esc to interrupt";
/// Dim grey — the committed `"{done verb} for {n}s"` turn summary.
const STATUS_DONE_COLOR: Color = TOOL_DIM_COLOR;
/// The status line's row in the streaming strip.
const STATUS_ROWS: u16 = 1;
/// A blank row between the status line and the box's top rule, so the status
/// never butts up against the box (mirrors the gap above, under the preview).
const STATUS_GAP_ROWS: u16 = 1;

// --- The verb's shimmer wave (ported from openai/codex `shimmer_spans`): each
// char's colour blends from the white-grey base toward bright white by a
// raised-cosine band that sweeps the text once per `SHIMMER_SWEEP`. The wave's
// phase derives from the boundary-supplied `TurnStatus::elapsed`, keeping the
// renderer pure (codex reads a process clock instead). ---

/// The white-grey base of the shimmering verb text (codex's truecolor fallback
/// foreground) — dim enough that the bright band reads clearly.
const SHIMMER_BASE: (u8, u8, u8) = (0x88, 0x88, 0x88);
/// The bright white the band's crest blends toward.
const SHIMMER_HIGHLIGHT: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
/// One full sweep of the band across the text (codex's `sweep_seconds`).
const SHIMMER_SWEEP: Duration = Duration::from_secs(2);
/// Off-text run-in/out, in chars, so the band slides on and off the ends
/// instead of wrapping abruptly (codex's `padding`).
const SHIMMER_PADDING: usize = 10;
/// The band's half-width in chars (codex's `band_half_width`).
const SHIMMER_BAND_HALF_WIDTH: f32 = 5.0;
/// The crest's blend toward the highlight (codex blends `t * 0.9`).
const SHIMMER_MAX_BLEND: f32 = 0.9;

// --- Slash-command palette. A scrolling, single-line-per-command list pinned
// **below the input box** (a third live-region band) whenever the input is a bare
// command token. Each row is `/name` padded to a column, then its description. The
// selection is shown **by colour**: the whole highlighted row lights up cyan — name
// *and* description the same colour — while the others are dimmed grey (no
// caret/arrow), Claude-Code style. Capped at `MENU_MAX_ROWS`; longer lists scroll
// to keep the selection visible (`menu_window`). ---

/// The most command rows shown at once; longer match lists scroll within this.
const MENU_MAX_ROWS: u16 = 5;
/// The column descriptions start at — names are padded out to here so the
/// descriptions line up in a tidy column regardless of command-name length.
const MENU_DESC_COL: usize = 25;
/// Cyan — the **selected** row: its `/name` *and* description share this colour
/// (for consistency); the name is additionally bold.
const MENU_SELECTED_COLOR: Color = Color::Rgb(0x56, 0xB6, 0xC2);
/// Dim grey — an unselected row (name and description alike).
const MENU_DIM_COLOR: Color = TOOL_DIM_COLOR;
/// The palette's single placeholder row when the `/token` matches no command.
const MENU_NO_MATCH: &str = "No matching commands";

// --- The `@` file picker. A file list pinned **below the input box** (the
// palette's slot — the bands never show together), opened by an `@token` under
// the cursor — a port of codex's file-search popup. It reuses the palette's
// cyan-selected / dim-unselected colours, additionally **bolding the characters
// the query matched** (from `FileMatch.indices`). See docs/file-search.md. ---

/// The most file rows shown at once; longer match lists scroll to keep the
/// selection visible (`menu_window`), like the command palette.
const FILE_MENU_MAX_ROWS: u16 = 8;
/// The picker's single placeholder row while a search is in flight.
const FILE_MENU_SEARCHING: &str = "Searching…";
/// The picker's single placeholder row when the query matched nothing.
const FILE_MENU_NO_MATCH: &str = "No matching files";

// --- The `?` shortcuts band. A keyboard-shortcuts overview pinned **below the
// input box** (the palette's slot — the two never show together), toggled by
// `?` from an empty composer — a port of codex's footer shortcut overlay
// (`footer.rs::shortcut_overlay_lines`): two aligned columns of
// `{key} for {thing}` entries, keys cyan, labels dim. See docs/shortcuts.md. ---

/// The bindings listed in the band, as `(key, label)` pairs laid out two per
/// row in declaration order. The `esc` entry's label is context-sensitive —
/// [`shortcuts_lines`] swaps it for ` to interrupt` while a turn runs (codex's
/// quit entry does the same).
const SHORTCUTS: &[(&str, &str)] = &[
    ("/", " for commands"),
    ("!", " for shell command"),
    ("↑", " for input history"),
    ("ctrl+r", " to search history"),
    ("alt+enter", " for newline"),
    ("ctrl+o", " for tool output"),
    ("esc", " to quit"),
    ("ctrl+c", " to quit"),
    ("alt+↑", " to edit queue"),
    ("tab", " to queue next turn"),
    ("ctrl+v", " for image paste"),
];
/// The display column where a row's second entry starts (the first entry is
/// padded out to here) — [`MENU_DESC_COL`]'s tidy-column idea.
const SHORTCUTS_COL: usize = 25;
/// Cyan — an entry's key (the palette-selection accent).
const SHORTCUTS_KEY_COLOR: Color = MENU_SELECTED_COLOR;
/// Dim grey — an entry's label (codex dims the whole overlay).
const SHORTCUTS_TEXT_COLOR: Color = TOOL_DIM_COLOR;

// --- Queued messages. While a turn streams, messages submitted with Enter (or
// Tab) join `App::queued` and are shown **above the box** (in the strip, just
// under the status line's gap) styled exactly like a sent user message — the
// `❯ ` bullet, the dark background, wrapped — so a queued follow-up reads like
// it is already on its way. The queue is a sequence of turn-batches: Enter
// appends to the current batch, **Tab opens a new one**, and the loop sends one
// batch per turn-end (a blank row divides the batches). See docs/queue.md. ---

/// Indent prefixed to every queued row, insetting the queue from the strip's
/// left edge; the dark user-message block starts after it.
const QUEUED_INDENT: &str = "  ";

// --- The session-context footer: the dim `{model} · {cwd}` row pinned under
// the input box (codex's footer status line). See docs/footer.md. ---

/// Indent prefixed to the footer row (codex's `FOOTER_INDENT_COLS`).
const FOOTER_INDENT: &str = "  ";
/// Separator between the footer's segments (codex's dim ` · `).
const FOOTER_SEPARATOR: &str = " · ";
/// The footer's text colour — every segment dim, codex's no-theme-colours
/// status-line style.
const FOOTER_COLOR: Color = TOOL_DIM_COLOR;

// --- The Ctrl+R reverse history search line (codex's reverse-i-search footer,
// `chat_composer/history_search.rs::history_search_footer_line`). It takes the
// session footer's slot while a search is open, and the previewed match in the
// composer highlights the query occurrences. See docs/history-search.md. ---

/// The dim prompt opening the search line.
const SEARCH_PROMPT: &str = "reverse-i-search: ";
/// Cyan — the query text and the accept/cancel hint keys (codex's `.cyan()`;
/// the palette-selection accent).
const SEARCH_QUERY_COLOR: Color = MENU_SELECTED_COLOR;
/// The notice appended to the line when the query matches nothing — red, like
/// codex's `"  no match"`.
const SEARCH_NO_MATCH: &str = "  no match";
/// How a previewed match's query occurrences light up in the input box
/// (codex's `REVERSED | BOLD` textarea highlight).
const SEARCH_HIGHLIGHT: Modifier = Modifier::REVERSED.union(Modifier::BOLD);

// --- The `!` shell-mode footer hint. While the composer holds a `!command`
// the footer slot reads `Shell mode` in red (codex's light-red
// `shell_mode_footer_line`), displacing the `{model} · {cwd}` line. See
// docs/shell-command.md. ---

/// The shell-mode hint text.
const SHELL_MODE_LABEL: &str = "Shell mode";
/// The hint's colour — red, like codex's `light_red()` (reuses our error red).
/// Also colours the `! ` bullet/prompt everywhere shell mode shows.
const SHELL_MODE_COLOR: Color = ERROR_COLOR;
/// The bullet opening a committed shell command's header (`! pwd` on the dark
/// user-style line) — and the composer prompt while shell mode is on (the
/// absorbed bang rendered back; same two columns as [`PROMPT`]).
const SHELL_BULLET: &str = "! ";

// --- Live-region geometry. The bottom region's height is dynamic: it grows with
// the wrapped input (see `live_height`). `render_live` and `cursor_position` both
// derive their layout from `input_box` so the drawn text and cursor never drift;
// `main.rs`/`term.rs` size the viewport from `live_height`/`LIVE_MIN_HEIGHT`. ---

/// Rows in the streaming-preview line shown above the input box (the in-progress
/// reply's last, not-yet-committed line). Shown only while a reply streams.
const PREVIEW_ROWS: u16 = 1;
/// A blank gap row between the streaming preview and the box, so the live reply
/// never butts up against the box's top rule. Present only while streaming.
const GAP_ROWS: u16 = 1;
/// The input box's non-text rows: a top rule and a bottom rule.
const INPUT_CHROME_ROWS: u16 = 2;
/// The smallest the live region ever gets: a one-text-row box framed by two
/// rules (idle has no preview strip). `main.rs` sizes the initial viewport from this.
pub const LIVE_MIN_HEIGHT: u16 = INPUT_CHROME_ROWS + 1;

/// Rows of the streaming strip above the box, shown only while a turn is active.
/// The **status line** and its trailing gap are always present during a turn; the
/// **preview line and its gap are only reserved when there is something to
/// preview** (`has_preview`: a running tool, or a reply whose buffer is
/// non-empty). During the pre-stream pause — and any moment before the first
/// chunk — there is no preview, so the strip is just status + gap and the box
/// sits one blank below the committed user message (codex doesn't reserve an
/// empty preview line). Idle, the strip collapses to nothing. The **queued
/// messages** (`queued_rows`) stack below this, between the status gap and the
/// box's top rule — added separately by [`live_height`]/[`live_layout`] since
/// their height depends on the queue.
const fn strip_rows(streaming: bool, has_preview: bool) -> u16 {
    if !streaming {
        return 0;
    }
    let preview = if has_preview {
        PREVIEW_ROWS + GAP_ROWS
    } else {
        0
    };
    preview + STATUS_ROWS + STATUS_GAP_ROWS
}

/// Whether the streaming strip reserves a **preview** row: a tool is running
/// (its coloured header / `⎿` peek previews) or the reply has actually begun
/// (a non-empty streaming buffer). False during the pre-stream pause, so the
/// strip drops the preview row and its gap rather than leaving a stray blank
/// (the user's "don't preserve a line" — codex's behaviour). Used by
/// [`render_live`]/[`cursor_position`]/`main.rs` to feed `strip_rows`.
#[must_use]
pub fn strip_has_preview(app: &App) -> bool {
    app.current_tool().is_some() || app.streaming_text().is_some_and(|t| !t.is_empty())
}

/// Columns the input field's text occupies: the box spans the full width (no side
/// borders) minus the prompt/indent that prefixes every text row.
fn field_width(width: u16) -> u16 {
    width.saturating_sub(BULLET_WIDTH).max(1)
}

/// Height of the bottom live region for the current `input` at this terminal
/// size: the streaming strip (only while `streaming`) plus the `queued_rows`
/// queued-message lines stacked under its status (the strip's
/// [`queued_rows`]), two framing rules, one row per wrapped input line — so the
/// box **grows** as the message wraps — the band below it (`band_rows`: the
/// command palette's [`menu_rows`] plus the shortcuts band's [`shortcuts_rows`],
/// 0 when both are closed), and the session-context footer under that
/// (`footer_rows`: [`footer_rows`], 0 when a band displaces it) — clamped to
/// the terminal height (after which the box scrolls internally; see
/// [`render_live`]).
// A flat list of irreducible geometry measurements — bundling them into a
// struct would only obscure the positional layout the tests assert directly.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn live_height(
    input: &TextArea,
    width: u16,
    term_height: u16,
    streaming: bool,
    has_preview: bool,
    queued_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> u16 {
    let rows = input.row_count(field_width(width)) as u16;
    (strip_rows(streaming, has_preview)
        + queued_rows
        + INPUT_CHROME_ROWS
        + rows
        + band_rows
        + footer_rows)
        .min(term_height.max(1))
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
/// status, gap, **and the `queued_rows` queued-message lines below them**
/// (height 0 when idle); the band — the command palette *or* the `?` shortcuts
/// overview — takes its fixed `band_rows` below the box (0 when closed); the
/// session-context footer sits on the very last `footer_rows` (0 when unset or
/// displaced by the band); the input box takes whatever rows remain in
/// between, so it **grows** as `area` grows (see [`live_height`]). Reserving
/// the band and footer below rather than between keeps the box's top — and the
/// cursor — put when they appear. The only place the split is expressed.
fn live_layout(
    area: Rect,
    streaming: bool,
    has_preview: bool,
    queued_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(strip_rows(streaming, has_preview) + queued_rows),
        Constraint::Min(0),
        Constraint::Length(band_rows),
        Constraint::Length(footer_rows),
    ])
    .areas(area)
}

/// The geometry shared by [`render_live`] and [`cursor_position`] so the drawn
/// text and the hardware cursor can never drift apart: where the input text rows
/// live, the input wrapped to the field width, the cursor's wrapped row/column,
/// and how far it's scrolled so the **cursor** stays visible when the box is full.
struct InputBox {
    /// The rule-framed box area (below the preview); borders are drawn here.
    frame: Rect,
    /// The inner area that holds the text rows (the box minus its two rules).
    text: Rect,
    /// Every wrapped input row's displayed text (always at least one, possibly empty).
    rows: Vec<String>,
    /// Each row's byte range into the input text (parallel to `rows`) — what
    /// maps the Ctrl+R search-highlight byte ranges onto row columns.
    row_ranges: Vec<Range<usize>>,
    /// The cursor's wrapped-row index and display column (from the [`TextArea`]).
    cursor_row: usize,
    cursor_col: usize,
    /// Index of the first wrapped row shown — the window follows the cursor.
    scroll: usize,
}

fn input_box(
    area: Rect,
    input: &TextArea,
    streaming: bool,
    has_preview: bool,
    queued_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> InputBox {
    let [_, frame, _, _] = live_layout(
        area,
        streaming,
        has_preview,
        queued_rows,
        band_rows,
        footer_rows,
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

/// Split one wrapped input row into spans, styling the parts inside
/// `highlights` with [`SEARCH_HIGHLIGHT`] — the Ctrl+R match preview
/// (codex highlights its textarea the same way). `row` is the text of the
/// row whose byte range into the whole input is `range`; `highlights` are
/// sorted, non-overlapping byte ranges into that same text
/// ([`App::search_highlight_ranges`]). With no highlights the row comes back
/// as one plain span.
fn highlight_row_spans(
    row: &str,
    range: &Range<usize>,
    highlights: &[Range<usize>],
) -> Vec<Span<'static>> {
    if highlights.is_empty() {
        return vec![Span::raw(row.to_string())];
    }
    let style = Style::new().add_modifier(SEARCH_HIGHLIGHT);
    let mut spans = Vec::new();
    let mut pos = range.start;
    for h in highlights {
        // This row's slice of the highlight (a match can span wrapped rows).
        let start = h.start.clamp(pos, range.end);
        let end = h.end.clamp(pos, range.end);
        if start >= end {
            continue;
        }
        if start > pos {
            spans.push(Span::raw(
                row[pos - range.start..start - range.start].to_string(),
            ));
        }
        spans.push(Span::styled(
            row[start - range.start..end - range.start].to_string(),
            style,
        ));
        pos = end;
    }
    if pos < range.end {
        spans.push(Span::raw(row[pos - range.start..].to_string()));
    }
    spans
}

/// Greedy word-wrap `text` to `width` columns.
///
/// - Existing `'\n'`s are honoured (and blank lines preserved).
/// - Words longer than `width` are hard-broken across lines.
/// - `width == 0` disables wrapping (text is only split on `'\n'`).
///
/// Crucially this is *prefix-stable*: appending more text only ever changes the
/// last produced line, which is what lets streaming commit completed lines to
/// scrollback (see `main.rs`).
#[must_use]
pub fn wrap_text(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(str::to_string).collect();
    }
    let width = width as usize;
    let mut out = Vec::new();
    for segment in text.split('\n') {
        if segment.split_whitespace().next().is_none() {
            out.push(String::new()); // blank line
            continue;
        }
        out.extend(wrap_segment(segment, width));
    }
    out
}

/// Greedy-wrap a single newline-free segment that has at least one word.
///
/// All length comparisons are in display columns (see [`cols`]), so wide CJK
/// glyphs count as two and zero-width marks as zero.
fn wrap_segment(segment: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0; // display width of `cur` in columns

    for word in segment.split_whitespace() {
        let word_w = cols(word);

        if word_w > width {
            // Hard-break a word that can't fit on any line, splitting on
            // **grapheme** boundaries (like `textarea::place_word`) so a ZWJ
            // emoji cluster is never severed mid-joiner; measured in columns —
            // a single cluster that overflows a narrow line is placed alone (a
            // grapheme can't be split further).
            if cur_w > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for g in word.graphemes(true) {
                let g_w = cols(g);
                if cur_w > 0 && cur_w + g_w > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push_str(g);
                cur_w += g_w;
            }
            continue;
        }

        let needed = if cur_w == 0 {
            word_w
        } else {
            cur_w + 1 + word_w
        };
        if needed > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = word_w;
        } else {
            if cur_w > 0 {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(word);
            cur_w += word_w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Wrap `text` to `width` columns **preserving whitespace verbatim** — the
/// counterpart of [`wrap_text`] for pre-formatted output (a tool's captured
/// stdout: `ls -l` columns, `tree` guides, indented code), where collapsing
/// space runs would destroy the alignment. Each `'\n'`-separated line keeps
/// its bytes exactly; a line wider than `width` is hard-broken on grapheme
/// boundaries, measured in display columns like [`wrap_segment`]'s hard break
/// (an overflowing cluster is placed alone — it can't be split further).
/// `width == 0` disables wrapping, like [`wrap_text`]. Used by
/// [`tool_full_lines`], so the Ctrl+O expanded output is at least as faithful
/// as the inline peek's `truncate_cols`; **messages** keep [`wrap_text`].
fn wrap_verbatim(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(str::to_string).collect();
    }
    let width = width as usize;
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut cur_w = 0; // display width of `cur` in columns
        for g in line.graphemes(true) {
            let g_w = cols(g);
            if cur_w > 0 && cur_w + g_w > width {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push_str(g);
            cur_w += g_w;
        }
        out.push(cur);
    }
    out
}

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

/// Render the bottom live region into `buf`. While a reply streams, the strip's
/// top row previews the in-progress line and the row below it is a blank gap, so
/// the reply never touches the rule-framed, **growing** input box; idle, the
/// strip collapses and the box sits at the top of the region. The input wraps
/// across as many rows as `area` allows; the prompt marks its first line and
/// continuation lines are indented to align under it. When the input is taller
/// than the box, the tail is kept in view (the cursor is always at the end).
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App) {
    let streaming = app.is_streaming();
    let menu = menu_rows(app);
    let shortcuts = shortcuts_rows(app);
    let file = file_menu_rows(app);
    // The band below the box holds the palette, the shortcuts overview, *or* the
    // `@` file picker (mutually exclusive: the palette needs a `/token`, the
    // shortcuts an empty composer, the picker an `@token`). Queued messages
    // render in the strip *above* the box instead; the session-context footer
    // takes the very last row unless a band displaces it.
    let band = menu + shortcuts + file;
    let queued = queued_rows(app, area.width);
    let footer = footer_rows(app, band);
    // The preview row + its gap are only reserved when there is something to
    // preview; the pre-stream pause shows status-only (no stray blank line).
    let has_preview = strip_has_preview(app);
    let [strip, _, band_area, footer_area] =
        live_layout(area, streaming, has_preview, queued, band, footer);
    // Rows the preview occupies at the strip's top (0 when there's no preview).
    let preview_rows = if has_preview {
        PREVIEW_ROWS + GAP_ROWS
    } else {
        0
    };

    // Strip preview (top row; the row below it is the blank gap). A running
    // tool takes precedence — its coloured header (blue) shows what's executing;
    // otherwise the in-progress reply's last line previews. Nothing when idle —
    // or before the first chunk (an empty buffer has nothing to preview, so the
    // pre-stream pause shows only the status line, no stray `●` bullet and no
    // reserved row for it).
    let preview = if let Some(tool) = app.current_tool() {
        tool_lines(tool, strip.width).into_iter().next()
    } else {
        app.streaming_text().filter(|t| !t.is_empty()).map(|text| {
            message_lines(Role::Assistant, text, strip.width)
                .pop()
                .unwrap_or_default()
        })
    };
    if let Some(preview) = preview {
        let preview_area = Rect {
            height: PREVIEW_ROWS.min(strip.height),
            ..strip
        };
        Paragraph::new(preview).render(preview_area, buf);
    }

    // The live status line, pinned below the preview (or at the strip top during
    // the pause), just above the box, while a turn is in flight.
    if let Some(status) = app.status() {
        let status_y = strip.y + preview_rows;
        if status_y < strip.y + strip.height {
            let status_area = Rect {
                x: strip.x,
                y: status_y,
                width: strip.width,
                height: STATUS_ROWS,
            };
            Paragraph::new(status_line(status)).render(status_area, buf);
        }
    }

    // The queued messages, styled like sent user messages (❯ bullet, dark
    // background, wrapped), stacked below the status's gap and just above the
    // box's top rule — only while a turn streams (the only time the queue is
    // non-empty). codex's pending-input preview, in our user-message style.
    if queued > 0 {
        let q_y = strip.y + preview_rows + STATUS_ROWS + STATUS_GAP_ROWS;
        let strip_bottom = strip.y + strip.height;
        if q_y < strip_bottom {
            let q_area = Rect {
                x: strip.x,
                y: q_y,
                width: strip.width,
                height: queued.min(strip_bottom - q_y),
            };
            Paragraph::new(queued_lines(app, q_area.width)).render(q_area, buf);
        }
    }

    // The input box: a top/bottom rule framing the wrapped input rows.
    let bx = input_box(
        area,
        &app.input,
        streaming,
        has_preview,
        queued,
        band,
        footer,
    );
    let block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_COLOR));
    block.render(bx.frame, buf);

    // While a Ctrl+R search previews a match, the query's occurrences in it
    // light up reversed+bold (codex's textarea highlight); otherwise the rows
    // render as single plain spans.
    let highlights = app.search_highlight_ranges();
    let lines: Vec<Line> = bx
        .rows
        .iter()
        .zip(&bx.row_ranges)
        .enumerate()
        .skip(bx.scroll)
        .take(bx.text.height as usize)
        .map(|(i, (line, range))| {
            // The prompt prefixes the real first line; wrapped/continuation lines
            // get a matching-width indent so the text stays aligned under it. In
            // shell mode the absorbed `!` renders back as a red prompt (`! pwd`
            // instead of `❯ pwd` — docs/shell-command.md).
            let (prefix, style) = if i != 0 {
                (INDENT, Style::default())
            } else if app.shell_mode {
                (SHELL_BULLET, Style::new().fg(SHELL_MODE_COLOR))
            } else {
                (PROMPT, Style::new().fg(PROMPT_COLOR))
            };
            let mut spans = vec![Span::styled(prefix, style)];
            spans.extend(highlight_row_spans(line, range, &highlights));
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).render(bx.text, buf);

    // The palette, the shortcuts overview, or the file picker, pinned in the
    // band below the box (at most one is open).
    if menu > 0 {
        Paragraph::new(command_menu_lines(app, band_area.width)).render(band_area, buf);
    } else if shortcuts > 0 {
        Paragraph::new(shortcuts_lines(app.turn_active())).render(band_area, buf);
    } else if file > 0 {
        Paragraph::new(file_menu_lines(app, band_area.width)).render(band_area, buf);
    }

    // The session-context footer on the region's last row — only when no band
    // is open (the band takes its place; see docs/footer.md). An open Ctrl+R
    // search (docs/history-search.md) or a `!command` shell mode
    // (docs/shell-command.md) takes the same slot with its own line.
    if footer > 0 {
        let line = if let Some(search) = app.history_search.as_ref() {
            search_line(search)
        } else if app.shell_mode {
            shell_mode_line()
        } else {
            footer_line(app, footer_area.width)
        };
        Paragraph::new(line).render(footer_area, buf);
    }
}

/// How many rows the command palette occupies for `app`: 0 when closed, otherwise
/// the match count capped at [`MENU_MAX_ROWS`] (or a single placeholder row when
/// the query matches nothing). [`live_height`] adds this; [`render_live`] paints
/// exactly this many rows — the two must agree.
#[must_use]
pub fn menu_rows(app: &App) -> u16 {
    if app.command_menu.is_none() {
        return 0;
    }
    match command_query(app.input.text()) {
        None => 0,
        Some(query) => {
            let matches = matching_commands(query).len();
            if matches == 0 {
                1
            } else {
                (matches as u16).min(MENU_MAX_ROWS)
            }
        }
    }
}

/// The scroll offset (first visible match index) so a window of `max` rows keeps
/// `selected` visible: 0 while the selection fits the first window, then it
/// follows the selection toward the end, clamped so the last window sits flush
/// with the end.
#[must_use]
pub fn menu_window(len: usize, selected: usize, max: usize) -> usize {
    if max == 0 || len <= max || selected < max {
        0
    } else {
        (selected + 1 - max).min(len - max)
    }
}

/// One palette row: `/name` padded out to [`MENU_DESC_COL`] columns, then its
/// description. The selection is shown by **colour** — the selected row lights up
/// whole in cyan (name *and* description the same colour, name bold), the others
/// are dimmed grey. No caret, no background bar.
fn menu_row(cmd: &SlashCommand, selected: bool, width: u16) -> Line<'static> {
    let name = format!("/{}", cmd.name);
    let cw = width as usize;
    // Pad the name out to the description column so descriptions line up; truncate
    // the description to whatever room is left.
    let pad = " ".repeat(MENU_DESC_COL.saturating_sub(cols(&name)).max(1));
    let desc = truncate_cols(cmd.description, cw.saturating_sub(MENU_DESC_COL));
    // Name and description share one colour per row, for consistency.
    let color = if selected {
        MENU_SELECTED_COLOR
    } else {
        MENU_DIM_COLOR
    };
    let name_style = if selected {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    Line::from(vec![
        Span::styled(name, name_style),
        Span::raw(pad),
        Span::styled(desc, Style::new().fg(color)),
    ])
}

/// The styled lines for the open command palette: the filtered commands, windowed
/// to keep the selection visible and capped at [`MENU_MAX_ROWS`], with the
/// highlighted row marked; or a single dim placeholder when nothing matches.
/// Empty when the palette is closed.
#[must_use]
pub fn command_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.command_menu else {
        return Vec::new();
    };
    let Some(query) = command_query(app.input.text()) else {
        return Vec::new();
    };
    let matches = matching_commands(query);
    if matches.is_empty() {
        return vec![Line::from(Span::styled(
            MENU_NO_MATCH.to_string(),
            Style::new().fg(MENU_DIM_COLOR),
        ))];
    }
    let max = MENU_MAX_ROWS as usize;
    let offset = menu_window(matches.len(), menu.selected, max);
    matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, cmd)| menu_row(cmd, i == menu.selected, width))
        .collect()
}

/// How many rows the `@` file picker occupies for `app`: 0 when closed, one
/// placeholder row while searching / when nothing matched, else the match count
/// capped at [`FILE_MENU_MAX_ROWS`] (longer lists scroll, like the palette).
/// [`live_height`] adds this to its band and [`render_live`] paints exactly this
/// many rows — the two must agree. See `docs/file-search.md`.
#[must_use]
pub fn file_menu_rows(app: &App) -> u16 {
    match &app.file_search {
        None => 0,
        Some(fs) if fs.matches.is_empty() => 1,
        Some(fs) => (fs.matches.len() as u16).min(FILE_MENU_MAX_ROWS),
    }
}

/// One file-picker row: the path with the query's matched characters bolded
/// (the byte offsets in [`FileMatch::indices`]); the selected row lights up cyan
/// like the palette, the rest dim. Truncated to `width` (the surviving prefix
/// keeps the same byte offsets, so the highlight stays aligned).
fn file_menu_row(m: &FileMatch, selected: bool, width: u16) -> Line<'static> {
    let color = if selected {
        MENU_SELECTED_COLOR
    } else {
        MENU_DIM_COLOR
    };
    let base = Style::new().fg(color);
    let matched = base.add_modifier(Modifier::BOLD);
    let shown = truncate_cols(&m.path, width as usize);
    // Group consecutive matched / unmatched characters into spans.
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (off, ch) in shown.char_indices() {
        let is_match = m.indices.binary_search(&off).is_ok();
        if !run.is_empty() && is_match != run_matched {
            let style = if run_matched { matched } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_matched = is_match;
        run.push(ch);
    }
    if !run.is_empty() {
        let style = if run_matched { matched } else { base };
        spans.push(Span::styled(run, style));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    Line::from(spans)
}

/// The styled lines for the open file picker: a *Searching…* / *No matching
/// files* placeholder while the band has no matches, else the file rows windowed
/// (`menu_window`) to keep the selection visible and capped at
/// [`FILE_MENU_MAX_ROWS`]. Empty when the picker is closed.
#[must_use]
pub fn file_menu_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(fs) = &app.file_search else {
        return Vec::new();
    };
    if fs.matches.is_empty() {
        let text = if fs.waiting {
            FILE_MENU_SEARCHING
        } else {
            FILE_MENU_NO_MATCH
        };
        return vec![Line::from(Span::styled(
            text.to_string(),
            Style::new().fg(MENU_DIM_COLOR),
        ))];
    }
    let max = FILE_MENU_MAX_ROWS as usize;
    let offset = menu_window(fs.matches.len(), fs.selected, max);
    fs.matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, m)| file_menu_row(m, i == fs.selected, width))
        .collect()
}

/// How many rows the `?` shortcuts band occupies for `app`: 0 when closed,
/// otherwise the entry list two-per-row. [`live_height`] adds this (via its
/// band parameter); [`render_live`] paints exactly this many rows — the two
/// must agree, like [`menu_rows`].
#[must_use]
pub fn shortcuts_rows(app: &App) -> u16 {
    if app.shortcuts_open {
        SHORTCUTS.len().div_ceil(2) as u16
    } else {
        0
    }
}

/// The styled lines for the open shortcuts band: the [`SHORTCUTS`] entries two
/// per row — the second column starting at [`SHORTCUTS_COL`] — with keys cyan
/// and labels dim. While a turn is in flight the `esc` entry reads
/// ` to interrupt` (it would quit only when idle), codex's context-sensitive
/// quit entry.
#[must_use]
pub fn shortcuts_lines(turn_active: bool) -> Vec<Line<'static>> {
    let entry = |key: &'static str, label: &'static str| {
        let label = if key == "esc" && turn_active {
            " to interrupt"
        } else {
            label
        };
        [
            Span::styled(key, Style::new().fg(SHORTCUTS_KEY_COLOR)),
            Span::styled(label, Style::new().fg(SHORTCUTS_TEXT_COLOR)),
        ]
    };
    SHORTCUTS
        .chunks(2)
        .map(|pair| {
            let [key, label] = entry(pair[0].0, pair[0].1);
            let mut spans = vec![key, label];
            if let Some(&(key2, label2)) = pair.get(1) {
                let used = cols(pair[0].0) + cols(spans[1].content.as_ref());
                spans.push(Span::raw(
                    " ".repeat(SHORTCUTS_COL.saturating_sub(used).max(1)),
                ));
                spans.extend(entry(key2, label2));
            }
            Line::from(spans)
        })
        .collect()
}

/// How many rows the queued messages occupy in the strip at `width`: the total
/// wrapped height of every queued message (each styled like a user message),
/// uncapped — the whole backlog shows, codex-style; 0 when the queue is empty.
/// [`live_height`] reserves this and [`render_live`] paints exactly this many —
/// the two must agree (both go through [`queued_lines`], so they can't drift).
/// `live_height`'s terminal-height clamp still bounds the region as a whole.
#[must_use]
pub fn queued_rows(app: &App, width: u16) -> u16 {
    queued_lines(app, width).len() as u16
}

/// The styled lines for the queued follow-up messages: each rendered like a sent
/// user message ([`message_lines`] — the `❯ ` bullet, dark background, wrapped to
/// `width` minus the [`QUEUED_INDENT`] every row is inset by), concatenated —
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
    u16::from(app.session.is_some() && band_rows == 0)
}

/// The footer's single line: the [`FOOTER_INDENT`], then `{model} · {cwd}` —
/// every segment dim (codex's no-theme-colours status line, the separator dim
/// like its ` · `) — cut with a trailing `…` when it overflows `width`
/// (codex's `truncate_line_with_ellipsis_if_overflow`). Empty when no session
/// info has been injected.
#[must_use]
pub fn footer_line(app: &App, width: u16) -> Line<'static> {
    let Some(session) = &app.session else {
        return Line::default();
    };
    let dim = Style::new().fg(FOOTER_COLOR);
    let segments = [
        Span::styled(session.model.clone(), dim),
        Span::styled(FOOTER_SEPARATOR.to_string(), dim),
        Span::styled(session.cwd.clone(), dim),
    ];
    let mut budget = (width as usize).saturating_sub(cols(FOOTER_INDENT));
    let mut spans = vec![Span::raw(FOOTER_INDENT)];
    if segments.iter().map(|s| cols(&s.content)).sum::<usize>() <= budget {
        spans.extend(segments);
        return Line::from(spans);
    }
    // Overflow: keep whole leading segments while they fit, cut the first that
    // doesn't, and close with the ellipsis.
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
    Line::from(spans)
}

/// The `!` shell-mode footer line: the [`FOOTER_INDENT`] then `Shell mode` in
/// red ([`SHELL_MODE_COLOR`]) — codex's `shell_mode_footer_line`. Shown in the
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
/// the [`FOOTER_INDENT`], the query cyan, then per state the accept/cancel
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

/// The bullet colour for a tool's lifecycle: blue running, green ok, red fail.
const fn tool_status_color(status: ToolStatus) -> Color {
    match status {
        ToolStatus::Running => TOOL_RUNNING_COLOR,
        ToolStatus::Ok => TOOL_OK_COLOR,
        ToolStatus::Failed => TOOL_FAIL_COLOR,
    }
}

/// Truncate `s` to at most `max` display columns (column-aware, so wide glyphs
/// count as two), returning the kept prefix.
fn truncate_cols(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = char_cols(ch);
        if w + cw > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// The coloured bullet header line for a tool: `● name(args)`, the bullet
/// recoloured by lifecycle (blue/green/red). Shared by the inline collapsed view
/// ([`tool_lines`]) and the full-screen transcript ([`tool_full_lines`]).
fn tool_header(tool: &ToolCall) -> Line<'static> {
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status))
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(TOOL_BULLET.to_string(), bullet_style),
        Span::styled(
            tool.name.clone(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    // A `!` shell command is a tool with no args (name = the command), so it
    // shows as a bare `● {command}` — only append `(args)` when there are any.
    if !tool.args.is_empty() {
        spans.push(Span::styled(
            format!("({})", tool.args),
            Style::new().fg(TOOL_DIM_COLOR),
        ));
    }
    Line::from(spans)
}

/// One row of a `⎿` result block, dim: the **first** row (index 0) opens with
/// the [`TOOL_RESULT_PREFIX`] corner, continuation rows indent by its display
/// width so the text aligns under it (Claude-Code's exec-cell output style).
fn result_row(index: usize, text: String) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let prefix = if index == 0 {
        TOOL_RESULT_PREFIX.to_string()
    } else {
        " ".repeat(cols(TOOL_RESULT_PREFIX))
    };
    Line::from(vec![Span::styled(prefix, dim), Span::styled(text, dim)])
}

/// The output of `tool` split into display lines (a single trailing blank from a
/// final newline dropped, so a hidden-line count is accurate).
fn tool_output_lines(tool: &ToolCall) -> Vec<&str> {
    if tool.output.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<&str> = tool.output.split('\n').collect();
    if out.last() == Some(&"") {
        out.pop();
    }
    out
}

/// Build the styled lines for one tool call as shown **inline**.
///
/// A `!` shell command is **headerless** — its `Role::Shell` header (`! pwd`)
/// sits flush above (docs/shell-command.md) — and shows up to
/// [`TOOL_PEEK_LINES`] of its output as a `⎿` block (each line aligned under
/// the corner), then a `… +N lines (ctrl+o to expand)` hint when more is
/// hidden (Claude-Code's exec cell). A backend tool keeps its coloured
/// `● name(args)` header and a single collapsed peek line. The full output is
/// only rendered in the separate tool-output view, never here.
#[must_use]
pub fn tool_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let out_lines = tool_output_lines(tool);

    if tool.shell {
        // The running/empty single-row states; else up to TOOL_PEEK_LINES rows.
        // (Truncation of an over-cap output is marked only in the expanded view;
        // inline, the `… +N lines (ctrl+o to expand)` hint already signals more.)
        return match tool.status {
            ToolStatus::Running => vec![result_row(0, tool_running_marker(tool.shell).to_string())],
            _ if out_lines.is_empty() => vec![result_row(0, TOOL_NO_OUTPUT.to_string())],
            _ => {
                let shown = out_lines.len().min(TOOL_PEEK_LINES);
                let mut lines: Vec<Line> = out_lines[..shown]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| result_row(i, truncate_cols(line, peek_width)))
                    .collect();
                let hidden = out_lines.len() - shown;
                if hidden > 0 {
                    lines.push(Line::from(vec![
                        Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
                        Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
                    ]));
                }
                lines
            }
        };
    }

    // A backend tool: coloured header + a single collapsed peek line.
    let peek = match tool.status {
        ToolStatus::Running => tool_running_marker(tool.shell).to_string(),
        _ if out_lines.is_empty() => TOOL_NO_OUTPUT.to_string(),
        _ => truncate_cols(out_lines[0], peek_width),
    };
    let mut lines = vec![
        tool_header(tool),
        Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(peek, dim),
        ]),
    ];
    let hidden = out_lines.len().saturating_sub(1);
    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
            Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
        ]));
    }
    lines
}

/// One tool call's full lines for the transcript view: its **complete** output
/// (wrapped **verbatim** — [`wrap_verbatim`], so `ls -l`/`tree` alignment and
/// indentation survive), or `running…` / `(no output)` when there is none yet.
/// The expanded counterpart of [`tool_lines`]. A `!` shell command stays
/// **headerless** here too (the `! pwd` dark header is the `Role::Shell`
/// message above it) and its output renders as the same `⎿` block, uncapped; a
/// backend tool keeps its coloured `● name(args)` header with the output
/// indented under it. An over-cap shell output ([`ToolCall::truncated`])
/// appends a dim [`TOOL_TRUNCATED_MARKER`] line to show the rest was dropped.
fn tool_full_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    let running_word = tool_running_marker(tool.shell);
    if tool.shell {
        let body_width = width.saturating_sub(cols(TOOL_RESULT_PREFIX) as u16).max(1);
        let mut body = match (tool.status, tool.output.is_empty()) {
            (ToolStatus::Running, true) => vec![running_word.to_string()],
            (_, true) => vec![TOOL_NO_OUTPUT.to_string()],
            _ => wrap_verbatim(&tool.output, body_width),
        };
        // The output was cut at the in-memory cap — mark the end so the user
        // knows more was dropped (it is not recoverable; nothing to expand to).
        if tool.truncated {
            body.push(TOOL_TRUNCATED_MARKER.to_string());
        }
        return body
            .into_iter()
            .enumerate()
            .map(|(i, line)| result_row(i, line))
            .collect();
    }

    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut lines = vec![tool_header(tool)];
    let body = match (tool.status, tool.output.is_empty()) {
        (ToolStatus::Running, true) => vec![running_word.to_string()],
        (_, true) => vec![TOOL_NO_OUTPUT.to_string()],
        _ => wrap_verbatim(&tool.output, content_width),
    };
    for out in body {
        lines.push(Line::from(vec![
            Span::raw(INDENT.to_string()),
            Span::styled(out, dim),
        ]));
    }
    lines
}

/// Linearly blend `fg` toward `bg` by `1 - alpha` (codex's `blend`): `alpha` 1
/// is pure `fg`, 0 pure `bg`.
fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| (f32::from(f) * alpha + f32::from(b) * (1.0 - alpha)) as u8;
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// One bold span per char of `text`, shimmered codex-style: a raised-cosine
/// brightness band (half-width [`SHIMMER_BAND_HALF_WIDTH`], plus
/// [`SHIMMER_PADDING`] chars of off-text run-in/out) sweeps the text once per
/// [`SHIMMER_SWEEP`], each char blending from the white-grey [`SHIMMER_BASE`]
/// toward the bright [`SHIMMER_HIGHLIGHT`] by its distance from the band's
/// crest. A faithful port of openai/codex `tui/src/shimmer.rs::shimmer_spans`,
/// made pure: the phase comes from the boundary-supplied `elapsed` (sub-second
/// resolution), not a process-wide clock — so it's deterministic in tests.
fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let period = chars.len() + SHIMMER_PADDING * 2;
    let sweep = SHIMMER_SWEEP.as_secs_f32();
    let pos = ((elapsed.as_secs_f32() % sweep) / sweep * period as f32) as usize;

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let dist = (i as isize + SHIMMER_PADDING as isize - pos as isize).abs() as f32;
            let t = if dist <= SHIMMER_BAND_HALF_WIDTH {
                let x = std::f32::consts::PI * (dist / SHIMMER_BAND_HALF_WIDTH);
                0.5 * (1.0 + x.cos())
            } else {
                0.0
            };
            let (r, g, b) = blend(SHIMMER_HIGHLIGHT, SHIMMER_BASE, t * SHIMMER_MAX_BLEND);
            Span::styled(
                ch.to_string(),
                Style::new()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// The comet spinner opening the status line: the [`SPINNER_FRAMES`] frame
/// for `elapsed` (one frame per [`SPINNER_INTERVAL`], looping), split into
/// exactly [`SPINNER_SPAN_COUNT`] spans — one per cell, so each carries its
/// own fade step: the white bold [`SPINNER_HEAD`], the mid-grey
/// [`SPINNER_TAIL_MID`] behind it, and everything else (the faint `·` tail
/// end, the walls, the empty track) dim; the right wall carries the trailing
/// separator space. Pure, like [`shimmer_spans`]: the frame index derives
/// from the boundary-supplied `elapsed`, and the loop's animation re-arm
/// keeps it advancing.
fn spinner_spans(elapsed: Duration) -> Vec<Span<'static>> {
    let frame_index =
        (elapsed.as_millis() / SPINNER_INTERVAL.as_millis()) as usize % SPINNER_FRAMES.len();
    let frame = SPINNER_FRAMES[frame_index];
    let dim = Style::new().fg(STATUS_DETAIL_COLOR);
    let spans: Vec<Span<'static>> = frame
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let style = match c {
                SPINNER_HEAD => Style::new().fg(STATUS_COLOR).add_modifier(Modifier::BOLD),
                SPINNER_TAIL_MID => Style::new().fg(SPINNER_TAIL_COLOR),
                _ => dim,
            };
            let text = if i == SPINNER_SPAN_COUNT - 1 {
                format!("{c} ")
            } else {
                c.to_string()
            };
            Span::styled(text, style)
        })
        .collect();
    debug_assert_eq!(spans.len(), SPINNER_SPAN_COUNT);
    spans
}

/// The live status line shown in the strip above the box while a turn is in
/// flight:
/// `(●•·   ) {verb}… ({elapsed}s[ · {arrow} {n} tokens][ · Thinking for {m}s] · esc to interrupt)`.
///
/// It opens with the comet spinner ([`spinner_spans`]) and the verb
/// text **shimmers** — a bright-white band sweeping its white-grey chars
/// ([`shimmer_spans`]) — both animations phase-driven by the boundary-supplied
/// `elapsed`; the parenthesised metrics are dim. The token clause is omitted
/// while the tally is 0 (the "just submitted" state), and the thinking clause
/// only while `thinking` is `Some`. Pure — it formats the (already
/// boundary-stamped) [`TurnStatus`], so it is unit-tested with explicit values.
#[must_use]
pub fn status_line(status: &TurnStatus) -> Line<'static> {
    let mut detail = format!("{}s", status.elapsed.as_secs());
    if status.tokens > 0 {
        let arrow = match status.arrow {
            TokenArrow::Down => STATUS_ARROW_DOWN,
            TokenArrow::Up => STATUS_ARROW_UP,
        };
        detail.push_str(&format!(" · {arrow} {} tokens", status.tokens));
    }
    if let Some(thinking) = status.thinking {
        detail.push_str(&format!(" · Thinking for {}s", thinking.as_secs()));
    }
    detail.push_str(&format!(" · {STATUS_INTERRUPT_HINT}"));
    let mut spans = spinner_spans(status.elapsed);
    spans.extend(shimmer_spans(
        &format!("{}{STATUS_ELLIPSIS}", status.verb),
        status.elapsed,
    ));
    spans.push(Span::styled(
        format!(" ({detail})"),
        Style::new().fg(STATUS_DETAIL_COLOR),
    ));
    Line::from(spans)
}

/// The committed turn summary: a single dim, bullet-less `"{verb} for {secs}s"`
/// line. Shown inline (it flows into scrollback) and in the transcript like any
/// other [`HistoryItem`]; `width` is unused (the line never wraps) but kept for a
/// uniform `*_lines` signature.
#[must_use]
pub fn summary_lines(summary: &TurnSummary, _width: u16) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(
        format!("{} for {}s", summary.verb, summary.secs),
        Style::new().fg(STATUS_DONE_COLOR),
    ))]
}

/// The stamp footer under a **user** message in the transcript: a blank row,
/// then the dim timestamp right-aligned flush to `width`. Empty for an empty
/// timestamp (no clock injected). Only user messages get this — the other item
/// kinds record a stamp too but never display it. Called **only** from the
/// transcript builder — the inline view stays stamp-free.
fn user_stamp_lines(timestamp: &str, width: u16) -> Vec<Line<'static>> {
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

/// Build the full conversation transcript shown in the tool-output view: every
/// user/assistant/error message **and** every tool call's complete output,
/// interleaved in the exact order they happened (straight from `App::history`),
/// followed by the live tail — the in-progress reply and/or the running tool.
/// A blank line separates items. Tools are shown *expanded* here (the inline
/// view collapses them). Empty → a single placeholder line.
///
/// Only the **user** message shows its wall-clock `timestamp`: dim,
/// right-aligned on its own line below the message ([`user_stamp_lines`]) — the
/// **only** stamp displayed anywhere (AI replies, tools, and turn summaries
/// record one but never show it; the inline view never shows any; see
/// `docs/timestamps.md`).
#[must_use]
pub fn transcript_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in &app.history {
        match item {
            HistoryItem::Message(m) => {
                lines.extend(message_lines(m.role, &m.text, width));
                if m.role == Role::User {
                    lines.extend(user_stamp_lines(&m.timestamp, width));
                }
            }
            HistoryItem::Tool(t) => lines.extend(tool_full_lines(t, width)),
            HistoryItem::Summary(s) => lines.extend(summary_lines(s, width)),
        }
        // Blank spacer after every item — except a shell command's header
        // ([`is_shell_header`]): its tool's `⎿` output sits flush below it,
        // whether the tool is already in history or still the running live
        // tail, so the overlay renders the same exec cell as the inline view.
        if !is_shell_header(item) {
            lines.push(Line::default());
        }
    }
    // Live tail: the in-progress assistant text, then the running tool (only one
    // is ever active given how a turn streams, but both are handled in order).
    if let Some(text) = app.streaming_text()
        && !text.is_empty()
    {
        lines.extend(message_lines(Role::Assistant, text, width));
        lines.push(Line::default());
    }
    if let Some(tool) = app.current_tool() {
        lines.extend(tool_full_lines(tool, width));
        lines.push(Line::default());
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            TOOL_VIEW_EMPTY.to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        )));
    }
    lines
}

/// The largest the transcript scroll offset can be on a `screen_height`-row
/// screen — the total content height minus the scrolling body — so the last
/// line can reach the bottom but not scroll past it. The loop clamps
/// `App::tool_scroll` to this each draw.
#[must_use]
pub fn tool_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS) as usize;
    transcript_lines(app, width).len().saturating_sub(body)
}

/// Render the full-screen tool-output view: a title row, then the scrolling
/// conversation transcript (messages + every tool call's full output), windowed
/// by `App::tool_scroll` (clamped so it can't run past the end). Pure — `term.rs`
/// paints this onto the overlay.
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App) {
    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(TOOL_VIEW_TITLE_ROWS), Constraint::Min(0)])
            .areas(area);

    let title = Line::from(vec![
        Span::styled(
            TOOL_VIEW_TITLE.to_string(),
            Style::new().fg(AI_COLOR).add_modifier(Modifier::BOLD),
        ),
        Span::styled(TOOL_VIEW_HINT.to_string(), Style::new().fg(TOOL_DIM_COLOR)),
    ]);
    Paragraph::new(title).render(title_area, buf);

    let lines = transcript_lines(app, body_area.width);
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    Paragraph::new(visible).render(body_area, buf);
}

/// Decide which assistant lines are now safe to flush to scrollback as a reply
/// streams in.
///
/// Greedy word-wrap is *prefix-stable* — only the final wrapped line can still
/// change as more text arrives — so we commit everything up to it. Given how
/// many lines were `committed` already, returns the new lines to commit and the
/// updated committed count. `committed` is clamped so a mid-stream resize (which
/// re-wraps to a different line count) can't panic.
#[must_use]
pub fn stable_commit(text: &str, width: u16, committed: usize) -> (Vec<Line<'static>>, usize) {
    let lines = message_lines(Role::Assistant, text, width);
    let stable = lines.len().saturating_sub(1);
    let start = committed.min(stable);
    (lines[start..stable].to_vec(), stable.max(committed))
}

/// The remaining (last) lines to flush once the reply is complete.
#[must_use]
pub fn final_commit(text: &str, width: u16, committed: usize) -> Vec<Line<'static>> {
    let lines = message_lines(Role::Assistant, text, width);
    let start = committed.min(lines.len());
    lines[start..].to_vec()
}

/// Whether `item` is a `!` shell command's header message (`Role::Shell`).
/// Such an item gets **no** blank spacer after it: its tool's `⎿` output (or
/// the live `⎿ Running…` preview) sits flush below, forming one exec cell
/// (docs/shell-command.md). Shared by [`conversation_lines`] and
/// [`transcript_lines`] so the inline view and the Ctrl+O overlay agree.
fn is_shell_header(item: &HistoryItem) -> bool {
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
    let mut lines = conversation_lines(history, width);
    if lines.len() > max_rows {
        lines = lines.split_off(lines.len() - max_rows);
    }
    lines
}

/// How many history rows fit above a `live_height`-row live region on a
/// `term_height`-row screen — the number of lines to repaint after a resize.
/// Saturates at 0 so a live region taller than the screen can never underflow.
/// The pure counterpart of the terminal calls in `main.rs::repaint_after_resize`.
#[must_use]
pub fn repaint_budget(term_height: u16, live_height: u16) -> usize {
    term_height.saturating_sub(live_height) as usize
}

/// Absolute `(x, y)` where the terminal's hardware cursor should sit for the
/// current input. Shares [`input_box`] with [`render_live`] so the cursor lands
/// exactly where the editor's cursor is — on its wrapped row, at its column —
/// wherever the user has moved it, not just at the end.
#[must_use]
pub fn cursor_position(area: Rect, app: &App) -> (u16, u16) {
    // Laid out exactly as render_live lays the box out — the streaming strip
    // and queued rows above, the band and footer below — so the cursor sits on
    // the prompt row even mid-turn (codex keeps the composer focused while a
    // task runs: typing edits the draft, Enter queues it).
    let band = menu_rows(app) + shortcuts_rows(app) + file_menu_rows(app);
    let footer = footer_rows(app, band);
    let has_preview = strip_has_preview(app);
    // While a Ctrl+R search is open the hardware cursor tracks the end of the
    // *footer query*, not the textarea preview — the shell reverse-i-search
    // feel (codex's history_search_cursor_pos), clamped inside the row.
    if let Some(search) = &app.history_search {
        let [_, _, _, footer_area] = live_layout(
            area,
            app.is_streaming(),
            has_preview,
            queued_rows(app, area.width),
            band,
            footer,
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
        app.is_streaming(),
        has_preview,
        queued_rows(app, area.width),
        band,
        footer,
    );
    let row = bx.cursor_row.saturating_sub(bx.scroll) as u16;
    let col = bx.cursor_col as u16;
    (bx.text.x + BULLET_WIDTH + col, bx.text.y + row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{FileSearch, Message};

    /// Concatenate a line's span contents into its plain text.
    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Read row `y` of a buffer back as a string.
    fn row(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// A queued text batch with no attachments — the queue tests' usual shape.
    fn batch(texts: &[&str]) -> QueuedTurn {
        QueuedTurn::Messages {
            texts: texts.iter().map(|s| (*s).to_string()).collect(),
            images: Vec::new(),
        }
    }

    // --- wrap_text ---

    #[test]
    fn wrap_text_breaks_on_word_boundaries() {
        assert_eq!(wrap_text("hello world", 5), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_text_keeps_text_that_fits_on_one_line() {
        assert_eq!(wrap_text("hello world", 11), vec!["hello world"]);
    }

    #[test]
    fn wrap_text_hard_breaks_words_longer_than_width() {
        assert_eq!(wrap_text("aaaaaa", 3), vec!["aaa", "aaa"]);
        assert_eq!(wrap_text("abcdefg", 3), vec!["abc", "def", "g"]);
    }

    #[test]
    fn wrap_text_preserves_blank_lines() {
        assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_text_of_empty_string_is_a_single_blank_line() {
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn wrap_text_with_zero_width_does_not_wrap() {
        assert_eq!(wrap_text("a b", 0), vec!["a b"]);
    }

    #[test]
    fn wrap_text_hard_breaks_a_word_that_is_an_exact_multiple_of_width() {
        // A word whose length is an exact multiple of the width must not leave a
        // phantom trailing blank line or drop the final full chunk.
        assert_eq!(wrap_text("aaaaaaaaa", 3), vec!["aaa", "aaa", "aaa"]);
    }

    #[test]
    fn wrap_text_packs_a_following_word_onto_a_hard_break_remainder() {
        // "abcdefg" (7) hard-breaks at width 5 into "abcde" + the remainder "fg".
        // The next token "h" was whitespace-separated in the input, so the space
        // in "fg h" is real — this is ordinary greedy packing (like `fold`), not
        // an invented word boundary. Locks the behaviour against regression.
        assert_eq!(wrap_text("abcdefg h", 5), vec!["abcde", "fg h"]);
    }

    #[test]
    fn wrap_text_exact_multiple_remainder_does_not_absorb_the_next_word() {
        // "aaaaaa" is an exact multiple of 3 → two full lines, nothing buffered,
        // so the following word "bb" correctly starts its own line.
        assert_eq!(wrap_text("aaaaaa bb", 3), vec!["aaa", "aaa", "bb"]);
    }

    #[test]
    fn wrap_text_measures_wide_chars_as_two_columns() {
        // CJK glyphs occupy 2 terminal columns each, so only two fit in width 4
        // — not four, as a naive char count would allow.
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_text_treats_a_zero_width_combining_mark_as_zero_columns() {
        // "e" + combining acute is one column wide, so it fits a width-1 line
        // instead of being hard-broken onto two lines like a 2-char count implies.
        assert_eq!(wrap_text("e\u{0301}", 1), vec!["e\u{0301}"]);
    }

    #[test]
    fn wrap_text_hard_break_keeps_zwj_emoji_clusters_whole() {
        // A family emoji is one grapheme cluster (four emoji joined by U+200D
        // zero-width joiners). The hard-break must split the over-long "word"
        // only on grapheme boundaries — never inside a cluster, which would
        // leave a bare ZWJ at a line end and render broken glyphs.
        const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let text = FAMILY.repeat(50);
        let lines = wrap_text(&text, 78);
        assert!(lines.len() > 1, "the run is hard-broken across lines");
        for line in &lines {
            assert!(
                !line.ends_with('\u{200D}'),
                "no line ends mid-cluster on a joiner: {line:?}"
            );
            assert!(
                line.len() % FAMILY.len() == 0
                    && line.matches(FAMILY).count() * FAMILY.len() == line.len(),
                "every line is whole clusters only: {line:?}"
            );
        }
        assert_eq!(lines.concat(), text, "no text lost by the break");
    }

    // --- wrap_verbatim (the whitespace-preserving wrap for tool output) ---

    #[test]
    fn wrap_verbatim_preserves_leading_indentation() {
        assert_eq!(
            wrap_verbatim("    fn main() {", 40),
            vec!["    fn main() {"]
        );
    }

    #[test]
    fn wrap_verbatim_preserves_internal_space_runs() {
        // `ls -l` / `tree` output is column-aligned by space runs; every byte
        // of a line that fits must survive verbatim.
        assert_eq!(
            wrap_verbatim("-rw-r--r--  1 user   42 a.txt\n│   ├── b", 60),
            vec!["-rw-r--r--  1 user   42 a.txt", "│   ├── b"]
        );
    }

    #[test]
    fn wrap_verbatim_preserves_empty_lines() {
        assert_eq!(wrap_verbatim("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_verbatim_hard_breaks_wide_chars_on_column_boundaries() {
        // CJK glyphs are two columns each, so only two fit in width 4 — the
        // break is display-width-aware like wrap_text's.
        assert_eq!(wrap_verbatim("你好世界", 4), vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_verbatim_hard_breaks_on_grapheme_boundaries() {
        // A ZWJ family-emoji cluster is never severed mid-joiner.
        const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let text = FAMILY.repeat(10);
        let lines = wrap_verbatim(&text, 8);
        assert!(lines.len() > 1, "hard-broken across lines");
        for line in &lines {
            assert!(
                !line.ends_with('\u{200D}'),
                "no line ends mid-cluster: {line:?}"
            );
        }
        assert_eq!(lines.concat(), text, "no text lost by the break");
    }

    #[test]
    fn wrap_verbatim_keeps_spaces_at_a_hard_break() {
        // Unlike wrap_text, the break invents no word boundaries and drops no
        // whitespace: the wrapped pieces re-concatenate to the original line.
        let line = "  indented   with  runs  and  more  padding  ";
        let lines = wrap_verbatim(line, 10);
        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), line);
    }

    #[test]
    fn wrap_verbatim_with_zero_width_does_not_wrap() {
        assert_eq!(wrap_verbatim("a  b", 0), vec!["a  b"]);
    }

    // --- message_lines ---

    #[test]
    fn message_lines_prefixes_assistant_bullet() {
        let lines = message_lines(Role::Assistant, "hello", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(plain(&lines[0]), "● hello");
    }

    #[test]
    fn message_lines_prefixes_user_bullet() {
        let lines = message_lines(Role::User, "hello", 80);
        assert_eq!(plain(&lines[0]).trim_end(), "❯ hello");
    }

    #[test]
    fn message_lines_indents_wrapped_continuation_lines() {
        // width 8 → content width 6 → "hello"/"world" on separate lines.
        let lines = message_lines(Role::Assistant, "hello world", 8);
        assert!(lines.len() >= 2);
        assert_eq!(plain(&lines[0]), "● hello");
        assert_eq!(plain(&lines[1]), "  world");
    }

    #[test]
    fn message_lines_colours_the_bullet() {
        let lines = message_lines(Role::Assistant, "hi", 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(AI_COLOR));
    }

    #[test]
    fn message_lines_renders_errors_with_a_red_bullet() {
        let lines = message_lines(Role::Error, "stream failed", 80);
        assert_eq!(lines.len(), 1);
        assert!(plain(&lines[0]).contains("stream failed"));
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(ERROR_COLOR),
            "the error bullet is red, not white"
        );
        assert_ne!(ERROR_COLOR, AI_COLOR, "error colour differs from assistant");
    }

    #[test]
    fn message_lines_applies_background_to_user_lines() {
        let lines = message_lines(Role::User, "hi there long enough to wrap", 10);
        for line in &lines {
            assert_eq!(
                line.style.bg,
                Some(USER_BG_COLOR),
                "every user line has the background"
            );
        }
    }

    #[test]
    fn message_lines_user_spans_fill_the_full_width() {
        let width = 20u16;
        let lines = message_lines(Role::User, "hi", width);
        for line in &lines {
            let span_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(
                span_chars as u16, width,
                "spans cover full width so background extends edge-to-edge"
            );
        }
    }

    #[test]
    fn message_lines_pads_user_lines_to_full_display_width() {
        // Padding must count terminal columns, not chars: a CJK user line still
        // fills the row edge-to-edge so its dark background does not fall short.
        let width = 20u16;
        let lines = message_lines(Role::User, "你好", width);
        for line in &lines {
            let total: usize = line.spans.iter().map(|s| cols(s.content.as_ref())).sum();
            assert_eq!(total as u16, width, "user line fills full display width");
        }
    }

    // --- tool_lines (collapsed, colour-by-status) ---

    #[test]
    fn tool_lines_header_shows_name_and_args() {
        let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Ok, "a\nb\nc"), 80);
        assert_eq!(plain(&lines[0]), "● Bash(cargo test)");
    }

    #[test]
    fn tool_lines_header_omits_the_parens_when_args_are_empty() {
        // A `!` shell command is a tool with no args (name = the command), so
        // its header reads `● {command}`, not `● {command}()`.
        let lines = tool_lines(&tool("echo hi", "", ToolStatus::Ok, "hi"), 80);
        assert_eq!(plain(&lines[0]), "● echo hi");
    }

    #[test]
    fn tool_lines_colours_the_bullet_by_status() {
        for (status, color) in [
            (ToolStatus::Running, TOOL_RUNNING_COLOR),
            (ToolStatus::Ok, TOOL_OK_COLOR),
            (ToolStatus::Failed, TOOL_FAIL_COLOR),
        ] {
            let lines = tool_lines(&tool("X", "y", status, "out"), 80);
            assert_eq!(
                lines[0].spans[0].style.fg,
                Some(color),
                "bullet colour tracks status {status:?}"
            );
        }
    }

    #[test]
    fn tool_lines_collapses_multiline_output_to_a_peek_plus_expand_hint() {
        let lines = tool_lines(&tool("Read", "f", ToolStatus::Ok, "one\ntwo\nthree"), 80);
        assert_eq!(lines.len(), 3, "header + peek + hint");
        let peek = plain(&lines[1]);
        assert!(
            peek.contains("one"),
            "peek shows the first output line: {peek:?}"
        );
        assert!(!peek.contains("two"), "the rest is hidden inline: {peek:?}");
        let hint = plain(&lines[2]);
        assert!(
            hint.contains("+2 lines"),
            "hint counts hidden lines: {hint:?}"
        );
        assert!(
            hint.contains("ctrl+o to expand"),
            "hint mentions ctrl+o: {hint:?}"
        );
    }

    #[test]
    fn tool_lines_single_line_output_has_no_expand_hint() {
        let lines = tool_lines(&tool("Bash", "echo hi", ToolStatus::Ok, "hi"), 80);
        assert_eq!(lines.len(), 2, "header + peek only, nothing hidden");
        assert!(plain(&lines[1]).contains("hi"));
    }

    #[test]
    fn tool_lines_running_shows_a_running_peek() {
        let lines = tool_lines(&tool("Bash", "sleep 1", ToolStatus::Running, ""), 80);
        assert_eq!(lines.len(), 2);
        assert!(
            plain(&lines[1]).to_lowercase().contains("running"),
            "a running tool peeks as running: {:?}",
            plain(&lines[1])
        );
    }

    #[test]
    fn tool_lines_truncates_a_long_peek_to_the_width() {
        // A peek line never overflows the terminal width (column-aware).
        let long = "x".repeat(200);
        let lines = tool_lines(&tool("Bash", "y", ToolStatus::Ok, &long), 30);
        for line in &lines {
            assert!(cols(&plain(line)) <= 30, "no line exceeds the width");
        }
    }

    // --- tool-output view: the full conversation transcript (Ctrl+O overlay) ---

    /// Drive an app through `user → "let me check" → Read(ok) → "all done"`.
    fn transcript_fixture() -> App {
        let mut app = App::new();
        app.record_user_message("hello");
        app.begin_stream();
        app.push_chunk("let me check");
        app.flush_streaming_segment();
        app.start_tool("Read", "f");
        app.end_tool("L1\nL2\nL3", true);
        app.push_chunk("all done");
        app.finish_stream();
        app
    }

    #[test]
    fn transcript_lines_interleaves_messages_and_full_tool_output_in_order() {
        let app = transcript_fixture();
        let texts: Vec<String> = transcript_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // User + both assistant segments are present (the new behaviour).
        assert!(texts.iter().any(|t| t == "❯ hello"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "● let me check"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "● all done"), "{texts:?}");
        // The tool's FULL output is present — every line, not collapsed.
        assert!(texts.iter().any(|t| t == "● Read(f)"));
        for needle in ["L1", "L2", "L3"] {
            assert!(texts.iter().any(|t| t.contains(needle)), "missing {needle}");
        }
        // …in the exact order they happened: user, text, tool+output, text.
        let pos = |needle: &str| texts.iter().position(|t| t.contains(needle)).unwrap();
        assert!(pos("hello") < pos("let me check"));
        assert!(pos("let me check") < pos("Read(f)"));
        assert!(pos("L3") < pos("all done"));
    }

    #[test]
    fn transcript_lines_shows_the_in_progress_reply_and_running_tool() {
        // The live tail (not yet in history) is included so the view updates live.
        let mut app = App::new();
        app.record_user_message("q");
        app.begin_stream();
        app.push_chunk("partial answer");
        let texts: Vec<String> = transcript_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t.contains("partial answer")));

        // Now a tool starts running (text flushed); it shows as running.
        app.flush_streaming_segment();
        app.start_tool("Bash", "ls");
        let texts: Vec<String> = transcript_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t == "● Bash(ls)"));
        assert!(texts.iter().any(|t| t.to_lowercase().contains("running")));
    }

    #[test]
    fn tool_full_lines_colours_the_header_by_status() {
        let lines = tool_full_lines(&tool("Read", "f", ToolStatus::Ok, "x"), 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_OK_COLOR));
    }

    #[test]
    fn a_shell_tools_full_output_keeps_its_whitespace_verbatim() {
        // `ls -l` columns / `tree` guides / indented code must survive the
        // expanded (Ctrl+O) view byte-for-byte: the collapsed inline peek shows
        // these lines verbatim (`truncate_cols`), so the "full output" view
        // must never be *less* faithful by collapsing the space runs.
        let mut t = tool(
            "tree",
            "",
            ToolStatus::Ok,
            "/home/me\n│   ├── a\n    indented   run",
        );
        t.shell = true;
        let lines: Vec<String> = tool_full_lines(&t, 80).iter().map(plain).collect();
        assert_eq!(
            lines,
            vec!["  ⎿ /home/me", "    │   ├── a", "        indented   run",],
            "every output line verbatim under the corner"
        );
    }

    #[test]
    fn a_backend_tools_full_output_keeps_its_whitespace_verbatim() {
        let lines: Vec<String> = tool_full_lines(
            &tool(
                "Bash",
                "ls -l",
                ToolStatus::Ok,
                "total 8\n-rw-  1 user   42 a",
            ),
            80,
        )
        .iter()
        .map(plain)
        .collect();
        assert_eq!(
            lines,
            vec!["● Bash(ls -l)", "  total 8", "  -rw-  1 user   42 a"],
            "the indented body preserves the output's space runs"
        );
    }

    #[test]
    fn transcript_lines_when_empty_is_a_placeholder() {
        let lines = transcript_lines(&App::new(), 80);
        assert!(
            plain(&lines[0]).to_lowercase().contains("nothing"),
            "{:?}",
            plain(&lines[0])
        );
    }

    #[test]
    fn render_tool_view_shows_the_title_messages_and_full_output() {
        let app = transcript_fixture();
        let mut buf = buffer(40, 14);
        render_tool_view(buf.area, &mut buf, &app);
        let all: String = (0..14)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Conversation"), "title present: {all:?}");
        assert!(all.contains("hello"), "user message shown: {all:?}");
        assert!(
            all.contains("let me check") && all.contains("all done"),
            "{all:?}"
        );
        assert!(all.contains("Read(f)"), "{all:?}");
        assert!(
            all.contains("L1") && all.contains("L2"),
            "full output: {all:?}"
        );
    }

    #[test]
    fn render_tool_view_scrolls_past_the_top() {
        let mut app = App::new();
        app.start_tool("Read", "f");
        let output = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.end_tool(&output, true);
        app.view = crate::app::View::ToolOutput;
        app.tool_scroll = 9;
        let mut buf = buffer(40, 8);
        render_tool_view(buf.area, &mut buf, &app);
        let all: String = (0..8)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !all.contains("line0"),
            "scrolled past the first line: {all:?}"
        );
        assert!(all.contains("line"), "still shows some output: {all:?}");
    }

    #[test]
    fn tool_view_max_scroll_is_total_lines_minus_the_body() {
        let app = transcript_fixture();
        let total = transcript_lines(&app, 40).len();
        let screen_h = 10u16;
        let body = (screen_h - TOOL_VIEW_TITLE_ROWS) as usize;
        assert_eq!(
            tool_view_max_scroll(&app, 40, screen_h),
            total.saturating_sub(body)
        );
    }

    // --- timestamps: only the user message's, bottom-right, transcript-only ---

    const STAMP: &str = "03:20 AM";

    /// A user message, a tool call, and an assistant reply, all carrying a stamp
    /// (only the user's may show).
    fn stamped_history() -> Vec<HistoryItem> {
        vec![
            HistoryItem::Message(Message {
                role: Role::User,
                text: "hi".to_string(),
                timestamp: STAMP.to_string(),
            }),
            HistoryItem::Tool(ToolCall {
                name: "Read".to_string(),
                args: "f".to_string(),
                status: ToolStatus::Ok,
                output: "out".to_string(),
                timestamp: STAMP.to_string(),
                shell: false,
                truncated: false,
            }),
            HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "hello".to_string(),
                timestamp: STAMP.to_string(),
            }),
        ]
    }

    #[test]
    fn transcript_puts_the_user_stamp_on_its_own_line_bottom_right() {
        let mut app = App::new();
        app.history = stamped_history();
        let width = 60u16;
        let texts: Vec<String> = transcript_lines(&app, width).iter().map(plain).collect();

        let header = texts
            .iter()
            .position(|t| t.trim_end() == "❯ hi")
            .expect("the user header carries no stamp");
        assert_eq!(
            texts[header + 1].trim_end(),
            "",
            "a blank row separates the message from its stamp"
        );
        let stamp = &texts[header + 2];
        assert_eq!(stamp.trim(), STAMP, "the stamp sits alone on its own line");
        assert_eq!(
            cols(stamp),
            width as usize,
            "the stamp is flush to the right edge: {stamp:?}"
        );
    }

    #[test]
    fn transcript_shows_no_stamp_on_assistant_tool_or_summary_items() {
        let mut app = App::new();
        app.history = vec![
            HistoryItem::Message(Message {
                role: Role::Assistant,
                text: "hello".to_string(),
                timestamp: STAMP.to_string(),
            }),
            HistoryItem::Tool(ToolCall {
                name: "Read".to_string(),
                args: "f".to_string(),
                status: ToolStatus::Ok,
                output: "out".to_string(),
                timestamp: STAMP.to_string(),
                shell: false,
                truncated: false,
            }),
            HistoryItem::Summary(TurnSummary {
                verb: "Done",
                secs: 12,
                timestamp: STAMP.to_string(),
            }),
        ];
        let all: String = transcript_lines(&app, 60)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !all.contains(STAMP),
            "only the user message shows a stamp: {all:?}"
        );
    }

    #[test]
    fn transcript_styles_the_user_stamp_dim() {
        let mut app = App::new();
        app.history = stamped_history();
        let stamp_line = transcript_lines(&app, 60)
            .into_iter()
            .find(|l| plain(l).trim() == STAMP)
            .expect("the user stamp line");
        let stamp_span = stamp_line
            .spans
            .iter()
            .find(|s| s.content.contains(STAMP))
            .expect("the stamp span");
        assert_eq!(stamp_span.style.fg, Some(TIMESTAMP_COLOR));
    }

    #[test]
    fn transcript_omits_the_stamp_line_for_an_empty_user_stamp() {
        // No clock injected (the unit-test default) → no stamp line, no extra
        // blank under the user message.
        let mut app = App::new();
        app.history = vec![msg(Role::User, "hi"), msg(Role::Assistant, "hello")];
        let texts: Vec<String> = transcript_lines(&app, 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts, vec!["❯ hi", "", "● hello", ""]);
    }

    #[test]
    fn inline_conversation_never_shows_the_timestamp() {
        // The "only in Ctrl+O" invariant: the inline repaint path must never
        // carry a stamp, even though its history items hold one.
        let history = stamped_history();
        let inline: String = conversation_lines(&history, 80)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !inline.contains(STAMP),
            "timestamps never leak into the inline view: {inline:?}"
        );
    }

    // --- live status line + committed "Done" summary (docs/status-indicator.md) ---

    /// A live status with the given metrics (verb fixed to "Working").
    fn status(tokens: usize, arrow: TokenArrow, elapsed: u64, thinking: Option<u64>) -> TurnStatus {
        TurnStatus {
            verb: "Working",
            done_verb: "Done",
            tokens,
            arrow,
            elapsed: Duration::from_secs(elapsed),
            thinking: thinking.map(Duration::from_secs),
            shell: false,
        }
    }

    /// The RGB triple of a span's foreground (panics on a non-RGB colour).
    fn span_rgb(span: &Span) -> (u8, u8, u8) {
        match span.style.fg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected an RGB fg, got {other:?}"),
        }
    }

    /// The spinner contributes the first [`SPINNER_SPAN_COUNT`] spans of the
    /// status line; the shimmering verb's per-char spans start right after.
    const VERB_START: usize = SPINNER_SPAN_COUNT;

    #[test]
    fn status_line_just_submitted_shows_only_the_verb_and_seconds() {
        let line = status_line(&status(0, TokenArrow::Down, 0, None));
        let text = plain(&line);
        assert_eq!(
            text, "(●•·   ) Working… (0s · esc to interrupt)",
            "the bare just-submitted state, comet on the first frame"
        );
        assert!(
            !text.contains("tokens"),
            "no token clause while the tally is 0"
        );
        assert!(!text.contains("Thinking"), "no thinking clause");
    }

    #[test]
    fn status_line_shows_the_token_tally_with_a_down_arrow() {
        let text = plain(&status_line(&status(100, TokenArrow::Down, 1, None)));
        assert!(
            text.ends_with("Working… (1s · ↓ 100 tokens · esc to interrupt)"),
            "{text:?}"
        );
    }

    #[test]
    fn status_line_flips_to_an_up_arrow_after_a_tool() {
        let text = plain(&status_line(&status(200, TokenArrow::Up, 1, None)));
        assert!(
            text.contains("↑ 200 tokens"),
            "up arrow after a tool: {text:?}"
        );
        assert!(!text.contains('↓'), "not the down arrow: {text:?}");
    }

    #[test]
    fn status_line_shows_thinking_only_while_thinking() {
        let thinking = plain(&status_line(&status(150, TokenArrow::Down, 1, Some(0))));
        assert!(
            thinking.ends_with("Working… (1s · ↓ 150 tokens · Thinking for 0s · esc to interrupt)"),
            "{thinking:?}"
        );
        let not = plain(&status_line(&status(150, TokenArrow::Down, 1, None)));
        assert!(
            !not.contains("Thinking"),
            "dropped once thinking ends: {not:?}"
        );
    }

    #[test]
    fn status_line_always_ends_with_the_interrupt_hint() {
        // Codex's discoverability hint, the detail's final dim clause in
        // every phase (docs/interrupt.md).
        for status in [
            status(0, TokenArrow::Down, 0, None),
            status(100, TokenArrow::Down, 1, None),
            status(150, TokenArrow::Down, 1, Some(0)),
        ] {
            let text = plain(&status_line(&status));
            assert!(text.ends_with(" · esc to interrupt)"), "{text:?}");
        }
    }

    #[test]
    fn status_spinner_comet_sweeps_between_the_walls_and_its_tail_whips() {
        // The comet spinner advances one frame per SPINNER_INTERVAL (80 ms):
        // the head sweeps out to the right wall dragging its fading tail,
        // bounces (the tail whipping around behind it), sweeps back to the
        // left wall (flush against `(`, using the leftmost cell), and loops.
        let frame_at = |ms: u64| {
            let mut s = status(0, TokenArrow::Down, 0, None);
            s.elapsed = Duration::from_millis(ms);
            let text = plain(&status_line(&s));
            text.chars().take_while(|&c| c != 'W').collect::<String>()
        };
        assert_eq!(
            frame_at(0).trim_end(),
            "(●•·   )",
            "head flush at the left wall, tail trailing right"
        );
        assert_eq!(
            frame_at(80).trim_end(),
            "(•●    )",
            "one frame later — moving right, the tail behind (its faint end under the head)"
        );
        assert_eq!(
            frame_at(160).trim_end(),
            "(·•●   )",
            "the full tail streams out behind the head"
        );
        assert_eq!(
            frame_at(400).trim_end(),
            "(   ·•●)",
            "head out at the right wall"
        );
        assert_eq!(
            frame_at(480).trim_end(),
            "(    ●•)",
            "bounced — the tail whips around to trail rightward"
        );
        assert_eq!(
            frame_at(720).trim_end(),
            "( ●•·  )",
            "sweeping back toward the left wall"
        );
        assert_eq!(
            frame_at(800),
            frame_at(0),
            "the sweep loops after a full 10-frame cycle"
        );
    }

    #[test]
    fn status_line_has_a_white_comet_fading_tail_shimmering_verb_and_dim_metrics() {
        let line = status_line(&status(0, TokenArrow::Down, 0, None));
        // The spinner: a white bold comet head dragging a tail that fades
        // through mid grey to the dim detail grey, between dim walls.
        let head = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "●")
            .expect("the comet's head span");
        assert_eq!(head.style.fg, Some(STATUS_COLOR), "white head");
        assert!(
            head.style.add_modifier.contains(Modifier::BOLD),
            "the head is bold"
        );
        assert_eq!(STATUS_COLOR, AI_COLOR, "the status white is the text white");
        let mid = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "•")
            .expect("the tail's mid span");
        assert_eq!(mid.style.fg, Some(SPINNER_TAIL_COLOR), "mid-grey tail cell");
        let faint = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "·")
            .expect("the tail's faint span");
        assert_eq!(
            faint.style.fg,
            Some(STATUS_DETAIL_COLOR),
            "the tail's end fades to the dim detail grey"
        );
        // The fade is monotonic: head brighter than mid, mid than the end.
        let grey = |c: Option<Color>| match c {
            Some(Color::Rgb(r, _, _)) => r,
            other => panic!("expected an RGB fg, got {other:?}"),
        };
        assert!(
            grey(head.style.fg) > grey(mid.style.fg) && grey(mid.style.fg) > grey(faint.style.fg),
            "the tail fades behind the head"
        );
        assert_eq!(line.spans[0].content.as_ref(), "(", "the left wall span");
        assert_eq!(
            line.spans[0].style.fg,
            Some(STATUS_DETAIL_COLOR),
            "dim left wall"
        );
        assert_eq!(
            line.spans[SPINNER_SPAN_COUNT - 1].content.as_ref(),
            ") ",
            "the right wall carries the separator space"
        );
        assert_eq!(
            line.spans[SPINNER_SPAN_COUNT - 1].style.fg,
            Some(STATUS_DETAIL_COLOR),
            "dim right wall"
        );
        // The verb renders one bold span per char (the shimmer), every char a
        // greyscale white between the base and the bright highlight.
        let verb = "Working…";
        let verb_spans = &line.spans[VERB_START..VERB_START + verb.chars().count()];
        assert_eq!(
            verb_spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            verb,
            "per-char verb spans"
        );
        for span in verb_spans {
            let (r, g, b) = span_rgb(span);
            assert!(r == g && g == b, "greyscale white, got ({r},{g},{b})");
            assert!(r >= SHIMMER_BASE.0, "never dimmer than the base");
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "verb chars are bold"
            );
        }
        assert_eq!(
            line.spans.last().unwrap().style.fg,
            Some(STATUS_DETAIL_COLOR),
            "dim metrics"
        );
    }

    #[test]
    fn status_verb_wave_peaks_where_the_band_is_and_moves_with_time() {
        // "Working…" is 8 chars → period = 8 + 2·10 = 28. The crest sits on
        // char 0 when pos = padding (10), i.e. elapsed = 10/28 · 2 s ≈ 715 ms:
        // char 0 must then be brighter than char 7 (7 chars away — outside the
        // band, so at the base colour).
        let mut at_crest = status(0, TokenArrow::Down, 0, None);
        at_crest.elapsed = Duration::from_millis(715);
        let crest_on_first = status_line(&at_crest);
        let first = span_rgb(&crest_on_first.spans[VERB_START]);
        let last = span_rgb(&crest_on_first.spans[VERB_START + 7]);
        assert!(
            first.0 > last.0,
            "the band's crest is brighter than off-band chars: {first:?} vs {last:?}"
        );
        assert_eq!(last.0, SHIMMER_BASE.0, "off-band chars sit at the base");

        // Half a sweep later the band has moved on: char 0 is no longer the peak.
        let mut moved_on = status(0, TokenArrow::Down, 0, None);
        moved_on.elapsed = Duration::from_millis(715 + 1000);
        let first_later = span_rgb(&status_line(&moved_on).spans[VERB_START]);
        assert!(
            first_later.0 < first.0,
            "the wave moved off char 0 as time advanced: {first_later:?} vs {first:?}"
        );
    }

    #[test]
    fn summary_lines_is_a_single_dim_bulletless_line() {
        let summary = TurnSummary {
            verb: "Done",
            secs: 20,
            timestamp: String::new(),
        };
        let lines = summary_lines(&summary, 80);
        assert_eq!(lines.len(), 1, "one line");
        assert_eq!(plain(&lines[0]), "Done for 20s");
        assert!(!plain(&lines[0]).contains('●'), "no bullet");
        assert_eq!(lines[0].spans[0].style.fg, Some(STATUS_DONE_COLOR), "dim");
    }

    #[test]
    fn conversation_lines_renders_a_committed_turn_summary() {
        let history = [
            msg(Role::Assistant, "all done"),
            HistoryItem::Summary(TurnSummary {
                verb: "Done",
                secs: 7,
                timestamp: String::new(),
            }),
        ];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t == "● all done"));
        assert!(
            texts.iter().any(|t| t == "Done for 7s"),
            "the summary flows inline: {texts:?}"
        );
    }

    #[test]
    fn render_live_draws_the_status_row_while_streaming() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("hi");
        app.set_status_times(Duration::from_secs(3), None);
        let h = live_height(&app.input, 40, 24, true, true, 0, 0, 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains("Working…") && all.contains("3s"),
            "the live status line is drawn in the strip: {all:?}"
        );
    }

    #[test]
    fn the_pre_stream_pause_shows_up_tokens_and_no_preview_bullet() {
        // After submit, before the first chunk arrives: the user's input is
        // counted into the tally (arrow ↑), the status spins, and the empty
        // reply buffer shows NO preview bullet (just the status line).
        let mut app = App::new();
        app.begin_stream();
        app.count_user_input("hello there"); // ↑ N tokens
        // No preview content yet → the strip is status + gap only (no preview
        // row, no preview gap): exactly the box + status + the two gaps.
        assert!(!strip_has_preview(&app));
        let h = live_height(&app.input, 60, 24, true, false, 0, 0, 0);
        assert_eq!(
            h,
            STATUS_ROWS + STATUS_GAP_ROWS + INPUT_CHROME_ROWS + 1,
            "no preview row is reserved during the pause"
        );
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        // The status sits on the strip's top row (row 0) — no blank preview
        // line above it.
        assert!(
            row(&buf, 0, 60).contains('↑') && row(&buf, 0, 60).contains("tokens"),
            "the status with ↑ tokens is the strip's first row: {:?}",
            row(&buf, 0, 60)
        );
        // No row is an assistant preview line (`● ` at the start) — the only
        // `●` on screen is the comet spinner's head inside `(●•·   )`.
        for y in 0..h {
            assert!(
                !row(&buf, y, 60).starts_with("● "),
                "no reserved preview bullet row: {:?}",
                row(&buf, y, 60)
            );
        }
    }

    // --- render_live (against a plain Buffer) ---

    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn render_live_grows_the_box_and_wraps_input_across_rows() {
        let mut app = App::new();
        app.input = TextArea::from_text("first\nsecond");
        let h = live_height(&app.input, 20, 24, false, false, 0, 0, 0);
        assert_eq!(h, 4, "two rules + two input rows (no strip when idle)");
        let mut buf = buffer(20, h);
        render_live(buf.area, &mut buf, &app);

        assert_eq!(buf[(0, 0)].symbol(), "─", "top rule");
        assert!(row(&buf, 1, 20).contains("❯ first"), "prompt on first line");
        assert!(
            row(&buf, 2, 20).contains("second") && !row(&buf, 2, 20).contains("❯"),
            "continuation line is indented, no prompt"
        );
        assert_eq!(
            buf[(0, 3)].symbol(),
            "─",
            "bottom rule moved down as box grew"
        );
    }

    #[test]
    fn render_live_scrolls_input_to_keep_the_end_visible() {
        // Six input lines but a terminal that only fits four text rows: the box
        // shows the tail (so the cursor's line stays visible), not the head.
        let mut app = App::new();
        app.input = TextArea::from_text(
            &(0..6)
                .map(|i| format!("line{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let term_h = 6; // live clamps to 6 → text rows = 6 - 2 = 4
        assert_eq!(
            live_height(&app.input, 20, term_h, false, false, 0, 0, 0),
            6
        );
        let mut buf = buffer(20, 6);
        render_live(buf.area, &mut buf, &app);

        let text: String = (1..5).map(|y| row(&buf, y, 20)).collect();
        assert!(text.contains("line5"), "last line is visible: {text:?}");
        assert!(!text.contains("line0"), "first line scrolled off: {text:?}");
    }

    #[test]
    fn render_live_strip_stacks_preview_gap_status_then_gap_above_the_box() {
        // While streaming the strip is four rows: the reply preview, a blank gap,
        // the live status line, then another blank gap — and only below them the
        // box's top rule, so the status never butts up against the box.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("streaming reply");
        let mut buf = buffer(40, 7); // preview + gap + status + gap + (two rules + one input)
        render_live(buf.area, &mut buf, &app);

        assert!(
            row(&buf, 0, 40).contains("streaming reply"),
            "preview on row 0"
        );
        assert!(
            row(&buf, 1, 40).trim().is_empty(),
            "blank gap row below the preview"
        );
        assert!(
            row(&buf, 2, 40).contains("Working"),
            "status line on row 2: {:?}",
            row(&buf, 2, 40)
        );
        assert!(
            row(&buf, 3, 40).trim().is_empty(),
            "blank gap row below the status line"
        );
        assert_eq!(
            buf[(0, 4)].symbol(),
            "─",
            "top rule below the status gap, not touching the status"
        );
    }

    #[test]
    fn render_live_has_no_strip_when_idle() {
        // Idle, the box sits directly under the chat — no preview/gap strip — so
        // the only separation is the committed blank after the last message.
        let mut app = App::new();
        app.input = TextArea::from_text("hello");
        let mut buf = buffer(40, 3); // just the box: two rules + one input row
        render_live(buf.area, &mut buf, &app);

        assert_eq!(
            buf[(0, 0)].symbol(),
            "─",
            "top rule on row 0 — no preview strip above it"
        );
        assert!(row(&buf, 1, 40).contains("❯ hello"), "input on row 1");
        assert_eq!(buf[(0, 2)].symbol(), "─", "bottom rule on row 2");
    }

    #[test]
    fn cursor_sits_on_the_last_wrapped_input_row() {
        // "ab\ncd" → two rows; the cursor follows the end onto the second text
        // row (y = 2) just after the indented "cd" (x = 2 + 2).
        let mut app = App::new();
        app.input = TextArea::from_text("ab\ncd");
        let area = Rect::new(
            0,
            0,
            20,
            live_height(&app.input, 20, 24, false, false, 0, 0, 0),
        );
        assert_eq!(cursor_position(area, &app), (4, 2));
    }

    // --- streaming commit bookkeeping ---

    #[test]
    fn incremental_commits_reconstruct_the_whole_reply() {
        // Stream a reply word-by-word, committing stable lines as we go, and
        // confirm the committed lines (plus the final flush) exactly equal the
        // fully-rendered message — no gaps, no duplicates, no reordering.
        let full = "the quick brown fox jumps over the lazy dog and then \
                    some extra words to force several wrapped lines here";
        let width = 20;
        let expected: Vec<String> = message_lines(Role::Assistant, full, width)
            .iter()
            .map(plain)
            .collect();

        let mut committed = 0;
        let mut got: Vec<String> = Vec::new();
        let mut acc = String::new();
        for chunk in crate::stream::chunks(full) {
            acc.push_str(&chunk);
            let (lines, new_committed) = stable_commit(&acc, width, committed);
            got.extend(lines.iter().map(plain));
            committed = new_committed;
        }
        got.extend(final_commit(&acc, width, committed).iter().map(plain));

        assert_eq!(got, expected);
    }

    #[test]
    fn stable_commit_withholds_the_last_line() {
        // "hi there" fits one line → nothing is stable yet.
        let (lines, committed) = stable_commit("hi there", 80, 0);
        assert!(lines.is_empty());
        assert_eq!(committed, 0);
    }

    #[test]
    fn stable_commit_clamps_when_a_resize_shrinks_the_line_count() {
        // A mid-stream width *grow* re-wraps the same reply to fewer lines, so
        // the previously-committed count can exceed the new stable count. The
        // clamps must absorb that: no slice panic, an empty new batch, and a
        // committed counter that never regresses. (Raw indexing here would
        // panic with `start > end`.)
        let text = "the quick brown fox jumps over the lazy dog";
        let (_, committed_narrow) = stable_commit(text, 6, 0);
        assert!(committed_narrow >= 1, "a narrow wrap commits several lines");

        let (lines, committed_after) = stable_commit(text, 80, committed_narrow);
        assert!(lines.is_empty(), "re-wrapped wider, nothing new is stable");
        assert_eq!(
            committed_after, committed_narrow,
            "committed never regresses"
        );
    }

    #[test]
    fn final_commit_clamps_an_over_large_committed_count() {
        // If `committed` outruns the re-wrapped line count (e.g. after a resize),
        // final_commit returns nothing rather than panicking on `lines[start..]`.
        assert!(final_commit("a short reply", 80, 999).is_empty());
    }

    #[test]
    fn cursor_sits_after_the_prompt_and_input() {
        // Idle (the only time the cursor shows): the box fills the area, so on a
        // 40x4 area the text row sits at row 1, flush-left (no side border).
        // Empty input → cursor right after "❯ ".
        let mut app = App::new();
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), &app), (2, 1));
        app.input = TextArea::from_text("hi");
        assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), &app), (4, 1));
    }

    // --- growing input box: height + re-pin geometry ---

    #[test]
    fn live_height_is_minimal_for_short_input() {
        // Idle, empty or one-line input → a one-row box framed by two rules
        // (no preview strip) = LIVE_MIN_HEIGHT (3).
        assert_eq!(LIVE_MIN_HEIGHT, 3);
        assert_eq!(
            live_height(&TextArea::from_text(""), 40, 24, false, false, 0, 0, 0),
            LIVE_MIN_HEIGHT
        );
        assert_eq!(
            live_height(&TextArea::from_text("hi"), 40, 24, false, false, 0, 0, 0),
            LIVE_MIN_HEIGHT
        );
    }

    #[test]
    fn live_height_adds_the_streaming_strip_above_the_box() {
        // While streaming, the live region gains a preview row, a blank gap row,
        // the live status row, and a blank gap below it (PREVIEW_ROWS + GAP_ROWS
        // + STATUS_ROWS + STATUS_GAP_ROWS = 4) above whatever the idle box would be.
        for input in ["", "hi", "a\nb\nc"] {
            let ta = TextArea::from_text(input);
            assert_eq!(
                live_height(&ta, 40, 24, true, true, 0, 0, 0),
                live_height(&ta, 40, 24, false, false, 0, 0, 0) + 4,
                "streaming adds the preview + gap + status + gap rows for {input:?}"
            );
        }
    }

    #[test]
    fn live_height_grows_one_row_per_wrapped_input_line() {
        // Idle, three explicit lines → the box has three text rows, so the live
        // region is 2 (two rules) + 3 = 5 rows tall.
        assert_eq!(
            live_height(
                &TextArea::from_text("a\nb\nc"),
                40,
                24,
                false,
                false,
                0,
                0,
                0
            ),
            5
        );
    }

    #[test]
    fn live_height_grows_when_a_long_line_soft_wraps() {
        // No explicit newline: a line longer than the field width wraps and the
        // box still grows. field width = 10 - 2 = 8, so 16 columns fill two rows
        // exactly and the wrap reserves the sentinel row for the end-of-text
        // cursor (docs/textarea.md) → 3 rows → 5.
        assert_eq!(
            live_height(
                &TextArea::from_text("abcdefghijklmnop"),
                10,
                24,
                false,
                false,
                0,
                0,
                0
            ),
            5
        );
    }

    #[test]
    fn live_height_is_clamped_to_the_terminal_height() {
        let many = TextArea::from_text(&"a\n".repeat(50));
        assert_eq!(
            live_height(&many, 40, 10, false, false, 0, 0, 0),
            10,
            "never taller than the screen"
        );
    }

    #[test]
    fn repin_keeps_the_box_top_anchored_growing_downward() {
        // Room below: grow in place, top fixed, no scroll, nothing to clear.
        assert_eq!(
            repin(3, 4, 6, 24),
            Repin {
                scroll_up: 0,
                top: 3,
                clear_below: 0
            }
        );
        // Unchanged height that already fits is a no-op.
        assert_eq!(
            repin(10, 5, 5, 24),
            Repin {
                scroll_up: 0,
                top: 10,
                clear_below: 0
            }
        );
    }

    #[test]
    fn repin_clears_below_on_a_shrink_and_leaves_the_top_put() {
        // Shrinking pulls the bottom up; the two vacated rows below get blanked.
        assert_eq!(
            repin(3, 6, 4, 24),
            Repin {
                scroll_up: 0,
                top: 3,
                clear_below: 2
            }
        );
    }

    #[test]
    fn repin_scrolls_up_only_when_the_box_overflows_the_bottom() {
        // At the bottom (top 20 + new height 6 = 26 > 24): scroll up 2 and pin.
        assert_eq!(
            repin(20, 4, 6, 24),
            Repin {
                scroll_up: 2,
                top: 18,
                clear_below: 0
            }
        );
    }

    // --- restore_cursor_row (where the shell prompt resumes on exit) ---

    #[test]
    fn restore_cursor_row_lands_just_below_a_top_anchored_box() {
        // Box at rows 0..3 of a 24-row screen → prompt resumes on row 3, NOT at
        // the screen bottom (which would leave a 20-row blank gap).
        assert_eq!(restore_cursor_row(0, 3, 24), Some(3));
    }

    #[test]
    fn restore_cursor_row_follows_the_box_down_the_screen() {
        // Box one row above the bottom → prompt on the last row, still no gap.
        assert_eq!(restore_cursor_row(20, 3, 24), Some(23));
    }

    #[test]
    fn restore_cursor_row_is_none_when_the_box_occupies_the_last_row() {
        // No room below (top 21 + height 3 = 24): caller scrolls up one instead.
        assert_eq!(restore_cursor_row(21, 3, 24), None);
    }

    // --- live-region layout: single source of truth ---

    #[test]
    fn live_layout_splits_the_area_into_the_strip_box_band_and_footer() {
        // The strip takes its rows (preview + gap while streaming, none when
        // idle); the band takes its fixed rows below the box; the footer the
        // very last row; the input box takes everything left — the four always
        // tile the whole area, at the minimum height and beyond, streaming or
        // not, band open or closed, footer shown or not.
        for streaming in [false, true] {
            for has_preview in [false, true] {
                for band_rows in [0, 3] {
                    for footer_rows in [0, 1] {
                        // The smallest height still fits the streaming strip (4) +
                        // band (3) + footer (1).
                        for h in [LIVE_MIN_HEIGHT + 5, 9, 20] {
                            let [strip, input, band, footer] = live_layout(
                                Rect::new(0, 0, 40, h),
                                streaming,
                                has_preview,
                                0,
                                band_rows,
                                footer_rows,
                            );
                            assert_eq!(
                                strip.height + input.height + band.height + footer.height,
                                h,
                                "sub-areas tile the area"
                            );
                            assert_eq!(strip.height, strip_rows(streaming, has_preview));
                            assert_eq!(band.height, band_rows);
                            assert_eq!(footer.height, footer_rows);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn cursor_row_sits_on_the_rendered_prompt_row() {
        // render_live and cursor_position both derive their geometry from
        // input_box, so the hardware cursor lands on exactly the row where the
        // prompt is drawn — they cannot drift apart.
        let area = Rect::new(0, 0, 40, LIVE_MIN_HEIGHT);
        let mut app = App::new();
        app.input = TextArea::from_text("x");
        let mut buf = Buffer::empty(area);
        render_live(area, &mut buf, &app);
        let (_, cy) = cursor_position(area, &app);
        let rendered: String = (0..area.width).map(|x| buf[(x, cy)].symbol()).collect();
        assert!(
            rendered.contains('❯'),
            "cursor row carries the prompt glyph"
        );
    }

    #[test]
    fn cursor_row_sits_on_the_prompt_row_while_a_turn_streams() {
        // Codex keeps the composer focused while a task runs — typing mid-turn
        // edits the draft (Enter queues it), so the cursor must land on the
        // box's prompt row even with the streaming strip *and* a queued
        // message stacked above it, not on a strip row.
        let mut app = App::new();
        app.begin_stream();
        app.queued.push_back(batch(&["world"]));
        app.input = TextArea::from_text("x");
        let q = queued_rows(&app, 40);
        let h = live_height(&app.input, 40, 24, true, true, q, 0, 0);
        let area = Rect::new(0, 0, 40, h);
        let mut buf = buffer(40, h);
        render_live(area, &mut buf, &app);
        let (cx, cy) = cursor_position(area, &app);
        let rendered = row(&buf, cy, 40);
        assert!(
            rendered.starts_with("❯ x"),
            "cursor row carries the box prompt, not the strip: {rendered:?}"
        );
        assert_eq!(cx, 3, "right after the typed text");
    }

    #[test]
    fn repaint_budget_is_the_screen_minus_the_live_region() {
        assert_eq!(
            repaint_budget(10, LIVE_MIN_HEIGHT),
            10 - LIVE_MIN_HEIGHT as usize
        );
        assert_eq!(
            repaint_budget(10, 9),
            1,
            "a taller live region leaves fewer rows"
        );
        assert_eq!(repaint_budget(LIVE_MIN_HEIGHT, LIVE_MIN_HEIGHT), 0);
        assert_eq!(repaint_budget(4, 9), 0, "saturates, never wraps");
    }

    #[test]
    fn bullet_prefixes_all_occupy_bullet_width_columns() {
        let bw = BULLET_WIDTH as usize;
        assert_eq!(cols(PROMPT), bw, "prompt width matches BULLET_WIDTH");
        assert_eq!(cols(USER_BULLET), bw, "user bullet matches BULLET_WIDTH");
        assert_eq!(cols(AI_BULLET), bw, "assistant bullet matches BULLET_WIDTH");
        assert_eq!(cols(ERROR_BULLET), bw, "error bullet matches BULLET_WIDTH");
        assert_eq!(cols(INDENT), bw, "continuation indent matches BULLET_WIDTH");
    }

    // --- conversation repaint (after a resize) ---

    fn msg(role: Role, text: &str) -> HistoryItem {
        HistoryItem::Message(Message {
            role,
            text: text.to_string(),
            timestamp: String::new(),
        })
    }

    fn tool(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            args: args.to_string(),
            status,
            output: output.to_string(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
        }
    }

    #[test]
    fn conversation_lines_lays_out_a_turn_with_a_trailing_blank() {
        let history = [msg(Role::User, "hi"), msg(Role::Assistant, "hello")];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // User line, blank, assistant line, blank spacer after the reply.
        assert_eq!(texts, vec!["❯ hi", "", "● hello", ""]);
    }

    #[test]
    fn conversation_lines_renders_a_tool_call_between_messages() {
        let history = [
            msg(Role::User, "hi"),
            HistoryItem::Tool(tool("Bash", "ls", ToolStatus::Ok, "a\nb")),
            msg(Role::Assistant, "done"),
        ];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // user, blank, tool header, tool peek, tool hint, blank, assistant, blank
        assert_eq!(texts.first().map(String::as_str), Some("❯ hi"));
        assert!(
            texts.iter().any(|t| t == "● Bash(ls)"),
            "tool header is present in order: {texts:?}"
        );
        assert!(texts.iter().any(|t| t == "● done"));
    }

    #[test]
    fn repaint_lines_keeps_only_the_last_max_rows() {
        // Lines are: "❯ one", "● two", "" → keep the last 2.
        let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
        let texts: Vec<String> = repaint_lines(&history, 80, 2).iter().map(plain).collect();
        assert_eq!(texts, vec!["● two", ""]);
    }

    #[test]
    fn repaint_lines_returns_everything_when_it_fits() {
        let history = [msg(Role::User, "hi")];
        assert_eq!(repaint_lines(&history, 80, 100).len(), 2); // message + blank
    }

    #[test]
    fn repaint_lines_of_empty_history_is_empty() {
        assert!(repaint_lines(&[], 80, 10).is_empty());
    }

    #[test]
    fn render_live_shows_streaming_text_in_preview_row() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("Hi there");
        let mut buf = buffer(40, 4);
        render_live(buf.area, &mut buf, &app);

        let preview = row(&buf, 0, 40);
        assert!(preview.contains("●"), "preview shows assistant bullet");
        assert!(preview.contains("Hi there"), "preview shows streamed text");
    }

    #[test]
    fn render_live_previews_a_running_tool_in_blue() {
        // While a tool runs, the strip's preview row shows its coloured header
        // (blue) instead of the assistant text, so the user sees what's executing.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Read", "src/main.rs");
        let mut buf = buffer(40, 5);
        render_live(buf.area, &mut buf, &app);

        let preview = row(&buf, 0, 40);
        assert!(
            preview.contains("Read(src/main.rs)"),
            "preview shows the running tool header: {preview:?}"
        );
        assert_eq!(
            buf[(0, 0)].fg,
            TOOL_RUNNING_COLOR,
            "the running tool's bullet is blue"
        );
    }

    // --- slash-command palette rendering + geometry ---

    /// An app whose input is `input` with the palette open at `selected`.
    fn palette(input: &str, selected: usize) -> App {
        let mut app = App::new();
        app.input = TextArea::from_text(input);
        app.command_menu = Some(crate::app::CommandMenu { selected });
        app
    }

    #[test]
    fn menu_window_keeps_the_selection_visible() {
        assert_eq!(menu_window(8, 0, 5), 0);
        assert_eq!(menu_window(8, 4, 5), 0, "within the first window");
        assert_eq!(menu_window(8, 5, 5), 1, "scrolls so the selection shows");
        assert_eq!(menu_window(8, 7, 5), 3, "clamped to the last window");
        assert_eq!(menu_window(3, 2, 5), 0, "no scroll when everything fits");
    }

    #[test]
    fn menu_rows_is_zero_when_the_palette_is_closed() {
        assert_eq!(menu_rows(&App::new()), 0);
    }

    #[test]
    fn menu_rows_counts_matches_capped_with_a_placeholder_for_none() {
        assert_eq!(
            menu_rows(&palette("/", 0)),
            (crate::app::COMMANDS.len() as u16).min(MENU_MAX_ROWS),
            "match count, capped at the max"
        );
        assert_eq!(menu_rows(&palette("/zzz", 0)), 1, "placeholder row");
    }

    #[test]
    fn command_menu_lines_lists_commands_with_descriptions() {
        let texts: Vec<String> = command_menu_lines(&palette("/", 0), 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t.contains("/help")), "{texts:?}");
        assert!(
            texts
                .iter()
                .any(|t| t.contains("List the available commands")),
            "{texts:?}"
        );
    }

    #[test]
    fn selecting_highlights_the_whole_row_in_one_consistent_colour() {
        // Two commands, the second highlighted. The selected row's name AND
        // description share the highlight colour (consistency); the other row
        // shares the dim colour. No caret/arrow.
        let lines = command_menu_lines(&palette("/", 1), 60);
        let name_fg = |l: &Line| l.spans[0].style.fg; // spans = [name, pad, desc]
        let desc_fg = |l: &Line| l.spans[2].style.fg;
        assert_eq!(
            name_fg(&lines[1]),
            Some(MENU_SELECTED_COLOR),
            "selected name"
        );
        assert_eq!(
            desc_fg(&lines[1]),
            name_fg(&lines[1]),
            "selected name matches its description colour"
        );
        assert_eq!(
            name_fg(&lines[0]),
            Some(MENU_DIM_COLOR),
            "other name dimmed"
        );
        assert_eq!(
            desc_fg(&lines[0]),
            name_fg(&lines[0]),
            "unselected name matches its description colour"
        );
        for line in &lines {
            assert!(!plain(line).contains('❯'), "no caret: {:?}", plain(line));
        }
    }

    #[test]
    fn command_menu_aligns_descriptions_in_a_column() {
        // Names are padded so every description starts at the same column,
        // regardless of command-name length.
        let lines = command_menu_lines(&palette("/", 0), 60);
        for line in &lines {
            let before_desc: usize = line.spans[..2]
                .iter()
                .map(|s| cols(s.content.as_ref()))
                .sum();
            assert_eq!(
                before_desc,
                MENU_DESC_COL,
                "desc column for {:?}",
                plain(line)
            );
        }
    }

    #[test]
    fn command_menu_shows_a_placeholder_when_nothing_matches() {
        let texts: Vec<String> = command_menu_lines(&palette("/zzz", 0), 60)
            .iter()
            .map(|l| plain(l).to_string())
            .collect();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].to_lowercase().contains("no matching"), "{texts:?}");
    }

    #[test]
    fn message_lines_renders_a_system_notice_with_a_cyan_bullet() {
        let lines = message_lines(Role::System, "a notice", 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(SYSTEM_COLOR));
        assert_ne!(SYSTEM_COLOR, AI_COLOR, "distinct from an AI reply");
    }

    #[test]
    fn live_height_adds_the_command_menu_band() {
        let closed = live_height(&TextArea::from_text("hi"), 40, 24, false, false, 0, 0, 0);
        let open = live_height(
            &TextArea::from_text("/"),
            40,
            24,
            false,
            false,
            0,
            MENU_MAX_ROWS,
            0,
        );
        assert_eq!(open, closed + MENU_MAX_ROWS, "the menu band adds its rows");
    }

    #[test]
    fn render_live_draws_the_command_menu_below_the_box() {
        let app = palette("/", 0);
        let menu = menu_rows(&app);
        let h = live_height(&app.input, 40, 24, false, false, 0, menu, 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains("/help"),
            "menu rendered below the box: {all:?}"
        );
        assert!(
            row(&buf, h - 1, 40).contains('/'),
            "a command sits on the last row"
        );
    }

    #[test]
    fn cursor_stays_in_the_box_when_the_palette_opens() {
        // The menu is reserved *below* the box, so opening the palette must not
        // move the cursor (the end of the input).
        let mut app = App::new();
        app.input = TextArea::from_text("/");
        let closed_area = Rect::new(
            0,
            0,
            40,
            live_height(&TextArea::from_text("/"), 40, 24, false, false, 0, 0, 0),
        );
        let closed = cursor_position(closed_area, &app);
        app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
        let menu = menu_rows(&app);
        let open_area = Rect::new(
            0,
            0,
            40,
            live_height(&TextArea::from_text("/"), 40, 24, false, false, 0, menu, 0),
        );
        let open = cursor_position(open_area, &app);
        assert_eq!(open, closed, "cursor unchanged when the menu opens");
    }

    // --- the `?` shortcuts band (docs/shortcuts.md) ---

    #[test]
    fn shortcuts_rows_is_zero_closed_and_counts_the_band_open() {
        let mut app = App::new();
        assert_eq!(shortcuts_rows(&app), 0);
        app.shortcuts_open = true;
        assert_eq!(
            shortcuts_rows(&app),
            SHORTCUTS.len().div_ceil(2) as u16,
            "two entries per row"
        );
    }

    #[test]
    fn shortcuts_band_advertises_ctrl_v_image_paste() {
        // The `?` overlay lists Ctrl+V image paste (docs/image-paste.md).
        let texts: Vec<String> = shortcuts_lines(false)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("ctrl+v for image paste")),
            "{texts:?}"
        );
    }

    #[test]
    fn shortcuts_lines_list_the_bindings_in_two_columns() {
        let texts: Vec<String> = shortcuts_lines(false)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts.len(), SHORTCUTS.len().div_ceil(2));
        assert!(
            texts[0].contains("/ for commands") && texts[0].contains("! for shell command"),
            "{texts:?}"
        );
        assert!(
            texts[1].contains("↑ for input history")
                && texts[1].contains("ctrl+r to search history"),
            "{texts:?}"
        );
        assert!(
            texts[2].contains("alt+enter for newline")
                && texts[2].contains("ctrl+o for tool output"),
            "{texts:?}"
        );
        assert!(
            texts[3].contains("esc to quit") && texts[3].contains("ctrl+c to quit"),
            "{texts:?}"
        );
        assert!(texts[4].contains("alt+↑ to edit queue"), "{texts:?}");
        // The second column is aligned: both rows' right keys start at the
        // same display column.
        let col = |t: &str, needle: &str| cols(&t[..t.find(needle).unwrap()]);
        assert_eq!(
            col(&texts[0], "!"),
            col(&texts[1], "ctrl+r"),
            "right column aligned: {texts:?}"
        );
    }

    #[test]
    fn shortcuts_lines_list_the_alt_up_queue_edit_binding() {
        // Alt+Up (pull the last queued batch back into the composer,
        // docs/queue.md) is discoverable in the `?` band like every other
        // binding.
        let all: String = shortcuts_lines(false)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("alt+↑ to edit queue"), "{all:?}");
    }

    #[test]
    fn shortcuts_lines_flip_the_esc_entry_while_a_turn_runs() {
        // codex's quit entry is context-sensitive: "to interrupt" while a task
        // runs. Our Esc entry flips the same way.
        let idle: Vec<String> = shortcuts_lines(false).iter().map(|l| plain(l)).collect();
        let busy: Vec<String> = shortcuts_lines(true).iter().map(|l| plain(l)).collect();
        assert!(idle.iter().any(|t| t.contains("esc to quit")), "{idle:?}");
        assert!(
            busy.iter().any(|t| t.contains("esc to interrupt")),
            "{busy:?}"
        );
        assert!(!busy.iter().any(|t| t.contains("esc to quit")), "{busy:?}");
    }

    #[test]
    fn shortcuts_lines_style_keys_cyan_and_labels_dim() {
        for line in shortcuts_lines(false) {
            // spans = [key, label, pad, key, label] — keys cyan, labels dim.
            assert_eq!(line.spans[0].style.fg, Some(SHORTCUTS_KEY_COLOR));
            assert_eq!(line.spans[1].style.fg, Some(SHORTCUTS_TEXT_COLOR));
        }
    }

    #[test]
    fn live_height_adds_the_shortcuts_band() {
        let mut app = App::new();
        app.shortcuts_open = true;
        let closed = live_height(&app.input, 40, 24, false, false, 0, 0, 0);
        let open = live_height(&app.input, 40, 24, false, false, 0, shortcuts_rows(&app), 0);
        assert_eq!(open, closed + shortcuts_rows(&app));
    }

    #[test]
    fn render_live_draws_the_shortcuts_band_below_the_box() {
        let mut app = App::new();
        app.shortcuts_open = true;
        let h = live_height(&app.input, 60, 24, false, false, 0, shortcuts_rows(&app), 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 60))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("/ for commands"), "band rendered: {all:?}");
        assert!(
            row(&buf, h - 1, 60).contains("ctrl+v for image paste"),
            "the last band row sits on the last region row"
        );
    }

    #[test]
    fn cursor_stays_in_the_box_when_the_shortcuts_band_opens() {
        let mut app = App::new();
        let closed_area = Rect::new(
            0,
            0,
            40,
            live_height(&app.input, 40, 24, false, false, 0, 0, 0),
        );
        let closed = cursor_position(closed_area, &app);
        app.shortcuts_open = true;
        let open_area = Rect::new(
            0,
            0,
            40,
            live_height(&app.input, 40, 24, false, false, 0, shortcuts_rows(&app), 0),
        );
        let open = cursor_position(open_area, &app);
        assert_eq!(open, closed, "cursor unchanged when the band opens");
    }

    // --- message queue (docs/queue.md) ---

    #[test]
    fn queued_rows_is_zero_empty_and_counts_the_queue() {
        let mut app = App::new();
        assert_eq!(queued_rows(&app, 40), 0);
        app.queued.push_back(batch(&["a", "b"]));
        assert_eq!(queued_rows(&app, 40), 2, "one short message per row");
    }

    #[test]
    fn queued_rows_counts_the_blank_between_batches() {
        // Two single-message batches occupy three rows — a blank divides them.
        let mut app = App::new();
        app.queued.push_back(batch(&["a"]));
        app.queued.push_back(batch(&["b"]));
        assert_eq!(queued_rows(&app, 40), 3);
    }

    #[test]
    fn queued_lines_separate_batches_with_a_blank_row() {
        // Tab-opened batches read as separate turns: a blank row sits between
        // each batch, grouping the follow-ups apart from the first queue.
        let mut app = App::new();
        app.queued.push_back(batch(&["first"]));
        app.queued.push_back(batch(&["later"]));
        let lines = queued_lines(&app, 40);
        assert_eq!(lines.len(), 3, "two single-message batches + one separator");
        assert!(
            plain(&lines[0]).contains("❯ first"),
            "{:?}",
            plain(&lines[0])
        );
        assert_eq!(
            plain(&lines[1]).trim(),
            "",
            "a blank separator row divides the batches"
        );
        assert!(
            plain(&lines[2]).contains("❯ later"),
            "{:?}",
            plain(&lines[2])
        );
    }

    #[test]
    fn queued_lines_render_a_shell_entry_with_the_red_bang_prompt() {
        // A queued !command renders like the exec cell it becomes: the red `! `
        // Role::Shell header (not the ❯ user bullet), inset two columns.
        let mut app = App::new();
        app.queued.push_back(QueuedTurn::Shell("ls -la".into()));
        let lines = queued_lines(&app, 40);
        let expected = message_lines(Role::Shell, "ls -la", 38); // 40 minus the indent
        assert_eq!(lines.len(), expected.len());
        assert!(
            plain(&lines[0]).contains("! ls -la"),
            "the red shell prompt, not ❯: {:?}",
            plain(&lines[0])
        );
        assert_eq!(
            lines[0].spans[0].content.as_ref(),
            "  ",
            "inset two columns, the indent outside the dark block"
        );
    }

    #[test]
    fn queued_lines_divide_a_text_batch_and_a_shell_entry() {
        // A text batch and a shell entry are separate turns: a blank row divides
        // them, the text keeping its ❯ bullet and the command its red `! `.
        let mut app = App::new();
        app.queued.push_back(batch(&["hello"]));
        app.queued.push_back(QueuedTurn::Shell("ls".into()));
        let lines = queued_lines(&app, 40);
        assert_eq!(lines.len(), 3, "message + blank divider + shell");
        assert!(
            plain(&lines[0]).contains("❯ hello"),
            "{:?}",
            plain(&lines[0])
        );
        assert_eq!(plain(&lines[1]).trim(), "", "a blank divider");
        assert!(plain(&lines[2]).contains("! ls"), "{:?}", plain(&lines[2]));
    }

    #[test]
    fn queued_rows_count_wrapped_lines() {
        // A queued message wraps like a user message, so a long one is >1 row.
        let mut app = App::new();
        app.queued
            .push_back(batch(&["one two three four five six seven eight"]));
        assert!(queued_rows(&app, 16) >= 2, "a long queued message wraps");
    }

    #[test]
    fn queued_rows_are_uncapped_every_message_counts() {
        // No display cap (codex shows the whole backlog): ten queued messages
        // are ten rows.
        let mut app = App::new();
        app.queued.push_back(QueuedTurn::Messages {
            texts: (0..10).map(|i| format!("m{i}")).collect(),
            images: Vec::new(),
        });
        assert_eq!(queued_rows(&app, 40), 10);
    }

    #[test]
    fn queued_lines_indent_two_columns_and_keep_the_user_style() {
        // The queue is inset two columns from the strip's left edge; past the
        // indent each message is exactly a user message (❯ bullet, dark
        // background) wrapped to the remaining width — and the indent itself
        // stays *outside* the dark block.
        let mut app = App::new();
        app.queued.push_back(batch(&["world"]));
        let lines = queued_lines(&app, 40);
        let expected = message_lines(Role::User, "world", 38); // 40 minus the indent
        assert_eq!(lines.len(), expected.len());
        assert_eq!(plain(&lines[0]), format!("  {}", plain(&expected[0])));
        let indent = &lines[0].spans[0];
        assert_eq!(indent.content.as_ref(), "  ");
        assert_eq!(
            indent.style.bg, None,
            "the indent sits outside the dark block"
        );
        assert!(
            lines[0].spans[1..].iter().all(|s| s.style.bg.is_some()),
            "past the indent the user-message background holds: {lines:?}"
        );
    }

    #[test]
    fn queued_lines_list_every_message_with_the_user_bullet() {
        let mut app = App::new();
        app.queued.push_back(batch(&["world", "again"]));
        let all: String = queued_lines(&app, 40)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("  ❯ world"), "{all:?}");
        assert!(all.contains("  ❯ again"), "{all:?}");
    }

    #[test]
    fn queued_lines_wrap_a_long_message_across_rows() {
        let mut app = App::new();
        app.queued
            .push_back(batch(&["alpha beta gamma delta epsilon"]));
        let lines = queued_lines(&app, 18);
        assert!(lines.len() >= 2, "a long queued message wraps: {lines:?}");
        assert!(
            lines.iter().all(|l| plain(l).starts_with("  ")),
            "wrapped continuation rows carry the indent too: {lines:?}"
        );
    }

    #[test]
    fn queued_lines_list_the_whole_backlog_uncapped() {
        let mut app = App::new();
        app.queued.push_back(QueuedTurn::Messages {
            texts: (0..10).map(|i| format!("m{i}")).collect(),
            images: Vec::new(),
        });
        let lines = queued_lines(&app, 40);
        assert_eq!(lines.len(), 10, "every queued message shows");
        assert!(plain(&lines[9]).contains("❯ m9"), "{:?}", plain(&lines[9]));
    }

    #[test]
    fn live_height_grows_with_the_queue() {
        let mut app = App::new();
        app.begin_stream();
        let without = live_height(&app.input, 40, 24, true, true, 0, 0, 0);
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let with = live_height(&app.input, 40, 24, true, true, q, 0, 0);
        assert_eq!(with, without + q, "the queue grows the region by its rows");
        assert_eq!(q, 1, "one short queued message is one row");
    }

    #[test]
    fn render_live_draws_the_queue_above_the_box_as_a_user_message() {
        let mut app = App::new();
        app.begin_stream();
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let h = live_height(&app.input, 40, 24, true, true, q, 0, 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 40)).collect();
        // The queued "❯ world" sits *above* the box's (first) top rule, not below.
        let rule = rows
            .iter()
            .position(|r| r.contains('─'))
            .expect("a box rule");
        let world = rows
            .iter()
            .position(|r| r.contains("❯ world"))
            .expect("the queued message");
        assert!(
            world < rule,
            "the queued message is above the box: {rows:?}"
        );
        assert!(
            rows[world].starts_with("  ❯"),
            "the queued row is inset two columns: {:?}",
            rows[world]
        );
    }

    #[test]
    fn the_queue_and_the_shortcuts_band_show_in_their_own_slots() {
        // The queue is in the strip (above the box); the shortcuts band is below
        // it — independent slots, both visible at once.
        let mut app = App::new();
        app.begin_stream();
        app.shortcuts_open = true;
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let h = live_height(&app.input, 40, 24, true, true, q, shortcuts_rows(&app), 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 40)).collect();
        let rule = rows
            .iter()
            .position(|r| r.contains('─'))
            .expect("a box rule");
        let world = rows
            .iter()
            .position(|r| r.contains("❯ world"))
            .expect("the queued message");
        let cmds = rows
            .iter()
            .position(|r| r.contains("for commands"))
            .expect("the shortcuts band");
        assert!(world < rule, "queue above the box: {rows:?}");
        assert!(cmds > rule, "shortcuts below the box: {rows:?}");
    }

    // --- the session-context footer under the box (docs/footer.md) ---

    /// An app with session info injected, as `main.rs` does at startup.
    fn with_session() -> App {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/inline-tui");
        app
    }

    #[test]
    fn footer_rows_is_zero_without_session_info_and_one_with_it() {
        assert_eq!(footer_rows(&App::new(), 0), 0, "no session info → no row");
        assert_eq!(footer_rows(&with_session(), 0), 1);
    }

    #[test]
    fn footer_rows_yields_to_an_open_band() {
        // codex's popups / shortcut overlay take the footer's place; our
        // palette and `?` shortcuts band displace it the same way.
        assert_eq!(footer_rows(&with_session(), 3), 0);
    }

    #[test]
    fn footer_line_shows_model_and_cwd_dim_behind_the_indent() {
        let line = footer_line(&with_session(), 60);
        assert_eq!(plain(&line), "  dummy_model_name · ~/inline-tui");
        // spans = [indent, model, separator, cwd] — every segment dim (codex's
        // no-theme-colours status line), the indent unstyled.
        assert_eq!(line.spans[0].style.fg, None);
        for span in &line.spans[1..] {
            assert_eq!(span.style.fg, Some(FOOTER_COLOR), "dim: {:?}", span.content);
        }
    }

    #[test]
    fn footer_line_truncates_with_an_ellipsis_when_narrow() {
        let line = footer_line(&with_session(), 20);
        let text = plain(&line);
        assert!(cols(&text) <= 20, "fits the width: {text:?}");
        assert!(text.ends_with('…'), "cut is visible: {text:?}");
        assert!(text.starts_with("  dummy_model"), "head kept: {text:?}");
    }

    #[test]
    fn display_cwd_relativizes_home_to_a_tilde() {
        let home = Path::new("/home/user");
        assert_eq!(display_cwd(home, Some(home)), "~");
        assert_eq!(
            display_cwd(Path::new("/home/user/code/tui"), Some(home)),
            "~/code/tui"
        );
    }

    #[test]
    fn display_cwd_outside_home_or_without_one_stays_absolute() {
        let home = Path::new("/home/user");
        assert_eq!(
            display_cwd(Path::new("/etc/nginx"), Some(home)),
            "/etc/nginx"
        );
        assert_eq!(display_cwd(Path::new("/srv/app"), None), "/srv/app");
        assert_eq!(
            display_cwd(Path::new("/home/username"), Some(home)),
            "/home/username",
            "component-wise, not a string prefix"
        );
    }

    #[test]
    fn live_height_adds_the_footer_row() {
        let ta = TextArea::from_text("hi");
        assert_eq!(
            live_height(&ta, 40, 24, false, false, 0, 0, 1),
            live_height(&ta, 40, 24, false, false, 0, 0, 0) + 1,
            "the footer adds its row at the very bottom"
        );
    }

    #[test]
    fn render_live_paints_the_footer_on_the_last_row() {
        let app = with_session();
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(
            last.contains("dummy_model_name · ~/inline-tui"),
            "footer under the box: {last:?}"
        );
        assert!(last.starts_with("  dummy"), "two-column inset: {last:?}");
        assert_eq!(buf[(0, h - 2)].symbol(), "─", "bottom rule right above it");
    }

    #[test]
    fn the_footer_stays_while_a_turn_streams() {
        let mut app = with_session();
        app.begin_stream();
        app.push_chunk("hello");
        let h = live_height(&app.input, 60, 24, true, true, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert!(
            row(&buf, h - 1, 60).contains("dummy_model_name"),
            "the footer is ambient — present mid-turn too"
        );
    }

    #[test]
    fn the_palette_replaces_the_footer() {
        let mut app = palette("/", 0);
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let band = menu_rows(&app);
        let h = live_height(
            &app.input,
            60,
            24,
            false,
            false,
            0,
            band,
            footer_rows(&app, band),
        );
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 60))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("/help"), "palette shown: {all:?}");
        assert!(
            !all.contains("dummy_model_name"),
            "the footer yields its slot: {all:?}"
        );
    }

    #[test]
    fn the_cursor_stays_put_when_the_footer_shows() {
        // The footer is reserved *below* the box, so injecting session info
        // must not move the cursor.
        let mut app = App::new();
        app.input = TextArea::from_text("hi");
        let bare_h = live_height(&app.input, 40, 24, false, false, 0, 0, 0);
        let bare = cursor_position(Rect::new(0, 0, 40, bare_h), &app);
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let footer_h = live_height(&app.input, 40, 24, false, false, 0, 0, 1);
        let with_footer = cursor_position(Rect::new(0, 0, 40, footer_h), &app);
        assert_eq!(with_footer, bare, "cursor unchanged by the footer row");
    }

    // --- terminal-size sweep ---

    /// Mixed-width stress text: prose, wide CJK, emoji, and an unbreakable
    /// over-long token, so the sweep hits every wrap branch.
    const SWEEP_TEXT: &str = "The quick brown fox 世界你好 mixes wide CJK with \
        emoji 🎉🎊 and averyveryverylongunbreakabletokenthatmusthardbreak too.";

    /// One busy `App` per live-region feature, so the sweep exercises every
    /// width-dependent render path on top of a shared finished history
    /// (messages, a tool call, a summary, the session footer).
    fn size_sweep_apps() -> Vec<(&'static str, App)> {
        let base = || {
            let mut app = App::new();
            app.set_session_info("dummy_model_name", "~/repo/some/longish/path");
            app.record_user_message("first message with CJK 世界 and emoji 🎉");
            app.record_system_message("help text\nwith a second line");
            app.begin_stream();
            app.push_chunk("text before the tool call. ");
            app.start_tool("read_file", "src/app.rs with a long argument string");
            app.end_tool(SWEEP_TEXT, true);
            app.push_chunk(SWEEP_TEXT);
            app.finish_stream();
            app.end_turn(3);
            app
        };
        let streaming = || {
            let mut app = base();
            app.begin_stream();
            app.push_chunk(SWEEP_TEXT);
            app.set_status_times(Duration::from_secs(7), None);
            app
        };
        let tool = {
            let mut app = base();
            app.begin_stream();
            app.push_chunk("before tool ");
            app.start_tool("write_file", SWEEP_TEXT);
            app.set_status_times(Duration::from_secs(7), Some(Duration::from_secs(2)));
            app
        };
        let queued = {
            let mut app = streaming();
            app.queued.push_back(batch(&["queued one with CJK 世界"]));
            app
        };
        let menu = {
            let mut app = base();
            app.input = TextArea::from_text("/");
            app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
            app
        };
        let shortcuts = {
            let mut app = base();
            app.shortcuts_open = true;
            app
        };
        let draft = {
            let mut app = base();
            app.input = TextArea::from_text(&format!("{SWEEP_TEXT}\n{SWEEP_TEXT}"));
            app
        };
        let file_picker = {
            // An open `@` picker with match indices deep enough in a long
            // mixed-width path that tiny widths truncate past them, so
            // `file_menu_row`'s truncate + match-span grouping is swept too.
            let long = "src/some/deeply/nested/世界 with spaces/🎉emoji/averylongfilename.rs";
            let mut app = base();
            app.input = TextArea::from_text("@src");
            app.file_search = Some(FileSearch {
                selected: 0,
                query: "src".into(),
                matches: vec![
                    FileMatch {
                        path: "src/app.rs".into(),
                        score: 10,
                        indices: vec![0, 1, 2],
                    },
                    FileMatch {
                        path: long.into(),
                        // Byte offsets of `s`, `r`, `c`, `世`, `界`, `🎉`, `a`,
                        // and `l` — the last five land past a narrow truncation.
                        score: 5,
                        indices: vec![0, 1, 2, 23, 26, 42, 52, 57],
                    },
                ],
                waiting: false,
            });
            app
        };
        vec![
            ("idle", base()),
            ("streaming", streaming()),
            ("tool", tool),
            ("queued", queued),
            ("menu", menu),
            ("shortcuts", shortcuts),
            ("draft", draft),
            ("file_picker", file_picker),
        ]
    }

    #[test]
    fn render_pipeline_survives_extreme_terminal_sizes() {
        // codex clamps every wrap width (`.max(1)` and friends) so narrow or
        // short terminals degrade gracefully instead of panicking; this locks
        // the same property over our whole pure pipeline — the live region,
        // the cursor, the scrollback commits, the resize repaint tail, and the
        // Ctrl+O overlay — at every awkward size, wide CJK/emoji included.
        // (The terminal half of a real resize is smoke-covered: Phase 17.)
        let widths = [0u16, 1, 2, 3, 4, 5, 8, 13, 34, 80, 120];
        let heights = [1u16, 2, 3, 4, 5, 8, 12, 24, 48];
        for (state, app) in &size_sweep_apps() {
            for &w in &widths {
                for &h in &heights {
                    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        // mirror main.rs::draw
                        let band = menu_rows(app) + shortcuts_rows(app) + file_menu_rows(app);
                        let lh = live_height(
                            &app.input,
                            w,
                            h,
                            app.is_streaming(),
                            strip_has_preview(app),
                            queued_rows(app, w),
                            band,
                            footer_rows(app, band),
                        );
                        let area = Rect::new(0, 0, w, lh.clamp(1, h.max(1)));
                        let mut buf = Buffer::empty(area);
                        render_live(area, &mut buf, app);
                        let _ = cursor_position(area, app);
                        // mirror main.rs::repaint_conversation
                        let budget = repaint_budget(h, area.height);
                        let _ = repaint_lines(&app.history, w, budget);
                        // mirror the streaming commit path
                        if let Some(text) = app.streaming_text() {
                            let (_, c) = stable_commit(text, w, 0);
                            let _ = final_commit(text, w, c);
                        }
                        // mirror the Ctrl+O overlay
                        let screen = Rect::new(0, 0, w, h);
                        let mut overlay = Buffer::empty(screen);
                        render_tool_view(screen, &mut overlay, app);
                        let _ = tool_view_max_scroll(app, w, h);
                    }));
                    assert!(
                        run.is_ok(),
                        "render pipeline panicked: state={state} width={w} height={h}"
                    );
                }
            }
        }
    }

    // --- the Ctrl+R search line in the footer slot (docs/history-search.md) ---

    /// An app with `history` recorded and a Ctrl+R search open with `query`
    /// typed, driven through the real key path.
    fn searching(history: &[&str], query: &str) -> App {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new();
        for text in history {
            app.input_history.record(text);
        }
        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in query.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app
    }

    #[test]
    fn search_rows_take_the_footer_slot_even_without_session_info() {
        let app = searching(&[], "");
        assert_eq!(
            footer_rows(&app, 0),
            1,
            "the search line must show even when no session info is injected"
        );
    }

    #[test]
    fn the_search_line_displaces_the_session_footer() {
        let mut app = searching(&["git status"], "git");
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(
            last.starts_with("  reverse-i-search: git"),
            "the query line sits in the footer slot: {last:?}"
        );
        assert!(
            !last.contains("dummy_model_name"),
            "the session footer is displaced: {last:?}"
        );
    }

    #[test]
    fn the_search_line_shows_accept_hints_on_a_match() {
        let line = search_line(
            searching(&["git status"], "git")
                .history_search
                .as_ref()
                .unwrap(),
        );
        let text = plain(&line);
        assert_eq!(text, "  reverse-i-search: git  enter accept · esc cancel");
        // The query is cyan and the hint keys are cyan+bold, the rest dim
        // (codex's history_search_footer_line styling).
        let query = &line.spans[2];
        assert_eq!(query.content.as_ref(), "git");
        assert_eq!(query.style.fg, Some(SEARCH_QUERY_COLOR));
        let enter = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "enter")
            .expect("enter key span");
        assert_eq!(enter.style.fg, Some(SEARCH_QUERY_COLOR));
        assert!(enter.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn the_search_line_shows_no_match_in_red() {
        let line = search_line(
            searching(&["git status"], "zzz")
                .history_search
                .as_ref()
                .unwrap(),
        );
        assert_eq!(plain(&line), "  reverse-i-search: zzz  no match");
        let no_match = line.spans.last().unwrap();
        assert_eq!(no_match.style.fg, Some(ERROR_COLOR));
    }

    #[test]
    fn the_search_line_is_bare_while_idle() {
        let line = search_line(
            searching(&["git status"], "")
                .history_search
                .as_ref()
                .unwrap(),
        );
        assert_eq!(plain(&line), "  reverse-i-search: ");
    }

    #[test]
    fn the_cursor_sits_at_the_end_of_the_query_in_the_search_line() {
        let app = searching(&["git status"], "git");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let area = Rect::new(0, 0, 60, h);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, h - 1, "on the footer row, not in the textarea");
        let expected = cols(FOOTER_INDENT) + cols(SEARCH_PROMPT) + cols("git");
        assert_eq!(x as usize, expected);
    }

    #[test]
    fn the_search_cursor_clamps_inside_a_narrow_terminal() {
        let app = searching(&["git status"], "a very very long query indeed");
        let h = live_height(&app.input, 20, 24, false, false, 0, 0, 1);
        let area = Rect::new(0, 0, 20, h);
        let (x, _) = cursor_position(area, &app);
        assert!(x < 20, "clamped inside the width (codex clamps the same)");
    }

    #[test]
    fn the_previewed_match_highlights_the_query_reversed() {
        let app = searching(&["git status"], "stat");
        assert_eq!(app.input.text(), "git status", "the match previews");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        // The input row is "❯ git status" on the row inside the box frame:
        // "stat" starts at column 2 (prompt) + 4 ("git ").
        let y = 1;
        assert_eq!(row(&buf, y, 60).trim_end(), "❯ git status");
        for x in 6..10 {
            assert!(
                buf[(x, y)].modifier.contains(Modifier::REVERSED),
                "match cols reversed at x={x}"
            );
        }
        assert!(
            !buf[(2, y)].modifier.contains(Modifier::REVERSED),
            "outside the match stays plain"
        );
        assert!(
            !buf[(10, y)].modifier.contains(Modifier::REVERSED),
            "the highlight ends with the match"
        );
    }

    #[test]
    fn the_shortcuts_band_lists_ctrl_r() {
        let texts: Vec<String> = shortcuts_lines(false).iter().map(plain).collect();
        assert!(
            texts.iter().any(|t| t.contains("ctrl+r to search history")),
            "band lists the search binding: {texts:?}"
        );
    }

    // --- the `!` shell mode + its exec cell (docs/shell-command.md) ---

    /// An app in shell mode with `command` typed (the bang absorbed into the
    /// mode flag, codex-style — the textarea holds just the command).
    fn shelling(command: &str) -> App {
        let mut app = App::new();
        app.shell_mode = true;
        app.input = TextArea::from_text(command);
        app
    }

    #[test]
    fn shell_mode_takes_the_footer_slot_even_without_session_info() {
        assert_eq!(
            footer_rows(&shelling("ls"), 0),
            1,
            "the Shell mode hint shows even with no session info"
        );
        assert_eq!(footer_rows(&App::new(), 0), 0, "not in shell mode");
    }

    #[test]
    fn the_shell_mode_line_displaces_the_session_footer() {
        let mut app = shelling("ls -la");
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(last.starts_with("  Shell mode"), "the hint shows: {last:?}");
        assert!(
            !last.contains("dummy_model_name"),
            "the session footer is displaced: {last:?}"
        );
    }

    #[test]
    fn the_shell_mode_line_is_red() {
        let line = shell_mode_line();
        assert_eq!(plain(&line), "  Shell mode");
        let label = line.spans.last().unwrap();
        assert_eq!(label.content.as_ref(), "Shell mode");
        assert_eq!(label.style.fg, Some(SHELL_MODE_COLOR));
    }

    #[test]
    fn shell_mode_swaps_the_composer_prompt_for_a_red_bang() {
        // The absorbed `!` renders back as the prompt: `! pwd`, not `❯ pwd`.
        let app = shelling("pwd");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert_eq!(row(&buf, 1, 60).trim_end(), "! pwd");
        assert_eq!(
            buf[(0, 1)].fg,
            SHELL_MODE_COLOR,
            "the bang prompt is red, the shell accent"
        );
    }

    #[test]
    fn the_cursor_stays_in_the_box_in_shell_mode() {
        // Unlike the Ctrl+R search (which owns the footer cursor), shell mode
        // keeps the cursor on the composer's command line.
        let app = shelling("ls");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 1);
        let area = Rect::new(0, 0, 60, h);
        let (_, y) = cursor_position(area, &app);
        assert!(y < h - 1, "cursor is in the box, not on the footer row");
    }

    #[test]
    fn the_shortcuts_band_lists_the_bang() {
        let texts: Vec<String> = shortcuts_lines(false).iter().map(plain).collect();
        assert!(
            texts.iter().any(|t| t.contains("! for shell command")),
            "band lists the shell binding: {texts:?}"
        );
    }

    #[test]
    fn a_shell_header_message_renders_like_a_user_line_with_a_bang() {
        let lines = message_lines(Role::Shell, "pwd", 40);
        assert_eq!(plain(&lines[0]).trim_end(), "! pwd");
        // The bang is the shell accent; the row carries the dark user block,
        // padded to the full content width (the mock's "dark line wrap").
        assert_eq!(lines[0].spans[0].style.fg, Some(SHELL_MODE_COLOR));
        assert_eq!(lines[0].style.bg, Some(USER_BG_COLOR));
        assert_eq!(cols(&plain(&lines[0])), 40, "padded to the full width");
    }

    #[test]
    fn a_shell_tool_renders_headerless_output_only() {
        // The Role::Shell header message is the cell's first line; the tool
        // itself contributes only the `⎿` output lines, flush below it.
        let mut t = tool("pwd", "", ToolStatus::Ok, "/home/user/inline-tui");
        t.shell = true;
        let lines = tool_lines(&t, 60);
        assert_eq!(plain(&lines[0]), "  ⎿ /home/user/inline-tui");
        assert!(
            !plain(&lines[0]).contains("pwd"),
            "no `● pwd` header — the Shell message above is the header"
        );
    }

    #[test]
    fn a_running_shell_tool_peeks_running() {
        let mut t = tool("sleep 5", "", ToolStatus::Running, "");
        t.shell = true;
        let lines = tool_lines(&t, 60);
        assert_eq!(plain(&lines[0]), "  ⎿ Running…", "the mock's running cell");
    }

    #[test]
    fn a_short_shell_output_shows_every_line_aligned_under_the_corner() {
        // Up to TOOL_PEEK_LINES lines all show, the continuation lines indented
        // to align under the first (Claude-Code exec style), no truncation hint.
        let mut t = tool(
            "ls",
            "",
            ToolStatus::Ok,
            "index.html\nscript.js\nstyles.css",
        );
        t.shell = true;
        let lines: Vec<String> = tool_lines(&t, 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(
            lines,
            vec!["  ⎿ index.html", "    script.js", "    styles.css"],
            "every line shown, continuation aligned under the first"
        );
    }

    #[test]
    fn a_long_shell_output_caps_the_preview_with_an_expand_hint() {
        // More than the cap → the first TOOL_PEEK_LINES lines, then a
        // `… +N lines (ctrl+o to expand)` row aligned with them.
        let output = (1..=6)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut t = tool("seq 6", "", ToolStatus::Ok, &output);
        t.shell = true;
        let lines: Vec<String> = tool_lines(&t, 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(
            lines.len(),
            TOOL_PEEK_LINES + 1,
            "capped lines + the hint row"
        );
        assert_eq!(lines[0], "  ⎿ 1");
        assert_eq!(lines[1], "    2", "continuation aligned, no corner");
        let hidden = 6 - TOOL_PEEK_LINES;
        assert_eq!(
            lines[TOOL_PEEK_LINES],
            format!("    … +{hidden} lines (ctrl+o to expand)")
        );
    }

    #[test]
    fn a_truncated_shell_output_appends_an_ellipsis_marker_in_the_full_view() {
        // Over-cap output is cut to its head; the expanded (Ctrl+O) view appends
        // a dim `…` line after the last retained line so the user sees more was
        // dropped (it is not recoverable — nothing to expand to).
        let mut t = tool("tree ~/", "", ToolStatus::Ok, "/home/me\n├── a\n├── b");
        t.shell = true;
        t.truncated = true;
        let lines: Vec<String> = tool_full_lines(&t, 70)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(lines[0], "  ⎿ /home/me");
        assert_eq!(lines[1], "    ├── a");
        assert_eq!(lines[2], "    ├── b");
        assert_eq!(
            lines[3], "    …",
            "the truncation marker, aligned under the corner: {lines:?}"
        );
    }

    #[test]
    fn a_complete_shell_output_has_no_truncation_marker() {
        // When the output was kept in full, no `…` marker is appended.
        let mut t = tool("ls", "", ToolStatus::Ok, "a\nb");
        t.shell = true;
        let lines: Vec<String> = tool_full_lines(&t, 70)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(
            lines,
            vec!["  ⎿ a", "    b"],
            "no trailing marker: {lines:?}"
        );
    }

    #[test]
    fn the_tool_view_renders_a_shell_cell_headerless_with_full_output() {
        // In the Ctrl+O transcript the shell cell shows its `! ls` dark header
        // (the Role::Shell message) and the FULL output under `⎿` — no `● ls`
        // bullet, and uncapped (this is the expand view).
        let mut app = App::new();
        let output = ('a'..='f')
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut t = tool("ls", "", ToolStatus::Ok, &output);
        t.shell = true;
        app.history = vec![
            HistoryItem::Message(Message {
                role: Role::Shell,
                text: "ls".to_string(),
                timestamp: String::new(),
            }),
            HistoryItem::Tool(t),
        ];
        let texts: Vec<String> = transcript_lines(&app, 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(
            texts.contains(&"! ls".to_string()),
            "dark header: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.starts_with("● ls")),
            "no bullet header in the overlay: {texts:?}"
        );
        assert!(texts.contains(&"  ⎿ a".to_string()), "{texts:?}");
        assert!(
            texts.contains(&"    f".to_string()),
            "every output line, uncapped: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("ctrl+o to expand")),
            "no truncation hint in the expand view: {texts:?}"
        );
    }

    #[test]
    fn conversation_lines_keep_the_shell_cell_flush() {
        // No blank spacer between the `! pwd` header and its `⎿` output —
        // they form one cell (and the running preview sits flush the same way).
        let mut t = tool("pwd", "", ToolStatus::Ok, "/home");
        t.shell = true;
        let history = vec![
            HistoryItem::Message(Message {
                role: Role::Shell,
                text: "pwd".to_string(),
                timestamp: String::new(),
            }),
            HistoryItem::Tool(t),
        ];
        let texts: Vec<String> = conversation_lines(&history, 40)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts, vec!["! pwd", "  ⎿ /home", ""]);
    }

    #[test]
    fn transcript_lines_keep_the_shell_cell_flush() {
        // The Ctrl+O transcript renders the same shell cell as the inline view
        // (docs/shell-command.md): no blank spacer between the `! pwd` header
        // and its `⎿` output either.
        let mut t = tool("pwd", "", ToolStatus::Ok, "/home");
        t.shell = true;
        let mut app = App::new();
        app.history = vec![
            HistoryItem::Message(Message {
                role: Role::Shell,
                text: "pwd".to_string(),
                timestamp: String::new(),
            }),
            HistoryItem::Tool(t),
        ];
        let texts: Vec<String> = transcript_lines(&app, 40)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts, vec!["! pwd", "  ⎿ /home", ""]);
    }

    #[test]
    fn the_running_shell_transcript_sits_flush_under_its_header() {
        // The live tail too: while a `!` command runs, its committed header is
        // the last history item and the running tool renders flush below it.
        let mut app = App::new();
        app.begin_shell("sleep 5");
        let texts: Vec<String> = transcript_lines(&app, 40)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        let header = texts
            .iter()
            .position(|t| t == "! sleep 5")
            .expect("the shell header is in the transcript");
        assert_eq!(
            texts[header + 1],
            "  ⎿ Running…",
            "no blank between the header and the running peek: {texts:?}"
        );
    }

    #[test]
    fn a_backend_tool_keeps_its_transcript_spacing() {
        // Only the shell cell is flush — a backend tool still gets the blank
        // spacer after the preceding message, as today.
        let app = transcript_fixture();
        let texts: Vec<String> = transcript_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        let msg = texts.iter().position(|t| t == "● let me check").unwrap();
        assert_eq!(texts[msg + 1], "", "blank spacer after the message");
        assert_eq!(texts[msg + 2], "● Read(f)", "then the tool header");
    }

    #[test]
    fn the_running_shell_preview_is_the_flush_running_peek() {
        let mut app = App::new();
        app.begin_shell("sleep 5");
        let q = queued_rows(&app, 60);
        let h = live_height(&app.input, 60, 24, true, true, q, 0, footer_rows(&app, 0));
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert_eq!(
            row(&buf, 0, 60).trim_end(),
            "  ⎿ Running…",
            "the strip preview is the cell's running peek, flush under the \
             committed `! sleep 5` header just above the live region"
        );
    }

    // --- the `@` file picker (docs/file-search.md) ---

    /// A bare file match for the picker render tests.
    fn fmatch(path: &str) -> FileMatch {
        FileMatch {
            path: path.to_string(),
            score: 0,
            indices: Vec::new(),
        }
    }

    /// An app with an open file picker over `matches` (input is the `@query`).
    fn file_picker(query: &str, matches: Vec<FileMatch>, selected: usize) -> App {
        let mut app = App::new();
        app.input = TextArea::from_text(&format!("@{query}"));
        app.file_search = Some(FileSearch {
            selected,
            query: query.to_string(),
            matches,
            waiting: false,
        });
        app
    }

    #[test]
    fn file_menu_rows_counts_closed_placeholder_and_capped_matches() {
        assert_eq!(file_menu_rows(&App::new()), 0, "closed");
        assert_eq!(
            file_menu_rows(&file_picker("a", vec![], 0)),
            1,
            "one placeholder row while empty"
        );
        let many: Vec<_> = (0..12).map(|i| fmatch(&format!("d/f{i}.rs"))).collect();
        assert_eq!(
            file_menu_rows(&file_picker("f", many, 0)),
            FILE_MENU_MAX_ROWS,
            "capped"
        );
    }

    #[test]
    fn file_menu_lines_placeholder_is_searching_then_no_match() {
        let mut app = file_picker("a", vec![], 0);
        app.file_search.as_mut().unwrap().waiting = true;
        assert!(plain(&file_menu_lines(&app, 40)[0]).contains("Searching"));
        app.file_search.as_mut().unwrap().waiting = false;
        assert!(plain(&file_menu_lines(&app, 40)[0]).contains("No matching files"));
    }

    #[test]
    fn file_menu_lines_lists_paths_with_the_selection_highlighted() {
        let app = file_picker("ma", vec![fmatch("src/main.rs"), fmatch("READ.md")], 1);
        let lines = file_menu_lines(&app, 40);
        assert_eq!(lines.len(), 2);
        assert!(plain(&lines[0]).contains("src/main.rs"));
        assert!(plain(&lines[1]).contains("READ.md"));
        // The selected row (index 1) is rendered in the cyan selection colour;
        // the unselected row is not.
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(MENU_SELECTED_COLOR))
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|s| s.style.fg != Some(MENU_SELECTED_COLOR))
        );
    }

    #[test]
    fn file_menu_bolds_the_matched_characters() {
        // The query matched "ma" → bytes 0 and 1 of "main.rs".
        let m = FileMatch {
            path: "main.rs".to_string(),
            score: 1,
            indices: vec![0, 1],
        };
        let app = file_picker("ma", vec![m], 0);
        let line = &file_menu_lines(&app, 40)[0];
        assert!(
            line.spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "the matched characters are bolded: {line:?}"
        );
    }

    #[test]
    fn render_live_draws_the_file_picker_below_the_box() {
        let app = file_picker("ma", vec![fmatch("src/main.rs")], 0);
        let band = file_menu_rows(&app);
        let h = live_height(&app.input, 40, 24, false, false, 0, band, 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains("src/main.rs"),
            "the file picker rendered below the box: {all:?}"
        );
    }

    #[test]
    fn cursor_stays_in_the_box_when_the_file_picker_opens() {
        // The picker is reserved *below* the box, so opening it must not move the
        // cursor (mirrors the palette).
        let area = Rect::new(0, 0, 40, 24);
        let mut closed = App::new();
        closed.input = TextArea::from_text("@m");
        let before = cursor_position(area, &closed);
        let open = file_picker("m", vec![fmatch("main.rs")], 0);
        let after = cursor_position(area, &open);
        assert_eq!(after, before, "cursor unchanged when the picker opens");
    }
}

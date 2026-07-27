//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`live_height`], [`repin`], [`cursor_position`]),
//! build ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, BackgroundShell, BackgroundView, HistoryItem, HistorySearch, KeyOnboarding, KeyStep,
    ModelLoad, ModelPicker, ProviderChoice, QueuedTurn, ResumeControl, ResumeFilter, ResumePicker,
    ResumeSort, Role, SearchState, SlashCommand, ToastKind, TokenArrow, ToolCall, ToolStatus,
    TurnStatus, TurnSummary, command_query, matching_commands,
};
use crate::file_search::FileMatch;
use crate::highlight;
use crate::llm::ModelEntry;
use crate::markdown;
use crate::textarea::TextArea;

/// Display width of `s` in terminal columns.
///
/// All width math in this module goes through this instead of `chars().count()`:
/// CJK and many emoji are two columns wide and combining marks are zero, so a
/// raw char count would wrap and pad non-ASCII text incorrectly.
fn cols(s: &str) -> usize {
    s.width()
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

// --- Assistant markdown rendering (fenced code blocks + ATX headings;
// `docs/markdown.md`). Code sits under the bullet (no gutter, no language
// label), rendered VERBATIM (indentation preserved, no word-wrap) — the fix
// for code losing its indentation — and **syntax-highlighted** by the `highlight`
// module (syntect + two_face grammars, Catppuccin Mocha theme — codex parity;
// the theme owns the code palette now, so there are no `CODE_*_COLOR` consts).
// Headings keep their `#` markers and style the line per level, matching codex
// (`heading_style`). ---
/// A tab inside a code block expands to this many spaces **for display**. A tab
/// is zero display columns (unicode-width treats it as a control char), so code
/// rendered verbatim would lose all its tab indentation (Go, Makefiles, …
/// collapse flush-left). A fixed substitution — not tab-stop alignment — keeps
/// it simple and prefix-stable, matching codex's `expand_tabs`. Copy is
/// unaffected: `/copy` reads the raw message text, not the rendered rows.
const CODE_TAB_WIDTH: usize = 4;
/// The style an ATX heading of `level` (1–6) renders with — a faithful port of
/// codex's `markdown_render.rs::start_heading`: the `#` markers are **kept**
/// (see [`AssistantRenderer::content_rows`]) and the whole line carries text
/// *modifiers only*, no foreground colour, so a heading reads like codex's:
/// h1 bold+underlined, h2 bold, h3 bold+italic, h4–h6 italic. See
/// `docs/markdown.md`.
fn heading_style(level: u8) -> Style {
    let modifier = match level {
        1 => Modifier::BOLD | Modifier::UNDERLINED,
        2 => Modifier::BOLD,
        3 => Modifier::BOLD | Modifier::ITALIC,
        _ => Modifier::ITALIC, // h4–h6 (and any deeper level clamps here)
    };
    Style::new().add_modifier(modifier)
}

/// A markdown thematic break (`---` / `***` / `___`) renders as this em-dash rule,
/// a direct port of codex's `Event::Rule` (`Line::from("———")` — three U+2014 EM
/// DASH, unstyled/default foreground). See `docs/markdown.md`.
const THEMATIC_BREAK: &str = "———";

/// Whether `line` should render as a [`THEMATIC_BREAK`]. A `-` rule is
/// indistinguishable from a **setext `H2` underline** (which we can't render —
/// it needs lookahead that would break streaming prefix-stability), so a `-` rule
/// is only honoured when the previous line was blank (`prev_blank`); `*`/`_` runs
/// are unambiguous and always render. Keeps a `text\n---` pair as literal prose
/// rather than fabricating a rule where codex would show a heading.
fn is_thematic_break(line: &str, prev_blank: bool) -> bool {
    match markdown::thematic_break(line) {
        Some('-') => prev_blank,
        Some(_) => true,
        None => false,
    }
}

// --- Markdown table styling (docs/markdown.md, docs/table-streaming.md). A GFM
// pipe table renders as a box-drawing grid: dim borders, bold header cells.
// Column widths are locked from the header + first data row (fit to the width),
// so once the first row streams the block is prefix-stable and its rows commit
// to scrollback one at a time; every cell **word-wraps** into its column (taller
// rows) instead of truncating with `…` when the grid is narrow, matching codex.
// See `AssistantRenderer`/`StreamRender`. ---
/// Dim colour of a table's box-drawing borders (`│ ─ ┌┬┐ ├┼┤ └┴┘`).
const TABLE_BORDER_COLOR: Color = TOOL_DIM_COLOR;
/// The floor a table column shrinks to before its cells word-wrap (codex uses 3):
/// a narrow column keeps at least this many display columns, and cells wrap into
/// it across multiple rows rather than losing text to a `…`.
const TABLE_MIN_COL: usize = 3;
/// Records fallback (`docs/table-streaming.md`, Claude Code's key/value transpose):
/// a column at least this wide is considered scannable, so it never triggers the
/// fallback even if its content wraps (it's a legitimately wide narrative column).
const TABLE_SCANNABLE_COL: usize = 12;
/// A cell that wraps into at least this many rows *in a narrow column* means the
/// grid is growing tall because columns are starved — flip to vertical records.
const TABLE_RECORDS_MIN_LINES: usize = 3;
/// The narrowest value column the inline `label: value` record form keeps; below
/// this the field stacks (label on its own line, value indented beneath).
const TABLE_RECORD_MIN_VALUE: usize = 12;
/// A stacked record value's indent under its label line.
const TABLE_RECORD_STACK_INDENT: usize = 2;
/// The `─` rule between records caps at this many columns instead of spanning the
/// full content width — Claude Code's shorter separator reads cleaner for the
/// short key/value fields (a wide records table's full-width rule looked heavy).
const TABLE_RECORD_SEPARATOR_WIDTH: usize = 40;

// --- Inline markdown styling (docs/markdown.md). A prose line's `**bold**`,
// `*italic*`, `~~strike~~`, `` `code` `` and `[text](url)` render with these;
// emphasis is modifier-only (bold/italic/crossed-out), code and links carry a
// colour. Parsing lives in `markdown::parse_inline`; `ui` owns the styling. ---
/// Inline `` `code` `` — a distinct cyan so it reads as code within prose.
const INLINE_CODE_COLOR: Color = Color::Rgb(0x56, 0xB6, 0xC2);
/// A link's URL, shown as ` (url)` after its text — blue and underlined.
const LINK_URL_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);

// --- List and blockquote styling (docs/markdown.md). Bullets keep `-`, ordered
// items keep `N.` in an accent colour; a blockquote's `>` and text render dim.
// Continuation rows hang under the item's text. ---
/// The accent colour of an ordered list's `N.`/`N)` marker.
const LIST_MARKER_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// A blockquote's `>` marker and text — dim, so a quote reads as secondary.
const QUOTE_COLOR: Color = TOOL_DIM_COLOR;

// Code syntax-highlight colours now come from the `highlight` module's theme
// (syntect + two_face, Catppuccin Mocha — codex parity), baked into each
// `highlight::Seg`'s `Style`. `ui` no longer owns a code palette or maps token
// kinds to colours; it just renders the segments the grammar produced. See
// `docs/markdown.md`.

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
/// Prefix for the first result line: indent + a turnstile glyph + two spaces
/// (Claude-Code's two-space corner). Continuation lines are indented by its
/// display width so a multi-line result aligns under the content (see
/// [`result_row`]).
const TOOL_RESULT_PREFIX: &str = "  ⎿  ";
/// Prefix for the "+N lines" hint line under a capped peek — the `…` aligns
/// under the corner content (the [`TOOL_RESULT_PREFIX`] width of leading
/// spaces).
const TOOL_MORE_PREFIX: &str = "     … ";
/// Hint telling the user how to see the full output.
const EXPAND_HINT: &str = " (ctrl+o to expand)";
/// How many output lines a `!` shell command shows inline before collapsing the
/// rest behind a `… +N lines (ctrl+o to expand)` hint (Claude-Code's exec-cell
/// preview). The full output is always in the Ctrl+O view.
const TOOL_PEEK_LINES: usize = 4;
/// The finished peek's safety ceiling in wrapped display **rows**: the budget
/// above is *source lines* (each shown fully wrapped — a long first line must
/// not push its siblings out of the peek), so without a ceiling ONE
/// pathological line (a minified bundle, a 64 KiB log line) would balloon a
/// committed cell into hundreds of rows now that lines wrap instead of
/// clipping. Three rows per budgeted line keeps the everyday case — a `sudo`
/// error wrapping to 2–3 rows at a narrow width — fully visible.
const TOOL_PEEK_MAX_ROWS: usize = TOOL_PEEK_LINES * 3;
/// How many wrapped rows a tool's `● name(args)` header shows **inline** (and in
/// the live preview) before the rest is cut with [`TOOL_HEADER_ELLIPSIS`] — so a
/// very long `bash` command doesn't flood the cell. The Ctrl+O transcript view
/// (`tool_full_lines`) passes `None` and renders the whole command. Claude-Code's
/// truncated command header.
const TOOL_HEADER_MAX_ROWS: usize = 3;
/// The marker spliced in (before the closing `)`) when a header is truncated at
/// [`TOOL_HEADER_MAX_ROWS`].
const TOOL_HEADER_ELLIPSIS: &str = "…";
/// Dim marker appended at the end of a `!` shell command's **expanded** output
/// (`tool_full_lines`) when it was cut at the in-memory cap (`tool.truncated`),
/// to show that more output was dropped. See `docs/shell-command.md`.
const TOOL_TRUNCATED_MARKER: &str = "…";
/// Placeholder body for a finished tool that produced no output.
const TOOL_NO_OUTPUT: &str = "(no output)";

// ---------------------------------------------------------------------------
// Background shells (docs/background.md): the backgrounded cell's fixed row,
// the running-command Ctrl+B hint, the ↓ manager band, and the footer count.
// ---------------------------------------------------------------------------

/// The fixed `⎿` body of a call resolved as [`ToolStatus::Backgrounded`] —
/// the stored output (the model-facing launch text) is never shown.
const TOOL_BACKGROUNDED: &str = "Running in the background (↓ to manage)";
/// The dim live-only hint under a running command's preview cell: Ctrl+B
/// moves it to the background. Never committed to scrollback.
const TOOL_BACKGROUND_HINT: &str = "(ctrl+b to run in background)";
/// How long a command must have been running before its preview shows the
/// `(ctrl+b to run in background)` hint — Claude-Code-style, so a command that
/// finishes right away never flashes it (Ctrl+B itself still works the whole
/// time; only the discoverability hint waits). Gated on the boundary-injected
/// [`App::command_elapsed`]. See `docs/background.md`.
const TOOL_BACKGROUND_HINT_DELAY: Duration = Duration::from_secs(3);
/// The ↓ manager's list title.
const BG_TITLE: &str = "Background";
/// The details page's title.
const BG_DETAILS_TITLE: &str = "Shell details";
/// The list's empty state — shown when every shell has finished.
const BG_EMPTY: &str = "No tasks currently running";
/// The list page's key hints.
const BG_LIST_HINTS: &str = "↑/↓ to select · Enter to view · x to stop · Esc to close";
/// The empty state's key hints (nothing to stop).
const BG_EMPTY_HINTS: &str = "↑/↓ to select · Enter to view · Esc to close";
/// The details page's key hints.
const BG_DETAILS_HINTS: &str = "← to go back · Esc/Enter/Space to close · x to stop";
/// The `(running)` suffix on a list row.
const BG_ROW_SUFFIX: &str = " (running)";
/// The band's two-space inset (the picker/footer indent).
const BG_INDENT: &str = "  ";
/// The selected list row's marker (the resume picker's `❯`).
const BG_MARKER: &str = "❯ ";
/// At most this many list rows show at once (the window follows the
/// selection, like the pickers).
const BG_MENU_MAX_ROWS: usize = 8;
/// The details output box's interior height: the last rows of the live
/// output tail, blank-padded (the mock's fixed box).
const BG_OUTPUT_ROWS: usize = 10;
/// The details page's field labels, padded to one column.
const BG_FIELD_STATUS: &str = "Status:   ";
const BG_FIELD_RUNTIME: &str = "Runtime:  ";
const BG_FIELD_COMMAND: &str = "Command:  ";
/// The details page's output-box heading.
const BG_OUTPUT_LABEL: &str = "Output:";
/// The value of the status field while listed (an exited shell leaves the
/// manager, so a listed one is always running).
const BG_STATUS_RUNNING: &str = "running";
/// The manager's title/selection accent (the palette accent) and dim text.
const BG_SELECTED_COLOR: Color = MENU_SELECTED_COLOR;
const BG_DIM_COLOR: Color = TOOL_DIM_COLOR;
/// The notice bullet colours: green success, red failure/stop.
const BG_NOTICE_OK_COLOR: Color = TOOL_OK_COLOR;
const BG_NOTICE_FAIL_COLOR: Color = TOOL_FAIL_COLOR;
/// Placeholder body for a still-executing tool — the `⎿ Running…` row, shown
/// under a backend tool's `● name(args)` header (req 2: a running cell shows the
/// header *and* this row, previewed live) and as the whole `!` shell cell.
const TOOL_RUNNING: &str = "Running…";
/// Placeholder body for a tool queued in a **parallel batch** but not yet
/// started — the dim `⎿ Waiting…` row shown under a not-yet-running sibling's
/// `● name(args)` header while another call in the batch executes. See
/// `docs/parallel-tools.md`.
const TOOL_WAITING: &str = "Waiting…";

/// Blue — a tool that is still executing.
const TOOL_RUNNING_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// Dim grey — a tool queued in a batch but not yet started (its `● name(args)`
/// bullet and `⎿ Waiting…` row read muted, distinct from the blue running head,
/// since it hasn't begun). Shares the argument/peek dim grey.
const TOOL_WAITING_COLOR: Color = TOOL_DIM_COLOR;
/// Green — a tool that finished successfully. A vivid, saturated green (rather
/// than the old muted `#98C379`) so the `●` success bullet clearly stands out,
/// Claude-Code style. Shared by the `+`-line diff colour, the active-model tick
/// and the context view's assistant tag ([`TOOL_DIFF_ADD_COLOR`] etc.).
const TOOL_OK_COLOR: Color = Color::Rgb(0x3F, 0xB9, 0x50);
/// Red — a tool that failed (shares the backend-error red).
const TOOL_FAIL_COLOR: Color = ERROR_COLOR;
/// White — the tool's name.
const TOOL_NAME_COLOR: Color = AI_COLOR;
/// White (the normal assistant reply colour) + bold — the whole `(...)` header
/// body: the command/args text **and** its framing `(`/`)` (and a truncation
/// `…`) alike, so a `bash` command and its brackets read as prominently as a
/// normal reply rather than the old dim grey. Claude-Code's noticeable tool
/// header; shared by every tool via [`tool_header_lines`].
const TOOL_ARGS_COLOR: Color = AI_COLOR;
/// White (the normal reply colour) — a finished tool's **output** under the `⎿`
/// gutter (command/shell output), so it's as legible as a normal message rather
/// than dim grey. The `⎿` corner, the `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `… +N lines` hint all stay [`TOOL_DIM_COLOR`].
const TOOL_OUTPUT_COLOR: Color = AI_COLOR;
/// Dim grey — a tool's `⎿` gutter corner, its `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `… +N lines` hint.
const TOOL_DIM_COLOR: Color = Color::Rgb(0x8A, 0x8A, 0x8A);
/// Green — an added (`+`) line in an `edit`/`write` diff cell (codex's diff
/// look, adapted to the `⎿` gutter; see `docs/tools.md`).
const TOOL_DIFF_ADD_COLOR: Color = TOOL_OK_COLOR;
/// Red — a removed (`-`) line in an `edit`/`write` diff cell.
const TOOL_DIFF_DEL_COLOR: Color = TOOL_FAIL_COLOR;
/// Dark-green background tint of an added row in a numbered `edit`/`write`
/// cell (codex's dark-theme add tint): the syntax-coloured text reads over it
/// and the row pads to the full width, like the user-message block.
const TOOL_DIFF_ADD_BG: Color = Color::Rgb(0x21, 0x3A, 0x2B);
/// Dark-red background tint of a removed row (codex's dark-theme delete
/// tint); the removed text is additionally dimmed, codex-style.
const TOOL_DIFF_DEL_BG: Color = Color::Rgb(0x4A, 0x22, 0x1D);
/// How many numbered body rows a `write`/`edit` cell shows inline before the
/// `… +N lines (ctrl+o to expand)` hint (Claude-Code's ~10-row Write preview;
/// other tools keep the tighter [`TOOL_PEEK_LINES`]).
const FILE_PEEK_LINES: usize = 10;
/// The model tools whose output is a file change — rendered as the numbered,
/// syntax-highlighted codex-style cell when the output is in the
/// `llm::tools` gutter format ([`file_cell_lines`]), or with the legacy
/// first-char `+`/`-` colouring when it isn't (old sessions, error bodies).
/// A `!` shell command is never one (its output is command output).
const DIFF_TOOL_NAMES: [&str; 2] = ["Edit", "Write"];

/// The model tools whose output is **command output** — a shell run, streamed
/// and framed with an `Exit code: N` line. They render like the `!` shell cell
/// (a multi-line `⎿` peek, the frame stripped for display) and **tail** their
/// output live while running (`docs/tool-streaming.md`). Only `bash` today; a
/// non-command generic tool keeps the single collapsed peek line.
const COMMAND_TOOL_NAMES: [&str; 1] = ["Bash"];

// --- Tool-output view (the Ctrl+O full-screen overlay) — codex's Ctrl+T
// transcript pager: a slash-tiled dim title row over a scrolling body (the
// full conversation transcript — every message plus every tool call's
// *complete* (expanded) output — with vi-style `~` filler rows past its end),
// closed by a `─` separator carrying the scroll percentage and two dim
// key-hint rows above a final blank row. ---

/// The pager's spaced-caps title, overlaid on the slash tiling as
/// `/ T R A N S C R I P T` (codex's transcript overlay header).
const TOOL_VIEW_TITLE: &str = "T R A N S C R I P T";
/// Rows of chrome above the scrolling body (the slash-tiled title row).
const TOOL_VIEW_TITLE_ROWS: u16 = 1;
/// Rows of chrome below the body: the `─` separator carrying the scroll
/// percentage, two key-hint rows, and the final blank row (codex's pager).
const TOOL_VIEW_FOOTER_ROWS: u16 = 4;
/// First key-hint row under the separator (codex's pager hints, all dim).
const TOOL_VIEW_HINT_KEYS: &str = " ↑/↓ to scroll   pgup/pgdn to page   home/end to jump";
/// Second key-hint row: every key that closes the overlay.
const TOOL_VIEW_HINT_QUIT: &str = " q/esc/ctrl+o to quit";
/// Second key-hint row while a backtrack preview highlights a user message —
/// codex's highlighted-pager footer (`docs/backtrack.md`); it replaces
/// [`TOOL_VIEW_HINT_QUIT`], whose Esc meaning the preview takes over.
const TOOL_VIEW_HINT_BACKTRACK: &str =
    " esc/← to edit prev   → to edit next   enter to edit message   q to cancel";
/// The vi-style filler marking body rows below the transcript's end.
const TOOL_VIEW_FILL: &str = "~";
/// The dim placeholder shown when the transcript has nothing to list yet.
const TOOL_VIEW_EMPTY: &str = "Nothing here yet.";

// --- Ctrl+D context-debug view (the third alternate-screen overlay) — the
// raw LLM context window (docs/context.md): the transcript pager's chrome
// (slash-tiled title, `~` filler, percentage separator, dim key hints) over
// a body listing exactly what the model is sent — the system prompt, then
// every derived context message role-tagged, its text **verbatim** (image
// placeholders and bracketed tool/shell/notice formats unrendered) with the
// attachment paths dim beneath. ---

/// The view's spaced-caps title, overlaid on the slash tiling.
const CONTEXT_VIEW_TITLE: &str = "C O N T E X T";
/// Second key-hint row: every key that closes the view.
const CONTEXT_VIEW_HINT_QUIT: &str = " q/esc/ctrl+d to quit";
/// The dim placeholder when the context window is empty (no system prompt —
/// the dummy sends none — and nothing said yet).
const CONTEXT_VIEW_EMPTY: &str = "Context is empty — send a message to fill it.";
/// The system prompt's role tag — set apart from a mid-conversation
/// `system:` note (a derived `[system]`/`[error]` notice).
const CONTEXT_SYSTEM_PROMPT_TAG: &str = "system prompt:";
/// The inset of an entry's raw text (and attachment rows) under its tag.
const CONTEXT_INDENT: &str = "  ";
/// The label of an attachment row under a user entry's text.
const CONTEXT_IMAGE_LABEL: &str = "image: ";
/// The prefix of a native tool-call row under an assistant entry —
/// `→ name(arguments)`, the model's request in the raw wire form.
const CONTEXT_TOOL_CALL_PREFIX: &str = "→ ";
/// Role-tag colours — the tool palette's hues (user blue, assistant green,
/// system amber, tool-result purple) so the roles scan apart at a glance.
const CONTEXT_USER_COLOR: Color = TOOL_RUNNING_COLOR;
const CONTEXT_ASSISTANT_COLOR: Color = TOOL_OK_COLOR;
const CONTEXT_SYSTEM_COLOR: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
/// The `tool:` result-role tag and the `→ name(args)` tool-call lines under an
/// assistant entry — a distinct purple so the native tool round-trip reads
/// apart from plain assistant text.
const CONTEXT_TOOL_COLOR: Color = Color::Rgb(0xC6, 0x78, 0xDD);

// --- /resume session picker (the other alternate-screen overlay) — codex's
// resume picker, sized down (docs/resume.md): the same slash-tiled title
// chrome as the transcript pager, a type-to-search line, dense one-line
// session rows (`❯ {age:12}{preview}`, the palette's selection-by-colour),
// and a bottom rule carrying `{selected+1}/{total}` over a dim key-hint row. ---

/// The picker's spaced-caps title, overlaid on the slash tiling.
const RESUME_TITLE: &str = "R E S U M E";
/// The dim search-line placeholder while the query is empty (codex's).
const RESUME_SEARCH_PLACEHOLDER: &str = "Type to search";
/// The search line's prefix once a query is typed.
const RESUME_SEARCH_PROMPT: &str = "Search: ";
/// The picker's key-hint row (dim, under the separator). The search line's
/// own placeholder carries the type-to-search hint.
const RESUME_HINTS: &str = " ↑/↓ select   enter resume   esc cancel   tab + ←/→ filter/sort";
/// The dim list placeholder when nothing was ever saved (codex's).
const RESUME_NO_SESSIONS: &str = "No sessions yet";
/// The dim list placeholder when the query matches nothing (codex's).
const RESUME_NO_MATCH: &str = "No results for your search";
/// The age column's width in the dense rows — codex's 12-col relative date.
const RESUME_AGE_WIDTH: usize = 12;
/// The two-space inset shared by the search line and the placeholder rows
/// (the row marker is the same width, so everything lines up).
const RESUME_INDENT: &str = "  ";
/// The selected row's marker; unselected rows get spaces (codex's `❯ `).
const RESUME_MARKER: &str = "❯ ";
/// The selected row's full-width background tint — codex blends white over
/// the terminal background; a grey lift noticeably lighter than the
/// user-message block plays that role here.
const RESUME_SELECTED_BG: Color = Color::Rgb(0x3A, 0x40, 0x46);
/// The Tab-focused toolbar control's active value — codex's magenta.
const RESUME_FOCUS_COLOR: Color = Color::Magenta;
/// The gap between the toolbar's Filter and Sort tab pairs (codex's).
const RESUME_TOOLBAR_GAP: &str = "   ";
/// The smallest gap kept between the search text and the toolbar before the
/// toolbar compacts (and then drops).
const RESUME_TOOLBAR_MIN_GAP: usize = 2;

// --- Inline `/model` picker (docs/llm.md). Unlike `/resume`, this one is
// **inline** — it replaces the composer in the bottom live region with its own
// `>` search prompt over a scrolling model list, framed by top/bottom rules
// like the input box. A gold header, the palette's cyan selection accent, a
// dim `[provider]` tag, a green ✓ on the active model, then a `(n/total)`
// counter and a `Model Name:` line — the shape of the user's mock. ---

/// The two-space inset shared by every picker row (search, list rows,
/// counter, name) so the content sits off the frame's left edge.
const MODEL_INDENT: &str = "  ";
/// The search line's prompt glyph (cyan), the `❯` the query types after.
const MODEL_PROMPT: &str = "❯ ";
/// Cyan — the `❯` prompt and the selected row (the palette-selection accent).
const MODEL_SELECTED_COLOR: Color = MENU_SELECTED_COLOR;
/// The selected row's marker; unselected rows get spaces the same width.
const MODEL_MARKER: &str = "→ ";
/// Light grey — an unselected model id (readable but quieter than the selection).
const MODEL_ID_COLOR: Color = Color::Rgb(0xC8, 0xC8, 0xC8);
/// Dim — the `[provider]` tag, the counter, and the `Model Name:` line.
const MODEL_META_COLOR: Color = TOOL_DIM_COLOR;
/// Green — the ✓ marking the currently-active model (shares the tool-ok green).
const MODEL_ACTIVE_COLOR: Color = TOOL_OK_COLOR;
/// The mark appended to the active model's row.
const MODEL_ACTIVE_MARK: &str = " ✓";
/// The label opening the friendly-name line under the list.
const MODEL_NAME_LABEL: &str = "Model Name: ";
/// The most model rows shown at once; longer lists scroll to keep the selection
/// **centered** (`centered_window`) so the user sees the models above and below
/// it, not just up to the edge it last crossed.
const MODEL_MENU_MAX_ROWS: u16 = 10;
/// The list placeholder while the fetch is in flight.
const MODEL_LOADING: &str = "Loading models…";
/// The list placeholder when the provider returned no models.
const MODEL_NONE: &str = "No models available";
/// The list placeholder when the query matches nothing.
const MODEL_NO_MATCH: &str = "No matching models";
/// The list placeholder when no provider has a key yet ([`ModelLoad::NeedsLogin`]) —
/// shown cyan (an actionable hint, not a red error) pointing at `/login`.
const MODEL_LOGIN_HINT: &str = "No API key yet — run /login to add one";
/// The separator between the `(n/total)` counter and its trailing load status.
const MODEL_STATUS_SEP: &str = "   ·   ";
/// The counter's dim suffix while other providers are still being fetched (the
/// list shows what's landed so far and keeps growing). See `docs/llm.md`.
const MODEL_LOADING_MORE: &str = "loading more…";

// --- The `/login` API-key onboarding flow (docs/llm.md). A two-step inline
// picker sharing the model picker's framed look and colours: step 1 lists the
// providers to choose from (headerless, like `/model`), step 2 collects the key
// masked under a periwinkle prompt. All styling reuses the `MODEL_*` consts
// (indent, `❯` prompt, cyan selection, dim meta, green ✓, `→` marker) plus the
// `LOGIN_*` strings/geometry below. ---

/// Periwinkle (Claude Code's accent, `#96a0d5`) — the `Enter your … API key`
/// prompt on the key-entry step.
const LOGIN_KEY_PROMPT_COLOR: Color = Color::Rgb(0x96, 0xA0, 0xD5);
/// The dim hint under the provider list, prefixing the real `.env` path
/// (`onboarding.env_path`) so it names where the key actually lands.
const LOGIN_PROVIDER_HINT_PREFIX: &str = "Keys are saved to ";
/// The dim hint under the key-entry field.
const LOGIN_KEY_HINT: &str = "Enter to save · Esc to go back";
/// The dim placeholder shown in the key field before anything is entered.
const LOGIN_KEY_PLACEHOLDER: &str = "paste your API key, then press Enter";
/// The glyph each entered key character is masked to.
const LOGIN_MASK_CHAR: char = '•';
/// The list placeholder when the provider filter matches nothing.
const LOGIN_NO_MATCH: &str = "No matching providers";
/// The most provider rows shown at once (longer lists scroll to keep the
/// selection **centered**, like the `/model` list — `centered_window`).
const LOGIN_MENU_MAX_ROWS: u16 = 8;
/// Fixed rows framing the **provider** step (headerless): top rule, gap, search,
/// gap, (list), counter, gap, hint, gap, bottom rule.
const LOGIN_PROVIDER_CHROME_ROWS: u16 = 9;
/// The row the provider-step `❯` filter sits on — top(0) gap(1) search(2).
/// Shared by [`render_key_onboarding`] and [`cursor_position`].
const LOGIN_SEARCH_ROW: u16 = 2;
/// Total rows of the **key** step (no list): top rule, gap, prompt, gap, input,
/// gap, hint, gap, bottom rule.
const LOGIN_KEY_ROWS: u16 = 9;
/// The row the key-entry `❯` field sits on — top(0) gap(1) prompt(2) gap(3)
/// input(4).
const LOGIN_KEY_INPUT_ROW: u16 = 4;

// --- Transcript timestamps (Ctrl+O view only). Only the *user* message shows
// its wall-clock stamp: dim, right-aligned on its own line below the message
// (`hh:mm AM/PM`). AI replies, tools, and turn summaries record a stamp too but
// never display it; the inline view never shows any. See docs/timestamps.md. ---

/// Dim grey — the user message's right-aligned timestamp in the Ctrl+O transcript.
const TIMESTAMP_COLOR: Color = TOOL_DIM_COLOR;

// --- Live status indicator (codex / Claude-Code style). While a turn is in
// flight a status line sits in the strip above the box (with a blank gap row
// between it and the box's top rule):
// `(●•·   ) {verb}… ({elapsed} · {↓|↑} {n} tokens · Thinking for {m})`. The
// line opens with a **comet spinner** (a Larson-scanner sweep: a white head
// dragging a fading grey tail back and forth between dim walls, one frame per
// `SPINNER_INTERVAL` — see [`spinner_spans`]); the working verb is picked
// per-turn (in `App`) and its white text carries a codex-style **shimmer**: a
// bright-white band sweeps across the white-grey text (see [`shimmer_spans`],
// ported from openai/codex `tui/src/shimmer.rs`). The elapsed / thinking / done
// times are humanized by [`format_elapsed`] (`45s`, `1m 30s`, `1h 1m`). On
// finish a dim, bullet-less `{done verb} for {n}` summary commits to scrollback (a
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
/// Amber — the `retrying {n}/{max}` clause. A warning hue (One-Dark yellow),
/// distinct from the dim metrics and the error red: the request hasn't failed,
/// it's recovering. See `docs/llm.md`.
const STATUS_RETRY_COLOR: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
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
/// Dim grey — the committed `"{done verb} for {n}"` turn summary.
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
/// Sized to hold the whole [`crate::app::COMMANDS`] registry so a bare `/`
/// lists every command without scrolling (the
/// `the_menu_cap_holds_the_whole_command_registry` test pins it to the
/// registry's growth — `/compact` grew it to 8, `/init` to 9).
const MENU_MAX_ROWS: u16 = 9;
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
/// row in declaration order. The `esc` entry is context-sensitive —
/// [`shortcuts_lines`] swaps it for ` to interrupt` while a turn runs (codex's
/// quit entry does the same) and for the [`SHORTCUTS_BACKTRACK`] edit hint
/// when idle with a previous user message to edit (docs/backtrack.md).
const SHORTCUTS: &[(&str, &str)] = &[
    ("/", " for commands"),
    ("!", " for shell command"),
    ("↑", " for input history"),
    ("ctrl+r", " to search history"),
    ("shift+enter", " for newline"),
    ("ctrl+o", " for tool output"),
    ("esc", " to quit"),
    ("ctrl+c", " to quit"),
    ("alt+↑", " to edit queue"),
    ("tab", " to queue next turn"),
    ("ctrl+v", " for image paste"),
    ("ctrl+d", " for llm context"),
    ("shift+tab", " to cycle thinking"),
];
/// The display column where a row's second entry starts (the first entry is
/// padded out to here) — [`MENU_DESC_COL`]'s tidy-column idea.
const SHORTCUTS_COL: usize = 25;
/// Cyan — an entry's key (the palette-selection accent).
const SHORTCUTS_KEY_COLOR: Color = MENU_SELECTED_COLOR;
/// Dim grey — an entry's label (codex dims the whole overlay).
const SHORTCUTS_TEXT_COLOR: Color = TOOL_DIM_COLOR;

// --- Esc-Esc backtrack (docs/backtrack.md). A primed first Esc takes the
// footer slot with a hint naming the second (codex's `esc_backtrack_hint`
// footer); the transcript overlay then highlights the selected user message
// by *reversing* its rows (codex's `user_message_style().reversed()`). ---

/// The primed hint's key, bold-cyan like the search-line hint keys.
const BACKTRACK_HINT_KEY: &str = "esc";
/// The primed hint's dim label — codex's "esc again to edit previous message"
/// wording, minus the key it highlights separately.
const BACKTRACK_HINT_LABEL: &str = " again to edit previous message";
/// The shortcuts-band `esc` entry while idle with a backtrack target: the
/// gesture replaces quit as Esc's idle meaning (see [`shortcuts_lines`]).
const SHORTCUTS_BACKTRACK: (&str, &str) = ("esc esc", " to edit previous");

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
/// The **focused** shell indicator's fill: ↓ lights the footer's `{n} shell(s)`
/// segment on the palette-selection cyan and waits for the Enter that opens the
/// ↓ manager band (Claude-Code-style — see `docs/background.md`). Only that one
/// segment changes; the model / cwd / context-gauge segments stay dim.
const FOOTER_FOCUS_BG: Color = MENU_SELECTED_COLOR;
/// The focused indicator's ink on that cyan fill — near-black, so the lit
/// segment reads as a chip rather than a smudge.
const FOOTER_FOCUS_FG: Color = Color::Rgb(0x1E, 0x1E, 0x1E);

// --- The transient toast: a one-line, self-clearing status message pinned just
// above the box (`Copied last message to clipboard`, `/resume is disabled …`).
// It occupies the bottom of the strip, directly above the box's top rule, and
// fades after a few seconds (the expiry timed at the I/O boundary). See
// docs/toast.md. ---

/// Indent prefixed to the toast row — the two-space inset shared with the
/// footer and the queued messages.
const TOAST_INDENT: &str = "  ";
/// An info toast's colour (a confirmation / soft rejection) — dim, like the footer.
const TOAST_COLOR: Color = TOOL_DIM_COLOR;
/// An error toast's colour (a failure — a `/copy` error, a bad model switch) —
/// the error red.
const TOAST_ERROR_COLOR: Color = ERROR_COLOR;

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

// --- The startup header banner (docs/header.md): an ASCII wordmark + version +
// cwd + hint, committed to scrollback at launch and re-emitted atop every full
// repaint (resize, `/clear`) so it survives the scrollback purge. Pure chrome,
// like the footer — never in `history`, so it never reaches the model, the
// `/resume` rollout, or the Ctrl+O transcript. Borderless (no `─` rule row, no
// bare prompt, no model name) so the smoke resize counters don't see it. ---

/// The full ANSI-Shadow wordmark, shown when the terminal is wide enough
/// ([`header_lines`] falls back to [`HEADER_LOGO_COMPACT`], then a text badge).
/// One `&str` per row so the leading spaces survive verbatim — a `\`-continued
/// string literal would strip them and shift the `A`'s crown a column left.
const HEADER_LOGO_FULL: &[&str] = &[
    " █████╗ ██╗  ████████╗███████╗██████╗   ███████╗███████╗██████╗  ██████╗",
    "██╔══██╗██║  ╚══██╔══╝██╔════╝██╔══██╗  ╚══███╔╝██╔════╝██╔══██╗██╔═══██╗",
    "███████║██║     ██║   █████╗  ██████╔╝    ███╔╝ █████╗  ██████╔╝██║   ██║",
    "██╔══██║██║     ██║   ██╔══╝  ██╔══██╗   ███╔╝  ██╔══╝  ██╔══██╗██║   ██║",
    "██║  ██║███████╗██║   ███████╗██║  ██║  ███████╗███████╗██║  ██║╚██████╔╝",
    "╚═╝  ╚═╝╚══════╝╚═╝   ╚══════╝╚═╝  ╚═╝  ╚══════╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ",
];

/// The compact half-block wordmark, shown on mid-width terminals (too narrow for
/// [`HEADER_LOGO_FULL`], wide enough to still show art).
const HEADER_LOGO_COMPACT: &[&str] = &[
    "▄▀█ █   ▀█▀ █▀▀ █▀▄  ▀▀█ █▀▀ █▀▄ █▀█",
    "█▀█ █▄▄  █  ██▄ █▀▄  █▄▄ ██▄ █▀▄ █▄█",
];

/// The plain-text name for the one-line badge (a very narrow terminal, too small
/// for either wordmark).
const HEADER_NAME: &str = "ALTER ZERO";
/// The tagline under the logo — the persona, echoing `prompts/alter_zero.md`.
const HEADER_TAGLINE: &str = "autonomous ai agent · terminal ui";
/// The command hint under the metadata — bare `/token`s (the slashes accented,
/// separators dim). Deliberately prose-free so it can't collide with the smoke
/// suite's `for commands` / footer markers.
const HEADER_HINT: &[&str] = &["/help", "/model", "/resume"];
/// Indent shared with the footer and messages — the whole metadata block sits
/// two columns in. The logo art is drawn flush-left.
const HEADER_INDENT: &str = "  ";
/// The logo gradient's left endpoint — the inline-code cyan ([`INLINE_CODE_COLOR`]).
const HEADER_GRADIENT_START: (u8, u8, u8) = (0x56, 0xB6, 0xC2);
/// The logo gradient's right endpoint — the link blue ([`LINK_URL_COLOR`]).
const HEADER_GRADIENT_END: (u8, u8, u8) = (0x61, 0xAF, 0xEF);
/// The version badge + hint-token colour — the cyan accent, so they pop.
const HEADER_ACCENT_COLOR: Color = INLINE_CODE_COLOR;
/// The tagline / cwd / separator colour — dim, like the footer.
const HEADER_META_COLOR: Color = FOOTER_COLOR;

// --- Live-region geometry. The bottom region's height is dynamic: it grows with
// the wrapped input (see `live_height`). `render_live` and `cursor_position` both
// derive their layout from `input_box` so the drawn text and cursor never drift;
// `main.rs`/`term.rs` size the viewport from `live_height`/`LIVE_MIN_HEIGHT`. ---

/// A blank gap row between the streaming preview and the box, so the live reply
/// never butts up against the box's top rule. Present only while streaming.
const GAP_ROWS: u16 = 1;
/// The input box's non-text rows: a top rule and a bottom rule.
const INPUT_CHROME_ROWS: u16 = 2;
/// The smallest the live region ever gets: a one-text-row box framed by two
/// rules (idle has no preview strip). `main.rs` sizes the initial viewport from this.
pub const LIVE_MIN_HEIGHT: u16 = INPUT_CHROME_ROWS + 1;

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
const fn strip_rows(has_status: bool, preview_rows: u16) -> u16 {
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
/// sizes the layout from the same [`preview_lines`]). Used by
/// [`render_live`]/[`cursor_position`]/`main.rs` to feed `strip_rows`.
#[must_use]
pub fn preview_rows(app: &App, width: u16) -> u16 {
    // An agent session view previews the *viewed agent's* stream — its live
    // tool cells or its reply's last row (docs/agent-tool.md).
    if let Some(run) = app.viewed_agent() {
        return u16::try_from(agent_view_preview_lines(run, width).len()).unwrap_or(u16::MAX);
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

/// Rows of live-region chrome that must stay visible under a tall forming-table
/// preview: the preview's gap, the status line + its gap, the minimal box, and
/// the session footer — plus one row of headroom.
const STREAM_PREVIEW_RESERVED_ROWS: u16 =
    GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS + LIVE_MIN_HEIGHT + 2;
/// The forming-table preview never shrinks below this many rows, however small
/// the terminal — enough to see the newest row or two plus the border.
const STREAM_PREVIEW_MIN_ROWS: usize = 3;

/// The cap the boundary passes to [`StreamRender::preview`]: how many strip
/// rows a multi-row (forming-table) preview may take at this terminal height
/// before it tail-follows its newest rows — the screen minus the live-region
/// chrome ([`STREAM_PREVIEW_RESERVED_ROWS`]), floored at
/// [`STREAM_PREVIEW_MIN_ROWS`] (docs/table-streaming.md).
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
/// [`strip_rows`] for how `has_status`/`has_preview` size it) plus the
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

/// The fixed rows framing the inline `/model` picker's list when a real model
/// is highlighted: the top rule, a gap, the search line, a gap (4 above), then
/// below the list a counter, a gap, the model-name line, a gap, and the bottom
/// rule (5 below — headerless, the "Showing models…" banner was dropped). The
/// list rows sit between them (see [`model_list_rows`]).
const MODEL_CHROME_ROWS: u16 = 9;

/// The framing rows when the picker shows a **placeholder** instead of a model
/// (loading / error / needs-login / no match): the same 4 above the list, then
/// a single gap and the bottom rule. The blank counter + name rows collapse to
/// that one gap so the box hugs the placeholder. See [`model_chrome_rows`].
const MODEL_CHROME_ROWS_COLLAPSED: u16 = 6;

/// Whether the picker shows its counter + model-name detail rows below the
/// list — only when a real model is listed (Ready with at least one match).
/// The placeholder states have a blank counter and name, so those rows collapse
/// to a single trailing gap ([`MODEL_CHROME_ROWS_COLLAPSED`]).
fn model_has_detail(picker: &ModelPicker) -> bool {
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
fn live_layout(
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

#[allow(clippy::too_many_arguments)]
fn input_box(
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
/// `width == 0` disables wrapping, like [`wrap_text`]. Used for **diff
/// bodies** (code — inline peek and Ctrl+O alike, coloured by source line);
/// command/shell output word-wraps via [`wrap_output`] instead, and
/// **messages** keep [`wrap_text`].
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

/// Wrap `text` to `width` columns at **word boundaries, preserving
/// whitespace** — the middle ground between [`wrap_text`] (word boundaries but
/// *collapses* space runs) and [`wrap_verbatim`] (preserves whitespace but
/// hard-breaks *mid-word*). A tool's captured output wants both: prose errors
/// (`sudo: …`) should break cleanly at spaces, yet a line's exact spaces must
/// survive so `ls -l` columns / indentation that already fit are untouched —
/// only an over-wide line reflows. The boundary space stays at the end of the
/// current row (so concatenating the rows reconstructs the line byte-exactly)
/// and continuation rows start at a word. A single word wider than `width` is
/// hard-broken on grapheme boundaries, measured in display columns (like
/// [`wrap_segment`]'s hard break). `width == 0` disables wrapping. Used by the
/// command/shell peek ([`result_peek_block`]), the running tail
/// ([`running_command_lines`]), and the Ctrl+O output ([`tool_full_lines`]),
/// so the three wrap identically.
fn wrap_output(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(str::to_string).collect();
    }
    let width = width as usize;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        // Byte index in `cur` just past the most recent whitespace grapheme — a
        // clean break point. `None` until the first space and after each break.
        let mut brk: Option<usize> = None;
        for g in line.graphemes(true) {
            let g_w = cols(g);
            if cur_w > 0 && cur_w + g_w > width {
                match brk {
                    // Break at the last space: it stays on the current row, the
                    // partial word after it carries to the next.
                    Some(bp) => {
                        let cont = cur.split_off(bp);
                        out.push(std::mem::replace(&mut cur, cont));
                        cur_w = cols(&cur);
                    }
                    // No space to break on — hard-break the over-long word.
                    None => {
                        out.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                }
                brk = None;
            }
            cur.push_str(g);
            cur_w += g_w;
            if g.chars().all(char::is_whitespace) {
                brk = Some(cur.len());
            }
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

/// The `/compact` marker cell's text — codex's "Context compacted" info cell,
/// verbatim. See `docs/compact.md`.
pub const COMPACTED_NOTICE: &str = "Context compacted";

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
fn compaction_full_lines(compaction: &crate::app::Compaction, width: u16) -> Vec<Line<'static>> {
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

/// Expand tabs in a single code line to spaces **for display** (see
/// [`CODE_TAB_WIDTH`]). A tab is zero display columns, so tab-indented code
/// rendered verbatim would collapse flush-left; this substitutes a fixed run of
/// spaces (not tab-stop alignment) so the indentation survives, matching codex's
/// `expand_tabs`. Borrows unchanged in the common (tab-free) case. Applied only
/// on the render path — the stored message text keeps its tabs, so `/copy` is
/// byte-exact.
fn expand_code_tabs(line: &str) -> std::borrow::Cow<'_, str> {
    if line.contains('\t') {
        std::borrow::Cow::Owned(line.replace('\t', &" ".repeat(CODE_TAB_WIDTH)))
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

/// Hard-break a code line's **styled** segments into display rows of at most
/// `width` columns, preserving each run's style across the break — the verbatim,
/// whitespace-preserving counterpart of [`wrap_verbatim`] that keeps syntax
/// styling. Breaks on grapheme boundaries measured in display columns (an
/// overflowing cluster is placed alone); adjacent same-style graphemes coalesce
/// into one span. An empty line yields a single empty row (just the bullet/indent,
/// once stamped). Prefix-stable — appending only extends the last row.
fn code_content_rows(segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>> {
    let width = (width as usize).max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    let mut w = 0usize;
    let flush = |row: &mut Vec<Span<'static>>, run: &mut String, style: Style| {
        if !run.is_empty() {
            row.push(Span::styled(std::mem::take(run), style));
        }
    };
    for (text, style) in segments {
        for g in text.graphemes(true) {
            let gw = cols(g);
            if w > 0 && w + gw > width {
                flush(&mut row, &mut run, run_style);
                rows.push(std::mem::take(&mut row));
                w = 0;
            }
            if *style != run_style {
                flush(&mut row, &mut run, run_style);
                run_style = *style;
            }
            run.push_str(g);
            w += gw;
        }
    }
    flush(&mut row, &mut run, run_style);
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// Flatten a parsed inline tree ([`markdown::parse_inline`]) into styled text
/// segments under `base`. Emphasis adds a modifier, `code` a colour, a link its
/// text plus a ` (url)` suffix, an image its alt text. Nesting composes styles.
fn inline_spans(nodes: &[markdown::Inline], base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            markdown::Inline::Text(t) => out.push((t.clone(), base)),
            markdown::Inline::Bold(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::BOLD)));
            }
            markdown::Inline::Italic(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::ITALIC)));
            }
            markdown::Inline::Strike(inner) => {
                out.extend(inline_spans(
                    inner,
                    base.add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            markdown::Inline::Code(c) => out.push((c.clone(), base.fg(INLINE_CODE_COLOR))),
            markdown::Inline::Link { text, url } => {
                out.extend(inline_spans(text, base));
                out.push((
                    format!(" ({url})"),
                    base.fg(LINK_URL_COLOR).add_modifier(Modifier::UNDERLINED),
                ));
            }
            markdown::Inline::Image { alt } => out.push((alt.clone(), base)),
        }
    }
    out
}

/// Word-wrap styled inline `segments` to `width` columns, preserving each run's
/// style across wraps — the span-aware counterpart of [`wrap_text`]. Words (runs
/// of non-whitespace, which may span several styled pieces, e.g. `un**bold**`)
/// stay intact where they fit; a word wider than `width` hard-breaks on grapheme
/// boundaries; whitespace collapses to single base-styled spaces between words.
/// An empty line yields one empty row (matching [`wrap_text`]).
fn wrap_inline(segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>> {
    let words = tokenize_words(segments);
    let to_spans = |rows: Vec<Vec<(String, Style)>>| -> Vec<Vec<Span<'static>>> {
        rows.into_iter()
            .map(|r| r.into_iter().map(|(t, s)| Span::styled(t, s)).collect())
            .collect()
    };
    if width == 0 {
        // No wrapping: one row, words rejoined by single spaces.
        let mut row: Vec<(String, Style)> = Vec::new();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                push_piece(&mut row, " ", Style::default());
            }
            for (t, s) in word {
                push_piece(&mut row, t, *s);
            }
        }
        return to_spans(vec![row]);
    }
    let width = width as usize;
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let mut row: Vec<(String, Style)> = Vec::new();
    let mut row_w = 0usize;
    for word in &words {
        let ww: usize = word.iter().map(|(t, _)| cols(t)).sum();
        if ww > width {
            // A word too wide for any line: hard-break it grapheme by grapheme.
            if row_w > 0 {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            for (t, s) in word {
                for g in t.graphemes(true) {
                    let gw = cols(g);
                    if row_w > 0 && row_w + gw > width {
                        rows.push(std::mem::take(&mut row));
                        row_w = 0;
                    }
                    push_piece(&mut row, g, *s);
                    row_w += gw;
                }
            }
            continue;
        }
        let needed = if row_w == 0 { ww } else { row_w + 1 + ww };
        if needed > width {
            rows.push(std::mem::take(&mut row));
            row_w = 0;
        } else if row_w > 0 {
            push_piece(&mut row, " ", Style::default());
            row_w += 1;
        }
        for (t, s) in word {
            push_piece(&mut row, t, *s);
        }
        row_w += ww;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    to_spans(rows)
}

/// Split styled `segments` into words — each a run of non-whitespace pieces
/// (`Vec<(text, style)>`) that may cross piece/style boundaries — dropping the
/// whitespace between them ([`wrap_inline`] re-inserts single spaces).
fn tokenize_words(segments: &[(String, Style)]) -> Vec<Vec<(String, Style)>> {
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    let mut word: Vec<(String, Style)> = Vec::new();
    let mut piece = String::new();
    let mut piece_style = Style::default();
    for (text, style) in segments {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !piece.is_empty() {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                if !piece.is_empty() && *style != piece_style {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                piece_style = *style;
                piece.push(ch);
            }
        }
    }
    if !piece.is_empty() {
        word.push((piece, piece_style));
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Append `text` to a row of styled pieces, coalescing with the last piece when
/// the style matches so a run of same-styled graphemes stays one span.
fn push_piece(row: &mut Vec<(String, Style)>, text: &str, style: Style) {
    if let Some((last_text, last_style)) = row.last_mut()
        && *last_style == style
    {
        last_text.push_str(text);
        return;
    }
    row.push((text.to_string(), style));
}

/// Normalise a raw table `line` to exactly `ncols` styled cells under `base`
/// (bold for a header, plain for data): pad missing cells empty, drop extras,
/// each inline-parsed like prose so `` `code` `` / `**bold**` render styled.
fn normalize_row(line: &str, ncols: usize, base: Style) -> Vec<Vec<(String, Style)>> {
    let cells = markdown::table_cells(line);
    (0..ncols)
        .map(|i| table_cell_segments(cells.get(i).map_or("", String::as_str), base))
        .collect()
}

/// The per-column natural (unwrapped) width — the widest *rendered* cell over
/// `rows` (each already `ncols` styled cells), floored at 1.
fn natural_col_widths(rows: &[Vec<Vec<(String, Style)>>], ncols: usize) -> Vec<usize> {
    let mut w = vec![1usize; ncols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            w[i] = w[i].max(segments_cols(cell));
        }
    }
    w
}

/// The per-column **word** width — the widest single unbreakable word over
/// `rows`, words tokenized exactly as [`wrap_inline`] will break them. A column
/// allocated at least this many display columns wraps only at spaces; below it,
/// some word has to hard-break mid-token. [`allocate_column_widths`] seats every
/// column here first, which is what keeps a 33-column IPv6 (or a bare
/// `OPENROUTER_API_KEY`) whole while a column that *can* wrap gives way.
fn natural_word_widths(rows: &[Vec<Vec<(String, Style)>>], ncols: usize) -> Vec<usize> {
    let mut w = vec![1usize; ncols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            for word in tokenize_words(cell) {
                w[i] = w[i].max(segments_cols(&word));
            }
        }
    }
    w
}

/// Allocate the per-column widths for a grid that must fit `avail` display
/// columns, given each column's `natural` (widest cell) width and `word` width
/// (its widest single unbreakable word — [`natural_word_widths`]).
///
/// Three cases, in order — Claude Code's fit (docs/table-streaming.md):
///
/// 1. **The natural grid fits.** Return it: the table stays as narrow as its
///    content rather than stretching to the terminal.
/// 2. **It overflows but every column's word-safe floor fits.** Each column is
///    seated at `min(natural, word)` — the width it needs to wrap at *spaces*
///    only — and the surplus is split in proportion to each column's **unmet
///    demand** (`natural - floor`), largest-remainder so the columns fill the
///    budget exactly. This is what makes a table read like Claude Code's: a
///    column whose width is driven by one long outlier (`cargo clippy
///    --all-targets -- -D warnings`) doesn't hoard room every other row wastes,
///    and the column with the genuinely long content gets it instead. It also
///    keeps an unbreakable token (a 33-column IPv6) whole whenever another
///    column can give way at a space.
/// 3. **Even the floors overflow.** Some token has to hard-break, so level the
///    **widest column** down one display column at a time (ties leftmost-first),
///    floored at [`TABLE_MIN_COL`]: a short cell (`facebook.com`, a `Packets`
///    header) keeps its natural width for as long as possible. A proportional
///    shrink here would starve *every* column at once, breaking short words
///    mid-cell (an earlier bug).
///
/// Allocated widths are **wrap** widths throughout, so cells word-wrap into
/// them (taller rows) rather than truncating with a `…`.
fn allocate_column_widths(natural: &[usize], word: &[usize], avail: usize) -> Vec<usize> {
    let n = natural.len();
    if n == 0 {
        return Vec::new();
    }
    let overhead = 3 * n + 1; // `│`×(n+1) plus two pad spaces × n
    let content_avail = avail.saturating_sub(overhead);
    let total: usize = natural.iter().sum();
    if total == 0 || total <= content_avail {
        return natural.to_vec();
    }
    // The width each column needs to wrap at spaces only — never more than its
    // natural width, never below the floor a bordered cell needs to be legible.
    let floor: Vec<usize> = natural
        .iter()
        .zip(word)
        .map(|(&nat, &w)| nat.min(w.max(TABLE_MIN_COL)))
        .collect();
    let seated: usize = floor.iter().sum();
    if seated > content_avail {
        return level_widest_columns(natural, content_avail);
    }
    // Spend the surplus where the content still doesn't fit. `demand_total`
    // exceeds `surplus` (the naturals overflow by definition), so no column can
    // be handed more than it asked for.
    let surplus = content_avail - seated;
    let demand: Vec<usize> = natural.iter().zip(&floor).map(|(&n, &f)| n - f).collect();
    let demand_total: usize = demand.iter().sum();
    if demand_total == 0 {
        return floor;
    }
    let mut w = floor;
    let mut spent = 0usize;
    // (remainder, index) — largest-remainder rounding hands out the columns
    // integer division dropped, so the grid fills `content_avail` exactly.
    let mut remainders: Vec<(usize, usize)> = Vec::with_capacity(n);
    for (i, &d) in demand.iter().enumerate() {
        let exact = surplus * d;
        w[i] += exact / demand_total;
        spent += exact / demand_total;
        remainders.push((exact % demand_total, i));
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = surplus - spent;
    for &(_, i) in &remainders {
        if left == 0 {
            break;
        }
        if w[i] < natural[i] {
            w[i] += 1;
            left -= 1;
        }
    }
    w
}

/// Case 3 of [`allocate_column_widths`]: shrink the **widest** column one
/// display column at a time (ties leftmost-first, so equal columns level
/// evenly) until the grid fits `content_avail`, never below
/// [`TABLE_MIN_COL`].
fn level_widest_columns(natural: &[usize], content_avail: usize) -> Vec<usize> {
    // Pre-clamp to the whole content budget (a lone column can never usefully
    // exceed it), bounding the loop to O(content_avail · n) even for a
    // pathological megabyte-wide cell — the preview re-renders per frame.
    let mut w: Vec<usize> = natural
        .iter()
        .map(|&x| x.min(content_avail.max(1)).max(1))
        .collect();
    let mut sum: usize = w.iter().sum();
    while sum > content_avail {
        // The first of the widest columns still above the floor.
        let mut widest: Option<usize> = None;
        for (i, &x) in w.iter().enumerate() {
            if x > TABLE_MIN_COL && widest.is_none_or(|b| x > w[b]) {
                widest = Some(i);
            }
        }
        let Some(i) = widest else {
            break; // every column already at the floor — unavoidable at tiny widths
        };
        w[i] -= 1;
        sum -= 1;
    }
    w
}

/// A table border row: `left` + per-column `─`×(w+2) joined by `mid`, then
/// `right` — e.g. `┌──┬──┐` / `├──┼──┤` / `└──┴──┘`, rendered dim.
fn table_border_row(col_w: &[usize], left: char, mid: char, right: char) -> Vec<Span<'static>> {
    let mut s = String::new();
    s.push(left);
    for (i, &w) in col_w.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        s.extend(std::iter::repeat_n('─', w + 2));
    }
    s.push(right);
    vec![Span::styled(s, Style::new().fg(TABLE_BORDER_COLOR))]
}

/// Render one table row (its per-column styled `cells`) into box-drawing content
/// rows, each cell **word-wrapped** into its column width via [`wrap_inline`]
/// (which hard-breaks an over-wide token grapheme-by-grapheme, like codex/img2).
/// Returns as many rows as the tallest wrapped cell — so a data row spans several
/// rows when its cells wrap — each `│`-framed with single-space margins, its
/// styled segments padded/aligned per column.
///
/// A cell shorter than the row is **centred vertically** in it (Claude Code's
/// look): beside a neighbour that wrapped to three rows, a one-line cell sits on
/// the middle row rather than the top, so the two read as one record. The
/// leading blank rows round down, so a one-line cell in a two-row row stays on
/// top (`(height - lines) / 2`, matching how [`pad_cell_line`] rounds its
/// horizontal centring).
fn table_row_lines(
    cells: &[Vec<(String, Style)>],
    col_w: &[usize],
    aligns: &[markdown::Alignment],
) -> Vec<Vec<Span<'static>>> {
    let border = Style::new().fg(TABLE_BORDER_COLOR);
    let wrapped: Vec<Vec<Vec<Span<'static>>>> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| wrap_inline(cell, col_w[i] as u16))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..height)
        .map(|r| {
            let mut spans = vec![Span::styled("│".to_string(), border)];
            for (i, cell_rows) in wrapped.iter().enumerate() {
                spans.push(Span::raw(" "));
                // Where this cell's own rows start, so a short cell is centred
                // in the row instead of hugging its top.
                let top = height.saturating_sub(cell_rows.len()) / 2;
                let line = r
                    .checked_sub(top)
                    .and_then(|k| cell_rows.get(k))
                    .cloned()
                    .unwrap_or_default();
                spans.extend(pad_cell_line(line, col_w[i], aligns[i]));
                spans.push(Span::raw(" "));
                spans.push(Span::styled("│".to_string(), border));
            }
            spans
        })
        .collect()
}

/// Pad a wrapped cell sub-line's `spans` to `w` columns per `align` — no
/// truncation, since [`wrap_inline`] already fit it to `w`. Padding is
/// default-styled (an invisible space).
fn pad_cell_line(
    spans: Vec<Span<'static>>,
    w: usize,
    align: markdown::Alignment,
) -> Vec<Span<'static>> {
    let body_w: usize = spans.iter().map(|s| cols(&s.content)).sum();
    let pad = w.saturating_sub(body_w);
    let (left, right) = match align {
        markdown::Alignment::Right => (pad, 0),
        markdown::Alignment::Center => (pad / 2, pad - pad / 2),
        markdown::Alignment::Left | markdown::Alignment::None => (0, pad),
    };
    let mut out = Vec::with_capacity(spans.len() + 2);
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(spans);
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
    out
}

/// The per-column widths for a table from its `header` and **every** data row,
/// fit to `content_width` — the widths each cell then wraps into. Computed only
/// when the block closes (the rows are buffered until then), so a wide later
/// row can never be shattered by widths guessed from an earlier one — the
/// failure of the old first-data-row lock (docs/table-streaming.md).
fn table_column_widths(
    header: &str,
    rows: &[String],
    ncols: usize,
    content_width: u16,
) -> Vec<usize> {
    let mut all = vec![normalize_row(header, ncols, table_header_style())];
    all.extend(
        rows.iter()
            .map(|r| normalize_row(r, ncols, Style::default())),
    );
    allocate_column_widths(
        &natural_col_widths(&all, ncols),
        &natural_word_widths(&all, ncols),
        content_width as usize,
    )
}

/// The opening rows of a table: the top border, the (word-wrapped, bold) header
/// row, and the header/body separator.
fn table_open_rows(
    header: &str,
    col_w: &[usize],
    aligns: &[markdown::Alignment],
) -> Vec<Vec<Span<'static>>> {
    let header_style = Style::new().add_modifier(Modifier::BOLD);
    let header_cells = normalize_row(header, aligns.len(), header_style);
    let header_aligns: Vec<markdown::Alignment> =
        aligns.iter().copied().map(header_align).collect();
    let mut out = vec![table_border_row(col_w, '┌', '┬', '┐')];
    out.extend(table_row_lines(&header_cells, col_w, &header_aligns));
    out.push(table_border_row(col_w, '├', '┼', '┤'));
    out
}

/// How a **header** cell aligns given the column's delimiter `align`: centred
/// when the delimiter declared nothing (Claude Code's look — a centred label
/// over left-aligned data reads as a column heading rather than a first row),
/// otherwise the alignment the author actually asked for. Only the default
/// changes; a declared `:--`/`:-:`/`--:` still wins, since a markdown renderer
/// must not discard stated intent.
fn header_align(align: markdown::Alignment) -> markdown::Alignment {
    match align {
        markdown::Alignment::None => markdown::Alignment::Center,
        declared => declared,
    }
}

/// The bold header style shared by grid header cells and record labels.
fn table_header_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Split `line` into exactly `ncols` raw cell texts (padding/truncating), for the
/// records fallback's stored labels.
fn normalize_raw_cells(line: &str, ncols: usize) -> Vec<String> {
    let mut cells = markdown::table_cells(line);
    cells.resize(ncols, String::new());
    cells
}

/// Decide, at the block's close, whether the grid is too cramped to scan and
/// should render as vertical key/value **records** instead (codex's key/value
/// transpose). Made from the header and **every** buffered row — the same full
/// knowledge the widths use (docs/table-streaming.md). True when a column is
/// both **narrow** (`< TABLE_SCANNABLE_COL`) **and** holds content that wraps
/// into `>= TABLE_RECORDS_MIN_LINES` rows at its allocated width — i.e. the
/// grid is growing tall because columns are starved, not because one wide cell
/// is a legitimately long narrative. Never for a single-column table (it's
/// just a list).
fn table_should_use_records(header: &str, rows: &[String], col_w: &[usize]) -> bool {
    let ncols = col_w.len();
    if ncols < 2 {
        return false;
    }
    let all: Vec<Vec<Vec<(String, Style)>>> =
        std::iter::once(normalize_row(header, ncols, table_header_style()))
            .chain(
                rows.iter()
                    .map(|r| normalize_row(r, ncols, Style::default())),
            )
            .collect();
    col_w.iter().enumerate().any(|(i, &w)| {
        w < TABLE_SCANNABLE_COL
            && all
                .iter()
                .any(|cells| wrap_inline(&cells[i], w as u16).len() >= TABLE_RECORDS_MIN_LINES)
    })
}

/// A `─` rule separating two records, dim like a table border. Capped at
/// [`TABLE_RECORD_SEPARATOR_WIDTH`] (shrinking to fit a narrower `width`) rather
/// than spanning the full content width — Claude Code's cleaner short separator.
fn table_record_separator(width: usize) -> Vec<Span<'static>> {
    let w = width.clamp(1, TABLE_RECORD_SEPARATOR_WIDTH);
    vec![Span::styled(
        "─".repeat(w),
        Style::new().fg(TABLE_BORDER_COLOR),
    )]
}

/// Render one data row as a vertical **key/value record** block (Claude Code's
/// key/value transpose): for each column a `label: value` field — the bold label,
/// a `: ` separator, then the value inline-parsed and wrapped into the remaining
/// width, continuation lines aligned under the value. No aligned label column and
/// no trailing padding, so it reads clean and compact. When even `label: ` plus a
/// minimum value can't fit (`content_width` too small), the field **stacks**: the
/// label (with its colon) on its own line, the value wrapped and indented beneath.
/// No box drawing, nothing truncated.
fn table_record_block(labels: &[String], row: &str, content_width: u16) -> Vec<Vec<Span<'static>>> {
    let ncols = labels.len();
    let row_cells = normalize_row(row, ncols, Style::default());
    let content_width = content_width as usize;
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    for (label, value) in labels.iter().zip(&row_cells) {
        let label_segs = table_cell_segments(label, table_header_style());
        // The `label: ` prefix — bold label, colon, single space.
        let prefix_w = segments_cols(&label_segs) + cols(": ");
        if prefix_w + TABLE_RECORD_MIN_VALUE <= content_width {
            let value_width = content_width.saturating_sub(prefix_w).max(1);
            for (k, vrow) in wrap_inline(value, value_width as u16)
                .into_iter()
                .enumerate()
            {
                let mut spans: Vec<Span<'static>> = Vec::new();
                if k == 0 {
                    spans.extend(label_segs.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
                    spans.push(Span::raw(": "));
                } else {
                    spans.push(Span::raw(" ".repeat(prefix_w)));
                }
                spans.extend(vrow);
                out.push(spans);
            }
        } else {
            // Stacked: the label (with colon) on its own line, value indented beneath.
            let mut head: Vec<Span<'static>> = label_segs
                .iter()
                .map(|(t, s)| Span::styled(t.clone(), *s))
                .collect();
            head.push(Span::raw(":"));
            out.push(head);
            let value_width = content_width
                .saturating_sub(TABLE_RECORD_STACK_INDENT)
                .max(1);
            for vrow in wrap_inline(value, value_width as u16) {
                let mut spans = vec![Span::raw(" ".repeat(TABLE_RECORD_STACK_INDENT))];
                spans.extend(vrow);
                out.push(spans);
            }
        }
    }
    out
}

/// Re-join a hard-wrapped table row (docs/table-streaming.md). A model echoing
/// terminal-wrapped source can carry a line break MID-ROW, so the row arrives
/// as a leading-pipe line plus a fragment (`| google | … | 86.8 / 89.8 /` ␤
/// `96.4 ms |`). Strict GFM reads the fragment as a row of its own — a phantom
/// one-cell row — but in a leading-pipe table a genuine row *starts* with `|`;
/// a pipe-carrying line that doesn't is really the previous row's tail. Returns
/// the space-joined row when `prev` is a leading-pipe row, `line` isn't, and
/// the merged row still fits the delimiter's `ncols` (a fragment that would
/// overflow the column count is a real — if style-mixed — row, e.g. a no-pipe
/// `c | d` after a complete `| a | b |`; GFM keeps it a row and so do we).
fn join_wrapped_table_row(prev: &str, line: &str, ncols: usize) -> Option<String> {
    if !prev.trim_start().starts_with('|') || line.trim_start().starts_with('|') {
        return None;
    }
    let joined = format!("{} {}", prev.trim_end(), line.trim_start());
    (markdown::table_cells(&joined).len() <= ncols).then_some(joined)
}

/// Render a complete GFM table block — the `header`, the delimiter's `aligns`,
/// and the buffered data `rows` — into content rows (`docs/markdown.md`).
/// Column widths are allocated from the header **and every data row** (fit to
/// `content_width`), so the grid always fits its real content — the same full
/// knowledge then decides grid vs. key/value records. THE table renderer:
/// the batch path, the streaming close/`flush`, and the forming-table preview
/// all emit a table only through here, so they can never disagree
/// (docs/table-streaming.md).
fn table_block_rows(
    header: &str,
    aligns: &[markdown::Alignment],
    rows: &[String],
    content_width: u16,
) -> Vec<Vec<Span<'static>>> {
    let ncols = aligns.len();
    let col_w = table_column_widths(header, rows, ncols, content_width);
    if let Some(first) = rows.first()
        && table_should_use_records(header, rows, &col_w)
    {
        let labels = normalize_raw_cells(header, ncols);
        let mut out = table_record_block(&labels, first, content_width);
        for row in &rows[1..] {
            out.push(table_record_separator(content_width as usize));
            out.extend(table_record_block(&labels, row, content_width));
        }
        return out;
    }
    let mut out = table_open_rows(header, &col_w, aligns);
    for (i, row) in rows.iter().enumerate() {
        // Claude Code's full grid: every data row is framed — a `├──┼──┤` rule
        // between consecutive rows, not just under the header.
        if i > 0 {
            out.push(table_border_row(&col_w, '├', '┼', '┤'));
        }
        out.extend(table_row_lines(
            &normalize_row(row, ncols, Style::default()),
            &col_w,
            aligns,
        ));
    }
    out.push(table_border_row(&col_w, '└', '┴', '┘'));
    out
}

/// [`table_block_rows`] over raw table `lines` (`lines[0]` the header,
/// `lines[1]` the delimiter, `lines[2..]` the data rows) — a test-only
/// convenience; production buffers a table's lines in [`AssistantRenderer`]
/// and renders through [`table_block_rows`] when the block closes.
#[cfg(test)]
fn table_content_rows(lines: &[String], width: u16) -> Vec<Vec<Span<'static>>> {
    let Some(aligns) = lines.get(1).and_then(|l| markdown::table_delimiter(l)) else {
        return Vec::new(); // not a confirmed table (never reached in practice)
    };
    if aligns.is_empty() {
        return Vec::new();
    }
    let header = lines.first().map_or("", String::as_str);
    table_block_rows(header, &aligns, lines.get(2..).unwrap_or_default(), width)
}

/// A table cell's markdown inline-parsed into styled segments (markers removed),
/// under `base` (bold for a header cell) — the rendered counterpart of the raw
/// cell text, used both to size columns and to draw them.
fn table_cell_segments(cell: &str, base: Style) -> Vec<(String, Style)> {
    inline_spans(&markdown::parse_inline(cell), base)
}

/// Total display width of a cell's styled `segments` (the *rendered* width, so a
/// column sizes to `foo.db`, not `` `foo.db` ``).
fn segments_cols(segments: &[(String, Style)]) -> usize {
    segments.iter().map(|(t, _)| cols(t)).sum()
}

/// Build an assistant reply's lines, markdown-aware (`docs/markdown.md`):
/// [`markdown::parse_blocks`] splits prose from fenced code; prose word-wraps via
/// [`wrap_text`] (ATX headings keep their `#`s and style per level, codex-style)
/// while **code
/// blocks render verbatim** — each source line kept byte-for-byte,
/// **syntax-highlighted** ([`highlight::highlight`]) and hard-broken only on width
/// via [`code_content_rows`], with both fences (and the language info-string)
/// hidden. The bullet lands on row 0 and `INDENT` on the rest, exactly like the
/// plain path — so fence/heading-free text is byte-identical to before.
///
/// **Prefix-stable at the line level:** a line's prose/code mode and its highlight
/// are fixed by the text before it. (Highlighting uses one-char lookahead within a
/// line — a call's `(` — so an in-progress code *line* isn't safe to commit until
/// it completes; [`StreamRender`] withholds it, see there.)
fn assistant_lines(text: &str, width: u16, bullet: &str, color: Color) -> Vec<Line<'static>> {
    let mut renderer = AssistantRenderer::new(width, bullet, color);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in text.split('\n') {
        rows.extend(renderer.feed_line(line));
    }
    // Flush a trailing table (one the reply ended on, with no closing line) — its
    // rows were buffered pending a close that never came. The streaming
    // `StreamRender::finish` flushes the same way, so the two never disagree.
    rows.extend(renderer.flush());
    // Trim trailing blank rows (a model's `…\n\n` before a tool call, say): the
    // caller adds exactly one spacer, so trailing blanks would stack. Skipped
    // when the reply ends inside an open code fence — its blank lines are
    // content. The streaming [`StreamRender`] trims the same way, so the two
    // never disagree.
    if !renderer.in_code() {
        while rows.last().is_some_and(row_is_blank) {
            rows.pop();
        }
    }
    // A reply that renders to zero rows still gets one bullet row so the bullet
    // always has a home — an empty reply, or (now that fences render nothing) a
    // reply that is only a code fence. The streaming [`StreamRender`] applies the
    // same fallback, so the two never disagree on such a reply.
    if rows.is_empty() {
        rows.push(empty_assistant_row(bullet, color));
    }
    rows
}

/// The lone bullet row an assistant message falls back to when its body renders
/// to **zero** rows (an empty reply, or one that is only a hidden code fence), so
/// the role bullet always has a home. Shared by the batch [`assistant_lines`] and
/// the streaming [`StreamRender`] so they agree on such a reply.
fn empty_assistant_row(bullet: &str, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        bullet.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )])
}

/// Whether a rendered row is visually blank — every span is whitespace (an
/// indent-only continuation row for an empty source line). Used to trim a
/// message's **trailing** blank rows so a model's `…\n\n` before a tool call
/// doesn't stack blank rows on top of the caller's single spacer (the
/// 3-newline bug). The bullet-home fallback row (`● `) is *not* blank, so it is
/// never trimmed. Shared by [`assistant_lines`] (repaint) and [`StreamRender`]
/// (live) so they agree.
fn row_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// The incremental, prefix-stable core shared by the batch [`assistant_lines`]
/// and the streaming [`StreamRender`]: feed an assistant reply's source lines in
/// order with [`AssistantRenderer::feed_line`] and each returns that line's
/// finished rows. Fence state ([`markdown::BlockScanner`]) and highlight carry
/// ([`highlight::Highlighter`]) thread across the calls, so a completed line's
/// rows never change — which is what lets scrollback commits and the strip
/// preview cost O(new line) instead of O(whole reply). See `docs/markdown.md`.
///
/// The very first row emitted across the whole message carries the coloured
/// role bullet; every later row is indented under it. `Clone` lets [`StreamRender`]
/// *peek* an in-progress (not yet newline-terminated) line without advancing the
/// state it will resume from.
#[derive(Clone)]
struct AssistantRenderer {
    /// Columns available to prose (and code) after the bullet.
    content_width: u16,
    /// The role bullet stamped on the first row.
    bullet: String,
    /// The bullet's colour.
    color: Color,
    /// Fence state entering the next line.
    scanner: markdown::BlockScanner,
    /// The open code block's highlighter (`None` outside a fence).
    highlighter: Option<highlight::Highlighter>,
    /// Whether any row has been emitted yet (the first gets the bullet).
    emitted_any: bool,
    /// Whether the previous source line was blank — the gate that lets a `-` rule
    /// (`---`, ambiguous with a setext underline) render as an em-dash break.
    prev_blank: bool,
    /// In-progress GFM table accumulation (`docs/markdown.md`). A table is not
    /// prefix-stable — a later row can widen a column — so its source lines are
    /// buffered here and rendered **whole** only when the block closes (a
    /// non-table line, a code fence, or end-of-message via [`Self::flush`]).
    table: TableState,
}

/// The [`AssistantRenderer`]'s GFM-table state machine (docs/table-streaming.md).
/// A candidate header is held in `PendingHeader` until the next line confirms it
/// (a matching [`markdown::table_delimiter`]) — the one-line lookahead a pipe
/// table needs — then `Buffering` accumulates the data rows, emitting **nothing**,
/// until a non-table line (or a code fence, or end-of-message) closes the block
/// and it renders whole through [`table_block_rows`], its column widths fit to
/// **every** row. Nothing of an open table ever commits to scrollback, so a wide
/// later row can never invalidate a committed one (prefix-stability is trivial);
/// the streaming strip previews the forming block instead ([`StreamRender::preview`]).
#[derive(Clone)]
enum TableState {
    /// Not inside a table.
    None,
    /// A row that *looks* like a table header, buffered pending its delimiter.
    PendingHeader(String),
    /// Header + delimiter confirmed; the data rows accumulate here (raw source
    /// lines) until the block closes. `aligns` are the delimiter's alignments —
    /// their count is the column count.
    Buffering {
        header: String,
        aligns: Vec<markdown::Alignment>,
        rows: Vec<String>,
    },
}

impl AssistantRenderer {
    fn new(width: u16, bullet: &str, color: Color) -> Self {
        Self {
            content_width: width.saturating_sub(BULLET_WIDTH).max(1),
            bullet: bullet.to_string(),
            color,
            scanner: markdown::BlockScanner::new(),
            highlighter: None,
            emitted_any: false,
            prev_blank: true, // start-of-message is a blank boundary
            table: TableState::None,
        }
    }

    /// Render the next source `line` into its finished rows, advancing fence +
    /// highlight state. The first row ever emitted carries the bullet; the rest
    /// are indented under it.
    fn feed_line(&mut self, line: &str) -> Vec<Line<'static>> {
        let rows = self.content_rows(line);
        self.stamp(rows)
    }

    /// The row *content* spans for `line` (no bullet/indent prefix yet),
    /// advancing fence/highlight state. Prose word-wraps (headings keep their
    /// `#`s and style per level, codex-style); code renders verbatim,
    /// syntax-highlighted; both the opening and
    /// closing fence render nothing (no gutter, and the language info-string is
    /// not shown).
    fn content_rows(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        // Track blank-line boundaries for the `-` thematic-break gate below. This
        // mirrors the scanner's own `prev_blank` (used for indented code) but is
        // kept here because the rule decision lives in the prose branch, like
        // headings.
        let was_blank = self.prev_blank;
        self.prev_blank = line.trim().is_empty();
        match self.scanner.classify(line) {
            markdown::LineKind::CodeStart(lang) => {
                // A fence can't sit inside a table, so it closes any in-progress
                // one first; then it opens the block silently (primes highlighting
                // for the info-string language, emits no row — no gutter, no label).
                let out = self.flush_table();
                self.highlighter = Some(highlight::Highlighter::new(lang.as_deref()));
                out
            }
            markdown::LineKind::CodeEnd => {
                self.highlighter = None;
                Vec::new()
            }
            // A code line never arrives with a table open (a fence flushed it, or
            // the blank before indented code did), so no flush is needed here.
            markdown::LineKind::Code => self.render_code_line(line),
            markdown::LineKind::Prose => self.prose_or_table(line, was_blank),
        }
    }

    /// Render a fenced/indented **code** line: tabs expanded so indentation
    /// survives, syntax-highlighted by the open fence's language (plain for an
    /// indented block), hard-broken on width.
    fn render_code_line(&mut self, line: &str) -> Vec<Vec<Span<'static>>> {
        let expanded = expand_code_tabs(line);
        let segs = match self.highlighter.as_mut() {
            Some(h) => h.line(&expanded),
            None => vec![highlight::Seg {
                text: expanded.into_owned(),
                style: highlight::plain_style(),
            }],
        };
        let styled: Vec<(String, Style)> = segs.into_iter().map(|s| (s.text, s.style)).collect();
        code_content_rows(&styled, self.content_width)
    }

    /// Render a **non-table** prose line: an ATX heading (markers kept, styled per
    /// level — codex parity), a thematic break (`———`), or word-wrapped plain
    /// prose. Pure per-line, so it stays prefix-stable.
    fn render_prose_line(&self, line: &str, was_blank: bool) -> Vec<Vec<Span<'static>>> {
        if let Some((level, htext)) = markdown::heading_level(line) {
            // Codex keeps the `#` markers visible (`"#".repeat(level)`) and styles
            // the whole line per level — no colour, just modifiers. Normalise the
            // marker run + a single space, then word-wrap the reconstructed heading.
            let style = heading_style(level);
            let hashes = "#".repeat(level as usize);
            let content = if htext.is_empty() {
                hashes
            } else {
                format!("{hashes} {htext}")
            };
            wrap_text(&content, self.content_width)
                .into_iter()
                .map(|l| vec![Span::styled(l, style)])
                .collect()
        } else if is_thematic_break(line, was_blank) {
            // Codex renders `---`/`***`/`___` as an unstyled `———` rule on its own
            // row (`Event::Rule`). A single settled row, so prefix-stable. Checked
            // before lists so `- - -` / `* * *` stay rules, not bullets.
            vec![vec![Span::raw(THEMATIC_BREAK.to_string())]]
        } else if let Some(inner) = markdown::block_quote(line) {
            self.render_block_quote(inner)
        } else if let Some(item) = markdown::list_item(line) {
            self.render_list_item(&item)
        } else {
            // Plain prose: parse inline `**bold**`/`*italic*`/`~~strike~~`/
            // `` `code` ``/`[link](url)` into styled spans, then span-preserving
            // word-wrap. Emphasis is line-local, so a complete line's styling is
            // final; a trailing line with an open marker is withheld by
            // `StreamRender` (`markdown::has_open_inline`).
            let segs = inline_spans(&markdown::parse_inline(line), Style::default());
            wrap_inline(&segs, self.content_width)
        }
    }

    /// Render a list item (`docs/markdown.md`): the nesting indent, then the
    /// marker (`-` for bullets, an accent-coloured `N.` for ordered items), then
    /// the inline-parsed content span-wrapped with a **hanging indent** — every
    /// continuation row aligns under the text, not the marker. Line-local, so
    /// prefix-stable.
    fn render_list_item(&self, item: &markdown::ListItem) -> Vec<Vec<Span<'static>>> {
        let (marker_text, marker_style) = match item.marker {
            markdown::ListMarker::Bullet => ("- ".to_string(), Style::default()),
            markdown::ListMarker::Ordered(n, delim) => {
                (format!("{n}{delim} "), Style::new().fg(LIST_MARKER_COLOR))
            }
        };
        let hang = item.indent + cols(&marker_text);
        let text_w = (self.content_width as usize).saturating_sub(hang).max(1) as u16;
        let text_rows = wrap_inline(
            &inline_spans(&markdown::parse_inline(item.content), Style::default()),
            text_w,
        );
        text_rows
            .into_iter()
            .enumerate()
            .map(|(i, mut text)| {
                let mut row = Vec::new();
                if i == 0 {
                    if item.indent > 0 {
                        row.push(Span::raw(" ".repeat(item.indent)));
                    }
                    row.push(Span::styled(marker_text.clone(), marker_style));
                } else {
                    row.push(Span::raw(" ".repeat(hang))); // hang under the text
                }
                row.append(&mut text);
                row
            })
            .collect()
    }

    /// Render a blockquote (`docs/markdown.md`): a dim `> ` marker on every
    /// wrapped row, the inline-parsed text dim. Line-local, so prefix-stable.
    fn render_block_quote(&self, inner: &str) -> Vec<Vec<Span<'static>>> {
        let marker = "> ";
        let text_w = (self.content_width as usize)
            .saturating_sub(cols(marker))
            .max(1) as u16;
        let base = Style::new().fg(QUOTE_COLOR);
        let text_rows = wrap_inline(&inline_spans(&markdown::parse_inline(inner), base), text_w);
        text_rows
            .into_iter()
            .map(|mut text| {
                let mut row = vec![Span::styled(marker.to_string(), base)];
                row.append(&mut text);
                row
            })
            .collect()
    }

    /// Run the GFM-table state machine for a prose `line`
    /// (docs/table-streaming.md): buffer a candidate header, confirm it against
    /// the next line's delimiter, then **buffer every data row** — a non-table
    /// line closes the block, rendering it whole (column widths fit to every
    /// row) and recursing to render that closing line in place. Returns the
    /// rows to emit now (empty while the table accumulates — the streaming
    /// strip previews the forming block instead, [`StreamRender::preview`]).
    fn prose_or_table(&mut self, line: &str, was_blank: bool) -> Vec<Vec<Span<'static>>> {
        match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => {
                if markdown::is_table_row(line) {
                    self.table = TableState::PendingHeader(line.to_string());
                    Vec::new()
                } else {
                    self.render_prose_line(line, was_blank)
                }
            }
            TableState::PendingHeader(header) => {
                let ncols = markdown::table_cells(&header).len();
                if let Some(aligns) = markdown::table_delimiter(line).filter(|a| a.len() == ncols) {
                    // Header + a matching delimiter → confirmed; buffer the data
                    // rows until the block closes (the widths need them all).
                    self.table = TableState::Buffering {
                        header,
                        aligns,
                        rows: Vec::new(),
                    };
                    Vec::new()
                } else {
                    // Not a table — the buffered header was ordinary prose (a line
                    // with pipes is never a heading or rule, so `was_blank` is moot),
                    // then process the current line (it may start a fresh table).
                    let mut out = self.render_prose_line(&header, false);
                    out.extend(self.prose_or_table(line, was_blank));
                    out
                }
            }
            TableState::Buffering {
                header,
                aligns,
                mut rows,
            } => {
                if markdown::is_table_row(line) {
                    // A pipe-carrying line that doesn't start with `|`, inside a
                    // leading-pipe table, is the previous row's hard-wrapped tail
                    // — re-join it instead of minting a phantom one-cell row.
                    match rows
                        .last()
                        .and_then(|prev| join_wrapped_table_row(prev, line, aligns.len()))
                    {
                        Some(joined) => {
                            rows.pop();
                            rows.push(joined);
                        }
                        None => rows.push(line.to_string()),
                    }
                    self.table = TableState::Buffering {
                        header,
                        aligns,
                        rows,
                    };
                    Vec::new()
                } else {
                    // A non-table line closes the block: render it whole (grid or
                    // records, widths from every row), then the closing line.
                    let mut out = table_block_rows(&header, &aligns, &rows, self.content_width);
                    out.extend(self.prose_or_table(line, was_blank));
                    out
                }
            }
        }
    }

    /// Emit any open table's rows, clearing the state — called when a code
    /// fence interrupts a table (and by [`Self::flush`] at end-of-message).
    /// A never-confirmed `PendingHeader` renders as the plain prose line it
    /// actually was; a `Buffering` table renders whole (a header-only table is
    /// the opening + bottom border).
    fn flush_table(&mut self) -> Vec<Vec<Span<'static>>> {
        match std::mem::replace(&mut self.table, TableState::None) {
            TableState::None => Vec::new(),
            TableState::PendingHeader(header) => self.render_prose_line(&header, false),
            TableState::Buffering {
                header,
                aligns,
                rows,
            } => table_block_rows(&header, &aligns, &rows, self.content_width),
        }
    }

    /// Flush any buffered table at end-of-message, stamping bullet/indent. Both
    /// the batch [`assistant_lines`] and the streaming [`StreamRender::finish`]
    /// call this, so a trailing table (one with no closing line) still renders.
    fn flush(&mut self) -> Vec<Line<'static>> {
        let rows = self.flush_table();
        self.stamp(rows)
    }

    /// Whether the **next** line to be fed sits inside an open fenced code block.
    /// Such a line's rows aren't safe to commit until it completes: the
    /// highlighter's within-line lookahead (a call's `(`, a `//` comment, a
    /// closing `*/`) can recolour an *earlier* wrapped row of the same line. The
    /// streaming committer uses this to withhold an in-progress code line whole.
    fn in_code(&self) -> bool {
        self.highlighter.is_some()
    }

    /// Whether a GFM table block is open (`PendingHeader`/`Buffering`) — the
    /// phase that emits **no rows**: the block renders whole only when it
    /// closes, so [`StreamRender::commit`] withholds the trailing line while
    /// this holds (like [`Self::in_code`]) and [`StreamRender::preview`] shows
    /// the forming block instead (docs/table-streaming.md). It also gates
    /// feeding an *empty* trailing line into a clone — that would close the
    /// block early on a chunk boundary that landed right after a newline.
    fn in_table(&self) -> bool {
        !matches!(self.table, TableState::None)
    }

    /// Stamp the bullet (first row of the message) or `INDENT` (every later row)
    /// onto each content row.
    fn stamp(&mut self, rows: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
        rows.into_iter()
            .map(|mut spans| {
                let prefix = if self.emitted_any {
                    Span::raw(INDENT.to_string())
                } else {
                    self.emitted_any = true;
                    Span::styled(
                        self.bullet.clone(),
                        Style::new().fg(self.color).add_modifier(Modifier::BOLD),
                    )
                };
                let mut all = vec![prefix];
                all.append(&mut spans);
                Line::from(all)
            })
            .collect()
    }
}

/// Render the bottom live region into `buf`. While a reply streams, the strip's
/// top row previews the in-progress line and the row below it is a blank gap, so
/// the reply never touches the rule-framed, **growing** input box; idle, the
/// strip collapses and the box sits at the top of the region. The input wraps
/// across as many rows as `area` allows; the prompt marks its first line and
/// continuation lines are indented to align under it. When the input is taller
/// than the box, it scrolls internally to keep the **cursor's wrapped row** in
/// view (`input_scroll` follows the cursor wherever the user has moved it).
pub fn render_live(area: Rect, buf: &mut Buffer, app: &App) {
    render_live_with_preview(area, buf, app, None);
}

/// The streaming strip's preview line(s) for `app` at `width` — exactly what the
/// preview slot draws, and whose count is [`preview_rows`] (they must agree, so
/// the box and cursor sit right). A running backend tool previews its **whole**
/// collapsed cell (the wrapped `● name(args)` header + its `⎿ Running…` row) so a
/// long command isn't clipped and the running state shows (req 2); a running `!`
/// shell command previews one `⎿ Running… (Ns)` row (its elapsed rides here since
/// the status line is hidden); a streaming reply previews its last line — or the
/// whole forming table (docs/table-streaming.md) — `stream_preview` is the
/// boundary's cheap render of it ([`StreamRender::preview`], falling back to
/// re-rendering the last line from the buffer when absent, for unit tests).
/// Empty when there is nothing to preview (the pre-stream pause / idle).
fn preview_lines(
    app: &App,
    width: u16,
    stream_preview: Option<&[Line<'static>]>,
) -> Vec<Line<'static>> {
    if let Some(run) = app.viewed_agent() {
        agent_view_preview_lines(run, width)
    } else if app.agent_group().is_some() || !app.tool_queue().is_empty() {
        preview_tool_lines(app, width)
    } else if let Some(lines) = stream_preview {
        lines.to_vec()
    } else {
        app.streaming_text()
            .filter(|t| !t.is_empty())
            .map(|text| {
                message_lines(Role::Assistant, text, width)
                    .pop()
                    .unwrap_or_default()
            })
            .into_iter()
            .collect()
    }
}

/// The live tool queue rendered as preview rows: each call's collapsed cell,
/// blank-line-separated so a **parallel batch** reads like the committed
/// scrollback (the running call live, each not-yet-started sibling a dim
/// `⎿ Waiting…` cell — `docs/parallel-tools.md`). A running backend `bash` cell
/// that has streamed output **tails** it — the header + last lines + a
/// `+N lines (Ns)` footer (`running_command_lines`; the mock,
/// `docs/tool-streaming.md`) — before any output arrives it is the plain
/// `⎿ Running…` peek. A lone `!` shell run collapses to its single
/// `⎿ Running… (Ns)` row (the elapsed rides the preview since a shell turn hides
/// the status line); the shell is never batched, so it is always the only call.
/// Shared by [`preview_lines`] (drawn) and [`preview_rows`] (sized) so the two
/// agree by construction (the strip's `debug_assert`).
fn preview_tool_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let elapsed = app.status().map_or(Duration::ZERO, |s| s.elapsed);
    let mut lines = Vec::new();
    // The round's live agent group leads the strip — its blue tree cell over
    // any ordinary tool cells of a mixed round (docs/agent-tool.md).
    lines.extend(live_agent_group_lines(app, width));
    for (i, tool) in app.tool_queue().iter().enumerate() {
        if i > 0 || !lines.is_empty() {
            lines.push(Line::default()); // blank row between batch cells
        }
        if tool.shell && tool.status == ToolStatus::Running {
            lines.push(shell_running_line(elapsed));
        } else if is_command_tool(tool)
            && tool.status == ToolStatus::Running
            && !command_display_lines(tool).is_empty()
        {
            // A running backend command tool (bash) with streamed output tails it
            // live; other running tools fall to their plain `⎿ Running…` peek.
            lines.extend(running_command_lines(tool, elapsed, width));
        } else {
            lines.extend(tool_lines(tool, width));
        }
        // A running command (a model `bash` call or the `!` shell) can be
        // moved to the background with Ctrl+B — hint it under the live cell,
        // but only once the command has been running a few seconds
        // (`TOOL_BACKGROUND_HINT_DELAY`), Claude-Code-style: a command that
        // finishes right away never flashes the hint (Ctrl+B still works the
        // whole time — only the hint waits). The command's own elapsed is
        // boundary-injected each frame (`App::command_elapsed`). Live-only by
        // construction: this renderer never feeds scrollback commits, so the
        // hint is never committed (docs/background.md).
        if tool.status == ToolStatus::Running
            && (tool.shell || is_command_tool(tool))
            && app
                .command_elapsed()
                .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
        {
            lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
        }
    }
    lines
}

/// [`render_live`], but with the streaming strip's assistant-preview line(s)
/// supplied by the caller (the boundary's cheap [`StreamRender::preview`] —
/// O(one line), or the forming table's rows) instead of re-rendering the whole
/// reply here (which was O(reply) *every animation frame* and starved the
/// status spinner — `docs/markdown.md`). A `None` `stream_preview` falls back
/// to rendering the last line from the buffer, so unit tests (which don't
/// thread a `StreamRender`) keep their old behaviour; production always passes
/// `Some`, its row count injected via [`App::set_stream_preview_rows`] so
/// [`preview_rows`] sizes the same rows this draws (docs/table-streaming.md).
pub fn render_live_with_preview(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    stream_preview: Option<&[Line<'static>]>,
) {
    // The inline `/model` picker replaces the whole live region — the composer,
    // strip, band, and footer all give way to its own framed body. See
    // `docs/llm.md`.
    if let Some(picker) = &app.model_picker {
        render_model_picker(area, buf, picker);
        return;
    }
    // The inline `/login` onboarding flow likewise replaces the whole region.
    if let Some(onboarding) = &app.key_onboarding {
        render_key_onboarding(area, buf, onboarding);
        return;
    }
    // The ↓ background manager band likewise replaces the whole region. See
    // `docs/background.md`.
    if app.background_view.is_some() {
        render_background_view(area, buf, app);
        return;
    }
    // The band below the box holds the palette, the shortcuts overview, *or* the
    // `@` file picker (band_rows — mutually exclusive). Queued messages render
    // in the strip *above* the box instead; the session-context footer takes
    // the very last row unless a band displaces it, and the agent roster's
    // rows sit below it (docs/agent-tool.md).
    let band = band_rows(app);
    let queued = queued_rows(app, area.width);
    let toast = toast_rows(app);
    let footer = footer_rows(app, band);
    let agent_rows = agent_list_rows(app);
    // The preview row + its gap are only reserved when there is something to
    // preview; the pre-stream pause shows status-only (no stray blank line).
    // The status row + its gap are reserved unless this is a `!` shell turn,
    // which hides the spinner status and shows its elapsed in the preview.
    // The preview line(s): a running backend tool's whole cell (wrapped header +
    // `⎿ Running…`), a shell run's `⎿ Running… (Ns)`, or the reply's last line —
    // empty during the pre-stream pause / idle. `preview_rows` is the **single
    // source of truth** the box + cursor geometry size by (`cursor_position`,
    // `main.rs`); the strip layout here uses it too, and the drawn `preview_lines`
    // must match it exactly — same state, same width, so they agree by
    // construction. The `debug_assert` catches any future drift (a desync would
    // reserve one height but paint another, unseating the box/cursor).
    let preview = preview_lines(app, area.width, stream_preview);
    let preview_n = preview_rows(app, area.width);
    debug_assert_eq!(
        usize::from(preview_n),
        preview.len(),
        "preview_rows() must equal the drawn preview_lines()"
    );
    let has_status = strip_has_status(app);
    let [strip, _, band_area, footer_area, agent_area] = live_layout(
        area, has_status, preview_n, queued, toast, band, footer, agent_rows,
    );
    // Rows the preview slot (content + its trailing gap) / status each occupy at
    // the strip's top (0 when absent).
    let preview_slot = if preview_n > 0 {
        preview_n + GAP_ROWS
    } else {
        0
    };
    let status_rows = if has_status {
        STATUS_ROWS + STATUS_GAP_ROWS
    } else {
        0
    };

    // Strip preview at the top; the row below the last preview line is the blank
    // gap. A running tool takes precedence (its coloured cell shows what's
    // executing); otherwise the reply's last line previews. Nothing during the
    // pre-stream pause / idle (an empty `preview` reserves no rows, no stray
    // bullet — codex parity).
    if preview_n > 0 {
        let preview_area = Rect {
            height: preview_n.min(strip.height),
            ..strip
        };
        Paragraph::new(preview).render(preview_area, buf);
    }

    // The live status line, pinned below the preview (or at the strip top during
    // the pause), just above the box, while a turn is in flight — suppressed for
    // a `!` shell turn (has_status false), whose elapsed rides the preview above.
    // An agent session view shows the *viewed agent's* synthesized status
    // instead of the main turn's (docs/agent-tool.md).
    if has_status {
        let line = if let Some(run) = app.viewed_agent() {
            Some(status_line(&agent_view_status(run)))
        } else {
            app.status().map(status_line)
        };
        if let Some(line) = line {
            let status_y = strip.y + preview_slot;
            if status_y < strip.y + strip.height {
                let status_area = Rect {
                    x: strip.x,
                    y: status_y,
                    width: strip.width,
                    height: STATUS_ROWS,
                };
                Paragraph::new(line).render(status_area, buf);
            }
        }
    }

    // The queued messages, styled like sent user messages (❯ bullet, dark
    // background, wrapped), stacked below the status's gap and just above the
    // box's top rule — only while a turn streams (the only time the queue is
    // non-empty). codex's pending-input preview, in our user-message style. The
    // status slot is 0 rows for a shell turn (status_rows), so the queue sits
    // flush under the preview's gap then.
    if queued > 0 {
        let q_y = strip.y + preview_slot + status_rows;
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

    // The transient toast, on the strip's very last row — directly above the
    // box's top rule, below the status/queue when a turn streams and directly
    // above the box when idle. Self-clears after a few seconds (the expiry is
    // timed at the boundary). See docs/toast.md.
    if toast > 0 {
        let strip_bottom = strip.y + strip.height;
        let t_y = strip_bottom.saturating_sub(toast);
        if t_y < strip_bottom {
            let t_area = Rect {
                x: strip.x,
                y: t_y,
                width: strip.width,
                height: toast.min(strip_bottom - t_y),
            };
            Paragraph::new(toast_line(app, t_area.width)).render(t_area, buf);
        }
    }

    // The input box: a top/bottom rule framing the wrapped input rows. An
    // agent session view carries the agent's description as a right-aligned
    // label on the top rule (docs/agent-tool.md).
    let bx = input_box(
        area, &app.input, has_status, preview_n, queued, toast, band, footer, agent_rows,
    );
    let mut block = Block::new()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_COLOR));
    if let Some(run) = app.viewed_agent() {
        block = block.title_top(
            Line::from(Span::styled(
                format!(" {} ", run.description),
                Style::new().fg(TOOL_DIM_COLOR),
            ))
            .right_aligned(),
        );
    }
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
    // band below the box (at most one is open — band_rows).
    if menu_rows(app) > 0 {
        Paragraph::new(command_menu_lines(app, band_area.width)).render(band_area, buf);
    } else if shortcuts_rows(app) > 0 {
        Paragraph::new(shortcuts_lines(
            app.turn_active(),
            app.has_backtrack_target(),
        ))
        .render(band_area, buf);
    } else if file_menu_rows(app) > 0 {
        Paragraph::new(file_menu_lines(app, band_area.width)).render(band_area, buf);
    }

    // The session-context footer on the region's last row — only when no band
    // is open (the band takes its place; see docs/footer.md). An open Ctrl+R
    // search (docs/history-search.md), a `!command` shell mode
    // (docs/shell-command.md), a primed backtrack (docs/backtrack.md), or the
    // roster selection's hints (docs/agent-tool.md) take the same slot with
    // their own line.
    if footer > 0 {
        let line = if let Some(search) = app.history_search.as_ref() {
            search_line(search)
        } else if app.shell_mode {
            shell_mode_line()
        } else if app.backtrack.primed {
            backtrack_hint_line()
        } else if app.agent_selection().is_some() {
            agent_hint_line(app)
        } else {
            footer_line(app, footer_area.width)
        };
        Paragraph::new(line).render(footer_area, buf);
    }

    // The agent roster below the footer — the persistent `● main` + `◯ …`
    // list while agents exist (docs/agent-tool.md).
    if agent_rows > 0 {
        Paragraph::new(agent_list_lines(app, agent_area.width)).render(agent_area, buf);
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

/// The scroll offset (first visible match index) that keeps `selected`
/// **centered** in a window of `max` rows: the selection rides the middle row
/// (`max/2`) while there is room on both sides, so the user always sees as much
/// of the list *above and below* the highlight as fits — the "broad view" the
/// `/model` and `/login` pickers want. Near the ends the window can't center
/// (there aren't enough rows on one side), so it anchors: the top for the first
/// `max/2` selections, the bottom (flush with the tail) for the last. Clamped to
/// `[0, len - max]`.
///
/// Unlike [`menu_window`] — which only scrolls once the selection would leave
/// the window, pinning it to whichever edge it exited — this recenters on every
/// move, which is what stops the highlight getting stuck against the bottom row
/// of a long model list.
#[must_use]
pub fn centered_window(len: usize, selected: usize, max: usize) -> usize {
    if max == 0 || len <= max {
        return 0;
    }
    // Put the selection on the middle row, then clamp so the window never runs
    // off either end (`len - max` is safe: `len > max` here).
    selected.saturating_sub(max / 2).min(len - max)
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

/// Total rows of the band below the input box: the slash-command palette, the
/// `?` shortcuts overview, *or* the `@` file picker (mutually exclusive — the
/// palette needs a `/token`, the shortcuts an empty composer, the picker an
/// `@token`, so at most one term is non-zero). The **one** band-height sum
/// shared by [`render_live`], [`cursor_position`], and the boundary's
/// `live_region_height`, so the three can never drift.
#[must_use]
pub fn band_rows(app: &App) -> u16 {
    menu_rows(app) + shortcuts_rows(app) + file_menu_rows(app)
}

/// The styled lines for the open shortcuts band: the [`SHORTCUTS`] entries two
/// per row — the second column starting at [`SHORTCUTS_COL`] — with keys cyan
/// and labels dim. The `esc` entry is three-way context-sensitive (codex's
/// quit entry): ` to interrupt` while a turn is in flight, the
/// [`SHORTCUTS_BACKTRACK`] `esc esc` edit hint when idle with a previous user
/// message to edit, and ` to quit` only with nothing to backtrack to
/// (docs/backtrack.md).
#[must_use]
pub fn shortcuts_lines(turn_active: bool, can_backtrack: bool) -> Vec<Line<'static>> {
    let entry = |key: &'static str, label: &'static str| {
        let (key, label) = if key == "esc" && turn_active {
            (key, " to interrupt")
        } else if key == "esc" && can_backtrack {
            SHORTCUTS_BACKTRACK
        } else {
            (key, label)
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
                // Pad from the *displayed* widths — a context swap can change
                // the key text too (`esc` → `esc esc`).
                let used = cols(spans[0].content.as_ref()) + cols(spans[1].content.as_ref());
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
/// [`FOOTER_INDENT`], the `esc` key bold-cyan like the search-line hint keys,
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

/// The footer's single line: the [`FOOTER_INDENT`], then `{model} · {cwd}` —
/// every segment dim (codex's no-theme-colours status line, the separator dim
/// like its ` · `) — cut with a trailing `…` when it overflows `width`
/// (codex's `truncate_line_with_ellipsis_if_overflow`). A reasoning-capable
/// model carries its thinking mode right beside the name (`{model} {mode}`,
/// Shift+Tab cycles it — `docs/reasoning.md`). Empty when no session info has
/// been injected.
#[must_use]
pub fn footer_line(app: &App, width: u16) -> Line<'static> {
    let Some(session) = &app.session else {
        return Line::default();
    };
    let dim = Style::new().fg(FOOTER_COLOR);
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

/// How many rows the transient toast reserves above the box: 0 when none is
/// live, else 1 (a single row, truncated to the width). Added to the strip by
/// [`live_height`]/[`live_layout`], between the queued messages and the box's
/// top rule. See `docs/toast.md`.
#[must_use]
pub fn toast_rows(app: &App) -> u16 {
    u16::from(app.toast().is_some())
}

/// The transient toast's single line: the [`TOAST_INDENT`] then the message,
/// dim for an info toast ([`TOAST_COLOR`]) or red for a failure
/// ([`TOAST_ERROR_COLOR`]), cut with a trailing `…` when it overflows `width`
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

/// Linear interpolation between two RGB colours at `t` in `[0, 1]`.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Colour `text` with a left-to-right [`HEADER_GRADIENT_START`] →
/// [`HEADER_GRADIENT_END`] gradient keyed by absolute display column across
/// `total` columns, coalescing equal-colour runs into spans. The header logo's
/// cyan → blue wash (docs/header.md).
fn gradient_spans(text: &str, total: usize) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let mut run: Option<(Color, String)> = None;
    for g in text.graphemes(true) {
        let t = if total <= 1 {
            0.0
        } else {
            col as f32 / (total - 1) as f32
        };
        let color = lerp_rgb(HEADER_GRADIENT_START, HEADER_GRADIENT_END, t);
        match &mut run {
            Some((c, s)) if *c == color => s.push_str(g),
            _ => {
                if let Some((c, s)) = run.take() {
                    spans.push(Span::styled(s, Style::new().fg(c)));
                }
                run = Some((color, g.to_string()));
            }
        }
        col += cols(g);
    }
    if let Some((c, s)) = run {
        spans.push(Span::styled(s, Style::new().fg(c)));
    }
    spans
}

/// The widest display width across `art`'s rows.
fn logo_width(art: &[&str]) -> usize {
    art.iter().map(|row| cols(row)).max().unwrap_or(0)
}

/// Clamp a run of spans to `width` display columns, appending a dim `…` when
/// they overflow — the header metadata rows, truncated exactly like
/// [`footer_line`]. Preserves each kept span's style.
fn clamp_spans(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let total: usize = spans.iter().map(|s| cols(&s.content)).sum();
    if total <= width {
        return Line::from(spans);
    }
    let budget = width.saturating_sub(cols(STATUS_ELLIPSIS));
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let w = cols(&span.content);
        if used + w <= budget {
            used += w;
            out.push(span);
        } else {
            let cut = truncate_cols(&span.content, budget - used);
            if !cut.is_empty() {
                out.push(Span::styled(cut, span.style));
            }
            break;
        }
    }
    out.push(Span::styled(
        STATUS_ELLIPSIS.to_string(),
        Style::new().fg(HEADER_META_COLOR),
    ));
    Line::from(out)
}

/// The startup header banner as scrollback rows (docs/header.md): the ASCII
/// wordmark (sized to `width`), a blank, then the version, tagline, cwd, and the
/// command hint. Pure chrome — `main.rs` commits it once at launch and restores
/// it atop every repaint via [`banner_tail`] (uncapped on a Purge rebuild,
/// window-capped on an InPlace overlay return); it never enters `history`, and
/// the Ctrl+O transcript shows it as chrome too ([`transcript_lines`]).
/// Returns no trailing spacer (the caller adds one, the
/// `insert_before(msg); insert_before(blank)` pattern).
///
/// `width` picks the widest wordmark that fits — the full block art, a compact
/// half-block, or a one-line text badge on a very narrow terminal — so the
/// banner never overflows; the metadata rows are clamped with a trailing `…`.
#[must_use]
pub fn header_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let indent = cols(HEADER_INDENT);
    let accent = Style::new().fg(HEADER_ACCENT_COLOR);
    let dim = Style::new().fg(HEADER_META_COLOR);
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    let full_w = logo_width(HEADER_LOGO_FULL);
    let compact_w = logo_width(HEADER_LOGO_COMPACT);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut badge = false;

    // The widest wordmark that fits: full block → compact half-block → badge.
    if w >= indent + full_w {
        for &row in HEADER_LOGO_FULL {
            lines.push(Line::from(gradient_spans(row, full_w)));
        }
    } else if w >= indent + compact_w {
        for &row in HEADER_LOGO_COMPACT {
            lines.push(Line::from(gradient_spans(row, compact_w)));
        }
    } else {
        // One-line badge: the gradient name + a cyan version, no meta line.
        let mut spans = vec![Span::raw(HEADER_INDENT)];
        spans.extend(gradient_spans(HEADER_NAME, cols(HEADER_NAME)));
        spans.push(Span::styled(format!(" {version}"), accent));
        lines.push(clamp_spans(spans, w));
        badge = true;
    }

    // A blank between the art and the metadata, then `v… · tagline` — skipped
    // for the badge, which already carries the version inline.
    if !badge {
        lines.push(Line::default());
        lines.push(clamp_spans(
            vec![
                Span::raw(HEADER_INDENT),
                Span::styled(version, accent),
                Span::styled(FOOTER_SEPARATOR, dim),
                Span::styled(HEADER_TAGLINE, dim),
            ],
            w,
        ));
    }

    // The cwd (only with session info) and the command hint, shared by all tiers.
    if let Some(session) = &app.session {
        lines.push(clamp_spans(
            vec![
                Span::raw(HEADER_INDENT),
                Span::styled(session.cwd.clone(), dim),
            ],
            w,
        ));
    }
    let mut hint = vec![Span::raw(HEADER_INDENT)];
    for (i, token) in HEADER_HINT.iter().enumerate() {
        if i > 0 {
            hint.push(Span::styled("   ", dim));
        }
        hint.push(Span::styled(*token, accent));
    }
    lines.push(clamp_spans(hint, w));

    lines
}

/// The bullet colour for a tool's lifecycle: dim waiting, blue running, green
/// ok, red fail — and green for a call that resolved by moving to the
/// background (the launch succeeded; see `docs/background.md`).
const fn tool_status_color(status: ToolStatus) -> Color {
    match status {
        ToolStatus::Waiting => TOOL_WAITING_COLOR,
        ToolStatus::Running => TOOL_RUNNING_COLOR,
        ToolStatus::Ok | ToolStatus::Backgrounded => TOOL_OK_COLOR,
        ToolStatus::Failed => TOOL_FAIL_COLOR,
    }
}

/// Truncate `s` to at most `max` display columns (column-aware, so wide glyphs
/// count as two), returning the kept prefix. Measured per **grapheme cluster**
/// with [`cols`] — the same str-level width every fit-check, pad, and ratatui
/// paint uses — so a VS16 emoji (`❤️`, str width 2, char-sum 1) can't overflow
/// the budget and a ZWJ sequence (`👨‍👩‍👧`) is kept or dropped whole, never
/// split after a dangling joiner.
fn truncate_cols(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for g in s.graphemes(true) {
        let gw = cols(g);
        if w + gw > max {
            break;
        }
        out.push_str(g);
        w += gw;
    }
    out
}

/// The coloured bullet header row(s) for a backend tool call: `● {name}({args})`,
/// the bullet recoloured by lifecycle (blue/green/red) and the args made bold +
/// the normal reply white ([`TOOL_ARGS_COLOR`]) so a `bash` command reads
/// clearly, the framing `(`/`)` left a dim [`TOOL_DIM_COLOR`] delimiter. Shared
/// by the inline collapsed view ([`tool_lines`]) and the full-screen transcript
/// ([`tool_full_lines`]).
///
/// When the header overflows `width` the args **word-wrap** across continuation
/// rows, each indented to align **under the opening `(`** (the width of
/// `● {name}`), so a long command reads clean and is never clipped at the
/// terminal edge — Claude-Code's wrapped `Bash(…)` header. `max_rows` caps how
/// many rows are shown: `Some(n)` (the inline peek and the live preview) keeps
/// the first `n` and splices [`TOOL_HEADER_ELLIPSIS`] + `)` onto the last so a
/// huge command doesn't flood the cell; `None` (the Ctrl+O transcript) renders
/// the whole thing. A `!` shell command is a tool with no args (name = the
/// command), so it stays a bare single `● {command}` line.
fn tool_header_lines(tool: &ToolCall, width: u16, max_rows: Option<usize>) -> Vec<Line<'static>> {
    let bullet_style = Style::new()
        .fg(tool_status_color(tool.status))
        .add_modifier(Modifier::BOLD);
    let name_style = Style::new()
        .fg(TOOL_NAME_COLOR)
        .add_modifier(Modifier::BOLD);
    let bullet = || Span::styled(TOOL_BULLET.to_string(), bullet_style);
    let name = || Span::styled(tool.name.clone(), name_style);
    if tool.args.is_empty() {
        return vec![Line::from(vec![bullet(), name()])];
    }
    let args_style = Style::new()
        .fg(TOOL_ARGS_COLOR)
        .add_modifier(Modifier::BOLD);
    // Continuation rows indent to align under the opening `(`, which sits right
    // after `● {name}`; wrapping `(args)` as one run — parens and args alike bold
    // white (a uniform, noticeable header body) — keeps every row (the first
    // included) the same body width, so the wrapped rows land exactly beneath the
    // `(`.
    let indent_cols = cols(TOOL_BULLET) + cols(&tool.name);
    let body_width = (width as usize).saturating_sub(indent_cols).max(1);
    let mut rows = wrap_inline(
        &[(format!("({})", tool.args), args_style)],
        body_width as u16,
    );
    // Cap a very long header: keep the first `max` rows and replace the tail with
    // `…)` (fitted within the body width, the same bold white) — the whole command
    // is still in Ctrl+O.
    if let Some(max) = max_rows
        && rows.len() > max.max(1)
    {
        rows.truncate(max.max(1));
        if let Some(last) = rows.last_mut() {
            let keep = body_width.saturating_sub(cols(TOOL_HEADER_ELLIPSIS) + cols(")"));
            *last = truncate_spans(last, keep);
            last.push(Span::styled(format!("{TOOL_HEADER_ELLIPSIS})"), args_style));
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, mut body_spans)| {
            let mut spans = if i == 0 {
                vec![bullet(), name()]
            } else {
                vec![Span::raw(" ".repeat(indent_cols))]
            };
            spans.append(&mut body_spans);
            Line::from(spans)
        })
        .collect()
}

/// Keep the leading graphemes of `spans` that fit within `max` display columns,
/// preserving each span's style (a span-aware [`truncate_cols`]). Used to make
/// room for the `…)` when a header is truncated at [`TOOL_HEADER_MAX_ROWS`].
fn truncate_spans(spans: &[Span<'static>], max: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let w = cols(&span.content);
        if used + w <= max {
            out.push(span.clone());
            used += w;
        } else {
            let kept = truncate_cols(&span.content, max - used);
            if !kept.is_empty() {
                out.push(Span::styled(kept, span.style));
            }
            break;
        }
    }
    out
}

/// A `⎿` gutter row with an explicit content colour (`None` → dim): the
/// **first** row (index 0) opens with the [`TOOL_RESULT_PREFIX`] corner,
/// continuation rows indent by its display width so the text aligns under it
/// (Claude-Code's exec-cell output style). The shared basis for the dim
/// [`result_row`] and the diff-coloured rows.
fn gutter_row(index: usize, text: String, color: Option<Color>) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let prefix = if index == 0 {
        TOOL_RESULT_PREFIX.to_string()
    } else {
        " ".repeat(cols(TOOL_RESULT_PREFIX))
    };
    let content_style = color.map_or(dim, |c| Style::new().fg(c));
    Line::from(vec![
        Span::styled(prefix, dim),
        Span::styled(text, content_style),
    ])
}

/// A dim `⎿` result row — for the `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `+N lines (Ns)` footer (meta, not output).
fn result_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, None)
}

/// A `⎿` row for a finished tool's **output** — a dim corner over white content
/// ([`TOOL_OUTPUT_COLOR`]), so command/shell output reads like a normal reply.
/// The placeholders keep the dim [`result_row`].
fn output_row(index: usize, text: String) -> Line<'static> {
    gutter_row(index, text, Some(TOOL_OUTPUT_COLOR))
}

/// Is this a model tool whose output is a diff — so its `⎿` rows get `+`/`-`
/// diff colouring ([`diff_result_row`])? See [`DIFF_TOOL_NAMES`].
fn is_diff_tool(tool: &ToolCall) -> bool {
    !tool.shell && DIFF_TOOL_NAMES.contains(&tool.name.as_str())
}

/// Is this a model **command tool** (`bash`) — rendered like the `!` shell cell
/// (a multi-line `⎿` peek, its `Exit code: N` frame stripped for display) and
/// **tailed** live while running? Other generic backend tools keep the single
/// collapsed peek line. See [`COMMAND_TOOL_NAMES`] and `docs/tool-streaming.md`.
fn is_command_tool(tool: &ToolCall) -> bool {
    !tool.shell && COMMAND_TOOL_NAMES.contains(&tool.name.as_str())
}

/// The diff colour for a source line by its leading marker — `+` green, `-`
/// red, everything else (context, the summary header) dim (`None`).
fn diff_line_color(line: &str) -> Option<Color> {
    match line.chars().next() {
        Some('+') => Some(TOOL_DIFF_ADD_COLOR),
        Some('-') => Some(TOOL_DIFF_DEL_COLOR),
        _ => None,
    }
}

/// One parsed row of a numbered `Created …`/`Updated …` body — the
/// `llm::tools` gutter format (`{n:>W} {text}` / `{n:>W} {sign}{text}`) a
/// `write`/`edit` cell restyles ([`file_cell_lines`]).
enum FileRow {
    /// A numbered content line: the raw right-aligned number `gutter`, the
    /// diff `sign` (`None` in a `Created` body, which has no sign column),
    /// and the content `text`.
    Numbered {
        gutter: String,
        sign: Option<char>,
        text: String,
    },
    /// The `⋮` gap row between diff hunks (kept raw for display).
    Gap(String),
    /// A note row (the `… N more lines` cap tail) — rendered dim.
    Note(String),
}

/// Parse one body row of a numbered file cell; `signed` follows the head line
/// (`Updated` bodies carry a `+`/`-`/space sign column, `Created` bodies
/// don't). `None` means the row isn't in the format — the whole cell then
/// keeps the legacy first-char diff colouring (old sessions, error bodies).
fn parse_file_row(line: &str, signed: bool) -> Option<FileRow> {
    let trimmed = line.trim_start_matches(' ');
    if trimmed.starts_with('…') {
        return Some(FileRow::Note(line.to_string()));
    }
    if trimmed == "⋮" {
        return Some(FileRow::Gap(line.to_string()));
    }
    let indent = line.len() - trimmed.len();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let gutter = line[..indent + digits].to_string();
    let rest = &line[indent + digits..];
    if rest.is_empty() {
        // An empty content row whose trailing spaces something stripped.
        return Some(FileRow::Numbered {
            gutter,
            sign: signed.then_some(' '),
            text: String::new(),
        });
    }
    let rest = rest.strip_prefix(' ')?;
    if signed {
        let sign = rest.chars().next().unwrap_or(' ');
        if !matches!(sign, '+' | '-' | ' ') {
            return None;
        }
        Some(FileRow::Numbered {
            gutter,
            sign: Some(sign),
            text: rest.get(1..).unwrap_or("").to_string(),
        })
    } else {
        Some(FileRow::Numbered {
            gutter,
            sign: None,
            text: rest.to_string(),
        })
    }
}

/// Parse a `read`/`write`/`edit` cell's output as a numbered file change: the
/// summary head line for the `⎿` corner and the parsed body rows. A
/// `write`/`edit` cell carries its head in the output (`Created …`/`Updated
/// …`) over a signed (edit) or unsigned (created) body; a `read` cell has
/// **no** head — its whole output is unsigned numbered content, so the
/// `Read N lines` summary is synthesized here. `None` when the cell isn't a
/// finished file tool or the output isn't in the `llm::tools` gutter format (a
/// placeholder like `(file is empty)`, an old rollout, an error body) — the
/// caller keeps the legacy rendering.
fn parse_file_cell(tool: &ToolCall) -> Option<(String, Vec<FileRow>)> {
    if tool.shell || tool.status == ToolStatus::Running {
        return None;
    }
    let lines = tool_output_lines(tool);
    match tool.name.as_str() {
        "Read" => {
            if lines.is_empty() {
                return None;
            }
            // Every line must parse as a numbered content row; a placeholder
            // (`(file is empty)`, offset-past-end) doesn't, and falls back.
            let rows: Vec<FileRow> = lines
                .iter()
                .map(|l| parse_file_row(l, false))
                .collect::<Option<_>>()?;
            if !rows.iter().all(|r| matches!(r, FileRow::Numbered { .. })) {
                return None;
            }
            let n = lines.len();
            let head = format!("Read {n} line{}", if n == 1 { "" } else { "s" });
            Some((head, rows))
        }
        "Write" | "Edit" => {
            let (head, body) = lines.split_first()?;
            let signed = if head.starts_with("Updated ") {
                true
            } else if head.starts_with("Created ") {
                false
            } else {
                return None;
            };
            let rows: Vec<FileRow> = body
                .iter()
                .map(|l| parse_file_row(l, signed))
                .collect::<Option<_>>()?;
            Some((head.clone(), rows))
        }
        _ => None,
    }
}

/// The highlight language for a file cell — the extension of the path in the
/// cell's args (`index.html` → `html`); `None` (plain text) without one.
fn file_cell_lang(args: &str) -> Option<&str> {
    let name = args.rsplit(['/', '\\']).next().unwrap_or(args);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
}

/// The summary head of a file cell (`Created …`/`Updated …`/`Read N lines`) in
/// the white output colour ([`TOOL_OUTPUT_COLOR`]) so it's as noticeable as the
/// output, with its `(+A -D)` counts coloured green/red (codex's header counts);
/// all-white when there are no counts.
fn file_summary_spans(head: &str) -> Vec<Span<'static>> {
    let text = Style::new().fg(TOOL_OUTPUT_COLOR);
    if let Some(open) = head.rfind("(+") {
        let counts = head[open..]
            .strip_prefix("(+")
            .and_then(|t| t.strip_suffix(')'))
            .and_then(|t| t.split_once(" -"));
        if let Some((a, d)) = counts
            && !a.is_empty()
            && !d.is_empty()
            && a.chars().all(|c| c.is_ascii_digit())
            && d.chars().all(|c| c.is_ascii_digit())
        {
            return vec![
                Span::styled(format!("{}(", &head[..open]), text),
                Span::styled(format!("+{a}"), Style::new().fg(TOOL_DIFF_ADD_COLOR)),
                Span::styled(" ".to_string(), text),
                Span::styled(format!("-{d}"), Style::new().fg(TOOL_DIFF_DEL_COLOR)),
                Span::styled(")".to_string(), text),
            ];
        }
    }
    vec![Span::styled(head.to_string(), text)]
}

/// The left indent (display columns) of a numbered file cell's body — the
/// line-number gutter sits **one column past** the `⎿` corner content
/// ([`TOOL_RESULT_PREFIX`]), matching Claude-Code's file-change look (the
/// numbers land just inside the corner). The `⋮` hunk gaps and `…` notes align
/// here too. See `docs/tools.md`.
fn file_body_indent() -> usize {
    cols(TOOL_RESULT_PREFIX) + 1
}

/// Build the display rows for one numbered source row: a dim right-aligned
/// line number, the `+`/`-` sign in the diff colour, and the content
/// syntax-highlighted — added rows on the dark-green tint, removed rows
/// (their text dimmed) on the dark-red one, both padded to the full width.
/// Long content wraps ([`code_content_rows`]); continuations indent under the
/// content column and keep the tint.
fn numbered_row_lines(
    gutter: &str,
    sign: Option<char>,
    segs: &[highlight::Seg],
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent_cols = file_body_indent();
    let indent = " ".repeat(indent_cols);
    let (bg, dim_content) = match sign {
        Some('+') => (Some(TOOL_DIFF_ADD_BG), false),
        Some('-') => (Some(TOOL_DIFF_DEL_BG), true),
        _ => (None, false),
    };
    let sign_style = match sign {
        Some('+') => Style::new().fg(TOOL_DIFF_ADD_COLOR),
        Some('-') => Style::new().fg(TOOL_DIFF_DEL_COLOR),
        _ => dim,
    };
    let with_bg = |style: Style| bg.map_or(style, |b| style.bg(b));

    let number = format!("{gutter} ");
    let gutter_cols = cols(&number) + usize::from(sign.is_some());
    let content_width = (width as usize)
        .saturating_sub(indent_cols + gutter_cols)
        .max(1);

    let segments: Vec<(String, Style)> = segs
        .iter()
        .map(|seg| (seg.text.clone(), seg.style))
        .collect();
    code_content_rows(&segments, content_width as u16)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![Span::raw(indent.clone())];
            if i == 0 {
                spans.push(Span::styled(number.clone(), with_bg(dim)));
                if let Some(s) = sign {
                    spans.push(Span::styled(s.to_string(), with_bg(sign_style)));
                }
            } else {
                spans.push(Span::styled(" ".repeat(gutter_cols), with_bg(Style::new())));
            }
            let mut row_cols = 0usize;
            for span in row {
                row_cols += cols(&span.content);
                let mut style = with_bg(span.style);
                if dim_content {
                    style = style.add_modifier(Modifier::DIM);
                }
                spans.push(Span::styled(span.content.into_owned(), style));
            }
            let pad = content_width.saturating_sub(row_cols);
            if bg.is_some() && pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), with_bg(Style::new())));
            }
            Line::from(spans)
        })
        .collect()
}

/// Build the styled `⎿` block for a `read`/`write`/`edit` cell whose output is
/// the numbered `llm::tools` format — codex's file look in the existing gutter:
/// the white summary head (its `(+A -D)` counts coloured) on the corner row,
/// then every body row via [`numbered_row_lines`], the `⋮` hunk gaps and `…`
/// notes dim. `peek` caps the body at [`FILE_PEEK_LINES`] display rows (whole
/// source rows only) and appends the `… +N lines (ctrl+o to expand)` hint.
/// `None` when the output isn't in the format — the caller falls back to the
/// legacy rendering. See `docs/tools.md`.
fn file_cell_lines(tool: &ToolCall, width: u16, peek: bool) -> Option<Vec<Line<'static>>> {
    let (head, rows) = parse_file_cell(tool)?;
    let lang = file_cell_lang(&tool.args);
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let indent = " ".repeat(file_body_indent());
    let note_width = (width as usize).saturating_sub(indent.len()).max(1);

    let mut summary = vec![Span::styled(TOOL_RESULT_PREFIX.to_string(), dim)];
    summary.extend(file_summary_spans(&head));
    let mut out = vec![Line::from(summary)];

    let budget = if peek { FILE_PEEK_LINES } else { usize::MAX };
    let mut used = 0usize;
    let mut hidden = 0usize;
    let mut hl = highlight::Highlighter::new(lang);
    for (i, row) in rows.iter().enumerate() {
        let display = match row {
            FileRow::Gap(raw) => {
                // Hunks re-synchronize at the gap; the lexer state resets too.
                hl = highlight::Highlighter::new(lang);
                vec![Line::from(vec![
                    Span::raw(indent.clone()),
                    Span::styled(truncate_cols(raw, note_width), dim),
                ])]
            }
            FileRow::Note(raw) => vec![Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(truncate_cols(raw, note_width), dim),
            ])],
            FileRow::Numbered { gutter, sign, text } => {
                let segs = hl.line(text);
                numbered_row_lines(gutter, *sign, &segs, width)
            }
        };
        if used + display.len() > budget && used > 0 {
            hidden = rows[i..]
                .iter()
                .filter(|r| matches!(r, FileRow::Numbered { .. }))
                .count();
            break;
        }
        used += display.len();
        out.extend(display);
    }
    if hidden > 0 {
        out.push(more_hint_line(hidden));
    }
    Some(out)
}

/// The dim `… +N lines (ctrl+o to expand)` hint under a capped peek.
fn more_hint_line(hidden: usize) -> Line<'static> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Line::from(vec![
        Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
        Span::styled(format!("+{hidden} lines{EXPAND_HINT}"), dim),
    ])
}

/// The live preview row for a running `!` shell command: `⎿ Running… (Ns)`. A
/// shell turn hides the spinner status line entirely (see [`strip_has_status`]),
/// so its running elapsed lives here instead — the boundary-supplied
/// `elapsed` (whole seconds), like the status line's timer. Only shown live in
/// [`render_live`]; the committed cell renders its output, not `Running…`. See
/// `docs/shell-command.md`.
fn shell_running_line(elapsed: Duration) -> Line<'static> {
    result_row(0, format!("{TOOL_RUNNING} ({}s)", elapsed.as_secs()))
}

/// The output of `tool` split into display lines (a single trailing blank from a
/// final newline dropped, so a hidden-line count is accurate). Tabs are
/// expanded for display ([`expand_code_tabs`]) — a `'\t'` grapheme paints as
/// zero cells (ratatui filters control chars), gluing tab-separated fields
/// together — while the stored output stays byte-exact, like the code-block
/// render path.
fn tool_output_lines(tool: &ToolCall) -> Vec<String> {
    split_display_lines(&tool.output)
}

/// Split display `text` into lines: tabs expanded ([`expand_code_tabs`] — a
/// `'\t'` paints as zero cells otherwise), with a single trailing blank from a
/// final newline dropped so a hidden-line count stays accurate.
fn split_display_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = text
        .split('\n')
        .map(|line| expand_code_tabs(line).into_owned())
        .collect();
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// `output` reframed for **display**: on **success** the `Exit code: 0` line
/// is dropped so the cell reads like the real command output (the mock,
/// `docs/tool-streaming.md`); on **failure** it is rewritten to an
/// `Error: Exit code N` (or `Error: killed by signal`) line kept above the
/// body, so a red cell says *why* it failed even when the body is empty. The
/// raw frame stays in `tool.output` for the model / context replay
/// (`context::context_messages`) — this is display-only. Only fires when the
/// frame is present, so non-`bash` tools and old rollouts are untouched.
fn command_display_output(output: &str) -> std::borrow::Cow<'_, str> {
    let Some(rest) = output.strip_prefix("Exit code: ") else {
        return std::borrow::Cow::Borrowed(output);
    };
    // The frame is `Exit code: {code}\n{body}` (the body may be absent).
    let (code, body) = match rest.find('\n') {
        Some(nl) => (&rest[..nl], &rest[nl + 1..]),
        None => (rest, ""),
    };
    if code == "0" {
        // Success: the frame is noise — show just the body.
        return std::borrow::Cow::Borrowed(body);
    }
    // Failure: surface the exit code as an `Error: …` header above the body.
    let head = if code == "killed by signal" {
        "Error: killed by signal".to_string()
    } else {
        format!("Error: Exit code {code}")
    };
    std::borrow::Cow::Owned(if body.is_empty() {
        head
    } else {
        format!("{head}\n{body}")
    })
}

/// A command-style tool's output as display lines — [`tool_output_lines`] with
/// the `Exit code: N` frame reframed for display ([`command_display_output`]).
fn command_display_lines(tool: &ToolCall) -> Vec<String> {
    split_display_lines(&command_display_output(&tool.output))
}

/// The live preview for a **running** command-style backend tool (`bash`): the
/// coloured `● name(args)` header, the **last** [`TOOL_PEEK_LINES`] display
/// **rows** of its output under the `⎿` gutter (the *tail* — what just
/// streamed), then a `+{hidden} lines ({secs}s)` footer when any source lines
/// are fully hidden above it. This is Claude-Code's running-command look (the
/// mock; `docs/tool-streaming.md`) — the asymmetric twin of the finished head
/// peek in [`tool_lines`]. The `elapsed` is boundary-supplied (like the shell
/// running row and the status timer), so this is drawn from
/// [`preview_tool_lines`] where `App` is in hand.
///
/// Long lines **word-wrap, spaces preserved** ([`wrap_output`] — the same
/// wrapper the finished peek and the Ctrl+O view use, so alignment survives
/// and prose breaks at words) instead of clipping at the width; the window is
/// counted in wrapped rows so a single long line tail-follows its own newest
/// rows without growing the strip past its budget. Walking the source lines
/// newest-first wraps only what the window can show — never the whole
/// retained buffer — per animation frame.
fn running_command_lines(tool: &ToolCall, elapsed: Duration, width: u16) -> Vec<Line<'static>> {
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    let display = command_display_lines(tool);
    // The tail window: the last TOOL_PEEK_LINES wrapped rows, each remembering
    // its source line index so the footer can count what's *fully* hidden.
    let mut window: VecDeque<(usize, String)> = VecDeque::new();
    for (idx, line) in display.iter().enumerate().rev() {
        for row in wrap_output(line, wrap_width).into_iter().rev() {
            window.push_front((idx, row));
        }
        if window.len() >= TOOL_PEEK_LINES {
            break;
        }
    }
    while window.len() > TOOL_PEEK_LINES {
        window.pop_front();
    }
    // Source lines wholly above the window. A partially shown wrapped line is
    // on screen, not hidden — its index is the count of the lines above it.
    let hidden = window.front().map_or(0, |(idx, _)| *idx);
    let shown = window.len();
    for (i, (_, row)) in window.into_iter().enumerate() {
        lines.push(output_row(i, row));
    }
    if hidden > 0 {
        // A continuation row (index ≥ 1) so it indents under the content column;
        // the `+N lines (Ns)` footer is meta, so it stays the dim `result_row`.
        lines.push(result_row(
            shown,
            format!("+{hidden} lines ({}s)", elapsed.as_secs()),
        ));
    }
    lines
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
    let peek_width = (width as usize)
        .saturating_sub(cols(TOOL_RESULT_PREFIX))
        .max(1);
    let out_lines = tool_output_lines(tool);

    // A call resolved by moving to the background shows the fixed
    // `⎿ Running in the background (↓ to manage)` row — its stored output is
    // the model-facing launch text, never displayed (docs/background.md). A
    // `!` shell cell stays headerless like its other states.
    if tool.status == ToolStatus::Backgrounded {
        let row = result_row(0, TOOL_BACKGROUNDED.to_string());
        if tool.shell {
            return vec![row];
        }
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.push(row);
        return lines;
    }

    if tool.shell {
        // The running/empty single-row states; else up to TOOL_PEEK_LINES rows.
        // (Truncation of an over-cap output is marked only in the expanded view;
        // inline, the `… +N lines (ctrl+o to expand)` hint already signals more.)
        return match tool.status {
            // A shell command is never batched, so it is never `Waiting`; the
            // arm is here only to keep the match total and correct if it ever is.
            ToolStatus::Waiting => vec![result_row(0, TOOL_WAITING.to_string())],
            ToolStatus::Running => vec![result_row(0, TOOL_RUNNING.to_string())],
            _ if out_lines.is_empty() => vec![result_row(0, TOOL_NO_OUTPUT.to_string())],
            _ => result_peek_block(&out_lines, peek_width, wrap_output, |i, text, _| {
                output_row(i, text)
            }),
        };
    }

    // A `write`/`edit` cell in the numbered `llm::tools` format renders
    // codex-style — numbers, hunk gaps, tints, syntax colour
    // ([`file_cell_lines`]); output that doesn't parse (old sessions, error
    // bodies) falls through to the legacy first-char colouring below.
    if let Some(body) = file_cell_lines(tool, width, true) {
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.extend(body);
        return lines;
    }

    // An `edit`/`write` diff tool: coloured header + a multi-line `⎿` peek whose
    // `+`/`-` rows are diff-coloured (the codex trick shows inline, not just in
    // the Ctrl+O view). Other backend tools keep the single collapsed peek line.
    if is_diff_tool(tool) && tool.status != ToolStatus::Running && !out_lines.is_empty() {
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        // Wrap verbatim (a diff body is code, never reflowed at spaces) and
        // colour every wrapped row by the SOURCE line's `+`/`-` marker, so a
        // continuation row keeps its tint — the Ctrl+O view colours the same
        // way (docs/tools.md).
        lines.extend(result_peek_block(
            &out_lines,
            peek_width,
            wrap_verbatim,
            |i, text, src| gutter_row(i, text, diff_line_color(src)),
        ));
        return lines;
    }

    // A backend **command tool** (`bash`): coloured header (wrapped when long)
    // over a multi-line `⎿` peek — the *head*, up to TOOL_PEEK_LINES lines, then
    // `… +N lines (ctrl+o to expand)`, like the `!` shell cell (the mock's
    // finished state). The `Exit code: N` frame is stripped for display
    // (docs/tool-streaming.md); no output yet → the `⎿ Running…`/`Waiting…` row.
    // The running *tail* (last lines + elapsed) is a separate live-only render
    // (`running_command_lines`), used by the preview.
    if is_command_tool(tool) {
        let display = command_display_lines(tool);
        let peek = match tool.status {
            ToolStatus::Waiting => vec![result_row(0, TOOL_WAITING.to_string())],
            _ if display.is_empty() => vec![result_row(
                0,
                if tool.status == ToolStatus::Running {
                    TOOL_RUNNING.to_string()
                } else {
                    TOOL_NO_OUTPUT.to_string()
                },
            )],
            _ => result_peek_block(&display, peek_width, wrap_output, |i, text, _| {
                output_row(i, text)
            }),
        };
        let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
        lines.extend(peek);
        return lines;
    }

    // Any other backend tool (a `read`/`write`/`edit` cell whose output didn't
    // parse as the numbered/diff format, or an unknown tool): coloured header
    // (wrapped when long) + a single collapsed peek line — white output content,
    // dim placeholder — the rest behind the `… +N lines` hint.
    let mut lines = tool_header_lines(tool, width, Some(TOOL_HEADER_MAX_ROWS));
    lines.push(match tool.status {
        ToolStatus::Waiting => result_row(0, TOOL_WAITING.to_string()),
        ToolStatus::Running => result_row(0, TOOL_RUNNING.to_string()),
        _ if out_lines.is_empty() => result_row(0, TOOL_NO_OUTPUT.to_string()),
        _ => output_row(0, truncate_cols(&out_lines[0], peek_width)),
    });
    let hidden = out_lines.len().saturating_sub(1);
    if hidden > 0 {
        lines.push(more_hint_line(hidden));
    }
    lines
}

/// The head peek of `out_lines`: the first [`TOOL_PEEK_LINES`] **source
/// lines**, each **fully wrapped** to `peek_width` — "the first 4 lines of
/// output", so a long first line never pushes its siblings out of the peek —
/// built with `row` (which also receives the **source** line, so a diff cell
/// can colour a wrapped continuation by the source's `+`/`-` marker). The
/// wrapper is `wrap`: [`wrap_output`] for command/shell output (word
/// boundaries, spaces preserved, like the Ctrl+O view) or [`wrap_verbatim`]
/// for diff bodies (code — hard-break, never reflowed at spaces) — either
/// way a long line's tail no longer disappears past the terminal edge. A
/// `… +N lines` hint follows when any source line isn't fully shown.
///
/// [`TOOL_PEEK_MAX_ROWS`] is the safety ceiling in display rows: one
/// pathological line (a minified bundle) can't balloon a committed cell into
/// hundreds of rows. `hidden` counts **source lines** not fully shown (a
/// line the ceiling cut mid-wrap counts as hidden), so the hint appears
/// whenever any content is cut — even within a single line. The wrap stops
/// once a budget is spent, so this is O(peek), not O(output).
fn result_peek_block(
    out_lines: &[String],
    peek_width: usize,
    wrap: fn(&str, u16) -> Vec<String>,
    row: impl Fn(usize, String, &str) -> Line<'static>,
) -> Vec<Line<'static>> {
    let wrap_width = u16::try_from(peek_width).unwrap_or(u16::MAX);
    let mut lines: Vec<Line> = Vec::new();
    let mut fully_shown = 0usize; // source lines whose every wrapped row fits
    for line in out_lines.iter().take(TOOL_PEEK_LINES) {
        if lines.len() >= TOOL_PEEK_MAX_ROWS {
            break;
        }
        let wrapped = wrap(line, wrap_width);
        let total = wrapped.len();
        let room = TOOL_PEEK_MAX_ROWS - lines.len();
        let take = total.min(room);
        for text in wrapped.into_iter().take(take) {
            // The very first display row of the block gets the `⎿` corner
            // ([`gutter_row`]'s index 0); every later row — a wrapped
            // continuation or the next source line — indents under the content
            // column, exactly like the uncapped Ctrl+O block.
            let i = lines.len();
            lines.push(row(i, text, line));
        }
        if take < total {
            break; // the ceiling cut this line mid-wrap: only partially shown
        }
        fully_shown += 1;
    }
    let hidden = out_lines.len() - fully_shown;
    if hidden > 0 {
        lines.push(more_hint_line(hidden));
    }
    lines
}

/// One tool call's full lines for the transcript view: its **complete** output
/// (wrapped **verbatim** — [`wrap_verbatim`], so `ls -l`/`tree` alignment and
/// indentation survive), or `running…` / `(no output)` when there is none yet.
/// The expanded counterpart of [`tool_lines`]. The output renders as a `⎿`
/// gutter block — each row aligned under the corner ([`result_row`]), the same
/// gutter as the inline peek and a shell cell — uncapped. A `!` shell command
/// stays **headerless** (the `! pwd` dark header is the `Role::Shell` message
/// above it); a backend tool keeps its coloured `● name(args)` header over the
/// gutter. An over-cap shell output ([`ToolCall::truncated`]) appends a dim
/// [`TOOL_TRUNCATED_MARKER`] line to show the rest was dropped.
fn tool_full_lines(tool: &ToolCall, width: u16) -> Vec<Line<'static>> {
    // A backgrounded call shows its fixed row in the transcript too — the
    // live output belongs to the ↓ manager, and the final output arrives as
    // the completion notice (docs/background.md).
    if tool.status == ToolStatus::Backgrounded {
        let row = result_row(0, TOOL_BACKGROUNDED.to_string());
        if tool.shell {
            return vec![row];
        }
        let mut lines = tool_header_lines(tool, width, None);
        lines.push(row);
        return lines;
    }
    // A numbered `write`/`edit` cell renders wholesale (numbers, tints,
    // syntax colour — [`file_cell_lines`], uncapped here); everything else
    // goes through the plain row pipeline below.
    if let Some(body) = file_cell_lines(tool, width, false) {
        // The Ctrl+O transcript view never truncates the header (`None`).
        let mut lines = tool_header_lines(tool, width, None);
        lines.extend(body);
        if tool.truncated {
            lines.push(gutter_row(1, TOOL_TRUNCATED_MARKER.to_string(), None));
        }
        return lines;
    }
    // The body hangs under the `⎿` gutter, so it wraps to the width left of it
    // (like [`result_row`]'s continuation indent) — for a backend tool too, so
    // its expanded output aligns under the corner just like its inline peek.
    let body_width = width.saturating_sub(cols(TOOL_RESULT_PREFIX) as u16).max(1);
    // Each body row carries its content colour (`None` → dim). For an
    // `edit`/`write` diff cell the colour comes from the **source** line, then
    // the line is wrapped — so a long `+`/`-` line's continuation rows keep the
    // added/removed colour instead of being mis-coloured by their own
    // (marker-less) first char. Every other tool's output is dim.
    let mut rows: Vec<(String, Option<Color>)> = match (tool.status, tool.output.is_empty()) {
        (ToolStatus::Waiting, _) => vec![(TOOL_WAITING.to_string(), None)],
        (ToolStatus::Running, true) => vec![(TOOL_RUNNING.to_string(), None)],
        (_, true) => vec![(TOOL_NO_OUTPUT.to_string(), None)],
        _ if is_diff_tool(tool) => tool
            .output
            .split('\n')
            .flat_map(|src| {
                let color = diff_line_color(src);
                wrap_verbatim(&expand_code_tabs(src), body_width)
                    .into_iter()
                    .map(move |piece| (piece, color))
            })
            .collect(),
        // Tabs expanded for display (they paint as zero cells otherwise —
        // see `tool_output_lines`); the stored output stays byte-exact. A
        // backend command tool's `Exit code: N` frame is stripped for display
        // (a `!` shell command's output is raw — never framed, never stripped;
        // docs/tool-streaming.md).
        _ => {
            let body: std::borrow::Cow<'_, str> = if tool.shell {
                std::borrow::Cow::Borrowed(tool.output.as_str())
            } else {
                command_display_output(&tool.output)
            };
            wrap_output(&expand_code_tabs(&body), body_width)
                .into_iter()
                .map(|line| (line, Some(TOOL_OUTPUT_COLOR)))
                .collect()
        }
    };
    // The output was cut at the in-memory cap — mark the end so the user knows
    // more was dropped (it is not recoverable; nothing to expand to). Only the
    // `!` shell runner caps, so a backend tool never sets this.
    if tool.truncated {
        rows.push((TOOL_TRUNCATED_MARKER.to_string(), None));
    }
    let result = rows
        .into_iter()
        .enumerate()
        .map(|(i, (text, color))| gutter_row(i, text, color));
    // A `!` shell command is headerless (its `Role::Shell` header sits above);
    // a backend tool keeps its coloured `● name(args)` header (wrapped when long)
    // over the gutter.
    if tool.shell {
        result.collect()
    } else {
        // The Ctrl+O transcript view shows the whole command (`None`).
        let mut lines = tool_header_lines(tool, width, None);
        lines.extend(result);
        lines
    }
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

/// Humanize an elapsed count of whole seconds for the status indicator —
/// combined two-unit, so the display scales past a bare seconds counter:
/// `{s}s` under a minute (the live seconds keep ticking so a running timer
/// never looks frozen), `{m}m {s}s` under an hour, `{h}h {m}m` past an hour
/// (`h` grows unbounded). Distinct from [`crate::session::relative_age`], which
/// is a **single-unit** static age label (`2m`, `1h`). Shared by the live
/// status line, its `Thinking for …` clause, and the committed `… for …`
/// summary so all three read the same (`docs/status-indicator.md`).
#[must_use]
pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", secs % 60);
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

/// Humanize a token count for the status line and the turn summary: bare under
/// a thousand (`842`), one-decimal thousands up to a million (`8.1k`, a
/// trailing `.0` dropped — `15k`), one-decimal millions past that (`1.2M`).
/// Real provider usage counts the whole re-sent context per round, so an
/// agentic turn's tally runs to six digits — unreadable raw in a one-line
/// status (`docs/prompt-caching.md`).
#[must_use]
pub fn format_token_count(tokens: usize) -> String {
    /// One-decimal `value/scale` with a trailing `.0` dropped (`8.1`, `15`).
    fn scaled(tokens: usize, scale: f64, suffix: &str) -> String {
        #[allow(clippy::cast_precision_loss)] // display only — 1dp anyway
        let value = (tokens as f64 / scale * 10.0).round() / 10.0;
        if value.fract() == 0.0 {
            format!("{value:.0}{suffix}")
        } else {
            format!("{value:.1}{suffix}")
        }
    }
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        scaled(tokens, 1_000.0, "k")
    } else {
        scaled(tokens, 1_000_000.0, "M")
    }
}

/// The live status line shown in the strip above the box while a turn is in
/// flight:
/// `(●•·   ) {verb}… ({elapsed}[ · {arrow} {n} tokens][ · Thinking for {m}] · esc to interrupt)`.
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
    let dim = Style::new().fg(STATUS_DETAIL_COLOR);
    let mut spans = spinner_spans(status.elapsed);
    spans.extend(shimmer_spans(
        &format!("{}{STATUS_ELLIPSIS}", status.verb),
        status.elapsed,
    ));
    // The parenthesised metrics are dim, except the retry clause, which carries
    // its own warning colour — so it is built as its own span between the
    // (dim) token and hint clauses.
    spans.push(Span::styled(
        format!(" ({}", format_elapsed(status.elapsed.as_secs())),
        dim,
    ));
    if status.tokens > 0 {
        let arrow = match status.arrow {
            TokenArrow::Down => STATUS_ARROW_DOWN,
            TokenArrow::Up => STATUS_ARROW_UP,
        };
        spans.push(Span::styled(
            format!(" · {arrow} {} tokens", format_token_count(status.tokens)),
            dim,
        ));
    }
    if let Some(retry) = status.retry {
        spans.push(Span::styled(
            format!(" · retrying {}/{}", retry.attempt, retry.max),
            Style::new().fg(STATUS_RETRY_COLOR),
        ));
    }
    if let Some(thinking) = status.thinking {
        spans.push(Span::styled(
            format!(" · Thinking for {}", format_elapsed(thinking.as_secs())),
            dim,
        ));
    }
    spans.push(Span::styled(format!(" · {STATUS_INTERRUPT_HINT})"), dim));
    Line::from(spans)
}

/// The committed turn summary: a single dim, bullet-less `"{verb} for {elapsed}"`
/// (the seconds humanized by [`format_elapsed`] — `Done for 20s`, `Done for 1m 30s`)
/// line — with a `· {n} shells still running` suffix when background shells
/// were running at turn end (`docs/background.md`). Shown inline (it flows into
/// scrollback) and in the transcript like any other [`HistoryItem`]; `width` is
/// unused (the line never wraps) but kept for a uniform `*_lines` signature.
#[must_use]
pub fn summary_lines(summary: &TurnSummary, _width: u16) -> Vec<Line<'static>> {
    let mut text = format!("{} for {}", summary.verb, format_elapsed(summary.secs));
    if summary.tokens > 0 {
        // The turn's real billed tokens, with the cache-served share beside
        // them — the visible proof prompt caching worked. Absent (the dummy,
        // a `!` shell) the summary keeps its bare shape. See
        // docs/prompt-caching.md.
        text.push_str(&format!(" · {} tokens", format_token_count(summary.tokens)));
        if summary.cached > 0 {
            text.push_str(&format!(" ({} cached)", format_token_count(summary.cached)));
        }
    }
    if summary.shells > 0 {
        let plural = if summary.shells == 1 { "" } else { "s" };
        text.push_str(&format!(
            " · {} shell{plural} still running",
            summary.shells
        ));
    }
    vec![Line::from(Span::styled(
        text,
        Style::new().fg(STATUS_DONE_COLOR),
    ))]
}

/// A background shell's completion notice as committed lines: the coloured
/// `●` bullet — green for a clean exit, red for a failure or a user stop —
/// over the wrapped one-line headline (`Background command "{description}"
/// completed (exit code 0)`). The output tail the notice carries is
/// context-only and never rendered. See `docs/background.md`.
#[must_use]
pub fn background_notice_lines(
    notice: &crate::app::BackgroundNotice,
    width: u16,
) -> Vec<Line<'static>> {
    let color = if notice.ok() {
        BG_NOTICE_OK_COLOR
    } else {
        BG_NOTICE_FAIL_COLOR
    };
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    wrap_text(&notice.headline(), content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(AI_BULLET.to_string(), bullet_style),
                    Span::raw(line),
                ])
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(line)])
            }
        })
        .collect()
}

// --- The `Agent` tool (docs/agent-tool.md) ---

/// The tree connectors of a group cell's per-agent rows: `   ├ {description}`
/// for every agent but the last, `   └ {description}` for the last, with the
/// status row's gutter continuing the rail (`   │ ⎿  Done` / `     ⎿  Done`).
const AGENT_TREE_INDENT: &str = "   ";
const AGENT_TREE_MID: &str = "├ ";
const AGENT_TREE_LAST: &str = "└ ";
const AGENT_TREE_PIPE: &str = "│ ";
const AGENT_TREE_BLANK: &str = "  ";
/// The status row's corner inside the tree (`⎿  Done`).
const AGENT_TREE_CORNER: &str = "⎿  ";
/// The committed background-launch header's manager hint.
const AGENT_MANAGE_HINT: &str = " (↓ to manage)";
/// The Ctrl+O cell's `Prompt:` / `Response:` section labels (green bold,
/// Claude Code's transcript look).
const AGENT_PROMPT_LABEL: &str = "Prompt:";
const AGENT_RESPONSE_LABEL: &str = "Response:";
const AGENT_SECTION_COLOR: Color = TOOL_OK_COLOR;
/// Indent of a Ctrl+O agent cell's section bodies (under the `⎿  ` corner's
/// label, one level further in) and of its nested tool-header lines.
const AGENT_BODY_INDENT: &str = "       ";
const AGENT_NESTED_INDENT: &str = "     ";
/// The footer roster (the persistent agent list under the footer): the
/// selection marker, the main row's bullet, and an agent row's circle.
const AGENT_LIST_MARKER: &str = "❯ ";
const AGENT_LIST_INDENT: &str = "  ";
const AGENT_MAIN_BULLET: &str = "● ";
const AGENT_ROW_BULLET: &str = "◯ ";
const AGENT_MAIN_LABEL: &str = "main";
/// The roster selection's footer hints (they take the footer line's slot).
const AGENT_HINT_MAIN: &[(&str, &str)] = &[("↑/↓", " to select"), ("Enter", " to view")];
const AGENT_HINT_AGENT: &[(&str, &str)] = &[("Enter", " to view"), ("x", " to stop")];

/// The group header's noun phrase: `2 agents` / `1 agent`.
fn agent_count_phrase(count: usize) -> String {
    if count == 1 {
        "1 agent".to_string()
    } else {
        format!("{count} agents")
    }
}

/// ` · {n} tool use[s] · {tokens} tokens` — a tree row's counters clause
/// (omitted while both are zero, and on a background-launch cell).
fn agent_counters_clause(tool_uses: usize, tokens: u64) -> String {
    let mut clause = String::new();
    if tool_uses > 0 || tokens > 0 {
        let plural = if tool_uses == 1 { "" } else { "s" };
        clause.push_str(&format!(" · {tool_uses} tool use{plural}"));
        clause.push_str(&format!(
            " · {} tokens",
            format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX))
        ));
    }
    clause
}

/// One agent's two tree rows: the connector + description + dim counters,
/// then the rail + `⎿  {status}`. Rows truncate at the width (Claude Code's
/// truncate-end), so the tree never wraps.
fn agent_tree_rows(
    is_last: bool,
    description: &str,
    counters: &str,
    status: Option<(&str, Color)>,
    width: u16,
) -> Vec<Line<'static>> {
    let budget = (width as usize)
        .saturating_sub(cols(AGENT_TREE_INDENT) + cols(AGENT_TREE_MID))
        .max(1);
    let connector = if is_last {
        AGENT_TREE_LAST
    } else {
        AGENT_TREE_MID
    };
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut description = description.to_string();
    let counters_cols = cols(counters);
    if cols(&description) + counters_cols > budget {
        description = truncate_cols(&description, budget.saturating_sub(counters_cols + 1));
        description.push('…');
    }
    let mut lines = vec![Line::from(vec![
        Span::styled(AGENT_TREE_INDENT.to_string(), dim),
        Span::styled(connector.to_string(), dim),
        Span::styled(description, Style::new().fg(TOOL_OUTPUT_COLOR)),
        Span::styled(counters.to_string(), dim),
    ])];
    if let Some((status, color)) = status {
        let rail = if is_last {
            AGENT_TREE_BLANK
        } else {
            AGENT_TREE_PIPE
        };
        lines.push(Line::from(vec![
            Span::styled(AGENT_TREE_INDENT.to_string(), dim),
            Span::styled(rail.to_string(), dim),
            Span::styled(AGENT_TREE_CORNER.to_string(), dim),
            Span::styled(
                truncate_cols(status, budget.saturating_sub(cols(AGENT_TREE_CORNER))),
                Style::new().fg(color),
            ),
        ]));
    }
    lines
}

/// The group cell's `● {header}` row: the coloured bullet, the white header
/// text, and a dim trailing hint.
fn agent_group_header(color: Color, text: String, hint: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(text, Style::new().fg(TOOL_NAME_COLOR)),
        Span::styled(hint.to_string(), Style::new().fg(TOOL_DIM_COLOR)),
    ])
}

/// A **single** agent's `● Agent({description})` cell header — the tool-cell
/// look a lone launch keeps instead of the group tree (`docs/agent-tool.md`).
fn agent_cell_header(color: Color, description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({description})"), Style::new().fg(TOOL_ARGS_COLOR)),
    ])
}

/// The `Done ({n} tool uses · {tokens} tokens · {s}s)` settle clause shared by
/// the Ctrl+O cell footer and the single-agent committed cell.
fn agent_done_clause(tool_uses: usize, tokens: u64, secs: u64) -> String {
    format!(
        "Done ({tool_uses} tool use{} · {} tokens · {secs}s)",
        if tool_uses == 1 { "" } else { "s" },
        format_token_count(usize::try_from(tokens).unwrap_or(usize::MAX)),
    )
}

/// A running tool's header **inside a `⎿` corner** — the single-agent live
/// cell's `⎿  Bash(sleep 10 && curl -s "…` shape: the corner row leads,
/// continuations char-wrap aligned under the opening `(`, capped at
/// [`TOOL_HEADER_MAX_ROWS`] rows with a fitted `…)`.
fn corner_tool_header_lines(name: &str, args: &str, width: u16) -> Vec<Line<'static>> {
    let corner_cols = cols(TOOL_RESULT_PREFIX);
    let indent = " ".repeat(corner_cols + cols(name) + 1); // under the `(`
    let text = format!("{name}({args})");
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let white = Style::new().fg(TOOL_ARGS_COLOR);
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut budget = (width as usize).saturating_sub(corner_cols).max(1);
    for ch in text.chars() {
        let w = cols(&ch.to_string());
        if cols(&current) + w > budget {
            rows.push(std::mem::take(&mut current));
            budget = (width as usize).saturating_sub(cols(&indent)).max(1);
        }
        current.push(ch);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.len() > TOOL_HEADER_MAX_ROWS {
        rows.truncate(TOOL_HEADER_MAX_ROWS);
        if let Some(last) = rows.last_mut() {
            *last = truncate_cols(
                last,
                (width as usize)
                    .saturating_sub(cols(&indent) + cols(TOOL_HEADER_ELLIPSIS) + 1)
                    .max(1),
            );
            last.push_str(TOOL_HEADER_ELLIPSIS);
            last.push(')');
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
                    Span::styled(row, white),
                ])
            } else {
                Line::from(vec![Span::raw(indent.clone()), Span::styled(row, white)])
            }
        })
        .collect()
}

/// The **live** cell of a lone agent — `● Agent({description})` over its
/// current state instead of a one-row tree (`docs/agent-tool.md`): the
/// running tool's wrapped header + a dim `Running…`, or the sticky
/// `⎿ {activity}` line (`Initializing…` before any event, the last
/// `{Name}: {detail}` between calls).
fn single_live_agent_lines(
    app: &App,
    run: &crate::agents::AgentRun,
    background: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let mut lines = vec![agent_cell_header(TOOL_RUNNING_COLOR, &run.description)];
    let running_tool = run
        .tool_queue
        .front()
        .filter(|tool| tool.status == ToolStatus::Running);
    if let Some(tool) = running_tool {
        lines.extend(corner_tool_header_lines(&tool.name, &tool.args, width));
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(cols(TOOL_RESULT_PREFIX))),
            Span::styled(TOOL_RUNNING.to_string(), dim),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(
                truncate_cols(
                    &run.activity(),
                    (width as usize)
                        .saturating_sub(cols(TOOL_RESULT_PREFIX))
                        .max(1),
                ),
                dim,
            ),
        ]));
    }
    if !background
        && app
            .command_elapsed()
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A **committed** agent group's tree cell (`docs/agent-tool.md`):
/// `● {n} background agents launched (↓ to manage)` over description-only
/// rows for a background launch, else `● {n} agents finished (ctrl+o to
/// expand)` over counter rows with a `⎿ Done` / `⎿ Interrupted` / `⎿ Failed`
/// status row per agent — green bullet when every agent finished cleanly,
/// red otherwise. A **lone** agent keeps the tool-cell look instead:
/// `● Agent({description})` over `⎿ Done ({n} tool uses · {tokens} tokens ·
/// {s}s)` and a dim `(ctrl+o to expand)` line — or
/// `⎿ Running in the background (↓ to manage)` for a lone background launch.
#[must_use]
pub fn agent_group_lines(group: &crate::app::AgentGroup, width: u16) -> Vec<Line<'static>> {
    let color = if group.ok() {
        TOOL_OK_COLOR
    } else {
        TOOL_FAIL_COLOR
    };
    if let [entry] = group.agents.as_slice() {
        let dim = Style::new().fg(TOOL_DIM_COLOR);
        let mut lines = vec![agent_cell_header(color, &entry.description)];
        let (settle, settle_color) = if group.background {
            (TOOL_BACKGROUNDED.to_string(), TOOL_DIM_COLOR)
        } else {
            match entry.status {
                crate::agents::AgentStatus::Done => (
                    agent_done_clause(entry.tool_uses, entry.tokens, entry.secs),
                    TOOL_DIM_COLOR,
                ),
                status if status.is_final() => (status.label().to_string(), TOOL_FAIL_COLOR),
                status => (status.label().to_string(), TOOL_DIM_COLOR),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(settle, Style::new().fg(settle_color)),
        ]));
        if !group.background {
            lines.push(Line::from(vec![
                Span::raw(INDENT.to_string()),
                Span::styled(EXPAND_HINT.trim_start().to_string(), dim),
            ]));
        }
        return lines;
    }
    let mut lines = if group.background {
        vec![agent_group_header(
            color,
            format!(
                "{} background {} launched",
                group.agents.len(),
                if group.agents.len() == 1 {
                    "agent"
                } else {
                    "agents"
                }
            ),
            AGENT_MANAGE_HINT,
        )]
    } else {
        vec![agent_group_header(
            color,
            format!("{} finished", agent_count_phrase(group.agents.len())),
            EXPAND_HINT,
        )]
    };
    let count = group.agents.len();
    for (i, entry) in group.agents.iter().enumerate() {
        let is_last = i + 1 == count;
        if group.background {
            lines.extend(agent_tree_rows(
                is_last,
                &entry.description,
                "",
                None,
                width,
            ));
        } else {
            let status_color = match entry.status {
                s if s.ok() => TOOL_DIM_COLOR,
                crate::agents::AgentStatus::Running | crate::agents::AgentStatus::Pending => {
                    TOOL_DIM_COLOR
                }
                _ => TOOL_FAIL_COLOR,
            };
            lines.extend(agent_tree_rows(
                is_last,
                &entry.description,
                &agent_counters_clause(entry.tool_uses, entry.tokens),
                Some((entry.status.label(), status_color)),
                width,
            ));
        }
    }
    lines
}

/// The **live** agent group's tree cell — the strip preview while the round's
/// agents run: a blue `● Running {n} agents… (ctrl+o to expand)` header over
/// live tree rows (counters ticking, the status row showing each agent's
/// current activity). Rendered from the roster entries the live group names;
/// an id already swept renders nothing (it settled long ago).
#[must_use]
pub fn live_agent_group_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(live) = app.agent_group() else {
        return Vec::new();
    };
    let runs: Vec<&crate::agents::AgentRun> =
        live.ids.iter().filter_map(|id| app.agent(id)).collect();
    if runs.is_empty() {
        return Vec::new();
    }
    // A lone agent keeps the tool-cell look — `● Agent({description})` over
    // its live state — instead of a one-row tree (docs/agent-tool.md).
    if let [run] = runs.as_slice() {
        return single_live_agent_lines(app, run, live.background, width);
    }
    let mut lines = vec![agent_group_header(
        TOOL_RUNNING_COLOR,
        format!("Running {}…", agent_count_phrase(runs.len())),
        EXPAND_HINT,
    )];
    let count = runs.len();
    for (i, run) in runs.iter().enumerate() {
        let activity = run.activity();
        let status_color = match run.status {
            crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
                TOOL_FAIL_COLOR
            }
            _ => TOOL_DIM_COLOR,
        };
        lines.extend(agent_tree_rows(
            i + 1 == count,
            &run.description,
            &agent_counters_clause(run.tool_uses, run.tokens),
            Some((activity.as_str(), status_color)),
            width,
        ));
    }
    // The whole group can be moved to the background with Ctrl+B — the
    // delayed discoverability hint, exactly like a running bash cell's
    // (docs/background.md). Live-only by construction.
    if !live.background
        && app
            .command_elapsed()
            .is_some_and(|elapsed| elapsed >= TOOL_BACKGROUND_HINT_DELAY)
    {
        lines.push(result_row(1, TOOL_BACKGROUND_HINT.to_string()));
    }
    lines
}

/// A background agent's completion notice cell: the coloured `●` — green for
/// a clean finish, red for a stop/failure — over the one-line headline
/// (`Agent "{description}" finished · 35s`). The final response the notice
/// carries is context-only, never rendered. See `docs/agent-tool.md`.
#[must_use]
pub fn agent_notice_lines(notice: &crate::app::AgentNotice, width: u16) -> Vec<Line<'static>> {
    let color = if notice.ok() {
        BG_NOTICE_OK_COLOR
    } else {
        BG_NOTICE_FAIL_COLOR
    };
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    wrap_text(&notice.headline(), content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(AI_BULLET.to_string(), bullet_style),
                    Span::raw(line),
                ])
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(line)])
            }
        })
        .collect()
}

/// What one Ctrl+O agent cell renders — bridged from either a **recorded**
/// [`crate::app::AgentGroupEntry`] or a **live** roster
/// [`crate::agents::AgentRun`], so the two views share one renderer.
struct AgentCellView {
    description: String,
    status: crate::agents::AgentStatus,
    background: bool,
    prompt: String,
    tool_headers: Vec<String>,
    /// The live activity row (`Running…`) — live cells only.
    activity: Option<String>,
    result: String,
    tool_uses: usize,
    tokens: u64,
    secs: u64,
}

impl AgentCellView {
    fn of_entry(entry: &crate::app::AgentGroupEntry, background: bool) -> Self {
        Self {
            description: entry.description.clone(),
            status: entry.status,
            background,
            prompt: entry.prompt.clone(),
            tool_headers: entry.tool_headers.clone(),
            activity: None,
            result: entry.result.clone(),
            tool_uses: entry.tool_uses,
            tokens: entry.tokens,
            secs: entry.secs,
        }
    }

    fn of_run(run: &crate::agents::AgentRun) -> Self {
        let mut tool_headers: Vec<String> = run
            .history
            .iter()
            .filter_map(|item| match item {
                HistoryItem::Tool(tool) => Some(format!("{}({})", tool.name, tool.args)),
                _ => None,
            })
            .collect();
        for tool in &run.tool_queue {
            tool_headers.push(format!("{}({})", tool.name, tool.args));
        }
        Self {
            description: run.description.clone(),
            status: run.status,
            background: run.background,
            prompt: run.prompt.clone(),
            tool_headers,
            activity: (!run.status.is_final()).then(|| match run.status {
                crate::agents::AgentStatus::Pending => "Initializing…".to_string(),
                _ => "Running…".to_string(),
            }),
            result: run.result.clone().unwrap_or_default(),
            tool_uses: run.tool_uses,
            tokens: run.tokens,
            secs: run.runtime.as_secs(),
        }
    }
}

/// One agent's expanded Ctrl+O cell: the `● Agent({description})` header
/// (bullet coloured by status), the `⎿ Prompt:` block, the nested tool-call
/// headers it ran, the `⎿ Response:` block once a final response exists, and
/// the `⎿ Done ({n} tool uses · {tokens} tokens · {s}s)` /
/// `⎿ Interrupted` / `⎿ Failed` footer. See `docs/agent-tool.md`.
fn agent_cell_lines(cell: &AgentCellView, width: u16) -> Vec<Line<'static>> {
    let bullet_color = match cell.status {
        crate::agents::AgentStatus::Done => TOOL_OK_COLOR,
        crate::agents::AgentStatus::Failed | crate::agents::AgentStatus::Interrupted => {
            TOOL_FAIL_COLOR
        }
        _ => TOOL_RUNNING_COLOR,
    };
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let white = Style::new().fg(TOOL_OUTPUT_COLOR);
    let section = Style::new()
        .fg(AGENT_SECTION_COLOR)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            TOOL_BULLET.to_string(),
            Style::new().fg(bullet_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Agent".to_string(),
            Style::new()
                .fg(TOOL_NAME_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({})", cell.description),
            Style::new().fg(TOOL_ARGS_COLOR),
        ),
    ])];
    // ⎿  Prompt: over the indented prompt body.
    lines.push(Line::from(vec![
        Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
        Span::styled(AGENT_PROMPT_LABEL.to_string(), section),
    ]));
    let body_width = width.saturating_sub(cols(AGENT_BODY_INDENT) as u16).max(1);
    for row in wrap_text(&cell.prompt, body_width) {
        lines.push(Line::from(vec![
            Span::raw(AGENT_BODY_INDENT.to_string()),
            Span::styled(row, white),
        ]));
    }
    // The nested tool calls it ran (headers only — the agent session view has
    // the full cells), then the live activity row.
    if !cell.tool_headers.is_empty() || cell.activity.is_some() {
        lines.push(Line::default());
        let nested_width = width
            .saturating_sub(cols(AGENT_NESTED_INDENT) as u16)
            .max(1);
        for header in &cell.tool_headers {
            for (i, row) in wrap_text(header, nested_width).into_iter().enumerate() {
                let indent = if i == 0 {
                    AGENT_NESTED_INDENT.to_string()
                } else {
                    format!("{AGENT_NESTED_INDENT}  ")
                };
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(row, white),
                ]));
            }
        }
        if let Some(activity) = &cell.activity {
            lines.push(Line::from(vec![
                Span::raw(AGENT_NESTED_INDENT.to_string()),
                Span::styled(activity.clone(), dim),
            ]));
        }
    }
    // ⎿  Response: once a final response exists.
    if !cell.result.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(AGENT_RESPONSE_LABEL.to_string(), section),
        ]));
        for row in wrap_text(&cell.result, body_width) {
            lines.push(Line::from(vec![
                Span::raw(AGENT_BODY_INDENT.to_string()),
                Span::styled(row, white),
            ]));
        }
    }
    // The settle footer.
    let footer: Option<(String, Color)> = match cell.status {
        crate::agents::AgentStatus::Done => Some((
            agent_done_clause(cell.tool_uses, cell.tokens, cell.secs),
            TOOL_DIM_COLOR,
        )),
        crate::agents::AgentStatus::Interrupted => {
            Some(("Interrupted".to_string(), TOOL_FAIL_COLOR))
        }
        crate::agents::AgentStatus::Failed => Some(("Failed".to_string(), TOOL_FAIL_COLOR)),
        _ if cell.background => Some((TOOL_BACKGROUNDED.to_string(), TOOL_DIM_COLOR)),
        _ => None,
    };
    if let Some((text, color)) = footer {
        lines.push(Line::from(vec![
            Span::styled(TOOL_RESULT_PREFIX.to_string(), dim),
            Span::styled(text, Style::new().fg(color)),
        ]));
    }
    lines
}

/// A committed [`crate::app::AgentGroup`]'s Ctrl+O expansion: one
/// [`agent_cell_lines`] cell per entry, blank-separated.
fn agent_group_full_lines(group: &crate::app::AgentGroup, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, entry) in group.agents.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(agent_cell_lines(
            &AgentCellView::of_entry(entry, group.background),
            width,
        ));
    }
    lines
}

/// How many rows the footer's agent roster occupies: 0 with no visible
/// agents, else a blank spacer + the `● main` row + one row per agent.
/// [`live_height`] adds this below the footer; [`render_live`] paints exactly
/// these rows.
#[must_use]
pub fn agent_list_rows(app: &App) -> u16 {
    let agents = app.visible_agents().len();
    if agents == 0 {
        return 0;
    }
    u16::try_from(2 + agents).unwrap_or(u16::MAX)
}

/// The footer roster: a blank spacer, the `● main` row, then one
/// `◯ {type}  {description} {elapsed} · ↓ {tokens} tokens` row per visible
/// agent — the `❯` selection marker on the active row, the viewed session
/// bold, finished agents' `◯` coloured green/red for their linger. See
/// `docs/agent-tool.md`.
#[must_use]
pub fn agent_list_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let agents = app.visible_agents();
    if agents.is_empty() {
        return Vec::new();
    }
    let dim = Style::new().fg(TOOL_DIM_COLOR);
    let selection = app.agent_selection();
    let mut lines = vec![Line::default()];
    // The `● main` row.
    let main_selected = selection == Some(0);
    let main_viewed = app.agent_view.is_none();
    let marker = if main_selected {
        AGENT_LIST_MARKER
    } else {
        AGENT_LIST_INDENT
    };
    let mut main_style = if main_selected {
        Style::new().fg(MENU_SELECTED_COLOR)
    } else if main_viewed {
        Style::new().fg(TOOL_OUTPUT_COLOR)
    } else {
        dim
    };
    if main_viewed {
        main_style = main_style.add_modifier(Modifier::BOLD);
    }
    lines.push(Line::from(vec![
        Span::styled(marker.to_string(), Style::new().fg(MENU_SELECTED_COLOR)),
        Span::styled(AGENT_MAIN_BULLET.to_string(), main_style),
        Span::styled(AGENT_MAIN_LABEL.to_string(), main_style),
    ]));
    for (i, run) in agents.iter().enumerate() {
        let selected = selection == Some(i + 1);
        let viewed = app.agent_view.as_deref() == Some(run.id.as_str());
        // The `❯` marks the explicit selection — or, with none active, the
        // agent whose session view is open (the user's reference look).
        let marker = if selected || (selection.is_none() && viewed) {
            AGENT_LIST_MARKER
        } else {
            AGENT_LIST_INDENT
        };
        let bullet_style = match run.status {
            s if s.is_final() && s.ok() => Style::new().fg(TOOL_OK_COLOR),
            s if s.is_final() => Style::new().fg(TOOL_FAIL_COLOR),
            _ if selected => Style::new().fg(MENU_SELECTED_COLOR),
            _ => dim,
        };
        let mut text_style = if selected {
            Style::new().fg(MENU_SELECTED_COLOR)
        } else {
            dim
        };
        if viewed {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        // `{type}  {description}` truncated so the ` {elapsed} · ↓ {n} tokens`
        // suffix always fits.
        let mut suffix = format!(" {}", format_elapsed(run.runtime.as_secs()));
        if run.tokens > 0 {
            suffix.push_str(&format!(
                " · {} {} tokens",
                STATUS_ARROW_DOWN,
                format_token_count(usize::try_from(run.tokens).unwrap_or(usize::MAX))
            ));
        }
        let lead = format!("{}{}", marker, AGENT_ROW_BULLET);
        let budget = (width as usize)
            .saturating_sub(cols(&lead) + cols(&suffix))
            .max(1);
        let mut name = format!("{}  {}", run.agent_type, run.description);
        if cols(&name) > budget {
            name = truncate_cols(&name, budget.saturating_sub(1));
            name.push('…');
        }
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::new().fg(MENU_SELECTED_COLOR)),
            Span::styled(AGENT_ROW_BULLET.to_string(), bullet_style),
            Span::styled(name, text_style),
            Span::styled(suffix, dim),
        ]));
    }
    lines
}

/// The roster selection's footer hint line — `↑/↓ to select · Enter to view`
/// on the `● main` row, `Enter to view · x to stop` on an agent row — taking
/// the footer's slot while the selection is active (the shell-mode-line
/// pattern).
#[must_use]
pub fn agent_hint_line(app: &App) -> Line<'static> {
    let entries = if app.agent_selection() == Some(0) {
        AGENT_HINT_MAIN
    } else {
        AGENT_HINT_AGENT
    };
    let mut spans = vec![Span::raw(FOOTER_INDENT)];
    for (i, (key, label)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                FOOTER_SEPARATOR.to_string(),
                Style::new().fg(FOOTER_COLOR),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().fg(SHORTCUTS_KEY_COLOR),
        ));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::new().fg(FOOTER_COLOR),
        ));
    }
    Line::from(spans)
}

/// The synthesized status for an **agent session view**'s strip — the viewed
/// agent's own spinner line (`Working… (elapsed · ↓ tokens · esc to
/// interrupt)` shape, without the interrupt hint's meaning changing: Esc
/// leaves the view). Built per draw from the roster entry.
#[must_use]
pub fn agent_view_status(run: &crate::agents::AgentRun) -> crate::app::TurnStatus {
    crate::app::TurnStatus {
        verb: "Working",
        done_verb: "Done",
        tokens: usize::try_from(run.tokens).unwrap_or(usize::MAX),
        arrow: crate::app::TokenArrow::Down,
        elapsed: run.runtime,
        thinking: None,
        shell: false,
        retry: None,
    }
}

/// The agent session view's strip preview: the viewed agent's live tool
/// cells (the batch queue, blank-separated) or its streaming reply's last
/// row. Empty when idle. The [`preview_lines`]/[`preview_rows`] pair calls
/// this for a viewed agent so the two agree.
fn agent_view_preview_lines(run: &crate::agents::AgentRun, width: u16) -> Vec<Line<'static>> {
    if !run.tool_queue.is_empty() {
        let mut lines = Vec::new();
        for (i, tool) in run.tool_queue.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.extend(tool_lines(tool, width));
        }
        return lines;
    }
    run.streaming
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|text| {
            message_lines(Role::Assistant, text, width)
                .pop()
                .unwrap_or_default()
        })
        .into_iter()
        .collect()
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

/// Build the full conversation transcript shown in the tool-output view: the
/// startup header banner (docs/header.md — the overlay mirrors the inline
/// scrollback, which opens with it), then every user/assistant/error message
/// **and** every tool call's complete output, interleaved in the exact order
/// they happened (straight from `App::history`), followed by the live tail —
/// the in-progress reply and/or the running tool. A blank line separates
/// items. Tools are shown *expanded* here (the inline view collapses them).
/// Empty → the banner over a single placeholder line.
///
/// Only the **user** message shows its wall-clock `timestamp`: dim,
/// right-aligned on its own line below the message ([`user_stamp_lines`]) — the
/// **only** stamp displayed anywhere (AI replies, tools, and turn summaries
/// record one but never show it; the inline view never shows any; see
/// `docs/timestamps.md`).
#[must_use]
pub fn transcript_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    transcript_build(app, width).0
}

/// The line range the backtrack preview's highlighted user message occupies
/// in [`transcript_lines`]'s output — the scroll-into-view target
/// ([`backtrack_scroll`]); `None` when no preview is active. Computed by the
/// same walk that styles the highlight ([`transcript_build`]), so the two can
/// never drift. See `docs/backtrack.md`.
#[must_use]
pub fn transcript_selection(app: &App, width: u16) -> Option<Range<usize>> {
    transcript_build(app, width).1
}

/// The single transcript walk behind [`transcript_lines`] and
/// [`transcript_selection`]: a fresh [`TranscriptCache`] refreshed once — the
/// incremental build *is* the only transcript implementation (one code path,
/// so the cached and from-scratch renders can never drift apart; the draw loop
/// reuses a long-lived cache instead of paying this full build).
fn transcript_build(app: &App, width: u16) -> (Vec<Line<'static>>, Option<Range<usize>>) {
    let mut cache = TranscriptCache::new();
    cache.refresh(app, width);
    (std::mem::take(&mut cache.lines), cache.selection)
}

/// One history item's transcript rows — the message (or the tool's *expanded*
/// output, or a summary/background notice) plus its trailing blank spacer,
/// self-contained so [`TranscriptCache`] can render each committed item
/// exactly once and only ever append. The spacer is skipped after a shell
/// command's header ([`is_shell_header`]): its tool's `⎿` output sits flush
/// below it, whether that tool is already in history or still the running
/// live tail, so the overlay renders the same exec cell as the inline view.
///
/// The second value is `Some(message-row count)` for a **user** message — the
/// span the Esc-Esc backtrack preview reverses (its timestamp line below stays
/// normal; see `docs/backtrack.md`) — `None` for everything else.
fn transcript_item_lines(item: &HistoryItem, width: u16) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut user_rows = None;
    match item {
        HistoryItem::Message(m) => {
            let message = message_lines(m.role, &m.text, width);
            if m.role == Role::User {
                user_rows = Some(message.len());
            }
            lines.extend(message);
            if m.role == Role::User {
                lines.extend(user_stamp_lines(&m.timestamp, width));
            }
        }
        HistoryItem::Tool(t) => lines.extend(tool_full_lines(t, width)),
        HistoryItem::Summary(s) => lines.extend(summary_lines(s, width)),
        HistoryItem::Background(n) => lines.extend(background_notice_lines(n, width)),
        // The transcript expands the group into one `● Agent({description})`
        // cell per subagent — prompt, nested tool headers, response, and the
        // Done/Interrupted footer (docs/agent-tool.md); the inline view shows
        // the collapsed tree cell.
        HistoryItem::AgentGroup(g) => lines.extend(agent_group_full_lines(g, width)),
        HistoryItem::AgentNotice(n) => lines.extend(agent_notice_lines(n, width)),
        // The transcript expands the marker with its summary body — the
        // inline view keeps it collapsed (docs/compact.md).
        HistoryItem::Compaction(c) => lines.extend(compaction_full_lines(c, width)),
    }
    if !is_shell_header(item) {
        lines.push(Line::default());
    }
    (lines, user_rows)
}

/// The Ctrl+O transcript of the **viewed agent's** session
/// (`docs/agent-tool.md`), or `None` when no agent view is up (the caller
/// falls back to the main [`TranscriptCache`]). The banner over the agent's
/// items (tools expanded, exactly like the main walk) and its live tail —
/// the in-progress reply and the live tool queue. Built fresh per draw: an
/// agent transcript is bounded by one task's work, so the incremental cache
/// isn't warranted.
#[must_use]
pub fn agent_transcript_lines(app: &App, width: u16) -> Option<Vec<Line<'static>>> {
    let run = app.viewed_agent()?;
    let mut lines = header_lines(app, width);
    lines.push(Line::default());
    let chrome_rows = lines.len();
    for item in &run.history {
        let (rows, _) = transcript_item_lines(item, width);
        lines.extend(rows);
    }
    if let Some(text) = run.streaming.as_deref().filter(|text| !text.is_empty()) {
        lines.extend(message_lines(Role::Assistant, text, width));
        lines.push(Line::default());
    }
    for tool in &run.tool_queue {
        lines.extend(tool_full_lines(tool, width));
        lines.push(Line::default());
    }
    if lines.len() == chrome_rows {
        lines.push(Line::from(Span::styled(
            TOOL_VIEW_EMPTY.to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        )));
    }
    Some(lines)
}

/// The pager's scrolling-body height: the screen less the title + footer chrome.
#[must_use]
pub fn tool_view_body_rows(screen_height: u16) -> usize {
    screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize
}

/// The largest scroll offset for a transcript of `line_count` rows — the content
/// height minus the body window, so the last line can reach the bottom but not
/// past it. Pure over the count so the caller can reuse a cached line build (see
/// [`TranscriptCache`]) instead of rebuilding just to clamp.
#[must_use]
pub fn tool_view_max_scroll_for(line_count: usize, screen_height: u16) -> usize {
    line_count.saturating_sub(tool_view_body_rows(screen_height))
}

/// The largest the transcript scroll offset can be on a `screen_height`-row
/// screen. Convenience over [`tool_view_max_scroll_for`] that builds the
/// transcript itself (the draw loop instead reuses [`TranscriptCache`]).
#[must_use]
pub fn tool_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    tool_view_max_scroll_for(transcript_lines(app, width).len(), screen_height)
}

/// Caches the Ctrl+O overlay's built transcript **incrementally**, so opening
/// the overlay, scrolling it, and following a live stream under it all avoid
/// re-rendering history — that walk is O(history) and, with real grammar
/// highlighting over every expanded tool cell, cost hundreds of ms on a big
/// resumed session (the old open re-highlighted everything on a blank alt
/// screen). Owned by the event loop like [`StreamRender`]; it is **retained
/// across overlay closes** (reopening is O(live tail)) and pre-warmed at the
/// boundary ([`warm`]) so the open itself renders nothing.
///
/// Committed history items are immutable and history only ever grows — every
/// other mutation (a `/clear`, a `/resume` load, a backtrack truncation, an
/// interrupt-undo pop) bumps [`App::history_generation`]. That makes
/// `(generation, width)` pin the **frozen prefix** exactly: `lines[..frozen_rows]`
/// holds the banner chrome plus every committed item, rendered once
/// ([`transcript_item_lines`]); each refresh truncates the volatile live tail
/// (in-progress reply, live tool queue, queued backlog) off the end and
/// re-renders just that. The Esc-Esc backtrack highlight — REVERSED rows
/// *inside* the frozen prefix — is applied as an in-place style diff
/// ([`Self::restyle_selection`]), never a re-render. A cheap [`TranscriptSig`]
/// short-circuits the refresh entirely while nothing changed, so a scroll
/// keypress is O(viewport).
///
/// [`warm`]: Self::warm
#[derive(Default)]
pub struct TranscriptCache {
    /// Pins the frozen prefix: `(history generation, width, session cwd)` —
    /// any mismatch invalidates every rendered item (the cwd feeds the banner
    /// chrome above them). `None` until the first build.
    key: Option<FrozenKey>,
    sig: Option<TranscriptSig>,
    /// The full transcript: the frozen prefix (`..frozen_rows`) + the live tail.
    lines: Vec<Line<'static>>,
    /// Rows of banner chrome at the top of `lines` (the empty-transcript
    /// placeholder keys on "nothing beyond the banner").
    chrome_rows: usize,
    /// End of the frozen prefix in `lines`; the live tail is rebuilt above it.
    frozen_rows: usize,
    /// Per rendered history item: its row count (for selection offsets) and,
    /// for a user message, the reversible message-row span of the backtrack
    /// preview ([`transcript_item_lines`]).
    items: Vec<RenderedItem>,
    /// The frozen rows currently carrying the backtrack preview's REVERSED
    /// styling, so a selection step can undo exactly what it applied.
    reversed: Option<Range<usize>>,
    selection: Option<Range<usize>>,
    /// Test-only: how many refreshes did any rebuild work — so a test can
    /// prove a scroll (unchanged signature) is a cache hit, not a rebuild.
    #[cfg(test)]
    builds: usize,
    /// Test-only: how many history items were rendered, ever — so a test can
    /// prove the frozen prefix is reused, not re-rendered.
    #[cfg(test)]
    item_renders: usize,
}

/// What pins [`TranscriptCache`]'s frozen prefix — see the struct docs.
#[derive(PartialEq, Eq)]
struct FrozenKey {
    generation: u64,
    width: u16,
    cwd: Option<String>,
}

/// One rendered history item's shape inside the frozen prefix.
struct RenderedItem {
    /// Rows this item occupies (message + stamp + spacer / expanded tool cell).
    rows: usize,
    /// `Some(message-row count)` for a user message — the span the backtrack
    /// preview reverses; `None` otherwise.
    user_rows: Option<usize>,
}

/// The cheap fingerprint of every input to a [`TranscriptCache`] refresh — see
/// the struct docs for why lengths suffice (append-only history, plus the
/// generation catching every non-append mutation; a same-length replace after
/// an interrupt-undo pop would fool lengths alone).
#[derive(PartialEq, Eq)]
struct TranscriptSig {
    generation: u64,
    width: u16,
    history_len: usize,
    /// Display length of the session cwd shown in the banner chrome (`None`
    /// before the boundary injects it) — set once at startup, but cheap to
    /// fingerprint, so a late injection can't leave a stale banner.
    cwd_len: Option<usize>,
    /// Live in-progress reply length (`None` when not streaming).
    streaming_len: Option<usize>,
    /// The live tool queue's shape: `(number of live calls, front call status,
    /// front output length)`, `None` when none run. A **parallel batch** shrinks
    /// as each call commits (also bumping `history_len`) and the front flips
    /// `Waiting`→`Running` when it starts. The **front output length** changes as
    /// a running `bash` call **streams** its output
    /// ([`crate::stream::StreamEvent::ToolOutput`]) — so the Ctrl+O overlay
    /// rebuilds and tails the live output rather than showing a frozen snapshot
    /// (`docs/tool-streaming.md`); without it a single streaming call leaves the
    /// queue length and status unchanged and the overlay would go static. See
    /// `docs/parallel-tools.md`.
    tool_queue: Option<(usize, ToolStatus, usize)>,
    queued_len: usize,
    backtrack_selected: Option<usize>,
    /// The subagent roster's mutation counter — a live agent streaming (its
    /// tree counters, its Ctrl+O cell) invalidates the tail exactly like a
    /// streaming tool (docs/agent-tool.md).
    agents_generation: u64,
}

impl TranscriptSig {
    fn of(app: &App, width: u16) -> Self {
        let queue = app.tool_queue();
        Self {
            generation: app.history_generation(),
            width,
            history_len: app.history.len(),
            cwd_len: app.session.as_ref().map(|s| s.cwd.len()),
            streaming_len: app.streaming_text().map(str::len),
            tool_queue: queue
                .front()
                .map(|t| (queue.len(), t.status, t.output.len())),
            queued_len: app.queued.len(),
            backtrack_selected: app.backtrack.selected,
            agents_generation: app.agents_generation(),
        }
    }
}

impl TranscriptCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop the whole cache — the rendered transcript with it. Not part of the
    /// overlay lifecycle (the cache is deliberately retained across closes so
    /// reopening stays O(live tail)); for a caller that wants the memory back.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Pre-render the frozen prefix at the boundary, ahead of any overlay
    /// draw — after a `/resume` load, after each committed item — so pressing
    /// Ctrl+O finds every history item already rendered and pays only the
    /// live tail. When nothing changed this is a few integer compares.
    ///
    /// A **width-only** mismatch is deliberately skipped: resize events arrive
    /// in bursts (a drag delivers dozens), and a full O(history) re-render per
    /// event would freeze the loop — the first overlay draw at the new width
    /// pays the rebuild instead (behind the still-painted inline screen).
    pub fn warm(&mut self, app: &App, width: u16) {
        let width_only_miss = self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
                && k.width != width
        });
        if width_only_miss {
            return;
        }
        if self.ensure_frozen(app, width) {
            // The tail was truncated off (or the prefix reset): the next
            // refresh must rebuild it even if the volatile signature matches.
            self.sig = None;
        }
    }

    /// Bring the frozen prefix up to date with `app.history` at `width`:
    /// reset it when the [`FrozenKey`] mismatches, then render and append any
    /// items not yet cached. Returns whether anything changed; when it did,
    /// `lines` holds **only** chrome + frozen items (the live tail was
    /// truncated off) and the caller must rebuild the tail.
    fn ensure_frozen(&mut self, app: &App, width: u16) -> bool {
        let key_matches = self.key.as_ref().is_some_and(|k| {
            k.generation == app.history_generation()
                && k.width == width
                && k.cwd.as_deref() == app.session.as_ref().map(|s| s.cwd.as_str())
        });
        if !key_matches {
            self.key = Some(FrozenKey {
                generation: app.history_generation(),
                width,
                cwd: app.session.as_ref().map(|s| s.cwd.clone()),
            });
            // The header banner tops the transcript exactly as it tops the
            // inline conversation (docs/header.md) — the overlay mirrors the
            // real scrollback, so Ctrl+O never hides it.
            self.lines = header_lines(app, width);
            self.lines.push(Line::default());
            self.chrome_rows = self.lines.len();
            self.frozen_rows = self.chrome_rows;
            self.items.clear();
            self.reversed = None;
        } else if self.items.len() == app.history.len() {
            return false;
        } else {
            self.lines.truncate(self.frozen_rows);
        }
        // Append the not-yet-rendered items (all of them after a reset). The
        // generation guarantees the cached prefix is a prefix of `history`.
        for item in app.history.get(self.items.len()..).unwrap_or(&[]) {
            let (rows, user_rows) = transcript_item_lines(item, width);
            self.items.push(RenderedItem {
                rows: rows.len(),
                user_rows,
            });
            self.frozen_rows += rows.len();
            self.lines.extend(rows);
            #[cfg(test)]
            {
                self.item_renders += 1;
            }
        }
        true
    }

    /// Apply the backtrack preview's REVERSED highlight to the selected user
    /// message's rows — an in-place style diff on the frozen prefix (undo the
    /// old span, style the new), exactly mirroring the styling a from-scratch
    /// build applies, so stepping the selection never re-renders anything.
    fn restyle_selection(&mut self, app: &App) {
        let target = app.backtrack.selected.and_then(|ordinal| {
            let mut seen = 0usize;
            let mut row = self.chrome_rows;
            for item in &self.items {
                if let Some(user_rows) = item.user_rows {
                    if seen == ordinal {
                        return Some(row..row + user_rows);
                    }
                    seen += 1;
                }
                row += item.rows;
            }
            None
        });
        if self.reversed != target {
            if let Some(old) = self.reversed.take() {
                for line in &mut self.lines[old] {
                    line.style.add_modifier.remove(Modifier::REVERSED);
                }
            }
            if let Some(new) = target.clone() {
                for line in &mut self.lines[new] {
                    line.style.add_modifier.insert(Modifier::REVERSED);
                }
            }
            self.reversed = target.clone();
        }
        self.selection = target;
    }

    /// Rebuild the live tail above the frozen prefix: the in-progress
    /// assistant text, then every live tool call — the running one followed by
    /// any `⎿ Waiting…` siblings of a parallel batch, in order, so the overlay
    /// shows the full live picture and a waiting call is never hidden under
    /// Ctrl+O (docs/parallel-tools.md) — then the still-queued backlog
    /// ([`queued_lines`]' inset rows, docs/queue.md), and the placeholder when
    /// nothing at all follows the banner.
    fn build_tail(&mut self, app: &App, width: u16) {
        if let Some(text) = app.streaming_text()
            && !text.is_empty()
        {
            self.lines
                .extend(message_lines(Role::Assistant, text, width));
            self.lines.push(Line::default());
        }
        // The live agent group's members expand as their own `● Agent(…)`
        // cells — activity live — before the tool queue, mirroring the strip's
        // order (docs/agent-tool.md). Committed groups render from history.
        if let Some(live) = app.agent_group() {
            for id in &live.ids {
                if let Some(run) = app.agent(id) {
                    self.lines
                        .extend(agent_cell_lines(&AgentCellView::of_run(run), width));
                    self.lines.push(Line::default());
                }
            }
        }
        for tool in app.tool_queue() {
            self.lines.extend(tool_full_lines(tool, width));
            self.lines.push(Line::default());
        }
        if !app.queued.is_empty() {
            self.lines.extend(queued_lines(app, width));
            self.lines.push(Line::default());
        }
        if self.lines.len() == self.chrome_rows {
            self.lines.push(Line::from(Span::styled(
                TOOL_VIEW_EMPTY.to_string(),
                Style::new().fg(TOOL_DIM_COLOR),
            )));
        }
    }

    /// Rebuild what changed since the last call — nothing when the signature
    /// matches, otherwise the frozen-prefix append + selection restyle + live
    /// tail (see the struct docs).
    fn refresh(&mut self, app: &App, width: u16) {
        let sig = TranscriptSig::of(app, width);
        if self.sig.as_ref() == Some(&sig) {
            return;
        }
        if !self.ensure_frozen(app, width) {
            // Only the volatile tail changed: drop it, keep the frozen prefix.
            self.lines.truncate(self.frozen_rows);
        }
        self.restyle_selection(app);
        self.build_tail(app, width);
        self.sig = Some(sig);
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }

    /// The cached transcript rows, rebuilding first only if stale.
    pub fn lines(&mut self, app: &App, width: u16) -> &[Line<'static>] {
        self.refresh(app, width);
        &self.lines
    }

    /// The cached row count (for the scroll clamp) — no borrow held.
    pub fn line_count(&mut self, app: &App, width: u16) -> usize {
        self.refresh(app, width);
        self.lines.len()
    }

    /// The cached backtrack-preview selection range, rebuilding first if stale.
    pub fn selection(&mut self, app: &App, width: u16) -> Option<Range<usize>> {
        self.refresh(app, width);
        self.selection.clone()
    }
}

/// Where the transcript scroll must sit to show `target` in a `viewport`-row
/// window: up to its top when it is above, down just enough when it is below,
/// unmoved when already visible — and its top when it is taller than the
/// window (codex's `scroll_chunk_into_view`).
fn scroll_into_view(current: usize, target: &Range<usize>, viewport: usize) -> usize {
    if target.start < current {
        target.start
    } else if target.end > current + viewport {
        target.end.saturating_sub(viewport).min(target.start)
    } else {
        current
    }
}

/// The `tool_scroll` that brings a backtrack preview's `selection` range into a
/// `screen_height`-row pager, given the `current` scroll — or `None` when there
/// is no selection. Pure over the range so the draw loop can pass a cached
/// selection ([`TranscriptCache::selection`]) instead of rebuilding.
#[must_use]
pub fn backtrack_scroll_for(
    selection: Option<Range<usize>>,
    current: usize,
    screen_height: u16,
) -> Option<usize> {
    let range = selection?;
    Some(scroll_into_view(
        current,
        &range,
        tool_view_body_rows(screen_height),
    ))
}

/// The overlay draw's scroll decision while a backtrack preview is active:
/// the `tool_scroll` that brings the highlighted user message into the
/// pager's body window, or `None` with no selection. Convenience over
/// [`backtrack_scroll_for`] that builds the selection itself (the draw loop
/// reuses [`TranscriptCache`]). See `docs/backtrack.md`.
#[must_use]
pub fn backtrack_scroll(app: &App, width: u16, screen_height: u16) -> Option<usize> {
    backtrack_scroll_for(
        transcript_selection(app, width),
        app.tool_scroll,
        screen_height,
    )
}

/// The pager's title row: `/ ` tiled across the width (a `/` on every even
/// column) with the spaced-caps `/ T R A N S C R I P T` overlaid from the left
/// edge, all dim — codex's transcript overlay header.
fn tool_view_header(width: u16) -> Line<'static> {
    overlay_header(TOOL_VIEW_TITLE, width)
}

/// A full-screen overlay's title row: the slash tiling with the spaced-caps
/// `title` overlaid from the left edge, all dim — shared by the transcript
/// pager and the `/resume` picker.
fn overlay_header(title: &str, width: u16) -> Line<'static> {
    let title = format!("/ {title}");
    let mut text: String = title.chars().take(width as usize).collect();
    for col in cols(&text)..width as usize {
        text.push(if col.is_multiple_of(2) { '/' } else { ' ' });
    }
    Line::from(Span::styled(text, Style::new().fg(TOOL_DIM_COLOR)))
}

/// The pager's bottom rule: a dim `─` separator carrying the scroll position
/// as a right-aligned ` {pct}% ` one dash in from the right edge — 0% at the
/// top, 100% at the bottom (or whenever everything fits) — codex's transcript
/// overlay bottom bar.
fn tool_view_separator(width: u16, scroll: usize, max: usize) -> Line<'static> {
    let pct = if max == 0 {
        100
    } else {
        (scroll.min(max) * 100 + max / 2) / max
    };
    rule_with_label(width, &format!(" {pct}% "))
}

/// A dim full-width `─` rule with `label` embedded right-aligned one dash in
/// from the edge — the bottom bar shared by the transcript pager (its scroll
/// percentage) and the `/resume` picker (its selection count).
fn rule_with_label(width: u16, label: &str) -> Line<'static> {
    let width = width as usize;
    let mut rule = vec!['─'; width];
    let start = width.saturating_sub(cols(label) + 1);
    for (i, ch) in label.chars().enumerate() {
        if let Some(cell) = rule.get_mut(start + i) {
            *cell = ch;
        }
    }
    Line::from(Span::styled(
        rule.into_iter().collect::<String>(),
        Style::new().fg(TOOL_DIM_COLOR),
    ))
}

/// Render the full-screen tool-output view — codex's Ctrl+T transcript pager:
/// the slash-tiled title row, then the scrolling conversation transcript
/// (messages + every tool call's full output), windowed by `App::tool_scroll`
/// (clamped so it can't run past the end) with `~` filler on the body rows
/// past its end, then the percentage separator and the dim key-hint rows.
/// Pure — `term.rs` paints this onto the overlay.
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App, lines: &[Line<'static>]) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(tool_view_header(area.width)).render(title_area, buf);

    // `lines` is prebuilt (by the caller's `TranscriptCache`) at `area.width` —
    // the vertical split above keeps the full width, so `body_area.width` matches.
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
    let mut visible: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .cloned()
        .collect();
    while (visible.len() as u16) < body_area.height {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(tool_view_separator(area.width, scroll, max)).render(sep_area, buf);

    let dim = Style::new().fg(TOOL_DIM_COLOR);
    // While a backtrack preview highlights a message, the close-hint row
    // shows the preview's keys instead (codex's highlighted-pager footer —
    // docs/backtrack.md); the scroll keys above keep working either way.
    let closing = if app.backtrack.selected.is_some() {
        TOOL_VIEW_HINT_BACKTRACK
    } else {
        TOOL_VIEW_HINT_QUIT
    };
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(closing.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

/// The role-tag colour of one context entry (see the `CONTEXT_*` consts).
const fn context_role_color(role: crate::context::ContextRole) -> Color {
    match role {
        crate::context::ContextRole::User => CONTEXT_USER_COLOR,
        crate::context::ContextRole::Assistant => CONTEXT_ASSISTANT_COLOR,
        crate::context::ContextRole::System => CONTEXT_SYSTEM_COLOR,
        crate::context::ContextRole::Tool => CONTEXT_TOOL_COLOR,
    }
}

/// One context entry's rows: the coloured `role:` tag, the raw text wrapped
/// **verbatim** (never the markdown renderer — the whole point is showing the
/// unformatted wire content), any native tool calls as `→ name(arguments)`
/// rows, any attachment paths dim beneath, and a blank spacer. An empty text
/// (an assistant entry that only called tools) contributes no text row.
fn context_entry_lines(
    lines: &mut Vec<Line<'static>>,
    tag: &str,
    color: Color,
    text: &str,
    tool_calls: &[crate::context::ContextToolCall],
    images: &[std::path::PathBuf],
    width: u16,
) {
    lines.push(Line::from(Span::styled(
        tag.to_string(),
        Style::new().fg(color),
    )));
    let text_width = width.saturating_sub(cols(CONTEXT_INDENT) as u16);
    if !text.is_empty() {
        // Tool results ride this path too — expand their tabs for display
        // (they paint as zero cells otherwise; see `tool_output_lines`).
        for row in wrap_verbatim(&expand_code_tabs(text), text_width) {
            lines.push(Line::from(format!("{CONTEXT_INDENT}{row}")));
        }
    }
    for call in tool_calls {
        // The native tool request in its raw wire form: `→ name(arguments)`.
        let rendered = format!(
            "{CONTEXT_TOOL_CALL_PREFIX}{}({})",
            call.name, call.arguments
        );
        for row in wrap_verbatim(&rendered, text_width) {
            lines.push(Line::from(Span::styled(
                format!("{CONTEXT_INDENT}{row}"),
                Style::new().fg(CONTEXT_TOOL_COLOR),
            )));
        }
    }
    for path in images {
        // Wrapped like the text — a long temp path must not clip off-screen.
        let label = format!("{CONTEXT_IMAGE_LABEL}{}", path.display());
        for row in wrap_verbatim(&label, text_width) {
            lines.push(Line::from(Span::styled(
                format!("{CONTEXT_INDENT}{row}"),
                Style::new().fg(TOOL_DIM_COLOR),
            )));
        }
    }
    lines.push(Line::default());
}

/// The Ctrl+D body: the raw context window, oldest first — the system prompt
/// (when the backend sends one), then every message
/// [`crate::context::context_messages`] derives from the history. What you
/// read here is what [`crate::llm::backend::build_messages`] sends (the
/// attachments as their paths rather than encoded bytes). A dim placeholder
/// when there is nothing yet. See `docs/context.md`.
#[must_use]
pub fn context_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    // An agent session view debugs the *viewed agent's* context: its own
    // transcript derived through the same mapping (the shared persona prompt
    // stands in for the subagent's — it differs only by the subagent note),
    // with no AGENTS.md fragment (subagents get none). See
    // `docs/agent-tool.md`.
    let (history, instructions) = match app.viewed_agent() {
        Some(run) => (run.history.as_slice(), None),
        None => (app.history.as_slice(), app.user_instructions.as_deref()),
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(prompt) = &app.system_prompt {
        context_entry_lines(
            &mut lines,
            CONTEXT_SYSTEM_PROMPT_TAG,
            CONTEXT_SYSTEM_COLOR,
            prompt,
            &[],
            &[],
            width,
        );
    }
    for message in crate::context::context_messages_with(instructions, history) {
        context_entry_lines(
            &mut lines,
            &format!("{}:", message.role.wire_name()),
            context_role_color(message.role),
            &message.text,
            &message.tool_calls,
            &message.images,
            width,
        );
    }
    if lines.is_empty() {
        return vec![Line::from(Span::styled(
            CONTEXT_VIEW_EMPTY,
            Style::new().fg(TOOL_DIM_COLOR),
        ))];
    }
    lines
}

/// The largest scroll offset the context view can take on this screen —
/// [`tool_view_max_scroll`]'s sibling (the chrome rows are shared).
#[must_use]
pub fn context_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize;
    context_lines(app, width).len().saturating_sub(body)
}

/// Render the full-screen Ctrl+D context-debug view — the transcript pager's
/// chrome over the raw context window, windowed by `App::debug_scroll`
/// (clamped) with `~` filler past the end. Pure — `term.rs` paints this onto
/// the overlay. See `docs/context.md`.
pub fn render_context_view(area: Rect, buf: &mut Buffer, app: &App) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(overlay_header(CONTEXT_VIEW_TITLE, area.width)).render(title_area, buf);

    let lines = context_lines(app, body_area.width);
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.debug_scroll.min(max);
    let mut visible: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    while (visible.len() as u16) < body_area.height {
        visible.push(Line::from(TOOL_VIEW_FILL));
    }
    Paragraph::new(visible).render(body_area, buf);

    Paragraph::new(tool_view_separator(area.width, scroll, max)).render(sep_area, buf);

    let dim = Style::new().fg(TOOL_DIM_COLOR);
    Paragraph::new(vec![
        Line::from(Span::styled(TOOL_VIEW_HINT_KEYS.to_string(), dim)),
        Line::from(Span::styled(CONTEXT_VIEW_HINT_QUIT.to_string(), dim)),
    ])
    .render(hints_area, buf);
}

/// One dense session row: the `❯ ` marker (spaces when unselected), the age
/// padded to [`RESUME_AGE_WIDTH`] columns, and the preview truncated to the
/// rest of the width — the whole row lit in the palette's selected colour or
/// dimmed (the selection-by-colour convention). Codex's dense picker row.
fn resume_row(
    session: &crate::session::SessionSummary,
    sort: ResumeSort,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let marker = if selected {
        RESUME_MARKER
    } else {
        RESUME_INDENT
    };
    let secs = match sort {
        ResumeSort::Updated => session.updated_secs,
        ResumeSort::Created => session.created_secs,
    };
    let mut age = truncate_cols(&crate::session::relative_age(secs), RESUME_AGE_WIDTH);
    while cols(&age) < RESUME_AGE_WIDTH {
        age.push(' ');
    }
    let room = (width as usize).saturating_sub(cols(marker) + RESUME_AGE_WIDTH);
    let preview = truncate_cols(&session.preview, room);
    let mut text = format!("{marker}{age}{preview}");
    let style = if selected {
        // Pad to the full width in *columns* so the tint spans the row even
        // with wide CJK/emoji in the preview.
        let pad = (width as usize).saturating_sub(cols(&text));
        text.push_str(&" ".repeat(pad));
        Style::new().fg(MENU_SELECTED_COLOR).bg(RESUME_SELECTED_BG)
    } else {
        Style::new().fg(MENU_DIM_COLOR)
    };
    Line::from(Span::styled(text, style))
}

/// The toolbar's tab label for a filter mode.
const fn resume_filter_label(filter: ResumeFilter) -> &'static str {
    match filter {
        ResumeFilter::Cwd => "Cwd",
        ResumeFilter::All => "All",
    }
}

/// The toolbar's tab label for a sort key.
const fn resume_sort_label(sort: ResumeSort) -> &'static str {
    match sort {
        ResumeSort::Updated => "Updated",
        ResumeSort::Created => "Created",
    }
}

/// One toolbar tab value — codex's `toolbar_value`: the active one bracketed
/// (`[Cwd]`, magenta when its control holds the Tab focus, plain otherwise),
/// an inactive one space-padded and dim.
fn resume_toolbar_value(label: &'static str, active: bool, focused: bool) -> Span<'static> {
    if active {
        let text = format!("[{label}]");
        if focused {
            Span::styled(text, Style::new().fg(RESUME_FOCUS_COLOR))
        } else {
            Span::from(text)
        }
    } else {
        Span::styled(format!(" {label} "), Style::new().fg(MENU_DIM_COLOR))
    }
}

/// The Filter/Sort toolbar spans — codex's `toolbar_line`: dim `Filter:` /
/// `Sort:` labels with their tab pairs (`[Cwd] All`, `[Updated] Created`),
/// or — `compact` — each label with just its active value (`Filter:[Cwd]`).
fn resume_toolbar_spans(picker: &ResumePicker, compact: bool) -> Vec<Span<'static>> {
    let dim = Style::new().fg(MENU_DIM_COLOR);
    let filter_focused = picker.focus == ResumeControl::Filter;
    let sort_focused = picker.focus == ResumeControl::Sort;
    if compact {
        return vec![
            Span::styled("Filter:", dim),
            resume_toolbar_value(resume_filter_label(picker.filter), true, filter_focused),
            Span::styled(RESUME_TOOLBAR_GAP, dim),
            Span::styled("Sort:", dim),
            resume_toolbar_value(resume_sort_label(picker.sort), true, sort_focused),
        ];
    }
    vec![
        Span::styled("Filter: ", dim),
        resume_toolbar_value(
            resume_filter_label(ResumeFilter::Cwd),
            picker.filter == ResumeFilter::Cwd,
            filter_focused,
        ),
        resume_toolbar_value(
            resume_filter_label(ResumeFilter::All),
            picker.filter == ResumeFilter::All,
            filter_focused,
        ),
        Span::styled(RESUME_TOOLBAR_GAP, dim),
        Span::styled("Sort: ", dim),
        resume_toolbar_value(
            resume_sort_label(ResumeSort::Updated),
            picker.sort == ResumeSort::Updated,
            sort_focused,
        ),
        resume_toolbar_value(
            resume_sort_label(ResumeSort::Created),
            picker.sort == ResumeSort::Created,
            sort_focused,
        ),
    ]
}

/// Display columns a span list occupies.
fn spans_cols(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| cols(&span.content)).sum()
}

/// The row (within the picker's framed area) the `>` search line sits on — top
/// rule (0), gap (1), search (2). Shared by [`render_model_picker`] and
/// [`cursor_position`] so the cursor lands on the query.
const MODEL_SEARCH_ROW: u16 = 2;

/// A dim two-space-inset placeholder row (loading / empty / error) in the
/// picker's list area, truncated to `width`.
fn model_placeholder_row(text: &str, color: Color, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT));
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(truncate_cols(text, room), Style::new().fg(color)),
    ])
}

/// One model row: `{marker}{id} [{provider}]{✓}` — the selected row's marker and
/// id light up cyan (the palette accent), the `[provider]` tag is dim, and the
/// active model carries a green ✓. The id is truncated so the tag stays visible.
fn model_row(entry: &ModelEntry, selected: bool, active: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = format!(" [{}]", entry.provider);
    let active_mark = if active { MODEL_ACTIVE_MARK } else { "" };
    let reserved = cols(marker) + cols(&tag) + cols(active_mark);
    let id_room = (width as usize).saturating_sub(reserved).max(1);
    let id = truncate_cols(&entry.id, id_room);

    let (marker_style, id_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(id, id_style),
        Span::styled(tag, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(active_mark.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The picker's list lines: a single placeholder while loading / errored /
/// empty, else the model rows windowed ([`centered_window`]) to keep the
/// selection **centered** and capped at [`MODEL_MENU_MAX_ROWS`]. Its length
/// equals [`model_list_rows`] so the reserved height and painted rows agree.
fn model_list_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    match &picker.status {
        ModelLoad::Loading => vec![model_placeholder_row(
            MODEL_LOADING,
            MODEL_META_COLOR,
            width,
        )],
        // Every provider failed: one red row each (`{provider}: {reason}`), or a
        // single legacy message when there are no per-provider errors.
        ModelLoad::Error(msg) => {
            if picker.errors.is_empty() {
                vec![model_placeholder_row(
                    &format!("Error: {msg}"),
                    ERROR_COLOR,
                    width,
                )]
            } else {
                picker
                    .errors
                    .iter()
                    .map(|e| {
                        model_placeholder_row(
                            &format!("{}: {}", e.provider, e.message),
                            ERROR_COLOR,
                            width,
                        )
                    })
                    .collect()
            }
        }
        // No key configured yet — an inviting cyan hint, not a red error.
        ModelLoad::NeedsLogin => vec![model_placeholder_row(
            MODEL_LOGIN_HINT,
            MODEL_SELECTED_COLOR,
            width,
        )],
        ModelLoad::Ready => {
            let matches = picker.matches();
            if matches.is_empty() {
                let text = if picker.models.is_empty() {
                    MODEL_NONE
                } else {
                    MODEL_NO_MATCH
                };
                return vec![model_placeholder_row(text, MODEL_META_COLOR, width)];
            }
            let max = MODEL_MENU_MAX_ROWS as usize;
            let selected = picker.selected.min(matches.len() - 1);
            let offset = centered_window(matches.len(), selected, max);
            matches
                .iter()
                .enumerate()
                .skip(offset)
                .take(max)
                .map(|(i, m)| model_row(m, i == selected, picker.is_active(m), width))
                .collect()
        }
    }
}

/// The `(selected+1/total)` counter line under the list, or a blank line when
/// there's nothing selectable (loading / error / empty).
fn model_counter_line(picker: &ModelPicker) -> Line<'static> {
    if picker.status != ModelLoad::Ready {
        return Line::default();
    }
    let matches = picker.matches();
    if matches.is_empty() {
        return Line::default();
    }
    let selected = picker.selected.min(matches.len() - 1);
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, matches.len()),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ];
    // Beside the counter, the multi-provider load status: dim while more
    // providers are still fetching, red when one finished but failed.
    if let Some((text, color)) = model_load_status_suffix(picker) {
        spans.push(Span::styled(
            format!("{MODEL_STATUS_SEP}{text}"),
            Style::new().fg(color),
        ));
    }
    Line::from(spans)
}

/// The trailing status shown beside the `(n/total)` counter during a
/// multi-provider load: `loading more…` (dim) while fetches are still out, then
/// a red `{provider} unavailable` / `N providers unavailable` note if any
/// failed. `None` once every provider succeeded. See `docs/llm.md`.
fn model_load_status_suffix(picker: &ModelPicker) -> Option<(String, Color)> {
    if picker.pending > 0 {
        Some((MODEL_LOADING_MORE.to_string(), MODEL_META_COLOR))
    } else if picker.errors.len() == 1 {
        Some((
            format!("{} unavailable", picker.errors[0].provider),
            ERROR_COLOR,
        ))
    } else if !picker.errors.is_empty() {
        Some((
            format!("{} providers unavailable", picker.errors.len()),
            ERROR_COLOR,
        ))
    } else {
        None
    }
}

/// The `Model Name: {friendly}` line under the counter, naming the highlighted
/// model, or a blank line when nothing is highlighted.
fn model_name_line(picker: &ModelPicker, width: u16) -> Line<'static> {
    let Some(entry) = picker.highlighted() else {
        return Line::default();
    };
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_NAME_LABEL))
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_NAME_LABEL, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(
            truncate_cols(&entry.display_name, room),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ])
}

/// A full-width `─` rule in the box's border colour (the picker's top/bottom
/// frame, matching the input box's rules).
fn model_rule(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::new().fg(BORDER_COLOR),
    ))
}

/// Render the **inline** `/model` picker into the live region — the shape of the
/// user's mock: a top rule, the `❯` search line, the scrolling model list (each
/// row `→ id [provider] ✓`), a `(n/total)` counter, the `Model Name:` line, and
/// a bottom rule. Headerless (the "Showing models…" banner was dropped). Pure —
/// `render_live` paints this in place of the composer. See `docs/llm.md`.
pub fn render_model_picker(area: Rect, buf: &mut Buffer, picker: &ModelPicker) {
    // The `❯` search line and the list are the same in both layouts; only the
    // rows *below* the list differ (see the branch). Each arm moves these — a
    // value may be moved once per mutually-exclusive branch.
    let search_line = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
        Span::raw(picker.query.clone()),
    ]);
    let list_lines = model_list_lines(picker, area.width);

    if model_has_detail(picker) {
        // A real model is highlighted: counter, gap, name, gap below the list.
        let [
            top_rule,
            _gap1,
            search,
            _gap2,
            list,
            counter,
            _gap3,
            name,
            _gap4,
            bottom_rule,
        ] = Layout::vertical([
            Constraint::Length(1), // top rule
            Constraint::Length(1), // gap
            Constraint::Length(1), // search
            Constraint::Length(1), // gap
            Constraint::Min(0),    // model list
            Constraint::Length(1), // counter
            Constraint::Length(1), // gap
            Constraint::Length(1), // model name
            Constraint::Length(1), // gap
            Constraint::Length(1), // bottom rule
        ])
        .areas(area);

        Paragraph::new(model_rule(area.width)).render(top_rule, buf);
        Paragraph::new(search_line).render(search, buf);
        Paragraph::new(list_lines).render(list, buf);
        Paragraph::new(model_counter_line(picker)).render(counter, buf);
        Paragraph::new(model_name_line(picker, area.width)).render(name, buf);
        Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
    } else {
        // A placeholder (loading / error / needs-login / no match): the blank
        // counter + name collapse to a single gap above the bottom rule.
        let [top_rule, _gap1, search, _gap2, list, _gap3, bottom_rule] = Layout::vertical([
            Constraint::Length(1), // top rule
            Constraint::Length(1), // gap
            Constraint::Length(1), // search
            Constraint::Length(1), // gap
            Constraint::Min(0),    // placeholder list
            Constraint::Length(1), // gap
            Constraint::Length(1), // bottom rule
        ])
        .areas(area);

        Paragraph::new(model_rule(area.width)).render(top_rule, buf);
        Paragraph::new(search_line).render(search, buf);
        Paragraph::new(list_lines).render(list, buf);
        Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
    }
}

/// The `/login` `>` line: the cyan prompt then `text` (the provider filter, or
/// the masked key). Shared shape with the `/model` search line.
fn login_prompt_line(text: Line<'static>) -> Line<'static> {
    let Line { mut spans, .. } = text;
    let mut out = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
    ];
    out.append(&mut spans);
    Line::from(out)
}

/// One provider row in the `/login` list: `{marker}{name} [{env_var}]{✓}` — the
/// selected row lights up cyan (the palette accent), the `[env_var]` tag is dim,
/// and an already-configured provider carries a green ✓. Mirrors [`model_row`].
fn login_provider_row(choice: &ProviderChoice, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = format!(" [{}]", choice.env_var);
    let check = if choice.configured {
        MODEL_ACTIVE_MARK
    } else {
        ""
    };
    let reserved = cols(marker) + cols(&tag) + cols(check);
    let name_room = (width as usize).saturating_sub(reserved).max(1);
    let name = truncate_cols(&choice.name, name_room);

    let (marker_style, name_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name, name_style),
        Span::styled(tag, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(check.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The `/login` provider list: a single `No matching providers` placeholder when
/// the filter matches nothing, else the rows windowed ([`centered_window`]) to
/// keep the selection **centered** and capped at [`LOGIN_MENU_MAX_ROWS`]. Its
/// length equals [`login_provider_list_rows`] so the reserved height and painted
/// rows agree.
fn login_provider_list_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    let matches = onboarding.matches();
    if matches.is_empty() {
        return vec![model_placeholder_row(
            LOGIN_NO_MATCH,
            MODEL_META_COLOR,
            width,
        )];
    }
    let max = LOGIN_MENU_MAX_ROWS as usize;
    let selected = onboarding.selected.min(matches.len() - 1);
    let offset = centered_window(matches.len(), selected, max);
    matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, c)| login_provider_row(c, i == selected, width))
        .collect()
}

/// The `(selected+1/total)` counter under the `/login` provider list, or a blank
/// line when nothing is selectable.
fn login_counter_line(onboarding: &KeyOnboarding) -> Line<'static> {
    let matches = onboarding.matches();
    if matches.is_empty() {
        return Line::default();
    }
    let selected = onboarding.selected.min(matches.len() - 1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, matches.len()),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ])
}

/// The key-step prompt naming the provider — `Enter your {name} API key`, but
/// avoiding a doubled "API" when the name already ends in it (so "Agent Zero
/// API" reads `Enter your Agent Zero API key`, not `… API API key`).
fn login_key_prompt(name: &str) -> String {
    if name.trim_end().to_ascii_lowercase().ends_with("api") {
        format!("Enter your {name} key")
    } else {
        format!("Enter your {name} API key")
    }
}

/// The `/login` masked key field: the entered key rendered as [`LOGIN_MASK_CHAR`]
/// dots (one per character, truncated to width), or a dim placeholder when empty.
fn login_key_field(onboarding: &KeyOnboarding, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_PROMPT))
        .max(1);
    let body = if onboarding.key_input.is_empty() {
        Span::styled(
            truncate_cols(LOGIN_KEY_PLACEHOLDER, room),
            Style::new().fg(MODEL_META_COLOR),
        )
    } else {
        let dots: String = (0..onboarding.key_input.chars().count())
            .map(|_| LOGIN_MASK_CHAR)
            .collect();
        Span::styled(truncate_cols(&dots, room), Style::new().fg(MODEL_ID_COLOR))
    };
    login_prompt_line(Line::from(vec![body]))
}

/// Render the **inline** `/login` API-key onboarding flow into the live region,
/// in place of the composer. Two steps sharing the `/model` picker's framed
/// look: the provider list ([`KeyStep::Provider`]) and the masked key field
/// ([`KeyStep::Key`]). Pure — `render_live` paints this. See `docs/llm.md`.
pub fn render_key_onboarding(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    match onboarding.step {
        KeyStep::Provider => render_login_provider_step(area, buf, onboarding),
        KeyStep::Key => render_login_key_step(area, buf, onboarding),
    }
}

/// The provider-selection step (headerless, like `/model`): top rule, gap, `❯`
/// filter, gap, the provider list, a `(n/total)` counter, gap, a dim
/// `Keys are saved to {.env path}` hint, gap, bottom rule.
fn render_login_provider_step(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    let [
        top_rule,
        _gap1,
        search,
        _gap2,
        list,
        counter,
        _gap3,
        hint,
        _gap4,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1), // top rule
        Constraint::Length(1), // gap
        Constraint::Length(1), // search
        Constraint::Length(1), // gap
        Constraint::Min(0),    // provider list
        Constraint::Length(1), // counter
        Constraint::Length(1), // gap
        Constraint::Length(1), // hint
        Constraint::Length(1), // gap
        Constraint::Length(1), // bottom rule
    ])
    .areas(area);

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(login_prompt_line(Line::from(onboarding.query.clone()))).render(search, buf);
    Paragraph::new(login_provider_list_lines(onboarding, area.width)).render(list, buf);
    Paragraph::new(login_counter_line(onboarding)).render(counter, buf);
    Paragraph::new(model_placeholder_row(
        &format!("{LOGIN_PROVIDER_HINT_PREFIX}{}", onboarding.env_path),
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}

/// The key-entry step: top rule, gap, a periwinkle `Enter your {provider} API
/// key` prompt, gap, the masked `❯` field, gap, a dim
/// `Enter to save · Esc to go back` hint, gap, bottom rule.
fn render_login_key_step(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    let [
        top_rule,
        _gap1,
        prompt,
        _gap2,
        field,
        _gap3,
        hint,
        _gap4,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1), // top rule
        Constraint::Length(1), // gap
        Constraint::Length(1), // prompt
        Constraint::Length(1), // gap
        Constraint::Length(1), // masked field
        Constraint::Length(1), // gap
        Constraint::Length(1), // hint
        Constraint::Length(1), // gap
        Constraint::Length(1), // bottom rule
    ])
    .areas(area);

    let name = onboarding
        .chosen_provider()
        .map_or("the provider", |c| c.name.as_str());

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(model_placeholder_row(
        &login_key_prompt(name),
        LOGIN_KEY_PROMPT_COLOR,
        area.width,
    ))
    .render(prompt, buf);
    Paragraph::new(login_key_field(onboarding, area.width)).render(field, buf);
    Paragraph::new(model_placeholder_row(
        LOGIN_KEY_HINT,
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}

/// A dim, `BG_INDENT`-inset single line for the ↓ manager band, truncated to
/// the width.
fn bg_dim_line(text: &str, width: u16) -> Line<'static> {
    bg_line(text, Style::new().fg(BG_DIM_COLOR), width)
}

/// A `BG_INDENT`-inset single line in `style`, truncated to the width.
fn bg_line(text: &str, style: Style, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(BG_INDENT)).max(1);
    Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(truncate_cols(text, room), style),
    ])
}

/// One row of the manager's shell list: `❯ {command} (running)` — the
/// selected row lights up in the palette accent (marker and text alike), the
/// others are dim, mirroring the slash-command palette's colour-only
/// selection.
fn bg_list_row(shell: &BackgroundShell, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { BG_MARKER } else { "  " };
    let style = if selected {
        Style::new().fg(BG_SELECTED_COLOR)
    } else {
        Style::new().fg(BG_DIM_COLOR)
    };
    let room = (width as usize)
        .saturating_sub(cols(BG_INDENT) + cols(BG_MARKER) + cols(BG_ROW_SUFFIX))
        .max(1);
    Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(marker.to_string(), style),
        Span::styled(truncate_cols(&shell.command, room), style),
        Span::styled(BG_ROW_SUFFIX.to_string(), style),
    ])
}

/// The manager's **list** page (or its empty state): title, `{n} active
/// shells`, the windowed selectable rows, and the key hints — all framed by
/// the picker rules. See `docs/background.md`.
fn bg_list_lines(app: &App, selected: usize, width: u16) -> Vec<Line<'static>> {
    let shells = app.background();
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        bg_line(BG_TITLE, Style::new().fg(AI_COLOR), width),
    ];
    if shells.is_empty() {
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_EMPTY, width));
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_EMPTY_HINTS, width));
    } else {
        let plural = if shells.len() == 1 { "" } else { "s" };
        lines.push(bg_dim_line(
            &format!("{} active shell{plural}", shells.len()),
            width,
        ));
        lines.push(Line::default());
        let selected = selected.min(shells.len() - 1);
        let offset = menu_window(shells.len(), selected, BG_MENU_MAX_ROWS);
        for (i, shell) in shells
            .iter()
            .enumerate()
            .skip(offset)
            .take(BG_MENU_MAX_ROWS)
        {
            lines.push(bg_list_row(shell, i == selected, width));
        }
        lines.push(Line::default());
        lines.push(bg_dim_line(BG_LIST_HINTS, width));
    }
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The manager's **details** page for one shell: the status/runtime/command
/// fields, the rounded output box tailing the last [`BG_OUTPUT_ROWS`] lines
/// of the live output (streaming in as the shell runs), a `Showing N lines`
/// caption, and the key hints. See `docs/background.md`.
fn bg_details_lines(shell: &BackgroundShell, width: u16) -> Vec<Line<'static>> {
    let dim = Style::new().fg(BG_DIM_COLOR);
    let value = Style::new().fg(AI_COLOR);
    let field = |label: &str, text: &str| {
        let room = (width as usize)
            .saturating_sub(cols(BG_INDENT) + cols(label))
            .max(1);
        Line::from(vec![
            Span::raw(BG_INDENT),
            Span::styled(label.to_string(), dim),
            Span::styled(truncate_cols(text, room), value),
        ])
    };
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        bg_line(BG_DETAILS_TITLE, Style::new().fg(AI_COLOR), width),
        Line::default(),
        field(BG_FIELD_STATUS, BG_STATUS_RUNNING),
        field(BG_FIELD_RUNTIME, &format!("{}s", shell.runtime.as_secs())),
        field(BG_FIELD_COMMAND, &shell.command),
        Line::default(),
        bg_dim_line(BG_OUTPUT_LABEL, width),
    ];
    // The output box: rounded corners, one space of padding, the last
    // BG_OUTPUT_ROWS lines top-aligned over blank padding rows.
    let box_width = (width as usize).saturating_sub(cols(BG_INDENT) + 2).max(6);
    let inner = box_width - 2; // less the │ borders
    let text_room = inner.saturating_sub(2).max(1); // less one space each side
    let horizontal = "─".repeat(inner);
    lines.push(Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(format!("╭{horizontal}╮"), dim),
    ]));
    let output = shell.output.trim_end_matches('\n');
    let all: Vec<&str> = if output.is_empty() {
        Vec::new()
    } else {
        output.split('\n').collect()
    };
    let shown = all.len().min(BG_OUTPUT_ROWS);
    let tail = &all[all.len() - shown..];
    for row in 0..BG_OUTPUT_ROWS {
        let text = tail.get(row).copied().unwrap_or("");
        let clipped = truncate_cols(text, text_room);
        let pad = " ".repeat(text_room.saturating_sub(cols(&clipped)));
        lines.push(Line::from(vec![
            Span::raw(BG_INDENT),
            Span::styled("│ ".to_string(), dim),
            Span::styled(clipped, Style::new().fg(TOOL_OUTPUT_COLOR)),
            Span::raw(pad),
            Span::styled(" │".to_string(), dim),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(BG_INDENT),
        Span::styled(format!("╰{horizontal}╯"), dim),
    ]));
    let plural = if shown == 1 { "" } else { "s" };
    lines.push(bg_dim_line(&format!("Showing {shown} line{plural}"), width));
    lines.push(Line::default());
    lines.push(bg_dim_line(BG_DETAILS_HINTS, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// Every line of the open ↓ manager band, top rule to bottom rule — the
/// single source [`render_background_view`] paints and
/// [`background_view_height`] counts (no row ever wraps, so the count is
/// width-independent). Empty when the band is closed. See
/// `docs/background.md`.
#[must_use]
pub fn background_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    match &app.background_view {
        None => Vec::new(),
        Some(BackgroundView::List { selected }) => bg_list_lines(app, *selected, width),
        Some(BackgroundView::Details { id }) => match app.background_shell(id) {
            Some(shell) => bg_details_lines(shell, width),
            // The watched shell is gone (bg_exited retargets the view, so
            // this is a defensive fallback): show the list.
            None => bg_list_lines(app, 0, width),
        },
    }
}

/// Render the **inline** ↓ background manager band into the live region, in
/// place of the composer — the `/model` picker pattern (see
/// `docs/background.md`). Pure — `render_live` paints this.
pub fn render_background_view(area: Rect, buf: &mut Buffer, app: &App) {
    Paragraph::new(background_view_lines(app, area.width)).render(area, buf);
}

/// Render the full-screen `/resume` session picker — codex's resume picker,
/// sized down (docs/resume.md): the slash-tiled title, the type-to-search
/// line, the dense session rows (windowed to keep the selection visible, the
/// palette's [`menu_window`]), and the bottom rule carrying
/// `{selected+1}/{total}` over the dim key hints. Pure — `term.rs` paints
/// this onto the alternate screen, like the transcript pager.
pub fn render_resume_picker(area: Rect, buf: &mut Buffer, app: &App) {
    let [
        title_area,
        _,
        search_area,
        _,
        body_area,
        sep_area,
        hint_area,
        _,
    ] = Layout::vertical([
        Constraint::Length(1), // slash-tiled title
        Constraint::Length(1), // gap
        Constraint::Length(1), // search line
        Constraint::Length(1), // gap
        Constraint::Min(0),    // session rows
        Constraint::Length(1), // ─ rule + count
        Constraint::Length(1), // key hints
        Constraint::Length(1), // final blank
    ])
    .areas(area);

    Paragraph::new(overlay_header(RESUME_TITLE, area.width)).render(title_area, buf);

    let picker = app.resume_picker.as_ref();
    let query = picker.map_or("", |p| p.query.as_str());
    let mut search_spans = if query.is_empty() {
        vec![Span::styled(
            format!("{RESUME_INDENT}{RESUME_SEARCH_PLACEHOLDER}"),
            Style::new().fg(MENU_DIM_COLOR),
        )]
    } else {
        vec![
            Span::styled(
                format!("{RESUME_INDENT}{RESUME_SEARCH_PROMPT}"),
                Style::new().fg(MENU_DIM_COLOR),
            ),
            Span::styled(query.to_string(), Style::new().fg(SEARCH_QUERY_COLOR)),
        ]
    };
    // The Filter/Sort toolbar rides the search row's right edge (codex's):
    // the full tab pairs when they fit, the compact active-value form next,
    // dropped entirely on the narrowest screens.
    if let Some(picker) = picker {
        let left = spans_cols(&search_spans);
        let width = area.width as usize;
        let toolbar = [false, true].into_iter().find_map(|compact| {
            let spans = resume_toolbar_spans(picker, compact);
            let cols = spans_cols(&spans);
            (left + RESUME_TOOLBAR_MIN_GAP + cols <= width).then_some((spans, cols))
        });
        if let Some((spans, cols)) = toolbar {
            search_spans.push(Span::from(" ".repeat(width - left - cols)));
            search_spans.extend(spans);
        }
    }
    Paragraph::new(Line::from(search_spans)).render(search_area, buf);

    let matches = picker.map_or_else(Vec::new, |p| p.matches());
    let sort = picker.map_or(ResumeSort::Updated, |p| p.sort);
    let selected = picker
        .map_or(0, |p| p.selected)
        .min(matches.len().saturating_sub(1));
    let rows: Vec<Line> = if matches.is_empty() {
        // Two empty states (codex's): never saved anything, vs a query (or
        // the Cwd filter) leaving nothing to show.
        let placeholder = if picker.is_none_or(|p| p.sessions.is_empty()) {
            RESUME_NO_SESSIONS
        } else {
            RESUME_NO_MATCH
        };
        vec![Line::from(Span::styled(
            format!("{RESUME_INDENT}{placeholder}"),
            Style::new().fg(MENU_DIM_COLOR),
        ))]
    } else {
        let height = (body_area.height as usize).max(1);
        let start = menu_window(matches.len(), selected, height);
        matches
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(index, session)| resume_row(session, sort, index == selected, area.width))
            .collect()
    };
    Paragraph::new(rows).render(body_area, buf);

    // An empty list has no selection to count — the rule stays bare.
    let label = if matches.is_empty() {
        String::new()
    } else {
        format!(" {}/{} ", selected + 1, matches.len())
    };
    Paragraph::new(rule_with_label(area.width, &label)).render(sep_area, buf);
    Paragraph::new(Line::from(Span::styled(
        RESUME_HINTS.to_string(),
        Style::new().fg(TOOL_DIM_COLOR),
    )))
    .render(hint_area, buf);
}

/// The **incremental**, stateful renderer that commits an assistant reply to
/// scrollback as it streams — the boundary's replacement for the old
/// re-render-the-whole-reply `stable_commit`/`final_commit` pair (which cost
/// O(reply) *per chunk*, so a long code reply was O(reply²) and starved the
/// status animation — see `docs/markdown.md`).
///
/// It drives one [`AssistantRenderer`], caching the rendered rows of every
/// **complete** source line (`frozen`) and advancing over only the newly-arrived
/// lines on each call — so [`commit`](Self::commit)/[`preview`](Self::preview)
/// cost O(new text), and streaming a whole reply is O(reply).
///
/// Prefix-stability (CLAUDE.md invariant 2) holds by construction: a completed
/// source line's rows are frozen (markdown fence + highlight carry are threaded
/// left-to-right, wrapping is prefix-stable), so a row committed to scrollback
/// never changes. The still-growing trailing line is withheld from
/// [`commit`](Self::commit) and only peeked for the [`preview`](Self::preview),
/// then flushed by [`finish`](Self::finish) when the reply ends.
///
/// A width change (a resize) invalidates the cached rows; the next call rebuilds
/// from scratch (the boundary also [`reset`](Self::reset)s and re-commits the
/// reply, matching the old `committed = 0` behaviour).
pub struct StreamRender {
    /// The wrapping width the cache was built at; a change triggers a rebuild.
    width: u16,
    /// Renderer state (fence + highlight) entering the trailing partial line.
    renderer: AssistantRenderer,
    /// Rendered rows of every complete (newline-terminated) source line so far.
    frozen: Vec<Line<'static>>,
    /// Byte offset of the start of the trailing partial line (end of the last
    /// complete line consumed into `frozen`).
    consumed: usize,
    /// Rows already returned to scrollback — an index into `frozen ++ tail`.
    committed: usize,
}

impl Default for StreamRender {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamRender {
    /// A fresh renderer (before any width is known). The first call adopts the
    /// caller's width.
    #[must_use]
    pub fn new() -> Self {
        Self {
            width: 0,
            renderer: AssistantRenderer::new(0, AI_BULLET, AI_COLOR),
            frozen: Vec::new(),
            consumed: 0,
            committed: 0,
        }
    }

    /// Discard all cached state — used at every turn boundary (turn end, tool
    /// split, interrupt, `/clear`, resize) so the next reply starts clean.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Fold every source line that has become **complete** (is newline-terminated)
    /// since the last call into `frozen`, advancing the renderer. Cheap: only the
    /// lines past `consumed`. A width change rebuilds from scratch first.
    fn advance(&mut self, text: &str, width: u16) {
        if width != self.width {
            self.width = width;
            self.renderer = AssistantRenderer::new(width, AI_BULLET, AI_COLOR);
            self.frozen.clear();
            self.consumed = 0;
            self.committed = 0;
        }
        // The last '\n' at or after `consumed` terminates the last complete line;
        // everything up to it is now frozen. `consumed` always lands right after a
        // '\n' (or 0), so the slice is on char boundaries.
        if let Some(rel) = text[self.consumed..].rfind('\n') {
            let end = self.consumed + rel;
            for line in text[self.consumed..end].split('\n') {
                self.frozen.extend(self.renderer.feed_line(line));
            }
            self.consumed = end + 1;
        }
    }

    /// The rows of the still-growing trailing partial line, rendered without
    /// disturbing the renderer's state (a cheap clone peek — O(one line)). An
    /// **empty** trailing line inside an open table is NOT fed: doing so would
    /// close the table on the clone and pull its rendered block into `commit`'s
    /// stable range mid-stream. The block only becomes real when a non-table
    /// line completes, or at `finish` (docs/table-streaming.md).
    fn tail_rows(&self, text: &str) -> Vec<Line<'static>> {
        let tail_src = &text[self.consumed..];
        let mut clone = self.renderer.clone();
        if tail_src.is_empty() && clone.in_table() {
            return Vec::new();
        }
        clone.feed_line(tail_src)
    }

    /// Take rows `[committed, stable)` of the virtual `frozen ++ tail`
    /// concatenation, advancing `committed`. `committed` never regresses.
    fn take_rows(&mut self, tail: &[Line<'static>], stable: usize) -> Vec<Line<'static>> {
        let start = self.committed.min(stable);
        let frozen_len = self.frozen.len();
        let out = (start..stable)
            .map(|i| {
                if i < frozen_len {
                    self.frozen[i].clone()
                } else {
                    tail[i - frozen_len].clone()
                }
            })
            .collect();
        self.committed = stable.max(self.committed);
        out
    }

    /// The stable rows to append to scrollback for the current buffer `text` —
    /// every rendered row except the still-growing last one — since the previous
    /// call. Replaces `stable_commit`; O(text appended since the last call).
    #[must_use]
    pub fn commit(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        // Withhold the whole trailing line when its rows aren't final yet:
        //  - **inside a fenced block**: a code line's colour isn't settled until the
        //    whole line is seen (a call's `(`, a `//` comment, a closing `*/`), and
        //    a long code line wraps into several rows — committing an early row
        //    could recolour it once the lookahead char arrives; and
        //  - a **partial fence marker** (`` ` ``/`` `` ``): a third marker would
        //    flip it from prose to a one-row label, so its wrapped prose rows must
        //    not reach scrollback; and
        //  - a **partial thematic-break run** (`-`/`*`/`_`, 1–2 markers): a third
        //    marker would collapse its wrapped prose rows into a single `———`
        //    rule; and
        //  - a **bare `#` run** (1–6 hashes): its heading *level* — and so its
        //    style — isn't settled (another `#` deepens it, a 7th flips it to
        //    prose), so at a width narrower than the run its wrapped rows must
        //    not reach scrollback yet; and
        //  - an **open table** (`in_table`): the block is buffered and renders
        //    whole only when it closes (its widths need every row), so nothing
        //    of it exists to commit — this commits only the settled pre-table
        //    rows, and the strip previews the forming block. A trailing
        //    **table-row candidate** (`is_table_row`) is withheld the same way
        //    before the renderer has consumed it (docs/table-streaming.md); and
        //  - a trailing line with an **open inline marker** (`has_open_inline` —
        //    an unclosed `**`/`*`/`~~`/`` ` ``/`[`): its closer could still restyle
        //    an already-wrapped row, so the whole line is withheld until it settles
        //    (inline emphasis is line-local, so a *complete* line is always final).
        // Otherwise it's settled prose — only its still-growing *last* row is held
        // back. `tail_rows` (an O(one line) render) is computed only in that case,
        // never in the withhold path where it would be discarded.
        let tail_src = &text[self.consumed..];
        if self.renderer.in_code()
            || self.renderer.in_table()
            || markdown::is_table_row(tail_src)
            || markdown::is_partial_fence(tail_src)
            || markdown::is_partial_thematic_break(tail_src)
            || markdown::is_partial_heading(tail_src)
            || markdown::is_partial_list_marker(tail_src)
            || markdown::has_open_inline(tail_src)
        {
            let stable = self.frozen.len();
            // Inside a fence blank lines are content; otherwise still hold back a
            // trailing blank run (a paragraph break before this in-progress line)
            // — it commits once real content follows, or stays withheld to be
            // trimmed by `finish` if the message ends here.
            let stable = if self.renderer.in_code() {
                stable
            } else {
                self.without_trailing_blanks(&[], stable)
            };
            self.take_rows(&[], stable)
        } else {
            let tail = self.tail_rows(text);
            let total = self.frozen.len() + tail.len();
            // Withhold the **last non-blank row** (and any trailing blanks): it is
            // the row the strip previews, so committing it the moment its line ends
            // with a newline would show it twice — once in scrollback, once in the
            // preview — until the next chunk (the slow-stream duplicate-line bug).
            // It commits on a later call once newer content supersedes it, or at
            // `finish`. During active streaming the last row is the still-growing
            // trailing line, so this matches the old "withhold the last row".
            let stable = self.stable_keeping_preview_row(&tail, total);
            self.take_rows(&tail, stable)
        }
    }

    /// The commit boundary that **keeps the last non-blank row for the preview**:
    /// the index of the last non-blank row of the virtual `frozen ++ tail` (never
    /// below `committed`). Committing `[committed, boundary)` flushes every row
    /// *above* the one the strip previews; that row and any trailing blanks stay
    /// withheld. This is [`without_trailing_blanks`] backed off one further row, so
    /// a completed line is never both in scrollback and the preview at once.
    fn stable_keeping_preview_row(&self, tail: &[Line<'static>], total: usize) -> usize {
        let frozen_len = self.frozen.len();
        let mut s = total;
        while s > self.committed {
            let row = if s - 1 < frozen_len {
                &self.frozen[s - 1]
            } else {
                &tail[s - 1 - frozen_len]
            };
            if row_is_blank(row) {
                s -= 1; // withhold trailing blanks
            } else {
                return s - 1; // withhold this last non-blank row too (it previews)
            }
        }
        self.committed
    }

    /// Back off `stable` over trailing blank rows of the virtual `frozen ++ tail`
    /// sequence (never below `committed`), so a message-trailing blank run is
    /// **withheld** rather than committed on top of the next item's spacer (the
    /// 3-newline bug). A blank followed by real content is no longer trailing on
    /// the next call, so it commits then. Gated by the caller to skip code.
    fn without_trailing_blanks(&self, tail: &[Line<'static>], stable: usize) -> usize {
        let frozen_len = self.frozen.len();
        let mut s = stable;
        while s > self.committed {
            let row = if s - 1 < frozen_len {
                &self.frozen[s - 1]
            } else {
                &tail[s - 1 - frozen_len]
            };
            if row_is_blank(row) {
                s -= 1;
            } else {
                break;
            }
        }
        s
    }

    /// The remaining rows once the reply is complete: the withheld last row plus
    /// the whole trailing partial line, now rendered as a final complete line.
    /// Replaces `final_commit`.
    #[must_use]
    pub fn finish(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        let mut tail = self.renderer.feed_line(&text[self.consumed..]);
        // Flush a table the reply ended on (its rows were buffered pending a close
        // that never came), matching `assistant_lines`'s trailing `flush`.
        tail.extend(self.renderer.flush());
        self.consumed = text.len();
        self.frozen.extend(tail);
        if self.frozen.is_empty() {
            // The whole reply rendered to zero rows (only a code fence) — commit
            // the bullet home once, matching `assistant_lines`.
            self.frozen.push(empty_assistant_row(
                &self.renderer.bullet,
                self.renderer.color,
            ));
        }
        // Trim trailing blank rows (a model's `…\n\n` before a tool call, or at
        // the reply's end) — the caller adds exactly one spacer — unless the
        // reply ended inside an open fence (blank lines there are content). The
        // bullet-home fallback above is never blank, so an empty reply still
        // commits its bullet. Matches `assistant_lines` so the two agree.
        let mut total = self.frozen.len();
        if !self.renderer.in_code() {
            while total > self.committed && row_is_blank(&self.frozen[total - 1]) {
                total -= 1;
            }
        }
        self.take_rows(&[], total)
    }

    /// The rows this render has already handed to scrollback for `text` — the
    /// first `committed` rows of the virtual `frozen ++ tail` concatenation —
    /// re-rendered for a repaint (the Ctrl+O overlay return; see
    /// [`repaint_tail`]). Advances the line cache over `text` first, so rows
    /// committed from a since-completed trailing line are found in `frozen`;
    /// `committed` itself is untouched, so a follow-up [`commit`](Self::commit)
    /// still emits exactly the not-yet-committed rows. After a width change
    /// there *are* no already-committed rows at the new width (the cache
    /// rebuilt), so this returns nothing and the follow-up commit re-emits the
    /// whole reply.
    #[must_use]
    pub fn committed_rows(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        let mut rows: Vec<Line<'static>> =
            self.frozen.iter().take(self.committed).cloned().collect();
        if self.committed > self.frozen.len() {
            let tail = self.tail_rows(text);
            rows.extend(tail.into_iter().take(self.committed - self.frozen.len()));
        }
        rows
    }

    /// The strip's streaming preview for the current buffer — normally the last
    /// rendered row (one line), but **while a table is open** the entire
    /// uncommitted tail of the batch render: the forming block re-rendered from
    /// the rows seen so far, so the grid visibly streams row-by-row with its
    /// columns re-fitting as wider cells arrive, Claude Code-style, while
    /// nothing of it touches immutable scrollback (docs/table-streaming.md).
    /// Capped to the **newest** `max_rows` rows so a table taller than the
    /// screen tail-follows its frontier. O(new complete lines since the last
    /// call + the trailing line + the open table), so redrawing it every
    /// animation frame stays cheap (a table is bounded, unlike the reply).
    #[must_use]
    pub fn preview(&mut self, text: &str, width: u16, max_rows: usize) -> Vec<Line<'static>> {
        self.advance(text, width);
        // Feed the trailing line on a clone (not disturbing the resumable state)
        // and keep the clone so its *post-tail* fence state decides trimming —
        // the same state `finish`/`assistant_lines` see once the whole prefix is
        // rendered (a trailing `` ``` `` opens a fence, so a blank before it is
        // kept, not trimmed).
        let mut clone = self.renderer.clone();
        // An empty trailing line (a chunk boundary that ended right after a
        // newline) is *not* fed: feeding it would close an open table on the
        // clone early — the flush below renders the buffered block either way
        // (`tail_rows` carries the same guard, so commit and preview agree on
        // the frontier).
        let tail_src = &text[self.consumed..];
        let mut tail = if tail_src.is_empty() && clone.in_table() {
            Vec::new()
        } else {
            clone.feed_line(tail_src)
        };
        // A table is open — entering the trailing line (`self.renderer`), or
        // opened/kept open by it (`clone`): preview the batch render's WHOLE
        // uncommitted tail, so scrollback + strip always show the complete
        // reply. That is the frozen rows past `committed` (e.g. the withheld
        // blank between the pre-table prose and the table, or the block + a
        // closing line the clone just rendered), then the forming block flushed
        // on the clone — identical to what `assistant_lines` renders for this
        // prefix, trailing blanks trimmed the same way.
        if self.renderer.in_table() || clone.in_table() {
            tail.extend(clone.flush());
            let skip = self.committed.min(self.frozen.len());
            let mut rows: Vec<Line<'static>> = self.frozen[skip..].to_vec();
            rows.extend(
                tail.into_iter()
                    .skip(self.committed.saturating_sub(self.frozen.len())),
            );
            while rows.last().is_some_and(row_is_blank) {
                rows.pop();
            }
            // Tail-follow: keep the newest rows when the block outgrows the cap
            // (the top border scrolls out of the strip and reappears when the
            // closed block commits whole).
            if rows.len() > max_rows {
                rows.drain(..rows.len() - max_rows);
            }
            return rows;
        }
        // No table: the last rendered row, skipping trailing blank rows so the
        // strip shows content rather than a paragraph-break blank — matching the
        // trimmed batch render. Inside a fence blank lines are content, so keep
        // as-is.
        let last_row = |rows: &[Line<'static>]| -> Option<Line<'static>> {
            if clone.in_code() {
                rows.last().cloned()
            } else {
                rows.iter().rev().find(|r| !row_is_blank(r)).cloned()
            }
        };
        let row = last_row(&tail)
            .or_else(|| last_row(&self.frozen))
            .unwrap_or_else(|| {
                // The reply-so-far renders to zero rows (only a code fence, or only
                // whitespace): batch `assistant_lines` still emits the bullet home, so
                // the preview must match it or the strip would diverge from a repaint.
                empty_assistant_row(&self.renderer.bullet, self.renderer.color)
            });
        vec![row]
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{FileSearch, Message, ModelFetchError, RetryInfo};

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

    // --- markdown tables (docs/markdown.md) ---

    /// The plain text of each content row (span contents concatenated).
    fn rows_text(rows: &[Vec<Span<'static>>]) -> Vec<String> {
        rows.iter()
            .map(|r| r.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    /// Whether `prefix` ends inside an OPEN GFM table — its last non-blank source
    /// line is still a table row/delimiter/header, so no non-table line has
    /// closed the block. Used by the differential test to skip the preview
    /// equality check exactly where the streaming preview intentionally shows the
    /// last content row rather than the batch's flushed bottom border.
    fn ends_in_open_table(prefix: &str) -> bool {
        prefix
            .split('\n')
            .rev()
            .find(|l| !l.trim().is_empty())
            .is_some_and(markdown::is_table_row)
    }

    #[test]
    fn table_content_rows_draws_a_bordered_grid() {
        let lines: Vec<String> = ["| Name | Type |", "|------|------|", "| Alpha | X |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rows_text(&table_content_rows(&lines, 80)),
            vec![
                "┌───────┬──────┐",
                "│ Name  │ Type │",
                "├───────┼──────┤",
                "│ Alpha │ X    │",
                "└───────┴──────┘",
            ]
        );
    }

    #[test]
    fn table_header_cells_center_by_default() {
        // Claude Code's look: a header cell is CENTERED in its column, while the
        // data below it stays left-aligned. The delimiter here declares no
        // alignment (`|---|`), which is the common case a model emits.
        let lines: Vec<String> = [
            "| Check | Result |",
            "|-------|--------|",
            "| cargo fmt | ok |",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            rows_text(&table_content_rows(&lines, 80)),
            vec![
                "┌───────────┬────────┐",
                "│   Check   │ Result │",
                "├───────────┼────────┤",
                "│ cargo fmt │ ok     │",
                "└───────────┴────────┘",
            ]
        );
    }

    #[test]
    fn table_header_keeps_an_explicitly_declared_alignment() {
        // Centering is only the *default* (`Alignment::None`). When the author
        // declared an alignment with colons, the header honours it — a markdown
        // renderer must not throw away stated intent.
        let lines: Vec<String> = ["| head | x |", "| :--- | ---: |", "| a longer cell | y |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = rows_text(&table_content_rows(&lines, 80));
        assert_eq!(
            rows[1], "│ head          │ x │",
            "an explicit `:---` keeps the header left: {rows:#?}"
        );
    }

    #[test]
    fn table_row_centers_a_short_cell_against_a_wrapped_one() {
        // Claude Code centres a row's cells VERTICALLY: beside a cell that wrapped
        // to three rows, a one-line cell sits on the middle row, not the top.
        let cells = vec![
            table_cell_segments("one two three", Style::default()),
            table_cell_segments("x", Style::default()),
        ];
        let rows = table_row_lines(
            &cells,
            &[5, 3],
            &[markdown::Alignment::None, markdown::Alignment::None],
        );
        assert_eq!(
            rows_text(&rows),
            vec!["│ one   │     │", "│ two   │ x   │", "│ three │     │"]
        );
    }

    #[test]
    fn assistant_table_centers_headers_and_short_cells_like_claude_code() {
        // The reported screenshot, end to end: `Check`/`Result` centred in their
        // columns, and a one-line cell vertically centred beside its wrapped
        // neighbour — so `✅ pass (exit 0)` shares a row with `--all-targets`
        // (the middle of the three) rather than sitting at the top, and
        // `cargo test` shares a row with the middle line of its Result cell.
        let text = "| Check | Result |\n\
                    |-------|--------|\n\
                    | `cargo fmt --check` | ✅ clean |\n\
                    | `cargo clippy --all-targets -- -D warnings` | ✅ pass (exit 0) |\n\
                    | `cargo test` | ✅ all tests pass, 0 failures (23 live-API tests \
                    skipped — need `OPENROUTER_API_KEY` / `A0_VENICE_API_KEY`) |";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
            .iter()
            .map(plain)
            .collect();
        let header = rows
            .iter()
            .find(|r| r.contains("Check") && r.contains("Result"))
            .expect("a header row");
        assert!(
            header.starts_with("● │   ") || header.starts_with("  │   "),
            "the header cell is centred, not flush left: {header:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("--all-targets") && r.contains("✅ pass (exit 0)")),
            "the one-line Result cell sits on the middle row of its neighbour: {rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("cargo test") && r.contains("skipped — need")),
            "`cargo test` sits on the middle row of its Result cell: {rows:#?}"
        );
    }

    #[test]
    fn table_content_rows_aligns_per_delimiter() {
        // Left, right, and center alignment from the delimiter colons.
        let lines: Vec<String> = ["| a | b | c |", "| :-- | --: | :-: |", "| x | yy | zzz |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rows_text(&table_content_rows(&lines, 80)),
            vec![
                "┌───┬────┬─────┐",
                "│ a │  b │  c  │",
                "├───┼────┼─────┤",
                "│ x │ yy │ zzz │",
                "└───┴────┴─────┘",
            ]
        );
    }

    #[test]
    fn table_content_rows_pads_short_rows_to_the_column_count() {
        // A data row with fewer cells than the header is padded with blanks;
        // extra cells beyond the delimiter's column count are dropped.
        let lines: Vec<String> = ["| a | b |", "|---|---|", "| x |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rows_text(&table_content_rows(&lines, 80)),
            vec![
                "┌───┬───┐",
                "│ a │ b │",
                "├───┼───┤",
                "│ x │   │",
                "└───┴───┘",
            ]
        );
    }

    #[test]
    fn table_content_rows_renders_inline_markdown_in_cells() {
        // Regression (the reported bug): a table cell's `` `code` `` and
        // `**bold**` markers rendered literally instead of being inline-parsed
        // like prose (docs/markdown.md). They must be styled — backticks and
        // asterisks gone — and the column sized to the *rendered* width
        // (`a.db`, not `` `a.db` ``).
        let lines: Vec<String> = ["| Name  | When |", "|-------|------|", "| `a.db` | **X** |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = table_content_rows(&lines, 80);
        assert_eq!(
            rows_text(&rows),
            vec![
                "┌──────┬──────┐",
                "│ Name │ When │",
                "├──────┼──────┤",
                "│ a.db │ X    │",
                "└──────┴──────┘",
            ]
        );
        // The code cell is cyan, the bold cell carries BOLD, and no raw markers
        // survive anywhere in the grid.
        let spans: Vec<(String, Modifier, Option<Color>)> = rows
            .iter()
            .flat_map(|r| r.iter())
            .map(|s| (s.content.to_string(), s.style.add_modifier, s.style.fg))
            .collect();
        assert!(
            spans
                .iter()
                .any(|(t, _, fg)| t == "a.db" && *fg == Some(INLINE_CODE_COLOR)),
            "inline code cell is cyan: {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|(t, m, _)| t == "X" && m.contains(Modifier::BOLD)),
            "bold cell carries BOLD: {spans:?}"
        );
        let joined: String = spans.iter().map(|(t, _, _)| t.as_str()).collect();
        assert!(
            !joined.contains("**") && !joined.contains('`'),
            "no raw markers leak into the grid: {joined:?}"
        );
    }

    #[test]
    fn table_grid_draws_separators_between_every_row() {
        // Claude Code's grid look (the user's reference): every data row is
        // framed — a `├──┼──┤` rule between consecutive rows, not just under
        // the header (docs/table-streaming.md).
        let lines: Vec<String> = ["| a | b |", "|---|---|", "| 1 | 2 |", "| 3 | 4 |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rows_text(&table_content_rows(&lines, 80)),
            vec![
                "┌───┬───┐",
                "│ a │ b │",
                "├───┼───┤",
                "│ 1 │ 2 │",
                "├───┼───┤",
                "│ 3 │ 4 │",
                "└───┴───┘",
            ]
        );
    }

    #[test]
    fn allocate_column_widths_spends_the_surplus_where_the_content_is() {
        // The reported screenshot's shape: column 1's natural width is driven by
        // ONE long cell (`cargo clippy --all-targets -- -D warnings`, 40) while
        // every other cell in it is ≤ 17; column 2 holds a ~104-column value.
        // Levelling the two widths gave column 1 ~35 columns it never used and
        // starved column 2 into four wrapped rows. Word-safe floors [13, 18] fit
        // in the 71-column budget, so the 40-column surplus is split by unmet
        // demand (27:86) — the value column ends with roughly twice the room,
        // Claude Code's layout.
        let w = allocate_column_widths(&[40, 104], &[13, 18], 78);
        assert_eq!(w, vec![23, 48]);
        assert_eq!(
            w.iter().sum::<usize>(),
            71,
            "columns fill the content width"
        );
    }

    #[test]
    fn allocate_column_widths_wraps_words_before_shattering_a_token() {
        // The ping-summary fit: Host(12) IP(33) Packets(7) Loss(4) RTT(21) must
        // fit avail 85 (content 69, 8 over). The IPv6 column is a single
        // unbreakable 33-column token; the RTT column wraps at spaces. Taking
        // the overflow from the *widest* column hard-broke the address
        // mid-token, so the overflow comes from the column that can wrap
        // cleanly instead — and the short cells still keep their natural width.
        assert_eq!(
            allocate_column_widths(&[12, 33, 7, 4, 21], &[12, 33, 7, 4, 11], 85),
            vec![12, 33, 7, 4, 13]
        );
    }

    #[test]
    fn allocate_column_widths_levels_the_widest_when_no_floor_fits() {
        // When even the word-safe floors overflow the budget, some token has to
        // hard-break: fall back to levelling the widest column down one display
        // column at a time (ties leftmost-first), which keeps a short column
        // whole for as long as possible.
        let w = allocate_column_widths(&[8, 20], &[8, 20], 20);
        assert_eq!(
            w.iter().sum::<usize>(),
            13,
            "columns fill the content width"
        );
        assert!(
            w[1] > w[0],
            "the wider natural column keeps more room: {w:?}"
        );
        assert!(w.iter().all(|&c| c >= TABLE_MIN_COL), "floored: {w:?}");
    }

    #[test]
    fn narrow_table_keeps_short_cells_whole() {
        // End-to-end (the reported screenshot): at a width where the natural
        // grid overflows, "facebook.com" and the "Packets"/"Loss" headers stay
        // on one line — the wrapping lands on the wide IP column instead.
        let text = "| Host | IP | Packets | Loss | RTT min/avg/max |\n\
                    |------|----|---------|------|-----------------|\n\
                    | google.com | 2404:6800:4017:809::200e | 10/10 | 0% | 86.8 / 89.8 / 96.4 ms |\n\
                    | facebook.com | 2a03:2880:f372:1:face:b00c:0:25de | 10/10 | 0% | 15.0 / 16.4 / 17.9 ms |\n\
                    | x.com | 162.159.140.229 | 10/10 | 0% | 14.4 / 15.5 / 16.2 ms |";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 87)
            .iter()
            .map(plain)
            .collect();
        assert!(
            rows.iter().any(|r| r.contains(" facebook.com ")),
            "a short host cell never breaks mid-word: {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains(" Packets ")),
            "a short header never breaks mid-word: {rows:#?}"
        );
    }

    #[test]
    fn table_cells_wrap_across_rows_instead_of_truncating() {
        // A cell wider than its column WRAPS across rows (hard-breaking an
        // unbreakable token grapheme-by-grapheme, wide CJK included) — never
        // truncated with a `…`, so no cell content is ever lost.
        let cells = vec![table_cell_segments("世界世界", Style::default())]; // 8 cols
        let rows = table_row_lines(&cells, &[5], &[markdown::Alignment::None]);
        let text = rows_text(&rows);
        assert!(
            rows.len() >= 2,
            "a too-wide cell wraps into >1 row: {text:?}"
        );
        assert!(
            !text.join("").contains('…'),
            "cells wrap, never truncate: {text:?}"
        );
        let kept: String = text
            .iter()
            .flat_map(|r| r.chars())
            .filter(|c| matches!(c, '世' | '界'))
            .collect();
        assert_eq!(
            kept, "世界世界",
            "no cell content lost to wrapping: {text:?}"
        );
    }

    #[test]
    fn allocate_column_widths_keeps_a_grid_that_fits_natural() {
        // Fits: the natural grid (2+3 content + 7 overhead = 12) is ≤ avail, so
        // columns keep their natural widths and the table stays as narrow as its
        // content — never stretched to the terminal.
        assert_eq!(allocate_column_widths(&[2, 3], &[2, 3], 40), vec![2, 3]);
    }

    #[test]
    fn narrow_table_wraps_cells_into_taller_rows_no_ellipsis() {
        // The reported fix (img1 → img2): when the natural grid overflows the
        // width, cells WORD-WRAP into taller rows instead of truncating with `…`,
        // so no content is ever lost. Width 28 keeps this a grid — the email
        // column levels to 13 and wraps twice; any narrower and the records
        // fallback (its own tests) takes over.
        let lines: Vec<String> = [
            "| Name | Email |",
            "|------|-------|",
            "| John Doe | john.doe@example.com |",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let text = rows_text(&table_content_rows(&lines, 28));
        assert!(
            !text.join("").contains('…'),
            "cells wrap, never truncate: {text:?}"
        );
        let mid = text.iter().position(|r| r.starts_with('├')).unwrap();
        let bot = text.iter().position(|r| r.starts_with('└')).unwrap();
        let data = &text[mid + 1..bot];
        assert!(data.len() >= 2, "the data row wraps across rows: {text:?}");
        // The email column's content survives intact across the wrapped rows
        // (a spaceless token is only hard-broken, never dropped).
        let email: String = data
            .iter()
            .map(|r| {
                r.split('│')
                    .nth(2)
                    .map_or(String::new(), |c| c.trim().to_string())
            })
            .collect();
        assert_eq!(
            email, "john.doe@example.com",
            "wrapped cell content is preserved: {text:?}"
        );
    }

    #[test]
    fn very_narrow_table_renders_as_key_value_records() {
        // At a very narrow width the grid is too cramped to scan, so it flips to
        // codex-style key/value records: no box-drawing, a `label value` field
        // per column, a `─` rule between rows, nothing truncated.
        let table = "| Name | Email | Role |\n|------|-------|------|\n\
                     | Alice Johnson | alice@example.com | Administrator |\n\
                     | Bob Smith | bob@example.com | Editor |";
        let lines = message_lines(Role::Assistant, table, 24);
        let text: Vec<String> = lines
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        let joined = text.join("\n");
        // Records mode has NO grid box-drawing.
        assert!(
            !joined.contains('│') && !joined.contains('┌') && !joined.contains('┐'),
            "records mode has no grid borders: {text:?}"
        );
        // Every column label appears as a field key (once per record).
        for label in ["Name", "Email", "Role"] {
            assert!(
                text.iter().any(|r| r.contains(label)),
                "label {label}: {text:?}"
            );
        }
        // Nothing truncated; content survives even where a value wrapped.
        assert!(!joined.contains('…'), "records never truncate: {text:?}");
        let squished: String = joined.split_whitespace().collect();
        for needle in [
            "alice@example.com",
            "Administrator",
            "bob@example.com",
            "BobSmith",
        ] {
            assert!(
                squished.contains(needle),
                "content preserved: {needle} in {text:?}"
            );
        }
        // A `─` rule separates the two records (rows carry the bullet indent).
        assert!(
            text.iter().any(|r| {
                let t = r.trim();
                !t.is_empty() && t.chars().all(|c| c == '─')
            }),
            "a separator rule between records: {text:?}"
        );
    }

    #[test]
    fn record_block_uses_colon_key_value_fields() {
        // Records render as Claude Code-style `label: value` fields — a colon and
        // a single space, no aligned label column, no trailing padding.
        let labels = vec!["Name".to_string(), "Role".to_string()];
        let block = table_record_block(&labels, "| Alice Johnson | Admin |", 40);
        let rows: Vec<String> = block
            .iter()
            .map(|r| r.iter().map(|s| s.content.to_string()).collect())
            .collect();
        assert_eq!(rows, vec!["Name: Alice Johnson", "Role: Admin"]);
    }

    #[test]
    fn record_separator_caps_its_width() {
        // The `─` rule between records caps at TABLE_RECORD_SEPARATOR_WIDTH rather
        // than spanning the full content width (Claude Code's shorter separator),
        // but still shrinks to fit a narrow content width.
        let wide: String = table_record_separator(80)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(cols(&wide), TABLE_RECORD_SEPARATOR_WIDTH);
        let narrow: String = table_record_separator(15)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(cols(&narrow), 15);
    }

    #[test]
    fn moderately_narrow_table_stays_a_wrapping_grid() {
        // Records must not over-trigger: a moderately narrow table still renders
        // as a box-drawing grid (cells wrap into taller rows). Records only kick
        // in when the grid is genuinely cramped (docs/table-streaming.md).
        let table = "| Name | Email | Role |\n|------|-------|------|\n\
                     | Alice Johnson | alice@example.com | Administrator |\n\
                     | Bob Smith | bob@example.com | Editor |";
        let joined: String = message_lines(Role::Assistant, table, 48)
            .iter()
            .map(plain)
            .collect();
        assert!(
            joined.contains('│') && joined.contains('┌'),
            "a moderately narrow table stays a grid: {joined:?}"
        );
    }

    #[test]
    fn table_should_use_records_only_when_narrow_and_cramped() {
        let header = "| Name | Email | Role |";
        let rows = vec!["| Alice Johnson | alice@example.com | Administrator |".to_string()];
        // Wide terminal → scannable grid, not records.
        let wide = table_column_widths(header, &rows, 3, 80);
        assert!(
            !table_should_use_records(header, &rows, &wide),
            "wide stays a grid"
        );
        // Very narrow → columns starved and cells wrap tall → records.
        let narrow = table_column_widths(header, &rows, 3, 24);
        assert!(
            table_should_use_records(header, &rows, &narrow),
            "narrow flips to records"
        );
        // The decision sees EVERY row, not just the first: a cramped cell in a
        // later row flips the block too (full knowledge, like the widths).
        let later = vec![
            "| a | b | c |".to_string(),
            "| Alice Johnson | alice@example.com | Administrator |".to_string(),
        ];
        let later_w = table_column_widths(header, &later, 3, 24);
        assert!(
            table_should_use_records(header, &later, &later_w),
            "a cramped later row flips to records"
        );
        // A single-column table is just a list — never records.
        let one = vec!["| a very long value that wraps a lot here |".to_string()];
        let one_col = table_column_widths("| X |", &one, 1, 12);
        assert!(
            !table_should_use_records("| X |", &one, &one_col),
            "single-column never records"
        );
    }

    #[test]
    fn table_commits_whole_and_previews_while_forming() {
        // The core behavior (docs/table-streaming.md): nothing of an open table
        // reaches scrollback (its widths need every row), while the strip
        // previews the ENTIRE forming grid — so `committed ++ preview` equals
        // the batch render of every prefix (the reply is always fully visible,
        // split between scrollback and the strip), and the whole grid commits
        // at the close sized to all rows.
        let full = "here:\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | a wider cell |\n| 5 | 6 |\ndone";
        let width = 40;
        let batch: Vec<String> = message_lines(Role::Assistant, full, width)
            .iter()
            .map(plain)
            .collect();
        let mut render = StreamRender::new();
        let mut committed: Vec<String> = Vec::new();
        let mut previewed_forming_grid = false;
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let prefix = &full[..end];
            committed.extend(render.commit(prefix, width).iter().map(plain));
            let preview: Vec<String> = render
                .preview(prefix, width, usize::MAX)
                .iter()
                .map(plain)
                .collect();
            // Scrollback never holds a fragment of the open table.
            if !prefix.contains("done") {
                assert!(
                    !committed.iter().any(|r| r.contains('│')),
                    "no table row commits before the close: {committed:?}"
                );
            }
            // Mid-table the strip shows the whole forming grid — borders,
            // header, and the rows seen so far (the just-arrived one included).
            if prefix.ends_with("| 3 | a wider cell |") {
                assert!(
                    preview.iter().any(|r| r.contains('┌'))
                        && preview.iter().any(|r| r.contains("a wider cell")),
                    "the forming grid previews whole: {preview:?}"
                );
                previewed_forming_grid = true;
            }
            // The full-visibility property: while the table is open, scrollback
            // plus the strip reconstruct the batch render of this prefix.
            if ends_in_open_table(prefix) {
                let batch_prefix: Vec<String> = message_lines(Role::Assistant, prefix, width)
                    .iter()
                    .map(plain)
                    .collect();
                let mut visible = committed.clone();
                visible.extend(preview);
                assert_eq!(
                    visible, batch_prefix,
                    "scrollback + strip show the whole render at {prefix:?}"
                );
            }
        }
        committed.extend(render.finish(full, width).iter().map(plain));
        assert_eq!(committed, batch, "the whole grid commits at the close");
        assert!(previewed_forming_grid);
        // And the close sized the columns to the WIDEST row, not the first.
        assert!(
            committed.iter().any(|r| r.contains("│ a wider cell │")),
            "columns fit the widest row: {committed:?}"
        );
    }

    #[test]
    fn table_preview_caps_to_its_newest_rows() {
        // A forming table taller than the strip's budget tail-follows: the cap
        // keeps the NEWEST rows (the frontier stays visible), dropping the top
        // (docs/table-streaming.md).
        let mut full = String::from("| A | B |\n|---|---|\n");
        for i in 0..30 {
            full.push_str(&format!("| r{i} | v{i} |\n"));
        }
        let width = 40;
        let mut render = StreamRender::new();
        let _ = render.commit(&full, width);
        let capped: Vec<String> = render.preview(&full, width, 6).iter().map(plain).collect();
        assert_eq!(capped.len(), 6, "capped to the budget: {capped:?}");
        assert!(
            capped.iter().any(|r| r.contains("r29")),
            "the newest row stays visible: {capped:?}"
        );
        assert!(
            !capped.iter().any(|r| r.contains('┌')),
            "the top border scrolled out of the capped window: {capped:?}"
        );
        // Uncapped, the whole forming block previews.
        assert!(render.preview(&full, width, usize::MAX).len() > 30);
    }

    #[test]
    fn preview_rows_reports_the_injected_stream_preview_height() {
        // Only the boundary's StreamRender knows the multi-row preview's height,
        // so it injects the count (`set_stream_preview_rows`, the
        // set_status_times pattern) and `preview_rows` reserves it — 1 (the
        // single-row preview) until a draw injects otherwise, 0 when idle.
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(preview_rows(&app, 40), 1, "default: the single-row preview");
        app.set_stream_preview_rows(5);
        assert_eq!(preview_rows(&app, 40), 5, "the injected height is reserved");
        app.finish_stream();
        app.end_turn(1);
        assert_eq!(
            preview_rows(&app, 40),
            0,
            "idle reserves nothing, however stale the injected count"
        );
    }

    #[test]
    fn stream_preview_max_rows_leaves_room_for_the_chrome() {
        // The cap the boundary passes to `preview`: the screen minus the strip
        // gaps + status + box + footer chrome, floored so a tiny terminal still
        // previews a few rows.
        assert_eq!(
            stream_preview_max_rows(24),
            usize::from(24 - STREAM_PREVIEW_RESERVED_ROWS)
        );
        assert_eq!(stream_preview_max_rows(4), STREAM_PREVIEW_MIN_ROWS);
    }

    #[test]
    fn assistant_inline_emphasis_styles_spans() {
        let lines = message_lines(Role::Assistant, "a **b** _i_ ~~s~~ `c`", 80);
        let spans: Vec<(String, Modifier, Option<Color>)> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| (s.content.to_string(), s.style.add_modifier, s.style.fg))
            .collect();
        assert!(
            spans
                .iter()
                .any(|(t, m, _)| t == "b" && m.contains(Modifier::BOLD))
        );
        assert!(
            spans
                .iter()
                .any(|(t, m, _)| t == "i" && m.contains(Modifier::ITALIC))
        );
        assert!(
            spans
                .iter()
                .any(|(t, m, _)| t == "s" && m.contains(Modifier::CROSSED_OUT))
        );
        assert!(
            spans
                .iter()
                .any(|(t, _, fg)| t == "c" && *fg == Some(INLINE_CODE_COLOR))
        );
        // No raw markers leak through.
        let joined: String = spans.iter().map(|(t, _, _)| t.as_str()).collect();
        assert!(!joined.contains("**") && !joined.contains("~~") && !joined.contains('`'));
    }

    #[test]
    fn assistant_inline_link_shows_text_then_url() {
        let joined: String = message_lines(Role::Assistant, "see [docs](https://x.com) ok", 80)
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.to_string())
            .collect();
        assert!(joined.contains("docs"), "link text kept: {joined:?}");
        assert!(joined.contains("(https://x.com)"), "url shown: {joined:?}");
        assert!(
            !joined.contains("[docs]"),
            "raw link syntax gone: {joined:?}"
        );
    }

    #[test]
    fn inline_emphasis_survives_a_wrap_boundary() {
        // A bold run that wraps keeps its style on every wrapped word.
        let lines = message_lines(Role::Assistant, "**alpha beta gamma delta**", 12);
        assert!(lines.len() >= 2, "wrapped into multiple rows");
        for line in &lines {
            for s in &line.spans {
                let t = s.content.as_ref();
                if t.trim().is_empty() || t == "● " {
                    continue;
                }
                assert!(
                    s.style.add_modifier.contains(Modifier::BOLD),
                    "{t:?} should stay bold across the wrap"
                );
            }
        }
    }

    #[test]
    fn assistant_renders_bullet_and_ordered_lists() {
        // Bullets keep the `-`, ordered items keep `N.`, and nesting indent
        // survives (the earlier wrap_text-collapses-whitespace bug).
        let text = "- first\n- second\n  - nested\n\n1. one\n2. two";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 40)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            rows,
            vec![
                "● - first",
                "  - second",
                "    - nested",
                "  ",
                "  1. one",
                "  2. two",
            ]
        );
    }

    #[test]
    fn ordered_list_number_is_colored() {
        let lines = message_lines(Role::Assistant, "1. item", 40);
        let num = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("1."))
            .expect("the ordered marker span");
        assert_eq!(num.style.fg, Some(LIST_MARKER_COLOR));
    }

    #[test]
    fn assistant_renders_blockquote() {
        let rows: Vec<String> = message_lines(Role::Assistant, "> quoted text", 40)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(rows, vec!["● > quoted text"]);
    }

    #[test]
    fn list_item_wraps_with_a_hanging_indent() {
        // A long bullet wraps under the text, not back to the marker column.
        let rows: Vec<String> = message_lines(Role::Assistant, "- alpha beta gamma delta", 12)
            .iter()
            .map(plain)
            .collect();
        // content width 10: "- " marker leaves 8 for text.
        assert_eq!(
            rows,
            vec!["● - alpha", "    beta", "    gamma", "    delta"]
        );
    }

    #[test]
    fn assistant_message_renders_a_table_inline() {
        // A table inside a reply: bullet on the first row, the grid indented under
        // it, an interior blank kept between the prose and the table. The header
        // labels are centred over their left-aligned data (`header_align`).
        let text = "Here:\n\n| Name | Type |\n|------|------|\n| Alpha | String |";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 40)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            rows,
            vec![
                "● Here:",
                "  ",
                "  ┌───────┬────────┐",
                "  │ Name  │  Type  │",
                "  ├───────┼────────┤",
                "  │ Alpha │ String │",
                "  └───────┴────────┘",
            ]
        );
    }

    #[test]
    fn assistant_table_sizes_columns_from_all_rows() {
        // The reported issue: a key/value table whose header is EMPTY (`| | |`)
        // and whose first data row is much narrower than the later ones. The old
        // lock-at-first-data-row streaming sized column 1 from "Time" (4) and
        // column 2 from "03:00 (CEST, GMT+2)" (19), shattering every later row
        // into slivers ("Temp/erat/ure", "10.1 km/h from NNE/(28°)"). Widths
        // must fit ALL rows (docs/table-streaming.md).
        let text = "**Warsaw, Poland**\n\n\
                    | | |\n\
                    |---|---|\n\
                    | **Time** | 03:00 (CEST, GMT+2) |\n\
                    | **Temperature** | 20.2 °C |\n\
                    | **Wind** | 10.1 km/h from NNE (28°) |\n\
                    | **Condition** | Clear sky (WMO code 0) |\n\
                    | **Day/Night** | Night |\n\n\
                    Data from Open-Meteo.";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            rows.iter().any(|r| r.contains("│ Temperature │ 20.2 °C")),
            "columns fit every row, not just the first: {rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("│ 10.1 km/h from NNE (28°)")),
            "the widest cell sits on one line: {rows:#?}"
        );
    }

    /// The rendered cell widths of a table's grid, read off its `┌──┬──┐` top
    /// border — the widths the columns were actually allocated, minus the two
    /// pad spaces each side of a cell.
    fn grid_column_widths(rows: &[String]) -> Vec<usize> {
        // The border may carry the message bullet (`● ┌──…`) when the table is
        // the reply's first block, so slice from the corner itself.
        let top = rows
            .iter()
            .find_map(|r| r.find('┌').map(|i| &r[i..]))
            .expect("a grid top border");
        top.trim()
            .trim_start_matches('┌')
            .trim_end_matches('┐')
            .split('┬')
            .map(|seg| cols(seg).saturating_sub(2))
            .collect()
    }

    #[test]
    fn assistant_table_gives_the_content_heavy_column_the_room() {
        // The reported screenshot (img1 → img2): one long cell in the Check
        // column pulled it to ~35 columns — width every *other* row wasted —
        // while the Result column, holding a ~100-column value, was starved into
        // four wrapped rows. Claude Code gives each column what its longest word
        // needs and spends the rest where the content is, so Result ends up the
        // wide one.
        let text = "| Check | Result |\n\
                    |-------|--------|\n\
                    | `cargo fmt --check` | ✅ clean |\n\
                    | `cargo clippy --all-targets -- -D warnings` | ✅ pass (exit 0) |\n\
                    | `cargo test` | ✅ all tests pass, 0 failures (23 live-API tests \
                    skipped — need `OPENROUTER_API_KEY` / `A0_VENICE_API_KEY`) |\n\
                    | tmux | ✅ installed — tmux 3.6b (`/usr/bin/tmux`) |";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
            .iter()
            .map(plain)
            .collect();
        let w = grid_column_widths(&rows);
        assert!(
            w[1] > w[0],
            "the content-heavy column gets the room: {w:?} in {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("│ cargo fmt --check ")),
            "a short cell still sits on one line: {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("0 failures (23 live-API")),
            "the wide value keeps a scannable run per row: {rows:#?}"
        );
    }

    #[test]
    fn cols_measures_emoji_clusters_as_two_columns() {
        // The Claude-Code / string-width policy, delivered by unicode-width 0.2:
        // every emoji cluster — a VS16 presentation pair, a ZWJ sequence, a
        // skin-tone modifier, a flag, a keycap — is TWO columns, matching how
        // modern terminals draw them. All the table column math (natural widths,
        // allocation, cell padding) rests on this, so a dependency regression
        // here would shatter emoji grids again.
        for (cluster, what) in [
            ("✅", "EAW-wide check mark"),
            ("⚠\u{FE0F}", "VS16 emoji-presentation pair"),
            ("👍🏽", "skin-tone modifier sequence"),
            ("👨\u{200D}👩\u{200D}👧\u{200D}👦", "family ZWJ sequence"),
            ("🇵🇭", "regional-indicator flag pair"),
            ("1\u{FE0F}\u{20E3}", "keycap sequence"),
            ("❤\u{FE0F}\u{200D}🔥", "ZWJ sequence over a VS16 base"),
        ] {
            assert_eq!(cols(cluster), 2, "{what}: {cluster:?}");
        }
    }

    #[test]
    fn table_rows_with_mixed_emoji_clusters_render_the_same_width() {
        // Every rendered row — borders and content rows alike — spans the same
        // display columns whatever mix of clusters the cells hold, so the `│`
        // seams line up.
        let lines: Vec<String> = [
            "| Status | Name | Note |",
            "| --- | --- | --- |",
            "| ✅ | build | emoji cell |",
            "| ⚠\u{FE0F} | lint 🔥 | mixed 👍🏽 emoji |",
            "| 👨\u{200D}👩\u{200D}👧\u{200D}👦 | family | ZWJ cluster |",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = table_content_rows(&lines, 60);
        assert!(rows.len() > 5, "a full grid renders: {rows:?}");
        let widths: Vec<usize> = rows
            .iter()
            .map(|r| r.iter().map(|s| cols(&s.content)).sum())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "every grid row spans the same columns: {widths:?}"
        );
    }

    #[test]
    fn emoji_table_rows_all_end_at_the_same_column() {
        // A 2-column-wide grapheme must be measured as two columns everywhere —
        // the cell's natural width, its wrap, and its pad — so an emoji row is
        // exactly as wide as a border row. (The terminal-side half of this bug
        // was the wide-grapheme filler cell the draw paths used to print, see
        // `term::visible_cells`.)
        let text = "| Check | Result |\n\
                    |-------|--------|\n\
                    | fmt | ✅ clean |\n\
                    | test | ❌ 3 failures |\n\
                    | docs | 世界 ok |";
        for width in [24u16, 40, 60, 80, 120] {
            let rows: Vec<String> = message_lines(Role::Assistant, text, width)
                .iter()
                .map(plain)
                .collect();
            let grid: Vec<&String> = rows.iter().filter(|r| r.contains('│')).collect();
            assert!(!grid.is_empty(), "a grid at width {width}: {rows:#?}");
            let widths: Vec<usize> = rows
                .iter()
                .filter(|r| {
                    let t = r.trim_start();
                    t.starts_with('│')
                        || t.starts_with('┌')
                        || t.starts_with('├')
                        || t.starts_with('└')
                })
                .map(|r| cols(r.trim_end()))
                .collect();
            assert!(
                widths.windows(2).all(|p| p[0] == p[1]),
                "every grid row is the same width at {width}: {widths:?} in {rows:#?}"
            );
            assert!(
                widths[0] <= width as usize,
                "the grid fits the terminal at {width}: {widths:?}"
            );
        }
    }

    #[test]
    fn assistant_table_joins_hard_wrapped_rows() {
        // The reported bug: the model echoed a table whose source carries a
        // terminal line break MID-ROW — google's RTT cell split
        // (`… | 86.8 / 89.8 /` ␤ `96.4 ms |`, the first piece unterminated) and
        // facebook's last cell wholly on the next line (`… | 0% |` ␤
        // `15.0 / 16.4 / 17.9 ms |`). Strict GFM reads each line as a row, so
        // the fragments became phantom one-cell rows (`│ 96.4 ms │ │ │ …`).
        // Inside a leading-pipe table a genuine row starts with `|`; a
        // pipe-carrying line that doesn't is the previous row's tail and must
        // re-join it (docs/table-streaming.md).
        let text = "All three pings completed in parallel. Here's the summary:\n\n\
                    | Host | IP | Packets | Loss | RTT min/avg/max |\n\
                    |------|----|---------|------|-----------------|\n\
                    | **google.com** | 2404:6800:4017:809::200e | 10/10 | 0% | 86.8 / 89.8 /\n\
                    96.4 ms |\n\
                    | **facebook.com** | 2a03:2880:f372:1:face:b00c:0:25de | 10/10 | 0% |\n\
                    15.0 / 16.4 / 17.9 ms |\n\
                    | **x.com** | 162.159.140.229 | 10/10 | 0% | 14.4 / 15.5 / 16.2 ms |\n\n\
                    done";
        let rows: Vec<String> = message_lines(Role::Assistant, text, 120)
            .iter()
            .map(plain)
            .collect();
        assert!(
            rows.iter().any(|r| r.contains("86.8 / 89.8 / 96.4 ms")),
            "google's split RTT cell is rejoined: {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("│ 15.0 / 16.4 / 17.9 ms")),
            "facebook's wrapped last cell lands in the RTT column: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|r| r.trim_start().starts_with("│ 96.4 ms")),
            "no phantom row from a wrapped fragment: {rows:#?}"
        );
        assert_eq!(
            rows.iter().filter(|r| r.contains('├')).count(),
            3,
            "three data rows → header rule + two inter-row rules: {rows:#?}"
        );
    }

    #[test]
    fn wrapped_row_join_accepts_fragments_and_rejects_real_rows() {
        // A pipe-carrying fragment that doesn't start with `|` re-joins the
        // previous row — the mid-cell wrap (unterminated first piece)…
        assert_eq!(
            join_wrapped_table_row("| a | b | 86.8 /", "96.4 ms |", 3).as_deref(),
            Some("| a | b | 86.8 / 96.4 ms |")
        );
        // …and the at-cell-boundary wrap (first piece `|`-terminated but short).
        assert_eq!(
            join_wrapped_table_row("| a | b |", "c |", 3).as_deref(),
            Some("| a | b | c |")
        );
        // A line that DECLARES itself a row (leading `|`) never joins.
        assert_eq!(join_wrapped_table_row("| a | b | c /", "| d |", 3), None);
        // A join that would overflow the column count is a real row, not a tail.
        assert_eq!(join_wrapped_table_row("| a | b | c |", "d | e |", 3), None);
        // In the no-leading-pipe row style, rows legitimately don't start with
        // `|`, so a pipe-carrying follow-up is a row of its own — never joined
        // even when the merged cell count would fit.
        assert_eq!(join_wrapped_table_row("a | b", "c | d", 4), None);
    }

    #[test]
    fn stream_render_withholds_a_table_until_it_closes() {
        // A table is not prefix-stable, so nothing is committed while it is open;
        // `finish` flushes the whole block, matching the batch render exactly.
        let width = 40;
        let open = "| a | b |\n|---|---|\n| 1 | 2 |"; // header + delim + a row, unclosed
        let mut render = StreamRender::new();
        assert!(
            render.commit(open, width).is_empty(),
            "an open table commits nothing to scrollback"
        );
        let flushed = render.finish(open, width);
        assert_eq!(
            flushed,
            message_lines(Role::Assistant, open, width),
            "finish flushes the buffered table, matching the batch render"
        );
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

    // --- wrap_output (word-boundary + whitespace-preserving, for tool output) ---

    #[test]
    fn wrap_output_breaks_at_word_boundaries_keeping_the_space() {
        let rows = wrap_output("sudo: a terminal is required", 12);
        assert!(rows.len() > 1, "the line wraps: {rows:?}");
        for r in &rows {
            assert!(cols(r) <= 12, "no row overflows: {rows:?}");
        }
        for r in &rows[1..] {
            assert!(
                !r.starts_with(' '),
                "continuations start at a word: {rows:?}"
            );
        }
        // The boundary space stays at the end of the current row, so
        // concatenating the rows reconstructs the line byte-exactly.
        assert_eq!(rows.concat(), "sudo: a terminal is required");
        for r in &rows[..rows.len() - 1] {
            assert!(
                r.ends_with(' '),
                "each break lands just past a space: {rows:?}"
            );
        }
    }

    #[test]
    fn wrap_output_preserves_internal_space_runs_that_fit() {
        // Column-aligned output that fits is untouched — byte-exact.
        assert_eq!(
            wrap_output("-rw-r--r--  1 user   42 a.txt", 60),
            vec!["-rw-r--r--  1 user   42 a.txt"]
        );
    }

    #[test]
    fn wrap_output_hard_breaks_an_overlong_word() {
        // A single word wider than the width can't break at a space — it
        // hard-breaks on grapheme boundaries like wrap_verbatim.
        let rows = wrap_output(&"x".repeat(25), 10);
        assert_eq!(rows, vec!["x".repeat(10), "x".repeat(10), "x".repeat(5)]);
    }

    #[test]
    fn wrap_output_keeps_leading_indentation() {
        assert_eq!(
            wrap_output("    indented text", 40),
            vec!["    indented text"]
        );
    }

    #[test]
    fn wrap_output_preserves_empty_lines() {
        assert_eq!(wrap_output("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_output_zero_width_disables_wrapping() {
        assert_eq!(wrap_output("a b c", 0), vec!["a b c"]);
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

    // --- assistant markdown: fenced code blocks + headings (docs/markdown.md) ---

    #[test]
    fn assistant_prose_is_byte_identical_to_the_plain_path() {
        // Fence/heading-free assistant text must render exactly as before — the
        // markdown path is transparent to ordinary prose (regression guard).
        let text = "the quick brown fox jumps over the lazy dog and then more";
        let width = 20u16;
        let expected = wrap_text(text, width - BULLET_WIDTH);
        let got: Vec<String> = message_lines(Role::Assistant, text, width)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(got.len(), expected.len());
        assert_eq!(got[0], format!("● {}", expected[0]));
        for (g, e) in got.iter().zip(expected.iter()).skip(1) {
            assert_eq!(g, &format!("  {e}"));
        }
    }

    #[test]
    fn assistant_code_block_preserves_indentation() {
        // THE BUG: fenced code keeps its leading whitespace — not collapsed the
        // way prose word-wrap would.
        let src = "```py\ndef f():\n    return 1\n```";
        let joined: Vec<String> = message_lines(Role::Assistant, src, 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            joined.iter().any(|l| l.contains("    return 1")),
            "4-space indent kept: {joined:?}"
        );
        assert!(joined.iter().any(|l| l.contains("def f():")), "{joined:?}");
    }

    #[test]
    fn assistant_code_hides_the_fences_and_the_language_label() {
        let joined: Vec<String> = message_lines(Role::Assistant, "```python\nx = 1\n```", 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            !joined.iter().any(|l| l.contains("```")),
            "fences hidden: {joined:?}"
        );
        assert!(
            !joined.iter().any(|l| l.contains("python")),
            "language label NOT shown: {joined:?}"
        );
        // The code itself still renders.
        assert!(
            joined.iter().any(|l| l.contains("x = 1")),
            "code shown: {joined:?}"
        );
    }

    #[test]
    fn assistant_code_rows_have_no_gutter_and_use_the_code_colour() {
        let lines = message_lines(Role::Assistant, "```\nx=1\n```", 80);
        let code = lines
            .iter()
            .find(|l| plain(l).contains("x=1"))
            .expect("a code row");
        assert!(
            !plain(code).contains('▏'),
            "no gutter bar: {:?}",
            plain(code)
        );
        assert!(
            code.spans
                .iter()
                .any(|s| s.style.fg == highlight::plain_style().fg),
            "unhighlighted code text uses the plain (theme-default) code colour"
        );
    }

    #[test]
    fn assistant_over_width_code_hard_breaks_keeping_whitespace() {
        // A code line wider than the code area hard-breaks (no word-collapse); the
        // first row keeps the leading indentation. There is no language label and
        // no gutter, so the rendered rows are the wrapped code alone (under the
        // bullet/indent).
        let src = "```\n        eight_spaces_then_a_very_long_token_here\n```";
        let lines = message_lines(Role::Assistant, src, 22);
        let code: Vec<String> = lines.iter().map(plain).collect();
        // At least two wrapped code rows (no label row now).
        assert!(code.len() >= 2, "hard-broke into rows: {code:?}");
        assert!(
            code.iter().any(|l| l.contains("        eight")),
            "leading spaces survive on the first code row: {code:?}"
        );
    }

    #[test]
    fn assistant_code_expands_tabs_so_indentation_survives() {
        // THE BUG: Go (and many langs) indent with TAB. A tab is zero display
        // width (unicode-width), so rendered verbatim it collapses and the code
        // loses all its indentation. Tabs must be expanded to spaces for display
        // (matching codex — a fixed CODE_TAB_WIDTH substitution, not tab stops).
        let src = "```go\nfunc main() {\n\tfmt.Println(\"hi\")\n\t\tnested()\n}\n```";
        let lines = message_lines(Role::Assistant, src, 80);
        let row = |needle: &str| -> String {
            plain(lines.iter().find(|l| plain(l).contains(needle)).unwrap())
        };
        let println = row("fmt.Println");
        assert!(!println.contains('\t'), "no raw tab survives: {println:?}");
        // The 2-col continuation indent, then one tab → CODE_TAB_WIDTH spaces.
        let one = "  ".to_string() + &" ".repeat(CODE_TAB_WIDTH);
        assert!(
            println.starts_with(&format!("{one}fmt.Println")),
            "one tab expands to {CODE_TAB_WIDTH} spaces of indent: {println:?}"
        );
        // Two tabs → twice the indent, so nesting reads as deeper.
        let nested = row("nested()");
        let two = "  ".to_string() + &" ".repeat(CODE_TAB_WIDTH * 2);
        assert!(
            nested.starts_with(&format!("{two}nested()")),
            "two tabs expand to {} spaces: {nested:?}",
            CODE_TAB_WIDTH * 2
        );
    }

    #[test]
    fn assistant_headings_keep_the_hashes_and_style_per_level_like_codex() {
        // Codex keeps the `#` markers visible and styles the whole heading line
        // per level with text *modifiers only* (no foreground colour): h1
        // bold+underlined, h2 bold, h3 bold+italic, h4-6 italic. See
        // docs/markdown.md and codex-rs/tui/src/markdown_render.rs::start_heading.
        let h2 = message_lines(Role::Assistant, "## The Code", 80);
        assert_eq!(plain(&h2[0]), "● ## The Code", "hashes kept, not stripped");

        // The heading-text span carries the level's modifiers and no fg override.
        let style_of = |level: u8| {
            let src = format!("{} Heading", "#".repeat(level as usize));
            let lines = message_lines(Role::Assistant, &src, 80);
            lines[0]
                .spans
                .iter()
                .find(|s| s.content.contains("Heading"))
                .expect("heading text span")
                .style
        };
        for level in 1..=6u8 {
            assert_eq!(
                style_of(level).fg,
                None,
                "codex headings carry no colour (h{level})"
            );
        }
        let m = |level: u8| style_of(level).add_modifier;
        assert!(
            m(1).contains(Modifier::BOLD) && m(1).contains(Modifier::UNDERLINED),
            "h1 bold+underlined"
        );
        assert!(
            m(2).contains(Modifier::BOLD)
                && !m(2).contains(Modifier::ITALIC)
                && !m(2).contains(Modifier::UNDERLINED),
            "h2 bold only"
        );
        assert!(
            m(3).contains(Modifier::BOLD) && m(3).contains(Modifier::ITALIC),
            "h3 bold+italic"
        );
        assert!(
            m(4).contains(Modifier::ITALIC) && !m(4).contains(Modifier::BOLD),
            "h4 italic only"
        );
        assert!(
            m(6).contains(Modifier::ITALIC) && !m(6).contains(Modifier::BOLD),
            "h6 italic"
        );
    }

    #[test]
    fn assistant_thematic_break_renders_an_em_dash_rule_like_codex() {
        // `---`/`***`/`___` after a blank line render as codex's `———` (Event::Rule),
        // with the raw markers gone.
        for src in [
            "intro\n\n---\nmore",
            "intro\n\n***\nmore",
            "intro\n\n___\nmore",
        ] {
            let joined: Vec<String> = message_lines(Role::Assistant, src, 80)
                .iter()
                .map(plain)
                .collect();
            assert!(
                joined.iter().any(|l| l.contains(THEMATIC_BREAK)),
                "em-dash rule rendered for {src:?}: {joined:?}"
            );
            assert!(
                !joined
                    .iter()
                    .any(|l| l.contains("---") || l.contains("***") || l.contains("___")),
                "raw markers gone for {src:?}: {joined:?}"
            );
        }
    }

    #[test]
    fn a_dash_rule_without_a_preceding_blank_stays_literal() {
        // `text\n---` is a setext H2 underline in codex, which we can't render;
        // rather than fabricate a rule we leave `---` as literal prose.
        let joined: Vec<String> = message_lines(Role::Assistant, "some text\n---", 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            joined.iter().any(|l| l.contains("---")),
            "literal --- kept: {joined:?}"
        );
        assert!(
            !joined.iter().any(|l| l.contains(THEMATIC_BREAK)),
            "no fabricated rule: {joined:?}"
        );
    }

    #[test]
    fn assistant_indented_code_renders_verbatim_like_codex() {
        // A 4-space-indented run after a blank is an indented code block: its
        // indentation is preserved (unlike prose, which collapses leading space).
        let joined: Vec<String> = message_lines(
            Role::Assistant,
            "intro\n\n    x = 1\n        deep()\nback",
            80,
        )
        .iter()
        .map(plain)
        .collect();
        assert!(
            joined.iter().any(|l| l.contains("    x = 1")),
            "4-space indent kept: {joined:?}"
        );
        assert!(
            joined.iter().any(|l| l.contains("        deep()")),
            "8-space indent kept: {joined:?}"
        );
    }

    #[test]
    fn assistant_code_is_syntax_highlighted() {
        // We assert real, multi-colour highlighting without pinning the theme's
        // exact RGB (codex's own test style): each token class is coloured, and
        // the classes differ from one another and from plain prose.
        let lines = message_lines(
            Role::Assistant,
            "```python\ndef f():\n    x = \"hi\"  # note\n```",
            80,
        );
        let fg_of = |needle: &str| -> Option<Color> {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.contains(needle))
                .and_then(|s| s.style.fg)
        };
        let kw = fg_of("def");
        let func = fg_of("f");
        let string = fg_of("\"hi\"");
        let comment = fg_of("# note");
        for (name, c) in [
            ("def", kw),
            ("f", func),
            ("\"hi\"", string),
            ("# note", comment),
        ] {
            assert!(c.is_some(), "{name} should be coloured");
        }
        assert_ne!(kw, string, "keyword and string differ");
        assert_ne!(string, comment, "string and comment differ");
        assert_ne!(kw, comment, "keyword and comment differ");
    }

    #[test]
    fn streamed_code_never_recolours_a_committed_row() {
        // A code line LONGER than the code width wraps into several rows; the
        // highlighter's one-char lookahead (a call's `(`) must not recolour an
        // already-committed row when it finally streams in. Compare the streamed
        // commits' SPAN COLOURS (not just text) to the final render.
        let full = "```py\nsome_really_long_function_name_here()\n```";
        let width = 22; // content_width 20 → the long name wraps
        let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), s.style.fg))
                .collect()
        };
        let expected: Vec<Vec<(String, Option<Color>)>> =
            message_lines(Role::Assistant, full, width)
                .iter()
                .map(styled)
                .collect();

        // Stream char-by-char so a commit boundary lands mid-identifier.
        let mut render = StreamRender::new();
        let mut got: Vec<Vec<(String, Option<Color>)>> = Vec::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            got.extend(render.commit(&full[..end], width).iter().map(styled));
        }
        got.extend(render.finish(full, width).iter().map(styled));

        assert_eq!(got, expected, "a committed code row must never recolour");
    }

    #[test]
    fn code_fences_stay_literal_for_non_assistant_roles() {
        // A user pasting triple-backticks must not be markdown-processed.
        let joined: Vec<String> = message_lines(Role::User, "```\ncode\n```", 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            joined.iter().any(|l| l.contains("```")),
            "user fences stay literal: {joined:?}"
        );
    }

    #[test]
    fn incremental_commits_reconstruct_a_fenced_code_reply() {
        // The critical prefix-stability integration test: stream a reply that
        // contains a fenced code block chunk-by-chunk; the committed lines plus
        // the final flush must exactly equal the fully-rendered message, so
        // scrollback never disagrees with a resize/Ctrl+O repaint.
        let full = "Here is code:\n```python\ndef f():\n    return 1\n\n    x = 2\n```\nDone.";
        let width = 24;
        let expected: Vec<String> = message_lines(Role::Assistant, full, width)
            .iter()
            .map(plain)
            .collect();

        let mut render = StreamRender::new();
        let mut got: Vec<String> = Vec::new();
        let mut acc = String::new();
        for chunk in crate::stream::chunks(full) {
            acc.push_str(&chunk);
            got.extend(render.commit(&acc, width).iter().map(plain));
        }
        got.extend(render.finish(&acc, width).iter().map(plain));

        assert_eq!(got, expected, "streamed commits reconstruct the code reply");
    }

    #[test]
    fn a_fence_only_reply_renders_a_lone_bullet_in_batch_and_streaming() {
        // With fences hidden and no language label, a reply that is *only* a code
        // fence renders to zero body rows — but the role bullet still needs a
        // home, and the batch and streaming paths must agree on it (else the strip
        // preview would diverge from a scrollback repaint).
        let full = "```";
        let width = 40;
        let expected: Vec<String> = message_lines(Role::Assistant, full, width)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            expected,
            vec!["● ".to_string()],
            "batch: a lone bullet home"
        );

        let mut render = StreamRender::new();
        assert_eq!(
            render
                .preview(full, width, usize::MAX)
                .iter()
                .map(plain)
                .collect::<Vec<_>>(),
            vec!["● ".to_string()],
            "preview matches the batch bullet home"
        );
        let committed: Vec<String> = render.finish(full, width).iter().map(plain).collect();
        assert_eq!(committed, expected, "streamed finish matches batch");
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
    fn tool_lines_collapses_a_command_output_to_a_multiline_peek_plus_hint() {
        // A finished command-style backend tool (bash) shows up to TOOL_PEEK_LINES
        // of its output — the head, Claude-Code style — then a
        // `… +N lines (ctrl+o to expand)` hint (docs/tool-streaming.md), like the
        // `!` shell cell. (This is the mock's finished state.)
        let out = "l1\nl2\nl3\nl4\nl5\nl6";
        let lines = tool_lines(&tool("Bash", "seq 6", ToolStatus::Ok, out), 80);
        assert_eq!(
            lines.len(),
            TOOL_PEEK_LINES + 2,
            "header + {TOOL_PEEK_LINES} peek rows + hint: {:?}",
            lines.iter().map(plain).collect::<Vec<_>>()
        );
        assert!(
            plain(&lines[1]).contains("l1"),
            "peek opens at the first line"
        );
        assert!(
            plain(&lines[TOOL_PEEK_LINES]).contains("l4"),
            "peek shows up to the {TOOL_PEEK_LINES}th line"
        );
        let hint = plain(&lines[TOOL_PEEK_LINES + 1]);
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
    fn tool_lines_wraps_a_long_command_output_line_instead_of_clipping() {
        // The FINISHED (committed) bash cell must wrap a long output line like
        // the Ctrl+O view does, not clip it at the width — the text used to
        // disappear past the terminal edge (the reported bug;
        // docs/tool-streaming.md).
        let long = "0123456789".repeat(6); // 60 cols
        let lines = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &long), 40);
        // width 40 − the 5-col `  ⎿  ` gutter = 35 content cols → 2 rows, and a
        // single source line → no hint.
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert_eq!(body.len(), 2, "the 60-col line wraps to two rows: {body:?}");
        let joined: String = body
            .iter()
            .map(|l| l.chars().skip(5).collect::<String>()) // drop the 5-col gutter
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(joined, long, "every column is preserved, not clipped");
        for l in &lines {
            assert!(cols(&plain(l)) <= 40, "no row overflows the width: {l:?}");
        }
    }

    #[test]
    fn a_finished_shell_output_wraps_a_long_line_showing_every_line() {
        // The reported bug, mechanism for mechanism: a `! sudo …` cell whose
        // first output line is longer than the terminal used to lose the text
        // past the edge. Both lines must show in full, wrapped, with no expand
        // hint (they fit the row budget).
        let out = "sudo: a terminal is required to read the password; either use the -S option\n\
                   sudo: a password is required";
        let mut t = tool("sudo pacman -Rns steam", "", ToolStatus::Ok, out);
        t.shell = true;
        let lines: Vec<String> = tool_lines(&t, 50).iter().map(plain).collect();
        let joined = lines.join("\n");
        assert!(
            joined.contains("either use the -S option"),
            "the full first line survives the wrap: {joined:?}"
        );
        assert!(
            joined.contains("a password is required"),
            "the second line still shows: {joined:?}"
        );
        assert!(
            !joined.contains("ctrl+o to expand"),
            "both lines fit the budget — no hint: {joined:?}"
        );
        for l in &lines {
            assert!(cols(l) <= 50, "no row overflows the width: {l:?}");
        }
    }

    #[test]
    fn a_finished_peek_shows_the_first_lines_fully_wrapped() {
        // The peek budget is SOURCE lines (`TOOL_PEEK_LINES` of them, each
        // fully wrapped) — "the first 4 lines of output", not "the first 4
        // display rows": a long first line must not push its siblings out of
        // the peek. Three lines here, the first wrapping to 3 rows → all
        // three lines visible (5 rows), no hint.
        let out = format!("{}\nbee\nsea", "a".repeat(80)); // 35 content cols → 3 rows
        let lines: Vec<String> = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &out), 40)
            .iter()
            .map(plain)
            .collect();
        let joined = lines.join("\n");
        assert!(
            joined.contains("bee") && joined.contains("sea"),
            "every source line within the budget shows: {lines:?}"
        );
        assert!(
            !joined.contains("ctrl+o to expand"),
            "nothing is hidden — no hint: {lines:?}"
        );
        assert_eq!(lines.len(), 1 + 5, "header + 3+1+1 wrapped rows: {lines:?}");
    }

    #[test]
    fn a_finished_peek_bounds_rows_and_hints_when_one_line_overflows_the_budget() {
        // The safety ceiling: a single pathological line (a minified bundle)
        // wraps but is bounded to TOOL_PEEK_MAX_ROWS display rows, so it can't
        // balloon the committed cell into hundreds of rows; the expand hint
        // appears because content is hidden below the ceiling — even though it
        // is one source line (a partially-shown line counts as not-fully-shown).
        let long = "x".repeat(600); // 35 content cols → 18 rows uncapped
        let lines: Vec<String> = tool_lines(&tool("Bash", "cat big", ToolStatus::Ok, &long), 40)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            lines.len(),
            1 + TOOL_PEEK_MAX_ROWS + 1,
            "header + the {TOOL_PEEK_MAX_ROWS}-row ceiling + hint: {lines:?}"
        );
        let hint = lines.last().unwrap();
        assert!(
            hint.contains("ctrl+o to expand"),
            "the hint signals more: {hint:?}"
        );
        assert!(
            hint.contains("+1 lines"),
            "one source line, partially hidden below the window: {hint:?}"
        );
    }

    #[test]
    fn a_finished_peek_wraps_the_sudo_error_at_word_boundaries() {
        // The user-visible polish over the plain hard-break: "askpass" must
        // never render as "as / kpass" across rows — the peek word-wraps like
        // prose while still preserving the line's own spaces.
        let out = "sudo: a terminal is required to read the password; either use \
                   the -S option to read from standard input or configure an \
                   askpass helper";
        let mut t = tool("sudo pacman -Rns steam", "", ToolStatus::Failed, out);
        t.shell = true;
        let rows: Vec<String> = tool_lines(&t, 66).iter().map(plain).collect();
        assert!(
            rows.iter().any(|r| r.contains("askpass")),
            "askpass stays intact on one row: {rows:?}"
        );
        for r in &rows[1..] {
            let content: String = r.chars().skip(5).collect(); // drop the gutter
            assert!(
                !content.starts_with(' '),
                "continuations start at a word: {rows:?}"
            );
        }
    }

    #[test]
    fn a_failed_command_cell_surfaces_its_exit_code() {
        // A red cell says WHY it failed: the display reframes the model-facing
        // `Exit code: N` line as an `Error: Exit code N` header above the body
        // (the raw frame stays in tool.output for the model / context replay).
        let lines = tool_lines(
            &tool("Bash", "false", ToolStatus::Failed, "Exit code: 3\nboom"),
            80,
        );
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert!(
            body[0].contains("Error: Exit code 3"),
            "the failure header leads: {body:?}"
        );
        assert!(body[1].contains("boom"), "the body follows: {body:?}");
    }

    #[test]
    fn a_failed_command_cell_with_no_body_still_says_why() {
        // A failure with empty output used to show "(no output)" — now the
        // exit code itself is the body.
        let lines = tool_lines(
            &tool("Bash", "false", ToolStatus::Failed, "Exit code: 3"),
            80,
        );
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert_eq!(body.len(), 1, "one row: {body:?}");
        assert!(
            body[0].contains("Error: Exit code 3"),
            "the reason shows: {body:?}"
        );
    }

    #[test]
    fn a_signal_killed_command_cell_says_so() {
        let lines = tool_lines(
            &tool(
                "Bash",
                "x",
                ToolStatus::Failed,
                "Exit code: killed by signal",
            ),
            80,
        );
        assert!(
            plain(&lines[1]).contains("Error: killed by signal"),
            "got {:?}",
            plain(&lines[1])
        );
    }

    #[test]
    fn the_full_view_surfaces_the_exit_code_on_failure_too() {
        let lines = tool_full_lines(
            &tool("Bash", "false", ToolStatus::Failed, "Exit code: 2\nnope"),
            80,
        );
        let all = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(
            all.contains("Error: Exit code 2") && all.contains("nope"),
            "got {all:?}"
        );
    }

    #[test]
    fn a_diff_peek_continuation_row_keeps_the_source_line_colour() {
        // The legacy (unparseable-output) diff peek: a long `+` line wraps; the
        // continuation rows must keep the SOURCE line's green, not fall to dim
        // because their own first char has no marker — the Ctrl+O view already
        // colours by source line (diff_line_color before wrapping).
        let long = format!("+{}", "a".repeat(60));
        let lines = tool_lines(&tool("Edit", "f", ToolStatus::Ok, &long), 40);
        let rows: Vec<_> = lines[1..].iter().collect();
        assert!(
            rows.len() >= 2,
            "the long + line wrapped: {:?}",
            rows.iter().map(|l| plain(l)).collect::<Vec<_>>()
        );
        for r in &rows {
            assert_eq!(
                r.spans[1].style.fg,
                Some(TOOL_DIFF_ADD_COLOR),
                "every wrapped row keeps the + colour: {:?}",
                plain(r)
            );
        }
    }

    #[test]
    fn a_non_command_tool_with_raw_multiline_output_keeps_a_single_peek_line() {
        // Only a command tool (bash) expands to a multi-line peek. A generic
        // backend tool whose output isn't the numbered file-cell format (e.g. the
        // dummy's canned `Read`, or an unknown tool) keeps the compact single
        // peek line + hint — so its committed footprint is unchanged
        // (docs/tool-streaming.md; guards the resize/reflow layout, smoke Phase 17).
        let lines = tool_lines(&tool("Read", "f", ToolStatus::Ok, "one\ntwo\nthree"), 80);
        assert_eq!(
            lines.len(),
            3,
            "header + one peek line + hint: {:?}",
            lines.iter().map(plain).collect::<Vec<_>>()
        );
        assert!(
            plain(&lines[1]).contains("one"),
            "peek shows the first line"
        );
        assert!(
            !plain(&lines[1]).contains("two"),
            "the rest stays hidden inline"
        );
        assert!(
            plain(&lines[2]).contains("+2 lines"),
            "the hint counts the rest"
        );
    }

    #[test]
    fn tool_lines_strips_the_leading_exit_code_frame_from_a_bash_cell() {
        // `tool.output` stays framed (`Exit code: N\n…`) for the model / context
        // replay, but the display drops that first line so the cell reads like
        // the real command output (docs/tool-streaming.md).
        let out = "Exit code: 0\nhello\nworld";
        let lines = tool_lines(&tool("Bash", "echo", ToolStatus::Ok, out), 80);
        let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(
            !all.contains("Exit code"),
            "the frame line is hidden: {all:?}"
        );
        assert!(
            all.contains("hello") && all.contains("world"),
            "the body shows: {all:?}"
        );
    }

    #[test]
    fn running_command_lines_tails_recent_output_with_the_elapsed() {
        // The mock's running state: the header, the last TOOL_PEEK_LINES output
        // lines (the *tail* — what just happened), and a `+N lines (Ns)` footer
        // counting the lines hidden above plus the elapsed.
        let out = (1..=9)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tool("Bash", "ping -c 10 x", ToolStatus::Running, &out);
        let lines = running_command_lines(&t, Duration::from_secs(9), 80);
        assert_eq!(plain(&lines[0]), "● Bash(ping -c 10 x)");
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert!(
            body[0].contains("line 6"),
            "the tail starts at line 6: {body:?}"
        );
        assert!(
            body[3].contains("line 9"),
            "the tail ends at the newest line: {body:?}"
        );
        assert!(
            !body.iter().any(|l| l.contains("line 4")),
            "older lines are hidden above the tail: {body:?}"
        );
        assert_eq!(
            body.last().unwrap().trim(),
            "+5 lines (9s)",
            "the footer counts hidden lines and the elapsed: {body:?}"
        );
    }

    #[test]
    fn running_command_lines_without_overflow_shows_no_footer() {
        // Fewer lines than the window: show them all, no `+N lines` footer (the
        // status line carries the timer).
        let t = tool("Bash", "echo", ToolStatus::Running, "a\nb");
        let lines = running_command_lines(&t, Duration::from_secs(1), 80);
        assert_eq!(
            lines.len(),
            3,
            "header + 2 output rows, no footer: {:?}",
            lines.iter().map(plain).collect::<Vec<_>>()
        );
        assert!(
            !lines.iter().any(|l| plain(l).contains("lines (")),
            "no footer when nothing is hidden"
        );
    }

    #[test]
    fn running_command_lines_wraps_a_long_tail_line_instead_of_clipping() {
        // The inline streaming tail must wrap like the Ctrl+O view does
        // (wrap_verbatim — docs/tool-streaming.md), not silently clip at the
        // width: every streamed column stays visible in the live cell.
        let long = "0123456789".repeat(6); // 60 cols
        let t = tool("Bash", "cat log", ToolStatus::Running, &long);
        let lines = running_command_lines(&t, Duration::from_secs(1), 40);
        // width 40 − the 5-col `  ⎿  ` gutter = 35 content cols → 2 rows.
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert_eq!(body.len(), 2, "the 60-col line wraps to two rows: {body:?}");
        // Strip the 5-char `  ⎿  ` gutter / continuation indent off each row.
        let joined: String = body
            .iter()
            .map(|l| l.chars().skip(5).collect::<String>())
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(joined, long, "no column is dropped: {body:?}");
    }

    #[test]
    fn running_command_lines_tail_window_counts_display_rows_when_lines_wrap() {
        // The TOOL_PEEK_LINES cap bounds *display rows*, so a wrapping tail
        // can't grow the strip past its budget — a long newest line
        // tail-follows its own newest rows. The `+N lines` footer keeps
        // counting source lines, and only the ones *fully* hidden above the
        // window (a partially shown wrapped line is on screen, not hidden).
        let long = "x".repeat(70); // 35 content cols → exactly 2 rows
        let out = format!("alpha\nbeta\ngamma\n{long}");
        let t = tool("Bash", "cat log", ToolStatus::Running, &out);
        let lines = running_command_lines(&t, Duration::from_secs(7), 40);
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert_eq!(body.len(), 5, "4 tail rows + the footer: {body:?}");
        assert!(
            !body.iter().any(|l| l.contains("alpha")),
            "alpha is fully hidden above the window: {body:?}"
        );
        assert!(
            body[0].contains("beta") && body[1].contains("gamma"),
            "the window opens on the still-visible lines: {body:?}"
        );
        assert_eq!(body[2].trim(), "x".repeat(35), "the long line wraps…");
        assert_eq!(body[3].trim(), "x".repeat(35), "…across the window's rows");
        assert_eq!(
            body.last().unwrap().trim(),
            "+1 lines (7s)",
            "the footer counts the one fully hidden line: {body:?}"
        );
    }

    #[test]
    fn preview_rows_counts_a_wrapped_running_tail() {
        // Strip sizing and paint agree when the tail wraps: preview_rows
        // counts the wrapped rows (header + windowed tail + the Ctrl+B hint),
        // not one row per source line.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "cat log");
        // Past the hint delay so the Ctrl+B hint row is part of the preview.
        app.set_command_elapsed(Some(Duration::from_secs(3)));
        app.push_tool_output(&"y".repeat(70)); // 35 content cols → 2 rows
        assert_eq!(
            preview_rows(&app, 40),
            4,
            "header + 2 wrapped tail rows + the Ctrl+B hint"
        );
    }

    #[test]
    fn render_live_tails_a_running_bash_tool_with_its_streamed_output() {
        // End-to-end: streamed output accumulates on the running call and the
        // live strip tails it — the newest line and the `+N lines (Ns)` footer
        // both show (docs/tool-streaming.md).
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "ping -c 10 x");
        for i in 1..=9 {
            app.push_tool_output(&format!("line {i}\n"));
        }
        app.set_status_times(Duration::from_secs(9), None);
        let pv = preview_rows(&app, 60);
        let h = live_height(&app.input, 60, 24, true, pv, 0, 0, 0, 0, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 60))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("line 9"), "the newest line tails: {all:?}");
        assert!(!all.contains("line 4"), "older lines are hidden: {all:?}");
        assert!(all.contains("+5 lines (9s)"), "the footer shows: {all:?}");
    }

    #[test]
    fn tool_full_lines_strips_the_exit_code_frame_from_a_bash_cell() {
        // The Ctrl+O full view shows the whole body but, like the inline cell,
        // drops the `Exit code: N` frame line (docs/tool-streaming.md).
        let out = "Exit code: 0\nalpha\nbeta";
        let lines = tool_full_lines(&tool("Bash", "echo", ToolStatus::Ok, out), 80);
        let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(
            !all.contains("Exit code"),
            "the frame is hidden in the full view too: {all:?}"
        );
        assert!(
            all.contains("alpha") && all.contains("beta"),
            "the whole body shows: {all:?}"
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
    fn tool_lines_running_backend_peek_reads_running_capitalized() {
        // The running `⎿` row reads `Running…` (capital, like the shell cell) so
        // the live preview under the header matches Claude-Code's look.
        let lines = tool_lines(&tool("Bash", "sleep 1", ToolStatus::Running, ""), 80);
        assert!(
            plain(&lines[1]).contains("Running…"),
            "backend running peek is capitalised: {:?}",
            plain(&lines[1])
        );
    }

    #[test]
    fn tool_lines_waiting_shows_a_waiting_peek() {
        // A not-yet-started call in a parallel batch renders `● name(args)` over
        // a dim `⎿ Waiting…` row — the queued-but-not-running state. See
        // `docs/parallel-tools.md`.
        let lines = tool_lines(&tool("Bash", "ping x.com", ToolStatus::Waiting, ""), 80);
        assert_eq!(lines.len(), 2, "header + waiting peek");
        assert!(
            plain(&lines[0]).contains("Bash(ping x.com)"),
            "the waiting call still shows its header: {:?}",
            plain(&lines[0])
        );
        assert!(
            plain(&lines[1]).contains("Waiting…"),
            "a waiting call peeks as Waiting…: {:?}",
            plain(&lines[1])
        );
    }

    #[test]
    fn tool_lines_colours_a_waiting_bullet_dim_not_blue() {
        // The waiting bullet is dim grey (distinct from the blue running head),
        // since the call hasn't started.
        let lines = tool_lines(&tool("Bash", "ping x.com", ToolStatus::Waiting, ""), 80);
        let bullet = lines[0].spans.first().expect("a bullet span");
        assert_eq!(
            bullet.style.fg,
            Some(TOOL_WAITING_COLOR),
            "the waiting bullet is dim, not the running blue"
        );
    }

    #[test]
    fn tool_lines_wraps_a_long_header_aligned_under_the_open_paren() {
        // A long `Bash(…)` command must not run off the terminal edge: the args
        // word-wrap across continuation rows, each indented to align **under the
        // opening `(`** (Claude-Code's wrapped header) so the wrap reads clean and
        // no part of the command is lost.
        let cmd = "curl -s \"wttr.in/Warsaw?format=%C+%t+%w+%h\" 2>/dev/null \
                   || echo \"wttr.in unavailable, trying alternative...\"";
        let lines = tool_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 80);
        let header: Vec<String> = lines
            .iter()
            .take_while(|l| !plain(l).contains('⎿'))
            .map(plain)
            .collect();
        // The header spans more than one row but stays within the inline cap.
        assert!(
            (2..=TOOL_HEADER_MAX_ROWS).contains(&header.len()),
            "long header wraps: {header:?}"
        );
        // No row exceeds the width — nothing is clipped.
        for l in &lines {
            assert!(
                cols(&plain(l)) <= 80,
                "row stays within the width: {:?}",
                plain(l)
            );
        }
        // Row 0 opens the header; the `(` sits at `cols("● Bash")`.
        assert!(header[0].starts_with("● Bash("), "row 0 opens the header");
        let paren_col = cols("● Bash");
        assert_eq!(
            header[0].chars().nth(paren_col),
            Some('('),
            "the open paren sits at cols(\"● Bash\")"
        );
        // The continuation is indented by exactly that many spaces, so its first
        // character lands directly under the `(` — not one column past it.
        assert!(
            header[1].starts_with(&" ".repeat(paren_col)),
            "continuation is indented to the open paren: {:?}",
            header[1]
        );
        assert_ne!(
            header[1].chars().nth(paren_col),
            Some(' '),
            "the continuation's content begins right under the (: {:?}",
            header[1]
        );
        // Nothing is lost — the command's start and end both survive.
        let joined: String = header
            .iter()
            .map(|r| r.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("curl -s"),
            "keeps the command head: {joined:?}"
        );
        assert!(
            joined.contains("alternative...\")"),
            "keeps the command tail and closing paren: {joined:?}"
        );
    }

    #[test]
    fn tool_header_body_is_bold_white_parens_included() {
        // The whole `(...)` header body — the command text AND its framing parens
        // — reads like a normal reply (bold + the white assistant colour), so a
        // bash command and its brackets are all noticeable rather than dim.
        let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Ok, "out"), 80);
        let arg = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("cargo"))
            .expect("an args span");
        assert_eq!(
            arg.style.fg,
            Some(TOOL_ARGS_COLOR),
            "args use the normal reply colour"
        );
        assert!(
            arg.style.add_modifier.contains(Modifier::BOLD),
            "args are bold"
        );
        // The opening `(` and closing `)` are the same bold white, not dim.
        let open = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains('('))
            .expect("a span carrying the open paren");
        assert_eq!(
            open.style.fg,
            Some(TOOL_ARGS_COLOR),
            "the opening paren is bold white too, not a dim delimiter"
        );
        let close = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains(')'))
            .expect("a span carrying the close paren");
        assert_eq!(
            close.style.fg,
            Some(TOOL_ARGS_COLOR),
            "the closing paren is bold white too"
        );
    }

    #[test]
    fn tool_lines_truncates_a_very_long_header_with_an_ellipsis() {
        // A very long command is capped inline at TOOL_HEADER_MAX_ROWS wrapped
        // rows, the remainder replaced by `…)` (Claude-Code's truncated command);
        // the whole thing is still shown in the Ctrl+O view.
        let cmd = "for i in {1..5}; do echo \"=== Iteration $i ===\" \
                   && echo \"Current time: $(date)\" \
                   && echo \"System uptime: $(uptime)\" \
                   && echo \"Memory usage: $(free -h | grep Mem)\"; done";
        let lines = tool_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 50);
        let header: Vec<String> = lines
            .iter()
            .take_while(|l| !plain(l).contains('⎿'))
            .map(plain)
            .collect();
        assert_eq!(
            header.len(),
            TOOL_HEADER_MAX_ROWS,
            "header caps at the row limit: {header:?}"
        );
        assert!(
            header
                .last()
                .unwrap()
                .trim_end()
                .ends_with(&format!("{TOOL_HEADER_ELLIPSIS})")),
            "the last shown row ends with the ellipsis + closing paren: {:?}",
            header.last().unwrap()
        );
        // The truncation `…` is the same bold white as the args, not dim grey.
        let ell_line = lines
            .iter()
            .find(|l| plain(l).contains(TOOL_HEADER_ELLIPSIS))
            .unwrap();
        let ell = ell_line
            .spans
            .iter()
            .find(|s| s.content.contains(TOOL_HEADER_ELLIPSIS))
            .unwrap();
        assert_eq!(
            ell.style.fg,
            Some(TOOL_ARGS_COLOR),
            "the truncation … matches the args colour, not grey"
        );
        // Still no clipping past the width.
        for l in &lines {
            assert!(
                cols(&plain(l)) <= 50,
                "row stays within the width: {:?}",
                plain(l)
            );
        }
        // The continuations still align under the `(`.
        let paren_col = cols("● Bash");
        assert!(header[1].starts_with(&" ".repeat(paren_col)));
    }

    #[test]
    fn tool_full_lines_keeps_the_whole_header_untruncated() {
        // The Ctrl+O transcript view shows the entire command — no `…)` cap —
        // however many rows it wraps to.
        let cmd = "for i in {1..5}; do echo \"=== Iteration $i ===\" \
                   && echo \"Current time: $(date)\" \
                   && echo \"System uptime: $(uptime)\" \
                   && echo \"Memory usage: $(free -h | grep Mem)\"; done";
        let lines = tool_full_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 50);
        let header: Vec<String> = lines
            .iter()
            .take_while(|l| !plain(l).contains('⎿'))
            .map(plain)
            .collect();
        assert!(
            header.len() > TOOL_HEADER_MAX_ROWS,
            "full view shows every header row: {header:?}"
        );
        let joined: String = header
            .iter()
            .map(|r| r.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !joined.contains('…'),
            "no ellipsis truncation in the full view: {joined:?}"
        );
        assert!(
            joined.trim_end().ends_with(')') && joined.contains("done"),
            "keeps the command tail and closing paren: {joined:?}"
        );
    }

    #[test]
    fn edit_tool_inline_peek_colours_the_diff_rows() {
        // An `edit` cell shows its diff inline (codex's trick): a `+` row is
        // green, a `-` row is red, the summary/context dim.
        let output = "Updated a.rs (+1 -1)\n keep\n-old\n+new";
        let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
        assert_eq!(
            plain(&lines[0]),
            "● Edit(a.rs)",
            "keeps the coloured header"
        );
        // Rows: header, summary, ` keep`, `-old`, `+new`.
        let del = lines.iter().find(|l| plain(l).contains("-old")).unwrap();
        let add = lines.iter().find(|l| plain(l).contains("+new")).unwrap();
        // The content span (after the gutter) carries the diff colour.
        assert_eq!(
            del.spans.last().unwrap().style.fg,
            Some(TOOL_DIFF_DEL_COLOR)
        );
        assert_eq!(
            add.spans.last().unwrap().style.fg,
            Some(TOOL_DIFF_ADD_COLOR)
        );
    }

    #[test]
    fn a_non_diff_tool_peek_is_not_diff_coloured() {
        // A `bash` cell whose output happens to start with `+`/`-` is NOT a diff
        // tool, so its rows render as plain (white) output, never green/red.
        let lines = tool_lines(
            &tool("Bash", "diff a b", ToolStatus::Ok, "-removed\n+added"),
            80,
        );
        let fg = lines[1].spans.last().unwrap().style.fg;
        assert_eq!(
            fg,
            Some(TOOL_OUTPUT_COLOR),
            "bash output is the plain white output colour"
        );
        assert_ne!(fg, Some(TOOL_DIFF_ADD_COLOR), "never diff-coloured");
        assert_ne!(fg, Some(TOOL_DIFF_DEL_COLOR), "never diff-coloured");
    }

    #[test]
    fn tool_output_content_is_white_the_corner_stays_dim() {
        // A finished tool's output under the ⎿ gutter is the noticeable white
        // output colour, while the ⎿ corner glyph itself stays a dim delimiter.
        let lines = tool_lines(&tool("Bash", "echo hi", ToolStatus::Ok, "hello world"), 80);
        let out = lines
            .iter()
            .find(|l| plain(l).contains("hello world"))
            .expect("an output row");
        let content = out
            .spans
            .iter()
            .find(|s| s.content.contains("hello"))
            .expect("the content span");
        assert_eq!(
            content.style.fg,
            Some(TOOL_OUTPUT_COLOR),
            "output content is the white output colour"
        );
        let corner = out
            .spans
            .iter()
            .find(|s| s.content.contains('⎿'))
            .expect("the ⎿ corner span");
        assert_eq!(
            corner.style.fg,
            Some(TOOL_DIM_COLOR),
            "the ⎿ corner stays a dim delimiter"
        );
    }

    #[test]
    fn tool_running_and_waiting_placeholders_stay_dim() {
        // The `Running…`/`Waiting…` placeholders are meta, not output, so they
        // keep the dim colour even though real output is now white.
        for status in [ToolStatus::Running, ToolStatus::Waiting] {
            let lines = tool_lines(&tool("Bash", "sleep 1", status, ""), 80);
            let row = lines
                .iter()
                .find(|l| {
                    let p = plain(l);
                    p.contains("Running…") || p.contains("Waiting…")
                })
                .expect("a placeholder row");
            let content = row.spans.last().unwrap();
            assert_eq!(
                content.style.fg,
                Some(TOOL_DIM_COLOR),
                "the {status:?} placeholder stays dim"
            );
        }
    }

    #[test]
    fn write_tool_full_view_colours_the_diff() {
        let output = "Updated a.rs (+1 -0)\n keep\n+added";
        let lines = tool_full_lines(&tool("Write", "a.rs", ToolStatus::Ok, output), 80);
        let add = lines.iter().find(|l| plain(l).contains("+added")).unwrap();
        assert_eq!(
            add.spans.last().unwrap().style.fg,
            Some(TOOL_DIFF_ADD_COLOR)
        );
    }

    #[test]
    fn edit_full_view_colours_wrapped_continuation_rows_by_their_source_line() {
        // A `+` line longer than the width wraps into several rows in the Ctrl+O
        // view. Every wrapped row of an added line must stay green (and a removed
        // line red) — colouring each display row by ITS OWN first char would
        // leave the marker-less continuation rows dim (or mis-colour them).
        let added = format!("+{}", "x".repeat(60));
        let removed = format!("-{}", "y".repeat(60));
        let output = format!("Updated a.rs (+1 -1)\n{added}\n{removed}");
        let width = 24; // body width ~20 → the 61-char lines wrap into several rows
        let lines = tool_full_lines(&tool("Edit", "a.rs", ToolStatus::Ok, &output), width);
        let content_fg = |l: &Line| l.spans.last().unwrap().style.fg;
        let add_rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('x')).collect();
        let del_rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('y')).collect();
        assert!(
            add_rows.len() > 1,
            "the added line wrapped into multiple rows"
        );
        assert!(
            del_rows.len() > 1,
            "the removed line wrapped into multiple rows"
        );
        for row in add_rows {
            assert_eq!(
                content_fg(row),
                Some(TOOL_DIFF_ADD_COLOR),
                "every wrapped +row is green"
            );
        }
        for row in del_rows {
            assert_eq!(
                content_fg(row),
                Some(TOOL_DIFF_DEL_COLOR),
                "every wrapped -row is red"
            );
        }
    }

    #[test]
    fn write_cell_shows_numbered_syntax_highlighted_rows() {
        // A `Created …` body (llm::tools::render_numbered_content) renders as
        // Claude-Code's Write preview: dim right-aligned line numbers, the
        // content syntax-highlighted by the path's extension.
        let output = "Created hello.py (2 lines)\n1 def main():\n2     x = \"hi\"";
        let lines = tool_lines(&tool("Write", "hello.py", ToolStatus::Ok, output), 80);
        assert_eq!(plain(&lines[0]), "● Write(hello.py)");
        assert_eq!(plain(&lines[1]), "  ⎿  Created hello.py (2 lines)");
        let row1 = &lines[2];
        assert_eq!(plain(row1), "      1 def main():");
        let num = &row1.spans[1];
        assert_eq!(num.content.as_ref(), "1 ");
        assert_eq!(num.style.fg, Some(TOOL_DIM_COLOR), "line number is dim");
        let kw = row1
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "def")
            .expect("keyword segment");
        assert!(kw.style.fg.is_some(), "keyword coloured");
        let s = lines[3]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "\"hi\"")
            .expect("string segment");
        assert!(s.style.fg.is_some(), "string coloured");
        assert_ne!(
            kw.style.fg, s.style.fg,
            "keyword and string are distinct colours"
        );
    }

    #[test]
    fn file_cell_uses_claude_code_gutter_spacing() {
        // Claude-Code's file-change look: the `⎿` corner is two spaces wide
        // (`  ⎿  Created…`, content at col 5), and the numbered body sits one
        // column further in so the gutter reads like Claude Code — for a
        // two-digit file line `1` lands under the corner word's 3rd letter and
        // its content under the 5th (number at col 7, content at col 9). The
        // `… +N lines` hint keeps the corner-content column (col 5).
        let body: String = (1..=12)
            .map(|i| format!("{i:>2} line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!("Created f.txt (12 lines)\n{body}");
        let lines = tool_lines(&tool("Write", "f.txt", ToolStatus::Ok, &output), 80);
        assert_eq!(plain(&lines[1]), "  ⎿  Created f.txt (12 lines)");
        assert_eq!(plain(&lines[2]), "       1 line 1");
        assert_eq!(
            plain(lines.last().unwrap()),
            "     … +2 lines (ctrl+o to expand)"
        );
    }

    #[test]
    fn bash_cell_output_aligns_under_the_two_space_corner() {
        // Claude-Code's bash cell: the corner is two spaces wide, so output
        // opens at `  ⎿  {line}` (col 5) and the `… +N lines` hint aligns under
        // it.
        let out = "l1\nl2\nl3\nl4\nl5\nl6";
        let lines = tool_lines(&tool("Bash", "seq 6", ToolStatus::Ok, out), 80);
        assert_eq!(plain(&lines[0]), "● Bash(seq 6)");
        assert_eq!(plain(&lines[1]), "  ⎿  l1");
        assert_eq!(
            plain(lines.last().unwrap()),
            "     … +2 lines (ctrl+o to expand)"
        );
    }

    #[test]
    fn read_cell_shows_numbered_syntax_highlighted_rows() {
        // A `read` cell renders like a `write`: the `Read N lines` summary on
        // the corner, then dim right-aligned line numbers with the content
        // syntax-highlighted by the path's extension — no diff sign or tint.
        let output = "1 def main():\n2     return 42";
        let lines = tool_lines(&tool("Read", "app.py", ToolStatus::Ok, output), 80);
        assert_eq!(plain(&lines[0]), "● Read(app.py)");
        assert_eq!(plain(&lines[1]), "  ⎿  Read 2 lines");
        let row = &lines[2];
        assert_eq!(plain(row), "      1 def main():");
        let num = &row.spans[1];
        assert_eq!(num.content.as_ref(), "1 ");
        assert_eq!(num.style.fg, Some(TOOL_DIM_COLOR), "line number is dim");
        let kw = row
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "def")
            .expect("keyword segment");
        assert!(kw.style.fg.is_some(), "keyword coloured");
        let lit = lines[3]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "42")
            .expect("number literal");
        assert!(lit.style.fg.is_some(), "number coloured");
        assert_ne!(
            kw.style.fg, lit.style.fg,
            "keyword and number are distinct colours"
        );
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .all(|s| s.style.bg.is_none()),
            "a read cell carries no diff background tint"
        );
    }

    #[test]
    fn read_cell_peek_caps_at_file_peek_lines_with_the_expand_hint() {
        let body: String = (1..=30)
            .map(|i| format!("{i:>2} row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = tool_lines(&tool("Read", "big.txt", ToolStatus::Ok, &body), 80);
        // header + summary + FILE_PEEK_LINES rows + the hint.
        assert_eq!(lines.len(), 2 + FILE_PEEK_LINES + 1);
        assert!(
            plain(lines.last().unwrap())
                .contains(&format!("+{} lines{EXPAND_HINT}", 30 - FILE_PEEK_LINES))
        );
    }

    #[test]
    fn read_cell_full_view_shows_every_row_uncapped() {
        let body: String = (1..=30)
            .map(|i| format!("{i:>2} row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = tool_full_lines(&tool("Read", "big.txt", ToolStatus::Ok, &body), 80);
        assert_eq!(lines.len(), 2 + 30, "header + summary + every row");
        assert!(plain(lines.last().unwrap()).contains("30 row 30"));
    }

    #[test]
    fn read_cell_placeholder_output_falls_back_to_a_plain_peek() {
        // The `(file is empty)` / offset-past-end placeholders aren't numbered,
        // so the cell keeps the plain output peek (no numbering, no crash).
        let lines = tool_lines(
            &tool("Read", "x.txt", ToolStatus::Ok, "(file x.txt is empty)"),
            80,
        );
        assert_eq!(plain(&lines[0]), "● Read(x.txt)");
        assert!(plain(&lines[1]).contains("(file x.txt is empty)"));
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .all(|s| s.style.bg.is_none())
        );
    }

    #[test]
    fn edit_cell_shows_numbered_hunks_with_diff_tints() {
        // An `Updated …` body (llm::tools::render_numbered_diff) renders as
        // codex's diff cell: numbered rows, `+` rows on the dark-green tint,
        // `-` rows dimmed on the dark-red tint, context syntax-highlighted.
        let output =
            "Updated a.rs (+1 -1)\n 9  before()\n10 -let x = 1;\n10 +let x = 2;\n11  after()";
        let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
        assert_eq!(plain(&lines[1]), "  ⎿  Updated a.rs (+1 -1)");
        let del = lines.iter().find(|l| plain(l).contains("-let")).unwrap();
        let add = lines.iter().find(|l| plain(l).contains("+let")).unwrap();
        let ctx = lines
            .iter()
            .find(|l| plain(l).contains("before()"))
            .unwrap();
        // The sign spans carry the diff colours…
        let del_sign = del
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "-")
            .unwrap();
        assert_eq!(del_sign.style.fg, Some(TOOL_DIFF_DEL_COLOR));
        let add_sign = add
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "+")
            .unwrap();
        assert_eq!(add_sign.style.fg, Some(TOOL_DIFF_ADD_COLOR));
        // …every span past the `⎿` indent sits on the row's background tint…
        assert!(
            del.spans
                .iter()
                .skip(1)
                .all(|s| s.style.bg == Some(TOOL_DIFF_DEL_BG)),
            "removed row is tinted red"
        );
        assert!(
            add.spans
                .iter()
                .skip(1)
                .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG)),
            "added row is tinted green"
        );
        // …the added text keeps its syntax colour, the removed text is dimmed,
        // and context rows are highlighted with no tint.
        let add_kw = add
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "let")
            .unwrap();
        assert!(
            add_kw.style.fg.is_some(),
            "added keyword keeps its syntax colour"
        );
        assert!(!add_kw.style.add_modifier.contains(Modifier::DIM));
        let del_kw = del
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "let")
            .unwrap();
        assert!(del_kw.style.add_modifier.contains(Modifier::DIM));
        let ctx_fn = ctx
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "before")
            .unwrap();
        assert!(
            ctx_fn.style.fg.is_some(),
            "context row is syntax-highlighted"
        );
        assert!(ctx.spans.iter().all(|s| s.style.bg.is_none()));
    }

    #[test]
    fn edit_cell_colours_the_summary_counts() {
        let output = "Updated a.rs (+6 -2)\n1 +x";
        let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
        let summary = &lines[1];
        let plus = summary
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "+6")
            .unwrap();
        assert_eq!(plus.style.fg, Some(TOOL_DIFF_ADD_COLOR));
        let minus = summary
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "-2")
            .unwrap();
        assert_eq!(minus.style.fg, Some(TOOL_DIFF_DEL_COLOR));
    }

    #[test]
    fn file_cell_summary_head_is_white_not_dim() {
        // The `Created …`/`Updated …`/`Read N lines` summary head reads in the
        // white output colour (noticeable) rather than dim grey — the `(+A -D)`
        // counts still show green/red.
        let write = tool_lines(
            &tool(
                "Write",
                "f.txt",
                ToolStatus::Ok,
                "Created f.txt (2 lines)\n1 a\n2 b",
            ),
            80,
        );
        let w_head = write[1]
            .spans
            .iter()
            .find(|s| s.content.contains("Created"))
            .expect("the Created summary span");
        assert_eq!(
            w_head.style.fg,
            Some(TOOL_OUTPUT_COLOR),
            "a write summary head is white"
        );

        let read = tool_lines(&tool("Read", "f.txt", ToolStatus::Ok, "1 a\n2 b"), 80);
        let r_head = read[1]
            .spans
            .iter()
            .find(|s| s.content.contains("Read"))
            .expect("the Read summary span");
        assert_eq!(
            r_head.style.fg,
            Some(TOOL_OUTPUT_COLOR),
            "a read summary head is white"
        );

        let edit = tool_lines(
            &tool(
                "Edit",
                "a.rs",
                ToolStatus::Ok,
                "Updated a.rs (+1 -1)\n1 +x\n2 -y",
            ),
            80,
        );
        let e_head = edit[1]
            .spans
            .iter()
            .find(|s| s.content.contains("Updated"))
            .expect("the Updated summary span");
        assert_eq!(
            e_head.style.fg,
            Some(TOOL_OUTPUT_COLOR),
            "an edit summary path is white"
        );
        // The counts still stand out green/red.
        let plus = edit[1].spans.iter().find(|s| s.content == "+1").unwrap();
        assert_eq!(
            plus.style.fg,
            Some(TOOL_DIFF_ADD_COLOR),
            "counts stay green"
        );
    }

    #[test]
    fn edit_cell_renders_the_hunk_gap_dim() {
        let output = "Updated a.rs (+2 -0)\n1 +first()\n  ⋮\n9 +second()";
        let lines = tool_full_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
        let gap = lines
            .iter()
            .find(|l| plain(l).trim_end().ends_with('⋮'))
            .expect("the ⋮ gap row is rendered");
        assert_eq!(gap.spans.last().unwrap().style.fg, Some(TOOL_DIM_COLOR));
    }

    #[test]
    fn write_cell_peek_caps_at_file_peek_lines_with_the_expand_hint() {
        let body: String = (1..=30)
            .map(|i| format!("{i:>2} line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!("Created big.txt (30 lines)\n{body}");
        let lines = tool_lines(&tool("Write", "big.txt", ToolStatus::Ok, &output), 80);
        // header + summary + FILE_PEEK_LINES numbered rows + the hint.
        assert_eq!(lines.len(), 2 + FILE_PEEK_LINES + 1);
        let hint = plain(lines.last().unwrap());
        assert!(
            hint.contains(&format!("+{} lines{EXPAND_HINT}", 30 - FILE_PEEK_LINES)),
            "got {hint}"
        );
    }

    #[test]
    fn write_cell_full_view_shows_every_numbered_row() {
        let body: String = (1..=30)
            .map(|i| format!("{i:>2} line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!("Created big.txt (30 lines)\n{body}");
        let lines = tool_full_lines(&tool("Write", "big.txt", ToolStatus::Ok, &output), 80);
        assert_eq!(lines.len(), 2 + 30, "header + summary + every row");
        assert!(plain(lines.last().unwrap()).contains("30 line 30"));
    }

    #[test]
    fn file_cell_wraps_long_rows_under_the_content_column() {
        // A long numbered row wraps (not truncates); continuations align under
        // the content column and keep the row's tint, and no row overflows.
        let output = format!("Updated a.rs (+1 -0)\n1 +{}", "x".repeat(60));
        let width = 30u16;
        let lines = tool_full_lines(&tool("Edit", "a.rs", ToolStatus::Ok, &output), width);
        let rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('x')).collect();
        assert!(rows.len() > 1, "the 60-char row wrapped");
        let cont = plain(rows[1]);
        // 6 (numbered indent) + cols("1 +") = 9 blank columns, then the content.
        assert!(cont.starts_with("         x"), "got {cont:?}");
        for r in &rows {
            assert!(
                r.spans
                    .iter()
                    .skip(1)
                    .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG)),
                "every wrapped row keeps the add tint"
            );
            assert!(cols(&plain(r)) <= width as usize);
        }
    }

    #[test]
    fn a_bash_cell_with_a_created_looking_output_gets_no_diff_tint() {
        // Only Write/Edit cells opt into the numbered rendering — a bash
        // command whose output mimics the format keeps the plain output peek
        // (white content, no diff background tint).
        let lines = tool_lines(
            &tool("Bash", "gen", ToolStatus::Ok, "Created x (1 line)\n1 hi"),
            80,
        );
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .all(|s| s.style.bg.is_none()),
            "no diff tint on a bash cell"
        );
    }

    #[test]
    fn tool_lines_wraps_a_peek_line_within_the_width_preserving_content() {
        // A peek line never overflows the terminal width (column-aware) — and,
        // when it's longer than the width, it **wraps** rather than clipping,
        // so no content is lost. A 50-col line at width 30 (25 content cols)
        // fits the row budget → two wrapped rows, no hint, every column kept.
        let long = "abcdefghij".repeat(5); // 50 cols
        let lines = tool_lines(&tool("Bash", "y", ToolStatus::Ok, &long), 30);
        for line in &lines {
            assert!(cols(&plain(line)) <= 30, "no line exceeds the width");
        }
        let body: Vec<String> = lines[1..].iter().map(plain).collect();
        assert_eq!(body.len(), 2, "the 50-col line wraps to two rows: {body:?}");
        let joined: String = body
            .iter()
            .map(|l| l.chars().skip(5).collect::<String>()) // drop the 5-col gutter
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(joined, long, "every column is preserved by the wrap");
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

    /// The transcript rows *after* the banner chrome (banner + spacer),
    /// trimmed — for tests asserting on the conversation walk itself (the
    /// banner atop it has its own tests).
    fn transcript_body(app: &App, width: u16) -> Vec<String> {
        let chrome = header_lines(app, width).len() + 1;
        transcript_lines(app, width)
            .iter()
            .skip(chrome)
            .map(|l| plain(l).trim_end().to_string())
            .collect()
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
    fn transcript_lines_list_the_queued_backlog_after_the_live_tail() {
        // The overlay shows the full live picture: entries still waiting in
        // the queue render after the live tail, styled exactly like the inline
        // strip's queued rows (two-space inset, ❯ user / red ! shell, a blank
        // dividing entries), so Ctrl+O never hides a queued message
        // (docs/queue.md).
        let mut app = App::new();
        app.record_user_message("q");
        app.begin_stream();
        app.push_chunk("partial");
        app.queued.push_back(batch(&["world"]));
        app.queued.push_back(QueuedTurn::Shell("ls".into()));
        let texts: Vec<String> = transcript_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        let pos = |needle: &str| {
            texts
                .iter()
                .position(|t| t.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle}: {texts:?}"))
        };
        assert!(
            pos("partial") < pos("❯ world"),
            "queued after the live tail"
        );
        assert!(pos("❯ world") < pos("! ls"), "entries in queue order");
        assert_eq!(
            texts[pos("❯ world")],
            format!("{QUEUED_INDENT}❯ world"),
            "the inline strip's two-space inset user style"
        );
        assert_eq!(
            texts[pos("❯ world") + 1].trim(),
            "",
            "a blank divides the entries"
        );
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
            vec!["  ⎿  /home/me", "     │   ├── a", "         indented   run",],
            "every output line verbatim under the corner"
        );
    }

    #[test]
    fn a_backend_tools_full_output_hangs_under_the_gutter_verbatim() {
        // The expanded (Ctrl+O) view opens a backend tool's output with the same
        // `⎿` gutter as its inline peek (and as a shell cell), continuation rows
        // aligned under the corner — Claude-Code's exec-cell style — with the
        // output's own space runs preserved (`ls -l` columns must survive).
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
            vec!["● Bash(ls -l)", "  ⎿  total 8", "     -rw-  1 user   42 a"],
            "the gutter opens the body, continuation rows aligned, space runs kept"
        );
    }

    #[test]
    fn transcript_lines_when_empty_is_a_placeholder() {
        // Even an empty transcript opens with the header banner (the overlay
        // mirrors the inline conversation — docs/header.md); the placeholder
        // sits under it, not lost.
        let app = App::new();
        let lines = transcript_lines(&app, 80);
        let chrome = header_lines(&app, 80).len() + 1; // banner + spacer
        assert!(
            plain(&lines[chrome]).to_lowercase().contains("nothing"),
            "{:?}",
            plain(&lines[chrome])
        );
    }

    #[test]
    fn transcript_opens_with_the_header_banner() {
        // The Ctrl+O overlay shows the same conversation the inline view
        // holds, and that conversation opens with the startup banner
        // (docs/header.md): the transcript's first rows are the banner plus a
        // blank spacer, then the history walk.
        let app = transcript_fixture();
        let lines = transcript_lines(&app, 80);
        let banner = header_lines(&app, 80);
        assert!(!banner.is_empty());
        let head: Vec<String> = lines.iter().take(banner.len()).map(plain).collect();
        let want: Vec<String> = banner.iter().map(plain).collect();
        assert_eq!(head, want, "the banner tops the transcript");
        assert_eq!(
            plain(&lines[banner.len()]).trim(),
            "",
            "a spacer divides the banner from the conversation"
        );
        let texts: Vec<String> = lines
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(texts.iter().any(|t| t == "❯ hello"), "{texts:?}");
    }

    #[test]
    fn transcript_cache_reuses_the_build_while_scrolling_but_refreshes_on_change() {
        // The Ctrl+O scroll-perf fix: repeated draws with the same content (a
        // scroll only moves the viewport) must reuse the cached build, while any
        // real change — new history, a width change — rebuilds. Correctness is
        // that the cache always equals a fresh `transcript_lines`.
        let mut app = transcript_fixture();
        let mut cache = TranscriptCache::new();

        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        assert_eq!(cache.builds, 1, "first access builds");

        // Repeated accesses with unchanged state (scrolling) are cache hits.
        cache.line_count(&app, 80);
        cache.lines(&app, 80);
        cache.selection(&app, 80);
        assert_eq!(cache.builds, 1, "scrolling is a cache hit, not a rebuild");

        // A new message changes the content → one rebuild, still correct.
        app.record_user_message("a brand new question");
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        assert_eq!(cache.builds, 2, "a history change rebuilds");

        // A width change (resize) also refreshes.
        assert_eq!(cache.lines(&app, 40), transcript_lines(&app, 40).as_slice());
        assert_eq!(cache.builds, 3, "a width change rebuilds");

        // clear() drops the cache so the next access rebuilds.
        cache.clear();
        cache.lines(&app, 40);
        assert_eq!(cache.builds, 1, "clear resets the build state");
    }

    #[test]
    fn transcript_cache_rebuilds_as_a_running_bash_streams_output() {
        // The Ctrl+O overlay must show a running `bash` tool's output LIVE, not a
        // frozen snapshot: as the tool streams (`App::push_tool_output`), the
        // cache signature has to notice the front call's output growing so the
        // overlay rebuilds — and, since it tail-follows, scrolls the new output
        // into view (docs/tool-streaming.md). Without the output length in the
        // signature the overlay would stay static: a single running call's queue
        // length and status don't change while it streams.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "make");
        let mut cache = TranscriptCache::new();
        let _ = cache.lines(&app, 80); // first build — the running cell, no output yet
        let builds = cache.builds;
        app.push_tool_output("compiling module_a\n");
        let lines = cache.lines(&app, 80).to_vec();
        assert!(
            cache.builds > builds,
            "the cache rebuilds when the running tool streams output"
        );
        assert!(
            lines
                .iter()
                .any(|l| plain(l).contains("compiling module_a")),
            "the overlay shows the newly streamed output: {:?}",
            lines.iter().map(plain).collect::<Vec<_>>()
        );
        // Each further chunk keeps it live and matches a fresh build.
        let builds = cache.builds;
        app.push_tool_output("compiling module_b\n");
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        assert!(
            cache.builds > builds,
            "each streamed chunk refreshes the overlay"
        );
    }

    #[test]
    fn transcript_cache_appends_new_items_without_rerendering_frozen_ones() {
        // The slow-Ctrl+O fix: the transcript build is O(history) with real
        // grammar highlighting (~hundreds of ms on a resumed session), so the
        // cache must render each committed item ONCE and only append — a new
        // item, a streamed chunk, or a reopen must never re-render the frozen
        // prefix. Correctness stays "equals a fresh full build".
        let mut app = transcript_fixture();
        let mut cache = TranscriptCache::new();
        let _ = cache.lines(&app, 80);
        let rendered = cache.item_renders;
        assert_eq!(
            rendered,
            app.history.len(),
            "the first build renders every item exactly once"
        );

        app.record_user_message("appended later");
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        assert_eq!(
            cache.item_renders,
            rendered + 1,
            "a committed item renders once; the frozen prefix is reused"
        );

        let _ = cache.lines(&app, 80);
        assert_eq!(cache.item_renders, rendered + 1, "a scroll renders nothing");
    }

    #[test]
    fn transcript_cache_streams_a_reply_without_rerendering_history() {
        // While a reply streams under the open overlay the signature changes
        // every chunk — the live tail must rebuild (the overlay follows the
        // stream) without paying the frozen prefix again.
        let mut app = transcript_fixture();
        let mut cache = TranscriptCache::new();
        let _ = cache.lines(&app, 80);
        let rendered = cache.item_renders;
        app.begin_stream();
        for chunk in ["stream", "ing 1", " and 2"] {
            app.push_chunk(chunk);
            assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        }
        assert_eq!(
            cache.item_renders, rendered,
            "streamed chunks re-render only the live tail, never the history"
        );
    }

    #[test]
    fn transcript_cache_backtrack_selection_restyles_without_rerendering() {
        // The Esc-Esc preview reverses the highlighted user message's rows —
        // rows in the *frozen* prefix. Stepping the selection must restyle in
        // place (and un-restyle exactly, byte-for-byte) without re-rendering.
        let mut app = transcript_fixture();
        app.record_user_message("second question");
        let mut cache = TranscriptCache::new();
        let _ = cache.lines(&app, 80);
        let rendered = cache.item_renders;

        for selected in [Some(1), Some(0), None, Some(1)] {
            app.backtrack.selected = selected;
            assert_eq!(
                cache.lines(&app, 80),
                transcript_lines(&app, 80).as_slice(),
                "selection {selected:?} matches a fresh build"
            );
            assert_eq!(
                cache.selection(&app, 80),
                transcript_selection(&app, 80),
                "selection range {selected:?} matches a fresh build"
            );
        }
        assert_eq!(
            cache.item_renders, rendered,
            "restyling the selection never re-renders items"
        );
    }

    #[test]
    fn transcript_cache_rebuilds_when_history_is_replaced_at_the_same_length() {
        // A pop + re-push (the interrupt-undo, then a new submission) can land
        // history back on the SAME length with different content — the length
        // signature alone would serve the stale build; the generation catches it.
        let mut app = App::new();
        app.record_user_message("first try");
        let mut cache = TranscriptCache::new();
        let _ = cache.lines(&app, 80);

        app.begin_stream();
        assert_eq!(
            app.interrupt_turn(),
            Some(crate::app::InterruptedTurn::Undone),
            "the no-output interrupt undoes the submission"
        );
        app.record_user_message("second try");
        let lines = cache.lines(&app, 80).to_vec();
        assert_eq!(lines, transcript_lines(&app, 80), "no stale frozen rows");
        let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        assert!(all.contains("second try"), "{all:?}");
        assert!(!all.contains("first try"), "{all:?}");
    }

    #[test]
    fn transcript_cache_warm_prebuilds_so_the_open_renders_nothing() {
        // The loop warms the cache at the boundary (after a /resume load, after
        // each commit) so pressing Ctrl+O finds every item already rendered —
        // the open then only assembles the live tail.
        let mut app = transcript_fixture();
        let mut cache = TranscriptCache::new();
        cache.warm(&app, 80);
        let rendered = cache.item_renders;
        assert_eq!(rendered, app.history.len(), "warm renders every item");
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
        assert_eq!(cache.item_renders, rendered, "the open renders no items");

        // A /resume swaps the whole history: the next warm rebuilds it.
        let loaded: Vec<HistoryItem> = transcript_fixture().history.clone();
        app.load_session(loaded);
        cache.warm(&app, 80);
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());

        // A width-only mismatch (a resize while the overlay is closed) defers:
        // warm must NOT burn a full rebuild per resize event…
        let rendered = cache.item_renders;
        cache.warm(&app, 40);
        assert_eq!(cache.item_renders, rendered, "warm skips a width change");
        // …the next real access (the overlay opening at the new width) pays it.
        assert_eq!(cache.lines(&app, 40), transcript_lines(&app, 40).as_slice());
    }

    #[test]
    fn render_tool_view_shows_the_title_messages_and_full_output() {
        let app = transcript_fixture();
        let mut buf = buffer(40, 24);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        let all: String = (0..24)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains(TOOL_VIEW_TITLE), "title present: {all:?}");
        assert!(
            all.contains(env!("CARGO_PKG_VERSION")),
            "the header banner opens the transcript: {all:?}"
        );
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
    fn render_tool_view_paints_the_codex_pager_chrome() {
        // The overlay is codex's Ctrl+T transcript pager: a slash-tiled dim
        // title row, the scrolling body, a `─` separator carrying the scroll
        // percentage right-aligned one dash in from the edge, two dim key-hint
        // rows, and a final blank row.
        let app = transcript_fixture();
        let mut buf = buffer(40, 24);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        let header = row(&buf, 0, 40);
        assert!(
            header.starts_with("/ T R A N S C R I P T / / "),
            "the title overlays the slash tiling: {header:?}"
        );
        // body rows 1..=19, then the separator at 24 - 4.
        let sep = row(&buf, 20, 40);
        assert!(sep.starts_with('─'), "{sep:?}");
        assert!(
            sep.contains(" 100% "),
            "everything fits → pinned at 100%: {sep:?}"
        );
        assert!(sep.ends_with('─'), "one dash right of the percent: {sep:?}");
        let hints = row(&buf, 21, 40);
        assert!(
            hints.contains("to scroll") && hints.contains("pgup/pgdn"),
            "{hints:?}"
        );
        assert!(
            row(&buf, 22, 40).contains("q/esc/ctrl+o to quit"),
            "{:?}",
            row(&buf, 22, 40)
        );
        assert_eq!(row(&buf, 23, 40).trim(), "", "a blank final row");
    }

    #[test]
    fn render_tool_view_fills_rows_below_the_content_with_tildes() {
        // Body rows past the transcript's end read `~` (codex's pager, vi-style).
        let mut app = App::new();
        app.record_user_message("hi");
        let mut buf = buffer(40, 24);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        // Content is the banner chrome + two lines (message + spacer) in a
        // 19-row body: the rows after it are filler.
        let chrome = header_lines(&app, 40).len() + 1;
        let msg_row = 1 + chrome as u16;
        assert!(
            row(&buf, msg_row, 40).contains("❯ hi"),
            "{:?}",
            row(&buf, msg_row, 40)
        );
        for y in (msg_row + 2)..=19 {
            assert_eq!(row(&buf, y, 40).trim_end(), "~", "row {y} is filler");
        }
    }

    #[test]
    fn render_tool_view_separator_tracks_the_scroll_position() {
        // 0% at the top, 100% at the bottom — codex's pager percentage.
        let mut app = App::new();
        app.start_tool("Read", "f");
        let output = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.end_tool(&output, true);
        let mut buf = buffer(40, 12);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        assert!(row(&buf, 8, 40).contains(" 0% "), "{:?}", row(&buf, 8, 40));

        app.tool_scroll = usize::MAX; // pinned to the bottom (clamped)
        let mut buf = buffer(40, 12);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        assert!(
            row(&buf, 8, 40).contains(" 100% "),
            "{:?}",
            row(&buf, 8, 40)
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
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
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
        let body = (screen_h - TOOL_VIEW_TITLE_ROWS - TOOL_VIEW_FOOTER_ROWS) as usize;
        assert_eq!(
            tool_view_max_scroll(&app, 40, screen_h),
            total.saturating_sub(body)
        );
    }

    // ===== Ctrl+D context-debug view (docs/context.md) =====

    /// A conversation with a system prompt, a user turn, a raw tool record,
    /// and a summary — everything the context window derives from.
    fn context_fixture() -> App {
        let mut app = transcript_fixture();
        app.set_system_prompt(Some("be nice".to_string()));
        app.end_turn(2);
        app
    }

    #[test]
    fn context_lines_show_the_raw_window_with_role_tags() {
        let app = context_fixture();
        let texts: Vec<String> = context_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        // The system prompt leads, tagged apart from mid-conversation notes.
        assert_eq!(texts[0], "system prompt:", "{texts:?}");
        assert_eq!(texts[1], "  be nice", "{texts:?}");
        assert!(texts.iter().any(|t| t == "user:"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "  hello"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "assistant:"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "  let me check"), "{texts:?}");
        // The tool call appears as a native `→ name(arguments)` request under
        // the assistant — the raw wire form, not the old `[tool …]` bracket and
        // not the TUI's bullet rendering.
        assert!(
            texts.iter().any(|t| t == r#"  → read({"path":"f"})"#),
            "{texts:?}"
        );
        assert!(!texts.iter().any(|t| t.contains("[tool")), "{texts:?}");
        // The result rides its own `tool:` role entry, in full.
        assert!(texts.iter().any(|t| t == "tool:"), "{texts:?}");
        for needle in ["  L1", "  L2", "  L3"] {
            assert!(texts.iter().any(|t| t == needle), "{texts:?}");
        }
        // Turn summaries are TUI chrome; they never reach the context.
        assert!(!texts.iter().any(|t| t.contains("Done")), "{texts:?}");
    }

    #[test]
    fn context_lines_show_the_user_instructions_first() {
        // The AGENTS.md instructions fragment (docs/project-doc.md) is the
        // first user entry of the window — right after the system prompt, in
        // front of the conversation, exactly what the request carries.
        let mut app = context_fixture();
        app.set_user_instructions(Some("# AGENTS.md instructions\n\nguide".to_string()));
        let texts: Vec<String> = context_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts[0], "system prompt:", "{texts:?}");
        let first_user = texts.iter().position(|t| t == "user:").unwrap();
        assert_eq!(
            texts[first_user + 1],
            "  # AGENTS.md instructions",
            "{texts:?}"
        );
        assert_eq!(texts[first_user + 3], "  guide", "{texts:?}");
    }

    #[test]
    fn context_lines_list_image_attachments_under_their_message() {
        let mut app = App::new();
        app.record_user_message_with_images(
            "[Image #1] what is this?",
            vec![std::path::PathBuf::from("/tmp/shot.png")],
        );
        let texts: Vec<String> = context_lines(&app, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(
            texts.iter().any(|t| t == "  [Image #1] what is this?"),
            "the placeholder stays raw in the text: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "  image: /tmp/shot.png"),
            "the attachment path lists beneath: {texts:?}"
        );
    }

    #[test]
    fn context_lines_wrap_a_long_image_path_instead_of_clipping() {
        let mut app = App::new();
        app.record_user_message_with_images(
            "[Image #1]",
            vec![std::path::PathBuf::from(
                "/tmp/a-very-long-temp-directory-name/alter-zero-clipboard-0123456789.png",
            )],
        );
        let texts: Vec<String> = context_lines(&app, 30)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert!(
            texts.iter().any(|t| t.starts_with("  image: /tmp")),
            "the label row starts the path: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.ends_with(".png") && !t.contains("image:")),
            "the path's tail wraps onto a continuation row: {texts:?}"
        );
        assert!(
            texts.iter().all(|t| cols(t) <= 30),
            "no row exceeds the width: {texts:?}"
        );
    }

    #[test]
    fn an_empty_context_shows_the_placeholder() {
        let lines = context_lines(&App::new(), 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(plain(&lines[0]), CONTEXT_VIEW_EMPTY);
    }

    #[test]
    fn render_context_view_paints_the_pager_chrome_with_its_own_title_and_keys() {
        let app = context_fixture();
        let mut buf = buffer(50, 14);
        render_context_view(buf.area, &mut buf, &app);
        let header = row(&buf, 0, 50);
        assert!(
            header.starts_with("/ C O N T E X T / "),
            "the title overlays the slash tiling: {header:?}"
        );
        let sep = row(&buf, 10, 50);
        assert!(sep.starts_with('─'), "{sep:?}");
        assert!(sep.contains('%'), "the scroll percentage rides it: {sep:?}");
        assert!(
            row(&buf, 11, 50).contains("to scroll"),
            "{:?}",
            row(&buf, 11, 50)
        );
        assert!(
            row(&buf, 12, 50).contains("q/esc/ctrl+d to quit"),
            "{:?}",
            row(&buf, 12, 50)
        );
        assert_eq!(row(&buf, 13, 50).trim(), "", "a blank final row");
    }

    #[test]
    fn render_context_view_fills_rows_below_the_content_with_tildes() {
        let mut app = App::new();
        app.record_user_message("hi");
        let mut buf = buffer(30, 16);
        render_context_view(buf.area, &mut buf, &app);
        assert!(
            row(&buf, 9, 30).starts_with('~'),
            "vi-style filler past the end: {:?}",
            row(&buf, 9, 30)
        );
    }

    #[test]
    fn context_view_max_scroll_is_total_lines_minus_the_body() {
        let app = context_fixture();
        let total = context_lines(&app, 40).len();
        let screen_h = 10u16;
        let body = (screen_h - TOOL_VIEW_TITLE_ROWS - TOOL_VIEW_FOOTER_ROWS) as usize;
        assert_eq!(
            context_view_max_scroll(&app, 40, screen_h),
            total.saturating_sub(body)
        );
    }

    #[test]
    fn render_context_view_windows_by_the_debug_scroll() {
        let mut app = context_fixture();
        app.debug_scroll = 2; // past "system prompt:" and "  be nice"
        let mut buf = buffer(50, 14);
        render_context_view(buf.area, &mut buf, &app);
        assert!(
            !row(&buf, 1, 50).contains("system prompt"),
            "the scrolled-off tag is gone: {:?}",
            row(&buf, 1, 50)
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
                images: Vec::new(),
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
                images: Vec::new(),
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
                images: Vec::new(),
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
                shells: 0,
                tokens: 0,
                cached: 0,
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
        assert_eq!(transcript_body(&app, 60), vec!["❯ hi", "", "● hello", ""]);
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
            retry: None,
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
    fn format_elapsed_is_bare_seconds_under_a_minute() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(5), "5s");
        assert_eq!(format_elapsed(59), "59s");
    }

    #[test]
    fn format_elapsed_combines_minutes_and_seconds() {
        // The live seconds keep ticking within the minute, so a "working…"
        // timer never looks frozen.
        assert_eq!(format_elapsed(60), "1m 0s");
        assert_eq!(format_elapsed(90), "1m 30s");
        assert_eq!(format_elapsed(600), "10m 0s");
        assert_eq!(format_elapsed(3_599), "59m 59s");
    }

    #[test]
    fn format_elapsed_combines_hours_and_minutes_past_an_hour() {
        assert_eq!(format_elapsed(3_600), "1h 0m");
        assert_eq!(format_elapsed(3_661), "1h 1m");
        assert_eq!(format_elapsed(7_200), "2h 0m");
        assert_eq!(format_elapsed(7_380), "2h 3m");
        assert_eq!(format_elapsed(90_000), "25h 0m");
    }

    #[test]
    fn status_line_humanizes_a_long_elapsed_into_minutes_and_seconds() {
        let text = plain(&status_line(&status(100, TokenArrow::Down, 90, None)));
        assert!(
            text.contains("(1m 30s · ↓ 100 tokens"),
            "the elapsed reads m/s past a minute: {text:?}"
        );
        assert!(
            !text.contains("(90s"),
            "no bare-seconds form past a minute: {text:?}"
        );
    }

    #[test]
    fn status_line_humanizes_the_thinking_clause_too() {
        // elapsed 200s → 3m 20s, thinking 75s → 1m 15s.
        let text = plain(&status_line(&status(150, TokenArrow::Down, 200, Some(75))));
        assert!(text.contains("(3m 20s "), "elapsed humanized: {text:?}");
        assert!(
            text.contains("Thinking for 1m 15s"),
            "thinking clause humanized: {text:?}"
        );
    }

    #[test]
    fn summary_humanizes_a_long_turn() {
        let summary = TurnSummary {
            verb: "Done",
            secs: 3_661,
            timestamp: String::new(),
            shells: 0,
            tokens: 0,
            cached: 0,
        };
        assert_eq!(plain(&summary_lines(&summary, 80)[0]), "Done for 1h 1m");
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
    fn format_token_count_humanizes_thousands_and_millions() {
        // Real usage tallies run to six digits (the whole context re-billed
        // per agent round) — raw ints stop reading in a one-line status.
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1k");
        assert_eq!(format_token_count(8_063), "8.1k");
        assert_eq!(format_token_count(15_049), "15k");
        assert_eq!(format_token_count(154_302), "154.3k");
        assert_eq!(format_token_count(2_000_000), "2M");
        assert_eq!(format_token_count(1_234_567), "1.2M");
    }

    #[test]
    fn status_line_humanizes_a_large_token_tally() {
        // Once the real usage snaps the tally past a thousand, the status
        // shows the compact form (the small-estimate case stays bare).
        let text = plain(&status_line(&status(8_063, TokenArrow::Down, 1, None)));
        assert!(
            text.contains("↓ 8.1k tokens"),
            "the tally reads compact: {text:?}"
        );
    }

    #[test]
    fn summary_lines_append_the_real_token_usage() {
        // A turn whose backend reported usage commits it with the summary —
        // the cached share beside it as the visible proof caching worked.
        let mut summary = TurnSummary {
            verb: "Done",
            secs: 12,
            timestamp: String::new(),
            shells: 0,
            tokens: 8_203,
            cached: 8_063,
        };
        assert_eq!(
            plain(&summary_lines(&summary, 80)[0]),
            "Done for 12s · 8.2k tokens (8.1k cached)"
        );
        summary.cached = 0;
        assert_eq!(
            plain(&summary_lines(&summary, 80)[0]),
            "Done for 12s · 8.2k tokens",
            "no parenthetical when nothing was cached"
        );
        summary.tokens = 0;
        assert_eq!(
            plain(&summary_lines(&summary, 80)[0]),
            "Done for 12s",
            "a usage-less turn (the dummy) keeps the bare summary"
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

    /// A live status carrying a retry indicator (verb fixed to "Working").
    fn status_retrying(attempt: u32, max: u32, tokens: usize) -> TurnStatus {
        let mut s = status(tokens, TokenArrow::Up, 5, None);
        s.retry = Some(RetryInfo { attempt, max });
        s
    }

    #[test]
    fn status_line_shows_the_retry_count_between_tokens_and_the_hint() {
        let text = plain(&status_line(&status_retrying(2, 3, 42)));
        assert!(
            text.ends_with("Working… (5s · ↑ 42 tokens · retrying 2/3 · esc to interrupt)"),
            "the retry clause sits after the tokens and before the hint: {text:?}"
        );
    }

    #[test]
    fn status_line_shows_the_retry_count_even_with_no_tokens_yet() {
        // A connection that fails before the first byte has only the input
        // counted; the clause still shows.
        let text = plain(&status_line(&status_retrying(1, 3, 0)));
        assert!(text.contains("· retrying 1/3 ·"), "{text:?}");
    }

    #[test]
    fn status_line_has_no_retry_clause_when_not_retrying() {
        let text = plain(&status_line(&status(42, TokenArrow::Down, 5, None)));
        assert!(!text.contains("retrying"), "{text:?}");
    }

    #[test]
    fn the_retry_clause_stands_out_in_its_own_colour() {
        let line = status_line(&status_retrying(1, 3, 0));
        let retry = line
            .spans
            .iter()
            .find(|s| s.content.contains("retrying"))
            .expect("a span carrying the retry clause");
        assert_eq!(
            retry.style.fg,
            Some(STATUS_RETRY_COLOR),
            "the retry clause uses the warning colour, not the dim metric grey"
        );
        assert_ne!(
            STATUS_RETRY_COLOR, STATUS_DETAIL_COLOR,
            "the retry colour is distinct from the dim metrics"
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
            shells: 0,
            tokens: 0,
            cached: 0,
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
                shells: 0,
                tokens: 0,
                cached: 0,
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
        let h = live_height(&app.input, 40, 24, true, 1, 0, 0, 0, 0, 0);
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
        assert_eq!(preview_rows(&app, 60), 0);
        let h = live_height(&app.input, 60, 24, true, 0, 0, 0, 0, 0, 0);
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
        let h = live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 0, 0);
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
            live_height(&app.input, 20, term_h, false, 0, 0, 0, 0, 0, 0),
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
    fn render_live_shell_run_shows_running_elapsed_and_hides_the_status_line() {
        // A `!` shell run suppresses the spinner status line entirely and shows
        // its elapsed in the `⎿ Running… (Ns)` preview (req 3): the strip is
        // the preview cell — the running row + the live-only Ctrl+B hint
        // (docs/background.md) — plus its gap, then the box's top rule — no
        // status row, no `esc to interrupt` hint. See docs/shell-command.md.
        let mut app = App::new();
        app.begin_shell("sleep 30");
        app.set_status_times(Duration::from_secs(3), None);
        // Past the hint delay so the Ctrl+B hint row shows (docs/background.md).
        app.set_command_elapsed(Some(Duration::from_secs(3)));
        let mut buf = buffer(40, 6); // preview (2) + gap + (two rules + one input)
        render_live(buf.area, &mut buf, &app);

        assert_eq!(
            row(&buf, 0, 40).trim_end(),
            "  ⎿  Running… (3s)",
            "the running preview carries the elapsed the status line would have"
        );
        assert_eq!(
            row(&buf, 1, 40).trim(),
            "(ctrl+b to run in background)",
            "the live cell hints the Ctrl+B background handoff"
        );
        assert!(
            row(&buf, 2, 40).trim().is_empty(),
            "blank gap row below the preview"
        );
        assert_eq!(
            buf[(0, 3)].symbol(),
            "─",
            "the box's top rule sits right under the preview gap — no status line between"
        );
        let all: String = (0..6).map(|y| row(&buf, y, 40)).collect();
        assert!(
            !all.contains("esc to interrupt"),
            "a shell run shows no status line (and so no interrupt hint): {all:?}"
        );
        assert_eq!(
            all.matches("Running…").count(),
            1,
            "`Running…` appears only in the preview, not also in a status line"
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
            live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 0, 0),
        );
        assert_eq!(cursor_position(area, &app), (4, 2));
    }

    // --- streaming commit bookkeeping ---

    /// Adversarial differential test: for a corpus of tricky replies at several
    /// widths, drive [`StreamRender`] over **every character-prefix** and assert
    /// that (a) the streamed commits + `finish` reconstruct the batch
    /// [`message_lines`] render exactly (text, colour, **and modifiers** —
    /// heading levels differ only by bold/italic, so an fg-only comparison is
    /// blind to a level flip), (b) no committed row ever changes, and (c)
    /// `preview(prefix)` equals the last row of the batch render of that
    /// prefix. This is the guard against immutable-scrollback corruption — the
    /// incremental renderer must never diverge from the batch one.
    #[test]
    fn stream_render_matches_batch_render_on_every_prefix() {
        let styled = |l: &Line| -> Vec<(String, Option<Color>, Modifier)> {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), s.style.fg, s.style.add_modifier))
                .collect()
        };
        let corpus = [
            // Prose that wraps several times.
            "the quick brown fox jumps over the lazy dog and keeps on running along",
            // Prose, a fenced code block, then prose.
            "Intro line here.\n```python\ndef f(x):\n    return x + 1\n```\nOutro line.",
            // A long code line whose call `(` lands past a wrap boundary — the
            // recolour trap (identifier turns blue only when the `(` arrives).
            "```rust\nfn some_really_long_function_name_that_wraps(argument: i32) -> i32 {\n    argument\n}\n```",
            // Python triple-quoted multi-line string (highlight carry across lines).
            "```python\ndoc = \"\"\"first\nsecond line\nthird\"\"\"\nx = f(1)\n```",
            // C block comment carried across lines.
            "```c\nint a; /* open\nstill comment\nclose */ int b;\n```",
            // Rust lifetimes (a `'a` must not open a string) + a char literal.
            "```rust\nimpl<'a> Foo<'a> {\n    let c = 'x';\n}\n```",
            // Tilde fence, indented fence, blank lines inside code.
            "~~~\nplain code\n\nmore\n~~~",
            "   ```\nindented fence body\n   ```",
            // ATX headings interleaved with prose and a bare code block.
            "# Title\nsome text under it\n## Sub heading here that is quite long and wraps\n```\ncode\n```",
            // Reply that is exactly a code block; unterminated fence at the end.
            "```go\npackage main\nfunc main() {}",
            // TAB-indented code (Go): tabs expand to spaces on the render path, so
            // the streamed commits must still match the batch render at every
            // prefix (the expansion is a pure per-line transform, prefix-stable).
            "```go\nfunc main() {\n\tif x {\n\t\tfmt.Println(\"hi\")\n\t}\n}\n```",
            // Consecutive fences (open immediately closed) and empty prose lines.
            "a\n\n```\n```\n\nb",
            // Multi-byte UTF-8: emoji + CJK in prose and inside a string, so the
            // per-prefix char-boundary handling is exercised.
            "greeting 🎮 hello 世界 more text to wrap around\n```python\nprint(\"🎮 世界!\")\n```\ndone 🚀",
            // Indented (4-space) code block: after a blank it renders verbatim
            // plain; the committed rows must stay stable as it streams in.
            "intro line\n\n    def f(x):\n        return x + 1\nback to prose",
            // A long indented-code line that hard-breaks at narrow widths, plus a
            // blank line inside the block (kept as code), then prose ends it.
            "note\n\n    a_really_long_indented_code_line_that_wraps_several_times = 42\n\n    tail\ndone",
            // Thematic breaks (`---` after a blank, and `***`) render as `———`.
            "one\n\n---\n\ntwo",
            "a\n\n***\nb",
            // Indented code followed by a thematic break and more prose.
            "lead\n\n    code_here()\n\n---\n\ntrailer",
            // A deep heading: while the trailing line is a bare `#` run its
            // LEVEL (→ style) is unsettled — another `#` deepens it, a 7th
            // flips it to prose — so at content-width 1 its wrapped rows must
            // be withheld (`markdown::is_partial_heading`), like a fence's.
            "lead\n###### deep heading level six",
            // The 7-hash flip: `#######` is prose, not a heading.
            "a\n####### not a heading",
            // Trailing paragraph break (a model's `…\n\n` before a tool call):
            // the trailing blank rows must be trimmed at every prefix, and a
            // committed row must never regress when they are.
            "building the thing now.\n\n",
            "first paragraph.\n\nsecond paragraph.\n\n",
            // A blank line *inside* a still-open fence at the end is content, not
            // a trailing blank — it must survive (the `!in_code` trim gate).
            "intro\n```\ncode\n\n",
            // --- GFM tables (docs/markdown.md): a table is buffered whole and
            // committed only when the block closes, so the streamed commits must
            // still match the batch render at every prefix and width (incl. the
            // column shrink at tiny widths). ---
            // A basic table between prose.
            "intro\n\n| Name | Type | Notes |\n|------|------|-------|\n| Alpha | String | Example row |\n| Beta | Number | Another row |\n\nafter",
            // A table the reply ends on (no closing line) — `finish` flushes it.
            "here is data:\n| a | b |\n|:--|--:|\n| 1 | 2 |",
            // Per-column alignment (left/center/right), then prose closes it.
            "| L | C | R |\n| :-- | :-: | --: |\n| x | yy | zzz |\ntail",
            // A pipe-carrying prose line that is NOT a table (no delimiter follows):
            // rendered as plain prose, buffered one line then flushed.
            "use a | b pipe here\nnext line of prose",
            // A candidate header whose delimiter column count mismatches → all prose.
            "| a | b | c |\n|---|---|\nnot a table",
            // A table immediately followed by a code fence (flush on CodeStart).
            "| a | b |\n|---|---|\n| 1 | 2 |\n```\ncode\n```",
            // A single-column table, then prose.
            "| Item |\n|------|\n| one |\n| two |\ndone",
            // A table with long cells that must WRAP into taller rows at the
            // narrow widths (img2): the progressive streamed commits + the final
            // flush must still equal the batch render at every prefix/width, so
            // the row-by-row streaming and the cell wrapping stay prefix-stable.
            "data:\n\n| Name | Email | Role |\n|------|-------|------|\n| John Doe | john.doe@example.com | Developer |\n| Jane Smith | jane@corp.io | Manager |\n\nend",
            // A table whose cells carry inline markdown (`` `code` ``, **bold**):
            // cells are inline-parsed like prose and the column sizes to the
            // rendered width, so the withheld-whole grid's streamed commits must
            // still match batch at every prefix/width (incl. the narrow shrink).
            "files:\n\n| Database | Modified |\n|----------|----------|\n| `core.db` | **Jul 13** |\n| plain.db | today |\n\nend",
            // A table whose cells are long enough that at the narrower sweep widths
            // the grid is too cramped to scan and flips to codex-style key/value
            // RECORDS (docs/table-streaming.md): the block (a `─` rule before each
            // non-first record) is buffered and rendered whole like the grid, so
            // the streamed commits + the final flush must still equal the batch
            // render at every prefix/width.
            "summary:\n\n| Component | Description of the thing |\n|-----------|--------------------------|\n| Parser | Reads and validates the input tokens |\n| Renderer | Draws styled cells into the terminal |\n\ndone",
            // HARD-WRAPPED rows (the model echoing terminal-wrapped source): the
            // pipe-carrying fragments (`96.4 ms |`, `15.0 ms |`) don't start
            // with `|` and re-join their rows (docs/table-streaming.md) — the
            // streamed commits + preview must match the batch render at every
            // prefix while the join forms (incl. mid-fragment prefixes, where
            // the pipe hasn't arrived yet and the tail still reads as prose).
            "pings:\n\n| Host | Loss | RTT |\n|------|------|-----|\n| google | 0% | 86.8 /\n96.4 ms |\n| fb | 0% |\n15.0 ms |\n\ndone",
            // --- Inline emphasis (docs/markdown.md): line-local, so a complete
            // line's styling is final (its frozen rows never restyle) while a
            // trailing line with an open marker is withheld (has_open_inline). The
            // streamed commits must match the batch render at every prefix/width,
            // including the marker-reveal flip and mid-word style changes. ---
            "first line **bold** here\nsecond *italic* line\nthird `code` end",
            // Emphasis closes early, then a long plain tail streams per row.
            "this **bold** part settles then a long plain tail that wraps across several rows here",
            // A bold phrase that itself wraps across rows (narrow widths hard-break it).
            "**wide bold phrase that wraps across multiple rows** then plain tail here",
            // Nested emphasis, a code span, a link and an image across a wrap.
            "nested **bold with _italic_ inside** and a `code span`\nsee [the docs](https://example.com/x) or ![pic](https://img/y.png) inline",
            // Non-emphasis markers stay literal: spaced `*`, snake_case `_`.
            "compute 2 * 3 and read foo_bar_baz then stop\ndone",
            // A strikethrough and inline code together, then a closing paragraph.
            "~~removed~~ and `kept` values differ\n\nsummary paragraph after a blank",
            // --- Lists & blockquotes (docs/markdown.md): line-local, so frozen
            // rows never restyle; the streamed commits must match batch at every
            // prefix/width, incl. the `-`-vs-partial-thematic-break handoff, the
            // hanging-indent wraps, and list text with inline emphasis. ---
            "- first item\n- second item\n  - nested item\ndone",
            "1. one\n2. two\n10. ten\ntail",
            "> a quoted line\n> continued quote\n\nafter the quote",
            "- a bullet with **bold** and `code` that wraps over rows\n- next item",
            "intro\n\n- [x] done task\n- [ ] pending task\n\nafter",
            "* star bullet\n+ plus bullet\ndone",
        ];

        for full in corpus {
            // Include pathologically narrow widths (content_width 1–2) where a
            // partial fence marker wraps into ≥2 rows — the invariant must hold
            // there too (`markdown::is_partial_fence`), not just at usable widths.
            for width in [3u16, 4, 5, 10, 16, 24, 40] {
                let expected: Vec<Vec<(String, Option<Color>, Modifier)>> =
                    message_lines(Role::Assistant, full, width)
                        .iter()
                        .map(styled)
                        .collect();

                let mut render = StreamRender::new();
                let mut committed: Vec<Vec<(String, Option<Color>, Modifier)>> = Vec::new();
                for end in 1..=full.len() {
                    if !full.is_char_boundary(end) {
                        continue;
                    }
                    let prefix = &full[..end];
                    // Real streaming order: `commit` runs on the chunk's arrival,
                    // the draw's `preview` after it.
                    // (a)/(b) commit rows extend a stable prefix of the final render.
                    committed.extend(render.commit(prefix, width).iter().map(styled));
                    assert_eq!(
                        committed[..],
                        expected[..committed.len()],
                        "a committed row diverged while streaming {full:?} (w={width})"
                    );
                    // (c) the preview is a SUFFIX of the batch render of this
                    // prefix — its last row outside a table, the whole
                    // uncommitted tail (the forming block) while one is open
                    // (docs/table-streaming.md).
                    let batch_prefix: Vec<Vec<(String, Option<Color>, Modifier)>> =
                        message_lines(Role::Assistant, prefix, width)
                            .iter()
                            .map(styled)
                            .collect();
                    let got_preview: Vec<Vec<(String, Option<Color>, Modifier)>> = render
                        .preview(prefix, width, usize::MAX)
                        .iter()
                        .map(styled)
                        .collect();
                    assert!(
                        got_preview.len() <= batch_prefix.len()
                            && got_preview[..]
                                == batch_prefix[batch_prefix.len() - got_preview.len()..],
                        "preview must be a suffix of the batch render at {prefix:?} (w={width}):\n got {got_preview:?}\nwant a tail of {batch_prefix:?}"
                    );
                    // (d) while a table is open, scrollback + strip together show
                    // the WHOLE render — the table streams visibly even though
                    // none of it has committed to scrollback yet.
                    if ends_in_open_table(prefix) {
                        assert_eq!(
                            committed.len() + got_preview.len(),
                            batch_prefix.len(),
                            "open table: committed + preview must span the whole render at {prefix:?} (w={width})"
                        );
                    }
                }
                committed.extend(render.finish(full, width).iter().map(styled));
                assert_eq!(committed, expected, "reconstruct {full:?} (w={width})");
            }
        }
    }

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

        let mut render = StreamRender::new();
        let mut got: Vec<String> = Vec::new();
        let mut acc = String::new();
        for chunk in crate::stream::chunks(full) {
            acc.push_str(&chunk);
            got.extend(render.commit(&acc, width).iter().map(plain));
        }
        got.extend(render.finish(&acc, width).iter().map(plain));

        assert_eq!(got, expected);
    }

    /// Exhaustively drive [`StreamRender`] over **every prefix** of a reply
    /// (char-by-char) and prove two invariants that keep immutable scrollback
    /// sound (CLAUDE.md invariant 2): a row, once committed, is **never**
    /// re-emitted or changed, and committed rows + the final flush reconstruct
    /// the whole rendered reply exactly. Runs a prose reply and a fenced-code
    /// reply (the highlight-carry case).
    #[test]
    fn stream_render_is_prefix_stable_over_every_prefix() {
        // Compare full styled rows (text + fg colour of each span), so the scan
        // catches a recolour — e.g. a code identifier turning blue at its call
        // `(` after an earlier wrapped row was committed — not just a text change.
        let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), s.style.fg))
                .collect()
        };
        for full in [
            "a short prose reply that wraps a few times across the width here",
            "intro line\n```python\ndef long_function_name_here(x):\n    s = \"\"\"multi\n    line\"\"\"\n    return s\n```\nend",
        ] {
            let width = 18;
            let expected: Vec<Vec<(String, Option<Color>)>> =
                message_lines(Role::Assistant, full, width)
                    .iter()
                    .map(styled)
                    .collect();

            let mut render = StreamRender::new();
            let mut committed: Vec<Vec<(String, Option<Color>)>> = Vec::new();
            for end in 1..full.len() {
                if !full.is_char_boundary(end) {
                    continue;
                }
                committed.extend(render.commit(&full[..end], width).iter().map(styled));
                // Everything committed so far must be a stable prefix of the
                // final render — never a row that later changes text or colour.
                assert_eq!(
                    committed[..],
                    expected[..committed.len()],
                    "a committed row changed while streaming {full:?}"
                );
            }
            committed.extend(render.finish(full, width).iter().map(styled));
            assert_eq!(committed, expected, "reconstruct {full:?}");
        }
    }

    #[test]
    fn stream_render_withholds_the_last_line() {
        // "hi there" fits one line → nothing is stable yet.
        let mut render = StreamRender::new();
        assert!(render.commit("hi there", 80).is_empty());
    }

    #[test]
    fn preview_never_shows_a_committed_row_while_streaming() {
        // Regression for the slow-stream duplicate-line bug: a chunk ending in a
        // newline completes a line, which `commit` must not flush to scrollback
        // while the strip still previews it — otherwise the line shows twice (once
        // committed, once previewed) until the next chunk arrives. Drive real
        // streaming order (commit before the draw's preview) over every prefix and
        // assert the preview is never a row already committed.
        let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), s.style.fg))
                .collect()
        };
        let width = 24;
        for full in [
            "first line here\nsecond line here\n\nthird paragraph line",
            "- bullet one\n- bullet two\n- bullet three\n",
            "some words that wrap a little here\n\nnext paragraph body\n",
            "## Heading Row\n\nbody text below it\n",
            // A streaming table: its rows commit progressively, so the preview
            // (the last streamed content row) must never duplicate a committed
            // one (docs/table-streaming.md).
            "here:\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\ndone\n",
        ] {
            let mut render = StreamRender::new();
            let mut committed: Vec<Vec<(String, Option<Color>)>> = Vec::new();
            for end in 1..=full.len() {
                if !full.is_char_boundary(end) {
                    continue;
                }
                let prefix = &full[..end];
                committed.extend(render.commit(prefix, width).iter().map(styled));
                for p in render.preview(prefix, width, usize::MAX) {
                    let p = styled(&p);
                    // A non-blank preview row must not already sit in scrollback.
                    if p.iter().any(|(t, _)| !t.trim().is_empty()) {
                        assert!(
                            !committed.contains(&p),
                            "preview {p:?} duplicates a committed row streaming {full:?} at {prefix:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn stream_render_preview_is_the_last_rendered_row() {
        // The strip preview must equal the last row of the full render — but
        // computed cheaply. Check it across a growing code reply.
        let full = "Here:\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
        let width = 30;
        let mut render = StreamRender::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let acc = &full[..end];
            let expected: Vec<String> = message_lines(Role::Assistant, acc, width)
                .pop()
                .map(|l| plain(&l))
                .into_iter()
                .collect();
            let got: Vec<String> = render
                .preview(acc, width, usize::MAX)
                .iter()
                .map(plain)
                .collect();
            assert_eq!(got, expected, "preview mismatch at {acc:?}");
            // Advancing the preview must not disturb a subsequent commit.
            let _ = render.commit(acc, width);
        }
    }

    #[test]
    fn stream_render_rebuilds_on_a_width_change() {
        // A mid-stream width change (resize) must rebuild the cache from scratch
        // — no stale rows, no panic — matching the boundary's `reset` + re-commit.
        let text = "the quick brown fox jumps over the lazy dog";
        let mut render = StreamRender::new();
        let narrow = render.commit(text, 6);
        assert!(!narrow.is_empty(), "a narrow wrap commits several lines");

        // Re-wrapped wider: the cache rebuilds; committed + finish still equals
        // the whole reply rendered at the new width.
        let wide_commit = render.commit(text, 80);
        let wide_finish = render.finish(text, 80);
        let got: Vec<String> = wide_commit
            .iter()
            .chain(wide_finish.iter())
            .map(plain)
            .collect();
        let expected: Vec<String> = message_lines(Role::Assistant, text, 80)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(got, expected, "rebuilt at the new width");
    }

    #[test]
    fn assistant_lines_trims_a_trailing_paragraph_break() {
        // A reply ending with a blank line (a model often emits "…\n\n" before a
        // tool call) must render no trailing blank rows: the caller adds exactly
        // one spacer, so trailing blanks would stack (the 3-newline bug).
        let with: Vec<String> = assistant_lines("Building it.\n\n", 80, AI_BULLET, AI_COLOR)
            .iter()
            .map(plain)
            .collect();
        let without: Vec<String> = assistant_lines("Building it.", 80, AI_BULLET, AI_COLOR)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(with, without, "trailing blank rows are trimmed");
    }

    #[test]
    fn assistant_lines_keeps_interior_blank_lines() {
        // Only *trailing* blanks are trimmed — a paragraph break in the middle
        // stays (it separates two paragraphs).
        let rows = assistant_lines("One.\n\nTwo.", 80, AI_BULLET, AI_COLOR);
        assert_eq!(rows.len(), 3, "the interior blank is preserved");
        assert!(plain(&rows[0]).contains("One."));
        assert!(plain(&rows[1]).trim().is_empty(), "middle row is blank");
        assert!(plain(&rows[2]).contains("Two."));
    }

    #[test]
    fn stream_render_trims_a_trailing_paragraph_break() {
        // Streaming "text.\n\n" char-by-char (as a real model emits before a tool
        // call) then finishing must commit no trailing blank rows — otherwise the
        // boundary's single spacer stacks into three (the reported bug).
        let full = "Building it.\n\n";
        let width = 80;
        let mut render = StreamRender::new();
        let mut got: Vec<String> = Vec::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            got.extend(render.commit(&full[..end], width).iter().map(plain));
        }
        got.extend(render.finish(full, width).iter().map(plain));
        assert_eq!(
            got,
            vec!["● Building it.".to_string()],
            "no trailing blanks"
        );
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
            live_height(&TextArea::from_text(""), 40, 24, false, 0, 0, 0, 0, 0, 0),
            LIVE_MIN_HEIGHT
        );
        assert_eq!(
            live_height(&TextArea::from_text("hi"), 40, 24, false, 0, 0, 0, 0, 0, 0),
            LIVE_MIN_HEIGHT
        );
    }

    #[test]
    fn live_height_adds_the_streaming_strip_above_the_box() {
        // While streaming, the live region gains a preview row, a blank gap row,
        // the live status row, and a blank gap below it (1 preview + GAP_ROWS
        // + STATUS_ROWS + STATUS_GAP_ROWS = 4) above whatever the idle box would be.
        for input in ["", "hi", "a\nb\nc"] {
            let ta = TextArea::from_text(input);
            assert_eq!(
                live_height(&ta, 40, 24, true, 1, 0, 0, 0, 0, 0),
                live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0) + 4,
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
                0,
                0,
                0,
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
                0,
                0,
                0,
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
            live_height(&many, 40, 10, false, 0, 0, 0, 0, 0, 0),
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
            // 0 = no preview; 1 = a single-row preview; 2 = a running backend
            // tool's multi-row cell (wrapped header + `⎿ Running…`).
            for preview_rows in [0u16, 1, 2] {
                for band_rows in [0, 3] {
                    for footer_rows in [0, 1] {
                        // The smallest height still fits the tallest strip (a
                        // 2-row preview cell → 2 + gap + status + gap = 5) +
                        // band (3) + footer (1) = 9.
                        for h in [LIVE_MIN_HEIGHT + 6, 12, 24] {
                            let [strip, input, band, footer, _] = live_layout(
                                Rect::new(0, 0, 40, h),
                                streaming,
                                preview_rows,
                                0,
                                0,
                                band_rows,
                                footer_rows,
                                0,
                            );
                            assert_eq!(
                                strip.height + input.height + band.height + footer.height,
                                h,
                                "sub-areas tile the area"
                            );
                            assert_eq!(strip.height, strip_rows(streaming, preview_rows));
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
        let h = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
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
            images: Vec::new(),
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
    fn conversation_lines_puts_one_blank_between_trailing_break_text_and_a_tool() {
        // The reported bug: an assistant segment ending with a paragraph break
        // (`…\n\n`) before a tool call must show exactly ONE blank row between
        // them on a repaint — not three (the trailing blanks plus the spacer).
        let history = [
            msg(Role::User, "go"),
            msg(Role::Assistant, "I'll do it.\n\n"),
            HistoryItem::Tool(tool("Bash", "ls", ToolStatus::Ok, "out")),
        ];
        let texts: Vec<String> = conversation_lines(&history, 80)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        let text_idx = texts
            .iter()
            .position(|t| t == "● I'll do it.")
            .unwrap_or_else(|| panic!("assistant text present: {texts:?}"));
        let tool_idx = texts
            .iter()
            .position(|t| t == "● Bash(ls)")
            .unwrap_or_else(|| panic!("tool header present: {texts:?}"));
        assert_eq!(
            tool_idx - text_idx,
            2,
            "exactly one blank row between text and tool: {texts:?}"
        );
        assert_eq!(texts[text_idx + 1], "", "the single separator is blank");
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
    fn repaint_tail_repaints_the_partial_reply_rows_already_committed() {
        // Mid-stream repaint (the Ctrl+O overlay return): the tail must carry
        // the rows the stream had already committed to scrollback — repainting
        // from history alone blanks the partial reply until the next chunk
        // arrives (the disappear-then-flicker bug).
        let width = 30;
        let history = [msg(Role::User, "hi")];
        let partial = "first line of the reply\nsecond line still growing";
        let mut render = StreamRender::new();
        let committed: Vec<String> = render.commit(partial, width).iter().map(plain).collect();
        assert!(!committed.is_empty(), "the completed first line is stable");

        let tail: Vec<String> = repaint_tail(&history, Some(partial), &mut render, width, 100)
            .iter()
            .map(plain)
            .collect();
        let mut expected: Vec<String> = repaint_lines(&history, width, 100)
            .iter()
            .map(plain)
            .collect();
        expected.extend(committed);
        assert_eq!(tail, expected);
    }

    #[test]
    fn repaint_tail_repaints_committed_rows_of_a_still_open_line() {
        // The committed counter can point past `frozen` (wrapped rows of a
        // prose line that hasn't seen its newline yet) — those rows reached
        // scrollback too, so the repaint must reproduce them.
        let width = 18;
        let before = "a long prose line that wraps into a good number of rows here";
        let mut render = StreamRender::new();
        let committed: Vec<String> = render.commit(before, width).iter().map(plain).collect();
        assert!(
            committed.len() > 1,
            "several wrapped rows are stable: {committed:?}"
        );
        let full = format!("{before} and more");
        let tail: Vec<String> = repaint_tail(&[], Some(&full), &mut render, width, 100)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(tail, committed);
    }

    #[test]
    fn repaint_tail_then_commit_catches_up_without_duplicate_or_gap() {
        // Chunks that arrived while the overlay was up commit right after the
        // repaint: committed-before ++ committed-after ++ finish must
        // reconstruct the whole reply exactly — no row lost, none inserted
        // twice (the scrollback-duplication half of the bug).
        let width = 24;
        let before = "streamed before the overlay opened\nand a second line\n";
        let full =
            format!("{before}plus lines that arrived\nwhile the overlay was up\nstill going");
        let mut render = StreamRender::new();
        let mut inserted: Vec<String> = render.commit(before, width).iter().map(plain).collect();

        // The overlay round-trip: the tail repaints exactly what was already
        // committed (empty history keeps the comparison direct)…
        let tail: Vec<String> = repaint_tail(&[], Some(&full), &mut render, width, 100)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(tail, inserted, "the tail repaints the committed rows only");
        // …and the follow-up commit emits just the overlay-time delta.
        inserted.extend(render.commit(&full, width).iter().map(plain));
        inserted.extend(render.finish(&full, width).iter().map(plain));

        let expected: Vec<String> = message_lines(Role::Assistant, &full, width)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(inserted, expected);
    }

    #[test]
    fn repaint_tail_without_a_stream_matches_repaint_lines() {
        let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
        let mut render = StreamRender::new();
        let tail: Vec<String> = repaint_tail(&history, None, &mut render, 80, 2)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(tail, vec!["● two", ""]);
    }

    #[test]
    fn repaint_tail_cap_keeps_the_newest_rows_including_the_partial() {
        // The row budget applies to the combined tail — history and partial
        // together — keeping the newest rows, like a screen would.
        let width = 80;
        let history = [msg(Role::User, "one")];
        let partial = "alpha\nbeta\ngamma";
        let mut render = StreamRender::new();
        let _ = render.commit(partial, width);
        let tail: Vec<String> = repaint_tail(&history, Some(partial), &mut render, width, 2)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(tail.len(), 2);
        assert!(
            tail[1].contains("beta"),
            "newest stream rows kept: {tail:?}"
        );
    }

    #[test]
    fn repaint_tail_after_a_width_change_carries_no_stale_rows() {
        // A width change rebuilt the render's cache: nothing is "already
        // committed" at the new width, so the tail carries no stale-width rows
        // and the follow-up commit re-emits the reply wrapped fresh.
        let partial = "one two three four five six seven\nnext";
        let mut render = StreamRender::new();
        let _ = render.commit(partial, 20);
        let tail = repaint_tail(&[], Some(partial), &mut render, 40, 100);
        assert!(tail.is_empty(), "no stale-width rows repainted");
        let recommitted: Vec<String> = render.commit(partial, 40).iter().map(plain).collect();
        let expected: Vec<String> = message_lines(Role::Assistant, partial, 40)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(recommitted[..], expected[..recommitted.len()]);
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

    #[test]
    fn preview_rows_counts_the_running_backend_tool_cell() {
        // The strip's preview slot is sized by `preview_rows`: nothing to preview
        // → 0; a streaming reply → 1; a running backend tool → its whole collapsed
        // cell (header + `⎿ Running…`), so a long command isn't clipped live.
        let mut app = App::new();
        app.begin_stream();
        assert_eq!(
            preview_rows(&app, 40),
            0,
            "empty pre-stream pause: no preview"
        );
        app.push_chunk("hi");
        assert_eq!(
            preview_rows(&app, 40),
            1,
            "a streaming reply previews one row"
        );
        app.start_tool("Bash", "sleep 1");
        // Past the hint delay so the Ctrl+B hint row is part of the preview.
        app.set_command_elapsed(Some(Duration::from_secs(3)));
        assert_eq!(
            preview_rows(&app, 40),
            3,
            "a running backend tool previews header + ⎿ Running… + the Ctrl+B hint"
        );
    }

    #[test]
    fn preview_shows_the_whole_parallel_batch_running_plus_waiting() {
        // A parallel batch previews every call: the running one + each `Waiting`
        // sibling, blank-separated. `preview_rows` counts them all, and
        // `render_live` paints the running cell alongside the `⎿ Waiting…`
        // siblings — so the batch is visible and clear. See
        // `docs/parallel-tools.md`.
        let mut app = App::new();
        app.begin_stream();
        let batch: Vec<crate::stream::ToolCallSummary> =
            ["ping google.com", "ping facebook.com", "ping x.com"]
                .iter()
                .map(|cmd| crate::stream::ToolCallSummary {
                    name: "Bash".to_string(),
                    args: (*cmd).to_string(),
                })
                .collect();
        app.start_tool_batch(&batch);
        app.start_tool("Bash", "ping google.com"); // the front call → Running
        // Past the hint delay so the running cell's Ctrl+B hint row shows.
        app.set_command_elapsed(Some(Duration::from_secs(3)));
        // Three 2-row cells (header + peek) with two blank separators, plus
        // the running cell's Ctrl+B hint row = 9 rows.
        assert_eq!(
            preview_rows(&app, 40),
            9,
            "the whole batch (3 cells + 2 gaps + the running cell's hint) is previewed"
        );
        let pv = preview_rows(&app, 40);
        let h = live_height(&app.input, 40, 30, true, pv, 0, 0, 0, 0, 0);
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains("ping google.com") && all.contains("Running…"),
            "the running call shows its header + Running…: {all:?}"
        );
        assert!(
            all.contains("ping facebook.com") && all.contains("ping x.com"),
            "the waiting siblings are shown: {all:?}"
        );
        assert_eq!(
            all.matches("Waiting…").count(),
            2,
            "exactly the two not-yet-run siblings show Waiting…: {all:?}"
        );
    }

    #[test]
    fn render_live_previews_a_running_tool_with_its_running_row() {
        // While a backend tool runs the strip shows the whole cell — the header
        // *and* a `⎿ Running…` row beneath it — not just the header (req 2).
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "sleep 1");
        let h = live_height(
            &app.input,
            40,
            24,
            true,
            preview_rows(&app, 40),
            0,
            0,
            0,
            0,
            0,
        );
        let mut buf = buffer(40, h);
        render_live(buf.area, &mut buf, &app);
        assert!(
            row(&buf, 0, 40).contains("Bash(sleep 1)"),
            "row 0 shows the header: {:?}",
            row(&buf, 0, 40)
        );
        let all: String = (0..h)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all.contains('⎿') && all.contains("Running…"),
            "the strip shows the ⎿ Running… row: {all:?}"
        );
    }

    #[test]
    fn render_live_grows_the_preview_to_fit_a_long_running_command() {
        // A running backend tool with a *long* command previews its whole cell:
        // the header wraps across rows (never clipped at the edge) AND the
        // `⎿ Running…` row sits beneath the last header row — so the live preview
        // grows past the usual two rows. Guards the header-wrap + running-row
        // composition in the real render (not just `tool_lines` in isolation).
        let cmd = "curl -s \"wttr.in/Warsaw?format=%C+%t+%w+%h\" 2>/dev/null \
                   || echo \"wttr.in unavailable, trying alternative...\"";
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", cmd);
        let width = 40;
        let pv = preview_rows(&app, width);
        assert!(
            pv > 2,
            "a wrapped header + ⎿ Running… is more than two rows: {pv}"
        );
        let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0);
        let mut buf = buffer(width, h);
        render_live(buf.area, &mut buf, &app);
        let rows: Vec<String> = (0..h).map(|y| row(&buf, y, width)).collect();
        // The header wraps: row 0 opens it, and at least one later row is an
        // indented continuation (aligned under the opening `(`), before the ⎿ row.
        assert!(
            rows[0].starts_with("● Bash("),
            "row 0 opens the header: {:?}",
            rows[0]
        );
        let running_y = rows
            .iter()
            .position(|r| r.contains('⎿') && r.contains("Running…"))
            .expect("a ⎿ Running… row is drawn");
        assert!(running_y >= 2, "the header took ≥2 rows before ⎿: {rows:?}");
        assert!(
            rows[1].starts_with(&" ".repeat(cols("● Bash"))),
            "the header's second row aligns under the (: {:?}",
            rows[1]
        );
        // Nothing clipped: no drawn row exceeds the width.
        for r in &rows {
            assert!(
                cols(r.trim_end()) <= width as usize,
                "row within width: {r:?}"
            );
        }
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
    fn centered_window_keeps_the_selection_centered() {
        // Everything fits in one window — never scrolls.
        assert_eq!(
            centered_window(3, 2, 10),
            0,
            "no scroll when everything fits"
        );
        assert_eq!(centered_window(10, 9, 10), 0, "an exact fit never scrolls");
        // Near the top of a long list the window is anchored at the top (it can't
        // center a selection with too few rows above it); the highlight walks down
        // to the middle row (max/2).
        assert_eq!(centered_window(445, 0, 10), 0, "top of the list");
        assert_eq!(
            centered_window(445, 4, 10),
            0,
            "still climbing to the middle"
        );
        assert_eq!(
            centered_window(445, 5, 10),
            0,
            "reaches the middle row (max/2)"
        );
        // In the interior the window follows the selection so it stays centered —
        // the fix: broad view above *and* below, not pinned to the bottom edge.
        assert_eq!(centered_window(445, 20, 10), 15, "stays centered mid-list");
        assert_eq!(centered_window(445, 60, 10), 55, "stays centered mid-list");
        // Near the end the window clamps flush with the tail; the highlight rides
        // down from the middle to the bottom row (it can't center past the end).
        assert_eq!(
            centered_window(445, 440, 10),
            435,
            "clamped to the last window"
        );
        assert_eq!(centered_window(445, 444, 10), 435, "last row sits flush");
        // A degenerate zero-height window never panics.
        assert_eq!(centered_window(5, 3, 0), 0, "zero window");
    }

    #[test]
    fn menu_rows_is_zero_when_the_palette_is_closed() {
        assert_eq!(menu_rows(&App::new()), 0);
    }

    #[test]
    fn the_menu_cap_holds_the_whole_command_registry() {
        // MENU_MAX_ROWS is sized so a bare `/` lists EVERY command without
        // scrolling (its doc contract; smoke.sh asserts /quit — the last —
        // is visible). Adding a command must grow the cap with it.
        assert!(
            crate::app::COMMANDS.len() <= MENU_MAX_ROWS as usize,
            "MENU_MAX_ROWS ({MENU_MAX_ROWS}) no longer fits the {} registered commands",
            crate::app::COMMANDS.len()
        );
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

    // --- /compact: the marker cell (docs/compact.md) ---

    /// A test marker with no token info (the pre-gauge shape).
    fn bare_compaction(summary: &str) -> crate::app::Compaction {
        crate::app::Compaction {
            summary: summary.into(),
            timestamp: String::new(),
            before: 0,
            after: 0,
            auto: false,
        }
    }

    #[test]
    fn compaction_lines_render_the_cyan_marker_cell() {
        // The inline cell is codex's "Context compacted" info line, in our
        // system-notice dress (cyan ●). No token info → just the notice.
        let lines = compaction_lines(&bare_compaction("s"), 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
        assert_eq!(lines[0].spans[0].style.fg, Some(SYSTEM_COLOR));
    }

    #[test]
    fn the_compaction_cell_appends_the_token_shrink_and_auto_tag() {
        // The gauge info rides the cell: `· {before} → {after} tokens`, plus
        // `· auto` when the compaction was auto-triggered (docs/compact.md).
        let compaction = crate::app::Compaction {
            summary: "s".into(),
            timestamp: String::new(),
            before: 88_000,
            after: 2_100,
            auto: true,
        };
        let text = plain(&compaction_lines(&compaction, 120)[0]);
        assert!(text.starts_with(&format!("● {COMPACTED_NOTICE}")), "{text}");
        assert!(text.contains("88k → 2.1k tokens"), "{text}");
        assert!(text.ends_with("· auto"), "{text}");
    }

    #[test]
    fn a_manual_compaction_cell_shows_the_shrink_without_the_auto_tag() {
        let compaction = crate::app::Compaction {
            summary: "s".into(),
            timestamp: String::new(),
            before: 1_000,
            after: 300,
            auto: false,
        };
        let text = plain(&compaction_lines(&compaction, 120)[0]);
        assert!(text.contains("1k → 300 tokens"), "{text}");
        assert!(!text.contains("auto"), "{text}");
    }

    #[test]
    fn conversation_lines_keep_the_compaction_marker_collapsed() {
        // The inline repaint shows the one-line cell + the spacer — the summary
        // body is Ctrl+O-only.
        let history = vec![HistoryItem::Compaction(bare_compaction("kept the gist"))];
        let lines = conversation_lines(&history, 80);
        assert_eq!(lines.len(), 2, "marker + spacer: {:?}", lines.len());
        assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
    }

    #[test]
    fn the_transcript_expands_the_compaction_summary_dim_below_the_marker() {
        let compaction = bare_compaction("kept the gist");
        let lines = compaction_full_lines(&compaction, 80);
        assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
        assert_eq!(plain(&lines[1]), format!("{INDENT}kept the gist"));
        assert_eq!(
            lines[1].spans[1].style.fg,
            Some(TOOL_DIM_COLOR),
            "the summary body is dim"
        );
    }

    #[test]
    fn an_empty_compaction_summary_expands_to_just_the_marker() {
        assert_eq!(compaction_full_lines(&bare_compaction(""), 80).len(), 1);
    }

    // --- the footer context gauge (docs/compact.md) ---

    #[test]
    fn the_footer_shows_the_context_gauge_when_the_window_is_known() {
        let mut app = App::new();
        app.set_session_info("some-model", "~/x");
        app.set_context_window(Some(300_000));
        app.begin_stream();
        app.apply_usage(&crate::stream::TokenUsage {
            input: 17_900,
            output: 100,
            cached: 0,
            cache_write: 0,
        });
        let text = plain(&footer_line(&app, 120));
        assert!(text.contains("18k/300k (6.0%)"), "{text}");
    }

    #[test]
    fn the_footer_gauge_humanizes_the_used_tokens_beside_the_window() {
        // The numerator is the live context size, humanized like the window
        // (`1.3k/160k (0.8%)`) — the raw count the percentage alone hid.
        let mut app = App::new();
        app.set_session_info("deepseek-v3.2", "~/Codes/tmp");
        app.set_context_window(Some(160_000));
        app.begin_stream();
        app.apply_usage(&crate::stream::TokenUsage {
            input: 1_250,
            output: 50,
            cached: 0,
            cache_write: 0,
        });
        let text = plain(&footer_line(&app, 120));
        assert!(text.contains("1.3k/160k (0.8%)"), "{text}");
    }

    #[test]
    fn the_footer_gauge_shows_a_small_context_bare() {
        // Under a thousand the formatter stays bare (`842`), so a fresh
        // session reads `842/160k (0.5%)` rather than `0.8k/160k`.
        let mut app = App::new();
        app.set_session_info("deepseek-v3.2", "~/Codes/tmp");
        app.set_context_window(Some(160_000));
        app.begin_stream();
        app.apply_usage(&crate::stream::TokenUsage {
            input: 800,
            output: 42,
            cached: 0,
            cache_write: 0,
        });
        let text = plain(&footer_line(&app, 120));
        assert!(text.contains("842/160k (0.5%)"), "{text}");
    }

    #[test]
    fn the_footer_omits_the_gauge_without_a_window() {
        let mut app = App::new();
        app.set_session_info("some-model", "~/x");
        let text = plain(&footer_line(&app, 120));
        assert!(!text.contains('%'), "{text}");
    }

    #[test]
    fn live_height_adds_the_command_menu_band() {
        let closed = live_height(&TextArea::from_text("hi"), 40, 24, false, 0, 0, 0, 0, 0, 0);
        let open = live_height(
            &TextArea::from_text("/"),
            40,
            24,
            false,
            0,
            0,
            0,
            MENU_MAX_ROWS,
            0,
            0,
        );
        assert_eq!(open, closed + MENU_MAX_ROWS, "the menu band adds its rows");
    }

    #[test]
    fn render_live_draws_the_command_menu_below_the_box() {
        let app = palette("/", 0);
        let menu = menu_rows(&app);
        let h = live_height(&app.input, 40, 24, false, 0, 0, 0, menu, 0, 0);
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
            live_height(&TextArea::from_text("/"), 40, 24, false, 0, 0, 0, 0, 0, 0),
        );
        let closed = cursor_position(closed_area, &app);
        app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
        let menu = menu_rows(&app);
        let open_area = Rect::new(
            0,
            0,
            40,
            live_height(
                &TextArea::from_text("/"),
                40,
                24,
                false,
                0,
                0,
                0,
                menu,
                0,
                0,
            ),
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
        let texts: Vec<String> = shortcuts_lines(false, false)
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
        let texts: Vec<String> = shortcuts_lines(false, false)
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
            texts[2].contains("shift+enter for newline")
                && texts[2].contains("ctrl+o for tool output"),
            "{texts:?}"
        );
        assert!(
            texts[3].contains("esc to quit") && texts[3].contains("ctrl+c to quit"),
            "{texts:?}"
        );
        assert!(texts[4].contains("alt+↑ to edit queue"), "{texts:?}");
        assert!(
            texts[5].contains("ctrl+v for image paste")
                && texts[5].contains("ctrl+d for llm context"),
            "{texts:?}"
        );
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
        let all: String = shortcuts_lines(false, false)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("alt+↑ to edit queue"), "{all:?}");
    }

    #[test]
    fn shortcuts_lines_list_the_shift_tab_thinking_binding() {
        // Shift+Tab (cycle the thinking mode, docs/reasoning.md) is
        // discoverable in the `?` band like every other binding.
        let all: String = shortcuts_lines(false, false)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("shift+tab to cycle thinking"), "{all:?}");
    }

    #[test]
    fn shortcuts_lines_flip_the_esc_entry_while_a_turn_runs() {
        // codex's quit entry is context-sensitive: "to interrupt" while a task
        // runs. Our Esc entry flips the same way.
        let idle: Vec<String> = shortcuts_lines(false, false)
            .iter()
            .map(|l| plain(l))
            .collect();
        let busy: Vec<String> = shortcuts_lines(true, false)
            .iter()
            .map(|l| plain(l))
            .collect();
        assert!(idle.iter().any(|t| t.contains("esc to quit")), "{idle:?}");
        assert!(
            busy.iter().any(|t| t.contains("esc to interrupt")),
            "{busy:?}"
        );
        assert!(!busy.iter().any(|t| t.contains("esc to quit")), "{busy:?}");
    }

    #[test]
    fn shortcuts_lines_style_keys_cyan_and_labels_dim() {
        for line in shortcuts_lines(false, false) {
            // spans = [key, label, pad, key, label] — keys cyan, labels dim.
            assert_eq!(line.spans[0].style.fg, Some(SHORTCUTS_KEY_COLOR));
            assert_eq!(line.spans[1].style.fg, Some(SHORTCUTS_TEXT_COLOR));
        }
    }

    #[test]
    fn live_height_adds_the_shortcuts_band() {
        let mut app = App::new();
        app.shortcuts_open = true;
        let closed = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0);
        let open = live_height(
            &app.input,
            40,
            24,
            false,
            0,
            0,
            0,
            shortcuts_rows(&app),
            0,
            0,
        );
        assert_eq!(open, closed + shortcuts_rows(&app));
    }

    #[test]
    fn render_live_draws_the_shortcuts_band_below_the_box() {
        let mut app = App::new();
        app.shortcuts_open = true;
        let h = live_height(
            &app.input,
            60,
            24,
            false,
            0,
            0,
            0,
            shortcuts_rows(&app),
            0,
            0,
        );
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 60))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("/ for commands"), "band rendered: {all:?}");
        assert!(
            row(&buf, h - 1, 60).contains("shift+tab to cycle thinking"),
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
            live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0),
        );
        let closed = cursor_position(closed_area, &app);
        app.shortcuts_open = true;
        let open_area = Rect::new(
            0,
            0,
            40,
            live_height(
                &app.input,
                40,
                24,
                false,
                0,
                0,
                0,
                shortcuts_rows(&app),
                0,
                0,
            ),
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
        let without = live_height(&app.input, 40, 24, true, 1, 0, 0, 0, 0, 0);
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let with = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
        assert_eq!(with, without + q, "the queue grows the region by its rows");
        assert_eq!(q, 1, "one short queued message is one row");
    }

    #[test]
    fn render_live_draws_the_queue_above_the_box_as_a_user_message() {
        let mut app = App::new();
        app.begin_stream();
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let h = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
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
        let h = live_height(
            &app.input,
            40,
            24,
            true,
            1,
            q,
            0,
            shortcuts_rows(&app),
            0,
            0,
        );
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
        app.set_session_info("dummy_model_name", "~/alter-zero");
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
        assert_eq!(plain(&line), "  dummy_model_name · ~/alter-zero");
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
    fn footer_line_shows_the_thinking_mode_beside_the_model() {
        // A reasoning-capable model carries its mode right after the model
        // name — `{model} {mode} · {cwd}` — so the current thinking level is
        // always visible (docs/reasoning.md).
        use crate::llm::{ReasoningEffort, ReasoningSupport, ThinkingMode};
        let mut app = with_session();
        app.set_thinking(Some((
            ReasoningSupport {
                efforts: vec![ReasoningEffort::Medium],
                can_disable: true,
                default_effort: None,
            },
            ThinkingMode::Effort(ReasoningEffort::Medium),
        )));
        let line = footer_line(&app, 60);
        assert_eq!(plain(&line), "  dummy_model_name medium · ~/alter-zero");
        for span in &line.spans[1..] {
            assert_eq!(span.style.fg, Some(FOOTER_COLOR), "dim: {:?}", span.content);
        }
        // Off is a mode too — the user must see thinking is disabled.
        app.thinking.as_mut().unwrap().mode = ThinkingMode::Off;
        assert_eq!(
            plain(&footer_line(&app, 60)),
            "  dummy_model_name off · ~/alter-zero"
        );
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

    // --- the startup header banner (docs/header.md) ---

    /// The whole banner as one plain string (rows joined by newlines).
    fn header_text(app: &App, width: u16) -> String {
        header_lines(app, width)
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn header_full_shows_logo_version_cwd_tagline_and_hint() {
        let text = header_text(&with_session(), 90);
        assert!(text.contains('█'), "block wordmark art: {text:?}");
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "version: {text:?}"
        );
        assert!(
            text.contains("~/alter-zero"),
            "cwd from the session: {text:?}"
        );
        assert!(text.contains("autonomous ai agent"), "tagline: {text:?}");
        assert!(text.contains("terminal ui"), "tagline: {text:?}");
        for token in ["/help", "/model", "/resume"] {
            assert!(text.contains(token), "hint token {token}: {text:?}");
        }
    }

    #[test]
    fn header_falls_back_to_the_compact_wordmark_when_mid_width() {
        let width = 50;
        let lines = header_lines(&with_session(), width);
        let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        // The compact half-block wordmark uses `▀`, which the full block art
        // never does — so its presence proves the mid-width tier was chosen.
        assert!(text.contains('▀'), "compact half-block wordmark: {text:?}");
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "version kept: {text:?}"
        );
        assert!(text.contains("~/alter-zero"), "cwd kept: {text:?}");
        for line in &lines {
            assert!(
                cols(&plain(line)) <= width as usize,
                "fits {width}: {line:?}"
            );
        }
    }

    #[test]
    fn header_falls_back_to_a_text_badge_when_very_narrow() {
        let width = 24;
        let lines = header_lines(&with_session(), width);
        let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        // Neither wordmark spells the name in literal letters — a literal
        // "ALTER ZERO" can only be the one-line text badge.
        assert!(text.contains("ALTER ZERO"), "text badge: {text:?}");
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "version: {text:?}"
        );
        for line in &lines {
            assert!(
                cols(&plain(line)) <= width as usize,
                "fits {width}: {line:?}"
            );
        }
    }

    #[test]
    fn header_never_exceeds_the_width() {
        let app = with_session();
        for width in [16u16, 20, 24, 39, 40, 50, 73, 75, 80, 120] {
            for line in header_lines(&app, width) {
                assert!(
                    cols(&plain(&line)) <= width as usize,
                    "width {width}: row {:?} overflows",
                    plain(&line)
                );
            }
        }
    }

    #[test]
    fn header_logo_carries_the_cyan_to_blue_gradient() {
        let lines = header_lines(&with_session(), 90);
        // Row 0 starts at column 0 (gradient t=0) → the exact cyan endpoint.
        let first = lines[0].spans.first().expect("a logo span");
        assert_eq!(
            first.style.fg,
            Some(Color::Rgb(0x56, 0xB6, 0xC2)),
            "logo starts cyan"
        );
        // Some cell reaches the far edge (t=1) → the exact blue endpoint.
        let has_blue = lines.iter().take(6).any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Rgb(0x61, 0xAF, 0xEF)))
        });
        assert!(has_blue, "logo ends blue");
    }

    #[test]
    fn header_logo_rows_match_the_wordmark_art_verbatim() {
        // The `A`'s crown row leads with a space; a `\`-continued string literal
        // strips it and shifts the glyph a column left — regression guard.
        let lines = header_lines(&with_session(), 90);
        for (i, art) in HEADER_LOGO_FULL.iter().enumerate() {
            assert_eq!(&plain(&lines[i]), art, "logo row {i} rendered verbatim");
        }
        assert!(
            plain(&lines[0]).starts_with(' '),
            "the A's crown keeps its leading indent"
        );
    }

    #[test]
    fn header_without_a_session_still_shows_logo_and_version() {
        let text = header_text(&App::new(), 90);
        assert!(text.contains('█'), "logo still drawn: {text:?}");
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "version: {text:?}"
        );
    }

    #[test]
    fn header_avoids_the_smoke_reserved_strings() {
        // The banner shares the screen with the smoke suite's structural
        // counters (docs/header.md, smoke Phases 11/16/17): it must never carry
        // these markers, nor a full `─` rule / bare `❯` row.
        for width in [24u16, 50, 90] {
            let lines = header_lines(&with_session(), width);
            let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
            for banned in [
                "for commands",
                "dummy_model_name",
                "Happy",
                "Done for",
                "esc to interrupt",
                "Conversation interrupted",
            ] {
                assert!(
                    !text.contains(banned),
                    "width {width} leaks {banned:?}: {text:?}"
                );
            }
            for line in &lines {
                let row = plain(line);
                let trimmed = row.trim();
                let is_rule = !trimmed.is_empty() && trimmed.chars().all(|c| c == '─');
                assert!(!is_rule, "width {width} drew a rule row: {row:?}");
                assert_ne!(trimmed, "❯", "width {width} drew a bare prompt row");
            }
        }
    }

    // --- the banner-topped repaint tail (docs/header.md) ---

    #[test]
    fn banner_tail_restores_the_banner_over_a_short_tail() {
        // The InPlace overlay return on a short conversation: the banner the
        // window still showed comes back — banner, spacer, then the tail.
        let out = banner_tail(
            vec![Line::raw("LOGO"), Line::raw("meta")],
            vec![Line::raw("❯ hi"), Line::raw("ok")],
            10,
        );
        let texts: Vec<String> = out.iter().map(plain).collect();
        assert_eq!(texts, ["LOGO", "meta", "", "❯ hi", "ok"]);
    }

    #[test]
    fn banner_tail_drops_the_banner_once_the_window_is_full() {
        // A conversation that already fills the repaint window: the recap
        // drops the banner — it scrolled into the terminal's kept scrollback,
        // and re-adding it on screen would duplicate it.
        let tail: Vec<Line<'static>> = (0..4).map(|i| Line::raw(format!("r{i}"))).collect();
        let out = banner_tail(vec![Line::raw("LOGO")], tail, 4);
        let texts: Vec<String> = out.iter().map(plain).collect();
        assert_eq!(texts, ["r0", "r1", "r2", "r3"]);
    }

    #[test]
    fn banner_tail_keeps_the_banner_bottom_when_it_half_fits() {
        // Mid-scroll: the window held only the banner's bottom rows, so only
        // those come back — the top rows stay in the kept scrollback above.
        let out = banner_tail(
            vec![Line::raw("top"), Line::raw("bottom")],
            vec![Line::raw("❯ hi")],
            3,
        );
        let texts: Vec<String> = out.iter().map(plain).collect();
        assert_eq!(texts, ["bottom", "", "❯ hi"]);
    }

    #[test]
    fn banner_tail_uncapped_never_clips() {
        // The Purge rebuild passes usize::MAX: the banner tops the fresh
        // scrollback whatever the conversation's length.
        let tail: Vec<Line<'static>> = (0..100).map(|i| Line::raw(format!("r{i}"))).collect();
        let out = banner_tail(vec![Line::raw("LOGO")], tail, usize::MAX);
        assert_eq!(out.len(), 102, "banner + spacer + every tail row");
        assert_eq!(plain(&out[0]), "LOGO");
    }

    #[test]
    fn live_height_adds_the_footer_row() {
        let ta = TextArea::from_text("hi");
        assert_eq!(
            live_height(&ta, 40, 24, false, 0, 0, 0, 0, 1, 0),
            live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0) + 1,
            "the footer adds its row at the very bottom"
        );
    }

    #[test]
    fn render_live_paints_the_footer_on_the_last_row() {
        let app = with_session();
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(
            last.contains("dummy_model_name · ~/alter-zero"),
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
        let h = live_height(&app.input, 60, 24, true, 1, 0, 0, 0, 1, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert!(
            row(&buf, h - 1, 60).contains("dummy_model_name"),
            "the footer is ambient — present mid-turn too"
        );
    }

    // --- the transient toast (docs/toast.md) ---

    #[test]
    fn toast_rows_is_zero_without_a_toast_and_one_with_it() {
        let mut app = App::new();
        assert_eq!(toast_rows(&app), 0);
        app.show_toast("Copied last message to clipboard", ToastKind::Info);
        assert_eq!(toast_rows(&app), 1);
    }

    #[test]
    fn live_height_reserves_exactly_one_row_for_the_toast() {
        let mut app = App::new();
        let without = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 0);
        app.show_toast("hi", ToastKind::Info);
        let with = live_height(&app.input, 60, 24, false, 0, 0, toast_rows(&app), 0, 0, 0);
        assert_eq!(with, without + 1, "the toast adds exactly one row");
    }

    #[test]
    fn render_live_paints_the_toast_directly_above_the_box_when_idle() {
        let mut app = App::new();
        app.show_toast("Copied last message to clipboard", ToastKind::Info);
        let h = live_height(&app.input, 60, 24, false, 0, 0, toast_rows(&app), 0, 0, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert!(
            row(&buf, 0, 60).contains("Copied last message to clipboard"),
            "the toast is the region's first row: {:?}",
            row(&buf, 0, 60)
        );
        assert!(row(&buf, 0, 60).starts_with("  "), "two-column inset");
        assert_eq!(
            buf[(0, 1)].symbol(),
            "─",
            "the box's top rule sits directly below the toast"
        );
    }

    #[test]
    fn render_live_paints_the_toast_below_the_status_while_streaming() {
        let mut app = App::new();
        app.begin_stream();
        app.push_chunk("hi");
        app.set_status_times(Duration::from_secs(1), None);
        app.show_toast(
            "/resume is disabled while a task is in progress",
            ToastKind::Info,
        );
        let h = live_height(&app.input, 60, 24, true, 1, 0, toast_rows(&app), 0, 0, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        // The toast sits on the strip's last row — directly above the box's top
        // rule, below the status line.
        let top_rule = (0..h)
            .find(|&y| row(&buf, y, 60).chars().all(|c| c == '─'))
            .expect("the box has a top rule");
        assert!(top_rule >= 1);
        assert!(
            row(&buf, top_rule - 1, 60).contains("/resume is disabled"),
            "the toast is the row just above the box: {:?}",
            row(&buf, top_rule - 1, 60)
        );
        let all: String = (0..h).map(|y| row(&buf, y, 60)).collect();
        assert!(all.contains("Working…"), "the status still shows above it");
    }

    #[test]
    fn toast_line_colors_info_dim_and_error_red() {
        let mut app = App::new();
        app.show_toast("ok", ToastKind::Info);
        assert_eq!(toast_line(&app, 40).spans[1].style.fg, Some(TOAST_COLOR));
        app.show_toast("bad", ToastKind::Error);
        assert_eq!(
            toast_line(&app, 40).spans[1].style.fg,
            Some(TOAST_ERROR_COLOR)
        );
    }

    #[test]
    fn toast_line_truncates_with_an_ellipsis_when_narrow() {
        let mut app = App::new();
        app.show_toast(
            "a very long toast message that overflows the width",
            ToastKind::Info,
        );
        let text = plain(&toast_line(&app, 12));
        assert!(text.ends_with('…'), "truncated: {text:?}");
        assert!(cols(&text) <= 12, "fits the width: {text:?}");
    }

    #[test]
    fn the_palette_replaces_the_footer() {
        let mut app = palette("/", 0);
        app.set_session_info("dummy_model_name", "~/alter-zero");
        let band = menu_rows(&app);
        let h = live_height(
            &app.input,
            60,
            24,
            false,
            0,
            0,
            0,
            band,
            footer_rows(&app, band),
            0,
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
        let bare_h = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0);
        let bare = cursor_position(Rect::new(0, 0, 40, bare_h), &app);
        app.set_session_info("dummy_model_name", "~/alter-zero");
        let footer_h = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 1, 0);
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
                        let band = band_rows(app);
                        let lh = live_height(
                            &app.input,
                            w,
                            h,
                            strip_has_status(app),
                            preview_rows(app, w),
                            queued_rows(app, w),
                            0,
                            band,
                            footer_rows(app, band),
                            0,
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
                            let mut render = StreamRender::new();
                            let _ = render.commit(text, w);
                            let _ = render.finish(text, w);
                        }
                        // mirror the Ctrl+O overlay
                        let screen = Rect::new(0, 0, w, h);
                        let mut overlay = Buffer::empty(screen);
                        render_tool_view(screen, &mut overlay, app, &transcript_lines(app, w));
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
        app.set_session_info("dummy_model_name", "~/alter-zero");
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
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
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
        let area = Rect::new(0, 0, 60, h);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, h - 1, "on the footer row, not in the textarea");
        let expected = cols(FOOTER_INDENT) + cols(SEARCH_PROMPT) + cols("git");
        assert_eq!(x as usize, expected);
    }

    #[test]
    fn the_search_cursor_clamps_inside_a_narrow_terminal() {
        let app = searching(&["git status"], "a very very long query indeed");
        let h = live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 1, 0);
        let area = Rect::new(0, 0, 20, h);
        let (x, _) = cursor_position(area, &app);
        assert!(x < 20, "clamped inside the width (codex clamps the same)");
    }

    #[test]
    fn the_previewed_match_highlights_the_query_reversed() {
        let app = searching(&["git status"], "stat");
        assert_eq!(app.input.text(), "git status", "the match previews");
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
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
        let texts: Vec<String> = shortcuts_lines(false, false).iter().map(plain).collect();
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
        app.set_session_info("dummy_model_name", "~/alter-zero");
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
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
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
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
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
        let area = Rect::new(0, 0, 60, h);
        let (_, y) = cursor_position(area, &app);
        assert!(y < h - 1, "cursor is in the box, not on the footer row");
    }

    #[test]
    fn the_shortcuts_band_lists_the_bang() {
        let texts: Vec<String> = shortcuts_lines(false, false).iter().map(plain).collect();
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
        let mut t = tool("pwd", "", ToolStatus::Ok, "/home/user/alter-zero");
        t.shell = true;
        let lines = tool_lines(&t, 60);
        assert_eq!(plain(&lines[0]), "  ⎿  /home/user/alter-zero");
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
        assert_eq!(plain(&lines[0]), "  ⎿  Running…", "the mock's running cell");
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
            vec!["  ⎿  index.html", "     script.js", "     styles.css"],
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
        assert_eq!(lines[0], "  ⎿  1");
        assert_eq!(lines[1], "     2", "continuation aligned, no corner");
        let hidden = 6 - TOOL_PEEK_LINES;
        assert_eq!(
            lines[TOOL_PEEK_LINES],
            format!("     … +{hidden} lines (ctrl+o to expand)")
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
        assert_eq!(lines[0], "  ⎿  /home/me");
        assert_eq!(lines[1], "     ├── a");
        assert_eq!(lines[2], "     ├── b");
        assert_eq!(
            lines[3], "     …",
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
            vec!["  ⎿  a", "     b"],
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
                images: Vec::new(),
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
        assert!(texts.contains(&"  ⎿  a".to_string()), "{texts:?}");
        assert!(
            texts.contains(&"     f".to_string()),
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
                images: Vec::new(),
            }),
            HistoryItem::Tool(t),
        ];
        let texts: Vec<String> = conversation_lines(&history, 40)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts, vec!["! pwd", "  ⎿  /home", ""]);
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
                images: Vec::new(),
            }),
            HistoryItem::Tool(t),
        ];
        assert_eq!(transcript_body(&app, 40), vec!["! pwd", "  ⎿  /home", ""]);
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
            "  ⎿  Running…",
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
        app.set_status_times(Duration::from_secs(5), None);
        let q = queued_rows(&app, 60);
        // A shell turn hides the status line (has_status false), so the strip is
        // preview + gap only — sized exactly as main.rs::draw does.
        let h = live_height(
            &app.input,
            60,
            24,
            strip_has_status(&app),
            preview_rows(&app, 60),
            q,
            0,
            0,
            footer_rows(&app, 0),
            0,
        );
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert_eq!(
            row(&buf, 0, 60).trim_end(),
            "  ⎿  Running… (5s)",
            "the strip preview is the cell's running peek with its elapsed, flush \
             under the committed `! sleep 5` header just above the live region"
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
        let h = live_height(&app.input, 40, 24, false, 0, 0, 0, band, 0, 0);
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

    // --- Esc-Esc backtrack rendering (docs/backtrack.md) ---

    /// An app holding two finished exchanges, ready to preview.
    fn backtrack_app() -> App {
        let mut app = App::new();
        for (user, reply) in [("first", "a"), ("second", "b")] {
            app.record_user_message(user);
            app.begin_stream();
            app.push_chunk(reply);
            app.finish_stream();
            app.end_turn(1);
        }
        app
    }

    #[test]
    fn transcript_reverses_the_selected_user_message() {
        let mut app = backtrack_app();
        app.backtrack.selected = Some(0);
        let lines = transcript_lines(&app, 40);
        let range = transcript_selection(&app, 40).expect("a selection range");
        assert!(
            plain(&lines[range.start]).contains("first"),
            "the range points at the selected message"
        );
        for (i, line) in lines.iter().enumerate() {
            let reversed = line.style.add_modifier.contains(Modifier::REVERSED);
            assert_eq!(
                reversed,
                range.contains(&i),
                "only the highlighted rows are reversed (row {i})"
            );
        }
    }

    #[test]
    fn transcript_selection_follows_the_stepped_ordinal() {
        let mut app = backtrack_app();
        app.backtrack.selected = Some(1);
        let lines = transcript_lines(&app, 40);
        let range = transcript_selection(&app, 40).expect("a selection range");
        assert!(plain(&lines[range.start]).contains("second"));
    }

    #[test]
    fn transcript_without_a_selection_reverses_nothing() {
        let app = backtrack_app();
        assert!(transcript_selection(&app, 40).is_none());
        for line in transcript_lines(&app, 40) {
            assert!(!line.style.add_modifier.contains(Modifier::REVERSED));
        }
    }

    #[test]
    fn scroll_into_view_moves_only_when_the_target_is_off_screen() {
        // Above the window: scroll up to its top. Below: down just enough.
        // Visible: stay put. Taller than the window: show its top.
        assert_eq!(scroll_into_view(10, &(2..4), 5), 2, "above → its top");
        assert_eq!(scroll_into_view(0, &(8..10), 5), 5, "below → just enough");
        assert_eq!(scroll_into_view(2, &(3..6), 5), 2, "visible → unchanged");
        assert_eq!(scroll_into_view(0, &(4..20), 5), 4, "tall → its top");
    }

    #[test]
    fn backtrack_scroll_targets_the_highlight() {
        let mut app = backtrack_app();
        app.backtrack.selected = Some(0);
        app.tool_scroll = 50; // scrolled far past the first message
        let scroll = backtrack_scroll(&app, 40, 24).expect("a scroll decision");
        let range = transcript_selection(&app, 40).unwrap();
        assert_eq!(scroll, range.start, "scrolls back up to the highlight");
        assert!(
            backtrack_scroll(&App::new(), 40, 24).is_none(),
            "no selection, no decision"
        );
    }

    #[test]
    fn tool_view_hints_swap_while_previewing() {
        let mut app = backtrack_app();
        let mut buf = buffer(80, 16);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        let idle: String = (0..16).map(|y| row(&buf, y, 80)).collect();
        assert!(idle.contains("q/esc/ctrl+o to quit"), "normal pager hints");

        app.backtrack.selected = Some(1);
        let mut buf = buffer(80, 16);
        let tv_lines = transcript_lines(&app, buf.area.width);
        render_tool_view(buf.area, &mut buf, &app, &tv_lines);
        let preview: String = (0..16)
            .map(|y| row(&buf, y, 80))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            preview.contains("enter to edit message"),
            "backtrack hints while previewing: {preview:?}"
        );
        assert!(
            !preview.contains("q/esc/ctrl+o to quit"),
            "the quit hint made way: {preview:?}"
        );
    }

    #[test]
    fn footer_rows_reserves_the_slot_while_primed() {
        // Like the Ctrl+R search line, the hint shows even with no session
        // info injected — the slot exists whenever the gesture is armed.
        let mut app = App::new();
        assert_eq!(footer_rows(&app, 0), 0);
        app.backtrack.primed = true;
        assert_eq!(footer_rows(&app, 0), 1);
    }

    #[test]
    fn backtrack_hint_line_names_the_second_esc() {
        let line = backtrack_hint_line();
        assert_eq!(
            plain(&line),
            format!("{FOOTER_INDENT}esc again to edit previous message"),
        );
        // Spans: indent, the bold-cyan key, the dim label (the search-hint
        // styling).
        assert_eq!(line.spans[1].style.fg, Some(SEARCH_QUERY_COLOR));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[2].style.fg, Some(FOOTER_COLOR));
    }

    #[test]
    fn render_live_paints_the_hint_in_the_footer_slot() {
        let mut app = backtrack_app();
        app.set_session_info("model", "~/repo");
        app.backtrack.primed = true;
        let footer = footer_rows(&app, 0);
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, footer, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(
            last.contains("esc again to edit previous message"),
            "the hint takes the footer slot: {last:?}"
        );
        assert!(!last.contains("model"), "the session footer made way");
    }

    #[test]
    fn shortcuts_esc_entry_is_three_way_context_sensitive() {
        // Interrupt while a turn runs (as before); the Esc-Esc edit hint when
        // idle with a previous user message; quit only with nothing to edit.
        let idle: String = shortcuts_lines(false, false).iter().map(plain).collect();
        assert!(idle.contains("esc to quit"), "{idle:?}");
        let busy: String = shortcuts_lines(true, true).iter().map(plain).collect();
        assert!(busy.contains("esc to interrupt"), "{busy:?}");
        let target: String = shortcuts_lines(false, true).iter().map(plain).collect();
        assert!(target.contains("esc esc to edit previous"), "{target:?}");
        assert!(!target.contains("esc to quit"), "{target:?}");
    }

    // ===== /resume session picker (docs/resume.md) =====

    /// An app with the picker open over one session per preview, all aged
    /// `5m ago` (updated) / `2h ago` (created), recorded in the picker's own
    /// cwd, at paths `s0`, `s1`, ….
    fn resume_app(previews: &[&str]) -> App {
        let mut app = App::new();
        app.open_resume_picker(
            previews
                .iter()
                .enumerate()
                .map(|(i, preview)| crate::session::SessionSummary {
                    path: std::path::PathBuf::from(format!("s{i}")),
                    updated_secs: 300,
                    created_secs: 7_200,
                    cwd: "/repo".into(),
                    preview: (*preview).into(),
                })
                .collect(),
            "/repo".into(),
        );
        app
    }

    #[test]
    fn resume_picker_titles_with_the_slash_tiled_resume_header() {
        let app = resume_app(&["hello"]);
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let title = row(&buf, 0, 40);
        assert!(title.starts_with("/ R E S U M E"), "{title:?}");
        // The tiling continues to the right edge (the transcript pager's
        // slash-tiled header pattern).
        assert!(title.trim_end().ends_with('/'), "{title:?}");
    }

    #[test]
    fn resume_picker_shows_the_search_placeholder_then_the_query_echo() {
        let mut app = resume_app(&["hello"]);
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(row(&buf, 2, 40).contains("Type to search"));
        app.resume_picker.as_mut().unwrap().query = "wrap".into();
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(row(&buf, 2, 40).contains("Search: wrap"));
    }

    #[test]
    fn resume_rows_show_marker_age_and_preview_with_the_selection_lit() {
        let mut app = resume_app(&["first message", "second message"]);
        app.resume_picker.as_mut().unwrap().selected = 1;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let first = row(&buf, 4, 40);
        let second = row(&buf, 5, 40);
        assert!(first.starts_with("  5m ago"), "{first:?}");
        assert!(first.contains("first message"), "{first:?}");
        assert!(second.starts_with("❯ 5m ago"), "{second:?}");
        assert!(second.contains("second message"), "{second:?}");
        // The age column pads to a fixed width (codex's dense 12-col date) —
        // measured on the ASCII-marker row (`find` is byte-indexed; `❯` is
        // multi-byte).
        assert_eq!(first.find("first message"), Some(2 + RESUME_AGE_WIDTH));
        // The whole selected row lights up; the others dim — the palette's
        // selection-by-colour convention.
        assert_eq!(buf[(0, 5)].fg, MENU_SELECTED_COLOR);
        assert_eq!(buf[(4, 5)].fg, MENU_SELECTED_COLOR, "age too");
        assert_eq!(buf[(4, 4)].fg, MENU_DIM_COLOR, "unselected rows dim");
    }

    #[test]
    fn resume_picker_counts_the_selection_in_the_separator() {
        let mut app = resume_app(&["one", "two", "three"]);
        app.resume_picker.as_mut().unwrap().selected = 1;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        // Bottom chrome: separator + hints + blank ⇒ separator at h-3.
        let sep = row(&buf, 9, 40);
        assert!(sep.contains("─"), "{sep:?}");
        assert!(sep.contains(" 2/3 "), "{sep:?}");
        assert!(row(&buf, 10, 40).contains("enter resume"));
    }

    #[test]
    fn resume_picker_shows_no_sessions_yet_when_nothing_is_saved() {
        let app = resume_app(&[]);
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(row(&buf, 4, 40).contains("No sessions yet"));
    }

    #[test]
    fn resume_picker_shows_no_results_for_a_query_matching_nothing() {
        let mut app = resume_app(&["hello"]);
        app.resume_picker.as_mut().unwrap().query = "zzz".into();
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(row(&buf, 4, 40).contains("No results for your search"));
        let sep = row(&buf, 9, 40);
        assert!(
            !sep.contains('/') || !sep.contains("1/"),
            "no count: {sep:?}"
        );
    }

    #[test]
    fn resume_rows_truncate_to_the_width() {
        let app = resume_app(&["a very long preview that cannot possibly fit"]);
        let mut buf = buffer(24, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let line = row(&buf, 4, 24);
        assert!(line.starts_with("❯ 5m ago"), "{line:?}");
        assert!(!line.contains("possibly"), "truncated: {line:?}");
    }

    #[test]
    fn resume_search_row_carries_the_filter_sort_toolbar_right_aligned() {
        let app = resume_app(&["hello"]);
        let mut buf = buffer(80, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let search = row(&buf, 2, 80);
        assert!(search.contains("Type to search"), "{search:?}");
        // Codex's toolbar: active values bracketed, both tab pairs shown.
        assert!(search.contains("Filter: [Cwd] All"), "{search:?}");
        assert!(search.contains("Sort: [Updated] Created"), "{search:?}");
        assert!(search.trim_end().ends_with("Created"), "right-aligned");
    }

    #[test]
    fn resume_toolbar_brackets_follow_the_toggles() {
        let mut app = resume_app(&["hello"]);
        {
            let picker = app.resume_picker.as_mut().unwrap();
            picker.filter = crate::app::ResumeFilter::All;
            picker.sort = crate::app::ResumeSort::Created;
        }
        let mut buf = buffer(80, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let search = row(&buf, 2, 80);
        assert!(search.contains("Filter:  Cwd [All]"), "{search:?}");
        assert!(search.contains("Sort:  Updated [Created]"), "{search:?}");
    }

    #[test]
    fn resume_toolbar_compacts_to_the_active_values_when_narrow() {
        let app = resume_app(&["hello"]);
        let mut buf = buffer(50, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let search = row(&buf, 2, 50);
        // Codex's compact form: label + active value only.
        assert!(search.contains("Filter:[Cwd]"), "{search:?}");
        assert!(search.contains("Sort:[Updated]"), "{search:?}");
        // Too narrow for even the compact form: the toolbar drops, the
        // search line stays.
        let mut buf = buffer(30, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let search = row(&buf, 2, 30);
        assert!(search.contains("Type to search"), "{search:?}");
        assert!(!search.contains("Filter:"), "{search:?}");
    }

    #[test]
    fn resume_selected_row_gets_a_full_width_background_tint() {
        let mut app = resume_app(&["first message", "second message"]);
        app.resume_picker.as_mut().unwrap().selected = 1;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        // The tint spans the whole selected row — marker cell through the
        // padding past the text (codex's full-width background blend)…
        assert_eq!(buf[(0, 5)].bg, RESUME_SELECTED_BG);
        assert_eq!(buf[(20, 5)].bg, RESUME_SELECTED_BG);
        assert_eq!(buf[(39, 5)].bg, RESUME_SELECTED_BG);
        // …and the unselected row keeps the plain background.
        assert_ne!(buf[(0, 4)].bg, RESUME_SELECTED_BG);
    }

    #[test]
    fn resume_rows_show_the_age_of_the_active_sort_key() {
        // Codex shows only the active sort key's timestamp per row: the
        // fixture's rows are updated 5m ago but created 2h ago.
        let mut app = resume_app(&["hello"]);
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(
            row(&buf, 4, 40).contains("5m ago"),
            "Updated sort: mtime age"
        );
        app.resume_picker.as_mut().unwrap().sort = crate::app::ResumeSort::Created;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        assert!(
            row(&buf, 4, 40).contains("2h ago"),
            "Created sort: start age"
        );
    }

    #[test]
    fn resume_picker_scrolls_to_keep_the_selection_visible() {
        let previews: Vec<String> = (0..30).map(|i| format!("message number {i}")).collect();
        let refs: Vec<&str> = previews.iter().map(String::as_str).collect();
        let mut app = resume_app(&refs);
        app.resume_picker.as_mut().unwrap().selected = 29;
        let mut buf = buffer(40, 12);
        render_resume_picker(buf.area, &mut buf, &app);
        let body: String = (4..9).map(|y| row(&buf, y, 40)).collect();
        assert!(
            body.contains("message number 29"),
            "the window follows the selection: {body:?}"
        );
    }

    // ===== inline /model picker (docs/llm.md) =====

    fn model_entry(id: &str, provider: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            provider: provider.into(),
            display_name: name.into(),
            reasoning: None,
            vision: None,
            context: None,
        }
    }

    /// A ready picker over the given models, with `selected`/`active` set.
    fn model_picker(models: Vec<ModelEntry>, selected: usize, active: &str) -> ModelPicker {
        ModelPicker {
            models,
            status: ModelLoad::Ready,
            selected,
            query: String::new(),
            active_id: active.into(),
            ..ModelPicker::default()
        }
    }

    fn three_models() -> Vec<ModelEntry> {
        vec![
            model_entry(
                "anthropic/claude-3-haiku",
                "openrouter",
                "Anthropic: Claude 3 Haiku",
            ),
            model_entry(
                "anthropic/claude-fable-5",
                "openrouter",
                "Anthropic: Claude Fable 5",
            ),
            model_entry(
                "moonshotai/kimi-k2.6",
                "openrouter",
                "MoonshotAI: Kimi K2.6",
            ),
        ]
    }

    #[test]
    fn model_picker_frames_with_rules_and_no_header() {
        let picker = model_picker(three_models(), 0, "anthropic/claude-3-haiku");
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        // Top rule, then a blank gap where the old "Showing models…" banner was.
        assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
        assert!(row(&buf, 1, 60).trim().is_empty(), "no header banner");
        // The search line moved up to row 2.
        assert!(row(&buf, MODEL_SEARCH_ROW, 60).contains('❯'), "search line");
    }

    #[test]
    fn model_picker_shows_the_search_prompt_and_query() {
        let mut picker = model_picker(three_models(), 0, "x");
        picker.query = "haiku".into();
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        let search = row(&buf, MODEL_SEARCH_ROW, 60);
        assert!(search.contains("❯ haiku"), "{search:?}");
        // The `❯` prompt is cyan.
        assert_eq!(buf[(2, MODEL_SEARCH_ROW)].fg, MODEL_SELECTED_COLOR);
    }

    #[test]
    fn model_rows_show_marker_provider_tag_and_active_check() {
        // Selected = row 0, active = row 1 (claude-fable-5).
        let picker = model_picker(three_models(), 0, "anthropic/claude-fable-5");
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        // First list row (y = MODEL_SEARCH_ROW + 2 = 4).
        let first = row(&buf, 4, 60);
        assert!(first.starts_with("→ anthropic/claude-3-haiku"), "{first:?}");
        assert!(first.contains("[openrouter]"), "provider tag: {first:?}");
        // The selected marker is cyan.
        assert_eq!(buf[(0, 4)].fg, MODEL_SELECTED_COLOR);
        // The active model (row 1, y=5) carries the ✓.
        let second = row(&buf, 5, 60);
        assert!(second.contains('✓'), "active model has a check: {second:?}");
    }

    #[test]
    fn model_picker_counter_and_name_reflect_the_selection() {
        let picker = model_picker(three_models(), 2, "x");
        // Size the buffer to the picker's natural height (9 chrome + 3 list),
        // like the boundary does — otherwise the Min(0) list would expand and
        // push the counter/name rows down.
        let mut buf = buffer(60, 12);
        render_model_picker(buf.area, &mut buf, &picker);
        // Counter row = top(0) gap(1) search(2) gap(3) list(4,5,6) → 7.
        let counter = row(&buf, 7, 60);
        assert!(counter.contains("(3/3)"), "{counter:?}");
        // Model-name row = counter(7) + gap(8) + 1 = 9.
        let name = row(&buf, 9, 60);
        assert!(
            name.contains("Model Name: MoonshotAI: Kimi K2.6"),
            "{name:?}"
        );
        // A trailing blank gap (the user's mock), then the bottom rule last.
        assert!(row(&buf, 10, 60).trim().is_empty(), "trailing gap");
        assert!(row(&buf, 11, 60).starts_with('─'), "bottom rule");
    }

    #[test]
    fn model_list_keeps_the_selection_centered_not_pinned_to_an_edge() {
        // A long list (30 models) with a deep-interior selection (15) that has
        // plenty of room on both sides — the case the old bottom-anchored window
        // got wrong (it pinned the highlight to the last visible row).
        let models: Vec<ModelEntry> = (0..30)
            .map(|i| {
                model_entry(
                    &format!("openrouter/model-{i:02}"),
                    "openrouter",
                    &format!("Model {i}"),
                )
            })
            .collect();
        let picker = model_picker(models, 15, "x");
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        // The list spans rows 4..14 — top(0) gap(1) search(2) gap(3) then 10 rows.
        let list: Vec<String> = (4..14).map(|y| row(&buf, y, 60)).collect();
        // The highlight lands on the middle row of the window (max/2), carrying
        // the `→` marker — centered, not jammed against the bottom edge.
        let middle = &list[MODEL_MENU_MAX_ROWS as usize / 2];
        assert!(
            middle.contains("model-15") && middle.contains('→'),
            "selection sits centered: {middle:?}"
        );
        let joined = list.join("\n");
        // Models both above *and* below the selection are on screen — the broad
        // view the fix restores.
        assert!(joined.contains("model-11"), "rows above show: {joined:?}");
        assert!(
            joined.contains("model-19"),
            "rows below show (the old window hid these): {joined:?}"
        );
    }

    #[test]
    fn counter_notes_more_providers_still_loading() {
        // A partial list (one provider in, another still fetching) shows the
        // list now with a dim "loading more…" hint beside the counter.
        let mut picker = model_picker(three_models(), 0, "x");
        picker.pending = 1;
        let mut buf = buffer(60, 12);
        render_model_picker(buf.area, &mut buf, &picker);
        let counter = row(&buf, 7, 60);
        assert!(counter.contains("(1/3)"), "{counter:?}");
        assert!(counter.contains("loading more"), "{counter:?}");
    }

    #[test]
    fn counter_notes_a_provider_that_failed() {
        let mut picker = model_picker(three_models(), 0, "x");
        picker.errors.push(ModelFetchError {
            provider: "Agent Zero API".into(),
            message: "HTTP 401".into(),
        });
        let mut buf = buffer(60, 12);
        render_model_picker(buf.area, &mut buf, &picker);
        let counter = row(&buf, 7, 60);
        assert!(counter.contains("Agent Zero API"), "{counter:?}");
        assert!(counter.contains("unavailable"), "{counter:?}");
    }

    #[test]
    fn all_failed_picker_shows_one_red_row_per_provider() {
        let mut picker = model_picker(vec![], 0, "x");
        picker.status = ModelLoad::Error(String::new());
        picker.errors = vec![
            ModelFetchError {
                provider: "OpenRouter".into(),
                message: "HTTP 500".into(),
            },
            ModelFetchError {
                provider: "Agent Zero API".into(),
                message: "HTTP 401".into(),
            },
        ];
        // Natural height: collapsed chrome (6) + 2 error rows = 8.
        let mut buf = buffer(60, 8);
        render_model_picker(buf.area, &mut buf, &picker);
        let r0 = row(&buf, 4, 60);
        let r1 = row(&buf, 5, 60);
        assert!(r0.contains("OpenRouter"), "{r0:?}");
        assert!(r0.contains("HTTP 500"), "{r0:?}");
        assert!(r1.contains("Agent Zero API"), "{r1:?}");
        assert_eq!(buf[(2, 4)].fg, ERROR_COLOR, "error rows are red");
    }

    #[test]
    fn all_failed_picker_height_covers_each_error_row() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.begin_model_load(2);
        app.add_model_error("OpenRouter", "HTTP 500");
        app.add_model_error("Agent Zero API", "HTTP 401");
        // Collapsed chrome (6) + 2 error rows = 8.
        assert_eq!(model_picker_height(&app, 40), Some(8));
    }

    #[test]
    fn model_picker_shows_a_loading_placeholder() {
        let picker = ModelPicker {
            active_id: "x".into(),
            ..ModelPicker::default()
        };
        assert_eq!(picker.status, ModelLoad::Loading);
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        let list = row(&buf, 4, 60);
        assert!(list.contains("Loading models…"), "{list:?}");
    }

    #[test]
    fn model_picker_placeholder_collapses_to_a_single_trailing_gap() {
        // A placeholder state (loading / error / no match) has no counter or
        // model-name to show, so those detail rows collapse: exactly one blank
        // gap sits between the placeholder and the bottom rule — not the four
        // trailing blanks the counter/gap/name/gap layout leaves for a real
        // model. Size the buffer to the picker's natural height so the Min(0)
        // list can't expand into the gap.
        let picker = ModelPicker {
            active_id: "x".into(),
            ..ModelPicker::default()
        };
        assert_eq!(picker.status, ModelLoad::Loading);
        // 6 collapsed chrome rows + 1 placeholder list row = 7.
        let mut buf = buffer(60, 7);
        render_model_picker(buf.area, &mut buf, &picker);
        assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
        assert!(row(&buf, 4, 60).contains("Loading models…"), "list row");
        assert!(row(&buf, 5, 60).trim().is_empty(), "single trailing gap");
        assert!(row(&buf, 6, 60).starts_with('─'), "bottom rule");
    }

    #[test]
    fn model_picker_shows_an_error_placeholder_in_red() {
        let picker = ModelPicker {
            status: ModelLoad::Error("401 bad key".into()),
            active_id: "x".into(),
            ..ModelPicker::default()
        };
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        let list = row(&buf, 4, 60);
        assert!(list.contains("Error: 401 bad key"), "{list:?}");
        assert_eq!(buf[(2, 4)].fg, ERROR_COLOR);
    }

    #[test]
    fn model_picker_shows_no_match_when_the_query_filters_everything() {
        let mut picker = model_picker(three_models(), 0, "x");
        picker.query = "zzzz".into();
        let mut buf = buffer(60, 20);
        render_model_picker(buf.area, &mut buf, &picker);
        assert!(row(&buf, 4, 60).contains("No matching models"));
    }

    #[test]
    fn model_picker_height_covers_the_chrome_plus_list() {
        let mut app = App::new();
        app.open_model_picker("a");
        app.set_models(three_models());
        // 9 chrome rows + 3 list rows.
        assert_eq!(model_picker_height(&app, 40), Some(12));
        // Clamped to the terminal height.
        assert_eq!(model_picker_height(&app, 8), Some(8));
        // A placeholder state (still loading — no models) drops the counter +
        // name detail rows: 6 collapsed chrome + 1 placeholder row = 7.
        app.open_model_picker("a");
        assert_eq!(model_picker_height(&app, 40), Some(7));
        // None when the picker is closed.
        app.close_model_picker();
        assert_eq!(model_picker_height(&app, 40), None);
    }

    #[test]
    fn render_live_shows_the_model_picker_when_open() {
        let mut app = App::new();
        app.open_model_picker("anthropic/claude-3-haiku");
        app.set_models(three_models());
        let mut buf = buffer(60, 14);
        render_live(buf.area, &mut buf, &app);
        // The picker stands in for the composer: top rule, then the `❯` search
        // line (headerless), then the model rows.
        assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
        assert!(row(&buf, MODEL_SEARCH_ROW, 60).contains('❯'), "search line");
        assert!(
            row(&buf, 4, 60).contains("anthropic/claude-3-haiku"),
            "a model row"
        );
    }

    #[test]
    fn cursor_sits_at_the_end_of_the_model_search_query() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.set_models(three_models());
        app.model_picker.as_mut().unwrap().query = "hai".into();
        let area = Rect::new(0, 0, 60, 14);
        let (x, y) = cursor_position(area, &app);
        // indent(2) + prompt("❯ " = 2) + "hai"(3) = 7.
        assert_eq!((x, y), (7, MODEL_SEARCH_ROW));
    }

    // --- The inline `/login` onboarding flow (docs/llm.md). ---

    fn login_choices() -> Vec<ProviderChoice> {
        vec![
            ProviderChoice {
                id: "openrouter".into(),
                name: "OpenRouter".into(),
                env_var: "OPENROUTER_API_KEY".into(),
                configured: true,
            },
            ProviderChoice {
                id: "together".into(),
                name: "Together AI".into(),
                env_var: "TOGETHER_API_KEY".into(),
                configured: false,
            },
        ]
    }

    fn login_app_provider() -> App {
        let mut app = App::new();
        app.open_key_onboarding(login_choices(), "~/.alter-zero/.env");
        app
    }

    fn login_app_key() -> App {
        let mut app = login_app_provider();
        // Advance to the masked key step for the highlighted provider (index 0).
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Key;
        onboarding.chosen = Some(0);
        app
    }

    #[test]
    fn render_login_provider_step_frames_lists_and_marks_configured() {
        let app = login_app_provider();
        let onboarding = app.key_onboarding.as_ref().unwrap();
        // Natural height: 9 chrome + 2 provider rows = 11.
        let mut buf = buffer(60, 11);
        render_key_onboarding(buf.area, &mut buf, onboarding);
        assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
        // Headerless (like /model): row 1 is a blank gap, not a banner.
        assert!(row(&buf, 1, 60).trim().is_empty(), "no header banner");
        // The `❯` filter line.
        assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'));
        // First provider row (y = LOGIN_SEARCH_ROW + 2 = 4): selected →, env tag,
        // and a green ✓ because OpenRouter is configured.
        let first = row(&buf, 4, 60);
        assert!(first.starts_with("→ OpenRouter"), "{first:?}");
        assert!(first.contains("[OPENROUTER_API_KEY]"), "{first:?}");
        assert!(
            first.contains('✓'),
            "configured provider has a check: {first:?}"
        );
        // Together AI (row 1, y=5) is not configured → no check.
        assert!(!row(&buf, 5, 60).contains('✓'));
        // The hint (row 8: counter(6) gap(7) hint(8)) names the real .env path.
        assert!(
            row(&buf, 8, 60).contains("Keys are saved to ~/.alter-zero/.env"),
            "hint: {:?}",
            row(&buf, 8, 60)
        );
    }

    #[test]
    fn render_login_key_step_masks_the_entered_key() {
        let mut app = login_app_key();
        // Chose the highlighted provider (OpenRouter) and typed a key.
        app.key_onboarding.as_mut().unwrap().key_input = "sk-secret-1234".into();
        let onboarding = app.key_onboarding.as_ref().unwrap();
        assert_eq!(onboarding.step, KeyStep::Key);
        let mut buf = buffer(60, LOGIN_KEY_ROWS);
        render_key_onboarding(buf.area, &mut buf, onboarding);
        // Periwinkle prompt (row 2, after the top rule + gap) names the provider.
        assert!(row(&buf, 2, 60).contains("Enter your OpenRouter API key"));
        assert_eq!(
            buf[(2, 2)].fg,
            LOGIN_KEY_PROMPT_COLOR,
            "prompt is periwinkle"
        );
        // The field is masked: dots, never the plaintext key.
        let field = row(&buf, LOGIN_KEY_INPUT_ROW, 60);
        assert!(field.contains('•'), "masked: {field:?}");
        assert!(
            !field.contains("sk-secret"),
            "no plaintext leaks: {field:?}"
        );
    }

    #[test]
    fn login_key_prompt_avoids_a_doubled_api() {
        // A provider whose name already ends in "API" doesn't gain a second one.
        assert_eq!(
            login_key_prompt("Agent Zero API"),
            "Enter your Agent Zero API key"
        );
        // A normal name still gets the "API key" suffix.
        assert_eq!(
            login_key_prompt("OpenRouter"),
            "Enter your OpenRouter API key"
        );
    }

    #[test]
    fn render_login_key_step_shows_a_placeholder_when_empty() {
        let app = login_app_key();
        let onboarding = app.key_onboarding.as_ref().unwrap();
        let mut buf = buffer(60, LOGIN_KEY_ROWS);
        render_key_onboarding(buf.area, &mut buf, onboarding);
        assert!(row(&buf, LOGIN_KEY_INPUT_ROW, 60).contains("paste your API key"));
    }

    #[test]
    fn key_onboarding_height_covers_both_steps() {
        let mut app = login_app_provider();
        // Provider step: 9 chrome + 2 provider rows.
        assert_eq!(key_onboarding_height(&app, 40), Some(11));
        // Clamped to the terminal height.
        assert_eq!(key_onboarding_height(&app, 6), Some(6));
        // Key step: a fixed height.
        app.key_onboarding.as_mut().unwrap().step = KeyStep::Key;
        assert_eq!(key_onboarding_height(&app, 40), Some(LOGIN_KEY_ROWS));
        // None when closed.
        app.close_key_onboarding();
        assert_eq!(key_onboarding_height(&app, 40), None);
    }

    #[test]
    fn render_live_shows_the_onboarding_when_open() {
        let app = login_app_provider();
        let mut buf = buffer(60, 12);
        render_live(buf.area, &mut buf, &app);
        // The onboarding stands in for the composer: top rule, `❯` filter, and
        // the provider list (headerless — no banner on row 1).
        assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
        assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'), "filter line");
        assert!(row(&buf, 4, 60).contains("OpenRouter"), "a provider row");
    }

    #[test]
    fn cursor_tracks_the_login_filter_then_the_masked_key() {
        let mut app = login_app_provider();
        app.key_onboarding.as_mut().unwrap().query = "tog".into();
        let area = Rect::new(0, 0, 60, 12);
        let (x, y) = cursor_position(area, &app);
        // indent(2) + "❯ "(2) + "tog"(3) = 7 on the filter row.
        assert_eq!((x, y), (7, LOGIN_SEARCH_ROW));
        // Advance to the key step and type: the cursor tracks the mask length.
        {
            let onboarding = app.key_onboarding.as_mut().unwrap();
            onboarding.step = KeyStep::Key;
            onboarding.chosen = Some(0);
            onboarding.key_input = "abcd".into();
        }
        let (x, y) = cursor_position(area, &app);
        // indent(2) + "❯ "(2) + 4 mask glyphs = 8 on the key row.
        assert_eq!((x, y), (8, LOGIN_KEY_INPUT_ROW));
    }

    #[test]
    fn model_picker_shows_a_login_hint_when_no_provider_is_configured() {
        let mut app = App::new();
        app.open_model_picker("x");
        app.set_models_needs_login();
        let picker = app.model_picker.as_ref().unwrap();
        let mut buf = buffer(60, 12);
        render_model_picker(buf.area, &mut buf, picker);
        // The list area (row 4) points at /login, in cyan (not a red error).
        let list = row(&buf, 4, 60);
        assert!(list.contains("run /login"), "{list:?}");
        assert_eq!(buf[(2, 4)].fg, MODEL_SELECTED_COLOR);
        // No counter or model-name line when there's nothing selectable.
        assert!(row(&buf, 7, 60).trim().is_empty(), "no counter");
        assert!(row(&buf, 9, 60).trim().is_empty(), "no model name");
    }

    // ===== audited-defect regressions (2026-07 review) =====

    #[test]
    fn truncate_cols_measures_graphemes_not_chars() {
        // ❤️ (U+2764 U+FE0F) paints 2 columns (str-level width, what ratatui
        // uses); summing per-char widths counted 1 and let truncations
        // overflow their budget 2x.
        let hearts = "❤️".repeat(4); // 8 display columns
        assert_eq!(cols(&hearts), 8);
        let kept = truncate_cols(&hearts, 4);
        assert_eq!(cols(&kept), 4, "the kept prefix fits the budget as painted");
        assert_eq!(kept, "❤️".repeat(2));
    }

    #[test]
    fn truncate_cols_never_splits_a_zwj_cluster() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"; // 👨‍👩‍👧, 2 cols
        let s = format!("{family} ok");
        assert_eq!(
            truncate_cols(&s, 3),
            format!("{family} "),
            "the whole cluster (2 cols) fits a 3-col budget"
        );
        assert_eq!(
            truncate_cols(&s, 1),
            "",
            "a cluster wider than the budget is dropped whole, never split"
        );
    }

    #[test]
    fn live_height_saturates_instead_of_overflowing_u16() {
        // A recalled multi-megabyte paste (tens of thousands of wrapped rows)
        // plus the same text queued mid-turn used to overflow the u16 row sum
        // and panic in dev builds (overflow checks on).
        let input = TextArea::from_text(&"x".repeat(170_000));
        let h = live_height(&input, 10, 24, true, 0, 60_000, 0, 0, 1, 0);
        assert_eq!(h, 24, "clamped to the terminal height");
    }

    #[test]
    fn cursor_position_stays_in_the_live_region_when_the_box_collapses() {
        // A tall queue on a short terminal leaves the input frame no rows at
        // all: Rect::inner returns Rect::ZERO (origin discarded) and the
        // hardware cursor used to teleport to the screen's top-left, sitting
        // over the scrollback.
        let mut app = App::new();
        for i in 0..20 {
            let text = format!("q{i}");
            app.queued.push_back(batch(&[text.as_str()]));
        }
        let area = Rect::new(0, 5, 30, 15);
        let (_, y) = cursor_position(area, &app);
        assert!(
            y >= area.y,
            "the cursor stays inside the live region (y={y}, region starts at {})",
            area.y
        );
    }

    #[test]
    fn tool_peek_expands_tabs_for_display() {
        // A '\t' paints as zero cells (ratatui filters control-char
        // graphemes), gluing tab-separated fields together: `! printf
        // 'name\tsize'` showed "namesize". Tool output expands tabs on the
        // render path exactly like code blocks (expand_code_tabs); the
        // stored output stays byte-exact.
        let mut t = tool("pwd", "", ToolStatus::Ok, "name\tsize");
        t.shell = true;
        let texts: Vec<String> = tool_lines(&t, 80).iter().map(plain).collect();
        assert!(
            !texts.iter().any(|l| l.contains('\t')),
            "no raw tab reaches a painted row: {texts:?}"
        );
        let tab = " ".repeat(CODE_TAB_WIDTH);
        assert!(
            texts.iter().any(|l| l.contains(&format!("name{tab}size"))),
            "the separator survives as spaces: {texts:?}"
        );
    }

    #[test]
    fn tool_full_lines_expand_tabs_for_display() {
        let t = tool("bash", "cat Makefile", ToolStatus::Ok, "target:\n\tcc -o x");
        let texts: Vec<String> = tool_full_lines(&t, 80).iter().map(plain).collect();
        assert!(
            !texts.iter().any(|l| l.contains('\t')),
            "no raw tab reaches the expanded view: {texts:?}"
        );
        let tab = " ".repeat(CODE_TAB_WIDTH);
        assert!(
            texts.iter().any(|l| l.contains(&format!("{tab}cc -o x"))),
            "the recipe keeps its indentation: {texts:?}"
        );
    }

    // --- background shells (docs/background.md) ---

    fn bg_notice(code: Option<i32>, killed: bool) -> crate::app::BackgroundNotice {
        crate::app::BackgroundNotice {
            description: "Ping x.com 200 times".to_string(),
            id: "bash_1".to_string(),
            code,
            killed,
            output_tail: "tail".to_string(),
            timestamp: String::new(),
        }
    }

    #[test]
    fn a_backgrounded_tool_cell_shows_the_fixed_row_not_its_output() {
        // The stored output is the model-facing launch text — the cell (inline
        // and expanded alike) shows the fixed backgrounded row instead, under
        // a green header bullet (the launch succeeded).
        let tool = ToolCall {
            name: "Bash".to_string(),
            args: "ping -c 50 google.com".to_string(),
            status: ToolStatus::Backgrounded,
            output: "Command running in background with ID: bash_1.".to_string(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
        };
        for lines in [tool_lines(&tool, 60), tool_full_lines(&tool, 60)] {
            let texts: Vec<String> = lines.iter().map(plain).collect();
            assert_eq!(texts.len(), 2, "header + the fixed row: {texts:?}");
            assert!(texts[0].contains("Bash(ping -c 50 google.com)"));
            assert_eq!(
                texts[1].trim(),
                "⎿  Running in the background (↓ to manage)"
            );
            assert!(
                !texts.join("\n").contains("Command running"),
                "the model-facing text never renders: {texts:?}"
            );
        }
        assert_eq!(
            tool_lines(&tool, 60)[0].spans[0].style.fg,
            Some(TOOL_OK_COLOR),
            "a backgrounded launch gets the green bullet"
        );
    }

    #[test]
    fn a_backgrounded_shell_cell_is_the_headerless_fixed_row() {
        let tool = ToolCall {
            name: "ping x.com".to_string(),
            args: String::new(),
            status: ToolStatus::Backgrounded,
            output: "[moved to background as task bash_1]".to_string(),
            timestamp: String::new(),
            shell: true,
            truncated: false,
        };
        let texts: Vec<String> = tool_lines(&tool, 60).iter().map(plain).collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(
            texts[0].trim(),
            "⎿  Running in the background (↓ to manage)"
        );
    }

    #[test]
    fn the_running_preview_hints_ctrl_b_but_the_committed_cell_does_not() {
        // The hint is live-only: the preview renderer appends it under the
        // running cell; the committed `tool_lines` never carry it. The hint
        // waits a few seconds (see below), so inject an elapsed past the delay.
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "ping google.com -c 50");
        app.set_command_elapsed(Some(Duration::from_secs(5)));
        let preview: Vec<String> = preview_tool_lines(&app, 60).iter().map(plain).collect();
        assert!(
            preview
                .iter()
                .any(|l| l.trim() == "(ctrl+b to run in background)"),
            "the running preview hints Ctrl+B: {preview:?}"
        );
        let committed: Vec<String> = tool_lines(app.current_tool().unwrap(), 60)
            .iter()
            .map(plain)
            .collect();
        assert!(
            !committed.iter().any(|l| l.contains("ctrl+b")),
            "the commit-path cell never carries the hint: {committed:?}"
        );
    }

    #[test]
    fn the_ctrl_b_hint_waits_a_few_seconds_before_showing() {
        // Like Claude Code: a command that finishes right away never shows the
        // Ctrl+B hint (it isn't needed) — the hint appears only once the
        // command has been running a few seconds. The boundary injects the
        // running command's own elapsed each frame (`set_command_elapsed`);
        // the preview gates the hint on it (docs/background.md).
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "ping google.com -c 50");
        let shows_hint = |app: &App| {
            preview_tool_lines(app, 60)
                .iter()
                .map(plain)
                .any(|l| l.trim() == TOOL_BACKGROUND_HINT)
        };

        // Freshly started — no elapsed injected yet: no hint.
        assert!(!shows_hint(&app), "no hint the instant the command starts");
        // Under the delay: still no hint (a fast command stays clean).
        app.set_command_elapsed(Some(TOOL_BACKGROUND_HINT_DELAY - Duration::from_millis(1)));
        assert!(
            !shows_hint(&app),
            "no hint before the command has run a few seconds"
        );
        // At/past the delay: the hint appears.
        app.set_command_elapsed(Some(TOOL_BACKGROUND_HINT_DELAY));
        assert!(
            shows_hint(&app),
            "the hint shows once the command has run a few seconds"
        );
    }

    #[test]
    fn a_running_shell_only_hints_ctrl_b_after_the_delay() {
        // The `!` shell run is the same: its own elapsed rides the status
        // (turn == command for a shell), so a quick `!` command never flashes
        // the hint, and a long one shows it after the delay.
        let mut app = App::new();
        app.begin_shell("sleep 30");
        let shows_hint = |app: &App| {
            preview_tool_lines(app, 60)
                .iter()
                .map(plain)
                .any(|l| l.trim() == TOOL_BACKGROUND_HINT)
        };
        app.set_command_elapsed(Some(Duration::from_secs(1)));
        assert!(!shows_hint(&app), "a fast `!` command shows no hint");
        app.set_command_elapsed(Some(Duration::from_secs(4)));
        assert!(
            shows_hint(&app),
            "a long `!` command hints Ctrl+B after the delay"
        );
    }

    #[test]
    fn a_waiting_batch_sibling_gets_no_ctrl_b_hint() {
        let mut app = App::new();
        app.begin_stream();
        app.start_tool_batch(&[
            crate::stream::ToolCallSummary {
                name: "Bash".to_string(),
                args: "a".to_string(),
            },
            crate::stream::ToolCallSummary {
                name: "Bash".to_string(),
                args: "b".to_string(),
            },
        ]);
        app.start_tool("Bash", "a");
        // Past the hint delay so the running call shows its hint — the point
        // here is that the `⎿ Waiting…` sibling still gets none.
        app.set_command_elapsed(Some(Duration::from_secs(5)));
        let preview: Vec<String> = preview_tool_lines(&app, 60).iter().map(plain).collect();
        let hints = preview.iter().filter(|l| l.contains("ctrl+b")).count();
        assert_eq!(hints, 1, "only the running call hints: {preview:?}");
    }

    #[test]
    fn summary_lines_append_the_still_running_shell_count() {
        let mut summary = TurnSummary {
            verb: "Done",
            secs: 22,
            timestamp: String::new(),
            shells: 3,
            tokens: 0,
            cached: 0,
        };
        assert_eq!(
            plain(&summary_lines(&summary, 80)[0]),
            "Done for 22s · 3 shells still running"
        );
        summary.shells = 1;
        assert_eq!(
            plain(&summary_lines(&summary, 80)[0]),
            "Done for 22s · 1 shell still running",
            "singular for one shell"
        );
        summary.shells = 0;
        assert_eq!(plain(&summary_lines(&summary, 80)[0]), "Done for 22s");
    }

    #[test]
    fn the_footer_appends_the_running_shell_count() {
        let mut app = App::new();
        app.set_session_info("kimi-k2", "~/repo");
        assert_eq!(
            plain(&footer_line(&app, 80)).trim_end(),
            "  kimi-k2 · ~/repo"
        );
        app.bg_started("bash_1", "ping x.com", None, true);
        assert_eq!(
            plain(&footer_line(&app, 80)).trim_end(),
            "  kimi-k2 · ~/repo · 1 shell"
        );
        app.bg_started("bash_2", "ping y.com", None, true);
        assert_eq!(
            plain(&footer_line(&app, 80)).trim_end(),
            "  kimi-k2 · ~/repo · 2 shells"
        );
    }

    #[test]
    fn the_focused_footer_shell_count_lights_up_on_cyan() {
        let mut app = App::new();
        app.set_session_info("kimi-k2", "~/repo");
        app.bg_started("bash_1", "ping x.com", None, true);
        let shell_span = |line: &Line<'static>| {
            line.spans
                .iter()
                .find(|s| s.content.contains("shell"))
                .expect("the footer carries a shell segment")
                .clone()
        };
        let idle = shell_span(&footer_line(&app, 80));
        assert_eq!(idle.style.bg, None, "unfocused it stays dim like the rest");
        assert_eq!(idle.style.fg, Some(FOOTER_COLOR));
        // ↓ focuses the indicator: only that segment lights up — the model,
        // cwd and gauge segments keep their text and their dim styling.
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let focused = footer_line(&app, 80);
        assert_eq!(
            plain(&focused).trim_end(),
            "  kimi-k2 · ~/repo · 1 shell",
            "the other footer segments stay put"
        );
        let lit = shell_span(&focused);
        assert_eq!(lit.style.bg, Some(FOOTER_FOCUS_BG));
        assert_eq!(lit.style.fg, Some(FOOTER_FOCUS_FG));
        assert!(
            focused
                .spans
                .iter()
                .filter(|s| !s.content.contains("shell"))
                .all(|s| s.style.bg.is_none()),
            "nothing else is highlighted"
        );
    }

    #[test]
    fn render_live_paints_the_focused_count_on_the_footer_row() {
        // The painted cells, not just the styled spans: the tint must cover
        // exactly the `1 shell` columns on the live region's last row — the
        // ` · ` separator before it stays untinted, so the highlight hugs the
        // indicator instead of bleeding across the footer.
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new();
        app.set_session_info("kimi-k2", "~/repo");
        app.bg_started("bash_1", "ping x.com", None, true);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let last = row(&buf, h - 1, 60);
        assert!(last.starts_with("  kimi-k2 · ~/repo · 1 shell"), "{last:?}");
        // Column, not byte offset — the ` · ` separators are multi-byte.
        let byte = last.find("1 shell").expect("the count");
        let start = u16::try_from(cols(&last[..byte])).unwrap();
        for x in start..start + u16::try_from(cols("1 shell")).unwrap() {
            assert_eq!(buf[(x, h - 1)].bg, FOOTER_FOCUS_BG, "tinted at column {x}");
            assert_eq!(buf[(x, h - 1)].fg, FOOTER_FOCUS_FG, "ink at column {x}");
        }
        assert_ne!(
            buf[(start - 1, h - 1)].bg,
            FOOTER_FOCUS_BG,
            "the separator before the count stays clean"
        );
    }

    #[test]
    fn background_notice_lines_render_the_headline_with_outcome_colours() {
        let ok = background_notice_lines(&bg_notice(Some(0), false), 80);
        assert_eq!(
            plain(&ok[0]),
            "● Background command \"Ping x.com 200 times\" completed (exit code 0)"
        );
        assert_eq!(ok[0].spans[0].style.fg, Some(TOOL_OK_COLOR), "green bullet");
        let failed = background_notice_lines(&bg_notice(Some(2), false), 80);
        assert_eq!(
            failed[0].spans[0].style.fg,
            Some(TOOL_FAIL_COLOR),
            "red bullet on failure"
        );
        let stopped = background_notice_lines(&bg_notice(None, true), 80);
        assert!(plain(&stopped[0]).contains("was stopped by the user"));
        assert!(
            !plain(&ok[0]).contains("tail"),
            "the output tail is context-only, never rendered"
        );
    }

    #[test]
    fn conversation_and_transcript_walks_render_background_notices() {
        let mut app = App::new();
        app.history
            .push(HistoryItem::Background(bg_notice(Some(0), false)));
        let inline: Vec<String> = conversation_lines(&app.history, 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            inline.iter().any(|l| l.contains("completed (exit code 0)")),
            "the inline repaint shows the notice: {inline:?}"
        );
        let transcript: Vec<String> = transcript_lines(&app, 80).iter().map(plain).collect();
        assert!(
            transcript
                .iter()
                .any(|l| l.contains("completed (exit code 0)")),
            "the Ctrl+O transcript shows the notice: {transcript:?}"
        );
    }

    #[test]
    fn the_manager_list_shows_title_count_rows_and_hints() {
        let mut app = App::new();
        for (i, cmd) in [
            "ping -c 100 x.com",
            "ping -c 100 facebook.com",
            "ping -c 100 google.com",
        ]
        .iter()
        .enumerate()
        {
            app.bg_started(&format!("bash_{}", i + 1), cmd, None, true);
        }
        app.open_background_view();
        let texts: Vec<String> = background_view_lines(&app, 74)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts[0], "─".repeat(74), "top rule");
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "  Background");
        assert_eq!(texts[3], "  3 active shells");
        assert_eq!(texts[4], "");
        assert_eq!(texts[5], "  ❯ ping -c 100 x.com (running)");
        assert_eq!(texts[6], "    ping -c 100 facebook.com (running)");
        assert_eq!(texts[7], "    ping -c 100 google.com (running)");
        assert_eq!(texts[8], "");
        assert_eq!(
            texts[9],
            "  ↑/↓ to select · Enter to view · x to stop · Esc to close"
        );
        assert_eq!(texts[10], "");
        assert_eq!(texts[11], "─".repeat(74), "bottom rule");
        assert_eq!(texts.len(), 12);
        // The height helper reserves exactly the painted rows.
        assert_eq!(background_view_height(&app, 40), Some(12));
    }

    #[test]
    fn the_manager_empty_state_says_no_tasks_running() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping x.com", None, true);
        app.bg_exited("bash_1", Some(0), false);
        app.open_background_view();
        let texts: Vec<String> = background_view_lines(&app, 60)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts[2], "  Background");
        assert_eq!(texts[4], "  No tasks currently running");
        assert_eq!(texts[6], "  ↑/↓ to select · Enter to view · Esc to close");
        assert!(
            !texts.iter().any(|l| l.contains("x to stop")),
            "nothing to stop in the empty state: {texts:?}"
        );
    }

    #[test]
    fn the_manager_details_page_shows_fields_and_the_output_box() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping -c 120 x.com", None, true);
        for i in 1..=12 {
            app.bg_output("bash_1", &format!("64 bytes from x.com seq={i}\n"));
        }
        app.set_background_runtime("bash_1", Duration::from_secs(3));
        app.background_view = Some(BackgroundView::Details {
            id: "bash_1".to_string(),
        });
        let texts: Vec<String> = background_view_lines(&app, 74)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        assert_eq!(texts[2], "  Shell details");
        assert_eq!(texts[4], "  Status:   running");
        assert_eq!(texts[5], "  Runtime:  3s");
        assert_eq!(texts[6], "  Command:  ping -c 120 x.com");
        assert_eq!(texts[8], "  Output:");
        assert!(texts[9].starts_with("  ╭") && texts[9].ends_with('╮'));
        // The box tails the LAST rows: seq=3..=12 fill its 10 interior rows.
        assert!(
            texts[10].contains("seq=3"),
            "tails the newest lines: {texts:?}"
        );
        assert!(texts[19].contains("seq=12"));
        assert!(texts[20].starts_with("  ╰") && texts[20].ends_with('╯'));
        assert_eq!(texts[21], "  Showing 10 lines");
        assert_eq!(texts[22], "");
        assert_eq!(
            texts[23],
            "  ← to go back · Esc/Enter/Space to close · x to stop"
        );
        assert_eq!(background_view_height(&app, 40), Some(texts.len() as u16));
    }

    #[test]
    fn the_manager_band_replaces_the_composer_in_render_live() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping x.com", None, true);
        app.open_background_view();
        let h = background_view_height(&app, 30).unwrap();
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        let all: String = (0..h)
            .map(|y| row(&buf, y, 60))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Background"), "the band renders: {all}");
        assert!(all.contains("1 active shell"));
        assert!(
            !all.contains('❯') || all.contains("❯ ping"),
            "no composer prompt — the band replaced it: {all}"
        );
    }

    #[test]
    fn details_of_a_vanished_shell_fall_back_to_the_list() {
        let mut app = App::new();
        app.bg_started("bash_1", "ping x.com", None, true);
        // A Details view pointing at an unknown id renders the list instead
        // (bg_exited retargets, so this is the defensive path).
        app.background_view = Some(BackgroundView::Details {
            id: "ghost".to_string(),
        });
        let texts: Vec<String> = background_view_lines(&app, 60).iter().map(plain).collect();
        assert!(texts.iter().any(|l| l.contains("Background")));
        assert!(!texts.iter().any(|l| l.contains("Shell details")));
    }

    // ===== The Agent tool's cells + roster (docs/agent-tool.md) =====

    fn agent_entry(
        id: &str,
        desc: &str,
        status: crate::agents::AgentStatus,
    ) -> crate::app::AgentGroupEntry {
        crate::app::AgentGroupEntry {
            id: id.to_string(),
            description: desc.to_string(),
            agent_type: "general-purpose".to_string(),
            prompt: format!("What is the weather in {desc}?"),
            status,
            tool_uses: 2,
            tokens: 16_100,
            secs: 39,
            result: "It is 19°C.".to_string(),
            tool_headers: vec!["Bash(curl wttr.in)".to_string()],
            output: "It is 19°C.".to_string(),
        }
    }

    #[test]
    fn agent_group_lines_render_the_finished_tree() {
        use crate::agents::AgentStatus;
        let group = crate::app::AgentGroup {
            background: false,
            agents: vec![
                agent_entry("a1", "Fetch Warsaw", AgentStatus::Done),
                agent_entry("a2", "Fetch Manila", AgentStatus::Done),
            ],
            timestamp: String::new(),
        };
        let lines = agent_group_lines(&group, 80);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(texts[0], "● 2 agents finished (ctrl+o to expand)");
        assert_eq!(texts[1], "   ├ Fetch Warsaw · 2 tool uses · 16.1k tokens");
        assert_eq!(texts[2], "   │ ⎿  Done");
        assert_eq!(texts[3], "   └ Fetch Manila · 2 tool uses · 16.1k tokens");
        assert_eq!(texts[4], "     ⎿  Done");
        // All clean → green bullet; one interrupted → red.
        assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_OK_COLOR));
        let mut stopped = group.clone();
        stopped.agents[1].status = AgentStatus::Interrupted;
        let lines = agent_group_lines(&stopped, 80);
        assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_FAIL_COLOR));
        assert_eq!(plain(&lines[4]), "     ⎿  Interrupted");
    }

    #[test]
    fn agent_group_lines_render_the_background_launch() {
        use crate::agents::AgentStatus;
        let group = crate::app::AgentGroup {
            background: true,
            agents: vec![
                agent_entry("a1", "Fetch Warsaw", AgentStatus::Running),
                agent_entry("a2", "Fetch Manila", AgentStatus::Running),
            ],
            timestamp: String::new(),
        };
        let texts: Vec<String> = agent_group_lines(&group, 80).iter().map(plain).collect();
        assert_eq!(texts[0], "● 2 background agents launched (↓ to manage)");
        assert_eq!(texts[1], "   ├ Fetch Warsaw");
        assert_eq!(texts[2], "   └ Fetch Manila");
        assert_eq!(texts.len(), 3, "description-only rows, no status");
    }

    fn spec(id: &str, desc: &str, background: bool) -> crate::stream::AgentSpec {
        crate::stream::AgentSpec {
            id: id.into(),
            description: desc.into(),
            agent_type: "general-purpose".into(),
            prompt: "task?".into(),
            background,
        }
    }

    #[test]
    fn a_lone_live_agent_renders_the_tool_cell_shape() {
        let mut app = App::new();
        app.begin_stream();
        app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
        // Announced, no event yet: the Agent cell over `⎿ Initializing…`.
        let texts: Vec<String> = live_agent_group_lines(&app, 80).iter().map(plain).collect();
        assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
        assert_eq!(texts[1], "  ⎿  Initializing…");
        // The strip sizes from the same walk (the box/cursor geometry contract).
        assert_eq!(usize::from(preview_rows(&app, 80)), texts.len());
        // A running tool shows its wrapped header + a dim Running… row.
        app.apply_agent_event(
            "a1",
            &crate::stream::StreamEvent::ToolStart {
                name: "Bash".into(),
                args: "sleep 10 && curl -s https://api.open-meteo.com/v1/forecast".into(),
                detail: Some("Fetching Warsaw weather".into()),
            },
        );
        let texts: Vec<String> = live_agent_group_lines(&app, 44).iter().map(plain).collect();
        assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
        assert!(
            texts[1].starts_with("  ⎿  Bash(sleep 10 && curl"),
            "{}",
            texts[1]
        );
        assert!(
            texts[2].starts_with("         "),
            "continuations align under the (: {}",
            texts[2]
        );
        assert!(texts.iter().any(|t| t.trim() == "Running…"));
        assert_eq!(usize::from(preview_rows(&app, 44)), texts.len());
        // Between calls the sticky activity line holds — never `Working…`.
        app.apply_agent_event(
            "a1",
            &crate::stream::StreamEvent::ToolEnd {
                output: "+19°C".into(),
                ok: true,
                truncated: false,
            },
        );
        let texts: Vec<String> = live_agent_group_lines(&app, 80).iter().map(plain).collect();
        assert_eq!(texts[1], "  ⎿  Bash: Fetching Warsaw weather");
        // …and rendering the live region upholds the debug_assert.
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_live(area, &mut buf, &app);
    }

    #[test]
    fn a_lone_committed_agent_renders_done_with_the_expand_hint() {
        use crate::agents::AgentStatus;
        let group = crate::app::AgentGroup {
            background: false,
            agents: vec![agent_entry(
                "a1",
                "Fetch current weather in Warsaw",
                AgentStatus::Done,
            )],
            timestamp: String::new(),
        };
        let texts: Vec<String> = agent_group_lines(&group, 80).iter().map(plain).collect();
        assert_eq!(texts[0], "● Agent(Fetch current weather in Warsaw)");
        assert_eq!(texts[1], "  ⎿  Done (2 tool uses · 16.1k tokens · 39s)");
        assert_eq!(texts[2], "  (ctrl+o to expand)");
        // Interrupted: the red footer, no counters.
        let mut stopped = group.clone();
        stopped.agents[0].status = AgentStatus::Interrupted;
        let texts: Vec<String> = agent_group_lines(&stopped, 80).iter().map(plain).collect();
        assert_eq!(texts[1], "  ⎿  Interrupted");
        // A lone background launch keeps the manage row instead.
        let mut launched = group;
        launched.background = true;
        launched.agents[0].status = AgentStatus::Running;
        let texts: Vec<String> = agent_group_lines(&launched, 80).iter().map(plain).collect();
        assert_eq!(texts[0], "● Agent(Fetch current weather in Warsaw)");
        assert_eq!(texts[1], "  ⎿  Running in the background (↓ to manage)");
        assert_eq!(texts.len(), 2, "no expand hint on the backgrounded cell");
    }

    #[test]
    fn a_multi_agent_tree_keeps_the_sticky_tool_activity() {
        let mut app = App::new();
        app.begin_stream();
        app.start_agent_group(
            false,
            &[
                spec("a1", "Fetch Warsaw", false),
                spec("a2", "Write a game", false),
            ],
        );
        app.apply_agent_event(
            "a1",
            &crate::stream::StreamEvent::ToolStart {
                name: "Bash".into(),
                args: "curl wttr.in/Warsaw".into(),
                detail: Some("Fetching Warsaw weather".into()),
            },
        );
        for event in [
            crate::stream::StreamEvent::ToolStart {
                name: "Write".into(),
                args: "game.py".into(),
                detail: None,
            },
            // The call resolves — the activity line stays (sticky, no
            // `Working…` between calls).
            crate::stream::StreamEvent::ToolEnd {
                output: "Created game.py (10 lines)".into(),
                ok: true,
                truncated: false,
            },
        ] {
            app.apply_agent_event("a2", &event);
        }
        let texts: Vec<String> = live_agent_group_lines(&app, 90).iter().map(plain).collect();
        assert_eq!(texts[0], "● Running 2 agents… (ctrl+o to expand)");
        assert!(
            texts[2].ends_with("⎿  Bash: Fetching Warsaw weather"),
            "{}",
            texts[2]
        );
        assert!(texts[4].ends_with("⎿  Write: game.py"), "{}", texts[4]);
    }

    #[test]
    fn agent_cell_lines_expand_prompt_response_and_done() {
        use crate::agents::AgentStatus;
        let group = crate::app::AgentGroup {
            background: false,
            agents: vec![agent_entry("a1", "Fetch Warsaw", AgentStatus::Done)],
            timestamp: String::new(),
        };
        let texts: Vec<String> = agent_group_full_lines(&group, 100)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
        assert_eq!(texts[1], "  ⎿  Prompt:");
        assert_eq!(texts[2], "       What is the weather in Fetch Warsaw?");
        assert!(texts.contains(&"     Bash(curl wttr.in)".to_string()));
        assert!(texts.contains(&"  ⎿  Response:".to_string()));
        assert!(texts.contains(&"       It is 19°C.".to_string()));
        assert!(
            texts
                .last()
                .unwrap()
                .contains("Done (2 tool uses · 16.1k tokens · 39s)")
        );
        // An interrupted agent ends with the bare Interrupted footer instead.
        let mut stopped = group;
        stopped.agents[0].status = AgentStatus::Interrupted;
        let texts: Vec<String> = agent_group_full_lines(&stopped, 100)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(texts.last().unwrap(), "  ⎿  Interrupted");
    }

    #[test]
    fn the_transcript_expands_agent_groups_and_notices() {
        use crate::agents::AgentStatus;
        let mut app = App::new();
        app.history
            .push(HistoryItem::AgentGroup(crate::app::AgentGroup {
                background: false,
                agents: vec![agent_entry("a1", "Fetch Warsaw", AgentStatus::Done)],
                timestamp: String::new(),
            }));
        app.history
            .push(HistoryItem::AgentNotice(crate::app::AgentNotice {
                id: "a1".into(),
                description: "Fetch Warsaw".into(),
                status: AgentStatus::Done,
                secs: 35,
                result: "19°C".into(),
                timestamp: String::new(),
            }));
        let texts: Vec<String> = transcript_lines(&app, 100).iter().map(plain).collect();
        assert!(texts.contains(&"● Agent(Fetch Warsaw)".to_string()));
        assert!(
            texts
                .iter()
                .any(|t| t == "● Agent \"Fetch Warsaw\" finished · 35s")
        );
    }

    #[test]
    fn the_footer_roster_lists_main_and_the_agents() {
        let mut app = App::new();
        app.begin_stream();
        app.start_agent_group(
            false,
            &[crate::stream::AgentSpec {
                id: "a1".into(),
                description: "Fetch current weather and time in Warsaw".into(),
                agent_type: "general-purpose".into(),
                prompt: "warsaw?".into(),
                background: false,
            }],
        );
        app.set_agent_runtime("a1", Duration::from_secs(48));
        assert_eq!(agent_list_rows(&app), 3, "blank + main + one agent");
        let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
        assert_eq!(texts[0], "");
        assert_eq!(texts[1], "  ● main");
        assert!(
            texts[2].starts_with("  ◯ general-purpose  Fetch current weather and time in Warsaw"),
            "{}",
            texts[2]
        );
        assert!(texts[2].ends_with(" 48s"), "{}", texts[2]);
        // A narrow width truncates the description, never the suffix.
        let narrow: Vec<String> = agent_list_lines(&app, 46).iter().map(plain).collect();
        assert!(narrow[2].contains('…'), "{}", narrow[2]);
        assert!(narrow[2].ends_with(" 48s"), "{}", narrow[2]);
    }

    #[test]
    fn the_roster_selection_marks_rows_and_swaps_the_footer_hint() {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/repo");
        app.begin_stream();
        app.start_agent_group(
            false,
            &[crate::stream::AgentSpec {
                id: "a1".into(),
                description: "Fetch Warsaw".into(),
                agent_type: "general-purpose".into(),
                prompt: "warsaw?".into(),
                background: false,
            }],
        );
        // ↓ opens the selection on `● main`.
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
        assert!(texts[1].starts_with("❯ ● main"), "{}", texts[1]);
        assert_eq!(
            plain(&agent_hint_line(&app)),
            "  ↑/↓ to select · Enter to view"
        );
        assert_eq!(footer_rows(&app, 0), 1, "the hint takes the footer slot");
        // ↓ moves onto the agent row; the hint gains the stop key.
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
        assert!(texts[2].starts_with("❯ ◯ "), "{}", texts[2]);
        assert_eq!(plain(&agent_hint_line(&app)), "  Enter to view · x to stop");
    }

    #[test]
    fn the_agent_view_overlays_show_the_agents_transcript_and_context() {
        let mut app = App::new();
        app.begin_stream();
        app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
        for event in [
            crate::stream::StreamEvent::ToolStart {
                name: "Bash".into(),
                args: "curl wttr.in".into(),
                detail: None,
            },
            crate::stream::StreamEvent::ToolEnd {
                output: "+19°C".into(),
                ok: true,
                truncated: false,
            },
        ] {
            app.apply_agent_event("a1", &event);
        }
        assert!(
            agent_transcript_lines(&app, 80).is_none(),
            "no agent view — the main cache path renders"
        );
        app.open_agent_view("a1");
        let texts: Vec<String> = agent_transcript_lines(&app, 80)
            .expect("the agent view has its own transcript")
            .iter()
            .map(plain)
            .collect();
        assert!(
            texts.iter().any(|t| t.contains("task?")),
            "the prompt shows"
        );
        assert!(texts.iter().any(|t| t.starts_with("● Bash(curl wttr.in)")));
        assert!(texts.iter().any(|t| t.contains("+19°C")));
        // Ctrl+D derives the *agent's* context: its prompt is the user entry.
        let ctx: Vec<String> = context_lines(&app, 80).iter().map(plain).collect();
        assert!(ctx.iter().any(|t| t.contains("task?")), "{ctx:?}");
        assert!(
            ctx.iter().any(|t| t.contains("→ bash(")),
            "the agent's tool call replays: {ctx:?}"
        );
    }

    #[test]
    fn the_agent_view_swaps_the_strip_to_the_agents_stream() {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/repo");
        app.begin_stream();
        app.start_agent_group(
            false,
            &[crate::stream::AgentSpec {
                id: "a1".into(),
                description: "Fetch Warsaw".into(),
                agent_type: "general-purpose".into(),
                prompt: "warsaw?".into(),
                background: false,
            }],
        );
        app.apply_agent_event(
            "a1",
            &crate::stream::StreamEvent::ToolStart {
                name: "Bash".into(),
                args: "curl wttr.in".into(),
                detail: None,
            },
        );
        app.open_agent_view("a1");
        // The preview previews the AGENT's running tool, not the main group.
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_live(area, &mut buf, &app);
        let all: String = (0..24).map(|y| row(&buf, y, 80) + "\n").collect();
        assert!(all.contains("Bash(curl wttr.in)"), "{all}");
        assert!(
            all.contains(" Fetch Warsaw "),
            "the box rule carries the label: {all}"
        );
        assert!(
            all.contains("❯ ◯ "),
            "the roster marks the viewed agent: {all}"
        );
    }
}

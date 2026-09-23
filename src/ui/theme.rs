//! All of the TUI's styling and live-region geometry, in one place.
//!
//! Bullets, prompts, colours, the tool-cell and overlay chrome, the status
//! indicator's spinner/shimmer, the band and footer metrics — retheme or
//! re-size here, never inline at a call site. That rule is a project convention
//! (CLAUDE.md); the consts each feature owns are described in its own doc, e.g.
//! `docs/status-indicator.md`, `docs/footer.md`, `docs/toast.md`,
//! `docs/shortcuts.md`.
//!
//! The **colours** are accessor functions rather than consts: each names a
//! *role* (`tool_ok_color()` — a finished tool's bullet; `menu_selected_color()`
//! — what every picker selects with) and reads the active theme's
//! [`Palette`](super::palette::Palette), so a `/theme` switch recolours every
//! call site with no call site knowing (`docs/theme.md`). A colour word in a
//! role's doc comment — "cyan", "green", "dim grey" — describes the role as
//! the palettes paint it (the original One Dark chrome's words, kept because
//! every other theme paints the same role with the same *kind* of colour);
//! the exact value is the palette's. A new colour is a new role on the
//! palette plus its accessor here, never a literal `Color::Rgb` at a call
//! site.

use super::palette::palette;
use super::wrap::lerp_color;
use super::*;

// --- Claude-Code-ish styling. Centralised so it's trivial to retheme. ---

/// Prompt shown at the start of the input field.
pub(super) const PROMPT: &str = "❯ ";

/// Bullet prefixing a user message.
pub(super) const USER_BULLET: &str = "❯ ";

/// Bullet prefixing an assistant message.
pub(super) const AI_BULLET: &str = "● ";

/// Bullet prefixing a backend-error notice — same glyph as the assistant, but
/// coloured red (see [`error_color`]) so a failure reads as a red bullet point.
pub(super) const ERROR_BULLET: &str = "● ";

/// Bullet prefixing a system notice (slash-command output) — same glyph, coloured
/// cyan (see [`system_color`]) so it reads as meta rather than an AI reply.
pub(super) const SYSTEM_BULLET: &str = "● ";

/// Indent for wrapped continuation lines (matches a bullet's width).
pub(super) const INDENT: &str = "  ";

/// Columns a bullet/indent occupies, subtracted from the content width.
pub(super) const BULLET_WIDTH: u16 = 2;

/// The column every text field keeps past its text for the caret — the cell
/// the cursor sits in at the end of a row the wrap left exactly full
/// (`layout::text_field_width`, `docs/textarea.md`).
pub(super) const CURSOR_COLUMN: u16 = 1;

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
pub(super) const CODE_TAB_WIDTH: usize = 4;

/// A markdown thematic break (`---` / `***` / `___`) renders as this em-dash rule,
/// a direct port of codex's `Event::Rule` (`Line::from("———")` — three U+2014 EM
/// DASH, unstyled/default foreground). See `docs/markdown.md`.
pub(super) const THEMATIC_BREAK: &str = "———";

// --- Markdown table styling (docs/markdown.md, docs/table-streaming.md). A GFM
// pipe table renders as a box-drawing grid: dim borders, bold header cells.
// Column widths are locked from the header + first data row (fit to the width),
// so once the first row streams the block is prefix-stable and its rows commit
// to scrollback one at a time; every cell **word-wraps** into its column (taller
// rows) instead of truncating with `…` when the grid is narrow, matching codex.
// See `AssistantRenderer`/`StreamRender`. ---
/// Dim colour of a table's box-drawing borders (`│ ─ ┌┬┐ ├┼┤ └┴┘`).
pub(super) fn table_border_color() -> Color {
    tool_dim_color()
}

/// The floor a table column shrinks to before its cells word-wrap (codex uses 3):
/// a narrow column keeps at least this many display columns, and cells wrap into
/// it across multiple rows rather than losing text to a `…`.
pub(super) const TABLE_MIN_COL: usize = 3;

/// Records fallback (`docs/table-streaming.md`, Claude Code's key/value transpose):
/// a column at least this wide is considered scannable, so it never triggers the
/// fallback even if its content wraps (it's a legitimately wide narrative column).
pub(super) const TABLE_SCANNABLE_COL: usize = 12;

/// A cell that wraps into at least this many rows *in a narrow column* means the
/// grid is growing tall because columns are starved — flip to vertical records.
pub(super) const TABLE_RECORDS_MIN_LINES: usize = 3;

/// The narrowest value column the inline `label: value` record form keeps; below
/// this the field stacks (label on its own line, value indented beneath).
pub(super) const TABLE_RECORD_MIN_VALUE: usize = 12;

/// A stacked record value's indent under its label line.
pub(super) const TABLE_RECORD_STACK_INDENT: usize = 2;

/// The `─` rule between records caps at this many columns instead of spanning the
/// full content width — Claude Code's shorter separator reads cleaner for the
/// short key/value fields (a wide records table's full-width rule looked heavy).
pub(super) const TABLE_RECORD_SEPARATOR_WIDTH: usize = 40;

// --- Inline markdown styling (docs/markdown.md). A prose line's `**bold**`,
// `*italic*`, `~~strike~~`, `` `code` `` and `[text](url)` render with these;
// emphasis is modifier-only (bold/italic/crossed-out), code and links carry a
// colour. Parsing lives in `markdown::parse_inline`; `ui` owns the styling. ---
/// Inline `` `code` `` — a distinct cyan so it reads as code within prose.
pub(super) fn inline_code_color() -> Color {
    palette().accent
}

/// A link's URL, shown as ` (url)` after its text — blue and underlined.
pub(super) fn link_url_color() -> Color {
    palette().link
}

// --- List and blockquote styling (docs/markdown.md). Bullets keep `-`, ordered
// items keep `N.` in an accent colour; a blockquote's `>` and text render dim.
// Continuation rows hang under the item's text. ---
/// The accent colour of an ordered list's `N.`/`N)` marker.
pub(super) fn list_marker_color() -> Color {
    palette().link
}

/// A blockquote's `>` marker and text — dim, so a quote reads as secondary.
pub(super) fn quote_color() -> Color {
    tool_dim_color()
}

// Code syntax-highlight colours now come from the `highlight` module's theme
// (syntect + two_face, Catppuccin Mocha — codex parity), baked into each
// `highlight::Seg`'s `Style`. `ui` no longer owns a code palette or maps token
// kinds to colours; it just renders the segments the grammar produced. See
// `docs/markdown.md`.

pub(super) fn user_color() -> Color {
    palette().user_fg
}

pub(super) fn user_bg_color() -> Color {
    palette().user_bg
}

pub(super) fn ai_color() -> Color {
    palette().text
}

pub(super) fn error_color() -> Color {
    palette().error
}

/// Cyan — a system notice's bullet (slash-command output).
pub(super) fn system_color() -> Color {
    palette().accent
}

pub(super) fn prompt_color() -> Color {
    palette().text
}

pub(super) fn border_color() -> Color {
    palette().border
}

// --- Tool-call styling. A tool renders as a coloured bullet header
// `● name(args)` plus a collapsed `⎿` peek of its output; the bullet colour is
// the tool's lifecycle (a blinking grey while it runs, green ok, red fail —
// `docs/tool-pulse.md`). The full output is
// only shown in the Ctrl+O tool-output view, never inline. ---

/// Bullet prefixing a tool call (same glyph as the assistant, recoloured by
/// status — see [`tool_status_color`]).
pub(super) const TOOL_BULLET: &str = "● ";

/// Prefix for the first result line: indent + a turnstile glyph + two spaces
/// (Claude-Code's two-space corner). Continuation lines are indented by its
/// display width so a multi-line result aligns under the content (see
/// [`result_row`]).
pub(super) const TOOL_RESULT_PREFIX: &str = "  ⎿  ";

/// Prefix for the "+N lines" hint line under a capped peek — the `…` aligns
/// under the corner content (the [`TOOL_RESULT_PREFIX`] width of leading
/// spaces).
pub(super) const TOOL_MORE_PREFIX: &str = "     … ";

/// Hint telling the user how to see the full output.
pub(super) const EXPAND_HINT: &str = " (ctrl+o to expand)";

/// The collapsed MCP cell's vocabulary (`docs/mcp.md`): a running call's
/// `● Calling {server}… (ctrl+o to expand)` header and the resolved
/// bullet-less dim `Called {server} (ctrl+o to expand)` line (the settled
/// thinking line's shape — `reasoning_label_color()`). The full
/// `{server} - {tool} (MCP)({args})` story lives in Ctrl+O.
pub(super) const MCP_CALLING_PREFIX: &str = "Calling ";
pub(super) const MCP_CALLING_SUFFIX: &str = "…";
pub(super) const MCP_CALLED_PREFIX: &str = "Called ";

/// How many wrapped display **rows** ONE source line may spend in a collapsed
/// **numbered file cell** (`ui/file_cell.rs`, `docs/long-lines.md`). Past this
/// it shows its head and stops, marked with [`TOOL_LINE_ELLIPSIS`], so a
/// minified `.json` line in a `read`/`write` peek paints three rows instead of
/// dozens while the short lines after it still show. The exec cells need no
/// per-line budget of their own: their whole block folds at
/// [`TOOL_FOLD_ROWS`], which bounds a blob and everything else alike.
pub(super) const TOOL_LINE_MAX_ROWS: usize = 3;

/// The marker closing a numbered row whose source line was cut at
/// [`TOOL_LINE_MAX_ROWS`]: without it a clipped line reads as a line that
/// simply ended, and a complete row is indistinguishable from the head of a
/// 2 KB blob. Fitted so the row still ends inside the width, like
/// [`ellipsize`]'s cut.
pub(super) const TOOL_LINE_ELLIPSIS: &str = "…";

/// The window the **running** command tail shows (`running_command_lines`):
/// the last four wrapped display **rows** of what the command has printed,
/// over the `+N lines (Ns · timeout …)` clock row — the unit the user reads
/// the cell in (`docs/long-lines.md`). It is also the ceiling of the
/// **settled** block,
/// hint included: [`TOOL_FOLD_ROWS`] rows over the `… +N lines` hint, or
/// `TOOL_FOLD_ROWS + 1` rows shown whole — so a `bash` cell never *grows*
/// when it settles, whatever shape its output has; the rest is one `ctrl+o`
/// away.
pub(super) const TOOL_PEEK_ROWS: usize = 4;

/// The marker spliced in (before the closing `)`) when a collapsed header's
/// command is cut at [`TOOL_HEADER_MAX_LINES`] lines or
/// [`TOOL_HEADER_MAX_COLS`] columns (`tool_header_lines`).
pub(super) const TOOL_HEADER_ELLIPSIS: &str = "…";

/// How many **lines** of a command the collapsed header shows before the rest
/// is cut — Claude Code's `Bash(…)` header keeps two (`docs/tools.md`
/// *Long headers*). The Ctrl+O transcript shows every line.
pub(super) const TOOL_HEADER_MAX_LINES: usize = 2;

/// How many display **columns** of a command the collapsed header shows
/// before the rest is cut with [`TOOL_HEADER_ELLIPSIS`] — Claude Code's 160,
/// a budget on the *text* rather than on rows, so the same command is cut at
/// the same character in every terminal width and never floods the cell.
pub(super) const TOOL_HEADER_MAX_COLS: usize = 160;

/// How many display **rows** of its output a settled exec cell shows *above
/// the fold* — Claude Code's three — before the rest collapses behind the
/// `… +N lines (ctrl+o to expand)` hint (`docs/long-lines.md`). An output of
/// exactly `TOOL_FOLD_ROWS + 1` rows shows whole: a hint hiding one row would
/// cost the very row it hides. So the block is at most [`TOOL_PEEK_ROWS`]
/// rows either way, hint included.
pub(super) const TOOL_FOLD_ROWS: usize = 3;

/// The size cap on the display-only JSON reshaping of an exec cell's output
/// (`docs/long-lines.md` *Pretty JSON*): an output longer than this many
/// bytes is shown as it came — Claude Code's own 10 000-character guard,
/// which bounds the `Value` tree the reshaping parses per render.
pub(super) const TOOL_JSON_PRETTY_MAX_BYTES: usize = 10_000;

/// The parens framing a header's arguments — `● {name}({args})`. The opening
/// one rides the name's row (so an argument too wide for what is left beside
/// the name spills to the continuation row whole), the closing one the last
/// argument row; both wear the arguments' bold white (`tool_header_lines`).
pub(super) const TOOL_HEADER_OPEN: &str = "(";
pub(super) const TOOL_HEADER_CLOSE: &str = ")";

/// The share of the terminal a wrapped header may spend aligning its
/// continuation rows under the opening `(`: at most one part in this many.
/// Past it (an MCP call's `● deepwiki - ask_question (MCP)` is 31 columns —
/// 40% of an 76-column terminal) the header hangs its continuations at the
/// bullet's own two columns instead, so the arguments get the width rather
/// than a ragged column. See `tool_header_lines` and `docs/mcp.md`.
pub(super) const TOOL_HEADER_ALIGN_SHARE: usize = 3;

/// Dim marker appended at the end of a `!` shell command's **expanded** output
/// (`tool_full_lines`) when it was cut at the in-memory cap (`tool.truncated`),
/// to show that more output was dropped. See `docs/shell-command.md`.
pub(super) const TOOL_TRUNCATED_MARKER: &str = "…";

/// Placeholder body for a finished tool that produced no output.
pub(super) const TOOL_NO_OUTPUT: &str = "(no output)";

// ---------------------------------------------------------------------------
// The thinking stream (docs/thinking-stream.md). It borrows the tool cell's
// shape while it runs — a [`TOOL_BULLET`] `● Thinking…` header (its bullet
// blinking like a running tool's, its label carrying the status line's
// [`shimmer_base`] white sweep) over the chain-of-thought in the
// [`TOOL_RESULT_PREFIX`] `⎿` gutter — and then collapses into a **bullet-less**
// `Thought for … · … tokens (ctrl+o to expand)` line: the [`summary_lines`]
// shape *and* its dim, because a settled thought is turn meta, not a cell —
// a footnote about work already done. The strip is where the weight and the
// motion are: nothing there is ever committed, so it can afford both.
// ---------------------------------------------------------------------------

/// The chain-of-thought's own colour — dim, and italic
/// ([`REASONING_TEXT_MODIFIER`]), so it reads as the model thinking aloud
/// rather than as a tool's output in the same `⎿` gutter.
pub(super) fn reasoning_text_color() -> Color {
    tool_dim_color()
}

/// Italic: the one cue separating a thought's `⎿` body from a tool's.
pub(super) const REASONING_TEXT_MODIFIER: Modifier = Modifier::ITALIC;

/// The live block's header, under which the thought tails.
pub(super) const REASONING_RUNNING: &str = "Thinking…";

/// The **resting** colour of the live header's shimmer — what `Thinking…`
/// reads as between crests, which is most of the sweep (the band is
/// [`SHIMMER_BAND_HALF_WIDTH`] wide inside a period of the text plus twice
/// [`SHIMMER_PADDING`]).
///
/// Deliberately *not* codex's grey [`shimmer_base`]: that makes the status
/// verb read as grey text with a white wave, which is right for a metric and
/// wrong for a header — at rest it is indistinguishable from the dim body
/// under it. This near-white floor reads as **bold white**, with the wave a
/// brightening on top rather than the only thing making it visible — the one
/// place in the feature that draws the eye, because it is the one place
/// something is still happening.
pub(super) fn reasoning_shimmer_base() -> Color {
    model_id_color()
}

/// The settled line's opener — `Thought for 1m 5s · 1.5k tokens`.
pub(super) const REASONING_DONE: &str = "Thought for ";

/// The settled line's colour: [`status_done_color`], the dim the committed
/// `Done for Ns` summary wears. The two are the same kind of line — turn meta,
/// bullet-less, one row — and they bracket a turn, so they read as a pair.
///
/// One tone across the whole line and both surfaces: a settled thought is a
/// footnote about work already done, not a heading. What is *happening* — the
/// live `● Thinking…` block — is what carries weight and motion.
pub(super) fn reasoning_label_color() -> Color {
    status_done_color()
}

/// How many **wrapped display rows** of the thought the live block tails. The
/// window is small on purpose: it grows the live region upward (invariant 3),
/// and what the user wants is the frontier — what the model is thinking
/// *now*. The whole thing is in the Ctrl+O transcript.
pub(super) const REASONING_PEEK_LINES: usize = 5;

// ---------------------------------------------------------------------------
// Background shells (docs/background.md): the backgrounded cell's fixed row,
// the running-command Ctrl+B hint, the ↓ manager band, and the footer count.
// ---------------------------------------------------------------------------

/// The fixed `⎿` body of a call resolved as [`ToolStatus::Backgrounded`] —
/// the stored output (the model-facing launch text) is never shown.
pub(super) const TOOL_BACKGROUNDED: &str = "Running in the background (↓ to manage)";

/// The dim live-only hint under a running command's preview cell: Ctrl+B
/// moves it to the background. Never committed to scrollback.
pub(super) const TOOL_BACKGROUND_HINT: &str = "(ctrl+b to run in background)";

/// How long a command must have been running before its preview shows the
/// `(ctrl+b to run in background)` hint — Claude-Code-style, so a command that
/// finishes right away never flashes it (Ctrl+B itself still works the whole
/// time; only the discoverability hint waits). Gated on the boundary-injected
/// [`App::background_hint_elapsed`]. See `docs/background.md`.
pub(super) const TOOL_BACKGROUND_HINT_DELAY: Duration = Duration::from_secs(3);

/// The separator inside a running command's clock clause, between the
/// command's own elapsed and the timeout it runs under:
/// `(22s · timeout 1m 50s)` — the footer's [`FOOTER_SEPARATOR`], the one
/// dot the whole chrome joins facts with (`docs/tool-streaming.md`).
pub(super) const TOOL_CLOCK_SEPARATOR: &str = " · ";

/// The label in front of the timeout in that clause — `timeout 1m 50s`.
/// A limit needs naming where an elapsed does not: bare, the two numbers
/// would read as a range.
pub(super) const TOOL_TIMEOUT_LABEL: &str = "timeout ";

/// The ↓ manager's list title.
pub(super) const BG_TITLE: &str = "Background";

/// The details page's title.
pub(super) const BG_DETAILS_TITLE: &str = "Shell details";

/// The list's empty state — shown when every shell has finished.
pub(super) const BG_EMPTY: &str = "No tasks currently running";

/// The list page's key hints.
pub(super) const BG_LIST_HINTS: &str = "↑/↓ to select · Enter to view · x to stop · Esc to close";

/// The empty state's key hints (nothing to stop).
pub(super) const BG_EMPTY_HINTS: &str = "↑/↓ to select · Enter to view · Esc to close";

/// The details page's key hints.
pub(super) const BG_DETAILS_HINTS: &str = "← to go back · Esc/Enter/Space to close · x to stop";

/// The `(running)` suffix on a list row.
pub(super) const BG_ROW_SUFFIX: &str = " (running)";

/// The band's two-space inset (the picker/footer indent).
pub(super) const BG_INDENT: &str = "  ";

/// The selected list row's marker (the resume picker's `❯`).
pub(super) const BG_MARKER: &str = "❯ ";

/// At most this many list rows show at once (the window follows the
/// selection, like the pickers).
pub(super) const BG_MENU_MAX_ROWS: usize = 8;

/// The details output box's interior height: the last rows of the live
/// output tail, blank-padded (the mock's fixed box).
pub(super) const BG_OUTPUT_ROWS: usize = 10;

/// The details page's field labels, padded to one column.
pub(super) const BG_FIELD_STATUS: &str = "Status:   ";

pub(super) const BG_FIELD_RUNTIME: &str = "Runtime:  ";

pub(super) const BG_FIELD_COMMAND: &str = "Command:  ";

/// The launcher field a subagent-launched shell adds (`From: {type} agent`).
pub(super) const BG_FIELD_FROM: &str = "From:     ";

/// The details page's output-box heading.
pub(super) const BG_OUTPUT_LABEL: &str = "Output:";

/// The value of the status field while listed (an exited shell leaves the
/// manager, so a listed one is always running).
pub(super) const BG_STATUS_RUNNING: &str = "running";

/// The manager's title/selection accent (the palette accent) and dim text.
pub(super) fn bg_selected_color() -> Color {
    menu_selected_color()
}

pub(super) fn bg_dim_color() -> Color {
    tool_dim_color()
}

/// The notice bullet colours: green success, red failure/stop.
pub(super) fn bg_notice_ok_color() -> Color {
    tool_ok_color()
}

pub(super) fn bg_notice_fail_color() -> Color {
    tool_fail_color()
}

/// Placeholder body for a still-executing tool — the `⎿ Running…` row, shown
/// under a backend tool's `● name(args)` header (req 2: a running cell shows the
/// header *and* this row, previewed live) and as the whole `!` shell cell.
pub(super) const TOOL_RUNNING: &str = "Running…";

/// Placeholder body for a tool queued in a **parallel batch** but not yet
/// started — the dim `⎿ Waiting…` row shown under a not-yet-running sibling's
/// `● name(args)` header while another call in the batch executes. See
/// `docs/parallel-tools.md`.
pub(super) const TOOL_WAITING: &str = "Waiting…";

/// The **resting** colour of a tool that is still executing: the same grey the
/// permission prompt shows over the call it is asking about, so everything
/// in flight reads muted and only the green/red *resolution* lands as colour.
/// (It was a blue `#61AFEF`; the blue survives as [`context_user_color`], which
/// is a role tag, not a running state.)
///
/// In the **live region** the bullet does not sit still — it **blinks**
/// ([`bullet_span`](super::tool::bullet_span)), Claude-Code's running dot:
/// shown in this one grey for half of every [`TOOL_PULSE_PERIOD`], then
/// hidden behind blanks of its own width for the other half. It is the only
/// colour a running bullet ever wears — there is no dimmer second shade for
/// it to ease toward — so a frozen render (a scrollback commit, the Ctrl+O
/// transcript's pager) shows exactly this, and the strip shows this or
/// nothing.
pub(super) fn tool_running_color() -> Color {
    tool_dim_color()
}

// The running bullet's **cadence**, and the breath the `pulse` spinner style
// still borrows (`docs/spinner.md`). The bullet itself no longer blends
// colours — it blinks (`tool::tool_pulse_visible`) — but the `pulse` spinner
// keeps the raised-cosine dim→bright→dim breath these two ends describe, its
// ends blended through `wrap::blend_color` (which steps rather than mixes when
// a theme names a terminal-palette colour, `docs/theme.md`), in step with the
// bullet off the same boundary-injected frame clock (`App::set_pulse`). See
// `docs/tool-pulse.md`.

/// The dim end of the `pulse` spinner's breath — where its cycle starts and
/// ends. Well below the resting grey, so the dip carries the whole animation
/// without the dot ever vanishing.
pub(super) fn tool_pulse_dim() -> Color {
    palette().pulse_dim
}

/// The bright end of that breath at the tool bullet's level —
/// [`tool_dim_color`] exactly, the running bullet's own grey — which the
/// `bars` spinner style takes for its lowest bar so even `▁` reads
/// ([`spinner_bars_low`]); the `pulse` style crests past it to white.
pub(super) fn tool_pulse_bright() -> Color {
    tool_dim_color()
}

/// One full blink — shown for the first half, hidden for the second — and one
/// full breath of the `pulse` spinner. Slow enough to read as a pulse rather
/// than a flicker, brisk enough to say "something is happening" — and
/// comfortably coarser than the 32 ms animation frame the loop re-arms while
/// a turn runs.
pub(super) const TOOL_PULSE_PERIOD: Duration = Duration::from_millis(1000);

/// Dim grey — a tool queued in a batch but not yet started (its `● name(args)`
/// bullet and `⎿ Waiting…` row read muted; it is the running head's *resting*
/// grey too, the difference being that a running bullet **moves**,
/// since it hasn't begun). Shares the argument/peek dim grey.
pub(super) fn tool_waiting_color() -> Color {
    tool_dim_color()
}

/// Green — a tool that finished successfully. A vivid, saturated green (rather
/// than the old muted `#98C379`) so the `●` success bullet clearly stands out,
/// Claude-Code style. Shared by the `+`-line diff colour, the active-model tick
/// and the context view's assistant tag ([`tool_diff_add_color`] etc.).
pub(super) fn tool_ok_color() -> Color {
    palette().success
}

/// Red — a tool that failed (shares the backend-error red).
pub(super) fn tool_fail_color() -> Color {
    error_color()
}

/// White — the tool's name.
pub(super) fn tool_name_color() -> Color {
    ai_color()
}

/// White (the normal assistant reply colour) + bold — the whole `(...)` header
/// body: the command/args text **and** its framing `(`/`)` (and a truncation
/// `…`) alike, so a `bash` command and its brackets read as prominently as a
/// normal reply rather than the old dim grey. Claude-Code's noticeable tool
/// header; shared by every tool via [`tool_header_lines`].
pub(super) fn tool_args_color() -> Color {
    ai_color()
}

/// White (the normal reply colour) — a finished tool's **output** under the `⎿`
/// gutter (command/shell output), so it's as legible as a normal message rather
/// than dim grey. The `⎿` corner, the `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `… +N lines` hint all stay [`tool_dim_color`].
pub(super) fn tool_output_color() -> Color {
    ai_color()
}

/// Dim grey — a tool's `⎿` gutter corner, its `Running…`/`Waiting…`/`(no output)`
/// placeholders and the `… +N lines` hint.
pub(super) fn tool_dim_color() -> Color {
    palette().dim
}

// ===== The task tools' live checklist (docs/task-tools.md) =====

/// The checklist's status glyphs — Claude Code's `figures` trio: pending's
/// empty square, in-progress's filled square, completed's tick.
pub(super) const TASK_PENDING_GLYPH: &str = "◻";
pub(super) const TASK_IN_PROGRESS_GLYPH: &str = "◼";
pub(super) const TASK_COMPLETED_GLYPH: &str = "✔";

/// The in-progress glyph's colour — the system cyan (Claude Code paints its
/// filled square in the brand colour; cyan is this TUI's accent). The
/// subject beside it renders bold in the normal reply colour.
pub(super) fn task_in_progress_color() -> Color {
    system_color()
}

/// The completed glyph's green ([`tool_ok_color`]); the subject beside it is
/// dim and struck through — Claude Code's done row.
pub(super) fn task_completed_color() -> Color {
    tool_ok_color()
}

/// The dim `› blocked by #1, #2` suffix's marker (Claude Code's
/// `figures.pointerSmall`).
pub(super) const TASK_BLOCKED_MARKER: &str = "›";

/// The inset the **idle** task block sits at — the count line and its rows
/// (`docs/task-tools.md`). Two spaces, the same inset the footer and the
/// queued messages use, so the resting screen reads as one column; in a turn
/// the rows take [`TOOL_RESULT_PREFIX`]'s `⎿` gutter instead.
pub(super) const TASK_IDLE_INDENT: &str = "  ";

/// The most checklist rows shown in the strip before the tail folds into a
/// dim `… +N pending` summary row (Claude Code caps at ten and prioritises
/// what is actionable — see `ui::tasks`).
pub(super) const TASK_MAX_ROWS: usize = 10;

/// Green — an added (`+`) line in an `edit`/`write` diff cell (codex's diff
/// look, adapted to the `⎿` gutter; see `docs/tools.md`).
pub(super) fn tool_diff_add_color() -> Color {
    tool_ok_color()
}

/// Red — a removed (`-`) line in an `edit`/`write` diff cell.
pub(super) fn tool_diff_del_color() -> Color {
    tool_fail_color()
}

/// Dark-green background tint of an added row in a numbered `edit`/`write`
/// cell (codex's dark-theme add tint): the syntax-coloured text reads over it
/// and the row pads to the full width, like the user-message block.
pub(super) fn tool_diff_add_bg() -> Color {
    palette().diff_add_bg
}

/// Dark-red background tint of a removed row (codex's dark-theme delete
/// tint); the removed text is additionally dimmed, codex-style.
pub(super) fn tool_diff_del_bg() -> Color {
    palette().diff_del_bg
}

/// Background tint of the **characters that changed** on an added row — the
/// character-level refinement's own colour (`docs/inline-diff.md`). A clearly
/// brighter green than the row's [`tool_diff_add_bg`], because the two are
/// read together: the muted row tint says *this line changed*, the bright mark
/// says *here*. Bright enough to find at a glance, dark enough that the row's
/// syntax colours still read over it.
pub(super) fn tool_diff_add_mark_bg() -> Color {
    palette().diff_add_mark_bg
}

/// Background tint of the characters that changed on a removed row — the
/// [`tool_diff_add_mark_bg`] twin over [`tool_diff_del_bg`].
pub(super) fn tool_diff_del_mark_bg() -> Color {
    palette().diff_del_mark_bg
}

/// How much of a `-`/`+` line pair must be **common** for the character-level
/// refinement to run at all, as a percentage of the longer line's
/// non-whitespace display columns (`docs/inline-diff.md`). Below it the two
/// lines are a replacement rather than an edit: there is no "what changed" to
/// point at, and marking most of the line is noisier than marking none of it,
/// so the row keeps its flat tint. Whitespace is excluded from the count so a
/// shared indent alone never reads as similarity.
pub(super) const INLINE_DIFF_MIN_COMMON_PCT: usize = 30;

/// The most token-LCS cells one refined pair will fill. The common prefix and
/// suffix are trimmed first, so a typical edit's changed middle is a token or
/// two however long the line; past this bound (a minified bundle line) the
/// whole middle is marked changed instead — the trimmed ends are still real
/// common context, so the answer stays honest and the live cell keeps
/// re-rendering inside its 32 ms frame.
pub(super) const INLINE_DIFF_MAX_CELLS: usize = 40_000;

/// How many numbered body rows a `write`/`edit` cell shows inline before the
/// `… +N lines (ctrl+o to expand)` hint (Claude-Code's ~10-row Write preview;
/// other tools keep the tighter [`TOOL_PEEK_LINES`]).
pub(super) const FILE_PEEK_LINES: usize = 10;

/// The model tools whose output is a file change — rendered as the numbered,
/// syntax-highlighted codex-style cell when the output is in the
/// `llm::tools` gutter format ([`file_cell_lines`]), or with the legacy
/// first-char `+`/`-` colouring when it isn't (old sessions, error bodies).
/// A `!` shell command is never one (its output is command output).
pub(super) const DIFF_TOOL_NAMES: [&str; 2] = ["Edit", "Write"];
/// The model tools whose output is **command output** — a shell run, streamed
/// and framed with an `Exit code: N` line. They render like the `!` shell cell
/// (a multi-line `⎿` peek, the frame stripped for display) and **tail** their
/// output live while running (`docs/tool-streaming.md`): `bash`, and
/// `bash_session` — the same command's later steps
/// (`docs/interactive-shell.md`). A non-command generic tool keeps the single
/// collapsed peek line.
pub(super) const COMMAND_TOOL_NAMES: [&str; 2] = ["Bash", "BashSession"];

/// The dim closing row of a command cell whose session is still alive
/// (`docs/interactive-shell.md`) — `{state} · session {id}`, in place of the
/// report's frame line, which is the model's: the program sits at a prompt,
/// is still busy, or was ended by the call.
pub(super) const SESSION_WAITING_ROW: &str = "Waiting for input";
/// See [`SESSION_WAITING_ROW`].
pub(super) const SESSION_RUNNING_ROW: &str = "Still running";
/// See [`SESSION_WAITING_ROW`].
pub(super) const SESSION_STOPPED_ROW: &str = "Stopped";
/// The separator between a session row's state and its id.
pub(super) const SESSION_ROW_ID: &str = " · session ";

// --- Tool-output view (the Ctrl+O full-screen overlay) — codex's Ctrl+T
// transcript pager: a slash-tiled dim title row over a scrolling body (the
// full conversation transcript — every message plus every tool call's
// *complete* (expanded) output — with vi-style `~` filler rows past its end),
// closed by a `─` separator carrying the scroll percentage and two dim
// key-hint rows above a final blank row. ---

/// The pager's spaced-caps title, overlaid on the slash tiling as
/// `/ T R A N S C R I P T` (codex's transcript overlay header).
pub(super) const TOOL_VIEW_TITLE: &str = "T R A N S C R I P T";

/// Rows of chrome above the scrolling body (the slash-tiled title row).
pub(super) const TOOL_VIEW_TITLE_ROWS: u16 = 1;

/// Rows of chrome below the body: the `─` separator carrying the scroll
/// percentage, two key-hint rows, and the final blank row (codex's pager).
pub(super) const TOOL_VIEW_FOOTER_ROWS: u16 = 4;

/// First key-hint row under the separator (codex's pager hints, all dim).
pub(super) const TOOL_VIEW_HINT_KEYS: &str =
    " ↑/↓ to scroll   pgup/pgdn to page   home/end to jump";

/// Second key-hint row: every key that closes the overlay.
pub(super) const TOOL_VIEW_HINT_QUIT: &str = " q/esc/ctrl+o to quit";

/// Second key-hint row when idle Esc would instead **begin** the backtrack
/// preview (`App::overlay_esc_backtracks` — `docs/backtrack.md`): Esc is not
/// a quit key in that state, and promising it was ("I pressed Esc to exit
/// and got edit-previous-message") is the reported surprise this hint fixes.
/// Same three-space entry separator as [`TOOL_VIEW_HINT_KEYS`].
pub(super) const TOOL_VIEW_HINT_QUIT_EDIT: &str = " q/ctrl+o to quit   esc to edit prev";

/// Second key-hint row while a backtrack preview highlights a user message —
/// codex's highlighted-pager footer (`docs/backtrack.md`); it replaces
/// [`TOOL_VIEW_HINT_QUIT`], whose Esc meaning the preview takes over.
pub(super) const TOOL_VIEW_HINT_BACKTRACK: &str =
    " esc/← to edit prev   → to edit next   enter to edit message   q to cancel";

/// The vi-style filler marking body rows below the transcript's end.
pub(super) const TOOL_VIEW_FILL: &str = "~";

/// The dim placeholder shown when the transcript has nothing to list yet.
pub(super) const TOOL_VIEW_EMPTY: &str = "Nothing here yet.";

// --- Ctrl+D context-debug view (the third alternate-screen overlay) — the
// raw LLM context window (docs/context.md): the transcript pager's chrome
// (slash-tiled title, `~` filler, percentage separator, dim key hints) over
// a body listing exactly what the model is sent — the system prompt, then
// every derived context message role-tagged, its text **verbatim** (image
// placeholders and bracketed tool/shell/notice formats unrendered) with the
// attachment paths dim beneath. ---

/// The view's spaced-caps title, overlaid on the slash tiling.
pub(super) const CONTEXT_VIEW_TITLE: &str = "C O N T E X T";

/// Second key-hint row: every key that closes the view.
pub(super) const CONTEXT_VIEW_HINT_QUIT: &str = " q/esc/ctrl+d to quit";

/// The dim placeholder when the context window is empty (no system prompt —
/// the dummy sends none — and nothing said yet).
pub(super) const CONTEXT_VIEW_EMPTY: &str = "Context is empty — send a message to fill it.";

/// The system prompt's role tag — set apart from a mid-conversation
/// `system:` note (a derived `[system]`/`[error]` notice).
pub(super) const CONTEXT_SYSTEM_PROMPT_TAG: &str = "system prompt:";

// --- The Ctrl+D view's classifier page (`docs/permissions.md`) — Tab's
// other half: the same pager chrome over the bounded task context auto
// mode's classifier reads before every command and MCP call. ---

/// The view's spaced-caps title, overlaid on the slash tiling.
pub(super) const CLASSIFIER_VIEW_TITLE: &str = "C L A S S I F I E R";

/// The page-flip hint appended to the second key-hint row, naming the page
/// Tab would show — the view's only discovery affordance for its other half.
pub(super) const CONTEXT_VIEW_HINT_TAB_CLASSIFIER: &str = "   tab for classifier context";

/// …and the way back.
pub(super) const CONTEXT_VIEW_HINT_TAB_LLM: &str = "   tab for llm context";

/// The dim placeholder before anything is recorded (a fresh session, or a
/// backend that keeps no log — the dummy).
pub(super) const CLASSIFIER_VIEW_EMPTY: &str =
    "No classifier context yet — send a message to fill it.";

/// The dim note above the block in **auto** mode: the log is live, and this
/// is what the next verdict reads.
pub(super) const CLASSIFIER_VIEW_NOTE_AUTO: &str =
    "Auto mode — the classifier reads this before each command or MCP call.";

/// The same note in every other mode. The log is recorded in all of them (the
/// boundary feeds it per call, not per verdict), so a view that stayed silent
/// here would read as "the classifier is deciding this" when it is not.
pub(super) const CLASSIFIER_VIEW_NOTE_INACTIVE: &str =
    "Recorded every turn; consulted only in auto mode (shift+tab to switch).";

/// …and with the gate off entirely (`ALTER_ZERO_PERMISSIONS=0`), where no
/// verdict is ever asked for.
pub(super) const CLASSIFIER_VIEW_NOTE_OFF: &str =
    "Tool permissions are disabled — no classifier runs.";

/// The display-column budget for each section's opening paragraph in the
/// page's **abridged system prompt** (`ui::classifier_view::abridge_prompt`)
/// — three rows of an 80-column terminal, enough to read what a section is
/// about. The overflowing line is cut and closed with `…`; the section's
/// remaining lines fold into the counted row under it. Shown whole, the
/// 3.6 KB rubric would push the live block the page exists for sixty rows
/// down (`docs/permissions.md`).
pub(super) const CLASSIFIER_PROMPT_PEEK_COLS: usize = 240;

/// The lead of the dim `… +N lines` row that counts a section's folded
/// lines — the tool cell's [`TOOL_MORE_PREFIX`] idiom at the page's own
/// indent and without its `(ctrl+o to expand)`, there being nothing to
/// expand it into.
pub(super) const CLASSIFIER_PROMPT_MORE_PREFIX: &str = "… ";

/// The dim placeholder under the `## Action to review` header that closes
/// the page's `user:` message: the request's real shape ends on the one
/// action being judged, which is only known when a verdict is asked.
pub(super) const CLASSIFIER_ACTION_PLACEHOLDER: &str =
    "(the command or MCP call being judged — filled in when a verdict is asked)";

/// The inset of an entry's raw text (and attachment rows) under its tag.
pub(super) const CONTEXT_INDENT: &str = "  ";

/// The label of an attachment row under a user entry's text.
pub(super) const CONTEXT_IMAGE_LABEL: &str = "image: ";

/// The prefix of a native tool-call row under an assistant entry —
/// `→ name(arguments)`, the model's request in the raw wire form.
pub(super) const CONTEXT_TOOL_CALL_PREFIX: &str = "→ ";

/// Role-tag colours — the tool palette's hues (user blue, assistant green,
/// system amber, tool-result purple) so the roles scan apart at a glance. The
/// blue was the running tool's until that went grey ([`tool_running_color`]);
/// a role tag is not a running state, so it keeps the hue as its own value.
pub(super) fn context_user_color() -> Color {
    palette().link
}

pub(super) fn context_assistant_color() -> Color {
    tool_ok_color()
}

pub(super) fn context_system_color() -> Color {
    palette().warning
}

/// The `tool:` result-role tag and the `→ name(args)` tool-call lines under an
/// assistant entry — a distinct purple so the native tool round-trip reads
/// apart from plain assistant text.
pub(super) fn context_tool_color() -> Color {
    palette().purple
}

// --- /resume session picker (the other alternate-screen overlay) — codex's
// resume picker, sized down (docs/resume.md): the same slash-tiled title
// chrome as the transcript pager, a type-to-search line, dense one-line
// session rows (`❯ {age:12}{preview}`, the palette's selection-by-colour),
// and a bottom rule carrying `{selected+1}/{total}` over a dim key-hint row. ---

/// The picker's spaced-caps title, overlaid on the slash tiling.
pub(super) const RESUME_TITLE: &str = "R E S U M E";

/// The dim search-line placeholder while the query is empty (codex's).
pub(super) const RESUME_SEARCH_PLACEHOLDER: &str = "Type to search";

/// The search line's prefix once a query is typed.
pub(super) const RESUME_SEARCH_PROMPT: &str = "Search: ";

/// The picker's key-hint row (dim, under the separator). The search line's
/// own placeholder carries the type-to-search hint.
pub(super) const RESUME_HINTS: &str =
    " ↑/↓ select   enter resume   esc cancel   tab + ←/→ filter/sort";

/// The dim list placeholder when nothing was ever saved (codex's).
pub(super) const RESUME_NO_SESSIONS: &str = "No sessions yet";

/// The dim list placeholder when the query matches nothing (codex's).
pub(super) const RESUME_NO_MATCH: &str = "No results for your search";

/// The age column's width in the dense rows — codex's 12-col relative date.
pub(super) const RESUME_AGE_WIDTH: usize = 12;

/// The two-space inset shared by the search line and the placeholder rows
/// (the row marker is the same width, so everything lines up).
pub(super) const RESUME_INDENT: &str = "  ";

/// The selected row's marker; unselected rows get spaces (codex's `❯ `).
pub(super) const RESUME_MARKER: &str = "❯ ";

/// The selected row's full-width background tint — codex blends white over
/// the terminal background; a grey lift noticeably lighter than the
/// user-message block plays that role here.
pub(super) fn resume_selected_bg() -> Color {
    palette().selection_bg
}

/// The Tab-focused toolbar control's active value — codex's magenta.
pub(super) fn resume_focus_color() -> Color {
    palette().purple
}

/// The gap between the toolbar's Filter and Sort tab pairs (codex's).
pub(super) const RESUME_TOOLBAR_GAP: &str = "   ";

/// The smallest gap kept between the search text and the toolbar before the
/// toolbar compacts (and then drops).
pub(super) const RESUME_TOOLBAR_MIN_GAP: usize = 2;

// --- Inline `/model` picker (docs/llm.md). Unlike `/resume`, this one is
// **inline** — it replaces the composer in the bottom live region with its own
// `>` search prompt over a scrolling model list, framed by top/bottom rules
// like the input box. A gold header, the palette's cyan selection accent, a
// dim `[provider]` tag, a green ✓ on the active model, then a `(n/total)`
// counter and a `Model Name:` line — the shape of the user's mock. ---

/// The two-space inset shared by every picker row (search, list rows,
/// counter, name) so the content sits off the frame's left edge.
pub(super) const MODEL_INDENT: &str = "  ";

/// The glyph a framed view's top and bottom rules repeat across the width
/// (`model_rule`). Named so the hidden-cursor seat can tell a rule row from
/// text: a page with no `❯` seats after its last text row and skips the
/// rule under it (`layout::menu_marker_seat`, `docs/view-flow.md`).
pub(super) const VIEW_RULE: &str = "─";

/// The search line's prompt glyph (cyan), the `❯` the query types after.
pub(super) const MODEL_PROMPT: &str = "❯ ";

/// Cyan — the `❯` prompt and the selected row (the palette-selection accent).
pub(super) fn model_selected_color() -> Color {
    menu_selected_color()
}

/// The selected row's marker; unselected rows get spaces the same width.
pub(super) const MODEL_MARKER: &str = "→ ";

/// Light grey — an unselected model id (readable but quieter than the selection).
pub(super) fn model_id_color() -> Color {
    palette().text_muted
}

/// Dim — the `[provider]` tag, the counter, and the `Model Name:` line.
pub(super) fn model_meta_color() -> Color {
    tool_dim_color()
}

/// Green — the ✓ marking the currently-active model (shares the tool-ok green).
pub(super) fn model_active_color() -> Color {
    tool_ok_color()
}

/// The mark appended to the active model's row.
pub(super) const MODEL_ACTIVE_MARK: &str = " ✓";

/// The label opening the friendly-name line under the list.
pub(super) const MODEL_NAME_LABEL: &str = "Model Name: ";

/// The most model rows shown at once; longer lists scroll to keep the selection
/// **centered** (`centered_window`) so the user sees the models above and below
/// it, not just up to the edge it last crossed.
pub(super) const MODEL_MENU_MAX_ROWS: u16 = 10;

/// The list placeholder while the fetch is in flight.
pub(super) const MODEL_LOADING: &str = "Loading models…";

/// The list placeholder when the provider returned no models.
pub(super) const MODEL_NONE: &str = "No models available";

/// The list placeholder when the query matches nothing.
pub(super) const MODEL_NO_MATCH: &str = "No matching models";

/// The list placeholder when no provider has a key yet ([`ModelLoad::NeedsLogin`]) —
/// shown cyan (an actionable hint, not a red error) pointing at `/login`.
pub(super) const MODEL_LOGIN_HINT: &str = "No API key yet — run /login to add one";

/// The separator between the `(n/total)` counter and its trailing load status.
pub(super) const MODEL_STATUS_SEP: &str = "   ·   ";

/// The counter's dim suffix while other providers are still being fetched (the
/// list shows what's landed so far and keeps growing). See `docs/llm.md`.
pub(super) const MODEL_LOADING_MORE: &str = "loading more…";

/// The most display rows a failed provider's *reason* block may take under the
/// list. The collapsed `{provider} unavailable` note names the provider and
/// nothing else, so the reason is rendered beneath it — but a provider that
/// answers with an HTML page (an intercepting proxy, a captive portal) would
/// otherwise push the model list off the top of the frame.
pub(super) const MODEL_ERROR_MAX_ROWS: u16 = 3;

// --- The `/login` API-key onboarding flow (docs/llm.md). A two-step inline
// picker sharing the model picker's framed look and colours: step 1 lists the
// providers to choose from (headerless, like `/model`), step 2 collects the key
// masked under a periwinkle prompt. All styling reuses the `MODEL_*` consts
// (indent, `❯` prompt, cyan selection, dim meta, green ✓, `→` marker) plus the
// `LOGIN_*` strings/geometry below. ---

/// Every `/login` page title — `Use a subscription`, `Sign in to GitHub
/// Copilot`, `Enter your Agent Zero API key`. The palette accent the whole
/// picker family already selects with, so a title reads as *this* flow's own
/// heading rather than a fourth colour to learn.
pub(super) fn login_title_color() -> Color {
    model_selected_color()
}

/// The separator between a `/login` row's name and its configured status.
pub(super) const LOGIN_STATUS_SEP: &str = " · ";

/// What a provider / subscription row says about itself: whether this session
/// can actually reach it. Both states are **spelled out**, where a bare row
/// used to mean "no key yet" — a fact the reader could only take from the
/// *absence* of a mark two columns further right, which is a poor way to
/// answer the one question a sign-in list is opened to answer.
///
/// Split into a **mark** and a **label** because they are coloured
/// differently. The `✔` keeps the green the `/model` picker's ✓ wears
/// ([`model_active_color`]) — it is the thing worth finding down a list of
/// names — while its word, the `◯`, and the separator before them stay dim
/// ([`model_meta_color`]): a status is a fact about a row rather than an
/// alert, and colouring the whole tail made a list of facts read as a column
/// of them.
pub(super) const LOGIN_CONFIGURED_MARK: &str = "✔";
pub(super) const LOGIN_CONFIGURED_LABEL: &str = " configured";
pub(super) const LOGIN_UNCONFIGURED_MARK: &str = "◯";
pub(super) const LOGIN_UNCONFIGURED_LABEL: &str = " unconfigured";

/// The dim hint under the method step (the root: Esc closes).
pub(super) const LOGIN_METHOD_HINT: &str = "↑↓ navigate  enter select  escape/ctrl+c cancel";

/// The dim hint under the subscription list.
pub(super) const LOGIN_SUBSCRIPTION_HINT: &str = "↑↓ navigate  enter sign in  esc back";

/// The dim hint under the API-key provider list, below the `.env` path row.
pub(super) const LOGIN_PROVIDER_HINT: &str = "↑↓ navigate  enter select  esc back";

/// The list placeholder when the method filter matches nothing.
pub(super) const LOGIN_NO_METHOD_MATCH: &str = "No matching options";

/// The list placeholder when the subscription filter matches nothing.
pub(super) const LOGIN_NO_SUBSCRIPTION_MATCH: &str = "No matching subscriptions";

// --- The sign-in method choice (docs/chatgpt.md). A subscription offering two
// ways in — ChatGPT Codex's browser flow, or a device code for a headless
// machine — asks which before opening a page: a cyan title naming the
// subscription over two rows in the root's own dress, and no filter, since a
// question with two answers is not searched. ---

/// The choice's title, around the subscription's name:
/// `Select ChatGPT Codex login method:`.
pub(super) const LOGIN_SIGNIN_METHOD_TITLE_PREFIX: &str = "Select ";
pub(super) const LOGIN_SIGNIN_METHOD_TITLE_SUFFIX: &str = " login method:";

/// The dim hint under the choice — the root's own words: Esc steps back to
/// the subscription list here, which is what cancelling a choice means.
pub(super) const LOGIN_SIGNIN_METHOD_HINT: &str = LOGIN_METHOD_HINT;

// --- The device-code page (docs/copilot.md). A subscription sign-in shows the
// provider's one-time code in a rounded box over the URL to enter it at, and
// waits. No browser is launched — the URL is text the user opens themselves. ---

/// The device page's title prefix — `Sign in to {provider}`.
pub(super) const DEVICE_TITLE_PREFIX: &str = "Sign in to ";

/// The device page's instruction, in two rows: `Visit {uri}` then this.
pub(super) const DEVICE_VISIT_PREFIX: &str = "Visit ";
pub(super) const DEVICE_ENTER_LINE: &str = "and enter this one-time code";

/// A **browser** sign-in's twin of the pair above (`docs/chatgpt.md`): there
/// is no code to follow the link, because the browser redirects back to this
/// process on its own. The URL takes the row **bare** — it is an OSC 8
/// hyperlink (`docs/links.md`), and a verb in front of a clickable link is a
/// word doing nothing but pushing the target off the start of its own row —
/// so only the row beneath it is needed, saying what happens next instead of
/// pointing at a box that isn't there.
pub(super) const DEVICE_RETURN_LINE: &str = "Sign in there — this window continues by itself";

/// The code box's extra indent past [`MODEL_INDENT`], and its rounded corners.
pub(super) const DEVICE_BOX_INDENT: &str = "   ";
pub(super) const DEVICE_BOX_TOP_LEFT: &str = "╭";
pub(super) const DEVICE_BOX_TOP_RIGHT: &str = "╮";
pub(super) const DEVICE_BOX_BOTTOM_LEFT: &str = "╰";
pub(super) const DEVICE_BOX_BOTTOM_RIGHT: &str = "╯";
pub(super) const DEVICE_BOX_HORIZONTAL: &str = "─";
pub(super) const DEVICE_BOX_VERTICAL: &str = "│";
/// The padding inside the box, each side of the code.
pub(super) const DEVICE_BOX_PAD: &str = "  ";

/// The code itself — bright and bold, the one thing on the page to transcribe.
pub(super) fn device_code_color() -> Color {
    ai_color()
}

/// The verification URL — **dim**, like the sentence under it. The one thing
/// on this page the eye should land on is the code in its box; an accented URL
/// competed with it, and the URL is an instruction rather than a choice. It is
/// still a clickable hyperlink; the underline is what says so, which is why
/// linking it does not repaint it (`docs/links.md`).
pub(super) fn device_uri_color() -> Color {
    model_meta_color()
}

/// The status line's two states, and the countdown clause appended to the wait.
pub(super) const DEVICE_STARTING: &str = "Requesting a code…";
pub(super) const DEVICE_WAITING: &str = "Waiting for approval…";
pub(super) const DEVICE_EXPIRES_PREFIX: &str = " · expires in ";
pub(super) const DEVICE_EXPIRED: &str = " · code expired";

/// A browser sign-in's twin of the two states above: nothing is requested
/// from a provider first (the link is built locally), and what is waited on
/// is the browser coming back, not a code being approved.
pub(super) const DEVICE_LINK_STARTING: &str = "Opening the sign-in…";
pub(super) const DEVICE_LINK_WAITING: &str = "Waiting for the browser…";
pub(super) const DEVICE_LINK_EXPIRED: &str = " · timed out";

/// The dim hint under the sign-in page — the code page's, and the browser
/// page's, which copies the link instead.
pub(super) const DEVICE_HINT: &str = "c copy code  esc cancel";
pub(super) const DEVICE_LINK_HINT: &str = "c copy link  esc cancel";

/// Where the device page's **hidden** cursor parks — the frame's first content
/// row, the title.
///
/// The caret is invisible here ([`cursor_visible`]), but a terminal with a
/// cursor-trail animation (kitty and kin) still animates toward wherever it is
/// *seated*, and this page re-arms a frame every 32 ms to tick its countdown.
/// So the seat must be somewhere harmless and somewhere **still**: the code
/// box is neither — anything the emulator paints at the cursor lands on the
/// one thing the page exists to be read from, and the box's row moves when the
/// page grows from the waiting shape to the full one.
pub(super) const DEVICE_CURSOR_ROW: u16 = 2;

/// The dim hint under the provider list, prefixing the real `.env` path
/// (`onboarding.env_path`) so it names where the key actually lands.
pub(super) const LOGIN_PROVIDER_HINT_PREFIX: &str = "Keys are saved to ";

/// The dim hint under the key-entry field.
pub(super) const LOGIN_KEY_HINT: &str = "Enter to save · Esc to go back";

/// The hint under a **host** field ([`KeyKind::Host`]): an empty Enter saves
/// the default the field shows (`docs/ollama.md`).
///
/// [`KeyKind::Host`]: crate::app::KeyKind::Host
pub(super) const LOGIN_HOST_HINT: &str =
    "Enter to save (empty = the default shown) · Esc to go back";

/// What the key step's link is introduced by: the page a **key** is created
/// on, or — for a provider that takes none — where the server the host field
/// asks about comes from (`docs/ollama.md`).
pub(super) const LOGIN_KEY_URL_PREFIX: &str = "Create a key at ";
pub(super) const LOGIN_HOST_URL_PREFIX: &str = "Install it from ";

/// The dim placeholder shown in the key field before anything is entered.
pub(super) const LOGIN_KEY_PLACEHOLDER: &str = "paste your API key, then press Enter";

/// The glyph each entered key character is masked to.
pub(super) const LOGIN_MASK_CHAR: char = '•';

/// The list placeholder when the provider filter matches nothing.
pub(super) const LOGIN_NO_MATCH: &str = "No matching providers";

/// The most provider rows shown at once (longer lists scroll to keep the
/// selection **centered**, like the `/model` list — `centered_window`).
pub(super) const LOGIN_MENU_MAX_ROWS: u16 = 8;

// --- The inline `/settings` menu (docs/settings.md). The `/model` picker's
// framed shape with one extra row — the key hint under the description — and a
// value column instead of a `[provider]` tag. Every colour is borrowed from the
// `MODEL_*` set so the three inline pickers read as one family. ---

/// The most setting rows shown at once; a longer (or unfiltered) list scrolls
/// to keep the selection **centered**, like the `/model` list.
pub(super) const SETTINGS_MENU_MAX_ROWS: u16 = 10;

/// Columns between the longest visible label and the value column, so the
/// values line up in a block (the palette's `MENU_DESC_COL` idea, sized to the
/// visible rows rather than pinned).
pub(super) const SETTINGS_VALUE_GAP: usize = 3;

/// Light grey — a value that is *on* (`true`, a retry count, a temperature):
/// readable, and the same weight an unselected model id carries.
pub(super) fn settings_value_color() -> Color {
    model_id_color()
}

/// Dim — a value that is *off* (`false`, `default`) or unavailable, so a
/// glance down the column shows what is actually doing something.
pub(super) fn settings_value_off_color() -> Color {
    model_meta_color()
}

/// The values rendered in the dim "off" colour.
pub(super) const SETTINGS_OFF_VALUES: &[&str] = &["false", "default", "0", "disabled"];

/// The key hint pinned under the description — the menu's whole grammar.
pub(super) const SETTINGS_HINT: &str = "Type to search · Enter/Space to change · Esc to cancel";

/// The list placeholder when the search matches no setting.
pub(super) const SETTINGS_NO_MATCH: &str = "No matching settings";

/// The row (within the menu's framed area) the `❯` search line sits on — top
/// rule (0), gap (1), search (2). Shared by [`render_settings`] and
/// [`cursor_position`] so the cursor lands on the query.
pub(super) const SETTINGS_SEARCH_ROW: u16 = 2;

// --- the inline /skills menu (docs/skills.md). Deliberately the /settings
// menu's twin rather than a new shape: it reuses that menu's frame, its
// value column geometry (SETTINGS_VALUE_GAP) and its two-tone value colours
// wholesale, so the only consts it needs of its own are the words. ---

/// The value column's two states. `disabled` is already in
/// [`SETTINGS_OFF_VALUES`], so it dims through the same rule that dims a
/// `false` knob — one look down the column shows what is live.
pub(super) const SKILLS_ON_VALUE: &str = "enabled";
pub(super) const SKILLS_OFF_VALUE: &str = "disabled";

/// The key hint pinned under the description — the menu's whole grammar.
pub(super) const SKILLS_HINT: &str =
    "Type to search · Enter/Space to enable/disable · Esc to cancel";

/// The list placeholder when the search matches no skill.
pub(super) const SKILLS_NO_MATCH: &str = "No matching skills";

/// The row (within the menu's framed area) the `❯` search line sits on — top
/// rule (0), gap (1), search (2). Shared by `render_skills_menu` and
/// [`cursor_position`](super::layout::cursor_position) so the caret lands on
/// the line drawn.
pub(super) const SKILLS_SEARCH_ROW: u16 = 2;

/// The list placeholder when **nothing was discovered** — an empty list's only
/// real question is "where should I put one?", so the answer is the row.
pub(super) const SKILLS_NONE_FOUND: &str = "No skills found. Add one at:";

/// The note under the search line when skills are off for the whole session
/// (the `/settings` **Skills** row): the rows still browse and toggle, but
/// nothing here reaches the model until that row goes back on. The `/hooks`
/// menu's disabled-note rule.
pub(super) const SKILLS_SESSION_OFF: &str =
    "Skills are off for this session — turn them on in /settings";

// --- the inline /mascot picker (docs/mascot.md). The /settings menu's twin
// again: it reuses the family's frame, marker, prompt and colours, adding
// only its own words — the page's centre is the live banner preview, drawn
// by the header's own builder so it can never disagree with the real thing. ---

/// The key hint pinned under the preview — the picker's whole grammar.
pub(super) const MASCOT_HINT: &str = "Type to search · Enter to choose · Esc to cancel";

/// The list placeholder when the search matches no mascot.
pub(super) const MASCOT_NO_MATCH: &str = "No matching mascots";

/// The row (within the picker's framed area) the `❯` search line sits on —
/// top rule (0), gap (1), search (2). Shared by `render_mascot_picker` and
/// [`cursor_position`](super::layout::cursor_position) so the caret lands on
/// the line drawn.
pub(super) const MASCOT_SEARCH_ROW: u16 = 2;

/// The cap on the picker's visible list rows (the settings window's size —
/// the eight-mascot catalog never actually windows today).
pub(super) const MASCOT_MENU_MAX_ROWS: u16 = SETTINGS_MENU_MAX_ROWS;

// --- the inline /spinner picker (docs/spinner.md). The /mascot picker's frame
// — the MODEL_* accents, the same hint grammar — over the spinner-style
// catalog, plus a name column that leaves room for each row's live spinner.
// The styles' own frames and colours sit with the status indicator's consts
// below (`SPINNER_*_FRAMES`). ---

/// The key hint pinned under the preview — the picker's whole grammar.
pub(super) const SPINNER_HINT: &str = "Type to search · Enter to choose · Esc to cancel";

/// The list placeholder when the search matches no style.
pub(super) const SPINNER_NO_MATCH: &str = "No matching spinners";

/// The row (within the picker's framed area) the `❯` search line sits on —
/// top rule (0), gap (1), search (2). Shared by `render_spinner_picker` and
/// [`cursor_position`](super::layout::cursor_position) so the caret lands on
/// the line drawn.
pub(super) const SPINNER_SEARCH_ROW: u16 = 2;

/// The cap on the picker's visible list rows (the settings window's size —
/// the nine-style catalog never actually windows today).
pub(super) const SPINNER_MENU_MAX_ROWS: u16 = SETTINGS_MENU_MAX_ROWS;

/// Columns between the widest visible style name and the spinner column, so
/// the live spinners line up down the list.
pub(super) const SPINNER_MENU_GAP: usize = 3;

/// The verbs the picker's sample status line wears — the turn verbs' first
/// pair, so the preview reads like a first turn's line.
pub(super) const SPINNER_PREVIEW_VERB: &str = "Working";
pub(super) const SPINNER_PREVIEW_DONE_VERB: &str = "Done";

// --- The inline `/theme` picker (docs/theme.md). The `/spinner` picker's
// frame — its search line, its `→` marker, its counter, description and hint
// — over one row per colour theme, each wearing a swatch of its own accents,
// and a preview built from REAL cells (a user bubble, an `Edit` diff cell, an
// assistant reply with a code block) rendered under the highlighted theme
// through the conversation's own builders. The palettes themselves live in
// `ui::palette`; only the picker's words and geometry are here. ---

/// The key hint pinned under the description — the picker's whole grammar.
pub(super) const THEME_HINT: &str = "Type to search · Enter to choose · Esc to cancel";

/// The list placeholder when the search matches no theme.
pub(super) const THEME_NO_MATCH: &str = "No matching themes";

/// The row (within the picker's framed area) the `❯` search line sits on —
/// top rule (0), gap (1), search (2). Shared by `render_theme_picker` and
/// [`cursor_position`](super::layout::cursor_position) so the caret lands on
/// the line drawn.
pub(super) const THEME_SEARCH_ROW: u16 = 2;

/// The cap on the picker's visible list rows (the settings window's size —
/// the eleven-theme catalog windows by one row).
pub(super) const THEME_MENU_MAX_ROWS: u16 = SETTINGS_MENU_MAX_ROWS;

/// Columns between the widest visible theme name and the swatch column, so
/// the swatches line up down the list.
pub(super) const THEME_MENU_GAP: usize = 3;

/// One swatch cell — a row wears five, in its own theme's accent, link,
/// success, warning and error colours, so the palettes compare at a glance
/// the way the `/spinner` rows' live spinners do.
pub(super) const THEME_SWATCH: &str = "●";

/// The preview's sample conversation: the user asks for a change, an `Edit`
/// call makes it (a real numbered diff cell — the code theme, the diff tints,
/// the inline-diff marks and the success bullet in one cell), and the reply
/// names the file in inline code over a fenced code line. Rendered by the
/// conversation's own builders under the highlighted theme, so the preview
/// and the conversation can never disagree.
pub(super) const THEME_PREVIEW_USER: &str = "Rename the greeting in greet.py";
pub(super) const THEME_PREVIEW_TOOL: &str = "Edit";
pub(super) const THEME_PREVIEW_PATH: &str = "greet.py";
pub(super) const THEME_PREVIEW_OLD: &str = "def greet(name):\n    print(\"Hello, world\")\n";
pub(super) const THEME_PREVIEW_NEW: &str = "def greet(name):\n    print(f\"Hello, {name}\")\n";
pub(super) const THEME_PREVIEW_REPLY: &str = "Done — `greet.py` greets by name now:\n```python\ngreet(\"Alter Zero\")  # Hello, Alter Zero\n```";

// --- the read-only /donate page (docs/donate.md). The /hooks menu's frame —
// the family's indent, its `❯` marker, the dim meta ink — over the const
// donation-address catalog, each address inside the /login device page's
// rounded box (the `DEVICE_BOX_*` glyphs). Only its own words and the three
// colours below live here; the title borrows the banner's gradient. ---

/// The heart leading the title — the sponsor glyph, in a span of its own so
/// it keeps a colour of its own beside the gradient-washed name.
pub(super) const DONATE_HEART: &str = "♥ ";

/// The title's verb; the app's name ([`HEADER_NAME`]) follows it, so the
/// page reads `Support Alter Zero` from the one place the name lives.
pub(super) const DONATE_TITLE_PREFIX: &str = "Support ";

/// The dim rows under the title: what the project is, what a donation does.
/// Wrapped, never cut (`docs/view-flow.md`).
pub(super) const DONATE_BLURB: &str = "Free and open source, developed in the open. \
If it earns a place in your terminal, a donation keeps the work going — thank you.";

/// The amber caution over the hint — the one irreversible mistake the page
/// can lead to. It points at the rows rather than restating them, so a
/// catalog entry (or a network) added later is covered without touching
/// it: the entries say where their address is reachable, and this says
/// nowhere else is.
pub(super) const DONATE_CAUTION: &str = "Send each coin only over a network listed under \
its address — a transfer on any other network cannot be recovered.";

/// The dim key hint — the page's whole grammar.
pub(super) const DONATE_HINT: &str = "↑↓ navigate  enter/c copy address  esc close";

/// Columns between a row's ticker and its coin name.
pub(super) const DONATE_LABEL_GAP: &str = "  ";

/// The networks caption's lead-in when the address is reachable on exactly
/// one network, and when it is reachable on several. Two constants rather
/// than one plus an `s`: the label agrees with what it introduces, and a
/// `Network(s):` hedge would read as generated text on the one page a
/// reader is checking character by character.
pub(super) const DONATE_NETWORK_LABEL: &str = "Network: ";
pub(super) const DONATE_NETWORKS_LABEL: &str = "Networks: ";

/// What joins the network names in that caption.
pub(super) const DONATE_NETWORK_SEPARATOR: &str = ", ";

/// The heart's colour — the palette's red, the one place that hue means
/// affection rather than failure.
pub(super) fn donate_heart_color() -> Color {
    error_color()
}

/// An address — bright and bold like the device page's one-time code
/// ([`device_code_color`]): the one thing on the page to transcribe.
pub(super) fn donate_address_color() -> Color {
    device_code_color()
}

/// The caution's amber — the ask review's warning hue
/// ([`ask_warning_color`]), on a page where everything else is dim or
/// bright, so the eye lands on the one line that can go wrong first.
pub(super) fn donate_caution_color() -> Color {
    ask_warning_color()
}

// --- the read-only /export page (docs/export.md). The /donate page's frame
// — the family's indent, its `❯` marker, the dim meta ink — over the two
// export targets, the highlighted row's description under the list (the
// /settings shape).

/// The page's title.
pub(super) const EXPORT_TITLE: &str = "Export conversation";

/// The dim rows under the title: what the export *is*. Wrapped, never cut
/// (`docs/view-flow.md`).
pub(super) const EXPORT_BLURB: &str = "The whole transcript as plain text — every message and \
every tool call's full output, as Ctrl+O shows it.";

/// The clipboard row's description.
pub(super) const EXPORT_CLIPBOARD_DESC: &str = "Copies the transcript to the system clipboard.";

/// The file row's description: the file's shape, then `into {cwd}.` — the
/// directory as the footer shows it ([`EXPORT_FILE_DESC_DIR_FALLBACK`] before
/// the boundary has injected one).
pub(super) const EXPORT_FILE_DESC_PREFIX: &str = "Writes conversation-YYYY-MM-DD-HHMMSS.txt into ";

/// The directory the file row names when no session info has been injected
/// yet (the pure core never reads the environment).
pub(super) const EXPORT_FILE_DESC_DIR_FALLBACK: &str = "the working directory";

/// The dim key hint — the page's whole grammar.
pub(super) const EXPORT_HINT: &str = "↑↓ navigate  enter select  esc close";

// --- the read-only /hooks menu (docs/hooks-menu.md). It reuses the picker
// family's accents — model_selected_color() for the selection, model_id_color()
// for unselected labels, model_meta_color() for everything dim,
// hooks_title_color() for the titles, border_color() for the frame and the
// detail page's command box. ---

/// Every level's title colour — the **cyan** its `/mcp` twin wears
/// ([`mcp_title_color`]). The headline is the row that answers "where am I?"
/// in a menu you walk several levels deep, so both menus land the eye the
/// same way.
pub(super) fn hooks_title_color() -> Color {
    model_selected_color()
}

/// The events-level title.
pub(super) const HOOKS_TITLE: &str = "Hooks";

/// The detail page's title.
pub(super) const HOOKS_DETAIL_TITLE: &str = "Hook details";

/// The most list rows shown at once — the reference `Select`'s visible-option
/// count; a longer list scrolls to keep the selection **centered**
/// (`centered_window`, the `/model` list's), the window's edge rows wearing
/// the [`HOOKS_UP_MARKER`]/[`HOOKS_DOWN_MARKER`] overflow arrows.
pub(super) const HOOKS_MENU_MAX_ROWS: usize = 5;

/// The selected row's marker (the permission prompt's `❯`); unselected rows
/// get spaces the same width.
pub(super) const HOOKS_MARKER: &str = "❯ ";

/// The scrolled window's edge markers — more rows above / below.
pub(super) const HOOKS_UP_MARKER: &str = "↑ ";
pub(super) const HOOKS_DOWN_MARKER: &str = "↓ ";

/// Columns between the widest visible label and the description column, so
/// the summaries line up in a block (the `/settings` value column's idea).
pub(super) const HOOKS_DESC_GAP: usize = 3;

/// The read-only banner under the count (events level only) — the reference's
/// info line with our file and assistant names, its docs link swapped for the
/// format's own doc.
pub(super) const HOOKS_INFO: &str = "ℹ This menu is read-only. To add or modify hooks, \
edit hooks.json directly or ask alter-zero. See docs/hooks.md";

/// The note when the file holds hooks but the session has them off
/// (`/settings`, `ALTER_ZERO_HOOKS=0`) — the reference's restricted-by-policy
/// slot, red because every listed guard is currently not running.
pub(super) const HOOKS_DISABLED_NOTE: &str =
    "Hooks are disabled this session — enable them in /settings";

/// The empty state (an event with nothing configured), two dim lines.
pub(super) const HOOKS_EMPTY: &str = "No hooks configured for this event.";
pub(super) const HOOKS_EMPTY_HINT: &str =
    "To add hooks, edit hooks.json directly or ask alter-zero.";

/// The list levels' key hint, and the detail/empty pages' Esc-only one.
pub(super) const HOOKS_HINT: &str = "Enter to confirm · Esc to cancel";
pub(super) const HOOKS_DETAIL_HINT: &str = "Esc to go back";

/// The single-source tags — one `hooks.json`, one origin (the reference has
/// user/project/local/plugin layers; ours is user-level only,
/// `docs/hooks.md`): the `[User]` matcher-row prefix, the hook rows'
/// `User Settings` description, and the detail page's `Source:` label.
pub(super) const HOOKS_SOURCE_INLINE: &str = "User";
pub(super) const HOOKS_SOURCE_HEADER: &str = "User Settings";
pub(super) const HOOKS_SOURCE_LABEL: &str = "User settings";

/// The detail page's field-name column: `Source:` (the widest) plus its pad,
/// so the values line up (`Event:    PreToolUse`).
pub(super) const HOOKS_FIELD_COL: usize = 10;

/// The `/trust` review menu (`docs/project-config.md`) — the hooks menu's
/// sibling, borrowing the `HOOKS_*` marker and frame; only its own
/// vocabulary lives here.
///
/// The page title (the root's display path is appended).
pub(super) const TRUST_TITLE: &str = "Project trust";

/// The banner under the status: what approving means, in one breath.
pub(super) const TRUST_INFO: &str = "ℹ A project's .alter-zero config can run commands — hooks \
fire on lifecycle events and MCP servers are spawned processes — so nothing below runs until \
you approve it. Approval is pinned to each file's content: an edited file asks again. See \
docs/project-config.md";

/// The two status headlines beside `Status:`.
pub(super) const TRUST_STATUS_TRUSTED: &str = "trusted";
pub(super) const TRUST_STATUS_UNTRUSTED: &str = "not trusted";

/// The per-file badges: waiting on approval, recorded, or unreadable.
pub(super) const TRUST_BADGE_PENDING: &str = "pending approval";
pub(super) const TRUST_BADGE_TRUSTED: &str = "trusted";
pub(super) const TRUST_BADGE_ERROR: &str = "won't parse";

/// The option rows' labels — what Enter applies.
pub const TRUST_APPROVE_LABEL: &str = "Trust this project's config";
pub const TRUST_REVOKE_LABEL: &str = "Revoke trust";

/// The most item lines shown per file before the fold — enough to review a
/// real config, few enough that a huge one can't push the options off the
/// terminal (the permission prompt's body-cap instinct).
pub const TRUST_MENU_MAX_ITEMS: usize = 8;

/// The empty state: nothing found, so name exactly where the layer looks.
pub(super) const TRUST_EMPTY: &str = "No project config found. Add one at:";

/// The key hints — options on offer, or Esc-only when there are none.
pub(super) const TRUST_HINT: &str = "Enter to apply · Esc to close";
pub(super) const TRUST_CLOSE_HINT: &str = "Esc to close";

/// The `/mcp` manager (`docs/mcp.md`) — the hooks menu's twin, so it borrows
/// the whole `HOOKS_*` frame (marker, overflow arrows, window size, hints)
/// and adds only its own vocabulary.
///
/// The list page's title + hint.
pub(super) const MCP_TITLE: &str = "Manage MCP servers";
pub(super) const MCP_LIST_HINT: &str = "↑/↓ to navigate · Enter to confirm · Esc to cancel";
pub(super) const MCP_SERVER_HINT: &str = "↑/↓ to navigate · Enter to select · Esc to back";
/// The empty list names both config files — "why isn't my server here?" is
/// its only question (the skills empty-state posture).
pub(super) const MCP_NONE_FOUND: &str = "No MCP servers configured. Add one at:";
/// The detail page's field column (`Config location:  ` is the widest).
pub(super) const MCP_FIELD_COL: usize = 18;

/// Every page's headline — **cyan**, shared with its `/hooks` twin
/// ([`hooks_title_color`]). This is a *walk* four pages deep, and the
/// headline is the only row that answers "where am I?", so it is the row the
/// eye must land on first (`docs/mcp.md`).
pub(super) fn mcp_title_color() -> Color {
    model_selected_color()
}

/// The separator between a server row's name, status and tool count. It is
/// **chrome, not status**, so it stays [`model_meta_color`] dim at every
/// state — only the glyph carries the status colour. Painting it with the
/// glyph made a connected row's first `·` green while its second stayed dim.
pub(super) const MCP_ROW_SEPARATOR: &str = " · ";

/// Both detail pages' two-tone: **every field label is bright** (`Status:`,
/// `Tool name:`, `Description:`, `Parameters:`, a parameter's `● name`), and
/// the values they introduce are quiet by default — an address, a path, a
/// protocol revision, a count. The label is the column the eye runs down;
/// the value is what it stops on once it has found the row.
pub(super) fn mcp_detail_label_color() -> Color {
    ai_color()
}
pub(super) fn mcp_detail_value_color() -> Color {
    model_meta_color()
}

/// The exception: a value that is itself the answer to "is this server
/// working, and what can it do?" — the `Status:`/`Auth:` words and the
/// capability list — keeps the light the addresses around it give up.
pub(super) fn mcp_detail_state_color() -> Color {
    ai_color()
}

/// The tool's own description — **half white**: a step down from the label
/// announcing it, a clear step up from the schema prose below it. It is the
/// one paragraph on the page written *for* a reader rather than derived from
/// a schema, so it must not read as boilerplate; full white made it shout
/// over the labels that organise the page.
pub(super) fn mcp_description_color() -> Color {
    model_id_color()
}

/// The tool page's compact field spacing: one space after the label, not the
/// server page's [`MCP_FIELD_COL`] pad. Its two labels (`Tool name:`,
/// `Full name:`) are the same width, so they line up on their own and the
/// value sits where the eye already is instead of across a gulf.
pub(super) const MCP_TOOL_FIELD_GAP: &str = " ";

/// The `Parameters:` listing's bullet and its continuation-row indent (the
/// same width, so a wrapped parameter hangs under its own name).
pub(super) const MCP_PARAM_BULLET: &str = "  ● ";
pub(super) const MCP_PARAM_INDENT: &str = "    ";
/// The auth page's fixed lines.
pub(super) const MCP_AUTH_BROWSER_NOTE: &str = "*  A browser window will open for authentication";
pub(super) const MCP_AUTH_COPY_NOTE: &str =
    "If your browser doesn't open automatically, copy this URL manually (c to copy)";
pub(super) const MCP_AUTH_PASTE_NOTE: &str =
    "If the redirect page shows a connection error, paste the URL from your browser's address bar:";
pub(super) const MCP_AUTH_PROMPT: &str = "URL > ";
pub(super) const MCP_AUTH_RETURN_NOTE: &str =
    "Return here after authenticating in your browser. Press Esc to go back.";
pub(super) const MCP_AUTH_WAITING: &str = "Preparing the authorization request…";
pub(super) const MCP_AUTH_SUBMITTED: &str = "Checking the pasted URL…";

/// The closing direction on the detail page.
pub(super) const HOOKS_MODIFY_NOTE: &str =
    "To modify or remove this hook, edit hooks.json directly or ask alter-zero to help.";

// --- Transcript timestamps (Ctrl+O view only). Only the *user* message shows
// its wall-clock stamp: dim, right-aligned on its own line below the message
// (`hh:mm AM/PM`). AI replies, tools, and turn summaries record a stamp too but
// never display it; the inline view never shows any. See docs/timestamps.md. ---

/// Dim grey — the user message's right-aligned timestamp in the Ctrl+O transcript.
pub(super) fn timestamp_color() -> Color {
    tool_dim_color()
}

// --- Live status indicator (codex / Claude-Code style). While a turn is in
// flight a status line sits in the strip above the box (with a blank gap row
// between it and the box's top rule):
// `⣤⣀⣀⣀⣀⣀⣀⣀ {verb}… ({elapsed} · {↓|↑} {n} tokens · Thinking for {m})`. The
// line opens with the session's **spinner style** — by default `gravity`, a
// ball hopping along a braille track (see [`spinner_spans`] and
// docs/spinner.md); before there was a catalog it was always the **comet**
// (a Larson-scanner sweep: a white head dragging a fading grey tail back and
// forth between dim walls, one frame per `SPINNER_INTERVAL`), which is still
// what the `SPINNER_*` consts just below describe. The working verb is picked
// per-turn (in `App`) and its white text carries a codex-style **shimmer**: a
// bright-white band sweeps across the white-grey text (see [`shimmer_spans`],
// ported from openai/codex `tui/src/shimmer.rs`). The elapsed / thinking / done
// times are humanized by [`format_elapsed`] (`45s`, `1m 30s`, `1h 1m`). On
// finish a dim, bullet-less `{done verb} for {n}` summary commits to scrollback (a
// `HistoryItem::Summary`). See docs/status-indicator.md. ---

/// White — the comet's head (matches the codex/Claude-Code white status text).
pub(super) fn status_color() -> Color {
    ai_color()
}

/// The comet-spinner animation frames (a Larson-scanner sweep, ten frames):
/// the bright head (`●`) drags a two-cell fading tail (`•` then `·`) out to
/// the right wall and back across to the **left wall** (flush against `(` —
/// no wasted leading cell), the tail whipping around behind it at each
/// bounce (a tail cell the head overlaps is hidden under it). Every frame is
/// the same width, so the verb after it never jitters.
pub(super) const SPINNER_FRAMES: &[&str] = &[
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
pub(super) const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// The comet's bright bold head in a [`SPINNER_FRAMES`] frame.
pub(super) const SPINNER_HEAD: char = '●';

/// The tail cell right behind the head; the `·` end (and everything else in
/// the frame — walls, empty track) fades to [`status_detail_color`].
pub(super) const SPINNER_TAIL_MID: char = '•';

/// Mid grey — the `•` tail cell, between the white head and the dim tail end.
pub(super) fn spinner_tail_color() -> Color {
    model_id_color()
}

/// How many spans [`spinner_spans`] emits (one per frame cell: the left wall,
/// six track cells, the right wall) — the verb's per-char spans start at this
/// index in the status line.
pub(super) const SPINNER_SPAN_COUNT: usize = 8;

/// Dim grey — the parenthesised metrics (`elapsed · tokens · thinking`).
pub(super) fn status_detail_color() -> Color {
    tool_dim_color()
}

/// Amber — the `retrying {n}/{max}` clause. A warning hue (One-Dark yellow),
/// distinct from the dim metrics and the error red: the request hasn't failed,
/// it's recovering. See `docs/llm.md`.
pub(super) fn status_retry_color() -> Color {
    palette().warning
}

/// Trailing ellipsis after the working verb (`Working…`).
pub(super) const STATUS_ELLIPSIS: &str = "…";

/// Arrow for output tokens while the reply streams.
pub(super) const STATUS_ARROW_DOWN: &str = "↓";

/// Arrow once a tool result is folded back in.
pub(super) const STATUS_ARROW_UP: &str = "↑";

/// The interrupt hint, the detail's final clause while a turn is in flight —
/// codex's `Esc to interrupt` discoverability hint, lowercased to match this
/// codebase's hint convention (`(ctrl+o to expand)`, `esc return`).
pub(super) const STATUS_INTERRUPT_HINT: &str = "esc to interrupt";

/// Dim grey — the committed `"{done verb} for {n}"` turn summary.
pub(super) fn status_done_color() -> Color {
    tool_dim_color()
}

/// The status line's row in the streaming strip.
pub(super) const STATUS_ROWS: u16 = 1;

/// A blank row between the status line and the box's top rule, so the status
/// never butts up against the box (mirrors the gap above, under the preview).
pub(super) const STATUS_GAP_ROWS: u16 = 1;

// --- The verb's shimmer wave (ported from openai/codex `shimmer_spans`): each
// char's colour blends from the white-grey base toward bright white by a
// raised-cosine band that sweeps the text once per `SHIMMER_SWEEP`. The wave's
// phase derives from the boundary-supplied `TurnStatus::elapsed`, keeping the
// renderer pure (codex reads a process clock instead). ---

/// The white-grey base of the shimmering verb text (codex's truecolor fallback
/// foreground) — dim enough that the bright band reads clearly.
pub(super) fn shimmer_base() -> Color {
    palette().shimmer_base
}

/// The bright white the band's crest blends toward.
pub(super) fn shimmer_highlight() -> Color {
    ai_color()
}

/// One full sweep of the band across the text (codex's `sweep_seconds`).
pub(super) const SHIMMER_SWEEP: Duration = Duration::from_secs(2);

/// Off-text run-in/out, in chars, so the band slides on and off the ends
/// instead of wrapping abruptly (codex's `padding`).
pub(super) const SHIMMER_PADDING: usize = 10;

/// The band's half-width in chars (codex's `band_half_width`).
pub(super) const SHIMMER_BAND_HALF_WIDTH: f32 = 5.0;

/// The crest's blend toward the highlight (codex blends `t * 0.9`).
pub(super) const SHIMMER_MAX_BLEND: f32 = 0.9;

// --- The spinner **styles** (docs/spinner.md). The comet above is one of
// them; `/spinner` swaps the frames the status line opens with, and the
// default is `gravity`, a track style drawn from the geometry below. The
// catalog's *identity* — names, order, descriptions — is `app::Spinner`; its
// *look* is here, beside every other styling decision, and `spinner_spans`
// maps one to the other. Two rules every style keeps, pinned by
// `ui::tests::status`: every frame of a style is the same width (so the verb
// after it never jitters) and every glyph is single-width
// (docs/table-streaming.md "Wide glyphs"). A one-cell style is drawn as one
// span — the glyph plus the separator space — in the comet head's white bold
// unless its own colour rule below says otherwise. The two **track** styles
// (`gravity`, `wave`) have no frame table at all: they draw themselves on a
// braille track from the geometry consts below. ---

/// The braille **track** the `gravity` and `wave` styles draw on: this many
/// cells of 2 × 4 dots each — sixteen dot columns by four dot rows inside one
/// text row, the resolution that lets a ball visibly hop and a wave visibly
/// roll where a glyph table could only step. Eight, the comet's footprint,
/// so the three wide styles share one width.
pub(super) const SPINNER_TRACK_CELLS: usize = 8;

/// `gravity` — one round trip of the ball along the track (left wall → right
/// wall → left) at constant speed, reversing hard at each wall.
pub(super) const SPINNER_GRAVITY_SWEEP: Duration = Duration::from_millis(2400);

/// One hop of the `gravity` ball, floor to floor — four per round trip, so it
/// touches down exactly as it meets each wall.
pub(super) const SPINNER_GRAVITY_HOP: Duration = Duration::from_millis(600);

/// `wave` — one crawl of the wave down the track and back.
pub(super) const SPINNER_WAVE_SWEEP: Duration = Duration::from_millis(3400);

/// How many wavelengths the `wave` travels each way per sweep — a whole
/// number, so the frame at the reversal is the frame it set out from.
pub(super) const SPINNER_WAVE_TRAVEL: f32 = 3.0;

/// The `wave`'s wavelength in dot columns — the whole track, so one crest and
/// one trough are always in view.
pub(super) const SPINNER_WAVE_LENGTH: f32 = 16.0;

/// `sparkle` — a spark opening into a heavy star and closing again. Its
/// colour walks the banner's [`header_gradient_start`] → [`header_gradient_end`]
/// with the bloom (cyan at the spark, blue at the full star), so the theme's
/// accent rides the status line.
pub(super) const SPINNER_SPARKLE_FRAMES: &[&str] =
    &["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

/// How long each `sparkle` frame shows — a 1.2 s bloom-and-fade.
pub(super) const SPINNER_SPARKLE_INTERVAL: Duration = Duration::from_millis(120);

/// `dots` — the classic braille spinner (cli-spinners' `dots`), white bold.
pub(super) const SPINNER_DOTS_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// How long each `dots` frame shows (the cli-spinners cadence).
pub(super) const SPINNER_DOTS_INTERVAL: Duration = Duration::from_millis(80);

/// `blocks` — the mascots' own three-quarter block glyphs (docs/mascot.md),
/// the missing quadrant walking clockwise, coloured along the banner's
/// gradient as it turns: cyan on the first frame, blue on the last.
pub(super) const SPINNER_BLOCKS_FRAMES: &[&str] = &["▙", "▛", "▜", "▟"];

/// How long each `blocks` frame shows.
pub(super) const SPINNER_BLOCKS_INTERVAL: Duration = Duration::from_millis(150);

/// `pulse` — one still `●` whose colour breathes [`spinner_pulse_dim`] →
/// [`spinner_pulse_bright`] → dim once per [`SPINNER_PULSE_PERIOD`]: a
/// raised-cosine swell at the running tool bullet's cadence (that bullet
/// itself blinks, docs/tool-pulse.md), taken up to white at the crest so it
/// reads as a status line's head rather than a resting cell.
pub(super) const SPINNER_PULSE_FRAMES: &[&str] = &["●"];

/// The bottom of the `pulse` breath — the palette's `pulse_dim`.
pub(super) fn spinner_pulse_dim() -> Color {
    tool_pulse_dim()
}

/// The crest of the `pulse` breath — the shimmer's white.
pub(super) fn spinner_pulse_bright() -> Color {
    shimmer_highlight()
}

/// One `pulse` breath — the tool bullet's blink period, so a pulsing status
/// line and a running tool cell move in step.
pub(super) const SPINNER_PULSE_PERIOD: Duration = TOOL_PULSE_PERIOD;

/// `bars` — a bar rising `▁` → `█` and falling back, brightening
/// [`spinner_bars_low`] → [`spinner_bars_high`] with its height, like a
/// level meter.
pub(super) const SPINNER_BARS_FRAMES: &[&str] = &[
    "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂",
];

/// How long each `bars` frame shows — a 0.84 s rise and fall.
pub(super) const SPINNER_BARS_INTERVAL: Duration = Duration::from_millis(60);

/// The lowest bar's grey — the running tool bullet's own grey
/// ([`tool_pulse_bright`]), so even `▁` reads.
pub(super) fn spinner_bars_low() -> Color {
    tool_pulse_bright()
}

/// The full bar's white.
pub(super) fn spinner_bars_high() -> Color {
    shimmer_highlight()
}

/// `line` — the classic ASCII spinner, for a font with none of the glyphs
/// above; white bold.
pub(super) const SPINNER_LINE_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// How long each `line` frame shows.
pub(super) const SPINNER_LINE_INTERVAL: Duration = Duration::from_millis(100);

// --- Slash-command palette. A scrolling, single-line-per-command list pinned
// **below the input box** (a third live-region band) whenever the input is a bare
// command token. Each row is `/name` padded to a column, then its description. The
// selection is shown **by colour**: the whole highlighted row lights up cyan — name
// *and* description the same colour — while the others are dimmed grey (no
// caret/arrow), Claude-Code style. Capped at `MENU_MAX_ROWS`; longer lists scroll
// to keep the selection visible (`menu_window`). ---

/// The most command rows shown at once; longer match lists scroll within this
/// (`menu_window` follows the selection, like the `@` file picker's cap). The
/// registry has outgrown the window — a bare `/` shows the first eight and ↓
/// scrolls the rest in (the `the_palette_shows_at_most_eight_commands` /
/// `the_palette_scrolls_down_to_the_last_command` tests pin both halves).
/// The cap is a **row budget**, not a match count: at widths where every
/// description fits its row the two are the same eight, and where a
/// description *wraps* (`menu_row_lines` — narrow terminals continue it on
/// rows indented to the description column instead of clipping it) the
/// window shows fewer whole commands so the band never outgrows the budget
/// (`menu_window_rows`).
pub(super) const MENU_MAX_ROWS: u16 = 8;

/// The column descriptions start at — names are padded out to here so the
/// descriptions line up in a tidy column regardless of command-name length.
pub(super) const MENU_DESC_COL: usize = 25;

/// Cyan — the **selected** row: its `/name` *and* description share this colour
/// (for consistency); the name is additionally bold.
pub(super) fn menu_selected_color() -> Color {
    palette().accent
}

/// Dim grey — an unselected row (name and description alike).
pub(super) fn menu_dim_color() -> Color {
    tool_dim_color()
}

/// The palette's single placeholder row when the `/token` matches no command.
pub(super) const MENU_NO_MATCH: &str = "No matching commands";

// --- The `@` file picker. A file list pinned **below the input box** (the
// palette's slot — the bands never show together), opened by an `@token` under
// the cursor — a port of codex's file-search popup. Each row is columned —
// `→ name  parent/  …  File|Dir` — reusing the palette's cyan-selected /
// dim-unselected colours, additionally **bolding the characters the query
// matched** (from `FileMatch.indices`). See docs/file-search.md. ---

/// The most file rows shown at once; longer match lists scroll to keep the
/// selection visible (`menu_window`), like the command palette.
pub(super) const FILE_MENU_MAX_ROWS: u16 = 8;

/// The picker's single placeholder row while a search is in flight.
pub(super) const FILE_MENU_SEARCHING: &str = "Searching…";

/// The picker's single placeholder row when the query matched nothing.
pub(super) const FILE_MENU_NO_MATCH: &str = "No matching files";

/// The selected row's arrow marker; unselected rows indent by its width.
pub(super) const FILE_MENU_MARKER: &str = "→ ";

/// The unselected rows' inset — the marker's width in spaces, so the name
/// column starts at the same place on every row.
pub(super) const FILE_MENU_INDENT: &str = "  ";

/// Gap between the name column and the parent-dir column (the name column is
/// the widest visible name plus this).
pub(super) const FILE_MENU_GAP: usize = 2;

/// Columns reserved at the right edge for the kind label — `File`/`Dir`
/// left-aligned in this slot, so both start at `width − FILE_MENU_TYPE_WIDTH`.
pub(super) const FILE_MENU_TYPE_WIDTH: usize = 6;

/// The parent-dir cell of a root-level entry (its parent is the cwd itself).
pub(super) const FILE_MENU_ROOT_DIR: &str = "./";

/// The kind-column label for a directory match.
pub(super) const FILE_MENU_DIR_LABEL: &str = "Dir";

/// The kind-column label for a file match.
pub(super) const FILE_MENU_FILE_LABEL: &str = "File";

// --- The `$` skill picker. A fourth band in the same slot, opened while the
// cursor sits in a usable `$mention`: one row per matching skill — the name
// column (widest visible name + `FILE_MENU_GAP`, matched characters bolded)
// then the skill's own description, `…`-cut at the width. It reuses the
// palette's colours and the file picker's marker/indent — the selection lights
// up cyan, the rest dim. See docs/skill-mentions.md. ---

/// The most skill rows shown at once; longer lists scroll to keep the
/// selection visible (`menu_window`), like the file picker.
pub(super) const SKILL_MENU_MAX_ROWS: u16 = 8;

/// The band's single placeholder row when the query matched no skill.
pub(super) const SKILL_MENU_NO_MATCH: &str = "No matching skills";

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
pub(super) const SHORTCUTS: &[(&str, &str)] = &[
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
    ("ctrl+t", " to cycle thinking"),
    ("shift+tab", " for permission mode"),
    ("$", " for skills"),
    ("ctrl+w/u/k", " to kill text"),
];

/// The display column where a row's second entry starts (the first entry is
/// padded out to here) — [`MENU_DESC_COL`]'s tidy-column idea. Sized so the
/// **widest** first-column variant keeps a readable gutter (the pairing once
/// put a 27-column entry in the first column, which at the old 28 left a
/// single space before its neighbour — one run-on line). The
/// `the_shortcuts_columns_keep_a_readable_gutter_in_every_state` test pins a
/// ≥ 2-column gutter across every context state; widen this with any new
/// entry that needs it.
///
/// Note that an entry added anywhere **reflows the pairing**, so a wide entry
/// that had been safe in a second column can land in a first one — check this
/// still holds when adding one.
pub(super) const SHORTCUTS_COL: usize = 30;

/// Cyan — an entry's key (the palette-selection accent).
pub(super) fn shortcuts_key_color() -> Color {
    menu_selected_color()
}

/// Dim grey — an entry's label (codex dims the whole overlay).
pub(super) fn shortcuts_text_color() -> Color {
    tool_dim_color()
}

// --- Esc-Esc backtrack (docs/backtrack.md). A primed first Esc takes the
// footer slot with a hint naming the second (codex's `esc_backtrack_hint`
// footer); the transcript overlay then highlights the selected user message
// by *reversing* its rows (codex's `user_message_style().reversed()`). ---

/// The primed hint's key, bold-cyan like the search-line hint keys.
pub(super) const BACKTRACK_HINT_KEY: &str = "esc";

/// The primed hint's dim label — codex's "esc again to edit previous message"
/// wording, minus the key it highlights separately.
pub(super) const BACKTRACK_HINT_LABEL: &str = " again to edit previous message";

/// The shortcuts-band `esc` entry while idle with a backtrack target: the
/// gesture replaces quit as Esc's idle meaning (see [`shortcuts_lines`]).
pub(super) const SHORTCUTS_BACKTRACK: (&str, &str) = ("esc esc", " to edit previous");

// --- Queued messages. While a turn streams, messages submitted with Enter (or
// Tab) join `App::queued` and are shown **above the box** (in the strip, just
// under the status line's gap) styled exactly like a sent user message — the
// `❯ ` bullet, the dark background, wrapped — so a queued follow-up reads like
// it is already on its way. The queue is a sequence of turn-batches: Enter
// appends to the current batch, **Tab opens a new one**, and the loop sends one
// batch per turn-end (a blank row divides the batches). See docs/queue.md. ---

/// Indent prefixed to every queued row, insetting the queue from the strip's
/// left edge; the dark user-message block starts after it.
pub(super) const QUEUED_INDENT: &str = "  ";

// --- The session-context footer: the dim `{model} · {cwd}` row pinned under
// the input box (codex's footer status line). See docs/footer.md. ---

/// Indent prefixed to the footer row (codex's `FOOTER_INDENT_COLS`).
pub(super) const FOOTER_INDENT: &str = "  ";

/// Separator between the footer's segments (codex's dim ` · `).
pub(super) const FOOTER_SEPARATOR: &str = " · ";

/// The footer's text colour — every segment dim, codex's no-theme-colours
/// status-line style.
pub(super) fn footer_color() -> Color {
    tool_dim_color()
}

/// The least gap kept between the footer's left chain and the permission
/// mode pinned at the row's right edge — the mode's reservation is its own
/// columns plus this, so the left content's `…` cut can never run into it
/// (`docs/permissions.md`).
pub(super) const FOOTER_MODE_GAP: usize = 2;

/// The **focused** shell indicator's fill: ↓ lights the footer's `{n} shell(s)`
/// segment on the palette-selection cyan and waits for the Enter that opens the
/// ↓ manager band (Claude-Code-style — see `docs/background.md`). Only that one
/// segment changes; the model / cwd / context-gauge segments stay dim.
pub(super) fn footer_focus_bg() -> Color {
    menu_selected_color()
}

/// The focused indicator's ink on that cyan fill — near-black, so the lit
/// segment reads as a chip rather than a smudge.
pub(super) fn footer_focus_fg() -> Color {
    palette().on_accent
}

// --- The transient toast: a one-line, self-clearing status message pinned just
// above the box (`Copied last message to clipboard`, `/resume is disabled …`).
// It occupies the bottom of the strip, directly above the box's top rule, and
// fades after a few seconds (the expiry timed at the I/O boundary). See
// docs/toast.md. ---

/// Indent prefixed to the toast row — the two-space inset shared with the
/// footer and the queued messages.
pub(super) const TOAST_INDENT: &str = "  ";

/// An info toast's colour (a confirmation / soft rejection) — dim, like the footer.
pub(super) fn toast_color() -> Color {
    tool_dim_color()
}

/// How far an error toast's red is mixed toward the info toast's dim — 0 the
/// full error red, 1 the dim. A third of the way keeps a failure red at a
/// glance without the four-second line outshouting the error bullet. See
/// docs/toast.md.
pub(super) const TOAST_ERROR_DIM_MIX: f32 = 0.35;

/// An error toast's colour (a failure — a `Copy failed`, a refused model
/// switch): the theme's error red mixed [`TOAST_ERROR_DIM_MIX`] of the way
/// toward [`toast_color`], a softer red than the error bullet's. A named
/// terminal red (the `ansi` theme) has nothing to mix and stays itself.
pub(super) fn toast_error_color() -> Color {
    lerp_color(error_color(), toast_color(), TOAST_ERROR_DIM_MIX)
}

// --- The Ctrl+R reverse history search line (codex's reverse-i-search footer,
// `chat_composer/history_search.rs::history_search_footer_line`). It takes the
// session footer's slot while a search is open, and the previewed match in the
// composer highlights the query occurrences. See docs/history-search.md. ---

/// The dim prompt opening the search line.
pub(super) const SEARCH_PROMPT: &str = "reverse-i-search: ";

/// Cyan — the query text and the accept/cancel hint keys (codex's `.cyan()`;
/// the palette-selection accent).
pub(super) fn search_query_color() -> Color {
    menu_selected_color()
}

/// The notice appended to the line when the query matches nothing — red, like
/// codex's `"  no match"`.
pub(super) const SEARCH_NO_MATCH: &str = "  no match";

/// How a previewed match's query occurrences light up in the input box
/// (codex's `REVERSED | BOLD` textarea highlight).
pub(super) const SEARCH_HIGHLIGHT: Modifier = Modifier::REVERSED.union(Modifier::BOLD);

// --- The `!` shell-mode footer hint. While the composer holds a `!command`
// the footer slot reads `Shell mode` in red (codex's light-red
// `shell_mode_footer_line`), displacing the `{model} · {cwd}` line. See
// docs/shell-command.md. ---

/// The shell-mode hint text.
pub(super) const SHELL_MODE_LABEL: &str = "Shell mode";

/// The hint's colour — red, like codex's `light_red()` (reuses our error red).
/// Also colours the `! ` bullet/prompt everywhere shell mode shows.
pub(super) fn shell_mode_color() -> Color {
    error_color()
}

/// The bullet opening a committed shell command's header (`! pwd` on the dark
/// user-style line) — and the composer prompt while shell mode is on (the
/// absorbed bang rendered back; same two columns as [`PROMPT`]).
pub(super) const SHELL_BULLET: &str = "! ";

// --- The startup header banner (docs/header.md, docs/mascot.md): the
// gradient mascot beside the title + cwd + hint, committed to scrollback at
// launch and re-emitted atop every full repaint (resize, `/clear`, a /mascot
// switch) so it survives the scrollback purge. Pure chrome, like the footer —
// never in `history`, so it never reaches the model or the `/resume` rollout.
// Borderless (no `─` rule row, no bare prompt, no model name) so the smoke
// resize counters don't see it. The art itself lives on `app::Mascot`. ---

/// The product name — bold in the banner's title row, gradient-washed in the
/// narrow one-line badge. Also the smoke suite's tier-independent banner
/// marker (`scripts/smoke.sh` Phase 45), so every tier must carry it. Read
/// from [`crate::APP_NAME`], the one place the agent's name lives, so the
/// banner and every sentence the app speaks its name in can't drift apart.
pub(super) const HEADER_NAME: &str = crate::APP_NAME;

/// The command hint beside the mascot — bare `/token`s in the accent colour,
/// three-space separated. Deliberately prose-free so it can't collide with
/// the smoke suite's `for commands` / footer markers.
pub(super) const HEADER_HINT: &[&str] = &["/login", "/model", "/resume"];

/// Columns between the mascot art's right edge and the metadata column —
/// two, per the user's spec (a wider gap read as detached).
pub(super) const HEADER_ART_GAP: usize = 2;

/// Indent shared with the footer and messages — the badge tier's rows sit
/// two columns in. The mascot art is drawn flush-left.
pub(super) const HEADER_INDENT: &str = "  ";

/// The logo gradient's left endpoint — the inline-code cyan ([`inline_code_color`]).
pub(super) fn header_gradient_start() -> Color {
    menu_selected_color()
}

/// The logo gradient's right endpoint — the link blue ([`link_url_color`]).
pub(super) fn header_gradient_end() -> Color {
    link_url_color()
}

/// The version badge + hint-token colour — the cyan accent, so they pop.
pub(super) fn header_accent_color() -> Color {
    inline_code_color()
}

/// The tagline / cwd / separator colour — dim, like the footer.
pub(super) fn header_meta_color() -> Color {
    footer_color()
}

/// Keep a startup notice card — the first-run telemetry disclosure, the
/// update notice — readable on wide terminals, with room for a rounded
/// frame; very narrow panes use the same content without the box.
pub(super) const NOTICE_CARD_MAX_WIDTH: usize = 76;
pub(super) const NOTICE_CARD_MIN_WIDTH: usize = 24;
pub(super) const TELEMETRY_CARD_TITLE: &str = "Telemetry";
/// The update card's title (`docs/update.md`).
pub(super) const UPDATE_CARD_TITLE: &str = "Update available";

/// Notice prose needs more contrast than the banner's cwd metadata.
pub(super) fn telemetry_text_color() -> Color {
    palette().text_muted
}

// --- Live-region geometry. The bottom region's height is dynamic: it grows with
// the wrapped input (see `live_height`). `render_live` and `cursor_position` both
// derive their layout from `input_box` so the drawn text and cursor never drift;
// `main.rs`/`term.rs` size the viewport from `live_height`/`LIVE_MIN_HEIGHT`. ---

/// A blank gap row between the streaming preview and the box, so the live reply
/// never butts up against the box's top rule. Present only while streaming.
pub(super) const GAP_ROWS: u16 = 1;

/// The input box's non-text rows: a top rule and a bottom rule.
pub(super) const INPUT_CHROME_ROWS: u16 = 2;

/// The smallest the live region ever gets: a one-text-row box framed by two
/// rules (idle has no preview strip). `main.rs` sizes the initial viewport from this.
pub const LIVE_MIN_HEIGHT: u16 = INPUT_CHROME_ROWS + 1;

/// The `/compact` marker cell's text — codex's "Context compacted" info cell,
/// verbatim. See `docs/compact.md`.
pub const COMPACTED_NOTICE: &str = "Context compacted";

// --- The `Agent` tool (docs/agent-tool.md) ---

/// The tree connectors of a group cell's per-agent rows: `   ├ {description}`
/// for every agent but the last, `   └ {description}` for the last, with the
/// status row's gutter continuing the rail (`   │ ⎿  Done` / `     ⎿  Done`).
pub(super) const AGENT_TREE_INDENT: &str = "   ";

pub(super) const AGENT_TREE_MID: &str = "├ ";

pub(super) const AGENT_TREE_LAST: &str = "└ ";

pub(super) const AGENT_TREE_PIPE: &str = "│ ";

pub(super) const AGENT_TREE_BLANK: &str = "  ";

/// The status row's corner inside the tree (`⎿  Done`).
pub(super) const AGENT_TREE_CORNER: &str = "⎿  ";

/// The committed background-launch header's hint: the ↓ manager plus the
/// Ctrl+O transcript, where the launch cell expands to each agent's prompt
/// and tool headers (the user-requested pairing).
pub(super) const AGENT_MANAGE_HINT: &str = " (↓ to manage · ctrl+o to expand)";

/// The fixed `⎿` body of a **lone** agent cell resolved by a background
/// launch — [`TOOL_BACKGROUNDED`]'s agent twin with the transcript hint
/// added, since the cell expands in Ctrl+O to the agent's prompt, nested
/// tool headers, and response.
pub(super) const AGENT_BACKGROUNDED: &str =
    "Running in the background (↓ to manage · ctrl+o to expand)";

/// The Ctrl+O cell's `Prompt:` / `Response:` section labels (green bold,
/// Claude Code's transcript look).
pub(super) const AGENT_PROMPT_LABEL: &str = "Prompt:";

pub(super) const AGENT_RESPONSE_LABEL: &str = "Response:";

pub(super) fn agent_section_color() -> Color {
    tool_ok_color()
}

/// The rule cell painted **after** the agent session view's composer label —
/// `── {description} ─` instead of `── {description} ` — so the label sits
/// embedded in the top rule rather than dangling off its right end
/// (`docs/agent-tool.md`). One border glyph, [`border_color`]-styled at the
/// render site.
pub(super) const AGENT_VIEW_RULE_TAIL: &str = "─";

/// The most of that top rule the label may take: **half** its width, the rest
/// staying rule. A `description` is the model's own sentence and can run the
/// width of the terminal, and ratatui skids an over-wide right-aligned title
/// off its **left** end — so an unclipped label ate the whole frame *and* lost
/// the head of the very text it was showing. Past this budget the description
/// is cut with [`TOOL_HEADER_ELLIPSIS`] instead (`docs/agent-tool.md`).
pub(super) const AGENT_VIEW_LABEL_DIVISOR: usize = 2;

/// That label's ground: the agent session view's composer label rides its
/// top rule as a **lit chip** — the theme's accent under the on-accent ink,
/// the ↓-focused footer chip's dress ([`footer_focus_bg`]/[`footer_focus_fg`]),
/// since it answers the same kind of question (*which* thing the keys act
/// on: here, which conversation the composer feeds). Dim text embedded in a
/// dim rule was the one row saying the screen was not the main session, and
/// the eye passed over it (the user-requested fill, `docs/agent-tool.md`).
/// The label's own padding spaces sit inside the fill, the
/// [`AGENT_VIEW_RULE_TAIL`] outside it, so the chip reads as a tab set into
/// the rule rather than a smudge on it.
pub(super) fn agent_view_label_bg() -> Color {
    menu_selected_color()
}

/// The chip's ink — `on_accent`, the pair the ↓-focused footer chip and the
/// current ask-question chip already stand on: near-black on a dark theme's
/// accent, black on ANSI's cyan, and Latte's pale `base` on its blue —
/// Catppuccin's own text-on-accent rule, and the catalog's weakest contrast
/// at about 2.5:1 — so a theme that reads at the footer chip reads here.
pub(super) fn agent_view_label_fg() -> Color {
    palette().on_accent
}

/// Indent of a Ctrl+O agent cell's section bodies (under the `⎿  ` corner's
/// label, one level further in) and of its nested tool-header lines.
pub(super) const AGENT_BODY_INDENT: &str = "       ";

pub(super) const AGENT_NESTED_INDENT: &str = "     ";

/// The footer roster (the persistent agent list under the footer): the
/// selection marker, the main row's bullet, and an agent row's circle.
pub(super) const AGENT_LIST_MARKER: &str = "❯ ";

pub(super) const AGENT_LIST_INDENT: &str = "  ";

pub(super) const AGENT_MAIN_BULLET: &str = "● ";

pub(super) const AGENT_ROW_BULLET: &str = "◯ ";

pub(super) const AGENT_MAIN_LABEL: &str = "main";

/// The roster selection's footer hints (they take the footer line's slot).
pub(super) const AGENT_HINT_MAIN: &[(&str, &str)] = &[("↑/↓", " to select"), ("Enter", " to view")];

pub(super) const AGENT_HINT_AGENT: &[(&str, &str)] = &[("Enter", " to view"), ("x", " to stop")];

/// The same hint on a **settled** row (a user stop, or a finished agent still
/// lingering): the key is the same, its job isn't — `x` clears the row it
/// stopped rather than stopping it twice (`docs/agent-tool.md`).
pub(super) const AGENT_HINT_AGENT_DONE: &[(&str, &str)] =
    &[("Enter", " to view"), ("x", " to clear")];

/// The row (within the picker's framed area) the `>` search line sits on — top
/// rule (0), gap (1), search (2). Shared by [`render_model_picker`] and
/// [`cursor_position`] so the cursor lands on the query.
pub(super) const MODEL_SEARCH_ROW: u16 = 2;

// --- the inline tool-permission prompt (docs/permissions.md) ---

/// The prompt's outer frame: a full-width rule above and below, in the input
/// box's border colour so the modal reads as the same surface.
pub(super) const PERMISSION_RULE: &str = "─";

/// The dashed rules framing a file change's numbered body, dim so the outer
/// frame stays the stronger line.
pub(super) const PERMISSION_BODY_RULE: &str = "╌";

pub(super) fn permission_body_rule_color() -> Color {
    tool_dim_color()
}

/// One-space inset on every text row (the body's numbers land here too).
pub(super) const PERMISSION_INDENT: &str = " ";

/// The extra inset on a `bash` prompt's command and description rows.
pub(super) const PERMISSION_COMMAND_INDENT: &str = "   ";

/// The action title (`Create file` / `Edit file` / `Bash command`) — the
/// palette accent, bold.
pub(super) fn permission_title_color() -> Color {
    menu_selected_color()
}

/// ` · from the {type} agent`, appended to the title when a subagent asked.
pub(super) const PERMISSION_AGENT_SEPARATOR: &str = " · from the ";

pub(super) const PERMISSION_AGENT_SUFFIX: &str = " agent";

pub(super) fn permission_agent_color() -> Color {
    tool_dim_color()
}

/// The file path under the title, and the command on a `bash` prompt.
pub(super) fn permission_target_color() -> Color {
    tool_output_color()
}

/// The model's own description of a `bash` call, under the command.
pub(super) fn permission_detail_color() -> Color {
    tool_dim_color()
}

/// The standing notice above a `bash` prompt's question.
pub(super) const PERMISSION_NOTICE: &str = "This command requires approval";

pub(super) fn permission_notice_color() -> Color {
    tool_dim_color()
}

/// The `❯ ` on the highlighted option row (the unselected rows indent by its
/// width so the list stays aligned).
pub(super) const PERMISSION_MARKER: &str = "❯ ";

/// The highlighted option row lights up whole — marker, number, and label —
/// like the slash-command palette's selection.
pub(super) fn permission_selected_color() -> Color {
    menu_selected_color()
}

/// The hint row under the options, `{key}{label}` pairs joined by ` · `.
pub(super) const PERMISSION_HINT_SEPARATOR: &str = " · ";

pub(super) fn permission_hint_key_color() -> Color {
    menu_selected_color()
}

pub(super) fn permission_hint_text_color() -> Color {
    tool_dim_color()
}

/// Tab's amend field: the hints that replace the option row's set.
pub(super) const PERMISSION_AMEND_HINTS: &[(&str, &str)] = &[
    ("Enter", " to reject with this feedback"),
    ("Esc", " to go back"),
];

/// Rows between the last option/amend row and the bottom of the prompt (gap,
/// hint, gap, rule) — how [`cursor_position`] seats the cursor on the
/// highlighted option (or in the amend field) from the region's bottom edge
/// without re-deriving the body. `permission_lines` pads a capped prompt
/// *above* the question to keep that block flush against this tail.
pub(super) const PERMISSION_TAIL_ROWS: u16 = 4;

/// The most rows one option label may wrap to before it caps with a `…` —
/// a long "don't ask again" rule (an exact command) wraps instead of hiding
/// its tail, but a *pathological* one (kilobytes on a line) must not stack
/// the option block taller than the terminal and push `3. No` and the hints
/// off the bottom (the region clamps to the screen and paints top-down).
/// Four ~100-column rows show any realistic command whole.
pub(super) const PERMISSION_OPTION_MAX_ROWS: usize = 4;

/// The body rows a permission prompt is guaranteed even when a big parallel
/// batch queues a screenful of `⎿ Waiting…` siblings above it. The body is
/// the point of the prompt — you read what you approve — so the *context*
/// gives way first: excess sibling cells collapse into the dim
/// `… +N more waiting` summary row rather than squeezing the body's budget
/// to nothing (the "prompt with no content" bug). Sized like the inline
/// cell's [`FILE_PEEK_LINES`] peek, which is what a squeezed body degrades
/// to (numbered rows + the `… +N lines` tail). A body naturally shorter
/// reserves only what it needs.
pub(super) const PERMISSION_MIN_BODY_ROWS: usize = 10;

/// The prompt body's safety ceiling. The body is shown **whole** — a page
/// taller than the terminal flows into scrollback rather than capping
/// (`docs/view-flow.md`) — but the page is rebuilt (and its body
/// syntax-highlighted) every draw tick while the prompt is open, so a
/// pathological multi-megabyte write must not turn each frame into an
/// unbounded build. Past this many body rows the familiar `… +N lines` tail
/// returns — far beyond anything a human reviews, bounded for the loop.
pub const PERMISSION_BODY_MAX_ROWS: usize = 2_000;

// --- the inline AskUserQuestion modal (docs/ask.md) ---

/// One-space inset on every text row — the permission prompt's, so the two
/// modals read as the same surface (both share [`PERMISSION_RULE`] frames,
/// [`PERMISSION_MARKER`], and the hint-row colours).
pub(super) const ASK_INDENT: &str = " ";

/// The chip-strip glyphs: an unanswered question's box, an answered one's
/// checked box, and the Submit tab's check.
pub(super) const ASK_CHIP_UNANSWERED: &str = "☐";

pub(super) const ASK_CHIP_ANSWERED: &str = "☒";

pub(super) const ASK_CHIP_SUBMIT: &str = "✔";

/// The dim `←`/`→` bookends of a multi-question chip strip — the reminder
/// that ←/→ move between the tabs.
pub(super) const ASK_ARROW_LEFT: &str = "←";

pub(super) const ASK_ARROW_RIGHT: &str = "→";

/// The gap between chips.
pub(super) const ASK_CHIP_GAP: &str = "  ";

/// The **current** chip lights on the selection background — the cyan block
/// that says which section the keys act on (the user-requested highlight).
pub(super) fn ask_chip_current_bg() -> Color {
    menu_selected_color()
}

pub(super) fn ask_chip_current_fg() -> Color {
    footer_focus_fg()
}

/// The idle chips, dim so the current one carries the eye.
pub(super) fn ask_chip_color() -> Color {
    tool_dim_color()
}

/// A multi-select option's checkbox, checked and not.
pub(super) const ASK_CHECKED: &str = "[✔] ";

pub(super) const ASK_UNCHECKED: &str = "[ ] ";

/// The green check after a single-select question's chosen label
/// (`1. Black ✔`).
pub(super) const ASK_PICKED_MARK: &str = " ✔";

pub(super) fn ask_picked_color() -> Color {
    tool_ok_color()
}

/// An option's description, dim under its label.
pub(super) fn ask_desc_color() -> Color {
    tool_dim_color()
}

/// The auto-added free-text row's label.
pub(super) const ASK_OTHER_LABEL: &str = "Type something.";

/// A multi-select question's own confirm row (unnumbered, per the reference).
pub(super) const ASK_CONFIRM_LABEL: &str = "Submit";

/// The row that resolves the whole call as "let's talk instead".
pub(super) const ASK_CHAT_LABEL: &str = "Chat about this";

/// The notes line under a preview panel: the label, the dim placeholder
/// before any notes exist, and the hint key that opens the field.
pub(super) const ASK_NOTES_LABEL: &str = "Notes: ";

pub(super) const ASK_NOTES_PLACEHOLDER: &str = "press n to add notes";

/// The preview panel's box-drawing corners and edges, dim like the body
/// rules so the content carries the eye.
pub(super) fn ask_preview_color() -> Color {
    tool_dim_color()
}

/// The widest the option column may grow in the side-by-side layout, as a
/// share of the region width — the preview box keeps the rest.
pub(super) const ASK_LEFT_MAX_SHARE: f32 = 0.45;

/// The most rows the preview box shows of an option's preview content before
/// clipping (the box is a peek, not a pager).
pub(super) const ASK_PREVIEW_MAX_ROWS: usize = 12;

/// The Submit page's texts.
pub(super) const ASK_REVIEW_TITLE: &str = "Review your answers";

pub(super) const ASK_REVIEW_QUESTION: &str = "Ready to submit your answers?";

pub(super) const ASK_SUBMIT_LABEL: &str = "Submit answers";

pub(super) const ASK_CANCEL_LABEL: &str = "Cancel";

/// The review page's warning when any question is still unanswered — the
/// submission would be partial, and the list below shows only what was
/// answered.
pub(super) const ASK_WARNING: &str = "⚠ You have not answered all questions";

/// The warning's colour — the retry/system amber, the one "caution" tone the
/// theme already speaks.
pub(super) fn ask_warning_color() -> Color {
    status_retry_color()
}

/// The review page's `→ {answer}` text — green, so the recorded answer is
/// the row that carries the eye.
pub(super) fn ask_answer_color() -> Color {
    tool_ok_color()
}

/// The most wrapped rows one review answer shows before capping with a dim
/// `…` row — an expanded multi-kilobyte paste must not flood the page (the
/// committed cell and the answers JSON still carry it whole).
pub(super) const ASK_REVIEW_ANSWER_MAX_ROWS: usize = 4;

/// The review page's `● {question}` bullet and the `→ {answer}` arrow.
pub(super) const ASK_REVIEW_BULLET: &str = "● ";

pub(super) const ASK_ANSWER_ARROW: &str = "→ ";

// --- Full-screen Git review (/diff). Geometry is shared with paging. ---
pub(super) const DIFF_MARGIN: u16 = 1;
pub(super) const DIFF_HEADER_ROWS: u16 = 4;
pub(super) const DIFF_FOOTER_ROWS: u16 = 2;
pub(super) const DIFF_PANE_HEADER_ROWS: u16 = 1;
pub(super) const DIFF_SPLIT_MIN_WIDTH: u16 = 76;
pub(super) const DIFF_SIDEBAR_MIN: u16 = 26;
pub(super) const DIFF_SIDEBAR_MAX: u16 = 42;
pub(super) const DIFF_PANE_GAP: u16 = 1;
pub(super) const DIFF_FILE_ROWS: usize = 2;
pub(super) const DIFF_MIN_GUTTER: usize = 3;
pub(super) const DIFF_COMPACT_FILTER_WIDTH: u16 = 58;
pub(super) const DIFF_BADGE: &str = " DIFF ";
pub(super) const DIFF_SELECTED: &str = "▎";
pub(super) const DIFF_SEPARATOR: &str = " · ";
pub(super) const DIFF_GUTTER_SEPARATOR: &str = "│";
pub(super) const DIFF_SCROLL_TRACK: &str = "│";
pub(super) const DIFF_SCROLL_THUMB: &str = "┃";
pub(super) const DIFF_CARET: &str = "▏";

pub(super) fn diff_pane_bg() -> Color {
    palette().user_bg
}

pub(super) fn diff_border_color(focused: bool) -> Color {
    if focused {
        menu_selected_color()
    } else {
        tool_dim_color()
    }
}

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
    App, HistoryItem, HistorySearch, KeyOnboarding, KeyStep, ModelLoad, ModelPicker,
    ProviderChoice, QueuedTurn, ResumeControl, ResumeFilter, ResumePicker, ResumeSort, Role,
    SearchState, SlashCommand, ToastKind, TokenArrow, ToolCall, ToolStatus, TurnStatus,
    TurnSummary, command_query, matching_commands,
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

// --- Assistant markdown rendering (fenced code blocks + ATX headings;
// `docs/markdown.md`). Code sits under the bullet (no gutter, no language
// label), rendered VERBATIM (indentation preserved, no word-wrap) — the fix
// for code losing its indentation — and **syntax-highlighted** by the
// hand-rolled `highlight` tokenizer (One Dark palette below). Headings keep
// their `#` markers and style the line per level, matching codex
// (`heading_style`). ---
/// Code text — a neutral light grey, the default (unhighlighted) code colour.
const CODE_TEXT_COLOR: Color = Color::Rgb(0xAB, 0xB2, 0xBF);
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

// Syntax-highlight palette (One Dark) — `highlight::Kind` → colour, mapped here
// so all styling stays centralized in `ui.rs` (the tokenizer is colour-agnostic).
/// Keywords — magenta.
const CODE_KEYWORD_COLOR: Color = Color::Rgb(0xC6, 0x78, 0xDD);
/// String / char literals — green.
const CODE_STRING_COLOR: Color = Color::Rgb(0x98, 0xC3, 0x79);
/// Comments — dim slate.
const CODE_COMMENT_COLOR: Color = Color::Rgb(0x5C, 0x63, 0x70);
/// Numbers — orange.
const CODE_NUMBER_COLOR: Color = Color::Rgb(0xD1, 0x9A, 0x66);
/// Names in call position — blue.
const CODE_FUNCTION_COLOR: Color = Color::Rgb(0x61, 0xAF, 0xEF);

/// Map a highlighter [`highlight::Kind`] to its code colour.
fn code_kind_color(kind: highlight::Kind) -> Color {
    match kind {
        highlight::Kind::Plain => CODE_TEXT_COLOR,
        highlight::Kind::Keyword => CODE_KEYWORD_COLOR,
        highlight::Kind::Str => CODE_STRING_COLOR,
        highlight::Kind::Comment => CODE_COMMENT_COLOR,
        highlight::Kind::Number => CODE_NUMBER_COLOR,
        highlight::Kind::Function => CODE_FUNCTION_COLOR,
    }
}

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
/// Role-tag colours — the tool palette's hues (user blue, assistant green,
/// system amber) so the roles scan apart at a glance.
const CONTEXT_USER_COLOR: Color = TOOL_RUNNING_COLOR;
const CONTEXT_ASSISTANT_COLOR: Color = TOOL_OK_COLOR;
const CONTEXT_SYSTEM_COLOR: Color = Color::Rgb(0xE5, 0xC0, 0x7B);

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
/// visible (`menu_window`), like the palette.
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
/// The most provider rows shown at once (longer lists scroll, like the palette).
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
/// Sized to hold the whole [`crate::app::COMMANDS`] registry so a bare `/`
/// lists every command without scrolling.
const MENU_MAX_ROWS: u16 = 7;
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
    ("alt+enter", " for newline"),
    ("ctrl+o", " for tool output"),
    ("esc", " to quit"),
    ("ctrl+c", " to quit"),
    ("alt+↑", " to edit queue"),
    ("tab", " to queue next turn"),
    ("ctrl+v", " for image paste"),
    ("ctrl+d", " for llm context"),
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

/// Rows of the streaming strip above the box. The strip has two independent
/// slots, each a content row plus a trailing gap:
///
/// - the **status line** (`has_status`: a turn is active *and* it is not a `!`
///   shell turn — a shell run hides the spinner status entirely, showing its
///   elapsed in the `⎿ Running… (Ns)` preview instead, see [`strip_has_status`]
///   and `docs/shell-command.md`);
/// - the **preview line** (`has_preview`: a running tool, or a reply whose
///   buffer is non-empty — [`strip_has_preview`]).
///
/// So a normal streaming turn is preview + gap + status + gap (4 rows); the
/// pre-stream pause is status + gap only (2 rows, no empty preview line, codex
/// parity); a shell run is preview + gap only (2 rows, no status); and idle it
/// collapses to nothing (both flags false — `has_preview` can't be true while
/// idle, since it needs a running tool or a live buffer). The **queued
/// messages** (`queued_rows`) stack below this, between the strip and the box's
/// top rule — added separately by [`live_height`]/[`live_layout`] since their
/// height depends on the queue.
const fn strip_rows(has_status: bool, has_preview: bool) -> u16 {
    let preview = if has_preview {
        PREVIEW_ROWS + GAP_ROWS
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

/// Whether the streaming strip shows the **status line** (the spinner + timer +
/// `esc to interrupt`): true while a turn is active, **except a `!` shell
/// turn**, which suppresses the whole status row and shows its elapsed in the
/// `⎿ Running… (Ns)` preview instead (docs/shell-command.md). Idle → false
/// (no turn). Used by [`render_live`]/[`cursor_position`]/`main.rs` to feed
/// `strip_rows` (and to gate the status render in [`render_live`]).
#[must_use]
pub fn strip_has_status(app: &App) -> bool {
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
    has_preview: bool,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> u16 {
    let rows = input.row_count(field_width(width)) as u16;
    (strip_rows(has_status, has_preview)
        + queued_rows
        + toast_rows
        + INPUT_CHROME_ROWS
        + rows
        + band_rows
        + footer_rows)
        .min(term_height.max(1))
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
    has_preview: bool,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(strip_rows(has_status, has_preview) + queued_rows + toast_rows),
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

#[allow(clippy::too_many_arguments)]
fn input_box(
    area: Rect,
    input: &TextArea,
    has_status: bool,
    has_preview: bool,
    queued_rows: u16,
    toast_rows: u16,
    band_rows: u16,
    footer_rows: u16,
) -> InputBox {
    let [_, frame, _, _] = live_layout(
        area,
        has_status,
        has_preview,
        queued_rows,
        toast_rows,
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

/// Hard-break a code line's **coloured** segments into display rows of at most
/// `width` columns, preserving each run's colour across the break — the verbatim,
/// whitespace-preserving counterpart of [`wrap_verbatim`] that keeps syntax
/// colours. Breaks on grapheme boundaries measured in display columns (an
/// overflowing cluster is placed alone); adjacent same-colour graphemes coalesce
/// into one span. An empty line yields a single empty row (just the bullet/indent,
/// once stamped). Prefix-stable — appending only extends the last row.
fn code_content_rows(segments: &[(String, Color)], width: u16) -> Vec<Vec<Span<'static>>> {
    let width = (width as usize).max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_color = CODE_TEXT_COLOR;
    let mut w = 0usize;
    let flush = |row: &mut Vec<Span<'static>>, run: &mut String, color: Color| {
        if !run.is_empty() {
            row.push(Span::styled(std::mem::take(run), Style::new().fg(color)));
        }
    };
    for (text, color) in segments {
        for g in text.graphemes(true) {
            let gw = cols(g);
            if w > 0 && w + gw > width {
                flush(&mut row, &mut run, run_color);
                rows.push(std::mem::take(&mut row));
                w = 0;
            }
            if *color != run_color {
                flush(&mut row, &mut run, run_color);
                run_color = *color;
            }
            run.push_str(g);
            w += gw;
        }
    }
    flush(&mut row, &mut run, run_color);
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
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
        // kept here because the rule decision lives in this Prose branch, like
        // headings.
        let was_blank = self.prev_blank;
        self.prev_blank = line.trim().is_empty();
        match self.scanner.classify(line) {
            markdown::LineKind::CodeStart(lang) => {
                // The fence opens the block silently: it primes highlighting for
                // the info-string language but emits no row (no gutter, no label).
                self.highlighter = Some(highlight::Highlighter::new(lang.as_deref()));
                Vec::new()
            }
            markdown::LineKind::CodeEnd => {
                self.highlighter = None;
                Vec::new()
            }
            markdown::LineKind::Code => {
                // Expand tabs to spaces first so tab-indented code (Go, Makefiles)
                // keeps its indentation — a tab is zero-width and would collapse.
                let expanded = expand_code_tabs(line);
                let segs = match self.highlighter.as_mut() {
                    Some(h) => h.line(&expanded),
                    None => vec![highlight::Seg {
                        text: expanded.into_owned(),
                        kind: highlight::Kind::Plain,
                    }],
                };
                let colored: Vec<(String, Color)> = segs
                    .into_iter()
                    .map(|s| (s.text, code_kind_color(s.kind)))
                    .collect();
                code_content_rows(&colored, self.content_width)
            }
            markdown::LineKind::Prose => {
                if let Some((level, htext)) = markdown::heading_level(line) {
                    // Codex keeps the `#` markers visible (`"#".repeat(level)`) and
                    // styles the whole line per level — no colour, just modifiers.
                    // We normalise the marker run + a single space like codex does,
                    // then word-wrap the reconstructed heading.
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
                    // Codex renders `---`/`***`/`___` as an unstyled `———` rule on
                    // its own row (`Event::Rule`). It's a single settled row, so
                    // it stays prefix-stable while streaming.
                    vec![vec![Span::raw(THEMATIC_BREAK.to_string())]]
                } else {
                    wrap_text(line, self.content_width)
                        .into_iter()
                        .map(|l| vec![Span::raw(l)])
                        .collect()
                }
            }
        }
    }

    /// Whether the **next** line to be fed sits inside an open fenced code block.
    /// Such a line's rows aren't safe to commit until it completes: the
    /// highlighter's within-line lookahead (a call's `(`, a `//` comment, a
    /// closing `*/`) can recolour an *earlier* wrapped row of the same line. The
    /// streaming committer uses this to withhold an in-progress code line whole.
    fn in_code(&self) -> bool {
        self.highlighter.is_some()
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

/// [`render_live`], but with the streaming strip's assistant-preview line
/// supplied by the caller (the boundary's cheap [`StreamRender::preview`], O(one
/// line)) instead of re-rendering the whole reply here (which was O(reply) *every
/// animation frame* and starved the status spinner — `docs/markdown.md`). A
/// `None` `stream_preview` falls back to rendering the last line from the buffer,
/// so unit tests (which don't thread a `StreamRender`) keep their old behaviour;
/// production always passes `Some`.
pub fn render_live_with_preview(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    stream_preview: Option<&Line<'static>>,
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
    // The band below the box holds the palette, the shortcuts overview, *or* the
    // `@` file picker (band_rows — mutually exclusive). Queued messages render
    // in the strip *above* the box instead; the session-context footer takes
    // the very last row unless a band displaces it.
    let band = band_rows(app);
    let queued = queued_rows(app, area.width);
    let toast = toast_rows(app);
    let footer = footer_rows(app, band);
    // The preview row + its gap are only reserved when there is something to
    // preview; the pre-stream pause shows status-only (no stray blank line).
    // The status row + its gap are reserved unless this is a `!` shell turn,
    // which hides the spinner status and shows its elapsed in the preview.
    let has_preview = strip_has_preview(app);
    let has_status = strip_has_status(app);
    let [strip, _, band_area, footer_area] =
        live_layout(area, has_status, has_preview, queued, toast, band, footer);
    // Rows the preview / status each occupy at the strip's top (0 when absent).
    let preview_rows = if has_preview {
        PREVIEW_ROWS + GAP_ROWS
    } else {
        0
    };
    let status_rows = if has_status {
        STATUS_ROWS + STATUS_GAP_ROWS
    } else {
        0
    };

    // Strip preview (top row; the row below it is the blank gap). A running
    // tool takes precedence — its coloured header (blue) shows what's executing;
    // otherwise the in-progress reply's last line previews. A running `!` shell
    // command shows `⎿ Running… (Ns)` instead — the elapsed the hidden status
    // line would have carried (docs/shell-command.md). Nothing when idle — or
    // before the first chunk (an empty buffer has nothing to preview, so the
    // pre-stream pause shows only the status line, no stray `●` bullet and no
    // reserved row for it).
    let preview = if let Some(tool) = app.current_tool() {
        if tool.shell && tool.status == ToolStatus::Running {
            let elapsed = app.status().map_or(Duration::ZERO, |s| s.elapsed);
            Some(shell_running_line(elapsed))
        } else {
            tool_lines(tool, strip.width).into_iter().next()
        }
    } else if let Some(line) = stream_preview {
        // The boundary already rendered the reply's last line cheaply.
        Some(line.clone())
    } else {
        // Fallback (unit tests): render the last line from the buffer.
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
    // the pause), just above the box, while a turn is in flight — suppressed for
    // a `!` shell turn (has_status false), whose elapsed rides the preview above.
    if has_status && let Some(status) = app.status() {
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
    // non-empty). codex's pending-input preview, in our user-message style. The
    // status slot is 0 rows for a shell turn (status_rows), so the queue sits
    // flush under the preview's gap then.
    if queued > 0 {
        let q_y = strip.y + preview_rows + status_rows;
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

    // The input box: a top/bottom rule framing the wrapped input rows.
    let bx = input_box(
        area,
        &app.input,
        has_status,
        has_preview,
        queued,
        toast,
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
    // (docs/shell-command.md), or a primed backtrack (docs/backtrack.md)
    // takes the same slot with its own line.
    if footer > 0 {
        let line = if let Some(search) = app.history_search.as_ref() {
            search_line(search)
        } else if app.shell_mode {
            shell_mode_line()
        } else if app.backtrack.primed {
            backtrack_hint_line()
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
    // A primed backtrack's "esc again…" hint likewise (priming requires an
    // empty composer, so no band/search/shell can be open with it; see
    // docs/backtrack.md).
    if app.backtrack.primed {
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
        format!(" ({}s", status.elapsed.as_secs()),
        dim,
    ));
    if status.tokens > 0 {
        let arrow = match status.arrow {
            TokenArrow::Down => STATUS_ARROW_DOWN,
            TokenArrow::Up => STATUS_ARROW_UP,
        };
        spans.push(Span::styled(
            format!(" · {arrow} {} tokens", status.tokens),
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
            format!(" · Thinking for {}s", thinking.as_secs()),
            dim,
        ));
    }
    spans.push(Span::styled(format!(" · {STATUS_INTERRUPT_HINT})"), dim));
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
/// [`transcript_selection`]: builds every row and, when a backtrack preview
/// has a user message selected, reverses that message's rows (codex's
/// `user_message_style().reversed()` highlight — the timestamp line under it
/// stays normal) and reports the row range it occupies.
fn transcript_build(app: &App, width: u16) -> (Vec<Line<'static>>, Option<Range<usize>>) {
    let mut lines = Vec::new();
    let mut selection = None;
    let mut user_ordinal = 0usize;
    for item in &app.history {
        match item {
            HistoryItem::Message(m) => {
                let mut message = message_lines(m.role, &m.text, width);
                if m.role == Role::User {
                    if app.backtrack.selected == Some(user_ordinal) {
                        for line in &mut message {
                            line.style = line.style.add_modifier(Modifier::REVERSED);
                        }
                        selection = Some(lines.len()..lines.len() + message.len());
                    }
                    user_ordinal += 1;
                }
                lines.extend(message);
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
    // Entries still waiting in the queue come last — after the live tail, in
    // dispatch order, styled exactly like the inline strip's queued rows
    // ([`queued_lines`] — the two-space inset user-/shell-style lines, a blank
    // dividing entries), so the overlay shows the full live picture and a
    // queued message is never invisible under Ctrl+O (docs/queue.md).
    if !app.queued.is_empty() {
        lines.extend(queued_lines(app, width));
        lines.push(Line::default());
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            TOOL_VIEW_EMPTY.to_string(),
            Style::new().fg(TOOL_DIM_COLOR),
        )));
    }
    (lines, selection)
}

/// The largest the transcript scroll offset can be on a `screen_height`-row
/// screen — the total content height minus the scrolling body (the screen less
/// the pager's title and footer chrome) — so the last line can reach the
/// bottom but not scroll past it. The loop clamps `App::tool_scroll` to this
/// each draw.
#[must_use]
pub fn tool_view_max_scroll(app: &App, width: u16, screen_height: u16) -> usize {
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize;
    transcript_lines(app, width).len().saturating_sub(body)
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

/// The overlay draw's scroll decision while a backtrack preview is active:
/// the `tool_scroll` that brings the highlighted user message into the
/// pager's body window, or `None` with no selection. Pure —
/// `main.rs::draw_tool_view` applies it (once per selection change, gated by
/// [`App::take_backtrack_scroll`]). See `docs/backtrack.md`.
#[must_use]
pub fn backtrack_scroll(app: &App, width: u16, screen_height: u16) -> Option<usize> {
    let range = transcript_selection(app, width)?;
    let body = screen_height.saturating_sub(TOOL_VIEW_TITLE_ROWS + TOOL_VIEW_FOOTER_ROWS) as usize;
    Some(scroll_into_view(app.tool_scroll, &range, body))
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
pub fn render_tool_view(area: Rect, buf: &mut Buffer, app: &App) {
    let [title_area, body_area, sep_area, hints_area] = Layout::vertical([
        Constraint::Length(TOOL_VIEW_TITLE_ROWS),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(TOOL_VIEW_FOOTER_ROWS - 1),
    ])
    .areas(area);

    Paragraph::new(tool_view_header(area.width)).render(title_area, buf);

    let lines = transcript_lines(app, body_area.width);
    let max = lines.len().saturating_sub(body_area.height as usize);
    let scroll = app.tool_scroll.min(max);
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
    }
}

/// One context entry's rows: the coloured `role:` tag, the raw text wrapped
/// **verbatim** (never the markdown renderer — the whole point is showing the
/// unformatted wire content), any attachment paths dim beneath, and a blank
/// spacer.
fn context_entry_lines(
    lines: &mut Vec<Line<'static>>,
    tag: &str,
    color: Color,
    text: &str,
    images: &[std::path::PathBuf],
    width: u16,
) {
    lines.push(Line::from(Span::styled(
        tag.to_string(),
        Style::new().fg(color),
    )));
    let text_width = width.saturating_sub(cols(CONTEXT_INDENT) as u16);
    for row in wrap_verbatim(text, text_width) {
        lines.push(Line::from(format!("{CONTEXT_INDENT}{row}")));
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
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(prompt) = &app.system_prompt {
        context_entry_lines(
            &mut lines,
            CONTEXT_SYSTEM_PROMPT_TAG,
            CONTEXT_SYSTEM_COLOR,
            prompt,
            &[],
            width,
        );
    }
    for message in crate::context::context_messages(&app.history) {
        context_entry_lines(
            &mut lines,
            &format!("{}:", message.role.wire_name()),
            context_role_color(message.role),
            &message.text,
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
/// empty, else the model rows windowed ([`menu_window`]) to keep the selection
/// visible and capped at [`MODEL_MENU_MAX_ROWS`]. Its length equals
/// [`model_list_rows`] so the reserved height and painted rows agree.
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
            let offset = menu_window(matches.len(), selected, max);
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
/// the filter matches nothing, else the rows windowed ([`menu_window`]) to keep
/// the selection visible and capped at [`LOGIN_MENU_MAX_ROWS`]. Its length
/// equals [`login_provider_list_rows`] so the reserved height and painted rows
/// agree.
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
    let offset = menu_window(matches.len(), selected, max);
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
    /// disturbing the renderer's state (a cheap clone peek — O(one line)).
    fn tail_rows(&self, text: &str) -> Vec<Line<'static>> {
        self.renderer.clone().feed_line(&text[self.consumed..])
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
        //    not reach scrollback yet.
        // Otherwise it's settled prose — only its still-growing *last* row is held
        // back. `tail_rows` (an O(one line) render) is computed only in that case,
        // never in the withhold path where it would be discarded.
        let tail_src = &text[self.consumed..];
        if self.renderer.in_code()
            || markdown::is_partial_fence(tail_src)
            || markdown::is_partial_thematic_break(tail_src)
            || markdown::is_partial_heading(tail_src)
        {
            let stable = self.frozen.len();
            self.take_rows(&[], stable)
        } else {
            let tail = self.tail_rows(text);
            let stable = (self.frozen.len() + tail.len()).saturating_sub(1);
            self.take_rows(&tail, stable)
        }
    }

    /// The remaining rows once the reply is complete: the withheld last row plus
    /// the whole trailing partial line, now rendered as a final complete line.
    /// Replaces `final_commit`.
    #[must_use]
    pub fn finish(&mut self, text: &str, width: u16) -> Vec<Line<'static>> {
        self.advance(text, width);
        let tail = self.renderer.feed_line(&text[self.consumed..]);
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
        let total = self.frozen.len();
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

    /// The last rendered row of the current buffer — the strip's streaming
    /// preview. O(new complete lines since the last call + the one trailing line),
    /// so redrawing it every animation frame is cheap.
    #[must_use]
    pub fn preview(&mut self, text: &str, width: u16) -> Option<Line<'static>> {
        self.advance(text, width);
        let tail = self.tail_rows(text);
        tail.last()
            .or_else(|| self.frozen.last())
            .cloned()
            .or_else(|| {
                // The reply-so-far renders to zero rows (only a code fence): batch
                // `assistant_lines` still emits the bullet home, so the preview must
                // match it or the strip would diverge from a repaint.
                Some(empty_assistant_row(
                    &self.renderer.bullet,
                    self.renderer.color,
                ))
            })
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
    let has_preview = strip_has_preview(app);
    let has_status = strip_has_status(app);
    let toast = toast_rows(app);
    // While a Ctrl+R search is open the hardware cursor tracks the end of the
    // *footer query*, not the textarea preview — the shell reverse-i-search
    // feel (codex's history_search_cursor_pos), clamped inside the row.
    if let Some(search) = &app.history_search {
        let [_, _, _, footer_area] = live_layout(
            area,
            has_status,
            has_preview,
            queued_rows(app, area.width),
            toast,
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
        has_status,
        has_preview,
        queued_rows(app, area.width),
        toast,
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
                .any(|s| s.style.fg == Some(CODE_TEXT_COLOR)),
            "code text uses the code colour"
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
        let lines = message_lines(
            Role::Assistant,
            "```python\ndef f():\n    x = \"hi\"  # note\n```",
            80,
        );
        let color_of = |needle: &str, want: Color| {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.content.contains(needle) && s.style.fg == Some(want))
        };
        assert!(color_of("def", CODE_KEYWORD_COLOR), "keyword magenta");
        assert!(color_of("f", CODE_FUNCTION_COLOR), "call blue");
        assert!(color_of("\"hi\"", CODE_STRING_COLOR), "string green");
        assert!(color_of("# note", CODE_COMMENT_COLOR), "comment dim");
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
            render.preview(full, width).map(|l| plain(&l)).as_deref(),
            Some("● "),
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
        let mut buf = buffer(40, 16);
        render_tool_view(buf.area, &mut buf, &app);
        let all: String = (0..16)
            .map(|y| row(&buf, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains(TOOL_VIEW_TITLE), "title present: {all:?}");
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
        let mut buf = buffer(40, 16);
        render_tool_view(buf.area, &mut buf, &app);
        let header = row(&buf, 0, 40);
        assert!(
            header.starts_with("/ T R A N S C R I P T / / "),
            "the title overlays the slash tiling: {header:?}"
        );
        // body rows 1..=11, then the separator at 16 - 4.
        let sep = row(&buf, 12, 40);
        assert!(sep.starts_with('─'), "{sep:?}");
        assert!(
            sep.contains(" 100% "),
            "everything fits → pinned at 100%: {sep:?}"
        );
        assert!(sep.ends_with('─'), "one dash right of the percent: {sep:?}");
        let hints = row(&buf, 13, 40);
        assert!(
            hints.contains("to scroll") && hints.contains("pgup/pgdn"),
            "{hints:?}"
        );
        assert!(
            row(&buf, 14, 40).contains("q/esc/ctrl+o to quit"),
            "{:?}",
            row(&buf, 14, 40)
        );
        assert_eq!(row(&buf, 15, 40).trim(), "", "a blank final row");
    }

    #[test]
    fn render_tool_view_fills_rows_below_the_content_with_tildes() {
        // Body rows past the transcript's end read `~` (codex's pager, vi-style).
        let mut app = App::new();
        app.record_user_message("hi");
        let mut buf = buffer(40, 12);
        render_tool_view(buf.area, &mut buf, &app);
        // Content is two lines (message + spacer) in a 7-row body: rows 3..=7
        // are filler.
        assert!(row(&buf, 1, 40).contains("❯ hi"), "{:?}", row(&buf, 1, 40));
        for y in 3..=7 {
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
        render_tool_view(buf.area, &mut buf, &app);
        assert!(row(&buf, 8, 40).contains(" 0% "), "{:?}", row(&buf, 8, 40));

        app.tool_scroll = usize::MAX; // pinned to the bottom (clamped)
        let mut buf = buffer(40, 12);
        render_tool_view(buf.area, &mut buf, &app);
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
        // The tool call appears in its raw bracketed format — the form the
        // model sees — not the TUI's bullet rendering.
        assert!(
            texts.iter().any(|t| t == "  [tool Read(f) ok]"),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "  L1"), "{texts:?}");
        // Turn summaries are TUI chrome; they never reach the context.
        assert!(!texts.iter().any(|t| t.contains("Done")), "{texts:?}");
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
                "/tmp/a-very-long-temp-directory-name/inline-tui-clipboard-0123456789.png",
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
        let h = live_height(&app.input, 40, 24, true, true, 0, 0, 0, 0);
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
        let h = live_height(&app.input, 60, 24, true, false, 0, 0, 0, 0);
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
        let h = live_height(&app.input, 20, 24, false, false, 0, 0, 0, 0);
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
            live_height(&app.input, 20, term_h, false, false, 0, 0, 0, 0),
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
        // just preview + gap (2 rows), then the box's top rule — no status row,
        // no `esc to interrupt` hint. See docs/shell-command.md.
        let mut app = App::new();
        app.begin_shell("sleep 30");
        app.set_status_times(Duration::from_secs(3), None);
        let mut buf = buffer(40, 5); // preview + gap + (two rules + one input)
        render_live(buf.area, &mut buf, &app);

        assert_eq!(
            row(&buf, 0, 40).trim_end(),
            "  ⎿ Running… (3s)",
            "the running preview carries the elapsed the status line would have"
        );
        assert!(
            row(&buf, 1, 40).trim().is_empty(),
            "blank gap row below the preview"
        );
        assert_eq!(
            buf[(0, 2)].symbol(),
            "─",
            "the box's top rule sits right under the preview gap — no status line between"
        );
        let all: String = (0..5).map(|y| row(&buf, y, 40)).collect();
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
            live_height(&app.input, 20, 24, false, false, 0, 0, 0, 0),
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
                    // (c) preview == last row of the batch render of this prefix.
                    let want_preview = message_lines(Role::Assistant, prefix, width)
                        .pop()
                        .map(|l| styled(&l));
                    let got_preview = render.preview(prefix, width).map(|l| styled(&l));
                    assert_eq!(
                        got_preview, want_preview,
                        "preview diverged at {prefix:?} (w={width})"
                    );
                    // (a)/(b) commit rows extend a stable prefix of the final render.
                    committed.extend(render.commit(prefix, width).iter().map(styled));
                    assert_eq!(
                        committed[..],
                        expected[..committed.len()],
                        "a committed row diverged while streaming {full:?} (w={width})"
                    );
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
            let expected = message_lines(Role::Assistant, acc, width)
                .pop()
                .map(|l| plain(&l));
            let got = render.preview(acc, width).map(|l| plain(&l));
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
            live_height(&TextArea::from_text(""), 40, 24, false, false, 0, 0, 0, 0),
            LIVE_MIN_HEIGHT
        );
        assert_eq!(
            live_height(&TextArea::from_text("hi"), 40, 24, false, false, 0, 0, 0, 0),
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
                live_height(&ta, 40, 24, true, true, 0, 0, 0, 0),
                live_height(&ta, 40, 24, false, false, 0, 0, 0, 0) + 4,
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
            live_height(&many, 40, 10, false, false, 0, 0, 0, 0),
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
        let h = live_height(&app.input, 40, 24, true, true, q, 0, 0, 0);
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
        let closed = live_height(&TextArea::from_text("hi"), 40, 24, false, false, 0, 0, 0, 0);
        let open = live_height(
            &TextArea::from_text("/"),
            40,
            24,
            false,
            false,
            0,
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
        let h = live_height(&app.input, 40, 24, false, false, 0, 0, menu, 0);
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
            live_height(&TextArea::from_text("/"), 40, 24, false, false, 0, 0, 0, 0),
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
                false,
                0,
                0,
                menu,
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
            texts[2].contains("alt+enter for newline")
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
        let closed = live_height(&app.input, 40, 24, false, false, 0, 0, 0, 0);
        let open = live_height(
            &app.input,
            40,
            24,
            false,
            false,
            0,
            0,
            shortcuts_rows(&app),
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
            false,
            0,
            0,
            shortcuts_rows(&app),
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
            live_height(&app.input, 40, 24, false, false, 0, 0, 0, 0),
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
                false,
                0,
                0,
                shortcuts_rows(&app),
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
        let without = live_height(&app.input, 40, 24, true, true, 0, 0, 0, 0);
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let with = live_height(&app.input, 40, 24, true, true, q, 0, 0, 0);
        assert_eq!(with, without + q, "the queue grows the region by its rows");
        assert_eq!(q, 1, "one short queued message is one row");
    }

    #[test]
    fn render_live_draws_the_queue_above_the_box_as_a_user_message() {
        let mut app = App::new();
        app.begin_stream();
        app.queued.push_back(batch(&["world"]));
        let q = queued_rows(&app, 40);
        let h = live_height(&app.input, 40, 24, true, true, q, 0, 0, 0);
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
            true,
            q,
            0,
            shortcuts_rows(&app),
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
            live_height(&ta, 40, 24, false, false, 0, 0, 0, 1),
            live_height(&ta, 40, 24, false, false, 0, 0, 0, 0) + 1,
            "the footer adds its row at the very bottom"
        );
    }

    #[test]
    fn render_live_paints_the_footer_on_the_last_row() {
        let app = with_session();
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
        let h = live_height(&app.input, 60, 24, true, true, 0, 0, 0, 1);
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
        let without = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 0);
        app.show_toast("hi", ToastKind::Info);
        let with = live_height(&app.input, 60, 24, false, false, 0, toast_rows(&app), 0, 0);
        assert_eq!(with, without + 1, "the toast adds exactly one row");
    }

    #[test]
    fn render_live_paints_the_toast_directly_above_the_box_when_idle() {
        let mut app = App::new();
        app.show_toast("Copied last message to clipboard", ToastKind::Info);
        let h = live_height(&app.input, 60, 24, false, false, 0, toast_rows(&app), 0, 0);
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
        let h = live_height(&app.input, 60, 24, true, true, 0, toast_rows(&app), 0, 0);
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
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let band = menu_rows(&app);
        let h = live_height(
            &app.input,
            60,
            24,
            false,
            false,
            0,
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
        let bare_h = live_height(&app.input, 40, 24, false, false, 0, 0, 0, 0);
        let bare = cursor_position(Rect::new(0, 0, 40, bare_h), &app);
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let footer_h = live_height(&app.input, 40, 24, false, false, 0, 0, 0, 1);
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
                            strip_has_preview(app),
                            queued_rows(app, w),
                            0,
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
                            let mut render = StreamRender::new();
                            let _ = render.commit(text, w);
                            let _ = render.finish(text, w);
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
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
        let area = Rect::new(0, 0, 60, h);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, h - 1, "on the footer row, not in the textarea");
        let expected = cols(FOOTER_INDENT) + cols(SEARCH_PROMPT) + cols("git");
        assert_eq!(x as usize, expected);
    }

    #[test]
    fn the_search_cursor_clamps_inside_a_narrow_terminal() {
        let app = searching(&["git status"], "a very very long query indeed");
        let h = live_height(&app.input, 20, 24, false, false, 0, 0, 0, 1);
        let area = Rect::new(0, 0, 20, h);
        let (x, _) = cursor_position(area, &app);
        assert!(x < 20, "clamped inside the width (codex clamps the same)");
    }

    #[test]
    fn the_previewed_match_highlights_the_query_reversed() {
        let app = searching(&["git status"], "stat");
        assert_eq!(app.input.text(), "git status", "the match previews");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
        app.set_session_info("dummy_model_name", "~/inline-tui");
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, 1);
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
                images: Vec::new(),
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
                images: Vec::new(),
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
        app.set_status_times(Duration::from_secs(5), None);
        let q = queued_rows(&app, 60);
        // A shell turn hides the status line (has_status false), so the strip is
        // preview + gap only — sized exactly as main.rs::draw does.
        let h = live_height(
            &app.input,
            60,
            24,
            strip_has_status(&app),
            strip_has_preview(&app),
            q,
            0,
            0,
            footer_rows(&app, 0),
        );
        let mut buf = buffer(60, h);
        render_live(buf.area, &mut buf, &app);
        assert_eq!(
            row(&buf, 0, 60).trim_end(),
            "  ⎿ Running… (5s)",
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
        let h = live_height(&app.input, 40, 24, false, false, 0, 0, band, 0);
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
        render_tool_view(buf.area, &mut buf, &app);
        let idle: String = (0..16).map(|y| row(&buf, y, 80)).collect();
        assert!(idle.contains("q/esc/ctrl+o to quit"), "normal pager hints");

        app.backtrack.selected = Some(1);
        let mut buf = buffer(80, 16);
        render_tool_view(buf.area, &mut buf, &app);
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
        let h = live_height(&app.input, 60, 24, false, false, 0, 0, 0, footer);
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
        app.open_key_onboarding(login_choices(), "~/.inline-tui/.env");
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
            row(&buf, 8, 60).contains("Keys are saved to ~/.inline-tui/.env"),
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
}

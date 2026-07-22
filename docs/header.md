# The startup header banner

A Claude-Code-style **header** committed to scrollback at launch: an ASCII
wordmark (`ALTER ZERO`), the version, the working directory, and a one-line
command hint. It is the first thing the user sees, and it scrolls up into
history naturally as the conversation grows — exactly like Claude Code's welcome
box.

## What it shows

```
 █████╗ ██╗  ████████╗███████╗██████╗   ███████╗███████╗██████╗  ██████╗
██╔══██╗██║  ╚══██╔══╝██╔════╝██╔══██╗  ╚══███╔╝██╔════╝██╔══██╗██╔═══██╗
███████║██║     ██║   █████╗  ██████╔╝    ███╔╝ █████╗  ██████╔╝██║   ██║
██╔══██║██║     ██║   ██╔══╝  ██╔══██╗   ███╔╝  ██╔══╝  ██╔══██╗██║   ██║
██║  ██║███████╗██║   ███████╗██║  ██║  ███████╗███████╗██║  ██║╚██████╔╝
╚═╝  ╚═╝╚══════╝╚═╝   ╚══════╝╚═╝  ╚═╝  ╚══════╝╚══════╝╚═╝  ╚═╝ ╚═════╝

  v0.1.0 · autonomous ai agent · terminal ui
  ~/inline-tui
  /help · /model · /resume
```

- **Logo** — the ANSI-Shadow wordmark, coloured with a left-to-right
  **cyan → blue gradient** (`#56B6C2` → `#61AFEF`, the app's own accent palette —
  the inline-code cyan and the link blue). Borderless, so the art breathes and no
  box rule is drawn (see *Why borderless* below).
- **Version** — `v{CARGO_PKG_VERSION}` in the cyan accent, so it reads as a badge.
- **Tagline** — `autonomous ai agent · terminal ui`, dim (the persona, echoing
  `prompts/alter_zero.md`).
- **cwd** — the same `~`-relative display string the footer uses
  (`App::session.cwd`, produced by `ui::display_cwd` at the boundary), dim.
- **Hint** — bare command tokens (`/help`, `/model`, `/resume`), the slashes in
  the cyan accent, separators dim.

## Responsive fallback

The banner never overflows: `header_lines` picks the widest wordmark that fits
the terminal, so a narrow pane degrades gracefully instead of wrapping the art.

| width (cols)         | wordmark                                   |
| -------------------- | ------------------------------------------ |
| `≥ indent + full`    | the 6-row ANSI-Shadow block (`~73` wide)   |
| `≥ indent + compact` | a 2-row half-block wordmark (`~37` wide)   |
| otherwise            | a one-line `ALTER ZERO v0.1.0` text badge  |

The metadata rows (version/tagline, cwd, hint) are clamped to the width with a
trailing `…`, exactly like `footer_line`.

## It is chrome, not history

The header is **pure chrome**, like the session footer — it never enters
`App::history`. That means it is never sent to the model
(`context::context_messages` only walks history), never recorded to a `/resume`
rollout file (`main.rs::SessionRecorder` mirrors history), and never appears in
the Ctrl+O transcript. `header_lines` is a pure `ui` helper fed from
`App::session` + `env!("CARGO_PKG_VERSION")`; the boundary owns the timing.

### Surviving the scrollback purge

The one wrinkle of an inline TUI whose reflow rebuilds from `history`
(invariant 3): a resize and `/clear` **purge scrollback and rebuild from
scratch** (`ReflowClear::Purge`). Anything not in `history` would be lost. So the
header is re-emitted in two places, both at the I/O boundary:

1. **Startup** — `main.rs::run` commits it once via
   `term.insert_before(ui::header_lines(&app, width))` (plus a blank spacer),
   right before the first paint. It flows into scrollback through the normal
   flicker-free pipeline.
2. **Every `Purge` repaint** — `repaint_conversation` prepends the header (and a
   spacer) to the rebuilt tail whenever `clear == ReflowClear::Purge`, so it
   reappears at the top after a resize **and** after `/clear` (a fresh-start
   banner, matching Claude Code's `/clear`). An `InPlace` repaint (the Ctrl+O /
   `/resume` overlay return) keeps the terminal's own scrollback, so the header
   is already there and is *not* re-added.

Because the repaint window is **bottom-anchored** (the input box is pinned to the
bottom and overflow scrolls up — invariant 3), prepending the header never hides
content that was on screen before: the header simply occupies scrollback above
the conversation. On a long conversation it scrolls off the top, capped like
everything else by `RESIZE_REFLOW_MAX_ROWS`.

## Why borderless

The `smoke.sh` resize check (Phase 17) counts an input box by its parts —
`^─+$` rule rows == 2, bare `^❯ *$` prompt rows == 1, `dummy_model_name ·`
footers == 1. A *framed* header (a `╭──╮ … ╰──╯` box) would add `─`-only rule
rows and inflate that count. The borderless Signal design draws no full-width
rule, no bare prompt, and no model name, so it is invisible to those structural
counters — it only ever adds logo/metadata rows. The header text is likewise
kept clear of the strings other phases key on (`for commands`,
`dummy_model_name ·`, `Happy`, `⎿`, `Done for`, `esc to interrupt`).

## API (all pure, in `ui.rs`)

- `header_lines(app: &App, width: u16) -> Vec<Line<'static>>` — the whole banner
  as scrollback rows (logo + blank + metadata), sized to `width`. No trailing
  spacer; the caller adds one (the `insert_before(msg); insert_before(blank)`
  pattern).
- `HEADER_LOGO_FULL` / `HEADER_LOGO_COMPACT` / `HEADER_NAME` — the three
  wordmark tiers.
- `HEADER_TAGLINE` / `HEADER_HINT` — the persona line and the command hint.
- `HEADER_GRADIENT_START` / `HEADER_GRADIENT_END` — the logo's cyan → blue
  endpoints; `gradient_spans` lerps per display column and coalesces equal-colour
  runs into spans.

## Tests

- `ui.rs` unit tests (`header_*`): the full form shows the block logo + version +
  cwd + tagline + hint; the narrow forms fall back to the compact wordmark then
  the text badge and never exceed the width; the logo carries the cyan → blue
  gradient; a session-less `App` still renders logo + version (no cwd row); the
  banner contains none of the smoke-reserved strings.
- `scripts/smoke.sh` Phase 45: the header shows at startup (version + tagline),
  survives a resize (still present after 80×24 → 50×24 → 80×24), and re-shows
  after `/clear`; Phases 16/17 confirm it doesn't disturb the `/clear`-blank or
  resize structural counts.

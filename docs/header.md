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
(`context::context_messages` only walks history) and never recorded to a
`/resume` rollout file (`tui::recorder::SessionRecorder` mirrors history). It **does**
top the Ctrl+O transcript — `ui::transcript_build` prepends `header_lines` + a
blank before walking history, so the overlay mirrors the inline scrollback,
which opens with the banner (the empty-transcript placeholder keys on "nothing
beyond the banner", and the backtrack selection/scroll offsets come from the
same walk, so they stay consistent). `header_lines` is a pure `ui` helper fed
from `App::session` + `env!("CARGO_PKG_VERSION")`; the boundary owns the
timing.

### Surviving every rebuild

The one wrinkle of an inline TUI whose reflow rebuilds from `history`
(invariant 3): every repaint regenerates the screen from history, which the
header is not part of. So it is re-emitted at the I/O boundary:

1. **Startup** — `tui::event_loop::run` commits it once via
   `term.insert_before(ui::header_lines(&app, width))` (plus a blank spacer),
   right before the first paint. It flows into scrollback through the normal
   flicker-free pipeline.
2. **Every purge rebuild** (resize, `/clear`, a history rewind) — the purge
   dropped scrollback outright, so `repaint_conversation` restores the banner
   over the rebuilt tail via the pure `ui::banner_tail(banner, tail)` (banner +
   spacer + tail): it reappears after a resize **and** after `/clear` (a
   fresh-start banner, matching Claude Code's `/clear`).

An **overlay return** (Ctrl+O / `/resume` / Ctrl+D) needs no re-emission at
all: it never rewrites the screen — everything that committed while the
overlay was up flushes from the viewport's pending queue *beneath* whatever is
already there — so the banner the launch committed simply survives, on screen
or in the terminal's kept scrollback, and is never duplicated (`smoke.sh`
Phase 3 pins the no-duplicate half, Phase 45 the settled round-trips; the
overwrite-the-window return this replaces used to wipe it on a short
conversation, and, when a reply had grown past the window under the overlay,
silently dropped everything above the window from the terminal — the
scrollback hole Phases 81/82 now pin shut).

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

## API (all pure, in `ui/header.rs`)

- `header_lines(app: &App, width: u16) -> Vec<Line<'static>>` — the whole banner
  as scrollback rows (logo + blank + metadata), sized to `width`. No trailing
  spacer; the caller adds one (the `insert_before(msg); insert_before(blank)`
  pattern).
- `banner_tail(banner, tail) -> Vec<Line<'static>>` — the banner + a blank
  spacer over a purge rebuild's repaint tail (see *Surviving every rebuild*).
- `HEADER_LOGO_FULL` / `HEADER_LOGO_COMPACT` / `HEADER_NAME` — the three
  wordmark tiers.
- `HEADER_TAGLINE` / `HEADER_HINT` — the persona line and the command hint.
- `HEADER_GRADIENT_START` / `HEADER_GRADIENT_END` — the logo's cyan → blue
  endpoints; `gradient_spans` lerps per display column and coalesces equal-colour
  runs into spans.

## Tests

- `ui/tests/header.rs` unit tests (`header_*`): the full form shows the block logo + version +
  cwd + tagline + hint; the narrow forms fall back to the compact wordmark then
  the text badge and never exceed the width; the logo carries the cyan → blue
  gradient; a session-less `App` still renders logo + version (no cwd row); the
  banner contains none of the smoke-reserved strings.
- `ui/tests/header.rs` unit tests (`banner_tail_*`): the banner + spacer
  restore over the rebuilt tail, and a long tail never clips it.
- `ui/tests/header.rs` unit tests (`transcript_*`): the transcript opens with the banner + a
  spacer before the history walk; an empty transcript still shows the
  placeholder under it; `render_tool_view` paints the version badge in the
  overlay body.
- `scripts/smoke.sh` Phase 45: the header shows at startup (version + tagline),
  survives a resize (still present after 80×24 → 50×24 → 80×24), re-shows
  after `/clear`, tops the Ctrl+O transcript (empty and with a real
  conversation), and **survives the Ctrl+O round-trip** — the return leaves it
  untouched over both an empty and a full conversation tail; Phase 3 pins the
  no-duplication half (never more than one banner in screen+scrollback after a
  mid-stream return over a long conversation); Phases 16/17 confirm it doesn't
  disturb the `/clear`-blank or resize structural counts.

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
`/resume` rollout file (`main.rs::SessionRecorder` mirrors history). It **does**
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

1. **Startup** — `main.rs::run` commits it once via
   `term.insert_before(ui::header_lines(&app, width))` (plus a blank spacer),
   right before the first paint. It flows into scrollback through the normal
   flicker-free pipeline.
2. **Every repaint** — `repaint_conversation` restores the banner over the
   rebuilt tail via the pure `ui::banner_tail(banner, tail, budget)` (banner +
   spacer + tail, re-capped to the last `budget` rows):
   - A **`Purge`** rebuild (resize, `/clear`) purged scrollback outright, so the
     banner tops the full rebuild uncapped (`usize::MAX`) — it reappears after a
     resize **and** after `/clear` (a fresh-start banner, matching Claude Code's
     `/clear`).
   - An **`InPlace`** repaint (the Ctrl+O / `/resume` / Ctrl+D overlay return)
     overwrites the on-screen window top-down — where a short conversation still
     *shows* the banner, which the overwrite used to wipe (the header vanished
     on a Ctrl+O round-trip until the next resize or `/clear` re-emitted it). The
     banner joins that tail too, re-capped to the window budget
     (`ui::repaint_budget`): exactly as much of it as the window held comes back
     — fully when the conversation is short, only its bottom rows when it had
     partly scrolled, and not at all once it scrolled wholly into the terminal's
     kept scrollback (re-adding it there would paint a duplicate). The recap is
     exact because `keep_last_rows` keeps suffixes:
     `keep(banner + keep(x, n), n) == keep(banner + x, n)`.

Because the repaint window is **bottom-anchored** (the input box is pinned to the
bottom and overflow scrolls up — invariant 3), prepending the header never hides
content that was on screen before: the header simply occupies scrollback above
the conversation. On a long conversation it scrolls off the top, capped like
everything else by `RESIZE_REFLOW_MAX_ROWS`.

**Residual edge (pre-existing, not banner-specific):** an `InPlace` return only
rewrites the window and trusts the terminal's kept scrollback for everything
above it. When a reply streams *while the overlay is open*, the conversation can
grow past the window, and the return's rebuild then starts below rows that were
on the pre-overlay screen but had never scrolled into scrollback — those rows
(banner or conversation alike) leave the scroll record until the next `Purge`
rebuild (resize, `/clear`) regenerates everything. Fixing that generally would
need the viewport to track how many rows the terminal actually holds in
scrollback and rebuild everything above it — an invariant-3 redesign, out of
scope for the banner. The banner is never *duplicated* by a return
(`smoke.sh` Phase 3 pins that), and every settled round-trip keeps it
(Phase 45).

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
- `banner_tail(banner, tail, budget) -> Vec<Line<'static>>` — the banner + a
  blank spacer over a rebuilt repaint tail, re-capped to the last `budget` rows
  (`usize::MAX` on a `Purge` rebuild, the window budget on an `InPlace` one —
  see *Surviving every rebuild*).
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
- `ui.rs` unit tests (`banner_tail_*`): the banner + spacer restore over a short
  tail; the recap drops the banner once the window is full and keeps only its
  bottom rows when it half-fits; `usize::MAX` never clips.
- `ui.rs` unit tests (`transcript_*`): the transcript opens with the banner + a
  spacer before the history walk; an empty transcript still shows the
  placeholder under it; `render_tool_view` paints the version badge in the
  overlay body.
- `scripts/smoke.sh` Phase 45: the header shows at startup (version + tagline),
  survives a resize (still present after 80×24 → 50×24 → 80×24), re-shows
  after `/clear`, tops the Ctrl+O transcript (empty and with a real
  conversation), and **survives the Ctrl+O round-trip** — the InPlace return
  restores it over both an empty and a full conversation tail; Phase 3 pins the
  no-duplication half (never more than one banner in screen+scrollback after a
  mid-stream return over a long conversation); Phases 16/17 confirm it doesn't
  disturb the `/clear`-blank or resize structural counts.

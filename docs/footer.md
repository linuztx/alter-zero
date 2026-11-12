# The session-context footer under the input box

Port of openai/codex's footer **status line**: a single row pinned directly
below the input box's bottom rule, giving ambient session context —

```
────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────
  dummy_model_name · ~/alter-zero                      auto
```

— the backend's model name and the current working directory, joined with
` · `, and — pinned flush at the row's **right edge**, its own zone — the
session's **permission mode** (`auto` until a project saves another, the
posture Shift+Tab cycles — `docs/permissions.md`; hidden when permissions are
disabled). See `CLAUDE.md` for where this sits in the runtime model.

## What codex does (findings)

Codex's bottom pane reserves a footer row under the composer
(`bottom_pane/footer.rs`). It multiplexes many things through it (quit
reminders, the `?` shortcut overlay, queue hints); the piece we are porting is
the **passive status line** — "the configurable contextual row built from
`/statusline` items such as model, git branch, and context usage":

- **Default content** (`chatwidget.rs`):
  `DEFAULT_STATUS_LINE_ITEMS = ["model-with-reasoning", "current-dir"]` —
  exactly a model name and a cwd.
- **Assembly** (`bottom_pane/status_line_style.rs::status_line_from_segments`):
  segments joined by a **dim ` · ` separator**; with theme colours disabled
  every segment is plain `Style::default().dim()` (with them enabled, items get
  accent colours — cyan model, green path).
- **Cwd format** (`status/helpers.rs::format_directory_display`): relativized
  to home — `~` for home itself, `~/rel` under home, the absolute path
  otherwise.
- **Layout**: rendered with a two-column indent (`FOOTER_INDENT_COLS = 2`,
  via `prefix_lines`), and the assembled line is ellipsis-truncated when it
  overflows the width (`truncate_line_with_ellipsis_if_overflow`).
- **Yielding** (`footer.rs::shows_passive_footer_line`): the status line gives
  the row up to instructional content — the command popup, the `?` shortcut
  overlay, quit/Esc reminders, and (with a draft while a task runs) the
  queue hint.

## What we build

One always-on context row in the live region, **below the box, below where the
band opens** — present from startup, while idle *and* while a turn streams —
that yields to any open band: the palette, the `?` shortcuts band, or the `@`
file picker (our equivalents of codex's popups and shortcut overlay). Two later
features **take the slot outright** instead of blanking it, codex's
footer-mode multiplexing: the Ctrl+R `reverse-i-search: {query}` line
(`docs/history-search.md`) and the `!` shell mode's red `Shell mode` hint
(`docs/shell-command.md`).

### State (`app/`, `stream/`)

- `SessionInfo { model: String, cwd: String }` and `App.session:
  Option<SessionInfo>` — display-ready strings, injected **once at the I/O
  boundary** via `App::set_session_info(model, cwd)` (the `set_clock` pattern:
  `main.rs` does the env reads, the pure core only sees strings). Unset — the
  unit-test default — means no footer, so geometry tests without session info
  are unaffected.
- `stream::ReplySource::model_name() -> String` — the footer names whatever
  backend is plugged in, so swapping in a real model updates it for free (a
  real backend returns its real model id). `DummyAi` reports
  `"dummy_model_name"`.

### Geometry & display (`ui/footer.rs`, `ui/layout.rs`)

- `footer_rows(app, band_rows) -> u16` — `1` whenever a Ctrl+R search is open
  or `!` shell mode is on (the search line / `Shell mode` hint own the slot,
  **even with no session info** — no band can be open in either state);
  otherwise `1` when session info is set **and** no band is open
  (`band_rows == 0` — the palette, `?` shortcuts band, and `@` file picker all
  displace the row), else `0`. The single place the
  band-replaces-footer swap is decided; `live_height` adds it,
  [`render_live`]/`cursor_position` pass it, so reserve and paint can't drift.
- `footer_line(app, width) -> Line` — `FOOTER_INDENT` (two spaces, codex's
  `FOOTER_INDENT_COLS`) + `{model} · {cwd}`, each segment in its own colour
  and the ` · ` separators dim (`footer_color()`/`FOOTER_SEPARATOR`; see
  *Colour* below); the content is truncated with a trailing `…` when it
  overflows the width (codex's `truncate_line_with_ellipsis_if_overflow`),
  each cut segment keeping its colour. Two later segments append to the
  same dim ` · ` chain when they apply — the **context gauge**
  `{used}/{window} ({pct}%)` whenever the model's context window is known
  (`1.3k/160k (0.8%)`, both counts through `format_token_count`, the share one
  decimal — `docs/compact.md`) and the running **background shell** count
  (`docs/background.md`) — and one segment sits *outside* the chain, pinned
  flush at the row's **right edge**: the **permission mode**
  (`auto`/`manual`/`edit`/`master` — `App::permission_mode`, injected by the
  boundary and `None` while permissions are disabled, which drops it;
  `docs/permissions.md`). Its
  columns plus a `FOOTER_MODE_GAP` gutter are reserved off the chain's budget
  up front, so the `…` truncation always eats the left content and never the
  one segment with a safety meaning. A full row reads
  `deepseek-v3.2 medium · ~/Codes/tmp · 1.3k/160k (0.8%) · 1 shell     auto`;
  a selected speed tier follows the thinking mode — `gpt-5.5 medium fast ·
  ~/repo`, `gpt-5.6-sol ultrafast · ~/repo` — wearing whatever name the
  model's listing gave the tier its command selected (`/fast`,
  `/ultrafast`, `docs/fast-mode.md`), and standard shows nothing.
  **Inside a subagent's session view the model and gauge segments describe
  that agent** (`docs/agent-context-gauge.md`): the gauge is the viewed
  agent's own context — `AgentRun::context_used`, the last usage frame's
  `input` plus the estimate of what joined after it, the transcript estimate
  when a settle saw no frame (`docs/token-estimate.md`) —
  against the window of the model it runs on (`App::context_gauge`), and a
  type whose definition pins another model names it in place of the session's,
  mode-less (`App::viewed_agent_model`); the lead's pair is back with the main
  view, and it alone drives auto-compact.
  That last segment is the one **focusable** one: ↓ from an idle composer
  lights it on the palette cyan (`footer_focus_bg()`/`footer_focus_fg()`) and
  Enter opens the ↓ manager band, Claude-Code-style. Only that span's style
  changes — the row keeps every other segment in its own colour — and the
  truncation path carries the style through, so a narrow width cuts the lit
  segment without losing the highlight (see `docs/background.md`).
- `display_cwd(path, home) -> String` — codex's `format_directory_display`
  relativization: `~` for home itself, `~/sub` under home, the absolute path
  otherwise (or when no home is known). Pure — `main.rs` passes
  `env::current_dir()` and `$HOME`.
- `live_height`/`live_layout`/`input_box` gain a `footer_rows` parameter; the
  layout grows a fourth area `[strip, input, band, footer]` at the very
  bottom, so the box's top — and the cursor — stay put whether or not the
  footer (or the band that replaces it) is showing.

### Colour

Codex's status line with its theme colours on — *cyan model, green path* —
read off the active `/theme` palette, so the row follows every theme the way
the banner does (`docs/theme.md`). Colour marks what the row **names**; the
punctuation between the names stays dim:

```
  [magpie]deepseek-v4.1-flash medium fast · ~/Codes/rust/alter-zero · 0/475k (0.0%)       master
```

- **model** — `footer_model_color()`, the accent: what the session talks to
  is the row's first fact; the `[provider]` bracket is the row's dim
  (`footer_color()`), so the provider frames the model without competing with
  it. A subagent's pinned model wears the accent too (no bracket — the pin
  carries no provider).
- **thinking mode and speed tier** — `footer_qualifier_color()`, the status
  line's thinking purple (`status_thinking_color()`), one colour for what
  Ctrl+T and a tier's command turn; the `Thinking for Ns` clause above the
  box wears the same hue, so the effort and the thinking it buys read as one
  fact (`docs/status-indicator.md`).
- **cwd** — `footer_cwd_color()`, the success green, codex's path colour; the
  banner's cwd row wears it too (`header_cwd_color()`, `docs/header.md`), so
  the path is one colour wherever the app names it.
- **context gauge** — `footer_gauge_color(used, window, threshold)`: the
  `/context` page's share rule, shared rather than copied
  (`context_fill_color`, which that page's subtitle calls too): dim while
  there is room, amber from 75%, red from the auto-compact threshold that
  page reads for the conversation on screen (`App::context_usage_threshold`,
  90% with none). A gauge in colour is a gauge saying compaction is near.
- **permission mode** — `footer_mode_color(mode)`, warming with what the
  mode lets the agent do without asking: `manual`, which asks before
  everything, stays dim; `edit` (file changes unasked) takes the link blue;
  `auto` (the classifier approves commands) the caution amber; and `master`,
  which asks nothing, the error red (Claude Code's bypass-permissions dress).
  Four hues every theme paints apart, so the posture reads at a glance and the
  riskiest one is the loudest. The alternatives were mocked first and lost
  on the same row's own colours: a purple `edit` (Claude Code's accept-edits
  colour) repeats the thinking mode's purple, a cyan one the model's accent,
  and a green one the path's.
- the ` · ` separators, the `…` cut and an unfocused shell count stay
  `footer_color()`.

### Wiring (`main.rs`, the I/O boundary)

Right after the backend is chosen, `run` injects the session info:
`app.set_session_info(backend.model_name(), display_cwd(cwd, home))` with
`cwd`/`home` read from the environment. `live_region_height` adds
`ui::footer_rows` so every viewport-height decision (draws, the `StreamDone`
reseat, resizes) already accounts for the row.

## Known divergences from codex

- **Not configurable.** Codex's `/statusline` lets users pick items (branch,
  context %, …) and turn its theme colours off; ours is the fixed default
  pair — model + cwd — always in theme colour (*Colour* above), painted from
  the active `/theme` palette. More items later are new spans in
  `footer_line`.
- **Fewer footer modes.** Codex multiplexes quit reminders, Esc hints, and
  queue hints through the same row; our slot has exactly four occupants — the
  context line, the Ctrl+R search line, the `!` shell-mode hint, and the
  primed backtrack's `esc again to edit previous message` hint
  (`backtrack_hint_line`, `docs/backtrack.md`) — plus the
  bands that displace it. In particular the context line stays visible while a
  turn streams (codex hides it for the queue hint when the composer has a
  draft mid-run — we have no such hint).
- **Plain right-truncation.** Codex center-truncates long paths in some
  surfaces; the footer line is simply cut with a trailing `…` (which is what
  codex's footer does to the assembled line too).

## Testing

- `stream`: `DummyAi` reports its model name.
- `app`: session info is unset by default; `set_session_info` stores the
  display strings.
- `ui`: `footer_rows` is 0 without session info / 1 with it / 0 when a band is
  open — and 1 whenever a Ctrl+R search is open or shell mode is on, session
  info or not (see `docs/history-search.md` / `docs/shell-command.md`, which
  test the slot's other occupants); `footer_line` renders `  {model} · {cwd}`
  with the model in the accent, the cwd green and the separator dim, every
  theme painting the two apart from each other and from the dim; the thinking
  mode and a speed tier wear the thinking colour; the gauge turns amber from
  75% and red from the threshold; the mode at the right edge is dim for
  `manual`, blue for `edit`, amber for `auto` and red for `master`, four
  colours apart in every theme;
  a ↓-focused shell count lights up on cyan while every other segment keeps
  its text and its own styling; it truncates with `…` at narrow
  widths; `display_cwd` maps home →
  `~`, under-home → `~/sub`, outside/unknown home → absolute; `live_height`
  grows one row with the footer; `render_live` paints the footer on the last
  row (and not while the palette is open); the cursor doesn't move when the
  footer shows; inside an agent session view the gauge is the viewed agent's
  own context and a pinned model is named without the session's thinking mode
  (`docs/agent-context-gauge.md`); a selected speed tier wears its name after
  the thinking mode and standard adds nothing (`docs/fast-mode.md`).
- `scripts/smoke.sh`: the startup frame shows `dummy_model_name · ~` under the
  box; opening the palette (`/`) hides it; dismissing brings it back; Phase
  111 opens a subagent's session view under a forced window and reads that
  agent's gauge, not the lead's.

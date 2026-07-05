# The session-context footer under the input box

Port of openai/codex's footer **status line**: a single dim row pinned directly
below the input box's bottom rule, giving ambient session context —

```
────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────
  dummy_model_name · ~/inline-tui
```

— the backend's model name and the current working directory, joined with
` · `. See `CLAUDE.md` for where this sits in the runtime model.

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

### State (`app.rs`, `stream.rs`)

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

### Geometry & display (`ui.rs`)

- `footer_rows(app, band_rows) -> u16` — `1` whenever a Ctrl+R search is open
  or `!` shell mode is on (the search line / `Shell mode` hint own the slot,
  **even with no session info** — no band can be open in either state);
  otherwise `1` when session info is set **and** no band is open
  (`band_rows == 0` — the palette, `?` shortcuts band, and `@` file picker all
  displace the row), else `0`. The single place the
  band-replaces-footer swap is decided; `live_height` adds it,
  [`render_live`]/`cursor_position` pass it, so reserve and paint can't drift.
- `footer_line(app, width) -> Line` — `FOOTER_INDENT` (two spaces, codex's
  `FOOTER_INDENT_COLS`) + `{model} · {cwd}`, every span dim
  (`FOOTER_COLOR`/`FOOTER_SEPARATOR`); the content is truncated with a
  trailing `…` when it overflows the width (codex's
  `truncate_line_with_ellipsis_if_overflow`).
- `display_cwd(path, home) -> String` — codex's `format_directory_display`
  relativization: `~` for home itself, `~/sub` under home, the absolute path
  otherwise (or when no home is known). Pure — `main.rs` passes
  `env::current_dir()` and `$HOME`.
- `live_height`/`live_layout`/`input_box` gain a `footer_rows` parameter; the
  layout grows a fourth area `[strip, input, band, footer]` at the very
  bottom, so the box's top — and the cursor — stay put whether or not the
  footer (or the band that replaces it) is showing.

### Wiring (`main.rs`, the I/O boundary)

Right after the backend is chosen, `run` injects the session info:
`app.set_session_info(backend.model_name(), display_cwd(cwd, home))` with
`cwd`/`home` read from the environment. `live_region_height` adds
`ui::footer_rows` so every viewport-height decision (draws, the `StreamDone`
reseat, resizes) already accounts for the row.

## Known divergences from codex

- **Not configurable.** Codex's `/statusline` lets users pick items (branch,
  context %, …) and theme colours; ours is the fixed default pair — model +
  cwd — in the all-dim no-colour style. More items later are new spans in
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
  test the slot's other occupants); `footer_line` renders `  {model} · {cwd}` all-dim with the dim
  separator; it truncates with `…` at narrow widths; `display_cwd` maps home →
  `~`, under-home → `~/sub`, outside/unknown home → absolute; `live_height`
  grows one row with the footer; `render_live` paints the footer on the last
  row (and not while the palette is open); the cursor doesn't move when the
  footer shows.
- `scripts/smoke.sh`: the startup frame shows `dummy_model_name · ~` under the
  box; opening the palette (`/`) hides it; dismissing brings it back.

# The running bullet's blink

A tool that is executing no longer shows a blue `●`. It shows the same grey the
permission prompt puts over the call it is asking about — and in the live region
that bullet **blinks**: there for half a second, gone for the next, the header
text beside it never moving. Claude Code's running dot, ported to the inline
TUI.

```
● Bash(npm test)          ← grey, shown…
  ⎿  Running… (3s · timeout 2m)

  Bash(npm test)          ← …then hidden, the text holding its column
  ⎿  Running… (3s · timeout 2m)

● Bash(npm run lint)      ← flat grey: queued, not started
  ⎿  Waiting…

● Read(src/main.rs)       ← green: it resolved
  ⎿  Read 412 lines
```

## Why grey

The old palette spent a colour on *being in flight*: blue while running, green
or red when it finished. But "in flight" is the one state that is already
obvious — it is the cell at the bottom of the screen, with a spinner above it and
a `⎿ Running…` under it — and blue competed with the green/red that actually
carries information.

So everything in flight is now muted and only the **resolution** lands as colour.
That is the same rule the permission prompt already followed: the call it asks
about renders as a bare grey `● Write(hello.py)` header, because nothing has
happened yet. A running call is the same kind of thing — not finished, not
judged — and now reads the same way.

The motion is what replaces the hue. A still grey bullet and a blinking one are
easy to tell apart, and the blink says "still going" far more directly than a
colour ever did.

## Why a blink, not a breath

The first version *breathed*: a raised cosine easing the bullet between two
greys, dim → bright → dim, once a second. It moved, but it moved by showing a
**second colour** — a darker grey that existed for nothing else — and on a
terminal palette without room for the in-between shades it stepped rather
than swelled, or (under the `ansi` theme, whose two ends were the same
bright-black) did not move at all. Claude Code's running dot does not do that:
it is one colour, and it is either there or it is not.

So the bullet keeps **one grey** and animates by *presence*. On the hidden half
of the cycle the `●` is replaced by blanks of its own width, in the same style,
so the header text never shifts a column and the row is exactly as wide as it
was; a `Bash(for i in …)` header reads at the same place with or without its
dot. There is no in-between frame — inside each half the bullet holds — which
is what makes it read as a blink rather than a flicker.

## The cycle

`ui::tool::tool_pulse_visible` is a pure function of one `Duration`:

```
shown = (elapsed mod PERIOD) < PERIOD / 2
```

Whole milliseconds, so the edge lands on the same frame however the clock is
read. `ui::tool::bullet_span(color, blink)` is the one place a header's `●`
comes from: `blink: Some(phase)` renders the glyph or its blanks by that
rule, `None` renders it at rest. The constants live in `ui/theme.rs` with every
other styling decision:

| accessor | palette role (the `onedark` value) | meaning |
| --- | --- | --- |
| `tool_running_color()` | `dim` (`#8A8A8A`) | the **one** colour a running bullet wears — shown, and what a frozen render shows |
| `TOOL_PULSE_PERIOD` | `1000 ms` | one full blink: shown for the first half, hidden for the second |

The grey comes from the active theme's palette (`docs/theme.md`); under the
`ansi` theme it is the terminal's bright-black, and the bullet blinks in it
exactly as it does everywhere — a blink needs no second shade.

The period is comfortably coarser than the loop's 32 ms animation frame and
slow enough to read as a pulse rather than a flicker.

The `pulse_dim` palette role and the `tool_pulse_dim()` / `tool_pulse_bright()`
accessors the breath blended between are still there, but for the **`pulse`
spinner style** alone (`docs/spinner.md`): the status line's `●` keeps the
raised-cosine swell, from that dim up to white, at the bullet's period, so a
pulsing status line and a blinking cell still move in step.

## Where the phase comes from

The same place every other clock in this codebase does: the boundary.
`App::set_pulse(Duration)` is written before each draw from
`StatusClocks::loop_start.elapsed()`, exactly like `set_status_times` and
`set_command_elapsed` — the pure core never reads a clock.

It is a **phase, not a measurement**: nothing displays it, so its epoch is
arbitrary. What matters is that it advances, which the loop's existing
animation re-arm already guarantees — the draw branch schedules the next frame
32 ms out whenever `turn_active()` (a tool can only run inside a turn), the
background band is open, or any agent is alive.

One clock, deliberately, rather than each bullet on its own timer:

- a mixed round's tool cells and its `● Running 3 agents…` tree are on screen
  together, and blinking out of step would look broken rather than intentional;
- unlike the turn's `elapsed` it keeps running between turns, so a **background**
  agent's live cell animates too.

`StatusClocks` therefore has no `Default` any more — `loop_start` is a real
reading, and stamping it implicitly would let a stray `default()` silently
restart every blink mid-session.

## Which bullets blink

Every live `●` that means "running", through the one `bullet_span`:

- a running tool cell's header (`live_tool_lines`, and the streaming `bash`
  cell's `running_command_lines`);
- the aggregated `● Calling {server}…` MCP cell (`docs/mcp.md`);
- the live `● Running {n} agents…` tree and a lone agent's `● Agent(…)` cell
  (`docs/agent-tool.md`), in the main strip and in an agent session view's;
- the `● Thinking…` header of an open reasoning phase
  (`docs/thinking-stream.md`).

Only `Running` ever animates. A `⎿ Waiting…` sibling in a parallel batch, the
call a permission prompt is asking about (it waits on *you*), and every
resolved cell keep their `●` on every frame — they mean the same thing on every
frame, so moving them would say nothing.

## Where it does *not* blink

The animation is live-region-only, and the renderer split says so:

| renderer | blink | why |
| --- | --- | --- |
| `live_tool_lines` (the strip) | **yes** | redrawn every frame, never committed |
| `running_command_lines` (a streaming `bash`) | **yes** | the same strip |
| `agent_view_preview_lines` (an agent session view) | **yes** | the same strip, for the viewed agent |
| `tool_lines` (scrollback commits, resize repaint) | no | a committed row is frozen **forever** — it must never capture the hidden half |
| `tool_full_lines` (the Ctrl+O transcript) | no | see below |
| the permission prompt's context rows | no | nothing is running: the call waits on *you*, and the prompt is a still frame |

`tool_lines` and `live_tool_lines` are the same function with the phase as
`None`/`Some` — one body, so the two can't drift. What a scrollback commit must
never freeze is the **hidden** frame: a cell committed without its bullet would
read as a header that lost its dot for good, which is exactly the accident the
split exists to prevent. An un-injected clock (phase 0, the unit-test default)
renders the bullet **shown** — the top of the cycle.

The **Ctrl+O transcript** sits still on purpose. It is a pager over an
incrementally-built cache whose refresh short-circuits on a signature with no
clock in it (`docs/tool-view-performance.md`); animating there would either not
move at all or cost a full live-tail re-render every 32 ms, for a bullet nobody
opened the overlay to watch blink. Its running bullet renders at rest — still
the grey, still distinct from the green/red around it.

## What stayed blue

`context_user_color()`, the Ctrl+D context view's `user:` role tag, used to
alias the running colour. It is a *role* tag, not a running state, so it keeps
the link blue (the palette's `link` role — `#61AFEF` in the One Dark theme) as
its own value and the two simply parted ways.

## Tests

- `ui/tests/tool.rs` — the running bullet is the permission prompt's grey, one
  colour only; it is shown at the top of the cycle and at the quarter, hidden
  behind blanks of its own width at the half and the three-quarter with the
  header text holding its column, and loops a period later;
  `Waiting`/`Ok`/`Failed` never animate whatever the clock says; and
  `tool_lines` — the renderer that feeds scrollback — always shows it.
- `ui/tests/live.rs` — the wiring: `render_live` paints the strip's bullet at the
  injected phase, and moving `App::set_pulse` half a period visibly blanks the
  painted cell while the header text stays where it was.
- `ui/tests/agent.rs` — the live agent group's bullet blinks on the same clock.
- `ui/tests/reasoning.rs` — the live `● Thinking…` bullet blinks, the committed
  one never does.
- `smoke.sh` Phase 57 — in a real terminal, a batch's running cell is sampled
  across frames and must be caught both with its bullet and without it, no
  frame may hide more than one bullet (the `⎿ Waiting…` siblings hold theirs),
  every bullet grey is the one resting grey, and the resolved cell lands green.

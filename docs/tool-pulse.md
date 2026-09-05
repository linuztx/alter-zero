# The running bullet's pulse

A tool that is executing no longer shows a blue `●`. It shows the same grey the
permission prompt puts over the call it is asking about — and in the live region
that grey **breathes**, dim → bright → dim, once a second. Claude Code's running
dot, ported to the inline TUI.

```
● Bash(npm test)          ← grey, breathing
  ⎿  Running…

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

The motion is what replaces the hue. A flat grey bullet and a breathing grey
bullet are easy to tell apart, and the breathing one says "still going" far more
directly than a colour ever did.

## The wave

`ui::tool::tool_pulse_color` is a pure function of one `Duration`:

```
t = ½·(1 − cos(2π · phase))        phase = (elapsed mod PERIOD) / PERIOD
colour = blend(tool_pulse_bright(), tool_pulse_dim(), t)
```

A raised cosine, so the bullet **swells and fades** instead of flicking on and
off — `t` is 0 at the top of the cycle, 1 at the half, 0 again at the end, and it
moves slowest at the extremes. The constants live in `ui/theme.rs` with every
other styling decision:

| accessor | palette role (the `onedark` value) | meaning |
| --- | --- | --- |
| `tool_pulse_dim()` | `pulse_dim` (`#4A4A4A`) | the bottom of the breath (where a cycle starts) |
| `tool_pulse_bright()` | `dim` (`#8A8A8A`) | the top, reached at the half-cycle |
| `TOOL_PULSE_PERIOD` | `1000 ms` | one full dim → bright → dim breath |
| `tool_running_color()` | `dim` (`#8A8A8A`) | the **resting** colour — what a frozen render shows |

The two colour ends come from the active theme's palette (`docs/theme.md`);
the values above are the original One Dark look's. Under the `ansi` theme
both ends are the same bright-black, so the bullet holds still.

The period is comfortably coarser than the loop's 32 ms animation frame, so the
sweep is smooth rather than steppy, and slow enough to read as a pulse rather
than a flicker.

**The breath only goes down.** Its peak is `tool_dim_color()` — the same grey the
bullet rests on and the same grey the permission prompt uses — so the bullet
dips below that and returns, and never brightens past it. An earlier version
swung up to a near-white `#E8E8E8`; that read as a blink rather than a breath,
and a bullet flashing brighter than the reply text beside it pulled the eye off
the actual content. Dimming is the quieter half of the same signal: it still
plainly moves, but the tool cell never out-shouts the message it sits under.

A consequence worth knowing when reading the code: the resting colour and the
pulse's peak are now the *same value*, so what a scrollback commit must never
freeze is the **dip** — which is also what an un-injected clock (phase 0) would
render, and exactly the accident the `tool_lines` / `live_tool_lines` split
exists to prevent.

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
restart every pulse mid-session.

## Where it does *not* pulse

The animation is live-region-only, and the renderer split says so:

| renderer | pulse | why |
| --- | --- | --- |
| `live_tool_lines` (the strip) | **yes** | redrawn every frame, never committed |
| `running_command_lines` (a streaming `bash`) | **yes** | the same strip |
| `agent_view_preview_lines` (an agent session view) | **yes** | the same strip, for the viewed agent |
| `tool_lines` (scrollback commits, resize repaint) | no | a committed colour is frozen **forever** — it must never capture a frame mid-breath |
| `tool_full_lines` (the Ctrl+O transcript) | no | see below |
| the permission prompt's context rows | no | nothing is running: the call waits on *you*, and the prompt is a still frame |

`tool_lines` and `live_tool_lines` are the same function with the phase as
`None`/`Some` — one body, so the two can't drift.

The **Ctrl+O transcript** sits still on purpose. It is a pager over an
incrementally-built cache whose refresh short-circuits on a signature with no
clock in it (`docs/tool-view-performance.md`); animating there would either not
move at all or cost a full live-tail re-render every 32 ms, for a bullet nobody
opened the overlay to watch breathe. Its running bullet renders at rest — still
the grey, still distinct from the green/red around it.

## What stayed blue

`context_user_color()`, the Ctrl+D context view's `user:` role tag, used to
alias the running colour. It is a *role* tag, not a running state, so it keeps
the link blue (the palette's `link` role — `#61AFEF` in the One Dark theme) as
its own value and the two simply parted ways.

## Tests

- `ui/tests/tool.rs` — the running bullet is the permission prompt's grey; it
  reaches `tool_pulse_dim()` at the top of the cycle and `tool_pulse_bright()` at the
  half, is genuinely in between at the quarter, and loops; the peak is the
  resting grey, so the breath never brightens past it; `Waiting`/`Ok`/`Failed`
  never animate whatever the clock says; and `tool_lines` — the renderer that
  feeds scrollback — never freezes the dip.
- `ui/tests/live.rs` — the wiring: `render_live` paints the strip's bullet at the
  injected phase, and moving `App::set_pulse` visibly changes the painted cell.
- `ui/tests/agent.rs` — the live agent group's bullet breathes on the same clock.
- `smoke.sh` Phase 57 — in a real terminal, a running cell's bullet is sampled
  across frames and must actually change, while its `⎿ Waiting…` sibling's does
  not, and the resolved cell lands green.

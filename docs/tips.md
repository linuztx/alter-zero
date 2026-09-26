# Spinner tips — a hint under the status line while a turn runs

Claude Code hangs a dim `⎿  Tip: …` row off its spinner while it works: a
key or a command worth knowing, offered at the one moment the user is
watching the screen with nothing to do. This is that row.

```
❯ refactor the parser

⣤⣀⣀⣀⣀⣀⣀⣀ Working… (5s · ↓ 120 tokens · esc to interrupt)
  ⎿  Tip: Press ctrl+o to see the whole transcript and every tool's output

────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────
```

## What shows, and when

- **Not at once.** A turn opens with the status line alone; the tip appears
  once the turn has run [`TIP_DELAY`] (3 s). A quick answer never flashes
  one — it is there for the user who is actually waiting. Three seconds is
  the "a few seconds" the Ctrl+B hint already waits
  (`TOOL_BACKGROUND_HINT_DELAY`, `docs/background.md`), so the strip's two
  delayed hints keep one pace.
- **One tip per turn, then a new one every [`TIP_ROTATION`]** (3 minutes)
  of the same turn — measured from when the tip appeared. Most turns end
  inside the first slot, so what the user mostly sees is *the next tip at
  the next turn*; a long agentic turn gets a fresh one every few minutes,
  slow enough never to pull the eye off the reply streaming above it (the
  verb already moves every 30 s, `docs/status-indicator.md`).
- **It hangs off the status line** — directly under it, in the `⎿` gutter
  a tool's output hangs from, inside the status slot and above its trailing
  gap. Dim throughout, word-wrapped under the gutter on a terminal too
  narrow for it (every tip fits one row at 80 columns).
- **Live only.** The row goes when the turn does and is never committed:
  no scrollback line, no history item, nothing in Ctrl+O, the rollout or the
  context.
- **Where it doesn't show:**
  - while the **task checklist** is up — the plan takes the slot: it is
    the more useful thing to hang off the spinner (`docs/task-tools.md`);
  - on a **`!` shell** turn, which has no status line to hang from
    (`docs/shell-command.md`);
  - inside an **agent session view**, whose status line is the subagent's
    (`docs/agent-tool.md`);
  - with **Show tips** off (below).

  A `/compact` turn is an ordinary status-line turn and shows one.

## The catalog

`tips::TIPS` — one sentence each, in the order they are walked. Every entry
names something the app really has: a key (`ctrl+o`, `shift+tab`, `alt+↑` —
the `?` band's spelling), a slash command, or a CLI form, and the unit tests
hold the catalog to the rules a reader relies on:

- **one row at 80 columns** under the `⎿  Tip: ` prefix;
- **every `/command` it names is in `COMMANDS`**, so a renamed command
  cannot leave a tip pointing at nothing;
- no duplicates, nothing empty, no trailing period (Claude Code's tips read
  as fragments of a sentence, and so do these).

The order interleaves keys, commands and the command line so neighbours
differ, and opens on the tip that leads to all the others (`?` for the
shortcuts). Deterministic — no RNG, the verb table's rule.

## The walk

The app keeps a **cursor** into the catalog: the index the next tip shown
opens on. A turn **draws** a tip at the first frame its slot is on screen —
the slot being which [`TIP_ROTATION`] of the turn's elapsed (past the delay)
the clock is in — and the draw takes the cursor's tip and moves the cursor
one on. A later slot draws again; a frame in the same slot keeps the tip it
drew.

Two consequences follow, both deliberate:

- **The cursor moves only when a tip is seen.** A turn that ends before the
  delay draws nothing, so the next turn shows the tip it would have — a
  string of quick answers does not burn through the catalog unseen. Likewise
  a turn spent under the task checklist or with tips turned off — and any
  frame on which the strip is not on screen at all: a permission prompt or an
  `AskUserQuestion` modal replaces the whole live region, and Ctrl+O, Ctrl+D
  or `/resume` covers it, so the draw waits for the conversation to come back
  (`App::modal_open`, `View::is_overlay`).
- **A tip does not change while it is hidden.** It is drawn for its slot,
  so a checklist that comes and goes, or Show tips toggled off and back on,
  returns the same tip, not the next.

All of it runs in `App::set_status_times` — the one place the boundary's
clock reaches the status (`docs/status-indicator.md` *Where the impurity
lives*) — so the pure core stays deterministic and the tests drive it with
plain `Duration`s.

`App`'s cursor is an `Option`, **`None` until the boundary seeds it**
(`App::seed_tips`): the unit-test default, exactly like the footer's session
info, so every existing test that runs a turn past three seconds sees the
strip it always saw.

## `tips.json` — per user

```json
{
  "enabled": true,
  "next": 7
}
```

`{config_home}/tips.json` holds the **Show tips** switch and the cursor.
It is per *user*, beside `telemetry.json` and `update.json`
(`docs/per-directory-state.md` *What is not per directory*): whether you want
tips is yours, not a project's — a switch turned off in one directory and
back on in the next would read as the setting not working — and the walk
should carry on across sessions and directories: the next session opens on
the tip after the last one shown, anywhere, so every tip comes round before
any repeats — a rotation, with no history kept per tip.

The boundary reads it at bootstrap (seeding both the setting and the
cursor), and writes the cursor back **at the loop bottom whenever it
moved** — beside the other on-disk mirrors in `Session::after_iteration`,
an integer compare on every other iteration. Every write is a
read-modify-write over the file (`config::update_tips_file`, the
`update.json` helper's twin), so a concurrent session's switch is never
clobbered by this one's cursor. Absent, unreadable or corrupt, the file
reads as the defaults — on, from the first tip — and a cursor past the end
of a catalog that shrank simply wraps.

## Turning tips off

- **`/settings` → Show tips** — `true`/`false`, persisted to `tips.json`
  (never `settings.json`: `SessionSettings::tips` is `#[serde(skip)]` and
  `copy_value` never moves it — the Telemetry row's pattern). Toggled
  mid-turn, the row goes (or comes) at the next frame.
- **`ALTER_ZERO_TIPS=0`** seeds the row off for a run, never saved — the
  `ALTER_ZERO_TOOLS` grammar. Not a privacy switch, so unlike
  `ALTER_ZERO_TELEMETRY` it does not withdraw the row: the user can still
  turn tips back on for the session.
- The smoke suite exports `ALTER_ZERO_TIPS=0` in its base environment
  (`scripts/smoke/lib.sh`), so a phase that holds a turn past three seconds
  sees the strip it asserts on; the tips phase turns them back on.

## Where the pieces live

| Piece | Where |
|---|---|
| The catalog, [`TIP_DELAY`], [`TIP_ROTATION`], the slot arithmetic, the `tips.json` format | `src/tips.rs` (pure) |
| The walk: the cursor, the draw, what hides the row | `src/app/tips.rs` (`App::set_status_times` calls it) |
| The row: `  ⎿  Tip: …`, wrapped under the gutter | `src/ui/tips.rs` |
| Its height: the status line's **hanging rows** — the checklist, else the tip — threaded through `live_height`/`live_layout`/`cursor_position`/the strip exactly as the checklist was | `ui::hang_rows` (`src/ui/layout.rs`) |
| Seeding, persisting, the `/settings` apply, `ALTER_ZERO_TIPS` | `src/tui/bootstrap.rs`, `src/tui/settings.rs`, `src/tui/config.rs` |

The checklist and the tip share one slot, so they share one row count:
`strip_rows`/`live_height`/`live_layout`/`input_box` take the **hanging
rows** where they took the checklist's alone, and every caller hands them
`ui::hang_rows` — the one sum, spelled once, so the rows the region reserves
and the rows `strip_lines` paints cannot disagree. The strip's scrollback
flow (`docs/strip-flow.md`) does not sign the tip: the row sits low in the
strip, which a squeezed strip keeps on screen, and a tip appearing changes
only the strip's height — which the flow re-signs on by itself.

## Testing

Unit: `src/tips.rs` (the slot arithmetic at the delay and rotation edges,
the catalog rules above, the lenient `tips.json` parse and its defaults),
`src/app/tests/tips.rs` (no tip before the delay or unseeded, the draw and
the cursor, one tip per slot, the next turn's next tip, the cursor held by a
turn that ended early, every hiding condition — a modal and an overlay
included — the same tip back after a hide), `src/settings/tests.rs` (the
row: label, default, cycle, never in `settings.json`), `src/ui/tests/tips.rs`
(the row's text and dim dress, the wrap, the height threading through the
live region, the strip and a picker's strip above itself).

`scripts/smoke.sh` Phase 125 drives the binary: `ALTER_ZERO_TIPS=0` keeping
a turn tipless and `tips.json` unwritten; on a fresh config home no tip in a
turn's first seconds, then the catalog's first directly under the status
line over the slot's gap once three have passed, gone with the turn and
never in scrollback; the next turn on the next tip and `tips.json` recording
the cursor; a relaunch carrying the walk on; `/settings` → Show tips turning
the row off, persisting that to `tips.json` (not `settings.json`) and a
relaunch keeping it off; and `ALTER_ZERO_TIPS=1` turning a run back on
without saving anything.

[`TIP_DELAY`]: ../src/tips.rs
[`TIP_ROTATION`]: ../src/tips.rs

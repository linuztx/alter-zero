# Usage tips under the status line

A few seconds into a turn, a dim `⎿  Tip: …` row appears under the spinner
line and names something the user may not know yet — Claude Code's spinner
tip, in the place its task list already has here:

```
❯ refactor the parser

⣀⣀⣰⣆⣀⣀⣀⣀ Working… (6s · ↑ 12 tokens · esc to interrupt)
  ⎿  Tip: Press Ctrl+O to open the full transcript, every tool output included

────────────────────────────────────────────────────────────────────────────
❯ ▏
```

The row is **live-only**: it hangs off the status line while the turn runs
and goes with it. Nothing about it reaches history, the rollout or the
Ctrl+O transcript — it is chrome for a wait, not a record of anything.

## What Claude Code does, and what this ports

Claude Code's `Spinner` picks one tip when a turn starts (`getSpinnerTip`,
run from the query callback), asynchronously — its relevance checks read the
config and the git state — and shows it under the spinner as a dim
`Tip: {content}` for the rest of the turn; the next turn picks again. The
catalog is a list of `{id, content, cooldownSessions, isRelevant}` records:
a tip that does not apply (the Shift+Enter tip once the keybinding is
installed, the IDE tip inside an IDE) is filtered out, a tip shown within
its last `cooldownSessions` launches is filtered out (`tipsHistory` in the
config records the launch count each tip was last shown at), and of what is
left the one shown **longest ago** wins — never-shown tips first, in catalog
order. Two time-based tips override the pick inside a long turn: `/btw`
after thirty seconds, `/clear` after thirty minutes. The whole thing is off
with the `spinnerTipsEnabled` setting.

This is that, in the shape the status verbs already have here
(`docs/status-indicator.md`, *The verb: rotation and the summary*):

- **A walk, not a per-tip cooldown.** The catalog is walked in order and the
  cursor is one past the last tip a line showed — the next turn opens on the
  next tip, and a relaunch opens after the last one *any* session showed.
  Claude Code's "the tip shown longest ago wins" over a per-tip history is,
  for a catalog every entry of which applies, exactly this walk; the walk
  needs one value to persist instead of a map, and it can never repeat a
  tip before the whole catalog has come round.
- **A delay, then a rotation.** Nothing shows until the turn is
  [`TIP_DELAY`] (5 s) old — a quick answer keeps its strip to the status
  line, since a tip under a spinner about to vanish is a flash, not a hint —
  and a long turn moves on to the next tip every [`TIP_ROTATION`]
  (3 minutes). Claude Code holds one tip per turn and swaps it only for its
  two time-based ones; an agentic turn here can run twenty minutes, and one
  sentence under the spinner for all of it says nothing new after the
  first. Three minutes is long enough to have been read and short enough
  that a long wait shows a few; it is a deliberately slow cadence beside
  the verb's thirty seconds, because the tip row is prose the eye reads
  rather than a word that signals *still going*, and a row that keeps
  changing under a streaming reply is a distraction. Both are constants in
  `app::tips`.
- **Relevance, reduced to facts the pure core holds.** Each tip names the
  [`TipFeature`] it needs — `Always`, `Permissions` (the gate is on,
  `App::permission_mode` is `Some`: a Shift+Tab tip is dead advice under
  `ALTER_ZERO_PERMISSIONS=0`), `Thinking` (the model reasons, `App::thinking`
  is `Some`: Ctrl+T cycles nothing otherwise), `Skills` (the `$` picker's
  snapshot is non-empty). The walk steps over a tip whose feature is absent,
  so the same seed lands on the next applicable one; a session where every
  tip's feature is absent shows none (not reachable with the catalog as it
  is — most tips are `Always`).
- **One switch, per directory like every knob.** The `/settings` **Tips**
  row (`docs/settings.md`), seeded from `ALTER_ZERO_TIPS`; off, the row
  never renders **and the walk stands still**, so turning it back on resumes
  where it was rather than having silently spent tips nobody saw.

## The walk, precisely

`TurnStatus` carries `tips_from: Option<usize>` — the catalog index the
turn's walk opens on, `Some(App::tip_cursor)` for a model turn and a
`/compact` turn (a summarization is a wait like any other), `None` for a `!`
shell turn (whose status line is hidden anyway, `docs/shell-command.md`) and
for the per-draw copies that walk nothing (the agent session view's, the
`/spinner` picker's sample) — and `tip: Option<usize>`, the index showing
now. `App::set_status_times`, the boundary's once-per-draw clock injection,
runs the verb rotation and then `walk_tip`:

```
walk(start, elapsed, applies):
  elapsed < TIP_DELAY                       → None
  order = the applicable indices from `start`, wrapping, in catalog order
  order is empty                            → None
  steps = (elapsed − TIP_DELAY) / TIP_ROTATION
  → order[steps mod len(order)]
```

A pure function of `(start, elapsed)` — like the verb's — so the tip a frame
shows depends on nothing but the clock, and a test pins every transition
with explicit durations. When the result changes, the status takes it, the
cursor moves to one past it (the next turn's opening), and the id is queued
for the boundary (`App::take_tip_record`). `App::tip` is what the strip
reads: the shown tip while a status line is up **and the Tips row is on**
(a switch flipped mid-turn hides the row at once — the walk is gated on it
too, so it also stops moving).

## The row

`ui::tips::tip_line` builds it, under the same rules as the task checklist's
rows, whose place it takes:

- **On the status slot's gap row** — the blank row between the status line
  and the box's top rule — in the tool gutter (`  ⎿  `) with the dim `Tip: `
  label ([`TIP_PREFIX`]) and the dim sentence after it: the whole row is
  `tool_dim_color()`, nothing bold — Claude Code renders it `dimColor`. A
  hint, not an alert, and still the row that keeps the status line off the
  rule.
- **One row, clipped, never wrapped**: at a narrow width the sentence is cut
  with a closing `…` the way a task subject's row is; the catalog keeps
  every tip at seventy columns or fewer so an 80-column terminal never clips
  (pinned by `every_tip_has_a_unique_id_and_fits_one_row_at_eighty_columns`).
  Why one row, and why the gap's, is the geometry below.
- **The checklist wins the slot.** While the task list has rows
  (`docs/task-tools.md`) the tip renders nothing — Claude Code's spinner
  shows its task list *instead of* the tip, and two things hanging off one
  spinner is a pile — and the list keeps its blank gap. The moment the plan
  retires the tip is back.
- **The main session's only.** An agent session view shows that agent's
  status line and no tip: the tips are about the composer the user is typing
  into and the session's own commands, and a row about `/compact` under a
  subagent's spinner would be advice about the wrong conversation.

### Geometry: the tip adds no row

The first cut gave the tip a row of its own under the status line, the
checklist's shape, and `smoke.sh` Phase 5 failed at once: the box no longer
stayed flush at the bottom after a turn. The reason is the turn-end
collapse (`docs/status-indicator.md` *Strip geometry*, `docs/design.md`
*Streaming strip collapse*). When a reply finishes, the strip's rows —
preview, gap, status, gap — are handed back to scrollback as the committed
final line, its spacer, the summary and its spacer: four rows for four, so
the commit refills exactly what the strip vacates and the box does not move.
A tip row on top of that vacated one row more than the commit refilled (three
more at a width that wrapped it), and a terminal cannot pull scrollback back
down to cover the difference: the box rose, and a blank band sat under it
until the next commit. The only other cure would have been a purge rebuild
at the end of every turn that showed a tip — the answer the permission
prompt's close takes, and far too expensive to pay per turn (it rewrites the
whole history and re-uploads every picture).

So the tip is drawn **on the gap row** the slot already has, and clipped to
one row. `strip_rows(has_status, preview_rows, task_rows)` is untouched —
the strip is exactly as tall with a tip as without, every geometry caller
(`live_height`, `live_layout`, `input_box`, `cursor_position`, the
boundary's `live_region_height`) sees the same numbers, and the collapse
refills row for row as before. `strip_lines` pushes `tip_line`'s row where
it would push the blank gap; with the checklist up `tip_line` is `None`, so
the list keeps its gap.

The strip's scrollback flow (`docs/strip-flow.md`) signs on the shown tip's
id: a tip coming up or moving on is a structural change to the strip, so a
flowed strip re-signs rather than freezing a stale tip row in scrollback —
but not on its rows or the clock, so the 32 ms animation never re-signs it.

## Across launches: `tips.json`

The walk's position persists **per user** in `{config_home}/tips.json`, its
own file beside `telemetry.json` and `update.json` — a tip is about the app,
not the directory, so a per-directory file would show a new checkout the
same first tip every time:

```json
{ "last": "compact" }
```

One value, the id of the last tip shown ([`TipsFile`], every field optional,
a malformed file reads as nothing known). Ids rather than indices, so the
catalog can be reordered or reworded without losing the walk's place; an id
this build does not know starts at the first tip. The boundary seeds the
walk at bootstrap (`App::seed_tips`, the `set_clock` pattern) and writes the
file from the draw tick whenever `take_tip_record` hands it a newly shown
tip — once every few minutes at most, a whole-file write since the file
holds nothing else, a failed write swallowed (the next launch merely opens
on the first tip). Written at show time rather than at turn end so a Ctrl+C
mid-turn loses nothing.

## The catalog

[`TIPS`] in `app::tips`: one sentence per feature of this app in Claude
Code's register — what to press, what it does — a stable slug `id`, and the
feature it needs. The order is the order a new user meets things: the
composer's keys first (Enter into the running turn, Esc-Esc, Ctrl+O, Ctrl+D,
Shift+Tab, `@`, `$`, Ctrl+R, `!`, Ctrl+V, Shift+Enter, Ctrl+T, Ctrl+B, ↓,
Alt+↑, `?`), then the commands (`/compact`, `/resume`, `/copy`, `/export`,
`/init`, tasks, subagents, `/model`, `/settings`, checkpoints, `/theme`,
`/spinner`, `/mascot`, `/skills`, `/mcp`, hooks). A tip is one sentence,
no closing period, at most seventy columns, and never starts with the
`Tip:` the renderer adds — all pinned by the catalog test. Adding one is one
`Tip::new` row; a tip about a feature that can be absent names its
[`TipFeature`], and a new feature kind is one variant plus its arm in
`App::tip_applies`.

## Tests

- `app::tests::tips` — the walk: nothing under the delay, the first tip at
  it, the rotation, the wrap around the applicable set, the next turn
  opening after the last shown tip, a turn that showed none leaving the
  cursor alone, the seed (known, unknown, absent, the catalog's last tip
  wrapping to its first), the relevance gates (permissions, thinking,
  skills), the switch (no row, no movement, resumes in place), a `!` shell
  turn and a `/compact` turn, the record handed to the boundary once per
  tip, the catalog's invariants, and the file's round trip.
- `ui::tests::tips` — the row: its place on the gap row under the status
  line, its dim dress, the clip at a narrow width, the geometry left exactly
  as it was (`live_height` unchanged, the box's rule right under the tip),
  the checklist displacing it and keeping its gap, the switch, and the flow
  key.
- `smoke.sh` Phase 125 — the real binary under tmux: no tip on the status
  line's first frame, the `⎿  Tip:` row within a few seconds, gone with the
  turn and absent from scrollback, the next turn's tip a different one, a
  relaunch opening on a third (`tips.json` written), and `ALTER_ZERO_TIPS=0`
  showing none.

[`TIP_DELAY`]: ../src/app/tips.rs
[`TIP_ROTATION`]: ../src/app/tips.rs
[`TIPS`]: ../src/app/tips.rs
[`TipFeature`]: ../src/app/tips.rs
[`TipsFile`]: ../src/app/tips.rs
[`TIP_PREFIX`]: ../src/ui/theme.rs

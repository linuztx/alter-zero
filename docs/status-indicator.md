# Status indicator (codex / Claude-Code style)

A live status line while a turn is in flight, plus a persistent "done" summary —
modelled on the spinner line in openai/codex and Claude Code.

```
> Hi                               (committed)

● Happy to help! …                 (streaming preview — existing)

● Working… (1s · ↓ 100 tokens)     (NEW: live status line)
────────────────────────────────   (input box)
❯ ▏
────────────────────────────────
```

On finish the status line is replaced by a dim summary committed to scrollback:

```
> Hi
● Happy to help! …                 (the finished reply)
Done for 20s                       (NEW: committed turn summary)

> Thanks                           (the next turn — "Done for 20s" stays put)
```

## What shows, and when

The live line is `● {verb}… ({elapsed}s[ · {arrow} {n} tokens][ · Thinking for {m}s])`:

| phase                | line                                              |
|----------------------|---------------------------------------------------|
| just submitted       | `● Working… (0s)`                                  |
| streaming text       | `● Working… (1s · ↓ 100 tokens)`                   |
| streaming + thinking | `● Working… (1s · ↓ 150 tokens · Thinking for 0s)` |
| after a tool result  | `● Working… (1s · ↑ 200 tokens)`                   |
| finished (committed) | `Done for 20s`                                     |

- **verb** — a whimsical word (`Working`, `Cooking`, …) chosen *once per turn*, and
  a matching **done verb** (`Done`, `Finished`, …) for the summary. Picked by a
  per-turn counter (`App::turn_count`) so it varies across turns yet stays
  deterministic — no RNG, testable like `dummy_response`.
- **elapsed** — whole seconds since the turn was submitted. Ticks even when no
  events arrive (a 1 s timer in the loop).
- **tokens** — a single cumulative tally for the whole turn (text **and** tool
  output), estimated app-side (≈ `chars / 4`). It is **never reset** mid-turn.
  Omitted while it is 0 (the "just submitted" state).
- **arrow** — `↓` while the reply streams (output), flipping to `↑` right after a
  tool result (its output is "uploaded" back); the count keeps growing either way.
- **Thinking for {m}s** — shown *only while actively thinking*; dropped once
  thinking ends.

## Where the impurity lives (boundary, not the pure core)

Time is impure, so — exactly like `docs/timestamps.md` — it stays in `main.rs`:

- the loop owns `turn_start` / `thinking_start` `Instant`s;
- a `tokio::time::interval(1s)` branch schedules a frame each second while a turn
  is active (`App::turn_active`), so the seconds tick with no events;
- before each draw the loop writes the computed `elapsed_secs` / `thinking_secs`
  onto the live status via `App::set_status_times`.

The pure `App` owns only what *isn't* time: the chosen verbs, the token tally, and
the `↓`/`↑` arrow. `ui::status_line(&TurnStatus)` is a pure formatter that reads the
struct (with the boundary-supplied seconds) — unit-tested with explicit values.

## State (App)

- `TokenArrow { Down, Up }`.
- `TurnStatus { verb, done_verb, tokens, arrow, elapsed_secs, thinking_secs }`
  (`elapsed_secs`/`thinking_secs` are written by the boundary each frame;
  `thinking_secs: Option<u64>` is `Some` only while thinking).
- `App.status: Option<TurnStatus>` — `Some` from `begin_stream` to turn end
  (`turn_active()` == `status.is_some()`).
- `App.turn_count: usize` — drives verb selection.
- `begin_stream` creates the status (picks verbs, increments the counter);
  `push_chunk` adds tokens (`↓`); `end_tool` adds tokens (`↑`); `fail_stream`
  clears the status (an error is the summary — no "Done" line); `end_turn(secs)`
  records the summary and clears the status.

## The "Done for Ns" summary

A new ordered history entry so it survives a resize and lists in the Ctrl+O
transcript (with a timestamp, like every other item):

- `TurnSummary { verb, secs, timestamp }`, `HistoryItem::Summary(TurnSummary)`.
- `ui::summary_lines` renders a single dim, bullet-less `"{verb} for {secs}s"`.
- `App::end_turn(elapsed_secs)` (called by the loop on `StreamDone`) pushes it and
  returns it for the loop to commit to scrollback. On `StreamDone` the loop
  reseats the viewport to the idle box height first (the strip is gone) — same
  flush-at-the-bottom dance the final reply line already uses.

## Strip geometry

The strip above the box already shows the streaming/tool **preview + gap**; the
status line is a third strip row pinned at its bottom (just above the box):

```
strip = preview (1) + gap (1) + status (1) = 3 rows while a turn streams, else 0
```

`strip_rows(streaming)` grows from 2 → 3; `render_live` draws `status_line` in the
bottom strip row. No `live_height`/`live_layout` signature changes — the existing
`streaming` flag still drives the whole strip.

## Thinking in the dummy backend

Two new opaque events, `StreamEvent::ThinkingStart` / `ThinkingEnd`, are emitted by
`DummyAi` after the first text segment (so the demo shows `↓ tokens · Thinking
for Ns`), with a short pause so the thinking timer is visible. The loop maps them
to `thinking_start = Some(now)` / `None`; nothing in the pure `App` knows about
thinking — its seconds reach the status only through `set_status_times`.

## Testing

- `app`: verbs cycle per turn; tokens accumulate (`↓`) and survive a tool (`↑`,
  not reset); `end_turn` records the summary and clears status; `fail_stream`
  clears status; `set_status_times` writes the seconds.
- `ui`: `status_line` for each phase (no tokens at 0; `↓`/`↑`; `Thinking for`);
  `summary_lines` is one dim line; the strip gains the status row; `conversation`
  / `transcript` render a `Summary`.
- `stream`: `turn_events` emits a paired `ThinkingStart`/`ThinkingEnd`; chunks
  still reconstruct the reply.
- `main.rs` (smoke): the live line shows `tokens` while streaming and a committed
  `Done for Ns` after the turn settles.

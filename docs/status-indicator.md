# Status indicator (codex / Claude-Code style)

A live status line while a turn is in flight, plus a persistent "done" summary —
modelled on the spinner line in openai/codex and Claude Code.

```
> Hi                                                        (committed)

● Happy to help! …                                          (streaming preview — existing)

( ●    ) Working… (1s · ↓ 100 tokens · esc to interrupt)    (live status — ball bounces, verb shimmers)
                                                            (blank gap so the status clears the box)
─────────────────────────────────────                       (input box)
❯ ▏
─────────────────────────────────────
```

On finish the status line is replaced by a dim summary committed to scrollback:

```
> Hi
● Happy to help! …                 (the finished reply)
Done for 20s                       (NEW: committed turn summary)

> Thanks                           (the next turn — "Done for 20s" stays put)
```

## What shows, and when

The live line is
`( ●    ) {verb}… ({elapsed}s[ · {arrow} {n} tokens][ · Thinking for {m}s] · esc to interrupt)`:

| phase                | line                                                                          |
|----------------------|-------------------------------------------------------------------------------|
| just submitted       | `( ●    ) Working… (0s · esc to interrupt)`                                    |
| streaming text       | `(  ●   ) Working… (1s · ↓ 100 tokens · esc to interrupt)`                     |
| streaming + thinking | `(   ●  ) Working… (1s · ↓ 150 tokens · Thinking for 0s · esc to interrupt)`   |
| after a tool result  | `(    ● ) Working… (1s · ↑ 200 tokens · esc to interrupt)`                     |
| finished (committed) | `Done for 20s`                                                                 |
| interrupted (Esc)    | *no summary* — the red `Conversation interrupted` notice (see `docs/interrupt.md`) |

- **spinner** — the line opens with the classic cli-spinners **`bouncingBall`**:
  a white bold ball ping-ponging between dim parenthesis walls, one frame per
  80 ms (`( ●    )` → `(  ●   )` → … → `(     ●)` → back; 8 fixed-width frames,
  so nothing after it jitters). Like the shimmer, `ui::spinner_spans` is pure —
  the frame index derives from the boundary-supplied `elapsed`.
- **verb** — a whimsical word (`Working`, `Cooking`, …) chosen *once per turn*, and
  a matching **done verb** (`Done`, `Finished`, …) for the summary. Picked by a
  per-turn counter (`App::turn_count`) so it varies across turns yet stays
  deterministic — no RNG, testable like `dummy_response`. The white verb text
  carries a white **shimmer wave** (below).
- **elapsed** — whole seconds since the turn was submitted. Advances even when no
  events arrive (the draw branch re-arms an animation frame while a turn is
  active — see the shimmer section).
- **tokens** — a single cumulative tally for the whole turn (text **and** tool
  output), estimated app-side (≈ `chars / 4`). It is **never reset** mid-turn.
  Omitted while it is 0 (the "just submitted" state).
- **arrow** — `↓` while the reply streams (output), flipping to `↑` right after a
  tool result (its output is "uploaded" back); the count keeps growing either way.
- **Thinking for {m}s** — shown *only while actively thinking*; dropped once
  thinking ends.
- **esc to interrupt** — the closing clause, always present while the line
  shows: codex's discoverability hint for the Esc interrupt
  (`docs/interrupt.md`). An interrupted turn gets **no** `Done for Ns` summary —
  the committed `Conversation interrupted` notice is its terminal state, like a
  backend error.

## Where the impurity lives (boundary, not the pure core)

Time is impure, so — exactly like `docs/timestamps.md` — it stays in `main.rs`:

- the loop owns `turn_start` / `thinking_start` `Instant`s;
- while a turn is active the draw branch **re-arms** the next animation frame
  (`schedule_frame_in(32ms)` — codex's status-widget cadence), so the timer
  advances and the shimmer sweeps even through event-less pauses (a tool run, a
  thinking pause); the chain seeds from the Submit keypress's frame and stops by
  itself on the first draw after the turn ends;
- before each draw the loop writes the computed `elapsed` / `thinking`
  `Duration`s onto the live status via `App::set_status_times`.

The pure `App` owns only what *isn't* time: the chosen verbs, the token tally, and
the `↓`/`↑` arrow. `ui::status_line(&TurnStatus)` is a pure formatter that reads the
struct (with the boundary-supplied durations) — unit-tested with explicit values.

## State (App)

- `TokenArrow { Down, Up }`.
- `TurnStatus { verb, done_verb, tokens, arrow, elapsed, thinking }`
  (`elapsed: Duration` / `thinking: Option<Duration>` are written by the boundary
  each frame; `thinking` is `Some` only while thinking. A `Duration` rather than
  whole seconds so one value drives both the displayed seconds and the shimmer's
  sub-second phase).
- `App.status: Option<TurnStatus>` — `Some` from `begin_stream` to turn end
  (`turn_active()` == `status.is_some()`).
- `App.turn_count: usize` — drives verb selection.
- `begin_stream` creates the status (picks verbs, increments the counter);
  `push_chunk` adds tokens (`↓`); `end_tool` adds tokens (`↑`); `fail_stream`
  clears the status (an error is the summary — no "Done" line); `interrupt_turn`
  clears it the same way (the `Conversation interrupted` notice is the summary);
  `end_turn(secs)` records the summary and clears the status.

## The "Done for Ns" summary

A new ordered history entry so it survives a resize and lists in the Ctrl+O
transcript (stamp-free there — only user messages display a timestamp):

- `TurnSummary { verb, secs, timestamp }`, `HistoryItem::Summary(TurnSummary)`.
- `ui::summary_lines` renders a single dim, bullet-less `"{verb} for {secs}s"`.
- `App::end_turn(elapsed_secs)` (called by the loop on `StreamDone`) pushes it and
  returns it for the loop to commit to scrollback. On `StreamDone` the loop
  reseats the viewport to the idle box height first (the strip is gone) — same
  flush-at-the-bottom dance the final reply line already uses.

## Strip geometry

The strip above the box already shows the streaming/tool **preview + gap**; the
status line is a third strip row, followed by a blank gap so it never butts up
against the box's top rule (mirroring the gap under the preview):

```
strip = preview (1) + gap (1) + status (1) + gap (1) = 4 rows while a turn streams, else 0
```

`render_live` draws `status_line` at `PREVIEW_ROWS + GAP_ROWS`; the
`STATUS_GAP_ROWS` row below it stays blank. No `live_height`/`live_layout`
signature changes — the existing `streaming` flag still drives the whole strip.

## The shimmer wave (ported from openai/codex)

The verb (`Working…`) renders one **bold span per char**, colours from
`ui::shimmer_spans` — a faithful port of codex `tui/src/shimmer.rs`:

- a raised-cosine brightness band (`t = ½(1 + cos(π·dist/5))`, half-width 5
  chars) sweeps the text once per **2 s**, with 10 chars of off-text padding on
  each side so it slides on and off the ends;
- each char blends from the white-grey base `(0x88,0x88,0x88)` toward bright
  white `(0xFF,0xFF,0xFF)` by `t · 0.9` — white text, noticeably brighter at the
  crest;
- codex reads a process-start clock inside the renderer; our port stays **pure**
  by deriving the phase from the boundary-supplied `TurnStatus::elapsed`
  (sub-second resolution), so tests pin the wave at exact phases;
- codex animates by re-arming a frame from its widget every 32 ms; our loop does
  the same from the draw branch while `App::turn_active()` (replacing the earlier
  1 s ticker — the seconds now also advance from these frames).

The `SHIMMER_*` constants (base/highlight colours, sweep, padding, band width,
max blend) live with the other styling consts at the top of `ui.rs`.

## The bouncing-ball spinner

The line's opening `( ●    )` is cli-spinners' **`bouncingBall`** (codex has no
equivalent — it shimmers a static `•`): eight fixed-width frames
(`SPINNER_FRAMES`), the ball stepping one cell per `SPINNER_INTERVAL` (80 ms) to
the right wall and back, looping every 640 ms. `ui::spinner_spans` styles each
frame as three spans — dim left wall, white **bold** ball, dim right wall — and,
like the shimmer, derives the frame index purely from `TurnStatus::elapsed`; the
same 32 ms draw re-arm animates it. Fixed-width frames mean the verb after the
spinner never shifts as the ball moves.

## Thinking in the dummy backend

Two new opaque events, `StreamEvent::ThinkingStart` / `ThinkingEnd`, are emitted by
`DummyAi` after the first text segment (so the demo shows `↓ tokens · Thinking
for Ns`), with a short pause so the thinking timer is visible. The loop maps them
to `thinking_start = Some(now)` / `None`; nothing in the pure `App` knows about
thinking — its seconds reach the status only through `set_status_times`.

## Testing

- `app`: verbs cycle per turn; tokens accumulate (`↓`) and survive a tool (`↑`,
  not reset); `end_turn` records the summary and clears status; `fail_stream`
  clears status; `set_status_times` writes the boundary durations.
- `ui`: `status_line` for each phase (no tokens at 0; `↓`/`↑`; `Thinking for`);
  the spinner's ball is white bold between dim walls, steps a frame per
  interval, reverses at the right wall, and loops after a full cycle; the verb
  per-char greyscale-white bold spans, the metrics
  dim; the wave's crest is brighter than off-band chars and moves as `elapsed`
  advances; `summary_lines` is one dim line; the strip stacks preview / gap /
  status / gap above the box; `conversation` / `transcript` render a `Summary`.
- `stream`: `turn_events` emits a paired `ThinkingStart`/`ThinkingEnd`; chunks
  still reconstruct the reply.
- `main.rs` (smoke): the live line shows `tokens` while streaming with a blank
  gap row between it and the box, and a committed `Done for Ns` after the turn
  settles.

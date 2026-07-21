# Status indicator (codex / Claude-Code style)

A live status line while a turn is in flight, plus a persistent "done" summary —
modelled on the spinner line in openai/codex and Claude Code.

```
> Hi                                                        (committed)

● Happy to help! …                                          (streaming preview — existing)

(●•·   ) Working… (1s · ↓ 100 tokens · esc to interrupt)    (live status — comet sweeps, verb shimmers)
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
`(●•·   ) {verb}… ({elapsed}s[ · {arrow} {n} tokens][ · retrying {a}/{max}][ · Thinking for {m}s] · esc to interrupt)`:

| phase                | line                                                                          |
|----------------------|-------------------------------------------------------------------------------|
| pre-stream pause     | `(●•·   ) Working… (1s · ↑ 7 tokens · esc to interrupt)`                       |
| streaming text       | `(•●    ) Working… (3s · ↓ 100 tokens · esc to interrupt)`                     |
| streaming + thinking | `(·•●   ) Working… (3s · ↓ 150 tokens · Thinking for 0s · esc to interrupt)`   |
| generating a tool call | `( ·•●  ) Working… (3s · ↓ 170 tokens · esc to interrupt)` (count ticks as the call streams) |
| after a tool result  | `( ·•●  ) Working… (4s · ↑ 200 tokens · esc to interrupt)`                     |
| retrying a failure   | `( ·•●  ) Working… (5s · ↑ 42 tokens · retrying 2/3 · esc to interrupt)`       |
| finished (committed) | `Done for 20s`                                                                 |
| interrupted (Esc)    | *no summary* — the red `Conversation interrupted` notice (see `docs/interrupt.md`) |

The turn opens with a **pre-stream pause** (the backend waits `STARTUP_DELAY`,
3s, before its first chunk — so the indicator is visibly *working* before any
text appears): the just-sent **user message is counted into the tally up front
with the `↑` arrow** (uploaded input, like a tool result folded back in), so the
pause shows `↑ N tokens` and the timer ticks. The first streamed chunk flips the
arrow to `↓`. The pause is `DummyAi`'s (`stream::STARTUP_DELAY`, overridable via
`ALTER_ZERO_STARTUP_DELAY_MS` — the smoke test runs short, one phase long); a
real backend's own latency plays the same role.

- **spinner** — the line opens with a **comet** (a Larson-scanner sweep): a
  white bold head dragging a two-cell fading tail between dim parenthesis
  walls, one frame per 80 ms (`(●•·   )` → `(•●    )` → … → `(   ·•●)` (right
  wall) → `(    ●•)` (the tail whips around at the bounce) → back across to
  `(●•·   )` (left wall, flush against `(` — the leftmost cell is used, not
  wasted); all **ten** fixed-width frames, so nothing after it jitters). Like
  the shimmer, `ui::spinner_spans` is pure — the frame index derives from the
  boundary-supplied `elapsed`.
- **verb** — a whimsical word (`Working`, `Cooking`, …) chosen *once per turn*, and
  a matching **done verb** (`Done`, `Finished`, …) for the summary. Picked by a
  per-turn counter (`App::turn_count`) so it varies across turns yet stays
  deterministic — no RNG, testable like `dummy_response`. The white verb text
  carries a white **shimmer wave** (below).
- **elapsed** — whole seconds since the turn was submitted. Advances even when no
  events arrive (the draw branch re-arms an animation frame while a turn is
  active — see the shimmer section).
- **tokens** — a single cumulative tally for the whole turn (the **user's input**
  message, the reply text, **reasoning deltas**, the **tool-call the model
  generates** (its streamed `name`/`arguments` fragments — counted like reasoning
  so the tally keeps ticking *while the model produces the call*, before it runs),
  and tool output), counted
  app-side by a real `tiktoken` tokenizer (`o200k_base` — the count seam is
  `app::count_tokens` → [`tokenizer::count`], see `src/tokenizer.rs`). Exact for
  current OpenAI models and close for the other models the providers serve. Each
  input/chunk/tool-output is tokenized as it arrives and **added** to the tally
  (`↑`/`↓` per source); a token straddling a chunk boundary can nudge the live
  total a hair above a whole-buffer re-encode — fine for a status estimate, and
  it keeps counting O(text), non-blocking. It is **never reset** mid-turn.
  Omitted only while it is 0 — which, now that the input is counted up front, is
  just the very first frame before `count_user_input` runs.
- **arrow** — `↑` for **uploaded** tokens (the user's input at turn start, and a
  tool result folded back in), `↓` while the reply (or its reasoning) streams.
  So a turn opens `↑` (the counted input during the pre-stream pause), flips `↓`
  on the first chunk, and back to `↑` after each tool; the count keeps growing
  either way.
- **retrying {a}/{max}** — shown in **amber** (`STATUS_RETRY_COLOR`, the only
  non-dim clause) *only while a failed request is being retried*: the
  connection/send failed (or a transient `429`/`5xx` came back) before any
  content streamed, so the real backend is reconnecting (`llm::retry`, see
  `docs/llm.md`). `App::set_retry` sets it from a `StreamEvent::Retrying`; the
  next streamed chunk clears it (the request recovered). The dummy never retries.
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
- `RetryInfo { attempt, max }` — a live retry indicator (see `docs/llm.md`).
- `TurnStatus { verb, done_verb, tokens, arrow, elapsed, thinking, shell, retry }`
  (`elapsed: Duration` / `thinking: Option<Duration>` are written by the boundary
  each frame; `thinking` is `Some` only while thinking. A `Duration` rather than
  whole seconds so one value drives both the displayed seconds and the shimmer's
  sub-second phase. `retry: Option<RetryInfo>` is `Some` only while a failed
  request is being retried).
- `App.status: Option<TurnStatus>` — `Some` from `begin_stream` to turn end
  (`turn_active()` == `status.is_some()`).
- `App.turn_count: usize` — drives verb selection.
- `begin_stream` creates the status (picks verbs, increments the counter);
  `count_user_input(text)` adds the user message's tokens (`↑`) right after, so
  the pre-stream pause shows the input count uploaded; `push_chunk` adds tokens
  (`↓`, and clears any `retry`); `push_thinking` adds tokens (`↓`, the reply
  buffer untouched — reasoning text is opaque, and clears any `retry`);
  `push_tool_call_progress` adds tokens the same way (`↓`, buffer untouched) as
  the model *generates* a tool call — driven by `StreamEvent::ToolCallDelta`, the
  streamed `name`/`arguments` fragments the real backend surfaces before the
  `ToolStart`; `set_retry(a, max)` sets the amber `retrying a/max` clause (from a
  `StreamEvent::Retrying`); `end_tool` adds tokens (`↑`); `fail_stream`
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

The strip above the box has two independent slots, each a content row plus a
trailing gap. The **status line + its gap are reserved while a turn runs**,
*except a `!` shell turn* — which hides the spinner status entirely and shows
its elapsed in the `⎿ Running… (Ns)` preview (`ui::strip_has_status`; see
`docs/shell-command.md`). The **preview + its gap are only reserved when there is
something to preview**, and the preview's *height* is dynamic (`ui::preview_rows`):
a reply whose buffer is non-empty or a running `!` shell command previews **one**
row, while a running backend tool previews its **whole collapsed cell** (the
wrapped `● name(args)` header *plus* its `⎿ Running…` row — `N` rows, so a long
command isn't clipped mid-run; see `docs/tools.md`). During the pre-stream pause
(and any moment before the first chunk) there is no preview, so the strip is just
status + gap and **no empty preview line is reserved** — codex does the same (its
bottom-pane only reserves a blank *separator* when the status is visible, never an
empty content/preview row; `bottom_pane/mod.rs`):

```
strip = (preview_rows > 0 ? preview (N) + gap (1) : 0) + (has_status ? status (1) + gap (1) : 0)
      = preview(1) + gap + status + gap   streaming a reply
      = preview(N) + gap + status + gap   running a backend tool (N = header + `⎿ Running…`)
      = status + gap only                 during the pre-stream pause
      = preview(1) + gap only             during a `!` shell run (status hidden)
      = 0                                 idle
```

`render_live` draws `status_line` below the preview slot (`preview_rows + gap`, 0
when there is no preview, so the status is the strip's top row) *only when*
`has_status`; the `STATUS_GAP_ROWS` row below it stays blank. `live_height`/
`live_layout`/`input_box` take a `has_status: bool` and a `preview_rows: u16`
(the preview content-row count), fed by `strip_has_status`/`preview_rows` from the
`App`-having callers (`render_live`, `cursor_position`, `main.rs`).

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

## The comet spinner

The line's opening `(●•·   )` is a **comet** — a Larson-scanner sweep (codex
has no equivalent — it shimmers a static `•`): **ten** fixed-width frames
(`SPINNER_FRAMES`), the bright head (`●`) stepping one cell per
`SPINNER_INTERVAL` (80 ms) out to the right wall (`(   ·•●)`) and back across
to the left wall (`(●•·   )`, flush against `(`), looping every 800 ms. A
two-cell tail fades behind the head — `•` mid grey, `·` down at the dim
detail grey — whipping around behind it at each bounce (`(   ·•●)` →
`(    ●•)`); a tail cell the head overlaps is hidden under it.
`ui::spinner_spans` styles each frame as one span per cell — the white
**bold** head, the mid-grey `•`, everything else (the faint `·`, walls, empty
track) dim, the right wall carrying the separator space — and, like the
shimmer, derives the frame index purely from `TurnStatus::elapsed`; the same
32 ms draw re-arm animates it. Fixed-width frames mean the verb after the
spinner never shifts as the comet moves.

## The pre-stream pause (dummy backend)

`DummyAi` waits `STARTUP_DELAY` (3s) before playing back its first event, so the
status indicator is visibly working before any reply text — the point of the
pause is to show it off. The wait is an interruptible `nap` (an Esc during it
reaps the thread at once and streams nothing). The delay is configurable
(`DummyAi::with_startup_delay`); `main.rs` reads `ALTER_ZERO_STARTUP_DELAY_MS`
(so the smoke test runs short and one phase long), defaulting to `STARTUP_DELAY`.
A real backend's own first-token latency plays the same role. During the pause
the strip is **status + gap only** — no preview row is reserved (`preview_rows`
is 0 while the reply buffer is empty and no tool runs), so the status sits
one blank below the committed user message, with no stray empty line above it
(the "don't preserve a line" fix; codex reserves no empty preview row either —
see *Strip geometry*).

## Thinking in the dummy backend

`StreamEvent::ThinkingStart` / `ThinkingEnd` are emitted by `DummyAi` after the
first text segment (so the demo shows `↓ tokens · Thinking for Ns`), with
`DUMMY_THINKING` streamed word-by-word as `StreamEvent::ThinkingChunk`s in
between — one `THINK_CHUNK_DELAY` pause per event, so the timer is visible and
the token tally keeps ticking through the phase (a real API's reasoning
deltas; the text is never rendered, only counted via `App::push_thinking`).
The loop maps the pair to `thinking_start = Some(now)` / `None`; the thinking
*seconds* reach the status only through `set_status_times`.

## Tool-call generation in the dummy backend

The same demo trick covers the model *generating* a tool call. Before each
scripted `ToolStart`, `DummyAi` streams a few `StreamEvent::ToolCallDelta`
fragments (`DUMMY_READ_CALL` / `DUMMY_BASH_CALL` — the `name` + JSON `arguments`
pieces), one `THINK_CHUNK_DELAY` pause each, so the token tally visibly ticks
while the call is produced — just like reasoning. The loop counts them via
`App::push_tool_call_progress` (never rendered). A real backend surfaces the
same events straight from its streamed `tool_calls` deltas
(`openai::Delta::tool_call`), ahead of the `ToolStart` that runs the tool.

## Testing

- `app`: verbs cycle per turn; `count_user_input` adds the input tokens with
  `↑` (no-op when idle) and the first chunk flips the arrow back to `↓`; tokens
  accumulate (`↓`) and survive a tool (`↑`, not reset); thinking chunks grow the
  tally (`↓`) without touching the reply buffer; `end_turn` records the summary
  and clears status; `fail_stream` clears status; `set_status_times` writes the
  boundary durations.
- `ui`: `status_line` for each phase (no tokens at 0; `↓`/`↑`; `Thinking for`);
  `preview_rows` is 0 on the pre-stream pause, so `live_height` reserves
  no preview row and `render_live` draws the status as the strip's top row (with
  `↑` tokens, no reserved blank above it); `live_layout` tiles the four areas for
  every `(streaming, has_preview)` combination;
  the spinner's comet head is white bold with a monotonically fading tail
  between dim walls, steps a frame per interval, reverses at the right wall
  (the tail whipping around behind it), and loops after a full cycle; the verb
  per-char greyscale-white bold spans, the metrics
  dim; the wave's crest is brighter than off-band chars and moves as `elapsed`
  advances; `summary_lines` is one dim line; the strip stacks preview / gap /
  status / gap above the box; `conversation` / `transcript` render a `Summary`.
- `stream`: `turn_events` emits a paired `ThinkingStart`/`ThinkingEnd` with
  `ThinkingChunk`s strictly inside the pair; chunks still reconstruct the reply;
  `DummyAi` waits the startup delay before the first chunk (a short delay in the
  test), and a cancel during the wait streams nothing.
- `main.rs` (smoke): the live line shows `tokens` while streaming with a blank
  gap row between it and the box, and a committed `Done for Ns` after the turn
  settles. Phase 20 (longer startup delay): mid-pause the status shows
  `↑ N tokens` with no reply text, then the reply streams with the arrow `↓`.

# The thinking stream

A reasoning model's chain-of-thought used to be **invisible**: the backend
peeled it out of the stream (`llm::thinking::ThinkingSplitter`), wrapped it in
`ThinkingStart`/`ThinkingChunk`/`ThinkingEnd`, and the loop threw the text away
— counting its tokens so the tally kept ticking, and showing nothing but a
`· Thinking for 12s` clause in the status line.

Now it **streams live**, then **collapses**:

```
● Thinking…
  ⎿  I need to look at the file first. The user asked for a modern
      landing page, so the structure should be: a hero, three feature
      cards, a footer. Let me check the repo before writing…
```

…and when the phase ends, the whole block gives way to one committed line:

```
Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)
```

Two shapes, deliberately: while it runs it **is** a tool cell — a `●` bullet
over a `⎿` gutter, because that is what is happening (something working, with
output under it) — and it is the **only** part of the feature that moves: the
bullet blinks like any running tool's, and `Thinking…` carries the status
line's shimmer sweep. When it settles the cell shape goes away entirely: no
bullet, because nothing is happening any more. What is left is a fact about
the turn, in the bullet-less shape `Done for 7s` already uses, with the
`(ctrl+o to expand)` hint saying where the thought went — the same promise a
capped tool peek makes.

## Where the weight goes

All of it to the live block, none to the settled line:

| Surface | Renders | Why |
| --- | --- | --- |
| **Strip** (live) | `● Thinking…` — blinking bullet, shimmering bold-white label | the one place something is still happening, and nothing here is ever committed, so motion is free |
| **Inline** (settled) | one dim row, `Done for Ns`'s exact dress | a finished thought is a footnote about work already done |
| **Ctrl+O** (expanded) | the same dim row, minus the hint | no hint to make room for — this *is* the expansion |

The settled line is dim on both surfaces so a thought looks like the same thing
wherever you meet it, and so it never competes with the assistant reply it sits
directly above.

The full text is never lost — it expands in the **Ctrl+O transcript** like a
tool call's output, records into the rollout, and comes back on `/resume`.

Gated by **`ALTER_ZERO_SHOW_THINKING`**: falsy (`0`/`false`/`no`/`off`) restores
the old invisible behaviour exactly (the status clause and the token tally stay
— only the live block and the committed cell go).

## Why it collapses instead of scrolling into scrollback

Scrollback is **immutable** — `insert_before` queues rows the terminal keeps
forever. A design that streamed reasoning to scrollback could never take it
back, so `Thought for …` could not replace it. So the reasoning block lives
**only in the live region**, in the streaming strip's preview slot (where a
running tool's tail already lives), and the *only* thing that ever commits is
the one-line cell. That is the same collapsed-inline / expanded-in-Ctrl+O
contract every tool call has (CLAUDE.md invariant 4), and it needs no new
machinery in `term.rs`.

## The pieces

### The live block (strip preview)

`ui::live_reasoning_lines` renders

- the `● Thinking…` header — the tool cell's own `TOOL_BULLET`, **blinking**
  at the shared frame pulse (`docs/tool-pulse.md`) exactly like a running
  tool's, beside a label carrying the status line's **shimmer** sweep
  (`ui::status::shimmer_spans_from`, the same wave the `Working…` verb below it
  wears, off the same frame clock); and
- the **tail** of the reasoning so far in the `⎿` gutter
  (`TOOL_RESULT_PREFIX`, via `ui::file_cell::gutter_row_styled` — the corner on
  the first row, the rest aligned under it): the last `REASONING_PEEK_LINES`
  wrapped display rows, word-wrapped with spaces preserved (`ui::wrap_output`,
  the same wrapper the streaming `bash` tail and the Ctrl+O view use), dim and
  **italic** — the one cue separating a thought's `⎿` body from a tool's.

Blank source lines are skipped: reasoning is full of paragraph breaks, and
spending the small window on them would show a third as much thought. Walking
the source lines newest-first wraps only what the window shows, so a long think
costs O(window) per animation frame, not O(reasoning).

**The shimmer's floor is raised here.** Codex's `shimmer_base()` is grey
`#888888`, so the status verb reads as *grey text with a white wave* — right
for a metric, wrong for a header: between crests (roughly a third of each
sweep) it would be indistinguishable from the dim body under it. So
`shimmer_spans_from` takes the resting colour, and `Thinking…` passes the
near-white `reasoning_shimmer_base()` (`#C8C8C8`). At rest it is bold near-white;
the wave brightens it to `#FFFFFF` rather than being the only thing making it
visible. The status line keeps codex's grey, byte-identical. This is the one
place in the feature that draws the eye — deliberately, because it is the one
place something is still happening.

`ui::preview_rows` sizes the same block from the same state (the strip's
`debug_assert` holds it to that), so the box and cursor stay seated.

The reasoning branch sits **after** the agent/tool branches and **before** the
streaming-reply branch. In practice they never collide — reasoning precedes
both the text and the tool calls of its round — but a tool that is genuinely
executing wins, because that is what the user is waiting on.

The status line keeps its `· Thinking for 12s` clause: it is the *timer*, the
block is the *content*.

### The committed cell

`ThinkingEnd` closes the buffer and appends `HistoryItem::Reasoning`:

```rust
pub struct Reasoning {
    pub text: String,       // the whole streamed chain-of-thought
    pub secs: u64,          // how long the phase ran (boundary clock)
    pub tokens: usize,      // the estimate, snapped to the provider's count
    pub timestamp: String,
}
```

`ui::reasoning_lines` renders it as one unwrapped, **bullet-less**, dim line —
`summary_lines`' shape *and* its colour (`status_done_color()`), so the pair that
brackets a turn reads as a pair:

```
Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)
Done for 1m 12s · 2.8k tokens (64 cached)
```

`format_elapsed` humanizes the seconds (`5s`, `1m 5s`, `1h 2m`) and
`format_token_count` the tokens (`842`, `1.5k`, `1.2M`) — the same two helpers
the status line and the turn summary use, so every duration and count in the
TUI reads the same. A zero token count hides its clause; the `EXPAND_HINT` is
the same string a capped tool peek ends with.

**Ordering is invariant 4's business, not free.** The item is appended the
moment thinking ends — which is usually before the round's reply text exists,
but a model can reason *after* it has begun answering (the dummy's default turn
is exactly that shape). So the run of assistant text before the phase is
**flushed** (`App::flush_streaming_segment`, the `ToolStart` dance): without it
the cell splices itself into the paragraph that was streaming, and `history` —
where the thought was just appended — disagrees with scrollback, so a resize
repaint reorders them.

**The flush happens at `ThinkingStart`, as the live block goes up** (updated
2026-08-08) — not at the phase's end. The block is a cell like a tool's, and a
cell needs the spacer above it *while it is live*, not once it settles: the
`● Thinking…` header used to butt straight against the paragraph that was
still streaming (the reported bug) and then jolt down a row when the phase
collapsed and the flush finally ran. Flushing at the start also releases the
paragraph's **last line** — `StreamRender::commit` withholds a still-growing
line, `finish` doesn't — so the text the model just wrote is whole on screen
while it thinks about what to do next. The gate is the display: a hidden
phase (`show_thinking()` false) shows no block, so it must not split the
reply, and nothing about that path changes. A *shown* phase that ends up with
no text still leaves the split — but its header was on screen, so the break
is honest, and the real backend only ever opens a phase holding a reasoning
delta (`llm::backend` sends `ThinkingStart` from inside
`if !delta.reasoning.is_empty()`). `settle_reasoning` keeps its own flush:
it is a no-op after the start's, and it is still the one that runs for an
Esc/error mid-phase.

`ui::reasoning_full_lines` is the Ctrl+O expansion — the same dim line
**without** the `(ctrl+o to expand)` hint (this *is* the expansion) over the
whole text in the `⎿` gutter, the model's own paragraph breaks preserved.

**Ctrl+O mid-think** shows the phase too, whole rather than windowed
(`ui::reasoning_live_full_lines` — the same `● Thinking…` header over the same
gutter): the pager has
no row budget, so it tail-follows the frontier the way it already does a
streaming `bash` call's output. `TranscriptSig` gained a `reasoning_len` field
for that, so a thought streaming under the overlay invalidates the cached tail
exactly like a streaming tool does (`docs/tool-view-performance.md`). The
bullet renders at rest there — the transcript's signature is deliberately
clock-free.

### The token count

The status tally counts reasoning with the app-side tokenizer
(`App::push_thinking` → `tokenizer::count`), and that estimate is what the cell
is built with, because `ThinkingEnd` fires long before the round's usage frame.
When that frame arrives the app **snaps** the cell to the provider's real
number — exactly what `App::apply_usage` already does to the live tally
(`docs/prompt-caching.md`):

```json
"usage": {
  "completion_tokens": 74,
  "completion_tokens_details": { "reasoning_tokens": 23 }
}
```

`TokenUsage::reasoning` carries it (parsed in `llm::openai::parse_sse_usage`),
and `App::apply_usage` distributes it over the reasoning items **this round**
recorded (`App::round_reasoning`) — proportionally to their estimates, the last
one taking the remainder so the sum is exact. The common case is one phase per
round, which is just "set it to `reasoning_tokens`".

The row already in scrollback keeps the estimate it was committed with (nothing
can rewrite a scrollback row); every later render — the Ctrl+O transcript, a
resize repaint, a `/resume` — shows the provider's number.

A backend that reports no `reasoning_tokens` (the dummy, a provider that omits
the detail) simply keeps the estimate.

### Where the buffer is closed

| Event | What happens to an open buffer |
| --- | --- |
| `ThinkingEnd` | recorded + committed — the normal path |
| Esc interrupt, **kept** | recorded + committed, ahead of the partial reply (codex never retracts what streamed) |
| Esc interrupt, **undone** | dropped with the rest of the submission |
| backend `Error` | recorded + committed, ahead of the partial |
| `/clear` | dropped |
| a `/compact` turn | never opened — the summarization turn is invisible by design (`docs/compact.md`) |

`Session::settle_reasoning` is the one boundary helper: it closes the buffer
with the thinking clock's elapsed, commits the cell (view-gated like every
commit), and clears the clock. The three settle sites call it before they
record anything else, which is what keeps stream order right.

### Persistence and context

- **Rollout** — `session::ItemRecord::Reasoning`, so `/resume` restores the
  cells and their text. Old rollouts (no such record) parse unchanged.
- **Derived context** — a reasoning item is **skipped** by
  `context::context_messages`. Chat Completions has no place to put a previous
  round's raw chain-of-thought, and re-sending it would burn context for
  nothing. Ctrl+D therefore shows no trace of it, which is the truth about what
  the model receives.
- **Checkpoints / backtrack** — nothing special: the item is an ordinary
  append, so the conversation-length keys and the Esc-Esc truncation work as
  they always did.

## Subagents think out loud too

Everything above describes the *main* session. A **subagent** now runs the
same shape over its own state (`docs/agent-view-streaming.md`): `AgentRun`
carries the phase buffer, its round's snap targets, and the boundary-injected
elapsed; `Session::settle_agent_reasoning` is `settle_reasoning`'s sibling,
recording `HistoryItem::Reasoning` on that agent's transcript and committing
the collapsed cell when its session view is on screen. The strip's live block,
the `Thinking for Ns` clause, the Ctrl+O expansion, the `reasoning_tokens`
snap, and the `/settings` **Hide thinking** gate are all the same code paths.
The one piece not shared is `distribute`'s caller: the snap is the pure
`app::reasoning::snap_reasoning_tokens`, which `App` and `AgentRun` both call
rather than each keeping a copy.

## The flag

`tui::config::show_thinking()` reads `ALTER_ZERO_SHOW_THINKING` with the
`ALTER_ZERO_PERMISSIONS` grammar — **on unless** the value is `0`, `false`,
`no`, or `off` — and `Session` stores it once at bootstrap. The gate is
entirely at the boundary: when it is off the `Thinking*` arms never open a
buffer, so `App` has nothing to preview and nothing to record, and every pure
renderer below simply never sees a reasoning item. There is no second code path
to keep in step.

Note it does **not** change what is *asked of* the model — the Ctrl+T
thinking mode (`docs/reasoning.md`) still rides the request. `ALTER_ZERO_SHOW_THINKING=0`
hides thinking; `Ctrl+T` to `off` stops it happening.

## Testing

Pure and unit-tested throughout: the usage parse (`llm::openai`), the buffer
lifecycle and the usage snap (`app::tests::reasoning`), the renderers and the
strip geometry (`ui::tests::reasoning`), the record round-trip (`session`), and
the context skip (`context`).

The boundary wiring is covered by `scripts/smoke.sh` **Phase 66**, which drives
the real binary in tmux: the live block appears while the dummy "thinks", it is
replaced by the collapsed cell, the chain-of-thought is **not** in scrollback,
Ctrl+O expands it back — and the same turn under `ALTER_ZERO_SHOW_THINKING=0`
shows neither while still settling to its `Done for Ns` summary.

Two `#[ignore]`d live tests in `tests/live_openrouter.rs` pin the provider end
(run with a real key):

- `live_usage_reports_the_reasoning_token_count` — a real stream's final frame
  really does carry `completion_tokens_details.reasoning_tokens`;
- `live_thought_cell_snaps_to_the_providers_reasoning_tokens` — replays a real
  stream's events into a real `App` and asserts the recorded cell traded its
  tokenizer estimate for the provider's count (observed: a 164-token estimate
  snapping to the reported 89 — the two accountings genuinely differ, which is
  the whole reason to prefer the provider's).

## Known limitations (v1)

- **Subagents** don't show their reasoning. `agents::AgentRun::apply` still
  counts a subagent's `ThinkingChunk` into its token tally and nothing more —
  the roster's tree rows have no clock of their own to time a phase with, so
  the `Thought for …` cell has nothing to say. Their reasoning is dropped,
  as it always was.
- One collapsed cell per **thinking phase**, not per turn: a model that
  interleaves reasoning and text across a round commits one cell per burst.
  That is faithful to what happened, but a chatty interleaver produces several
  short cells.
- The scrollback row keeps the tokenizer estimate when the provider's count
  arrives later (see above). Only the row already printed; every re-render is
  corrected.

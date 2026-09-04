# Live-streaming `bash` output — the running tail + collapsed peek

Claude-Code renders a running shell command as a **live tail**: the coloured
`● Bash(cmd)` header, the most-recent output lines under the `⎿` gutter, and a
`+N lines (Ns)` footer that ticks as more arrives — and, when the command
finishes, collapses to the first few lines plus `… +N lines (ctrl+o to expand)`.
Before this change the backend `bash` tool only showed `⎿ Running…` while it ran
and a **single** peek line once done. This document covers making it stream.

```
Running (live, in the strip above the box):        Finished (committed to scrollback):

● Bash(ping -c 10 google.com)                       ● Bash(ping -c 10 google.com)
  ⎿  64 bytes from … icmp_seq=6 … time=68.3 ms         ⎿  PING google.com (142.250.205.238) …
     64 bytes from … icmp_seq=7 … time=247 ms             64 bytes from … icmp_seq=1 … time=32.1 ms
     64 bytes from … icmp_seq=8 … time=332 ms             64 bytes from … icmp_seq=2 … time=71.1 ms
     64 bytes from … icmp_seq=9 … time=144 ms             … +11 lines (ctrl+o to expand)
     +5 lines (9s)

● Bash(ping -c 10 facebook.com)                     (a parallel batch's not-yet-run
  ⎿  Waiting…                                        siblings stay `⎿ Waiting…` until
                                                      each one's turn — docs/parallel-tools.md)
```

The two states are deliberately asymmetric — while **running** you want the
**tail** (what just happened); once **finished** you want the **head** with an
expand hint. The tail shows the last `TOOL_PEEK_ROWS` (4) display rows; the
head **folds** at `TOOL_FOLD_ROWS` (3) rows over the hint — Claude Code's
fold, at most the same four rows hint included — so the cell never grows when
it settles. (It can settle *shorter*: the finished head is the output's first
**block**, so a blank line ends it — `docs/long-lines.md`. The running tail
keeps its blanks, being what the command just printed, and shows the output
as printed where the settled cell reshapes a JSON line for display.)

## The protocol — `StreamEvent::ToolOutput`

A new event carries a chunk of the **currently-running** tool's live output,
between its `ToolStart` and `ToolEnd`:

```rust
/// A chunk of the running tool's output, streamed live as it is produced
/// (one or more complete lines, stdout+stderr merged in arrival order). The
/// loop appends it to the front running call so the live cell tails it; the
/// authoritative full output still arrives in `ToolEnd`. Only the real `bash`
/// executor emits it today.
ToolOutput(String),
```

It targets the **front** of `App::tool_queue` (the only `Running` call —
execution is sequential even for a parallel batch, so the running call is always
the front; see `docs/parallel-tools.md`). A backend that never streams simply
omits it — `ToolStart`/`ToolEnd` still work unchanged.

## App state — `push_tool_output`

`App::push_tool_output(chunk)` appends `chunk` to the front call's `output`
**only when it is `Running`** (a no-op otherwise). It does **not** count tokens
— the tally is charged once, from the authoritative `ToolEnd` output, in
`end_tool` (which overwrites `output` with the final framed text). So the live
tail and the final cell never double-count, and the final cell is always exact
even if a chunk was dropped.

## The executor — `run_bash` streams complete lines

`ToolExecutor::execute` gains an `on_output: &mut dyn FnMut(&str)` sink;
`run_agent` passes one that sends `ToolOutput`. `run_bash` still drains both
pipes on reader threads (so a chatty command can't deadlock a full pipe), but
now **forwards complete lines as they arrive**: each reader sends raw byte
chunks over an `mpsc` channel; the main poll loop merges them into the capped
`combined` buffer (arrival order) and forwards every newly-**completed** line
(`combined[forwarded .. last '\n']`) through the sink, holding a partial trailing
line until its newline lands (or EOF). Forwarding whole lines from the merged
buffer keeps a multi-byte UTF-8 char from ever splitting across a chunk boundary
(a `'\n'` is never inside a char), and the cap bounds the live tail's memory just
like the final output. `read`/`write`/`edit` ignore the sink (nothing to stream).

The final `ToolOutcome.output` is unchanged — codex's `Exit code: N` frame over
the (capped) body — so the model still gets the framed result and `end_tool`
overwrites the display `output` with it.

## Rendering (`ui/tool.rs`)

A **command-style** tool (a non-shell backend tool that is not a `read`/`write`/
`edit` file cell — in practice `bash`) now renders its output as a multi-line
`⎿` block, like the `!` shell cell:

- **Finished** (`tool_lines`): the head of the output's display lines
  (`exec_display_lines` — the frame stripped, a JSON line reshaped, trailing
  blank lines dropped) — its first **block** (leading blank lines skipped, the
  first blank line after them closing the peek: a three-row fold cannot
  afford a row that says nothing, and hopping the gap would present two
  stretches of output as one), **folded** at `TOOL_FOLD_ROWS` display
  **rows** — then `… +N lines (ctrl+o to expand)`, via the shared
  `result_peek_block`; an output of exactly `TOOL_FOLD_ROWS + 1` rows shows
  whole, since a hint hiding one row costs the row it hides. Each line
  **word-wraps, spaces preserved** (`wrap_output`, the same wrapper the
  Ctrl+O view uses — a prose error like `sudo`'s breaks at words, never
  mid-"askpass"; `ls -l` columns that fit stay byte-exact; a token wider than
  any row fills the row it is on, Claude Code's wrap) rather than clipping at
  the terminal edge, so a long line's tail no longer disappears — and the fold
  is what bounds one pathological line (a minified bundle, a 2 KB `curl`
  body): it shows three rows of it and the hint says the rest follows
  (`docs/long-lines.md`). The `+N lines` hint
  counts the **display rows** it didn't show — what pressing Ctrl+O actually
  adds, counted with the same wrapper the expansion uses — instead of source
  lines, which is how 1.8 KB of hidden JSON used to report itself as
  `+1 lines`. (The legacy diff-fallback peek wraps **verbatim** instead —
  code, never reflowed at spaces — with each wrapped row coloured by its
  *source* line's `+`/`-` marker, so a continuation row keeps its tint.)
- **Running** (`running_command_lines`, drawn only in the live strip's preview
  where the boundary-supplied `elapsed` is available): the header, the **last**
  `TOOL_PEEK_ROWS` display **rows** of output, then `+{hidden} lines
  ({secs}s)` when any source lines are fully hidden above (else just the tail —
  the status line carries the timer). No output yet → the existing
  `⎿ Running…` row. Long lines **word-wrap** the same way (`wrap_output`)
  instead of clipping at the terminal edge; the window is counted in wrapped
  rows, so a single long line tail-follows its own newest rows without
  growing the strip past its budget, and the newest-first walk wraps only
  what the window can show per animation frame. The footer counts *display
  rows* above the window — the lines wholly above it plus the rows the oldest
  shown line lost off its own top — so a 2 KB line that scrolled past reads
  `+18 lines`, not `+1 lines` (`docs/long-lines.md`; `WrapMode::rows` counts
  without building a row, so the whole retained buffer can be measured every
  animation frame). (The strip stays sized by the same
  `preview_tool_lines` walk `render_live` paints from, so the count and the
  paint agree by construction.)
- **Ctrl+O** (`tool_full_lines`): the whole output, uncapped — `wrap_output`
  too, so the expanded view and the inline peek render identically.

### The `Exit code: N` frame, reframed for display

`tool.output` stays framed (`Exit code: 0\n…`) because `context::context_messages`
replays it verbatim as the model-facing `tool` result on later turns. For the
**display** (`command_display_output`): on **success** the leading
`Exit code: 0` line is dropped so the cell reads like the mock — the real
command output, not the frame; on **failure** it is rewritten to an
`Error: Exit code N` (or `Error: killed by signal`) line kept above the body,
so a red cell says *why* it failed even when the command printed nothing (a
bare `exit 3` used to show `(no output)`). Reframing only fires when the
frame line is actually present, so non-`bash` tools, `!` shell cells (whose
output is raw, never framed), and old rollouts are untouched.

## Live in the Ctrl+O overlay (ours streams; Claude Code doesn't)

Claude Code's transcript pager shows a tool's output only once the tool
**finishes**; while it runs the cell is a static spinner. Ours does better — the
**Ctrl+O overlay updates live** as `bash` streams, so you can pop it open and
watch a long command's output flow in the full-screen view (tail-followed to the
frontier).

Two pieces make this work, and both already existed except one:

- **The loop keeps draining reply events while the overlay is up** (invariant 4)
  and re-arms a ~30 fps draw whenever a turn is active, so `ToolOutput` deltas
  reach `App::push_tool_output` and the overlay repaints — no overlay-specific
  wiring needed.
- **The `TranscriptCache` signature** (`ui::TranscriptSig`) must notice the
  running call's output growing, or the cached transcript never rebuilds and the
  overlay goes static. The signature's tool-queue term is
  `(queue length, front status, **front output length**)` — the third field is
  the fix: a lone running call's length and status don't change while it streams,
  but its `output.len()` does, so each delta invalidates the cache and the
  overlay re-renders (and, being **tail-following** by default —
  `App::tool_follow` / `settle_tool_scroll`, engaged on open — scrolls the new
  output into view). The unit test
  `transcript_cache_rebuilds_as_a_running_bash_streams_output` locks it; smoke
  Phase 40 proves it end-to-end (a running `Bash(ping)` cell streams into the
  overlay while its siblings still show `⎿ Waiting…`).

The overlay renders the running call's output through `tool_full_lines` (the
*whole* streamed-so-far output, uncapped), the same walk that shows a finished
tool — so a running cell and a finished one read identically, just growing.

## Demo (the dummy backend)

`stream::turn_events` interleaves a few `ToolOutput` chunks between each `Bash`
call's `ToolStart` and `ToolEnd`, so `cargo run` (and `scripts/smoke.sh`) show
the live tail without an API key. The committed scrollback is unchanged — the
final cell still comes from `ToolEnd`.

## Verifying against a real model

`examples/tool_smoke.rs` prints every event, `ToolOutput` included:

```bash
OPENROUTER_API_KEY=sk-... ALTER_ZERO_CA_FILE=/root/.ccr/ca-bundle.crt \
  cargo run --example tool_smoke -- openai/gpt-4o-mini \
  "Use bash to run: ping -c 10 google.com"
```

## A short terminal freezes the cell instead of trimming it

The live tail is the region's elastic content, so a terminal without room for
the whole cell used to drop its rows — the `+N lines (Ns)` footer first, then
the output rows, then the header — into no buffer at all. The strip
bottom-anchors now and commits the rows it cannot paint into the terminal's own
scrollback, frozen: the cell **scrolls**, keeping its newest rows and its
`+N lines` footer on screen while its head stays readable by scrolling up.
See `docs/strip-flow.md`.

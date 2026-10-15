# Streaming a `write` while the model generates it

A `write` call used to be **invisible until it ran**. A model generating a
300-line file spends tens of seconds streaming the call's `content` argument,
and all the screen showed was the status line's token tally ticking: the
fragments (`StreamEvent::ToolCallDelta`) were counted and thrown away, the
same treatment reasoning used to get (`docs/thinking-stream.md`). The cell
appeared only after the round ended, the permission prompt was answered and
the file was on disk.

Now the cell streams **while the model writes it**:

```
Generating (live, in the strip):            Generated (live, the round still streaming):

● Write(fizzbuzz.py)                        ● Write(fizzbuzz.py)
  ⎿  Writing 14 lines…                        ⎿  Writing 16 lines…
       5 def fizzbuzz(n: int) -> None:             1 #!/usr/bin/env python3
       6     for i in range(1, n + 1):             2 """Print FizzBuzz for the first n numbers."""
       7         if i % 3 == 0:                    3
       8             print("Fizz")                 …
       …                                          10             print("Buzz")
      14             print(i                       … +6 lines (ctrl+o to expand)
```

Two shapes, the running/finished asymmetry `docs/tool-streaming.md` gave the
`bash` cell:

- **While the `content` string is still open** the cell shows its **tail** —
  the newest lines, at most `FILE_PEEK_LINES` (10) display rows, the last of
  them the line still being typed — so the file *streams down* the window the
  way a terminal scrolls, its line numbers counting up.
- **Once the string closes** the cell shows its **head** — the first ten rows
  over `… +N lines (ctrl+o to expand)` — which is exactly the shape the
  committed `Wrote N lines to …` cell takes, so nothing jumps when the call
  runs and resolves.

The live cell is never committed. When the round ends the call goes the way
every call goes: the batch announcement replaces it with the ordinary
`⎿ Waiting…` cell, the permission prompt (manual mode) shows the whole
content, and the resolved cell commits with the executor's own report — the
numbered head for a new file, the diff for an overwrite.

## The wire: which call does a fragment belong to?

The fragments arrived as one opaque `String` — the `name` and `arguments`
pieces of every call in the round concatenated, which is all a token tally
needs and nothing a renderer can use. A round can carry several calls, and
on some wires their fragments interleave. So the event carries the call now:

```rust
pub struct ToolCallFragment {
    /// The call this piece belongs to — a key unique within the round.
    pub index: usize,
    /// The call's wire tool name, on the piece that carries it.
    pub name: Option<String>,
    /// The next piece of the call's JSON arguments, verbatim.
    pub arguments: String,
}

StreamEvent::ToolCallDelta(ToolCallFragment)
```

`index` is the wire's own key, never a position the UI may count on:

| Wire | Key | Name arrives |
| --- | --- | --- |
| Chat Completions | the `ToolCallAccumulator` slot the piece folded into (so Gemini's reused index 0 with distinct ids stays two calls) | on the call's first `tool_calls` delta |
| Responses | the `output_index` of the `function_call` item | on `response.output_item.added`, ahead of its argument deltas |
| Messages (Anthropic) | the `tool_use` content-block index | on `content_block_start` |
| Ollama | the call's position in the frame | whole, with the arguments as one piece |

The Responses wire used to surface a call's name only on
`response.output_item.done`, after every argument delta. It reads
`output_item.added` now, and the done frame emits only what no earlier frame
did — the name of a call that never announced itself, the arguments of one
that arrived whole — so the tally never counts a piece twice.

The tally is unchanged: a fragment counts its name and its arguments, as the
concatenated string did.

## Decoding a JSON string that is still arriving

`partial_json::StringFields` decodes chosen **top-level string fields** of a
JSON object fed a fragment at a time — here `path` and `content`. It is a
character state machine (`{`, key string, `:`, value, `,` …) whose string
decoder keeps its escape state across fragments: a piece may end on a lone
`\`, inside `é`, or between the two halves of a surrogate pair, and the
next piece picks up where it stopped. Every field it reads is **append-only**
— the text decoded so far, whether the closing quote has been read, and how
many `\n` it holds (counted as it decodes, so `Writing N lines…` is O(1)
however long the file grows). Values under other keys are skipped, nested
objects and arrays included, without storing them.

It is lenient where a display can afford to be: a raw control character
inside a string is kept as text, and anything that is not an object stops
decoding quietly — the executor's strict `parse_args` still decides whether
the call can run.

## App state: the round's draft calls

`App::drafts` (`app::DraftCalls`) holds one `DraftCall` per call the round has
begun streaming: its key, its wire name once known, its decoded fields and a
process-unique id (the renderer's memo key). `App::push_tool_call_fragment`
folds a fragment in, counts its tokens, and reports whether a **`write` draft
just became visible** — the boundary's cue to finalise the run of assistant
text before it (invariant 4's flush-before-you-interleave, the `ThinkingStart`
dance), so the cell slots after that text in scrollback and history alike.

A draft lives only as long as the round's generation. Everything that ends it
clears the list: the round's announcement (`RoundCalls`), a batch, a lone
`ToolStart`, an agent group, a task call, and every turn boundary — start, end,
error, Esc, `/clear`. An interrupted draft leaves no record: the round was
cancelled, so the model never made the call, and the wire history agrees.

Only `write` drafts render. Every call is tracked — a fragment with no name
still has to be attributed to its call — but an `edit` or a `bash` command is
short, and its own cell follows within a moment.

The subagent fold keeps the same list on `AgentRun::drafts`, so a subagent's
session view streams its writes exactly as the main view does
(`docs/agent-view-streaming.md`); `AgentRun::flushes_segment_on` answers the
boundary's "does this event finalise the text before it?" for the fragment
that opens one.

## Rendering: incremental highlighting, memoized

The strip is rebuilt every 32 ms animation frame and reaches the preview walk
several times per frame (`live_height`, the budget, the layout, the paint), so
re-highlighting ten lines per call was not free, and starting the highlighter
at the window's first line coloured a line inside a docstring or block comment
as code until the window scrolled past its opener.

`ui::draft` keeps a per-thread memo per draft (the `ui::footer` queued-rows
pattern): a `Highlighter` advanced over every **complete** line once, the
highlighted segments of the first ten (the head) and the last ten (the tail),
and the built rows keyed on the content's length, completion, the width and
the theme. The line still being typed is highlighted from a clone of the
carried state, so it is coloured as the file's own lexer state says. A new
fragment costs the lines it completed plus the one in progress; a frame that
brings no fragment costs a hash.

The language comes from the path's extension; a path that lands after the
content began (key order is the model's) restarts the memo once under the
right grammar.

## The surfaces

| Surface | Shows |
| --- | --- |
| **Strip** | the live cell: `● Write({path})` (blinking, like any running call), the dim `⎿ Writing N lines…` corner, then the tail or the head |
| **Ctrl+O** | the whole content so far, numbered and coloured, under the same header — the transcript's live tail, beside the running tool and the thinking block |
| **Agent session view** | the viewed agent's own drafts, in the same branch order |

The header names the path once its string has closed, so it never reflows
character by character; before that it is a bare `● Write`.

## What did not change

- The resolved cell, the permission prompt, the rollout and the derived
  context — a draft is display state, never history.
- `edit` and every other tool: their fragments are tracked, counted and
  dropped as before.
- A provider that delivers a call whole (Ollama) still shows the draft, for
  as long as the rest of its round takes to stream.

## Offline

The dummy's file-change demo (any prompt saying "write", "edit" or "diff" —
`docs/dummy-backend.md`) streams its `write` call's real arguments a token at
a time (`stream::tokens`, a third of a word's pause per piece), so the cell
is on screen with no provider: `write a fizzbuzz` shows the tail scroll past
ten lines, the head while the edit and the run are generated, then the
committed cell.

## Testing

- `partial_json` (unit): every split point of a set of objects decodes to
  what `serde_json` reads; a value fed a character at a time only ever grows;
  escapes, `\u` digits and surrogate pairs split across pieces; a lone
  surrogate, an escaped quote after one, a quote cutting a `\u` short; values
  under other keys skipped, nested and scalar alike; `Field::lines` matching
  `str::lines`.
- `llm` (unit): each wire's pieces carry their call's key — interleaved
  parallel calls and Gemini's reused index on Chat Completions, the name at
  `output_item.added` and nothing doubled at `output_item.done` on Responses,
  the block index on Messages, the whole call on Ollama.
- `app` (unit): a piece counts its name and arguments into the tally and ends
  an outage; the piece naming a `write` opens its draft and reports it once;
  a non-`write` is tracked but never shown; the path shows only once closed;
  every round and turn boundary clears the drafts; an interrupt with only a
  draft streamed still undoes the submission. `agents`: the same on
  `AgentRun`, with `flushes_segment_on` pinned by
  `every_flush_point_is_declared`.
- `ui` (unit): the corner's count; the tail's last ten rows ending on the line
  being typed; the head and its hint once the content closes; the row budget
  counting display rows; a huge line clipped and marked; the window's colours
  matching a whole-file highlight (a docstring opened above it); the memo
  highlighting each complete line once; `preview_rows` agreeing with the
  painted rows; two drafts, two cells; the Ctrl+O expansion and the cache
  following it; the agent view; the flow key re-signing at the switch to the
  head but not per line.
- `stream` (unit): the demo's write streams the arguments it runs with, in at
  least forty pieces, decoding to the whole file; its generation plays back
  in one to five seconds.
- `scripts/smoke.sh` **Phase 138** drives the real binary twice — the offline
  demo, and the real backend against a stub provider that streams a 40-line
  write and holds the round open after the file closes — recording the pane
  every 15 ms and requiring tail frames, head frames before the call ran, the
  committed `Wrote 40 lines` cell, no `Writing` row left in scrollback, and
  the file on disk whole.

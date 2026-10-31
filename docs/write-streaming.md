# Streaming a `Write` while the model writes it

A `write` call's `content` is usually the largest thing a model generates in a
turn: a three-hundred-line file streams for tens of seconds. Until this
change none of it reached the screen while it streamed. The backend counted
the tool-call fragments for the status line's token tally
(`docs/status-indicator.md`), and nothing else. The `● Write(…)` cell
appeared only once the whole round had finished, so for half a minute the
user watched `↓ 4.1k tokens` climb with no sign of what was being written.

The cell now goes up the moment the model names the call, and the file
appears in it as the model writes it:

```
While the content streams:                    Once it has all arrived:

● Write(fizzbuzz.py)                          ● Write(fizzbuzz.py)
  ⎿  Writing 17 lines…                          ⎿  Writing 17 lines…
      8             print("Fizz")                   1 #!/usr/bin/env python3
      9         elif i % 5 == 0:                    2 """Print FizzBuzz for the first n numbers."""
     10             print("Buzz")                   3
     11         else:                               4
     12             print(i)                        5 def fizzbuzz(n: int) -> None:
     13                                             6     for i in range(1, n + 1):
     14                                             7         if i % 3 == 0:
     15 if __name__ == "__main__":                  8             print("Fizz")
     16     fizzbuzz(15)                            9         elif i % 5 == 0:
     17 # the line still arriving                  10             print("Buzz")
                                                    … +7 lines (ctrl+o to expand)
```

While the content streams, the cell shows the **last** ten rows, so the
newest line is always the bottom row and the window moves down the file as it
grows. The corner row counts the lines as they arrive. When the content's
closing quote arrives, the cell shows the **first** ten rows instead, with the
`… +N lines (ctrl+o to expand)` hint. That is the same peek the committed cell
shows, so the cell does not change shape again when the call resolves. Only the
corner row (`Waiting…` while queued, then `Wrote 17 lines to fizzbuzz.py`) and
the bullet's colour move. Ctrl+O shows the whole file, still streaming, like
everything else live.

## The wire: which call a fragment belongs to

The four wire formats all stream a call in pieces, and each already knew which
call a piece belonged to. That identity was dropped at the seam: `Delta` carried
one opaque `tool_call: String` per frame, the concatenated name and argument
text of every call in it, and `StreamEvent::ToolCallDelta(String)` carried that
string to a loop that could only count it.

The piece is a type now, `stream::ToolCallFragment { index, name, arguments }`,
on both `Delta::tool_calls` and `StreamEvent::ToolCallDelta`. `index` is the
wire's own slot for the call:

| Wire | Where the slot comes from | Where the name comes from |
|---|---|---|
| Chat Completions | the entry `ToolCallAccumulator::push` resolved the delta into (it already split same-index calls with different ids) | the first delta's `function.name` |
| Anthropic Messages | the `tool_use` content block's index | `content_block_start` |
| Responses | the item's `output_index` — or, on a frame without one, the call still open (a new call when the frame opens one) | `response.output_item.added`, which the drain now reads; the name used to arrive only with the finished item |
| Ollama | the call's position in the round | the whole call, which arrives in one frame |

The slot is an identity, not a position: an Anthropic text block takes an index
too. The token tally is unchanged, because `ToolCallFragment::tally_text` is the
name and the argument text, which is what the old string carried. The Responses
drain keeps the books in `responses::ToolStreams`: it counts a name once, at
`output_item.added`, and `output_item.done` adds only what no earlier frame
sent — the name of a call whose opening it never saw, and the arguments of a
call none of whose arguments streamed as deltas, which is how a server that
hands over a call's arguments only with its finished item still gets its file
previewed, and counted once. `output_index` is the server's to send. A stream
produces its output items one at a time, so a frame without one belongs to the
call still open, or starts a new call when it opens one or when every earlier
call has finished — never folded into the first call's preview, which is where
a missing index used to put it.

## Reading arguments that have not finished arriving

`app::ArgsScanner` is an incremental decoder for one call's arguments object. It
is a character state machine that takes each piece as it comes (`feed`) and
keeps every **top-level string field** decoded as far as it has arrived:
`{key, value, closed}`. Nested objects, arrays and scalars are skipped; escapes
are resolved, including a `\uXXXX` split across two pieces and a surrogate pair
split across two escapes. A lone surrogate decodes as U+FFFD. Two properties
matter downstream:

- **A value only ever grows.** Every prefix of the stream decodes to a prefix of
  the whole value, whatever the split. The render cache below relies on it.
- **It never needs the whole text again.** A 100 KB file costs one pass over its
  characters over the whole stream, where re-parsing the accumulated arguments
  per piece would cost quadratic time.

The first field of a duplicated key is the one read. Text that is not an object
yields no fields and nothing is shown, since the preview is cosmetic and the call
itself is still parsed by `serde_json` when the round ends.

`serde_json` is also the decoder's oracle in the tests. Each sample — every
escape, a surrogate pair, literal non-ASCII, a nested `content` under another
key, scalars, whitespace between the tokens — is fed split in two at **every**
character boundary, and must decode to exactly the fields serde reads in the
whole text, each closed; fed a character at a time, every field must only ever
grow toward serde's value. A decoder that agreed only with itself would pass a
test that checked it against its own whole-text answer.

## The round's drafts

`app::ToolDrafts` holds the calls of the round now being generated, one
`ToolDraft` per wire slot in the order each opened. `App` keeps one for the main
session and every `AgentRun` keeps its own. Only a `write` is **previewed**
(`ToolDraft::is_previewed`). Every other call is tracked so its pieces land in the
right place, and nothing of it is shown. A `Bash(…)` header that flashed for the
few hundred milliseconds its command takes to generate would say nothing the
batch does not say a moment later.

A draft's lifecycle is the round's:

1. **`push`**: a piece folds into its slot's draft, opening one if the slot is
   new. It answers whether the piece **opened a previewed call**, which is when
   the boundary flushes the streaming text segment. This is the first rule of
   `tui/stream.rs`, *flush before you interleave*, as `ThinkingStart` applies it.
   The paragraph that introduced the file commits to scrollback with its
   withheld last line, so the cell sits under it and not beside it.
   `opens_preview` asks the same question without folding, for the agent view,
   whose flush runs before its fold.
2. **`seal`**: the round's calls were announced (`RoundCalls`, `AgentBatch`, a
   `TaskCall`), so nothing more will stream. Every draft is finished, and the next
   piece opens a new round instead of continuing a slot the last round used.
3. **`adopt`**: the batch announcement (`ToolBatch`, or a lone `ToolStart` that
   pushed a fresh call) takes the previews over. The queued `Write` cells are
   paired with the previewed drafts **in order**, but only when the two counts
   agree. A count that disagrees pairs nothing rather than guessing, the rule
   `start_tool_batch` already applies to the round's call ids. Adopting
   replaces whatever an earlier adoption left, so a stale draft is never
   paired with a later batch's cell. A lone `ToolStart` adopts only onto an
   empty queue. Pushed beside calls still live, pairing by order could hand a
   cell another call's file, so the drafts are cleared and nothing previews.
4. **`resolve`**: a queued call left the queue (`App::resolve_front_tool`, the
   subagent fold's resolutions). A `Write` gives up the front adopted draft. The
   adopted drafts are front-first in queue order, so the k-th queued `Write` is
   always paired with the k-th adopted draft.
5. **`clear`**: every turn edge drops it all (`begin_stream`, `begin_shell`,
   `finish_stream`, `fail_stream`, an interrupt, a blocked prompt, the end of a
   `/compact` turn, `/clear`). A half-generated call is never recorded. Its
   JSON is incomplete and the model never finished asking for it, so there is
   nothing a later request could replay.

One rule outside the drafts moved with them. The Esc interrupt **undoes** a
turn that showed nothing, putting the message back in the composer
(`docs/interrupt.md`), and a file that had begun streaming into its cell was
something. So an Esc after a previewed `write` appeared interrupts the turn
instead, keeping the message and committing the notice. A call that previews
nothing, such as a `bash` command being generated, still leaves the undo in
place.

`revision` moves on every change. The Ctrl+O transcript signs on it.

## The cell

`ui::file_cell`'s draft renderer builds the cell from the draft alone. Its
geometry is the committed cell's, so the two cannot drift:

- **The header** is the ordinary `● Write({path})` row (`tool_header_lines`,
  so the path display rule and the file link come with it). The path shows once
  its string has closed, since a header spelling `/home/us` for one frame would
  read as a bug. While generating, the bullet blinks in the running grey
  (`docs/tool-pulse.md`). Once queued it wears the queue's colours.
- **The corner row** says where the call stands: `Writing {n} lines…` while it
  is generated, the line count growing as the lines arrive (`Writing…` before the
  first), then `Waiting…`/`Running…` once queued.
- **The body** is the numbered, syntax-highlighted gutter of the committed
  `Wrote …` cell, built by the same `numbered_row_lines`. It uses the same
  `{n:>W}` gutter and the same one-column indent past the corner. The
  window is the committed peek's `FILE_PEEK_LINES` (10) **display rows**:
  - **tail**, while the content streams: the newest ten rows, the unfinished
    last line at the bottom. Rows, not lines: a long line still arriving
    shows the text that just arrived, and the top row may be the end of a
    line whose start has scrolled off, so the window never grows past ten
    rows and cuts no line (`docs/long-lines.md` §7);
  - **head**, once it has all arrived: the committed peek — lines from the
    top while the ten rows last, each shown whole when the rows left hold it
    and the one that needs more cut on the tenth with a `…`
    (`docs/long-lines.md` §8) — over the `… +N lines (ctrl+o to expand)`
    hint, where N counts the lines the peek never started;
  - **whole**, in Ctrl+O: every line.

The window flips from tail to head on the content's closing quote, or when the
round seals without one. It does not wait for the call to be announced. A
`content` streamed before its `path` shows its head while the path is still
arriving.

Queued, a `Write` with an adopted draft shows the head under `Waiting…` instead
of the bare `⎿ Waiting…` row. This applies in the live strip only. The
permission prompt's context cells still use the one-row cell
(`docs/permissions.md`), because the prompt already shows the whole file.

**The highlight cache.** The tail is syntax-highlighted correctly, which means
the highlighter must have seen every line above it: a docstring opened on line 3
colours line 40. Re-highlighting the whole file costs tens of microseconds a
line, and the strip is built several times a frame, so a few hundred lines
would spend whole frames in syntect. `highlight::LineCache` highlights a growing
text incrementally instead: each complete line once, with the highlighter's state
carried after the last of them, and the unfinished line on a *clone* of that
state, the same peek `StreamRender` uses for a code block's withheld line. It is
cached by text length, so the repeated builds within one frame are free. The
draft owns it behind a `RefCell` that the render path fills, the textarea's wrap
cache pattern, so the cache lives exactly as long as the draft. The language or
code theme changing, or the path arriving after the content, rebuilds it once.

**What the cache keeps.** Every line is highlighted, but the strip's cell only
ever *shows* its first or newest lines: the head and tail walks take at most
`FILE_PEEK_LINES` source lines from their end, since every line is at least one
row, and each walk stops once its budget is full rather than reading a line it
cannot show. So the draft keeps two caches. The **peek** cache
(`LineCache::windowed(FILE_PEEK_LINES)`, through `ToolDraft::highlighted_peek`)
holds the segments of only the first and newest ten lines, plus the unfinished
one. It still highlights every line in order, because the grammar's state has to
pass through the middle, but lets the middle's segments go. The **whole** cache
(`ToolDraft::highlighted_whole`) keeps every line, and is filled only when a view
that shows every line asks: the Ctrl+O transcript or `/export`. It goes with the
draft at the turn's end.

This matters because a line's segments are about fifty times its text: a
`String` and a style per token. Keeping every line cost ~0.93 KB a line, about 19
times the file, for as long as the call was live, and glibc keeps what a process
has once used, so one 2,000-line write left the app ~2.3 MB bigger for the rest of
the session. The peek holds the decoded file plus about twenty lines: 147 KB for
the same write instead of 1.86 MB. `ui::tests::file_cell`'s
`the_cells_rows_are_the_ctrl_o_views_rows` pins the peek's rows to the whole
view's, styles included. `tests/write_preview_memory.rs` gates the resident growth
of a streaming 5,000-line write at four copies of the file. It measures one copy,
where the every-line cache's resident cost was about twenty-seven
(`docs/memory.md`).

## Where it sits in the strip

`preview_lines`/`preview_rows` share a branch: the previews come after a live
agent group or tool queue, and before a thinking block and the reply's frontier.
They are mutually exclusive with the first two in time, because a round's calls
stream before its batch exists, and the backend closes a thinking phase before
the first fragment. Several previewed `write`s stack blank-separated, like a batch.
The cells count as **live cells** (`strip_preview_is_live_cells`), so a strip
too tall for a small terminal flows its head into scrollback rather than
trimming it (`docs/strip-flow.md`). `strip_flow_key` signs on each preview's
path and whether it has finished, never on its rows. A streamed line therefore
re-signs nothing, while the flip from tail to head, which replaces every row,
does.

## The subagent session view

A subagent's `write` streams in its session view exactly as the main one's does
(`docs/agent-view-streaming.md`). `AgentRun` keeps its own `ToolDrafts` with the
same lifecycle, and `agent_view_preview_lines`/`agent_preview_rows` render the
previews through the same builder. The view's segment flush runs **before** the
fold, so it asks `AgentRun::will_flush_segment`, the fold's own answer, which now
includes a piece opening a previewed call — and `every_flush_point_is_declared`
asks it the same way of one of every event, that opening piece included. A
hand-kept list here is what dropped a `SubagentStop` continuation's tail once
(`docs/agent-view-streaming.md`).

## Offline

The dummy's file-change demo (a prompt mentioning *write*, *edit* or *diff*)
streams its `write` the way a live model does: the call's real JSON arguments,
cut into token-sized pieces (`stream::tokens`) at the reply's own pace
(`ALTER_ZERO_CHUNK_DELAY_MS`). `smoke.sh` Phase 138 drives it in tmux, and then
drives the **real** backend against a stub provider that streams a 30-line
`write` six characters at a time, sampling the cell as it moves. Past ten
lines the window's bottom row must keep moving down the file and line 1 must
never show. Once the content closes, the cell must show lines 1 to 10, and the
committed cell must keep them over `… +20 lines`. Phase 138b streams one long
paragraph line the same way and reads the file back: the streaming cell must
show more than three rows of it with no row ending in `…`, and the committed
`Write` and `Read` cells must both show its last word (`docs/long-lines.md`
§7). Phase 138c streams a stylesheet whose fourth line is thousands of
columns long and reads it back: no sample of the cell may pass ten body rows,
and the committed `Write` cell and the `Read` cell must each cut that line on
their tenth row with a `…` over `… +6 lines` (`docs/long-lines.md` §8).

## What it does not do

- **`edit`** streams no preview. Its `new_string` would preview the same way, but
  the cell an edit resolves into is a diff, and a growing replacement shown as a
  new file would look like a different kind of cell.
- **An overwrite** still resolves as the `Updated {path} (+A -D)` diff. The draft
  cannot know whether the file exists, so it previews the content as new, and
  the diff replaces it when the executor reports.
- **A call the round never finished** (an Esc, an error, a request cut off) is not
  recorded, as before. Its cell disappears with the turn's other live rows.

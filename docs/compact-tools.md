# The compact tool display

Claude Code folds the model's housekeeping — the reads, the searches, the
directory listings it runs between one thing it says and the next — into
one line that grows as the work goes (`⏺ Searched for 13 patterns, read 6
files`), and shows a cell per call only in verbose mode. Alter Zero's inline
cells were always the verbose form: every `● Bash(ls -la)`, every `● Read(…)`
with its ten-row peek. This page is the other form, and the `/settings`
**Verbose output** row that chooses between them (`true`, the default, is
exactly what the app did before the row existed; `ALTER_ZERO_VERBOSE=0` seeds
the compact form for a run).

With the row off, the strip shows **one summary** for a run of read-only
calls while the group is open, and scrollback keeps **one dim line** for it
once it has settled:

```
● Listing 1 directory… (ctrl+o to expand)
  ⎿  $ ls -la

● Reading 1 file, listing 1 directory… (ctrl+o to expand)
  ⎿  mathutil.py

● Searching for 1 pattern, reading 1 file, listing 1 directory… (ctrl+o to expand)
  ⎿  $ grep -rn 'def ' mathutil.py

Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to expand)
```

The bullet keeps **blinking after a call returns**: the next call may be
another read, and the group is still taking members. It stops — the summary
settles into its dim line — when something that is not a member happens: the
model writes a file, runs a command that is not read-only, answers, or the
turn ends. Nothing about the conversation changes underneath: the Ctrl+O
transcript shows every member's whole cell, the derived context replays every
call as it always did, and the rollout records every one, so a `/resume`
folds the same cells again. The fold is a **way of showing** the records,
derived from them and never recorded.

## Who joins

A call is a **member** ([`app::member_of`]) when it is

- a `read` — counted once per distinct path, so reading the same file twice
  is `read 1 file` — unless it read a picture (an image `read` commits its
  picture with its cell, something to look at, never folded); or
- a `bash` whose command *looks like* a listing, a read or a search
  ([`readonly::kind`], the same table that vouches for a read-only command in
  auto mode, `docs/permissions.md` *Read-only commands*): every segment of
  the pipeline names a listing program (`ls`, `tree`, `du`), a reading one
  (`cat`, `head`, `tail`, `wc`, `jq`, `sort`…), a searching one (`grep`,
  `rg`, `find`, `which`…) or a neutral one (`echo`, `cd`, `pwd`, `test`…),
  nothing is redirected into a file, nothing substituted, and none of the
  flags that would make the program write or run something (`find -exec`,
  `find -delete`, `sort -o`, `tree -o`, `rg --pre`) is there. A pipeline
  counts once, as the most specific thing it does — `cat f | grep x` is a
  search, `ls | head` a listing. The display tolerates an expansion
  (`cat $FILE` *looks like* a read) and a pager or awk (reads to look at),
  which the permission verdict refuses.

A member must have **succeeded**: a failed or refused read is a breaker, and
its red cell shows whole — a failure never whispers.

Three kinds of record are read **through**: a thinking phase (whose
`Thought for …` line commits as it happens and says nothing about the tools
around it), a task call and a hook note (cell-less inline). So a model that
thinks between every read still folds its reads, and the `Thought for` lines
stand above the summary, where they landed.

Everything else **breaks** the group: assistant text, a `write`/`edit`, a
command the tables cannot read, an MCP call, a subagent launch, a user message
steered into the turn, a background notice, the turn's receipt.

## The summary

Three counts, in Claude Code's order — searches, reads, listings — each
clause only when counted, the first capitalized, present tense with a `…`
while the group is open and past tense once it has settled
([`app::ToolGroup::summary`]):

| Open | Settled |
| --- | --- |
| `Searching for 1 pattern` | `Searched for 1 pattern` |
| `Reading 2 files` | `Read 2 files` |
| `Listing 1 directory` | `Listed 1 directory` |

## Each value, long enough to read

The open row's `⎿` hint names a member's **value**: a command with its
whitespace collapsed, cut to the row with `…` (`$ ls -la`), or a file by the
session's path display rule (`mathutil.py`). *Which* member's is a matter of
time. A read resolves in milliseconds, and a parallel batch of them is over
before the strip has drawn twice, so a row that simply named the running
call flashed every value but the last past in a frame: a batch of `ls -la`
and `read README.md` read as `README.md`, and the listing was never seen to
run. Claude Code holds each hint for `MIN_HINT_DISPLAY_MS` (700 ms); the row
does the same ([`app::HintHold`], [`app::MIN_HINT_DISPLAY`]):

- each member's value **joins a queue as its call starts**
  (`App::start_tool`), so a batch's values line up in call order — the same
  file read twice is one value;
- the value on the row **stays at least 700 ms**; then the next waiting
  takes it — at once when the row has been up longer than that, so one call
  a round, seconds apart, still shows each value the moment it starts;
- **at most [`app::HINT_BACKLOG_MAX`] (8) values wait**: a batch of twenty
  reads would otherwise keep the row busy for fourteen seconds after the
  batch was over, and every batch after it would push the row further behind
  what is actually running. Past eight the oldest waiting gives way to the
  newest, so the row trails reality by a few seconds at most and always ends
  on the newest member;
- the group **closing forgets the queue** — a settled line has no row, and
  the next group's first value takes its row at once.

Time reaches the pure core the way it always does here: the boundary injects
the frame clock before every draw ([`App::advance_tool_hint`], the same
reading as [`App::set_pulse`]), and the draw tick's 32 ms cadence while a
turn runs is what moves the row on. A running command that has been at it
for `TOOL_BACKGROUND_HINT_DELAY` (3 s) shows its own clock on the hint — `⎿ $
find / -name x (12s)` — **only once the row has reached its value** (a clock
on an earlier file's name would be a lie), and the Ctrl+B hint row still
appears under it, the running cell's own rule; a fast command never shows
either.

## How it is built: a hold, and the flushes

Scrollback is immutable, so a row that could still change cannot be written
there. The parallel MCP run already solved this shape (`docs/mcp.md`): a
member's cell is **held** out of scrollback while the run goes on, the strip's
aggregated cell speaks for it, and the run commits once, as one line, when it
ends — and the rebuild walk derives the same line from the same history, so
a resize can never disagree with the commit path. The compact group is that
mechanism with a different "run": not one announced batch, but every member
since the last breaker, across rounds.

One runtime bit says whether the trailing run is held: [`App::tool_group_open`]
(`app/tool_group.rs`). The call that resolves decides, as it resolves, what
its commit does about the group ([`app::GroupCommit`], read by
`Session::commit_tool_cell` through `ui::tool_commit_lines_with`):

- **Hold** — a member, and the next queued call (if any) is a member too:
  nothing commits, the bit is set, the strip's summary counts it.
- **FlushWith** — a member whose announced successor is a breaker (a batch of
  `[read a, write b]`): the group ends *with* this call, committing its one
  settled line now, so the summary never stands through the write's
  permission prompt.
- **FlushBefore** — a breaker that found the group open (a failed read, a
  write with no batch announcement ahead of it): the settled line commits
  first, then the breaker's own cell, a spacer between.
- **Off** — verbose output, or nothing open.

The breakers that are not tool resolutions flush the group **explicitly**,
each at the one place it commits, through `Session::flush_tool_group`: the
first non-blank reply chunk (`StreamEvent::Chunk`), a call that is no member
starting (`ToolStart` — and the batch announcement opening on one, so the
group settles before that call's prompt can open over it), a steered user
message landing, a background or agent notice settling, the agent group's
tree cell, the turn's end (`StreamDone` before the receipt, a backend error or
an Esc before the cells the death resolved — the open bit read *before* the
death resolves the queue, since the resolutions close it). The flush is a
no-op when nothing is open, which is always in verbose output.

The **rebuild** — a resize, an overlay return that purges, a `/resume`, the
Verbose output switch itself — walks history under a [`ui::GroupWalk`]
(`conversation_lines_with`): each run of consecutive members
([`app::run_len`], thoughts and the cell-less records among them included) is
one settled line placed where the run **closed**, after the `Thought for`
lines of the thoughts it read through; and the run still open at the
history's end (`open`, the bit) paints nothing but those thoughts — the strip
speaks for it. The walk and the commit path are pinned to agree
(`ui::tests::tool`).

The **strip** ([`ui::live::preview_tool_lines`]): the held members and the
queue's leading members read as one summary ([`App::live_tool_group`]) over
the value the row has up, ahead of whatever the queue holds behind them — a
mixed batch's `write` keeps its own `⎿ Waiting…` cell under the summary. With
nothing queued at all the summary stands alone, blinking between rounds, or
over the next round's streaming `write` draft or its `● Thinking…` block,
which would otherwise take its place; the preview is sized from the same walk
it is painted from, the strip's standing rule. The permission prompt's context
cells are deliberately untouched: a prompt is about one call, and the user
reads its whole `● Bash(ls -la)` cell there.

Switching **Verbose output** mid-turn re-decides the bit
([`App::reopen_tool_group`]): on, with a turn running and no reply started,
the trailing run is held again and the rebuild moves it into the strip; off,
it is released and the rebuild paints every cell. Every history rewind — a
`/clear`, a `/resume`, a backtrack, an interrupt-undo — forgets the bit.

## What stays as it was

- **Verbose output on** is byte-identical to the app before the row: the
  walk is `GroupWalk::Off`, every commit `GroupCommit::Off`, the strip the
  per-cell one. The whole existing smoke suite runs with the default.
- **Ctrl+O, Ctrl+D, `/export`, the rollout** — every member whole; the group
  is a view, not a record.
- **The agent session view** keeps a cell per call, as it keeps a cell per
  MCP call (`docs/mcp.md`): a subagent's transcript is its own conversation,
  and its commits go through `GroupCommit::Off`.
- **A `!` shell command** is the user's own and never a member.

## Tests

- `app::tests::tool_group` — membership (a picture, a `!` run, a mutating
  command, a companion call are not members), runs and the trailing run, the
  tally (distinct paths, the pipeline rule, the grammar in both tenses), the
  hold/flush decision at every shape, the live group's counts and hint, the
  switch mid-turn; and the row's hold — a batch's values in turn, each its
  minimum, a value up past the minimum giving way at once, the bounded
  backlog ending on the newest member, the clock riding the running
  command's own value only, a closed group forgetting its queue, a repeated
  value counted once, and the switch to compact feeding nothing stale.
- `ui::tests::tool` — the live rows (the blinking bullet, the `$ ` hint, the
  path by the display rule, the clock, the narrow cut), the settled line's
  dress, and that `tool_commit_lines_with` and `conversation_lines_with`
  write the same rows in every shape, thoughts included; `ui::tests::live` —
  the strip's one summary over the hint and the waiting write, a batch's
  first value held on the row while the read behind it runs, the held group
  with nothing queued (and under a thinking block), verbose untouched.
- `readonly::tests` — the lexer, the display classification and the
  permission verdict (`docs/permissions.md`).
- `smoke.sh` Phase 146 drives the real binary through the dummy's `explore`
  demo (a listing and a read in one parallel batch, then a thought and a
  search): the batch's summary with its two values taking the row one after
  another — `$ ls -la` held past the listing's own end, then `mathutil.py` —
  the search joining across the thought, the one settled line under the
  `Thought for` row, Ctrl+O showing every cell, the overlay return and a
  resize keeping exactly one summary, verbose output's three cells, and the
  `/settings` switch folding them on the rebuilt screen.

[`app::member_of`]: ../src/app/tool_group.rs
[`app::ToolGroup::summary`]: ../src/app/tool_group.rs
[`App::tool_group_open`]: ../src/app/tool_group.rs
[`app::GroupCommit`]: ../src/app/tool_group.rs
[`app::run_len`]: ../src/app/tool_group.rs
[`App::live_tool_group`]: ../src/app/tool_group.rs
[`app::HintHold`]: ../src/app/tool_group.rs
[`app::MIN_HINT_DISPLAY`]: ../src/app/tool_group.rs
[`app::HINT_BACKLOG_MAX`]: ../src/app/tool_group.rs
[`App::advance_tool_hint`]: ../src/app/tool_group.rs
[`App::set_pulse`]: ../src/app/status.rs
[`App::reopen_tool_group`]: ../src/app/tool_group.rs
[`ui::GroupWalk`]: ../src/ui/conversation.rs
[`ui::live::preview_tool_lines`]: ../src/ui/live.rs
[`readonly::kind`]: ../src/readonly.rs

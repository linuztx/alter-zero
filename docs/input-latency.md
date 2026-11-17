# Input latency — a key is shown when it is typed, whatever the session is doing

## What is held

Type into the composer while a reply streams, a command prints, or a turn
ends, and the character appears as fast as it does on an idle screen. The same
for a scroll key in the Ctrl+O pager. Nothing the session does in the
background may stand between a key and the frame that shows it.

It did not hold. The composer never dropped a key, but it showed one late: a
few milliseconds late under any stream, tens of milliseconds late under a
command that printed a lot, and most of a second late at the end of a long
code reply. This document is what was measured, what caused each delay, and
what guards the fix.

## The probe

`examples/latency_probe.rs` is the terminal. It starts the real binary in a
pseudo-terminal, answers its startup questions through the session emulator
(`pty::screen::Screen`), and plays a provider on the loopback that streams a
reply of a chosen shape at a chosen pace. While the reply streams it types at
a typist's cadence and times each key from the write that sent it to the
frame that shows it: the cursor back on the composer, right after the
character.

```bash
cargo build --release && cargo run --release --example latency_probe
cargo run --release --example latency_probe -- --scenario code --rate 0
cargo run --release --example latency_probe -- --scenario code --overlay
cargo run --release --example latency_probe -- --scenario burst --draft 4000
```

A scenario is the shape of what streams: `prose`, `code`, `table`, `mixed`,
`reasoning`, `thought` (reasoning in one paragraph, no line break in it),
`oneline` (one line with no newline in it), `write` (a file
streamed into a `write` call), `bash` (a command printing at `--rate` lines a
second), `idle`, and `burst` (`--draft` characters typed in one write).
`--rate 0` is a provider that never pauses, which no model is, and which is
what shows whether the loop keeps its promises under load. `--overlay` times
the Ctrl+O pager's scroll keys instead of the composer's. `--warmup N` gives
the measured turn a history of N turns. Per scenario it prints the latency's
distribution, the CPU the loop thread spent, the frames and bytes written and
the resident memory; `--json` adds the five slowest keys and where in the run
they fell.

Everything below is the release binary on a 120×40 terminal. "A model's
pace" is 400 stream events a second of four characters each, a 60 KB reply,
and a key every 60 ms.

## What it found

Key to paint, in milliseconds (p50 / p90 / p99), and the loop thread's CPU
over the run:

| streaming | before | loop | after | loop |
|---|---|---|---|---|
| nothing (idle) | 1.96 / 2.16 / 3.44 | 41 ms | 0.84 / 1.09 / 2.03 | 41 ms |
| prose | 4.49 / 7.79 / 9.44 | 2.2 s | 0.61 / 2.37 / 2.84 | 2.3 s |
| code | 4.68 / 8.00 / 9.36 | 2.6 s | 0.71 / 2.32 / 2.94 | 2.5 s |
| a table | 5.32 / 8.60 / 10.16 | 4.8 s | 1.37 / 2.79 / 4.05 | 5.0 s |
| reasoning | 4.72 / 8.20 / 9.43 | 2.2 s | 0.63 / 2.50 / 3.21 | 2.1 s |
| a `write` call | 5.22 / 8.69 / 10.23 | 4.6 s | 0.94 / 2.69 / 3.79 | 3.4 s |
| a command, 900 lines in 2.6 s | 13.2 / 32.4 / 46.1 | 1.2 s | 0.84 / 1.12 / 2.74 | 0.07 s |
| a command, 4000 lines/s for 7 s | 39.5 / 66.1 / 87.5 | 6.3 s | 0.85 / 1.02 / 2.04 | 0.19 s |
| one 60 KB line | 6.17 / 9.59 / 11.44 | 8.1 s | 2.56 / 4.89 / 8.30 | 9.4 s |
| a thought, one 100 KB paragraph at 1000 events/s | 19.9 / 35.7 / 51.1 | 21.2 s | 0.51 / 0.72 / 3.00 | 1.7 s |

And the stalls, which a percentile over a paced run hides because they
happen once:

| | before | after |
|---|---|---|
| the slowest key as a 400 KB code reply ends | 778 ms | 19 ms |
| …a 400 KB mixed reply | 205 ms | 4 ms |
| …a 400 KB table | 79 ms | 7 ms |
| a scroll key in Ctrl+O under a streaming code reply (p50 / p99) | 145 / 280 ms | 1.3 / 3.9 ms |
| the loop's CPU over that 38 s run | 35.6 s | 3.6 s |
| 4,000 characters typed in one write | never shown whole | 3.9 ms |
| 20,000 | never shown whole | 10 ms |

Ten causes, none of them the textarea.

## 1. A frame that was due waited on a timer

`frame::run_scheduler` sent every frame through `tokio::time::sleep_until`,
including one that was due the moment it was asked for — which is every key
on an idle composer. The runtime's timer wheel counts in whole milliseconds,
so a deadline of *now* fires one to two milliseconds later. That was the
whole of the idle latency: 1.96 ms to paint one character.

A frame whose deadline has passed is now emitted at once, with no timer; only
a frame still inside its floor, or one asked for a beat out, sleeps. The
requests one turn of the loop makes (a key's handler, then its bookkeeping)
are folded into the pending frame first, so emitting at once does not turn
them into a frame now and a second, identical one a floor later.

`a_due_frame_is_emitted_without_a_timer` and
`requests_made_together_are_one_frame` run the scheduler on a runtime built
without a time driver, where any path that arms a timer ends the task.

## 2. A due frame lost to the stream's own events

The loop was one `select!` over every source with tokio's default random
branch order, which this codebase described as fairness for free. It is fair
between sources that are each ready now and then. Against a reply channel
that *always* has an event ready, a due draw tick won one poll in fourteen:
the frame that would show a key stood behind a dozen more stream events, each
of which asked for the same frame again.

Input is now polled first and the draw tick second (`biased`), and every
other source is chosen among the rest by `next_wake`'s own unbiased `select!`,
so the sources that can each arrive in a flood — a reply, a subagent's
stream, a shell's output — still cannot starve one another. A key cannot
crowd anything out, since keys arrive at a person's pace, and neither can a
frame, since the scheduler emits at most one per floor and one per run of
keys handled together.

Draining several reply events per wake was considered and not done: with the
frame no longer behind them it measured nothing, and one event per iteration
keeps the loop bottom's bookkeeping exact.

## 3. A key waited out the stream's frame floor

With 1 and 2 fixed the latency under a stream was a clean 0 to 8.3 ms, 4.5 on
average: the 120 fps floor (`MIN_FRAME_INTERVAL`). A stream's frames are
paced by it because nothing is lost by showing a token eight milliseconds
late. A key typed just after one of those frames waited the rest of the floor
for the next.

A key press — and a paste — now asks for an **input frame**
(`FrameRequester::schedule_input_frame`), which is due when it is asked for
(`FrameRateLimiter::clamp`). It still paints everything, so the stream loses
nothing; the stream's next frame is simply a floor after it, so typing adds
at most its own rate to the frame rate. A character inside a detected paste
burst keeps asking for a relaxed frame, as before.

It first kept a short floor of its own, two milliseconds after the last
frame, so that a run of keys no burst detector caught would be a bounded
number of paints. That floor cost more than it read: any wait at all goes
through the runtime's timer (cause 1), so a key that owed a tenth of a
millisecond paid one to two, and under the offline demo's stream — a frame
every 10 ms, keys every 50 — a tenth of the keys took 2.1 ms and one in a
hundred 3.6, against 0.7 at the median. Nor was it what bounded the paints:
the loop takes every key already waiting before it draws (cause 2), so keys
that arrive together are one frame with no floor at all — 20,000 characters
typed in one write are still one paint. Without it the same stream reads
0.6 / 0.7 / 1.0 ms (median / p90 / p99).

That is what took a key under a stream at a model's pace from 4.5 ms to
about half of one.

## 4. The strip's live cells were built nine times a frame

The live region's height, its paint, its cursor seat and its flow signature
are separate pure functions (`ui::live_height`, `render_live`,
`cursor_position`, `view_flow_signature`), and each asked the same question —
how many rows does the strip's live cell take — by building the cell: the
running command's tail, a `write` call's preview, the agent tree. Eight or
nine builds per draw. Under a `write` stream that was 60% of the loop's time;
under a noisy command it was most of the 52 ms a frame took.

`ui::frame` opens a frame: inside it, the first reading of the strip's live
cells at a width builds them and the rest are answered from that build
(`ui/frame_memo.rs`). The boundary opens one around each draw
(`Session::draw_conversation`, the rebuilds, and the draw tick's checks
together with its paint). Outside a frame nothing is kept, so every caller
that is not a draw — and every unit test — sees exactly what it saw before.

The memo rests on one claim: a draw changes nothing the cells are built from.
That is why the scope is opened by the boundary, around a draw, and never
lives in `App`; and it is checked wherever debug assertions are on — a hit is
compared with a fresh build — so the smoke suite, which runs the debug
binary, fails on any state a draw mutates. The rebuilds open their frame
*after* they have looked for the pictures' files again, which is the one
thing on a draw path that can change what a cell reserves.

`a_frame_builds_the_strips_live_cells_once` counts builds, not time.

## 5. A running command's tail walked its whole output, twice, per build

The running `bash` cell shows four rows of an output that can be thousands of
lines (`docs/tool-streaming.md`), and to know which four it counted the
display rows of every line. Each build copied every line into a `String` of
its own and then walked each one a grapheme at a time, twice — about 2 ms a
build at the 64 KB the cell retains.

Two changes. A line no longer than its row **in bytes** is one row, without a
scan: a character is never wider than its bytes, so nothing in it can
overflow (`WrapMode::scan_hanging`'s first branch; almost every line a
command prints). And the tail reads its lines borrowed from the output
(`display_lines`, a `Cow` per line, owned only where a tab is expanded) and
counts each line's rows once, for the sums and the window walk alike.

With 4, a key under a command printing 4,000 lines a second went from 39.5 ms
to about one, and the loop from 90% of a core to 3%.

`the_running_tail_counts_a_long_output_without_walking_its_lines` counts
scans.

## 6. A committed item was rendered for Ctrl+O between two keystrokes

The Ctrl+O transcript is pre-rendered so the pager never opens cold
(`docs/tool-view-performance.md`), and the pre-render ran at the loop bottom
of the iteration that committed the item. Rendering an item is the grammar
over all of it. A 2000-line `read` cost 216 ms there; a 200-line one, 26 ms;
an `edit` of 600 lines, 43 ms; a 60 KB code reply about 100 ms and a 400 KB
one 650 to 850 ms — each a stretch in which no key was read, at the exact
moment a reply or a tool call lands and the user starts typing the next
message. A resumed session paid for its whole history at once.

The rendering now happens on a worker thread
(`tui::workers::spawn_transcript_worker`). The cache hands out what it would
have rendered as jobs and takes the rows back:

- `TranscriptCache::warm_jobs` returns the history items not yet in the
  frozen prefix as `TranscriptJob`s, each once, at most
  `TRANSCRIPT_WARM_WINDOW` (8) out at a time. A job owns a copy of its item
  and everything its rows depend on that another thread does not have: the
  active theme and the way a labelled link shows its target are per-thread
  cells, so the job carries the loop's and renders under them. The link
  interner and the image registry are process-wide, so nothing else needs
  carrying.
- `TranscriptJob::run` renders the item straight into compact rows
  (`CachedLines`), so what crosses back is a few buffers.
- `TranscriptCache::accept` splices a result onto the prefix
  (`CachedLines::append`: the text copied whole, each style mapped onto the
  cache's own ids). It is refused unless it is the very next item of the
  prefix as it stands. Every reset of the prefix — a rewound history, another
  width, another theme — is a new **epoch**; a job is of the epoch it was
  handed out in, so a result from before a reset can never be spliced into
  what followed it, and the worker skips a job whose epoch is already over
  without rendering it (a resize drag leaves dozens).

The loop bottom calls `warm_jobs` where it called `warm`, and each result's
arrival is a wake of the loop (`Wake::Transcript`) whose own loop bottom
hands out the next window. An open pager is the exception: its draws render
what they are missing themselves, there and then, exactly as before — the
pager is where the rows are needed now — so no jobs go out while it is open,
and a result for an item a draw already rendered is refused.

Two things changed with it.

A **resize** used to leave the cache at the old width until the next Ctrl+O,
which then rebuilt the whole history on the loop thread: the synchronous
`warm` skips a width-only mismatch, since a drag delivers dozens of resizes
and each would have cost a full render. Jobs cost the loop nothing, so
`warm_jobs` re-keys at once and the pager is warm at the new width too.

A render that **panics** used to end the app. It still does: the worker holds
the only sender of the result channel, so its thread ending closes the
channel, which the loop reads as `Wake::Transcript(None)` and ends on — after
the panic hook has printed why and put the terminal back.

### Rows go straight onto the page

A worker thread is a second allocator arena. Rendering an item used to build
every row as owned lines — a `String` and a `Style` per highlighted token —
and then compact them. On the loop thread that transient was a hole the next
allocation reused; on a worker it was a heap of its own that stayed resident:
+2 MB after twenty 60 KB replies, +11 MB after one 400 KB table
(`docs/memory.md` has the rule).

So an item's rows are no longer built first and compacted after.
`transcript_item_into` writes them onto the page in whatever form the page
keeps (`RowSink`: the cache's compact rows, or plain lines for an agent's
page), and an assistant reply's are handed out one at a time as its lines
render (`assistant_rows`, the one implementation `assistant_lines` now
collects). The worker's transient is a line. Resident memory after the same
runs is the baseline's or under it.

## 7. The pager rendered a streaming reply whole, per refresh

The transcript's live tail — the reply still streaming — was
`message_lines(text)` on every refresh: the grammar over the whole reply so
far, per frame. A scroll key in Ctrl+O waited 145 ms behind a 60 KB code
reply, and the loop spent 35.6 s of a 38 s run rebuilding it.

The cache now keeps a `StreamRender` of its own for the live reply
(`LiveReply`). `StreamRender::whole_after` renders only the lines that
completed since the last call and says how many of the reply's leading rows
are **settled** — the rows of finished lines, less a trailing run of blank
ones, which are dropped while they end the reply and shown once content
follows. The tail's rebuild cuts the page back to the settled rows it
already holds and appends what follows them, so a refresh costs what
changed.

It is only as good as the claim that the buffer it is shown is the one it
rendered, grown, and that does not hold across a turn: a tool call finalises
the segment and another starts, a failed attempt is withdrawn and made
again. So the text the rows came from is kept, and a buffer that does not
start with it starts the render over. One comparison of bytes — a length or
a counter could be fooled by a longer buffer.

`the_reply_so_far_is_the_batch_render_at_every_prefix` holds `whole` to the
batch render over the hostile corpus;
`the_pagers_live_reply_is_a_fresh_build_at_every_step_of_a_stream` holds the
cache to a from-scratch build through the markdown tour, with tool calls
splitting the reply and jobs coming back in between.

## 8. A typed burst over 1 KB stopped at 1 KB

Type — or paste into a terminal with no bracketed paste, or dictate — more
than a kilobyte in one piece, and the composer showed the first 1024 bytes.
The rest appeared 1024 bytes at a time, one batch per further key press.

crossterm's unix event source reads the terminal 1024 bytes at a time behind
mio's poll, which is edge-triggered: it reports the terminal readable when
input *arrives*, not while input *remains*. The source returns the first
event of a read without reading on, so once the events of one read were
handed out it went back to a poll that had nothing new to report. A
bracketed paste was never affected: the parser returns no event until the
paste's end, so the read loop runs on.

crossterm 0.29.0 is the latest release and its master is the same, so the
crate is vendored with the fix (`vendor/crossterm`, `vendor/README.md`): a
read that fills the buffer is noted, and before waiting again the source
reads what is left — while `FIONREAD` says there is something to read, since
the descriptor may be a blocking one and a read with nothing to return would
wait there for the next key.

`scripts/smoke.sh` Phase 159 types 3,200 bytes in one write and reads the
composer's last words.

## 9. A frame was several writes

Every paint queues its escapes and ends on one flush, so that a frame is one
write and the terminal never reads half of one (`docs/flicker.md`). It was
not one write, for two reasons.

std's `Stdout` is a line writer over a 1 KB buffer: it writes whenever it
holds a newline and whenever it fills. And ratatui's backend flushes inside
`set_cursor_position` and `append_lines`, which the frame paths called
part-way through. A frame went out as three writes on average — the paint,
then the cursor's `Show` and the end of the synchronized update on their own
— and a frame that scrolled wrote the scroll first, ahead of the repaint it
made room for. A flood of commits was one write a row: 17,600 writes for
6 MB.

`term::output::FrameWriter` holds a frame until the flush and writes it in
one piece, through a descriptor of the viewport's own rather than through
`Stdout`. The frame paths queue their cursor moves, scrolls and clears
(`InlineViewport::queue_cursor`). A frame is now one `write`; past 64 KB a
frame is no longer held whole, since a purge rebuild is megabytes and no
terminal takes that in one read.

This is the one change here that is not about a key: it is what the code
already said it did, and it is what keeps a terminal without synchronized
updates from painting between a scroll and the repaint.

## 10. A long thought was wrapped whole on every frame

The live `● Thinking…` block shows the last three wrapped rows of the
thought, and to find them it wrapped the newest paragraph whole on every
build. A model that thinks in one long paragraph — many do, and some
providers stream reasoning with hardly a line break — made that the whole
thought: over a 100 KB thought at a thousand events a second a key took
20 ms at the median and 51 at the 99th percentile, and the loop held 85% of
a core. One build a frame (4, above) brought the key to 3 ms and left the
wrap itself, half a core of it.

The thought now keeps the rows a later delta cannot change and wraps only
what arrived since (`app::LiveThought`, `ui::wrap`'s `output_tail` —
`docs/thinking-stream.md` *A long thought costs what it adds*): half a
millisecond, and 7% of a core.

## What was measured and left alone

- **One huge line.** A reply that is a single 60 KB line costs the loop 25%
  of a core while it streams: the strip previews the uncommitted tail, and a
  line with no newline in it is all tail. Keys stay under 9 ms at the 99th
  percentile. The commit
  path already amortizes its own share (`TAIL_EVAL_MIN`); doing the same to
  the preview would show a stale frontier.
- **A `write` call under a flood.** With a provider that never pauses, a key
  under a streaming `write` is 8 ms at the median and 15 at the 90th
  percentile: the preview shows the newest ten
  lines coloured, and the grammar's state for those depends on every line
  before them, so each frame highlights whatever arrived since the last. At a
  model's pace that is a dozen bytes a frame.
- **Rendering items on the loop in budgeted slices.** It needs no thread, but
  an item is rendered whole or not at all, so the slice is as long as the
  largest item.
- **Handing the pager the rows the inline render already made.** A reply is
  rendered twice, once for scrollback and once for the transcript, and the
  second could be skipped by passing the first's rows across. It would tie
  the cache to the two dozen places the inline render is reset; the worker
  made the second render free to the loop instead.

## What guards it

- `frame::tests` — the scheduler on a runtime with no timer: a due frame, a
  key's frame right behind a stream's, and keys handled together as one.
- `ui::tests::live::a_frame_builds_the_strips_live_cells_once` and its
  siblings — one build a frame, one per width, nothing kept outside a frame.
- `ui::tests::tool::the_running_tail_counts_a_long_output_without_walking_its_lines`,
  `ui::tests::wrap::a_line_no_longer_than_its_row_is_counted_without_a_scan`.
- `ui::tests::transcript` — a job's rows are the cache's own; handed out
  once, a window at a time; refused after a reset, out of turn, or once a
  draw has rendered the item; a job on another thread carries the theme and
  the link display; the live reply renders a line once and puts its settled
  rows down once.
- `ui::cached_lines::tests` — appending a store is extending with its rows.
- `ui::tests::reasoning::the_live_block_wraps_only_what_arrived_since_the_last_frame`
  counts the bytes the wrapper walks, and the two `the_kept_tail_agrees_…`
  tests hold every frame of a hostile thought to the whole-paragraph wrap.
- `term::output::tests` — a frame is one write.
- `scripts/smoke.sh` Phase 159 — the typed burst.
- `examples/latency_probe.rs` — the numbers above. It is not a gate: a
  latency in milliseconds measures the machine it runs on.

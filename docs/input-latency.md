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
`reasoning`, `oneline` (one line with no newline in it), `write` (a file
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
frame, since the scheduler emits at most one per floor.

Draining several reply events per wake was considered and not done: with the
frame no longer behind them it measured nothing, and one event per iteration
keeps the loop bottom's bookkeeping exact.

## 3. A key waited out the stream's frame floor

With 1 and 2 fixed the latency under a stream was a clean 0 to 8.3 ms, 4.5 on
average: the 120 fps floor (`MIN_FRAME_INTERVAL`). A stream's frames are
paced by it because nothing is lost by showing a token eight milliseconds
late. A key typed just after one of those frames waited the rest of the floor
for the next.

A key press now asks for an **input frame**
(`FrameRequester::schedule_input_frame`), which waits only
`MIN_INPUT_FRAME_INTERVAL` (2 ms) after the last frame. It still paints
everything, so the stream loses nothing; the stream's next frame is simply a
floor after it. The short floor is still a floor: a run of keys the paste
burst detector did not catch is a bounded number of paints. A character
inside a detected burst keeps asking for a relaxed frame, as before.

That is what took a key under a stream at a model's pace from 4.5 ms to
under one.

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

## What guards it

- `frame::tests` — the scheduler on a runtime with no timer, and both floors.
- `ui::tests::live::a_frame_builds_the_strips_live_cells_once` and its
  siblings — one build a frame, one per width, nothing kept outside a frame.
- `examples/latency_probe.rs` — the numbers above. It is not a gate: a
  latency in milliseconds measures the machine it runs on.

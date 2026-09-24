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
     +5 lines (9s · timeout 2m)
     (ctrl+b to run in background)

● Bash(ping -c 10 facebook.com)                     (a parallel batch's not-yet-run
  ⎿  Waiting…                                        siblings stay `⎿ Waiting…` until
                                                      each one's turn — docs/parallel-tools.md)
```

The footer's clause is the **command's own clock beside the timeout it
runs under** — `(9s · timeout 2m)` — and it is on the live cell whatever
the output's shape (*The clock row is always there*, below).

The two states are deliberately asymmetric — while **running** you want the
**tail** (what just happened); once **finished** you want the **head** with an
expand hint. The tail shows the last `TOOL_PEEK_ROWS` (4) display rows; the
head **folds** at `TOOL_FOLD_ROWS` (3) rows over the hint — Claude Code's
fold, at most the same four rows hint included — so the cell never grows when
it settles. (It can settle *shorter*: the finished head is the output's first
**block**, so a blank line ends it — `docs/long-lines.md`. The running tail
keeps its blanks, being what the command just printed, and shows the output
as printed where the settled cell reshapes a JSON line for display.)

## The protocol — `ToolOutput` and `ToolScreen`

Two events carry the **currently-running** tool's live output, between its
`ToolStart` and `ToolEnd`:

```rust
/// A chunk of the running tool's output, appended as it is produced.
ToolOutput(String),
/// The running tool's output as it builds up: `settled` text appended for
/// good, `live` rows replacing the `live` rows sent last.
ToolScreen { settled: String, live: String },
```

`ToolOutput` is the append-only form — the offline dummy's scripted commands
stream with it. `ToolScreen` is what the real executor sends: a line still
being drawn (a progress bar redrawn with `\r`, a prompt taking its answer)
travels as `live` and is **replaced** by the next update instead of stacking
a row per frame, and a line that is final travels once as `settled`
(`docs/interactive-shell.md` *Streaming the running cell*). Both target the
**front** of `App::tool_queue` (the only `Running` call — execution is
sequential even for a parallel batch, so the running call is always the
front; see `docs/parallel-tools.md`). A backend that never streams simply
omits them — `ToolStart`/`ToolEnd` still work unchanged.

## App state — `push_tool_output` / `push_tool_screen`

`App::push_tool_output(chunk)` appends `chunk` to the front call's `output`;
`App::push_tool_screen(settled, live)` truncates the `live` tail it appended
last (`App::tool_live_len`), appends `settled`, then the new `live`
(`app::apply_tool_screen`, which the subagent fold in `agents` shares). Both
act **only when the front call is `Running`** (a no-op otherwise), and both
bump `App::tool_revision`, which the Ctrl+O transcript cache signs on — a
redraw can leave the output the same length. Neither counts tokens — the
tally is charged once, from the authoritative `ToolEnd` output, in
`end_tool` (which overwrites `output` with the final framed text). So the live
tail and the final cell never double-count, and the final cell is always exact
even if an update was dropped.

## The executor — `run_bash` streams folded output

`ToolExecutor::execute` takes an `on_output: &mut dyn FnMut(ToolProgress<'_>)`
sink; `run_agent` maps `ToolProgress::Screen` onto `ToolScreen` (and
`ToolProgress::Title`, a `bash_session` call's refined header, onto
`ToolTitle`). `run_bash` drains both pipes on reader threads (so a chatty
command can't deadlock a full pipe); each reader sends raw byte chunks over an
`mpsc` channel, and the main poll loop feeds them in arrival order to a
`PipeOutput`: a **fold** (`pty::fold`) that replays the stream the way a
terminal shows it — `\r` and backspace overwrite, colour escapes vanish, tabs
and trailing spaces stay — keeping each line once it ends, within the output
cap. It streams the ended lines as `settled` and the line still being drawn as
`live`, at most every `PIPE_STREAM_INTERVAL` (50 ms, and once more at the
end), so a burst of progress frames costs one redraw per interval. The parser
carries a UTF-8 character or an escape split across chunks to the next one,
so the tail never shows a stray replacement glyph. `read`/`write`/`edit`
ignore the sink (nothing to stream).

The final `ToolOutcome.output` is codex's `Exit code: N` frame over the
(capped) **folded** body — a `\r` progress bar reaches the model once, in its
final state — and `end_tool` overwrites the display `output` with it. A
Ctrl+B handoff gives the background registry the bytes as they were read, so
its monitor folds the whole stream itself (`docs/background.md`).

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
  `TOOL_PEEK_ROWS` display **rows** of output, then the **clock row** —
  `+{hidden} lines ({elapsed} · timeout {limit})` when any rows are fully
  hidden above the window, the bare `({elapsed} · timeout {limit})` when
  none are. No output yet → `⎿ Running… ({elapsed} · timeout {limit})`, the
  clause on the corner row itself. The `{elapsed}` is the **command's own**
  runtime, never the turn's — *Whose clock the footer shows*, below — and
  the `{limit}` is the timeout the call runs under — *The clock row is
  always there*, below. Long lines **word-wrap** the same way (`wrap_output`)
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

### Whose clock the footer shows

The `+N lines (Ns · timeout …)` footer — and the `!` shell's `⎿ Running… (Ns)`
row — count from the moment the **command** started, never from the turn's start.
`preview_tool_lines` used to hand the status indicator's clock (the turn's
`elapsed`) down to `running_command_lines`, so a `bash` call that began a
minute into a turn opened on `+N lines (60s)` under a cell that had just
started, the footer and the `Working… (60s ·` beside it counting in step —
the reported bug. Two clocks, two questions: the status line answers *how
long has this turn run*, the footer *how long has this command run*.

The command clock already existed. `StatusClocks::command_start` is set at
a model `bash` call's `ToolStart` and at a `!` shell's launch (`run_shell`,
which starts it together with the turn's, so the shell row's number is
unchanged), cleared at every resolution and injected each frame as
`App::set_command_elapsed` — the delayed `(ctrl+b to run in background)`
hint had waited on it since `docs/background.md`. The strip now reads the
same injection through `App::command_elapsed`, the plain getter that answers
what the setter set. The hint's gate is the separately named
`App::background_hint_elapsed`: the same value masked to `None` wherever
Ctrl+B is swallowed — a permission prompt, the ↓ manager band, every
composer-replacing picker — because those keep the running cell on screen
above themselves, where its footer must go on counting while a hint for a
key they eat must not show. Naming the masked one for its one job is what
keeps a reader from picking the wrong twin: the getter that shares the
setter's name is the honest one.

The **agent session view** had the same bug with the agent's whole runtime
(`run.runtime`, what its status line shows) and takes the same shape: a
per-agent clock, `Session::agent_command_clocks`, inserted at that agent's
`ToolStart` and injected before each draw as `AgentRun::command_elapsed`,
the thinking clock's twin (`docs/agent-view-streaming.md`). Which entries
still apply is *derived* at the roster tick from the run's own queue — an
agent without a running front call loses its clock — rather than removed at
each of the four resolution events and two local settles, so no future
resolution path has to remember it. The main strip needs no such pruning:
its one `command_start` is cleared at the boundary's own resolution arms.

### The clock row is always there

The footer used to exist only while the output had overflowed the tail
window: a command whose output fit showed just its rows, and a command that
had printed nothing sat on a bare `⎿ Running…` for as long as it took — a
`sleep 100`, a build with quiet output, a test suite's warm-up — with
nothing on the cell saying it was alive, and nothing anywhere saying how
long it might go on. The status line carries the *turn's* timer, which is
the wrong number (the reported "footer counts the turn" bug, above), and the
timeout the model chose was known only to the executor.

The live cell now shows the command's clock **beside the timeout it runs
under**, in every shape the running cell takes:

```
● Bash(for i in $(seq 1 100); do echo $i; sleep 1; done)
  ⎿  19
     20
     21
     22
     +18 lines (22s · timeout 1m 50s)          ← rows hidden above the window
     (ctrl+b to run in background)

● Bash(python3 -u -c "
      import time…)
  ⎿  hello world
     (10s · timeout 10m)                       ← output that fits: the clause alone
     (ctrl+b to run in background)

● Bash(python3 -c "import time; time.sleep(100)")
  ⎿  Running… (10s · timeout 2m)              ← nothing printed yet
     (ctrl+b to run in background)
```

The timeout is the model's own: `llm::tools::bash_timeout_ms` reads the
`timeout` (or the pre-rename `timeout_ms`) off the call's verbatim
`ToolCall::arguments` and applies `BashArgs::timeout_ms`'s rule — the
120 000 ms default when the call names none, clamped to the 600 000 ms cap
— so the cell names exactly the limit the executor enforces. A call with no
argument record (the dummy backend's scripted calls) shows the default, which
is what such a call would run under. Reading the one field off the arguments
per animation frame is deliberate: a `bash` call's `command` can be a
kilobyte of heredoc, and a typed one-field parse skips it without copying
it, where parsing the whole `BashArgs` would allocate the command thirty
times a second for nothing.

The limit is humanized by `app::format_timeout`, `format_elapsed`'s sibling
with the zero parts dropped — `2m`, `10m`, `1m 50s`, `1.5s` for a
sub-second remainder — because a limit is a whole (`2m 0s` says the same
thing twice) while the elapsed keeps its seconds because it moves. The
`!` shell's `⎿ Running… (Ns)` row is unchanged: a `!` command has no
timeout, so there is nothing to name.

The row is live-only, like the tail it closes: `tool_lines` (scrollback
commits, the permission prompt's context, the frozen Ctrl+O transcript)
still renders a running command at rest as `⎿ Running…`, since those
surfaces have no clock to tick.

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
the whole cell used to drop its rows — the `+N lines (Ns · timeout …)` footer
first, then the output rows, then the header — into no buffer at all. The strip
bottom-anchors now and commits the rows it cannot paint into the terminal's own
scrollback, frozen: the cell **scrolls**, keeping its newest rows and its
clock row on screen while its head stays readable by scrolling up.
See `docs/strip-flow.md`.

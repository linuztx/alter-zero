# Background shells — `run_in_background`, Ctrl+B, and the ↓ manager

Claude-Code-style background command execution: the model can launch a `bash`
command that keeps running while the conversation continues, the user can move
a running command to the background with **Ctrl+B** (with nothing
backgroundable running, Ctrl+B is the terminal's cursor-left instead —
`docs/textarea.md`), and a **↓ manager band**
(reached from an empty composer — ↓ lights up the footer's shell indicator,
Enter opens the band) lists the running shells, streams a selected shell's
output live, and can stop one. When a background shell finishes, the
feedback is **immediate** — mid-turn, its model-facing note is posted onto the
registry's notice board (the in-flight agent takes the board before each round,
so a shell the model just `kill`ed — or the user just `x`-stopped — is known to
it *within the same turn*, right after the tool result that did the deed) and
the notice cell commits at the next safe boundary (a tool resolution), not the
turn's distant end; while idle, the notice commits at once and — for a
model-launched shell whose note no agent read — the model is automatically
told the result in a new turn.

## The pieces

### Protocol

- `llm::tools::ToolOutcome` gains `background: Option<String>` (the task id).
  `bash` args gain `run_in_background: Option<bool>` and
  `description: Option<String>`; the tool schema advertises both.
- A backgrounded call resolves with `StreamEvent::ToolBackgrounded {id, output}`
  **instead of** `ToolEnd` (`llm::agent` picks by `outcome.background`).
  `output` is the *model-facing* text — the **session id**, the interim-output
  file path and the completion promise. The id used to be left out, since
  nothing model-facing took one back; `bash_session` does now — it waits on
  a background command, interrupts it with `<C-c>` or ends it with `kill`
  (`docs/interactive-shell.md`) — so the launch names it. The cell never shows
  the text — it renders the fixed `⎿ Running in the background (↓ to manage)`
  row.
- That text differs by **who** backgrounded the call. A `run_in_background`
  launch gets the plain acknowledgement (`exec::background_launch_text` — the
  model asked, so the interim path + completion promise suffice). A
  **Ctrl+B handoff** gets `exec::background_handoff_text`: the same facts led
  by `The user moved this command to the background …` and closed with a
  don't-re-run/don't-poll steer — the model requested a *foreground* run, and
  without being told the user moved it, it expects the full output and
  re-reads the interim file round after round waiting for it. The recorded
  `tool.output` is this same text, so `context::context_messages` replays the
  explanation into every later turn's context too. Both close on what happens
  to the **model** — *you will be notified with the final output when it
  finishes* — rather than on the harness's own re-invocation: "you will be
  re-invoked" names a mechanism the model can do nothing with, and reads as an
  interruption to brace for instead of a result to expect. The `agent` tool's
  launch text (`backend::agent_launch_text`) promises it in the same words, so
  one vocabulary covers both ways of backgrounding. (The `bash` description
  still keeps the launch facts out — it says what the flag does and names one
  case it is for, a dev server or a full test suite, while the handoff text
  carries its own instructions at the moment they matter.)
- `ToolStatus::Backgrounded` is the resolved status: green header bullet, the
  fixed row as its body inline, in the preview, and in the Ctrl+O transcript.
  The wire/tool-result content in the derived context stays `tool.output`
  (`context::context_messages` needs no special case).
- **Subagents background too** (`docs/agent-tool.md`): a subagent's executor
  carries the same registry, launching via `launch_from` with a
  `BgOrigin { agent_id, agent_type }` so the shell is attributed everywhere —
  the details page's `From: {type} agent` field, the notice's `· from the
  {type} agent` suffix, the context note's `launched by the {type} agent`
  clause (persisted on the notice record; omitted when absent so old rollouts
  parse). The **Ctrl+B latch stays main-only**: a subagent's `bash` neither
  clears nor consumes it. A subagent-launched shell's completion note routes
  to its launcher first (`AgentRegistry::queue_input` onto that agent's steer
  seam, `docs/queue.md`); a settled launcher can't hear it, so the note falls
  to the shared board — the normal main-turn / follow-up path. Queueing is
  **all** the boundary does: the `StreamEvent::Steered` echo records the note
  on the agent's transcript when its loop actually takes it. Recording it
  eagerly as well put it there twice, permanently — in history, the rollout and
  every rebuild.

### The registry (`src/background.rs` — boundary, like `term`)

`BackgroundRegistry` is a cloneable handle shared by the event loop, the
executor (`llm::exec`), and the `!` shell runner:

- Task ids are **Claude-Code-style** — a `b` prefix + 8 lowercase base36
  chars (`bvyo7tkbe`), rolled per launch from a splitmix64-mixed entropy seed
  (nanos ⊕ pid ⊕ a launch counter — `tui::host::session_id`'s no-`rand`
  pattern) with a collision re-roll against the running set. The pure
  `task_id(seed)` pins the format.
- The interim files live in the session's own temp tree
  (`docs/scratchpad.md`) — the pure `scratchpad::tasks_dir(session_root)`:
  `{tmp}/alter-zero-{uid}/{session}/tasks/{id}.output`
  (e.g. `/tmp/alter-zero-0/18f…-4e2/tasks/bvyo7tkbe.output`) —
  a stable per-user root (Claude Code's `claude-{uid}` pattern) and a
  per-session dir keeping concurrent instances apart, with the `tasks` leaf
  naming these files apart from the agent's `scratchpad/` beside them. Short
  deliberately: the model reads these paths back out of every launch text, and
  the session id already separates projects, so a dashed-cwd segment would be
  pure length. The boundary injects the uid (`tui::host::process_uid` —
  `/proc/self`'s owner; no `libc` in a `forbid(unsafe)` crate) and session id.

- `launch(command, description, from_model)` spawns `sh -c` in its own process
  group **detached from the controlling terminal**
  (`subprocess::spawn_detached_shell` with the registry's detach helper — a
  `/dev/tty` password prompt fails fast instead of wedging the task; the
  registry also *carries* that helper to the executor and the `!` runner,
  `docs/tools.md`) and a **monitor thread** that merges
  stdout/stderr in arrival order, streams completed lines as
  `BgEvent::Output` — **folded** the way a terminal shows them (`pty::fold`:
  a `\r` progress bar arrives once, finished, and colour escapes never, so
  the manager and the completion note read text; the line left unfinished
  arrives at the exit) — tees every byte, as written, to
  `{tasks_dir}/{id}.output` (so the model can `read` interim output), and
  sends `BgEvent::Exited {code, killed}` when the child dies. `BgEvent::Started {id, command, description,
  from_model}` is sent up front.
- `adopt(child, chunk_rx, prior, …)` is the **Ctrl+B transfer**: a
  foreground `bash` (or `!` shell) run hands its child + pipe channel +
  already-read output — the bytes as read — to a fresh monitor thread
  mid-run, which takes `prior` as its first chunk into the same fold as the
  rest, so a bar interrupted mid-line finishes as one line.
- `request_background()` / `take_background_request()` is the Ctrl+B flag: the
  loop sets it when a running `bash`/`!` command is on screen; the runner's
  poll loop consumes it (and clears any stale request when a new command
  starts). At most one command ever runs at a time, so one flag suffices.
- `kill(id)` / `kill_all()` send `SIGKILL` to the process **group** directly
  (synchronously — quit must not depend on monitor-thread scheduling) after
  marking the task killed, so the monitor reports `killed: true`.
- The **completion notice board** (`PendingNotice { context, from_model }`,
  `post_notice`/`take_pending_notices`): the event loop posts a finished
  shell's model-facing note (`BgCompletion::context_text` — the same text the
  settled notice replays into later contexts) the moment it handles the
  `Exited` event. Two consumers race, each taking a note exactly once:
  - the **in-flight agent** — `llm::agent::run_agent` takes the board at the
    top of every round (after its cancel check, so an abandoned turn can't
    steal notes) and appends each note as a user-role message after the prior
    round's tool results, so the model hears "was terminated by a signal"
    right after the tool result of the very `kill` it ran, within the same
    turn (the `LlmBackend::spawn` wiring; the note lands ephemeral in that
    turn's request list exactly where the settled history item replays it for
    later turns);
  - the **turn-boundary dispatch** (`tui::turn::Session::dispatch_after_turn`) — notes
    still on the board at a turn end were never heard by a model: a
    model-launched one (with nothing queued) starts the automatic follow-up
    turn; an agent that already read the note owes no follow-up, so a
    deliberate mid-turn kill no longer triggers a redundant extra turn.
  `/clear` takes-and-drops the board alongside `kill_all()` (a wiped
  conversation owes no phantom follow-up).

Events travel a dedicated tokio channel (`BgEvent`) — a sixth `select!`
source — because background shells outlive turns: the reply channel is
swapped on every interrupt/`/clear`, and these events must survive that.

### Pure state (`app`)

- `App::background: Vec<BackgroundShell>` — the **running** shells (an exited
  shell leaves the list; its notice is the record). Each keeps a tail-capped
  output buffer for the details view and the completion notice; `runtime` is
  boundary-injected per frame (the `set_status_times` pattern — the clocks
  live in `main.rs`).
- `bg_started` / `bg_output` / `bg_exited` apply `BgEvent`s. `bg_exited`
  returns a `BgCompletion`; the loop posts its `context_text()` onto the
  registry board at once, defers the completion (`pending_bg`), and settles
  all pending completions at the **next safe boundary**: every tool
  resolution (`ToolEnd`/`ToolBackgrounded`) and segment-flush point
  (`ToolBatch`/`ToolStart`) mid-turn — the streaming buffer is empty there,
  so a notice cell can never split a committed reply — plus every turn end,
  and immediately while idle. `BgCompletion::context_text` is byte-identical
  to the settled notice's `context_text`, so the note the agent injects and
  the one later contexts replay never diverge.
- At **turn end** the settle is placed *above* the `Done for Ns` summary — a
  completion that landed during the final assistant text (no tool call after
  it) gets the same placement a mid-turn tool boundary would give it, in both
  history and scrollback (invariant 3). `StreamDone` splits the old
  `App::end_turn` into `take_turn_summary` (clear the status → idle height,
  **build** the summary) and `record_turn_summary` (push it), settling the
  held completions between the two: reseat, commit the final reply, settle,
  then record + commit the summary. The `TurnSummary::shells` count is
  unaffected — a finished shell already left `App::background` at `bg_exited`,
  before the count is snapshotted.
- Settling a completion: record `HistoryItem::Background(BackgroundNotice)`
  (the green/red `●` one-liner; the output tail rides the item for the model,
  never rendered). The **automatic turn** is the boundary dispatch's job now:
  at turn end, an untaken **model-launched** note on the board (the agent
  never heard it) with no queued turn dispatching starts one. The notice is a
  history item, so when a queued user batch dispatches instead, the notice
  simply rides that turn's derived context (no extra request). User-launched
  (`!` + Ctrl+B) shells commit the notice only — the model learns mid-turn
  via the board if one is in flight, else on the next turn.
- `TurnSummary::shells` snapshots the running count at `end_turn`, rendering
  `Done for Ns · N shells still running` (not persisted — a resumed session's
  shells are gone).
- `/clear` kills everything (the conversation reset is a hard reset);
  quitting kills everything (no orphan pings).

### The footer's shell indicator, and ↓ focusing it (`App::background_focus`)

The footer's running count (`{model} · {cwd} · {n} shell(s)`) is also the
**entry point**, Claude-Code-style: ↓ does not jump straight into the manager,
it **steps onto the indicator** — that one segment lights up on the palette
cyan (`ui`'s `footer_focus_bg()`/`footer_focus_fg()`) and waits, while every other
footer segment keeps its text *and* its dim styling:

```
────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────
  kimi-k3 medium · ~/Codes/rust/alter-zero · 1.8k/1M (0.2%) · 1 shell
                                                              ▔▔▔▔▔▔▔ cyan
```

- `App::background_focus` is the pure flag (`App::background_focused()` is what
  `ui::footer_line` paints from). It is set by ↓ from an idle composer, gated
  by `background_focusable`: an empty composer, no palette/file-picker/search/
  shell-mode, history recall tried first — **and a shell actually running**.
  The highlight lands *on* the footer's count, so you can only focus an
  indicator that is on screen: with none running ↓ keeps its history-recall/
  cursor meaning and the manager has no hidden keybinding. (That replaced the
  old `had_background` "ever ran this session" gate, which is gone with
  `background_ever()`.)
- While lit, `App::on_key_background_focus` claims a few keys ahead of
  everything else (routed at the top of `on_key`, right after the band, so it
  also wins over the global Ctrl+C/Ctrl+O/Ctrl+D): a **plain Enter** opens the
  manager band, **Esc** / **↑** / **Ctrl+C** dismiss the highlight, a second
  **↓** keeps it — or, with an agent roster listed, steps past the indicator
  into the roster selection (`docs/agent-tool.md`; ↑ from the roster's
  `● main` row steps back onto the indicator the same way, so ↑/↓ walk
  composer ⇄ indicator ⇄ roster symmetrically). Every other key clears the
  highlight and then does its normal job — codex's reset-after-activity, the
  `?` band's rule — so typing dismisses it and types, the newline keys
  (Alt/Shift+Enter, Ctrl+J — `docs/shift-enter.md`) still insert their newline
  rather than opening the band, and a Ctrl+O/Ctrl+D overlay can never be
  returned from onto a stale lit indicator.
- The highlight is dropped by anything that removes the indicator: the **last
  shell exiting** (`bg_exited`), opening the band (`open_background_view`), and
  `/clear`.

### The ↓ manager band (`App::background_view` + `ui`)

An **inline** band that replaces the composer, exactly like the `/model`
picker (never an alternate-screen overlay): `Option<BackgroundView>` with
`List {selected}` and `Details {id}` modes, owning every key while open.

It replaces the composer **only** — never the streaming strip. Opening the
manager mid-turn keeps the running tool's live cell (its streamed tail
included), the spinner status line, the queued messages, and the toast in
their rows *above* the band (a user report: the band used to take the whole
region, so the manager hid exactly the foreground command it sat next to).
`ui::background_view_height` reserves the strip's rows over the band's own
line count, and `render_live_with_preview`'s band branch paints the same
strip the composer path does (the shared `render_strip` helper) with the
band pinned at the bottom — on a terminal too short for both, the band keeps
its full height and the strip is squeezed first. The **three inline pickers**
(`/model`, `/login`, `/settings`) had the same bug and now share this exact
geometry (updated 2026-08-08): the reservation is `ui::layout`'s
`strip_above_rows`, the split `view_split`, the paint `render_strip_above` —
all four views go through them, so none can drift (`docs/llm.md`).
Tool cells still commit
beneath the region as they resolve (commits stay allowed while the band is
open — unless its page is flowing, where they pause like under any other
flowed view and the flow-exit rebuild regenerates them), so the transition
running-cell → committed-cell reads exactly like it does over the composer. One consequence of the band owning every key: the
running cell's delayed `(ctrl+b to run in background)` hint is suppressed
while the band is open — `App::background_hint_elapsed` reads `None`, the
permission-prompt rule — since Ctrl+B would not reach the runner from
inside the band.

- **Enter on the lit footer indicator** opens the list — the only way in, so
  the band never appears without a running shell behind it. It leaves the same
  way: when the **last** listed shell exits *while the band is open*
  (`App::bg_exited`), the band closes and the composer comes back, because a
  manager with nothing left to manage is a dead end — every key it owns
  (`↑/↓`, `Enter`, `x`) has nothing to act on, so the page's only remaining
  purpose is to be dismissed. The renderer keeps a
  `No tasks currently running` page as the defensive fallback for a band
  opened with no shells behind it (`App::open_background_view` is public; the
  app's own entry gate, `background_focusable`, can't reach it).
- List: `Background` title, `{n} active shells`, `❯`-marked selectable rows
  (`{command} (running)`), hints
  `↑/↓ to select · Enter to view · x to stop · Esc to close`.
- Details: `Shell details`, `Status:/Runtime:/Command:` fields — the Runtime
  humanized (`format_elapsed`: `2m 3s`, never a bare `123s`), a `From:
  {type} agent` field when a subagent launched the shell, and the Command
  **word-wrapped** across rows under the value column (`wrap_output`, spaces
  preserved) so a long command line is never truncated away
  (`background_view_height` therefore takes the real width — the band's
  height is width-dependent now) — an `Output:`
  box (rounded `╭─╮` border) tailing the last rows of the live output,
  `Showing N lines`, hints `← to go back · Esc/Enter/Space to close · x to
  stop`. The shell keeps streaming into the box (the bg channel schedules
  frames; the draw tick re-arms ~30fps while the band is open so `Runtime`
  ticks).
- **A page taller than the terminal flows its top into real scrollback**
  (`docs/view-flow.md`). The band is bottom-anchored like every framed view,
  so its interactive tail — the hints, the closing rule — always stays on
  screen; the rows the anchor skips used to be dropped into **no buffer at
  all**, which on a 24-row terminal ate the details page's top rule and left
  the conversation running straight into a headless box (the reported bug:
  *"it removes the line texts when I scroll up"*). They are committed above
  the live region now, so the terminal's own scrolling reads the page whole.
  The details page is the one page in the TUI that changes **between**
  keystrokes — `Runtime` ticks, the output box fills — so its flow is signed
  on the shell's id (`ui::view_flow`'s `FlowSign::Frozen`) rather than on its
  rows: the flowed top **freezes** in scrollback where it was committed,
  instead of re-signing the flow and purge-rebuilding the screen at the
  band's own ~30 fps. A resize, `←` back to the list, or a different shell
  re-signs it and the rebuild re-flows the page. Nothing about what the band
  *draws* changes — the anchor, the fields, the box and the hints are exactly
  as they were; the fix is only where the skipped rows go. The list page
  changes on a keystroke, so it signs its rows like the menus do.
- `x` returns `Action::KillBackground(id)`; the loop kills via the registry
  and the resulting `Exited` event removes the row (details falls back to the
  list; stopping the last shell closes the band — see above).
- The footer gains a running count: `{model} · {cwd} · {n} shell(s)` — which
  doubles as the band's entry point (see above).

### Ctrl+B

The running `bash` cell's live preview appends a dim
`(ctrl+b to run in background)` row (live-only — never committed); the
running `!` shell cell gets the same row. The hint is **delayed**,
Claude-Code-style: it appears only once the command has been running for
`ui::TOOL_BACKGROUND_HINT_DELAY` (3s), so a command that finishes right away
never flashes it (it isn't needed for a fast command). The gate is the
command's **own** elapsed — `App::background_hint_elapsed`, boundary-injected each
frame from `StatusClocks::command_start` (the `set_status_times` pattern): a
model `bash` call's clock starts at its `ToolStart`, a `!` shell run's in
`run_shell`, cleared at the command's resolution and each turn start. Turn
elapsed won't do — a model tool can start deep into a turn. The same clock
is what the running cell **displays** — the `bash` cell's `+N lines (Ns ·
timeout …)` clock row and the `!` shell's `⎿ Running… (Ns)` row read it through the plain
`App::command_elapsed`, what `set_command_elapsed` set; `background_hint_elapsed`
is that value *masked* for the hint's one job: the band and every
composer-replacing picker blank it (they swallow the key) while keeping the
running cell on screen above themselves, where its footer must go on
counting (`docs/tool-streaming.md`, *Whose clock the footer shows*). **Ctrl+B itself
works the whole time** (`can_move_to_background` is ungated); only the
discoverability hint waits, so pressing it on a still-fresh command still
backgrounds it. Ctrl+B returns
`Action::MoveToBackground` when a running command is on screen; the loop sets
the registry flag; the runner (executor `run_bash` or `spawn_shell_command`)
consumes it mid-poll, `adopt`s the child, and resolves the call as
backgrounded — the model bash via `ToolBackgrounded` (the agent loop keeps
going with the **handoff text** as the tool result — the user-moved preamble
over the launch facts, see Protocol above), the `!` shell via its normal
`ToolEnd`+`StreamDone` with the cell resolving to the backgrounded row.

### Interactive sessions

A `bash` call with `tty` runs in a pseudo-terminal and is a task of this same
registry (`launch_tty`, `docs/interactive-shell.md`) — the footer count, the ↓
manager, `kill_all` and subagent attribution all hold — with four
differences. It is **announced** (`BgEvent::Started`) only if it outlives its
launching call, so a command that finishes inside its call never shows up
here. Its monitor sends `BgEvent::Screen` (the emulated screen, throttled)
instead of line output, which the details page shows in place of a tail. An
exit the model *saw* — a `bash_session` result framed `Exit code: N` or
`Stopped` — arrives as `Exited { observed: true }`, which posts no completion
notice and starts no follow-up turn. And at most `MAX_TTY_SESSIONS` (16) run
at once. Ending one kills its process group and everything left in its
session.

### Persistence (`session`)

- `ToolRecord` gains `backgrounded: bool` (skipped when false — old files
  parse unchanged) mapping to `ToolStatus::Backgrounded`.
- A new `background` line record round-trips `BackgroundNotice`. Old builds
  skip unknown record types (the forward-compatibility contract).

## Invariant notes

- Background shells are **turn-independent**: Esc-interrupt, `fail_stream`,
  and channel swaps never touch them (their events ride their own channel).
- Completion notices are committed only at **safe boundaries** — tool
  resolutions / segment flushes (where the streaming buffer is empty) and
  turn ends — never inside a streaming text run: a foreign cell inside a
  streaming reply would corrupt the committed-row order (invariant 2/3). A
  completion that lands mid-text therefore waits for the next such boundary
  (its board note still reaches the agent immediately). Commits happen only
  in the conversation view (invariant 4); history always records them, so
  overlay returns/resizes repaint them.
- The board and `pending_bg` are parallel queues over the same completions —
  the board feeds the **model** (taken by the agent or the boundary
  dispatch), `pending_bg` feeds the **TUI/history** (drained at settle
  points). A note can reach the model before its cell commits (the agent
  drains between the loop's settle points) and vice versa; both always land,
  each exactly once.
- The band is inline state, not a `View` — all four alternate-screen views
  and the resize repaint behave exactly as before. It displaces the composer
  but not the streaming strip: a running tool's preview, the status line,
  the queue, and the toast render above it from the same helpers the
  composer path uses, so the strip's geometry (`preview_rows`,
  `strip_has_status`) stays the single source of truth for both.
- The footer highlight is **not a mode**: it claims only a plain
  Enter/Esc/↑/↓/Ctrl+C and lets every other key through (after clearing
  itself), so no keystroke can be stranded on it and no existing key loses its
  meaning — the newline keys included. It also changes no geometry — the
  footer row it paints is the one `ui::footer_rows` already reserved, so
  `live_height`, the cursor seat, and the scrollback commits are untouched.

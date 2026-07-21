# Background shells — `run_in_background`, Ctrl+B, and the ↓ manager

Claude-Code-style background command execution: the model can launch a `bash`
command that keeps running while the conversation continues, the user can move
a running command to the background with **Ctrl+B**, and a **↓ manager band**
(opened from an empty composer) lists the running shells, streams a selected
shell's output live, and can stop one. When a background shell finishes, the
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
  `output` is the *model-facing* text (task id + interim-output file path); the
  cell never shows it — it renders the fixed
  `⎿ Running in the background (↓ to manage)` row.
- That text differs by **who** backgrounded the call. A `run_in_background`
  launch gets the plain acknowledgement (`exec::background_launch_text` — the
  model asked, so the id + interim path + notification promise suffice). A
  **Ctrl+B handoff** gets `exec::background_handoff_text`: the same facts led
  by `The user moved this command to the background …` and closed with a
  don't-re-run/don't-poll steer — the model requested a *foreground* run, and
  without being told the user moved it, it expects the full output and
  re-reads the interim file round after round waiting for it. The recorded
  `tool.output` is this same text, so `context::context_messages` replays the
  explanation into every later turn's context too. The `bash` tool
  description also warns the model up front that the user may background a
  running command mid-run.
- `ToolStatus::Backgrounded` is the resolved status: green header bullet, the
  fixed row as its body inline, in the preview, and in the Ctrl+O transcript.
  The wire/tool-result content in the derived context stays `tool.output`
  (`context::context_messages` needs no special case).

### The registry (`src/background.rs` — boundary, like `term`)

`BackgroundRegistry` is a cloneable handle shared by the event loop, the
executor (`llm::exec`), and the `!` shell runner:

- Task ids are **Claude-Code-style** — a `b` prefix + 8 lowercase base36
  chars (`bvyo7tkbe`), rolled per launch from a splitmix64-mixed entropy seed
  (nanos ⊕ pid ⊕ a launch counter — `main.rs::session_id`'s no-`rand`
  pattern) with a collision re-roll against the running set. The pure
  `task_id(seed)` pins the format.
- The interim files live in **Claude Code's tasks layout** — the pure
  `tasks_dir(temp, uid, cwd, session)`:
  `{tmp}/alter-zero-{uid}/{cwd, non-alphanumerics dashed}/{session}/tasks/{id}.output`
  (e.g. `/tmp/alter-zero-0/-home-user-proj/18f…-4e2/tasks/bvyo7tkbe.output`) —
  a stable per-user root, the project cwd as one dashed segment, and a
  per-session dir keeping concurrent instances apart. The boundary injects
  the uid (`main.rs::process_uid` — `/proc/self`'s owner; no `libc` in a
  `forbid(unsafe)` crate), cwd, and session id.

- `launch(command, description, from_model)` spawns `sh -c` in its own process
  group **detached from the controlling terminal**
  (`subprocess::spawn_detached_shell` with the registry's detach helper — a
  `/dev/tty` password prompt fails fast instead of wedging the task; the
  registry also *carries* that helper to the executor and the `!` runner,
  `docs/tools.md`) and a **monitor thread** that merges
  stdout/stderr in arrival order, streams completed lines as
  `BgEvent::Output`, tees everything to `{tasks_dir}/{id}.output` (so the
  model can `read` interim output), and sends `BgEvent::Exited {code, killed}`
  when the child dies. `BgEvent::Started {id, command, description,
  from_model}` is sent up front.
- `adopt(child, chunk_rx, combined, forwarded, …)` is the **Ctrl+B transfer**:
  a foreground `bash` (or `!` shell) run hands its child + pipe channel +
  already-read output to a fresh monitor thread mid-run.
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
  - the **turn-boundary dispatch** (`main.rs::dispatch_after_turn`) — notes
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

### The ↓ manager band (`App::background_view` + `ui`)

An **inline** band that replaces the composer, exactly like the `/model`
picker (never an alternate-screen overlay): `Option<BackgroundView>` with
`List {selected}` and `Details {id}` modes, owning every key while open.

- **↓ from an empty composer** (no palette/file picker/search/shell-mode, not
  recalling history) opens the list once any shell has ever run
  (`had_background`); the empty state shows `No tasks currently running`.
- List: `Background` title, `{n} active shells`, `❯`-marked selectable rows
  (`{command} (running)`), hints
  `↑/↓ to select · Enter to view · x to stop · Esc to close`.
- Details: `Shell details`, `Status:/Runtime:/Command:` fields, an `Output:`
  box (rounded `╭─╮` border) tailing the last rows of the live output,
  `Showing N lines`, hints `← to go back · Esc/Enter/Space to close · x to
  stop`. The shell keeps streaming into the box (the bg channel schedules
  frames; the draw tick re-arms ~30fps while the band is open so `Runtime`
  ticks).
- `x` returns `Action::KillBackground(id)`; the loop kills via the registry
  and the resulting `Exited` event removes the row (details falls back to the
  list; the list falls back to the empty state).
- The footer gains a running count: `{model} · {cwd} · {n} shell(s)`.

### Ctrl+B

The running `bash` cell's live preview appends a dim
`(ctrl+b to run in background)` row (live-only — never committed); the
running `!` shell cell gets the same row. The hint is **delayed**,
Claude-Code-style: it appears only once the command has been running for
`ui::TOOL_BACKGROUND_HINT_DELAY` (3s), so a command that finishes right away
never flashes it (it isn't needed for a fast command). The gate is the
command's **own** elapsed — `App::command_elapsed`, boundary-injected each
frame from `StatusClocks::command_start` (the `set_status_times` pattern): a
model `bash` call's clock starts at its `ToolStart`, a `!` shell run's in
`run_shell`, cleared at the command's resolution and each turn start. Turn
elapsed won't do — a model tool can start deep into a turn. **Ctrl+B itself
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
  and the resize repaint behave exactly as before.

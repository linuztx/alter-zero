# The task tools — a live checklist instead of tool cells

Claude Code's structured task list (`TaskCreate` / `TaskGet` / `TaskList` /
`TaskUpdate`), ported to the inline TUI: the model plans multi-step work as
tasks, marks them `in_progress`/`completed` as it goes, and the user watches a
**live checklist** under the status line instead of a stream of tool-call
bullets.

```
(  ●•· ) Setting up project structure… (25s · ↓ 1.2k tokens · esc to interrupt)
  ⎿  ◼ Set up project structure
     ◻ Write core logic › blocked by #1
     ◻ Add tests › blocked by #2
```

## What the model gets

Four function tools, offered only when the boundary attached a
[`TaskRegistry`] (`LlmBackend::with_tasks` — the `with_ask` pattern: without a
registry nobody holds the list, so the tools aren't there to call). Subagents
never get them — the lead agent plans, subagents execute — and the one-off
`/compact` backend doesn't either. Wire names are lowercase like every other
tool (`taskcreate`, …), display names CamelCase (`TaskCreate`), which is what
lets the context replay's lowercasing fallback reproduce the wire name.

The schemas are Claude Code's **minus `owner` and `metadata`**: `owner` is a
multi-agent-swarm concept (agents claiming tasks by name) and this session has
exactly one agent driving the list, and `metadata` is an arbitrary blob nothing
in this TUI consumes. Removing them rather than accepting-and-ignoring keeps
the schema honest — what the model can send is what the feature does.

- `taskcreate` — `subject` (required), `description` (required), `activeForm`
  (optional; the present-continuous label the spinner wears while the task is
  in progress). Result: `Task #3 created successfully: {subject}`.
  Dependencies are `taskupdate`'s job and stay out of this schema, but a live
  model folds them into the create anyway (seen with gpt-4o-mini), in either
  the `addBlocks`/`addBlockedBy` spelling it just read there or the bare
  `blocks`/`blockedBy` — so the parser **honours all four** rather than
  silently dropping them, wiring the edges as `taskupdate` would. They are
  validated before the task is created, so a dependency on a missing task
  fails the call whole (`Task #9 not found`, nothing created) instead of
  leaving a half-linked row.
- `taskget` — `taskId`. Result: `Task #3: {subject}` / `Status: …` /
  `Description: …` (+ `Blocked by: #2` / `Blocks: #4` when linked).
- `tasklist` — no parameters. Result: one `#3 [pending] {subject}` line per
  task, `[blocked by #2]` naming only **open** blockers; `No tasks found` when
  empty.
- `taskupdate` — `taskId` plus any of `subject`/`description`/`activeForm`/
  `status` (`pending`/`in_progress`/`completed`/`deleted`)/`addBlocks`/
  `addBlockedBy`. Result: `Updated task #3 {changed fields}`; unknown id:
  `Task #9 not found`. `deleted` removes the task permanently and scrubs it
  from every other task's dependency list.

All the texts above are Claude Code's own result strings, so a model trained on
them is at home. Task ids count up from a **high-water mark** that survives
deletion (`TaskStore::next_id`): after tasks #1–3 are deleted the next create
is #4, proving the old ones are really gone rather than renumbered.

## Hidden cells, visible checklist

Claude Code renders **nothing** for these tool calls (its
`renderToolUseMessage()` returns `null`) — the live checklist *is* the
feedback, and a five-task plan would otherwise commit ten-plus `● TaskUpdate`
cells of pure noise. We do the same, but keep the record everywhere it
matters:

- **Inline scrollback: nothing.** A task call commits no cell, and the round's
  narration text before it still finalises as its own `●` message (the
  `ToolStart` flush dance), so each round's prose reads as its own bullet —
  Claude Code's exact rhythm.
- **The strip: the checklist.** While a turn is active (`strip_has_status`)
  and the list is non-empty, the rows render directly **under the status
  line**, above its trailing gap: `⎿ {glyph} {subject}` in the tool gutter.
  `◻` pending (dim), `◼` in progress (the glyph in the system cyan, the
  subject bright), `✔` completed (green glyph, dim struck-through subject),
  and a dim `› blocked by #1, #2` suffix naming a blocked task's **open**
  blockers (a completed or deleted blocker drops out on the next render).
  Subjects truncate to one row (ellipsis) like the footer; past
  [`TASK_MAX_ROWS`] the list prioritises in-progress → unblocked pending →
  blocked → completed and folds the rest into a dim `… +N pending, M done`
  tail row.
- **At rest: the standalone block.** A plan with work left doesn't vanish
  when the turn ends — the same rows sit above the composer under Claude
  Code's dim count line, so what remains is in view while the user reads
  and types:

  ```
    1 tasks (0 done, 1 open)
    ◻ Review demo output with user
  ```

  Same glyphs, same truncation, same fold (`task_rows_lines` builds both);
  only the prefix differs — the `⎿` gutter hangs off a spinner, and at rest
  there is none, so the rows take the composer's own two-space inset. The
  count line adds an `N in progress` clause when any task is running, and
  its three numbers **partition** the list — `open` is the not-yet-started
  tasks alone, the `pendingCount` Claude Code prints there, so a running
  task is reported once rather than counted as both in progress *and* open
  (`3 tasks (1 done, 1 in progress, 1 open)` on a list of three, never
  `2 open`).
- **A finished plan retires.** Once every task is completed the list has
  done its job: the turn that finished it keeps showing the all-green rows
  (the payoff), and at that turn's end it is **dropped whole**
  (`App::retire_finished_tasks` → `TaskStore::retire_if_finished`, called
  from the loop's `dispatch_after_turn`, which re-syncs the shared registry
  so the model's next `tasklist` agrees). So it is *gone*, not hidden: no
  resting block, nothing on later turns, and — the reason this matters —
  the next `taskcreate` starts a genuinely new plan instead of appending a
  row to three old ticks. The **id high-water mark survives**, so that new
  plan opens at `#4`: the same proof of continuity deletion gives. A rewind
  (`/resume`, the Esc-Esc backtrack) applies the rule to the snapshot it
  restores, since a rewind lands between turns too.
- **The spinner wears the active task.** While some task is `in_progress`,
  the status verb is the **first** such task's `activeForm` (falling back to
  its `subject`) instead of the turn's whimsical verb —
  `✻ Setting up project structure…` — Claude Code's spinner rule
  (`currentTodo.activeForm ?? currentTodo.subject ?? randomVerb`). Derived at
  render time (`App::task_verb` → `ui::status_line_with_verb`), never stored,
  so completing the task snaps the verb back mid-turn.
- **Ctrl+O: the full record.** The transcript view lists every task call as
  an ordinary tool cell (`● TaskCreate(Set up project structure)` over its
  `⎿` result gutter) — the debugging surface hides nothing.
- **Ctrl+D / the model's context: native replay, verbatim.** Each record
  replays as a provider-native `tool_calls` + `tool` result pair
  (`context::context_messages`) — under the provider's own call id
  (`TaskCallRecord::call_id`, taken from the round's announcement in the
  model's order) and in the round's one `tool_calls` message beside the
  calls it was made with (`docs/prompt-caching.md`) — carrying the **raw
  arguments the model sent** — `TaskCallRecord::arguments`, kept beside the
  header summary precisely so the replay can. The summary is lossy by design (`#1 →
  completed` is a header, not a payload), and replaying *that* would leave a
  later turn reading `taskupdate {}` over a result line, its own subjects,
  descriptions and dependency wiring gone from the conversation. A record
  written before the field existed replays as `{}`, exactly as it used to.

## How it flows

One new event carries the whole resolution — a task call is instant, needs no
permission, and streams no output, so the `ToolBatch`/`ToolStart`/`ToolEnd`
trio would only exist to be filtered back out:

```
StreamEvent::TaskCall { name, args, arguments, output, ok, tasks }
```

`args` is the one-line header summary the Ctrl+O cell shows; `arguments` is
the raw JSON the model sent, which the record keeps for the context replay
(see above).

`llm::agent::run_agent` routes task calls the way it routes `agent` calls:
they are excluded from the `ToolBatch` announcement (a round of only task
calls announces nothing), skipped by the permission seam, executed through the
ordinary `execute` closure — whose outcome carries the post-call snapshot on
`ToolOutcome::tasks` — and emitted as one `TaskCall` each, in the model's call
order, with the result text feeding back as the `tool` message like any other
call. A task call refused by the **Max tool calls** ceiling is answered with
the limit text and emits nothing (the refused-`agent`-call rule: no cell, just
the answer).

The executor half is `llm::task::run_task_tool`: parse the args, apply the op
to the shared [`TaskRegistry`] (`Arc<Mutex<TaskStore>>`, the
background-registry sibling), and hand back the split outcome. All the
semantics — id assignment, field-diff updates, dependency wiring, deletion
scrubbing, every result string — live in the pure [`crate::tasks`] module,
unit-tested without a terminal.

The loop's `TaskCall` arm mirrors `ToolBatch`: flush the streamed segment
(each round's text becomes its own bullet), settle held background
completions at the safe boundary, then `App::record_task_call` — which
appends the hidden `HistoryItem::TaskCall` record, stores the snapshot on
`App::tasks` for the strip, and charges the result text to the token tally
(`↑`, the uploaded-back arrow) exactly like a visible tool result.

## Keeping the list current — the guard

A checklist is only worth watching if the model keeps it true, and a small
model doesn't. Measured with gpt-oss:120b on Ollama Cloud, asked to *create a
facebook login page UI/UX and expose it to port 3000*: one `taskcreate`,
eleven rounds of `bash`/`write`/`read`, and the task never marked
`in_progress` — completed in the last call, or left open. The tool
descriptions already say to mark a task in progress before its work and
completed right after; the model reads them once, at the top of a long
context, and forgets.

Claude Code answers the same forgetting with a reminder: after ten assistant
turns without a `TaskCreate`/`TaskUpdate`, the list goes into the
conversation inside a `<system-reminder>`. Ten rounds is longer than the
whole job takes a small model, so the reminder never fires while it matters.
The guard keeps that wait and adds the two checks it lacks — the lapse the
moment it happens, and the moment that would freeze the list wrong:

| Nudge | When | What the model reads |
| --- | --- | --- |
| **Idle** | at a round boundary: open tasks, none in progress, and work (any non-task tool) since the last create/update | `None of these tasks is in progress. If you are working on one, mark it in_progress with taskupdate, and mark each one completed as soon as it is done.` |
| **Stale** | at a round boundary: a task in progress, but [`TASK_REMINDER_ROUNDS`] (10, the reference's number) rounds of work without a create/update | `These tasks have not been updated in a while. Mark finished ones completed and the one you are working on in_progress with taskupdate.` |
| **Closing** | the model answers with plain text while open tasks remain that this turn created or updated (or any task in progress), and the turn did work | `You are ending your turn with these tasks still open. Mark each one you finished completed with taskupdate and leave the rest open. Do not repeat your answer …` |

Each text closes on `Do not mention this reminder to the user.` and is
followed by the list exactly as `tasklist` prints it (`#1 [pending] Write the
page`), the whole wrapped in `<system-reminder>` tags — the shape the
project's other reminders and the reference's both use. Short on purpose: it
rides the conversation for good (see below), and an instruction a small model
can act on is one sentence, not a paragraph.

The rules that keep it from nagging, each pinned by a test in
`src/tasks/guard.rs`:

- **Making the plan is not a lapse.** A round of `taskcreate`s, or of
  `tasklist`/`taskget` reads, is not work; a turn that only plans (and ends
  asking the user to confirm) hears nothing.
- **The order inside a round counts.** `write` then `taskupdate` is a list
  kept current; `taskupdate` then `write` has worked since.
- **One round reminder per [`TASK_REMINDER_ROUNDS`] while ignored — but a
  model that acts on one starts fresh.** Any create/update resets the gap, so
  the next lapse (task #1 completed, work going on, #2 never started) is
  caught at once, while a model ignoring the reminder isn't told again every
  round.
- **A turn that never touches the list hears one round reminder at most.**
  The plan may be an earlier turn's and the request something else entirely;
  the conditional wording (`If you are working on one …`) is safe there, and
  once is enough.
- **The closing reminder fires once per turn**, and only for a turn that did
  work and either created/updated tasks or has one in progress (an
  in-progress task is current work, whoever started it). It fires *even when
  the turn's last call was a `taskupdate`*: marking the task just finished
  says nothing about the others the same work finished (#1 completed last,
  while #2's work was done too and it still says pending).

### Where it plugs in

The guard (`tasks::TaskGuard`) is pure: the loop feeds it each round's call
names and the current list, and sends what it returns. The loop is
`llm::agent::run_agent_with_tasks` — `run_agent` plus an
`Option<&TaskRegistry>`; the backend's main turn passes its list, while the
subagents and every `run_agent` caller pass none and run exactly as before.
The guard is fresh per turn, so it counts this turn's rounds only.

- **The round reminder** goes in at the top of a round: after the background
  notices (results, like the tool results they follow) and **before** the
  user's own queued messages, which stay the last thing the model reads
  (`docs/queue.md`).
- **The closing reminder** answers `RoundOutcome::Complete`: the reply so far
  becomes an assistant message, the reminder the next user message, and the
  same turn runs one more round — the `Stop` hook's continuation shape
  (`docs/hooks.md`), but ahead of the hook, so the hook hears the answer the
  turn really ends on, and without touching `stop_hook_active`, which is the
  hook's own loop guard. Never after an Esc.
- **No reminder once the Max tool calls budget is spent** — it could only ask
  for a call the budget refuses, turning a finished turn into an error.

Each reminder is announced as a `StreamEvent::HookNote` labelled `Task
reminder` — the event the hooks already use for conversation text the loop
adds — so it is recorded as a cell-less `HistoryItem::HookNote`: invisible
inline (the checklist is the visible record), shown in Ctrl+O under its
label, round-tripped by the rollout, and replayed by `context_messages` at
exactly the position the model read it, which keeps every later turn's
request a byte-stable extension of this one (`docs/prompt-caching.md`).

Measured live with gpt-oss:120b on Ollama Cloud, through the probe below and
the TUI itself: before the guard, the one run in four that made a plan never
marked its task in progress. With it, in all ten runs that made a plan and
started work, the Idle reminder came after the first round of work and the
model's very next call marked a task `in_progress`; from there it kept the
list moving, completing tasks and starting the next. Twice the closing
reminder caught a turn about to end with the job half-done: the model marked
what it had finished, saw what was left, and carried on. (Ollama Cloud cut
most of those runs short with HTTP 500s on the longer conversations, at the
same rate with or without a reminder in the request.)

Two alternatives were considered and dropped. A rule in the system
prompt is read once, at the top, which is the reading that was already
failing. Re-sending the list on every round would cost tokens on every
request and teach the model to skim it; the guard speaks only when the list
is going wrong.

`examples/task_probe.rs` drives the real agent loop against a live model the
way the TUI does and prints every task change and every reminder, then a
tally of what each task went through:

```bash
PROVIDER=ollama_cloud MODEL=gpt-oss:120b OLLAMA_API_KEY=… \
  cargo run --example task_probe -- /tmp/fb "Create a facebook login page UI/UX and expose it to port 3000"
```

`tests/task_guard_wire.rs` proves the backend wiring end to end against a
provider stand-in on the loopback: the list arrives after the work, then once
more before the turn may end.

## The record and the three rewinds

`HistoryItem::TaskCall(TaskCallRecord)` keeps `{name, args, output, ok,
timestamp, tasks}` — the **post-call snapshot rides every record**, which is
what makes all three history rewinds exact without replaying arguments:

- **`/resume`** — the rollout serialises each record
  (`session::ItemRecord::TaskCall`; old builds skip the unknown type, the
  forward-compatibility contract), and a load rebuilds the list from the
  *last* record's snapshot (`tasks::latest_snapshot`) — the high-water
  `next_id` included, so a resumed session's ids keep counting.
- **Esc-Esc backtrack** — rewinding to an earlier user message truncates
  history; the list resets to the last snapshot **before the cut** (the state
  the conversation actually had there).
- **`/clear`** — wipes the list with everything else.

In each case the boundary syncs the shared registry to the app's snapshot
(`tui`'s `sync_task_registry`), so the model's next `tasklist` agrees with
the strip.

## The offline demo

The dummy plays a `todo`/`task` prompt as a scripted lifecycle
(`stream/dummy/turns.rs::tasks_turn`): the turn function drives a **real**
`TaskStore` and emits the genuine outputs and snapshots — three creates, the
dependency wiring, `in_progress` → `completed` — so the offline checklist,
spinner override, and Ctrl+O cells are byte-for-byte what the live executor
produces, and it closes on the shared `handoff!()` sentence like every other
scenario. It ends with **work outstanding** (#1 done, #2 running, #3 pending),
which is what makes the resting block and the cross-turn list drivable
offline — and `smoke.sh` Phase 69 asserts exactly that.

The other ending has its own scenario: a todo prompt that also says **finish**
(`tasks_finished_turn`) walks two tasks to all-✔ and stops there, so the
retirement is drivable too — the closure shows to its own turn's end, nothing
shows at rest, and the next turn's spinner comes up clean (`smoke.sh` Phase 70,
the reported stale-list bug). Splitting it in two rather than extending the
first demo keeps each one's ending unambiguous: a single script can only
demonstrate one of them.

[`TaskRegistry`]: ../src/tasks.rs
[`crate::tasks`]: ../src/tasks.rs
[`TASK_MAX_ROWS`]: ../src/ui/theme.rs
[`TASK_REMINDER_ROUNDS`]: ../src/tasks/guard.rs

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

## The guard — keeping the list current

Creating a plan is the half every model manages. Ticking it off is the half
a weaker one forgets: gpt-oss:120b (Ollama Cloud), asked to *create a
facebook login page and expose it on port 3000*, created its task, worked
through eleven tool calls, and ended the turn on a list still reading
`◻ pending` — the spinner never wore the task and the resting block claimed
nothing had been done. The schemas already say when to update (`taskupdate`:
*in_progress before you start, completed immediately after*); what such a
model lacks is the list **in front of it** at the moment it falls behind.

So the main session's agent loop carries a guard (`tasks::TaskGuard`, pure,
consulted by `llm::agent::run_agent_guarded`) that watches each round's calls
and, when the list has fallen behind the work, shows it to the model again:

- **Progress reminder** — at a round boundary, once tool work has gone
  untracked: after **one** round of work when no task is `in_progress` (it
  started working without starting a task — the slip above), after
  `TASK_REMINDER_ROUNDS` (3) rounds with one running — but that stale case
  only for a model that has already needed a reminder this turn. One that
  starts its own tasks is trusted to tick them: gemma4 started and ticked
  every task itself and still drew a reminder three rounds into its last
  one, while gpt-oss always trips the first reminder and keeps the stale
  ones that helped it. Once per stretch of untracked work — a model still
  working after the reminder is most likely still on its task (one coarse
  task, a long debugging run): repeating it every three rounds sent one live
  run four reminders it rightly ignored — and never two within three rounds
  of work.
- **Closing reminder** — when the model **answers** with open tasks after
  work it never ticked off, or after a turn of work with a task still
  `in_progress` (seen live: the work done, the task marked in progress as the
  last act, the answer saying *completed once you confirm*). Once per turn;
  after that the answer stands.
- **Silent stop** — when the model ends its turn with open tasks and **no
  answer at all**. gpt-oss does this mid-plan, several times a turn: a plan
  created and nothing more, or one task ticked and silence. An empty answer is
  no answer, so it is sent back to work — again each time it stops silently,
  for as long as each nudge buys progress (work, or a list update), with
  `TASK_SILENT_STOP_LIMIT` (8) behind that as the backstop. A turn that did
  nothing at all is left alone: an old plan left open is not its to restart.

The closing reminder and the silent-stop nudge both continue the **same
turn** — the answer, if any, stands as a message, the reminder follows it,
and the model gets one more round: the `Stop` hook's continuation
(`docs/hooks.md`), minus the hook, and asked **before** the hooks so a `Stop`
hook judges the answer the turn actually ends on.

What counts: a successful `taskcreate`/`taskupdate` keeps the list current
(and starts a new stretch); any other call — a `bash`, a `write`, an agent
launch — is work; `tasklist`/`taskget` are neither, and a refused update
(`missing field taskId`) is not tracking. With no open task — no plan, or a
finished one — the guard has nothing to say, so a model that keeps its list
current never meets it. A turn that only plans (`plan this for me`) and
answers is left alone too.

### The texts

Short and direct, in the `tasklist` shape the model already reads its list
in, wrapped as a `<system-reminder>` so it reads as the session speaking
rather than the user:

```
<system-reminder>
Your tasks, none in_progress:
#1 [pending] Initialize project
#2 [pending] Create login UI

Use taskupdate: completed for each task you have finished, in_progress for the one you are working on.
</system-reminder>
```

(`Your tasks:` when one is running.) The advice names **finished** tasks
first because the first wording — *mark the one you are working on
in_progress* — had gpt-oss mark a task it had just finished `in_progress` and
stop there. The closing reminder lists only the open tasks:

```
<system-reminder>
You are finishing with open tasks:
#2 [in_progress] Create login UI

Use taskupdate first: completed for each finished task, deleted for any no longer needed. If work remains, do it or say what is left.
</system-reminder>
```

and the silent-stop nudge asks for the work, not the bookkeeping — under the
closing reminder's wording a silent gpt-oss ticked a task it had not done:

```
<system-reminder>
You stopped without an answer. Open tasks:
#3 [pending] Create UI files

Continue: mark the task you work on in_progress, do it, then mark it completed.
</system-reminder>
```

### Where it lands

Each reminder is a user-role message at the **frontier** — after the round's
tool results and any background notices, before any message the user queued
(the user's own words stay the last thing the model reads) — so nothing the
provider cached moves (`docs/prompt-caching.md`). It is announced as a
`StreamEvent::HookNote` labelled `Task reminder`, which is exactly what it
needs to be: the loop records the cell-less `HistoryItem::HookNote`
(invisible inline, under a dim `● Task reminder` heading in Ctrl+O), the
derived context replays it **verbatim** on every later turn — so the next
turn's request is the cached one, Ctrl+D shows what the model read — and the
rollout round-trips it, so a `/resume` keeps it. No new event or history kind:
it is conversation text the harness injected mid-turn, which is what a hook
note already is.

Only the main session has a guard: it is built per turn from the shared
`TaskRegistry`'s snapshot (so a plan an earlier turn left open is watched
too) wherever the task tools are offered. Subagents and the `/compact`
backend never carry the tools, run the unguarded `run_agent`, and are
untouched.

### Measuring it

`examples/task_probe.rs` drives the real agent loop with the task list
attached — the real system prompt, a background registry, uncapped like the
app — and prints every task call with the checklist it left, every reminder
the guard sent, and the final list (`SHOW_THINKING=1` adds the model's
reasoning, which is where gpt-oss says *we need to mark task 1 as
in_progress*):

```sh
PROVIDER=ollama_cloud MODEL=gpt-oss:120b OLLAMA_API_KEY=… \
  cargo run --example task_probe -- /tmp/work "Create a facebook login page UI/UX and expose it to port 3000"
```

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

# Lifecycle hooks

Claude Code and codex both let a user wedge their own command into the agent's
lifecycle: run a linter after every `edit`, refuse a `bash` command that
matches a pattern, feed the model extra context when a session opens. This is
that feature, ported to this codebase.

```
● Bash(rm -rf build/)
  ⎿  Blocked by PreToolUse hook: no destructive deletes outside ./tmp
     Hook: ~/.alter-zero/hooks/no-deletes.sh
     Config: ~/.alter-zero/hooks.json
```

The **`/hooks`** command opens a read-only inline browser over this
configuration — which events have hooks, under which matchers, running what —
see `docs/tools/hooks-menu.md`.

## The contract

codex ships a `codex-rs/hooks` crate whose engine type is literally named
`ClaudeHooksEngine`: it implements **Claude Code's** hook wire contract rather
than a novel one. We target that same contract, so a hook script written for
either tool works here unchanged.

Config is a `hooks.json` whose shape is event → matcher groups → handlers:

```json
{
  "description": "optional, ignored",
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "bash|write",
        "hooks": [
          { "type": "command", "command": "./guard.sh", "timeout": 60 }
        ]
      }
    ]
  }
}
```

A handler receives its event payload as **JSON on stdin** and answers with
**JSON on stdout**. The two halves are spelled differently, and both
references agree on the asymmetry, so we copy it exactly:

- **stdin (the payload) is `snake_case`** — `session_id`, `transcript_path`,
  `cwd`, `permission_mode`, `scratchpad_dir`, `hook_event_name`, `tool_name`,
  `tool_input`, `tool_use_id`, `agent_id`, `agent_type`. `session_id` is the
  **conversation's** id — the rollout file's, the handle `--resume` takes —
  so a hook that logs it can hand it back to `alter-zero --resume`. It is
  known before the file exists (a fresh conversation has its id from its
  first event), moves on at a `/clear`, and becomes the resumed file's at a
  `/resume`; `transcript_path` moves with it. The first conversation's file
  is named by the session's startup id, so until a `/clear` the id is also
  the temp tree's (`docs/tools/scratchpad.md`) — but `scratchpad_dir` is how a
  handler finds the session's scratchpad and task output *after* one, which
  is Claude Code's field for it. (`session_id` used to be the process's id,
  which a `/clear` never moved and `--resume` could not take.)
- **stdout (the verdict) is `camelCase`** — `continue`, `stopReason`,
  `suppressOutput`, `systemMessage`, `decision`, `reason`,
  `hookSpecificOutput.{hookEventName,permissionDecision,
  permissionDecisionReason,updatedInput,additionalContext}`.

Fourteen events are modelled. What each may return:

| event                           | the hook may return                                   |
| ------------------------------- | ----------------------------------------------------- |
| `PreToolUse`                    | allow / deny / ask · block + reason · `additionalContext` · `updatedInput` |
| `PermissionRequest`             | `allow` / `deny` + message                             |
| `PostToolUse` / `PostToolUseFailure` | block + reason · `additionalContext`              |
| `UserPromptSubmit`              | block + reason · `additionalContext`                   |
| `Stop` / `SubagentStop`         | block + reason (make the agent keep going)             |
| `SessionStart` / `SubagentStart`| `additionalContext`                                    |
| `SessionEnd`                    | nothing — output ignored (both references)             |
| `StopFailure` / `Notification`  | nothing — output ignored (Claude Code's rule)          |
| `PreCompact`                    | `additionalContext` / stdout → extra compact instructions — **no block** (see below) |
| `PostCompact`                   | nothing — envelope only                                |

**Exit codes** (Claude Code's semantics, verbatim): `0` — success, stdout is
read as a verdict when it starts with `{`, else treated as plain text. `2` —
**block**, and the reason is **stderr**, not stdout; for `PermissionRequest`,
which answers in permissions, a block is a **deny** (it used to fall through
to the classifier and the prompt), and on an event with nothing to block —
`SessionStart`, `SubagentStart`, `PreCompact`, `PostCompact` — the reason is
shown to the user alone, on the hook-run cell (*The hook-run cell*). Any other non-zero — a
non-blocking error, surfaced to the user and otherwise ignored. Stdout that
starts with `{` but fails to parse is a non-blocking error too, never a
silent allow.

**Merging.** When several handlers match one event: **any** block wins, the
**first** reason is kept, the contexts **concatenate** in handler order, and
for `PreToolUse` a `deny` outranks an `ask`, which outranks an `allow`.

The three events added after the first eleven carry Claude Code's own
fields. `PostToolUseFailure` has `tool_name`, `tool_input`, `tool_use_id`, the
failure's text as `error`, and `is_interrupt` (always `false` — an
interrupted call fires no post-tool event here). `StopFailure` has `error`
(the kind, below), `error_details` and `last_assistant_message`.
`Notification` has `notification_type`, `title` and `message`.

Two payload fields are **extensions**, since the contract has no slot for
facts this app has: `tool_response` is `{"output": …, "success": …}` (our tools
return text, not a structured result, and a stable shape beats
sometimes-a-string), and `SubagentStop` carries `success`. That second one
exists because the obvious shortcut is wrong: `stop_hook_active` means *this
run was started by a `Stop` hook's block* — the loop guard — so reporting a
failed agent through it would hand a hook author the wrong fact under a
documented name. It is always `false` here, because nothing yet continues an
agent from a hook. codex extends these payloads the same way, with `turn_id`.

## Matchers

Both references share one implementation, and so do we
(`hooks::matcher::matches`):

- absent, empty, or `*` — matches everything.
- every character in `[A-Za-z0-9_|]` — the **fast path**: split on `|`, trim,
  and compare for **exact equality**. No regex is compiled. Note this means
  `bash` deliberately does *not* match a hypothetical `bashoutput`.
- anything else — compiled as a regex.

The original design proposed warn-and-skipping real regexes to avoid taking a
`regex` dependency. That turned out to be a cost that barely exists: `regex`
is a thin wrapper over the `regex-automata` engine, which `fancy-regex` (the
token counter's split pattern, since replaced by a vocabulary-free estimate —
`docs/architecture/token-estimate.md`) already pulled into every build, and `regex` is now
the crate's one regex engine. Since `^Bash$` is a common
matcher in real Claude Code configs, silently skipping it would be a footgun
with no offsetting saving, and we compile it instead. A matcher that fails to
compile is warned about and skipped.

Once `regex` was this crate's alone, its `perf` features were dropped
(`default-features = false`, `std` + `unicode`): the lazy and one-pass DFAs,
the backtracker and the `aho-corasick` prefilters speed up searches through
long texts, and a matcher is only ever tried against a tool name. The crate
promises the same matches with or without them, `unicode` keeps `\w`, `\d`
and `(?i)` as they were, and the binary is 0.57 MB smaller
(`docs/ops/binary-size.md`).

**Tool names answer to both spellings.** The match query for the tool events
is this app's own tool name — `bash`, `read`, `write`, `edit`, `agent` —
lowercase, as the model sees it, and each also answers to its Claude Code
spelling as a **second exact name** (`Bash`, `Read`, `Write`, `Edit`, and
`Task` for `agent` — the reference's own legacy-alias precedent,
`permissionRuleParser.ts`). So `"matcher": "Bash"` — the single most common
hook config in the wild — and `^Bash$` both select `bash` here, while
`bashoutput` still matches neither: an alias is a second exact name, never a
prefix or case-folding rule. The bash companions (`docs/tools/bash-tools.md`) have
no reference spelling to borrow, so each one's second name is the one its cell
shows — `bashsend` answers to `BashSend`, likewise `BashWait`, `BashKill`,
`BashList`, and the legacy `bash_session` to `BashSession` — and `Bash` never
selects them: it is an exact name. Agent types (`SubagentStart`/`SubagentStop`)
and the other non-tool queries are never aliased.

## Where each event attaches

| event                | site                                                            |
| -------------------- | --------------------------------------------------------------- |
| `PreToolUse`         | `llm/agent.rs` — before `approve`; **`agent` launches included** |
| `PermissionRequest`  | `llm/approval.rs` — after the standing rules                     |
| `Notification`       | `llm/approval.rs` registers it; it runs when the loop opens the prompt, on the notification lane |
| `PostToolUse`        | `llm/agent.rs` — on `execute`'s return, **successes only**       |
| `PostToolUseFailure` | `llm/agent.rs` — on `execute`'s return, **failures only**        |
| `SubagentStart`      | `llm/backend.rs` — inside `spawn_subagent_run`                   |
| `SubagentStop`       | `llm/agent.rs` — `run_agent`'s Complete arm, via the tagged sink |
| `SessionStart`       | `tui` — fired in the background as a conversation opens; the next spawn's top waits for it |
| `SessionEnd`         | `tui` — `/clear`, `/resume` and quit, under a 2 s event budget   |
| `UserPromptSubmit`   | `llm/backend.rs` — the spawn top, after the SessionStart wait    |
| `Stop`               | `llm/agent.rs` — `run_agent`'s Complete arm                      |
| `StopFailure`        | `llm/agent.rs` — a failed round, the tool-call ceiling, or a reply that never finished |
| `PreCompact`         | the compact spawn's top, via the `CompactHooks` wrapper          |
| `PostCompact`        | the compact spawn's completion, via the same wrapper             |

Every one of the fourteen fires. None runs on the tokio loop: the tool,
turn and compact events run on a **backend thread**, `Notification` on the
notification lane's worker (the loop only hands it over), and the
boundary's session events on threads of their own (`SessionStart`) or under
a hard budget (`SessionEnd`) — which is the second half of the story below.

`PermissionRequest` sits exactly where the auto-mode classifier does — after
the standing allowlist, before the user — because it answers the same
question: *may this run without asking?* Two consequences worth stating: a
call a standing rule already covers never reaches it (nothing is being asked),
and neither does any call when the permission gate is off entirely
(`ALTER_ZERO_PERMISSIONS=0`), since there is no permission decision to make.
`PreToolUse` still fires in both cases — it is about the call, not about
asking.

### One execution context — the premise that dissolved

The original phasing assumed the six "loop-path" events had to run on the
tokio current-thread loop, where a blocking subprocess freezes the spinner
and the keyboard — and that attaching them therefore needed a pending-turn
state machine the codebase doesn't have. Verifying the references killed
that premise:

- **Claude Code fires its stop hooks inside the query loop**, not at the
  REPL layer — a `Stop` block *is* the next iteration of that loop
  (`query.ts:1282-1305`), which is exactly `run_agent` here.
- **Neither reference blocks the first paint on `SessionStart`.** Claude
  Code kicks it off without awaiting before render and blocks only the
  *first API call* (`main.tsx:3761-3765`) — the hook does run at launch,
  in the background; codex queues a pending source and drains it at the
  top of the next turn (`turn.rs:239-241`). We follow Claude Code's
  timing now (below), and the first paint stays free either way — the
  checkpoint probe's lesson, honoured by both references before we ever
  hit it.

So every event lands on a **backend thread**, where blocking is already
normal and correct — the permission gate parks there on a `Condvar`,
`llm::exec` polls its children every 20 ms — and the loop-thread problem
never existed:

- **`Stop`/`SubagentStop`** fire in `run_agent` at `RoundOutcome::Complete`,
  before `StreamDone` — on a real answer only: an empty reply is continued
  first, and a turn that gives up on one ends through `StopFailure` instead
  (`docs/architecture/unfinished-replies.md`). A block appends the reply-so-far as an assistant
  message and the feedback (`Stop hook feedback: {reason}` over the hook's
  `Hook:` and `Config:` lines — *What the model reads*, below) as the next user
  message, flips `stop_hook_active`, and the
  **same turn runs another round** — so the status line keeps working, the
  queue and token tally are untouched, and `dispatch_after_turn`'s
  turn-end checkpoint lands *after* every continuation: a formatter hook's
  writes are inside the turn's snapshot and an Esc-Esc backtrack restores
  them. By construction it never fires for a `/compact` turn (that backend
  carries the wrapper below), a `!` shell (no agent loop), a backend error
  (that is `StopFailure`'s, below), or an
  interrupt (Claude Code returns before stop hooks on an abort — Esc always
  breaks a continuation chain). The engine never refuses a re-block:
  `stop_hook_active` is the hook's own guard, the reference's exact posture.
  The tagged sink routes a subagent's completion to `SubagentStop`
  (matcher = agent type) with the same block-to-continue semantics on the
  agent's own loop; a killed or errored agent fires nothing.
- **`StopFailure`** fires where the turn ends on an error instead of an
  answer: a round that failed (after the bounded retry gave up), the
  **Max tool calls** ceiling, or a reply that still never finished after
  its continuations (`docs/architecture/unfinished-replies.md`). Its `error` — also the
  matcher — is Claude Code's error type read off the failure
  (`llm::hooks::stop_failure_kind`): `rate_limit` (429), `overloaded`
  (529), `authentication_failed` (401/403), `billing_error` (402),
  `model_not_found` (404), `invalid_request` (any other 4xx),
  `server_error` (5xx, a transport failure, an unreachable host),
  `max_output_tokens` for a reply that kept being cut off at the output
  cap, and `unknown` for the rest — the ceiling and a model that never
  replied included. Output is ignored, as in the reference. An interrupt fires
  none (Esc is not a failure), and neither does a subagent's failed round:
  the event is the session's turn ending.
- **`SessionStart`** fires **in the background as a conversation
  opens** — Claude Code's timing: the user can type at once, and the first
  turn waits for the hooks so their context still lands. The boundary
  fires `startup` right after the first frame is queued (`resume` for a
  `--continue`/`--resume` boot; a bare `--resume` waits for its picker,
  whose pick is the conversation that begins — dismissed, the fresh one it
  lands in begins then, as `startup`), `clear` after a `/clear`,
  and `resume` after a `/resume` load — each once the recorder has moved
  on, so the payload names the new conversation (matcher = source). The
  run is held in `HookHandles::started`; the next spawn's top waits for it
  — polling the turn's `CancelToken`, so Esc is prompt, and a cancelled
  wait leaves the run held for the turn after — and takes its context,
  which lands as user messages after the prompt and is recorded (see
  `HookNote` below). One run is held at a time: a later firing replaces,
  and kills, one no turn read, since its context belonged to the
  conversation that ended. A source the boundary could not fire — hooks
  off, or no real backend yet (the dummy runs no hooks) — stays queued
  and runs at the first turn that has both, the old codex drain.
- **`UserPromptSubmit`** fires right after that wait, before the first
  request. A **block** streams `StreamEvent::PromptBlocked` instead of
  anything else: the loop rolls the just-recorded submission back out of
  history (the recorder's shrink-rewrite erases it from the rollout — the
  reference's "erased from context", so nothing a secrets-filter censored
  can reach a later turn), returns the text to the composer, purge-repaints
  the echo away, and commits a red one-line notice, `Prompt blocked by
  UserPromptSubmit hook: {reason}`. `additionalContext`
  injects even on a block — both references' rule. A loop-initiated turn (a
  background completion's follow-up) is marked synthetic and skips the
  event: its prompt is not the user's. Only a sink running the turn's hooks
  consumes the mark, so every **user** turn clears it before it spawns — a
  mark set while hooks were off, or on the offline demo, used to outlive
  its turn and silence the user's next real prompt. The rollout is synced
  before the turn's thread exists, too (codex's rule: the file exists
  before the hook runs), so a conversation's very first `UserPromptSubmit`
  already names its `transcript_path` instead of racing the loop-bottom
  sync that creates it. The backend consults its retained
  request prefix only past this check, so a block costs no prompt-cache
  prefix (`docs/providers/prompt-caching.md`).
- **`PreCompact`/`PostCompact`** ride the summarization spawn through the
  `CompactHooks` wrapper: its "prompt" hook is PreCompact (context/stdout →
  extra summarization instructions; **it cannot block — in either
  reference**: Claude Code's own in-app docs claim exit 2 blocks compaction,
  but the `blocked` field is never read at any PreCompact call site, and
  codex has no exit-2 arm for it), and its completion is PostCompact. The
  wrapper never drains the session-source queue and never continues a
  summarization; its notes stay out of the conversation record (they are
  ephemeral, like the compact chunks themselves).
- **`SessionEnd`** fires as a conversation closes — `/clear` (`clear`),
  a `/resume` switching away (`resume`), a quit (`prompt_input_exit`) and a
  headless run's end (`other`, `docs/cli/headless.md`), Claude Code's reasons —
  **before** the recorder moves on, so it names the
  conversation that ended, and under a **2 s whole-event budget**: Claude
  Code caps this event at 1.5 s and codex clamps it to 3 s, because
  quitting must never hang on a hook. A quit fires it **after** the turn
  and every agent were cancelled, so no hook of theirs can land after the
  session's end. Output is ignored, as in both references. Any
  `SessionStart` context no turn read is discarded first (its hooks killed
  if still running). It fires **only for a conversation whose start
  fired**, keeping the pair honest: if a `SessionStart` source is still
  queued — hooks off, no real backend (a dummy-only session, which runs no
  hooks), a bare `--resume` boot still in its picker — the conversation
  never started for the hooks, so no `SessionEnd` fires and the pending
  source goes with it. A session-logging hook never sees an end without
  its start. A `/login` mid-session makes the pair real: the next turn
  runs the queued `startup`, and the end fires at quit.
- **`PostToolUseFailure`** is `PostToolUse`'s twin for a call that ran and
  **failed** (`ok: false` — a non-zero exit, an edit whose text was not
  found), with the same answers: a block is feedback the model must read,
  and `additionalContext` is appended to the result. A refused or blocked
  call never ran, so it fires neither, and a call handed to the
  background has not finished, so it fires neither yet.
- **`Notification`** fires with `notification_type` `permission_prompt`
  the moment the permission prompt is **on screen** — the moment a human
  is actually being asked, which `PermissionRequest` (consulted before the
  classifier) cannot say. Title `Permission needed`, message `Alter Zero
  needs your permission to use {Tool}`, matcher = the type. Claude Code's
  other notification types (idle, auth) are not modelled.

  *On screen* is not *raised*. A request can arrive while another prompt
  holds the screen and queue behind it, and it can be settled there unseen:
  a "don't ask again" covering it, its agent stopped, an Esc. So the
  thread that raises a prompt only **registers** the notification on the
  permission gate (`PermissionGate::on_shown`, before the request is
  sent, so the loop cannot open it unseen), the loop marks the prompt
  shown at the bottom of the iteration that opens it (`mark_shown` — the
  prompt open then is the one the next frame paints), and that is what
  runs it. A request that ends first drops what it registered
  (`forget_shown`): a prompt nobody saw notifies nobody.

  It is observation only and must not hold the prompt up, so it cannot
  run on the asking thread — and run anywhere else with no order, a slow
  notifier landed after the `PostToolUse` of the call it announced, or
  after the next prompt's notification, and a hook keeping a status (a
  multiplexer's "waiting on you" mark, say) ended on the wrong one. So
  notifications run on the session's **notification lane**
  (`NotificationLane`, on `HookHandles` so every sink and tagged subagent
  sink shares it): one worker thread, one notification at a time, in the
  order their prompts were shown; a job whose turn ended while it queued
  runs nothing, and an Esc reaps one that hangs. Every other dispatch that
  has handlers to run first waits for the notifications already shown —
  polling its own `CancelToken` — and `SessionEnd` does so inside its own
  2 s budget. The prompt itself never waits: the gate's wait is under way
  before the notification runs, and an answer given while a slow notifier
  runs is acted on at once — only the call's next hook event waits.

**A blocking hook must poll the `CancelToken`.** The reason the existing
`Condvar` park is safe is that `PermissionGate::wait` takes a cancellation
predicate and `run_bash` re-checks cancellation every 20 ms. The hook runner
polls on the same 20 ms cadence — the payload write included, on its own
thread, since a pipe-buffer-filling payload fed to a handler that never
reads stdin used to park an inline write past both Esc and the timeout —
and kills the process group on cancel, so Esc stays prompt.

**A hook's own exit ends the wait for its output.** Its stdout and stderr
are read on threads of their own, and once the hook's process has exited
the runner gives them `PIPE_DRAIN_GRACE` (1 s) to deliver the last bytes —
not as long as the pipes stay open. A hook that starts a background
process (`server &`, a daemonising notifier) hands that process its
pipes, and waiting for their end held the event — and the turn, or the
tool call — for as long as the process lived. What arrived in the grace is
the hook's output; a reader still running then is left to drain the rest.

## How a verdict reaches the screen

The single most useful finding of the audit was that this codebase already had
somewhere for every hook verdict to land, under another name. **No new
`StreamEvent` variant was added**, and none was needed:

- **A block** is `Approval::Reject { display, result, kind }` — the permission
  gate's own two-text rejection. It already renders as a red cell, already
  keeps the longer model-facing text on `ToolCall::context_output`, already
  round-trips through the rollout, and already shows correctly in Ctrl+D. A
  hook block's cell says which event's hook refused, which hook, and the
  `hooks.json` it is configured in; the model reads the same and one line on
  what to do (*What the user sees* and *What the model reads*, below). A
  hook that blocks and also speaks is
  heard on both counts (`llm::agent`'s `refuse_by_hook`, an `agent` launch
  included): its `additionalContext` follows the refusal the model reads, and
  its note — a sibling handler's warning, a `systemMessage` — sits under the
  red cell. Both used to be dropped with the call.
- **A `PreToolUse` allow** is `Approval::AllowNoted { note }` — the auto-mode
  classifier's existing "this ran without you" provenance row.
- **`additionalContext`** rides `StreamEvent::ToolNote` for the dim `⎿` row
  the user sees, and is appended to the **model-facing** result, under its
  `{event} hook additional context:` label, through the
  existing `ToolAnswered { display, result }` split, so the cell stays clean
  while `context_output` carries what the model actually read. Both are
  recorded, both survive a `/resume` — a call handed to the background
  included, whose recorded launch text used to drop the context after the
  round that read it.

Two additions were needed for the conversation-level events, because nothing
existing could express them:

- **`StreamEvent::HookNote { label, text, report }`** carries hook-injected
  conversation text — a Stop block's feedback, a SessionStart/
  UserPromptSubmit hook's additional context — and, when the hooks said
  something, the **report** that is their cell (*The hook-run cell*, below;
  `text` is then empty for a report that added nothing to the
  conversation). The loop finalises the assistant run before it (invariant
  4's flush-before-you-interleave), records a **`HistoryItem::HookNote`** and
  commits the report's cell; a note with no report stays cell-less inline
  and shows in the Ctrl+O transcript under its dim `● {label}` heading. The
  text is replayed **verbatim**
  by `context_messages` as the user-role message the model actually read,
  and round-tripped through the rollout so a `/resume` keeps it. Context
  wire text uses Claude Code's exact shape — the
  `<system-reminder>`-wrapped `{event} hook additional context: …`. The
  agent loop's own continuation prompts ride it too — the note an empty or
  cut-off reply is answered with, under `Empty reply` / `Reply cut off`
  (`docs/architecture/unfinished-replies.md`) — because a continuation needs exactly what
  a Stop block's feedback gets.
- **`StreamEvent::PromptBlocked { reason }`** is a `UserPromptSubmit`
  block's terminal event — sent instead of `StreamDone`, nothing follows
  it; the loop's arm owns the rollback described above.

The one change that *was* needed: `ToolAnswered` and `ToolRejected` gained a
`truncated` flag. They had none because their only users — the ask tool and a
permission refusal — never produce capped output. Routing an **executed**
call's resolution through them does, so without it a hook-amended `bash`
whose output hit `TOOL_OUTPUT_MAX_BYTES` silently lost the `…` marker its
expanded cell appends. `false` at every site where nothing ran.

**Injected context appends at the frontier, never at the front.** The obvious
design — reuse the `user_instructions` slot — is wrong twice over. That slot
is owned by the project-doc feature and is rewritten wholesale at turn start;
and `context_messages_with` places it *first*, where implicit-caching
providers key the prompt prefix, so front-injection would invalidate the
prompt cache every single turn. Hook context therefore lands on the tool call
that produced it: cache-stable, `/resume`-safe, and — being an append —
needing no `history_generation` bump.

## What the user sees

Every row a hook leaves on screen names its **event**, so a session with
several hooks — or hooks in both the user's and a project's `hooks.json` —
never leaves the user guessing which one spoke. A refusal also names the
**hook** and the **file** it is configured in, since a call that did not run
is the one thing a user goes looking for the cause of:

| the hook…                                             | the user sees                                                          |
| ----------------------------------------------------- | ---------------------------------------------------------------------- |
| refuses a call (`PreToolUse`)                         | the red cell `Blocked by PreToolUse hook: {reason}` · `Hook: {command}` · `Config: {hooks.json}` |
| denies it (`PermissionRequest`)                       | the same, as `Blocked by PermissionRequest hook: …`                    |
| lets it run without asking                            | the dim row `Allowed by PreToolUse hook`, or `Allowed by PermissionRequest hook: {reason}` |
| rewrites it (`PreToolUse` `updatedInput`)             | the dim row `Rewritten by PreToolUse hook` — the cell shows the hook's arguments, not the model's |
| blocks a result (`PostToolUse`, `PostToolUseFailure`) | the dim row `Flagged by {event} hook: {reason}`                        |
| adds context to a call                                | the dim row `Context added by {event} hook`                            |
| blocks a prompt (`UserPromptSubmit`)                  | the red notice `Prompt blocked by UserPromptSubmit hook: {reason}`     |
| blocks a stop (`Stop`, `SubagentStop`)                | the `● Ran 1 Stop hook` cell, `Stop hook feedback: {reason}` under it  |
| says anything at a turn's edge (the events below)     | the same cell — *The hook-run cell*, below                             |

(`·` separates lines.) A refusal, as it reads on screen:

```
● Bash(npm --version)
  ⎿  Blocked by PreToolUse hook: Use pnpm instead of npm.
     Hook: ~/.alter-zero/hooks/pnpm-only.sh
     Config: ~/.alter-zero/hooks.json
```

It used to read `Blocked by hook: {reason}` and nothing else, which left the
user to work out which of their hooks had stopped the call — and from which
file — while the model was told both.

**The cell is what the model reads, minus the instruction**, built by the
same `llm::hooks::block_texts`, with one difference: `Config:` names the file
**`~`-relative** under the user's home and absolute elsewhere — the spelling
the startup review's headings and the `/hooks` detail page's `Source:` row
give the same file (`ui::display_cwd`'s rule, which a test holds the cell's
own helper to, the backend importing no renderer), so the file a hook was
trusted in reads the same on the cell it refuses. The model
keeps the absolute path, which it can open. The rule is home-relative and
never cwd-relative because the cell's text is recorded: a `/resume` from
another directory replays it as written. The boundary hands the sink the
user's home (`CommandHooks::with_home`, read back through `HookSink::home`),
the way it hands the app its path display; a sink with none — a test's fake —
names the file as recorded, and so does one whose home is not absolute: a
`$HOME` set but empty would otherwise strip from every path and show
`~//home/u/…` — `display_cwd` relates nothing to such a home either. A hook read
from no file (a config parsed from text) has no `Config:` line, and a block
with no known handler shows the headline alone. A conversation recorded
before this wording keeps its old `Blocked by hook:` line after a `/resume`:
the record is what the screen showed.

**The dim rows name only the event.** They are one line under a call, joined
by ` · ` when several things are said, where a command and a path would
crowd out the rest; the hook behind a `Flagged by` row is named where the
model reads its feedback (Ctrl+D). A warning about a broken hook names it by
its **label** — the command's first line, cut at 80 characters with `…`
marking the cut (`hooks::hook_label`, the `Hook:` line's own name for it) — so
`hook ./lint.sh failed (exit 1): …` names a script by its path, while an
inline launcher of a dozen lines is `hook set -e… failed (exit 1): …` rather
than the whole program poured into a one-line row.

A refusal folds like the cell's other output (`docs/ui/long-lines.md`): a reason
of a row or two leaves the whole refusal inline, while a longer one keeps its
first rows and the `Hook:` and `Config:` lines join the rest behind `… +N
lines (ctrl+o to expand)` — the reason comes first because it is what the
user reads the cell for.

### The hook-run cell

A hook that answers a **call** leaves its trace on that call's cell. The
events at a turn's edges have no call to hang a row from, so what their hooks
said used to land nowhere the user looked: a `Stop` block was a cell-less note
only Ctrl+O showed, and a `Stop` hook that broke, said a `systemMessage` or
answered `continue: false` was dropped outright — as was everything but the
context a `SessionStart` or `UserPromptSubmit` hook gave. They get a cell of
their own now, Claude Code's `Ran 1 stop hook` in this app's dress:

```
● Ran 1 Stop hook (ctrl+o to expand)
  ⎿  Stop hook feedback: There are uncommitted Git changes. Review the
     changes and commit the relevant files with a descriptive commit
     message before finishing.
```

and in Ctrl+O, every hook that ran with what it said under it:

```
● Ran 1 Stop hook
  ⎿  bash "$HOME/.alter-zero/hooks/check-git.sh" (0.4s)
     Config: ~/.alter-zero/hooks.json
  ⎿  Stop hook feedback: There are uncommitted Git changes. Review the
     changes and commit the relevant files with a descriptive commit
     message before finishing.
```

**Which events, and where.** The cell lands where the hooks ran:

| event                          | the cell sits                                            |
| ------------------------------ | -------------------------------------------------------- |
| `Stop`                         | under the reply, above the continuation it bought or the turn's `Worked for` line |
| `SubagentStop`                 | the same, on the agent's own transcript (its session view and its Ctrl+O) |
| `SubagentStart`                | under the task that opens the agent's own transcript, where its first round reads them |
| `SessionStart`, `UserPromptSubmit` | under the prompt that read them                      |
| `PreCompact`, `PostCompact`    | above the compaction's `● Context compacted` line        |
| `StopFailure`                  | above the turn's red error                               |

The tool events have none — their rows are on the call's own cell — and
neither do two events with nowhere to show one: `Notification` runs on its
lane while the prompt it announces holds the screen, and `SessionEnd` fires as
the conversation it ends goes away.

**A `SubagentStart` cell can beat its agent.** The hooks run on the agent's
own thread before its first round, while the lead announces the launch
(`StreamEvent::AgentBatch`) on its own channel once every agent of the batch
is spawned — and the loop reads the two channels in either order, so the
hooks' note can arrive for an agent the roster does not list yet. Such a
note waits for the announcement (`App::start_agent_group`) instead of being
dropped with the rest of an unknown agent's events — a chunk racing a sweep
is still dropped — at most 32 of them, oldest first.

**Only when a hook said something.** A hook that exits `0` and prints nothing
has approved by saying nothing — the common case, and on every turn — so it
leaves no cell, inline or in Ctrl+O, exactly as before. What counts, row by
row, most consequential first:

| row                                     | when                                                           |
| --------------------------------------- | -------------------------------------------------------------- |
| `Stopped by Stop hook: {reason}`        | `continue: false` from `Stop`/`SubagentStop` — the agent stops, a sibling's block notwithstanding |
| `Stop hook feedback: {reason}`          | the block the agent keeps going on — the first line of what the model reads |
| `Context added by {event} hook`         | `additionalContext`, or the plain stdout that is context for that event |
| `{systemMessage}`                       | a line the hook addressed to the user, verbatim                |
| `{reason}`                              | a block where there is nothing to block — exit `2` (its stderr) or `decision: "block"` from `SessionStart`, `SubagentStart`, `PreCompact` or `PostCompact` — verbatim |
| `{event} hook error: {hook} {what broke}` | it could not run, timed out, exited non-zero (not `2`), printed invalid JSON — red: `Stop hook error: ./check.sh failed (exit 1): {stderr}` |
| `hooks.json: …`                         | a config problem on the way: a matcher that is no regex, a handler skipped — red |

Each row opens its own `⎿` and wraps under its own text, and **each row folds
on its own** like a tool cell's output (`docs/ui/long-lines.md`): three rows and a
`… +N lines` row, or four shown whole. Row by row rather than the body as a
whole, because folding the body let one long row — a Stop hook's paragraph of
feedback — fold every row after it out of sight, a red bullet over an error
row nobody could see; every row shows at least its head. The header carries
the `(ctrl+o to expand)` hint — there is always something to expand, the
hooks themselves — so the fold row does not repeat it.

**`feedback`, not `error`.** Claude Code prints a blocking Stop hook's reason
as `Stop hook error:`. It is not an error: the hook did its job, and its words
are what the agent acts on next. So the row says *feedback* — the word the
model reads it under — and *error* is kept for a hook that broke, in red, so a
guard doing its work and a guard that crashed cannot be mistaken for each
other at a glance. An error row says what went wrong **and which hook**: the
warning the parse wrote (`hook ./check.sh failed (exit 1): …`) loses only its
first word (`HookSaid::error`). The collapsed cell lists its rows without
their hooks, so a cell of several hooks would otherwise leave the one that
broke to Ctrl+O; and since the hook goes by its label, an inline program
costs the row its first line, never its whole text. Expanded, where the row
sits under that hook's own line, it drops the name again
(`RanHook::row_under`) rather than say it twice.

**Only what took effect.** The rows are the outcome, not every word printed:
the feedback row is the first block's, the one the agent continues on (a
second hook's block is merged away, the rule above); `continue: false`
outranks a block, so a stopped turn shows the stop and no feedback; a
`UserPromptSubmit` block is the red notice that replaces the prompt, so the
cell never repeats it. A block on an event with nothing to block —
`SessionStart`, `SubagentStart`, `PreCompact`, `PostCompact` — changes
nothing, so its reason
is a message for the user, never feedback: Claude Code shows that stderr to
the user alone, where this runner used to drop it; a bare exit `2` gave no
reason and says nothing. Plain stdout from an event that does not take it as
context (`Stop`, `PostCompact`, `StopFailure`) is chatter and stays unshown.
And an Esc that reaps a running Stop hook leaves no cell: the runner reports
the hook as `did not run: interrupted`, which would read as a hook that
broke when it was the user who stopped the turn.

**Expanded**, the cell lists every hook that ran, in order: its command (the
first line, cut at 80 characters — the `Hook:` line's label), how long it ran,
and the `hooks.json` it is configured in (`~`-relative, the refusal cell's
rule); then the rows that hook said, a context row followed by the text it
added and an error row without the name the line above it gives. A silent
hook beside a talkative one is listed too — the header counts every hook that
ran.

**The bullet** is green when every hook ran cleanly — a block is a hook
working — and red when any row is an error.

**The record.** The cell is the hook note's **report**: `HistoryItem::HookNote`
carries an optional `hooks::HookReport` beside its text, built by
`llm::hooks::hook_report` from each handler's run (command, file, how long)
and its parsed answer, every row's words composed by the function that words
the matching row elsewhere (`context_added_note`, the model's `Stop hook
feedback:` line). The note's `text` is still exactly what the model read —
empty for a cell that added nothing to the conversation (a hook's failure, a
`PostCompact` message) — and the derived context replays only non-empty text,
so **a cell changes nothing the model reads**. On disk a note that carries
text keeps its `hook_note` record with the report beside it — an older build
replays the text and ignores the report — and one with no text is a
`hook_report` record, which an older build skips (it would otherwise replay an
empty message). A note recorded before this keeps its old Ctrl+O rendering:
the record is what the screen showed. The agent loop's own continuation
prompts (`docs/architecture/unfinished-replies.md`) ride the note with no report and stay
cell-less.

## What the model reads

Every hook text the model reads has codex's shape: one line saying which
event's hook spoke and what it said, the hook named under it when it
decided something, and an instruction only where a call was refused. Each
is built by one function in `llm::hooks`, which the offline demo calls too.

| the hook…                                        | the model reads                                                                 |
| ------------------------------------------------ | ------------------------------------------------------------------------------- |
| refuses a call (`PreToolUse`)                    | `Blocked by PreToolUse hook: {reason}` · `Hook: {command}` · `Config: {hooks.json}` · `Do not retry the same call; address the reason or ask the user.` — as the call's result, any context it also added after it |
| denies it (`PermissionRequest`)                  | the same, as `Blocked by PermissionRequest hook: …`                              |
| blocks a result (`PostToolUse`, `PostToolUseFailure`) | `{event} hook feedback: {reason}` · `Hook: {command}` · `Config: {hooks.json}` — after the call's own output |
| adds context to a call                           | `{event} hook additional context: {text}` — after the call's own output          |
| adds context to a turn or an agent (`SessionStart`, `UserPromptSubmit`, `SubagentStart`, `PreCompact`) | that line inside `<system-reminder>` tags, as a user message |
| blocks a stop (`Stop`, `SubagentStop`)           | `Stop hook feedback: {reason}` · `Hook: {command}` · `Config: {hooks.json}` — the next user message       |
| blocks a prompt (`UserPromptSubmit`)             | nothing that turn (the prompt is erased); the red `Prompt blocked by UserPromptSubmit hook: {reason}` notice replays as an `[error]` line on later turns |

(`·` separates lines.) A refusal used to be five lines that never said
which hook refused — `A configured lifecycle hook blocked this tool call.`,
`Reason: …`, two sentences of instruction — and a `PostToolUse` block was
`A lifecycle hook flagged this result: …`.

**A decision names its hook by its command and its file.** The handler
that gave the reason travels with it (`ParsedHook::blocked_by`, a
`HookOrigin` merged beside `block_reason` into `HookOutcome::block`), so
with several handlers the hook named is the one whose words the model read.
`Hook:` is its command as the user wrote it: for a script it *is* the
hook's path, which the agent can read when it needs the rule, and it is the
name the user knows the hook by. Only its first line shows, cut to 80
characters with `…` marking the cut, since an inline program is not a name.
`Config:` is the absolute path of the `hooks.json` it is configured in —
the user's own or a project's, which matters once the two layers
merge — so the agent can read the file, or tell the user exactly where to
change the rule. Each group learns its file when it is loaded
(`HooksFile::with_source`, a field the JSON never carries), the selection
copies it onto the handler (`CommandHook::source`) and the run onto its
report (`HookRun::file`); a config parsed from text alone has none, and its
verdicts name only the command. codex names no hook on a refusal — its
`Command: …` suffix echoes the model's own call back, which the call's
arguments already carry here — and only a Stop continuation carries one,
the `hooks.json` path in its `hook_run_id`; Claude Code names the command.
A context line names its event and not its hook: it reports a fact rather
than a decision anyone will ask about.

**Only a refusal carries an instruction.** A refused call is where a model
reliably does the wrong thing next — the identical call again — so it gets
one line. Feedback and context say nothing about what to do with them,
because what they say is what to do.

**Context is labelled** because it always shares its place with text it
must not be mistaken for. codex gives `additionalContext` a developer
message of its own; ours is appended to the call's result (the cache-stable
frontier above) or sent as a user message, where unlabelled text reads as
the tool's output or as the user talking. `SubagentStart` context used to
arrive exactly that way, as bare user messages at the top of the agent's
first turn; it is wrapped like the session's now (`subagent_start_note`).

**A refusal without a reason reads `no reason given`** (`hooks::NO_REASON`):
exit 2 with empty stderr, a deny with no message, `continue: false` with no
`stopReason`. It follows `Blocked by PreToolUse hook:`, so it says only what
is missing; the old `blocked by a hook (no reason given)` made the cell say
it twice.

A rollout recorded before this wording keeps what it was sent with: a
`/resume` replays what the model actually read.

## A worked config

`~/.alter-zero/hooks.json`, doing the two things people ask for most — guard a
command, and lint after a file change:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "bash",
        "hooks": [
          {
            "type": "command",
            "command": "jq -re '.tool_input.command | test(\"rm -rf\") | not' >/dev/null || { echo 'no recursive deletes' >&2; exit 2; }",
            "timeout": 10
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "write|edit",
        "hooks": [
          {
            "type": "command",
            "command": "path=$(jq -r '.tool_input.path'); case \"$path\" in *.rs) cargo fmt -- \"$path\" >/dev/null 2>&1;; esac",
            "timeout": 60
          }
        ]
      }
    ]
  }
}
```

The first refuses through the shell-native route — exit `2`, reason on stderr.
The second answers with JSON when it wants to say something and stays silent
otherwise; a hook that prints nothing and exits `0` has approved by saying
nothing, which is the common case and costs one process.

To hand the model a fact instead of a refusal, print the verdict object:

```bash
printf '%s' '{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"the build id is ZX-4417"}}'
```

## Configuration and trust

**Two layers, and nothing runs until it is trusted.**
`~/.alter-zero/hooks.json` (`ALTER_ZERO_HOOKS_FILE` overrides that user
path; `ALTER_ZERO_HOOKS` gates the feature off entirely, the
`ALTER_ZERO_PERMISSIONS` pattern) — and, since `docs/state/project-config.md`, the
project's own `{root}/.alter-zero/hooks.json`, discovered at the
nearest-`.git` root and **union-merged** after the user file
(`HooksFile::merged` — one more entry from the loader, no precedence rules to
invent: `select`'s existing dedup-by-command keeps the user's copy of a
duplicate, any block wins, contexts concatenate). A hook runs a shell command
on its own whenever its event fires, outside the permission prompt, so
**every hook in either file runs only once the user has trusted it** —
codex's answer, which asks about each new or changed hook at startup. The
**startup review** (`docs/state/config-review.md`) lists them verbatim on the first
frame, `trust.json` records each trusted hook by its event, matcher and
command, an edited hook is asked about again with its old command beside the
new one, and `/hooks` trusts or stops trusting one later. A project-local
hooks file that ran on clone would earn a repository arbitrary code execution
on the first tool call; a user file anything else can write to (the agent
included) is the same hole one directory over.

A malformed `hooks.json` is **not** silently treated as "no hooks" — that is
how a user comes to believe a guard is running when it is not. The parse
returns an error, the boundary raises it as a red startup toast, and the
session continues with hooks off.

The child is spawned through the same **tty-detach tier walk** every other
shell child uses (`docs/tools/tty-detach.md`), so a hook that tries to prompt on
`/dev/tty` fails fast instead of hijacking the TUI. It runs in the session's
cwd and gets `ALTER_ZERO_PROJECT_DIR` — plus `CLAUDE_PROJECT_DIR`, so scripts
written for Claude Code find their project root unchanged — set to the
**project root**: the nearest `.git` ancestor, the root the project layer
reads `.alter-zero/` from, and the cwd outside a repository or when that
ancestor is the home directory (`trust::hooks_project_dir`). So a project
reaches its own scripts the same way from any subdirectory:

```json
{ "type": "command",
  "command": "${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/format-and-test.sh" }
```

It used to be the cwd, so launched in `repo/src` that command named a script
that does not exist, the hook failed open, and a guard it was meant to run
never did. Like every shell child, a hook runs without the herdr pane id
(`docs/architecture/herdr.md`).

The `/settings` menu carries a **Hooks** row. It is a plain on/off toggle,
which is all a cycling menu with no free-text field can ever offer, and it
reports `false (unavailable)` when no config file resolved. It is **on by
default** and **per working directory** (`docs/state/per-directory-state.md`) —
the startup review being the gate the old opt-in stood in for — so a
directory that should not run them opts out: one cycle records
`"hooks": false` in its `settings.json` entry. With the row off the review
asks nothing about hooks (none would run), and turning it back on over hooks
waiting for trust opens the review's hooks page.
`ALTER_ZERO_HOOKS` seeds the row for a run (`=1` on, a falsy value off — the
`ALTER_ZERO_TOOLS` pattern) and is never saved.

## Module layout

Following the pure-core / boundary split the rest of the crate uses:

| where               | what                                                                | tested by  |
| ------------------- | -------------------------------------------------------------------- | ---------- |
| `src/hooks/` (pure) | config parse · matcher · payload builders · verdict parse · merge · the cell's report | unit, TDD  |
| `src/subprocess.rs` | `spawn_shell_with` — the tier walk with caller-chosen stdio            | unit + real-`sh` |
| `src/llm/hooks.rs`  | the runner and the `HookSink` glue: payload → spawn → parse → merge    | real-`sh` unit tests + `smoke.sh` |
| `src/tui/config.rs` | locating and loading `hooks.json`                                      | existing   |

The pure half takes a hook's captured stdout **as a string**, so every verdict
and merge rule is unit-testable from fixtures with no process anywhere — the
same split `session`/`history` use for their JSONL formats. Only the spawn is
boundary code.

### The seam: one trait, not fourteen closures

`run_agent` already takes nine parameters under an
`#[allow(clippy::too_many_arguments)]`. Adding a closure per hook event is how
this feature rots into fourteen hand-wired call sites and an unreviewable
signature. Instead there is one trait object with defaulted no-op methods:

```rust
pub trait HookSink: Send + Sync + Debug {
    fn pre_tool_use(&self, …) -> PreToolVerdict { PreToolVerdict::default() }
    fn post_tool_use(&self, …) -> PostToolVerdict { PostToolVerdict::default() }
    fn post_tool_use_failure(&self, …) -> PostToolVerdict { PostToolVerdict::default() }
    fn permission_request(&self, …) -> Option<HookPermissionVerdict> { None }
    fn permission_prompt(&self, _call: &ToolCallRequest, _cancel: &CancelToken) -> Option<OnShown> { None }
    fn subagent_start(&self, …) -> Vec<String> { Vec::new() }
    fn stop(&self, _active: bool, _last: &str, _cancel: &CancelToken) -> StopVerdict { … }
    fn stop_failure(&self, _error: &str, _details: &str, _cancel: &CancelToken) -> Option<HookReport> { None }
    fn session_start(&self, _cancel: &CancelToken) -> HookContexts { … }
    fn user_prompt_submit(&self, _prompt: &str, _cancel: &CancelToken) -> PromptVerdict { … }
    fn session_end(&self, _reason: &str) {}
    fn for_subagent(&self, …) -> Option<Arc<dyn HookSink>> { None }
    fn prompt_hook_label(&self) -> &'static str { "UserPromptSubmit" }
    fn home(&self) -> Option<&Path> { None }
}
```

Every blocking method takes the turn's `CancelToken` — the trait says one
thing about blocking, and the one exception (`session_end`, which runs past
cancellation under its own 2 s budget) says so at the method. `stop` routes
itself: the lead sink fires `Stop`, a `for_subagent`-tagged one fires
`SubagentStop` — Claude Code picks the event the same way, by the presence
of an agent id.

Every method defaults, so `NoHooks` costs nothing and every existing test
compiles untouched. **Adding event number six is one defaulted method plus one
call site** — not a wider signature and not a new subsystem. That property is
the whole point of the design; if a later change breaks it, the change is
wrong. It held for the three events added after the first eleven:
`post_tool_use_failure`, `stop_failure` and `permission_prompt` are three
defaulted methods, each with one call site. The two methods that are not
events answer the loop's questions about the sink itself: `prompt_hook_label`
which event the compact wrapper's prompt hook is, and `home` the directory a
refusal's cell names the hook's file from (*What the user sees*).

## Deliberately not doing

- **`mcp_tool`, `prompt` and `agent` handler types.** codex parses them; only
  `command` is implemented there either. Ours parse and are skipped with a
  warning rather than failing the file.
- **Async (fire-and-forget) handlers.** Every v1 hook is synchronous. The
  `"async": true` flag parses and is ignored, with a warning. The
  bounded-concurrency pool codex uses is a phase-two concern.
- **Output spilling.** codex writes an oversized `additionalContext` to disk
  with a preview. We cap and truncate (`HOOK_OUTPUT_MAX_BYTES`) instead until
  someone needs more.

## Offline coverage

`scripts/smoke.sh` unsets every `*_API_KEY` before launching, so the boundary
suite always runs on the dummy backend, and `src/tui/` gets no unit tests by
policy. A hook runner would therefore have zero automated coverage unless a
dummy scenario drives it — so one exists. A prompt mentioning **`hook`** plays
a scripted turn with both halves of the contract:

```
● Bash(rm -rf build/)
  ⎿  Blocked by PreToolUse hook: no destructive deletes outside ./tmp
     Hook: ~/.alter-zero/hooks/no-deletes.sh
     Config: ~/.alter-zero/hooks.json

● Bash(ls -la src)
  ⎿  total 8
     drwxr-xr-x  app
     drwxr-xr-x  ui
  ⎿  Context added by PostToolUse hook

● …

● Ran 1 Stop hook (ctrl+o to expand)
  ⎿  Stop hook feedback: Before you finish, explain what a Stop hook does.

● And as I went to finish, a Stop hook exited 2: …
```

Its refusal text comes from `llm::hooks::block_texts`, its row from
`llm::hooks::context_added_note`, and its closing Stop hook's feedback and
cell from `llm::hooks::stop_feedback_texts` and `llm::hooks::hook_report` —
the functions the live runner calls, handed a stand-in hook command and file
(the Stop hook's run read by `hooks::parse_run`, the way a real one is) — so
the offline cells are byte-for-byte the ones a real `hooks.json` produces,
the rule every other scripted demo follows with the real executor's
formatters. **`smoke.sh` Phase 72** drives it and asserts the blocked call
produced no output of its own, its cell named the event, the hook and the
file, the allowed one kept its output under the provenance row naming its
event, the model-only instruction never reached scrollback, the Ctrl+O
transcript kept the refusal, and the closing Stop hook's block is a `● Ran 1
Stop hook` cell inline — the feedback right under its header, the round it
bought after it, its command nowhere inline — that Ctrl+O expands into the
hook that ran, how long it ran and its `hooks.json`. **Phase 150** drives
the hook-run cell through the real runner and a stub provider: a
`SessionStart` hook's message and a `UserPromptSubmit` hook's context as
cells under the prompt, a `Stop` hook that blocks once beside one that
always breaks — the feedback leading the first cell and the error row
saying what broke and which hook, the expansion not naming it twice; the
round it bought; the second
firing a cell of the error alone — the model reading the feedback and the
context and none of what was the user's alone, Ctrl+O listing both hooks
with their times, and a refused turn's `StopFailure` hook's cell above the
red error. **Phase 151** drives a subagent's `SubagentStart` cell: a lead
launching one foreground agent beside two `SubagentStart` hooks, one briefing
it and one exiting 2 — the agent's first request carrying the briefing and
not the reason, its session view showing the `● Ran 2 SubagentStart hooks`
cell under its task (the context row, then the reason as a message), and the
lead's conversation never showing it.
**Phase 73** drives the `prompt-block` scenario (cue: `hook` + `block my
prompt`) through the real loop arm: the blocked submission leaves scrollback
(exactly the composer's restored copy of the text remains) under the red
one-line notice. **Phase 133** drives the session events, which need a
real backend, against a stub provider that refuses every request: a
`SessionStart` hook that sleeps 2 s fires before any prompt without holding
up the first frame, its context reaches the first request, the prompt's
`session_id` is the conversation's and its `transcript_path` names the
file, the refused turn fires `StopFailure` typed `invalid_request`, a
`/clear` moves the id on (`SessionEnd` naming the old one, the turn sent
during the new `SessionStart` waiting for it), a quit's `SessionEnd` comes
last; a bare `--resume` fires nothing while its picker is up, then
`SessionStart(startup)` when it is dismissed or `SessionStart(resume)`
under the picked conversation's id — with no `SessionEnd` for the fresh
conversation the boot held, which never began — and the offline demo fires
neither end. **Phase 134** raises a real permission prompt (the stub asks
for one `bash` call) under a `Notification` hook that sleeps 4 s: answered
at once, the command runs at once, and the hooks still record
`PreToolUse`, `Notification`, `PostToolUse` in that order — on the
previous build the notifier landed last. Each of these phases answers the
startup review's hooks page first, trusting its fixture's hooks
(`docs/state/config-review.md`). **Phase 134b** trusts a project's
own `hooks.json` whose guard is `${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh`
and launches in the repository's `src/`: the guard runs (in the cwd), the
stub's `rm -rf build/` never does, the model reads the hook and the
project's `hooks.json` by path, and the refusal's cell names both too — the
file from the phase's own home, `~/projrepo/.alter-zero/hooks.json`.

Three layers cover the rest:

- the **pure core** is unit-tested from fixtures with no process anywhere;
- the **runner** has real-`sh` tests beside `llm::exec`'s — a hook reading the
  payload off stdin and blocking on what it finds, a timeout being reaped, a
  cancelled turn reaping a running hook, a missing command failing open, a
  hook whose leftover process holds its pipes, a background `SessionStart`
  waited for, cancelled, discarded and replaced;
- the **wiring** has `run_agent` tests over a fake sink, and three
  `#[ignore]`d **live** tests in `tests/live_openrouter.rs` that put a real
  model through a real `hooks.json`: a `PreToolUse` block (the command never
  runs), a `PostToolUse` `additionalContext` (the model reads the injected
  fact and repeats it), and an `updatedInput` rewrite (the hook's command is
  what executes).

## Deliberate deviations from the references

Verified against both sources, and chosen — not accidental:

- **Exit 2 beats a JSON verdict here and in codex; Claude Code parses the
  JSON first** (`hooks.ts:2533` returns before the exit-2 arm at `:2648`),
  so a script that prints a valid verdict *and* exits 2 reads differently
  there. Scripts that do both are ambiguous everywhere; we follow codex.
- **`{`-prefixed stdout that fails to parse is a loud warning** here and in
  codex; Claude Code swallows it silently (`hooks.ts:447-450`) and treats
  the output as plain text. Loud beats silent for a guard.
- **Exit 2 with empty stderr still blocks** (with `no reason given` as its
  reason) — the Claude Code behaviour (`'No stderr output'`); codex fails
  open there.
- **A `PostToolUse` block keeps the call's output.** codex replaces the
  result with the hook's feedback; here the feedback follows it, since the
  call already ran and what it printed is still true — a linter's complaint
  about a file does not make the test results above it wrong.
- **`Notification` keeps its place in the event order.** Claude Code fires
  its notification hooks without awaiting them, so a slow one can land
  after the events that followed it; here it runs on the notification
  lane and the next event with hooks waits for it (above), since a status
  kept by hooks must end on the last thing that happened.
- **Handlers run sequentially**, not in parallel (both references
  parallelize one event's handlers). Deterministic order is what keeps the
  merge's "first reason wins" meaningful — Claude Code's parallel version
  has a lost-deny-reason race (`hooks.ts:2862-2867`) we decline to copy.
  The cost: N pathological handlers stack N timeouts.
- **`tool_response` is a stable `{"output": …, "success": …}` object.** The
  references send the raw tool result (a bare string for codex's bash, an
  arbitrary object for Claude Code), which is sometimes-a-string; ours is
  one shape a script can always index.
- **`stop_hook_active` is advisory, engine-side too** — verified as Claude
  Code's exact posture (no code refuses a re-block; `maxTurns` is its only
  backstop). Esc is ours: an interrupt never fires Stop, so it always
  breaks a continuation chain.
- **A handler carrying Claude Code's `if` pre-filter is skipped with a
  warning** rather than run *wider than the author scoped it* — we don't
  evaluate permission-rule expressions. `shell: "powershell"` is skipped
  the same way; `shell: "bash"` runs under `sh`.
- **Oversized output is capped in memory at 64 KiB** as it is read, with a
  truncation marker — codex spills to a temp file with a preview instead
  (~2.5k-token default); nobody has needed the spill here yet.

## Known gaps

- **`"async": true` runs synchronously**, with a warning. So does a
  `statusMessage`, which is parsed and not displayed.
- **Two events leave no cell.** `Notification` runs on its lane while the
  prompt it announces holds the screen, with no channel back to the
  conversation, and `SessionEnd` fires as the conversation it ends goes
  away; both ignore their hooks' output by contract, so only a hook that
  broke goes unseen.
- **Plain stdout an event does not read is not shown.** A `Stop` hook that
  prints and exits `0` is chatter (Claude Code's `suppressOutput` exists to
  hide such output from its transcript); it never reaches the cell.
- **The rest of Claude Code's events are unmodelled** (`PermissionDenied`,
  `TeammateIdle`, `FileChanged`, …), as are its `Notification` types other
  than `permission_prompt`; a config naming them loads fine and never
  fires.
- **A PermissionRequest hook that abstains loses its warnings** — the
  fail-open posture has no channel to the user from that position; a
  deciding verdict carries them on its note.
- **The hooks files are read once, at bootstrap.** A `.alter-zero/hooks.json`
  that appears (or changes) mid-session is a restart away, and the startup
  review asks about it then (`docs/state/config-review.md`).
- **Trust pins the command, not what it reads.** `./fmt.sh` stays trusted when
  `fmt.sh` changes — both references draw the same line.

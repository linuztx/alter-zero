# Lifecycle hooks

Claude Code and codex both let a user wedge their own command into the agent's
lifecycle: run a linter after every `edit`, refuse a `bash` command that
matches a pattern, feed the model extra context when a session opens. This is
that feature, ported to this codebase.

```
● Bash(rm -rf build/)
  ⎿  Blocked by hook: no destructive deletes outside ./tmp
```

The **`/hooks`** command opens a read-only inline browser over this
configuration — which events have hooks, under which matchers, running what —
see `docs/hooks-menu.md`.

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
  `cwd`, `permission_mode`, `hook_event_name`, `tool_name`, `tool_input`,
  `tool_use_id`, `agent_id`, `agent_type`.
- **stdout (the verdict) is `camelCase`** — `continue`, `stopReason`,
  `suppressOutput`, `systemMessage`, `decision`, `reason`,
  `hookSpecificOutput.{hookEventName,permissionDecision,
  permissionDecisionReason,updatedInput,additionalContext}`.

Eleven events are modelled. What each may return:

| event                           | the hook may return                                   |
| ------------------------------- | ----------------------------------------------------- |
| `PreToolUse`                    | allow / deny / ask · block + reason · `additionalContext` · `updatedInput` |
| `PermissionRequest`             | `allow` / `deny` + message                             |
| `PostToolUse`                   | block + reason · `additionalContext`                   |
| `UserPromptSubmit`              | block + reason · `additionalContext`                   |
| `Stop` / `SubagentStop`         | block + reason (make the agent keep going)             |
| `SessionStart` / `SubagentStart`| `additionalContext`                                    |
| `SessionEnd`                    | nothing — output ignored (both references)             |
| `PreCompact`                    | `additionalContext` / stdout → extra compact instructions — **no block** (see below) |
| `PostCompact`                   | nothing — envelope only                                |

**Exit codes** (Claude Code's semantics, verbatim): `0` — success, stdout is
read as a verdict when it starts with `{`, else treated as plain text. `2` —
**block**, and the reason is **stderr**, not stdout. Any other non-zero — a
non-blocking error, surfaced to the user and otherwise ignored. Stdout that
starts with `{` but fails to parse is a non-blocking error too, never a
silent allow.

**Merging.** When several handlers match one event: **any** block wins, the
**first** reason is kept, the contexts **concatenate** in handler order, and
for `PreToolUse` a `deny` outranks an `ask`, which outranks an `allow`.

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
`regex` dependency. That turned out to be a cost that does not exist:
`tiktoken-rs` (a direct dependency, for the status-line tally) already pulls
`regex` into every build, so the crate is compiled either way. Since `^Bash$`
is a common matcher in real Claude Code configs, silently skipping it would be
a footgun with no offsetting saving, and we compile it instead. A matcher that
fails to compile is warned about and skipped.

**Tool names answer to both spellings.** The match query for the tool events
is this app's own tool name — `bash`, `read`, `write`, `edit`, `agent` —
lowercase, as the model sees it, and each also answers to its Claude Code
spelling as a **second exact name** (`Bash`, `Read`, `Write`, `Edit`, and
`Task` for `agent` — the reference's own legacy-alias precedent,
`permissionRuleParser.ts`). So `"matcher": "Bash"` — the single most common
hook config in the wild — and `^Bash$` both select `bash` here, while
`bashoutput` still matches neither: an alias is a second exact name, never a
prefix or case-folding rule. Agent types (`SubagentStart`/`SubagentStop`)
and the other non-tool queries are never aliased.

## Where each event attaches

| event               | site                                                            |
| ------------------- | --------------------------------------------------------------- |
| `PreToolUse`        | `llm/agent.rs` — before `approve`; **`agent` launches included** |
| `PermissionRequest` | `llm/approval.rs` — after the standing rules                     |
| `PostToolUse`       | `llm/agent.rs` — on `execute`'s return, **successes only**       |
| `SubagentStart`     | `llm/backend.rs` — inside `spawn_subagent_run`                   |
| `SubagentStop`      | `llm/agent.rs` — `run_agent`'s Complete arm, via the tagged sink |
| `SessionStart`      | `llm/backend.rs` — the spawn top drains the queued sources       |
| `SessionEnd`        | `tui` — `/clear` and `shutdown`, under a 2 s event budget        |
| `UserPromptSubmit`  | `llm/backend.rs` — the spawn top, after the SessionStart drain   |
| `Stop`              | `llm/agent.rs` — `run_agent`'s Complete arm                      |
| `PreCompact`        | the compact spawn's top, via the `CompactHooks` wrapper          |
| `PostCompact`       | the compact spawn's completion, via the same wrapper             |

Every one of the eleven fires. All of them run on a **backend thread** —
none on the tokio loop — which is the second half of the story below.

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
- **Neither reference runs `SessionStart` at startup.** Claude Code kicks it
  off without awaiting before render and blocks only the *first API call*
  (`main.tsx:3761-3765`); codex queues a pending source and drains it at the
  top of the next turn (`turn.rs:239-241`). Nothing ever blocks the first
  paint — the checkpoint probe's lesson, honoured by both references before
  we ever hit it.

So every event lands on a **backend thread**, where blocking is already
normal and correct — the permission gate parks there on a `Condvar`,
`llm::exec` polls its children every 20 ms — and the loop-thread problem
never existed:

- **`Stop`/`SubagentStop`** fire in `run_agent` at `RoundOutcome::Complete`,
  before `StreamDone`. A block appends the reply-so-far as an assistant
  message and the feedback (`Stop hook feedback:\n{reason}`, Claude Code's
  exact wording) as the next user message, flips `stop_hook_active`, and the
  **same turn runs another round** — so the status line keeps working, the
  queue and token tally are untouched, and `dispatch_after_turn`'s
  turn-end checkpoint lands *after* every continuation: a formatter hook's
  writes are inside the turn's snapshot and an Esc-Esc backtrack restores
  them. By construction it never fires for a `/compact` turn (that backend
  carries the wrapper below), a `!` shell (no agent loop), a backend error
  (Claude Code fires `StopFailure` there, which we don't model), or an
  interrupt (Claude Code returns before stop hooks on an abort — Esc always
  breaks a continuation chain). The engine never refuses a re-block:
  `stop_hook_active` is the hook's own guard, the reference's exact posture.
  The tagged sink routes a subagent's completion to `SubagentStop`
  (matcher = agent type) with the same block-to-continue semantics on the
  agent's own loop; a killed or errored agent fires nothing.
- **`SessionStart`** is a queued-source drain: bootstrap queues `startup`
  (a `--continue`/`--resume` boot swaps it for `resume`), `/clear` queues
  `clear`, a `/resume` load queues `resume`, and the next spawn's thread
  drains the queue before round 1 (matcher = source). Context lands as
  user messages after the prompt and is recorded (see `HookNote` below).
- **`UserPromptSubmit`** fires right after that drain, before the first
  request. A **block** streams `StreamEvent::PromptBlocked` instead of
  anything else: the loop rolls the just-recorded submission back out of
  history (the recorder's shrink-rewrite erases it from the rollout — the
  reference's "erased from context", so nothing a secrets-filter censored
  can reach a later turn), returns the text to the composer, purge-repaints
  the echo away, and commits a red reason-only notice. `additionalContext`
  injects even on a block — both references' rule. A loop-initiated turn (a
  background completion's follow-up) is marked synthetic and skips the
  event: its prompt is not the user's.
- **`PreCompact`/`PostCompact`** ride the summarization spawn through the
  `CompactHooks` wrapper: its "prompt" hook is PreCompact (context/stdout →
  extra summarization instructions; **it cannot block — in either
  reference**: Claude Code's own in-app docs claim exit 2 blocks compaction,
  but the `blocked` field is never read at any PreCompact call site, and
  codex has no exit-2 arm for it), and its completion is PostCompact. The
  wrapper never drains the session-source queue and never continues a
  summarization; its notes stay out of the conversation record (they are
  ephemeral, like the compact chunks themselves).
- **`SessionEnd`** fires on `/clear` (`clear`) and at quit
  (`prompt_input_exit`), under a **2 s whole-event budget** — Claude Code
  caps this event at 1.5 s and codex clamps it to 3 s, because quitting must
  never hang on a hook. Output is ignored, as in both references. It fires
  **only when a real backend is active**, keeping the pair honest: a
  dummy-only session drains no `SessionStart` (the queue drains at a real
  spawn's top), so it fires no `SessionEnd` either — a session-logging hook
  never sees an end without its start. A `/login` mid-session makes the
  pair real: the next turn drains `startup`, and the end fires at quit.

**A blocking hook must poll the `CancelToken`.** The reason the existing
`Condvar` park is safe is that `PermissionGate::wait` takes a cancellation
predicate and `run_bash` re-checks cancellation every 20 ms. The hook runner
polls on the same 20 ms cadence — the payload write included, on its own
thread, since a pipe-buffer-filling payload fed to a handler that never
reads stdin used to park an inline write past both Esc and the timeout —
and kills the process group on cancel, so Esc stays prompt.

## How a verdict reaches the screen

The single most useful finding of the audit was that this codebase already had
somewhere for every hook verdict to land, under another name. **No new
`StreamEvent` variant was added**, and none was needed:

- **A block** is `Approval::Reject { display, result }` — the permission
  gate's own two-text rejection. It already renders as a red cell, already
  keeps the longer model-facing text on `ToolCall::context_output`, already
  round-trips through the rollout, and already shows correctly in Ctrl+D. A
  hook block is spelled `⎿ Blocked by hook: {reason}` and the model reads the
  stop-and-wait instruction.
- **A `PreToolUse` allow** is `Approval::AllowNoted { note }` — the auto-mode
  classifier's existing "this ran without you" provenance row.
- **`additionalContext`** rides `StreamEvent::ToolNote` for the dim `⎿` row
  the user sees, and is appended to the **model-facing** result through the
  existing `ToolAnswered { display, result }` split, so the cell stays clean
  while `context_output` carries what the model actually read. Both are
  recorded, both survive a `/resume`.

Two additions were needed for the conversation-level events, because nothing
existing could express them:

- **`StreamEvent::HookNote { label, text }`** carries hook-injected
  conversation text — a Stop block's feedback, a SessionStart/
  UserPromptSubmit hook's additional context. The loop finalises the
  assistant run before it (invariant 4's flush-before-you-interleave) and
  records a cell-less **`HistoryItem::HookNote`**: invisible inline (Claude
  Code hides these from its normal view too — `isMeta`), expanded in the
  Ctrl+O transcript under its dim `● {label}` heading, replayed **verbatim**
  by `context_messages` as the user-role message the model actually read,
  and round-tripped through the rollout so a `/resume` keeps it. Context
  wire text uses Claude Code's exact shape — the
  `<system-reminder>`-wrapped `{event} hook additional context: …`.
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

**Two layers: the user's file, plus the project's behind a trust gate.**
`~/.alter-zero/hooks.json` (`ALTER_ZERO_HOOKS_FILE` overrides that user
path; `ALTER_ZERO_HOOKS` gates the feature off entirely, the
`ALTER_ZERO_PERMISSIONS` pattern) — and, since `docs/project-config.md`, the
project's own `{root}/.alter-zero/hooks.json`, discovered at the
nearest-`.git` root and **union-merged** after the user file
(`HooksFile::merged` — one more entry from the loader, no precedence rules to
invent: `select`'s existing dedup-by-command keeps the user's copy of a
duplicate, any block wins, contexts concatenate). The reason it was
user-level-only for so long is the reason the project layer is
**default-deny**: a project-local hooks file without a trust gate means
cloning a repository earns arbitrary code execution on the first tool call.
The gate is codex's content-hash answer with the interaction design solved
by `/trust` — the project file is read and fingerprinted at bootstrap but
contributes **nothing** to the merge until the user approves it in the
`/trust` review menu, and an edited file is untrusted again. See
`docs/project-config.md` for the whole gate (the `trust.json` store, the
review menu, live activation, `ALTER_ZERO_PROJECT_CONFIG`).

A malformed `hooks.json` is **not** silently treated as "no hooks" — that is
how a user comes to believe a guard is running when it is not. The parse
returns an error, the boundary raises it as a red startup toast, and the
session continues with hooks off.

The child is spawned through the same **tty-detach tier walk** every other
shell child uses (`docs/tty-detach.md`), so a hook that tries to prompt on
`/dev/tty` fails fast instead of hijacking the TUI. It inherits the session's
cwd and gets `ALTER_ZERO_PROJECT_DIR` — plus `CLAUDE_PROJECT_DIR`, so scripts
written for Claude Code find their project root unchanged.

The `/settings` menu carries a **Hooks** row. It is a plain on/off toggle,
which is all a cycling menu with no free-text field can ever offer, and it
reports `false (unavailable)` when no config file resolved.

## Module layout

Following the pure-core / boundary split the rest of the crate uses:

| where               | what                                                                | tested by  |
| ------------------- | -------------------------------------------------------------------- | ---------- |
| `src/hooks/` (pure) | config parse · matcher · payload builders · verdict parse · merge      | unit, TDD  |
| `src/subprocess.rs` | `spawn_shell_with` — the tier walk with caller-chosen stdio            | unit + real-`sh` |
| `src/llm/hooks.rs`  | the runner and the `HookSink` glue: payload → spawn → parse → merge    | real-`sh` unit tests + `smoke.sh` |
| `src/tui/config.rs` | locating and loading `hooks.json`                                      | existing   |

The pure half takes a hook's captured stdout **as a string**, so every verdict
and merge rule is unit-testable from fixtures with no process anywhere — the
same split `session`/`history` use for their JSONL formats. Only the spawn is
boundary code.

### The seam: one trait, not eleven closures

`run_agent` already takes nine parameters under an
`#[allow(clippy::too_many_arguments)]`. Adding a closure per hook event is how
this feature rots into eleven hand-wired call sites and an unreviewable
signature. Instead there is one trait object with defaulted no-op methods:

```rust
pub trait HookSink: Send + Sync + Debug {
    fn pre_tool_use(&self, …) -> PreToolVerdict { PreToolVerdict::default() }
    fn post_tool_use(&self, …) -> PostToolVerdict { PostToolVerdict::default() }
    fn permission_request(&self, …) -> Option<HookPermissionVerdict> { None }
    fn subagent_start(&self, …) -> Vec<String> { Vec::new() }
    fn stop(&self, _active: bool, _last: &str, _cancel: &CancelToken) -> Option<String> { None }
    fn session_start(&self, _cancel: &CancelToken) -> Vec<String> { Vec::new() }
    fn user_prompt_submit(&self, _prompt: &str, _cancel: &CancelToken) -> PromptVerdict { … }
    fn session_end(&self, _reason: &str) {}
    fn for_subagent(&self, …) -> Option<Arc<dyn HookSink>> { None }
    fn prompt_hook_label(&self) -> &'static str { "UserPromptSubmit" }
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
wrong.

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
- **A hooks browser UI.** codex has one. The `/settings` on/off row is the
  whole surface.

## Offline coverage

`scripts/smoke.sh` unsets every `*_API_KEY` before launching, so the boundary
suite always runs on the dummy backend, and `src/tui/` gets no unit tests by
policy. A hook runner would therefore have zero automated coverage unless a
dummy scenario drives it — so one exists. A prompt mentioning **`hook`** plays
a scripted turn with both halves of the contract:

```
● Bash(rm -rf build/)
  ⎿  Blocked by hook: no destructive deletes outside ./tmp

● Bash(ls -la src)
  ⎿  total 8
     drwxr-xr-x  app
     drwxr-xr-x  ui
  ⎿  Context added by hook
```

Its refusal text comes from `llm::hooks::block_texts` — the function the live
runner calls — so the offline cell is byte-for-byte the one a real `hooks.json`
produces, the rule every other scripted demo follows with the real executor's
formatters. **`smoke.sh` Phase 72** drives it and asserts the blocked call
produced no output of its own, the allowed one kept its output under the
provenance row, the model-facing paragraph never reached scrollback, the
Ctrl+O transcript kept the refusal, and the demo's closing Stop-hook
feedback note stayed cell-less inline while the transcript recorded it.
**Phase 73** drives the `prompt-block` scenario (cue: `hook` + `block my
prompt`) through the real loop arm: the blocked submission leaves scrollback
(exactly the composer's restored copy of the text remains) under the red
reason-only notice.

Three layers cover the rest:

- the **pure core** is unit-tested from fixtures with no process anywhere;
- the **runner** has real-`sh` tests beside `llm::exec`'s — a hook reading the
  payload off stdin and blocking on what it finds, a timeout being reaped, a
  cancelled turn reaping a running hook, a missing command failing open;
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
- **Exit 2 with empty stderr still blocks** (with a stand-in reason) — the
  Claude Code behaviour (`'No stderr output'`); codex fails open there.
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
- **`PostToolUseFailure`, `StopFailure`, `Notification` and the rest of
  Claude Code's twenty-seven events are unmodelled.** A failed tool call
  fires nothing (PostToolUse fires only on success, both references'
  behaviour); a config naming them loads fine and never fires.
- **A PermissionRequest hook that abstains loses its warnings** — the
  fail-open posture has no channel to the user from that position; a
  deciding verdict carries them on its note.
- **The project layer is read once, at bootstrap** — like the user file. A
  `.alter-zero/hooks.json` that appears (or changes) mid-session is a
  restart away; `/trust`'s approval activates exactly the reviewed startup
  snapshot (`docs/project-config.md`).

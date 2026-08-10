# Lifecycle hooks

Claude Code and codex both let a user wedge their own command into the agent's
lifecycle: run a linter after every `edit`, refuse a `bash` command that
matches a pattern, feed the model extra context when a session opens. This is
that feature, ported to this codebase.

```
● Bash(rm -rf build/)
  ⎿  Blocked by hook: no destructive deletes outside ./tmp
```

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
| `SessionEnd`                    | the universal envelope only                            |
| `PreCompact` / `PostCompact`    | the universal envelope only                            |

**Exit codes** (Claude Code's semantics, verbatim): `0` — success, stdout is
read as a verdict when it starts with `{`, else treated as plain text. `2` —
**block**, and the reason is **stderr**, not stdout. Any other non-zero — a
non-blocking error, surfaced to the user and otherwise ignored. Stdout that
starts with `{` but fails to parse is a non-blocking error too, never a
silent allow.

**Merging.** When several handlers match one event: **any** block wins, the
**first** reason is kept, the contexts **concatenate** in handler order, and
for `PreToolUse` a `deny` outranks an `ask`, which outranks an `allow`.

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

**Tool names are ours, not Claude Code's.** The match query for the tool
events is this app's own tool name — `bash`, `read`, `write`, `edit`, `agent`
— lowercase, as the model sees it. A config written against Claude Code's
`Bash`/`Write` will not match; that is a naming difference between the two
tools, not a contract difference, and `Bash|bash` covers both.

## Where each event attaches

| event               | site                                            | v1  |
| ------------------- | ----------------------------------------------- | --- |
| `PreToolUse`        | `llm/agent.rs` — before `approve`                | yes |
| `PermissionRequest` | `llm/approval.rs` — after the standing rules     | yes |
| `PostToolUse`       | `llm/agent.rs` — on `execute`'s return           | yes |
| `SubagentStart`     | `llm/backend.rs` — inside `spawn_subagent_run`   | yes |
| `SubagentStop`      | `llm/backend.rs` — at `registry.finish`          | yes |
| `SessionStart`      | `tui/bootstrap.rs`                               | no  |
| `SessionEnd`        | `tui/bootstrap.rs` — `shutdown`                  | no  |
| `UserPromptSubmit`  | `tui/turn.rs` — `start_turn`                     | no  |
| `Stop`              | `tui/turn.rs` — `dispatch_after_turn`            | no  |
| `PreCompact`        | `tui/turn.rs` — `start_compact_turn`             | no  |
| `PostCompact`       | `app/compact.rs` — `finish_compact`              | no  |

### Two execution contexts, and why v1 stops where it does

codex is async throughout, so every hook is simply awaited. This codebase is
not, and that difference decides the phasing.

- **Tool-path events** fire on the backend's own OS thread, where blocking is
  already normal and correct: the permission gate parks there on a `Condvar`
  today, and `llm::exec` already runs a child against a deadline in a 20 ms
  poll loop.
- **Loop-path events** fire on the tokio current-thread loop, where a blocking
  subprocess freezes the spinner, the shimmer and the keyboard.

v1 ships the tool-path set. That is not timidity: it is the half that needs no
new concurrency machinery, and it is the half users actually ask for (guard
`bash`, lint after `edit`). The **pure core models all eleven events anyway**,
so the deferred phase is one call site each and no new subsystem.

Three loop-path landings are also not as clean as they look, and the deferred
phase has to answer them rather than paper over them:

- **`Stop` over-fires.** `dispatch_after_turn` runs at `StreamDone`, at a
  backend error, at *both* Esc-interrupt outcomes, and on an idle
  background-completion arrival — so it also covers `/compact` turns and `!`
  shell turns. None of those is "the model stopped answering the user". It
  also begins with `checkpoint_turn_end()`, so a hook at the head of that
  function writes *into* the turn's snapshot and its work is reverted by a
  later Esc-Esc backtrack. Placement decides whether the hook's output
  survives.
- **`UserPromptSubmit` and the `!` shell.** A `!command` is a user submission,
  it queues and dispatches like one, and gating what the user runs locally is
  arguably the event's strongest case — but it is not a *prompt*, and the
  payload has one field. Open.
- **`SessionStart` blocks the first paint.** `Session::bootstrap` runs after
  the terminal is in raw mode and before the first frame, in exactly the spot
  where the checkpoint probe already taught us that seconds of silence read as
  a hang.

**A blocking hook must poll the `CancelToken`.** The reason the existing
`Condvar` park is safe is that `PermissionGate::wait` takes a cancellation
predicate and `run_bash` re-checks cancellation every 20 ms. The hook runner
polls on the same 20 ms cadence and kills the process group on cancel, so Esc
stays prompt.

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

`~/.alter-zero/hooks.json`, doing the three things people actually ask for —
guard a command, lint after an edit, and feed the model a fact it can't see:

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

**User-level config only.** `~/.alter-zero/hooks.json`
(`ALTER_ZERO_HOOKS_FILE` overrides the path; `ALTER_ZERO_HOOKS` gates the
feature off entirely, the `ALTER_ZERO_PERMISSIONS` pattern). A project-local
`.alter-zero/hooks.json` without a trust gate means cloning a repository earns
arbitrary code execution on the first tool call. codex answers this with a
per-hook content hash and a `trusted_hash` state, but the hard part is the
interaction design — what a user *does* about an untrusted hook — not the
hashing. Because layers would **union** rather than override, the project
layer stays a purely additive change later: one more entry from the loader, no
precedence rules to invent.

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
| `src/llm/hooks.rs`  | the runner and the `HookSink` glue: payload → spawn → parse → merge    | `smoke.sh` |
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
pub trait HookSink: Send + Sync {
    fn pre_tool_use(&self, _call: &ToolCallRequest, _cancel: &CancelToken) -> PreToolVerdict {
        PreToolVerdict::default()
    }
    fn post_tool_use(&self, _call: &ToolCallRequest, _out: &ToolOutcome, _cancel: &CancelToken)
        -> PostToolVerdict { PostToolVerdict::default() }
    fn permission_request(&self, …) -> Option<HookPermission> { None }
    fn subagent_start(&self, _id: &str, _agent_type: &str) -> Vec<String> { Vec::new() }
    fn subagent_stop(&self, …) {}
}
```

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
provenance row, the model-facing paragraph never reached scrollback, and the
Ctrl+O transcript kept the refusal.

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

## Known gaps

- **`transcript_path` is always `null`.** The rollout file is created lazily on
  the first recorded item, so at the moment the sink is built there is no path
  to promise. The field is emitted explicitly as `null` (codex's
  `NullableString`) rather than omitted, so a script can tell "no transcript"
  from "old build". Publishing it later means giving the sink a shared cell the
  recorder writes to.
- **Loop-path events do not fire.** `SessionStart`, `SessionEnd`,
  `UserPromptSubmit`, `Stop`, `PreCompact` and `PostCompact` are modelled —
  payload, verdict, merge — but not yet attached; see the two-execution-contexts
  section for why, and the three questions the deferred phase has to answer.
- **`"async": true` runs synchronously**, with a warning. So does a
  `statusMessage`, which is parsed and not displayed.

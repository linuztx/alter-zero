# What the rollout records about each call and request

Every conversation is recorded to a rollout file (`docs/state/resume.md`). That file
was enough to *resume* a session, but not to *analyse* one. A tool line said
whether the call succeeded and what it printed, and nothing more. The exit code
existed only as the text `Exit code: N` at the head of `output`, a frame that a
long or truncated output could make awkward to find. The duration and the reason
for a failure were not recorded at all. Token usage was one sum per turn on the
`summary` line, so the cost of a single request, or of a single tool step, could
not be computed. A benchmark comparing runs with another harness, which logged
usage per model call and a duration per tool, hit exactly this: it had to regex
`Exit code` out of the text, and its token counts per request had nothing here
to compare with.

The rollout now carries two more kinds of fact:

- every `tool` line records **how the call ran**: `exit_code`, `duration_ms`
  and, on a failure, `error_kind`;
- every model request leaves a **`usage` line** with the provider's own
  counts, written where the request finished.

Both are additions. The existing fields are untouched.

## What codex does (findings)

The rollout format is a port of codex's (`docs/state/resume.md`), so codex is the
model here too (checked against `openai/codex` `codex-rs`):

- **One usage record per model response.** Each completed response persists a
  `token_usage_record` line (`history/src/lib.rs`, written by
  `record_observed_response_completed` in `core/src/session/mod.rs`). Its
  `usage` is that response's `TokenUsage`: `input_tokens` (cached reads
  included), `cached_input_tokens`, `cache_write_input_tokens`,
  `output_tokens`, `reasoning_output_tokens`. The record is always persisted,
  whatever the history mode. It also carries running sums for the turn and
  the thread and the response's id. `TokenCount` is a separate, cumulative UI
  snapshot, not a per-request ledger.
- **Command items keep the exit code and the duration.** `CommandExecutionItem`
  (`protocol/src/items.rs`) has `exit_code: Option<i32>` and `duration`, timed
  with an `Instant` around the spawn and the wait (`core/src/exec.rs`).
  Internally the duration serializes as serde's `{secs, nanos}`, but every
  public schema (the app server, analytics, OpenTelemetry's
  `codex.tool_result`) converts it to an integer `duration_ms`.
- **Failure kinds are an analytics vocabulary.** The durable record has only a
  status (`completed`/`failed`/`declined`). The analytics events classify the
  failure (`ToolItemFailureKind`: `tool_error`, `approval_denied`, …) and the
  end (`ToolItemTerminalStatus`: `completed`, `failed`, `rejected`,
  `interrupted`).

For contrast, Claude Code's transcript stores the API's `usage` on assistant
lines. A response is written as one line per content block, so a reader has to
dedupe by request id. Its exit codes and durations live only in its telemetry.

What we take from codex: one record per request rather than per block, codex's
counts, integer `duration_ms` as every public codex schema has it, and a closed
failure vocabulary built on codex's names, written onto the durable record
where an analysis can read it.

## The `tool` line

Three fields join the `tool` record (`session::ToolRecord`). Each is **omitted
when it has no value**, the rule every later field of that record follows, so
a line without them reads exactly as before:

```json
{"timestamp":"…","type":"tool","payload":{"name":"Bash","args":"cargo test","ok":false,
 "exit_code":101,"duration_ms":48213,"error_kind":"tool_error",
 "output":"Exit code: 101\n…","timestamp":"03:21 PM","shell":false,"truncated":false, …}}
```

- **`exit_code`** is the command's exit status, recorded when it **exited**
  during the call: a `bash` call, a `bashsend`/`bashwait`/`bashkill` whose
  session ended, and a `!` shell command. It is absent when there is none: the
  call is not a command, the command is still running (handed back as a
  session, or moved to the background), it was killed by a signal, or it was
  stopped at its time limit. The `Exit code: N` frame stays in `output` as
  before.
- **`duration_ms`** is how long the call **ran**, in whole milliseconds. It is
  measured from the moment the call starts running, after any permission
  prompt and any `PreToolUse` hook, to its result (`run_agent` times the
  executor, as codex times the spawn and the wait). A `!` command is timed by
  its runner. A call that never ran has none: one refused at the prompt,
  blocked by a hook, over the call limit, or a batch sibling an Esc ended
  before it started, or one naming a secret that is not stored. The running
  call an Esc or a backend error cuts short records how long it had run: the
  loop reads the call's own clock (the one its live cell counts) at that
  moment rather than at the last frame, so a headless run, which draws none,
  records it too.
- **`error_kind`** says why a failed call (`ok: false`) failed. It is
  never present on a call that succeeded:

| `error_kind` | The call… | Set by |
| --- | --- | --- |
| `tool_error` | failed in the tool itself: it ran and failed (a nonzero exit, which `exit_code` names, a missing file, an edit that matched nothing, an MCP error), or it could not run as written (arguments the tool could not read, a secret placeholder naming nothing stored) | the default for any failed outcome (`run_agent`, the `!` runner); the approval seam for the placeholder (`docs/state/secrets.md`) |
| `timed_out` | was stopped at its time limit: a command with nowhere to keep running past its `wait`, or a question nobody answered | the executor, `llm::ask` |
| `interrupted` | was cut short by Esc, running or still waiting its turn | `App::interrupt_turn` |
| `rejected` | was refused by the user: No at the permission prompt (or Tab's amended no, or Ctrl+E), or a declined question | the approval seam, `llm::ask` |
| `classifier_denied` | was refused by auto mode's classifier | the approval seam |
| `hook_blocked` | was refused by a hook: a `PreToolUse` block or a `PermissionRequest` deny | `run_agent`, the approval seam |
| `approval_required` | needed an approval nobody could give: a headless (`-p`) run asks no one | the approval seam |
| `tool_call_limit` | was refused by the `/settings` **Max tool calls** ceiling | `run_agent` |
| `backend_error` | was in flight when the turn's request failed | `App::fail_stream` |

The wire names are codex's where codex has one (`tool_error`, `timed_out`,
`interrupted`, `rejected`). The rest name what only this app has: auto mode's
classifier, hooks, headless runs and the call ceiling.

### How the facts travel

The facts are produced where they are known and carried as data from there:

- The executor's `ToolOutcome` carries `exit_code` and any specific
  `error_kind` (`timed_out`). It sets the exit code at every place it learns
  one: the pipe runner, the terminal runner when the command exited, a session
  call that saw the exit, and the report of an exit claimed after the fact.
- `run_agent` times the executor and builds the call's **`ToolRun`**
  (`stream::ToolRun`: the three facts together). A failed outcome with no
  specific kind becomes `tool_error`. The refusals it decides itself (the
  hook block, the call limit) and the ones the approval seam hands it
  (`Approval::Reject` now names its `kind`) carry their kind and no duration.
- Every event that resolves a call (`ToolEnd`, `ToolAnswered`,
  `ToolRejected`, `ToolBackgrounded`) has a `run` field, so a new resolution
  path has to state its facts to compile. The loop applies it to the running
  call before resolving it (`App::set_tool_run`, the `set_tool_truncated`
  pattern), and the recorded `ToolCall::run` is what `session` writes.
- The calls the app resolves on its own, the batch an Esc or a backend
  error ends, are stamped there (`App::fail_live_queue`).

## The `usage` line

One line per model request the provider reported usage for, the counts
exactly as `StreamEvent::Usage` carried them:

```json
{"timestamp":"…","type":"usage","payload":{"input":18240,"output":312,"cached":17920,"cache_write":0,"reasoning":128}}
```

- **`input`** is every prompt token the request billed, **cached reads and
  cache writes included**. Each wire normalizes to this, the OpenAI accounting
  shape, and the Anthropic wire sums its uncached remainder with both cache
  counts to reach it (`docs/providers/claude.md`).
- **`cached`** is the part of `input` served from the provider's prompt cache,
  and **`cache_write`** the part written to it (`docs/providers/prompt-caching.md`).
- **`output`** is every completion token, **`reasoning`** included: the part
  spent thinking.
- **`agent`** is present only on a subagent's request. It names the agent by
  the same `id` its `agent_group` entry carries, whose `tokens` totals the
  agent's requests. The conversation's own requests, `/compact`'s
  summarization included, carry no `agent`.
- **`compact`** is present, as `true`, only on the request that wrote a
  compaction's summary (`/compact` or auto-compact, `docs/architecture/compact.md`). That
  request is the conversation's own, and its cost is the one an analysis
  most often wants to set apart: it reads the whole context to replace it.

All five counts are always written, zeros included, so a reader needs no
defaults. Their meaning is the per-turn `summary`'s: a finished turn's
`tokens` is the sum of `input + output` over the conversation's own `usage`
lines it produced, and its `cached`/`cache_write` the sums of theirs. A
subagent's lines are not in that sum (they are its group entry's), and a
compaction has no `summary`: its `compact` line is followed by the
`compaction` record instead.

### Where the line sits

A `usage` line is written **where its request finished**, among the items
recorded so far. In a tool round that means the request's line comes before
the tool calls it asked for, and the next request's line comes after them:

```text
message (user)
usage            ← request 1: the reply that asked for the tools
message (assistant)
tool             ← the calls request 1 made
tool
usage            ← request 2: it read their results
message (assistant)
summary
```

So the calls between one `usage` line and the next are the calls the first
request asked for, and the next line's `input` grew by their results. That is
the per-step cost an analysis wants, read off file order the way codex's own
readers read theirs. A thought the model streamed before its text lands where
it settled, which can be ahead of its request's `usage` line. Only the tool
calls are guaranteed to sit between their own request and the next.

### Recording it

A `usage` line is a **sidecar**, like `checkpoint` and `model`
(`docs/state/resume.md`). It is not a history item: the transcript, the context the
model reads, `/resume`'s history and the Ctrl+O view are untouched, and a
build from before it skips the unknown record type.

The recorder (`tui::recorder::SessionRecorder`) holds each request's usage
the way it holds checkpoints. `record_usage` notes it against the history
length when the request finished (`session::RequestUsage::after`), the next
`sync` writes it, and the lines are placed by that position
(`session::history_lines`). The position is what keeps the file honest
through the two ways it is rewritten:

- **A rewrite** (a backtrack, an interrupt's undo, a retry withdrawing an
  attempt, a blocked prompt) re-serializes the whole file from history. Every
  usage record whose position still exists is re-placed exactly where it was
  (`session::retain_usage`). Past the cut, the conversation's own requests go
  with the turns they made, as those turns' own lines do. A subagent's
  request stays, moved to the end of what is left: it was billed whatever
  became of the conversation around it, and a background agent's request can
  finish inside a turn that is then cut (an undone submission, a withdrawn
  attempt) while the agent runs on.
- **A resume** adopts the file. `session::parse_usage` reads the usage lines
  back, each placed after the history items that precede it in the file, so a
  later rewrite keeps them where they were. The payload holds no position: the
  file's order is the record of it.

The loop records the conversation's requests as their `Usage` event arrives
(`tui::stream`) and a subagent's as its own does (`tui::agent`). One reply
event is folded per loop iteration and the recorder syncs at the bottom of
each, so the line lands right after the items recorded before it.

### What is not recorded

- **A request whose provider sent no usage.** The line is the provider's
  report, never an estimate, so a provider that omits the usage frame (or
  the offline demo) leaves none. Codex's record is best-effort the same way.
- **Auto mode's classifier checks.** They are requests on the session's
  provider, but the classifier reports nothing back to the loop, so they are
  in no tally today: not the turn's, not `/context`'s, and not the rollout's.
- **A subagent's own tool calls.** The rollout keeps a subagent as its
  `agent_group` entry, never its transcript (`docs/agents/agent-tool.md`), so its
  calls' exit codes and durations are not in the file. Its requests' usage
  is.

## Compatibility

- **Old files read as before.** Every new field is `serde(default)`ed, and an
  `error_kind` this build does not know reads as absent rather than failing
  the line, the same rule unknown roles and statuses follow.
- **Old builds read new files.** Unknown fields are ignored and unknown record
  types skipped (the forward-compatibility contract in `docs/state/resume.md`). An
  older build that rewrites the file (after a backtrack, say) drops the
  `usage` lines it never knew, as such a rewrite drops any line it skipped.
- Nothing about the **summary** changed: its per-turn sums stay as they were,
  so a reader of them is unaffected.

## Reading a rollout

A failure rate by kind, and the cost of each request, from one file:

```bash
jq -r 'select(.type=="tool" and .payload.ok==false) | .payload.error_kind' rollout.jsonl | sort | uniq -c
```

```bash
jq -c 'select(.type=="usage") | .payload' rollout.jsonl
```

## Testing

- `session`'s own tests pin the format: the three tool fields (written,
  omitted when absent, round-tripped, an unknown `error_kind` read as absent),
  the `usage` line's shape (`agent` and `compact` only when they say
  something), the placement merge (`history_lines`), what a rewrite keeps
  (`retain_usage`), the read back (`parse_usage`), and `parse_session`
  skipping the line.
- `llm::agent`'s tests drive `run_agent` with a fake executor: the outcome's
  exit code and a measured duration on the resolving event, `tool_error` as
  the default kind, each refusal's kind with no duration, and a call handed
  to the background timed to the handoff with no exit code.
- `llm::exec`'s tests run real commands for the exit codes (`exit 3` on the
  pipe and in a terminal, a session wait and a kill that saw the exit, a
  timeout on the pipe runner), `llm::approval`'s and `llm::ask`'s pin each
  refusal's kind, `agents`' the subagent fold, and `app::tests` the
  interrupt's and the backend error's stamps.
- `tests/headless.rs` runs the real binary against the stand-in provider and
  reads the rollout it wrote: a `usage` line per request, in order around the
  `tool` line, that line's `exit_code`, `duration_ms` and `error_kind`, the
  turn's `summary` summing the lines — and, after a `--continue`, the first
  turn's lines where they were with the second turn's after them. It also
  reads a subagent's requests under its id, a compaction's request marked
  `compact`, and a headless refusal recorded as `approval_required` with no
  exit code and no time.
- `smoke.sh` Phase 19 reads the `!` commands' records off a real session: an
  exit code and a time on each, `tool_error` on `exit 3`, and an Esc'd
  `sleep 9` recorded `interrupted` with the second-plus it ran and no exit
  code.

# Headless mode — `alter-zero -p`

`alter-zero -p "prompt"` runs one prompt with **no screen**: no inline
viewport, no raw mode, no cursor query, no key reader, nothing asked. The
answer goes to stdout, anything that went wrong to stderr — every step of
the run too with `--verbose` — and the process exits with a status a script
can branch on. It is the entry point for everything that is
not a person at a terminal — a cron job, a CI step, an n8n node, a benchmark
harness, one stage of a pipeline.

```
$ alter-zero -p "Reply with exactly: OK"
OK
$ git diff origin/main | alter-zero -p "Review this diff; list real bugs only"
$ alter-zero -p --output-format json "Summarize the failing tests" | jq -r .result
$ alter-zero -p -c "Run the suite again and compare with last night"
$ alter-zero -p --permission-mode master --max-turns 40 -C ~/proj < task.md
```

## Why it was missing

The agent loop lived behind `InlineViewport::init`, which enables raw mode
and asks the terminal where its cursor is (DSR) before anything else runs.
With stdin a pipe that fails at once (`Device not configured`); with a pty
that never answers the query it times out; with a `0x0` pty it renders empty
frames. Every caller had to build a terminal emulator — answer `ESC[6n`, set
a window size — before it could send one prompt. crossterm's `EventStream`
has the same flaw on its own: constructing one opens the controlling
terminal eagerly, so even a session that never read a key could not exist
without a tty.

The interactive launch still needs that terminal, but it no longer dies on
the error's Debug form (`Error: Os { code: 6, … }`): when
`InlineViewport::init` fails, `main` prints the cause and names print mode
(`cli::no_terminal_error`), then exits 1. It never switches to `-p` on its
own — a run in which nobody can approve anything is the caller's choice.
`init` has already unwound the modes it set by then.

```text
error: could not start the interface: No such device or address (os error 6)
It needs a terminal; to run without one (cron, CI, a script), use print mode:
alter-zero -p "your prompt"
```

## What the references do (findings)

- **Claude Code** — `claude -p "query"`: the same session, printed instead
  of shown; `--output-format text|json|stream-json`; `--max-turns` bounds
  the agentic turns (model replies), ending in a `result` whose `subtype` is
  `error_max_turns`; `--permission-mode`; `-c`/`--resume` continue a
  conversation; piped stdin rides into the prompt. The `json` result object
  (`type`, `subtype`, `is_error`, `result`, `session_id`, `num_turns`,
  `duration_ms`, `usage`, `permission_denials`) is what scripts already
  parse.
- **codex** — `codex exec` (read in its source, `codex-rs/exec`): stdout
  carries **only** the final message (or, with `--json`, JSONL events) and
  everything else goes to stderr — a rule its crate enforces with
  `deny(clippy::print_stdout)`. Approval is forced to *never*: whatever would
  ask is refused and the model is told. The JSONL stream is item-shaped —
  `thread.started`, `item.started` / `item.completed` with `item_N` ids
  counted per process, `turn.completed {usage}`, `turn.failed {error}` — and
  carries no deltas: an agent message appears whole, once. A positional
  prompt with a non-tty stdin gets the input appended as
  `{prompt}\n\n<stdin>\n{stdin}\n</stdin>`; no positional reads stdin as the
  prompt; `-` names stdin explicitly. Two of its pitfalls this port avoids:
  the stdin read blocks until EOF, so an **open pipe that is never closed —
  n8n's command node, `docker run -i`, `ssh` without `-n` — hangs the run
  forever**; and an interrupted turn ends with no terminal JSON event at all.

## The shape: the same session, a different shell

Headless runs **the same `Session`** the TUI runs — the same bootstrap, the
same `start_turn`, the same stream folding, the same notice settling and
follow-up turns, the same hooks, recorder, compaction and subagents — over a
viewport that paints nowhere (`InlineViewport::headless`, a sink with a fixed
size whose commits are dropped as they are queued), with **no `EventStream`
at all**. What it replaces is the terminal half of the loop: `tui::headless`
selects over the reply, background, subagent and MCP channels plus the
process signals, and turns what the session records into output.

The alternative — a lean runner calling `App` and the backend directly —
would have re-implemented the TUI's event folding: where a notice settles,
how an agent group resolves after its members' events drain, what a failed
turn records, how a retried attempt is withdrawn. Every one of those rules is
a bug someone already paid for, and a second copy of them is a second place
for each to come back. "The CLI path can never drift from the typed one"
(`docs/cli/cli.md`) is the rule `submit_startup_prompt` was built on; headless
keeps it by construction. The price is rendering work into a sink that
nobody reads — the commits are dropped before they are drawn, and the Ctrl+O
transcript is never warmed.

## Invocation

```
alter-zero -p [OPTIONS] [PROMPT]
```

| Flag | Meaning |
| --- | --- |
| `-p`, `--print` | Run headless: answer, print, exit. |
| `--output-format <FORMAT>` | `text` (default), `json` or `stream-json`. Needs `-p`. |
| `--max-turns <N>` | Stop after **N model replies** of the lead agent, counted over the whole run. Needs `-p`. |
| `--verbose` | Log each step of the run to stderr. Needs `-p`. |
| `--permission-mode <MODE>` | `manual`, `edit`, `auto` or `master` for this run. Both modes. |
| `-C`, `--cwd <DIR>` | Run as if started in `DIR`. Both modes. |
| `-c`, `--continue` / `-r`, `--resume <ID>` | Send the prompt into that conversation (as without `-p`). |

The grammar is `cli::parse`'s (`docs/cli/cli.md`): `-p` is a switch and the prompt
stays the one positional, so `alter-zero -p "…"` reads the way Claude Code's
does. A usage error exits `2` with the clap-shaped trailer: `--output-format`,
`--max-turns` or `--verbose` without `-p`, an unknown format or mode, `--max-turns 0`, a
flag given twice, and `-p` with the bare `--resume` — the picker is a screen,
so a headless run must name its session.

`--permission-mode` and `-C` work in the TUI too: the first is the run seed
`ALTER_ZERO_PERMISSION_MODE` already was (it outranks that variable and the
project's saved mode, and is never saved); the second is `chdir` before
anything reads the working directory — which is why it runs before
`--continue` looks for the newest conversation *there*.

`--max-turns` counts what Claude Code counts — **model replies** (requests
the lead agent sends), not alter-zero's turns (a turn is a whole exchange,
however many replies its tool loop takes). Each request takes one from a
budget shared by the prompt's turn and every follow-up turn the run starts;
subagents' requests and a compaction's are not counted. When it is spent the
next request is never sent: the turn ends on `Stopped after N model replies —
the run's --max-turns limit`, the run exits `1`, and the result's `subtype`
is `error_max_turns`. The library half is `llm::agent::RoundBudget`, attached
with `LlmBackend::with_round_budget` (re-attached on every rebuild, like the
registries) and checked in the round closure, where `RoundOutcome::Limit` ends
the turn exactly as the tool-call ceiling does — its `StopFailure` hook
included.

### The prompt: the argument, stdin, or both

| Argument | stdin | Prompt |
| --- | --- | --- |
| given | a terminal | the argument |
| given | a file (`< task.md`) | the argument, then the file as a `<stdin>` block |
| given | a pipe (a shell's `\|`) | the argument, then the pipe as a `<stdin>` block, read to its end however slow its writer |
| given | a socket (what Node starts commands with — n8n) | the argument, then the socket as a `<stdin>` block — **if it says anything within 3 s** |
| none, or `-` | not a terminal | stdin, read to its end |
| `-` | a terminal | what is typed there, until Ctrl+D — `cat`'s rule |
| none | a terminal | usage error: nothing to answer |

The `<stdin>` block is codex's, because a model told *what* came from a pipe
reads a diff as a diff: `{prompt}\n\n<stdin>\n{input}\n</stdin>`. Blank input
adds nothing.

A **pipe** is never cut off. A shell's `|` ends when its writer does, so
waiting for it loses nothing — `./slow-tests.sh 2>&1 | alter-zero -p
"summarize the failures"` sends the whole log however long the tests stay
quiet first — and a run that cut it off after a few seconds would answer the
bare instruction with exit 0, the worst way to fail. The cost is a parent
that holds a pipe open and never writes to it (Python's `Popen(stdin=PIPE)`
never closed): the run waits for it, and after `STDIN_NOTE_AFTER` (10 s)
says on stderr that it is still reading stdin and that `< /dev/null` stops
it.

A **socket** — anything that is neither a terminal, a file nor a pipe —
beside an argument gets the 3 s guard instead (`STDIN_WAIT`, Claude Code's
rule). Node's `spawn`/`exec` give the commands they start a socket for
stdin, and n8n's Execute Command node starts its command that way and never
closes it, which would otherwise hold every run forever (codex's hang):
`poll` waits up to 3 s for the socket to become readable — data *or* EOF —
and one silent that long is left alone, with a note on stderr. Once it says
anything it is read to its end. A regular file never waits, and without an
argument stdin *is* the prompt, so the read waits for its end exactly as
asked.

Input is read straight from the descriptor (never through std's buffer, so
`poll` sees everything still to come), at most `STDIN_MAX_BYTES` (8 MiB —
beyond any context window; the error says to save it to a file and name the
file instead). It is decoded as UTF-8 with a BOM dropped, and a byte that is
not UTF-8 reads as `U+FFFD` with a note on stderr: a log with one stray
byte is still worth reading.

## What the run does

1. **Before the runtime**: `cli::parse`, `chdir` (`-C`), `--continue`/
   `--resume` resolved (fail fast, exit 1), the prompt read and combined.
2. **Bootstrap**, headless: the same assembly as the TUI's, minus the screen.
   No `EventStream`; the startup review is never opened (see *Trust*); no
   telemetry ping, update check or herdr report; nothing joins the ↑ input
   history; a `--continue`/`--resume` load **does not restore the working
   tree** to the conversation's last checkpoint (see *Resume*). An error
   toast — a `SKILL.md` that would not parse, a provider file's problem —
   is written to stderr as an `alter-zero: …` line; an info toast only with
   `--verbose`.
3. **No model, no run**: when no provider, model and key resolve, the TUI
   falls back to the offline demo. A headless run refuses instead (exit 1,
   naming `/login` and the `ALTER_ZERO_PROVIDER`/`ALTER_ZERO_MODEL`
   variables) — a CI job "succeeding" on a canned demo reply is the worst
   failure a misconfiguration could have. `ALTER_ZERO_DUMMY=1` still forces
   the demo, which is how the tests run offline.
4. **MCP servers settle first**: the TUI connects servers in the background
   and a turn simply runs without the ones still connecting; a headless run
   is over in seconds, so it waits until no server is `Pending` (each is
   bounded by its own startup timeout) before the first request.
5. **The prompt is submitted** through the same path as the `[PROMPT]`
   shortcut — redacted against the secrets, then `start_turn`. A
   `--continue`d conversation already past its window compacts first: the
   TUI's idle-boundary auto-compaction (`docs/architecture/compact.md`), run once before
   the prompt and afterwards only while the run waits on its subagents —
   never after the last answer, where it would only cost a request.
6. **The loop runs until the session is idle**: no turn in flight, nothing
   queued, no subagent running. A background agent's completion starts the
   follow-up turn the TUI would start, so "launch the critic agent, write a
   report" reports. That wait is bounded: once nothing but background agents
   is left, the run waits on them at most
   `ALTER_ZERO_HEADLESS_AGENT_WAIT_SECS` (default 600, `0` for no wait), then
   stops the ones still working, names them on stderr and ends on its last
   reply — an agent that never finishes cannot hold a cron job forever. A
   notice already in hand is never cut off: its follow-up turn is coming.
   Background **shells** are not waited for — a dev server started with `&`
   never exits — so any still running then are stopped and named on stderr;
   the system prompt tells the model as much (below).
7. **Shutdown** is the TUI's: the backend cancelled, shells and agents
   killed, `SessionEnd` fired (reason `other`), MCP children reaped, the
   rollout flushed. Then the result is printed and the process exits.

### What the model is told

The system prompt gains a fourth block, `prompts/headless.md`, appended by
`backend::augment_with_headless` after the scratchpad's:

```
## Headless

This run is non-interactive no one can answer questions or approve actions
A call that needs approval is refused work around it or report what you needed
The run ends when your turn ends background commands still running are stopped
Your final message is the run's output
```

`AskUserQuestion` is not offered at all (the ask gate is never attached), so
the model cannot wait on an answer that will not come.

### Permissions: nothing is asked

The gate is the same, and so is everything that answers before a prompt —
the project's allowlist (`permissions.json`), the scratchpad exemption, the
read-only fast path, a `PermissionRequest` hook, auto mode's classifier. What
changes is the last step: a headless gate is **unattended**
(`PermissionGate::set_unattended`), and where `approve_call` would raise the
prompt it refuses instead — no request, no wait, no `Notification` hook,
since nobody is shown anything. The model reads
`permission::UNATTENDED_RESULT`: the call did not run, nobody can approve it,
carry on without it or say what was needed. The cell reads `Not run: needs
approval (headless run)`. Subagents share the gate, so theirs are refused the
same way.

A **failed safety check** is told apart. When auto mode's classifier errors
before it reaches a verdict — the network, a rate limit, a reply it cannot
read — the TUI falls back to asking; a headless run cannot, but a failure is
not a denial, and telling the model "do not retry" would make a passing
outage quietly cost the run its work. So the model reads
`permission::CHECK_FAILED_RESULT` instead: the check failed with an error,
nobody else can approve the call, it may retry **this exact call once**, and
if the check fails again it must not. The gate remembers each call whose
check failed (`PermissionGate::first_check_failure`, keyed on the call's
kind and target), and a second failure on the same call answers
`CHECK_FAILED_AGAIN_RESULT` — failed again, do not retry — with no
invitation left, so a check that keeps failing cannot hold a model in a
loop whatever it does. The cells read `Not run: the safety check failed
(headless run)` and `… failed again …`; every attempt is a refusal in
`permission_denials`.

The mode decides how much reaches that step. `master` runs everything;
`auto` (the default, unless the project saved another) runs edits, read-only
commands and whatever the classifier allows; `edit` refuses commands nobody
allow-listed; `manual` refuses every change. A rule saved interactively ("don't
ask again for `cargo *`") is honoured, so a project can be prepared once in
the TUI and then run unattended.

Every refusal is collected (`PermissionGate::take_unattended_refusals`): the
result's `permission_denials` lists them, and the text log names them with
the flag that would have allowed them. A refusal does not fail the run — the
model was told and answered; a script that wants to fail on one reads the
list.

### Trust

The startup review (`docs/state/config-review.md`) is a screen, so it never opens:
a hook or a project MCP server nobody has trusted **stays off**, exactly as
it would after the review was declined, and stderr says what is waiting and
that `alter-zero` run interactively in that directory asks about it. A
headless run never trusts anything on its own.

### Resume

`-c` and `--resume <ID>` load the conversation, its model and its rollout as
the TUI does, and the prompt is its next turn. Two differences. The working
tree is **not** restored to the conversation's last checkpoint: in a
pipeline the tree is whatever the pipeline made it (a `git pull`, a build),
and reverting it to last night's state under a cron job would undo that
silently. And nothing to continue is still an error (exit 1), as without
`-p` — a scripted `-c` must not quietly start a new thread; the first run of
a cron job omits `-c`, or names its session with `--resume`.

Every headless run records its rollout like any session, so the `session_id`
in the output — or `--continue` — picks it up again, headless or in the TUI.

## Output

**stdout** carries the answer and nothing else (codex's rule): in `text` the
final answer and a newline; in `json` one result object; in `stream-json`
one event per line, the result last.

**stderr** is quiet unless something needs saying — Claude Code's posture,
and the one a cron job wants, since cron mails whatever a job writes. A run
that went well writes nothing there. What it always says: why a run failed
(`alter-zero: HTTP 401: …`, `interrupted by SIGTERM`, the `--max-turns`
limit), the calls nobody could approve, the background work it stopped,
stdin it waited on or left alone, an outage it is waiting out, the hooks and
MCP servers held for review, an error toast. **`--verbose`** adds the run's
step log below, in every format: the header, each tool call and its result,
the narration, a retry, an info toast, the closing footer.

The **answer** is the run's last reply: the assistant messages after the last
thing that was not one (a tool call, a notice, a hook's note, the prompt) in
the history this run added, joined by a blank line — so a Stop hook's
continuation answers with its own reply, and narration before a tool call is
never mistaken for the result.

### `text`

```
$ alter-zero -p --verbose "Run the tests and fix what fails"
alter-zero 0.18.0 · gpt-5.5 · ~/proj · auto · session 18a9f2c33d41e5b6-1a2b   ← stderr
● Bash(cargo test)                                                           ← stderr
  ⎿ exit 101 · 240 lines
I'll fix the empty-input case first.                                         ← stderr
● Read(src/parse.rs)
  ⎿ read 120 lines
● Edit(src/parse.rs)
  ⎿ Updated src/parse.rs (+3 -1)
● Bash(cargo test)
  ⎿ exit 0 · 238 lines
Done in 1m 12s · 5 model replies · 48k tokens (31k cached)                   ← stderr
Fixed the empty-input panic in parse.rs; all 238 tests pass.                 ← stdout
```

Without `--verbose` the same run writes only its last line, to stdout. The
log is plain text — no colour, no cursor movement — so it reads the same in
a CI log, a cron mail and a terminal. A reply the run went on from is
narration and goes to the log; only the answer goes to stdout.

### `json`

```json
{"type":"result","subtype":"success","is_error":false,"result":"Fixed the …",
 "session_id":"18a9f2c33d41e5b6-1a2b","model":"gpt-5.5","num_turns":5,
 "duration_ms":72113,"usage":{"input_tokens":46100,"output_tokens":2100,
 "cached_input_tokens":31000,"cache_write_input_tokens":0},
 "permission_denials":[]}
```

`subtype` is `success`, `error_max_turns`, `error_during_execution` (the turn
failed — the provider, a hook that blocked the prompt) or `error_interrupted`
(a signal); `is_error` is every one but the first, and `error` carries the
message when there is one. `num_turns` is the model replies the budget
counted. `usage` is the provider's own accounting, subagents included.

Each `permission_denials` entry is Claude Code's: the tool as the model
called it, the call's id and its input, plus the subagent type that asked
when it was not the lead —
`{"tool_name":"bash","tool_use_id":"call_7","tool_input":{"command":"rm -rf build"},"agent":"explore"}`.
The gate records the call at the moment it refuses it
(`permission::UnattendedRefusal`), so the input is exactly what the model
sent — its placeholders, never a secret's value.

A run that **never started** — no model configured, nothing to continue, an
unknown `--resume` id, stdin that could not be read, a session that would not
bootstrap — still prints a result in `json` and `stream-json`
(`error_during_execution`, `session_id` and `model` null, `num_turns` 0), so
a script piping to `jq .result` meets an error object rather than nothing.

### `stream-json`

codex's item stream, closed by the result:

```
{"type":"session.started","session_id":"…","model":"gpt-5.5","cwd":"/home/u/proj","permission_mode":"auto","resumed":false}
{"type":"item.completed","item":{"id":"item_0","type":"user_message","text":"Run the tests and fix what fails"}}
{"type":"item.started","item":{"id":"item_1","type":"tool_call","name":"bash","summary":"cargo test","arguments":{"command":"cargo test"},"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"tool_call","name":"bash","summary":"cargo test","arguments":{"command":"cargo test"},"status":"failed","output":"Exit code: 101\n…"}}
{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"I'll fix the empty-input case first."}}
…
{"type":"turn.completed","duration_ms":71000,"usage":{"total_tokens":48200,"cached_input_tokens":31000,"cache_write_input_tokens":0}}
{"type":"result","subtype":"success",…}
```

- **Items** are the history the run records, in its order, ids `item_N`
  counted per process: `user_message`, `agent_message`, `reasoning` (only
  when the thinking display is on), `tool_call` (every tool — `bash`,
  `write`, `mcp__…`, the task tools — with the model's own `arguments`, the
  header's `summary`, the `output` the **model** read, and an approval
  `note`; `status` is `completed`, `failed`, `declined` — it never ran — or
  `background`; its `name` is the one the model called — `bash`,
  `mcp__github__create_issue` — never the screen's `Bash` label),
  `subagents` (a launched group: each agent's id, type,
  status and result), `background_notice` (its `shell_id`), `agent_notice`
  (its `agent_id`), `compaction`, `hook` and `notice`. No item has a field
  of its own named `id`: the envelope's `id` is flattened in beside the
  item's fields, and a second one would be a duplicate key — codex's
  `web_search` items carry exactly that bug.
- A tool call is **started** when it starts and **completed** when it
  resolves, under one id; everything else appears once, whole — there are no
  deltas, codex's rule.
- **Nothing is ever printed that the session later takes back**: a reply
  attempt a lost connection voids is withdrawn from the history
  (`docs/providers/offline.md`), so items past `App::settled_history_len` — what such a
  retry could still withdraw — wait until the attempt ends.
- A turn ends in `turn.completed` (its summary's duration and tokens) or
  `turn.failed`; an interrupted run still ends in a `result` — codex's
  missing terminal event.

## Exit status

| Status | When |
| --- | --- |
| `0` | The run finished and its last turn completed. |
| `1` | The last turn failed (provider error, a blocked prompt), `--max-turns` ran out, no model is configured, the prompt could not be read, nothing to continue / unknown session — each said on stderr, and in the JSON formats as the result. |
| `2` | Usage error. |
| `130` / `143` | Interrupted by `SIGINT` / `SIGTERM`. |

A signal is handled, not just suffered: it cancels the turn and runs the
shutdown above — shells live in their own sessions (`setsid`) and MCP
servers are children, so dying on the default action would orphan them —
then prints the result (`error_interrupted`, a tool call still open
completed `failed` ahead of it) and exits `128 + signal`. The handlers are
installed before bootstrap starts any server, and the shutdown is bounded
(`SessionEnd` has its two seconds), so nothing outlives the run.

## Not in headless mode, on purpose

- **No telemetry ping and no update check.** The ping's rule is that its
  notice is shown before any send (`docs/ops/telemetry.md`); a run with no screen
  shows nothing, and CI machines are where a phone-home is least welcome.
- **No input history.** An automated prompt is not something the user typed;
  ↑ in the TUI should not walk through a cron job's prompts.
- **No `--output-schema`, `-o FILE`, `--ephemeral` or `--model`** yet:
  stdout redirection covers `-o`, and `ALTER_ZERO_PROVIDER`/
  `ALTER_ZERO_MODEL` already pin the model for a run.

## Testing

- `cli`: every flag's spellings, `=`-attached values, the `-p`-only flags
  (`--verbose` among them) refused without `-p`, unknown values listing the
  choices, `--max-turns 0`, a flag given twice, `-p` with the bare picker,
  the help page pinned.
- `headless`: the format labels; the stdin rule per kind (a pipe read to its
  end, a socket cut off at 3 s beside an argument) and the lossy decode;
  `combine_prompt`'s table; the answer extraction (narration, continuation,
  a failed turn, a resumed history's older replies never counted); the
  outcome and exit status; every item mapping and event line, tools named
  as the model called them; the quiet default and `--verbose`'s log in every
  format; the toast rule; a denial in Claude Code's fields; a run that never
  started; the agent wait and the agents it stopped.
- `permission` / `llm::approval`: an unattended gate refuses where it would
  have asked — after the allowlist, the scratchpad, the read-only fast path,
  a hook and the classifier have had their say — sends no request, and
  records the refusal with the call as the model made it.
- `stream` (the offline demo): an unattended gate refuses the demo's gated
  calls, the lead's and its subagent's, without raising a request.
- `llm::agent` / `llm::backend`: `RoundOutcome::Limit` ends the turn on the
  error with no `StreamDone`; the budget counts across turns and refuses the
  request past its limit.
- `tests/round_budget.rs`: a real `LlmBackend` against a loopback stand-in
  — a budget of one reply never sends the second request, one that covers
  the turn counts every reply, and the budget spans every turn of the run.
- `tests/headless.rs`, the real binary with **no terminal anywhere** (every
  stream a pipe): the offline demo's answer on stdout, nothing on stderr,
  and `--verbose`'s log beside the same stdout, no escape on either stream;
  `json` and `stream-json` parsed line by line; a prompt from stdin, an
  argument plus a `<stdin>` block, a pipe that stays silent past 3 s waited
  for and its data used, an open socket left alone after the wait; nothing
  to answer and a `-p`-only flag without `-p` as usage errors; `-C`; `-c`
  continuing the run before it; no model configured refused, and with
  nothing to continue or an unknown id, still a JSON result; a SIGTERM
  mid-run exiting 143 with an `error_interrupted` result. Against a local
  stand-in provider (a few lines of `std::net`) whose model asks for one
  `bash` call: `manual` refuses it, tells the model and lists the denial in
  Claude Code's fields; `master` runs it; `--max-turns 1` stops before the
  second reply; the system prompt carries the headless block and the
  request no `askuserquestion`; a background subagent is waited for and its
  completion answers the run, and one that never finishes is stopped once
  the wait runs out; a command left running is stopped and named.
- `scripts/smoke.sh` Phase 153: `alter-zero -p --verbose` from a tmux pane —
  the log and the answer on the terminal, its modes unchanged (`stty -g`), and none
  of the TUI's escapes in the raw byte stream (no DSR, no alternate screen,
  no hidden cursor, no bracketed paste, no synchronized update); and the TUI
  launched with `--permission-mode master -C <dir>` naming both in its
  footer.

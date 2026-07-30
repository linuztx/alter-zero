# Tool permission requests

Claude Code asks before it changes anything: a `write`, an `edit`, or a `bash`
command stops the turn and puts an inline approval prompt where the composer
was. This is that, ported to the inline TUI — the pure request/decision
vocabulary in [`crate::permission`], the modal prompt state in
`app/permission.rs`, its rendering in `ui/permission_view.rs`, and the
cross-thread handshake in [`crate::permission::PermissionGate`].

```
● Write(hello.py)

────────────────────────────────────────────────────────────────────────

 Create file
 hello.py
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
  1 #!/usr/bin/env python3
  2
  3 def main():
  4     print("hello")
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 Do you want to create hello.py?
 ❯ 1. Yes
   2. Yes, allow all edits during this session (a)
   3. No

 Esc to cancel · Tab to amend

────────────────────────────────────────────────────────────────────────
```

## The shape

Three kinds, one layout, per `permission::PermissionKind`:

| kind    | title          | body                              | question                              |
| ------- | -------------- | --------------------------------- | ------------------------------------- |
| `Write` | `Create file`  | the numbered new contents         | `Do you want to create {file}?`       |
| `Edit`  | `Edit file`    | the numbered diff hunks           | `Do you want to make this edit to {file}?` |
| `Bash`  | `Bash command` | the indented command + description | `Do you want to proceed?`             |

A `write` whose target **already exists** is an `Edit` — it shows the diff, not
the whole file, exactly as the resulting `Updated {path} (+A -D)` cell will.

The body is the same numbered, syntax-highlighted, `+`/`-`-tinted block the
finished `write`/`edit` cell renders (`ui/file_cell.rs`'s
`numbered_body_lines`, shared by both) — the whole file/diff, not a peek: the
point of the prompt is that you read what you are approving. Only when the
prompt would not fit the terminal is the body capped, with the familiar
`… +N lines` tail; the cap is derived from the terminal height so the options
and the hint row are always visible.

A request raised by a **subagent** (`docs/agent-tool.md`) says so in the title:
`Create file · from the general-purpose agent`.

## What stays on screen

A prompt is a question *about something*, so the modal never hides what raised
it. The live cells above it survive: the call being asked about, any batch
siblings queued behind it, and — for a subagent's request — the round's whole
live agent tree (`● Running 3 agents…` and its rows). Everything else in the
live region gives way: the status line (nothing is running; the turn is blocked
on you), the composer, the bands, and the footer.

The call under the prompt renders as its **header alone** — `● Write(hello.py)`,
no `⎿ Waiting…`. It is not waiting on a queue, it is waiting on you, and the
prompt directly below already says so; the siblings behind it keep their
ordinary `⎿ Waiting…` rows. For the same reason `App::command_elapsed` reads as
`None` while a prompt is open, which drops the delayed
`(ctrl+b to run in background)` hint: the prompt owns every key, so that one
would be advertising a binding it swallows.

## The options

Every prompt offers three, selected with ↑/↓ + Enter or by typing `1`/`2`/`3`:

1. **Yes** — approve this call only.
2. **Yes, allow all edits during this session (a)** for `write`/`edit`;
   **Yes, and don't ask again for: {prefix} (a)** for `bash`. Also bound to `a`
   (`shift+tab` already cycles the thinking mode — `docs/reasoning.md`).
3. **No** — reject; the model is told to stop and wait.

Plus, on the hint row:

- **Esc** — cancel: reject *and* interrupt the turn (the ordinary Esc-interrupt
  path, `docs/interrupt.md`).
- **Tab** — amend: the option list is replaced by the composer, so you can type
  what the model should do instead. Enter rejects **with that feedback**
  attached to the tool result; Esc goes back to the options.
- **ctrl+e** (`bash` only) — explain: reject with an instruction to explain what
  the command does and ask again, instead of running it.

### What "don't ask again" remembers

`permission::command_scope` reduces a command to the keys the session allowlist
stores:

- The command is split into segments on `|`, `||`, `&&`, `;`, `&` and newlines
  (quotes respected), and each segment is reduced to its **prefix**: the first
  token, plus the second when it isn't a flag — `git status --short` → `git
  status`, `ls -la` → `ls`, `python3 script.py` → `python3 script.py`.
- A later command is auto-approved only when **every** one of its segment
  prefixes is allow-listed, so allow-listing `python3 script.py` never silently
  admits a `; rm -rf /` tail.
- Approving stores every segment's prefix (so the identical command never asks
  twice) while the option's **label** names the last one — the command the user
  reads as the action, matching Claude Code (`echo "" | python3 script.py`
  offers `python3 script.py`).
- A segment carrying a redirect or a substitution (`>`, `<`, `` ` ``, `$(`)
  cannot be summarized by a prefix — the prefix wouldn't mention the part that
  matters — so the scope degrades to `Exact`: the label and the stored key are
  the **whole command**, and only a byte-identical command is ever
  auto-approved.

Approving with option 2 also **sweeps the requests already waiting**. Parallel
agents raise theirs before any of them is answered — each thread consulted the
rules before the first prompt was even drawn — so without the sweep three agents
running the same command would ask three times *after* you said not to. The loop
remembers the scope, then releases every open/queued request the new rule now
covers (`App::drain_covered_permissions`, given the gate's own `allows`); one
that isn't covered still asks.

There is no built-in safe-command list. Every `bash`, `write`, and `edit` asks
until the session allowlist says otherwise — Claude Code's default posture, and
the only one that can't be wrong about what is safe. `read` is never gated.

## The handshake

The tool runs on the backend thread; the prompt lives in the event loop. The
round trip is a `PermissionGate` — the `Arc<Mutex<…>> + Condvar` sibling of
`background::BackgroundRegistry` and `agents::AgentRegistry`:

```
run_agent (backend thread)          event loop                     user
  approve(call) ─┐
                 ├─ gate.allows()? ── yes ──────────────────────────► (runs)
                 └─ no
                    tx.send(StreamEvent::Permission(request)) ──► App::open_permission
                    gate.wait(id, cancelled) …blocked…            (composer draft stashed)
                                                    ◄── Action::ResolvePermission ── 1/2/3
                    ◄─────────────── gate.resolve(id, decision)
   Allow  → ToolStart … ToolEnd
   Reject → ToolStart + red ToolEnd(display), the model gets `result`
```

`wait` polls its condvar on a short timeout and gives up the moment the turn's
`CancelToken` trips, so Esc / `/clear` / quit reap a blocked tool thread
promptly instead of wedging it.

`run_agent` gained one seam for this — `approve: FnMut(&ToolCallRequest) ->
Approval` — called immediately **before** each ordinary call's `ToolStart`, so
nothing has run and nothing has been announced as running when the prompt
appears. A rejection still emits the `ToolStart`/`ToolEnd` pair, so the call
lands in history and the transcript as a red cell (`⎿ User rejected write to
hello.py`) while the *model* reads the longer "stop and wait" text: the display
string and the tool result are separate fields of `Approval::Reject` precisely
so the cell can be short and the instruction complete.

Subagent calls take the same seam — `spawn_subagent_run` passes the agent's type
as the request's `agent`, and the event rides the agent channel's forwarder, so
`on_agent_event` opens the same prompt.

## The prompt is modal, and the draft survives

`App::permission` is checked first in `on_key`, ahead of the Ctrl+R search and
the inline pickers: while a request is open it owns every key. `render_live`
replaces the **whole** live region with it, the streaming strip included — the
turn is blocked on you, so there is nothing to animate.

Opening stashes the composer (`TextArea` text + cursor, and the `!` shell-mode
flag) and clears it; closing restores them. So a request that lands mid-sentence
does not eat what you were typing — and Tab's amend field starts empty, because
it *is* the same textarea. A second request arriving while one is open queues
(`App::pending_permissions`) and opens as soon as the first resolves; its
backend thread simply stays blocked meanwhile.

`/clear`, an interrupt, and a quit all drop the prompt and the queue; the
blocked threads notice their cancel token and return.

## Turning it off

`ALTER_ZERO_PERMISSIONS=0` (or `false`/`no`/`off`) starts the session with no
gate attached, and every tool runs as it did before this feature. The
`LlmBackend` only asks when a gate was installed, so an embedder (and the live
integration tests) that builds a backend directly is unaffected.

## Tests

- `permission.rs` — the pure vocabulary: titles, questions, option labels,
  `command_scope`'s segmentation/prefixing/degradation, the rules' allow +
  remember, and the gate's blocking round trip (real threads).
- `app/tests` — opening stashes and closing restores the draft, the key map
  (↑/↓/1/2/3/a/Tab/Esc/ctrl+e), the amend field, the queue.
- `ui/tests` — the rendered prompt: rules, coloured title, the agent suffix, the
  numbered/diff body, the cyan `❯` on the selection, the hint row, the live
  cells kept above it (header-only for the pending call, `⎿ Waiting…` for its
  siblings, the whole tree for a subagent's), and that `permission_height`
  equals the painted rows — at every height, the context rows included.
- `smoke.sh` Phase 55 — the whole round trip against the dummy backend in a real
  terminal: draft typed, prompt shown, `2` approving, draft restored.

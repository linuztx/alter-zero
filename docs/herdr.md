# herdr support — the pane's state, reported by the agent itself

**Run alter-zero in a [herdr](https://herdr.dev) pane and the sidebar tells
the truth about it: working while a turn runs, blocked the moment a
permission prompt or a question waits on you, idle — "done" until you look —
when the turn ends, and back where it was after a herdr restart.** Nothing to
install or configure: herdr sets three variables on every process in a pane,
and when the app finds them it reports to herdr's local socket on its own.

herdr is a terminal multiplexer for coding agents. It recognises the agents it
ships rules for by their process and their screen; it has none for
alter-zero, and a third-party agent cannot add screen rules (herdr's manifest
overrides key on the agents it already knows). What it offers instead is
self-reporting — herdr's *Add Herdr support* page — and a self-reporting agent
is the pane's only authority: no screen scraping, no hooks file, no
`herdr integration install`. The pure half is `src/herdr.rs`; the boundary is
`src/tui/herdr.rs`. Everything below was checked against herdr 0.9.3's source
(the latest release, 2026-09-29), not only its docs.

## What the sidebar shows, and when

The state is **derived from the app, never tracked as events** —
`herdr::status(&App)`, evaluated at every loop bottom after everything that
iteration did:

| herdr state | while |
|---|---|
| `blocked` | A permission prompt or an `AskUserQuestion` modal is open (`App::modal_open`) — whoever raised it, a background subagent included. |
| `working` | A turn is in flight — a reply, a `!` command, a `/compact` or `/init` — or any subagent is still running. |
| `idle` | Otherwise. |

herdr derives the fourth state itself: **done** is idle the user has not
looked at yet. A move from working or blocked to idle is a *completion* —
the "finished" toast and sound when the pane is not in view — and entering
blocked is the "needs attention" one. Three consequences shaped the rules:

- **No idle between chained turns.** Because the state is read at the loop
  bottom, after the turn end has already dispatched whatever comes next —
  a queued message, a background notice's follow-up turn, an auto-compaction
  — a chain of turns reports one `working` and one `idle`, never a false
  "finished" in the middle of it (the job herdr's own reporters do with a
  250 ms debounce or Pi's `agent_settled`).
- **Running subagents are work.** The lead's turn can end with agents still
  at it, and their results start the next turn by themselves; reporting idle
  there would announce "finished" while the work goes on, and again when it
  really ends. This deliberately differs from herdr's newest rule for its
  own Grok and agy support, which counts anything left running at the prompt
  as idle — because there a background *monitor* can run forever and hung
  `herdr agent wait` for an hour. A subagent always finishes; a background
  **shell** may not (a dev server runs until stopped), so shells never count,
  which is herdr's own Claude rule.
- **An error ends the turn idle.** The agent is back at the prompt; a red
  blocked would stand until the next turn with nothing for the user to
  decide. The "finished" notification still brings them to the error.

A blocked report carries the prompt in one line — `Bash command: cargo test`,
`Create file: hello.py`, `Tool use: Deepwiki - ask_question`, `Question: Which
database? (+2 more)` — the prompt's own title and what it names, cut at 160
characters. herdr 0.9.3 stores the message but shows it nowhere yet; it is
sent because the docs ask for it, and costs a few bytes.

## The session, and coming back after a restart

Once the conversation has a rollout file (the first message), every report
names its id — the one `--resume` takes — and the command that brings it back:

```
alter-zero --resume 18dbf0f64f371a36-4473
```

herdr keeps the newest such command per pane and, after a **cold restart** of
its server (with `resume_agents_on_restore`, the default), types it into the
restored pane's shell in the pane's own directory. The first word is the name
the app was invoked by (`argv[0]`'s basename) and is offered only when an
executable of that name is on `PATH`, since that shell is where herdr runs it;
a development build run by path reports no command. herdr validates the
command (`[A-Za-z0-9_.-]` first word not leading with `-`, no apostrophe or
control character anywhere, ≤ 64 words, ≤ 8 KiB) and refuses an invalid one
**together with the state it rode in with**, so `herdr::resume_argv` applies
the same rules first and leaves the command off rather than lose the report.
The command needs herdr ≥ 0.9.2; an older herdr ignores the unknown key.

`/resume` reports the new session at once (the report differs, so it is sent
even though the state did not change), and a `--resume`/`--continue` launch
names it on its very first report. `/clear` starts a conversation with no file
yet: until its first message, herdr keeps the previous command, since nothing
short of a release clears one. The session id itself (`agent_session_id`) is
discarded by herdr for a third-party source today; it is sent anyway, for the
day herdr shows it.

## The wire

herdr's API socket is an `AF_UNIX` stream socket at `$HERDR_SOCKET_PATH`
speaking **one request per connection**: one compact JSON line ending in
`\n`, one reply line back, then the server closes. A line that never ends is
dropped unanswered.

```json
{"id":"alter-zero:1791290217491819","method":"pane.report_agent","params":{"pane_id":"w1:p3","source":"alter-zero","agent":"alter-zero","state":"working","seq":1791290217491819,"agent_session_id":"18dbf0f64f371a36-4473","resume_argv":["alter-zero","--resume","18dbf0f64f371a36-4473"]}}
{"id":"alter-zero:1791290262462611","method":"pane.release_agent","params":{"pane_id":"w1:p3","source":"alter-zero","agent":"alter-zero","seq":1791290262462611}}
```

The reply is `{"id":…,"result":{"type":"ok"}}` or `{"id":…,"error":{"code":…,
"message":…}}` (`pane_not_found`, `invalid_resume_argv`, …). `source` and
`agent` are both `alter-zero`: herdr's own sources are `herdr:`-prefixed,
and an agent name herdr knows (or an alias of one) would be rewritten into
that agent and judged by its rules. Keys the app does not know are left out,
never sent as `null`; herdr ignores keys it does not know, so a newer field
never breaks an older herdr.

**`seq` is the whole ordering contract.** herdr keeps, per pane and per
source, the highest `seq` it has accepted, and drops any request that does not
exceed it — still answering `ok` — for the **lifetime of the pane**, across
the agent quitting and relaunching in it. So `herdr::next_seq` follows the
clock in microseconds (herdr's own reporters' unit) and steps one past the
last value when the clock has not moved on or went back; a relaunch outranks
the process before it because the clock has moved on. The id is
`alter-zero:{seq}`, unique for free.

Every turn, then, is a handful of requests — `working`, maybe `blocked` and
`working` again, `idle` — and a quit is one `release`, which tells herdr to
forget the agent, its state and its resume command and to read the screen
again. The release is sent only when the session ends — a quit (`/quit`,
Ctrl+C on an empty composer), or the terminal closing under it — never on
`/clear` or `/resume`, where the next report names the new session instead.
A process that dies without one is caught by herdr ≥ 0.9.2 within about a
second, once the pane's shell is back alone in the foreground.

## Never in the agent's way

herdr's own advice is "don't let Herdr slow your agent down", and the
boundary is built to it:

- **The loop never touches the socket.** `Session::sync_herdr` runs at every
  loop bottom (after the recorder's sync, so the first message's report
  already names the file it created), builds the `Report` — status, session,
  resume command — and posts it only when it differs from the last one
  posted: a compare and nothing else on the common path. Outside herdr there
  is no reporter at all: no thread, no I/O.
- **One worker, one request at a time, newest report only.** A detached
  thread drains a single-slot `herdr::Outbox`: a report the worker has not
  sent yet is replaced by a newer one, never queued behind it, and requests
  go out one at a time — herdr serves every connection on its own thread, so
  two in flight at once could be applied out of order. The worker owns the
  `seq` and stamps each send fresh.
- **Bounded, silent failure — and repair.** Each request's write and reply
  are bounded at 500 ms; a refused or failed send is retried after 1 s, 2 s,
  4 s … up to 30 s (`herdr::resend_after`), and a delivered one is re-sent
  every 30 s while nothing changes. herdr holds a self-reported state with no
  expiry, so without the resend a report lost to a timeout — or to herdr's
  live upgrade, which does not carry a third-party agent's state across —
  would stand wrong until the next change. A resend of an unchanged state
  notifies nobody; an absent herdr costs a failed connect every half minute.
- **The quit waits at most 400 ms.** `release` replaces any unsent report,
  nothing is reported after it, and the loop waits that long for the worker
  to write it before the process exits regardless. Against a socket nobody
  listens on, the connect fails at once and the quit is not delayed at all.

## Nested agents stay out of the pane

Every process in a herdr pane inherits its variables, the model's shell
commands included — and an agent run *by* the agent (`claude -p`, `codex
exec`, another alter-zero) would report into this pane under them: a
third-party report would fight this one for the state, and an official
herdr integration claiming the pane would lock alter-zero's reports out and
be what herdr resumes there after a restart. So `HERDR_PANE_ID` is one of the
variables the model's terminal sessions drop (`pty::spawn`'s
`FOREIGN_TERMINAL_VARS`, beside `TMUX_PANE` and `WEZTERM_PANE`, for the same
reason: it names the pane the TUI is drawn in, not the session's own
terminal — and `herdr … --current` from there would type into the TUI).
herdr's integrations all require a pane id, so they stand down.
`HERDR_ENV`, `HERDR_SOCKET_PATH` and `HERDR_BIN_PATH` stay, so the model can
still drive herdr itself — list panes, open a new one, run something there.
The user's own `!` commands, hooks and MCP servers keep the environment
unchanged: what they run is the user's call.

## Turning it off

`ALTER_ZERO_HERDR=0` (`false`/`no`/`off`, the usual grammar) turns it off for
a run: no reporter is started, nothing is sent, and herdr treats the pane as
an unknown program. There is no `/settings` row — outside herdr the feature
does nothing, and inside it the reports are the point of running there.

## Testing

`src/herdr.rs`'s unit tests pin the derived state against real `App` states
(a turn, a `!` command, a compaction, each modal, a running and a settled
subagent), the pane detection and its off switch, the request shapes field by
field, `seq` and the resend schedule, herdr's resume-command rules and the
outbox; `send` is tested against a real Unix socket — the reply read back,
a refusal surfaced, a silent server costing its timeout and no more, a missing
socket failing at once. `pty::spawn`'s environment test pins the
`HERDR_PANE_ID` drop. `smoke.sh` Phase 132 drives the real binary against a
stub socket that logs what it is sent: the launch's `idle`, a turn's
`idle working blocked working idle` with the prompt's message, the session
and its resume command from the first message on, strictly climbing `seq`s,
the release as a quit's last request, a `--resume` relaunch naming its
session on its first report and outranking the old release, nothing at all
with `ALTER_ZERO_HERDR=0`, and a dead socket costing a turn and a quit
nothing. The fixture unsets every `HERDR_*` variable, so a suite run from a
herdr pane never reports to the real one.

## Limits

- **Unix only.** herdr's Windows socket is a named pipe this integration does
  not speak; there, no reporter starts.
- **A herdr popup** has no pane id, so nothing is reported from one.
- **The blocked message is not shown** by herdr 0.9.3 (see above).
- **`/clear` leaves the previous resume command** in herdr until the new
  conversation's first message (see above).

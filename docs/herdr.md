# herdr support — the pane's state, reported by the agent itself

**Run alter-zero in a [herdr](https://herdr.dev) pane and the sidebar tells
the truth about it: working while a turn runs, blocked the moment a
permission prompt or a question waits on you — or a turn has failed — idle,
"done" until you look, when the turn ends, and back where it was after a
herdr restart.** Nothing to install or configure: herdr sets three variables
on every process in a pane, and when the app finds them it reports to herdr's
local socket on its own.

herdr is a terminal multiplexer for coding agents. It recognises the agents it
ships rules for by their process and their screen; it has none for
alter-zero, and a third-party agent cannot add screen rules (herdr's manifest
overrides key on the agents it already knows). What it offers instead is
self-reporting — herdr's *Add Herdr support* page — and a self-reporting agent
is the pane's only authority: no screen scraping, no hooks file, no
`herdr integration install`. The pure half and the socket worker are
`src/herdr.rs`; the boundary is `src/tui/herdr.rs`. Everything below was
checked against herdr 0.9.3's source (the latest release, 2026-09-29), not
only its docs, and the parts that decide behaviour were run against a real
herdr.

## What the sidebar shows, and when

The state is **derived from the app, never tracked as events**:
`herdr::activity(&App)` reads what the session is doing at every loop
bottom, after everything that iteration did, and `herdr::Tracker` turns it
into what herdr is told — first match wins:

| herdr state | while |
|---|---|
| `blocked` | A permission prompt or an `AskUserQuestion` modal is open (`App::modal_open`) — whoever raised it, a background subagent included. |
| `working` | A turn is in flight — a reply, a `!` command, a `/compact` or `/init`. |
| `blocked` | The last turn failed on a backend error — until a turn starts again, or the conversation is replaced or rewound (`/clear`, `/resume`, a backtrack). |
| `working` | A subagent is at work — running, or settled with a message or a Tab follow-up queued for it. |
| `idle` | Otherwise. |

herdr derives the fourth state itself: **done** is idle the user has not
looked at yet. A move from working or blocked to idle is a *completion* —
the "finished" toast and sound when the pane is not in view — and entering
blocked is the "needs attention" one. Those two notifications shaped every
rule:

- **No idle between chained turns.** Because the state is read at the loop
  bottom, after the turn end has already dispatched whatever comes next —
  a queued message, a background notice's follow-up turn, an auto-compaction
  — a chain of turns reports one `working` and one `idle`, never a false
  "finished" in the middle of it (the job herdr's own reporters do with a
  250 ms debounce or Pi's `agent_settled`).
- **Subagents at work are work.** The lead's turn can end with agents still
  at it, and their results start the next turn by themselves; reporting idle
  there would announce "finished" while the work goes on, and again when it
  really ends. An agent that settled with something queued for it counts
  too — a message its loop takes up, or a Tab follow-up the boundary hands
  over at the next tick (`herdr::agent_busy`) — or the moment between its
  settle and its next run would be a "finished" of its own. This
  deliberately differs from herdr's newest rule for its own Grok and agy
  support, which counts anything left running at the prompt as idle —
  because there a background *monitor* can run forever and hung `herdr agent
  wait` for an hour. A subagent always finishes; a background **shell** may
  not (a dev server runs until stopped), so shells never count, which is
  herdr's own Claude rule.
- **A failed turn stays blocked.** Idle would be a completion — herdr's
  "finished" — for a turn that died on a rate limit, a refused request or an
  expired key with its work half done. Blocked is herdr's "needs attention",
  which is what it is: the user has a decision to make (retry, switch model,
  sign in again), and the message says what happened —
  `Turn failed: HTTP 429: …`. The hold is released by the user moving past
  it: any turn starting (a retry, a queued message, a follow-up), or the
  conversation it ended leaving the screen — the tracker keeps the failed
  turn's history generation (`App::history_generation`, which a `/clear`, a
  `/resume` or a backtrack moves) and drops the hold once it changes, for
  good. An open modal still outranks it, and it outranks a subagent still at
  work beside it. Only the lead's turn holds: a subagent's failure is the
  lead's to read.

A blocked report carries what it waits on in one line — `Bash command: cargo
test`, `Create file: src/hello.py` (the path as the prompt's own target row
shows it), `Tool use: Deepwiki - ask_question`, `Question: Which database?
(+2 more)`, `Turn failed: …` — every run of whitespace and control characters
one space, so a terminal escape in a command line never reaches herdr, cut at
80 characters. herdr 0.9.3 stores the message but shows it nowhere yet; it is
sent because the docs ask for it, and costs a few bytes.

## The session, and coming back after a restart

Every report carries the command that brings the session back:

```
alter-zero --resume 18dbf0f64f371a36-4473
```

once the conversation has a rollout file (the first message), and `alter-zero`
alone before that and after a `/clear` — the fresh session the pane was.
herdr keeps the newest such command per pane and, after a **cold restart** of
its server (with `resume_agents_on_restore`, the default), types it into the
restored pane's shell in the pane's own directory. Nothing short of a release
clears a command herdr holds, so a report without one would leave the last
session's in place: a `/clear` followed by a herdr restart would bring back the
conversation the user had just cleared. The bare command replaces it.

The first word is the name the app was invoked by (`argv[0]`'s basename) and
is offered only when an executable of that name is on `PATH`, since that shell
is where herdr runs it — and herdr replays no scrollback for a pane with a
resume command, so a name the shell cannot find would cost the pane its
history *and* its session. A development build run by path reports no command
at all. herdr validates the command (`[A-Za-z0-9_.-]` first word not leading
with `-`, no apostrophe or control character anywhere, ≤ 64 words, ≤ 8 KiB)
and refuses an invalid one **together with the state it rode in with**, so
`herdr::resume_argv` applies the same rules first: a session id that breaks
one is left off, offering the fresh launch rather than losing the report.
The command needs herdr ≥ 0.9.2; an older herdr ignores the unknown key.

`/resume` reports the new command at once (the report differs, so it is sent
even though the state did not change), and a `--resume`/`--continue` launch
carries it on its very first report. No `agent_session_id` is sent: herdr
keeps one only from its own integrations.

## The wire

herdr's API socket is an `AF_UNIX` stream socket at `$HERDR_SOCKET_PATH`
speaking **one request per connection**: one compact JSON line ending in
`\n`, one reply line back, then the server closes. A line that never ends is
dropped unanswered.

```json
{"id":"alter-zero:1791290217491819","method":"pane.report_agent","params":{"pane_id":"w1:p3","source":"alter-zero","agent":"alter-zero","state":"working","seq":1791290217491819,"resume_argv":["alter-zero","--resume","18dbf0f64f371a36-4473"]}}
{"id":"alter-zero:1791290262462611","method":"pane.release_agent","params":{"pane_id":"w1:p3","source":"alter-zero","agent":"alter-zero","seq":1791290262462611}}
```

The reply is `{"id":…,"result":{"type":"ok"}}` or `{"id":…,"error":{"code":…,
"message":…}}` (`pane_not_found`, `invalid_resume_argv`, …). `source` and
`agent` are both `alter-zero`: herdr's own sources are `herdr:`-prefixed,
and an agent name herdr knows (or an alias of one) would be rewritten into
that agent and judged by its rules. Keys the app does not know are left out,
never sent as `null`; herdr ignores keys it does not know, so a newer field
never breaks an older herdr. `herdr::send` writes only to a socket this user
owns — the path's own entry, never followed through a symlink, since what is
checked must be what is connected to — because a report can carry the
command a prompt asks about. herdr's socket is its owner's alone anyway.

**`seq` is the whole ordering contract.** herdr keeps, per pane and per
source, the highest `seq` it has accepted, and drops any request that does not
exceed it — still answering `ok` — for the **lifetime of the pane**, across
the agent quitting and relaunching in it. So `herdr::next_seq` follows the
clock in microseconds (the unit herdr's own plugin reporters count in; its
shell hooks count nanoseconds, which is fine because the mark is per source)
and steps one past the last value when the clock has not moved on or went
back; a relaunch outranks the process before it because the clock has moved
on, and the unit must never become a coarser one, or the old mark would
outrank every new request. The id is `alter-zero:{seq}`, unique for free.

Every turn, then, is a handful of requests — `working`, maybe `blocked` and
`working` again, `idle` — and a quit is one `release`, which tells herdr to
forget the agent, its state and its resume command and to read the screen
again. The release is the last thing a quit does (`/quit`, Ctrl+C on an empty
composer), after the `SessionEnd` hooks and the MCP servers' teardown, so the
pane is the agent's for as long as any of it runs — and dropping the
reporter sends it too, so a loop that bails out on an error still hands the
pane back. It is never sent on `/clear` or `/resume`, where the next report
names the new session instead. A process that dies without one — killed, or
hung up by its terminal closing (there is no handler: the hang-up's default
ends the process) — is caught by herdr ≥ 0.9.2 within about a second, once
the pane's shell is back alone in the foreground.

## Never in the agent's way

herdr's own advice is "don't let Herdr slow your agent down", and the code is
built to it:

- **The loop never touches the socket.** `Session::sync_herdr` runs at every
  loop bottom (after the recorder's sync, so the first message's report
  already names the file it created), reads `herdr::activity` and posts the
  report `Tracker::update` answers — only when the state, its message or the
  session changed: a compare and nothing else on the common path. Outside
  herdr there is no reporter at all: no thread, no I/O.
- **One worker, one request at a time, newest report only.** `herdr::Reporter`
  owns a detached thread draining a single slot: a report the worker has not
  sent yet is replaced by a newer one, never queued behind it, and requests
  go out one at a time — herdr serves every connection on its own thread, so
  two in flight at once could be applied out of order. The worker owns the
  `seq` and stamps each send fresh.
- **Bounded, silent failure — and repair.** Each request's write and reply
  are bounded at 500 ms; a refused or failed send is retried after 1 s, 2 s,
  4 s … up to 30 s (`Timing::resend_after`), and a delivered one is re-sent
  every 30 s while nothing changes. herdr holds a self-reported state with no
  expiry, so without the resend a report lost to a timeout — or to herdr's
  live upgrade (`herdr update --handoff`), which carries the state of six of
  herdr's own integrations across and drops everyone else's — would stand
  wrong until the next change: an idle session, or one waiting on a prompt,
  would vanish from the sidebar until it was next used. A resend of an
  unchanged state notifies nobody (herdr compares the state before raising
  anything); an absent herdr costs a failed connect every half minute.
- **The quit waits at most 400 ms.** `release` replaces any unsent report,
  nothing is reported after it, and the quit waits that long for the worker
  to write it before the process exits regardless. Against a socket nobody
  listens on, the connect fails at once and the quit is not delayed at all.

## Nested agents stay out of the pane

Every process in a herdr pane inherits its variables. An agent run *by* the
session — `claude -p` or `codex exec` in a `bash` call, a hook that asks
another model for a review, an MCP server that is an agent itself (`codex
mcp-server`), another alter-zero — would report into this pane under them,
and one of herdr's official integrations doing so is not a fight over the
state but the end of it: its session report makes that agent the pane's
session owner, after which herdr drops every report alter-zero sends —
answering `ok` — for as long as the pane lives, a relaunch included (herdr's
`current_session_owner_conflicts`; reproduced against a real herdr with the
Claude and Codex integrations). A third-party source cannot claim the pane
back: herdr keeps a session identity only from its own integrations.

So **nothing the session starts gets `HERDR_PANE_ID`**: the model's terminal
sessions (`pty::spawn`'s `FOREIGN_TERMINAL_VARS`, beside `TMUX_PANE` and
`WEZTERM_PANE`, for the same reason — it names the pane the TUI is drawn in,
not the session's own terminal), every pipe-run child — the `!` shell, the
hooks, a `bash` call with no terminal to be had (`subprocess::command_in`) —
and the MCP stdio servers (`StdioTransport`, where a server's own config
naming the variable still wins). Every herdr integration requires a pane id,
so they all stand down, and so does a nested alter-zero, whose `herdr::pane`
needs one too.

Why the pane id and not `HERDR_ENV`, which every integration also requires
to be `1`? Because herdr's own tools read that one: herdr's agent skill opens
with `test "${HERDR_ENV:-}" = 1` and tells the agent to say it is not inside
herdr and stop when it fails, and herdr refuses to start nested when it sees
it. Without the pane id, herdr's CLI still works from every child —
`HERDR_ENV`, `HERDR_SOCKET_PATH` and `HERDR_BIN_PATH` stay, so the model can
list panes, split one, start an agent there — and what it loses is
`--current`, an untargeted command going to the focused pane instead. That
is exactly the environment herdr gives its own popups, the one other place a
process runs under herdr without being a pane's occupant (`without_pane_identity`
in its source); it also keeps `herdr pane run --current` from typing into the
TUI itself.

## Pictures in a pane

Separate from the state reports, and needing no setup either: a herdr pane
draws alter-zero's pictures (a pasted screenshot, an image `read`) as kitty
graphics. herdr's emulator is libghostty, which speaks the kitty graphics
protocol, unicode placeholders included, but a pane's environment names none
of it — `TERM=xterm-256color`, `TERM_PROGRAM=herdr`, and the outer terminal's
`KITTY_WINDOW_ID` stripped — so detection by environment alone fell to
half-blocks there. Where the environment names nothing, alter-zero asks the
terminal inside its startup cursor query, and herdr answers `OK` and calls
itself `libghostty` (`docs/images.md` *Asking the terminal*). herdr's
`terminal.kitty_graphics` setting has been on by default since 0.9.0; with it
off, a pane's terminal has no kitty graphics to answer for, says nothing, and
the pictures stay half-blocks. herdr then draws the picture on its own outer
terminal, so what an outer terminal without kitty graphics shows is herdr's
call; `terminal.kitty_graphics = false` in herdr's config, or
`ALTER_ZERO_IMAGE_PROTOCOL=halfblocks` in the pane, keeps half-blocks there.

## Turning it off

`ALTER_ZERO_HERDR=0` (`false`/`no`/`off`, the usual grammar) turns it off for
a run: no reporter is started, nothing is sent, and herdr treats the pane as
an unknown program. There is no `/settings` row — outside herdr the feature
does nothing, and inside it the reports are the point of running there.

## Testing

`src/herdr.rs`'s unit tests pin `herdr::activity` against real `App` states (a
turn, a `!` command, a compaction, each modal and its message, a running, a
settled and a follow-up-holding subagent, the history generation), the
tracker's state machine (the ranking, the failure hold and both ways it ends,
nothing sent while nothing changed, the resume command following the
conversation), the pane detection with `HERDR_ENV` exactly `1` and the off
switch, the request shapes field by field, `seq`, the resend schedule and
herdr's resume-command rules. The worker is tested against real Unix sockets:
reports in order under climbing `seq`s with the release last, a burst behind
a slow send collapsing to the newest, the release overtaking a waiting
report, a drop releasing, a release that waits no longer than told, posting
that never blocks on a dead herdr, a refused report sent again within the
first retry and a delivered one on the keepalive — each mutation-checked.
`send` is tested against real sockets too: the reply read back, a refusal
surfaced, a silent server costing its timeout, a missing socket failing at
once, a symlink to a live socket refused, a plain file never written.
`pty::spawn`, `subprocess` and the MCP transport each pin the `HERDR_PANE_ID`
drop. `smoke.sh` Phase 132 drives the real binary against a stub socket that
logs what it is sent: the launch's `idle` with `alter-zero` as its resume
command, a turn's `idle working blocked working idle` with the prompt's
message, `alter-zero --resume {id}` on every report from the first message
on, strictly climbing `seq`s, a `!` command seeing no pane id but herdr's
marker and socket, the release as a quit's last request, a `--resume`
relaunch carrying its command on its first report and outranking the old
release, nothing at all with `ALTER_ZERO_HERDR=0`, a dead socket costing a
turn and a quit nothing — and, through the real backend against a stub
provider that refuses every request, a failed turn reporting `blocked` with
`Turn failed: …` and holding it until a `/clear`, whose report goes back to
`idle` with the bare `alter-zero`. The fixture unsets every `HERDR_*`
variable, so a suite run from a herdr pane never reports to the real one.

## Limits

- **Unix only.** herdr's Windows socket is a named pipe this integration does
  not speak; there, no reporter starts.
- **A herdr popup** has no pane id, so nothing is reported from one.
- **The blocked message is not shown** by herdr 0.9.3 (see above).
- **`--current` fails in the model's shells** (see *Nested agents*): herdr's
  CLI there behaves as it does in a herdr popup.

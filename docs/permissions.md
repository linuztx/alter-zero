# Tool permission requests

Claude Code asks before it changes anything: a `write`, an `edit`, or a `bash`
command stops the turn and puts an inline approval prompt where the composer
was. This is that, ported to the inline TUI — the pure request/decision
vocabulary in [`crate::permission`], the modal prompt state in
`app/permission.rs`, its rendering in `ui/permission_view.rs`, and the
cross-thread handshake in [`crate::permission::PermissionGate`].

```
● Write(hello.py)
  ⎿  Waiting…

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

Every queued call renders its ordinary collapsed cell — Claude Code's look.
The one under the prompt shows the same dim `⎿ Waiting…` its batch siblings
do: it genuinely *is* waiting — the approve seam runs **before** its
`ToolStart`, so nothing has started — and rendering it bare (the old
header-only special case) just made a two-call batch read as one waiting call
and one mystery. A call that is truly executing (the main turn's own tool,
under a *subagent's* request) keeps its running row instead, drawn at rest —
the prompt is a still frame (`docs/tool-pulse.md`). `App::command_elapsed`
reads as `None` while a prompt is open, which drops the delayed
`(ctrl+b to run in background)` hint: the prompt owns every key, so that one
would be advertising a binding it swallows.

The context must not crowd out the prompt itself, though. A model's big
parallel batch (fifteen edits in one round) queues a screenful of
`⎿ Waiting…` cells, which used to eat the whole terminal before the body's
budget was computed: the prompt showed its title, question, and options with
**no content at all** — the user could not see what edit they were approving
— and with enough siblings even the options ran off the screen bottom. So the
cells are budgeted: `permission_lines` sets aside its fixed rows *and* a
floor for the body (the body's own height when short, else
`PERMISSION_MIN_BODY_ROWS` — the inline peek size), and the context keeps
whole cells in queue order while they fit, collapsing the excess into one dim
`… +N more waiting` row. The first chunk — the agent tree that asked, else
the asked-about call itself — is never dropped, so the prompt stays a
question about something on screen; a body naturally shorter than the floor
reserves only what it needs, handing the rest back to the siblings.

## The screen it scrolls, and gives back

A prompt is the one inline view that can be as tall as the whole terminal —
the body is shown *whole* — and it grows exactly like every other region
(invariant 3): downward in place while there is room, then each further row it
needs **scrolls** a row of chat off the top into the terminal's scrollback.
That keeps the conversation where the user expects it, in both senses. On
screen, the newest messages sit directly above the question — the message just
sent, the cell that just finished — Claude Code's picture. And off screen they
are in **real scrollback**: the user can scroll the terminal up and re-read
anything while the prompt waits. (The previous design *covered* the
conversation instead — the region grew upward over it, deliberately pushing
nothing into scrollback so the close could repaint the covered stretch in
place. The cost only showed up in use: the covered rows lived in **no buffer**
— not on screen, not in scrollback — so scrolling up during a prompt found
nothing. It read as "the terminal's scroll is disabled while it asks", worst
in kitty, until the answer or a resize brought the rows back.)

The scroll is a one-way move, and that is the part that needs care. When the
prompt closes and the region collapses back to a three-row composer, nothing
can refill the rows it vacates — a plain shrink strands the box mid-screen
above a band of blank rows. So the boundary notes every one-way move that
happens while a prompt is open — `term`'s `modal_scrolled` flag, set by
`ui::region_is_modal` at the two places such moves happen:

- **a paint**: the prompt's own growth scrolls, or a commit lands beneath it
  (`term::paint_live`) — and
- **a rebuild**: a `reflow` runs while the prompt is open (`term::reflow`) — a
  mid-prompt resize's purge, or an overlay return whose prompt opened
  underneath (Ctrl+O / Ctrl+D up when the request arrived, so the return's
  reflow is the prompt's first inline paint).

The first draw after the prompt closes consumes the note
(`InlineViewport::take_modal_scrolled`) with a **purge rebuild**: scrollback
and screen are rebuilt together from history, the box lands flush at the
bottom, and nothing is lost or doubled — the same repaint every resize gets,
in one synchronized frame. A prompt that never scrolled (an early-session
composer with free rows below it) sets no note, and its plain shrink just
blanks rows that were already free. Guarded by `smoke.sh` Phases 58, 60 and
62.

One consequence worth knowing: **commits flow while a prompt is up**. The
region sits at the bottom like any other, so a cell resolving between a
batch's back-to-back prompts simply scrolls in above the still-open next
prompt — visible at once, exactly once (`main.rs::commits_allowed` keeps only
the alternate-screen overlay and the agent session view on its held-back
list). The scroll such a commit causes is one of the one-way moves the note
tracks, so the eventual close still purge-rebuilds cleanly (`smoke.sh`
Phase 59).

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
  attached to the tool result *and* recorded on the cell; Esc goes back to the
  options. See [What the amend feedback is worth](#what-the-amend-feedback-is-worth).
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
   Reject → ToolStart + red ToolRejected{display, result}
```

`wait` polls its condvar on a short timeout and gives up the moment the turn's
`CancelToken` trips, so Esc / `/clear` / quit reap a blocked tool thread
promptly instead of wedging it.

`run_agent` gained one seam for this — `approve: FnMut(&ToolCallRequest) ->
Approval` — called immediately **before** each ordinary call's `ToolStart`, so
nothing has run and nothing has been announced as running when the prompt
appears. A rejection still emits a Start/End pair — `ToolStart` then
`StreamEvent::ToolRejected` in place of `ToolEnd` — so the call lands in history
and the transcript as a red cell while the *model* reads the longer "stop and
wait" text. The display string and the tool result are separate fields of
`Approval::Reject` precisely so the cell can be short and the instruction
complete, and `ToolRejected` carries **both** so the recorded call keeps them
both too (below).

Subagent calls take the same seam — `spawn_subagent_run` passes the agent's type
as the request's `agent`, and the event rides the agent channel's forwarder, so
`on_agent_event` opens the same prompt.

## What the amend feedback is worth

Tab's whole point is saying *what to do instead*, so that sentence has to
outlive the round it was typed in. Two places record it:

- **The cell** — `permission::denied_display` puts it on a second line, so the
  transcript shows what was asked for rather than a bare refusal:

  ```
  ● Write(hello.py)
    ⎿  User rejected write to hello.py
       Instructions: use pathlib instead
  ```

- **The derived context** — the model-facing `result` rides the recorded call as
  `ToolCall::context_output`, and `context::context_messages` replays *that* as
  the `tool` result (`ToolCall::context_text()`), not the cell text. So Ctrl+D
  shows what the model was actually told, every later turn keeps carrying it,
  and a `/resume` of the session restores it (`session::ToolRecord`'s
  `context_output`, omitted when absent so older rollouts still parse).

Without the second one the instruction reached the model for exactly one round —
the live `run_agent` message list had it, but the next turn rebuilds the context
from history, which kept only `User rejected write to hello.py`. The same split
covers option 3 (plain `No`) and Ctrl+E's explain-instead, whose model-facing
texts were being dropped the same way. A subagent's rejected call keeps both
texts on its own transcript, so its Ctrl+D view and any continuation run see
what it read.

The token tally charges `context_text()` too: what the next request uploads is
the long instruction, not the one-line cell.

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

### The cursor goes away

While the options are up the frame shows **no hardware cursor at all**
(`ui::cursor_visible`). An option list is a menu, not a text field: there is
nothing for a cursor to point at, and the terminal cursor is the one thing on
screen that moves *by itself* — a terminal with a cursor-trail animation
(kitty) draws every jump it makes, so a prompt opening threw a streak across
the screen and every ↑/↓ threw another. Nothing to show, so nothing is shown.

It costs the boundary nothing: every frame already opens with a `Hide` (so no
redraw is ever caught dragging the cursor around), and `term.rs` simply skips
the closing `Show` — the cursor stays away for exactly as long as the options
do. Tab's amend field **is** typed into, so the caret comes back with it, and
so does the composer's when the prompt closes.

`ui::cursor_position` still seats it either way — on the highlighted option,
one column past the `❯` marker, tracking ↑/↓ — so its return starts from a
meaningful row rather than wherever the last scroll left it (and a terminal
that ignores the hide still looks right). Both seats, the option row and the
amend field, are found from the region's bottom edge, `PERMISSION_TAIL_ROWS` up
(gap, hint, gap, rule), so the cursor never has to re-derive the body. That is
why a capped prompt's padding goes **above** the question rather than below the
hint: it keeps the question/options/hint block flush against the closing rule
at every body size.

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
  (↑/↓/1/2/3/a/Tab/Esc/ctrl+e), the amend field, the queue — plus the whole
  amend round trip (real gate, real keys) asserting the recorded call and the
  derived context carry exactly what the model was told.
- `app/tests/tools.rs` — `reject_tool` keeps both texts and charges the tally
  on the model-facing one; `session.rs` — the rejection round-trips through a
  rollout file while an ordinary call's line keeps its old shape.
- `ui/tests` — the rendered prompt: rules, coloured title, the agent suffix, the
  numbered/diff body, the cyan `❯` on the selection, the hint row, the live
  cells kept above it (`⎿ Waiting…` under the pending call and its siblings
  alike, a genuinely running call's `⎿ Running…`, the whole tree for a
  subagent's), the plain render painting exactly the prompt's rows, that
  `permission_height` equals the painted
  rows — at every height, the context rows included — and the big-batch cap:
  fifteen queued edits still leave the body its rows and the options on
  screen (the excess siblings collapse into `… +N more waiting`, the
  asked-about call survives at the top, the height contract holds), a tall
  body under the same batch keeps its guaranteed peek + `… +N lines` tail,
  and a small batch shows every sibling with no summary row.
- `ui/tests/layout.rs` — `region_is_modal` is a prompt and nothing else (the
  predicate the boundary reads to note one-way moves for the close's purge).
- `stream.rs` — the dummy's "parallel permission" turn: two gated `Bash` calls
  announced up front, each asking before it starts, the next request following
  the previous cell's resolution with no scripted pause.
- `ui/tests/permission_view.rs` — the options show no cursor while the amend
  field and the composer do; and the seat, pinned to the rendered rows: it
  lands on whichever row carries the `❯` marker and steps down with each ↓, on
  a capped prompt as well as one that fits.
- `smoke.sh` Phase 55 — the whole round trip against the dummy backend in a real
  terminal: draft typed, prompt shown, `2` approving, draft restored — plus the
  hardware cursor read back from the terminal (`#{cursor_flag}`): hidden over
  the options, shown again in Tab's amend field and in the composer after, and
  resting on the `❯ 1. Yes` row, one lower after ↓.
- `smoke.sh` Phase 56 — Tab's amend end to end: the instructions land on the red
  cell and the model-facing denial (feedback included) shows in the Ctrl+D
  context view, with neither text leaking into the other's place.
- `smoke.sh` Phase 58 — the scrolled geometry in a real terminal: while a
  screen-tall prompt is open, the earlier reply is reachable in
  screen+scrollback exactly once (the covered rows used to live in no buffer
  — the "terminal scroll is disabled while it asks" bug), and answering puts
  the box back flush at the bottom with the conversation whole and each
  message committed exactly once.
- `smoke.sh` Phase 59 — the back-to-back gap in a real terminal: the
  "parallel permission" batch's first prompt shows the just-sent message, the
  previous turn, and both `⎿ Waiting…` cells above it as real rows; the
  second prompt (landing in the same frame gap as the first cell's commit)
  shows that finished cell — committed above the open prompt, at once — and
  the message; and the final screen is whole — box flush at the bottom, the
  message exactly once in scrollback+screen.
- `smoke.sh` Phase 60 — a resize while the prompt is open, then the answer:
  the prompt survives the mid-prompt purge rebuild, and the close's own purge
  lands the box flush at the bottom instead of floating above the rows the
  collapsed prompt vacated, each message committed exactly once.
- `smoke.sh` Phase 62 — the same close, reached through the overlay: Ctrl+O is
  up when the request arrives, the return's reflow seats the prompt below the
  rebuilt tail (a one-way reseat the note records), and answering still lands
  the box flush at the bottom with the message committed exactly once — the
  "newlines at the bottom, but only when Ctrl+O was opened first" bug.
- `tests/live_openrouter.rs` — against a real provider: the replayed rejection
  is a legible context shape and the model still follows the instructions a
  turn later.

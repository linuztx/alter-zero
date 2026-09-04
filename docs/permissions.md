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
   2. Yes, allow all edits during this session (shift+tab)
   3. No

 Esc to cancel · Tab to amend

────────────────────────────────────────────────────────────────────────
```

## The shape

Four kinds, one layout, per `permission::PermissionKind`:

| kind    | title          | body                              | question                              |
| ------- | -------------- | --------------------------------- | ------------------------------------- |
| `Write` | `Create file`  | the numbered new contents         | `Do you want to create {file}?`       |
| `Edit`  | `Edit file`    | the numbered diff hunks           | `Do you want to make this edit to {file}?` |
| `Bash`  | `Bash command` | the indented command + description | `Do you want to proceed?`             |
| `Mcp`   | `Tool use`     | the indented `{server} - {tool}({args}) (MCP)` + the server's description | `Do you want to proceed?` |

The `Mcp` row is the `Bash` row with a different target: it names a call
rather than a command, so it wears the same shape (`docs/mcp.md`).

A `write` whose target **already exists** is an `Edit` — it shows the diff, not
the whole file, exactly as the resulting `Updated {path} (+A -D)` cell will.
The file row under the title names the file the way that cell will too —
relative under the cwd, `~`-relative under home, absolute elsewhere
(`docs/tools.md` *Path display*, through `App::path_display`); the request's
own `target` stays the model's verbatim path, which is what the rule engine
and the model see.

The body is the same numbered, syntax-highlighted, `+`/`-`-tinted block the
finished `write`/`edit` cell renders (`ui/file_cell.rs`'s
`numbered_body_lines`, shared by both) — the whole file/diff, not a peek: the
point of the prompt is that you read what you are approving, **all of it**.
The prompt is a framed view like the menus and pickers (`docs/view-flow.md`):
a page taller than the terminal paints bottom-anchored — the question, the
options and the hints close the page, so they are always on screen — and the
skipped top flows into the terminal's real scrollback, where the terminal's
own scrolling reads the whole diff. The retired cap hid the body's middle
behind a `… +N lines` tail; the tail survives only past the
`PERMISSION_BODY_MAX_ROWS` safety ceiling (the page is rebuilt and
highlighted every draw tick, so a pathological multi-megabyte write must not
turn each frame into an unbounded build).

A request raised by a **subagent** (`docs/agent-tool.md`) says so in the title:
`Create file · from the general-purpose agent`.

## What stays on screen

A prompt is a question *about something*, so the modal never hides what raised
it. The live cells above it survive: the call being asked about, any batch
siblings queued behind it, and — for a subagent's request — the round's whole
live agent tree (`● Running 3 agents…` and its rows). Everything else in the
live region gives way: the status line (nothing is running; the turn is blocked
on you), the composer, the bands, and the footer.

Those cells come from **the conversation on screen**, which is what makes them
worth showing. In the main view that is the live agent group plus
`App::tool_queue`, as above. Inside a **subagent's session view** it is that
agent's own queue and nothing else (`App::viewed_agent`) — the same walk its
strip previews, so the prompt looks exactly like the strip it replaced. The
divergence was a reported bug, and manual mode met it on every command: with
one *foreground* subagent running, the lead's live `● Agent(Run ls -la via
subagent)` / `⎿ Working…` cell is up for as long as the agent works, so
standing inside that agent's session its every `bash` request opened over the
lead's cell — a cell belonging to a screen the user had left — with the
agent's own `● Bash(ls -la)` / `⎿ Waiting…` and its batch siblings nowhere.
`context_chunks` and `context_is_stable` both take the viewed agent's branch,
because the flow test has to read the cells the context actually renders: the
lead's group is *always* live while a foreground subagent runs, so judging
staticness by it dropped the agent view's own static cells from every page too
tall to fit. The rest of the boundary already followed this rule —
`ui::preview_lines`, `ui::preview_rows`, `queued_lines`, `task_lines`,
`context_lines`, `agent_transcript_lines` and `App::last_assistant_text` all
swap on `viewed_agent()`; the prompt was the last holdout
(`docs/agent-view-streaming.md`, `scripts/smoke.sh` Phase 98).

Which agent asked is knowable because the request carries it:
`PermissionRequest::agent` is the *type* the title names, and
`PermissionRequest::agent_id` is **which** run, stamped by the boundary that
routes the event (`tui::agent::Session::on_agent_event`) — the only place that
knows it, since two agents can share a type. A request whose id is not the
open view's — the main turn's own call, or a sibling agent's — raised its
cells on another screen, so the prompt opens with no context at all (the idle
shape) rather than borrowing the viewed agent's unrelated batch.

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
`… +N more waiting` row; a body naturally shorter than the floor reserves
only what it needs, handing the rest back to the siblings. The first chunk —
the agent tree that asked, else the asked-about call itself — is never
dropped, so the prompt stays a question about something on screen.

When the page **flows** (a body taller than the terminal), the context flows
with it — provided it is *static*. A queued `⎿ Waiting…` cell is: the approve
seam runs before its `ToolStart`, so nothing can change while the answer is
pending, and it rides into scrollback **whole and uncollapsed** (there is no
screenful left to compete for, so hiding siblings behind the summary row
would lose them for nothing). A **live agent group** or a **running call** is
not — the tree's bullet breathes at the frame pulse and its `{n} tool uses ·
{tokens} tokens` counters advance, a running call's streamed output grows its
peek — and a flowed row is frozen in scrollback, so ticking content would go
stale there or re-sign the flow into a purge rebuild every tick
(`context_is_stable`, `docs/view-flow.md`). That context gives way and only
the static frame flows. Dropping the *static* cell too was a regression: on a
short terminal the `● Write(…)` header vanished from screen **and**
scrollback, leaving exactly the box out of nowhere this section exists to
prevent.

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
prompt — visible at once, exactly once (`tui::commit::Session::commits_allowed` keeps only
the alternate-screen overlay and the agent session view on its held-back
list). The scroll such a commit causes is one of the one-way moves the note
tracks, so the eventual close still purge-rebuilds cleanly (`smoke.sh`
Phase 59).

The close is not the only moment the one-way scrolls bite: the region can
**shrink while a prompt is still open**. Back-to-back prompts differ in
height — a body-capped prompt fills the terminal while a one-line file's is a
dozen rows; answering the tall one commits its cell out of the live region
and opens the short one, often inside the same frame gap (and a subagent's
tree-topped prompt alternating with a main-turn one moves the height just the
same). Painted in place, that frame stranded the *open* prompt above a band
of blank rows for as long as it asked — the flush's scroll plan seats the
region below the committed lines and reserves only the new, shorter height
(its trailing clear blanking everything beneath), and the separate-frame
ordering repin-shrinks into the same band without even setting the note (no
flush, no scroll). So the loop's draw tick decides *before* painting
(`ui::modal_needs_rebuild`, fed by `tui::view::Session::modal_rebuild_due`): while the
region is modal, if the last **painted** frame reached the screen bottom
(`InlineViewport::painted_bottom` — the tracked `view` height re-syncs
between paints for the flush plan, so it cannot serve) and this frame's plan
— `view_top + pending rows + new height` — would seat it short of that
bottom, the frame is answered with the same purge rebuild the close uses,
reseating the open prompt flush with the resolved cell visible above it. The
rebuild runs under the open prompt, so `reflow` re-arms the note and the
eventual close still purges; a prompt floating above the bottom (a short
conversation — nothing ever scrolled) keeps its plain shrink, blanking rows
that were already free. Guarded by `smoke.sh` Phase 63 (the dummy's
"staggered permission demo": a screen-tall `write` answered into a one-line
one).

## The options

Every prompt offers three, selected with ↑/↓ + Enter (the steps wrap — ↓ past
**No** comes back to **Yes**) or by typing `1`/`2`/`3`:

1. **Yes** — approve this call only.
2. **Yes, allow all edits during this session (shift+tab)** for `write`/`edit` —
   this *is* the switch to [edit mode](#permission-modes-manual--edit), and
   Shift+Tab (the mode toggle) selects it directly;
   **Yes, and don't ask again for: {rule}** for `bash` — the rule as it will
   be stored: `python3 *` for a prefix rule (the star saying "this program,
   any arguments", Claude Code's `Bash(prefix:*)`), or the whole command when
   only an exact match is safe. No letter shortcut: the old `(a)` binding was
   one fat-finger away from a standing approval, so the remember row is
   picked by number or ↑/↓ + Enter, like Claude Code;
   **Yes, and don't ask again for {server} - {tool} commands in {project}**
   for an MCP call — the tool named the way the user knows it and the
   project the rule is kept for (`App::project_dir`), while the rule stored
   stays the exact `mcp__…` wire name (`docs/mcp.md`).
3. **No** — reject; the model is told to stop and wait.

A long option **word-wraps** instead of truncating (`option_rows`, the
command body's `wrap_output`): an exact-only rule is the whole command, and
the old one-row `…` cut hid exactly the text being approved. Continuation
rows align under the label past the number, only the first row carries the
`❯`, the whole block lights up when selected — and the cursor's resting seat
still lands on the highlighted option's first row, because the seat and the
renderer share the per-option heights (`option_heights`, read by
`ui::cursor_position`). A *pathological* label (a kilobytes-long exact
command) caps at `PERMISSION_OPTION_MAX_ROWS` with the familiar `…` — the
region clamps to the terminal and paints top-down, so an unbounded wrap
would push `3. No` and the hints off the screen bottom.

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

`permission::command_scope` reduces a command to the rules the allowlist
stores:

- The command is split into segments on `|`, `||`, `&&`, `;`, `&` and newlines
  (quotes respected), and each segment is reduced to its **prefix**: the
  program word — `python3 script.py` → `python3`, `ls -la` → `ls`, `mkdir
  build` → `mkdir` — a file/argument is an *argument*, not part of the rule.
  (The old prefix kept a non-flag second token, so approving `python3
  script.py` never covered `python3 other.py`: every new script re-asked,
  which made "don't ask again" nearly useless — the flaw this rework fixes.)
- The curated **subcommand tools** (`git`, `cargo`, `npm`, `docker`, …) keep
  their verb — `git status --short` → `git status`, `npm run build` → `npm
  run` — because `git *` would cover `git push --force`; the verb keeps the
  rule as narrow as the action approved. A flag or path in the verb seat
  falls back to the program (`git -C /tmp status` → `git`).
- A later command is auto-approved only when **every** one of its segment
  prefixes is allow-listed, so allow-listing `python3` never silently admits
  a `; rm -rf /` tail.
- Approving stores every segment's prefix (so the identical command never asks
  twice) while the option's **label** names the last one with the `*` —
  the command the user reads as the action, matching Claude Code
  (`echo "" | python3 script.py` offers `python3 *`).
- A segment carrying a redirect or a substitution (`>`, `<`, `` ` ``, `$(`)
  cannot be summarized by a prefix — the prefix wouldn't mention the part that
  matters — so the scope degrades to `Exact`: the label and the stored rule are
  the **whole command** (no star), and only a byte-identical command is ever
  auto-approved. The scan is quote-aware: `echo "a > b"` redirects nothing and
  stays a prefix scope (the old raw `contains('>')` degraded it for no
  reason), while `"$(whoami)"` still substitutes inside double quotes and
  degrades; only single quotes defuse a substitution.
- A segment that **no prefix can honestly summarize** degrades the same way:
  a leading env assignment (`FOO=1 python3 …` — `PATH=…` can redirect what
  the program *is*) or a command wrapper (`sudo`, `sh -c`, `xargs`, `env`,
  `timeout`, …) whose "argument" is itself a command — a `sudo` prefix would
  allow-list everything sudo can carry.

Approving with option 2 also **sweeps the requests already waiting**. Parallel
agents raise theirs before any of them is answered — each thread consulted the
rules before the first prompt was even drawn — so without the sweep three agents
running the same command would ask three times *after* you said not to. The loop
remembers the scope, then releases every open/queued request the new rule now
covers (`App::drain_covered_permissions`, given the gate's own `allows`); one
that isn't covered still asks.

There is no built-in safe-command list. Every `bash`, `write`, and `edit` asks
until the rules say otherwise — Claude Code's default posture, and
the only one that can't be wrong about what is safe. `read` is never gated.

## Permission modes (manual · edit · auto · master)

The session has a **permission posture**, `permission::PermissionMode`, pinned
flush at the footer's **right edge** (`{model} · {cwd}      manual` — its own
zone, reserved off the left chain's truncation budget so a long cwd's `…` cut
can never eat it; `docs/footer.md`) and **cycled** with **Shift+Tab** from the
composer, one step per press in increasing autonomy:

- **manual** (the default) — every `write`/`edit` and every `bash` command
  asks, as above.
- **edit** — Claude Code's "auto-accept edits on": `write`/`edit` run without
  asking, `bash` commands still ask (until allow-listed).
- **auto** — Claude Code's auto mode: `write`/`edit` run like edit mode, and
  a `bash` command — or an MCP tool call (`docs/mcp.md`) — the allowlist
  doesn't already cover is reviewed by the **auto mode classifier** — a
  silent LLM safety check in the user's stead (the next section). No prompt
  opens in auto mode unless the classifier itself fails.
- **master** — Claude Code's bypass-permissions posture: everything runs
  unasked. No prompt, no classifier; the user has taken the seatbelt off.

The cycle wraps (`master` → `manual`), so one key walks the whole ladder.
`PermissionRules::allows` encodes the standing coverage: file changes are
covered by every mode above manual; a command or an MCP call only by master
or the allowlist — auto mode's classifier is a **per-call consult in the
approve seam**, never a standing rule, which is what lets a classifier
failure fall back to the prompt.

Option 2 on a `write`/`edit` prompt **is** the switch to edit mode — the mode
is exactly the old "allow all edits during this session" flag, made visible
and reversible — so choosing it (or pressing Shift+Tab on the prompt) approves
the pending change, flips the footer segment, and raises the confirming toast
(`Mode: edit — file edits run without asking (shift+tab to switch back)`).
Shift+Tab on a **bash** prompt only steps the posture — a step onto a mode that
covers the open request (master covers everything) resolves it through the
ordinary sweep; otherwise the prompt stays open — and from the composer it
works idle or mid-turn (the gate's rules are shared state; the very next
`approve` consult obeys the new posture). Cycling **back to manual makes file
changes ask again**: the mode is the single source of truth the gate
consults, not a one-way latch. Every toggle path lands on
`Action::SetPermissionMode` (or the `ResolvePermission` arm for option 2),
where the loop mirrors the mode onto the gate, **sweeps** queued requests the
new mode now covers (parallel agents' file changes waiting behind the open
prompt approve at once — the option-2 sweep above; a switch to master sweeps
every queued prompt), persists it (below), and toasts. With permissions
disabled there is no mode: the footer segment is hidden and Shift+Tab raises a
`Tool permissions are disabled` toast instead of silently doing nothing.

## Auto mode: the classifier

In auto mode a `bash` command — or an MCP tool call (`docs/mcp.md`) — that
would have prompted goes to the **auto mode classifier** instead — Claude
Code's auto-mode reviewer, rebuilt on the session's own provider
(`llm::classifier::SafetyClassifier`; the reference feeds its MCP calls to
the same reviewer, `mcpToolInputToAutoClassifierInput`). The approve seam
(`llm::approval::approve_call`) consults it between the allowlist check and
the prompt:

```
approve(bash/MCP call) ── gate.allows()? ── yes ─────────────────► runs (no note)
                           └ no · mode == auto
                              classifier ── allow ──► runs, cell notes the classifier
                                          ── deny ───► rejected with the reason
                                          ── error ──► the ordinary prompt (fallback)
```

The request is **one silent completion** — no events reach the UI, so the
asked-about cell just keeps the `⎿ Waiting…` row its batch announcement gave
it while the verdict is decided. Its one user message has two parts. The
`## Task context` block is the session's recent story — the **task context**
(`classifier::ClassifierContext`): the recent user requests, quoted line by
line (`> ` per line, so a request carrying markdown of its own stays visibly
quoted material rather than becoming structure), over one line per action
the agent has taken, each in the transcript cell's own `Name(args)`
vocabulary (`Read(/path)`, `Bash(cargo test)`,
`deepwiki - ask_question (MCP)({…})` — the same `display_name`/
`summarize_call` pair the cells use, so the classifier and the user read
the turn in the same words), a refused call marked `— denied, not run` (an
agent re-trying a variant of a denied command should be *seen* doing so).

Both halves are **rolling windows over the conversation**, not one turn. A
turn boundary is the wrong reset point for either: the request that explains
a command is often two turns back — "set up the project" → … → an `rm -rf`
on the build output — and an agent that had a command denied and re-tries a
variant of it a turn later should still be seen doing so. So each new user
message *pushes onto* the window rather than clearing it, and the caps alone
bound the block: the newest `CONTEXT_MAX_REQUESTS` (10) requests and
`CONTEXT_MAX_ACTIONS` (20) actions, each line truncated, with counted
`(+N … omitted)` markers where a window cuts. One verdict therefore costs
the same on turn fifty as on turn one.
Then, under its own `## Action to review` header so what is being *decided*
can never blur into what already happened: the cwd and the request itself —
for a command: the command and the model's stated `description` (labelled a
claim, not proof); for an MCP call: the tool named `{server} - {tool}`, the
server's own description of it, and the arguments in the cell's
`key: "value"` form (`classifier_request_prompt`'s MCP arm — the wire name
alone would hide where a remote call's risk actually lives). Context makes
the verdict *task-aware* — `rm -rf build/` right after a failed
`cargo build` reads differently from `rm -rf` out of nowhere — without
opening the old poisoned-transcript hole, because the block is **bounded
and inert**: every part is truncated (a `CONTEXT_REQUEST_MAX_CHARS` excerpt
per request, `CONTEXT_ACTION_MAX_CHARS` per action line closed with `…`,
and the two windows above), tool *outputs* never ride along (the cheapest
channel for a poisoned repo to lobby through), and the system prompt pins
the whole block as information-never-instructions — nothing in it can
authorize an action (the live suite proves a context that *begs* for an
allow changes nothing). The log lives on the **backend**, one per session:
each spawn pushes its user message onto the window (a subagent keeps its
own, seeded from its launch prompt or continuation chat —
`classifier::latest_user_text`, read before the skill reminder and hook
notes push more user-role messages), the execute/launch closures record each
executed call and agent launch, and the approve closure records refusals. Its system
prompt (`prompts/classifier.md`, the `include_str!` seam every prompt uses)
ends with the reference's strict output contract — the reply must begin
`<block>yes</block><reason>…</reason>` or `<block>no</block>` — which
survives arbitrary OpenAI-compatible models far better than JSON; the parse
(`classifier::parse_verdict`) still tolerates a chatty model's preamble, and
anything without a recognisable verdict is an **error, never an allow**. The
call runs on a tools-free clone of the session's `ModelConfig` with thinking
dropped (a verdict needs no reasoning budget); `ALTER_ZERO_CLASSIFIER_MODEL`
swaps in a cheaper model id on the same provider — Claude Code's
small-fast-model slot. Cancellation rides the same `CancelToken` polling as
every request, and a classify that fails *because* the turn was torn down
resolves like a reaped gate wait — no prompt is raised into a dying turn.

An **allowed** call runs exactly like a user-approved one, plus a
provenance note: `run_agent` emits `StreamEvent::ToolNote` right after the
`ToolStart` (the `Approval::AllowNoted` variant), the loop keeps it on the
running call (`App::set_tool_note` → `ToolCall::approval_note`), and once the
call resolves the cell appends a fresh dim `⎿` row — the transcript's record
that no human approved it:

```
● Bash(ls -la)
  ⎿  total 40
     drwxr-xr-x 1 user user  256 Aug  4 17:14 .
     …
     … +12 lines (ctrl+o to expand)
  ⎿  Allowed by auto mode classifier
```

The note renders in the collapsed cell and the Ctrl+O transcript alike
(`ui::tool` appends it after every branch's body, only for resolved
statuses — a running cell keeps its live look), rides the rollout as
`ToolRecord::approval_note` (omitted when absent, so old files parse) and so
survives a `/resume`, and a subagent's allowed call carries it onto its own
transcript through the same `ToolNote` event (`AgentRun::apply`). One cell
skips it inline: the **quiet resolved MCP cell** — its whole inline presence
is the one dim `Called {server}` line, and in auto mode every server call
resolves noted, so the row doubled each cell into noise (a parallel run's
aggregated line never carried it anyway); the Ctrl+O transcript, the rollout
and a failed MCP call's loud cell keep the record (`docs/mcp.md`).

A **denied** call never runs: the seam returns `Approval::Reject` with
`Denied by auto mode classifier` (+ `Reason: {…}` on a second line, the
amend-feedback shape) as the red cell and a longer model-facing result —
adapted from Claude Code's auto-mode denial, worded kind-neutrally since it
answers commands and MCP calls alike — telling the model the call was not
executed, other work may continue, a safer approach is fine, the denial's
intent must not be bypassed, and an essential capability means stop and ask
the user (who can approve it in manual mode, do it themselves, or Shift+Tab).
Both texts ride the recorded call like any rejection (`context_output`), so
later turns replay exactly what the model was told.

A classifier **failure** — network, an unparseable reply — falls back to the
ordinary prompt: asking the user is the safe posture, and the one that still
works offline. The **dummy backend** never talks HTTP, so its auto-mode demo
(a prompt naming "auto" + "permission") consults the deterministic offline
heuristic instead (`permission::auto_verdict` — a small read-only prefix
list): the scripted `ls -la` runs with the note, the scripted
`rm -rf /tmp/scratch` rejects, and the same demo in manual mode prompts —
which is what lets `smoke.sh` drive the whole feature without a provider.
The live OpenRouter suite (`tests/live_openrouter.rs`) covers the real
thing: verdicts both ways for commands *and* MCP calls — with the task
context attached and bare — plus the two context properties that matter (a
context that begs for an allow lifts nothing, and a retried variant of a
`— denied, not run` action stays blocked), and a full auto-mode turn whose
events show `ToolStart → ToolNote → ToolEnd` with no `Permission` in sight
— `tests/live_mcp.rs` closing the loop with a real server tool classified
end to end (`live_auto_mode_classifies_an_mcp_call_instead_of_prompting`).

## Seeing what the classifier sees — Ctrl+D, Tab

The task context decides whether a command runs unasked, so it is the one
input to a verdict the user cannot otherwise read: the request is silent,
the cell shows only the outcome. It lives one key away, as the **second page
of the Ctrl+D view** — Tab flips between them:

```
/ C L A S S I F I E R / / / / / / / / / / / / / / / / / / / / / / / / / / /
Auto mode — the classifier reads this before each command or MCP call.

## Task context
User requests (oldest first; the last is the current task):
> set up the project
> now clean up the build output

Recent actions (oldest first):
- Read(/home/user/proj/Makefile)
- Bash(make)
- Bash(sudo rm -rf /var/log) — denied, not run
~
──────────────────────────────────────────────────────────── 100% ─
 ↑/↓ to scroll   pgup/pgdn to page   home/end to jump
 q/esc/ctrl+d to quit   tab for llm context
```

Pairing them under one key is the point: both answer *what is this turn
actually sending* — one the model's own context window, the other its
reviewer's — so they share the chrome (`ui::render_context_view` paints
either; only the title, the scroll offset and the direction of the Tab hint
differ) and differ only in the body `ui::classifier_lines` /
`ui::context_lines` build. Four details earn their keep:

- **Whose window it is.** Inside an agent session view the page shows **that
  agent's** context, not the lead's — its own launch prompt as the request,
  its own executed calls as the actions. A subagent's verdicts are reviewed
  against its own run (`spawn_subagent_run` seeds a context from the prompt it
  was launched with and records one line per call it makes), so showing the
  lead's log there would describe decisions nobody on that screen made. The
  context lives on the agent's **registry slot**, minted with its id in
  `AgentRegistry::register` and read back through
  `AgentRegistry::classifier_context` — it used to be a plain local inside the
  agent's thread, which is precisely why nothing outside could read it and the
  page fell through to `ReplySource::classifier_context`, the lead's. A chat
  continuation pushes onto the same rolling windows, because it is the same
  agent's conversation; an agent the registry no longer holds renders the empty
  placeholder rather than falling back to the lead. The other Ctrl+D page has
  always swapped this way (`ui::context_lines`' first branch keys on
  `viewed_agent`); this one now matches it. Covered by
  `agents::tests::each_agent_gets_its_own_classifier_context` and, end to end
  on the real wire, `live_a_subagents_classifier_context_is_its_own_not_the_leads`.

- **The mode note.** The log is recorded in *every* mode — the boundary
  feeds it per call, not per verdict — but only auto mode consults it. A
  page that said nothing would read as "the classifier is deciding this" in
  the modes where the user is, so the row above the block says which it is:
  `Auto mode — …`, `Recorded every turn; consulted only in auto mode
  (shift+tab to switch)`, or `Tool permissions are disabled — no classifier
  runs` with no gate at all.
- **Tab reaches it over a permission prompt.** Ctrl+D already escapes the
  modal (with Ctrl+O, the two read-only views), and once the view is up its
  own handler owns the keys — the modal's routing only runs in the
  conversation view — so the prompt's Tab (its amend field) and the page
  flip never contend. That matters most here: a prompt in auto mode means
  the classifier *failed* or the call was a file change it never sees, and
  "what did it know?" is exactly the question being asked. Read-only, so the
  blocked tool thread keeps waiting and the prompt is still open on the way
  back.
- **Each page keeps its own scroll**, so flipping to compare them and back
  lands where you left off (`App::debug_page_scroll` hands the pager arms
  whichever page is up), and the page itself persists across opens — Ctrl+D
  returns to whichever you were last reading, the title saying which.
- **The classifier page reads live.** Its block is pulled from the backend
  on every draw (`ReplySource::classifier_context` →
  `App::set_classifier_context`, the system-prompt injection pattern) rather
  than cached, because it grows as the turn runs and rolls its windows as
  the conversation goes on — a page left open tail-follows the actions as
  they land. It needs no `ContextCache` sibling: the block is bounded to
  `CONTEXT_MAX_REQUESTS` + `CONTEXT_MAX_ACTIONS` short lines by
  construction, which is the whole point of the caps.

That live read is why `LlmBackend` holds the `ClassifierContext` behind an
`Arc<Mutex<…>>` rather than a per-spawn local: it must outlive a turn (both
halves are windows over the conversation), `spawn` pushes each user message
onto it, the tool closures append, and the boundary reads it out. The dummy
backend keeps no log — its offline auto-mode demo answers from the pure
`permission::auto_verdict` heuristic — so the page shows its dim
`No classifier context yet` placeholder there, under the same mode note.

A **subagent** keeps its own context (seeded from its launch prompt), and
that one is not surfaced: the page shows the lead's. Its own verdicts read
its own log, exactly as the lead's read the lead's.

## The scratchpad exemption

One more thing resolves before the prompt: a `write`/`edit` **inside the
session's own scratchpad directory** (`docs/scratchpad.md`). The system prompt
sends every temporary file there, and the directory is outside the user's
project, so `approve_call` consults `PermissionGate::scratchpad_covers` right
beside the standing allowlist — before the `PermissionRequest` hook, the
classifier and the prompt, because it answers the same question they do.

It is narrow: the two file tools only (a `bash` command naming a scratchpad
path still asks — what it goes on to touch is its own business), the path
strictly inside and lexically checked, and a forced ask still asks. And it is
visible, through the same channel the classifier's verdict uses:

```
● Write(/tmp/alter-zero-1000/18ce…-5d77f/scratchpad/plan.md)
  ⎿  Wrote 12 lines to …/scratchpad/plan.md
  ⎿  Allowed in the session scratchpad
```

## Persisted per project (`~/.alter-zero/permissions.json`)

Standing approvals used to be session-only — quit and every rule was gone,
which is not what "don't ask again" says. They now persist in the config home
(`ALTER_ZERO_CONFIG_DIR`, else `~/.alter-zero`), **keyed by project
directory** so a rule granted in one project never leaks into another —
Claude Code's per-project settings:

```json
{
  "projects": {
    "/home/user/Codes/rust/project/alter-zero": {
      "allow_commands": ["git status *", "python3 *"],
      "mode": "edit"
    }
  }
}
```

`allow_commands` rows ending in ` *` are prefix rules; anything else is an
exact command. `mode` is the saved posture (absent = `manual`, and an unknown
label degrades to `manual` — a hand-edited file asks more, never less). The
pure format/parse is `permission::PermissionsFile`/`ProjectPermissions` (the
`Settings` pattern); `main.rs` owns the file I/O: the startup load seeds the
gate (`seed_commands` + `set_mode`) before the first frame, and every rule
change — an option-2 approval, a Shift+Tab toggle — **re-reads, updates this
project's entry, and rewrites** (read-modify-write, so instances in other
directories never clobber each other; best-effort like `config.json`, a
write failure never kills the TUI). Restart the app — or `--continue` /
`--resume` a session — in the same directory and the footer comes up in the
saved mode with the saved allowlist in force.

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
the inline pickers: while a request is open it owns every key but the two
read-only overlays' ([below](#except-the-two-keys-that-only-look)). `render_live`
replaces the **whole** live region with it, the streaming strip included — the
turn is blocked on you, so there is nothing to animate.

Opening stashes the composer (`TextArea` text + cursor, and the `!` shell-mode
flag) and clears it, and drops everything else that hangs off the composer —
the palette, the `@`/`$` pickers, the `?` band, an open Ctrl+R search, a primed
backtrack, and the footer's ↓ selections (`background_focus`,
`agent_selection`). The footer ones matter for the same reason as the rest: the
prompt paints over the footer, so a highlight left lit is one the user can
neither see nor clear — the modal routes above its key handler — and that comes
back *armed* when the prompt closes, where the next Enter opens the background
manager band instead of doing what they meant. Closing restores the draft, so a
request that lands mid-sentence does not eat what you were typing — and Tab's
amend field starts empty, because it *is* the same textarea. A second request
arriving while one is open queues (`App::pending_permissions`) and opens as
soon as the first resolves; its backend thread simply stays blocked meanwhile.

`/clear`, an interrupt, and a quit all drop the prompt and the queue; the
blocked threads notice their cancel token and return.

### …except the two keys that only look

**Ctrl+O** (the transcript pager) and **Ctrl+D** (the raw LLM context) are the
one exception to "owns every key", and they are the exception for the same
reason the rest of the rule exists. The prompt asks about work that is already
on the transcript, and the way to answer it is often to read further back than
the cell above the question — what the model said it was doing, what the
previous tool returned, what the context it is working from actually contains.
Swallowing those two keys made a prompt the one moment in the session when the
conversation could not be inspected, which is precisely the moment it matters
most. So `on_key_permission` runs `App::on_key_overlay_toggle` first and
returns whatever it decides — one helper, extracted from `on_key`'s global arms
into `app/views.rs` beside the two toggles it wraps, so the global binding and
the modal's cannot drift — and only then swallows the rest. Every other Ctrl+key
is still absorbed: the prompt must not be answerable by accident.

The transcript shows the pending call the way the strip above the prompt does —
`● Write(hello.py)` over its dim `⎿ Waiting…` — because it reads the same
`tool_queue`, so what you opened it to look at is the first thing on it.

Nothing else changes. Both views are read-only — they scroll, and that is all
they do — so the tool thread stays blocked on the gate, the queue stays queued,
the stashed draft stays stashed, and the prompt is still open on the way back.
The keys work from **Tab's amend field** too (neither is an editing key), where
the typed feedback survives the round trip. And because the views are the
alternate screen, the modal-region routing takes care of itself: `on_key`'s
guards are all `view == View::Conversation`, so while the overlay is up its own
key map owns the keyboard.

Two boundary details make the round trip a no-op on the terminal, and both were
already there for the "the request arrived while the overlay was up" case:

- The return is the ordinary `Session::overlay_return_repaint` — everything
  that committed under the overlay sits in the viewport's pending queue and
  flushes above the live region, and the next draw tick's flow check
  (`view_flow_stale`) re-establishes a screen-tall prompt's flow with the usual
  purge rebuild. Screen **and** scrollback come back byte-identical at all three
  geometries `smoke.sh` Phase 90 drives — a floating region, one flush at the
  screen bottom, one flush whose page flows.
- The overlay's idle **Esc** must not arm the backtrack preview
  (`App::overlay_esc_backtracks` gained a `!App::modal_open()` clause). A
  *background agent* can raise a prompt with no turn running, so "idle" alone
  would offer a rewind that truncates history and prefills the very composer the
  prompt has stashed — with a tool thread still parked on the gate. Nothing is
  rewindable while something is blocked on the user; the closing hint row reads
  `q/esc/ctrl+o to quit` accordingly, since hint and key share the one
  predicate.

`App::modal_open` **is** that predicate — a permission prompt or an ask modal is
open — and `ui::region_is_modal` is now its caller too, so the key routing, the
region's re-pin and its close's purge, and the backtrack guard all read one
definition. A second hand-rolled copy is exactly the drift the shared one exists
to prevent.

The `AskUserQuestion` modal follows the identical rule — same helper, same
reason (`docs/ask.md`). The inline **pickers** (`/model`, `/settings`,
`/hooks`, the ↓ manager, …) deliberately do not: they own every key too, but
Esc just closes them, so the two views are one keystroke away already. The
modals are the only regions you cannot leave without answering — Esc on a
prompt cancels the turn, Esc on a question declines the call — which is
exactly why they are the two that must not lock the conversation away.

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
(gap, hint, gap, rule), so the cursor never has to re-derive the body. The
tail block closes the page, so the bottom anchor keeps it flush against the
closing rule at every body size — a flowing page and a fitting one seat the
cursor identically.

## Turning it off

`ALTER_ZERO_PERMISSIONS=0` (or `false`/`no`/`off`) starts the session with no
gate attached, and every tool runs as it did before this feature. The
`LlmBackend` only asks when a gate was installed, so an embedder (and the live
integration tests) that builds a backend directly is unaffected. With no gate
there is no mode either: the footer's right-edge segment disappears and Shift+Tab
explains itself with a toast instead of pretending to toggle anything.

## Tests

- `permission.rs` — the pure vocabulary: titles, questions, option labels
  (the `{prefix} *` display, the exact command verbatim),
  `command_scope`'s segmentation/prefixing/degradation (subcommand tools,
  wrappers, env assignments, quote-aware redirects), the mode's
  label/parse round trip and the four-step Shift+Tab cycle, the rules' allow +
  remember (mode gating file changes both ways; auto covering files but
  never commands; master covering everything), the classifier vocabulary
  (the allowed note, the denied display/result texts, the offline
  `auto_verdict` heuristic's allow/deny split), the `PermissionsFile` round
  trip (the documented shape, auto/master labels included, other projects
  preserved, garbage → default, manual omitted), and the gate's blocking
  round trip (real threads) + shared mode + seeded commands.
- `llm/approval.rs` — the auto-mode seam with scripted classifiers: an
  allowed verdict returns `AllowNoted` (no prompt), a denial the classifier
  texts, files and allow-listed commands never consult it, manual/edit modes
  never consult it, master allows everything silently, a classifier failure
  falls back to the prompt, a failure on a cancelled turn rejects without
  one, and auto mode with no classifier attached still asks.
  `llm/classifier.rs` — the user-prompt build and the `<block>` contract
  parse (noise/case/whitespace tolerated; no verdict → error, never allow).
  `llm/agent.rs` — a noted approval emits `ToolStart → ToolNote → ToolEnd`
  and still runs the call.
- `app/tests` — opening stashes and closing restores the draft, the key map
  (↑/↓/1/2/3/Tab/Esc/ctrl+e — a bare `a` now does nothing; Shift+Tab takes the
  remember option on a file prompt, toggles the mode on a bash prompt and
  from the composer both ways, and explains itself when permissions are
  disabled), the amend field, the queue — plus the whole
  amend round trip (real gate, real keys) asserting the recorded call and the
  derived context carry exactly what the model was told; and the two keys the
  prompt lets through — Ctrl+O/Ctrl+D open and close over an open prompt
  (options *and* amend field, feedback intact), every other Ctrl+key is still
  swallowed, and a waiting prompt is not a backtrack target — plus the footer
  selections the open takes with the composer.
- `ui/tests/transcript.rs` — the overlay's **closing hint row** under an open
  prompt: a background agent's request (idle, with a backtrack target) still
  reads `q/esc/ctrl+o to quit`, so what the row promises and what Esc does
  agree by construction.
- `ui/tests/footer.rs` — the footer's right-edge mode segment (flush at the
  row's edge, dim, the left content truncating first at narrow widths; absent
  when no mode is injected).
- `ui/tests/permission_view.rs` — a long remember rule wraps instead of
  hiding its tail (every token survives, continuations aligned and
  marker-free), a pathological one caps at `PERMISSION_OPTION_MAX_ROWS` with
  the `…` (the options and hints stay on screen), every wrapped row of the
  selected option lights up, and the cursor seat still lands on each
  option's `❯` row past a wrapped block.
- `app/tests/tools.rs` — `reject_tool` keeps both texts and charges the tally
  on the model-facing one; `session.rs` — the rejection round-trips through a
  rollout file while an ordinary call's line keeps its old shape.
- `ui/tests` — the rendered prompt: rules, coloured title, the agent suffix, the
  numbered/diff body, the cyan `❯` on the selection, the hint row, the live
  cells kept above it (`⎿ Waiting…` under the pending call and its siblings
  alike, a genuinely running call's `⎿ Running…`, the whole tree for a
  subagent's), the plain render painting exactly the prompt's rows, that
  `permission_height` equals the painted
  rows clamped to the terminal — the builder a fixpoint at the region's own
  height — and the big-batch rules:
  fifteen queued edits still leave the body its rows and the options on
  screen (the excess siblings collapse into `… +N more waiting`, the
  asked-about call survives at the top, the height contract holds), a tall
  body under the same batch flows whole with the ticking context dropped,
  a small batch shows every sibling with no summary row, and a body past
  the `PERMISSION_BODY_MAX_ROWS` ceiling still caps with the `… +N lines`
  tail.
- `ui/tests/layout.rs` — `region_is_modal` is a prompt and nothing else (the
  predicate the boundary reads to note one-way moves for the close's purge);
  and `modal_needs_rebuild`, the draw tick's whole rebuild decision: it fires
  when a pinned open prompt's next frame would seat short of the screen
  bottom (with or without pending lines, whether or not the note is set),
  holds while the frame stays seated (same height, growth, a flush whose own
  scroll re-seats flush), skips a prompt floating above the bottom, and at
  the close follows the one-way note exactly as before.
- `stream/dummy/gated.rs` — the dummy's "parallel permission" turn: two gated `Bash` calls
  announced up front, each asking before it starts, the next request following
  the previous cell's resolution with no scripted pause; and the "staggered
  permission" turn — two gated `Write`s whose prompts differ wildly in height
  (the first's body caps any sane terminal, the second's is one line), the
  same no-pause timing, for the mid-open shrink; and the "auto permission"
  demo three ways — auto mode classifying both calls with no prompt (the
  note on the listing, the rejection on the delete), manual mode prompting
  for both, master mode running both silently.
- `app/tests/tools.rs` — `set_tool_note` rides the running call into history
  (and ignores a Waiting front / an idle queue); `agents.rs` — a subagent's
  `ToolNote` lands on its own transcript; `ui/tests/tool.rs` — the note row
  renders after the collapsed peek and in the expanded view, dim, only once
  the call resolves (a failed run included, a backgrounded cell under its
  fixed row), with an unnoted cell byte-identical to before; `session.rs` —
  the note round-trips a rollout while an unnoted line keeps its shape.
- `tests/live_openrouter.rs` (ignored; needs `OPENROUTER_API_KEY`) — the real
  classifier allowing `ls -la` and denying `sudo rm -rf /etc` with a reason,
  and a whole live auto-mode turn: `ToolStart → ToolNote → ToolEnd`, no
  `Permission` event, the command's output in the model's reply.
- `ui/tests/permission_view.rs` — the options show no cursor while the amend
  field and the composer do; and the seat, pinned to the rendered rows: it
  lands on whichever row carries the `❯` marker and steps down with each ↓, on
  a flowing prompt (mapped through the bottom anchor's skip) as well as one
  that fits.
- `smoke.sh` Phase 55 — the whole round trip against the dummy backend in a real
  terminal: draft typed, prompt shown, `2` approving, draft restored — the
  footer's right-edge mode flipping to `edit` and the project's entry landing
  in `permissions.json` with `"mode": "edit"` (cleaned up after, so the later
  permission phases still get their prompts) — plus the
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
  up when the request arrives, the return seats the open prompt over the
  restored screen (its growth and the queued commits' flush are one-way moves
  the note records), and answering still lands
  the box flush at the bottom with the message committed exactly once — the
  "newlines at the bottom, but only when Ctrl+O was opened first" bug.
- `smoke.sh` Phase 63 — the mid-open shrink in a real terminal: the
  "staggered permission" batch's screen-tall first prompt is answered, and
  while the one-line second prompt is open its closing rule is the pane's
  **last row** — no band of blank rows underneath the still-open prompt (the
  reported empty-newlines bug) — with the tall `write`'s resolved cell
  visible above it, committed exactly once, and the box back flush after.
- `smoke.sh` Phase 90 — Ctrl+O/Ctrl+D over an **open** prompt in a real
  terminal, at three geometries, each asserting the seat it is named for (a
  region floating with rows to spare, one flush at the screen bottom, and one
  flush whose page flows its top into scrollback) so no case can quietly
  degenerate into a copy of another: the transcript opens with the asked-about
  call still `⎿ Waiting…` on it and offers `q/esc/ctrl+o to quit`, **Esc**
  leaves it — the key that would otherwise arm the rewind — landing back on the
  same prompt still flush, Ctrl+D round-trips the same way, both trips leave the
  screen *and* the scrollback byte-identical, and the prompt still resolves
  afterwards.
- `tests/live_openrouter.rs` — against a real provider: the replayed rejection
  is a legible context shape and the model still follows the instructions a
  turn later.

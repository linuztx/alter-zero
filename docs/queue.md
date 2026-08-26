# Queueing messages while a turn runs

Port of openai/codex's *queued user messages* **and its steering**: while a
reply is generating, a submitted message doesn't have to wait at the keyboard —
it shows above the box and reaches the model **without the turn having to
finish**. The two keys carry codex's two intents:

- **Enter** hands the message to the turn **already running**. Its agent loop
  takes it at the next **round boundary** — right after the round's tool
  results, before the next request is built — so the model reads it *within*
  the same turn, typically a tool call away rather than a whole turn away.
- **Tab** opens a **new turn-batch** — its message runs as a *separate
  follow-up turn* after the running one, and after any batch already queued
  (codex's Tab-to-queue).

So there are two pending sets, and which key you press picks one: what the
current turn is about to read (`App::steered`), and the follow-up turns waiting
behind it (`App::queued`). Both render the same way above the box — a pending
message is a pending message — and a turn that ends **before** reading what was
handed to it gives it back, so the follow-up path is also the fallback and
nothing the user typed is ever dropped.

**The same mechanism runs one level down.** A subagent's session view has this
exact queue over that agent's own loop: a message typed while it works waits
above the box and lands on *its* transcript at *its* next round boundary. Main
and subagent share the seam (`SteerQueue`), the event (`StreamEvent::Steered`),
the rendering (`ui::queued_lines`) and the reclaim. See `docs/agent-tool.md`.

See `CLAUDE.md` for where this sits in the runtime model.

## What codex does (findings)

The queue lives on `chatwidget/input_queue.rs::InputQueueState` as
`queued_user_messages: VecDeque<QueuedUserMessage>` (plus steer/slash machinery
we don't have). The mechanics that matter:

- **Tab is the queue key** (`bottom_pane/chat_composer.rs` ~3106): the composer's
  `queue_keys` is `Tab`. While a turn runs, **Tab** yields `InputResult::Queued`
  → `queue_user_message_with_options` (push onto `queued_user_messages`), whereas
  **Enter** (`submit_keys`) yields `InputResult::Submitted` → `submit_user_message`,
  which *steers the running turn* rather than queueing. Idle, Tab on a draft just
  submits like Enter (and on a `!` bang draft it inserts a literal tab); the
  footer shows `Tab to queue message` only while a turn runs and the composer has
  a draft (`footer.rs`).
- **Queue vs send** (`input_flow.rs::queue_user_message_with_options`):
  `if !is_session_configured() || is_user_turn_pending_or_running() { push_back } else { submit }`.
- **Drain on completion** (`input_flow.rs::maybe_send_next_queued_input`, from
  `turn_runtime.rs::on_task_complete`): pops from the **front** and submits **one
  `Plain` message per completion** (it `break`s after one) — so every queued
  message becomes **its own sequential turn**. (Its interrupt path
  `merge_user_messages_with_history_record` instead merges the backlog into one
  fresh turn.)
- **Display** (`bottom_pane/pending_input_preview.rs`): a dim italic
  "Queued follow-up inputs" section, each entry prefixed `↳`, truncated.
- **Edit/dequeue** (`chatwidget/interaction.rs` + `input_restore.rs`): the
  `edit_queued_message` binding (Alt+Up) pops the **last** queued message
  (`pop_back`) back into the composer to edit, resend, or drop.
- **Interrupt** (`interaction.rs` ~115, `input_restore.rs::on_interrupted_turn`):
  two tiers. *Plain* queued messages are restored to the composer on Esc. But a
  *steer* takes the `submit_pending_steers_after_interrupt` path: Esc interrupts
  and resubmits the steer immediately as a fresh turn.

## What we build

Two pending sets, one rendering, one reclaim.

**Steering — what the running turn is about to read.** `App::steered:
VecDeque<String>` holds the messages Enter handed to the turn in flight. The
boundary pushes each onto a shared `steer::SteerQueue` — an
`Arc<Mutex<Vec<String>>>` the backend thread drains — and `run_agent` takes it at
the top of **every round**, right where the background-completion notes are
taken, appending each as a user-role message after the round's tool results. The
take is announced with `StreamEvent::Steered { text }`, and that event is the
handoff: the loop drops the pending row, finalises the assistant text streamed
ahead of it (invariant 4), records the real user message and counts it into the
turn's `↑` tally. From then on it is an ordinary part of the conversation —
Ctrl+O shows it, the derived context carries it, the rollout keeps it, a
backtrack can rewind to it.

**Follow-up turns — the classic queue.** `App::queued: VecDeque<QueuedTurn>` is
unchanged: typed entries (codex's action-tagged `QueuedInputAction`) drained
FIFO, one per turn end. A `QueuedTurn::Messages { texts, images }` is a text
batch (sent to the model, its Ctrl+V attachments riding along as
`(placeholder, path)` pairs — `docs/image-paste.md`); a `QueuedTurn::Shell(String)`
is a standalone `!` command (**run locally**). The variant is the dispatch
discriminator, so submission order is preserved across mixed entries. **Tab**
opens a new `Messages` batch; a **shell-mode `!` draft** queues as its own
`Shell` entry, **never merged**.

**The reclaim is what joins them.** A turn can end without ever reaching another
round boundary — the model simply answered. At *every* turn end
(`dispatch_after_turn`, so `StreamDone`, a backend error and both Esc-interrupt
outcomes share it) the boundary takes the shared queue back and
`App::reclaim_steered` moves whatever is left onto the **front** of `queued` as
one batch: it was submitted into the turn that just ran, so it goes before a Tab
follow-up that was always meant for later. `flush_next_queued` then dispatches it
as the next turn, exactly as the old queue always did. That is why nothing is
lost, and why the old batching survives as the fallback: three Enters a turn
never read become one next turn carrying all three.

Two drafts can't ride a round boundary and fall through to the follow-up queue
instead, both handled inside `App::steer_draft` so the caller asks once:

- a **shell-mode `!` command** — it runs locally and is its own turn by
  definition;
- a draft carrying **Ctrl+V attachments** — a boundary injection is a text
  `ChatMessage`, while the images need the typed channel a real turn start opens
  (`docs/image-paste.md`).

Two **turns** likewise take nothing — `App::turn_steerable` is
`is_streaming() && !is_compacting() && !status.shell` — and their drafts queue
as follow-ups exactly as they always did: a `!` **shell turn**, because nothing
is reading a conversation there, and a **`/compact` turn**, whose request is the
fixed handoff prompt over the context being summarized rather than a
conversation (a user message folded into it would corrupt the summary,
`docs/compact.md`).

### The seam (`steer.rs`, `llm/agent.rs`)

`steer::SteerQueue` is the whole cross-thread surface: `push` (the loop),
`take` (the round boundary), `take_last` (Alt+Up's pull-back — `None` once the
turn has read it, which is the only honest answer), `is_empty`. Cloneable, every
clone the same queue, so every backend rebuild (`/model`, a `/settings` knob)
re-attaches the one the loop is already pushing into — `LlmBackend::with_steer`,
beside `with_ask`/`with_tasks`.

`run_agent`'s `pending_notices` closure widens into `pending_inputs`, returning
typed `PendingInput`s:

- `PendingInput::Notice(String)` — a background shell or agent that finished
  (`docs/background.md`). Invisible; the loop already recorded it.
- `PendingInput::User(String)` — a message the user queued. Announced with
  `StreamEvent::Steered`.

**Notices lead, the user's own messages close.** A completion is a *result* and
belongs with the results it follows; the newest thing the user said must be the
last thing the model reads. The take stays after the cancel check, so an
abandoned turn steals neither a note owed to the follow-up turn nor a message
the boundary is about to re-dispatch.

### State (`app/queue.rs`)

- `App.steered: VecDeque<String>` — the app's mirror of the shared queue: what
  the strip renders, and what survives a turn that ended without reading it.
- `App.queued: VecDeque<QueuedTurn>` — the follow-up turns. We never push an
  empty entry, so `is_empty()`/`len()` count entries and `queued[0]` is the next
  turn.
- `App::turn_steerable()` — can a message submitted now go into the running
  turn? A model turn can; a `!` shell turn can't.
- `App::steer_draft() -> Option<String>` — Enter's mid-turn path. Consumes the
  composer, records the text in `input_history` (↑ recalls it like a submit),
  parks it in `steered`, and returns it for `Action::Steer` to push onto the
  shared queue. `None` for the two fall-through cases above, which it routes to
  `queue_draft` itself.
- `App::deliver_steered(text)` — the `StreamEvent::Steered` half: flush the
  streamed segment, drop the pending row, record the user message, count the
  `↑` tokens. Records even when no row matched — what the model read is what the
  transcript owes the user.
- `App::reclaim_steered()` — the turn-end fallback described above. A no-op when
  nothing is waiting, so no empty batch is ever invented.
- `App::recall_steered(text)` / `App::recall_last_queued()` — Alt+Up's two
  halves (see below).
- `App::queue_draft(new_batch: bool)` — the follow-up path. `new_batch` picks
  the semantics: `true` (Tab) pushes a new `Messages` batch, `false` appends to
  the last one (which is how a shell turn's Enters and a reclaimed batch behave);
  an empty queue — or a `Shell` entry at the back — starts a fresh batch either
  way.
- `App::queue_shell()` — queues the shell-mode draft as a standalone
  `QueuedTurn::Shell` entry (codex's `submit_queued_shell_prompt`), exits the
  mode, records the full `!command` for ↑ recall. **Always a new entry.**
- `App::drain_next_batch()` / `drain_last_batch()` — pop the front entry (the
  loop's turn-end dispatch) / the last one (Alt+Up).
- **Enter while a turn is in flight** (`on_key_conversation`): `steer_draft()`
  when `turn_steerable()`, else `queue_draft(false)`. Idle Enter is unchanged
  (`Action::Submit` / `Action::RunShell`); a bare `/token` keeps the palette
  path.
- **Tab while a turn is in flight** (after the palette's Tab arm so the menu
  still wins): `queue_draft(true)`. **Idle or empty Tab falls through to a
  no-op.**
- **Alt+Up** (`KeyCode::Up` with `ALT`, an **empty** composer): the **follow-up
  queue first** — it is the deliberate backlog, and the one Alt+Up has always
  edited — pulling the last entry back into the composer (a `Messages` batch
  newline-joined with its images re-attached, a `Shell` entry as `!command`,
  which `recall_input` re-absorbs into shell mode). With nothing left there it
  reaches the running turn's messages via `Action::ReclaimSteered`: only the
  shared queue knows whether one can still be taken back, so the boundary asks
  it (`take_last`) and hands any answer to `recall_steered`. Guarded on an empty
  composer so it never clobbers a draft.

### Flush and reclaim (`tui/turn.rs`, the I/O boundary)

`dispatch_after_turn` runs at every turn-end site, so they can't drift. It takes
the shared queue back, calls `reclaim_steered`, then `flush_next_queued`, which
pops the next entry and dispatches by kind — a `Messages` batch through
`start_turn` (record + commit each user bubble, open the stream, spawn the
backend on the newline-joined prompt), a `Shell` entry through `run_shell`.

- **`StreamDone` / `Error`**: reclaim, then flush — **under the Ctrl+O overlay
  too** (codex's queue drains at turn end regardless of its Ctrl+T view). That
  doesn't violate invariant 4: it records history and *queues* the user bubbles;
  `term` never flushes pending lines into the alternate screen, and the overlay
  return's draw flushes them above the live region.
- **Esc interrupt**: same path (the `Action::Interrupt` arm ends in
  `dispatch_after_turn`), so a message the interrupted turn never read is sent
  right away as the next turn — codex's `submit_pending_steers_after_interrupt`,
  arrived at by the general rule rather than a special case.
- **`/clear`**: `App::clear_conversation` drops `steered` with the rest of the
  turn, and the boundary drains the shared handle too — the dying backend must
  not open the next turn by reading a message from the conversation just wiped.

## The subagent's queue (`docs/agent-tool.md`)

An agent session view is the same picture one level down, and deliberately the
same code:

- `AgentRegistry` already had the per-agent queue (`queue_input` /
  `take_pending_inputs`); it now feeds `run_agent`'s `pending_inputs` seam, so a
  subagent announces its takes with `StreamEvent::Steered` on the agent channel
  exactly as the main turn does on the reply channel.
- `AgentRun::queued: Vec<String>` is the mirror, `AgentRun::apply`'s `Steered`
  arm the delivery (flush the streamed segment, drop the row, `push_user_message`
  on **that agent's** transcript), and `AgentRun::reclaim_queued` the fallback.
- `ui::queued_lines` renders the **viewed agent's** queue in the agent view and
  the main session's two sets outside it — same rows, same geometry.
- **The registry decides, not the roster.** `ReplySource::spawn_agent_chat`
  returns an `AgentChatDelivery`: `Queued` (the loop is running — park the row
  and wait for the round boundary) or `Started` (it was idle — a continuation run
  carries the message as its newest user turn, so the transcript records it at
  once and the bubble commits, an idle submit one level down). A roster status
  that lagged the registry by one event would either strand the row forever or
  record the message twice, which is exactly why the key arm no longer decides.
- **A settled run reconciles** (`Session::reclaim_agent_chat`): the unread
  messages come off the registry's queue *and* the roster's rows, and a run that
  finished **naturally** gets them straight back as a chat continuation. A run
  that failed, or that the user stopped with `x`, keeps nothing — there is
  nothing to continue, and restarting an agent the user just killed is the
  opposite of what the key meant.

The bug this fixed on that side: the view recorded the message the instant it
was typed, claiming the agent had read something it had not, and showed no
pending state at all.

### Display (`ui/footer.rs`)

Pending messages render **above the box, in the streaming strip** — stacked just
under the status line's gap, between it and the box's top rule — each **inset two
columns** (`QUEUED_INDENT`). Past the indent a text message is styled **exactly
like a sent user message** (the `❯ ` bullet, the dark background, wrapped) and a
**shell command** like the exec cell it becomes (the red `! ` `Role::Shell`
header). What is going into the **running** turn leads — it is what happens next
— and a **blank row divides each turn from the next**, so Tab-opened follow-ups
and standalone `!` commands read as separate turns:

```
● Happy to help!…            ← streaming preview
( ●    ) Working… (…)         ← status line

  ❯ Hello                     ← handed to the RUNNING turn (Enter): two-space
  ❯ World                       inset, user-style; gone at its next round
                                boundary, where each becomes a real bubble
                              ← blank: a turn boundary
  ! ls -la                    ← a Shell entry (Enter in shell mode): red `! `,
                                runs locally as its own turn
────────────────────────────
❯                             ← the input box
────────────────────────────
```

The strip's height collapses to 0 when idle and both sets are only non-empty
while a turn runs, so the rows live naturally in the strip. `queued_rows` /
`queued_lines` (threaded through `live_height`/`live_layout`/`input_box`):

- `queued_lines(app, width)` — **whose** messages depends on which conversation
  is on screen, the same rows either way. The main view renders `App::steered`
  first (one blank-divided block: the model reads them together at its next
  round boundary) then each `queued` entry — a `Messages` batch's messages via
  `message_lines(Role::User, …)`, a `Shell` entry via
  `message_lines(Role::Shell, …)` (the red `! ` header). An **agent session
  view** renders that agent's `queued` and nothing else: the view is the agent's
  world, and the main session's rows re-appear on return. Every row is prefixed
  with `QUEUED_INDENT` (`indent_queued_line` keeps the indent outside the dark
  block); **uncapped** (the whole backlog shows).
- `queued_rows(app, width)` — `queued_lines(...).len()`, so the strip reserves
  exactly what `render_live` paints (they can't drift). `live_height`'s
  terminal-height clamp still bounds the region.

The **Ctrl+O transcript view shows the backlog too**: `ui::transcript_lines`
appends `queued_lines` after the live tail — the same inset rows, reading as
"pending, not yet read" below the in-progress reply — so opening the overlay
never hides a pending message, and the round boundary turns it into a real
transcript user entry before the viewer's eyes.

The `tab to queue next turn` binding is listed in the `?` shortcuts band
(alongside `alt+↑ to edit queue`, `docs/shortcuts.md`).

## Known divergences from codex

- **The steer lands at a round boundary, not at once.** Codex injects into its
  running task; ours rides `run_agent`'s per-round `pending_inputs` take, so the
  message is read after the round's tool results rather than mid-request. In
  practice that is a tool call away, and it is the only point where a
  Chat Completions request can honestly gain a message.
- **A steered message is visibly pending until then.** Codex's steer becomes a
  transcript message immediately; ours waits above the box as an inset row and
  commits when `StreamEvent::Steered` says the model actually has it. Claiming
  the model read something it has not is the bug this design exists to avoid —
  and it is exactly what the subagent view used to do.
- **Enter's batching survives as the fallback, not the rule.** Consecutive
  Enters a turn never read are reclaimed into **one** next turn (Claude-Code's
  merge), where codex's queue makes every message its own turn. A turn that does
  read them gets them as separate user messages, in order, which is codex's
  shape.
- **We keep the red interrupt notice.** Codex's steer path shows a gentle info;
  we keep our standard `Conversation interrupted` notice, then send whatever the
  turn never read — the interrupt honestly happened.
- **Slash commands aren't queued.** A bare `/token` runs inline via the palette;
  only plain text queues. (The palette's own Tab still runs the highlighted
  command — it wins over the queue Tab.)
- **Idle Tab is a no-op.** Codex's idle Tab submits like Enter (and inserts a tab
  in a `!` bang draft); ours does nothing — Tab only queues against a running
  turn, which is all the user asked for.
- **Alt+Up restores the last entry (like codex), as an editable draft.**
  Codex's `edit_queued_message` pops the most recent queued entry; ours pops the
  most recent **entry** (`pop_back`) — a `Messages` batch newline-joined into the
  composer (a batch can hold several, where codex's entry is one), a `Shell` entry
  back as `!command` re-entering shell mode — leaving the earlier entries queued.
  With no follow-up left it reaches the running turn's own messages, which codex
  has no equivalent for: a steer there is gone the moment it is submitted, where
  ours can still be taken back until the turn reads it.
- **Mid-turn `!` commands now match codex (run locally, not queued as text).** A
  shell command submitted while a turn streams queues as its own `Shell` entry and
  runs locally when its turn comes — codex's action-tagged `RunShell` dispatch.
  This retires the v1 limitation (`docs/shell-command.md`) where a mid-turn
  `!command` queued as literal text and was sent to the backend. Each `!` command
  is a **standalone** entry (never merged into a text batch), so the user's
  "individual separate queue" intent and codex's per-completion dispatch coincide.
- **No per-queue edit hint row.** Codex shows a dim hint line under its queued
  list; we spend no strip row on it — the bindings live in the `?` shortcuts band
  (`alt+↑ to edit queue`, `tab to queue next turn`).
- **Quitting drops both sets.** They only exist mid-turn; Ctrl+C there quits
  (the composer is empty) and everything pending is discarded with the session.

## Testing

- `steer`: a pushed message is taken once, in submission order; every clone is
  the same queue; `take_last` reclaims only an undelivered message.
- `llm::agent`: a steered message lands in the **next round's** context, after
  that round's tool results, and is announced with `StreamEvent::Steered`; a
  background notice leads it in the same round and is **not** announced (it is
  no user bubble).
- `app` (steering): Enter mid-turn returns `Action::Steer` and parks the text in
  `steered`, not `queued`; consecutive Enters keep submission order; the text is
  recorded for ↑ recall; `deliver_steered` drops the row and records the user
  message, **finalising the reply streamed ahead of it** (assistant then user in
  history); Enter during a `!` shell turn still queues a follow-up; a draft with
  attachments queues instead of steering; the reclaim makes one batch at the
  **front** of the queue, ahead of a Tab follow-up, and invents nothing when
  empty; Alt+Up prefers the follow-up queue and only then asks the boundary
  (`Action::ReclaimSteered`), with `recall_steered` returning the text to the
  composer; `/clear` mid-turn drops them; a **`/compact` turn takes none**
  (`turn_steerable` is false there, so the draft queues as a follow-up); and a
  message the turn never read **blocks the interrupt-undo** — the interrupt is
  what sends it, so the submission must not be pulled back at the same time
  (`docs/interrupt.md`).
- `app` (follow-up queue): **Tab mid-turn opens a new batch** (composer
  consumed, a second batch added) and never steers; the batches drain one turn
  at a time (`drain_next_batch` yields the first, then the follow-up); **idle Tab
  and empty mid-turn Tab are no-ops**; idle Enter still submits; **Alt+Up pulls
  only the last batch** into the composer newline-joined, leaving earlier
  batches queued (and concats a multi-message batch); Alt+Up with a draft is a
  no-op; `/clear` mid-turn drops the backlog.
- `app` (mid-turn shell, `docs/shell-command.md`): a `!command` mid-turn queues
  as a standalone `QueuedTurn::Shell` entry (not text), recording `!command` for
  ↑ recall and exiting the mode; a `Shell` entry is **never merged** — a Tab-text
  after it opens a fresh `Messages` batch, and two `!` commands queue as two
  separate `Shell` entries; **Alt+Up over a `Shell` entry re-enters shell mode**
  with the command in the composer.
- `agents` (the subagent's own queue): a message queued into a running agent
  waits on its rows and claims **nothing** on its transcript; its round boundary
  (`StreamEvent::Steered`) turns it into a user message there, after the
  streamed segment ahead of it is finalised; a settled run hands its unread
  messages back once.
- `app` (agent queue): `queue_agent_chat` parks the row and bumps
  `agents_generation` (the transcript cache is told); the round boundary lands
  it on **that agent's** transcript and never the main conversation's;
  `reclaim_agent_chat` hands the unread ones back.
- `stream::dummy`: the offline backend takes a queued message right after its
  **first** tool call resolves (not at the end of the turn) and drains the
  queue; `spawn_agent_chat` queues into a running agent and declines with no
  registry attached.
- `ui` (queue): `queued_rows` is 0 empty / counts a batch's messages / counts a
  long message's wrapped rows / **counts the blank between batches**;
  `queued_lines` styles each message like a user message, **divides turns with a
  blank row**, renders a `Shell` entry with the red `! ` `Role::Shell` header,
  shows the steered messages **before** the follow-ups, and in an **agent
  session view** shows that agent's queue and none of the main session's;
  `live_height` grows with the queue; `render_live` draws it above the box;
  `transcript_lines` lists the backlog after the live tail in the same inset
  style, **including a message handed to the running turn** (with the cache
  signature counting both sets, so queueing one refreshes a page that would
  otherwise sit frozen), and `agent_transcript_lines` does the same for the
  viewed agent's own queue — the Ctrl+O view never hides a pending message.
- `scripts/smoke.sh` Phase 12 (mid-turn delivery): submit `hello there`, queue
  `world` and `again` mid-stream **with Enter** (both inset rows show), then
  both commit at column 0 **while the status line is still up** and the turn
  ends with its own `Done for` summary — a second turn (`Finished for`) must
  **not** run.
- `scripts/smoke.sh` Phase 13 (interrupt-send): submit `hello`, queue `world`,
  press Esc — `Conversation interrupted` commits and what the turn never read is
  sent right away.
- `scripts/smoke.sh` Phase 14 (Alt+Up restore): queue `world` with **Enter**
  (into the running turn) and `again` with **Tab** (a follow-up), press Alt+Up —
  only `again` returns to the box as the draft while `world` keeps its inset row.
- `scripts/smoke.sh` Phase 21 (Tab follow-up): submit `hello`, queue `world`
  with **Enter** then `later` with **Tab** mid-stream (a blank divides them);
  `world` is read by the running turn and `later` runs as a **separate** turn
  after it (`Finished for`) — the extra turn the all-Enter Phase 12 never
  produces.
- `scripts/smoke.sh` Phase 24 (mid-turn shell queue): submit `hello there`, queue
  `world` (Enter) and `!echo smoke_queue_ok` (Enter in shell mode) mid-stream —
  both show inset (`  ❯ world`, `  ! echo smoke_queue_ok`); then the running turn
  reads `world` and the command runs **locally** as its own turn, committing the
  exec cell (`! echo …` header + `⎿ smoke_queue_ok`), never a `❯ !echo …` user
  message.
- `scripts/smoke.sh` Phase 29 (queue under the overlay): submit `hello there`,
  queue `world` mid-stream, press Ctrl+O — the inset `  ❯ world` row shows in
  the transcript view; the turn takes it at its round boundary *under the
  overlay* (the column-0 `❯ world` entry appears and the turn runs on to its
  `Done for` summary, the overlay still open), and the Ctrl+O return flushes it
  all into the inline view.
- `scripts/smoke.sh` Phase 96 (the subagent's queue): open the demo subagent's
  session while it works and type a message — it waits inset above the box, its
  loop takes it at its own round boundary (after its `write` batch) where it
  commits on **that agent's** transcript while the agent still runs, and the Esc
  return shows no trace of it in the main conversation.

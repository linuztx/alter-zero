# Queueing messages while a turn streams

Port of openai/codex's *queued user messages*: while a reply is generating, a
submitted message doesn't have to wait at the keyboard — it joins a queue (every
entry shown above the box) and flushes automatically as a later turn. Two keys
queue, with **different intent** (codex's `submit_keys` vs `queue_keys` split,
mapped onto our no-steer model):

- **Enter** appends to the batch currently being accumulated — consecutive
  Enters flush **together as the immediate next turn** (Claude-Code-style
  batching).
- **Tab** opens a **new** batch — its message runs as a *separate follow-up
  turn* that iterates **after** the first queue finishes, instead of merging
  into it (codex's Tab-to-queue).

So the queue is a sequence of **turn-batches**, and each batch is one future
turn. See `CLAUDE.md` for where this sits in the runtime model.

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

We have no steering, so we map codex's two-key intent onto the queue itself: the
queue is a `VecDeque<QueuedTurn>` of **typed entries** — codex's action-tagged
queued messages (`QueuedInputAction`), drained FIFO one entry per turn-end. A
`QueuedTurn::Messages { texts, images }` is a text batch (sent to the model,
its Ctrl+V attachments riding along as `(placeholder, path)` pairs — see
`docs/image-paste.md`); a `QueuedTurn::Shell(String)` is a standalone `!`
command (**run locally**). The variant is the dispatch discriminator, so
submission order is preserved across mixed entries.

**Enter** appends to the last `Messages` batch (the Claude-Code merge — those
messages share one turn); **Tab** opens a new `Messages` batch (codex's
one-per-completion sequencing — that message runs as its own turn after the
batches already queued); a **shell-mode `!` draft** queues as its own `Shell`
entry, **never merged** (a `Shell` at the back of the queue isn't a `Messages`,
so the next Enter-text starts a fresh batch — codex's per-completion shell
dispatch). On **every** turn end — `StreamDone`, `Error`, or **Esc** (which
interrupts and flushes right away) — the loop pops the **front** entry and
dispatches it: a text batch to the model, a `!` command run locally. The
remaining entries each flush at their own following turn-end, so the queue
iterates in order.

A `Messages` batch commits as its own user bubbles and the backend gets them
joined with newlines as a single prompt (`start_turn`, a Submit being a batch of
one); a `Shell` entry runs through `run_shell` exactly like an idle `!command`
(`begin_shell` + `spawn_shell_command`), committing a codex-style exec cell.

### State (`app/queue.rs`)

- `App.queued: VecDeque<QueuedTurn>` — the typed entries awaiting their turns
  (`Messages { texts, images }` text batches and `Shell(String)` commands). We
  never push an empty entry, so `is_empty()`/`len()` count entries and
  `queued[0]` is the next turn.
- `App::queue_draft(new_batch: bool)` — the shared mid-turn **text**-queue path.
  Consumes the composer and records the text in `input_history` (so ↑ recalls it
  like a submit). `new_batch` picks the semantics: `false` (Enter) appends to the
  last `Messages` batch (`back_mut`), `true` (Tab) pushes a new one; an empty
  queue — or a `Shell` entry at the back — starts a fresh batch either way. A
  **shell-mode draft short-circuits to `queue_shell`** instead.
- `App::queue_shell()` — queues the shell-mode draft as a standalone
  `QueuedTurn::Shell` entry (codex's `submit_queued_shell_prompt` on a queued
  `RunShell` action), exits the mode, and records the full `!command` for ↑
  recall (mirroring the idle `Action::RunShell` path). **Always a new entry,
  never merged.**
- **Enter while a turn is in flight** (`on_key_conversation`): with a non-empty,
  non-command composer, `queue_draft(false)` — which routes a shell draft to
  `queue_shell`. Idle Enter is unchanged (`Action::Submit` / `Action::RunShell`);
  a bare `/token` keeps the palette path.
- **Tab while a turn is in flight** (`on_key_conversation`, after the palette's
  Tab arm so the menu still wins): with a non-empty composer, `queue_draft(true)`
  → a new follow-up batch (or, in shell mode, a `Shell` entry — `queue_shell`
  ignores `new_batch`, a shell command being always standalone). **Idle or empty
  Tab falls through to a no-op** — Tab only queues against a running turn.
- `App::drain_next_batch() -> Option<QueuedTurn>` — pops the **front** entry
  (FIFO) for the loop to dispatch as the next turn; `None` when nothing is queued.
  Popping one per turn-end is what makes the entries iterate sequentially.
- `App::drain_last_batch() -> Option<QueuedTurn>` — pops the **last** entry
  (`pop_back`), for Alt+Up. The earlier entries stay queued; `None` when empty.
- **Alt+Up** (`KeyCode::Up` with `ALT`, an **empty** composer, a non-empty queue):
  `drain_last_batch` the most-recent entry into the composer to edit/extend/drop,
  the earlier entries left queued (codex's `edit_queued_message`). A `Messages`
  batch returns as one newline-joined draft (its messages oldest first); a `Shell`
  entry returns as `!command` — `recall_input` re-absorbs the bang, so the
  composer **re-enters shell mode** with the red `! ` prompt. Guarded on an empty
  composer so it never clobbers a draft.

### Flush (`main.rs`, the I/O boundary)

`flush_next_queued(…)` pops the next entry (`drain_next_batch`) and dispatches by
kind — a `Messages` batch through `start_turn` (record + commit each user bubble,
open the stream, spawn the backend on the newline-joined prompt), a `Shell` entry
through `run_shell` (`begin_shell` + `spawn_shell_command`, the same path an idle
`!command` takes). It returns the new in-flight handle, or `None` when the queue
is empty. **Every** turn-end drain site calls it, so they can't drift:

- **`StreamDone` / `Error`** (after `on_stream_event` returns "ended"): flush the
  next entry — **under the Ctrl+O overlay too** (codex's queue drains at turn
  end regardless of its Ctrl+T view, the open transcript following the new turn
  live). Dispatching there doesn't violate invariant 4: it only records history
  and *queues* the user bubbles — `term` never flushes pending lines into the
  alternate screen, and the overlay return's `repaint_conversation`/`reflow`
  drops + regenerates them from history. The **next** turn-end flushes the
  **next** entry, so the queue iterates one turn at a time.
- **Esc interrupt** (`Action::Interrupt`): after committing the partial + the red
  `Conversation interrupted` notice, flush the **front** entry (the first queue)
  right away; later entries iterate at the following turn-ends.

(The Submit arm still calls `start_turn` directly with the just-typed text, and
the idle `Action::RunShell` arm calls `run_shell` — `flush_next_queued` is only
the *queue* drain.)

Because a message can only be queued *while streaming*, and a non-empty queue
always starts a fresh turn the instant the current one ends — in whichever view —
the invariant holds: **the queue is non-empty only while a turn is active** (no
draw ever shows a queued band at idle).

### Display (`ui/footer.rs`)

Queued entries render **above the box, in the streaming strip** — stacked just
under the status line's gap, between it and the box's top rule — each **inset two
columns** (`QUEUED_INDENT`). Past the indent a text message is styled **exactly
like a sent user message** (the `❯ ` bullet, the dark background, wrapped) and a
**shell command** like the exec cell it becomes (the red `! ` `Role::Shell`
header). A **blank row divides each entry from the next**, so Tab-opened
follow-ups and standalone `!` commands read as separate turns:

```
● Happy to help!…            ← streaming preview
( ●    ) Working… (…)         ← status line

  ❯ Hello                     ← batch 1 (Enter): two-space inset, user-style
  ❯ World
                              ← blank: an entry boundary
  ! ls -la                    ← a Shell entry (Enter in shell mode): red `! `,
                                runs locally as its own turn
────────────────────────────
❯                             ← the input box
────────────────────────────
```

The strip's height collapses to 0 when idle and the queue is only non-empty while
streaming, so the queued rows live naturally in the strip. `queued_rows` /
`queued_lines` (threaded through `live_height`/`live_layout`/`input_box`):

- `queued_lines(app, width)` — each `Messages` batch's messages rendered via
  `message_lines(Role::User, …)` and each `Shell` entry via
  `message_lines(Role::Shell, …)` (the red `! ` header), every row prefixed with
  `QUEUED_INDENT` (`indent_queued_line` keeps the indent outside the dark block),
  with a blank `Line` inserted between entries; **uncapped** (the whole backlog
  shows).
- `queued_rows(app, width)` — `queued_lines(...).len()`, so the strip reserves
  exactly what `render_live` paints (they can't drift). `live_height`'s
  terminal-height clamp still bounds the region.

The **Ctrl+O transcript view shows the backlog too**: `ui::transcript_lines`
appends `queued_lines` after the live tail — the same inset rows, reading as
"pending, not yet sent" below the in-progress reply — so opening the overlay
never hides a queued message, and when the turn ends the drain (above) turns
the front entry into a real transcript user entry before the viewer's eyes.

The `tab to queue next turn` binding is listed in the `?` shortcuts band
(alongside `alt+↑ to edit queue`, `docs/shortcuts.md`).

## Known divergences from codex

- **Enter batches, Tab sequences — no steering.** Codex's Enter steers the
  running turn and Tab queues; *every* codex queued message is its own turn
  (one per completion). We have no steer path, so Enter instead **batches** into
  the next turn (Claude-Code style) and **Tab** is what splits the queue into
  sequential follow-up turns. Esc sends the front batch right away (codex does
  that only for steers).
- **We keep the red interrupt notice.** Codex's steer path shows a gentle info;
  we keep our standard `Conversation interrupted` notice, then send the front
  batch — the interrupt honestly happened.
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
  The user edits/extends/drops it and re-queues however they like.
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
- **Quitting drops the queue.** The queue only exists mid-turn; Ctrl+C there quits
  (the composer is empty) and the queue is discarded with the session.

## Testing

- `app` (queue): Enter mid-turn appends to one batch in order; **Tab mid-turn
  opens a new batch** (composer consumed, a second batch added); an Enter after a
  Tab joins the Tab's batch; the batches drain one turn at a time
  (`drain_next_batch` yields the first queue, then the follow-up); a Tab-queued
  message is recorded in `input_history` (↑ recalls it); **idle Tab and empty
  mid-turn Tab are no-ops** (nothing queued, draft intact); idle Enter still
  submits; `drain_next_batch` takes the front batch and empties it; **Alt+Up pulls
  only the last batch** into the composer newline-joined, leaving earlier batches
  queued (and concats a multi-message last batch); Alt+Up with a draft is a no-op;
  `/clear` mid-turn drops the backlog.
- `app` (mid-turn shell, `docs/shell-command.md`): a `!command` mid-turn queues
  as a standalone `QueuedTurn::Shell` entry (not text), recording `!command` for
  ↑ recall and exiting the mode; a `Shell` entry is **never merged** — a text
  Enter after it opens a fresh `Messages` batch, and two `!` commands queue as two
  separate `Shell` entries; **Alt+Up over a `Shell` entry re-enters shell mode**
  with the command in the composer.
- `ui` (queue): `queued_rows` is 0 empty / counts a batch's messages / counts a
  long message's wrapped rows / **counts the blank between batches** (two
  single-message batches are three rows); `queued_lines` styles each message
  like a user message and **divides batches with a blank row**, and renders a
  `Shell` entry with the red `! ` `Role::Shell` header (a text batch + a shell
  entry are three lines: message, blank divider, command); `live_height` grows
  with the queue; `render_live` draws the queue above the box;
  `transcript_lines` lists the backlog after the live tail in the same inset
  style (the Ctrl+O view never hides a queued message).
- `scripts/smoke.sh` Phase 12 (batch-send): submit `hello`, queue `world` and
  `again` mid-stream **with Enter** (both inset rows show), then the backlog
  batch-sends as ONE turn.
- `scripts/smoke.sh` Phase 13 (interrupt-send): submit `hello`, queue `world`,
  press Esc — `Conversation interrupted` commits and the front batch is sent
  right away.
- `scripts/smoke.sh` Phase 14 (Alt+Up restore): queue `world` with **Enter**
  (batch 1) and `again` with **Tab** (batch 2) mid-stream, press Alt+Up — only
  `again` returns to the box as the draft while the `world` batch stays queued
  (its inset row remains).
- `scripts/smoke.sh` Phase 21 (Tab follow-up): submit `hello`, queue `world`
  with **Enter** then `later` with **Tab** mid-stream (a blank divides them),
  and watch `world` send as one turn and `later` send as a **separate** turn
  after it — a third turn the all-Enter Phase 12 never produces.
- `scripts/smoke.sh` Phase 24 (mid-turn shell queue): submit `hello there`, queue
  `world` (Enter) and `!echo smoke_queue_ok` (Enter in shell mode) mid-stream —
  both show inset (`  ❯ world`, `  ! echo smoke_queue_ok`); then `world` runs as a
  model turn and the command runs **locally** as its own turn, committing the exec
  cell (`! echo …` header + `⎿ smoke_queue_ok`), never a `❯ !echo …` user message.
- `scripts/smoke.sh` Phase 29 (queue under the overlay): submit `hello there`,
  queue `world` mid-stream, press Ctrl+O — the inset `  ❯ world` row shows in
  the transcript view; when turn 1 ends *under the overlay* the queue
  dispatches right there (the column-0 `❯ world` user entry appears and turn 2
  runs to its `Finished for` summary, the overlay still open), and the Ctrl+O
  return repaints the inline view with both turns.

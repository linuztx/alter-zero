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
queue is a `VecDeque<Vec<String>>` of **turn-batches**, each batch one future
turn. **Enter** appends to the last batch (the Claude-Code merge — those messages
share one turn); **Tab** opens a new batch (codex's one-per-completion sequencing
— that message runs as its own turn after the batches already queued). On
**every** turn end — `StreamDone`, `Error`, or **Esc** (which interrupts and
flushes right away) — the loop pops the **front** batch and sends it as one turn;
the remaining batches each flush at their own following turn-end, so Tab-opened
follow-ups iterate in order.

A batch's messages commit as their own user bubbles and the backend gets them
joined with newlines as a single prompt (`start_turn`, a Submit being a batch of
one).

### State (`app.rs`)

- `App.queued: VecDeque<Vec<String>>` — the turn-batches awaiting their turns.
  Each inner `Vec` is one batch (one turn); we never push an empty batch, so
  `is_empty()`/`len()` count batches and `queued[0]` is the next turn's messages.
- `App::queue_draft(new_batch: bool)` — the shared mid-turn queue path. Consumes
  the composer, re-absorbs a shell-mode `!` (queued as literal text, a v1
  limitation), and records the text in `input_history` (so ↑ recalls it like a
  submit). `new_batch` picks the semantics: `false` (Enter) appends to the last
  batch (`back_mut`), `true` (Tab) pushes a new batch; an empty queue starts a
  fresh batch either way.
- **Enter while a turn is in flight** (`on_key_conversation`): with a non-empty,
  non-command composer, `queue_draft(false)`. Idle Enter is unchanged
  (`Action::Submit`); a bare `/token` keeps the palette path.
- **Tab while a turn is in flight** (`on_key_conversation`, after the palette's
  Tab arm so the menu still wins): with a non-empty composer, `queue_draft(true)`
  → a new follow-up batch. **Idle or empty Tab falls through to a no-op** — Tab
  only queues against a running turn (the user's spec; idle behaviour unchanged).
- `App::drain_next_batch() -> Vec<String>` — pops the **front** batch (FIFO) for
  the loop to send as the next turn; empty when nothing is queued. Popping one
  per turn-end is what makes Tab batches iterate sequentially.
- `App::drain_last_batch() -> Vec<String>` — pops the **last** batch (`pop_back`),
  for Alt+Up. The earlier batches stay queued; empty when nothing is queued.
- **Alt+Up** (`KeyCode::Up` with `ALT`, an **empty** composer, a non-empty queue):
  `drain_last_batch` the most-recent batch into the composer as one newline-joined
  draft (its own messages oldest first) via `recall_input`, to edit/extend/drop —
  the earlier batches stay queued (codex's `edit_queued_message` pops the most
  recent entry the same way). Guarded on an empty composer so it never clobbers a
  draft.

### Flush (`main.rs`, the I/O boundary)

`start_turn(texts, …)` records + commits each user bubble, opens the stream,
resets the per-turn clocks/`committed`, and spawns the backend once on the
newline-joined prompt. Both the Submit arm and every flush call it. After a turn
goes idle the loop `drain_next_batch()`es one batch and — if non-empty — starts
it:

- **`StreamDone` / `Error`** (after `on_stream_event` returns "ended"): pop the
  next batch — *only in the conversation view* (committing under the Ctrl+O
  overlay would violate invariant 4). The **next** turn-end pops the **next**
  batch, so Tab follow-ups iterate one turn at a time.
- **Esc interrupt** (`Action::Interrupt`): after committing the partial + the red
  `Conversation interrupted` notice, pop the **front** batch (the first queue) and
  send it right away; later Tab batches iterate at the following turn-ends.
- **Returning from the Ctrl+O overlay**: if a turn *ended while the overlay was
  up* (`inflight` is now `None`), the deferred pop runs after
  `repaint_conversation`.

Because a message can only be queued *while streaming*, and a non-empty queue
always starts a fresh turn the instant the current one ends, the invariant holds:
**the queue is non-empty only while a turn is active** (no draw ever shows a
queued band at idle).

### Display (`ui.rs`)

Queued messages render **above the box, in the streaming strip** — stacked just
under the status line's gap, between it and the box's top rule — each **inset two
columns** (`QUEUED_INDENT`) and past the indent styled **exactly like a sent user
message** (the `❯ ` bullet, the dark background, wrapped). A **blank row divides
each turn-batch from the next**, so Tab-opened follow-ups read as separate turns
from the first queue:

```
● Happy to help!…            ← streaming preview
( ●    ) Working… (…)         ← status line

  ❯ Hello                     ← batch 1 (Enter): two-space inset, user-style
  ❯ World
                              ← blank: a batch boundary (Tab opened the next)
  ❯ Later                     ← batch 2 (Tab): its own follow-up turn
────────────────────────────
❯                             ← the input box
────────────────────────────
```

The strip's height collapses to 0 when idle and the queue is only non-empty while
streaming, so the queued rows live naturally in the strip. `queued_rows` /
`queued_lines` (threaded through `live_height`/`live_layout`/`input_box`):

- `queued_lines(app, width)` — each batch's messages rendered via
  `message_lines(Role::User, …)`, every row prefixed with `QUEUED_INDENT`
  (`indent_queued_line` keeps the indent outside the dark block), with a blank
  `Line` inserted between batches; **uncapped** (the whole backlog shows).
- `queued_rows(app, width)` — `queued_lines(...).len()`, so the strip reserves
  exactly what `render_live` paints (they can't drift). `live_height`'s
  terminal-height clamp still bounds the region.

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
- **Alt+Up restores the last batch (like codex), as a newline-joined draft.**
  Codex's `edit_queued_message` pops the most recent queued entry; ours pops the
  most recent **batch** (`pop_back`) — its messages newline-joined into the
  composer (a batch can hold several, where codex's entry is one) — leaving the
  earlier batches queued. The user edits/extends/drops it and re-queues however
  they like.
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
- `ui` (queue): `queued_rows` is 0 empty / counts a batch's messages / counts a
  long message's wrapped rows / **counts the blank between batches** (two
  single-message batches are three rows); `queued_lines` styles each message
  like a user message and **divides batches with a blank row**; `live_height`
  grows with the queue; `render_live` draws the queue above the box.
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
- `scripts/smoke.sh` Phase 18 (Tab follow-up): submit `hello`, queue `world`
  with **Enter** then `later` with **Tab** mid-stream (a blank divides them),
  and watch `world` send as one turn and `later` send as a **separate** turn
  after it — a third turn the all-Enter Phase 12 never produces.

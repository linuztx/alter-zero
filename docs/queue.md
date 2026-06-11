# Queueing messages while a turn streams

Port of openai/codex's *queued user messages*: while a reply is generating, a
submitted message doesn't have to wait at the keyboard — it joins a queue and is
sent automatically, one per turn, as each turn ends. See `CLAUDE.md` for where
this sits in the runtime model.

## What codex does (findings)

The queue lives on `chatwidget/input_queue.rs::InputQueueState` as
`queued_user_messages: VecDeque<QueuedUserMessage>` (plus steer/slash machinery
we don't have). The mechanics that matter:

- **Queue vs send** (`input_flow.rs::queue_user_message_with_options`): on submit,
  `if !is_session_configured() || is_user_turn_pending_or_running() { push_back } else { submit }`.
  A turn in flight means the message is queued, not sent.
- **Drain, one at a time** (`input_flow.rs::maybe_send_next_queued_input`, called
  from `turn_runtime.rs::on_task_complete`): when a turn completes it pops **one**
  message off the **front** and submits it as the next turn (`break` after a
  `Plain` submit). Each queued message becomes its own turn — the queue is never
  concatenated.
- **Display** (`bottom_pane/pending_input_preview.rs`): a dim italic
  "Queued follow-up inputs" section, each entry prefixed `↳`, truncated.
- **Edit/dequeue** (`chatwidget/interaction.rs` + `input_restore.rs`): the
  `edit_queued_message` binding (Alt+Up) pops the **last** queued message
  (`pop_back`) back into the composer to edit, resend, or drop.
- **Interrupt** (`interaction.rs` ~115, `input_restore.rs::on_interrupted_turn`):
  two tiers. *Plain* queued messages are restored to the composer on Esc. But a
  *steer* (a message aimed at the running turn) takes the
  `submit_pending_steers_after_interrupt` path: Esc interrupts the current turn
  and **resubmits the steer immediately as a fresh turn**, with a gentle info
  notice instead of the red error.

## What we build

We have one simple queue, not codex's steer/queue split, so we give it the
unified form the user asked for: **Esc interrupts the current turn and sends the
next queued message right away** — codex's steer-after-interrupt behaviour, made
the rule for every queued message.

### State (`app.rs`)

- `App.queued: VecDeque<String>` — messages awaiting their own turns.
- **Enter while a turn is in flight** (`on_key_conversation`): with a non-empty,
  non-command composer, instead of the old "do nothing", consume the composer,
  record it in `input_history` (so ↑ still recalls it), and `push_back` to
  `queued`. Idle Enter is unchanged (`Action::Submit`). A bare `/token` keeps the
  palette path (slash commands aren't queued — they run inline via the palette as
  before).
- `App::dequeue() -> Option<String>` — `pop_front`, for the loop to start the
  next turn. FIFO, one per completed turn (codex's `maybe_send_next_queued_input`).
- **Alt+Up** (`KeyCode::Up` with `ALT`, an **empty** composer, a non-empty queue):
  `pop_back` the most-recent queued message into the composer via `recall_input`
  (so it can be edited, resent, or dropped). Guarded on an empty composer so it
  never clobbers a draft (the composer is empty in the normal flow — Enter emptied
  it when queueing).

### Flush (`main.rs`, the I/O boundary)

The `Action::Submit` body is extracted into `start_turn(...)` (record + commit the
user bullet, `begin_stream`, reset the per-turn clocks/`committed`, spawn the
backend, return the in-flight handle). Both the Submit arm and every flush call
it, so they can never drift. After a turn goes **idle**, the loop pops one queued
message and starts it:

- **`StreamDone` / `Error`** (branch 2, after `on_stream_event` returns "ended"):
  flush one — *only in the conversation view* (committing to scrollback under the
  Ctrl+O overlay would violate invariant 4).
- **Esc interrupt** (`Action::Interrupt` arm): after committing the partial + the
  red `Conversation interrupted` notice, flush one — this is the user's spec
  ("interrupt `hello`, send `world` right away"). Interrupt only arises in the
  conversation view, so no gate is needed.
- **Returning from the Ctrl+O overlay** (`Action::ToggleToolView` exit branch): if
  a turn *ended while the overlay was up* (`inflight` is now `None`), the deferred
  flush runs after `repaint_conversation`, so the queue can't get stuck.

Because a message can only be queued *while streaming*, and a non-empty queue
always starts a fresh turn the instant the current one ends, the invariant holds:
**the queue is non-empty only while a turn is active** (no draw ever shows a
queued band at idle — the flush happens in the same loop turn as the end event,
before the next frame).

### Display (`ui.rs`)

Queued messages render **above the box, in the streaming strip** — stacked just
under the status line's gap, between it and the box's top rule — each **inset
two columns** (`QUEUED_INDENT`) and past the indent styled **exactly like a sent
user message** (the `❯ ` bullet, the dark background, wrapped), so a queued
follow-up reads like it is already on its way:

```
● Happy to help!…            ← streaming preview
( ●    ) Working… (…)         ← status line

  ❯ Hello                     ← queued: two-space inset, user-message style
  ❯ World
────────────────────────────
❯                             ← the input box
────────────────────────────
```

The strip's height already collapses to 0 when idle, and the queue is only ever
non-empty while streaming, so the queued rows live naturally in the strip. This
needs a `queued_rows` parameter threaded through `live_height` / `live_layout` /
`input_box` (the strip's height now depends on the wrapped queue), separate from
the palette/shortcuts `band_rows` below the box:

- `queued_rows(app, width)` — the total wrapped height of every queued message
  (each via `message_lines(Role::User, …)`), capped at `QUEUED_MAX_ROWS`; 0 when
  empty. `live_height`/`live_layout` add it to the strip; `render_live` paints
  exactly that many — both go through `queued_lines`, so they can't drift.
- `queued_lines(app, width)` — each queued message rendered by `message_lines`
  (`❯` bullet, dark background) wrapped to `width` minus the indent, every row
  prefixed with `QUEUED_INDENT` (`indent_queued_line` folds the line style into
  the spans so the indent stays *outside* the dark block), concatenated and
  truncated to `QUEUED_MAX_ROWS` rows so a long queue can't crowd out the box.

## Known divergences from codex

- **No steer/queue split.** One `VecDeque<String>`; every queued message is the
  same "send as its own turn" kind. So Esc always sends the next queued message
  right away (codex only does that for steers; for plain queued messages it
  restores them to the composer).
- **We keep the red interrupt notice.** Codex's steer path shows a gentle info
  ("Model interrupted to submit steer instructions."); we keep our standard
  `Conversation interrupted` notice, then send the queued message — the interrupt
  honestly happened.
- **Slash commands aren't queued.** A bare `/token` runs inline via the palette
  (as today); only plain text queues.
- **No persistent edit hint.** Codex shows "Alt+Up edit last queued message" by
  the composer; we have no footer row, so Alt+Up is documented here and works, but
  isn't advertised in the static `?` shortcuts band.
- **Quitting drops the queue.** The queue only exists mid-turn; Ctrl+C there quits
  (the composer is empty) and the queue is discarded with the session.

## Testing

- `app` (queue): Enter mid-turn queues (composer cleared, `queued` grows, FIFO
  order preserved); idle Enter still submits; `dequeue` pops front / `None` empty;
  a queued message is recorded in `input_history` (↑ recalls it); Alt+Up pops the
  last into the composer; Alt+Up with a draft is a no-op (no clobber); Alt+Up on an
  empty queue is harmless.
- `ui` (queue): `queued_rows` is 0 empty / counts the queue / counts wrapped
  lines / caps at `QUEUED_MAX_ROWS`; `queued_lines` styles each message exactly
  like a user message (`❯` bullet, dark background) and wraps long ones;
  `live_height` grows with the queue; `render_live` draws the queue *above* the
  box (and the shortcuts band still shows below it, in its own slot).
- `scripts/smoke.sh` Phase 12 (auto-send): submit `hello`, queue `world`
  mid-stream (`❯ world` shows above the box while turn 1 streams), then both turns
  complete — `❯ hello` and `❯ world` both land, `Finished for` (turn 2) confirms
  `world` was auto-sent.
- `scripts/smoke.sh` Phase 13 (interrupt-send): submit `hello`, queue `world`,
  press Esc — `Conversation interrupted` commits and `world` is sent right away
  (`❯ world` + `Finished for`).

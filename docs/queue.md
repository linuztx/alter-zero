# Queueing messages while a turn streams

Port of openai/codex's *queued user messages*: while a reply is generating, a
submitted message doesn't have to wait at the keyboard — it joins a queue
(every entry shown above the box) and the **whole backlog is sent automatically
as the next turn** when the current one ends, Claude-Code style. See `CLAUDE.md`
for where this sits in the runtime model.

## What codex does (findings)

The queue lives on `chatwidget/input_queue.rs::InputQueueState` as
`queued_user_messages: VecDeque<QueuedUserMessage>` (plus steer/slash machinery
we don't have). The mechanics that matter:

- **Queue vs send** (`input_flow.rs::queue_user_message_with_options`): on submit,
  `if !is_session_configured() || is_user_turn_pending_or_running() { push_back } else { submit }`.
  A turn in flight means the message is queued, not sent.
- **Drain on completion** (`input_flow.rs::maybe_send_next_queued_input`, called
  from `turn_runtime.rs::on_task_complete`): when a turn completes the queue
  flushes from the **front** into the next turn. (Codex's plain path submits one
  `Plain` message per completion; its interrupt path
  `merge_user_messages_with_history_record` merges the backlog into a single
  fresh turn — Claude Code likewise batches everything queued into the next
  turn, which is the form we adopt for every flush.)
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
unified form the user asked for: on **every** turn end — `StreamDone`, `Error`,
or **Esc** (which interrupts the current turn and sends the backlog right away,
codex's steer-after-interrupt) — the **entire queue drains into one batched next
turn**: each message commits as its own user bubble, and the backend gets the
texts joined with newlines as a single prompt.

### State (`app.rs`)

- `App.queued: VecDeque<String>` — messages awaiting their own turns.
- **Enter while a turn is in flight** (`on_key_conversation`): with a non-empty,
  non-command composer, instead of the old "do nothing", consume the composer,
  record it in `input_history` (so ↑ still recalls it), and `push_back` to
  `queued`. Idle Enter is unchanged (`Action::Submit`). A bare `/token` keeps the
  palette path (slash commands aren't queued — they run inline via the palette as
  before).
- `App::drain_queued() -> Vec<String>` — takes the whole queue (FIFO order) for
  the loop to send as one batched next turn; empty when nothing is queued.
- **Alt+Up** (`KeyCode::Up` with `ALT`, an **empty** composer, a non-empty queue):
  `drain_queued` the **whole backlog** into the composer as one newline-joined,
  multi-line draft (oldest first) via `recall_input` — so the box shows
  `❯ Hello` / `  World` / … for editing, extending, or dropping; Enter then
  re-queues it as one message (codex merges pending messages the same way when
  restoring to the composer). Guarded on an empty composer so it never clobbers
  a draft (the composer is empty in the normal flow — Enter emptied it when
  queueing).

### Flush (`main.rs`, the I/O boundary)

The `Action::Submit` body is extracted into `start_turn(texts: Vec<String>, …)`:
for each text it records + commits the user bubble, then `begin_stream`s, resets
the per-turn clocks/`committed`, and spawns the backend **once** on the
newline-joined prompt (a Submit is a batch of one). Both the Submit arm and
every flush call it, so they can never drift. After a turn goes **idle**, the
loop `drain_queued()`s and — if the backlog is non-empty — starts it as one
batched turn:

- **`StreamDone` / `Error`** (branch 2, after `on_stream_event` returns "ended"):
  flush the backlog — *only in the conversation view* (committing to scrollback
  under the Ctrl+O overlay would violate invariant 4).
- **Esc interrupt** (`Action::Interrupt` arm): after committing the partial + the
  red `Conversation interrupted` notice, flush the backlog — this is the user's
  spec ("interrupt `hello`, send `world` right away"). Interrupt only arises in
  the conversation view, so no gate is needed.
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
  (each via `message_lines(Role::User, …)`), **uncapped** — the whole backlog
  shows, codex-style; 0 when empty. `live_height`/`live_layout` add it to the
  strip; `render_live` paints exactly that many — both go through
  `queued_lines`, so they can't drift. (`live_height`'s terminal-height clamp
  still bounds the region as a whole; the queue drains entirely at the next turn
  end, so a backlog taller than the screen is a momentary, self-healing state.)
- `queued_lines(app, width)` — each queued message rendered by `message_lines`
  (`❯` bullet, dark background) wrapped to `width` minus the indent, every row
  prefixed with `QUEUED_INDENT` (`indent_queued_line` folds the line style into
  the spans so the indent stays *outside* the dark block), concatenated.

## Known divergences from codex

- **No steer/queue split.** One `VecDeque<String>`; every flush batches the
  whole backlog into the next turn (codex's plain path sends one per completion
  and only its steer path merges; Claude Code batches like we do). So Esc always
  sends the backlog right away (codex only does that for steers; for plain
  queued messages it restores them to the composer).
- **We keep the red interrupt notice.** Codex's steer path shows a gentle info
  ("Model interrupted to submit steer instructions."); we keep our standard
  `Conversation interrupted` notice, then send the queued message — the interrupt
  honestly happened.
- **Slash commands aren't queued.** A bare `/token` runs inline via the palette
  (as today); only plain text queues.
- **Alt+Up restores everything, not just the last.** Codex's
  `edit_queued_message` pops one message; ours drains the whole backlog into the
  composer (the merge codex itself applies when restoring after an interrupt) —
  one binding, the entire queue editable at once.
- **No persistent edit hint.** Codex shows "Alt+Up edit last queued message" by
  the composer; we have no footer row, so Alt+Up is documented here and works, but
  isn't advertised in the static `?` shortcuts band.
- **Quitting drops the queue.** The queue only exists mid-turn; Ctrl+C there quits
  (the composer is empty) and the queue is discarded with the session.

## Testing

- `app` (queue): Enter mid-turn queues (composer cleared, `queued` grows, FIFO
  order preserved); idle Enter still submits; `drain_queued` takes everything in
  order and empties; a queued message is recorded in `input_history` (↑ recalls
  it); Alt+Up pulls the whole backlog into the composer newline-joined (cursor
  at the end, queue emptied); Alt+Up with a draft is a no-op (no clobber);
  Alt+Up on an empty queue is harmless.
- `ui` (queue): `queued_rows` is 0 empty / counts the queue / counts wrapped
  lines / is uncapped (ten messages are ten rows); `queued_lines` styles each
  message exactly like a user message (`❯` bullet, dark background) and wraps
  long ones; `live_height` grows with the queue; `render_live` draws the queue
  *above* the box (and the shortcuts band still shows below it, in its own slot).
- `scripts/smoke.sh` Phase 12 (batch-send): submit `hello`, queue `world` and
  `again` mid-stream (both inset rows show above the box while turn 1 streams),
  then the backlog batch-sends as ONE turn — `❯ world` and `❯ again` both
  commit, `Finished for` (turn 2) appears and `Completed for` (a third turn)
  must not.
- `scripts/smoke.sh` Phase 13 (interrupt-send): submit `hello`, queue `world`,
  press Esc — `Conversation interrupted` commits and the backlog is sent right
  away (`❯ world` + `Finished for`).
- `scripts/smoke.sh` Phase 14 (Alt+Up restore): queue `world` and `again`
  mid-stream, press Alt+Up — the inset queued rows clear and the box shows the
  multi-line draft (`❯ world` + the `  again` continuation).

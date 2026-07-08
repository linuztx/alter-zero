# Transient toasts

A **toast** is a one-line, self-clearing status message shown just above the
input box — the ephemeral counterpart to a committed `Role::System` /
`Role::Error` scrollback message. It exists for confirmations and soft
rejections that the user should *see* but never wants to keep: running `/copy`,
switching a model, or being told a command is disabled mid-turn. Instead of
minting a permanent bulletpoint in the conversation, the app "just shows it
there" and fades it after a few seconds.

```
❯ Hi

(  ●•· ) Working… (1s · ↑ 1 tokens · esc to interrupt)

  Copied last message to clipboard          ← the toast (dim), auto-clears ~4s
───────────────────────────────────────────────────────────────
❯
───────────────────────────────────────────────────────────────
```

## What routes through a toast

| trigger | toast | kind |
| --- | --- | --- |
| `/copy` succeeds | `Copied last message to clipboard` | info |
| `/copy` with no reply yet | `No agent response to copy` | error |
| `/copy` clipboard write fails | `Copy failed: {reason}` | error |
| `/model` switch succeeds | `Switched model to {id}` | info |
| `/model` switch with no key | `Can't switch to {id}: run /login to set {ENV}` | error |
| `/login` key saved | `Saved {ENV} — run /model to use {provider}` | info |
| `/login` write fails | `Couldn't write {path}: {e}` | error |
| `/resume` run mid-turn | `/resume is disabled while a task is in progress` | info |
| `/help` run mid-turn | `/help is disabled while a task is in progress` | info |

These were all committed as scrollback messages before; now they surface as
toasts and leave no trace in `history` (so they never appear in the Ctrl+O
transcript or a `/resume` rollout). The one thing that *stays* in scrollback is
**idle `/help`** — its multi-line command list is real content the user wants to
scroll, so it commits as a `Role::System` message as before. Only `/help`
*mid-turn* becomes the toast rejection above.

## `/model` and `/login` now work mid-turn

`/resume` still blocks while a turn is active (it swaps the whole conversation —
racing the stream). But `/model` and `/login` only replace the composer with
their inline picker; they never touch the running turn. So they now **open
regardless of turn state** (the old `MODEL_BUSY_NOTICE` / `LOGIN_BUSY_NOTICE`
rejections are gone). Two consequences:

- **The running turn is unaffected.** It streams on its own background thread
  into its own channel. Selecting a model rebuilds `main.rs`'s
  `Box<dyn ReplySource>`, which only changes the backend used for the *next*
  turn — the in-flight thread keeps going.
- **The status line is hidden while the picker is open.** The `/model` and
  `/login` pickers own the entire live region (`render_live` returns early for
  them), so the streaming strip — preview, spinner, timer — is not drawn while
  you browse. It reappears the moment the picker closes. This is the deliberate
  trade the user asked for ("it only modifies the inside of the textarea").

Because a mid-turn model switch or key save must not split the streaming reply
in scrollback, their confirmations are **toasts**, not committed notices — the
same reason `/copy` is.

## The pure state (`app.rs`)

Time is impure and stays out of the library, so `App` holds only *what* the
toast says, never *when* it dies:

```rust
pub enum ToastKind { Info, Error }
pub struct Toast { pub text: String, pub kind: ToastKind }
// App { … toast: Option<Toast> }
```

- `App::show_toast(text, kind)` — set (overwriting any current toast). It does
  **not** push to `history`: a toast is UI, not conversation.
- `App::clear_toast()` — drop it.
- `App::toast()` — the current toast, for the renderer.

`clear_conversation` (`/clear`) also drops the toast — a cleared slate shows
nothing lingering.

The slash-command dispatch returns `Action::Toast(String)` for the two mid-turn
rejections (`/resume`, `/help`); every other toast is raised by the boundary
directly (it already owns the clipboard / backend / `.env` I/O those confirm).

## The expiry (`main.rs`, the timestamp pattern)

Exactly like the status clock — the boundary keeps the impurity and the pure
core only ever sees the result. `main.rs` holds a `toast_deadline:
Option<Instant>`. Showing a toast (`present_toast`) sets `App::show_toast`, the
deadline to `now + TOAST_TTL` (**4 s**), and asks the frame scheduler for a draw
at the deadline:

```rust
app.show_toast(text, kind);
toast_deadline = Some(Instant::now() + TOAST_TTL);
frame.schedule_frame_in(TOAST_TTL);
```

The coalesced draw tick clears it when due, and **re-arms** while it is still
live — a later keystroke's frame can consume the pending deadline (the scheduler
keeps only the soonest), so without re-arming the clear could be dropped:

```rust
if let Some(dl) = toast_deadline {
    let now = Instant::now();
    if now >= dl { app.clear_toast(); toast_deadline = None; }
    else { frame.schedule_frame_in(dl - now); }   // keep a frame pending for the clear
}
```

This mirrors the `if app.turn_active() { schedule_frame_in(STATUS_FRAME_INTERVAL) }`
re-arm right beside it. No turn need be active for a toast to expire — the
scheduled frame fires on its own.

## The layout (`ui.rs`)

The toast is a single row (truncated with `…` at the width, like the footer)
pinned **at the bottom of the strip, directly above the box's top rule** — below
the status/gap and any queued messages when a turn streams, and directly above
the box when idle. It is reserved by `toast_rows(app)` (0 or 1) and threaded
through `live_height` / `live_layout` / `input_box` /
`render_live_with_preview` / `cursor_position` beside `queued_rows` — the same
"rows above the box" the strip already grows by. Because it lives *above* the
box, showing/clearing it grows/shrinks the region from the top exactly as the
streaming strip and the queue already do (`ui::repin` re-anchors, no box jump).

Styling is centralized in `ui.rs`: `TOAST_INDENT` (the two-space inset shared
with the footer/queue), `TOAST_COLOR` (dim, `TOOL_DIM_COLOR`) for info, and
`TOAST_ERROR_COLOR` (`ERROR_COLOR`) for failures.

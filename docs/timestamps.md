# Timestamps in the Ctrl+O transcript — Design

Date: 2026-06-09 (revised 2026-06-10: user-only, bottom-right, no seconds)

## Goal

Show a wall-clock timestamp for the **user's messages** — **only** inside the
Ctrl+O tool-output overlay (the full-screen conversation transcript). AI
replies, tool calls, and the committed `Done for Ns` turn summaries show **no**
stamp (each still *records* one). The inline conversation stays exactly as it
was: no timestamps there.

Format: **12-hour time with am/pm, no seconds, no date**, e.g. `03:20 AM`
(`chrono`'s `%I:%M %p`).

Placement: **bottom-right of the user message** — a blank row under the
message, then the dim stamp alone on its own line, flush to the right edge:

```
❯ thanks

                                                     03:20 AM

● let me check the file for you
  it wraps onto a second line here
● Read(src/main.rs)
  fn main() -> io::Result<()> {
  …
Done for 3s
```

## Why this shape

The library (`app`, `ui`) is pure and deterministically unit-tested — a
wall-clock can't live there. So:

1. **The clock is injected at the I/O boundary.** `App` holds an optional
   `clock: Option<fn() -> String>`. `main.rs` sets a real
   `chrono::Local`-based clock once at startup (`App::set_clock`); tests leave
   it `None` (→ empty timestamp) or set a fixed stub. The only impurity
   (`chrono`) lives in `main.rs`, the existing I/O boundary.
2. **Each item stores a pre-formatted `String`.** `Message` and `ToolCall`
   carry a `timestamp: String`, stamped at the moment they are recorded into
   history (`record_user_message`, `record_system_message`,
   `flush_streaming_segment`, `finish_stream`, `fail_stream`, `end_tool`) —
   i.e. completion time. `ui` stays dumb: it just renders the stored string.
3. **The stamp is added in `transcript_lines` only**, and only for
   `Role::User` messages (`ui::user_stamp_lines`: a blank row + the dim stamp
   right-aligned to the view width). The shared line-builders
   (`message_lines`, `tool_lines`, `tool_full_lines`, `summary_lines`) are
   untouched, so the inline view (`render_live`, `conversation_lines`) can
   never show a timestamp. With the stamp on its own line there is no reserved
   right column — every item wraps into the full width.

## Scope

Every recorded `HistoryItem` is still **stamped** (user, assistant, tool,
summary, and the system/error notices) — the data is uniform — but only the
user message's stamp is **displayed**. Timestamps appear **only** in the
Ctrl+O overlay; the inline conversation and the streaming preview are
unchanged.

## Testing

- `app`: with a fixed stub clock, `record_user_message` / `end_tool` /
  `finish_stream` stamp the item with the clock's value; with no clock the
  timestamp is empty (so existing equality tests are unaffected).
- `ui`: `transcript_lines` puts the user's stamp alone on its own
  right-aligned dim line below the message (after a blank row), omits the
  stamp line entirely for an empty stamp, shows **no** stamp on
  assistant/tool/summary items, and the inline builders
  (`conversation_lines`, `message_lines`) never contain it.
- `main.rs` (smoke): the overlay shows a right-aligned `hh:mm AM/PM` line and
  no dated/seconds stamp anywhere.

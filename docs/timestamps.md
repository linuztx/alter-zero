# Timestamps in the Ctrl+O transcript — Design

Date: 2026-06-09

## Goal

Show a wall-clock timestamp for every conversation item — user messages,
assistant replies, and tool calls — **only** inside the Ctrl+O tool-output
overlay (the full-screen conversation transcript). The inline conversation
stays exactly as it was: no timestamps there.

Format: **local date + time, 12-hour**, e.g. `2026-06-09 02:32:05 PM`
(`chrono`'s `%Y-%m-%d %I:%M:%S %p`). Constant width (22 cols), so right-aligned
stamps line up.

Placement: **right-aligned at the top-right of every item** — on the item's
first (header) line, flush to the right edge; continuation lines leave that
column blank.

```
❯ hi there                                  2026-06-09 02:32:05 PM
● let me check the file for you             2026-06-09 02:32:06 PM
  it wraps onto a second line here
● Read(src/main.rs)                         2026-06-09 02:32:07 PM
  fn main() -> io::Result<()> {
  …
```

## Why this shape

The library (`app`, `ui`) is pure and deterministically unit-tested — a
wall-clock can't live there. So:

1. **The clock is injected at the I/O boundary.** `App` holds an optional
   `clock: Option<fn() -> String>`. `main.rs` sets a real
   `chrono::Local`-based clock once at startup (`App::set_clock`); tests leave
   it `None` (→ empty timestamp) or set a fixed stub. The only impurity
   (`chrono`) lives in `main.rs`, the existing I/O boundary.
2. **Each item stores a pre-formatted `String`.** `Message` and `ToolCall` gain
   a `timestamp: String`, stamped at the moment they are recorded into history
   (`record_user_message`, `record_system_message`, `flush_streaming_segment`,
   `finish_stream`, `fail_stream`, `end_tool`) — i.e. completion time. `ui`
   stays dumb: it just renders the stored string.
3. **The stamp is added in `transcript_lines` only.** The shared line-builders
   (`message_lines`, `tool_lines`, `tool_full_lines`) are untouched, so the
   inline view (`render_live`, `conversation_lines`) can never show a timestamp.
   `transcript_lines` reserves a right-hand column the width of the timestamp
   (plus a small gap), wraps each item's content into the remaining width, then
   right-aligns the dim stamp onto the item's first line. The live tail
   (in-progress reply / running tool) has no timestamp yet, so it reserves the
   same column but leaves it blank — no layout jump when it finalises.

## Scope

Every recorded `HistoryItem` is stamped (user, assistant, tool, and also the
system/error notices), so the transcript reads uniformly. Timestamps appear
**only** in the Ctrl+O overlay; the inline conversation and the streaming
preview are unchanged.

## Testing

- `app`: with a fixed stub clock, `record_user_message` / `end_tool` /
  `finish_stream` stamp the item with the clock's value; with no clock the
  timestamp is empty (so existing equality tests are unaffected).
- `ui`: `transcript_lines` right-aligns the stamp on the item's first line
  (within the width, dim), and the inline builders (`conversation_lines`,
  `message_lines`) never contain it.

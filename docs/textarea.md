# Textarea — a codex-style editable input

The input box used to be **append-only**: `App.input` was a `String` that only
ever grew at the end (`push`) or shrank at the end (`pop`). The cursor was
always at the end; Left/Right/Up/Down/Home/End/Delete did nothing. You could not
go back and fix a character in the middle of what you had typed.

This document describes the replacement: a focused port of the editing core of
openai/codex's `bottom_pane/textarea.rs` — a real cursor with insert/delete
*anywhere* and codex's movement model — captured as a small `textarea` module
(`src/textarea.rs`). The scope is deliberately **"minimal cursor"**: the editing
model + navigation, and nothing else. Codex's vim mode, atomic `@mention`
elements, emacs kill-buffer/yank, password masking, search highlights and
configurable keymap system are **out of scope** (codex-specific; not ported).

## The model

```rust
pub struct TextArea {
    text: String,                              // the raw UTF-8 draft
    cursor: usize,                             // byte offset; always a grapheme boundary
    wrap_cache: RefCell<Option<WrapCache>>,    // cached wrapped rows, keyed on width
    preferred_col: Option<usize>,              // remembered column for vertical motion
}
struct WrapCache { width: u16, rows: Vec<Range<usize>> }
```

This mirrors codex's `TextArea` exactly (same four load-bearing fields), minus
the vim/element/kill-buffer/keymap state.

- **`text` + `cursor`** — the cursor is a byte offset into `text`, kept on a
  grapheme-cluster boundary (so CJK, emoji-ZWJ sequences and combining marks
  move as one). Editing inserts/deletes *at the cursor*, not at the end.
- **`wrap_cache`** — codex technique #5. Wrapping the input to visual rows is
  recomputed only when the width changes; every edit clears the cache
  (`wrap_cache.replace(None)`). It stores byte ranges per visual row so the
  cursor can be mapped to a (row, col) and back. The cache is filled lazily by
  the render path (`wrapped_rows(width)`), so vertical motion can read it without
  knowing the terminal width (see below).
- **`preferred_col`** — when you move up/down through lines of differing length,
  the column you *want* is remembered, so a short line in the middle does not
  permanently snap the cursor left. Any horizontal move or edit clears it.

## Wrapping: byte ranges, not strings

`ui::wrap_text` (used for finished messages) returns `Vec<String>` and collapses
runs of whitespace — fine for prose, wrong for an editor, and it loses the byte
offsets the cursor needs. The textarea instead wraps to **byte ranges**
(`wrap_rows(text, width) -> Vec<Range<usize>>`):

- Each range is the *displayed* slice of one visual row (`text[start..end]`),
  preserving the user's exact characters (spaces included).
- Greedy word-wrap, same shape as `ui::wrap_text`: explicit `'\n'`s are honoured
  (and a blank logical line is an empty range), a soft break consumes the run of
  spaces at the break point, and a word longer than the width is hard-broken on
  grapheme boundaries. Leading indentation and trailing space runs are *shown*
  (so the cursor after a trailing space is visible) and wrap exactly like words,
  so no row — and no cursor column — ever grows wider than the field.
- Consecutive rows may have a byte *gap* between them — the whitespace consumed
  at a soft break — so the ranges are display ranges, not a strict tiling.
- When the last row ends **exactly full** at the very end of the text, an empty
  trailing range (`len..len`) follows it — the row the end-of-text cursor sits
  on (and the box reserves), keeping the cursor inside the field instead of one
  column past it.

codex's own `wrap_ranges` is built on the `textwrap` crate (Cow/​pointer math and
a `+1` sentinel byte). We don't depend on `textwrap`, so this is a clean
re-derivation of the same idea on top of our existing greedy algorithm; it is
unit-tested from scratch rather than mirroring codex's sentinel arithmetic —
the empty trailing range above is the sentinel's *effect*, re-derived.

### Cursor ↔ (row, col)

- **cursor → (row, col)** (`cursor_row_col`): the row is the *last* wrapped row
  whose `start <= cursor`; the column is the display width of
  `text[row.start .. min(cursor, row.end)]`. Preferring the later row at a shared
  boundary matches codex's `partition_point` rule, so a cursor sitting exactly at
  a wrap point shows at the start of the next row.
- **(row, target_col) → cursor**: walk graphemes across the row accumulating
  display width until it exceeds `target_col`. Used by vertical motion.

## Movement

| Key | Method | Behaviour |
|-----|--------|-----------|
| ←/→ | `move_left`/`move_right` | one **grapheme**; clears `preferred_col` |
| ↑/↓ | `move_up`/`move_down` | across **wrapped visual rows**, keeping `preferred_col`; falls back to logical-line motion when the wrap cache is cold |
| Home/End | `move_home`/`move_end` | start/end of the current **logical** line (between `'\n'`s) |
| Backspace | `delete_backward` | delete the grapheme before the cursor |
| Delete | `delete_forward` | delete the grapheme at the cursor |
| char | `insert_char` | insert at the cursor |
| Ctrl+J / Alt+Enter / Shift+Enter | `insert_newline` | insert `'\n'` at the cursor (see `docs/shift-enter.md`) |

Vertical motion reads the wrap cache **without a width argument** — exactly like
codex's `move_cursor_up`/`down`. The cache is populated by the most recent render
(which knows the width); between an edit and the next render the cache is cold and
motion falls back to logical-line navigation. This is what keeps `App::on_key`
width-agnostic: the pure state machine never has to know the terminal size.

Up/Down are *visual* (wrapped) while Home/End are *logical* — this is codex's
behaviour, kept for fidelity.

## Integration

- **`App.input`** is a `TextArea` instead of a `String`. `on_key_conversation`
  routes editing/navigation keys to it; `Action::Submit` is `self.input.take()`
  (returns the text and resets the editor). The slash-command palette still keys
  off `command_query(self.input.text())` exactly as before — when the palette is
  open it intercepts ↑/↓/Tab/Enter/Esc, otherwise those drive the cursor.
- **`ui.rs`** renders from the textarea: `render_live`/`cursor_position` ask it
  for `display_rows(width)` and `cursor_row_col(width)`, and the box now scrolls
  to keep the **cursor** visible (codex's `effective_scroll`), not just the tail.
  `live_height` sizes the box from `input.row_count(width)`.

`main.rs` and `term.rs` are unchanged in shape: `live_height(&app.input, …)` and
`cursor_position(area, app)` keep their signatures (now reading through the
`TextArea`).

## What is intentionally *not* here (YAGNI / out of scope)

Vim mode, atomic `@mention` text elements, the emacs kill-buffer + `Ctrl+K`/​
`Ctrl+U`/`Ctrl+Y`, word-wise navigation/deletion (`Alt/Ctrl+←/→`, `Ctrl+W`),
password masking, render-only search highlights, and the runtime keymap-config
system. Each is a codex feature with no analogue in this app; adding any later is
additive and does not change the model above.

# Textarea — a codex-style editable input

The input box used to be **append-only**: `App.input` was a `String` that only
ever grew at the end (`push`) or shrank at the end (`pop`). The cursor was
always at the end; Left/Right/Up/Down/Home/End/Delete did nothing. You could not
go back and fix a character in the middle of what you had typed.

This document describes the replacement: a focused port of the editing core of
openai/codex's `bottom_pane/textarea.rs` — a real cursor with insert/delete
*anywhere* and codex's movement model — captured as a small `textarea` module
(`src/textarea.rs`), plus the **readline set** on top (word motion, the kill
keys, the Ctrl-letter cursor keys — see *Terminal shortcuts* below). Codex's
vim mode, atomic `@mention` elements, emacs kill-ring/yank, password masking,
search highlights and configurable keymap system stay **out of scope**
(codex-specific; not ported).

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
| ←/→ (Ctrl+B/Ctrl+F) | `move_left`/`move_right` | one **grapheme**; clears `preferred_col` (Ctrl+B only while nothing backgroundable runs — see below) |
| ↑/↓ (Ctrl+P/Ctrl+N) | `move_up`/`move_down` | across **wrapped visual rows**, keeping `preferred_col`; falls back to logical-line motion when the wrap cache is cold |
| Home/End (Ctrl+A/Ctrl+E) | `move_home`/`move_end` | start/end of the current **logical** line (between `'\n'`s) |
| Alt+B/Alt+F, Ctrl/Alt+←/→ | `move_word_left`/`move_word_right` | by **readline words** (runs of alphanumerics — `foo/bar.txt` is three stops); left lands at a word's start, right at its end (Emacs' `forward-word`) |
| Backspace (Ctrl+H) | `delete_backward` | delete the grapheme before the cursor |
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

## Terminal shortcuts (the readline set)

The composer answers the shell's editing keys. Motion pairs the table above
already shows: **Ctrl+A/Ctrl+E** (line start/end), **Ctrl+B/Ctrl+F** (one
grapheme), **Ctrl+P/Ctrl+N** (row up/down), **Alt+B/Alt+F** and **Ctrl/Alt+←/→**
(word-wise). The kill keys delete *spans*:

| Key | Span | Notes |
|-----|------|-------|
| Ctrl+W | `prev_unix_word_boundary()..cursor` | the shell's unix-word-rubout: back over whitespace, then the whole non-whitespace run — `src/main.rs` goes as one word |
| Alt+Backspace / Ctrl+Backspace | `prev_word_boundary()..cursor` | readline's backward-kill-word — stops at punctuation, so the same path is three kills |
| Alt+D / Alt+Delete / Ctrl+Delete | `cursor..next_word_boundary()` | forward word kill |
| Ctrl+U | `cursor_line_start()..cursor` | kill to the logical line's start |
| Ctrl+K | `cursor..cursor_kill_end()` | kill to the logical line's end — *at* the end it takes the `'\n'` itself (Emacs' join), so it is never a dead key |

Three deliberate wrinkles:

- **The word class is readline's, not UAX#29's.** Unicode word segmentation
  joins `bar.txt` into one word (its `.`-between-letters rule); terminal muscle
  memory expects the dot to stop an Alt+B / Alt+Backspace. A "word" here is a
  run of alphanumerics, walked by grapheme so emoji/CJK still move whole.
- **The textarea exposes kill *targets*, not kill methods** (`prev_word_boundary`,
  `prev_unix_word_boundary`, `next_word_boundary`, `cursor_line_start`,
  `cursor_kill_end`): the composer must widen a span over any pasted
  `[Pasted Content N chars]` / `[Image #N]` placeholder it intersects before
  deleting (`App::kill_span` — a kill stays as atomic as Backspace on a
  placeholder, the swallowed pairs dropped and a killed image's temp file
  handed to the boundary; `docs/paste.md`, `docs/image-paste.md`). Every kill
  then re-derives the palette/`@`/`$` bands and the shell mode exactly like
  Backspace (`App::kill_and_refresh`).
- **Session keys own their combos first.** Ctrl+B is *move to background* while
  a backgroundable command or foreground agent group runs
  (`App::can_move_to_background`, `docs/background.md`) and cursor-left the
  rest of the time; ↓ keeps its footer walk (the shell indicator, the agent
  roster) while Ctrl+N never walks — it is purely ↓'s editing half. Ctrl+P/N
  do drive an open palette/`@`/`$` band's selection, like the arrows.

The same set works in the permission prompt's Tab-amend field and the ask
modal's free-text entries (`App::edit_amend` — Ctrl+P/N are plain cursor keys
there, and Ctrl+B is always ←, since nothing backgroundable is actionable from
inside a modal). The Ctrl+R search line is its own key world and unchanged.

Two session-level keys moved to make room:

- **Shift+Tab** cycles the permission mode (was Ctrl+A) — Claude Code's own
  key for it, `docs/permissions.md`; inside a `write`/`edit` permission prompt
  it still *is* option 2, and the option's label advertises `(shift+tab)`.
- **Ctrl+T** cycles the thinking mode (was Shift+Tab), `docs/reasoning.md`.

(Ctrl+L stays deliberately unbound.)

## Integration

- **`App.input`** is a `TextArea` instead of a `String`. `on_key_conversation`
  routes editing/navigation keys to it; `Action::Submit` is `self.input.take()`
  (returns the text and resets the editor). The slash-command palette still keys
  off `command_query(self.input.text())` exactly as before — when the palette is
  open it intercepts ↑/↓/Tab/Enter/Esc, otherwise those drive the cursor.
- **`ui/live.rs` + `ui/layout.rs`** render from the textarea: `render_live`/`cursor_position` ask it
  for `display_rows(width)` and `cursor_row_col(width)`, and the box now scrolls
  to keep the **cursor** visible (codex's `effective_scroll`), not just the tail.
  `live_height` sizes the box from `input.row_count(width)`.

`main.rs` and `term.rs` are unchanged in shape: `live_height(&app.input, …)` and
`cursor_position(area, app)` keep their signatures (now reading through the
`TextArea`).

## What is intentionally *not* here (YAGNI / out of scope)

Vim mode, atomic `@mention` text elements, the emacs **kill-ring + `Ctrl+Y`
yank** (the kill keys delete; nothing is stashed for re-insertion), password
masking, render-only search highlights, and the runtime keymap-config system.
Each is a codex feature with no analogue in this app; adding any later is
additive and does not change the model above.

# Pasted-content placeholders

Pasting a large chunk of text into the composer used to dump the whole thing
into the input box — thousands of characters wrapping across dozens of rows,
burying the box and the cursor. This document describes the fix: a focused port
of openai/codex's large-paste handling. A paste over a threshold is replaced in
the composer by a compact placeholder —

```
[Pasted Content 3907 chars]
```

— while the real text is remembered off to the side and spliced back in when the
message is sent. Small pastes still insert inline, exactly as before.

> Note: the `paste` module has **two** unrelated jobs that both go by "paste".
> [`PasteBurst`](../src/paste.rs) is the *redraw-coalescing* burst detector (fast
> input → one deferred paint; see `docs/async-rewrite.md`). The placeholder logic
> here is separate: it keys off a real **bracketed-paste** event, not a detected
> burst.

## Where a paste comes from

The TUI now enables **bracketed paste** (`EnableBracketedPaste` in `term::init`,
`DisableBracketedPaste` in `restore` and the panic hook). A terminal in this mode
wraps a paste in `ESC[200~ … ESC[201~`, which crossterm's `EventStream` delivers
as a single `Event::Paste(String)` — distinct from the `Event::Key` stream that
ordinary typing (and `tmux send-keys -l`) produces. So:

- **A real paste** → one `Event::Paste` → the placeholder path below.
- **Fast typing / a key burst** → many `Event::Key`s, typed verbatim, coalesced
  into one repaint by `PasteBurst` (unchanged; `smoke.sh` Phase 6).

`main.rs` routes `Event::Paste(text)` to `App::on_paste(&text)` (conversation
view only — the Ctrl+O overlay has no composer, like typing there), then
schedules a frame and re-runs the `@`-token file-search dispatch (a small inline
paste can land inside an `@token`).

## The model

```rust
// App
pub pasted: Vec<(String, String)>,   // (placeholder, real text), insertion order
```

`App::on_paste`:

1. Normalise newlines (`\r\n` and lone `\r` → `\n`) — terminals such as iTerm2
   send CR on paste; the count and stored text use the normalised form.
2. `char_count = text.chars().count()` (chars, not bytes — codex parity).
3. If `char_count > LARGE_PASTE_CHAR_THRESHOLD` (**1000**): build the placeholder
   (`next_paste_placeholder`), `input.insert_str(&placeholder)`, and push
   `(placeholder, text)` onto `pasted`.
4. Otherwise insert the text inline (`input.insert_str(&text)`) — a small paste
   is indistinguishable from typing it.
5. Either way, re-derive the slash-command palette / shell-mode / `@`-picker
   state (the same trio every edit runs), so a small paste of `/`, `!`, or `@…`
   behaves like typing it.

### The placeholder string

`next_paste_placeholder(char_count, &pasted)` (pure, in `paste.rs`) mirrors
codex's `next_large_paste_placeholder`:

- base form `[Pasted Content {char_count} chars]`;
- if a placeholder of that exact base is already pending, disambiguate with a
  ` #2`, ` #3`, … suffix (the max existing suffix + 1) so two same-size pastes
  never collide.

### Expansion on send

The placeholder is a convenience for the *composer*; the model must receive the
real text. Every site where the draft leaves the composer routes through
`App::take_input()` instead of a bare `self.input.take()`:

```rust
fn take_input(&mut self) -> String {
    let text = self.input.take();
    let expanded = paste::expand_pastes(&text, &self.pasted);
    self.pasted.clear();            // the composer is now empty
    expanded
}
```

`expand_pastes(text, pastes)` (pure) finds each placeholder's position in the
*original* text, then rebuilds the string once, substituting each placeholder
with its real content (a placeholder the user deleted simply isn't found, so its
content is dropped). Searching the original — rather than `replace`-ing in place
— means real content that happens to contain another placeholder string can't be
re-expanded.

The sites: the normal **Enter** submit, the `!command` **shell** submit, and the
mid-turn **queue** (`queue_draft` / `queue_shell`) all take the *expanded* text,
so the model — and the scrollback/​history record of what you sent — gets the
full content (the user's chosen behaviour: the conversation shows the real text,
not the placeholder). Ctrl+C-clear also routes through expansion so a recalled
draft never carries a dangling placeholder. `pasted` is **not** cleared by
`/clear` (the input draft survives `/clear`, so its pastes do too).

### Atomic deletion

A placeholder deletes as **one unit**: a single Backspace (or Delete) removes the
whole `[Pasted Content N chars]`, not one character of it, and drops the matching
`pasted` entry. This lives in `App`, not the textarea — `Backspace`/`Delete` first
ask `paste::placeholder_to_delete(text, cursor, &pasted, backward)` for the byte
span of a placeholder the cursor is on; if there is one it's spliced out with
`input.replace_range(span, "")` and its entry dropped, otherwise the keypress
falls through to the normal per-grapheme `delete_backward`/`delete_forward`. The
cursor rules mirror codex's atomic boundary: Backspace fires at the placeholder's
end or anywhere inside it (a cursor at its *start* deletes the char before
instead); Delete fires at the start or inside. Placeholders are matched
longest-first (shared with `expand_pastes`), so a base placeholder can't shadow
its `… #N` extension.

## What is intentionally *not* here (scope)

Matching `docs/textarea.md`, the placeholder is **plain text** in the composer,
not one of codex's atomic `TextElement`s — the deletion above is a focused,
paste-only shortcut, not a general element model. So:

- The cursor is **not guarded** from stepping into a placeholder, and typing
  *inside* one isn't blocked (codex's full element model prevents both). A
  placeholder broken up that way simply won't match on deletion or expansion —
  its content is dropped. Atomic Backspace/Delete covers the common case (paste,
  then remove it) without that bookkeeping.
- The placeholder is **expanded on send**, so it never persists into history as
  a collapsed element; the transcript shows the real text. (Codex keeps a
  collapsed element; that needs history items to carry the hidden payload.)

Both are additive: either could be layered on later without changing the model
above.

## Tests

- `paste.rs` unit tests cover `next_paste_placeholder` (base + `#N`
  disambiguation), `expand_pastes` (substitution, multiple pastes, a deleted
  placeholder, no-op when empty), and `placeholder_to_delete` (the Backspace vs
  Delete cursor rules, including a base/`#2` prefix pair).
- `app.rs` unit tests cover `on_paste` (large → placeholder + recorded pair,
  small → inline, `\r\n` normalisation, insertion at the cursor), the round-trip
  (a large paste then Enter yields `Action::Submit(full_text)`), and atomic
  deletion (one Backspace/Delete clears the whole placeholder and drops its
  paste; a Backspace just before a placeholder still deletes only the preceding
  char).
- `smoke.sh` Phase 26 drives a real bracketed paste through tmux (`set-buffer` +
  `paste-buffer -p`), asserts the composer shows `[Pasted Content N chars]`
  rather than the raw text, then sends a single Backspace and asserts the
  placeholder is gone in one keystroke (the I/O boundary — `term`/`main` — has
  no unit tests).

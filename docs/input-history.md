# ↑/↓ input history — shell-style recall of submitted messages

> **Ctrl+P/Ctrl+N** are ↑/↓'s terminal twins (`docs/textarea.md`): same band
> navigation, same recall gate, same cursor fallback — except Ctrl+N never
> walks the footer (the shell indicator / agent roster stay ↓'s own).

Date: 2026-06-11

## Goal

Pressing **↑ in the composer recalls the last submitted message** (older with
further presses), ↓ steps back toward the newest and then clears — the way
openai/codex (and a shell) recall input — without breaking the arrows' existing
job of moving the cursor inside a multi-line draft.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

`bottom_pane/chat_composer_history.rs` (`ChatComposerHistory`) +
`chat_composer.rs`:

- **Entries** are the texts submitted this session, **newest at the end**;
  empty submissions are ignored and an entry **identical to the newest is
  collapsed** (`record_local_submission`). Recording always **exits browsing**.
- **The gate** (`should_handle_navigation(text, cursor)`): ↑/↓ browse history
  only when the composer is **empty**, or when the text **exactly equals the
  last recalled entry** (`last_history_text`) *and* the cursor sits at the
  text's very start or end. Anything else — a typed draft, an edited recall, a
  cursor moved inside — falls through to normal cursor movement, so recall
  never clobbers work in progress.
- **Navigation** (`navigate_up`/`navigate_down`): a cursor `Option<index>` is
  `Some` only while browsing. ↑ from idle starts at the newest; at the oldest
  it returns `None` and the key falls through to cursor movement. ↓ past the
  newest returns an **empty entry** — "clear the composer and stop browsing";
  ↓ when not browsing is plain cursor movement.
- **A recall replaces the whole draft and puts the cursor at the end**
  (`apply_history_entry` → `move_cursor_to_history_entry_end`), which is what
  keeps the gate satisfied for the *next* press.
- **Popups win**: with a popup open, ↑/↓ drive the popup, not history (their
  match arms come first).
- **Ctrl+C's composer-clear records the cleared draft** into this same history
  (`clear_for_ctrl_c`, pinned by `clear_for_ctrl_c_records_cleared_draft`), so
  ↑ brings an accidentally-cleared draft back.
- Codex also persists history across sessions (an async
  `LookupMessageHistoryEntry` fetch) and offers Ctrl+R incremental search. This
  doc ports the in-session `local_history` machine; the Ctrl+R search was ported
  later on top of it (`docs/history-search.md`), and **cross-session persistence
  was added later too** — `entries` is now seeded from an on-disk history file
  at startup and grows it on each submission, so both ↑/↓ recall and Ctrl+R span
  sessions (`docs/history-persistence.md`). Codex's dispatched slash commands are
  recorded too; we record only submitted messages and Ctrl+C-cleared drafts
  (YAGNI — a recalled `/token` would mostly re-open the palette). The
  Ctrl+C-cleared draft recalls this session but is **not** persisted (codex keeps
  cleared drafts in `local_history` only — see `docs/history-persistence.md`).

## The mapping onto this codebase

### Pure state (`app::InputHistory`, a field on `App`)

A direct port of the synchronous core, in `app/input_history.rs` beside the other state
types:

```rust
pub struct InputHistory {
    entries: Vec<String>,        // submitted texts, newest at the END
    cursor: Option<usize>,       // Some(index) only while browsing
    last_recall: Option<String>, // what navigation last wrote into the composer
}
```

`record(text)` (exit browsing; ignore blank; collapse adjacent duplicate),
`should_navigate(text, cursor)` (the gate above), `up() -> Option<String>`,
`down() -> Option<String>` (`Some("")` past the newest = clear). All pure,
unit-tested through `App::on_key`.

`App.input_history` **survives `/clear`** — `/clear` wipes the conversation
(`App.history`), not the composer's recall, matching codex's net UX (its
history even spans sessions).

### Key handling (`App::on_key_conversation`)

The existing ↑/↓ arms gain a recall attempt *before* the cursor fallback; the
palette's menu-open arms stay above both (codex's "popups win"):

```rust
KeyCode::Up => {
    if self.should_browse_history()
        && let Some(text) = self.input_history.up()
    {
        self.recall_input(&text);
        return Action::None;
    }
    self.input.move_up();
    Action::None
}
```

(↓ symmetric with `down()`.) `recall_input` is `set_text` — which already puts
the cursor at the end, codex's recall placement — plus `refresh_command_menu`,
so recalling a bare `/token` reopens the palette exactly like typing one (and
the *next* ↑ is then palette navigation until it's dismissed — same dynamic as
codex's slash popup).

### Recording sites

- **Enter (submit)**: `Action::Submit`'s arm records the taken text before
  returning it.
- **Ctrl+C composer-clear**: records the cleared draft (codex's
  `clear_for_ctrl_c`), so ↑ undoes the clear. This closes the gap noted in
  `docs/design.md` when the clear step was first ported.

Nothing changes in `main.rs`, `ui/`, or the protocol: recall is pure `App`
state and the box already renders whatever the textarea holds.

## Testing

- `app`: ↑ recalls the newest submission (cursor at the end) and steps to older
  ones, clamping at the oldest; ↓ steps newer and **clears past the newest**;
  ↓ when not browsing does nothing; a typed draft is never clobbered (↑ falls
  through to cursor movement); editing a recall returns the arrows to cursor
  movement (and Home/End edges re-enable recall); submitting resets browsing to
  the newest; adjacent duplicate submissions collapse; blank texts are never
  recorded; Ctrl+C's cleared draft is recallable; recalling a `/token` reopens
  the palette; `/clear` keeps the recall history; with no history the arrows
  behave exactly as before (existing cursor-movement tests stay green).
- `src/tui/` (smoke, Phase 10): after a finished turn, ↑ shows the sent message
  in the input box again, ↓ clears it, and ↑ + Enter resubmits it — a second
  committed copy and a second turn summary appear.

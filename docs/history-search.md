# Ctrl+R history search — reverse incremental search over submitted inputs

Date: 2026-06-12

## Goal

Pressing **Ctrl+R in the composer opens a shell-style reverse incremental
search** over the ↑/↓ input history (`docs/input-history.md`): type to filter,
the newest matching entry previews in the composer, Ctrl+R again steps to older
matches, Enter accepts the preview as an editable draft, Esc restores exactly
what was there before. A port of openai/codex's Ctrl+R search — the piece
`docs/input-history.md` explicitly left out of the ↑/↓ port.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

`bottom_pane/chat_composer/history_search.rs` (the session + UI lifecycle) and
`bottom_pane/chat_composer_history.rs::search` (the traversal):

- **A session** (`HistorySearchSession`) holds the **original draft snapshot**,
  the footer-owned **query**, and a **status** ∈ Idle / Searching / Match /
  NoMatch. Ctrl+R opens it with an empty query and *no preview* — "opening
  Ctrl+R never previews the latest entry by itself"; it also closes any open
  popup and flushes a pending paste burst before snapshotting.
- **While search is active every key is consumed** before normal composer
  handling (`handle_history_search_key`): plain chars **append to the query**
  and restart the search from the newest entry; Backspace/Ctrl+H pops a char;
  Ctrl+U clears the query; **Ctrl+R / ↑ step older**, **Ctrl+S / ↓ step
  newer**; **Enter accepts only on Match** (the session closes, the previewed
  text stays as an ordinary editable draft, cursor at the end); **Esc and
  Ctrl+C cancel**, restoring the snapshotted draft *including its cursor*;
  a **bracketed paste appends to the query too** (readline's paste-into-isearch
  — `App::on_paste` guards on the open search): it never edits the doomed
  preview underneath, never flips shell mode or opens the `@` picker, and
  control characters flatten to spaces so the single-row query line survives;
  anything else is swallowed.
- **Matching** is a **case-insensitive substring** test (`to_lowercase()
  .contains`), traversed **newest → oldest**, with **duplicate texts skipped**
  per session (`seen_texts`). Stepping past either end returns `AtBoundary`,
  which *keeps* the current match — never a "no match" flicker at the end of
  the list. A query with no match shows **NoMatch** and restores the original
  draft to the composer, but the search stays open for more typing.
- **The footer line** (its hint row) renders the search:
  `reverse-i-search: ` dim + the query **cyan**; on Match it appends
  `enter accept · esc cancel` (keys cyan **bold**, labels dim); on NoMatch it
  appends `  no match` in **red**. The **hardware cursor sits at the end of
  the query** in that footer line (`history_search_cursor_pos`), not in the
  textarea — the shell reverse-i-search feel.
- **The previewed match highlights** every case-insensitive query occurrence
  in the textarea with `REVERSED | BOLD` (`history_search_highlight_ranges` →
  `case_insensitive_match_ranges`, a char-fold mapping so e.g. `İ` folds
  correctly) until the match is accepted.
- Accepting seats codex's shared history cursor at the match
  (`search_match` sets `history_cursor`/`last_history_text`), so **↑ right
  after accepting steps to the entry older than the accepted one**.
- Codex's search also spans its **persistent cross-session history** with
  async entry fetches (the Searching status / `Pending` result). We have no
  persistent history, so that whole pending machine — and the Searching
  status — drops out; everything is synchronous over
  `InputHistory::entries`.

## The mapping onto this codebase

### Pure state (`app::HistorySearch`, a field on `App`)

```rust
pub struct HistorySearch {
    snapshot: TextArea,     // the draft (text + cursor) from before search
    pub query: String,      // the footer-owned query
    pub state: SearchState, // Idle | Match { selected } | NoMatch
}
```

`TextArea` derives `Clone` so the snapshot/restore is codex's `ComposerDraft`
snapshot — cancel restores the text *and* the exact cursor. `Match.selected`
indexes the derived newest-first unique match list; deriving per keystroke
(like the palette's `matching_commands`) replaces codex's cached
`unique_matches` — entries cannot change while search is open (search consumes
every key, so nothing can submit/queue/record).

`InputHistory` grows three methods: `search(query) -> Vec<usize>` (indices of
entries containing the query case-insensitively, newest first, newest
occurrence of each duplicate text only), `entry(index)`, and
`resume_at(index)` — accepting seats ↑/↓ browsing at the accepted entry, so ↑
continues older from it (codex's shared cursor).

### Key handling

`App::on_key` routes **every** key to `on_key_search` while
`history_search.is_some()` (Conversation view) — *before* the global
Ctrl+C/Ctrl+O arms, because search redefines both: **Ctrl+C cancels the
search** (restores the draft; it neither clears it nor quits — codex), and
**Ctrl+O cancels then opens the overlay** (the global toggle keeps working;
search state never leaks into the overlay). **Esc cancels the search even
mid-turn** — the palette-dismiss precedent; it never reaches the interrupt.
Enter on Idle/NoMatch is swallowed (search stays open), so a search can never
submit or queue a message. The preview writes the composer via `set_text`
without re-deriving the palette (search owns the keys); **accepting re-derives
it** like ↑-recall, so accepting a bare `/token` reopens the palette.

Ctrl+R itself is a new arm in `on_key_conversation`: it closes the palette,
snapshots the draft, and opens the session (an open `?` shortcuts band already
closes first — the existing any-key rule). With the search open, Ctrl+R steps
older instead.

### Rendering (`ui.rs`)

The search line takes the **session footer's slot** — codex renders it in its
footer hint line: `footer_rows` returns 1 whenever a search is active (even
with no session info injected — unlike the `{model} · {cwd}` line it must
always show), `render_live` paints `search_line` there instead of
`footer_line`, and `cursor_position` moves the hardware cursor to the end of
the query in that row (clamped inside the width, codex's
`history_search_cursor_pos`). No band can be open during a search (opening
closes the palette; the band keys never reach the band), so the slot is free
by construction. Geometry (`live_height`/`live_layout`) is untouched: the
search row *is* the footer row.

Match highlighting: `App::search_highlight_ranges` ports codex's
`case_insensitive_match_ranges` (char-fold mapping, so multi-char lowercase
folds keep byte ranges aligned); `render_live` splits the input box's rows
into spans at those ranges — the textarea's `wrapped_rows` byte ranges line up
with `display_rows` — and styles the overlap `REVERSED | BOLD`.

The `?` shortcuts band gains a `ctrl+r to search history` entry.

`main.rs` does not change: the footer slot, cursor, and live height all flow
through the existing pure helpers.

## Testing

- `app`: Ctrl+R opens idle with no preview; typing builds the query and
  previews the newest match (case-insensitively); Ctrl+R/↑ step older and
  clamp at the oldest (the match stays — no flicker); Ctrl+S/↓ step newer and
  clamp at the newest; duplicate texts match once; Backspace pops the query
  and restarts (a no-match query recovers); Ctrl+U clears to Idle and restores
  the draft; a no-match query restores the draft but keeps the search open;
  Enter accepts only on Match (search closes, the text stays, nothing
  submits/queues) and seats ↑ browsing at the accepted entry; Enter on
  Idle/NoMatch is swallowed; Esc and Ctrl+C cancel and restore the draft text
  *and cursor*; Esc mid-turn cancels the search, not the turn; Ctrl+O cancels
  and opens the overlay; opening closes the palette and typing never reopens
  it; accepting a bare `/token` reopens the palette; with no history typing
  shows NoMatch; the highlight ranges are case-insensitive and only exposed
  while a match is previewed.
- `ui`: the footer slot shows `reverse-i-search: {query}` while searching
  (displacing the session footer, present even with no session info); Match
  appends the accept/cancel hints and NoMatch appends `no match`; the cursor
  sits at the end of the query in the footer row; the previewed match's query
  occurrences render `REVERSED` in the input box; the shortcuts band lists
  `ctrl+r`.
- `main.rs` (smoke, Phase 18): drive the real binary — Ctrl+R shows the search
  line, typing a query previews the matching history entry in the composer,
  Ctrl+R steps to the older match, Enter accepts it (search line gone, footer
  back, text editable), a garbage query shows `no match` with the draft
  restored, and Esc closes the search without quitting the app.

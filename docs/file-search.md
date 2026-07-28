# `@` file-path mentions — a file picker below the box

Date: 2026-06-18

## Goal

Typing **`@`** in the composer opens a **file picker** below the input box —
the way openai/codex's `@` mention popup works, presented Claude-Code-style:
type `@`, fuzzy-filter the workspace files as you type, ↑/↓ to choose,
**Tab/Enter inserts the path** into the message (replacing the `@token`), Esc
dismisses. The path is inserted as plain text — it just becomes part of the
message you send (there is no special on-the-wire encoding; codex leaves raw
file paths literal too).

## What codex does (from `/tmp/codex/codex-rs`)

- **Engine** (`file-search/src/lib.rs`): a background, `.gitignore`-respecting
  walk feeding **nucleo** fuzzy matching; results are `FileMatch { score, path,
  indices, … }` where `indices` are the matched character positions (for
  highlighting), capped (default 20), ranked by score then path.
- **Wiring** (`tui/src/file_search.rs`, `chatwidget.rs`): the composer sends
  `AppEvent::StartFileSearch(query)` whenever the `@`-token changes; the manager
  runs the search off-thread and sends `AppEvent::FileSearchResult { query,
  matches }` back. Stale results are dropped (`current_token.starts_with(query)`);
  a "waiting" flag shows *loading…* until results arrive.
- **Trigger** (`chat_composer.rs::current_prefixed_token_range`): scan back from
  the cursor over non-whitespace to the token start; it's an `@`-token only if
  that char is `@` **and** it sits at start-of-line or after whitespace (so
  `email@host` never triggers). The query is the token after `@`.
- **State machine** (`bottom_pane/chat_composer/popup_state.rs`): an
  `ActivePopup` enum (None/Command/File/…); the file popup is one band, mutually
  exclusive with the slash-command popup. ↑/↓ move, Tab/Enter accept, Esc
  dismisses (and remembers the dismissed token so it doesn't reopen).
- **Insertion** (`insert_selected_path`): replace the whole `@token` range with
  the path **+ a trailing space**; paths containing whitespace are quoted.

## The mapping onto this codebase

This is an **inline** TUI whose logic is pure and unit-tested, with all I/O at
the `main.rs`/`term.rs` boundary. The `@` picker is a **third live-region band**
below the box, exactly parallel to the slash-command palette
(`docs/shortcuts.md`, the palette in `app::COMMANDS`) — but its list comes from
the **filesystem**, fetched **asynchronously** the way codex does it (the agreed
design: full async request/response, not a startup index).

### Async pipeline (chosen design)

Mirrors codex's `StartFileSearch`/`FileSearchResult` round-trip, adapted to this
loop's existing `select!` + boundary-injection idioms:

```
key ─► App edits draft ─► (loop) app.file_search_query() changed?
                                   └─ yes ─► req_tx.send(query)         [loop → worker]
worker thread: recv(query) ─► coalesce (drain to newest) ─► walk once+cache
              ─► file_search::rank_files(query, &files, LIMIT)
              ─► res_tx.send(FileSearchResult{query, matches})         [worker → loop]
loop select!: file_rx.recv() ─► app.set_file_matches(query, matches)   [stale-dropped]
```

- **Worker** (`main.rs::spawn_file_search_worker`): a dedicated thread (not the
  reply backend — invariant 1 keeps the one stdin reader untouched). It owns a
  `std::sync::mpsc::Receiver<String>` of queries and a tokio
  `UnboundedSender<FileSearchResult>` back to the loop (its `send` is sync,
  callable from any thread — same as the reply backend). On each request it
  **coalesces** (drains queued queries to the newest — the debounce), walks the
  cwd **once and caches** the file list, then re-ranks per query.
- **Walk** (`main.rs::walk_files`): dependency-free (the agreed choice). An
  iterative walk of the cwd skipping **hidden** entries (dotfiles, so `.git`
  too) and a small **denylist** (`target`, `node_modules`), capped at
  `FILE_INDEX_CAP` to bound memory/time. Directories are listed with a trailing
  `/`. (Known divergence from codex: no `.gitignore` parsing, and the list is
  captured on first use per worker lifetime.)
- **Dispatch** (`main.rs::dispatch_file_search`): after each key, compare
  `app.file_search_query()` to the last dispatched query; on change, send the
  new query (or nothing when the picker closed). Boundary state (`last_file_query`),
  like `committed`/`clocks`.
- **Staleness**: `App::set_file_matches(query, matches)` applies the results
  only if they're for the *currently active* token query, else drops them.

### Pure core

- `file_search.rs` (new module, mirrors codex's `file_search.rs`):
  - `at_token(text, cursor) -> Option<AtToken>` — the trigger rule above.
    `AtToken { range, query }`: `range` is the byte range of the whole `@token`
    (to replace on accept), `query` the text after `@`.
  - `FileMatch { path: String, score: i32, indices: Vec<usize> }` — `indices`
    are byte offsets of matched chars (for popup highlighting).
  - `fuzzy_match(query, candidate) -> Option<(i32, Vec<usize>)>` —
    ASCII-case-insensitive **subsequence** match (the fold is
    `eq_ignore_ascii_case`, so non-ASCII characters must match exactly); scores
    boundary/contiguous/basename hits higher. Empty query matches everything
    (score 0).
  - `rank_files(query, &[String], limit) -> Vec<FileMatch>` — filter+sort
    (score desc, then shorter path, then lexicographic), capped at `limit`.
- `app/file_picker.rs`:
  - `FileSearch { selected, query, matches: Vec<FileMatch>, waiting }` on
    `App::file_search: Option<…>` (`None` = closed), parallel to `command_menu`.
  - `refresh_file_search(had_token)` — open on the None→Some token transition,
    clamp the selection as the filter narrows, close when the token's gone;
    Esc-dismiss is **sticky** within a token (the palette's `had_query` trick),
    suppressed in shell mode.
  - `file_search_query()` (the active token query, for the loop to dispatch),
    `move_file_selection`, `highlighted_file`, `accept_file_selection`
    (replace the `@token` range via `TextArea::replace_range`, + trailing space,
    quoting paths with whitespace), `set_file_matches` (staleness).
- `textarea.rs`: `replace_range(range, with)` — splice + place the cursor after
  the inserted text (new pure method; the existing `insert_str` only inserts at
  the cursor).

### Key dispatch (`App::on_key_conversation`)

With the picker open (`file_search.is_some()`), parallel to the palette arms:
Esc dismisses, ↑/↓ move the selection, **Tab/Enter accept** the highlighted file
(when there is one — otherwise Enter falls through and submits, Tab queues).
Typing / Backspace / Delete edit the draft and then call `refresh_file_search`
(alongside `refresh_command_menu`/`sync_shell_mode`). The picker and the palette
are mutually exclusive by construction (a bare `/token` has no whitespace, so any
`@` in it isn't at a boundary).

### Rendering (`ui/menu.rs`)

A third band sharing the palette's slot below the box:
`band = menu_rows + shortcuts_rows + file_menu_rows` (at most one is non-zero).

- `file_menu_rows(app)` — 0 closed; one placeholder row for *Searching…* /
  *No matching files*; else `matches.len().min(FILE_MENU_MAX_ROWS)`.
- `file_menu_lines(app, width)` — one row per match, windowed (`menu_window`)
  to keep the selection visible; the **selected** row lights up cyan (the
  palette's convention) and **matched characters** are emphasized (from
  `FileMatch.indices`).
- `render_live`/`cursor_position`/`live_height` (and `main.rs::live_region_height`)
  add `file_menu_rows` to the band so the box, cursor, and footer stay put when
  the picker opens — exactly like the palette/shortcuts.

## Testing

- `file_search` (unit): `at_token` triggers after whitespace/start, not inside
  `email@host`, finds the token around the cursor, empty query for a lone `@`;
  `fuzzy_match` is an ASCII-case-insensitive subsequence with correct indices and
  None on no match; `rank_files` ranks basename/contiguous hits first, sorts
  ties by length then name, caps at the limit, and lists everything for an
  empty query.
- `textarea` (unit): `replace_range` splices and seats the cursor after.
- `app` (unit): `@` opens the picker and `file_search_query` tracks the token;
  ↑/↓ move and clamp; Enter/Tab accept (the `@token` becomes `path ` with the
  cursor after); Esc dismisses and is sticky; `set_file_matches` drops stale
  results; the picker stays closed in shell mode and is suppressed in the
  tool view; submit/clear/recall close it.
- `ui` (unit): `file_menu_rows` is 0/1/n; `file_menu_lines` lists matches with
  the selected row highlighted and matched chars emphasized; `live_height`
  grows by the band and `cursor_position` stays put when it opens.
- `main.rs` (smoke, Phase 25): launch in a temp dir with known files, type
  `@alpha`, the picker lists the file, Enter inserts its path into the composer.

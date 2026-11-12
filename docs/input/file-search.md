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
(`docs/ui/shortcuts.md`, the palette in `app::COMMANDS`) — but its list comes from
the **filesystem**, fetched **asynchronously** the way codex does it (the agreed
design: full async request/response, not a startup index).

### Async pipeline (chosen design)

Mirrors codex's `StartFileSearch`/`FileSearchResult` round-trip, adapted to this
loop's existing `select!` + boundary-injection idioms:

```
key ─► App edits draft ─► (loop) app.file_search_query() changed?
                                   └─ yes ─► req_tx.send(query)         [loop → worker]
worker thread: recv(query) ─► coalesce (drain to newest) ─► walk afresh
              ─► file_search::rank_files(query, walk.paths)   (every match, ranked)
              ─► res_tx.send(FileSearchResult{query, matches, truncated})  [worker → loop]
loop select!: file_rx.recv() ─► app.set_file_matches(query, matches, truncated)  [stale-dropped]
```

- **Worker** (`tui::workers::spawn_file_search_worker`): a dedicated thread (not the
  reply backend — invariant 1 keeps the one stdin reader untouched). It owns a
  `std::sync::mpsc::Receiver<String>` of queries and a tokio
  `UnboundedSender<FileSearchResult>` back to the loop (its `send` is sync,
  callable from any thread — same as the reply backend). On each request it
  **coalesces** (drains queued queries to the newest — the debounce), then walks
  the cwd **afresh** and ranks the result. The walk is deliberately per-query,
  *not* cached at startup: a startup index went stale the moment the agent
  created a file, so `@` in a fresh dir only ever listed the boot-time contents
  (the "new files never appear" bug). Codex gets the same freshness by starting
  a new walk per `@`-token session; per-query is simpler here, and the
  `FILE_INDEX_CAP` bound plus the coalescing keep the repeated walks cheap and
  off the loop.
- **Walk** (`tui::workers::walk_files`): dependency-free (the agreed choice). An
  iterative breadth-first walk of the cwd skipping **hidden** entries
  (dotfiles, so `.git` too) and a small **denylist** (`target`,
  `node_modules`), capped at `FILE_INDEX_CAP` to bound memory/time.
  Directories are listed with a trailing `/`. It returns a
  `Walk { paths, truncated }`, `truncated` only when an entry it would have
  listed was left out (see *A capped walk says so* below). Each path is
  **built once**, its folder's path followed by its name — the folders
  still to read being positions in the list, not paths of their own — and
  each folder's entries are gathered, never past what the cap has room for,
  then **sorted into listing order** (`file_search::sibling_order`) before
  any is listed, so the walk comes out already in the order a bare `@`
  shows (see *What a search costs* below). Joining by hand keeps a name's
  own `\`, an ordinary character on Linux and macOS, which a
  backslash-to-slash rewrite once turned into a folder that did not exist.
  (Known divergence from codex: no `.gitignore` parsing.)
- **Dispatch** (`tui::workers::dispatch_file_search`): after each key, compare
  `app.file_search_query()` to the last dispatched query; on change, send the
  new query (or nothing when the picker closed). Boundary state (`last_file_query`),
  like `committed`/`clocks`.
- **Staleness**: `App::set_file_matches(query, matches, truncated)` applies
  the results only if they're for the *currently active* token query, else
  drops them.

### Pure core

- `file_search.rs` (new module, mirrors codex's `file_search.rs`):
  - `at_token(text, cursor) -> Option<AtToken>` — the trigger rule above.
    `AtToken { range, query }`: `range` is the byte range of the whole `@token`
    (to replace on accept), `query` the text after `@`.
  - `FileMatch { path: String, score: i32 }` — which characters the query
    matched is not kept: the band works that out with `fuzzy_match` for the
    few rows it draws (a list holds every match the walk found, and almost
    none is ever on screen). Its accessors feed the columned rows: `is_dir()`
    (the walk lists directories with a trailing `/`, so the kind rides the
    path), `name()` / `name_start()` (the final component and the byte where
    it starts — the offset that remaps the matched characters onto the name
    column), and `parent()` (the leading directories through the last
    interior `/`, empty for a root-level entry).
  - `fuzzy_match(query, candidate) -> Option<(i32, Vec<usize>)>` —
    ASCII-case-insensitive **subsequence** match (the fold is
    `eq_ignore_ascii_case`, so non-ASCII characters must match exactly); scores
    boundary/contiguous/basename hits higher (a directory's basename is the
    component *before* its trailing `/`, or every dir would lose the bonus).
    Empty query matches everything (score 0). The score comes from
    `fuzzy_score`, one core that hands each matched offset to a callback:
    `fuzzy_match` collects them, the ranking collects nothing.
  - `sibling_order(a_name, a_is_dir, b_name, b_is_dir)` — two entries of one
    folder: folders first, then the names case-insensitively, the spelling
    breaking a tie. The listing order ends on it and the walk sorts each
    folder with it, so the two cannot drift.
  - `rank_files(query, Vec<String>) -> Vec<FileMatch>` — filter+sort
    (score desc, then shorter path, then lexicographic; an empty query in
    the directory-listing order of *A bare `@` reads like a directory
    listing* below), **every match kept** (see *Every match, five at a time*
    below). It takes the walk's
    list by value and moves each matched path into its `FileMatch`, so
    ranking the whole walk never copies it, and scoring a path allocates
    nothing.
- `app/file_picker.rs`:
  - `FileSearch { selected, query, matches: Vec<FileMatch>, waiting,
    truncated }` on `App::file_search: Option<…>` (`None` = closed), parallel
    to `command_menu`; `matches` holds every match the last search returned,
    which is what the band scrolls through and counts, and `truncated` says
    the walk behind them stopped at its cap.
  - `refresh_file_search(had_token)` — open on the None→Some token transition,
    clamp the selection as the filter narrows, close when the token's gone;
    Esc-dismiss is **sticky** within a token (the palette's `had_query` trick),
    suppressed in shell mode.
  - `file_search_query()` (the active token query, for the loop to dispatch),
    `move_file_selection`, `highlighted_file`, `accept_file_selection`
    (replace the `@token` range via `TextArea::replace_range`, + trailing space,
    quoting paths with whitespace), `set_file_matches` (staleness, and the
    walk's `truncated` beside the matches).
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
`band = menu_rows + shortcuts_rows + file_menu_rows + skill_menu_rows` (at
most one is non-zero).

- `file_menu_rows(app, width)` — **the built line count** of
  `file_menu_lines` (the palette's `menu_rows` rule, so the reserved rows and
  the painted ones agree at every width, the counter coming and going with
  it): 0 closed; one placeholder row for *Searching…* / *No matching files*
  (inset by `MENU_INDENT`, so it says so in the names' column rather than the
  marker's gutter); else the matches capped at `FILE_MENU_MAX_ROWS` (**5**;
  longer lists scroll) and the counter under them.
- `file_menu_lines(app, width)` — one **columned** row per match, windowed
  (`centered_window`, the `/` palette's window) to keep the selection on the
  middle row, over the palette's dim `(n/total)` counter:

  ```text
    public      ./                                                        Dir
    assets      public/                                                   Dir
  → cv.pdf      public/assets/                                            File
    next.svg    public/                                                   File
    vercel.svg  public/                                                   File
    (3/12)
  ```

  The selected row carries the `→` marker (`MENU_MARKER`, which the slash
  palette and the `$` picker wear too; the rest open on `MENU_INDENT`, its
  width in spaces) and lights up cyan (the palette's convention). The **name**
  column is the widest visible name + `FILE_MENU_GAP`; the **parent** column
  shows the entry's directory (`FILE_MENU_ROOT_DIR` `./` for root-level
  entries, deeper parents truncated so the row never overflows); the
  **kind** column pins `File`/`Dir` at the right edge
  (`width − FILE_MENU_TYPE_WIDTH`). **Matched characters** stay emphasized
  (their offsets worked out by `fuzzy_match` for each row drawn, remapped
  across the name/parent split — `file_menu_highlight`). A width too
  narrow for the columns degrades to
  marker + name alone. Under the rows hangs the palette's dim `(n/total)`
  counter (`menu_counter_line`), in the names' column: which match the
  highlight is on, of how many the query matched — `+` closing the total
  (`FILE_MENU_PARTIAL_MARK`) when the walk stopped at its cap. A terminal too
  narrow to hold the whole counter goes without it rather than cutting a
  number in half, and a placeholder has nothing to count.
- `render_live`/`cursor_position`/`live_height` (and `tui::view::Session::live_region_height`)
  add `file_menu_rows` to the band so the box, cursor, and footer stay put when
  the picker opens — exactly like the palette/shortcuts.
- The `?` shortcuts band names the sigil, `@ for file paths`, beside
  `/`, `!` and `$` (`docs/ui/shortcuts.md`).

## Every match, five at a time

Date: 2026-11-11

The picker used to show the **best eight** matches and nothing past them: the
worker ranked every entry the walk found, then dropped all but eight
(`FILE_MENU_LIMIT`), so ↑/↓ wrapped inside those eight. In this repository a
bare `@` reached eight of the 890 entries the walk lists. It now keeps them all,
and shows them the way the `/` palette and the `$` picker show theirs:

- **The worker sends back every match.** `rank_files(query, files)` ranks the
  whole walk and caps nothing. The walk's own bounds are unchanged, and they
  are what keep "every match" finite: hidden entries and the
  `target`/`node_modules` denylist are skipped, and `FILE_INDEX_CAP` (10,000
  entries) stops the breadth-first walk, so a launch in `~` or `/` lists the
  shallowest ten thousand entries rather than walking the disk on every
  keystroke.
- **Five rows at a time** (`FILE_MENU_MAX_ROWS`), the rest a scroll away,
  over the `(n/total)` counter: `(1/12)` on a query twelve files match, where
  a counter over the old list could never have read past `/8`.
- **The highlight stays centered** (`centered_window`): ↓ walks the `→` down
  to the middle row, then the list scrolls under it, so the files above *and*
  below it stay in view, sliding to an edge only where the list runs out (the
  first rows anchored at the top, the last flush with the tail — ↑ from the
  first match wraps to the last on the bottom row). The edge-pinned
  `menu_window` it replaced scrolled only once the highlight reached the
  bottom row, then held it there, hiding everything past it.

What it costs: `rank_files` already scored and sorted every walked entry
before it truncated, so a search does no more work than it did, and taking
the walk's list by value means the matched paths are moved, never copied.
What changes is that the matches outlive the search: `App::file_search` holds
them while the band is open — at most `FILE_INDEX_CAP` of them, about a
megabyte of data at the cap and a few dozen kilobytes in a typical project —
and drops them when it closes. That megabyte was already resident, though:
the old ranking built the same list before cutting it to eight, and glibc
keeps a freed list's pages. Measured in `/usr`, where the walk stops at its
cap, with a bare `@` open over 10,000 matches (two runs of each build): 16.3
and 16.4 MiB resident after the change, 16.4 and 16.6 MiB before it. Ten more
searches after that grew both builds alike.

Considered and not done: walking *everything* — hidden entries included, no
cap. Without `.gitignore` support a hidden directory is as likely to be
`.venv/` or `.next/` (tens of thousands of cache and build files) as
`.github/`, and they would fill the cap and bury the project's own files;
uncapped, a launch in `~` would walk the whole home directory on every
keystroke. Finding dotfiles (`.github/workflows/ci.yml`, `.env.example`)
properly needs the walk to honour `.gitignore`, which is a change of its own.

## A bare `@` reads like a directory listing

Date: 2026-11-11

Typing just `@` asks for the whole workspace, and an empty query has no score
to rank by. It sorted shortest path first, which, cut to eight, roughly meant
"the top level" — but over every match it interleaves the levels (`a/b.rs`
ahead of `README.md`, a nested file between two top-level ones) and reads as
noise. It now lists the workspace the way a directory listing reads, a level
at a time (`file_search::listing_order`):

```text
→ docs         ./      Dir      the cwd's own entries first: directories,
  src          ./      Dir      then files, names compared case-insensitively
  Cargo.toml   ./      File
  README.md    ./      File
  guide.md     docs/   File     then a level down, grouped under their parents
  (1/1342)                      in the order the parents were listed above
```

Parents are compared component by component, so each level's groups follow
the order of the level above. A case-only tie (`B.txt`/`b.txt`) falls to the
spelling, which makes the order **total**: `read_dir` hands back a directory's
entries in no promised order, and a merely partial order would let the same
files reshuffle between two keystrokes. A typed query is ranked exactly as
before (score, then shorter path, then the path).

The listing order is dearer to sort than shortest-first, and it runs on
every bare `@`: over a 10,000-entry walk of `/usr` it took about 22 ms
against 6 ms — 55 ms before two shortcuts that change no order, siblings
(whose parents are spelled byte for byte the same) skipping the component
walk and two plain-ASCII names folding a byte at a time instead of through
`char`'s Unicode tables (`the_bare_listing_folds_case_beyond_ascii_too`
pins the non-ASCII case). What made it cheap is that the walk now hands it
a list already in this order: see *What a search costs*.

### A capped walk says so

The walk stops at `FILE_INDEX_CAP` (10,000 entries), breadth-first, so a
bigger tree loses its deepest entries — and a counter that read `(1/10000)`
there would claim the whole workspace. A walk now reports `truncated` when an
entry it would have listed was left out; it reads on past the cap until it
meets one, so a tree that exactly fills the cap is complete. The flag rides
the result beside the matches (`FileSearchResult::truncated` →
`App::set_file_matches` → `FileSearch::truncated`), and the counter closes
its total with `+` — `(1/10000+)`, `(1/23+)` — since the total is then a
floor. It is left out whole where the width cannot hold it, like the counter
itself.

## What a search costs

Date: 2026-11-11

A search runs on every keystroke (coalesced), so its cost was measured
before and after four changes, in `/usr` — where the walk stops at its
10,000-entry cap, the worst case — and in this repository (890 entries).
Times are medians of 21 runs of a throwaway timing test over the real
`walk_files` and `rank_files`; keypress to list is tmux polling for the
counter after typing `@` (median of 9); memory is the process's heap,
`RssAnon`, because `VmRSS` also counts the binary's own pages, which move
with the page cache — two identical idle processes differed by 1.5 MB
there and by 4 KB here.

| `/usr`, 10,000 entries | before | after |
|---|---|---|
| reading the folders | 9.2 ms | 7–8 ms |
| ranking a bare `@` | 22.6 ms | 2.6 ms |
| ranking `@s` | 5.0 ms | 2.2–2.6 ms |
| ranking `@lib` | 3.7 ms | 1.9–2.3 ms |
| keypress to list, bare `@` | 43 ms | 22 ms |
| heap with a bare `@` open | +1,372 KB | +895 KB |
| heap with `@s` open | +2,438 KB | +1,752 KB |
| heap kept after closing | +2,438 KB | +1,316 KB |

In this repository a search was already 2–3 ms and stays so; keypress to
list there is the frame scheduling's, 16–18 ms either way.

1. **Each folder is read into listing order.** The walk sorts a folder's
   own entries as it reads them (`sibling_order`, names alone), so
   breadth-first the whole walk comes out in the bare listing's order and
   that sort, being adaptive, finds nothing to move: 22.6 ms became
   2.6 ms, for about 0.2 ms of walk time. The listing still sorts whatever
   it is handed — the walk being in order makes the sort cheap, never
   right.
2. **Each path is built once.** The walk allocated three or four strings
   per entry and kept one — a full path, a copy relative to the root, a
   copy with backslashes rewritten, a folder's copy with its `/` — and now
   joins the folder's path and the name into the one it keeps.
3. **Ranking stops early and counts once.** Scoring a path no longer
   allocates (the query's characters gathered once, no offsets kept),
   which alone bought about 5%: the time was elsewhere. The scoring loop
   now stops at the query's last matched character, and each match's
   length is counted once before the sort instead of twice in every
   comparison between equal scores; together, about half.
4. **Bold letters are worked out for the rows drawn.** A match keeps its
   path and score; `file_menu_lines` asks `fuzzy_match` for the offsets of
   at most five rows a frame. That is most of the heap a typed query
   saves.

Not done: one buffer for every path instead of a string each (the largest
remaining heap item, but a rewrite of the walk's and the band's types for
a saving only huge folders see); walking once per `@` instead of once per
keystroke (faster, but a file created while the list is open would not
appear until `@` was typed again); and handing freed memory back to the
system (`malloc_trim` needs `unsafe`, which the crate forbids).

## Testing

- `file_search` (unit): `at_token` triggers after whitespace/start, not inside
  `email@host`, finds the token around the cursor, empty query for a lone `@`;
  `fuzzy_match` is an ASCII-case-insensitive subsequence with correct indices and
  None on no match; `rank_files` ranks basename/contiguous hits first, sorts
  ties by length then name, drops non-matches but keeps every match, and
  lists an empty query like a directory listing — the same order whatever
  order the walk found it in.
- `src/tui/` (bin unit, `cargo test --bin alter-zero tui::workers`): the walk
  lists the tree with directories marked, skips hidden and denylisted
  entries, reports `truncated` only when its cap left an entry unlisted,
  keeps a `\` in a name as part of the name, and reads each folder into
  listing order — its output equal to an explicit list and to the pure
  listing of itself; `sibling_order` has its own unit test.
- `textarea` (unit): `replace_range` splices and seats the cursor after.
- `app` (unit): `@` opens the picker and `file_search_query` tracks the token;
  ↑/↓ move and wrap at the ends; Enter/Tab accept (the `@token` becomes `path ` with the
  cursor after); Esc dismisses and is sticky; `set_file_matches` drops stale
  results and keeps the walk's `truncated` beside the matches; the picker
  stays closed in shell mode and is suppressed in the tool view;
  submit/clear/recall close it.
- `ui` (unit): `file_menu_rows` is 0 closed, 1 for a placeholder, five rows
  and the counter for a long list, and equal to the painted lines at every
  width; `file_menu_lines` splits each match into aligned name / parent /
  kind columns (`./` for root-level, `File`/`Dir` pinned at `width − 6`, a
  truncated deep parent keeping the pin), marks only the selected row with
  `→` and the cyan highlight, bolds the chars its query matches in *both*
  columns — worked out from the query for the rows drawn, a row the query
  no longer matches drawn plain — and degrades to marker + name when too
  narrow; the dim `(n/total)` counter
  follows the highlight, counts every match, takes a `+` for a truncated
  walk, and is left out where the width cannot hold it; the window keeps the `→` on the middle row once there is
  room above it; `live_height` grows by the band and `cursor_position` stays
  put when it opens.
- `src/tui/` (smoke, Phase 25): launch in a temp dir with known files; a bare
  `@` opens on `subdir/`, then the top-level files, over `(1/16)`, and ↑ wraps
  to the nested `beta_smoke.txt` over `(16/16)`; type
  `@alpha`, the picker lists the file in the columned `→ name  ./  File`
  layout, Enter inserts its path into the composer — then create a file and
  search again: the per-query walk lists a file that didn't exist at startup.
  Then `@pick_` over twelve planted files: five rows over `(1/12)`, three ↓
  put `pick_04.txt` on the middle row with `pick_01.txt` scrolled out, and ↑
  from the first wraps to `pick_12.txt` over `(12/12)` — the twelfth match,
  which the old best-eight list could not reach.

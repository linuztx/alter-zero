# /resume — pick up a saved session (rollout files + session picker)

A port of openai/codex's `/resume`: every conversation is recorded to a
JSONL "rollout" file as it happens, and the `/resume` slash command opens a
full-screen picker of those saved sessions — `❯ {age}  {first user message}`
rows, type-to-search — whose Enter loads the chosen conversation back into the
app (repainted inline, exactly as a resize repaints it) and appends the turns
that follow to the *same* file. Codex sources: the `codex-rs/rollout` crate
(`recorder.rs`, `list.rs`, `policy.rs` — the file format, the lazy-create
recorder, the date-dir walk + head-scan listing), `tui/src/resume_picker.rs`
(the alt-screen picker), `tui/src/session_resume.rs` and
`tui/src/app/session_lifecycle.rs` (the swap-the-live-session flow), and the
`SlashCommand::Resume` entry in `tui/src/slash_command.rs`.

## What codex does (findings)

Recording (the `rollout` crate):

- Sessions live under `~/.codex/sessions/YYYY/MM/DD/` (local date of session
  start), one file per session:
  `rollout-YYYY-MM-DDThh-mm-ss-{uuid}.jsonl` (`recorder.rs::
  precompute_log_file_info`; `-` for `:` in the time so the name is
  filesystem-safe).
- Every line is `{"timestamp": <UTC write time>, "type": <kind>, "payload":
  {…}}`. The **first** line of a file is always `type: "session_meta"`
  (`payload`: the thread `id`, session-start `timestamp`, `cwd`, `originator`,
  `cli_version`, model/provider fields…); the rest are the conversation's
  items in file order — identity lives in the meta line + filename, not in
  per-line ids.
- **Only completed items persist, never streaming deltas** (`policy.rs`:
  user/agent messages, tool calls + outputs persist; `*Delta`, begin-events,
  and UI-only noise are dropped).
- File creation is **deferred**: the path is precomputed but nothing touches
  disk until the first recordable item, so empty sessions leave no file
  (`recorder_tests.rs::recorder_materializes_on_flush_with_pending_items`).
  Resuming opens the existing file in append mode — one file accumulates the
  whole thread across resumes.
- Reading back skips unparseable lines (forward compatibility) and errors on
  a file with no records.

Listing (`list.rs`):

- Walk `sessions/` newest-first (year/month/day dirs sorted descending
  numerically, filenames by embedded timestamp descending), with a hard scan
  cap. Default sort in the picker is **Updated** — file mtime, newest first.
- Per file a **head scan** (up to 10 lines, extended to ~210 while hunting for
  a user message) extracts the meta line and the **first user message**, which
  doubles as the picker preview. A session is eligible **only if both are
  found** — sessions the user never typed into don't list.

The picker (`resume_picker.rs`) — an alternate-screen overlay:

- Layout: a title (`Resume a previous session`), a search line (`Type to
  search` placeholder), the session rows, and a footer — a `─` separator with
  a right-aligned `{selected+1} / {total}` progress label over dim key-hint
  rows. Dense rows are one line each:
  `❯ {relative-age padded to 12}{preview}` — the marker on the selected row,
  the age dim, the preview truncated with a `...`.
- Ages are single-unit relative times — `now`, `{N}s ago`, `{N}m ago`,
  `{N}h ago`, `{N}d ago` (`format_relative_time`) — computed against a
  reference **frozen when the picker opens**, so the list doesn't tick.
- Keys: ↑/↓ move (wrapping at the ends), PageUp/PageDown jump by a viewport
  (clamped), Home/End jump
  to the ends, Enter resumes the selection, and **any plain printable char is
  search input** (case-insensitive substring filter; selection resets to the
  top match), Backspace pops the query. **Esc clears the query first**; only
  an Esc on an empty query cancels. From a running session, cancel (and
  Ctrl+C) simply closes the picker — the live conversation is untouched.
- Empty states in the list area: `No sessions yet` / `No results for your
  search`.

The flow (`slash_dispatch.rs`, `session_lifecycle.rs`):

- `/resume` is `SlashCommand::Resume`, description "resume a saved chat".
  It is **rejected while a real agent turn is running** (an error cell;
  `slash_command_blocked_by_active_task`) — the picker never has to race a
  streaming turn.
- On selection the backend loads the rollout and **only on success** does the
  UI swap to the resumed conversation; a failure surfaces as an error cell and
  the current session continues unharmed. The resumed thread keeps appending
  to its existing file; the abandoned session's file stays resumable later.

## What we build

The same feature, sized to this codebase: a pure `session` module (the file
format + listing/parse logic + the picker's pure state), a small recorder at
the I/O boundary in `main.rs`, and a third `View` rendered with the Ctrl+O
overlay machinery.

### The session file (src/session.rs, pure)

- Files: `{root}/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-{id}.jsonl`, where
  `{root}` is `~/.alter-zero/sessions` (override: `ALTER_ZERO_SESSIONS_DIR`,
  the `ALTER_ZERO_STARTUP_DELAY_MS` pattern — the smoke test points it at a
  temp dir). `{id}` is nanos-since-epoch + pid in hex — unique enough without
  a uuid dependency, and never parsed back (we resume by *path*). The path
  derivation is the pure `session::rollout_rel_path(date, time, id)`; the
  clock/pid stay at the boundary.
- Lines are codex's shape: `{"timestamp": <UTC millis Z>, "type": …,
  "payload": …}`. Line 1 is `session_meta`
  (`{id, timestamp, cwd, model, originator: "alter-zero", version}`); then one
  line per finished [`HistoryItem`] as it lands in `App::history`:
  - `"message"` → `{role: "user"|"assistant"|"system"|"error"|"shell", text,
    timestamp}` (the display stamp the item already carries),
  - `"tool"` → `{name, args, ok: bool, output, timestamp, shell, truncated}`
    (a running tool is never in `history`, so only finished statuses exist),
  - `"summary"` → `{verb, secs, timestamp}` (`verb` maps back to the
    `DONE_VERBS` static on load, falling back to `"Done"` — `TurnSummary.verb`
    is `&'static str`),
  - `"reasoning"` → `{text, timestamp, secs, tokens}` — one settled thinking
    phase, so a resumed session keeps its `Thought for …` cells *and* the
    chain-of-thought their Ctrl+O expansion shows (`docs/thinking-stream.md`);
    the counts are `serde(default)`ed, so a record without them loads with 0s.
  Streaming deltas, the status line, and token tallies are never recorded —
  codex's persistence policy. (A *settled* thinking phase is an ordinary
  history item and does get its line; the raw deltas do not.)
- Serialization is `serde`/`serde_json` on **module-local record types**
  (`SessionMeta`, a tagged line enum) mapped to/from the app types, so the
  on-disk format is decoupled from `app/` and the app types stay
  serde-free. Parsing skips malformed/unknown lines (forward compatibility);
  `parse_session` yields the meta + items and `None` for a file with no valid
  meta line.
- Listing helpers: `preview_of` (the first non-blank `user` — or `shell`, see
  divergences — message's text, whitespace flattened, `! ` prefix for shell)
  and `relative_age(secs)` (codex's `now`/`Ns ago`/`Nm ago`/`Nh ago`/`Nd ago`
  buckets). `SessionSummary { path, updated_secs, created_secs, cwd,
  preview }` is what the picker holds: both sort keys as scan-frozen
  seconds-ago values (the displayed age is `relative_age` of the active key's
  value), and the meta line's recorded cwd for the `Cwd` filter.

### Recording (main.rs, the I/O boundary)

A `SessionRecorder` owns the root dir, the active file path + meta, and a
`recorded` watermark (the scrollback `committed` pattern, applied to disk):

- `sync(&app.history)` runs once per event-loop iteration (and once after the
  loop exits): `history.len() > recorded` appends the new items —
  lazily creating the date dirs + file and writing the meta line first on the
  very first append, so empty sessions never touch disk (codex's deferred
  create) — and `history.len() < recorded` **rewrites** the file (meta +
  every item): the only truncation paths are the Esc-Esc backtrack rewind
  and `/clear`, and `/clear` resets the recorder to a fresh (deferred) session
  file first, codex's `/new`. History is append-or-truncate only, so the
  watermark compare is sound. Chunks never touch `history`, so streaming
  costs no I/O.
- Recording failures are ignored (`let _ =`) — the TUI must not die because
  a disk filled; codex logs and carries on similarly.
- On resume, the recorder **adopts** the loaded file (path, its parsed meta,
  `recorded = items.len()`) and appends from there — same-file accumulation,
  codex's `Resume{path}` mode.
- The scan for the picker also lives here: walk `{root}` year/month/day dirs
  descending collecting candidates (bounded by `RESUME_WALK_CAP`), sort by
  mtime descending (codex's Updated sort) **before** capping the expensive
  part — only the newest `RESUME_SCAN_CAP` files get their heads read
  (`RESUME_HEAD_LINES` lines under a generous `RESUME_HEAD_BYTES` ceiling, so
  a large pasted first message still yields its preview), keep files with a
  meta line + a preview (`session::parse_session` + `session::preview_of`),
  exclude the recorder's active file, and hand `Vec<SessionSummary>` (ages
  computed here, frozen at open) to the pure core.

### State + keys (app/resume.rs)

- `View::ResumePicker` — a third view. `ResumePicker { sessions, selected,
  query, cwd, filter, sort, focus }` state on `App` (`Some` while open); the
  rows derive on demand (`ResumePicker::matches()` — the `Cwd`/`All` filter,
  then a case-insensitive substring over the preview (codex's client-side
  `Row::matches_query`), then the active sort key's order), like the
  palette's `matching_commands`. The **Filter/Sort toolbar** is codex's:
  `ResumeFilter { Cwd (default), All }` compares each session's recorded cwd
  to the picker's own (verbatim — both sides are the recorder's
  `Path::display` formatting), `ResumeSort { Updated (default), Created }`
  picks the key, and `ResumeControl { Filter (initial), Sort }` is the
  Tab-cycled focus ←/→ act on; any toggle reseats the selection (codex
  restarts its list the same way).
- `COMMANDS` gains `resume` ("Resume a saved chat") →
  `CommandEffect::Resume`. Running it while a turn is active returns
  `Action::Toast(RESUME_BUSY_NOTICE)` — a transient toast above the box (codex
  rejects mid-task; it swaps the whole conversation, racing the stream), *not* a
  committed message (updated 2026-07-08; was `Action::ErrorNotice`, see
  `docs/toast.md`). Idle it returns `Action::OpenResumePicker` — the *loop* scans
  the disk and calls `App::open_resume_picker(sessions)` (sets the view, resets
  the backtrack prime, like `toggle_tool_view`).
- `on_key_resume_picker`: ↑/↓ wrapping moves (`wrap_step`); PageUp/PageDown by
  `RESUME_PAGE` (10 — the tool view's page); Home/End; **Tab/BackTab swap the
  toolbar focus and ←/→ toggle the focused control's value** (Filter
  Cwd↔All, Sort Updated↔Created — reseating the selection); plain chars
  append to `query` + reset `selected`; Backspace pops; Esc clears a
  non-empty query first, else closes (`Action::CloseResumePicker`); Enter
  returns `Action::ResumeSession(path)` for the selected filtered row
  (`None` on an empty list); Ctrl+C closes too (codex's from-a-session
  picker), overriding the global quit; Ctrl+O is swallowed (both views share
  the alt screen). A bracketed paste joins the query via
  `paste_into_resume_search` (codex's `normalize_pasted_search_query` —
  whitespace runs collapse to single spaces, a separating space before a
  non-empty append, whitespace-only pastes ignored).
- `App::load_session(items)` — the `/clear` reset shape (drain the queue into
  discarded images, drop streaming/tool/status state — defensive; a turn
  can't be active) but installing `items` as the new `history` and returning
  to `View::Conversation`. `input_history`, the composer draft, and
  attachments survive, like `/clear`.

### The loop (main.rs)

- `Action::OpenResumePicker` → scan, `app.open_resume_picker(sessions)`,
  `term.enter_overlay()`, paint at once (the Ctrl+O no-black-flash pattern).
- `Action::CloseResumePicker` → `term.exit_overlay()` + the ordinary
  overlay return (`overlay_return_repaint` — flush what queued, repaint the
  live region).
- `Action::ResumeSession(path)` → read + `session::parse_session` at the
  boundary. Ok: `app.load_session(items)`, recorder adopts the file,
  `exit_overlay` + a **purge** `repaint_conversation` (like `/clear`) — the
  loaded session *replaces* the whole conversation, so the rebuild fills
  scrollback with its full history; a plain return would leave the previous
  chat above it and commit none of the resumed session's earlier turns. Err:
  close the picker with the normal return, then
  `commit_error_notice("Failed to load session: …")` — the current
  conversation continues unharmed (codex).
- `Action::Toast(text)` → `present_toast` (a transient info toast; the
  `Action::ErrorNotice` red-committed twin of `Action::Notice` it replaced was
  removed on 2026-07-08 — see `docs/toast.md`). A failed session *load* still
  commits a red `commit_error_notice` (it's a real error worth keeping, not an
  ephemeral rejection).
- The Quit arm's overlay unwind generalizes from `view == ToolOutput` to
  `view != Conversation`; the draw branch gains a
  `View::ResumePicker => term.draw_overlay(ui::render_resume_picker)` arm.
  Scrollback commits were already gated on `view == Conversation`, so nothing
  can write the alt screen while the picker is up (invariant 4 holds — and no
  turn is running anyway).

### Rendering (ui/resume_view.rs)

`render_resume_picker` on the alternate screen, chrome matching the Ctrl+O
transcript pager, styling in `RESUME_*` consts:

```
/ R E S U M E / R E S U M E / R E S U M E …          ← slash-tiled title row
                                                      ← blank
  Type to search     Filter: [Cwd] All   Sort: [Updated] Created
                     └ the toolbar, right-aligned on the search line ┘
                                                      ← blank
  ❯ 5m ago       fix the wrap bug in ui.rs            ← rows: marker+age+preview
    2h ago       ! cargo test                           (selected row on a
    3d ago       hello there                            full-width bg tint)
  …                                                   ← `No sessions yet` /
                                                        `No results for your search`
─────────────────────────────────────── 1/3 ─        ← separator + right count
  ↑/↓ select · enter resume · esc cancel · tab + ←/→ filter/sort
```

The search line's left side is the dim `Type to search` placeholder (or
`Search: {query}`); its right edge carries codex's **Filter/Sort toolbar**
(`resume_toolbar_spans`): dim `Filter:`/`Sort:` labels, the active value
bracketed (`[Cwd]` — `resume_focus_color()` magenta when its control holds the
Tab focus, plain otherwise), inactive values dim. When the full tab pairs
don't fit beside the search text the toolbar compacts to the active values
(`Filter:[Cwd]   Sort:[Updated]`, codex's compact form), and on the
narrowest screens it drops.

Rows are codex's dense density: `❯ ` marker (spaces when unselected), the
**active sort key's** age padded to `RESUME_AGE_WIDTH` (12 — codex shows only
that timestamp per row), the preview ellipsis-truncated to the width
(`cols()` math). The selected row lights up in the palette's selected colour
**on a full-width `resume_selected_bg()` tint** (padded to the edge in columns
— the user-message block pattern, codex's background blend); the rest dim.
The row window scrolls to keep the selection visible (derived from `selected`
and the list height; no stored scroll offset).

## Known divergences from codex

- **`!` shell sessions list too**: codex's eligibility wants a *user
  message*; our `Role::Shell` header is user input in every sense, so it
  counts (previewed as `! {command}`). A session that's only ever run shell
  commands is still worth resuming.
- **The active session is excluded from the list** instead of codex's
  select-time "Already viewing" guard — codex's picker is thread-based with
  ids/names; ours is file-based, and the file you're writing is noise in a
  list of things to *return* to.
- **No pagination, density toggle, or transcript preview**: the scan is
  capped and loaded whole at open (a local dir, not an app-server), always
  dense rows. Type-to-search and the Filter/Sort toolbar re-derive from what
  was loaded. Codex's Ctrl+T full-transcript preview already exists here as
  Ctrl+O once resumed. (The toolbar itself — `Filter: [Cwd] All`,
  `Sort: [Updated] Created`, Tab focus + ←/→ — **is** ported; codex
  additionally persists the density choice to config, which we don't have.)
- **The `Cwd` filter compares recorded cwd strings verbatim** (both sides are
  the recorder's `Path::display` formatting) — codex normalizes paths before
  comparing; a session recorded through a symlinked path won't match its
  canonical twin here.
- **`/resume <id>` inline args are out of scope** — but the CLI twins now
  exist (`docs/cli.md`, 2026-07-31; the binary took no args before then):
  `--continue` reopens the newest session recorded in this cwd,
  `--resume {id}` one by id (the filename's `{id}` segment, now addressable
  via `session::rollout_file_id`), bare `--resume` boots into this picker,
  and quitting a session that recorded anything prints the
  `--resume {id}` hint.
- **No cwd prompt**: codex offers to chdir to the session's recorded cwd.
  Ours records `cwd` in the meta (forward-compatible) but resumes in place —
  the dummy backend has no cwd-dependent behaviour to protect.
- **Timestamps inside items are display strings** (`hh:mm AM/PM`, possibly
  empty) — they round-trip verbatim; only the line/meta stamps are
  machine-readable UTC. Codex records RFC3339 everywhere.
- **A truncation rewrite normalizes the file.** A backtrack rewind after a
  resume rewrites the file from the *parsed* history, so lines a future build
  wrote and this build skipped (the forward-compat reader) are dropped by
  that rewrite. Codex's files are append-only (rollbacks are recorded as
  events); our rewrite-on-truncate keeps the file a mirror of `App::history`
  instead. (A resumed file whose last line lost its newline — a torn write —
  is repaired on the first append rather than glued onto.)
- **Concurrent instances**: two alter-zero processes never share a file
  (the id embeds the pid), but resuming the *same* saved session from two
  instances interleaves appends unguarded — codex has state-db arbitration;
  documented limitation here.

## Testing

- `session`: meta/item lines round-trip through `parse_session` (multiline +
  quote + unicode text, every role, ok/failed/truncated tools, summary verb
  restored to the `DONE_VERBS` static — unknown verb falls back to `Done`);
  malformed and unknown lines are skipped; a file without a meta line (or
  empty) parses to `None`; `preview_of` finds the first non-blank user/shell
  message (flattening whitespace, `! ` for shell, whitespace-only messages
  skipped) and `None` without one;
  `relative_age` buckets (`now`, `59s ago`, `1m`, `59m`, `1h`, `23h`, `1d`);
  `rollout_rel_path` zero-pads and swaps `:` for `-`.
- `app`: the `/resume` palette entry runs to `OpenResumePicker` idle and a
  `Toast(RESUME_BUSY_NOTICE)` mid-turn (2026-07-08; was `ErrorNotice`);
  `open_resume_picker` enters the view (and resets a
  primed backtrack) with codex's defaults (filter `Cwd`, sort `Updated`,
  focus `Filter`); picker keys — ↑/↓ wrap, PageUp/PageDown/Home/End,
  type-to-filter narrows and reseats the selection, Backspace pops, a paste
  joins the query flattened, Esc clears-then-closes, Ctrl+C closes, Ctrl+O
  is swallowed, Enter yields `ResumeSession` with the selected (filtered)
  path and `None` on an empty list; the `Cwd` filter hides other
  directories until → toggles `All`; Tab moves the toolbar focus and ←/→
  toggle the focused control (`Created` re-orders by the start stamp);
  `load_session` installs history, returns to the conversation view, and
  clears queued/streaming leftovers into discarded images.
- `ui`: `render_resume_picker` — title row, search placeholder vs `Search:`
  echo, the right-aligned toolbar (brackets following the toggles, the
  compact form when narrow, dropped when narrower), marker + dim age +
  preview rows with the selection lit **on the full-width background tint**,
  the age column following the active sort key, the right-aligned
  `{n}/{total}` count, both empty states, narrow-width truncation, and the
  selection kept visible in a short window.
- `scripts/smoke.sh` Phase 31 (the I/O boundary): with
  `ALTER_ZERO_SESSIONS_DIR` pointed at a temp dir — a turn writes a rollout
  file (meta + user + assistant lines); a second launch's `/resume` picker
  lists it (preview visible), Enter repaints the old conversation inline, a
  follow-up turn **appends to the same file** (no second file), and the
  relaunch after *that* shows both turns; `/clear` then starts a fresh file.

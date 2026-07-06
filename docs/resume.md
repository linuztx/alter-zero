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
- Keys: ↑/↓ move (clamped), PageUp/PageDown jump by a viewport, Home/End jump
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
  `{root}` is `~/.inline-tui/sessions` (override: `INLINE_TUI_SESSIONS_DIR`,
  the `INLINE_TUI_STARTUP_DELAY_MS` pattern — the smoke test points it at a
  temp dir). `{id}` is nanos-since-epoch + pid in hex — unique enough without
  a uuid dependency, and never parsed back (we resume by *path*). The path
  derivation is the pure `session::rollout_rel_path(date, time, id)`; the
  clock/pid stay at the boundary.
- Lines are codex's shape: `{"timestamp": <UTC millis Z>, "type": …,
  "payload": …}`. Line 1 is `session_meta`
  (`{id, timestamp, cwd, model, originator: "inline-tui", version}`); then one
  line per finished [`HistoryItem`] as it lands in `App::history`:
  - `"message"` → `{role: "user"|"assistant"|"system"|"error"|"shell", text,
    timestamp}` (the display stamp the item already carries),
  - `"tool"` → `{name, args, ok: bool, output, timestamp, shell, truncated}`
    (a running tool is never in `history`, so only finished statuses exist),
  - `"summary"` → `{verb, secs, timestamp}` (`verb` maps back to the
    `DONE_VERBS` static on load, falling back to `"Done"` — `TurnSummary.verb`
    is `&'static str`).
  Streaming deltas, thinking, the status line, and token tallies are never
  recorded — codex's persistence policy.
- Serialization is `serde`/`serde_json` on **module-local record types**
  (`SessionMeta`, a tagged line enum) mapped to/from the app types, so the
  on-disk format is decoupled from `app.rs` and the app types stay
  serde-free. Parsing skips malformed/unknown lines (forward compatibility);
  `parse_session` yields the meta + items and `None` for a file with no valid
  meta line.
- Listing helpers: `preview_of` (the first non-blank `user` — or `shell`, see
  divergences — message's text, whitespace flattened, `! ` prefix for shell)
  and `relative_age(secs)` (codex's `now`/`Ns ago`/`Nm ago`/`Nh ago`/`Nd ago`
  buckets). `SessionSummary { path, age, preview }` is what the picker holds.

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

### State + keys (app.rs)

- `View::ResumePicker` — a third view. `ResumePicker { sessions, selected,
  query }` state on `App` (`Some` while open); the filtered rows derive on
  demand (`ResumePicker::matches()` — case-insensitive substring over the
  preview, codex's client-side `Row::matches_query`), like the palette's
  `matching_commands`.
- `COMMANDS` gains `resume` ("Resume a saved chat") →
  `CommandEffect::Resume`. Running it while a turn is active returns the new
  `Action::ErrorNotice(RESUME_BUSY_NOTICE)` (codex rejects mid-task); idle it
  returns `Action::OpenResumePicker` — the *loop* scans the disk and calls
  `App::open_resume_picker(sessions)` (sets the view, resets the backtrack
  prime, like `toggle_tool_view`).
- `on_key_resume_picker`: ↑/↓ clamped moves; PageUp/PageDown by
  `RESUME_PAGE` (10 — the tool view's page); Home/End; plain chars append to
  `query` + reset `selected`; Backspace pops; Esc clears a non-empty query
  first, else closes (`Action::CloseResumePicker`); Enter returns
  `Action::ResumeSession(path)` for the selected filtered row (`None` on an
  empty list); Ctrl+C closes too (codex's from-a-session picker), overriding
  the global quit; Ctrl+O is swallowed (both views share the alt screen). A
  bracketed paste joins the query via `paste_into_resume_search` (codex's
  `normalize_pasted_search_query` — whitespace runs collapse to single
  spaces, a separating space before a non-empty append, whitespace-only
  pastes ignored).
- `App::load_session(items)` — the `/clear` reset shape (drain the queue into
  discarded images, drop streaming/tool/status state — defensive; a turn
  can't be active) but installing `items` as the new `history` and returning
  to `View::Conversation`. `input_history`, the composer draft, and
  attachments survive, like `/clear`.

### The loop (main.rs)

- `Action::OpenResumePicker` → scan, `app.open_resume_picker(sessions)`,
  `term.enter_overlay()`, paint at once (the Ctrl+O no-black-flash pattern).
- `Action::CloseResumePicker` → `term.exit_overlay()`, `repaint_conversation`
  (the Ctrl+O return).
- `Action::ResumeSession(path)` → read + `session::parse_session` at the
  boundary. Ok: `app.load_session(items)`, recorder adopts the file,
  `exit_overlay` + `repaint_conversation` — the loaded conversation repaints
  from history exactly like a resize (invariant 3). Err: close the picker the
  same way, then `commit_error_notice("Failed to load session: …")` — the
  current conversation continues unharmed (codex).
- `Action::ErrorNotice(text)` → `commit_error_notice` (a general red twin of
  `Action::Notice`).
- The Quit arm's overlay unwind generalizes from `view == ToolOutput` to
  `view != Conversation`; the draw branch gains a
  `View::ResumePicker => term.draw_overlay(ui::render_resume_picker)` arm.
  Scrollback commits were already gated on `view == Conversation`, so nothing
  can write the alt screen while the picker is up (invariant 4 holds — and no
  turn is running anyway).

### Rendering (ui.rs)

`render_resume_picker` on the alternate screen, chrome matching the Ctrl+O
transcript pager, styling in `RESUME_*` consts:

```
/ R E S U M E / R E S U M E / R E S U M E …          ← slash-tiled title row
                                                      ← blank
  Type to search        (dim; or `Search: {query}`)   ← search line
                                                      ← blank
  ❯ 5m ago       fix the wrap bug in ui.rs            ← rows: marker+age+preview
    2h ago       ! cargo test
    3d ago       hello there
  …                                                   ← `No sessions yet` /
                                                        `No results for your search`
─────────────────────────────────────── 1/3 ─        ← separator + right count
  ↑/↓ select · enter resume · esc cancel · type to search
```

Rows are codex's dense density: `❯ ` marker (spaces when unselected), the age
dim and padded to `RESUME_AGE_WIDTH` (12), the preview ellipsis-truncated to
the width (`cols()` math). The selected row lights up in the palette's
selected colour, the rest dim — the `MENU_*` selection convention. The row
window scrolls to keep the selection visible (derived from `selected` and the
list height; no stored scroll offset).

## Known divergences from codex

- **`!` shell sessions list too**: codex's eligibility wants a *user
  message*; our `Role::Shell` header is user input in every sense, so it
  counts (previewed as `! {command}`). A session that's only ever run shell
  commands is still worth resuming.
- **The active session is excluded from the list** instead of codex's
  select-time "Already viewing" guard — codex's picker is thread-based with
  ids/names; ours is file-based, and the file you're writing is noise in a
  list of things to *return* to.
- **No pagination, sort/filter toolbar, density toggle, or transcript
  preview**: the scan is capped and loaded whole at open (a local dir, not an
  app-server), always mtime-descending, always dense rows. Type-to-search
  filters what was loaded. Codex's Ctrl+T full-transcript preview already
  exists here as Ctrl+O once resumed.
- **`/resume <id>` inline args and `codex resume --last`/CLI subcommands are
  out of scope** — the binary takes no args today; the picker is the whole
  surface.
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
- **Concurrent instances**: two inline-tui processes never share a file
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
- `app`: the `/resume` palette entry runs to `OpenResumePicker` idle and
  `ErrorNotice` mid-turn; `open_resume_picker` enters the view (and resets a
  primed backtrack); picker keys — ↑/↓ clamp, PageUp/PageDown/Home/End,
  type-to-filter narrows and reseats the selection, Backspace pops, Esc
  clears-then-closes, Ctrl+C closes, Ctrl+O is swallowed, Enter yields
  `ResumeSession` with the selected (filtered) path and `None` on an empty
  list; `load_session` installs history, returns to the conversation view,
  and clears queued/streaming leftovers into discarded images.
- `ui`: `render_resume_picker` — title row, search placeholder vs `Search:`
  echo, marker + dim age + preview rows with the selection lit, the
  right-aligned `{n}/{total}` count, both empty states, narrow-width
  truncation, and the selection kept visible in a short window.
- `scripts/smoke.sh` Phase 31 (the I/O boundary): with
  `INLINE_TUI_SESSIONS_DIR` pointed at a temp dir — a turn writes a rollout
  file (meta + user + assistant lines); a second launch's `/resume` picker
  lists it (preview visible), Enter repaints the old conversation inline, a
  follow-up turn **appends to the same file** (no second file), and the
  relaunch after *that* shows both turns; `/clear` then starts a fresh file.

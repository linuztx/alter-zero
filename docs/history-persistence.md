# Cross-session input history — persisting the ↑/↓ + Ctrl+R history to disk

Date: 2026-07-13

## Goal

Make the composer's input history **survive a restart**, the way openai/codex
does. Today `App::input_history` (the `InputHistory` that backs the shell-style
↑/↓ recall — `docs/input-history.md` — *and* the Ctrl+R reverse search —
`docs/history-search.md`) is built up fresh each run and lost on exit. After
this change, submitted inputs are written to an append-only history file, loaded
back at startup, and both recall and search span every past session.

Both input-history.md and history-search.md explicitly left this out ("no
cross-session persistence"); this closes that gap.

## What codex does (`/tmp/codex/codex-rs/message-history`)

Codex keeps a global, append-only **`~/.codex/history.jsonl`**, one JSON object
per line:

```text
{"session_id":"<id>","ts":<unix_seconds>,"text":"<message>"}
```

- **Append** (`append_entry`) builds the full line + `\n` and writes it in a
  **single `write(2)`** with the fd opened `O_APPEND` — POSIX makes writes up to
  `PIPE_BUF` atomic, so concurrent TUI processes don't interleave. It also holds
  an advisory `try_lock` (belt-and-suspenders, and needed because codex *also*
  rewrites the file to trim it) and enforces a `max_bytes` cap, dropping the
  oldest lines down to a soft cap when the file grows too large. The file is
  `0o600` (it can hold whatever the user typed).
- **Read** is lazy and async: the composer holds only a `(log_id, entry_count)`
  metadata snapshot and fetches each cross-session entry on demand by
  `(log_id, offset)` while browsing/searching (`LookupMessageHistoryEntry` →
  `on_entry_response`). That machinery — the `Pending`/`Searching` states — only
  exists because history can be huge and is shared across processes.

## The mapping onto this codebase — load-at-startup, not lazy-fetch

`docs/history-search.md` already records the key simplification: *"We have no
persistent history, so that whole pending machine — and the Searching status —
drops out; everything is synchronous over `InputHistory::entries`."* We keep
that. Instead of codex's async per-offset fetch we **load the file once at
startup and seed `InputHistory::entries`**. Seeding the very vector that ↑/↓ and
Ctrl+R already read means *neither `up`/`down` nor `search` changes at all* —
they transparently span sessions. New submissions append to disk as they are
recorded.

This mirrors the project's existing split exactly: the pure on-disk format lives
in a new `history` module (like `session`), and every impurity — the path from
the environment, the clock for `ts`, the reads/appends — lives at the boundary
in `main.rs` (`InputHistoryStore`, like `SessionRecorder`). The pure core
*queues* newly-recorded inputs and the boundary *drains* them to disk — the same
"core queues, boundary flushes" pattern as `insert_before`.

### Pure format core (`src/history.rs`)

Serde on its **own** record type (the app types stay serde-free, like
`session`), matching codex's schema for forward-compat and interop:

```rust
#[derive(Serialize, Deserialize)]
struct HistoryRecord { session_id: String, ts: u64, text: String }

/// Serialize one entry as a single JSONL line (no trailing newline). serde_json
/// escapes embedded newlines, so a multi-line draft is still ONE physical line.
pub fn history_line(session_id: &str, ts: u64, text: &str) -> String;

/// Parse a whole file's contents into the recorded texts, oldest-first (the
/// order `InputHistory::entries` wants). Blank/malformed/unknown-shape lines
/// **and records whose `text` is empty** are skipped (codex's forward-compatible
/// reader), so a torn or future-version line never breaks the load and a
/// hand-edited empty entry never seeds a blank recall.
pub fn parse_history(contents: &str) -> Vec<String>;
```

Unknown extra fields parse fine (serde ignores them by default), so a file a
future build wrote still loads.

### Pure state (`InputHistory`, in `app/input_history.rs`)

One new field and two new methods; **`record`, `up`, `down`, `search` keep their
existing behaviour**:

```rust
pub struct InputHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    last_recall: Option<String>,
    unpersisted: Vec<String>,   // recorded-this-session, not yet flushed to disk
}
```

- `record(text)` — unchanged blank/adjacent-dup filter for `entries`; **it also
  queues the text on `unpersisted`** for the boundary to write, deduped against a
  separate `last_persisted` watermark (**not** `entries`) so a never-persisted
  `record_ephemeral` draft can't mask a genuine submission that happens to repeat
  it — while the persisted stream still collapses adjacent duplicates.
- `record_ephemeral(text)` — records to `entries` **without** queuing for disk.
  The **Ctrl+C-cleared draft** uses this: codex keeps cleared drafts in its
  in-session `local_history` only, never the persistent file — so a cleared,
  abandoned draft recalls this session but doesn't pollute cross-session history.
  (Every *sent* input — idle submit, `!command`, and mid-turn queued
  batches/shell — uses plain `record` and persists.)
- `seed(entries)` — bulk-load from disk into `entries` at startup as a faithful
  **replay of `record`** (each text runs the same blank-skip + adjacent-duplicate
  collapse), so a messy or concurrently-written file seeds the same clean buffer
  a fresh session would build. **Without** queuing them (they are already on
  disk); the newest becomes the `last_persisted` dedup target. Called once on the
  fresh history.
- `take_unpersisted()` — drain the queue for the boundary to append.

`App` exposes `seed_input_history(entries)` and `take_unpersisted_inputs()`
around these (the `set_clock` boundary-injection pattern). `/clear` still leaves
`input_history` untouched (↑-recall survives — `docs/input-history.md`), so it
neither wipes memory nor the file.

### Boundary (`src/main.rs`, `InputHistoryStore`)

Resolves the path, loads at startup, appends per loop iteration — best-effort,
swallowing every error (recording must never kill the TUI, like `SessionRecorder`
and `save_settings`):

- **Path**: `ALTER_ZERO_HISTORY_FILE` override, else `{config_home}/history.jsonl`
  (`~/.alter-zero/history.jsonl` — beside `config.json`/`.env`/`sessions/`;
  `ALTER_ZERO_CONFIG_DIR` already redirects it, which is how the smoke test
  isolates it). `None` (no HOME, no override) disables persistence.
- **`load()`** → `Vec<String>`: read the file's **bytes** and lossy-decode
  (`String::from_utf8_lossy`, like the shell reader) — a torn/interleaved append
  can leave invalid UTF-8, and `read_to_string` would error on it and throw away
  the *whole* history; lossy decode turns only the bad bytes into U+FFFD so
  `parse_history` skips just that one line. Missing/unreadable → empty. Capped to
  the last `HISTORY_MAX_ENTRIES` (10 000) in memory so recall/search stay
  O(small) no matter how big the file grew.
- **Compaction**: when the file holds more than `HISTORY_MAX_ENTRIES * 5/4`
  entries, `load()` rewrites it down to the newest `HISTORY_MAX_ENTRIES`
  (codex's hard-cap → soft-cap idea, count-based instead of byte-based). Rewritten
  entries are re-stamped with this session's id + now (the `ts`/`session_id`
  fields are unused by the app — they exist only for the file's self-description
  and codex interop). Done once at startup; the tiny race with a concurrent
  instance's append is accepted (codex accepts eventual consistency here too).
- **`append(texts)`**: build every `history_line(session_id, now, text)` + `\n`
  into one buffer and write it with a single `O_APPEND` `write_all` (atomic up to
  `PIPE_BUF`), creating the parent dir + a `0o600` file on first write. An input
  over `HISTORY_MAX_ENTRY_BYTES` (100 KiB) is **skipped** — a large paste is
  expanded in the composer, so its full text would otherwise be written, and the
  entry-*count* cap can't bound the file's *bytes*; a giant blob still recalls
  this session (from `entries`) but isn't worth a reverse-search entry (codex
  bounds by `max_bytes` for the same reason). Called once per loop iteration on
  `app.take_unpersisted_inputs()`, right beside `recorder.sync(...)`, plus once
  more after the loop for the final flush.

`session_id` reuses `main.rs`'s existing `session_id()` helper (nanos+pid hex);
`ts` is `SystemTime::now()` — both impurities stay at the boundary.

## Known limitations

- **No advisory lock.** Appends rely on `O_APPEND` single-write atomicity, and
  startup compaction runs unlocked. Two instances appending very large
  (>`PIPE_BUF`) inputs at the same instant, or one appending exactly during the
  other's rare startup compaction, could lose/tear one line — the loader
  lossy-decodes the file and the parser skips a torn line, so at worst one
  history entry is dropped (never the whole file). codex adds a lock because it
  trims on every write; we trim only at startup when far over cap, so the window
  is negligible.
- **Ordering is append-time, not global-clock.** Concurrent sessions interleave
  their appends; a single session is always in order.

## Testing

- `history` (unit): `history_line` round-trips through `parse_history`; a
  multi-line/`"`-containing/unicode text stays one physical line and round-trips;
  blank lines, malformed JSON, wrong-shape lines, **and empty-`text` records** are
  skipped; extra unknown fields still parse; order is preserved oldest-first;
  empty input → empty vec.
- `app` (unit): `record` queues the text on `unpersisted` (drained by
  `take_unpersisted`) and a blank/adjacent-dup record queues nothing;
  `record_ephemeral` records to `entries` but queues nothing; `seed` fills
  `entries` (so ↑ recalls a seeded entry and `search` finds it) without queuing;
  the Ctrl+C-clear path records ephemerally (a cleared draft recalls but is not
  queued for disk); **a sent message that duplicates a cleared ephemeral draft is
  still persisted** (persist dedup is against `last_persisted`, not `entries`);
  an immediately re-sent message persists once; a submission equal to the newest
  seeded entry isn't re-persisted; **`seed` collapses adjacent duplicates** like
  `record` (a messy/concurrent file seeds a clean buffer).
- `src/tui/` (smoke, Phase 37): submit a message in one process, quit, **append an
  invalid-UTF-8 line to the file**, start a **second** process against the same
  `ALTER_ZERO_HISTORY_FILE`, and confirm ↑ recalls the previous session's message
  and Ctrl+R finds it — cross-session persistence end to end, and a corrupt tail
  line doesn't wipe the history (the lossy load skips only the bad line).

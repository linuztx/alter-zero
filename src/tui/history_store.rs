//! Persisting the composer's input history across sessions — codex's
//! `history.jsonl`, sized down to this loop (`docs/history-persistence.md`).
//!
//! The ↑/↓ recall and the Ctrl+R search read `App::input_history`, which this
//! store seeds at startup ([`InputHistoryStore::load`]) and grows once per
//! loop iteration ([`InputHistoryStore::append`]) — so history spans past
//! runs without the pure core ever knowing there is a file.
//!
//! Three bounds keep it honest, since this file outlives every session:
//! entries are capped at [`HISTORY_MAX_ENTRIES`] (recall and search are
//! O(entries)), the file is compacted only once it passes the
//! [`HISTORY_COMPACT_AT`] hard cap, and a single entry over
//! [`HISTORY_MAX_ENTRY_BYTES`] is never written at all — an expanded
//! large paste would otherwise bloat the file without moving the entry count.
//!
//! The JSONL format is the pure `history` module's. Reads are lossy on
//! purpose: a torn append must cost one line, not the whole history. Writes
//! are owner-only (`0o600`) and best-effort — persistence never kills the TUI.

use std::io;
use std::path::{Path, PathBuf};

use alter_zero::history;

use super::config::config_home;
use super::host::{session_id, unix_secs};

/// Entries kept in memory (and the file's target after compaction). Recall and
/// Ctrl+R search are O(entries), so this bounds their cost no matter how big
/// the file grew — a shell-style HISTSIZE. See `docs/history-persistence.md`.
const HISTORY_MAX_ENTRIES: usize = 10_000;

/// Compact the file only once it exceeds this many entries (hard cap), trimming
/// back to [`HISTORY_MAX_ENTRIES`] (soft cap) — codex's hard-/soft-cap idea, so
/// we don't rewrite on every startup while hovering at the cap.
const HISTORY_COMPACT_AT: usize = HISTORY_MAX_ENTRIES + HISTORY_MAX_ENTRIES / 4;

/// Don't persist an input longer than this many bytes. A large paste is
/// **expanded** in the composer (`take_input` splices the payload back), so its
/// full text would otherwise be written — and the entry-*count* cap alone
/// wouldn't bound the file's byte size. A giant blob isn't a useful
/// reverse-search target anyway; it still recalls **this** session (it lives in
/// `InputHistory::entries`), it just isn't written to disk. codex bounds its
/// file by `max_bytes` for the same reason. See `docs/history-persistence.md`.
const HISTORY_MAX_ENTRY_BYTES: usize = 100 * 1024;

/// Persists the composer's input history across sessions (codex's
/// `history.jsonl`, sized down to this loop — see `docs/history-persistence.md`).
///
/// The pure format lives in [`history`]; this owns the impurities: the path
/// from the environment, the clock for `ts`, and the file reads/appends/
/// compaction. Best-effort — every failure is swallowed (persistence must never
/// kill the TUI, like [`SessionRecorder`] and `save_settings`).
pub(crate) struct InputHistoryStore {
    /// The history file (`ALTER_ZERO_HISTORY_FILE`, else
    /// `{config_home}/history.jsonl` — beside `config.json`/`.env`, which
    /// `ALTER_ZERO_CONFIG_DIR` already redirects). `None` disables persistence
    /// (no HOME and no override).
    path: Option<PathBuf>,
    /// This process's id, stamped into each record's `session_id` field. The
    /// app never reads it back — it only labels who wrote the line.
    session_id: String,
}

impl InputHistoryStore {
    pub(crate) fn new() -> Self {
        let path = std::env::var_os("ALTER_ZERO_HISTORY_FILE")
            .map(PathBuf::from)
            .or_else(|| config_home().map(|dir| dir.join("history.jsonl")));
        Self {
            path,
            session_id: session_id(),
        }
    }

    /// Load the persisted entries (oldest first), capped to the last
    /// [`HISTORY_MAX_ENTRIES`]. When the file has grown past the hard cap,
    /// rewrite it down to the soft cap first (best-effort). A missing or
    /// unreadable file yields no entries — a first run just starts empty.
    pub(crate) fn load(&self) -> Vec<String> {
        let Some(path) = &self.path else {
            return Vec::new();
        };
        // Read bytes + lossy-decode (not `read_to_string`): a torn/interleaved
        // append can leave invalid UTF-8 in the file, and `read_to_string`
        // would error on it and discard the WHOLE history. Lossy decoding turns
        // only the bad bytes into U+FFFD, so `parse_history` skips just that one
        // line (and the compaction below can then repair the file). This mirrors
        // the shell reader's lossy decode. See `docs/history-persistence.md`.
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let contents = String::from_utf8_lossy(&bytes);
        let mut texts = history::parse_history(&contents);
        let over_hard_cap = texts.len() > HISTORY_COMPACT_AT;
        if texts.len() > HISTORY_MAX_ENTRIES {
            texts.drain(..texts.len() - HISTORY_MAX_ENTRIES);
        }
        if over_hard_cap {
            self.compact(path, &texts);
        }
        texts
    }

    /// Append newly-recorded inputs as JSONL lines in a **single** `O_APPEND`
    /// `write_all` (atomic up to `PIPE_BUF`, so concurrent instances don't
    /// interleave), materializing the parent dir + a `0o600` file on the first
    /// write. Failures are dropped.
    pub(crate) fn append(&self, texts: &[String]) {
        if texts.is_empty() {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        let ts = unix_secs();
        let mut buf = String::new();
        for text in texts {
            // Skip a giant (expanded large-paste) input — bounds the file's
            // bytes, which the entry-count cap can't. It still recalls this
            // session from `entries`.
            if text.len() > HISTORY_MAX_ENTRY_BYTES {
                continue;
            }
            buf.push_str(&history::history_line(&self.session_id, ts, text));
            buf.push('\n');
        }
        if buf.is_empty() {
            return; // every entry was skipped — nothing to write, don't touch the fs
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = append_history_bytes(path, buf.as_bytes());
    }

    /// Rewrite the file to just `texts` (the compaction tail), re-stamped with
    /// this session (the `ts`/`session_id` fields are unused by the app — only
    /// `text` is read back). Best-effort: a failure leaves the oversized file
    /// in place, to be retried next startup.
    fn compact(&self, path: &Path, texts: &[String]) {
        let ts = unix_secs();
        let mut buf = String::new();
        for text in texts {
            buf.push_str(&history::history_line(&self.session_id, ts, text));
            buf.push('\n');
        }
        let _ = write_history_bytes(path, buf.as_bytes());
    }
}

/// Append `bytes` to the history file in one `O_APPEND` write, creating a
/// `0o600` file (the input may hold whatever the user typed — codex writes
/// `history.jsonl` owner-only too).
fn append_history_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

/// Truncate-and-write the history file owner-only (`0o600` on unix) — the
/// compaction rewrite. Tightens the mode on a pre-existing file, since
/// `mode()` only applies at creation (like `write_key_store`).
fn write_history_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(bytes)
}

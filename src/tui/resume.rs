//! Finding recorded sessions on disk: the `/resume` picker's listing and the
//! `--resume {id}` / `--continue` lookups (`docs/resume.md`, `docs/cli.md`).
//!
//! The sessions root is a dated tree of rollout files
//! (`{root}/YYYY/MM/DD/rollout-*.jsonl`), and the only hard problem here is
//! **cost**: a listing must stay cheap however many sessions have piled up.
//! So the scan is layered — a readdir-only walk newest-first
//! ([`RESUME_WALK_CAP`] paths), an mtime sort, then head reads of only the
//! newest [`RESUME_SCAN_CAP`] candidates, each bounded by
//! [`RESUME_HEAD_LINES`]/[`RESUME_HEAD_BYTES`]. Nothing reads a whole file:
//! that happens once, when a session is actually chosen.
//!
//! Eligibility is codex's — a session lists only if its head yields both a
//! meta line and a first user message to preview, so a conversation that was
//! never typed into never shows up.
//!
//! The format and the picker's own display logic are pure and live in the
//! library's `session` module; this is the filesystem half.

use std::path::{Path, PathBuf};

use alter_zero::app::{CHECKPOINT_RESTORED_NOTICE, ToastKind};
use alter_zero::checkpoint;
use alter_zero::cli;
use alter_zero::session::{self, SessionSummary};

use super::Session;

/// How many candidate paths the `/resume` walk will collect before stopping —
/// codex's `MAX_SCAN_FILES` runaway bound (readdir only; cheap).
pub(crate) const RESUME_WALK_CAP: usize = 10_000;

/// How many of the newest-modified candidates get their heads read per
/// `/resume` open — the expensive per-file work (codex pages at 25 with the
/// same 10k scan bound; ours loads one capped page).
pub(crate) const RESUME_SCAN_CAP: usize = 200;

/// How many lines of a rollout file's head the scan reads while hunting for
/// the meta line + first-user-message preview — codex's 10-line head extended
/// by a 200-line user-message hunt.
pub(crate) const RESUME_HEAD_LINES: usize = 210;

/// A byte ceiling on the head read so a pathological no-newline file stays
/// bounded — generous, so a large pasted first message (expanded back to its
/// full text on send) still yields its preview. A line cut at the ceiling
/// fails to parse and is skipped — safe by construction.
pub(crate) const RESUME_HEAD_BYTES: u64 = 2 * 1024 * 1024;

/// Scan the sessions root for resumable rollout files, newest-modified first
/// (codex's Updated sort): walk the `YYYY/MM/DD` date dirs newest-first,
/// head-read each `rollout-*.jsonl` for its meta line and first-user-message
/// preview — files without both never list (codex's eligibility) — and stop
/// considering files past [`RESUME_SCAN_CAP`]. `exclude` is the recorder's
/// active file. Ages are humanized here and frozen (codex freezes its
/// reference when the picker opens).
pub(crate) fn list_sessions(root: Option<&Path>, exclude: Option<&Path>) -> Vec<SessionSummary> {
    let Some(root) = root else {
        return Vec::new();
    };
    let files = rollout_candidates(root);

    // Order by mtime (newest-modified first — a resumed old session floats
    // back to the top, codex's Updated sort) BEFORE capping the expensive
    // head reads, so the cap can't cut a recently-touched old file.
    let mut stamped: Vec<(std::time::SystemTime, PathBuf)> = files
        .into_iter()
        .filter(|path| exclude.is_none_or(|active| active != path))
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, path)
        })
        .collect();
    stamped.sort_by(|a, b| b.0.cmp(&a.0));
    stamped.truncate(RESUME_SCAN_CAP);

    let now = std::time::SystemTime::now();
    let mut sessions = Vec::new();
    for (modified, path) in stamped {
        let Some(head) = read_head(&path, RESUME_HEAD_LINES, RESUME_HEAD_BYTES) else {
            continue;
        };
        // Eligibility (codex's): a parseable meta line AND a user message to
        // preview, both within the head window.
        let Some((meta, items)) = session::parse_session(&head) else {
            continue;
        };
        let Some(preview) = session::preview_of(&items) else {
            continue;
        };
        let updated_secs = now.duration_since(modified).map_or(0, |age| age.as_secs());
        // The Created sort key comes from the meta line's session-start
        // stamp; a foreign/unparseable stamp falls back to the mtime so the
        // row still sorts sanely under either key.
        let created_secs = chrono::DateTime::parse_from_rfc3339(&meta.timestamp)
            .ok()
            .map_or(updated_secs, |created| {
                let age = chrono::Utc::now() - created.with_timezone(&chrono::Utc);
                u64::try_from(age.num_seconds()).unwrap_or(0)
            });
        sessions.push(SessionSummary {
            path,
            updated_secs,
            created_secs,
            cwd: meta.cwd,
            preview,
        });
    }
    sessions
}

/// Candidate rollout paths under `root`, newest-first by the date layout
/// (year desc / month desc / day desc / filename desc — the name embeds the
/// stamp), so the [`RESUME_WALK_CAP`] runaway bound keeps the newest files if
/// it ever bites. Readdir only — no file contents are touched. Shared by the
/// `/resume` listing and the CLI id finder (`docs/cli.md`).
fn rollout_candidates(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    'walk: for year in numeric_dirs_desc(root) {
        for month in numeric_dirs_desc(&year) {
            for day in numeric_dirs_desc(&month) {
                let mut names: Vec<PathBuf> = std::fs::read_dir(&day)
                    .map(|entries| {
                        entries
                            .flatten()
                            .map(|entry| entry.path())
                            .filter(|path| {
                                path.file_name().and_then(|name| name.to_str()).is_some_and(
                                    |name| name.starts_with("rollout-") && name.ends_with(".jsonl"),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                names.sort();
                names.reverse();
                files.extend(names);
                if files.len() >= RESUME_WALK_CAP {
                    files.truncate(RESUME_WALK_CAP);
                    break 'walk;
                }
            }
        }
    }
    files
}

/// Resolve `--resume {id}` to a rollout path (`docs/cli.md`), printing its
/// own failure to stderr (exit 1 — [`resolve_cli`]'s contract). Accepts, in
/// order: an existing **path** (the sessions dir is plain files), an exact
/// rollout **filename**, an exact **id** (the filename segment
/// [`session::rollout_file_id`] extracts — the exit hint prints it), then a
/// **unique id prefix**; several prefix matches are ambiguous, an error
/// rather than a guess.
pub(crate) fn find_session_by_id(root: Option<&Path>, id: &str) -> Result<PathBuf, i32> {
    let as_path = Path::new(id);
    if as_path.is_file() {
        return Ok(as_path.to_path_buf());
    }
    let Some(root) = root else {
        eprintln!("No sessions directory (set HOME or ALTER_ZERO_SESSIONS_DIR)");
        return Err(1);
    };
    let mut exact = Vec::new();
    let mut prefixed = Vec::new();
    for path in rollout_candidates(root) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == id {
            exact.push(path);
            continue;
        }
        let Some(file_id) = session::rollout_file_id(name) else {
            continue;
        };
        if file_id == id {
            exact.push(path);
        } else if file_id.starts_with(id) {
            prefixed.push(path);
        }
    }
    let mut matches = if exact.is_empty() { prefixed } else { exact };
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => {
            eprintln!("No session found for id: {id}");
            eprintln!(
                "Pick one interactively with: {} --resume",
                cli::bin_name(std::env::args().next().as_deref()),
            );
            Err(1)
        }
        n => {
            eprintln!("Ambiguous session id: {id} matches {n} sessions");
            Err(1)
        }
    }
}

/// The sessions root: `ALTER_ZERO_SESSIONS_DIR`, else `{config_home}/sessions`
/// — `~/.alter-zero/sessions` by default, and *inside* a moved
/// `ALTER_ZERO_CONFIG_DIR`, the way the checkpoints root resolves
/// (`config::checkpoints_root`); `None` disables recording (no HOME and no
/// override). Shared by the [`SessionRecorder`] and the CLI resolution in
/// `main` (`docs/cli.md`). It used to fall back to `$HOME` regardless of the
/// config home, so every session the smoke suite recorded behind its throwaway
/// config dir landed in the developer's own `/resume` picker.
pub(crate) fn sessions_root() -> Option<PathBuf> {
    sessions_root_from(
        std::env::var_os("ALTER_ZERO_SESSIONS_DIR").map(PathBuf::from),
        super::config::config_home(),
    )
}

/// The pure half of [`sessions_root`]: the override wins, else the config
/// home's `sessions/`, else nothing.
fn sessions_root_from(
    override_dir: Option<PathBuf>,
    config_home: Option<PathBuf>,
) -> Option<PathBuf> {
    override_dir.or_else(|| config_home.map(|home| home.join("sessions")))
}

/// The numerically-named subdirectories of `dir`, sorted descending — the
/// `YYYY`/`MM`/`DD` walk visits newest first (codex's listing walk).
fn numeric_dirs_desc(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let number: u32 = entry.file_name().to_str()?.parse().ok()?;
                    entry
                        .file_type()
                        .ok()?
                        .is_dir()
                        .then(|| (number, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.into_iter().map(|(_, path)| path).collect()
}

/// Read up to `max_lines` lines of `path`'s head, stopping past the `cap`
/// byte ceiling (so a pathological no-newline file stays bounded). Line-based
/// like codex's head scan, so a long line — a large pasted first message —
/// is read whole; a line cut by the ceiling fails to parse and is skipped by
/// the caller. `None` when the file can't be opened or isn't UTF-8.
fn read_head(path: &Path, max_lines: usize, cap: u64) -> Option<String> {
    use std::io::{BufRead, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file).take(cap);
    let mut head = String::new();
    for _ in 0..max_lines {
        match reader.read_line(&mut head) {
            Ok(0) => break, // EOF (or the ceiling exhausted)
            Ok(_) => {}
            // A ceiling cut inside a multi-byte character makes read_line
            // report InvalidData — keep the (valid) lines already read
            // instead of discarding the whole head, so a session whose
            // preview was already in the buffer still lists.
            Err(_) => break,
        }
    }
    Some(head)
}

// ===== the loop's `/resume` + backtrack arms =====

impl Session<'_> {
    /// `/resume` from an idle composer (`docs/resume.md`): scan the sessions dir
    /// here at the boundary — excluding the file being written — then swap to the
    /// picker on the alternate screen, painted at once (the Ctrl+O
    /// no-black-flash pattern).
    pub(crate) fn open_resume_picker(&mut self) -> std::io::Result<()> {
        let sessions = list_sessions(self.recorder.root(), self.recorder.active_path());
        // The picker's cwd seeds its default Cwd filter — the same display
        // formatting the recorder writes into each session's meta line.
        let cwd = self.cwd.display().to_string();
        self.app.open_resume_picker(sessions, cwd);
        self.term.enter_overlay()?;
        self.draw_resume_picker()
    }

    /// Esc/Ctrl+C dismissed the picker: the view is already back on the
    /// conversation — leave the overlay and catch up, the Ctrl+O return.
    pub(crate) fn close_resume_picker(&mut self) -> std::io::Result<()> {
        self.term.exit_overlay()?;
        self.overlay_return_repaint()
    }

    /// Read the header of every pasted picture the loaded conversation
    /// carries and record its size, so the pictures draw again. A paste has
    /// no fact line to reserve its rows from (`docs/images.md`, *Where the
    /// pixel size comes from*), and since pastes are kept under the config
    /// home rather than `/tmp` the file is usually still there — a path that
    /// isn't simply isn't drawn, as before.
    pub(crate) fn remember_loaded_image_sizes(&self) {
        for item in &self.app.history {
            let alter_zero::app::HistoryItem::Message(message) = item else {
                continue;
            };
            if message.role != alter_zero::app::Role::User {
                continue;
            }
            for path in &message.images {
                if let Some(name) = path.to_str()
                    && let Some(px) = super::workers::image_dimensions(path)
                {
                    alter_zero::images::remember_size(name, px);
                }
            }
        }
    }

    /// Enter on a picker row: read + parse the rollout here (the I/O). Success
    /// swaps the conversation and adopts the file for further recording; failure
    /// leaves the current conversation unharmed under a red notice (codex).
    /// Either way the overlay closes and the inline view catches up.
    pub(crate) fn resume_session(&mut self, path: std::path::PathBuf) -> std::io::Result<()> {
        let loaded = std::fs::read_to_string(&path).ok().and_then(|text| {
            session::parse_session(&text).map(|(meta, items)| (text, meta, items))
        });
        let Some((text, meta, items)) = loaded else {
            self.app.close_resume_picker();
            self.term.exit_overlay()?;
            self.overlay_return_repaint()?;
            self.commit_error_notice(&format!("Failed to load session: {}", path.display()));
            return Ok(());
        };
        let count = items.len();
        // The code-reset side of resume (docs/checkpoint.md): restore the cwd to
        // the session's *final* checkpoint so the files match the transcript
        // being loaded. Back up the current tree first (recoverable via the
        // store's reflog); an unknown commit (a session from a different cwd) or
        // no checkpoints leave the code untouched.
        let session_checkpoints = session::parse_checkpoints(&text);
        let restored = self.restore_final_checkpoint(&session_checkpoints);
        self.app.load_session(items);
        self.remember_loaded_image_sizes();
        // The checklist came back with the conversation (the last task
        // record's snapshot) — the shared registry follows, so the model's
        // next `tasklist` sees the resumed tasks (docs/task-tools.md).
        self.sync_task_registry();
        // A file whose last line lost its newline (a torn write) must not have
        // the next append glued onto it — the recorder prefixes the repair. The
        // parsed checkpoints are adopted too so later turns extend the same chain
        // and a backtrack restores against them.
        let torn = !text.is_empty() && !text.ends_with('\n');
        self.recorder.adopt(
            path,
            meta,
            count,
            torn,
            session_checkpoints,
            self.app.history_generation(),
        );
        // The session boundary for the hooks: SessionStart(resume) fires at
        // the next turn's top (docs/hooks.md).
        self.models.queue_session_source("resume");
        self.term.exit_overlay()?;
        // A resumed session REPLACES the whole conversation: purge-rebuild
        // (like /clear) so the loaded history fills scrollback — a plain
        // catch-up would leave the old chat above it, and the resumed
        // session's earlier turns were never committed in this run. An open
        // agent session view closes: the user picked a conversation, so the
        // main screen is what they land on (the roster keeps its agents). A
        // resize that landed under the picker is consumed by the purge.
        self.overlay_resized = false;
        self.app.close_agent_view();
        self.repaint_conversation()?;
        if restored {
            self.toast(CHECKPOINT_RESTORED_NOTICE, ToastKind::Info);
        }
        Ok(())
    }

    /// Restore the working directory to the latest checkpoint at or before
    /// `history_len` — the Esc-Esc backtrack's code reset, against the chain this
    /// session recorded (`docs/checkpoint.md`). The current tree is snapshotted
    /// first, so the rewind is recoverable through the store's reflog. `false`
    /// when there is nothing to restore to.
    pub(crate) fn restore_checkpoint_at(&self, history_len: usize) -> bool {
        self.restore_to(
            checkpoint::restore_target(self.recorder.checkpoints(), history_len),
            "before backtrack restore",
        )
    }

    /// Restore the working directory to a loaded session's **final** checkpoint —
    /// the `/resume` code reset, against the chain parsed out of that file.
    pub(crate) fn restore_final_checkpoint(&self, checkpoints: &[checkpoint::Checkpoint]) -> bool {
        self.restore_to(
            checkpoint::restore_target(checkpoints, usize::MAX),
            "before resume restore",
        )
    }

    /// Back up the current tree under `label`, then restore `target`. A missing
    /// target, an unknown commit, or a disabled store all leave the working
    /// directory untouched.
    fn restore_to(&self, target: Option<&str>, label: &str) -> bool {
        match target {
            Some(commit) => {
                let commit = commit.to_string();
                let _ = self.checkpoints.snapshot(label);
                self.checkpoints.restore(&commit).unwrap_or(false)
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_root_follows_the_config_home_when_nothing_overrides_it() {
        // A moved `ALTER_ZERO_CONFIG_DIR` takes its rollouts with it, the way
        // the checkpoints root already does: recording into `$HOME/.alter-zero`
        // behind a redirected config home is how the smoke suite's throwaway
        // sessions leaked into the developer's own `/resume` picker.
        let home = PathBuf::from("/cfg-home");
        assert_eq!(
            sessions_root_from(None, Some(home.clone())),
            Some(home.join("sessions"))
        );
    }

    #[test]
    fn sessions_root_override_outranks_the_config_home() {
        assert_eq!(
            sessions_root_from(
                Some(PathBuf::from("/elsewhere")),
                Some(PathBuf::from("/cfg-home"))
            ),
            Some(PathBuf::from("/elsewhere"))
        );
    }

    #[test]
    fn sessions_root_is_none_without_an_override_or_a_config_home() {
        assert_eq!(sessions_root_from(None, None), None);
    }
}

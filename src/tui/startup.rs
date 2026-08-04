//! Resolving the process arguments before the TUI exists (`docs/cli.md`).
//!
//! `--continue` and `--resume {id}` name a conversation to reopen, so they are
//! resolved **here, in cooked mode**: [`resolve_cli`] reads and parses the
//! rollout file itself and hands the loop a [`Startup`] directive that cannot
//! fail. That ordering is the whole point — a bad id, an unreadable file or a
//! `--help` prints to normal stdout/stderr and exits, with no terminal to
//! restore and no TUI flash.
//!
//! The argument grammar is the pure `cli` module's; this is the resolution
//! (which file, does it parse) and the printing.

use std::path::PathBuf;

use alter_zero::app::HistoryItem;
use alter_zero::cli::{self, Cli};
use alter_zero::session::{self, SessionMeta};

use super::resume::{find_session_by_id, list_sessions, sessions_root};

/// The `--continue`/`--resume` directive [`resolve_cli`] hands `run` — the
/// session to install before the first frame, or the `/resume` picker as the
/// first screen (`docs/cli.md`).
pub(crate) enum Startup {
    /// `--continue` / `--resume {id}`: the chosen rollout, read + parsed in
    /// `main` (fail-fast), so the in-TUI path cannot fail. Boxed — the
    /// payload dwarfs `Picker` (clippy's large-enum-variant).
    Load(Box<LoadedSession>),
    /// Bare `--resume`: boot into the `/resume` session picker.
    Picker,
}

/// A rollout file read + parsed ahead of the TUI. `text` rides along for the
/// pieces the picker's `ResumeSession` arm also derives from it (the recorded
/// checkpoints, the torn-tail repair).
pub(crate) struct LoadedSession {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    pub(crate) meta: SessionMeta,
    pub(crate) items: Vec<HistoryItem>,
}

/// Parse the process arguments and resolve them to a [`Startup`] directive
/// (`docs/cli.md`). `Err(code)` means "exit now with this status": usage
/// errors are 2 (message + usage on stderr), resolution failures — nothing to
/// continue, an unknown id, an unreadable file — are 1, and `--help` /
/// `--version` exit 0 after printing. All printing happens here; the TUI
/// never starts on an error path.
pub(crate) fn resolve_cli() -> Result<Option<Startup>, i32> {
    let parsed = cli::parse(std::env::args().skip(1)).map_err(|message| {
        eprintln!("{message}\n\n{}", cli::USAGE);
        2
    })?;
    match parsed {
        Cli::Run => Ok(None),
        Cli::Help => {
            println!("{}", cli::USAGE);
            Err(0)
        }
        Cli::Version => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            Err(0)
        }
        Cli::Resume(None) => Ok(Some(Startup::Picker)),
        Cli::Resume(Some(id)) => {
            let path = find_session_by_id(sessions_root().as_deref(), &id)?;
            Ok(Some(Startup::Load(Box::new(load_rollout(path)?))))
        }
        Cli::Continue => {
            // The newest session recorded in THIS directory — the picker's
            // Cwd-filter rule: the meta line's verbatim `Path::display`
            // string. The listing's eligibility applies too, so a session
            // never typed into doesn't continue.
            let cwd = std::env::current_dir().unwrap_or_default();
            let root = sessions_root();
            let sessions = list_sessions(root.as_deref(), None);
            match session::latest_for_cwd(&sessions, &cwd.display().to_string()) {
                Some(path) => Ok(Some(Startup::Load(Box::new(load_rollout(
                    path.to_path_buf(),
                )?)))),
                None => {
                    eprintln!("No conversation found to continue in {}", cwd.display());
                    Err(1)
                }
            }
        }
    }
}

/// Read + parse one rollout file for [`Startup::Load`], failing fast on
/// stderr (exit 1) — the CLI twin of the picker arm's read, run before the
/// terminal boots so an error never flashes a TUI.
fn load_rollout(path: PathBuf) -> Result<LoadedSession, i32> {
    let text = std::fs::read_to_string(&path).map_err(|err| {
        eprintln!("Failed to read session file {}: {err}", path.display());
        1
    })?;
    let Some((meta, items)) = session::parse_session(&text) else {
        eprintln!("Not a session file: {}", path.display());
        return Err(1);
    };
    Ok(LoadedSession {
        path,
        text,
        meta,
        items,
    })
}

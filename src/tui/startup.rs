//! Resolving the process arguments before the TUI exists (`docs/cli/cli.md`).
//!
//! `--continue` and `--resume {id}` name a conversation to reopen, so they are
//! resolved **here, in cooked mode**: [`resolve_cli`] reads and parses the
//! rollout file itself and hands the loop a [`Startup`] directive that cannot
//! fail. That ordering is the whole point — a bad id, an unreadable file or a
//! `--help` prints to normal stdout/stderr and exits, with no terminal to
//! restore and no TUI flash. The `[PROMPT]` positional rides the directive
//! untouched; the session it goes into is decided here, the turn it starts
//! belongs to `bootstrap` (`Session::submit_startup_prompt`).
//!
//! The argument grammar, the help pages and the colour rule are the pure
//! `cli` module's; this is the resolution (which file, does it parse), the
//! one impure half of the colour decision (is the stream a terminal), and
//! the printing.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Instant;

use alter_zero::app::HistoryItem;
use alter_zero::cli::{self, Cli, HelpPage, HelpStyle, SessionArgs, SessionStart};
use alter_zero::headless;
use alter_zero::session::{self, SessionMeta};

use super::resume::{find_session_by_id, list_sessions, sessions_root};

/// What [`resolve_cli`] hands `run` (`docs/cli/cli.md`): the session to open —
/// or `None` for a fresh one — and the `[PROMPT]` to send into it as its
/// first turn, if one was given.
pub(crate) struct Startup {
    pub(crate) session: Option<StartupSession>,
    pub(crate) prompt: Option<String>,
    /// `--permission-mode`: the run's permission seed, in either kind of run.
    pub(crate) permission_mode: Option<alter_zero::permission::PermissionMode>,
    /// `-p`: run headless, reporting this way (`docs/cli/headless.md`) — the
    /// prompt above then already holds what stdin added. `None` is the TUI.
    pub(crate) print: Option<alter_zero::cli::PrintOptions>,
}

/// The `--continue`/`--resume` directive — the session to install before the
/// first frame, or the `/resume` picker as the first screen.
pub(crate) enum StartupSession {
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
/// (`docs/cli/cli.md`). `Err(code)` means "exit now with this status": usage
/// errors are 2 (the clap-shaped `error:` trailer on stderr), resolution
/// failures — nothing to continue, an unknown id, an unreadable file — are
/// 1, and `--help` / `--version` exit 0 after printing. All printing happens
/// here; the TUI never starts on an error path.
pub(crate) fn resolve_cli() -> Result<Startup, i32> {
    let parsed = cli::parse(std::env::args().skip(1)).map_err(|message| {
        // An `mcp` grammar error gets the subcommand's own usage block
        // (`mcp` only ever routes as the first argument).
        let page = if std::env::args().nth(1).is_some_and(|first| first == "mcp") {
            HelpPage::Mcp
        } else {
            HelpPage::Main
        };
        let style = help_style(std::io::stderr().is_terminal());
        eprintln!("{}", cli::usage_error(page, &message, style));
        2
    })?;
    match parsed {
        Cli::Help => {
            println!("{}", cli::help(HelpPage::Main, stdout_style()));
            Err(0)
        }
        // The mcp subcommand family (docs/cli/mcp-cli.md): does its file work
        // and exits — success rides the same Err(0) channel --help uses.
        Cli::Mcp(cmd) => Err(super::mcp_cli::run(&cmd)),
        // `alter-zero update` (docs/ops/update.md): the same print-and-exit path.
        Cli::Update => Err(super::update_cli::run()),
        Cli::Version => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            Err(0)
        }
        Cli::Session(SessionArgs {
            start,
            prompt,
            cwd,
            permission_mode,
            print,
        }) => {
            let began = Instant::now();
            let (session, prompt) =
                prepare(start, prompt, cwd, print.is_some()).map_err(|halt| match halt {
                    Halt::Usage(message) => {
                        let style = help_style(std::io::stderr().is_terminal());
                        eprintln!("{}", cli::usage_error(HelpPage::Main, message, style));
                        2
                    }
                    Halt::Failed(message) => {
                        match print {
                            // A headless run's failure is its result too: a
                            // script reading JSON meets an error object.
                            Some(options) => super::headless::write(&headless::not_started(
                                options.output_format,
                                &message,
                                began.elapsed(),
                            )),
                            None => eprintln!("{message}"),
                        }
                        1
                    }
                })?;
            Ok(Startup {
                session,
                prompt,
                permission_mode,
                print,
            })
        }
    }
}

/// Why a launch stops before it starts — said once, by [`resolve_cli`].
pub(crate) enum Halt {
    /// Nothing sound to run: the usage block, exit 2.
    Usage(&'static str),
    /// Something to say about it, exit 1 — for a headless run in the form
    /// its output format promises.
    Failed(String),
}

/// The working directory, the conversation and — for a headless run — the
/// prompt, resolved in that order, in cooked mode, before anything starts.
fn prepare(
    start: SessionStart,
    prompt: Option<String>,
    cwd: Option<PathBuf>,
    headless: bool,
) -> Result<(Option<StartupSession>, Option<String>), Halt> {
    // `-C` first: everything after reads the working directory —
    // `--continue` looks for the newest conversation *there*.
    if let Some(dir) = cwd
        && let Err(err) = std::env::set_current_dir(&dir)
    {
        return Err(Halt::Failed(format!(
            "Cannot run in {}: {err}",
            dir.display()
        )));
    }
    let session = resolve_session(start).map_err(Halt::Failed)?;
    // A headless run reads its prompt now, before anything else starts: an
    // argument, stdin, or both (docs/cli/headless.md).
    let prompt = if headless {
        Some(super::headless::read_prompt(prompt.as_deref())?)
    } else {
        prompt
    };
    Ok((session, prompt))
}

/// The style for a page written to a stream: the app's colour when the
/// stream is a terminal and the environment allows it, plain text otherwise
/// — a pipe, a log file, `NO_COLOR`, `TERM=dumb` (`docs/cli/cli.md`).
pub(crate) fn help_style(is_terminal: bool) -> HelpStyle {
    let no_color = std::env::var("NO_COLOR").ok();
    let term = std::env::var("TERM").ok();
    if is_terminal && cli::colour_enabled(no_color.as_deref(), term.as_deref()) {
        HelpStyle::Ansi
    } else {
        HelpStyle::Plain
    }
}

/// [`help_style`] for stdout — the `--help` pages' stream.
pub(crate) fn stdout_style() -> HelpStyle {
    help_style(std::io::stdout().is_terminal())
}

/// Resolve a session flag to the directive: `None` for a fresh session, the
/// picker for a bare `--resume`, else the rollout file — found and parsed
/// here, failing fast on stderr with exit 1.
fn resolve_session(start: SessionStart) -> Result<Option<StartupSession>, String> {
    match start {
        SessionStart::Fresh => Ok(None),
        SessionStart::Resume(None) => Ok(Some(StartupSession::Picker)),
        SessionStart::Resume(Some(id)) => {
            let path = find_session_by_id(sessions_root().as_deref(), &id)?;
            Ok(Some(StartupSession::Load(Box::new(load_rollout(path)?))))
        }
        SessionStart::Continue => {
            // The newest session recorded in THIS directory — the picker's
            // Cwd-filter rule: the meta line's verbatim `Path::display`
            // string. The listing's eligibility applies too, so a session
            // never typed into doesn't continue.
            let cwd = std::env::current_dir().unwrap_or_default();
            let root = sessions_root();
            let sessions = list_sessions(root.as_deref(), None);
            match session::latest_for_cwd(&sessions, &cwd.display().to_string()) {
                Some(path) => Ok(Some(StartupSession::Load(Box::new(load_rollout(
                    path.to_path_buf(),
                )?)))),
                None => Err(format!(
                    "No conversation found to continue in {}",
                    cwd.display()
                )),
            }
        }
    }
}

/// Read + parse one rollout file for [`StartupSession::Load`], failing fast
/// with what went wrong — the CLI twin of the picker arm's read, run before
/// the terminal boots so an error never flashes a TUI.
fn load_rollout(path: PathBuf) -> Result<LoadedSession, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("Failed to read session file {}: {err}", path.display()))?;
    let Some((meta, items)) = session::parse_session(&text) else {
        return Err(format!("Not a session file: {}", path.display()));
    };
    Ok(LoadedSession {
        path,
        text,
        meta,
        items,
    })
}

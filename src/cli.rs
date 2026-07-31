//! Command-line arguments — the pure core of `--continue` / `--resume`
//! (see `docs/cli.md`).
//!
//! Claude-Code-style session flags: `--continue` reopens the newest
//! conversation recorded in the current directory, `--resume {id}` a
//! specific one, bare `--resume` boots into the `/resume` picker — and a
//! quit that recorded anything prints [`resume_hint`]'s copy-paste command.
//!
//! This module owns the parse (`argv` → [`Cli`], with usage errors as
//! `Err(message)`), the [`USAGE`] text, and the exit hint's exact shape.
//! Everything impure — reading `std::env::args`, resolving a flag to a
//! rollout *path* (the sessions-dir scan), printing, exiting — stays at the
//! boundary in `main.rs`, which runs the parse *after* the detached-exec
//! hook (invariant 1: a helper re-exec never parses TUI flags).

/// One parsed invocation. `Run` is the flagless default; the rest map
/// one-to-one onto the flags in [`USAGE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// No arguments: start a fresh session (the pre-CLI behaviour).
    Run,
    /// `--continue` / `-c`: resume the newest session recorded in this cwd.
    Continue,
    /// `--resume [id]` / `-r`: resume the named session — or, bare, open
    /// the `/resume` picker as the first screen.
    Resume(Option<String>),
    /// `--help` / `-h`: print [`USAGE`] and exit.
    Help,
    /// `--version` / `-V`: print the version line and exit.
    Version,
}

/// The `--help` text (and the trailer under a usage error).
pub const USAGE: &str = "\
alter-zero — an inline terminal AI coding agent

Usage: alter-zero [OPTIONS]

Options:
  -c, --continue      Continue the most recent conversation recorded in this
                      directory
  -r, --resume [ID]   Resume a conversation — by the session id the exit hint
                      prints, or picked from a list when no id is given
  -h, --help          Print help
  -V, --version       Print version";

/// Parse the process arguments (without `argv[0]`). Usage errors — unknown
/// flags, stray positionals, a value on `--continue`, more than one session
/// flag — come back as `Err(message)`; the boundary prints the message plus
/// [`USAGE`] to stderr and exits 2. `--help`/`--version` win wherever they
/// appear (so `--resume --help` prints help rather than eating `--help` as
/// an id); `--resume`'s id may be the next argument or `=`-attached, and a
/// following `-`-leading argument is never taken as an id.
pub fn parse<I>(args: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = String>,
{
    let mut session: Option<Cli> = None;
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        // `--flag=value` splits here; a bare flag carries no value.
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let picked = match flag.as_str() {
            "--help" | "-h" => return Ok(Cli::Help),
            "--version" | "-V" => return Ok(Cli::Version),
            "--continue" | "-c" => {
                if value.is_some() {
                    return Err(format!("{flag} takes no value"));
                }
                Cli::Continue
            }
            "--resume" | "-r" => {
                // The id is the attached value, else the next argument —
                // unless that argument is itself a flag. An empty attached
                // value (`--resume=`) is bare.
                let id = value
                    .filter(|value| !value.is_empty())
                    .or_else(|| args.next_if(|next| !next.starts_with('-')));
                Cli::Resume(id)
            }
            _ => return Err(format!("unrecognized argument: {arg}")),
        };
        if session.replace(picked).is_some() {
            return Err("pass at most one of --continue / --resume".to_string());
        }
    }
    Ok(session.unwrap_or(Cli::Run))
}

/// The exit hint printed below the restored terminal after a quit that
/// recorded a conversation (`docs/cli.md`) — exactly two lines, no indent:
///
/// ```text
/// Resume this session with:
/// alter0 --resume 18a9f2c33d41e5b6-1a2b
/// ```
#[must_use]
pub fn resume_hint(bin: &str, session_id: &str) -> String {
    format!("Resume this session with:\n{bin} --resume {session_id}")
}

/// The program name the hint prints — how the user actually invoked us
/// (`argv[0]`'s basename: the crate ships both `alter-zero` and the `alter0`
/// alias, and the hint should echo whichever was run), falling back to the
/// canonical `alter-zero` when `argv[0]` is absent or empty.
#[must_use]
pub fn bin_name(arg0: Option<&str>) -> String {
    arg0.map(std::path::Path::new)
        .and_then(std::path::Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty())
        .map_or_else(|| "alter-zero".to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse` over string literals.
    fn parsed(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(ToString::to_string))
    }

    // ===== parse (docs/cli.md) =====

    #[test]
    fn no_args_is_a_plain_run() {
        assert_eq!(parsed(&[]), Ok(Cli::Run));
    }

    #[test]
    fn continue_parses_long_and_short() {
        assert_eq!(parsed(&["--continue"]), Ok(Cli::Continue));
        assert_eq!(parsed(&["-c"]), Ok(Cli::Continue));
    }

    #[test]
    fn bare_resume_parses_to_the_picker() {
        assert_eq!(parsed(&["--resume"]), Ok(Cli::Resume(None)));
        assert_eq!(parsed(&["-r"]), Ok(Cli::Resume(None)));
        // An empty attached id is bare too — `--resume=` must not go
        // hunting for a session whose id is "".
        assert_eq!(parsed(&["--resume="]), Ok(Cli::Resume(None)));
    }

    #[test]
    fn resume_takes_an_id_separate_attached_or_short() {
        let want = Ok(Cli::Resume(Some("18a9f2c3-1a2b".into())));
        assert_eq!(parsed(&["--resume", "18a9f2c3-1a2b"]), want);
        assert_eq!(parsed(&["--resume=18a9f2c3-1a2b"]), want);
        assert_eq!(parsed(&["-r", "18a9f2c3-1a2b"]), want);
    }

    #[test]
    fn a_following_flag_is_not_eaten_as_a_resume_id() {
        // `--resume --help` asks for help about resume, not for a session
        // literally named "--help".
        assert_eq!(parsed(&["--resume", "--help"]), Ok(Cli::Help));
    }

    #[test]
    fn help_and_version_win_wherever_they_appear() {
        assert_eq!(parsed(&["--help"]), Ok(Cli::Help));
        assert_eq!(parsed(&["-h"]), Ok(Cli::Help));
        assert_eq!(parsed(&["--version"]), Ok(Cli::Version));
        assert_eq!(parsed(&["-V"]), Ok(Cli::Version));
        assert_eq!(parsed(&["--continue", "--help"]), Ok(Cli::Help));
    }

    #[test]
    fn unknown_arguments_are_usage_errors_naming_the_culprit() {
        let err = parsed(&["--frobnicate"]).expect_err("unknown flag");
        assert!(err.contains("--frobnicate"), "{err}");
        let err = parsed(&["hello"]).expect_err("stray positional");
        assert!(err.contains("hello"), "{err}");
        let err = parsed(&["--resume", "id", "extra"]).expect_err("trailing junk");
        assert!(err.contains("extra"), "{err}");
    }

    #[test]
    fn continue_takes_no_value() {
        let err = parsed(&["--continue=now"]).expect_err("a value on --continue");
        assert!(err.contains("--continue"), "{err}");
    }

    #[test]
    fn session_flags_do_not_combine() {
        assert!(parsed(&["--continue", "--resume"]).is_err());
        assert!(parsed(&["-r", "abc", "-c"]).is_err());
        assert!(parsed(&["-c", "-c"]).is_err());
    }

    // ===== the exit hint (docs/cli.md) =====

    #[test]
    fn resume_hint_is_the_two_line_command() {
        assert_eq!(
            resume_hint("alter0", "18a9f2c3-1a2b"),
            "Resume this session with:\nalter0 --resume 18a9f2c3-1a2b",
        );
    }

    #[test]
    fn bin_name_basenames_argv0_and_falls_back() {
        assert_eq!(bin_name(Some("target/debug/alter0")), "alter0");
        assert_eq!(bin_name(Some("/usr/local/bin/alter-zero")), "alter-zero");
        assert_eq!(bin_name(Some("alter0")), "alter0");
        assert_eq!(bin_name(Some("")), "alter-zero");
        assert_eq!(bin_name(None), "alter-zero");
    }
}

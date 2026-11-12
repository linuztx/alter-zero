//! Command-line arguments — the pure core of `--help`, the `[PROMPT]`
//! shortcut, `--continue` and `--resume` (see `docs/cli/cli.md`).
//!
//! Claude-Code-style session flags: `--continue` reopens the newest
//! conversation recorded in the current directory, `--resume {id}` a
//! specific one, bare `--resume` boots into the `/resume` picker, a quoted
//! `[PROMPT]` is sent as the first turn of whichever session those open —
//! and a quit that recorded anything prints [`resume_hint`]'s copy-paste
//! command.
//!
//! This module owns the parse (`argv` → [`Cli`], with usage errors as
//! `Err(message)`), the two `--help` pages ([`help`], rendered plain or in
//! the app's colour by [`HelpStyle`]), the usage-error trailer
//! ([`usage_error`]), the colour rule ([`colour_enabled`]) and the exit
//! hint's exact shape. Everything impure — reading `std::env::args`,
//! asking whether stdout is a terminal, resolving a flag to a rollout
//! *path* (the sessions-dir scan), printing, exiting — stays at the
//! boundary (`tui::startup`), which runs the parse *after* the
//! detached-exec hook (invariant 1: a helper re-exec never parses TUI
//! flags).
//!
//! A first argument of `mcp` routes to the subcommand family instead
//! (`docs/cli/mcp-cli.md`): `mcp add`/`add-json`/`remove`/`get`/`list` manage
//! the **user** MCP config file (`docs/tools/mcp.md`) from a script, answered at
//! the same pre-TUI boundary as the session flags. The grammar and its
//! mapping onto [`McpServerConfig`] live here; the file I/O is the
//! boundary's (`tui::mcp_cli`).

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use unicode_width::UnicodeWidthStr;

use crate::headless::OutputFormat;
use crate::mcp::{self, McpServerConfig};
use crate::permission::PermissionMode;

/// One parsed invocation: a TUI run ([`Cli::Session`] — the flagless
/// default included), one of the two print-and-exit flags, or the `mcp`
/// subcommand family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// Run the TUI: how the session starts, plus the optional `[PROMPT]`
    /// sent as its first turn.
    Session(SessionArgs),
    /// `--help` / `-h`: print the main [`help`] page and exit.
    Help,
    /// `--version` / `-V`: print the version line and exit.
    Version,
    /// `mcp …` as the first argument: manage the user MCP config file
    /// (`docs/cli/mcp-cli.md`).
    Mcp(McpCli),
    /// `update` as the first argument: install the newest release over this
    /// binary when one is out (`docs/ops/update.md`). Takes nothing.
    Update,
}

/// A session run's arguments (`docs/cli/cli.md`): which conversation to open,
/// the message to send into it first — `None` opens on an empty composer —
/// where to run, and whether to run with a screen at all (`-p`,
/// `docs/cli/headless.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionArgs {
    /// How the session starts.
    pub start: SessionStart,
    /// The `[PROMPT]` positional, verbatim: the first turn of a fresh
    /// session, or the next turn of the one `--continue`/`--resume {id}`
    /// reopens. Never blank (the parse refuses one), never combined with
    /// the bare picker (likewise). A headless run without one reads its
    /// prompt from stdin.
    pub prompt: Option<String>,
    /// `-C` / `--cwd`: the directory the run starts in — `chdir`ed to before
    /// anything reads the working directory, `--continue`'s search included.
    pub cwd: Option<PathBuf>,
    /// `--permission-mode`: the run's permission posture — the flag form of
    /// `ALTER_ZERO_PERMISSION_MODE`, outranking it and the project's saved
    /// mode, and never saved (`docs/state/permissions.md`).
    pub permission_mode: Option<PermissionMode>,
    /// `-p` / `--print`: run headless with these options — no screen, the
    /// answer on stdout (`docs/cli/headless.md`). `None` is the TUI.
    pub print: Option<PrintOptions>,
}

/// A headless run's own options (`docs/cli/headless.md`) — the flags that mean
/// nothing with a screen, so the parse refuses them without `-p`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrintOptions {
    /// `--output-format`: how the run reports.
    pub output_format: OutputFormat,
    /// `--max-turns`: the most model replies the lead agent may make over
    /// the whole run — Claude Code's agentic turns. `None` is no limit.
    pub max_turns: Option<NonZeroUsize>,
    /// `--verbose`: log each step of the run to stderr. Without it stderr
    /// says only what went wrong or needs attention.
    pub verbose: bool,
}

/// How a session run begins — the session flags, one-to-one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SessionStart {
    /// No session flag: a fresh conversation.
    #[default]
    Fresh,
    /// `--continue` / `-c`: resume the newest session recorded in this cwd.
    Continue,
    /// `--resume [id]` / `-r`: resume the named session — or, bare, open
    /// the `/resume` picker as the first screen.
    Resume(Option<String>),
}

/// One parsed `mcp` subcommand (`docs/cli/mcp-cli.md`). `add` and `add-json`
/// both resolve to [`McpCli::Add`] here — the JSON path routes through the
/// config module's own entry parser, so there is no second schema to drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCli {
    /// `mcp add <name> --url …` / `mcp add <name> [--] <command> …` /
    /// `mcp add-json <name> <json>`: the validated name and the entry to
    /// record.
    Add {
        name: String,
        config: McpServerConfig,
        /// `--client-secret`: read the entry's OAuth client secret (from
        /// `MCP_CLIENT_SECRET`, a prompt, or stdin) and keep it with the
        /// tokens — never in the file.
        client_secret: bool,
    },
    /// `mcp remove <name>`.
    Remove { name: String },
    /// `mcp get <name>`.
    Get { name: String },
    /// `mcp list`.
    List,
    /// `-h`/`--help` anywhere in the `mcp` arguments (outside a `--`
    /// command): print the [`HelpPage::Mcp`] page and exit 0.
    Help,
}

// ===== the --help pages (docs/cli/cli.md) =====

/// The `--help` page's description: what running the command *does*, in
/// one line — both references' shape (codex's "If no subcommand is
/// specified, options will be forwarded to the interactive CLI", Claude
/// Code's "starts an interactive session by default, use -p/--print for
/// non-interactive output"). A help page is opened to find out how to
/// invoke something, so the line answers that and leaves what the app *is*
/// to the README; the sections below carry the rest. Under 80 columns like
/// every row on the page — nothing is wrapped at render time, so the page
/// reads in the source exactly as it prints.
pub const DESCRIPTION: &str = "\
Starts an interactive session by default; with -p, answers once and exits.";

/// Which `--help` page: the binary's own, or the `mcp` subcommand's
/// (`docs/cli/mcp-cli.md`). Both render through the one [`help`] renderer, so
/// they share a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpPage {
    /// `alter-zero --help`.
    Main,
    /// `alter-zero mcp --help`.
    Mcp,
}

/// How a page is dressed. `Plain` emits no escape sequence at all — what a
/// pipe, a log file and the tests receive; `Ansi` colours the title and the
/// section headings **bold cyan** (the app's accent hue in the terminal's
/// own palette — the banner's cyan, the pickers' selection colour), bolds
/// the literals (`alter-zero`, `mcp`, the flags) and leaves the
/// placeholders (`[PROMPT]`, `<COMMAND>`, `[ID]`) bare, the way clap and
/// cargo dress theirs. The two renderings differ by escapes alone; the
/// boundary picks one per stream with [`colour_enabled`] and `IsTerminal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpStyle {
    Plain,
    Ansi,
}

impl HelpStyle {
    /// A section heading, or the page title.
    fn heading(self, text: &str) -> String {
        self.wrap("1;36", text)
    }

    /// A command word or a flag.
    fn literal(self, text: &str) -> String {
        self.wrap("1", text)
    }

    /// A usage error's `error:` lead.
    fn error(self, text: &str) -> String {
        self.wrap("1;31", text)
    }

    /// `ESC[{sgr}m{text}ESC[0m` under `Ansi`, the bare text under `Plain`.
    /// An empty text emits nothing either way — an empty escape pair is
    /// noise a stripped comparison would still catch.
    fn wrap(self, sgr: &str, text: &str) -> String {
        match self {
            Self::Ansi if !text.is_empty() => format!("\x1b[{sgr}m{text}\x1b[0m"),
            Self::Ansi | Self::Plain => text.to_string(),
        }
    }
}

/// Columns of indent before every section row.
const HELP_INDENT: usize = 2;
/// Columns between a section's widest literal cell and the descriptions.
const HELP_GAP: usize = 3;
/// The continuation indent under `Usage: ` — the label's own width, so
/// every further form aligns with the first.
const USAGE_CONTINUATION: &str = "       ";

/// One help page's content — rendered by [`help`], never printed as-is.
struct HelpDoc {
    /// The first line, in the heading style: the product's name on the
    /// main page (`Alter Zero`, [`crate::APP_NAME`]), the command on the
    /// subcommand's (`alter-zero mcp`).
    title: &'static str,
    /// The paragraph under it, plain and pre-wrapped.
    description: &'static str,
    /// The `Usage:` forms as `(literal, rest)` — the command words bolded,
    /// the placeholders after them bare.
    usage: &'static [(&'static str, &'static str)],
    /// The sections in print order.
    sections: &'static [Section],
}

/// A heading over aligned rows.
struct Section {
    heading: &'static str,
    rows: &'static [Row],
}

/// One `  {literal}{placeholder}   {description}` row. The description's
/// further lines continue at the description column.
struct Row {
    /// The bold part (`-r, --resume`, `mcp`) — empty for a bare positional.
    literal: &'static str,
    /// The bare part beside it (` [ID]`, `[PROMPT]`), leading space included.
    placeholder: &'static str,
    description: &'static str,
}

const MAIN_DOC: HelpDoc = HelpDoc {
    title: crate::APP_NAME,
    description: DESCRIPTION,
    usage: &[
        ("alter-zero", " [OPTIONS] [PROMPT]"),
        ("alter-zero -p", " [OPTIONS] [PROMPT]"),
        ("alter-zero mcp", " <COMMAND>"),
        ("alter-zero update", ""),
    ],
    sections: &[
        Section {
            heading: "Commands:",
            rows: &[
                Row {
                    literal: "mcp",
                    placeholder: "",
                    description: "Manage MCP servers in the user config file —\n\
                                  see alter-zero mcp --help",
                },
                Row {
                    literal: "update",
                    placeholder: "",
                    description: "Install the newest release over this binary,\n\
                                  if one is out (the checksum-verified installer)",
                },
            ],
        },
        Section {
            heading: "Arguments:",
            rows: &[Row {
                literal: "",
                placeholder: "[PROMPT]",
                description: "First message of a new conversation, or the\n\
                              next of the one --continue/--resume reopens",
            }],
        },
        Section {
            heading: "Options:",
            rows: &[
                Row {
                    literal: "-c, --continue",
                    placeholder: "",
                    description: "Continue the most recent conversation recorded\n\
                                  in this directory",
                },
                Row {
                    literal: "-r, --resume",
                    placeholder: " [ID]",
                    description: "Resume a conversation by the id the exit hint\n\
                                  prints — or pick it from a list, given no ID",
                },
                Row {
                    literal: "-C, --cwd",
                    placeholder: " <DIR>",
                    description: "Run in DIR instead of the current directory",
                },
                // A long-only flag lines its `--` up under the others'
                // (clap's four-column pad), so the pad rides the literal.
                Row {
                    literal: "    --permission-mode",
                    placeholder: " <MODE>",
                    description: "manual, edit, auto or master, for this run only",
                },
                Row {
                    literal: "-h, --help",
                    placeholder: "",
                    description: "Print help",
                },
                Row {
                    literal: "-V, --version",
                    placeholder: "",
                    description: "Print version",
                },
            ],
        },
        // The headless run's own flags (docs/cli/headless.md) — a section of
        // their own, since they mean nothing without -p.
        Section {
            heading: "Print mode:",
            rows: &[
                Row {
                    literal: "-p, --print",
                    placeholder: "",
                    description: "Answer PROMPT (or stdin) with no screen: the\n\
                                  answer on stdout, problems on stderr, then exit",
                },
                Row {
                    literal: "    --output-format",
                    placeholder: " <FORMAT>",
                    description: "text (the answer), json (one result object) or\n\
                                  stream-json (one event per line)",
                },
                Row {
                    literal: "    --max-turns",
                    placeholder: " <N>",
                    description: "Stop after N model replies",
                },
                Row {
                    literal: "    --verbose",
                    placeholder: "",
                    description: "Log each step of the run to stderr",
                },
            ],
        },
    ],
};

const MCP_DOC: HelpDoc = HelpDoc {
    title: "alter-zero mcp",
    description: "\
Manage MCP servers in the user config file (ALTER_ZERO_MCP_FILE, else
~/.alter-zero/mcp.json) from a script — inside the app, /mcp manages them live.",
    usage: &[
        (
            "alter-zero mcp add",
            " <name> --url <URL> [--transport http|sse] [-H \"Name: Value\"]...",
        ),
        (
            "alter-zero mcp add",
            " <name> [-e KEY=VALUE]... [--] <command> [args...]",
        ),
        ("alter-zero mcp add-json", " <name> <json>"),
        ("alter-zero mcp remove", " <name>"),
        ("alter-zero mcp get", " <name>"),
        ("alter-zero mcp list", ""),
    ],
    sections: &[Section {
        heading: "Options:",
        rows: &[
            Row {
                literal: "-t, --transport",
                placeholder: " <T>",
                description: "Pin the transport — http or sse with --url, stdio with\n\
                              a command; a bare --url tries streamable HTTP first and\n\
                              falls back to legacy SSE",
            },
            Row {
                literal: "-e, --env",
                placeholder: " KEY=VALUE",
                description: "Environment variable for a stdio server (repeatable)",
            },
            Row {
                literal: "-H, --header",
                placeholder: " <H>",
                description: "\"Name: Value\" header for a remote server (repeatable)",
            },
            Row {
                literal: "--client-id",
                placeholder: " <ID>",
                description: "Pre-registered OAuth client, for a server whose\n\
                              login provider registers none",
            },
            Row {
                literal: "--client-secret",
                placeholder: "",
                description: "Ask for its secret (or read MCP_CLIENT_SECRET) and\n\
                              keep it with the tokens, never in the file",
            },
            Row {
                literal: "--callback-port",
                placeholder: " <N>",
                description: "The port the client's redirect URI names",
            },
            Row {
                literal: "-h, --help",
                placeholder: "",
                description: "Print help",
            },
        ],
    }],
};

impl HelpPage {
    const fn doc(self) -> &'static HelpDoc {
        match self {
            Self::Main => &MAIN_DOC,
            Self::Mcp => &MCP_DOC,
        }
    }
}

/// Render a `--help` page (`docs/cli/cli.md`): the title, the description, the
/// `Usage:` forms (each further one indented to the first), then each
/// section's heading over its rows — two columns of indent, the literal
/// cell padded to the page's widest, three more columns, the description,
/// its further lines continuing at that column. Padding is
/// measured on the plain text, so the columns line up under either style.
/// No trailing newline: the caller's `println!` supplies it.
#[must_use]
pub fn help(page: HelpPage, style: HelpStyle) -> String {
    let doc = page.doc();
    let mut out = style.heading(doc.title);
    out.push_str("\n\n");
    out.push_str(doc.description);
    out.push_str("\n\n");
    out.push_str(&usage_block(doc, style));
    // One description column for the whole page — the widest cell of ANY
    // section — so `mcp`, `[PROMPT]` and the options read as one table.
    let column = HELP_INDENT
        + HELP_GAP
        + doc
            .sections
            .iter()
            .flat_map(|section| section.rows)
            .map(|row| row.literal.width() + row.placeholder.width())
            .max()
            .unwrap_or(0);
    for section in doc.sections {
        out.push_str("\n\n");
        out.push_str(&style.heading(section.heading));
        for row in section.rows {
            let cell_width = HELP_INDENT + row.literal.width() + row.placeholder.width();
            for (i, line) in row.description.lines().enumerate() {
                out.push('\n');
                if i == 0 {
                    out.push_str(&" ".repeat(HELP_INDENT));
                    out.push_str(&style.literal(row.literal));
                    out.push_str(row.placeholder);
                    out.push_str(&" ".repeat(column - cell_width));
                } else {
                    out.push_str(&" ".repeat(column));
                }
                out.push_str(line);
            }
        }
    }
    out
}

/// The `Usage:` block alone — the page's forms, no title or sections.
fn usage_block(doc: &HelpDoc, style: HelpStyle) -> String {
    let mut out = String::new();
    for (i, (literal, rest)) in doc.usage.iter().enumerate() {
        if i == 0 {
            out.push_str(&style.heading("Usage:"));
            out.push(' ');
        } else {
            out.push('\n');
            out.push_str(USAGE_CONTINUATION);
        }
        out.push_str(&style.literal(literal));
        out.push_str(rest);
    }
    out
}

/// The trailer under a grammar error, clap's shape: `error: {message}`, the
/// page's `Usage:` block, and where the rest is. Printed to stderr with
/// exit 2 by the boundary; the `error:` lead is bold red under `Ansi`.
#[must_use]
pub fn usage_error(page: HelpPage, message: &str, style: HelpStyle) -> String {
    format!(
        "{} {message}\n\n{}\n\nFor more information, try '--help'.",
        style.error("error:"),
        usage_block(page.doc(), style)
    )
}

/// Whether a terminal may be coloured, given the two environment variables
/// that say otherwise: `NO_COLOR` set to anything non-empty
/// (<https://no-color.org>) or `TERM=dumb`. The boundary ANDs this with
/// whether the stream is actually a terminal — a pipe gets [`HelpStyle::Plain`]
/// without being asked.
#[must_use]
pub fn colour_enabled(no_color: Option<&str>, term: Option<&str>) -> bool {
    no_color.is_none_or(str::is_empty) && term != Some("dumb")
}

// ===== the argument grammar (docs/cli/cli.md) =====

/// Parse the process arguments (without `argv[0]`). Usage errors — unknown
/// flags, a second positional, a blank prompt, a value on `--continue`,
/// more than one session flag, a prompt with the bare picker, a flag given
/// twice, a headless-only flag without `-p` — come back as `Err(message)`;
/// the boundary prints [`usage_error`] to stderr and exits 2.
/// `--help`/`--version` win wherever they appear before a `--` (so
/// `--resume --help` prints help rather than eating `--help` as an id);
/// `--resume`'s id may be the next argument or `=`-attached, and a
/// following `-`-leading argument is never taken as an id. Every
/// value-taking flag (`--output-format`, `--max-turns`,
/// `--permission-mode`, `-C`/`--cwd`) reads its value the same way. The
/// one positional is the `[PROMPT]`, anywhere among the flags; `--` ends
/// the flags so a dash-leading prompt is reachable.
pub fn parse<I>(args: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = String>,
{
    let mut start: Option<SessionStart> = None;
    let mut prompt: Option<String> = None;
    // The headless switch and its companions (docs/cli/headless.md), each
    // allowed once.
    let mut print: Option<()> = None;
    let mut output_format: Option<OutputFormat> = None;
    let mut max_turns: Option<NonZeroUsize> = None;
    let mut verbose: Option<()> = None;
    let mut permission_mode: Option<PermissionMode> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut args = args.into_iter().peekable();
    // `mcp` routes as the FIRST argument only (docs/cli/mcp-cli.md) — anywhere
    // else it is a prompt like any other word, so the session grammar is
    // untouched.
    if args.peek().is_some_and(|first| first == "mcp") {
        args.next();
        return parse_mcp(args).map(Cli::Mcp);
    }
    // `update` routes the same way (docs/ops/update.md), and takes nothing: a
    // flag or a word after it is a mistake to report, not a prompt to run.
    if args.peek().is_some_and(|first| first == "update") {
        args.next();
        return match args.next() {
            None => Ok(Cli::Update),
            Some(extra) => Err(format!("update takes no arguments (got '{extra}')")),
        };
    }
    while let Some(arg) = args.next() {
        if arg == "--" {
            // The flags end here: what follows is the prompt — and exactly
            // one argument of it.
            for rest in args.by_ref() {
                take_prompt(&mut prompt, rest)?;
            }
            break;
        }
        if !is_flag(&arg) {
            take_prompt(&mut prompt, arg)?;
            continue;
        }
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
                SessionStart::Continue
            }
            "--resume" | "-r" => {
                // The id is the attached value, else the next argument —
                // unless that argument is itself a flag. An explicitly
                // empty attached value (`--resume=`) is bare and never goes
                // hunting for the next argument. Otherwise greedy, like
                // Claude Code's: `--resume "fix the bug"` names a session,
                // not a prompt.
                let id = match value {
                    Some(id) if !id.is_empty() => Some(id),
                    Some(_) => None,
                    None => args.next_if(|next| !next.starts_with('-')),
                };
                SessionStart::Resume(id)
            }
            "--print" | "-p" => {
                if value.is_some() {
                    return Err(format!("{flag} takes no value"));
                }
                set_once(&mut print, (), "--print")?;
                continue;
            }
            "--verbose" => {
                if value.is_some() {
                    return Err(format!("{flag} takes no value"));
                }
                set_once(&mut verbose, (), "--verbose")?;
                continue;
            }
            "--output-format" => {
                let raw = flag_value(&flag, value, &mut args)?;
                let format = OutputFormat::parse(&raw).ok_or_else(|| {
                    format!("invalid --output-format \"{raw}\" (use text, json or stream-json)")
                })?;
                set_once(&mut output_format, format, "--output-format")?;
                continue;
            }
            "--max-turns" => {
                let raw = flag_value(&flag, value, &mut args)?;
                let turns = raw
                    .parse::<usize>()
                    .ok()
                    .and_then(NonZeroUsize::new)
                    .ok_or_else(|| {
                        format!(
                            "--max-turns needs a number of model replies above 0, not \"{raw}\""
                        )
                    })?;
                set_once(&mut max_turns, turns, "--max-turns")?;
                continue;
            }
            "--permission-mode" => {
                let raw = flag_value(&flag, value, &mut args)?;
                let mode = PermissionMode::parse(&raw).ok_or_else(|| {
                    format!(
                        "invalid --permission-mode \"{raw}\" (use manual, edit, auto or master)"
                    )
                })?;
                set_once(&mut permission_mode, mode, "--permission-mode")?;
                continue;
            }
            "--cwd" | "-C" => {
                let dir = flag_value(&flag, value, &mut args)?;
                set_once(&mut cwd, PathBuf::from(dir), "--cwd")?;
                continue;
            }
            _ => return Err(format!("unrecognized argument: {arg}")),
        };
        if start.replace(picked).is_some() {
            return Err("pass at most one of --continue / --resume".to_string());
        }
    }
    let start = start.unwrap_or(SessionStart::Fresh);
    // The headless run's own flags mean nothing with a screen
    // (docs/cli/headless.md): refused rather than silently ignored.
    let print = match print {
        Some(()) => Some(PrintOptions {
            output_format: output_format.unwrap_or_default(),
            max_turns,
            verbose: verbose.is_some(),
        }),
        None => {
            if output_format.is_some() {
                return Err("--output-format needs --print (-p)".to_string());
            }
            if max_turns.is_some() {
                return Err("--max-turns needs --print (-p)".to_string());
            }
            if verbose.is_some() {
                return Err("--verbose needs --print (-p)".to_string());
            }
            None
        }
    };
    // The picker is a screen: a headless run has to name the session it
    // continues (docs/cli/headless.md).
    if print.is_some() && start == SessionStart::Resume(None) {
        return Err(
            "--print can't open the session picker: name the session with --resume <ID>, \
             or use --continue"
                .to_string(),
        );
    }
    // The picker is interactive and dismissable: a prompt parked behind it
    // would either start a fresh conversation on Esc or have to be dropped,
    // so the grammar says no up front (docs/cli/cli.md).
    if prompt.is_some() && start == SessionStart::Resume(None) {
        return Err(
            "--resume needs a session id to send a prompt into (or use --continue)".to_string(),
        );
    }
    Ok(Cli::Session(SessionArgs {
        start,
        prompt,
        cwd,
        permission_mode,
        print,
    }))
}

/// Fill a flag's slot, refusing a second time — clap's rule for a
/// single-value argument. `name` is the flag's long form, whichever spelling
/// was typed.
fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{name} was given more than once"));
    }
    Ok(())
}

/// A flag is a dash plus something; a lone `-` is a positional.
fn is_flag(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1
}

/// Take `arg` as the `[PROMPT]`: the one positional, non-blank. A second
/// one is refused by name with the quoting hint — never joined, since
/// `alter-zero write a -c program` would otherwise read `-c` as
/// `--continue` and lose a word of the prompt (Claude Code errors here too).
fn take_prompt(slot: &mut Option<String>, arg: String) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!(
            "unexpected argument: {arg} (quote the prompt so the shell passes it as one argument)"
        ));
    }
    if arg.trim().is_empty() {
        return Err("the prompt is empty".to_string());
    }
    *slot = Some(arg);
    Ok(())
}

/// Parse the arguments after a leading `mcp` (`docs/cli/mcp-cli.md`). Usage
/// errors come back as `Err(message)` exactly like [`parse`]'s; the boundary
/// prints [`usage_error`]'s trailer for the [`HelpPage::Mcp`] page to stderr
/// and exits 2.
fn parse_mcp<I>(mut args: std::iter::Peekable<I>) -> Result<McpCli, String>
where
    I: Iterator<Item = String>,
{
    let Some(sub) = args.next() else {
        return Err("mcp needs a subcommand: add, add-json, remove, get, list".to_string());
    };
    match sub.as_str() {
        "--help" | "-h" => Ok(McpCli::Help),
        "add" => parse_mcp_add(args),
        "add-json" => {
            let mut client_secret = false;
            let args = args.filter(|arg| {
                let switch = arg == "--client-secret";
                client_secret |= switch;
                !switch
            });
            let Some(mut positionals) = plain_arguments(args)? else {
                return Ok(McpCli::Help);
            };
            if let Some(extra) = positionals.get(2) {
                return Err(format!("unrecognized argument: {extra}"));
            }
            let (Some(name), Some(json)) = (
                (!positionals.is_empty()).then(|| positionals.remove(0)),
                (!positionals.is_empty()).then(|| positionals.remove(0)),
            ) else {
                return Err("mcp add-json needs a server name and its JSON entry".to_string());
            };
            mcp::validate_server_name(&name)?;
            let config =
                mcp::parse_server_entry(&json).map_err(|e| format!("invalid server JSON: {e}"))?;
            if client_secret
                && config
                    .oauth()
                    .and_then(|oauth| oauth.client_id.as_ref())
                    .is_none()
            {
                return Err(
                    "--client-secret needs the entry's oauth.clientId to keep a secret for"
                        .to_string(),
                );
            }
            Ok(McpCli::Add {
                name,
                config,
                client_secret,
            })
        }
        "remove" | "get" => {
            let Some(positionals) = plain_arguments(args)? else {
                return Ok(McpCli::Help);
            };
            if let Some(extra) = positionals.get(1) {
                return Err(format!("unrecognized argument: {extra}"));
            }
            let Some(name) = positionals.into_iter().next() else {
                return Err(format!("mcp {sub} needs a server name"));
            };
            Ok(if sub == "remove" {
                McpCli::Remove { name }
            } else {
                McpCli::Get { name }
            })
        }
        "list" => {
            let Some(positionals) = plain_arguments(args)? else {
                return Ok(McpCli::Help);
            };
            if let Some(extra) = positionals.first() {
                return Err(format!("unrecognized argument: {extra}"));
            }
            Ok(McpCli::List)
        }
        other => Err(format!(
            "unrecognized mcp command: {other} (use add, add-json, remove, get or list)"
        )),
    }
}

/// `mcp add`'s own grammar (`docs/cli/mcp-cli.md`): flags parse until the first
/// non-flag argument after the name — or a `--` — and from there everything
/// belongs to the command **verbatim** (codex's trailing capture), so a
/// child's own `-y`/`--port` flags survive with no separator needed.
fn parse_mcp_add<I>(mut args: std::iter::Peekable<I>) -> Result<McpCli, String>
where
    I: Iterator<Item = String>,
{
    let mut name: Option<String> = None;
    let mut url: Option<String> = None;
    let mut transport: Option<String> = None;
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut command: Vec<String> = Vec::new();
    let mut client_id: Option<String> = None;
    let mut callback_port: Option<u16> = None;
    let mut client_secret = false;
    while let Some(arg) = args.next() {
        if arg == "--" {
            command.extend(args.by_ref());
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            let (flag, value) = match arg.split_once('=') {
                Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            match flag.as_str() {
                "--help" | "-h" => return Ok(McpCli::Help),
                "--url" => url = Some(flag_value(&flag, value, &mut args)?),
                "--transport" | "-t" => {
                    let kind = flag_value(&flag, value, &mut args)?;
                    if !matches!(kind.as_str(), "stdio" | "http" | "sse") {
                        return Err(format!(
                            "invalid transport \"{kind}\" (use stdio, http or sse)"
                        ));
                    }
                    transport = Some(kind);
                }
                "--env" | "-e" => {
                    let (key, val) = parse_env_pair(&flag_value(&flag, value, &mut args)?)?;
                    env.insert(key, val);
                }
                "--header" | "-H" => {
                    let (key, val) = parse_header_pair(&flag_value(&flag, value, &mut args)?)?;
                    headers.insert(key, val);
                }
                "--client-id" => client_id = Some(flag_value(&flag, value, &mut args)?),
                "--callback-port" => {
                    let port = flag_value(&flag, value, &mut args)?;
                    callback_port = Some(
                        port.parse::<u16>()
                            .ok()
                            .filter(|port| *port != 0)
                            .ok_or_else(|| {
                                format!("--callback-port needs a port number, not \"{port}\"")
                            })?,
                    );
                }
                "--client-secret" => {
                    if value.is_some() {
                        return Err("--client-secret takes no value: a secret on the command \
                                    line is never safe — it is read from MCP_CLIENT_SECRET, \
                                    or asked for"
                            .to_string());
                    }
                    client_secret = true;
                }
                _ => return Err(format!("unrecognized mcp add flag: {arg}")),
            }
            continue;
        }
        // A positional: the name first; the next one starts the raw command.
        if name.is_none() {
            name = Some(arg);
        } else {
            command.push(arg);
            command.extend(args.by_ref());
            break;
        }
    }
    let Some(name) = name else {
        return Err("mcp add needs a server name".to_string());
    };
    mcp::validate_server_name(&name)?;
    let mut config = build_add_config(url, transport.as_deref(), env, headers, command)?;
    if client_secret && client_id.is_none() {
        return Err("--client-secret needs --client-id, the client it belongs to".to_string());
    }
    if client_id.is_some() || callback_port.is_some() {
        let oauth = Some(mcp::OAuthClientConfig {
            client_id,
            client_secret: None,
            callback_port,
            scopes: None,
        });
        match &mut config {
            McpServerConfig::Http { oauth: slot, .. }
            | McpServerConfig::Sse { oauth: slot, .. } => {
                *slot = oauth;
            }
            McpServerConfig::Stdio { .. } => {
                return Err(
                    "--client-id and --callback-port are for a remote (http/sse) server"
                        .to_string(),
                );
            }
        }
    }
    Ok(McpCli::Add {
        name,
        config,
        client_secret,
    })
}

/// A value-taking `mcp add` flag's value: the `=`-attached one, else the
/// next argument — unless that argument leads with `-`. Empty is missing
/// (`--url=` must not go hunting for the next token).
fn flag_value<I>(
    flag: &str,
    value: Option<String>,
    args: &mut std::iter::Peekable<I>,
) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    let value = match value {
        Some(value) => value,
        None => args
            .next_if(|next| !next.starts_with('-'))
            .unwrap_or_default(),
    };
    if value.is_empty() {
        return Err(format!("{flag} needs a value"));
    }
    Ok(value)
}

/// `-e KEY=VALUE`: split on the **first** `=` (`A=b=c` keeps `b=c`), the
/// key non-empty, the value free to be empty. One value per flag — the
/// variadic form is the trap that broke Claude Code's own help example.
fn parse_env_pair(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
        _ => Err(format!("invalid --env value \"{raw}\": expected KEY=VALUE")),
    }
}

/// `-H "Name: Value"`: split on the **first** `:` (a bearer token keeps its
/// own colons), both sides trimmed, the name non-empty.
fn parse_header_pair(raw: &str) -> Result<(String, String), String> {
    let err = || format!("invalid --header value \"{raw}\": expected \"Name: Value\"");
    let (name, value) = raw.split_once(':').ok_or_else(err)?;
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty() {
        return Err(err());
    }
    Ok((name.to_string(), value.to_string()))
}

/// Map the collected `mcp add` pieces onto the config entry — the transport
/// comes from the **form** (`--url` vs a command), `--transport` only
/// refines it, and a bare `--url` arms the http→sse fallback (the type-less
/// `{"url": …}` entry, `docs/tools/mcp.md`).
fn build_add_config(
    url: Option<String>,
    transport: Option<&str>,
    env: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    command: Vec<String>,
) -> Result<McpServerConfig, String> {
    match (url, command.is_empty()) {
        (Some(_), false) => Err("pass a --url or a command, not both".to_string()),
        (None, true) => Err("mcp add needs a --url or a command".to_string()),
        (Some(url), true) => {
            if !env.is_empty() {
                return Err("--env only applies to stdio servers".to_string());
            }
            match transport {
                None => Ok(McpServerConfig::Http {
                    url,
                    headers,
                    sse_fallback: true,
                    oauth: None,
                }),
                Some("http") => Ok(McpServerConfig::Http {
                    url,
                    headers,
                    sse_fallback: false,
                    oauth: None,
                }),
                Some("sse") => Ok(McpServerConfig::Sse {
                    url,
                    headers,
                    oauth: None,
                }),
                Some(other) => Err(format!("--transport {other} takes a command, not --url")),
            }
        }
        (None, false) => {
            if !headers.is_empty() {
                return Err("--header only applies to remote (http/sse) servers".to_string());
            }
            match transport {
                None | Some("stdio") => {
                    let mut parts = command.into_iter();
                    let command = parts.next().expect("non-empty checked above");
                    if command.trim().is_empty() {
                        return Err("the command cannot be empty".to_string());
                    }
                    Ok(McpServerConfig::Stdio {
                        command,
                        args: parts.collect(),
                        env,
                    })
                }
                Some(other) => Err(format!("--transport {other} takes --url, not a command")),
            }
        }
    }
}

/// Collect a plain subcommand's positional arguments (`remove`/`get`/
/// `list`/`add-json` take no flags). `Ok(None)` means help was asked.
fn plain_arguments<I>(args: I) -> Result<Option<Vec<String>>, String>
where
    I: Iterator<Item = String>,
{
    let mut positionals = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return Err(format!("unrecognized argument: {arg}"));
            }
            _ => positionals.push(arg),
        }
    }
    Ok(Some(positionals))
}

/// The exit hint printed below the restored terminal after a quit that
/// recorded a conversation (`docs/cli/cli.md`) — exactly two lines, no indent:
///
/// ```text
/// Resume this session with:
/// alter-zero --resume 18a9f2c33d41e5b6-1a2b
/// ```
#[must_use]
pub fn resume_hint(bin: &str, session_id: &str) -> String {
    format!("Resume this session with:\n{bin} --resume {session_id}")
}

/// What a launch of the interface prints when it cannot start: no terminal
/// to open (cron, CI, a pipe), or one that never answered the cursor query
/// (`docs/cli/headless.md`). The cause verbatim, then print mode, which needs no
/// terminal — named, never switched to: a run in which nobody can approve
/// anything is the caller's choice to make.
///
/// ```text
/// error: could not start the interface: No such device or address (os error 6)
/// It needs a terminal; to run without one (cron, CI, a script), use print mode:
/// alter-zero -p "your prompt"
/// ```
#[must_use]
pub fn no_terminal_error(bin: &str, cause: &str) -> String {
    format!(
        "error: could not start the interface: {cause}\n\
         It needs a terminal; to run without one (cron, CI, a script), use print mode:\n\
         {bin} -p \"your prompt\""
    )
}

/// The program name the hint prints — how the user actually invoked us
/// (`argv[0]`'s basename, so a renamed or symlinked install echoes the name
/// that was actually run), falling back to the canonical `alter-zero` when
/// `argv[0]` is absent or empty.
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

    // ===== parse (docs/cli/cli.md) =====

    /// A TUI run: how it starts, plus its optional `[PROMPT]`.
    fn session(start: SessionStart, prompt: Option<&str>) -> Result<Cli, String> {
        Ok(Cli::Session(SessionArgs {
            start,
            prompt: prompt.map(str::to_string),
            ..SessionArgs::default()
        }))
    }

    #[test]
    fn no_args_is_a_plain_run() {
        assert_eq!(parsed(&[]), session(SessionStart::Fresh, None));
    }

    #[test]
    fn continue_parses_long_and_short() {
        assert_eq!(
            parsed(&["--continue"]),
            session(SessionStart::Continue, None)
        );
        assert_eq!(parsed(&["-c"]), session(SessionStart::Continue, None));
    }

    #[test]
    fn bare_resume_parses_to_the_picker() {
        let picker = session(SessionStart::Resume(None), None);
        assert_eq!(parsed(&["--resume"]), picker);
        assert_eq!(parsed(&["-r"]), picker);
        // An empty attached id is bare too — `--resume=` must not go
        // hunting for a session whose id is "".
        assert_eq!(parsed(&["--resume="]), picker);
    }

    #[test]
    fn resume_takes_an_id_separate_attached_or_short() {
        let want = session(SessionStart::Resume(Some("18a9f2c3-1a2b".into())), None);
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
        assert_eq!(parsed(&["a prompt", "--version"]), Ok(Cli::Version));
    }

    #[test]
    fn unknown_flags_are_usage_errors_naming_the_culprit() {
        let err = parsed(&["--frobnicate"]).expect_err("unknown flag");
        assert!(err.contains("--frobnicate"), "{err}");
        let err = parsed(&["-x"]).expect_err("unknown short flag");
        assert!(err.contains("-x"), "{err}");
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

    // ===== the [PROMPT] shortcut (docs/cli/cli.md) =====

    #[test]
    fn a_positional_is_the_prompt_of_a_fresh_session() {
        assert_eq!(
            parsed(&["fix the failing test"]),
            session(SessionStart::Fresh, Some("fix the failing test"))
        );
        // Verbatim: a leading `!` or `/` is message text here, not the
        // composer's shell or slash grammar.
        assert_eq!(
            parsed(&["/init please"]),
            session(SessionStart::Fresh, Some("/init please"))
        );
    }

    #[test]
    fn the_prompt_rides_with_continue_and_resume_before_or_after_the_flag() {
        let id = || SessionStart::Resume(Some("18a9f2c3-1a2b".into()));
        assert_eq!(
            parsed(&["-c", "and the docs"]),
            session(SessionStart::Continue, Some("and the docs"))
        );
        assert_eq!(
            parsed(&["and the docs", "--continue"]),
            session(SessionStart::Continue, Some("and the docs"))
        );
        assert_eq!(
            parsed(&["--resume", "18a9f2c3-1a2b", "and the docs"]),
            session(id(), Some("and the docs"))
        );
        assert_eq!(
            parsed(&["and the docs", "-r", "18a9f2c3-1a2b"]),
            session(id(), Some("and the docs"))
        );
        assert_eq!(
            parsed(&["--resume=18a9f2c3-1a2b", "and the docs"]),
            session(id(), Some("and the docs"))
        );
    }

    #[test]
    fn double_dash_ends_the_flags_so_a_dash_leading_prompt_is_reachable() {
        assert_eq!(
            parsed(&["--", "-v is not a flag here"]),
            session(SessionStart::Fresh, Some("-v is not a flag here"))
        );
        assert_eq!(
            parsed(&["-c", "--", "--continue"]),
            session(SessionStart::Continue, Some("--continue"))
        );
        // Even `--help` is prose after the separator.
        assert_eq!(
            parsed(&["--", "--help"]),
            session(SessionStart::Fresh, Some("--help"))
        );
    }

    #[test]
    fn a_second_positional_is_a_usage_error_with_the_quote_hint() {
        // Never joined: `alter-zero write a -c program` would otherwise
        // read `-c` as --continue and lose a word of the prompt.
        let err = parsed(&["fix", "the bug"]).expect_err("two positionals");
        assert!(err.contains("the bug") && err.contains("quote"), "{err}");
        let err = parsed(&["--", "a", "b"]).expect_err("two after --");
        assert!(err.contains('b') && err.contains("quote"), "{err}");
        let err = parsed(&["a", "-c", "b"]).expect_err("one each side of a flag");
        assert!(err.contains("unexpected argument: b"), "{err}");
        let err = parsed(&["--resume", "id", "extra", "more"]).expect_err("trailing junk");
        assert!(err.contains("more"), "{err}");
    }

    #[test]
    fn a_blank_prompt_is_a_usage_error() {
        let err = parsed(&[""]).expect_err("empty");
        assert!(err.contains("empty"), "{err}");
        let err = parsed(&["   "]).expect_err("blank");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn a_prompt_with_the_bare_picker_is_refused() {
        let err = parsed(&["fix it", "--resume"]).expect_err("picker + prompt");
        assert!(
            err.contains("--resume") && err.contains("session id") && err.contains("--continue"),
            "{err}"
        );
        let err = parsed(&["--resume=", "fix it"]).expect_err("picker + prompt");
        assert!(err.contains("--resume"), "{err}");
    }

    #[test]
    fn resume_still_takes_the_next_argument_as_its_id_greedily() {
        // Claude Code's rule: `--resume "fix the bug"` looks "fix the bug"
        // up as an id (and fails at resolution) rather than guessing it was
        // a prompt — the prompt for a reopened session names the session
        // first.
        assert_eq!(
            parsed(&["--resume", "fix the bug"]),
            session(SessionStart::Resume(Some("fix the bug".into())), None)
        );
    }

    // ===== headless mode (docs/cli/headless.md) =====

    /// A run with these arguments.
    fn run(args: SessionArgs) -> Result<Cli, String> {
        Ok(Cli::Session(args))
    }

    /// A headless run of `prompt` with `options`.
    fn printed(prompt: Option<&str>, options: PrintOptions) -> Result<Cli, String> {
        run(SessionArgs {
            prompt: prompt.map(str::to_string),
            print: Some(options),
            ..SessionArgs::default()
        })
    }

    #[test]
    fn print_runs_the_prompt_headless_in_text_by_default() {
        let want = printed(Some("Reply with OK"), PrintOptions::default());
        assert_eq!(parsed(&["-p", "Reply with OK"]), want);
        assert_eq!(parsed(&["--print", "Reply with OK"]), want);
        // A switch, not a value-taker: the prompt may come first too.
        assert_eq!(parsed(&["Reply with OK", "-p"]), want);
        assert_eq!(
            PrintOptions::default().output_format,
            OutputFormat::Text,
            "text is the default format"
        );
    }

    #[test]
    fn print_without_a_prompt_leaves_it_to_stdin() {
        assert_eq!(parsed(&["-p"]), printed(None, PrintOptions::default()));
        // A lone `-` is a positional the boundary reads as "stdin".
        assert_eq!(
            parsed(&["-p", "-"]),
            printed(Some("-"), PrintOptions::default())
        );
    }

    #[test]
    fn print_takes_no_value() {
        let err = parsed(&["--print=yes", "x"]).expect_err("a value on --print");
        assert!(err.contains("--print") && err.contains("no value"), "{err}");
    }

    #[test]
    fn verbose_turns_on_the_step_log_of_a_print_run() {
        let verbose = printed(
            Some("x"),
            PrintOptions {
                verbose: true,
                ..PrintOptions::default()
            },
        );
        assert_eq!(parsed(&["-p", "--verbose", "x"]), verbose);
        assert_eq!(parsed(&["x", "--verbose", "-p"]), verbose);
        // Quiet unless asked.
        assert_eq!(
            parsed(&["-p", "x"]),
            printed(Some("x"), PrintOptions::default())
        );
        let err = parsed(&["--verbose=yes", "-p", "x"]).expect_err("a value");
        assert!(
            err.contains("--verbose") && err.contains("no value"),
            "{err}"
        );
        let err = parsed(&["-p", "--verbose", "--verbose", "x"]).expect_err("twice");
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn output_format_takes_every_format_separate_or_attached() {
        for format in OutputFormat::ALL {
            let want = printed(
                Some("x"),
                PrintOptions {
                    output_format: format,
                    ..PrintOptions::default()
                },
            );
            assert_eq!(
                parsed(&["-p", "--output-format", format.label(), "x"]),
                want
            );
            let attached = format!("--output-format={}", format.label());
            assert_eq!(parsed(&["x", &attached, "-p"]), want);
        }
    }

    #[test]
    fn an_unknown_output_format_names_the_choices() {
        let err = parsed(&["-p", "--output-format", "xml", "x"]).expect_err("xml");
        assert!(
            err.contains("\"xml\"") && err.contains("text, json or stream-json"),
            "{err}"
        );
    }

    #[test]
    fn a_value_flag_with_no_value_is_a_usage_error() {
        for args in [
            &["-p", "--output-format"][..],
            &["-p", "--output-format="],
            &["-p", "--output-format", "--max-turns", "3"],
            &["-p", "--max-turns"],
            &["--permission-mode"],
            &["-C"],
            &["--cwd="],
        ] {
            let err = parsed(args).expect_err("a missing value");
            assert!(err.contains("needs a value"), "{args:?}: {err}");
        }
    }

    #[test]
    fn max_turns_takes_a_positive_count() {
        let want = |n: usize| {
            printed(
                Some("x"),
                PrintOptions {
                    max_turns: NonZeroUsize::new(n),
                    ..PrintOptions::default()
                },
            )
        };
        assert_eq!(parsed(&["-p", "--max-turns", "5", "x"]), want(5));
        assert_eq!(parsed(&["-p", "--max-turns=12", "x"]), want(12));
    }

    #[test]
    fn max_turns_refuses_zero_and_anything_but_a_count() {
        for value in ["0", "many", "2.5", "-3"] {
            let attached = format!("--max-turns={value}");
            let err = parsed(&["-p", &attached, "x"]).expect_err("not a count");
            assert!(
                err.contains("--max-turns") && err.contains(value),
                "{value}: {err}"
            );
        }
    }

    #[test]
    fn the_print_only_flags_need_print() {
        for args in [
            &["--output-format", "json", "x"][..],
            &["--max-turns", "3", "x"],
            &["-c", "--output-format=stream-json"],
            &["--verbose", "x"],
        ] {
            let err = parsed(args).expect_err("a headless flag without -p");
            assert!(err.contains("needs --print"), "{args:?}: {err}");
        }
    }

    #[test]
    fn permission_mode_seeds_either_kind_of_run() {
        for mode in [
            PermissionMode::Manual,
            PermissionMode::Edit,
            PermissionMode::Auto,
            PermissionMode::Master,
        ] {
            assert_eq!(
                parsed(&["--permission-mode", mode.label()]),
                run(SessionArgs {
                    permission_mode: Some(mode),
                    ..SessionArgs::default()
                })
            );
            let attached = format!("--permission-mode={}", mode.label());
            assert_eq!(
                parsed(&["-p", &attached, "x"]),
                run(SessionArgs {
                    prompt: Some("x".into()),
                    permission_mode: Some(mode),
                    print: Some(PrintOptions::default()),
                    ..SessionArgs::default()
                })
            );
        }
    }

    #[test]
    fn an_unknown_permission_mode_names_the_choices() {
        let err = parsed(&["--permission-mode", "yolo"]).expect_err("yolo");
        assert!(
            err.contains("\"yolo\"") && err.contains("manual, edit, auto or master"),
            "{err}"
        );
    }

    #[test]
    fn cwd_takes_a_directory_short_long_or_attached_in_either_run() {
        let want = |print: Option<PrintOptions>| {
            run(SessionArgs {
                cwd: Some(PathBuf::from("/srv/repo")),
                print,
                ..SessionArgs::default()
            })
        };
        assert_eq!(parsed(&["-C", "/srv/repo"]), want(None));
        assert_eq!(parsed(&["--cwd", "/srv/repo"]), want(None));
        assert_eq!(parsed(&["--cwd=/srv/repo"]), want(None));
        assert_eq!(
            parsed(&["-p", "-C", "/srv/repo"]),
            want(Some(PrintOptions::default()))
        );
        // `-C` is the directory; `-c` is still --continue.
        assert_eq!(
            parsed(&["-c", "-C", "/srv/repo"]),
            run(SessionArgs {
                start: SessionStart::Continue,
                cwd: Some(PathBuf::from("/srv/repo")),
                ..SessionArgs::default()
            })
        );
    }

    #[test]
    fn a_flag_given_twice_is_a_usage_error() {
        for args in [
            &["-p", "-p", "x"][..],
            &["-p", "--print", "x"],
            &["-C", "a", "--cwd", "b"],
            &["--permission-mode", "auto", "--permission-mode=master"],
            &["-p", "--output-format", "json", "--output-format", "text"],
            &["-p", "--max-turns", "1", "--max-turns=2"],
        ] {
            let err = parsed(args).expect_err("twice");
            assert!(err.contains("more than once"), "{args:?}: {err}");
        }
    }

    #[test]
    fn print_with_the_bare_picker_is_refused() {
        for args in [&["-p", "--resume"][..], &["-r", "-p"], &["--resume=", "-p"]] {
            let err = parsed(args).expect_err("the picker headless");
            assert!(
                err.contains("--resume") && err.contains("--continue"),
                "{args:?}: {err}"
            );
        }
    }

    #[test]
    fn print_sends_the_prompt_into_a_continued_or_resumed_session() {
        assert_eq!(
            parsed(&["-p", "-c", "and the docs"]),
            run(SessionArgs {
                start: SessionStart::Continue,
                prompt: Some("and the docs".into()),
                print: Some(PrintOptions::default()),
                ..SessionArgs::default()
            })
        );
        assert_eq!(
            parsed(&["--resume", "18a9f2c3-1a2b", "-p"]),
            run(SessionArgs {
                start: SessionStart::Resume(Some("18a9f2c3-1a2b".into())),
                print: Some(PrintOptions::default()),
                ..SessionArgs::default()
            })
        );
    }

    // ===== the exit hint (docs/cli/cli.md) =====

    #[test]
    fn resume_hint_is_the_two_line_command() {
        assert_eq!(
            resume_hint("alter-zero", "18a9f2c3-1a2b"),
            "Resume this session with:\nalter-zero --resume 18a9f2c3-1a2b",
        );
    }

    #[test]
    fn no_terminal_error_names_the_cause_and_the_mode_that_needs_none() {
        // The interactive launch with no terminal behind it (cron, CI, a
        // pipe) used to end on main's Debug print of the io::Error. The
        // cause stays verbatim, then the way to run without a terminal, its
        // command alone on the last line like the resume hint's.
        assert_eq!(
            no_terminal_error("az", "No such device or address (os error 6)"),
            "error: could not start the interface: No such device or address (os error 6)\n\
             It needs a terminal; to run without one (cron, CI, a script), use print mode:\n\
             az -p \"your prompt\"",
        );
    }

    #[test]
    fn bin_name_basenames_argv0_and_falls_back() {
        assert_eq!(bin_name(Some("target/debug/alter-zero")), "alter-zero");
        assert_eq!(bin_name(Some("/usr/local/bin/alter-zero")), "alter-zero");
        assert_eq!(bin_name(Some("az")), "az");
        assert_eq!(bin_name(Some("")), "alter-zero");
        assert_eq!(bin_name(None), "alter-zero");
    }

    // ===== the --help page (docs/cli/cli.md) =====

    /// The plain main page — exactly what a pipe receives, and what the
    /// design doc shows.
    const MAIN_PAGE: &str = "\
Alter Zero

Starts an interactive session by default; with -p, answers once and exits.

Usage: alter-zero [OPTIONS] [PROMPT]
       alter-zero -p [OPTIONS] [PROMPT]
       alter-zero mcp <COMMAND>
       alter-zero update

Commands:
  mcp                            Manage MCP servers in the user config file —
                                 see alter-zero mcp --help
  update                         Install the newest release over this binary,
                                 if one is out (the checksum-verified installer)

Arguments:
  [PROMPT]                       First message of a new conversation, or the
                                 next of the one --continue/--resume reopens

Options:
  -c, --continue                 Continue the most recent conversation recorded
                                 in this directory
  -r, --resume [ID]              Resume a conversation by the id the exit hint
                                 prints — or pick it from a list, given no ID
  -C, --cwd <DIR>                Run in DIR instead of the current directory
      --permission-mode <MODE>   manual, edit, auto or master, for this run only
  -h, --help                     Print help
  -V, --version                  Print version

Print mode:
  -p, --print                    Answer PROMPT (or stdin) with no screen: the
                                 answer on stdout, problems on stderr, then exit
      --output-format <FORMAT>   text (the answer), json (one result object) or
                                 stream-json (one event per line)
      --max-turns <N>            Stop after N model replies
      --verbose                  Log each step of the run to stderr";

    /// Strip every `ESC[…m` sequence.
    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("\x1b[") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find('m').expect("a closed SGR sequence");
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn the_plain_main_page_is_the_documented_one() {
        assert_eq!(help(HelpPage::Main, HelpStyle::Plain), MAIN_PAGE);
    }

    #[test]
    fn the_main_page_opens_on_the_product_name() {
        // `Alter Zero` — the one constant the app speaks its name from —
        // not the binary's `alter-zero`, which the Usage line names.
        let page = help(HelpPage::Main, HelpStyle::Plain);
        assert_eq!(page.lines().next(), Some(crate::APP_NAME));
        assert!(page.contains("\nUsage: alter-zero "), "{page}");
    }

    #[test]
    fn main_help_fits_eighty_columns_with_no_trailing_blanks() {
        use unicode_width::UnicodeWidthStr;
        for line in help(HelpPage::Main, HelpStyle::Plain).lines() {
            assert!(line.width() <= 80, "{line:?}");
            assert_eq!(line.trim_end(), line, "{line:?}");
        }
    }

    #[test]
    fn ansi_help_strips_back_to_the_plain_page() {
        for page in [HelpPage::Main, HelpPage::Mcp] {
            let styled = help(page, HelpStyle::Ansi);
            let plain = help(page, HelpStyle::Plain);
            assert_ne!(styled, plain, "{page:?} is dressed");
            assert_eq!(strip_ansi(&styled), plain, "{page:?}");
        }
    }

    #[test]
    fn ansi_help_dresses_the_title_headings_and_literals() {
        let styled = help(HelpPage::Main, HelpStyle::Ansi);
        let heading = |text: &str| format!("\x1b[1;36m{text}\x1b[0m");
        // The title wears the heading style — the same colour as Usage /
        // Commands / Options.
        assert!(styled.starts_with(&heading("Alter Zero")), "{styled}");
        for h in ["Usage:", "Commands:", "Arguments:", "Options:"] {
            assert!(
                styled.contains(&heading(h)),
                "{h} styled as a heading:\n{styled}"
            );
        }
        // Literals are bold; placeholders wear nothing.
        assert!(
            styled.contains("\x1b[1malter-zero\x1b[0m [OPTIONS] [PROMPT]"),
            "{styled}"
        );
        assert!(
            styled.contains("\x1b[1malter-zero mcp\x1b[0m <COMMAND>"),
            "{styled}"
        );
        assert!(
            styled.contains("\x1b[1m-r, --resume\x1b[0m [ID]              Resume"),
            "{styled}"
        );
        assert!(
            styled.contains("\n  [PROMPT]                       First"),
            "{styled}"
        );
        // The headless section is a section like any other, and a long-only
        // flag's pad rides inside its literal.
        assert!(styled.contains("\x1b[1;36mPrint mode:\x1b[0m"), "{styled}");
        assert!(
            styled.contains("\x1b[1malter-zero -p\x1b[0m [OPTIONS] [PROMPT]"),
            "{styled}"
        );
        assert!(
            styled.contains("\x1b[1m    --max-turns\x1b[0m <N>"),
            "{styled}"
        );
        assert!(!styled.contains("\x1b[1m[PROMPT]"), "{styled}");
        // An empty literal never emits an empty escape pair.
        assert!(!styled.contains("\x1b[1m\x1b[0m"), "{styled}");
    }

    #[test]
    fn the_mcp_page_is_titled_by_its_command_over_the_six_usage_lines() {
        let page = help(HelpPage::Mcp, HelpStyle::Plain);
        assert_eq!(page.lines().next(), Some("alter-zero mcp"));
        assert!(
            page.contains("\nUsage: alter-zero mcp add <name> --url <URL> [--transport http|sse]"),
            "{page}"
        );
        assert_eq!(
            page.matches("\n       alter-zero mcp ").count(),
            5,
            "{page}"
        );
        assert!(page.contains("\n       alter-zero mcp list\n"), "{page}");
        // The options column is the widest cell plus the gap.
        assert!(
            page.contains("\nOptions:\n  -t, --transport <T>   Pin the transport"),
            "{page}"
        );
        assert!(
            page.contains("\n  -e, --env KEY=VALUE   Environment"),
            "{page}"
        );
        assert!(page.contains("ALTER_ZERO_MCP_FILE"), "{page}");
        let styled = help(HelpPage::Mcp, HelpStyle::Ansi);
        assert!(
            styled.starts_with("\x1b[1;36malter-zero mcp\x1b[0m\n"),
            "{styled}"
        );
        assert!(
            styled.contains("\x1b[1malter-zero mcp add-json\x1b[0m <name>"),
            "{styled}"
        );
    }

    #[test]
    fn a_usage_error_is_the_error_line_the_usage_block_and_the_help_pointer() {
        let text = usage_error(
            HelpPage::Main,
            "unrecognized argument: --frob",
            HelpStyle::Plain,
        );
        assert_eq!(
            text,
            "error: unrecognized argument: --frob\n\n\
             Usage: alter-zero [OPTIONS] [PROMPT]\n       alter-zero -p [OPTIONS] [PROMPT]\n       \
             alter-zero mcp <COMMAND>\n       alter-zero update\n\n\
             For more information, try '--help'."
        );
        let styled = usage_error(HelpPage::Main, "x", HelpStyle::Ansi);
        assert!(
            styled.starts_with("\x1b[1;31merror:\x1b[0m x\n"),
            "{styled}"
        );
        assert_eq!(
            strip_ansi(&styled),
            usage_error(HelpPage::Main, "x", HelpStyle::Plain)
        );
        let mcp = usage_error(
            HelpPage::Mcp,
            "mcp add needs a server name",
            HelpStyle::Plain,
        );
        assert!(
            mcp.starts_with("error: mcp add needs a server name\n\nUsage: alter-zero mcp add "),
            "{mcp}"
        );
        assert!(
            mcp.ends_with("       alter-zero mcp list\n\nFor more information, try '--help'."),
            "{mcp}"
        );
    }

    #[test]
    fn colour_is_on_unless_no_color_is_set_or_the_terminal_is_dumb() {
        assert!(colour_enabled(None, Some("xterm-256color")));
        assert!(colour_enabled(None, None));
        // https://no-color.org: only a NON-empty NO_COLOR disables.
        assert!(colour_enabled(Some(""), Some("xterm")));
        assert!(!colour_enabled(Some("1"), Some("xterm")));
        assert!(!colour_enabled(None, Some("dumb")));
    }

    // ===== the update subcommand (docs/ops/update.md) =====

    #[test]
    fn update_routes_as_the_first_argument_only_and_takes_nothing() {
        assert_eq!(parsed(&["update"]), Ok(Cli::Update));
        let err = parsed(&["update", "now"]).unwrap_err();
        assert!(err.contains("update takes no arguments"), "{err}");
        let err = parsed(&["update", "--help"]).unwrap_err();
        assert!(err.contains("update takes no arguments"), "{err}");
        // Anywhere else the word is a prompt like any other.
        assert!(matches!(
            parsed(&["-c", "update"]),
            Ok(Cli::Session(SessionArgs { prompt: Some(p), .. })) if p == "update"
        ));
    }

    // ===== the mcp subcommand (docs/cli/mcp-cli.md) =====

    use crate::mcp::McpServerConfig;
    use std::collections::BTreeMap;

    /// `parse` expecting the `mcp` route.
    fn parsed_mcp(args: &[&str]) -> Result<McpCli, String> {
        match parsed(args) {
            Ok(Cli::Mcp(cmd)) => Ok(cmd),
            Ok(other) => panic!("expected an mcp command, got {other:?}"),
            Err(err) => Err(err),
        }
    }

    /// `mcp add` takes a pre-registered OAuth client the way other MCP
    /// clients' CLIs do — `--client-id`, `--callback-port`, and
    /// `--client-secret` as a switch, since a secret on the command line
    /// lands in the shell history and the process list (`docs/cli/mcp-cli.md`).
    #[test]
    fn mcp_add_takes_a_preregistered_oauth_client() {
        let Ok(McpCli::Add {
            name,
            config,
            client_secret,
        }) = parsed_mcp(&[
            "mcp",
            "add",
            "calendar",
            "--url",
            "https://calendarmcp.googleapis.com/mcp/v1",
            "--client-id",
            "123.apps.googleusercontent.com",
            "--callback-port",
            "8976",
            "--client-secret",
        ])
        else {
            panic!("an add");
        };
        assert_eq!(name, "calendar");
        assert!(client_secret);
        let oauth = config.oauth().expect("the client");
        assert_eq!(
            oauth.client_id.as_deref(),
            Some("123.apps.googleusercontent.com")
        );
        assert_eq!(oauth.callback_port, Some(8976));
        assert_eq!(oauth.client_secret, None, "never written to the file");
        for (args, needle) in [
            (
                &[
                    "mcp",
                    "add",
                    "x",
                    "--url",
                    "https://x/mcp",
                    "--client-secret=s",
                ][..],
                "never",
            ),
            (
                &[
                    "mcp",
                    "add",
                    "x",
                    "--url",
                    "https://x/mcp",
                    "--callback-port",
                    "0",
                ],
                "--callback-port",
            ),
            (
                &[
                    "mcp",
                    "add",
                    "x",
                    "--url",
                    "https://x/mcp",
                    "--client-secret",
                ],
                "--client-id",
            ),
            (
                &["mcp", "add", "x", "--client-id", "c", "--", "srv"],
                "remote",
            ),
        ] {
            let error = parsed_mcp(args).expect_err("refused");
            assert!(error.contains(needle), "{args:?}: {error}");
        }
        // `add-json` names its client in the entry, and takes the switch.
        let Ok(McpCli::Add { client_secret, .. }) = parsed_mcp(&[
            "mcp",
            "add-json",
            "cal",
            r#"{"url":"https://x/mcp","oauth":{"clientId":"c"}}"#,
            "--client-secret",
        ]) else {
            panic!("an add");
        };
        assert!(client_secret);
        let error = parsed_mcp(&[
            "mcp",
            "add-json",
            "cal",
            r#"{"url":"https://x/mcp"}"#,
            "--client-secret",
        ])
        .expect_err("no client to keep a secret for");
        assert!(error.contains("clientId"), "{error}");
    }

    fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn mcp_add_with_a_bare_url_is_http_with_the_sse_fallback() {
        // The default remote add writes the type-less `{"url": …}` shape —
        // try streamable HTTP, retry as legacy SSE (docs/tools/mcp.md).
        let want = Ok(McpCli::Add {
            name: "vercel".to_string(),
            config: McpServerConfig::Http {
                url: "https://mcp.vercel.com".to_string(),
                headers: BTreeMap::new(),
                sse_fallback: true,
                oauth: None,
            },
            client_secret: false,
        });
        assert_eq!(
            parsed_mcp(&["mcp", "add", "vercel", "--url", "https://mcp.vercel.com"]),
            want
        );
        // Attached value and flags-before-name both parse.
        assert_eq!(
            parsed_mcp(&["mcp", "add", "vercel", "--url=https://mcp.vercel.com"]),
            want
        );
        assert_eq!(
            parsed_mcp(&["mcp", "add", "--url", "https://mcp.vercel.com", "vercel"]),
            want
        );
    }

    #[test]
    fn mcp_add_transport_pins_the_remote_kind() {
        let http = parsed_mcp(&[
            "mcp",
            "add",
            "vercel",
            "--transport",
            "http",
            "--url",
            "https://x",
        ]);
        assert_eq!(
            http,
            Ok(McpCli::Add {
                name: "vercel".to_string(),
                config: McpServerConfig::Http {
                    url: "https://x".to_string(),
                    headers: BTreeMap::new(),
                    sse_fallback: false,
                    oauth: None,
                },
                client_secret: false,
            })
        );
        let sse = parsed_mcp(&["mcp", "add", "old", "-t", "sse", "--url", "https://x/sse"]);
        assert_eq!(
            sse,
            Ok(McpCli::Add {
                name: "old".to_string(),
                config: McpServerConfig::Sse {
                    url: "https://x/sse".to_string(),
                    headers: BTreeMap::new(),
                    oauth: None,
                },
                client_secret: false,
            })
        );
        // `--transport stdio` is the explicit spelling of the command form.
        let stdio = parsed_mcp(&["mcp", "add", "docs", "--transport", "stdio", "--", "server"]);
        assert_eq!(
            stdio,
            Ok(McpCli::Add {
                name: "docs".to_string(),
                config: McpServerConfig::Stdio {
                    command: "server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
                client_secret: false,
            })
        );
        let err = parsed_mcp(&["mcp", "add", "x", "-t", "ws", "--url", "https://x"])
            .expect_err("bad transport");
        assert!(
            err.contains("ws") && err.contains("stdio, http or sse"),
            "{err}"
        );
    }

    #[test]
    fn mcp_add_captures_the_command_raw_from_its_first_token() {
        // The child's own flags survive with no `--` needed (codex's
        // trailing capture; Claude Code errors with `unknown option` here).
        let got = parsed_mcp(&["mcp", "add", "docs", "npx", "-y", "pkg", "--port", "4000"]);
        assert_eq!(
            got,
            Ok(McpCli::Add {
                name: "docs".to_string(),
                config: McpServerConfig::Stdio {
                    command: "npx".to_string(),
                    args: ["-y", "pkg", "--port", "4000"]
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    env: BTreeMap::new(),
                },
                client_secret: false,
            })
        );
        // `--` starts the command outright — needed when its first word
        // leads with a dash, and `--help` after it belongs to the child.
        let dashed = parsed_mcp(&["mcp", "add", "x", "--", "-weird", "--help"]);
        assert_eq!(
            dashed,
            Ok(McpCli::Add {
                name: "x".to_string(),
                config: McpServerConfig::Stdio {
                    command: "-weird".to_string(),
                    args: vec!["--help".to_string()],
                    env: BTreeMap::new(),
                },
                client_secret: false,
            })
        );
    }

    #[test]
    fn mcp_add_env_and_headers_parse_and_repeat() {
        let got = parsed_mcp(&[
            "mcp", "add", "srv", "-e", "A=1", "--env", "B=x=y", "--env", "C=", "--", "cmd",
        ]);
        assert_eq!(
            got,
            Ok(McpCli::Add {
                name: "srv".to_string(),
                config: McpServerConfig::Stdio {
                    command: "cmd".to_string(),
                    args: Vec::new(),
                    // First `=` splits; empty values are legal.
                    env: pairs(&[("A", "1"), ("B", "x=y"), ("C", "")]),
                },
                client_secret: false,
            })
        );
        let got = parsed_mcp(&[
            "mcp",
            "add",
            "api",
            "--url",
            "https://x",
            "-H",
            "Authorization: Bearer a:b",
            "--header",
            " X-Custom :v ",
        ]);
        assert_eq!(
            got,
            Ok(McpCli::Add {
                name: "api".to_string(),
                config: McpServerConfig::Http {
                    url: "https://x".to_string(),
                    // First `:` splits, both sides trimmed.
                    headers: pairs(&[("Authorization", "Bearer a:b"), ("X-Custom", "v")]),
                    sse_fallback: true,
                    oauth: None,
                },
                client_secret: false,
            })
        );
        let err = parsed_mcp(&["mcp", "add", "x", "-e", "NOEQ", "--", "c"]).expect_err("bad env");
        assert!(err.contains("NOEQ") && err.contains("KEY=VALUE"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "x", "--url", "u", "-H", "nocolon"])
            .expect_err("bad header");
        assert!(
            err.contains("nocolon") && err.contains("Name: Value"),
            "{err}"
        );
        let err =
            parsed_mcp(&["mcp", "add", "x", "--url", "u", "-H", ": v"]).expect_err("empty name");
        assert!(err.contains(": v"), "{err}");
    }

    #[test]
    fn mcp_add_rejects_conflicting_forms() {
        let err =
            parsed_mcp(&["mcp", "add", "x", "--url", "https://u", "--", "cmd"]).expect_err("both");
        assert!(err.contains("not both"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "x"]).expect_err("neither");
        assert!(err.contains("--url") && err.contains("command"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "x", "--url", "u", "-e", "A=1"]).expect_err("env+url");
        assert!(err.contains("--env") && err.contains("stdio"), "{err}");
        let err =
            parsed_mcp(&["mcp", "add", "x", "-H", "A: b", "--", "cmd"]).expect_err("header+cmd");
        assert!(err.contains("--header") && err.contains("remote"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "x", "-t", "stdio", "--url", "u"])
            .expect_err("stdio with url");
        assert!(err.contains("--transport stdio"), "{err}");
        let err =
            parsed_mcp(&["mcp", "add", "x", "-t", "http", "--", "cmd"]).expect_err("http with cmd");
        assert!(err.contains("--transport http"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "--url", "u"]).expect_err("no name");
        assert!(err.contains("server name"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "my server", "--url", "u"]).expect_err("bad name");
        assert!(err.contains("my server"), "{err}");
        let err = parsed_mcp(&["mcp", "add", "x", "--frob", "--url", "u"]).expect_err("bad flag");
        assert!(err.contains("--frob"), "{err}");
    }

    #[test]
    fn mcp_add_json_routes_through_the_one_entry_schema() {
        let got = parsed_mcp(&[
            "mcp",
            "add-json",
            "wiki",
            r#"{"type": "http", "url": "https://x/mcp"}"#,
        ]);
        assert_eq!(
            got,
            Ok(McpCli::Add {
                name: "wiki".to_string(),
                config: McpServerConfig::Http {
                    url: "https://x/mcp".to_string(),
                    headers: BTreeMap::new(),
                    sse_fallback: false,
                    oauth: None,
                },
                client_secret: false,
            })
        );
        let err = parsed_mcp(&["mcp", "add-json", "wiki", "{not json"]).expect_err("bad json");
        assert!(err.contains("not valid JSON"), "{err}");
        let err = parsed_mcp(&["mcp", "add-json", "wiki"]).expect_err("missing json");
        assert!(err.contains("JSON"), "{err}");
    }

    #[test]
    fn mcp_remove_get_and_list_parse() {
        assert_eq!(
            parsed_mcp(&["mcp", "remove", "wiki"]),
            Ok(McpCli::Remove {
                name: "wiki".to_string()
            })
        );
        assert_eq!(
            parsed_mcp(&["mcp", "get", "wiki"]),
            Ok(McpCli::Get {
                name: "wiki".to_string()
            })
        );
        assert_eq!(parsed_mcp(&["mcp", "list"]), Ok(McpCli::List));
        let err = parsed_mcp(&["mcp", "remove"]).expect_err("missing name");
        assert!(err.contains("server name"), "{err}");
        let err = parsed_mcp(&["mcp", "list", "extra"]).expect_err("stray");
        assert!(err.contains("extra"), "{err}");
        let err = parsed_mcp(&["mcp", "get", "a", "b"]).expect_err("stray");
        assert!(err.contains("b"), "{err}");
    }

    #[test]
    fn mcp_help_and_subcommand_errors() {
        assert_eq!(parsed_mcp(&["mcp", "--help"]), Ok(McpCli::Help));
        assert_eq!(parsed_mcp(&["mcp", "-h"]), Ok(McpCli::Help));
        assert_eq!(parsed_mcp(&["mcp", "add", "--help"]), Ok(McpCli::Help));
        assert_eq!(parsed_mcp(&["mcp", "list", "-h"]), Ok(McpCli::Help));
        let err = parsed_mcp(&["mcp"]).expect_err("bare mcp");
        assert!(err.contains("add") && err.contains("list"), "{err}");
        let err = parsed_mcp(&["mcp", "frobnicate"]).expect_err("unknown sub");
        assert!(err.contains("frobnicate"), "{err}");
    }

    #[test]
    fn mcp_is_a_first_argument_only() {
        // Anywhere else it is a word of the prompt like any other.
        assert_eq!(
            parsed(&["--continue", "mcp"]),
            session(SessionStart::Continue, Some("mcp"))
        );
    }
}

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
//!
//! A first argument of `mcp` routes to the subcommand family instead
//! (`docs/mcp-cli.md`): `mcp add`/`add-json`/`remove`/`get`/`list` manage
//! the **user** MCP config file (`docs/mcp.md`) from a script, answered at
//! the same pre-TUI boundary as the session flags. The grammar and its
//! mapping onto [`McpServerConfig`] live here; the file I/O is the
//! boundary's (`tui::mcp_cli`).

use std::collections::BTreeMap;

use crate::mcp::{self, McpServerConfig};

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
    /// `mcp …` as the first argument: manage the user MCP config file
    /// (`docs/mcp-cli.md`).
    Mcp(McpCli),
}

/// One parsed `mcp` subcommand (`docs/mcp-cli.md`). `add` and `add-json`
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
    },
    /// `mcp remove <name>`.
    Remove { name: String },
    /// `mcp get <name>`.
    Get { name: String },
    /// `mcp list`.
    List,
    /// `-h`/`--help` anywhere in the `mcp` arguments (outside a `--`
    /// command): print [`MCP_USAGE`] and exit 0.
    Help,
}

/// The `--help` text (and the trailer under a usage error).
pub const USAGE: &str = "\
alter-zero — an inline terminal AI coding agent

Usage: alter-zero [OPTIONS]
       alter-zero mcp <COMMAND>

Commands:
  mcp                 Manage MCP servers in the user config file — see
                      alter-zero mcp --help

Options:
  -c, --continue      Continue the most recent conversation recorded in this
                      directory
  -r, --resume [ID]   Resume a conversation — by the session id the exit hint
                      prints, or picked from a list when no id is given
  -h, --help          Print help
  -V, --version       Print version";

/// The `mcp --help` text (and the trailer under an `mcp` usage error) —
/// `docs/mcp-cli.md`.
pub const MCP_USAGE: &str = "\
alter-zero mcp — manage MCP servers in the user config file

Usage:
  alter-zero mcp add <name> --url <URL> [--transport http|sse] [-H \"Name: Value\"]...
  alter-zero mcp add <name> [-e KEY=VALUE]... [--] <command> [args...]
  alter-zero mcp add-json <name> <json>
  alter-zero mcp remove <name>
  alter-zero mcp get <name>
  alter-zero mcp list

Options:
  -t, --transport <T>  Pin the transport — http or sse with --url, stdio with
                       a command; a bare --url tries streamable HTTP first and
                       falls back to legacy SSE
  -e, --env KEY=VALUE  Environment variable for a stdio server (repeatable)
  -H, --header <H>     \"Name: Value\" header for a remote server (repeatable)
  -h, --help           Print help

The servers land in the user MCP config (ALTER_ZERO_MCP_FILE, else
~/.alter-zero/mcp.json); manage them live with /mcp inside the app.";

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
    // `mcp` routes as the FIRST argument only (docs/mcp-cli.md) — anywhere
    // else it stays the stray positional it always was, so the session-flag
    // grammar is untouched.
    if args.peek().is_some_and(|first| first == "mcp") {
        args.next();
        return parse_mcp(args).map(Cli::Mcp);
    }
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

/// Parse the arguments after a leading `mcp` (`docs/mcp-cli.md`). Usage
/// errors come back as `Err(message)` exactly like [`parse`]'s; the boundary
/// prints the message plus [`MCP_USAGE`] to stderr and exits 2.
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
            Ok(McpCli::Add { name, config })
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

/// `mcp add`'s own grammar (`docs/mcp-cli.md`): flags parse until the first
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
    let config = build_add_config(url, transport.as_deref(), env, headers, command)?;
    Ok(McpCli::Add { name, config })
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
/// `{"url": …}` entry, `docs/mcp.md`).
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
                }),
                Some("http") => Ok(McpServerConfig::Http {
                    url,
                    headers,
                    sse_fallback: false,
                }),
                Some("sse") => Ok(McpServerConfig::Sse { url, headers }),
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

    // ===== the mcp subcommand (docs/mcp-cli.md) =====

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

    fn pairs(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn mcp_add_with_a_bare_url_is_http_with_the_sse_fallback() {
        // The default remote add writes the type-less `{"url": …}` shape —
        // try streamable HTTP, retry as legacy SSE (docs/mcp.md).
        let want = Ok(McpCli::Add {
            name: "vercel".to_string(),
            config: McpServerConfig::Http {
                url: "https://mcp.vercel.com".to_string(),
                headers: BTreeMap::new(),
                sse_fallback: true,
            },
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
                },
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
                },
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
                },
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
                },
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
        // Anywhere else it stays the stray positional it always was.
        let err = parsed(&["--continue", "mcp"]).expect_err("not a subcommand here");
        assert!(err.contains("mcp"), "{err}");
    }
}

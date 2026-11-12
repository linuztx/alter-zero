//! The `mcp` subcommand boundary (`docs/cli/mcp-cli.md`): the file I/O and
//! printing behind `alter-zero mcp add/add-json/remove/get/list`, run by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like every other CLI resolution (`docs/cli/cli.md`).
//!
//! The grammar and its mapping onto [`McpServerConfig`] are the pure
//! `cli`/`mcp` modules'; this file only resolves the user config path, does
//! the read-modify-write around the pure writers, and maps each outcome to
//! its message and exit code. Unlike the TUI's best-effort writers
//! (`docs/tools/mcp.md`), every failure here is loud: a CLI that could not
//! persist must say so and exit non-zero.

use std::path::{Path, PathBuf};

use alter_zero::cli::{self, HelpPage, McpCli};
use alter_zero::mcp::{self, McpServerConfig, McpWriteError};

use super::config;
use super::startup::stdout_style;

/// Run one `mcp` subcommand and return the process exit code: `0` success,
/// `1` resolution failures (unknown name, unparseable file, nowhere to
/// persist, a failed write). Grammar errors never reach here — `parse`
/// reports them and `resolve_cli` exits 2.
pub(crate) fn run(cmd: &McpCli) -> i32 {
    match cmd {
        McpCli::Help => {
            // The subcommand's page, through the same renderer and colour
            // rule as the main `--help` (docs/cli/cli.md).
            println!("{}", cli::help(HelpPage::Mcp, stdout_style()));
            0
        }
        McpCli::Add {
            name,
            config,
            client_secret,
        } => {
            // The secret first, so a missing one never leaves the server
            // half-added.
            let secret = if *client_secret {
                match read_client_secret() {
                    Ok(secret) => Some(secret),
                    Err(message) => {
                        eprintln!("{message}");
                        return 1;
                    }
                }
            } else {
                None
            };
            with_user_file(
                |path, contents| match mcp::record_server(&contents, name, config) {
                    Ok(updated) => {
                        write_user_file(path, &updated)?;
                        println!(
                            "Added {} MCP server \"{name}\" ({})",
                            config.transport_label(),
                            config.target()
                        );
                        println!("File: {}", path.display());
                        if let Some(secret) = &secret {
                            let store = config::mcp_auth_path();
                            alter_zero::llm::mcp::save_client_secret(
                                store.as_deref(),
                                &resolved_url(config),
                                Some(secret),
                            )
                            .map_err(|e| {
                                format!(
                                    "Added the server, but couldn't keep its client secret: {e}"
                                )
                            })?;
                            if let Some(store) = store {
                                println!("Client secret: kept in {}", store.display());
                            }
                        }
                        Ok(())
                    }
                    Err(err) => Err(write_error(&err, name, path)),
                },
            )
        }
        McpCli::Remove { name } => {
            with_user_file(|path, contents| match mcp::remove_server(&contents, name) {
                Ok(updated) => {
                    write_user_file(path, &updated)?;
                    println!("Removed MCP server \"{name}\"");
                    println!("File: {}", path.display());
                    Ok(())
                }
                Err(err) => Err(write_error(&err, name, path)),
            })
        }
        McpCli::List => with_user_file(|path, contents| {
            let file = parse_document(path, &contents)?;
            for error in &file.errors {
                eprintln!("warning: invalid entry — {error}");
            }
            if file.servers.is_empty() {
                println!("No MCP servers in {}", path.display());
                println!("Add one with: {} mcp add <name> --url <url>", bin_name());
                return Ok(());
            }
            println!("MCP servers in {}:", path.display());
            for (name, config) in &file.servers {
                println!(
                    "  {name}: {} ({})",
                    config.target(),
                    config.transport_label()
                );
            }
            Ok(())
        }),
        McpCli::Get { name } => with_user_file(|path, contents| {
            let file = parse_document(path, &contents)?;
            let Some((_, config)) = file.servers.iter().find(|(n, _)| n == name) else {
                // A name that IS in the file but didn't parse gets its own
                // reason, not a lying "no such server".
                let prefix = format!("{name}: ");
                if let Some(error) = file.errors.iter().find(|e| e.starts_with(&prefix)) {
                    return Err(format!(
                        "MCP server \"{name}\" has an invalid entry — {error}"
                    ));
                }
                return Err(format!(
                    "No MCP server named \"{name}\" in {}",
                    path.display()
                ));
            };
            print_server(name, config, path);
            Ok(())
        }),
    }
}

/// Resolve the user file, read it (missing = empty), and run the
/// subcommand's body; an `Err(message)` prints to stderr and exits 1.
fn with_user_file(body: impl FnOnce(&Path, String) -> Result<(), String>) -> i32 {
    let Some(path) = config::mcp_user_file_path() else {
        eprintln!("No MCP config path (set HOME or ALTER_ZERO_CONFIG_DIR)");
        return 1;
    };
    let contents = match read_user_file(&path) {
        Ok(contents) => contents,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    match body(&path, contents) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

/// Read the user file — a missing one is an empty config, any other failure
/// is a loud error.
fn read_user_file(path: &PathBuf) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("Failed to read {}: {err}", path.display())),
    }
}

/// Write the updated document — atomically, keeping the file's permissions
/// (owner-only for a new one) and any symlink to it, creating the config
/// dir on first use (`docs/cli/mcp-cli.md`).
fn write_user_file(path: &Path, contents: &str) -> Result<(), String> {
    alter_zero::llm::mcp::write_config_file(path, contents)
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

/// Parse the whole file for `list`/`get`, refusing a document that isn't
/// JSON at all (per-entry problems come back as `errors` and are only
/// warnings there).
fn parse_document(path: &Path, contents: &str) -> Result<mcp::McpFile, String> {
    if contents.trim().is_empty() {
        return Ok(mcp::McpFile::default());
    }
    if let Err(err) = serde_json::from_str::<serde_json::Value>(contents.trim()) {
        return Err(format!("{} is not valid JSON ({err})", path.display()));
    }
    Ok(mcp::parse_mcp_file(contents))
}

/// Map a pure writer's refusal to its message (`docs/cli/mcp-cli.md`).
fn write_error(err: &McpWriteError, name: &str, path: &Path) -> String {
    match err {
        McpWriteError::InvalidJson(reason) => {
            format!("{}: {reason}; refusing to rewrite it", path.display())
        }
        McpWriteError::Exists => format!(
            "MCP server \"{name}\" already exists in {}\nRemove it first with: {} mcp remove {name}",
            path.display(),
            bin_name()
        ),
        McpWriteError::Missing => {
            format!("No MCP server named \"{name}\" in {}", path.display())
        }
    }
}

/// `mcp get`'s fact sheet — the `/mcp` detail page's plain-text cousin, with
/// env/header **values masked** (they routinely carry tokens; the file has
/// the real thing).
fn print_server(name: &str, config: &McpServerConfig, path: &Path) {
    println!("{name}:");
    println!("  Type: {}", config.transport_label());
    match config {
        McpServerConfig::Stdio { command, args, env } => {
            println!("  Command: {command}");
            if !args.is_empty() {
                println!("  Args: {}", args.join(" "));
            }
            if !env.is_empty() {
                println!("  Env: {}", masked(env, "="));
            }
        }
        McpServerConfig::Http { url, headers, .. } | McpServerConfig::Sse { url, headers, .. } => {
            println!("  URL: {url}");
            if !headers.is_empty() {
                println!("  Headers: {}", masked(headers, ": "));
            }
        }
    }
    if let Some(oauth) = config.oauth() {
        if let Some(id) = &oauth.client_id {
            println!("  OAuth client: {id}");
        }
        if let Some(port) = oauth.callback_port {
            println!("  Callback port: {port}");
        }
        if let Some(scopes) = &oauth.scopes {
            println!("  Scopes: {scopes}");
        }
        // Where the secret is — never the secret.
        let kept = alter_zero::llm::mcp::load_client_secret(
            config::mcp_auth_path().as_deref(),
            &resolved_url(config),
        )
        .is_some();
        match (&oauth.client_secret, kept) {
            (Some(_), _) => println!("  Client secret: ***** (in the file)"),
            (None, true) => println!("  Client secret: kept with the tokens"),
            (None, false) => {}
        }
    }
    println!("  File: {}", path.display());
    println!();
    println!("Remove with: {} mcp remove {name}", bin_name());
}

/// A remote entry's URL as a session reaches it — its `${VAR}`s expanded
/// from this environment, the address its tokens and client secret are
/// kept under (`docs/tools/mcp.md`) — else as written.
fn resolved_url(config: &McpServerConfig) -> String {
    let expanded = mcp::expand_config(config, &|name| std::env::var(name).ok());
    expanded
        .ok()
        .as_ref()
        .and_then(|config| config.url().map(str::to_string))
        .or_else(|| config.url().map(str::to_string))
        .unwrap_or_default()
}

/// The client secret `--client-secret` asks for: `MCP_CLIENT_SECRET` when
/// set, else a prompt with echo off when stdin is a terminal, else one line
/// of stdin — never a command-line value, which would land in the shell
/// history and the process list (`docs/cli/mcp-cli.md`).
fn read_client_secret() -> Result<String, String> {
    use std::io::{BufRead, IsTerminal, Write};
    if let Ok(secret) = std::env::var("MCP_CLIENT_SECRET")
        && !secret.trim().is_empty()
    {
        return Ok(secret.trim().to_string());
    }
    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.is_terminal() {
        eprint!("Client secret: ");
        let _ = std::io::stderr().flush();
        let original = rustix::termios::tcgetattr(&stdin).ok();
        if let Some(original) = &original {
            let mut quiet = original.clone();
            quiet.local_modes.remove(rustix::termios::LocalModes::ECHO);
            let _ =
                rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &quiet);
        }
        let read = stdin.lock().read_line(&mut line);
        if let Some(original) = &original {
            let _ =
                rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, original);
        }
        eprintln!();
        read.map_err(|e| format!("couldn't read the client secret: {e}"))?;
    } else {
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("couldn't read the client secret: {e}"))?;
    }
    let secret = line.trim().to_string();
    if secret.is_empty() {
        return Err(
            "no client secret was given (set MCP_CLIENT_SECRET, or type it when asked)".to_string(),
        );
    }
    Ok(secret)
}

/// `KEY=*****, Other=*****` — names visible, values masked.
fn masked(map: &std::collections::BTreeMap<String, String>, sep: &str) -> String {
    map.keys()
        .map(|key| format!("{key}{sep}*****"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The program name hint lines echo — how the user actually invoked us.
fn bin_name() -> String {
    cli::bin_name(std::env::args().next().as_deref())
}

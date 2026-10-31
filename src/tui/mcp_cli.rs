//! The `mcp` subcommand boundary (`docs/mcp-cli.md`): the file I/O and
//! printing behind `alter-zero mcp add/add-json/remove/get/list`, run by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like every other CLI resolution (`docs/cli.md`).
//!
//! The grammar and its mapping onto [`McpServerConfig`] are the pure
//! `cli`/`mcp` modules'; this file only resolves the user config path, does
//! the read-modify-write around the pure writers, and maps each outcome to
//! its message and exit code. Unlike the TUI's best-effort writers
//! (`docs/mcp.md`), every failure here is loud: a CLI that could not
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
            // rule as the main `--help` (docs/cli.md).
            println!("{}", cli::help(HelpPage::Mcp, stdout_style()));
            0
        }
        McpCli::Add {
            name,
            config,
            read_client_secret,
        } => {
            let mut config = config.clone();
            if *read_client_secret {
                let secret = match client_secret(name) {
                    Ok(secret) => secret,
                    Err(message) => {
                        eprintln!("{message}");
                        return 1;
                    }
                };
                if let McpServerConfig::Http {
                    oauth: Some(oauth), ..
                }
                | McpServerConfig::Sse {
                    oauth: Some(oauth), ..
                } = &mut config
                {
                    oauth.client_secret = Some(secret);
                }
            }
            with_user_file(
                |path, contents| match mcp::record_server(&contents, name, &config) {
                    Ok(updated) => {
                        write_user_file(path, &updated)?;
                        println!(
                            "Added {} MCP server \"{name}\" ({})",
                            config.transport_label(),
                            config.target()
                        );
                        println!("File: {}", path.display());
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

/// Write the updated document, creating the config dir on first use. A
/// document holding an OAuth client secret is kept owner-only (`0600`, the
/// token store's mode): the secret is the one thing in it someone else
/// could use.
fn write_user_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return Err(format!("Failed to create {}: {err}", parent.display()));
    }
    std::fs::write(path, contents)
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    #[cfg(unix)]
    if mcp::parse_mcp_file(contents)
        .servers
        .iter()
        .any(|(_, config)| config.oauth().is_some_and(|o| o.client_secret.is_some()))
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("Failed to restrict {}: {err}", path.display()))?;
    }
    Ok(())
}

/// The pre-registered client's secret for `--client-secret`: Claude Code's
/// `MCP_CLIENT_SECRET` when set (the scripted path), else one line from
/// stdin — asked for on the terminal without echo, or read as given when
/// stdin is a pipe. Never an argument, which the shell would keep.
fn client_secret(name: &str) -> Result<String, String> {
    use std::io::{BufRead, IsTerminal, Write};
    if let Some(secret) = std::env::var("MCP_CLIENT_SECRET")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Ok(secret);
    }
    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.is_terminal() {
        eprint!("Client secret for {name}: ");
        let _ = std::io::stderr().flush();
        let quiet = without_echo(&stdin);
        let read = stdin.lock().read_line(&mut line);
        drop(quiet);
        eprintln!();
        read.map_err(|err| format!("Failed to read the client secret: {err}"))?;
    } else {
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|err| format!("Failed to read the client secret: {err}"))?;
    }
    let secret = line.trim().to_string();
    if secret.is_empty() {
        return Err("No client secret given (set MCP_CLIENT_SECRET, or type it when asked)".into());
    }
    Ok(secret)
}

/// Turns the terminal's echo off until dropped — so a typed secret never
/// reaches the screen, and a failure to read it still restores the echo.
struct EchoOff<'a> {
    stdin: &'a std::io::Stdin,
    restore: rustix::termios::Termios,
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            self.stdin,
            rustix::termios::OptionalActions::Now,
            &self.restore,
        );
    }
}

/// Echo off for the secret prompt; `None` when the terminal refuses, and
/// the prompt then simply echoes.
fn without_echo(stdin: &std::io::Stdin) -> Option<EchoOff<'_>> {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};
    let restore = tcgetattr(stdin).ok()?;
    let mut quiet = restore.clone();
    quiet.local_modes.remove(LocalModes::ECHO);
    tcsetattr(stdin, OptionalActions::Now, &quiet).ok()?;
    Some(EchoOff { stdin, restore })
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

/// Map a pure writer's refusal to its message (`docs/mcp-cli.md`).
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
        McpServerConfig::Http {
            url,
            headers,
            oauth,
            ..
        }
        | McpServerConfig::Sse {
            url,
            headers,
            oauth,
        } => {
            println!("  URL: {url}");
            if !headers.is_empty() {
                println!("  Headers: {}", masked(headers, ": "));
            }
            if let Some(oauth) = oauth {
                if let Some(id) = &oauth.client_id {
                    println!("  OAuth client: {id}");
                }
                if oauth.client_secret.is_some() {
                    println!("  OAuth client secret: *****");
                }
                if let Some(port) = oauth.callback_port {
                    println!("  OAuth callback: http://localhost:{port}/callback");
                }
                if let Some(scopes) = &oauth.scopes {
                    println!("  OAuth scopes: {scopes}");
                }
                if let Some(meta) = &oauth.auth_server_metadata_url {
                    println!("  OAuth metadata: {meta}");
                }
            }
        }
    }
    println!("  File: {}", path.display());
    println!();
    println!("Remove with: {} mcp remove {name}", bin_name());
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

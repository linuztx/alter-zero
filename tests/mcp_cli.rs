//! Integration proof of the `mcp` CLI subcommand family (`docs/mcp-cli.md`)
//! against the **real built binary** (the `detached_exec.rs` pattern —
//! cargo hands the path as `CARGO_BIN_EXE_alter-zero`), with
//! `ALTER_ZERO_MCP_FILE` pointed into a per-test temp dir. Offline and
//! deterministic: nothing connects; the commands are file surgery + print.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The TUI binary this test run just built.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

/// A per-test scratch dir (unique per test name, wiped at entry) and the
/// mcp.json path inside it.
fn scratch(test: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir()
        .join("alter-zero-mcp-cli-tests")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join("mcp.json");
    (dir, file)
}

/// Run `alter-zero mcp …` with the user file redirected at `file`.
fn mcp(file: &PathBuf, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("mcp")
        .args(args)
        .env("ALTER_ZERO_MCP_FILE", file)
        .output()
        .expect("binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn add_writes_the_claude_code_compatible_shapes() {
    let (_dir, file) = scratch("add_shapes");
    // The bare-url add (the user-facing default): exit 0, both output lines.
    let out = mcp(&file, &["add", "vercel", "--url", "https://mcp.vercel.com"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Added http MCP server \"vercel\" (https://mcp.vercel.com)"),
        "{text}"
    );
    assert!(
        text.contains(&format!("File: {}", file.display())),
        "{text}"
    );
    // stdio with env, the raw command capture keeping the child's flags.
    let out = mcp(
        &file,
        &[
            "add", "docs", "-e", "K=v", "npx", "-y", "pkg", "--port", "4000",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // sse with a header.
    let out = mcp(
        &file,
        &[
            "add",
            "legacy",
            "--transport",
            "sse",
            "--url",
            "https://x/sse",
            "-H",
            "Authorization: Bearer t",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // The file is the exact `mcpServers` document the TUI reads — the
    // bare-url entry carries NO "type" key (the http→sse fallback shape).
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("file written"))
            .expect("valid JSON");
    let servers = written.get("mcpServers").expect("mcpServers key");
    assert_eq!(
        servers.get("vercel"),
        Some(&serde_json::json!({"url": "https://mcp.vercel.com"}))
    );
    assert_eq!(
        servers.get("docs"),
        Some(&serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "pkg", "--port", "4000"],
            "env": {"K": "v"}
        }))
    );
    assert_eq!(
        servers.get("legacy"),
        Some(&serde_json::json!({
            "type": "sse",
            "url": "https://x/sse",
            "headers": {"Authorization": "Bearer t"}
        }))
    );
}

#[test]
fn list_shows_file_order_and_remove_keeps_it() {
    let (_dir, file) = scratch("list_order");
    for name in ["a", "b", "c"] {
        let out = mcp(&file, &["add", name, "--url", &format!("https://{name}")]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    // Removing the FIRST entry must not reorder the rest (the
    // preserve_order swap_remove trap).
    let out = mcp(&file, &["remove", "a"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Removed MCP server \"a\""));
    let out = mcp(&file, &["list"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let b = text.find("b: https://b").expect("b listed");
    let c = text.find("c: https://c").expect("c listed");
    assert!(b < c, "declaration order survives the remove: {text}");
    assert!(!text.contains("https://a"), "{text}");
}

#[test]
fn get_masks_secret_values() {
    let (_dir, file) = scratch("get_masks");
    let out = mcp(
        &file,
        &[
            "add",
            "api",
            "--url",
            "https://x",
            "-H",
            "Authorization: Bearer secret123",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let out = mcp(&file, &["get", "api"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Type: http"), "{text}");
    assert!(text.contains("URL: https://x"), "{text}");
    assert!(text.contains("Authorization: *****"), "{text}");
    assert!(
        !text.contains("secret123"),
        "the value must be masked: {text}"
    );
    assert!(text.contains("mcp remove api"), "{text}");
}

#[test]
fn add_json_routes_through_the_same_schema() {
    let (_dir, file) = scratch("add_json");
    let out = mcp(
        &file,
        &[
            "add-json",
            "wiki",
            r#"{"type": "http", "url": "https://x/mcp"}"#,
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Added http MCP server \"wiki\""));
    let out = mcp(&file, &["add-json", "bad", r#"{"type": "ws"}"#]);
    assert_eq!(out.status.code(), Some(2), "a bad entry is a usage error");
    assert!(
        stderr(&out).contains("invalid server JSON"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn failures_exit_1_and_grammar_errors_exit_2() {
    let (_dir, file) = scratch("exit_codes");
    let out = mcp(&file, &["add", "wiki", "--url", "https://a"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // Duplicate: exit 1, names the escape hatch, file untouched.
    let before = std::fs::read_to_string(&file).expect("written");
    let out = mcp(&file, &["add", "wiki", "--url", "https://b"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));
    assert!(stderr(&out).contains("mcp remove wiki"), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&file).expect("still there"), before);
    // Unknown names: exit 1.
    for args in [&["remove", "nope"][..], &["get", "nope"][..]] {
        let out = mcp(&file, args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(stderr(&out).contains("No MCP server named \"nope\""));
    }
    // Grammar errors: exit 2 with the clap-shaped trailer — `error:`, the
    // mcp `Usage:` block, the pointer at --help (docs/cli.md).
    let out = mcp(&file, &["add", "x", "--url", "https://u", "--", "cmd"]);
    assert_eq!(out.status.code(), Some(2));
    let text = stderr(&out);
    assert!(
        text.starts_with(
            "error: pass a --url or a command, not both\n\nUsage: alter-zero mcp add "
        ),
        "{text}"
    );
    assert!(text.contains("\n       alter-zero mcp list\n"), "{text}");
    assert!(
        text.ends_with("For more information, try '--help'.\n"),
        "{text}"
    );
    // Help: exit 0 on stdout, the page titled by its command.
    let out = mcp(&file, &["--help"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).lines().next(), Some("alter-zero mcp"));
    assert!(
        stdout(&out).contains("Usage: alter-zero mcp add"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn an_unparseable_file_is_refused_never_clobbered() {
    let (_dir, file) = scratch("refuse_clobber");
    std::fs::write(&file, "{broken json").expect("plant garbage");
    let out = mcp(&file, &["add", "wiki", "--url", "https://a"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("refusing to rewrite"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("still there"),
        "{broken json",
        "the file must be byte-identical"
    );
    let out = mcp(&file, &["list"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not valid JSON"), "{}", stderr(&out));
}

#[test]
fn siblings_survive_and_an_empty_file_reports_where_to_add() {
    let (_dir, file) = scratch("siblings");
    // A pre-existing document with the projects disabled-state key: the RMW
    // must pass it through untouched.
    std::fs::write(
        &file,
        r#"{"mcpServers": {"old": {"url": "https://old"}},
           "projects": {"/p": {"disabled": ["old"]}}}"#,
    )
    .expect("seed file");
    let out = mcp(&file, &["add", "new", "--url", "https://new"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("written")).expect("JSON");
    assert_eq!(
        written["projects"]["/p"]["disabled"],
        serde_json::json!(["old"])
    );
    // Empty file: the list names the path and the add command.
    let (_dir, empty) = scratch("siblings_empty");
    let out = mcp(&empty, &["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!("No MCP servers in {}", empty.display())),
        "{text}"
    );
    assert!(text.contains("mcp add <name> --url <url>"), "{text}");
}

/// `mcp` with a piped stdin, `MCP_CLIENT_SECRET` set or cleared.
fn mcp_with_secret(file: &PathBuf, args: &[&str], env: Option<&str>, stdin: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new(bin());
    command
        .arg("mcp")
        .args(args)
        .env("ALTER_ZERO_MCP_FILE", file)
        .env_remove("MCP_CLIENT_SECRET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(secret) = env {
        command.env("MCP_CLIENT_SECRET", secret);
    }
    let mut child = command.spawn().expect("binary runs");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("stdin written");
    child.wait_with_output().expect("binary finishes")
}

#[test]
fn a_client_secret_is_stored_from_the_environment_or_stdin_never_the_command_line() {
    let (_dir, file) = scratch("client_secret");
    let calendar = "https://calendarmcp.googleapis.com/mcp/v1";
    // Claude Code's scripted path: MCP_CLIENT_SECRET.
    let out = mcp_with_secret(
        &file,
        &[
            "add",
            "google-calendar",
            "--url",
            calendar,
            "--client-id",
            "123-abc.apps.googleusercontent.com",
            "--client-secret",
        ],
        Some("GOCSPX-from-env"),
        "",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        written["mcpServers"]["google-calendar"],
        serde_json::json!({
            "url": calendar,
            "oauth": {
                "clientId": "123-abc.apps.googleusercontent.com",
                "clientSecret": "GOCSPX-from-env"
            }
        })
    );
    // A file holding a secret is the owner's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }
    // `get` names the client and masks the secret.
    let out = mcp(&file, &["get", "google-calendar"]);
    let text = stdout(&out);
    assert!(
        text.contains("OAuth client: 123-abc.apps.googleusercontent.com"),
        "{text}"
    );
    assert!(text.contains("OAuth client secret: *****"), "{text}");
    assert!(
        !text.contains("GOCSPX"),
        "the secret must be masked: {text}"
    );
    // Without the variable, a piped stdin line is the secret.
    let out = mcp_with_secret(
        &file,
        &[
            "add-json",
            "--client-secret",
            "piped",
            r#"{"type": "http", "url": "https://x.test/mcp", "oauth": {"clientId": "c", "callbackPort": 8085}}"#,
        ],
        None,
        "s3cret-from-stdin\n",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        written["mcpServers"]["piped"]["oauth"],
        serde_json::json!({"clientId": "c", "clientSecret": "s3cret-from-stdin", "callbackPort": 8085})
    );
    // Nothing given at all: a loud failure, and nothing written.
    let out = mcp_with_secret(
        &file,
        &[
            "add",
            "empty",
            "--url",
            "https://y",
            "--client-id",
            "c",
            "--client-secret",
        ],
        None,
        "",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("No client secret given"),
        "{}",
        stderr(&out)
    );
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"empty\""),
        "a failed add writes nothing"
    );
    // On the command line it is refused outright.
    let out = mcp(
        &file,
        &[
            "add",
            "leak",
            "--url",
            "https://z",
            "--client-id",
            "c",
            "--client-secret=oops",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

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

/// Run `alter-zero mcp …` with the user file at `file`, the config home
/// (where the token store lives) at `home`, extra environment, and `stdin`
/// piped in.
fn mcp_in(
    file: &PathBuf,
    home: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: &str,
) -> Output {
    use std::io::Write;
    let mut child = Command::new(bin())
        .arg("mcp")
        .args(args)
        .env("ALTER_ZERO_MCP_FILE", file)
        .env("ALTER_ZERO_CONFIG_DIR", home)
        .env_remove("MCP_CLIENT_SECRET")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("binary runs");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("stdin written");
    child.wait_with_output().expect("binary exits")
}

const CALENDAR: &str = "https://calendarmcp.googleapis.com/mcp/v1";

#[test]
fn a_client_secret_goes_to_the_token_store_never_the_config() {
    let (dir, file) = scratch("client_secret_env");
    let home = dir.join("home");
    let out = mcp_in(
        &file,
        &home,
        &[
            "add",
            "cal",
            "--url",
            CALENDAR,
            "--client-id",
            "cid.apps",
            "--client-secret",
            "--callback-port",
            "8976",
        ],
        &[("MCP_CLIENT_SECRET", "GOCSPX-shh")],
        "",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("OAuth client: cid.apps"), "{text}");
    assert!(!text.contains("GOCSPX-shh"), "{text}");
    // The config carries Claude Code's `oauth` object and no secret.
    let written = std::fs::read_to_string(&file).expect("config written");
    assert!(!written.contains("GOCSPX-shh"), "{written}");
    let config: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
    assert_eq!(
        config["mcpServers"]["cal"],
        serde_json::json!({"url": CALENDAR, "oauth": {"clientId": "cid.apps", "callbackPort": 8976}})
    );
    // The secret sits in the owner-only token store, beside its client id.
    let store_path = home.join("mcp-auth.json");
    let store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&store_path).expect("store written"))
            .expect("valid JSON");
    assert_eq!(store["clients"][CALENDAR]["client_id"], "cid.apps");
    assert_eq!(store["clients"][CALENDAR]["client_secret"], "GOCSPX-shh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&store_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the store is owner-only");
    }
    // `get` names the client and says the secret is kept, never its value.
    let out = mcp_in(&file, &home, &["get", "cal"], &[], "");
    let text = stdout(&out);
    assert!(text.contains("OAuth client: cid.apps"), "{text}");
    assert!(text.contains("Callback port: 8976"), "{text}");
    assert!(text.contains("Client secret: stored"), "{text}");
    assert!(!text.contains("GOCSPX-shh"), "{text}");
}

#[test]
fn a_piped_secret_is_read_and_a_missing_one_writes_nothing() {
    let (dir, file) = scratch("client_secret_stdin");
    let home = dir.join("home");
    let args = [
        "add",
        "cal",
        "--url",
        CALENDAR,
        "--client-id",
        "cid",
        "--client-secret",
    ];
    // Nothing on stdin and no MCP_CLIENT_SECRET: a loud failure, and the
    // server is not half-added without the secret it needs.
    let out = mcp_in(&file, &home, &args, &[], "");
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("MCP_CLIENT_SECRET"),
        "{}",
        stderr(&out)
    );
    assert!(!file.exists(), "nothing written without the secret");
    // A script pipes it in.
    let out = mcp_in(&file, &home, &args, &[], "piped-secret\n");
    assert!(out.status.success(), "{}", stderr(&out));
    let store = std::fs::read_to_string(home.join("mcp-auth.json")).expect("store written");
    assert!(store.contains("piped-secret"), "{store}");
}

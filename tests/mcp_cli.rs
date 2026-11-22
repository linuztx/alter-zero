//! Integration proof of the `mcp` CLI subcommand family (`docs/mcp-cli.md`)
//! against the **real built binary** (the `detached_exec.rs` pattern —
//! cargo hands the path as `CARGO_BIN_EXE_alter-zero`), with
//! `ALTER_ZERO_MCP_FILE` pointed into a per-test temp dir. Offline and
//! deterministic: nothing connects; the commands are file surgery + print.

use std::path::{Path, PathBuf};
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

/// A client entered on the `/mcp` auth page lives with the tokens, not in
/// the file — `get` names it all the same, and never its secret.
#[test]
fn get_names_a_client_entered_in_the_tui_never_its_secret() {
    let (dir, file) = scratch("get_entered_client");
    let out = mcp(&file, &["add", "calendar", "--url", "https://cal.test/mcp"]);
    assert!(out.status.success(), "{}", stderr(&out));
    std::fs::write(
        dir.join("mcp-auth.json"),
        r#"{"clients":{"https://cal.test/mcp":{"client_id":"typed-cid.apps.example","client_secret":"GOCSPX-hunter2","callback_port":9001,"issuer":"https://accounts.example.com"}}}"#,
    )
    .expect("write the store");
    let out = Command::new(bin())
        .args(["mcp", "get", "calendar"])
        .env("ALTER_ZERO_MCP_FILE", &file)
        .env("ALTER_ZERO_CONFIG_DIR", &dir)
        .output()
        .expect("binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("OAuth client: typed-cid.apps.example (entered in /mcp)"),
        "{text}"
    );
    assert!(text.contains("Callback port: 9001"), "{text}");
    assert!(
        text.contains("Client secret: kept with the tokens"),
        "{text}"
    );
    assert!(
        !text.contains("GOCSPX"),
        "the secret is never printed: {text}"
    );
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

/// Run `alter-zero mcp …` with the whole config home at `dir` — the user
/// file at `{dir}/mcp.json`, the token store at `{dir}/mcp-auth.json` — so a
/// test reading what is kept for a server never reads the real one.
fn mcp_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("mcp")
        .args(args)
        .env_remove("ALTER_ZERO_MCP_FILE")
        .env_remove("MCP_CLIENT_SECRET")
        .env("ALTER_ZERO_CONFIG_DIR", dir)
        .output()
        .expect("binary runs")
}

/// Write `{dir}/mcp-auth.json` whole.
fn write_store(dir: &Path, store: serde_json::Value) {
    std::fs::write(dir.join("mcp-auth.json"), store.to_string()).expect("store written");
}

/// The store's `clients` and `servers` entries for `server`.
fn kept_for(dir: &Path, server: &str) -> (serde_json::Value, serde_json::Value) {
    let store: serde_json::Value = std::fs::read_to_string(dir.join("mcp-auth.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    (
        store["clients"][server].clone(),
        store["servers"][server].clone(),
    )
}

/// A client entered in `/mcp` outranks the entry's own client: `get` names
/// it, says where it is from, and says the entry's is not used. With only a
/// secret kept for the entry's client, the entry's is the one named.
#[test]
fn get_says_an_entered_client_outranks_the_entrys_own() {
    let (dir, _file) = scratch("get_entered_over_entry");
    let server = "https://s.test/mcp";
    let out = mcp_in(
        &dir,
        &["add", "srv", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    write_store(
        &dir,
        serde_json::json!({"clients": {server: {
            "client_id": "typed", "client_secret": "GOCSPX-hunter2",
            "callback_port": 9001, "issuer": "https://as.test"
        }}}),
    );
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(
        text.contains("OAuth client: typed (entered in /mcp)"),
        "{text}"
    );
    assert!(text.contains("Callback port: 9001"), "{text}");
    assert!(
        text.contains("Entry's own client: from-file (not used while"),
        "{text}"
    );
    assert!(!text.contains("GOCSPX"), "{text}");
    write_store(
        &dir,
        serde_json::json!({"clients": {server: {"client_secret": "shh"}}}),
    );
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(
        text.contains("OAuth client: from-file (in the file)"),
        "{text}"
    );
    assert!(
        text.contains("Client secret: kept with the tokens"),
        "{text}"
    );
    assert!(!text.contains("shh"), "{text}");
}

/// An entry added with its own client is the statement about its address:
/// a client entered in `/mcp` there is forgotten, and said, and so is the
/// login made as it — never a login made as another client. An entry added
/// without a client leaves what is kept alone.
#[test]
fn add_with_its_own_client_forgets_one_entered_in_the_tui() {
    let server = "https://s.test/mcp";
    let store = |login_as: &str| {
        serde_json::json!({
            "clients": {server: {"client_id": "typed", "client_secret": "old", "issuer": "https://as.test"}},
            "servers": {server: {"access_token": "at", "client_id": login_as}}
        })
    };
    let (dir, _file) = scratch("add_forgets_entered");
    write_store(&dir, store("typed"));
    let out = mcp_in(&dir, &["add", "plain", "--url", server]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("Forgot"), "{}", stdout(&out));
    assert_eq!(kept_for(&dir, server).0["client_id"], "typed");
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let (client, login) = kept_for(&dir, server);
    assert!(
        client.is_null(),
        "the entered client outlived the add: {client}"
    );
    assert!(login.is_null(), "its login outlived it: {login}");
    assert!(
        stdout(&out)
            .contains("Forgot the OAuth client entered in /mcp for this server, and its login\n"),
        "{}",
        stdout(&out)
    );
    // A login made as another client is not the forgotten one's to take.
    let (dir, _file) = scratch("add_keeps_other_login");
    write_store(&dir, store("someone-else"));
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let (client, login) = kept_for(&dir, server);
    assert!(client.is_null(), "{client}");
    assert_eq!(login["client_id"], "someone-else");
    assert!(
        stdout(&out).contains("Forgot the OAuth client entered in /mcp for this server\n"),
        "{}",
        stdout(&out)
    );
    // Nothing kept and no store: none is made for it.
    let (dir, _file) = scratch("add_forgets_nothing");
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!dir.join("mcp-auth.json").exists(), "a store was made");
}

/// The secret kept for the entry's own client is not an entered client:
/// adding the entry again, naming its client without `--client-secret`,
/// leaves it where it is.
#[test]
fn add_naming_its_client_keeps_the_secret_kept_for_it() {
    let (dir, _file) = scratch("add_keeps_cli_secret");
    let server = "https://s.test/mcp";
    write_store(
        &dir,
        serde_json::json!({"clients": {server: {"client_secret": "for-the-entry"}}}),
    );
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        kept_for(&dir, server).0,
        serde_json::json!({"client_secret": "for-the-entry"})
    );
    assert!(!stdout(&out).contains("Forgot"), "{}", stdout(&out));
}

/// `--client-secret` replaces what is kept for the address with the
/// entry's secret: an entered client it replaces is said, and its login
/// goes with it.
#[test]
fn add_with_a_secret_says_it_replaced_an_entered_client() {
    let (dir, _file) = scratch("add_secret_replaces_entered");
    let server = "https://s.test/mcp";
    write_store(
        &dir,
        serde_json::json!({
            "clients": {server: {"client_id": "typed", "issuer": "https://as.test"}},
            "servers": {server: {"access_token": "at", "client_id": "typed"}}
        }),
    );
    let out = Command::new(bin())
        .args(["mcp", "add", "own", "--url", server])
        .args(["--client-id", "from-file", "--client-secret"])
        .env_remove("ALTER_ZERO_MCP_FILE")
        .env("MCP_CLIENT_SECRET", "for-the-entry")
        .env("ALTER_ZERO_CONFIG_DIR", &dir)
        .output()
        .expect("binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    let (client, login) = kept_for(&dir, server);
    assert_eq!(
        client,
        serde_json::json!({"client_secret": "for-the-entry"})
    );
    assert!(login.is_null(), "{login}");
    let text = stdout(&out);
    assert!(
        text.contains("Forgot the OAuth client entered in /mcp for this server, and its login\n"),
        "{text}"
    );
    assert!(!text.contains("for-the-entry"), "{text}");
}

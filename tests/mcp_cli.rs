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
/// test that reads what is kept for a server never reads the real one.
fn mcp_in(dir: &PathBuf, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("mcp")
        .args(args)
        .env_remove("ALTER_ZERO_MCP_FILE")
        .env_remove("MCP_CLIENT_SECRET")
        .env("ALTER_ZERO_CONFIG_DIR", dir)
        .output()
        .expect("binary runs")
}

/// Write `{dir}/mcp-auth.json` with `client` kept for `server`.
fn keep_client(dir: &Path, server: &str, client: serde_json::Value) {
    let store = serde_json::json!({ "clients": { server: client } });
    std::fs::write(dir.join("mcp-auth.json"), store.to_string()).expect("store written");
}

fn kept_clients(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join("mcp-auth.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|store| store.get("clients").cloned())
        .unwrap_or(serde_json::Value::Null)
}

/// A client entered in `/mcp` lives in the token store, not the file: `get`
/// names it — it is the one a sign-in uses — says a secret is kept, and
/// never prints one.
#[test]
fn get_names_a_client_entered_in_mcp_and_never_its_secret() {
    let (dir, _file) = scratch("get_entered_client");
    let server = "https://calendarmcp.googleapis.com/mcp/v1";
    let out = mcp_in(&dir, &["add", "calendar", "--url", server]);
    assert!(out.status.success(), "{}", stderr(&out));
    // Nothing kept: nothing said about a client.
    let text = stdout(&mcp_in(&dir, &["get", "calendar"]));
    assert!(!text.contains("OAuth client"), "{text}");
    keep_client(
        &dir,
        server,
        serde_json::json!({
            "client_id": "typed.apps.googleusercontent.com",
            "client_secret": "GOCSPX-hunter2",
            "callback_port": 8976,
            "issuer": "https://accounts.google.com"
        }),
    );
    let out = mcp_in(&dir, &["get", "calendar"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("OAuth client: typed.apps.googleusercontent.com (entered in /mcp)"),
        "{text}"
    );
    assert!(text.contains("Callback port: 8976"), "{text}");
    assert!(
        text.contains("Client secret: kept with the tokens"),
        "{text}"
    );
    assert!(
        !text.contains("GOCSPX-hunter2"),
        "the secret was printed: {text}"
    );
}

/// Where the file names its own client too, `get` says which one signs in —
/// the entered one — rather than leaving the file's to be assumed.
#[test]
fn get_says_an_entered_client_stands_in_for_the_files_own() {
    let (dir, _file) = scratch("get_entered_over_config");
    let server = "https://s.test/mcp";
    let out = mcp_in(
        &dir,
        &["add", "srv", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    keep_client(
        &dir,
        server,
        serde_json::json!({"client_id": "typed", "issuer": "https://as.test"}),
    );
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(
        text.contains("OAuth client: typed (entered in /mcp)"),
        "{text}"
    );
    assert!(text.contains("from-file"), "{text}");
    assert!(text.contains("not used"), "{text}");
    // The secret kept for the file's own client still reads as it did.
    keep_client(&dir, server, serde_json::json!({"client_secret": "shh"}));
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(text.contains("OAuth client: from-file\n"), "{text}");
    assert!(
        text.contains("Client secret: kept with the tokens"),
        "{text}"
    );
    assert!(!text.contains("shh"), "{text}");
}

/// A client entered as the file's own — to give it a port, say, its secret
/// field left empty to keep the one the file carries — signs in with the
/// file's secret, and `get` says where that is. Entered as a public client
/// it has none, whatever the file holds, and its port is what was entered.
#[test]
fn get_says_where_an_entered_clients_secret_is() {
    let (dir, _file) = scratch("get_secret_from_entry");
    let server = "https://s.test/mcp";
    let entry = serde_json::json!({
        "type": "http",
        "url": server,
        "oauth": { "clientId": "cid", "clientSecret": "in-file", "callbackPort": 8000 }
    })
    .to_string();
    let out = mcp_in(&dir, &["add-json", "srv", &entry]);
    assert!(out.status.success(), "{}", stderr(&out));
    keep_client(
        &dir,
        server,
        serde_json::json!({
            "client_id": "cid", "issuer": "https://as.test",
            "callback_port": 9000, "secret_from_entry": true
        }),
    );
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(
        text.contains("OAuth client: cid (entered in /mcp)"),
        "{text}"
    );
    assert!(text.contains("Callback port: 9000"), "{text}");
    assert!(
        text.contains("Client secret: ***** (in the file)"),
        "{text}"
    );
    assert!(!text.contains("in-file"), "the secret was printed: {text}");
    keep_client(
        &dir,
        server,
        serde_json::json!({"client_id": "cid", "issuer": "https://as.test"}),
    );
    let text = stdout(&mcp_in(&dir, &["get", "srv"]));
    assert!(
        text.contains("OAuth client: cid (entered in /mcp)"),
        "{text}"
    );
    assert!(!text.contains("Client secret"), "{text}");
    assert!(!text.contains("Callback port"), "{text}");
}

/// An entry added with its own client is the statement about that address:
/// a client entered in `/mcp` for an earlier entry there would otherwise
/// sign in in its place, unseen. One added without a client leaves what is
/// kept alone.
#[test]
fn add_with_its_own_client_forgets_one_kept_for_the_address() {
    let (dir, _file) = scratch("add_forgets_entered");
    let server = "https://s.test/mcp";
    let typed = serde_json::json!({
        "client_id": "typed", "client_secret": "old", "issuer": "https://as.test"
    });
    keep_client(&dir, server, typed.clone());
    let out = mcp_in(&dir, &["add", "plain", "--url", server]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(kept_clients(&dir)[server], typed, "a plain add keeps it");
    assert!(!stdout(&out).contains("Forgot"), "{}", stdout(&out));
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        kept_clients(&dir).get(server).is_none(),
        "the entered client outlived the entry that names its own: {}",
        kept_clients(&dir)
    );
    // What went is said: it was the user's, typed somewhere else.
    assert!(
        stdout(&out).contains("Forgot the OAuth client entered in /mcp for this address\n"),
        "{}",
        stdout(&out)
    );
    // Nothing kept and no store at all: nothing is created for it.
    let (dir, _file) = scratch("add_forgets_nothing");
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !dir.join("mcp-auth.json").exists(),
        "an empty store was written"
    );
}

/// Write `{dir}/mcp-auth.json` whole.
fn write_store(dir: &Path, store: serde_json::Value) {
    std::fs::write(dir.join("mcp-auth.json"), store.to_string()).expect("store written");
}

/// The client a login kept for `server` was made as, if one is kept.
fn login_of(dir: &Path, server: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("mcp-auth.json")).ok()?;
    let store: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(store["servers"][server]["client_id"].as_str()?.to_string())
}

/// The login made as an entered client goes with it: the grant keeps its
/// own copy of the id and secret, so left behind it would go on signing in
/// as the client the new entry just replaced. Both ways an `add` replaces
/// one — an entry naming its own client, and `--client-secret` — say so.
#[test]
fn add_replacing_an_entered_client_signs_out_of_it() {
    let server = "https://s.test/mcp";
    let store = |login_as: &str| {
        serde_json::json!({
            "clients": { server: {
                "client_id": "typed", "client_secret": "old", "issuer": "https://as.test"
            } },
            "servers": { server: { "access_token": "at", "client_id": login_as } }
        })
    };
    let (dir, _file) = scratch("add_signs_out");
    write_store(&dir, store("typed"));
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(login_of(&dir, server), None, "its login outlived it");
    assert!(
        stdout(&out).contains(
            "Forgot the OAuth client entered in /mcp for this address, and signed out of it"
        ),
        "{}",
        stdout(&out)
    );
    // `--client-secret` replaces what is kept with the entry's secret.
    let (dir, _file) = scratch("add_secret_signs_out");
    write_store(&dir, store("typed"));
    let out = Command::new(bin())
        .args(["mcp", "add", "own", "--url", server])
        .args(["--client-id", "from-file", "--client-secret"])
        .env_remove("ALTER_ZERO_MCP_FILE")
        .env("MCP_CLIENT_SECRET", "for-the-entry")
        .env("ALTER_ZERO_CONFIG_DIR", &dir)
        .output()
        .expect("binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        kept_clients(&dir)[server],
        serde_json::json!({"client_secret": "for-the-entry"})
    );
    assert_eq!(login_of(&dir, server), None, "its login outlived it");
    assert!(
        stdout(&out).contains(
            "Forgot the OAuth client entered in /mcp for this address, and signed out of it"
        ),
        "{}",
        stdout(&out)
    );
    assert!(!stdout(&out).contains("for-the-entry"), "{}", stdout(&out));
    // A login made as another client is not the forgotten one's to take.
    let (dir, _file) = scratch("add_keeps_other_login");
    write_store(&dir, store("someone-else"));
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(login_of(&dir, server).as_deref(), Some("someone-else"));
}

/// The secret `mcp add --client-secret` kept is the entry's client's, not a
/// client entered in `/mcp`: adding the entry again — naming its client,
/// without the flag — leaves it where it is.
#[test]
fn add_naming_its_client_keeps_the_secret_kept_for_it() {
    let (dir, _file) = scratch("add_keeps_cli_secret");
    let server = "https://s.test/mcp";
    let kept = serde_json::json!({"client_secret": "for-the-entry"});
    keep_client(&dir, server, kept.clone());
    let out = mcp_in(
        &dir,
        &["add", "own", "--url", server, "--client-id", "from-file"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(kept_clients(&dir)[server], kept);
    assert!(!stdout(&out).contains("Forgot"), "{}", stdout(&out));
}

/// An entry's client registered with a redirect of its own — codex's
/// `callback_url` — has `get` name it; one entered in `/mcp` never uses it.
#[test]
fn get_names_the_callback_url_of_an_entrys_client() {
    let (_dir, file) = scratch("get_callback_url");
    std::fs::write(
        &file,
        r#"{"mcpServers":{"gh":{"type":"http","url":"https://api.example.test/mcp/",
            "oauth":{"client_id":"Iv1.fake","callback_port":12799,
                     "callback_url":"http://127.0.0.1:12799/callback/fakePath"}}}}"#,
    )
    .unwrap();
    let out = mcp(&file, &["get", "gh"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("OAuth client: Iv1.fake\n"), "{text}");
    assert!(
        text.contains("Callback URL: http://127.0.0.1:12799/callback/fakePath\n"),
        "{text}"
    );
    // codex's spelling of an id alone is presented on codex's redirect.
    std::fs::write(
        &file,
        r#"{"mcpServers":{"slack":{"type":"http","url":"https://mcp.example.test/mcp","oauth":{"client_id":"1.2"}}}}"#,
    )
    .unwrap();
    let text = stdout(&mcp(&file, &["get", "slack"]));
    assert!(
        text.contains("Callback URL: http://127.0.0.1/callback\n"),
        "{text}"
    );
}

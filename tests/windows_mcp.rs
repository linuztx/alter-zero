//! Native Windows MCP stdio proofs, offline: local npm-shaped shims and batch
//! files exercise argv, environment, cwd, pipe traffic and live-tree teardown.
//! These tests must run on Windows; Linux planning tests cannot establish cmd
//! quoting or Git for Windows process behavior. No Node/npm download is used.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use alter_zero::llm::mcp::{StdioTransport, Transport};
use alter_zero::stream::CancelToken;
use serde_json::{Value, json};

fn posix_path(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

fn fixture(dir: &Path) -> BTreeMap<String, String> {
    // Capture argv without any shell serialization: each argument ends in NUL.
    std::fs::write(
        dir.join("server.sh"),
        r#"#!/bin/sh
: > "$MCP_ARGUMENTS_FILE"
for arg do printf '%s\0' "$arg" >> "$MCP_ARGUMENTS_FILE"; done
[ "$MCP_FIXTURE_ENV" = 'overlay value' ] || exit 91
[ -f cwd-marker ] || exit 92
IFS= read -r request || exit 93
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"fixture":true}}'
cat > /dev/null
"#,
    )
    .unwrap();
    std::fs::write(dir.join("cwd-marker"), "cwd").unwrap();
    BTreeMap::from([
        (
            "MCP_ARGUMENTS_FILE".into(),
            posix_path(&dir.join("arguments.bin")),
        ),
        ("MCP_FIXTURE_ENV".into(), "overlay value".into()),
    ])
}

fn round_trip(program: &str, args: &[&str], env: &BTreeMap<String, String>, cwd: &Path) {
    let mut transport = Transport::Stdio(
        StdioTransport::spawn(
            program,
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
            env,
            Some(cwd),
        )
        .expect("local MCP fixture starts"),
    );
    assert_eq!(
        transport
            .request(
                "fixture",
                Value::Null,
                Duration::from_secs(10),
                &CancelToken::new()
            )
            .unwrap(),
        json!({"fixture": true})
    );
    let expected = args
        .iter()
        .flat_map(|arg| arg.as_bytes().iter().copied().chain([0]))
        .collect::<Vec<_>>();
    assert_eq!(std::fs::read(cwd.join("arguments.bin")).unwrap(), expected);
}

#[test]
fn extensionless_npx_shim_preserves_literal_argv_env_and_cwd() {
    let dir = tempfile::Builder::new()
        .prefix("mcp npm shim ")
        .tempdir()
        .unwrap();
    let mut env = fixture(dir.path());
    std::fs::copy(dir.path().join("server.sh"), dir.path().join("npx")).unwrap();
    // The child's PATH overlay must also be the resolver's PATH. Retain Git
    // utilities for the fixture's cat, but never run the installed npx.
    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    env.insert(
        "PATH".into(),
        std::env::join_paths(paths).unwrap().into_string().unwrap(),
    );
    round_trip(
        "npx",
        &[
            "-y",
            "@fixture/server",
            "two words",
            "",
            "東京",
            "$(touch owned)",
            "a\"b",
            "%PATH%",
            "!value!",
            "x&y",
            "/literal/path",
        ],
        &env,
        dir.path(),
    );
    assert!(
        !dir.path().join("owned").exists(),
        "arguments never become shell source"
    );
}

#[test]
fn explicit_cmd_and_bat_files_keep_the_safe_argument_subset() {
    alter_zero::subprocess::validate_tool_shell().unwrap();
    for extension in ["cmd", "bat"] {
        let dir = tempfile::Builder::new()
            .prefix("mcp batch ")
            .tempdir()
            .unwrap();
        let env = fixture(dir.path());
        let program = dir.path().join(format!("npx.{extension}"));
        // An npm-shaped batch fixture: cmd hands its argument tail to a
        // native executable; the fixture then records the received argv.
        std::fs::write(
            &program,
            format!(
                "@echo off\r\n\"{}\" --noprofile --norc server.sh %*\r\n",
                alter_zero::subprocess::tool_shell().display()
            ),
        )
        .unwrap();
        round_trip(
            program.to_str().unwrap(),
            &["-y", "@fixture/server", "two words", "", "東京"],
            &env,
            dir.path(),
        );
        let unsafe_args = vec!["x&echo unsafe".to_string()];
        assert!(
            StdioTransport::spawn(
                program.to_str().unwrap(),
                &unsafe_args,
                &env,
                Some(dir.path())
            )
            .is_err()
        );
    }
}

#[test]
fn dropping_stdio_stops_a_live_servers_descendant() {
    alter_zero::subprocess::validate_tool_shell().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let heartbeat = dir.path().join("heartbeat");
    let env = BTreeMap::from([("MCP_HEARTBEAT".into(), posix_path(&heartbeat))]);
    // The root stays alive. This proves taskkill's live-tree behavior, not
    // orphan ownership after the root exits (which needs a Job helper).
    let transport = StdioTransport::spawn(
        alter_zero::subprocess::tool_shell().to_str().unwrap(),
        &[
            "-c".into(),
            "(for ((n=0; n<200; n++)); do printf x >> \"$MCP_HEARTBEAT\"; sleep 0.1; done) & wait"
                .into(),
        ],
        &env,
        Some(dir.path()),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::metadata(&heartbeat).map_or(0, |metadata| metadata.len()) < 3 {
        assert!(Instant::now() < deadline, "descendant never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(transport);
    std::thread::sleep(Duration::from_millis(250));
    let stopped = std::fs::metadata(&heartbeat).unwrap().len();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        std::fs::metadata(&heartbeat).unwrap().len(),
        stopped,
        "live server descendant survived Drop"
    );
}

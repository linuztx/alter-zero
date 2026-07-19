//! Integration proof of the terminal detach (`inline_tui::subprocess`,
//! docs/tools.md) against the **real built binary**: cargo hands integration
//! tests the bin's path as `CARGO_BIN_EXE_inline-tui`, whose `main` installs
//! the helper hook — so the re-exec tier here is the exact production one,
//! not a stand-in.
//!
//! Offline and deterministic (no network, no model): the executor is driven
//! directly, the way the agent loop drives it. The user-visible symptom this
//! locks out: a command that prompts on `/dev/tty` (`sudo`'s password read)
//! used to write its prompt over the TUI and block forever; detached, the
//! open fails at once and the call resolves as a prompt error.

use std::path::PathBuf;

use inline_tui::llm::exec::{RealToolExecutor, ToolExecutor};
use inline_tui::llm::tools::ToolCallRequest;
use inline_tui::stream::CancelToken;

/// The production detach helper: the TUI binary this test run just built.
fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_inline-tui"))
}

/// Run one `bash` tool call through the real executor + real helper.
fn bash(command: &str) -> inline_tui::llm::tools::ToolOutcome {
    let call = ToolCallRequest {
        id: "c".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({ "command": command, "timeout_ms": 30_000 }).to_string(),
    };
    RealToolExecutor::new()
        .with_detach_helper(Some(helper()))
        .execute(&call, &CancelToken::new(), &mut |_| {})
}

#[cfg(unix)]
#[test]
fn detached_bash_child_has_no_controlling_terminal() {
    // Opening /dev/tty is exactly what a password prompt does first. In the
    // helper's fresh session there is no controlling terminal, so the open
    // must fail — instantly — even when the test itself runs on a real tty
    // (`cargo test` in a terminal). Without `setsid` this prints GOT_TTY
    // there, which is the sudo-hijack bug.
    let out = bash("if sh -c ': < /dev/tty' 2>/dev/null; then echo GOT_TTY; else echo NO_TTY; fi");
    assert!(out.ok, "the probe itself succeeds: {}", out.output);
    assert!(
        out.output.contains("NO_TTY"),
        "the child still reaches a controlling terminal: {}",
        out.output
    );
    assert!(
        !out.output.contains("GOT_TTY"),
        "the child still reaches a controlling terminal: {}",
        out.output
    );
}

#[cfg(unix)]
#[test]
fn helper_reexec_preserves_output_and_exit_status() {
    // The re-exec is transparent plumbing: stdout and stderr ride the same
    // pipes (merged in arrival order) and the command's own exit code frames
    // the outcome — nothing about the tool contract changes.
    let out = bash("echo through-stdout; echo through-stderr 1>&2; exit 3");
    assert!(!out.ok, "exit 3 resolves the call failed");
    assert!(
        out.output.contains("Exit code: 3"),
        "the command's own status frames the result: {}",
        out.output
    );
    assert!(
        out.output.contains("through-stdout") && out.output.contains("through-stderr"),
        "both pipes survive the re-exec: {}",
        out.output
    );
}

#[cfg(unix)]
#[test]
fn the_helper_reexec_tier_detaches_on_its_own() {
    // macOS-representative: there is no `setsid` binary there, so the chain's
    // second tier — the TUI's own binary re-execed in helper mode — must
    // detach by itself. Drive that tier directly (`command_for`), bypassing
    // the `setsid` tier Linux would normally win with.
    use inline_tui::subprocess::{DetachTier, command_for};
    let helper = helper();
    let mut child = command_for(
        &DetachTier::HelperReexec(&helper),
        "if sh -c ': < /dev/tty' 2>/dev/null; then echo GOT_TTY; else echo NO_TTY; fi",
    )
    .spawn()
    .expect("spawns");
    let mut out = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut().expect("piped stdout"), &mut out)
        .expect("reads output");
    let _ = child.wait();
    assert!(
        out.contains("NO_TTY") && !out.contains("GOT_TTY"),
        "the helper tier detaches on its own: {out:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_password_prompt_fails_fast_instead_of_hanging() {
    // The user-visible regression, mechanism-for-mechanism: `read x < /dev/tty`
    // is sudo's password read. Attached, it blocks forever (the `Running…`
    // cell that never resolves, with the prompt glued to the composer);
    // detached, the open errors and the call resolves failed in well under a
    // second — no timeout involved.
    let start = std::time::Instant::now();
    let out = bash("read x < /dev/tty && echo READ_A_BYTE");
    let elapsed = start.elapsed();
    assert!(!out.ok, "the prompt read must fail: {}", out.output);
    assert!(
        !out.output.contains("READ_A_BYTE"),
        "the child read from a terminal: {}",
        out.output
    );
    assert!(
        !out.output.contains("timed out"),
        "the read blocked until the timeout — that IS the hijack bug: {}",
        out.output
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the prompt path took {elapsed:?} — it must fail fast"
    );
}

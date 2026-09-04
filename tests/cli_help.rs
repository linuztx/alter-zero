//! Integration proof of the `--help` page, the `[PROMPT]` grammar's errors
//! and the version line (`docs/cli.md`) against the **real built binary**
//! (the `detached_exec.rs` pattern — cargo hands the path as
//! `CARGO_BIN_EXE_alter-zero`). Offline and deterministic: every invocation
//! here prints and exits before the terminal boots, and the pipes cargo
//! attaches are exactly the non-terminal streams the colour rule must
//! answer with plain text.

use std::path::PathBuf;
use std::process::{Command, Output};

use alter_zero::cli::{self, HelpPage, HelpStyle};

/// The TUI binary this test run just built.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

/// Run `alter-zero …` with its stdio piped (so nothing is a terminal).
fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        // The sessions dir is never reached by these paths, but keep the
        // process off the developer's real config home regardless.
        .env(
            "ALTER_ZERO_CONFIG_DIR",
            std::env::temp_dir().join("alter-zero-cli-help-tests"),
        )
        .env_remove("NO_COLOR")
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
fn piped_help_is_the_plain_main_page_byte_for_byte() {
    for flag in ["--help", "-h"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{}", stderr(&out));
        // `println!` supplies the one trailing newline; no escapes off a tty.
        assert_eq!(
            stdout(&out),
            format!("{}\n", cli::help(HelpPage::Main, HelpStyle::Plain))
        );
        assert!(!stdout(&out).contains('\x1b'), "no escapes on a pipe");
        assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    }
}

#[test]
fn the_help_page_opens_on_the_product_name_and_names_the_prompt() {
    let text = stdout(&run(&["--help"]));
    assert_eq!(text.lines().next(), Some(alter_zero::APP_NAME));
    assert!(
        text.contains("Usage: alter-zero [OPTIONS] [PROMPT]"),
        "{text}"
    );
    assert!(text.contains("\nArguments:\n  [PROMPT]"), "{text}");
    assert!(text.contains("\nCommands:\n  mcp"), "{text}");
    assert!(text.contains("\nOptions:\n  -c, --continue"), "{text}");
}

#[test]
fn piped_mcp_help_is_the_plain_mcp_page() {
    let out = run(&["mcp", "--help"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("{}\n", cli::help(HelpPage::Mcp, HelpStyle::Plain))
    );
    assert_eq!(stdout(&out).lines().next(), Some("alter-zero mcp"));
}

#[test]
fn version_prints_the_bin_name_and_version() {
    let out = run(&["--version"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("alter-zero {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_grammar_error_is_the_clap_shaped_trailer_on_stderr_with_exit_2() {
    let out = run(&["--frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stdout(&out).is_empty(),
        "nothing on stdout: {}",
        stdout(&out)
    );
    assert_eq!(
        stderr(&out),
        format!(
            "{}\n",
            cli::usage_error(
                HelpPage::Main,
                "unrecognized argument: --frobnicate",
                HelpStyle::Plain
            )
        )
    );
    let text = stderr(&out);
    assert!(text.starts_with("error: "), "{text}");
    assert!(
        text.contains("\nUsage: alter-zero [OPTIONS] [PROMPT]\n"),
        "{text}"
    );
    assert!(
        text.ends_with("For more information, try '--help'.\n"),
        "{text}"
    );
    assert!(!text.contains('\x1b'), "no escapes on a pipe");
}

#[test]
fn two_positionals_name_the_second_with_the_quote_hint() {
    let out = run(&["fix", "the bug"]);
    assert_eq!(out.status.code(), Some(2));
    let text = stderr(&out);
    assert!(
        text.contains("error: unexpected argument: the bug"),
        "{text}"
    );
    assert!(text.contains("quote the prompt"), "{text}");
}

#[test]
fn a_prompt_with_the_bare_picker_is_refused() {
    let out = run(&["fix it", "--resume"]);
    assert_eq!(out.status.code(), Some(2));
    let text = stderr(&out);
    assert!(
        text.contains("error: --resume needs a session id"),
        "{text}"
    );
    assert!(text.contains("--continue"), "{text}");
}

#[test]
fn a_blank_prompt_is_refused() {
    let out = run(&["   "]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("error: the prompt is empty"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn an_mcp_grammar_error_carries_the_mcp_usage_block() {
    let out = run(&["mcp", "add"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        format!(
            "{}\n",
            cli::usage_error(
                HelpPage::Mcp,
                "mcp add needs a server name",
                HelpStyle::Plain
            )
        )
    );
}

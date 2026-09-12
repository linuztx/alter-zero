//! The `update` subcommand boundary (`docs/update.md`): `alter-zero update`
//! asks the repository for its newest release and, when it is newer than
//! this binary, fetches the one-line installer and runs it over this
//! binary's own directory. Run by [`super::startup::resolve_cli`] in cooked
//! mode — before the tokio runtime and the terminal, like every other CLI
//! resolution (`docs/cli.md`).
//!
//! The decisions are the pure `update` module's; this file resolves the
//! executable's own path, prints, and hands the installer its variables:
//! `ALTER_ZERO_INSTALL_DIR` (this binary's directory, so the update lands
//! over the binary that asked for it), `ALTER_ZERO_VERSION` (the release
//! just resolved, so the two halves cannot disagree about which version is
//! newest) and `ALTER_ZERO_INSTALL_BASE_URL` (the same repository the check
//! read, so a fork or a stand-in server serves both). The script is fetched
//! here and run from a file rather than piped: a fetch that fails must fail
//! loudly, and `sh` reading an empty pipe exits 0.
//!
//! `ALTER_ZERO_UPDATE_CHECK=0` silences the *automatic* check; an explicit
//! `alter-zero update` is the user's own request and runs regardless.

use std::io::Write;
use std::process::Command;

use alter_zero::update;

use super::config;

/// Run the subcommand and return the process exit code: `0` when this binary
/// is already the newest release or the installer finished, `1` when the
/// check, the fetch or the install could not be done — or when this binary
/// is a `cargo` build directory's, which is a checkout to `git pull`, not an
/// install to overwrite. The installer's own output is the progress report.
pub(crate) fn run() -> i32 {
    let bin = env!("CARGO_PKG_NAME");
    let current = env!("CARGO_PKG_VERSION");
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{bin} update: cannot tell where this binary is: {err}");
            return 1;
        }
    };
    if update::is_cargo_build_dir(&exe) {
        eprintln!(
            "{bin} update: {} is a cargo build directory, not an install — update the checkout instead:\n  git pull && cargo build --release",
            exe.display()
        );
        return 1;
    }
    let Some(dir) = exe.parent() else {
        eprintln!("{bin} update: {} has no parent directory", exe.display());
        return 1;
    };
    let repo = config::update_repo_url();
    println!("Checking {repo} for a newer release…");
    let latest = match update::fetch_latest(&repo, current) {
        Ok(latest) => latest,
        Err(err) => {
            eprintln!("{bin} update: could not check for a release: {err}");
            return 1;
        }
    };
    if !update::is_newer(current, &latest) {
        println!("{bin} {current} is the newest release.");
        return 0;
    }
    println!(
        "{bin} v{latest} is out — this is v{current}. Installing it over {}.",
        dir.display()
    );
    let script = match update::fetch_installer(&repo, current) {
        Ok(script) => script,
        Err(err) => {
            eprintln!("{bin} update: could not fetch the installer: {err}");
            return 1;
        }
    };
    let mut file = match tempfile::Builder::new()
        .prefix("alter-zero-install-")
        .suffix(".sh")
        .tempfile()
    {
        Ok(file) => file,
        Err(err) => {
            eprintln!("{bin} update: could not write the installer to a temp file: {err}");
            return 1;
        }
    };
    if let Err(err) = file
        .write_all(script.as_bytes())
        .and_then(|()| file.flush())
    {
        eprintln!("{bin} update: could not write the installer to a temp file: {err}");
        return 1;
    }
    let status = Command::new("sh")
        .arg(file.path())
        .env("ALTER_ZERO_INSTALL_DIR", dir)
        .env("ALTER_ZERO_VERSION", format!("v{latest}"))
        .env("ALTER_ZERO_INSTALL_BASE_URL", &repo)
        .status();
    match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("{bin} update: could not run sh: {err}");
            1
        }
    }
}

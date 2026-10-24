//! The safe ConPTY boundary. The emulator, transcript and input notation stay
//! shared with Unix; Windows exposes no termios or terminal input-queue probe.
//!
//! In production the executable's helper joins its own kill-on-close Job
//! **before** starting Bash in this console. The helper owns that Job until
//! Bash exits, so descendants are owned from their first instruction. A library
//! caller without a helper can start Bash directly, but only gets the weaker
//! taskkill tree fallback, not that ownership guarantee.

use std::io::{self, Read, Write};
use std::path::Path;

use portable_pty::{CommandBuilder, MasterPty, PtySize};

use super::{COLUMNS, FOREIGN_TERMINAL_VARS, ROWS, TERMINAL_ENV, utf8_locale};

/// A command in ConPTY. Keep `master` alive while draining `reader`: the two
/// pipe handles alone do not keep the pseudoconsole alive. Reading and writing
/// never share a lock or borrow, so a blocked read cannot prevent typing.
pub struct PtyProcess {
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
}

impl std::fmt::Debug for PtyProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyProcess")
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

/// Start the real helper (when installed), or Bash directly for library users,
/// inside a new ConPTY of the session's size.
///
/// # Errors
/// Git for Windows is missing, ConPTY is unavailable (Windows older than
/// 10 1809), or the command/helper could not be started. A configured helper
/// that cannot start is an error, not a silent loss of Job ownership.
pub fn spawn(detach_helper: Option<&Path>, command: &str) -> io::Result<PtyProcess> {
    crate::subprocess::validate_tool_shell()?;
    let shell = crate::subprocess::tool_shell();
    let mut cmd = match detach_helper {
        Some(helper) => {
            let mut cmd = CommandBuilder::new(helper);
            cmd.arg(crate::subprocess::DETACH_TTY_ARG);
            cmd.arg(command);
            cmd.arg(shell);
            cmd
        }
        None => {
            let mut cmd = CommandBuilder::new(shell);
            cmd.arg("-c");
            cmd.arg(command);
            cmd
        }
    };
    cmd.cwd(std::env::current_dir()?);
    let path = crate::subprocess::windows_shell_path(shell, std::env::var_os("PATH").as_deref())
        .map_err(|err| io::Error::other(format!("could not prepare Git Bash PATH: {err}")))?;
    cmd.env("PATH", path);
    for name in FOREIGN_TERMINAL_VARS {
        cmd.env_remove(name);
    }
    for (name, value) in TERMINAL_ENV {
        cmd.env(name, value);
    }
    if let Some((name, value)) = utf8_locale(|name| std::env::var(name).ok()) {
        cmd.env(name, value);
    }
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: ROWS,
            cols: COLUMNS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| io::Error::other(format!("could not allocate ConPTY: {err}")))?;
    // Acquire both streams before spawning: a stream-setup failure must never
    // leave an unmonitored process (and its descendants) behind.
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| io::Error::other(format!("could not start Bash in ConPTY: {err}")))?;
    drop(pair.slave);
    if child.process_id().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other(
            "ConPTY did not expose the child process id",
        ));
    }
    Ok(PtyProcess {
        child,
        reader,
        writer,
        master: pair.master,
    })
}

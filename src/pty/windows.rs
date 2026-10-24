//! Safe ConPTY boundary for native Windows (see `docs/windows.md`).

use std::io::{self, Read, Write};
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;

use portable_pty::{CommandBuilder, MasterPty, PtySize};

/// A Windows terminal and the process whose lifetime owns it.
pub struct PtyProcess {
    pub child: Process,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

/// ConPTY process, exposing the same monitor operations as `std::process::Child`.
pub struct Process {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Option<Box<dyn MasterPty + Send>>,
    pid: u32,
}

impl Process {
    #[must_use]
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Poll without blocking the registry's output and cancellation loop.
    ///
    /// # Errors
    /// The Windows process status could not be read.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            // ClosePseudoConsole may wait for its output pipe to drain. The
            // registry's reader remains active while the monitor closes it.
            self.master.take();
        }
        Ok(status.map(|status| ExitStatus::from_raw(status.exit_code())))
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child
            .wait()
            .map(|status| ExitStatus::from_raw(status.exit_code()))
    }
}

/// Start Bash in a private Windows pseudo-console, never the TUI's console.
///
/// # Errors
/// ConPTY is unavailable, Git for Windows Bash is missing, or spawning fails.
pub fn spawn(command: &str) -> io::Result<PtyProcess> {
    let pty = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: super::screen::ROWS,
            cols: super::screen::COLUMNS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| io::Error::other(err.to_string()))?;
    let reader = pty
        .master
        .try_clone_reader()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let writer = pty
        .master
        .take_writer()
        .map_err(|err| io::Error::other(err.to_string()))?;
    let mut cmd = CommandBuilder::new(crate::subprocess::tool_shell());
    cmd.args(["-c", command]);
    cmd.cwd(std::env::current_dir()?);
    for name in super::spawn::FOREIGN_TERMINAL_VARS {
        cmd.env_remove(name);
    }
    for (name, value) in super::spawn::TERMINAL_ENV {
        cmd.env(name, value);
    }
    if let Some((name, value)) = super::spawn::utf8_locale(|name| std::env::var(name).ok()) {
        cmd.env(name, value);
    }
    let child = pty.slave.spawn_command(cmd).map_err(|err| io::Error::other(format!(
        "could not start Git for Windows Bash in ConPTY: {err}; install Git for Windows or set ALTER_ZERO_BASH to its absolute bash.exe path"
    )))?;
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("ConPTY returned no process id"))?;
    drop(pty.slave);
    Ok(PtyProcess {
        child: Process {
            child,
            master: Some(pty.master),
            pid,
        },
        reader,
        writer,
    })
}

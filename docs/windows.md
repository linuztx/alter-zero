# Native Windows support

Windows builds use the MSVC Rust targets with a static C runtime (no Visual
C++ Redistributable required), for x64 and ARM64, on Windows 10
version 1809 or newer (ConPTY) and Windows 11. Use Windows Terminal for the TUI.
Git for Windows is required: the existing `bash` tools, hooks and `!` commands
keep their Bash syntax. Set `ALTER_ZERO_BASH` to an absolute `bash.exe` path to
override discovery; otherwise absolute PATH entries and Git's installation directories are
searched. The Windows system-directory `bash.exe` (the legacy WSL launcher) is
never selected automatically. Captured shells and ConPTY sessions prepend the
chosen Git installation's utility directories to the inherited PATH, so
PowerShell launches can run Unix utilities without loading a login profile.

## Design

The implementation uses native MSVC executables and a Windows-only
`portable-pty` adapter for ConPTY. The Unix rustix backend stays in place.
Both feed the existing session emulator, transcript, writer and background
registry. Windows cannot supply Linux `/proc` or Unix termios probes; prompt
settling uses the existing screen-based fallback. No unsafe code is added.
Captured commands run without inheriting the TUI's console. Cancellation
terminates the Windows process tree; interactive input travels through ConPTY.

WSL-only distribution would not provide a native Windows binary. MinGW would
add a second Windows C runtime/toolchain without solving terminal ownership.
MSVC plus ConPTY uses the Windows console API through a safe dependency.

State lives under `%USERPROFILE%\.alter-zero` unless `ALTER_ZERO_CONFIG_DIR`
overrides it. Unix continues to use `$HOME`. npm installs the matching
`@linuztx/alter-zero-win32-{x64,arm64}` optional package, containing
`bin/alter-zero.exe`; the launcher spawns it with the terminal inherited because
Windows has no `execve`.

The existing release workflow builds both Windows targets, verifies the PE CPU
and checksums, and stages the same binaries into npm. Archives keep the project's
`.tar.gz` layout, with `alter-zero.exe` beside LICENSE, README and CHANGELOG.
Windows includes `tar.exe`; the PowerShell installer resolves it from the
Windows system directory so Git's GNU tar cannot misinterpret drive-letter
paths. npm is the recommended installer. `install.ps1`
provides a native PowerShell install and is also used by `alter-zero update`,
with SHA-256 and archive-layout checks before replacing an executable. A running
old executable is renamed first; if Windows still holds it open, the
`alter-zero.old-*.exe` backup can be deleted after exiting the old process. Releases remain
tag-controlled and npm publishing remains behind the existing environment and
repository variable. Adding Windows support does not cut a version or publish an
existing immutable npm version again.

## Building

Install Git for Windows, Rust with its MSVC toolchain, and Visual Studio Build
Tools with the Desktop development with C++ workload (including ARM64 tools for
ARM64 builds). From PowerShell:

```powershell
cargo build --release --locked
.\target\release\alter-zero.exe --version
```

From Git Bash, the shared release tooling builds and verifies archives:

```bash
scripts/release.sh build x86_64-pc-windows-msvc
scripts/release.sh verify dist
```

On ARM64 use `aarch64-pc-windows-msvc`. Native Windows CI exercises the build,
CLI, shell and ConPTY boundaries. Linux runs the full regression suite and
release/npm fixture tests. A cross-check on Linux proves Windows code compiles;
it does not substitute for executing the Windows jobs.

## PowerShell installation

Download and run the installer from PowerShell (5.1 or newer):

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/linuztx/alter-zero/main/install.ps1 -OutFile "$env:TEMP/alter-zero-install.ps1"
& "$env:TEMP/alter-zero-install.ps1"
```

It defaults to `%LOCALAPPDATA%\Programs\Alter Zero` and prints a PATH hint.
Use `-InstallDir`, `-Version vX.Y.Z`, or the same `ALTER_ZERO_INSTALL_DIR`,
`ALTER_ZERO_VERSION`, and `ALTER_ZERO_INSTALL_BASE_URL` variables as the Unix
installer. The script verifies `SHA256SUMS`, the exact archive layout, and the
binary's version before installing. `alter-zero update` uses PowerShell for a
manual install; npm-owned installs continue to use their package manager.

Windows uses screen-based prompt settling; Linux's `/proc` wait probes and Unix
termios password detection are unavailable. herdr's Unix-socket integration and
the tmux-specific `pty_oracle` diagnostic remain Unix-only. MCP stdio servers
resolve npm's `npx.cmd`/`npm.cmd` shims on Windows, preserving argument boundaries
through Rust's Windows command handling.

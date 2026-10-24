# Native Windows support

## Scope and decision

The initial native Windows release targets **Windows 10 version 1809 or later
and Windows 11, x64**, using `x86_64-pc-windows-msvc`. It is a real Windows
executable, not a WSL launcher. Windows Terminal is recommended.

**Git for Windows is a prerequisite.** The model's `bash` tool, its session
companions, hooks and local `!` commands retain their existing POSIX shell
contract. A PowerShell or cmd command is never substituted silently for a Bash
command. Bash discovery must avoid the legacy Windows/WSL `bash.exe` launcher,
ignore relative PATH entries, and support an explicit absolute
`ALTER_ZERO_BASH` path plus standard Git for Windows installation locations.
A missing prerequisite produces actionable installation advice.

The alternatives considered were WSL-only support (does not supply a native
release) and a native PowerShell tool dialect (requires a separate tool,
permission, hooks and replay design, rather than reinterpreting existing Bash
calls). The compatibility-first Git Bash approach is the implementation
assumption after presenting these alternatives while the requester was
unavailable. Windows ARM64 is not advertised until native execution coverage
is available; x64 emulation is not ARM64 validation.

## Runtime boundaries

- Keep the existing Unix rustix PTY, detach/helper tiers and Linux process
  introspection. Windows code is selected at compile time, not by weakening
  Unix behavior or disabling warnings.
- Use crossterm's native Windows cursor query before creating the sole
  `EventStream`. Do not start an extra stdin reader. Native Windows startup
  does not send Unix DSR/graphics queries; unsupported cell-pixel dimensions
  remain unknown and image rendering uses its existing fallback.
- Interactive commands use **ConPTY** through a safe wrapper. Their reader
  and writer remain separate from the user's terminal, and the existing VT
  screen, transcript, key notation, reporting and bounded-output cores stay
  shared. Unix-only termios, input-queue and `/proc` capabilities remain
  unknown rather than fabricated on Windows.
- Cancellation must stop descendants, not only a shell process or an empty
  non-Unix signal stub. The helper joins a kill-on-close Job before spawning
  Bash and retains it across background adoption. A caught console-control
  handler protects the helper from Ctrl+C, allowing Bash to handle the event
  instead of prematurely closing its Job. This is not the inheritable
  ignore-Ctrl+C attribute. No local unsafe FFI or `pre_exec` is allowed.
- Resolve the profile consistently for configuration, user skills, trust and
  path display. Preserve `ALTER_ZERO_CONFIG_DIR` and existing Unix `$HOME`
  behavior; normal native Windows launches use the Windows user profile.
  Scratch data uses the OS temporary directory, not a hardcoded `/tmp`.
- The native clipboard remains arboard. herdr's Unix-domain socket reporting
  is optional and remains unavailable on Windows. Native-only capabilities
  need native tests; Linux compilation alone cannot establish them.

## Checkpoint and file-access limits

Checkpoints remain **off by default**. The shared lexical scope guard covers
filesystem roots, the resolved user profile and its ancestors, the app's
state directory, and the native temporary directory itself. Its fixed system
and shared-directory lists are Unix/macOS lists, **not a Windows system-path
denylist**: it does not discover and categorically refuse the Windows or
Program Files locations from `SystemRoot` / `ProgramFiles`. Enable checkpoints
only in a project you intend to snapshot and restore, never in a system or
shared installation directory. The snapshot budget and filesystem permission
errors are not substitutes for that scope protection.

The initial Windows implementation also **does not create, tighten or verify
owner-only Windows ACLs** for state or temporary files. Files use the access
policy inherited from their filesystem and parent directories. Unix `0600`
claims elsewhere apply to Unix only; a per-profile or session-specific path
is not itself an access-control guarantee. This matters for plaintext API
keys, `secrets.json`, MCP OAuth tokens, input history and raw task logs. Keep
configuration and scratch data in locations with suitably restricted ACLs,
including when overriding their paths or using shared/network directories.

## Distribution and update policy

Ship a checksum-verified Windows ZIP with `alter-zero.exe`, LICENSE, README.md
and CHANGELOG.md under the archive's stem. Keep the four existing Unix
`.tar.gz` assets and their layouts unchanged. Verification must reject a
mismatched or malformed PE image, wrong CPU, unsafe archive member or unknown
target rather than skipping validation.

Add a Windows npm platform package with `bin/alter-zero.exe`, preserving the
no-install-script policy. The launcher directly spawns the executable with
inherited stdio and unchanged argument boundaries. Windows cannot replace
Node's process with `execve`; its launcher retains a Node parent, unlike the
modern Unix path. Unsupported Windows architectures receive a clear error.

A separate PowerShell installer uses the release's SHA256SUMS and a per-user
writable installation directory. It never changes machine-wide execution
policy. Running/locked executables must not be reported as successfully
replaced. Package-manager installations remain owned by their manager.
Unmanaged Windows updates initially instruct the user to close Alter Zero and
rerun the PowerShell installer: synchronous Unix rename-over-running-binary
self-update is not a Windows guarantee.

## Verification and publishing

Add a required native Windows MSVC CI lane and release build. Test the
packaged executable and installer on Windows, not only PE headers on Linux.
Keep the pinned Rust toolchain, warnings-as-errors, unsafe prohibition,
Linux glibc floor, immutable released versions and existing publication
permission boundaries. New Windows npm packages require the same registry
permissions/trusted-publisher setup as the existing platform packages.

Local Linux checks and native Windows CI results are reported separately.
No version bump, release tag or actual publication is part of implementing
support. The feature branch is `feat/native-windows-support`.

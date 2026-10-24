# Alter Zero

An open-source AI coding agent for your terminal, written in Rust. It works
directly in your project — reading files, making edits, running commands and
checking results while you follow along — and the same tools serve
cybersecurity research and everyday automation. This package installs the
prebuilt release binary for your machine.

```bash
npm install -g @linuztx/alter-zero
alter-zero
```

Runs on **Linux** (x86_64, arm64; glibc 2.35 or newer), **macOS** (Intel,
Apple silicon), and **native Windows x64** (Windows 10 1809+ / Windows 11).
Windows requires **Git for Windows**, not WSL; Windows Terminal is recommended.
Windows ARM64 and 32-bit Windows are not supported.

npm picks the one binary package that matches your machine, with **no install
scripts or extra downloads**. On modern Unix Node.js replaces itself with the
binary; on Windows it directly starts `alter-zero.exe` with inherited terminal
I/O and stays as its parent. No command shell reinterprets your arguments.

Without a configured provider, Alter Zero opens an offline demo you can
explore. Run `/login` to connect a subscription, an API key, or a local model
server, then `/model` to choose a model.

**Update** with `npm install -g @linuztx/alter-zero@latest`, and **uninstall**
with `npm uninstall -g @linuztx/alter-zero`. On Windows, close running Alter
Zero instances before updating or uninstalling: Windows locks running `.exe`
files. Update this installation with npm, not the standalone installer.

The [repository](https://github.com/linuztx/alter-zero) has the full README,
the other ways to install, the changelog, and what the app sends over the
network ([`TELEMETRY.md`](https://github.com/linuztx/alter-zero/blob/main/TELEMETRY.md)).
Licensed under Apache-2.0.

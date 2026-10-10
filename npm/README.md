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

Runs on **Linux** (x86_64, arm64; glibc 2.35 or newer) and **macOS** (Intel,
Apple silicon). Node.js is only the installer here: npm picks the one binary
package that matches your machine, and the `alter-zero` command replaces
itself with that binary when it starts.

Without a configured provider, Alter Zero opens an offline demo you can
explore. Run `/login` to connect a subscription, an API key, or a local model
server, then `/model` to choose a model.

**Update** with `npm install -g @linuztx/alter-zero@latest`, and **uninstall**
with `npm uninstall -g @linuztx/alter-zero`.

The [repository](https://github.com/linuztx/alter-zero) has the full README,
the other ways to install, the changelog, and what the app sends over the
network ([`TELEMETRY.md`](https://github.com/linuztx/alter-zero/blob/main/TELEMETRY.md)).
Licensed under Apache-2.0.

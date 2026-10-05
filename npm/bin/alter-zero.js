#!/usr/bin/env node
// The `alter-zero` command an npm install puts on PATH (docs/npm.md).
//
// npm installs this launcher and exactly one of its four optional
// dependencies — @linuztx/alter-zero-{linux,darwin}-{x64,arm64}, each
// holding that platform's release binary and chosen by its os/cpu/libc
// fields — and this file finds that binary and becomes it. Nothing is
// downloaded and nothing runs at install time, so --ignore-scripts, pnpm
// and bun install it like any other package.
//
// "Becomes it" is literal where Node can (process.execve, Node 22.15+ and
// 23.11+): this process is replaced by the binary, which then owns the
// terminal, its signals, its job control and its exit status as if it had
// been run directly, and no Node process stays resident beside a TUI that
// may idle in a terminal all day. A failed execve cannot be caught — Node
// aborts — so `locate` checks everything it could fail on first. An older
// Node runs the binary as a child instead, relaying signals and mirroring
// how it ended.
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const COMMAND = 'alter-zero';
const REPO = 'https://github.com/linuztx/alter-zero';
const SOURCE_BUILD = `${REPO}#build-from-source`;

// `${process.platform}-${process.arch}` → the release target its platform
// package carries. These are the four targets the release builds
// (docs/release.md); scripts/release/lib.sh's npm_platform maps them back
// and package.json's optionalDependencies names one package per key — the
// tests hold all three together.
const PLATFORMS = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

function platformKey(platform, arch) {
  const key = `${platform}-${arch}`;
  return Object.prototype.hasOwnProperty.call(PLATFORMS, key) ? key : null;
}

// `@linuztx/alter-zero` + `linux-x64` → `@linuztx/alter-zero-linux-x64`.
function platformPackage(name, key) {
  return `${name}-${key}`;
}

// The program interpreter an ELF binary names (PT_INTERP): the dynamic
// loader the kernel must find for an exec to succeed. null for a static
// binary, anything that is not a little-endian ELF64 (a Mach-O on macOS, a
// script) or a file that cannot be read. Three small reads, never the file.
function elfInterpreter(file) {
  let fd;
  try {
    fd = fs.openSync(file, 'r');
    const head = Buffer.alloc(64);
    if (fs.readSync(fd, head, 0, 64, 0) < 64) return null;
    // \x7fELF, ELFCLASS64, ELFDATA2LSB — both release CPUs.
    if (head.readUInt32BE(0) !== 0x7f454c46 || head[4] !== 2 || head[5] !== 1) return null;
    const phoff = Number(head.readBigUInt64LE(0x20));
    const phentsize = head.readUInt16LE(0x36);
    const phnum = head.readUInt16LE(0x38);
    if (phentsize < 56 || phnum === 0 || phnum > 512) return null;
    const table = Buffer.alloc(phentsize * phnum);
    if (fs.readSync(fd, table, 0, table.length, phoff) < table.length) return null;
    for (let i = 0; i < phnum; i += 1) {
      const entry = i * phentsize;
      if (table.readUInt32LE(entry) !== 3) continue; // PT_INTERP
      const offset = Number(table.readBigUInt64LE(entry + 8));
      const size = Number(table.readBigUInt64LE(entry + 32));
      if (size === 0 || size > 4096) return null;
      const name = Buffer.alloc(size);
      if (fs.readSync(fd, name, 0, size, offset) < size) return null;
      const end = name.indexOf(0);
      return name.toString('latin1', 0, end === -1 ? size : end);
    }
    return null;
  } catch (_) {
    return null;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

// glibc or musl, read off a process report the way npm reads it to honour a
// package's `libc` field; null when it says neither.
function libcFamily(report) {
  if (!report) return null;
  if (report.header && report.header.glibcVersionRuntime) return 'glibc';
  const libs = Array.isArray(report.sharedObjects) ? report.sharedObjects : [];
  return libs.some((lib) => /(^|\/)(ld-musl-|libc\.musl-)/.test(lib)) ? 'musl' : null;
}

// The package manager that installed this launcher, read off its own real
// directory by the rule the binary applies to its own path (src/update.rs's
// package_manager), so a launcher error and the binary's update card name
// the same one: pnpm's store is node_modules/.pnpm, Bun's global packages
// live under ~/.bun, Yarn classic's under …/yarn/global/node_modules, and
// anything else — a checkout run through `npm link` included — is npm's.
function packageManager(dir) {
  const parts = dir.split(/[\\/]+/);
  if (parts.includes('.pnpm')) return 'pnpm';
  if (parts.includes('.bun')) return 'bun';
  const yarn = parts.some((part, i) => part === 'yarn' && parts[i + 1] === 'global' && parts[i + 2] === 'node_modules');
  return yarn ? 'yarn' : 'npm';
}

// The command that installs `name` globally with `manager`: the binary's
// update command (src/update.rs's update_command) without the `@latest`.
function installCommand(manager, name) {
  switch (manager) {
    case 'pnpm':
      return `pnpm add -g ${name}`;
    case 'yarn':
      return `yarn global add ${name}`;
    case 'bun':
      return `bun add -g ${name}`;
    default:
      return `npm install -g ${name}`;
  }
}

const reinstall = (command) => `Reinstall it:\n\n  ${command}\n`;
const needsGlibc = (why) =>
  `${why}\nThe Linux build of Alter Zero needs glibc 2.35 or newer, which musl-based\n` +
  `distributions such as Alpine do not have. Build from source instead:\n${SOURCE_BUILD}\n`;

// Where the binary is — { binary } — or why it cannot be run — { error },
// the message the user reads. Every way the exec could fail is checked
// here, before it: Node aborts on a failed execve rather than throwing.
// `report` (process.report.getReport, a few milliseconds) is read only to
// explain a failure; `manager` names the package manager whose command the
// advice to reinstall gives.
function locate({ name, platform, arch, resolve, report, manager = 'npm' }) {
  const key = platformKey(platform, arch);
  if (key === null) {
    return {
      error:
        `there is no Alter Zero build for ${platform} ${arch}.\n` +
        'It runs on Linux and macOS, on x64 and arm64 — on Windows, inside WSL.\n',
    };
  }
  const pkg = platformPackage(name, key);
  const again = reinstall(installCommand(manager, name));
  const onMusl = () => platform === 'linux' && libcFamily(report()) === 'musl';
  let manifest;
  try {
    // require.resolve walks node_modules up from this file, so it finds the
    // package wherever the package manager put it: nested under this one
    // (npm), hoisted beside it (bun), or in pnpm's store.
    manifest = resolve(`${pkg}/package.json`);
  } catch (_) {
    if (onMusl()) return { error: needsGlibc(`${pkg} was not installed: this system uses musl.`) };
    return {
      error:
        `${pkg}, the package holding the ${key} binary, is not installed.\n` +
        `npm installs it beside ${name} as an optional dependency, and skips it\n` +
        'when optional dependencies are turned off (--omit=optional, --no-optional).\n' +
        again,
    };
  }
  const binary = path.join(path.dirname(manifest), 'bin', COMMAND);
  let stat;
  try {
    stat = fs.statSync(binary);
  } catch (_) {
    stat = null;
  }
  if (!stat || !stat.isFile()) {
    return { error: `${binary} is missing — the ${pkg} install is incomplete.\n${again}` };
  }
  try {
    fs.accessSync(binary, fs.constants.X_OK);
  } catch (_) {
    // A package manager that unpacked the file without its mode: give it
    // back the 0755 it was published with, if this user may.
    try {
      fs.chmodSync(binary, 0o755);
      fs.accessSync(binary, fs.constants.X_OK);
    } catch (err) {
      return { error: `${binary} is not executable and could not be made so (${err.code || err.message}).\n${again}` };
    }
  }
  if (platform === 'linux') {
    const loader = elfInterpreter(binary);
    if (loader !== null && !fs.existsSync(loader)) {
      const why = `this system has no ${loader}, the dynamic loader ${binary} needs.`;
      if (onMusl()) return { error: needsGlibc(why) };
      return {
        error:
          `${why}\nThe Linux build of Alter Zero is built for glibc-based distributions\n` +
          `(glibc 2.35 or newer). Build from source instead:\n${SOURCE_BUILD}\n`,
      };
    }
  }
  return { binary };
}

// Say why the launcher cannot go on, and exit 1. One synchronous write
// straight to fd 2: process.stderr writes to a pipe asynchronously, so a
// message the pipe has no room for yet would be queued, and the exit right
// after it would drop the queue.
function fail(message) {
  try {
    fs.writeSync(2, `${COMMAND}: ${message}`);
  } catch (_) {
    // nowhere to say it; the exit status still does
  }
  process.exit(1);
}

// The signals whose default action dumps core. The fallback never re-raises
// one: dying of it here would dump this Node process's core too — a crash
// report about node, not the binary.
const CORE_SIGNALS = new Set([
  'SIGABRT',
  'SIGBUS',
  'SIGFPE',
  'SIGILL',
  'SIGQUIT',
  'SIGSEGV',
  'SIGSYS',
  'SIGTRAP',
  'SIGXCPU',
  'SIGXFSZ',
]);

// Become the binary (execve), or — on a Node without it — run it as a
// child that owns the terminal, relay the signals sent to this process,
// and end the way it ended.
function launch(binary, args, { execve = process.execve, spawn = require('child_process').spawn } = {}) {
  if (typeof execve === 'function') {
    // Node ignores SIGPIPE and SIGXFSZ, and an ignored signal stays ignored
    // across exec, while a caught one is reset to its default. Catching both
    // hands the binary the dispositions a direct run would have.
    for (const signal of ['SIGPIPE', 'SIGXFSZ']) process.on(signal, () => {});
    execve.call(process, binary, [COMMAND, ...args], process.env);
    return; // not reached: this process is the binary now
  }
  const child = spawn(binary, args, { stdio: 'inherit', argv0: COMMAND });
  // What a process manager or a closing terminal sends to this process;
  // without a listener, Node would die of it and leave the binary running.
  const relayed = ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];
  const relay = (signal) => {
    try {
      child.kill(signal);
    } catch (_) {
      // the child is already gone; its exit decides how this one ends
    }
  };
  for (const signal of relayed) process.on(signal, relay);
  child.on('error', (err) => fail(`could not run ${binary}: ${err.message}\n`));
  child.on('exit', (code, signal) => {
    for (const s of relayed) process.removeListener(s, relay);
    if (signal) {
      // Die of the same signal, so a shell sees what the binary saw — except
      // one that dumps core, and SIGUSR1, which Node answers by starting its
      // inspector. Not re-raised, or still here after the kill (an ignored
      // signal): exit as a shell reports a death by signal.
      if (signal !== 'SIGUSR1' && !CORE_SIGNALS.has(signal)) process.kill(process.pid, signal);
      process.exit(128 + (os.constants.signals[signal] || 0));
    }
    process.exit(code === null ? 1 : code);
  });
}

function main() {
  const { name } = require('../package.json');
  const found = locate({
    name,
    platform: process.platform,
    arch: process.arch,
    resolve: require.resolve,
    report: () => process.report.getReport(),
    manager: packageManager(__dirname),
  });
  if (found.error) fail(found.error);
  launch(found.binary, process.argv.slice(2));
}

if (require.main === module) {
  main();
}

module.exports = {
  PLATFORMS,
  platformKey,
  platformPackage,
  elfInterpreter,
  libcFamily,
  packageManager,
  installCommand,
  locate,
  launch,
};

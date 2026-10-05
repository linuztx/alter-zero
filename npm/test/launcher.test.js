// The npm launcher's tests (docs/npm.md): `node --test` from npm/.
//
// The pure half — the platform table, the ELF loader read, the libc read,
// `locate`'s verdicts — is driven directly; the impure half — exec, the
// spawn fallback, signals and exit statuses — by running the real launcher
// in a child process over a fake install tree whose "binary" is a shell
// script (or /bin/sh itself), so nothing here needs a Rust build.
'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { after, describe, test } = require('node:test');

const ROOT = path.join(__dirname, '..');
const LAUNCHER = path.join(ROOT, 'bin', 'alter-zero.js');
const MANIFEST = require('../package.json');
const launcher = require(LAUNCHER);

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'alter-zero-npm-test-'));
after(() => fs.rmSync(scratch, { recursive: true, force: true }));
let counter = 0;
const tempDir = () => {
  counter += 1;
  const dir = path.join(scratch, String(counter));
  fs.mkdirSync(dir, { recursive: true });
  return dir;
};

const HOST = launcher.platformKey(process.platform, process.arch);

// An install the way npm lays one out globally: the launcher package with
// its platform package nested under it. `binary` is the platform package's
// bin/alter-zero — file contents, or { symlink: target } — and null leaves
// the platform package out entirely.
function fakeInstall({ key = HOST, binary, mode = 0o755 } = {}) {
  const root = tempDir();
  const pkg = path.join(root, 'node_modules', ...MANIFEST.name.split('/'));
  fs.mkdirSync(path.join(pkg, 'bin'), { recursive: true });
  fs.copyFileSync(path.join(ROOT, 'package.json'), path.join(pkg, 'package.json'));
  fs.copyFileSync(LAUNCHER, path.join(pkg, 'bin', 'alter-zero.js'));
  let bin = null;
  if (binary !== null) {
    const plat = path.join(pkg, 'node_modules', ...launcher.platformPackage(MANIFEST.name, key).split('/'));
    fs.mkdirSync(path.join(plat, 'bin'), { recursive: true });
    fs.writeFileSync(
      path.join(plat, 'package.json'),
      JSON.stringify({ name: launcher.platformPackage(MANIFEST.name, key), version: MANIFEST.version }),
    );
    bin = path.join(plat, 'bin', 'alter-zero');
    if (binary && binary.symlink) {
      fs.symlinkSync(binary.symlink, bin);
    } else if (binary !== undefined) {
      fs.writeFileSync(bin, binary, { mode });
      fs.chmodSync(bin, mode);
    }
  }
  return { root, pkg, bin, entry: path.join(pkg, 'bin', 'alter-zero.js') };
}

// A resolver rooted in a fake install, standing in for the launcher's own
// require.resolve.
const resolverFor = (install) => (id) => require.resolve(id, { paths: [path.join(install.pkg, 'bin')] });

const glibc = () => ({ header: { glibcVersionRuntime: '2.39' }, sharedObjects: [] });
const musl = () => ({ header: {}, sharedObjects: ['/lib/ld-musl-x86_64.so.1'] });

// A minimal little-endian ELF64 image: the header, one program header table
// of `types.length` entries, and — for a PT_INTERP entry — the loader path.
function elfImage(interp, { types = [6, 3, 1] } = {}) {
  const phoff = 64;
  const phentsize = 56;
  const pathOffset = phoff + phentsize * types.length;
  const body = Buffer.from(`${interp || ''}\0`, 'latin1');
  const image = Buffer.alloc(pathOffset + body.length);
  image.writeUInt32BE(0x7f454c46, 0);
  image[4] = 2; // ELFCLASS64
  image[5] = 1; // ELFDATA2LSB
  image[6] = 1;
  image.writeUInt16LE(2, 0x10); // ET_EXEC
  image.writeUInt16LE(62, 0x12); // EM_X86_64
  image.writeBigUInt64LE(BigInt(phoff), 0x20);
  image.writeUInt16LE(64, 0x34);
  image.writeUInt16LE(phentsize, 0x36);
  image.writeUInt16LE(types.length, 0x38);
  types.forEach((type, i) => {
    const at = phoff + i * phentsize;
    image.writeUInt32LE(type, at);
    if (type === 3) {
      image.writeBigUInt64LE(BigInt(pathOffset), at + 8);
      image.writeBigUInt64LE(BigInt(body.length), at + 32);
    }
  });
  body.copy(image, pathOffset);
  return image;
}

describe('the platform table', () => {
  test('maps the four release platforms and nothing else', () => {
    assert.equal(launcher.platformKey('linux', 'x64'), 'linux-x64');
    assert.equal(launcher.platformKey('linux', 'arm64'), 'linux-arm64');
    assert.equal(launcher.platformKey('darwin', 'x64'), 'darwin-x64');
    assert.equal(launcher.platformKey('darwin', 'arm64'), 'darwin-arm64');
    for (const [platform, arch] of [
      ['win32', 'x64'],
      ['linux', 'ia32'],
      ['linux', 'riscv64'],
      ['freebsd', 'x64'],
      ['android', 'arm64'],
      ['constructor', ''],
    ]) {
      assert.equal(launcher.platformKey(platform, arch), null, `${platform} ${arch}`);
    }
  });

  test('agrees with package.json: one optional dependency per platform, each at this version', () => {
    const expected = Object.keys(launcher.PLATFORMS)
      .map((key) => launcher.platformPackage(MANIFEST.name, key))
      .sort();
    assert.deepEqual(Object.keys(MANIFEST.optionalDependencies).sort(), expected);
    for (const [name, version] of Object.entries(MANIFEST.optionalDependencies)) {
      assert.equal(version, MANIFEST.version, `${name} is pinned exactly at the launcher's version`);
    }
  });

  test('names the Rust target each platform package carries', () => {
    assert.deepEqual(launcher.PLATFORMS, {
      'darwin-arm64': 'aarch64-apple-darwin',
      'darwin-x64': 'x86_64-apple-darwin',
      'linux-arm64': 'aarch64-unknown-linux-gnu',
      'linux-x64': 'x86_64-unknown-linux-gnu',
    });
  });

  test('installs only where a platform package exists', () => {
    assert.deepEqual([...MANIFEST.os].sort(), ['darwin', 'linux']);
    assert.deepEqual([...MANIFEST.cpu].sort(), ['arm64', 'x64']);
  });
});

describe('the ELF loader read', () => {
  test('reads PT_INTERP', () => {
    const file = path.join(tempDir(), 'elf');
    fs.writeFileSync(file, elfImage('/lib64/ld-linux-x86-64.so.2'));
    assert.equal(launcher.elfInterpreter(file), '/lib64/ld-linux-x86-64.so.2');
  });

  test('is null for a static binary, a script, a short or missing file', () => {
    const dir = tempDir();
    const cases = {
      static: elfImage(null, { types: [6, 1] }),
      script: Buffer.from('#!/bin/sh\necho hi\n'),
      short: elfImage('/lib/ld.so').subarray(0, 40),
      truncatedTable: elfImage('/lib/ld.so').subarray(0, 100),
      bigEndian: (() => {
        const image = elfImage('/lib/ld.so');
        image[5] = 2;
        return image;
      })(),
    };
    for (const [name, bytes] of Object.entries(cases)) {
      fs.writeFileSync(path.join(dir, name), bytes);
      assert.equal(launcher.elfInterpreter(path.join(dir, name)), null, name);
    }
    assert.equal(launcher.elfInterpreter(path.join(dir, 'missing')), null);
  });

  test('finds the loader of this very Node on Linux', { skip: process.platform !== 'linux' }, () => {
    const interp = launcher.elfInterpreter(process.execPath);
    // The official builds are dynamically linked; a static Node has none.
    if (interp !== null) assert.ok(fs.existsSync(interp), interp);
  });
});

describe('the libc read', () => {
  test('tells glibc from musl the way npm does', () => {
    assert.equal(launcher.libcFamily(glibc()), 'glibc');
    assert.equal(launcher.libcFamily(musl()), 'musl');
    assert.equal(launcher.libcFamily({ header: {}, sharedObjects: ['/usr/lib/libc.musl-aarch64.so.1'] }), 'musl');
    assert.equal(launcher.libcFamily({ header: {}, sharedObjects: [] }), null);
    assert.equal(launcher.libcFamily(undefined), null);
  });
});

describe('locate', () => {
  const locateIn = (install, overrides = {}) =>
    launcher.locate({
      name: MANIFEST.name,
      platform: 'linux',
      arch: 'x64',
      resolve: resolverFor(install),
      report: glibc,
      ...overrides,
    });

  test('finds the platform package binary', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: '#!/bin/sh\n' });
    assert.deepEqual(locateIn(install), { binary: install.bin });
  });

  test('refuses an unsupported platform, naming the supported ones', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: '#!/bin/sh\n' });
    const { error } = locateIn(install, { platform: 'win32', arch: 'x64' });
    assert.match(error, /win32 x64/);
    assert.match(error, /Linux and macOS/);
    assert.match(error, /WSL/);
  });

  test('explains a platform package npm did not install, and how to get it', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: null });
    const { error } = locateIn(install);
    assert.match(error, /@linuztx\/alter-zero-linux-x64/);
    assert.match(error, /optional/);
    assert.match(error, /npm install -g @linuztx\/alter-zero/);
  });

  test('on musl, says the build needs glibc instead of suggesting a reinstall that cannot work', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: null });
    const { error } = locateIn(install, { report: musl });
    assert.match(error, /musl/);
    assert.match(error, /glibc/);
    assert.match(error, /build-from-source/);
    assert.doesNotMatch(error, /npm install -g/);
  });

  test('reads the libc only to explain a failure', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: '#!/bin/sh\n' });
    let reads = 0;
    locateIn(install, {
      report: () => {
        reads += 1;
        return glibc();
      },
    });
    assert.equal(reads, 0, 'process.report costs milliseconds; the happy path never asks');
  });

  test('explains an install whose binary is missing', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: undefined });
    const { error } = locateIn(install);
    assert.match(error, /is missing/);
    assert.match(error, /npm install -g @linuztx\/alter-zero/);
  });

  test('restores the executable bit a package manager dropped', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: '#!/bin/sh\n', mode: 0o644 });
    assert.deepEqual(locateIn(install), { binary: install.bin });
    assert.ok(fs.statSync(install.bin).mode & 0o100, 'chmodded back to executable');
  });

  test('refuses a binary whose dynamic loader this system does not have', () => {
    const install = fakeInstall({ key: 'linux-x64', binary: elfImage('/nonexistent/ld-linux-x86-64.so.2') });
    const { error } = locateIn(install);
    assert.match(error, /\/nonexistent\/ld-linux-x86-64\.so\.2/);
    assert.match(error, /glibc/);
    const onMusl = locateIn(install, { report: musl }).error;
    assert.match(onMusl, /musl/);
  });

  test('accepts a binary whose loader exists', () => {
    const loader = path.join(tempDir(), 'ld.so');
    fs.writeFileSync(loader, '');
    const install = fakeInstall({ key: 'linux-x64', binary: elfImage(loader) });
    assert.deepEqual(locateIn(install), { binary: install.bin });
  });
});

// The launcher run for real, as a child process, the way a shell runs it.
function run(entry, args, options = {}) {
  return childProcess.spawnSync(process.execPath, [entry, ...args], {
    encoding: 'utf8',
    timeout: 20000,
    ...options,
  });
}

const echoArgs = '#!/bin/sh\nfor a in "$@"; do printf "[%s]\\n" "$a"; done\necho "env=$ALTER_ZERO_NPM_TEST" >&2\nexit "${FAKE_EXIT:-0}"\n';

describe('the launcher', { skip: HOST === null && 'no platform package for this machine' }, () => {
  test('passes every argument through verbatim', () => {
    const install = fakeInstall({ binary: echoArgs });
    const result = run(install.entry, ['--version', 'two words', '', 'ünï', '--', '-c']);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '[--version]\n[two words]\n[]\n[ünï]\n[--]\n[-c]\n');
  });

  test("exits with the binary's status, its stderr and environment intact", () => {
    const install = fakeInstall({ binary: echoArgs });
    const result = run(install.entry, [], { env: { ...process.env, FAKE_EXIT: '7', ALTER_ZERO_NPM_TEST: 'kept' } });
    assert.equal(result.status, 7);
    assert.equal(result.stderr, 'env=kept\n');
  });

  test('names the binary `alter-zero` in its argv[0]', () => {
    const install = fakeInstall({ binary: { symlink: '/bin/sh' } });
    const result = run(install.entry, ['-c', 'echo "$0"']);
    assert.equal(result.stdout, 'alter-zero\n', result.stderr);
  });

  test(
    'becomes the binary, leaving no Node process behind',
    { skip: typeof process.execve !== 'function' && 'this Node has no process.execve' },
    () => {
      const install = fakeInstall({ binary: { symlink: '/bin/sh' } });
      const result = run(install.entry, ['-c', 'echo "$$"']);
      assert.equal(result.stdout.trim(), String(result.pid), 'the binary runs in the launcher\'s own process');
    },
  );

  test(
    "hands the binary a direct run's signal dispositions, not Node's",
    { skip: (process.platform !== 'linux' || typeof process.execve !== 'function') && 'needs /proc and process.execve' },
    () => {
      const install = fakeInstall({ binary: { symlink: '/bin/sh' } });
      const result = run(install.entry, ['-c', 'grep SigIgn /proc/self/status']);
      // Node ignores SIGPIPE and SIGXFSZ, and an ignored signal survives exec.
      assert.equal(result.stdout, 'SigIgn:\t0000000000000000\n', result.stderr);
    },
  );

  test('explains a missing platform package and exits 1', () => {
    const install = fakeInstall({ binary: null });
    const result = run(install.entry, ['--version']);
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, /^alter-zero: /);
    assert.match(result.stderr, /npm install -g @linuztx\/alter-zero/);
  });
});

// The fallback for a Node without process.execve: the binary runs as a
// child, and the launcher relays signals and mirrors how it ended.
describe('the spawn fallback', { skip: HOST === null && 'no platform package for this machine' }, () => {
  const harness = (install) => {
    const file = path.join(install.root, 'harness.js');
    fs.writeFileSync(
      file,
      `require(${JSON.stringify(install.entry)}).launch(${JSON.stringify(install.bin)}, process.argv.slice(2), { execve: null });\n`,
    );
    return file;
  };

  test('mirrors the exit status and passes arguments through', () => {
    const install = fakeInstall({ binary: echoArgs });
    const result = run(harness(install), ['a b', ''], { env: { ...process.env, FAKE_EXIT: '3' } });
    assert.equal(result.status, 3, result.stderr);
    assert.equal(result.stdout, '[a b]\n[]\n');
  });

  test('runs the binary as a child named `alter-zero`', () => {
    const install = fakeInstall({ binary: { symlink: '/bin/sh' } });
    const result = run(harness(install), ['-c', 'echo "$0 $PPID"']);
    assert.equal(result.stdout, `alter-zero ${result.pid}\n`, result.stderr);
  });

  test('relays SIGTERM to the binary and dies of it too', async () => {
    const install = fakeInstall({ binary: '#!/bin/sh\necho ready\nexec sleep 30\n' });
    const child = childProcess.spawn(process.execPath, [harness(install)], { stdio: ['ignore', 'pipe', 'inherit'] });
    await new Promise((resolve) => child.stdout.once('data', resolve));
    child.kill('SIGTERM');
    const [code, signal] = await new Promise((resolve) => child.on('exit', (...end) => resolve(end)));
    assert.equal(code, null);
    assert.equal(signal, 'SIGTERM');
  });

  test('dies of the signal that killed the binary', () => {
    const install = fakeInstall({ binary: '#!/bin/sh\nkill -HUP $$\nsleep 5\n' });
    const result = run(harness(install), []);
    assert.equal(result.signal, 'SIGHUP');
  });

  test('exits 128+n for a signal Node will not die of', () => {
    const install = fakeInstall({ binary: '#!/bin/sh\nkill -USR1 $$\nsleep 5\n' });
    const result = run(harness(install), []);
    assert.equal(result.status, 128 + os.constants.signals.SIGUSR1);
  });
});

describe('the package', () => {
  test('ships one file beside package.json, README and LICENSE', () => {
    assert.deepEqual(MANIFEST.files, ['bin/alter-zero.js']);
    assert.deepEqual(MANIFEST.bin, { 'alter-zero': 'bin/alter-zero.js' });
  });

  test('the launcher is a node script, executable in the repository', () => {
    assert.ok(fs.readFileSync(LAUNCHER, 'utf8').startsWith('#!/usr/bin/env node\n'));
    assert.ok(fs.statSync(LAUNCHER).mode & 0o100);
  });

  test('refuses to be published from npm/ itself', () => {
    const result = childProcess.spawnSync(process.execPath, [path.join(ROOT, 'scripts', 'refuse-direct-publish.js')], {
      encoding: 'utf8',
      env: { ...process.env, npm_config_dry_run: '' },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /scripts\/release\.sh npm/);
    const dry = childProcess.spawnSync(process.execPath, [path.join(ROOT, 'scripts', 'refuse-direct-publish.js')], {
      encoding: 'utf8',
      env: { ...process.env, npm_config_dry_run: 'true' },
    });
    assert.equal(dry.status, 0, 'a dry run publishes nothing, so it may look');
  });
});

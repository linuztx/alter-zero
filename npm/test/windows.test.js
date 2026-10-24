'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { after, test } = require('node:test');
const launcherPath = path.resolve(__dirname, '../bin/alter-zero.js');
const launcher = require(launcherPath);
const manifest = require('../package.json');
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'alter-zero-windows-npm-'));
after(() => fs.rmSync(scratch, { recursive: true, force: true }));

function install(binary = Buffer.from('MZ')) {
  const root = fs.mkdtempSync(path.join(scratch, 'install-'));
  const pkg = path.join(root, 'node_modules', ...manifest.name.split('/'));
  const platform = path.join(pkg, 'node_modules', ...`${manifest.name}-win32-x64`.split('/'));
  fs.mkdirSync(path.join(platform, 'bin'), { recursive: true });
  fs.mkdirSync(path.join(pkg, 'bin'), { recursive: true });
  fs.copyFileSync(launcherPath, path.join(pkg, 'bin/alter-zero.js'));
  fs.writeFileSync(path.join(pkg, 'package.json'), JSON.stringify(manifest));
  fs.writeFileSync(path.join(platform, 'package.json'), JSON.stringify({ name: `${manifest.name}-win32-x64` }));
  const exe = path.join(platform, 'bin/alter-zero.exe');
  fs.writeFileSync(exe, binary, { mode: 0o644 });
  return { pkg, exe, entry: path.join(pkg, 'bin/alter-zero.js') };
}

test('Windows resolves bin/alter-zero.exe without requiring Unix execute bits', () => {
  const fixture = install();
  const found = launcher.locate({
    name: manifest.name, platform: 'win32', arch: 'x64',
    resolve: (id) => require.resolve(id, { paths: [fixture.pkg] }),
    report: () => assert.fail('Windows does not read a libc report'),
  });
  assert.deepEqual(found, { binary: fixture.exe });
  if (process.platform !== 'win32') assert.equal(fs.statSync(fixture.exe).mode & 0o111, 0);
});

test('Windows spawns the exe directly with inherited stdio, no execve or POSIX signal handlers', () => {
  const harness = path.join(scratch, 'spawn.js');
  fs.writeFileSync(harness, `
    const assert = require('node:assert/strict');
    const child = new (require('node:events').EventEmitter)();
    const signals = ['SIGPIPE', 'SIGXFSZ', 'SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];
    const before = signals.map(s => process.listenerCount(s));
    process.kill = () => assert.fail('no signal reraising on Windows');
    require(${JSON.stringify(launcherPath)}).launch('C:\\\\Program Files\\\\Alter Zero\\\\alter-zero.exe', ['two words', '', '& |', 'ü', 'a"b'], {
      platform: 'win32',
      execve: () => assert.fail('no execve on Windows, even when the API exists'),
      spawn: (binary, args, options) => {
        console.log(JSON.stringify({binary, args, options}));
        return child;
      },
    });
    assert.deepEqual(signals.map(s => process.listenerCount(s)), before);
    child.emit('exit', 7, null);
  `);
  const result = childProcess.spawnSync(process.execPath, [harness], { encoding: 'utf8' });
  assert.equal(result.status, 7, result.stderr);
  const call = JSON.parse(result.stdout);
  assert.match(call.binary, /alter-zero\.exe$/);
  assert.deepEqual(call.args, ['two words', '', '& |', 'ü', 'a"b']);
  assert.equal(call.options.stdio, 'inherit');
  assert.equal(Boolean(call.options.shell), false);
});

test('Windows never reraises a child signal as a POSIX signal', () => {
  const harness = path.join(scratch, 'signal.js');
  fs.writeFileSync(harness, `
    const child = new (require('node:events').EventEmitter)();
    process.kill = () => { throw new Error('must not reraise'); };
    require(${JSON.stringify(launcherPath)}).launch('alter-zero.exe', [], {
      platform: 'win32', execve: null, spawn: () => child,
    });
    child.emit('exit', null, 'SIGTERM');
  `);
  const result = childProcess.spawnSync(process.execPath, [harness], { encoding: 'utf8' });
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.stderr, '');
});

test('native Windows launcher preserves arguments and exit code through a real executable',
  { skip: process.platform !== 'win32' }, () => {
    const fixture = install(fs.readFileSync(process.execPath));
    const args = ['two words', '', 'ü', 'a"b', '& | < >', 'trailing\\'];
    const result = childProcess.spawnSync(process.execPath, [fixture.entry, '-e',
      'console.log(JSON.stringify(process.argv.slice(1))); process.exit(7)', '--', ...args], { encoding: 'utf8' });
    assert.equal(result.status, 7, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), args);
  });

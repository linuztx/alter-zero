'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

test('Windows launcher preserves arguments, stdio and exit status without execve',
  { skip: process.platform !== 'win32' }, () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'alter zero launcher '));
    try {
      const manifest = require('../package.json');
      const pkg = path.join(root, 'node_modules', ...manifest.name.split('/'));
      const platform = path.join(pkg, 'node_modules', ...`${manifest.name}-win32-${process.arch}`.split('/'));
      fs.mkdirSync(path.join(pkg, 'bin'), { recursive: true });
      fs.mkdirSync(path.join(platform, 'bin'), { recursive: true });
      fs.copyFileSync(path.join(__dirname, '../bin/alter-zero.js'), path.join(pkg, 'bin/alter-zero.js'));
      fs.writeFileSync(path.join(pkg, 'package.json'), JSON.stringify(manifest));
      fs.writeFileSync(path.join(platform, 'package.json'), JSON.stringify({ name: `${manifest.name}-win32-${process.arch}` }));
      // Node is a real PE executable, unlike the Unix launcher's shell fixtures.
      fs.copyFileSync(process.execPath, path.join(platform, 'bin/alter-zero.exe'));
      const args = ['with spaces', '世界', '& echo unintended', '"quoted"', 'C:\\Users\\Ada Lovelace\\'];
      const run = spawnSync(process.execPath, [path.join(pkg, 'bin/alter-zero.js'), '-e',
        'process.stdout.write(JSON.stringify(process.argv.slice(1))); process.stderr.write("stderr preserved"); process.exitCode = 7;',
        '--', ...args], { encoding: 'utf8', timeout: 15000 });
      assert.ifError(run.error);
      assert.equal(run.status, 7, run.stderr);
      assert.deepEqual(JSON.parse(run.stdout), args);
      assert.equal(run.stderr, 'stderr preserved');
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });

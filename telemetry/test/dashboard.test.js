import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Script } from 'node:vm';

import { renderDashboard, shapeStats } from '../src/lib.js';
import { DASHBOARD_BOOT, DASHBOARD_SCRIPT } from '../src/dashboard-client.js';

function fixture(overrides = {}) {
  return shapeStats({
    today: '2026-09-10',
    days: 3,
    daily: [{ day: '2026-09-10', users: 6 }],
    newInstalls: [{ day: '2026-09-10', installs: 2 }],
    countries: [{ country: 'PH', users: 6 }],
    versions: [{ version: '0.1.0', users: 6 }],
    oses: [{ os: 'linux', users: 6 }],
    platforms: [{ platform: 'ubuntu', os_version: '24.04', users: 6 }],
    totals: { users_7d: 6, users_30d: 8, installs: 12 },
    generatedAt: '2026-09-10T12:00:00.000Z',
    ...overrides,
  });
}

function scripts(html) {
  return [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)]
    .map((match) => ({ attributes: match[1].trim(), source: match[2] }));
}

function bootWith(storage) {
  const document = { documentElement: { dataset: {} } };
  const context = { document };
  if (storage !== undefined) context.localStorage = storage;
  new Script(DASHBOARD_BOOT).runInNewContext(context);
  return document.documentElement.dataset.theme;
}

test('the theme preference restores only a supported palette', () => {
  for (const theme of ['system', 'mocha', 'macchiato', 'frappe', 'latte', 'nord', 'dracula']) {
    const storage = { getItem(key) {
      assert.equal(key, 'alter-zero-telemetry-theme');
      return theme;
    } };
    assert.equal(bootWith(storage), theme);
  }
  for (const saved of [null, '', 'removed-theme', '__proto__', 'constructor', '<svg onload=alert(1)>']) {
    assert.equal(bootWith({ getItem: () => saved }), 'system', String(saved));
  }
});

test('unavailable browser storage leaves a usable default theme', () => {
  assert.equal(bootWith(undefined), 'system', 'storage may be absent');
  assert.equal(bootWith({ getItem() { throw new Error('Storage access denied'); } }), 'system');
});

test('dashboard interactions are self-contained, syntactically valid static scripts', () => {
  const html = renderDashboard(fixture());
  assert.deepEqual(scripts(html), [
    { attributes: 'id="theme-init"', source: DASHBOARD_BOOT },
    { attributes: 'id="dashboard-script"', source: DASHBOARD_SCRIPT },
  ]);
  for (const { source } of scripts(html)) {
    assert.doesNotThrow(() => new Script(source));
    assert.doesNotMatch(source, /\b(?:fetch|WebSocket|EventSource|XMLHttpRequest)\s*\(|\.sendBeacon\s*\(/,
      'viewing and interacting does not contact another service');
  }
  assert.doesNotMatch(html,
    /<(?:script|iframe|img|source|audio|video|object|embed|image|use|link)\b[^>]*\b(?:src|href|data|poster)\s*=\s*["'](?:https?:)?\/\//i,
    'scripts, imagery, fonts, styles and the map ship with the document');
  assert.doesNotMatch(html, /@import\b|url\(\s*["']?(?:https?:)?\/\//i);
});

test('untrusted data cannot escape attributes or enter an executable dashboard script', () => {
  const attack = '"><img src=x onerror=alert(1)></script><script>alert(2)</script>&';
  const stats = fixture();
  stats.generated_at = attack;
  stats.window.since = attack;
  stats.today.day = attack;
  stats.daily[0].day = attack;
  stats.countries = [{ country: attack, users: attack }];
  stats.versions = [{ version: attack, users: attack }];
  stats.os = [{ os: attack, users: attack }];
  stats.platforms = [{ platform: attack, version: attack, users: attack }];
  const html = renderDashboard(stats, { token: attack });
  assert.ok(!html.includes(attack), 'hostile markup is never printed verbatim');
  assert.ok(html.includes('&lt;img'), 'text from the table is still visible and escaped');
  assert.deepEqual(scripts(html), scripts(renderDashboard(fixture())),
    'database fields and the navigation token never become executable source');
  assert.equal((html.match(/<script\b/gi) ?? []).length, 2);
  assert.doesNotMatch(html, /<img\s+src=x|<script>alert/);
});

test('all window and JSON routes preserve the supplied token as one encoded query value', () => {
  const token = 'quotes" & equals= slash/ hash# plus+ ü';
  const html = renderDashboard(fixture(), { token });
  const links = [...html.matchAll(/\bhref="([^"]*)"/g)]
    .map((match) => match[1].replaceAll('&amp;', '&'))
    .filter((href) => /^\/(?:[?#]|$|v1\/stats(?:[?#]|$))/.test(href))
    .map((href) => new URL(href, 'https://collector.example'));
  assert.ok(links.some((url) => url.pathname === '/v1/stats'));
  for (const days of [3, 7, 30, 90, 365]) {
    assert.ok(links.some((url) => url.pathname === '/' && url.searchParams.get('days') === String(days)));
  }
  for (const url of links) {
    assert.equal(url.origin, 'https://collector.example');
    assert.equal(url.searchParams.get('token'), token);
    assert.deepEqual([...url.searchParams.keys()].sort(), ['days', 'token']);
  }
  assert.doesNotMatch(scripts(html).map(({ source }) => source).join('\n'), /quotes/);
});

test('empty and partial documents stay readable with finite visual geometry', () => {
  const empty = fixture({ daily: [], newInstalls: [], countries: [], versions: [], oses: [], platforms: [], totals: {} });
  for (const stats of [empty, {}, { daily: null, countries: null }, { window: null, today: null, totals: null }]) {
    const html = renderDashboard(stats);
    assert.ok(html.startsWith('<!doctype html>'));
    assert.match(html, /no pings|nothing yet|no activity|no data|empty/i);
    assert.doesNotMatch(html, /(?:style|d|cx|cy|r|width|height)="[^"]*(?:NaN|Infinity)/);
    assert.doesNotMatch(html, /<[^>]*class="[^"]*\bmap-marker\b/,
      'an empty country list never invents a geographic marker');
  }
});

test('unlocatable country rows remain available without a fabricated map position', () => {
  const html = renderDashboard(fixture({ countries: [
    { country: 'PH', users: 6 },
    { country: 'ZZ', users: 3 },
    { country: 'QQ', users: 2 },
  ] }));
  const markers = [...html.matchAll(/<[^>]*class="[^"]*\bmap-marker\b[^>]*>/g)].map(([tag]) => tag);
  assert.ok(markers.some((tag) => /\bdata-country="PH"/.test(tag)));
  assert.ok(markers.every((tag) => !/\bdata-country="(?:ZZ|QQ)"/.test(tag)));
  assert.match(html, /Unknown/);
  assert.match(html, /QQ/);
});

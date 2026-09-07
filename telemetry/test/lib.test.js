// The collector's pure half (docs/telemetry.md), tested with Node's built-in
// runner: `node --test test/` from `telemetry/`. No framework, no install.
import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEFAULT_WINDOW_DAYS,
  MAX_BODY_BYTES,
  MAX_WINDOW_DAYS,
  PAYLOAD_VERSION,
  UNKNOWN_COUNTRY,
  authorized,
  daysBefore,
  normalizeCountry,
  renderDashboard,
  shapeStats,
  utcDay,
  validatePing,
  windowDays,
} from '../src/lib.js';

const GOOD = {
  v: 1,
  id: '6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980',
  version: '0.1.0',
  os: 'linux',
  arch: 'x86_64',
};

test('a well-formed ping validates to exactly its four stored fields', () => {
  const result = validatePing(GOOD);
  assert.equal(result.ok, true);
  assert.deepEqual(result.ping, {
    id: GOOD.id,
    version: '0.1.0',
    os: 'linux',
    arch: 'x86_64',
  });
  assert.equal(PAYLOAD_VERSION, 1);
  assert.equal(MAX_BODY_BYTES, 1024);
});

test('extra fields are dropped, never stored', () => {
  // A future client — or anyone — sending more is answered with the shape we
  // keep, so the table can never grow a column by accident.
  const result = validatePing({ ...GOOD, hostname: 'laptop', ip: '1.2.3.4' });
  assert.equal(result.ok, true);
  assert.deepEqual(Object.keys(result.ping).sort(), ['arch', 'id', 'os', 'version']);
});

test('every malformed ping is refused with a reason', () => {
  const cases = [
    [null, 'not an object'],
    ['string', 'a string'],
    [[], 'an array'],
    [{ ...GOOD, v: 2 }, 'a future version'],
    [{ ...GOOD, v: '1' }, 'a stringly version'],
    [{ ...GOOD, id: GOOD.id.toUpperCase() }, 'an uppercase id'],
    [{ ...GOOD, id: GOOD.id.slice(1) }, 'a short id'],
    [{ ...GOOD, id: `${GOOD.id}0` }, 'a long id'],
    [{ ...GOOD, id: 'zz1c2a4d9e0b7c3a5f8e1d2c4b6a7980' }, 'a non-hex id'],
    [{ ...GOOD, id: 42 }, 'a numeric id'],
    [{ ...GOOD, version: '' }, 'an empty version'],
    [{ ...GOOD, version: '0.1.0 <script>' }, 'a version with markup'],
    [{ ...GOOD, version: 'x'.repeat(33) }, 'a 33-char version'],
    [{ ...GOOD, os: 'Linux' }, 'an uppercase os'],
    [{ ...GOOD, os: 'mac os' }, 'an os with a space'],
    [{ ...GOOD, arch: 'x'.repeat(17) }, 'a 17-char arch'],
    [{ v: 1 }, 'missing fields'],
  ];
  for (const [body, label] of cases) {
    const result = validatePing(body);
    assert.equal(result.ok, false, label);
    assert.equal(typeof result.error, 'string', label);
    assert.ok(result.error.length > 0, label);
  }
});

test('the country is the two-letter code the edge saw, else ZZ', () => {
  assert.equal(normalizeCountry('PH'), 'PH');
  assert.equal(normalizeCountry('de'), 'DE', 'upper-cased');
  assert.equal(normalizeCountry(' us '), 'US', 'trimmed');
  // Cloudflare's own "unknown" and Tor markers, an absent value, and junk.
  assert.equal(normalizeCountry('XX'), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry('T1'), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry(undefined), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry(null), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry(''), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry('USA'), UNKNOWN_COUNTRY);
  assert.equal(normalizeCountry('<b>'), UNKNOWN_COUNTRY);
  assert.equal(UNKNOWN_COUNTRY, 'ZZ');
});

test('days are UTC dates and arithmetic crosses month, year and leap boundaries', () => {
  assert.equal(utcDay(new Date('2026-09-06T23:59:59Z')), '2026-09-06');
  assert.equal(utcDay(new Date('2026-09-06T00:00:00Z')), '2026-09-06');
  assert.equal(daysBefore('2026-09-06', 0), '2026-09-06');
  assert.equal(daysBefore('2026-09-06', 6), '2026-08-31');
  assert.equal(daysBefore('2026-01-01', 1), '2025-12-31');
  assert.equal(daysBefore('2028-03-01', 1), '2028-02-29', 'leap day');
  assert.equal(daysBefore('2026-09-06', 29), '2026-08-08');
});

test('the ?days= window clamps to 1..365 and defaults when absent or junk', () => {
  assert.equal(windowDays(null), DEFAULT_WINDOW_DAYS);
  assert.equal(windowDays(undefined), DEFAULT_WINDOW_DAYS);
  assert.equal(windowDays('abc'), DEFAULT_WINDOW_DAYS);
  assert.equal(windowDays(''), DEFAULT_WINDOW_DAYS);
  assert.equal(windowDays('7'), 7);
  assert.equal(windowDays('0'), 1);
  assert.equal(windowDays('-4'), 1);
  assert.equal(windowDays('9999'), MAX_WINDOW_DAYS);
  assert.equal(DEFAULT_WINDOW_DAYS, 30);
  assert.equal(MAX_WINDOW_DAYS, 365);
});

test('stats are shaped with every day of the window present, zero-filled and ordered', () => {
  const stats = shapeStats({
    today: '2026-09-06',
    days: 3,
    daily: [
      { day: '2026-09-06', users: 4 },
      { day: '2026-09-04', users: 1 },
    ],
    newInstalls: [{ day: '2026-09-06', installs: 2 }],
    countries: [
      { country: 'PH', users: 3 },
      { country: 'ZZ', users: 1 },
    ],
    versions: [{ version: '0.1.0', users: 4 }],
    oses: [{ os: 'linux', users: 4 }],
    totals: { users_7d: 5, users_30d: 9, installs: 12 },
    generatedAt: '2026-09-06T12:00:00.000Z',
  });
  assert.deepEqual(stats.window, { days: 3, since: '2026-09-04', until: '2026-09-06' });
  assert.deepEqual(stats.today, { day: '2026-09-06', users: 4 });
  assert.deepEqual(stats.totals, { users_7d: 5, users_30d: 9, installs: 12 });
  assert.deepEqual(stats.daily, [
    { day: '2026-09-04', users: 1, new_installs: 0 },
    { day: '2026-09-05', users: 0, new_installs: 0 },
    { day: '2026-09-06', users: 4, new_installs: 2 },
  ]);
  assert.deepEqual(stats.countries, [
    { country: 'PH', users: 3 },
    { country: 'ZZ', users: 1 },
  ]);
  assert.deepEqual(stats.versions, [{ version: '0.1.0', users: 4 }]);
  assert.deepEqual(stats.os, [{ os: 'linux', users: 4 }]);
  assert.equal(stats.generated_at, '2026-09-06T12:00:00.000Z');
});

test('an empty table shapes to zeros, not to an error', () => {
  const stats = shapeStats({
    today: '2026-09-06',
    days: 2,
    daily: [],
    newInstalls: [],
    countries: [],
    versions: [],
    oses: [],
    totals: { users_7d: 0, users_30d: 0, installs: 0 },
    generatedAt: 'now',
  });
  assert.deepEqual(stats.today, { day: '2026-09-06', users: 0 });
  assert.equal(stats.daily.length, 2);
  assert.ok(stats.daily.every((d) => d.users === 0 && d.new_installs === 0));
});

test('the dashboard shows the numbers and escapes what it prints', () => {
  const stats = shapeStats({
    today: '2026-09-06',
    days: 2,
    daily: [{ day: '2026-09-06', users: 7 }],
    newInstalls: [],
    countries: [{ country: 'PH', users: 7 }],
    // Validation keeps markup out of the table, but the renderer must be safe
    // on its own — a hand-edited row must not become a script.
    versions: [{ version: '<script>alert(1)</script>', users: 7 }],
    oses: [{ os: 'linux', users: 7 }],
    totals: { users_7d: 7, users_30d: 7, installs: 7 },
    generatedAt: '2026-09-06T12:00:00.000Z',
  });
  const html = renderDashboard(stats);
  assert.ok(html.startsWith('<!doctype html>'), 'a whole page');
  assert.ok(html.includes('Alter Zero'), 'names the app');
  assert.ok(html.includes('2026-09-06'), 'the day');
  assert.ok(html.includes('PH'), 'the country');
  assert.ok(!html.includes('<script>'), 'markup in a value is escaped');
  assert.ok(html.includes('&lt;script&gt;'), 'and shown as text');
  assert.ok(!/<script[\s>]/i.test(html), 'the page itself runs no script');
  assert.ok(!html.includes('http://') && !html.includes('https://'), 'no external assets');
});

test('the dashboard token gates when set and opens when not', () => {
  assert.equal(authorized(undefined, null, null), true, 'no token configured: open');
  assert.equal(authorized('', null, null), true, 'an empty secret is no secret');
  assert.equal(authorized('s3cret', null, null), false);
  assert.equal(authorized('s3cret', 'Bearer s3cret', null), true);
  assert.equal(authorized('s3cret', 'bearer s3cret', null), true, 'scheme is case-insensitive');
  assert.equal(authorized('s3cret', 'Bearer wrong', null), false);
  assert.equal(authorized('s3cret', null, 's3cret'), true, 'the ?token= form');
  assert.equal(authorized('s3cret', null, 'wrong'), false);
  assert.equal(authorized('s3cret', 'Basic s3cret', null), false, 'only bearer');
});

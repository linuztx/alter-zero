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
  ACCEPTED_PAYLOAD_VERSIONS,
  authorized,
  byteLength,
  countryLabel,
  dashboardHref,
  daysBefore,
  normalizeCountry,
  renderDashboard,
  shapeStats,
  utcDay,
  validatePing,
  windowDays,
} from '../src/lib.js';

const GOOD = {
  v: 2,
  id: '6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980',
  version: '0.1.0',
  os: 'linux',
  arch: 'x86_64',
  distro: 'ubuntu',
  os_version: '24.04',
};

test('a well-formed ping validates to exactly its six stored fields', () => {
  const result = validatePing(GOOD);
  assert.equal(result.ok, true);
  assert.deepEqual(result.ping, {
    id: GOOD.id,
    version: '0.1.0',
    os: 'linux',
    arch: 'x86_64',
    distro: 'ubuntu',
    os_version: '24.04',
  });
  assert.equal(PAYLOAD_VERSION, 2);
  assert.equal(MAX_BODY_BYTES, 1024);
});

test('a v1 client is still counted, and simply reports no distribution', () => {
  // Bumping the payload version must not stop counting everyone who has not
  // updated: a collector that only spoke the newest shape would answer every
  // older install a 400 and read as "our users all left".
  assert.deepEqual(ACCEPTED_PAYLOAD_VERSIONS, [1, 2]);
  const v1 = validatePing({ v: 1, id: GOOD.id, version: '0.1.0', os: 'linux', arch: 'x86_64' });
  assert.equal(v1.ok, true);
  assert.equal(v1.ping.distro, '', 'nothing to file it under, not a guess');
  assert.equal(v1.ping.os_version, '');
});

test('the platform version is a version number, or nothing at all', () => {
  for (const v of ['24.04', '39', '15.3.1', '12', '3.20.3', '24.11pre']) {
    assert.equal(validatePing({ ...GOOD, os_version: v }).ping?.os_version, v, v);
  }
  // A rolling release names none, and neither does a platform this client
  // cannot ask: absent is a blank column, not a 400.
  const { os_version: _none, ...rolling } = GOOD;
  assert.equal(validatePing({ ...rolling, distro: 'arch' }).ping.os_version, '');
  assert.equal(validatePing({ ...GOOD, os_version: null }).ping.os_version, '');
  for (const bad of ['', '24 04', '.24', '9'.repeat(17), '<b>', 24.04, {}]) {
    assert.equal(validatePing({ ...GOOD, os_version: bad }).ok, false, JSON.stringify(bad));
  }
});

test('the distribution is one os-release ID, or nothing at all', () => {
  for (const distro of ['ubuntu', 'arch', 'nixos', 'opensuse-leap', 'sles_sap', 'centos.stream', 'debian11']) {
    assert.equal(validatePing({ ...GOOD, distro }).ping?.distro, distro, distro);
  }
  // macOS and Windows send no such key; absent is a blank column, not a 400.
  const { distro: _dropped, ...noDistro } = GOOD;
  assert.equal(validatePing(noDistro).ping.distro, '');
  assert.equal(validatePing({ ...GOOD, distro: null }).ping.distro, '', 'null reads as absent');
  // Anything that is not one is refused rather than stored: this column is
  // grouped on, and one machine's junk would be a row of its own forever.
  for (const bad of ['Ubuntu', 'ubuntu 22.04', '', 'x'.repeat(33), '<script>', 42, {}]) {
    assert.equal(validatePing({ ...GOOD, distro: bad }).ok, false, JSON.stringify(bad));
  }
});

test('extra fields are dropped, never stored', () => {
  // A future client — or anyone — sending more is answered with the shape we
  // keep, so the table can never grow a column by accident.
  const result = validatePing({ ...GOOD, hostname: 'laptop', ip: '1.2.3.4' });
  assert.equal(result.ok, true);
  assert.deepEqual(Object.keys(result.ping).sort(), [
    'arch',
    'distro',
    'id',
    'os',
    'os_version',
    'version',
  ]);
});

test('every malformed ping is refused with a reason', () => {
  const cases = [
    [null, 'not an object'],
    ['string', 'a string'],
    [[], 'an array'],
    [{ ...GOOD, v: 3 }, 'a future version'],
    [{ ...GOOD, v: '2' }, 'a stringly version'],
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

// --- The redesigned dashboard, and the bugs the old one carried ---

test('a body is measured in UTF-8 bytes, not UTF-16 code units', () => {
  // The cap is stated in bytes (`MAX_BODY_BYTES`), and `String.length` is not
  // bytes: 1024 three-byte characters are 3 KiB of body that a `.length`
  // check waves through as "1024".
  assert.equal(byteLength('abc'), 3);
  assert.equal(byteLength(''), 0);
  assert.equal(byteLength('é'), 2);
  assert.equal(byteLength('日'), 3);
  assert.equal(byteLength('😀'), 4);
  const cjk = '日'.repeat(1024);
  assert.equal(cjk.length, 1024, 'a .length check would call this 1 KiB');
  assert.ok(byteLength(cjk) > MAX_BODY_BYTES, 'and it is really 3 KiB');
});

test('a country code becomes a flag and a name, and ZZ reads as unknown', () => {
  const ph = countryLabel('PH');
  assert.equal(ph.code, 'PH');
  assert.equal(ph.name, 'Philippines');
  assert.equal(ph.flag, '\u{1F1F5}\u{1F1ED}', 'the regional-indicator pair');

  const zz = countryLabel(UNKNOWN_COUNTRY);
  assert.equal(zz.name, 'Unknown', 'ZZ is not a country, and its flag is not one either');
  assert.notEqual(zz.flag, '\u{1F1FF}\u{1F1FF}');

  // A code `Intl` cannot name reads as itself rather than as nothing — and
  // gets no flag: the regional-indicator pair for a region that does not
  // exist renders as a tofu box, which says less than a globe does.
  assert.equal(countryLabel('QQ').name, 'QQ');
  assert.equal(countryLabel('QQ').flag, countryLabel(UNKNOWN_COUNTRY).flag);
  assert.notEqual(countryLabel('DE').flag, countryLabel(UNKNOWN_COUNTRY).flag);
  // Junk can only get in by hand, and the page shows it rather than hiding it.
  assert.equal(countryLabel('USA').code, 'USA');
  assert.equal(countryLabel('USA').name, 'Unknown');

  // A hand-edited or future row must never throw or inject.
  for (const junk of ['QQ', 'ph', 'USA', '', '<b>', null, 42]) {
    const label = countryLabel(junk);
    assert.equal(typeof label.name, 'string', String(junk));
    assert.equal(typeof label.flag, 'string', String(junk));
    assert.ok(label.name.length > 0, String(junk));
  }
});

test('dashboard links carry the ?token= the reader arrived with, and nothing else', () => {
  // Following the old page's "add ?days=90" advice dropped the token and
  // answered 401 — the one navigation the dashboard suggests must work.
  assert.equal(dashboardHref({ days: 90 }), '/?days=90');
  assert.equal(dashboardHref({ days: 90, token: 's3cret' }), '/?days=90&token=s3cret');
  assert.equal(dashboardHref({ days: 7, token: '', path: '/v1/stats' }), '/v1/stats?days=7');
  assert.equal(dashboardHref({ days: 30, token: 'a b&c=d' }), '/?days=30&token=a%20b%26c%3Dd');
  assert.equal(dashboardHref({ days: 30, token: null }), '/?days=30');
});

test('the dashboard renders a chart, the panels, and every window link', () => {
  const html = renderDashboard(
    shapeStats({
      today: '2026-09-06',
      days: 3,
      daily: [
        { day: '2026-09-06', users: 7 },
        { day: '2026-09-05', users: 0 },
      ],
      newInstalls: [{ day: '2026-09-06', installs: 2 }],
      countries: [
        { country: 'PH', users: 7 },
        { country: 'ZZ', users: 1 },
      ],
      versions: [{ version: '0.1.0', users: 7 }],
      oses: [{ os: 'linux', users: 7 }],
      // As the query returns them — `os_version` is the column's name.
      platforms: [
        { platform: 'ubuntu', os_version: '24.04', users: 4 },
        { platform: 'arch', os_version: '', users: 2 },
        { platform: 'macos', os_version: '15.3.1', users: 1 },
      ],
      totals: { users_7d: 7, users_30d: 7, installs: 9 },
      generatedAt: '2026-09-06T12:00:00.000Z',
    }),
    { token: 's3cret' },
  );
  assert.ok(html.includes('Platforms'), 'the platform panel is on the page');
  assert.ok(html.includes('ubuntu') && html.includes('24.04'), 'a distribution and its version');
  assert.ok(html.includes('macos') && html.includes('15.3.1'), 'and a Mac beside it');
  assert.ok(html.includes('arch'), 'a rolling release shows with no version rather than not at all');
  assert.ok(html.includes('Philippines'), 'a country is named, not just coded');
  assert.ok(html.includes('Unknown'), 'and ZZ says so');
  assert.ok(html.includes('href="/?days=90&amp;token=s3cret"'), 'the window links keep the token');
  assert.ok(html.includes('href="/v1/stats?days=3&amp;token=s3cret"'), 'so does the JSON link');
  assert.ok(html.includes('rel="icon"'), 'no /favicon.ico round trip');
  assert.ok(!/<script[\s>]/i.test(html), 'still no script');
  assert.ok(!html.includes('http://') && !html.includes('https://'), 'still no external assets');
});

test('the dashboard draws nothing above the baseline for a day with no users', () => {
  const zeroes = renderDashboard(
    shapeStats({
      today: '2026-09-06',
      days: 2,
      daily: [],
      newInstalls: [],
      countries: [],
      versions: [],
      oses: [],
      totals: { users_7d: 0, users_30d: 0, installs: 0 },
      generatedAt: 'now',
    }),
  );
  // The old page gave every empty day a 1px bar, so "nobody" and "one
  // person" looked the same.
  assert.ok(!/--h: *[1-9]/.test(zeroes), 'no column has a height');
  assert.ok(/nothing|no pings|empty/i.test(zeroes), 'and the page says so');
});

test('the renderer coerces every number it prints, whatever it is handed', () => {
  // shapeStats already numbers these, but the renderer is the last line of
  // defence for a hand-made document or a future caller.
  const html = renderDashboard({
    generated_at: '<b>now</b>',
    window: { days: '3" onmouseover="steal()', since: '2026-09-04', until: '2026-09-06' },
    today: { day: '2026-09-06', users: '<b>7</b>' },
    totals: { users_7d: 'x', users_30d: 5, installs: 9 },
    daily: [{ day: '2026-09-06', users: '<i>4</i>', new_installs: 'nope' }],
    countries: [{ country: '<script>alert(1)</script>', users: '<b>1</b>' }],
    versions: [],
    os: [],
  });
  // Every one of these arrived inside a value; none may reach the page as
  // markup. (The page's own chrome uses <b> for a KPI, so the check is on the
  // injected fragments rather than on the tag.)
  for (const injected of ['<b>7</b>', '<i>4</i>', '<b>now</b>', '<script>', 'onmouseover']) {
    assert.ok(!html.includes(injected), injected);
  }
  assert.ok(!/<script[\s>]/i.test(html));
  assert.ok(html.includes('&lt;script&gt;'), 'a string value is shown as text instead');
  assert.ok(html.includes('&lt;b&gt;now&lt;/b&gt;'), 'and so is the timestamp');
  assert.ok(/<b>0<\/b>/.test(html), 'a number that is not one reads as zero, not as markup');
});

test('a long panel folds its tail into one line rather than running off the page', () => {
  const codes = ['PH','US','DE','IN','BR','GB','JP','FR','CA','AU','NL','SE','IT','ES','PL','KR','SG','MX','ZA','ZZ'];
  const html = renderDashboard(
    shapeStats({
      today: '2026-09-06',
      days: 7,
      daily: [],
      newInstalls: [],
      countries: codes.map((country, i) => ({ country, users: 20 - i })),
      versions: [],
      oses: [],
      totals: { users_7d: 1, users_30d: 1, installs: 1 },
      generatedAt: '2026-09-06T00:00:00.000Z',
    }),
  );
  assert.equal((html.match(/<tr><th scope="row"><span class="name">/g) ?? []).length, 12);
  assert.ok(html.includes('+ 8 more'), 'and says how many it is not showing');
  assert.ok(html.includes('/v1/stats'), 'pointing at where all of them are');
  // Every panel table has three columns; an empty or folded row must span all
  // of them or the rule under it stops short.
  assert.ok(!html.includes('colspan="2"'));
});

test('a stats document missing half its fields renders a page, not a 500', () => {
  // `/` has one job when the table misbehaves: still answer. Every field the
  // renderer reads is optional to it.
  for (const stats of [{}, { daily: null, countries: null }, { window: null, totals: null, today: null }]) {
    const html = renderDashboard(stats);
    assert.ok(html.startsWith('<!doctype html>'), JSON.stringify(stats));
    assert.ok(html.includes('Alter Zero'), JSON.stringify(stats));
  }
});

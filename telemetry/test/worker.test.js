// The collector's *routes* (docs/telemetry.md), over a fake D1 — the half
// `lib.test.js` cannot reach. The pure functions can all be right while the
// handler binds the INSERT in the wrong order, drops the edge's country, or
// answers a bad body with a 500; each of those produces a table that looks
// plausible and counts the wrong thing, so the request path gets its own
// tests. No network and no wrangler: `fetch` is called directly with a
// `Request` and a stub `env`.
import { test } from 'node:test';
import assert from 'node:assert/strict';

import worker from '../src/worker.js';

const ID = '6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980';
const PING = {
  v: 2,
  id: ID,
  version: '0.1.0',
  os: 'linux',
  arch: 'x86_64',
  distro: 'ubuntu',
  os_version: '24.04',
};

/** A D1 stand-in that records every statement and its bound arguments. */
function fakeDb(rows = {}) {
  const calls = [];
  const statement = (sql) => ({
    sql,
    args: [],
    bind(...args) {
      this.args = args;
      return this;
    },
    async run() {
      calls.push({ sql, args: this.args });
      return { success: true };
    },
    async first() {
      calls.push({ sql, args: this.args });
      return rows.first ?? null;
    },
  });
  return {
    calls,
    prepare: (sql) => statement(sql),
    // `batch` answers each statement with the canned result set the test
    // wants, in the order handleStats asks for them.
    async batch(statements) {
      for (const s of statements) calls.push({ sql: s.sql, args: s.args });
      return (rows.batch ?? []).map((results) => ({ results }));
    },
  };
}

/** A ping request carrying `body`, from `country` at the edge. */
function pingRequest(body, country = 'PH') {
  const request = new Request('https://c.example/v1/ping', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: typeof body === 'string' ? body : JSON.stringify(body),
  });
  // `request.cf` is Cloudflare's own property; Request has no setter for it.
  Object.defineProperty(request, 'cf', { value: country ? { country } : {} });
  return request;
}

const statsRows = () => [
  [{ day: '2026-09-06', users: 3 }],
  [{ day: '2026-09-06', installs: 1 }],
  [{ country: 'PH', users: 3 }],
  [{ version: '0.1.0', users: 3 }],
  [{ os: 'linux', users: 3 }],
  [{ platform: 'ubuntu', os_version: '24.04', users: 2 }],
  [{ n: 3 }],
  [{ n: 9 }],
  [{ n: 12 }],
];

test('a good ping is stored once, keyed on the day and id, with the edge country', async () => {
  const db = fakeDb();
  const response = await worker.fetch(pingRequest(PING), { DB: db });
  assert.equal(response.status, 204);
  assert.equal(db.calls.length, 1);
  const { sql, args } = db.calls[0];
  // INSERT OR IGNORE is what makes a second ping that day a no-op.
  assert.match(sql, /INSERT OR IGNORE INTO pings/);
  // The binding ORDER is the thing worth pinning: a swap here would file
  // every install under the wrong column and no test of the pure half
  // would notice.
  const [day, id, country, version, os, arch, distro, osVersion] = args;
  assert.match(day, /^\d{4}-\d{2}-\d{2}$/, 'the server dates the row, not the client');
  assert.equal(id, ID);
  assert.equal(country, 'PH', "the edge's country reached the row");
  assert.equal(version, '0.1.0');
  assert.equal(os, 'linux');
  assert.equal(arch, 'x86_64');
  assert.equal(distro, 'ubuntu');
  assert.equal(osVersion, '24.04');
});

test('a client with no distribution files a blank one, never a placeholder', async () => {
  const db = fakeDb();
  const { distro: _none, os_version: _also, ...v1 } = { ...PING, v: 1 };
  const response = await worker.fetch(pingRequest(v1), { DB: db });
  assert.equal(response.status, 204, 'and is still counted');
  assert.equal(db.calls[0].args[6], '');
  assert.equal(db.calls[0].args[7], '');
});

test('an edge with no country files the row under ZZ, never blank', async () => {
  const db = fakeDb();
  const response = await worker.fetch(pingRequest(PING, null), { DB: db });
  assert.equal(response.status, 204);
  assert.equal(db.calls[0].args[2], 'ZZ');
});

test('a malformed ping is a 400 and writes nothing', async () => {
  for (const body of [{ ...PING, id: 'nope' }, { ...PING, v: 3 }, { ...PING, distro: 'Ubuntu' }, 'not json', {}]) {
    const db = fakeDb();
    const response = await worker.fetch(pingRequest(body), { DB: db });
    assert.equal(response.status, 400, JSON.stringify(body));
    assert.equal(db.calls.length, 0, 'nothing reached the database');
    assert.match((await response.json()).error, /\S/);
  }
});

test('an oversized body is refused before it is parsed', async () => {
  const db = fakeDb();
  const request = pingRequest({ ...PING, pad: 'x'.repeat(2000) });
  const response = await worker.fetch(request, { DB: db });
  assert.equal(response.status, 413);
  assert.equal(db.calls.length, 0);
});

test('the ping route takes POST only', async () => {
  const db = fakeDb();
  const response = await worker.fetch(new Request('https://c.example/v1/ping'), { DB: db });
  assert.equal(response.status, 405);
  assert.equal(response.headers.get('allow'), 'POST');
  assert.equal(db.calls.length, 0);
});

test('an unknown path is a 404 and healthz answers ok', async () => {
  const db = fakeDb();
  assert.equal((await worker.fetch(new Request('https://c.example/nope'), { DB: db })).status, 404);
  const health = await worker.fetch(new Request('https://c.example/healthz'), { DB: db });
  assert.equal(health.status, 200);
  assert.equal((await health.text()).trim(), 'ok');
});

test('stats answer JSON over the window the query asked for', async () => {
  const db = fakeDb({ batch: statsRows() });
  const response = await worker.fetch(
    new Request('https://c.example/v1/stats?days=7'),
    { DB: db },
  );
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-type'), /application\/json/);
  const stats = await response.json();
  assert.equal(stats.window.days, 7);
  assert.equal(stats.daily.length, 7, 'every day of the window, zero-filled');
  assert.deepEqual(stats.totals, { users_7d: 3, users_30d: 9, installs: 12 });
  assert.deepEqual(stats.countries, [{ country: 'PH', users: 3 }]);
  assert.deepEqual(stats.platforms, [{ platform: 'ubuntu', version: '24.04', users: 2 }]);
});

test('the platform query names the distribution where there is one, else the OS', async () => {
  // `ubuntu 24.04` and `macos 15.3.1` answer the same question, so they
  // belong in one panel; a Linux row whose client never named a distribution
  // still counts, as plain `linux`.
  const db = fakeDb({ batch: statsRows() });
  await worker.fetch(new Request('https://c.example/v1/stats'), { DB: db });
  const query = db.calls.find((c) => /AS platform/.test(c.sql));
  assert.ok(query, 'the batch asks for platforms');
  assert.match(query.sql, /CASE WHEN distro != '' THEN distro ELSE os END/);
  assert.match(query.sql, /GROUP BY platform, os_version/);
});

test('the dashboard answers HTML from the same numbers', async () => {
  const db = fakeDb({ batch: statsRows() });
  const response = await worker.fetch(new Request('https://c.example/'), { DB: db });
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-type'), /text\/html/);
  const html = await response.text();
  assert.match(html, /Alter Zero telemetry/);
  assert.match(html, /PH/);
  // Neither route should ever be indexed or cached.
  assert.equal(response.headers.get('x-robots-tag'), 'noindex');
  assert.equal(response.headers.get('cache-control'), 'no-store');
});

test('DASHBOARD_TOKEN gates the dashboard and stats but never the ping', async () => {
  const env = () => ({ DB: fakeDb({ batch: statsRows() }), DASHBOARD_TOKEN: 's3cret' });
  const bare = await worker.fetch(new Request('https://c.example/'), env());
  assert.equal(bare.status, 401);
  assert.match(bare.headers.get('www-authenticate'), /Bearer/);

  const withHeader = await worker.fetch(
    new Request('https://c.example/v1/stats', { headers: { authorization: 'Bearer s3cret' } }),
    env(),
  );
  assert.equal(withHeader.status, 200);

  const withQuery = await worker.fetch(new Request('https://c.example/?token=s3cret'), env());
  assert.equal(withQuery.status, 200);

  const wrong = await worker.fetch(new Request('https://c.example/?token=nope'), env());
  assert.equal(wrong.status, 401);

  // The app cannot carry a token, so the ping must stay open.
  const ping = await worker.fetch(pingRequest(PING), env());
  assert.equal(ping.status, 204);
});

test('the retention cron deletes only rows past the window', async () => {
  const db = fakeDb();
  await worker.scheduled(null, { DB: db, RETENTION_DAYS: '30' }, { waitUntil: (p) => p });
  assert.equal(db.calls.length, 1);
  assert.match(db.calls[0].sql, /DELETE FROM pings WHERE day < \?1/);
  const cutoff = db.calls[0].args[0];
  assert.match(cutoff, /^\d{4}-\d{2}-\d{2}$/);
  assert.ok(cutoff < new Date().toISOString().slice(0, 10), 'the cutoff is in the past');
});

test('a body over the cap in bytes is refused even when its .length is not', async () => {
  // 1024 three-byte characters: a UTF-16 `.length` of 1024 (under the cap)
  // but 3 KiB on the wire. The declared content-length catches it here; the
  // read check must agree rather than wave it through.
  const db = fakeDb();
  const request = pingRequest(JSON.stringify({ ...PING, pad: '日'.repeat(1024) }));
  const response = await worker.fetch(request, { DB: db });
  assert.equal(response.status, 413);
  assert.equal(db.calls.length, 0);
});

test('a refused stats request answers JSON, like the route it guards', async () => {
  // `/v1/stats` is read by scripts; answering its 401 in text/plain made the
  // one response a caller cannot parse the only one it is likely to get.
  const env = { DB: fakeDb({ batch: statsRows() }), DASHBOARD_TOKEN: 's3cret' };
  const denied = await worker.fetch(new Request('https://c.example/v1/stats'), env);
  assert.equal(denied.status, 401);
  assert.match(denied.headers.get('content-type'), /application\/json/);
  assert.match((await denied.json()).error, /\S/);

  // The page keeps its plain-text refusal — a browser is reading that one.
  const page = await worker.fetch(new Request('https://c.example/'), env);
  assert.equal(page.status, 401);
  assert.match(page.headers.get('content-type'), /text\/plain/);
});

test('the dashboard hands its window links the token the reader used', async () => {
  const env = { DB: fakeDb({ batch: statsRows() }), DASHBOARD_TOKEN: 's3cret' };
  const html = await (await worker.fetch(new Request('https://c.example/?token=s3cret'), env)).text();
  assert.ok(html.includes('token=s3cret'), 'the window links keep working');

  // A reader who authenticated with the header gave the page no token to
  // spread, and the page must not invent one.
  const viaHeader = await worker.fetch(
    new Request('https://c.example/', { headers: { authorization: 'Bearer s3cret' } }),
    env,
  );
  assert.ok(!(await viaHeader.text()).includes('s3cret'), 'the secret is not printed into the page');
});

test('the ping route refuses in JSON whatever the reason', async () => {
  // One endpoint answered a bad body in JSON, a big one in text and a wrong
  // method in text. A caller holding curl should get one shape.
  const cases = [
    [new Request('https://c.example/v1/ping'), 405],
    [pingRequest({ ...PING, pad: 'x'.repeat(2000) }), 413],
    [pingRequest('not json'), 400],
  ];
  for (const [request, status] of cases) {
    const response = await worker.fetch(request, { DB: fakeDb() });
    assert.equal(response.status, status);
    assert.match(response.headers.get('content-type'), /application\/json/, String(status));
    assert.match((await response.json()).error, /\S/, String(status));
  }
});

test('nothing this worker serves is cacheable or indexable', async () => {
  const env = () => ({ DB: fakeDb({ batch: statsRows() }), DASHBOARD_TOKEN: 's3cret' });
  const responses = [
    await worker.fetch(pingRequest(PING), env()),
    await worker.fetch(pingRequest('not json'), env()),
    await worker.fetch(new Request('https://c.example/healthz'), env()),
    await worker.fetch(new Request('https://c.example/nope'), env()),
    await worker.fetch(new Request('https://c.example/'), env()),
    await worker.fetch(new Request('https://c.example/?token=s3cret'), env()),
    await worker.fetch(new Request('https://c.example/v1/stats?token=s3cret'), env()),
  ];
  for (const response of responses) {
    assert.equal(response.headers.get('cache-control'), 'no-store', String(response.status));
    assert.equal(response.headers.get('x-robots-tag'), 'noindex', String(response.status));
  }
});

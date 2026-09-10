import { test } from 'node:test';
import assert from 'node:assert/strict';
import worker from '../src/worker.js';

const PING = { v: 2, id: '12'.repeat(16), version: '0.1.0', os: 'linux', arch: 'x86_64' };
function ping({ ip = '192.0.2.1', body = JSON.stringify(PING), headers = {} } = {}) {
  return new Request('https://collector.example/v1/ping', {
    method: 'POST', body, duplex: 'half',
    headers: { ...(ip === null ? {} : { 'cf-connecting-ip': ip }), 'content-type': 'application/json', ...headers },
  });
}
function environment(limit = 60) {
  const counts = new Map(), writes = [];
  let checks = 0;
  return { counts, writes, get checks() { return checks; }, RATE_LIMIT_SECRET: 'ab'.repeat(32),
    PING_RATE_LIMITER: { async limit({ key }) {
      checks += 1;
      const n = (counts.get(key) ?? 0) + 1;
      counts.set(key, n);
      return { success: n <= limit };
    } },
    DB: { prepare(sql) { return { bind(...args) { return { async run() { writes.push({ sql, args }); } }; } }; } },
  };
}
function assertNoStore(response) {
  assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.equal(response.headers.get('x-robots-tag'), 'noindex');
  assert.match(response.headers.get('content-type'), /application\/json/);
}

test('60 attempts pass; changing install IDs does not bypass the IP budget', async () => {
  const env = environment();
  for (let index = 0; index < 60; index += 1) {
    const response = await worker.fetch(ping({ body: JSON.stringify({ ...PING, id: index.toString(16).padStart(32, '0') }) }), env);
    assert.equal(response.status, 204);
  }
  const request = ping({ body: JSON.stringify({ ...PING, id: 'ff'.repeat(16) }) });
  const denied = await worker.fetch(request, env);
  assert.equal(denied.status, 429);
  assert.equal(denied.headers.get('retry-after'), '60');
  assertNoStore(denied);
  assert.deepEqual(await denied.json(), { error: 'too many ping requests' });
  assert.equal(request.bodyUsed, false, 'denial happens before consuming the request body');
  assert.equal(env.writes.length, 60);
  assert.equal((await worker.fetch(ping({ ip: '192.0.2.2' }), env)).status, 204, 'another peer has its own quota');
  env.counts.clear(); // Simulate the platform resetting its window, not the Worker clock.
  assert.equal((await worker.fetch(ping(), env)).status, 204);
});

test('concurrent requests use the shared binding, not per-isolate counters', async () => {
  const env = environment(2);
  const responses = await Promise.all(Array.from({ length: 8 }, () => worker.fetch(ping(), env)));
  assert.equal(responses.filter((response) => response.status === 204).length, 2);
  assert.equal(responses.filter((response) => response.status === 429).length, 6);
  assert.equal(env.writes.length, 2);
  assert.equal(env.checks, 8);
});

test('malformed and oversized attempts consume quota without writing to D1', async () => {
  const env = environment(2);
  assert.equal((await worker.fetch(ping({ body: 'not JSON' }), env)).status, 400);
  assert.equal((await worker.fetch(ping({ body: 'x'.repeat(1025), headers: { 'content-length': '1025' } }), env)).status, 413);
  assert.equal((await worker.fetch(ping(), env)).status, 429);
  assert.equal(env.writes.length, 0);
});

test('missing secrets, binding failures and missing trusted peers fail closed before body/D1', async () => {
  for (const change of [
    (env) => { delete env.RATE_LIMIT_SECRET; },
    (env) => { env.RATE_LIMIT_SECRET = 'bad'; },
    (env) => { delete env.PING_RATE_LIMITER; },
    (env) => { env.PING_RATE_LIMITER.limit = async () => { throw new Error('sensitive backend details'); }; },
  ]) {
    const env = environment();
    change(env);
    const request = ping();
    const response = await worker.fetch(request, env);
    assert.equal(response.status, 503);
    assert.equal(response.headers.get('retry-after'), '60');
    assertNoStore(response);
    assert.deepEqual(await response.json(), { error: 'telemetry temporarily unavailable' });
    assert.equal(request.bodyUsed, false);
    assert.deepEqual(env.writes, []);
  }
  const env = environment();
  assert.equal((await worker.fetch(ping({ ip: null, headers: { 'x-forwarded-for': '192.0.2.1' } }), env)).status, 503);
  assert.equal(env.checks, 0);
  assert.deepEqual(env.writes, []);
});

test('IPv6 host rotation and spoofed forwarding headers cannot reset the quota', async () => {
  const env = environment(1);
  assert.equal((await worker.fetch(ping({ ip: '2001:db8:1:2::1' }), env)).status, 204);
  assert.equal((await worker.fetch(ping({ ip: '2001:0db8:0001:0002:ffff::42', headers: { 'x-forwarded-for': '192.0.2.99' } }), env)).status, 429);
  assert.equal((await worker.fetch(ping({ ip: '2001:db8:1:3::1' }), env)).status, 204);
  for (const { args } of env.writes) {
    assert.equal(args.length, 8, 'the telemetry schema stays unchanged');
    assert.ok(!args.some((value) => String(value).includes('2001:')));
    assert.ok(!args.some((value) => String(value).startsWith('ping:v1:')));
  }
});

test('chunked bodies stop at 1 KiB even with no length or an understated length', async () => {
  for (const headers of [{}, { 'content-length': '1' }]) {
    let cancelled = false;
    const body = new ReadableStream({
      start(controller) { controller.enqueue(new Uint8Array(600)); controller.enqueue(new Uint8Array(500)); },
      cancel() { cancelled = true; },
    });
    const env = environment();
    const response = await worker.fetch(ping({ body, headers }), env);
    assert.equal(response.status, 413);
    assertNoStore(response);
    assert.equal(cancelled, true);
    assert.deepEqual(env.writes, []);
  }
});

test('failed request streams get a generic JSON error after rate limiting', async () => {
  const env = environment();
  const body = new ReadableStream({ start(controller) { controller.error(new Error('private stream detail')); } });
  const response = await worker.fetch(ping({ body }), env);
  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), { error: 'body could not be read' });
  assert.equal(env.checks, 1);
  assert.deepEqual(env.writes, []);
});

test('method and liveness routes do not depend on the ping limiter', async () => {
  const env = environment();
  delete env.PING_RATE_LIMITER;
  assert.equal((await worker.fetch(new Request('https://collector.example/v1/ping'), env)).status, 405);
  assert.equal((await worker.fetch(new Request('https://collector.example/healthz'), env)).status, 200);
  assert.equal(env.checks, 0);
  assert.deepEqual(env.writes, []);
});

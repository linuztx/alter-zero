import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { limitPing, normalizeClientNetwork } from '../src/rate-limit.js';

const SECRET = '19'.repeat(32);
const DAY = '2026-09-10';
const request = (ip, extra = {}) => new Request('https://collector.example/v1/ping', {
  method: 'POST', headers: { ...(ip === null ? {} : { 'cf-connecting-ip': ip }), ...extra },
});
function environment(overrides = {}) {
  const keys = [];
  return { keys, RATE_LIMIT_SECRET: SECRET,
    PING_RATE_LIMITER: { async limit({ key }) { keys.push(key); return { success: true }; } },
    ...overrides };
}

test('IPv4 and equivalent IPv6 representations share canonical network identities', () => {
  for (const ip of ['192.0.2.1', '192.000.002.001', '::ffff:192.0.2.1', '0:0:0:0:0:FFFF:c000:0201']) {
    assert.equal(normalizeClientNetwork(ip), 'v4:192.0.2.1', ip);
  }
  for (const ip of ['2001:db8:1234:5678::1', '2001:0DB8:1234:5678:ffff:ffff:ffff:ffff']) {
    assert.equal(normalizeClientNetwork(ip), 'v6:2001:0db8:1234:5678/64');
  }
  assert.equal(normalizeClientNetwork('::1'), 'v6:0000:0000:0000:0000/64');
  assert.notEqual(normalizeClientNetwork('2001:db8:1234:5679::1'), normalizeClientNetwork('2001:db8:1234:5678::1'));
  for (const ip of ['', null, 5, '256.2.3.4', '127.1', '0x7f000001', '1.2.3.4, 5.6.7.8', '[::1]', 'fe80::1%eth0', ':::', 'foo:bar', 'https://example.com', '1:2:3:4:5:6:7:8:9', '1'.repeat(100)]) {
    assert.equal(normalizeClientNetwork(ip), null, String(ip));
  }
});

test('limiter keys are domain-separated HMAC digests, never raw peers or unkeyed hashes', async () => {
  const env = environment();
  assert.equal(await limitPing(request('192.0.2.1'), env, DAY), 'allowed');
  const expected = createHmac('sha256', Buffer.from(SECRET, 'hex')).update(`alter-zero:ping:v1:${DAY}:v4:192.0.2.1`).digest('hex');
  assert.deepEqual(env.keys, [`ping:v1:${expected}`]);
  assert.ok(!env.keys[0].includes('192.0.2.1'));
  assert.equal(await limitPing(request('::ffff:192.0.2.1'), env, DAY), 'allowed');
  assert.equal(env.keys[0], env.keys[1], 'mapped IPv4 cannot reset its budget');
});

test('network, secret and UTC-day changes affect keys; IPv6 host rotation does not', async () => {
  const env = environment();
  for (const ip of ['2001:db8:1:2::1', '2001:db8:1:2::ffff', '2001:db8:1:3::1']) {
    await limitPing(request(ip), env, DAY);
  }
  assert.equal(env.keys[0], env.keys[1]);
  assert.notEqual(env.keys[0], env.keys[2]);
  await limitPing(request('2001:db8:1:2::1'), env, '2026-09-11');
  assert.notEqual(env.keys[0], env.keys[3]);
  env.RATE_LIMIT_SECRET = '29'.repeat(32);
  await limitPing(request('2001:db8:1:2::1'), env, DAY);
  assert.notEqual(env.keys[0], env.keys[4]);
});

test('forwarded and arbitrary IPv6 headers cannot select a different budget', async () => {
  const env = environment();
  await limitPing(request('192.0.2.1'), env, DAY);
  await limitPing(request('192.0.2.1', {
    'x-forwarded-for': '198.51.100.99', 'x-real-ip': '198.51.100.12',
    'true-client-ip': '198.51.100.13', 'cf-connecting-ipv6': '2001:db8::dead',
  }), env, DAY);
  assert.equal(env.keys[0], env.keys[1]);
  assert.equal(await limitPing(request(null, { 'x-forwarded-for': '192.0.2.1' }), env, DAY), 'unavailable');
  assert.equal(env.keys.length, 2, 'a missing trusted peer never falls back to a spoofable header');
});

test('Pseudo IPv4 uses the original IPv6 network only for Class E peers', async () => {
  const env = environment();
  await limitPing(request('2001:db8:1:2::1'), env, DAY);
  await limitPing(request('240.1.2.3', { 'cf-connecting-ipv6': '2001:db8:1:2::2' }), env, DAY);
  assert.equal(env.keys[0], env.keys[1]);
  for (const extra of [{}, { 'cf-connecting-ipv6': 'garbage' }, { 'cf-connecting-ipv6': '192.0.2.1' }]) {
    assert.equal(await limitPing(request('240.1.2.3', extra), env, DAY), 'unavailable');
  }
});

test('configuration, identity, and backend failures never admit a ping', async () => {
  const req = request('192.0.2.1');
  for (const secret of [undefined, '', 'a'.repeat(32), 'z'.repeat(64), '19'.repeat(33), 123]) {
    const env = environment({ RATE_LIMIT_SECRET: secret });
    assert.equal(await limitPing(req, env, DAY), 'unavailable');
    assert.deepEqual(env.keys, []);
  }
  for (const binding of [undefined, {}, { limit: true }, { async limit() { throw new Error('provider failure'); } }, { async limit() { return undefined; } }, { async limit() { return { success: 'true' }; } }]) {
    assert.equal(await limitPing(req, environment({ PING_RATE_LIMITER: binding }), DAY), 'unavailable');
  }
  assert.equal(await limitPing(request('not an IP'), environment(), DAY), 'unavailable');
  assert.equal(await limitPing(req, environment({ PING_RATE_LIMITER: { async limit() { return { success: false }; } } }), DAY), 'limited');
});

// Abuse-prevention identities are never telemetry fields. Only a keyed digest
// reaches Cloudflare's rate limiter; neither the peer nor its digest reaches D1.
import { utcDay } from './lib.js';

export const RATE_LIMIT_RETRY_SECONDS = 60;
const SECRET_RE = /^[a-f0-9]{64}$/i;
const encoder = new TextEncoder();

function ipv4(raw) {
  if (!/^\d{1,3}(?:\.\d{1,3}){3}$/.test(raw)) return null;
  const bytes = raw.split('.').map(Number);
  return bytes.every((byte) => byte <= 255) ? bytes.join('.') : null;
}

/** Canonical IPv4, or the first 64 bits of IPv6, including compressed forms. */
export function normalizeClientNetwork(value) {
  if (typeof value !== 'string') return null;
  const raw = value.trim();
  if (raw.length > 45 || !/^[a-f0-9:.]+$/i.test(raw)) return null;
  const v4 = ipv4(raw);
  if (v4) return `v4:${v4}`;
  if (!raw.includes(':')) return null;
  try {
    // The URL parser validates IPv6 without DNS or a Node-specific dependency,
    // and normalizes embedded IPv4 into hextets. Reject brackets/zone IDs above.
    const canonical = new URL(`http://[${raw}]/`).hostname.slice(1, -1);
    const [left, right] = canonical.split('::');
    const front = left ? left.split(':') : [];
    const back = right ? right.split(':') : [];
    const parts = right === undefined ? front : [...front, ...Array(8 - front.length - back.length).fill('0'), ...back];
    const words = parts.map((part) => Number.parseInt(part, 16));
    // IPv4-mapped IPv6 shares the IPv4 quota; it must not create a second key.
    if (words.slice(0, 5).every((word) => word === 0) && words[5] === 0xffff) {
      return `v4:${[words[6] >> 8, words[6] & 255, words[7] >> 8, words[7] & 255].join('.')}`;
    }
    return `v6:${words.slice(0, 4).map((word) => word.toString(16).padStart(4, '0')).join(':')}/64`;
  } catch {
    return null;
  }
}

function peerNetwork(request) {
  const network = normalizeClientNetwork(request.headers.get('cf-connecting-ip'));
  // Cloudflare Pseudo IPv4's Overwrite Headers mode replaces the peer with a
  // Class E address. Only in that mode consult its original-IPv6 header; an
  // arbitrary IPv6 header alongside a normal peer must never choose the key.
  if (network?.startsWith('v4:') && Number(network.slice(3).split('.')[0]) >= 240) {
    const original = normalizeClientNetwork(request.headers.get('cf-connecting-ipv6'));
    return original?.startsWith('v6:') ? original : null;
  }
  return network;
}

/** All POST attempts consume budget before parsing; infrastructure failure closes the gate. */
export async function limitPing(request, env, day = utcDay()) {
  if (typeof env.PING_RATE_LIMITER?.limit !== 'function' ||
      typeof env.RATE_LIMIT_SECRET !== 'string' || !SECRET_RE.test(env.RATE_LIMIT_SECRET)) {
    return 'unavailable';
  }
  const network = peerNetwork(request);
  if (!network) return 'unavailable';
  try {
    const secret = Uint8Array.from(env.RATE_LIMIT_SECRET.match(/../g), (byte) => Number.parseInt(byte, 16));
    const key = await crypto.subtle.importKey('raw', secret, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
    const message = encoder.encode(`alter-zero:ping:v1:${day}:${network}`);
    const digest = new Uint8Array(await crypto.subtle.sign('HMAC', key, message));
    const opaque = [...digest].map((byte) => byte.toString(16).padStart(2, '0')).join('');
    const verdict = await env.PING_RATE_LIMITER.limit({ key: `ping:v1:${opaque}` });
    if (verdict?.success === true) return 'allowed';
    if (verdict?.success === false) return 'limited';
  } catch {
    // Do not log exceptions: a binding/provider exception might contain its
    // input. Optional telemetry can safely lose a ping during an outage.
  }
  return 'unavailable';
}

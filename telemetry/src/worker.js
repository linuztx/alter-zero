// The Alter Zero telemetry collector (docs/telemetry.md): a Cloudflare Worker
// over a D1 table. Four routes and a cron:
//
//   POST /v1/ping        the app's daily ping → one `INSERT OR IGNORE` row keyed
//                        on the server's UTC date and the install id, with the
//                        country the edge saw. 204 on success.
//   GET  /v1/stats       the numbers as JSON (`?days=`, 1–365, default 30)
//   GET  /               the numbers as a page
//   GET  /healthz        `ok`
//   scheduled            delete rows older than RETENTION_DAYS
//
// `/` and `/v1/stats` require DASHBOARD_TOKEN when that secret is set; the
// ping route is always open. Every decision that can be made without a
// request or a database lives in lib.js, where `node --test` reaches it.

import {
  DEFAULT_RETENTION_DAYS,
  MAX_BODY_BYTES,
  authorized,
  daysBefore,
  normalizeCountry,
  renderDashboard,
  shapeStats,
  utcDay,
  validatePing,
  windowDays,
} from './lib.js';

const TEXT = { 'content-type': 'text/plain; charset=utf-8' };
const JSON_TYPE = { 'content-type': 'application/json; charset=utf-8' };
const HTML = { 'content-type': 'text/html; charset=utf-8' };
// The dashboard is for the maintainer; nothing here should be indexed.
const NO_STORE = { 'cache-control': 'no-store', 'x-robots-tag': 'noindex' };

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    switch (url.pathname) {
      case '/v1/ping':
        return handlePing(request, env);
      case '/v1/stats':
        return guarded(request, env, url, () => handleStats(env, url, JSON_TYPE, (s) => JSON.stringify(s, null, 2)));
      case '/':
        return guarded(request, env, url, () => handleStats(env, url, HTML, renderDashboard));
      case '/healthz':
        return new Response('ok\n', { headers: TEXT });
      default:
        return new Response('not found\n', { status: 404, headers: TEXT });
    }
  },

  async scheduled(_event, env, ctx) {
    ctx.waitUntil(purge(env));
  },
};

/** `POST /v1/ping`: validate, then record one row for (today, id). */
async function handlePing(request, env) {
  if (request.method !== 'POST') {
    return new Response('method not allowed\n', { status: 405, headers: { ...TEXT, allow: 'POST' } });
  }
  const declared = Number.parseInt(request.headers.get('content-length') ?? '0', 10);
  if (declared > MAX_BODY_BYTES) {
    return new Response('payload too large\n', { status: 413, headers: TEXT });
  }
  const text = await request.text();
  if (text.length > MAX_BODY_BYTES) {
    return new Response('payload too large\n', { status: 413, headers: TEXT });
  }
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    return reject('body is not JSON');
  }
  const verdict = validatePing(body);
  if (!verdict.ok) {
    return reject(verdict.error);
  }
  const { id, version, os, arch } = verdict.ping;
  // The edge's country, never the address: `request.cf` is Cloudflare's own
  // lookup on the peer; the header is the same fact for a request that
  // arrived through a custom-domain proxy.
  const country = normalizeCountry(request.cf?.country ?? request.headers.get('cf-ipcountry'));
  await env.DB.prepare(
    'INSERT OR IGNORE INTO pings (day, id, country, version, os, arch) VALUES (?1, ?2, ?3, ?4, ?5, ?6)',
  )
    .bind(utcDay(), id, country, version, os, arch)
    .run();
  return new Response(null, { status: 204 });
}

function reject(error) {
  return new Response(JSON.stringify({ error }), { status: 400, headers: JSON_TYPE });
}

/** The dashboard token, when configured, gates a route. */
async function guarded(request, env, url, handler) {
  if (request.method !== 'GET' && request.method !== 'HEAD') {
    return new Response('method not allowed\n', { status: 405, headers: { ...TEXT, allow: 'GET, HEAD' } });
  }
  if (!authorized(env.DASHBOARD_TOKEN, request.headers.get('authorization'), url.searchParams.get('token'))) {
    return new Response('unauthorized\n', {
      status: 401,
      headers: { ...TEXT, ...NO_STORE, 'www-authenticate': 'Bearer realm="alter-zero-telemetry"' },
    });
  }
  return handler();
}

/** `GET /v1/stats` and `GET /`: the same queries, rendered by `render`. */
async function handleStats(env, url, headers, render) {
  const days = windowDays(url.searchParams.get('days'));
  const today = utcDay();
  const since = daysBefore(today, days - 1);
  const [daily, newInstalls, countries, versions, oses, users7, users30, installs] = await env.DB.batch([
    env.DB.prepare('SELECT day, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY day ORDER BY day').bind(since),
    env.DB.prepare(
      'SELECT first_day AS day, COUNT(*) AS installs FROM (SELECT id, MIN(day) AS first_day FROM pings GROUP BY id) WHERE first_day >= ?1 GROUP BY first_day ORDER BY first_day',
    ).bind(since),
    env.DB.prepare('SELECT country, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY country ORDER BY users DESC, country').bind(since),
    env.DB.prepare('SELECT version, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY version ORDER BY users DESC, version').bind(since),
    env.DB.prepare('SELECT os, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY os ORDER BY users DESC, os').bind(since),
    env.DB.prepare('SELECT COUNT(DISTINCT id) AS n FROM pings WHERE day >= ?1').bind(daysBefore(today, 6)),
    env.DB.prepare('SELECT COUNT(DISTINCT id) AS n FROM pings WHERE day >= ?1').bind(daysBefore(today, 29)),
    env.DB.prepare('SELECT COUNT(DISTINCT id) AS n FROM pings'),
  ]);
  const stats = shapeStats({
    today,
    days,
    daily: daily.results,
    newInstalls: newInstalls.results,
    countries: countries.results,
    versions: versions.results,
    oses: oses.results,
    totals: {
      users_7d: users7.results[0]?.n ?? 0,
      users_30d: users30.results[0]?.n ?? 0,
      installs: installs.results[0]?.n ?? 0,
    },
    generatedAt: new Date().toISOString(),
  });
  return new Response(render(stats), { headers: { ...headers, ...NO_STORE } });
}

/** The retention cron: rows older than RETENTION_DAYS go. */
async function purge(env) {
  const retention = Number.parseInt(env.RETENTION_DAYS ?? '', 10);
  const days = Number.isFinite(retention) && retention > 0 ? retention : DEFAULT_RETENTION_DAYS;
  await env.DB.prepare('DELETE FROM pings WHERE day < ?1').bind(daysBefore(utcDay(), days)).run();
}

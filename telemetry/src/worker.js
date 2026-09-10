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
// ping route needs no token, but has an anonymous per-network rate limit.
// Every decision that can be made without a
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
import { limitPing, RATE_LIMIT_RETRY_SECONDS } from './rate-limit.js';
import { readBoundedText } from './request-body.js';

const TEXT = { 'content-type': 'text/plain; charset=utf-8' };
const JSON_TYPE = { 'content-type': 'application/json; charset=utf-8' };
const HTML = { 'content-type': 'text/html; charset=utf-8' };
// This worker is a private counter with a maintainer's dashboard on it:
// nothing it serves should be indexed or held in a cache, so every response
// carries these rather than only the two that happen to print numbers.
const NO_STORE = { 'cache-control': 'no-store', 'x-robots-tag': 'noindex' };

/** A plain-text response — the shape every non-JSON reply here takes. */
function plain(status, message, extra) {
  return new Response(`${message}\n`, { status, headers: { ...TEXT, ...NO_STORE, ...extra } });
}

/** A JSON `{ error }` — what the ping route and `/v1/stats` refuse with. */
function refuse(status, error, extra) {
  return new Response(JSON.stringify({ error }), {
    status,
    headers: { ...JSON_TYPE, ...NO_STORE, ...extra },
  });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    switch (url.pathname) {
      case '/v1/ping':
        return handlePing(request, env);
      case '/v1/stats':
        // A refusal on this route answers JSON too: it is read by scripts, and
        // the one response a caller is likeliest to meet should not be the one
        // shape it cannot parse.
        return guarded(request, env, url, JSON_TYPE, () =>
          handleStats(env, url, JSON_TYPE, (s) => JSON.stringify(s, null, 2)),
        );
      case '/':
        return guarded(request, env, url, TEXT, () =>
          handleStats(env, url, HTML, (s) =>
            // The `?token=` the reader arrived with, so the page's own links
            // keep working — never a token it was not given.
            renderDashboard(s, { token: url.searchParams.get('token') }),
          ),
        );
      case '/healthz':
        return plain(200, 'ok');
      default:
        return plain(404, 'not found');
    }
  },

  async scheduled(_event, env, ctx) {
    ctx.waitUntil(purge(env));
  },
};

/** `POST /v1/ping`: rate limit, validate, then record one row for (today, id). */
async function handlePing(request, env) {
  // The route answers JSON throughout: the app ignores the body, but a person
  // holding curl should not get three shapes from one endpoint.
  if (request.method !== 'POST') {
    return refuse(405, 'method not allowed', { allow: 'POST' });
  }
  const allowed = await limitPing(request, env);
  if (allowed !== 'allowed') {
    return refuse(allowed === 'limited' ? 429 : 503,
      allowed === 'limited' ? 'too many ping requests' : 'telemetry temporarily unavailable',
      { 'retry-after': String(RATE_LIMIT_RETRY_SECONDS) });
  }
  const declared = Number.parseInt(request.headers.get('content-length') ?? '0', 10);
  if (declared > MAX_BODY_BYTES) {
    return refuse(413, `body must be at most ${MAX_BODY_BYTES} bytes`);
  }
  const read = await readBoundedText(request, MAX_BODY_BYTES);
  if (!read.ok && read.reason === 'too-large') {
    return refuse(413, `body must be at most ${MAX_BODY_BYTES} bytes`);
  }
  if (!read.ok) return refuse(400, 'body could not be read');
  let body;
  try {
    body = JSON.parse(read.text);
  } catch {
    return refuse(400, 'body is not JSON');
  }
  const verdict = validatePing(body);
  if (!verdict.ok) {
    return refuse(400, verdict.error);
  }
  const { id, version, os, arch, distro, os_version: osVersion } = verdict.ping;
  // The edge's country, never the address: `request.cf` is Cloudflare's own
  // lookup on the peer; the header is the same fact for a request that
  // arrived through a custom-domain proxy.
  const country = normalizeCountry(request.cf?.country ?? request.headers.get('cf-ipcountry'));
  await env.DB.prepare(
    'INSERT OR IGNORE INTO pings (day, id, country, version, os, arch, distro, os_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)',
  )
    .bind(utcDay(), id, country, version, os, arch, distro, osVersion)
    .run();
  return new Response(null, { status: 204, headers: NO_STORE });
}

/**
 * The dashboard token, when configured, gates a route. `type` is the route's
 * own content type, so a refusal speaks whatever the route speaks.
 */
async function guarded(request, env, url, type, handler) {
  const deny = type === JSON_TYPE ? refuse : plain;
  if (request.method !== 'GET' && request.method !== 'HEAD') {
    return deny(405, 'method not allowed', { allow: 'GET, HEAD' });
  }
  if (!authorized(env.DASHBOARD_TOKEN, request.headers.get('authorization'), url.searchParams.get('token'))) {
    return deny(401, 'unauthorized', { 'www-authenticate': 'Bearer realm="alter-zero-telemetry"' });
  }
  return handler();
}

/** `GET /v1/stats` and `GET /`: the same queries, rendered by `render`. */
async function handleStats(env, url, headers, render) {
  const days = windowDays(url.searchParams.get('days'));
  const today = utcDay();
  const since = daysBefore(today, days - 1);
  const [daily, newInstalls, countries, versions, oses, platforms, users7, users30, installs] =
    await env.DB.batch([
    env.DB.prepare('SELECT day, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY day ORDER BY day').bind(since),
    env.DB.prepare(
      'SELECT first_day AS day, COUNT(*) AS installs FROM (SELECT id, MIN(day) AS first_day FROM pings GROUP BY id) WHERE first_day >= ?1 GROUP BY first_day ORDER BY first_day',
    ).bind(since),
    env.DB.prepare('SELECT country, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY country ORDER BY users DESC, country').bind(since),
    env.DB.prepare('SELECT version, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY version ORDER BY users DESC, version').bind(since),
    env.DB.prepare('SELECT os, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY os ORDER BY users DESC, os').bind(since),
    // What people actually run: the distribution where the row named one,
    // else the OS — `ubuntu 24.04` and `macos 15.3.1` answer the same
    // question, so they belong in one panel rather than two. A Linux row from
    // a client that named no distribution still counts, as plain `linux`.
    env.DB.prepare(
      "SELECT CASE WHEN distro != '' THEN distro ELSE os END AS platform, os_version, COUNT(DISTINCT id) AS users FROM pings WHERE day >= ?1 GROUP BY platform, os_version ORDER BY users DESC, platform, os_version",
    ).bind(since),
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
    platforms: platforms.results,
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

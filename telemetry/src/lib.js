// The collector's pure half (docs/telemetry.md): what a ping must look like,
// what the edge's country becomes, the day arithmetic the window and the
// retention share, the shape of the stats document, and the dashboard page.
// Nothing here touches a request, a database or a clock it wasn't handed, so
// `node --test` covers all of it with no network and no install.

/** The payload shape this collector understands (`telemetry::PAYLOAD_VERSION`). */
export const PAYLOAD_VERSION = 1;
/** A ping is a few dozen bytes; anything past this is not one. */
export const MAX_BODY_BYTES = 1024;
/** The dashboard's default window, and the widest it will compute. */
export const DEFAULT_WINDOW_DAYS = 30;
export const MAX_WINDOW_DAYS = 365;
/** How long a row lives when `RETENTION_DAYS` is unset. */
export const DEFAULT_RETENTION_DAYS = 400;
/** The country stored when the edge had none (ISO 3166's user-assigned "unknown"). */
export const UNKNOWN_COUNTRY = 'ZZ';

// The same shapes the app's `telemetry::is_valid_install_id` and the smoke
// suite's stub check, so client and collector can never disagree.
const ID_RE = /^[0-9a-f]{32}$/;
const VERSION_RE = /^[0-9A-Za-z.+-]{1,32}$/;
const TOKEN_RE = /^[a-z0-9_]{1,16}$/;
const COUNTRY_RE = /^[A-Z]{2}$/;

/**
 * Validate a decoded request body. `{ ok: true, ping }` carries exactly the
 * four stored fields — extra keys are dropped, so the table can never grow a
 * column by accident — else `{ ok: false, error }` with a one-line reason.
 */
export function validatePing(body) {
  if (body === null || typeof body !== 'object' || Array.isArray(body)) {
    return refuse('body must be a JSON object');
  }
  if (body.v !== PAYLOAD_VERSION) {
    return refuse(`v must be ${PAYLOAD_VERSION}`);
  }
  const { id, version, os, arch } = body;
  if (typeof id !== 'string' || !ID_RE.test(id)) {
    return refuse('id must be 32 lowercase hex characters');
  }
  if (typeof version !== 'string' || !VERSION_RE.test(version)) {
    return refuse('version must be 1-32 characters of [0-9A-Za-z.+-]');
  }
  if (typeof os !== 'string' || !TOKEN_RE.test(os)) {
    return refuse('os must be 1-16 characters of [a-z0-9_]');
  }
  if (typeof arch !== 'string' || !TOKEN_RE.test(arch)) {
    return refuse('arch must be 1-16 characters of [a-z0-9_]');
  }
  return { ok: true, ping: { id, version, os, arch } };
}

function refuse(error) {
  return { ok: false, error };
}

/**
 * The two-letter country the edge saw, upper-cased, or `ZZ`. Cloudflare's
 * `request.cf.country` is undefined for some requests, `XX` when unknown and
 * `T1` for Tor; none of those is a country, and neither is anything longer
 * than two letters.
 */
export function normalizeCountry(raw) {
  if (typeof raw !== 'string') return UNKNOWN_COUNTRY;
  const code = raw.trim().toUpperCase();
  if (!COUNTRY_RE.test(code) || code === 'XX') return UNKNOWN_COUNTRY;
  return code;
}

/** `YYYY-MM-DD` in UTC — the day a row is keyed on, by the server's clock. */
export function utcDay(date = new Date()) {
  return date.toISOString().slice(0, 10);
}

/** The UTC date `days` before `day` (`YYYY-MM-DD` both ways). */
export function daysBefore(day, days) {
  const date = new Date(`${day}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() - days);
  return utcDay(date);
}

/** The `?days=` window: an integer clamped to 1..MAX, the default for junk. */
export function windowDays(raw) {
  const n = Number.parseInt(raw ?? '', 10);
  if (!Number.isFinite(n)) return DEFAULT_WINDOW_DAYS;
  return Math.min(Math.max(n, 1), MAX_WINDOW_DAYS);
}

/**
 * Shape the query results into the stats document `/v1/stats` answers and
 * the dashboard renders. Every day of the window is present, oldest first,
 * zero-filled where the table had no row; the per-country/version/os lists
 * come through in the query's order (users descending).
 */
export function shapeStats({
  today,
  days,
  daily,
  newInstalls,
  countries,
  versions,
  oses,
  totals,
  generatedAt,
}) {
  const usersByDay = new Map(daily.map((row) => [row.day, Number(row.users)]));
  const newByDay = new Map(newInstalls.map((row) => [row.day, Number(row.installs)]));
  const series = [];
  for (let back = days - 1; back >= 0; back -= 1) {
    const day = daysBefore(today, back);
    series.push({
      day,
      users: usersByDay.get(day) ?? 0,
      new_installs: newByDay.get(day) ?? 0,
    });
  }
  return {
    generated_at: generatedAt,
    window: { days, since: daysBefore(today, days - 1), until: today },
    today: { day: today, users: usersByDay.get(today) ?? 0 },
    totals: {
      users_7d: Number(totals.users_7d ?? 0),
      users_30d: Number(totals.users_30d ?? 0),
      installs: Number(totals.installs ?? 0),
    },
    daily: series,
    countries: countries.map((row) => ({ country: row.country, users: Number(row.users) })),
    versions: versions.map((row) => ({ version: row.version, users: Number(row.users) })),
    os: oses.map((row) => ({ os: row.os, users: Number(row.users) })),
  };
}

/**
 * Whether a dashboard request may pass: always when no token is configured;
 * otherwise a `Bearer` header or a `?token=` query carrying exactly it.
 */
export function authorized(expected, authorizationHeader, tokenQuery) {
  if (!expected) return true;
  if (tokenQuery === expected) return true;
  if (typeof authorizationHeader !== 'string') return false;
  const match = /^bearer\s+(.+)$/i.exec(authorizationHeader.trim());
  return match !== null && match[1] === expected;
}

/** HTML-escape a value for the page. Every printed value goes through it. */
export function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

const STYLE = `
  :root { color-scheme: light dark; --ink: #1e1e2e; --dim: #6c7086; --accent: #89b4fa; --bar: #89dceb; --rule: #cdd6f4; --bg: #fafafa; }
  @media (prefers-color-scheme: dark) { :root { --ink: #cdd6f4; --dim: #a6adc8; --rule: #313244; --bg: #1e1e2e; } }
  body { margin: 0; padding: 2rem clamp(1rem, 4vw, 3rem); font: 15px/1.5 ui-sans-serif, system-ui, sans-serif; color: var(--ink); background: var(--bg); }
  h1 { font-size: 1.4rem; margin: 0 0 .25rem; }
  h2 { font-size: 1rem; margin: 2rem 0 .5rem; color: var(--dim); font-weight: 600; letter-spacing: .02em; text-transform: uppercase; }
  .sub { color: var(--dim); margin: 0 0 1.5rem; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr)); gap: 1rem; }
  .tile { border: 1px solid var(--rule); border-radius: .5rem; padding: 1rem; }
  .tile b { display: block; font-size: 2rem; line-height: 1.1; }
  .tile span { color: var(--dim); }
  table { border-collapse: collapse; width: 100%; max-width: 48rem; }
  th, td { text-align: left; padding: .3rem .6rem .3rem 0; border-bottom: 1px solid var(--rule); font-variant-numeric: tabular-nums; }
  th { color: var(--dim); font-weight: 600; }
  td.n { text-align: right; width: 4rem; }
  .bar { display: inline-block; height: .8rem; background: var(--bar); border-radius: .2rem; vertical-align: middle; min-width: 1px; }
  .day { color: var(--dim); white-space: nowrap; }
  footer { margin-top: 3rem; color: var(--dim); font-size: .85rem; }
`;

/**
 * The dashboard: the stats document as a page. Plain HTML and inline CSS,
 * no script, no external asset — and every value escaped, whatever the table
 * holds.
 */
export function renderDashboard(stats) {
  const maxUsers = Math.max(1, ...stats.daily.map((d) => d.users));
  const dayRows = stats.daily
    .map((d) => {
      const width = Math.round((d.users / maxUsers) * 100);
      return `<tr><td class="day">${escapeHtml(d.day)}</td><td class="n">${d.users}</td><td class="n">${d.new_installs}</td><td><span class="bar" style="width:${width}%"></span></td></tr>`;
    })
    .join('\n');
  const list = (rows, key) =>
    rows.length === 0
      ? '<tr><td colspan="2" class="day">nothing yet</td></tr>'
      : rows
          .map((r) => `<tr><td>${escapeHtml(r[key])}</td><td class="n">${Number(r.users)}</td></tr>`)
          .join('\n');
  const { window, today, totals } = stats;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Alter Zero telemetry</title>
<style>${STYLE}</style>
</head>
<body>
<h1>Alter Zero telemetry</h1>
<p class="sub">Distinct installs that pinged, by UTC day and by the country the connection came from. Window: ${escapeHtml(window.since)} → ${escapeHtml(window.until)} (${window.days} days).</p>
<div class="tiles">
  <div class="tile"><b>${today.users}</b><span>users today (${escapeHtml(today.day)})</span></div>
  <div class="tile"><b>${totals.users_7d}</b><span>users, last 7 days</span></div>
  <div class="tile"><b>${totals.users_30d}</b><span>users, last 30 days</span></div>
  <div class="tile"><b>${totals.installs}</b><span>installs seen</span></div>
</div>
<h2>Users per day</h2>
<table>
<tr><th>day</th><th class="n">users</th><th class="n">new</th><th></th></tr>
${dayRows}
</table>
<h2>Countries</h2>
<table><tr><th>country</th><th class="n">users</th></tr>
${list(stats.countries, 'country')}
</table>
<h2>Versions</h2>
<table><tr><th>version</th><th class="n">users</th></tr>
${list(stats.versions, 'version')}
</table>
<h2>Operating systems</h2>
<table><tr><th>os</th><th class="n">users</th></tr>
${list(stats.os, 'os')}
</table>
<footer>Generated ${escapeHtml(stats.generated_at)}. A user is an install id; a new install is an id first seen that day. Add <code>?days=90</code> for a wider window; <code>/v1/stats</code> serves the same as JSON.</footer>
</body>
</html>
`;
}

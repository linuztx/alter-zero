// The collector's pure half (docs/telemetry.md): what a ping must look like,
// what the edge's country becomes, the day arithmetic the window and the
// retention share, the shape of the stats document, and the dashboard page.
// Nothing here touches a request, a database or a clock it wasn't handed, so
// `node --test` covers all of it with no network and no install.

/** The payload shape the current client sends (`telemetry::PAYLOAD_VERSION`). */
export const PAYLOAD_VERSION = 2;
/**
 * Every shape this collector still counts. `2` is `1` plus the optional
 * `distro`; `1` stays on the list because refusing it would answer every
 * install that has not updated a `400` — which reads, in the numbers, as
 * everyone leaving at once.
 */
export const ACCEPTED_PAYLOAD_VERSIONS = [1, 2];
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
// The os-release spec's own charset for `ID`, which allows `.` and `-` that
// `os`/`arch` never need: `opensuse-leap`, `centos.stream`.
const DISTRO_RE = /^[a-z0-9._-]{1,32}$/;
// A version number as its own file writes it: `24.04`, `39`, `15.3.1`.
const OS_VERSION_RE = /^[a-z0-9][a-z0-9._-]{0,15}$/;
const COUNTRY_RE = /^[A-Z]{2}$/;

/**
 * Validate a decoded request body. `{ ok: true, ping }` carries exactly the
 * six stored fields — extra keys are dropped, so the table can never grow a
 * column by accident — else `{ ok: false, error }` with a one-line reason.
 *
 * `distro` and `os_version` are the optional two: a v1 client has never heard
 * of either, macOS and Windows have no distribution, and a rolling release
 * has no version. Absent is a blank column rather than a refusal.
 */
export function validatePing(body) {
  if (body === null || typeof body !== 'object' || Array.isArray(body)) {
    return refuse('body must be a JSON object');
  }
  if (!ACCEPTED_PAYLOAD_VERSIONS.includes(body.v)) {
    return refuse(`v must be one of ${ACCEPTED_PAYLOAD_VERSIONS.join(', ')}`);
  }
  const { id, version, os, arch, distro, os_version: osVersion } = body;
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
  // Absent and `null` both mean "did not say"; anything else present is
  // checked, because these columns are grouped on and one machine's junk
  // would be a row of its own for as long as the window holds it.
  const hasDistro = distro !== undefined && distro !== null;
  if (hasDistro && (typeof distro !== 'string' || !DISTRO_RE.test(distro))) {
    return refuse('distro must be 1-32 characters of [a-z0-9._-]');
  }
  const hasVersion = osVersion !== undefined && osVersion !== null;
  if (hasVersion && (typeof osVersion !== 'string' || !OS_VERSION_RE.test(osVersion))) {
    return refuse('os_version must be 1-16 characters of [a-z0-9._-] opening on a letter or digit');
  }
  return {
    ok: true,
    ping: {
      id,
      version,
      os,
      arch,
      distro: hasDistro ? distro : '',
      os_version: hasVersion ? osVersion : '',
    },
  };
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
  platforms,
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
    platforms: (platforms ?? []).map((row) => ({
      platform: row.platform,
      version: row.os_version ?? '',
      users: Number(row.users),
    })),
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


/**
 * The UTF-8 size of a string — what [`MAX_BODY_BYTES`] is measured in.
 * `String.length` counts UTF-16 code units, which is a different number for
 * everything outside ASCII: 1024 CJK characters are 3 KiB on the wire and a
 * `.length` check calls them 1 KiB.
 */
export function byteLength(text) {
  return ENCODER.encode(String(text)).length;
}

const ENCODER = new TextEncoder();

/**
 * A country code as something a person reads: the flag emoji derived from
 * the code's own letters (no image, no table) and the region's English name
 * from `Intl`, falling back to the code itself where the runtime or the code
 * has nothing better. `ZZ` is not a country: it reads `Unknown` under a globe,
 * and so does anything else that is not a code at all.
 */
export function countryLabel(raw) {
  const stored = typeof raw === 'string' ? raw.trim() : '';
  const code = stored.toUpperCase();
  if (COUNTRY_RE.test(code) && code !== UNKNOWN_COUNTRY) {
    const name = regionName(code);
    // Regional indicator symbols: 'P','H' → U+1F1F5 U+1F1ED → 🇵🇭. A pair for
    // a region that does not exist renders as a tofu box, so a code `Intl`
    // could not name gets the globe instead — but only when `Intl` was there
    // to be asked, since without it *every* code looks unnamed.
    const flag =
      REGIONS !== null && name === code
        ? GLOBE
        : String.fromCodePoint(...[...code].map((letter) => 0x1f1e6 + letter.charCodeAt(0) - 65));
    return { code, flag, name };
  }
  // Not a country. Keep whatever the row actually held rather than hiding it:
  // the only way junk gets in is by hand, and that is worth seeing.
  return { code: stored || UNKNOWN_COUNTRY, flag: GLOBE, name: 'Unknown' };
}

/** What stands in for a flag when the code is not a country. */
const GLOBE = '\u{1F310}';

const REGIONS = (() => {
  try {
    return new Intl.DisplayNames(['en'], { type: 'region' });
  } catch {
    return null;
  }
})();

function regionName(code) {
  try {
    return REGIONS?.of(code) || code;
  } catch {
    return code;
  }
}

/**
 * A link back to this collector, carrying the `?token=` the reader arrived
 * with. The page's own navigation is the reason: a dashboard reached with a
 * token whose links drop it answers 401 on the first click.
 */
export function dashboardHref({ days, token, path = '/' } = {}) {
  const query = `days=${windowDays(days)}`;
  return token ? `${path}?${query}&token=${encodeURIComponent(token)}` : `${path}?${query}`;
}

/** The windows the page offers as one click. */
const WINDOW_PRESETS = [7, 30, 90, 365];
/** How many rows a panel shows before it folds the rest into one line. */
const PANEL_ROWS = 12;

/** A finite integer, grouped for reading — the only way a number is printed. */
function num(value) {
  const n = Number(value);
  const safe = Number.isFinite(n) ? Math.trunc(n) : 0;
  return safe.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** The same coercion without the grouping, for arithmetic on a stats field. */
function int(value) {
  const n = Number(value);
  return Number.isFinite(n) ? Math.trunc(n) : 0;
}

/** A share of a whole as a percentage string, clamped and never NaN. */
function pct(part, whole) {
  const total = int(whole);
  if (total <= 0) return '0';
  const share = (int(part) / total) * 100;
  return (Math.round(Math.min(Math.max(share, 0), 100) * 10) / 10).toString();
}

/** `2026-09-06T12:00:00.000Z` → `2026-09-06 12:00 UTC`; anything else as-is. */
function readableInstant(value) {
  const text = String(value);
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(text);
  return match ? `${match[1]} ${match[2]} UTC` : text;
}

/** `2026-09-06` → `Sep 6`, for an axis that has no room for the year. */
function shortDay(day) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(day));
  if (!match) return String(day);
  const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  return `${months[Number(match[2]) - 1] ?? match[2]} ${Number(match[3])}`;
}

const STYLE = `
:root{
  color-scheme:light dark;
  --bg:#eff1f5; --surface:#fff; --surface-2:#e6e9ef; --ink:#4c4f69; --ink-2:#5c5f77;
  --dim:#8c8fa1; --rule:#dce0e8; --accent:#04a5e5; --link:#1e66f5; --new:#8839ef;
  --good:#40a02b; --shadow:0 1px 2px rgba(76,79,105,.07),0 8px 24px rgba(76,79,105,.06);
}
@media (prefers-color-scheme:dark){:root{
  --bg:#11111b; --surface:#1e1e2e; --surface-2:#313244; --ink:#cdd6f4; --ink-2:#bac2de;
  --dim:#7f849c; --rule:#313244; --accent:#89dceb; --link:#89b4fa; --new:#cba6f7;
  --good:#a6e3a1; --shadow:0 1px 2px rgba(0,0,0,.3),0 8px 24px rgba(0,0,0,.25);
}}
*{box-sizing:border-box}
body{
  margin:0;padding:clamp(1.25rem,3vw,2.5rem) clamp(1rem,4vw,3rem) 4rem;
  font:15px/1.55 ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
  color:var(--ink);background:var(--bg);
  -webkit-font-smoothing:antialiased;
}
.wrap{max-width:72rem;margin:0 auto}
.top{display:flex;flex-wrap:wrap;align-items:center;gap:1rem 1.25rem;margin-bottom:.35rem}
.brand{display:flex;align-items:center;gap:.7rem;min-width:0}
.mark{width:1.85rem;height:1.85rem;flex:none;color:var(--accent)}
h1{font-size:1.3rem;line-height:1.2;margin:0;letter-spacing:-.01em;font-weight:650}
h1 span{color:var(--dim);font-weight:450}
.windows{display:flex;gap:.35rem;margin-left:auto;flex-wrap:wrap}
.pill{
  display:inline-block;padding:.3rem .7rem;border-radius:999px;border:1px solid var(--rule);
  color:var(--ink-2);text-decoration:none;font-size:.82rem;font-variant-numeric:tabular-nums;
  background:var(--surface);transition:border-color .15s,color .15s;
}
.pill:hover{border-color:var(--accent);color:var(--ink)}
.pill[aria-current]{border-color:var(--accent);color:var(--accent);font-weight:600}
.sub{color:var(--dim);margin:0 0 1.6rem;font-size:.92rem}
.kpis{display:grid;grid-template-columns:repeat(auto-fit,minmax(11rem,1fr));gap:.9rem;margin-bottom:1.5rem}
.kpi{background:var(--surface);border:1px solid var(--rule);border-radius:.85rem;padding:1rem 1.1rem;box-shadow:var(--shadow)}
.kpi b{display:block;font-size:2.15rem;line-height:1.05;letter-spacing:-.02em;font-variant-numeric:tabular-nums;font-weight:640}
.kpi .label{display:block;margin-top:.3rem;color:var(--ink-2);font-size:.88rem}
.kpi .note{display:block;margin-top:.15rem;color:var(--dim);font-size:.78rem;font-variant-numeric:tabular-nums}
.kpi .up{color:var(--good)}
.card{background:var(--surface);border:1px solid var(--rule);border-radius:.85rem;padding:1.1rem 1.2rem 1.2rem;box-shadow:var(--shadow);margin-bottom:1.1rem}
h2{font-size:.76rem;margin:0 0 .9rem;color:var(--dim);font-weight:650;letter-spacing:.08em;text-transform:uppercase}
.legend{float:right;font-size:.74rem;color:var(--dim);letter-spacing:0;text-transform:none;font-weight:450}
.legend i{display:inline-block;width:.55rem;height:.55rem;border-radius:2px;margin:0 .25rem 0 .75rem;vertical-align:baseline}
.chart{position:relative;height:190px;margin-bottom:.5rem}
.chart .grid{position:absolute;inset:0;display:flex;flex-direction:column;justify-content:space-between;pointer-events:none}
.chart .grid span{border-top:1px solid var(--rule);font-size:.7rem;color:var(--dim);padding-left:.1rem;line-height:1;height:0;font-variant-numeric:tabular-nums}
.bars{position:absolute;inset:0;display:flex;align-items:flex-end;gap:2px;margin:0;padding:0;list-style:none}
.bars li{
  flex:1 1 0;min-width:0;height:var(--h,0%);position:relative;border-radius:3px 3px 0 0;
  background:linear-gradient(180deg,var(--accent),var(--accent) 70%,transparent 340%);
}
.bars li i{position:absolute;left:0;right:0;bottom:0;height:var(--n,0%);background:var(--new)}
.axis{display:flex;justify-content:space-between;color:var(--dim);font-size:.74rem;font-variant-numeric:tabular-nums}
.blank{display:flex;align-items:center;justify-content:center;height:190px;color:var(--dim);font-size:.9rem;border:1px dashed var(--rule);border-radius:.6rem}
/* Multi-column rather than a grid: four panels of four different lengths in a
   grid leave a hole wherever a short one shares a row with a long one, and the
   column flow packs them whatever their row counts turn out to be. */
.panels{columns:19rem;column-gap:1.1rem}
.panels .card{break-inside:avoid;margin:0 0 1.1rem}
table{border-collapse:collapse;width:100%;font-size:.9rem}
th,td{text-align:left;padding:.4rem .5rem .4rem 0;border-bottom:1px solid var(--rule);font-weight:450;font-variant-numeric:tabular-nums;vertical-align:middle}
tbody tr:last-child th,tbody tr:last-child td{border-bottom:0}
thead th{color:var(--dim);font-size:.72rem;letter-spacing:.06em;text-transform:uppercase;font-weight:600}
td.n,th.n{text-align:right;width:4.5rem;white-space:nowrap}
td.share{width:38%;padding-right:0}
.name{display:flex;align-items:baseline;gap:.45rem;min-width:0}
.name .txt{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.flag{font-size:1.05em;line-height:1}
.code{color:var(--dim);font-size:.76em;font-variant-numeric:tabular-nums}
.meter{display:block;height:.4rem;border-radius:999px;background:var(--surface-2);overflow:hidden}
.meter span{display:block;height:100%;border-radius:999px;background:var(--accent)}
.empty{color:var(--dim)}
details{background:var(--surface);border:1px solid var(--rule);border-radius:.85rem;box-shadow:var(--shadow);margin-bottom:1.1rem}
summary{padding:.85rem 1.2rem;cursor:pointer;color:var(--ink-2);font-size:.88rem;list-style-position:inside}
summary::marker{color:var(--dim)}
details[open] summary{border-bottom:1px solid var(--rule)}
.scroll{max-height:26rem;overflow:auto;padding:.4rem 1.2rem 1rem}
footer{color:var(--dim);font-size:.82rem;line-height:1.7;max-width:52rem}
footer a,.sub a{color:var(--link)}
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.92em;background:var(--surface-2);padding:.05rem .3rem;border-radius:.25rem}
@media (max-width:34rem){
  .windows{margin-left:0;width:100%}
  .chart,.blank{height:150px}
  td.share{display:none}
}
`;

const MARK = `<svg class="mark" viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="currentColor"><rect x="2" y="2" width="9" height="9" rx="2.5"/><rect x="13" y="2" width="9" height="9" rx="2.5" opacity=".5"/><rect x="2" y="13" width="9" height="9" rx="2.5" opacity=".5"/><rect x="13" y="13" width="9" height="9" rx="2.5"/></svg>`;

/** One `<li>` per day: a bar of users with the new installs marked at its foot. */
function chartBars(daily, peak) {
  return daily
    .map((d) => {
      const users = int(d.users);
      const fresh = Math.min(int(d.new_installs), users);
      const title = `${d.day} · ${num(users)} ${users === 1 ? 'user' : 'users'} · ${num(fresh)} new`;
      // A day with nobody draws nothing: the old page gave it a 1px bar, so
      // "no one" and "one person" looked alike.
      const height = `--h:${pct(users, peak)}%`;
      const share = users > 0 && fresh > 0 ? `;--n:${pct(fresh, users)}%` : '';
      return `<li style="${height}${share}" title="${escapeHtml(title)}"><i></i></li>`;
    })
    .join('');
}

/** A `Countries`/`Versions`/`Platforms` panel: name, count, share. */
function panel(title, heading, rows, decorate) {
  const peak = rows.reduce((max, row) => Math.max(max, int(row.users)), 0);
  const shown = rows.slice(0, PANEL_ROWS);
  const rest = rows.slice(PANEL_ROWS);
  const body =
    shown.length === 0
      ? `<tr><td colspan="3" class="empty">nothing yet</td></tr>`
      : shown
          .map((row) => {
            const users = int(row.users);
            return `<tr><th scope="row"><span class="name">${decorate(row)}</span></th><td class="n">${num(users)}</td><td class="share"><span class="meter"><span style="width:${pct(users, peak)}%"></span></span></td></tr>`;
          })
          .join('\n');
  const more =
    rest.length === 0
      ? ''
      : `<tr><td colspan="3" class="empty">+ ${num(rest.length)} more, in <code>/v1/stats</code></td></tr>`;
  return `<section class="card">
<h2>${escapeHtml(title)}</h2>
<table>
<thead><tr><th scope="col">${escapeHtml(heading)}</th><th scope="col" class="n">Users</th><th scope="col" class="share"></th></tr></thead>
<tbody>
${body}${more}
</tbody>
</table>
</section>`;
}

/**
 * The country cell: flag, English name, and the code it was stored as — the
 * code omitted when it *is* the name, so a region `Intl` cannot name reads
 * `QQ` once rather than twice.
 */
function countryCell(row) {
  const { code, flag, name } = countryLabel(row.country);
  const suffix = name === code ? '' : `<span class="code">${escapeHtml(code)}</span>`;
  return `<span class="flag" aria-hidden="true">${flag}</span><span class="txt">${escapeHtml(name)}</span>${suffix}`;
}

/** A plain value cell — a version string, an OS name. */
function plainCell(raw) {
  const text = String(raw ?? '').trim();
  return `<span class="txt">${text === '' ? '<span class="empty">(blank)</span>' : escapeHtml(text)}</span>`;
}

/**
 * A platform cell: `ubuntu 24.04`, `macos 15.3.1`, or just `arch` where the
 * platform is a rolling release that names no version. The version wears the
 * country code's dim styling — it qualifies the name rather than being it.
 */
function platformCell(row) {
  const version = String(row.version ?? '').trim();
  const suffix = version === '' ? '' : `<span class="code">${escapeHtml(version)}</span>`;
  return `${plainCell(row.platform)}${suffix}`;
}

/**
 * The dashboard: the stats document as a page. Plain HTML and inline CSS —
 * no script, no external asset, no web font — with every value escaped and
 * every number coerced, whatever the table holds.
 *
 * `options.token` is the `?token=` the reader arrived with, so the page's own
 * links keep working behind `DASHBOARD_TOKEN`. It is never invented: a reader
 * who authenticated with the header gives the page nothing to spread.
 */
export function renderDashboard(stats, options = {}) {
  const token = typeof options.token === 'string' && options.token !== '' ? options.token : null;
  const view = stats.window ?? {};
  const days = windowDays(view.days);
  const daily = Array.isArray(stats.daily) ? stats.daily : [];
  const peak = daily.reduce((max, d) => Math.max(max, int(d.users)), 0);
  const totals = stats.totals ?? {};
  const today = stats.today ?? {};

  const yesterday = daily.length >= 2 ? int(daily[daily.length - 2].users) : null;
  const delta = yesterday === null ? null : int(today.users) - yesterday;
  const trend =
    delta === null
      ? `<span class="note">&nbsp;</span>`
      : `<span class="note${delta > 0 ? ' up' : ''}">${delta > 0 ? '+' : delta < 0 ? '−' : '±'}${num(Math.abs(delta))} vs. the day before</span>`;

  const windows = [...new Set([...WINDOW_PRESETS, days])]
    .sort((a, b) => a - b)
    .map((n) => {
      const here = n === days ? ' aria-current="page"' : '';
      return `<a class="pill" href="${escapeHtml(dashboardHref({ days: n, token }))}"${here}>${num(n)}d</a>`;
    })
    .join('');

  const chart =
    peak === 0
      ? `<p class="blank">No pings in this window yet.</p>`
      : `<div class="chart">
  <div class="grid" aria-hidden="true"><span>${num(peak)}</span><span>${num(Math.round(peak / 2))}</span><span>0</span></div>
  <ol class="bars" role="img" aria-label="Distinct installs per day, ${escapeHtml(String(view.since ?? ''))} to ${escapeHtml(String(view.until ?? ''))}">${chartBars(daily, peak)}</ol>
</div>
<p class="axis"><span>${escapeHtml(shortDay(daily[0]?.day ?? view.since))}</span><span>${escapeHtml(shortDay(daily[daily.length - 1]?.day ?? view.until))}</span></p>`;

  const perDay = daily
    .slice()
    .reverse()
    .map(
      (d) =>
        `<tr><th scope="row">${escapeHtml(String(d.day ?? ''))}</th><td class="n">${num(d.users)}</td><td class="n">${num(d.new_installs)}</td></tr>`,
    )
    .join('\n');

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>Alter Zero telemetry</title>
<link rel="icon" href="data:,">
<style>${STYLE}</style>
</head>
<body>
<div class="wrap">
<header class="top">
  <div class="brand">${MARK}<h1>Alter Zero <span>telemetry</span></h1></div>
  <nav class="windows" aria-label="Window">${windows}</nav>
</header>
<p class="sub">Distinct installs that pinged, by UTC day and by the country the connection came from. Showing ${escapeHtml(String(view.since ?? ''))} to ${escapeHtml(String(view.until ?? ''))} (${num(days)} days).</p>

<div class="kpis">
  <div class="kpi"><b>${num(today.users)}</b><span class="label">users today</span>${trend}</div>
  <div class="kpi"><b>${num(totals.users_7d)}</b><span class="label">users, last 7 days</span><span class="note">distinct install ids</span></div>
  <div class="kpi"><b>${num(totals.users_30d)}</b><span class="label">users, last 30 days</span><span class="note">distinct install ids</span></div>
  <div class="kpi"><b>${num(totals.installs)}</b><span class="label">installs seen</span><span class="note">every id still retained</span></div>
</div>

<section class="card">
<h2>Activity<span class="legend"><i style="background:var(--accent)"></i>users<i style="background:var(--new)"></i>new installs</span></h2>
${chart}
</section>

<div class="panels">
${panel('Countries', 'Country', stats.countries ?? [], countryCell)}
${panel('Versions', 'Version', stats.versions ?? [], (row) => plainCell(row.version))}
${panel('Operating systems', 'OS', stats.os ?? [], (row) => plainCell(row.os))}
${panel('Platforms', 'Platform', stats.platforms ?? [], platformCell)}
</div>

<details>
<summary>Every day in this window (${num(daily.length)})</summary>
<div class="scroll">
<table>
<thead><tr><th scope="col">Day</th><th scope="col" class="n">Users</th><th scope="col" class="n">New</th></tr></thead>
<tbody>
${perDay || '<tr><td colspan="3" class="empty">nothing yet</td></tr>'}
</tbody>
</table>
</div>
</details>

<footer>
Generated ${escapeHtml(readableInstant(stats.generated_at))}. A user is an install id; a new install is an id first seen that day. Both are counted on the UTC day the ping <em>arrived</em>, by the collector's clock.
<br>The same numbers as JSON: <a href="${escapeHtml(dashboardHref({ days, token, path: '/v1/stats' }))}">/v1/stats?days=${num(days)}</a>. No address is stored, logged, or derived into anything finer than the country.
</footer>
</div>
</body>
</html>
`;
}

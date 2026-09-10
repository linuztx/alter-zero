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

// Keep the collector's public import path stable while the UI lives separately.
export { renderDashboard } from './dashboard.js';

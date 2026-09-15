# Alter Zero telemetry collector

The server half of Alter Zero's usage telemetry. `TELEMETRY.md` in the
repository root states what is collected and how to turn it off;
`docs/telemetry.md` is the design. A **Cloudflare Worker** receives
the app's anonymous daily ping, notes the **country** the connection came
from — never the address — in a **D1** table, and serves a small dashboard of
users per day and per country.

Nothing here depends on a package at runtime. The collector is plain ES
modules; the dashboard is server-rendered HTML with inline CSS and vanilla
JavaScript. Its charts and bundled map need no external assets, web fonts,
map service, or client framework. Node's built-in runner covers the collector,
renderer, browser scripts, and map.

```
telemetry/
├── wrangler.toml            D1 and rate-limit bindings, retention cron
├── .dev.vars.example        local secret setup example; no working secrets
├── schema.sql               the `pings` table
├── migrations/              one file per column added since
├── src/lib.js               validation, stats shaping, shared helpers
├── src/worker.js            the routes over D1
├── src/rate-limit.js        private network keys and the ping rate-limit gate
├── src/request-body.js      streamed request body size limit
├── src/dashboard.js         server-rendered dashboard markup
├── src/dashboard-style.js   responsive layout and theme palettes
├── src/dashboard-client.js  theme, chart, map, and navigation interactions
├── src/world-map.js         country-level SVG map renderer
├── src/world-map-data.js    bundled Natural Earth outlines and anchors
├── src/world-map-data.md    map sources and transformation notes
├── scripts/preview.js       local dashboard with labeled sample data
└── test/                    collector, dashboard, script, and map tests
```

## Deploy

You need a Cloudflare account (the free plan is enough), a Node.js version
supported by Wrangler, and **Wrangler 4.36.0 or later**.

```bash
cd telemetry
npm install                       # wrangler, the Cloudflare CLI (or use `npx wrangler …` below)
npx wrangler login
npx wrangler d1 create alter-zero-telemetry
#   → paste the printed database_id into wrangler.toml
npm run db:init                   # creates the `pings` table in the remote database
#   already deployed before payload v2? `npm run db:migrate` adds its two columns
npm run secret:rate-limit         # required: generate and upload RATE_LIMIT_SECRET
npm run secret:token              # optional: a token the dashboard will require
npm run deploy
#   → prints https://alter-zero-telemetry.<your-subdomain>.workers.dev
```

`secret:rate-limit` generates 32 random bytes and uploads their 64 hex
characters as the `RATE_LIMIT_SECRET` secret without printing the value.
Use a separate secret from `DASHBOARD_TOKEN`; do not place it in
`wrangler.toml` or commit it. Existing deployments need this secret before
deploying the hardened collector, but need no D1 schema migration for rate
limiting. A missing or invalid secret makes pings return `503`.

Then make the app agree with the deployment: `telemetry::DEFAULT_ENDPOINT`
in `src/telemetry.rs` (repository root) must be that URL plus `/v1/ping`. It
ships as `https://alter-zero-telemetry.linuztx.workers.dev/v1/ping`; if
`wrangler deploy` printed something else — a different subdomain, a custom
domain — change the constant and rebuild. Until the two agree every ping
fails silently and the dashboard stays empty.

To try a build against the collector before changing the constant:

```bash
ALTER_ZERO_TELEMETRY_URL=https://alter-zero-telemetry.<subdomain>.workers.dev/v1/ping alter-zero
```

## Read the numbers

- **`https://…workers.dev/`** — the dashboard: users today and the change
  from yesterday, distinct users over 7 and 30 days, and installs seen within
  retained history. Switch the activity chart between **bars and line**, or
  between **users and new installs**; focus it and use **Left / Right / Home /
  End** to inspect days. The country map uses bundled Natural Earth outlines
  and fixed country positions; select a marker or country row to see its
  count, with **Enter / Space** available on markers. Unknown or unmapped
  countries remain in the country list. Versions, operating systems, and
  platforms (`ubuntu 24.04`, `macos 15.3.1`, `arch`) describe the installs;
  expandable rows and the **Daily data** table keep the full numbers nearby.
  The **7d / 30d / 90d / 365d** switcher is the `?days=` window (1–365), and
  its links carry your `?token=` along.
- **`https://…workers.dev/v1/stats?days=30`** — the same as JSON, for a
  script or a spreadsheet. Its refusals are JSON too.

The theme picker offers **System**, **Catppuccin Mocha, Macchiato, Frappé,
Latte**, **Nord**, and **Dracula**. System follows the browser's light/dark
preference with Latte/Mocha; an explicit choice stays in that browser's
local storage under `alter-zero-telemetry-theme`. It does not change the
terminal app's theme. Reduced-motion preferences are respected. With
JavaScript disabled, the charts, map, data tables, and window links remain
readable; theme, chart-mode, refresh, and zoom controls are hidden. Chart,
map, and theme interactions need no additional requests; refreshing or
changing the window loads the collector again.

With `DASHBOARD_TOKEN` set, both take `Authorization: Bearer <token>` or
`?token=<token>`; without it they are public. Set it to protect access to
these database-backed read routes; the ping rate limit does not cover them.
The ping route needs no dashboard token and is subject to its own rate limit.
Reach the page with `?token=` and its own links keep it; reach it with the
header and the token is never printed into the page at all.

```bash
curl -s -H "Authorization: Bearer $TOKEN" https://alter-zero-telemetry.<subdomain>.workers.dev/v1/stats?days=7 | jq .today
```

## What arrives, what is kept

The app sends
`{"v":2,"id":"…32 hex…","version":"0.1.0","os":"linux","arch":"x86_64","distro":"ubuntu","os_version":"24.04"}`
once per UTC day per install, and once more on the day the install is
updated (the version is one of the fields). `distro` is the Linux distribution's
os-release `ID` — absent entirely on macOS and Windows — and `os_version` is
that platform's own version: `VERSION_ID` on Linux, `ProductVersion` on
macOS, absent for a rolling release and on Windows. Both are read from a
file; nothing is run to find them. A client still on payload `v1` has never
heard of either and is counted exactly as before. The worker validates every
field
(anything else is a `400`, and a body over 1 KiB a `413`), keys the row on
**its own** UTC date and the id — an upsert, `INSERT … ON CONFLICT(day, id)
DO UPDATE`, so an install's second ping on one day refreshes that row (the
ping an update sends moves it onto the new version) rather than adding one
or being dropped — and adds the
country Cloudflare's edge saw (`request.cf.country`; `ZZ` when it had none).
The client's address is not stored or logged. For abuse prevention, the
collector uses it in memory to derive a daily keyed digest, passed only to
Cloudflare's rate limiter. Neither the address nor digest enters D1 or the
response, and no mapping to install IDs is kept. Workers observability and
Logpush are explicitly disabled in `wrangler.toml`; keep them off, since
request logs containing headers would break that promise.

A cron (`17 3 * * *` UTC) deletes rows older than `RETENTION_DAYS` (400).

## Ping protection

`PING_RATE_LIMITER` allows **60 POST attempts per minute** per IPv4 address
or IPv6 `/64` prefix. This leaves room for multiple installs behind a shared
network while slowing repeated submissions. The check runs before reading
the body or using D1, so malformed attempts also consume the budget.
Cloudflare's counters are approximate and separate for each edge location;
this is not a global quota or protection against every distributed attack.

The key is an HMAC-SHA-256 digest of a scope, the server's UTC day, and the
normalized address or prefix, using `RATE_LIMIT_SECRET` (exactly 64 hex
characters encoding 32 random bytes). Keys change at UTC midnight and are
not joined to telemetry records. The `[[ratelimits]]` namespace in
`wrangler.toml` must be unique within your Cloudflare account unless you
intend another Worker to share the same counters.

The worker expects direct Cloudflare ingress and trusts `CF-Connecting-IP`,
never `X-Forwarded-For`. With Cloudflare's Pseudo IPv4 overwrite mode, it
uses `CF-Connecting-IPv6` only when the primary address is in `240.0.0.0/4`;
that case requires a valid original IPv6 address. Do not put an untrusted
same-zone Worker in front of it: such Workers can alter the apparent client
address. Requests forwarded by cross-zone Workers can share an address.

Refusals are JSON `{ "error": ... }`: `429` when limited, or `503` if the
secret, binding, trusted source address, or limiter is unavailable. Both
include `Retry-After: 60` and write no row. The body reader stops and cancels
once it exceeds **1,024 bytes**, even without `Content-Length` (`413`);
unreadable or invalid bodies return `400`. Responses remain `no-store`.
See the [design notes](../docs/telemetry.md#ping-abuse-prevention) for the
trust boundary and Cloudflare references.

## Develop

From the `telemetry/` directory:

```bash
npm test                          # collector and dashboard, no network or wrangler
npm run preview                   # sample dashboard at http://127.0.0.1:8788
```

The preview needs only Node, with no package installation, database, or
secret. It binds to `127.0.0.1` and labels its generated numbers **Sample data**; they are
not actual telemetry. Open `http://127.0.0.1:8788/?empty=1` for the empty
state, or use the preview's toggle. This server does not collect pings.

The actual Worker needs a local `RATE_LIMIT_SECRET` as well as local D1.
The command below appends a fresh secret to the ignored `.dev.vars`,
preserves any existing `DASHBOARD_TOKEN`, and refuses to replace an existing
rate-limit secret. `.dev.vars.example` describes the expected fields; its
placeholder is not a working secret. Run this once from `telemetry/`:

```bash
node --input-type=module <<'NODE'
import { existsSync, readFileSync, appendFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
const path = '.dev.vars';
const current = existsSync(path) ? readFileSync(path, 'utf8') : '';
if (/^\s*RATE_LIMIT_SECRET\s*=/m.test(current)) {
  throw new Error('RATE_LIMIT_SECRET already exists; keep it or edit it explicitly');
}
appendFileSync(path, '\nRATE_LIMIT_SECRET=' + randomBytes(32).toString('hex') + '\n', { mode: 0o600 });
NODE
```

Then run the Worker:

```bash
npm run db:init:local && npm run dev   # a local worker + local D1 on http://localhost:8787
ALTER_ZERO_TELEMETRY_URL=http://localhost:8787/v1/ping alter-zero   # point a local build at it
```

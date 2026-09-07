# Alter Zero telemetry collector

The server half of Alter Zero's usage telemetry. `TELEMETRY.md` in the
repository root states what is collected and how to turn it off;
`docs/telemetry.md` is the design. A **Cloudflare Worker** receives
the app's anonymous daily ping, notes the **country** the connection came
from — never the address — in a **D1** table, and serves a small dashboard of
users per day and per country.

Nothing here depends on a package at runtime: `src/worker.js` is plain ES
modules and `src/lib.js`, its pure half, is tested with Node's built-in
runner.

```
telemetry/
├── wrangler.toml     the worker's name, the D1 binding, the retention cron
├── schema.sql        the `pings` table
├── migrations/       one file per column added since
├── src/lib.js        validation, country normalisation, stats shaping, the page
├── src/worker.js     the routes over D1
├── test/lib.test.js  the pure half, under `node --test`
└── test/worker.test.js  the routes, over a fake D1
```

## Deploy

You need a Cloudflare account (the free plan is enough) and Node 18+.

```bash
cd telemetry
npm install                       # wrangler, the Cloudflare CLI (or use `npx wrangler …` below)
npx wrangler login
npx wrangler d1 create alter-zero-telemetry
#   → paste the printed database_id into wrangler.toml
npm run db:init                   # creates the `pings` table in the remote database
#   already deployed before payload v2? `npm run db:migrate` adds its two columns
npm run secret:token              # optional: a token the dashboard will require
npm run deploy
#   → prints https://alter-zero-telemetry.<your-subdomain>.workers.dev
```

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
  from yesterday, distinct users over 7 and 30 days, installs seen, a bar per
  day of the window with each day's new installs marked at its foot, and
  panels for countries (flag and name, not just the code), versions and
  operating systems, and platforms (`ubuntu 24.04`, `macos 15.3.1`, `arch`) —
  with every day's exact numbers folded into a `details`
  at the bottom. The **7d / 30d / 90d / 365d** switcher in the header is the
  `?days=` window (1–365), and its links carry your `?token=` along.
  Plain HTML and inline CSS: no script, no external asset, nothing fetched
  when you open it. It follows your system's light or dark theme.
- **`https://…workers.dev/v1/stats?days=30`** — the same as JSON, for a
  script or a spreadsheet. Its refusals are JSON too.

With `DASHBOARD_TOKEN` set, both take `Authorization: Bearer <token>` or
`?token=<token>`; without it they are public. The ping route is always open.
Reach the page with `?token=` and its own links keep it; reach it with the
header and the token is never printed into the page at all.

```bash
curl -s -H "Authorization: Bearer $TOKEN" https://alter-zero-telemetry.<subdomain>.workers.dev/v1/stats?days=7 | jq .today
```

## What arrives, what is kept

The app sends
`{"v":2,"id":"…32 hex…","version":"0.1.0","os":"linux","arch":"x86_64","distro":"ubuntu","os_version":"24.04"}`
at most once per UTC day per install. `distro` is the Linux distribution's
os-release `ID` — absent entirely on macOS and Windows — and `os_version` is
that platform's own version: `VERSION_ID` on Linux, `ProductVersion` on
macOS, absent for a rolling release and on Windows. Both are read from a
file; nothing is run to find them. A client still on payload `v1` has never
heard of either and is counted exactly as before. The worker validates every
field
(anything else is a `400`, and a body over 1 KiB a `413`), keys the row on
**its own** UTC date and the id —
`INSERT OR IGNORE`, so a second ping on one day is a no-op — and adds the
country Cloudflare's edge saw (`request.cf.country`; `ZZ` when it had none).
The client's address is not stored, not logged and not derived into anything
finer than the country. Keep Workers Logs and Logpush off for this worker:
a request log with headers would break that promise.

A cron (`17 3 * * *` UTC) deletes rows older than `RETENTION_DAYS` (400).

## Develop

```bash
npm test                          # both halves, no network and no wrangler
npm run db:init:local && npm run dev   # a local worker + local D1 on http://localhost:8787
ALTER_ZERO_TELEMETRY_URL=http://localhost:8787/v1/ping alter-zero   # point a local build at it
```

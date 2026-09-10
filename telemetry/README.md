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
├── wrangler.toml            the worker's name, D1 binding, retention cron
├── schema.sql               the `pings` table
├── migrations/              one file per column added since
├── src/lib.js               validation, stats shaping, shared helpers
├── src/worker.js            the routes over D1
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

From the `telemetry/` directory:

```bash
npm test                          # collector and dashboard, no network or wrangler
npm run preview                   # sample dashboard at http://127.0.0.1:8788
```

The preview needs only Node, with no package installation or database. It
binds to `127.0.0.1` and labels its generated numbers **Sample data**; they are
not actual telemetry. Open `http://127.0.0.1:8788/?empty=1` for the empty
state, or use the preview's toggle. This server does not collect pings.

To develop the actual Worker and local D1, use the existing Wrangler flow:

```bash
npm run db:init:local && npm run dev   # a local worker + local D1 on http://localhost:8787
ALTER_ZERO_TELEMETRY_URL=http://localhost:8787/v1/ping alter-zero   # point a local build at it
```

# Telemetry — counting users by day and country

**One anonymous ping a day per install**, so the project can answer two
questions and nothing else: *how many people ran Alter Zero today?* and
*where in the world are they?* The client side is a few dozen bytes of JSON
sent once a day from a background thread after the first frame; the server
side is a Cloudflare Worker over a D1 table that notes the **country** the
connection came from (never the address) and serves a small dashboard. Both
halves live in this repository — the client in `src/telemetry.rs` and
`src/tui/telemetry.rs`, the collector in `telemetry/` — because a ping with
nothing listening is just a request that fails quietly, and the collector is
the half a "usage counter" is most often missing.

The design is Homebrew's / Next.js's / Astro's: **opt-out, anonymous,
disclosed once, off with one setting or one variable, and honouring
`DO_NOT_TRACK`.** What follows is exactly what is sent, what is deliberately
not, how the two halves agree, and how to deploy the collector.

This file is the *design*. The **user-facing statement** — what leaves a
user's machine, how to turn it off, what the server keeps, and how to verify
any of it — is `TELEMETRY.md` at the repository root, which is where the
app's own first-run notice sends people. The README carries none of it by
design — a privacy statement is a document someone goes looking for, not a
section they scroll past. Change one of the two and re-read the other: they
describe the same feature to two different readers.

## What is sent

One `POST` to the collector, body `application/json`, at most once per UTC
day per install:

```json
{"v":2,"id":"6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980","version":"0.1.0","os":"linux","arch":"x86_64","distro":"ubuntu","os_version":"24.04"}
```

| field | what it is | why |
|---|---|---|
| `v` | the payload version, `2` | so a future shape can be told from this one; the collector still accepts `1` |
| `id` | the **install id** — 16 random bytes as 32 hex characters, minted once and kept in `telemetry.json` | "how many people" needs to tell two launches by one person from two people; a random id does that without identifying anyone. It is not derived from anything about the machine — no MAC, no hostname, no user name — so it cannot be reversed into one, and deleting the file mints a new one |
| `version` | `CARGO_PKG_VERSION` | which versions are still out there |
| `os`, `arch` | `std::env::consts::OS` / `ARCH` | which platforms to build for |
| `distro` | the Linux distribution's os-release `ID` — **Linux only**, and absent from the body entirely elsewhere | which distributions to test and package for (below) |
| `os_version` | the version of whatever `distro`/`os` names: the distribution's `VERSION_ID`, or macOS's `ProductVersion`. Absent when the platform names none | which versions to build against — glibc, SDK floor (below) |

### The platform

`os: "linux"` answers "should we ship a Linux build" and nothing after it.
`distro` and `os_version` answer the next question — Debian-family or Arch,
which glibc, which macOS SDK floor — and between them they read **two lines
of two files**:

| platform | file | keys |
|---|---|---|
| Linux | `/etc/os-release`, then `/usr/lib/os-release` (systemd's search order) | `ID`, `VERSION_ID` |
| macOS | `/System/Library/CoreServices/SystemVersion.plist` | `ProductVersion` |
| Windows | — | neither |

Five decisions are worth stating, because each of them is where these fields
could have gone wrong:

- **The machine-readable keys, never the display ones.** `ubuntu` + `24.04`,
  not `Ubuntu 22.04.3 LTS (Jammy Jellyfish)`: `PRETTY_NAME` and `VERSION` are
  free text that would make the column ungroupable, and macOS's
  `ProductBuildVersion` (`24D70`) is a build id, not a version anyone reads.
  `telemetry::os_release_value` matches its key with `split_once('=')` rather
  than a prefix, because `ID_LIKE=debian` is Ubuntu's ancestry and not its
  identity; `macos_version_from_plist` matches `<key>ProductVersion</key>`
  *with its tags*, so neither `ProductBuildVersion` (which comes first in the
  file) nor `ProductUserVisibleVersion` (which contains the whole key name)
  can be picked up in its place.
- **Files, not subprocesses.** macOS's version is read straight out of the
  plist `sw_vers` itself reports from — one `read_to_string` and a `find`,
  rather than spawning a process before the first frame. Nothing is run, and
  no dependency is added: one key out of a nine-line file does not need a
  plist parser.
- **One read serves both on Linux.** `ID` and `VERSION_ID` are two keys of
  one file, so `tui::telemetry::platform` opens it once.
- **Absent beats invented.** A rolling release names no `VERSION_ID`, so it
  sends none — `arch` says more than `arch` with a made-up number would.
  macOS and Windows have no distribution, and `distro: "macos"` would be a
  bucket on the dashboard that is not a distribution. Both fields are
  `Option<String>` with `skip_serializing_if`, so a platform that names
  neither sends a body byte-identical to v1's plus the version bump.
- **A value that will not fit is dropped, never sent.** One malformed field
  is a `400`, and a refused ping counts nobody — so a value outside the
  wire's charset costs the field and not the install.

A Linux system whose os-release names no `ID`, or that has no such file,
reports `linux`: the [spec's own default][os-release], and the honest answer
for a minimal container. **Windows reports neither** — its version lives in
the registry, which means a crate or a subprocess, and neither has earned a
place at startup yet.

[os-release]: https://www.freedesktop.org/software/systemd/man/os-release.html

Two headers ride along: `User-Agent: alter-zero/{version}` and
`Content-Type: application/json`. The response body is ignored; any `2xx`
counts as delivered.

**The country is not in the payload.** The collector reads it off the
connection at the edge (`request.cf.country`, Cloudflare's own lookup on the
peer address) and stores the **two-letter code only** — `PH`, `DE`, `US`, or
`ZZ` when the edge has none. The address itself is never written anywhere:
not to the table, not to a log line. That is why the country lookup belongs
to the collector and not the client — asking the client to geolocate itself
would either need a database it does not have or an IP it should not be
sending.

### What is deliberately not sent

No prompts, no replies, no file paths, no working directory, no model or
provider names, no keys, no hostname, no user name, no locale, no terminal,
no session id, no timings. The rollout and the input history never leave the
machine. A reader who wants to check this against the code has one function
to read: `telemetry::Ping::to_json` is the whole wire body, and
`the_payload_carries_exactly_the_seven_fields` pins it.

## When it is sent

At **startup**, after the first frame is queued (`Session::bootstrap` calls
`start_telemetry` right after `paint_first_frame`), on a detached worker
thread that only *sends* — the loop never waits on it, so a slow or dead
collector costs the user nothing (invariant 1 holds too: it reads no stdin).
The send rides the same cached HTTP client every provider request uses
(`llm::http_client` at the chat's own timeout), so the ping adds no second
connection pool and no second runtime thread to a process that idles in a
terminal all day (`docs/memory.md`).

The client throttles itself to **one ping per UTC day**: `telemetry.json`
records `last_ping_day`, and `telemetry::should_ping` compares it with the
boundary's UTC date. The day is recorded **only when the collector answered
`2xx`** — the worker reports back over its own channel
(`Session::on_telemetry_result`, the eleventh `select!` source) and the
*loop* does the read-modify-write, so a launch while offline is simply
retried by the next launch that day rather than lost, and the file is never
written from two threads. The collector dedups on `(day, id)` too, so even a
client that pinged twice (a clock jump, two machines sharing one config home)
counts once.

It is also checked at **every turn start**, not only at launch: this app idles
in a terminal all day, so a session left open across midnight would otherwise
count once for a week of use — an undercount of exactly the heaviest users.
The check is cheap on the common path (`Session::telemetry_day_check`): the
day this session already attempted is held in memory, so a turn that is not
the first of a new day costs a date read and a string compare and touches no
file.

That memory is also the **bound on a failing collector**. Because the day is
recorded on delivery, a collector that never answers leaves the file unchanged
— and a turn-start check with no other guard would then re-send on every turn.
So an attempt is remembered whether or not it succeeds: at most one per UTC
day *per session*, with the file's `last_ping_day` still retrying at the next
launch.

Turning the setting **on** mid-session sends today's ping right away if it has
not gone yet; turning it **off** sends nothing more (a result already in
flight still records its day, which the read-modify-write keeps from
resurrecting the `true` the user just cleared).

**No ping is ever sent before the disclosure.** Every path that can send — the
launch, a turn's rollover, the `/settings` row — goes through the one
`Session::telemetry_tick`, which commits the notice first when it has never
been shown. Leaving that to each call site is exactly how the `/settings` path
came to send a ping with `notice_shown: false` still in the file.

## Turning it off

Three doors, any one of which is enough:

| door | scope | how |
|---|---|---|
| **`/settings` → Telemetry** | persistent, for this user | cycles to `false`; written to `telemetry.json` as `"enabled": false` |
| **`ALTER_ZERO_TELEMETRY=0`** | this run | the app's own on/off grammar (`0`/`false`/`no`/`off`); `=1` turns it on for a run whose file says off |
| **`DO_NOT_TRACK=1`** | this run | the cross-tool convention (consoledonottrack.com); when it is set it outranks `ALTER_ZERO_TELEMETRY` — a blanket statement beats an app default |

An environment that forbids telemetry is **hard**, unlike every other
`ALTER_ZERO_*` override: it does not merely seed the row, it makes the row
*unavailable* (`config::telemetry_forbidden_by_env` feeds
`SettingAvailability::telemetry`), so it reads `false (unavailable)` and
cycling it raises the refusal toast instead of opting back in. An opt-out a
keystroke could undo is not an opt-out — and the keystroke used to work: the
row cycled to `true`, wrote `enabled: true`, and sent a ping under
`DO_NOT_TRACK=1`.

The asymmetry is deliberate and one-directional. An environment that forces
telemetry **on** (`ALTER_ZERO_TELEMETRY=1`) leaves the row cyclable, because
turning it *off* must always be possible; only "no" is enforced against the
UI.

And a fourth that is not a door but a fact: **no config home, no telemetry**.
Without `~/.alter-zero` (or `ALTER_ZERO_CONFIG_DIR`) there is nowhere to keep
an install id, and a fresh random id per launch would count one person as
many; the `/settings` row then reads `false (unavailable)` like every knob the
host can't serve.

The Telemetry row is the one `/settings` knob that is **not per directory**
(`docs/per-directory-state.md`): an opt-out that applied only to the project
you happened to be in would be a surprise, so its value lives in
`telemetry.json` beside the install id, never in `settings.json` —
`SessionSettings::telemetry` is `#[serde(skip)]` and `copy_value` never moves
it, the `PermissionMode` pattern. The environment's *value* seeds the row for
a run like any other override (`config::apply_setting_overrides`) and is never
written back; its *veto* additionally withdraws the row, as above.

### The one-time notice

The first launch that would ping says so, once, in scrollback under the
banner:

```
  Alter Zero sends one anonymous ping a day so its users can be counted: the
  app version and OS, and the country the connection came from — never your
  prompts, files, keys or IP address. Turn it off in /settings → Telemetry,
  or with ALTER_ZERO_TELEMETRY=0.
```

`ui::startup_paragraph_lines` — the banner's indent and dim colour, **wrapped**
where the checkpoint notice's `startup_notice_lines` clamps (that one is a
status row; this is a sentence the user is meant to finish). `notice_shown`
in `telemetry.json` keeps it to once, and it is skipped entirely when nothing
will be sent (the variable or the file says off), since there is nothing to
disclose. Chrome like the banner, it never enters `history`, and like the
checkpoint notice it is a startup fact a purge rebuild does not re-emit.

## `telemetry.json`

`{config_home}/telemetry.json`, its own file (one file per feature that owns
it):

```json
{
  "enabled": true,
  "install_id": "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
  "last_ping_day": "2026-09-06",
  "notice_shown": true
}
```

`telemetry::TelemetryFile` is the pure format. `parse` is lenient — a
corrupt or empty file reads as the defaults (on, no id, never pinged, notice
not yet shown), so a bad file costs at most a new id and a repeated notice,
never the session. `install_id_or_mint` keeps a valid id (32 lowercase hex)
and replaces anything else with the random bytes the boundary hands it
(`getrandom`, the PKCE verifier's source), so the module never touches the
entropy source itself and a test can mint a known id. Every write is a
read-modify-write over the file (`config::update_telemetry_file`), so the
loop's `last_ping_day` write and the `/settings` toggle's `enabled` write
cannot clobber each other.

Deleting the file resets the install: a new id next launch, and the notice
once more.

## The collector

`telemetry/` is a **Cloudflare Worker** over a **D1** (SQLite) database —
chosen because the free tier covers this project many times over, because the
edge already knows the country (`request.cf.country`) so no GeoIP database
has to be shipped or updated, and because it deploys with one command and no
server to keep patched. Nothing in it depends on a package: `worker.js` is
plain ES modules, and its pure half (`lib.js`) is tested with Node's built-in
runner.

### Routes

| route | who | does |
|---|---|---|
| `POST /v1/ping` | the app | validates the payload (`v` ∈ {1, 2}, `id` 32 hex, `version` ≤ 32 chars of `[0-9A-Za-z.+-]`, `os`/`arch` ≤ 16 of `[a-z0-9_]`, an optional `distro` ≤ 32 of `[a-z0-9._-]` and an optional `os_version` ≤ 16 of the same opening on a letter or digit, body ≤ 1 KiB **of UTF-8**, not of `String.length` — 1024 CJK characters are 3 KiB), then `INSERT OR IGNORE` one row keyed on **the server's** UTC date and the id — the client's clock is never trusted for the day — with the edge's country. Answers `204`; a bad body `400`; a big one `413`; anything but `POST` `405` — every refusal a `{"error": …}`, since one endpoint owes a caller one shape |
| `GET /v1/stats?days=30` | you | JSON: today's users, 7- and 30-day distinct users, total installs seen, per-day users and new installs, users per country, per app version, per OS, per platform (`ubuntu` + `24.04`) over the window (`days` clamped to 1–365). Its refusals are JSON too — this is the route a script reads |
| `GET /` | you | the same numbers as a page (below) |
| `GET /healthz` | uptime checks | `ok` |

"Users" is always `COUNT(DISTINCT id)`; a "new install" is an id whose
earliest day is the day in question. Set the `DASHBOARD_TOKEN` secret and
`/` and `/v1/stats` require it (`Authorization: Bearer …` or `?token=`);
unset, they are public. `/v1/ping` is always open — it has to be. Every
response — the ping's `204` included — carries `cache-control: no-store` and
`x-robots-tag: noindex`: this is a private counter with a maintainer's page
on it, and that rule is stated once rather than on the two routes that
happen to print numbers.

### The dashboard

`GET /` is the stats document as **server-rendered HTML with inline CSS and
vanilla JavaScript**. The browser receives the numbers, chart geometry, map,
and interactions in one document. There are no runtime packages, external
assets, web fonts, or map tiles to load; inspecting data or changing the
theme contacts no other service. Refresh and window navigation request the
same collector again.

The responsive layout carries Alter Zero's terminal styling into a sidebar
and overview, with sections for activity, geography, environments, and
daily data:

- The **7d / 30d / 90d / 365d** switcher also includes the current window
  when it is none of those. Four summary cards show users today, their change
  from yesterday, distinct users over 7 and 30 days, and installs seen within
  retained history.
- **Activity** switches between bars and a line, and between daily users and
  new installs. Hover or focus a day for its exact numbers; **Left / Right /
  Home / End** move through the chart with the keyboard. A zero day stays on
  the baseline, and an empty window has an explicit no-data state.
- **Around the world** uses bundled Natural Earth outlines and fixed country
  anchors in an SVG. The map works offline once the document is loaded.
  Markers represent country totals, never device positions or more precise
  locations. Select a marker or country row to inspect its count; markers
  support **Enter / Space**, and zoom controls let the reader inspect the
  map. Unknown or unmapped countries stay in the textual list and are never
  assigned invented positions. Source and transformation details live in
  `telemetry/src/world-map-data.md`.
- **Countries**, **Versions**, and **Platforms** show counts and bars
  relative to each panel's largest row. Additional rows expand in place.
  **Platforms** is `distro` where the row named one and `os` where it did
  not, qualified by `os_version`: `ubuntu 24.04`, `macos 15.3.1`, `arch`,
  `linux`. **Operating systems** shows a ring and counts for the coarse OS
  split. An install can appear in multiple country or system groups, so
  those grouped counts do not imply a global distinct-user total.
- **Daily data** is a native `<details>` table with every day of the window,
  newest first. The footer defines the counts and links to the same numbers
  as JSON.

The theme picker offers **System**, the four Catppuccin flavours
**Mocha / Macchiato / Frappé / Latte**, **Nord**, and **Dracula**, matching
families in the terminal's `/theme` picker. System uses Mocha for a dark
browser preference and Latte for a light one. An explicit choice is saved
only in that browser's local storage under `alter-zero-telemetry-theme`;
it neither changes the terminal's theme nor goes to the collector. Unknown
saved values or unavailable storage fall back to System. Theme colours
apply to every surface, chart, and map, and reduced-motion preferences turn
off animated transitions and smooth scrolling.

The page is also useful without JavaScript: server-rendered charts, the
map, counts, native expandable tables, and ordinary window/JSON links
remain available. Theme, chart-mode, refresh, and zoom controls are hidden.
The enhancements add theme persistence, keyboard chart inspection, map
selection, and section navigation without replacing that underlying page.

The renderer preserves four data and privacy rules:

- **Its own window and JSON links carry only the query token it was given.**
  A reader who authenticated through `Authorization` gives the page no token
  to print, and it never invents one from `DASHBOARD_TOKEN`. A no-referrer
  policy also prevents the dashboard URL from becoming a referrer.
- **Counts describe installs and retained history.** A user is an anonymous
  install ID; a new install is an ID first seen that day within the rows
  still retained. No zero count receives an artificial visible bar.
- **Country rows retain their readable identity.** Flags come from the
  code's regional-indicator letters and names from `Intl.DisplayNames`.
  Unknown codes receive the globe; a malformed stored country value remains
  visible as escaped text. The map does not discard those rows from the
  rest of the dashboard.
- **Telemetry values never become executable source.** Every printed string
  is escaped and every count is coerced at the renderer. The two inline
  scripts are static application code; they read escaped `data-*`
  attributes and update readouts as text. A hand-edited database row cannot
  inject markup or an executable script.

The page lives in `telemetry/src/dashboard.js`, its layout and theme roles
in `dashboard-style.js`, and its two static scripts in `dashboard-client.js`.
`world-map.js` renders the bundled outlines and anchors in
`world-map-data.js`; `lib.js` retains the shared helpers and re-exports the
dashboard renderer. Node's built-in tests cover the collector, renderer,
script behaviour, and map without a network or client framework.

For design work, `cd telemetry && npm run preview` starts a dependency-free
Node server at `http://127.0.0.1:8788`. Its dashboard is labeled **Sample
data** and uses generated numbers, not actual telemetry; `/?empty=1`
previews the empty state. It has no D1 connection and does not collect pings.
The existing `npm run db:init:local` / `npm run dev` Wrangler flow remains
the way to exercise the actual Worker and local D1.

### The table

```sql
CREATE TABLE IF NOT EXISTS pings (
  day     TEXT NOT NULL,  -- the UTC date the ping arrived, YYYY-MM-DD, by the server's clock
  id      TEXT NOT NULL,  -- the install's anonymous id
  country TEXT NOT NULL,  -- ISO 3166-1 alpha-2 from the edge; 'ZZ' when it had none
  version TEXT NOT NULL,
  os      TEXT NOT NULL,
  arch    TEXT NOT NULL,
  distro  TEXT NOT NULL DEFAULT '',  -- the os-release ID; '' off Linux, and from any client older than payload v2
  os_version TEXT NOT NULL DEFAULT '',  -- VERSION_ID, or macOS's ProductVersion; '' when the platform names none
  PRIMARY KEY (day, id)
);
```

The platforms panel groups on `CASE WHEN distro != '' THEN distro ELSE os END`
and `os_version`, so a blank distro is not a bucket of its own: a macOS row
files under `macos`, and a Linux row from a client older than payload v2
files under plain `linux`. An existing deployment gains both columns with
`migrations/0001_platform.sql` (`npm run db:migrate`); a database created from
the current `schema.sql` already has them.

One row per install per day, whatever the client does. A daily cron
(`scheduled`) deletes rows older than `RETENTION_DAYS` (400 by default) so
the table stays a rolling window rather than a permanent record of every
install's every day; "new installs" is derived from the rows that remain, so
past the retention horizon an old install can read as new again — an
accepted imprecision for a counter, not a ledger.

### Deploying it

```bash
cd telemetry
npx wrangler login
npx wrangler d1 create alter-zero-telemetry          # paste the database_id into wrangler.toml
npx wrangler d1 execute alter-zero-telemetry --remote --file=schema.sql
npx wrangler d1 execute alter-zero-telemetry --remote --file=migrations/0001_platform.sql  # only if the table predates payload v2
npx wrangler secret put DASHBOARD_TOKEN              # optional: gate the dashboard
npx wrangler deploy                                  # prints https://alter-zero-telemetry.<subdomain>.workers.dev
```

The client's `telemetry::DEFAULT_ENDPOINT` is
`https://alter-zero-telemetry.linuztx.workers.dev/v1/ping` — the URL
`wrangler deploy` prints for a worker named `alter-zero-telemetry` on the
`linuztx` workers.dev subdomain. **If the printed URL differs, change the
constant** (or give the worker a custom domain and point the constant at it);
until the deployed URL and the constant agree every ping fails silently and
the dashboard stays empty. `ALTER_ZERO_TELEMETRY_URL` overrides the endpoint
for a run — how the smoke suite points a real binary at a local stub, and how
anyone who forks the app points their build at their own collector.

`cd telemetry && node --test` runs the collector's unit tests;
`telemetry/README.md` repeats the deploy steps for someone who never opens
this file.

## Testing

- **Pure** (`src/telemetry.rs`): the file round-trip and its leniency, the id
  mint (a valid id is kept, an invalid one replaced, the hex shape), the
  payload's exact fields, the once-a-day decision, and the environment
  predicate (`DO_NOT_TRACK` outranking `ALTER_ZERO_TELEMETRY`, an unset pair
  deferring to the file).
- **Settings** (`src/settings/tests.rs`, `src/app/tests/settings.rs`,
  `src/ui/tests/settings_view.rs`): the row exists, cycles, is never
  serialized into `settings.json`, never moves through `copy_value`, and
  reads unavailable without a config home.
- **Header** (`src/ui/tests/header.rs`): the wrapped notice.
- **Collector** (`telemetry/test/lib.test.js`): payload validation edge by
  edge, the country normalisation, the day arithmetic, the stats shaping,
  the country label (flag, name, an unnameable code, junk), the byte length a
  body is capped by, the token-carrying links, and that the dashboard escapes
  and coerces what it prints — including that an empty window draws no bar.
- **Collector routes** (`telemetry/test/worker.test.js`): the fetch handler
  over a fake D1 — the `INSERT OR IGNORE` binding *in order*, the edge's
  country reaching the row (and `ZZ` when it has none), a malformed body as a
  400 that writes nothing, the size and method refusals, the stats and
  dashboard shapes, the token gating both read routes but never the ping (and
  reaching the page's own links without ever printing a token the reader did
  not send), each route refusing in its own content type, every response
  being `no-store`/`noindex`, and the retention cron's cutoff. The pure half can be entirely right while the
  handler files every install under the wrong column.
- **Boundary** (`scripts/smoke.sh` Phase 115): a local Python stub stands in
  for the collector; a fresh config home's first launch shows the notice,
  posts exactly the five-field body once, and records the day; the relaunch
  shows no notice and posts nothing; `/settings` turns it off with a toast and
  the file says so; under `DO_NOT_TRACK=1` the row is unavailable, Space
  refuses, and no file or ping appears; and against a collector that answers
  `500`, one attempt is made at boot and **still one** after a turn — the
  rollover check must not retry per turn. Every other phase runs with
  `ALTER_ZERO_TELEMETRY=0`, so the suite never pings anything real.

One thing the suite deliberately does not test is the rollover's *positive*
case — a session crossing midnight and pinging again — which needs control of
the clock. What is covered is that it cannot fire twice in a day and cannot
storm a failing collector; `should_ping` itself is unit-tested.

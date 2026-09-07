# Telemetry

Alter Zero sends **one anonymous ping a day** so the project can answer two
questions: how many people use it, and roughly where they are. That is the
whole purpose. It is on by default, it is disclosed in the app on the first
launch that would send one, and it goes away with a single setting, a single
environment variable, or the `DO_NOT_TRACK` convention.

This document is the complete statement of what leaves your machine. The
implementation is `src/telemetry.rs` and `src/tui/telemetry.rs`; the design
notes are in [`docs/telemetry.md`](docs/telemetry.md), and the collector that
receives the ping is in [`telemetry/`](telemetry/) — nothing here talks to a
third-party analytics service.

## What is sent

One `POST` of about ninety bytes, at most once per UTC day:

```json
{"v":1,"id":"6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980","version":"0.1.0","os":"linux","arch":"x86_64"}
```

| field | what it is |
|---|---|
| `v` | the payload's shape version, so a later shape can be told from this one |
| `id` | the **install id**: 16 random bytes as hex, generated once and kept in `telemetry.json`. It is drawn from the OS random source, not derived from your machine — no MAC address, no hostname, no user name — so it cannot be turned back into you. Delete the file and the next launch generates a new one |
| `version` | the app version, so we know which releases are still in use |
| `os`, `arch` | `linux`/`macos`/`windows` and `x86_64`/`aarch64`, so we know what to build for |

Two headers ride along: `Content-Type: application/json` and
`User-Agent: alter-zero/{version}`. The reply is ignored.

## What is never sent

No prompts. No replies. No file names, paths, or contents. No working
directory. No model, provider, or API key. No hostname, user name, locale,
terminal, or session id. No timings, and no record of which commands or
features you used. Your conversations, your rollout files and your input
history never leave the machine.

There is one function to read if you want to check that rather than take my
word for it — `Ping::to_json` in `src/telemetry.rs` is the entire request
body, and the test `the_payload_carries_exactly_the_five_fields` fails the
build if a sixth field is ever added.

## The country, and your IP address

The ping itself carries no location. The collector reads the country from the
connection at the edge — Cloudflare's own lookup on the incoming request — and
stores the **two-letter code alone** (`PH`, `DE`, `US`, or `ZZ` when it has
none). Your IP address is never stored in the database and never written to a
log; the collector runs with request logging off for exactly that reason.

Doing it this way is deliberate: the alternative is asking your machine to
work out where it is, which means either shipping a geolocation database or
sending an address that should not be sent.

## When it is sent

At startup, after the first frame is drawn, on a background thread. The app
never waits for it — a slow or unreachable collector costs you nothing, and a
failed ping is silent, since there is nothing you could act on.

It is also checked when you start a turn, so a session you leave open across
midnight is counted on the new day rather than only on the day you launched
it. Either way it is one ping per day: launch the app fifty times, or leave it
open and send a hundred messages, and the day still produces exactly one.

The day is recorded only once the collector has actually accepted the ping, so
launching while offline does not burn the day; the next launch that gets
through counts. A collector that keeps failing is tried once per day, not once
per message.

## Turning it off

Any one of these is enough:

| how | scope |
|---|---|
| **`/settings` → Telemetry** | permanent, for your user, in every directory |
| **`ALTER_ZERO_TELEMETRY=0`** | that run (`0`/`false`/`no`/`off` all work; `=1` turns it on for a run) |
| **`DO_NOT_TRACK=1`** | that run — the [cross-tool convention](https://consoledonottrack.com), and it outranks the variable above |

When either variable turns telemetry off, the `/settings` row goes with it:
it reads `false (unavailable)` and will not cycle, so nothing running in the
app can quietly opt you back in. Turning it *off* from the row always works,
whatever the environment says.

```bash
ALTER_ZERO_TELEMETRY=0 alter-zero        # this run
export DO_NOT_TRACK=1                    # every tool that honours it, including this one
```

There is also a case where it never runs at all: **no config home**. Without
`~/.alter-zero` (or `ALTER_ZERO_CONFIG_DIR`) there is nowhere to keep an
install id, and inventing a fresh one each launch would count one person as
many — so nothing is sent, and the `/settings` row reads
`false (unavailable)`.

Turning it off stops the sending; it does not delete what has already been
counted, since a row of `(day, random id, country)` has nothing in it to
identify the person who would ask.

The Telemetry row is the only `/settings` knob that is **not** per working
directory. Every other one is a fact about a project; this is a fact about
you, and an opt-out that quietly applied only to the folder you happened to be
in would be worse than useless.

## The first-run notice

No ping is ever sent before this notice has appeared — not at launch, not
when you switch the row on. The first launch that would send one says so,
once, under the banner:

```
  Alter Zero sends one anonymous ping a day so its users can be counted: the
  app version and OS, and the country the connection came from — never your
  prompts, files, keys or IP address. Turn it off in /settings → Telemetry,
  or with ALTER_ZERO_TELEMETRY=0.
```

A run that will not ping — the variable or your saved choice says off — shows
nothing, because there is nothing to disclose.

## What is kept on your machine

`~/.alter-zero/telemetry.json`, which you are welcome to read or edit:

```json
{
  "enabled": true,
  "install_id": "6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980",
  "last_ping_day": "2026-09-06",
  "notice_shown": true
}
```

Setting `"enabled": false` by hand is the same as cycling the `/settings` row.
Deleting the file resets the install: a new id, and the notice once more.

## What is kept on the server

One row per install per day, and the primary key is what makes a second ping
that day a no-op:

```sql
CREATE TABLE pings (
  day     TEXT NOT NULL,  -- the UTC date the ping arrived, by the server's clock
  id      TEXT NOT NULL,  -- the anonymous install id
  country TEXT NOT NULL,  -- two-letter code from the edge; 'ZZ' when unknown
  version TEXT NOT NULL,
  os      TEXT NOT NULL,
  arch    TEXT NOT NULL,
  PRIMARY KEY (day, id)
);
```

That is the whole schema — there is no other table and no request log. A daily
job deletes rows older than 400 days, so the database is a rolling window
rather than a permanent history.

## Check it for yourself

Point the app at a collector you control and watch exactly what it sends:

```bash
# In one terminal — a server that prints whatever arrives:
python3 -c "
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        print(self.rfile.read(int(self.headers['content-length'])).decode())
        self.send_response(204); self.end_headers()
HTTPServer(('127.0.0.1', 8787), H).serve_forever()"

# In another — a fresh config home, so this is a 'first install':
ALTER_ZERO_TELEMETRY_URL=http://127.0.0.1:8787/v1/ping \
ALTER_ZERO_CONFIG_DIR=/tmp/az-telemetry-check alter-zero
```

The first terminal prints the one line the app sends, and nothing else, for as
long as you leave it running. `scripts/smoke.sh` Phase 115 is the same check,
automated.

## Running your own collector

Forks and self-hosters are the reason `ALTER_ZERO_TELEMETRY_URL` exists.
[`telemetry/`](telemetry/) is a complete Cloudflare Worker over a D1 database
with a dashboard of users per day, per country, per version and per OS;
[`telemetry/README.md`](telemetry/README.md) deploys it in about five
commands. Point your build at it with that variable, or change
`telemetry::DEFAULT_ENDPOINT` in `src/telemetry.rs`.

## Why this is opt-out

An opt-in counter measures the people who go looking for a setting, which is
not the number anyone actually wants to know. The compromise the project makes
in exchange is the rest of this document: the payload is five fields with
nothing personal in it, the disclosure is in the app rather than buried here,
the off switch is one keystroke and honours a convention you may already have
set globally, and both halves of the system are in this repository where you
can read them.

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

One `POST` of about a hundred and twenty bytes, at most once per UTC day:

```json
{"v":2,"id":"6f1c2a4d9e0b7c3a5f8e1d2c4b6a7980","version":"0.1.0","os":"linux","arch":"x86_64","distro":"ubuntu","os_version":"24.04"}
```

| field | what it is |
|---|---|
| `v` | the payload's shape version, so a later shape can be told from this one |
| `id` | the **install id**: 16 random bytes as hex, generated once and kept in `telemetry.json`. It is drawn from the OS random source, not derived from your machine — no MAC address, no hostname, no user name — so it cannot be turned back into you. Delete the file and the next launch generates a new one |
| `version` | the app version, so we know which releases are still in use |
| `os`, `arch` | `linux`/`macos`/`windows` and `x86_64`/`aarch64`, so we know what to build for |
| `distro` | **Linux only**: the distribution's `ID` from `/etc/os-release` — `ubuntu`, `arch`, `fedora`, `nixos` — so we know which distributions to test and package for. On macOS and Windows the field is not sent at all |
| `os_version` | the version of that platform: on Linux the distribution's own `VERSION_ID` (`24.04`), on macOS the system's `ProductVersion` (`15.3.1`). Not sent when the platform names none — a rolling release like Arch — and not sent on Windows |

### About `distro` and `os_version`

Together they say `ubuntu 24.04` or `macos 15.3.1`, which is the whole point
of them: a build that works on one distribution's glibc may not work on
another's, and knowing which versions people are actually on is the
difference between guessing and knowing what to build against.

They come from **two lines of two files**, and nothing else on either:

| your system | where it is read | what is taken |
|---|---|---|
| Linux | `/etc/os-release` (then `/usr/lib/os-release`) | the `ID=` and `VERSION_ID=` lines |
| macOS | `/System/Library/CoreServices/SystemVersion.plist` | the `ProductVersion` key |
| Windows | nothing is read | neither field is sent |

Everything else in those files is left where it is. Not `PRETTY_NAME`, not
`BUILD_ID`, not `VARIANT`, not `HOME_URL`; on macOS not `ProductBuildVersion`
(`24D70`) and not the serial number, hardware model, or anything else about
the machine — the plist holds none of that, and this reads one key out of it.
So `ubuntu` and `24.04`, never `Ubuntu 22.04.3 LTS (Jammy Jellyfish)`.

Nothing is run to find this out: both are file reads, so no `sw_vers`, no
`lsb_release`, no subprocess of any kind.

Some systems name less than that, and the ping says so rather than
inventing a value:

- **A rolling release** — Arch, Void, Debian sid — has no `VERSION_ID`, so no
  `os_version` is sent. `arch` on its own is more honest than `arch` with a
  number attached to it.
- **No `ID`, or no os-release file at all** (a minimal container) reads
  `linux`, which is what the [os-release
  spec](https://www.freedesktop.org/software/systemd/man/os-release.html)
  says to default to.
- **Windows** sends neither: reading its version needs a registry crate or a
  subprocess, and neither is worth adding for this.

And if either value does not fit the shape the collector accepts, that field
is dropped and the rest of the ping is sent without it.

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
body, and the test `the_payload_carries_exactly_the_seven_fields` fails the
build if an eighth field is ever added.

## The country, and your IP address

The ping itself carries no location. The collector reads the country from the
connection at the edge — Cloudflare's own lookup on the incoming request — and
stores the **two-letter code alone** (`PH`, `DE`, `US`, or `ZZ` when it has
none). Your IP address is never stored in the database and never written to a
log; the collector runs with request logging off for exactly that reason.

Doing it this way is deliberate: the alternative is asking your machine to
work out where it is, which means either shipping a geolocation database or
sending an address that should not be sent.

To slow spam, the collector also uses the connection's address in memory to
apply a limit of **60 ping attempts per minute** from one IPv4 address or
IPv6 network prefix (`/64`). People behind the same network share that
budget. Cloudflare's enforcement is approximate and applies separately at
each edge location.

The rate limiter receives only a keyed cryptographic digest that changes
each UTC day. The collector does not keep the address or a mapping back to
it, save that digest in the telemetry database, associate it with install
IDs, or send it to the app. This is separate abuse-prevention state, not
another field in your daily ping or a new location record. Excess requests
are refused before they can add a row; the app remains silent on a refused
ping, just as it does when the collector is unavailable.

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
  ╭─ Telemetry ──────────────────────────────────────────────────────────────╮
  │  Alter Zero sends one anonymous ping a day to count active users.        │
  │  Shares App version, OS and connection country.                          │
  │  Never Your prompts, files, keys or IP address.                          │
  │                                                                          │
  │  Opt out /settings → Telemetry or ALTER_ZERO_TELEMETRY=0                 │
  ╰──────────────────────────────────────────────────────────────────────────╯
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

There is no rate-limit table or request log. Cloudflare's rate limiter keeps
separate counters keyed by the daily digest described above. A daily job
deletes telemetry rows older than 400 days, so the database is a rolling
window rather than a permanent history.

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
with a dashboard of users per day, per country, per app version, per OS and
per platform (`ubuntu 24.04`, `macos 15.3.1`);
[`telemetry/README.md`](telemetry/README.md) deploys it in about five
commands. Point your build at it with that variable, or change
`telemetry::DEFAULT_ENDPOINT` in `src/telemetry.rs`.

## The update check

One other request leaves your machine on its own: **once a day**, after the
first frame, the app asks whether a newer release is out, so it can say so
under the banner and point you at `alter-zero update`. It is a separate
feature with a separate switch, described in [`docs/update.md`](docs/update.md),
and it belongs in this document because the document promises to be the
complete list.

The whole request is one `HEAD` of the repository's releases page —

```
HEAD https://github.com/linuztx/alter-zero/releases/latest
User-Agent: alter-zero/0.1.0
```

— and the answer is a redirect to `…/releases/tag/vX.Y.Z`, whose tag is the
version. **No body, no install id, no query string**: nothing identifies the
install, and nothing is recorded by this project. GitHub sees the connection
the way it sees you opening that page in a browser, no more. It is not the
GitHub API and it downloads nothing; only `alter-zero update`, when you run
it, fetches the installer and a release archive — and that command is your
own request, so it runs whatever the switches below say.

It is not covered by `DO_NOT_TRACK`, because it is not tracking: the
convention is about analytics, and this measures nothing. It has its own
switches instead, because a request to github.com you did not type is still
one you may not want:

| how | scope |
|---|---|
| **`/settings` → Update check** | permanent, for your user, in every directory |
| **`ALTER_ZERO_UPDATE_CHECK=0`** | that run (`0`/`false`/`no`/`off`; `=1` turns it on for a run) |

As with Telemetry, the variable withdraws the `/settings` row (`false
(unavailable)`) so nothing in the app can quietly turn it back on, and without
a config home nothing runs at all. What is kept is
`~/.alter-zero/update.json`: the switch, the last day a check was attempted,
the newest version the last check found, and the last day the notice was
shown. `ALTER_ZERO_UPDATE_URL` points the check at another repository — a
fork of your own — and `scripts/smoke.sh` Phase 116 drives the whole thing
against a local stand-in server, so you can watch the one request it makes.

## Why this is opt-out

An opt-in counter measures the people who go looking for a setting, which is
not the number anyone actually wants to know. The compromise the project makes
in exchange is the rest of this document: the payload is five fields with
nothing personal in it, the disclosure is in the app rather than buried here,
the off switch is one keystroke and honours a convention you may already have
set globally, and both halves of the system are in this repository where you
can read them.

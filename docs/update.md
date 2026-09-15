# Update notice — the once-a-day release check and `alter-zero update`

**One request a day, a card under the banner when a newer release is out,
and one command that installs it.** A binary installed with the one-line
installer (`docs/release.md`) has no package manager behind it to say a
release shipped, so the app asks the repository itself — `HEAD
https://github.com/linuztx/alter-zero/releases/latest`, once per UTC day,
after the first frame — reads the tag off the redirect it lands on, and, when
that tag is newer than `CARGO_PKG_VERSION`, says so **once a day** in the
telemetry card's frame:

```
  ╭─ Update available ───────────────────────────────────────────────────────╮
  │  Alter Zero v0.2.0 is out — you are running v0.1.0.                      │
  │  What's new https://github.com/linuztx/alter-zero/releases/tag/v0.2.0    │
  │  Update run alter-zero update, then restart.                             │
  │                                                                          │
  │  Turn off /settings → Update check or ALTER_ZERO_UPDATE_CHECK=0          │
  ╰──────────────────────────────────────────────────────────────────────────╯
```

`alter-zero update` then does the install: the same check, and — when a
newer release exists — the one-line `install.sh` fetched to a temp file and
run over the directory the running binary lives in, pinned to the version the
check just resolved. The pure half is `src/update.rs`; the boundary is
`src/tui/update.rs` (the check, the card, the file) and `src/tui/update_cli.rs`
(the subcommand); the design is `tui::telemetry`'s, rule for rule, so the two
once-a-day features that touch the network read the same way.

The user-facing statement — what the request carries, how to turn it off —
is the *Update check* section of the root `TELEMETRY.md`, which promises to
be the complete list of what leaves a machine and so must name this request
too. Change one and re-read the other.

## What is sent

One `HEAD` request per UTC day per install, to
`{repo}/releases/latest`, carrying:

| header | value |
|---|---|
| `User-Agent` | `alter-zero/{version}` — GitHub refuses a request with none |

That is the whole request: **no body, no install id, no query string**. It is
the request a browser makes opening the releases page, minus the page. What
comes back is a `302` to `/releases/tag/vX.Y.Z`; the client follows it and
`update::version_from_release_url` reads the tag off the URL it landed on
(`response.url()` — the query string and fragment stripped, the `v` dropped,
the remainder required to parse as a version). The body is never read.

Three things this deliberately is not:

- **Not the REST API.** `api.github.com/repos/…/releases/latest` answers
  with a JSON document of every asset and its uploader, rate-limited at 60
  unauthenticated requests an hour per address — a shared office is over
  that by lunch — and the body would have to be parsed for one string.
  The redirect carries the same string in its `Location` header and is not
  rate-limited that way. It also keeps the memory posture: no `Value` tree of
  a body we read one field from (`docs/memory.md`).
- **Not a ping.** Nothing identifies the install, and nothing is recorded by
  this project — GitHub sees the connection the way it sees a page load, and
  that is all. Which is also why `DO_NOT_TRACK` does **not** cover it: the
  convention is about analytics, and this measures nothing. It has its own
  switches instead (below), because a request to github.com the user did not
  type is still a request the user may not want.
- **Not a download.** The check fetches no binary; only `alter-zero update`
  does, and only when asked.

## When, and what it costs

- **At launch, after the first frame** (`Session::bootstrap` calls
  `start_update_check` after `start_telemetry`, both behind
  `paint_first_frame`), on a detached worker (`workers::spawn_update_check`).
  The app never waits for it.
- **At any turn start that opens a new UTC day** (`Session::update_day_check`
  beside `telemetry_day_check`): a session left open across midnight checks
  on the new day too.
- **Once per day, whatever happens to the request.** The attempt's day is
  recorded in `update.json` *at the spawn* (`record_check(today, None)`), not
  when an answer arrives, so a dead network, a repository with no release
  yet, or a proxy that refuses costs one request per day per install and
  nothing the user sees — there is nothing they could act on. The session
  additionally remembers the day it tried (`Session::update_attempted`) so a
  turn start on the same day costs a date read and a string compare.
- **The worker reports, the loop writes.** The worker sends the version it
  found on its own `select!` channel (`update_rx`), and
  `Session::on_update_result` records it — `update.json` has one writer
  thread, and the `/settings` toggle can never race it. Every write is
  `config::update_update_file`'s read-modify-write, so a result recorded while
  the user was turning the check off cannot resurrect their `true`.

## The card

`ui::update_notice_lines(width, current, latest, repo)` renders
`update::notice`'s text in the telemetry card's frame — both go through
`ui::notice_card_lines(heading, body, width)`, the rounded `DEVICE_BOX_*`
box titled in the header accent, the body wrapped to the box with the
`**bold**` labels and the accent code spans `ui::inline` already gives a
reply — so the two cards under the banner are one design. The body says four
things and stops: the release and the running version, the release page
(`update::release_page_url`, a real link in a terminal that draws them —
`docs/links.md`), the command that installs it, and the off switch.

Three rules decide *when* it shows:

- **Once a day.** `update::notice_due(file, current, today)` answers the
  version to announce only when the file's `latest` is newer than this build
  **and** `notice_day` is not today. A launch that already showed the card
  shows it again tomorrow, not on every launch — a notice you cannot act on
  in the next minute is one you stop reading.
- **After the first frame, never inside a reply.** A result that lands while
  a turn streams would interleave with the text the model is producing, so it
  waits in `Session::update_notice_pending` and
  `Session::flush_pending_update_notice` — run from `on_update_result` and
  from the loop bottom (`after_iteration`) — commits it at the first idle
  moment: no turn in flight, nothing streaming, commits allowed (invariant 4,
  the agent-view rule). Before committing it re-reads the file and re-asks
  `notice_due`: the day may have rolled, or another launch may have shown the
  card since the result was queued.
- **Chrome, never `history`.** The card rides `insert_before` like the banner
  and the telemetry notice; a purge rebuild (a resize, `/clear`) does not
  re-emit it. It is a notice, not a message.

A launch that already knows of a newer release (the file's `latest` from
yesterday's check) announces it from the file before spawning today's
request, so the card does not wait on the network — and a build *ahead* of
the newest release (a checkout) sees no card, since `is_newer` is a
comparison and not a difference.

## Versions

`update::parse_version` reads `MAJOR.MINOR.PATCH[-pre][+build]` with or
without the tag's `v`, refusing anything else (a `nightly` tag, `0.1`, a
leading zero) so that a repository whose latest release is not a version
compares as *not newer* rather than as *newer* — garbage is never an update.
`Version`'s `Ord` is semver's: numeric fields as numbers (`0.10.0 > 0.9.0`,
which a string compare gets wrong), a pre-release **older** than its release
(`0.2.0-rc.1 < 0.2.0`, so the release supersedes its own candidate),
pre-release identifiers dot-split and compared numerically where both are
numbers, build metadata ignored.

## `update.json`

The per-**user** file beside `telemetry.json` (`docs/per-directory-state.md`
— a check you turned off in one directory and not another would be a
surprise, the Telemetry rule):

```json
{
  "enabled": true,
  "last_check_day": "2026-09-12",
  "latest": "0.2.0",
  "notice_day": "2026-09-12"
}
```

| field | what it is |
|---|---|
| `enabled` | the `/settings` **Update check** row's value; `false` by hand is the same as cycling it |
| `last_check_day` | the last UTC day a request was *attempted* — set at the spawn |
| `latest` | the newest release the last successful check found, so a launch can announce it without waiting on the network |
| `notice_day` | the last day the card was shown — the once-a-day bound on the notice itself |

Deleting the file resets all four: the check is on, the next launch asks
again, and a newer release is announced again.

## Turning it off

| how | scope |
|---|---|
| **`/settings` → Update check** | permanent, for your user, in every directory |
| **`ALTER_ZERO_UPDATE_CHECK=0`** | that run (`0`/`false`/`no`/`off`; `=1` turns it on for a run) |
| **no config home** | nothing runs — there is nowhere to remember the day, and a check per launch is not once a day |

The variable **withdraws the `/settings` row** rather than seeding it —
`false (unavailable)`, refusing to cycle — the Telemetry rule
(`config::update_check_forbidden_by_env` → `SettingAvailability::update_check`):
an opt-out a keystroke could undo is not one. `ALTER_ZERO_UPDATE_URL` points
the check (and `alter-zero update`) at another repository root — a fork, or
the smoke suite's stand-in server — and `smoke.sh`'s base environment sets
`ALTER_ZERO_UPDATE_CHECK=0` so no other phase reaches github.com.

## `alter-zero update`

The subcommand is routed at the same pre-TUI boundary as `mcp`
(`cli::parse` → `Cli::Update`, `tui::startup::resolve_cli` →
`tui::update_cli::run`, `docs/cli.md`): a first argument of `update`, taking
nothing — `alter-zero update now` is a usage error, not a guess. It runs in
cooked mode before the runtime and the terminal exist, prints as it goes, and
exits `0` when the binary is already the newest release or the installer
finished, `1` when it could not check, fetch or install.

```
$ alter-zero update
Checking https://github.com/linuztx/alter-zero for a newer release…
alter-zero v0.2.0 is out — this is v0.1.0. Installing it over /home/me/.local/bin.
  …the installer's own output…
```

What it does, in order:

1. **Refuses a build directory.** `update::is_cargo_build_dir` recognises
   `…/target/debug/alter-zero` and `…/target/release/alter-zero` (and cargo's
   per-target `target/{triple}/release/` shape) and stops with `update the
   checkout instead: git pull && cargo build --release`. Overwriting a
   checkout's build output with a release binary would be *undone* by the
   next `cargo build`, silently.
2. **The same check** — `update::fetch_latest`, the `HEAD` above — and
   `is_newer`. Already newest prints so and exits `0`.
3. **Fetches the installer** — `install.sh` from the repository's `main`
   branch (`update::installer_url`: `raw.githubusercontent.com/{slug}/main/
   install.sh` for a github.com repository, `{repo}/install.sh` for any other
   root, which is what lets the stand-in server serve it) — **to a temp
   file**, never a pipe. `sh` reading an empty pipe exits `0`; a fetch that
   fails must fail loudly. `update::looks_like_installer` checks the shape
   (`#!/bin/sh`, the `main "$@"` trailer, the `ALTER_ZERO_INSTALL_DIR`
   variable) before anything is run, so an HTML error page served with a
   `200` is refused as *did not serve the installer*.
4. **Runs it** with three variables the installer already honours
   (`docs/release.md`): `ALTER_ZERO_INSTALL_DIR` = the running binary's own
   directory, so the update lands over the binary that asked for it (not
   `~/.local/bin` if that is not where it lives); `ALTER_ZERO_VERSION` =
   `v{latest}`, the tag the check resolved, so the two halves cannot disagree
   about which release is newest; `ALTER_ZERO_INSTALL_BASE_URL` = the same
   repository the check read. The installer's checksum verification, the
   platform pick and the `PATH` advice are unchanged — the subcommand adds no
   second install path.

The subcommand ignores `ALTER_ZERO_UPDATE_CHECK`: that variable silences the
*automatic* check, and an explicit command is the user's own request.

The next launch on the new binary also sends the day's telemetry ping again,
carrying the new version — the one exception to that ping's once-a-day rule
(`docs/telemetry.md` *When it is sent*) — so the dashboard's Versions panel
moves the install the same day rather than at midnight UTC. It is the same
install id, so it counts as the same user on a new version, never as a new
install.

## The `/settings` row

**Update check** sits above **Telemetry** (`SettingKey::UpdateCheck`), the
sixteenth row: `true`/`false`, per user (`SessionSettings::update_check` is
`#[serde(skip)]`, `copy_value` never moves it, `apply_setting` skips the
`settings.json` write), seeded at bootstrap from `update.json` then the
environment, `false (unavailable)` without a config home or under the
variable. Cycling it on takes today's step at once
(`Session::apply_update_setting` → `update_tick`): a newer release the file
already knows of is announced, and the day's request is sent when none has
run. Cycling it off sends nothing more; a result already in flight still
records its day.

## Testing

- **Unit** (`update::tests`): the version grammar and ordering, the tag read
  off a redirect URL (query strings, a `nightly` tag, the bare `/releases`
  page), the once-a-day rules for the check and the notice, the URLs for a
  github.com repository and for any other root, the environment predicate,
  the notice text, the build-directory recognition and the installer shape
  check. `ui::tests::header` pins the card's frame and body;
  `settings::tests` the row's position, cycling, availability and its absence
  from `settings.json`; `cli::tests` and `tests/cli_help.rs` the `update`
  routing, the usage error for an argument, and the help page's rows.
- **Smoke** (`scripts/smoke.sh 116`): `scripts/release/release_server.py` —
  the stand-in for github.com's release pages the release tooling already
  uses — gained an optional third argument, the installer to serve at
  `/install.sh`. The phase packages a fake `v9.9.9` release from the debug
  binary with `package_dist`, points `ALTER_ZERO_UPDATE_URL` at the server,
  and drives: a fresh home showing the card and recording all four fields;
  a relaunch the same day showing no card; the `/settings` row cycling into
  `update.json` and never `settings.json`; `ALTER_ZERO_UPDATE_CHECK=0`
  withdrawing the row and writing no file; a server whose latest release *is*
  the running version recording `latest` with no `notice_day`; `alter-zero
  update` from a copy of the binary against that server (`is the newest
  release`, exit `0`), from `target/debug` (refused, exit `1`), and against
  `v9.9.9` — the copy replaced by the served archive's binary (a new inode)
  and still answering `--version`, since the fake release is this build.

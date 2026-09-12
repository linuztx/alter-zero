# Releases — the tag, the workflow, the assets, the notes

Date: 2026-09-12

## Goal

Cutting a release is one short command sequence, and a tag push does the
rest: the project's gate, a release build for every supported platform,
checksums, release notes, and the GitHub release that carries them. Nothing
about a version is typed twice, every step the workflow runs is a command a
developer can run on a laptop, and the whole pipeline can be rehearsed —
builds, verification, notes — without publishing anything. There was no CI
and no packaged release before this; `v0.1.0` is the first, and the tooling
is what lets it be cut with confidence rather than by hand.

## The shape

```
scripts/release.sh prepare X.Y.Z     # bump + roll the changelog (a commit's worth of edits)
git commit … && git tag -a vX.Y.Z -m "alter-zero vX.Y.Z" && git push origin HEAD vX.Y.Z
        │
        ▼  .github/workflows/release.yml
   check ──► gate ──► build ×4 ──► publish
```

1. **`check`** — the tag names Cargo.toml's version, and Cargo.lock, the
   README's version badge and `CHANGELOG.md` all agree with it; then the
   tooling's own `selftest` and `shellcheck`. Every mismatch is reported
   before the job fails, so one run names all of them.
2. **`gate`** — the pre-commit gate from `CLAUDE.md`, unchanged: `cargo fmt
   --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
   `cargo doc --no-deps --lib`.
3. **`build`** — one job per target (the matrix below), each running
   `scripts/release.sh build TARGET` then `scripts/release.sh verify dist`
   on its own output, and uploading `dist/` as a workflow artifact.
4. **`publish`** — downloads every `dist/`, verifies them *together*,
   renders the notes (`scripts/release.sh notes`) into the job summary, and
   either publishes (`scripts/release.sh publish`) or, on a rehearsal,
   prints the exact `gh` commands it would have run.

Every job but `publish` holds `contents: read`; `publish` alone is granted
`contents: write`, and only the `GH_TOKEN` step uses it.

### What a release carries

| Asset | Platform | Built on |
| --- | --- | --- |
| `alter-zero-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 (glibc ≥ 2.35) | `ubuntu-22.04`, native |
| `alter-zero-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64 (glibc ≥ 2.35) | `ubuntu-22.04`, cross-compiled with `gcc-aarch64-linux-gnu` |
| `alter-zero-vX.Y.Z-x86_64-apple-darwin.tar.gz` | macOS Intel | `macos-latest` (arm64), cross-compiled by Apple's clang |
| `alter-zero-vX.Y.Z-aarch64-apple-darwin.tar.gz` | macOS Apple silicon | `macos-latest`, native |

Each archive unpacks to a directory of the same stem holding the
`alter-zero` binary (mode 755), `LICENSE`, `README.md` and the release's
`CHANGELOG.md` — nothing else, and `verify` enforces exactly that list.
Beside every archive sits `{archive}.sha256`, one line in `sha256sum -c`
form (`<hex>  <filename>`), and `SHA256SUMS` collects every such line so a
user can check any download with one command. Assets are named by the full
Rust target triple rather than a friendlier `linux-arm64`: the triple is
unambiguous about the libc, it is what `rustup target add` and `cargo build
--target` take, and `verify` parses the target straight back out of the
name to decide what `file(1)` must say about the binary inside.

The binary is the one Cargo.toml documents for a packaged release —
`cargo build --release --locked --target T --config
profile.release.incremental=false`. `--locked` refuses a build whose
Cargo.lock is stale (a bumped Cargo.toml without its lock entry), and the
incremental release profile is a developer's rebuild speed, not a shipped
layout (`docs/build-time.md`). It is **not stripped**: the release profile
already drops debuginfo, and the symbol table that remains is what turns a
user's `RUST_BACKTRACE=1` report into function names. It costs ~7 MB of a
~22 MB binary that compresses to a ~9 MB archive; `--config
profile.release.strip=true` on the build line is the knob if that ever
matters more.

### The notes

A release's notes are `CHANGELOG.md`'s `## [X.Y.Z]` section, verbatim, then
an **Assets** table (each archive linked to its download URL, its platform,
its SHA-256 read from the `.sha256` beside it), a verify-and-install
snippet, and the compare link to the previous release (`…/commits/vX.Y.Z`
for the first). The changelog is the one place a release is described —
`notes` reads it and never paraphrases — so the entry written there is the
entry users read, and `check` refuses a tag whose section is missing or
empty. GitHub's auto-generated "what's changed" commit list was considered
and not used: a commit subject is written for a reviewer, a changelog entry
for a user, and the two rarely read the same.

## The installer

```bash
curl -fsSL https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh | sh
```

`install.sh` at the repository root is the one line the README leads with.
It maps `uname` to a target triple (refusing Windows, musl, and a glibc
older than the 2.35 floor with a message that points at building from
source), reads the latest tag off github.com's `/releases/latest` redirect
(no API call, so no rate limit — `ALTER_ZERO_VERSION=vX.Y.Z` or `--version`
pins one instead), downloads the archive **and** its `.sha256`, refuses to
go on without the checksum file or with one that does not match, extracts,
copies the binary into `~/.local/bin` (`ALTER_ZERO_INSTALL_DIR` or `--dir`
for elsewhere) beside any old one and moves it into place so a running
`alter-zero` keeps its mapped file and the new one appears whole, runs the
installed binary's `--version` as the last check, and ends with what to do
next — the PATH line for the user's own shell when the directory is not on
it, and how to uninstall. It never prompts: under `curl | sh` its stdin
*is* the script.

It is POSIX `sh`, not bash — no arrays, no `local`, no `[[` — because the
pipe target is whatever `sh` is (bash 3.2 in POSIX mode on macOS, dash on
Debian, busybox on a container), and the whole file is functions with a
single `main "$@"` on the last line, so a connection that drops mid-download
hands `sh` an incomplete file that defines nothing and runs nothing. The
output wears the app's own look: the `crest` mascot in the banner gradient
(truecolor when `COLORTERM` says so, the terminal's cyan otherwise), the
`→`/`✔`/`✘` step glyphs, and none of it — no colour, ASCII glyphs — in a
pipe, under `NO_COLOR`, or outside a UTF-8 locale.

The script is served from `main` rather than attached to each release
because the URL then never changes: an asset's name carries the version,
and the installer resolves the version itself. That also means a fix to the
installer reaches users without a release. Its offline test double is
`scripts/release/release_server.py`, a stand-in for github.com's release
pages — the redirect, the tag page, the download URLs over a directory —
that the selftest points `ALTER_ZERO_INSTALL_BASE_URL` at, so every path
through the installer runs against real archives with no network: the
latest and a pinned release, a re-install over itself, a release with no
asset for the machine, a tampered checksum (nothing installed, and the
output says so), `--help`, and an unknown flag.

The app drives the same script from inside: `alter-zero update`
(`docs/update.md`) reads the latest tag off the same redirect and, when it is
newer than the running binary, fetches `install.sh` to a temp file and runs
it with `ALTER_ZERO_INSTALL_DIR` set to the binary's own directory,
`ALTER_ZERO_VERSION` to the tag it just read and `ALTER_ZERO_INSTALL_BASE_URL`
to the same repository — so the in-app update *is* the one-line install with
its three variables filled in, and there is no second install path to keep
correct. `release_server.py` therefore also serves the installer at
`/install.sh` (its optional third argument), which is what lets `smoke.sh`
Phase 116 run that command end to end against a packaged fake release.

## The scripts

`scripts/release.sh` is one entry point over the steps under
`scripts/release/` — the smoke suite's runner-over-phases shape: each step
is its own file sourcing `scripts/release/lib.sh`, the workflow is a thin
caller of the same commands, and `RELEASE_ROOT` points every reader at a
tree, which is how the selftest drives them against fixtures.

| Command | What it does |
| --- | --- |
| `version` | Prints Cargo.toml's `[package]` version. |
| `check [TAG]` | Rules 1–6 below; reads `GITHUB_REF_TYPE`/`GITHUB_REF_NAME` when no TAG is given. |
| `build [TARGET] [--dist DIR]` | `rustup target add` if needed, the cross-toolchain variables for a Linux GNU target, the documented `cargo build`, then `package_dist` into `dist/`. |
| `verify [DIST] [--version X]` | Every archive's name, checksum, layout, executable bit, `file` format/CPU, and — when this machine can run it — `--version`; `SHA256SUMS` agreement; no stray files. |
| `notes VERSION [DIST]` | The markdown above, on stdout. |
| `publish VERSION [DIST] [--notes FILE] [--dry-run]` | `verify`, assemble `SHA256SUMS`, then `gh release create` **as a draft** with notes and every asset, then `gh release edit --draft=false` (`--latest` unless a pre-release). |
| `prepare VERSION` | The version bump and changelog roll, then `check`, then the commit/tag/push to run. |
| `selftest` | 120-odd fixture-driven assertions over all of the above. |

`check`'s rules:

1. Cargo.toml's `[package]` version is semver (`X.Y.Z`, optionally `-pre`
   and `+build`); a hyphenated version is a **pre-release**, which `publish`
   flags and never marks latest.
2. Cargo.lock records the same version for the crate.
3. The README's version badge agrees — both the URL label (shields.io
   escapes `-` as `--`, so `0.2.0-rc.1` rides it as `0.2.0--rc.1`) and the
   alt text.
4. `CHANGELOG.md` has `## [X.Y.Z] - YYYY-MM-DD` with a non-empty body and a
   `[X.Y.Z]: url` link reference.
5. `CHANGELOG.md` keeps `## [Unreleased]` and its link.
6. With a tag: it is `vX.Y.Z`, and `[Unreleased]` is **empty** — an entry
   left there was written for this release and never rolled into it.

CI runs `check` with no tag on every push, so the tree is held to rules 1–5
at all times: the version in Cargo.toml is always the latest released one or
the one about to be released, never a "next version" bumped at the start of
a cycle, and a pull request that bumps it without a changelog section fails
CI rather than the eventual tag.

`verify`'s CPU check is what makes cross-compilation safe to automate. A
cross build that quietly fell back to a host binary (a stale `target/`
artifact, a mis-set linker) would upload a perfectly good archive with the
wrong machine code inside; `file -b` on the extracted binary must name the
target's object format and CPU (`ELF 64-bit` + `aarch64`, `Mach-O 64-bit` +
`x86_64`, …), and the selftest proves the check fires by packaging a host
binary under another target's name.

`publish`'s order is what makes a failed upload harmless. The release is
created as a **draft** with its notes and every asset in one `gh` call, so
nothing is visible until everything is there, and only then flipped to
published. A published release for the tag is **never** touched — releases
are immutable here; a fix is a new version — while a leftover draft (a run
that died mid-upload) is deleted and recreated. `--verify-tag` refuses to
mint a tag: the tag push is the trigger, and a release without a tag in the
repository would be a release of nothing.

`prepare` rewrites four files atomically (through a temp file and `mv`, no
`sed -i`, whose argument GNU and BSD disagree on): Cargo.toml's `[package]`
version and nothing else's, Cargo.lock's entry for the crate (exactly what
`cargo update --workspace` would write), the README badge's label and alt
text, and `CHANGELOG.md`, where `## [Unreleased]` stays, empty, over a new
`## [X.Y.Z] - <today>` heading that takes its body, and the link block
gains the version's compare link (or the tag link for a first release) with
`[Unreleased]` re-pointed past it. It refuses a version that is not semver,
is the current one, already has a section or a local tag, or would release
an empty `[Unreleased]` — the notes would be empty too.

## Rehearsing

**On GitHub, before the first tag**: run the Release workflow by hand
(Actions → Release → *Run workflow*, `publish` off). It does everything a
tag push does — `check`, the gate, all four builds, `verify` on each and on
all together — renders the notes into the job summary, keeps every `dist/`
and the notes as artifacts for seven days, and prints the `gh` commands it
would have run. This is the recommended first run: it exercises the
platforms a laptop cannot (the two macOS builds, the arm64 cross build on a
runner) with nothing at stake.

**Locally** (the same commands, the host's platform):

```bash
scripts/release.sh check                       # the tree agrees with itself
scripts/release.sh selftest                    # the tooling agrees with its fixtures
scripts/release.sh build                       # dist/alter-zero-vX.Y.Z-<host>.tar.gz (+ .sha256)
scripts/release.sh verify dist                 # …checksum, layout, CPU, `alter-zero --version`
scripts/release.sh notes X.Y.Z dist            # the notes the workflow would publish
scripts/release.sh publish X.Y.Z dist --dry-run
```

A Linux box with the cross toolchain also builds the arm64 asset
(`apt-get install gcc-aarch64-linux-gnu libc6-dev-arm64-cross`, then
`scripts/release.sh build aarch64-unknown-linux-gnu`; `verify` checks its
CPU and skips running it). The macOS assets build on a Mac, both CPUs from
either.

**Publishing by hand**, if the automatic run failed after the builds: run
the workflow on the tag with `publish` on, or locally `scripts/release.sh
publish X.Y.Z dist` with `gh` signed in (the dry run shows the exact calls).

## Cutting a release

```bash
scripts/release.sh prepare 0.2.0               # edits Cargo.toml, Cargo.lock, README.md, CHANGELOG.md; runs check
git add Cargo.toml Cargo.lock README.md CHANGELOG.md
git commit -m "Release v0.2.0"
git tag -a v0.2.0 -m "alter-zero v0.2.0"
git push origin HEAD v0.2.0
```

`v0.1.0` needs no `prepare`: Cargo.toml already says `0.1.0` and
`CHANGELOG.md` carries its section, so the first release is the tag alone —
`git tag -a v0.1.0 -m "alter-zero v0.1.0" && git push origin v0.1.0` from
`main` — ideally after one rehearsal run. The section's date is the day it
was written; edit it to the tag's day if that differs.

While a version is in progress, changes land under `## [Unreleased]` in
Keep a Changelog's groups (`Added`, `Changed`, `Deprecated`, `Removed`,
`Fixed`, `Security`), written for the user who reads the release notes, not
the reviewer who reads the commit.

## CI

`.github/workflows/ci.yml` runs on every push to `main` and every pull
request: **gate** (the four commands), **smoke** (the tmux suite over the
real binary), **release tooling** (`shellcheck` over the scripts and
`install.sh`, `selftest`, `check`), and
**telemetry** (the collector's `node --test`). The smoke job runs one
worker per core rather than the suite's default of twice that: a `-j 8`
run on a four-core box here failed phases 58 and 61 — the permission
prompt "never showed" inside its wait — and both passed alone. So the job
re-runs whatever failed in the parallel pass serially, once, before it is
called red, and keeps both passes' logs as an artifact whenever the first
pass failed, so a flake leaves evidence instead of a green square. The release workflow repeats the gate and the
tooling jobs rather than trusting a CI run that may not have happened on
the tagged commit. The smoke suite is in CI because it is the only
automated coverage of the terminal I/O boundary (`docs/smoke.md`); it is
not in the release workflow, whose tag is a commit CI has already proven.

## Decisions

**Hand-written GitHub Actions plus the `gh` CLI, not cargo-dist or a
release action.** `cargo-dist` generates a several-hundred-line workflow
with its own installers and its own notion of the changelog, and changed
maintainers in 2025; the release actions (`softprops/action-gh-release`
and kin) are a second trust root for the one step that has write access.
`gh` is preinstalled on every runner and signed in with the job's own
token, so publishing is two commands anyone can read and run, and the
logic that decides *what* to publish lives in shell that runs locally. The
price is the installers cargo-dist would have generated — listed under
follow-ups.

**The scripts are the source of truth; the workflow is a caller.** Every
`run:` is a `scripts/release.sh` command (the smoke suite's arrangement),
so there is no YAML-only logic to get wrong without a way to run it, and
the fixture selftest covers the logic rather than a workflow run.

**Four targets, glibc, no musl, no Windows.** The crate is unix-shaped —
`rustix` termios and `setsid`, the tty detach, `/proc` reads — and has
never been built for Windows; a target the tests cannot cover is not a
target to ship. A static musl Linux build would run on any distribution,
but musl's allocator is markedly slower on the allocation-heavy render
path, and every resident-memory figure in `docs/memory.md` was measured
against glibc's allocator; shipping musl means measuring again first. The
Linux assets build on `ubuntu-22.04` rather than `-latest` to pin their
glibc floor at 2.35: a release built on a newer image would silently stop
running on every older distribution. arm64 is cross-compiled with the
distribution's `gcc-aarch64-linux-gnu` instead of built on an arm64
runner, because it needs no special runner label and the `cc` crate
(oniguruma, `ring`) finds the cross compiler by the triple's prefix on
every plan.

**Reproducible archives.** `package_dist` stamps every staged file with the
binary's own mtime (its build time), archives without the builder's uid/gid,
in name order, and gzips without a timestamp, so two packagings of the same
binary are byte-identical whenever they run. The mtime line is the one that
had to be learned: `cp` stamps each copy with *now*, so the check passed only
while both packagings landed inside the same second, and the selftest now
sleeps a second between them to keep that from coming back. The binary itself
is not claimed reproducible across machines.

**Versions are read in four places because they are written in four.** A
single source would be better; short of generating the README, the next
best thing is a check that fails when any copy drifts, and a `prepare` that
edits all four so nobody has to remember.

**Actions pinned to major tags, moved by Dependabot.** `actions/checkout`,
`actions/upload-artifact`, `actions/download-artifact` and
`Swatinem/rust-cache` are referenced by major version, the ecosystem's
norm, with `.github/dependabot.yml` proposing bumps monthly. Pinning each
to a commit SHA (with the version in a trailing comment) is the hardening
step for a repository that wants immunity from a retagged action; it costs
a Dependabot-driven bump per release of each action.

**The toolchain comes from `rust-toolchain.toml` alone.** The workflows
run `rustup show active-toolchain || rustup toolchain install`, which
installs the pinned channel and its `rustfmt`/`clippy` components from the
file (the first form on older rustups, the second on current ones), so a
toolchain bump is the one edit it already is.

## Verification

- `scripts/release.sh selftest` drives every rule against fixture trees
  under a temp dir — a manifest whose dependencies carry their own
  `version =` lines and a crate literally named `version`, a changelog
  with two releases, `check` on a consistent tree and then each way of
  breaking it, `package_dist` + `verify` over a real (tiny, C) binary so
  the `file` and `--version` checks run for real, the CPU check firing on
  a mislabelled binary, `notes` with and without assets, `prepare` rolling
  a fixture forward and a first release, `publish --dry-run` with `gh`
  absent from `PATH`, and `install.sh` end to end against the stand-in
  release server (above). It needs bash, awk, sed, tar, gzip, `file`, a C
  compiler and python3, and runs in a few seconds; CI and the release
  workflow both run it.
- `shellcheck -x` over the scripts and `install.sh` (which it checks as
  POSIX `sh`, flagging any bashism), in CI and in the release `check` job.
- The scripts avoid bash 4 (`${var,,}`, associative arrays, `mapfile`),
  `sed -i`, `readlink -f` and gawk-only awk, since a macOS runner may hand
  them bash 3.2, BSD sed and BWK awk.
- The first local rehearsal of this tooling built the x86_64 Linux asset
  in 2 m 15 s (a 22 MB binary, a 9.0 MB archive), `verify` ran the
  packaged binary's `--version`, the arm64 asset cross-compiled under the
  same script and ran under qemu, and `install.sh` installed the x86_64
  archive from the stand-in server, checksum verified, in one line.

## Follow-ups

Not done, and worth doing in this order when wanted: a Homebrew tap
formula pointing at the macOS assets; signing the checksums
(Sigstore `cosign` keyless from the workflow, or `minisign`) so a
downloader can verify provenance and not just integrity; publishing the
crate to crates.io (`cargo publish` from the `publish` job needs a token
secret); a musl Linux asset once its allocator is measured; SHA-pinned
actions.

# The npm packages — `npm/`

Date: 2026-10-05

```bash
npm install -g @linuztx/alter-zero
```

installs the release binary built for the machine, through npm, with Node.js
running nothing but a small launcher — and only until the binary takes over
its process. This is the design: what was decided, what was measured, and
what was tried and refused; then the steps that publish a release to npm.

## The shape

Five packages per release, all at the release's version:

```
@linuztx/alter-zero                  npm/ — the launcher: bin/alter-zero.js, README.md
  optionalDependencies (exact):
    @linuztx/alter-zero-linux-x64     os linux   cpu x64    libc glibc   ← x86_64-unknown-linux-gnu
    @linuztx/alter-zero-linux-arm64   os linux   cpu arm64  libc glibc   ← aarch64-unknown-linux-gnu
    @linuztx/alter-zero-darwin-x64    os darwin  cpu x64                 ← x86_64-apple-darwin
    @linuztx/alter-zero-darwin-arm64  os darwin  cpu arm64               ← aarch64-apple-darwin
```

Each platform package holds one release archive's binary at
`bin/alter-zero` (mode 755), the LICENSE and a README pointing at the
launcher. npm reads the `os`/`cpu`/`libc` of every optional dependency and
installs only the one that matches, so an install downloads the launcher
(a few kilobytes) and one ~10 MB binary package; an optional dependency that
does not match is skipped rather than failed. The launcher's own `os`/`cpu`
make npm refuse an unsupported platform (Windows, a 32-bit CPU) at install
time instead of at first run. The packages are built from the **same
verified archives** the GitHub release carries (`docs/release.md`), so an npm
install, `install.sh` and `alter-zero-bin` all run one binary.

The names are Node's own words for the platform — `process.platform` and
`process.arch` — because those are what the `os`/`cpu` fields are written in
and what the launcher looks the package up by. `scripts/release/lib.sh`'s
`npm_platform` maps each release target to its name, the launcher's
`PLATFORMS` maps the name back, and `npm/package.json` names one optional
dependency per target; the launcher's tests and the release selftest hold
all three together, and the selftest also checks that every target
`release.yml` builds has a package.

## Why platform packages

Three ways to put a native binary behind `npm install -g` were weighed:

- **Platform packages** — this design, which esbuild, Biome and Turborepo
  use. Nothing runs at install time, so it installs exactly like a package
  with no binary in it: under `--ignore-scripts`, under pnpm and bun (which
  skip dependencies' install scripts by default), from a registry mirror or
  through a proxy npm is configured for, and from npm's cache offline. The
  registry's integrity hash covers the binary itself. The cost is five
  packages per release instead of one, which the release workflow publishes.
- **Download on install** — one package whose `postinstall` fetches the
  GitHub archive. Refused: the script does not run under `--ignore-scripts`,
  pnpm 10 or bun's defaults, which leaves an install with no binary at all;
  the download goes around npm's proxy and mirror settings (Node's own
  `https` does not read `HTTPS_PROXY` unless told to); and the binary's
  integrity then rests on a checksum fetched from the same place as the
  binary.
- **All four binaries in one package** — no script, one publish. Refused:
  every install downloads ~38 MB and unpacks ~90 MB to run one of them.

## The launcher

`npm/bin/alter-zero.js` is the `alter-zero` command npm links onto `PATH`.
It finds `{launcher}-{platform}-{arch}` with `require.resolve`, which walks
`node_modules` up from the launcher the way Node resolves any dependency, so
the package is found wherever the package manager put it — nested under the
launcher (npm), hoisted beside it (bun), or in pnpm's store — and then
**becomes** the binary.

**`process.execve`, where Node has it** (22.15+ and 23.11+): the launcher's
process is replaced by the binary, which then owns the terminal, its
signals, its job control and its exit status exactly as if it had been run
directly. It also means no Node process stays resident beside a TUI that may
idle in a terminal all day: an idle Node measured **42 MB** resident here,
against the ~16 MB the app itself idles at (`docs/memory.md`). Under tmux, the
installed `alter-zero`'s pane held one process, `alter-zero`, and nothing
else.

**A failed `execve` cannot be caught.** Node aborts with a native stack trace
(`process.execve failed with error code ENOENT`, exit 134) rather than
throwing, so the launcher's `locate` proves everything an exec could fail on
first, and answers each with a message instead:

| check | what the user is told |
|---|---|
| a platform package exists for this OS and CPU | the supported platforms, and WSL for Windows |
| the platform package resolves | it is an optional dependency: reinstall, without `--omit=optional` |
| …and on musl | the Linux build needs glibc; build from source |
| `bin/alter-zero` is a file | the install is incomplete: reinstall |
| it is executable | the 0755 it was published with is restored if possible, else reinstall |
| on Linux, the ELF loader it names (`PT_INTERP`) exists | glibc is needed — musl, or NixOS without its loader stub |

The loader check reads three small pieces of the binary's ELF header
(~0.1 ms) and catches the one exec failure a present, executable file can
still hit: a glibc binary on a system with no glibc loader, which the kernel
reports as `ENOENT`. The libc itself is read only to explain a failure —
`process.report.getReport()` costs ~6 ms — the way npm reads it to honour a
package's `libc` field. A glibc older than the build's is not pre-checked
(that would mean the report on every launch); the dynamic loader's own
error names the missing symbol version.

**Node's ignored signals are not handed on.** Node ignores `SIGPIPE` and
`SIGXFSZ`, and an ignored signal stays ignored across `exec`: the binary's
`/proc/self/status` read `SigIgn: 0000000001001000` through a bare `execve`,
and every child it spawned would have inherited that. A *caught* signal is
reset to its default by `exec`, so the launcher installs a no-op listener on
both first, and the binary starts with `SigIgn: 0000000000000000`, the same
as a direct run.

**`argv[0]` is `alter-zero`**, not the path inside `node_modules`: the binary
prints its own name back in `--help` and in the `Resume this session with:`
hint. Its *path* is still its own — `current_exe()`, which the tty-detach
helper re-executes (`docs/tty-detach.md`) — because `execve` keeps the
binary's file as the process image.

**On an older Node** the binary runs as a child with the terminal inherited;
the launcher relays `SIGINT`, `SIGTERM` and `SIGHUP` to it and ends the way
it ended — the same exit status, or death by the same signal (re-raised with
its own listener removed first, else the listener would swallow it), or
`128 + n` where Node will not die of the signal (`SIGUSR1` starts Node's
inspector, so it is never re-raised).

## Updating

A package manager owns the files it installed, so `alter-zero update` —
which reinstalls over the running binary's own directory — must not touch an
npm install: it would replace a file under `node_modules` behind npm's back,
and the next `npm install -g` would put npm's copy back. The binary knows it
was installed by a package manager from **its own path**, `current_exe()`:
every manager unpacks the platform package's binary under a `node_modules`
directory, and the layout around it names the manager (`update::package_manager`):

| path holds | manager | update command |
|---|---|---|
| `node_modules/.pnpm/` | pnpm | `pnpm add -g @linuztx/alter-zero@latest` |
| `.bun/…/node_modules/` | Bun | `bun add -g @linuztx/alter-zero@latest` |
| `yarn/global/node_modules/` | Yarn (classic) | `yarn global add @linuztx/alter-zero@latest` |
| any other `node_modules/` | npm | `npm install -g @linuztx/alter-zero@latest` |

For such an install the daily update card (`docs/update.md`) names that
command instead of `alter-zero update`, and `alter-zero update` refuses with
it, the way it refuses a `cargo` build directory. The path is the signal
rather than an environment variable the launcher sets (codex's approach):
it holds however the binary was started, and it leaks nothing into the
environment of every command the agent runs.

## The release tooling

`npm/package.json` is a version written in a fifth place, so it is kept in
step the way the other four are (`docs/release.md`): `scripts/release.sh
check` requires its version and each platform package's pin to equal
Cargo.toml's (rule 7, which also refuses a pin for a platform no release
target builds), and `prepare` rewrites the version and the four pins in
place.

```bash
scripts/release.sh npm X.Y.Z dist --dry-run   # verify, stage into target/npm, `npm publish --dry-run` each
scripts/release.sh npm X.Y.Z dist             # …and publish: four platform packages, then the launcher
```

The command first runs `verify` over the archives — which accepts a release
**as built** (each archive beside its `.sha256`) or **as published** (the
archives and `SHA256SUMS`, what `gh release download` hands back), so a
version already on GitHub can be published to npm by hand. It then stages
the five packages under `target/npm/` (`--out` elsewhere): each platform
package from its archive, and the launcher as `npm/` with the LICENSE added
and its `scripts` removed. Publishing is ordered — **the launcher last** — so
its optional dependencies are on the registry before anything can install
it, and a real publish refuses a dist missing any platform, which would be
a missing binary for every install on it (a dry run warns and stages the
rest). A version already on the registry is left as it is — npm versions are
immutable — so a run that died halfway is simply run again. A pre-release
goes to the `next` dist-tag, keeping `npm install -g` on the latest stable.

`npm/package.json`'s own `prepublishOnly` refuses any publish made from
`npm/` itself, pointing at the command above: published from there, the
launcher could reach the registry ahead of its platform packages and install
with no binary at all. The staged copy carries no scripts, so the published
package runs nothing anywhere.

Registry and credentials are npm's own configuration — `npm login` on a
laptop, `NODE_AUTH_TOKEN` or trusted publishing in the workflow — and a dry
run touches neither the network nor the registry.

## Publishing

### The first publish, by hand

npm's trusted publishing is configured per package on npmjs.com, which
needs the package to exist, so the first publish is made with an account —
the `linuztx` account owns the `@linuztx` scope:

```bash
npm login
gh release download v0.11.0 -D dist           # the release's four archives and SHA256SUMS
scripts/release.sh npm 0.11.0 dist --dry-run  # look first
scripts/release.sh npm 0.11.0 dist
```

With two-factor authentication on the account, npm asks for a one-time
password (or a browser confirmation) as it publishes each package. Run it
from a checkout whose `npm/package.json` is at that version. A
release cut before this package existed carries a binary that does not know
it can be installed by npm — its update card says `alter-zero update`, which
on such an install reinstalls the binary under `node_modules` — so the first
release cut after it is the better first npm version.

### Every release after it

In the repository's **Settings → Secrets and variables → Actions**, add the
secret `NPM_TOKEN` (an npm granular access token with read and write access
to the five packages, allowed to publish without a one-time password) and
the variable `NPM_PUBLISH` set to `true`. A
`vX.Y.Z` tag push then publishes to npm right after the GitHub release, from
the same verified archives (`release.yml`'s `npm` job, with npm's provenance
statement attached). Without the variable the job only rehearses — it
stages and dry-runs the packages, as it does on every pull request that
touches `npm/` or the release scripts — so a release is never held up by
npm.

npm limits how long a token that can publish stays valid, so the lasting
setup is **trusted publishing**: on npmjs.com, open each of the five
packages' settings, add a trusted publisher (GitHub Actions, repository
`linuztx/alter-zero`, workflow `release.yml`), then delete the `NPM_TOKEN`
secret. The job already holds `id-token: write`, and the npm 11 that Node 24
bundles exchanges it for a short-lived publish token on its own.

If the job fails after the GitHub release went out, re-run it: whatever was
published is skipped.

## Verification

- `node --test` in `npm/` (CI's release-tooling job): the platform table
  against `package.json`, the ELF loader read over real and synthetic
  headers, every `locate` verdict, and the launcher run for real over a
  fake install — arguments passed through verbatim, exit statuses, `argv[0]`,
  the binary taking over the launcher's own PID, `SigIgn` empty, and the
  spawn fallback's relaying and mirroring, including death by signal.
- `scripts/release.sh selftest`: the readers, rule 7, `prepare`'s bump,
  `verify` on a release as published, and a four-target fixture release
  (the host's binary real, the others headers `file(1)` accepts) staged,
  dry-run, published to `scripts/release/npm_registry.py` — a stand-in
  registry that serves packuments and tarballs and takes `npm publish`'s
  PUT — in order, re-run to prove it skips what is there, and installed back
  with `npm install -g`, whose `alter-zero --version` is the host's binary.
- The v0.11.0 release itself: its four archives, downloaded from GitHub,
  verified as published, staged (9.5–10.1 MB packages, 20.5–24.1 MB
  unpacked), published to the stand-in and installed with `npm install -g`,
  which added two packages — the launcher and `linux-x64`. The installed
  `alter-zero` answered `--version`, `--help` and a usage error's exit 2;
  under tmux it opened the TUI with no Node process beside it, ran a `!`
  command through the tty-detach helper and a demo turn, and quit printing
  `alter-zero --resume …`.

## Known gaps

- **musl and older glibc.** There is no musl build (`docs/release.md`
  explains why), so Alpine gets the launcher's explanation; a glibc older
  than 2.35 installs and then fails in the dynamic loader. Building from
  source covers both.
- **Windows** is refused at install time, as by `install.sh`; WSL works.

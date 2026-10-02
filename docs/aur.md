# The Arch Linux packages — `aur/`

`aur/` holds the two recipes the [Arch User Repository](https://aur.archlinux.org/)
installs Alter Zero from, one directory per package because each is its own
AUR git repository:

- **`alter-zero`** builds the tagged release from source: `makepkg` on the
  user's own machine downloads the tag's source, builds it with Arch's Rust,
  runs the test suite and packages the one binary — about fifteen minutes on
  four cores.
- **`alter-zero-bin`** repackages the release's own prebuilt Linux archive,
  checksum-pinned: seconds, and nothing to compile.

This is the design — what was decided, what was measured, and what was tried
and refused — and the steps that publish both and keep them current.

```
aur/alter-zero/PKGBUILD     the source recipe
aur/alter-zero/.SRCINFO     its metadata as the AUR reads it — generated, never edited
aur/alter-zero/.gitignore   an allowlist: makepkg's src/, pkg/, downloads and packages stay out
aur/alter-zero-bin/         the same three files for the prebuilt package
```

Each directory is its AUR repository's contents exactly, `.gitignore`
included, so publishing is copying three files.

## `alter-zero`: built from the tag's source

The AUR's naming rule decides the shape: a package named plainly
(`alter-zero`) builds a stable release from source, `-bin` repackages a
prebuilt binary and `-git` builds a branch. This is the plain one. Its source
is the tag's own tarball, `{url}/archive/refs/tags/vX.Y.Z.tar.gz`, pinned by
SHA-256 — GitHub generates it from the tag, so the checksum exists only once
the tag is pushed, which is why the package follows a release rather than
being bumped by `scripts/release.sh prepare` (`docs/release.md`).

It builds with **Arch's** `rust`, not the `1.94.1` that `rust-toolchain.toml`
pins: `makedepends=('cargo')` is satisfied by either `rust` or `rustup`, and
`RUSTUP_TOOLCHAIN=stable` makes a rustup user build with stable instead of
having rustup download the pinned toolchain in the middle of a package build.
Everything else in the PKGBUILD is a difference from the release build
(`scripts/release.sh build`): Arch's newer compiler, makepkg's defaults, and
Arch's conventions for what a package links.

## Where it differs from the release build

### Warnings stay warnings

`Cargo.toml` denies every rustc warning (`[lints.rust] warnings = "deny"`),
which is safe only on the pinned toolchain — its own comment says a new stable
shipping a new lint would break every build. An AUR build *is* every build on
whatever rustc Arch ships that week, and Arch moves within days of a stable
release (1.98.1 at the time of writing, with 1.99 already branched). So
`build()` and `check()` append `--cap-lints=warn` to `RUSTFLAGS`: a lint added
upstream of us prints as the warning it is and the user still gets a package.

Measured against what could go wrong with that:

- Cargo puts its own `--cap-lints allow` for dependencies **before**
  `RUSTFLAGS`, and rustc takes the last one given, so the flag does not fail
  on being repeated; it does let a dependency's warnings print, and there
  are none — on 1.98.1 all 260 crates of the build compile warning-free, and
  the 1.99.0 pre-release (2026-09-28) checks every target, tests included,
  without one. Today the cap changes nothing; it is there for the release
  that adds a lint.
- Doc-tests are compiled by rustdoc, which `RUSTFLAGS` does not reach, but
  they do not inherit the crate's `--deny=warnings` either: a doc-test written
  to trip `non_snake_case` passed with the cap and without it. The crate has
  no doc-tests today regardless.

`clippy::all = "deny"` needs nothing: clippy does not run in a package build.

### LTO is off

Arch's stock `makepkg.conf` turns on `lto`, which appends `-flto=auto` to
`CFLAGS`, `CXXFLAGS` and `LDFLAGS` — never to `RUSTFLAGS`. In this crate it
therefore reaches exactly one thing: `ring`'s C sources, which the `cc` crate
compiles with the ambient `CFLAGS`. GCC then emits them as GIMPLE bytecode
only, and since Rust 1.90 an `x86_64-unknown-linux-gnu` binary is linked by
`rust-lld`, which cannot read GCC bytecode. The first build with makepkg's
defaults compiled for five minutes and failed at the last link:

```
ld.lld: error: undefined symbol: ring_core_0_17_14__LIMBS_window5_split_window
ld.lld: error: undefined symbol: ring_core_0_17_14__CRYPTO_poly1305_init
ld.lld: error: undefined symbol: ring_core_0_17_14__aes_nohw_set_encrypt_key
…
```

`options=('!lto')` keeps the flags out and `ring` links. Arch's own `uv`
takes the other route, `CFLAGS+=' -ffat-lto-objects'`, which links too — the
objects then carry machine code beside the bytecode — but every object is
then generated both ways for an LTO pass `rust-lld` never runs, so it buys
nothing here. `ring` is the only crate in the build that compiles C (every
build script's output was checked), so nothing else is affected either way.

### The system oniguruma

`syntect`'s regex engine is oniguruma through `onig_sys`, which compiles a
bundled copy by default. `RUSTONIG_SYSTEM_LIBONIG=1` links Arch's
`oniguruma` instead, the Arch convention for a library the distribution
ships (`bat` and `git-delta` do the same). The bundled copy in
`onig_sys 69.9.3` is oniguruma **6.9.10**, the version Arch and Arch Linux
ARM ship, so the regex behaviour — and the 8× memory win over `fancy-regex`
that chose oniguruma in the first place (`Cargo.toml`) — is unchanged.
`SYSTEM` rather than the `RUSTONIG_DYNAMIC_LIBONIG` those two set, because
the dynamic variable falls back to compiling the bundled copy when pkg-config cannot
find the library, while the system one fails the build saying so. The build
script's output confirms the link: `cargo:rustc-link-lib=onig` against
`/usr/lib`, no C compiled.

### Debug info reaches the debug package

With makepkg's default `debug` option, `RUSTFLAGS` carries
`-C debuginfo=2` and makepkg splits whatever debug info the binary has into
`alter-zero-debug`. But Cargo strips debug info from a release build whose
profile has none (its default since 1.77), so the first package's
`alter-zero-debug` held a symbol table and no DWARF — `gdb-add-index` said
*No debugging symbols*. `CARGO_PROFILE_RELEASE_STRIP=false` leaves that to
makepkg, which strips the shipped binary either way: with `debug` the
DWARF moves into the debug package, and with `!debug` the crates are
compiled without any, as before. (`ripgrep` and `git-delta` set
`CARGO_PROFILE_RELEASE_DEBUG=2` instead, which compiles debug info even
for a `!debug` build, only for makepkg to strip it.)

### Incremental is off

`[profile.release]` sets `incremental = true` for a developer's rebuild after
an edit (`docs/build-time.md`). A package is built once, so
`CARGO_INCREMENTAL=0` turns it off — what the release workflow's gate job
does with the same variable.

## Dependencies

| | | why |
|---|---|---|
| `depends` | `glibc` `libgcc` `oniguruma` | the binary's whole `NEEDED` list: `libc`, `libm`, `ld-linux`, `libgcc_s`, `libonig` |
| `makedepends` | `cargo` | `rust` or `rustup` |
| `checkdepends` | `git` `python` | the checkpoint, PTY and relay tests skip without `git`/`python3`; listing them makes `check()` run them |
| `optdepends` | `git` | checkpoints (off until a directory turns them on) and the `/diff` review |

`libgcc`, not `gcc-libs`: Arch split GCC's runtime, and `gcc-libs` is now a
metapackage pulling a dozen libraries the binary never loads — namcap says
so. Nothing graphical or cryptographic is a runtime dependency because none
is linked: TLS is `rustls` over `ring`, compiled in, and the clipboard is
`x11rb` and `wl-clipboard-rs`, which speak X11 and Wayland in Rust. The
binary names no shared library beyond its `NEEDED` entries, so nothing is
`dlopen`ed either. `bash`, `setsid` and `script` come with `base`.

`arch=('x86_64' 'aarch64')` names the two Linux targets upstream ships and
tests. Arch Linux ARM carries the same `libgcc` split, `oniguruma 6.9.10` and
`rust 1.98.1`.

## `check()`

`cargo test --frozen` in the dev profile — what upstream CI's gate runs, and
what Arch's `ripgrep`, `fd` and `git-delta` do — with three decisions on top.

**Not `--release`.** Testing in the release profile looks as if it would
reuse `build()`'s dependencies. Measured, it does not: the test build adds
the `tiktoken-rs` dev-dependency, which changes feature resolution, and Cargo
recompiled every dependency anyway (9m 39s, against 10m 23s for the dev
profile). It also re-links `target/release/alter-zero` after `build()` made
it. The bytes came out identical, but the package should ship `build()`'s
binary by construction rather than by luck, and the dev profile builds into
`target/debug`, never touching it.

**No debug info.** With makepkg's `debug` option, `RUSTFLAGS` carries
`-C debuginfo=2` into the test build too, on top of the dev profile's own
full debug info: the test build measured **11 GB**. `check()` appends
`-C debuginfo=0`, which rustc honours as the last `-C debuginfo` it is
given, and the same build measured **1.2 GB** — and compiled in 7m 28s
rather than 10m 23s. Nothing in the suite reads a backtrace.

**Five frame-budget gates are skipped.** A few tests hold rendering to a
wall-clock budget — `the_strip_redraw_stays_inside_a_frame_on_a_huge_prose_line`
wants an average redraw under 25 ms on a 130 KB line, the inline-diff and
highlighter gates 32 and 50 ms. They are performance regression gates, and on
a user's machine what they measure is the machine: in the dev profile on four
cores, under makepkg's flags, that redraw averaged 33.9 ms and failed
`check()`, while the release-profile run passed it. Upstream CI enforces
them; the package skips them by name. The complexity gates beside them — a
130 KB reply must stream in 10–20 s, which only a quadratic regression
misses — still run, as do the responsiveness tests (a cancel lands within
seconds). A renamed gate simply runs again, so the list is worth a glance
when bumping the package.

The suite is offline by construction: every test that reaches a provider is
`#[ignore]`d. On the verification build it ran 23 suites — 4,782 tests
passed, none failed, 111 ignored (the live-provider and X11 clipboard tests),
5 skipped — the unit tests themselves in 95 s.

## What it installs

| path | |
|---|---|
| `/usr/bin/alter-zero` | stripped by makepkg; with `debug`, the symbols and DWARF are in `alter-zero-debug` |
| `/usr/share/licenses/alter-zero/LICENSE` | installed because it names the copyright holder — Arch ships the generic Apache-2.0 text, not this one |
| `/usr/share/doc/alter-zero/` | `README.md`, `CHANGELOG.md`, and `TELEMETRY.md`, the statement the first-run notice points at |

The package is 8.5 MB, 17.8 MiB installed. `alter-zero-debug` is 80 MB: a
395 MB DWARF file and the crate's own sources under `/usr/src/debug`. With
that debug info the release build directory is 1.9 GB, and the test build
beside it 1.2 GB.

## `alter-zero-bin`: the release archive, repackaged

For the user who would rather not compile. The package downloads the
archive the release workflow built for their CPU —
`alter-zero-vX.Y.Z-{x86_64,aarch64}-unknown-linux-gnu.tar.gz` from the
GitHub release (`docs/release.md`) — through `source_x86_64` and
`source_aarch64`, each pinned by the SHA-256 the release's own `SHA256SUMS`
lists, and installs what the archive holds: the binary, `LICENSE`,
`README.md` and `CHANGELOG.md`. It is the binary `install.sh` installs, so
nothing about it differs from the one-line install but who owns the file.
`provides=('alter-zero=X.Y.Z')` and `conflicts=('alter-zero')` make the two
packages interchangeable: either one satisfies anything that depends on
`alter-zero`, and pacman never installs both.

Three things differ from the source package, all because the binary is the
release's rather than one Arch built:

- **`depends=('glibc' 'libgcc')`, no `oniguruma`.** The release build
  compiles `onig_sys`'s bundled copy in, so the binary's `NEEDED` list is
  `libc`, `libm`, `libgcc_s` and the loader. Its newest glibc symbol is
  `GLIBC_2.34` (it is built on Ubuntu 22.04), far below any Arch install.
- **`options=('!strip' '!debug')`.** The release binary keeps its symbol
  table deliberately, so a user's `RUST_BACKTRACE=1` report names functions
  (`docs/release.md`, *What a release carries*); makepkg's default strip
  would throw that away, and there is no debug info to split into a debug
  package. namcap's `ELF file … is unstripped` warning is that decision.
- **No `build()` or `check()`.** Nothing is compiled; the release workflow
  ran the gate and `verify` on the archive before publishing it.

namcap also flags `Reference to x86_64 should be changed to $CARCH` on the
PKGBUILD: it is the URL in `source_x86_64`, which has to name its own
architecture — `$CARCH` there would resolve to the host's when `.SRCINFO`
is generated and give the other architecture the wrong file.

## Publishing

Once, from an [AUR account](https://aur.archlinux.org/register) with an SSH
key added, for each package:

```bash
for p in alter-zero alter-zero-bin; do
  git clone ssh://aur@aur.archlinux.org/$p.git ~/aur/$p   # an empty repository the first time
  cp aur/$p/PKGBUILD aur/$p/.SRCINFO aur/$p/.gitignore ~/aur/$p/
  (cd ~/aur/$p && git add PKGBUILD .SRCINFO .gitignore &&
    git commit -m "Initial import: $p 0.10.0" &&
    git push origin HEAD:master)                          # the AUR accepts master only
done
```

The push creates the package page; `.SRCINFO` is what the AUR reads to fill
it, which is why the server rejects a push whose `.SRCINFO` is missing.

## Updating for a release

After `vX.Y.Z` is published on GitHub — the publish job uploads every
archive before the release becomes visible, so both packages can move at
once — on an Arch machine:

```bash
for p in alter-zero alter-zero-bin; do (
  cd aur/$p
  sed -i 's/^pkgver=.*/pkgver=X.Y.Z/; s/^pkgrel=.*/pkgrel=1/' PKGBUILD
  updpkgsums                          # pacman-contrib: the tag tarball, or both architectures' archives
  makepkg -Cf                         # a clean build (+ check() for the source package)
  namcap PKGBUILD $p-X.Y.Z-1-*.pkg.tar.zst
  makepkg --printsrcinfo > .SRCINFO
) done
```

`updpkgsums` refills both of `alter-zero-bin`'s arrays, not just the host's:
from zeroed checksums it reproduced the two `SHA256SUMS` values exactly.
Commit each directory's `PKGBUILD` and `.SRCINFO` here, then copy them into
its AUR clone and push as above with `Update to X.Y.Z`. A change to the recipe alone — a
dependency, a flag — bumps `pkgrel` instead of `pkgver`. `extra-x86_64-build`
(from `devtools`) builds in a clean chroot, the strictest stand-in for a fresh
machine; `.SRCINFO` must be regenerated after **every** PKGBUILD edit, since
the AUR never reads the PKGBUILD itself.

## Known gap: `alter-zero update`

The daily update check (`docs/update.md`) sees a new release on GitHub before
the AUR packages are bumped, and its card says *run `alter-zero update`*. With
either package installed, that
command reinstalls over the running binary's own directory — `/usr/bin` here.
As a user it stops at `/usr/bin is not writable`; under `sudo` it would
replace a file pacman owns, which `pacman -Qkk` reports as modified until the
next package upgrade puts pacman's copy back. An AUR install updates through
its AUR helper (`paru -Syu`, `yay -Syu`) or `makepkg -si`, and
`/settings → Update check` or `ALTER_ZERO_UPDATE_CHECK=0` silences the card.

The fix belongs upstream rather than in a source patch here: a build-time
variable read with `option_env!` — say `ALTER_ZERO_UPDATE_HINT` — that
replaces the card's **Update** line with the package manager's command and
makes `alter-zero update` refuse with the same text, which `build()` would
then export. `alter-zero-bin` installs the release binary, so it would need
the hint as a runtime signal instead — a marker file the package installs
beside the binary, say. v0.10.0 predates both.

## Verification

Built in a chroot made from Arch's official bootstrap image (`base-devel`,
`rust 1:1.98.1`, `gcc 16.2.1`, `pacman 7.1.0`), as an unprivileged user,
under the stock `makepkg.conf` — `lto`, `debug` and `check` all on — so the
build is the one an AUR helper on a stock install runs.

- `makepkg -C` from an empty `src/`: 15 minutes on four cores — `build()`
  5m 27s, the test build 7m 28s, the tests themselves under two minutes.
- `namcap` on the PKGBUILD reports nothing. On the package it reports one
  warning, `Unused shared library … ld-linux-x86-64.so.2`, which is the Rust
  toolchain's: a `cargo new` hello-world built in the same chroot carries the
  same `NEEDED` entry. On `alter-zero-debug` it reports the build-id symlink
  pointing at `/usr/bin/alter-zero`, which lives in the main package, as
  every split debug package's does.
- `pacman -U` installed it, `pacman -Qkk` found no altered file, `ldd`
  resolved `libonig.so.5` to Arch's, and the installed binary answered
  `--version`, `--help` and `mcp list`. Under tmux it opened the TUI and
  streamed the offline demo's markdown tour, its Python and Rust blocks drawn
  in five and six colours by the system oniguruma, and quit on Ctrl+C.
- `makepkg --printsrcinfo` generated `.SRCINFO`.
- Not run: an aarch64 source build. Its dependencies were checked against
  Arch Linux ARM's repositories, and upstream cross-builds that target on
  every release.

`alter-zero-bin`, in the same chroot:

- Both archives matched the release's `SHA256SUMS` before they were pinned.
- `makepkg -C` built the x86_64 package in 16 seconds, download included,
  and a configuration with `CARCH=aarch64` built the aarch64 one — its
  download, checksum and `package()` all ran, and the packaged binary is an
  `ARM aarch64` ELF.
- `namcap` reports only the warnings explained above: `unstripped`, the
  `source_x86_64` URL, and the toolchain's `ld-linux` entry (on the aarch64
  package checked from an x86_64 host, the loader is reported as not
  installed).
- `pacman -U` replaced the installed `alter-zero` with it; `pacman -Qi`
  shows `Provides: alter-zero=0.10.0` and `Conflicts With: alter-zero`,
  `pacman -T alter-zero` is satisfied, `pacman -Qkk` found no altered file,
  and the binary answered `--version` and ran the same TUI tour.

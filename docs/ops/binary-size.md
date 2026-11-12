# Binary size — the binary a release ships

Every way of installing Alter Zero downloads the same file: `install.sh`, the
npm platform packages, `alter-zero-bin` and the Docker image all unpack the
release archive `scripts/release.sh build` makes (`docs/ops/release.md`). Its size
is a download for every install and a disk cost for every machine, so it was
measured before anything changed, the way `docs/ops/build-time.md` measures
compile time and `docs/architecture/memory.md` measures RSS: what the bytes are, which
knobs move them, and what each knob costs at runtime. This page is that
measurement, what it changed, and what it refused.

## The measurement

Everything here ran on one x86_64 Linux machine (4 cores, 15 GB, rustc
1.94.1, the pinned toolchain, linking with its bundled `rust-lld`). The
baseline is what `scripts/release.sh build` used to build:
`cargo build --release --locked --config profile.release.incremental=false`.

- **The file and its download.** The binary's bytes, `gzip -n -9` of it (what
  `package_dist` compresses an asset with) and, for the before and after, the
  release archive itself, packaged by `package_dist` with its `LICENSE`,
  `README.md` and `CHANGELOG.md`.
- **Where the bytes are.** `readelf -S`, grouped: code (`.text`), read-only
  data (`.rodata`), unwind tables (`.eh_frame`, `.eh_frame_hdr`,
  `.gcc_except_table`), relocated data (`.data.rel.ro` and the `.rela.dyn`
  relocations a position-independent executable needs to fix it up at load)
  and the symbol table (`.symtab`, `.strtab`).
- **Whose code it is.** `cargo bloat --release --crates`, which attributes by
  symbol name, so code inlined into another crate counts toward that crate.
- **What a knob costs at runtime.** `perf_probe`'s whole-render column
  (`ui::message_lines` over a 50- to 1600-line code reply, five runs averaged
  per size), the proxy `docs/ops/build-time.md` judged `lto = "off"` by. Each
  build's probe ran in rotation with the others on an otherwise idle machine,
  seven rounds, and the tables give the median per size.

Every knob was tried as a profile defined on the command line, so Cargo.toml
did not change until the numbers were in:

```bash
cargo build --locked --profile x --bin alter-zero \
  --config 'profile.x.inherits="release"' --config 'profile.x.incremental=false' \
  --config 'profile.x.lto="fat"' --config 'profile.x.codegen-units=1'
```

## Where the bytes were

The baseline binary, 25,560,432 bytes:

| | bytes | share |
| --- | --- | --- |
| code | 14,081,248 | 55.1% |
| read-only data | 2,828,956 | 11.1% |
| unwind tables | 2,433,028 | 9.5% |
| relocated data | 1,324,944 | 5.2% |
| symbol table | 4,811,923 | 18.8% |
| everything else | 80,333 | 0.3% |
| `gzip -9` | 10,012,965 | |
| the release archive | 10,081,762 | |

The code by crate (`cargo bloat`, of 13.4 MiB of `.text`):

| crate | size | share |
| --- | --- | --- |
| `alter_zero` | 6.4 MiB | 47.4% |
| `std` | 1.7 MiB | 12.5% |
| `reqwest` | 465 KiB | 3.4% |
| `rustls` | 440 KiB | 3.2% |
| `image_webp` | 386 KiB | 2.8% |
| `image` | 347 KiB | 2.5% |
| `regex_automata` | 317 KiB | 2.3% |
| `ring` | 286 KiB | 2.1% |
| `arboard` | 241 KiB | 1.8% |
| `regex_syntax` | 217 KiB | 1.6% |
| `aho_corasick` | 189 KiB | 1.4% |
| `wl_clipboard_rs` | 144 KiB | 1.0% |
| `zune_jpeg` | 135 KiB | 1.0% |
| `tokio` | 132 KiB | 1.0% |
| `toml_edit` | 131 KiB | 1.0% |

Three things fall out of it.

**Half the code is the app's own.** No single dependency is the problem: the
largest, `reqwest` and `rustls`, are about 3% of the code each. So the lever
that reaches the most bytes is how the whole program is optimised, not which
features a dependency is built with — and the trims `docs/ops/build-time.md`
already made (`syntect`'s file loaders, `ratatui`'s calendar, `toml`'s
serializer) had taken the obvious unused features out.

**A fifth of the file is symbol names, and much of that is how the program
was cut into codegen units.** 26,147 function symbols with a median name of
91 bytes; 3,566 of them carry a `.llvm.<hash>` suffix, which thin LTO adds
when it promotes a function so another unit can call it; 7,342 anonymous
constants got a symbol of their own the same way
(`anon.<hash>.<n>.llvm.<hash>`); and 15,839 `GCC_except_table` labels mark
the unwind data of the functions they follow.

**The hook matchers carried a whole regex engine's fast paths.**
`regex-automata`, `regex-syntax` and `aho-corasick` come to 723 KiB of code,
for a matcher that is tried against a tool name (`src/hooks/matcher.rs`, the
crate's only regex), and much of it is machinery for searching long texts
quickly.

What did not need looking at: the files embedded with `include_str!` (the
prompts, the built-in skills, `providers.toml`) are 146 KB together, and of
`two-face`'s two 0.98 MB syntax dumps only the newline-mode one the
highlighter loads is linked — nothing calls `extra_no_newlines`, so the
linker already drops its dump.

## What changed

### `[profile.dist]`: the release build gets fat LTO over one codegen unit

```toml
[profile.dist]
inherits = "release"
incremental = false
lto = "fat"
codegen-units = 1
```

`release` optimises each crate on its own, split into sixteen codegen units
joined by thin LTO. Fat LTO over a single unit hands LLVM the whole program —
every crate, `std` included — as one module, so code no caller reaches goes
whatever crate it is in, a generic used by many units is no longer copied
into each, and inlining crosses every crate boundary. The profiles tried:

| profile | file | `gzip -9` | code | unwind | relocated | symbols | cold build | peak memory |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `release` (the baseline) | 25,560,432 | 10,012,965 | 14,081,248 | 2,433,028 | 1,324,944 | 4,811,923 | 2 min 17 s | 1.7 GB |
| thin LTO across crates | 25,195,792 | 10,026,631 | 14,264,384 | 2,233,224 | 1,273,864 | 4,523,717 | 2 min 9 s | 1.7 GB |
| fat LTO, 16 units | 19,978,968 | 8,932,170 | 12,720,960 | 1,519,312 | 878,792 | 2,096,937 | 3 min 41 s | 2.0 GB |
| **fat LTO, 1 unit** | **19,167,696** | **8,750,011** | **12,338,112** | **1,367,172** | **859,632** | **1,851,041** | 4 min 26 s | 1.9 GB |

(A cold build is every dependency and the crate from an empty target
directory; peak memory is the largest single process, from `getrusage`. The
baseline's two were taken again on the final manifest, since its first build
shared the machine with another for part of the run; `dist` itself built in
the same 4 min 26 s either way.)

Every section shrinks, not only the code. Fewer, larger functions mean fewer
unwind entries (−44%), and the symbol table falls by 62%: the thin-LTO
suffixes and the promoted constants are gone, and inlining left half as many
functions to name — 26,147 down to 12,857, every one that remains still
named. Thin LTO
across crates, the other common setting, moved almost nothing, and a single
codegen unit is worth its 45 seconds over sixteen: 0.81 MB.

And it is faster, not slower, which is the same finding `docs/ops/build-time.md`
made from the other side: switching `release`'s thin-local LTO off cost the
render path 7–19%, because the cross-unit inlining went with it, and
whole-program LTO is more of that inlining:

| code lines | `release` | `dist` |
| --- | --- | --- |
| 50 | 19.1 ms | 18.2 ms (−4.5%) |
| 100 | 35.3 ms | 34.0 ms (−3.8%) |
| 200 | 74.8 ms | 65.1 ms (−12.9%) |
| 400 | 146.0 ms | 135.9 ms (−6.9%) |
| 800 | 305.6 ms | 270.6 ms (−11.5%) |
| 1200 | 447.1 ms | 408.6 ms (−8.6%) |
| 1600 | 580.9 ms | 543.3 ms (−6.5%) |

(Measured before the `regex` trim below, which reaches nothing on the render
path.)

**Why it is a profile of its own.** Fat LTO re-optimises the whole program on
every link, so it cannot be incremental: a one-function change to
`src/update.rs` took 3 min 41 s to rebuild in `dist`. `release` exists to
rebuild after an edit in ~10 s (`docs/ops/build-time.md`), and every
`cargo run --release --example …` probe builds in it; with fat LTO there,
each would pay those minutes. A release is built once per target, so the
extra two minutes fall on the release workflow, not on a developer. So
`release` stays the developer's profile, `dist` is the shipped one, and
`scripts/release.sh build` is `cargo build --profile dist --locked --target T`. The binary lands in
`target/{T}/dist/` (`target/dist/` without `--target`), which
`update::is_cargo_build_dir` also knows for a build directory, so
`alter-zero update` refuses to overwrite one (`docs/ops/update.md`).

The cross-compiles need nothing new: rustc runs the LTO itself, before the
link, so `aarch64-linux-gnu-gcc` and Apple's linker are handed ordinary
object files, and the C code (`ring`'s, oniguruma's) arrives as the same
static archives as before. The macOS sizes were not measured here; the
profile is the same on every target.

### `regex` without `perf`

```toml
regex = { version = "1", default-features = false, features = ["std", "unicode"] }
```

`perf` is six features — the lazy DFA, the one-pass DFA, the bounded
backtracker, the literal prefilters (`aho-corasick` and `memchr`), the cache
and the inlining — and every one of them only makes a search faster. The
PikeVM, which every build keeps, finds the same matches: that is the `regex`
crate's own guarantee for its features, and a matcher is tried against a tool
name a few bytes long, where the PikeVM answers in microseconds. `unicode`
stays, so `\w`, `\d` and `(?i)` mean exactly what they meant. Nothing else in
the dependency graph turned `perf` on, so the trim is real: on the `dist`
build, 19,167,696 → **18,599,080 bytes** (−0.57 MB, 403 KB of it code), the
`gzip -9` 8,750,011 → 8,532,140. `hooks::matcher`'s tests are the check that
the matchers still select what they selected.

### Where that leaves the release

| | before | after | |
| --- | --- | --- | --- |
| the binary | 25,560,432 | 18,599,080 | −27.2% |
| code | 14,081,248 | 11,934,720 | −15.2% |
| read-only data | 2,828,956 | 2,698,588 | −4.6% |
| unwind tables | 2,433,028 | 1,322,748 | −45.6% |
| relocated data | 1,324,944 | 831,832 | −37.2% |
| symbol table | 4,811,923 | 1,769,014 | −63.2% |
| the release archive | 10,081,762 | 8,601,400 | −14.7% |
| cold build, 4 cores | 2 min 17 s | 4 min 26 s | +2 min 9 s |

The archive shrinks less than the binary because what went first is what
compressed best: symbol names and unwind tables are repetitive text, the code
that remains is not.

## Measured and refused

### Stripping the symbol table: −1.77 MB, −0.44 MB of the archive

The release keeps its symbol table on purpose: it is what turns a user's
`RUST_BACKTRACE=1` report into function names (`docs/ops/release.md`), and
`alter-zero-bin` keeps it for the same reason (`docs/ops/aur.md`). Stripping was
the cheapest saving before this change — 4.8 MB, a fifth of the file — and
fat LTO took most of it without losing a name: what is left is 1.77 MB of the
binary and 442 KB of the download (16,829,912 bytes stripped, 8,089,697
gzipped). Names in a backtrace are worth that. `--config
profile.dist.strip=true` on the build line is the knob if it ever matters
more. The symbol table is never mapped into memory, so it costs disk and
download only, never RSS.

### `panic = "abort"`: −2.4 MB, and a crash where there was an error

Measured on top of fat LTO it is the next largest saving: 16,792,288 bytes
(−2.38 MB, its `gzip -9` −0.98 MB), mostly landing pads and the
`.gcc_except_table` that describes them. But the app catches panics on
purpose. The hook-notification lane runs each dispatch under `catch_unwind`
so one panicking handler cannot strand every notification after it
(`src/llm/hooks.rs`); the backend's forwarder thread, the held `SessionStart`
run and an MCP server's sign-in probe are joined with `unwrap_or_default` and
`unwrap_or(false)`, a panic in them costing that one result; and the Git diff
loader turns a reader thread that died into a message on the `/diff` page.
Each turns a bug in a worker into a degraded feature. Under `abort` every one
of them ends the process and the session with it (the terminal would still be
restored — the panic hook runs before the abort — but the turn in flight is
gone). A smaller file is not worth a crash where there was an error row.

### `opt-level = "s"`: −3.7 MB more, and a slower render path

With fat LTO and one unit, optimising for size took the binary to
15,455,168 bytes (the `gzip -9` to 7,154,710) — the code alone 12.3 →
8.6 MB. It is also the `lto = "off"` trade again, against `release` and
worse against `dist`:

| code lines | `release` | `opt-level = "s"` |
| --- | --- | --- |
| 50 | 19.1 ms | 21.3 ms (+11.4%) |
| 100 | 35.3 ms | 38.3 ms (+8.4%) |
| 200 | 74.8 ms | 79.4 ms (+6.3%) |
| 400 | 146.0 ms | 158.4 ms (+8.5%) |
| 800 | 305.6 ms | 320.3 ms (+4.8%) |
| 1200 | 447.1 ms | 490.2 ms (+9.7%) |
| 1600 | 580.9 ms | 661.0 ms (+13.8%) |

For a TUI whose promise is to be light and fast the render path is the wrong
side of that trade, as it was for the compile time `lto = "off"` would have
saved. `"z"` goes further the same way. A per-crate `opt-level = "s"` for the
dependencies nowhere near the render path (the clipboard and Wayland stacks,
`toml`, `url`) is the lever left if size ever matters more than that; it was
not taken because the dependencies are a minority of the code, each override
is a guess about which paths are cold, and several (image decoding, the sixel
encoder, TLS) have costs `perf_probe` cannot see.

### Linker settings

- **`-z pack-relative-relocs`** (`DT_RELR`) would pack `.rela.dyn` — 0.48 MB
  of relocations after LTO — into a few kilobytes, but glibc understands
  `DT_RELR` only from 2.36, and the Linux release promises glibc ≥ 2.35: it
  is built on Ubuntu 22.04 for exactly that floor (`docs/ops/release.md`).
- **A non-PIE executable** (`-C relocation-model=static`) needs no load-time
  relocations at all, and gets no address-space randomisation for its own
  code — a hardening a security tool should not trade for 0.48 MB.
- **`--icf=all`** folds identical functions, which after fat LTO LLVM's own
  function merging has already done with the whole program in view; only
  `lld` has it, and the aarch64 cross-build links through GNU `ld`.

### What a stable toolchain cannot do

`-Zbuild-std` with `panic_immediate_abort`, `-Zlocation-detail=none` and
`-Zfmt-debug=none` shrink `std` itself, and every one is nightly-only. The
toolchain is pinned to a stable release (`rust-toolchain.toml`), and a
`RUSTC_BOOTSTRAP` build is not one a release should be.

### The dependencies that stay

- **`image-webp` (386 KiB)** — the Ctrl+V paste and the `read` tool accept
  WebP (`docs/input/image-paste.md`).
- **`rustls` + `ring` + `webpki-roots`** — the pure-Rust TLS stance
  (`Cargo.toml`); a system OpenSSL is a smaller binary and a runtime
  dependency the release does not have.
- **`arboard`, `x11rb`, the Wayland stack** — the clipboard, both ways.
- **`toml_edit` (131 KiB)** — `providers.toml`'s parser. `toml` 0.9 parses
  without it, but with a new parser whose error messages the startup toasts
  quote word for word (`docs/providers/user-providers.md`); that upgrade is a change of
  its own.
- **`two-face`'s syntax dump (0.98 MB of read-only data)** — ~250 grammars,
  already compressed; fewer would mean fewer highlighted languages.

## What it does not reach

`cargo build --release` and `cargo install --path .` build `release`, as
before; `cargo install --path . --profile dist` installs the shipped build.
The AUR source package builds `release` with its own flags (`docs/ops/aur.md`),
and makepkg strips its binary anyway; it can move to `--profile dist` from
the first release that has the profile, since it builds a tagged release's
source and never leads one.

## Re-measuring

```bash
scripts/release.sh build                                    # the shipped binary, into dist/
B=target/$(rustc -vV | sed -n 's/^host: //p')/dist/alter-zero
ls -l "$B" dist/*.tar.gz && gzip -n -9 -c "$B" | wc -c      # the file and its download
readelf -S -W "$B"                                          # where the bytes are
CARGO_PROFILE_RELEASE_INCREMENTAL=false cargo bloat --release --crates -n 30   # whose code
cargo run --profile dist --example perf_probe               # the render path, against --release
```

`cargo bloat` is `cargo install cargo-bloat`; it builds the profile it is
given, and `--profile dist` works there too, though under fat LTO its
attribution is coarser still.

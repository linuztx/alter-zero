# Build time — `cargo build --release`

`cargo build --release` is one crate of ~90 000 non-test lines over 340-odd
dependency units, and it was measured — the way `docs/memory.md` measures
RSS — before anything was changed, because the intuitive fixes (a faster
linker, a `codegen-units` knob) turned out not to be where the time was.
This page is the measurement, what it changed, what it deliberately did
not, and how to re-measure.

## The measurement

Three probes, all on the same machine (4 cores, 15 GB, rustc 1.94.1 — the
pinned toolchain, whose default linker on `x86_64-unknown-linux-gnu` is
already the self-contained `rust-lld`, so there was no linker to switch):

- **`cargo build --release --timings`** — cargo's own per-unit report,
  which `scripts/build_timings.py` ranks (the newest report under
  `target/cargo-timings/`, or the one you name): each unit's duration and
  start, then the serial sum against the wall time, so the parallelism the
  machine reached is a number too.
- **A warm rebuild** — `touch src/app/mod.rs && cargo build --release`:
  what a developer pays per edit once the dependencies are cached, the
  number that shapes the day.
- **`RUSTC_BOOTSTRAP=1 cargo rustc --release --lib -- -Ztime-passes`** —
  rustc's per-pass timing of the library alone, which splits the crate's
  own compile into its front end (parsing, type check, borrow check,
  metadata) and its LLVM back end (codegen, optimisation, the thin-local
  LTO pass). `RUSTC_BOOTSTRAP` unlocks a `-Z` flag on the stable
  toolchain for a measurement; it also re-fingerprints every build script
  and proc macro, so budget a rebuild of those on either side.

Runtime was checked with `cargo run --release --example perf_probe`: its
whole-render column (`ui::message_lines` over a 50- to 1600-line code
reply, five runs averaged per size) is a fair CPU proxy for the markdown +
syntax-highlight path, and two of the knobs below trade code quality for
compile time.

## Where the time went

The cold build, before any change:

| | |
| --- | --- |
| wall | **3 min 00 s** |
| CPU (`user`) | 9 min 41 s |
| units | 342 (serial sum 568 s, parallelism 3.15× on 4 cores) |
| this crate | library **43.9 s** (starting at 127 s), binary **8.7 s** (starting at 171 s) |
| warm rebuild after `touch src/app/mod.rs` | **52.7 s** (library 43.8 s + binary 8.7 s, one core busy) |

The dependency units, grouped by what holds them in the graph (CPU seconds,
from the report):

| family | s | what it is |
| --- | --- | --- |
| TLS: `rustls`, `ring` (a C + assembly build script), `webpki-roots` | 46 | `reqwest` with `rustls-tls` |
| `image` + codecs (`image` alone 27.8) | 44 | the Ctrl+V paste, the inline pictures |
| the Wayland stack: `wayland-protocols` (13.0, every protocol generated through `wayland-scanner`), `wl-clipboard-rs`, `tree_magic_mini`, … | 44 | `clipboard::linux`, arboard's `wayland-data-control` |
| `moxcms` (24.3) + `pxfm` (16.9) | 41 | colour management, a hard dependency of `image` ≥ 0.25.7 |
| regex engines: `regex-automata` (19.6), `regex-syntax`, `aho-corasick`, `fancy-regex` | 41 | the tokenizer's o200k pattern, the hook matchers |
| the sixel quantiser: `quantette`, `palette`, `wide`, `rand 0.10`, `bytemuck_derive` + `syn 3`, … | 34 | `ratatui-image` → `icy_sixel`, unconditionally |
| `toml` + `toml_edit` + `winnow` | 16 | `providers.toml` |
| `x11rb-protocol` (11.6) + `x11rb` | 14 | the X11 clipboard |
| `reqwest` | 14 | the HTTP client |
| `onig_sys` (a C build, 9.8) + `onig` | 11 | the highlighter's regex engine |
| `syntect`'s file loaders: `plist`, `quick-xml 0.41`, `yaml-rust`, … | 10 | `default-onig`, **unused** |
| `tokio` | 10 | the event loop, reqwest's runtime |
| `arboard` | 10 | `/copy`, the non-Linux paste |
| ICU4X via `url` → `idna`: twenty small crates | 9 | `reqwest` needs `url` regardless |
| the `time` family | 6 | `ratatui`'s calendar widget and `plist`, **unused** |

Three things fall out of it.

**The dependencies run on every core; the crate runs on one.** The
dependency units finish at about 130 s with the machine ~3.2× busy, then
the library compiles for 44 s essentially alone — rustc's front end is
single-threaded and only codegen fans out — and the binary for 9 s after
it. Cargo *pipelines* library crates (a dependent starts on the `.rmeta`
before codegen finishes) but never a binary against its own library: a
target that links needs the finished rlib (`requires_upstream_objects`),
so `src/tui/`'s ~11 500 lines compile strictly after the library's
~80 000. On four cores the crate is 30% of a cold build and the whole of
a warm one; on sixteen it would be most of a cold one too.

**Two-fifths of the library's time is thin-local LTO.** rustc's own
passes for the library (`-Ztime-passes`, wall seconds):

| pass | s |
| --- | --- |
| `LLVM_thinlto` | **16.7** |
| `LLVM_passes` | 11.9 |
| `generate_crate_metadata` | 5.5 |
| `MIR_borrow_checking` | 3.3 |
| `type_check_crate` | 2.8 |
| `codegen_to_LLVM_IR` | 2.4 |
| `monomorphization_collector_graph_walk` | 1.9 |
| expansion, resolution, coherence, lints, … | ~2 |
| `link` (the rlib) | 0.1 |
| total | 42.3 |

The front end is ~16 s of it. With `lto` unset, a release build still
runs **thin LTO across the crate's own sixteen codegen units** (cargo's
`lto = false` means "local thin", not "none"), and for a crate this size
that pass alone costs 16.7 s on top of the 11.9 s the optimiser spends on
the units themselves — and a non-incremental build redoes all of it, front
end included, for a one-line edit.

**Three of the families were pulled in by features nobody used.**
`syntect`'s `default-onig` brings `plist`, `yaml-rust` and a second
`quick-xml` for loading grammars and themes from *files*, which this crate
never does — its grammars are `two-face`'s prebuilt dumps (`highlight`).
`ratatui`'s default `all-widgets` brings the calendar widget and with it
the `time` family, and `toml`'s default `display` half compiles a
serializer for a file that is only ever read.

## What changed

### Dependency features (no behaviour change)

| crate | was | now | drops |
| --- | --- | --- | --- |
| `syntect` | `default-onig` | `parsing`, `regex-onig` | `html`, `plist-load` (`plist`, `quick-xml 0.41`), `yaml-load` (`yaml-rust`, `linked-hash-map`), the bundled default syntax/theme dumps |
| `two-face` | `syntect-default-onig` | `syntect-onig` | the same — it only forwarded `syntect/default-onig` |
| `ratatui` | default | `crossterm`, `underline-color`, `layout-cache` | `all-widgets` → `widget-calendar` → `time`, `time-macros`, `deranged`, …; `macros` → `ratatui-macros` |
| `toml` | default | `parse` | the `display` half of `toml` and `toml_edit` |

`parsing` already implies `dump-load` (and `flate2` + `bincode`), which is
what `two-face`'s embedded dumps deserialize through, and `two-face` itself
asks syntect for exactly `parsing` + `dump-load`; `syntect-onig` is what
selects its **onig-compiled** dump over the fancy-regex one (the two are
separate binaries, the engines compiling grammars differently), so the
highlighter's memory rationale in `Cargo.toml` and `highlight` is
untouched. `underline-color` is not optional here — `links` and `images`
ride the `underline_color` channel as their carrier — and `layout-cache`
stays because dropping a default that costs nothing buys nothing.

Fourteen crates left the release graph (343 → 329 units; `Cargo.lock` lost
71 lines), `syntect` itself compiles in 7.6 s instead of 11.1 and
`toml_edit` sheds its serializer. A cold build measured **2 min 54 s
against 3 min 00 s**, the serial sum down 27 s (568 → 541 s): a modest
saving, because the units that went were the small ones compiling in
parallel with the heavy families above, and because the profile below
spends a few of those seconds back on the crate's own first build. The
trims are kept for what they are — less to compile, less to link, nothing
lost — not for the wall clock.

### `[profile.release]`

```toml
[profile.release]
incremental = true
```

The warm rebuild is the number that shapes the day, and it was 52.7 s for
a one-line edit — the library's 44 s (front end and LLVM both, from
scratch) then the binary's 9 s. With incremental compilation on, rustc
keeps its dependency graph and query results between runs and reuses the
object code of every codegen unit an edit did not reach (it partitions
finer for this, up to 256 units, and caches the thin-LTO imports per
unit), so only the changed units and their importers are re-optimised.
Measured on the same edits, deps cached:

| rebuild | before | after |
| --- | --- | --- |
| first build of the crate (the cache is written) | 52.7 s | 59.3 s |
| `touch src/app/mod.rs`, nothing changed | 52.7 s | 4.4–5.8 s |
| a statement added to `ui::wrap_text` (called from every renderer) | 52.7 s | **9.1 s** |
| a statement added to `App::on_key` | 52.7 s | **10.0 s** |
| reverting either | 52.7 s | 8.9–10.4 s |

The ~5 s floor is the work an incremental build cannot skip — re-parsing
the crate, re-encoding its metadata, linking the 23 MB binary — and is the
part a workspace split (below) would attack.

What it costs. The first build is ~10% slower (the cache is written; on a
cold build the crate takes 46 s instead of 44). `target/` grows from
792 MB to 1.4 GB with the cache, and the binary by 0.3% (22.76 → 22.83 MB).
And the shipped code is **unchanged within noise**: the same `perf_probe` whole-render column, plain build
against incremental —

| code lines | plain | incremental |
| --- | --- | --- |
| 50 | 32.9 ms | 33.1 ms |
| 100 | 61.6 ms | 62.2 ms |
| 200 | 123.8 ms | 123.9 ms |
| 400 | 251.4 ms | 247.7 ms |
| 800 | 526.8 ms | 506.1 ms |
| 1200 | 750.5 ms | 770.2 ms |
| 1600 | 983.4 ms | 1011.3 ms |

— five runs averaged per size, every difference inside ±3% and in both
directions, which is what thin LTO across the finer units buys back. The
setting reaches **this crate only**: cargo never compiles registry
dependencies incrementally (the first build under it recompiled one
package, the dependency units byte-for-byte what they were), which is also
why `cargo install --path .` pays only the first-build overhead. A build
that wants the plain layout regardless — a packaged release, a CI job —
asks for it on the command line, no manifest edit needed:

```bash
cargo build --release --config profile.release.incremental=false
```

**`lto = "off"` was measured and refused.** Cargo's default for release is
not "no LTO" but thin LTO across each crate's own codegen units, and
switching it off is the one knob that touches the *dependencies'* compile
too:

| | `lto` unset | `lto = "off"` |
| --- | --- | --- |
| cold build | 3 min 00 s (9 min 41 s CPU) | **2 min 22 s** (7 min 49 s CPU) |
| the library | 43.9 s | 33.9 s |
| warm rebuild, non-incremental | 52.7 s | 42.4 s |
| warm rebuild, incremental, after an edit | 7.3 s | 5.9 s |
| `perf_probe`, 50 → 1600 lines | — | **+11.6%, +18.8%, +9.8%, +7.2%, +1.9%, +10.5%, +9.5%** |

A fifth off the cold build, and the render path 7–19% slower at every
size — every crate loses the cross-unit inlining its thin LTO pass
provided, and the syntect + wrap path is exactly the kind of code that
notices. For a TUI whose promise is to be light and fast, the cold build
is the wrong side of that trade; the number is recorded here so nobody
has to measure it twice.

## What stays, and why

The report's other big families are each held in place by something the
app needs, so they are documented here rather than trimmed:

- **`moxcms` + `pxfm` (41 s CPU)** — `image` ≥ 0.25.7 depends on the
  colour-management crate unconditionally, and its SIMD shaper paths are
  what take the time; features are additive, so a dependent cannot turn
  them off. The one `image` without it, 0.25.6, wants `png 0.17`, and
  `images::fitted` is written against the `png 0.18` decoder — pinning back
  would compile two `png`s and undo the "already in the tree" rationale
  `Cargo.toml` gives the direct `png` dependency.
- **The sixel quantiser (~34 s CPU)** — `ratatui-image` depends on
  `icy_sixel` unconditionally (no feature to drop it, through 11.0.8), and
  sixel is one of the three graphics protocols `docs/images.md` supports.
  It is also what keeps a second `rustix` (0.38, 4 s) and a `thiserror 1`
  in the graph.
- **The Wayland stack (44 s CPU)** — `clipboard::linux`'s streamed
  `image/png` read is built on `wl-clipboard-rs` directly, and arboard's
  `wayland-data-control` is what makes `/copy` work on a Wayland desktop.
- **TLS (46 s CPU)** — the crate's pure-Rust stance (`reqwest` with
  `rustls-tls`); `native-tls` would be cheaper to compile and bring a
  system OpenSSL.
- **The regex engines (41 s CPU)** — the tokenizer's o200k split pattern
  needs `fancy-regex` with the full Unicode tables (`\p{L}`, `\p{N}`), and
  every `unicode-*` feature is a table `regex-automata` compiles; `regex`
  itself is a thin wrapper over the same engine.
- **`x11rb-protocol` (12 s)** — the core X11 protocol as generated code;
  no extension feature is on.
- **`onig_sys` (10 s, a C build)** — the deliberate oniguruma choice
  (`highlight`: 187 MB RSS with fancy-regex against ~23 MB).
- **ICU4X via `url` (9 s across twenty small crates)** — `reqwest` needs
  `url` regardless; the documented `idna_adapter = "=1.1.0"` pin would swap
  ICU for `unicode-normalization`, but for nine CPU seconds spread over
  crates that compile in parallel with the heavy ones, an exact-version pin
  on a transitive crate is not a trade worth making.

## The structural option

The lever left is the one this page does not pull: the library is a single
crate, so its front end — ~16 s of the 44 — serialises on one core while
the other three idle, and even an incremental rebuild re-runs the whole
crate's parse, metadata and link (the ~5 s floor below). A **workspace
split** would let the front ends of independent modules run side by side
and let an edit recompile only its own crate: `tokenizer`, `markdown` +
`highlight`, `textarea`, `file_search`, `session` + `history` and the
`llm` wire formats are each reachable only through `crate::` paths that
`tests/api_surface.rs` locks, so each is a candidate leaf. It is a design
change, not a build setting — the module map in `docs/module-layout.md`,
the `crate::app::X` paths every doc names and the API-surface test all
move with it — so it is recorded here as the next step and not taken.

## Re-measuring

```bash
cargo build --release --timings && scripts/build_timings.py           # cold: rank the units
touch src/app/mod.rs && time cargo build --release                     # warm: the per-edit cost
RUSTC_BOOTSTRAP=1 cargo rustc --release --lib -- -Ztime-passes 2>&1 \
  | grep '^time:' | sort -t: -k2 -rn | head                            # the library's passes
cargo run --release --example perf_probe                               # the runtime check
```

# AGENTS.md

Instructions for coding agents (**Claude Code**, Codex, Copilot, Cursor, anything
that reads `AGENTS.md`) working in this repository. `CLAUDE.md` is read by Claude
Code too; where the two disagree, this file wins on **what to run**, `CLAUDE.md`
wins on **how the code works**.

Read [`CLAUDE.md`](CLAUDE.md) (the architecture map) and the `docs/` page for the
area you are touching before changing it. `docs/` holds the design rationale —
what looks arbitrary in this code is usually a bug someone already paid for.

## The most important rule: run the smallest check that can fail

This is a single crate of about 250k lines of Rust (some 80k of it outside the
tests) with **about 5,500 unit tests** and **146 smoke phases**. The full suite
is not a "does it still work" button — it is the last step before a commit, and
CI runs it for you on every push.

Do **not** run `cargo test` (or `cargo clippy --all-targets`, or
`scripts/smoke.sh`) after every edit to see whether something broke. Pick the
targeted command for the code you touched, and escalate only when the change
crosses a boundary.

### What each command actually costs here

Measured on an 8-core Apple-silicon Mac, warm `target/` (not a cold checkout):

| Command | Wall time | Run it when |
| --- | --- | --- |
| `cargo check --lib` | ~6 s | after any edit, if you only want "does it compile" |
| `cargo test --lib <module>::` | ~0.4 s if nothing rebuilt, **~9 s after an edit** (recompile + link) | default loop after every edit in the library |
| `cargo test --lib` (every unit test) | ~44 s | a full unit pass mid-change |
| `cargo test --test <name>` | ~12–35 s (rebuilds the bin for `CARGO_BIN_EXE`) | you touched a binary-facing seam (see matrix) |
| `cargo fmt --check` | ~2 s | before commit |
| `cargo clippy --all-targets -- -D warnings` | ~23 s warm | before commit, or after touching a lint-visible shape |
| `cargo doc --no-deps --lib` | ~4 s | before commit **or** after moving/renaming a public item |
| `cargo test` (everything, every integration binary) | **~2 m 20 s** | once, before you call the work done (it is the gate's test step) |
| `scripts/smoke.sh <id>` (one phase) | ~2–20 s | you touched the terminal boundary |
| `scripts/smoke.sh` (every phase) | minutes, parallel | **only** for `src/tui/`, `term.rs`, rendering/geometry |
| `cargo build --release` / `--example …` | 40–60 s+, plus a second profile in `target/`, which grows to ~10 GB | only when the task explicitly needs a release artifact |

`target/` grows to ~10 GB and is incremental. Deleting it to "clean up" costs a
multi-minute full rebuild — don't, unless the user asks.

### Which tests to run after a given change

| You changed | Run this |
| --- | --- |
| `src/app/<area>.rs` (pure state) | `cargo test --lib app::tests::<area>` — the tests live beside the code, often in `src/app/tests/<area>.rs` |
| `src/ui/<area>.rs` (renderers, theme) | `cargo test --lib ui::` — plus one smoke phase if the change is visible |
| `src/llm/<provider or wire>` | `cargo test --lib llm::` — plus the live test for that provider **only if the user asks** (`--ignored`, spends real tokens) |
| `src/tui/`, `src/main.rs` | the boundary is verified on screen: `cargo build && scripts/smoke.sh <relevant id or range>` — plus `cargo test --bin alter-zero` for the few pure helpers tested in `src/tui/` |
| `src/term.rs` | `cargo test --lib term::` for its pure helpers (the cell emitter, the startup-read scanner), then the smoke phases that paint what you changed |
| anything on screen (spacing, glyphs, colours, cursor) | one or two smoke phases; the whole suite only at the end |
| a public item moved/renamed, or a `mod.rs` re-export | `cargo test --test api_surface` + `cargo doc --no-deps --lib` |
| `src/cli.rs`, `--help`, arg parsing | `cargo test --lib cli::` + `cargo test --test cli_help` |
| `src/headless.rs`, `src/tui/headless.rs`, anything `-p` reaches | `cargo test --lib headless::` + `cargo test --test headless` (the real binary over pipes, a stand-in provider on the loopback) + `scripts/smoke.sh 153` for the tty case |
| `src/pty/`, `src/subprocess.rs` | `cargo test --lib pty::` or `subprocess::` — plus `cargo test --test detached_exec` when the detach or the helper re-exec changed |
| `src/mcp/`, `src/llm/mcp/`, the `mcp` CLI | `cargo test --lib mcp::` (the filter matches both modules) + `cargo test --test mcp_cli` for the CLI |
| a memory-sensitive path (per-turn/per-request/per-picture allocation) | the matching `tests/*_memory.rs` gate (`ls tests/*_memory.rs`) |
| `providers.toml`, `src/llm/config.rs` | `cargo test --lib llm::config` — it pins the shipped provider set and the `snake_case` spellings |
| `scripts/smoke/lib.sh`, `scripts/smoke.sh` (the phase readers or the selector) | `bash scripts/smoke/selftest.sh` — under a second, no app, no tmux |
| `scripts/smoke/phases/`, screen behaviour | `cargo build && scripts/smoke.sh <ids>` — by **id**, and widen only if you must |
| `scripts/`, `install.sh`, `npm/`, `telemetry/`, `docker/`, `aur/` | that subtree's own suite (see [`CONTRIBUTING.md`](CONTRIBUTING.md) *Testing*) — not `cargo test` |
| `prompts/`, `docs/`, `CHANGELOG.md` | nothing; prose has no gate |

A filtered run is not a compromise: `cargo test --lib app::tests` runs about
1,060 tests in ~0.4 s once the binary is built, and while the tree is being
edited the only cost is the ~9 s relink. The expensive part is the build, not the
tests.

A filter matches any test whose path contains it, so a module path selects
everything under it: `cargo test --lib app::tests` covers every area under
`src/app/`, where `cargo test --lib app::tests::login` covers one file — and the
two cost the same. Prefer the wider filter to two narrow runs; when the areas
don't share a prefix, pass several filters in one run after `--`
(`cargo test --lib -- app::tests::login term::`), and a test any of them
matches runs. Because filters are substrings, `--skip background::` would also
skip the pure `app::tests::background` tests, so when you want nearly everything
except the environment-dependent modules,
`cargo test --lib -- --skip pty:: --skip background::tests:: --skip llm::exec::`
runs about 4,950 tests in ~20 s.

### Before you say the work is done

The project's gate (also what `.github/workflows/ci.yml` runs for you on push):

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                 # once, at the end
cargo doc --no-deps --lib
```

`Cargo.toml`'s `[lints]` table already denies `warnings`, `clippy::all` and
`unsafe_code`, so a warning is a build failure locally, not just in CI. The doc
build is part of the gate because a broken intra-doc link is an error here: a
public item's docs may not link to a private one, so moving an item between
modules breaks the links that pointed at it.

## Linux vs macOS — what actually differs

CI runs the gate and the smoke suite on `ubuntu-latest`, and the gate's
`cargo test` and `cargo doc` on `macos-latest` too (the `test · doc (macOS)`
job, beside the clipboard tests and the smoke readers' selftest). Both are
expected to be green, so a red `cargo test --lib` is yours on either — once a
Mac has `setsid` on `PATH` (below). Know which platform you are on before you
diagnose anything: the two compile different code
(`cfg(target_os = "linux")`) and run different tools (GNU and procps, or
BSD).

| | Linux (CI) | macOS (CI) |
| --- | --- | --- |
| `cargo test --lib` | green | green with `setsid` on `PATH`; **about 46 failures** without it |
| `setsid` | ships with util-linux | **absent**; `brew install util-linux`, then put it on `PATH` |
| `pkill`, `ps`, `od` | procps and GNU | BSD's: **`pkill` has no `-s` and its `-t` matches nothing (`ps -t` works), `ps` has no `sid`, `od` pads its columns** — code and tests must not lean on the GNU forms |
| `--lib` tests compiled | about 6,540 | about 6,510 — **38** are `cfg(target_os = "linux")` and never compile here, **6** (`clipboard/macos.rs`) compile only here; the 10 in `src/tui/` are bin-only on both |
| `scripts/smoke.sh` | bash 5 from the distro | needs **bash 4+**; macOS `/bin/bash` is 3.2 and dies |
| `--list`, phase titles/tags | works | works — the readers were BSD-`sed`-broken once; `selftest.sh` pins them, and CI runs it on macOS |
| `timeout`, `nproc`, `grep -P` | present | **absent** (`smoke.sh` already falls back to `sysctl -n hw.ncpu`) |
| a stale incremental link | rare | **`Undefined symbols: … (.llvm.…)`** after a branch switch |

**38 tests never run on macOS.** They are not skipped-and-reported; the Linux
`cfg` compiles them out, so nothing in the output mentions them. By file:
`llm/exec.rs` 18, `pty/probe.rs` 8, `clipboard.rs` 4, `subprocess.rs` 4,
`background.rs` 2, `pty/spawn.rs` 1, `llm/mcp/transport.rs` 1. Each asserts
something only Linux has — the `/proc` probe, the foreground program's name,
`setsid` detaching — and `docs/interactive-shell.md` *Limits* says what macOS
does instead. A test of something every platform has stays `cfg(unix)`, with
its Linux-only lines gated inside it. The modules themselves (`pty/probe`,
`clipboard/linux`) have Linux-only bodies, and `tests/clipboard_linux.rs` is
`#![cfg(target_os = "linux")]` — it compiles to **0 tests** on a Mac, silently.
The reverse holds for `clipboard/macos.rs`'s 6 unit tests and
`tests/clipboard_macos.rs`: Linux never compiles them, and CI's
`clipboard (macOS pasteboard)` job is what runs them.

So a green macOS run says nothing about: the `/proc`-based "is it waiting for
input" probe, the streamed `image/png` clipboard read, `setsid -c` detaching,
or the Linux half of the executor. Say in the commit which platform you
actually ran. Those areas need the Linux CI run (or a container) to mean
anything.

### macOS: the lib suite needs `setsid`

A test that starts a terminal session gets it from the detach chain's TTY
tiers (`subprocess::tty_tiers`): util-linux's `setsid -c`, then the app's own
helper re-exec. A test binary has libtest's `main`, not the app's, so there is
no helper to re-exec, and macOS ships no `setsid`. Without one on `PATH`, about
46 tests fail with `no way to give the command a terminal of its own here` —
the environment, not your change. Put util-linux's on `PATH`:

```bash
brew install util-linux
PATH="$(brew --prefix util-linux)/bin:$PATH" cargo test --lib
```

CI's macOS job links util-linux's `setsid` alone onto `PATH`, so every other
tool the tests run is still macOS's own. The helper tier, the one the app uses
on a Mac, is covered against the real binary by `tests/detached_exec.rs`,
which `cargo test` runs.

A test that fails on a Mac with `setsid` on `PATH` is a real failure: diagnose
it. Six such failures used to be listed here as a baseline. One was `bashkill`
never sending `SIGTERM` on macOS (its `pkill` has no `-s`); the rest were
tests that read GNU output or asserted Linux-only behaviour.

Other platform notes:

- **`scripts/smoke.sh` needs bash 4+** (`declare -A` at its worker table).
  macOS `/bin/bash` is 3.2, so running it under that shell dies with
  `declare: -A: invalid option` and `TOTAL: unbound variable` — not a phase
  failure. Put a newer bash first on `PATH` (`brew install bash`); when the suite
  dies like this, `bash --version` is the first thing to check.
- **The smoke suite's own tests are `bash scripts/smoke/selftest.sh`** — the
  phase readers and the selector, over fixture headers, in under a second. Run
  it after touching `scripts/smoke/lib.sh` or `scripts/smoke.sh`; CI runs it
  before the phases and again on macOS under BSD `sed`. It exists because those
  readers fail *silently*: they decide which phases a query runs, so a reader
  that returns nothing runs the wrong suite rather than failing.
- **A stale incremental build can fail to link** with
  `Undefined symbols: … (.llvm.…)` after switching branches. That is
  incremental codegen, not your change. `cargo clean -p alter-zero` fixes it
  and keeps every dependency, so the rebuild is short — do **not** reach for
  `rm -rf target/`.
- Every test that reaches the network, a real clipboard or a real X server is
  `#[ignore]`d (`tests/live_*.rs`, `tests/clipboard_*.rs`, and two in
  `tests/wire_history.rs`). `cargo test` is
  offline and deterministic — keep it that way. Never add `-- --ignored` to a
  routine run: those tests need API keys and **spend real tokens** on whatever
  account the key belongs to. The clipboard ones also overwrite your real
  pasteboard / need an X server.
- Smoke phases each get their own tmux server, config home, `$HOME` and temp
  tree, so they neither see nor disturb your real `~/.alter-zero`, clipboard or
  `/resume` list.

### What a platform-blind diagnosis gets wrong

Three failure modes to name, because each looks like something it is not:

- **"My change broke 46 tests"** on a fresh Mac — it did not; `setsid` is
  missing. Confirm with `command -v setsid` before reading a single stack trace.
- **"It passes on macOS, ship it"** for anything in `pty/probe`, `exec`,
  `subprocess`, `background` or `clipboard` — half of that code is behind
  `cfg(target_os = "linux")` and macOS never compiled it. Those need the Linux
  CI run (or a container) to mean anything.
- **"It passes on Linux, ship it"** for code or a test that runs a system
  tool — `pkill`, `ps`, `od`, `sed`. Linux's are procps and GNU, a Mac's are
  BSD's, and they take different flags and print different spacing. Read the
  BSD man page, or let CI's macOS job tell you.

## Working style (standing instructions, not suggestions)

From [`CONTRIBUTING.md`](CONTRIBUTING.md), unchanged:

- **TDD is rigid**: no production code before a failing test, and you must see
  the test fail for the right reason. The only exception is `src/main.rs` +
  `src/tui/` — the terminal I/O boundary, verified by running the app and the
  smoke suite instead.
- **Design before implementing** for a feature or non-trivial change: explore,
  ask your questions one at a time, propose 2–3 approaches, agree, then write it
  down in `docs/` (and update `docs/design.md` when behaviour changes). Trivial
  fixes need no ceremony — the failing test still does.
- **One logical change per commit**, message a reader can act on. Don't bump the
  version or edit a released changelog section.
- Intent lives in [`docs/`](docs/), user-visible changes in `CHANGELOG.md`, and
  the README stays a front door. A `docs/` page that no longer matches the code
  is a bug.

## Conventions

- All styling is centralized: glyphs/geometry as `const`s in `src/ui/theme.rs`,
  colours as accessors reading `src/ui/palette.rs`. A new colour is a new role on
  `Palette` in **every** theme table plus its accessor — never a literal
  `Color::Rgb` outside `palette.rs`.
- Width math goes through `cols()` (display columns), never `chars().count()`;
  every cell handed to the backend goes through `term::visible_cells`.
- The input line is a `textarea::TextArea`, not a `String`.
- Never build a `Value` tree for a body you read a few fields from, and never
  decode a picture whole to make a small one — resident memory is a feature
  here ([`docs/memory.md`](docs/memory.md), the `tests/*_memory.rs` gates).
- `unsafe_code = "forbid"`. The terminal and the pty are reached through safe
  wrappers (`rustix`), not `pre_exec`.
- Keep logic out of the I/O boundary: when `tui/` needs a decision, the decision
  belongs in a pure helper it calls (the live-region geometry works this way).

## Don't

- Don't run the whole `cargo test` or the whole smoke suite "to check" after a
  small edit — see the table above.
- Don't `rm -rf target/`, don't re-enable/disable `incremental`, and don't touch
  `[profile.*]` or `rust-toolchain.toml` to "make it faster". The toolchain pin
  exists so the gate is reproducible; a floating `stable` breaks the build.
- Don't leave a second filtered `cargo test` in the same edit; widen the filter.
- Don't run `scripts/release.sh prepare`, `build`, `npm` or `download`,
  `docker/build.sh`, `makepkg`, or anything else that bumps a version,
  publishes, builds images, or pushes — those are the user's calls.
  (`scripts/release.sh selftest` and `check` are the release tooling's own
  tests; run them when you touch it.)
- Don't add `#[ignore]` to a failing test, or `#[allow(...)]` to silence a lint,
  in order to get green. Fix the cause or say you are blocked.

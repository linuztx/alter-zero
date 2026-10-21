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
tests) with **about 5,500 unit tests** and **145 smoke phases**. The full suite
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

CI runs the gate and the smoke suite on `ubuntu-latest`, so **Linux is the
reference**: `cargo test --lib` there is expected to be green, and any red is
yours. On macOS a red run usually is not. Know which platform you are on before
you diagnose anything. (CI's two macOS jobs run only the clipboard tests and the
smoke readers' selftest.)

| | Linux (CI) | macOS |
| --- | --- | --- |
| `cargo test --lib` | green | **6 baseline failures** with `setsid` on `PATH`, **46** without it |
| `setsid` | ships with util-linux | **absent**; `brew install util-linux`, then put it on `PATH` |
| `ps … sid` | supported | **no `sid` keyword** — one test can never pass here |
| `--lib` tests compiled | about 5,460 | about 5,430 — **35** are `cfg(target_os = "linux")` and never compile here, **6** (`clipboard/macos.rs`) compile only here; the 10 in `src/tui/` are bin-only on both |
| `scripts/smoke.sh` | bash 5 from the distro | needs **bash 4+**; macOS `/bin/bash` is 3.2 and dies |
| `--list`, phase titles/tags | works | works — the readers were BSD-`sed`-broken once; `selftest.sh` pins them, and CI runs it on macOS |
| `timeout`, `nproc`, `grep -P` | present | **absent** (`smoke.sh` already falls back to `sysctl -n hw.ncpu`) |
| a stale incremental link | rare | **`Undefined symbols: … (.llvm.…)`** after a branch switch |

**35 tests never run on macOS.** They are not skipped-and-reported; the Linux
`cfg` compiles them out, so nothing in the output mentions them. By file:
`llm/exec.rs` 17, `pty/probe.rs` 8, `clipboard.rs` 4, `subprocess.rs` 4,
`background.rs` 1, `pty/spawn.rs` 1. The modules themselves (`pty/probe`,
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

### macOS: the six baseline failures

Do not chase them, do not "fix" them, and do not report them as regressions.
They need a real Linux/tty environment or a `setsid` binary:

- `pty::spawn::tests::the_session_leads_its_own_group_and_session` — macOS `ps`
  has no `sid` keyword.
- `llm::exec::tests::kill_asks_the_command_to_stop_before_it_insists`,
  `…::indented_lines_reach_a_program_that_takes_pastes_as_one_paste`,
  `…::a_key_name_sent_without_brackets_is_the_key_in_a_full_screen_program` —
  SIGTERM/paste timing through the pty.
- `background::tests::a_background_display_that_reads_whole_lines_is_never_told_of`,
  `…::a_picker_reading_its_terminal_waits_however_its_line_animated`.

Without `setsid` on `PATH`, the count is **46**: every tty-spawning test fails
with `no way to give the command a terminal of its own here`. macOS ships no
`setsid`; util-linux does. Put it on `PATH` before running the lib suite:

```bash
PATH="/opt/homebrew/opt/util-linux/bin:$PATH" cargo test --lib
```

A green lib run with the six skipped:

```bash
PATH="/opt/homebrew/opt/util-linux/bin:$PATH" cargo test --lib -- \
  --skip background::tests::a_background_display_that_reads_whole_lines \
  --skip background::tests::a_picker_reading_its_terminal_waits \
  --skip llm::exec::tests::a_key_name_sent_without_brackets \
  --skip llm::exec::tests::indented_lines_reach_a_program \
  --skip llm::exec::tests::kill_asks_the_command_to_stop \
  --skip pty::spawn::tests::the_session_leads_its_own_group
```

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

Two failure modes to name, because both look like your bug and are not:

- **"My change broke 46 tests"** on a fresh Mac — it did not; `setsid` is
  missing. Confirm with `command -v setsid` before reading a single stack trace.
- **"It passes on macOS, ship it"** for anything in `pty/probe`, `exec`,
  `subprocess`, `background` or `clipboard` — half of that code is behind
  `cfg(target_os = "linux")` and macOS never compiled it. Those need the Linux
  CI run (or a container) to mean anything.

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

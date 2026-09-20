# Contributing to Alter Zero

Thanks for looking. Alter Zero is a terminal coding agent written in Rust, and
this page is what you need in order to change it: how to get a build running,
the gate every change passes, the two practices the codebase was built with,
and where the rest is written down.

The design rationale is deliberately not here. [`docs/`](docs/) holds a page per
feature saying what it does *and why it is built that way*, and `CLAUDE.md` is
the architecture map. Read the page for the area you are touching before you
change it — most of what looks arbitrary in this code is a bug someone already
paid for.

## Ways to help

| | |
| --- | --- |
| **Report a bug** | Open an issue with your terminal, OS, `alter-zero --version`, and the steps that reproduce it. A screenshot or a `tmux capture-pane` dump is worth a paragraph for anything visual. |
| **Fix a bug** | Start with the failing test that reproduces it, then the fix. Small, obvious fixes need no discussion first. |
| **Add a feature** | Open an issue and agree a design before you write it — see [Design before implementing](#design-before-implementing). |
| **Improve the docs** | A `docs/` page that no longer matches the code is a bug. So is a `CHANGELOG.md` entry that does not say what a user will notice. |
| **Add a model provider** | Usually a `[providers.<id>]` block in [`providers.toml`](providers.toml) — the file's own header documents every key. A provider with a new authentication shape or wire format is a feature: agree it first ([`docs/llm.md`](docs/llm.md)). |

## Getting set up

You need [rustup](https://rustup.rs) and a C compiler. The toolchain is pinned
in `rust-toolchain.toml`, and rustup installs it for you — don't override it, a
floating `stable` is exactly what the pin exists to prevent.

```bash
git clone https://github.com/linuztx/alter-zero.git
cd alter-zero
cargo run
```

With no provider configured, the app opens its **offline demo** — scripted turns
whose tool calls resolve through the *real* executors, so the cells are numbered,
highlighted and red-on-failure like live ones. Most UI work is developed against
it, and the smoke suite drives it exclusively ([`docs/dummy-backend.md`](docs/dummy-backend.md)).

<details>
<summary><strong>Tools you need only for some areas</strong></summary>

| Tool | Needed for |
| --- | --- |
| `tmux` | `scripts/smoke.sh` — the only automated coverage of the terminal I/O boundary |
| `node` | the telemetry collector's tests (`telemetry/`) |
| `shellcheck` | `install.sh` and `scripts/release/*.sh`, which CI lints |
| `python3` | the container tests, `scripts/build_timings.py` |
| Docker or Podman | `docker/` — the headless Kali image |
| `Xvfb` | the Linux clipboard's X11 paste read (`cargo test --test clipboard_linux -- --ignored`) |

</details>

## The gate

Four commands. All four clean before you call a change done:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps --lib
```

CI runs exactly these, and `Cargo.toml`'s `[lints]` table bakes the same posture
into every build — `unsafe_code = "forbid"`, `warnings` and `clippy::all` denied
— so a warning fails your local build too.

The doc build is in the gate for a specific reason: because the crate denies
warnings, a broken intra-doc link is an **error**, and a public item's docs may
not link to a private one. Moving an item between modules or narrowing its
visibility breaks the links that pointed at it. Fix it by qualifying the path if
the target is still public (`` [`x`] `` → `` [`x`](App::x) ``), else demote the
link to a plain code span (`` [`x`] `` → `` `x` ``) so the prose still names it.

## Working style

Two practices shaped this codebase. They are standing instructions, not
suggestions.

### Test-driven development

**The Iron Law: no production code without a failing test first.** Work in
Red → Green → Refactor:

1. **Red** — write one minimal test naming the behaviour you want, run it, and
   **watch it fail for the right reason** (the feature is missing, not a typo in
   the test). A test you did not see fail proves nothing.
2. **Green** — write the *minimal* code to pass it. No speculative features.
3. **Refactor** — clean up with the tests staying green.

If you wrote production code before its test, delete it and re-derive it from the
test. Bug fixes are not an exception: a fix starts with the test that reproduces
the bug, or you have no evidence the cause is what you think it is.

The one exception is `src/main.rs` and `src/tui/` — the terminal I/O boundary,
which has no unit tests. Verify changes there by running the app and the smoke
suite instead.

### Design before implementing

"Add X" states *what*, not "skip the design". For a feature or any non-trivial
change: explore the existing code, ask your clarifying questions one at a time,
propose two or three approaches with a recommendation, and get agreement on a
design first. Then capture it — a new `docs/` page for a new feature, and
`docs/design.md` updated whenever behaviour changes.

Trivial, self-evident edits and bug fixes with an obvious cause do not need the
ceremony. The failing test still does.

## How the code is laid out

A **library** (`src/lib.rs`) holds the logic; **`src/main.rs`** is a thin
shell over **`src/tui/`**, the binary-private tree that drives the async
(`tokio`) `select!` loop.

That split is what makes the crate testable, and it is the rule to preserve:
pure, unit-tested logic in `app/`, `ui/`, `stream/`, `textarea`, `session`,
`history`, `context` and the pure cores of `frame`/`paste`/`subprocess`;
terminal I/O confined to `src/tui/` and `term.rs`. **Keep logic out of the
boundary** — when the boundary needs a decision, the decision belongs in a pure
helper it calls (the live-region geometry works this way: `term.rs` acts on
`ui::live_height`, `ui::repin`, `ui::cursor_position`).

| Where | What |
| --- | --- |
| `src/app/` | Conversation state and the pure update logic — one module per area, with the `App` struct in `mod.rs` so every submodule keeps its private-field access |
| `src/ui/` | Every renderer, plus the centralized theme and palette |
| `src/tui/` | The event loop and the I/O boundary — one `impl Session` block per area |
| `src/stream/` | The backend seam (`ReplySource`, `StreamEvent`) and the offline demo backend |
| `src/llm/` | The real backend: the agent tool loop, the four wire formats, auth, MCP |
| `tests/` | Integration tests, including the memory gates and the `#[ignore]`d live ones |
| `scripts/smoke/phases/` | One file per smoke phase |
| `prompts/` | The system prompt, the built-in agent definitions and the built-in skill |
| `telemetry/`, `docker/` | The collector Worker and the headless Kali image, each with its own tests |

[`docs/module-layout.md`](docs/module-layout.md) is the full map.

### The four invariants

`CLAUDE.md` states them in full, under *The runtime model and its invariants*.
They are not style preferences — breaking one reintroduces a class of bug that is
very hard to see in a diff:

1. **One stdin reader**, created after the init cursor query.
2. **Greedy word-wrap is prefix-stable** — appending only ever changes the last
   wrapped line, which is what makes streaming straight to scrollback safe.
3. **The viewport is content-anchored** (top fixed), and a resize reflows both
   directions from history.
4. **Tool calls interleave with text**, and commits made under a full-screen
   overlay *queue* rather than draw.

Read them before touching the loop, `term.rs`, or the wrap/markdown renderers.

## Testing

`cargo test` runs the unit tests (around 3,900, in `#[cfg(test)]` modules beside
the code they cover) plus the integration tests in `tests/` that are not
`#[ignore]`d. It is offline and deterministic — keep it that way.

| Suite | How | Run it when |
| --- | --- | --- |
| Unit + integration | `cargo test` | always |
| Smoke suite | `cargo build && scripts/smoke.sh` | you changed `src/tui/`, `term.rs`, or anything you can only see on screen |
| Live provider tests | `OPENROUTER_API_KEY=… cargo test --test live_openrouter -- --ignored --nocapture` | you changed a provider, a wire format, or the context assembly |
| Memory gates | `cargo test --test image_turn_memory`, `--test model_parse_memory`, `--test image_paste_memory` | you changed anything that allocates per turn, per request or per picture |
| Telemetry collector | `(cd telemetry && node --test)` | you changed `telemetry/` |
| Release tooling | `scripts/release.sh selftest` and `scripts/release.sh check` | you changed `scripts/release/` or `install.sh` |
| Container | `python3 -m unittest discover -s docker/tests -p 'test_*.py'` | you changed `docker/` |

### The smoke suite

`scripts/smoke.sh` drives the real binary inside tmux and **asserts on what it
painted**. It is the only automated coverage of the I/O boundary, so a change
there is not tested until a phase says so.

```bash
cargo build && scripts/smoke.sh               # every phase, in parallel
scripts/smoke.sh 55 permission                # an id, a range (50-60), a name substring
scripts/smoke.sh --list                       # the phases, their tags and last durations
scripts/smoke.sh --failed --serial            # re-run what failed, one at a time
bash scripts/smoke/phases/055-permission.sh   # one phase on its own, output live
```

Adding coverage is adding a file — `scripts/smoke/phases/NNN-slug.sh`, sourcing
`scripts/smoke/lib.sh` and calling `smoke_begin`. The id, slug and title all come
from the file, so nothing registers anywhere.
[`docs/smoke.md`](docs/smoke.md) has the helper vocabulary, the phase contract
and the rules that keep a phase from being flaky. The one to internalise:
**poll for the state, never fixed-sleep and hope.**

Each phase gets its own tmux server, config home, sessions root, `$HOME` and temp
tree, and sanitizes the environment (every `*_API_KEY`, every `ALTER_ZERO_*`,
`OLLAMA_HOST`, `NO_COLOR`, the display variables) before its first launch. That
is what makes running phases in parallel correct — and what keeps a run off your
real clipboard, config and `/resume` list.

### Live tests

Every test that reaches the network is `#[ignore]`d so `cargo test` stays offline.
They read credentials from the environment. **Never commit a key**, and note that
`tests/live_caching.rs` and friends spend real tokens on whatever account you
point them at.

## Conventions

The ones that come up in review:

- **Styling is centralized.** Glyphs and geometry are `const`s in
  `src/ui/theme.rs`; every colour is an accessor function of the same name
  reading the active theme's `Palette` (`src/ui/palette.rs`). A new colour is a
  new role on `Palette`, filled in *every* theme table, plus its accessor. Never
  write a literal `Color::Rgb` outside `palette.rs` — a literal is a colour that
  ignores the theme.
- **All width math goes through `cols()`** (display columns via `unicode-width`),
  never `chars().count()`. A wide glyph also owns blank shadow cells, so every
  paint that hands cells to the backend goes through `term::visible_cells`; miss
  one and the bug lives on in that view alone.
- **The input line is a `textarea::TextArea`, not a `String`.** Route editing
  through its methods, read it with `.text()`.
- **Never build a `Value` tree of a body you read a few fields out of, and never
  decode a picture whole to make a small one.** Resident memory is a feature
  here — the process idles in a terminal all day and glibc does not return a
  freed tree's pages to the OS, so a parse that spikes is a parse that *stays*.
  [`docs/memory.md`](docs/memory.md) has the measurements and the shapes that fix
  it; the `tests/*_memory.rs` gates keep them fixed.
- **The app's name is one constant**, `alter_zero::APP_NAME`. Wording ported from
  another tool arrives carrying *that* tool's product name — re-read a ported
  sentence for whose name it says.
- **A tool schema's prose is short, concrete and direct.** Every word rides in
  every request that offers the tool. State the capability and its sharp edges,
  drop the padding.
- **The README is the front door, not the manual.** A feature landing does not
  earn a README section: the mechanism goes in `docs/`, what a user will notice
  goes in `CHANGELOG.md`.

## Commits

One logical change per commit, with a message a reader can act on:

- **A short imperative subject line** saying what the change does — `Blink the
  running tool bullet instead of breathing it between two greys`, not
  `fix bullet`.
- **A body explaining what was wrong and why this is the fix**: the symptom, the
  cause, what the change does, and how it is covered. The history is the record
  of why the code is shaped the way it is; write for someone reading it in a
  year. `git log` is full of examples.
- **No attribution trailers, no tool or assistant names, no session links.**
  The same goes for pull request titles and bodies.

Author and committer should be you. Don't bump the version or edit a released
`CHANGELOG.md` section — `scripts/release.sh prepare` does both when a release is
cut, and CI checks that `Cargo.toml`, `Cargo.lock`, the README badge and the
changelog agree.

## Pull requests

Branch off `main` (the history names topic branches `update/<topic>`). Before you
open the pull request:

- the gate is clean — all four commands;
- the smoke suite passes if you touched anything that shows on screen;
- new behaviour has a test you watched fail first;
- the feature's `docs/` page is written or updated, and `docs/design.md` too if
  behaviour changed;
- `CHANGELOG.md` has an entry under `## [Unreleased]` in the right section
  (Added / Changed / Fixed / Removed). A release's notes on GitHub are generated
  from that section, so what you write there is what users read — describe what
  they will notice, not which function you renamed;
- the diff is your change and nothing else. No drive-by reformatting.

CI runs four jobs on every pull request: the gate, the smoke suite under tmux,
the release tooling (`shellcheck`, `selftest`, and the version/changelog `check`),
and the telemetry collector's tests. The container image has its own workflow,
triggered by changes to `docker/`. A smoke failure uploads every phase's log as
an artifact — start there rather than guessing.

In the description, say what changed, why, and how you tested it. If a review
comment turns out to be a design question, it is cheaper to settle it in an issue
than in a diff.

## Releases

Cutting a release is a maintainer task and is fully scripted: `scripts/release.sh
prepare X.Y.Z`, a commit, an annotated `vX.Y.Z` tag, a push. The tag push runs
`.github/workflows/release.yml` — the gate, one build per platform, checksums,
verification, and a GitHub release whose notes are the changelog's section for
that version. `workflow_dispatch` rehearses the whole pipeline without publishing
anything, and every step runs locally the same way
([`docs/release.md`](docs/release.md)).

## Reporting a security issue

Please don't open a public issue for a vulnerability. Use GitHub's private
vulnerability reporting on the repository (**Security → Report a vulnerability**)
so a fix can ship before the problem is described in public.

An agent that runs commands has a large attack surface by design. The permission
gate ([`docs/permissions.md`](docs/permissions.md)) and the project-config trust
gate ([`docs/project-config.md`](docs/project-config.md)) are where that surface
is decided, so a report against either is worth making even if you are not sure
it is exploitable.

## License

Alter Zero is released under the [Apache License 2.0](LICENSE). By contributing,
you agree that your contribution is licensed under it (Apache-2.0 §5) and that
you have the right to submit it.

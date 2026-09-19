# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo run                                   # run the TUI
cargo test                                  # all unit tests
cargo test <name_substring>                 # run a single test by name fragment
cargo test app::tests                       # run one module's tests
cargo clippy --all-targets -- -D warnings   # lint (warnings are errors here)
cargo fmt --check                           # formatting gate
cargo doc --no-deps --lib                   # intra-doc links must resolve
cargo build && scripts/smoke.sh             # drive the real binary in tmux (every phase, in parallel)
scripts/smoke.sh 55 permission              # only some phases (an id, a range, a name substring)
scripts/smoke.sh --list                     # the phases, their tags and last durations
bash scripts/smoke/phases/055-permission.sh # one phase on its own, output live (docs/smoke.md)
scripts/release.sh check [vX.Y.Z]           # Cargo.toml, Cargo.lock, the README badge and CHANGELOG.md agree (docs/release.md)
scripts/release.sh build [TARGET]           # one platform's release archive + .sha256 into dist/
scripts/release.sh verify dist              # the archives: checksums, layout, CPU, `alter-zero --version`
scripts/release.sh notes X.Y.Z              # the release notes the workflow publishes: CHANGELOG.md's section, nothing else
scripts/release.sh selftest                 # the release tooling's own fixture-driven tests
scripts/release.sh prepare X.Y.Z            # bump the version everywhere, roll [Unreleased] into a dated section, then tag
cargo run --release --example mem_probe     # /model parse RSS (docs/memory.md)
cargo build --release --timings && scripts/build_timings.py   # where a release build's time goes (docs/build-time.md)
DISPLAY=:99 cargo test --test clipboard_linux -- --ignored   # the X11 paste read, under Xvfb
(cd telemetry && node --test)               # the telemetry collector's pure half (docs/telemetry.md)
docker/build.sh [--engine podman] [DIR]     # the headless Kali image from the latest release; DIR also creates the container on it (docs/docker.md)
docker/run.sh [--engine podman] [DIR]       # create the container: DIR at /workspace, ports 8080/8888, --clipboard
python3 -m unittest discover -s docker/tests -p 'test_*.py'   # both scripts under a stub engine: no engine, no network
docker/tests/smoke.sh [--engine podman]     # the built image + run.sh against a real engine
```

The standard pre-commit gate used throughout this project is: `cargo fmt --check`
+ `cargo clippy --all-targets -- -D warnings` + `cargo test` + `cargo doc
--no-deps --lib` all clean. The doc build is part of the gate because the crate
denies warnings, which promotes a broken intra-doc link to an error: a public
item's docs may not link to a private one, so moving an item between modules (or
narrowing its visibility) breaks the links that pointed at it. When that happens
the fix is to qualify the path if the target is still public
(`[`x`]` → `` [`x`](App::x) ``), else demote the link to a plain code span
(`[`x`]` → `` `x` ``) so the prose still names it.

CI runs that gate on every push to `main` and every pull request
(`.github/workflows/ci.yml`: the gate, the smoke suite under tmux, the release
tooling's `selftest` + `check`, and the telemetry collector's `node --test`),
and a `vX.Y.Z` tag push runs `.github/workflows/release.yml` — the same gate,
then one release build per platform (Linux x86_64 and arm64, macOS Intel and
Apple silicon) packaged, checksummed and verified by `scripts/release.sh`, and
a GitHub release whose notes are `CHANGELOG.md`'s section for the version
(`docs/release.md`). Cutting a release is `scripts/release.sh prepare X.Y.Z`,
a commit, an annotated tag and a push; `workflow_dispatch` rehearses the whole
pipeline without publishing, and every step runs locally the same way. Users
install a release with the one-line `install.sh` (`curl … | sh`, POSIX `sh`,
checksum-verified), which the selftest drives against
`scripts/release/release_server.py`, a stand-in for github.com's release pages.

**`docker/`** packages that same published release on headless **Kali
Rolling**, for Docker and Podman alike (`docs/docker.md` the design,
`docker/README.md` + the plain-text `docker/build.txt` the usage). Nothing is
compiled: the Dockerfile runs `install.sh` in a stage of its own and copies
the one binary out. The decision the build script exists for is that
**"latest" is resolved outside the build** — a `latest` asked for inside a
`RUN` is cached on the instruction's text and reinstalls the release it first
saw forever, reporting success, so `docker/build.sh` follows the
`/releases/latest` redirect itself, validates the tag, and passes it as the
`ALTER_ZERO_VERSION` build argument, whose value is what re-keys the layer.
The context is the repository root behind an **allowlist** `/.dockerignore`
(`**` then three `!` lines — those three files and ~40 kB sent, never `.git`,
`target/` or a local `.env`). The image is **root with no user added**, its
tools chosen by
measured size (a dpkg `path-exclude` keeps docs/man/locales from unpacking:
435 → 393 MB), `EXPOSE`s 8080/8888, and idles under `tini` so the container is
somewhere to `exec` into. Kali's `nmap` carries forced file capabilities a
container cannot grant, so it would not even exec — `setcap -r` plus a
`DPkg::Post-Invoke` hook that re-strips after every `apt` run.
`docker/run.sh` owns what a user would otherwise paste: the workspace (a
folder of theirs, created as *them*, or a named volume), the shared
`alter-zero-home` volume at `/root`, the two ports on **loopback**,
`no-new-privileges`, never `--privileged`, anything after `--` handed to the
engine; `build.sh DIR` is build-then-`run.sh DIR`. Two things there are easy
to get wrong. **The `exec` command forwards the terminal's identity** —
`-e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX`, the
variables `ImageStore::detect` actually reads — without which pictures fall to
half-blocks; it is the documentation, printed by `run.sh`, not a wrapper. And
**`--clipboard` forwards the display server, since `exec -it` carries only
keystrokes**: the Wayland socket alone (never the runtime dir) **and** X11
with a cookie rewritten to the wildcard family (the server files it under a
hostname the container does not share) — both, because a compositor with no
data-control protocol serves its clipboard over XWayland and
`clipboard::linux` falls back to it; sources are `--mount`, which fails on a
missing path where `-v` would create a root-owned folder in the runtime dir,
and such a container gets no restart policy. **Telemetry is left alone** —
the image sets neither off switch, and `smoke_image.py` proves it is still on
by launching the real CLI under `--network=none` beside a stub collector, so
no test ping can reach production. The one source change is
`clipboard::require_display`: a session naming no display is refused at once
with the cause and the way around it, instead of whatever arboard's X11 probe
ran into.

Toolchain: Rust **edition 2024**, `ratatui = 0.30.1` (crossterm is re-exported as
`ratatui::crossterm` — import it from there, not as a separate crate), plus
`ratatui-image` for the inline pictures (`default-features = false`: its
default `chafa-dyn` links a C library we don't have, and `image-defaults`
would turn on every `image` codec — see `docs/images.md`), `png` reached
directly for its row-streaming decoder (`images::fitted`, `docs/memory.md`),
and — Linux only — `x11rb` + `wl-clipboard-rs`, arboard's own backends at
arboard's own versions, for the clipboard's `image/png` bytes
(`clipboard::linux`, `docs/image-paste.md`),
`unicode-width` for display-width math, `unicode-segmentation` for the textarea's
grapheme-aware cursor/wrapping, and **`tokio`** (current-thread runtime) +
`tokio-stream` for the async event loop. The `Cargo.toml` `crossterm` entry exists
*only* to enable its `event-stream` feature (for `EventStream`); code still imports
crossterm through `ratatui::crossterm`, never as `crossterm::…`. `rust-toolchain.toml`
pins the toolchain; a `[lints]` table in `Cargo.toml` bakes the gate into every
build (`unsafe_code = "forbid"`, plus `warnings` and `clippy::all` denied).
Dependency features are cut to what the code reaches — `syntect`/`two-face`
load only two-face's prebuilt onig dumps (`parsing` + `regex-onig` /
`syntect-onig`, never the `plist`/`yaml` file loaders `default-onig` carried),
`ratatui` runs without `all-widgets`/`macros` (the calendar widget was the
whole `time` family), `toml` is parse-only — and `[profile.release]` turns on
**incremental** compilation, which is what makes a release rebuild after an
edit ~9–10 s instead of 53 s; `docs/build-time.md` holds the measurements,
what each knob bought, why `lto = "off"` was measured and refused (a faster
cold build for a 7–19% slower render path), and what a release build still
pays for and why (`moxcms` under `image`, the sixel quantiser under
`ratatui-image`, the Wayland and TLS stacks).

## Architecture

A **library** (`src/lib.rs` → `app`, `stream`, `ui`, `term`, `frame`, `paste`,
`session`, `subprocess`, `history`, `textarea`, `file_search`, `clipboard`, `context`, `background`, `scratchpad`, `agents`, `subagents`, `frontmatter`, `ask`, `tasks`, `skills`, `steer`, `mcp`, `trust`, `checkpoint`, `project_doc`, `reminder`, `permission`, `settings`, `telemetry`, `update`, `cli`, `links`, `images`) holds the logic; **`src/main.rs`** is a 77-line shell —
the detached-exec hook, the CLI resolution, the viewport, the loop — over
**`src/tui/`**, the binary-private tree that drives the codex-style **async
(tokio) `select!`** loop (`event_loop`, `actions`, `turn`, `stream`, `agent`,
`background`, `permission`, `view`, `commit`, `models`, `config`, `bootstrap`,
`startup`, `recorder`, `resume`, `history_store`, `settings`, `telemetry`, `update`, `update_cli`, `shell`, `workers`, `host`, `mascot`, `spinner`, `theme`, `donate`, `mcp`, `trust`, `login`,
with the **`Session`** struct itself in `mod.rs` — every handler is an `impl
Session` block in its area module, reaching the private fields the way `app/`'s
submodules reach `App`'s). The four big ones are **directories
of per-area modules**, not single files — `src/app/` (`types`, `action`, `keys`,
`composer`, `commands`, `file_picker`, `input_history`, `queue`, `tools`, `turn`,
`compact`, `backtrack`, `views`, `resume`, `model_picker`, `login`, `settings`, `look`, `mascot`, `spinner`, `theme`, `donate`, `hooks_menu`, `mcp_menu`, `trust_menu`, `background`,
`agent`, `status`, `permission`, with the `App` struct itself in `mod.rs` so every submodule and
the test tree keeps its private-field access), `src/ui/` (`theme`, `wrap`,
`layout`, `assistant`, `inline`, `table`, `message`, `conversation`, `tool`,
`file_cell`, `inline_diff`, `status`, `agent`, `menu`, `footer`, `header`, `hooks_view`, `live`, `transcript`,
`context_view`, `resume_view`, `model_view`, `login_view`, `background_view`,
`permission_view`, `settings_view`, `mascot_view`, `spinner_view`, `palette`, `theme_view`, `donate_view`, `mcp_view`, `trust_view`, `view_flow`, `stream_render`), and **`src/stream/`** — the backend seam
kept apart from the offline demo that used to crowd it: `event` (the whole
`StreamEvent` wire format), `source` (the `ReplySource` trait), `cancel`
(`CancelToken`), `stall` (`StallAi`), and the self-contained **`dummy/`**
subtree (`mod` — `DummyAi` + `turn_events` + the playback pacing, `scenario` —
**the registry that decides which demo a prompt plays**, `script` — the canned
replies, `turns` — the pure `Cue → Vec<StreamEvent>` turns, `gated` — the ones
that block on the permission gate, `agent` — the one that streams a launched
**subagent's own round** on the agent channel, so the agent session view is
drivable offline, `docs/agent-view-streaming.md`); adding a demo is one `SCENARIOS` entry plus
its turn function plus its example prompt in the suite, and the suite proves
every entry is still reachable (the two hand-written `if`/`else` chains it
replaced could retire a demo silently by shadowing its cue). The dummy is what a
first run *meets*, so it behaves like one: each scenario carries its own
two-part reply (split at a blank line — a tool call finalises the text before it
as its own history message, so a mid-paragraph split would break the block)
narrating the cells it is drawing, every user-facing one closes on the shared
`handoff!()` sentence pointing at **`/login`** then **`/model`** (the suite fails
a scenario that doesn't; `smoke.sh` settles on that sentence), and every scripted
call resolves with **the real executor's output** — `tools::format_read`'s
numbered gutter for `Read`, `Wrote …`/`Updated …` via the shared
`tools::write_report`/`update_report` for `Write`/`Edit`, and the
`Exit code: N` frame for `Bash` (streamed body first, framed only at the
`ToolEnd`, exactly as `llm::exec` does) — so the offline cells are numbered,
syntax-highlighted and red-on-failure like the live ones instead of plain text
peeks — see `docs/dummy-backend.md`. The three library `mod.rs`es re-export their areas **by name** — never a glob,
so the public surface is auditable and `tests/api_surface.rs` can lock it — and
every `crate::app::X` / `ui::y(…)` / `stream::Z` path is what it always was; `src/tui/` needs
no facade (nothing outside the binary can name it — `main.rs` reaches exactly
`tui::startup::resolve_cli` and `tui::event_loop::run`);
see `docs/module-layout.md` for the map. The pure, unit-tested logic lives in
`app`/`stream`/`ui`/`textarea`/`file_search`/`session`/`history`/`context` (plus the pure cores of `frame`/`paste`/`subprocess`) so behavior
is testable with a plain `Buffer`/`TestBackend` and no real terminal. `src/tui/`
**and `term.rs`** are the I/O boundary (as is `clipboard.rs`'s Ctrl+V read, the
`/resume` session recording + dir scan — `tui::recorder::SessionRecorder`/`list_sessions`,
whose JSONL format/parse core is the pure `session` module — and the
cross-session input-history file — `tui::history_store::InputHistoryStore`, whose JSONL
format/parse core is the pure `history` module, `docs/history-persistence.md`) — verified via `scripts/smoke.sh`
(a runner over one file per phase under `scripts/smoke/phases/`, each sourcing
`scripts/smoke/lib.sh` for its own tmux server, config home and the
launch/poll/assert vocabulary — `docs/smoke.md`), not
unit-tested save for the odd pure helper that has no terminal in it (like
`term`'s `keyboard_enhancement_disabled` env predicate — see
`docs/shift-enter.md` — or its `visible_cells` cell emitter, which skips the
cells shadowed by a wide emoji/CJK glyph so painted rows never drift — see
`docs/table-streaming.md` *Wide glyphs*); `frame`'s async scheduler **task** is smoke-covered too
(its rate-limit/coalesce math is unit-tested). Keep logic out of the boundary;
the geometry *policy* `term.rs` acts on — live-region height, the box's re-pin,
the cursor seat — comes from pure `ui` helpers it calls (`ui::live_height`,
`ui::repin`, `ui::cursor_position`, `ui::restore_cursor_row`); only the viewport
bookkeeping (`init`'s anchor math, `write_above`'s scroll plan, `resized`'s
re-clamp) is its own, smoke-covered.

The design rationale lives in `docs/design.md`; the async-loop design in
`docs/async-rewrite.md`; the editable input (textarea) design in
`docs/textarea.md`; the Esc-interrupt design in `docs/interrupt.md`; the ↑/↓
input-history recall in `docs/input-history.md`; the Ctrl+R reverse search over
that history in `docs/history-search.md`; the **cross-session persistence** of
that input history (an on-disk `history.jsonl` seeding `App::input_history` at
startup, so ↑/↓ recall *and* Ctrl+R span sessions) in
`docs/history-persistence.md`; the `!` local shell commands in
`docs/shell-command.md`; the `?` shortcuts band in
`docs/shortcuts.md`; the Shift+Enter / Ctrl+J newline keys in
`docs/shift-enter.md`; the mid-turn message queue — and its **delivery into the
running turn** at the next round boundary, main session and subagent session
alike — in `docs/queue.md`; the
session-context footer in `docs/footer.md`; the **startup banner** (the
gradient mascot beside the bold `Alter Zero (v…)` title, the dim cwd, and the
cyan `/login /model /resume` hint — chrome outside `history`, re-emitted atop
every purge rebuild) in `docs/header.md`, and the **`/mascot` picker** that
switches it (the `/settings` family's frame over the six-mascot catalog
with a **live banner preview** rendered by the header's own builder, the
choice persisted **per working directory** in `mascot.json` (`docs/per-directory-state.md`) and the switch's purge rebuild redrawing
the banner at once) in `docs/mascot.md`, and the **`/spinner` picker** that
chooses the status line's **spinner style** (the `/mascot` picker's twin over
a nine-style catalog — `comet` (the list's first row, and the status line's
look before there was a catalog), the braille-track `gravity`
ball (**the default**) and `wave`, `sparkle`, `dots`, `blocks`, `pulse`, `bars`, `line` —
whose page is **live**: every
row wears its own spinner and the highlighted style previews as a whole
sample status line through `ui::styled_status_line`, the strip's own
renderer, animated off the injected frame clock with no turn running
(`App::wants_animation_frames`) and its flow signed on the selection rather
than its rows so a frame never churns a purge rebuild; the choice persisted
**per working directory** in `spinner.json` (`docs/per-directory-state.md`) and seeded at bootstrap, the styles' frames and colour
rules in `ui/theme.rs` — `sparkle`/`blocks` wearing the banner gradient,
`pulse` the tool bullet's breath, the two tracks drawn procedurally on a
braille canvas from whole-millisecond ping-pong/hop curves rather than
tabled — and the switch needing no rebuild, the status line being
live-only) in `docs/spinner.md`, and the **`/theme` picker** that switches
the whole **colour theme** (`docs/theme.md`: every colour the chrome paints
— the accent the pickers select with, the success/error/warning hues, the
dim, the user bubble, the diff tints, the banner gradient — plus the
syntect theme the code blocks and file cells are coloured with, one design
system per entry: the four Catppuccin flavours with **Mocha the default**
(the code had worn it since the syntect port; the chrome now matches),
`onedark` — the TUI's original chrome value for value, over Atom's One Dark
code — Dracula, Nord, Gruvbox, Solarized, Monokai, and `ansi`, the
terminal's own sixteen colours with bat's `ansi` code theme, so the TUI
follows the terminal's theme. The `/spinner` picker's frame over that
catalog, every row wearing a five-`●` swatch of its own palette, the
highlighted theme previewed on **real cells** — a user bubble, an `Edit`
diff cell, a reply with a code line, built by `message_lines`/`tool_lines`
with the theme scoped active around them (`ui::with_theme`) so the preview
and the conversation can never disagree — a still page signed on its rows.
The mechanism: `ui/theme.rs`'s colour `const`s became **accessor functions
of the same name** (`tool_ok_color()` where the `TOOL_OK_COLOR` const was) reading the
**active theme's** `Palette` (`ui/palette.rs`, twenty-one roles per theme),
the active theme a **thread-local** the boundary sets — at bootstrap before
the banner is built, and on a switch (`tui::theme::Session::select_theme`:
save `theme.json`, `ui::activate_theme`, toast, purge rebuild, which is
what recolours every committed row) — a thread-local rather than a global
so every test thread has its own, and rather than a parameter because four
hundred call sites in builders that take no `&App` read it; the three
row caches (`TranscriptCache`, `ContextCache`, the queued-rows memo) key on
it; `highlight::Highlighter::new(lang, code)` takes its `CodeTheme`
explicitly and keeps it for the block; `wrap::lerp_color`/`blend_color`
mix RGB ends and *step* between named ones, which is what lets the ANSI
theme exist) in `docs/theme.md`; the **`/donate` page** (`docs/donate.md`: the
project's crypto donation addresses, one command away — the `/hooks`
browser's sibling, a read-only composer-replacing page with no text entry
and the hidden cursor seated on its `❯`: a red heart over the
banner-gradient `Support Alter Zero` title, a dim two-row blurb, each
address as a numbered `❯ 1. BTC  Bitcoin` row (the ticker and the coin,
never a network — the row stays one glance wide) over its rounded box (the
`/login` device page's), the
highlighted row and box lit in the accent, and **under** each box the dim
`Networks: …` caption naming where that address is reachable — a list, not
a name, because the one EVM address answers on Ethereum, Linea, Base,
Arbitrum, BNB Chain, OP and Polygon alike, and a page naming only the first
would leave the other six to a guess whose wrong answer is unrecoverable
(the label agrees with the count — `Network: Solana` — and the caption
wraps at the box's own inset rather than cutting a chain name); an amber
caution then points at those captions, saying once that a coin goes only
over a network listed under its address, and Enter/`c`/a digit
copies the highlighted address through `/copy`'s clipboard path with a
`Copied the BTC address to clipboard` toast while the page stays open; the
catalog is the const `app::DONATION_ADDRESSES`, never a file, and the
page is still, so it flows signed on its rows) in `docs/donate.md`; the
**`/login` sign-in fork** (the
flow's root now asks *how* you sign in — **Use a subscription** or **Use an
API key** — because GitHub Copilot is not a key you paste. Below that root the
two lists carry **no heading** (each used to repeat the row that opened it, in
cyan, for two rows that said nothing the hint below them doesn't) and every
row states its own reachability — `{name} · ✔ configured` when a key or token
resolves, `{name} · ◯ unconfigured` when none does — only the **`✔` is
coloured** (the `/model` picker's green: the mark is what the eye hunts for
down a column of names, while its word, the `◯` and the ` · ` stay dim, a
status being a fact rather than an alert) — since the absence of a mark is a
poor answer to the one question a sign-in list is opened to ask — while the **key step introduces the provider it is asking
for**: the file's own `description` and a linked `Create a key at
{api_key_url}` (`Install it from …` for a keyless one) between the title and
the field, wrapped never clipped, omitted whole when the file says neither,
which is also what made the field's row stop being a constant —
`login_prompt_row` finds the `❯` in the page the paint just built. `auth =
"github_copilot"` in `providers.toml` puts it in the subscription list, where
Enter runs GitHub's **device flow** on the loop's fourth worker (the one that
runs for *minutes*) and the page shows the one-time code in a rounded box over
the URL to enter it at — no browser is launched, `c` copies the code through
`/copy`'s own clipboard path, the `expires in 14:11` countdown is a boundary
clock read injected per draw and the cursor hides (a wait, not a field), and a
failure stays *on* the page in red rather than closing the explanation away
with itself. Both halves end in the **same** `.env` store under the provider's
`api_key_env`, which is what makes the fork cheap — `/model`, the ✓ marks, the
capability probe and the next launch need no second mechanism. What is stored
is the long-lived GitHub **OAuth** token; what the API takes is a ~30-minute
**bearer**, exchanged from it (and cached in memory, never written) by the one
`copilot::request_auth` seam both `stream_chat` and `fetch_models` resolve
through — `AuthScheme::ApiKey` answering with the stored key and **no I/O at
all**, so every existing provider's path is byte-identical, and the exchange
additionally naming the account's own host, since a Business seat is served
from one the file cannot know. Its cached life comes from `refresh_in`, never
`expires_at`: a clock running ahead makes the latter already past and
re-exchanges on every request. Copilot's `/models` then feeds the three
existing per-record sniffs a branch each — the prompt cap
(`max_prompt_tokens`, which Copilot sets *below* the nominal window and
actually enforces) as the context window, `supports.vision`, and
`supports.reasoning_effort`, which is the **Ctrl+T ladder itself** rather than
the hardcoded guess every other provider gets — while `entry_of` drops the
records this client cannot call at all (embeddings, and the `/responses`-only
reasoning family). On the wire the mode is a top-level `reasoning_effort`
string *instead of* the `reasoning` object, whose unknown-field 400 blames the
model, and three per-request headers ride along: `X-Initiator` (billing —
GitHub charges the user's round and not the agent's tool loop),
`Copilot-Vision-Request` with an image, and `X-Request-Id`) in
`docs/copilot.md`; the **OpenAI ChatGPT sign-in** — the subscription list's
second row, and the two things a second subscription turned out to need
(`docs/chatgpt.md`): a second *sign-in shape* and a second *wire format*.
`auth = "chatgpt_codex"` runs OpenAI's **browser PKCE loopback** on the same
worker Copilot's device flow uses and reports on the same two messages,
because the two pages are the same page — something to show, then a wait; what
differs rides `SigninKind` on the row, injected from the provider's `auth`
scheme rather than guessed from what the flow has filled in yet, so the
browser page gives the URL its own row **bare** over `Sign in there — this
window continues by itself`, shows no code box, and `c` copies the **link** (a
URL far too long to retype, where a `c` bound to a code that does not exist
would be dead on the one page that most needs it). Both pages' URLs are real
**OSC 8 hyperlinks** (`ui::model_view`'s `model_linked_rows`, `docs/links.md`)
— stamped on the *unwrapped* text so every hard-broken fragment opens the whole
target, which is why the browser page needs no verb in front of its link, and
keeping the caller's colour so the device page's deliberately dim URL gains an
underline rather than the chat link dress. The listener binds `127.0.0.1:1455` (falling
back to `1457`) while the redirect URI names `localhost`: OpenAI's allow-list
is pinned to those two ports against Codex's client id, so a port of our own
is refused at the authorize step — and binding `"localhost"` can resolve to
`::1` and miss the browser entirely. A headless machine has no browser to
hand that link to, so the row offers **two** flows: Enter on `ChatGPT
Codex` opens a titled two-row **sign-in method choice** first
(`KeyStep::SigninMethod` — `Select ChatGPT Codex login method:` over
`Browser login (default)` / `Device code login (headless)`, the root's own
row dress and hint, no `❯` filter and a hidden cursor since two rows are a
question rather than a list, Esc from *either* page returning to it with the
row just tried highlighted, codex's own "on a headless machine press Esc and
choose the device code"), whose device row runs **Codex's device-code flow**
(`chatgpt::request_device_code` / `await_device_approval`,
`docs/chatgpt.md`): `POST {issuer}/api/accounts/deviceauth/usercode` for a
code (its `interval` arriving as a *string*, floored at a second, five when
absent), the Copilot device page showing it beside `{issuer}/codex/device`
with a fifteen-minute countdown, a poll of `/deviceauth/token` that reads
`403` **and** `404` as pending, and the grant's **server-minted** PKCE
verifier redeemed at the shared `/oauth/token` with `redirect_uri =
{issuer}/deviceauth/callback` — no port bound, no `state`, the same
`exchange_code` the browser flow ends in, so everything after the token set
is byte-identical between the two. Which pages a row can open is the
scheme's to say (`tui::config::signin_kinds` → `SubscriptionChoice::kinds`,
the first the default; a row listing one opens it at once, Copilot's way),
`Action::StartDeviceLogin { provider, kind }` carries the pick to the
worker, and `ALTER_ZERO_OPENAI_ISSUER` (read at the boundary, handed in once
through `chatgpt::set_issuer`) points both flows at `smoke.sh` Phase 119's
local stub, which answers codex's own wire shapes and reads the exchange
back off its log. What is stored is the **refresh** token,
and it **rotates**: OpenAI may retire the one just used, and re-presenting a
retired one is terminal, so `chatgpt::persist_refresh` writes the new value
straight back into the `.env` store (through a path `tui::models` hands the
module once at startup, since the mint runs several layers below the
boundary — skipping that call is not a crash but a forced re-login at the next
launch). The `chatgpt-account-id` the backend routes on is read out of the
minted access token's own claims, which is what keeps the store to one value
with nothing to drift; the cached life comes from that token's `exp` with a
five-minute skew **and a sixty-second floor**, the floor standing in for the
`refresh_in` duration Copilot has and OpenAI does not (a clock hours ahead
then costs one extra mint a minute instead of one per request). Copilot's
`(bearer, base)` seam widened into `auth::request_auth`'s `RequestAuth`
{bearer, base, headers} to carry that account header — `AuthScheme::ApiKey`
still answering with the stored key, no override and **no I/O at all**. The
wire format is a **separate** provider key (`wire_api = "responses"`), not a
consequence of `auth`: how you authenticate and what shape the request takes
are two questions, and an API key can reach the Responses API too.
`src/llm/responses.rs` translates both directions between it and the Chat
Completions currency the rest of the crate uses, so the agent loop, the
transcript, the rollout and the derived context are untouched — `messages`
becomes an `input` array of typed items (a tool call is a `function_call` item
of its own, its result a `function_call_output` linked by `call_id`, and there
is no `role: "tool"`), the system prompt is hoisted into the **required**
top-level `instructions`, parts are `input_text`/`input_image` (the image URL
being the *value*, not a nested object), `store: false` is mandatory, an empty
assistant `content` array is a 400 where chat completions tolerates `""`, and
`Off` sends no `reasoning` field at all since this API has no `enabled:
false`. `drain_responses` is `drain_stream`'s sibling over the shared
`pump_lines` byte loop, so Esc is honoured identically either way; its one
structural difference is that a tool call arrives **whole** on a single
`response.output_item.done` frame rather than as accumulated fragments (the
fragments that do stream are surfaced for the token tally only). Its
`/models?client_version=…` listing — the query is required, not defaulted —
answers `{"models": […]}` with `slug`, `context_window` ×
`effective_context_window_percent` (the share the backend enforces, Copilot's
`max_prompt_tokens` rule), `input_modalities`, and a
`supported_reasoning_levels` ladder that is the Ctrl+T cycle itself, `ultra`
included — a rung above `max` that no other provider names and that
`ReasoningEffort` gained for it. Encrypted reasoning deliberately does **not**
round-trip: carrying it would mean a new `ChatMessage` field threaded through
`context`, the rollout and the transcript, so the model re-reasons each round
— a quality cost, not an error) in `docs/chatgpt.md`, and **fast mode**
— codex's `/fast`, `docs/fast-mode.md`: a model whose ChatGPT record lists
a `priority` service tier (`service_tiers: [{id, name, description}]`,
every model on the account today, `Fast — 1.5x speed, increased usage`) is
switched to priority processing by **a command per listed tier** — the
palette builds `/fast`, `/ultrafast`, whatever the record names, out of
the model's own `service_tiers` (`App::commands`: the static `COMMANDS`
registry with one `SlashCommand::tier` row per tier spliced in right after
`/model`, codex's `SlashCommandItem::ServiceTier`; named by
`ServiceTier::command_name`, the lowercased name slugged to one token,
described by the record's own cost statement, carrying the tier as
`CommandEffect::ServiceTier`, no row for a tier listing nothing usable or
shadowing a built-in), so nothing about a tier is hardcoded and a tier the
backend adds tomorrow is a command the day it is listed; each command
toggles its tier (`SpeedState::toggle`, codex's
`toggle_service_tier_from_ui`: select it, the same command again for
standard, another tier's command switching straight over), with a `Speed:
fast — 1.5x speed, increased usage` toast repeating the backend's own cost
statement, and a model listing no tier lists no such command at all (an
unmatched `/fast` is swallowed like any other miss) — the choice riding both OpenAI wires as the top-level
`service_tier` beside codex's `x-codex-routing-hint:
model={model}[;tier={tier}]` header (sent on every ChatGPT request, tier or
not), shown after the thinking mode in the footer (`gpt-5.5 medium fast ·
~/repo`), persisted as `config.json`'s `speed` blob beside the `thinking`
one (the empty blob marking a model known to list none, an absent one
probed for like an unknown thinking state) and carried across a `/model`
switch to a model listing the same tier — while a pinned subagent model
(`with_model`) and the auto-mode classifier run standard, since a tier the
model does not list is a request the backend may refuse; verified live
(`tests/live_caching.rs`); the **Anthropic
provider** — Claude reached two ways over one **third wire format**
(`wire_api = "anthropic"`, `src/llm/anthropic.rs`, `docs/claude.md`): a
pasted Console key and an **account sign-in**, `auth = "anthropic_console"`,
whose PKCE loopback runs on the same `/login` browser page ChatGPT's does and
whose constants come from Anthropic's own published CLI. It is deliberately
**not** the Claude Pro/Max subscription sign-in: Anthropic's usage policy does
not permit a third-party client to offer Claude.ai login or route requests
through subscription credentials, and making that one work would mean
impersonating Claude Code down to a mandated *"You are Claude Code…"* first
system block — so `no_client_identity_is_ever_injected_into_the_system_prompt`
pins that the system array is the user's own prompt and nothing else. The
credential's *placement* is what the seam gained: this is the one wire format
whose API key is not a bearer (`x-api-key`, with `Authorization` reserved for
OAuth and both together refused), decided from the **(scheme, wire format)
pair** since an Anthropic key on a chat-completions shim is still a bearer —
while an OAuth bearer additionally carries the `anthropic-beta:
oauth-2025-04-20` a pasted key must not send. The two OAuth grants disagree
about everything (form-encoded code exchange with *no* beta header; JSON
refresh *requiring* one), refresh tokens rotate through the same write-back
`docs/chatgpt.md` describes, and a sign-in granting no `user:inference` scope
is refused **at the sign-in** rather than at the first turn. The translation
is `responses.rs`'s contract over a stateful round: the system prompt hoists
out of `messages` whole, a tool result is a `tool_result` block in a **user**
message with a batch's results merged into **one** (splitting them teaches the
model to stop calling tools in parallel), a tool call's arguments arrive as
`input_json_delta` **partial JSON** keyed by content-block index (a parallel
batch interleaves them), `max_tokens` is mandatory, `temperature` is never
sent (removed from every current model, where it is a 400), and usage is
**summed** — `input_tokens` is the *uncached remainder*, so reading it alone
reports a few dozen tokens against a 1M window. `capabilities.effort` is the
**Ctrl+T ladder itself** (the third provider to name one natively) beside
`capabilities.image_input` and `max_input_tokens` — the window, not the
`max_tokens` output cap sitting next to it — and the listing asks
`limit=1000` because its default page of 20 truncates the catalog with no
error anywhere) in `docs/claude.md`; the **Ollama provider** — local models
over a **fourth wire format** (`wire_api = "ollama"`, `src/llm/ollama.rs`,
`docs/ollama.md`), Ollama's *native* `/api/chat` rather than its
OpenAI-compatible `/v1`, for the one thing `/v1` cannot carry:
`options.num_ctx`. Ollama loads a model at a 4096-token context on most
machines and truncates a longer prompt **silently** from the front, so the
window the footer gauges against is the window the request asks for —
`ModelConfig::context`, filled from the same `context_window()` the gauge
reads, the startup probe rebinding on this wire alone once the listing
names it; the per-model rule is the Modelfile's own `num_ctx` (only
`/api/show` carries it, which is why the catalog shows every model), else
`OLLAMA_CONTEXT_LENGTH`, else the 32K `DEFAULT_NUM_CTX_CAP`, never past the
model's maximum, the cloud uncapped, `ALTER_ZERO_CONTEXT_WINDOW` outranking
all of it. No key is needed (`auth = "optional_key"`, the one scheme
`is_usable` accepts keyless; a stored `OLLAMA_API_KEY` still rides as a
bearer for the cloud or a proxy), so the provider is **configured by being
pointed at** — `OLLAMA_HOST` resolving (`api_base_env`, Ollama's own grammar
ported as `ollama::host_url`), a key, or `ALTER_ZERO_PROVIDER` — which is
what keeps every `/model` open from fetching a server most users don't run;
`/login`'s row for it is a **host field** (`KeyKind::Host`: shown as typed,
an empty Enter saving the default) rather than a masked secret. The catalog
is `/api/tags` plus one `/api/show` per model, answering all three
capability questions natively — `vision`/`thinking` off the explicit
`capabilities` list (absence is *no*; an embedding-only model is not
offered), the window as above; `thinking` makes an on/off reasoner
(`think: true/false`, on by default — the seeded mode says so) except the
gpt-oss family, whose thinking is a `low/medium/high` **level** with no Off.
The stream is NDJSON over the shared `pump_lines`, a tool call landing
**whole** with **object** arguments (and sent back as one — a string is a
400), an image riding as bare base64 in `images`, a tool result naming its
tool, usage the final frame's `prompt_eval_count`/`eval_count` (verified to
report the whole prompt on a cache hit), and the machine-shaped refusals —
server not running, model not pulled, `does not support tools/thinking`, an
image on a blind model — rewritten into what to do. `tests/live_ollama.rs`
drives it against a real server; `smoke.sh` unsets `OLLAMA_HOST` beside its
key scrub for hermeticity; the **Ctrl+T thinking-mode
cycle** (a reasoning-capable model's effort — detected per model from the
provider's `/v1/models`, shown beside the model name in the footer, cycled
with a `Thinking: {mode}` toast, riding the request as the unified `reasoning`
parameter, persisted beside the `/model` selection) in `docs/reasoning.md`;
the **thinking stream** — that reasoning, *shown* (a phase's
chain-of-thought streams live in the strip wearing the **tool cell's shape**:
a `● Thinking…` header — the same `TOOL_BULLET` a running tool wears, because
it means the same thing, breathing at the frame pulse beside a label carrying
the status line's **shimmer** (`ui::status::shimmer_spans_from`, the same wave
`Working…` wears but floored at the near-white `reasoning_shimmer_base()`, since
codex's grey base is right for a metric and unreadable for a header) — over the
thought in the `⎿` gutter, dim and **italic** (the one cue separating it from a
tool's output there), tail-following its last `REASONING_PEEK_LINES` wrapped
rows; at the phase's end the cell shape goes away entirely and it **collapses**
into one committed **bullet-less** two-tone
`Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)` line — the `summary_lines`
shape, because nothing is happening any more and what is left is a fact about
the turn — **dim throughout** (`reasoning_label_color()` = `status_done_color()`,
`Done for Ns`'s exact dress, so the pair bracketing a turn reads as a pair);
the text itself never reaches immutable scrollback (which is *why* it can
collapse) but expands in Ctrl+O under that same dim line, minus the hint since
the expansion has none to make room for; `HistoryItem::Reasoning` records it, the rollout keeps it
across a `/resume`, `context::context_messages` **skips** it (a Chat
Completions request has nowhere to put a previous round's chain-of-thought,
so Ctrl+D shows no trace either), the settle points are `ThinkingEnd`/Esc/a
backend error via the one `tui::stream::Session::settle_reasoning`, the
**block goes up over a finalised segment** — `ThinkingStart` flushes the run
of assistant text before it (invariant 4's "flush before you interleave",
gated on the display), so the header sits under the same blank spacer the
settled cell will instead of butting against the paragraph that was
streaming, and the paragraph's withheld last line lands with it — and the
cell's tokenizer estimate is **snapped** to the provider's own
`completion_tokens_details.reasoning_tokens` — `TokenUsage::reasoning` — when
the round's usage frame lands, split by weight across a round's several
phases; gated by `ALTER_ZERO_SHOW_THINKING`, whose falsy value restores the
old counted-and-dropped behaviour exactly) in `docs/thinking-stream.md`;
the **running bullet's pulse** (a tool in flight no longer
shows a blue `●` — it shows the permission prompt's grey, and in the live
region that grey *breathes* dim→bright→dim once a second, Claude-Code's
running dot: a raised cosine over the boundary-injected `App::set_pulse`
frame clock (one shared phase, so a mixed round's tool cells and its agent
tree blink in step and a background agent animates between turns), applied
by `ui::live_tool_lines` in the strip only — `tool_lines` renders at rest so
a scrollback commit can never freeze a frame mid-breath, and the Ctrl+O
transcript stays still to keep its cache's signature clock-free) in
`docs/tool-pulse.md`; the flicker-free frame pipeline
(scrollback commits deferred into the draw's synchronized update) in
`docs/flicker.md`; the **clickable OSC 8 links** (every URL an assistant
reply shows — a bare URL in prose/lists/table cells, inline/fenced/indented
code, a heading, or a `[text](url)` target — is painted inside an OSC 8
hyperlink carrying the whole URL, so a wrapped URL's every fragment opens the
full target instead of the truncated row text the terminal's own detection
saw — and a `● Read/Write/Edit({path})` header's
path is a link to the **file**, `links::file_url`'s absolute `file://` URI
behind whatever short form the row shows, stamped before the wrap so any
fragment opens the whole file, while the `(`/`)` and the `⎿ Wrote … to
{path}` corner row beneath stay plain — the rule a `[text](url)` suffix's own
` (`/`)` follow too, keeping the prose dress rather than the target's blue +
underline, so the styled run and the clickable run are one run: the pure
`links` module detects/interns
and stamps the id into `Style::underline_color`, `ui::inline` marks prose and
code (a code URL gaining only the invisible target, never the link dress),
`ui::tool`'s header stamps the file target, and
`term::draw_cells` — the choke point all four cell-write paths share — strips
the carrier and brackets marked runs, `ALTER_ZERO_HYPERLINKS` gating emission)
in `docs/links.md`; the `@` file-path picker (async walk+rank file search below
the box) in `docs/file-search.md`; the large-paste `[Pasted Content N chars]`
placeholder (bracketed paste → a compact placeholder, expanded back on send) in
`docs/paste.md`; the **Ctrl+V image paste** (clipboard image → the session's
paste folder, `{config_home}/image-cache/{session}/N.{ext}` numbered in paste
order, staged and header-checked before it takes its number → an `[Image #N]`
composer placeholder whose path rides a separate typed channel to the backend
and reaches the model as `[Image #N: {path}]` in the message text — stamped by
`context::context_messages`, so Ctrl+D shows exactly what the wire carries —
so it knows where the picture was saved and a `/resume` finds it again (the
store bounded by
**size** and not age, oldest session folders evicted past
`IMAGE_CACHE_MAX_BYTES`, since a rollout is kept indefinitely and a calendar
rule would break a conversation still worth resuming) — on Linux the
owner's own encoded bytes are **streamed** into that folder by
`clipboard::linux` *before arboard is constructed*: `image/png`
first, then `jpeg`/`gif`/`webp`, a Wayland pipe or 1 MiB X11 property slices
with `INCR` segments, capped at `CLIPBOARD_IMAGE_MAX_BYTES` and never
decoded, since arboard's decode-to-RGBA plus our re-encode was a ~24 MB
spike per paste and the block that taught glibc to keep the next one,
`docs/memory.md`) in `docs/image-paste.md`; the **inline images** (`docs/images.md`:
a pasted screenshot and the `read` tool's image reads drawn as **real
pictures** in the conversation — kitty / iTerm2 / sixel where the terminal
speaks one, unicode half-blocks everywhere else, via **`ratatui-image`** —
flush at the left margin under the cell that produced them, one blank row
apart, in the terminal's real scrollback **and** in the Ctrl+O transcript.
The split is the crate's usual one and is what makes it cheap: pure `ui`
**reserves rows** (a block is `rows` ordinary `Line`s of `cols` spaces, each
cell carrying a marker in its `underline_color` — so a picture rides every
path a `Vec<Line>` already rides) and the boundary **draws into them**
(`images::store::ImageStore::stamp`, run in the four paint paths —
`write_above_chunk`, `paint_live`, `paint_reflow`, `draw_overlay` — the
`visible_cells` rule, since a path that skips it shows blank rows in that
view alone). The carrier shares [`links`]' 24-bit `underline_color` channel
by **splitting** it rather than sharing it — bit 23 set means an image, and
`LINK_ID_MAX` dropped to `0x7F_FFFF` — so a URL can never decode as a
picture; below the flag it packs `(placement id, row index)`, and the row
index is what lets the stamp find a block's top-left corner. Placements
intern on `(path, cols, rows)`, which is what makes a **resize** correct: a
narrower terminal is a different id, so the boundary re-encodes instead of
re-placing the old size (every resize purge-rebuilds from history anyway,
which is what re-measures the picture — and the purge drops the encoded
protocols, since a kitty placement transmits its pixels once and one that
outlived the `ESC[3J` would place an image the terminal may have dropped).
The key is deliberately **not** the file's bytes, so an encoding remembers
the **file state** it was made from — the `(len, mtime)` the payload sidecar
and the wire attachment already key on — and a hit is a hit only while the
file still has it: the agent turning the cat it just showed black and white
*in place* and reading it again reserves the same placement, which used to
be served the colour cat from the cache while Ctrl+O, encoding the
alternate screen's copy fresh, showed the conversion
(`ImageStore::encode` stats the path per paint — microseconds — a gone file
keeping its picture rather than retrying per frame; `docs/images.md` *A
file rewritten in place*, `smoke.sh` Phase 107d).
**A screen switch is not the same thing and it is not optional**: kitty (and
Ghostty) keeps a *separate image store per screen buffer* — its own spec says
the alternate screen's images are cleared on the 1049 switch — so a protocol
carried across the hop painted the Ctrl+O transcript with placeholders naming
an image that screen's store had never heard of, failing **silently** (the
lookup returns, `q=2` suppresses replies) as reserved rows with nothing in
them. So an encoding **belongs to a screen** and, once made, keeps: the cache is
keyed `(placement, screen)`, `enter_overlay`/`exit_overlay` call
`enter_screen`, and each screen uploads a given picture **once, ever**. That
the alternate screen's copy survives the switch is verified, not assumed —
kitty's clear filter opens `if (ref->is_virtual_ref) return false;`, the
image behind it is not collected because that virtual ref counts as a ref,
and the spec exempts virtual placements from the very deletion classes the
switch and our `ESC [ 2 J` use (kitty ≥ 0.28, Ghostty ≥ 1.1). It matters
because an upload is not cheap: `ratatui_image` transmits kitty images as raw
RGBA, so a 120×35-cell picture measures **~4.5 MB of base64** — and one
Ctrl+O toggle went 4.53 MB / 4.53 MB / 4.53 MB (open/close/reopen) to
4.53 MB / 1.3 KB / **19.6 KB**. `ALTER_ZERO_IMAGE_RETRANSMIT=1` takes the
conservative path for a terminal that speaks the protocol but not that part
of it; we cannot ask it, since the reply would need a second stdin reader.
Only kitty pays any of this — sixel, iTerm2 and half-blocks keep nothing per
screen and share the primary's entry. `smoke.sh` Phase 107b guards all three
legs off the **raw byte stream** (`pipe-pane`, not the pane text, which cannot
tell the cases apart — a placeholder *is* an ordinary cell).
And a block is **drawn into exactly the reserved cells the frame holds** —
their extent read back off the carriers, never taken from the placement — via
`ratatui_image`'s `SlicedProtocol`/`SlicedImage` at the row offset the first
visible carrier names. Two bugs live in that one sentence. The Ctrl+O pager
opens pinned to the **bottom**, so a picture taller than its body starts
*above* the window and its first visible row is not row 0: rendering only
from a head row drew nothing at all on the most ordinary open there is
(`smoke.sh` Phase 107c — 0 rows before, 10 after). And clamping to the
*buffer* rather than to the reserved run let a tall picture paint over the
pager's own separator and key-hint rows. Slicing is also what gives sixel and
iTerm2 clipping at all: they decline to draw an image larger than its area,
so the sliced form strips sixel bands and cuts iTerm2 into one protocol per
row.
The geometry is `image_budget` then `fit_cells`, both pure: the **Image
width** cap clamped to the terminal less a two-column gutter, then a row cap
that is that width *as a square pixel box* (`⌈max_cols × cell_w / cell_h⌉` —
without it a 600×4000 portrait screenshot spends the whole width budget on
its width and takes four hundred rows to match), then `ratatui_image`'s own
`Resize::Fit` — proportional and **shrink-only**, so a 32×32 icon stays a
handful of cells instead of being blown up blurry to fill 120 columns.
`fit_cells` deliberately *reproduces* the encoder's arithmetic rather than
inventing its own, because a row of disagreement is a blank gap under every
picture; a differential test pins the two together against the real
`Resize::Fit`. The pixel size comes from the `read` tool's own fact line
(`images::read_image_size` — the only record that survives a `/resume`,
since the rollout keeps the cell's text and not the file's header) or, for a
paste, from the header the boundary read when the paste landed. Terminal
detection **never reads stdin**: `ratatui_image`'s `Picker::from_query_stdio`
spawns a reader thread behind a 2 s timeout and never joins it, so on a
terminal that doesn't answer that thread eats the user's keystrokes
(observed under tmux — a 2 s stall and then every key swallowed), which is
invariant 1, so the cell size comes from `TIOCGWINSZ` and the protocol from
the environment, falling to half-blocks;
`ALTER_ZERO_IMAGE_PROTOCOL`/`ALTER_ZERO_IMAGE_CELL_SIZE` override both and
`ALTER_ZERO_IMAGES` gates it. The encoded pictures are bounded in **bytes**
(a kitty placement is the whole picture as base64 RGBA), estimated from the
placement's own geometry, since this process idles in a terminal all day
(`docs/memory.md`) — and a PNG is **decoded at its fitted size**:
`images::fitted` streams the file's rows through an area-average shrink into
the reserved block (or the model's 2000-pixel cap), so the decode peak is the
block's ~3 MB rather than the file's 8–33 MB, the whole-picture decode having
been the residue three pasted screenshots left behind as a 103 MB process;
anything else decodes whole — refused past `WHOLE_DECODE_MAX_PIXELS` — and
`thumbnail_exact`s into the box with no `f32` pass. The model payload rides
the same fit and is **cached on disk per session** (`{session}/images/`,
`images::payload` — keyed on path, size, mtime and cap; `cached_downscale`
serves a re-sent attachment without opening the original), **and encoded
once per session** (`images::attachment` — the base64 `data:` URL streamed
from the sidecar or the file into the one string the session keeps, handed
to every later request by reference as `AttachmentUrl`, bounded at
`ATTACHMENT_CACHE_MAX_BYTES`, swept at each turn start to the pictures the
context still carries; the request is then serialized **from the
messages by reference, straight into the upload** on every wire —
`openai::ChatRequest`, and the Messages/Responses/`/api/chat` translations'
typed borrowed requests, each a `llm::body::BodySource` written by a
serializer thread into a bounded pipe of 64 KB chunks that the transport
pumps (`streamed_request`, the `Content-Length` from a counting pass), never
as a `Value` tree of the conversation and never as a whole-body buffer, the
cache breakpoints marking a shallow typed copy —
because every turn used to re-read the paste, re-encode it, copy it into a
tree and grow the body by doubling, five picture-sized blocks a round that
glibc's arenas kept, +14 MB whenever a turn landed on a new one, and even
one exactly-sized body per round still left a body's worth per arena: the
reported RAM-grows-per-message bug, `docs/memory.md`), and
`tests/image_paste_memory.rs` gates all three stages' resident growth while
`tests/image_turn_memory.rs` gates the turns after the paste. Three `/settings` rows: **Show images** and **Image
width** (60/80/120, a *cap*) republish the policy and purge-rebuild so
committed pictures change at once, while **Auto-resize images** is a
different kind of thing entirely — the *payload*, not the screen: a
12-megapixel photo is megabytes of base64 a provider refuses or bills in
full and a model reads no better than the same picture at 2000 pixels, so
the two paths that upload pixels downscale first (a JPEG stays a JPEG, since
a photo as PNG *grows*), leaving the file — and so the picture on screen —
untouched, which is why an auto-resized read's fact line leads with the
file's own dimensions and names the sent ones after; `smoke.sh` Phase 107);
the **Esc-Esc backtrack** (edit a
previous user message: prime → transcript preview → rewind + prefill) in
`docs/backtrack.md`; the **`/resume` session picker** (every conversation
recorded to a rollout JSONL file, listed in a full-screen picker whose Enter
loads it back and appends the turns that follow to the same file — and its
**CLI twins**: `--continue` reopens the newest session recorded in this cwd,
`--resume {id}` one by id (bare `--resume` boots into the picker), the flags
resolved to a rollout path in `main()` *after* the detached-exec hook and
*before* the terminal boots (fail-fast on stderr, the pure parse in `cli`,
the id lookup via `session::rollout_file_id`/`latest_for_cwd`) — plus the
**`[PROMPT]` shortcut**: one quoted positional (`alter-zero "fix the failing
test"`, `alter-zero --resume {id} "and now the docs"`, `-c "…"`) submitted
as the first turn of whichever session those open, through
`Session::submit_startup_prompt` — the Enter path minus the composer, so it
records into ↑ recall and `start_turn`s like a typed message; strict (a
second positional is a usage error carrying a quote hint, never a join that
would read a stray `-c` as `--continue`; a blank prompt and a prompt behind
the bare picker are refused; `--` ends the flags; `--resume` still takes the
next token as its id greedily, Claude Code's rule) — and the **`--help`
page**: `Alter Zero` (`APP_NAME`) on the first line in the app's bold-cyan
heading style over a one-line description, then clap's shape — an inline
`Usage:` with continuation lines, `Commands:` / `Arguments:` / `Options:`
rows aligned on one page-wide column, literals bold, placeholders bare —
from one `cli::help(page, style)` renderer that `mcp --help` shares
(`HelpPage::Mcp`), `HelpStyle::Plain` on a pipe, under `NO_COLOR` or
`TERM=dumb` (`cli::colour_enabled`, ANDed with `IsTerminal` at the boundary),
and a grammar error the clap-shaped `error: {message}` + `Usage:` block +
`For more information, try '--help'.` (`cli::usage_error`, stderr, exit 2)
instead of the old whole-page dump — the loaded
transcript committed under the banner through `insert_before` — never a
startup Purge, the user's terminal scrollback survives — and a quit that
recorded anything printing `Resume this session with: {bin} --resume {id}`
after `term.restore()`, `docs/cli.md` — and the **`mcp` subcommand family**,
`docs/mcp-cli.md`: `alter-zero mcp add {name} --url {url}` / `mcp add {name}
[--] {command} [args…]` (plus `add-json`/`remove`/`get`/`list`) edits the
*user* MCP config file at that same pre-TUI boundary — codex's explicit
url-or-command grammar with Claude Code's `--transport sse`/`--header`/
duplicate-error ergonomics on top, a bare `--url` writing the type-less
http→sse-fallback shape, the pure grammar in `cli::parse_mcp` → `McpCli`,
the RMW writers `mcp::record_server`/`remove_server` (order-preserving
`shift_remove`, unparseable files refused never clobbered), the file I/O in
`tui::mcp_cli`) in
`docs/resume.md`; the **filesystem checkpoints** (every turn snapshots the whole
cwd into an *isolated* git store — never the user's real `.git` — keyed to the
conversation length, so the Esc-Esc backtrack **and** `/resume` **reset the
code**, not just the transcript: rewinding restores the working directory to the
`checkpoint::restore_target` at that point, backing up the current tree first;
the pure mapping/format is `checkpoint` + `session::parse_checkpoints`, the git
I/O is `checkpoint::CheckpointStore`, turn-end snapshots ride
`dispatch_after_turn`, and restores hang off the `ResumeSession` /
`ConfirmBacktrack` arms; gated by the per-directory `/settings` **Checkpoints**
knob — **off until a directory turns it on**, `ALTER_ZERO_CHECKPOINTS` seeding
it for a run (`docs/per-directory-state.md`) — **and by the cwd
being worth snapshotting at all** — because the session-start snapshot's
whole-cwd `git add -A` runs *before the first frame paints*, in raw mode where
Ctrl+C is an unread key event, and hashing is O(bytes): a 235 MB cwd measured
**9.7 s** to first frame and a 260 MB store. Two guards decide, both pure with
the paths injected at the boundary. The **categorical**
`checkpoint::cwd_scope` refuses a filesystem root, the home dir or an ancestor
of it (the original "hangs in `~`" bug), **alter-zero's own state dir and
everything under it** (it holds the rollouts, the input history and every
project's store, so a restore's `git clean -fd` there deletes other sessions'
records), every `SYSTEM_TREES` entry **and its subtree** (`/proc`, `/sys`,
`/dev`, `/run` — not ordinary filesystems at all, and `/dev/shm` holds other
*live* processes' files), and each `SYSTEM_ROOTS` (`/usr`, `/etc`, `/root`, …)
and `SHARED_PARENTS` (plus `$TMPDIR`) entry **itself only** — so `/tmp` is
refused as every program's scratch space while `/tmp/my-project`,
`/usr/local/src/thing` and `/etc/nginx` checkpoint exactly as before, which is
also what the smoke suite's `mktemp -d` work dirs rely on. The *other*
direction of the self-inclusion bug — a store root **inside** the cwd — is
not a refusal but a fix: `checkpoint::store_exclude_line` appends an anchored,
metacharacter-escaped `info/exclude` line so the store never re-stages its own
objects (without it the tracked set compounds 59→130→269→528 over four turns,
41 MB→165 MB — the reported 2 GB `.alter-zero`; `CHECKPOINT_EXCLUDES` can't
cover it, being *name* patterns against a root the user chooses), which also
means an `ALTER_ZERO_CHECKPOINTS_DIR` pointed into a project costs that
project nothing. The **general**
`checkpoint::SnapshotBudget` then catches the huge directory no denylist can
name: `CheckpointStore::probe` asks `git ls-files --others --exclude-standard`
for precisely the paths `git add -A` would stage — honouring `.gitignore`, so
a repo whose bulk is ignored is never falsely refused (a walk of our own would
refuse a Next.js `.next/` or a gitignored `dist/` for weight git never carries)
— sums their `stat` sizes, and bails the moment a cap trips (26 ms against the
20 s it predicts, because git walks and stats but never reads content; a warm
store reports only what is new, which is exactly what it will hash, so a
project that grew past the cap over months keeps checkpointing). Past the
default 20 000 files / 256 MiB — a claim about what a project *is*, not a time,
since hashing throughput varies ~20× across disks; overridable via
`ALTER_ZERO_CHECKPOINT_MAX_FILES`/`_MAX_BYTES`, `0` = no limit — the store is
retired with `disable()` (capability, not just the flag, so `/settings` reports
the row unavailable). Running out of *time* is its own verdict
(`ProbeOutcome::OutOfTime` → `TooSlow`), never folded into "too big": a probe
that timed out learned nothing about the size. Every refusal raises a one-row `Checkpoints off —
{reason}` toast, suppressed only when the user had already turned checkpoints
off: going quiet is what made "alter-zero takes seconds to boot in `/tmp`" and
"checkpoints do nothing here" read as two unrelated bugs — and a session-start
snapshot that *does* run announces itself the same way: the probe's bound
`SnapshotCost` becomes the pure `checkpoint::snapshot_notice` row
(`Snapshotting 326 files (7.9 MB) for checkpoints…`), committed above the
banner via `ui::startup_notice_lines` with one forced frame before the
O(bytes) `git add -A` blocks, quiet on a warm relaunch since a store with
nothing new probes as zero) in
`docs/checkpoint.md`; the **parallel tool-call batch** (the model's several tool
calls in one round announced up front so the running one shows live while the
not-yet-run ones show `⎿ Waiting…`, executed sequentially) in
`docs/parallel-tools.md`; the **live-streaming `bash` tool** (a running command
tails its output — the last rows, long lines word-wrapped to the width with
spaces preserved, + a `+N lines (Ns)` footer whose `(Ns)` is the
**command's own** runtime — `App::command_elapsed`, the boundary's
per-command clock started at the call's `ToolStart`, the masked
`background_hint_elapsed` being the Ctrl+B hint's gate over the same value —
never the turn's elapsed the status line counts (a call started a minute
into a turn used to open on `(60s)`), the agent session view counting its
own from the per-agent `AgentRun::command_elapsed` the same way — via a
`StreamEvent::ToolOutput` channel, collapsing to the head peek `… +N lines
(ctrl+o to expand)` when it finishes, Claude-Code style) in
`docs/tool-streaming.md`; the **bounded peek** (`docs/long-lines.md`: a peek
budgeted in *source lines* let ONE pathological line — a minified bundle, a
2 KB `curl` JSON body — spend the whole cell on itself, twelve rows of wrapped
noise under a hint claiming `+1 lines` was hidden, and then let FOUR wrapping
lines do the same thing legally, each inside its own per-line budget: a
`curl | grep` of a web page cost ten rows where the same four lines cost four
when they happened to be short. The **block** is budgeted in display rows now
— it **folds** Claude Code's way, `TOOL_FOLD_ROWS` (3) rows over the `… +N
lines` hint, an output of exactly four rows shown whole since a hint hiding
one row costs the row it hides — so a `bash` cell is at most four rows
whatever shape its output has, hint included, and the running tail's
`TOOL_PEEK_ROWS` (4) window is the same ceiling, so the cell never *grows*
when it settles. The fold is what bounds a blob: it shows three rows of it
as they are and the hint under them says the rest follows (Claude Code's
look — no `…` on the row); only the numbered file cell keeps a per-line
budget, `TOOL_LINE_MAX_ROWS` rows closed by `TOOL_LINE_ELLIPSIS`, since its
ten-line peek has no row ceiling of its own — never in Ctrl+O and never in
the permission preview, where the whole line is the point. The `… +N lines`
hint counts the **display rows** the expansion adds rather than source lines
(the numbered file cells keep counting *file* lines: the rule is "count in
the unit the expansion shows", and their gutter numbers them). And what the
settled cell shows is the output's **display lines** (`ui::exec_display_lines`,
shared with Ctrl+O so the hint counts exactly what the expansion adds): the
`Exit code` frame stripped, **a line that is a JSON document reshaped with
two-space indentation** — Claude Code's tool result does the same, which is
why its `curl` of an API reads `{` / `"batchcomplete": "",` where a compact
line read as a wall of braces; only a line that round-trips is reshaped, and
an output past `TOOL_JSON_PRETTY_MAX_BYTES` is left alone whole — and
trailing blank lines dropped (an all-blank output reads `(no output)`); the
running tail shows the output as printed. The counting is exact and
allocation-free — `ui::wrap`'s `WrapMode` pairs each wrapper with its own row
counter and clip over one range-emitting scan, so a hint can never count rows
a different wrapper would have produced and the running tail's footer can
measure the whole retained buffer every animation frame. And what the block is
a peek *of* is the output's **first block**, not its first four lines
(`BlankPolicy::FirstBlock`): a blank line costs a full row of a four-row cell
and says nothing, so leading blanks are skipped and the first blank after them
closes the peek — a `\n`-led output stops spending a quarter of the budget on
nothing, and a `git status`-shaped one stops painting a gap and then a fragment
of the *next* section as though the two were adjacent. The hidden-row count
stays exact (a skipped blank is a row Ctrl+O paints, so it is counted like any
other — dropping them from the tally would re-open the `+1 lines` lie in a
politer dress), an all-blank output is left exactly as it was (no first block
to prefer, and an empty window would strand the hint with no `⎿` corner), and
only the two exec cells take the rule — the backend `bash` tool and the `!`
shell command, one cell shape by design — while a diff body's spacing (content)
and an ask cell's `· Q → A` rows keep `BlankPolicy::Keep` and render
byte-identically; the running *tail* keeps its blanks too, being what the
command just printed); and the **session scratchpad** (Claude-Code's
temp directory, `docs/scratchpad.md`: one per-user, per-session temp root —
`{tmp}/alter-zero-{uid}/{session}/` — holding the agent's `scratchpad/`
beside the background shells' `tasks/` (the pure `scratchpad` module's
`session_root`/`scratchpad_dir`/`tasks_dir`, the temp dir + uid + session id
injected at the boundary, the session id **minted once** now and shared with
the lifecycle hooks' payloads, which used to carry a *different* nanos-derived
id than the tree they name). The directory is created before the prompt is
assembled, and `prompts/scratchpad.md` becomes the system prompt's **third**
block — persona → environment → scratchpad, composed by the pure
`augment_with_scratchpad`, omitted whole when there is no directory to name
(pointing the model at a path that does not exist, and refusing its writes
there in the same breath, is worse than saying nothing) — telling the model
every temporary file goes there instead of `/tmp`. And because the block says
those writes need no approval, they don't: `approve_call` consults
`PermissionGate::scratchpad_covers` **beside the standing allowlist** (before
the `PermissionRequest` hook, the classifier and the prompt — it answers the
same question), narrowly — `write`/`edit` only (a `bash` command naming a
scratchpad path still asks; what it goes on to touch is its own business), the
containment strictly lexical (`scratchpad::contains` — both paths absolute, a
`..` anywhere refusing outright rather than resolving, the match component-wise
so `{root}-elsewhere` is outside), and a `PreToolUse` forced ask still asking —
and **visibly**, resolving as `Approval::AllowNoted` with the classifier
note's sibling `SCRATCHPAD_ALLOWED_NOTE` (`⎿ Allowed in the session
scratchpad`); gated by `ALTER_ZERO_SCRATCHPAD`, relocated by
`ALTER_ZERO_SCRATCHPAD_DIR`) in `docs/scratchpad.md`; and the **background
shells** (the `bash` tool's
`run_in_background` arg — the call resolves at once with the interim-output
path — `{session}/tasks/{id}.output` — while a `BackgroundRegistry` process streams on its own channel; **Ctrl+B** moves a
running model-`bash`/`!` command to the background mid-run (the live cell hints
it with a dim `(ctrl+b to run in background)` row that waits a few seconds —
`ui::TOOL_BACKGROUND_HINT_DELAY`, gated on the command's own boundary-injected
`App::background_hint_elapsed` — so a fast command never flashes it, Claude-Code-style;
Ctrl+B itself works the whole time); the cell resolves `⎿ Running in the
background (↓ to manage)`, the footer
counts `· N shells` — and that count is the band's **entry point**: **↓ from an
empty composer lights the indicator on cyan** (`App::background_focus`, the
rest of the footer untouched; Esc/↑/Ctrl+C dismiss it, any other key clears it
and acts) and **Enter opens the inline manager band**
(list → per-shell details with a live-tailing output box → `x` stops) — a band
that both opens *and* closes with the shells: the indicator needs a running one
to light, and the **last** shell exiting (or being `x`-stopped) closes the band
outright rather than leaving an empty page whose every key has nothing to act
on (`App::bg_exited`), and a
completion is **immediate feedback**: its model-facing note posts onto the
registry's notice board the moment it exits (the in-flight agent takes the
board before each round, so a shell the model just `kill`ed is known to it
within the same turn, right after the killing call's tool result) and the
green/red `● Background command "…" completed` notice cell commits at the next
safe boundary — a tool resolution mid-turn, else the turn end — while a
model-launched note **no agent read** auto-starts a follow-up turn that tells
the model the result when nothing else is queued (an agent that already heard
it mid-turn owes no follow-up), `Done for Ns · N shells still running` on the
summary) in `docs/background.md`; and the **`Agent` tool** (Claude-Code-style subagents,
`docs/agent-tool.md`: the model launches autonomous side-agents —
`description`/`prompt`/`subagent_type`/`run_in_background` (default true) —
each running its own `run_agent` tool loop over a fresh context on its own
thread, the **type** coming from an `agents/*.md` **definition** on disk
(`docs/subagents.md`: Claude Code's agent files — YAML frontmatter naming and
describing the type, optionally pinning a `model:` (`inherit` by default) and
a `tools:` allowlist (omit for all; `Bash, Read, Skill, mcp__deepwiki__*` —
capitalized built-ins, MCP wire names, a trailing `*` globbing a server), over
an optional body that **replaces the persona** for that type while the
environment/scratchpad blocks and the subagent note stay; the built-in
`general-purpose`/`explore` are seeded into `~/.alter-zero/agents/` on first
run from the copies embedded out of `prompts/agents/` — editable files, never
clobbered, and the last-resort fallback (`AgentDefinition::is_builtin`, a
`path` of `None`) since the schema's default type must always resolve —
discovered from `{cwd}/.alter-zero/agents`, the project root's, then
`{config_home}/agents` (**no `.claude/agents`**: a `SKILL.md` is inert
markdown, an agent file names a model and a tool reach), **re-walked at every
turn start** beside the skills so a type the agent just wrote is launchable
now, an unknown `subagent_type` resolving as a recoverable error listing the
real ones, and the whole roster riding the context as the **second section of
the same `<system-reminder>` the skills listing opens** — `- {name}:
{description} (Tools: …)`, sharing **one** 1%-of-window budget with the skills
half (spent skills-first, `subagents::agent_budget`: one fragment, one budget,
where a listing of its own would have put 2% in front of every turn), gated on
the `agent` tool actually being on the wire so the offline dummy's context is
unchanged;
`ALTER_ZERO_AGENTS_DIR` relocates the roots; the allowlist guards the
**executor** too, since a model can name a tool it was never offered, and it
also decides the **briefing** — a launched agent opens on the skills
`<system-reminder>` *ahead of* its task (standing session information, not an
answer to it), assembled once where the launch is built so a chat continuation
never re-pushes it, and surfaced by `ReplySource::agent_system_reminder` so
the agent session view's Ctrl+D leads with the same block the agent read),
reporting on a dedicated `agents::AgentEvent` channel (a seventh
`select!` source — agents outlive turns); a foreground group shows the live
breathing-grey `● Running {n} agents…` tree (per-agent description · tool uses · tokens
· a **sticky** `{Name}: {detail}` activity — one grammar for every call
(`agents::activity_line`): a bash call's own `description`, held between
calls, else `{Name}: {args}` (`Write: game.py` — never the header's
parenthesised `{Name}({args})`), an MCP call as the capitalized
`{Server}: {tool}` (`Deepwiki: ask_question`) —
Ctrl+B moves the group to the background; a **lone** agent renders
`● Agent({description})` over that same one activity row instead, and **every**
such row — tree and lone cell alike — is one **dim, clipped** line
(`ui::agent`'s `agent_activity_row`/`clip_cols`), never the white multi-row
tool header a running call used to wrap open here)
committing as
`● {n} agents finished (ctrl+o to expand)` with `⎿ Done`/`⎿ Interrupted` rows
(a lone agent as `● Agent({description})` + `⎿ Done ({n} tool uses · {tokens}
tokens · {s}s)`),
a background launch resolves at once as `● {n} background agents launched
(↓ to manage)` with each completion posting its model-facing note on the
shared notice board (the in-flight turn hears it mid-round, an idle
completion auto-starts the follow-up turn — the background-shell pattern) and
its green/red `● Agent "…" finished · Ns` cell settling at the same safe
boundaries; the footer gains a persistent roster — `● main` over
`◯ {type}  {description} {elapsed} · ↓ {tokens} tokens` rows — that ↓ steps
into **after** the shell indicator (`❯` selection, Enter views, hint lines in
the footer slot; **`x` stops, then `x` clears** — the stop interrupts the
agent and leaves its row in place wearing a red `◯` for the long
`AGENT_STOPPED_LINGER` (30s, `AgentRun::linger` — a row that vanished under
the keypress left no evidence of what was stopped) while the hint swaps to
`x to clear`, and that second `x` takes it off at once; a naturally finished
agent lingers green for the same 30s `AGENT_LINGER` window — a row swept in
seconds vanished before it could be read — and answers the same clear;
and the `❯` **resumes where it left off** — `App::agent_selection_memory`
holds the last picked agent's *id*, so ↓ comes back to that row instead of
restarting at `● main`, with entering a session view counting as the pick and
a walk back onto `main` the way to forget one); Enter on an agent opens its **inline session
view** — a purge-rebuild showing the agent's own transcript under the banner,
the composer's top rule labelled with its description **embedded in the
rule** (`── {description} ─`, the rule resuming for one border cell after
the text) and **budgeted to half of it** (a `description` is whatever the
model wrote, and a right-aligned title too long to fit is trimmed from the
**left**, so an unbudgeted paragraph ate the rule whole and left a sentence
fragment with a border glyph stuck on its end; `ui::agent`'s
`agent_view_rule_label` clips it from the **end** with `TOOL_HEADER_ELLIPSIS`
instead, keeping the head that says what the agent is, and drops the label
outright once the rule cannot hold the mark plus one real character beside
it), its **commits keyed on what the fold recorded** — the transcript's
new `Tool`/`Summary` item, through the same builders a rebuild uses
(`Session::commit_agent_tail`; keying on the *event* is what dropped a
subagent's `write`/`edit` cells when the file tools moved onto
`ToolAnswered`, so they showed only after a resize) — and **streaming there
exactly as the main view streams**: the strip
previews the *same* `agent_render` frontier its commits leave behind — the
whole forming table, the whole withheld code line, a running `bash` cell
tailing its output through the shared `ui::live::live_call_lines` — over the
agent's own live `● Thinking…` block, which settles onto its transcript as a
`Thought for Ns · N tokens` cell like the main session's (the phase's elapsed
boundary-injected from `Session::agent_thinking_clocks`, the same `/settings`
**Hide thinking** gate); previewing one batch-rendered row instead, while the
commits withheld the block whole, was the reported "streaming disappears in
the subagent TUI" bug, and `/copy` there copied the *lead's* last answer
rather than the agent's — both `docs/agent-view-streaming.md`. Typing **chats with
the agent** (queued into its running loop at the next round boundary via the
registry's pending-input seam, or a continuation run over its stored message
list when idle) while the composer keeps its full functionality — the `/`
palette, `?` band, Ctrl+R, the `@` picker, **Ctrl+O showing the agent's own
transcript** and **Ctrl+D its derived context** (`!` shell mode stays
literal chat text), **and the footer's model and gauge segments describing
that agent** — its own context size (`AgentRun::context_used`, the last
usage frame's `input + output`, the transcript estimate when a settle saw no
frame) against the window of the model it runs on, the pinned model named
when its definition pins one (`App::context_gauge`,
`docs/agent-context-gauge.md`) — and the agent's turns **end like the main session's**:
each settle records the dim `Done for 59s · 6.1k tokens (2.8k cached)`
summary on the agent's own transcript (the turn's billed usage, a chat
continuation resetting the receipt while the roster tally stays cumulative;
a failed/interrupted run records none) — Esc returning to the purge-rebuilt
main view (main
commits are suppressed while the view is up, invariant-4 style); the Ctrl+O
transcript expands each agent as `● Agent({description})` with `⎿ Prompt:`,
the nested tool headers, `⎿ Response:`, and `⎿ Done ({n} tool uses ·
{tokens} tokens · {s}s)`; the parent's calls replay as native `agent`
tool_calls + results (`context.rs`), the records round-trip (`session.rs`),
and subagents never get the `agent` tool — no nesting); and the **tty detach** (every shell child —
model `bash`, `!`, background — spawned into a fresh session with no
controlling terminal via `subprocess::spawn_detached_shell`'s
setsid-binary → helper-re-exec → attached tier chain, so a `/dev/tty`
password prompt like `sudo`'s fails fast in a captured error instead of
hijacking the TUI and hanging) in `docs/tty-detach.md`; and the **tool
permission requests** (Claude-Code's ask-before-you-change: the `approve` seam
`llm::agent::run_agent` consults before every `write`/`edit`/`bash` call's
`ToolStart` raises an inline modal — a coloured `Create file`/`Edit file`/`Bash
command` title (`· from the {type} agent` when a subagent asked), the target,
the **whole** numbered content/diff framed by `╌` rules (shown whole — a page
taller than the terminal bottom-anchors and flows its top into real
scrollback like every framed view, `docs/view-flow.md`; the `… +N lines`
tail survives only past the `PERMISSION_BODY_MAX_ROWS` per-tick-build safety
ceiling), the question, and `❯ 1. Yes` / `2. Yes,
allow all edits during this session (shift+tab)` — for `bash`, `2. Yes, and don't
ask again for: {rule}` where the rule reads `python3 *` for a prefix scope
(the star = any arguments; an exact-only scope shows the whole command, no
star, and there is no letter shortcut any more) — / `3. No` over `Esc to cancel · Tab to amend`
(`· ctrl+e to explain` on a command; the options show **no hardware cursor at
all** — `ui::cursor_visible`, the frame just skips its closing `Show`: a menu
has nothing for one to point at, and a kitty cursor trail drew a streak on
every open and every ↑/↓ — while its *seat* still tracks the highlighted
option, found `PERMISSION_TAIL_ROWS` up from the region's bottom, which is why
a capped prompt pads *above* the question; Tab's amend field is typed into, so
the caret comes back with it) — while the tool thread **blocks** on the
shared `permission::PermissionGate` (the `Arc<Mutex<…>> + Condvar` sibling of
the background/agent registries, its `wait` polling the turn's `CancelToken` so
an Esc reaps it); the prompt is modal (routed first in `on_key`, replacing the
composer, the status line, the bands and the footer — and swallowing every key
**except Ctrl+O and Ctrl+D**, the two read-only views, which
`on_key_permission` runs off the shared `App::on_key_overlay_toggle` arm ahead
of everything else (Tab's amend field included — neither is an editing key):
the prompt asks about work that is already on the transcript, so locking the
transcript and the derived context is locking exactly what the answer is read
from, and both views only scroll, so the tool thread stays blocked, the draft
stays stashed and the prompt is still open on the way back (the return is the
ordinary `overlay_return_repaint` + flow check, so screen *and* scrollback come
back byte-identical — `smoke.sh` Phase 90; the overlay's idle Esc gains a
matching guard, `App::overlay_esc_backtracks` refusing to arm a rewind while a
modal waits, since a background agent can raise one with no turn running, and
the shared `App::modal_open` — `ui::region_is_modal`'s definition too — is what
keeps that guard, the key routing and the region's re-pin reading one
predicate) —
but **never the cells that raised it**: the call being asked about keeps its
`● Write(tt.py)` header over
the same dim `⎿ Waiting…` its batch siblings show (the approve seam runs before
`ToolStart`, so it genuinely is waiting — Claude Code's look; a truly running
call, the main turn's own under a subagent's request, keeps its `⎿ Running…`
at rest), and a subagent's request keeps the whole live
`● Running 3 agents…` tree, with `App::background_hint_elapsed` reading `None`
meanwhile so the delayed Ctrl+B hint never advertises a key the modal
swallows — *on the main screen*: **inside a subagent session view the context
is that agent's own queue**, its `● Bash(ls -la)` over `⎿ Waiting…` with every
parallel sibling behind it, the same `queue_chunks` walk the main branch uses,
since the lead's `● Agent({description})` cell and the main turn's queue belong
to a screen the user is not looking at (the reported "the subagent TUI shows
the Agent cell" bug — `docs/agent-view-streaming.md`); telling the two apart
needs `PermissionRequest::agent_id`, **which** run asked, stamped by
`tui::agent`'s handler (the type on `agent` names the asker in the title, but
two agents share one), and a request from another conversation keeps no
context at all rather than borrowing this one's — but the context is
**budgeted**: a big parallel batch's screenful
of `⎿ Waiting…` siblings used to squeeze the body's budget to zero (a prompt
with no content) and push the options off the bottom, so `permission_lines`
reserves its fixed rows plus a body floor (`PERMISSION_MIN_BODY_ROWS`, the
peek size; a shorter body reserves only its own height) and collapses the
cells that don't fit into one dim `… +N more waiting` row, the first chunk —
the asked-about call, or the tree that asked — never dropped; on a page that
**flows** the context rides along whole and uncollapsed when it is *static*
(a queued `⎿ Waiting…` cell cannot change — the approve seam runs before its
`ToolStart`), and gives way only when it **ticks** (a live agent tree's
breathing bullet and advancing counters, a running call's streamed peek —
a flowed row is frozen in scrollback, so ticking content would go stale
there or re-sign the flow into a purge rebuild per tick;
`context_is_stable`, `docs/view-flow.md`)); the region
**grows like any other** (ordinary `ui::repin`, invariant 3): the chat above
the prompt scrolls into the terminal's **real scrollback**, so the newest
messages sit right above the question — Claude Code's picture — and the user
can scroll the terminal up and re-read anything while it asks (the retired
covering geometry held the newest screenful in *no* buffer, which read as
"terminal scroll is disabled while it asks", worst in kitty — `smoke.sh`
Phase 58); commits flow under it too, so a batch's **back-to-back prompts**
(the next request landing in the same frame gap as the previous cell's
commit) just scroll the resolved cell in above the still-open prompt, visible
at once and exactly once (`smoke.sh` Phase 59); the scrolls are one-way
though, so every such move under an open prompt — its growth, a commit
beneath it, or a rebuild while it was open (a mid-prompt resize's purge, an
overlay return whose prompt opened underneath — Ctrl+O/Ctrl+D up when the
request arrived) — is noted on the viewport itself (`term::paint_live` and
`term.reflow` set the modal-scrolled flag, the two places one-way moves
happen), and the first draw after the prompt closes consumes it with a
**purge rebuild** (`InlineViewport::take_modal_scrolled`, `smoke.sh` Phases
58/60/62) — box flush at the bottom, scrollback rebuilt from history, nothing
lost or doubled — instead of a plain shrink stranding the box above the rows
the collapse vacates; the same rebuild answers a **shrink while the prompt is
still open** (`ui::modal_needs_rebuild` over the viewport's `painted_bottom`,
Phase 63): back-to-back prompts differ in height — a body-capped screen-tall
prompt answered into a one-line file's, the resolved cell committing out of
the live region between them, a subagent's tree-topped prompt giving way to a
main-turn one — and a frame that would seat the pinned region short of the
screen bottom it was *painted* flush against used to strand the open prompt
above a band of blank rows until it was answered (the reported
empty-newlines-under-the-prompt bug), where the draw tick now purge-rebuilds
first, `reflow` re-arming the note so the eventual close still purges; it
**stashes the composer draft**
and hands it straight back on close so a request landing mid-sentence costs
nothing, Tab swaps the options for that same textarea as an amend field whose
Enter rejects *with* the typed feedback, Esc cancels (reject + the ordinary
turn interrupt, the abandoned id released on the gate so a background agent's
thread never parks), a rejection still commits the red `⎿ User rejected write
to hello.py` cell — Tab's typed feedback on a second `Instructions: …` line,
the transcript's only record of it — while the *model* reads the longer
stop-and-wait text (`Approval::Reject`'s two fields, streamed together as
`StreamEvent::ToolRejected` in place of the `ToolEnd`), and **that
model-facing text is what the conversation keeps**: it rides the recorded call
as `ToolCall::context_output` and `context::context_messages` replays it —
`ToolCall::context_text()` — as the `tool` result, so Ctrl+D shows what the
model was actually told, every later turn still carries the user's
instructions (they used to survive exactly one round, history having kept only
the one-line cell), a `/resume` restores them (`session::ToolRecord`, the
field omitted when absent so old rollouts still parse), a subagent's refused
call keeps both texts on its own transcript, and the token tally charges the
uploaded text rather than the cell line; and option 2's allowlist remembers
every segment's **program-word prefix** (`python3 script.py` → `python3 *`;
the curated subcommand tools keep their verb — `git status`, `npm run` —
Claude-Code-style) — degrading to the exact command when a
redirect/substitution (quote-aware), a leading env assignment, or a command
wrapper (`sudo`, `sh -c`, …) means a prefix would hide what matters — and
**sweeps the requests already queued** that the new rule covers
(`App::drain_covered_permissions` — parallel agents all ask before any is
answered, so one answer covers them all); the session carries a **permission
mode** (`permission::PermissionMode` — `manual` asks for everything, `edit`
auto-approves `write`/`edit` while commands still ask, `auto` additionally
sends a non-allowlisted `bash` command to the **auto mode classifier** — a
silent LLM safety check on the session's own provider
(`llm::classifier::SafetyClassifier`, prompt in `prompts/classifier.md`,
`ALTER_ZERO_CLASSIFIER_MODEL` overrides the model) consulted by the approve
seam in the user's stead, **task-aware**: each verdict reads the session's
truncated context (`classifier::ClassifierContext` — two **rolling windows
over the conversation**, not one turn: the newest `CONTEXT_MAX_REQUESTS`
(10) user requests over the newest `CONTEXT_MAX_ACTIONS` (20) capped
`Name(args)` action lines, denials marked, each line truncated and every cut
counted, since the request that explains a command is often two turns back
and a re-tried denial should still be seen; the system prompt pins the block
as data-never-instructions; **Ctrl+D's second page** shows it — **Tab**
flips between the model's context window and its reviewer's, the two sharing
one chrome (`ui::render_context_view` paints either) with the classifier
page pulled live from the backend per draw via
`ReplySource::classifier_context` (which is why `LlmBackend` holds the log
behind an `Arc<Mutex<…>>` rather than a per-spawn local — it outlives the
turn), over a mode note saying whether it is actually being consulted, under
the classifier's own system prompt **abridged to its structure**
(`ui::classifier_view::abridge_prompt` — every heading whole, each section's
opening paragraph cut at `CLASSIFIER_PROMPT_PEEK_COLS`, the rest counted into
a dim `… +N lines` row; injected from `ReplySource::classifier_system_prompt`,
`None` for the dummy) and closed by the `## Action to review` header over a
placeholder for the action itself, so the page reads as the request's real
shape, each page keeping its own scroll, and Tab reaching it over an open
permission prompt since the modal's key routing only runs in the conversation
view)
above the one `## Action to review`: the asked-about cell just keeps its `⎿ Waiting…`
row while the verdict streams (silently — no UI events), an **allow** runs
the call with a dim `⎿ Allowed by auto mode classifier` row appended to the
resolved cell (the `Approval::AllowNoted` → `StreamEvent::ToolNote` →
`ToolCall::approval_note` chain — rendered inline and in Ctrl+O, recorded in
the rollout so a `/resume` keeps it, and on a subagent's own transcript via
the same event; the one inline exception is the quiet resolved MCP cell,
whose one-line `Called {server}` stays note-less — Ctrl+O and the rollout
keep its record, `docs/mcp.md`), a **deny** rejects it red (`Denied by auto mode
classifier` + `Reason: …`) through the ordinary `ToolRejected` path with a
Claude-Code-style stop-or-adjust model text, and a classifier **failure**
falls back to the ordinary prompt (never an allow); `master` runs
*everything* unasked — no prompt, no classifier, Claude Code's
bypass-permissions) pinned flush at the
footer's **right edge** (`{model} · {cwd}      manual` — its columns reserved
off the left chain's budget, so the `…` truncation can never eat it) and
**cycled** with **Shift+Tab** (manual → edit → auto → master → manual, one step
per press — from the composer, or on an open prompt: a file prompt's option
2 *is* the switch to `edit`, with a `Mode: edit …` toast; back to `manual`
and file changes ask again; a step onto `master` sweeps the open/queued
prompts it now covers; the offline dummy demos auto mode with the pure
heuristic `permission::auto_verdict` instead of an LLM); the rules **persist
per project** in
`~/.alter-zero/permissions.json` (`{"projects": {"/abs/cwd":
{"allow_commands": ["python3 *", …], "mode": "edit"}}}` — prefix rules
star-suffixed, exact commands verbatim, `auto`/`master` labels round-tripping
the same way, the pure format in
`permission::PermissionsFile`, the read-modify-write I/O + startup gate seed
in `tui::config`/`tui::permission`), so "don't ask again" and the mode survive a restart in the
same directory; gated by `ALTER_ZERO_PERMISSIONS` (disabled = no gate, no
footer segment, Shift+Tab explains via toast)) in
`docs/permissions.md`; and the **`AskUserQuestion` tool** (Claude-Code's
mid-turn questions, `docs/ask.md`: the model asks 1–4 multiple-choice
questions — `askuserquestion`, offered only when `LlmBackend::with_ask`
attached the session's `ask::AskGate` (always, in the app; subagents never
get it) — and its thread **blocks on the gate** exactly like a permission
request while an inline modal (the permission prompt's sibling: modal keys
routed first — Ctrl+O/Ctrl+D excepted, off the same shared
`App::on_key_overlay_toggle` arm, so a question about the conversation never
locks the two views that show it — composer draft stashed/restored,
`ui::region_is_modal` so the
close purge-rebuilds, the two modals queueing behind each other via
`App::open_next_pending`) walks the user through a chip strip of question
tabs (`☐`/`☒`/`✔ Submit`, the **current chip lit on the cyan selection
background**, ←/→/Tab/Shift+Tab moving, a lone question showing no Submit
tab), numbered options with dim descriptions (digits jump-activate; Enter on
a single-select records + advances — a lone question resolves at once —
while multi-select `[✔]` checkboxes toggle and confirm via their own
unnumbered `Submit` row), an auto-added free-text **`Type something.`** row
(the entry reuses `App::input` like Tab's amend field — a real composer
field: **Shift+Enter/Ctrl+J** newlines render as wrapped rows in place and
survive into the answer, and a large **bracketed paste** collapses to the
`[Pasted Content N chars]` placeholder (atomic Backspace), spliced back to
the real text on the entry's exit via `paste::expand_pastes_consuming` — the
stashed composer draft's own pairs survive the modal; Enter accepts —
single-select advances with the custom text as the answer, multi-select
checks it — Esc keeps the draft unchosen), a side-by-side **preview panel**
when any option carries `preview` content (options left, the focused
option's bordered panel right, the `Notes: press n to add notes` line
beneath — `n` opens the notes field, the same multi-line/paste-capable
entry, Enter/Esc both keep the text), a
**`Chat about this`** row resolving the whole call as "the user wants to
talk" (red cell + stop-and-wait result), and — for several questions — a
closing **review page** (an amber `⚠ You have not answered all questions`
warning whenever the submission would be partial, then `● question` over the
**green** `→ answer` for the **answered questions only** — an unanswered one
is omitted; its ☐ chip and the warning already say so —
`❯ 1. Submit answers / 2. Cancel`) whose empty submission walks to the first
unanswered question instead of submitting nothing; Esc anywhere **declines**
(the turn continues: red `User declined to answer questions` cell over the
`· question (options)` rows, the model reading the stop-and-wait result and
reacting in the same turn — never a forced interrupt); a submission resolves
green via the new `StreamEvent::ToolAnswered { display, result }` (the
`ToolRejected` twin, from `ToolOutcome::context` — the split the executor
`llm::ask::ask_user` builds with the pure `ask::answered_display` /
`answered_result`), the committed cell rendering the headline as its `●`
header (`ui::tool`'s ask special case) over the `⎿ · Q → A` gutter rows
while the model reads the schema's `{"answers": {question: labels},
"annotations": {question: {notes, preview}}}` JSON — kept on
`ToolCall::context_output`, so the derived context, Ctrl+D, and a `/resume`
all replay exactly what was sent; abandoned requests (Esc-cancelled
permission turns, `/clear`) release on the gate as declines at the loop
bottom so no thread parks; the offline dummy's `Play::Asked` scenario (cue
"ask" + "question") drives the whole round trip through the real
`ask_user` mapping — three questions: single-select coffee, multi-select
demo topics, preview+notes code style) in `docs/ask.md`; and the **task
tools** (Claude-Code's structured task list, `docs/task-tools.md`: the model
plans multi-step work with `taskcreate`/`taskget`/`tasklist`/`taskupdate` —
Claude Code's schemas minus the `owner`/`metadata` parameters this
single-agent TUI has no use for, the create additionally *honouring* the
dependency fields a live model folds into it (`blocks`/`blockedBy` and their
`add…` spellings, validated before the task exists so a bad id creates
nothing) rather than dropping them for neither an error nor an effect — over
a shared `tasks::TaskRegistry`
(`LlmBackend::with_tasks`, the ask-gate pattern; subagents and the `/compact`
backend never get it), and the calls render **no tool cells anywhere
inline** (Claude Code hides them): each resolves through the single
`StreamEvent::TaskCall` — `run_agent` skips the batch announcement, the
permission seam, and the Start/End pair for task calls, the post-call
snapshot riding `ToolOutcome::tasks` — whose loop arm flushes the streamed
segment (each round's narration stays its own `●` bullet) and records a
cell-less `HistoryItem::TaskCall`; what the user sees instead is the **live
checklist under the status line** (`ui::task_rows`/`checklist_lines`, its
rows threaded through `live_height`/`live_layout`/`cursor_position` beside
the queued rows, inside the status slot so the `⎿ ◻ subject` rows hang off
the spinner): `◻` pending, `◼` in progress (cyan glyph, bold subject), `✔`
completed (green glyph, dim struck-through subject), a dim `› blocked by #1`
suffix naming only **open** blockers, one-row truncated subjects, and past
`TASK_MAX_ROWS` a prioritised fold into a dim `… +N pending` row — while the
spinner **wears the active task's `activeForm`** (`App::task_verb` →
`ui::status_line_with_verb`, Claude Code's `currentTodo.activeForm ??
randomVerb`, derived per frame so completing the task snaps the verb back);
**at rest** the same rows sit above the composer under Claude Code's dim
`1 tasks (0 done, 1 open)` count line (`ui::idle_task_lines` — the same
glyphs/fold, the `⎿` gutter swapped for the composer's inset, since there is
no spinner to hang from), whose three numbers **partition** the list —
`open` is the not-yet-started tasks alone, the reference's `pendingCount`, so
a running task is reported once instead of counted as both in progress *and*
open (`3 tasks (1 done, 1 in progress, 1 open)`, never `2 open`) — so work
left over stays in view between turns;
and a **finished** plan *retires* — the turn that ticked the last task keeps
its all-green rows, then `dispatch_after_turn` drops the list whole
(`App::retire_finished_tasks` → `TaskStore::retire_if_finished`, re-syncing
the registry), so it is gone rather than hidden and the next `taskcreate`
opens a new plan at `#4` instead of appending to the old ticks (the id
high-water mark survives; a `/resume`/backtrack applies the same rule to the
snapshot it restores);
the record keeps everything the cell-less display doesn't: Ctrl+O expands
each call as an ordinary tool cell (`TaskCallRecord::as_tool_call`), the
derived context replays the native `tool_calls`+result pair **with the
model's own arguments verbatim** (`TaskCallRecord::arguments`, recorded
beside the header summary and round-tripped through the rollout, so a later
round re-reads the `{"taskId":"1","status":"completed"}` it actually sent
rather than a placeholder `{}` a validating provider would reject), the
rollout
round-trips the call **with its post-call snapshot** (`session`'s
`task_call` record, ids on a high-water mark that survives deletion), and
all three history rewinds restore the list exactly — `/resume` and the
Esc-Esc backtrack from the last record('s snapshot) before the cut
(`App::reset_tasks_from_history`), `/clear` to empty — with the boundary
syncing the shared registry after each (`Session::sync_task_registry`) so
the model's next `tasklist` agrees with the strip; the offline dummy's
`tasks` scenario (cue `todo`/`task`) drives a real `TaskStore` through the
whole lifecycle so every scripted result string and snapshot is
byte-for-byte the live executor's, ending with work outstanding so the
resting panel and the cross-turn list show, while its `tasks-finished` twin
(the same cue plus `finish`) walks a two-task list to all-✔ so the
retirement is drivable too — `smoke.sh` Phases 69 and 70) in
`docs/task-tools.md`; and the **lifecycle hooks** (Claude Code's
`hooks.json`, ported whole — `docs/hooks.md`: the user's own commands wedged
into the tool loop, `~/.alter-zero/hooks.json` mapping event → matcher groups
→ `{"type":"command"}` handlers, each fed its event as **`snake_case` JSON on
stdin** and answering with **`camelCase` JSON on stdout** — the asymmetry is
the contract, and both references agree on it, so a script written for either
tool works here unchanged; exit `2` blocks with **stderr** as the reason,
`0` + `{…}` is a verdict, any other non-zero is a non-blocking error, and a
handler that could not be spawned or timed out **fails open** with a warning
(a broken guard must not wedge the agent); several matching handlers merge —
any block wins, the first reason is kept, contexts concatenate, `deny` >
`ask` > `allow`. The pure half is **`src/hooks/`** (`config` — the file
format + `select`, which dedups by command and warns rather than failing on a
handler type or event name we don't model; `matcher` — both references'
non-regex fast path for `bash|write`, exact-equality so `bash` never matches
`bashoutput`, else a real regex (cheap: `regex` is a thin wrapper over the
`regex-automata` engine `fancy-regex` already puts in every build, so
warn-and-skipping `^Bash$` would have been a footgun with no offsetting
saving); `event`; `payload`; `verdict`; `overview` — the display tree the
read-only `/hooks` menu browses), taking a handler's stdout **as a
string** so every rule is unit-testable with no process anywhere. The boundary
is **`src/llm/hooks.rs`**: the `HookSink` trait — *one trait object with
defaulted no-op methods*, not a closure per event, so adding event number six
is one defaulted method plus one call site rather than a wider `run_agent`
signature — plus the runner, which spawns through
`subprocess::spawn_shell_with` (the tty-detach tier walk with **piped stdin**,
so a hook that opens `/dev/tty` fails fast like every other shell child) and
waits on `llm::exec`'s 20 ms poll cadence, **re-checking the turn's
`CancelToken`** and group-killing on cancel or timeout — a blocking wait that
skipped that would silently break Esc — the payload write included, on its
own thread, since a pipe-buffer-filling `PostToolUse` payload fed to a
handler that never reads stdin used to park an inline `write_all` past both
Esc and the timeout. **All eleven events fire, every one on a backend
thread** — verifying the references dissolved the old loop-path premise
(Claude Code runs its stop hooks *inside the query loop*, and neither
reference runs SessionStart at startup — both block only the next request):
the tool-path five gate/annotate/rewrite calls (**`agent` launches
included** — `Task` aliases to `agent`, and each tool name answers to its
Claude Code spelling as a second exact name; `PostToolUse` fires only for a
call that **succeeded**, both references' behaviour), `Stop`/`SubagentStop`
fire in `run_agent`'s Complete arm where a block is a **same-turn
continuation** (the reply-so-far becomes an assistant message, the feedback
the next user message, `stop_hook_active` flips true and is the hook's own
guard — the engine never refuses a re-block, the reference's posture, and an
interrupt never fires Stop so Esc always breaks a chain; the turn-end
checkpoint lands after every continuation, so a formatter hook's writes are
inside the snapshot), `SessionStart` drains queued sources
(`startup`/`resume`/`clear`) at the next spawn's top, `UserPromptSubmit` can
refuse the prompt — `StreamEvent::PromptBlocked`, the submission rolled back
out of history *and* the rollout (the recorder's shrink-rewrite), the text
returned to the composer under a red reason-only notice — or inject context,
a loop-initiated background follow-up turn being marked synthetic and
skipped; `PreCompact`/`PostCompact` ride the summarization spawn via the
`CompactHooks` wrapper (PreCompact stdout/context = extra compact
instructions, **no block — neither reference honours one**), and
`SessionEnd` runs under a 2 s whole-event budget at `/clear`/quit. A block
*is* `Approval::Reject`'s two-text split (red cell, model-facing instruction,
`ToolCall::context_output`, `/resume`-safe), a `PreToolUse` allow *is*
`Approval::AllowNoted` (`permissionDecision: "ask"` skips the allowlist
**and** the auto-mode classifier — ask means a human), and
`additionalContext` rides `ToolNote` for the dim `⎿` row plus the
`ToolAnswered` split so the cell keeps the tool's own output while the model
reads the augmented text — appended **at the frontier**, never in front of
`user_instructions`, which is rewritten per turn and would invalidate the
prompt cache. The two conversation-level additions that *were* needed:
`StreamEvent::HookNote` → the cell-less `HistoryItem::HookNote` (invisible
inline, expanded in Ctrl+O, replayed verbatim by `context_messages`,
rollout-round-tripped) and the terminal `PromptBlocked`. `PreToolUse` may
also **rewrite** the call (`updatedInput` replaces the arguments for the
gate, the executor and the cell), `PermissionRequest` sits exactly where the
auto-mode classifier does, and payloads resolve `permission_mode` and
`transcript_path` **live at dispatch** (the gate's current mode; the rollout
path the recorder publishes into a shared cell). User-level config only — a
project layer needs a trust model, and layers
would union, so it stays purely additive; a malformed file is a red startup
toast, not a silent "no hooks". `/settings` gains a **Hooks** row, unavailable
when no file resolved and **off until a directory turns it on**
(`docs/per-directory-state.md`); `ALTER_ZERO_HOOKS` seeds it for a run and
`ALTER_ZERO_HOOKS_FILE` locates the file; the offline `hook` scenario drives the tool-path shape and the
`prompt-block` scenario the rollback, `smoke.sh` Phases 72 and 73) in
`docs/hooks.md`; and the **read-only `/hooks` menu** (Claude Code's `/hooks`
browser, `docs/hooks-menu.md`: the fourth composer-replacing inline picker —
no text entry, so the hardware cursor hides while its seat tracks the
selected `❯` row (the permission prompt's kitty-cursor-trail rule, shared
with `/mcp` and `/trust` — `ui::cursor_visible`) — walking events → matchers →
hooks → detail over `hooks::HooksOverview`, the digest of the runner's own
parsed file; ↑/↓/digits/Enter/Esc, a five-row selection-centered window
with ↑/↓ overflow markers, per-event summaries/descriptions stating **this**
runner's exit-code semantics, matcher level only for the events whose
dispatch matches on something (`hooks::event_has_matchers` — `Stop` and
`UserPromptSubmit` skip it), the detail page's rounded box holding the real
command word-wrapped, works mid-turn, `smoke.sh` Phase 75); and the **view
flow** (`docs/view-flow.md`: a content-driven framed view's page taller than
the terminal — an `/mcp` tool detail's long description, a `/hooks` detail's
wrapped command box, a `/trust` review's verbatim listing — no longer clips
at the bottom: every framed body paints **bottom-anchored**
(`ui::view_body_skip`; the seat's `menu_marker_seat` and the pickers'
`anchored_view_row` subtract the same skip) so the hint and closing rule
stay on screen, and the skipped top **flows into the terminal's real
scrollback** directly above the region (`ui::view_flow` — eligibility
mirrors the render precedence, so the painted view is the one that flows,
a covering ask/permission modal's own page included; **every** framed view
flows, the ↓ manager band with them — but what a flow is *signed* on is a
per-view choice (`FlowSign`): a page that changes only on a keystroke signs
its **rows**, so any edit re-signs and the rebuild re-flows them, while the
manager's details page, the one page that ticks **between** keystrokes (its
runtime advances and its output box tails at the open band's ~30 fps), signs
the **shell it describes** and its flowed top *freezes* in scrollback rather
than purge-rebuilding the screen every frame — anchoring alone used to drop
those rows into no buffer at all, so the conversation ran straight into a
headless output box, `docs/background.md`, `smoke.sh` Phase 105) where the
terminal's own scrolling reads the whole page — and
the four windowed pickers (`/model`, `/login`, `/settings`, `/skills`) are
the same shape now: each render is one **line builder**
(`model_view_lines`/`key_onboarding_lines`/`settings_view_lines`/
`skills_view_lines`, the retired internal `Layout` stacks), its height the
built line count so the reserved rows and the painted rows can never
disagree, flow-eligible like the menus (their top-of-frame `❯` search line
re-flows per keystroke, since a keystroke re-signs the flow); the
boundary keeps the flowed rows' signature (`Session::flowed_view`) and
answers any mismatch — navigation, resize, close — with the standard purge
rebuild (`repaint_conversation`/`repaint_agent_view` append the flow to the
reflow tail), while scrollback commits pause under an active flow
(`Session::commits_allowed`, the agent-view pattern — a queued turn
dispatching under an open screen-tall menu regenerates from history at
flow-exit instead of tearing the page), `smoke.sh` Phase 85); and the
**project-level `.alter-zero` config layer behind the `/trust` gate**
(`docs/project-config.md`: a project's own `{root}/.alter-zero/hooks.json`
(union-merged after the user file — `HooksFile::merged`, the pre-committed
additive semantics) and `{root}/.alter-zero/mcp.json` + the compat
`{root}/.mcp.json` (first-name-wins ahead of the user file) are read once at
bootstrap — the root being the nearest-`.git` ancestor-or-self, else the cwd,
**except the home directory, which is never a project**
(`trust::is_project_root`: a git-less `~` would make `{root}/.alter-zero`
the user's own config home and ask the user to trust their own files; a
per-file identity guard catches the same collision under a moved
`ALTER_ZERO_CONFIG_DIR`) — each file's bytes SHA-256-fingerprinted
(`trust::fingerprint`)
and checked against `{config_home}/trust.json` — **default-deny**, because a
checked-in hooks handler or stdio server executing on clone is the hole the
hooks doc refused to open (and `.mcp.json` used to have): untrusted hooks
contribute nothing to the merge, untrusted servers sit in `/mcp` as
`⚠ untrusted` (never launched, never shadowing the user's own same-named
server — `mcp::merge_project_scopes`) while a startup toast points at
**`/trust`**, the seventh composer-replacing picker (the `/hooks` browser's
sibling: no text entry, hidden cursor seated on the `❯`) showing the root, each file's
hook commands / server targets **verbatim** with a
pending-approval/trusted/won't-parse badge, over `❯ 1. Trust this project's
config` / `2. Revoke trust`; approval records the **reviewed snapshot's**
fingerprints (never a re-read — no approve-what-you-didn't-see race, an
edited file re-pends at the next launch) via `trust::record_trust`'s RMW and
activates **live** — `ModelSession::set_hooks_file` swaps the merge into the
next backend build, `McpManager::set_trusted` connects the held servers
(revoke kills them back to `untrusted`); a malformed `trust.json` fails
**closed** with a red toast (guard files are loud), an unparseable project
file is pending-but-unapprovable (recording a hash sight-unseen is not
trust); skills stay outside the gate (inert markdown; their bodies' commands
still meet the permission gate); the pure format/fingerprint/review model is
`src/trust.rs`, the boundary load/apply `src/tui/trust.rs`, gated by
`ALTER_ZERO_PROJECT_CONFIG` — which `smoke.sh`'s base env turns **off** for
hermeticity (the skills Phase 36 lesson: the suite's cwd is a real
checkout), Phase 83 driving the whole flow — pending toast → untrusted
`/mcp` row → review → approve → live connect + merged `/hooks` → relaunch
still trusted — in a temp project); and the **`Skill`
tool** (Claude Code's skills, `docs/skills.md`: folders of authored markdown
the model pulls into the conversation on demand — a `<root>/<name>/SKILL.md`
of YAML frontmatter (`name`/`description`, everything else **ignored not
rejected** so an ecosystem skill carrying `allowed-tools:`/`model:` loads
unchanged) over a body, discovered from the cwd's `.alter-zero/skills` +
`.claude/skills`, **the nearest-`.git` project root's** pair (skipped when it
*is* the cwd, so launching in `repo/src` still finds the repo's skills — the
`AGENTS.md` walk-up, `project_doc::find_project_root`), the config home's
`skills`, and `~/.claude/skills`, first root winning a name
(`ALTER_ZERO_SKILLS_DIR` **replaces** the list, the `*_DIR` convention — and
what makes a smoke run hermetic; a `SKILL.md` that won't parse is a toast
naming it, never silence). **One skill ships in the binary** — `skill-creator`,
which teaches this format (the frontmatter contract, the roots, how to word a
description that triggers, how to update one without clobbering it), because
the format is *ours*: a model asked for "a skill" without it writes a lone
`my-skill.md` at a root, or an `allowed-tools:` line it expects honoured, and
every such near-miss fails **silently**, the walk reading only
`<root>/<name>/SKILL.md`. Authored in `prompts/skills/skill-creator/` beside
every other `include_str!`'d markdown and seeded into `{config_home}/skills`
**before** the startup walk (so the session that installed the app can already
use it) the way the agent definitions are — editable, never clobbered, a
deleted file back next launch, the off-switch being `/skills`, which persists,
rather than `rm -rf` — but **never into an `ALTER_ZERO_SKILLS_DIR` override**,
the one place the two seeds differ: that variable says *only these*, and a
built-in skill is a convenience the session works without where a built-in
agent *type* must resolve (`general-purpose` is the `agent` schema's default).
It is **two files** because the loader's own expansion runs over the body:
a body that documents `${…SKILL_DIR}` has it rewritten out from under it —
the first live run got the same path twice for the sentence naming both
skill-dir spellings — so detail that must survive verbatim lives in the
`reference.md` beside it, which the model **reads** (the multi-file pattern the
skill teaches, demonstrated rather than described), with
`no_built_in_body_carries_a_placeholder_the_loader_would_eat` rendering every
built-in and requiring the body back byte-for-byte. The walk
re-runs at **every turn start**
(`Session::rescan_skills`, beside the `AGENTS.md` refresh): a startup-only
discovery froze the session at what it booted with — a skill you added, or one
the agent had just written *for* you, was invisible until a restart — and the
cost is four to six `read_dir`s against a turn about to hit the network. The
`/settings` **Skills** availability is re-derived **before** the listing (it
gates it, so the other order shipped the listing a turn late), the backend is
rebuilt **only** when the tool set actually flips
(`ModelSession::skills_attached` vs the live verdict — toggling one of five
skills reshapes no request), and the repeated walk's toast is held to once per
file by `skills::unreported_errors`, re-seeded each pass so a file fixed and
re-broken reports again. Only each skill's one-line description rides the context — the
budgeted `<system-reminder>` listing (the reference's 1%-of-window character
budget, descriptions trimmed to an even share and degrading to names-only
rather than **dropping** a skill, since one you can't see is one you can't
invoke) that `context::context_messages_full` composes into the derived
context's one leading `<system-reminder>`, right behind the AGENTS.md
instructions section and in front of everything else, a fixed position
because every section is re-rendered per turn and one that moved would
invalidate the prompt cache behind it —
**one** reminder with a section each for the instructions, the skills and the
subagent types (`reminder::reminder_message` wraps the sections,
`subagents::listing_sections` joining the two listing sections into
`App::listings` beside `App::user_instructions`, `skills::listing_message`
being the skills section wrapped alone for a subagent; `docs/subagents.md`),
since they are one kind of thing (what this session tells the model about
itself that the tool schemas don't name) and a second fragment would be a
second place the cached prefix can shift.
A call resolves through the **`ToolOutcome::context` two-text split the ask
tool already had** rather than a parallel mechanism: `output` is the whole
visible surface — `● Skill(dataviz)` over one green `⎿ Successfully loaded
skill` — while `context` is the rendered body (its `Base directory` header,
`${…SKILL_DIR}` expansion, 100 KiB cap — and **nothing else**: the schema is
`{"skill": "<name>"}` alone, the reference's optional `args` string and the
`$ARGUMENTS`/`$1`…`$9` substitution pass it fed deliberately retired, since
that pass rewrote the body's own prose (it is what forced `skill-creator`
into two files), nothing but the model's own guess ever supplied it — a `$name`
mention is plain text carrying no parameter — and a schema parameter is one
the model weighs and fills on every call; a resumed rollout's older
`{"skill": …, "args": …}` still loads, serde ignoring the unknown field),
so the green `ToolAnswered` path, `ToolCall::context_output`,
the context replay, the rollout round-trip and Ctrl+D all come for free;
Ctrl+O keeps the one line too (the transcript is what *happened*, Ctrl+D what
was *sent* — the rule every other two-text call follows), and the replay
reconstructs `{"skill": "<name>"}` from the summary, which for this tool
**is** the name, since a validating provider rejects the `{}` an unmapped
tool would have sent. Nothing runs, so no permission prompt is raised — the
body's own `bash` calls still meet it, and the call *does* meet the lifecycle
hooks like any other (`{"matcher": "Skill"}` selects it —
`hooks::claude_code_alias`). Subagents carry the tool **and its listing** —
`subagent_skill_reminder` pushes the `<system-reminder>` right after the
launch prompt, since a side agent starts on a fresh context and a spec whose
own description points at a reminder that isn't there is the same
listing/tool mismatch in mirror; the
`/settings` **Skills** row (unavailable when none loaded) and
`ALTER_ZERO_SKILLS` gate it — as does **Tools**, since the spec rides the tool
set: `SessionSettings::skills_offered` is the one gate the listing and the
tool set share (`skills_active` stays the row's own value, which Tools must
not rewrite), because a `<system-reminder>` naming a tool the request never
carries is a dead end the model spends a round hunting for; the tools-free
`/compact` turn carries no listing for the same reason;
`$<skill-name>` needs no code — the mention submits as
plain text and the Skill tool's own description tells the model a `$<name>`
mention is a load request (verified live —
`live_dollar_mention_loads_the_mentioned_skill`), the composer's **`$`
mention picker** (`docs/skill-mentions.md`) completing one in place with
Tab/Enter, and the offline `skills`
scenario answers `$dataviz` mentions beside its "skill" cue so the demo and
the smoke suite drive the round trip with no network; the offline `skills` scenario drives the cell
through the real formatters, `smoke.sh` Phases 76, 78 and 79 — 78 planting
a `SKILL.md` mid-session and proving the next turn's Ctrl+D already lists it,
79 driving the `$` band end to end);
and the **`/skills`
menu** (the fifth composer-replacing inline picker and deliberately the
`/settings` menu's **twin** rather than a new shape — same frame, same `❯`
type-to-search, same aligned `{label}  {value}` column with the same two-tone
colouring, same `(n/total)` counter, same Enter/Space grammar — over one row
per discovered skill instead of one per knob, the skill's **own description**
as the line under the list so the picker doubles as the browser that answers
"what is this skill for?"; two rows `/settings` has no need of: a
session-off note when the **Skills** row is down (always reserved, so the
frame can't jump) and an empty list that **names the roots**
(`No skills found. Add one at:` over `~/.claude/skills/<name>/SKILL.md`),
since "why isn't my skill here?" is an empty list's only question. A toggle
holds in **two** places — the skill leaves the `<system-reminder>` listing
*and* `SkillRegistry::find` refuses it, so a model that remembers the name
from an earlier turn gets the recoverable "unknown skill" rather than
loading what the user turned off — while `snapshot` still returns it (the
menu must show a disabled skill or you could never turn it back on), which
is why the registry answers two questions: `is_empty` (was anything
**found**? the `/settings` row's availability) and `has_enabled` (is
anything **on**? whether the tool is offered), so turning every skill off
withdraws the tool exactly as having none installed does — and that
withdrawal is why a toggle rebuilds the backend, the shared handle already
carrying the change to the executor and the listing but the *tool set* being
decided when `with_skills` runs. It persists **per project** in
`{config_home}/skills.json` (`{"projects": {"/abs/cwd": {"disabled":
["haiku-writer"]}}}` — `permissions.json`'s shape, read-modify-write,
best-effort, an empty set **dropping** the entry so the file stays a diff
from everything-on, and a disabled name not installed here **kept** rather
than pruned since the same file serves a checkout elsewhere), because skill
relevance is project-specific while the `/settings` **Skills** row is
already the session-wide switch; `smoke.sh` Phase 77 drives the palette
entry, the rows, a toggle and its persistence across a restart); and the **Ctrl+O
performance work** (the incrementally-built, boundary-warmed transcript cache
and the atomic queued overlay switch, so the transcript opens instantly on a
big resumed session with no blank alt screen / kitty cursor-trail streak) in
`docs/tool-view-performance.md`; and the **overlay repaint fix** (the two
full-screen views are **copyable while a turn runs**: a terminal drops a mouse
selection the moment the cells under it are rewritten, and `draw_overlay` used
to re-serialize **every cell** of the alternate screen on every frame — up to
120 a second, since each reply event schedules one, floored at ~31 by the
status animation's clock chain — so text in Ctrl+O / Ctrl+D could not be
selected until the turn finished. It now **diffs against the frame already on
the screen** (`term::overlay_paint` → `OverlayPaint::{Unchanged,Diff,Full}`,
the pure sibling of the inline `paint_frame`'s `prev.diff(buf)`, over its own
`overlay_prev` baseline — `prev` describes a *different* screen and the
entry/exit paths clear it) and an **unchanged frame emits nothing at all**: no cells, and no synchronized-update
or cursor-move escapes either, so a still page is silence on the wire. The
baseline drops on every entry (the queued `Clear(All)` blanks the screen),
every exit, every `resized` (**unconditionally** — the area check alone is not
enough: a resize burst can coalesce into one frame and land back on the recorded
area while the emulator clipped and regrew the screen in between, stranding
stale rows); `Buffer::diff` skips wide-glyph shadows
itself, so the diff path inherits `visible_cells`' rule rather than reopening
the table tear. Beside it — a CPU saving, **not** part of the copy fix, since a
re-armed frame over an unchanged page now writes nothing anyway — the draw tick
stops **re-arming** the 32 ms chain under an overlay
(`App::wants_animation_frames` → `View::is_overlay`): every thing it animates is
inline (`tool_full_body` pins `pulse = None`, `TranscriptSig` has no clock, and
the elapsed-bearing `shell_running_line`/`running_command_lines` are
`src/ui/live.rs`-only), every event source already schedules its own frame, and
the chain re-seeds on the first draw after the return. What pauses is the agent
roster's runtime/linger bookkeeping — recomputed from absolute `Instant`s, so
deferred rather than lost, and invisible under an overlay anyway. Measured with `tmux pipe-pane` over three seconds of an active turn:
**316 KB → 0 B** for Ctrl+D, **389 KB → 0 B** for Ctrl+O, the inline control
unchanged at 4.5 KB. Ctrl+D is provably still (its `ContextSig` carries no
streaming state, so it is zero for the whole turn); a scrolled-back Ctrl+O
writes **only the cells that moved** — zero when the frontier is off-screen,
else a few hundred bytes a second of the growing line's own words landing on
the one row they belong to, every other row untouched; and a Ctrl+O pinned to
the bottom still tail-follows, which is what follow is *for* — a new row shifts
the window and costs the selection, ↑ / PageUp / Home disengages it and the page
holds still. `smoke.sh` Phase 94 measures the silence **and** that the chain
re-seeds on the return, since a chain left broken would freeze the inline timer
for the rest of the turn) in `docs/overlay-repaint.md`; and **`/compact` + auto-compact** (codex's
context compaction, ported append-only: a summarization turn streams the
model's handoff summary invisibly into `App::compact_buffer`,
`finish_compact` appends a `HistoryItem::Compaction` marker — the transcript,
recorder, checkpoint keys, and backtrack all untouched — and
`context::context_messages` derives codex's compacted shape from the *last*
marker: the 20k-approx-token budget of recent user texts + the
`SUMMARY_PREFIX\n{summary}` bridge in place of everything before it, the
`● Context compacted · {before} → {after} tokens` cell the visible record;
with the model's **context window** known — `/v1/models` `context_length`
via `ModelEntry::context`, persisted in `config.json`, overridable via
`ALTER_ZERO_CONTEXT_WINDOW` — the footer shows a `{used}/{window} ({pct}%)`
gauge (usage-frame fed, tokenizer-estimated offline; **inside an agent
session view it is the viewed agent's own** — `AgentRun::context_used` /
`App::context_gauge`, `docs/agent-context-gauge.md`) and the loop **auto-runs** the
same turn past codex's 90% threshold (`App::should_auto_compact`, one
attempt per user turn, the cell tagged `· auto`)) in `docs/compact.md`; and
the **`/settings` menu** (`docs/settings.md`: the knobs that were only ever
`ALTER_ZERO_*` environment variables — plus a hard-coded `retry::MAX_RETRIES`,
a hard-coded `agent::MAX_TOOL_ITERATIONS`, and an always-on auto-compaction —
made *visible and changeable mid-session*
in the `/model` picker's inline frame, the third composer-replacing picker:
sixteen rows (**Hide thinking**, **Show images**, **Image width**,
**Auto-resize images** — the three from `docs/images.md` — **Error retry**,
**Tools**, **Permission
mode**, **Checkpoints**, **Auto compact**, **Project docs**, **Hooks**,
**Skills**, **Temperature**,
**Max tool calls** — whose `0` default means *no limit*, since a cap that
trips mid-task abandons the work half-done and Esc is already the stop
button; it counts the **calls**, not the rounds, because a round can be a
whole parallel batch, and a round the budget can only partly afford is
clamped rather than refused whole; **Update check** — the once-a-day
newer-release check, `docs/update.md`: one `HEAD` of the repository's
`/releases/latest` per UTC day, the tag read off the redirect it lands on (no
API, no body, no install id — the `alter-zero/{version}` user agent and
nothing else), the attempt's day recorded **at the spawn** so a dead network
costs one request a day and nothing the user sees, the answer kept in the
per-**user** `update.json` beside `telemetry.json` (`enabled`,
`last_check_day`, `latest`, `notice_day`), and a newer release announced
**once a day** through the `ui::update_notice_lines` card — the telemetry
card's own frame, `ui::notice_card_lines`, naming the release page, `alter-zero
update` and the off switch — committed after the first frame and **never
inside a reply** (a result landing mid-turn waits in
`Session::update_notice_pending` for the idle loop bottom, invariant 4), off
with the row or `ALTER_ZERO_UPDATE_CHECK=0` (which withdraws the row, the
Telemetry rule), `ALTER_ZERO_UPDATE_URL` pointing a fork or `smoke.sh`
Phase 116's stand-in `release_server.py` at another repository root; and the
**`alter-zero update` subcommand** (`cli::Cli::Update` → `tui::update_cli`,
routed at the pre-TUI boundary like `mcp`) doing the install: the same
`HEAD`, then `install.sh` fetched **to a temp file**, never piped (`sh` on an
empty pipe exits 0) and shape-checked before it runs, with
`ALTER_ZERO_INSTALL_DIR` = the running binary's own directory,
`ALTER_ZERO_VERSION` = the tag just resolved and `ALTER_ZERO_INSTALL_BASE_URL`
= the same repository, refusing a `target/{debug,release}` binary (a checkout
to `git pull`, not an install to overwrite) and running regardless of
`ALTER_ZERO_UPDATE_CHECK=0`, since an explicit command is the user's own
request; and **Telemetry** — the anonymous daily
usage ping, `docs/telemetry.md`: one `POST` a day per install carrying seven
fields (a payload version, a random 128-bit install id, the app version, the
OS, the architecture, on Linux the distribution's os-release `ID`, and that
platform's own version — `VERSION_ID` on Linux, macOS's `ProductVersion` read
straight out of `SystemVersion.plist` rather than through `sw_vers`, absent
for a rolling release and on Windows; never a display name, never a build id,
and never a prompt, a path, a model name or a key), the
**country** noted by the collector at the edge from the connection and never
the address, sent *after* the first frame — and again at any **turn start** that
opens a new UTC day, so a session left open across midnight still counts,
bounded to one attempt per day per session so a refusing collector is never
retried per turn, **and once more on the first launch after an update**,
whatever the day, since the version is one of the fields (`last_ping_version`
recorded beside `last_ping_day`, `telemetry::should_ping` keyed on both; the
collector's `(day, id)` row is an upsert, so that ping moves the install onto
its new version instead of being dropped with a `204` — the "updated but the
dashboard still says 0.1.0" bug had both halves) — on a detached worker whose
only report is the delivered day and version (`telemetry::Delivery`), which
the **loop** records (the worker never
writes the file) in `telemetry.json` — its own per-**user** file beside the
install id, the one row not in `settings.json`, since an opt-out that applied
to one directory would be a surprise — disclosed once under the banner through
the themed, wrapped `ui::telemetry_notice_lines` card and stated in full in the root
**`TELEMETRY.md`** — the user-facing half (what leaves a machine, the three
off switches, what the server keeps, how to verify it), which the README
deliberately does not duplicate and which moves whenever `docs/telemetry.md`
does — every path to a ping routed through the one
`Session::telemetry_tick` so the notice is committed **before** any send (the
`/settings` path having shipped a ping with `notice_shown: false` when the two
call sites each had to remember), off with the row,
`ALTER_ZERO_TELEMETRY=0` or `DO_NOT_TRACK=1` (which outranks it) — a
forbidding environment **withdrawing the row** rather than seeding it, alone
among the overrides, since a row that cycles back on is not an opt-out — and off
outright without a config home (nowhere to keep an id, and a fresh one per
launch would count one person as many); the collector is the Cloudflare
Worker + D1 in `telemetry/`, tested with `node --test`, validating every field
(and still accepting payload `v1`, so bumping the shape never stops counting
an install that has not updated) and serving a users-per-day / per-country /
per-platform (`ubuntu 24.04`, `macos 15.3.1`) dashboard at `/`; `smoke.sh` runs
every phase with `ALTER_ZERO_TELEMETRY=0` and Phase 115 drives the whole loop
against a local stub)
of `{label}  {value}` in an aligned column over a `(n/total)` counter, the
highlighted row's description, and a `Type to search · Enter/Space to change ·
Esc to cancel` hint; every value **cycles** — there is no free-text field, so
Enter and Space mean the same thing on every row and Space never reaches the
type-to-search (which matches the label *and* the description, so `agents.md`
finds **Project docs**). The pure model is `settings::SessionSettings` +
`SettingKey`; the rows are **derived, never stored** (`App::setting_rows`),
so the value column can't drift from what the session is doing, and
**Permission mode** is a second door onto `App::permission_mode` — cycling it
returns the existing `Action::SetPermissionMode` so Shift+Tab's whole path (the
gate, the covered-request sweep, the per-project persist) still runs. A knob
the host can't serve is **unavailable** — `SettingAvailability`, injected at
the boundary like the clock: it renders `false (unavailable)`, refuses to
cycle with an explanatory toast, and is never persisted. Everything else
returns `Action::SettingChanged(key)` and `tui::settings::Session::apply_setting`
does the work: **Tools**/**Error retry**/**Temperature**/**Max tool calls**
rebuild the backend (`ModelSession::set_tools`/`set_max_retries`/
`set_temperature`/`set_max_tool_calls` — the `/model` switch's full-attachment
rebuild, carrying the active thinking mode forward; the retry budget and the
tool-round ceiling ride `LlmBackend::with_max_retries`/`with_max_tool_calls`
into every round, a subagent's included), **Checkpoints** flips `CheckpointStore::set_enabled`
(which can only ever turn a *capable* store on or off), **Project docs**
reloads or drops `App::user_instructions` at once, **Telemetry** writes its
own `telemetry.json` and sends today's ping if none has gone yet
(`Session::apply_telemetry_setting`, skipping the `settings.json` write), and **Hide thinking** /
**Auto compact** need nothing — they are read where they are used
(`tui::stream`'s `ThinkingStart` arm and `App::should_auto_compact`), so
there is no second copy to drift. It persists to its own
`~/.alter-zero/settings.json` — beside `config.json` and `permissions.json`,
one file per feature that owns it — **per working directory**
(`settings::SettingsFile`, `docs/per-directory-state.md`: one entry per cwd
under `projects`, each a **diff from the defaults** so only what the user
changed reaches the wire, over the file's top-level keys — the seed a
directory with no entry starts from, and the whole of a pre-directory file),
written as a **read-modify-write** over the file itself (the directory's
entry re-read, `SessionSettings::copy_value` moving across only the cycled
key, `SettingsFile::record_value`) so an `ALTER_ZERO_*` override merged in at
startup can never *stick*: the environment wins for the run, per setting,
and only the row the user actually cycled is saved. **Hooks** and
**Checkpoints** default to `false` — each runs code on the user's behalf, so a
directory opts in. The `/model` selection is per directory the same way
(`llm::settings::Settings`'s `projects` map over the top-level **last**
selection, which a directory launched in for the first time adopts and pins
as its own at startup — `ModelSession::resolve` via `config::adopt_selection`
— so a switch elsewhere never moves it; `switch_to` records the directory's
entry *and* the last selection, `persist` only the directory's own pair) — **and
the session's own on top of that** (`docs/session-model.md`: the rollout
gains an append-only `model` record — `llm::settings::ModelSelection`, the
`config.json` entry's own shape — written with the file, on every `/model`
pick, Ctrl+T cycle and probe answer, the newest winning
(`session::parse_model`, the `parse_checkpoints` sidecar's twin), that
`/resume`, `--resume` and `--continue` all hand to `ModelSession::restore`
through the one `Session::restore_session_model` — a `/model` switch minus
the `config.json` write, an `ALTER_ZERO_MODEL`/`ALTER_ZERO_PROVIDER` pin
outranking it (`llm::settings::env_outranks`, the startup precedence asked
again), a provider this machine has no key for keeping the current model
under a red `Can't resume on …` toast and leaving the record alone — so the
directory's entry is only what a *new* session starts on, and two instances
in one directory each keep, and each resume, their own model; `smoke.sh`
Phase 118) — and so are the **`/mascot` and `/spinner` looks** (`mascot.json`/`spinner.json` each gaining `config.json`'s `projects` map over the last choice, a directory pinning that last at its first launch and a choice made in it becoming its entry *and* the last, through one pure `app::LookFile<T>` shared by the two twin catalogs via the `app::Look` trait — `tui::config::adopt_look` at bootstrap, `save_look` from the pickers' Enter, `docs/per-directory-state.md`, `smoke.sh` Phase 114).

### The runtime model and its invariants

This is an **inline** TUI: finished messages *and tool calls* flow into the
terminal's real scrollback; a live region (a rule-framed input box — a codex-style
**`textarea`** whose cursor moves anywhere (←/→ by grapheme, ↑/↓ across *wrapped*
rows, Home/End — plus the readline set, `docs/textarea.md`: Ctrl+A/E/B/F/P/N,
Alt+B/F and Ctrl/Alt+←/→ word motion, the placeholder-atomic kills
Ctrl+W/U/K + Alt+D/Alt+Backspace, and Ctrl+H backspace) with insert/delete
at the cursor, growing as the input wraps;
from an **empty composer (or an unedited recall) ↑/↓ instead step through
previously submitted inputs** shell-style (`App::input_history`, codex's
`ChatComposerHistory` — ↓ past the newest clears; **persisted across sessions**
in an on-disk `history.jsonl` seeded at startup, `docs/history-persistence.md`;
see `docs/input-history.md`),
and **Ctrl+R reverse-searches them** codex-style (`App::history_search` — the
footer slot becomes a `reverse-i-search: {query}` line owning **every** key,
the newest case-insensitive substring match previews in the composer with the
query occurrences highlighted, Ctrl+R/↑ and Ctrl+S/↓ step older/newer clamping
at the ends, Enter accepts the preview as an editable draft seating ↑ at it,
Esc/Ctrl+C cancel restoring the pre-search draft and cursor; see
`docs/history-search.md`) —
plus, *while a turn is in flight*, a strip above it — a streaming preview row (the
preview shows a running tool's cell when one is executing — its bullet a
**breathing grey**, `docs/tool-pulse.md` — a backend tool's
**whole** collapsed cell, the wrapped `● name(args)` header *plus* its output;
before any output a `⎿ Running…` row, and once a `bash` command **streams** it
**tails** its output — the last `TOOL_PEEK_ROWS` display **rows**, long lines
word-wrapped like the Ctrl+O view (`ui::wrap_output` — never clipped at the
width, spaces preserved), + a
`+N lines (Ns)` footer counting the fully hidden source lines, its `(Ns)`
the command's own runtime (`App::command_elapsed`, never the turn's)
(`ui::running_command_lines`, `docs/tool-streaming.md`) — so a long
command isn't clipped and the running state shows; a **parallel
batch** previews the *whole* `tool_queue` — the running call over each dim
`⎿ Waiting…` sibling, blank-separated, `docs/parallel-tools.md`; the preview slot
is sized by `ui::preview_rows`, a running `!` shell/streaming reply staying one
row; `docs/tools.md` — and **budgeted**, since it is the region's only elastic
row: `ui::preview_budget` is what the terminal has left once every other row
the region owes is paid (the status slot, the queued messages, the toast, the
box, the band, the footer, the agent roster), `ui::fitted_preview_rows` is the
content's ask clamped to it — the one count `live_height`/`live_layout`/
`input_box`/`cursor_position`/the paint all size by, with `preview_lines`
trimming the built rows to match (from the front for a reply's frontier, off
the end for live cells) — and `live_layout` holds the box's `LIVE_MIN_HEIGHT`
back **first** as the backstop. A **fixed** allowance was the reported bug:
pressing `/` under a forming table on a small terminal asked for rows the
terminal did not have and the constraint solver spent them on the strip, so
the textarea vanished until the turn ended, `docs/table-streaming.md` *The
preview slot is budgeted*. What the budget could not afford it used to
**throw away** — a 13-row terminal lost a running command's `+N lines (Ns)`
footer and its ctrl+b hint, a 9-row one its whole `⎿` output block, a 4-row
one the spinner status line, none of it in any buffer — so the strip is one
line builder now (`ui::live::strip_lines`, the exact rows `strip_rows` +
`queued_rows` + `toast_rows` reserve) painted through the framed views'
**bottom anchor**, and the rows it cannot paint **flow into real scrollback,
frozen**: the cell *scrolls*, keeping its newest rows and its footer on
screen while its head stays readable by scrolling up. Reserved rows are
untouched (`fitted_preview_rows` still sizes the region, so the composer is
protected exactly as before) — only the strip's *content* is built at the
full ask (`strip_content_preview_rows`). Frozen for the band's reason: the
strip moves at the turn's own 32 ms cadence, so it signs
`ui::live::strip_flow_key` — what the strip is *of* — never its rows. A
streaming reply's **frontier** is the one exclusion, since `StreamRender`
commits its completed lines already and flowing it would re-sign per chunk;
and the flow is the composer path's alone, a composer-replacing view's own
page being what flows there. `docs/strip-flow.md`, `smoke.sh` Phase 106), a blank gap row,
a codex-style **status line** (`⣤⣀⣀⣀⣀⣀⣀⣀ {verb}… ({elapsed}s · {↓|↑} {n} tokens ·
Thinking for {m}s · esc to interrupt)` — opened by the session's spinner
style (`docs/spinner.md`; by default `gravity`, a ball hopping along a
braille track, and before the catalog always the comet: a
Larson-scanner sweep, a white head dragging a fading grey tail back and forth
between dim walls), the verb text
shimmering with a white sweep ported from
codex's `shimmer_spans`; on finish a dim `{done verb} for {n}s` summary commits to
scrollback, while **Esc mid-turn interrupts** instead (codex-style — cancel + reap
the backend, drain the channel, keep the partial, resolve a running tool as
failed, commit the red `Conversation interrupted` notice, **no** summary — but
when **nothing had streamed** (no partial, no tool, empty queue) it instead
**undoes** the submission, the message back in the composer and no notice, and a
**`!` shell interrupt** commits no notice either (its `⎿ Interrupted by user`
cell is the record); see
`docs/interrupt.md`; **idle Esc instead arms the Esc-Esc backtrack** — a second
Esc previews previous user messages in the transcript overlay and Enter rewinds
the conversation to the highlighted one, its text back in the composer
(`App::backtrack`, codex's `BacktrackState`; see `docs/backtrack.md`) — Esc
quits only with an empty composer and no user message to backtrack to, a
typed draft making it a codex-style no-op) — see `docs/status-indicator.md`),
then another blank gap row so the
status clears the box's top rule — plus a
scrollable **slash-command palette** band *below* the box when the input is a bare
`/token` (the same slot shows a **`?` shortcuts band** — codex's footer shortcut
overlay, two dim columns of `{key} for {thing}` entries — when `?` is pressed in
an empty composer; any other key dismisses it, Esc dismiss-only; see
`docs/shortcuts.md`; **and the same slot shows an `@` file picker** — a fuzzy
file list — whenever the cursor is in an `@token`: the boundary's background
worker walks the cwd **afresh per query** and ranks it off-thread (so a file
the agent just created appears immediately — never a startup-cached index;
codex's `StartFileSearch`/`FileSearchResult` round-trip — `App::file_search_query`
changes drive a `dispatch_file_search`, results come back via
`App::set_file_matches` with a staleness guard), the rows **columned**
codex-style — `→ name  parent/  File|Dir`: the selected row's `→` marker, the
name column sized to the widest visible name, the parent dir (`./` at the
root), and the kind label pinned at the right edge, at most 8 rows — ↑/↓ move
and **Tab/Enter insert
the path** (replacing the `@token`, a trailing space added, whitespace paths
quoted), Esc dismisses sticky-per-token; the matched characters are bolded in
each row (remapped across the name/parent split); suppressed in `!` shell mode
and mutually exclusive with the palette;
see `docs/file-search.md`; **and the same slot shows a `$` skill picker** —
codex's skill mentions — whenever the cursor is in a usable `$mention`: the
discovered skills fuzzy-filtered on their names (`skills::mention_token` +
`rank_skills` over the boundary-injected `App::set_skills` snapshot — the
registry's *enabled* skills, synchronous, no walk), rows columned
`→ name  description` (widest visible name + gap, matched characters bolded,
the description `…`-cut at the width), at most 8 rows; the grammar is codex's
— the `$` must open its whitespace-delimited word (`US$5` never triggers),
`[A-Za-z0-9_-]` continue the name so `$dataviz,` still queries `dataviz`, a
bare `$` lists everything, and shell-flavored queries (`$1`, `$-`/`$_`, the
well-known uppercase `$PATH`-style names) stay closed, as does `!` shell mode
wholesale; ↑/↓ move, **Tab/Enter insert `$name `** — the sigil kept, an
existing following space reused rather than doubled — Esc dismisses
sticky-per-mention, and a submitted message carrying `$name` makes the model
**load that skill** via the ordinary `skill` tool (its description names the
mention syntax, so the green `● Skill(name)` cell, the
context replay and the rollout all come for free; deliberately *not* codex's
eager `<skill>` injection, which exists because codex has no skill tool);
see `docs/skill-mentions.md`); plus, *above* the box while a turn streams, **messages
submitted with Enter go into the turn already running** instead of waiting
(shown like sent user messages, inset two columns — `  ❯ {msg}` rows in the
strip under the status line — until the model's **next round boundary** takes
them, right after that round's tool results, which is codex's steering: the
loop pushes onto a shared `steer::SteerQueue`, `llm::agent::run_agent` drains
it at the top of every round through its typed `PendingInput` seam (background
notices lead, the user's own messages close — a completion is a *result* and
belongs with the results it follows, while the newest thing said must be the
last thing read), and `StreamEvent::Steered` announces each take so the pending
row becomes a **real user bubble** — `App::deliver_steered` finalising the
assistant run ahead of it (invariant 4), recording the message and counting its
`↑` tokens — while **Tab** instead opens a new `QueuedTurn` batch (codex's
Tab-to-queue — its message runs as a *separate follow-up turn* after the
running one, a blank row dividing them) in the classic `App::queued`
`VecDeque` of **typed entries** (text `Messages` batches and standalone `Shell`
commands, one dispatched per turn end — a text batch to the model, a queued
`!command` run locally). A turn that ends **before** reading what was handed to
it hands it back (`App::reclaim_steered` at every turn-end site, so the front
of the follow-up queue carries it as the next turn) — which is why nothing
typed is ever dropped, why Enter's old batching survives as the fallback, and
why **Esc interrupts and sends what the turn never read right away**; Alt+Up
pulls the **last follow-up batch** (its messages newline-joined) back into the
composer to edit, and with none left reaches the running turn's own messages
through the boundary (only the shared queue knows whether one can still be
taken back). **A subagent session view has the same queue over that agent's own
loop** — same seam, same event, same rows, the registry (not the roster's
one-event-behind status) deciding whether a message is queued or starts a chat
continuation (`AgentChatDelivery`), the run's **own thread** reconciling the
settle window at `finish` (a slot stays `busy` between its loop's last drain
and `finish`, so a message typed there is accepted and read by nobody —
`has_pending_inputs` is the guard and the thread starts the continuation
itself, which a `Steered` echo then folds onto the **settled** entry,
reopening it, since otherwise that continuation runs invisibly), and a
delivered message opting the turn out of the Esc-interrupt undo
(`steered_this_turn` — the undo reads the history tail, which a delivery
makes a user message again) — **and the same Tab**, which queues a **follow-up
turn for that agent** (`AgentRun::followups`, rendered below its steered rows
and blank-divided from them) that `Session::dispatch_agent_followups` starts as
its own chat continuation once that loop settles, one entry per settle, asking
the registry (`ReplySource::agent_ready_for_turn`) rather than the roster
before it hands one over so a follow-up can't be folded into the settle-window
continuation as a steer; a stopped agent drops its follow-ups with its steered
rows; and **Alt+Up** there walks that agent's own sets — its last follow-up,
then the message its loop has not read (`Action::ReclaimAgentChat` →
`AgentRegistry::take_last_input`, `SteerQueue::take_last`'s twin) — reaching
the main backlog never. Tab used to read the *lead's* `is_streaming()` and push
onto the *lead's* `queued`, so a message typed into a subagent's session ran as
a follow-up turn of the **main** conversation. **And the pending rows are
memoized** (`ui::footer`'s `with_queued_lines`, keyed on a content fingerprint
of the width, whose session is on screen, and both pending sets — content
rather than a mutation counter because the queues are plain fields the whole
crate writes directly): they are reached six or seven times per draw and each
build word-wraps and styles the whole backlog, so with the 32 ms frame chain
re-arming, a growing queue burned 15% of a core on a screen holding still —
fifty ~530-character messages measured **75 CPU ticks per five idle seconds
against a 6-tick empty-queue baseline, now 13**; see
`docs/queue.md`; plus **`!command` runs a local shell command** (codex's `!`
shell mode: a leading `!` is **absorbed** into `App::shell_mode` and rendered
back as the composer's red `! ` prompt — `! pwd`, never `❯ !pwd` — with a red
`Shell mode` hint in the footer slot (Backspace/Esc on the empty shell composer
exit the mode; the palette/`?` band are suppressed in it); Enter from an idle
composer runs the draft under `sh -c` on a background thread as a turn
(`App::begin_shell`; every shell child — this, the model's `bash`, a
background launch — spawns **detached from the controlling terminal** via the
shared `subprocess` module's setsid detach chain, so a `/dev/tty` password
prompt like `sudo`'s fails fast instead of printing over the TUI and fighting
the loop for the keyboard, `docs/tools.md`) committing a codex-style **exec cell** — the `! command`
header on the dark user-style line (a `Role::Shell` message) with the `⎿`
output **flush** below, `⎿ Running… (Ns)` while it runs (the elapsed rides the
preview — a shell turn **hides the spinner status line** entirely,
`ui::strip_has_status`), **no** `Ran for Ns` summary, Esc killing the child
(resolving `⎿ Interrupted by user` with **no** `Conversation interrupted`
notice — the cell is the record);
mid-turn it queues as a standalone `Shell` entry run locally when its turn comes
— codex parity, never merged into a text batch, Alt+Up over it re-enters shell
mode; see `docs/shell-command.md` and `docs/queue.md`); plus a **transient toast**
— a one-row, self-clearing status line pinned at the *bottom of the strip, just
above the box's top rule* (`App::toast`/`ui::toast_rows`/`toast_line`, threaded
beside `queued_rows`; dim for info, red for errors). It's raised for
confirmations and soft rejections the user should see but never keep — `/copy`
(`Copied last message to clipboard`), a model switch, a mid-turn `/resume`/`/help`
rejection — instead of committing a scrollback bullet; it never enters `history`,
and its expiry is timed at the boundary (`Session::toast_deadline` +
`Session::toast`, the timestamp pattern, cleared in the draw tick). See
`docs/toast.md`; plus a one-row
**session footer** on the region's last row —
codex's footer status line, `{model} · {cwd}` dim and two-space inset
(`dummy_model_name · ~/repo      manual` — the Shift+Tab **permission mode**
pinned flush at the row's right edge, `docs/permissions.md`, hidden when
permissions are off; a reasoning-capable model carries its Ctrl+T
thinking mode beside the name — `{model} {mode} · {cwd}`, `docs/reasoning.md`)
— whenever no band is open (the palette/shortcuts
band displaces it, and the Ctrl+R search line / `!` shell-mode hint take its
slot; `App::set_session_info` injects the strings at the boundary
like the clock, the model name coming from `ReplySource::model_name`; see
`docs/footer.md`) stays pinned at the bottom. The alternate screen hosts the
full-screen overlays: the **Ctrl+O tool-output view**, a full-screen overlay listing every tool
call's complete output while the conversation keeps streaming underneath (see
invariant 4), the **`/resume` picker**, and the **Ctrl+D context-debug view**
(the raw LLM context window — the derived conversation the real backend sends
each turn, tool calls in the provider-native `tool_calls`/`tool` wire format
(an assistant `→ name(args)` request + a `tool:` result entry) and `[Image #N]`
placeholders unrendered; `ui::render_context_view`/`ui::context_lines` over
the pure `context::context_messages`, see `docs/context.md`) — whose **Tab** opens its
second page, auto mode's reviewer's own window: the bounded task context the
classifier reads before each command, pulled live from the backend per draw
(`ReplySource::classifier_context` → `App::set_classifier_context`, so it
tail-follows actions as they land) over a note saying whether the mode
actually consults it (`ui::classifier_lines` under the same
`ui::render_context_view` chrome, `docs/permissions.md`). ratatui's `Viewport::Inline` can't change height after startup, so
`term::InlineViewport` is a *custom* inline viewport over a `CrosstermBackend`
whose height is **dynamic** — the input box grows with the wrapped input, the
streaming strip, the palette band, and the session footer (`ui::live_height`). Four non-obvious
invariants hold the whole thing together — breaking any one reintroduces a class
of bug:

1. **One stdin reader, created after the init cursor query.** The async loop reads
   keys from a single crossterm `EventStream`; the reply backend (a
   `stream::ReplySource`, e.g. `DummyAi`) streams on a background thread that *only
   sends* `StreamEvent`s on a tokio channel. `InlineViewport::init` queries the
   cursor position (DSR) over stdin *once, synchronously, before the `EventStream`
   exists*; a second stdin reader would steal that reply and cause "cursor position
   could not be read". So: create the `EventStream` only after init, and never add
   another thread or task that reads stdin (`insert_before` tracks the viewport row
   itself and never queries the cursor). **Relatedly, the detached-exec hook
   (`subprocess::run_detached_exec_if_requested`) must stay the *first statement*
   of `main()`** — before the tokio runtime and any terminal I/O: in a helper
   re-exec (`{exe} __alter-zero-detached-exec {cmd}`) the process must `setsid`
   away and `exec` `sh` before it ever touches stdin/stdout or spawns a thread, or
   it would boot a TUI into the caller's pipes and the tty detach would silently
   break (`docs/tty-detach.md`). Never move it, and never let anything run above it.

2. **Greedy word-wrap is prefix-stable** (`ui::wrap_text`): appending text only
   ever changes the *last* wrapped line. This is what makes streaming-to-scrollback
   safe — the **incremental** `ui::StreamRender` flushes every completed line to
   scrollback via `term::InlineViewport::insert_before` as the reply grows,
   *caching* the rendered rows of already-complete source lines and only rendering
   the newly-arrived tail (so streaming a reply is **O(reply)**, not O(reply²) — it
   replaced a per-chunk *whole-reply* re-render that starved the status animation on
   long code; see `docs/markdown.md`). `StreamRender::commit` withholds the
   still-growing last line (and, inside a fenced code block, the **whole**
   in-progress line — a code line's colour isn't final until its closing `(`/`//`
   streams in, and it may span several wrapped rows), plus any **trailing blank
   rows** (a model's `…\n\n` before a tool call — trimmed so they don't stack on
   the boundary's single spacer into three blank rows; the batch `assistant_lines`
   trims them too, both gated on `!in_code` so a blank inside an open fence
   survives), `StreamRender::preview` renders
   just that last line for the strip (cheap enough to redraw every animation frame),
   and `StreamRender::finish` flushes the remainder on `StreamDone`. The tests
   `ui::tests::incremental_commits_reconstruct_the_whole_reply`,
   `stream_render_is_prefix_stable_over_every_prefix`, and
   `streamed_code_never_recolours_a_committed_row` lock this property (committed rows
   + final flush == the fully-rendered message, and no committed row ever changes
   text *or* colour). Don't change the wrap algorithm — or the markdown/highlight
   line renderers (`AssistantRenderer`, driven by `markdown::BlockScanner` +
   `highlight::Highlighter`) — without re-checking those invariants.

3. **The viewport is content-anchored (top fixed), and resize reflows both
   directions** (`tui::view::Session::repaint_conversation`). Like Claude Code / codex, the
   box grows *downward* in place — `term::draw` keeps its top put and only scrolls
   the screen *up* (oldest chat into scrollback) once the box would overflow the
   bottom; a shrink blanks the rows it vacates (the decision is the pure
   `ui::repin`). Never force it to `screen.height - height` — that reintroduces
   the "box jumps to the bottom" bug. **One view closes differently: an inline
   *modal*** — the tool-permission prompt, `ui::region_is_modal`, the one region
   that can be as tall as the terminal. It *grows* by the same `ui::repin` as
   everything else — the chat it displaces scrolls into the terminal's real
   scrollback, staying reachable while it asks (`docs/permissions.md`) — but
   those scrolls are one-way, so a plain shrink at the close would strand the
   box above the rows it vacates. `InlineViewport` notes every one-way move
   under an open prompt (`modal_scrolled`, set by `paint_live`'s scrolls and by
   any `reflow` run while the prompt is open), and the first draw after the
   prompt closes consumes the note with a purge rebuild — box flush at the
   bottom, scrollback rebuilt from history, nothing lost or doubled
   (`smoke.sh` Phases 58/60/62). A shrink **while the prompt is still open**
   gets the same answer *before* the paint (back-to-back prompts of different
   heights): `ui::modal_needs_rebuild` rebuilds when the frame's plan —
   `view_top` + pending rows + the new height — would seat the region short
   of the screen bottom the last frame was *painted* flush against
   (`InlineViewport::painted_bottom`; the tracked height re-syncs between
   paints, so it can't serve), since painted in place it would strand the
   open prompt above that same blank band for as long as it asks (Phase 63).
   The streaming strip (preview + gap + status
   + gap) sits *above* the box, so it grows the region upward; when a reply ends the
   strip's rows become the committed final line + spacer + the `Done for Ns` summary
   and the box must **stay put**, so `StreamDone`/`Error` call `term::set_view_height`
   to reseat the viewport to its idle height *before* the final `insert_before`s —
   the queued lines flush with the *latest* tracked height, so skipping the reseat
   makes the flush over-scroll, the box rise off the bottom, and blank rows appear
   beneath it (guarded by `smoke.sh` Phase 5; and because a strip collapse can now
   coincide with a flush *mid-stream* too — a forming table's whole block commits
   at its close while the multi-row preview drops to one row — `term::paint_live`
   syncs the tracked height to the frame's height before `flush_pending` whenever
   lines are pending, `docs/table-streaming.md`, guarded by Phase 41). (`insert_before` itself only
   **queues**: the next `term::draw` writes the lines and repaints the live region
   inside one synchronized update, so a commit can never flash a boxless frame —
   `docs/flicker.md`, guarded by `smoke.sh` Phase 15.) On **any** size change the
   conversation repaints from source — a width change stales every wrapped line, and
   a height-only change moves the screen contents out from under the tracked
   viewport row (the emulator scrolls/clips to fit; repainting at the stale row
   leaves phantom input boxes — `term::resized` re-clamps the viewport like codex,
   and the repaint reseats it; guarded by `smoke.sh` Phase 17). `App` retains a
   `history: Vec<HistoryItem>` of finished messages *and
   tool calls* (kept for two reasons: this repaint, and listing tools in the Ctrl+O
   view) and `term::reflow` **purges scrollback + clears the whole screen** first
   (codex's `clear_scrollback_and_visible_screen_ansi` — `ESC[2J` then the `ESC[3J`
   scrollback purge, emitted as one ANSI write), seats the viewport at the top,
   writes the re-wrapped **full** history (`RESIZE_REFLOW_MAX_ROWS`-capped,
   `ui::repaint_lines`) into the blank screen — `write_above` scrolling the
   overflow back into the now-empty scrollback — and paints the live region below
   it, all in the same synchronized frame (stale queued lines are dropped: the
   tail regenerates them). Every full rebuild goes through it: `/clear` (truly
   wiping scrollback — scrolling up shows nothing, not the old chat), every
   resize (the purge is what stops the emulator's own reflowed copy of the old
   rows surviving — the TUI-text duplication an in-place overwrite showed on a
   width change; guarded by `smoke.sh` Phases 16 and 17), and every history
   rewind (a backtrack, a `/resume` load, an interrupt-undo, a blocked prompt).
   An **ordinary overlay return is deliberately *not* one of them**: everything
   that committed while the overlay was up sits in the viewport's pending queue
   (invariant 4), so the return is one ordinary `draw` — the flush writes the
   backlog above the live region (overwriting the stale strip in place, no tmux
   spill) and the terminal's own scrollback survives untouched
   (`Session::overlay_return_repaint`; a resize that landed *while* the overlay
   was up is the exception that forces the purge rebuild — `overlay_resized`,
   the emulator reflowed the main screen underneath and the queued lines carry
   the stale width). A mid-stream purge rebuild must not lose the in-flight
   partial reply (it lives in the streaming buffer, not `history`): the purge
   dropped its committed rows with everything else, so the render resets and
   the whole partial is re-committed right after via the normal `insert_before`
   pipeline, reaching scrollback exactly once (guarded by `smoke.sh` Phases 35
   and 82).

4. **Tool calls interleave with text, and Ctrl+O opens a separate overlay.** A
   tool call splits the assistant text around it: `App::flush_streaming_segment`
   finalises the run of text before a `ToolStart` as its own history message so the
   tool slots *after* it in order (the scrollback and the resize/return repaint
   must agree). Inline a tool is collapsed (`ui::tool_lines` — coloured bullet +
   one-line peek). When the model requests a **parallel batch** of calls in one
   round, they are announced up front (`StreamEvent::ToolBatch` →
   `App::start_tool_batch`, filling the `App::tool_queue` `VecDeque`) so the live
   region shows *every* call at once — the running one live (pulsing grey), the not-yet-run
   siblings as dim `⎿ Waiting…` cells (`ToolStatus::Waiting`), each committing to
   scrollback as its `ToolEnd` arrives. Execution stays **sequential** (only the
   front of the queue is ever `Running`, so the invariant is "at most one running
   tool", not "at most one live tool"); an interrupt/error resolves the running
   call and drops the un-started `Waiting` siblings (`docs/parallel-tools.md`). The
   Ctrl+O overlay (`ui::render_tool_view` on the alternate
   screen) is codex's **Ctrl+T transcript pager** — a slash-tiled dim
   `/ T R A N S C R I P T` title row, the scrolling transcript body with
   vi-style `~` filler past its end, a `─` separator carrying the scroll
   percentage right-aligned, and two dim key-hint rows (↑/↓, pgup/pgdn,
   home/end jump; q/esc/ctrl+o close — though Esc when idle with a previous
   user message instead *begins the backtrack preview* in place, the closing
   hint row saying so honestly (`q/ctrl+o to quit   esc to edit prev`, the
   key arm and the row sharing `App::overlay_esc_backtracks`), and while one
   highlights a message the second hint row swaps to the backtrack keys;
   `docs/backtrack.md`) — showing the **full conversation
   transcript**: `ui::transcript_lines`
   walks `history` (messages + each tool's *expanded* output) plus the live tail
   (in-progress reply / the running tool **and any `⎿ Waiting…` batch siblings**,
   the whole `tool_queue` in order) plus the still-queued backlog
   (`ui::queued_lines`' inset rows, so Ctrl+O never hides a queued message —
   `docs/queue.md`). That walk is **O(history)** and re-runs the markdown +
   syntax highlighter over the whole transcript, so the loop drives it through a
   **`ui::TranscriptCache`** (a `Session`-owned cache, like `StreamRender`) that
   builds **incrementally** (`docs/tool-view-performance.md`): committed items
   are immutable and history otherwise only grows — every non-append mutation
   (a `/clear`, a `/resume` load, a backtrack truncation, an interrupt-undo
   pop) bumps `App::history_generation` — so the cache keeps a **frozen
   prefix** of per-item rendered rows pinned on `(generation, width, cwd)` and
   each refresh re-renders only the newly committed items + the volatile live
   tail (the Esc-Esc highlight is an in-place style diff on the frozen rows,
   never a re-render). A cheap signature (generation, history length, live-tail
   length, the tool queue's shape — its length + front-call status **+ front
   output length**, so a Waiting→Running flip, a batch call committing, *or a
   running `bash` call streaming its output* invalidates it — the last is what
   makes the overlay show the **live streaming output** (unlike Claude Code,
   which only shows a tool's output once it finishes; the overlay tail-follows
   the frontier, `docs/tool-streaming.md`) — backtrack selection, width)
   short-circuits a refresh entirely, so a scroll keypress is a cache hit
   (O(viewport)) and a streamed chunk costs O(live tail), not O(history).
   `draw_tool_view` refreshes it **once** per draw (shared by the scroll clamp
   and the render); it is **retained across overlay closes** and pre-warmed at
   the loop bottom (`TranscriptCache::warm` — a no-op when nothing committed,
   the whole loaded history on the iteration a `/resume` swaps it in), so
   Ctrl+O never opens cold: the switch itself is atomic (`term::enter_overlay`
   only *queues* hide+switch+clear; the first `draw_overlay` flush delivers
   them **with** the painted frame as one write — no blank alt screen for a
   kitty cursor-trail to streak across, `docs/tool-view-performance.md`).
   **The overlay paints diffed, like the inline region** — `draw_overlay`
   emits only `overlay_paint`'s cells and an unchanged frame emits *nothing*,
   which is what keeps a mouse selection alive there while a turn streams
   (`docs/overlay-repaint.md`); the draw tick correspondingly stops re-arming
   the status animation's clock chain under an overlay, where nothing it
   animates is on screen.
   Only the **user** message shows its
   wall-clock `timestamp` (`hh:mm AM/PM`, no seconds): dim, **right-aligned on
   its own line below the message** — the *only* stamp displayed anywhere
   (AI/tool/summary stamps are recorded but never shown; never inline; the
   clock is injected via `App::set_clock`, see `docs/timestamps.md`). **While
   the overlay is up the loop keeps draining reply events into `App` and keeps
   committing — the commits merely *queue*** (`insert_before` never does I/O,
   and only the inline `draw`/`reflow`/`restore` flush the queue — never
   `draw_overlay` — so nothing can write the alt screen); the return's ordinary
   draw then flushes the whole backlog above the live region, so a turn that
   streamed — or *finished* — under the overlay keeps every row in the
   terminal (`smoke.sh` Phases 81 and 82; the retired regenerate-from-history
   return could re-emit at most one screenful, which silently dropped the rest
   — the scrollback-hole bug). The one view that truly *defers* commits is an
   open **agent session view** covering the inline screen (the one gate,
   `tui::commit::Session::commits_allowed` — its screen shows a different
   conversation, and its returns purge-rebuild from history; an open
   permission prompt is deliberately *not* on the list — a commit beneath it
   scrolls in above the region, visible at once, and the scroll it causes is
   one of the one-way moves the close's purge rebuild answers —
   `docs/permissions.md`). **Quitting from the overlay is also a return**:
   the `Action::Quit` arm must `exit_overlay` *then* `overlay_return_repaint`
   before breaking — otherwise `restore` lands on the stale streaming strip a turn
   that finished in the overlay left behind, instead of the committed `Done for Ns`
   summary (`smoke.sh` Phase 7).

### Data flow

The loop is an async (`tokio`, current-thread) `select!` over five sources —
input, reply events, draw ticks, `@` file-search results, and finished Ctrl+V
clipboard reads (each paste's read — a byte copy on Linux, a decode + encode
on the fallback path — runs on its own worker thread so the loop — and the
status animations — never block on it);
`select!`'s randomized branch order gives input/draw fairness for free. Every state
change calls `frame.schedule_frame()`; the `frame` scheduler coalesces those into a
single draw tick, rate-limited to 120 fps (`MIN_FRAME_INTERVAL`). A paste/fast-type
run is caught by `paste::PasteBurst` so its characters request relaxed frames
(`schedule_frame_in` — the rate-limit floor does the coalescing; the scheduler
keeps the soonest pending deadline). *While a turn is active* the draw branch **re-arms** the next
animation frame (`schedule_frame_in(32ms)`, codex's status-widget cadence) so the
status line's shimmer sweeps and its timer advances with no events; before each
draw the loop writes the computed `elapsed`/`thinking` `Duration`s onto the status
(`App::set_status_times`), keeping time out of the pure core (the timestamp-clock
pattern). `insert_before` only **queues** its lines: the draw tick writes them and
repaints the live region in **one synchronized frame** (codex's pending-history
pattern — no flushed state ever lacks the box, the flicker fix; `reflow` paints the
same way, and `restore` flushes any quit-before-tick leftovers; `docs/flicker.md`).
(See `docs/async-rewrite.md`, `docs/status-indicator.md`.)

```
keyboard / resize ──► EventStream ─┐
reply backend ─────► tokio mpsc ───┼─► select! ─► App::on_key / push_chunk / start_tool / set_file_matches / attach_image / set_status_times / … ─► schedule_frame
frame scheduler ───► draw-tick ────┤                                        coalesce + 120fps ─► draw / draw_overlay
file-search worker ► tokio mpsc ───┤
image-paste worker ► tokio mpsc ───┘
                                        └─ turn active? re-arm a frame in 32ms (status shimmer + timer)
```

`Submit(text)` runs `tui::turn::Session::start_turn` — a batch of one: it records each
user message (the Enter arm also records the text into `App::input_history` for
the ↑/↓ shell-style recall — adjacent duplicates collapse, `/clear` doesn't
touch it), `insert_before`s it, then spawns one reply via the selected
`ReplySource` (`backend.spawn(texts.join("\n"), images, tx, cancel)` — the
Ctrl+V image paths drained from `App::take_submission_images`, see
`docs/image-paste.md`), keeping the
thread handle + `CancelToken` so a quit mid-stream cancels and reaps it.
**Enter *while a turn is in flight* hands the message to that turn**
(`steer_draft` → `Action::Steer`, codex's `submit_user_message`) instead of
producing `Submit`: it waits in `App::steered` — shown like sent user messages
(`❯` rows) in the strip above the box — while the boundary pushes it onto the
shared `steer::SteerQueue` the backend thread drains at the top of every round,
and `StreamEvent::Steered` lands it in the conversation the moment the model has
it (`deliver_steered`). **Tab** instead opens a follow-up entry in `App::queued`
(`queue_draft(true)`), and a **mid-turn `!command` queues as a standalone
`Shell` entry** (`queue_shell`, run locally, never merged);
`tui::turn::Session::dispatch_after_turn` reclaims whatever the ended turn never
read (`reclaim_steered`, onto the **front** of the queue) and
`flush_next_queued` dispatches **one entry** (`drain_next_batch`) per turn end —
a `Messages` batch via `start_turn`, a `Shell` via `run_shell` — so unread
Enters batch into one turn while Tab follow-ups and `!` commands iterate in
order (Alt+Up pulls the **last entry** (`drain_last_batch`, `pop_back`) back
into the composer to edit — a `Messages` batch newline-joined, a `Shell` entry
as `!command` re-entering shell mode — and with none left asks the boundary for
the newest still-unread steered message (`Action::ReclaimSteered`); see
`docs/queue.md`). The
backend interleaves `StreamEvent::ToolStart{name,args}`/`ToolEnd{output,ok,truncated}` pairs
(with `ToolOutput(chunk)` **live-output** deltas streamed in between — the
running `bash` cell tails them via `App::push_tool_output`, `docs/tool-streaming.md`)
and a `ThinkingStart`/`ThinkingEnd` pair (with `ThinkingChunk` reasoning
deltas streamed in between) between `Chunk`s; the loop shows the tool
running (pulsing grey) then commits it collapsed (green/red), and flips its `thinking_start`
`Instant` so the status line shows/drops `Thinking for Ns` — while the
reasoning deltas themselves accumulate in `App::reasoning` for the live
`● Thinking…` block, collapsing at `ThinkingEnd` into the committed
`Thought for …` cell (`docs/thinking-stream.md`; with
`ALTER_ZERO_SHOW_THINKING` falsy no buffer is ever opened and they stay opaque,
counted-only, as they were). Before a tool runs, the backend also streams the model **generating** the call as
`ToolCallDelta(fragment)` events (the `name`/`arguments` pieces of a `tool_calls`
delta — `openai::Delta::tool_call`, surfaced ahead of the `ToolStart`); the loop
counts them via `App::push_tool_call_progress` (never rendered) so the tally ticks
while the call is produced, exactly like reasoning. The just-sent
**user message is counted up front** (`App::count_user_input` after
`begin_stream`, arrow `↑` — uploaded input), so the status shows `↑ N tokens`
through the backend's **pre-stream pause** (`DummyAi` waits `STARTUP_DELAY`/3s
before its first chunk so the indicator is visibly working first — overridable
via `ALTER_ZERO_STARTUP_DELAY_MS`; the strip reserves **no preview row** while
there's nothing to preview — `ui::preview_rows` 0 — so the pause is status +
gap only, no stray empty line, like codex). Then `Chunk`s,
`ThinkingChunk`s (counted via `App::push_thinking`, and kept for the thinking
stream's live block when a phase is open — `docs/thinking-stream.md`), the
tool-call generation deltas, and a tool's
output grow the cumulative token tally on `App::status` (`↓` while replying,
thinking, or generating a tool call, `↑` for the input and right after a tool — never reset); on `StreamDone` `App::end_turn`
records the `Done for Ns` summary. A backend may send `StreamEvent::Error(msg)` instead of
`StreamDone` — even mid-tool; the loop turns that into a red `Role::Error` notice via
`App::fail_stream` (which also resolves a still-running tool as failed —
`Interrupted by a backend error` — and clears the status). **Esc while the turn is in
flight returns `Action::Interrupt`** (palette-dismiss still wins; when idle Esc
arms the Esc-Esc backtrack instead, is a no-op over a typed draft, and quits
only with an empty composer and no user message to edit —
`docs/backtrack.md`): the loop cancels + detaches the backend, **swaps the channel** (a stale
`ToolStart` would wedge a phantom running tool), and `App::interrupt_turn` returns
either `Kept` — keep the partial, resolve a running tool as failed
(`Interrupted by user`), record the red `INTERRUPT_NOTICE` (**`None` for a shell
turn** — its cell is the record), clear the status with no summary — **or**
`Undone` when nothing had streamed and nothing is queued: the submission rolls
back, its message returned to the composer and dropped from history (the loop
purge-repaints, no notice; `docs/interrupt.md`). On any turn-end — `StreamDone`, `Error`, *or* the Esc
interrupt (its `Kept` branch) — the loop pops the front queued batch (`App::drain_next_batch`) and
`start_turn`s it as the next turn (the remaining batches iterate at the
following turn-ends), so **Esc sends the front batch right away**; the drain
runs **under the Ctrl+O overlay too** (codex's queue dispatches at turn end
regardless of its Ctrl+T view, the transcript following the new turn live) —
dispatching records history and *queues* the user bubbles, which the return's
draw flushes, so invariant 4 holds.
`App` (`src/app/`) is pure state +
`on_key` (dispatched per `View`); `Action`, `Role`, `Message`, `StreamError`,
`InterruptedTurn`, `ToolStatus`, `ToolCall`, `TokenArrow`, `TurnStatus`,
`TurnSummary`, `HistoryItem`, `QueuedTurn`, `Toast`, `ToastKind`, `FileSearch`, `ResumePicker`, `View` live there too
(the `@`-picker primitives `AtToken`/`FileMatch`/`at_token`/`fuzzy_match`/`rank_files`
live in the pure `file_search` module, and the `/resume` primitives
`SessionMeta`/`SessionSummary`/`meta_line`/`item_line`/`parse_session`/`preview_of`/
`relative_age`/`rollout_rel_path` in the pure `session` module — `docs/resume.md`).

Typing a bare `/token` opens a **slash-command palette** below the input box (a
third live-region band): `App::command_menu` holds the highlight, the registry
`app::COMMANDS` (`SlashCommand { name, description, effect }` — currently `/help`,
`/clear`, `/copy`, `/init`, `/compact`, `/resume`, `/model`, `/login`, `/settings`, `/theme`, `/mascot`, `/spinner`, `/hooks`, `/skills`, `/mcp`, `/trust`, `/donate`, and `/quit` — plus, right after `/model`, one row per speed tier the active model lists, `/fast`, `/ultrafast`, built from the listing by `App::commands` rather than registered, `docs/fast-mode.md`) is filtered by `matching_commands`, and ↑/↓ scroll / Tab+Enter run
the highlighted command. Descriptions line up in a column, and the selection is
shown **by colour** — the whole highlighted row lights up cyan (name *and*
description the same colour) while the others are dimmed grey, no caret. A command
dispatches an `Action`
(`/clear`→`Clear`, `/help`→`Notice(String)` committed as a `Role::System`
message — **but mid-turn `/help` is rejected with a `Toast`** (its list would
interleave with the reply; `docs/toast.md`),
`/quit`→`Quit`, **`/copy`→`Copy(Option<String>)`** — codex's `/copy`:
the pure core picks the last assistant message (`App::last_assistant_text`) and
the loop writes it to the system clipboard, arboard with an OSC 52 fallback for
headless/SSH/tmux, then shows a transient `Copied last message to clipboard` toast
(or a red `No agent response to copy`/`Copy failed` toast) — **a self-clearing
line above the box, not a scrollback bullet** (`docs/toast.md`); the clipboard write is
the I/O boundary, the `base64`/OSC 52 framing a tested pure core in `clipboard`;
see `docs/copy.md`, **and `/resume`→`OpenResumePicker`** — codex's `/resume`:
every conversation records to a rollout JSONL file (the recorder + dir scan at
the boundary, the format/parse in the pure `session` module) and the command
opens a full-screen alt-screen picker (`View::ResumePicker`,
`ui::render_resume_picker` — dense `❯ {age:12}{preview}` rows with the
selection lit on a full-width background tint, type-to-search, and codex's
Filter/Sort toolbar on the search row (`Filter: [Cwd] All   Sort: [Updated]
Created` — Tab moves the focus, ←/→ toggle, `Cwd`/`Updated` default),
Enter → `ResumeSession(path)` loading the file's history via
`App::load_session` and appending later turns to the same file; Esc clears the
query first then closes, Ctrl+C closes, mid-turn `/resume` is rejected with a
transient `Toast` (was a red `ErrorNotice`; `docs/toast.md`) like codex; see
`docs/resume.md`, `smoke.sh` Phase 31. **`/model` and `/login` (the inline
pickers, `docs/llm.md`) now open *mid-turn* too** — they only replace the
composer, never the running turn, so their old busy rejections are gone; their
confirmations are toasts (`smoke.sh` Phase 33). **And "only the composer" is
literal**: the three composer-replacing pickers (`/model`, `/login`,
`/settings`) keep the **streaming strip above themselves** — the running
tool's live cell, the `● Thinking…` block, the spinner status line, the
queued messages, the toast — exactly as the ↓ manager band does, sharing its
geometry (`ui::layout`'s `strip_above_rows` reserves the rows, `view_split`
splits the region with the view a bottom-pinned `Length` so a short terminal
squeezes the strip and not the view, `ui::live`'s `render_strip_above`
paints it, and the cursor seat comes from that same split); taking the whole
region hid exactly the turn the picker was opened beside — the reported bug,
which the band had first (`docs/llm.md`, `docs/background.md`,
`docs/status-indicator.md`; the running cell's Ctrl+B hint is suppressed
while a picker is open, the permission-prompt rule, since the picker owns
every key).
**`/init`→`Submit(INIT_PROMPT.trim_end())`** — codex's `/init`
(`docs/init.md`): the canned `prompts/init.md` prompt (generate
an `AGENTS.md` contributor guide, never overwriting an existing one) submitted as
a regular user turn — echoed as the `❯` message, recorded, checkpointed — that
the model's agentic tool loop answers by exploring the repo and writing the file;
mid-turn it is rejected with a `Toast` like `/compact` (codex's
`available_during_task = false`), it never records into the ↑-recall history,
and an Esc-undo of the turn restores the literal `/init` (palette reopened),
not the prompt. **The generated guide feeds back into the model's context**
(`docs/project-doc.md` — codex's project doc): every turn start re-reads the
project's `AGENTS.md` files (the pure `project_doc` module — nearest-`.git`
root→cwd discovery, the first of `AGENTS.override.md`/`AGENTS.md` per dir
read bytes-lossily, codex's 32 KiB cap via `ALTER_ZERO_PROJECT_DOC_MAX_BYTES`
with `0` disabling, each file rendered under its own `Contents of {path}
(project instructions, checked into the codebase):` heading — the override
captioned `not checked in` — behind the reference's override paragraph as the
`<system-reminder>`'s **instructions section** (`project_doc::instructions_section`;
codex's `# AGENTS.md instructions … <INSTRUCTIONS>` fragment is retired);
the read is
`tui::turn`'s `Session::start_turn` + a startup seed) into
`App::user_instructions`, and
`context::context_messages_full` wraps it with the listing sections
(`App::listings`) into the one block the derived context leads with
(`reminder::reminder_message`: the tags and the `Use the following contexts
and instructions:` preamble over the non-blank sections, empty when none says
anything) — in front of the post-`/compact` shape too, never entering
`history` — so the Ctrl+D view shows it and `App::estimate_context_tokens`
counts it.
**`/compact`→`Compact`** —
codex's manual compaction (`docs/compact.md`): the loop runs the summarization
turn on a one-off tools-free `LlmBackend::configure(cfg, system_prompt,
false)` (the dummy scripts a text-only summary), the reply diverts into
`App::compact_buffer` (never rendered), and `StreamDone` appends the
`HistoryItem::Compaction` marker + commits the cyan `● Context compacted`
cell; mid-turn it is rejected with a `Toast` like `/resume`, an empty derived
context with `Nothing to compact` (`smoke.sh` Phase 50)).
**`/clear` mid-turn is a kill**, not codex's
"disabled while a task is in progress" rejection: `App::clear_conversation`
wipes history, the streaming buffer, the running tool, the status, and the
queued backlog (recording no partial/notice/summary; ↑-recall survives), and
the loop's `Clear` arm cancels + reaps the backend and drains its channel —
the Esc-interrupt dance minus the commits — before the blank repaint, so
nothing can stream into the cleared screen (`smoke.sh` Phase 16). Adding a command later is a one-line `COMMANDS` entry plus an effect arm
in `App::run_selected_command`; the palette/filter/scroll don't change. **Ctrl+C
first clears a non-empty input** (codex's composer-clear: one press empties the
draft — recording it in `App::input_history` so ↑ brings it back, closing the
palette, never touching a streaming turn — and only an empty-input Ctrl+C quits;
an open Ctrl+R search wins over both: that Ctrl+C only cancels the search,
restoring the pre-search draft; the Ctrl+O overlay has no input box, so Ctrl+C
there always quits).

## Working style

Two practices shaped this codebase — TDD and design-first. Follow them as
standing instructions.

### Test-driven development (rigid — this is how the tested crates are built)

The Iron Law: **no production code without a failing test first.** Work in
Red → Green → Refactor cycles:

1. **Red** — write one minimal test naming the behavior you want, then run it and
   **watch it fail for the right reason** (feature missing, not a typo). A test
   you didn't see fail proves nothing.
2. **Green** — write the *minimal* code to pass. No speculative features (YAGNI).
3. **Refactor** — clean up with tests staying green.

If you wrote production code before its test, delete it and re-derive it from the
test. `src/main.rs` + `src/tui/` are the **only** exception (terminal I/O
boundary) — verify changes there by running the app and `scripts/smoke.sh` in
tmux, not unit tests. Every
gate (`fmt`, `clippy -D warnings`, `test`) must be clean before you call work done.

### Design before implementing (for features / non-trivial changes)

"Add X" / "build X" states *what*, not "skip the design." Before non-trivial work:
explore the existing code, ask clarifying questions one at a time, propose 2–3
approaches with a recommendation, and get agreement on a design first. Capture the
agreed design in `docs/` and keep `docs/design.md` updated when behavior changes
(it already documents the architecture and known limitations). Trivial,
self-evident edits and bug fixes with an obvious cause don't need this ceremony —
but bug fixes still get a failing test first (TDD applies to fixes too).

## Conventions

- **All styling is centralized** in `ui/theme.rs` — the glyphs and geometry
  as `const`s, and every **colour** as an accessor function of the same name
  (`tool_ok_color()`, `menu_selected_color()`, …) that reads the **active
  theme's** palette (`ui/palette.rs`, one `Palette` table of twenty-one
  roles per `/theme` entry; `docs/theme.md`). A new colour is a new role on
  `Palette` filled in every table plus its accessor; a new theme is one
  table; and a colour is never written as a literal `Color::Rgb` outside
  `palette.rs`, since a literal is a colour that ignores the theme. The
  accessors name — bullets,
  prompt, colours (including the red error bullet and the cyan system bullet),
  border, the tool-call styling (`TOOL_*` — dim-waiting/grey-running/green/red status
  colours (`tool_waiting_color()` for a batch's not-yet-run `⎿ Waiting…` calls,
  `docs/parallel-tools.md`), the
  `⎿` peek prefix, the `(ctrl+o to expand)` hint), tool-view chrome
  (`TOOL_VIEW_*`), the thinking stream (`REASONING_*` — it *borrows* the tool
  cell's `TOOL_BULLET`/`TOOL_RESULT_PREFIX` while it runs rather than owning a
  glyph, so its own consts are the dim italic `reasoning_text_color()`/
  `REASONING_TEXT_MODIFIER` the chain-of-thought renders in, the
  `REASONING_RUNNING`/`REASONING_DONE` labels, the near-white
  `reasoning_shimmer_base()` the live label's sweep rests at (codex's grey
  `shimmer_base()` would read as dim), the `reasoning_label_color()` the settled
  line takes from `status_done_color()` on **both** surfaces, and the
  `REASONING_PEEK_LINES` live tail window — see `docs/thinking-stream.md`), the transcript timestamp (`timestamp_color()` — the dim
  `hh:mm AM/PM` stamp right-aligned on its own line under the *user* message,
  the only stamp shown, only in the Ctrl+O view), the status
  indicator (`STATUS_*` — the comet style's white head + mid-grey
  `spinner_tail_color()` fading tail + dim walls and the
  `SPINNER_FRAMES`/`SPINNER_INTERVAL` animation, dim metrics, the `↓`/`↑` arrows
  and `…` ellipsis, the `STATUS_INTERRUPT_HINT` (`esc to interrupt`, the detail's
  closing clause), the dim committed-summary colour, and `STATUS_ROWS`/`STATUS_GAP_ROWS`;
  the verb's white shimmer wave is the `SHIMMER_*` consts — base/highlight
  colours, sweep period, padding, band half-width, max blend — a port of codex's
  `shimmer_spans`; the verbs themselves are `WORKING_VERBS`/`DONE_VERBS` in
  `app/turn.rs`, picked per-turn), the
  slash-command palette (`MENU_*` — the `MENU_DESC_COL`
  description column, the cyan/dimmed colours that light up the whole selected row
  — name and description alike — and the `MENU_MAX_ROWS` cap), the `@` file
  picker (`FILE_MENU_*` — it reuses the palette's `menu_selected_color()`/
  `menu_dim_color()`, additionally bolding the query-matched characters; the
  columned row geometry is `FILE_MENU_MARKER`/`FILE_MENU_INDENT` (the selected
  `→ ` and the matching inset), `FILE_MENU_GAP` (name column = widest visible
  name + gap), `FILE_MENU_TYPE_WIDTH` with the `FILE_MENU_FILE_LABEL`/
  `FILE_MENU_DIR_LABEL` kind labels pinned at the right edge, and
  `FILE_MENU_ROOT_DIR` (`./`) for root-level parents, with a
  `FILE_MENU_MAX_ROWS` cap and the `FILE_MENU_SEARCHING`/`FILE_MENU_NO_MATCH`
  placeholder rows; `file_menu_rows`/`file_menu_lines`/`file_menu_row` mirror the
  palette helpers — see `docs/file-search.md`), the `$` skill picker
  (`SKILL_MENU_*` — only its own `SKILL_MENU_MAX_ROWS` cap and
  `SKILL_MENU_NO_MATCH` placeholder: it reuses the file picker's
  marker/indent/`FILE_MENU_GAP` geometry and the palette colours, bolding the
  matched name characters the same way, the description column `…`-cut at the
  width; `skill_menu_rows`/`skill_menu_lines`/`skill_menu_row` mirror the file
  helpers — see `docs/skill-mentions.md`), the `?` shortcuts
  band (`SHORTCUTS*` — the entry list, the second-entry column, and the cyan
  key / dim label colours), the inline `/settings` menu (`SETTINGS_*` — it
  reuses the `/model` picker's frame, indent, `❯` prompt, `→` marker and cyan
  selection, adding only the value column's geometry (`SETTINGS_VALUE_GAP`,
  sized off the widest visible label) and its two-tone colouring
  (`settings_value_color()` for a live value, `settings_value_off_color()` for the
  `SETTINGS_OFF_VALUES` — `false`/`default`/`0`/`disabled` — and anything
  unavailable), the `SETTINGS_HINT` key line, `SETTINGS_NO_MATCH`,
  `SETTINGS_MENU_MAX_ROWS`, and the `SETTINGS_SEARCH_ROW` cursor seat the
  `settings_view_lines` builder and `cursor_position` share (the page height
  is the built line count — `docs/view-flow.md`) — see `docs/settings.md`), the queued entries (the `QUEUED_INDENT` two-space
  inset, `queued_rows`/`queued_lines` — uncapped; a text `Messages` batch
  rendered by `message_lines(Role::User…)` and a standalone `Shell` command by
  `message_lines(Role::Shell…)` (the red `! ` header), so they reuse the
  user-/shell-message style, with a blank row dividing each entry from the next),
  the
  session footer (`FOOTER_*` — the two-space `FOOTER_INDENT`, the ` · `
  `FOOTER_SEPARATOR`, the dim `footer_color()`, plus the cyan
  `footer_focus_bg()`/`footer_focus_fg()` that light the ↓-focused shell-count
  segment (`docs/background.md`); `footer_rows`/`footer_line`,
  ellipsis-truncated at narrow widths, with `display_cwd` formatting the
  `~`-relative path), the transient toast row above the box (`TOAST_*` — the
  two-space `TOAST_INDENT`, the dim `toast_color()` (info) / red `toast_error_color()`
  (failure); `toast_rows`/`toast_line`, ellipsis-truncated like the footer — see
  `docs/toast.md`), the Ctrl+R search line that takes the footer's slot while
  a search is open (`SEARCH_*` — the dim `SEARCH_PROMPT`, the cyan
  `search_query_color()` shared by the bold accept/cancel hint keys, the red
  `SEARCH_NO_MATCH` notice, and `SEARCH_HIGHLIGHT` — the reversed+bold styling
  of the query occurrences in the previewed match; `search_line`, the
  query-end cursor in `cursor_position`, `highlight_row_spans`), the Esc-Esc
  backtrack (`BACKTRACK_*`/`SHORTCUTS_BACKTRACK`/`TOOL_VIEW_HINT_BACKTRACK` —
  the primed `esc again to edit previous message` hint that takes the same
  footer slot (`backtrack_hint_line`), the preview's reversed user-message
  highlight + scroll target from the single `transcript_build` walk
  (`transcript_selection`/`backtrack_scroll`), and the overlay's swapped
  key-hint row; see `docs/backtrack.md`), the `!`
  shell mode (`SHELL_MODE_*`/`SHELL_BULLET` — the red `Shell mode` footer
  hint (`shell_mode_line`) and the red `! ` that doubles as the composer
  prompt while `App::shell_mode` is on and as the `Role::Shell` exec-cell
  header bullet in `message_lines`; shell `tool_lines`/`tool_full_lines` are
  headerless `⎿` blocks — inline folded at `TOOL_FOLD_ROWS` aligned display
  rows (an output of exactly four rows shown whole), each wrapped
  (`result_row` does the corner/continuation indent; a line wider than the
  terminal **word-wraps with spaces preserved** like the Ctrl+O view
  (`wrap_output`, via `result_peek_block`)
  rather than clipping — the fold is the budget, in the unit the cell is
  *read* in, keeping four wrapping lines from costing three times what four
  short ones do; `docs/long-lines.md`) then `… +N lines (ctrl+o
  to expand)` (counting display **rows**, what expanding adds), `⎿ Running…` live, the retained output uncapped in the Ctrl+O view;
  output over `tui::shell`'s `SHELL_OUTPUT_MAX_BYTES` is **capped in memory** as it's
  read (`tui::shell::append_capped`, codex's pattern — bounds peak RSS so `! tree ~/`
  can't spike memory; the dropped tail is gone, not saved) and the expanded cell
  appends a dim `TOOL_TRUNCATED_MARKER` (`…`) when `tool.truncated` —
  kept flush by `conversation_lines`), and
  the live-region row geometry (`GAP_ROWS`/`STATUS_ROWS`/`STATUS_GAP_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`;
  the status + gap strip shows *while a turn is active*, and the preview + gap
  is added *only when there's content to preview* (`strip_rows(streaming,
  preview_rows)`/`preview_rows` — the **count** of preview content rows: 0 idle,
  1 for a streaming reply or `!` shell run, N for a running backend tool's whole
  cell (or the whole parallel `tool_queue`: every batched call's cell, running +
  `⎿ Waiting…`, blank-separated — `docs/parallel-tools.md`); the pre-stream pause
  reserves **no** empty preview row, like codex, and `preview_budget`/
  `fitted_preview_rows` clamp the count to the rows the region actually has —
  `docs/table-streaming.md`) —
  (`render_live` draws the status line under the preview's gap, or at the strip
  top during the pause) — with the
  **queued messages stacked below the status, *above* the box** (`queued_rows`,
  user-message style), the
  command palette *or* the shortcuts band forms the band *below* the box —
  `menu_rows` + `shortcuts_rows` — and the session footer takes the region's
  **last** row whenever no band is open (`footer_rows` — the band displaces
  it) — so the box's
  dynamic `live_height` is streaming-, queue-, band- and footer-aware, and idle with no band
  there is exactly one blank above the box: the committed spacer after the last
  message. `render_live` and `cursor_position` share the `input_box` helper, which
  reserves the band and footer so the cursor stays put when they open; `tool_lines`
  and `tool_view_lines` share `tool_header`). Retheme or re-size there, not inline.
- **The agent's own name is one constant** — `alter_zero::APP_NAME`. Every
  string in which the app speaks its name reads it: the startup banner's title
  (`ui::theme::HEADER_NAME`) and the `AskUserQuestion` cell's headline
  (`ask::ANSWERED_HEADLINE`, pinned to it by a test). Wording ported from a
  reference tool arrives carrying **that** tool's product name — `User answered
  Claude's questions:` was exactly that — so when you port a user-facing
  sentence, re-read it for whose name it says.
- **A tool schema's prose is short, concrete, and direct** — every word of a
  `description` rides in *every* request that offers the tool, so the model
  pays for it on each turn and a hedge buried in a clause is a hedge it may
  skim. Say the one thing the parameter is and stop: `The absolute path to
  the file to read.`, `The absolute path to the file to write (must be
  absolute, not relative).`, `The absolute path to the file to modify.` — the
  reference's own wording, one sentence, no restated fallback behaviour (a
  relative path still resolves; saying so invites one). The `read`/`write`/
  `edit` path params are pinned to that shape by a test
  (`file_tool_path_params_instruct_absolute_paths`, which checks both the
  lead-in and the length), and the same rule governs the tool descriptions
  around them: state the capability and its sharp edges, drop the padding.
- **The README is the front door, not the manual.** It answers three
  questions — *what is this*, *why would I use it*, *how do I start* — and
  stops. Everything else already has a home: the mechanism and the design
  rationale in `docs/`, the complete statement of what leaves a machine in
  `TELEMETRY.md`, what changed between versions in `CHANGELOG.md`. **A
  feature landing does not earn a README section.** The daily update check
  got a four-sentence paragraph in the install flow and it came straight
  back out: it explained a background request, an env var and an off switch
  to a reader who was two lines into running one command — the same rule
  `docs/telemetry.md` had already written down for the ping ("a privacy
  statement is a document someone goes looking for, not a section they
  scroll past"). What stays in the main flow is what the reader needs *at
  that moment* in order to act — the one-liner, and the sentence saying the
  script checksums what it downloads and names the directory it installs
  into, because trust asked for on a screen is trust earned on that same
  screen. Everything needed only *sometimes* — environment variables, a
  manual install, the slash-command table, the keyboard map, the `--help`
  block, the config paths — folds into a
  `<details><summary><strong>…</strong></summary>` block, the README's own
  established shape for it, so the page stays short while the fact stays
  reachable. Write for someone deciding whether to run this at all, not for
  someone who already has: a sentence that begins "it also…" is usually a
  `docs/` sentence.
- **Never build a `Value` tree of a body you read a few fields out of.**
  Resident memory is a feature here — the app idles in the user's terminal
  all day, and glibc does **not** return a freed tree's pages to the OS
  (thousands of small interleaved allocations coalesce into nothing), so a
  parse that spikes is a parse that *stays*. Deserializing OpenRouter's
  669 KB `/v1/models` list into `Vec<serde_json::Value>` cost **+6.5 MB
  resident, permanently**, to read seven keys per record — one `/model` open
  took the process from 16 MB to 25 MB and left it there. The shape that
  fixes it is `Vec<&RawValue>` (borrowed slices of the body, `serde_json`'s
  `raw_value` feature) decoded **one record at a time** (`entry_of`), which
  keeps peak at a single record and leaves the field-reading code untouched:
  ~0.4 MB for the same list. Measure with `cargo run --release --example
  mem_probe` (`--dom` isolates the old shape) and gate with
  `tests/model_parse_memory.rs` (its own test binary — `VmRSS` is
  process-wide, so the reading needs a test with nothing running beside it).
  Bound network reads too (`MODELS_BODY_MAX_BYTES`, the
  `SHELL_OUTPUT_MAX_BYTES` posture), and see `docs/memory.md` for the
  end-to-end numbers plus what measured as noise and stays unchanged
  (`ModelPicker::matches`' per-keystroke churn; the marginal cost of a second
  cached HTTP client — the models fetch shares the chat client for the
  thread, the pool and the warm connection, not for megabytes). The rule's
  picture-shaped twin: **never decode a picture whole to make a small one,
  and never decode what you were handed encoded** — `images::fitted` streams
  a PNG's rows into its fitted size and `clipboard::linux` streams the
  clipboard's PNG to disk, because a screenshot is the largest allocation
  this process ever makes and glibc's dynamic `mmap` threshold turns the
  second such allocation into a permanent one (three pasted screenshots
  measured a 103 MB process the other way; `docs/memory.md`). And its
  request-shaped third coat: **never allocate anything picture-sized per
  turn** — an attachment is encoded once per session and shared
  (`images::attachment`, `AttachmentUrl`), and the request body is
  serialized from the messages by reference **as it uploads**, through a
  pipe of small chunks (`llm::body::streamed_request`, every wire's request
  a typed, borrowing `BodySource`) — never built whole; a change that
  rebuilds the conversation as a `Value` per round, clones a `data:` URL
  into a `String`, or buffers the body is the wrong change,
  because each such block lands on a thread arena's heap and stays (an
  exactly-sized body freed per round measured a body's worth per arena) —
  which read as "RAM grows every message" (`tests/image_turn_memory.rs`
  gates it, `examples/image_turn_probe.rs` and `scripts/turn_mem.sh`
  measure it).
- **All width math goes through `cols()`** (display columns via `unicode-width`),
  never `chars().count()` — so CJK/emoji wrap and pad correctly. Measuring right
  is only half of it: a **wide glyph occupies one `Buffer` cell plus a blank
  shadow** for each column it covers, and ratatui only skips those shadows inside
  `Buffer::diff`. **Every** paint that hands cells to `Backend::draw` directly
  must go through `term::visible_cells` — there are three (`draw_lines` for
  scrollback + `reflow`, `blit` for the live region, `draw_overlay` for the alt
  screen), and missing any one leaves the bug alive in that view alone. Printing
  a shadow spends a third column on a two-column glyph, shifting the rest of the
  row: the emoji that tore a table's right border off the grid
  (`docs/table-streaming.md` *Wide glyphs*, `smoke.sh` Phase 41). The same
  emitter owes the **graphics protocols** two more rules (`docs/images.md`): a
  `CellDiffOption::Skip` cell is never written (the escape already painted
  those columns, and a space over them punches a hole in the picture), and the
  shadow count comes from `Cell::cell_width()`, never
  `cell.symbol().cell_width()` — an image cell's symbol is hundreds of bytes of
  escape and exactly one column on screen, and only the `Cell` impl honours the
  `ForcedWidth` that says so. `Buffer::diff` honours both itself, so the diff
  paths came for free; `visible_cells` is the one that had to learn them.
- **The input line is a `textarea::TextArea`, not a `String`.** Route all editing
  through it (`insert_char`/`delete_backward`/`move_*`/`take`/…), never raw string
  `push`/`pop`; read it with `.text()`. Its cursor is a byte offset on a grapheme
  boundary and the wrap cache is filled by the render path (`wrapped_rows`), which
  is why `App::on_key` (and so `move_up`/`move_down`) stays width-agnostic. The
  textarea wraps faithfully (preserving spaces) into byte ranges — distinct from
  `ui::wrap_text`, which is for the **assistant's markdown** and collapses
  whitespace (a user's own bubble, a `!` shell header and a `Bash(…)` header
  keep what was typed — `ui::wrap_output`). Every field that renders it sizes
  its text width through `ui::layout::text_field_width`, which keeps **one
  column for the caret**: a word that would land in the field's last column
  wraps to the next row (Claude Code's rule), so the caret always has a cell
  on its own row and the box never grows an empty row for it. See
  `docs/textarea.md`.
- **Adding a lifecycle-hook event** (`docs/hooks.md`) is *one defaulted method
  on `llm::hooks::HookSink` plus one call site*. That property is the design;
  a change that makes it untrue — a closure per event on `run_agent`, a new
  `StreamEvent` variant for a verdict that `Approval::Reject` already
  expresses — is the wrong change. Anything a hook must **block** on runs on
  the backend's own thread and **must poll the turn's `CancelToken`** on the
  20 ms cadence, or Esc silently stops working for as long as the hook takes.
- **A picture is reserved in `ui` and drawn at the boundary** (`docs/images.md`).
  Pure line builders never open an image file: they call `images::place` and
  emit marked blank rows, and `ImageStore::stamp` turns them into a picture in
  the four paint paths. A change that opens a file from `ui`, or that draws in
  three of those four places, is the wrong change — and `fit_cells` must keep
  agreeing with `ratatui_image`'s own `Resize::Fit` to the cell, which its
  differential test is there to keep true.
- **Swapping in a real AI** means implementing `stream::ReplySource` (use `DummyAi`
  as a template) and changing the single `let backend = …;` line in
  `tui::event_loop::run`. `spawn(prompt, images, tx, cancel)` hands you the text prompt
  **plus** the paths of any Ctrl+V-pasted images (`images: Vec<PathBuf>` — codex's
  `UserInput::LocalImage` typed channel; a real vision backend reads each file and
  attaches it, the dummy only acknowledges the count — see `docs/image-paste.md`).
  Stream `StreamEvent::Chunk(..)` per token on the `tokio`
  `UnboundedSender` (its `send` is sync — callable straight from your background
  thread, no runtime needed), poll the `CancelToken` so a quit can stop you, then
  send `StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure. For tool calls, optionally
  stream `StreamEvent::ToolCallDelta(fragment)`s as the model *generates* the call
  (its `name`/`arguments` pieces — counted like reasoning so the tally ticks while
  the call is produced, the text never shown), then send a
  `StreamEvent::ToolStart{name,args}`, optionally stream `ToolOutput(chunk)`
  live-output deltas while the tool runs (the running cell tails them —
  `docs/tool-streaming.md`; the real `bash` executor streams completed lines),
  then a `ToolEnd{output,ok,truncated}`
  (`truncated: false` from a backend tool — only the `!` shell runner caps); wrap a reasoning
  phase in a `ThinkingStart`/`ThinkingEnd` pair to drive the `Thinking for Ns`
  status, streaming each reasoning delta as a `ThinkingChunk(text)` in between so
  the token tally keeps ticking while the model thinks (the text is never shown —
  only counted; see `stream::turn_events` for the dummy's interleaved script). Return
  your real model id from `model_name()` — the session footer under the box
  displays it. A real backend's own first-token latency replaces `DummyAi`'s
  artificial `STARTUP_DELAY` (the deliberate pre-stream pause that shows off the
  status indicator); the loop already counts the user's input into the tally
  (`↑`) at turn start, so the status reads `↑ N tokens` until your first chunk.
  The loop and
  rendering treat chunks and tool output as opaque text, and count the status
  tokens app-side with a real `tiktoken` `o200k_base` tokenizer via the
  `app::count_tokens` → `tokenizer::count` seam — exact for OpenAI models,
  close for the rest — **as the live estimate between usage frames**: a real
  backend forwards each round's final `usage` frame as `StreamEvent::Usage`
  and `App::apply_usage` snaps the tally to the provider's own accounting
  (the `Done for Ns` summary appending `· {n} tokens ({c} cached)`), and every
  request is shaped for **prompt caching** — `llm::cache`'s `cache_control`
  breakpoints on the models that need them (OpenRouter's `~vendor/…-latest`
  aliases included), a per-session `prompt_cache_key` (+ OpenRouter
  `session_id`) for affinity — the ChatGPT backend keying on Codex's
  `session_id`/`conversation_id` **headers** instead, the body key alone
  earning it no cache reads at all (`chatgpt::session_headers`) —
  `stream_options.include_usage` in the payload, and the receipt naming both
  cache halves, `(8k cached · 1.2k written)`; every wire's cached numbers are
  the provider's own, never estimated, and `tests/live_caching.rs` proves each
  provider on the wire — see `docs/prompt-caching.md`.
  **The real `LlmBackend` also drives an agentic tool loop** (`docs/tools.md`):
  it offers the model `bash`/`read`/`write`/`edit` as Chat Completions function
  tools, and `llm::agent::run_agent` streams a round, runs the tools the model
  requested (emitting the same `ToolStart`/`ToolEnd` events the dummy scripts,
  via the `llm::exec::ToolExecutor` seam), feeds the results back, and loops
  until the model answers with plain text. The pure pieces — the tool defs +
  edit engine (`llm::tools`), the streamed `tool_calls` accumulator
  (`llm::openai::ToolCallAccumulator`), and the loop itself — are unit-tested;
  the executor's file/process I/O is boundary code. Tools are on by default,
  off via `ALTER_ZERO_TOOLS`. A `read`/`edit`/`write` cell renders its output as
  a **numbered file change** (codex's `diff_render` look in the `⎿` gutter —
  `ui/file_cell.rs`'s `file_cell_lines`): the executor emits `Wrote {N} lines to {path}`
  over the numbered contents, `Updated {path} (+A -D)` over numbered diff
  (both heads showing the cwd-relative `tools::display_path` — `../` climbs
  outside the cwd — while the header records the argument verbatim and
  **shows** it by the **path display rule** — `llm::tools::header_path`,
  `display_path`'s sibling over one lexical core, carried by the session
  policy `app::PathDisplay`, `docs/tools.md` *Path display*: relative under
  the cwd (`hello.py`),
  `~`-relative outside it but under home (`~/hello.py`), absolute elsewhere
  (`/tmp/x.py`) — applied by `ui::tool_header_lines` at render time, inline,
  in the live strip, in Ctrl+O, on the permission prompt's target row and on
  the agent rows that name a file alike, from a cwd + `$HOME` policy the
  boundary injects once (`App::set_path_display`), so Ctrl+D, the rollout,
  the classifier's action log and the permission rules keep the absolute
  path the model sent; the legacy `Created {path} ({N} lines)` head still
  parses for old rollouts)
  **hunks** (3 context lines, `⋮` between distant hunks — the pure
  `tools::render_numbered_content`/`render_numbered_diff`) — but **only on the
  cell**: a `write`/`edit` resolves through the ask tool's two-text split
  (`ToolOutcome::context` → `ToolAnswered` → `ToolCall::context_output`) and
  what the *model* reads is one line — `File created successfully at: {path}
  (file state is current in your context — no need to read it back)` /
  `File overwritten successfully at: {path} (…)` (an overwrite says so
  rather than borrowing `edit`'s verb — that the file already existed is a
  fact only the executor knows) / `The file {path} has been updated
  successfully. (…)`
  (`tools::write_ack`/`edit_ack`; a multi-occurrence `replace_all` adds
  `Replaced {n} occurrences.`, a no-op write and a failure both stay
  single-text). That clause
  is honest because every call now records the model's **verbatim arguments**
  beside the lossy header summary (`ToolCall::arguments`, carried on
  `StreamEvent::ToolStart` and taken as a **parameter** of `App::start_tool`
  rather than a `set_tool_note`-style follow-up call — a note is optional and
  conditional, while every backend call has arguments, so a second call a
  future path could forget would drop them into the silent summary fallback;
  round-tripped through the rollout) and
  `context::reconstruct_arguments` replays them whenever they parse as an
  object — so a `write`'s `content`, an `edit`'s two strings and a `bash`
  call's `timeout` all ride the *call* now instead of being rebuilt from
  `{"path": …}`, with the old per-tool reconstruction left as the fallback
  for pre-field rollouts and the `!` shell. Both file schemas close with the
  matching sentence (*do not read the file back to check it*), since a model
  that cannot see why the result shrank reaches for a verifying `read` that
  uploads the file twice; `docs/tools.md`, `docs/context.md`. Or — for `read` —
  the file numbered by `tools::format_read` in the **same** `{n:>W} {text}`
  gutter (dynamic-width numbers + a space, not the old `cat -n` tab; the UI
  synthesizes the `Read {N} lines` corner; a `read` of an **image**
  (png/jpg/jpeg/gif/webp) instead returns the concise
  `Read image ({format}, {W}x{H}, {size})` fact line while the pixels ride
  `ToolOutcome::image` as a base64 `data:` URL —
  `run_agent` attaches them after the round's tool results as a user-role parts
  message (tool-role content rejects image parts on most providers) and
  `context_messages` replays the same note on later turns from the output
  marker, the path served from the session's one shared encoding like a Ctrl+V
paste (`images::remember_attachment`, `docs/memory.md`); and a model
  whose `/v1/models` record says it **can't** see images (`ModelEntry::vision`
  — detected beside the reasoning support, riding the selection into
  `ModelConfig::vision` and `config.json`) degrades gracefully instead of
  letting the provider 404 the turn: the `read` tool declines the image with a
  recoverable error, attachments become `[image omitted: …]` notes in
  `build_messages_for`, and a Ctrl+V paste raises a red toast;
  `docs/tools.md` "Image reads"/"Vision detection"). The cell re-styles those rows — dim
  line numbers, green/red signs (`read`/`created` have none), the content
  syntax-highlighted by the path's extension, added/removed rows on
  dark-green/red background tints (`TOOL_DIFF_*_BG`), a 10-row inline peek
  (`FILE_PEEK_LINES`) with the `… +N lines` hint, everything in Ctrl+O;
  unparseable output (old rollouts, error bodies, a `read` placeholder) keeps
  the legacy rendering. And where a `-`/`+` pair is an **edit** of a line
  rather than a replacement of one, the **characters that actually differ**
  are lifted off the row tint onto a brighter one and bolded
  (`ui::inline_diff`, `TOOL_DIFF_*_MARK_BG`, `docs/inline-diff.md`): the muted
  row tint answers *did this line change*, the bright mark answers *where*, so
  `Bruce Rivera` → `Bruce Rivero` marks the `a` and the `o` instead of painting
  two flat blocks the eye has to compare character by character. The one
  constraint the whole design serves is that **only what changed is
  coloured** — the units are grapheme clusters, not words, because painting all
  of `Rivera` says the surname changed when only its last letter did; adjacent
  changed units coalesce into one run but unchanged text between two changes is
  **never** bridged (the `0`s of `8080` → `9090` stay plain), and a `1` that
  became `10` marks the added `0` alone, leaving the untouched `1` plain. The
  removed row's marked run escapes that row's `DIM` (dimming the one thing the
  eye is meant to find defeats marking it), and a pair too dissimilar to be an
  edit is deliberately left flat, since a *replaced* line has no "what changed"
  to point at and would come back speckled with whatever letters the two texts
  coincidentally share (`refine_pair`'s guard, measured in non-whitespace
  columns so a shared indent never reads as similarity). Cost is held by a
  common prefix/suffix trim plus a cheap upper-bound check before any LCS table
  is filled. It is derived from the parsed body rather than recorded, so the
  model-facing output, the rollout and the derived context are byte-identical
  and **old rollouts light up too**; both body builders share the pass, so the
  Ctrl+O expansion and the **permission prompt's preview** — where "what
  exactly am I approving?" is load-bearing — get it as well. The backend's **system prompt** is assembled from two
  `include_str!`d markdown files — the persona (`prompts/alter_zero.md`) and the
  runtime **environment context** of date/os/cwd (`prompts/environment.md`,
  folded in at the boundary via `backend::augment_with_environment` so the agent
  has context awareness) — persona → environment, nothing else: the tool
  schemas carry their own capability detail (`docs/environment.md`).

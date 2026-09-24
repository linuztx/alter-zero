# Changelog

All notable changes to Alter Zero are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A release's notes on GitHub are generated from its section below
(`scripts/release.sh notes`, see `docs/release.md`), so the entry written
here is the entry users read. Changes land under **Unreleased** as they
merge; `scripts/release.sh prepare X.Y.Z` rolls that section into a dated
release heading when a version is cut.

## [Unreleased]

### Added

- **Interactive commands.** The agent can now drive programs that need a
  terminal: prompts (`[Y/n]` questions, setup wizards, password prompts),
  REPLs (`python3`, `node`, `psql`), full-screen programs (`vim`, `less`,
  `top`) and arrow-key menus. `bash` gains a `tty` option that runs a command
  in a pseudo-terminal of its own and returns as soon as the command exits or
  stops to wait for input; a new `bash_session` tool types into it — text and
  named keys such as `<Enter>`, `<C-c>` and `<Up>` — waits on it, reads what
  it printed (or a full-screen program's screen) and ends it. Each call
  reports only the lines that are new or changed since the last one, and its
  cell streams them as they come, a progress bar redrawn in place. A
  full-screen program's screen reads as you would see it — boxes drawn as
  boxes even with no UTF-8 locale, columns and indentation intact — and says
  what it highlights, so the agent can tell which item of a `whiptail`,
  `dialog`, `htop` or `mc` menu is selected; `btop`, `ranger`, `tig`, `fzf`,
  `ncdu`, `nvim` and the rest of a 27-program sweep read exactly as tmux
  draws them. A session gets a UTF-8 locale when your environment names
  none, so `btop` starts in a bare container, and keys reach a program one
  at a time, each once it has read the last, as a person's do, so one that
  reads a key per read — `btop` again, `top` — sees every arrow and every
  letter typed into its filter, on a busy machine and behind `sudo` too. A
  program that switches screens and takes a moment to draw its first frame
  (`btop` probing a GPU) is shown once it has drawn it, not as a blank
  screen; a menu drawn on the main screen (`dialog`) keeps showing its screen
  as its focus moves; and the screen's heading quotes the line the cursor is
  on (`"File Name to Write: notes.txt‸"`), so the agent sees a prompt's
  default rather than typing it again. A screen
  that never stops redrawing (`watch -n 0.1`, `top -d 0.1`) answers within
  two seconds rather than holding the call to its timeout, and keys held
  with modifiers — `<C-Left>`, `<S-Up>`, `<M-F7>` — are sent as a terminal
  sends them. Code typed into an editor or a REPL that takes pastes (Vim,
  nano, Python 3.13) arrives as a paste, so its auto-indent can no longer
  turn a function into a staircase, while a shell is still typed to line by
  line, so `python3` and the code for it sent in one call reach `python3`;
  terminal output codes a model slips into its keys are dropped rather than
  typed, and keys it HTML-escapes (`&lt;Esc&gt;`) are still pressed. A
  progress bar or a command run under `sudo`, `ssh` or `docker run -it` is
  waited out rather than taken for a prompt — a download under `sudo pacman`
  is one wait, not a dozen — and on Linux the kernel is asked what the command is
  blocked in, so one waiting with no prompt at all (`read`, `cat`) is
  recognised and a busy one that left `Compiling… ` on screen is not taken
  for a question. A password prompt is told by the terminal itself, which
  reads it with echo off — under `sudo` too, where the kernel cannot be
  asked — so the agent hears within half a second that a password is wanted,
  and a retry after a wrong one no longer leaves it waiting out its timeout;
  what it types there still shows as typed. A password it types is answered
  in the same call — a refusal and the next prompt, or the command's first
  words — rather than with `(no new output)` while sudo is still checking
  it, and a question that comes up while the agent is deciding to wait ends
  that wait too, instead of the wait running out its timeout with the
  question already on screen. A call's header names the
  program it types into (`● BashSession(python3 ← print(1)⏎)`), above its
  permission prompt too and on a call you refused. It also
  reaches `run_in_background` commands, which can now be waited on,
  interrupted and ended the same way. A session still running shows in the
  footer's shell count, and the ↓ manager shows its live screen; its cells
  end on a dim `Waiting for input · session …` row (`Waiting for a password
  · …` at a password prompt). Typing into a session asks permission like a
  command does — covered by the "don't ask again" rule of the command that
  started it, or approved once for the whole session — and auto mode's
  classifier reviews it; waiting on a session, interrupting it or ending it
  never asks. The agent never guesses a password it was not given: it asks
  you (`docs/interactive-shell.md`).

### Changed

- **Progress bars in command output read as one line.** A plain `bash`
  command's output, a background shell's and a `!` command's are now read the
  way a terminal shows them: a `curl`, `tqdm`, `ffmpeg` or `rsync` progress
  bar redrawn with `\r` reaches the agent once, in its final state, instead
  of every frame it drew run together on one line, and colour escapes are
  gone; tabs and trailing spaces are kept. A running command's cell shows the
  bar moving in place. A background shell's `.output` file still holds the
  output exactly as written (`docs/interactive-shell.md`).
- **Error toasts use a softer red.** A failure toast — `Copy failed: …`,
  `Can't switch to …`, a config file that would not parse — was painted in
  the theme's full error red, the ink of the error bullet and a failed tool
  cell, which made a four-second status line the loudest thing on screen.
  It now wears the theme's red mixed a third of the way toward the dim that
  info toasts use: still red at a glance, but quieter than the error bullet
  (`#F38BA8` becomes `#CA89A4` on the default Catppuccin Mocha). The
  `ansi` theme keeps the terminal's own red, which has nothing to mix. The
  `/skills` picker's session-off note uses the same colour, so it is softer
  too (`docs/toast.md`).

### Fixed

- **Esc during parallel tool calls keeps every call — on screen and in the
  model's context.** When the model ran several tools in one go, an
  interrupt settled only the call at the front of the batch and threw the
  ones still `⎿ Waiting…` away: they vanished from the screen and from the
  conversation, so the next turn's model no longer knew it had asked for
  them. Every call in the batch now resolves as its own red
  `⎿ Interrupted by user` cell, in order, above the `Conversation
  interrupted` notice, and the next request replays the whole round — each
  call with the arguments the model gave it, each answered `Interrupted by
  user`. The same goes for a backend error mid-batch and for a subagent
  stopped mid-batch. This also fixes interrupted cells not reaching the
  screen at all: after Esc on a running tool the pane showed the notice
  alone until something redrew it (`docs/interrupt.md`,
  `docs/parallel-tools.md`).
- **"Don't ask again" on an MCP tool prompt no longer claims to switch to
  edit mode.** Choosing it stored the tool's rule as it should, but the
  footer then read `edit` and a `Mode: edit — file edits run without asking`
  toast appeared, though file edits still asked. The mode is left alone now,
  and the toast names the rule: `Won't ask again for mcp__… in this project`
  (`docs/permissions.md`).
- **`/copy` with nothing to copy is no longer shown as an error.** `No
  agent response to copy` was red, while `Nothing to export` and `Nothing to
  compact` — the same "nothing to act on yet" case for `/export` and
  `/compact` — were dim. Nothing has failed, so it is now a plain info toast,
  raised the same way as those two. A clipboard write that really fails is
  still an error (`docs/toast.md`, `docs/copy.md`).
- **Signing in to Ollama Cloud no longer configures the local Ollama
  provider too.** Both read their key from `OLLAMA_API_KEY`, and a key
  resolving is one of the things that makes a provider configured, so a key
  pasted for **Ollama Cloud** marked plain **Ollama** configured in the same
  keystroke: its row went `✔ configured` in `/login`, the next `/model` open
  fetched `http://127.0.0.1:11434` beside the cloud's list and painted that
  server's refusal in red, and the cloud's key rode as the bearer of every
  request to whatever `OLLAMA_HOST` named. The two providers now read two
  variables: `ollama_cloud` keeps `OLLAMA_API_KEY` — Ollama's own name for
  the hosted API's key, so nothing already stored or exported has to move —
  and the server you run reads `OLLAMA_HOST_API_KEY`, the optional bearer
  for a proxy in front of it. If you had set `OLLAMA_API_KEY` for your own
  server rather than for the cloud, rename it; an unauthorized reply now
  names both variables (`docs/ollama.md`).

## [0.6.0] - 2026-09-22

### Added

- **`/export` — the conversation as plain text, to the clipboard or a
  file.** `/copy`'s sibling over the whole transcript: the command opens a
  two-row page — `Copy to clipboard` / `Save to file` — and the pick writes
  the Ctrl+O page as text, top to bottom — the startup banner it opens
  with, then every message and every tool call's full output — at the
  terminal's width, either to the system clipboard (`/copy`'s own path,
  with the OSC 52 fallback for a headless or tmux session) or to a
  `conversation-YYYY-MM-DD-HHMMSS.txt` in the working directory, never over
  a file already there. Inside a subagent's session view it exports that
  agent's transcript; an empty conversation is a `Nothing to export` toast
  rather than an empty file (`docs/export.md`, `scripts/smoke.sh` Phase
  122).
- **A slow-stream stress rig for the offline backend.**
  `ALTER_ZERO_CHUNK_DELAY_MS` sets the dummy's pause after every streamed
  piece (the twin of `ALTER_ZERO_STARTUP_DELAY_MS`), and a prompt mentioning
  *markdown* plays a new demo: every markdown element the renderer knows —
  headings, inline styles, links, nested and ordered lists, task items, a
  blockquote, two fenced code blocks, a table, a rule — in one long reply
  streamed in token-sized pieces that split markers one character at a time,
  the way a real model's tokens arrive. `ALTER_ZERO_CHUNK_DELAY_MS=400
  alter-zero` then `stream some markdown` watches the pipeline hold still at
  a struggling model's pace; `scripts/smoke.sh` Phase 121 proves it does.

### Fixed

- **`/mcp` no longer parks the cursor on its bottom rule when nothing is
  configured.** A `/mcp` page with no server rows has no highlighted `❯` for
  the hidden cursor to rest on, so it fell to the far corner of the closing
  rule — where a terminal with a cursor-move animation (kitty and kin) flew
  to nowhere on every open. A page without a highlight now rests just past
  its closing hint, the way Ctrl+O, Ctrl+D and `/resume` do; the `/hooks`
  and `/mcp` detail pages and a hookless event's empty state follow the same
  rule, and only a terminal too short to show the hint keeps the corner.
- **A slow model no longer blinks the input box on terminals without
  synchronized output.** Every line a reply committed to scrollback blanked
  the live region before repainting it. Inside a synchronized update
  (DEC mode 2026) that was invisible, but Terminal.app, xterm and older VTE
  terminals render whatever they have parsed when their refresh comes due,
  and a commit frame of a few kilobytes can split across a pty read with
  the clear in the first half — a frame with no box, once per committed
  line, which a model streaming a few tokens a second made a blink about
  once a second. The region is repainted in place now and only the rows the
  previous region left below it are cleared, so no frame ever holds a
  boxless state, bracketed or not (`docs/slow-stream.md`).
- **The input box no longer bounces at a streamed code fence.** A closing
  fence left the streaming strip with nothing to preview, so the strip
  dropped its preview row and gap and the box hopped up two rows until the
  next line's first character arrived; an opening fence streamed a
  character at a time went from a row of prose to no row at all for the
  length of a token, a one-row hop. One frame each at a fast stream, half a
  second on every code block at a slow model's pace. The strip now keeps
  the rows it had until the next content fills them (the same rule keeps a
  table that re-lays out shorter on a narrow terminal from lifting the box),
  so the box holds still between tokens and only ever moves down.
- **Prompt caching across turns, and the token refreshes behind it.** The
  request a new human turn sent used to rebuild every earlier tool call from
  the display history — a parallel batch split into one call per assistant
  message, and fresh `call_0`, `call_1`… ids on the wires that carry an id
  through unrewritten — so the provider saw a different prefix from the one
  it had cached and re-read the conversation at full price from the first
  batch on (measured live: a follow-up turn after a parallel batch read 62%
  of its input from OpenRouter's cache before, 99.9% after). The backend now
  keeps its last
  request and reuses that exact prefix whenever the rebuilt conversation
  matches it — text, images, tool names, arguments and results alike, so a
  rewind, an edit or a compaction still send what they mean — and a blank
  line a model emits before its calls no longer counts as a difference,
  which is what had kept the Claude and ChatGPT wires from ever matching.
  The explicit cache breakpoints (OpenRouter's Anthropic and Qwen routing)
  anchor on the previous request's frontier — a tool result, not only the
  last human message — so a large parallel batch cannot push the previous
  write out of the provider's lookback. Requests that miss the access-token
  cache at the same moment (a batch of subagents, the `/model` fetch beside
  a turn) share one refresh instead of each presenting the same single-use
  refresh token, which retired it for all of them; a rotation from a rebuilt
  config repoints every older alias, and a fresh sign-in forgets every
  cached bearer of the account it replaces. The `.env` key store is written
  atomically through one serialized updater, sign-in and refresh alike —
  created private, written through a symlink to the file it names, never
  replaced with a partial file when the old one cannot be read. And a
  ChatGPT token's cached life comes from the grant's own `expires_in`
  duration rather than the bearer's absolute `exp`, so a clock running ahead
  cannot make every request re-mint (and rotate) the token.
  (`docs/prompt-caching.md`, `docs/chatgpt.md`, `docs/claude.md`,
  `docs/copilot.md`)
- **The cached prefix now survives `/resume`, a backend rebuild and a
  restart.** Every tool round's records keep the provider's own call ids and
  the batch they arrived in, plus each call's place in that batch, so the
  request a later turn derives replays a parallel batch — task calls and
  subagent launches included, in the order the model made them even where
  a subagent group's record landed ahead of the round's ordinary calls — as
  the one message the provider cached, where the fix above could only reuse
  what the running backend still held. A prompt a `UserPromptSubmit` hook blocks no
  longer costs the prefix either. And two sign-in races: signing in again
  no longer waits behind a token refresh in flight (the screen could freeze
  for the whole request timeout, and the refresh could then cache a bearer
  under the credential the sign-in had just replaced), and a refresh that
  rotates the token writes it back only while the store still holds this
  session's own token — never over one a newer sign-in stored meanwhile,
  which used to force the re-login the sign-in had just done at the next
  launch. (`docs/prompt-caching.md`, `docs/context.md`, `docs/chatgpt.md`)
- **`install.sh` reads the latest release off the redirect with wget too.**
  On a machine with wget and no curl the installer asked GitHub's API for
  the newest tag instead — rate-limited per address, and blind to
  `ALTER_ZERO_INSTALL_BASE_URL`, so a fork, or the release tooling's own
  stand-in server, was answered with this repository's newest release and
  then found no such asset where it had been told to look. Both fetchers
  now read the tag off `/releases/latest`'s final `Location`, as
  `docker/build.sh` already did, and the selftest drives the installer
  under a PATH with no curl on it. (`docs/release.md`)

### Changed

- **The agent session view's composer label is a lit chip.** The
  `── {description} ─` label on the top rule of an agent's session view
  used to be dim text embedded in a dim rule — the one row saying the
  composer feeds a subagent rather than the main conversation, and the
  easiest one to pass over. It now rides the rule on the active theme's
  accent under the on-accent ink, the ↓-focused footer chip's dress, so it
  follows `/theme` like everything else (`docs/agent-tool.md`).

## [0.5.0] - 2026-09-20

### Added

- **A headless Kali Linux container** (`docker/README.md`, `docs/docker.md`).
  `docker/build.sh` builds a small Kali Rolling image around the latest
  published release — downloaded and SHA-256-verified by `install.sh`, never
  compiled, so a build is about a minute — and `docker/build.sh ~/projects/site`
  goes on to create a container with that folder as its `/workspace`. It works
  the same with Podman: `--engine podman`. The container runs as root, keeps
  sign-ins, settings and sessions in a volume that outlives it, and publishes
  ports 8080 and 8888 to your machine for whatever you start inside
  (`docker/run.sh --port`, `--bind`, `--no-ports`). The tools are chosen by
  measured size — `git`, `ssh`, `curl`, `jq`, `rg`, `python3`, `nmap`, `nc`,
  `socat`, `whois`, `dig`, `ping`, `traceroute`, `ip`, `ss`, `net-tools` and a
  few more, about 280 MB over the Kali base with no desktop, metapackage or
  compiler — and `--with "PKG …"` bakes in your own. Opening it with
  `docker exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID
  -e TMUX alter-zero-kali alter-zero` forwards your terminal's identity, which
  is what draws real pictures in kitty, Ghostty, iTerm2 and WezTerm instead of
  half-blocks; `docker/run.sh --clipboard` forwards the desktop's Wayland
  and/or X11 socket so Ctrl+V can paste a copied image. Telemetry is left
  exactly as it is in any install, and the container reports its distribution
  as `kali`. The container is granted `NET_RAW` — as root, `nmap localhost` is
  a SYN scan, and Docker grants that capability by default where Podman 4.x
  does not, so without it the same image answered `Couldn't open a raw socket`
  on one engine and scanned on the other. It stays one named capability, never
  `--privileged` and never host networking; `docker/run.sh --no-net-raw` drops
  it. `python3` and `pip` come from a virtualenv at `/opt/az-venv` that is
  active by default in your shell and the agent's own commands, so
  `pip install` works out of the box. Kali's system Python is
  marked externally managed (PEP 668) and the image carries no system `pip`,
  so without it a Python package could not be installed at all; the system
  interpreter stays untouched at `/usr/bin/python3`, and pip caches into
  `/var/cache/pip` rather than under `/root`, so a home volume the container
  cannot write does not make every install open with a warning that the cache
  was disabled. The container `docker/run.sh` creates is named
  **`alter-zero-kali`**, with the Linux hostname **`az-kali`**. The virtualenv
  also works in login shells: `bash -l` retains `VIRTUAL_ENV` but loses its
  `PATH` entry, while `su - root` also clears `VIRTUAL_ENV` and `PIP_CACHE_DIR`.
  The shared shell hook restores missing defaults and `PATH`, preserving
  nonempty overrides. Interactive shells get `deactivate` from the image
  rather than the `/root` volume. Recreate the container using the rebuilt
  image to receive these changes while preserving home data.

### Changed

- **Image paste says why it cannot work where there is no display**
  (`docs/image-paste.md`). In a container or an SSH session Ctrl+V has no
  desktop clipboard to read, and it used to answer with whatever a probe for
  an X server ran into, sometimes after a pause. It now answers at once:
  `no desktop clipboard in this session (neither DISPLAY nor WAYLAND_DISPLAY
  is set)`, followed by what works instead — save the image where the session
  can reach it and ask Alter Zero to read its path. A display that is named
  but unreachable keeps its original cause and gains the same advice.

- **The running `Bash` cell shows its own clock and its timeout, whatever
  its output** (`docs/tool-streaming.md`). The live cell's footer reads
  `+18 lines (22s · timeout 1m 50s)`: how long the command has run beside
  the timeout it runs under — the model's own `timeout`, the tool's 2m
  default when it named none — so a long command says how much of its
  budget is left. The clock row is always there now: a command whose
  output fits the window shows a bare `(10s · timeout 10m)` row under it,
  and a command that has printed nothing counts on its `⎿ Running… (10s ·
  timeout 2m)` row instead of sitting on a bare `Running…` for as long as
  it takes. The Ctrl+B hint follows as before; the `!` shell's
  `⎿ Running… (Ns)` row is unchanged, a `!` command having no timeout.
- **A `!` shell command shows its whole output inline**
  (`docs/shell-command.md`). The committed cell used to fold after three
  rows behind `… +N lines (ctrl+o to expand)`, so reading a `! git status`
  or a `! ls -la` meant opening the transcript. Every line shows in the
  conversation now — blank lines kept, long lines word-wrapped, the
  reshaped JSON document whole — with nothing to expand. Output over the
  in-memory cap still stops where it always did, and the dim `…` marker
  that only the Ctrl+O view carried now closes the inline cell too, so a
  cut is visible where the output is read. The model's `Bash` cell keeps
  Claude Code's fold.
- **A running tool's bullet blinks instead of breathing**
  (`docs/tool-pulse.md`). The `●` on a `Bash`, `Read`, `Write` or `Edit`
  cell that is still executing used to ease between two greys once a
  second. It is now Claude Code's running dot: the one resting grey, shown
  for half a second and hidden for the next, the header text holding its
  column while the dot is away, so `Bash(…)` never shifts. The same blink
  runs the live `● Running {n} agents…` tree, a lone `● Agent(…)` cell, the
  `● Calling …` MCP cell and the `● Thinking…` header, all on one clock, and
  it works the same under the `ansi` theme, where the old breath had no
  second shade to move between and stood still. Committed cells and the
  Ctrl+O transcript keep their bullet as before; the `pulse` spinner style
  keeps its breath.

## [0.4.0] - 2026-09-18

### Added

- **Full-screen Git change review with `/diff`** (`docs/diff.md`). Review
  staged, unstaged, and untracked changes in a searchable file browser beside
  the selected file's patch. A partially staged file appears separately for
  the index and working tree, so later edits do not hide staged work. The
  review follows the active theme, with addition/deletion counts, old/new
  line numbers, colored changes, hunk navigation, and horizontal scrolling.
  Tab switches panes; narrow terminals show the focused pane at full width.
  Git reads run in the background, `r` refreshes the snapshot, and Esc, `q`,
  or Ctrl+C returns to the conversation, including replies that finished
  while the review was open. Works from repository subdirectories and linked
  worktrees, including repositories without a first commit. Binary files and
  large previews carry explicit notices. Only opens inside a Git worktree;
  reviewing never stages or changes project files.

## [0.3.0] - 2026-09-17

### Added

- **Device code sign-in for ChatGPT Codex** (`docs/chatgpt.md`). A machine
  with no browser — an SSH session, a container — could not sign in to the
  ChatGPT subscription, since its only flow handed the user a link a browser
  had to bring back to a local port. Enter on the `ChatGPT Codex` row in
  `/login` now asks how to sign in first, `Browser login (default)` or
  `Device code login (headless)`, and the device row runs Codex's own
  device-code flow: a one-time code shown beside `auth.openai.com/codex/device`
  on the same page GitHub Copilot's code lands on, counted down while OpenAI
  is polled for the approval, and the same refresh token stored at the end —
  `/model`, the ✓ marks and the next launch need nothing new. Esc from either
  sign-in page returns to the choice with the row just tried highlighted.
  `ALTER_ZERO_OPENAI_ISSUER` points both flows at another auth server (a
  fork's own, or the smoke suite's local stub, which drives the whole flow
  offline).

### Changed

- **Every speed tier a model lists is a command of its own**
  (`docs/fast-mode.md`). `/fast` was a static palette row that cycled
  standard → every listed tier → standard, and answered `does not support
  fast mode` on a model listing none. The palette now builds one command
  per tier out of the model's own `service_tiers` — `/fast`, `/ultrafast`,
  whatever the record names — listed right after `/model` with the
  backend's own description (`1.5x speed, increased usage`), so a tier the
  backend adds tomorrow is a command the day it is listed and nothing about
  a tier is hardcoded. Each command toggles its tier, codex's way: run it to
  select the tier (`Speed: fast — 1.5x speed, increased usage`, the footer
  wearing the name after the thinking mode), run it again for standard, run
  another tier's to switch straight over. A model listing no tier lists no
  such command — `/fast` typed there matches nothing — and `/help` lists
  whatever the palette shows. `smoke.sh` Phase 117 drives both a model
  listing none and a stub-served listing naming two.
- The OpenAI subscription provider is named **ChatGPT Codex** in `/login`
  and the docs (it was `OpenAI (ChatGPT)`), **its id is `chatgpt_codex`
  and its token variable `CHATGPT_CODEX_REFRESH_TOKEN`** (they were
  `openai_chatgpt` and `OPENAI_CHATGPT_REFRESH_TOKEN`): it is the ChatGPT
  seat reached the way Codex reaches it, and the old spellings read as a
  second OpenAI API-key provider (`docs/chatgpt.md`). Nothing needs signing
  in or choosing again — a token stored under the old variable still signs
  in until the next rotation writes it back under the new one, a
  `config.json` selection or a rollout's `model` record naming the old id
  lands on the provider, an `ALTER_ZERO_PROVIDER` still exporting it pins
  the same provider, and a provider file of your own spelling
  `auth = "openai_chatgpt"` still reads as the sign-in.

### Fixed

- **A picture rewritten in place shows its new bytes.** Ask the agent to
  download a picture, then to turn it black and white: the conversion
  happened, the agent read the file again, and the new inline cell still
  showed the colour version — while Ctrl+O showed the conversion. The
  encoded pictures are cached per placement, and a placement is the file's
  path and cell size, so the same file rewritten at the same size was served
  the entry encoded before the change. An encoding now remembers the file's
  size and mtime it was made from — the two facts the upload caches already
  key on — and a file that no longer matches is encoded afresh
  (`docs/images.md`, *A file rewritten in place*; `smoke.sh` Phase 107d).

## [0.2.0] - 2026-09-16

### Added

- **Fast mode for ChatGPT models** — codex's `/fast`, ported
  (`docs/fast-mode.md`). A model whose ChatGPT listing names a speed tier
  (every model on a signed-in account today: `Fast — 1.5x speed, increased
  usage`, `2x` on `gpt-6-astra`) can be switched to priority processing with
  `/fast`: the request carries `service_tier: "priority"` beside codex's
  `x-codex-routing-hint` header, the footer shows `fast` after the thinking
  mode, the toast repeats the backend's own cost statement, and the choice
  persists per directory beside the model selection (`config.json`'s new
  `speed` blob) and carries across a `/model` switch to another model that
  lists the tier. A model listing more than one tier cycles through each; one
  listing none answers `/fast` with a `does not support fast mode` toast, as
  Ctrl+T does for a non-reasoner. A subagent pinned to another model and the
  auto-mode classifier run at standard speed. Existing installs probe the
  listing once in the background at their next launch, exactly as the
  vision field did when it arrived, and record what they learn.

### Changed

- **A conversation remembers its model.** Every session's rollout now
  records the model it runs on — with the file, and again on each `/model`
  pick, Ctrl+T cycle and capability probe — and `/resume`, `--resume` and
  `--continue` bring that model back instead of the directory's current
  entry. Two alter-zero instances in the same directory can each run their
  own model: a `/model` pick in one still sets what a *new* session there
  starts on, but the other instance keeps the model it has, and resuming
  either conversation later reopens it on the model it was on. An
  `ALTER_ZERO_MODEL`/`ALTER_ZERO_PROVIDER` pin still wins for the run, and a
  recorded model whose provider has no key on this machine is reported with
  a `Can't resume on …` toast rather than silently swapped. The record
  carries the model's speed tier too, so a conversation switched to `/fast`
  comes back on it (`docs/session-model.md`).

## [0.1.2] - 2026-09-15

### Changed

- The `/resume` picker scrolls like the `/model` list: on a list taller
  than the screen the highlighted session rides the middle row, so the
  sessions above and below it stay in view and each ↑/↓ scrolls the next
  one in. It used to pin the highlight to the edge it had crossed, so
  scrolling down showed one new row at the bottom and nothing beyond it,
  and scrolling back up moved the highlight over the rows already on
  screen without scrolling at all.
- The `bash` tool's `run_in_background` now says what the flag is *for* — a
  dev server, a watch build, a full test suite — rather than only how it
  works, and notes that it defaults to false (the `agent` tool's own
  `run_in_background` defaults the other way). And both background launches,
  a command and a subagent alike, now answer with *you will be notified with
  the final output when it finishes* in place of *you will be re-invoked*: a
  result to expect rather than an internal event to brace for. Between them,
  fewer long commands run in the foreground, and fewer rounds are spent
  re-reading the interim output file for a result that arrives on its own.

### Fixed

- Telemetry now reports an update the day it happens. The daily ping names
  the app version, but its once-a-day throttle was keyed on the day alone, so
  the first launch after `alter-zero update` sent nothing until midnight UTC
  — and the collector's `INSERT OR IGNORE` dropped a same-day ping outright
  (answering `204`), so even a forced one left the day's row, and the
  dashboard's Versions panel, on the old version. `telemetry.json` now records
  `last_ping_version` beside `last_ping_day`, a launch on a different version
  pings again, and the collector's `(day, id)` row is an upsert that moves the
  install onto its new version. The install id survives an update, so an
  update is still one user, never a new install. Redeploy the collector
  (`cd telemetry && npx wrangler deploy`); no database migration is needed.

## [0.1.1] - 2026-09-15

### Added

- **Venice as a direct provider.** `/login` → **Use an API key** →
  **Venice** takes a key from your own Venice.ai account, and `/model` lists
  Venice's catalog straight from `api.venice.ai` — the same private,
  uncensored models the Agent Zero API reaches through its proxy, over the
  same request: the thinking mode still drives
  `venice_parameters.disable_thinking`, the session's `prompt_cache_key`
  still pins Venice's implicit prompt cache, and each model's context
  window, vision, and reasoning support are still read off its `model_spec`
  record. Set `VENICE_API_KEY` (or `ALTER_ZERO_PROVIDER=venice` with the key
  in `.env`) to launch on it directly.

### Fixed

- The running `bash` cell's `+N lines (Ns)` footer now counts from the
  moment the command started instead of copying the status indicator's
  turn timer, so a command launched a minute into a turn opens on `(0s)`
  rather than `(60s)`. A subagent's session view counts its own running
  command the same way, from that call's start rather than the agent's
  whole runtime.

## [0.1.0] - 2026-09-13

**Alter Zero Initial Release**

### Added

- **An agent that works in your project.** The model reads files, writes
  and edits them, and runs shell commands through an agentic tool loop.
  Parallel tool batches are announced before they run, command output
  streams live into the conversation and folds into an expandable preview,
  and long-running commands move to the background with **Ctrl+B** (or
  start there), with a manager band on **↓** from an empty composer.
- **Control over what runs.** An inline approval prompt shows the whole
  file, diff, or command before it executes, with "allow once", a session
  or per-project rule, or a rejection carrying your instructions (**Tab**).
  Four permission modes — `manual`, `edit`, `auto` (an LLM safety
  classifier reviews routine commands), `master` — cycle with
  **Shift+Tab** and persist per project. Shell children are detached from
  the terminal so a `sudo` prompt fails fast instead of hijacking the UI.
- **Checkpoints and rewind.** Per-directory filesystem checkpoints snapshot
  the working tree into an isolated git store each turn; **Esc Esc** steps
  back to an earlier message and restores the files with it.
- **Extensibility.** Subagents defined in `agents/*.md` (built-in
  `general-purpose` and `explore`), each with its own context, model, and
  tool allowlist, and an inline session view to chat with a running one;
  skills as `SKILL.md` folders with a `$` mention picker, the `/skills`
  browser, and a built-in `skill-creator`; MCP servers over stdio, HTTP,
  and SSE (OAuth included) managed with `/mcp` and `alter-zero mcp`;
  Claude Code-compatible lifecycle hooks in `hooks.json`; `AGENTS.md`
  project instructions with `/init` to write one; structured task lists
  with a live checklist; mid-turn questions to the user; a per-session
  scratchpad for temporary files.
- **Project trust.** Project-level `.alter-zero/` hooks and MCP servers are
  fingerprinted and inert until approved with `/trust`.
- **Model providers.** OpenAI-compatible Chat Completions (OpenRouter, the
  Agent Zero API, Ollama Cloud), GitHub Copilot by device-code sign-in,
  OpenAI ChatGPT by browser sign-in over the Responses API, Anthropic by
  API key or Console sign-in over the Messages API, and local Ollama over
  its native `/api/chat`. `/login` connects, `/model` picks a model, each
  provider's catalog reports context windows, vision, and reasoning
  support, **Ctrl+T** cycles the thinking mode, and prompt caching is
  requested wherever the provider offers it.
- **Context management.** A footer gauge of the context window, `/compact`
  with automatic compaction past 90%, and the **Ctrl+D** view of exactly
  what the next request carries (including the classifier's own context).
- **A native terminal UI.** Conversation flows into the terminal's real
  scrollback under a pinned composer; streaming Markdown with tables and
  syntax-highlighted code; collapsible reasoning; clickable URLs and file
  paths (OSC 8); inline images in kitty, iTerm2, and sixel terminals with a
  half-block fallback; **Ctrl+V** image paste; the **Ctrl+O** full
  transcript; a readline-style composer with **Shift+Enter** newlines,
  cross-session input history, and **Ctrl+R** search; `@` file picker and
  `!` shell commands; messages that steer the running turn (**Enter**) or
  queue a follow-up (**Tab**).
- **Looks.** Eleven colour themes (`/theme`), six banner mascots
  (`/mascot`), nine spinner styles (`/spinner`), and a searchable
  `/settings` menu, all remembered per directory.
- **Sessions and the command line.** Every conversation is recorded;
  `/resume` opens a searchable picker, `--continue` reopens the latest
  session in the directory, `--resume <id>` a specific one, and a quoted
  `[PROMPT]` argument starts a turn directly. `--help` and `--version` are
  available, as is the `mcp` subcommand family.
- **Telemetry, disclosed.** One anonymous daily ping (install id, version,
  OS, architecture) described in full in `TELEMETRY.md`, shown once at
  startup, and switched off with the **Telemetry** setting,
  `ALTER_ZERO_TELEMETRY=0`, or `DO_NOT_TRACK=1`.
- **One-line install.** `curl -fsSL https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh | sh`
  picks the build for the machine, verifies its SHA-256 against the
  published checksum, and installs `alter-zero` into `~/.local/bin`.
- **Update notice.** Once a day the app asks the repository's releases page
  whether a newer version is out — one request, carrying nothing about you
  — and says so under the banner, naming the release and `alter-zero
  update`, which installs it over the running binary through the same
  checksum-verified installer. Off with the **Update check** setting or
  `ALTER_ZERO_UPDATE_CHECK=0`; `TELEMETRY.md` states the request in full.
- **Release tooling.** A CI workflow running the project's gate (`fmt`,
  `clippy`, `test`, `doc`), the smoke suite, and the release tooling's own
  tests; a release workflow that, on a `vX.Y.Z` tag, verifies the version
  against this changelog, builds Linux (x86_64, arm64) and macOS (Intel,
  Apple silicon) archives with SHA-256 checksums, and publishes a GitHub
  release whose notes come from this file — driven end to end by
  `scripts/release.sh`, which also rehearses a release locally.

[Unreleased]: https://github.com/linuztx/alter-zero/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/linuztx/alter-zero/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/linuztx/alter-zero/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/linuztx/alter-zero/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/linuztx/alter-zero/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/linuztx/alter-zero/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/linuztx/alter-zero/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/linuztx/alter-zero/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/linuztx/alter-zero/releases/tag/v0.1.0

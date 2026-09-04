# CLI — `--help`, the `[PROMPT]` shortcut, `--continue`, `--resume [id]`, and the exit hint

Claude Code's session CLI, ported onto the `/resume` rollout files
(`docs/resume.md`): `alter-zero --continue` reopens the newest conversation
recorded in the current directory, `alter-zero --resume {id}` reopens a
specific session from anywhere, a bare `alter-zero --resume` opens the
`/resume` picker as the very first screen — and quitting a session that
recorded anything prints the copy-paste command that brings it back. On top
of those, **a quoted message is a `[PROMPT]`** — `alter-zero "fix the
failing test"` boots straight into that turn, and `alter-zero --resume {id}
"and now the docs"` sends it into the reopened conversation — and `--help`
is a titled, sectioned page in the app's own colour. (The same pre-TUI
boundary also answers the `mcp` subcommand family — `alter-zero mcp
add/add-json/remove/get/list`, `docs/mcp-cli.md` — routed by a first
argument of `mcp` before the flag grammar below applies.)

```
$ alter-zero --help
Alter Zero

Starts an interactive session by default — a quoted PROMPT is its first turn.

Usage: alter-zero [OPTIONS] [PROMPT]
       alter-zero mcp <COMMAND>

Commands:
  mcp                 Manage MCP servers in the user config file — see
                      alter-zero mcp --help

Arguments:
  [PROMPT]            Send this message as the first turn — of a new
                      conversation, or of the one --continue/--resume reopens

Options:
  -c, --continue      Continue the most recent conversation recorded in this
                      directory
  -r, --resume [ID]   Resume a conversation — by the session id the exit hint
                      prints, or picked from a list when no id is given
  -h, --help          Print help
  -V, --version       Print version
```

```
────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────
  dummy_model_name · ~/repo

Resume this session with:
alter-zero --resume 18a9f2c33d41e5b6-1a2b
```

## What Claude Code does (findings)

- `claude --continue` / `-c` — non-interactive: load the most recent
  conversation **in the current directory** and carry on. With nothing to
  continue it errors out ("No conversation found to continue") instead of
  starting fresh, so a scripted `-c` can't silently lose its thread.
- `claude --resume [id]` / `-r` — with an id (Claude's are UUIDs), resume
  that session; **bare**, open the interactive session picker. An unknown id
  is an error, not a fresh session.
- `claude "prompt"` — one optional positional, *the* prompt: the session
  boots and that message is its first turn. `claude -c "prompt"` and
  `claude -r {id} "prompt"` send it into the reopened conversation. A
  second positional is `error: too many arguments`, never joined; and
  `--resume` takes the next token as its id greedily, so `claude --resume
  "prompt"` looks that "id" up and fails.
- `claude --help` — commander's page: a `Usage:` line, a one-line
  description, then `Arguments:` / `Options:` / `Commands:` sections with
  the descriptions aligned in one column.
- Both session flags are startup-only concerns; in-session behaviour is
  unchanged.

## What we build

The same surfaces, sized to this codebase: a pure `cli` module (the arg
grammar, the help *pages* and the hint/usage strings), two pure `session`
helpers (the filename → id mapping and the newest-in-cwd pick), and a thin
boundary in `src/tui/startup.rs` that resolves flags to a rollout *path* —
and the prompt to a first turn — before the terminal is ever touched.

### Session ids

The id is the one the recorder already mints (`docs/resume.md`): the
`{id}` segment of `rollout-YYYY-MM-DDThh-mm-ss-{id}.jsonl`, also recorded on
the file's `session_meta` line — nanos-since-epoch + pid in hex. Nothing
about the on-disk format changes; the id merely becomes *addressable*:

- `session::rollout_file_id(file_name)` extracts it back out of a rollout
  filename (`None` for foreign names — the stamp's shape is checked, so a
  stray `rollout-notes.jsonl` can't yield a garbage id).
- `--resume {arg}` matches, in order: an existing rollout **path** (handy —
  the sessions dir is plain files), an exact **filename**, an exact **id**,
  then a **unique id prefix** (two matches make the prefix ambiguous — an
  error listing nothing, never a guess). The scan reuses the `/resume`
  listing's newest-first date-dir walk and needs no head reads.

### The `[PROMPT]` shortcut

`alter-zero "…"` is Enter without the composer: the message is recorded
into the ↑ input history (it *was* submitted), committed as the `❯`
bubble under the banner, and the turn spawns on it — the boundary's
`Session::submit_startup_prompt` (`src/tui/turn.rs`, beside every other
way a turn begins) runs `App::input_history.record` and then the same
`start_turn` the `Submit` key arm calls, so the CLI path can never drift
from the typed one. It runs **after** the session directive is applied and
the first frame is scheduled, so with `--continue`/`--resume {id}` the
loaded transcript sits above the new bubble and the turn appends to the
adopted rollout file; on a fresh launch the banner, the bubble and the
streaming strip land in the first synchronized frame together, exactly as
a fast Enter would paint them.

The grammar (`cli::parse`):

- **One positional, anywhere among the flags.** `alter-zero "p" -c` and
  `alter-zero -c "p"` are the same invocation. A second positional is a
  usage error naming it — `unexpected argument: bug (quote the prompt so
  the shell passes it as one argument)` — rather than a join: `alter-zero
  write a -c program` would otherwise read `-c` as `--continue`, and a
  prompt that silently loses a word is worse than one that has to be
  quoted. Claude Code errors here too.
- **`--` ends the flags**; what follows must be exactly that one prompt, so
  a message that starts with a dash is reachable (`alter-zero --
  "-flags first, then prose"`).
- **A blank prompt is a usage error** (`the prompt is empty`): an empty
  string is what a shell hands over when a variable was unset, and Enter
  refuses a blank draft too.
- **The prompt is a message, verbatim.** A leading `!` or `/` is sent to
  the model as text, not run as a shell command or a slash command — the
  composer's `!`/`/` grammars are keystroke grammars, and a script has
  `alter-zero mcp …` for the one thing it might have wanted `/mcp` for.
- **`--resume` keeps taking the next non-dash argument as its id**, so
  `alter-zero --resume "fix the bug"` resolves `fix the bug` as an id and
  fails with `No session found for id: fix the bug` (Claude Code's
  behaviour; the lookup's hint names the picker). To send a prompt into a
  reopened session, name the session: `--resume {id} "prompt"`, or
  `--continue "prompt"` for the newest one here.
- **A prompt with the bare picker is refused** — `alter-zero "p" --resume`
  is `--resume needs a session id to send a prompt into (or use
  --continue)`. The picker is interactive and dismissable, and a prompt
  parked behind it would either start a fresh conversation on Esc or have
  to be dropped: two surprising outcomes for one flag, so the grammar
  says no up front.

### The `--help` page

One renderer, two pages: `cli::help(HelpPage::Main, style)` and
`cli::help(HelpPage::Mcp, style)` each build a `HelpDoc` — a title, a
description, the `Usage:` lines, and the sections' rows — and render it
the way clap and cargo do, so a reader who knows any Rust CLI knows where
to look:

- **The title is `Alter Zero`** — `crate::APP_NAME`, the one place the
  app's name lives — on the first line, wearing the *heading* style: the
  page names the product, and `alter-zero` below it names the binary. The
  `mcp` page is titled by its command, `alter-zero mcp`, in the same style.
- **The description** is the line under it, and it describes the
  *invocation*, not the product: `Starts an interactive session by
  default — a quoted PROMPT is its first turn.` Both references write this
  line the same way — codex's *"If no subcommand is specified, options
  will be forwarded to the interactive CLI"*, Claude Code's *"starts an
  interactive session by default, use -p/--print for non-interactive
  output"* — because a help page is opened to find out how to run
  something, not to be sold it; what the app *is* belongs to the README.
  One line, since it is the first thing read and a paragraph there is a
  paragraph skipped, and under 80 columns like everything else
  (`main_help_fits_eighty_columns` pins it).
- **`Usage:`** is inline, the way clap prints it — `Usage: alter-zero
  [OPTIONS] [PROMPT]` — with each further form on its own line indented to
  the first (seven spaces). The command words (`alter-zero`, `mcp`) are
  *literals*; the bracketed and angled parts are *placeholders*.
- **Sections** — `Commands:`, `Arguments:`, `Options:` — each a heading
  over rows of `  {literal}{placeholder}   {description}`: two columns of
  indent, the widest literal cell in the section table, three more
  columns, then the description; a multi-line description continues at
  that same column. Nothing wraps at render time — every description is
  pre-wrapped in the source, so the page reads on disk exactly as it
  prints and a test can pin it.
- **Styles.** `HelpStyle::Plain` emits no escapes at all — the string the
  tests compare against and what a pipe receives. `HelpStyle::Ansi`
  dresses headings and the title in **bold cyan** (`ESC[1;36m` — the app's
  accent hue in the terminal's own palette, the banner's cyan and the
  pickers' selection colour, so the page belongs to the app that printed
  it), literals in **bold**, and placeholders in nothing; padding is
  measured on the plain text, so the columns line up in both. A usage
  error's `error:` is bold red (clap's). The two renderings differ by
  escapes alone — `ansi_help_strips_back_to_the_plain_page` pins that.
- **The policy is the boundary's.** `cli::colour_enabled(no_color, term)`
  is the pure rule — colour unless `NO_COLOR` is set non-empty
  (https://no-color.org) or `TERM` is `dumb` — and `startup.rs` combines it
  with `std::io::IsTerminal` on the stream being written: `--help` styles
  by stdout, a usage error by stderr. A pipe, a log file and a CI job get
  plain text without asking; a terminal gets the colour.

A **usage error** is clap's shape, not the old whole-page dump:

```
error: unrecognized argument: --frob

Usage: alter-zero [OPTIONS] [PROMPT]
       alter-zero mcp <COMMAND>

For more information, try '--help'.
```

`cli::usage_error(page, message, style)` renders it — the `error:` lead,
the page's own `Usage:` block, the pointer at `--help` — and the `mcp`
page's grammar errors take the same shape over its six usage lines. Exit
`2`, on stderr, as before.

### The pure `cli` module (src/cli.rs)

- `Cli` — the parsed invocation: `Session(SessionArgs)` (the TUI run — how
  to start, plus the optional prompt), `Help`, `Version`, `Mcp(McpCli)`.
  `SessionArgs { start: SessionStart, prompt: Option<String> }` with
  `SessionStart::{Fresh, Continue, Resume(Option<String>)}`.
- `parse(args)` — over `std::env::args().skip(1)`: `--continue`/`-c`,
  `--resume [id]`/`-r [id]` (also `--resume=id`), `--help`/`-h`,
  `--version`/`-V`, one positional `[PROMPT]`, `--` before it. Anything
  else — unknown flags, a second positional, `--continue --resume`
  together, a prompt with the bare picker — is `Err(message)`; the
  boundary prints `usage_error` to stderr and exits `2`, while a
  *resolution* failure (nothing to continue, unknown id) exits `1`.
  `--resume`'s value may be a following argument or `=`-attached; a leading
  `-` on the next argument means "bare `--resume`, then that flag" (so
  `--resume --help` doesn't eat `--help` as an id).
- `help(page, style)` / `usage_error(page, message, style)` — the two
  pages above, in either style; `colour_enabled(no_color, term)` the
  colour rule.
- `resume_hint(bin, id)` — the exit hint exactly as shown above (two lines,
  no indent). `bin_name(arg0)` derives the printed program name from how the
  user actually invoked it (a renamed or symlinked install echoes the name
  that was run), falling back to `alter-zero`.

### The boundary (src/tui/startup.rs, main.rs)

- **Arg parsing runs in `main()`, *after* the detached-exec hook.** The hook
  must stay the first statement (invariant 1 — a helper re-exec never parses
  TUI flags; its argv is `__alter-zero-detached-exec {cmd}`), and the CLI
  work touches no terminal state, so `--help`/errors print to a normal
  cooked-mode stdio and exit before the tokio runtime boots.
- **Flags resolve to a `Startup` before `InlineViewport::init`** —
  `Startup { session: Option<StartupSession>, prompt: Option<String> }`,
  fail fast, no TUI flash:
  - `--continue`: `list_sessions(root, None)` (the `/resume` scan —
    eligibility included, so a session you never typed into doesn't
    continue) filtered by `session::latest_for_cwd` against
    `cwd.display()` — the same verbatim string the recorder writes and the
    picker's `Cwd` filter compares. No match → stderr + exit 1.
  - `--resume {id}`: the finder above. No match → stderr + exit 1.
  - The chosen file is read + parsed **here**; an unreadable/foreign file is
    the same fail-fast error. `StartupSession::Load` carries the parsed
    `(path, text, meta, items)` into `run()`, so the in-TUI path cannot
    fail.
  - bare `--resume` → `StartupSession::Picker`.
  - the prompt rides along untouched; the grammar already refused the one
    combination (`Picker` + prompt) that has no sound meaning.
- **`run(term, startup)` → `Session::bootstrap` applies the directive
  before the first frame:**
  - `Load`: mirrors the picker's `ResumeSession` arm — restore the cwd to
    the session's final checkpoint (backup snapshot first, unknown commits
    no-op; `docs/checkpoint.md`), `app.load_session(items)`, recorder
    `adopt` (same file accumulates, torn-tail repair included) — then
    commits the loaded conversation to scrollback at the boundary: the
    header banner, a blank, and `ui::repaint_lines(history, width,
    RESIZE_REFLOW_MAX_ROWS)` through the normal `insert_before` pipeline.
    **No `Purge` at startup**: a fresh launch must not wipe the user's
    terminal scrollback (the picker's mid-session purge exists to drop the
    *previous conversation's* rows; at startup there is none), and
    `insert_before` is exactly how the header already lands. A restore
    that actually moved files raises the usual toast.
  - `Picker`: the `OpenResumePicker` arm verbatim (scan → open →
    `enter_overlay` → paint) before the loop. The header banner is *not*
    pre-committed on this path — every overlay return re-emits it via
    `ui::banner_tail`, so the close repaints it (and quitting straight from
    the picker exits through the same return).
  - then the prompt, if any: `submit_startup_prompt` as described above —
    the last thing `bootstrap` does, after the first frame is scheduled.
- **The exit hint:** `run()` returns `Option<String>` — the recorder's
  active session id, only when `app.history` is non-empty (codex's deferred
  create means an empty session has no file, no id, and prints nothing;
  a `/clear`ed-then-idle session likewise). `tui_main` prints it **after**
  `term.restore()` so the two lines land below the box in normal terminal
  flow, through `cli::resume_hint` with the invoked bin name. A run that
  errored out surfaces its error instead (no hint).

## Known divergences from Claude Code

- **Ids are the recorder's hex stamps, not UUIDs** — same length class, same
  copy-paste ergonomics, no new dependency; old rollout files keep working
  because nothing re-mints ids. Unique-prefix matching is a convenience
  Claude doesn't offer; ambiguity is an error, so it can't misfire.
- **`--continue` is cwd-scoped by the meta line's verbatim cwd string**
  (the picker's `Cwd` filter rule, `docs/resume.md`) — a session recorded
  through a symlinked path won't match its canonical twin.
- **The hint prints on every quit with history** (Claude gates some exits);
  it is plain stdout after restore, so it scrolls away naturally and never
  enters the transcript, the recorder, or the model's context.
- **`[PROMPT]` is interactive-only.** Claude Code's prompt doubles as the
  input of its `-p/--print` headless mode; this TUI has no non-interactive
  mode, so the prompt always opens the session it starts. `--print`,
  `--fork-session` and `--session-id` stay out of scope for the same reason.
- **The help page is titled and coloured.** commander's page opens on
  `Usage:`; ours opens on the product name and the description, cargo's
  ordering, in the app's cyan where commander's is plain.

## Testing

- `cli`: `parse` — bare/`-c`/`--continue`, `--resume` with and without id
  (separate, `=`-attached, `-r`), a following flag not eaten as an id,
  help/version, unknown flag / `-c -r` conflicts as errors; the prompt —
  alone, before and after a flag, after `--` (a dash-leading one), with
  `-c` and `-r {id}`; a second positional as an error carrying the quote
  hint; a blank prompt refused; a prompt with the bare picker refused;
  `--resume "x y"` still an id. The help — the plain main page pinned
  line for line, every line under 80 columns with no trailing blanks, the
  title `Alter Zero` first, the `mcp` page's title and six usage lines;
  the ANSI rendering stripped of escapes equal to the plain one, the
  title/headings carrying the heading escape and `alter-zero` the bold
  one; `usage_error`'s three-part shape; `colour_enabled`'s three rules;
  `resume_hint` matches the printed shape; `bin_name` basenames a path and
  falls back on `None`/empty.
- `session`: `rollout_file_id` round-trips the id `rollout_rel_path` embeds
  (dashed ids included), rejects foreign/stampless names; `latest_for_cwd`
  picks the newest `updated_secs` among matching-cwd rows only, `None`
  otherwise.
- `tests/cli_help.rs` (the real binary, `CARGO_BIN_EXE_alter-zero`,
  offline): a piped `--help` is byte-for-byte `help(Main, Plain)` — no
  escapes off a tty — and `mcp --help` the `Mcp` page; `--version` prints
  `alter-zero {version}`; a grammar error exits 2 with the `error:` shape
  on stderr and an empty stdout; two positionals name the second with the
  quote hint; a prompt with the bare picker is refused.
- `scripts/smoke.sh` Phase 61 (the I/O boundary): quitting a session that
  recorded a turn prints `Resume this session with:` + a `--resume {id}`
  line whose id names the rollout file; `--continue` relaunches straight
  into the old conversation (no picker) and appends to the same file;
  `--resume {id}` does the same by id; bare `--resume` boots into the
  picker; quitting an *empty* session prints no hint; `--continue` in a
  fresh cwd fails fast with exit 1 and no TUI. **Phase 108**: `alter-zero
  "hello there"` boots straight into the turn — the `❯` bubble, the
  streamed reply and `Done for` with no key pressed — and its quit prints
  the hint; `--resume {id} "again please"` reloads the transcript and runs
  the prompt as the next turn in the same rollout file; `-c "…"` likewise;
  `--help` on the pane's tty opens on `Alter Zero`, carries `[PROMPT]`,
  and is coloured (the raw pane holds the bold-cyan escape) while the same
  page through a pipe holds none; a usage error prints the clap-shaped
  trailer and exits 2.
- `scripts/live_smoke.sh` Phase L4: against a real provider, the prompt
  shortcut's turn streams and settles, and `--resume {id} "…"` continues
  it.

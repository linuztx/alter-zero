# System prompt flags — `--system-prompt`, `--append-system-prompt`

Date: 2026-11-20

## Goal

Say what the system prompt is **for one run, from the command line**: replace
the built-in persona, or keep it and add a few lines to it — in the TUI and in
a headless `-p` run alike.

```bash
alter-zero --append-system-prompt "Answer in French."
alter-zero -p --append-system-prompt-file rules.md "review the last commit"
alter-zero -p --system-prompt "You write commit messages. Reply with the message alone." "$(git diff --cached)"
alter-zero --system-prompt-file prompts/reviewer.md --append-system-prompt "Be brief."
```

Before this the only handle was `ALTER_ZERO_SYSTEM_PROMPT`, which replaces the
persona and does nothing else. Adding one rule meant copying the whole persona
into a shell variable, a prompt of any length had to fit in one, and a script
had no flag to pass.

## What Claude Code does (findings)

- `--system-prompt <text>` replaces the default prompt; `--system-prompt-file
  <path>` is the same, read from a file. The two are mutually exclusive.
- `--append-system-prompt <text>` keeps the default prompt and adds the text
  to it; `--append-system-prompt-file <path>` reads it from a file. An append
  combines with either replacement.
- All four belong to the run. Nothing is saved, and a later run without them
  is back on the default prompt.

## What we build

The same four flags, under the same names, in both kinds of run:

| Flag | What it does |
| --- | --- |
| `--system-prompt <TEXT>` | TEXT is the persona for this run, in place of the built-in one. The flag form of `ALTER_ZERO_SYSTEM_PROMPT`, which it outranks. |
| `--system-prompt-file <PATH>` | The same, with the persona read from PATH. |
| `--append-system-prompt <TEXT>` | TEXT is added after the persona, whichever persona the run has: the built-in one, the variable's or the flag's. |
| `--append-system-prompt-file <PATH>` | The same, with the text read from PATH. |

They are the run's and are never saved, like `--permission-mode`: a
`--continue` without them is back on the built-in persona, and one with them
applies them to the conversation it reopens.

### Where the text lands

The system prompt is a few blocks in a fixed order (`docs/environment.md`).
The flags touch the first one and nothing after it:

```
persona        --system-prompt[-file], else ALTER_ZERO_SYSTEM_PROMPT, else prompts/alter_zero.md
               then --append-system-prompt[-file], after a blank line
environment    prompts/environment.md   (always)
scratchpad     prompts/scratchpad.md    (when the session has one)
headless       prompts/headless.md      (alter-zero -p only)
```

- **A replaced persona keeps the blocks after it.** The date, the os, the
  user, the cwd, where temporary files go and — in a headless run — that
  nobody is watching are facts about the session, not about who the agent is.
  This is what `ALTER_ZERO_SYSTEM_PROMPT` already did, and the flag is its
  twin.
- **Appended text follows the persona and comes before `## Environment`.**
  It is instruction, and the blocks after it are headed sections of facts.
  Text put after the last of them would read as more of `## Scratchpad` or
  `## Headless`, and the `/context` page, which splits the prompt at its
  headings, would count it under that heading.
- **The text goes in as written.** Its ends are trimmed and nothing else is
  done to it: no heading is added, and no `{placeholder}` is filled in. The
  skills loader's substitution pass once rewrote a body's own prose
  (`docs/skills.md`); a prompt that shows a `{cwd}` in an example means those
  characters. The environment block is always there, so a prompt never needs
  to ask for the cwd.

What the persona block comes to, by what the run names
(`llm::backend::compose_persona`):

| Replacement | Appended | The persona block |
| --- | --- | --- |
| none | none | the built-in persona |
| R | none | R |
| none | A | the built-in persona, a blank line, A |
| R | A | R, a blank line, A |
| blank | none | blank, so no system prompt at all |
| blank | A | A |

A blank replacement can only come from the variable: an empty
`ALTER_ZERO_SYSTEM_PROMPT` still means *send no system prompt*
(`docs/context.md`), and the flags refuse a blank value (below). Appended text
is something to say, so a run that appends has a system prompt, and a prompt
that is not blank gets the runtime blocks.

### The grammar (`cli::parse`)

`SessionArgs` gains `system_prompt` and `append_system_prompt`, each an
`Option<SystemText>` — `Inline(String)` from the text flag, `File(PathBuf)`
from the file flag.

- **Each flag reads its value like every other value flag**: the next
  argument, or attached with `=`, and never a next argument that leads with a
  dash. A system prompt can start with one (a Markdown list does), so that
  refusal says how to pass it: `--append-system-prompt needs a value (attach
  text that starts with a dash: --append-system-prompt="-…")`. The hint is
  there only when the next argument does lead with a dash; a flag with
  nothing after it says `needs a value` alone. Taking a dash-leading argument
  as the text was weighed and refused: `alter-zero --system-prompt -p "fix
  it"` would run the TUI on a system prompt of `-p`.
- **A blank value is refused** (`needs a value`), whitespace alone included.
  An empty string is what a shell passes for an unset variable, and a run
  that silently lost its persona, or the rules a script meant to add, is
  worse than one that stops. The `[PROMPT]` argument has the same rule.
- **One of each pair**: `--system-prompt` with `--system-prompt-file` is
  `pass at most one of --system-prompt / --system-prompt-file`, and the
  append pair likewise. A replacement and an append combine.
- **A flag given twice is refused**, as every flag is.
- All four work with and without `-p`, and beside `--continue`/`--resume`.

### Reading a file (`tui::startup`)

A file is read where a session flag is resolved: in `prepare`, in cooked
mode, before the terminal is touched, so a bad path prints one line and exits
`1` with no TUI flash. A headless run reports it through
`headless::not_started`, so `--output-format json` still hands a script its
result object (`docs/headless.md`).

- **After `-C`.** `prepare` changes directory first, so a relative PATH is
  read from the directory the run happens in — git's rule for `-C`.
- **Bounded.** At most `cli::SYSTEM_PROMPT_FILE_MAX_BYTES` (1 MiB) is taken.
  A prompt is sent with every request, so a megabyte of it is already past
  most context windows, and the bound is what stops a path like `/dev/zero`
  from filling memory. The read takes one byte past the cap, which is enough
  to know the file is over it. A pipe is fine (`--system-prompt-file
  <(make-prompt)`).
- **What the bytes come to is pure** (`cli::system_prompt_file_text`): over
  the cap, not UTF-8, or blank is the run's failure; anything else is the
  text. A byte order mark at the start is dropped first — some editors
  write one ahead of UTF-8, it is the file's encoding rather than its text,
  and no trim takes it — so it is never the prompt's first character and a
  file holding nothing else is empty.

| The file | The line on stderr |
| --- | --- |
| cannot be opened or read | `Cannot read --system-prompt-file prompts/x.md: No such file or directory (os error 2)` |
| is over the cap | `--system-prompt-file prompts/x.md is larger than 1 MiB` |
| is not UTF-8 | `--system-prompt-file prompts/x.md is not UTF-8 text` |
| is blank | `--system-prompt-file prompts/x.md is empty` |

Each names the flag as typed and the path as given.

The texts ride `Startup` to `Session::bootstrap` and from there to
`ModelSession::resolve`, where `tui::config::system_prompt` composes the
prompt once. Every backend the session builds afterwards — a `/model` switch,
a Ctrl+T change, a `/settings` knob, the capability probe — inherits it by
clone, so there is still one place the prompt is assembled.

### Who else reads the prompt

- **Subagents.** A type with no body runs on the session's own prompt
  (`docs/subagents.md`), so it carries the replaced persona and the appended
  text. A type whose definition has a body replaces the persona block — which
  is where both flags write — and keeps the environment and scratchpad
  blocks, so the flags do not reach it. Its instructions are its file's.
- **`/compact`.** The summarization turn runs on the session's prompt, as it
  did.
- **The safety reviewer.** Auto mode's classifier has a prompt of its own, a
  constant (`llm::classifier::CLASSIFIER_SYSTEM_PROMPT`). Nothing a run says
  about its system prompt reaches it, and a test holds it to that.
- **Ctrl+D and `/context`** show the prompt the backend sends, so both show
  the flags' text with no change of their own.
- **The offline demo** sends no system prompt, so the flags are accepted
  there and change nothing.

### The help page

The four rows go in `Options:`, under `--permission-mode`. Three of their
cells are wider than any the page had (`    --append-system-prompt-file
<PATH>` is 38 columns), and the page has one description column for all its
sections: widening it to fit would leave 37 columns for every description on
the page. So `cli::help` takes clap's rule for a long flag. A cell wider than
`HELP_CELL_MAX` (28 columns, which leaves the 47-column descriptions the page
is written to inside 80) does not count toward the column, and its
description starts on the next line:

```
      --permission-mode <MODE>   manual, edit, auto or master, for this run only
      --system-prompt <TEXT>     Use TEXT as the system prompt for this run, in
                                 place of the built-in one
      --system-prompt-file <PATH>
                                 The same, with the prompt read from PATH
      --append-system-prompt <TEXT>
                                 Add TEXT to the system prompt for this run
      --append-system-prompt-file <PATH>
                                 The same, with the text read from PATH
  -h, --help                     Print help
```

## Considered and left out

The proposal this came from was wider. What it would have added, and why it
is not here:

- **Prompt files found by themselves** (`~/.alter-zero/SYSTEM.md`, a
  project's `.alter-zero/SYSTEM.md`, and an append twin of each). A project
  file that replaces the persona is a way for a cloned repository to rewrite
  the agent, so it would have to go through the startup review
  (`docs/config-review.md`) with a fingerprint of its text. That is a feature
  of its own, and a flag naming a file already covers the need: the file is
  the user's choice, per run.
- **More environment variables** (a file form, an append form). The flag is
  what was missing. `ALTER_ZERO_SYSTEM_PROMPT` stays as it was.
- **Changing what an empty `ALTER_ZERO_SYSTEM_PROMPT` means.** It sends no
  system prompt, on purpose (`docs/context.md`), and tests pin it.
- **Placeholders in the user's text.** See *The text goes in as written*.
- **A heading over appended text naming where it came from.** The text is
  the user's; a heading they did not write is text they did not ask for.
  Ctrl+D shows the prompt whole.
- **Appending to an agent type that has its own body, and a frontmatter key
  to opt out.** The body is that type's instructions. The lead knows what the
  run appended and can pass on what a task needs.
- **Reading the files again at `/clear` or `/resume`.** The prompt is the
  run's, resolved once; reading it again mid-run would change the cached
  prefix of every request after it.
- **A user-level `AGENTS.md`.** Standing instructions for every project are
  the project doc's business (`docs/project-doc.md`), not this flag's.

## Testing

- `cli` (pure): the four flags parse to `SystemText` — separate and
  `=`-attached, with and without `-p`, beside a prompt and beside
  `--continue`; a replacement and an append combine; each pair is exclusive;
  a flag twice, a missing value, a blank value and a dash-leading next
  argument (with its hint) are usage errors. `system_prompt_file_text` takes
  text as it is, less a byte order mark, and refuses a file over the cap, not
  UTF-8, or blank, each in its own words; `system_prompt_file_unreadable`
  names the flag, the path and the cause. The help page is pinned line for line with the four rows, fits
  80 columns with no trailing blanks, strips back to the plain page from the
  ANSI one, and the `mcp` page is unchanged by the long-cell rule.
- `llm::backend` (pure): `compose_persona` over every row of the table above;
  composed with `augment_with_environment`, the appended text sits between
  the persona and `## Environment`; a backend built on a custom prompt still
  shows the classifier its own constant.
- `tests/cli_help.rs` (the real binary, no terminal): the piped `--help` is
  the pinned page; a text flag with no value, both flags of a pair, and a
  blank value exit `2` with the usage trailer; a file that cannot be read
  exits `1` with its line on stderr, before any interface starts.
- `tests/headless.rs` (the real binary, a stand-in provider on the
  loopback): the system message of the request carries the appended text
  between the built-in persona and `## Environment`, with `## Headless` still
  after it; `--system-prompt` replaces the persona and keeps the blocks; the
  flag outranks `ALTER_ZERO_SYSTEM_PROMPT` while an append adds to the
  variable's persona; the file flags read relative to `-C`; a file that
  cannot be read sends no request and, under `--output-format json`, is one
  error result.
- `scripts/smoke.sh` Phase 162 (the TUI): launched with `--system-prompt-file`
  and `--append-system-prompt` against a stub provider, the request on the
  wire opens on the file's text, then the appended line, then
  `## Environment`, without the built-in persona; Ctrl+D shows the same.

## Known limitations

- The flags are read at launch. Nothing in a running session changes its
  system prompt.
- An agent type with its own body never sees the flags' text.
- A PATH is not `~`-expanded by the app; the shell does that, and not after
  an `=`.
- On the offline demo the flags do nothing, since it sends no system prompt.

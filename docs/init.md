# `/init` — generate an AGENTS.md contributor guide (codex's `/init`)

## What codex does

Codex's `/init` is deliberately tiny. The slash command
(`SlashCommand::Init`, description "create an AGENTS.md file with
instructions for Codex") does exactly one thing:

```rust
SlashCommand::Init => {
    const INIT_PROMPT: &str = include_str!("../../prompt_for_init_command.md");
    self.submit_user_message(INIT_PROMPT.to_string().into());
}
```

It submits a canned prompt as a **regular user turn**. The prompt asks the
model to generate a file named `AGENTS.md` — a concise (200–400 word)
contributor guide with recommended sections (project structure, build/test
commands, style, testing, commit/PR conventions) — and, crucially, to check
first whether `AGENTS.md` already exists and *not* overwrite it. Everything
else — exploring the repo, writing the file — is the model's normal agentic
tool loop. There is no app-side file I/O, no special rendering, no new
machinery: the prompt *is* the feature.

Codex gates it like `/compact`: `available_during_task` is `false` — you
can't start it while a task is running.

## Ours

The port mirrors that shape exactly; every piece rides existing machinery.

- **`prompts/init.md`** — codex's `prompt_for_init_command.md`, verbatim.
  Compiled in as `app::INIT_PROMPT` via `include_str!` (the
  `prompts/compact_prompt.md` seam — wording stays in maintainable
  markdown, not a Rust string).
- **The palette entry** — a one-line `COMMANDS` addition
  (`CommandEffect::Init`), listed right before `/compact` (codex's
  Init-then-Compact adjacency). Description adapted like `/quit`'s "Exit
  alter-zero": "create an AGENTS.md file with instructions for alter-zero".
  `help_text()` walks `COMMANDS`, so `/help` lists it for free.
- **Dispatch** (`App::run_selected_command`):
  - idle → `Action::Submit(INIT_PROMPT.to_string())`. The loop's existing
    `Submit` arm runs `start_turn`, so the whole prompt is echoed as the
    user `❯` message (codex renders its submitted prompt the same way),
    recorded to the session rollout, checkpointed, and answered by the real
    backend's tool loop — the model `read`s the repo and `write`s
    `AGENTS.md` itself. No new `Action` variant.
  - mid-turn → `Action::Toast(INIT_BUSY_NOTICE)` ("/init is disabled while
    a task is in progress") — codex's `available_during_task = false`,
    expressed in our `/help`/`/resume`/`/compact` busy-toast pattern
    (`docs/toast.md`).
- **↑ recall** — *not* recorded into `App::input_history`. The composer
  held `/init`, not the canned prompt; palette commands never record (the
  Enter arm's recording is for typed submissions), and recalling a 40-line
  prompt the user never typed would be noise.

## What deliberately isn't here

- **No app-side `AGENTS.md` existence check.** The prompt itself instructs
  the model not to overwrite an existing file — same trust codex places.
- **No new smoke phase.** The decision is pure (palette → `Submit`) and
  unit-tested; the `Submit` → `start_turn` path is already smoke-covered.
- **No special-cased display.** The full prompt shows as the user message,
  codex-style. If that ever feels noisy, a display-text/send-text split
  would be a follow-up design, not part of this port.

## Tests

`app::tests` (the `--- /init (docs/init.md) ---` block): the palette entry
+ description, the prompt's content (mentions `AGENTS.md`, guards
overwrite), idle Enter → `Submit(INIT_PROMPT)` with the composer cleared,
no ↑-recall recording, and the mid-turn busy toast leaving the running turn
untouched.

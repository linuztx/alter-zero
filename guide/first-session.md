# Your first session

You installed Alter Zero and started it. This page covers the first screen and
the three commands that take you from the offline demo to a real model.

## The first screen

```
 ▙▄▙▄▟▄▟   Alter Zero (vX.Y.Z)
▝▜▄███▄▛▘  ~/projects/site
  ▘▘ ▝▝    /login   /model   /resume
```

The banner scrolls away as the conversation grows. Below it is the composer,
and under that the footer: the active model and your directory on the left, the
permission mode pinned on the right.

```
dummy_model_name · ~/projects/site      auto
```

Until a model is connected, replies come from the built-in offline demo —
scripted turns that drive the interface without touching the network. Its
answers end by pointing at `/login` and `/model`, which is the next step.

## Say something

Type a message and press Enter. The status line above the composer shows the
turn running (`esc to interrupt` stops it), and each tool call appears as a
collapsed `●` cell with its output underneath.

- `Ctrl+O` opens the full transcript with tool output expanded.
- `Ctrl+D` shows exactly what is sent to the model.
- `esc` interrupts a running turn; the partial answer and the tool results stay.

The default permission mode is `auto`: file changes and read-only commands run
without asking, and other commands go to a safety classifier, which asks you
when it cannot decide. When a prompt does appear, Enter accepts, Esc cancels,
and the second option is broader — allow all edits for the session, or remember
a command rule for this directory.

## Connect a model

Run `/login`. It asks how you want to sign in:

| Row | Use it when |
| --- | --- |
| **Sign in with an account** | You have a subscription or console account — ChatGPT, Anthropic Console, GitHub Copilot, Cline, Google Antigravity. Some open a browser; some show a code to enter at a URL. |
| **Sign in with an API key** | You have a provider key. It is masked while you type and saved to `~/.alter-zero/.env`, outside your repository. |
| **Sign in with a local model** | You run Ollama or LM Studio. The page tests the address before it saves it. |

Each provider row shows `✔ configured` or `◯ unconfigured`; `Esc` backs out
without changing anything.

Then run `/model` to choose a model. The picker lists what that provider
offers; type to search, `↑`/`↓` to move, Enter to choose. The choice is
remembered for this directory, and the footer updates at once.

## Do real work

Ask for something concrete: "explain what this project does", "run the tests
and fix the first failure", or "trace how authentication works here". The agent
reads files, runs commands and edits as needed; every step is on screen and
interruptible.

- `/` lists commands; `?` lists keyboard shortcuts.
- `Shift+Tab` cycles the permission mode: `manual`, `edit`, `auto` (the
  default), `master`. The word at the footer's right edge is the current one.
- Enter while a turn runs hands the message to that turn; `Tab` queues it as a
  follow-up turn.

## Leave and come back

`Ctrl+C` clears the draft first; with an empty composer it quits. `/quit` exits
directly.

Every conversation is recorded. On exit the app prints:

```
Resume this session with: alter-zero --resume {id}
```

- `alter-zero --continue` reopens the newest conversation in this directory.
- `alter-zero --resume <id>` reopens one by id; bare `--resume` opens the
  picker.
- `/resume` does the same from inside a session.

## Where to read more

- [`docs/llm.md`](docs/llm.md) — signing in, model selection, and every provider.
- [`docs/permissions.md`](docs/permissions.md) — the four modes and the approval
  prompt.
- [`docs/resume.md`](docs/resume.md) — sessions, rollouts and the picker.

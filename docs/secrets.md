# `/secrets` — credentials the agent uses but never sees

The **`/secrets`** command opens a page where the user stores **secrets**: a
name, a value, and an optional line of context. The agent is told the names
and the context, and uses a secret by writing its **placeholder** —
`<secret:ROOT_PASSWORD>` — in a tool call. The placeholder becomes the real
value at the moment the tool runs, and the value becomes the placeholder
again in everything the tool reports. So the model can log in, call an API or
write a config file with a credential it has never seen, and a screen
recording of the session shows placeholders where the values would be.

## Where a value may exist

A value lives in exactly three places:

1. **`{config_home}/secrets.json`**, written owner-only (`0600`) through a
   temp file and a rename — the `.env` key store's write
   (`llm::keystore::EnvFile::update_if`), never a plain `fs::write`.
2. **The shared [`SecretRegistry`](../src/secrets.rs)** in memory — the
   `SkillRegistry` shape: one `Arc<Mutex<SecretStore>>`, cloned by handle into
   the backend, the subagents, the background registry and the `!` runner.
3. **The arguments of the one tool call that needs it**, for as long as that
   call runs.

Beyond the store, a value reaches disk only where a tool puts it on the
user's behalf: a file the agent wrote it into (the point of `.env`), and the
session's raw task logs (`{tmp}/alter-zero-{uid}/{session}/tasks/*.output`),
which keep a command's output as it was printed — `read` back through the
tool, a log comes out redacted like any other output. Those logs are created
owner-only (`0600`), since a temp directory's default mode would let every
user on the machine read them.

It is never in a message, a tool cell, the Ctrl+O transcript, the Ctrl+D
context, the rollout, a request body, the classifier's context, a hook's
`tool_input`, or a `Debug` print: `SecretValue`'s `Debug` says
`<redacted>`, it has no `Display`, and `expose()` is called by the expansion
and the file writer alone.

## The placeholder

`<secret:NAME>`, `NAME` being `[A-Z_][A-Z0-9_]*` — an environment
variable's shape, at most 64 characters. Expansion also accepts the name in
lower case; redaction always writes it as stored. An inserted value is never
scanned again.

The feature was built as `/secrete` with `<secrete:NAME>` and renamed before
it was released: "secrete" is a real word, and the spelling fought the
model's priors — the code had to accept `<secret:NAME>` as an alias in case a
model "corrected" it. With the spelling models reach for being the real one,
there is one opener and no alias.

**A placeholder naming nothing stored refuses the call.** Run as written, a
misspelled or guessed name reaches the tool as its literal text: a wrong
password typed at a prompt (an attempt counted against the account), a
request sent with `Bearer <secret:TOKN>`, a `.env` line that looks written.
So a tool that acts, and a `!` command, is refused before anything of it
runs, with a message naming what it wrote and what is stored
(`SecretStore::unknown_refusal`):

```
Not run: <secret:DEPLOY_TOKN> is not a stored secret. Stored:
<secret:DEPLOY_TOKEN>. The user adds secrets with /secrets.
```

One step corrects a slip, and a secret the user never stored sends the model
to the page instead of asking for the value in the chat. Nothing is refused
while no secret is stored — the session that never used the feature can
write the placeholder syntax as text. The price is that, with secrets stored,
a tool cannot write the literal text of a placeholder naming none of them;
writing a known one as text was never possible, since it expands.

## Expansion: only where a tool acts

`secrets::expands_placeholders` decides per tool. The tools that **act** —
`bash`, `bashsend` (and the legacy `bash_session`), `read`, `write`, `edit`,
and every `mcp__server__tool` — get their arguments expanded
(`secrets::expand_arguments`: every JSON **string** walked, keys left alone,
the document re-serialized, so a value holding `"` or `\` is escaped
correctly). The tools whose arguments are shown to a person or handed to
another model never do: `agent` (the subagent gets the placeholder and its
own listing), `askuserquestion` (the modal would show the value), `skill`,
and the task tools.

**Typed input is the one exception.** A `bashsend` input is read for key
notation — `<Enter>`, a slipped `\\n` undone into a line break, `&lt;Esc&gt;`
— so its placeholders are expanded *after* that reading, inside the text runs
it leaves (`llm::exec::run_session`): expanded first, a password holding `\n`
or `<Up>` would be typed as keys. A value is always typed as text — pinned by
a test that types `C:\new<Up>&lt;x` through a real terminal and reads it back
reversed.

It happens in `llm::secret_exec::run_with_secrets`, which both execute
closures in `llm::backend` call — the main turn's and a subagent's — **after**
the classifier's action log records the call, so auto mode's reviewer reads
the placeholder, and after `ToolStart`/`RoundCalls` have carried the model's
own arguments to the transcript and the rollout. The hooks see the
placeholder too: `PreToolUse` runs before the closure.

The refusal of an unknown placeholder is checked in four places, each
before anything runs: `approval::approve_call`, so a call that cannot run
never raises a prompt (resolving as a rejection there also means a secret
saved between the answer and the run cannot slip an unasked call through);
`run_with_secrets`, the authoritative check, for the paths that skip the
prompt (no gate, a pre-approving hook); `run_session`, over the text a
`bashsend` input's key notation leaves — so a placeholder escaped as
`&lt;secret:X&gt;` is caught too; and `tui::shell`, for a `!` command.

## Redaction: every way output leaves a tool

`SecretStore::redact` replaces each exact occurrence of a value with its
placeholder in one greedy left-to-right pass — the **longer** value winning
where two could match, so a value that contains another is hidden whole, and
an emitted placeholder never scanned again. A value of at least
`WRAP_MATCH_MIN_CHARS` (12) characters also matches across a line break, the
way a terminal's 120-column screen wraps a long token; a shorter one only
matches exactly, since `12\n34` is ordinary output. Values need at least
`MIN_SECRET_VALUE_CHARS` (4) characters: every occurrence is replaced, and a
one-letter "secret" would shred every output that contains the letter.

The paths:

| path | where it is redacted |
| --- | --- |
| a tool's result — `ToolEnd`, `ToolAnswered`, `ToolRejected`, the model's `tool` message, the `PostToolUse` payload | `run_with_secrets`, on `output` and `context` |
| a running command's live cell (`ToolProgress::Screen`) | `run_with_secrets`: `settled` through a `StreamRedactor`, `live` with the held tail |
| a refined header (`ToolProgress::Title`) — built from the expanded input | `run_with_secrets` |
| a background task's output, screen, command, description | `BackgroundRegistry` (`with_secrets`): `send_output`, `send_screen`, the stored `Launch` — the description too, which the executor expanded with the rest of the call and which the ↓ manager, the completion notice and the model's completion note all name |
| the permission prompt's preview | `approval::approve_call` (see below) |
| a `!` command's output | `tui::shell`, before its `ToolEnd` (the command itself expanded with `expand_checked`, so an unknown placeholder runs nothing) |
| the `/diff` review | `tui::diff`, on the loaded snapshot |

**Streaming.** A value can arrive split across two pieces of a running
command's output. `StreamRedactor` holds back the tail that could still grow
into a value, and emits everything before it redacted exactly as the whole
text would be — pinned by a test that splits a stream at every byte. The
held tail is shown with the live row, masked if it is a long enough piece of
a value.

**Truncation.** Three tools cut their output at the end — `read`, MCP, and
the `!` shell's cap — and mark it `truncated`. A cut through a value would
leave its first half behind, so a truncated output is redacted with
`redact_cut_tail`: a trailing piece of four or more characters that begins a
value is masked as that value's placeholder. `HeadTail`, the bash tools'
middle cut, keeps whole lines and needs nothing.

**What it cannot see.** Redaction is textual: a value the tool transforms —
base64-encoded, reversed, split by something other than a line break — comes
out as the transformation. That is the contract the feature makes: *exact*
text is hidden.

## The permission prompt

The prompt's preview is built from the **expanded** call and then redacted:

- An `edit` whose `old_string` holds a placeholder only matches the file once
  expanded. Built from the model's text, the preview failed to apply —
  `permission_request` returned `None`, which `approve_call` reads as *nothing
  to ask* — and the edit would have run **unprompted**.
- A `write` over a file already holding a value, or an `edit` whose context
  lines do, would otherwise put the value on screen in the diff.

The request's `target`, `body` and `detail` are redacted before anything
reads them, so the allowlist match, the scratchpad rule and the auto-mode
classifier all see the placeholder — the same text the user approves. A call
naming a secret that is not stored is refused before any of this, and asks
nothing.

The classifier is told what a placeholder is (`prompts/classifier.md`): a
credential the user stored for the agent, filled in when the action runs,
its name saying what it is for — so a login or an authorization header for
its own service reads as ordinary work, and sending one anywhere else as
exfiltration. Without that sentence the rubric's "reading or sending
secrets" rule reads every use of the feature as credential theft.

## The reminder

The secrets ride the session's one `<system-reminder>` as a **third listing
section**, after the skills and the agent types (`Session::sync_listings` →
`App::listings`; `docs/context.md`):

```
Each <secret:NAME> below is one of the user's credentials. Use it verbatim
in any tool argument and the tool gets the real value (quote it in shell).
Output shows the placeholder only where the exact value appears, so never
encode or hash one. Never ask for or reveal a value.

- <secret:ROOT_PASSWORD>: Root password for the staging box
- <secret:VENICE_API_KEY>
```

Four sentences, each a behaviour the live suite relies on: what the
placeholders are, that one goes into the tool call as written (a command,
a file's content, typed input — "any tool argument"), quoted for the shell
since the value is inserted as-is, and that a value is never asked for. The
third is the one limit the model has to know about: masking is exact text,
so a Basic-auth header `curl -v` prints, or a base64 of the value, would
show the secret to the model and the screen alike — told so, the model
hands the placeholder to what needs it instead of transforming it itself.
The header rides every request, so it is pinned at 55 words and each of
those points by a test (`the_header_says_what_the_placeholders_are_for_and_stays_short`).

Last because it changes least often of the three and the reminder is a
prompt-cache prefix. Gated on **Tools** (`/settings`): without tools a
placeholder has nowhere to go. A subagent's briefing carries the same
section beside its skills when its type can use a tool that expands
placeholders (`SubagentConfig::briefing_for`).

## The page

```
──────────────────────────────────────────────────────────────────

  Secrets
  Credentials the agent uses by placeholder. The values stay out of
  the conversation, the screen and the model's context.

  ❯ <secret:ROOT_PASSWORD>    ••••••••  Root password for staging
    <secret:VENICE_API_KEY>   ••••••••
    + Add a secret

  Enter edit · n new · c copy placeholder · d delete · Esc close

──────────────────────────────────────────────────────────────────
```

The list shows every secret's placeholder, a **fixed** eight-dot mask — never
one dot per character, which would put the length on screen — and its
context. The last row adds one. `d` asks before deleting (the hint row turns
red: `Delete <secret:NAME>? d again to confirm`), `c` copies the
placeholder to the clipboard to paste into a message.

The form:

```
  New secret

  Name     ❯ ROOT_PASSWORD
             Use it as <secret:ROOT_PASSWORD>
  Value      ••••••••••
  Context    Root password for the staging box

  Enter next · Tab/↑↓ move · Esc back
```

- **Name** normalizes as it is typed — `root password` becomes
  `ROOT_PASSWORD` — and previews the placeholder under itself.
- **Value** shows one dot per character typed, capped at the field's width,
  so a keystroke is visible without the value being readable; a paste goes
  in whole, surrounding whitespace trimmed on save. Editing an existing
  secret starts it **empty** — `leave empty to keep the current value` — so
  the value is never loaded back into the interface at all.
- **Context** is free text, told to the agent beside the placeholder.
- Enter moves to the next field and saves from the last; Tab, Shift+Tab, ↑
  and ↓ move; Esc returns to the list; Ctrl+C closes the page. A refusal —
  a taken name, a value under four characters — shows in red under the
  fields and moves the focus to the field at fault.

The page is the composer-replacing family's (`docs/view-flow.md`): an
`Option<SecretsPage>` on `App`, built as lines by `ui::secrets_view_lines`
with the value masked **in the builder** — a flowed row is committed to real
scrollback as text, so a mask applied at paint time would be too late. Paste
is routed to the focused field before the composer's catch-all, which would
otherwise put a pasted value into the hidden draft. It works mid-turn: a
save rebinds the next tool call at once (the registry is shared) and the
reminder at the next turn.

## Persistence

`secrets.json` is per **user**, not per directory — a credential belongs to
the person, the `.env` store's posture:

```json
{
  "secrets": [
    { "name": "ROOT_PASSWORD", "value": "…", "context": "Root password for staging" }
  ]
}
```

Every save and delete is a read-modify-write of the file under a
process-wide lock — so two sessions never drop each other's secrets — and a
file that no longer parses is **refused, never overwritten**: the save fails
with a red toast naming it. The page reloads the file when it opens, so a
secret added in another session is there. A record with a bad name, a value
under four characters or a duplicate name is skipped on load.

## API

- `secrets` (pure): `SecretStore` (`apply`, `remove`, `expand`,
  `expand_checked`, `unknown_placeholders`, `unknown_refusal`, `redact`,
  `redact_cut_tail`, `listing`), `SecretRegistry`, `SecretDraft`,
  `SecretMeta`, `SecretValue`, `StreamRedactor`, `expand_arguments`,
  `check_arguments`, `expands_placeholders`, `secret_section`,
  `parse_secrets_file`, `format_secrets_file`, `normalize_name`,
  `validate_draft`.
- `llm::secret_exec` — `run_with_secrets`, the executor seam, and
  `expand_call` for the permission preview (both refusing an unknown
  placeholder).
- `app::secrets` — the page's state and keys; `ui::secrets_view` — its lines.
- `tui::secrets` — load, save, delete, copy.

## Tests

The pure module pins the grammar, both directions, the refusal, the
streaming and truncation rules, the file format and that nothing
`Debug`-prints a value. `llm::secret_exec` drives a fake tool through the
seam: arguments expanded, every output channel redacted, an unknown
placeholder refused unrun, a non-acting tool untouched. The approval tests
pin the placeholder `edit` still asking with its preview redacted, and a
refused call raising no prompt. The page's app and ui tests assert the value
never reaches a rendered line or the composer.

`tests/secrets_wire.rs` runs real `LlmBackend` turns with the real executor
against a provider stand-in on the loopback, and reads every request body
back: a command acting on the value, a `write` → `read` → `edit` round trip
through the placeholder, a password typed into a real terminal session, a
background shell named by its placeholder, and an unknown placeholder refused
with and without the permission gate — the value in no event, no shell report
and no request, the first request's reminder listing the secret.
`tests/live_secrets.rs` (`#[ignore]`d, `A0_VENICE_API_KEY`) asks real models
to use a secret they are told of only by the reminder: write it to a file,
quote a file holding it, log in through a password prompt, survive a
misspelled name, and make a real authenticated request with the very key the
test runs on.

`scripts/smoke/phases/126-secrets.sh` drives the page in tmux — add, mask,
edit, delete, persistence — and `!` commands that expand a placeholder, have
their output redacted and refuse an unknown one; then a second launch drives
the real backend against a stub provider: the secret loaded from the file at
startup, the reminder in the first request, the permission prompt showing the
placeholder, the command writing the value, the cell masking it, an unknown
placeholder refused without a prompt, and the value absent from the pane, the
Ctrl+O transcript, the rollout and every request the stub logged.

# `/secrete` — credentials the agent uses but never sees

The **`/secrete`** command opens a page where the user stores **secrets**: a
name, a value, and an optional line of context. The agent is told the names
and the context, and uses a secret by writing its **placeholder** —
`<secrete:ROOT_PASSWORD>` — in a tool call. The placeholder becomes the real
value at the moment the tool runs, and the value becomes the placeholder
again in everything the tool reports. So the model can log in, call an API or
write a config file with a credential it has never seen, and a screen
recording of the session shows placeholders where the values would be.

The command and the placeholder are spelled `secrete` — the user's spelling,
kept verbatim. Prose, types and files say *secret*.

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

`<secrete:NAME>`, `NAME` being `[A-Z_][A-Z0-9_]*` — an environment
variable's shape, at most 64 characters. Expansion also accepts the name in
lower case and the `<secret:NAME>` spelling, because a model "correcting" the
spelling would otherwise run a command with the literal text in it; redaction
always writes the canonical form. A placeholder naming nothing stored is left
exactly as written, and an inserted value is never scanned again.

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
| a background task's output, screen, command | `BackgroundRegistry` (`with_secrets`): `send_output`, `send_screen`, the stored `Launch.command` |
| the permission prompt's preview | `approval::approve_call` (see below) |
| a `!` command's output | `tui::shell`, before its `ToolEnd` |
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
classifier all see the placeholder — the same text the user approves.

## The reminder

The secrets ride the session's one `<system-reminder>` as a **third listing
section**, after the skills and the agent types (`Session::sync_listings` →
`App::listings`; `docs/context.md`):

```
The user's secrets, as placeholders: write one verbatim in any tool call
argument (…) and the tool gets the real value, inserted as-is, so quote it in
shell commands. Tool output shows the placeholder instead of the value. Use
them whenever a task needs these credentials; the values are hidden from you
on purpose, so never ask for or try to reveal them.

- <secrete:ROOT_PASSWORD>: Root password for the staging box
- <secrete:VENICE_API_KEY>
```

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

  ❯ <secrete:ROOT_PASSWORD>    ••••••••  Root password for staging
    <secrete:VENICE_API_KEY>   ••••••••
    + Add a secret

  Enter edit · n new · c copy placeholder · d delete · Esc close

──────────────────────────────────────────────────────────────────
```

The list shows every secret's placeholder, a **fixed** eight-dot mask — never
one dot per character, which would put the length on screen — and its
context. The last row adds one. `d` asks before deleting (the hint row turns
red: `Delete <secrete:NAME>? d again to confirm`), `c` copies the
placeholder to the clipboard to paste into a message.

The form:

```
  New secret

  Name     ❯ ROOT_PASSWORD
             Use it as <secrete:ROOT_PASSWORD>
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

- `secrets` (pure): `SecretStore` (`apply`, `remove`, `expand`, `redact`,
  `redact_cut_tail`, `listing`), `SecretRegistry`, `SecretDraft`,
  `SecretMeta`, `SecretValue`, `StreamRedactor`, `expand_arguments`,
  `expands_placeholders`, `secret_section`, `parse_secrets_file`,
  `format_secrets_file`, `normalize_name`, `validate_draft`.
- `llm::secret_exec` — `run_with_secrets`, the executor seam, and
  `expand_call` for the permission preview.
- `app::secrets` — the page's state and keys; `ui::secrets_view` — its lines.
- `tui::secrets` — load, save, delete, copy.

## Tests

The pure module pins the grammar, both directions, the streaming and
truncation rules, the file format and that nothing `Debug`-prints a value.
`llm::secret_exec` drives a fake tool through the seam: arguments expanded,
every output channel redacted, a non-acting tool untouched. The approval
tests pin the placeholder `edit` still asking and its preview redacted. The
page's app and ui tests assert the value never reaches a rendered line or the
composer. `scripts/smoke/phases/126-secrete.sh` drives the page in tmux —
add, mask, edit, delete, persistence — and a `!` command that expands a
placeholder and has its output redacted, asserting the value never appears
in the pane.

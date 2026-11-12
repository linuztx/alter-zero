# The conversation context, vision attachments, and the Ctrl+D debug view

Three features share one data model, so they share one document:

1. **The per-session LLM conversation context** — a real backend now sends the
   *whole* conversation every turn, so the model never loses context.
2. **Real vision image paste** — a Ctrl+V-pasted image reaches the model as an
   embedded base64 `data:` URL (OpenAI's multimodal parts form), not a text
   apology.
3. **The Ctrl+D context-debug view** — a full-screen overlay (the Ctrl+O
   pager's sibling) showing the raw context window: exactly what the model is
   sent — image placeholders raw, and every tool call/result as its own native
   `assistant` request + `tool` result entry.

> One provider is the exception to "exactly what the model is sent": under a
> ChatGPT sign-in the request is translated into the **Responses** format at
> the last moment (`docs/providers/chatgpt.md`), so Ctrl+D shows the derived
> conversation *before* that step. It is still the conversation the session
> holds and what a `/resume` restores — the `input` array is a rendering of
> it, not a second source of truth. That translation also depends on a
> property of this derivation: a tool call and its result are always emitted
> together, never one without the other.

## The context is *derived*, not stored

The design decision everything else follows from: there is **no second store**
of the conversation. The pure [`context`](../../src/context.rs) module derives
the context window from `App::history` on demand —
`context_messages(&app.history) -> Vec<ContextMessage>`, or
`context_messages_full(instructions, listings, &app.history)` when the
session's `<system-reminder>` leads it — one block (`reminder::reminder_message`,
`docs/state/project-doc.md`) composed *at derivation* from the two sections `App`
holds: the project's AGENTS.md instructions (`App::user_instructions`,
`project_doc::instructions_section`), then the skills it can load, the
subagent types it can launch and the secrets its tools can use by placeholder
(`App::listings`, `subagents::listing_sections` + `secrets::secret_section`,
`docs/tools/skills.md`, `docs/agents/subagents.md`, `docs/state/secrets.md`);
`context_messages_with(instructions,
&app.history)` is the instructions-only case the tools-free `/compact` turn
sends, and `context_messages_behind(fragment, &app.history)` takes an
already-wrapped block verbatim — a viewed subagent's briefing. The two
emptiness checks deliberately derive *without* the block, so a standing guide
alone is still "nothing to compact". Derived, because `history`
is already the single source of truth the TUI keeps correct everywhere it
matters:

- `/clear` wipes it (and the loop kills the in-flight backend);
- the Esc-Esc backtrack rewind truncates it;
- an Esc interrupt's *undo* pops the submission back out of it;
- a `/resume` load replaces it with the parsed rollout file.

The reminder sits **in front of** history rather than in it, its sections in
a fixed order (the instructions, then the skills, then the agent types, then
the secrets): every
section is re-rendered per turn, so one that moved position would invalidate
the prompt cache behind it, and keeping the block out of `history` means a
backtrack cannot rewind past it and the recorder cannot store it twice. It is
composed where the context derives rather than stored assembled because its
two inputs change at different moments — the instructions at every turn start
and on the **Project docs** toggle, the listings on every rescan and `/model`
switch — and a block assembled at either site would be stale at the other.

A stored context would have to mirror every one of those transitions and
would drift on the first missed one. A derived context *cannot* drift: the
next turn's request is rebuilt from whatever history now says, and the Ctrl+D
view renders the same derivation, so what you see is what is sent.

Each session's context persists via the `/resume` rollout file (the recorder
already mirrors `history` to disk per session — `docs/state/resume.md`): loading a
session back restores its history, and therefore its context, in one move.

### The mapping (`context::context_messages`)

| history item | context entry |
| --- | --- |
| `Message(User)` | `user`, text verbatim (placeholders included) + its image paths |
| `Message(Assistant)` | `assistant`, text verbatim |
| `Tool` (backend) | an assistant `tool_calls` entry (native `{id, name, arguments}` — the model's **verbatim** arguments) folded onto the preceding assistant segment, then a `tool`-role result carrying `ToolCall::context_text()` — the **model-facing** text |
| `Tool` (`!` shell) | `user`, a `$ {command}\n{output}` transcript (the user ran it locally) |
| `Message(Shell)` | skipped — its tool cell above carries the command and output |
| `Message(Error)` | `user`, `[error] {text}` (interrupts and backend failures) |
| `Message(System)` | `user`, `[system] {text}` (slash-command notices) |
| `Summary` | skipped — `Done for Ns` is TUI chrome, not conversation |
| `Reasoning` | skipped — Chat Completions has nowhere to put a previous round's raw chain-of-thought, and re-sending it would burn context for nothing (`docs/ui/thinking-stream.md`) |

A model tool call maps to the **provider-native** Chat Completions shape — the
exact protocol the live agent loop already streams within a turn (`docs/tools/tools.md`),
now replayed across turns: an `assistant` message carrying a `tool_calls`
array, immediately followed by one `tool`-role message per call. The call
**ids are the provider's own**, and a round's calls replay **as the one
message they arrived in**: the agent loop announces every tool round's ids
first (`StreamEvent::RoundCalls`, in the model's order, each with the model's
verbatim arguments — what a call that never reaches its `ToolStart`, a
`⎿ Waiting…` sibling an Esc resolves, is recorded with, `docs/input/interrupt.md`),
the loop stamps them onto the records the round's cells resolve into — `ToolCall::call_id`,
`TaskCallRecord::call_id`, an agent group entry's `call_id` beside its
verbatim `arguments` — under a per-round `batch` number and each call's
`position` in the round, the rollout round-trips all of it, and the
derivation folds the records sharing a batch into one `tool_calls` array
**in the model's order** (a subagent group resolves before the round's
ordinary calls run, so its record lands first whatever order the model made
the calls in; the position puts it back): a parallel batch, its task calls
and its subagent launches on one assistant message, every result behind it
in the same order, an image read's attachment note after the results, and a
notice
that committed mid-batch (a background shell's completion, a subagent's
note) deferred past the batch, since a user-role message cannot sit between
a call and its result. So the request a later turn derives is the one the
provider cached — after a `/resume`, a backend rebuild (a `/settings` or
`/model` switch) and a restart alike, which is what keeps the prompt cache
warm across them (`docs/providers/prompt-caching.md`). A record carrying no id — a
rollout written before the field, the `!` shell, a lone scripted call —
gets a synthetic `call_N` minted clear of every recorded id, so the pairing
stays consistent within the request; it is a round of its own. The replayed
`arguments` are the model's own, **verbatim**:
alongside the one-line header summary (`● Write(a.py)`) every call records the
raw JSON it was made with (`ToolCall::arguments`, carried on
`StreamEvent::ToolStart`, round-tripped through the rollout), and
`reconstruct_arguments` returns it whenever it parses as an object.

That used to be a *reconstruction* from the summary — `{"command": …}` for
`bash`, `{"path": …}` for the file tools — which is lossy in a way that
mattered: a `write` replayed as `write({"path": "a.py"})`, the model watching
itself create a file with no content, while the whole content rode the tool
**result** as a numbered body re-uploaded every turn. Recording the arguments
makes the replay lossless (a `write`'s `content`, an `edit`'s two strings and
`replace_all`, a `bash` call's `timeout`) and is what lets the file tools'
result collapse to one line (`docs/tools/tools.md`): the change is on the *call*
now, not in the result. The reconstruction stays as the fallback for records
that carry no arguments — every rollout written before the field, the `!`
shell, a hand-scripted event — and for the odd damaged record, since a
validating provider rejects arguments that are not a JSON object.

An **image `read`** (detected from the stored record: `name == "Read"` + the
`Read image ` output marker) additionally replays the follow-up user note the
live loop attached — `llm::tools::image_attachment_note` over the path as an
`images` attachment — so later turns keep *seeing* the image, served from the
session's one shared encoding like a Ctrl+V paste (`docs/tools/tools.md`,
`docs/architecture/memory.md`).

### The result the model read, not the cell it saw

A tool's result is `ToolCall::context_text()`: `context_output` when the call
recorded one, else its displayed `output`. The two are the same for every
ordinary call — but a **permission rejection** (`docs/state/permissions.md`) and a
`write`/`edit` (`docs/tools/tools.md`) each resolve with two texts on purpose. The red cell reads

```
⎿ User rejected write to hello.py
  Instructions: use pathlib instead
```

while the model was handed the full stop-and-wait instruction with the same
feedback appended. Replaying the cell text would hand a later turn a *different*
tool result than the one the live round sent — dropping the user's instructions
from the conversation entirely, one turn after they were given. Storing both
keeps the replay honest: what Ctrl+D shows, and what the next request carries,
is exactly what the model was told. A `write`/`edit` splits for the opposite
reason — not to keep something, but to drop it: the cell keeps the numbered
content or diff hunks while the model reads

```
File created successfully at: /tmp/a.py (file state is current in your context — no need to read it back)
```

which is honest only because the arguments above it replay verbatim. (The
`Backgrounded` split runs the other way: `output` holds the model-facing launch text and the *cell* row is
synthesized from the status, so `context_output` stays `None` there.)

Adjacent same-role **plain-text** entries still **merge** (texts joined with a
blank line, attachments concatenated) so message batches and notice runs
collapse; tool-call assistant messages and tool-result messages are never merge
targets (a result must sit between them). Mapping the TUI notices to *user*-role
bracketed notes rather than mid-conversation `system` messages is deliberate
wire-compatibility (strict providers reject non-leading system messages); the
native tool round-trip is accepted by any provider that supports function
calling, which is exactly the set that would emit tool calls in the first place.
A `!` shell command rides back as a `user` `$ command` transcript — it is the
*user's* local action, not a model tool call, so it has no assistant
`tool_calls` entry to pair a native `tool` message to.

## The seam

`ReplySource::spawn` gains a `context: Vec<ContextMessage>` parameter:

```rust
fn spawn(&self, prompt: String, images: Vec<PathBuf>,
         context: Vec<ContextMessage>, tx: …, cancel: …) -> JoinHandle<()>;
fn system_prompt(&self) -> Option<String> { None }  // for the Ctrl+D view
```

`tui::turn::Session::start_turn` records the turn's user message(s) into history first
(as it always did), then derives the context — so its **last entry is the
current user message**, text and attachments included — and hands it to the
backend. `prompt`/`images` still travel for the backends that want them:
`DummyAi` ignores the context entirely (canned replies; smoke.sh is
untouched), while `LlmBackend` builds its request from the context alone
(falling back to the bare `prompt` only if the context were ever empty, so a
request can never be user-less).

`llm::backend::build_messages(system_prompt, prompt, context, encode_image)`
is the pure assembly: the optional system prompt, then one wire message per
context entry. It is generic over `encode_image: Fn(&Path) -> Option<String>`
— the file-reading seam — so it unit-tests with a fake encoder and no disk.

## Vision: images on the wire

`app::Message` gains `images: Vec<PathBuf>` — a user message *owns* its
Ctrl+V attachments (the `[Image #N]` placeholders stay in the recorded text;
the paths ride beside it — and **this derivation** annotates each placeholder
with its path, `[Image #N: {path}]`, so the model knows where the picture was
saved and Ctrl+D shows the same text the wire carries:
`paste::annotate_image_placeholders`, run per message before the merge so a
batch's drafts each name their own pictures — `docs/input/image-paste.md`). The
submit path stages the whole `(placeholder, path)` pairs
and records each path onto the message whose text carries its placeholder, in
text-occurrence order (`paste::distribute_images` — so a merged batch's
duplicate `[Image #1]`s resolve to their own drafts' paths, and a
within-draft reorder records faithfully). The session file round-trips the
paths (a new optional `images` field on the message record — omitted when
empty, so old files and imageless lines keep the old shape; written as lossy
strings so a non-UTF8 path can't panic the recorder). The interrupt-undo
**and** the Esc-Esc backtrack rewind both restore them to the composer:
occurrences zipped back over each message's recorded paths
(`paste::image_placeholder_occurrences`), any pairs backing a clobbered draft
discarded first, and — for a backtrack — the dropped *later* user messages'
orphaned attachments queued for deletion.

`ChatMessage.content` is now `MessageContent::Text(String) |
Parts(Vec<ContentPart>)` (serde-`untagged`, so imageless messages keep the
classic `"content": "…"` string every OpenAI-compatible endpoint accepts).
An image-carrying context message becomes the parts array: one `text` part,
then one `image_url` part per attachment, each a
`data:image/{png,jpeg,gif,webp};base64,…` URL (`image_data_url`, reusing
`clipboard`'s tested RFC 4648 encoder; MIME by extension, PNG default —
the clipboard writer only produces those formats). Encoding happens **on the
backend thread**, never the event loop. An attachment whose file has vanished
(a paste folder someone cleared — pastes live under the config home now, so
an OS temp cleaner no longer takes them) is *noted in the text*
(`[image unavailable: {path}]`) rather than dropped silently.

Because past user messages keep their paths, earlier turns' images are
**re-sent on every later request** — the model can be asked a follow-up about
an image three turns back (codex re-sends the same way). This is also why
submitted pastes are deliberately kept in the session's paste folder
(`docs/input/image-paste.md`).

## The Ctrl+D context-debug view

A third alternate-screen overlay, `View::ContextDebug`, with the Ctrl+O
pager's chrome: the slash-tiled `/ C O N T E X T` title row, a scrolling
body with vi-style `~` filler, the `─` separator carrying the scroll
percentage (and, on its left end, the window's summary), and two dim
key-hint rows (`↑/↓`, `←/→` between messages, pgup/pgdn, home/end;
q/esc/ctrl+d close). It opens with **Ctrl+D** from the conversation — mid-turn
too, like Ctrl+O — and is inert where Ctrl+O is inert (the overlays share the
alt screen and never stack: Ctrl+D does nothing under the pager or the
`/resume` picker, Ctrl+O nothing under Ctrl+D). Scrolling is its own state
(`App::debug_scroll`/`debug_follow`, settled per-draw like the pager's), so
flipping between overlays never clobbers the transcript's place.

The body (`ui::context_body`, its rows alone `ui::context_lines`) is the raw
context window, one entry per message the request carries:

```
/ C O N T E X T / / / / / / / / / / / / / / / / / / / / / / / / / / / / / / / /
 SYSTEM  #1 · ≈1.8k tokens            (amber chip — the backend's prompt)
  ⎿  # System prompt
     You are Alter Zero, an autonomous agent harness …
     ## Environment · Date … OS … User … CWD …           (docs/architecture/environment.md)

 USER  #2 · ≈40 tokens · 1 image      (blue chip — context_user_color(), the one
                                       blue the running bullet left behind)
  ⎿  [Image #1: /home/me/.alter-zero/image-cache/773c1c6cb321/1.png] what's in
     this picture?
     image: /home/me/.alter-zero/image-cache/773c1c6cb321/1.png  (dim attachment)

 ASSISTANT  #3 · ≈25 tokens · 1 tool call                       (green chip)
  ⎿  Let me look at the file.
     → read({"path":"src/main.rs"})  call_1      (the arrow and name purple,
                                                  bold; the call's id dim)

 TOOL  #4 · read · call_1 · ≈310 tokens   (purple chip — names the call it answers)
  ⎿  1 fn main() { … }

─ 4 messages · ≈2.2k tokens ─────────────────────────────────────────── 100% ─
 ↑/↓ to scroll   ←/→ prev/next message   pgup/pgdn to page   home/end to jump
 q/esc/ctrl+d to quit   tab for classifier context
```

— the system prompt first (injected at the boundary via
`App::set_system_prompt` from `ReplySource::system_prompt()`, at startup and
on every `/model` switch; the dummy has none — the real backend's is the
persona, then the runtime **environment context** of date/os/user/cwd:
persona → environment, `docs/architecture/environment.md`), then every derived context
message. Each entry has two parts:

- **The header — a role chip and its facts.** The wire role upper-case
  (`SYSTEM`, `USER`, `ASSISTANT`, `TOOL`) on a fill of the role's hue under
  the theme's on-accent ink, bold — the agent label's and the ask
  countdown's dress (`context_chip_fg`). The lower-case `user:` tags this
  replaced read as a word of the text under them, and on a page of raw text
  the header is the row the eye hunts for. Beside the chip, dim: the entry's
  `#n` place in the request, the **call a result answers** — its tool,
  resolved through its `tool_call_id` from the calls above it, then that id,
  which the call's own row ends on, so a parallel batch's results pair with
  their calls by eye (a bare result used to say nothing of which call it
  belonged to) — a **token estimate** of its size marked `≈` like the
  `/context` page's, and how many tool calls or pictures it carries. The
  estimate is the footer gauge's own rule for one message
  (`app::message_tokens`, which `estimate_messages_tokens` sums): the text, a
  flat charge per picture, each call's name and arguments — so the entries,
  and the bottom rule's total, add up to what the gauge reads offline. A
  header is always one row, cut with `…` at the width.
- **The body, hung on the tool cell's `⎿` gutter** (`TOOL_RESULT_PREFIX`,
  through `file_cell::gutter_row_spans`): the corner on the first row, every
  other row aligned under it. The text is **word-wrapped with its own
  spacing kept** (`wrap_output`, never the markdown renderer — the whole
  point is the unformatted wire content; the hard mid-word break it replaced
  turned `scripted` into `a s` / `cripted`), its tabs expanded, its trailing
  blank lines dropped (a model's `…\n\n` before a tool call stacked empty
  rows between the text and the call it introduces), and the
  `<system-reminder>` markers dimmed so the injected block reads apart from
  the message it leads. An assistant entry's native tool calls follow as
  `→ name(arguments)` rows — the arrow and the name purple and bold, the
  arguments in the body's ink, word-wrapped hanging under the `→ name` lead
  (the tool header's rule; a terminal too narrow for the lead wraps the whole
  call verbatim), closed by the call's dim id beside its last row, or on a
  row of its own where the two do not fit — and attachment paths dim beneath, wrapped verbatim since a
  path has no spaces worth breaking at. An entry with nothing in it says
  `(empty)`: an empty result still went over the wire. Below the gutter's
  width plus room for one wide glyph, the gutter gives its columns back.

Image placeholders, the raw tool-call JSON, and the tool results appear raw
here and only here.

**Finding your way.** A window is a few dozen messages over thousands of
rows, so the view carries three aids, all fed by `DebugBody::headers` — the
row each entry's header sits on, built beside the rows:

- **←/→ step between messages**, opening the previous or next entry at the
  top of the body. The key records only the direction (`App::debug_jump`, an
  `EntryJump`): where the neighbouring entry starts is a fact about the page
  as built — its width, its wrapping — so the draw resolves it against the
  rows it is about to paint (`App::resolve_debug_jump`, before the scroll
  settle), never against a previous frame's. ← from inside an entry opens
  that entry's own head first, the way a paragraph motion does; past the
  last entry → is the end and before the first ← is the top; the window's
  top row is the page's current end while it tail-follows, so a page that
  grew since its last draw is stepped from its new bottom; and a step that
  lands within the last screenful re-engages the tail-follow, as End does.
  Tab and a close drop a step still pending, so it never lands on a page it
  was not pressed on. They are the keys the transcript pager's backtrack
  uses for the previous and next message, meaning the same thing here.
- **The entry the window opens inside is pinned** over the top row while its
  own header is scrolled off (`pinned_header`): deep in a long system prompt
  or tool result the top row still says whose text it is. Not on the header
  row itself (it is already on top), not once only the entry's spacer would
  sit under the pin (a header stacked on the next one), and not in a body
  shorter than `CONTEXT_PIN_MIN_ROWS`, where the pin would cost the text it
  names.
- **The bottom rule counts the window**: `5 messages · ≈1.2k tokens` set into
  its left end (`transcript::labeled_separator`), the percentage keeping its
  place — left out whole when the rule cannot hold both.

While the
view is up the loop keeps draining stream events (their commits queue on the
viewport, invariant 4) and the tail-follow keeps the newest entries in view;
closing flushes the queued commits and repaints the live region exactly like a
Ctrl+O return. The `?` shortcuts band gains `ctrl+d for llm context`.

The view shows the context as of *finished* items: an in-flight partial reply
lives in the streaming buffer, entering the window (and the next request)
when its segment lands in history.

### The window is cached (`ui::ContextCache`)

Deriving the window (`context_messages_full` over the whole history) and
wrapping every entry is **O(conversation)** — and the view redraws on
every animation frame while a turn runs (the loop's 32 ms status re-arm), plus
once per scroll key. Rebuilt per frame, a big context — a loaded skill's whole
rendered body rides the window — pegged the event loop and starved the scroll
keys: the reported "Ctrl+D freezes the TUI" bug, worst exactly when the user
opens the view to read what a long skill injected. So the loop owns a
`ui::ContextCache` (the `TranscriptCache`'s little sibling, `docs/
tool-view-performance.md`): `draw_context_view` asks it for the built page
(the scroll clamp, `tool_view_max_scroll_for` — the pager formula, shared, and
the ←/→ step resolve against the same build the render paints), and the cache
rebuilds **only when its signature changes** —
history generation + length, the width, the viewed agent (id + its own
transcript length), and the leading fragments' lengths (system/agent prompt,
the `AGENTS.md` instructions section, the listing sections, a viewed agent's
briefing — they only otherwise change beside a turn-start history append, and
the lengths catch the direct edits: a `/settings` toggle dropping the
instructions, a `/model` switch swapping the prompt). A scroll key or a status tick is a cache hit (O(viewport) to window
the rows); a streamed chunk doesn't invalidate it at all (the window shows
finished items only). No incremental prefix is needed — unlike the
transcript, the window only changes at item boundaries, never per chunk.

The headers' token estimates put every text through the estimator, which on
a long conversation is the most expensive part of a build — so the cache
keeps a `context_usage::CountMemo` (the `/context` page's own) across
rebuilds, and across a close too, being counts and never text: a rebuild for
one new message tokenizes that message and serves every other count from the
memo (`context_cache_counts_only_what_is_new`).

Inside an **agent session view** (`docs/agents/agent-tool.md`) the same overlay
debugs the *viewed agent's* context instead: the body derives from that
agent's own transcript through the same mapping, and the `SYSTEM` entry
shows the prompt a subagent is **actually sent** — the main prompt with
the subagent note appended (`prompts/subagent.md`), surfaced as
`ReplySource::agent_system_prompt()` and injected beside the main one
(`App::set_agent_system_prompt`) — with no AGENTS.md section, because
subagent conversations start fresh without one (`llm::backend`'s
`run_agent_calls`), and with the agent's own briefing — the skills section
wrapped alone, exactly as the launch sent it — taken verbatim in front of
its transcript (`context_messages_behind`, `docs/agents/subagents.md`). So the
note's presence is verifiable right where the user looks for it. And the
title says whose window it is — `C O N T E X T · explore agent · Fetch the
weather`, the agent named the way its roster row names it — since nothing
else on the page tells the lead's window from an agent's. `overlay_header`
cuts that title by display columns rather than characters (a description is
whatever the model wrote, CJK included, and a cut at `width` characters is
up to twice the row wide) and starts its slash tiling with a blank, so the
tiling never glues a `/` onto a title ending on the other parity.

## Also fixed while wiring: the default system prompt was dead

`build_backend` passed the raw `ALTER_ZERO_SYSTEM_PROMPT` env read straight
through, so with the var unset the real backend got **no** system prompt —
`DEFAULT_SYSTEM_PROMPT` was unreachable, contradicting `docs/providers/llm.md`
("Override with…"). Now: unset → the default; set → the override; set to
empty → no system prompt at all (`with_system_prompt` drops blanks). The
runtime environment context (date/os/user/cwd) is folded onto any **non-empty**
base — default or override — so the empty → no-prompt contract still holds
(`docs/architecture/environment.md`).

## Known limitations (v1)

- The status line's `↑` token estimate still counts only the new turn's input,
  not the re-sent history (the tally was always an estimate).
- There is no context-window **cap**: a very long session sends its whole
  history until it hits the provider's limit (the provider's error surfaces
  in-band like any other). Trimming/summarising is future work.
- Session files record attachment *paths*, not bytes: resuming after the OS
  temp-cleaner ran sends the `[image unavailable]` note instead of the image.
- The undo/backtrack attachment re-key zips a message's placeholder
  occurrences over its recorded paths; a placeholder typed *by hand* (never
  attach-backed) in the same message can shift that pairing — the string-keyed
  scheme's known edge (`docs/input/paste.md`).
- ~~A replayed tool call's `arguments` is reconstructed from history's
  one-line summary~~ — **fixed**: every call records the model's verbatim
  arguments and replays them (see above). The reconstruction survives only as
  the fallback for rollouts written before the field, where the limitation
  still reads as written.
- Replaying native `tool_calls`/`tool` messages assumes the provider supports
  function calling (the same providers that would emit tool calls). Resuming a
  tools-on session with `ALTER_ZERO_TOOLS=0` would replay tool messages to a
  request that declares no tools — a narrow edge a strict provider could reject.

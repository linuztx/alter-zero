# The conversation context, vision attachments, and the Ctrl+D debug view

Three features share one data model, so they share one document:

1. **The per-session LLM conversation context** — a real backend now sends the
   *whole* conversation every turn, so the model never loses context.
2. **Real vision image paste** — a Ctrl+V-pasted image reaches the model as an
   embedded base64 `data:` URL (OpenAI's multimodal parts form), not a text
   apology.
3. **The Ctrl+D context-debug view** — a full-screen overlay (the Ctrl+O
   pager's sibling) showing the raw context window: exactly what the model is
   sent, placeholders and bracketed tool formats unrendered.

## The context is *derived*, not stored

The design decision everything else follows from: there is **no second store**
of the conversation. The pure [`context`](../src/context.rs) module derives
the context window from `App::history` on demand —
`context_messages(&app.history) -> Vec<ContextMessage>` — because `history`
is already the single source of truth the TUI keeps correct everywhere it
matters:

- `/clear` wipes it (and the loop kills the in-flight backend);
- the Esc-Esc backtrack rewind truncates it;
- an Esc interrupt's *undo* pops the submission back out of it;
- a `/resume` load replaces it with the parsed rollout file.

A stored context would have to mirror every one of those transitions and
would drift on the first missed one. A derived context *cannot* drift: the
next turn's request is rebuilt from whatever history now says, and the Ctrl+D
view renders the same derivation, so what you see is what is sent.

Each session's context persists via the `/resume` rollout file (the recorder
already mirrors `history` to disk per session — `docs/resume.md`): loading a
session back restores its history, and therefore its context, in one move.

### The mapping (`context::context_messages`)

| history item | context entry |
| --- | --- |
| `Message(User)` | `user`, text verbatim (placeholders included) + its image paths |
| `Message(Assistant)` | `assistant`, text verbatim |
| `Tool` (backend) | `assistant`, raw format: `[tool {name}({args}) {ok\|failed}]\n{output}` |
| `Tool` (`!` shell) | `user`, raw format: `[shell {ok\|failed}] $ {command}\n{output}` |
| `Message(Shell)` | skipped — its tool cell above carries the command and output |
| `Message(Error)` | `user`, `[error] {text}` (interrupts and backend failures) |
| `Message(System)` | `user`, `[system] {text}` (slash-command notices) |
| `Summary` | skipped — `Done for Ns` is TUI chrome, not conversation |

…then **adjacent same-role entries merge** (texts joined with a blank line,
attachments concatenated), so an assistant segment and the tool record that
split it become one assistant message and the derived sequence strictly
alternates `user`/`assistant`. That shape — plus mapping the TUI notices to
*user*-role bracketed notes rather than mid-conversation `system` messages —
is deliberate wire-compatibility: strict OpenAI-compatible providers
(alternation chat templates) reject consecutive same-role messages and
non-leading system messages.

Every message type therefore reaches the context (the user-visible ask), in
one of two shapes: verbatim conversation text, or a **raw bracketed record**.
The bracketed forms are what the wire carries and what Ctrl+D shows; the
inline TUI keeps rendering the same items prettily from `history`. The
default system prompt (`llm::backend::DEFAULT_SYSTEM_PROMPT`) tells the model
what the brackets mean so it treats them as context, not as a format to
imitate. A backend tool call rides back as an `assistant` entry (the model
ran it); a `!` shell command rides back as a `user` entry (the user ran it
locally).

## The seam

`ReplySource::spawn` gains a `context: Vec<ContextMessage>` parameter:

```rust
fn spawn(&self, prompt: String, images: Vec<PathBuf>,
         context: Vec<ContextMessage>, tx: …, cancel: …) -> JoinHandle<()>;
fn system_prompt(&self) -> Option<String> { None }  // for the Ctrl+D view
```

`main.rs::start_turn` records the turn's user message(s) into history first
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
Ctrl+V attachments (the `[Image #N]` placeholders stay in the text; the paths
ride beside it). The submit path stages the whole `(placeholder, path)` pairs
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
orphaned attachments queued for temp-file deletion.

`ChatMessage.content` is now `MessageContent::Text(String) |
Parts(Vec<ContentPart>)` (serde-`untagged`, so imageless messages keep the
classic `"content": "…"` string every OpenAI-compatible endpoint accepts).
An image-carrying context message becomes the parts array: one `text` part,
then one `image_url` part per attachment, each a
`data:image/{png,jpeg,gif,webp};base64,…` URL (`image_data_url`, reusing
`clipboard`'s tested RFC 4648 encoder; MIME by extension, PNG default —
the clipboard writer only produces those formats). Encoding happens **on the
backend thread**, never the event loop. An attachment whose temp file has
vanished (e.g. the OS cleaned `/tmp` between sessions) is *noted in the text*
(`[image unavailable: {path}]`) rather than dropped silently.

Because past user messages keep their paths, earlier turns' images are
**re-sent on every later request** — the model can be asked a follow-up about
an image three turns back (codex re-sends the same way). This is also why
submitted temp files are deliberately left on disk (`docs/image-paste.md`).

## The Ctrl+D context-debug view

A third alternate-screen overlay, `View::ContextDebug`, with the Ctrl+O
pager's exact chrome: the slash-tiled `/ C O N T E X T` title row, a
scrolling body with vi-style `~` filler, the `─` separator carrying the
scroll percentage, and two dim key-hint rows (`↑/↓`, pgup/pgdn, home/end;
q/esc/ctrl+d close). It opens with **Ctrl+D** from the conversation — mid-turn
too, like Ctrl+O — and is inert where Ctrl+O is inert (the overlays share the
alt screen and never stack: Ctrl+D does nothing under the pager or the
`/resume` picker, Ctrl+O nothing under Ctrl+D). Scrolling is its own state
(`App::debug_scroll`/`debug_follow`, settled per-draw like the pager's), so
flipping between overlays never clobbers the transcript's place.

The body (`ui::context_lines`) is the raw context window:

```
system prompt:                        (amber tag — the backend's prompt)
  You are a helpful assistant …
user:                                 (blue tag)
  [Image #1] what's in this picture?
  image: /tmp/inline-tui-clipboard-x.png    (dim attachment row)
assistant:                            (green tag)
  [tool Read(src/main.rs) ok]
  fn main() { … }
```

— the system prompt first (injected at the boundary via
`App::set_system_prompt` from `ReplySource::system_prompt()`, at startup and
on every `/model` switch; the dummy has none), then every derived context
message: a coloured `role:` tag over its text wrapped **verbatim**
(`wrap_verbatim`, never the markdown renderer — the whole point is the
unformatted wire content), attachment paths dim beneath. Placeholders and the
bracketed tool/shell/notice formats appear raw here and only here. While the
view is up the loop keeps draining stream events (commits stay gated on the
conversation view, invariant 4) and the tail-follow keeps the newest entries
in view; closing repaints the inline conversation exactly like a Ctrl+O
return. The `?` shortcuts band gains `ctrl+d for llm context`.

The view shows the context as of *finished* items: an in-flight partial reply
lives in the streaming buffer, entering the window (and the next request)
when its segment lands in history.

## Also fixed while wiring: the default system prompt was dead

`build_backend` passed the raw `INLINE_TUI_SYSTEM_PROMPT` env read straight
through, so with the var unset the real backend got **no** system prompt —
`DEFAULT_SYSTEM_PROMPT` was unreachable, contradicting `docs/llm.md`
("Override with…"). Now: unset → the default; set → the override; set to
empty → no system prompt at all (`with_system_prompt` drops blanks).

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
  scheme's known edge (`docs/paste.md`).
- Tool calls are replayed in the bracketed raw format, not the provider's
  native `tool_calls` protocol (there is no live tool-calling loop yet —
  `docs/llm.md`).

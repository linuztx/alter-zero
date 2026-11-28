# Image generation (`imagegen`)

Signed in with **ChatGPT Codex**, a session can make and edit raster images
through OpenAI's Images 2.6 (`gpt-image-2.6-sunburst`): photos,
illustrations, sprites, textures, mockups and cutouts with a transparent
background. It works whatever provider the conversation runs on. A Claude,
OpenRouter or Ollama model calls the same `imagegen` tool, and the image is
made with the ChatGPT sign-in, billed to that ChatGPT plan. No API key is
involved.

There are two halves, ported from OpenAI Codex:

- **The `imagegen` tool**: native, offered while ChatGPT Codex is signed in.
  It is Codex's built-in `image_gen.imagegen`, which runs on the client side.
- **The `imagegen` skill**: Codex's system skill, the sixth built-in here
  (`docs/skills.md`). It covers when to make a bitmap and when to write
  SVG/CSS, how to shape the prompt, and where the result belongs. It also
  carries Codex's fallback CLI (`scripts/image_gen.py`, which needs
  `OPENAI_API_KEY`) for the controls the tool does not take. It is listed
  only while the tool is offered.

## What Codex does, and what was kept

Codex's built-in tool (`codex-rs/ext/image-generation`) is not the hosted
Responses `image_generation` tool. That path is legacy, and Codex no longer
sends it. The model is offered a function. When the model calls it, Codex
POSTs JSON to the Codex backend's Images endpoints using the same ChatGPT
credential as its model requests:

```
POST https://chatgpt.com/backend-api/codex/images/generations
{"prompt": "…", "background": "opaque", "model": "gpt-image-2", "quality": "auto", "size": "auto"}

POST https://chatgpt.com/backend-api/codex/images/edits
{"prompt": "…", "images": [{"image_url": "data:image/png;base64,…"}], "background": "transparent", …}
```

The answer is `{"created": …, "data": [{"b64_json": "…"}], …}`. Codex saves
`data[0]` under `$CODEX_HOME/generated_images/<thread>/<call>.png`. It hands
the model the picture itself together with a hint naming the saved path.

Alter Zero takes the same endpoints, the same credential and the same
answer. What it sends them is in *The tool* and *The call* below; the other
differences are these:

| | Codex | Alter Zero |
| --- | --- | --- |
| Tool | `image_gen.imagegen`, a Responses namespace | `imagegen`, a plain function on every wire |
| When offered | ChatGPT auth **and** a provider that requires OpenAI auth, an image-capable model, not the Free plan | the ChatGPT Codex sign-in is stored, **whatever provider the session runs on**; a Free-plan account is refused when the call runs |
| Edit inputs | `referenced_image_paths` or `num_last_images_to_include` | the same two, never both: a path for a picture with a file (a paste, a `read`, a generated image), the count for one without (an MCP tool's), which goes up as the conversation's own copy (*Images already in the conversation*) |
| Saved to | `$CODEX_HOME/generated_images/<thread>/<call>.png` | `~/.alter-zero/generated_images/<session>/<N>.png` |
| What the model sees | the picture | the picture, or a copy held to the attachment cap when the picture is past it |
| GIF output | behind an under-development flag | not offered: the backend refused every GIF tried (*Tried live*) |
| Skill visibility | always | only while the tool is offered |
| Switch | none | the `/settings` **Image generation** row, per directory |

The tool exists for one reason: a session on another provider has no other
way to reach a ChatGPT plan's image quota. The conversation's provider and
the image provider are separate, much as `/usage` reads every subscription
that is signed in, not just the one in use (`docs/usage.md`). That is why
the gate is the stored sign-in, not the active model.

## The tool

```json
{"name": "imagegen",
 "parameters": {"type": "object",
   "properties": {
     "prompt": {"type": "string"},
     "transparent_background": {"type": "boolean"},
     "referenced_image_paths": {"type": "array", "items": {"type": "string"}},
     "num_last_images_to_include": {"type": "integer", "minimum": 1, "maximum": 5}},
   "required": ["prompt"], "additionalProperties": false}}
```

The description is short on purpose: every word rides every request that
offers the tool (CLAUDE.md's tool-prose rule), and the craft Codex's 315-word
`imagegen_description.md` spells out — when to make a bitmap, how to word a
prompt, where a picture belongs — is the skill's, which is listed whenever the
tool is offered. What stays is the capability and its sharp edges: one image
per call, taking minutes, saved and shown with its path returned, no asking
for confirmation, edits by `referenced_image_paths` after looking with `read`
(where Codex names `view_image`) or, for a picture with no file, by
`num_last_images_to_include`, and `transparent_background` for a cutout.
There is no code mode and no GIF (*Tried live*).

The tool takes no size, quality or count parameter, because Codex's sends
`size: "auto"` and `quality: "auto"` and lets the backend choose. A model
that needs those controls is pointed at the skill's CLI by the skill.

An unknown argument is refused, with the list of accepted ones. That is
Codex's `deny_unknown_fields`. A model that passes `size` learns at once that
it does nothing, instead of believing it got a landscape image. A `gif` is
refused the same way, before anything is sent: GIF output is not offered
(*Tried live*).

### Images already in the conversation

`num_last_images_to_include` names a picture that has no file. Here that is
an MCP tool's image result: a paste, an image `read` and a generated image
each have one, and go by `referenced_image_paths`. The count
asks for the newest 1 to 5 images the conversation holds:

- **Which.** The image parts of the request the model wrote the call
  against. Every round records the pictures the request it sends carries
  (`imagegen::ConversationImages`, filled in the round closure of
  `llm::backend` for the lead and a subagent alike, and read by the
  executor), so a call sees what the model saw when it wrote it — never a
  picture an earlier call of the same round has just added. Whoever sent a
  picture, it counts: a paste, a `read`, an image made earlier, an MCP tool's
  result. What `/compact` folded away is gone, and a model that cannot see
  images has none.
- **Order.** The newest `n`, sent oldest first. The same picture twice counts
  twice, since a picture with no file has no name to tell a repeat by.
- **What goes up.** The conversation's own copy: the encoding the request
  already carries, shared rather than copied (`docs/memory.md`). A picture
  past the attachment cap goes up as the smaller copy the model saw, and a
  GIF as its frames tiled. `referenced_image_paths` reads the file instead.
- **Refusals**, before anything is sent: a count outside 1 to 5 (`` `num_last_images_to_include` must be between 1 and 5 ``), a count
  beside paths (``provide only one of `referenced_image_paths` or
  `num_last_images_to_include` ``; an empty list of paths names none), and a
  count past what the conversation holds (`requested the last 2 conversation
  images, but only 1 were available`). A request never goes with fewer
  pictures than the model asked for.

The schema states the 1-to-5 bounds as `minimum` and `maximum`, and the
parameter's description says them in words for a wire that drops the
keywords.

### The call

1. **Arguments.**
   - The prompt must not be blank.
   - At most 5 referenced images, or a count of the newest images in the
     conversation checked as *Images already in the conversation* says.
   - Each reference resolves against the working directory, the file tools'
     rule. It must be a PNG, JPEG, WebP or GIF, judged by its content, and
     under the Images API's 50 MB limit. A GIF goes up as its first frame
     (*A GIF the model looks at*).
2. **Credential.** The sign-in is read from a shared handle the boundary
   keeps current, so a `/login` mid-session reaches the next call without a
   rebuild (`ImagegenSetup`). The access token is minted through the same
   cache every ChatGPT request uses (`docs/chatgpt.md`): a session running on
   ChatGPT shares one bearer with its chat stream, and a rotation is written
   back as usual. An account whose token names the `free` plan is refused
   here, without a request.
3. **Request.** The POST carries:
   - the bearer;
   - the provider's `extra_headers` (`originator`);
   - the credential's headers (the Codex user agent, `chatgpt-account-id`);
   - a fresh `x-codex-image-turn-id`. Codex sends its turn's id there; this
     app has no turn id, so each call gets a UUID of its own. Codex sends no
     `version` header, and neither does this.

   The body is `prompt`, the `images` of an edit, `background`, `model`, and
   `quality` and `size` always `auto`:

   ```
   {"prompt": "…", "background": "opaque", "model": "gpt-image-2.6-sunburst",
    "quality": "auto", "size": "auto"}
   ```

   `background` is never `auto`: `transparent` when the call asked for a
   cutout, else `opaque`. The model is *The model*'s.

   Each reference image goes up as a `data:` URL taken from the session's one
   shared encoding (`images::attachment_data_url`, `docs/memory.md`), and an
   image named by count as the conversation's own copy. A picture already in
   the conversation costs no second copy, and
   **Auto-resize images** applies as it does to every upload. The edit body
   is streamed (`llm::body::streamed_request`), never built whole.
4. **Waiting.** A generation takes from tens of seconds to a few minutes. The
   whole request runs on a worker thread while the tool thread polls the
   turn's `CancelToken` every 20 ms (`llm::off_thread`, the MCP transport's
   rule), so Esc ends the call at once. The worker's own deadline is ten
   minutes. A refused connection, or a 5xx where nothing was answered, is
   tried twice more with backoff. A 429 is never retried: it is a usage
   limit. Neither is a connection lost **after** the request went out — a
   body cut short, a timeout: the backend may already have made, and
   billed, the picture, and a second request would make it twice (Codex
   retries those; this is deliberately narrower). A worker that dies without
   an answer reads as a failure, never as the user's Esc.
5. **The answer** is read as it arrives, never held whole
   (`llm::imagegen::scan::split_response`):
   - A scanner over the body streams the first `data[].b64_json` string
     through a base64 decoder straight into a staging file in the output
     folder.
   - Every other byte is passed over: the scanner keeps only the few bytes of
     the string it is in that could still be the `b64_json` key. Whatever else
     the answer carries — Codex's per-image `image_metadata`, a revised prompt
     — never fails the call, since the picture was made, and paid for, before
     the answer arrived.
   - A 4K PNG is twenty-odd MB of base64. Read whole, it would be a
     picture-sized block on a thread arena that glibc never returns
     (`docs/memory.md`). Streamed, the call costs kilobytes whatever the
     image weighs (`tests/image_generation_memory.rs`).
   - The staged file's header must parse as a picture. Its format is sniffed
     from its content. Then it takes the folder's next number (the paste
     folder's exclusive-create numbering, `clipboard`), with the extension its
     content calls for. A cancelled or refused body leaves nothing behind.
6. **Result.** The call's output is one fact line:

   ```
   Generated image (PNG, 1536x1024, 2.1 MB) saved to /home/me/.alter-zero/generated_images/1763812345/1.png
   ```

   The cell shows it as two rows under `● ImageGen({prompt})`: the facts,
   then `Saved to` and the file. The file is shown by the path display rule
   (`~/.alter-zero/…`) and links to itself (`docs/links.md`). While the call
   runs, the corner row counts the call's own time,
   `⎿ Generating image… (42s)`: a bare `Running…` that lasts a minute reads
   as stuck. The model reads the same line plus Codex's hint: where generated images
   go, to copy one where the project needs it and leave the original, and
   that the user already sees it, so there is no need to embed it in the
   reply. That is the `ToolOutcome::context` split (`docs/tools.md`). The
   picture itself:
   - is attached to the model's context after the round's results, like an
     image `read`, **held to the limit an image `read` is held to**
     (`tools::READ_IMAGE_MAX_BYTES`, the 5 MB of base64 the strictest
     provider takes). A picture past it — a detailed 1536×1024 PNG can be —
     is sent as a copy shrunk until it fits
     (`images::fitted_attachment_data_url`), and the model is told so. The
     copy is kept beside the picture in `.attachments/`, and the encoder
     sends it in the picture's place on every later turn, a `/resume` after
     a restart included: the conversation re-sends every picture it holds,
     so a picture past a provider's limit would otherwise fail every request
     from then on. The picture itself is never touched. The copy is decoded
     straight from the file into its fitted size, and the picture's own
     encoding is never made just to be thrown away
     (`tests/image_fit_memory.rs`);
   - is withheld from a model known not to take images. Such a model gets
     the file and the note, and the round carries the attachment's note
     degraded to `[image omitted: …]` exactly as every later turn's replay
     degrades it (`ToolOutcome::image_withheld`, `backend::omitted_image_line`),
     so the next request starts as this one did;
   - is drawn inline under the cell (`docs/images.md`);
   - is re-attached on every later turn from the path on the fact line, so
     a follow-up edit can look at what it is editing.

### A GIF the model looks at

A GIF with a file never goes up to a model as itself, whether it was read,
pasted or referenced. OpenAI's vision takes only a GIF that does not move,
Gemini's takes no GIF at all, and the conversation re-sends every picture it
holds on every turn, so one refused GIF would fail every request after it.
(An MCP tool's image result has no file: it goes up as the server sent it, a
GIF included.) What goes up instead is a **still** (`images::gif`): one PNG
of the frames tiled in a regular grid of equal cells, left to right and top
to bottom, in the squarest grid that holds them (two frames side by side,
nine in a 3×3, sixteen in a 4×4).

- A GIF of one frame is that frame. One of more than 16 frames is shown by
  16 of them, evenly spaced from its first frame to its last.
- The still is drawn at most 1024 pixels on its long side, then redrawn
  smaller until its PNG fits the attachment limit
  (`tools::READ_IMAGE_MAX_BYTES`). 1536 was tried first: decoding nine
  1024×1024 frames into a sheet that size left 8.5 MB resident beyond the
  encodings the session keeps, against 2.2 MB at 1024, and a 1024-pixel still
  costs the model fewer tokens on every turn that re-sends it.
- The frames are decoded one at a time, each shrunk into its cell before the
  next is read: a still costs one decoded frame and the sheet, whatever the
  GIF's length, and what stays resident is the encoding the session keeps
  (`tests/gif_still_memory.rs`).
- It is deterministic, and drawn by one function every path shares
  (`images::gif_still`, behind `images::attachment_data_url`): the turn that
  read or pasted the GIF, every later turn and a `/resume` after a restart
  all send the same bytes, which the prompt cache depends on
  (`docs/prompt-caching.md`). A `read` draws it itself and keeps it as the
  session's encoding, the way a `read` keeps a picture it shrank. No copy is
  kept on disk.
- The model is told what it is looking at: a `read` of a GIF that moves
  says it on the fact line, with the size of the picture sent:
  `Read image (GIF, 512x512, 9 frames, 1.2 MB; sent as its 9 frames tiled
  left to right and top to bottom, 3 to a row, in one 1023x1023 picture)`.
  A `read` of a GIF of one frame reads like any picture, and like one sent
  resized when its frame was drawn smaller
  (`… 1100x600, …; sent resized to 1024x558, …`).
- Passed to `imagegen` in `referenced_image_paths`, a GIF goes up as its
  **first frame** (`images::gif_first_frame`, drawn and capped the same
  way): the picture being edited, not a grid of it. That encoding is made for
  the one request and never kept.
- A GIF whose frames cannot be drawn sends nothing in its place, never the
  GIF: a `read` of it fails, saying to convert it to a PNG first, and a later
  turn carries `[image unavailable: …]`.

### Errors the model reads

| Situation | The result |
| --- | --- |
| Not signed in (the tool was offered by a stale build) | `not signed in — run /login and choose Sign in with an account` |
| Free plan | image generation needs a paid ChatGPT plan; this account is on the Free plan |
| 401 | `chatgpt::auth_advice`'s sign-in wording, then `ChatGPT said: …` when the endpoint said something |
| 403 | the plan wording `auth_advice` gives, then the endpoint's own words, which say whether it was the plan |
| 429 `usage_limit_reached` | the image limit is reached, with when it resets when the body says |
| other status | `image generation failed: HTTP {status}: {message}` |
| a count of the newest images past what the conversation holds | `requested the last {count} conversation images, but only {available} were available` |
| a count beside paths | ``provide only one of `referenced_image_paths` or `num_last_images_to_include` `` |
| `gpt-image-2.6-sunburst` refused by name, and `gpt-image-2` failed too | the older model's failure as above, then on a second line `gpt-image-2 was asked after the image backend refused gpt-image-2.6-sunburst: HTTP {status}: {message}` |
| no image in the answer | `image generation returned no image data` |
| Esc | `Interrupted by user` |
| the worker died without an answer | `image generation failed: the request stopped unexpectedly` |

## Where the pieces live

- **`src/llm/imagegen/`** (`mod.rs`, and `scan.rs` for the answer), the
  tool, its pure core first:
  - the spec and argument parsing;
  - the typed request bodies (a `BodySource` borrowing the shared encodings);
  - the base64 push decoder and the response scanner;
  - the fact line (format and parse) and the model's hint;
  - the error wording.

  The boundary half, behind them, is `run_imagegen`: the worker thread, the
  HTTP request and the file. It lives in the library because
  `auth::request_auth` is crate-private.
- **`llm::exec`**: `RealToolExecutor::with_imagegen` and a dispatch arm, so
  the lead and a subagent run the call through one path.
- **`llm::backend`**: `with_imagegen(setup)` attaches the spec through
  `sync_tool_specs`, beside `skill`. A subagent gets it too, through its
  type's `tools:` allowlist (`ImageGen`). A type whose allowlist leaves it
  out is not briefed on the `imagegen` skill either, since the tool and the
  listing travel together (`docs/subagents.md`).
- **The replay and the picture.** Three places used to recognise only
  `read`, and each now knows a generated image by its fact line
  (`imagegen::generated_image`):
  - the live attachment note in `run_agent`, `tools::attachment_note`;
  - the attachment on later turns in `context::record_calls`,
    `tools::recorded_attachment`;
  - the inline picture in `ui::image::tool_image`.

  The live note and the replayed note are built by one rule from what the
  call recorded, so they are byte-identical, which the prompt cache depends
  on (`docs/prompt-caching.md`). A `read`'s note is unchanged.
- **`images::fitted_attachment_data_url`**: the attachment held to a cap,
  and the `.attachments/` copy the encoder prefers (`images::attachment`).
- **`images::gif`**: a GIF as the still of its frames — the block walker
  (`gif_info`), the layout (`sheet_grid`, `sheet_frames`, `sheet_cell`), the
  words for it (`tiled_frames`), `gif_still` and `gif_first_frame`. The
  attachment encoder sends the sheet in every GIF's place, and `llm::exec`'s
  `read` of a GIF draws it the same way; an edit's GIF reference goes up as
  its first frame, through `imagegen::reference_url` and
  `images::gif_first_frame_data_url`.
- **`settings::SettingKey::ImageGen`**: the **Image generation** row,
  `SessionSettings::imagegen_offered` its gate with **Tools**, and
  `SettingKey::unavailable_reason` the toast's way to make it available.
- **The boundary** (`src/tui/`):
  - **`ModelSession`** keeps:
    - the `ImagegenSetup`: the shared sign-in handle, refreshed at startup,
      after a `/login` save and after a provider reload;
    - the output folder;
    - `imagegen_enabled`, the row's value;
    - `imagegen_attached`, the last build's verdict.

    `refresh_imagegen` rebuilds only when the verdict flips, which is the
    `skills_attached` pattern.
  - **The skills walk** withholds the seeded built-in while the tool is not
    offered.

## The model

Every request names `gpt-image-2.6-sunburst`, Images 2.6's model for
detailed work, the one that keeps an edit consistent with what it edits
(`imagegen::IMAGE_MODEL`).

A backend that does not take it yet still makes the image. When the answer
refuses the model by name — a 400, 404 or 422 whose error gives `model` as
its `param`, carries a code such as `model_not_found`, or names the model in
its message (`refuses_model`) — the same request goes once more with
`gpt-image-2` (`FALLBACK_IMAGE_MODEL`, `with_model_fallback`). Such a
refusal comes back at once, so nothing was made or billed. The model's note
says which model made the image (`Made::note`). The cell says it on one dim
row
under the fact line, `⎿ Made with gpt-image-2 (Images 2.6 not available)`
(`Made::cell_note`): the outcome carries it as `ToolOutcome::note`, which the
agent loop raises as the call's provenance note (`StreamEvent::ToolNote`,
the channel an auto mode approval's `Allowed by auto mode classifier` row
uses), so the row is in the Ctrl+O transcript, the rollout and a `/resume`
too. An image made as asked has no row. Any other refusal stands, since
another model would be refused the same way.

Two things keep the second request honest. An Esc that lands while the
refusal comes back sends nothing more: the older model's request would make,
and bill, an image nobody waits for. And when the older model fails too, the
call fails with both answers (`ImagegenError::FellBack`): what stopped the
image first, then why the older model was asked.

`ALTER_ZERO_IMAGEGEN_MODEL` names another model for the run, never saved:
`gpt-image-2.6-flare`, Images 2.6's fast model, or an older one. A model
named there is the one asked for, so its refusal is the answer and nothing
falls back. It is read once, at startup, into `ImagegenSetup::model`.

### Tried live

On a ChatGPT account's sign-in in November 2026, headless turns on
`gpt-5.6-luna` called the tool against the real backend:

- `gpt-image-2.6-sunburst` was taken, so nothing fell back and the cell had
  no row.
- GIF output, asked for with `output_format: "gif"` and its frame controls,
  was refused every time with `HTTP 400: GIF output is not enabled for this
  request.` (the raw answer `{"detail": "GIF output is not enabled for this
  request."}`): on a ChatGPT Pro plan, at quality `low` and `auto`, on
  `gpt-image-2.6-flare`, and with the reference client's own request byte
  for byte (`model: "gpt-image-2"`, `quality: "auto"`, `size: "auto"`, two
  frames of 200 ms). Neither the model nor the quality is the gate, and
  nothing about the request is: the reference keeps GIF output behind a
  feature that is off by default and still under development, and OpenAI
  enables the GIF preview per account. So the tool does not offer it. Its
  schema has no `gif`, a call that passes one anyway is refused before
  anything is sent, and the skill teaches none.

## When it is offered

It is offered while **all** of these hold:
- **Tools** is on;
- the backend is a real one (the offline dummy is never given it);
- the `/settings` **Image generation** row is on — the default, kept per
  directory like **MCP**, and seeded for a run by `ALTER_ZERO_IMAGEGEN`;
- the **ChatGPT Codex sign-in is stored**: the `chatgpt_codex` provider's own
  token variable, `CHATGPT_CODEX_REFRESH_TOKEN` (or its legacy name),
  resolves from the environment or the `.env` key store
  (`ProvidersFile::signed_in_with`).

Only the provider's own variable counts, never the generic
`ALTER_ZERO_API_KEY` fallback. A key meant for another provider must not
read as a ChatGPT sign-in.

Without a sign-in the row reads `false (unavailable)`, and Enter on it says
what would make it available: a ChatGPT sign-in, through `/login`.

Signing in mid-session (`/login` → ChatGPT Codex) turns it on at once: the
save rescans the skills, which checks the tool set against the new token, so
`/settings` and `/skills` show both before the next turn — no restart and no
`/model` switch. Every turn start checks again. There is no sign-out command,
so the gate only ever flips from off to on through a sign-in.

## The skill

`prompts/skills/imagegen/` is Codex's
`codex-rs/skills/src/assets/samples/imagegen/`. It is Apache-2.0, like this
repository, and its `LICENSE.txt` is seeded beside it. It is adapted to this
app:

- `image_gen` becomes `imagegen`. Edits use `referenced_image_paths`, where
  Codex's stale text says to load the image with `view_image` first.
- `$CODEX_HOME/generated_images` becomes the folder the tool result names.
- The fallback CLI is run as `python3 <base directory>/scripts/image_gen.py`.
  The path is built from the `Base directory` header, as `web-search` does;
  the scripts are seeded without the executable bit (`docs/skills.md`).
  `image_gen.py` and `remove_chroma_key.py` are Codex's own, unmodified.
- `references/codex-network.md` is not ported. It explains Codex's sandbox
  and approval config. Here the CLI's network call is an ordinary `bash`
  command under the permission mode, and one sentence in `SKILL.md` says so.
- `agents/openai.yaml` and the icons are Codex's UI metadata and are not
  ported. The built-in seed carries text files only.
- *Built-in edit semantics* in `SKILL.md` teaches which way to name the
  images: a path, or
  `num_last_images_to_include` only for a picture with no file, the smallest
  count that holds every target, never both. *When not to use* sends an
  animation elsewhere: the tool makes still images, and a GIF reference goes
  up as its first frame.

**Gating.** The skill is always seeded, so the built-in sync never retires it
while signed out and re-seeds it on sign-in (`docs/builtin-updates.md`).
Discovery **withholds** the seeded folder, `{config_home}/skills/imagegen`,
while the tool is not offered (`llm::skill::load_skills`):

- Every surface reads the one registry, so the listing, `/skills`, the `$`
  picker, the `/settings` availability, subagent briefings and the Skill
  tool's `find` agree without a second check.
- The gate recognises ours by **where** it is, not by name. A project's
  `imagegen` skill, a `~/.claude/skills/imagegen` or a plugin's is never
  withheld.
- An `ALTER_ZERO_SKILLS_DIR` override seeds no built-ins at all, so there is
  nothing to withhold.

## Off switches

- `/settings` **Image generation** off withdraws the tool and the skill, for
  this directory from then on.
- `ALTER_ZERO_IMAGEGEN=0` starts a run with that row off, never saved — the
  `ALTER_ZERO_MCP` posture, so `/settings` can turn it back on.
- `/skills` turns the skill off per project.
- `/settings` **Tools** off withdraws every tool.

## Testing

- **Pure core:** `cargo test --lib llm::imagegen`. This covers:
  - the spec, the arguments (`num_last_images_to_include`'s bounds and its
    rule against paths, and `size`, `quality` and `gif` refused as unknown),
    the bodies, the decoder, the scanner over split and escaped input, the
    fact line and the error wording;
  - the newest images: a request's image parts in order whoever sent them,
    the newest `n` oldest first, a repeat counted twice, a count past them
    refused before anything runs, and `cargo test --lib llm::exec` for the
    executor handing its record to the call;
  - the model fallback: which refusals name the model, one request more
    with `gpt-image-2`, nothing more after an Esc, both answers when the
    older model fails too, a model the run named never replaced, and the
    note saying what made the image;
  - a GIF reference: its first frame;
  - the cell's row: the outcome's note set only when the older model made
    the image, and `cargo test --lib llm::agent` for the loop raising an
    outcome's note after an approval's; `cargo test --lib ui::tests::image`
    for the row closing the inline and expanded cell;
  - the HTTP half against a loopback stub (the `llm::usage` pattern): the
    URL, the headers, the body, the saved file, a 429 and a cancel.
- **The replay and the picture:** `cargo test --lib context:: ui::`.
- **The gate and the skill:** `cargo test --lib llm::skill llm::config`, and
  the row: `cargo test --lib settings`.
- **The attachment:** `cargo test --lib images:: llm::agent` — the fitted copy
  sent on a restart too, a stale copy never sent, and a vision-less model's
  live note equal to the replay's.
- **A GIF's still:** `cargo test --lib images::` — the block walker (over
  another encoder's blocks too: an application block, a graphic control
  block per frame, a frame with its own colours, a file cut short), the
  grid, the cells in order, a long GIF's first and last frames, the first
  frame alone, the cap, and a GIF's attachment the same PNG on every turn;
  `cargo test --lib llm::exec` for a `read` of a GIF and the size it names.
- **Another provider's model draws:** `cargo test --test headless` runs a
  headless session on a stand-in Chat Completions provider with a ChatGPT
  sign-in beside it: the tool offered, the mint, the body and headers, the
  saved file and the attachment on the next request; the picture just drawn
  edited by count, the edit carrying the copy the model saw, and a
  foreground subagent's the same way, counted in its own conversation; a
  backend that refuses
  Images 2.6 by name drawn on with `gpt-image-2`, its stream item carrying
  the cell's row as its `note`; the model the run names;
  and signed out, or with the row off, nothing offered and nothing asked of
  ChatGPT.
- **Memory:** `cargo test --test image_generation_memory` streams a
  multi-megabyte answer and holds the growth under 2 MB;
  `cargo test --test image_fit_memory` fits three pictures past the cap and
  holds the growth to the copies' encodings plus 4 MB;
  `cargo test --test gif_still_memory` draws the stills of three GIFs of nine
  1024×1024 frames and holds the growth to their encodings plus 4 MB.
- **On screen:** `smoke.sh` Phase 168 drives a turn end to end against a
  stand-in for the ChatGPT backend, with no network:
  - a ChatGPT Codex session asks for an image;
  - the stub's Responses stream calls `imagegen`;
  - the stub's Images endpoint answers with a PNG;
  - the cell, the file and the next request's attachment are checked;
  - the **Image generation** row reads `true` signed in, and signed out
    `false (unavailable)`, whose Enter says how to sign in.

  Phase 165 counts the built-ins `/skills` lists, `imagegen` excepted, since
  the suite stores no sign-in.

# Image generation (`imagegen`)

Signed in with **ChatGPT Codex**, a session can make and edit raster images
through OpenAI's newest image model, GPT-Image-2.6 Sunburst: photos,
illustrations, sprites, textures, mockups, cutouts with a transparent
background, and short looping GIFs. It works whatever provider the
conversation runs on. A Claude, OpenRouter or Ollama model calls
the same `imagegen` tool, and the image is made with the ChatGPT sign-in,
billed to that ChatGPT plan. No API key is involved.

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

Alter Zero's call has the same shape, with the differences in this table.
It also names its own model, takes a size, a quality and a GIF's frames,
and saves an animation as one (*The model*, *The tool* and *A GIF* below).

| | Codex | Alter Zero |
| --- | --- | --- |
| Tool | `image_gen.imagegen`, a Responses namespace | `imagegen`, a plain function on every wire |
| When offered | ChatGPT auth **and** a provider that requires OpenAI auth, an image-capable model, not the Free plan | the ChatGPT Codex sign-in is stored, **whatever provider the session runs on**; a Free-plan account is refused when the call runs |
| Edit inputs | `referenced_image_paths` or `num_last_images_to_include` | `referenced_image_paths` only: every picture in a conversation here has a file (a paste is saved, a `read` names its file, a generated image is saved) |
| Saved to | `$CODEX_HOME/generated_images/<thread>/<call>.png` | `~/.alter-zero/generated_images/<session>/<N>.png` |
| What the model sees | the picture | the picture, or a copy held to the attachment cap when the picture is past it; a GIF as its frames tiled on one PNG (*A GIF*) |
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
     "size": {"type": "string"},
     "quality": {"type": "string", "enum": ["auto", "low", "medium", "high"]},
     "gif": {"type": "object",
             "properties": {
               "frame_count": {"type": "integer", "minimum": 2, "maximum": 16},
               "frame_duration_ms": {"type": "integer", "minimum": 20, "maximum": 2000}},
             "additionalProperties": false}},
   "required": ["prompt"], "additionalProperties": false}}
```

The description is short on purpose: every word rides every request that
offers the tool (CLAUDE.md's tool-prose rule), and the craft — when to make
a bitmap, how to word a prompt, where a picture belongs, how to animate — is
the skill's, which is listed whenever the tool is offered. What stays is the
capability and its sharp edges: one image per call, a minute or two, saved
and shown with its path returned, no asking for confirmation, edits by
`referenced_image_paths` after looking with `read`, `transparent_background`
for a cutout, and `gif` for a short loop, a GIF referenced with `gif` set
being remade from its own frames. There is no code mode and no
`num_last_images_to_include`.

`size` is `auto`, which lets the backend choose from the prompt, or
`WIDTHxHEIGHT`. It is checked against the backend's grid before anything is
sent:

- both edges are multiples of 16;
- the longer is at most 3840, and at most three times the shorter;
- there are no more pixels than 3840x2160 has.

A size off the grid is refused with the nearest one on it, when that one
passes the rest (`1920x1080` → `try 1920x1088`). The model learns in a
second what the backend would have answered after a round trip. A rule that
differs by model, such as the smallest size, is left to the backend, whose
refusal the model reads like any other.

`quality` is `auto`, `low`, `medium` or `high`, the four every GPT Image
model takes. The `xhigh` and `max` the Images API lists are documented for
its 2.5 models only, neither Images 2.6 model nor the model before (*The
model*) among them, so they are not offered.

Both are sent on every request, `auto` when not asked. There is no count:
one call makes one image, and many assets are many calls, which the skill
says.

An unknown argument is refused, with the list of accepted ones, and so is a
field `gif` does not take. A model that passes `n` or `gif.loop` learns at
once that it does nothing, instead of believing it got four images or a GIF
that plays once.

### The model

Every request names `gpt-image-2.6-sunburst` (`IMAGE_MODEL`). Images 2.6 is
two models: Sunburst, for finished work and consistency across edits, and
Flare, for speed. Sunburst is the one named, and it is also the Images API's
own default for an edit. The tool takes no model; `quality: "low"` is its
speed control.

`ALTER_ZERO_IMAGEGEN_MODEL` names another model for the run, such as
`gpt-image-2.6-flare`. It is read once, at startup, into
`ImagegenSetup::model` (`image_model`: a blank value names none), and never
saved.

A backend that does not take the default model yet must not break the tool.
So when the backend refuses `IMAGE_MODEL` itself, the same request goes once
more with `gpt-image-2` (`FALLBACK_IMAGE_MODEL`, `with_model_fallback`). A
model the run named is never swapped: its refusal is what the model reads,
since the user asked for that model and not for an image from another. A
refusal is of the model (`refuses_model`) when it is a 400, 404 or 422 and:

- its error's `param` is `model`. A `param` naming another field means the
  refusal is about that field, even when its message names the model;
- or, with no `param`, its `code` is `model_not_found`, `invalid_model` or
  `unsupported_model`;
- or, with neither, its message names the model.

A bad request is answered at once, so nothing was made or billed, and asking
again is safe. Any other refusal stands: another model would be refused the
same way. When the model before made the image, the note the model reads
says so (`This image was made with gpt-image-2: the image backend did not
take gpt-image-2.6-sunburst.`), so a user who asks which model was used gets
a true answer. Nothing is remembered between calls: a refusal costs a
second against a generation's minute, and the next call tries the newest
again.

### A GIF

`gif` makes a looping GIF instead of a still. `{}` takes the backend's
defaults (9 frames, 200 ms each); `frame_count` takes 2 to 16 and
`frame_duration_ms` 20 to 2000. A bare `true` reads as `{}` and `false` as
no GIF, since a model may send either. On the wire it is the Images API's
own shape: the format, with its numbers beside it as siblings, each left out
when not asked:

```
{"prompt": "…", "background": "opaque", "model": "gpt-image-2.6-sunburst",
 "quality": "auto", "size": "auto", "output_format": "gif", "frame_count": 4,
 "frame_duration_ms": 120}
```

- `transparent_background` is sent as asked. The Images API documents
  transparency for PNG and WebP only, so the skill says to leave it off for
  a GIF, and what the backend makes of it is the backend's.
- An account without GIF output is refused by the backend, and the refusal
  is worded like any other (*Errors the model reads*).
- The answer is saved as `N.gif`: its format is read from its content, as
  every answer's is, and its frames are counted off its blocks without a
  pixel decoded (`images::gif_info`). The fact line names them:
  `Generated image (GIF, 512x512, 9 frames, 1.1 MB) saved to …/1.gif`. A
  GIF of one frame names none.
- The model is shown the GIF's frames tiled on one PNG, never the GIF
  itself: OpenAI's vision takes only a GIF that does not move, and Gemini's
  none (`docs/images.md` *A GIF goes up as its frames*). Its note says what
  it is looking at and how to change it: `The picture attached for you is
  its 9 frames tiled left to right and top to bottom, 3 to a row; the file
  itself is the animated GIF. To make a new GIF from this one, list it in
  `referenced_image_paths` and set `gif`: it goes up the same way.`
- The cell draws the first frame. While the call runs, its corner row says
  `Generating GIF… (12s)` (`asks_for_gif`, read off the call's arguments).
- An animation is changed by referencing it with `gif` set. It goes up as
  that same tiled still, which is the spritesheet an image model draws a new
  GIF from: equal cells in a regular grid, in order, so the model has no
  tiling to do by hand. A spritesheet of its own, laid out with `bash`,
  works the same way.

### The call

1. **Arguments.**
   - The prompt must not be blank.
   - At most 5 referenced images.
   - `size`, `quality` and `gif` within the rules *The tool* and *A GIF*
     give.
   - Each reference resolves against the working directory, the file tools'
     rule. It must be a PNG, JPEG, WebP or GIF, judged by its content, and
     under the Images API's 50 MB limit. A GIF goes up as its frames tiled
     (*A GIF*).
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

   The body names the model (*The model*), the size and the quality, `auto`
   when not asked, and a GIF's format and frames (*A GIF*).

   Each reference image goes up as a `data:` URL taken from the session's one
   shared encoding (`images::attachment_data_url`, `docs/memory.md`). A
   picture already in the conversation costs no second copy, and
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
   an answer reads as a failure, never as the user's Esc. A refusal of the
   model itself is sent once more with the model before it (*The model*).
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
     folder's exclusive-create numbering, `clipboard`). A cancelled or
     refused body leaves nothing behind.
6. **Result.** The call's output is one fact line, an animation's frames
   after its size:

   ```
   Generated image (PNG, 1536x1024, 2.1 MB) saved to /home/me/.alter-zero/generated_images/1763812345/1.png
   Generated image (GIF, 480x270, 9 frames, 1.1 MB) saved to /home/me/.alter-zero/generated_images/1763812345/2.gif
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

### Errors the model reads

| Situation | The result |
| --- | --- |
| Not signed in (the tool was offered by a stale build) | `not signed in — run /login and choose Sign in with an account` |
| Free plan | image generation needs a paid ChatGPT plan; this account is on the Free plan |
| 401 | `chatgpt::auth_advice`'s sign-in wording, then `ChatGPT said: …` when the endpoint said something |
| 403 | the plan wording `auth_advice` gives, then the endpoint's own words, which say whether it was the plan |
| 429 `usage_limit_reached` | the image limit is reached, with when it resets when the body says |
| a `size` off the grid, past 3840 or 3:1, or past 3840x2160's pixels | the rule it breaks, and the nearest size on the grid when one passes |
| a `quality` not one of the four | the four |
| a `gif` number out of range, or a field `gif` does not take | the field, and its range or the fields it takes |
| 400, 404 or 422 refusing the model itself | sent once more with `gpt-image-2`; the note names the model that made it |
| other status | `image generation failed: HTTP {status}: {message}` |
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
- It teaches the tool's `size`, `quality` and `gif`, and how to change an
  animation: reference it with `gif` set (*Built-in size, quality and GIF*,
  *GIF requests*).

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
  - the spec, the arguments (`size`, `quality` and `gif` among them), the
    bodies, the decoder, the scanner over split and escaped input, the fact
    line and the error wording;
  - the model before: which refusals are of the model, the second request
    a stand-in that refuses the newest one answers, and a model the run
    named never swapped;
  - the HTTP half against a loopback stub (the `llm::usage` pattern): the
    URL, the headers, the body, the saved file, a 429 and a cancel.
- **The replay and the picture:** `cargo test --lib context:: ui::`.
- **The gate and the skill:** `cargo test --lib llm::skill llm::config`, and
  the row: `cargo test --lib settings`.
- **The attachment:** `cargo test --lib images:: llm::agent` — the fitted copy
  sent on a restart too, a stale copy never sent, and a vision-less model's
  live note equal to the replay's.
- **Another provider's model draws:** `cargo test --test headless draw
  imagegen` runs a headless session on a stand-in Chat Completions provider
  with a ChatGPT sign-in beside it: the tool offered, the mint, Codex's body
  and headers, the saved file and the attachment on the next request; and
  signed out, or with the row off, nothing offered and nothing asked of
  ChatGPT.
- **A GIF and the models:** `cargo test --test headless animate refuses
  the_run_can_name`. The stand-in model asks for a two-frame GIF: the body,
  the `.gif` saved, its frames on the fact line, and the PNG of its frames
  tiled that the next request carries. A backend that refuses the newest
  model makes the image on a second request, with `gpt-image-2`. And
  `ALTER_ZERO_IMAGEGEN_MODEL` names the model every request of the run asks
  for.
- **Memory:** `cargo test --test image_generation_memory` streams a
  multi-megabyte answer and holds the growth under 2 MB;
  `cargo test --test image_fit_memory` fits three pictures past the cap and
  holds the growth to the copies' encodings plus 4 MB; and
  `cargo test --test gif_still_memory` tiles three GIFs of nine 1024-pixel
  frames and holds the growth to the stills' encodings plus 4 MB.
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

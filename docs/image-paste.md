# Ctrl+V image paste


> The pasted screenshot is also **drawn** in the conversation, under its own
> user bubble — see `docs/images.md`, which additionally covers the
> `/settings` **Auto-resize images** row that downscales a large paste before
> it is uploaded.

Pressing **Ctrl+V** (or **Ctrl+Alt+V**) reads an image off the system clipboard
on a background worker, writes it to a temporary image file (a verbatim copy
when a pasted file is already in an accepted format, a PNG otherwise), and
drops a compact `[Image #N]` placeholder into the composer — the real file
path is remembered off to the side and handed to the backend, alongside the
message text, when the turn is sent. This is a focused port
of openai/codex's clipboard image paste (`tui/src/clipboard_paste.rs` +
`bottom_pane/chat_composer/attachment_state.rs`). It builds directly on the
large-text-paste placeholder machinery already in `docs/paste.md` — the image
placeholder is the *same* idea with a `PathBuf` payload instead of `String`.

> Note: this is **not** a bracketed-paste event. Ordinary text paste arrives as
> `Event::Paste` (see `docs/paste.md`); an *image* never does — terminals don't
> stream image bytes that way. Codex (and this port) instead bind an explicit
> `Ctrl+V` key that **actively reads the system clipboard** via `arboard`. The two
> paths are unrelated despite both being "paste".

## Trigger

`App::on_key` (conversation view only — the Ctrl+O overlay has no composer) maps
`Ctrl+V`, `Alt+V`, and `Ctrl+Alt+V` to a new `Action::PasteImage` (codex binds
the same set — Ctrl+Alt+V is the WSL-friendly alias where a bare Ctrl+V is eaten
by the terminal). It is a pure decision: `on_key` does **no** I/O, so the actual
clipboard read happens at the boundary in `main.rs`.

## Clipboard read (the I/O boundary)

`crate::clipboard::read_clipboard_image() -> Result<PathBuf, String>` is the
clipboard I/O boundary (like `term.rs`): a port of codex's
`paste_image_as_png` + `paste_image_to_temp_png`, **run on a worker thread**
(see *Async delivery* below):

1. `arboard::Clipboard::new()` — failure (no display / headless / no clipboard
   server) returns `Err("clipboard unavailable: …")`.
2. **Files first**: if the clipboard holds a file list (e.g. a file copied from
   a GUI file manager), `temp_image_from_files` takes the first usable entry —
   a file already in an accepted format (`png`/`jpg`/`jpeg`/`gif`/`webp`,
   header-validated with `image::image_dimensions`, which reads only the
   header) is **copied verbatim** to the temp file, extension preserved: no
   decode, no re-encode, effectively instant. Any other file that decodes
   (content-sniffed via `with_guessed_format`, so a mislabelled image still
   works) is transcoded to PNG. This half is filesystem-only, so unlike the
   clipboard half it **is** unit-tested headless.
3. **The owner's own PNG, streamed (Linux)**: a screenshot tool puts its
   picture on the clipboard *as a PNG* — the `image/png` X11 target, the
   `image/png` Wayland MIME — and that is the only shape arboard asks a Linux
   owner for before decoding it to RGBA. `clipboard::linux::stream_png_to_temp`
   asks for it directly and copies the bytes to the temp file as they arrive:
   a Wayland offer is a pipe (`wl-clipboard-rs`, the crate arboard's own
   Wayland backend is built on); an X11 selection is a `ConvertSelection`
   whose answer is fetched a 1 MiB property slice at a time, `INCR` segments
   as the owner sends them (GTK and Qt hand over anything larger than a few
   hundred kilobytes that way). No decode, no encode, about a megabyte in hand
   whatever the picture weighs. An owner that offers no PNG, a server the read
   can't reach, or a transfer that stalls all fall through to the next step,
   so the worst case is exactly what it was. `docs/memory.md` has the numbers:
   the round trip this replaces was a ~24 MB spike per paste and — through
   glibc's dynamic `mmap` threshold — the reason later pictures *stuck*.
4. **Raw image fallback**: otherwise `clipboard.get_image()` yields raw RGBA
   (a screenshot on macOS or Windows, where the OS hands over pixels);
   rebuild an `image::RgbaImage` and PNG-encode it **straight into the temp
   file** through a `BufWriter` — not into a growing in-memory vector copied
   to disk afterwards. `Err("no image on the clipboard")` when no path yields
   an image.
5. Either way the bytes land in a *kept* temp file (`tempfile::Builder` prefix
   `alter-zero-clipboard-`) whose path is returned — always **our own copy**,
   never the user's original (the discard cleanup deletes what this returns);
   the backend reads the file (the TUI never base64-encodes it — codex parity).

## Async delivery (why paste can't freeze the UI)

Codex calls `paste_image_to_temp_png()` synchronously in its key handler — fine
for a release binary, where a 1080p screenshot PNG-encodes in ~25 ms. Run inline
on our loop it would stall **everything** for the encode's duration — the comet
spinner, the shimmer, the timer, keystrokes — since one thread drives the whole
`select!`. So the `Action::PasteImage` arm only **spawns**
(`tui::workers::spawn_image_paste`): a short-lived thread does the clipboard read +
decode + encode and sends the `Result` back on the loop's **fifth `select!`
channel**, whose branch attaches the image (`App::attach_image`) or surfaces
the red notice — view-gated like every commit (under the Ctrl+O overlay the
notice is recorded only; invariant 4). The worker only sends — never a stdin
reader (invariant 1) — and is detached like the shell pipe readers: a straggler
at quit finishes writing a temp file harmlessly. A double Ctrl+V spawns two
workers and attaches two placeholders in completion order — exactly what
codex's synchronous handler produces, minus the freeze.

Two build-profile notes make the *encode itself* fast in dev runs (the shipped
release binary was always fast — that is why codex feels instant): dependencies
compile at `opt-level = 3` and our crate at `opt-level = 1` (the PNG encoder is
generic over the writer, so it monomorphises *into this crate* and would
otherwise run unoptimised). Measured on a 1920×1080 RGBA encode: ~1.6 s at the
old dev settings → ~60 ms now (~25 ms in release). On Linux the common case no
longer encodes at all — the owner's PNG is copied — so a paste costs what a
file copy costs, in time and in memory.

On `Ok(path)` the loop calls `App::attach_image(path)`; on `Err(msg)` it commits a
red `Failed to paste image: {msg}` notice (codex's `new_error_event`), using the
same mid-stream flush-segment ordering as a slash-command `Notice` so the notice
slots correctly if a reply is streaming.

**Temp-file lifecycle**: an attachment that is *dropped without being submitted*
— an atomic placeholder Backspace/Delete, a Ctrl+C-cleared draft, a `/clear`'d
queue — lands its path in `App::take_discarded_images`, which the loop drains
after each key event to `remove_file` the orphaned PNG (the pure core records
the drops; the file I/O stays at the boundary). *Submitted* images are left on
disk deliberately: the backend reads them by path — possibly again on a
history re-send — so they fall to the OS temp cleaner, codex-style.

`Cargo.toml` gains `arboard` (with `wayland-data-control`), `image`
(`jpeg,png,gif,webp`, default features off), and `tempfile` — plus, on Linux
only, `x11rb` and `wl-clipboard-rs` reached directly for the streamed
`image/png` read: the two backends arboard itself is built on, at the versions
it pins, so nothing new is compiled. The crates are pure-Rust on Linux, so
they build in CI with no system packages. **Out of scope for v1** (additive
later): codex's WSL PowerShell fallback and its Android `cfg` stub.

## The model (pure, unit-tested)

```rust
// App — parallel to `pasted: Vec<(String, String)>` (text pastes)
pub images: Vec<(String, PathBuf)>,   // (placeholder, temp-png path), insertion order
```

`App::attach_image(path)` (pure — tested by passing a fake `PathBuf`, exactly how
codex unit-tests it, no real clipboard needed):

1. `placeholder = paste::next_image_placeholder(&self.images)` → `[Image #N]`,
   where `N` is `max existing #k + 1` (or 1). Numbering is by **max existing**,
   not count, so deleting an image then attaching another never reuses a label —
   placeholders must stay unique because (unlike codex's element-ID model) we
   match them **by string** for deletion (`docs/paste.md` "intentionally not
   here"). Gaps in the numbers after a deletion are harmless.
2. `self.input.insert_str(&placeholder)` at the cursor, then push
   `(placeholder, path)` onto `self.images`.
3. Re-derive the `/`-palette / shell-mode / `@`-picker state (the same trio every
   composer edit runs), so an insert next to an `@token` behaves correctly.

### Placeholder string

`paste::next_image_placeholder` mirrors `next_paste_placeholder`: base form
`[Image #N]`. It scans the existing placeholders for the max `#k` and returns
`#(k+1)`, starting at `#1`. (`paste.rs` keeps both placeholder builders together.)

### Atomic deletion

A single Backspace/Delete removes the whole `[Image #N]` and drops its `images`
entry, identical to a text placeholder. `paste::placeholder_to_delete` and its
helper `longest_placeholder_at` are generalised from `&[(String, String)]` to
`&[(String, T)]` (they only ever read the placeholder string `.0`), so the same
cursor-rule logic serves both payloads. `App::delete_placeholder(backward)` tries
the text pastes first, then the images; the first hit wins.

### Send time

Unlike a text placeholder — which `take_input` **expands** back to its real text —
an `[Image #N]` placeholder **stays** in the message text (codex parity: the
sent/recorded text is literally `"[Image #1] describe this"`). The *paths* travel
a separate typed channel. At the `Action::Submit` boundary the loop drains
`App::take_submission_images() -> Vec<PathBuf>` (clearing `self.images`, like
codex's `take_recent_submission_images`) and threads it into `start_turn`.

Because the placeholder lives in the draft text, a draft holding only images is
never "empty" (the `[Image #N]` is non-whitespace), so Enter submits it; deleting
the placeholder drops the image too, so there is no "image with no text" state.

## Protocol (the extended backend seam)

`ReplySource::spawn` gains an `images: Vec<PathBuf>` parameter — codex's
`UserInput::LocalImage { path }` as a typed side channel, distinct from the text
`prompt`. This is the documented "swap in a real AI" seam: a real vision backend
reads each path and attaches it to its request. `start_turn` passes
`app.take_submission_images()` through; `count_input_images` (the image-count
sibling of `count_user_input`, which sizes the text) bumps the `↑` token tally
per attached image so the status reflects them.

`DummyAi` has no vision, so it **acknowledges** the images instead: `turn_events`
takes the image count and, when non-zero, prepends a short
`Looking at your N image(s). ` chunk to the reply — visible proof the channel
carried the attachments end to end. **The real `LlmBackend` has vision now**:
the recorded user message keeps its attachment paths, and each request embeds
them as base64 `data:` URLs in OpenAI's multimodal parts form — past turns'
images re-send with the conversation context, so follow-up questions about an
earlier image work. See `docs/context.md`. The mid-turn message **queue carries the
attachments with the batch**: `queue_draft` stages the `(placeholder, path)`
pairs into the `QueuedTurn::Messages` entry (an Enter merging into a batch
merges its images too, in attach order), the queue flush dispatches the paths
through the same typed channel as an idle submit, and an Alt+Up pull-back
re-attaches them to the composer so the placeholders in the restored draft are
backed again — a mid-turn Enter never silently drops an attachment.

**A known non-vision model degrades the paste instead of dying on it.** The
active model's image-input support is detected from its `/v1/models` record
(`ModelEntry::vision` — OpenRouter's `architecture.input_modalities`, Venice's
`supportsVision`; see `docs/tools.md` "Vision detection") and rides
`ModelConfig::vision` into every rebuilt backend. When it is `Some(false)`,
sending the parts array anyway would fail the whole request (OpenRouter 404s
"No endpoints found that support image input"), so `build_messages_for`
replaces each attachment with an `[image omitted: {path} — the current model
does not support image input]` text note — the model knows an image existed
and tells the user it can't see it — and the paste itself raises a red toast
(`{model} does not support image input`) the moment the attachment lands, so
the user knows before ever sending. Unknown support (`None` — a provider whose
records don't say) keeps today's optimistic attach.

## Rendering

The placeholder is **plain text** in the composer and in the committed user
message (the dark `❯ …` block), exactly like the text-paste placeholder — no
special styling, no inline terminal-image protocol (sixel/kitty). Codex likewise
renders `[Image #N]` as a text marker. The `?` shortcuts band gains a
`ctrl+v for image paste` entry.

## Tests

- `clipboard.rs`: the file half of the read — `accepted_image_extension` (the
  verbatim-copy allowlist) and `temp_image_from_files` (a real PNG copies
  byte-identical to a fresh temp path; a junk file with an image extension is
  rejected; an unlisted extension still decodes via content sniffing and
  transcodes to PNG) — filesystem-only, so testable headless; only the
  clipboard half stays boundary-untested.
- `paste.rs`: `next_image_placeholder` (base `#1`, `#k+1` disambiguation, gaps
  after deletion) and the generalised `placeholder_to_delete` over a
  `(String, PathBuf)` list.
- `app/composer.rs`: `attach_image` (inserts `[Image #N]`, records the pair, at the
  cursor), the round-trip (attach then Enter → `Action::Submit("[Image #1] …")`
  *and* `take_submission_images()` yields the path), atomic deletion (one
  Backspace clears the whole `[Image #N]` and drops the path), and Ctrl+V →
  `Action::PasteImage`.
- `stream/dummy/turns.rs`: every user-facing script `opening`s with the
  acknowledgement when the cue carries images
  and emits nothing at 0; `DummyAi::spawn` carries the new parameter.
- `scripts/smoke.sh`: a phase pressing Ctrl+V with **no image on the clipboard**
  (the headless CI reality — `arboard` errors) asserts the red
  `Failed to paste image` notice appears and the app stays alive. The happy
  path's composer half (a path → placeholder) is covered by the `attach_image`
  unit tests — codex tests it the same way.
- `tests/clipboard_linux.rs` (ignored by default — it needs an X server):
  the read itself, against a real one. `tests/support/x11_owner.rs` is a
  minimal selection owner that serves one `image/png` payload the way a
  screenshot tool does — whole in a single property, or in `INCR` segments —
  and the test requires the temp file to hold the served bytes **verbatim**
  on both shapes. The served PNG is encoded at a non-default compression
  level and carries a text chunk, so no decode-and-re-encode could reproduce
  it: byte equality is proof the bytes were streamed. A second test owns the
  clipboard with no picture and checks the failure message is the one the red
  notice always carried. Run it under a virtual server:

  ```
  Xvfb :99 & DISPLAY=:99 cargo test --test clipboard_linux -- --ignored
  ```

## Measuring

`docs/memory.md` has the before/after numbers; this is how they were taken,
so they can be taken again. Everything runs against `Xvfb`, which is what
lets a headless box drive the real clipboard path:

```
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 cargo run --example clipboard_owner -- 1920 1080 &   # serves a PNG, INCR
DISPLAY=:99 ALTER_ZERO_DUMMY=1 cargo run                          # Ctrl+V, then Enter
```

`clipboard_owner` is the same selection owner the integration test uses,
serving a screenshot-shaped picture (a gradient with per-pixel noise, so it
compresses like a real one) in 256 KB `INCR` segments — which is also how
it serves a 4K picture arboard's own owner cannot. Sample the process's
`VmRSS` and `VmHWM` from `/proc/<pid>/status` after each paste and each
send; the two `docs/memory.md` tables are exactly that, five pastes and
three paste-and-send rounds, under half-blocks and under kitty.

## What is intentionally *not* here (scope)

- **WSL PowerShell fallback** and the **Android** `cfg` stub.
- **Inline image display in the composer** — the marker is text there; the
  picture itself is drawn under the sent bubble (`docs/images.md`).
- The composer does not guard the cursor from stepping *into* `[Image #N]` (same
  limitation as the text placeholder — `docs/paste.md`); atomic Backspace/Delete
  covers the common case.
- **Placeholder numbering restarts per draft**, so two separately queued
  messages can each carry an `[Image #1]`; a batch merging both restores two
  pairs keyed by the same string on Alt+Up. An atomic Backspace over one
  occurrence drops only *its own* pair — occurrences in text order pair with
  list entries in order (`App::delete_placeholder`'s ordinal match) — so the
  other occurrence stays backed.
- A **Ctrl+C-cleared** draft drops its attachments (the recorded ↑-recall text
  keeps the now-unbacked placeholder as plain text — codex renders the marker
  as text in sent messages anyway); a **shell-mode** draft never carries images
  (`!` commands run locally; both the idle and the queued path drop them).

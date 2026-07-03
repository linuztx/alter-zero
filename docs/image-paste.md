# Ctrl+V image paste

Pressing **Ctrl+V** (or **Ctrl+Alt+V**) reads an image off the system clipboard,
writes it to a temporary PNG, and drops a compact `[Image #N]` placeholder into
the composer — the real file path is remembered off to the side and handed to the
backend, alongside the message text, when the turn is sent. This is a focused port
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
untested I/O boundary (like `term.rs`): a thin port of codex's
`paste_image_as_png` + `paste_image_to_temp_png`:

1. `arboard::Clipboard::new()` — failure (no display / headless / no clipboard
   server) returns `Err("clipboard unavailable: …")`.
2. **Files first**: if the clipboard holds a file list (e.g. a file copied from a
   GUI file manager), try `image::open` on each entry and take the first that
   decodes — this is the path for "an image *file* is on the clipboard".
3. **Raw image fallback**: otherwise `clipboard.get_image()` yields raw RGBA;
   rebuild an `image::RgbaImage` from it. `Err("no image on the clipboard")` when
   neither path yields an image.
4. **Re-encode to PNG** (`image::ImageFormat::Png`) — one consistent on-disk
   format regardless of source — and write it to a *kept* temp file
   (`tempfile::Builder` prefix `inline-tui-clipboard-`, suffix `.png`). The path
   is returned; the backend reads the file (the TUI never base64-encodes it —
   codex parity).

On `Ok(path)` the loop calls `App::attach_image(path)`; on `Err(msg)` it commits a
red `Failed to paste image: {msg}` notice (codex's `new_error_event`), using the
same mid-stream flush-segment ordering as a slash-command `Notice` so the notice
slots correctly if a reply is streaming.

`Cargo.toml` gains `arboard` (with `wayland-data-control`), `image`
(`jpeg,png,gif,webp`, default features off), and `tempfile`. The crates are
pure-Rust on Linux (`arboard` → `x11rb`/`wl-clipboard`), so they build in CI with
no system packages. **Out of scope for v1** (additive later): codex's WSL
PowerShell fallback and its Android `cfg` stub.

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
carried the attachments end to end. The mid-turn message **queue carries the
attachments with the batch**: `queue_draft` stages the `(placeholder, path)`
pairs into the `QueuedTurn::Messages` entry (an Enter merging into a batch
merges its images too, in attach order), the queue flush dispatches the paths
through the same typed channel as an idle submit, and an Alt+Up pull-back
re-attaches them to the composer so the placeholders in the restored draft are
backed again — a mid-turn Enter never silently drops an attachment.

## Rendering

The placeholder is **plain text** in the composer and in the committed user
message (the dark `❯ …` block), exactly like the text-paste placeholder — no
special styling, no inline terminal-image protocol (sixel/kitty). Codex likewise
renders `[Image #N]` as a text marker. The `?` shortcuts band gains a
`ctrl+v for image paste` entry.

## Tests

- `paste.rs`: `next_image_placeholder` (base `#1`, `#k+1` disambiguation, gaps
  after deletion) and the generalised `placeholder_to_delete` over a
  `(String, PathBuf)` list.
- `app.rs`: `attach_image` (inserts `[Image #N]`, records the pair, at the
  cursor), the round-trip (attach then Enter → `Action::Submit("[Image #1] …")`
  *and* `take_submission_images()` yields the path), atomic deletion (one
  Backspace clears the whole `[Image #N]` and drops the path), and Ctrl+V →
  `Action::PasteImage`.
- `stream.rs`: `turn_events` prepends the acknowledgement when `image_count > 0`
  and is unchanged at 0; `DummyAi::spawn` carries the new parameter.
- `scripts/smoke.sh`: a phase pressing Ctrl+V with **no image on the clipboard**
  (the headless CI reality — `arboard` errors) asserts the red
  `Failed to paste image` notice appears and the app stays alive. The happy path
  (a real clipboard image → placeholder) can only be exercised on a desktop with
  a clipboard server, so it is covered by the `attach_image` unit tests, not
  smoke — codex tests it the same way.

## What is intentionally *not* here (scope)

- **WSL PowerShell fallback** and the **Android** `cfg` stub.
- **Inline image display** in the terminal (sixel/kitty) — the marker is text.
- The composer does not guard the cursor from stepping *into* `[Image #N]` (same
  limitation as the text placeholder — `docs/paste.md`); atomic Backspace/Delete
  covers the common case.
- **Placeholder numbering restarts per draft**, so two separately queued
  messages can each carry an `[Image #1]`; a batch merging both restores two
  pairs keyed by the same string on Alt+Up, and an atomic Backspace over one
  occurrence then drops both pairs (the string-keyed scheme's known edge).
- A **Ctrl+C-cleared** draft drops its attachments (the recorded ↑-recall text
  keeps the now-unbacked placeholder as plain text — codex renders the marker
  as text in sent messages anyway); a **shell-mode** draft never carries images
  (`!` commands run locally; both the idle and the queued path drop them).

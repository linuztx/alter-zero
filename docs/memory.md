# Resident memory: what the app holds, and what `/model` used to cost

Alter Zero is a long-lived foreground process in the user's terminal, so its
resident set is a feature: it sits idle between turns and whatever it holds it
holds for the whole session. Startup is **~16 MB**. Opening `/model` once used
to take that to **25 MB** and leave it there — climbing past 26 MB on repeat
opens — because the picker's model list was parsed in a way that cost ten
times the body it read, and none of it came back.

## The measurement

Two probes, both `/proc/self/status` (the same method `docs/tokenizer.md`
used — `VmRSS` for what is resident now, `VmHWM` for the peak a transient
spike leaves behind):

- **`cargo run --release --example mem_probe [body.json]`** — what one
  `/v1/models` parse costs the process (a synthetic OpenRouter-shaped body is
  generated when no capture is given, so it runs from a fresh checkout).
  `--dom` re-measures the whole-body `serde_json::Value` tree this replaced,
  alone in a fresh process, for a like-for-like reading on the same body.
  With `OPENROUTER_API_KEY` set, Part B drives the real fetch.
- **`tests/model_parse_memory.rs`** — the regression gate, run by plain
  `cargo test`. It lives in its own test binary because the reading is
  process-wide: a lone `#[test]` has nothing running beside it to pollute the
  measurement, which a `#[cfg(test)]` module sharing the library's 2800-test
  binary could not promise. It skips (rather than asserting on noise) where
  `/proc` isn't available.

End-to-end numbers below come from driving the real binary under tmux with an
OpenRouter key and sampling the process's `VmRSS`.

## What was wrong

`llm::models::parse_models` deserialized the response into
`ModelsResponse { data: Vec<serde_json::Value> }` — a full tree of **every
field of every record** — and then read seven keys out of each: `id`, `name`,
`context_length`, `reasoning`, `supported_parameters`, `architecture` and
`model_spec`.

OpenRouter's live list measured 417 records, 669 KB of JSON. Weighed by field
(value bytes plus key/syntax overhead):

| field | bytes | read? |
|---|---|---|
| `benchmarks` | 153.7 KB | no |
| `description` | 93.0 KB | no |
| `supported_parameters` | 84.2 KB | yes |
| `architecture` | 67.9 KB | yes |
| `pricing` | 53.5 KB | no |
| `top_provider` | 39.5 KB | no |
| `default_parameters` | 35.4 KB | no |
| `links` | 31.4 KB | no |
| `reasoning` | 23.7 KB | yes |
| `canonical_slug` | 19.5 KB | no |
| `name` + `id` + `context_length` | ~36 KB | yes |

Roughly two-thirds of the bytes are never read — but the read fraction was
never really the point: a `Value` tree pays ~10x on read and unread fields
alike, because every string is its own heap `String`, every object an
`IndexMap` (the `preserve_order` feature), every number a boxed enum. The
whole-list tree measured **+6.5 MB** resident against that 669 KB body, where
the rows the picker keeps (`ModelEntry` — id, provider, name, an effort
ladder, two small options) total **~47 KB**.

The second half of the problem is that **dropping the tree gave nothing
back**. The probe reads the same RSS before and after the `drop`. The tree is
thousands of small allocations interleaved across the arena, so glibc's
`free` can coalesce almost none of it into a returnable span — the pages stay
mapped and the process stays big. That is why one `/model` open was a
permanent +8.7 MB rather than a spike, and why closing the picker (which does
drop the rows — `App::close_model_picker` sets `model_picker = None`) changed
nothing. Repeat opens crept further still, because each fetch runs on a fresh
worker thread that can land on a different glibc arena, each growing its own
pool.

## The fix: decode one record at a time

The envelope now holds each record as an **unparsed
`serde_json::value::RawValue`** — a borrowed slice of the response body, not
a heap copy:

```rust
struct ModelsResponse<'a> {
    #[serde(default, borrow)]
    data: Vec<&'a serde_json::value::RawValue>,
}
```

and `parse_models` decodes them one at a time through the per-record
`entry_of`, dropping each record's tree before the next. Peak is now **one
record's tree** (~2 KB) plus 417 fat pointers (~7 KB), and each record's
allocations are freed straight back into the same blocks the next record
reuses — so the arena never grows to hold the whole list at once.

The sniff functions (`reasoning_support_of`, `vision_support_of`,
`context_window_of`) still take a `&Value`: this is a change to how that
`Value` is produced, not to what is read out of it. Every element of an array
that parsed as JSON is itself valid JSON, so the per-record `from_str` cannot
fail on a body that got here, and the per-record skip (a malformed aggregator
row is dropped, never fatal) is byte-for-byte the old behaviour — locked by
`entry_of_reads_one_record_and_skips_an_unusable_one` and the lenient-field
tests beside it.

## What it bought

Parsing OpenRouter's real 669 KB list:

| | RSS growth | returned on drop |
|---|---|---|
| whole-body `Value` tree (`--dom`) | **+6.5 MB** | none |
| per-record decode (`parse_models`) | **+0.4 MB** | n/a — it never grew |

The ~0.4 MB is mostly the 417 `ModelEntry` rows the picker legitimately keeps
plus one round of parse churn.

End-to-end, the real binary against OpenRouter:

| | before | after |
|---|---|---|
| startup | 16.3 MB | 16.3 MB |
| after the first `/model` | 25.0 MB | **19.1 MB** |
| steady state (three opens) | 26.0 MB, still climbing | **19.6 MB, flat** |

**~6 MB saved**, and the saving scales with the fan-out: `/model` fetches
**every** configured provider in parallel (`ModelSession::begin_model_fetch`),
so a user with three keys was paying three concurrent trees where they now pay
three ~0.4 MB parses. The startup capability probe (`ModelSession::resolve`'s
`probe`) runs the same parse before the first frame paints for an env-selected
model, so it gets the same saving.

## What the remaining `/model` cost is, and why it stays

The ~+2.9 MB that the first `/model` still costs is not JSON:

| | cost | one-time? |
|---|---|---|
| first `reqwest::blocking::Client` (runtime thread, pool, TLS config) | ~1.4 MB | yes |
| first real HTTPS exchange (crypto provider init, handshake buffers) | ~1 MB | yes |
| the response body `String` | ~0.7 MB | transient; a single large allocation, so it *does* return |
| the kept `ModelEntry` rows | ~0.05 MB | held while the picker is open |

The client and TLS costs are paid by the first network call of *any* kind — a
real chat turn pays them too (measured: a chat-first session lands at the same
~20 MB steady state, and `/model` then adds only ~0.5 MB). They are not
`/model`'s to save.

Two smaller guards keep the fetch bounded beyond the parse:

- **One client, not two.** `llm::http_client` caches clients keyed by their
  per-operation timeout, and the models fetch used to pass its own 30 s
  deadline — building a second full client (its own blocking-runtime thread,
  pool and TLS config) beside the chat client, held for the life of the
  process. It now shares `openai::NET_OP_TIMEOUT`, so the whole session rides
  one client. Honestly sized: the *marginal* client measured well under
  ~200 KB (the trust anchors are static and the crypto-provider init is
  process-global — only the first client pays the megabyte), so this is one
  fewer permanent thread and one shared connection pool — `/model`'s fetch
  warms the very connection the next chat turn reuses — more than it is a
  memory win.
- **The body read is capped** (`MODELS_BODY_MAX_BYTES`, 8 MiB — the
  `SHELL_OUTPUT_MAX_BYTES` posture): a broken or hostile endpoint can't
  balloon the worker with an unbounded `read_to_string`; a body cut at the
  cap fails the parse as an ordinary decode error.

## Measured and deliberately not changed

**`ModelPicker::matches` allocates** a `Vec<&ModelEntry>` per call and
lowercases every id/provider/name per keystroke, from several call sites per
render. Typing into a 417-model picker moved RSS by ~0.05 MB once and then
not at all across further keystrokes: the allocations are uniform,
short-lived and immediately reused. It is CPU churn, not a memory problem,
and this document is about memory.

## Inline images pay the same rent

A kitty placement holds the whole picture as base64 RGBA — a 120-column
screenshot at a 10x20 cell is 1200x600 pixels, ~2.9 MB of pixels and ~3.8 MB
of base64 — so the encoded-picture store is bounded in **bytes**, not entries
(`docs/images.md`). The budget is charged from the placement's own geometry
rather than from the encoder's internals, and the least-recently-drawn picture
is evicted past 24 MB; meeting it again costs one re-encode, never a wrong
picture.

The model-facing downscale is bounded the other way round: decoding is
`4 x width x height` bytes resident, so it declines outright past 50
megapixels (or 64 MB of source), and the caller refuses the read instead of
materialising a 12000x12000 scan to discover it shouldn't have.

## Pasting a screenshot: the round trip that stuck

The report was "every Ctrl+V of an image grows the process, past 100 MB".
Reproduced under a virtual X server — `Xvfb :99`, the `clipboard_owner`
example serving a 1920x1080 screenshot-shaped PNG (a gradient with per-pixel
noise, so it compresses like a real one), the binary in tmux, `VmRSS` and
`VmHWM` sampled after each step:

| paste only, five times | RSS | peak |
|---|---|---|
| startup | 18.0 MB | 18.0 MB |
| paste 1 | 24.3 MB | 42.5 MB |
| paste 2 … 5 | 19.0 MB | 42.8 MB |

| paste + send, three times | after the paste | after the send |
|---|---|---|
| picture 1 | 24.1 MB | 41.2 MB |
| picture 2 | 59.9 MB | 78.9 MB |
| picture 3 | 103.1 MB | 103.3 MB |

(half-blocks; under kitty the same run reached **122.5 MB**.) A paste alone
was a 24 MB spike that mostly came back. *Sending* the picture is what stuck,
~20 MB a time, and it never came back.

### Three costs, and why they compounded

**The paste decoded and re-encoded bytes it had been handed.** On Linux a
screenshot tool puts its picture on the clipboard as a PNG, and `image/png`
is the one target arboard asks an owner for — after which it decodes it to
RGBA and hands the pixels over, and `read_clipboard_image` PNG-encoded them
back onto disk. For 1920x1080 that is an 8.3 MB RGBA buffer, an encoder's
working set and an output `Vec` doubling its way past 8 MiB, all to
reproduce the owner's bytes.

**The display decoded the picture whole to draw a small one.** Committing the
user bubble draws the picture under it (`docs/images.md`), and
`ImageStore::encode` decoded the whole PNG — `4 x width x height` again —
before shrinking it to the ~118-column block, which is ~3 MB.

**glibc kept the second one.** `malloc` serves a block above its `mmap`
threshold with a private mapping that `free` really does return — but the
threshold is *dynamic*: freeing such a block raises it to that block's size
(up to 32 MiB), so the *next* block of that size is carved from the heap,
where a free that isn't at the very top returns nothing to the OS. The
paste's 8 MiB output vector lifted the threshold past the RGBA size, and from
then on every whole-picture decode — the render's, and a re-send's — landed
in the heap and stayed. That is the +20 MB per send, and why it read as a
leak.

### What changed

- **The paste streams the PNG.** `clipboard::linux` asks the owner for
  `image/png` itself and copies the bytes into the paste folder as they arrive: a
  Wayland offer is a pipe, an X11 selection is fetched a 1 MiB property slice
  at a time, `INCR` segments included. No decode, no encode, never more than
  about a megabyte in hand — and a 4K screenshot, which arboard could not read
  at all on X11 (its own owner-side transfer exceeds the server's request
  limit), pastes in the same memory as a small one. Whatever the direct read
  can't settle falls through to arboard's path, and that path now encodes
  straight into the file rather than through a growing vector.
- **A PNG is decoded at the size it will be shown or sent.** `images::fitted`
  streams the file's rows through an area-average shrink and materialises
  only the target: the reserved block for the display (~3 MB at 120 columns),
  the 2000-pixel cap for the model. The peak no longer scales with the
  screenshot — a 4K one costs what a 1080p one does.
- **The `data:` URL is one allocation**, the base64 appended onto its prefix
  instead of encoded into a string and copied in behind one.
- **The payload is shrunk once per session, not once per turn.** An
  attachment is re-sent with the context on every later turn, and each turn
  used to decode and shrink it again — on a 4K screenshot, a 20 MB spike per
  turn for as long as the picture stayed in context. `images::payload` keeps
  the downscaled bytes on disk (`{session}/images/{key}`, keyed on the file's
  path, size, mtime and the cap; `docs/scratchpad.md`) and `cached_downscale`
  serves a later turn from that small file before the original is even
  opened. The `read` tool's pictures go through the same cache. The directory
  is bounded at `PAYLOAD_CACHE_MAX_BYTES` (64 MiB): before a new copy lands,
  `cache_eviction` names the oldest sidecars to drop so it fits. A copy
  larger than the whole cap empties the cache and is still written — the cap
  bounds what is *kept*, and refusing to cache a big picture would restore
  the per-turn decode this exists to remove.
- **Nothing else is decoded whole without asking first.** A non-PNG picture
  (a pasted JPEG photo, a `read` of one) decodes whole only under
  `WHOLE_DECODE_MAX_PIXELS` (50 megapixels) and shrinks with
  `thumbnail_exact` — a box filter with no `f32` working copy, where
  `resize` allocated 16 bytes a pixel over the source width. The streaming
  PNG decode is bounded in *time* by `FIT_MAX_SOURCE_PIXELS` (200 megapixels),
  since it never holds the rows it walks.

### What it bought

Same procedure, same pictures:

| paste only, five times | RSS | peak |
|---|---|---|
| startup | 18.0 MB | 18.0 MB |
| paste 1 … 5 | **19.1 MB** | **19.1 MB** |

| paste + send, three times | after the paste | after the send |
|---|---|---|
| picture 1 | 19.1 MB | 24.1 MB |
| picture 2 | 24.1 MB | 28.8 MB |
| picture 3 | 29.4 MB | **29.5 MB** |

A 3840x2160 screenshot: 18.8 MB after the paste, 28.8 MB after three sends —
the same numbers, because nothing left in the path is sized by the file.
kitty settles at 36.9 MB for the three, its per-screen placements being the
honest cost `docs/images.md` accounts for. What remains per send is the
fitted picture and the protocol built from it, a few megabytes the threshold
dance can still hold once per size class — bounded, and small.

Live, against a real vision model on Venice: a 1080p paste read 22.9 MB after
the paste and 25.3 MB after the answer, with a 54 MB peak in between that is
the request body itself (the file, its base64, the JSON) and comes back. A 4K
paste sent on **two** turns is where the payload cache shows: the first turn
peaked at 63 MB building the 2000-pixel payload; the second turn, which used
to decode and shrink the picture again and pushed the peak to 85 MB, now
reads the cached copy and moves the peak not at all.

The guards: `tests/clipboard_linux.rs` drives the read against a real X
server on both transfer shapes (a whole property, and `INCR` segments) and
requires the saved file to hold the owner's bytes *verbatim* — served at a
non-default compression level and carrying a text chunk, which no
decode-and-re-encode can reproduce — plus an owner with only a JPEG;
`images::tests` pins the fitted decoder to a naive area-average oracle and
its cell arithmetic to the encoder's own, and the payload cache to
write-once-read-back semantics; and `tests/image_paste_memory.rs` gates the
resident growth of the streamed copy, the fitted decode and the payload
shrink of a 2560x1440 screenshot the way `tests/model_parse_memory.rs` gates
the `/model` parse — the streamed copy under 2 MB, the other two under their
own output plus a few megabytes of slack, all an order of magnitude under the
source's 15 MB of RGBA. The procedure for the end-to-end numbers is
`scripts/paste_mem.sh` (`docs/image-paste.md`, *Measuring*).

## Every turn re-sent the picture

The report after the paste was fixed was the picture's *afterlife*: "every
time I send a new message the RAM increases", with a screenshot pasted once
and drawn in the conversation. Reproduced with `scripts/turn_mem.sh` — the
same harness as above, one paste, one send, then plain-text follow-ups that
each re-send the picture with the context — against a real vision model on
Venice, a 1920x1200 screenshot-shaped PNG of 6.3 MB:

| before, halfblocks | RSS | peak |
|---|---|---|
| startup | 22.0 MB | 22.0 MB |
| after the send | 25.7 MB | 58.1 MB |
| follow-up 1 | **39.3 MB** | 63.1 MB |
| follow-ups 2–6 | 39.3 MB | 63.2 MB |
| follow-up 7 | **53.2 MB** | 77.0 MB |
| follow-up 8 | 53.2 MB | 77.0 MB |

| before, kitty | RSS | peak |
|---|---|---|
| after the send | 29.1 MB | 60.5 MB |
| follow-up 1 | **42.7 MB** | 66.4 MB |
| follow-up 2 | **56.5 MB** | 80.2 MB |
| follow-ups 3–6 | 56.6 MB | 80.2 MB |

Not every message, but a step of **~14 MB** every few, and never back —
open-ended over a session. The step is the picture: 6 MB of file plus 8 MB
of base64.

### What a turn did with a picture

The conversation is re-sent whole on every request (`docs/context.md`), so
the attachment rides every later turn, and each of those turns — and each
agentic round within it — rebuilt it from scratch on a fresh backend thread:

1. `image_data_url` read the **file whole** (6 MB) and base64-encoded it
   into a new string (8 MB); the payload cache only helps a picture the
   auto-resize *shrank*, and this one already fit the 2000-pixel cap;
2. the messages were **cloned** for the round (`messages.to_vec()` into
   `stream_chat`) — a deep copy of that 8 MB string;
3. `build_payload` built a `serde_json::Value` **tree** of the whole request
   — the string copied once more into a `Value::String`;
4. `reqwest`'s `.json()` serialized the tree into a `Vec` grown **by
   doubling**, ending at 16 MB of capacity for an 8.4 MB body, every step of
   the growth a copy freed behind it;
5. the body went to the transport thread and was freed there.

Five picture-sized blocks a round, ~46 MB of transient traffic, and — the
part that made it *stay* — none of it mmapped. glibc's dynamic `mmap`
threshold had already been raised past 8 MB by an earlier free (the same
mechanism *Pasting a screenshot* describes), so every one of those blocks was
carved from a **thread arena's heap** instead: `strace` shows the turn's
thread growing its arena by exactly 6,172,672 and then 8,404,992 bytes
(`mprotect`), and nothing ever giving them back — a heap only shrinks from
its top, the trim threshold is twice the mmap threshold, and the small
long-lived allocations that follow pin what is below them. Each backend
thread takes an arena from glibc's free list; when the list happens to be
empty (a transport thread, a tool, a paste worker alive at the same moment),
the turn lands on a **new** arena and leaves the picture's worth in it. That
is the step, and why it came every few messages rather than every one.

`examples/image_turn_probe.rs` replays the request path with no network
(one turn per thread, the body handed to a second thread like the transport)
and reproduces the shape: +8 MB resident after the second turn, a 46 MB
peak, flat after. Run under `MALLOC_MMAP_THRESHOLD_=65536` — a fixed
threshold, no dynamic raise — the residue vanishes and the peak drops by
14 MB, which is the allocator confirming the diagnosis. The fix cannot be
that knob (it is an `mallopt` behind `unsafe`, and it would tax every small
allocation in the process); it is to stop asking for picture-sized blocks
every turn.

### What changed

- **An attachment is encoded once per session and shared.**
  `images::attachment` keeps each picture's `data:` URL under its path,
  validated against an [`AttachmentStamp`] — the file's size and mtime plus
  the **Auto-resize images** setting, everything that decides what goes up —
  and hands every request the same string by reference: `AttachmentUrl` is
  an `Arc`, so the round's copy of the messages and the marked copy an
  explicit-caching model takes are shallow. The one encoding is **streamed**
  into its string from the file or the payload sidecar (`base64_encode_reader`,
  a 48 KB buffer), so building it costs exactly what it keeps; the original
  is read whole only to shrink it, once. The `read` tool hands the payload it
  already has to the same cache (`remember_attachment`). Bounded at
  `ATTACHMENT_CACHE_MAX_BYTES` (32 MB, least recently sent first out) and
  swept at every turn start to the pictures the context still carries
  (`retain_attachments`; a `/clear` empties it) so a backtracked or compacted
  picture lets its megabytes go.
- **No tree of the messages, ever — on any wire.** The Chat request is
  `ChatRequest`, the messages **by reference** beside a small map of every
  other field, and the prompt-caching breakpoints mark a shallow typed copy
  (`cache::apply_cache_breakpoints` over `ChatMessage`s,
  `docs/prompt-caching.md`) instead of rewriting a JSON tree. The three
  translated wires — Anthropic's Messages API, OpenAI's Responses API,
  Ollama's `/api/chat` — build typed requests too, every block, item and
  message **borrowing** from the conversation, so an image block's base64 is
  a slice of the shared encoding rather than a `Value::String` copy of it
  (each module's `the_request_borrows_a_pictures_bytes_rather_than_copying_them`
  pins the pointer). `build_payload` — the tree — is now the tests' view of
  a request on every wire and nothing the wire builds.
- **The body is never held whole.** `llm::body::streamed_request` takes
  each wire's request as a `BodySource` — owned data that writes its typed,
  borrowed form — and serializes it on its own thread into a bounded pipe of
  64 KB chunks
  (`BODY_CHUNK_BYTES`, four deep) that the transport pumps as it uploads,
  the `Content-Length` known from a counting pass that allocates nothing.
  An intermediate version serialized into a buffer sized **exactly once**
  and still measured a body-sized step (+7.5 MB) every few turns: an 8.4 MB
  body a few bytes larger than the last one lands under the page-rounded
  threshold the last one's free set, and is carved from the arena like
  everything else. The only way not to keep a body's worth per arena is not
  to build the body.

### What it bought

Same harness, same picture, same model:

| after, halfblocks | RSS | peak |
|---|---|---|
| startup | 22.1 MB | 22.1 MB |
| after the send | 32.6 MB | 36.7 MB |
| follow-ups 1–8 | **32.7 → 33.2 MB** | 36.7 MB |

| after, kitty | RSS | peak |
|---|---|---|
| after the send | 36.9 MB | 45.2 MB |
| follow-ups 1–6 | **37.0 → 37.5 MB** | 45.2 MB |

The send now costs the one encoding the session keeps (8.4 MB of base64
for this picture — the honest price of re-sending it every turn without
rebuilding it) and the follow-ups cost the words in them; the peak is a
third of what it was and never moves again. The probe reads the same way:
the resident set flat to the kilobyte across seven turns and three rounds a
turn, and no turn raising the peak at all. `tests/image_turn_memory.rs` is
the gate — six follow-up turns after the one that carries a 1920x1200 paste
may grow neither the resident set nor the peak by more than 2 MB — and
`scripts/turn_mem.sh` is how the tables above are taken again.

[`AttachmentStamp`]: ../src/images/attachment.rs

## The streaming `write` cell kept every line's colours

The live `Write` cell (`docs/write-streaming.md`) colours the file as the model
writes it, and its cache kept the segments of **every** line it had
highlighted: a `String` and a style per token, about fifty times the line's
text. Counted with a global allocator over the real `App` and the live-region
renderer, the preview held this much while one write streamed:

| file | before | after |
| --- | --- | --- |
| 300 lines, 14.5 KB | 281 KB | 35 KB |
| 800 lines, 39 KB | 761 KB | 83 KB |
| 2,000 lines, 99 KB | 1,859 KB | 147 KB |
| 5,000 lines, 251 KB | 4,656 KB | 275 KB |

All of it was freed at the turn's end, so nothing leaked. But glibc keeps what
a process has once used, so the largest file a session previewed set its size
for good. This was measured end to end: release builds of the commit before the
preview (`317316a`), the preview as merged (`f77d92d`) and the fix, driving a
stand-in provider under tmux. The reading after each turn is the process's
**private** memory, its heap plus anonymous mappings from `/proc/PID/smaps`,
averaged over two runs. The rest of `VmRSS` is the binary's own file-backed
pages, which differ by up to a megabyte between a freshly built binary and an
older one, whatever the code does:

| after | no preview | every line kept | first and newest only |
| --- | --- | --- | --- |
| one 300-line write, 4th turn | 10.2 MB | 10.4 MB | 10.2 MB |
| one 2,000-line write, 1st turn | 9.0 MB | 11.3 MB | 9.3 MB |
| one 2,000-line write, 4th turn | 12.7 MB | 15.3 MB | 12.7 MB |
| three 800-line writes in a round, 3rd turn | 15.9 MB | 18.0 MB | 16.2 MB |

The growth per turn is the same on all three, because that growth is the
conversation's own history (each committed write's cell, arguments and
transcript rows), which predates the preview.

**The fix** keeps what the cell can show rather than what it has seen. Every
line is still highlighted once, in order, since the grammar's state has to pass
through the middle to colour the newest line. But the strip's cache,
`LineCache::windowed`, keeps the segments of only the first and newest ten
lines, which is all the cell's head and tail can show. The whole file is
coloured into a second cache only when a view that shows every line asks for
it: Ctrl+O or `/export`. That leaves the decoded file and about twenty lines.
`tests/write_preview_memory.rs` gates the resident growth of a streaming
5,000-line write at four copies of the file. It measures one, where the old
cache's resident cost was about twenty-seven.

## Code-generation transcripts and rewrite diffs

The retained Ctrl+O transcript was the largest avoidable growing display
allocation found in a task-manager generation replay. Every committed tool
result was pre-rendered into expanded ratatui rows: each syntax run held a
separate string and a full style, even while the overlay was closed. Keeping
the warm cache is necessary for fast opens (`docs/tool-view-performance.md`).

The cache now keeps one UTF-8 text slab, an interned style table and encoded
span lengths/indexes. The pager borrows text for only its visible rows. It
still highlights each committed item once, retains every recorded row,
preserves line alignment and image/link carriers, and restyles backtrack
selection without highlighting again. Full owned rows remain available for
explicit callers, including export; ordinary pager draws never build them.

Refreshing the live tail now decrements the style references held by removed
rows and spans, then releases unused trailing styles. This prevents unique
link/image carriers from accumulating on every refresh. Selection changes
balance those references too. Retained span IDs and buffer capacity policy
stay unchanged; unused middle slots remain until later styles are released.
A failing-first churn test left 3,998 stale styles after 1,999 refreshes before
the fix, and none afterward. Unicode, empty spans and multibyte style IDs
round-trip through truncation and append.

In `tests/transcript_memory.rs`, warming twelve 800-line TypeScript writes
grew resident memory **11.2 MB before, 2.1 MB after**. This includes the app's
existing numbered-output cap, which the change leaves intact. The regression
test first failed on the old representation, then passed on the compact one.
Round-trip tests preserve every span/line property, and buffer comparisons
exercise themes, Unicode, short/narrow terminals, scrolling and backtracking.

The rewrite path had another substantial temporary allocation:
`llm::tools::diff_lines` filled up to a million `usize` LCS scores in a vector
per old line, about **8 MB** on 64-bit systems. The matching budget implies
that a score is at most 1,000, so a flat `u16` table holds the same scores in
at most **2 MB**, with the zero boundary implicit. Empty middles allocate no
table. Prefix/suffix trimming, remove-on-tie choices, counts and the existing
large-diff fallback are unchanged. An exhaustive small-input oracle and a
full-budget match test compare exact `DiffLine` output to the original
algorithm. The isolated peak regression measured **8.5 MB before, 2.1 MB
after** and requires a budget-sized diff to stay below 4 MiB.

### Share generated arguments across request snapshots

A `write` tool call contains the complete generated file in its JSON arguments.
Cloning the conversation for a request round, retained wire prefix or prompt
cache markers used to copy that source each time. `llm::ToolArguments` now
holds an immutable `Arc<String>`: moving an owned string preserves its buffer,
and cloning a message shares it. Serialization preserves the original JSON
text, including whitespace and escape spelling. The retained wire-prefix
budget charges the string's capacity and shared-allocation metadata, so a
short argument with an oversized buffer cannot bypass the existing 8 MiB cap.

`tests/tool_arguments_memory.rs` keeps four snapshots of a 6 MiB write alive
in an isolated process. The test first failed with **24.0 MiB** of additional
RSS, then passed with **0 bytes** of measured growth. Unit tests check shared
allocation identity, exact serialized bytes and wire-prefix reuse. This is
the removed duplication in that controlled case; whole-process RSS also
depends on rendering and allocator reuse.

For library callers, `FunctionCall.arguments` is now `ToolArguments` instead
of `String`. `ToolCallSpec::function` accepts the same inputs as before. Manual
field initializers can use `arguments: source.into()`; read through `as_str()`
or produce an independent string with `to_string()`.

These two additions selectively port the useful changes from
`perf/reduce-post-generation-ram` (`342bac9`): shared arguments and style
reference cleanup. The compact varint rows, borrowed viewport rendering,
implicit-border diff table and existing buffer growth remain in place.

### Repeating the real CLI workload without a model key

Build a release binary, then run:

```sh
python3 scripts/codegen_mem.py target/release/alter-zero --lines 5000 --turns 4
```

The Linux/tmux harness starts an isolated loopback Chat Completions provider
that streams TSX, CSS and JSON writes in fragments, then drives the real
binary through repeated generation turns. It verifies every generated file
byte-for-byte, opens Ctrl+O and Ctrl+D, resizes both directions, and clears
the conversation. `--output work/codegen.json` saves RSS, anonymous/file
resident memory and sampled high-water readings. No generated application is
started. Automatic compaction is disabled in the fixture to retain the whole
stress history; normal application defaults are unchanged.

Matched release builds of pristine `main` (`e98680d`, v0.15.0) and the initial
compact-cache/diff change (`774ff7c`), on Linux x86_64 with glibc, after four
generation turns:

| TSX lines per turn, plus CSS/JSON | before RSS | after RSS | reduction |
| --- | --- | --- | --- |
| 800 | 38.85 MiB | 35.17 MiB | 9.5% |
| 2,000 | 44.76 MiB | 40.46 MiB | 9.6% |
| 5,000, two-run mean | 57.40 MiB | 52.00 MiB | 9.4% |

For the 5,000-line case, the mean after opening Ctrl+D was **64.04 → 53.78
MiB** (16.0% less); after `/clear`, **58.15 → 47.34 MiB** (18.6% less).
All eight runs verified generated files, request counts, both overlays,
resizing and clear. Final Ctrl+O opens were observed within 7–33 ms, including
tmux command/capture overhead. The allocator varies between runs: the two
baseline large-workload readings after generation were 60.53 and 54.26 MiB,
versus 50.98 and 53.02 MiB after the change. The means are a comparison of
these trials, not a guarantee for every session. Saved JSON includes the
binary's SHA-256 so future comparisons can identify the exact executable.

The selective shared-arguments/style-cleanup follow-up was measured separately
against `774ff7c`, with those first improvements already present. Both builds
use the same v0.15.0 base. Two trials per build and workload gave:

| Retained RSS, two-run mean | before follow-up | combined | reduction |
| --- | --- | --- | --- |
| 5,000-line TSX plus CSS/JSON, four turns | 58.23 MiB | 49.34 MiB | 15.3% |
| 13-file dense generation, four turns | 59.89 MiB | 52.44 MiB | 12.4% |
| Dense generation after export | 61.94 MiB | 55.35 MiB | 10.6% |
| Dense generation after clear | 59.87 MiB | 53.28 MiB | 11.0% |

The dense fixture is `scripts/task_mem.py` from `342bac9`, with 600 lines,
eight components and 563,299 generated bytes per turn. Its peak RSS mean fell
from 103.57 to 96.47 MiB. All eight runs verified generated file bytes,
overlays, resize and clear; the dense runs also checked verbatim tool context
and transcript export. A scratch extension of `codegen_mem.py` checked all
prior generated paths and contents in every request too. Export's retained
increment stayed between 0.55 and 3.65 MiB across both builds.

The large TSX readings ranged from 52.64–63.82 MiB before to 47.71–50.96 MiB
combined. These trials show additional savings in the tested workloads, with
the same allocator variation described above; they do not predict an exact
RSS for a live model's task-manager implementation.

This is a deterministic replay of the allocation paths, not the output of a
particular live model. Fixed costs such as the grammar database, tokenizer
and HTTP/TLS state remain, and raw conversation history still grows as work
is recorded. A single item's temporary expanded rows and the allocator's
retained pages can also affect RSS, so the reduction varies with the session.

## The rule

The pattern generalises past this one function: **do not build a tree of a
body you are going to read seven fields out of.** Where a response is large
and mostly ignored, hold the records as `RawValue` and decode them one at a
time. The cost of getting this wrong is not a spike — glibc will not give the
pages back, so it is a permanent addition to a process the user leaves
running all day. `src/llm/models.rs` was the only place in the tree parsing a
large external list this way; the other `Vec<Value>`s (`llm::tools`'
tool specs, the MCP manager's tool specs) are small, fixed schemas the app
*sends*.

The same rule wears a second coat for pictures: **never decode a picture
whole to make a small one, and never decode what you were handed encoded.** A
screenshot is the largest single allocation this process ever sees, and the
allocator's dynamic threshold turns the second such allocation into a
permanent one. `images::fitted` streams rows; `clipboard::linux` streams
bytes.

And a third for the request: **never allocate anything picture-sized per
turn.** An attachment is encoded once and shared (`images::attachment`), the
request is serialized from the messages by reference on every wire, and the
body leaves the process through a pipe of small chunks rather than as a buffer
(`llm::body::streamed_request`). A block that size, asked for every turn on a
fresh thread, is a block glibc will keep in some arena sooner or later.

And a fourth for anything rendered as it streams: **keep what the view can
show, not everything it has seen.** A cache sized to the input rather than to
its window is a cache sized by the largest input the session ever met, for the
rest of the session.

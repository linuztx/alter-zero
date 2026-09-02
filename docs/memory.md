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

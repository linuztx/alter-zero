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

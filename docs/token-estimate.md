# Token estimates

Every token figure the app works out for itself goes through one seam,
`app::count_tokens` → `token_estimate::estimate`: the status line's tally
between usage frames, the context gauge before a provider has reported, the
`/context` page's breakdown, Ctrl+D's per-entry sizes, a `Thought for` cell
before its round's frame, the share of an interrupted turn's receipt no frame
covered. It is an **estimate**, and a provider-agnostic one on purpose: what a
provider reports in its usage frames always replaces it
(`docs/prompt-caching.md`), and teaches every context estimate after it that
provider's units.

## Why not a tokenizer

The app used to count with tiktoken's `o200k_base` — first through
`tiktoken-rs`'s `CoreBPE` (45.5 MB resident), then through a compact store of
our own over the same vocabulary (a 1.6 MB embedded asset, ~5 MB resident, a
`fancy-regex` split pattern compiled on a warm-up thread at startup, and
`tiktoken-rs` kept as a dev-dependency to prove the two counted alike). It
was exact for one vendor's models. The providers this app talks to tokenize
with a dozen vocabularies — OpenAI's, Anthropic's, Gemini's SentencePiece,
Llama's, Qwen's, DeepSeek's, Mistral's — none of which a client can ship for
all of them, and they disagree by a factor of two or three outside English:
the Hindi sample below is 28 tokens to `o200k_base` and 100 to `cl100k_base`.
"Exact for OpenAI models, close for the rest" was only true for English and
code.

An exact count for one vocabulary was never what the figures needed. Each
one is a live stand-in until the provider's own count arrives — and the
provider's own count is the only exact one there is. What they need is to be
cheap, close for every provider, and *consistent* across kinds of text, so
that the correction a provider's report teaches carries from one context to
the next (*Corrected by every provider's report*, below). A vocabulary bought
none of that.

## What it models

One pass over the text, in thousandths of a token, with no vocabulary, no
regex and no allocation — what every byte-pair tokenizer does, as rules:

| piece | estimate |
| --- | --- |
| a word (letters of any non-CJK script) | a token for its first six letters, a quarter token for each letter after; cut into parts where `camelCase` changes case, `HTMLParser` at the last capital |
| an all-capitals part (`HTTP`, `MAX`) | about six letters a token, at least one |
| an accented Latin letter | 0.4 on top of its word |
| a Cyrillic letter | 0.3 |
| a Greek, Armenian, Georgian, Hebrew or Arabic letter | 0.5 |
| a letter of the Indic scripts, Thai, Lao, Tibetan, Myanmar, Khmer, Ethiopic | 0.55 |
| a Han character, kana or Hangul syllable | 0.85 |
| a number | a token per three digits |
| one space before a word or punctuation | nothing — it rides the piece |
| one space before a number | 1.5 — no pre-tokenizer joins a space to digits |
| two or more spaces, or spaces ending the text | 1.3 for the run |
| a run of line breaks (spaces between them included) | 0.75 |
| a line break straight after punctuation | nothing — it joins the punctuation |
| a run of punctuation | a token per five characters, at least one |
| one punctuation character opening a word | 0.5 — or 0.15 for the ones paths, URLs and identifiers are made of: `/ . _ - ' @ $ # ? & % \ < * ! + =` |
| an emoji or pictograph | 2.2, and 0.6 per joiner, variation selector or skin tone |
| a random-looking base64 run | about a token per one and a half characters |
| a control character | 1 |

A word never costs less than a token, and text that is not empty is never
less than one. The base64 rule is the one place the estimator looks ahead: a
run of the base64 alphabet at least twenty long, with both letter cases, whose
letters flip case or turn into digits at a third of its characters or more.
Words, paths and identifiers flip a handful of times in that span; base64
flips at every other character, and tokenizes at three times the density of
words — the integrity hashes in a lockfile, a JWT in a log, a `data:` URL a
tool printed. A run that fails the test is read as words and numbers, and
never tested again, which keeps the pass linear.

## How it was fitted, and how close it is

The rules' costs were fitted by coordinate descent, then rounded, against
tiktoken's `o200k_base` (GPT-4o onward) and `cl100k_base` (GPT-4, and the
shape of the Llama 3 and Qwen vocabularies) over 307 samples of about 3 KB:
this repository's Rust, Markdown, JavaScript, Python, shell, TOML, YAML and SQL;
pretty and minified JSON; `git log --stat`, diffs, `ls -la`, `ps`, `find` and
`grep` output; logs, CSV, URLs, UUIDs, hex digests and base64; prose in sixteen
languages and an emoji-heavy chat. The target was the two references'
geometric mean weighted 0.65 toward `o200k_base`, since most providers now
serve the newer, denser vocabularies.

Estimate over reference, as the geometric mean of each category ± the spread
of its natural log — a spread of 0.04 is about ±4% — against the two rules
of thumb other agents use, `bytes/4` (codex) and `chars/4` (Claude Code):

| category | `estimate` vs midpoint | `bytes/4` | `chars/4` |
| --- | --- | --- | --- |
| code | 1.00 ± 0.04 | 0.89 ± 0.28 | 0.89 ± 0.28 |
| data (JSON, CSV, logs, blobs) | 1.00 ± 0.03 | 0.73 ± 0.45 | 0.73 ± 0.45 |
| English prose | 1.00 ± 0.03 | 1.00 ± 0.08 | 0.99 ± 0.08 |
| tool output | 0.99 ± 0.04 | 0.84 ± 0.19 | 0.83 ± 0.19 |
| other languages | 1.00 ± 0.09 | 1.10 ± 0.20 | 0.68 ± 0.46 |
| **all** (spread) | **0.051** | 0.303 | 0.353 |

Where the two vocabularies agree — English, code, data, tool output — the
estimate is within a few percent of both. Where they do not, it lands between
them: 1.21× `o200k_base` and 0.70× `cl100k_base` on the other languages.

## Corrected by every provider's report

The estimate is where the figures start; a provider's usage frame says what
it actually counted. So the backend estimates each request as it sends it —
the system prompt, the tool schemas as the client serializes them (worked out
once, when the set is attached), every message's text, a flat charge per
image, each replayed call's name and arguments — and announces it,
`StreamEvent::RequestEstimate { input, tools }`, before the request goes out.
Two things follow:

- **Before the request is answered**, the footer gauge reads it. A pasted
  file or a long tool result shows what it costs while the model is still
  working, where the gauge used to sit still until the round's frame landed.
- **When the round's frame lands**, the gauge snaps to the provider's own
  `input + output`, as it always has, and `token_estimate::Calibration`
  keeps the ratio of the frame's `input` to that request's estimate.

The ratio is the provider's tokenizer — and its chat framing, and what it
charges for an image — relative to the estimate, and every figure the app
works out after it is scaled by it:

| figure | scaled |
| --- | --- |
| the gauge while a request is out | the request's estimate × the ratio |
| the gauge after a rewind, a `/resume`, a compaction, or a turn that sent no frame | the derived context's estimate, plus the tool schemas' share of the last request (`App` never sees the schemas), × the ratio |
| the `/context` breakdown | every category × the ratio — a viewed subagent's own in its session view |
| Ctrl+D's entry sizes and their total | × the ratio |

The newest report **replaces** the last rather than averaging with it. Every
request carries the whole conversation before it, so the newest ratio is the
one measured over the context as it is now; and scaling the next request's
estimate by it is exactly the last reported count plus the estimated growth
since, in the provider's tokens:

```
r = reported_last / estimated_last
r × estimated_next = reported_last + r × (estimated_next − estimated_last)
```

That is the anchor-plus-growth rule codex and Claude Code keep their context
counts by (the last count the API reported, plus an estimate of what has been
added since), with the growth in the provider's units rather than the
estimate's — and it holds for a shrink as well: a rewind or a compaction
re-estimates what is left, in the units the provider last reported in.

What it ignores, and when it starts over:

- **A request estimated under 256 tokens** teaches nothing: the provider's
  per-message framing outweighs the text.
- **A ratio outside 0.25–4×** is a usage frame gone wrong — a shim reporting
  only the uncached share, or a session total — not a tokenizer, and is
  dropped.
- **Another model answering** starts it over (`App::set_session_info`), since
  its tokenizer was never measured; a rebuild on the same model — a
  `/settings` knob, Ctrl+T — keeps it, and so does `/clear`.
- **A `/compact` request** teaches the session's ratio like any other, but is
  sent without tools, so it never replaces the tool schemas' share the
  conversation's requests carry.
- **Each subagent** learns its own from its own frames; a definition may pin
  another model. A subagent turn that settles with no frame builds on its
  last announced request — which carried its system prompt, briefing and tool
  schemas — plus the reply that ended it.
- **The live tally** is not scaled: between frames it counts what streams,
  and each round's frame replaces it outright.

## Cost

About 106 MB/s on the corpus above, in a release build — `tiktoken-rs`'s
`o200k_base` encodes the same text at 14 MB/s — with nothing to load, nothing
resident and nothing to warm: the startup thread that built the old store is
gone, and with it 1.6 MB of the binary, the `fancy-regex` dependency and the
dev-dependency on `tiktoken-rs`. The live tally estimates each streamed piece
as it lands; word-sized pieces add up to exactly what their whole estimates.

## Proven, permanently

`token_estimate`'s tests hold the calibration to the rules above; `app` and
`agents` hold the gauge to them — the announcement moving it, a frame
teaching the next request, a re-estimate in the provider's tokens with the
tool schemas counted, the compaction request leaving their share, a model
switch starting over — and `llm::backend` holds a round to announcing its
estimate exactly once, before it streams, a retry included.


`src/token_estimate.rs`'s tests pin the behaviours above — a common word is a
token with the space before it, the space before a number is not, digits go
in threes, indentation is a token a run, a base64 run is dense and a path is
not one — and keep the fit honest without a reference crate: twenty-one
samples of the corpus carry the `o200k_base` and `cl100k_base` counts they had
when the costs were fitted, every one must land within a quarter of the
weighted midpoint, and the English, code and tool-output ones within a tenth
of `o200k_base` in sum. Changing a cost is changing the fit, and those tests
say whether it still holds.

## Not modelled

- **Images.** One is `IMAGE_TOKENS` (256) whatever its size; providers price
  them by tiles, by pixels or flat. The ratio absorbs the difference, which
  over-scales the text that follows a picture-heavy request until the next
  frame corrects it.
- **Chat framing.** The few tokens each provider wraps a message in, and its
  own rendering of a tool schema, are nobody's text to estimate.
- **The compaction budget** stays codex's `bytes/4` (`docs/compact.md`): it
  chooses which user messages a compacted context replays, and keeping it
  byte-for-byte codex's keeps that choice the reference's.

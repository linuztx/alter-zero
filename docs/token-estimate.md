# Token estimates

Every token figure the app works out for itself goes through one seam,
`app::count_tokens` → `token_estimate::estimate`: the status line's tally
between usage frames, the context gauge before a provider has reported, the
`/context` page's breakdown, Ctrl+D's per-entry sizes, a `Thought for` cell
before its round's frame, the share of an interrupted turn's receipt no frame
covered. It is an **estimate**, and a provider-agnostic one on purpose: what a
provider reports in its usage frames always replaces it
(`docs/prompt-caching.md`).

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
cheap, close for every provider, and *consistent* across kinds of text. A
vocabulary bought none of that.

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

## Cost

About 106 MB/s on the corpus above, in a release build — `tiktoken-rs`'s
`o200k_base` encodes the same text at 14 MB/s — with nothing to load, nothing
resident and nothing to warm: the startup thread that built the old store is
gone, and with it 1.6 MB of the binary, the `fancy-regex` dependency and the
dev-dependency on `tiktoken-rs`. The live tally estimates each streamed piece
as it lands; word-sized pieces add up to exactly what their whole estimates.

## Proven, permanently

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
  them by tiles, by pixels or flat.
- **Chat framing.** The few tokens each provider wraps a message in, and its
  own rendering of a tool schema, are nobody's text to estimate.
- **The compaction budget** stays codex's `bytes/4` (`docs/compact.md`): it
  chooses which user messages a compacted context replays, and keeping it
  byte-for-byte codex's keeps that choice the reference's.

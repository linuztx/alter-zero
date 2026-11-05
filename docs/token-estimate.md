# Token estimates, corrected by the provider

Date: 2026-11-05

The app needs token counts it has not been given yet. The status line's tally
ticks while a reply streams, the footer's context gauge has to say how full
the window is before the next request is sent, auto-compact decides whether
to run at 90% of it, the `/context` page splits the window into its parts,
Ctrl+D labels every message, and a compaction records how far it shrank the
context. The provider only reports a request's real size once per round, in
its usage frame (`StreamEvent::Usage`, `docs/prompt-caching.md`). Everything
else is estimated.

`src/tokens.rs` is the whole answer, in two halves:

1. **`tokens::estimate`** counts a text's tokens before any provider has
   seen it. It needs no vocabulary and favours no provider.
2. **`tokens::ContextMeter`** and **`tokens::Calibration`** take each
   usage frame as the truth it is, and correct every later estimate of the
   same conversation with what it taught.

## What it replaced, and why

The app used to count with OpenAI's `o200k_base` byte-pair encoding: a
compact in-crate port of tiktoken, with a 1.6 MB vocabulary embedded in the
binary, ~5 MB resident once loaded, `fancy-regex` for its split pattern, a
thread at startup to build its tables before the first turn needed them,
and `tiktoken-rs` as a dev-dependency to prove it counted the same.

It was exact, but only for the wrong tokenizer most of the time. Every
provider outside OpenAI counts with its own vocabulary: Anthropic's,
Gemini's on Antigravity, a local model's on Ollama or LM Studio, any of
OpenRouter's catalog. Exactness for one vocabulary bought nothing for the
others. The gap mattered most exactly where the app trusted the count
alone: after a compaction, a rewind, `/clear` or `/resume` the gauge fell
back to the raw count, which could see neither the provider's tokenizer nor
the tool schemas every request carries. Between usage frames, nothing added
to the conversation moved the gauge at all, so a large paste or tool output
showed up only once the next request had been counted.

So the exact count was replaced by an estimate that is close for every
provider. Each provider's own reports then pull the estimate to that
provider.

## The estimate

Every model this app talks to splits text the same way **before** its
vocabulary is consulted: the GPT-style pre-tokenizer that OpenAI,
Anthropic, Llama, Qwen, DeepSeek and Mistral encoders share. A word with
the space or the single symbol in front of it is one piece, a case change
starts another, digits go three at a time, a run of whitespace is one
piece, and punctuation runs are pieces of their own. A vocabulary of
100k–200k entries holds nearly every common piece whole, so counting the
pieces gets most of the way to a real count. The rest is a small charge
per letter for the long and the rare.

`estimate` is one pass over the characters. It classifies each one (an
ASCII table on the hot path) and prices each piece as it closes, in
hundredths of a token so that fractions add up exactly and only the total
is rounded:

| Piece | Cost | Why |
| --- | --- | --- |
| a word after a space | 1, +0.04 a letter past 4 and again past 7 | ` function` is one token in every modern vocabulary |
| a word after nothing | 1, +0.06 past 4, +0.09 past 7 | identifiers and line starts split a little more |
| a word after one symbol (`.method`, `/usr`, `_name`) | 1, +0.1 a letter past 3 and again past 8 | the symbol rides with the word, but such pieces are learned whole less often |
| a word with no vowel (`lrwxrwxrwx`, `kvfree`) | 1, +0.35 a letter past 2, +0.45 past 5 | an abbreviation when short, random letters when long |
| digits | 1 per 3 | the shared `\p{N}{1,3}` rule |
| whitespace | 1 for the line breaks, 1 for the run after them less the one space the next piece takes; a number takes none | a run of any length is one token |
| line breaks straight after symbols | free | `{` and its newline are one piece |
| symbols | 1 for one or two, then ~1 per 2 (non-ASCII ones count double) | `();`, `=>` and `":"` merge |
| a repeated rule character (`-`, `=`, `_`, `─`) | 0.08 per repeat | 24 dashes are 2 tokens |
| other repeated ASCII symbols | 0.25 per repeat | brackets and quotes merge in pairs |
| an emoji | 2, its joiners, selectors and skin tones 1 | |
| CJK ideographs, kana, hangul | 0.7 each | common two-character words are one token |
| other scripts | 0.24 (Greek, Cyrillic) to 0.45 (Thai, Khmer…) per letter | thinner vocabulary coverage |
| a stretch of random data | at least 0.68 per character | a 16+ character stretch between whitespace that mixes all of lower case, upper case and digits and switches between them on 30% of its characters: base64, keys, hashes |

### How close it is

The rules were fitted against the exact `o200k_base` counts of the old
encoder, measured before it was removed. The corpus was this repository's
own Rust, docs, prompts, JavaScript, Python and shell, plus `ps`, `ls`,
`git log --stat`, a diff, a hexdump, base64, UUIDs, numbers, pretty and
compact JSON, and prose in eight other languages:

| Content | Estimate ÷ o200k | Mean error per file |
| --- | --- | --- |
| Rust (376 files, 3.0M tokens) | 1.008 | 1.5% |
| Markdown docs (113 files, 630k tokens) | 0.982 | 2.0% |
| JavaScript / Python / shell | 0.996 / 0.979 / 0.969 | 2.7% / 1.9% / 3.3% |
| JSON (pretty / compact schema) | 0.995 / 1.053 | |
| a diff, `git log --stat`, `ps`, `ls -la` | 0.983, 0.963, 0.956, 0.911 | |
| base64, UUIDs, numbers, hexdump | 0.993, 0.974, 1.000, 0.948 | |
| emoji | 1.017 | |
| Spanish/German/French, Japanese, Korean, Arabic, Thai, Hindi | 0.97–1.06 | |
| Russian, Chinese | 1.10, 1.12 | |
| **everything (4.3M tokens)** | **0.999** | |

Russian and Chinese come out a little high on purpose. `o200k_base` is the
most efficient tokenizer there is on those scripts, and most of the others
are less so, so a slight over-count is the side to err on before a provider
has reported anything. The test suite keeps the estimate honest without
the encoder. Fourteen hand-written samples (prose, Rust, Python, two kinds
of JSON, `ls -la`, a diff, a markdown table, base64, UUIDs, a log, CJK,
Cyrillic, emoji) carry their frozen `o200k_base` counts, and each estimate
must land within 15% of its count, the fourteen together within 3%
(`every_reference_sample_lands_within_fifteen_percent`,
`the_samples_together_land_within_three_percent`).

It costs nothing at rest. There are no tables to build and nothing stays
resident, so the startup thread that warmed the encoder is gone. It is a
single pass with no allocation, about four times faster than the encoder
in the same build (58 MB/s against 14 MB/s over the corpus above). That
speed matters because a tool's output and every whole-context estimate go
through it.

## The correction

### What the gauge reads

`ContextMeter` keeps one conversation's context size in the provider's
tokens:

- **A usage frame sets the base.** Its `input` is the provider's own count
  of the request's whole context, covering what the estimate can see and
  what it cannot: tool schemas, the chat template's framing, anything the
  provider adds. The meter sets that count beside the estimate of the same
  context, taken at the frame, and the gauge reads exactly `input`. The
  reply the frame reports as `output` is not context until it is recorded.
  Its reasoning never is on most wires, which is why the gauge no longer
  adds `output` in (the old `input + output` over-counted a reasoning
  model's turns until the next frame).
- **What joins after the frame adds its corrected estimate.** The reply, a
  tool's output, a message the user sends next. The loop bottom
  (`App::sync_context_used`, which costs two integer compares when nothing
  moved) estimates only the history that joined since the base, derived
  exactly as a request would carry it. So a large paste or tool output
  shows on the footer at once, before the next request is counted.
- **A history the frame no longer describes moves the base.** A
  compaction, a rewind, `/clear` followed by a new message, or `/resume`
  re-estimates the whole context once and moves the reading by the
  corrected estimate of the change. The share of the count the estimate
  never saw stays in. Here is what that means for a compaction. The last
  frame counted 88k, of which the estimate saw 78k, and the summary leaves
  2k estimated. The gauge, and the `● Context compacted · 88k → 12k
  tokens` cell, read 12k: the 2k left plus the 10k of schemas and framing
  that ride the next request too. They used to read the bare 2k.
- **An empty conversation still reads a true zero**, as it always did
  (`docs/compact.md`).
- **The summarizer's own frame measures nothing.** A `/compact` request is
  shaped unlike the conversation's: no tools, plus the compact prompt.

### What the provider teaches

`Calibration` learns how this provider's tokens compare with the
estimate's. A sample is the change between two consecutive frames: the
provider's count of what joined the context against the estimate of the
same change. Whatever both frames carried unchanged (the tool schemas, the
system prompt, the framing) cancels out. What remains is how many tokens
the provider's tokenizer makes of this session's text per estimated one —
say 1.18 for a model whose tokenizer splits code into more pieces than
`o200k_base` does. Several guards keep bad samples out:

- a change under 256 estimated tokens is skipped, since a chat template's
  per-message framing is a real share of anything smaller;
- a change whose ratio falls outside 0.4–2.5 measured something else (a
  tool set that connected, a changed system prompt) and is skipped;
- a change during which a picture came or went is skipped, because the
  estimate charges pictures a flat 256 tokens the provider does not share;
- a model switch (`App::set_session_info` naming another model) starts the
  calibration over, and the new model's first frame is not compared with
  the old model's;
- the ratio is the samples' sum over the estimates' sum, so big changes
  weigh more than small ones. Older samples are halved away once the
  estimates pass 64k tokens, so it follows what the session is doing now.
  It is held to 0.5–2.0, the most one modern tokenizer differs from
  another.

The ratio corrects every estimate of context: what joins after a frame,
the change a compaction or rewind makes, every count on the `/context`
page, and every Ctrl+D label. The page's total and the footer then speak
the same tokens. Both views apply it outside their memos, which hold the
estimate's own counts, and both key their caches on it, so a new lesson
recounts without re-tokenizing. It does **not** touch the status line's
live tally, which every frame snaps to the provider's billed total anyway
(`docs/prompt-caching.md`), or a `Thought for …` cell, which snaps to the
provider's `reasoning_tokens` (`docs/thinking-stream.md`).

A subagent keeps its own meter on `AgentRun`, with the same rules one
level down (`docs/agent-context-gauge.md`). It reads at its frames and at
every settle, so its session view's gauge folds in the reply each turn
ends on.

## Limits

- The calibration lives in memory. A new session or a resumed one starts
  at 1.0 and learns from its second frame on.
- A picture is a flat 256 tokens in the estimate. The provider's real
  charge enters only through a frame's base, never through the ratio.
- One ratio covers a whole conversation, so a session that mixes text a
  provider counts very differently (prose, then base64) is corrected on
  average.
- The gauge between frames is still an estimate, but only of what joined
  since the last frame, which is usually a small share of the window.

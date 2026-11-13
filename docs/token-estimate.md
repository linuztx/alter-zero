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

The providers this app talks to tokenize with a dozen vocabularies —
OpenAI's, Anthropic's, Gemini's SentencePiece, Llama's, Qwen's, DeepSeek's,
Mistral's — and outside English they disagree by a factor of two or three:
the Hindi sample in the tests is 28 tokens to `o200k_base` and 100 to
`cl100k_base`. No local count can be exact for all of them, so the estimate
does not try to be exact for one. What all of them do with a text is much
the same, and `estimate` models that: one pass over the characters, an
ASCII table on the hot path, every piece priced in thousandths of a token
so that fractions add up exactly and only the total is rounded, with no
vocabulary, no regex and no allocation.

| Piece | Estimate |
| --- | --- |
| a word (letters of any non-CJK script) | a token for its first six letters, a quarter token for each letter after; cut into parts where `camelCase` changes case, `HTMLParser` at the last capital |
| an all-capitals part (`HTTP`, `MAX`) | about six letters a token, at least one |
| an accented Latin letter | 0.4 on top of its word |
| a Cyrillic letter | 0.3 |
| a Greek, Armenian, Georgian, Hebrew or Arabic letter | 0.5 |
| a letter of the Indic scripts, Thai, Lao, Tibetan, Myanmar, Khmer, Ethiopic | 0.55 |
| a Han character, kana or Hangul syllable | 0.85 |
| a number | a token per three digits |
| one space before a word or punctuation | nothing: it rides the piece |
| one space before a number | 1.5: no pre-tokenizer joins a space to digits |
| two or more spaces, or spaces ending the text | 1.3 for the run |
| a run of line breaks (spaces between them included) | 0.75 |
| a line break straight after punctuation | nothing: it joins the punctuation |
| a run of punctuation | a token per five characters, at least one |
| one punctuation character opening a word | 0.5, or 0.15 for the ones paths, URLs and identifiers are made of: `/ . _ - ' @ $ # ? & % \ < * ! + =` |
| an emoji or pictograph | 2.2, and 0.6 per joiner, variation selector or skin tone |
| a random-looking base64 run | about a token per one and a half characters |
| a control character | 1 |

A word never costs less than a token, and text that is not empty is never
less than one. The base64 rule is the one place the estimate looks ahead: a
run of the base64 alphabet at least twenty long, with both letter cases,
whose letters flip case or turn into digits at a third of its characters or
more. Words, paths and identifiers flip a handful of times in that span;
base64 flips at every other character, and tokenizes at three times the
density of words: the integrity hashes in a lockfile, a JWT in a log, a
`data:` URL a tool printed. A run that fails the test is read as words and
numbers, and never tested again, which keeps the pass linear.

### How it was fitted

The costs were fitted by coordinate descent, then rounded, against
tiktoken's `o200k_base` (GPT-4o onward) and `cl100k_base` (GPT-4, and the
shape of the Llama 3 and Qwen vocabularies) over 307 samples of about 3 KB:
this repository's Rust, Markdown, JavaScript, Python, shell, TOML, YAML and
SQL; pretty and minified JSON; `git log --stat`, diffs, `ls -la`, `ps`,
`find` and `grep` output; logs, CSV, URLs, UUIDs, hex digests and base64;
prose in sixteen languages and an emoji-heavy chat. The target was the two
references' geometric mean weighted 0.65 toward `o200k_base`, since most
providers now serve the newer, denser vocabularies. Where the two agree
(English, code, data, tool output) that target is within a few percent of
both; where they do not, it lands between them.

### How close it is

Measured on 13 MB of text it was not fitted on, 3.7M `o200k_base` tokens:
Rust from 198 crates, the Python standard library, C headers, JavaScript and
TypeScript declarations, JSON, TOML, shell scripts, fresh command output,
and two texts (the vim tutor and a chapter of *Alice*) in about thirty
languages:

| Content | ÷ `o200k_base` | ÷ `cl100k_base` | Mean error per file |
| --- | --- | --- | --- |
| Rust / Python / C | 0.99 / 1.01 / 0.97 | 1.00 / 1.02 / 0.98 | 4–6% |
| JavaScript / TypeScript declarations | 1.00 / 1.11 | 1.01 / 1.12 | 5% / 16% |
| Markdown / JSON / TOML | 0.97 / 0.94 / 0.96 | 0.97 / 0.94 / 0.96 | 4% / 9% / 4% |
| shell scripts / command output | 0.92 / 0.99 | 0.93 / 0.99 | 8% / 6% |
| English prose | 1.08 | 1.00 | |
| 21 other Latin-script languages | 0.89 | 0.79 | 11% |
| Russian, Ukrainian, Bulgarian | 0.98 | 0.68 | |
| Greek / Arabic and Hebrew / Thai | 1.14 / 1.11 / 1.15 | 0.55 / 0.52 / 0.58 | |
| Chinese, Japanese, Korean | 1.08 | 0.83 | |
| **all of it** | **0.98** | **0.97** | |

A provider whose vocabulary is like `o200k_base` meets an estimate within a
few percent on what a coding session mostly carries. A provider with a
thinner vocabulary for a language counts that language higher than the
estimate does, and that is what the calibration below learns. Once a
provider has reported, the level matters less than how *consistently* the
estimate tracks it across kinds of text, because one ratio covers a whole
conversation. Across the rows above, the log of the ratio spreads by 0.07
(about ±7%) against `o200k_base` and by 0.23 against `cl100k_base`.

The tests keep the fit honest without a reference crate. Twenty-one samples
carry the `o200k_base` and `cl100k_base` counts they had when the costs were
fitted. Each must land within a quarter of the weighted midpoint, and the
English, code and tool-output ones within a tenth of `o200k_base` in sum
(`every_sample_lands_within_a_quarter_of_the_reference_tokenizers`,
`english_code_and_tool_output_land_within_a_tenth_of_the_tokenizers`).
Changing a cost is changing the fit, and those tests say whether it still
holds.

It costs nothing at rest. There are no tables to build and nothing stays
resident, so the startup thread that warmed the encoder is gone. It is a
single pass with no allocation, about 160 MB/s on the corpus above in a
release build, against 17–19 MB/s for `tiktoken-rs`'s `o200k_base` on the
same machine. Word-sized pieces add up to exactly what their whole
estimates, so the live tally, which estimates each streamed piece as it
lands, agrees with an estimate of the whole reply.

## The correction

### What the gauge reads

`ContextMeter` keeps one conversation's context size in the provider's
tokens:

- **A usage frame sets the base.** Its `input` is the provider's own count
  of the request's whole context, including what the estimate can only
  approximate: the tool schemas as the provider renders them, the chat
  template's framing, anything the provider adds. The meter sets that count
  beside the estimate of the same context, taken at the frame, and the gauge
  reads exactly `input`. The reply the frame reports as `output` is not
  context until it is recorded. Its reasoning never is on most wires, which
  is why the gauge does not add `output` in: the old `input + output`
  over-counted a reasoning model's turns until the next request, and could
  start an auto-compaction early.
- **What joins after the frame adds its corrected estimate.** The reply, a
  tool's output, a message the user sends next. The loop bottom
  (`App::sync_context_used`, which costs two integer compares when nothing
  moved) estimates only the history that joined since the base, derived
  exactly as a request would carry it. So a large paste or tool output
  shows on the footer at once, before the next request is counted.
- **Before any frame, the estimate is the reading, tool schemas included.**
  The backend's tool set (`ReplySource::tool_definitions`, each schema as
  the client serializes it) is handed to `App` at every loop bottom and
  estimated with the rest of the context. So the first request of a session
  and a provider that reports no usage both count the schemas every request
  carries.
- **A tool set that changes moves the reading, not the base.** An MCP
  server connecting, the Tools knob: the reading moves by the estimate of
  the schemas now offered less the estimate of the ones the base counted
  (`App::context_base_tools`). The base stays where the provider counted
  the old set, so the same set back reads the same number, and nothing is
  re-estimated.
- **A history the frame no longer describes moves the base.** A
  compaction, a rewind, or `/clear` followed by a new message re-estimates
  the whole context once and moves the reading by the corrected estimate of
  the change. Whatever the provider counts beyond the estimate stays in. A
  resumed conversation is not one of these: its rollout kept the meter, and
  the resume brings it back (*Kept with the session*, below). Here is what
  moving the base means for a compaction. The last
  frame counted 88k for a context the estimate put at 84k, since the
  provider renders the tool schemas and frames the messages its own way. The
  summary leaves 6k estimated, the schemas among it. The gauge, and the
  `● Context compacted · 88k → 10k tokens` cell, read 10k: the provider's
  count less the 78k the summary replaced. They used to read the bare
  estimate, without the schemas at all.
- **An empty conversation still reads a true zero**, as it always did
  (`docs/compact.md`), whatever the tools.
- **The summarizer's own frame measures nothing.** A `/compact` request is
  shaped unlike the conversation's: no tools, plus the compact prompt.

### What the provider teaches

`Calibration` learns how this provider's tokens compare with the
estimate's. A sample is the change between two consecutive frames: the
provider's count of what joined the context against the estimate of the
same change. Whatever both frames carried unchanged (the tool schemas, the
system prompt, the framing) cancels out. What remains is how many tokens
the provider's tokenizer makes of this session's text per estimated one,
say 1.18 for a model whose tokenizer splits code into more pieces than the
estimate does. Several guards keep bad samples out:

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

**Why the change between frames, and not the whole request.** A ratio of a
whole request's count to its estimate exists from the first frame, but it
scales what is not text as if it were: tool schemas a provider renders more
compactly than their JSON reads, a picture the provider charges 1,500 tokens
for and the estimate 256. Played through the same constructed cases, a
whole-request ratio read a compaction's result 50–65% high when the schemas'
estimate ran 8k over the provider's count, and the request after an early
picture 40% high. The change between frames read both exactly. The price is
a second frame before the ratio exists; the first frame is already the
reading's base.

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

### Kept with the session

The meter is the session's memory of what the provider said, and it used to
end with the process. A reopened conversation (`/resume`, `--resume`,
`--continue`) had no base, so it read the estimate of its whole context,
and the estimate is not the provider's count. A footer reading `8.1k/475k`
read `8.8k/475k` after `alter-zero -c`: 0.7k of distance between the
estimate and the provider's own count, nearly all of it in the system
prompt and the tool schemas every request carries, none of it something
the conversation had gained. The ratio went with it, so the `/context`
page and the Ctrl+D labels counted the same conversation in other numbers
too.

The rollout now keeps the meter, as a `context_meter` sidecar line
(`docs/rollout-records.md`, `tokens::MeterSnapshot`):

- the **base**: the provider's count (`tokens`) beside the estimate of the
  same context (`estimate`), and how many history items it covers
  (`after`). What the file records after them joined the context later;
- **`tools`**, the tool schemas' share of that estimate;
- **`report`**, the last usage report, which the next one learns against,
  and **`learned`**, the two sums the ratio is taken from;
- **`model`**, the model all of it is counted in.

A line is written whenever the meter moves (a report, a rewind, a
compaction, a model switch), behind the `usage` line of the request that
moved it, and the newest wins, like the `model` record. Between reports nothing is written:
what joins is read on top of the base, and the base holds still.

A resume restores it as written (`App::restore_context_meter`), once the
session's model is the one that will answer. The gauge then reads the
recorded count plus the estimate of what the file recorded after it, which
is the sum the live session read, to the token. Nothing is estimated again,
on purpose. An estimate of the whole context taken in the new process is
never quite the recorded one: the system prompt names that launch's own
scratch directory, whose session id differs in every process. Six launches
sending one message from one directory estimated the same request at
3,557 to 3,560 tokens. Three tokens are enough to turn `8.1k` into `8.2k`.

What is different about the reopened session is read as a difference:

- **Another model.** The recorded one has no key on this machine, or an
  environment pin outranks it (`docs/session-model.md`). The count stands
  until this model reports, and what the other model's tokenizer taught is
  dropped, as on a `/model` switch.
- **Another tool set.** The reading moves by the schemas now offered less
  `tools`. An MCP server that had connected when the session was recorded
  has not yet when the reopened one paints its first frame, so the footer
  reads that much less, and then the recorded number once the server is
  back. The meter never moves for it, so nothing is written either. This is
  the reason a tool-set change no longer re-estimates the context.
- **A file cut short**, whose record stands past the end of the history,
  is left alone: the estimate stands.

**A rollout from before the record** still has every request's `usage`
line, and the newest of the conversation's own (never a subagent's or a
compaction's) is the provider's count of the context up to where it sits.
`App::anchor_context_meter` takes it as the base, beside this session's
estimate of that context, and reads what was recorded after it on top. With
nothing learned yet that is the number the live session read. It teaches
nothing (`ContextMeter::anchor`): the estimate beside the count was taken
today, and a ratio across estimates from two different launches measures
the launches. The same rule covers a request an older build appended to a
file that has a record: a `usage` line behind the newest `context_meter`
line is one the record never saw (`session::parse_context_meter`).

A resume writes nothing. The restored meter is what the file says, so the
recorder takes it as written (`SessionRecorder::adopt`), and the next line
is the one the next report leaves. A session no provider reported usage
for (the offline demo) keeps no record at all: the estimate alone is
worked out again.

Tested at each layer. `tokens`: a restored meter is the meter it was taken
from and learns on from the same report, an anchor teaches nothing, a
reading moves by what joined less what left. `app::tests::compact`: a
reopened session reads what it read before it was closed, reads on as the
live one would, keeps the ratio, drops it on another model, and reads a
missing tool as that much less until it is back; a rewound and a compacted
session resume on what the rewind and the compaction left. `session`: the
line's shape, the newest record read back whole, a request recorded behind
it offered as unseen, and the line invisible to every other reader.
`tests/headless.rs` reads the lines off a real run and its `-c`.
`smoke.sh` Phase 154 compares the footer before the quit with the footer
after `--continue`, after the `/resume` picker, on a file with the records
taken out, once a ratio has been learned, and with an MCP server's tools in
the count.

## Limits

- A new session starts at a ratio of 1.0 and learns from its second frame
  on. A resumed one carries on from what its rollout kept; one recorded
  before the meter was kept starts at 1.0 on the provider's last count.
- Between reports the gauge reads only what joined the history and how
  the tool set changed. A system prompt or `<system-reminder>` that changed
  under it (an edited `AGENTS.md`, a skill added, an upgrade between a
  session and its resume) is counted by the next report.
- A picture is a flat 256 tokens in the estimate. The provider's real
  charge enters only through a frame's base, never through the ratio.
- One ratio covers a whole conversation, so a session that mixes text a
  provider counts very differently (prose, then base64) is corrected on
  average.
- A subagent's estimate holds no tool set, so its gauge counts its tool
  schemas from its first frame on.
- Re-estimating the whole context runs on the loop's thread once per frame,
  with every text's count remembered from the last time: under 2 ms for a
  2 MB history (3 ms the first time), so it stays there rather than moving
  to the backend's thread.
- The gauge between frames is still an estimate, but only of what joined
  since the last frame, which is usually a small share of the window.

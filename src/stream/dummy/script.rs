//! The dummy's canned replies and the pure primitives every scripted turn is
//! built from: pick a reply, split it into streamable chunks, acknowledge
//! attached images, and turn a canned tool output into live `ToolOutput`
//! deltas.
//!
//! The replies are the offline session's only voice, so they do a job: each
//! one narrates the demo it accompanies and ends on the **hand-off**
//! ([`HANDOFF`]) — the two commands that swap the dummy for a real model. See
//! `docs/dummy-backend.md`.

use super::super::StreamEvent;

/// The hand-off every user-facing demo closes on: with no provider configured
/// the session *is* the dummy, and `/login` (save a key) then `/model` (pick
/// one) are the way out. A macro rather than a `const` so the reply literals
/// can [`concat!`] it in — one source of truth, still a compile-time
/// `&'static str`.
macro_rules! handoff {
    () => {
        "Two commands away from the real thing: `/login` saves a provider API key, \
         then `/model` picks the model to run."
    };
}

// So the bespoke per-demo replies in `turns` close on the same sentence.
pub(in crate::stream) use handoff;

/// The hand-off sentence as a value — what the tests pin (every reply must end
/// on it) and what `scripts/smoke.sh` greps for as the settle marker of a
/// finished demo turn. Test-only, like `Scenario::name`: the replies embed the
/// macro directly, so nothing at runtime reads this.
#[cfg(test)]
pub(in crate::stream) const HANDOFF: &str = handoff!();

/// The dummy's **markdown table** demo reply, played for any prompt mentioning
/// "table" (opt-in, like the "parallel" batch): prose, a 10-row GFM table whose
/// cells carry `` `code` `` spans **and a two-column-wide status emoji**, then
/// closing prose. Its turn is text-only —
/// no thinking phase and no tool calls, which would split the reply around them
/// and flush the block early — so the whole forming table previews in the strip
/// and its block commits at the close (docs/table-streaming.md). This is the
/// reported blank-band regression's shape: the strip collapses from the tall
/// forming-table preview to one row in the same frame the block's rows flush,
/// which the smoke suite guards (the box must stay flush at the bottom).
///
/// The emoji make it the **emoji-table** shape too: a wide grapheme costs two
/// terminal columns, so a mismeasured one tears the grid's right border out of
/// line — the reported "emoji cuts the table" (`term::visible_cells`,
/// docs/table-streaming.md), which the smoke suite guards by asserting every
/// grid row is the same width.
pub(super) const DUMMY_TABLE_REPLY: &str = concat!(
    "Here's a table with data that uses backticks:\n\n\
     | ID | Name | Code Snippet | Description |\n\
     |----|------|--------------|-------------|\n\
     | 1 | Hello World | `` `print(\"Hello\")` `` | ✅ Basic greeting function |\n\
     | 2 | SQL Query | `` `SELECT * FROM users` `` | ✅ Database selection query |\n\
     | 3 | Markdown | `` `**bold text**` `` | ✅ Formatting example |\n\
     | 4 | Shell Command | `` `ls -la` `` | ✅ List directory contents |\n\
     | 5 | JavaScript | `` `const x = 42;` `` | ❌ Variable declaration |\n\
     | 6 | Rust | `` `let mut vec = Vec::new();` `` | ✅ Mutable vector creation |\n\
     | 7 | Python | `` `def foo(): return None` `` | ✅ Empty function definition |\n\
     | 8 | HTML | `` `<div class=\"container\">` `` | ❌ Container element |\n\
     | 9 | CSS | `` `.class { color: red; }` `` | ✅ Style rule |\n\
     | 10 | Regex | `` `/^[A-Z]+$/` `` | ✅ Pattern matching |\n\n\
     The backticks are wrapped in double backticks (`` `code` ``) so they display \
     properly in Markdown.\n\n",
    handoff!()
);

/// The **default turn's** canned replies — the offline session's first
/// impression, so each one is a guided tour rather than filler: it says plainly
/// that no model is attached, narrates the tool cells the turn is drawing, and
/// closes on the [`HANDOFF`].
///
/// Every reply is written in **two parts** separated by a blank line: the text
/// before the turn's tool batch and the text after it. A tool call finalises
/// the run of text before it as its own history message
/// (`App::flush_streaming_segment`, invariant 4), so splitting anywhere else
/// would cut a paragraph — or a list — across two messages and render it as two
/// broken blocks. [`reply_parts`] does the split; the tests pin it.
///
/// One is chosen deterministically per prompt so the demo has a little variety
/// with no model behind it.
const DEMO_REPLIES: &[&str] = &[
    concat!(
        "Sure thing — fair warning, though: I'm the built-in demo backend, so my \
         words are canned and these three calls are scripted. The errand is real \
         enough, mind you: `about.py` knows who wrote alter-zero and then prints a \
         card that forgets to say so.\n\n",
        "Fixed, and the run proves it: **alter-zero is by linuztx** — \
         https://github.com/linuztx. That is the loop a coding agent lives in, \
         and every cell above is the one a live model draws: the `Read` numbers \
         and highlights the file, the `Edit` shows only the line it touched (green \
         in, red out), the command prints the result. **ctrl+o** opens the full \
         transcript, **ctrl+d** the raw context.\n\n",
        handoff!()
    ),
    concat!(
        "Absolutely — with the caveat that I am a cardboard cutout of an AI. No \
         model is attached, so the errand below is scripted end to end: read \
         alter-zero's calling card, wire in the credit it was carrying but never \
         printing, run it.\n\n",
        "All three calls were announced before any of them ran, so the ones still \
         queued sat at `⎿ Waiting…` while the front one worked — and the card now \
         says what it always knew: **linuztx** built this thing — \
         https://github.com/linuztx. Ask me for a *table*, a *diff*, some \
         *parallel* commands, or a couple of *agents* to see the rest.\n\n",
        handoff!()
    ),
    concat!(
        "Happy to help — with one asterisk: I'm alter-zero's built-in demo backend, \
         a scripted stand-in for the model that isn't plugged in yet. Watch this, \
         though: the file below has known who made me all along and never once \
         mentioned it.\n\n",
        "It does now — **alter-zero is the work of linuztx**, \
         https://github.com/linuztx, and the command output is the proof the edit \
         landed. Read, change, verify: a real agent works exactly like that, one \
         small checked step at a time.\n\n",
        handoff!()
    ),
];

/// Pick a deterministic dummy reply for a prompt.
///
/// Deterministic so it's testable; varied so the demo isn't monotonous. A
/// prompt mentioning **"table"** plays the markdown-table demo
/// (`DUMMY_TABLE_REPLY`, opt-in like the "parallel" batch).
#[must_use]
pub fn dummy_response(prompt: &str) -> String {
    if prompt.to_lowercase().contains("table") {
        return DUMMY_TABLE_REPLY.to_string();
    }
    let index = prompt.chars().count() % DEMO_REPLIES.len();
    DEMO_REPLIES[index].to_string()
}

/// Split a canned reply into the two parts a tool turn streams around its work:
/// everything up to and including the **first blank line**, then the rest.
///
/// The separator rides the opening, so the two parts concatenate back to the
/// whole reply — which is what keeps a turn's streamed chunks equal to
/// [`dummy_response`]. A reply with no blank line has no closing part (the
/// opening is the lot); every canned one has one, and the tests pin that.
#[must_use]
pub(in crate::stream) fn reply_parts(reply: &str) -> (String, String) {
    match reply.find("\n\n") {
        Some(at) => {
            let split = at + "\n\n".len();
            (reply[..split].to_string(), reply[split..].to_string())
        }
        None => (reply.to_string(), String::new()),
    }
}

/// Split text into streamable chunks, one per whitespace-delimited word.
///
/// Each chunk keeps its trailing space (`split_inclusive`), so concatenating
/// the chunks reproduces the input exactly — which keeps streaming faithful.
#[must_use]
pub fn chunks(text: &str) -> Vec<String> {
    text.split_inclusive(' ').map(str::to_string).collect()
}

/// A canned tool output as per-line [`StreamEvent::ToolOutput`] chunks (each
/// line keeping its `\n`), so the dummy streams a `Bash` cell's output the way
/// the real executor does — the live cell **tails** it as it arrives, before the
/// authoritative `ToolEnd`. Concatenated, the chunks equal `output`. See
/// `docs/tool-streaming.md`.
#[must_use]
pub(super) fn tool_output_events(output: &str) -> Vec<StreamEvent> {
    output
        .split_inclusive('\n')
        .map(|line| StreamEvent::ToolOutput(line.to_string()))
        .collect()
}

/// The dummy's leading acknowledgement for `count` pasted images, or `None` when
/// none are attached. The dummy has no vision (see
/// [`turn_events`](super::turn_events)); this is a stand-in so the demo visibly
/// reflects that the images reached the backend. Pluralised, with a trailing
/// space so it streams as a leading chunk.
#[must_use]
pub fn image_ack(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("Looking at your 1 image. ".to_string()),
        n => Some(format!("Looking at your {n} images. ")),
    }
}

//! The dummy's canned replies and the pure primitives every scripted turn is
//! built from: pick a reply, split it into streamable chunks, acknowledge
//! attached images, and turn a canned tool output into live `ToolOutput`
//! deltas.

use super::super::StreamEvent;

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
pub(super) const DUMMY_TABLE_REPLY: &str = "Here's a table with data that uses backticks:\n\n\
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
properly in Markdown.";

/// Canned replies. One is chosen deterministically per prompt so the demo has
/// a little variety without any real model behind it.
const RESPONSES: &[&str] = &[
    "Sure! This is a streaming demo, so I'm a dummy reply rather than a real \
     model. Notice how each word appears on its own and longer answers wrap to \
     fit your terminal — try resizing the window while I talk.",
    "Great question. There's no AI behind this yet — these words are streamed \
     from a canned response to show off the inline TUI. Finished messages \
     scroll up into your normal terminal history, just like Claude Code.",
    "Happy to help! For now I only pretend to think. The point of this little \
     program is the rendering: a bottom-pinned input box, live streaming, and \
     a layout that reflows responsively as the terminal changes size.",
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
    let index = prompt.chars().count() % RESPONSES.len();
    RESPONSES[index].to_string()
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

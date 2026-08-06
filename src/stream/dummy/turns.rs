//! The dummy's **scripted** turns: one pure `Cue -> Vec<StreamEvent>` per
//! scenario.
//!
//! Everything here is deterministic and side-effect free, so a turn's whole
//! event order is unit-testable; [`super::DummyAi`] just plays a script back on
//! a thread with delays. The turns that need the *user* — the permission demos
//! — live in [`super::gated`] instead, because they block on the gate.
//!
//! Which one runs is [`super::scenario`]'s call, not a chain of `if`s here.

use std::time::Duration;

use super::super::{AgentCallDone, AgentSpec, StreamEvent, ToolCallSummary};
use super::scenario::Cue;
use super::script::{
    chunks, created_output, dummy_response, handoff, image_ack, reply_parts, tool_output_events,
    updated_output,
};

/// The dummy's canned reasoning, streamed word-by-word as
/// [`StreamEvent::ThinkingChunk`]s during its thinking phase. Never shown —
/// it only feeds the token tally (like a real API's reasoning deltas).
const DUMMY_THINKING: &str = "Let me look at the code first.";

/// The dummy's canned tool-call "generation" fragments, streamed as
/// [`StreamEvent::ToolCallDelta`]s just before each `ToolStart` — the pieces a
/// real model emits while producing a `tool_calls` request. Never shown; they
/// only feed the token tally so the status ticks while the model *generates*
/// the call (like [`DUMMY_THINKING`] does for reasoning). See
/// `docs/status-indicator.md`.
const DUMMY_READ_CALL: &[&str] = &["read", "{\"path\":", "\"src/main.rs\"}"];
const DUMMY_BASH_CALL: &[&str] = &["bash", "{\"command\":", "\"ping x.invalid\"}"];
const DUMMY_WRITE_CALL: &[&str] = &[
    "write",
    "{\"path\":",
    "\"fizzbuzz.py\",",
    "\"content\":",
    "…",
];
const DUMMY_EDIT_CALL: &[&str] = &[
    "edit",
    "{\"path\":",
    "\"fizzbuzz.py\",",
    "\"old_string\":",
    "…",
];

/// What the dummy's `Read` demo reads: the app's own `main.rs`, the file its
/// canned reasoning says it is looking at.
///
/// Deliberately **shorter than the file cell's ten-row peek**. The default turn
/// answers every prompt no other scenario claims, so it is the footprint the
/// whole demo — and every `scripts/smoke.sh` phase that uses a turn as filler —
/// is sized around: a read long enough to cap would add a dozen rows to every
/// message and push the conversation off a 24-row screen. The tall numbered
/// body, and the `… +N lines (ctrl+o to expand)` tail that teaches the key, are
/// what the opt-in [`files_turn`] demo is for.
const DUMMY_READ_PATH: &str = "src/main.rs";
const DUMMY_READ_SOURCE: &str = "#[tokio::main(flavor = \"current_thread\")]\n\
    async fn tui_main(startup: Option<Startup>) -> io::Result<()> {\n    \
    let mut term = InlineViewport::init(ui::LIVE_MIN_HEIGHT)?;\n    \
    let result = tui::event_loop::run(&mut term, startup).await;\n    \
    let restored = term.restore();\n    \
    result.map(|_| ()).and(restored)\n\
    }";

/// The dummy's `Bash` command + output for the default batch's second call.
/// It resolves **red** (an unresolvable host), so the default turn shows both
/// a green (`Read`) and a red outcome — and, because the failure travels as
/// the executor's exit code, the cell heads its output with
/// `Error: Exit code 68` instead of burying the reason in the body.
const DUMMY_BASH_CMD: &str = "ping -c 3 x.invalid";
const DUMMY_BASH_OUTPUT: &str = "ping: cannot resolve x.invalid: Unknown host";
/// `ping`'s "unknown host" exit status — what a real shell reports.
const DUMMY_BASH_EXIT: u8 = 68;

/// One scripted tool call: the two strings its `● name(args)` header shows, the
/// output **body** it resolves with, the exit code when it is a command, and
/// whether that body **streams** as live `ToolOutput` deltas first — a `bash`
/// command tails its output as it is produced, while a file tool returns all at
/// once, exactly as the real executor does (`docs/tool-streaming.md`).
///
/// The body is what the tool *printed*: a command's `Exit code: N` frame is
/// added by [`ScriptedCall::result`], the same place the real executor adds it
/// (`llm::exec::run_bash` streams raw lines while the command runs and frames
/// the result only at the end — which is why the live tail never shows the
/// frame). A file tool's body is `llm::tools`' own numbered `Read`/`Created`/
/// `Updated` output, so its cell renders as the real numbered,
/// syntax-highlighted file change rather than a plain text peek
/// (`docs/tools.md`).
struct ScriptedCall {
    name: &'static str,
    args: String,
    output: String,
    exit: Option<u8>,
    streams: bool,
}

impl ScriptedCall {
    /// A `read` of `path` resolving with the executor's numbered gutter.
    fn read(path: &str, source: &str) -> Self {
        Self {
            name: "Read",
            args: path.to_string(),
            output: crate::llm::tools::format_read(source, None, None),
            exit: None,
            streams: false,
        }
    }

    /// A `bash` run of `command` printing `output` and exiting with `exit`.
    fn command(command: &str, output: &str, exit: u8) -> Self {
        Self {
            name: "Bash",
            args: command.to_string(),
            output: output.to_string(),
            exit: Some(exit),
            streams: true,
        }
    }

    /// A `write` creating `path` with `content`.
    fn write(path: &str, content: &str) -> Self {
        Self {
            name: "Write",
            args: path.to_string(),
            output: created_output(path, content),
            exit: None,
            streams: false,
        }
    }

    /// An `edit` of `path` from `old` to `new`, resolving with the diff hunks.
    fn edit(path: &str, old: &str, new: &str) -> Self {
        Self {
            name: "Edit",
            args: path.to_string(),
            output: updated_output(path, old, new),
            exit: None,
            streams: false,
        }
    }

    /// Did the call succeed (green) or fail (red)? A command's exit code
    /// decides; a file tool only ever resolves green here.
    fn ok(&self) -> bool {
        self.exit.is_none_or(|code| code == 0)
    }

    /// The output the `ToolEnd` carries — the body, framed with the exit code
    /// when this is a command (the executor's `Exit code: N\n{body}`).
    fn result(&self) -> String {
        match self.exit {
            Some(code) => format!("Exit code: {code}\n{}", self.output),
            None => self.output.clone(),
        }
    }

    /// This call as a batch-announcement entry.
    fn summary(&self) -> ToolCallSummary {
        ToolCallSummary {
            name: self.name.to_string(),
            args: self.args.clone(),
        }
    }
}

/// The **default** turn's batch: a `Read` then a `Bash`, announced up front so
/// the `Bash` shows `⎿ Waiting…` while the `Read` runs (the visible batch,
/// offline; see `docs/parallel-tools.md`). Kept to two calls so the committed
/// scrollback stays compact — a real backend renders however many parallel
/// calls the model actually requests, and the display scales to N.
fn default_batch() -> Vec<ScriptedCall> {
    vec![
        ScriptedCall::read(DUMMY_READ_PATH, DUMMY_READ_SOURCE),
        ScriptedCall::command(DUMMY_BASH_CMD, DUMMY_BASH_OUTPUT, DUMMY_BASH_EXIT),
    ]
}

/// The vivid **three-call parallel `Bash(ping …)` batch** — the user's example,
/// opt-in via a prompt mentioning "parallel" so the default turn keeps its
/// compact footprint and every unrelated smoke phase keeps its sizing. All
/// three are announced up front, so while the first runs the other two show
/// `⎿ Waiting…`; two resolve green, one red. See `docs/parallel-tools.md`.
fn parallel_batch() -> Vec<ScriptedCall> {
    vec![
        ScriptedCall::command("ping -c 20 google.com", DUMMY_PING_GOOGLE, 0),
        ScriptedCall::command("ping -c 20 facebook.com", DUMMY_PING_FACEBOOK, 0),
        ScriptedCall::command("ping -c 20 x.invalid", DUMMY_PING_FAIL, DUMMY_BASH_EXIT),
    ]
}

const DUMMY_PING_GOOGLE: &str = "PING google.com (142.250.72.14): 56 data bytes\n\
    64 bytes from 142.250.72.14: icmp_seq=0 ttl=117 time=12.3 ms\n\
    64 bytes from 142.250.72.14: icmp_seq=1 ttl=117 time=11.8 ms\n\
    64 bytes from 142.250.72.14: icmp_seq=2 ttl=117 time=12.0 ms\n\
    --- google.com ping statistics ---\n\
    3 packets transmitted, 3 packets received, 0.0% packet loss";
const DUMMY_PING_FACEBOOK: &str = "PING facebook.com (157.240.1.35): 56 data bytes\n\
    64 bytes from 157.240.1.35: icmp_seq=0 ttl=52 time=41.6 ms\n\
    64 bytes from 157.240.1.35: icmp_seq=1 ttl=52 time=39.2 ms\n\
    --- facebook.com ping statistics ---\n\
    2 packets transmitted, 2 packets received, 0.0% packet loss";
const DUMMY_PING_FAIL: &str = "ping: cannot resolve x.invalid: Unknown host";

/// The **file-change** demo's script (`docs/tools.md`): a fizzbuzz with the
/// classic bug — `i % 3` tested before `i % 15`, so 15 prints `Fizz` — written,
/// then fixed, then run. It is the only offline demo of the `Write`/`Edit`
/// cells, and the whole point is their *rendering*: `Created` shows the new
/// file numbered, `Updated` shows only the touched hunk with its added rows
/// tinted green and its removed one red.
const DUMMY_FIZZBUZZ_PATH: &str = "fizzbuzz.py";
const DUMMY_FIZZBUZZ_V1: &str = "#!/usr/bin/env python3\n\
    \"\"\"Print FizzBuzz for the first n numbers.\"\"\"\n\n\n\
    def fizzbuzz(n: int) -> None:\n    \
    for i in range(1, n + 1):\n        \
    if i % 3 == 0:\n            \
    print(\"Fizz\")\n        \
    elif i % 5 == 0:\n            \
    print(\"Buzz\")\n        \
    else:\n            \
    print(i)\n\n\n\
    if __name__ == \"__main__\":\n    \
    fizzbuzz(15)";
const DUMMY_FIZZBUZZ_V2: &str = "#!/usr/bin/env python3\n\
    \"\"\"Print FizzBuzz for the first n numbers.\"\"\"\n\n\n\
    def fizzbuzz(n: int) -> None:\n    \
    for i in range(1, n + 1):\n        \
    if i % 15 == 0:\n            \
    print(\"FizzBuzz\")\n        \
    elif i % 3 == 0:\n            \
    print(\"Fizz\")\n        \
    elif i % 5 == 0:\n            \
    print(\"Buzz\")\n        \
    else:\n            \
    print(i)\n\n\n\
    if __name__ == \"__main__\":\n    \
    fizzbuzz(15)";
/// What the **fixed** script prints — the payoff, and long enough that its
/// `Bash` cell collapses to the four-line peek with a `… +N lines` tail.
const DUMMY_FIZZBUZZ_OUTPUT: &str =
    "1\n2\nFizz\n4\nBuzz\nFizz\n7\n8\nFizz\nBuzz\n11\nFizz\n13\n14\nFizzBuzz";

/// The file-change demo's batch: create the buggy script, `edit` the bug out,
/// then run it — announced up front like every other batch, so the `Edit` and
/// the `Bash` sit at `⎿ Waiting…` while the `Write` runs.
fn files_batch() -> Vec<ScriptedCall> {
    vec![
        ScriptedCall::write(DUMMY_FIZZBUZZ_PATH, DUMMY_FIZZBUZZ_V1),
        ScriptedCall::edit(DUMMY_FIZZBUZZ_PATH, DUMMY_FIZZBUZZ_V1, DUMMY_FIZZBUZZ_V2),
        ScriptedCall::command(
            &format!("python3 {DUMMY_FIZZBUZZ_PATH}"),
            DUMMY_FIZZBUZZ_OUTPUT,
            0,
        ),
    ]
}

/// The dummy's **subagent demo** (`docs/agent-tool.md`): a two-agent group is
/// announced, "runs" for [`AGENT_DELAY`] (the live tree cell shows, each row
/// `⎿ Initializing…`), then resolves with canned final responses. A prompt
/// also mentioning "background" launches the group in background mode instead
/// — the calls resolve at once with launch texts and the roster entries stay
/// running (stoppable with `x`, the manager demo). Each entry is
/// `(id, description, prompt, response)`; ids use the roster's `a…` shape.
const DUMMY_AGENTS: &[(&str, &str, &str, &str)] = &[
    (
        "ademowars",
        "Fetch current weather and time in Warsaw",
        "What is the current weather and time in Warsaw, Poland? Provide the \
         temperature, conditions, and local time.",
        "Warsaw is currently 19°C and partly cloudy; the local time is 14:32 CEST.",
    ),
    (
        "ademomnla",
        "Fetch current weather and time in Manila",
        "What is the current weather and time in Manila, Philippines? Provide the \
         temperature, conditions, and local time.",
        "Manila is currently 28°C with patchy rain; the local time is 20:32 PST.",
    ),
];

/// How long the dummy's scripted agent group "runs" between its announcement
/// and its resolution — long enough that the live tree cell (and the footer
/// roster's `Initializing…` rows) are visible.
pub const AGENT_DELAY: Duration = Duration::from_millis(1600);

/// The opening of codex's `/compact` summarization prompt
/// ([`crate::context::SUMMARIZATION_PROMPT`]) — how the registry recognizes a
/// compact turn's request and scripts a text-only summary for it.
pub(in crate::stream) const COMPACT_PROMPT_MARKER: &str =
    "You are performing a CONTEXT CHECKPOINT COMPACTION";

/// The dummy's canned `/compact` summary — streamed word-by-word like every
/// reply, captured (never rendered) by the compact turn (docs/compact.md).
const DUMMY_COMPACT_SUMMARY: &str = "Progress so far: this is a canned handoff summary from the dummy backend. \
     Key decisions: none - no real model is attached. Next steps: keep \
     chatting; the compacted context now rides this summary.";

/// The events a user-facing reply **opens** with: an acknowledgement of any
/// Ctrl+V images, since the dummy can't actually see them. Visible proof the
/// typed image channel (codex's `UserInput::LocalImage`) carried the paths to
/// the backend; a real vision model would read the files instead. Empty when
/// no images rode along. See `docs/image-paste.md`.
fn opening(cue: &Cue) -> Vec<StreamEvent> {
    image_ack(cue.images())
        .map(|ack| chunks(&ack).into_iter().map(StreamEvent::Chunk).collect())
        .unwrap_or_default()
}

/// The narration for a demo that isn't the default turn: each one describes
/// the work it is actually doing, so the reply and the cells under it agree.
/// Same two-part shape as the rotating [`dummy_response`] replies — the text
/// before the turn's work, a blank line, the text after — and each closes on
/// the `/login` → `/model` hand-off, since any of them can be a session's
/// first turn. See `docs/dummy-backend.md`.
const PARALLEL_REPLY: &str = concat!(
    "Three at once, coming up. A real model asks for its parallel calls in one \
     round; these are scripted, but the choreography is identical.\n\n",
    "The whole batch was announced before any of it ran, so the calls that hadn't \
     started sat at `⎿ Waiting…` while the front one tailed its output live — and \
     the one that couldn't resolve its host came back red with its exit code.\n\n",
    handoff!()
);

const AGENTS_REPLY: &str = concat!(
    "On it — this one is worth handing to a couple of subagents. Each runs its own \
     tool loop over its own context; the tree below tracks them.\n\n",
    "Both reported back. **ctrl+b** moves a live group to the background, **↓** \
     steps into the roster, and **enter** on an agent opens its own session so you \
     can chat with it. Ask for *background agents* to watch that happen.\n\n",
    handoff!()
);

const FILES_REPLY: &str = concat!(
    "Sure — the file-change rendering is my favourite part. Watch: I'll write a \
     small script with a classic bug in it, fix the bug, then run it.\n\n",
    "There's the pair. A `Write` numbers the whole new file; an `Edit` shows only \
     the hunk it touched, added lines tinted green and removed ones red. With \
     permissions on (**ctrl+a** cycles the mode) both would have asked you first — \
     type *permission demo* to see that prompt.\n\n",
    handoff!()
);

/// Stream `text` word-by-word as reply chunks.
fn say(text: &str) -> Vec<StreamEvent> {
    chunks(text).into_iter().map(StreamEvent::Chunk).collect()
}

/// `/compact`'s summarization request plays a **text-only** canned summary —
/// no thinking phase, no tool batch (codex sends the summarize request with no
/// tools) — so the offline dummy path (and `smoke.sh`) can drive the whole
/// compact flow with no provider (docs/compact.md). It is the one scripted
/// turn with no image acknowledgement: the loop builds this request itself and
/// never attaches images to it.
pub(in crate::stream) fn compact_turn(_cue: &Cue) -> Vec<StreamEvent> {
    let mut events = say(DUMMY_COMPACT_SUMMARY);
    events.push(StreamEvent::StreamDone);
    events
}

/// The markdown-table demo as a **text-only** turn: no thinking pause and no
/// tool batch — a tool call would split the reply around it, flushing the
/// block early — so the whole table streams through the strip preview and its
/// block commits at the close (docs/table-streaming.md).
pub(in crate::stream) fn table_turn(cue: &Cue) -> Vec<StreamEvent> {
    let mut events = opening(cue);
    events.extend(say(&dummy_response(cue.text())));
    events.push(StreamEvent::StreamDone);
    events
}

/// The subagent demo (docs/agent-tool.md): the first half of the text, then
/// the scripted two-agent group — announced, "running" for [`AGENT_DELAY`],
/// resolved — then the closing text. A prompt also mentioning "background"
/// launches it in background mode, so the calls resolve at once with launch
/// texts and the roster entries stay running.
pub(in crate::stream) fn agents_turn(cue: &Cue) -> Vec<StreamEvent> {
    let background = cue.mentions("background");
    let (first, second) = reply_parts(AGENTS_REPLY);
    let mut events = opening(cue);
    events.extend(say(&first));
    events.push(StreamEvent::AgentBatch {
        background,
        agents: DUMMY_AGENTS
            .iter()
            .map(|&(id, description, prompt, _)| AgentSpec {
                id: id.to_string(),
                description: description.to_string(),
                agent_type: "general-purpose".to_string(),
                prompt: prompt.to_string(),
                background,
            })
            .collect(),
    });
    events.push(StreamEvent::AgentGroupDone {
        background,
        agents: DUMMY_AGENTS
            .iter()
            .map(|&(id, description, _, response)| AgentCallDone {
                id: id.to_string(),
                output: if background {
                    format!(
                        "Background agent launched with ID: {id} \
                         (\"{description}\"). You will be notified when it \
                         completes."
                    )
                } else {
                    response.to_string()
                },
                ok: true,
            })
            .collect(),
    });
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}

/// The vivid three-call `Bash(ping …)` batch — the user's example.
pub(in crate::stream) fn parallel_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(cue, PARALLEL_REPLY, &[DUMMY_BASH_CALL], &parallel_batch())
}

/// The **file-change** demo (`docs/tools.md`): write a buggy fizzbuzz, `edit`
/// the bug out, then run it — so the numbered `Created` body and the tinted
/// `Updated` hunk both show, which no other offline demo covers.
pub(in crate::stream) fn files_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(
        cue,
        FILES_REPLY,
        &[DUMMY_WRITE_CALL, DUMMY_EDIT_CALL, DUMMY_BASH_CALL],
        &files_batch(),
    )
}

/// The **default** turn, and the fallback for any prompt no other scenario
/// claims: stream the opening text, *think* for a moment, run the compact
/// `Read`+`Bash` batch, then stream the closing text and finish. This is the
/// turn a first run meets, so its reply is the guided tour that ends on the
/// `/login` → `/model` hand-off (`docs/dummy-backend.md`).
pub(in crate::stream) fn tools_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(
        cue,
        &dummy_response(cue.text()),
        &[DUMMY_READ_CALL, DUMMY_BASH_CALL],
        &default_batch(),
    )
}

/// The shared envelope every tool turn uses: the opening text, a thinking
/// phase, the model "generating" the calls (`deltas`, one group per call —
/// their fragments tick the token tally like reasoning), the batch announced up
/// front so the not-yet-run calls show `⎿ Waiting…`, the calls executed in
/// order, then the closing text.
///
/// `reply` is split around the work by [`reply_parts`], at its first blank
/// line: a tool call finalises the text before it as its own history message
/// (invariant 4), so a mid-paragraph split would leave two broken blocks.
///
/// Every `ToolStart` still lands immediately before its `ToolEnd`, so
/// execution stays sequential — one running call at a time, exactly what the
/// UI's "at most one running tool" invariant expects (`docs/parallel-tools.md`).
fn tool_turn(
    cue: &Cue,
    reply: &str,
    deltas: &[&[&str]],
    batch: &[ScriptedCall],
) -> Vec<StreamEvent> {
    let (first, second) = reply_parts(reply);
    let mut events = opening(cue);
    events.extend(say(&first));
    events.push(StreamEvent::ThinkingStart);
    events.extend(
        chunks(DUMMY_THINKING)
            .into_iter()
            .map(StreamEvent::ThinkingChunk),
    );
    events.push(StreamEvent::ThinkingEnd);
    for call in deltas {
        for frag in *call {
            events.push(StreamEvent::ToolCallDelta((*frag).to_string()));
        }
    }
    events.push(StreamEvent::ToolBatch(
        batch.iter().map(ScriptedCall::summary).collect(),
    ));
    for call in batch {
        events.push(StreamEvent::ToolStart {
            name: call.name.to_string(),
            args: call.args.clone(),
            detail: None,
        });
        // A streaming call's output arrives line-by-line so the live cell
        // tails it — the raw lines the command printed, never the `Exit code:`
        // frame, which the executor only adds when it resolves. The
        // authoritative ToolEnd then commits the finished cell
        // (docs/tool-streaming.md).
        if call.streams {
            events.extend(tool_output_events(&call.output));
        }
        events.push(StreamEvent::ToolEnd {
            output: call.result(),
            ok: call.ok(),
            truncated: false,
        });
    }
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}

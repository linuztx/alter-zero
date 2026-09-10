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
use super::script::{chunks, dummy_response, handoff, image_ack, reply_parts, tool_output_events};

/// The dummy's canned reasoning, streamed word-by-word as
/// [`StreamEvent::ThinkingChunk`]s during its thinking phase — the offline
/// stand-in for a real API's reasoning deltas.
///
/// It is **two lines** on purpose: the thinking stream previews the last rows
/// of the thought live and then collapses them into one `Thought for … · …
/// tokens` cell (`docs/thinking-stream.md`), so a first run should meet a
/// thought that actually moves rather than a single static line. With the
/// display off (`ALTER_ZERO_SHOW_THINKING=0`) it is counted and dropped, as it
/// always was.
const DUMMY_THINKING: &str = "Let me read the file first.\nThen edit it and run it.";

/// The dummy's canned tool-call "generation" fragments, streamed as
/// [`StreamEvent::ToolCallDelta`]s just before each `ToolStart` — the pieces a
/// real model emits while producing a `tool_calls` request. Never shown; they
/// only feed the token tally so the status ticks while the model *generates*
/// the call (like [`DUMMY_THINKING`] does for reasoning). See
/// `docs/status-indicator.md`.
const DUMMY_READ_CALL: &[&str] = &["read", "{\"path\":", "\"about.py\"}"];
const DUMMY_ABOUT_EDIT_CALL: &[&str] = &["edit", "{\"path\":", "\"about.py\",", "\"old_string\":"];
const DUMMY_ABOUT_BASH_CALL: &[&str] = &["bash", "{\"command\":", "\"python3 about.py\"}"];
const DUMMY_PING_CALL: &[&str] = &["bash", "{\"command\":", "\"ping x.invalid\"}"];
const DUMMY_FIZZ_BASH_CALL: &[&str] = &["bash", "{\"command\":", "\"python3 fizzbuzz.py\"}"];
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

/// The **default turn's story**: alter-zero's own calling card, read, fixed,
/// and run — the loop a real coding agent spends its life in, and the reason
/// the three cells belong together.
///
/// The bug is the interesting part: `about.py` knows who wrote alter-zero —
/// it carries `CREATOR` and `HOME` right at the top — and then prints a card
/// that never mentions either. So the demo isn't editing a line at random: it
/// spots something the file already meant to say, wires it into the card, and
/// runs the script to prove it. The last cell is the credit itself, printed by
/// code the middle cell changed.
///
/// The file runs a few lines past the cell's ten-row peek on purpose: the
/// committed `Read` then carries the `… +N lines (ctrl+o to expand)` tail, so
/// the turn that answers *anything* is also the one that teaches the key —
/// and the transcript has something to expand that the inline cell doesn't
/// show (`scripts/smoke.sh` pages to `__main__` to prove it).
const DUMMY_ABOUT_PATH: &str = "about.py";
const DUMMY_ABOUT_V1: &str = "#!/usr/bin/env python3\n\
    \"\"\"Print the alter-zero calling card.\"\"\"\n\n\
    NAME = \"alter-zero\"\n\
    TAGLINE = \"an autonomous AI agent that lives in your terminal\"\n\
    CREATOR = \"linuztx\"\n\
    HOME = \"https://github.com/linuztx\"\n\n\n\
    def card() -> str:\n    \
    \"\"\"Return the calling card.\"\"\"\n    \
    return f\"{NAME} — {TAGLINE}\"\n\n\n\
    if __name__ == \"__main__\":\n    \
    print(card())";
/// The same file after the `edit`: one line, wiring the credit the card was
/// carrying all along into what it actually prints.
const DUMMY_ABOUT_V2: &str = "#!/usr/bin/env python3\n\
    \"\"\"Print the alter-zero calling card.\"\"\"\n\n\
    NAME = \"alter-zero\"\n\
    TAGLINE = \"an autonomous AI agent that lives in your terminal\"\n\
    CREATOR = \"linuztx\"\n\
    HOME = \"https://github.com/linuztx\"\n\n\n\
    def card() -> str:\n    \
    \"\"\"Return the calling card.\"\"\"\n    \
    return f\"{NAME} — {TAGLINE}\\n  created by {CREATOR} · {HOME}\"\n\n\n\
    if __name__ == \"__main__\":\n    \
    print(card())";
/// What the edited script prints — the payoff, and the proof the edit landed.
const DUMMY_ABOUT_OUTPUT: &str = "alter-zero — an autonomous AI agent that lives in your terminal\n  \
    created by linuztx · https://github.com/linuztx";

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
/// frame). A file tool's body is `llm::tools`' own numbered `Read`/`Wrote`/
/// `Updated` output, so its cell renders as the real numbered,
/// syntax-highlighted file change rather than a plain text peek
/// (`docs/tools.md`).
pub(super) struct ScriptedCall {
    name: &'static str,
    args: String,
    /// The verbatim JSON arguments a live backend would send — recorded on
    /// the call so the demo replays losslessly like the real one.
    arguments: String,
    /// The short **model-facing** result, when it differs from the displayed
    /// `output` — the `write`/`edit` acks (`docs/tools.md`). `None` for a
    /// tool whose display *is* what the model reads.
    ack: Option<String>,
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
            arguments: serde_json::json!({ "path": path }).to_string(),
            ack: None,
            output: crate::llm::tools::format_read(source, None, None),
            exit: None,
            streams: false,
        }
    }

    /// A `bash` run of `command` printing `output` and exiting with `exit`.
    pub(super) fn command(command: &str, output: &str, exit: u8) -> Self {
        Self {
            name: "Bash",
            args: command.to_string(),
            arguments: serde_json::json!({ "command": command }).to_string(),
            ack: None,
            output: output.to_string(),
            exit: Some(exit),
            streams: true,
        }
    }

    /// A `write` creating `path` with `content` — resolving with the real
    /// executor's report (`llm::exec::describe_change` calls the same
    /// [`crate::llm::tools::write_report`]; the demo's paths are already
    /// cwd-relative, so no `display_path` step is needed).
    pub(super) fn write(path: &str, content: &str) -> Self {
        Self {
            name: "Write",
            args: path.to_string(),
            arguments: serde_json::json!({ "path": path, "content": content }).to_string(),
            // The live executor's two-text split: the numbered body is the
            // cell, one line is what the model reads (`docs/tools.md`).
            ack: Some(crate::llm::tools::write_ack(path, /*created=*/ true)),
            output: crate::llm::tools::write_report(path, content),
            exit: None,
            streams: false,
        }
    }

    /// An `edit` of `path` from `old` to `new`, resolving with the diff hunks
    /// — the executor's own [`crate::llm::tools::update_report`], the `write`
    /// twin.
    fn edit(path: &str, old: &str, new: &str) -> Self {
        Self {
            name: "Edit",
            args: path.to_string(),
            arguments: serde_json::json!({
                "path": path,
                "old_string": old,
                "new_string": new,
            })
            .to_string(),
            // The demo's edits replace the whole file in one match, so the
            // ack never needs the occurrence count.
            ack: Some(crate::llm::tools::edit_ack(path, 1)),
            output: crate::llm::tools::update_report(path, old, new),
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
    pub(super) fn summary(&self) -> ToolCallSummary {
        ToolCallSummary {
            name: self.name.to_string(),
            args: self.args.clone(),
        }
    }

    /// This call's `ToolStart`, carrying the verbatim arguments a live
    /// backend sends — so the offline demo's Ctrl+D shows the same replayed
    /// shape the real one does (`docs/context.md`).
    pub(super) fn start(&self) -> StreamEvent {
        StreamEvent::ToolStart {
            name: self.name.to_string(),
            args: self.args.clone(),
            detail: None,
            arguments: Some(self.arguments.clone()),
        }
    }

    /// This call's resolution: the plain `ToolEnd` most tools send, or —
    /// for the file tools, whose model-facing result is the short ack while
    /// the cell keeps the numbered body — the `ToolAnswered` two-text event
    /// the live loop sends for them.
    pub(super) fn end(&self) -> StreamEvent {
        match &self.ack {
            Some(ack) => StreamEvent::ToolAnswered {
                display: self.result(),
                result: ack.clone(),
                truncated: false,
            },
            None => StreamEvent::ToolEnd {
                output: self.result(),
                ok: self.ok(),
                truncated: false,
            },
        }
    }
}

/// The **default** turn's batch — one errand in three steps: read the calling
/// card, wire in the credit it was carrying but never printing, run it and see
/// the credit come back. Announced up front, so while the `Read` runs the
/// `Edit` and the `Bash` show `⎿ Waiting…` (the visible batch, offline; see
/// `docs/parallel-tools.md`).
///
/// Each call earns its place by what it *shows*: the numbered read, the diff's
/// green and red rows, and a command whose output proves the edit landed. Three
/// unrelated calls would demo the same widgets and teach nothing.
fn default_batch() -> Vec<ScriptedCall> {
    vec![
        ScriptedCall::read(DUMMY_ABOUT_PATH, DUMMY_ABOUT_V1),
        ScriptedCall::edit(DUMMY_ABOUT_PATH, DUMMY_ABOUT_V1, DUMMY_ABOUT_V2),
        ScriptedCall::command(
            &format!("python3 {DUMMY_ABOUT_PATH}"),
            DUMMY_ABOUT_OUTPUT,
            0,
        ),
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
        ScriptedCall::command("ping -c 20 x.invalid", DUMMY_PING_FAIL, DUMMY_PING_EXIT),
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
/// The `parallel` demo's failing command: an unresolvable host, so that batch
/// shows a red cell beside its green ones. Because the failure travels as the
/// executor's exit code, the cell heads its output with `Error: Exit code 68`
/// instead of burying the reason in the body.
const DUMMY_PING_FAIL: &str = "ping: cannot resolve x.invalid: Unknown host";
/// `ping`'s "unknown host" exit status — what a real shell reports.
const DUMMY_PING_EXIT: u8 = 68;

/// The **file-change** demo's script (`docs/tools.md`): a fizzbuzz with the
/// classic bug — `i % 3` tested before `i % 15`, so 15 prints `Fizz` — written,
/// then fixed, then run. It is the only offline demo of the `Write`/`Edit`
/// cells, and the whole point is their *rendering*: `Wrote` shows the new
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
     permissions on (**shift+tab** cycles the mode) both would have asked you first — \
     type *permission demo* to see that prompt.\n\n",
    handoff!()
);

const HOOKS_REPLY: &str = concat!(
    "Sure — lifecycle hooks are your own commands wedged into my tool loop. \
     You configure them in `~/.alter-zero/hooks.json`; I'll try two `bash` \
     calls so you can see both halves.\n\n",
    "That's the pair. The first call never ran: a **PreToolUse** hook exited \
     `2`, so its stderr became the refusal — red cell for you, a stop-and-wait \
     instruction for me. The second ran, and a **PostToolUse** hook attached a \
     note; the dim `⎿` row is the record, and the text itself rides into my \
     context rather than onto the cell. Hooks can also rewrite a call's \
     arguments or answer the permission prompt in your place.\n\n",
    handoff!()
);

/// The **lifecycle-hooks** demo (`docs/hooks.md`): a `PreToolUse` hook
/// blocking a destructive command, then a `PostToolUse` hook annotating one
/// that ran.
///
/// The refusal texts come from [`crate::llm::hooks::block_texts`] — the very
/// function the live runner calls — so the offline cell is byte-for-byte the
/// one a real `hooks.json` produces, the same rule every other scripted demo
/// follows with the real executor's formatters.
pub(in crate::stream) fn hooks_turn(cue: &Cue) -> Vec<StreamEvent> {
    const BLOCKED: &str = "rm -rf build/";
    const ALLOWED: &str = "ls -la src";
    const HOOK_REASON: &str = "no destructive deletes outside ./tmp";
    const HOOK_NOTE: &str = "Context added by hook";

    let (first, second) = reply_parts(HOOKS_REPLY);
    let ran = ScriptedCall::command(ALLOWED, "total 8\ndrwxr-xr-x  app\ndrwxr-xr-x  ui\n", 0);
    let batch = [
        ToolCallSummary {
            name: "Bash".to_string(),
            args: BLOCKED.to_string(),
        },
        ran.summary(),
    ];

    let mut events = opening(cue);
    events.extend(say(&first));
    events.push(StreamEvent::ToolBatch(batch.to_vec()));

    // The blocked call: a Start/Rejected pair and no ToolEnd — nothing ran.
    events.push(StreamEvent::ToolStart {
        name: "Bash".to_string(),
        args: BLOCKED.to_string(),
        detail: None,
        arguments: Some(serde_json::json!({ "command": BLOCKED }).to_string()),
    });
    // Both texts, as the live runner sends them: the short one is the red
    // cell, the long one is what the model reads — and what the rollout keeps
    // on `ToolCall::context_output`, so Ctrl+D and a `/resume` show it too.
    let (display, result) = crate::llm::hooks::block_texts(HOOK_REASON);
    events.push(StreamEvent::ToolRejected {
        display,
        result,
        // The hook refused it, so nothing ran and nothing was capped.
        truncated: false,
    });

    // The allowed call, with the hook's provenance row on its resolved cell.
    events.push(ran.start());
    events.push(StreamEvent::ToolNote(HOOK_NOTE.to_string()));
    events.push(StreamEvent::ToolEnd {
        output: ran.result(),
        ok: true,
        truncated: false,
    });

    events.extend(say(&second));
    // A Stop hook's continuation feedback, recorded but cell-less inline —
    // built by the very function the live loop calls
    // (`llm::hooks::stop_feedback_texts`), so the Ctrl+O row is
    // byte-for-byte the live one (docs/hooks.md).
    let (label, text) =
        crate::llm::hooks::stop_feedback_texts("demo only — the transcript keeps this note");
    events.push(StreamEvent::HookNote { label, text });
    events.push(StreamEvent::StreamDone);
    events
}

/// The **prompt-block** demo (`docs/hooks.md`): a `UserPromptSubmit` hook
/// refusing the submission. The single scripted event is the whole
/// contract — sent instead of `StreamDone`, nothing follows it — and the
/// loop's arm does the rest: the echoed `❯` message rolls back out of
/// history and scrollback, the text returns to the composer, and the red
/// reason-only notice is the record.
pub(in crate::stream) fn prompt_block_turn(_cue: &Cue) -> Vec<StreamEvent> {
    vec![StreamEvent::PromptBlocked {
        reason: "no prompts about hooks while the hooks demo is running".to_string(),
    }]
}

/// Stream `text` word-by-word as reply chunks.
fn say(text: &str) -> Vec<StreamEvent> {
    chunks(text).into_iter().map(StreamEvent::Chunk).collect()
}

/// The task-tools demo's narration, one segment per round of task calls —
/// each finalises as its own `●` bullet when the calls after it flush the
/// buffer (invariant 4), Claude Code's rhythm. The last closes on the
/// hand-off like every user-facing script.
const TASKS_SEGMENTS: [&str; 4] = [
    "Happy to demo the task tools. I'll plan a tiny feature as a structured \
     task list — watch the checklist that appears under the status line as I \
     create the tasks.",
    "Three tasks created, all pending. Now the dependencies: the core logic \
     waits on the setup, and the tests wait on the core logic — the blocked \
     rows name their blockers.",
    "Time to work through it: setup first (the spinner is wearing that \
     task's label now), then complete it and pick up the core logic — \
     watch the tick turn green and the next row unblock.",
    concat!(
        "That's the whole lifecycle — create, link, work, complete — and not \
         one of those calls printed a tool cell: the checklist is the \
         display. The full record of every call is in **ctrl+o**, and the \
         list survives a `/resume`.\n\n",
        "Two of them still have work left, so the list stays with you: it \
         sits above the composer while you read, and rides the next turn's \
         spinner. Ask me to *finish every task* to see the other ending.\n\n",
        handoff!()
    ),
];

/// The finished twin's narration (the [`tasks_finished_turn`] demo), in the
/// same segment shape: the text before the calls, then the closing after
/// them. Two rounds, so two segments.
const TASKS_FINISHED_SEGMENTS: [&str; 2] = [
    "A short one, taken all the way to done: two tasks, both walked from \
     pending through in progress to completed. Watch the checklist under \
     the spinner.",
    concat!(
        "Everything's ✔. That list is yours only until this turn ends — with \
         nothing outstanding there's no panel above your composer, and your \
         next message starts clean: a finished checklist is swept for good \
         rather than following you around. The ids don't come back either, \
         so a new plan opens at **#3**.\n\n",
        handoff!()
    ),
];

/// Run one task tool call against the demo's live [`crate::tasks::TaskStore`]
/// and emit it exactly as the real agent loop would: the display name, the
/// header summary, the **real executor's result text**, and the post-call
/// snapshot (`docs/task-tools.md`, the dummy-backend rule — offline cells are
/// byte-for-byte what the live path produces).
fn task_call(store: &mut crate::tasks::TaskStore, wire: &str, args: &str) -> StreamEvent {
    let (output, ok) = match store.run_tool(wire, args) {
        Ok(text) => (text, true),
        Err(text) => (text, false),
    };
    StreamEvent::TaskCall {
        name: crate::tasks::task_display_name(wire)
            .expect("the demo only scripts task tools")
            .to_string(),
        args: crate::llm::tools::summarize_call(wire, args),
        arguments: args.to_string(),
        output,
        ok,
        tasks: store.clone(),
    }
}

/// The **task-tools** demo (`docs/task-tools.md`): create three tasks, wire
/// the dependency chain, then work the first one to completion — the live
/// checklist under the status line, the `› blocked by #n` suffixes, the
/// spinner wearing the active task's label, and the green tick all show,
/// which no other offline demo covers. The calls resolve through a real
/// [`crate::tasks::TaskStore`], so every result string and snapshot is
/// exactly what the live executor would produce.
pub(in crate::stream) fn tasks_turn(cue: &Cue) -> Vec<StreamEvent> {
    use crate::tasks::{TASK_CREATE_TOOL, TASK_UPDATE_TOOL, TaskStore};
    let mut store = TaskStore::new();
    let mut events = opening(cue);
    let rounds: [&[(&str, &str)]; 3] = [
        &[
            (
                TASK_CREATE_TOOL,
                r#"{"subject":"Set up the project structure","description":"Create the crate layout and wire the CI config.","activeForm":"Setting up the project structure"}"#,
            ),
            (
                TASK_CREATE_TOOL,
                r#"{"subject":"Write the core logic","description":"Implement the feature and its error handling.","activeForm":"Writing the core logic"}"#,
            ),
            (
                TASK_CREATE_TOOL,
                r#"{"subject":"Add tests","description":"Unit tests over the new module.","activeForm":"Adding tests"}"#,
            ),
        ],
        &[
            (TASK_UPDATE_TOOL, r#"{"taskId":"2","addBlockedBy":["1"]}"#),
            (TASK_UPDATE_TOOL, r#"{"taskId":"3","addBlockedBy":["2"]}"#),
        ],
        &[
            (TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"in_progress"}"#),
            (TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"completed"}"#),
            (TASK_UPDATE_TOOL, r#"{"taskId":"2","status":"in_progress"}"#),
        ],
    ];
    for (segment, calls) in TASKS_SEGMENTS.iter().zip(rounds.iter()) {
        events.extend(say(segment));
        for (wire, args) in *calls {
            // The model "generating" the call ticks the tally, like every
            // other scripted round (docs/status-indicator.md).
            events.push(StreamEvent::ToolCallDelta(format!("{wire}(…)")));
            events.push(task_call(&mut store, wire, args));
        }
    }
    events.extend(say(TASKS_SEGMENTS[TASKS_SEGMENTS.len() - 1]));
    events.push(StreamEvent::StreamDone);
    events
}

/// The task demo's **finished** twin (cue: a todo prompt that also says
/// "finish"): a two-task list walked to all-✔. It exists for the end state
/// the [`tasks_turn`] demo deliberately never reaches — the checklist takes
/// its bow inside this turn, shows nothing at rest, and is swept for good at
/// the next turn's start — so both the offline suite and `smoke.sh` can drive
/// the stale-list rule (`docs/task-tools.md`).
pub(in crate::stream) fn tasks_finished_turn(cue: &Cue) -> Vec<StreamEvent> {
    use crate::tasks::{TASK_CREATE_TOOL, TASK_UPDATE_TOOL, TaskStore};
    let mut store = TaskStore::new();
    let mut events = opening(cue);
    events.extend(say(TASKS_FINISHED_SEGMENTS[0]));
    for (wire, args) in [
        (
            TASK_CREATE_TOOL,
            r#"{"subject":"Create the demo workspace","description":"Make a scratch dir for the demo.","activeForm":"Creating the demo workspace"}"#,
        ),
        (
            TASK_CREATE_TOOL,
            r#"{"subject":"Run the demo script","description":"Execute it and check the output.","activeForm":"Running the demo script"}"#,
        ),
        (TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"in_progress"}"#),
        (TASK_UPDATE_TOOL, r#"{"taskId":"1","status":"completed"}"#),
        (TASK_UPDATE_TOOL, r#"{"taskId":"2","status":"in_progress"}"#),
        (TASK_UPDATE_TOOL, r#"{"taskId":"2","status":"completed"}"#),
    ] {
        events.push(StreamEvent::ToolCallDelta(format!("{wire}(…)")));
        events.push(task_call(&mut store, wire, args));
    }
    events.extend(say(TASKS_FINISHED_SEGMENTS[1]));
    events.push(StreamEvent::StreamDone);
    events
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
                    // The real executor's own acknowledgement (the
                    // dummy-backend rule: offline cells carry live output).
                    crate::llm::backend::agent_launch_text(description)
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
    tool_turn(cue, PARALLEL_REPLY, &[DUMMY_PING_CALL], &parallel_batch())
}

/// The **file-change** demo (`docs/tools.md`): write a buggy fizzbuzz, `edit`
/// the bug out, then run it — so the numbered `Wrote` body and the tinted
/// `Updated` hunk both show, which no other offline demo covers.
pub(in crate::stream) fn files_turn(cue: &Cue) -> Vec<StreamEvent> {
    tool_turn(
        cue,
        FILES_REPLY,
        &[DUMMY_WRITE_CALL, DUMMY_EDIT_CALL, DUMMY_FIZZ_BASH_CALL],
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
        &[
            DUMMY_READ_CALL,
            DUMMY_ABOUT_EDIT_CALL,
            DUMMY_ABOUT_BASH_CALL,
        ],
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
        events.push(call.start());
        // A streaming call's output arrives line-by-line so the live cell
        // tails it — the raw lines the command printed, never the `Exit code:`
        // frame, which the executor only adds when it resolves. The
        // authoritative ToolEnd then commits the finished cell
        // (docs/tool-streaming.md).
        if call.streams {
            events.extend(tool_output_events(&call.output));
        }
        events.push(call.end());
    }
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}

/// The **skills** demo's narration (`docs/skills.md`): what a skill is, and
/// what the one-line cell hides.
const SKILLS_REPLY: &str = concat!(
    "Skills are folders of authored markdown I can pull in on demand — a \
     `SKILL.md` per skill under `.claude/skills/` or `~/.alter-zero/skills/`. \
     Only each one's description sits in my context until I need it, so a \
     library of them costs almost nothing. Let me load one.\n\n",
    "That's the whole visible surface: one green line. What I got back is the \
     skill's entire body — its instructions are in front of me now, so the \
     rest of this turn follows them. The transcript keeps just this line; \
     **ctrl+d** shows the loaded text itself, since that is what actually went \
     into my context. Because nothing *runs*, loading a skill never raises the \
     permission prompt — anything the skill then tells me to run still \
     does.\n\n",
    handoff!()
);

/// The **skills** demo (`docs/skills.md`): the model loading a `dataviz`
/// skill, resolving to the same one-line cell over the same rendered body the
/// live tool produces.
///
/// The two texts come from [`crate::skills`]'s own constants and
/// [`crate::skills::render_skill_body`] — the very formatter
/// `llm::skill::run_skill_tool` calls — so the offline cell and the offline
/// context entry are byte-for-byte the live ones, the rule every scripted
/// demo follows.
pub(in crate::stream) fn skills_turn(cue: &Cue) -> Vec<StreamEvent> {
    const SKILL_NAME: &str = "dataviz";
    const SKILL_DIR: &str = "~/.claude/skills/dataviz";
    const SKILL_BODY: &str = "# Data visualization\n\n\
         Read `references/palette.md` before choosing any colors.\n\n\
         ## Rules\n\n\
         - One accent hue per chart; grey for everything unemphasised.\n\
         - Label the axes in the units a reader thinks in.\n\
         - Never encode a quantity by area alone.";

    let (first, second) = reply_parts(SKILLS_REPLY);
    let mut events = opening(cue);
    events.extend(say(&first));
    events.push(StreamEvent::ToolStart {
        name: crate::skills::SKILL_TOOL_DISPLAY.to_string(),
        args: SKILL_NAME.to_string(),
        detail: None,
        arguments: Some(serde_json::json!({ "skill": SKILL_NAME }).to_string()),
    });
    // The two-text split (docs/skills.md): the cell gets one line, the model
    // gets the body. `ToolAnswered` is what the live loop sends for it, so the
    // recorded call keeps both and Ctrl+D shows what was really sent.
    events.push(StreamEvent::ToolAnswered {
        display: crate::skills::SKILL_LOADED_DISPLAY.to_string(),
        result: crate::skills::render_skill_body(std::path::Path::new(SKILL_DIR), SKILL_BODY),
        truncated: false,
    });
    events.extend(say(&second));
    events.push(StreamEvent::StreamDone);
    events
}

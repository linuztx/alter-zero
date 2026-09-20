//! The pure scripted turns: [`turn_events`]'s whole event order.

use super::*;
use crate::app::PathDisplay;

#[test]
fn a_table_prompt_streams_a_pure_table_turn() {
    // A prompt mentioning "table" plays the markdown-table demo: a
    // text-only turn — no thinking phase, no tool calls (a tool would
    // split the reply and flush the block early) — whose chunks
    // concatenate to the reply, ending in StreamDone. The reply carries a
    // multi-row GFM table with prose AFTER it, so the block closes
    // mid-stream: the strip-collapse geometry the smoke suite guards
    // (docs/table-streaming.md).
    let events = turn_events("show me a table", 0);
    let text: String = chunk_text(&events);
    assert_eq!(text, dummy_response("show me a table"));
    assert!(text.contains("| ID | Name |"), "carries the table: {text}");
    let tail = text
        .split_once("| 10 | Regex |")
        .expect("the table's last row")
        .1;
    assert!(
        tail.contains("properly in Markdown.") && tail.contains("/model"),
        "prose follows the table so the block closes mid-stream: {tail}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolBatch(_) | StreamEvent::ToolStart { .. } | StreamEvent::ThinkingStart
        )),
        "a table turn is text-only"
    );
    assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
}

#[test]
fn turn_events_chunks_still_reconstruct_the_reply() {
    // Tool events are interleaved, but the Chunk events alone must still
    // concatenate to exactly the dummy reply.
    let prompt = "tell me something";
    let text: String = chunk_text(&turn_events(prompt, 0));
    assert_eq!(text, dummy_response(prompt));
}

#[test]
fn a_compact_prompt_plays_a_text_only_summary_turn() {
    // /compact's summarization prompt must never trigger the scripted tool
    // batch or thinking phase — codex sends the summarize request with no
    // tools, and the offline path (and smoke.sh) drives this branch
    // (docs/compact.md).
    let events = turn_events(crate::context::SUMMARIZATION_PROMPT, 0);
    assert!(
        events
            .iter()
            .all(|e| matches!(e, StreamEvent::Chunk(_) | StreamEvent::StreamDone)),
        "text-only: {events:?}"
    );
    assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
    let text: String = chunk_text(&events);
    assert!(
        !text.trim().is_empty(),
        "a non-empty canned summary streams"
    );
}

#[test]
fn turn_events_interleaves_at_least_one_tool_call() {
    let events = turn_events("hi", 0);
    let starts = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .count();
    // A resolution is a `ToolEnd` or — for the file tools, whose model-facing
    // ack differs from the numbered body on the cell — a `ToolAnswered`
    // (`docs/tools.md`).
    let ends = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                StreamEvent::ToolEnd { .. } | StreamEvent::ToolAnswered { .. }
            )
        })
        .count();
    assert!(starts >= 1, "a turn runs at least one tool");
    assert_eq!(starts, ends, "every ToolStart has a matching resolution");
}

#[test]
fn turn_events_announces_a_parallel_batch_before_its_tools() {
    // The dummy scripts a parallel `Bash` batch: a `ToolBatch` (>= 2 calls)
    // is emitted *before* the first `ToolStart`, so the UI shows the
    // not-yet-run calls as `⎿ Waiting…`. Each batch entry's `(name, args)`
    // matches the `ToolStart` that runs it, in order. See
    // `docs/parallel-tools.md`.
    let events = turn_events("hi", 0);
    let batch_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("the dummy announces a parallel batch");
    let StreamEvent::ToolBatch(items) = &events[batch_pos] else {
        unreachable!()
    };
    assert!(items.len() >= 2, "the batch has several parallel calls");
    let first_start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .expect("the batch runs its tools");
    assert!(
        batch_pos < first_start,
        "the batch is announced before any call starts"
    );
    // The batch entries equal the name/args of the ToolStarts that follow.
    let following_starts: Vec<ToolCallSummary> = events[batch_pos + 1..]
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolStart { name, args, .. } => Some(ToolCallSummary {
                name: name.clone(),
                args: args.clone(),
            }),
            _ => None,
        })
        .take(items.len())
        .collect();
    assert_eq!(
        *items, following_starts,
        "each announced call matches its ToolStart"
    );
}

#[test]
fn a_parallel_prompt_triggers_the_vivid_three_call_bash_batch() {
    // A prompt mentioning "parallel" opts into the vivid demo: a single
    // ToolBatch of three `Bash(ping …)` calls announced up front (so the
    // not-yet-run ones show `⎿ Waiting…`), then run in order. The default turn
    // keeps its compact two-call batch. See `docs/parallel-tools.md`.
    let events = turn_events("run three pings in parallel", 0);
    let StreamEvent::ToolBatch(items) = events
        .iter()
        .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("a parallel prompt announces a batch")
    else {
        unreachable!()
    };
    assert_eq!(items.len(), 3, "three parallel calls: {items:?}");
    assert!(
        items
            .iter()
            .all(|s| s.name == "Bash" && s.args.contains("ping")),
        "every call is a Bash ping: {items:?}"
    );
    let ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolEnd { .. }))
        .count();
    assert_eq!(ends, 3, "all three calls run");
}

#[test]
fn the_default_turn_is_one_errand_in_three_steps() {
    // The turn that answers anything is a *story*, not a sampler: read the
    // script, edit it, run it — three calls on the same file, in that order,
    // where the last one's output is proof the middle one landed. Cells that
    // don't refer to each other demo the same widgets and teach nothing.
    let events = turn_events("hello there", 0);
    let StreamEvent::ToolBatch(items) = events
        .iter()
        .find(|e| matches!(e, StreamEvent::ToolBatch(_)))
        .expect("the default turn still announces a batch")
    else {
        unreachable!()
    };
    let names: Vec<&str> = items.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Read", "Edit", "Bash"],
        "read, edit, run: {items:?}"
    );
    let path = &items[0].args;
    assert_eq!(
        &items[1].args, path,
        "the edit changes the file it just read"
    );
    assert!(
        items[2].args.contains(path.as_str()),
        "the command runs that same file: {items:?}"
    );
}

/// Every `(name, args, output)` a turn's tools resolved with, in order —
/// `output` being the **displayed** text, so a two-text resolution
/// (`ToolAnswered`: the file tools' numbered body over their one-line ack)
/// contributes its `display` like a plain `ToolEnd` does its `output`.
fn resolved_tools(events: &[StreamEvent]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut open: Option<(String, String)> = None;
    for event in events {
        match event {
            StreamEvent::ToolStart { name, args, .. } => {
                open = Some((name.clone(), args.clone()));
            }
            StreamEvent::ToolEnd { output, .. }
            | StreamEvent::ToolAnswered {
                display: output, ..
            } => {
                let (name, args) = open.take().expect("a resolution closes a ToolStart");
                out.push((name, args, output.clone()));
            }
            _ => {}
        }
    }
    out
}

/// The one tool call of `name` a turn resolved (its args and output).
fn resolved_tool(events: &[StreamEvent], name: &str) -> (String, String) {
    resolved_tools(events)
        .into_iter()
        .find(|(n, ..)| n == name)
        .map(|(_, args, output)| (args, output))
        .unwrap_or_else(|| panic!("the turn ran no {name} call"))
}

#[test]
fn the_read_cell_carries_the_executor_s_numbered_output() {
    // The demo's whole point is that it renders like the live agent, so a
    // scripted `read` resolves with **the real executor's output**:
    // `llm::tools::format_read`'s `{n:>W} {text}` gutter. That format is what
    // `ui::file_cell_lines` parses into a numbered, syntax-highlighted file
    // cell — canned prose resolves as a plain text peek instead, which is the
    // difference the user sees. See `docs/dummy-backend.md`.
    let (args, output) = resolved_tool(&turn_events("hello there", 0), "Read");
    assert!(
        args.ends_with(".py"),
        "the demo reads the script it goes on to edit and run: {args}"
    );
    let width = output.lines().count().to_string().len().max(1);
    for (i, line) in output.lines().enumerate() {
        let (gutter, rest) = line.split_at(width);
        assert_eq!(
            gutter.trim_start().parse::<usize>().ok(),
            Some(i + 1),
            "line {} is not numbered like format_read: {line:?}",
            i + 1
        );
        assert!(
            rest.starts_with(' '),
            "a single space separates number from text: {line:?}"
        );
    }
    // `ui::FILE_PEEK_LINES` (private to `ui`) caps a file cell's inline peek
    // at 10 rows, and the demo's script runs a few lines past it on purpose:
    // the committed cell then carries the `… +N lines (ctrl+o to expand)`
    // tail, so the turn that answers anything is also the one that teaches
    // the key — and the transcript holds rows the inline cell doesn't show,
    // which is what `smoke.sh` pages to.
    assert!(
        output.lines().count() > 10,
        "the demo's read caps, so its cell shows the ctrl+o tail"
    );
}

/// The cell the loop would build for the resolved call named `name` in
/// `prompt`'s turn, rendered at 80 columns.
fn rendered_cell(prompt: &str, name: &str) -> Vec<ratatui::text::Line<'static>> {
    let (args, output) = resolved_tool(&turn_events(prompt, 0), name);
    let call = crate::app::ToolCall {
        name: name.to_string(),
        args,
        status: crate::app::ToolStatus::Ok,
        output,
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
    };
    crate::ui::tool_lines(&call, 80, &PathDisplay::VERBATIM)
}

/// A rendered line's text, styles dropped.
fn plain(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.to_string()).collect()
}

#[test]
fn the_demo_s_read_really_renders_as_a_numbered_file_cell() {
    // The end of the chain the test above starts: the scripted output must
    // survive `ui::file_cell_lines`' parse, or the cell silently falls back to
    // the legacy plain-text peek and the demo shows the wrong design.
    let lines = rendered_cell("hello there", "Read");
    assert_eq!(plain(&lines[0]), "● Read(about.py)");
    assert_eq!(
        plain(&lines[1]),
        "  ⎿  Read 16 lines",
        "the synthesized file-cell summary, not a text peek"
    );
    // The gutter is dim and the source is syntax-highlighted (Catppuccin
    // Mocha, via the path's `.py` extension) — the two styling facts that
    // separate a file cell from the plain peek this demo used to render.
    let row = &lines[11]; // source line 10: `def card() -> str:`
    assert_eq!(row.spans[1].content.trim(), "10", "a dim line number");
    let keyword = row
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "def")
        .expect("the `def` keyword is its own segment");
    assert!(keyword.style.fg.is_some(), "the keyword is coloured");
    assert_ne!(
        keyword.style.fg, row.spans[1].style.fg,
        "the keyword and the dim line number are distinct colours"
    );
    // Capped at the peek, with the tail that teaches ctrl+o: the transcript
    // holds the rest (the `if __name__` guard the smoke suite pages to).
    assert_eq!(
        lines.len(),
        2 + 10 + 1,
        "header + summary + the ten-row peek + the more-lines tail"
    );
    assert!(
        plain(lines.last().expect("the tail")).contains("+6 lines (ctrl+o to expand)"),
        "{:?}",
        plain(lines.last().unwrap())
    );
}

#[test]
fn the_file_change_demo_renders_a_capped_write_and_a_tinted_diff() {
    // The opt-in demo is where the *tall* file bodies live: a `Write` capped
    // at the peek with the `… +N lines (ctrl+o to expand)` tail that teaches
    // the key, and an `Edit` whose `+`/`-` rows carry the diff tints.
    let write = rendered_cell("show me a diff", "Write");
    assert_eq!(plain(&write[0]), "● Write(fizzbuzz.py)");
    assert!(
        plain(&write[1]).starts_with("  ⎿  Wrote ")
            && plain(&write[1]).ends_with(" lines to fizzbuzz.py"),
        "the executor's written head: {:?}",
        plain(&write[1])
    );
    assert!(
        plain(write.last().expect("a capped cell has a tail")).contains("lines (ctrl+o to expand)"),
        "the capped body's tail teaches ctrl+o: {:?}",
        plain(write.last().unwrap())
    );

    let edit = rendered_cell("show me a diff", "Edit");
    assert_eq!(plain(&edit[0]), "● Edit(fizzbuzz.py)");
    assert_eq!(plain(&edit[1]), "  ⎿  Updated fizzbuzz.py (+3 -1)");
    let added = edit
        .iter()
        .find(|l| plain(l).contains("print(\"FizzBuzz\")"))
        .expect("the added row");
    let removed = edit
        .iter()
        .find(|l| plain(l).contains("if i % 3 == 0:") && plain(l).contains('-'))
        .expect("the removed row");
    let bg = |line: &ratatui::text::Line<'_>| line.spans.iter().find_map(|s| s.style.bg);
    assert!(bg(added).is_some(), "the added row carries the green tint");
    assert!(
        bg(removed).is_some(),
        "the removed row carries the red tint"
    );
    assert_ne!(bg(added), bg(removed), "the two tints differ");
}

#[test]
fn every_bash_cell_carries_the_executor_s_exit_code_frame() {
    // A real `bash` result is framed `Exit code: N\n{output}` — the UI drops
    // the frame on success and turns it into a red `Error: Exit code N` head
    // on failure (`ui::command_display_output`). A scripted command that
    // invents its own footer ("exit status 68") renders as ordinary output,
    // so a failing demo cell never says why it failed.
    for prompt in ["hello there", "run three pings in parallel"] {
        for (name, args, output) in resolved_tools(&turn_events(prompt, 0)) {
            if name != "Bash" {
                continue;
            }
            let code = output
                .strip_prefix("Exit code: ")
                .and_then(|rest| rest.split('\n').next())
                .unwrap_or_else(|| panic!("Bash({args}) is unframed: {output}"));
            assert!(
                code.parse::<u8>().is_ok(),
                "Bash({args}) has no numeric exit code: {output}"
            );
        }
    }
}

#[test]
fn the_scripted_file_calls_carry_their_arguments_and_resolve_with_the_ack() {
    // Demo/live parity past the cell (`docs/dummy-backend.md`): a scripted
    // `Write`/`Edit` carries the verbatim arguments a real call would — so
    // the offline Ctrl+D shows the same replayed shape — and resolves as the
    // two-text `ToolAnswered` the live executor sends, the numbered body on
    // the cell and one line to the model (`docs/tools.md`).
    let events = turn_events("show me a diff", 0);
    let mut seen = Vec::new();
    let mut open: Option<(String, String)> = None;
    for event in &events {
        match event {
            StreamEvent::ToolStart {
                name, arguments, ..
            } => {
                let arguments = arguments
                    .clone()
                    .unwrap_or_else(|| panic!("a scripted {name} call carries its arguments"));
                open = Some((name.clone(), arguments));
            }
            StreamEvent::ToolAnswered {
                display, result, ..
            } => {
                let (name, arguments) = open.take().expect("a resolution closes a start");
                seen.push((name, arguments, display.clone(), result.clone()));
            }
            StreamEvent::ToolEnd { .. } => {
                open = None;
            }
            _ => {}
        }
    }
    let names: Vec<&str> = seen.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(
        names,
        ["Write", "Edit"],
        "only the file tools split: {seen:?}"
    );
    for (name, arguments, display, result) in &seen {
        let parsed: serde_json::Value =
            serde_json::from_str(arguments).unwrap_or_else(|e| panic!("{name} args: {e}"));
        assert!(
            parsed
                .get("path")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "{name} carries its path: {arguments}"
        );
        let payload = if name == "Write" {
            "content"
        } else {
            "old_string"
        };
        assert!(
            parsed.get(payload).is_some(),
            "{name} carries its {payload} — the whole point: {arguments}"
        );
        assert!(
            display.lines().count() > 1,
            "the cell keeps the numbered body: {display:?}"
        );
        assert_eq!(
            result.lines().count(),
            1,
            "the model reads one line: {result:?}"
        );
        assert!(
            result.ends_with(crate::llm::tools::FILE_STATE_NOTE),
            "…closing on the in-context claim: {result:?}"
        );
    }
}

#[test]
fn a_diff_prompt_scripts_the_write_then_edit_demo() {
    // The file-change design (`docs/tools.md`) has no offline demo otherwise:
    // a `Write` shows the whole new file numbered, an `Edit` shows only the
    // changed hunks with `+`/`-` signs the cell tints green and red. The
    // scripted outputs are built by the executor's own renderers, so the
    // cells are the live ones.
    let events = turn_events("show me a diff", 0);
    let tools = resolved_tools(&events);
    let names: Vec<&str> = tools.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(
        names,
        ["Write", "Edit", "Bash"],
        "the demo writes a file, edits it, then runs it"
    );
    let (_, path, created) = &tools[0];
    assert!(
        created.starts_with("Wrote ")
            && created
                .lines()
                .next()
                .is_some_and(|head| head.ends_with(&format!(" lines to {path}"))),
        "the Write reports the executor's written head: {created}"
    );
    assert!(
        created
            .lines()
            .skip(1)
            .all(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())),
        "the created body is numbered content: {created}"
    );
    let (_, _, updated) = &tools[1];
    assert!(
        updated.starts_with(&format!("Updated {path} (+")),
        "the Edit reports the executor's updated head: {updated}"
    );
    let body: Vec<&str> = updated.lines().skip(1).collect();
    assert!(
        body.iter().any(|l| l
            .trim_start()
            .trim_start_matches(char::is_numeric)
            .starts_with(" +")),
        "the diff body carries an added row: {updated}"
    );
    assert!(
        body.iter().any(|l| l
            .trim_start()
            .trim_start_matches(char::is_numeric)
            .starts_with(" -")),
        "the diff body carries a removed row: {updated}"
    );
}

#[test]
fn turn_events_includes_one_paired_thinking_phase_before_the_tools() {
    let events = turn_events("hi", 0);
    let starts = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ThinkingStart))
        .count();
    let ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ThinkingEnd))
        .count();
    assert_eq!(starts, 1, "the turn thinks exactly once");
    assert_eq!(ends, 1, "every ThinkingStart has a matching ThinkingEnd");

    let start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingStart))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingEnd))
        .unwrap();
    let first_tool = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .unwrap();
    assert!(start < end, "thinking starts before it ends");
    assert!(
        end < first_tool,
        "thinking resolves before the first tool, so tool start/end stay adjacent"
    );
}

#[test]
fn turn_events_streams_thinking_chunks_inside_the_thinking_phase() {
    // The reasoning text travels as ThinkingChunk events strictly between
    // the ThinkingStart/ThinkingEnd pair — opaque to the renderer (never
    // displayed) but counted into the live token tally, like a real API's
    // reasoning deltas.
    let events = turn_events("hi", 0);
    let start = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingStart))
        .unwrap();
    let end = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ThinkingEnd))
        .unwrap();
    let chunk_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, StreamEvent::ThinkingChunk(_)))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !chunk_positions.is_empty(),
        "the dummy streams reasoning text while it thinks"
    );
    assert!(
        chunk_positions.iter().all(|&i| start < i && i < end),
        "every ThinkingChunk sits inside the Start/End pair"
    );
}

#[test]
fn turn_events_resolve_each_tool_before_the_next_starts() {
    // Tools don't nest: after a ToolStart, only its live ToolOutput chunks
    // may appear before the matching ToolEnd — never a second ToolStart — so
    // the loop only ever tracks one running tool at a time. See
    // `docs/tool-streaming.md`.
    let events = turn_events("anything", 0);
    let mut running = false;
    for event in &events {
        match event {
            StreamEvent::ToolStart { .. } => {
                assert!(!running, "a tool starts only after the previous one ended");
                running = true;
            }
            StreamEvent::ToolEnd { .. } | StreamEvent::ToolAnswered { .. } => {
                assert!(running, "a resolution closes a running tool");
                running = false;
            }
            StreamEvent::ToolOutput(_) => {
                assert!(running, "live output only streams while a tool runs");
            }
            _ => {}
        }
    }
    assert!(!running, "every tool that started also ended");
}

#[test]
fn turn_events_streams_each_bash_output_before_its_end() {
    // Each Bash call streams its output as ToolOutput chunks between its
    // ToolStart and ToolEnd. What streams is what the command *printed* — the
    // executor frames the result with its `Exit code: N` line only when it
    // resolves, so the live tail shows command output and nothing else. The
    // ToolEnd then carries the framed body (the authoritative full cell). See
    // `docs/tool-streaming.md`.
    let events = turn_events("run three pings in parallel", 0);
    let mut streamed = String::new();
    let mut resolved = 0;
    for event in &events {
        match event {
            StreamEvent::ToolOutput(chunk) => streamed.push_str(chunk),
            StreamEvent::ToolEnd { output, .. } => {
                let (frame, body) = output
                    .split_once('\n')
                    .expect("a framed command result has a body");
                assert!(frame.starts_with("Exit code: "), "unframed: {output}");
                assert_eq!(streamed, body, "the tail reconstructs the final cell");
                streamed.clear();
                resolved += 1;
            }
            _ => {}
        }
    }
    assert_eq!(resolved, 3, "all three Bash cells streamed then resolved");
}

#[test]
fn turn_events_generates_each_tool_call_before_it_starts() {
    // Each ToolStart is preceded by ToolCallDelta fragments (the model
    // "generating" the call) and never sits between a Start and its End, so
    // the tally ticks during generation and tools still resolve one at a time.
    let events = turn_events("anything", 0);
    let deltas = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolCallDelta(_)))
        .count();
    assert!(
        deltas >= 1,
        "the dummy streams tool-call generation fragments"
    );
    let mut running = false;
    for event in &events {
        match event {
            StreamEvent::ToolStart { .. } => running = true,
            StreamEvent::ToolEnd { .. } => running = false,
            StreamEvent::ToolCallDelta(_) => assert!(
                !running,
                "a generation fragment never streams while a tool is running"
            ),
            _ => {}
        }
    }
}

#[test]
fn the_demo_shows_both_a_success_and_a_failure() {
    // The demo has to exercise both cell colours, but they belong in
    // different turns. The default turn is one errand that *works* — a
    // gratuitously failing call in the middle of read/edit/run would only
    // muddle the story it tells — so red lives in the parallel batch, whose
    // third ping can't resolve its host.
    let outcomes = |prompt: &str| -> (usize, usize) {
        let events = turn_events(prompt, 0);
        (
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: true, .. }))
                .count(),
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::ToolEnd { ok: false, .. }))
                .count(),
        )
    };
    let (oks, fails) = outcomes("x");
    assert!(oks >= 1, "the default turn's steps succeed (green)");
    assert_eq!(fails, 0, "and none of them fails: the errand works");
    let (oks, fails) = outcomes("run three pings in parallel");
    assert!(oks >= 1, "the parallel batch has a green cell");
    assert!(fails >= 1, "and a red one");
}

#[test]
fn turn_events_ends_with_stream_done() {
    assert_eq!(turn_events("x", 0).last(), Some(&StreamEvent::StreamDone));
}

#[test]
fn turn_events_without_images_streams_just_the_reply() {
    // image_count 0 is the old behaviour: chunks reconstruct the reply.
    let prompt = "hello";
    assert_eq!(chunk_text(&turn_events(prompt, 0)), dummy_response(prompt));
}

#[test]
fn turn_events_acknowledges_attached_images_up_front() {
    // The dummy has no vision, so it acknowledges the attachments instead —
    // a leading chunk proving the typed image channel reached the backend.
    let prompt = "what is this";
    assert_eq!(
        chunk_text(&turn_events(prompt, 2)),
        format!("Looking at your 2 images. {}", dummy_response(prompt))
    );
}

#[test]
fn image_acknowledgement_is_singular_for_one_image() {
    assert!(chunk_text(&turn_events("x", 1)).starts_with("Looking at your 1 image. "));
}

#[test]
fn an_agents_prompt_scripts_the_two_agent_demo() {
    let events = turn_events("call agents for weather", 0);
    let batch = events.iter().find_map(|e| match e {
        StreamEvent::AgentBatch { background, agents } => Some((background, agents)),
        _ => None,
    });
    let (background, agents) = batch.expect("the demo announces a group");
    assert!(!background, "foreground by default");
    assert_eq!(agents.len(), 2);
    assert!(agents[0].description.contains("Warsaw"));
    assert!(agents[0].id.starts_with('a'));
    let done = events.iter().find_map(|e| match e {
        StreamEvent::AgentGroupDone { agents, .. } => Some(agents),
        _ => None,
    });
    let done = done.expect("the demo resolves the group");
    assert_eq!(done.len(), 2);
    assert!(done.iter().all(|d| d.ok));
    assert_eq!(events.last(), Some(&StreamEvent::StreamDone));
    // A "background" prompt launches in background mode with launch texts.
    let events = turn_events("call background agents", 0);
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::AgentBatch {
            background: true,
            ..
        }
    )));
    // /init's canned prompt names AGENTS.md — never the demo.
    let events = turn_events("Generate a file named AGENTS.md", 0);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::AgentBatch { .. }))
    );
}

#[test]
fn a_markdown_prompt_streams_a_long_structured_document_token_by_token() {
    // The slow-stream stress demo (`docs/slow-stream.md`): a prompt
    // mentioning "markdown" plays a text-only turn — no thinking phase, no
    // tool calls, so the whole document is ONE message and the incremental
    // renderer carries every block kind from the first character to the
    // last — whose chunks are token-sized pieces rather than words, so the
    // boundaries land inside markers and across line breaks the way a real
    // model's tokens do. Every markdown element the renderer knows is in it,
    // and it closes on the hand-off like every user-facing demo.
    let events = turn_events("stream some markdown to me", 0);
    let text = chunk_text(&events);
    assert_eq!(text, MARKDOWN_TOUR, "the chunks reconstruct the document");
    assert!(
        events
            .iter()
            .all(|e| matches!(e, StreamEvent::Chunk(_) | StreamEvent::StreamDone)),
        "a markdown turn is text-only: {events:?}"
    );
    assert!(matches!(events.last(), Some(StreamEvent::StreamDone)));
    let pieces = events.len() - 1;
    assert!(
        pieces > chunks(MARKDOWN_TOUR).len(),
        "the document streams in token-sized pieces, finer than words ({pieces} pieces)"
    );
    for needle in [
        "# ",
        "\n## ",
        "\n### ",
        "**",
        "*italic*",
        "~~",
        "`inline code`",
        "](https://",
        "\n- ",
        "\n  - ",
        "\n1. ",
        "\n10. ",
        "- [x] ",
        "- [ ] ",
        "\n> ",
        "```python\n",
        "```rust\n",
        "\n| ",
        "|---",
        "\n---\n",
        "✅",
        "https://",
    ] {
        assert!(
            MARKDOWN_TOUR.contains(needle),
            "the tour is missing the {needle:?} element"
        );
    }
    // A fence with a blank line INSIDE it (content, never a paragraph break)
    // and a code line long enough to wrap at eighty columns (a withheld
    // multi-row line): the two shapes the strip has to hold whole.
    assert!(
        MARKDOWN_TOUR.contains("\n\n    return"),
        "a blank line inside a fence"
    );
    assert!(
        MARKDOWN_TOUR.lines().any(|l| l.chars().count() > 80),
        "a code line that wraps at eighty columns"
    );
}

#[test]
fn the_markdown_tour_acknowledges_images_and_hands_off() {
    // Two images attached open the reply with the acknowledgement, in front
    // of the document (the image-channel rule every demo follows).
    let text = chunk_text(&turn_events("stream some markdown to me", 2));
    assert!(text.starts_with("Looking at your 2 images. "));
    assert!(text.ends_with(MARKDOWN_TOUR));
}

#[test]
fn every_content_line_of_the_markdown_tour_is_distinct() {
    // The smoke suite proves nothing streams twice by counting each row of
    // the settled transcript once (`docs/slow-stream.md`): that only works
    // if no two source lines render alike, so the document is written with
    // every non-blank line distinct — table delimiter rows and fence
    // markers excepted, being structural.
    let mut seen = std::collections::HashSet::new();
    for line in MARKDOWN_TOUR.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("```") || trimmed.starts_with("|-") {
            continue;
        }
        assert!(
            seen.insert(trimmed),
            "a repeated line in the tour: {trimmed:?}"
        );
    }
}

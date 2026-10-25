//! The calls a model is still **generating**: the incremental argument
//! decoder and the round's draft bookkeeping (`docs/write-streaming.md`).

use super::*;
use crate::stream::ToolCallFragment;

/// Feed `text` to a fresh scanner one character at a time — the worst split a
/// stream can make, so every state carries across a piece boundary.
fn scan_chars(text: &str) -> ArgsScanner {
    let mut scanner = ArgsScanner::default();
    for c in text.chars() {
        scanner.feed(c.encode_utf8(&mut [0; 4]));
    }
    scanner
}

/// A field's decoded value and whether its string closed.
fn field(scanner: &ArgsScanner, key: &str) -> Option<(String, bool)> {
    scanner
        .field(key)
        .map(|field| (field.value.clone(), field.closed))
}

#[test]
fn a_whole_object_decodes_every_top_level_string_field() {
    let mut scanner = ArgsScanner::default();
    scanner.feed(r#"{"path":"/tmp/a.py","content":"x = 1\nprint(x)\n"}"#);
    assert_eq!(
        field(&scanner, "path"),
        Some(("/tmp/a.py".to_string(), true))
    );
    assert_eq!(
        field(&scanner, "content"),
        Some(("x = 1\nprint(x)\n".to_string(), true))
    );
    assert_eq!(field(&scanner, "missing"), None);
}

#[test]
fn a_value_still_streaming_is_open_and_holds_what_arrived() {
    let mut scanner = ArgsScanner::default();
    scanner.feed(r#"{"path":"a.py","content":"line one\nline tw"#);
    assert_eq!(
        field(&scanner, "content"),
        Some(("line one\nline tw".to_string(), false))
    );
    scanner.feed(r#"o\n"}"#);
    assert_eq!(
        field(&scanner, "content"),
        Some(("line one\nline two\n".to_string(), true))
    );
}

#[test]
fn every_escape_decodes_even_split_one_character_at_a_time() {
    let scanner = scan_chars(
        r#"{"content":"q\" b\\ s\/ \b\f\n\r\t é=\u00e9 grin=\ud83d\ude00 snow=\u2603"}"#,
    );
    assert_eq!(
        field(&scanner, "content"),
        Some((
            "q\" b\\ s/ \u{8}\u{c}\n\r\t é=é grin=😀 snow=☃".to_string(),
            true
        ))
    );
}

#[test]
fn a_lone_surrogate_decodes_as_the_replacement_character() {
    let scanner = scan_chars(r#"{"content":"a\ud83db\ude00c\ud83d"}"#);
    assert_eq!(
        field(&scanner, "content"),
        Some(("a\u{fffd}b\u{fffd}c\u{fffd}".to_string(), true))
    );
}

#[test]
fn whitespace_key_order_and_skipped_values_do_not_disturb_the_fields() {
    let scanner = scan_chars(
        "{ \"meta\" : { \"a\": [1, \"}\\\"\", {\"b\": null}] } ,\n \"n\" : -12.5e3, \
         \"ok\": true, \"none\": null, \"content\" : \"body\" , \"path\":\"p.rs\" }",
    );
    assert_eq!(field(&scanner, "content"), Some(("body".to_string(), true)));
    assert_eq!(field(&scanner, "path"), Some(("p.rs".to_string(), true)));
    // A nested object's own keys are not top-level fields.
    assert_eq!(field(&scanner, "a"), None);
    assert_eq!(field(&scanner, "b"), None);
}

#[test]
fn an_escaped_key_is_decoded_too() {
    let scanner = scan_chars(r#"{"con\u0074ent":"x"}"#);
    assert_eq!(field(&scanner, "content"), Some(("x".to_string(), true)));
}

#[test]
fn the_first_of_two_same_named_fields_is_the_one_read() {
    let scanner = scan_chars(r#"{"content":"first","content":"second"}"#);
    assert_eq!(
        field(&scanner, "content"),
        Some(("first".to_string(), true))
    );
}

#[test]
fn text_that_is_not_an_object_yields_no_fields() {
    for text in [r#"["content","x"]"#, r#""content""#, "content", ""] {
        let scanner = scan_chars(text);
        assert_eq!(field(&scanner, "content"), None, "{text:?}");
    }
}

/// Objects whose fields hold everything the decoder must get right — escapes
/// of every kind, a surrogate pair, literal non-ASCII, nested values and
/// scalars under other keys (a nested `content` among them), whitespace
/// between the tokens, an empty value — for the tests that hold it to what
/// `serde_json` reads in the same text.
const SAMPLES: &[&str] = &[
    r#"{"path":"/tmp/a.py","content":"def f():\n    return \"x\\y\"\n"}"#,
    r#"{"path":"a.py","content":"def f():\n\treturn \"é😀\"\n"}"#,
    r#"{"content":"tab\there é 😀 \/ \b\f\r","path":"b"}"#,
    r#"{"skip":{"content":"no","list":[1,{"z":"}"}]},"content":"ünïcödé ✓","n":3}"#,
    "{ \"meta\" : { \"a\": [1, \"}\\\"\", {\"b\": null}] } ,\n \"n\" : -12.5e3, \
     \"ok\": true, \"none\": null, \"content\" : \"body\" , \"path\":\"p.rs\" }",
    r#"  { "path" : "spaced name.txt" , "content" : "" }  "#,
];

/// The keys a preview reads.
const KEYS: [&str; 2] = ["path", "content"];

/// What `serde_json` reads for `key` in the whole of `json` — the oracle.
fn serde_field(json: &str, key: &str) -> Option<String> {
    let whole: serde_json::Value = serde_json::from_str(json).expect("a valid sample");
    whole
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[test]
fn every_two_piece_split_decodes_what_serde_reads() {
    for json in SAMPLES {
        for at in (0..=json.len()).filter(|&at| json.is_char_boundary(at)) {
            let mut scanner = ArgsScanner::default();
            scanner.feed(&json[..at]);
            scanner.feed(&json[at..]);
            for key in KEYS {
                assert_eq!(
                    field(&scanner, key),
                    serde_field(json, key).map(|value| (value, true)),
                    "{json} split at {at}: {key}"
                );
            }
        }
    }
}

#[test]
fn a_decoded_value_only_ever_grows_whatever_the_split() {
    // The render cache keys on this: every prefix of the stream decodes to a
    // prefix of the whole value (docs/write-streaming.md) — of what serde
    // reads in it — fed one character at a time, the worst split a stream
    // can make.
    for json in SAMPLES {
        let mut scanner = ArgsScanner::default();
        let mut previous = [String::new(), String::new()];
        for c in json.chars() {
            scanner.feed(c.encode_utf8(&mut [0; 4]));
            for (key, previous) in KEYS.iter().zip(&mut previous) {
                let whole = serde_field(json, key).unwrap_or_default();
                let now = field(&scanner, key).map(|(v, _)| v).unwrap_or_default();
                assert!(
                    now.starts_with(previous.as_str()),
                    "{json}: {key} {previous:?} -> {now:?}"
                );
                assert!(
                    whole.starts_with(&now),
                    "{json}: {key} {now:?} is not a prefix of {whole:?}"
                );
                *previous = now;
            }
        }
    }
}

// --- the round's drafts ---

/// One streamed piece of call `index`.
fn piece(index: usize, name: &str, arguments: &str) -> ToolCallFragment {
    ToolCallFragment {
        index,
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

/// The previews' paths, in order (`?` for one whose path has not closed).
fn preview_paths(drafts: &ToolDrafts) -> Vec<String> {
    drafts
        .previews()
        .map(|draft| draft.path().unwrap_or("?").to_string())
        .collect()
}

#[test]
fn a_piece_naming_write_opens_a_preview_that_decodes_as_the_rest_streams() {
    let mut drafts = ToolDrafts::default();
    assert!(
        drafts.push(&piece(0, "write", "")),
        "naming the call opens its preview"
    );
    assert!(
        !drafts.push(&piece(0, "", r#"{"path":"/tmp/a.py","content":"one\ntw"#)),
        "a later piece of the same call opens nothing new"
    );
    let draft = drafts.previews().next().expect("one preview");
    assert_eq!(draft.name(), "write");
    assert_eq!(draft.path(), Some("/tmp/a.py"));
    assert_eq!(draft.content(), "one\ntw");
    assert!(!draft.finished(), "the content is still arriving");
    drafts.push(&piece(0, "", r#"o\n"}"#));
    let draft = drafts.previews().next().expect("one preview");
    assert_eq!(draft.content(), "one\ntwo\n");
    assert!(draft.finished(), "the content's closing quote finishes it");
}

#[test]
fn a_streaming_write_colours_only_the_lines_its_cell_can_show() {
    // The strip's cell shows the first and newest lines, so that is all the
    // draft's peek keeps coloured — a line's segments run ~50 times its text
    // (docs/memory.md). The whole file is coloured only when a view asks for
    // every line: Ctrl+O, `/export`.
    let content: String = (1..=500).map(|i| format!("v{i} = {i}\n")).collect();
    let encoded = serde_json::to_string(&content).expect("a string encodes");
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(
        0,
        "write",
        &format!(
            r#"{{"path":"a.py","content":{}"#,
            &encoded[..encoded.len() - 1]
        ),
    ));
    let draft = drafts.previews().next().expect("a preview");
    let code = crate::highlight::CodeTheme::default();
    let peek = draft.highlighted_peek(Some("python"), code, 4, 10);
    assert_eq!(peek.line_count(), 500);
    assert_eq!(peek.retained(), 20, "the first ten and the newest ten");
    drop(peek);
    let whole = draft.highlighted_whole(Some("python"), code, 4);
    assert_eq!(whole.retained(), 500);
}

#[test]
fn a_call_to_any_other_tool_is_tracked_but_never_previewed() {
    let mut drafts = ToolDrafts::default();
    assert!(!drafts.push(&piece(0, "bash", r#"{"command":"ls"}"#)));
    assert_eq!(drafts.previews().count(), 0);
    assert!(drafts.push(&piece(1, "write", r#"{"path":"b.py""#)));
    assert_eq!(preview_paths(&drafts), ["b.py"]);
}

#[test]
fn a_name_that_arrives_after_the_first_piece_opens_the_preview_then() {
    let mut drafts = ToolDrafts::default();
    assert!(!drafts.push(&piece(0, "", r#"{"path":"#)));
    assert_eq!(drafts.previews().count(), 0);
    assert!(drafts.push(&piece(0, "write", r#""a.py","content":"x"#)));
    assert_eq!(preview_paths(&drafts), ["a.py"]);
    assert_eq!(
        drafts.previews().next().map(ToolDraft::content),
        Some("x"),
        "the pieces before the name were kept"
    );
}

#[test]
fn opens_preview_predicts_push_and_changes_nothing() {
    let mut drafts = ToolDrafts::default();
    let opening = piece(0, "write", "{");
    let revision = drafts.revision();
    assert!(drafts.opens_preview(&opening));
    assert_eq!(drafts.revision(), revision, "a question moves nothing");
    assert_eq!(drafts.previews().count(), 0);
    assert!(drafts.push(&opening));
    let more = piece(0, "write", r#""path":"#);
    assert!(
        !drafts.opens_preview(&more),
        "the call is already previewed"
    );
    assert!(!drafts.opens_preview(&piece(1, "bash", "")));
    assert!(drafts.opens_preview(&piece(1, "write", "")));
}

#[test]
fn the_path_shows_only_once_its_string_has_closed() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"/home/us"#));
    assert_eq!(drafts.previews().next().and_then(ToolDraft::path), None);
    drafts.push(&piece(0, "", r#"er/a.py","#));
    assert_eq!(
        drafts.previews().next().and_then(ToolDraft::path),
        Some("/home/user/a.py")
    );
}

#[test]
fn parallel_writes_preview_in_the_order_they_opened() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"A"}"#));
    drafts.push(&piece(1, "write", r#"{"path":"b.py","content":"B"#));
    assert_eq!(preview_paths(&drafts), ["a.py", "b.py"]);
    let finished: Vec<bool> = drafts.previews().map(ToolDraft::finished).collect();
    assert_eq!(finished, [true, false]);
}

#[test]
fn sealing_finishes_every_draft_and_the_next_piece_opens_a_new_round() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"never clo"#));
    drafts.seal();
    assert!(
        drafts.previews().all(ToolDraft::finished),
        "a sealed round has nothing left to stream"
    );
    // The next round reuses the wire's slots from 0: its first piece must
    // not continue the last round's call.
    assert!(drafts.push(&piece(0, "write", r#"{"path":"b.py""#)));
    assert_eq!(preview_paths(&drafts), ["b.py"]);
    assert_eq!(drafts.previews().next().map(ToolDraft::content), Some(""));
}

#[test]
fn the_announced_batch_adopts_the_previews_one_per_queued_write() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"A"}"#));
    drafts.push(&piece(1, "bash", r#"{"command":"ls"}"#));
    drafts.push(&piece(2, "write", r#"{"path":"b.py","content":"B"}"#));
    drafts.adopt(["Write", "Bash", "Write"]);
    assert_eq!(drafts.previews().count(), 0, "the queue shows them now");
    let queued: Vec<Option<&str>> = drafts.queued().map(ToolDraft::path).collect();
    assert_eq!(queued, [Some("a.py"), Some("b.py")]);
    assert!(drafts.queued().all(ToolDraft::finished));
    drafts.resolve("Bash");
    assert_eq!(drafts.queued().count(), 2, "a bash call owns no draft");
    drafts.resolve("Write");
    let queued: Vec<Option<&str>> = drafts.queued().map(ToolDraft::path).collect();
    assert_eq!(queued, [Some("b.py")], "the front write took its own");
}

#[test]
fn a_batch_that_does_not_match_the_previews_adopts_none_of_them() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"A"}"#));
    drafts.adopt(["Write", "Write"]);
    assert_eq!(
        drafts.queued().count(),
        0,
        "a count that disagrees pairs nothing"
    );
    assert_eq!(drafts.previews().count(), 0);
    // …and a resolution with nothing adopted is a no-op.
    drafts.resolve("Write");
    assert!(drafts.is_empty());
}

#[test]
fn clear_drops_the_round_and_the_queue_alike() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"A"}"#));
    drafts.adopt(["Write"]);
    drafts.push(&piece(0, "write", r#"{"path":"b.py""#));
    assert!(!drafts.is_empty());
    drafts.clear();
    assert!(drafts.is_empty());
    assert_eq!(drafts.previews().count(), 0);
    assert_eq!(drafts.queued().count(), 0);
}

#[test]
fn a_restart_drops_the_calls_being_made_and_keeps_the_queue() {
    // The round's request is being sent again (docs/dropped-reply.md): the
    // calls the dropped attempt was making will never finish, while a queued
    // call's preview belongs to a cell that is still there.
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"a.py","content":"A"}"#));
    drafts.adopt(["Write"]);
    drafts.push(&piece(0, "write", r#"{"path":"b.py","content":"hal"#));
    let revision = drafts.revision();
    drafts.restart();
    assert!(drafts.revision() > revision, "a restart moves the revision");
    assert_eq!(drafts.previews().count(), 0, "the half-made call is gone");
    let queued: Vec<Option<&str>> = drafts.queued().map(ToolDraft::path).collect();
    assert_eq!(queued, [Some("a.py")], "the queued cell keeps its preview");
    // The call made again opens fresh under the same slot.
    assert!(drafts.push(&piece(0, "write", r#"{"path":"b.py","content":"whole"#)));
    let contents: Vec<&str> = drafts.previews().map(ToolDraft::content).collect();
    assert_eq!(contents, ["whole"]);
}

#[test]
fn every_change_moves_the_revision() {
    let mut drafts = ToolDrafts::default();
    let mut last = drafts.revision();
    let mut moved = |drafts: &ToolDrafts, what: &str| {
        assert!(drafts.revision() > last, "{what} must move the revision");
        last = drafts.revision();
    };
    drafts.push(&piece(0, "write", "{"));
    moved(&drafts, "a piece");
    drafts.push(&piece(0, "", r#""path":"a""#));
    moved(&drafts, "a later piece");
    drafts.seal();
    moved(&drafts, "a seal");
    drafts.adopt(["Write"]);
    moved(&drafts, "an adoption");
    drafts.resolve("Write");
    moved(&drafts, "a resolution");
    drafts.clear();
    moved(&drafts, "a clear");
}

// --- the App's drafts: the main session's round (docs/write-streaming.md) ---

/// An app mid-turn: a user message sent and the reply open.
fn turn() -> App {
    let mut app = App::new();
    app.record_user_message("write the script");
    app.begin_stream();
    app
}

/// The pieces of one streamed `write` of `path` with `content`, under slot 0.
fn write_pieces(path: &str, content: &str) -> Vec<ToolCallFragment> {
    let arguments = serde_json::json!({ "path": path, "content": content }).to_string();
    let (head, tail) = arguments.split_at(arguments.len() / 2);
    vec![piece(0, "write", head), piece(0, "", tail)]
}

/// A round of the given `(wire name, arguments)` calls, as `RoundCalls` names it.
fn round(calls: &[(&str, &str)]) -> Vec<crate::stream::RoundCall> {
    calls
        .iter()
        .enumerate()
        .map(|(i, (name, arguments))| crate::stream::RoundCall {
            id: format!("call_{i}"),
            name: (*name).to_string(),
            arguments: (*arguments).to_string(),
        })
        .collect()
}

/// The batch announcement's entry for one cell.
fn summary(name: &str, args: &str) -> ToolCallSummary {
    ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    }
}

#[test]
fn a_streamed_piece_counts_into_the_tally_exactly_as_its_old_string_did() {
    let mut app = turn();
    let piece = piece(0, "write", r#"{"path":"a.py","content":"x"#);
    let before = app.status().expect("a live turn").tokens;
    app.push_tool_call_fragment(&piece);
    let status = app.status().expect("a live turn");
    assert_eq!(
        status.tokens - before,
        count_tokens(r#"write{"path":"a.py","content":"x"#)
    );
    assert_eq!(status.arrow, TokenArrow::Down);
}

#[test]
fn a_write_mid_turn_opens_a_preview_and_says_so() {
    let mut app = turn();
    let mut opened = Vec::new();
    for piece in write_pieces("/tmp/a.py", "print(1)\n") {
        opened.push(app.push_tool_call_fragment(&piece));
    }
    assert_eq!(opened, [true, false], "only the naming piece opens it");
    let draft = app.tool_drafts().previews().next().expect("a preview");
    assert_eq!(draft.path(), Some("/tmp/a.py"));
    assert_eq!(draft.content(), "print(1)\n");
}

#[test]
fn an_idle_app_folds_no_piece() {
    let mut app = App::new();
    assert!(!app.push_tool_call_fragment(&piece(0, "write", "{")));
    assert!(app.tool_drafts().is_empty());
    assert!(app.status().is_none());
}

#[test]
fn the_round_announcement_seals_and_the_batch_adopts_the_previews() {
    let mut app = turn();
    for piece in write_pieces("a.py", "A\n") {
        app.push_tool_call_fragment(&piece);
    }
    app.push_tool_call_fragment(&piece(1, "bash", r#"{"command":"ls"}"#));
    app.open_round(round(&[("write", "{}"), ("bash", "{}")]));
    assert!(
        app.tool_drafts().previews().all(ToolDraft::finished),
        "the announcement finishes what streamed"
    );
    app.start_tool_batch(&[summary("Write", "a.py"), summary("Bash", "ls")]);
    assert_eq!(app.tool_drafts().previews().count(), 0);
    assert_eq!(
        app.tool_drafts()
            .queued()
            .map(ToolDraft::path)
            .collect::<Vec<_>>(),
        [Some("a.py")]
    );
}

#[test]
fn a_lone_tool_start_adopts_its_preview() {
    let mut app = turn();
    for piece in write_pieces("a.py", "A\n") {
        app.push_tool_call_fragment(&piece);
    }
    app.start_tool("Write", "a.py", Some(r#"{"path":"a.py","content":"A\n"}"#));
    assert_eq!(app.tool_drafts().queued().count(), 1);
}

#[test]
fn a_queued_write_gives_its_draft_up_as_it_resolves() {
    let mut app = turn();
    for piece in write_pieces("a.py", "A\n") {
        app.push_tool_call_fragment(&piece);
    }
    app.start_tool_batch(&[summary("Write", "a.py")]);
    app.start_tool("Write", "a.py", None);
    assert_eq!(app.tool_drafts().queued().count(), 1);
    app.answer_tool(
        "Wrote 1 line to a.py\n1 A",
        "File created successfully at: a.py",
    );
    assert!(
        app.tool_drafts().is_empty(),
        "the resolved call took its draft"
    );
}

#[test]
fn an_agent_group_or_a_task_call_seals_the_round() {
    let sealed = |app: &App| {
        let finished: Vec<bool> = app
            .tool_drafts()
            .previews()
            .map(ToolDraft::finished)
            .collect();
        finished == [true]
    };
    let mut app = turn();
    app.push_tool_call_fragment(&piece(0, "write", r#"{"path":"a.py","content":"op"#));
    assert!(!sealed(&app), "the content is still open");
    app.start_agent_group(true, &[]);
    assert!(sealed(&app), "an agent group");

    let mut app = turn();
    app.push_tool_call_fragment(&piece(0, "write", r#"{"path":"a.py","content":"op"#));
    app.record_task_call(
        "TaskList",
        "",
        "{}",
        "No tasks",
        true,
        crate::tasks::TaskStore::new(),
    );
    assert!(sealed(&app), "a task call");
}

#[test]
fn every_turn_edge_drops_the_drafts() {
    let streaming = || {
        let mut app = turn();
        app.push_tool_call_fragment(&piece(0, "write", r#"{"path":"a.py","content":"op"#));
        assert!(!app.tool_drafts().is_empty());
        app
    };
    let mut app = streaming();
    app.begin_stream();
    assert!(app.tool_drafts().is_empty(), "a new turn");
    let mut app = streaming();
    app.finish_stream();
    assert!(app.tool_drafts().is_empty(), "a finished turn");
    let mut app = streaming();
    app.fail_stream("boom");
    assert!(app.tool_drafts().is_empty(), "a failed turn");
    let mut app = streaming();
    app.interrupt_turn();
    assert!(app.tool_drafts().is_empty(), "an interrupted turn");
    let mut app = streaming();
    app.block_prompt("no");
    assert!(app.tool_drafts().is_empty(), "a blocked prompt");
    let mut app = streaming();
    app.clear_conversation();
    assert!(app.tool_drafts().is_empty(), "/clear");
}

#[test]
fn a_retry_drops_the_call_the_dropped_attempt_was_making() {
    // The connection dropped while the model was making a `write`, and the
    // round is being sent again (docs/dropped-reply.md). The call made again
    // arrives under the same slot: folded into the old draft, its pieces
    // would continue the cut-off arguments instead of starting their own.
    let mut app = turn();
    app.push_tool_call_fragment(&piece(0, "write", r#"{"path":"a.py","content":"hal"#));
    app.set_retry(1, 3);
    assert!(
        app.tool_drafts().is_empty(),
        "the half-made call went with its attempt"
    );
    for piece in write_pieces("a.py", "whole\n") {
        app.push_tool_call_fragment(&piece);
    }
    let contents: Vec<&str> = app
        .tool_drafts()
        .previews()
        .map(ToolDraft::content)
        .collect();
    assert_eq!(contents, ["whole\n"]);
}

#[test]
fn an_esc_after_a_previewed_write_streamed_interrupts_rather_than_undoing() {
    // The undo is for a turn that showed nothing (docs/interrupt.md); a file
    // streaming into its cell is something.
    let mut app = turn();
    for piece in write_pieces("a.py", "A\n") {
        app.push_tool_call_fragment(&piece);
    }
    assert!(matches!(
        app.interrupt_turn(),
        Some(InterruptedTurn::Kept { .. })
    ));
    assert_eq!(roles(&app).first(), Some(&Role::User), "the message stays");

    // A call that previews nothing (a bash command being generated) still
    // leaves the undo in place: nothing was shown.
    let mut app = turn();
    app.push_tool_call_fragment(&piece(0, "bash", r#"{"command":"ls"}"#));
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
}

#[test]
fn a_batch_announcement_replaces_what_an_earlier_one_left_queued() {
    // The batch IS the queue (`App::start_tool_batch` replaces it whole), so
    // its adoptions replace too — an earlier round's leftover draft must
    // never pair with this round's `Write`.
    let mut drafts = ToolDrafts::default();
    drafts.push(&piece(0, "write", r#"{"path":"old.py","content":"A"}"#));
    drafts.adopt(["Write"]);
    drafts.push(&piece(0, "write", r#"{"path":"new.py","content":"B"}"#));
    drafts.adopt(["Write"]);
    let queued: Vec<Option<&str>> = drafts.queued().map(ToolDraft::path).collect();
    assert_eq!(queued, [Some("new.py")]);
}

#[test]
fn a_lone_call_pushed_behind_a_live_write_borrows_no_draft() {
    // Pairing is by order, so a call pushed behind a live `Write` — never
    // seen from a real backend, which announces its batches — previews
    // nothing rather than hand either cell the other's file.
    let mut app = turn();
    for piece in write_pieces("a.py", "A\n") {
        app.push_tool_call_fragment(&piece);
    }
    app.start_tool("Write", "a.py", None);
    for piece in write_pieces("b.py", "B\n") {
        app.push_tool_call_fragment(&piece);
    }
    app.start_tool("Write", "b.py", None);
    let paths: Vec<Option<&str>> = app.tool_drafts().queued().map(ToolDraft::path).collect();
    assert!(
        paths.first().is_none_or(|first| *first == Some("a.py")),
        "the front Write keeps its own file or none: {paths:?}"
    );
    assert!(app.tool_drafts().previews().next().is_none());
}

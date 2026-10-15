//! The round's draft calls — the tool calls the model is still generating,
//! decoded as their arguments stream (`docs/write-streaming.md`).

use super::*;
use crate::stream::{RoundCall, ToolCallFragment, ToolCallSummary};

fn piece(index: usize, name: Option<&str>, arguments: &str) -> ToolCallFragment {
    ToolCallFragment {
        index,
        name: name.map(str::to_string),
        arguments: arguments.to_string(),
    }
}

/// A turn in flight with one `write` draft streaming `arguments`.
fn writing(arguments: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&piece(0, Some("write"), ""));
    app.push_tool_call_fragment(&piece(0, None, arguments));
    app
}

#[test]
fn a_piece_counts_its_tokens_pointing_down_without_touching_the_reply() {
    // While the model *generates* a call, its pieces count into the tally
    // (arrow ↓ — model output) so the status keeps ticking, but the JSON never
    // reaches the reply buffer (docs/status-indicator.md).
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("let me check ");
    let before = app.status().unwrap().tokens;
    app.push_tool_call_fragment(&piece(0, Some("bash"), r#"{"command":"ls -la"}"#));
    let status = app.status().unwrap();
    assert!(status.tokens > before, "the piece counts tokens");
    assert_eq!(status.arrow, TokenArrow::Down, "generation streams down");
    assert_eq!(
        app.streaming_text(),
        Some("let me check "),
        "the reply buffer is untouched by tool-call JSON"
    );
}

#[test]
fn a_piece_counts_its_name_and_its_arguments_both() {
    let mut named = App::new();
    named.begin_stream();
    named.push_tool_call_fragment(&piece(0, Some("bash"), ""));
    let mut arguments = App::new();
    arguments.begin_stream();
    arguments.push_tool_call_fragment(&piece(0, None, r#"{"command":"ls"}"#));
    let mut both = App::new();
    both.begin_stream();
    both.push_tool_call_fragment(&piece(0, Some("bash"), r#"{"command":"ls"}"#));
    let tokens = |app: &App| app.status().unwrap().tokens;
    assert_eq!(tokens(&both), tokens(&named) + tokens(&arguments));
}

#[test]
fn a_piece_is_a_no_op_when_idle() {
    let mut app = App::new();
    assert!(!app.push_tool_call_fragment(&piece(0, Some("write"), r#"{"content":"x"#)));
    assert!(app.status().is_none(), "no status conjured up");
    assert!(app.drafts().is_empty(), "no draft without a turn");
}

#[test]
fn a_write_call_opens_its_draft_on_the_piece_that_names_it() {
    let mut app = App::new();
    app.begin_stream();
    assert!(
        app.push_tool_call_fragment(&piece(0, Some("write"), "")),
        "the name makes the draft visible — the boundary flushes the text before it"
    );
    assert!(
        !app.push_tool_call_fragment(&piece(0, None, r#"{"path""#)),
        "it was already open"
    );
    let writes: Vec<&DraftCall> = app.drafts().writes().collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].name(), Some("write"));
    assert!(app.drafts().has_writes());
}

#[test]
fn a_call_that_is_not_a_write_is_tracked_but_never_shown() {
    let mut app = App::new();
    app.begin_stream();
    assert!(!app.push_tool_call_fragment(&piece(0, Some("bash"), r#"{"command":"ls"}"#)));
    assert!(
        !app.drafts().is_empty(),
        "tracked, so its later pieces have a home"
    );
    assert!(!app.drafts().has_writes());
    assert_eq!(app.drafts().writes().count(), 0);
}

#[test]
fn a_drafts_path_shows_only_once_its_string_has_closed() {
    let mut app = writing(r#"{"path":"src/ga"#);
    let path = |app: &App| {
        app.drafts()
            .writes()
            .next()
            .unwrap()
            .path()
            .map(str::to_string)
    };
    assert_eq!(
        path(&app),
        None,
        "a half-streamed path never reflows the header"
    );
    app.push_tool_call_fragment(&piece(0, None, r#"me.py","#));
    assert_eq!(path(&app).as_deref(), Some("src/game.py"));
}

#[test]
fn a_drafts_content_grows_as_its_pieces_decode() {
    let mut app = writing(r#"{"path":"a.py","content":"import os\nprint("#);
    let content = |app: &App| app.drafts().writes().next().unwrap().content().cloned();
    let field = content(&app).expect("the content string has opened");
    assert_eq!(field.text, "import os\nprint(");
    assert!(!field.complete);
    assert_eq!(field.lines(), 2);
    app.push_tool_call_fragment(&piece(0, None, r#"1)\n"}"#));
    let field = content(&app).expect("still there");
    assert_eq!(field.text, "import os\nprint(1)\n");
    assert!(field.complete, "the closing quote was read");
    assert_eq!(field.lines(), 2);
}

#[test]
fn pieces_of_interleaved_calls_land_on_their_own_drafts() {
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&piece(0, Some("write"), r#"{"path":"a","content":"A"#));
    app.push_tool_call_fragment(&piece(1, Some("write"), r#"{"path":"b","content":"B"#));
    app.push_tool_call_fragment(&piece(0, None, "1"));
    app.push_tool_call_fragment(&piece(1, None, "2"));
    let texts: Vec<(String, String)> = app
        .drafts()
        .writes()
        .map(|d| {
            (
                d.path().unwrap().to_string(),
                d.content().unwrap().text.clone(),
            )
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            ("a".to_string(), "A1".to_string()),
            ("b".to_string(), "B2".to_string()),
        ],
        "in the order the calls began"
    );
}

#[test]
fn arguments_that_arrive_before_the_name_are_kept_for_the_draft() {
    // A wire that names the call late: what streamed before is not lost.
    let mut app = App::new();
    app.begin_stream();
    assert!(!app.push_tool_call_fragment(&piece(0, None, r#"{"content":"early "#)));
    assert!(app.push_tool_call_fragment(&piece(0, Some("write"), "")));
    app.push_tool_call_fragment(&piece(0, None, "text"));
    let draft = app.drafts().writes().next().expect("visible now");
    assert_eq!(draft.content().unwrap().text, "early text");
}

#[test]
fn every_draft_has_its_own_id() {
    // The renderer's memo keys its highlighting on the id, so a new round's
    // call 0 must never inherit the last round's — nor another App's.
    let first = writing("{");
    let mut second = writing("{");
    second.push_tool_call_fragment(&piece(1, Some("write"), "{"));
    let mut ids: Vec<u64> = first
        .drafts()
        .writes()
        .chain(second.drafts().writes())
        .map(DraftCall::id)
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "ids are unique");
    assert_eq!(count, 3);
}

#[test]
fn would_open_answers_without_folding_the_piece() {
    let mut app = App::new();
    app.begin_stream();
    let naming = piece(0, Some("write"), "");
    assert!(app.drafts().would_open(&naming));
    assert!(app.drafts().is_empty(), "asking changed nothing");
    app.push_tool_call_fragment(&naming);
    assert!(!app.drafts().would_open(&piece(0, None, "{")));
    assert!(!app.drafts().would_open(&piece(1, Some("bash"), "")));
}

#[test]
fn the_rounds_announcement_takes_the_drafts_down() {
    // The calls are whole now: their own cells take over.
    let mut app = writing(r#"{"path":"a.py""#);
    app.open_round(vec![RoundCall {
        id: "c1".to_string(),
        name: "write".to_string(),
        arguments: r#"{"path":"a.py","content":""}"#.to_string(),
    }]);
    assert!(app.drafts().is_empty());
}

#[test]
fn a_batch_or_a_lone_start_takes_the_drafts_down() {
    // The offline dummy announces no round: its batch is the boundary.
    let mut app = writing("{");
    app.start_tool_batch(&[ToolCallSummary {
        name: "Write".to_string(),
        args: "a.py".to_string(),
    }]);
    assert!(app.drafts().is_empty());
    let mut app = writing("{");
    app.start_tool("Write", "a.py", None);
    assert!(app.drafts().is_empty());
}

#[test]
fn an_agent_group_or_a_task_call_takes_the_drafts_down() {
    let mut app = writing("{");
    app.start_agent_group(false, &[]);
    assert!(app.drafts().is_empty());
    let mut app = writing("{");
    app.record_task_call(
        "TaskList",
        "",
        "{}",
        "No tasks found",
        true,
        crate::tasks::TaskStore::default(),
    );
    assert!(app.drafts().is_empty());
}

#[test]
fn every_turn_boundary_takes_the_drafts_down() {
    let mut app = writing("{");
    app.end_turn(1);
    assert!(app.drafts().is_empty(), "a turn that ended");
    let mut app = writing("{");
    app.fail_stream("boom");
    assert!(app.drafts().is_empty(), "a turn that failed");
    let mut app = writing("{");
    app.interrupt_turn();
    assert!(app.drafts().is_empty(), "a turn the user interrupted");
    let mut app = writing("{");
    app.begin_stream();
    assert!(app.drafts().is_empty(), "a fresh turn");
}

#[test]
fn clear_takes_the_drafts_down() {
    let mut app = writing("{");
    type_str(&mut app, "/clear");
    app.on_key(key(KeyCode::Enter)); // run the highlighted /clear
    assert!(app.drafts().is_empty());
}

#[test]
fn an_interrupt_with_only_a_draft_streamed_still_undoes_the_submission() {
    // The draft never became a call — the round was cancelled, so the model
    // never made it — so the turn produced nothing to keep.
    let mut app = App::new();
    app.record_user_message("write me a game");
    app.begin_stream();
    app.push_tool_call_fragment(&piece(0, Some("write"), r#"{"content":"x"#));
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "write me a game");
    assert!(app.drafts().is_empty());
}

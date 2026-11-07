//! A settled thought held on screen until the next cell takes its rows
//! (`docs/thought-handoff.md`).
//!
//! The backend settles a thinking phase at the round's first tool-call piece,
//! but the call has a cell only once its batch is announced. The `● Thinking…`
//! block used to collapse at once — its rows gone, the box rising, then
//! dropping back when the call's cell landed: the flicker reported between a
//! quick think and a `skill` call. The block now settles **in place** — its
//! header becomes the `Thought for …` line, its thought stays under it — and
//! gives its rows to whatever takes the slot next. Nothing else ever holds
//! rows: between rounds the strip is the status line under one spacer.

use super::*;
use crate::app::Reasoning;
use crate::stream::{StreamEvent, ToolCallFragment, ToolCallSummary};
use crate::ui::layout::{live_height_for, live_rows};
use crate::ui::live::preview_lines;

const W: u16 = 80;
const H: u16 = 30;

const THOUGHT: &str = "The user wants the champion.\nUse the skill first.\nThen search.";

/// The composer path's region height for `app` — the boundary's
/// `live_region_height` with no composer-replacing view open.
fn region(app: &App) -> u16 {
    live_height_for(&app.input, W, H, &live_rows(app, W, H))
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(plain).collect()
}

/// The strip's preview rows for `app`, built the way the paint builds them.
fn strip(app: &App) -> Vec<Line<'static>> {
    preview_lines(app, W, None, preview_rows(app, W))
}

/// A turn thinking `text`, its live block in the strip.
fn thinking(text: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(text);
    app
}

/// A turn whose thought `text` has just settled the way `ThinkingEnd` settles
/// it: recorded in history, and held on screen.
fn settled(text: &str) -> (App, Reasoning) {
    let mut app = thinking(text);
    let thought = app.finish_reasoning(0).expect("a thought to settle");
    app.hold_thought(thought.clone());
    (app, thought)
}

fn call(name: &str, args: &str) -> ToolCallSummary {
    ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    }
}

#[test]
fn a_held_thought_stands_on_the_rows_its_block_did() {
    let block = strip(&thinking(THOUGHT));
    let (app, thought) = settled(THOUGHT);
    let held = strip(&app);
    assert_eq!(held.len(), block.len(), "the slot keeps the block's rows");
    assert_eq!(
        plain(&held[0]),
        plain(&reasoning_lines(&thought, W)[0]),
        "the header settles in place into the line scrollback will keep"
    );
    assert_eq!(
        texts(&held[1..]),
        texts(&block[1..]),
        "the thought stays under it until something takes its rows"
    );
    assert!(holds_thought(&app), "nothing has taken the slot yet");
}

#[test]
fn the_box_does_not_move_from_the_thought_to_the_call() {
    // The reported flicker: think → the Skill call.
    let block = region(&thinking(THOUGHT));
    let (mut app, thought) = settled(THOUGHT);
    assert_eq!(region(&app), block, "settling moves nothing");
    // The call is still being generated — the backend settled the thought at
    // its first piece — and a skill call previews nothing while it streams.
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: "skill".into(),
        arguments: String::new(),
    });
    assert!(
        holds_thought(&app),
        "a call still being generated has no cell yet"
    );
    assert_eq!(region(&app), block, "…so the thought keeps its rows");
    // The batch is announced: the call's cell takes the slot, and the held
    // line commits with it — the line and its spacer above, the cell where
    // the thought was.
    app.start_tool_batch(&[call("Skill", "web-search")]);
    assert!(!holds_thought(&app), "the call's cell takes the slot");
    app.drop_held_thought();
    let committed = u16::try_from(reasoning_lines(&thought, W).len() + 1).unwrap_or(u16::MAX);
    assert_eq!(
        region(&app) + committed,
        block,
        "the box ends where it started: no hop up, no drop back"
    );
}

#[test]
fn the_held_thought_gives_way_to_whatever_takes_the_slot() {
    // A `write` streaming into its own cell.
    let (mut app, _) = settled(THOUGHT);
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: "write".into(),
        arguments: String::new(),
    });
    assert!(
        !holds_thought(&app),
        "a streaming write previews in the slot"
    );
    // The reply's first words — but not a reply that is only whitespace yet.
    let (mut app, _) = settled(THOUGHT);
    app.push_chunk("\n\n");
    assert!(holds_thought(&app), "whitespace puts nothing in the slot");
    app.push_chunk("**Classical");
    assert!(!holds_thought(&app), "the reply's frontier takes the slot");
    // A new thought.
    let (mut app, _) = settled(THOUGHT);
    app.begin_reasoning();
    assert!(
        !holds_thought(&app),
        "the next phase's block takes the slot"
    );
    // A modal prompt, which replaces the whole region.
    let (mut app, _) = settled(THOUGHT);
    app.open_permission(crate::permission::PermissionRequest {
        id: "perm_0".to_string(),
        kind: crate::permission::PermissionKind::Write,
        target: "hello.py".to_string(),
        body: "1 print(\"hi\")".to_string(),
        detail: None,
        agent: None,
        agent_id: None,
    });
    assert!(!holds_thought(&app), "a modal prompt replaces the strip");
    // The turn's end, whose summary settles the whole strip.
    let (mut app, _) = settled(THOUGHT);
    app.finish_stream();
    app.take_turn_summary(3);
    assert!(!holds_thought(&app), "the turn is over");
}

#[test]
fn between_rounds_nothing_holds_a_row() {
    // The reported blank rows: a command whose live tail stood taller than
    // its settled peek, resolved between rounds. Nothing comes next until the
    // next request answers, so the strip is the status line alone — the
    // cell's spacer the one blank row above it.
    let (mut app, _) = settled(THOUGHT);
    app.start_tool_batch(&[call("Bash", "python3 search.py")]);
    app.drop_held_thought();
    app.start_tool("Bash", "python3 search.py", None);
    for i in 0..40 {
        app.push_tool_output(&format!("result {i}\n"));
    }
    assert!(preview_rows(&app, W) > 4, "the running tail stands tall");
    app.end_tool("Exit code: 0", true).expect("a running call");
    app.place_resolved_tool();
    assert!(!holds_thought(&app));
    assert_eq!(
        preview_rows(&app, W),
        0,
        "no rows are held between rounds: one spacer, then the status line"
    );
}

#[test]
fn a_viewed_agents_thought_is_held_in_its_own_strip() {
    // The agent session view streams the agent's own `● Thinking…` block and
    // settles it the same way (docs/agent-view-streaming.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Search", false)]);
    app.begin_agent_reasoning("a1");
    app.apply_agent_event("a1", &StreamEvent::ThinkingChunk(THOUGHT.into()));
    app.open_agent_view("a1");
    let block = strip(&app);
    let thought = app.finish_agent_reasoning("a1", 0).expect("a thought");
    app.hold_thought(thought.clone());
    let held = strip(&app);
    assert_eq!(
        held.len(),
        block.len(),
        "the agent's slot keeps the block's rows"
    );
    assert_eq!(plain(&held[0]), plain(&reasoning_lines(&thought, W)[0]));
    assert!(holds_thought(&app));
    app.apply_agent_event("a1", &StreamEvent::ToolBatch(vec![call("Bash", "ls")]));
    assert!(!holds_thought(&app), "the agent's call takes its slot");
}

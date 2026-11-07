//! The strip's **preview floor** (`docs/strip-floor.md`): mid-turn, the live
//! region never rises because a live cell gave way to fewer committed rows
//! than it stood on — the settled `● Thinking…` block before the round's call
//! lands, the collapsed read/search group settling under the call behind it.
//!
//! Each test plays frames the way the boundary paints them: ask the tracker
//! for this frame's floor given the rows committed since the last one, inject
//! it, measure the region, record what the frame reserved.

use super::*;
use crate::ui::layout::{live_height_for, live_rows};
use crate::ui::live::{preview_lines, strip_content_preview_rows};

const W: u16 = 80;
const H: u16 = 30;

/// The composer path's region height for `app` — the boundary's
/// `live_region_height` with no composer-replacing view open.
fn region(app: &App) -> u16 {
    live_height_for(&app.input, W, H, &live_rows(app, W, H))
}

/// One painted frame: inject the floor for `committed` rows queued since the
/// last frame, then record what this frame reserved. Returns the region's
/// height.
fn frame(app: &mut App, floor: &mut PreviewFloor, committed: usize) -> u16 {
    let rows = floor.floor(app, committed, W, H);
    app.set_preview_floor(rows);
    let height = region(app);
    floor.record(app, W, H);
    height
}

/// The invariant the floor exists for: a frame that committed `committed`
/// rows lands the region's bottom no higher than the frame before it.
fn assert_held(before: u16, after: u16, committed: usize, what: &str) {
    assert!(
        usize::from(after) + committed >= usize::from(before),
        "{what}: the box rose — the region went {before} → {after} rows with only \
         {committed} committed above it"
    );
}

/// A turn thinking `text`, its block in the strip.
fn thinking(text: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(text);
    app
}

/// Settle the open phase and return the rows the boundary commits for it —
/// the `Thought for …` line and its spacer.
fn settle(app: &mut App) -> usize {
    let thought = app.finish_reasoning(0).expect("a thought to settle");
    reasoning_lines(&thought, W).len() + 1
}

/// Resolve the running call the way `commit_tool_cell` does and return the
/// rows it commits — its cell and the spacer (0 when it is held in a group).
fn resolve(app: &mut App, output: &str) -> usize {
    app.end_tool(output, true).expect("a running call");
    if app.place_resolved_tool() == crate::app::ToolPlacement::Held {
        return 0;
    }
    tool_commit_lines(&app.history, app.tool_queue(), W, app.path_display())
        .map_or(0, |lines| lines.len() + 1)
}

#[test]
fn a_settled_thought_holds_its_rows_until_the_call_lands() {
    // The reported flicker: think → the Skill call. The block settles at the
    // call's first piece, but the call's cell only arrives with the batch, so
    // for the length of the call's generation the box used to sit three rows
    // up — then drop back when the cell landed.
    let mut app = thinking("The user wants the champion.\nUse the skill first.\nThen search.");
    let mut floor = PreviewFloor::default();
    let block = frame(&mut app, &mut floor, 0);
    let committed = settle(&mut app);
    let held = frame(&mut app, &mut floor, committed);
    assert_held(block, held, committed, "the thought settled");
    // The call is still being generated: nothing commits, nothing moves.
    assert_eq!(
        frame(&mut app, &mut floor, 0),
        held,
        "held while the call streams"
    );
    assert_eq!(frame(&mut app, &mut floor, 0), held, "…every frame of it");
    // The Skill call starts and answers inside one frame (it reads a file):
    // its cell commits whole, the floor lets go, and nothing rose.
    app.start_tool("Skill", "web-search", None);
    app.answer_tool("Successfully loaded skill", "the body")
        .expect("a running call");
    app.place_resolved_tool();
    let cell = tool_commit_lines(&app.history, app.tool_queue(), W, app.path_display())
        .map_or(0, |lines| lines.len() + 1);
    let landed = frame(&mut app, &mut floor, cell);
    assert_held(held, landed, cell, "the call's cell committed");
    assert_eq!(
        preview_rows(&app, W),
        0,
        "nothing is left holding rows once the cell has committed"
    );
}

#[test]
fn the_held_rows_are_blank_and_the_next_cell_lands_in_them() {
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    frame(&mut app, &mut floor, 0);
    let committed = settle(&mut app);
    let held = frame(&mut app, &mut floor, committed);
    let rows = preview_rows(&app, W);
    assert!(rows > 0, "the slot keeps rows for what comes next");
    let lines = preview_lines(&app, W, None, rows);
    assert_eq!(
        lines.len(),
        usize::from(rows),
        "preview_rows sizes what is drawn"
    );
    assert!(
        lines.iter().all(|line| plain(line).trim().is_empty()),
        "the held rows are geometry, not content: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    // The batch announces the call: its two-row cell takes the held rows,
    // and the box does not move at all.
    app.start_tool_batch(&[crate::stream::ToolCallSummary {
        name: "Skill".into(),
        args: "web-search".into(),
    }]);
    assert_eq!(
        frame(&mut app, &mut floor, 0),
        held,
        "the cell lands in the rows the thought left"
    );
    let lines = preview_lines(&app, W, None, preview_rows(&app, W));
    assert!(
        plain(&lines[0]).contains("Skill(web-search)"),
        "the cell heads the slot, never under padding: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
}

#[test]
fn a_thought_giving_way_to_the_reply_pads_under_its_first_row() {
    // Think → answer: the block settles as the reply's first token arrives,
    // and a one-row frontier used to leave the box a row higher until the
    // reply's first line committed.
    let mut app = thinking("Today is the 7th.\nThe match starts on the 24th.\nAnswer.");
    let mut floor = PreviewFloor::default();
    let block = frame(&mut app, &mut floor, 0);
    let committed = settle(&mut app);
    app.push_chunk("**Classical");
    // The boundary's frontier: one row, injected per frame.
    app.set_stream_preview_rows(1);
    let after = frame(&mut app, &mut floor, committed);
    assert_held(block, after, committed, "the thought gave way to the reply");
    let frontier = vec![Line::from("● **Classical")];
    let rows = preview_rows(&app, W);
    let lines = preview_lines(&app, W, Some(&frontier), rows);
    assert_eq!(
        plain(&lines[0]),
        "● **Classical",
        "the frontier stays on top"
    );
    assert!(
        lines[1..].iter().all(|line| plain(line).trim().is_empty()),
        "the padding sits under the frontier, where its next rows land"
    );
    // The reply's first line commits: the floor drops by what committed and
    // the padding goes with it.
    let next = frame(&mut app, &mut floor, 1);
    assert_held(after, next, 1, "the reply's first line committed");
    assert_eq!(preview_rows(&app, W), 1, "no padding outlives the commit");
}

#[test]
fn the_read_group_settling_under_the_next_call_does_not_lift_the_box() {
    // A read, then an edit: the group's two-row live cell settles into one
    // `Read 1 file` line while the edit runs on, a row short of what the
    // strip had held.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[
        crate::stream::ToolCallSummary {
            name: "Read".into(),
            args: "about.py".into(),
        },
        crate::stream::ToolCallSummary {
            name: "Edit".into(),
            args: "about.py".into(),
        },
    ]);
    let mut floor = PreviewFloor::default();
    app.start_tool("Read", "about.py", None);
    frame(&mut app, &mut floor, 0);
    assert_eq!(resolve(&mut app, "1 x"), 0, "the read joins the group");
    let grouped = frame(&mut app, &mut floor, 0);
    // The edit starts: the group closes, its one line commits above the edit.
    app.start_tool("Edit", "about.py", None);
    let range = app.close_tool_group().expect("the group was open");
    let line = tool_group_lines(&app.history[range], W, app.path_display()).len() + 1;
    let after = frame(&mut app, &mut floor, line);
    assert_held(grouped, after, line, "the group settled under the edit");
}

#[test]
fn commits_that_cover_the_slot_release_it_at_once() {
    // A cell that commits at least the rows it stood on leaves no floor: the
    // strip is exactly what is live, and the box moves down by the excess.
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    let block = frame(&mut app, &mut floor, 0);
    app.finish_reasoning(0).expect("a thought");
    let after = frame(&mut app, &mut floor, 40);
    assert_eq!(preview_rows(&app, W), 0, "a large commit holds nothing");
    assert!(after < block);
}

#[test]
fn no_floor_holds_once_the_turn_is_over() {
    // The turn's end collapses the whole strip into its summary — never a
    // floor, or the idle composer would sit under blank rows.
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    frame(&mut app, &mut floor, 0);
    settle(&mut app);
    app.finish_stream();
    app.take_turn_summary(3);
    assert!(!app.turn_active());
    assert_eq!(floor.floor(&app, 0, W, H), 0, "idle holds nothing");
    frame(&mut app, &mut floor, 0);
    assert_eq!(preview_rows(&app, W), 0);
}

#[test]
fn no_floor_holds_under_a_modal_prompt() {
    // A permission prompt replaces the whole region; its close
    // purge-rebuilds. Nothing of the strip's geometry carries across.
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    frame(&mut app, &mut floor, 0);
    let committed = settle(&mut app);
    app.open_permission(crate::permission::PermissionRequest {
        id: "perm_0".to_string(),
        kind: crate::permission::PermissionKind::Write,
        target: "hello.py".to_string(),
        body: "1 print(\"hi\")".to_string(),
        detail: None,
        agent: None,
        agent_id: None,
    });
    assert_eq!(
        floor.floor(&app, committed, W, H),
        0,
        "a modal holds nothing"
    );
    floor.record(&app, W, H);
    // The user answers Yes: the prompt closes, the turn runs on.
    let yes = ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('1'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    );
    app.on_key(yes);
    assert!(!app.modal_open() && app.turn_active());
    assert_eq!(
        floor.floor(&app, 0, W, H),
        0,
        "and what the modal frame recorded holds nothing after it"
    );
}

#[test]
fn the_floor_never_outgrows_the_preview_budget() {
    // On a short terminal the slot's budget is what the composer leaves; a
    // floor past it would squeeze the box — and, as padding taller than the
    // strip's rows, flow blank rows into scrollback with a purge rebuild.
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    frame(&mut app, &mut floor, 0);
    settle(&mut app);
    let short = 9;
    let rows = floor.floor(&app, 0, W, short);
    assert!(
        rows <= preview_budget(&app, W, short),
        "a floor of {rows} past the budget"
    );
    app.set_preview_floor(rows);
    assert_eq!(
        strip_content_preview_rows(&app, W, short),
        fitted_preview_rows(&app, W, short),
        "padding is never content the strip flow could move to scrollback"
    );
    assert!(
        view_flow(&app, W, short, 10_000).is_none(),
        "held rows never flow"
    );
}

#[test]
fn a_held_slot_under_a_live_cell_keeps_the_cell_on_top() {
    // A live cell shorter than the floor — the queued call after a command's
    // tall tail settled into its peek. The strip paints bottom-anchored, so
    // it must be built at the rows the slot reserved: built at the cell's own
    // two, the held rows would land ABOVE the cell, which would then jump up
    // when the floor let go.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[crate::stream::ToolCallSummary {
        name: "Bash".into(),
        args: "python3 about.py".into(),
    }]);
    app.set_preview_floor(4);
    assert_eq!(
        preview_rows(&app, W),
        4,
        "the cell's two rows, held to four"
    );
    let height = region(&app);
    let mut buf = buffer(W, height);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 0, W).contains("Bash(python3 about.py)"),
        "the cell heads the strip: {:?}",
        row(&buf, 0, W)
    );
    for y in 2..4 {
        assert!(
            row(&buf, y, W).trim().is_empty(),
            "row {y} is a held blank under the cell: {:?}",
            row(&buf, y, W)
        );
    }
}

#[test]
fn a_purge_rebuild_starts_the_floor_over() {
    let mut app = thinking("one\ntwo\nthree");
    let mut floor = PreviewFloor::default();
    frame(&mut app, &mut floor, 0);
    settle(&mut app);
    floor.reset();
    assert_eq!(
        floor.floor(&app, 0, W, H),
        0,
        "a rebuild re-seats the region"
    );
}

#[test]
fn a_viewed_agents_thought_holds_its_rows_too() {
    // The agent session view streams the agent's own `● Thinking…` block and
    // settles it the same way (docs/agent-view-streaming.md), so the same
    // bounce lived there.
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Search", false)]);
    app.begin_agent_reasoning("a1");
    app.apply_agent_event("a1", &StreamEvent::ThinkingChunk("one\ntwo\nthree".into()));
    app.open_agent_view("a1");
    let mut floor = PreviewFloor::default();
    let block = frame(&mut app, &mut floor, 0);
    let thought = app.finish_agent_reasoning("a1", 0).expect("a thought");
    let committed = reasoning_lines(&thought, W).len() + 1;
    let after = frame(&mut app, &mut floor, committed);
    assert_held(block, after, committed, "the agent's thought settled");
}

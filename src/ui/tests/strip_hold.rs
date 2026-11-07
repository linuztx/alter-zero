//! The strip hold: a thought that settles ahead of the call it led to keeps
//! its rows, blank, until the call's cell lands on them, so the box never
//! hops up and back between the two — and nothing else is ever held, so no
//! blank row is left between a cell and the status line
//! (`docs/strip-hold.md`).
//!
//! Each test drives the strip through the frames the boundary paints: before
//! a frame it injects [`StripHold::hold`], and after it
//! [`StripHold::record`]s what the frame painted — the two calls `tui::view`
//! makes.

use super::*;
use crate::app::Reasoning;
use crate::stream::ToolCallSummary;
use crate::ui::layout::{hang_rows, strip_slot_rows};
use crate::ui::live::preview_lines;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

/// The boundary's half of the hold, frame by frame.
#[derive(Default)]
struct Frames {
    /// What the boundary keeps between two frames.
    hold: StripHold,
    /// The live region's height in the last frame.
    region: u16,
}

impl Frames {
    /// Paint one frame after `committed` rows went to scrollback since the
    /// last, returning how far the box's bottom **rose** (0 when it held its
    /// place or moved down). The region is content-anchored with room below
    /// it, so a commit pushes its top down by the committed rows and the
    /// bottom ends at `top + committed + region`.
    fn paint(&mut self, app: &mut App, committed: usize) -> u16 {
        app.set_strip_hold(self.hold.hold(app, committed, WIDTH));
        let preview = fitted_preview_rows(app, WIDTH, HEIGHT);
        let region = live_height(
            &app.input,
            WIDTH,
            HEIGHT,
            strip_has_status(app),
            preview,
            hang_rows(app, WIDTH),
            0,
            0,
            0,
            0,
            0,
        );
        // What `render_live_with_preview` asserts, so a hold the paint can't
        // honour fails here rather than only in a debug build's frame.
        assert_eq!(
            preview_lines(app, WIDTH, None, preview).len(),
            usize::from(preview),
            "the reserved preview rows and the drawn ones agree"
        );
        let committed = u16::try_from(committed).unwrap_or(u16::MAX);
        let rose = self.region.saturating_sub(committed.saturating_add(region));
        self.region = region;
        self.hold.record(app, WIDTH, HEIGHT);
        rose
    }
}

/// A turn with a long thought on screen: the `● Thinking…` header over its
/// three newest rows, the block the reported flicker started from.
fn thinking_app() -> App {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("Let me check the champion.\nA skill can search the web.\nLoad it first.");
    app
}

/// The rows the boundary commits when the phase settles: the collapsed
/// `Thought for …` line and its blank spacer (`Session::settle_reasoning`).
fn settle(app: &mut App) -> usize {
    let reasoning: Reasoning = app.finish_reasoning(0).expect("a phase with text settles");
    reasoning_lines(&reasoning, WIDTH).len() + 1
}

fn skill_call() -> Vec<ToolCallSummary> {
    vec![ToolCallSummary {
        name: "Skill".to_string(),
        args: "web-search".to_string(),
    }]
}

/// The rows the boundary commits when the front call resolves: its cell and
/// the blank spacer (`Session::commit_tool_cell`).
fn resolve_skill(app: &mut App) -> usize {
    app.start_tool("Skill", "web-search", Some(r#"{"skill":"web-search"}"#));
    app.answer_tool("Successfully loaded skill", "the skill's body");
    app.place_resolved_tool();
    let lines = tool_commit_lines(
        &app.history,
        app.tool_queue(),
        WIDTH,
        &crate::app::PathDisplay::VERBATIM,
    )
    .expect("the resolved call has a cell");
    lines.len() + 1
}

#[test]
fn a_settled_thought_keeps_the_rows_it_did_not_commit() {
    // The reported flicker: the four-row block collapsed into a one-line
    // cell, the box rose three rows, and a frame or two later the call the
    // model was generating pushed it back down.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    assert_eq!(
        frames.paint(&mut app, committed),
        0,
        "the box rose when the thought settled"
    );
    // Nothing new while the model writes its call: the held rows stay held.
    assert_eq!(frames.paint(&mut app, 0), 0, "the box rose a frame later");
    let held = preview_lines(&app, WIDTH, None, fitted_preview_rows(&app, WIDTH, HEIGHT));
    assert!(
        !held.is_empty(),
        "the slot is held while the call is written"
    );
    assert!(
        held.iter().all(|line| plain(line).trim().is_empty()),
        "the held rows are blank — the rows the next cell lands on: {:?}",
        held.iter().map(plain).collect::<Vec<_>>()
    );
}

#[test]
fn the_call_being_written_keeps_the_hold() {
    // The model writes the call the thought led to before announcing it:
    // its pieces stream into the round's drafts, but a `skill` call
    // previews nothing, so the slot stays empty — and held — the whole time.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    assert_eq!(frames.paint(&mut app, committed), 0);
    for (name, arguments) in [
        ("skill", ""),
        ("", r#"{"skill":"#),
        ("", r#""web-search"}"#),
    ] {
        app.push_tool_call_fragment(&crate::stream::ToolCallFragment {
            index: 0,
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
        assert_eq!(
            frames.paint(&mut app, 0),
            0,
            "the box rose while the call was being written"
        );
    }
    assert!(
        fitted_preview_rows(&app, WIDTH, HEIGHT) > 0,
        "the thought's rows are still held"
    );
}

#[test]
fn the_next_cell_lands_on_the_held_rows() {
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    frames.paint(&mut app, committed);
    let held_region = frames.region;
    app.start_tool_batch(&skill_call());
    assert_eq!(frames.paint(&mut app, 0), 0);
    assert_eq!(
        frames.region, held_region,
        "the Skill cell fits the rows the thought left, so nothing moves"
    );
    let preview = preview_lines(&app, WIDTH, None, fitted_preview_rows(&app, WIDTH, HEIGHT));
    assert!(
        plain(&preview[0]).contains("Skill(web-search)"),
        "the cell opens the slot, where the thought's block was: {:?}",
        preview.iter().map(plain).collect::<Vec<_>>()
    );
    // And it resolves in place: what commits is what leaves the strip.
    let committed = resolve_skill(&mut app);
    assert_eq!(frames.paint(&mut app, committed), 0);
}

#[test]
fn the_whole_thought_to_skill_round_never_lifts_the_box() {
    // Every frame of the reported round, the second round's thought
    // included: think, settle, the call being written, the batch, the
    // answer, the next thought.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    let mut rose = vec![frames.paint(&mut app, 0)];
    let committed = settle(&mut app);
    rose.push(frames.paint(&mut app, committed));
    rose.push(frames.paint(&mut app, 0));
    app.start_tool_batch(&skill_call());
    rose.push(frames.paint(&mut app, 0));
    let committed = resolve_skill(&mut app);
    rose.push(frames.paint(&mut app, committed));
    app.begin_reasoning();
    app.push_thinking("Now search.");
    rose.push(frames.paint(&mut app, 0));
    let committed = settle(&mut app);
    rose.push(frames.paint(&mut app, committed));
    assert!(
        rose.iter().all(|&r| r == 0),
        "the box rose between frames: {rose:?}"
    );
}

#[test]
fn a_reply_after_a_thought_takes_the_slot_without_padding() {
    // A hold only ever fills an empty slot: the reply's first line takes
    // the slot whole, and no blank row is held under it while it streams.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    app.push_chunk("Gukesh");
    frames.paint(&mut app, committed);
    let preview = preview_lines(&app, WIDTH, None, fitted_preview_rows(&app, WIDTH, HEIGHT));
    assert_eq!(
        preview.iter().map(plain).collect::<Vec<_>>(),
        vec!["● Gukesh".to_string()],
        "the reply's line alone, no held row under it"
    );
}

#[test]
fn every_committed_row_releases_a_held_one() {
    // The hold is what scrollback has not taken yet: once the rows the
    // block occupied have all committed, nothing is held.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    frames.paint(&mut app, committed);
    let slot = strip_slot_rows(&app, WIDTH, HEIGHT);
    assert!(slot > 0, "the held slot is painted");
    frames.paint(&mut app, usize::from(slot));
    assert_eq!(
        strip_slot_rows(&app, WIDTH, HEIGHT),
        0,
        "a commit as tall as the hold releases it"
    );
    assert_eq!(app.strip_hold(), 0);
}

#[test]
fn the_hold_ends_with_the_turn() {
    // The turn's end collapses the strip to the idle composer: the one
    // shrink the hold never resists (the turn-end reseat keeps the box put).
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    frames.paint(&mut app, committed);
    assert!(app.strip_hold() > 0, "a hold is up mid-turn");
    // `StreamDone`'s order: the reply finishes, then the summary takes the
    // status line — before the boundary reseats to the idle height.
    app.finish_stream();
    app.take_turn_summary(0);
    assert_eq!(preview_rows(&app, WIDTH), 0, "no status line, no hold");
    assert_eq!(
        strip_slot_rows(&app, WIDTH, HEIGHT),
        0,
        "an idle frame paints no slot, so the next turn starts unheld"
    );
}

#[test]
fn a_hold_never_takes_more_than_the_terminal_has() {
    // A short terminal: the held rows are fitted like any preview, so the
    // padding can never squeeze the composer — nor flow the strip's real
    // rows into scrollback ahead of blank ones.
    let mut app = App::new();
    app.begin_stream();
    app.set_strip_hold(30);
    let height = 10;
    let fitted = fitted_preview_rows(&app, WIDTH, height);
    assert_eq!(fitted, preview_budget(&app, WIDTH, height));
    assert_eq!(
        crate::ui::live::strip_content_preview_rows(&app, WIDTH, height),
        fitted,
        "the strip's content is built at the fitted count — nothing to flow"
    );
    assert_eq!(
        preview_lines(&app, WIDTH, None, fitted).len(),
        usize::from(fitted)
    );
}

#[test]
fn a_modal_frame_paints_no_slot() {
    // A permission prompt replaces the whole region, strip and all: its
    // frame holds nothing for the next one to keep.
    let mut app = thinking_app();
    assert!(strip_slot_rows(&app, WIDTH, HEIGHT) > 0);
    app.open_permission(crate::permission::PermissionRequest {
        id: "perm_0".to_string(),
        kind: crate::permission::PermissionKind::Write,
        target: "hello.py".to_string(),
        body: "1 print(\"hi\")".to_string(),
        detail: None,
        agent: None,
        agent_id: None,
    });
    assert_eq!(strip_slot_rows(&app, WIDTH, HEIGHT), 0);
}

/// A turn whose round ran one command that tailed its output — four rows
/// and the `+N lines (Ns · wait …)` clock under its header, the ctrl+b hint
/// below that — the cell taller than the peek it settles into.
fn running_command_app() -> App {
    let mut app = App::new();
    app.begin_stream();
    start_command(&mut app);
    app
}

/// Announce and start the command [`running_command_app`] runs, its output
/// streamed and its clock past the ctrl+b hint's delay.
fn start_command(app: &mut App) {
    app.start_tool_batch(&[ToolCallSummary {
        name: "Bash".to_string(),
        args: "ping -c 9 host".to_string(),
    }]);
    app.start_tool(
        "Bash",
        "ping -c 9 host",
        Some(r#"{"command":"ping -c 9 host"}"#),
    );
    app.push_tool_output(COMMAND_OUTPUT);
    app.set_command_elapsed(Some(Duration::from_secs(5)));
}

const COMMAND_OUTPUT: &str =
    "reply 1\nreply 2\nreply 3\nreply 4\nreply 5\nreply 6\nreply 7\nreply 8\nreply 9\n";

/// Resolve the running command, returning the rows its cell and spacer
/// commit (`Session::commit_tool_cell`).
fn resolve_command(app: &mut App) -> usize {
    app.end_tool(COMMAND_OUTPUT, true);
    app.place_resolved_tool();
    app.set_command_elapsed(None);
    tool_commit_lines(
        &app.history,
        app.tool_queue(),
        WIDTH,
        &crate::app::PathDisplay::VERBATIM,
    )
    .expect("the resolved call has a cell")
    .len()
        + 1
}

#[test]
fn a_round_ending_on_a_shorter_cell_leaves_the_status_line_one_spacer_under_it() {
    // The reported gap: the command's tailing cell settles into its shorter
    // peek, and nothing follows until the next request comes back. Holding
    // the difference left three blank rows over the status line — the cell's
    // spacer, a held row and the slot's gap — for as long as that took. A
    // call resolving holds nothing: the strip is the status line alone.
    let mut app = running_command_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = resolve_command(&mut app);
    frames.paint(&mut app, committed);
    assert_eq!(
        fitted_preview_rows(&app, WIDTH, HEIGHT),
        0,
        "rows were held after the round's last call settled"
    );
}

#[test]
fn a_thoughts_hold_ends_once_its_call_has_landed() {
    // The thought's rows are held until the call it led to lands on them —
    // and no longer: that call settling at its round's end holds nothing,
    // however much taller its live cell grew meanwhile.
    let mut app = thinking_app();
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let committed = settle(&mut app);
    frames.paint(&mut app, committed);
    start_command(&mut app);
    frames.paint(&mut app, 0);
    let committed = resolve_command(&mut app);
    frames.paint(&mut app, committed);
    assert_eq!(
        fitted_preview_rows(&app, WIDTH, HEIGHT),
        0,
        "the thought's hold outlived the call that landed on it"
    );
}

#[test]
fn a_foreground_agent_tree_settling_never_lifts_the_box() {
    // A tree of two agents at work — a row each for its description and
    // its activity — settles in place: each activity row becomes its
    // `⎿ Done`, so the cell commits exactly the rows the tree held. Pinned
    // here because the tree is the other live cell a turn resolves into
    // scrollback mid-turn, and the hold is what keeps the box still should
    // the two shapes ever drift apart.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            spec("a1", "survey the API", false),
            spec("a2", "read the docs", false),
        ],
    );
    for id in ["a1", "a2"] {
        app.apply_agent_event(
            id,
            &crate::stream::StreamEvent::ToolStart {
                name: "Read".into(),
                args: "README.md".into(),
                detail: None,
                arguments: None,
            },
        );
    }
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    let done = |id: &str| crate::stream::AgentCallDone {
        id: id.into(),
        output: "done".into(),
        ok: true,
    };
    let group = app.finish_agent_group(false, &[done("a1"), done("a2")]);
    let committed = agent_group_lines(&group, WIDTH).len() + 1;
    assert_eq!(
        frames.paint(&mut app, committed),
        0,
        "the box rose when the agent tree settled"
    );
}

#[test]
fn a_read_group_settling_holds_no_row_under_the_call_behind_it() {
    // Verbose output off — what a user starts with: a batch opening on a
    // read collapses it into the read group's live cell, and the call behind
    // it closes the group, which commits its one line while the strip loses
    // the cell (docs/tool-groups.md). The slot still shows the calls, so
    // nothing is held: no blank row sits between them and the status line.
    let batch = |name: &str, args: &str| ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    };
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[
        batch("Read", "about.py"),
        batch("Edit", "about.py"),
        batch("Bash", "python3 about.py"),
    ]);
    app.start_tool("Read", "about.py", Some(r#"{"path":"about.py"}"#));
    app.end_tool("1 print('hi')", true);
    assert_eq!(
        app.place_resolved_tool(),
        crate::app::ToolPlacement::Held,
        "the read joins the open group"
    );
    let mut frames = Frames::default();
    frames.paint(&mut app, 0);
    app.start_tool("Edit", "about.py", Some(r#"{"path":"about.py"}"#));
    let range = app.close_tool_group().expect("the edit closes the group");
    let committed = tool_group_lines(
        &app.history[range],
        WIDTH,
        &crate::app::PathDisplay::VERBATIM,
    )
    .len()
        + 1;
    frames.paint(&mut app, committed);
    let preview = preview_lines(&app, WIDTH, None, fitted_preview_rows(&app, WIDTH, HEIGHT));
    assert!(
        preview
            .last()
            .is_some_and(|line| !plain(line).trim().is_empty()),
        "a blank row was held under the calls: {:?}",
        preview.iter().map(plain).collect::<Vec<_>>()
    );
}

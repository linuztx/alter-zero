//! The folded tool view: the count line in both tenses, the live cell, the
//! folded rebuild walk, and the commits that must add up to it. See
//! `docs/tool-groups.md`.

use super::*;
use crate::app::ToolFold;
use crate::ui::layout::preview_rows;
use crate::ui::live::preview_lines;
use crate::ui::theme::{
    TOOL_BACKGROUND_HINT_DELAY, TOOL_BULLET, TOOL_PULSE_PERIOD, reasoning_label_color,
    tool_running_color,
};
use crate::ui::tool_group::{GroupClock, live_tool_group_lines};

fn ok(name: &str, args: &str) -> ToolCall {
    let output = if name == "Bash" {
        "Exit code: 0\nout"
    } else {
        "1 x"
    };
    tool(name, args, ToolStatus::Ok, output)
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(plain).collect()
}

const PATHS: &PathDisplay = &PathDisplay::VERBATIM;

/// A live clock frame where the bullet shows and no command has run long.
const SHOWN: GroupClock = GroupClock {
    pulse: Some(Duration::ZERO),
    elapsed: None,
    hint_elapsed: None,
};

// ===== the count line =====

#[test]
fn a_settled_group_counts_in_claude_codes_order_and_the_past_tense() {
    let (grep, cat, read, ls) = (
        ok("Bash", "grep -rn foo ."),
        ok("Bash", "cat a.py"),
        ok("Read", "a.py"),
        ok("Bash", "ls -la"),
    );
    assert_eq!(
        plain(&tool_group_line(&[&ls, &read, &grep, &cat], PATHS)),
        "Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to expand)",
        "search, read, list — and `cat a.py` beside a read of a.py is one file"
    );
    let (b, c, src, tests) = (
        ok("Read", "b.py"),
        ok("Bash", "head -5 c.py"),
        ok("Bash", "ls src"),
        ok("Bash", "tree tests"),
    );
    let rg = ok("Bash", "rg bar");
    assert_eq!(
        plain(&tool_group_line(
            &[&read, &b, &c, &src, &tests, &grep, &rg],
            PATHS
        )),
        "Searched for 2 patterns, read 3 files, listed 2 directories (ctrl+o to expand)"
    );
    assert_eq!(
        plain(&tool_group_line(&[&read, &b], PATHS)),
        "Read 2 files (ctrl+o to expand)"
    );
    assert_eq!(
        plain(&tool_group_line(&[&ls], PATHS)),
        "Listed 1 directory (ctrl+o to expand)"
    );
}

#[test]
fn a_settled_group_is_one_dim_bullet_less_line() {
    // The settled-line family `Thought for …` and `Called {server}` belong
    // to: a fact about the turn, not output to read.
    let line = tool_group_line(&[&ok("Bash", "ls")], PATHS);
    assert!(
        !plain(&line).contains(TOOL_BULLET.trim()),
        "{:?}",
        plain(&line)
    );
    assert!(
        line.spans
            .iter()
            .all(|span| span.style.fg == Some(reasoning_label_color())),
        "{line:?}"
    );
}

// ===== the live cell =====

#[test]
fn a_live_group_blinks_over_its_counts_and_names_what_it_is_doing() {
    let (read, ls) = (ok("Read", "a.py"), ok("Bash", "ls -la"));
    let lines = live_tool_group_lines(&[&read, &ls], SHOWN, 80, PATHS);
    assert_eq!(
        texts(&lines),
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)",
            "  ⎿  $ ls -la",
        ]
    );
    // The bullet is the running tool's grey, and on the frame the pulse hides
    // it, blanks of its width keep the text where it was.
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_running_color()));
    let hidden = GroupClock {
        pulse: Some(TOOL_PULSE_PERIOD * 3 / 4),
        ..SHOWN
    };
    let blinked = live_tool_group_lines(&[&read, &ls], hidden, 80, PATHS);
    assert_eq!(
        plain(&blinked[0]),
        "  Reading 1 file, listing 1 directory… (ctrl+o to expand)"
    );
    // A read names its file.
    assert_eq!(
        texts(&live_tool_group_lines(&[&ls, &read], SHOWN, 80, PATHS))[1],
        "  ⎿  a.py"
    );
}

#[test]
fn the_hint_follows_the_running_call_and_shows_its_clock() {
    let read = ok("Read", "a.py");
    let grep = tool("Bash", "grep -r foo /", ToolStatus::Running, "");
    let waiting = tool("Read", "z.py", ToolStatus::Waiting, "");
    let clock = |secs: u64| GroupClock {
        elapsed: Some(Duration::from_secs(secs)),
        ..SHOWN
    };
    let lines = live_tool_group_lines(&[&read, &grep, &waiting], clock(12), 80, PATHS);
    assert_eq!(texts(&lines)[1], "  ⎿  $ grep -r foo / (12s)");
    // A command that has only just started shows no clock yet.
    let fresh = live_tool_group_lines(&[&read, &grep], clock(1), 80, PATHS);
    assert_eq!(texts(&fresh)[1], "  ⎿  $ grep -r foo /");
    // Past the Ctrl+B delay the running command says what the key does.
    let long = GroupClock {
        hint_elapsed: Some(TOOL_BACKGROUND_HINT_DELAY),
        ..clock(12)
    };
    let hinted = texts(&live_tool_group_lines(&[&grep], long, 80, PATHS));
    assert_eq!(
        hinted.last().map(String::as_str),
        Some("     (ctrl+b to run in background)")
    );
}

#[test]
fn a_long_command_hint_is_cut_to_two_rows() {
    let script = ok("Bash", "cat a.txt\n\ncat   b.txt\ncat c.txt");
    let lines = texts(&live_tool_group_lines(&[&script], SHOWN, 80, PATHS));
    assert_eq!(lines[1..], ["  ⎿  $ cat a.txt", "     cat b.txt…"]);
    let wide = ok("Bash", &format!("grep -rn {} .", "x".repeat(200)));
    let lines = live_tool_group_lines(&[&wide], SHOWN, 40, PATHS);
    assert!(lines.iter().all(|l| l.width() <= 40), "{:?}", texts(&lines));
    assert!(plain(&lines[1]).ends_with('…'), "{:?}", texts(&lines));
}

// ===== the folded walk =====

#[test]
fn the_folded_walk_draws_each_run_as_its_line() {
    let history = vec![
        msg(Role::User, "explore"),
        HistoryItem::Tool(ok("Read", "a.py")),
        HistoryItem::Tool(ok("Bash", "ls")),
        msg(Role::Assistant, "Now the tests."),
        HistoryItem::Tool(ok("Bash", "rg test")),
        HistoryItem::Tool(ok("Write", "b.py")),
    ];
    let folded = texts(&conversation_lines_with(
        &history,
        80,
        PATHS,
        ToolFold::On { held_from: None },
    ));
    let at = |needle: &str| {
        folded
            .iter()
            .position(|t| t.starts_with(needle))
            .unwrap_or_else(|| panic!("{needle:?} missing from {folded:#?}"))
    };
    assert!(at("Read 1 file, listed 1 directory") < at("● Now the tests."));
    assert!(at("● Now the tests.") < at("Searched for 1 pattern"));
    assert!(at("Searched for 1 pattern") < at("● Write(b.py)"));
    assert!(
        !folded
            .iter()
            .any(|t| t.starts_with("● Read(") || t.starts_with("● Bash(")),
        "no member keeps a cell of its own: {folded:#?}"
    );
}

#[test]
fn fold_off_draws_every_call_as_before() {
    let history = vec![
        HistoryItem::Tool(ok("Read", "a.py")),
        HistoryItem::Tool(ok("Bash", "ls")),
    ];
    assert_eq!(
        conversation_lines_with(&history, 80, PATHS, ToolFold::Off),
        conversation_lines(&history, 80, PATHS)
    );
}

#[test]
fn a_thought_inside_a_group_is_drawn_above_its_line() {
    // The phase's line committed where it landed — above the live group —
    // so a rebuild puts it there too.
    let history = vec![
        HistoryItem::Tool(ok("Read", "a.py")),
        HistoryItem::Reasoning(crate::app::Reasoning {
            text: "next".to_string(),
            secs: 2,
            tokens: 0,
            timestamp: String::new(),
        }),
        HistoryItem::Tool(ok("Bash", "ls")),
        msg(Role::Assistant, "Done."),
    ];
    let folded = texts(&conversation_lines_with(
        &history,
        80,
        PATHS,
        ToolFold::On { held_from: None },
    ));
    let thought = folded
        .iter()
        .position(|t| t.starts_with("Thought for"))
        .unwrap();
    let group = folded
        .iter()
        .position(|t| t.starts_with("Read 1 file, listed 1 directory"))
        .unwrap();
    assert!(thought < group, "{folded:#?}");
}

#[test]
fn a_failed_member_breaks_out_as_its_own_red_cell() {
    let history = vec![
        HistoryItem::Tool(ok("Read", "a.py")),
        HistoryItem::Tool(tool(
            "Bash",
            "cat missing.txt",
            ToolStatus::Failed,
            "Exit code: 1\ncat: missing.txt: No such file",
        )),
        HistoryItem::Tool(ok("Bash", "ls")),
    ];
    let folded = texts(&conversation_lines_with(
        &history,
        80,
        PATHS,
        ToolFold::On { held_from: None },
    ));
    let read = folded
        .iter()
        .position(|t| t.starts_with("Read 1 file"))
        .unwrap();
    let red = folded
        .iter()
        .position(|t| t.starts_with("● Bash(cat missing.txt)"))
        .unwrap();
    let listed = folded
        .iter()
        .position(|t| t.starts_with("Listed 1 directory"))
        .unwrap();
    assert!(read < red && red < listed, "{folded:#?}");
}

#[test]
fn the_open_group_is_left_to_the_strip() {
    let history = vec![
        HistoryItem::Tool(ok("Read", "a.py")),
        msg(Role::Assistant, "Then:"),
        HistoryItem::Tool(ok("Bash", "ls")),
    ];
    let folded = texts(&conversation_lines_with(
        &history,
        80,
        PATHS,
        ToolFold::On { held_from: Some(2) },
    ));
    assert!(
        folded.iter().any(|t| t.starts_with("Read 1 file")),
        "{folded:#?}"
    );
    assert!(
        !folded.iter().any(|t| t.starts_with("Listed 1 directory")),
        "the held run belongs to the strip: {folded:#?}"
    );
}

#[test]
fn a_held_run_split_by_the_floor_draws_its_committed_part() {
    // Members below the floor already committed as their own line (a
    // subagent launch sealed them, leaving no trace in history yet); the open
    // part starts at `held_from` and is the strip's.
    let history = vec![
        HistoryItem::Tool(ok("Read", "a.py")),
        HistoryItem::Tool(ok("Bash", "ls")),
    ];
    let folded = texts(&conversation_lines_with(
        &history,
        80,
        PATHS,
        ToolFold::On { held_from: Some(1) },
    ));
    assert!(
        folded.iter().any(|t| t.starts_with("Read 1 file (ctrl+o")),
        "{folded:#?}"
    );
    assert!(!folded.iter().any(|t| t.contains("listed")), "{folded:#?}");
}

// ===== the commits =====

/// An app mid-turn.
fn turn() -> App {
    let mut app = App::new();
    app.begin_stream();
    app
}

fn run(app: &mut App, name: &str, args: &str, output: &str, ok: bool) {
    app.start_tool(name, args, None);
    app.end_tool(output, ok);
}

#[test]
fn a_member_that_leaves_the_group_open_commits_nothing() {
    let mut app = turn();
    run(&mut app, "Read", "a.py", "1 x", true);
    assert_eq!(
        folded_tool_commit_lines(&app, 80),
        None,
        "held for the strip"
    );
}

#[test]
fn a_member_the_next_call_ends_commits_the_group_line() {
    let mut app = turn();
    app.start_tool_batch(&[
        crate::stream::ToolCallSummary {
            name: "Read".to_string(),
            args: "a.py".to_string(),
        },
        crate::stream::ToolCallSummary {
            name: "Write".to_string(),
            args: "b.py".to_string(),
        },
    ]);
    run(&mut app, "Read", "a.py", "1 x", true);
    let lines = folded_tool_commit_lines(&app, 80).expect("the write ends it");
    assert_eq!(texts(&lines), ["Read 1 file (ctrl+o to expand)"]);
}

#[test]
fn a_call_that_is_no_member_commits_the_held_line_then_its_own_cell() {
    let mut app = turn();
    run(&mut app, "Read", "a.py", "1 x", true);
    run(&mut app, "Bash", "npm test", "Exit code: 0\nok", true);
    let lines = texts(&folded_tool_commit_lines(&app, 80).expect("committed"));
    assert_eq!(lines[0], "Read 1 file (ctrl+o to expand)");
    assert_eq!(lines[1], "", "the spacer between the two");
    assert!(lines[2].starts_with("● Bash(npm test)"), "{lines:#?}");
}

#[test]
fn verbose_commits_every_call_as_before() {
    let mut app = turn();
    app.settings_mut().verbose = true;
    run(&mut app, "Read", "a.py", "1 x", true);
    assert_eq!(
        folded_tool_commit_lines(&app, 80),
        tool_commit_lines(&app.history, app.tool_queue(), 80, PATHS)
    );
}

#[test]
fn the_held_line_is_there_to_commit_until_it_is_sealed() {
    let mut app = turn();
    assert_eq!(held_tool_group_lines(&app), None);
    run(&mut app, "Bash", "ls", "Exit code: 0\nx", true);
    assert_eq!(
        held_tool_group_lines(&app).map(|lines| texts(&lines)),
        Some(vec!["Listed 1 directory (ctrl+o to expand)".to_string()])
    );
    app.seal_tool_group();
    assert_eq!(held_tool_group_lines(&app), None);
}

/// Play a turn the way the boundary does — a resolution commits through
/// [`folded_tool_commit_lines`], a breaker commits the held line first —
/// collecting every committed row with its spacer.
#[derive(Default)]
struct Scrollback(Vec<String>);

impl Scrollback {
    fn commit(&mut self, lines: Vec<Line<'static>>) {
        if !lines.is_empty() {
            self.0.extend(texts(&lines));
            self.0.push(String::new());
        }
    }

    fn close(&mut self, app: &mut App) {
        if let Some(lines) = held_tool_group_lines(app) {
            self.commit(lines);
        }
        app.seal_tool_group();
    }

    fn resolve(&mut self, app: &mut App, name: &str, args: &str, output: &str, ok: bool) {
        run(app, name, args, output, ok);
        if let Some(lines) = folded_tool_commit_lines(app, 80) {
            self.commit(lines);
            app.seal_tool_group();
        }
    }

    fn say(&mut self, app: &mut App, text: &str) {
        self.close(app);
        app.push_chunk(text);
        if let Some(segment) = app.flush_streaming_segment() {
            self.commit(message_lines(Role::Assistant, &segment, 80));
        }
    }
}

#[test]
fn what_the_commits_wrote_is_what_a_rebuild_draws() {
    let mut app = turn();
    let mut scroll = Scrollback::default();
    scroll.resolve(&mut app, "Bash", "ls -la", "Exit code: 0\nx", true);
    scroll.resolve(&mut app, "Read", "a.py", "1 x", true);
    scroll.resolve(&mut app, "Bash", "cat nope", "Exit code: 1\nno", false);
    scroll.resolve(&mut app, "Bash", "grep -rn x .", "Exit code: 1\n", false);
    scroll.say(&mut app, "Found the module.");
    scroll.resolve(&mut app, "Read", "b.py", "1 y", true);
    scroll.resolve(
        &mut app,
        "Write",
        "c.py",
        "Wrote 1 lines to c.py\n1 z",
        true,
    );
    scroll.resolve(&mut app, "Bash", "rg z", "Exit code: 0\nz", true);
    scroll.close(&mut app);
    let rebuilt = texts(&conversation_lines_with(
        &app.history,
        80,
        PATHS,
        ToolFold::On { held_from: None },
    ));
    assert_eq!(scroll.0, rebuilt);
}

// ===== the strip =====

#[test]
fn the_strip_shows_the_open_group_between_rounds() {
    let mut app = turn();
    run(&mut app, "Read", "a.py", "1 x", true);
    run(&mut app, "Bash", "ls", "Exit code: 0\nx", true);
    let rows = usize::from(preview_rows(&app, 80));
    let lines = texts(&preview_lines(&app, 80, None, preview_rows(&app, 80)));
    assert_eq!(lines.len(), rows, "sized from the same walk it draws");
    assert_eq!(
        lines,
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)",
            "  ⎿  $ ls",
        ]
    );
}

#[test]
fn the_strip_folds_the_queued_members_into_the_open_group() {
    let mut app = turn();
    run(&mut app, "Read", "a.py", "1 x", true);
    app.start_tool_batch(&[
        crate::stream::ToolCallSummary {
            name: "Bash".to_string(),
            args: "rg foo".to_string(),
        },
        crate::stream::ToolCallSummary {
            name: "Bash".to_string(),
            args: "npm test".to_string(),
        },
    ]);
    app.start_tool("Bash", "rg foo", None);
    let lines = texts(&preview_lines(&app, 80, None, preview_rows(&app, 80)));
    assert_eq!(
        lines[..3],
        [
            "● Searching for 1 pattern, reading 1 file… (ctrl+o to expand)",
            "  ⎿  $ rg foo",
            "",
        ],
        "{lines:#?}"
    );
    assert!(lines[3].starts_with("● Bash(npm test)"), "{lines:#?}");
}

#[test]
fn a_thought_between_reads_shows_under_the_open_group() {
    let mut app = turn();
    run(&mut app, "Read", "a.py", "1 x", true);
    app.begin_reasoning();
    app.push_thinking("which file next");
    let lines = texts(&preview_lines(&app, 80, None, preview_rows(&app, 80)));
    assert_eq!(
        lines[0], "● Reading 1 file… (ctrl+o to expand)",
        "{lines:#?}"
    );
    assert_eq!(lines[2], "", "{lines:#?}");
    assert!(lines[3].contains("Thinking"), "{lines:#?}");
}

#[test]
fn a_thought_under_a_live_agent_tree_stays_out_of_the_strip() {
    // Only a group that is open shows the thought under it: a live subagent
    // tree is a cell of its own, and the strip draws it as it always did.
    let mut app = turn();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Survey the repo".into(),
            agent_type: "general-purpose".into(),
            prompt: "survey".into(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    );
    app.begin_reasoning();
    app.push_thinking("while the agent works");
    let lines = texts(&preview_lines(&app, 80, None, preview_rows(&app, 80)));
    assert!(
        lines.iter().all(|row| !row.contains("Thinking")),
        "{lines:#?}"
    );
}

#[test]
fn verbose_keeps_the_strip_one_cell_per_call() {
    let mut app = turn();
    app.settings_mut().verbose = true;
    app.start_tool("Bash", "ls", None);
    let lines = texts(&preview_lines(&app, 80, None, preview_rows(&app, 80)));
    assert!(lines[0].starts_with("● Bash(ls)"), "{lines:#?}");
}

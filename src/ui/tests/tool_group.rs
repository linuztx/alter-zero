//! The collapsed read/search group's two looks and where they show
//! (`docs/tool-groups.md`): the live `● Reading 1 file…` cell over its `⎿`
//! hint in the strip, the settled bullet-less `Read 1 file (ctrl+o to
//! expand)` line in scrollback, and a rebuild that agrees with both.

use super::*;
use crate::app::Reasoning;
use crate::ui::live::preview_lines;
use crate::ui::theme::{TOOL_PULSE_PERIOD, reasoning_label_color, tool_running_color};
use crate::ui::tool_group::{live_group_lines, tool_group_lines};
use crate::ui::wrap::cols;

fn ok(name: &str, args: &str) -> HistoryItem {
    HistoryItem::Tool(tool(name, args, ToolStatus::Ok, "out"))
}

fn thought(secs: u64) -> HistoryItem {
    HistoryItem::Reasoning(Reasoning {
        text: "hmm".to_string(),
        secs,
        tokens: 0,
        timestamp: String::new(),
    })
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(plain).collect()
}

/// An app mid-turn whose open group holds a resolved read of `path`.
fn holding_a_read(path: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Read", path, None);
    app.end_tool("1 x", true);
    assert_eq!(app.place_resolved_tool(), crate::app::ToolPlacement::Held);
    app
}

#[test]
fn a_settled_group_is_one_dim_bullet_less_line_in_the_references_clause_order() {
    let items = [
        ok("Read", "mathutil.py"),
        ok("Bash", "grep -rn add ."),
        ok("Bash", "ls -la"),
    ];
    let lines = tool_group_lines(&items, 100);
    assert_eq!(
        texts(&lines),
        ["Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to expand)"]
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(reasoning_label_color())),
        "dim throughout, the settled thought's dress: {:?}",
        lines[0]
    );
}

#[test]
fn the_clauses_count_in_the_plural_and_the_counts_stand_out() {
    let items = [
        ok("Read", "a.py"),
        ok("Read", "b.py"),
        ok("Bash", "cat c.py"),
        ok("Bash", "ls"),
        ok("Bash", "ls src"),
    ];
    let lines = tool_group_lines(&items, 100);
    assert_eq!(
        texts(&lines),
        ["Read 3 files, listed 2 directories (ctrl+o to expand)"]
    );
    let count = lines[0]
        .spans
        .iter()
        .find(|span| span.content == "3")
        .expect("the count is its own span");
    assert!(count.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn a_thought_after_the_groups_last_read_follows_its_line_and_one_between_is_absorbed() {
    let items = [
        ok("Read", "a.py"),
        thought(2),
        ok("Bash", "ls -la"),
        thought(1),
    ];
    let lines = tool_group_lines(&items, 100);
    assert_eq!(
        texts(&lines),
        [
            "Read 1 file, listed 1 directory (ctrl+o to expand)",
            "",
            "Thought for 1s (ctrl+o to expand)",
        ]
    );
}

#[test]
fn a_narrow_terminal_drops_the_hint_before_it_cuts_the_clauses() {
    let items = [ok("Read", "a.py"), ok("Bash", "ls -la")];
    let line = &tool_group_lines(&items, 34)[0];
    assert_eq!(plain(line), "Read 1 file, listed 1 directory");
    let line = &tool_group_lines(&items, 20)[0];
    assert!(cols(&plain(line)) <= 20, "{:?}", plain(line));
    assert!(plain(line).ends_with('…'), "{:?}", plain(line));
}

#[test]
fn the_conversation_collapses_a_run_of_reads_unless_verbose() {
    let history = vec![
        msg(Role::User, "what does mathutil do?"),
        ok("Read", "mathutil.py"),
        ok("Bash", "ls -la"),
        msg(Role::Assistant, "It adds numbers."),
    ];
    let collapsed = texts(&conversation_lines(
        &history,
        80,
        &PathDisplay::VERBATIM,
        false,
    ));
    assert!(
        collapsed.contains(&"Read 1 file, listed 1 directory (ctrl+o to expand)".to_string()),
        "{collapsed:#?}"
    );
    assert!(
        !collapsed.iter().any(|row| row.starts_with("● Read(")),
        "{collapsed:#?}"
    );
    let verbose = texts(&conversation_lines(
        &history,
        80,
        &PathDisplay::VERBATIM,
        true,
    ));
    assert!(
        verbose
            .iter()
            .any(|row| row.starts_with("● Read(mathutil.py)")),
        "{verbose:#?}"
    );
    assert!(
        verbose.iter().any(|row| row.starts_with("● Bash(ls -la)")),
        "{verbose:#?}"
    );
    assert!(
        !verbose.iter().any(|row| row.contains("listed 1 directory")),
        "{verbose:#?}"
    );
}

#[test]
fn a_call_that_is_not_a_read_keeps_its_cell_between_two_groups() {
    let history = vec![
        ok("Read", "a.py"),
        HistoryItem::Tool(tool("Edit", "a.py", ToolStatus::Ok, "Updated a.py")),
        ok("Read", "a.py"),
    ];
    let rows = texts(&conversation_lines(
        &history,
        80,
        &PathDisplay::VERBATIM,
        false,
    ));
    assert_eq!(
        rows.iter()
            .filter(|row| row.as_str() == "Read 1 file (ctrl+o to expand)")
            .count(),
        2,
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with("● Edit(a.py)")),
        "{rows:#?}"
    );
}

#[test]
fn a_rebuild_leaves_out_the_group_still_open() {
    let history = vec![msg(Role::User, "look"), ok("Read", "a.py")];
    assert_eq!(
        committed_history(&history, &VecDeque::new(), Some(1)).len(),
        1,
        "the held read is the strip's, not scrollback's"
    );
    assert_eq!(committed_history(&history, &VecDeque::new(), None).len(), 2);
}

#[test]
fn the_live_cell_counts_the_held_and_the_running_reads_under_a_blinking_bullet() {
    let mut app = holding_a_read("mathutil.py");
    app.start_tool("Bash", "ls -la", None);
    let lines = live_group_lines(&app, 80, Some(Duration::ZERO), true);
    assert_eq!(
        texts(&lines),
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)",
            "  ⎿  $ ls -la",
        ]
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_running_color()));
    let hidden = live_group_lines(&app, 80, Some(TOOL_PULSE_PERIOD / 2), true);
    assert!(
        plain(&hidden[0]).starts_with("  Reading 1 file"),
        "the bullet blinks, the text holds its column: {:?}",
        plain(&hidden[0])
    );
}

#[test]
fn between_calls_the_open_group_keeps_its_last_read_as_the_hint() {
    let app = holding_a_read("mathutil.py");
    let lines = live_group_lines(&app, 80, Some(Duration::ZERO), true);
    assert_eq!(
        texts(&lines),
        ["● Reading 1 file… (ctrl+o to expand)", "  ⎿  mathutil.py",]
    );
}

#[test]
fn a_search_hint_is_the_command_on_one_row() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool(
        "Bash",
        "grep -rn  \"def add\"   src\ngrep -rn \"def sub\" src",
        None,
    );
    let lines = live_group_lines(&app, 80, None, true);
    assert_eq!(
        texts(&lines),
        [
            "● Searching for 1 pattern… (ctrl+o to expand)",
            "  ⎿  $ grep -rn \"def add\" src …",
        ]
    );
}

#[test]
fn verbose_output_has_no_live_group() {
    let mut app = holding_a_read("a.py");
    app.settings_mut().verbose = true;
    assert!(live_group_lines(&app, 80, None, true).is_empty());
}

#[test]
fn the_strip_shows_the_open_group_between_rounds() {
    // The read resolved and the model is writing its next call: the cell
    // stays up, its bullet still blinking, until something else arrives.
    let app = holding_a_read("mathutil.py");
    let rows = preview_rows(&app, 80);
    let preview = preview_lines(&app, 80, None, rows);
    assert_eq!(usize::from(rows), preview.len());
    assert_eq!(
        texts(&preview),
        ["● Reading 1 file… (ctrl+o to expand)", "  ⎿  mathutil.py",]
    );
}

#[test]
fn the_strip_shows_the_group_over_the_rest_of_its_batch() {
    let mut app = holding_a_read("a.py");
    let batch: Vec<crate::stream::ToolCallSummary> = [("Bash", "ls -la"), ("Write", "b.py")]
        .iter()
        .map(|(name, args)| crate::stream::ToolCallSummary {
            name: (*name).to_string(),
            args: (*args).to_string(),
        })
        .collect();
    app.start_tool_batch(&batch);
    app.start_tool("Bash", "ls -la", None);
    let rows = preview_rows(&app, 80);
    let preview = texts(&preview_lines(&app, 80, None, rows));
    assert_eq!(usize::from(rows), preview.len());
    assert_eq!(
        preview[..4],
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)".to_string(),
            "  ⎿  $ ls -la".to_string(),
            String::new(),
            "● Write(b.py)".to_string(),
        ],
        "{preview:#?}"
    );
    assert!(
        preview.iter().any(|row| row.contains("Waiting…")),
        "{preview:#?}"
    );
}

#[test]
fn a_thinking_phase_inside_the_group_shows_under_its_cell() {
    let mut app = holding_a_read("a.py");
    app.begin_reasoning();
    app.push_thinking("Now the tests.");
    let rows = preview_rows(&app, 80);
    let preview = texts(&preview_lines(&app, 80, None, rows));
    assert_eq!(usize::from(rows), preview.len());
    assert_eq!(preview[0], "● Reading 1 file… (ctrl+o to expand)");
    assert_eq!(preview[2], "", "{preview:#?}");
    assert!(preview[3].contains("Thinking"), "{preview:#?}");
}

#[test]
fn the_permission_prompt_keeps_the_open_group_above_the_call_it_asks_about() {
    let mut app = holding_a_read("a.py");
    app.start_tool_batch(&[crate::stream::ToolCallSummary {
        name: "Bash".to_string(),
        args: "ls -la".to_string(),
    }]);
    app.open_permission(crate::permission::PermissionRequest {
        id: "p1".to_string(),
        kind: crate::permission::PermissionKind::Bash,
        target: "ls -la".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
        agent_id: None,
    });
    let page = texts(&permission_lines(&app, 80, 40));
    assert_eq!(page[0], "● Reading 1 file… (ctrl+o to expand)", "{page:#?}");
    assert!(
        page.iter().any(|row| row.starts_with("● Bash(ls -la)")),
        "{page:#?}"
    );
}

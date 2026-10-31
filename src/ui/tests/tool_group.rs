//! The collapsed read/search group's two looks and where they show
//! (`docs/tool-groups.md`): the live `● Reading 1 file…` cell over its `⎿`
//! hint in the strip, the settled bullet-less `Read 1 file (ctrl+o to
//! expand)` line in scrollback, and a rebuild that agrees with both.

use super::*;
use crate::app::Reasoning;
use crate::ui::live::preview_lines;
use crate::ui::theme::{
    TOOL_PULSE_PERIOD, meta_key_color, meta_value_color, reasoning_label_color, tool_dim_color,
    tool_running_color,
};
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
    let lines = tool_group_lines(&items, 100, &PathDisplay::VERBATIM);
    assert_eq!(
        texts(&lines),
        ["Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to expand)"]
    );
    // The settled thought's dress: its words dim, the counts it reports
    // lifted and the key it names tinted (docs/meta-highlights.md).
    let spans = &lines[0].spans;
    assert_eq!(pieces_in(spans, meta_value_color()), ["1", "1", "1"]);
    assert_eq!(pieces_in(spans, meta_key_color()), ["ctrl+o"]);
    assert_eq!(
        pieces_in(spans, reasoning_label_color()).concat(),
        "Searched for  pattern, read  file, listed  directory ( to expand)",
        "every word stays dim: {spans:?}"
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
    let lines = tool_group_lines(&items, 100, &PathDisplay::VERBATIM);
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
    assert_eq!(
        count.style.fg,
        Some(meta_value_color()),
        "a count is a value, lifted out of the dim"
    );
}

#[test]
fn the_thoughts_a_group_held_leave_only_its_line() {
    // A thought that settled while the group was open shows nowhere inline:
    // the group's line is all the conversation shows of the run, and Ctrl+O
    // has every thought (docs/tool-groups.md *Thoughts*).
    let items = [
        ok("Read", "a.py"),
        thought(2),
        ok("Bash", "ls -la"),
        thought(1),
    ];
    let lines = tool_group_lines(&items, 100, &PathDisplay::VERBATIM);
    assert_eq!(
        texts(&lines),
        ["Read 1 file, listed 1 directory (ctrl+o to expand)"]
    );
}

#[test]
fn only_a_thought_outside_the_group_keeps_its_line() {
    // The thought before the first read is no part of the group; the ones
    // between and after its calls are, and stay out of the conversation —
    // until Verbose output gives every call and every thought a cell again.
    let history = vec![
        msg(Role::User, "look around"),
        thought(3),
        ok("Bash", "ls -la"),
        thought(2),
        ok("Read", "a.py"),
        thought(1),
        msg(Role::Assistant, "Done."),
    ];
    let thoughts = |verbose: bool| -> Vec<String> {
        texts(&conversation_lines(
            &history,
            80,
            &PathDisplay::VERBATIM,
            verbose,
        ))
        .into_iter()
        .filter(|row| row.starts_with("Thought for"))
        .collect()
    };
    assert_eq!(thoughts(false), ["Thought for 3s (ctrl+o to expand)"]);
    assert_eq!(thoughts(true).len(), 3, "{:#?}", thoughts(true));
}

#[test]
fn a_narrow_terminal_drops_the_hint_before_it_cuts_the_clauses() {
    let items = [ok("Read", "a.py"), ok("Bash", "ls -la")];
    let line = &tool_group_lines(&items, 34, &PathDisplay::VERBATIM)[0];
    assert_eq!(plain(line), "Read 1 file, listed 1 directory");
    let line = &tool_group_lines(&items, 20, &PathDisplay::VERBATIM)[0];
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
    // The read has had the row for its hold when the listing starts, so the
    // listing's value is on the row at the next tick (docs/tool-groups.md).
    app.advance_group_hint(Duration::ZERO);
    app.start_tool("Bash", "ls -la", None);
    app.advance_group_hint(crate::app::GROUP_HINT_HOLD);
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
fn a_parallel_batch_shows_its_first_calls_value_first_then_the_next() {
    // docs/tool-groups.md *The hint row*: a read and a listing in one round,
    // both resolved before a frame was drawn. The row used to jump straight
    // to the listing; each value now holds it for GROUP_HINT_HOLD in turn.
    let mut app = App::new();
    app.begin_stream();
    let batch: Vec<crate::stream::ToolCallSummary> = [("Read", "README.md"), ("Bash", "ls -la")]
        .iter()
        .map(|(name, args)| crate::stream::ToolCallSummary {
            name: (*name).to_string(),
            args: (*args).to_string(),
        })
        .collect();
    app.start_tool_batch(&batch);
    for (name, args) in [("Read", "README.md"), ("Bash", "ls -la")] {
        app.start_tool(name, args, None);
        app.end_tool("out", true).expect("running");
        app.place_resolved_tool();
    }
    let t0 = Duration::from_secs(5);
    app.advance_group_hint(t0);
    assert_eq!(
        texts(&live_group_lines(&app, 80, None, true)),
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)",
            "  ⎿  README.md",
        ]
    );
    app.advance_group_hint(t0 + crate::app::GROUP_HINT_HOLD);
    assert_eq!(
        texts(&live_group_lines(&app, 80, None, true)),
        [
            "● Reading 1 file, listing 1 directory… (ctrl+o to expand)",
            "  ⎿  $ ls -la",
        ]
    );
}

#[test]
fn a_command_running_two_seconds_shows_its_clock_on_the_hint_row() {
    // The reference's collapsed group does the same, so a long search does
    // not look frozen: the command's own clock, then how much it has printed.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "grep -rn todo .", None);
    app.set_command_elapsed(Some(Duration::from_millis(1900)));
    let row = |app: &App| texts(&live_group_lines(app, 80, None, true))[1].clone();
    assert_eq!(row(&app), "  ⎿  $ grep -rn todo .");
    app.set_command_elapsed(Some(Duration::from_secs(12)));
    assert_eq!(row(&app), "  ⎿  $ grep -rn todo . (12s)");
    app.push_tool_output("a.rs:1: todo\n");
    assert_eq!(row(&app), "  ⎿  $ grep -rn todo . (12s · 1 line)");
    app.push_tool_output("b.rs:2: todo\n");
    app.set_command_elapsed(Some(Duration::from_secs(65)));
    assert_eq!(row(&app), "  ⎿  $ grep -rn todo . (1m 5s · 2 lines)");
    // A row too narrow for both keeps the whole command and moves the clock,
    // whole, under it.
    assert_eq!(
        texts(&live_group_lines(&app, 34, None, true))[1..],
        ["  ⎿  $ grep -rn todo .", "       (1m 5s · 2 lines)"]
    );
}

#[test]
fn a_hint_rows_clock_lifts_its_values_and_never_the_commands_digits() {
    // docs/meta-highlights.md: the clock's time and line count are values;
    // a `42` in the command itself is nobody's value.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "grep -rn 42 .", None);
    app.push_tool_output("a.rs:1: 42\nb.rs:2: 42\n");
    app.set_command_elapsed(Some(Duration::from_secs(65)));
    let lines = live_group_lines(&app, 80, None, true);
    assert_eq!(plain(&lines[1]), "  ⎿  $ grep -rn 42 . (1m 5s · 2 lines)");
    assert_eq!(
        pieces_in(&lines[1].spans, meta_value_color()),
        ["1m 5s", "2"]
    );
    // Moved to a row of its own, the clock keeps its dress.
    let narrow = live_group_lines(&app, 30, None, true);
    let clock = narrow.last().expect("the clock's own row");
    assert_eq!(plain(clock), "       (1m 5s · 2 lines)");
    assert_eq!(pieces_in(&clock.spans, meta_value_color()), ["1m 5s", "2"]);
}

#[test]
fn the_clock_rides_only_the_running_commands_own_value() {
    // A row still showing an earlier call's value never wears the clock of
    // the command running behind it.
    let mut app = holding_a_read("mathutil.py");
    app.start_tool("Bash", "grep -rn todo .", None);
    app.set_command_elapsed(Some(Duration::from_secs(12)));
    let t0 = Duration::from_secs(5);
    app.advance_group_hint(t0);
    let row = |app: &App| texts(&live_group_lines(app, 80, None, true))[1].clone();
    assert_eq!(row(&app), "  ⎿  mathutil.py");
    app.advance_group_hint(t0 + crate::app::GROUP_HINT_HOLD);
    assert_eq!(row(&app), "  ⎿  $ grep -rn todo . (12s)");
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
fn a_command_hint_shows_every_line_of_the_command() {
    // Nothing on the row is hidden: each line of the command on a row of its
    // own, its spacing as typed, hanging under the text after the `$ `.
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
            "  ⎿  $ grep -rn  \"def add\"   src",
            "       grep -rn \"def sub\" src",
        ]
    );
}

#[test]
fn a_long_command_wraps_under_the_hint_instead_of_being_cut() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool(
        "Bash",
        "grep -rn \"fn render\" src/ui src/app src/tui",
        None,
    );
    assert_eq!(
        texts(&live_group_lines(&app, 34, None, true))[1..],
        [
            "  ⎿  $ grep -rn \"fn render\" src/ui",
            "       src/app src/tui",
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
    app.advance_group_hint(Duration::ZERO);
    app.start_tool_batch(&batch);
    app.start_tool("Bash", "ls -la", None);
    app.advance_group_hint(crate::app::GROUP_HINT_HOLD);
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
fn a_thinking_phase_inside_the_group_leaves_the_strip_to_its_cell() {
    // No `● Thinking…` block while the group is open: the status line says
    // the model is thinking and Ctrl+O shows the thought
    // (docs/tool-groups.md *Thoughts*).
    let mut app = holding_a_read("a.py");
    app.begin_reasoning();
    app.push_thinking("Now the tests.");
    let rows = preview_rows(&app, 80);
    let preview = texts(&preview_lines(&app, 80, None, rows));
    assert_eq!(usize::from(rows), preview.len());
    assert_eq!(
        preview,
        ["● Reading 1 file… (ctrl+o to expand)", "  ⎿  a.py"]
    );
}

#[test]
fn a_thought_with_no_group_open_keeps_its_own_block() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("Before anything is read.");
    let rows = preview_rows(&app, 80);
    let preview = texts(&preview_lines(&app, 80, None, rows));
    assert_eq!(usize::from(rows), preview.len());
    assert_eq!(preview, ["● Thinking…", "  ⎿  Before anything is read."]);
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

// ===== scratchpad edits (docs/tool-groups.md *Scratchpad edits*) =====

/// A session's scratchpad by the layout, so the verbatim policy knows it.
const PAD: &str = "/tmp/alter-zero-0/s1/scratchpad";

fn pad(name: &str) -> String {
    format!("{PAD}/{name}")
}

fn edited(name: &str, path: &str, output: &str) -> HistoryItem {
    HistoryItem::Tool(tool(name, path, ToolStatus::Ok, output))
}

/// One streamed piece of a `write` under slot 0.
fn write_piece(name: &str, arguments: &str) -> crate::stream::ToolCallFragment {
    crate::stream::ToolCallFragment {
        index: 0,
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn scratchpad_edits_settle_as_made_with_the_lines_they_changed() {
    let items = [
        edited(
            "Write",
            &pad("guess.py"),
            "Wrote 55 lines to guess.py\n 1 x",
        ),
        edited(
            "Edit",
            &pad("guess.py"),
            "Updated guess.py (+2 -1)\n3 -a\n3 +b",
        ),
    ];
    let lines = tool_group_lines(&items, 100, &PathDisplay::VERBATIM);
    assert_eq!(
        texts(&lines),
        ["Made 2 scratchpad edits +57 -1 (ctrl+o to expand)"]
    );
    let spans = &lines[0].spans;
    let span = |text: &str| {
        spans
            .iter()
            .find(|span| span.content == text)
            .unwrap_or_else(|| panic!("no {text:?} span in {spans:?}"))
    };
    assert!(span("2").style.add_modifier.contains(Modifier::BOLD));
    // The lines changed are values like the count (docs/meta-highlights.md)
    // — never the diff colours, and never bold like the count.
    for count in ["+57", "-1"] {
        assert_eq!(span(count).style.fg, Some(meta_value_color()), "{count}");
        assert!(!span(count).style.add_modifier.contains(Modifier::BOLD));
    }
}

#[test]
fn a_scratchpad_edit_leads_the_reads_on_their_line() {
    // The reference's order: what the group changed, then what it read.
    let items = [
        ok("Read", "a.py"),
        edited("Write", &pad("guess.py"), "Wrote 55 lines to guess.py"),
    ];
    assert_eq!(
        texts(&tool_group_lines(&items, 100, &PathDisplay::VERBATIM)),
        ["Made 1 scratchpad edit +55, read 1 file (ctrl+o to expand)"]
    );
    // An edit that changed nothing names no lines at all.
    let items = [edited("Write", &pad("guess.py"), "No changes to guess.py")];
    assert_eq!(
        texts(&tool_group_lines(&items, 100, &PathDisplay::VERBATIM)),
        ["Made 1 scratchpad edit (ctrl+o to expand)"]
    );
}

#[test]
fn an_open_scratchpad_edit_is_making_over_its_whole_path() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Write", &pad("guess_the_number.py"), None);
    app.end_tool("Wrote 55 lines to guess_the_number.py", true);
    app.place_resolved_tool();
    let lines = live_group_lines(&app, 100, None, true);
    assert_eq!(
        texts(&lines),
        [
            "● Making 1 scratchpad edit +55… (ctrl+o to expand)",
            "  ⎿  /tmp/alter-zero-0/s1/scratchpad/guess_the_number.py",
        ]
    );
    // Its lines are a value, lifted beside the white clause; the hint's
    // key is tinted and its words stay dim (docs/meta-highlights.md).
    let added = lines[0]
        .spans
        .iter()
        .find(|span| span.content == "+55")
        .expect("a +55 span");
    assert_eq!(added.style.fg, Some(meta_value_color()));
    assert_eq!(pieces_in(&lines[0].spans, meta_key_color()), ["ctrl+o"]);
    assert_eq!(
        pieces_in(&lines[0].spans[1..], tool_dim_color()).concat(),
        " ( to expand)",
        "past the bullet (at rest, the running grey)"
    );
    // Too wide for the row, the path wraps under the corner instead of
    // losing its file name to a cut…
    assert_eq!(
        texts(&live_group_lines(&app, 40, None, true))[1..],
        [
            "  ⎿  /tmp/alter-zero-0/s1/scratchpad/gue",
            "     ss_the_number.py",
        ]
    );
    // …over as many rows as it takes.
    assert_eq!(
        texts(&live_group_lines(&app, 20, None, true))[1..],
        [
            "  ⎿  /tmp/alter-zero",
            "     -0/s1/scratchpa",
            "     d/guess_the_num",
            "     ber.py",
        ]
    );
}

#[test]
fn a_write_streaming_into_the_scratchpad_is_the_group_counting_its_lines() {
    // No cell of its own: the group's line counts the lines as they arrive.
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&write_piece(
        "write",
        &format!(r#"{{"path":"{}","content":"a\nb\nc"#, pad("g.py")),
    ));
    let rows = preview_rows(&app, 80);
    let lines = preview_lines(&app, 80, None, rows);
    assert_eq!(usize::from(rows), lines.len(), "the count is the paint");
    assert_eq!(
        texts(&lines),
        [
            "● Making 1 scratchpad edit +3… (ctrl+o to expand)",
            "  ⎿  /tmp/alter-zero-0/s1/scratchpad/g.py",
        ]
    );
}

#[test]
fn a_scratchpad_write_joins_the_reads_while_it_streams() {
    let mut app = holding_a_read("mathutil.py");
    app.push_tool_call_fragment(&write_piece(
        "write",
        &format!(r#"{{"path":"{}","content":"a\nb"#, pad("g.py")),
    ));
    let rows = preview_rows(&app, 80);
    let lines = preview_lines(&app, 80, None, rows);
    assert_eq!(usize::from(rows), lines.len());
    assert_eq!(
        texts(&lines)[0],
        "● Making 1 scratchpad edit +2, reading 1 file… (ctrl+o to expand)"
    );
    assert!(
        !texts(&lines).iter().any(|row| row.contains("Write(")),
        "{:#?}",
        texts(&lines)
    );
}

#[test]
fn a_write_whose_path_is_not_out_yet_keeps_the_group_over_its_cell() {
    let mut app = holding_a_read("mathutil.py");
    app.push_tool_call_fragment(&write_piece("write", r#"{"pa"#));
    let rows = preview_rows(&app, 80);
    let lines = preview_lines(&app, 80, None, rows);
    assert_eq!(usize::from(rows), lines.len());
    assert_eq!(
        texts(&lines),
        [
            "● Reading 1 file… (ctrl+o to expand)",
            "  ⎿  mathutil.py",
            "",
            "● Write",
            "  ⎿  Writing…",
        ]
    );
}

#[test]
fn a_queued_scratchpad_write_stays_the_group_with_the_lines_it_streamed() {
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&write_piece(
        "write",
        &format!(r#"{{"path":"{}","content":"a\nb\n"}}"#, pad("g.py")),
    ));
    app.start_tool_batch(&[crate::stream::ToolCallSummary {
        name: "Write".to_string(),
        args: pad("g.py"),
    }]);
    let lines = crate::ui::live::preview_tool_lines(&app, 80);
    assert_eq!(
        texts(&lines),
        [
            "● Making 1 scratchpad edit +2… (ctrl+o to expand)",
            "  ⎿  /tmp/alter-zero-0/s1/scratchpad/g.py",
        ]
    );
}

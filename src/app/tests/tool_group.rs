//! The compact tool display's group model (`docs/compact-tools.md`): which
//! calls fold, how a run is found, what the summary says, and the hold /
//! flush decision a resolving call makes.

use super::*;
use crate::stream::ToolCallSummary;

fn call(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        args: args.to_string(),
        status,
        output: output.to_string(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
        call_id: None,
        position: None,
    }
}

fn ok(name: &str, args: &str) -> HistoryItem {
    HistoryItem::Tool(call(name, args, ToolStatus::Ok, "fine"))
}

fn text(role: Role, text: &str) -> HistoryItem {
    HistoryItem::Message(Message {
        role,
        text: text.to_string(),
        timestamp: String::new(),
        images: Vec::new(),
    })
}

fn thought() -> HistoryItem {
    HistoryItem::Reasoning(Reasoning {
        text: "hm".to_string(),
        secs: 1,
        tokens: 2,
        timestamp: String::new(),
    })
}

fn compact_app() -> App {
    let mut app = App::new();
    app.settings_mut().verbose_output = false;
    app
}

// --- membership ---

#[test]
fn a_read_a_listing_a_read_command_and_a_search_are_members() {
    assert_eq!(
        member_of(&call("Read", "/p/a.rs", ToolStatus::Ok, "1 x")),
        Some(Member::Read {
            path: "/p/a.rs".to_string()
        })
    );
    let listing = member_of(&call("Bash", "ls -la", ToolStatus::Ok, "")).expect("a listing");
    assert!(
        matches!(listing, Member::Command { ref command, kinds } if command == "ls -la" && kinds.list)
    );
    let search = member_of(&call("Bash", "rg -n x src", ToolStatus::Ok, "")).expect("a search");
    assert!(matches!(search, Member::Command { kinds, .. } if kinds.search));
    assert!(matches!(
        member_of(&call("Bash", "cat f | head", ToolStatus::Ok, "")),
        Some(Member::Command { kinds, .. }) if kinds.read && !kinds.list
    ));
}

#[test]
fn the_command_is_read_off_the_verbatim_arguments_when_they_exist() {
    let mut tool = call("Bash", "ls -la (summary)", ToolStatus::Ok, "");
    tool.arguments = Some(r#"{"command":"ls -la","description":"List"}"#.to_string());
    assert!(matches!(
        member_of(&tool),
        Some(Member::Command { command, .. }) if command == "ls -la"
    ));
}

#[test]
fn a_write_a_mutating_command_a_shell_run_and_a_picture_are_not_members() {
    assert_eq!(member_of(&call("Write", "x.rs", ToolStatus::Ok, "")), None);
    assert_eq!(member_of(&call("Edit", "x.rs", ToolStatus::Ok, "")), None);
    assert_eq!(
        member_of(&call("Bash", "cargo test", ToolStatus::Ok, "")),
        None
    );
    assert_eq!(
        member_of(&call("Bash", "ls > out", ToolStatus::Ok, "")),
        None
    );
    assert_eq!(member_of(&call("Agent", "look", ToolStatus::Ok, "")), None);
    assert_eq!(member_of(&call("BashSend", "y⏎", ToolStatus::Ok, "")), None);
    let mut shell = call("Bash", "ls", ToolStatus::Ok, "");
    shell.shell = true;
    assert_eq!(
        member_of(&shell),
        None,
        "the user's own `!` command is a cell"
    );
    assert_eq!(
        member_of(&call(
            "Read",
            "/p/cat.png",
            ToolStatus::Ok,
            "Read image (png, 2x2, 1 B)"
        )),
        None,
        "a picture is something to look at"
    );
}

#[test]
fn only_a_successful_member_joins_and_only_a_breaker_ends_a_run() {
    assert!(is_member(&ok("Read", "a")));
    assert!(!is_member(&HistoryItem::Tool(call(
        "Read",
        "a",
        ToolStatus::Failed,
        "no"
    ))));
    assert!(!is_member(&HistoryItem::Tool(call(
        "Bash",
        "ls",
        ToolStatus::Backgrounded,
        ""
    ))));
    assert!(is_skippable(&thought()));
    assert!(!breaks(&thought()));
    assert!(breaks(&text(Role::Assistant, "so")));
    assert!(breaks(&ok("Write", "x")));
    assert!(breaks(&HistoryItem::Tool(call(
        "Bash",
        "ls",
        ToolStatus::Failed,
        "Exit code: 1"
    ))));
}

// --- runs ---

#[test]
fn a_run_reads_through_thoughts_and_stops_at_a_breaker() {
    let history = vec![
        text(Role::User, "look"),
        ok("Read", "/p/a.rs"),
        thought(),
        ok("Bash", "ls -la"),
        thought(),
        text(Role::Assistant, "here"),
        ok("Read", "/p/b.rs"),
    ];
    assert_eq!(run_len(&history), 0, "a user message opens no run");
    assert_eq!(
        run_len(&history[1..]),
        4,
        "two members, the thoughts between and after"
    );
    assert_eq!(
        run_len(&history[2..]),
        0,
        "a run opens on a member, never a thought"
    );
    let trailing: Vec<String> = trailing_run(&history)
        .iter()
        .map(|item| format!("{item:?}"))
        .collect();
    assert_eq!(trailing.len(), 1, "{trailing:?}");
    assert!(trailing[0].contains("b.rs"));
}

#[test]
fn a_failed_member_or_a_breaker_leaves_no_trailing_run() {
    let failed = vec![
        ok("Read", "a"),
        HistoryItem::Tool(call("Read", "b", ToolStatus::Failed, "gone")),
    ];
    assert!(trailing_run(&failed).is_empty());
    let answered = vec![ok("Read", "a"), text(Role::Assistant, "done")];
    assert!(trailing_run(&answered).is_empty());
    let thought_only = vec![text(Role::User, "x"), thought()];
    assert!(trailing_run(&thought_only).is_empty());
}

// --- the tally and the summary ---

#[test]
fn the_summary_counts_searches_reads_and_listings_in_that_order() {
    let items = vec![
        ok("Bash", "ls -la"),
        ok("Read", "/p/a.rs"),
        ok("Bash", "rg -n main src"),
        ok("Read", "/p/b.rs"),
        ok("Read", "/p/a.rs"),
        ok("Bash", "grep -rn x ."),
        ok("Bash", "tree src"),
    ];
    let group = ToolGroup::of(&items);
    assert_eq!(
        (group.searches, group.reads, group.lists, group.members),
        (2, 2, 2, 7)
    );
    assert_eq!(
        group.summary(true),
        "Searching for 2 patterns, reading 2 files, listing 2 directories"
    );
    assert_eq!(
        group.summary(false),
        "Searched for 2 patterns, read 2 files, listed 2 directories"
    );
    assert_eq!(
        group.hint,
        Some(Hint::Command("tree src".to_string())),
        "the newest member's"
    );
}

#[test]
fn a_read_of_the_same_file_twice_is_one_file_and_singulars_read_right() {
    let group = ToolGroup::of(&[ok("Read", "/p/a.rs"), ok("Read", "/p/a.rs")]);
    assert_eq!(group.reads, 1);
    assert_eq!(group.summary(true), "Reading 1 file");
    assert_eq!(group.summary(false), "Read 1 file");
    let one = ToolGroup::of(&[ok("Bash", "ls")]);
    assert_eq!(one.summary(true), "Listing 1 directory");
    assert_eq!(one.summary(false), "Listed 1 directory");
    let search = ToolGroup::of(&[ok("Bash", "rg x")]);
    assert_eq!(search.summary(true), "Searching for 1 pattern");
    assert_eq!(search.summary(false), "Searched for 1 pattern");
    assert_eq!(
        ToolGroup::of(&[ok("Read", "/p/a.rs"), ok("Bash", "ls")]).summary(true),
        "Reading 1 file, listing 1 directory"
    );
    assert!(ToolGroup::of(&[ok("Write", "x")]).is_empty());
}

#[test]
fn a_pipeline_counts_once_as_the_most_specific_thing_it_does() {
    assert_eq!(
        ToolGroup::of(&[ok("Bash", "cat f | grep x")]).summary(false),
        "Searched for 1 pattern"
    );
    assert_eq!(
        ToolGroup::of(&[ok("Bash", "ls | head")]).summary(false),
        "Listed 1 directory"
    );
    assert_eq!(
        ToolGroup::of(&[ok("Bash", "cat f | wc -l")]).summary(false),
        "Read 1 file"
    );
}

// --- the hold / flush decision ---

#[test]
fn a_member_holds_and_a_breaker_that_finds_the_group_open_flushes_before_itself() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Bash", "ls -la", Some(r#"{"command":"ls -la"}"#));
    assert!(
        !app.tool_group_open(),
        "nothing held until a member resolves"
    );
    app.end_tool("Exit code: 0\n.", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::Hold);
    assert!(app.tool_group_open());
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::Hold);
    assert!(app.tool_group_open());
    // A write is a breaker: the boundary learns so before its cell can show…
    assert!(app.tool_group_breaks_on("Write", "/p/b.rs", None));
    assert!(!app.tool_group_breaks_on("Read", "/p/b.rs", None));
    assert!(!app.tool_group_breaks_on("Bash", "cat x", None));
    // …and a breaker that resolves with the group still open flushes it
    // ahead of its own cell: a refused or failed read.
    app.start_tool("Read", "/p/gone.rs", None);
    app.end_tool("no such file", false);
    assert_eq!(app.tool_group_commit(), GroupCommit::FlushBefore);
    assert!(!app.tool_group_open());
    // The next breaker finds nothing open.
    app.start_tool("Write", "/p/b.rs", None);
    app.end_tool("Wrote 1 line", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::Off);
}

#[test]
fn a_member_whose_announced_successor_is_a_breaker_ends_the_group_with_itself() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool_batch(&[
        ToolCallSummary {
            name: "Read".to_string(),
            args: "/p/a.rs".to_string(),
        },
        ToolCallSummary {
            name: "Write".to_string(),
            args: "/p/b.rs".to_string(),
        },
    ]);
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::FlushWith);
    assert!(
        !app.tool_group_open(),
        "the summary never stands through the write's prompt"
    );
    assert!(
        !app.tool_group_breaks_on("Write", "/p/b.rs", None),
        "nothing left to flush"
    );
}

#[test]
fn verbose_output_never_holds_a_cell() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ls -la", None);
    app.end_tool("Exit code: 0", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::Off);
    assert!(!app.tool_group_open());
    assert!(app.live_tool_group().is_none());
    assert!(!app.tool_group_breaks_on("Write", "x", None));
}

#[test]
fn closing_the_group_reports_whether_one_was_open_and_a_rewind_forgets_it() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    assert!(app.close_tool_group());
    assert!(!app.close_tool_group(), "closed once");
    app.start_tool("Read", "/p/b.rs", None);
    app.end_tool("1 y", true);
    assert!(app.tool_group_open());
    app.load_session(Vec::new());
    assert!(!app.tool_group_open());
    assert_eq!(app.tool_group_commit(), GroupCommit::Off);
}

// --- the live group ---

#[test]
fn the_live_group_adds_the_queues_leading_members_and_leads_with_the_running_hint() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    app.start_tool_batch(&[
        ToolCallSummary {
            name: "Bash".to_string(),
            args: "rg -n main src".to_string(),
        },
        ToolCallSummary {
            name: "Read".to_string(),
            args: "/p/b.rs".to_string(),
        },
        ToolCallSummary {
            name: "Write".to_string(),
            args: "/p/c.rs".to_string(),
        },
    ]);
    app.start_tool(
        "Bash",
        "rg -n main src",
        Some(r#"{"command":"rg -n main src"}"#),
    );
    let live = app.live_tool_group().expect("a group shows");
    assert_eq!(
        (live.group.searches, live.group.reads, live.group.lists),
        (1, 2, 0)
    );
    assert_eq!(
        live.queued, 2,
        "the search and the read, not the write behind them"
    );
    assert_eq!(
        live.group.hint,
        Some(Hint::Command("rg -n main src".to_string())),
        "the running call's hint, over the waiting read's"
    );
    assert_eq!(live.running, None, "no clock injected yet");
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert_eq!(
        app.live_tool_group().expect("still shows").running,
        Some(Duration::from_secs(5))
    );
    // The search resolves with the read still queued behind it: held.
    app.end_tool("src/main.rs:1:fn main", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::Hold);
    assert!(app.tool_group_open());
    // The read resolves with the write next: the group ends with it.
    app.end_tool("1 y", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::FlushWith);
    assert!(
        !app.tool_group_open(),
        "the write behind the read closed it"
    );
}

#[test]
fn a_held_group_with_nothing_queued_still_shows_its_newest_members_hint() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Bash", "ls -la", None);
    app.end_tool("Exit code: 0", true);
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    let live = app.live_tool_group().expect("held");
    assert_eq!(live.queued, 0);
    assert_eq!(live.group.hint, Some(Hint::Path("/p/a.rs".to_string())));
    assert_eq!(
        live.group.summary(true),
        "Reading 1 file, listing 1 directory"
    );
    assert_eq!(live.running, None, "no command runs");
}

#[test]
fn switching_verbose_output_mid_turn_reopens_or_releases_the_trailing_run() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    assert!(!app.tool_group_open());
    // Compact on, mid-turn, nothing streaming: the run is held again.
    app.settings_mut().verbose_output = false;
    app.reopen_tool_group();
    assert!(app.tool_group_open());
    // Back to verbose: released.
    app.settings_mut().verbose_output = true;
    app.reopen_tool_group();
    assert!(!app.tool_group_open());
    // Compact on with the reply already streaming: the group is over.
    app.settings_mut().verbose_output = false;
    app.push_chunk("So the file says");
    app.reopen_tool_group();
    assert!(!app.tool_group_open());
}

// --- the hint hold ---

fn summary(name: &str, args: &str) -> ToolCallSummary {
    ToolCallSummary {
        name: name.to_string(),
        args: args.to_string(),
    }
}

/// What the strip's `⎿` row names right now.
fn row(app: &App) -> Option<Hint> {
    app.live_tool_group().and_then(|live| live.group.hint)
}

fn command(text: &str) -> Option<Hint> {
    Some(Hint::Command(text.to_string()))
}

fn path(text: &str) -> Option<Hint> {
    Some(Hint::Path(text.to_string()))
}

#[test]
fn each_value_takes_the_row_in_turn_and_stays_the_minimum_display_time() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool_batch(&[summary("Bash", "ls -la"), summary("Read", "/p/README.md")]);
    app.start_tool("Bash", "ls -la", Some(r#"{"command":"ls -la"}"#));
    app.advance_tool_hint(Duration::from_secs(10));
    assert_eq!(row(&app), command("ls -la"));
    // The listing and the read both resolve within one frame.
    app.end_tool("Exit code: 0", true);
    app.start_tool("Read", "/p/README.md", None);
    app.end_tool("1 x", true);
    assert!(app.tool_group_open());
    assert_eq!(
        app.live_tool_group().expect("held").group.summary(true),
        "Reading 1 file, listing 1 directory"
    );
    // The listing's value stays its minimum before the read's takes the row.
    app.advance_tool_hint(Duration::from_millis(10_050));
    assert_eq!(
        row(&app),
        command("ls -la"),
        "the read's value waits its turn"
    );
    app.advance_tool_hint(Duration::from_millis(10_699));
    assert_eq!(row(&app), command("ls -la"));
    app.advance_tool_hint(Duration::from_millis(10_700));
    assert_eq!(row(&app), path("/p/README.md"), "the minimum has passed");
    // And rests there between rounds.
    app.advance_tool_hint(Duration::from_secs(20));
    assert_eq!(row(&app), path("/p/README.md"));
}

#[test]
fn a_value_on_the_row_past_the_minimum_gives_way_at_once() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Bash", "ls -la", None);
    app.advance_tool_hint(Duration::from_secs(10));
    app.end_tool("Exit code: 0", true);
    app.advance_tool_hint(Duration::from_secs(11));
    assert_eq!(row(&app), command("ls -la"));
    app.start_tool("Read", "/p/a.rs", None);
    app.advance_tool_hint(Duration::from_secs(11));
    assert_eq!(
        row(&app),
        path("/p/a.rs"),
        "a second on the row is more than the minimum"
    );
}

#[test]
fn the_backlog_is_bounded_so_the_row_never_falls_far_behind() {
    let mut app = compact_app();
    app.begin_stream();
    let paths: Vec<String> = (1..=12).map(|n| format!("/p/{n}.rs")).collect();
    app.start_tool("Read", &paths[0], None);
    app.advance_tool_hint(Duration::ZERO);
    app.end_tool("1 x", true);
    for file in &paths[1..] {
        app.start_tool("Read", file, None);
        app.end_tool("1 x", true);
    }
    assert_eq!(row(&app), path(&paths[0]));
    let mut seen = Vec::new();
    for step in 1..=10 {
        app.advance_tool_hint(MIN_HINT_DISPLAY * step);
        if let Some(Hint::Path(shown)) = row(&app)
            && seen.last() != Some(&shown)
        {
            seen.push(shown);
        }
    }
    // The first value shows whole; of the eleven behind it only the newest
    // `HINT_BACKLOG_MAX` waited their turn, the oldest giving way as more
    // arrived, so the row ends on the newest member.
    let expected: Vec<String> = (5..=12).map(|n| format!("/p/{n}.rs")).collect();
    assert_eq!(HINT_BACKLOG_MAX, 8);
    assert_eq!(seen, expected);
}

#[test]
fn a_running_commands_clock_rides_only_its_own_value() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool_batch(&[
        summary("Read", "/p/a.rs"),
        summary("Bash", "find / -name x"),
    ]);
    app.start_tool("Read", "/p/a.rs", None);
    app.advance_tool_hint(Duration::from_secs(10));
    app.end_tool("1 x", true);
    app.start_tool(
        "Bash",
        "find / -name x",
        Some(r#"{"command":"find / -name x"}"#),
    );
    app.set_command_elapsed(Some(Duration::from_secs(4)));
    app.advance_tool_hint(Duration::from_millis(10_100));
    let live = app.live_tool_group().expect("live");
    assert_eq!(
        live.group.hint,
        path("/p/a.rs"),
        "the read's value is still up"
    );
    assert_eq!(
        live.running, None,
        "the clock is the command's, not the value on the row"
    );
    app.advance_tool_hint(Duration::from_millis(10_700));
    let live = app.live_tool_group().expect("live");
    assert_eq!(live.group.hint, command("find / -name x"));
    assert_eq!(live.running, Some(Duration::from_secs(4)));
}

#[test]
fn a_closed_group_forgets_its_hold_so_the_next_opens_on_its_first_value_at_once() {
    let mut app = compact_app();
    app.begin_stream();
    // A member resolves with a write announced behind it: the group ends
    // with it, and the write's turn is no time for the row to catch up on.
    app.start_tool_batch(&[summary("Read", "/p/a.rs"), summary("Write", "/p/w.rs")]);
    app.start_tool("Read", "/p/a.rs", None);
    app.advance_tool_hint(Duration::from_secs(10));
    app.end_tool("1 x", true);
    assert_eq!(app.tool_group_commit(), GroupCommit::FlushWith);
    app.start_tool("Write", "/p/w.rs", None);
    assert_eq!(row(&app), None, "no group while the write runs");
    app.end_tool("Wrote 1 line", true);
    // The next group's first value takes the row at once, the old hold gone.
    app.start_tool("Read", "/p/b.rs", None);
    app.advance_tool_hint(Duration::from_millis(10_050));
    assert_eq!(row(&app), path("/p/b.rs"));
    app.end_tool("1 y", true);
    // Explicitly closed (the boundary's flush ahead of a breaker): the same.
    assert!(app.close_tool_group());
    app.start_tool("Read", "/p/c.rs", None);
    app.advance_tool_hint(Duration::from_millis(10_100));
    assert_eq!(row(&app), path("/p/c.rs"));
}

#[test]
fn a_repeated_value_is_not_queued_again() {
    let mut app = compact_app();
    app.begin_stream();
    app.start_tool("Read", "/p/a.rs", None);
    app.advance_tool_hint(Duration::ZERO);
    app.end_tool("1 x", true);
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    app.start_tool("Read", "/p/b.rs", None);
    app.end_tool("1 y", true);
    app.advance_tool_hint(MIN_HINT_DISPLAY);
    assert_eq!(
        row(&app),
        path("/p/b.rs"),
        "the same file twice is one value"
    );
    app.advance_tool_hint(MIN_HINT_DISPLAY * 2);
    assert_eq!(row(&app), path("/p/b.rs"));
}

#[test]
fn switching_to_compact_mid_turn_shows_the_newest_value_with_no_backlog() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ls -la", None);
    app.end_tool("Exit code: 0", true);
    app.start_tool("Read", "/p/a.rs", None);
    app.end_tool("1 x", true);
    app.settings_mut().verbose_output = false;
    app.reopen_tool_group();
    app.advance_tool_hint(Duration::ZERO);
    assert_eq!(
        row(&app),
        path("/p/a.rs"),
        "verbose output fed no hold: the newest member, not the listing first"
    );
}

//! The inline tool-permission prompt's rendering (`docs/permissions.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::theme::{
    PERMISSION_AGENT_COLOR, PERMISSION_SELECTED_COLOR, PERMISSION_TITLE_COLOR,
};
use super::*;
use crate::permission::{PermissionKind, PermissionRequest};
use crate::stream::{AgentSpec, ToolCallSummary};

fn request(kind: PermissionKind, target: &str, body: &str) -> PermissionRequest {
    PermissionRequest {
        id: "perm_0".to_string(),
        kind,
        target: target.to_string(),
        body: body.to_string(),
        detail: None,
        agent: None,
    }
}

/// An app with `request` open, ready to render.
fn app_with(request: PermissionRequest) -> App {
    let mut app = App::new();
    app.open_permission(request);
    app
}

const WRITE_BODY: &str = "1 #!/usr/bin/env python3\n2 \n3 def main():\n4     print(\"hi\")";
const EDIT_BODY: &str =
    "11      print(\"welcome\")\n12 \n13 -    old = 1\n13 +    new = 2\n14      done()";

/// The plain text of every rendered row.
fn rows(app: &App, width: u16, height: u16) -> Vec<String> {
    permission_lines(app, width, height)
        .iter()
        .map(plain)
        .collect()
}

#[test]
fn a_file_prompt_is_framed_by_rules_around_a_dashed_body() {
    let app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    let lines = rows(&app, 60, 40);
    assert_eq!(lines.first().unwrap(), &"─".repeat(60), "a full-width rule");
    assert_eq!(lines.last().unwrap(), &"─".repeat(60), "…top and bottom");
    let dashed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with('╌'))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(dashed.len(), 2, "the body is framed by two dashed rules");
    assert_eq!(lines[dashed[0]], "╌".repeat(60));
    // Everything between them is the numbered body, in order.
    let body: Vec<&String> = lines[dashed[0] + 1..dashed[1]].iter().collect();
    assert!(
        body.iter().any(|l| l.contains("#!/usr/bin/env python3")),
        "the file's first line shows: {body:?}"
    );
    assert!(
        body.iter().any(|l| l.contains("print(\"hi\")")),
        "…and its last: {body:?}"
    );
}

#[test]
fn the_title_names_the_action_and_the_row_below_it_the_file() {
    let app = app_with(request(PermissionKind::Write, "src/hello.py", WRITE_BODY));
    let lines = rows(&app, 60, 40);
    assert_eq!(lines[2].trim(), "Create file");
    assert_eq!(lines[3].trim(), "src/hello.py", "the path, as given");
    let app = app_with(request(PermissionKind::Edit, "script.py", EDIT_BODY));
    assert_eq!(rows(&app, 60, 40)[2].trim(), "Edit file");
}

#[test]
fn the_title_is_coloured_and_a_subagent_request_says_who_asked() {
    let mut req = request(PermissionKind::Write, "new_script.py", WRITE_BODY);
    req.agent = Some("general-purpose".to_string());
    let app = app_with(req);
    let lines = permission_lines(&app, 80, 40);
    assert_eq!(
        plain(&lines[2]).trim(),
        "Create file · from the general-purpose agent"
    );
    // The action itself is the accent colour; the attribution is dim.
    let title = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("Create file"))
        .expect("the title span");
    assert_eq!(title.style.fg, Some(PERMISSION_TITLE_COLOR));
    let attribution = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("general-purpose"))
        .expect("the attribution span");
    assert_eq!(attribution.style.fg, Some(PERMISSION_AGENT_COLOR));
}

#[test]
fn the_question_and_three_options_sit_under_the_body() {
    let app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    let lines = rows(&app, 70, 40);
    let q = lines
        .iter()
        .position(|l| l.contains("Do you want to create hello.py?"))
        .expect("the question");
    assert_eq!(lines[q + 1], " ❯ 1. Yes");
    assert_eq!(
        lines[q + 2],
        "   2. Yes, allow all edits during this session (a)"
    );
    assert_eq!(lines[q + 3], "   3. No");
}

#[test]
fn the_selected_row_carries_the_marker_and_the_accent_colour() {
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = permission_lines(&app, 70, 40);
    let idx = lines
        .iter()
        .position(|l| plain(l).contains("2. Yes, allow all edits"))
        .expect("the remember row");
    assert!(plain(&lines[idx]).starts_with(" ❯ "), "the ❯ moved down");
    assert!(
        lines[idx]
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty())
            .all(|s| s.style.fg == Some(PERMISSION_SELECTED_COLOR)),
        "the whole selected row lights up — marker and label alike: {:?}",
        lines[idx]
    );
    assert!(
        !plain(&lines[idx - 1]).starts_with(" ❯ "),
        "…and only that row"
    );
}

#[test]
fn a_command_prompt_shows_the_command_its_description_and_the_notice() {
    let mut req = request(PermissionKind::Bash, r#"echo "" | python3 script.py"#, "");
    req.detail = Some("Execute the Python script with empty input".to_string());
    let app = app_with(req);
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[2].trim(), "Bash command");
    assert!(
        !lines.iter().any(|l| l.starts_with('╌')),
        "a command has no framed body: {lines:?}"
    );
    let cmd = lines
        .iter()
        .position(|l| l.contains(r#"echo "" | python3 script.py"#))
        .expect("the command");
    assert_eq!(
        lines[cmd + 1].trim(),
        "Execute the Python script with empty input"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.trim() == "This command requires approval"),
        "{lines:?}"
    );
    let q = lines
        .iter()
        .position(|l| l.contains("Do you want to proceed?"))
        .expect("the question");
    assert_eq!(
        lines[q + 2],
        "   2. Yes, and don't ask again for: python3 script.py (a)"
    );
}

#[test]
fn a_command_with_no_description_skips_that_row() {
    let app = app_with(request(PermissionKind::Bash, "ls -la", ""));
    let lines = rows(&app, 70, 40);
    let cmd = lines
        .iter()
        .position(|l| l.contains("ls -la"))
        .expect("the command");
    assert_eq!(lines[cmd + 1], "", "straight to the gap");
}

#[test]
fn the_hint_row_offers_ctrl_e_only_for_a_command() {
    // …on the third-from-last row: hint, blank, rule.
    let app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    let lines = rows(&app, 70, 40);
    assert_eq!(
        lines[lines.len() - 3].trim(),
        "Esc to cancel · Tab to amend"
    );
    assert_eq!(lines[lines.len() - 2], "");
    let app = app_with(request(PermissionKind::Bash, "ls", ""));
    let lines = rows(&app, 70, 40);
    assert_eq!(
        lines[lines.len() - 3].trim(),
        "Esc to cancel · Tab to amend · ctrl+e to explain"
    );
}

#[test]
fn the_whole_file_shows_when_it_fits() {
    // The point of the prompt is that you read what you approve, so nothing is
    // capped at a peek the way a finished cell is.
    let body: String = (1..=30)
        .map(|n| format!("{n:>2} line {n}\n"))
        .collect::<String>();
    let app = app_with(request(PermissionKind::Write, "big.py", body.trim_end()));
    let lines = rows(&app, 70, 60);
    assert!(lines.iter().any(|l| l.contains("line 1")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("line 30")), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("+")),
        "no `… +N lines` tail when it all fits: {lines:?}"
    );
}

#[test]
fn a_body_too_tall_for_the_terminal_is_capped_so_the_options_stay_visible() {
    let body: String = (1..=200)
        .map(|n| format!("{n:>3} line {n}\n"))
        .collect::<String>();
    let app = app_with(request(PermissionKind::Write, "big.py", body.trim_end()));
    let lines = rows(&app, 70, 24);
    assert!(
        lines.len() <= 24,
        "the prompt fits the terminal: {}",
        lines.len()
    );
    assert!(
        lines.iter().any(|l| l.contains("… +")),
        "the tail says how much is hidden: {lines:?}"
    );
    // Everything below the body still shows.
    assert!(lines.iter().any(|l| l.contains("Do you want to create")));
    assert!(lines.iter().any(|l| l.contains("3. No")));
    assert!(lines.iter().any(|l| l.contains("Esc to cancel")));
}

#[test]
fn the_reserved_height_equals_the_painted_rows() {
    for (kind, target, body, height) in [
        (PermissionKind::Write, "hello.py", WRITE_BODY, 40u16),
        (PermissionKind::Edit, "script.py", EDIT_BODY, 40),
        (PermissionKind::Bash, "ls -la", "", 40),
        (PermissionKind::Write, "hello.py", WRITE_BODY, 12),
    ] {
        let app = app_with(request(kind, target, body));
        let painted = permission_lines(&app, 70, height).len();
        assert_eq!(
            permission_height(&app, 70, height),
            Some(painted as u16),
            "{target} at height {height}"
        );
        // …and the builder is a fixpoint at the height it just reported, so
        // the renderer (which only sees the sized region) paints the same rows.
        assert_eq!(
            permission_lines(&app, 70, painted as u16).len(),
            painted,
            "{target} at height {height} is stable"
        );
    }
}

#[test]
fn amending_replaces_the_options_with_the_composer_and_swaps_the_hint() {
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for c in "use pathlib".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let lines = rows(&app, 70, 40);
    assert!(
        !lines.iter().any(|l| l.contains("1. Yes")),
        "the options gave way: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == " ❯ use pathlib"),
        "the typed feedback shows on the composer row: {lines:?}"
    );
    assert_eq!(
        lines[lines.len() - 3].trim(),
        "Enter to reject with this feedback · Esc to go back"
    );
}

#[test]
fn the_prompt_replaces_the_whole_live_region() {
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    app.begin_stream(); // a turn is in flight — its strip must not show
    let area = Rect::new(0, 0, 60, permission_height(&app, 60, 40).unwrap());
    let mut buf = Buffer::empty(area);
    render_live(area, &mut buf, &app);
    let painted: Vec<String> = (0..area.height).map(|y| row(&buf, y, 60)).collect();
    assert_eq!(painted[0].trim_end(), "─".repeat(60));
    assert!(painted.iter().any(|l| l.contains("Create file")));
    assert!(painted.iter().any(|l| l.contains("1. Yes")), "{painted:?}");
    // No streaming strip and no composer box — the prompt is the whole region.
    assert!(
        !painted.iter().any(|l| l.contains("esc to interrupt")),
        "the status line gave way: {painted:?}"
    );
}

/// The row `permission_lines` puts the `❯` selection marker on, and the
/// hardware cursor's seat — the two must be the same row for every option.
fn marker_row_and_cursor(app: &App, width: u16, term_height: u16) -> (u16, (u16, u16)) {
    let lines = permission_lines(app, width, term_height);
    let marker = lines
        .iter()
        .position(|l| plain(l).starts_with(" ❯ "))
        .expect("the selected option row") as u16;
    let area = Rect::new(
        0,
        0,
        width,
        permission_height(app, width, term_height).unwrap(),
    );
    (marker, cursor_position(area, app))
}

#[test]
fn the_options_show_no_cursor_at_all() {
    // The terminal cursor is the one thing on screen that moves by itself — a
    // kitty cursor-trail animation draws every jump it makes — and an options
    // list is not a text field: there is nothing to point at. So the frame
    // simply doesn't show one, and the trail has nothing to draw.
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    assert!(!cursor_visible(&app), "the option rows show no cursor");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(!cursor_visible(&app), "…however the highlight moves");
    // Tab's amend field is a text field, so the caret comes back with it.
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(cursor_visible(&app), "the amend field is typed into");
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!cursor_visible(&app), "…and goes again with the options");
}

#[test]
fn the_composer_shows_its_cursor() {
    // The default, so the prompt's hiding can't quietly become the rule.
    assert!(cursor_visible(&App::new()));
}

#[test]
fn the_cursor_seat_follows_the_highlighted_option() {
    // Hidden, but not homeless: the seat still tracks the option you are
    // choosing, so a terminal that ignores the hide (and the cursor's return
    // when the prompt closes) starts from somewhere meaningful rather than a
    // corner. Straight up and down — the column is fixed at the option text.
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    let mut seats = Vec::new();
    for step in 0..3 {
        if step > 0 {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let (marker, (x, y)) = marker_row_and_cursor(&app, 70, 40);
        assert_eq!(y, marker, "the cursor sits on the highlighted option row");
        // One inset column + the two-column `❯ ` — where the amend field's
        // text starts too, so Tab never slides the cursor sideways.
        assert_eq!(x, 3, "the column is the option text's first");
        seats.push(y);
    }
    assert_eq!(
        seats,
        vec![seats[0], seats[0] + 1, seats[0] + 2],
        "↓ walks the cursor down one row per option"
    );
}

#[test]
fn the_cursor_seat_follows_the_option_on_a_capped_prompt() {
    // A body too tall for the terminal is capped and the prompt padded to fill
    // it exactly. The padding sits *above* the question — never between the
    // options and the closing rule — so the option block keeps its fixed seat
    // at the bottom and the cursor can be found from that edge.
    let body: String = (1..=200).map(|n| format!("{n} line {n}\n")).collect();
    let mut app = app_with(request(PermissionKind::Write, "big.py", &body));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let (marker, (x, y)) = marker_row_and_cursor(&app, 70, 30);
    assert_eq!(
        permission_lines(&app, 70, 30).len(),
        30,
        "it fills the screen"
    );
    assert_eq!(y, marker, "still on the highlighted option row");
    assert_eq!(x, 3);
}

#[test]
fn the_cursor_sits_at_the_end_of_the_amend_field() {
    let mut app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for c in "why".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let height = permission_height(&app, 70, 40).unwrap();
    let area = Rect::new(0, 0, 70, height);
    let lines = permission_lines(&app, 70, 40);
    let field = lines
        .iter()
        .position(|l| plain(l).starts_with(" ❯ "))
        .expect("the amend row") as u16;
    let (x, y) = cursor_position(area, &app);
    assert_eq!(y, field, "on the amend row");
    // Display columns, not bytes: one inset + the two-column `❯ ` + the text.
    assert_eq!(x, 3 + "why".len() as u16);
}

// --- the context above the prompt (docs/permissions.md) ---
//
// A request must not hide what raised it: the call being asked about (and any
// live agent group) stays visible above the modal, so the prompt reads as a
// question about something on screen rather than a box out of nowhere.

/// An app with one queued `Write` call and a prompt open for it.
fn pending_write() -> App {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[ToolCallSummary {
        name: "Write".to_string(),
        args: "tt.py".to_string(),
    }]);
    app.open_permission(request(PermissionKind::Write, "tt.py", WRITE_BODY));
    app
}

#[test]
fn the_call_being_asked_about_stays_visible_above_the_prompt() {
    let app = pending_write();
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[0], "● Write(tt.py)", "the cell leads the region");
    assert_eq!(lines[1], "  ⎿  Waiting…", "…over its waiting row");
    assert_eq!(lines[2], "", "…then a blank before the frame");
    assert_eq!(lines[3], "─".repeat(70), "…then the prompt's top rule");
}

#[test]
fn the_pending_call_shows_a_waiting_row_like_its_siblings() {
    // Claude Code's look: the call under the prompt renders exactly like a
    // batch sibling — header over a dim ⎿ Waiting…. It genuinely *is* waiting
    // (the approve seam runs before ToolStart, so nothing has started).
    let app = pending_write();
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[0], "● Write(tt.py)");
    assert_eq!(
        lines[1], "  ⎿  Waiting…",
        "the pending call waits like a sibling: {lines:?}"
    );
}

#[test]
fn a_batch_sibling_shows_its_waiting_row_too() {
    // Every not-yet-run call in the batch waits the same way — the one being
    // asked about and the ones queued behind it alike (docs/parallel-tools.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[
        ToolCallSummary {
            name: "Write".to_string(),
            args: "tt.py".to_string(),
        },
        ToolCallSummary {
            name: "Bash".to_string(),
            args: "ls".to_string(),
        },
    ]);
    app.open_permission(request(PermissionKind::Write, "tt.py", WRITE_BODY));
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[0], "● Write(tt.py)");
    assert_eq!(lines[1], "  ⎿  Waiting…");
    assert_eq!(lines[2], "");
    assert_eq!(lines[3], "● Bash(ls)");
    assert_eq!(
        lines[4], "  ⎿  Waiting…",
        "the sibling still waits: {lines:?}"
    );
}

/// An app with `n` queued calls (a `Write` then `Edit` siblings) and a prompt
/// with `body` open for the front one — the shape of a model's big parallel
/// edit batch.
fn pending_batch_with(n: usize, body: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    let mut calls = vec![ToolCallSummary {
        name: "Write".to_string(),
        args: "nexgrad/viz.py".to_string(),
    }];
    for i in 1..n {
        calls.push(ToolCallSummary {
            name: "Edit".to_string(),
            args: format!("nexgrad/file{i}.py"),
        });
    }
    app.start_tool_batch(&calls);
    app.open_permission(request(PermissionKind::Edit, "nexgrad/viz.py", body));
    app
}

fn pending_batch(n: usize) -> App {
    pending_batch_with(n, EDIT_BODY)
}

#[test]
fn a_big_batch_of_waiting_siblings_never_squeezes_out_the_body() {
    // The reported bug: a parallel batch of ~15 edits queues so many
    // `⎿ Waiting…` cells above the prompt that the body's budget saturates to
    // zero — the prompt shows the title, the question and the options with NO
    // content at all, so the user cannot see what edit they are approving.
    // The body is the point of the prompt: the *context* must give way first,
    // the excess siblings collapsing into a dim summary row.
    let app = pending_batch(15);
    let lines = rows(&app, 80, 44);
    assert!(
        lines.len() <= 44,
        "the prompt fits the terminal (options never pushed off): {}",
        lines.len()
    );
    assert_eq!(lines[0], "● Write(nexgrad/viz.py)", "the asked-about call");
    assert_eq!(lines[1], "  ⎿  Waiting…", "…keeps its cell: {lines:?}");
    let dashed = lines.iter().filter(|l| l.starts_with('╌')).count();
    assert_eq!(dashed, 2, "the body keeps its dashed frame: {lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("print(\"welcome\")")),
        "the edit's content shows: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("more waiting")),
        "the collapsed siblings are counted: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("make this edit")),
        "the question survives: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("1. Yes")),
        "…and the options: {lines:?}"
    );
    // The height contract holds under the cap too: the reserved height is the
    // painted rows, and the builder is a fixpoint at that height.
    let painted = permission_lines(&app, 80, 44).len();
    assert_eq!(permission_height(&app, 80, 44), Some(painted as u16));
    assert_eq!(permission_lines(&app, 80, painted as u16).len(), painted);
}

#[test]
fn a_big_batch_keeps_a_minimum_body_even_when_the_body_is_tall() {
    // A tall body under a big batch: the guaranteed floor shows several
    // numbered rows plus the `… +N lines` tail, never nothing.
    let body: String = (1..=120).map(|n| format!("{n:>3} line {n}\n")).collect();
    let app = pending_batch_with(15, body.trim_end());
    let lines = rows(&app, 80, 44);
    assert!(lines.len() <= 44, "{}", lines.len());
    let shown = lines.iter().filter(|l| l.contains(" line ")).count();
    assert!(
        shown >= 5,
        "a real slice of the body shows ({shown} rows): {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("… +") && l.contains("lines")),
        "the body's own tail says what was cut: {lines:?}"
    );
}

#[test]
fn a_small_batch_keeps_every_sibling_with_no_summary() {
    // The ordinary look is untouched: a batch that fits shows every cell and
    // no `… +N more waiting` row.
    let app = pending_batch(3);
    let lines = rows(&app, 80, 44);
    assert!(lines.iter().any(|l| l == "● Edit(nexgrad/file1.py)"));
    assert!(lines.iter().any(|l| l == "● Edit(nexgrad/file2.py)"));
    assert!(
        !lines.iter().any(|l| l.contains("more waiting")),
        "nothing was collapsed: {lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("print(\"welcome\")")));
}

#[test]
fn a_running_call_keeps_its_running_row_under_a_subagents_prompt() {
    // A subagent's request can land while the *main* turn's own tool is
    // genuinely executing — that cell keeps its running state (at rest, no
    // pulse) instead of being reduced to a bare header.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[ToolCallSummary {
        name: "Bash".to_string(),
        args: "sleep 5".to_string(),
    }]);
    app.start_tool("Bash", "sleep 5");
    let mut req = request(PermissionKind::Bash, "ls", "");
    req.agent = Some("general-purpose".to_string());
    app.open_permission(req);
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[0], "● Bash(sleep 5)");
    assert_eq!(
        lines[1], "  ⎿  Running…",
        "a genuinely running call says so: {lines:?}"
    );
}

#[test]
fn a_subagents_request_keeps_the_live_agent_tree_above_it() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            AgentSpec {
                id: "a1".to_string(),
                description: "Agent 1 to run ls -la in /tmp".to_string(),
                agent_type: "general-purpose".to_string(),
                prompt: "p".to_string(),
                background: false,
            },
            AgentSpec {
                id: "a2".to_string(),
                description: "Agent 2 to run ls -la in /tmp".to_string(),
                agent_type: "general-purpose".to_string(),
                prompt: "p".to_string(),
                background: false,
            },
        ],
    );
    let mut req = request(PermissionKind::Bash, "ls -la /tmp", "");
    req.agent = Some("general-purpose".to_string());
    app.open_permission(req);
    let lines = rows(&app, 80, 44);
    assert!(lines[0].contains("Running 2 agents…"), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Agent 1 to run ls -la in /tmp")),
        "the tree rows show: {lines:?}"
    );
    let rule = lines
        .iter()
        .position(|l| l.starts_with('─'))
        .expect("the prompt's top rule");
    assert!(
        lines[..rule].iter().any(|l| l.contains("Agent 2")),
        "the whole tree sits above the prompt: {lines:?}"
    );
    assert!(
        lines[rule..]
            .iter()
            .any(|l| l.contains("Bash command · from the general-purpose agent")),
        "{lines:?}"
    );
}

#[test]
fn an_idle_prompt_has_no_context_rows() {
    // Nothing raised it on screen (the dummy's scripted turn, a resumed
    // session): the prompt still opens with its own rule.
    let app = app_with(request(PermissionKind::Write, "hello.py", WRITE_BODY));
    assert_eq!(rows(&app, 70, 40)[0], "─".repeat(70));
}

// --- the conversation replay above a covering prompt (docs/permissions.md) ---
//
// Once the prompt has to take conversation rows (a full screen), the modal
// spans the whole terminal and the boundary hands the render the conversation
// tail: the newest rows repaint above the live cells + prompt, so opening a
// prompt never hides the messages the user just read — the Claude Code look.

/// A caller-built conversation tail of `n` numbered rows.
fn fake_tail(n: usize) -> Vec<Line<'static>> {
    (0..n)
        .map(|i| Line::from(format!("tail row {i}")))
        .collect()
}

#[test]
fn the_conversation_tail_replays_above_a_covering_prompt() {
    let app = pending_write();
    let p = permission_height(&app, 70, 40).unwrap();
    assert!(p < 40, "the prompt leaves room for a replay at this size");
    let tail = fake_tail(50);
    let area = Rect::new(0, 0, 70, 40);
    let mut buf = Buffer::empty(area);
    render_permission_with_context(area, &mut buf, &app, &tail);
    let budget = usize::from(40 - p);
    assert_eq!(
        row(&buf, 0, 70).trim_end(),
        format!("tail row {}", 50 - budget),
        "the newest tail rows that fit, oldest first"
    );
    assert_eq!(
        row(&buf, (40 - p) - 1, 70).trim_end(),
        "tail row 49",
        "…down to the very newest, hugging the prompt"
    );
    assert_eq!(
        row(&buf, 40 - p, 70).trim_end(),
        "● Write(tt.py)",
        "the prompt block sits right below the replay"
    );
    assert_eq!(
        row(&buf, 39, 70).trim_end(),
        "─".repeat(70),
        "…flush at the bottom"
    );
}

#[test]
fn a_short_tail_pads_above_so_the_prompt_stays_flush() {
    let app = pending_write();
    let p = permission_height(&app, 70, 40).unwrap();
    let tail = fake_tail(1);
    let area = Rect::new(0, 0, 70, 40);
    let mut buf = Buffer::empty(area);
    render_permission_with_context(area, &mut buf, &app, &tail);
    assert_eq!(row(&buf, 0, 70).trim_end(), "", "blank padding above");
    assert_eq!(
        row(&buf, (40 - p) - 1, 70).trim_end(),
        "tail row 0",
        "the short tail hugs the prompt"
    );
    assert_eq!(row(&buf, 39, 70).trim_end(), "─".repeat(70));
}

#[test]
fn a_region_sized_to_the_prompt_shows_no_replay() {
    // The prompt fits below the conversation (an early-session screen): the
    // region is exactly the prompt, and the tail the caller supplied has no
    // rows to fill — nothing of it may paint over the prompt.
    let app = pending_write();
    let p = permission_height(&app, 70, 40).unwrap();
    let tail = vec![Line::from("must not show")];
    let area = Rect::new(0, 0, 70, p);
    let mut buf = Buffer::empty(area);
    render_permission_with_context(area, &mut buf, &app, &tail);
    let painted: Vec<String> = (0..p).map(|y| row(&buf, y, 70)).collect();
    assert!(
        !painted.iter().any(|l| l.contains("must not show")),
        "{painted:?}"
    );
    assert_eq!(painted[0].trim_end(), "● Write(tt.py)");
    assert_eq!(painted.last().unwrap().trim_end(), &"─".repeat(70));
}

#[test]
fn the_context_rows_count_against_the_body_budget() {
    // The reserved height must still equal the painted rows once the cell
    // above the prompt takes its share of the terminal.
    let app = pending_write();
    for height in [40u16, 24, 18] {
        let painted = permission_lines(&app, 70, height).len();
        assert_eq!(permission_height(&app, 70, height), Some(painted as u16));
        assert_eq!(
            permission_lines(&app, 70, painted as u16).len(),
            painted,
            "stable at height {height}"
        );
        assert!(painted <= usize::from(height), "fits at height {height}");
    }
}

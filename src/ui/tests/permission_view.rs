//! The inline tool-permission prompt's rendering (`docs/permissions.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::theme::{
    PERMISSION_AGENT_COLOR, PERMISSION_DETAIL_COLOR, PERMISSION_OPTION_MAX_ROWS,
    PERMISSION_SELECTED_COLOR, PERMISSION_TITLE_COLOR,
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
        agent_id: None,
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
        "   2. Yes, allow all edits during this session (shift+tab)"
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
        "   2. Yes, and don't ask again for: python3 *",
        "the rule is the program with the any-arguments star, no (a) shortcut"
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
        !lines.iter().any(|l| l.contains("… +")),
        "no `… +N lines` tail when it all fits: {lines:?}"
    );
}

#[test]
fn the_reserved_height_equals_the_painted_rows() {
    // The region is the page clamped to the terminal, and the builder is a
    // **fixpoint at the height the region actually gets** — the renderer only
    // ever sees the sized region, so both must produce the same page whether
    // the page fits (region = page) or flows (region = terminal). The
    // (WRITE_BODY, 12) case flows: its page overflows a 12-row terminal.
    for (kind, target, body, height) in [
        (PermissionKind::Write, "hello.py", WRITE_BODY, 40u16),
        (PermissionKind::Edit, "script.py", EDIT_BODY, 40),
        (PermissionKind::Bash, "ls -la", "", 40),
        (PermissionKind::Write, "hello.py", WRITE_BODY, 12),
    ] {
        let app = app_with(request(kind, target, body));
        let painted = permission_lines(&app, 70, height).len();
        let region = painted.min(usize::from(height)) as u16;
        assert_eq!(
            permission_height(&app, 70, height),
            Some(region),
            "{target} at height {height}"
        );
        assert_eq!(
            permission_lines(&app, 70, region).len(),
            painted,
            "{target} at height {height} is stable at the region's height"
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

/// A command long enough that its exact-scope remember label cannot fit one
/// 60-column row — the wrap cases below all share it.
const LONG_EXACT: &str = "cat /very/long/path/segment/one/two/three/four/five.txt > /tmp/some/deep/output/location/result-file.txt";

#[test]
fn a_long_remember_option_wraps_instead_of_hiding_its_tail() {
    // An exact-only scope puts the WHOLE command in option 2 — a long one
    // used to be cut at the width with a `…`, hiding exactly the text being
    // approved. It now word-wraps like the command body, continuations
    // aligned under the label text.
    let mut app = app_with(request(PermissionKind::Bash, LONG_EXACT, ""));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)); // select it
    let lines = rows(&app, 60, 44);
    let start = lines
        .iter()
        .position(|l| l.contains("2. Yes, and don't ask again for:"))
        .expect("the remember row");
    let block: Vec<&String> = lines[start..]
        .iter()
        .take_while(|l| !l.contains("3. No"))
        .collect();
    assert!(block.len() > 1, "the long label wrapped: {block:?}");
    let joined = block
        .iter()
        .map(|l| l.trim().trim_start_matches("❯ ").to_string())
        .collect::<Vec<_>>()
        .join(" ");
    for token in format!("2. Yes, and don't ask again for: {LONG_EXACT}").split_whitespace() {
        assert!(joined.contains(token), "token {token} hidden: {joined}");
    }
    assert!(
        !joined.contains('…'),
        "wrapping replaced the truncation: {joined}"
    );
    // Continuations sit under the label text (past the marker and number),
    // marker-free — only the first row carries the `❯`.
    for row in &block[1..] {
        assert!(row.starts_with("      "), "aligned continuation: {row:?}");
        assert!(!row.contains('❯'), "one marker per option: {row:?}");
    }
}

#[test]
fn a_pathological_option_label_caps_its_rows_instead_of_eating_the_screen() {
    // Wrapping fixed the hidden tail, but an unbounded wrap has the opposite
    // failure: a kilobytes-long exact command would stack enough option rows
    // to push `3. No` and the hints off the terminal bottom (the region is
    // clamped to the screen and painted top-down). The label caps at
    // PERMISSION_OPTION_MAX_ROWS with the familiar `…` — still several rows
    // of context where the old cut showed one.
    let huge = format!("cat {} > /tmp/out", "a/very/long/path/segment ".repeat(80));
    let mut app = app_with(request(PermissionKind::Bash, huge.trim(), ""));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = rows(&app, 60, 44);
    let start = lines
        .iter()
        .position(|l| l.contains("2. Yes, and don't ask again for:"))
        .expect("the remember row");
    let block: Vec<&String> = lines[start..]
        .iter()
        .take_while(|l| !l.contains("3. No"))
        .collect();
    assert_eq!(
        block.len(),
        PERMISSION_OPTION_MAX_ROWS,
        "the block caps: {block:?}"
    );
    assert!(
        block.last().unwrap().ends_with('…'),
        "the cap is honest about the cut: {:?}",
        block.last()
    );
    // The rest of the prompt survives on screen.
    assert!(lines.iter().any(|l| l.contains("3. No")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("Esc to cancel")),
        "{lines:?}"
    );
}

#[test]
fn every_wrapped_row_of_the_selected_option_lights_up() {
    let mut app = app_with(request(PermissionKind::Bash, LONG_EXACT, ""));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = permission_lines(&app, 60, 44);
    let start = lines
        .iter()
        .position(|l| plain(l).contains("2. Yes, and don't ask again for:"))
        .expect("the remember row");
    let block: Vec<_> = lines[start..]
        .iter()
        .take_while(|l| !plain(l).contains("3. No"))
        .collect();
    assert!(block.len() > 1, "the long label wrapped");
    for line in &block {
        assert!(
            line.spans
                .iter()
                .filter(|s| !s.content.trim().is_empty())
                .all(|s| s.style.fg == Some(PERMISSION_SELECTED_COLOR)),
            "every wrapped row wears the selection colour: {line:?}"
        );
    }
}

#[test]
fn the_cursor_seat_survives_wrapped_options() {
    // Option 2 wraps to several rows; the seat still lands on each option's
    // FIRST row — the one carrying the `❯` — stepping past the wrapped block
    // to reach No.
    let mut app = app_with(request(PermissionKind::Bash, LONG_EXACT, ""));
    let mut seats = Vec::new();
    for step in 0..3 {
        if step > 0 {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let (marker, (x, y)) = marker_row_and_cursor(&app, 60, 44);
        assert_eq!(y, marker, "the cursor sits on the highlighted ❯ row");
        assert_eq!(x, 3);
        seats.push(y);
    }
    assert!(
        seats[2] - seats[1] > 1,
        "No sits past the wrapped remember block: {seats:?}"
    );
    assert_eq!(
        seats[1] - seats[0],
        1,
        "Yes → remember is one row: {seats:?}"
    );
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
fn the_cursor_seat_follows_the_option_on_a_flowing_prompt() {
    // A page taller than the terminal bottom-anchors (docs/view-flow.md): the
    // tail block — question, options, hints, rule — closes the page, so the
    // anchor keeps it flush at the region's bottom and the cursor is still
    // found from that edge. The marker's *page* row maps to its screen row by
    // the skipped top (`view_body_skip`).
    let body: String = (1..=200).map(|n| format!("{n} line {n}\n")).collect();
    let mut app = app_with(request(PermissionKind::Write, "big.py", &body));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let (width, height) = (70u16, 30u16);
    let lines = permission_lines(&app, width, height);
    let skip = lines.len() - usize::from(height);
    assert!(skip > 0, "the page flows: {} rows", lines.len());
    let marker = lines
        .iter()
        .position(|l| plain(l).starts_with(" ❯ "))
        .expect("the selected option row");
    assert!(marker >= skip, "the option block is in the painted tail");
    let area = Rect::new(0, 0, width, permission_height(&app, width, height).unwrap());
    let (x, y) = cursor_position(area, &app);
    assert_eq!(
        y,
        (marker - skip) as u16,
        "the seat lands on the painted ❯ row"
    );
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

// (A tall body under a big batch is `an_overflowing_prompt_drops_its_context_cells`
// below: the page flows whole and the context gives way — docs/view-flow.md.)

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
    app.start_tool("Bash", "sleep 5", None);
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

// --- the prompt rendered plain (docs/permissions.md) ---
//
// The region is sized to exactly the prompt's rows, and the conversation
// above it is the *real* screen — the prompt grew by scrolling like any other
// region, so nothing needs replaying into the render.

#[test]
fn the_render_paints_exactly_the_prompt_rows() {
    let app = pending_write();
    let p = permission_height(&app, 70, 40).unwrap();
    let area = Rect::new(0, 0, 70, p);
    let mut buf = Buffer::empty(area);
    render_permission(area, &mut buf, &app);
    let painted: Vec<String> = (0..p).map(|y| row(&buf, y, 70)).collect();
    assert_eq!(painted[0].trim_end(), "● Write(tt.py)");
    assert_eq!(painted.last().unwrap().trim_end(), &"─".repeat(70));
}

#[test]
fn the_context_rows_count_against_the_body_budget() {
    // The height contract holds once the cell above the prompt takes its
    // share of the terminal: the region is the page clamped to the terminal,
    // and the builder is a fixpoint at that region's own height — including
    // the short terminals where the page (context and all) flows.
    let app = pending_write();
    for height in [40u16, 24, 18] {
        let painted = permission_lines(&app, 70, height).len();
        let region = painted.min(usize::from(height)) as u16;
        assert_eq!(permission_height(&app, 70, height), Some(region));
        assert_eq!(
            permission_lines(&app, 70, region).len(),
            painted,
            "stable at height {height}"
        );
        // The cell rides the page at every height — dropping it would leave
        // the prompt a box out of nowhere (docs/permissions.md).
        let text: Vec<String> = permission_lines(&app, 70, height)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(text[0], "● Write(tt.py)", "at height {height}: {text:?}");
    }
}

// --- the MCP prompt (docs/mcp.md) ---

/// An app asking about a `deepwiki - read_wiki_structure` call, with the
/// session info the footer (and option 2's project clause) reads.
fn pending_mcp() -> App {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/Codes/tests");
    app.start_tool_batch(&[ToolCallSummary {
        name: "deepwiki - read_wiki_structure (MCP)".to_string(),
        args: r#"{"repoName":"linuztx/flaredantic"}"#.to_string(),
    }]);
    let mut request = request(
        PermissionKind::Mcp,
        "mcp__deepwiki__read_wiki_structure",
        r#"repoName: "linuztx/flaredantic""#,
    );
    request.detail =
        Some("Get a list of documentation topics for a GitHub repository.".to_string());
    app.open_permission(request);
    app
}

#[test]
fn an_mcp_prompt_reads_as_the_call_it_is_about() {
    let text = rows(&pending_mcp(), 76, 30);
    let body = text
        .iter()
        .position(|r| {
            r.trim_start()
                .starts_with("Deepwiki - read_wiki_structure(")
        })
        .expect("the call names the tool, the server and its arguments");
    // The title is the act, the call is the body — no framed JSON.
    assert_eq!(text[body - 2].trim_end(), " Tool use");
    assert_eq!(
        text[body],
        "   Deepwiki - read_wiki_structure(repoName: \"linuztx/flaredantic\") (MCP)"
    );
    assert_eq!(
        text[body + 1],
        "   Get a list of documentation topics for a GitHub repository."
    );
    assert!(
        !text.iter().any(|r| r.contains('╌') || r.contains('{')),
        "no framed JSON body: {text:?}"
    );
    // The question, and the rule option 2 would remember — named the way the
    // user knows the tool, and scoped to this project.
    assert_eq!(text[body + 3].trim_end(), " Do you want to proceed?");
    assert_eq!(text[body + 4].trim_end(), " ❯ 1. Yes");
    assert_eq!(
        text[body + 5].trim_end(),
        "   2. Yes, and don't ask again for Deepwiki - read_wiki_structure commands"
    );
    assert_eq!(text[body + 6].trim_end(), "      in ~/Codes/tests");
    assert_eq!(text[body + 7].trim_end(), "   3. No");
}

#[test]
fn the_mcp_marker_and_description_are_dim() {
    // `(MCP)` is a badge, not the call: it wears the description's colour so
    // the eye lands on the tool (`docs/mcp.md`).
    let app = pending_mcp();
    let lines = permission_lines(&app, 76, 30);
    let call = lines
        .iter()
        .find(|line| plain(line).contains("read_wiki_structure(repoName"))
        .expect("the call row");
    let last = call.spans.last().expect("the ` (MCP)` marker");
    assert_eq!(last.content.as_ref(), " (MCP)");
    assert_eq!(last.style.fg, Some(PERMISSION_DETAIL_COLOR));
    let detail = lines
        .iter()
        .find(|line| plain(line).contains("Get a list of documentation"))
        .expect("the description row");
    assert_eq!(detail.spans[1].style.fg, Some(PERMISSION_DETAIL_COLOR));
}

#[test]
fn a_parallel_mcp_batch_shows_one_context_cell_above_the_prompt() {
    // The three `⎿ Waiting…` copies of one aggregated line said the same
    // thing three times, in the rows the question needed (`docs/mcp.md`).
    let mut app = pending_mcp();
    app.start_tool_batch(&[
        ToolCallSummary {
            name: "deepwiki - read_wiki_structure (MCP)".to_string(),
            args: "{}".to_string(),
        },
        ToolCallSummary {
            name: "deepwiki - read_wiki_contents (MCP)".to_string(),
            args: "{}".to_string(),
        },
    ]);
    let text = rows(&app, 76, 30);
    assert_eq!(text[0], "● Calling Deepwiki 2 times… (ctrl+o to expand)");
    assert!(
        !text.iter().any(|r| r.contains("Waiting…")),
        "one line for the batch, not one per call: {text:?}"
    );
}

// --- the view flow (docs/view-flow.md): the body shows whole and the page
// flows into scrollback instead of capping with a `… +N lines` tail ---

#[test]
fn a_body_too_tall_for_the_terminal_flows_instead_of_capping() {
    // The point of the prompt is that you read what you are approving — ALL
    // of it. A body taller than the terminal no longer caps: the page builds
    // whole (the tail block at its end, where the bottom anchor keeps it on
    // screen) and the top flows into real scrollback like every framed view.
    let body: String = (1..=200)
        .map(|n| format!("{n:>3} line {n}\n"))
        .collect::<String>();
    let app = app_with(request(PermissionKind::Write, "big.py", body.trim_end()));
    let lines = rows(&app, 70, 24);
    assert!(
        lines.len() > 24,
        "the page overflows the terminal instead of capping: {}",
        lines.len()
    );
    assert!(lines.iter().any(|l| l.contains("line 1")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("line 200")),
        "the whole body is present: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("… +")),
        "no `… +N lines` tail below the ceiling: {lines:?}"
    );
    // The tail block closes the page, so the anchor keeps it on screen: the
    // last 24 rows must hold the question, the options and the hints.
    let tail: Vec<&String> = lines[lines.len() - 24..].iter().collect();
    assert!(tail.iter().any(|l| l.contains("Do you want to create")));
    assert!(tail.iter().any(|l| l.contains("3. No")));
    assert!(tail.iter().any(|l| l.contains("Esc to cancel")));
    assert!(tail.last().unwrap().starts_with('─'), "the closing rule");
}

#[test]
fn an_overflowing_prompt_keeps_its_static_context_cells() {
    // The reported regression: on a terminal too short for the page, the
    // `● Write(…)` / `⎿ Waiting…` cell vanished entirely — not on screen, not
    // in scrollback — so the prompt read as a box out of nowhere. A queued
    // cell is *static text* while the prompt blocks (nothing has started —
    // the approve seam runs before `ToolStart`), so it flows into scrollback
    // safely like the rest of the page. It rides **whole**, uncollapsed:
    // there is no screenful to compete for once the page flows, so hiding
    // siblings behind a `… +N more waiting` summary would lose them for
    // nothing.
    let body: String = (1..=120).map(|n| format!("{n:>3} line {n}\n")).collect();
    let app = pending_batch_with(15, body.trim_end());
    let lines = rows(&app, 80, 44);
    assert!(lines.len() > 44, "the page flows: {}", lines.len());
    assert_eq!(lines[0], "● Write(nexgrad/viz.py)", "the asked-about call");
    assert_eq!(lines[1], "  ⎿  Waiting…", "…keeps its cell: {lines:?}");
    assert_eq!(
        lines.iter().filter(|l| l.contains("Waiting…")).count(),
        15,
        "every queued sibling rides the flow: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("more waiting")),
        "nothing is collapsed on a flowing page: {lines:?}"
    );
    let shown = lines.iter().filter(|l| l.contains(" line ")).count();
    assert_eq!(shown, 120, "the whole body shows: {shown}");
}

#[test]
fn an_overflowing_prompt_drops_a_ticking_agent_tree() {
    // The one context that may NOT flow: a live agent group. Its bullet
    // breathes at the frame pulse and its `{n} tool uses · {tokens} tokens`
    // counters advance as the agents work — and a flowed row is frozen in
    // scrollback, so it would either go stale there or re-sign the flow into
    // a purge rebuild per tick. When the page cannot fit it, it gives way and
    // only the static frame flows.
    let body: String = (1..=120).map(|n| format!("{n:>3} line {n}\n")).collect();
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "audit the tests", false)]);
    let mut req = request(PermissionKind::Write, "nexgrad/viz.py", body.trim_end());
    req.agent = Some("general-purpose".to_string());
    app.open_permission(req);
    let lines = rows(&app, 80, 44);
    assert!(lines.len() > 44, "the page flows: {}", lines.len());
    assert!(
        !lines.iter().any(|l| l.contains("audit the tests")),
        "the ticking tree gave way: {lines:?}"
    );
    assert!(
        lines[0].starts_with('─'),
        "the page opens at the frame's rule: {:?}",
        lines[0]
    );
    // …and it is kept whole on a terminal that fits the page.
    let tall = rows(&app, 80, 200);
    assert!(
        tall.iter().any(|l| l.contains("audit the tests")),
        "a fitting page keeps the tree: {tall:?}"
    );
}

#[test]
fn a_pathological_body_still_caps_at_the_ceiling() {
    // Unbounded is not a goal: the page is rebuilt every draw tick, so a
    // multi-megabyte write caps at PERMISSION_BODY_MAX_ROWS with the familiar
    // tail — far past anything a human reviews, bounded for the render loop.
    let over = crate::ui::theme::PERMISSION_BODY_MAX_ROWS + 50;
    let body: String = (1..=over).map(|n| format!("line {n}\n")).collect();
    let app = app_with(request(PermissionKind::Write, "big.py", body.trim_end()));
    let lines = rows(&app, 70, 24);
    assert!(
        lines.iter().any(|l| l.contains("… +50 lines")),
        "the ceiling's tail counts the excess: {lines:?}"
    );
}

// --- the context follows the conversation on screen (`docs/permissions.md`) ---

/// A subagent `a1` mid-round with a parallel batch of `calls` announced, its
/// session view open, and a prompt from that agent open for the front call.
fn agent_view_pending(calls: &[(&str, &str)], request: PermissionRequest) -> App {
    // What `tui::agent::Session::on_agent_event` stamps on every request that
    // arrives over an agent's channel: **which** run asked. The type on
    // `agent` names the asker in the title; only the id identifies it.
    let mut request = request;
    request.agent_id = Some("a1".to_string());
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Run ls -la via subagent", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolBatch(
            calls
                .iter()
                .map(|(name, args)| ToolCallSummary {
                    name: (*name).to_string(),
                    args: (*args).to_string(),
                })
                .collect(),
        ),
    );
    app.open_agent_view("a1");
    app.open_permission(request);
    app
}

#[test]
fn an_agent_views_prompt_shows_the_agents_own_call_not_the_lead_cell() {
    // The reported bug (manual mode, one *foreground* subagent): standing
    // inside the subagent's session view, its `bash` request opened over the
    // lead's `● Agent(…)` / `⎿ Working…` cell — the main strip's lone-agent
    // tree, which is not on this screen at all — instead of the agent's own
    // `● Bash(ls -la)` / `⎿ Waiting…`. The prompt's context is the *viewed*
    // conversation's cells, exactly like the strip it replaces.
    let mut req = request(PermissionKind::Bash, "ls -la", "");
    req.agent = Some("general-purpose".to_string());
    let app = agent_view_pending(&[("Bash", "ls -la")], req);
    let lines = rows(&app, 72, 40);
    assert_eq!(
        lines[0], "● Bash(ls -la)",
        "the agent's own cell: {lines:?}"
    );
    assert_eq!(lines[1], "  ⎿  Waiting…", "…over its waiting row");
    assert_eq!(lines[2], "", "…then a blank before the frame");
    assert_eq!(lines[3], "─".repeat(72), "…then the prompt's top rule");
    assert!(
        !lines.iter().any(|l| l.contains("Run ls -la via subagent")),
        "the lead's agent cell is not on this screen: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("Working…")),
        "…nor its activity row: {lines:?}"
    );
}

#[test]
fn an_agent_views_prompt_shows_every_waiting_sibling_of_its_batch() {
    // A subagent's parallel batch reads like the main session's: the call
    // being asked about over each not-yet-run sibling's `⎿ Waiting…`.
    let app = agent_view_pending(
        &[("Bash", "ls -la"), ("Read", "main.rs"), ("Bash", "pwd")],
        request(PermissionKind::Bash, "ls -la", ""),
    );
    let lines = rows(&app, 72, 40);
    let rule = lines
        .iter()
        .position(|l| l.starts_with('─'))
        .expect("the prompt's top rule");
    let context = &lines[..rule];
    assert_eq!(context[0], "● Bash(ls -la)");
    assert_eq!(context[1], "  ⎿  Waiting…");
    assert_eq!(context[3], "● Read(main.rs)");
    assert_eq!(context[4], "  ⎿  Waiting…");
    assert_eq!(context[6], "● Bash(pwd)");
    assert_eq!(context[7], "  ⎿  Waiting…");
    assert_eq!(
        context.iter().filter(|l| l.contains("Waiting…")).count(),
        3,
        "every sibling waits: {context:?}"
    );
}

#[test]
fn the_main_view_still_shows_the_lead_agent_tree_above_a_subagents_prompt() {
    // The other half of the same rule: from the *main* conversation the
    // subagent's request keeps the lead's live agent cell, which is what is
    // on screen there (`docs/permissions.md`) — and never the agent's own
    // queue, which that screen does not show.
    let mut req = request(PermissionKind::Bash, "ls -la", "");
    req.agent = Some("general-purpose".to_string());
    let mut app = agent_view_pending(&[("Bash", "ls -la")], req);
    app.close_agent_view();
    let lines = rows(&app, 72, 40);
    assert_eq!(lines[0], "● Agent(Run ls -la via subagent)", "{lines:?}");
    assert!(
        !lines[..3].iter().any(|l| l.contains("Bash(ls -la)")),
        "the agent's own queue is not on the main screen: {lines:?}"
    );
}

#[test]
fn an_agent_views_overflowing_prompt_keeps_its_static_cells() {
    // The flow's stability test must read the same cells the context does. A
    // foreground subagent always leaves a live agent group on the *main*
    // session, so judging by that alone dropped the agent view's own static
    // `⎿ Waiting…` cells from every page too tall to fit — the prompt read as
    // a box out of nowhere again, one level down (`docs/view-flow.md`).
    let body: String = (1..=120).map(|n| format!("{n:>3} line {n}\n")).collect();
    let app = agent_view_pending(
        &[("Write", "viz.py"), ("Write", "plot.py")],
        request(PermissionKind::Write, "viz.py", body.trim_end()),
    );
    let lines = rows(&app, 80, 44);
    assert!(lines.len() > 44, "the page flows: {}", lines.len());
    assert_eq!(
        lines[0], "● Write(viz.py)",
        "the asked-about call: {lines:?}"
    );
    assert_eq!(lines[1], "  ⎿  Waiting…");
    assert_eq!(
        lines.iter().filter(|l| l.contains("Waiting…")).count(),
        2,
        "both queued cells ride the flow: {lines:?}"
    );
}

#[test]
fn an_agent_views_overflowing_prompt_drops_a_running_cell() {
    // …and gives way to the one thing that ticks: the agent's own running
    // call, whose streamed peek grows while the page sits frozen in
    // scrollback.
    let body: String = (1..=120).map(|n| format!("{n:>3} line {n}\n")).collect();
    let mut app = agent_view_pending(
        &[("Bash", "make test"), ("Write", "viz.py")],
        request(PermissionKind::Write, "viz.py", body.trim_end()),
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "make test".into(),
            detail: None,
            arguments: None,
        },
    );
    let lines = rows(&app, 80, 44);
    assert!(lines.len() > 44, "the page flows: {}", lines.len());
    assert!(
        !lines.iter().any(|l| l.contains("make test")),
        "the ticking cell gave way: {lines:?}"
    );
    assert!(lines[0].starts_with('─'), "{:?}", lines[0]);
    // …and a terminal that fits the page keeps it whole.
    let tall = rows(&app, 80, 220);
    assert!(
        tall.iter().any(|l| l.contains("make test")),
        "a fitting page keeps the running cell: {tall:?}"
    );
}

#[test]
fn an_agent_views_prompt_keeps_no_context_for_another_conversations_request() {
    // The main turn — or a *sibling* agent — can raise a prompt while the
    // user is inside agent `a1`'s view. The cells that raised it are on a
    // screen the user is not looking at, and a context cell out of nowhere is
    // exactly what the context exists not to be, so the prompt opens with its
    // own rule (the idle shape) rather than borrowing `a1`'s waiting batch.
    // Agent *type* cannot answer this — two agents share `general-purpose` —
    // which is why the request carries an id.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Run ls -la via subagent", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolBatch(vec![ToolCallSummary {
            name: "Bash".to_string(),
            args: "ls -la".to_string(),
        }]),
    );
    app.open_agent_view("a1");
    // The main turn's own call: no agent stamp at all.
    app.open_permission(request(PermissionKind::Bash, "rm -rf build", ""));
    let lines = rows(&app, 70, 40);
    assert_eq!(lines[0], "─".repeat(70), "no context rows: {lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("ls -la")),
        "the viewed agent's own batch is not the answer to this: {lines:?}"
    );
}

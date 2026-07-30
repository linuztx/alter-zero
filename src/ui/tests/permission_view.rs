//! The inline tool-permission prompt's rendering (`docs/permissions.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::theme::{
    PERMISSION_AGENT_COLOR, PERMISSION_SELECTED_COLOR, PERMISSION_TITLE_COLOR,
};
use super::*;
use crate::permission::{PermissionKind, PermissionRequest};

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

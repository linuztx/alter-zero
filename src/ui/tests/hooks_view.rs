//! The read-only `/hooks` menu's rendering (`docs/hooks-menu.md`).

use super::*;
use crate::hooks::{HooksFile, HooksOverview};
use crate::ui::theme::{
    AI_COLOR, HOOKS_DETAIL_HINT, HOOKS_DISABLED_NOTE, HOOKS_EMPTY, HOOKS_HINT, HOOKS_MARKER,
    HOOKS_MENU_MAX_ROWS, MODEL_META_COLOR, MODEL_SELECTED_COLOR,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// PreToolUse with a two-hook `bash` row (one wearing a status message) and a
/// one-hook match-everything row, plus one Stop hook — 4 configured in all.
const SAMPLE: &str = r#"{
  "hooks": {
    "PreToolUse": [
      { "matcher": "bash",
        "hooks": [
          { "type": "command", "command": "./guard.sh",
            "statusMessage": "checking…" },
          { "type": "command", "command": "./lint.sh" }
        ] },
      { "hooks": [ { "type": "command", "command": "./always.sh" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "./done.sh" } ] }
    ]
  }
}"#;

fn overview() -> HooksOverview {
    HooksOverview::from_file(&HooksFile::parse(SAMPLE).expect("fixture parses"))
}

fn hooks_app() -> App {
    let mut app = App::new();
    app.open_hooks_menu(overview(), Some("~/.alter-zero/hooks.json".into()), true);
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// The built body as trimmed plain rows.
fn texts(app: &App, width: u16) -> Vec<String> {
    hooks_view_lines(app, width)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn find(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}: {texts:#?}"))
}

/// Whether a built line is a numbered list row (`{marker}{n}. …`) — the
/// number-dot shape, so prose periods and the count line never match.
fn is_list_row(line: &str) -> bool {
    let text = line.trim_start().trim_start_matches(['❯', '↑', '↓', ' ']);
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && text[digits..].starts_with('.')
}

#[test]
fn the_events_level_is_framed_with_title_count_info_and_hint() {
    let app = hooks_app();
    let texts = texts(&app, 78);
    assert!(texts.first().expect("frame").starts_with('─'), "top rule");
    assert!(texts.last().expect("frame").starts_with('─'), "bottom rule");
    assert_eq!(texts[2], "  Hooks");
    assert_eq!(texts[3], "  4 hooks configured");
    assert!(
        texts[5].starts_with("  ℹ This menu is read-only."),
        "the read-only banner: {:?}",
        texts[5]
    );
    let hint = find(&texts, HOOKS_HINT);
    assert_eq!(texts[hint], format!("  {HOOKS_HINT}"));
    assert!(texts[hint - 1].is_empty() && texts[hint + 1].is_empty());
}

#[test]
fn the_events_rows_are_numbered_marked_and_counted() {
    let app = hooks_app();
    let texts = texts(&app, 78);
    let first = find(&texts, "PreToolUse (3)");
    assert!(
        texts[first].starts_with(&format!("  {HOOKS_MARKER}1.")),
        "the selected first row wears the marker: {:?}",
        texts[first]
    );
    assert!(
        texts[first].contains("Before tool execution"),
        "the summary rides the description column: {:?}",
        texts[first]
    );
    let second = find(&texts, "PostToolUse");
    assert!(
        texts[second].starts_with("    2."),
        "unselected rows get marker-width spaces: {:?}",
        texts[second]
    );
    assert!(
        !texts[second].contains('('),
        "a zero-count event drops the count suffix: {:?}",
        texts[second]
    );
}

#[test]
fn the_events_list_windows_five_rows_with_overflow_markers() {
    let mut app = hooks_app();
    let first = texts(&app, 78);
    let rows: Vec<&String> = first.iter().filter(|l| is_list_row(l)).collect();
    assert_eq!(rows.len(), HOOKS_MENU_MAX_ROWS, "five visible rows");
    assert!(
        rows[HOOKS_MENU_MAX_ROWS - 1].starts_with("  ↓ 5."),
        "more rows below — ↓ dresses the window's last row: {:?}",
        rows[HOOKS_MENU_MAX_ROWS - 1]
    );
    assert!(
        !first.iter().any(|l| l.contains('↑')),
        "nothing above the first window"
    );
    // Selection on the 4th row centers the window: rows 2–6, both markers —
    // the reference mock's second screen.
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    let scrolled = texts(&app, 78);
    assert!(
        scrolled.iter().any(|l| l.starts_with("  ↑ 2.")),
        "↑ on the window's first row: {scrolled:#?}"
    );
    assert!(
        scrolled
            .iter()
            .any(|l| l.starts_with(&format!("  {HOOKS_MARKER}4."))),
        "the selection keeps its absolute number: {scrolled:#?}"
    );
    assert!(
        scrolled.iter().any(|l| l.starts_with("  ↓ 6.")),
        "↓ on the window's last row: {scrolled:#?}"
    );
    assert!(!scrolled.iter().any(|l| l.contains("1.  PreToolUse")));
}

#[test]
fn the_description_column_aligns_across_the_visible_rows() {
    let app = hooks_app();
    let texts = texts(&app, 100);
    // Display columns, not byte offsets — the selected row's `❯` is three
    // bytes wide but one column.
    let column = |line: &str, needle: &str| {
        crate::ui::wrap::cols(&line[..line.find(needle).expect("the summary is on the row")])
    };
    let a = &texts[find(&texts, "Before tool execution")];
    let b = &texts[find(&texts, "After tool execution")];
    assert_eq!(
        column(a, "Before tool execution"),
        column(b, "After tool execution"),
        "summaries line up in a column:\n{a}\n{b}"
    );
}

#[test]
fn the_matchers_level_titles_the_event_and_lists_sourced_rows() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Enter);
    let texts = texts(&app, 90);
    assert_eq!(texts[2], "  PreToolUse - Matchers");
    assert!(
        texts[3].starts_with("  Input to command is the tool call"),
        "the event description sits under the title: {:?}",
        texts[3]
    );
    assert!(
        texts.iter().any(|l| l.contains("Exit code 2")),
        "the exit-code semantics are spelled out"
    );
    let bash = find(&texts, "[User] bash");
    assert!(
        texts[bash].starts_with(&format!("  {HOOKS_MARKER}1. [User] bash")),
        "{:?}",
        texts[bash]
    );
    assert!(texts[bash].contains("2 hooks"), "{:?}", texts[bash]);
    let all = find(&texts, "[User] (all)");
    assert!(texts[all].starts_with("    2. [User] (all)"));
    assert!(texts[all].contains("1 hook"), "{:?}", texts[all]);
    assert!(texts.iter().any(|l| l.contains(HOOKS_HINT)));
}

#[test]
fn the_hooks_level_lists_handlers_with_the_source_header() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    let texts = texts(&app, 90);
    assert_eq!(texts[2], "  PreToolUse - Matcher: bash");
    let first = find(&texts, "[command] checking…");
    assert!(
        texts[first].starts_with(&format!("  {HOOKS_MARKER}1.")),
        "the status message stands in for the command: {:?}",
        texts[first]
    );
    assert!(texts[first].contains("User Settings"), "{:?}", texts[first]);
    let second = find(&texts, "[command] ./lint.sh");
    assert!(texts[second].starts_with("    2."));
}

#[test]
fn a_matcherless_event_titles_bare_and_descends_straight_to_its_hooks() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Char('7')); // Stop
    let texts = texts(&app, 90);
    assert_eq!(texts[2], "  Stop", "no `- Matcher:` suffix");
    assert!(texts.iter().any(|l| l.contains("[command] ./done.sh")));
}

#[test]
fn an_event_with_nothing_configured_shows_the_empty_state() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Down); // PostToolUse
    press(&mut app, KeyCode::Enter);
    let texts = texts(&app, 90);
    assert_eq!(texts[2], "  PostToolUse - Matchers");
    let empty = find(&texts, HOOKS_EMPTY);
    assert!(
        texts[empty + 2].contains("To add hooks, edit hooks.json"),
        "{:?}",
        texts.get(empty + 2)
    );
    assert!(
        !texts.iter().any(|l| is_list_row(l)),
        "no numbered rows in the empty state: {texts:#?}"
    );
    assert!(
        texts.iter().any(|l| l.contains(HOOKS_DETAIL_HINT))
            && !texts.iter().any(|l| l.contains(HOOKS_HINT)),
        "nothing to confirm in the empty state — Esc is the only key: {texts:#?}"
    );
}

#[test]
fn the_detail_page_shows_fields_the_boxed_command_and_the_note() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Enter); // matchers
    press(&mut app, KeyCode::Enter); // bash hooks
    press(&mut app, KeyCode::Enter); // first hook's details
    let texts = texts(&app, 78);
    assert_eq!(texts[2], "  Hook details");
    assert_eq!(texts[4], "  Event:    PreToolUse");
    assert_eq!(texts[5], "  Matcher:  bash");
    assert_eq!(texts[6], "  Type:     command");
    assert_eq!(
        texts[7],
        "  Source:   User settings (~/.alter-zero/hooks.json)"
    );
    let label = find(&texts, "Command:");
    assert!(texts[label + 1].starts_with("  ╭") && texts[label + 1].ends_with('╮'));
    assert!(
        texts[label + 2].starts_with("  │ ./guard.sh"),
        "the box holds the REAL command, never the status message: {:?}",
        texts[label + 2]
    );
    assert!(texts[label + 3].starts_with("  ╰") && texts[label + 3].ends_with('╯'));
    assert!(
        texts
            .iter()
            .any(|l| l.contains("Status message: checking…")),
        "{texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|l| l.contains("To modify or remove this hook")),
        "{texts:#?}"
    );
    let hint = find(&texts, HOOKS_DETAIL_HINT);
    assert!(
        !texts.iter().any(|l| l.contains(HOOKS_HINT)),
        "the detail page has nothing to confirm"
    );
    assert!(texts[hint + 1].is_empty(), "trailing gap before the rule");
}

#[test]
fn a_matcherless_detail_omits_the_matcher_row() {
    let mut app = hooks_app();
    press(&mut app, KeyCode::Char('7')); // Stop → hooks
    press(&mut app, KeyCode::Enter); // its one hook's details
    let texts = texts(&app, 78);
    assert_eq!(texts[4], "  Event:    Stop");
    assert_eq!(
        texts[5], "  Type:     command",
        "a matcher the engine ignores is not presented as if it filtered"
    );
    assert!(!texts.iter().any(|l| l.contains("Matcher:")));
}

#[test]
fn a_long_command_truncates_in_its_row_but_wraps_in_the_detail_box() {
    let long = "jq -re '.tool_input.command | test(\"rm -rf\") | not' >/dev/null \
                || { echo 'no recursive deletes' >&2; exit 2; }";
    let file = HooksFile::parse(&format!(
        r#"{{"hooks":{{"PreToolUse":[{{"matcher":"bash","hooks":[
             {{"type":"command","command":{cmd}}}]}}]}}}}"#,
        cmd = serde_json::to_string(long).unwrap()
    ))
    .unwrap();
    let mut app = App::new();
    app.open_hooks_menu(HooksOverview::from_file(&file), None, true);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    let rows = texts(&app, 78);
    let row = &rows[find(&rows, "[command] jq -re")];
    assert!(
        row.contains('…') && row.ends_with("User Settings"),
        "the row keeps its source column by eliding the command: {row:?}"
    );
    press(&mut app, KeyCode::Enter);
    let detail = texts(&app, 78);
    let boxed: Vec<&String> = detail.iter().filter(|l| l.starts_with("  │")).collect();
    assert!(
        boxed.len() >= 2,
        "the command wraps across box rows: {detail:#?}"
    );
    let joined = boxed
        .iter()
        .map(|l| {
            l.trim_start_matches("  │ ")
                .trim_end_matches(" │")
                .trim_end()
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("no recursive deletes"),
        "nothing truncated away: {joined:?}"
    );
}

#[test]
fn the_disabled_note_shows_only_when_configured_hooks_are_off() {
    let mut off = App::new();
    off.open_hooks_menu(overview(), None, false);
    let texts_off = texts(&off, 90);
    find(&texts_off, HOOKS_DISABLED_NOTE);

    let on = hooks_app();
    assert!(
        !texts(&on, 90)
            .iter()
            .any(|l| l.contains(HOOKS_DISABLED_NOTE))
    );

    let mut empty_off = App::new();
    empty_off.open_hooks_menu(HooksOverview::from_file(&HooksFile::default()), None, false);
    assert!(
        !texts(&empty_off, 90)
            .iter()
            .any(|l| l.contains(HOOKS_DISABLED_NOTE)),
        "nothing configured — nothing to warn about"
    );
    assert!(
        texts(&empty_off, 90)
            .iter()
            .any(|l| l.contains("0 hooks configured"))
    );
}

#[test]
fn the_selection_and_chrome_wear_the_picker_familys_colours() {
    let app = hooks_app();
    let lines = hooks_view_lines(&app, 78);
    let title = &lines[2];
    assert_eq!(title.spans[1].style.fg, Some(AI_COLOR), "the title");
    let selected = lines
        .iter()
        .find(|l| plain(l).contains("PreToolUse (3)"))
        .expect("the selected row");
    assert!(
        selected
            .spans
            .iter()
            .any(|s| s.style.fg == Some(MODEL_SELECTED_COLOR)),
        "the selected row lights up in the palette accent"
    );
    let count = &lines[3];
    assert_eq!(
        count.spans[1].style.fg,
        Some(MODEL_META_COLOR),
        "the count line is dim"
    );
}

#[test]
fn the_height_is_the_built_lines_and_none_when_closed() {
    let app = hooks_app();
    let lines = hooks_view_lines(&app, 78).len() as u16;
    assert_eq!(hooks_menu_height(&app, 78, 200), Some(lines));
    assert_eq!(
        hooks_menu_height(&app, 78, 10),
        Some(10),
        "clamped to the terminal"
    );
    assert_eq!(hooks_menu_height(&App::new(), 78, 200), None);
}

#[test]
fn render_paints_the_lines_and_the_cursor_hides_seated_on_the_selection() {
    let app = hooks_app();
    let height = hooks_menu_height(&app, 78, 200).unwrap();
    let mut buf = buffer(78, height);
    render_hooks_menu(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 78).starts_with('─'));
    assert!(row(&buf, 2, 78).contains("Hooks"));
    // No text entry anywhere in the menu — the permission prompt's rule: the
    // frame shows no hardware cursor at all (a kitty cursor animation blinks
    // at whatever seat a menu picks), while the *seat* still tracks the
    // highlighted `❯` row so the cursor's return starts somewhere sensible.
    assert!(
        !crate::ui::cursor_visible(&app),
        "a menu has nothing for a cursor to point at"
    );
    let area = Rect::new(0, 0, 78, height);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 78).trim_start().starts_with('❯'))
        .expect("the selected row wears the marker");
    assert_eq!(
        cursor_position(area, &app),
        (2, marker_row),
        "the seat is the highlighted row's marker"
    );
}

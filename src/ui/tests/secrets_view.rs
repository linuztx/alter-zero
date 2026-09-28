//! The `/secrets` page (`docs/secrets.md`): the list and the form, values
//! masked in the builder.

use super::*;
use crate::secrets::SecretMeta;
use crate::ui::secrets_view::{secrets_menu_rows, secrets_view_lines};
use crate::ui::theme::{
    SECRETS_ADD_ROW, SECRETS_EMPTY, SECRETS_FIELD_COL, SECRETS_LIST_MASK, SECRETS_VALUE_KEEP,
    error_color,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const VALUE: &str = "hunter22";

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_in(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn meta(name: &str, context: &str) -> SecretMeta {
    SecretMeta {
        name: name.to_string(),
        context: context.to_string(),
    }
}

/// The page open over `secrets`.
fn open_with(secrets: &[(&str, &str)]) -> App {
    let mut app = with_session();
    app.set_secret_metas(secrets.iter().map(|(n, c)| meta(n, c)).collect());
    app.open_secrets_page();
    app
}

/// A new-secret form with `name` typed and `value` in the value field.
fn form_with(name: &str, value: &str) -> App {
    let mut app = open_with(&[]);
    press(&mut app, KeyCode::Enter);
    type_in(&mut app, name);
    press(&mut app, KeyCode::Tab);
    type_in(&mut app, value);
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    secrets_view_lines(app, width).iter().map(plain).collect()
}

fn is_rule(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().all(|c| c == '─')
}

fn row_with<'a>(texts: &'a [String], needle: &str) -> &'a str {
    texts
        .iter()
        .find(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is on the page: {texts:?}"))
}

#[test]
fn a_closed_page_builds_nothing() {
    assert!(secrets_view_lines(&with_session(), 80).is_empty());
    assert_eq!(secrets_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_list_shows_each_placeholder_a_fixed_mask_and_its_context() {
    let app = open_with(&[
        ("ROOT_PASSWORD", "Root password for staging"),
        ("TOKEN", ""),
    ]);
    let texts = texts(&app, 80);
    assert!(is_rule(texts.first().unwrap()), "top rule: {texts:?}");
    assert!(is_rule(texts.last().unwrap()), "bottom rule: {texts:?}");
    let first = row_with(&texts, "<secret:ROOT_PASSWORD>");
    assert!(first.contains('❯'), "the first row is highlighted: {first}");
    assert!(first.contains(SECRETS_LIST_MASK), "{first}");
    assert!(first.contains("Root password for staging"), "{first}");
    let second = row_with(&texts, "<secret:TOKEN>");
    assert!(!second.contains('❯'), "{second}");
    // The mask is fixed: it says a value is there, never how long it is.
    for row in [first, second] {
        assert_eq!(
            row.matches('•').count(),
            SECRETS_LIST_MASK.chars().count(),
            "{row}"
        );
    }
    // The masks line up in one column, whatever the placeholders' lengths.
    let column = |row: &str| crate::ui::wrap::cols(&row[..row.find(SECRETS_LIST_MASK).unwrap()]);
    assert_eq!(column(first), column(second), "{texts:?}");
    row_with(&texts, SECRETS_ADD_ROW);
}

#[test]
fn an_empty_store_says_how_to_start_on_the_add_row() {
    let texts = texts(&open_with(&[]), 80);
    let joined = texts.join("\n");
    assert!(
        joined.contains(SECRETS_EMPTY.split_whitespace().next().unwrap()),
        "{texts:?}"
    );
    assert!(row_with(&texts, SECRETS_ADD_ROW).contains('❯'), "{texts:?}");
}

#[test]
fn the_delete_question_replaces_the_hint_in_red() {
    let mut app = open_with(&[("TOKEN", "")]);
    press(&mut app, KeyCode::Char('d'));
    let lines = secrets_view_lines(&app, 80);
    let question = lines
        .iter()
        .find(|line| plain(line).contains("Delete <secret:TOKEN>?"))
        .expect("the question is asked");
    assert!(
        question
            .spans
            .iter()
            .any(|span| span.style.fg == Some(error_color())),
        "in red: {question:?}"
    );
}

#[test]
fn the_form_masks_the_value_one_dot_per_character() {
    let app = form_with("TOKEN", VALUE);
    let texts = texts(&app, 80);
    let value_row = row_with(&texts, "Value");
    assert_eq!(value_row.matches('•').count(), VALUE.len(), "{value_row}");
    assert!(
        texts.iter().all(|t| !t.contains(VALUE)),
        "the value never reaches a line: {texts:?}"
    );
}

#[test]
fn a_long_value_never_widens_the_page() {
    let long = "x".repeat(300);
    for width in [30_u16, 50, 80] {
        let app = form_with("TOKEN", &long);
        for line in secrets_view_lines(&app, width) {
            let text = plain(&line);
            assert!(
                crate::ui::wrap::cols(&text) <= usize::from(width),
                "{width}: {text:?}"
            );
            assert!(!text.contains("xxxx"), "{text}");
        }
    }
}

#[test]
fn the_name_previews_its_placeholder() {
    let mut app = open_with(&[]);
    press(&mut app, KeyCode::Enter);
    type_in(&mut app, "root password");
    let texts = texts(&app, 80);
    row_with(&texts, "ROOT_PASSWORD");
    row_with(&texts, "Use it as <secret:ROOT_PASSWORD>");
}

#[test]
fn an_edit_names_its_secret_and_invites_keeping_the_value() {
    let mut app = open_with(&[("TOKEN", "The staging key")]);
    press(&mut app, KeyCode::Enter);
    let texts = texts(&app, 80);
    row_with(&texts, "Edit <secret:TOKEN>");
    row_with(&texts, SECRETS_VALUE_KEEP);
    row_with(&texts, "The staging key");
}

#[test]
fn a_refusal_shows_in_red_under_the_fields() {
    let mut app = form_with("TOKEN", "abc");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Enter);
    let lines = secrets_view_lines(&app, 80);
    let error = lines
        .iter()
        .find(|line| plain(line).contains("Values need at least 4 characters."))
        .expect("the reason is shown");
    assert!(
        error
            .spans
            .iter()
            .any(|span| span.style.fg == Some(error_color())),
        "{error:?}"
    );
}

#[test]
fn the_hint_says_what_enter_does_on_each_field() {
    let mut app = open_with(&[]);
    press(&mut app, KeyCode::Enter);
    assert!(texts(&app, 80).iter().any(|t| t.contains("enter next")));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert!(texts(&app, 80).iter().any(|t| t.contains("enter save")));
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_with(&[("TOKEN", "")]);
    let lines = secrets_view_lines(&app, 80).len() as u16;
    assert_eq!(secrets_menu_rows(&app, 80), lines);
    assert_eq!(secrets_picker_height(&app, 80, 200), Some(lines));
    assert_eq!(secrets_picker_height(&app, 80, 6), Some(6));
}

#[test]
fn a_running_turn_keeps_its_strip_above_the_page() {
    let mut app = open_with(&[("TOKEN", "")]);
    app.begin_stream();
    app.push_chunk("streaming…");
    let lines = secrets_view_lines(&app, 80).len() as u16;
    assert!(
        secrets_picker_height(&app, 80, 200).unwrap() > lines,
        "the strip's rows ride above the page"
    );
}

#[test]
fn the_page_flows_like_its_family() {
    let app = open_with(&[("A", ""), ("B", ""), ("C", "")]);
    let page = secrets_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 6, 1000).expect("flows on a 6-row terminal");
    assert_eq!(flow.lines.len(), page - 6, "the skipped top flows");
}

#[test]
fn the_list_hides_the_cursor_on_its_marker() {
    let app = open_with(&[("A", ""), ("B", "")]);
    let height = secrets_picker_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_secrets_picker(buf.area, &mut buf, &app);
    assert!(!cursor_visible(&app), "a list has nothing to type into");
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the highlighted row wears the marker");
    assert_eq!(
        cursor_position(Rect::new(0, 0, 80, height), &app),
        (2, marker_row)
    );
}

#[test]
fn the_form_shows_the_cursor_at_the_end_of_the_focused_field() {
    let mut app = open_with(&[]);
    press(&mut app, KeyCode::Enter);
    type_in(&mut app, "AB");
    assert!(cursor_visible(&app), "the form is typed into");
    let height = secrets_picker_height(&app, 80, 200).unwrap();
    let area = Rect::new(0, 0, 80, height);
    let mut buf = buffer(80, height);
    render_secrets_picker(buf.area, &mut buf, &app);
    let name_row = (0..height)
        .find(|&y| row(&buf, y, 80).contains("Name"))
        .unwrap();
    assert_eq!(
        cursor_position(area, &app),
        (SECRETS_FIELD_COL + 2, name_row)
    );
    press(&mut app, KeyCode::Tab);
    type_in(&mut app, "abc");
    let mut buf = buffer(80, height);
    render_secrets_picker(buf.area, &mut buf, &app);
    let value_row = (0..height)
        .find(|&y| row(&buf, y, 80).contains("Value"))
        .unwrap();
    assert_eq!(
        cursor_position(area, &app),
        (SECRETS_FIELD_COL + 3, value_row),
        "one mask glyph per character typed"
    );
}

#[test]
fn no_row_is_wider_than_the_terminal() {
    let context = "a context line that goes on and on well past any narrow terminal width";
    let mut app = open_with(&[("A_FAIRLY_LONG_SECRET_NAME_FOR_THE_TEST", context)]);
    for width in [16_u16, 24, 40, 80] {
        for text in texts(&app, width) {
            assert!(
                crate::ui::wrap::cols(&text) <= usize::from(width),
                "{width}: {text:?}"
            );
        }
    }
    press(&mut app, KeyCode::Enter);
    for width in [16_u16, 24, 40, 80] {
        for text in texts(&app, width) {
            assert!(
                crate::ui::wrap::cols(&text) <= usize::from(width),
                "{width}: {text:?}"
            );
        }
    }
}

#[test]
fn a_long_context_wraps_inside_the_field() {
    let context = "one two three four five six seven eight nine ten eleven twelve";
    let mut app = open_with(&[("TOKEN", context)]);
    press(&mut app, KeyCode::Enter);
    let texts = texts(&app, 40);
    let words: String = texts
        .iter()
        .filter(|t| !is_rule(t))
        .flat_map(|t| t.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        words.contains(context),
        "the whole context is shown: {texts:?}"
    );
}

#[test]
fn the_value_never_reaches_a_painted_cell() {
    let app = form_with("TOKEN", VALUE);
    let height = secrets_picker_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_secrets_picker(buf.area, &mut buf, &app);
    for y in 0..height {
        assert!(!row(&buf, y, 80).contains(VALUE));
    }
}

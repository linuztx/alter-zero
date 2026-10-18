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

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

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
    secrets_view_lines(app, width, TALL)
        .iter()
        .map(plain)
        .collect()
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
fn every_page_hugs_its_frame() {
    // The title straight under the opening rule, the hint — or the delete
    // question standing in for it — straight over the closing one, on the
    // list and on the form alike.
    let mut confirming = open_with(&[("TOKEN", "")]);
    press(&mut confirming, KeyCode::Char('d'));
    let mut editing = open_with(&[("TOKEN", "The staging key")]);
    press(&mut editing, KeyCode::Enter);
    let mut refused = form_with("TOKEN", "abc");
    press(&mut refused, KeyCode::Tab);
    press(&mut refused, KeyCode::Enter);
    for (what, app) in [
        ("an empty list", open_with(&[])),
        ("a list", open_with(&[("TOKEN", "The staging key")])),
        ("the delete question", confirming),
        ("a new secret's form", form_with("TOKEN", VALUE)),
        ("an edit's form", editing),
        ("a refused form", refused),
    ] {
        assert_frame_hugs_content(what, &secrets_view_lines(&app, 80, TALL));
    }
}

#[test]
fn a_closed_page_builds_nothing() {
    assert!(secrets_view_lines(&with_session(), 80, TALL).is_empty());
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
    let lines = secrets_view_lines(&app, 80, TALL);
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
        for line in secrets_view_lines(&app, width, TALL) {
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
    let lines = secrets_view_lines(&app, 80, TALL);
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
    let lines = secrets_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(secrets_menu_rows(&app, 80, TALL), lines);
    assert_eq!(secrets_picker_height(&app, 80, 200), Some(lines));
    assert_eq!(secrets_picker_height(&app, 80, 6), Some(6));
}

#[test]
fn a_running_turn_keeps_its_strip_above_the_page() {
    let mut app = open_with(&[("TOKEN", "")]);
    app.begin_stream();
    app.push_chunk("streaming…");
    let lines = secrets_view_lines(&app, 80, TALL).len() as u16;
    assert!(
        secrets_picker_height(&app, 80, 200).unwrap() > lines,
        "the strip's rows ride above the page"
    );
}

#[test]
fn the_page_flows_like_its_family() {
    let app = open_with(&[("A", ""), ("B", ""), ("C", "")]);
    // Even the smallest page is taller than a 6-row terminal.
    let page = secrets_view_lines(&app, 80, 6).len();
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

#[test]
fn both_titles_wear_the_title_dress() {
    // The list's `Secrets` and the form's `New secret` — the dress every
    // page title shares (`docs/theme.md`).
    let mut app = open_with(&[]);
    let title = secrets_view_lines(&app, 80, TALL)
        .into_iter()
        .find(|line| plain(line).trim() == "Secrets")
        .expect("the list title");
    assert!(wears_the_title_dress(&title), "{title:?}");
    press(&mut app, KeyCode::Enter);
    let title = secrets_view_lines(&app, 80, TALL)
        .into_iter()
        .find(|line| plain(line).trim() == "New secret")
        .expect("the form title");
    assert!(wears_the_title_dress(&title), "{title:?}");
}

// --- a short terminal keeps the highlighted secret on screen ---

/// The list open over `n` secrets, `SECRET_0`, `SECRET_1`, ….
fn open_many(n: usize) -> App {
    let names: Vec<String> = (0..n).map(|i| format!("SECRET_{i}")).collect();
    let secrets: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "a context")).collect();
    open_with(&secrets)
}

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    secrets_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// The height one row short of `texts`.
fn short_of(texts: &[String]) -> u16 {
    u16::try_from(texts.len()).expect("rows") - 1
}

#[test]
fn every_short_secrets_page_hugs_its_frame() {
    // Whichever page a height builds — the whole list, the headed window,
    // the window alone — no blank row meets either rule, the `↑` slot
    // included: without the header it would sit straight under the top
    // rule, so that page has no slots (`docs/view-flow.md` *The frame hugs
    // its content*).
    for width in [40_u16, 90] {
        for n in [0_usize, 1, 2, 5, 12] {
            let mut app = open_many(n);
            for selected in 0..=n {
                for height in 1..=30_u16 {
                    let lines: Vec<Line<'static>> = page_at(&app, width, height)
                        .into_iter()
                        .map(Line::from)
                        .collect();
                    assert_frame_hugs_content(
                        &format!("width {width} n {n} at {selected} on {height} rows"),
                        &lines,
                    );
                }
                press(&mut app, KeyCode::Down);
            }
        }
    }
}

fn has_title(texts: &[String]) -> bool {
    texts.iter().any(|t| t.trim() == "Secrets")
}

/// The row wearing the `❯`.
fn highlighted(texts: &[String]) -> &str {
    texts
        .iter()
        .find(|t| t.trim_start().starts_with('❯'))
        .unwrap_or_else(|| panic!("a row is highlighted: {texts:#?}"))
}

/// How many secrets' rows the page lists.
fn listed(texts: &[String]) -> usize {
    texts.iter().filter(|t| t.contains("<secret:")).count()
}

#[test]
fn many_secrets_on_a_short_terminal_keep_the_highlighted_one_on_screen() {
    // The whole page lists every secret. Anchored on a terminal shorter
    // than that, it kept the last rows and the hint and flowed the title and
    // the first secrets — the highlighted one among them — off the top.
    // Past what fits, the list windows three rows around the selection
    // between the ↑/↓ overflow slots.
    let app = open_many(8);
    let whole = page_at(&app, 90, TALL);
    assert_eq!(listed(&whole), 8, "{whole:#?}");
    assert!(whole.len() > 12, "{whole:#?}");
    let texts = page_at(&app, 90, 12);
    assert!(texts.len() <= 12, "fits: {texts:#?}");
    assert!(
        highlighted(&texts).contains("<secret:SECRET_0>"),
        "{texts:#?}"
    );
    assert_eq!(listed(&texts), 3, "three rows of the list: {texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↓ 6 more below"), "{texts:#?}");
    assert!(
        texts.iter().any(|t| t.contains("esc close")),
        "the hint stays: {texts:#?}"
    );
    assert!(
        is_rule(&texts[0]) && is_rule(texts.last().expect("rows")),
        "{texts:#?}"
    );
}

#[test]
fn the_window_follows_the_selection_and_names_what_it_hides() {
    let mut app = open_many(8);
    for _ in 0..5 {
        press(&mut app, KeyCode::Down);
    }
    let texts = page_at(&app, 90, 12);
    assert!(
        highlighted(&texts).contains("<secret:SECRET_5>"),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t == "  ↑ 4 more above"), "{texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↓ 2 more below"), "{texts:#?}");
    // The add row is the list's last row, and the window reaches it.
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    let texts = page_at(&app, 90, 12);
    assert!(highlighted(&texts).contains(SECRETS_ADD_ROW), "{texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↑ 6 more above"), "{texts:#?}");
}

#[test]
fn the_header_stays_while_it_fits_and_yields_after_the_list_shrinks() {
    // codex's order: the list shrinks to its window first, then the header
    // (the title and the blurb) goes.
    let app = open_many(8);
    let whole = page_at(&app, 90, TALL);
    let headed = page_at(&app, 90, short_of(&whole));
    assert!(headed.len() < whole.len(), "{headed:#?}");
    assert!(has_title(&headed), "the header stays: {headed:#?}");
    assert_eq!(listed(&headed), 3, "{headed:#?}");
    let headless = page_at(&app, 90, short_of(&headed));
    assert!(headless.len() < headed.len(), "{headless:#?}");
    assert!(!has_title(&headless), "the header goes: {headless:#?}");
    assert_eq!(listed(&headless), 3, "{headless:#?}");
    assert!(
        highlighted(&headless).contains("<secret:SECRET_0>"),
        "{headless:#?}"
    );
}

#[test]
fn a_handful_of_secrets_keeps_every_row_when_only_the_header_must_go() {
    // Two secrets and the add row fit the window whole: the slots stay
    // blank and nothing is hidden.
    let app = open_many(2);
    let whole = page_at(&app, 90, TALL);
    let texts = page_at(&app, 90, short_of(&whole));
    assert!(texts.len() < whole.len(), "{texts:#?}");
    assert!(!has_title(&texts), "{texts:#?}");
    assert_eq!(listed(&texts), 2, "{texts:#?}");
    assert!(
        texts.iter().any(|t| t.contains(SECRETS_ADD_ROW)),
        "{texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains(" more ")),
        "nothing hidden: {texts:#?}"
    );
}

#[test]
fn an_empty_store_drops_its_header_on_a_short_terminal() {
    // The empty store's note says what a secret is for well enough on its
    // own; the title and the blurb go before it does.
    let app = open_with(&[]);
    let whole = page_at(&app, 80, TALL);
    let texts = page_at(&app, 80, short_of(&whole));
    assert!(texts.len() < whole.len(), "{texts:#?}");
    assert!(!has_title(&texts), "{texts:#?}");
    let note = SECRETS_EMPTY.split_whitespace().next().unwrap();
    assert!(texts.iter().any(|t| t.contains(note)), "{texts:#?}");
    assert!(highlighted(&texts).contains(SECRETS_ADD_ROW), "{texts:#?}");
}

#[test]
fn every_short_secrets_page_fits_holds_its_height_and_never_stacks_blanks() {
    for width in [40_u16, 60, 90] {
        for n in [0_usize, 1, 2, 3, 5, 12] {
            let mut app = open_many(n);
            let smallest = page_at(&app, width, 1).len();
            for height in [8_u16, 10, 12, 14, 18, 24] {
                let mut heights = Vec::new();
                for selected in 0..=n {
                    let texts = page_at(&app, width, height);
                    let context = format!("width {width} n {n} height {height} at {selected}");
                    if usize::from(height) >= smallest {
                        assert!(texts.len() <= usize::from(height), "{context}: {texts:#?}");
                    }
                    assert!(
                        texts
                            .windows(2)
                            .all(|pair| !(pair[0].is_empty() && pair[1].is_empty())),
                        "{context}: two blank rows stack: {texts:#?}"
                    );
                    highlighted(&texts);
                    heights.push(texts.len());
                    press(&mut app, KeyCode::Down);
                }
                assert!(
                    heights.windows(2).all(|pair| pair[0] == pair[1]),
                    "width {width} n {n} height {height}: one height at every selection: {heights:?}"
                );
                for _ in 0..=n {
                    press(&mut app, KeyCode::Up);
                }
            }
        }
    }
}

#[test]
fn a_short_list_seats_the_hidden_cursor_on_its_marker() {
    let mut app = open_many(8);
    for _ in 0..4 {
        press(&mut app, KeyCode::Down);
    }
    // The region a 12-row terminal reserves: the compact page, whole.
    let height = secrets_picker_height(&app, 90, 12).unwrap();
    assert!(height <= 12, "{height}");
    let area = Rect::new(0, 0, 90, height);
    let mut buf = buffer(90, height);
    render_secrets_picker(area, &mut buf, &app);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 90).trim_start().starts_with('❯'))
        .expect("the highlighted row is painted");
    assert!(row(&buf, marker_row, 90).contains("<secret:SECRET_4>"));
    assert_eq!(cursor_position(area, &app), (2, marker_row));
}

//! The inline `/settings` menu's rendering (`docs/settings.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::permission::PermissionMode;
use crate::settings::{SettingAvailability, SettingKey};
use crate::ui::theme::{
    GAP_ROWS, MODEL_SELECTED_COLOR, SETTINGS_CHROME_ROWS, SETTINGS_HINT, SETTINGS_MENU_MAX_ROWS,
    SETTINGS_SEARCH_ROW, SETTINGS_VALUE_COLOR, SETTINGS_VALUE_OFF_COLOR, STATUS_GAP_ROWS,
    STATUS_ROWS,
};

/// How many setting rows the menu actually shows: every one, until there are
/// more than the window holds — past [`SETTINGS_MENU_MAX_ROWS`] the list
/// scrolls with the selection centered rather than growing without bound.
fn visible_rows() -> u16 {
    u16::try_from(SettingKey::ALL.len())
        .unwrap()
        .min(SETTINGS_MENU_MAX_ROWS)
}

/// An app with the menu open and a gate present (the ordinary session shape).
fn settings_app() -> App {
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.open_settings();
    app
}

/// Render the menu at its natural height, like the boundary does — otherwise
/// the list's `Min(0)` would expand and push the rows below it down.
fn render(app: &App, width: u16) -> Buffer {
    let height = settings_height(app, 78, 200).expect("the menu is open");
    let mut buf = buffer(width, height);
    render_settings(buf.area, &mut buf, app);
    buf
}

#[test]
fn the_menu_is_framed_like_the_model_picker_with_a_hint_row() {
    let app = settings_app();
    let buf = render(&app, 78);
    let last = buf.area.height - 1;
    assert!(row(&buf, 0, 78).starts_with('─'), "top rule");
    assert!(row(&buf, 1, 78).trim().is_empty(), "gap, no header banner");
    assert!(
        row(&buf, SETTINGS_SEARCH_ROW, 78).contains('❯'),
        "the search line"
    );
    assert!(row(&buf, last, 78).starts_with('─'), "bottom rule");
    assert!(row(&buf, last - 1, 78).trim().is_empty(), "trailing gap");
    assert!(
        row(&buf, last - 2, 78).contains(SETTINGS_HINT),
        "the key hint sits just above the trailing gap: {:?}",
        row(&buf, last - 2, 78)
    );
}

#[test]
fn the_height_is_the_chrome_plus_the_listed_rows() {
    let mut app = settings_app();
    assert_eq!(
        settings_height(&app, 78, 200),
        Some(SETTINGS_CHROME_ROWS + visible_rows())
    );
    // A narrowed search shrinks the list — and so the region.
    for c in "retry".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(
        settings_height(&app, 78, 200),
        Some(SETTINGS_CHROME_ROWS + 1)
    );
    // …and it is clamped to the terminal.
    assert_eq!(settings_height(&app, 78, 5), Some(5));
    // Closed, there is no menu height at all (the caller falls back to the
    // composer's).
    app.close_settings();
    assert_eq!(settings_height(&app, 78, 200), None);
}

#[test]
fn the_menu_reserves_the_running_turn_strip_above_it() {
    // `/settings` opens mid-turn like `/model` and `/login`, and replaces the
    // composer only: the streaming strip (here a running tool's live cell over
    // the spinner status line) keeps its rows above the menu's frame — the ↓
    // manager band's rule (docs/background.md, docs/settings.md).
    let mut app = settings_app();
    app.begin_stream();
    app.start_tool("Bash", "cargo test");
    let preview = preview_rows(&app, 78);
    assert!(preview > 0, "the running tool previews mid-turn");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    assert_eq!(
        settings_height(&app, 78, 200),
        Some(strip + SETTINGS_CHROME_ROWS + visible_rows())
    );
    // Idle again, the menu is alone — the old geometry.
    app.end_tool("ok", true);
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(
        settings_height(&app, 78, 200),
        Some(SETTINGS_CHROME_ROWS + visible_rows())
    );
}

#[test]
fn every_row_shows_its_label_and_value_and_the_first_is_marked() {
    let app = settings_app();
    let buf = render(&app, 78);
    // The list starts at row 4: top(0) gap(1) search(2) gap(3).
    let first = row(&buf, 4, 78);
    assert!(first.starts_with("→ Hide thinking"), "{first:?}");
    assert!(first.contains("false"), "{first:?}");
    let second = row(&buf, 5, 78);
    assert!(second.starts_with("  Error retry"), "{second:?}");
    assert!(second.contains('3'), "{second:?}");
    // The selected row's marker takes the picker family's cyan accent.
    assert_eq!(buf[(0, 4)].fg, MODEL_SELECTED_COLOR);
}

#[test]
fn the_values_line_up_in_a_column() {
    // Sized off the widest visible label, so the column reads as a block
    // rather than tracking each label's length.
    let app = settings_app();
    let buf = render(&app, 78);
    // Display columns, not byte offsets — the selected row's `→` is 3 bytes.
    let value_col = |y: u16, needle: char| {
        row(&buf, y, 78)
            .chars()
            .position(|c| c == needle)
            .expect("the row has a value")
    };
    assert_eq!(
        value_col(4, 'f'),
        value_col(5, '3'),
        "the value column is shared"
    );
    assert_eq!(
        value_col(4, 'f'),
        // marker (2) + the widest label ("Permission mode", 15) + the gap (3).
        2 + 15 + 3,
        "sized off the widest visible label"
    );
}

#[test]
fn an_off_value_is_dimmed_and_an_on_value_is_not() {
    // A glance down the column should show what is actually doing something.
    let app = settings_app();
    let buf = render(&app, 78);
    let line = row(&buf, 4, 78);
    let at = u16::try_from(line.find("false").unwrap()).unwrap();
    assert_eq!(buf[(at, 4)].fg, SETTINGS_VALUE_OFF_COLOR, "false is dim");
    let line = row(&buf, 5, 78);
    let at = u16::try_from(line.find('3').unwrap()).unwrap();
    assert_eq!(buf[(at, 5)].fg, SETTINGS_VALUE_COLOR, "a live count is not");
}

#[test]
fn the_counter_and_description_track_the_selection() {
    let mut app = settings_app();
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let buf = render(&app, 78);
    let rows = visible_rows();
    let total = SettingKey::ALL.len();
    // counter = 4 (list start) + the rows actually drawn; the count it shows
    // is the whole list, not the window.
    let counter = row(&buf, 4 + rows, 78);
    assert!(counter.contains(&format!("(2/{total})")), "{counter:?}");
    // description = counter + gap + 1.
    let description = row(&buf, 4 + rows + 2, 78);
    assert!(
        description.contains(SettingKey::ErrorRetry.description()),
        "{description:?}"
    );
}

#[test]
fn the_search_query_shows_after_the_prompt() {
    let mut app = settings_app();
    for c in "temp".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    let search = row(&buf, SETTINGS_SEARCH_ROW, 78);
    assert!(search.contains("❯ temp"), "{search:?}");
    assert_eq!(buf[(2, SETTINGS_SEARCH_ROW)].fg, MODEL_SELECTED_COLOR);
    // …and the list narrowed to the one match.
    assert!(
        row(&buf, 4, 78).contains("Temperature"),
        "{:?}",
        row(&buf, 4, 78)
    );
}

#[test]
fn a_search_matching_nothing_shows_a_placeholder_and_a_blank_counter() {
    let mut app = settings_app();
    for c in "zzz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    assert!(row(&buf, 4, 78).contains("No matching settings"));
    assert!(row(&buf, 5, 78).trim().is_empty(), "no counter to show");
    // The frame is intact — the hint and rules still sit where they belong.
    let last = buf.area.height - 1;
    assert!(row(&buf, last, 78).starts_with('─'));
    assert!(row(&buf, last - 2, 78).contains(SETTINGS_HINT));
}

#[test]
fn an_unavailable_row_is_labelled_and_dimmed() {
    let mut app = settings_app();
    app.set_setting_availability(SettingAvailability {
        checkpoints: false,
        hooks: true,
        skills: true,
    });
    let buf = render(&app, 78);
    let all = SettingKey::ALL.len();
    let checkpoints = (0..all)
        .map(|i| row(&buf, 4 + u16::try_from(i).unwrap(), 78))
        .find(|line| line.contains("Checkpoints"))
        .expect("the row is listed");
    assert!(
        checkpoints.contains("false (unavailable)"),
        "{checkpoints:?}"
    );
}

#[test]
fn a_long_list_keeps_the_selection_centered() {
    // The window is the `/model` picker's `centered_window`, so a deep
    // selection shows rows either side of it rather than being pinned to an
    // edge. Our list is exactly at the cap, so End puts the last row in view.
    let mut app = settings_app();
    app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let buf = render(&app, 78);
    let visible: Vec<String> = (0..settings_height(&app, 78, 200).unwrap())
        .map(|y| row(&buf, y, 78))
        .collect();
    let last = SettingKey::ALL.last().unwrap().label();
    assert!(
        visible.iter().any(|l| l.starts_with(&format!("→ {last}"))),
        "the last row ({last}) is visible and marked: {visible:?}"
    );
}

#[test]
fn the_cursor_sits_at_the_end_of_the_search_query() {
    let mut app = settings_app();
    for c in "tools".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let height = settings_height(&app, 78, 200).unwrap();
    let area = Rect::new(0, 3, 78, height);
    let (x, y) = cursor_position(area, &app);
    assert_eq!(y, 3 + SETTINGS_SEARCH_ROW, "on the search row");
    // indent (2) + `❯ ` (2) + the 5-character query.
    assert_eq!(x, 2 + 2 + 5);
}

#[test]
fn a_narrow_terminal_truncates_rather_than_overflowing() {
    let app = settings_app();
    let buf = render(&app, 24);
    for y in 0..buf.area.height {
        assert_eq!(
            row(&buf, y, 24).chars().count(),
            24,
            "row {y} is exactly the width"
        );
    }
}

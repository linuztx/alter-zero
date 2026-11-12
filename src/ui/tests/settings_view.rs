//! The inline `/settings` menu's rendering (`docs/state/settings.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::permission::PermissionMode;
use crate::settings::{SettingAvailability, SettingKey};
use crate::ui::theme::{
    GAP_ROWS, SETTINGS_HINT, SETTINGS_MENU_MAX_ROWS, STATUS_GAP_ROWS, STATUS_ROWS,
    model_selected_color, settings_value_color, settings_value_off_color,
};

/// The fixed rows framing the menu: top rule, title, subtitle, gap, search,
/// gap (6 above the list), then counter, gap, description, gap, hint, bottom
/// rule (6 below) — pinned here so a builder change that adds or drops a
/// chrome row fails a height test (`settings_view_lines`). Counted at
/// [`WIDE`], where every description these tests highlight is one row.
const SETTINGS_CHROME_ROWS: u16 = 12;

/// A terminal wide enough for the descriptions the height tests highlight
/// to read on one row under the labels.
const WIDE: u16 = 90;

/// The page row the `❯` search line sits on on the whole page: top rule
/// (0), title (1), subtitle (2), gap (3), search (4).
const SETTINGS_SEARCH_ROW: u16 = 4;

/// The page row the first setting sits on: the search line, a gap, then the
/// list.
const FIRST_ROW: u16 = SETTINGS_SEARCH_ROW + 2;

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
    let height = settings_height(app, width, 200).expect("the menu is open");
    let mut buf = buffer(width, height);
    render_settings(buf.area, &mut buf, app);
    buf
}

// --- the page's shape (docs/state/settings.md, docs/ui/view-flow.md) ---

#[test]
fn the_whole_page_is_titled_and_lists_its_rows_under_the_search_prompt() {
    // The user's mock: a title over a dim line saying what a change does,
    // a gap, the search line, a gap, five rows whose `→` sits under the
    // `❯`, the counter and the highlighted row's description hanging under
    // the labels, a gap, the hint.
    let app = settings_app();
    let rule = "─".repeat(90);
    let counter = format!("    (1/{})", SettingKey::ALL.len());
    assert_eq!(
        page_at(&app, 90, TALL),
        [
            rule.as_str(),
            "  Settings",
            "  Changes take effect at once and persist across sessions",
            "",
            "  ❯",
            "",
            "  → Hide thinking        false",
            "    Verbose output       false",
            "    Show images          true",
            "    Image width          120",
            "    Auto-resize images   true",
            counter.as_str(),
            "",
            "    Hide the model's chain-of-thought instead of streaming it above the composer",
            "",
            "  Type to search · Enter/Space to change · Esc to cancel",
            rule.as_str(),
        ]
    );
}

#[test]
fn the_title_wears_the_page_title_dress_over_a_dim_subtitle() {
    let app = settings_app();
    let lines = crate::ui::settings_view::settings_view_lines(&app, 90, TALL);
    assert!(wears_the_title_dress(&lines[1]), "{:?}", lines[1]);
    assert!(
        lines[2]
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| span.style.fg == Some(crate::ui::theme::model_meta_color())),
        "{:?}",
        lines[2]
    );
}

#[test]
fn the_window_shows_five_settings_and_the_counter_names_the_rest() {
    let mut app = settings_app();
    let labels = labels(&app);
    assert_eq!(SETTINGS_MENU_MAX_ROWS, 5);
    assert_eq!(setting_rows(&page_at(&app, 90, TALL), &labels), 5);
    // Deep in the list the window rides the selection, centred on it.
    select(&mut app, 7); // Permission mode
    let texts = page_at(&app, 90, TALL);
    let rows: Vec<&String> = texts
        .iter()
        .filter(|t| value_col(t, &labels).is_some())
        .collect();
    assert_eq!(rows.len(), 5, "{texts:#?}");
    assert!(rows[2].starts_with("  → Permission mode"), "{texts:#?}");
    assert!(
        texts
            .iter()
            .any(|t| *t == format!("    (8/{})", SettingKey::ALL.len())),
        "{texts:#?}"
    );
}

#[test]
fn the_menu_is_framed_like_the_model_picker_with_a_hint_row() {
    let app = settings_app();
    let buf = render(&app, 78);
    let last = buf.area.height - 1;
    assert!(row(&buf, 0, 78).starts_with('─'), "top rule");
    assert_eq!(
        row(&buf, 1, 78).trim_end(),
        "  Settings",
        "the title hugs the rule"
    );
    assert!(
        row(&buf, SETTINGS_SEARCH_ROW, 78).contains('❯'),
        "the search line under the header"
    );
    assert!(row(&buf, last, 78).starts_with('─'), "bottom rule");
    assert!(
        row(&buf, last - 1, 78).contains(SETTINGS_HINT),
        "the key hint sits straight over the bottom rule: {:?}",
        row(&buf, last - 1, 78)
    );
}

#[test]
fn the_height_is_the_chrome_plus_the_listed_rows() {
    let mut app = settings_app();
    assert_eq!(
        settings_height(&app, WIDE, 200),
        Some(SETTINGS_CHROME_ROWS + visible_rows())
    );
    // A narrowed search shrinks the list — and so the region.
    for c in "retry".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(
        settings_height(&app, WIDE, 200),
        Some(SETTINGS_CHROME_ROWS + 1)
    );
    // …and it is clamped to the terminal.
    assert_eq!(settings_height(&app, WIDE, 5), Some(5));
    // Closed, there is no menu height at all (the caller falls back to the
    // composer's).
    app.close_settings();
    assert_eq!(settings_height(&app, WIDE, 200), None);
}

#[test]
fn the_menu_reserves_the_running_turn_strip_above_it() {
    // `/settings` opens mid-turn like `/model` and `/login`, and replaces the
    // composer only: the streaming strip (here a running tool's live cell over
    // the spinner status line) keeps its rows above the menu's frame — the ↓
    // manager band's rule (docs/tools/background.md, docs/state/settings.md).
    let mut app = settings_app();
    app.begin_stream();
    app.start_tool("Bash", "cargo test", None);
    let preview = preview_rows(&app, WIDE);
    assert!(preview > 0, "the running tool previews mid-turn");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    assert_eq!(
        settings_height(&app, WIDE, 200),
        Some(strip + SETTINGS_CHROME_ROWS + visible_rows())
    );
    // Idle again, the menu is alone — the old geometry.
    app.end_tool("ok", true);
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(
        settings_height(&app, WIDE, 200),
        Some(SETTINGS_CHROME_ROWS + visible_rows())
    );
}

#[test]
fn every_row_shows_its_label_and_value_and_the_first_is_marked() {
    let app = settings_app();
    let buf = render(&app, 78);
    // The list starts under the search line and its gap, the `→` under the
    // `❯`.
    let first = row(&buf, FIRST_ROW, 78);
    assert!(first.starts_with("  → Hide thinking"), "{first:?}");
    assert!(first.contains("false"), "{first:?}");
    let second = row(&buf, FIRST_ROW + 1, 78);
    assert!(second.starts_with("    Verbose output"), "{second:?}");
    assert!(second.contains("false"), "{second:?}");
    let third = row(&buf, FIRST_ROW + 2, 78);
    assert!(third.starts_with("    Show images"), "{third:?}");
    assert!(third.contains("true"), "{third:?}");
    // The selected row's marker takes the picker family's cyan accent.
    assert_eq!(buf[(2, FIRST_ROW)].fg, model_selected_color());
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
        value_col(FIRST_ROW, 'f'),
        value_col(FIRST_ROW + 2, 't'),
        "the value column is shared"
    );
    assert_eq!(
        value_col(FIRST_ROW, 'f'),
        // indent (2) + marker (2) + the widest label ("Auto-resize images",
        // 18) + the gap (3).
        2 + 2 + 18 + 3,
        "sized off the widest listed label"
    );
}

#[test]
fn an_off_value_is_dimmed_and_an_on_value_is_not() {
    // A glance down the column should show what is actually doing something.
    let app = settings_app();
    let buf = render(&app, 78);
    let line = row(&buf, FIRST_ROW, 78);
    let at = u16::try_from(line.find("false").unwrap()).unwrap();
    assert_eq!(
        buf[(at, FIRST_ROW)].fg,
        settings_value_off_color(),
        "false is dim"
    );
    let line = row(&buf, FIRST_ROW + 2, 78);
    let at = u16::try_from(line.find("true").unwrap()).unwrap();
    assert_eq!(
        buf[(at, FIRST_ROW + 2)].fg,
        settings_value_color(),
        "a live value is not"
    );
}

#[test]
fn the_counter_and_description_track_the_selection() {
    let mut app = settings_app();
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let buf = render(&app, WIDE);
    let rows = visible_rows();
    let total = SettingKey::ALL.len();
    // counter = the list start + the rows actually drawn, hanging under the
    // labels; the count it shows is the whole list, not the window.
    let counter = row(&buf, FIRST_ROW + rows, WIDE);
    assert!(
        counter.starts_with(&format!("    (2/{total})")),
        "{counter:?}"
    );
    // description = counter + gap + 1, under the labels too.
    let description = row(&buf, FIRST_ROW + rows + 2, WIDE);
    assert!(description.starts_with("    A cell per"), "{description:?}");
    assert!(
        description.contains(SettingKey::Verbose.description()),
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
    assert_eq!(buf[(2, SETTINGS_SEARCH_ROW)].fg, model_selected_color());
    // …and the list narrowed to the one match.
    assert!(
        row(&buf, FIRST_ROW, 78).contains("Temperature"),
        "{:?}",
        row(&buf, FIRST_ROW, 78)
    );
}

#[test]
fn a_search_matching_nothing_shows_a_placeholder_and_a_blank_counter() {
    let mut app = settings_app();
    for c in "zzz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    assert!(row(&buf, FIRST_ROW, 78).contains("No matching settings"));
    assert!(
        row(&buf, FIRST_ROW + 1, 78).trim().is_empty(),
        "no counter to show"
    );
    // The frame is intact — the hint and rules still sit where they belong.
    let last = buf.area.height - 1;
    assert!(row(&buf, last, 78).starts_with('─'));
    assert!(row(&buf, last - 1, 78).contains(SETTINGS_HINT));
}

#[test]
fn an_unavailable_row_is_labelled_and_dimmed() {
    let mut app = settings_app();
    app.set_setting_availability(SettingAvailability {
        checkpoints: false,
        hooks: true,
        skills: true,
        mcp: true,
        images: true,
        telemetry: true,
        update_check: true,
    });
    let at = labels(&app)
        .iter()
        .position(|label| *label == "Checkpoints")
        .expect("the row");
    select(&mut app, at);
    let checkpoints = page_at(&app, 78, TALL)
        .into_iter()
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
        visible
            .iter()
            .any(|l| l.starts_with(&format!("  → {last}"))),
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

#[test]
fn a_squeezed_settings_menu_paints_the_page_built_for_its_height() {
    // A terminal shorter than the compact page no longer bottom-anchors it —
    // which flowed the search line, and then the highlighted row, out of
    // sight — but gets a smaller rung that fits it whole: the region holds
    // exactly the page built for that height, every row painted, the search
    // line on screen. Each rung is a row shorter than the last but where the
    // overflow pair goes, two rows at once.
    let app = settings_app();
    let width = 78u16;
    for height in 1..=9u16 {
        let page = page_at(&app, width, height);
        let short_by = if height == 8 { 1 } else { 0 };
        assert_eq!(
            page.len(),
            usize::from(height - short_by),
            "{height}: {page:#?}"
        );
        let region = settings_height(&app, width, height).expect("open");
        assert_eq!(
            usize::from(region),
            page.len(),
            "{height}: the region is the page"
        );
        let mut buf = buffer(width, region);
        render_settings(buf.area, &mut buf, &app);
        let painted: Vec<String> = (0..region)
            .map(|y| row(&buf, y, width).trim_end().to_string())
            .collect();
        assert_eq!(painted, page, "{height}: the page, unskipped");
        assert!(painted.iter().any(|r| r == "  ❯"), "{height}: {painted:#?}");
    }
}

// --- the no-match page collapses its empty slots (docs/state/settings.md) ---

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // Nothing matched means there is no count and no description, so those
    // slots collapse to ONE blank gap instead of a band of empty rows (the
    // `/model` picker's placeholder rule). The whole page is rule, header,
    // blank, search, blank, placeholder, blank, hint, rule.
    let mut app = App::new();
    app.open_settings();
    app.settings_picker.as_mut().expect("open").query = "zzz".to_string();
    let texts: Vec<String> = crate::ui::settings_view::settings_view_lines(&app, 78, TALL)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 10, "the collapsed page is 10 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "  Settings", "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains('❯'), "{texts:?}");
    assert_eq!(texts[5], "", "{texts:?}");
    assert!(texts[6].contains("No matching settings"), "{texts:?}");
    assert_eq!(texts[7], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[8].contains("Type to search"), "{texts:?}");
    assert!(is_rule(&texts[9]), "{texts:?}");
}

#[test]
fn every_settings_page_hugs_its_frame() {
    // The search line straight under the opening rule, the key hint
    // straight over the closing one — a real selection, a narrowed search
    // and a search matching nothing alike.
    let mut app = settings_app();
    for query in ["", "tool", "zzz"] {
        app.settings_picker.as_mut().expect("open").query = query.to_string();
        assert_frame_hugs_content(
            &format!("the menu searching {query:?}"),
            &crate::ui::settings_view::settings_view_lines(&app, 78, TALL),
        );
    }
}

#[test]
fn no_settings_page_ever_stacks_two_blank_rows() {
    let mut app = App::new();
    app.open_settings();
    for query in ["", "tool", "zzz"] {
        app.settings_picker.as_mut().expect("open").query = query.to_string();
        let texts: Vec<String> = crate::ui::settings_view::settings_view_lines(&app, 78, TALL)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        for pair in texts.windows(2) {
            assert!(
                !(pair[0].is_empty() && pair[1].is_empty()),
                "query {query:?} stacked two blank rows: {texts:?}"
            );
        }
    }
}

#[test]
fn the_highlighted_description_wraps_instead_of_clipping() {
    // The description is why the row exists (type-to-search even matches on
    // it) — at a narrow width it wraps whole below the counter; the page's
    // height is the built line count, so the extra rows are safe.
    let app = settings_app();
    let lines = crate::ui::settings_view::settings_view_lines(&app, 34, TALL);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 34,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    // The first row's description survives whole (word-joined across rows).
    let desc = app.setting_rows()[0].description;
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let tail = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(all.contains(&tail), "nothing clipped: {all:?}");
}

#[test]
fn a_cut_value_ends_with_an_ellipsis() {
    // "false (unavailable)" clipped to "false" misrepresents an unavailable
    // knob as a live false — a cut value must say it was cut.
    use crate::app::SettingRow;
    use crate::settings::SettingKey;
    use crate::ui::theme::SETTINGS_VALUE_GAP;
    let row = SettingRow {
        key: SettingKey::Checkpoints,
        label: "Checkpoints",
        value: "false (unavailable)".into(),
        description: "d",
        available: false,
    };
    // marker(2) + label(11) + gap → value room too small for the tail.
    let width = (2 + 11 + SETTINGS_VALUE_GAP + 8) as u16;
    let line = crate::ui::settings_view::settings_row(&row, false, 11, width);
    let text = plain(&line);
    assert!(text.trim_end().ends_with('…'), "{text:?}");
}

#[test]
fn a_never_ask_timeout_is_dimmed_like_any_off_value() {
    // `never` is the Ask timeout's off state (docs/tools/ask.md): no clock runs.
    let mut app = settings_app();
    app.settings_mut().ask_timeout_secs = 0;
    for c in "timeout".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    let line = row(&buf, FIRST_ROW, 78);
    assert!(line.contains("Ask timeout"), "{line:?}");
    let at = u16::try_from(line.find("never").expect("the value")).unwrap();
    assert_eq!(
        buf[(at, FIRST_ROW)].fg,
        settings_value_off_color(),
        "never is dim"
    );
}

// --- the compact page: a short terminal keeps the choices on screen ---

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// The menu's page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    crate::ui::settings_view::settings_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// Highlight row `i` of the filtered list.
fn select(app: &mut App, i: usize) {
    app.settings_picker.as_mut().expect("open").selected = i;
}

/// The settings' labels, in list order.
fn labels(app: &App) -> Vec<&'static str> {
    app.setting_rows().iter().map(|row| row.label).collect()
}

/// The column the value starts at on a setting row, or `None` for a row
/// that is not one.
fn value_col(text: &str, labels: &[&str]) -> Option<usize> {
    let body = text.get(text.find(|c: char| c != ' ' && c != '→')?..)?;
    let label = labels.iter().find(|label| body.starts_with(**label))?;
    let after = &body[label.len()..];
    let value = after.trim_start();
    if value.is_empty() || after.len() == value.len() {
        return None;
    }
    Some(crate::ui::wrap::cols(text) - crate::ui::wrap::cols(value))
}

fn setting_rows(texts: &[String], labels: &[&str]) -> usize {
    texts
        .iter()
        .filter(|t| value_col(t, labels).is_some())
        .count()
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_setting_choices_on_screen() {
    // The whole page is 18 rows (the first description wraps under the
    // labels at 78 columns). On 12, bottom-anchoring it flowed the search
    // line and the highlighted row into scrollback — the row Enter would
    // change was off screen. The description and the counter yield instead,
    // and three rows of the list stay between the search line and the hint.
    let app = settings_app();
    let labels = labels(&app);
    assert_eq!(page_at(&app, 78, TALL).len(), 18, "the whole page");
    let texts = page_at(&app, 78, 12);
    assert!(texts.len() <= 12, "fits: {texts:#?}");
    assert!(
        texts.iter().any(|t| t == "  ❯"),
        "the search line: {texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("  → Hide thinking")),
        "the highlighted setting: {texts:#?}"
    );
    assert_eq!(setting_rows(&texts, &labels), 3, "three rows: {texts:#?}");
    assert!(
        texts
            .iter()
            .any(|t| *t == format!("  ↓ {} more below", SettingKey::ALL.len() - 3)),
        "the rest named under them: {texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(SETTINGS_HINT)),
        "{texts:#?}"
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains(&format!("(1/{})", SettingKey::ALL.len()))),
        "no counter: {texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("chain-of-thought")),
        "no description: {texts:#?}"
    );
}

#[test]
fn the_compact_settings_page_is_the_familys_short_page() {
    // Rule, the header and its gap, the search line with the `↑` slot
    // directly under it, three rows, the `↓` slot directly over the hint,
    // rule — the /theme and /skills compact pages, row for row, no blank row
    // hugging either rule.
    let texts = page_at(&settings_app(), 78, 12);
    assert_eq!(texts.len(), 12, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "  Settings", "the header stays: {texts:#?}");
    assert_eq!(texts[3], "", "{texts:#?}");
    assert_eq!(texts[4], "  ❯", "{texts:#?}");
    assert_eq!(texts[5], "", "nothing above the first row: {texts:#?}");
    assert!(texts[6].starts_with("  → Hide thinking"), "{texts:#?}");
    assert!(texts[7].starts_with("    Verbose output"), "{texts:#?}");
    assert!(texts[8].starts_with("    Show images"), "{texts:#?}");
    assert_eq!(
        texts[9],
        format!("  ↓ {} more below", SettingKey::ALL.len() - 3),
        "{texts:#?}"
    );
    assert_eq!(texts[10], format!("  {SETTINGS_HINT}"), "{texts:#?}");
    assert!(is_rule(&texts[11]), "{texts:#?}");
    // A row shorter, the header yields too: the search line hugs the rule.
    let texts = page_at(&settings_app(), 78, 11);
    assert_eq!(texts.len(), 9, "{texts:#?}");
    assert_eq!(texts[1], "  ❯", "{texts:#?}");
    assert!(texts[3].starts_with("  → Hide thinking"), "{texts:#?}");
    assert_eq!(texts[7], format!("  {SETTINGS_HINT}"), "{texts:#?}");
}

#[test]
fn the_compact_settings_window_keeps_the_selection_in_its_middle() {
    let mut app = settings_app();
    select(&mut app, 7); // Permission mode
    let texts = page_at(&app, 78, 12);
    assert_eq!(texts[5], "  ↑ 6 more above", "{texts:#?}");
    assert!(texts[6].starts_with("    Tools"), "{texts:#?}");
    assert!(texts[7].starts_with("  → Permission mode"), "{texts:#?}");
    assert!(texts[8].starts_with("    Checkpoints"), "{texts:#?}");
    assert_eq!(
        texts[9],
        format!("  ↓ {} more below", SettingKey::ALL.len() - 9),
        "{texts:#?}"
    );
}

#[test]
fn the_compact_settings_page_holds_still_across_selections() {
    // Neither the frame nor the value column moves as ↑/↓ walk the window:
    // the value column is sized off every listed row, not the three in view.
    let mut app = settings_app();
    let labels = labels(&app);
    let h0 = page_at(&app, 78, 12).len();
    let col0 = page_at(&app, 78, 12)
        .iter()
        .find_map(|t| value_col(t, &labels))
        .expect("a setting row");
    for i in 0..labels.len() {
        select(&mut app, i);
        let texts = page_at(&app, 78, 12);
        assert_eq!(texts.len(), h0, "selection {i} moved the frame");
        for text in &texts {
            if let Some(col) = value_col(text, &labels) {
                assert_eq!(col, col0, "selection {i} moved the values: {texts:#?}");
            }
        }
    }
}

#[test]
fn a_long_settings_description_is_cut_to_one_row_before_the_list_shrinks() {
    // `Ask timeout`'s description wraps to two rows at 90 columns, so its
    // whole page is a row taller than the first setting's. On a terminal
    // one row short of that, the page keeps its whole list and cuts the
    // description to one row — the page is one height at every selection,
    // so it never flips to the compact page and back as ↑/↓ pass it.
    let mut app = settings_app();
    let labels = labels(&app);
    let at = labels
        .iter()
        .position(|l| *l == "Ask timeout")
        .expect("row");
    select(&mut app, at);
    assert_eq!(page_at(&app, WIDE, TALL).len(), 18, "wrapped to two rows");
    let texts = page_at(&app, WIDE, 17);
    assert_eq!(texts.len(), 17, "{texts:#?}");
    assert_eq!(
        setting_rows(&texts, &labels),
        5,
        "the whole window: {texts:#?}"
    );
    let description = texts
        .iter()
        .find(|t| t.contains("How long a question waits"))
        .expect("the description");
    assert!(
        description.ends_with('…'),
        "cut to one row: {description:?}"
    );
    // The first setting's one-row description fits that terminal whole.
    select(&mut app, 0);
    assert_eq!(page_at(&app, WIDE, 17), page_at(&app, WIDE, TALL));
}

#[test]
fn the_whole_settings_page_returns_once_it_fits() {
    // At a width where the first description is one row, cutting it to one
    // row saves nothing, so a row short is the compact page.
    let app = settings_app();
    let whole = u16::try_from(page_at(&app, WIDE, TALL).len()).expect("rows");
    let counter = format!("(1/{})", SettingKey::ALL.len());
    let has_counter = |texts: &[String]| texts.iter().any(|t| t.contains(&counter));
    assert!(
        has_counter(&page_at(&app, WIDE, whole)),
        "whole at its own height"
    );
    assert!(
        !has_counter(&page_at(&app, WIDE, whole - 1)),
        "compact a row short"
    );
}

#[test]
fn a_compact_settings_page_fits_and_never_overflows_or_stacks_blanks() {
    let mut app = settings_app();
    for width in [24u16, 40, 78] {
        for query in ["", "image", "tools", "zzz"] {
            app.settings_picker.as_mut().expect("open").query = query.to_string();
            let len = app.setting_rows().len().max(1);
            for i in 0..len {
                select(&mut app, i);
                for height in [11u16, 12, 16] {
                    let texts = page_at(&app, width, height);
                    let context = format!("width {width} height {height} query {query:?} row {i}");
                    assert!(texts.len() <= usize::from(height), "{context}: {texts:#?}");
                    for text in &texts {
                        assert!(
                            crate::ui::wrap::cols(text) <= usize::from(width),
                            "{context}: {text:?} overflows"
                        );
                    }
                    for pair in texts.windows(2) {
                        assert!(
                            !(pair[0].is_empty() && pair[1].is_empty()),
                            "{context} stacked two blank rows: {texts:#?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_cursor_sits_on_the_compact_settings_search_line() {
    let mut app = settings_app();
    for c in "o".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // On 12 rows the compact page keeps its header, so the search line sits
    // under it; on 11 the header has yielded and it hugs the rule.
    for (rows, search) in [(12, SETTINGS_SEARCH_ROW), (11, SEARCH_ROW)] {
        let height = settings_height(&app, 78, rows).expect("open");
        assert_eq!(height, if rows == 12 { 12 } else { 9 }, "the compact page");
        let area = Rect::new(0, 1, 78, height);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, 1 + search, "{rows} rows: on the search line");
        // indent (2) + `❯ ` (2) + the one-character query.
        assert_eq!(x, 2 + 2 + 1);
    }
}

#[test]
fn the_settings_menu_never_flows() {
    let app = settings_app();
    for height in (1..=12).chain([TALL]) {
        assert!(
            crate::ui::view_flow(&app, 78, height, 1000).is_none(),
            "{height} rows: a rung fits"
        );
    }
}

#[test]
fn the_settings_menu_renders_the_page_its_height_reserves() {
    let app = settings_app();
    let height = settings_height(&app, 78, 12).expect("open");
    let mut buf = buffer(78, height);
    render_settings(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..height).map(|y| row(&buf, y, 78)).collect();
    assert!(is_rule(rows.last().expect("rows").trim()), "{rows:#?}");
    assert!(rows.iter().any(|r| r.trim() == "❯"), "{rows:#?}");
    assert!(
        rows.iter().any(|r| r.starts_with("  → Hide thinking")),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|r| r.contains(SETTINGS_HINT)), "{rows:#?}");
}

// --- a terminal shorter than the compact page keeps the search line ---

#[test]
fn a_tiny_terminal_keeps_the_settings_search_line() {
    let app = settings_app();
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 78, height),
        "",
        "→ Hide thinking",
        "Enter/Space to change",
    );
    for height in 1..=10 {
        assert_caret_on_search_line(&app, &page_at(&app, 78, height), 78, "");
    }
}

#[test]
fn a_tiny_terminal_keeps_what_was_typed_over_no_settings_match() {
    let mut app = settings_app();
    for c in "zzz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_sheds_to_its_search_line(
        |height| page_at(&app, 78, height),
        "zzz",
        "No matching settings",
        "Enter/Space to change",
    );
    for height in 1..=10 {
        assert_caret_on_search_line(&app, &page_at(&app, 78, height), 78, "zzz");
    }
}

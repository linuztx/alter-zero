//! The inline `/skills` menu's rendering (`docs/skills.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::skills::SkillMetadata;
use crate::ui::theme::{
    GAP_ROWS, SKILLS_HINT, SKILLS_NO_MATCH, SKILLS_NONE_FOUND, SKILLS_OFF_MARK, SKILLS_ON_MARK,
    SKILLS_SESSION_OFF, SKILLS_TITLE, STATUS_GAP_ROWS, STATUS_ROWS, model_active_color,
    model_meta_color, model_selected_color, settings_value_color, settings_value_off_color,
};

/// The page row the first skill sits on: top rule (0), title (1), count
/// (2), gap (3), search (4), gap (5) — the session-off note, when it
/// applies, takes rows under the count.
const FIRST_ROW: u16 = 6;

/// The page row the `❯` search line sits on when no note is shown.
const SEARCH_ROW: u16 = 4;

fn meta(name: &str, description: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: description.to_string(),
        dir: std::path::PathBuf::from("/skills").join(name),
        path: std::path::PathBuf::from("/skills")
            .join(name)
            .join("SKILL.md"),
    }
}

/// An app with the menu open over two skills, the second turned off.
fn skills_app() -> App {
    let mut app = App::new();
    app.open_skills_menu(
        vec![
            meta("commit-helper", "Write a commit message in house style"),
            meta("dataviz", "Charts and dashboards"),
        ],
        ["dataviz".to_string()].into_iter().collect(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    app
}

/// Render the menu at its natural height, like the boundary does — otherwise
/// the list's `Min(0)` would expand and push the rows below it down.
fn render(app: &App, width: u16) -> Buffer {
    let height = skills_menu_height(app, width, 200).expect("the menu is open");
    let mut buf = buffer(width, height);
    render_skills_menu(buf.area, &mut buf, app);
    buf
}

#[test]
fn every_skill_shows_its_state_and_the_first_is_marked() {
    let buf = render(&skills_app(), 78);
    let text = (0..buf.area.height)
        .map(|y| row(&buf, y, 78))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("→ commit-helper") && text.contains("enabled"),
        "{text}"
    );
    assert!(
        text.contains("dataviz") && text.contains("disabled"),
        "{text}"
    );
    // The counter, the highlighted row's own description, and the key hint.
    assert!(text.contains("(1/2)"), "{text}");
    assert!(
        text.contains("Write a commit message in house style"),
        "the description names the highlighted skill: {text}"
    );
    assert!(text.contains(SKILLS_HINT), "{text}");
}

/// The column `needle` starts at on row `y`, walked cell by cell: a
/// multi-byte glyph (`→`, `✔`, `◯`) is one cell, so a byte offset into the
/// row's string lands columns to the right of the cell it means.
fn col_of(buf: &Buffer, y: u16, width: u16, needle: &str) -> u16 {
    (0..width)
        .find(|&x| {
            (x..width)
                .map(|cx| buf[(cx, y)].symbol())
                .collect::<String>()
                .starts_with(needle)
        })
        .unwrap_or_else(|| panic!("{needle:?} on row {y}: {:?}", row(buf, y, width)))
}

/// The cell colour at the first occurrence of `needle` on row `y` — the
/// settings-view tests' idiom.
fn colour_at(buf: &Buffer, y: u16, width: u16, needle: &str) -> ratatui::style::Color {
    buf[(col_of(buf, y, width, needle), y)].fg
}

#[test]
fn the_page_opens_on_a_title_over_the_skill_count() {
    // The pages' header (`/hooks`, `/mcp`): the title straight under the top
    // rule in the pages' one title dress, the catalog's size under it, then
    // a gap and the search line.
    let app = skills_app();
    let lines = crate::ui::skills_view::skills_view_lines(&app, 78);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[1], format!("  {SKILLS_TITLE}"), "{texts:?}");
    assert!(wears_the_title_dress(&lines[1]), "{:?}", lines[1]);
    assert_eq!(texts[2], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains('❯'), "the search line follows: {texts:?}");
}

#[test]
fn the_count_line_is_dim_and_counts_the_catalog_not_the_search() {
    // The `(n/total)` counter already follows the search; the header says
    // what is installed, so a query must not shrink it.
    let mut app = skills_app();
    for c in "dataviz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    assert_eq!(row(&buf, 2, 78).trim_end(), "  2 skills · 1 enabled");
    assert_eq!(
        buf[(2, 2)].fg,
        model_meta_color(),
        "dim, like /hooks' count"
    );
}

#[test]
fn the_count_follows_a_toggle_in_the_same_frame() {
    let mut app = skills_app();
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(page(&app, 78)[2], "  2 skills · 0 enabled");
}

#[test]
fn a_lone_skill_is_counted_in_the_singular() {
    let mut app = App::new();
    app.open_skills_menu(vec![meta("pdf", "d")], Default::default(), true, vec![]);
    assert_eq!(page(&app, 78)[2], "  1 skill · 1 enabled");
}

#[test]
fn an_empty_catalog_counts_zero_with_no_enabled_clause() {
    // "0 skills · 0 enabled" says nothing twice; the placeholder below it is
    // what answers an empty list.
    let mut app = App::new();
    app.open_skills_menu(
        Vec::new(),
        Default::default(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    assert_eq!(page(&app, 78)[2], "  0 skills");
}

#[test]
fn an_on_value_and_an_off_value_are_coloured_apart() {
    // A glance down the value column has to show what is live — the
    // `/settings` menu's two-tone rule, reused wholesale for the words.
    let buf = render(&skills_app(), 78);
    assert_eq!(
        colour_at(&buf, FIRST_ROW, 78, "enabled"),
        settings_value_color()
    );
    assert_eq!(
        colour_at(&buf, FIRST_ROW + 1, 78, "disabled"),
        settings_value_off_color()
    );
}

#[test]
fn every_value_leads_with_its_state_mark() {
    // The `/login` rows' marks: the check is what the eye hunts for down a
    // column of names, so it alone wears the green — on the selected row too
    // — while the circle stays dim. Both sit in one aligned column.
    let buf = render(&skills_app(), 78);
    let on = row(&buf, FIRST_ROW, 78);
    let off = row(&buf, FIRST_ROW + 1, 78);
    assert!(
        on.trim_end()
            .ends_with(&format!("{SKILLS_ON_MARK} enabled")),
        "{on:?}"
    );
    assert!(
        off.trim_end()
            .ends_with(&format!("{SKILLS_OFF_MARK} disabled")),
        "{off:?}"
    );
    assert_eq!(
        colour_at(&buf, FIRST_ROW, 78, SKILLS_ON_MARK),
        model_active_color()
    );
    assert_eq!(
        colour_at(&buf, FIRST_ROW + 1, 78, SKILLS_OFF_MARK),
        model_meta_color()
    );
    assert_eq!(
        col_of(&buf, FIRST_ROW, 78, SKILLS_ON_MARK),
        col_of(&buf, FIRST_ROW + 1, 78, SKILLS_OFF_MARK),
        "the marks line up down the list"
    );
}

#[test]
fn the_selected_row_lights_up_like_every_sibling_picker() {
    let buf = render(&skills_app(), 78);
    assert_eq!(colour_at(&buf, FIRST_ROW, 78, "→"), model_selected_color());
}

#[test]
fn the_height_is_the_chrome_plus_the_listed_rows() {
    let mut app = skills_app();
    let full = skills_menu_height(&app, 78, 200).expect("open");
    // Narrowing the search shrinks the list — and so the region.
    for c in "dataviz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let narrowed = skills_menu_height(&app, 78, 200).expect("open");
    assert_eq!(narrowed + 1, full, "one row fewer");
    // …and it is clamped to the terminal.
    assert_eq!(skills_menu_height(&app, 78, 5), Some(5));
    // Closed, there is no menu height at all.
    app.close_skills_menu();
    assert_eq!(skills_menu_height(&app, 78, 200), None);
}

#[test]
fn a_search_matching_nothing_shows_its_placeholder() {
    let mut app = skills_app();
    for c in "zzzz".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let buf = render(&app, 78);
    let text = (0..buf.area.height)
        .map(|y| row(&buf, y, 78))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(SKILLS_NO_MATCH), "{text}");
}

#[test]
fn an_empty_list_names_where_a_skill_would_go() {
    // "Why is my skill not here?" is the only question an empty list raises,
    // so the placeholder answers it instead of just saying "none".
    let mut app = App::new();
    app.open_skills_menu(
        Vec::new(),
        Default::default(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    let buf = render(&app, 78);
    let text = (0..buf.area.height)
        .map(|y| row(&buf, y, 78))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(SKILLS_NONE_FOUND), "{text}");
    assert!(
        text.contains("~/.claude/skills/<name>/SKILL.md"),
        "the root is spelled out: {text}"
    );
}

#[test]
fn the_session_off_note_shows_only_when_it_applies() {
    // On: no note — and no row spent reserving one.
    let on = render(&skills_app(), 78);
    let on_text = (0..on.area.height)
        .map(|y| row(&on, y, 78))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!on_text.contains(SKILLS_SESSION_OFF), "{on_text}");

    // Off over the SAME skills: the note appears and costs exactly its own
    // row (it used to be reserved blank, stacking an empty line on the gap).
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").session_enabled = false;
    let off = render(&app, 78);
    let off_text = (0..off.area.height)
        .map(|y| row(&off, y, 78))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(off_text.contains(SKILLS_SESSION_OFF), "{off_text}");
    assert_eq!(
        off.area.height,
        on.area.height + 1,
        "the note costs exactly one row, and only when shown"
    );
}

#[test]
fn the_menu_reserves_the_running_turn_strip_above_it() {
    // `/skills` replaces the composer only — the streaming strip keeps its
    // rows, so opening it mid-turn never hides what is executing.
    let mut app = skills_app();
    let idle = skills_menu_height(&app, 78, 200).expect("open");
    app.begin_stream();
    app.start_tool("Bash", "cargo test", None);
    let preview = preview_rows(&app, 78);
    assert!(preview > 0, "the running tool previews mid-turn");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    assert_eq!(skills_menu_height(&app, 78, 200), Some(idle + strip));
}

// --- the no-match page collapses its empty slots (docs/skills.md) ---

/// The menu's page as trimmed plain rows.
fn page(app: &App, width: u16) -> Vec<String> {
    crate::ui::skills_view::skills_view_lines(app, width)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    // No skill matched: no count, no description — one blank gap carries the
    // placeholder to the hint (the `/model` picker's placeholder rule). The
    // header stays: it describes the catalog, which a search doesn't change.
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").query = "zzz".to_string();
    let texts = page(&app, 78);
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 10, "the collapsed page is 10 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], format!("  {SKILLS_TITLE}"), "{texts:?}");
    assert_eq!(texts[2], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains('❯'), "{texts:?}");
    assert_eq!(texts[5], "", "{texts:?}");
    assert!(texts[6].contains(SKILLS_NO_MATCH), "{texts:?}");
    assert_eq!(texts[7], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[8].contains("Type to search"), "{texts:?}");
    assert!(is_rule(&texts[9]), "{texts:?}");
}

#[test]
fn the_session_off_note_joins_the_header_only_when_it_applies() {
    // On: the count line, one gap, the search line — no row spent on a note.
    let on = skills_app();
    let texts = page(&on, 78);
    assert_eq!(texts[2], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[3], "", "one gap under the header: {texts:?}");
    assert!(texts[4].contains('❯'), "{texts:?}");

    // Off, the note sits right under the count: it says what state the
    // catalog is in, so it belongs to the header — not wedged between the
    // search line and the rows it filters.
    let mut off = App::new();
    off.open_skills_menu(
        vec![meta(
            "commit-helper",
            "Write a commit message in house style",
        )],
        Default::default(),
        false,
        vec!["~/.claude/skills".to_string()],
    );
    let texts = page(&off, 78);
    assert_eq!(texts[2], "  1 skill · 1 enabled", "{texts:?}");
    assert!(texts[3].contains(SKILLS_SESSION_OFF), "{texts:?}");
    assert_eq!(texts[4], "", "{texts:?}");
    assert!(texts[5].contains('❯'), "{texts:?}");
}

#[test]
fn an_empty_catalog_shows_no_session_off_note() {
    // With nothing installed the `/settings` Skills row is unavailable, so the
    // session reads as off — but the note exists to explain why LISTED rows
    // do nothing, and "turn them on in /settings" is advice that row refuses.
    // An empty list's one question is answered by the placeholder instead.
    let mut app = App::new();
    app.open_skills_menu(
        Vec::new(),
        Default::default(),
        false,
        vec!["~/.claude/skills".to_string()],
    );
    let texts = page(&app, 78);
    assert!(
        !texts.iter().any(|t| t.contains(SKILLS_SESSION_OFF)),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(SKILLS_NONE_FOUND)),
        "{texts:?}"
    );
}

#[test]
fn every_skills_page_hugs_its_frame() {
    // The title straight under the opening rule, the key hint straight over
    // the closing one — with the session on or off, whatever the search
    // shows, and over an empty catalog's where-to-put-one answer.
    let mut app = skills_app();
    for session_enabled in [true, false] {
        app.skills_menu.as_mut().expect("open").session_enabled = session_enabled;
        for query in ["", "commit", "zzz"] {
            app.skills_menu.as_mut().expect("open").query = query.to_string();
            assert_frame_hugs_content(
                &format!("the menu (session {session_enabled}) searching {query:?}"),
                &crate::ui::skills_view::skills_view_lines(&app, 78),
            );
        }
    }
    let mut empty = App::new();
    empty.open_skills_menu(
        Vec::new(),
        Default::default(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    assert_frame_hugs_content(
        "an empty catalog",
        &crate::ui::skills_view::skills_view_lines(&empty, 78),
    );
}

#[test]
fn no_skills_page_ever_stacks_two_blank_rows() {
    let mut app = skills_app();
    for session_enabled in [true, false] {
        app.skills_menu.as_mut().expect("open").session_enabled = session_enabled;
        for query in ["", "commit", "zzz"] {
            app.skills_menu.as_mut().expect("open").query = query.to_string();
            let texts = page(&app, 78);
            for pair in texts.windows(2) {
                assert!(
                    !(pair[0].is_empty() && pair[1].is_empty()),
                    "query {query:?} stacked two blank rows: {texts:?}"
                );
            }
        }
    }
}

#[test]
fn the_highlighted_skill_description_wraps_instead_of_clipping() {
    // The picker doubles as the browser answering "what is this skill for?"
    // — a SKILL.md description routinely runs past 100 columns, so it wraps
    // whole under the counter instead of silently ending at the width.
    let mut app = App::new();
    app.open_skills_menu(
        vec![meta(
            "dataviz",
            "Generate or edit charts, graphs and dashboards for websites, \
             games and applications with a validated accessible palette",
        )],
        Default::default(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 40,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        all.contains("validated accessible palette"),
        "the description's tail survives: {all:?}"
    );
}

#[test]
fn the_empty_state_skill_paths_wrap_instead_of_clipping() {
    // The `{root}/<name>/SKILL.md` row IS the instruction — a long root must
    // not eat the `/SKILL.md` tail.
    let mut app = App::new();
    app.open_skills_menu(
        vec![],
        Default::default(),
        true,
        vec!["~/.config/some/deeply/nested/alter-zero/project/skills".to_string()],
    );
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40);
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("")
        .replace(' ', "");
    assert!(
        all.contains("SKILL.md"),
        "the path's tail survives: {lines:#?}"
    );
}

#[test]
fn the_session_off_note_wraps_instead_of_clipping() {
    // The note explains why every toggle does nothing; its actionable tail
    // ("/settings") must survive a narrow terminal.
    let mut app = App::new();
    app.open_skills_menu(vec![meta("s", "d")], Default::default(), false, vec![]);
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40);
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(all.contains("/settings"), "the fix survives: {all:?}");
}

#[test]
fn a_long_skill_name_marks_its_cut_and_keeps_the_value_seated() {
    // An over-wide (user-controlled) name used to paint-clip at the buffer
    // edge and shove the enabled/disabled value off the row entirely.
    let mut app = App::new();
    app.open_skills_menu(
        vec![meta("an-extremely-long-skill-directory-name-indeed", "d")],
        Default::default(),
        true,
        vec![],
    );
    let lines = crate::ui::skills_view::skills_view_lines(&app, 36);
    let row = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("an-extremely"))
        .expect("the skill row");
    assert!(
        crate::ui::wrap::cols(row.trim_end()) <= 36,
        "the row fits: {row:?}"
    );
    assert!(
        row.contains('…')
            && row
                .trim_end()
                .ends_with(&format!("{SKILLS_ON_MARK} enabled")),
        "the name marks its cut and the value keeps its seat: {row:?}"
    );
}

#[test]
fn a_narrow_row_cuts_the_word_but_keeps_the_mark() {
    // The `/mcp` rows' rule: when the value can't fit whole, the word `…`-cuts
    // and the glyph survives — the mark IS the state, the word restates it.
    let lines = crate::ui::skills_view::skills_view_lines(&skills_app(), 12);
    let row = lines
        .iter()
        .map(plain)
        .find(|l| l.starts_with('→'))
        .expect("the selected row");
    assert!(
        crate::ui::wrap::cols(row.trim_end()) <= 12,
        "the row fits: {row:?}"
    );
    assert!(
        row.contains(SKILLS_ON_MARK) && row.trim_end().ends_with('…'),
        "the mark survives and the word marks its cut: {row:?}"
    );
}

// --- the cursor seat (docs/view-flow.md) ---

#[test]
fn the_cursor_sits_at_the_end_of_the_search_query() {
    let mut app = skills_app();
    for c in "data".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let height = skills_menu_height(&app, 78, 200).expect("open");
    let area = Rect::new(0, 3, 78, height);
    let (x, y) = cursor_position(area, &app);
    assert_eq!(y, 3 + SEARCH_ROW, "on the search row, under the header");
    // indent (2) + `❯ ` (2) + the 4-character query.
    assert_eq!(x, 2 + 2 + 4);
}

#[test]
fn the_cursor_finds_the_search_line_under_a_wrapped_session_note() {
    // The note joins the header and wraps with the width, so the search
    // line's row is found in the page the paint builds, never counted: at 40
    // columns the note takes two rows and pushes the search line to row 6.
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").session_enabled = false;
    let width = 40;
    let texts = page(&app, width);
    let search = texts
        .iter()
        .position(|t| t.contains('❯'))
        .expect("the search line");
    assert_eq!(search, 6, "below a two-row note: {texts:?}");
    let height = skills_menu_height(&app, width, 200).expect("open");
    let (_, y) = cursor_position(Rect::new(0, 0, width, height), &app);
    assert_eq!(usize::from(y), search, "the caret sits on the drawn line");
}

#[test]
fn the_cursor_seat_follows_the_bottom_anchor() {
    // A page one row taller than its region paints from its second row, so
    // the search line sits one row higher on screen than its page row — and
    // the caret must sit on the painted line (docs/view-flow.md).
    let app = skills_app();
    let page_rows = skills_menu_height(&app, 78, 200).expect("open");
    let (_, y) = cursor_position(Rect::new(0, 0, 78, page_rows - 1), &app);
    assert_eq!(y, SEARCH_ROW - 1, "the seat follows the anchored paint");
}

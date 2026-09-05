//! The inline `/skills` menu's rendering (`docs/skills.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::skills::SkillMetadata;
use crate::ui::theme::{
    GAP_ROWS, SKILLS_HINT, SKILLS_NO_MATCH, SKILLS_NONE_FOUND, SKILLS_SESSION_OFF, STATUS_GAP_ROWS,
    STATUS_ROWS, model_selected_color, settings_value_color, settings_value_off_color,
};

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

/// The cell colour at the first occurrence of `needle` on row `y` — the
/// settings-view tests' idiom.
fn colour_at(buf: &Buffer, y: u16, width: u16, needle: &str) -> ratatui::style::Color {
    let line = row(buf, y, width);
    let at = u16::try_from(
        line.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {line:?}")),
    )
    .unwrap();
    buf[(at, y)].fg
}

#[test]
fn an_on_value_and_an_off_value_are_coloured_apart() {
    // A glance down the value column has to show what is live — the
    // `/settings` menu's two-tone rule, reused wholesale. Rows start at 4:
    // top rule (0), gap (1), search (2), gap (3) — the session-off note
    // takes a row only when it applies.
    let buf = render(&skills_app(), 78);
    assert_eq!(colour_at(&buf, 4, 78, "enabled"), settings_value_color());
    assert_eq!(
        colour_at(&buf, 5, 78, "disabled"),
        settings_value_off_color()
    );
}

#[test]
fn the_selected_row_lights_up_like_every_sibling_picker() {
    let buf = render(&skills_app(), 78);
    assert_eq!(colour_at(&buf, 4, 78, "→"), model_selected_color());
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
    // placeholder to the hint (the `/model` picker's placeholder rule).
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").query = "zzz".to_string();
    let texts = page(&app, 78);
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 9, "the collapsed page is 9 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains(SKILLS_NO_MATCH), "{texts:?}");
    assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[6].contains("Type to search"), "{texts:?}");
    assert_eq!(texts[7], "", "{texts:?}");
    assert!(is_rule(&texts[8]), "{texts:?}");
}

#[test]
fn the_session_off_note_takes_no_row_when_skills_are_on() {
    // The note row is emitted only when there IS a note: with skills on it
    // used to render as a blank line stacked on the gap below it.
    let on = skills_app();
    let texts = page(&on, 78);
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert_eq!(texts[3], "", "one gap under the search line: {texts:?}");
    assert!(!texts[4].is_empty(), "the list starts here: {texts:?}");

    // Off, the note takes its row back — right under the search line.
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
    assert!(texts[3].contains(SKILLS_SESSION_OFF), "{texts:?}");
}

#[test]
fn no_skills_page_ever_stacks_two_blank_rows() {
    let mut app = skills_app();
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
        row.contains('…') && row.trim_end().ends_with("enabled"),
        "the name marks its cut and the value keeps its seat: {row:?}"
    );
}

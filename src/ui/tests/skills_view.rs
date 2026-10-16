//! The inline `/skills` menu's rendering (`docs/skills.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::skills::SkillMetadata;
use crate::ui::theme::{
    GAP_ROWS, SKILLS_HINT, SKILLS_NO_MATCH, SKILLS_NONE_FOUND, SKILLS_OFF_MARK, SKILLS_ON_MARK,
    SKILLS_SESSION_OFF, SKILLS_TITLE, STATUS_GAP_ROWS, STATUS_ROWS, model_active_color,
    model_meta_color, model_selected_color, settings_value_color, settings_value_off_color,
};

/// The page row the first skill sits on: top rule (0), gap (1), title (2),
/// count (3), gap (4), search (5), gap (6) — the session-off note, when it
/// applies, takes rows under the count.
const FIRST_ROW: u16 = 7;

/// The page row the `❯` search line sits on when no note is shown.
const SEARCH_ROW: u16 = 5;

/// A terminal tall enough for any whole page here.
const TALL: u16 = 200;

/// A terminal too short for [`many_app`]'s whole page (22 rows) but tall
/// enough for its compact page with the header kept (14 rows).
const SHORT: u16 = 15;

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
    // The pages' header (`/hooks`, `/mcp`): a gap under the top rule, the
    // title in the pages' one title dress, the catalog's size under it, then
    // a gap and the search line.
    let app = skills_app();
    let lines = crate::ui::skills_view::skills_view_lines(&app, 78, TALL);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[1], "", "a gap under the top rule: {texts:?}");
    assert_eq!(texts[2], format!("  {SKILLS_TITLE}"), "{texts:?}");
    assert!(wears_the_title_dress(&lines[2]), "{:?}", lines[2]);
    assert_eq!(texts[3], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[4], "", "{texts:?}");
    assert!(texts[5].contains('❯'), "the search line follows: {texts:?}");
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
    assert_eq!(row(&buf, 3, 78).trim_end(), "  2 skills · 1 enabled");
    assert_eq!(
        buf[(2, 3)].fg,
        model_meta_color(),
        "dim, like /hooks' count"
    );
}

#[test]
fn the_count_follows_a_toggle_in_the_same_frame() {
    let mut app = skills_app();
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(page(&app, 78)[3], "  2 skills · 0 enabled");
}

#[test]
fn a_lone_skill_is_counted_in_the_singular() {
    let mut app = App::new();
    app.open_skills_menu(vec![meta("pdf", "d")], Default::default(), true, vec![]);
    assert_eq!(page(&app, 78)[3], "  1 skill · 1 enabled");
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
    assert_eq!(page(&app, 78)[3], "  0 skills");
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

/// The menu's page on a tall terminal, as trimmed plain rows.
fn page(app: &App, width: u16) -> Vec<String> {
    page_at(app, width, TALL)
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
    assert_eq!(texts.len(), 12, "the collapsed page is 12 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert_eq!(texts[2], format!("  {SKILLS_TITLE}"), "{texts:?}");
    assert_eq!(texts[3], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[4], "", "{texts:?}");
    assert!(texts[5].contains('❯'), "{texts:?}");
    assert_eq!(texts[6], "", "{texts:?}");
    assert!(texts[7].contains(SKILLS_NO_MATCH), "{texts:?}");
    assert_eq!(texts[8], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[9].contains("Type to search"), "{texts:?}");
    assert_eq!(texts[10], "", "{texts:?}");
    assert!(is_rule(&texts[11]), "{texts:?}");
}

#[test]
fn the_session_off_note_joins_the_header_only_when_it_applies() {
    // On: the count line, one gap, the search line — no row spent on a note.
    let on = skills_app();
    let texts = page(&on, 78);
    assert_eq!(texts[3], "  2 skills · 1 enabled", "{texts:?}");
    assert_eq!(texts[4], "", "one gap under the header: {texts:?}");
    assert!(texts[5].contains('❯'), "{texts:?}");

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
    assert_eq!(texts[3], "  1 skill · 1 enabled", "{texts:?}");
    assert!(texts[4].contains(SKILLS_SESSION_OFF), "{texts:?}");
    assert_eq!(texts[5], "", "{texts:?}");
    assert!(texts[6].contains('❯'), "{texts:?}");
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
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40, TALL);
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
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40, TALL);
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
    let lines = crate::ui::skills_view::skills_view_lines(&app, 40, TALL);
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
    let lines = crate::ui::skills_view::skills_view_lines(&app, 36, TALL);
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
    let lines = crate::ui::skills_view::skills_view_lines(&skills_app(), 12, TALL);
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
    // columns the note takes two rows and pushes the search line to row 7.
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").session_enabled = false;
    let width = 40;
    let texts = page(&app, width);
    let search = texts
        .iter()
        .position(|t| t.contains('❯'))
        .expect("the search line");
    assert_eq!(search, 7, "below a two-row note: {texts:?}");
    let height = skills_menu_height(&app, width, 200).expect("open");
    let (_, y) = cursor_position(Rect::new(0, 0, width, height), &app);
    assert_eq!(usize::from(y), search, "the caret sits on the drawn line");
}

// --- the compact page: a short terminal keeps the choices on screen ---

/// An app with the menu open over eight skills, `pdf` turned off — enough
/// that a three-row window hides rows on both sides of a middle selection.
/// The first name is the longest, so a value column sized off only the rows
/// in view would move as the window scrolls.
fn many_app() -> App {
    let names = [
        "commit-message-helper",
        "dataviz",
        "haiku-writer",
        "jina-reader",
        "pdf",
        "skill-creator",
        "web-search",
        "yt-dlp",
    ];
    let mut app = App::new();
    app.open_skills_menu(
        names
            .iter()
            .map(|name| meta(name, &format!("What {name} does")))
            .collect(),
        ["pdf".to_string()].into_iter().collect(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    app
}

/// The menu's page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    crate::ui::skills_view::skills_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// Highlight row `i` of the filtered list.
fn select(app: &mut App, i: usize) {
    app.skills_menu.as_mut().expect("open").selected = i;
}

/// How many rows of `texts` are skill rows (they carry a state mark).
fn skill_rows(texts: &[String]) -> usize {
    texts.iter().filter(|t| mark_col(t).is_some()).count()
}

/// The display column of the row's `✔`/`◯` state mark, if it has one.
fn mark_col(text: &str) -> Option<usize> {
    text.find(SKILLS_ON_MARK)
        .or_else(|| text.find(SKILLS_OFF_MARK))
        .map(|i| crate::ui::wrap::cols(&text[..i]))
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_skill_choices_on_screen() {
    // The whole page is 22 rows here. On 15, bottom-anchoring it used to
    // flow the search line and the highlighted row into scrollback, leaving
    // the description and the rows below the selection — nothing showed
    // what Enter would toggle. codex's short-panel rule instead: the
    // description yields first, so the search line, three skill rows and
    // the hint stay on screen.
    let app = many_app();
    assert_eq!(page_at(&app, 78, TALL).len(), 22, "the whole page");
    let texts = page_at(&app, 78, SHORT);
    assert!(texts.len() <= usize::from(SHORT), "fits: {texts:#?}");
    assert!(
        texts.iter().any(|t| t == "  ❯"),
        "the search line: {texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("→ commit-message-helper")),
        "the highlighted skill: {texts:#?}"
    );
    assert_eq!(skill_rows(&texts), 3, "three skill rows: {texts:#?}");
    assert!(
        texts.iter().any(|t| t == "  ↓ 5 more below"),
        "the rest named under them: {texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains(SKILLS_HINT)),
        "the hint: {texts:#?}"
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("What commit-message-helper does")),
        "no description: {texts:#?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("(1/8)")),
        "no counter — the overflow slots say what is hidden: {texts:#?}"
    );
}

#[test]
fn the_compact_page_keeps_the_header_while_it_fits() {
    // codex keeps a picker's header for as long as it fits: rule, gap, the
    // title over the count, gap, the search line with the `↑` slot directly
    // under it, three rows, the `↓` slot directly over the hint, gap, rule.
    // Both slots are always there — blank when nothing is hidden that way —
    // so the page is one height at every selection, and a blank slot is the
    // gap the page wants there anyway.
    let texts = page_at(&many_app(), 78, SHORT);
    assert_eq!(texts.len(), 14, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "", "{texts:#?}");
    assert_eq!(texts[2], format!("  {SKILLS_TITLE}"), "{texts:#?}");
    assert_eq!(texts[3], "  8 skills · 7 enabled", "{texts:#?}");
    assert_eq!(texts[4], "", "{texts:#?}");
    assert_eq!(texts[5], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[6], "", "nothing above the first row: {texts:#?}");
    assert!(
        texts[7].starts_with("→ commit-message-helper"),
        "{texts:#?}"
    );
    assert!(texts[8].starts_with("  dataviz"), "{texts:#?}");
    assert!(texts[9].starts_with("  haiku-writer"), "{texts:#?}");
    assert_eq!(texts[10], "  ↓ 5 more below", "{texts:#?}");
    assert_eq!(texts[11], format!("  {SKILLS_HINT}"), "{texts:#?}");
    assert_eq!(texts[12], "", "{texts:#?}");
    assert!(is_rule(&texts[13]), "{texts:#?}");
}

#[test]
fn a_terminal_too_short_for_the_header_drops_it_too() {
    // The header yields after the description: rule, gap, the search line,
    // the slots around three rows, the hint, gap, rule — the /theme
    // picker's compact page, row for row.
    let texts = page_at(&many_app(), 78, 13);
    assert_eq!(texts.len(), 11, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "", "{texts:#?}");
    assert_eq!(texts[2], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[3], "", "{texts:#?}");
    assert!(
        texts[4].starts_with("→ commit-message-helper"),
        "{texts:#?}"
    );
    assert!(texts[5].starts_with("  dataviz"), "{texts:#?}");
    assert!(texts[6].starts_with("  haiku-writer"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 5 more below", "{texts:#?}");
    assert_eq!(texts[8], format!("  {SKILLS_HINT}"), "{texts:#?}");
    assert_eq!(texts[9], "", "{texts:#?}");
    assert!(is_rule(&texts[10]), "{texts:#?}");
    assert!(
        !texts.iter().any(|t| t.contains(SKILLS_TITLE)),
        "no header: {texts:#?}"
    );
}

#[test]
fn the_compact_window_keeps_the_selection_in_its_middle() {
    let mut app = many_app();
    select(&mut app, 3); // jina-reader: one row either side, the rest named
    let middle = page_at(&app, 78, SHORT);
    assert_eq!(middle[6], "  ↑ 2 more above", "{middle:#?}");
    assert!(middle[7].starts_with("  haiku-writer"), "{middle:#?}");
    assert!(middle[8].starts_with("→ jina-reader"), "{middle:#?}");
    assert!(middle[9].starts_with("  pdf"), "{middle:#?}");
    assert_eq!(middle[10], "  ↓ 3 more below", "{middle:#?}");
    select(&mut app, 7); // yt-dlp: the window pinned to the end
    let end = page_at(&app, 78, SHORT);
    assert_eq!(end[6], "  ↑ 5 more above", "{end:#?}");
    assert!(end[7].starts_with("  skill-creator"), "{end:#?}");
    assert!(end[8].starts_with("  web-search"), "{end:#?}");
    assert!(end[9].starts_with("→ yt-dlp"), "{end:#?}");
    assert_eq!(end[10], "", "nothing below the last row: {end:#?}");
}

#[test]
fn the_compact_page_holds_still_across_selections() {
    // Neither the frame nor the value column moves as ↑/↓ walk the window:
    // the page is one height at every selection, and the value column is
    // sized off every listed row rather than the three in view.
    for height in [SHORT, 13] {
        let mut app = many_app();
        let h0 = page_at(&app, 78, height).len();
        let col0 = page_at(&app, 78, height)
            .iter()
            .find_map(|t| mark_col(t))
            .expect("a skill row");
        for i in 0..8 {
            select(&mut app, i);
            let texts = page_at(&app, 78, height);
            assert_eq!(
                texts.len(),
                h0,
                "selection {i} moved the frame at height {height}"
            );
            for text in texts.iter().filter(|t| mark_col(t).is_some()) {
                assert_eq!(
                    mark_col(text),
                    Some(col0),
                    "selection {i} moved the value column: {texts:#?}"
                );
            }
        }
    }
}

#[test]
fn the_whole_page_returns_once_it_fits() {
    // The compact page is only for a terminal too short for the whole one:
    // at the whole page's own height it is the whole page, one row shorter
    // the compact one.
    let app = many_app();
    let has_description = |texts: &[String]| {
        texts
            .iter()
            .any(|t| t.contains("What commit-message-helper does"))
    };
    let whole = u16::try_from(page_at(&app, 78, TALL).len()).expect("rows");
    assert!(
        has_description(&page_at(&app, 78, whole)),
        "at its own height"
    );
    assert!(
        !has_description(&page_at(&app, 78, whole - 1)),
        "compact one row short of it"
    );
}

#[test]
fn the_session_off_note_yields_with_the_header() {
    // The note is part of the header — it says what state the whole catalog
    // is in — so the compact page keeps it while the header fits and drops
    // it with the header.
    let mut app = many_app();
    app.skills_menu.as_mut().expect("open").session_enabled = false;
    let kept = page_at(&app, 78, SHORT);
    assert_eq!(kept.len(), 15, "the note costs its row: {kept:#?}");
    assert!(kept[4].contains(SKILLS_SESSION_OFF), "{kept:#?}");
    let dropped = page_at(&app, 78, SHORT - 1);
    assert_eq!(dropped.len(), 11, "{dropped:#?}");
    assert!(
        !dropped.iter().any(|t| t.contains(SKILLS_SESSION_OFF)),
        "{dropped:#?}"
    );
}

#[test]
fn a_page_with_nothing_to_choose_drops_only_its_header() {
    // An unmatched search and an empty catalog list nothing, so there is no
    // window to compact; a terminal too short for the page still drops the
    // header, keeping the search line and the placeholder on screen.
    let mut app = many_app();
    app.skills_menu.as_mut().expect("open").query = "zzz".to_string();
    assert_eq!(page_at(&app, 78, TALL).len(), 12, "the whole no-match page");
    let texts = page_at(&app, 78, 11);
    assert_eq!(texts.len(), 9, "{texts:#?}");
    assert_eq!(texts[2], "  ❯ zzz", "{texts:#?}");
    assert_eq!(texts[3], "", "{texts:#?}");
    assert!(texts[4].contains(SKILLS_NO_MATCH), "{texts:#?}");
    assert_eq!(texts[5], "", "{texts:#?}");
    assert!(texts[6].contains(SKILLS_HINT), "{texts:#?}");
}

#[test]
fn a_compact_page_fits_and_never_overflows_the_width_or_stacks_blanks() {
    let mut app = many_app();
    for session_enabled in [true, false] {
        app.skills_menu.as_mut().expect("open").session_enabled = session_enabled;
        for width in [24u16, 40, 78] {
            for query in ["", "er", "pdf", "zzz"] {
                app.skills_menu.as_mut().expect("open").query = query.to_string();
                let len = app.skill_menu_rows().len().max(1);
                for i in 0..len {
                    select(&mut app, i);
                    for height in [11u16, 13, SHORT] {
                        let texts = page_at(&app, width, height);
                        let context = format!(
                            "width {width} height {height} query {query:?} selection {i} \
                             session {session_enabled}"
                        );
                        assert!(
                            texts.len() <= usize::from(height),
                            "{context} overflows the height: {texts:#?}"
                        );
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
}

#[test]
fn the_cursor_sits_on_the_compact_pages_search_line() {
    let mut app = many_app();
    for c in "er".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Three skills match `er`: too many for the whole page on 15 rows.
    for (term, search_row) in [(SHORT, 5u16), (12, 2)] {
        let height = skills_menu_height(&app, 78, term).expect("open");
        assert!(height <= term, "the compact page fits the terminal");
        let area = Rect::new(0, term - height, 78, height);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, area.y + search_row, "on the search line at {term} rows");
        // indent (2) + `❯ ` (2) + the two-character query.
        assert_eq!(x, 2 + 2 + 2);
    }
}

#[test]
fn the_menu_never_flows() {
    // The compact page exists so a short terminal keeps the page on screen,
    // and the rungs under it shed down to the search line alone: whatever
    // the height, nothing flows into scrollback above it.
    let app = many_app();
    for term in (1..=12).chain([SHORT, TALL]) {
        assert!(
            crate::ui::view_flow(&app, 78, term, 1000).is_none(),
            "a page fits {term} rows"
        );
    }
}

#[test]
fn the_menu_renders_the_page_its_height_reserves() {
    // On a 12-row terminal the menu reserves the compact page's rows, and the
    // paint fills exactly them: the search line, the highlighted skill, the
    // hint and the closing rule all on screen.
    let app = many_app();
    let height = skills_menu_height(&app, 78, 12).expect("open");
    assert_eq!(height, 11, "the compact page without its header");
    let mut buf = buffer(78, height);
    render_skills_menu(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..height).map(|y| row(&buf, y, 78)).collect();
    assert!(
        is_rule(rows.last().expect("rows").trim()),
        "the closing rule on the last row: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.trim() == "❯"),
        "the search line: {rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.starts_with("→ commit-message-helper")),
        "the highlighted skill: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains(SKILLS_HINT)),
        "the hint: {rows:#?}"
    );
}

#[test]
fn a_long_description_is_cut_to_one_row_before_the_list_shrinks() {
    // Descriptions wrap to different heights, so the whole page's height
    // moves with the selection; deciding whole-versus-compact on it alone
    // flipped the page between the two as ↑/↓ crossed a long description.
    // A terminal one row short of the wrapped description gets the whole
    // list with the description cut to one row instead, which is one height
    // at every selection.
    let mut app = many_app();
    let long = "Write a commit message in the house style, with a short subject and a body \
                that explains what changed and why";
    app.skills_menu.as_mut().expect("open").skills[0].description = long.to_string();
    let wrapped = page_at(&app, 78, TALL);
    assert_eq!(
        wrapped.len(),
        23,
        "the description wraps to two rows: {wrapped:#?}"
    );
    let texts = page_at(&app, 78, 22);
    assert_eq!(texts.len(), 22, "{texts:#?}");
    assert_eq!(skill_rows(&texts), 8, "the whole list stays: {texts:#?}");
    assert!(texts.iter().any(|t| t.contains("(1/8)")), "{texts:#?}");
    let description = texts
        .iter()
        .find(|t| t.contains("Write a commit message"))
        .expect("the description");
    assert!(
        description.ends_with('…'),
        "cut to one row: {description:?}"
    );
    // A skill whose description fits one row looks the same either way.
    select(&mut app, 1);
    assert_eq!(page_at(&app, 78, 22), page_at(&app, 78, TALL));
}

// --- a terminal shorter than the compact page keeps the search line ---

fn skills_page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    crate::ui::skills_view::skills_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn a_tiny_terminal_keeps_the_skills_search_line() {
    let app = skills_app();
    assert_sheds_to_its_search_line(
        |height| skills_page_at(&app, 78, height),
        "",
        "→ commit-helper",
        "Enter/Space to enable/disable",
    );
    for height in 1..=10 {
        assert_caret_on_search_line(&app, &skills_page_at(&app, 78, height), 78, "");
    }
    let mut app = skills_app();
    app.skills_menu.as_mut().expect("open").query = "zzz".to_string();
    assert_sheds_to_its_search_line(
        |height| skills_page_at(&app, 78, height),
        "zzz",
        "No matching skills",
        "Enter/Space to enable/disable",
    );
}

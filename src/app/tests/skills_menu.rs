//! The inline `/skills` menu's state and key map (`docs/skills.md`).

use super::*;

use crate::skills::SkillMetadata;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn meta(name: &str, description: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: description.to_string(),
        dir: PathBuf::from("/skills").join(name),
        path: PathBuf::from("/skills").join(name).join("SKILL.md"),
    }
}

/// An app with the menu open over three skills, none turned off.
fn skills_app() -> App {
    let mut app = App::new();
    app.open_skills_menu(
        vec![
            meta("commit", "Write a commit message"),
            meta("dataviz", "Charts and dashboards"),
            meta("pdf", "Extract text from PDFs"),
        ],
        BTreeSet::new(),
        true,
        vec!["~/.claude/skills".to_string()],
    );
    app
}

fn space() -> KeyEvent {
    key(KeyCode::Char(' '))
}

#[test]
fn the_menu_lists_every_skill_with_its_state() {
    let app = skills_app();
    let rows = app.skill_menu_rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].name, "commit");
    assert_eq!(rows[0].description, "Write a commit message");
    assert!(rows.iter().all(|row| row.enabled), "all on by default");
}

#[test]
fn a_disabled_skill_still_lists_showing_its_state() {
    // The whole point of the menu: a skill you turned off is exactly the one
    // you need to see to turn back on.
    let mut app = App::new();
    app.open_skills_menu(
        vec![meta("commit", "d"), meta("pdf", "d")],
        ["pdf".to_string()].into_iter().collect(),
        true,
        Vec::new(),
    );
    let rows = app.skill_menu_rows();
    assert_eq!(rows.len(), 2, "both listed");
    assert!(rows[0].enabled);
    assert!(!rows[1].enabled, "pdf reads as off");
}

#[test]
fn enter_toggles_the_highlighted_skill_and_reports_it() {
    let mut app = skills_app();
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::SkillToggled {
            name: "commit".to_string(),
            enabled: false,
        }
    );
    // The menu's own copy moved, so the row updates in the same frame rather
    // than waiting for the loop to hand the state back.
    assert!(!app.skill_menu_rows()[0].enabled);
    // …and again turns it back on.
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::SkillToggled {
            name: "commit".to_string(),
            enabled: true,
        }
    );
    assert!(app.skill_menu_rows()[0].enabled);
}

#[test]
fn space_toggles_too_and_never_reaches_the_search() {
    // The `/settings` menu's rule: space is the second toggle key, so a plain
    // space can't become a query character.
    let mut app = skills_app();
    assert!(matches!(app.on_key(space()), Action::SkillToggled { .. }));
    assert!(
        app.skills_menu.as_ref().expect("open").query.is_empty(),
        "the space never landed in the query"
    );
}

#[test]
fn the_menu_stays_open_across_toggles() {
    // Several skills get set in one visit — the menu is not a one-shot.
    let mut app = skills_app();
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(app.skills_menu.is_some(), "still open");
    let rows = app.skill_menu_rows();
    assert!(!rows[0].enabled && !rows[1].enabled && rows[2].enabled);
}

#[test]
fn typing_narrows_the_list_by_name_or_description() {
    let mut app = skills_app();
    for c in "chart".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let rows = app.skill_menu_rows();
    assert_eq!(rows.len(), 1, "matched on the description: {rows:?}");
    assert_eq!(rows[0].name, "dataviz");
}

#[test]
fn a_toggle_under_a_search_names_the_row_it_shows() {
    // The action must name the *filtered* row, not the same index in the
    // unfiltered list — the classic off-by-a-filter bug.
    let mut app = skills_app();
    for c in "pdf".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SkillToggled {
            name: "pdf".to_string(),
            enabled: false,
        }
    );
}

#[test]
fn esc_clears_the_query_first_then_closes() {
    let mut app = skills_app();
    app.on_key(key(KeyCode::Char('p')));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(
        app.skills_menu
            .as_ref()
            .expect("still open")
            .query
            .is_empty(),
        "the first Esc only cleared the query"
    );
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseSkillsMenu);
    assert!(app.skills_menu.is_none());
}

#[test]
fn ctrl_c_closes_the_menu_rather_than_quitting() {
    let mut app = skills_app();
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::CloseSkillsMenu
    );
    assert!(app.skills_menu.is_none());
}

#[test]
fn the_selection_clamps_to_the_filtered_rows() {
    let mut app = skills_app();
    app.on_key(key(KeyCode::End));
    assert_eq!(app.skills_menu.as_ref().expect("open").selected, 2);
    // Narrowing to one row must not leave the highlight past the end.
    for c in "commit".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.skills_menu.as_ref().expect("open").selected, 0);
    assert_eq!(app.highlighted_skill().expect("a row").name, "commit");
}

#[test]
fn an_empty_menu_toggles_nothing() {
    // No skills discovered: the menu still opens (it names where one goes),
    // but Enter has nothing to flip and must not panic.
    let mut app = App::new();
    app.open_skills_menu(Vec::new(), BTreeSet::new(), true, vec!["~/x".to_string()]);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.skill_menu_rows().is_empty());
    assert!(app.skills_menu.is_some(), "still open");
}

#[test]
fn opening_the_menu_abandons_the_bands_it_replaces() {
    // It takes the composer, so the palette / `?` band / file picker that were
    // using it must go — the `/settings` menu's rule.
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.shortcuts_open);
    app.open_skills_menu(vec![meta("commit", "d")], BTreeSet::new(), true, Vec::new());
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none() && app.file_search.is_none());
}

#[test]
fn the_session_off_note_rides_the_menu() {
    // The rows still browse and toggle with the /settings switch off; the note
    // is what stops that reading as "my toggles do nothing".
    let mut app = App::new();
    app.open_skills_menu(
        vec![meta("commit", "d")],
        BTreeSet::new(),
        false,
        Vec::new(),
    );
    assert!(!app.skills_menu.as_ref().expect("open").session_enabled);
}

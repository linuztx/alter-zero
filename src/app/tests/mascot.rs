//! The mascot catalog, the `/mascot` picker, and its persistence format
//! (`docs/mascot.md`).

use super::*;

use unicode_width::UnicodeWidthStr;

/// Display columns of `s` (the `ui::wrap::cols` measure, private to `ui`).
fn cols(s: &str) -> usize {
    s.width()
}

/// An app with the `/mascot` picker open.
fn mascot_app() -> App {
    let mut app = App::new();
    app.open_mascot_picker();
    app
}

/// The names of the rows the picker currently lists.
fn names(app: &App) -> Vec<&'static str> {
    app.mascot_rows().iter().map(|r| r.mascot.name()).collect()
}

// ===== the catalog =====

#[test]
fn the_catalog_holds_the_six_mascots_crest_first() {
    let names: Vec<&str> = Mascot::ALL.iter().map(|m| m.name()).collect();
    assert_eq!(names, ["crest", "bloom", "sprout", "twin", "skiter", "gem"]);
    assert_eq!(Mascot::default(), Mascot::Crest, "crest is the default");
}

#[test]
fn every_mascot_has_art_a_name_and_a_description() {
    for mascot in Mascot::ALL {
        assert!(!mascot.name().is_empty());
        assert!(!mascot.description().is_empty());
        let art = mascot.art();
        assert!(
            (3..=5).contains(&art.len()),
            "{}: {} art rows",
            mascot.name(),
            art.len()
        );
        assert!(
            art.iter().any(|row| !row.trim().is_empty()),
            "{}: blank art",
            mascot.name()
        );
    }
}

#[test]
fn mascot_art_uses_only_single_width_glyphs() {
    // A wide (emoji/CJK) glyph would shear every banner row after it
    // (docs/table-streaming.md "Wide glyphs") — the catalog must stay in
    // single-width block characters.
    for mascot in Mascot::ALL {
        for row in mascot.art() {
            assert_eq!(
                cols(row),
                row.chars().count(),
                "{}: wide glyph in {row:?}",
                mascot.name()
            );
        }
    }
}

#[test]
fn art_width_is_the_widest_row() {
    for mascot in Mascot::ALL {
        let widest = mascot.art().iter().map(|row| cols(row)).max().unwrap_or(0);
        assert_eq!(mascot.art_width(), widest, "{}", mascot.name());
        assert!(widest > 0);
    }
}

#[test]
fn from_name_round_trips_case_insensitively() {
    for mascot in Mascot::ALL {
        assert_eq!(Mascot::from_name(mascot.name()), Some(mascot));
        assert_eq!(
            Mascot::from_name(&mascot.name().to_uppercase()),
            Some(mascot)
        );
    }
    assert_eq!(Mascot::from_name("no-such-mascot"), None);
    assert_eq!(Mascot::from_name(""), None);
}

// ===== the persistence format (docs/mascot.md) =====

#[test]
fn the_mascot_file_round_trips() {
    let json = mascot_file_json(Mascot::Skiter);
    assert_eq!(parse_mascot_file(&json), Some(Mascot::Skiter));
}

#[test]
fn a_missing_or_corrupt_mascot_file_reads_as_none() {
    assert_eq!(parse_mascot_file(""), None);
    assert_eq!(parse_mascot_file("not json"), None);
    assert_eq!(parse_mascot_file("{}"), None);
    assert_eq!(parse_mascot_file(r#"{"mascot": "unknown"}"#), None);
}

// ===== the /mascot command =====

#[test]
fn slash_mascot_opens_the_picker() {
    let mut app = App::new();
    type_chars(&mut app, "/mascot");
    assert!(app.command_menu.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::OpenMascotPicker);
    assert!(app.mascot_picker.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /mascot token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_loop_side_open_lands_on_the_active_mascot() {
    let mut app = App::new();
    app.set_mascot(Mascot::Gem);
    app.open_mascot_picker();
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Gem),
        "the picker opens with the current mascot highlighted"
    );
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_mascot_picker();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.mascot_picker.is_some());
}

// ===== the picker's key grammar (the /settings family) =====

#[test]
fn up_and_down_wrap_at_the_ends() {
    // The picker opens seated on the session's mascot (crest, the default),
    // and ↑/↓ wrap at the ends — the shared `wrap_step` grammar every menu
    // follows; the jump keys still clamp.
    let mut app = mascot_app();
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Crest),
        "opens on the active mascot"
    );
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Crest));
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Gem),
        "↑ from the first row wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Crest),
        "↓ from the last row wraps to the first"
    );
    app.on_key(key(KeyCode::End));
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Gem),
        "End clamps at the bottom"
    );
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(
        app.highlighted_mascot(),
        Some(Mascot::Crest),
        "PageUp clamps at the top"
    );
}

#[test]
fn typing_filters_the_rows_and_resets_the_selection() {
    let mut app = mascot_app();
    app.on_key(key(KeyCode::Down));
    type_chars(&mut app, "sk");
    assert_eq!(names(&app), ["skiter"]);
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Skiter));
    // Backspace widens again.
    app.on_key(key(KeyCode::Backspace));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(names(&app).len(), Mascot::ALL.len());
}

#[test]
fn the_search_matches_descriptions_too() {
    let mut app = mascot_app();
    type_chars(&mut app, "seedling");
    assert_eq!(
        names(&app),
        ["sprout"],
        "sprout's description mentions a seedling"
    );
}

#[test]
fn enter_selects_the_highlighted_mascot_and_closes() {
    let mut app = mascot_app();
    app.on_key(key(KeyCode::Down)); // crest (the open seat) → bloom
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::SelectMascot(Mascot::Bloom));
    assert_eq!(app.mascot(), Mascot::Bloom, "the pure state already moved");
    assert!(app.mascot_picker.is_none(), "the picker closed");
}

#[test]
fn space_selects_like_enter() {
    // The /settings family's grammar: Enter and Space both "do the thing",
    // which is why a space never reaches the search query.
    let mut app = mascot_app();
    app.on_key(key(KeyCode::End));
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(action, Action::SelectMascot(Mascot::Gem));
    assert!(app.mascot_picker.is_none());
}

#[test]
fn enter_on_an_empty_filter_is_a_no_op() {
    let mut app = mascot_app();
    type_chars(&mut app, "zzz");
    assert!(names(&app).is_empty());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.mascot_picker.is_some(), "nothing selected, stays open");
}

#[test]
fn esc_clears_the_query_first_then_closes() {
    let mut app = mascot_app();
    type_chars(&mut app, "ge");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let picker = app.mascot_picker.as_ref().expect("still open");
    assert!(picker.query.is_empty(), "Esc cleared the query");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseMascotPicker);
    assert!(app.mascot_picker.is_none());
}

#[test]
fn ctrl_c_closes_without_quitting() {
    let mut app = mascot_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseMascotPicker);
    assert!(app.mascot_picker.is_none());
}

#[test]
fn the_picker_owns_every_key_while_open() {
    // `?` must type into the search, never toggle the shortcuts band; Esc
    // must not quit (the composer rules don't apply).
    let mut app = mascot_app();
    app.on_key(key(KeyCode::Char('?')));
    assert!(!app.shortcuts_open);
    let picker = app.mascot_picker.as_ref().expect("open");
    assert_eq!(picker.query, "?");
}

#[test]
fn selection_survives_a_narrowing_filter() {
    // The selection clamps into the narrowed list rather than pointing past
    // its end (the /settings rule).
    let mut app = mascot_app();
    app.on_key(key(KeyCode::End));
    type_chars(&mut app, "b");
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Bloom));
}

#[test]
fn the_active_row_is_marked() {
    let mut app = App::new();
    app.set_mascot(Mascot::Twin);
    app.open_mascot_picker();
    let rows = app.mascot_rows();
    let active: Vec<&'static str> = rows
        .iter()
        .filter(|r| r.active)
        .map(|r| r.mascot.name())
        .collect();
    assert_eq!(active, ["twin"], "exactly the session's mascot is marked");
}

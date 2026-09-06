//! The mascot catalog, the `/mascot` picker, and its persistence format
//! (`docs/mascot.md`).

use std::collections::BTreeMap;

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

// ===== the persistence format — per working directory (docs/mascot.md,
// docs/per-directory-state.md) =====

#[test]
fn a_pre_directory_mascot_file_is_the_last_choice_and_seeds_every_directory() {
    // The old one-value file is the new file's top level: the last choice
    // made anywhere, which every directory without an entry starts from.
    let file = MascotFile::parse("{\n  \"mascot\": \"skiter\"\n}\n");
    assert_eq!(file.last, Some(Mascot::Skiter));
    assert!(file.projects.is_empty());
    assert_eq!(file.choice_for("/a"), Some(Mascot::Skiter));
    assert_eq!(file.project("/a"), None, "no entry of its own yet");
}

#[test]
fn a_directory_entry_outranks_the_last_choice() {
    let file =
        MascotFile::parse(r#"{"mascot": "skiter", "projects": {"/a": {"mascot": "bloom"}}}"#);
    assert_eq!(file.choice_for("/a"), Some(Mascot::Bloom));
    assert_eq!(file.project("/a"), Some(Mascot::Bloom));
    assert_eq!(file.choice_for("/b"), Some(Mascot::Skiter));
    assert_eq!(file.project("/b"), None);
}

#[test]
fn adopt_pins_the_last_choice_for_a_new_directory_exactly_once() {
    // A directory launched in for the first time takes the last choice and
    // keeps it as its own, so a later choice elsewhere never moves it.
    let mut file = MascotFile::parse(r#"{"mascot": "gem"}"#);
    assert!(file.adopt("/a"), "pinned: the file changed");
    assert_eq!(file.project("/a"), Some(Mascot::Gem));
    assert!(!file.adopt("/a"), "already pinned: nothing to write");
    file.record("/b", Mascot::Twin);
    assert_eq!(file.last, Some(Mascot::Twin));
    assert_eq!(file.choice_for("/a"), Some(Mascot::Gem), "the pin held");
    assert_eq!(
        file.choice_for("/c"),
        Some(Mascot::Twin),
        "a new directory takes the new last"
    );
    // Nothing to pin without a last choice: the default stays implicit.
    let mut empty = MascotFile::default();
    assert!(!empty.adopt("/a"));
    assert!(empty.projects.is_empty());
    assert_eq!(empty.choice_for("/a"), None);
}

#[test]
fn record_sets_the_directory_entry_and_the_last_choice() {
    let mut file = MascotFile::default();
    file.record("/a", Mascot::Sprout);
    assert_eq!(file.project("/a"), Some(Mascot::Sprout));
    assert_eq!(file.last, Some(Mascot::Sprout));
    file.record("/b", Mascot::Gem);
    assert_eq!(
        file.project("/a"),
        Some(Mascot::Sprout),
        "another directory's entry is untouched"
    );
    assert_eq!(file.last, Some(Mascot::Gem));
    file.record("/a", Mascot::Crest);
    assert_eq!(
        file.project("/a"),
        Some(Mascot::Crest),
        "a directory can choose again"
    );
}

#[test]
fn the_mascot_file_round_trips_and_one_with_no_entries_is_the_old_shape() {
    let mut file = MascotFile::default();
    file.record("/home/u/a", Mascot::Bloom);
    file.record("/home/u/b", Mascot::Gem);
    let json = file.to_json();
    assert_eq!(
        json,
        "{\n  \"mascot\": \"gem\",\n  \"projects\": {\n    \"/home/u/a\": {\n      \"mascot\": \"bloom\"\n    },\n    \"/home/u/b\": {\n      \"mascot\": \"gem\"\n    }\n  }\n}\n"
    );
    assert_eq!(MascotFile::parse(&json), file);
    // A file with no entries is byte-for-byte the one-value file it used to be.
    let old = MascotFile {
        last: Some(Mascot::Sprout),
        projects: BTreeMap::new(),
    };
    assert_eq!(old.to_json(), "{\n  \"mascot\": \"sprout\"\n}\n");
    assert_eq!(MascotFile::default().to_json(), "{}\n");
}

#[test]
fn a_missing_or_corrupt_mascot_file_reads_as_nothing_chosen() {
    // A corrupt preference file must never block startup: it reads as no
    // choice, and the session keeps the default.
    for text in [
        "",
        "not json",
        "{}",
        "[]",
        r#"{"mascot": "unknown"}"#,
        r#"{"mascot": 3}"#,
        r#"{"projects": []}"#,
    ] {
        let file = MascotFile::parse(text);
        assert_eq!(file.last, None, "{text:?}");
        assert!(file.projects.is_empty(), "{text:?}");
    }
    // An unknown name at either level costs only that value, never the file.
    let file = MascotFile::parse(
        r#"{"mascot": "unknown", "projects": {"/a": {"mascot": "bloom"}, "/b": {"mascot": "nope"}, "/c": {}}}"#,
    );
    assert_eq!(file.last, None);
    assert_eq!(file.project("/a"), Some(Mascot::Bloom));
    assert_eq!(file.project("/b"), None);
    assert_eq!(file.project("/c"), None);
    // Names read case-insensitively, as `from_name` does.
    assert_eq!(
        MascotFile::parse(r#"{"mascot": "Sprout"}"#).last,
        Some(Mascot::Sprout)
    );
    // The spinner file's key is not this file's.
    assert_eq!(MascotFile::parse(r#"{"spinner": "comet"}"#).last, None);
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

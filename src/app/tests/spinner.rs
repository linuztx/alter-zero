//! The spinner-style catalog, the `/spinner` picker, and its persistence
//! format (`docs/spinner.md`).

use std::collections::BTreeMap;

use super::*;

/// An app with the `/spinner` picker open.
fn spinner_app() -> App {
    let mut app = App::new();
    app.open_spinner_picker();
    app
}

/// The names of the rows the picker currently lists.
fn names(app: &App) -> Vec<&'static str> {
    app.spinner_rows()
        .iter()
        .map(|r| r.spinner.name())
        .collect()
}

// ===== the catalog =====

#[test]
fn the_catalog_holds_the_nine_styles_in_picker_order() {
    let names: Vec<&str> = Spinner::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        [
            "comet", "gravity", "wave", "sparkle", "dots", "blocks", "pulse", "bars", "line"
        ]
    );
}

#[test]
fn gravity_is_the_default_style() {
    // What a session with no `spinner.json` entry opens with — and so what
    // `ui::status_line` renders (docs/spinner.md). The catalog still *lists*
    // the comet first; the list's order and the default are separate things,
    // and the picker seats its highlight on the active style either way.
    assert_eq!(
        Spinner::default(),
        Spinner::Gravity,
        "the gravity track is the default"
    );
    assert_eq!(App::new().spinner(), Spinner::Gravity, "a fresh session");
}

#[test]
fn every_style_has_a_distinct_lowercase_name_and_a_description() {
    let mut seen = std::collections::HashSet::new();
    for spinner in Spinner::ALL {
        let name = spinner.name();
        assert!(!name.is_empty());
        assert_eq!(name, name.to_lowercase(), "{name}: names are lowercase");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric()),
            "{name}: a name is one bare word (the search and the file read it)"
        );
        assert!(seen.insert(name), "{name}: duplicate name");
        assert!(
            !spinner.description().is_empty(),
            "{name}: missing description"
        );
    }
}

#[test]
fn from_name_round_trips_case_insensitively() {
    for spinner in Spinner::ALL {
        assert_eq!(Spinner::from_name(spinner.name()), Some(spinner));
        assert_eq!(
            Spinner::from_name(&spinner.name().to_uppercase()),
            Some(spinner)
        );
    }
    assert_eq!(Spinner::from_name("no-such-style"), None);
    assert_eq!(Spinner::from_name(""), None);
}

// ===== the persistence format — per working directory (docs/spinner.md,
// docs/per-directory-state.md) =====

#[test]
fn the_spinner_file_round_trips_under_its_own_key() {
    for spinner in Spinner::ALL {
        let file = SpinnerFile {
            last: Some(spinner),
            projects: BTreeMap::new(),
        };
        let json = file.to_json();
        assert_eq!(SpinnerFile::parse(&json), file, "{json}");
    }
    // A file with no entries is byte-for-byte the one-value file it used to be.
    let old = SpinnerFile {
        last: Some(Spinner::Sparkle),
        projects: BTreeMap::new(),
    };
    assert_eq!(old.to_json(), "{\n  \"spinner\": \"sparkle\"\n}\n");
    let mut file = SpinnerFile::default();
    file.record("/a", Spinner::Dots);
    assert_eq!(
        file.to_json(),
        "{\n  \"spinner\": \"dots\",\n  \"projects\": {\n    \"/a\": {\n      \"spinner\": \"dots\"\n    }\n  }\n}\n"
    );
}

#[test]
fn a_spinner_choice_is_the_directorys_own_and_the_last_made_anywhere() {
    // The mascot file's rules over the spinner key (docs/per-directory-state.md):
    // a new directory starts from the last choice and pins it; a choice
    // elsewhere never moves a pinned directory.
    let mut file = SpinnerFile::parse(r#"{"spinner": "wave"}"#);
    assert_eq!(file.choice_for("/a"), Some(Spinner::Wave));
    assert!(file.adopt("/a"));
    file.record("/b", Spinner::Line);
    assert_eq!(file.choice_for("/a"), Some(Spinner::Wave), "pinned");
    assert_eq!(file.choice_for("/b"), Some(Spinner::Line));
    assert_eq!(file.last, Some(Spinner::Line));
    assert_eq!(file.choice_for("/c"), Some(Spinner::Line));
}

#[test]
fn a_missing_or_corrupt_spinner_file_reads_as_nothing_chosen() {
    for text in ["", "not json", "{}", r#"{"spinner": "unknown"}"#] {
        assert_eq!(SpinnerFile::parse(text), SpinnerFile::default(), "{text:?}");
    }
    assert_eq!(
        SpinnerFile::parse(r#"{"mascot": "comet"}"#).last,
        None,
        "the mascot file's key is not this file's"
    );
    assert_eq!(
        SpinnerFile::parse(r#"{"spinner": "comet", "projects": {"/a": {"mascot": "dots"}}}"#)
            .project("/a"),
        None,
        "nor under an entry"
    );
}

// ===== the /spinner command =====

#[test]
fn slash_spinner_opens_the_picker() {
    let mut app = App::new();
    type_chars(&mut app, "/spinner");
    assert!(app.command_menu.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::OpenSpinnerPicker);
    assert!(app.spinner_picker.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /spinner token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_palette_lists_spinner_beside_mascot() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    let mascot = names.iter().position(|n| *n == "mascot").expect("mascot");
    assert_eq!(
        names.get(mascot + 1),
        Some(&"spinner"),
        "the two look-and-feel pickers sit together: {names:?}"
    );
}

#[test]
fn the_open_lands_on_the_active_style() {
    let mut app = App::new();
    app.set_spinner(Spinner::Bars);
    app.open_spinner_picker();
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Bars),
        "the picker opens with the current style highlighted"
    );
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_spinner_picker();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.spinner_picker.is_some());
}

#[test]
fn the_open_picker_asks_for_animation_frames() {
    // The page is live — every row's spinner turns and the preview line
    // ticks — so the picker re-arms the 32 ms chain exactly like an active
    // turn does, and stops asking the moment it closes.
    let mut app = App::new();
    assert!(!app.wants_animation_frames());
    app.open_spinner_picker();
    assert!(app.wants_animation_frames(), "the open picker animates");
    app.close_spinner_picker();
    assert!(!app.wants_animation_frames(), "closed: the chain stops");
}

#[test]
fn the_preview_clock_counts_from_the_open() {
    // The preview's elapsed is the frame clock measured from the moment the
    // picker opened, so the sample line reads `0s` when it appears and counts
    // up while the user browses — like a turn that just began.
    let mut app = App::new();
    app.set_pulse(Duration::from_millis(90_500));
    assert_eq!(
        app.spinner_preview_elapsed(),
        Duration::ZERO,
        "no picker: nothing to preview"
    );
    app.open_spinner_picker();
    assert_eq!(app.spinner_preview_elapsed(), Duration::ZERO);
    app.set_pulse(Duration::from_millis(92_250));
    assert_eq!(
        app.spinner_preview_elapsed(),
        Duration::from_millis(1750),
        "the clock advanced 1.75s since the open"
    );
}

#[test]
fn the_open_picker_hides_the_ctrl_b_hint_like_its_siblings() {
    // The picker owns every key while open, so Ctrl+B does nothing there —
    // and the running cell's `(ctrl+b to run in background)` hint hangs off
    // `command_elapsed`, which every composer-replacing picker blanks.
    let mut app = App::new();
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_spinner_picker();
    assert_eq!(
        app.background_hint_elapsed(),
        None,
        "the picker swallows Ctrl+B"
    );
    app.close_spinner_picker();
    assert!(
        app.background_hint_elapsed().is_some(),
        "closed: the hint comes back"
    );
}

// ===== the picker's key grammar (the /settings family) =====

#[test]
fn up_and_down_wrap_at_the_ends() {
    let mut app = spinner_app();
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Gravity),
        "opens on the active style — the default, the list's second row"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Comet),
        "↑ steps up the list"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Line),
        "↑ from the first row wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Comet),
        "↓ from the last row wraps to the first"
    );
    app.on_key(key(KeyCode::End));
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Line),
        "End clamps at the bottom"
    );
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(
        app.highlighted_spinner(),
        Some(Spinner::Comet),
        "PageUp clamps at the top"
    );
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.highlighted_spinner(), Some(Spinner::Comet), "Home");
}

#[test]
fn typing_filters_the_rows_and_resets_the_selection() {
    let mut app = spinner_app();
    app.on_key(key(KeyCode::Down));
    type_chars(&mut app, "spa");
    assert_eq!(names(&app), ["sparkle"]);
    assert_eq!(app.highlighted_spinner(), Some(Spinner::Sparkle));
    for _ in 0..3 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert_eq!(names(&app).len(), Spinner::ALL.len(), "Backspace widens");
}

#[test]
fn the_search_matches_descriptions_too() {
    let mut app = spinner_app();
    type_chars(&mut app, "braille");
    assert_eq!(
        names(&app),
        ["dots"],
        "the braille spinner is found by its description"
    );
}

#[test]
fn enter_selects_the_highlighted_style_and_closes() {
    let mut app = spinner_app();
    app.on_key(key(KeyCode::Down)); // gravity (the open seat) → wave
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::SelectSpinner(Spinner::Wave));
    assert_eq!(app.spinner(), Spinner::Wave, "the pure state already moved");
    assert!(app.spinner_picker.is_none(), "the picker closed");
}

#[test]
fn space_selects_like_enter() {
    let mut app = spinner_app();
    app.on_key(key(KeyCode::End));
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(action, Action::SelectSpinner(Spinner::Line));
    assert!(app.spinner_picker.is_none());
}

#[test]
fn enter_on_an_empty_filter_is_a_no_op() {
    let mut app = spinner_app();
    type_chars(&mut app, "zzz");
    assert!(names(&app).is_empty());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.spinner_picker.is_some(), "nothing selected, stays open");
    assert_eq!(app.spinner(), Spinner::Gravity, "the style is untouched");
}

#[test]
fn esc_clears_the_query_first_then_closes() {
    let mut app = spinner_app();
    type_chars(&mut app, "or");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let picker = app.spinner_picker.as_ref().expect("still open");
    assert!(picker.query.is_empty(), "Esc cleared the query");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseSpinnerPicker);
    assert!(app.spinner_picker.is_none());
}

#[test]
fn ctrl_c_closes_without_quitting() {
    let mut app = spinner_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseSpinnerPicker);
    assert!(app.spinner_picker.is_none());
}

#[test]
fn the_picker_owns_every_key_while_open() {
    let mut app = spinner_app();
    app.on_key(key(KeyCode::Char('?')));
    assert!(!app.shortcuts_open, "`?` types, never opens the band");
    let picker = app.spinner_picker.as_ref().expect("open");
    assert_eq!(picker.query, "?");
}

#[test]
fn typing_reseats_the_highlight_on_the_first_match() {
    // The /settings rule: a keystroke into the search resets the highlight
    // to the narrowed list's first row rather than leaving it pointing past
    // the end of what is now listed.
    let mut app = spinner_app();
    app.on_key(key(KeyCode::End));
    type_chars(&mut app, "bar");
    assert_eq!(names(&app), ["bars"]);
    assert_eq!(app.highlighted_spinner(), Some(Spinner::Bars));
}

#[test]
fn the_active_row_is_marked() {
    let mut app = App::new();
    app.set_spinner(Spinner::Wave);
    app.open_spinner_picker();
    let active: Vec<&'static str> = app
        .spinner_rows()
        .iter()
        .filter(|r| r.active)
        .map(|r| r.spinner.name())
        .collect();
    assert_eq!(active, ["wave"], "exactly the session's style is marked");
}

#[test]
fn the_picker_works_mid_turn_without_touching_the_turn() {
    // Like /mascot and /settings it only replaces the composer: opening,
    // browsing and choosing leave the streaming turn exactly as it was.
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_stream();
    app.push_chunk("streaming");
    type_chars(&mut app, "/spinner");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSpinnerPicker);
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SelectSpinner(Spinner::Wave)
    );
    assert!(app.is_streaming(), "the turn is untouched");
    assert!(app.turn_active());
}

//! The `@` file picker (`docs/file-search.md`).

use super::*;

// ===== `@` file picker (docs/file-search.md) =====

#[test]
fn typing_at_opens_the_file_picker() {
    let mut app = App::new();
    type_str(&mut app, "see @al");
    assert!(app.file_search.is_some());
    assert_eq!(app.file_search_query().as_deref(), Some("al"));
}

#[test]
fn the_file_picker_does_not_open_for_an_email() {
    let mut app = App::new();
    type_str(&mut app, "mail@host");
    assert!(app.file_search.is_none());
    assert_eq!(app.file_search_query(), None);
}

#[test]
fn esc_dismisses_the_file_picker_and_stays_dismissed_in_the_token() {
    let mut app = App::new();
    type_str(&mut app, "@a");
    assert!(app.file_search.is_some());
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.file_search.is_none());
    // Editing within the same token does not reopen it (sticky, like the palette).
    type_str(&mut app, "b");
    assert!(app.file_search.is_none());
}

#[test]
fn stale_file_matches_are_dropped() {
    let mut app = App::new();
    type_str(&mut app, "@ab");
    app.set_file_matches("a", vec![fm("axe")]); // results for the OLD query
    assert!(app.file_search.as_ref().unwrap().matches.is_empty());
    app.set_file_matches("ab", vec![fm("abc")]); // results for the live query
    assert_eq!(app.file_search.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn a_shrinking_result_refresh_clamps_the_file_selection() {
    // An async refresh can come back with fewer matches than the row the
    // highlight sits on — the highlight must be pulled back in bounds or
    // Tab/Enter accept nothing (and the render highlights no row).
    let mut app = App::new();
    type_str(&mut app, "@x");
    app.set_file_matches("x", vec![fm("x1"), fm("x2"), fm("x3")]);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down)); // highlight the third match
    assert_eq!(app.file_search.as_ref().unwrap().selected, 2);
    app.set_file_matches("x", vec![fm("x9")]); // the list shrank to one
    assert_eq!(app.file_search.as_ref().unwrap().selected, 0, "clamped");
    assert_eq!(
        app.highlighted_file().map(|m| m.path.as_str()),
        Some("x9"),
        "the highlight lands on a real row"
    );
}

#[test]
fn an_empty_result_refresh_leaves_a_harmless_selection() {
    let mut app = App::new();
    type_str(&mut app, "@x");
    app.set_file_matches("x", vec![fm("x1"), fm("x2")]);
    app.on_key(key(KeyCode::Down));
    app.set_file_matches("x", Vec::new()); // everything filtered away
    assert_eq!(app.file_search.as_ref().unwrap().selected, 0);
    assert!(app.highlighted_file().is_none(), "nothing to accept");
    assert_eq!(
        app.on_key(key(KeyCode::Tab)),
        Action::None,
        "Tab with no match falls through harmlessly"
    );
}

#[test]
fn the_file_picker_stays_closed_in_shell_mode() {
    let mut app = App::new();
    type_str(&mut app, "!ls @a");
    assert!(app.shell_mode);
    assert!(app.file_search.is_none());
}

#[test]
fn accepting_a_path_with_spaces_quotes_it() {
    let mut app = App::new();
    type_str(&mut app, "@my");
    app.set_file_matches("my", vec![fm("my docs/notes.md")]);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.input.text(), "\"my docs/notes.md\" ");
}

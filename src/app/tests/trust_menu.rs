//! The `/trust` review menu's state and key map (`docs/project-config.md`).

use super::*;

use crate::trust::{TrustAction, TrustFileReview, TrustReview};

/// A pending hooks file plus recorded trust — both options on offer.
fn sample_review() -> TrustReview {
    TrustReview {
        root: "~/repo".to_string(),
        trusted: true,
        files: vec![TrustFileReview {
            label: "Hooks".to_string(),
            path: "~/repo/.alter-zero/hooks.json".to_string(),
            items: vec!["Stop: ./fmt.sh".to_string()],
            error: None,
            pending: true,
        }],
    }
}

fn trust_app() -> App {
    let mut app = App::new();
    app.open_trust_menu(sample_review());
    app
}

#[test]
fn slash_trust_dispatches_the_open_action_mid_turn_too() {
    let mut app = App::new();
    type_chars(&mut app, "/trust");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenTrustMenu);
    assert!(
        app.input.is_empty(),
        "running the command consumes the token"
    );
    assert!(app.command_menu.is_none());

    // Mid-turn too — the menu only replaces the composer (the /model rule);
    // reviewing a config file touches nothing running.
    let mut busy = App::new();
    busy.begin_stream();
    type_chars(&mut busy, "/trust");
    assert_eq!(busy.on_key(key(KeyCode::Enter)), Action::OpenTrustMenu);
}

#[test]
fn opening_selects_the_first_option_and_abandons_the_composer_bands() {
    let mut app = App::new();
    app.shortcuts_open = true;
    type_chars(&mut app, "/tru");
    assert!(app.command_menu.is_some());
    app.open_trust_menu(sample_review());
    assert!(app.command_menu.is_none());
    assert!(!app.shortcuts_open);
    let menu = app.trust_menu.as_ref().expect("the menu is open");
    assert_eq!(menu.selected, 0);
    assert_eq!(
        menu.review.options(),
        vec![TrustAction::Approve, TrustAction::Revoke]
    );
}

#[test]
fn arrows_move_and_enter_applies_the_highlighted_option() {
    let mut app = trust_app();
    // ↓ to the revoke row (clamped past the end), ↑ back, Enter applies.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.trust_menu.as_ref().unwrap().selected, 1);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.trust_menu.as_ref().unwrap().selected, 0);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ApplyTrust(TrustAction::Approve)
    );
    assert!(app.trust_menu.is_none(), "applying closes the menu");
}

#[test]
fn digits_jump_apply_their_option() {
    let mut app = trust_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::ApplyTrust(TrustAction::Revoke)
    );
    assert!(app.trust_menu.is_none());
}

#[test]
fn esc_and_ctrl_c_close_without_applying() {
    let mut app = trust_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseTrustMenu);
    assert!(app.trust_menu.is_none());

    let mut app = trust_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseTrustMenu);
    assert!(app.trust_menu.is_none());
}

#[test]
fn a_review_with_no_options_answers_enter_with_nothing() {
    let mut app = App::new();
    app.open_trust_menu(TrustReview {
        root: "~/repo".to_string(),
        trusted: false,
        files: Vec::new(),
    });
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.trust_menu.is_some(), "nothing to apply, nothing closes");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseTrustMenu);
}

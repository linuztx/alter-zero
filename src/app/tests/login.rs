//! The inline `/login` API-key onboarding (`docs/llm.md`).

use super::*;

#[test]
fn slash_login_opens_the_onboarding_when_idle() {
    let mut app = App::new();
    type_chars(&mut app, "/login");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenKeyOnboarding);
    assert!(app.input.is_empty(), "running a command clears the draft");
}

#[test]
fn slash_login_opens_the_onboarding_mid_turn() {
    // /login works mid-turn like /model — saving a key never touches the
    // running turn (docs/toast.md). The *loop* builds the provider choices.
    let mut app = App::new();
    app.begin_stream();
    type_chars(&mut app, "/login");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenKeyOnboarding);
    assert!(app.turn_active(), "the turn keeps running underneath");
}

#[test]
fn open_key_onboarding_starts_on_the_provider_step() {
    let app = login_app();
    let onboarding = app.key_onboarding.as_ref().expect("open");
    assert_eq!(onboarding.step, KeyStep::Provider);
    assert_eq!(onboarding.providers.len(), 3);
    assert_eq!(app.view, View::Conversation, "inline, not an overlay");
}

#[test]
fn opening_onboarding_abandons_other_bands_and_the_model_picker() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_model_picker("m");
    app.open_key_onboarding(sample_choices(), "~/.alter-zero/.env");
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.model_picker.is_none(), "the model picker is dismissed");
}

#[test]
fn esc_on_the_key_step_steps_back_to_the_provider_list() {
    let mut app = key_app("openrouter");
    type_chars(&mut app, "half-typed");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Provider, "back to the list");
    assert!(onboarding.key_input.is_empty(), "the draft key is dropped");
    assert!(onboarding.chosen.is_none());
}

#[test]
fn ctrl_c_closes_the_onboarding_not_the_app() {
    let mut app = key_app("openrouter"); // even mid-key-entry
    assert_eq!(app.on_key(ctrl('c')), Action::CloseKeyOnboarding);
    assert!(app.key_onboarding.is_none());
}

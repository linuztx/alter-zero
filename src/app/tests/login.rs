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
fn opening_onboarding_abandons_other_bands_and_the_model_picker() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_model_picker("m");
    app.open_key_onboarding(
        sample_choices(),
        sample_subscriptions(),
        "~/.alter-zero/.env",
    );
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

// --- the method step: "Use a subscription" / "Use an API key" (docs/copilot.md) ---

#[test]
fn login_opens_on_the_method_step() {
    // /login now leads with the two ways in; the API-key provider list is one
    // step down rather than the root.
    let app = login_app();
    let onboarding = app.key_onboarding.as_ref().expect("open");
    assert_eq!(onboarding.step, KeyStep::Method);
    assert_eq!(
        onboarding.method_labels(),
        vec!["Use a subscription", "Use an API key"]
    );
    assert_eq!(onboarding.providers.len(), 3);
    assert_eq!(app.view, View::Conversation, "inline, not an overlay");
}

#[test]
fn enter_on_use_an_api_key_opens_the_provider_list() {
    let mut app = login_app();
    // Row 1 is "Use an API key".
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Provider);
    assert_eq!(onboarding.providers.len(), 3);
}

#[test]
fn enter_on_use_a_subscription_opens_the_subscription_list() {
    let mut app = login_app();
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Subscription);
    assert_eq!(onboarding.subscriptions.len(), 1);
    assert_eq!(onboarding.subscriptions[0].name, "GitHub Copilot");
}

#[test]
fn esc_steps_back_to_the_method_step_from_either_list() {
    let mut app = login_app();
    app.on_key(key(KeyCode::Enter)); // subscriptions
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Method);

    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // providers
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Method);
}

#[test]
fn esc_on_the_method_step_closes_the_flow() {
    let mut app = login_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseKeyOnboarding);
    assert!(app.key_onboarding.is_none());
}

#[test]
fn enter_on_a_subscription_starts_its_device_login() {
    let mut app = login_app();
    app.on_key(key(KeyCode::Enter)); // subscriptions
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::StartDeviceLogin("github_copilot".to_string())
    );
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Device);
    let device = onboarding.device.as_ref().expect("the device page opened");
    assert_eq!(device.provider_name, "GitHub Copilot");
    assert_eq!(device.status, DeviceStatus::Starting);
}

#[test]
fn the_device_page_shows_the_code_the_boundary_delivers() {
    let mut app = device_app();
    app.set_device_code("https://github.com/login/device", "C363-262E");
    let device = app
        .key_onboarding
        .as_ref()
        .unwrap()
        .device
        .as_ref()
        .unwrap();
    assert_eq!(device.user_code, "C363-262E");
    assert_eq!(device.verification_uri, "https://github.com/login/device");
    assert_eq!(device.status, DeviceStatus::Waiting);
}

#[test]
fn c_copies_the_device_code_only_once_there_is_one() {
    let mut app = device_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::None,
        "nothing to copy while the code is still being requested"
    );
    app.set_device_code("https://github.com/login/device", "C363-262E");
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::CopyDeviceCode("C363-262E".to_string())
    );
}

#[test]
fn esc_cancels_the_device_login_back_to_the_subscription_list() {
    let mut app = device_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CancelDeviceLogin);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Subscription);
    assert!(onboarding.device.is_none(), "the page is torn down");
}

#[test]
fn a_failed_device_login_is_reported_on_the_page_not_closed() {
    // The user must see *why* it failed while the page is still up; only Esc
    // takes it down.
    let mut app = device_app();
    app.fail_device_login("GitHub said no");
    let device = app
        .key_onboarding
        .as_ref()
        .unwrap()
        .device
        .as_ref()
        .unwrap();
    assert_eq!(
        device.status,
        DeviceStatus::Failed("GitHub said no".to_string())
    );
}

#[test]
fn the_device_page_does_not_ride_the_status_animation_chain() {
    // The 32 ms chain exists for a shimmering spinner. This page's only moving
    // part is an `mm:ss` countdown, and every frame it costs carries a cursor
    // hide and a re-seat — thirty a second is what a terminal with a
    // cursor-trail animation renders as a permanent shimmer over the page. The
    // boundary gives it its own once-a-second tick instead (`docs/copilot.md`).
    let app = device_app();
    assert!(app.device_login_active(), "the page is up");
    assert!(
        !app.wants_animation_frames(),
        "an open device page must not re-arm the status chain"
    );
}

#[test]
fn the_countdown_is_injected_by_the_boundary_clock() {
    // The remaining time is a clock read — injected per draw like the status
    // line's elapsed, never computed in the pure core.
    let mut app = device_app();
    assert!(app.device_login_active());
    app.set_device_remaining(Some(std::time::Duration::from_secs(851)));
    let device = app
        .key_onboarding
        .as_ref()
        .unwrap()
        .device
        .as_ref()
        .unwrap();
    assert_eq!(device.remaining, Some(std::time::Duration::from_secs(851)));
}

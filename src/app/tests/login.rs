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
    // A subscription offering one way in opens its page at once — no
    // method choice stands between the row and the sign-in.
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::StartDeviceLogin {
            provider: "github_copilot".to_string(),
            kind: SigninKind::DeviceCode,
        }
    );
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Device);
    let device = onboarding.device.as_ref().expect("the device page opened");
    assert_eq!(device.provider_name, "GitHub Copilot");
    assert_eq!(device.kind, SigninKind::DeviceCode);
    assert_eq!(device.status, DeviceStatus::Starting);
}

// --- the sign-in method choice: a subscription offering two ways in (docs/chatgpt.md) ---

#[test]
fn enter_on_a_subscription_offering_two_sign_ins_opens_the_method_choice() {
    // ChatGPT Codex signs in through a browser by default, or with a device
    // code on a headless machine. The row cannot open one page and hide the
    // other, so Enter on it asks which — a titled two-row choice, the
    // default highlighted, no page open yet.
    let mut app = login_app_with_chatgpt();
    app.on_key(key(KeyCode::Enter)); // subscriptions
    app.on_key(key(KeyCode::Down)); // ChatGPT Codex
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::SigninMethod);
    assert_eq!(
        onboarding.chosen_subscription().map(|s| s.name.as_str()),
        Some("ChatGPT Codex")
    );
    assert_eq!(
        onboarding.signin_method_labels(),
        vec!["Browser login (default)", "Device code login (headless)"]
    );
    assert_eq!(onboarding.selected, 0, "the default is highlighted");
    assert!(onboarding.device.is_none(), "no sign-in page yet");
}

#[test]
fn a_sign_in_kind_names_its_method_row() {
    // The first kind a subscription lists is its default and says so; the
    // device code says what it is for, since "headless" is the one word that
    // tells an SSH user which row is theirs.
    assert_eq!(
        SigninKind::BrowserLink.method_label(true),
        "Browser login (default)"
    );
    assert_eq!(
        SigninKind::DeviceCode.method_label(false),
        "Device code login (headless)"
    );
    assert_eq!(
        SigninKind::DeviceCode.method_label(true),
        "Device code login (default)"
    );
    assert_eq!(SigninKind::BrowserLink.method_label(false), "Browser login");
}

#[test]
fn enter_on_the_default_row_starts_the_browser_sign_in() {
    let mut app = signin_method_app();
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::StartDeviceLogin {
            provider: "chatgpt_codex".to_string(),
            kind: SigninKind::BrowserLink,
        }
    );
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Device);
    let device = onboarding.device.as_ref().expect("the sign-in page opened");
    assert_eq!(device.provider_id, "chatgpt_codex");
    assert_eq!(device.provider_name, "ChatGPT Codex");
    assert_eq!(device.kind, SigninKind::BrowserLink);
    assert_eq!(device.status, DeviceStatus::Starting);
}

#[test]
fn the_device_code_row_starts_the_device_sign_in() {
    // The same page GitHub Copilot's code lands on, so the boundary must be
    // told which flow to run — the page alone cannot say.
    let mut app = signin_method_app();
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::StartDeviceLogin {
            provider: "chatgpt_codex".to_string(),
            kind: SigninKind::DeviceCode,
        }
    );
    let device = app
        .key_onboarding
        .as_ref()
        .unwrap()
        .device
        .as_ref()
        .expect("the device page opened");
    assert_eq!(device.kind, SigninKind::DeviceCode);
    assert_eq!(device.provider_name, "ChatGPT Codex");
}

#[test]
fn the_method_choice_wraps_and_ignores_typing() {
    // Two rows, no filter: it is a question with two answers, not a list to
    // search, so a typed character neither moves the highlight nor opens a
    // query.
    let mut app = signin_method_app();
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 1, "wraps");
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.key_onboarding.as_ref().unwrap().selected,
        0,
        "wraps back"
    );
    app.on_key(key(KeyCode::End));
    assert_eq!(app.key_onboarding.as_ref().unwrap().selected, 1);
    assert_eq!(app.on_key(key(KeyCode::Char('x'))), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::SigninMethod);
    assert_eq!(onboarding.selected, 1, "typing moves nothing");
    assert!(onboarding.query.is_empty(), "and opens no filter");
    app.paste_into_key_onboarding("pasted");
    assert!(app.key_onboarding.as_ref().unwrap().query.is_empty());
}

#[test]
fn esc_on_the_method_choice_steps_back_to_the_subscription_list() {
    let mut app = signin_method_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Subscription);
    assert!(onboarding.chosen_subscription().is_none());
    assert_eq!(onboarding.subscriptions.len(), 2, "nothing was dropped");
}

#[test]
fn esc_on_a_page_reached_through_the_method_choice_returns_to_it() {
    // Codex's own browser page says "on a headless machine, press Esc and
    // choose the device code": the choice must be one Esc away from the page
    // it opened, with the row just tried still highlighted.
    let mut app = signin_method_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // the device page
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CancelDeviceLogin);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::SigninMethod);
    assert_eq!(onboarding.selected, 1, "the row just tried");
    assert!(onboarding.device.is_none(), "the page is torn down");
    // …and from a page a single-flow subscription opened, Esc still returns
    // to the list, since there was never a choice to return to.
    let mut app = device_app();
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.key_onboarding.as_ref().unwrap().step,
        KeyStep::Subscription
    );
}

#[test]
fn ctrl_c_on_the_method_choice_closes_the_flow_with_nothing_to_cancel() {
    let mut app = signin_method_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseKeyOnboarding);
    assert!(app.key_onboarding.is_none());
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

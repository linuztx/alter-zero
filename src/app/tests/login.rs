//! The inline `/login` API-key onboarding (`docs/llm.md`).

use super::*;
use crate::llm::local::Verdict;

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
        sample_locals(),
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
    // /login leads with the three ways in; the API-key provider list is one
    // step down rather than the root, and so is the local server list.
    let app = login_app();
    let onboarding = app.key_onboarding.as_ref().expect("open");
    assert_eq!(onboarding.step, KeyStep::Method);
    assert_eq!(
        onboarding.method_labels(),
        vec!["Use a subscription", "Use an API key", "Use a local model"]
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

// --- the local half: "Use a local model" (docs/local-models.md) ---

/// The test the boundary was asked to run for `host`, with `token`.
fn test_of(host: &str, token: Option<&str>) -> ServerTest {
    ServerTest {
        host: host.to_string(),
        token: token.map(str::to_string),
    }
}

/// Enter on the server page, expecting a connection test of `host`.
fn enter_tests(app: &mut App, provider: &str, host: &str, token: Option<&str>) -> ServerTest {
    let test = test_of(host, token);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::TestLocalServer {
            provider: provider.to_string(),
            test: test.clone(),
        }
    );
    assert_eq!(server_page(app).testing.as_ref(), Some(&test));
    test
}

#[test]
fn enter_on_use_a_local_model_opens_the_local_server_list() {
    let mut app = login_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down)); // "Use a local model"
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Local);
    let names: Vec<&str> = onboarding
        .local_matches()
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(names, ["LM Studio", "Ollama", "vLLM"]);
}

#[test]
fn the_local_list_filters_and_steps_back_to_the_root() {
    let mut app = local_app();
    type_chars(&mut app, "olla");
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.local_matches().len(), 1);
    assert_eq!(onboarding.highlighted_local().unwrap().id, "ollama");
    // Esc clears the filter first, then steps back, never out.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(
        app.key_onboarding.as_ref().unwrap().local_matches().len(),
        3
    );
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Method);
}

#[test]
fn enter_on_a_local_server_opens_its_page() {
    let mut app = local_app();
    app.on_key(key(KeyCode::Down)); // Ollama
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Server);
    assert_eq!(onboarding.chosen_local().unwrap().id, "ollama");
    let page = server_page(&app);
    assert!(page.address.is_empty(), "the field opens empty");
    assert!(page.testing.is_none() && page.failure.is_none());
    assert!(!page.asks_for_token());
}

#[test]
fn an_empty_address_tests_the_one_in_force() {
    // The field shows what is in force — the variable's value, else the
    // file's own — and an empty Enter tests it: re-testing a working server
    // is one keystroke.
    let mut app = server_app("ollama");
    enter_tests(&mut app, "ollama", "gpu-box:11434", None);
    let mut app = server_app("lmstudio");
    enter_tests(&mut app, "lmstudio", "http://127.0.0.1:1234", None);
}

#[test]
fn a_typed_address_is_tested_as_typed() {
    // The grammar is the config's (`localhost:4321` is a server — docs/
    // local-models.md); the field keeps what was typed, trimmed.
    let mut app = server_app("lmstudio");
    type_chars(&mut app, " localhost:4321 ");
    enter_tests(&mut app, "lmstudio", "localhost:4321", None);
}

#[test]
fn a_passed_test_closes_the_flow_and_saves_the_address() {
    let mut app = server_app("lmstudio");
    type_chars(&mut app, "localhost:4321");
    let test = enter_tests(&mut app, "lmstudio", "localhost:4321", None);
    let saved = app.finish_server_test(&test, Verdict::Passed(3));
    assert_eq!(
        saved,
        Some(ServerSave {
            provider: "lmstudio".into(),
            name: "LM Studio".into(),
            env: vec![("LMSTUDIO_HOST".into(), "localhost:4321".into())],
            models: 3,
        })
    );
    assert!(
        app.key_onboarding.is_none(),
        "a passed test closes the flow"
    );
}

#[test]
fn a_failed_test_keeps_the_page_and_says_why() {
    let mut app = server_app("lmstudio");
    type_chars(&mut app, "localhost:4321");
    let test = enter_tests(&mut app, "lmstudio", "localhost:4321", None);
    let reason = "Nothing is answering at localhost:4321 — is LM Studio's server running?";
    assert_eq!(
        app.finish_server_test(&test, Verdict::Failed(reason.into())),
        None,
        "nothing is saved"
    );
    let page = server_page(&app);
    assert_eq!(page.failure.as_deref(), Some(reason));
    assert!(page.testing.is_none());
    assert_eq!(page.address, "localhost:4321", "the address stays to fix");
    // Editing the address clears the reason: it was about the old one.
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(server_page(&app).failure, None);
    assert_eq!(server_page(&app).address, "localhost:432");
}

#[test]
fn an_answer_to_a_test_no_longer_running_is_dropped() {
    let mut app = server_app("lmstudio");
    type_chars(&mut app, "first:1234");
    let first = enter_tests(&mut app, "lmstudio", "first:1234", None);
    // Esc stops the test and keeps the page…
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CancelServerTest);
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Server);
    assert!(server_page(&app).testing.is_none());
    // …and an answer arriving for it afterwards changes nothing.
    assert_eq!(app.finish_server_test(&first, Verdict::Passed(1)), None);
    assert!(
        app.key_onboarding.is_some(),
        "a dropped answer saves nothing"
    );
    // Nor does it land on the test that replaced it.
    for _ in 0.."first:1234".len() {
        app.on_key(key(KeyCode::Backspace));
    }
    type_chars(&mut app, "second:1234");
    let second = enter_tests(&mut app, "lmstudio", "second:1234", None);
    assert_eq!(
        app.finish_server_test(&first, Verdict::Failed("old".into())),
        None
    );
    assert_eq!(server_page(&app).testing.as_ref(), Some(&second));
    assert_eq!(server_page(&app).failure, None);
}

#[test]
fn the_field_waits_while_its_test_runs() {
    let mut app = server_app("lmstudio");
    enter_tests(&mut app, "lmstudio", "http://127.0.0.1:1234", None);
    // Typing, Backspace, a second Enter and a paste all wait.
    type_chars(&mut app, "x");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    app.paste_into_key_onboarding("gpu-box:1234");
    let page = server_page(&app);
    assert!(page.address.is_empty());
    assert!(page.testing.is_some());
}

#[test]
fn ctrl_c_while_a_test_runs_closes_the_flow_and_stops_it() {
    let mut app = server_app("lmstudio");
    enter_tests(&mut app, "lmstudio", "http://127.0.0.1:1234", None);
    assert_eq!(app.on_key(ctrl('c')), Action::CancelServerTest);
    assert!(app.key_onboarding.is_none());
}

#[test]
fn esc_on_the_address_steps_back_to_the_local_list() {
    let mut app = server_app("ollama");
    type_chars(&mut app, "half-typed");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Local);
    assert!(onboarding.server.is_none(), "the page and its draft go");
    assert!(onboarding.chosen_local.is_none());
}

#[test]
fn an_unauthorized_answer_asks_for_a_token_for_the_same_address() {
    // LM Studio's "Require Authentication": the server is there and says
    // so — the page asks for its token rather than reporting a failure.
    let mut app = server_app("lmstudio");
    type_chars(&mut app, "gpu-box");
    let test = enter_tests(&mut app, "lmstudio", "gpu-box", None);
    let reason = "LM Studio at gpu-box:1234 wants an API token.";
    assert_eq!(
        app.finish_server_test(&test, Verdict::Unauthorized(reason.into())),
        None
    );
    let page = server_page(&app);
    assert!(page.asks_for_token());
    assert_eq!(
        page.token_for,
        Some(TokenAsk {
            host: "gpu-box".into(),
            reason: reason.into(),
        })
    );
    assert_eq!(page.failure, None, "a request for a token is no failure");
    // An empty token is nothing to test.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    type_chars(&mut app, "sk-lm-1");
    let with_token = enter_tests(&mut app, "lmstudio", "gpu-box", Some("sk-lm-1"));
    // Passed: the address and the token are both saved.
    let saved = app
        .finish_server_test(&with_token, Verdict::Passed(2))
        .expect("saved");
    assert_eq!(
        saved.env,
        vec![
            ("LMSTUDIO_HOST".to_string(), "gpu-box".to_string()),
            ("LM_API_TOKEN".to_string(), "sk-lm-1".to_string()),
        ]
    );
    assert!(app.key_onboarding.is_none());
}

#[test]
fn a_refused_token_says_so_on_the_token_page() {
    let mut app = server_app("lmstudio");
    let test = enter_tests(&mut app, "lmstudio", "http://127.0.0.1:1234", None);
    let reason = "LM Studio at 127.0.0.1:1234 wants an API token.";
    app.finish_server_test(&test, Verdict::Unauthorized(reason.into()));
    type_chars(&mut app, "wrong");
    let tried = enter_tests(&mut app, "lmstudio", "http://127.0.0.1:1234", Some("wrong"));
    assert_eq!(
        app.finish_server_test(&tried, Verdict::Unauthorized(reason.into())),
        None
    );
    let page = server_page(&app);
    assert!(page.asks_for_token(), "still asking");
    let failure = page.failure.as_deref().expect("the refusal is said");
    assert!(failure.contains("refused"), "{failure}");
    assert_eq!(page.token, "wrong", "kept to fix");
    // Typing clears it, as on the address.
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(server_page(&app).failure, None);
}

#[test]
fn esc_on_the_token_page_returns_to_the_address() {
    let mut app = server_app("lmstudio");
    type_chars(&mut app, "gpu-box");
    let test = enter_tests(&mut app, "lmstudio", "gpu-box", None);
    app.finish_server_test(&test, Verdict::Unauthorized("wants one".into()));
    type_chars(&mut app, "sk");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    let page = server_page(&app);
    assert!(!page.asks_for_token());
    assert!(page.token.is_empty(), "the token draft goes");
    assert_eq!(page.address, "gpu-box", "the address stays");
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Server);
}

#[test]
fn a_server_its_block_fixes_is_tested_and_nothing_is_saved() {
    // A keyless server described in the user's own providers.toml with no
    // host variable: its address is the file's to change, so the field takes
    // no typing — Enter tests it, and a pass saves nothing.
    let mut app = server_app("vllm");
    type_chars(&mut app, "ignored");
    app.paste_into_key_onboarding("ignored");
    assert!(server_page(&app).address.is_empty());
    let test = enter_tests(&mut app, "vllm", "http://localhost:8000/v1", None);
    let saved = app
        .finish_server_test(&test, Verdict::Passed(1))
        .expect("the pass still closes the flow");
    assert!(saved.env.is_empty());
}

#[test]
fn a_pasted_address_keeps_its_punctuation_and_loses_its_newline() {
    let mut app = server_app("ollama");
    app.paste_into_key_onboarding("http://192.168.1.5:11434/\n");
    assert_eq!(server_page(&app).address, "http://192.168.1.5:11434/");
}

#[test]
fn the_confirmation_counts_what_there_is_to_pick() {
    let save = |models| ServerSave {
        provider: "lmstudio".into(),
        name: "LM Studio".into(),
        env: Vec::new(),
        models,
    };
    assert_eq!(
        save(3).confirmation(),
        "Connected to LM Studio — 3 models · run /model to pick one"
    );
    assert_eq!(
        save(1).confirmation(),
        "Connected to LM Studio — 1 model · run /model to use it"
    );
    assert_eq!(
        save(0).confirmation(),
        "Connected to LM Studio, but it has no models to chat with yet"
    );
}

#[test]
fn a_save_the_environment_overrides_says_which_variable_to_unset() {
    // The process env is read before `.env`: a variable exported in the
    // shell keeps the session off the address that just passed its test,
    // which the confirmation must not hide (docs/local-models.md).
    let save = ServerSave {
        provider: "lmstudio".into(),
        name: "LM Studio".into(),
        env: vec![
            ("LMSTUDIO_HOST".into(), "127.0.0.1:1234".into()),
            ("LM_API_TOKEN".into(), "sk-lm".into()),
        ],
        models: 1,
    };
    assert_eq!(save.overridden_notice(&[]), None, "nothing overrides it");
    assert_eq!(
        save.overridden_notice(&["LMSTUDIO_HOST".into()]).as_deref(),
        Some("Saved — unset LMSTUDIO_HOST in your environment, which overrides it")
    );
    // Named in the order they were saved, and only what was saved.
    assert_eq!(
        save.overridden_notice(&["LM_API_TOKEN".into(), "LMSTUDIO_HOST".into(), "PATH".into()])
            .as_deref(),
        Some(
            "Saved — unset LMSTUDIO_HOST and LM_API_TOKEN in your environment, which override them"
        )
    );
    assert_eq!(save.overridden_notice(&["PATH".into()]), None);
}

//! The `/usage` page: what is left of each subscription (`docs/usage.md`).

use super::*;

use crate::llm::plan_usage::{Allowance, Limit, Meter, PlanUsage};

/// The subscriptions a test session is signed in to: the one in use first,
/// the way the boundary orders them.
fn entries() -> Vec<UsageEntry> {
    vec![
        UsageEntry::new(
            "chatgpt_codex",
            "ChatGPT Codex",
            true,
            Some("https://chatgpt.com/settings/usage"),
        ),
        UsageEntry::new(
            "github_copilot",
            "GitHub Copilot",
            false,
            Some("https://github.com/settings/copilot"),
        ),
    ]
}

/// An app with the page open over [`entries`], and the generation its
/// fetches were tagged with.
fn usage_app() -> (App, u64) {
    let mut app = App::new();
    let generation = app.open_usage_page(entries(), 1_000, 0);
    (app, generation)
}

/// One subscription's answer with a single window at `used` percent.
fn answer(used: f64) -> PlanUsage {
    PlanUsage {
        plan: Some("Pro".to_string()),
        limits: vec![Limit::new(
            "Weekly limit",
            Allowance::Metered(Meter::new(used, None, None)),
        )],
        ..PlanUsage::default()
    }
}

/// The open page.
fn page(app: &App) -> &UsagePage {
    app.usage_page.as_ref().expect("the page is open")
}

/// The state of the entry for `id`.
fn state<'a>(app: &'a App, id: &str) -> &'a UsageState {
    &page(app)
        .entries
        .iter()
        .find(|entry| entry.provider_id == id)
        .expect("listed")
        .state
}

// ===== the command =====

#[test]
fn the_palette_lists_usage_right_after_context() {
    // The two "what is used" pages sit together: the window, then the plan.
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let usage = names
        .iter()
        .position(|n| *n == "usage")
        .expect("/usage is registered");
    assert_eq!(names[usage - 1], "context");
    assert_eq!(
        COMMANDS[usage].description,
        "Show your subscriptions' usage limits"
    );
    assert_eq!(COMMANDS[usage].effect, CommandEffect::Usage);
}

#[test]
fn slash_usage_asks_the_loop_to_open_the_page_idle_and_mid_turn() {
    // Which subscriptions are signed in is the boundary's to know (the
    // providers table and the key store), so the pure side only asks — the
    // `/model` pattern — and the turn is never touched.
    for mid_turn in [false, true] {
        let mut app = App::new();
        if mid_turn {
            app.begin_stream();
        }
        type_str(&mut app, "/usage");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::OpenUsagePage,
            "mid-turn {mid_turn}"
        );
        assert!(app.input.is_empty(), "the command was consumed");
        assert!(app.command_menu.is_none());
        assert_eq!(app.is_streaming(), mid_turn, "the turn was not touched");
    }
}

#[test]
fn opening_lists_every_subscription_fetching_and_abandons_the_bands() {
    let mut app = App::new();
    app.shortcuts_open = true;
    let generation = app.open_usage_page(entries(), 1_000, -14_400);
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none() && app.file_search.is_none());
    let page = page(&app);
    assert_eq!(page.selected, 0, "the subscription in use is first");
    assert_eq!(page.asked_at, 1_000);
    assert_eq!(page.utc_offset, -14_400);
    assert_eq!(page.generation(), generation);
    let names: Vec<_> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["ChatGPT Codex", "GitHub Copilot"]);
    assert!(
        page.entries
            .iter()
            .all(|e| e.state == UsageState::Fetching && !e.refreshing)
    );
    assert!(page.is_fetching());
}

#[test]
fn each_answer_lands_on_its_own_row() {
    let (mut app, generation) = usage_app();
    assert!(app.set_usage_result(generation, "github_copilot", Ok(answer(40.0)), 1_002));
    assert_eq!(
        state(&app, "github_copilot"),
        &UsageState::Ready {
            usage: answer(40.0),
            fetched_at: 1_002
        }
    );
    assert_eq!(state(&app, "chatgpt_codex"), &UsageState::Fetching);
    assert!(page(&app).is_fetching(), "one is still out");
    assert!(app.set_usage_result(
        generation,
        "chatgpt_codex",
        Err("ChatGPT Codex refused the sign-in — /login signs in again.".to_string()),
        1_003,
    ));
    assert_eq!(
        state(&app, "chatgpt_codex"),
        &UsageState::Failed(
            "ChatGPT Codex refused the sign-in — /login signs in again.".to_string()
        )
    );
    assert!(!page(&app).is_fetching());
}

#[test]
fn an_answer_from_an_earlier_fetch_or_after_close_is_dropped() {
    let (mut app, first) = usage_app();
    let second = app.begin_usage_refresh(1_050).expect("the page is open");
    assert_ne!(first, second, "every fetch is its own generation");
    assert!(
        !app.set_usage_result(first, "chatgpt_codex", Ok(answer(90.0)), 1_051),
        "a superseded fetch reports nothing"
    );
    assert_eq!(state(&app, "chatgpt_codex"), &UsageState::Fetching);
    assert!(
        !app.set_usage_result(second, "anthropic_console", Ok(answer(1.0)), 1_051),
        "a provider the page doesn't list"
    );

    app.close_usage_page();
    assert!(!app.set_usage_result(second, "chatgpt_codex", Ok(answer(5.0)), 1_052));
    assert!(app.usage_page.is_none(), "an answer never reopens the page");

    // A reopened page is a new generation: the closed one's answers miss.
    let third = app.open_usage_page(entries(), 2_000, 0);
    assert!(third != first && third != second);
    assert!(!app.set_usage_result(second, "chatgpt_codex", Ok(answer(5.0)), 2_001));
    assert!(app.set_usage_result(third, "chatgpt_codex", Ok(answer(5.0)), 2_001));
}

#[test]
fn a_refresh_keeps_the_numbers_until_the_new_answer_lands() {
    let (mut app, generation) = usage_app();
    app.set_usage_result(generation, "chatgpt_codex", Ok(answer(40.0)), 1_002);
    let refresh = app.begin_usage_refresh(1_500).unwrap();
    let page_now = page(&app);
    assert_eq!(page_now.asked_at, 1_500);
    let codex = &page_now.entries[0];
    assert!(codex.refreshing, "marked as being read again");
    assert_eq!(
        codex.state,
        UsageState::Ready {
            usage: answer(40.0),
            fetched_at: 1_002
        },
        "the old numbers stay on screen meanwhile"
    );
    assert!(page_now.is_fetching());
    app.set_usage_result(refresh, "chatgpt_codex", Ok(answer(55.0)), 1_501);
    let codex = &page(&app).entries[0];
    assert!(!codex.refreshing);
    assert_eq!(
        codex.state,
        UsageState::Ready {
            usage: answer(55.0),
            fetched_at: 1_501
        }
    );
    assert_eq!(
        App::new().begin_usage_refresh(1),
        None,
        "no page, no refresh"
    );
}

// ===== the keys =====

#[test]
fn r_asks_for_a_refresh_and_keeps_the_page_open() {
    for code in [KeyCode::Char('r'), KeyCode::Char('R')] {
        let (mut app, _) = usage_app();
        assert_eq!(app.on_key(key(code)), Action::RefreshUsage, "{code:?}");
        assert!(app.usage_page.is_some());
    }
}

#[test]
fn arrows_move_the_highlight_wrapping_and_home_end_jump() {
    let (mut app, _) = usage_app();
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(page(&app).selected, 1);
    app.on_key(key(KeyCode::Down));
    assert_eq!(page(&app).selected, 0, "↓ past the last wraps");
    app.on_key(key(KeyCode::Up));
    assert_eq!(page(&app).selected, 1, "↑ from the first wraps");
    app.on_key(key(KeyCode::Home));
    assert_eq!(page(&app).selected, 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(page(&app).selected, 1);
}

#[test]
fn esc_and_ctrl_c_close_the_page_without_quitting() {
    for close in [key(KeyCode::Esc), ctrl('c')] {
        let (mut app, _) = usage_app();
        assert_eq!(app.on_key(close), Action::CloseUsagePage);
        assert!(app.usage_page.is_none());
        assert_eq!(app.view, View::Conversation, "Ctrl+C never quits from here");
    }
}

#[test]
fn every_other_key_is_swallowed() {
    let (mut app, _) = usage_app();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char('/'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Left,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
    }
    assert!(app.input.is_empty(), "nothing reached the composer");
    assert!(app.usage_page.is_some());
    assert_eq!(page(&app).selected, 0);
}

#[test]
fn the_two_read_only_overlays_open_over_the_page() {
    // Ctrl+O and Ctrl+D pass through, as on `/context`, and the page waits
    // behind them — but the transcript opened from it never arms a rewind.
    let (mut app, _) = usage_app();
    app.record_user_message("hello");
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.view, View::Conversation);
    assert!(
        app.usage_page.is_some(),
        "the page is there on the way back"
    );
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert!(
        !app.overlay_esc_backtracks(),
        "the hint must not offer a rewind"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.backtrack.selected, None, "no preview began");
    assert!(app.usage_page.is_some());
    assert_eq!(app.history.len(), 1, "nothing was rewound");
}

#[test]
fn an_open_page_suppresses_the_ctrl_b_hint_and_asks_no_animation() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 100", None);
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_usage_page(entries(), 1_000, 0);
    assert_eq!(
        app.background_hint_elapsed(),
        None,
        "the page swallows Ctrl+B"
    );

    let (app, _) = usage_app();
    assert!(
        !app.wants_animation_frames(),
        "a fetching page is still: its rows say so in words, not a spinner"
    );
}

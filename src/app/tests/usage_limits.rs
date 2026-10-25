//! The `/usage` page: the command, the session's store of each
//! subscription's limits, and the key grammar (`docs/usage-limits.md`).

use super::*;

use crate::llm::usage_limits::{LimitWindow, PlanLimits};

/// A signed-in subscription as the boundary describes it.
fn entry(id: &str, name: &str, active: bool) -> SubscriptionEntry {
    SubscriptionEntry {
        id: id.to_string(),
        name: name.to_string(),
        active,
        dashboard: Some(format!("https://example.com/{id}/usage")),
    }
}

/// The three-subscription session most tests open on, ChatGPT in use.
fn entries() -> Vec<SubscriptionEntry> {
    vec![
        entry("github_copilot", "GitHub Copilot", false),
        entry("chatgpt_codex", "ChatGPT Codex", true),
        entry("cline_pass", "ClinePass", false),
    ]
}

/// One weekly window at `used` percent.
fn weekly(used: f64) -> PlanLimits {
    PlanLimits {
        plan: Some("Pro".to_string()),
        windows: vec![LimitWindow {
            label: "Weekly limit".to_string(),
            used_percent: used,
            resets_at: Some(2_000),
            detail: None,
            reached: used >= 100.0,
        }],
        ..PlanLimits::default()
    }
}

/// An app with the page open over [`entries`], a refresh begun at `now`.
fn usage_app() -> (App, u64) {
    let mut app = App::new();
    app.open_usage_limits(entries());
    let generation = app.begin_usage_limits_refresh(1_000);
    (app, generation)
}

/// The highlighted section's index.
fn selected(app: &App) -> usize {
    app.usage_limits
        .as_ref()
        .expect("the page is open")
        .selected
}

/// The stored subscription `id`.
fn stored<'a>(app: &'a App, id: &str) -> &'a SubscriptionLimits {
    app.subscription_limits()
        .iter()
        .find(|s| s.entry.id == id)
        .expect("the subscription is stored")
}

// ===== the /usage command =====

#[test]
fn the_palette_lists_usage_with_a_concise_description() {
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "usage")
        .expect("/usage is registered");
    assert_eq!(cmd.description, "Show your subscriptions' usage limits");
    assert_eq!(cmd.effect, CommandEffect::Usage);
}

#[test]
fn slash_usage_asks_the_boundary_to_open_the_page() {
    // Only the boundary knows which subscriptions are signed in, so the
    // command hands it the open, the `/model` picker's way.
    let mut app = App::new();
    type_chars(&mut app, "/usage");
    assert!(app.command_menu.is_some());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenUsageLimits);
    assert!(app.input.is_empty(), "the /usage token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn slash_usage_works_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/usage");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenUsageLimits);
    assert!(app.is_streaming(), "the turn was not touched");
}

// ===== opening, and the store =====

#[test]
fn opening_lists_the_signed_in_subscriptions_highlighting_the_one_in_use() {
    let mut app = App::new();
    app.open_usage_limits(entries());
    let ids: Vec<&str> = app
        .subscription_limits()
        .iter()
        .map(|s| s.entry.id.as_str())
        .collect();
    assert_eq!(ids, ["github_copilot", "chatgpt_codex", "cline_pass"]);
    assert_eq!(selected(&app), 1, "the session's own subscription");
    // With none in use, the first.
    let mut app = App::new();
    app.open_usage_limits(vec![entry("cline_pass", "ClinePass", false)]);
    assert_eq!(selected(&app), 0);
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_usage_limits(entries());
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.usage_limits.is_some());
}

#[test]
fn a_refresh_marks_every_subscription_and_answers_land_one_by_one() {
    let (mut app, generation) = usage_app();
    assert!(app.usage_limits_refreshing());
    assert!(app.subscription_limits().iter().all(|s| s.refreshing));
    app.set_plan_limits("chatgpt_codex", generation, Ok(weekly(12.0)), 1_005);
    let chatgpt = stored(&app, "chatgpt_codex");
    assert_eq!(chatgpt.limits, Some(weekly(12.0)));
    assert_eq!(chatgpt.fetched_at, Some(1_005));
    assert!(!chatgpt.refreshing);
    assert!(app.usage_limits_refreshing(), "two answers are still out");
    app.set_plan_limits(
        "github_copilot",
        generation,
        Err("GitHub refused the stored sign-in — run /login to sign in again".to_string()),
        1_006,
    );
    let copilot = stored(&app, "github_copilot");
    assert_eq!(copilot.limits, None);
    assert_eq!(
        copilot.error.as_deref(),
        Some("GitHub refused the stored sign-in — run /login to sign in again")
    );
    app.set_plan_limits("cline_pass", generation, Ok(weekly(100.0)), 1_007);
    assert!(!app.usage_limits_refreshing(), "every answer is in");
}

#[test]
fn a_failed_refresh_keeps_the_answer_before_it() {
    let (mut app, generation) = usage_app();
    app.set_plan_limits("chatgpt_codex", generation, Ok(weekly(12.0)), 1_005);
    let generation = app.begin_usage_limits_refresh(1_100);
    app.set_plan_limits(
        "chatgpt_codex",
        generation,
        Err("No connection to chatgpt.com".to_string()),
        1_101,
    );
    let chatgpt = stored(&app, "chatgpt_codex");
    assert_eq!(chatgpt.limits, Some(weekly(12.0)), "the last answer stays");
    assert_eq!(chatgpt.fetched_at, Some(1_005), "…and says how old it is");
    assert_eq!(
        chatgpt.error.as_deref(),
        Some("No connection to chatgpt.com")
    );
    // A success clears the failure.
    let generation = app.begin_usage_limits_refresh(1_200);
    app.set_plan_limits("chatgpt_codex", generation, Ok(weekly(15.0)), 1_201);
    assert_eq!(stored(&app, "chatgpt_codex").error, None);
}

#[test]
fn an_answer_to_an_older_refresh_is_dropped() {
    let (mut app, stale) = usage_app();
    let current = app.begin_usage_limits_refresh(1_050);
    app.set_plan_limits("chatgpt_codex", stale, Ok(weekly(99.0)), 1_051);
    assert_eq!(stored(&app, "chatgpt_codex").limits, None);
    assert!(stored(&app, "chatgpt_codex").refreshing, "still waiting");
    app.set_plan_limits("chatgpt_codex", current, Ok(weekly(12.0)), 1_052);
    assert_eq!(stored(&app, "chatgpt_codex").limits, Some(weekly(12.0)));
}

#[test]
fn an_answer_for_a_subscription_no_longer_listed_is_dropped() {
    let (mut app, generation) = usage_app();
    app.set_plan_limits("openrouter", generation, Ok(weekly(1.0)), 1_005);
    assert_eq!(app.subscription_limits().len(), 3);
}

#[test]
fn closing_cancels_the_refresh_and_drops_what_it_was_still_owed() {
    let (mut app, generation) = usage_app();
    app.close_usage_limits();
    assert!(app.usage_limits.is_none());
    assert!(!app.usage_limits_refreshing(), "nothing is out any more");
    app.set_plan_limits("chatgpt_codex", generation, Ok(weekly(12.0)), 1_005);
    assert_eq!(stored(&app, "chatgpt_codex").limits, None);
}

#[test]
fn reopening_keeps_what_was_fetched_and_follows_the_sign_ins() {
    let (mut app, generation) = usage_app();
    app.set_plan_limits("chatgpt_codex", generation, Ok(weekly(12.0)), 1_005);
    app.set_plan_limits("cline_pass", generation, Ok(weekly(40.0)), 1_005);
    app.close_usage_limits();
    // Copilot was signed out meanwhile, and Anthropic signed in.
    app.open_usage_limits(vec![
        entry("chatgpt_codex", "ChatGPT Codex", true),
        entry("cline_pass", "ClinePass", false),
        entry("anthropic_console", "Anthropic Console", false),
    ]);
    let ids: Vec<&str> = app
        .subscription_limits()
        .iter()
        .map(|s| s.entry.id.as_str())
        .collect();
    assert_eq!(ids, ["chatgpt_codex", "cline_pass", "anthropic_console"]);
    assert_eq!(stored(&app, "chatgpt_codex").limits, Some(weekly(12.0)));
    assert_eq!(stored(&app, "cline_pass").fetched_at, Some(1_005));
    assert_eq!(stored(&app, "anthropic_console").limits, None);
}

#[test]
fn the_page_refreshes_itself_a_minute_after_the_last_refresh_began() {
    let (mut app, generation) = usage_app();
    for id in ["github_copilot", "chatgpt_codex", "cline_pass"] {
        app.set_plan_limits(id, generation, Ok(weekly(1.0)), 1_002);
    }
    assert!(!app.usage_limits_refresh_due(1_059));
    assert!(app.usage_limits_refresh_due(1_060));
    // Never while one is out.
    app.begin_usage_limits_refresh(1_060);
    assert!(!app.usage_limits_refresh_due(1_200));
    // Never with the page closed.
    app.close_usage_limits();
    assert!(!app.usage_limits_refresh_due(5_000));
}

#[test]
fn the_wall_clock_and_the_local_offset_are_injected() {
    let mut app = App::new();
    assert_eq!(app.usage_limits_now(), None);
    assert_eq!(app.local_offset(1_000), 0, "UTC until the boundary says");
    app.set_usage_limits_now(1_234);
    assert_eq!(app.usage_limits_now(), Some(1_234));
    app.set_local_offset(|_| 3_600);
    assert_eq!(app.local_offset(1_000), 3_600);
}

// ===== the key grammar =====

#[test]
fn up_and_down_wrap_at_the_ends_and_home_and_end_jump() {
    let (mut app, _) = usage_app();
    assert_eq!(selected(&app), 1);
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(selected(&app), 2);
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 0, "wrapped to the first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(selected(&app), 2, "wrapped to the last");
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), 2);
}

#[test]
fn r_refreshes_unless_a_refresh_is_already_out() {
    let (mut app, generation) = usage_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Action::None,
        "one is out already — never doubled"
    );
    for id in ["github_copilot", "chatgpt_codex", "cline_pass"] {
        app.set_plan_limits(id, generation, Ok(weekly(1.0)), 1_002);
    }
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Action::RefreshUsageLimits
    );
}

#[test]
fn c_copies_the_highlighted_subscriptions_usage_link() {
    let (mut app, _) = usage_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::CopyUsageLink("https://example.com/chatgpt_codex/usage".to_string())
    );
    assert!(app.usage_limits.is_some(), "the page stays open");
    // A subscription with no page of its own has nothing to copy.
    let mut app = App::new();
    app.open_usage_limits(vec![SubscriptionEntry {
        dashboard: None,
        ..entry("custom", "Custom", false)
    }]);
    assert_eq!(app.on_key(key(KeyCode::Char('c'))), Action::None);
}

#[test]
fn esc_and_ctrl_c_close_the_page() {
    let (mut app, _) = usage_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseUsageLimits);
    assert!(app.usage_limits.is_none());
    let (mut app, _) = usage_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseUsageLimits);
    assert!(app.usage_limits.is_none(), "Ctrl+C closes, never quits");
}

#[test]
fn the_two_read_only_overlays_pass_through() {
    let (mut app, _) = usage_app();
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert!(
        app.usage_limits.is_some(),
        "the page waits behind the overlay"
    );
    app.on_key(ctrl('o'));
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
}

#[test]
fn an_overlay_over_the_page_never_arms_a_rewind() {
    // Ctrl+O from the page is a look, not a way into the Esc-Esc backtrack —
    // the `/context` page's rule: a rewind begun there would land back on a
    // page still open over the draft it recalled.
    let (mut app, _) = usage_app();
    app.record_user_message("hello there");
    assert!(app.has_backtrack_target());
    app.on_key(ctrl('o'));
    assert_eq!(app.view, View::ToolOutput);
    assert!(
        !app.overlay_esc_backtracks(),
        "the hint must not offer a rewind"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.backtrack.selected, None, "no preview began");
    assert_eq!(app.view, View::Conversation, "Esc closed the transcript");
    assert!(app.usage_limits.is_some(), "back on the page");
}

#[test]
fn the_page_owns_every_other_key() {
    let (mut app, _) = usage_app();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
    }
    assert!(app.input.is_empty(), "nothing reached the composer");
    assert!(app.usage_limits.is_some());
}

#[test]
fn an_open_page_suppresses_the_ctrl_b_hint() {
    let (mut app, _) = usage_app();
    app.set_command_elapsed(Some(Duration::from_secs(9)));
    assert_eq!(app.background_hint_elapsed(), None);
    app.close_usage_limits();
    assert_eq!(app.background_hint_elapsed(), Some(Duration::from_secs(9)));
}

//! The `/usage` page (`docs/usage.md`).

use super::*;

use crate::llm::usage::{Limit, MeterValue, UsageMeter, UsageReport};

fn target(provider: &str, name: &str, in_use: bool) -> UsageTarget {
    UsageTarget {
        provider: provider.to_string(),
        name: name.to_string(),
        in_use,
        dashboard: Some(format!("https://{provider}.example/usage")),
    }
}

/// Three signed-in subscriptions, the session running on ClinePass.
fn three() -> Vec<UsageTarget> {
    vec![
        target("chatgpt_codex", "ChatGPT Codex", false),
        target("cline_pass", "ClinePass", true),
        target("github_copilot", "GitHub Copilot", false),
    ]
}

/// A report with one weekly window `used` percent spent.
fn report(used: f64) -> UsageReport {
    UsageReport {
        plan: Some("Pro".to_string()),
        meters: vec![UsageMeter {
            group: None,
            label: "Weekly limit".to_string(),
            value: MeterValue::Limit(Limit {
                used_percent: used,
                counts: None,
                resets_at: Some(10_000),
                reached: used >= 100.0,
            }),
        }],
        ..UsageReport::default()
    }
}

/// An open page whose first round over [`three`] has begun.
fn open_reading() -> (App, u64) {
    let mut app = App::new();
    let round = app.open_usage_page(three()).expect("something to read");
    (app, round)
}

/// [`open_reading`], every read landed at `at` with a `used` share.
fn open_read(used: f64, at: i64) -> App {
    let (mut app, round) = open_reading();
    for provider in ["chatgpt_codex", "cline_pass", "github_copilot"] {
        app.finish_usage_fetch(round, provider, Ok(report(used)), at);
    }
    app
}

fn page(app: &App) -> &UsagePage {
    app.usage_page.as_ref().expect("the page is open")
}

fn providers(app: &App) -> Vec<&str> {
    page(app)
        .sections
        .iter()
        .map(|section| section.provider.as_str())
        .collect()
}

fn section<'a>(app: &'a App, provider: &str) -> &'a UsageSection {
    page(app)
        .sections
        .iter()
        .find(|section| section.provider == provider)
        .expect("the section is on the page")
}

// ===== the command =====

#[test]
fn the_palette_lists_usage_right_after_context() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let usage = names
        .iter()
        .position(|n| *n == "usage")
        .expect("/usage is registered");
    assert_eq!(names[usage - 1], "context", "the usage pages sit together");
    assert_eq!(
        COMMANDS[usage].description,
        "Show your subscriptions' usage limits"
    );
    assert_eq!(COMMANDS[usage].effect, CommandEffect::Usage);
}

#[test]
fn slash_usage_hands_the_open_to_the_boundary_idle_and_mid_turn() {
    // Only the boundary knows which subscriptions are signed in, so the
    // command opens nothing itself (`App::open_usage_page`).
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
        assert!(app.usage_page.is_none());
        assert!(app.input.is_empty(), "the command token is consumed");
        assert!(app.command_menu.is_none());
    }
}

#[test]
fn with_no_subscription_signed_in_nothing_opens() {
    // The `/export` empty rule: no page with nothing on it — the boundary
    // raises USAGE_EMPTY_NOTICE instead.
    let mut app = App::new();
    assert_eq!(app.open_usage_page(Vec::new()), None);
    assert!(app.usage_page.is_none());
    assert!(USAGE_EMPTY_NOTICE.contains("/login"));
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_usage_page(three());
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
}

// ===== rounds =====

#[test]
fn opening_reads_every_subscription_the_one_in_use_first() {
    let (app, round) = open_reading();
    assert!(round > 0);
    assert_eq!(
        providers(&app),
        ["cline_pass", "chatgpt_codex", "github_copilot"]
    );
    let page = page(&app);
    assert_eq!(page.round, round);
    assert!(page.loading());
    assert!(page.sections.iter().all(|section| section.loading));
    assert_eq!(page.selected, 0, "the highlight opens on the one in use");
    assert_eq!(
        section(&app, "cline_pass").dashboard.as_deref(),
        Some("https://cline_pass.example/usage"),
        "each section links its own provider's page"
    );
}

#[test]
fn a_closed_page_starts_no_round() {
    let mut app = App::new();
    assert_eq!(app.begin_usage_round(three()), None);
}

#[test]
fn each_result_lands_on_its_own_section() {
    let (mut app, round) = open_reading();
    app.finish_usage_fetch(round, "github_copilot", Ok(report(42.0)), 500);
    let copilot = section(&app, "github_copilot");
    assert!(!copilot.loading);
    assert_eq!(copilot.report, Some(report(42.0)));
    assert!(section(&app, "cline_pass").loading, "the others still read");
    assert!(page(&app).loading());
    assert_eq!(page(&app).updated_at, None, "the round is not done yet");

    app.finish_usage_fetch(round, "cline_pass", Err("refused".to_string()), 600);
    app.finish_usage_fetch(round, "chatgpt_codex", Ok(report(7.0)), 700);
    let cline = section(&app, "cline_pass");
    assert_eq!(cline.error.as_deref(), Some("refused"));
    assert_eq!(cline.report, None);
    assert!(!page(&app).loading());
    assert_eq!(page(&app).updated_at, Some(700), "stamped by its last read");
}

#[test]
fn a_result_for_another_round_or_a_closed_page_is_dropped() {
    let (mut app, first) = open_reading();
    let second = app.begin_usage_round(three()).unwrap();
    assert_ne!(first, second);
    app.finish_usage_fetch(first, "github_copilot", Ok(report(99.0)), 5);
    assert_eq!(section(&app, "github_copilot").report, None);
    assert!(section(&app, "github_copilot").loading);

    // Closed and reopened: the old round's result names no round of the
    // new page, however the counts line up.
    app.close_usage_page();
    app.finish_usage_fetch(second, "github_copilot", Ok(report(1.0)), 5);
    assert!(app.usage_page.is_none());
    let third = app.open_usage_page(three()).unwrap();
    assert!(third > second);
    app.finish_usage_fetch(second, "github_copilot", Ok(report(1.0)), 5);
    assert_eq!(section(&app, "github_copilot").report, None);
}

#[test]
fn a_refresh_keeps_every_report_until_its_replacement_lands() {
    let mut app = open_read(10.0, 100);
    let next = app.begin_usage_round(three()).unwrap();
    let copilot = section(&app, "github_copilot");
    assert!(copilot.loading);
    assert_eq!(copilot.report, Some(report(10.0)), "kept while re-reading");

    // A failed refresh keeps the last report beside the reason.
    app.finish_usage_fetch(next, "github_copilot", Err("rate limited".to_string()), 160);
    let copilot = section(&app, "github_copilot");
    assert_eq!(copilot.report, Some(report(10.0)));
    assert_eq!(copilot.error.as_deref(), Some("rate limited"));

    // A good one replaces both.
    let again = app.begin_usage_round(three()).unwrap();
    app.finish_usage_fetch(again, "github_copilot", Ok(report(20.0)), 220);
    let copilot = section(&app, "github_copilot");
    assert_eq!(copilot.report, Some(report(20.0)));
    assert_eq!(copilot.error, None);
}

#[test]
fn a_reopened_page_shows_the_last_reports_while_it_reads_them_again() {
    // Codex polls the same numbers; the page never opens blank twice.
    let mut app = open_read(10.0, 100);
    app.on_key(key(KeyCode::End));
    app.close_usage_page();
    assert!(app.usage_page.is_none());

    let round = app.open_usage_page(three()).unwrap();
    let copilot = section(&app, "github_copilot");
    assert_eq!(
        copilot.report,
        Some(report(10.0)),
        "the last report, at once"
    );
    assert!(copilot.loading, "…and read again");
    assert_eq!(page(&app).updated_at, Some(100));
    assert_eq!(page(&app).round, round);
    assert_eq!(
        page(&app).selected,
        0,
        "a page opens on the subscription in use, wherever it was left"
    );
}

#[test]
fn a_reopened_page_follows_the_sign_ins_made_since() {
    let mut app = open_read(10.0, 100);
    app.close_usage_page();
    app.open_usage_page(vec![
        target("github_copilot", "GitHub Copilot", true),
        target("claude_console", "Anthropic Console", false),
    ]);
    assert_eq!(providers(&app), ["github_copilot", "claude_console"]);
    assert_eq!(section(&app, "github_copilot").report, Some(report(10.0)));
    assert_eq!(section(&app, "claude_console").report, None, "new: empty");
}

#[test]
fn a_subscription_signed_out_since_leaves_the_page() {
    let (mut app, round) = open_reading();
    app.finish_usage_fetch(round, "github_copilot", Ok(report(10.0)), 100);
    app.begin_usage_round(vec![target("github_copilot", "GitHub Copilot", false)]);
    assert_eq!(providers(&app), ["github_copilot"]);
    assert_eq!(section(&app, "github_copilot").report, Some(report(10.0)));
}

#[test]
fn the_highlight_follows_its_subscription_across_a_round() {
    let (mut app, _) = open_reading();
    app.on_key(key(KeyCode::End));
    assert_eq!(providers(&app)[page(&app).selected], "github_copilot");
    // The subscription in use moved on — the list reorders, the highlight
    // stays with Copilot.
    app.begin_usage_round(vec![
        target("chatgpt_codex", "ChatGPT Codex", true),
        target("cline_pass", "ClinePass", false),
        target("github_copilot", "GitHub Copilot", false),
    ]);
    assert_eq!(providers(&app)[page(&app).selected], "github_copilot");
    // Gone altogether: the highlight is clamped, never out of range.
    app.begin_usage_round(vec![target("chatgpt_codex", "ChatGPT Codex", true)]);
    assert_eq!(page(&app).selected, 0);
}

#[test]
fn a_refresh_that_finds_every_subscription_signed_out_empties_the_page() {
    let mut app = open_read(10.0, 100);
    app.begin_usage_round(Vec::new()).unwrap();
    let page = page(&app);
    assert!(page.sections.is_empty());
    assert!(!page.loading());
    assert!(!app.usage_refresh_due(), "nothing to re-read");
}

#[test]
fn the_page_is_due_a_refresh_a_minute_after_its_last_read() {
    let (mut app, round) = open_reading();
    app.set_usage_now(1_000);
    assert!(!app.usage_refresh_due(), "still reading");
    for provider in ["chatgpt_codex", "cline_pass", "github_copilot"] {
        app.finish_usage_fetch(round, provider, Ok(report(1.0)), 1_000);
    }
    app.set_usage_now(1_000 + USAGE_REFRESH_SECS - 1);
    assert!(!app.usage_refresh_due());
    app.set_usage_now(1_000 + USAGE_REFRESH_SECS);
    assert!(app.usage_refresh_due());
    app.begin_usage_round(three());
    assert!(!app.usage_refresh_due(), "a round is in flight again");
}

#[test]
fn the_clock_reaches_only_an_open_page() {
    let mut app = App::new();
    app.set_usage_now(42);
    app.open_usage_page(three());
    assert_eq!(page(&app).now, 0, "a fresh page has not been told yet");
    app.set_usage_now(43);
    assert_eq!(page(&app).now, 43);
}

#[test]
fn the_local_offset_is_utc_until_the_boundary_says_otherwise() {
    let mut app = App::new();
    assert_eq!(app.local_offset(1_793_000_000), 0);
    app.set_local_offset(|at| if at > 100 { -14_400 } else { -18_000 });
    assert_eq!(app.local_offset(1_793_000_000), -14_400);
    assert_eq!(app.local_offset(50), -18_000, "read at the instant asked");
}

// ===== keys =====

#[test]
fn up_and_down_move_between_subscriptions_wrapping() {
    let (mut app, _) = open_reading();
    let selected = |app: &App| page(app).selected;
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 1);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 0, "↓ wraps to the first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(selected(&app), 2, "↑ wraps to the last");
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), 2);
}

#[test]
fn r_asks_for_a_refresh_only_once_the_reads_have_landed() {
    let (mut app, round) = open_reading();
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), Action::None, "reading");
    for provider in ["chatgpt_codex", "cline_pass", "github_copilot"] {
        app.finish_usage_fetch(round, provider, Ok(report(1.0)), 10);
    }
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Action::RefreshUsagePage
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('R'))),
        Action::RefreshUsagePage
    );
}

#[test]
fn r_refreshes_a_page_emptied_by_a_sign_out_too() {
    // A sign-in made in another terminal shows up without reopening.
    let mut app = open_read(10.0, 100);
    app.begin_usage_round(Vec::new());
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Action::RefreshUsagePage
    );
}

#[test]
fn c_copies_the_highlighted_subscriptions_usage_page() {
    let (mut app, _) = open_reading();
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::CopyUsageLink("https://cline_pass.example/usage".to_string())
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Char('C'))),
        Action::CopyUsageLink("https://chatgpt_codex.example/usage".to_string())
    );
    assert!(app.usage_page.is_some(), "the page stays open");

    let mut unlinked = App::new();
    let mut lone = target("custom", "Custom", true);
    lone.dashboard = None;
    unlinked.open_usage_page(vec![lone]);
    assert_eq!(unlinked.on_key(key(KeyCode::Char('c'))), Action::None);
}

#[test]
fn esc_and_ctrl_c_close_the_page() {
    for close in [key(KeyCode::Esc), ctrl('c')] {
        let (mut app, _) = open_reading();
        assert_eq!(app.on_key(close), Action::CloseUsagePage);
        assert!(app.usage_page.is_none());
        assert_eq!(app.view, View::Conversation, "Ctrl+C never quits from here");
    }
}

#[test]
fn every_other_key_is_swallowed() {
    let (mut app, _) = open_reading();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char('/'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
    }
    assert_eq!(app.on_key(ctrl('r')), Action::None, "no search over it");
    assert!(app.input.is_empty(), "nothing reached the composer");
    assert!(app.usage_page.is_some());
    assert_eq!(page(&app).selected, 0);
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn ctrl_o_and_ctrl_d_open_their_overlays_over_the_page() {
    // The `/context` page's rule: the two read-only views are a look, and
    // the page waits behind them.
    let (mut app, _) = open_reading();
    app.record_user_message("an earlier message");
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput);
    assert!(
        app.usage_page.is_some(),
        "the page waits behind the overlay"
    );
    assert!(
        !app.overlay_esc_backtracks(),
        "an Esc in the overlay goes back to the page, never into a rewind"
    );
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::Conversation);
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
}

#[test]
fn the_page_blanks_the_ctrl_b_hint_it_would_swallow() {
    let mut app = App::new();
    app.set_command_elapsed(Some(std::time::Duration::from_secs(9)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_usage_page(three());
    assert_eq!(app.background_hint_elapsed(), None);
}

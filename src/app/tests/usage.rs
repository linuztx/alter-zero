//! The `/usage` page (`docs/usage.md`).

use super::*;

use crate::llm::usage::{Limit, MeterValue, UsageMeter, UsageReport};

fn target(provider: &str, name: &str, in_use: bool) -> UsageTarget {
    UsageTarget {
        provider: provider.to_string(),
        name: name.to_string(),
        in_use,
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
            }),
        }],
        ..UsageReport::default()
    }
}

/// An open page whose first round over [`three`] has begun.
fn open_reading() -> (App, u64) {
    let mut app = App::new();
    app.open_usage_page();
    let round = app.begin_usage_round(three()).expect("the page is open");
    (app, round)
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
fn slash_usage_opens_the_page_checking_idle_and_mid_turn() {
    for mid_turn in [false, true] {
        let mut app = App::new();
        if mid_turn {
            app.begin_stream();
        }
        app.shortcuts_open = true;
        type_str(&mut app, "/usage");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::OpenUsagePage,
            "mid-turn {mid_turn}"
        );
        let page = page(&app);
        assert!(!page.ready, "checking until the boundary says what to read");
        assert!(page.sections.is_empty());
        assert!(app.input.is_empty(), "the command token is consumed");
        assert!(app.command_menu.is_none());
        assert!(!app.shortcuts_open, "the bands sharing the composer close");
    }
}

// ===== rounds =====

#[test]
fn a_round_puts_the_subscription_in_use_first_and_reads_them_all() {
    let (app, round) = open_reading();
    assert!(round > 0);
    assert_eq!(
        providers(&app),
        ["cline_pass", "chatgpt_codex", "github_copilot"]
    );
    let page = page(&app);
    assert!(page.ready);
    assert_eq!(page.round, round);
    assert!(page.loading());
    assert!(page.sections.iter().all(|section| section.loading));
    assert_eq!(page.selected, 0, "the highlight opens on the one in use");
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
    app.open_usage_page();
    let third = app.begin_usage_round(three()).unwrap();
    assert!(third > second);
    app.finish_usage_fetch(second, "github_copilot", Ok(report(1.0)), 5);
    assert_eq!(section(&app, "github_copilot").report, None);
}

#[test]
fn a_refresh_keeps_every_report_until_its_replacement_lands() {
    let (mut app, round) = open_reading();
    for provider in ["chatgpt_codex", "cline_pass", "github_copilot"] {
        app.finish_usage_fetch(round, provider, Ok(report(10.0)), 100);
    }
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
fn no_subscription_is_a_ready_page_with_nothing_to_read() {
    let mut app = App::new();
    app.open_usage_page();
    app.begin_usage_round(Vec::new()).unwrap();
    let page = page(&app);
    assert!(page.ready);
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
    app.open_usage_page();
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
    // Before the boundary has said what there is, there is nothing to re-read.
    let mut app = App::new();
    app.open_usage_page();
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), Action::None);
}

#[test]
fn r_refreshes_a_page_with_no_subscription_too() {
    // A sign-in made in another terminal shows up without reopening.
    let mut app = App::new();
    app.open_usage_page();
    app.begin_usage_round(Vec::new());
    assert_eq!(
        app.on_key(key(KeyCode::Char('r'))),
        Action::RefreshUsagePage
    );
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
    assert!(app.input.is_empty(), "nothing reached the composer");
    assert!(app.usage_page.is_some());
    assert_eq!(page(&app).selected, 0);
    assert_eq!(app.on_key(ctrl('d')), Action::None, "no overlay over it");
    assert_eq!(app.view, View::Conversation);
}

#[test]
fn the_page_blanks_the_ctrl_b_hint_it_would_swallow() {
    let mut app = App::new();
    app.set_command_elapsed(Some(std::time::Duration::from_secs(9)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_usage_page();
    assert_eq!(app.background_hint_elapsed(), None);
}

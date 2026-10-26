//! The read-only `/usage` page (`docs/usage.md`).

use super::*;
use crate::app::{UsageEntry, UsageState};
use crate::llm::plan_usage::{Allowance, Balance, Limit, Meter, Note, PlanUsage, Reset};
use crate::ui::theme::{
    error_color, model_meta_color, model_selected_color, usage_full_color, usage_ok_color,
    usage_warn_color,
};
use crate::ui::usage_view::{bar_cells, percent_text, reset_text, usage_menu_rows};

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// When the test page asked: 2026-10-26 02:25 UTC, a Monday.
const ASKED: i64 = 1_792_981_500;

fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A Pro seat a third through its week, with credits.
fn chatgpt() -> PlanUsage {
    PlanUsage {
        plan: Some("Pro".to_string()),
        account: Some("dev@example.com".to_string()),
        limits: vec![Limit::new(
            "Weekly limit",
            // Thursday 10:38 AM UTC.
            Allowance::Metered(Meter::new(34.0, Some(Reset::At(1_793_270_280)), None)),
        )],
        balances: vec![Balance {
            label: "Credits".to_string(),
            value: "62,500".to_string(),
            detail: Some("≈15,625–81,250 messages".to_string()),
        }],
        notes: Vec::new(),
    }
}

/// A Copilot Pro seat two-thirds through its premium requests.
fn copilot() -> PlanUsage {
    PlanUsage {
        plan: Some("Pro".to_string()),
        limits: vec![
            Limit::new(
                "Premium requests",
                Allowance::Metered(Meter::new(
                    68.83,
                    Some(Reset::On(date(2026, 11, 1))),
                    Some("93.5 of 300 left".to_string()),
                )),
            ),
            Limit::new("Chat messages", Allowance::Unlimited),
        ],
        ..PlanUsage::default()
    }
}

/// A ClinePass account at its weekly cap — the live account's shape.
fn clinepass() -> PlanUsage {
    PlanUsage {
        plan: Some("Cline Pass (Monthly)".to_string()),
        limits: vec![
            Limit::new(
                "5-hour limit",
                Allowance::Metered(Meter::new(0.0, None, Some("$10 cap".to_string()))),
            ),
            Limit::new(
                "Weekly limit",
                // Tuesday 2:36 PM UTC — the day after the page asked.
                Allowance::Metered(Meter::new(
                    100.0,
                    Some(Reset::At(1_793_111_760)),
                    Some("$25 cap".to_string()),
                )),
            ),
            Limit::new(
                "Monthly limit",
                Allowance::Metered(Meter::new(
                    81.0,
                    Some(Reset::At(1_794_489_960)),
                    Some("$50 cap".to_string()),
                )),
            ),
        ],
        ..PlanUsage::default()
    }
}

/// The three subscriptions, ChatGPT in use, the page open over them with
/// nothing in yet; and the generation their answers carry.
fn opened() -> (App, u64) {
    let mut app = with_session();
    let generation = app.open_usage_page(
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
            UsageEntry::new(
                "cline_pass",
                "ClinePass",
                false,
                Some("https://app.cline.bot/dashboard/subscription?personal=true"),
            ),
        ],
        ASKED,
        0,
    );
    (app, generation)
}

/// The page with every answer in.
fn open_app() -> App {
    let (mut app, generation) = opened();
    app.set_usage_result(generation, "chatgpt_codex", Ok(chatgpt()), ASKED);
    app.set_usage_result(generation, "github_copilot", Ok(copilot()), ASKED);
    app.set_usage_result(generation, "cline_pass", Ok(clinepass()), ASKED);
    app
}

fn page_texts(app: &App, width: u16, height: u16) -> Vec<String> {
    usage_view_lines(app, width, height)
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

fn select(app: &mut App, index: usize) {
    app.usage_page.as_mut().expect("open").selected = index;
}

/// The row whose text contains `needle`.
fn line_with<'a>(lines: &'a [Line<'static>], needle: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|line| plain(line).contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is on the page"))
}

/// The style of the span in `line` whose text contains `needle`.
fn span_style(line: &Line<'static>, needle: &str) -> Style {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} in {:?}", plain(line)))
        .style
}

// ===== the frame =====

#[test]
fn closed_builds_nothing() {
    assert!(usage_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(usage_page_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_reads_top_to_bottom_at_eighty_columns() {
    // The bar yields columns so the longest trailer still fits its row, the
    // shares line up down the page, and the highlighted subscription's
    // dashboard sits over the hint.
    let texts = page_texts(&open_app(), 80, TALL);
    let rule = "─".repeat(80);
    let header = format!("{:<74}in use", "  ❯ ChatGPT Codex  Pro · dev@example.com");
    let expected = [
        rule.as_str(),
        "  Subscription usage",
        "  3 subscriptions · as of 2:25 AM",
        "",
        header.as_str(),
        "      Weekly limit      ███░░░░░░░   34% used  resets Thu 10:38 AM",
        "      Credits           62,500  ≈15,625–81,250 messages",
        "",
        "    GitHub Copilot  Pro",
        "      Premium requests  ███████░░░   68% used  93.5 of 300 left · resets Nov 1",
        "      Chat messages     Unlimited",
        "",
        "    ClinePass  Cline Pass (Monthly)",
        "      5-hour limit      ░░░░░░░░░░    0% used  $10 cap",
        "      Weekly limit      ██████████  100% used  $25 cap · resets tomorrow 2:36 PM",
        "      Monthly limit     ████████░░   81% used  $50 cap · resets Nov 12",
        "",
        "  Details: https://chatgpt.com/settings/usage",
        "",
        "  r refresh  ↑↓ select  esc close",
        rule.as_str(),
    ];
    assert_eq!(texts, expected, "{texts:#?}");
    let lines = usage_view_lines(&open_app(), 80, TALL);
    assert!(
        wears_the_title_dress(&lines[1]),
        "the title opens the page in the pages' title dress"
    );
}

#[test]
fn a_wide_terminal_gives_the_bar_its_whole_length() {
    let texts = page_texts(&open_app(), 120, TALL);
    assert!(
        texts.contains(&format!(
            "      Weekly limit      {}{}   34% used  resets Thu 10:38 AM",
            "█".repeat(7),
            "░".repeat(13)
        )),
        "a 20-cell bar: {texts:#?}"
    );
}

#[test]
fn the_frame_hugs_its_content_at_every_width_and_height() {
    let mut app = open_app();
    for width in [30u16, 44, 60, 80, 120] {
        for selected in 0..3 {
            select(&mut app, selected);
            for height in 1..=40u16 {
                let what = format!("width {width} height {height} selected {selected}");
                let page = usage_view_lines(&app, width, height);
                let texts: Vec<String> = page
                    .iter()
                    .map(|line| plain(line).trim_end().to_string())
                    .collect();
                assert_frame_hugs_content(&what, &page);
                for text in &texts {
                    assert!(
                        crate::ui::wrap::cols(text) <= usize::from(width),
                        "{what}: a row wider than the terminal: {text:?}"
                    );
                }
            }
        }
    }
}

// ===== the rows =====

#[test]
fn the_share_is_floored_and_never_claims_a_spent_limit_early() {
    assert_eq!(percent_text(0.0), "0%");
    assert_eq!(percent_text(0.3), "<1%", "a sliver is not nothing");
    assert_eq!(percent_text(1.0), "1%");
    assert_eq!(percent_text(34.9), "34%");
    assert_eq!(percent_text(99.6), "99%", "not spent until it is");
    assert_eq!(percent_text(100.0), "100%");
}

#[test]
fn the_bar_shows_a_sliver_and_never_fills_before_the_limit_is_spent() {
    assert_eq!(bar_cells(0.0, 20), 0);
    assert_eq!(bar_cells(0.3, 20), 1, "a sliver shows");
    assert_eq!(bar_cells(50.0, 20), 10);
    assert_eq!(bar_cells(99.6, 20), 19, "never full before it is spent");
    assert_eq!(bar_cells(100.0, 20), 20);
    assert_eq!(bar_cells(50.0, 0), 0);
}

#[test]
fn the_bar_and_the_share_turn_amber_at_three_quarters_and_red_at_ninety() {
    for (used, colour) in [
        (74.9, usage_ok_color()),
        (75.0, usage_warn_color()),
        (89.9, usage_warn_color()),
        (90.0, usage_full_color()),
        (100.0, usage_full_color()),
    ] {
        let (mut app, generation) = opened();
        app.set_usage_result(
            generation,
            "chatgpt_codex",
            Ok(PlanUsage {
                limits: vec![Limit::new(
                    "Weekly limit",
                    Allowance::Metered(Meter::new(used, None, None)),
                )],
                ..PlanUsage::default()
            }),
            ASKED,
        );
        let lines = usage_view_lines(&app, 80, TALL);
        let row = line_with(&lines, "Weekly limit");
        assert_eq!(span_style(row, "█").fg, Some(colour), "{used}% bar");
        assert_eq!(span_style(row, "%").fg, Some(colour), "{used}% share");
    }
}

#[test]
fn reset_times_are_absolute_and_worded_by_how_far_off_they_are() {
    // Asked on Monday 2026-10-26 at 2:25 AM, in UTC.
    let at = |instant| reset_text(Reset::At(instant), ASKED, 0);
    assert_eq!(at(1_793_025_360), "resets 2:36 PM", "the same day");
    assert_eq!(at(1_793_111_760), "resets tomorrow 2:36 PM");
    assert_eq!(at(1_793_270_280), "resets Thu 10:38 AM", "within the week");
    assert_eq!(at(1_793_586_300), "resets Nov 2", "a week off: the date");
    assert_eq!(at(1_794_489_960), "resets Nov 12");
    assert_eq!(
        at(1_798_934_400),
        "resets Jan 3, 2027",
        "another year names it"
    );
    assert_eq!(at(ASKED - 60), "resets now", "a reset already past");
    let on = |day| reset_text(Reset::On(day), ASKED, 0);
    assert_eq!(on(date(2026, 11, 1)), "resets Nov 1");
    assert_eq!(on(date(2026, 10, 26)), "resets today");
    assert_eq!(on(date(2026, 10, 27)), "resets tomorrow");
    assert_eq!(on(date(2026, 10, 20)), "resets now");
}

#[test]
fn reset_times_are_in_the_local_clock() {
    // Four hours west of UTC the page asked on Sunday evening, so a reset
    // at Monday 2:36 PM UTC is tomorrow at 10:36 AM there.
    assert_eq!(
        reset_text(Reset::At(1_793_025_360), ASKED, -4 * 3_600),
        "resets tomorrow 10:36 AM"
    );
    // And the subtitle's `as of` is the same clock's.
    let mut app = with_session();
    let generation = app.open_usage_page(
        vec![UsageEntry::new(
            "chatgpt_codex",
            "ChatGPT Codex",
            true,
            None,
        )],
        ASKED,
        2 * 3_600,
    );
    app.set_usage_result(generation, "chatgpt_codex", Ok(chatgpt()), ASKED);
    let texts = page_texts(&app, 80, TALL);
    assert_eq!(texts[2], "  1 subscription · as of 4:25 AM");
    assert!(
        texts.iter().any(|t| t.ends_with("resets Thu 12:38 PM")),
        "{texts:#?}"
    );
}

#[test]
fn the_subtitle_says_how_old_the_numbers_are() {
    let (mut app, generation) = opened();
    assert_eq!(
        page_texts(&app, 80, TALL)[2],
        "  3 subscriptions · fetching…"
    );
    app.set_usage_result(generation, "chatgpt_codex", Ok(chatgpt()), ASKED);
    app.set_usage_result(generation, "github_copilot", Ok(copilot()), ASKED);
    app.set_usage_result(generation, "cline_pass", Ok(clinepass()), ASKED);
    assert_eq!(
        page_texts(&app, 80, TALL)[2],
        "  3 subscriptions · as of 2:25 AM"
    );
    app.begin_usage_refresh(ASKED + 600);
    assert_eq!(
        page_texts(&app, 80, TALL)[2],
        "  3 subscriptions · refreshing…"
    );
    let texts = page_texts(&app, 80, TALL);
    assert!(
        texts.iter().any(|t| t.contains("34% used")),
        "the numbers stay while refreshing"
    );

    let mut one = with_session();
    one.open_usage_page(
        vec![UsageEntry::new(
            "chatgpt_codex",
            "ChatGPT Codex",
            true,
            None,
        )],
        ASKED,
        0,
    );
    assert_eq!(
        page_texts(&one, 80, TALL)[2],
        "  1 subscription · fetching…"
    );
}

#[test]
fn a_fetching_row_says_so_and_a_failed_one_says_why_in_red() {
    let (mut app, generation) = opened();
    app.set_usage_result(
        generation,
        "github_copilot",
        Err("GitHub Copilot refused the sign-in — /login signs in again.".to_string()),
        ASKED,
    );
    let lines = usage_view_lines(&app, 80, TALL);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.contains(&"      Fetching usage…".to_string()),
        "{texts:#?}"
    );
    let failed = line_with(&lines, "refused the sign-in");
    assert_eq!(
        plain(failed).trim_end(),
        "      ✗ GitHub Copilot refused the sign-in — /login signs in again."
    );
    assert_eq!(span_style(failed, "refused").fg, Some(error_color()));
    assert_eq!(
        span_style(line_with(&lines, "Fetching usage"), "Fetching").fg,
        Some(model_meta_color())
    );
}

#[test]
fn a_long_failure_wraps_under_its_mark() {
    let (mut app, generation) = opened();
    app.set_usage_result(
        generation,
        "chatgpt_codex",
        Err("could not reach chatgpt.com: error sending request: connection refused".to_string()),
        ASKED,
    );
    let texts = page_texts(&app, 50, TALL);
    let at = texts
        .iter()
        .position(|t| t.starts_with("      ✗ could not reach"))
        .expect("the failure row");
    assert!(
        texts[at + 1].starts_with("        ") && !texts[at + 1].trim().is_empty(),
        "the reason continues under the text, not the mark: {texts:#?}"
    );
}

#[test]
fn notes_close_the_block_warnings_marked_in_amber() {
    let (mut app, generation) = opened();
    let mut usage = chatgpt();
    usage.notes = vec![
        Note::warning("A limit is reached — requests wait for it to reset."),
        Note::info("Billed per token to your API organization."),
    ];
    app.set_usage_result(generation, "chatgpt_codex", Ok(usage), ASKED);
    let lines = usage_view_lines(&app, 100, TALL);
    let warning = line_with(&lines, "A limit is reached");
    assert_eq!(
        plain(warning).trim_end(),
        "      ⚠ A limit is reached — requests wait for it to reset."
    );
    assert_eq!(span_style(warning, "A limit").fg, Some(usage_warn_color()));
    let info = line_with(&lines, "Billed per token");
    assert_eq!(
        plain(info).trim_end(),
        "      Billed per token to your API organization."
    );
    assert_eq!(span_style(info, "Billed").fg, Some(model_meta_color()));
}

#[test]
fn a_named_bucket_is_a_heading_over_its_own_rows() {
    let (mut app, generation) = opened();
    let mut usage = chatgpt();
    usage.balances.clear();
    usage.limits.push(Limit {
        scope: Some("GPT-5.3-Codex-Spark".to_string()),
        label: "5-hour limit".to_string(),
        allowance: Allowance::Metered(Meter::new(88.0, None, None)),
    });
    app.set_usage_result(generation, "chatgpt_codex", Ok(usage), ASKED);
    let texts = page_texts(&app, 100, TALL);
    let heading = texts
        .iter()
        .position(|t| t == "      GPT-5.3-Codex-Spark")
        .expect("the bucket's heading");
    assert!(
        texts[heading + 1].starts_with("        5-hour limit"),
        "its rows sit under it: {texts:#?}"
    );
    let share = |t: &str| t.find("% used");
    assert_eq!(
        share(&texts[heading + 1]),
        share(texts.iter().find(|t| t.contains("Weekly limit")).unwrap()),
        "the shares stay in one column"
    );
}

#[test]
fn a_trailer_that_does_not_fit_drops_under_the_bar() {
    let texts = page_texts(&open_app(), 52, TALL);
    let at = texts
        .iter()
        .position(|t| t.starts_with("      Premium requests"))
        .expect("the premium row");
    assert!(!texts[at].contains("left"), "the trailer moved: {texts:#?}");
    let bar_column = texts[at].find('█').expect("a bar");
    let next = &texts[at + 1];
    assert!(
        next.trim_start().starts_with("93.5 of 300 left"),
        "{texts:#?}"
    );
    assert_eq!(
        crate::ui::wrap::cols(&next[..next.len() - next.trim_start().len()]),
        crate::ui::wrap::cols(&texts[at][..bar_column]),
        "aligned with the bar"
    );
}

#[test]
fn a_narrow_terminal_drops_the_bar_and_keeps_the_share() {
    let texts = page_texts(&open_app(), 34, TALL);
    let row = texts
        .iter()
        .find(|t| t.contains("Weekly limit"))
        .expect("the row");
    assert!(!row.contains('█') && !row.contains('░'), "{texts:#?}");
    assert!(row.contains("34% used"), "{texts:#?}");
}

#[test]
fn the_highlighted_block_wears_the_marker_in_the_accent() {
    let mut app = open_app();
    select(&mut app, 2);
    let lines = usage_view_lines(&app, 80, TALL);
    let header = line_with(&lines, "ClinePass");
    assert!(
        plain(header).starts_with("  ❯ ClinePass"),
        "{:?}",
        plain(header)
    );
    assert_eq!(
        span_style(header, "ClinePass").fg,
        Some(model_selected_color())
    );
    assert!(
        line_with(&lines, "ChatGPT Codex").spans[1]
            .content
            .trim()
            .is_empty(),
        "the others wear no marker"
    );
    assert_eq!(
        plain(line_with(&lines, "Details:")).trim_end(),
        "  Details: https://app.cline.bot/dashboard/subscription?personal=true",
        "the dashboard follows the highlight"
    );
}

#[test]
fn the_dashboard_is_a_link_the_terminal_can_open() {
    let lines = usage_view_lines(&open_app(), 80, TALL);
    let details = line_with(&lines, "Details:");
    let url = span_style(details, "https://chatgpt.com");
    assert!(url.add_modifier.contains(Modifier::UNDERLINED), "{url:?}");
}

#[test]
fn a_narrow_terminal_drops_the_account_before_the_plan() {
    let texts = page_texts(&open_app(), 44, TALL);
    let header = texts
        .iter()
        .find(|t| t.contains("ChatGPT Codex"))
        .expect("the header");
    assert!(
        crate::ui::wrap::cols(header) <= 44 && header.contains("Pro"),
        "{texts:#?}"
    );
    assert!(
        !header.contains("dev@example.com"),
        "the account goes first"
    );
}

// ===== a short terminal =====

#[test]
fn a_short_terminal_collapses_the_other_subscriptions_to_their_tightest_limit() {
    let app = open_app();
    let whole = page_texts(&app, 80, TALL).len() as u16;
    let texts = page_texts(&app, 80, whole - 1);
    assert!(
        texts.contains(&"    GitHub Copilot  Pro · Premium requests 68% used".to_string()),
        "{texts:#?}"
    );
    assert!(
        texts.contains(&"    ClinePass  Cline Pass (Monthly) · Weekly limit 100% used".to_string()),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("34% used")),
        "the highlighted block stays whole: {texts:#?}"
    );
    assert!(texts.iter().any(|t| t == "  Subscription usage"));
}

#[test]
fn the_shortest_page_is_the_highlighted_subscription_alone() {
    // Its own columns: the bar sized to its own longest trailer.
    let mut app = open_app();
    select(&mut app, 1);
    let rule = "─".repeat(80);
    assert_eq!(
        page_texts(&app, 80, 8),
        [
            rule.as_str(),
            "  ❯ GitHub Copilot  Pro",
            "      Premium requests  ████████░░░░   68% used  93.5 of 300 left · resets Nov 1",
            "      Chat messages     Unlimited",
            "",
            "  r refresh  ↑↓ select  esc close",
            rule.as_str(),
        ]
    );
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_app();
    let lines = usage_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(usage_menu_rows(&app, 80, TALL), lines);
    assert_eq!(usage_page_height(&app, 80, 200), Some(lines));
    assert_eq!(usage_page_height(&app, 80, 6), Some(6));
}

#[test]
fn render_paints_the_page_and_the_cursor_hides_seated_on_the_marker() {
    let app = open_app();
    let height = usage_page_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_usage_page(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 80).starts_with('─'));
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    assert!(painted.iter().any(|r| r.contains("Subscription usage")));
    assert!(
        !cursor_visible(&app),
        "a read-only page has nothing for a cursor to point at"
    );
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the highlighted block wears the marker");
    assert_eq!(
        cursor_position(Rect::new(0, 0, 80, height), &app),
        (2, marker_row)
    );
}

#[test]
fn a_page_that_fits_never_flows() {
    let app = open_app();
    for height in [8u16, 12, 20, TALL] {
        assert!(
            crate::ui::view_flow(&app, 80, height, 1000).is_none(),
            "{height} rows"
        );
    }
}

#[test]
fn the_fetching_page_is_one_the_state_test_can_read() {
    // A guard on the fixture: nothing is in until the answers land.
    let (app, _) = opened();
    assert!(
        app.usage_page
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .all(|e| e.state == UsageState::Fetching)
    );
}

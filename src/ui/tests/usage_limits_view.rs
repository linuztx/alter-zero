//! The read-only `/usage` page (`docs/usage-limits.md`).

use super::*;
use crate::app::SubscriptionEntry;
use crate::llm::usage_limits::{LimitNote, LimitWindow, PlanLimits, pay_as_you_go};
use crate::ui::theme::{ask_warning_color, error_color, model_selected_color};
use crate::ui::usage_limits_view::{
    age_text, countdown, filled_cells, reset_moment, share_text, usage_limits_menu_rows,
};
use crate::ui::wrap::cols;

/// The page's clock: Sunday 2026-10-25, 14:00 UTC.
const NOW: i64 = 1_792_936_800;

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn window(label: &str, used: f64, resets_in: Option<i64>, detail: Option<&str>) -> LimitWindow {
    LimitWindow {
        label: label.to_string(),
        used_percent: used,
        resets_at: resets_in.map(|seconds| NOW + seconds),
        detail: detail.map(str::to_string),
        reached: used >= 100.0,
    }
}

fn note(label: &str, value: &str, detail: Option<&str>) -> LimitNote {
    LimitNote {
        label: label.to_string(),
        value: value.to_string(),
        detail: detail.map(str::to_string),
    }
}

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

fn chatgpt() -> PlanLimits {
    PlanLimits {
        plan: Some("Pro".to_string()),
        windows: vec![window(
            "Weekly limit",
            12.0,
            Some(6 * DAY + 18 * HOUR),
            None,
        )],
        notes: vec![note(
            "Credits",
            "62,500 credits",
            Some("≈15.6k–81.3k local messages"),
        )],
        ..PlanLimits::default()
    }
}

fn clinepass() -> PlanLimits {
    PlanLimits {
        plan: Some("Monthly".to_string()),
        windows: vec![
            window("5-hour limit", 0.0, None, None),
            window("Weekly limit", 100.0, Some(2 * DAY), None),
            window("Monthly limit", 81.0, Some(18 * DAY), None),
        ],
        ..PlanLimits::default()
    }
}

fn copilot() -> PlanLimits {
    PlanLimits {
        plan: Some("Pro".to_string()),
        windows: vec![window(
            "Premium requests",
            49.4,
            Some(18 * DAY),
            Some("148 of 300 requests"),
        )],
        notes: vec![
            note("Chat", "Included", None),
            note("Code completions", "Included", None),
        ],
        ..PlanLimits::default()
    }
}

fn entry(id: &str, name: &str, active: bool) -> SubscriptionEntry {
    SubscriptionEntry {
        id: id.to_string(),
        name: name.to_string(),
        active,
        dashboard: Some(format!("https://example.com/{id}/usage")),
    }
}

/// The page open over four subscriptions — ChatGPT in use — every answer in,
/// fetched ten seconds ago.
fn open_app() -> App {
    let mut app = with_session();
    app.open_usage_limits(vec![
        entry("chatgpt_codex", "ChatGPT Codex", true),
        entry("cline_pass", "ClinePass", false),
        entry("github_copilot", "GitHub Copilot", false),
        entry("anthropic_console", "Anthropic Console", false),
    ]);
    let generation = app.begin_usage_limits_refresh(NOW - 30);
    app.set_plan_limits("chatgpt_codex", generation, Ok(chatgpt()), NOW - 10);
    app.set_plan_limits("cline_pass", generation, Ok(clinepass()), NOW - 10);
    app.set_plan_limits("github_copilot", generation, Ok(copilot()), NOW - 10);
    app.set_plan_limits(
        "anthropic_console",
        generation,
        Ok(pay_as_you_go()),
        NOW - 10,
    );
    app.set_usage_limits_now(NOW);
    app
}

/// The page with the highlight on section `selected`.
fn open_at(selected: usize) -> App {
    let mut app = open_app();
    app.usage_limits.as_mut().expect("open").selected = selected;
    app
}

fn texts(app: &App, width: u16, height: u16) -> Vec<String> {
    crate::ui::usage_limits_lines(app, width, height)
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

/// The row that starts (after its inset) with `text`.
fn row_starting<'a>(rows: &'a [String], text: &str) -> &'a str {
    rows.iter()
        .find(|row| row.trim_start().starts_with(text))
        .unwrap_or_else(|| panic!("no row starts with {text:?}: {rows:#?}"))
}

/// The rows of section `name`: its header to the next blank row.
fn section<'a>(rows: &'a [String], name: &str) -> Vec<&'a str> {
    let start = rows
        .iter()
        .position(|row| {
            row.trim_start()
                .trim_start_matches('❯')
                .trim_start()
                .starts_with(name)
        })
        .unwrap_or_else(|| panic!("no section {name:?}: {rows:#?}"));
    rows[start..]
        .iter()
        .take_while(|row| !row.is_empty())
        .map(String::as_str)
        .collect()
}

// ===== the words =====

#[test]
fn a_share_reads_as_a_whole_percent_never_rounding_to_none_or_all() {
    assert_eq!(share_text(0.0), "0%");
    assert_eq!(share_text(0.4), "<1%");
    assert_eq!(share_text(27.4), "27%");
    assert_eq!(share_text(27.5), "28%");
    assert_eq!(share_text(99.6), "99%");
    assert_eq!(share_text(100.0), "100%");
    assert_eq!(share_text(123.0), "123%");
}

#[test]
fn a_countdown_reads_in_the_two_largest_units() {
    assert_eq!(countdown(30), "<1m");
    assert_eq!(countdown(42 * 60), "42m");
    assert_eq!(countdown(3 * HOUR + 12 * 60), "3h 12m");
    // A part minute rounds up: the window is not back a minute early.
    assert_eq!(countdown(3 * HOUR + 11 * 60 + 1), "3h 12m");
    assert_eq!(countdown(3 * HOUR), "3h");
    assert_eq!(countdown(6 * DAY + 18 * HOUR), "6d 18h");
    assert_eq!(countdown(2 * DAY), "2d");
    // Past a week the days say enough.
    assert_eq!(countdown(18 * DAY + 5 * HOUR), "18d");
}

#[test]
fn a_reset_moment_is_local_time_said_by_how_far_off_it_is() {
    let utc = |_: i64| 0;
    // Within a day: the time.
    assert_eq!(reset_moment(NOW, NOW + 3 * HOUR + 12 * 60, utc), "5:12 PM");
    // Within a week: the weekday and the time.
    assert_eq!(reset_moment(NOW, NOW + 2 * DAY, utc), "Tue 2:00 PM");
    // Beyond: the date.
    assert_eq!(reset_moment(NOW, NOW + 18 * DAY, utc), "Nov 12");
    // The offset in force at the reset is the one applied.
    assert_eq!(
        reset_moment(NOW, NOW + 3 * HOUR + 12 * 60, |_| 2 * 3600),
        "7:12 PM"
    );
}

#[test]
fn an_age_says_how_long_ago_in_one_unit() {
    assert_eq!(age_text(0), "just now");
    assert_eq!(age_text(59), "just now");
    assert_eq!(age_text(3 * 60 + 5), "3m ago");
    assert_eq!(age_text(2 * HOUR), "2h ago");
    assert_eq!(age_text(DAY + 1), "1d ago");
}

#[test]
fn the_bar_fills_with_what_is_used() {
    assert_eq!(filled_cells(0.0, 20), 0);
    assert_eq!(filled_cells(50.0, 20), 10);
    assert_eq!(filled_cells(81.0, 20), 16);
    assert_eq!(filled_cells(100.0, 20), 20);
    assert_eq!(filled_cells(140.0, 20), 20);
    // A sliver is a cell, and a share short of whole never fills the bar.
    assert_eq!(filled_cells(0.4, 20), 1);
    assert_eq!(filled_cells(99.6, 20), 19);
}

// ===== the page =====

#[test]
fn nothing_is_built_while_the_page_is_closed() {
    assert!(crate::ui::usage_limits_lines(&with_session(), 80, TALL).is_empty());
}

#[test]
fn the_page_opens_on_its_title_and_how_fresh_it_is() {
    let app = open_app();
    let lines = crate::ui::usage_limits_lines(&app, 100, TALL);
    assert!(wears_the_title_dress(&lines[1]), "the pages' title dress");
    let rows = texts(&app, 100, TALL);
    assert_eq!(rows[1], "  Usage limits");
    assert_eq!(rows[2], "  4 subscriptions · updated just now");
    let mut later = open_app();
    later.set_usage_limits_now(NOW + 3 * 60);
    assert_eq!(
        texts(&later, 100, TALL)[2],
        "  4 subscriptions · updated 3m ago"
    );
    let mut refreshing = open_app();
    refreshing.begin_usage_limits_refresh(NOW);
    assert_eq!(
        texts(&refreshing, 100, TALL)[2],
        "  4 subscriptions · refreshing…"
    );
}

#[test]
fn each_subscription_is_a_section_under_its_name_and_plan() {
    let rows = texts(&open_app(), 100, TALL);
    let header = row_starting(&rows, "❯ ChatGPT Codex");
    assert!(header.starts_with("  ❯ ChatGPT Codex  Pro"), "{header:?}");
    assert!(header.ends_with("in use"), "the session's own: {header:?}");
    assert_eq!(cols(header), 98, "flush with the page's right inset");
    assert!(rows.iter().any(|r| r == "    ClinePass  Monthly"));
    assert!(rows.iter().any(|r| r == "    GitHub Copilot  Pro"));
    assert!(
        rows.iter()
            .any(|r| r == "    Anthropic Console  Pay as you go")
    );
}

#[test]
fn a_window_reads_its_label_bar_share_and_reset() {
    let rows = texts(&open_app(), 100, TALL);
    let chatgpt = section(&rows, "ChatGPT Codex");
    let weekly = chatgpt[1];
    assert!(weekly.starts_with("      Weekly limit "), "{weekly:?}");
    assert!(weekly.contains(" 12% used  "), "{weekly:?}");
    assert!(
        weekly.ends_with("resets in 6d 18h · Sun 8:00 AM"),
        "{weekly:?}"
    );
}

#[test]
fn every_bar_on_the_page_starts_in_one_column() {
    let rows = texts(&open_app(), 100, TALL);
    let starts: Vec<usize> = rows
        .iter()
        .filter(|row| row.contains(" used"))
        .map(|row| {
            let byte = row.find(['█', '░']).expect("a bar");
            cols(&row[..byte])
        })
        .collect();
    assert_eq!(starts.len(), 5, "every window has a bar: {rows:#?}");
    assert!(
        starts.windows(2).all(|pair| pair[0] == pair[1]),
        "{starts:?}"
    );
}

#[test]
fn a_window_with_no_reset_names_none() {
    let rows = texts(&open_app(), 100, TALL);
    let rolling = row_starting(&rows, "5-hour limit");
    assert!(rolling.ends_with("0% used"), "{rolling:?}");
}

#[test]
fn a_used_up_window_says_so_in_red_before_its_reset() {
    let app = open_app();
    let lines = crate::ui::usage_limits_lines(&app, 100, TALL);
    let rows: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let at = rows
        .iter()
        .position(|row| row.contains("100% used"))
        .expect("the used-up week");
    assert!(
        rows[at].ends_with("100% used  limit reached · resets in 2d · Tue 2:00 PM"),
        "{:?}",
        rows[at]
    );
    let reached = lines[at]
        .spans
        .iter()
        .find(|span| span.content.contains("limit reached"))
        .expect("the reached clause");
    assert_eq!(reached.style.fg, Some(error_color()));
}

#[test]
fn the_bar_and_share_turn_amber_then_red_as_the_window_fills() {
    let ink = |used: f64| {
        let mut app = with_session();
        app.open_usage_limits(vec![entry("cline_pass", "ClinePass", true)]);
        let generation = app.begin_usage_limits_refresh(NOW);
        let limits = PlanLimits {
            windows: vec![window("Weekly limit", used, Some(DAY), None)],
            ..PlanLimits::default()
        };
        app.set_plan_limits("cline_pass", generation, Ok(limits), NOW);
        app.set_usage_limits_now(NOW);
        let lines = crate::ui::usage_limits_lines(&app, 100, TALL);
        let row = lines
            .iter()
            .find(|line| plain(line).contains(" used"))
            .expect("the window row")
            .clone();
        let fill = row
            .spans
            .iter()
            .find(|span| span.content.contains('█'))
            .and_then(|span| span.style.fg);
        let share = row
            .spans
            .iter()
            .find(|span| span.content.ends_with('%'))
            .and_then(|span| span.style.fg);
        (fill, share)
    };
    assert_eq!(
        ink(50.0).0,
        Some(model_selected_color()),
        "room: the accent"
    );
    assert_eq!(
        ink(80.0),
        (Some(ask_warning_color()), Some(ask_warning_color()))
    );
    assert_eq!(ink(95.0), (Some(error_color()), Some(error_color())));
    assert_eq!(ink(100.0), (Some(error_color()), Some(error_color())));
}

#[test]
fn a_windows_counts_sit_under_its_bar() {
    let rows = texts(&open_app(), 100, TALL);
    let copilot = section(&rows, "GitHub Copilot");
    let premium = copilot[1];
    let detail = copilot[2];
    assert_eq!(detail.trim(), "148 of 300 requests");
    let bar_col = cols(&premium[..premium.find(['█', '░']).expect("a bar")]);
    let detail_col = cols(&detail[..detail.find('1').expect("the count")]);
    assert_eq!(bar_col, detail_col, "{premium:?} / {detail:?}");
}

#[test]
fn notes_read_their_label_and_value() {
    let rows = texts(&open_app(), 100, TALL);
    let credits = row_starting(&rows, "Credits");
    assert!(
        credits.ends_with("62,500 credits · ≈15.6k–81.3k local messages"),
        "{credits:?}"
    );
    let chat = row_starting(&rows, "Chat ");
    assert!(chat.ends_with("Included"), "{chat:?}");
    // Every value starts in one column — the bars' own.
    let column = |row: &str, value: &str| cols(&row[..row.find(value).expect("the value")]);
    let weekly = row_starting(&rows, "Weekly limit");
    let bar = cols(&weekly[..weekly.find(['█', '░']).expect("a bar")]);
    assert_eq!(column(credits, "62,500"), bar);
    assert_eq!(column(chat, "Included"), bar);
}

#[test]
fn a_notes_detail_too_long_for_its_row_goes_under_its_value_whole() {
    let rows = texts(&open_app(), 60, TALL);
    let chatgpt = section(&rows, "ChatGPT Codex");
    let credits = chatgpt
        .iter()
        .position(|row| row.trim_start().starts_with("Credits"))
        .expect("the credits row");
    assert!(
        chatgpt[credits].ends_with("62,500 credits"),
        "{:?}",
        chatgpt[credits]
    );
    let detail = chatgpt[credits + 1];
    assert_eq!(detail.trim(), "≈15.6k–81.3k local messages");
    let value = chatgpt[credits].find("62,500").expect("the value");
    assert_eq!(
        cols(&detail[..detail.find('≈').expect("the detail")]),
        cols(&chatgpt[credits][..value]),
        "under its value"
    );
}

#[test]
fn pay_as_you_go_reads_its_remark() {
    let rows = texts(&open_app(), 100, TALL);
    let anthropic = section(&rows, "Anthropic Console");
    assert_eq!(
        anthropic[1],
        "      Billed per token to your API organisation — no plan limits"
    );
}

#[test]
fn a_loading_section_says_so_and_a_failed_one_says_why_in_red() {
    let mut app = with_session();
    app.open_usage_limits(vec![
        entry("chatgpt_codex", "ChatGPT Codex", true),
        entry("github_copilot", "GitHub Copilot", false),
    ]);
    let generation = app.begin_usage_limits_refresh(NOW);
    app.set_plan_limits(
        "github_copilot",
        generation,
        Err("GitHub refused the stored sign-in — run /login to sign in again".to_string()),
        NOW,
    );
    app.set_usage_limits_now(NOW);
    let lines = crate::ui::usage_limits_lines(&app, 100, TALL);
    let rows: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(section(&rows, "ChatGPT Codex")[1], "      Loading…");
    let failed = section(&rows, "GitHub Copilot");
    assert_eq!(
        failed[1],
        "      GitHub refused the stored sign-in — run /login to sign in again"
    );
    let at = rows
        .iter()
        .position(|row| row.contains("refused"))
        .expect("the error");
    assert!(
        lines[at]
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| span.style.fg == Some(error_color()))
    );
}

#[test]
fn a_failed_refresh_keeps_the_last_answer_above_its_error() {
    let mut app = open_app();
    let generation = app.begin_usage_limits_refresh(NOW);
    app.set_plan_limits(
        "chatgpt_codex",
        generation,
        Err("No connection to chatgpt.com".to_string()),
        NOW,
    );
    let rows = texts(&app, 100, TALL);
    let chatgpt = section(&rows, "ChatGPT Codex");
    assert!(chatgpt[1].trim_start().starts_with("Weekly limit"));
    assert_eq!(
        *chatgpt.last().expect("rows"),
        "      No connection to chatgpt.com"
    );
}

#[test]
fn an_error_wraps_rather_than_being_cut() {
    let mut app = with_session();
    app.open_usage_limits(vec![entry("cline_pass", "ClinePass", true)]);
    let generation = app.begin_usage_limits_refresh(NOW);
    let reason = "Cline answered HTTP 500: the usage service is temporarily unavailable, \
                  please try again in a few minutes";
    app.set_plan_limits("cline_pass", generation, Err(reason.to_string()), NOW);
    app.set_usage_limits_now(NOW);
    let rows = texts(&app, 50, TALL);
    let joined: String = section(&rows, "ClinePass")[1..]
        .iter()
        .map(|row| row.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined,
        reason.split_whitespace().collect::<Vec<_>>().join(" ")
    );
}

#[test]
fn the_highlighted_sections_usage_page_is_linked_under_the_sections() {
    let app = open_at(1);
    let lines = crate::ui::usage_limits_lines(&app, 100, TALL);
    let at = lines
        .iter()
        .position(|line| plain(line).contains("Full usage: "))
        .expect("the link row");
    assert_eq!(
        plain(&lines[at]).trim_end(),
        "  Full usage: https://example.com/cline_pass/usage"
    );
    let target = lines[at]
        .spans
        .iter()
        .find_map(|span| crate::links::style_link(&span.style))
        .expect("an OSC 8 link");
    assert_eq!(&*target, "https://example.com/cline_pass/usage");
}

#[test]
fn the_hint_names_the_keys_and_drops_select_for_a_lone_subscription() {
    let rows = texts(&open_app(), 100, TALL);
    assert_eq!(
        rows[rows.len() - 2],
        "  ↑↓ select  r refresh  c copy link  esc close"
    );
    let mut app = with_session();
    app.open_usage_limits(vec![entry("cline_pass", "ClinePass", true)]);
    let rows = texts(&app, 100, TALL);
    assert_eq!(rows[rows.len() - 2], "  r refresh  c copy link  esc close");
}

#[test]
fn the_frame_hugs_its_content_at_every_width_and_height() {
    for selected in 0..4 {
        let app = open_at(selected);
        for width in [30, 50, 80, 120] {
            for height in 9..60 {
                let page = crate::ui::usage_limits_lines(&app, width, height);
                if plain(&page[0]).starts_with('─') && plain(page.last().unwrap()).starts_with('─')
                {
                    assert_frame_hugs_content(
                        &format!("/usage {width}x{height} at {selected}"),
                        &page,
                    );
                }
            }
        }
    }
}

#[test]
fn a_short_terminal_folds_the_other_sections_to_their_tightest_window() {
    let app = open_app();
    let whole = crate::ui::usage_limits_lines(&app, 100, TALL).len();
    // A few rows short of the whole page: the accordion.
    let rows = texts(&app, 100, u16::try_from(whole).unwrap() - 6);
    assert!(rows.len() <= whole - 6, "it fits: {rows:#?}");
    // The highlighted section keeps its windows…
    assert!(
        rows.iter()
            .any(|r| r.trim_start().starts_with("Weekly limit"))
    );
    // …and every other folds to its header, its tightest window beside it.
    assert!(
        rows.iter()
            .any(|r| r == "    ClinePass  Monthly · Weekly limit 100%"),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r == "    GitHub Copilot  Pro · Premium requests 49%"),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r == "    Anthropic Console  Pay as you go")
    );
    assert!(
        !rows.iter().any(|r| r.contains("148 of 300")),
        "folded away"
    );
}

#[test]
fn the_compact_page_windows_the_folded_sections_under_one_title_row() {
    let app = open_at(2);
    let rows = texts(&app, 100, 9);
    assert_eq!(rows.len(), 9);
    assert_eq!(rows[1], "  Usage limits · updated just now");
    // Three rows riding the highlight between the overflow slots.
    assert_eq!(rows[2], "  ↑ 1 more above");
    assert_eq!(rows[3], "    ClinePass  Monthly · Weekly limit 100%");
    assert!(rows[4].starts_with("  ❯ GitHub Copilot  Pro · Premium requests 49%"));
    assert_eq!(rows[5], "    Anthropic Console  Pay as you go");
    assert_eq!(rows[6], "");
    assert_eq!(rows[7], "  ↑↓ select  r refresh  c copy link  esc close");
}

#[test]
fn every_page_of_the_ladder_is_still_titled() {
    // A short terminal sheds rows, never the page's name: the accordion
    // keeps at least the one-row title, and the compact pages anchor on it.
    for selected in 0..4 {
        let app = open_at(selected);
        for height in 1..60 {
            let rows = texts(&app, 100, height);
            assert!(
                rows.iter().any(|row| row.starts_with("  Usage limits")),
                "untitled at {height} rows, section {selected}: {rows:#?}"
            );
        }
    }
}

#[test]
fn the_page_never_flows_however_short_the_terminal() {
    let app = open_app();
    for height in 1..60 {
        assert!(
            crate::ui::view_flow(&app, 80, height, 1000).is_none(),
            "flowed at {height} rows"
        );
        let built = crate::ui::usage_limits_lines(&app, 80, height).len();
        assert!(built <= usize::from(height), "{built} rows at {height}");
    }
}

#[test]
fn no_row_is_wider_than_the_terminal() {
    let app = open_app();
    for width in 2..=120 {
        for height in [5, 12, TALL] {
            for line in crate::ui::usage_limits_lines(&app, width, height) {
                assert!(
                    cols(&plain(&line)) <= usize::from(width),
                    "{:?} at {width}",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn a_narrow_terminal_drops_the_clock_then_puts_the_reset_under_the_bar() {
    let app = open_app();
    // Narrower than the full reset clauses allow: the relative part alone.
    let rows = texts(&app, 80, TALL);
    let weekly = section(&rows, "ChatGPT Codex")[1];
    assert!(weekly.ends_with("resets in 6d 18h"), "{weekly:?}");
    // Narrower still: the reset under the bar, still whole.
    let rows = texts(&app, 50, TALL);
    let chatgpt = section(&rows, "ChatGPT Codex");
    assert!(chatgpt[1].ends_with("12% used"), "{:?}", chatgpt[1]);
    assert_eq!(chatgpt[2].trim(), "resets in 6d 18h");
}

#[test]
fn the_cursor_hides_and_seats_on_the_highlighted_section() {
    let app = open_at(2);
    assert!(!cursor_visible(&app));
    let rows = usage_limits_menu_rows(&app, 100, 60);
    let area = Rect::new(0, 0, 100, rows);
    let lines = crate::ui::usage_limits_lines(&app, 100, rows);
    let marker = lines
        .iter()
        .position(|line| plain(line).starts_with("  ❯ "))
        .expect("the marker row");
    assert_eq!(
        cursor_position(area, &app),
        (2, u16::try_from(marker).unwrap())
    );
}

#[test]
fn the_height_is_the_page_clamped_to_the_terminal() {
    let app = open_app();
    let whole = u16::try_from(crate::ui::usage_limits_lines(&app, 100, TALL).len()).unwrap();
    assert_eq!(crate::ui::usage_limits_height(&app, 100, 200), Some(whole));
    // A short terminal reserves the page built for it, never more.
    let short = u16::try_from(crate::ui::usage_limits_lines(&app, 100, 8).len()).unwrap();
    assert!(short <= 8);
    assert_eq!(crate::ui::usage_limits_height(&app, 100, 8), Some(short));
    assert_eq!(
        crate::ui::usage_limits_height(&with_session(), 100, 50),
        None
    );
}

#[test]
fn the_painted_page_is_the_built_page() {
    let app = open_app();
    let lines = crate::ui::usage_limits_lines(&app, 100, 60);
    let height = u16::try_from(lines.len()).unwrap();
    let mut buf = buffer(100, height);
    crate::ui::render_usage_limits(buf.area, &mut buf, &app);
    for (y, line) in lines.iter().enumerate() {
        let y = u16::try_from(y).unwrap();
        assert_eq!(row(&buf, y, 100).trim_end(), plain(line).trim_end());
    }
}

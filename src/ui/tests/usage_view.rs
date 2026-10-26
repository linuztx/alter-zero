//! The read-only `/usage` page (`docs/usage.md`).

use super::*;
use crate::app::UsageTarget;
use crate::llm::usage::{Counts, Limit, MeterValue, NoteTone, UsageMeter, UsageNote, UsageReport};
use crate::ui::theme::{
    model_selected_color, usage_empty_color, usage_full_color, usage_meter_color, usage_ok_color,
    usage_warn_color,
};
use crate::ui::usage_view::usage_menu_rows;
use crate::ui::wrap::cols;

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// Sunday 2026-10-25, 15:04 UTC — the clock every test reads.
const NOW: i64 = 1_792_940_640;

fn limit(label: &str, used: f64, resets_at: Option<i64>) -> UsageMeter {
    UsageMeter {
        group: None,
        label: label.to_string(),
        value: MeterValue::Limit(Limit {
            used_percent: used,
            counts: None,
            resets_at,
            reached: used >= 100.0,
        }),
    }
}

fn chatgpt() -> UsageReport {
    UsageReport {
        plan: Some("Pro".to_string()),
        account: Some("dev@example.com".to_string()),
        meters: vec![
            // Saturday 2026-10-31, 15:58 UTC.
            limit("Weekly limit", 7.0, Some(1_793_462_280)),
            UsageMeter {
                group: None,
                label: "Credits".to_string(),
                value: MeterValue::Text {
                    value: "62,500 credits".to_string(),
                    detail: Some("≈ 15.6k–81.3k local messages".to_string()),
                },
            },
        ],
        notes: Vec::new(),
    }
}

fn clinepass() -> UsageReport {
    UsageReport {
        plan: Some("Monthly".to_string()),
        account: None,
        meters: vec![
            limit("5-hour limit", 0.0, None),
            // Tuesday 2026-10-27, 14:36:37 UTC.
            limit("Weekly limit", 100.0, Some(1_793_111_797)),
            // Thursday 2026-11-12, 13:26:58 UTC.
            limit("Monthly limit", 81.0, Some(1_794_490_018)),
        ],
        notes: vec![UsageNote {
            tone: NoteTone::Warning,
            text: "Paused until the weekly limit resets — switch with /model meanwhile."
                .to_string(),
        }],
    }
}

fn copilot() -> UsageReport {
    UsageReport {
        plan: Some("Pro".to_string()),
        account: None,
        meters: vec![
            UsageMeter {
                group: None,
                label: "AI credits".to_string(),
                value: MeterValue::Limit(Limit {
                    used_percent: 42.0,
                    counts: Some(Counts {
                        used: 420.0,
                        total: 1_000.0,
                        unit: Some("credits"),
                    }),
                    // Sunday 2026-11-01, 00:00 UTC.
                    resets_at: Some(1_793_491_200),
                    reached: false,
                }),
            },
            UsageMeter {
                group: None,
                label: "Chat".to_string(),
                value: MeterValue::Unlimited,
            },
        ],
        notes: Vec::new(),
    }
}

fn target(provider: &str, name: &str, in_use: bool) -> UsageTarget {
    UsageTarget {
        provider: provider.to_string(),
        name: name.to_string(),
        in_use,
        dashboard: Some(format!("https://{provider}.example/usage")),
    }
}

fn targets() -> Vec<UsageTarget> {
    vec![
        target("chatgpt_codex", "ChatGPT Codex", true),
        target("cline_pass", "ClinePass", false),
        target("github_copilot", "GitHub Copilot", false),
    ]
}

/// The page open over three subscriptions, every read landed at [`NOW`],
/// the local clock UTC.
fn loaded() -> App {
    let mut app = with_session();
    app.set_local_offset(|_| 0);
    let round = app.open_usage_page(targets()).unwrap();
    app.finish_usage_fetch(round, "chatgpt_codex", Ok(chatgpt()), NOW);
    app.finish_usage_fetch(round, "cline_pass", Ok(clinepass()), NOW);
    app.finish_usage_fetch(round, "github_copilot", Ok(copilot()), NOW);
    app.set_usage_now(NOW);
    app
}

/// [`loaded`] with ChatGPT's report swapped for `report`.
fn with_chatgpt(report: UsageReport) -> App {
    let mut app = loaded();
    let round = app.begin_usage_round(targets()).unwrap();
    app.finish_usage_fetch(round, "chatgpt_codex", Ok(report), NOW);
    app.finish_usage_fetch(round, "cline_pass", Ok(clinepass()), NOW);
    app.finish_usage_fetch(round, "github_copilot", Ok(copilot()), NOW);
    app
}

fn texts(app: &App, width: u16, height: u16) -> Vec<String> {
    usage_view_lines(app, width, height)
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

/// The first row whose text contains `needle`.
fn row_with<'a>(lines: &'a [Line<'static>], needle: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|line| plain(line).contains(needle))
        .unwrap_or_else(|| panic!("no row contains {needle:?}"))
}

/// The foreground of the span reading exactly `content` on `line`.
fn fg_of(line: &Line<'static>, content: &str) -> Option<Color> {
    line.spans
        .iter()
        .find(|span| span.content == content)
        .and_then(|span| span.style.fg)
}

/// A bar's spent and remaining cells on `line`.
fn bar_cells(line: &Line<'static>) -> (usize, usize) {
    let count = |glyph: char| {
        line.spans
            .iter()
            .filter(|span| span.content.starts_with(glyph))
            .map(|span| cols(&span.content))
            .sum()
    };
    (count('█'), count('░'))
}

/// How far `text` is indented.
fn indent(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

// ===== the frame =====

#[test]
fn closed_builds_nothing() {
    assert!(usage_view_lines(&App::new(), 80, TALL).is_empty());
    assert_eq!(usage_page_height(&App::new(), 80, 40), None);
}

#[test]
fn the_title_names_the_page_in_the_pages_dress() {
    let app = loaded();
    let page = usage_view_lines(&app, 80, TALL);
    assert!(wears_the_title_dress(&page[1]), "{:?}", plain(&page[1]));
    assert_eq!(plain(&page[1]).trim(), "Usage limits");
}

#[test]
fn a_page_whose_subscriptions_were_all_signed_out_points_at_login() {
    let mut app = loaded();
    app.begin_usage_round(Vec::new());
    let page = usage_view_lines(&app, 80, TALL);
    assert_frame_hugs_content("no subscription", &page);
    let joined = texts(&app, 80, TALL).join(" ");
    assert!(joined.contains("No subscription is signed in"), "{joined}");
    assert!(joined.contains("/login"), "{joined}");
    let hint = row_with(&page, "No subscription");
    assert_eq!(
        hint.spans.last().and_then(|span| span.style.fg),
        Some(model_selected_color()),
        "advice, in /model's accent"
    );
    assert!(
        joined.contains("r refresh  esc close"),
        "no ↑↓ and no link with nothing to move to or copy"
    );
    assert!(!joined.contains("copy link"), "{joined}");
}

#[test]
fn the_whole_page_hugs_its_rules() {
    let app = loaded();
    for width in [40u16, 60, 80, 120] {
        assert_frame_hugs_content(
            &format!("width {width}"),
            &usage_view_lines(&app, width, TALL),
        );
    }
}

// ===== a section per subscription =====

#[test]
fn the_subscription_in_use_comes_first_tagged_flush_right() {
    let app = loaded();
    let lines = usage_view_lines(&app, 100, TALL);
    let texts = texts(&app, 100, TALL);
    let heads: Vec<usize> = ["ChatGPT Codex", "ClinePass", "GitHub Copilot"]
        .iter()
        .map(|name| texts.iter().position(|t| t.contains(name)).unwrap())
        .collect();
    assert!(heads[0] < heads[1] && heads[1] < heads[2], "{texts:#?}");
    let head = &lines[heads[0]];
    assert!(
        plain(head).contains("❯ ChatGPT Codex  Pro · dev@example.com"),
        "{:?}",
        plain(head)
    );
    assert_eq!(
        plain(head).trim_end().len(),
        plain(head).len(),
        "flush to the edge"
    );
    assert!(plain(head).ends_with("in use"), "{:?}", plain(head));
    assert_eq!(cols(&plain(head)), 100);
    assert_eq!(fg_of(head, "in use"), Some(usage_ok_color()));
    assert!(!texts[heads[1]].contains("in use"));
}

#[test]
fn a_sections_rows_sit_under_its_name() {
    // Indented past the name's own column, so each section reads as one
    // block under its head.
    let app = loaded();
    let texts = texts(&app, 100, TALL);
    let name_column = texts
        .iter()
        .find_map(|t| t.find("ChatGPT Codex").map(|at| cols(&t[..at])))
        .expect("the head");
    for needle in ["Weekly limit", "Credits", "5-hour limit", "AI credits"] {
        let row = texts.iter().find(|t| t.contains(needle)).unwrap();
        assert!(indent(row) > name_column, "{needle}: {texts:#?}");
    }
}

#[test]
fn the_highlighted_section_wears_the_marker_and_the_accent() {
    let mut app = loaded();
    app.usage_page.as_mut().unwrap().selected = 2;
    let lines = usage_view_lines(&app, 100, TALL);
    let head = row_with(&lines, "GitHub Copilot");
    assert!(plain(head).contains("❯ GitHub Copilot"));
    assert_eq!(fg_of(head, "GitHub Copilot"), Some(model_selected_color()));
    let other = row_with(&lines, "ChatGPT Codex");
    assert!(!plain(other).contains('❯'));
    assert_ne!(fg_of(other, "ChatGPT Codex"), Some(model_selected_color()));
}

#[test]
fn a_limit_is_a_bar_its_share_and_when_it_resets() {
    let app = loaded();
    let lines = usage_view_lines(&app, 140, TALL);
    let weekly = plain(row_with(&lines, "7% used"));
    assert!(weekly.contains("Weekly limit"), "{weekly}");
    assert!(weekly.contains("resets in 6d · Sat 3:58 PM"), "{weekly}");
    let monthly = plain(row_with(&lines, "81% used"));
    assert!(
        monthly.contains("resets in 17d · Nov 12, 1:26 PM"),
        "{monthly}"
    );
    let five = plain(row_with(&lines, "5-hour limit"));
    assert!(
        five.contains("0% used") && !five.contains("resets"),
        "{five}"
    );
}

#[test]
fn a_reached_limit_says_so_in_red_on_its_own_row() {
    let app = loaded();
    let lines = usage_view_lines(&app, 140, TALL);
    let weekly = row_with(&lines, "100% used");
    assert!(
        plain(weekly).contains("100% used  limit reached · resets in 1d 23h · Tue 2:36 PM"),
        "{:?}",
        plain(weekly)
    );
    assert_eq!(fg_of(weekly, "limit reached"), Some(usage_full_color()));

    // Reached short of the whole — a Copilot bucket with no quota left —
    // is red too, whatever its share.
    let mut early = chatgpt();
    early.meters[0] = UsageMeter {
        group: None,
        label: "Spent limit".to_string(),
        value: MeterValue::Limit(Limit {
            used_percent: 60.0,
            counts: None,
            resets_at: None,
            reached: true,
        }),
    };
    let app = with_chatgpt(early);
    let lines = usage_view_lines(&app, 140, TALL);
    let row = row_with(&lines, "Spent limit");
    assert!(
        plain(row).contains("60% used  limit reached"),
        "{:?}",
        plain(row)
    );
    let filled = row
        .spans
        .iter()
        .find(|span| span.content.starts_with('█'))
        .expect("spent cells");
    assert_eq!(filled.style.fg, Some(usage_full_color()));
}

#[test]
fn the_local_offset_decides_the_hour_and_the_day() {
    let mut app = loaded();
    // Four hours west of UTC: the Copilot reset at midnight UTC on the 1st
    // is the evening of the 31st here.
    app.set_local_offset(|_| -4 * 3600);
    let lines = usage_view_lines(&app, 140, TALL);
    let credits = plain(row_with(&lines, "42% used"));
    assert!(credits.contains("Sat 8:00 PM"), "{credits}");
}

#[test]
fn a_countdown_names_minutes_hours_and_days() {
    let soon = |minutes: i64| NOW + minutes * 60;
    let app = with_chatgpt(UsageReport {
        meters: vec![
            limit("A limit", 1.0, Some(NOW + 20)),
            limit("B limit", 2.0, Some(soon(26))),
            limit("C limit", 3.0, Some(soon(4 * 60 + 56))),
            limit("D limit", 4.0, Some(soon(5 * 60))),
            limit("E limit", 5.0, Some(NOW - 30)),
        ],
        ..UsageReport::default()
    });
    let lines = usage_view_lines(&app, 140, TALL);
    let row = |label: &str| plain(row_with(&lines, label));
    assert!(
        row("A limit").contains("resets in <1m · 3:04 PM"),
        "{}",
        row("A limit")
    );
    assert!(
        row("B limit").contains("resets in 26m · 3:30 PM"),
        "{}",
        row("B limit")
    );
    assert!(
        row("C limit").contains("resets in 4h 56m · 8:00 PM"),
        "{}",
        row("C limit")
    );
    assert!(
        row("D limit").contains("resets in 5h · 8:04 PM"),
        "{}",
        row("D limit")
    );
    assert!(row("E limit").contains("resets now"), "{}", row("E limit"));
}

#[test]
fn the_bar_fills_as_the_limit_is_spent_and_warns_then_alarms() {
    let app = with_chatgpt(UsageReport {
        meters: vec![
            limit("Calm limit", 50.0, None),
            limit("Warm limit", 80.0, None),
            limit("Hot limit", 95.0, None),
        ],
        ..UsageReport::default()
    });
    let lines = usage_view_lines(&app, 120, TALL);
    for (label, color, percent) in [
        ("Calm limit", usage_meter_color(), "50%"),
        ("Warm limit", usage_warn_color(), "80%"),
        ("Hot limit", usage_full_color(), "95%"),
    ] {
        let row = row_with(&lines, label);
        let filled: Vec<&Span> = row
            .spans
            .iter()
            .filter(|span| span.content.starts_with('█'))
            .collect();
        assert_eq!(filled.len(), 1, "{label}: one run of spent cells");
        assert_eq!(filled[0].style.fg, Some(color), "{label}");
        let empty = row
            .spans
            .iter()
            .find(|span| span.content.starts_with('░'))
            .expect("the rest of the bar");
        assert_eq!(empty.style.fg, Some(usage_empty_color()));
        let (spent, left) = bar_cells(row);
        let share: f64 = percent.trim_end_matches('%').parse().unwrap();
        assert_eq!(
            spent,
            (share / 100.0 * (spent + left) as f64).round() as usize,
            "{label}"
        );
        assert!(plain(row).contains(&format!("{percent} used")), "{label}");
    }
}

#[test]
fn a_share_never_claims_all_or_none_it_is_not() {
    let app = with_chatgpt(UsageReport {
        meters: vec![
            limit("Sliver limit", 0.3, None),
            limit("Little limit", 2.0, None),
            limit("Idle limit", 0.0, None),
            limit("Nearly limit", 99.7, None),
            limit("Over limit", 104.5, None),
        ],
        ..UsageReport::default()
    });
    let lines = usage_view_lines(&app, 120, TALL);
    let sliver = row_with(&lines, "Sliver limit");
    assert!(plain(sliver).contains("<1% used"));
    assert_eq!(bar_cells(sliver).0, 1, "any use keeps one cell");
    assert_eq!(bar_cells(row_with(&lines, "Little limit")).0, 1);
    assert_eq!(bar_cells(row_with(&lines, "Idle limit")).0, 0);
    let nearly = row_with(&lines, "Nearly limit");
    assert!(plain(nearly).contains(">99% used"));
    assert!(
        bar_cells(nearly).1 > 0,
        "a bar short of the whole never looks full"
    );
    let over = row_with(&lines, "Over limit");
    assert!(plain(over).contains("100% used"));
    assert_eq!(bar_cells(over).1, 0);
}

#[test]
fn a_narrow_row_gives_up_the_clock_time_then_the_counts() {
    let app = loaded();
    let wide = plain(row_with(&usage_view_lines(&app, 150, TALL), "42% used"));
    assert!(
        wide.contains("420 of 1,000 credits · resets in 6d 8h · Nov 1, 12:00 AM"),
        "{wide}"
    );
    let medium = plain(row_with(&usage_view_lines(&app, 90, TALL), "42% used"));
    assert!(
        medium.contains("420 of 1,000 credits · resets in 6d 8h"),
        "{medium}"
    );
    assert!(!medium.contains("AM"), "{medium}");
    let lines = usage_view_lines(&app, 70, TALL);
    let narrow = plain(row_with(&lines, "42% used"));
    assert!(!narrow.contains("420 of"), "{narrow}");
    assert!(narrow.contains("resets in 6d 8h"), "{narrow}");
}

#[test]
fn counts_with_no_room_beside_the_bar_sit_under_it() {
    // Given up beside the bar, the counts are never lost: they move to a
    // row of their own under it, at the bar's column.
    let app = loaded();
    for width in [70u16, 44] {
        let texts = texts(&app, width, TALL);
        let at = texts
            .iter()
            .position(|t| t.contains("42% used"))
            .expect("the row");
        let bar_column = texts[at].find(['█', '░']).map(|at| cols(&texts[at][..at]));
        let counts = texts[at + 1..]
            .iter()
            .take(2)
            .find(|t| t.contains("420 of 1,000 credits"))
            .unwrap_or_else(|| panic!("{width}: {texts:#?}"));
        assert_eq!(
            Some(indent(counts)),
            bar_column,
            "{width}: under the bar: {texts:#?}"
        );
    }
    // With room beside the bar they stay there, and nothing repeats them.
    let texts = texts(&app, 150, TALL);
    assert_eq!(
        texts.iter().filter(|t| t.contains("420 of")).count(),
        1,
        "{texts:#?}"
    );
}

#[test]
fn every_bar_on_the_page_is_one_length() {
    // A column of meters lines up: the bar is sized for the page, never for
    // its own row's trailer.
    let app = loaded();
    for width in [150u16, 100, 80, 72, 64, 44] {
        let lines = usage_view_lines(&app, width, TALL);
        let lengths: Vec<usize> = lines
            .iter()
            .filter(|line| plain(line).contains("% used"))
            .map(|line| {
                let (spent, left) = bar_cells(line);
                spent + left
            })
            .collect();
        assert!(lengths.len() >= 4, "{width}: {lengths:?}");
        assert!(
            lengths.windows(2).all(|w| w[0] == w[1]),
            "{width}: {lengths:?}"
        );
    }
}

#[test]
fn a_reset_with_no_room_beside_the_bar_moves_under_it() {
    let app = loaded();
    let lines = usage_view_lines(&app, 44, TALL);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let at = texts
        .iter()
        .position(|t| t.contains("42% used"))
        .expect("the row");
    assert!(!texts[at].contains("resets"), "{texts:#?}");
    assert!(
        texts[at + 1].trim_start().starts_with("resets in 6d 8h"),
        "{texts:#?}"
    );
    for text in &texts {
        assert!(cols(text) <= 44, "{text:?}");
    }
}

#[test]
fn unlimited_and_balances_read_as_values() {
    let app = loaded();
    let lines = usage_view_lines(&app, 120, TALL);
    let chat = row_with(&lines, "Unlimited");
    assert!(
        plain(chat).trim_start().starts_with("Chat "),
        "{:?}",
        plain(chat)
    );
    assert_eq!(fg_of(chat, "Unlimited"), Some(usage_ok_color()));
    let credits = plain(row_with(&lines, "62,500 credits"));
    assert!(
        credits.contains("62,500 credits · ≈ 15.6k–81.3k local messages"),
        "{credits}"
    );
    // The detail gives way before the value does.
    let narrow = plain(row_with(
        &usage_view_lines(&app, 44, TALL),
        "62,500 credits",
    ));
    assert!(!narrow.contains("local messages"), "{narrow}");
}

#[test]
fn labels_line_up_across_the_page() {
    let app = loaded();
    let texts = texts(&app, 120, TALL);
    let starts: Vec<usize> = ["7% used", "100% used", "42% used"]
        .iter()
        .map(|needle| {
            let text = texts.iter().find(|t| t.contains(needle)).unwrap();
            text.find('█').or_else(|| text.find('░')).unwrap()
        })
        .collect();
    assert!(
        starts.windows(2).all(|w| w[0] == w[1]),
        "bars start together: {starts:?}"
    );
}

#[test]
fn per_model_limits_sit_under_their_model_whole() {
    // A model's name is a heading of its own, so neither it nor the window
    // under it is ever cut to fit a label column.
    let mut report = chatgpt();
    report.meters.push(UsageMeter {
        group: Some("GPT-5.3-Codex-Spark".to_string()),
        label: "30-minute limit".to_string(),
        value: MeterValue::Limit(Limit {
            used_percent: 88.0,
            counts: None,
            resets_at: None,
            reached: false,
        }),
    });
    let app = with_chatgpt(report);
    let texts = texts(&app, 100, TALL);
    let heading = texts
        .iter()
        .position(|t| t.trim() == "GPT-5.3-Codex-Spark")
        .expect("sub-heading");
    assert!(
        texts[heading + 1].contains("30-minute limit") && texts[heading + 1].contains("88% used"),
        "{texts:#?}"
    );
    assert!(
        indent(&texts[heading + 1]) > indent(&texts[heading]),
        "its rows sit under it: {texts:#?}"
    );
    assert!(!texts.iter().any(|t| t.contains('…')), "{texts:#?}");
}

#[test]
fn notes_follow_the_meters_in_their_tone() {
    let app = loaded();
    let lines = usage_view_lines(&app, 100, TALL);
    let note = row_with(&lines, "Paused until the weekly limit resets");
    assert!(plain(note).contains("⚠ Paused"), "{:?}", plain(note));
    assert!(
        note.spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| span.style.fg == Some(usage_warn_color())),
        "advice is amber"
    );
}

#[test]
fn one_link_names_the_highlighted_subscriptions_own_page() {
    let mut app = loaded();
    let lines = usage_view_lines(&app, 120, TALL);
    let links: Vec<String> = lines
        .iter()
        .map(|line| plain(line))
        .filter(|text| text.contains("Details at"))
        .collect();
    assert_eq!(
        links,
        ["  Details at https://chatgpt_codex.example/usage"],
        "one link, the highlighted section's"
    );
    let link_row = row_with(&lines, "Details at");
    let url = link_row
        .spans
        .iter()
        .find(|span| span.content.contains("https://"))
        .expect("the URL's span");
    assert!(url.style.add_modifier.contains(Modifier::UNDERLINED));
    assert!(
        url.style.underline_color.is_some(),
        "carries its link target"
    );
    let texts = texts(&app, 120, TALL);
    let at = texts.iter().position(|t| t.contains("Details at")).unwrap();
    assert!(
        texts[at + 1].contains("c copy link"),
        "the link sits over the hint naming its key: {texts:#?}"
    );

    app.usage_page.as_mut().unwrap().selected = 2;
    assert!(
        texts_contain(&app, "Details at https://github_copilot.example/usage"),
        "the link follows the highlight"
    );
}

fn texts_contain(app: &App, needle: &str) -> bool {
    texts(app, 120, TALL).iter().any(|t| t.contains(needle))
}

#[test]
fn a_reading_section_says_so_and_a_failed_one_says_why() {
    let mut app = with_session();
    let round = app.open_usage_page(targets()).unwrap();
    app.finish_usage_fetch(
        round,
        "cline_pass",
        Err("HTTP 401: refused".to_string()),
        NOW,
    );
    let lines = usage_view_lines(&app, 100, TALL);
    let texts = texts(&app, 100, TALL);
    assert!(
        texts.iter().any(|t| t.trim() == "Loading usage…"),
        "{texts:#?}"
    );
    let failed = row_with(&lines, "Couldn't load usage: HTTP 401: refused");
    assert!(
        failed
            .spans
            .iter()
            .any(|span| span.style.fg == Some(usage_full_color())),
        "in red"
    );
    assert!(
        texts.iter().any(|t| t.contains("loading…")),
        "the subtitle says so: {texts:#?}"
    );
}

#[test]
fn a_failed_refresh_keeps_the_last_report_under_a_warning() {
    let mut app = loaded();
    let round = app.begin_usage_round(targets()).unwrap();
    app.finish_usage_fetch(
        round,
        "github_copilot",
        Err("rate limited".to_string()),
        NOW + 60,
    );
    let lines = usage_view_lines(&app, 100, TALL);
    assert!(
        plain(row_with(&lines, "AI credits")).contains("42% used"),
        "still shown"
    );
    let warning = row_with(&lines, "Couldn't refresh: rate limited");
    assert!(
        warning
            .spans
            .iter()
            .any(|span| span.style.fg == Some(usage_warn_color())),
        "in amber"
    );
}

#[test]
fn the_subtitle_says_how_fresh_the_numbers_are() {
    let mut app = loaded();
    let subtitle = |app: &App| texts(app, 100, TALL)[2].clone();
    assert_eq!(subtitle(&app), "  3 subscriptions · updated just now");
    app.set_usage_now(NOW + 5 * 60 + 10);
    assert_eq!(subtitle(&app), "  3 subscriptions · updated 5m ago");
    app.set_usage_now(NOW + 2 * 3600);
    assert_eq!(subtitle(&app), "  3 subscriptions · updated 2h ago");
    app.set_usage_now(NOW + 3 * 86_400);
    assert_eq!(subtitle(&app), "  3 subscriptions · updated 3d ago");
    app.begin_usage_round(targets());
    assert_eq!(
        subtitle(&app),
        "  3 subscriptions · updated 3d ago · refreshing…"
    );
}

#[test]
fn a_first_read_still_landing_is_loading_not_refreshing() {
    let mut app = with_session();
    let round = app.open_usage_page(targets()).unwrap();
    app.finish_usage_fetch(round, "github_copilot", Ok(copilot()), NOW);
    app.set_usage_now(NOW);
    assert_eq!(
        texts(&app, 100, TALL)[2],
        "  3 subscriptions · loading…",
        "nothing was shown before, so nothing is being refreshed"
    );
}

#[test]
fn a_reopened_page_shows_its_last_numbers_while_refreshing() {
    let mut app = loaded();
    app.close_usage_page();
    app.open_usage_page(targets());
    app.set_usage_now(NOW + 120);
    let texts = texts(&app, 100, TALL);
    assert!(
        texts.contains(&"  3 subscriptions · updated 2m ago · refreshing…".to_string()),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t.contains("7% used")), "{texts:#?}");
    assert!(!texts.iter().any(|t| t.contains("Loading usage…")));
}

// ===== short terminals =====

#[test]
fn the_frame_hugs_its_content_at_every_width_and_height() {
    let mut app = loaded();
    for width in [40u16, 60, 80, 120] {
        for selected in [0, 2] {
            app.usage_page.as_mut().unwrap().selected = selected;
            for height in 1..=40u16 {
                let what = format!("width {width} height {height} selected {selected}");
                let page = usage_view_lines(&app, width, height);
                let texts: Vec<String> = page
                    .iter()
                    .map(|line| plain(line).trim_end().to_string())
                    .collect();
                let rule = |text: Option<&String>| text.is_some_and(|t| t.starts_with('─'));
                if rule(texts.first()) && rule(texts.last()) {
                    assert_frame_hugs_content(&what, &page);
                    continue;
                }
                let blank = |text: Option<&String>| text.is_some_and(String::is_empty);
                assert!(
                    !(rule(texts.first()) && blank(texts.get(1))),
                    "{what}: a blank row hugs the top rule: {texts:#?}"
                );
                assert!(
                    !(rule(texts.last()) && blank(texts.iter().rev().nth(1))),
                    "{what}: a blank row hugs the bottom rule: {texts:#?}"
                );
            }
        }
    }
}

#[test]
fn no_page_is_wider_than_the_terminal() {
    let app = loaded();
    for width in (2u16..12).chain((12..=130).step_by(3)) {
        for height in [TALL, 30, 12, 6, 2] {
            for line in usage_view_lines(&app, width, height) {
                assert!(
                    cols(&plain(&line)) <= usize::from(width),
                    "{width}x{height}: {:?}",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn a_short_terminal_sheds_the_link_then_folds_the_other_subscriptions() {
    let app = loaded();
    let whole = usage_view_lines(&app, 100, TALL).len() as u16;
    assert!(
        texts(&app, 100, whole)
            .iter()
            .any(|t| t.contains("Details at"))
    );
    let shed = texts(&app, 100, whole - 1);
    assert!(!shed.iter().any(|t| t.contains("Details at")), "{shed:#?}");
    assert!(
        shed.iter().any(|t| t.contains("Monthly limit")),
        "every section stays open"
    );

    // Shorter still: the highlighted section stays open, the others fold
    // to their head row and a summary of their fullest limit.
    let folded = texts(&app, 100, 14);
    assert!(folded.len() <= 14, "{folded:#?}");
    assert!(
        folded
            .iter()
            .any(|t| t.contains("Weekly limit") && t.contains("7% used"))
    );
    let cline = folded
        .iter()
        .find(|t| t.contains("ClinePass"))
        .expect("its head row");
    assert!(cline.contains("Weekly limit 100% used"), "{cline}");
    assert!(
        !folded.iter().any(|t| t.contains("Monthly limit")),
        "{folded:#?}"
    );
}

#[test]
fn every_page_of_the_ladder_keeps_the_title() {
    // A page never loses its name: past the whole header, the title folds
    // into one row with how fresh the numbers are.
    let app = loaded();
    for height in 1..=40u16 {
        let texts = texts(&app, 100, height);
        assert!(
            texts.iter().any(|t| t.contains("Usage limits")),
            "{height} rows: {texts:#?}"
        );
    }
    let short = texts(&app, 100, 9);
    assert!(
        short
            .iter()
            .any(|t| t.trim() == "Usage limits · updated just now"),
        "{short:#?}"
    );
}

#[test]
fn a_very_short_terminal_lists_every_subscription_folded_around_the_highlight() {
    let mut app = loaded();
    app.usage_page.as_mut().unwrap().selected = 1;
    let short = texts(&app, 100, 9);
    assert!(short.len() <= 9, "{short:#?}");
    let heads: Vec<&String> = short
        .iter()
        .filter(|t| t.contains("ChatGPT Codex") || t.contains("ClinePass") || t.contains("GitHub"))
        .collect();
    assert_eq!(heads.len(), 3, "{short:#?}");
    assert!(
        heads
            .iter()
            .any(|t| t.contains("❯ ClinePass  Monthly · Weekly limit 100% used")),
        "{short:#?}"
    );
    assert!(short.iter().any(|t| t.contains("esc close")), "{short:#?}");
}

#[test]
fn the_page_never_flows_however_short_the_terminal() {
    let app = loaded();
    for width in [100u16, 60] {
        for height in (1..=30).chain([TALL]) {
            assert!(
                crate::ui::view_flow(&app, width, height, 1000).is_none(),
                "{width}x{height}"
            );
            assert!(usage_view_lines(&app, width, height).len() <= usize::from(height.max(1)));
        }
    }
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = loaded();
    let lines = usage_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(usage_menu_rows(&app, 80, TALL), lines);
    assert_eq!(usage_page_height(&app, 80, TALL), Some(lines));
    assert_eq!(usage_page_height(&app, 80, 6), Some(6));
}

#[test]
fn render_paints_the_page_and_the_cursor_hides_seated_on_the_marker() {
    let app = loaded();
    let height = usage_page_height(&app, 80, TALL).unwrap();
    let mut buf = buffer(80, height);
    render_usage_page(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 80).starts_with('─'));
    assert!(
        !cursor_visible(&app),
        "a read-only page has nothing to point at"
    );
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the highlighted section wears the marker");
    assert_eq!(
        cursor_position(Rect::new(0, 0, 80, height), &app),
        (2, marker_row)
    );
}

#[test]
fn on_a_short_terminal_the_hidden_cursor_seats_on_the_marker() {
    // The region is the page's own height, as the loop reserves it.
    let mut app = loaded();
    app.usage_page.as_mut().unwrap().selected = 2;
    for terminal in [12u16, 8, 5] {
        let height = usage_page_height(&app, 80, terminal).unwrap();
        let mut buf = buffer(80, height);
        render_usage_page(buf.area, &mut buf, &app);
        let marker_row = (0..height)
            .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
            .expect("the highlighted section is on screen");
        assert_eq!(
            cursor_position(Rect::new(0, 0, 80, height), &app),
            (2, marker_row),
            "{terminal} rows"
        );
    }
}

#[test]
fn mid_turn_the_strip_keeps_its_rows_above_the_page() {
    let mut app = loaded();
    app.begin_stream();
    let height = usage_page_height(&app, 80, 60).unwrap();
    let page_rows = usage_menu_rows(&app, 80, 60);
    assert!(height > page_rows, "the strip's rows sit above the page");
    let mut buf = buffer(80, height);
    render_live(Rect::new(0, 0, 80, height), &mut buf, &app);
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    let status = painted
        .iter()
        .position(|r| r.contains("esc to interrupt"))
        .expect("status line");
    let title = painted
        .iter()
        .position(|r| r.trim() == "Usage limits")
        .expect("the page");
    assert!(status < title, "{painted:#?}");
}

//! The read-only `/context` page (`docs/context-usage.md`).

use std::sync::Arc;

use super::*;
use crate::app::{ContextUsageRow, count_tokens};
use crate::context_usage::{Category, ToolDefinition};
use crate::stream::TokenUsage;
use crate::ui::context_usage_view::{allot_cells, context_usage_menu_rows};
use crate::ui::theme::{
    context_usage_color, context_usage_free_color, context_usage_full_color,
    context_usage_meter_color, context_usage_reserved_color, context_usage_warn_color,
    model_meta_color, model_selected_color,
};

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn tool(name: &str, size: usize) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        schema: "x".repeat(size),
    }
}

/// A session with a prompt, seven built-in tools and an MCP server's tool,
/// one message, and `window` — the page open on it.
fn open_with(window: Option<u64>) -> App {
    let mut app = with_session();
    app.set_system_prompt(Some(
        "# System prompt\n\nBe terse.\n\n## Environment\n\nDate 2026-10-22".to_string(),
    ));
    app.set_tool_definitions(Arc::from(vec![
        tool("bash", 6_000),
        tool("read", 3_000),
        tool("edit", 2_000),
        tool("write", 1_500),
        tool("agent", 1_200),
        tool("taskcreate", 900),
        tool("skill", 800),
        tool("mcp__deepwiki__ask_question", 700),
    ]));
    app.record_user_message("hello there");
    app.set_context_window(window);
    app.open_context_usage();
    app
}

fn open_app() -> App {
    open_with(Some(200_000))
}

/// The page with the highlight on `row`.
fn select(app: &mut App, row: ContextUsageRow) {
    app.context_usage.as_mut().expect("open").selected = row;
}

fn page_texts(app: &App, width: u16) -> Vec<String> {
    context_usage_lines(app, width, TALL)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn has(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|t| t.contains(needle))
}

/// The page's tokens for `category`.
fn tokens(app: &App, category: Category) -> usize {
    app.context_usage
        .as_ref()
        .expect("open")
        .breakdown
        .slice(category)
        .map_or(0, |slice| slice.tokens)
}

/// The window bar: the row made of the bar's cells.
fn bar_row(app: &App, width: u16) -> Line<'static> {
    context_usage_lines(app, width, TALL)
        .into_iter()
        .find(|line| {
            let text = plain(line);
            let text = text.trim();
            !text.is_empty() && text.chars().all(|c| matches!(c, '█' | '░' | '╱'))
        })
        .expect("the page has a window bar")
}

/// Turn on usage reporting: one frame, applied mid-turn as the loop does.
fn report(app: &mut App, usage: TokenUsage) {
    app.begin_stream();
    app.apply_usage(&usage);
}

// ===== the frame =====

#[test]
fn closed_builds_nothing() {
    assert!(context_usage_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(context_usage_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_its_title_subtitle_bar_legend_usage_and_hint() {
    let app = open_app();
    let texts = page_texts(&app, 100);
    assert!(
        texts.first().is_some_and(|t| t.starts_with('─')),
        "top rule"
    );
    assert!(
        texts.last().is_some_and(|t| t.starts_with('─')),
        "bottom rule"
    );
    let total = format_token_count(app.context_usage.as_ref().unwrap().breakdown.total());
    for expect in [
        "Context usage",
        &format!("dummy_model_name · ≈{total} of 200k tokens ("),
        "System prompt",
        "System tools",
        "MCP tools",
        "Messages",
        "Free space",
        "Auto-compact",
        "Last request",
        "no usage reported yet",
        "↑↓ select  ctrl+d raw context  esc close",
    ] {
        assert!(has(&texts, expect), "{expect:?} is on the page: {texts:?}");
    }
    let lines = context_usage_lines(&app, 100, TALL);
    assert!(
        wears_the_title_dress(&lines[1]),
        "the title opens the page in the pages' title dress"
    );
}

#[test]
fn the_frame_hugs_its_content_at_every_width_and_height() {
    let app = open_app();
    for width in [40u16, 60, 80, 120] {
        for height in 1..=40u16 {
            assert_frame_hugs_content(
                &format!("width {width} height {height}"),
                &context_usage_lines(&app, width, height),
            );
        }
    }
}

#[test]
fn no_page_is_wider_than_the_terminal() {
    let mut app = open_app();
    report(
        &mut app,
        TokenUsage {
            input: 38_100,
            output: 1_200,
            cached: 31_600,
            cache_write: 2_100,
            reasoning: 0,
        },
    );
    let rows = app.context_usage_rows();
    for width in (12u16..=130).step_by(3) {
        for row in &rows {
            select(&mut app, *row);
            for height in [TALL, 18, 9] {
                for line in context_usage_lines(&app, width, height) {
                    assert!(
                        crate::ui::wrap::cols(&plain(&line)) <= width as usize,
                        "width {width} {row:?}: {:?} overflows",
                        plain(&line)
                    );
                }
            }
        }
    }
}

// ===== the window bar =====

#[test]
fn allot_cells_shares_the_cells_by_size_and_never_loses_one() {
    assert_eq!(allot_cells(&[50, 50], 10), [5, 5]);
    assert_eq!(allot_cells(&[30, 60, 10], 20), [6, 12, 2]);
    // Each boundary sits within half a cell of where it belongs (3.3 and
    // 6.7 of 10), so a position on the bar — the auto-compact threshold —
    // reads true.
    assert_eq!(allot_cells(&[1, 1, 1], 10), [3, 4, 3]);
    // Anything at all gets a cell, taken from what follows it.
    assert_eq!(allot_cells(&[1, 999], 10), [1, 9]);
    assert_eq!(allot_cells(&[0, 10, 0], 5), [0, 5, 0]);
    // More segments than cells: plain rounding decides.
    assert_eq!(allot_cells(&[1, 1, 1], 2), [1, 0, 1]);
    assert_eq!(allot_cells(&[], 5), Vec::<usize>::new());
    assert_eq!(allot_cells(&[0, 0], 5), [0, 0]);
    assert_eq!(allot_cells(&[3, 7], 0), [0, 0]);
    for (sizes, cells) in [
        (
            vec![161u64, 11_900, 1_100, 433, 21_200, 143_600, 20_000],
            76,
        ),
        (vec![5, 5, 5, 5, 5, 5, 5, 5], 7),
        (vec![1, 1_000_000], 3),
    ] {
        assert_eq!(
            allot_cells(&sizes, cells).iter().sum::<usize>(),
            cells,
            "{sizes:?} into {cells}"
        );
    }
}

#[test]
fn a_zero_size_never_takes_the_cell_held_back_for_the_size_after_it() {
    // A conversation past the threshold has no free space and a sliver of
    // headroom: the boundary before the empty free-space segment is held
    // back a cell so the headroom can show, and that cell is the headroom's
    // — drawn as free space, it would claim room the window doesn't have.
    assert_eq!(allot_cells(&[95, 0, 5], 10), [9, 0, 1]);
    assert_eq!(allot_cells(&[90, 0, 0, 5, 5], 10), [8, 0, 0, 1, 1]);
}

#[test]
fn the_bar_stands_for_the_whole_window() {
    let app = open_app();
    let width = 100u16;
    let bar = bar_row(&app, width);
    let cells: Vec<&Span> = bar
        .spans
        .iter()
        .filter(|s| !s.content.trim().is_empty())
        .collect();
    let count = |glyph: char| -> usize {
        cells
            .iter()
            .map(|s| s.content.chars().filter(|&c| c == glyph).count())
            .sum()
    };
    let used = count('█');
    let free = count('░');
    let reserved = count('╱');
    assert_eq!(
        used + free + reserved,
        width as usize - 4,
        "the bar spans the page less its insets"
    );
    // The last tenth is the hatched headroom auto-compact keeps.
    assert_eq!(reserved, 10, "10% of 96 cells");
    let text = plain(&bar);
    assert!(
        text.trim_end().ends_with('╱'),
        "the headroom closes the bar: {text:?}"
    );
    // Every category the window holds has at least one cell of its colour.
    for slice in &app.context_usage.as_ref().unwrap().breakdown.slices {
        assert!(
            cells.iter().any(|s| s.content.contains('█')
                && s.style.fg == Some(context_usage_color(slice.category))),
            "{:?} is on the bar",
            slice.category
        );
    }
    assert!(
        cells
            .iter()
            .filter(|s| s.content.contains('░'))
            .all(|s| s.style.fg == Some(context_usage_free_color()))
    );
    assert!(
        cells
            .iter()
            .filter(|s| s.content.contains('╱'))
            .all(|s| s.style.fg == Some(context_usage_reserved_color()))
    );
}

#[test]
fn a_window_past_the_threshold_fills_the_hatched_cells() {
    // The window barely holds the conversation: past 90%, the used cells
    // run into the headroom and the subtitle's share turns red.
    let mut app = open_with(Some(1_000));
    let total = app.context_usage.as_ref().unwrap().breakdown.total() as u64;
    app.set_context_window(Some(total + total / 50));
    app.refresh_context_usage();
    let bar = plain(&bar_row(&app, 100));
    let reserved = bar.chars().filter(|&c| c == '╱').count();
    assert!(reserved <= 2, "the headroom is nearly used up: {bar:?}");
    assert_eq!(bar.chars().filter(|&c| c == '░').count(), 0, "{bar:?}");
    let subtitle = &context_usage_lines(&app, 100, TALL)[2];
    let share = subtitle
        .spans
        .iter()
        .find(|s| s.content.ends_with('%'))
        .expect("the share");
    assert_eq!(share.style.fg, Some(context_usage_full_color()));
}

#[test]
fn the_share_turns_amber_then_red_as_the_window_fills() {
    let app = open_app();
    let total = app.context_usage.as_ref().unwrap().breakdown.total() as u64;
    for (window, expect) in [
        (total * 10, model_meta_color()),
        (total * 100 / 80, context_usage_warn_color()),
        (total * 100 / 95, context_usage_full_color()),
    ] {
        let mut app = open_with(Some(window));
        app.refresh_context_usage();
        let subtitle = &context_usage_lines(&app, 100, TALL)[2];
        let share = subtitle
            .spans
            .iter()
            .find(|s| s.content.ends_with('%'))
            .expect("the share");
        assert_eq!(share.style.fg, Some(expect), "window {window}");
    }
}

// ===== the legend =====

#[test]
fn each_legend_row_is_a_swatch_label_tokens_and_share_in_columns() {
    let app = open_app();
    let lines = context_usage_lines(&app, 60, TALL);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    let rows = app.context_usage_rows();
    let mut ends = std::collections::HashSet::new();
    for row in &rows {
        let label = match row {
            ContextUsageRow::Slice(category) => category.label(),
            ContextUsageRow::Free => "Free space",
            ContextUsageRow::AutoCompact => "Auto-compact",
        };
        let at = texts
            .iter()
            .position(|t| t.contains(label) && t.trim_end().ends_with('%'))
            .unwrap_or_else(|| panic!("{label} has a legend row: {texts:?}"));
        ends.insert(crate::ui::wrap::cols(texts[at].trim_end()));
        let swatch = match row {
            ContextUsageRow::Slice(_) => "■",
            ContextUsageRow::Free => "□",
            ContextUsageRow::AutoCompact => "╱",
        };
        let colour = match row {
            ContextUsageRow::Slice(category) => context_usage_color(*category),
            ContextUsageRow::Free => context_usage_free_color(),
            ContextUsageRow::AutoCompact => context_usage_reserved_color(),
        };
        let span = lines[at]
            .spans
            .iter()
            .find(|s| s.content.trim() == swatch)
            .unwrap_or_else(|| panic!("{label} wears its swatch"));
        assert_eq!(span.style.fg, Some(colour), "{label}'s swatch colour");
    }
    assert_eq!(
        ends.len(),
        1,
        "the share column is right-aligned: {texts:?}"
    );
}

#[test]
fn the_highlighted_row_wears_the_marker_and_the_accent() {
    let mut app = open_app();
    for row in app.context_usage_rows() {
        select(&mut app, row);
        let lines = context_usage_lines(&app, 60, TALL);
        let marked: Vec<&Line> = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.as_ref() == "❯ "))
            .collect();
        assert_eq!(marked.len(), 1, "one ❯ on the page for {row:?}");
        let label = marked[0]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .expect("the highlighted label is bold");
        assert_eq!(label.style.fg, Some(model_selected_color()));
    }
}

#[test]
fn the_page_opens_on_the_largest_category_with_its_parts_beside_the_legend() {
    let app = open_app();
    assert_eq!(
        app.context_usage.as_ref().unwrap().selected,
        ContextUsageRow::Slice(Category::SystemTools)
    );
    let texts = page_texts(&app, 100);
    // The detail sits beside the legend: its header shares the first
    // legend row, its largest part the second.
    let first = texts
        .iter()
        .position(|t| t.contains("System prompt") && t.contains('%'))
        .expect("the first legend row");
    assert!(
        texts[first].contains("System tools · 7 tools"),
        "the detail's header beside the first row: {:?}",
        texts[first]
    );
    assert!(
        texts[first + 1].contains("bash") && texts[first + 1].contains('━'),
        "the largest part beside the second row: {:?}",
        texts[first + 1]
    );
}

#[test]
fn a_narrow_terminal_stacks_the_detail_under_the_legend() {
    let app = open_app();
    let texts = page_texts(&app, 60);
    let header = texts
        .iter()
        .position(|t| t.contains("System tools · 7 tools"))
        .expect("the detail's header");
    let last_row = texts
        .iter()
        .rposition(|t| t.contains("Auto-compact") && t.contains('%'))
        .expect("the legend's last row");
    assert!(header > last_row + 1, "the detail is under the legend");
    assert!(texts[header - 1].is_empty(), "one blank row between them");
}

#[test]
fn a_long_detail_folds_its_tail_into_one_row() {
    // Seven tools, six parts: five shown and the rest summed.
    let app = open_app();
    let texts = page_texts(&app, 60);
    let more = texts
        .iter()
        .find(|t| t.contains("… 2 more"))
        .unwrap_or_else(|| panic!("the fold row: {texts:?}"));
    let rest = count_tokens(&"x".repeat(900)) + count_tokens(&"x".repeat(800));
    assert!(
        more.contains(&format_token_count(rest)),
        "the fold carries the folded parts' sum: {more:?}"
    );
    for shown in ["bash", "read", "edit", "write", "agent"] {
        assert!(has(&texts, shown), "{shown} is shown");
    }
}

#[test]
fn mcp_tools_are_listed_by_server() {
    let mut app = open_app();
    select(&mut app, ContextUsageRow::Slice(Category::McpTools));
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "MCP tools · 1 tool"), "{texts:?}");
    assert!(has(&texts, "deepwiki · 1 tool"), "{texts:?}");
}

#[test]
fn the_messages_detail_says_what_the_conversation_is_made_of() {
    let mut app = open_app();
    select(&mut app, ContextUsageRow::Slice(Category::Messages));
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "Messages · 1 message"), "{texts:?}");
    assert!(has(&texts, "Your messages"), "{texts:?}");
    // An empty conversation says so rather than showing nothing.
    let mut empty = with_session();
    empty.open_context_usage();
    let texts = page_texts(&empty, 100);
    assert!(has(&texts, "Messages · 0 messages"), "{texts:?}");
    assert!(has(&texts, "Nothing yet"), "{texts:?}");
}

#[test]
fn free_space_and_the_headroom_explain_themselves() {
    let mut app = open_app();
    select(&mut app, ContextUsageRow::Free);
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "until auto-compact runs at 180k"), "{texts:?}");
    select(&mut app, ContextUsageRow::AutoCompact);
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "stays free"), "{texts:?}");
    assert!(has(&texts, "/compact"), "{texts:?}");
    // With auto compact off the free space runs to the window's end.
    app.settings_mut().auto_compact = false;
    app.refresh_context_usage();
    select(&mut app, ContextUsageRow::Free);
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "until the window is full"), "{texts:?}");
    assert!(!has(&texts, "Auto-compact"), "no headroom row: {texts:?}");
}

#[test]
fn an_unknown_window_shares_the_total() {
    let app = open_with(None);
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "window unknown"), "{texts:?}");
    assert!(!has(&texts, "Free space"), "{texts:?}");
    assert!(!has(&texts, "Auto-compact"), "{texts:?}");
    let bar = plain(&bar_row(&app, 100));
    assert!(
        !bar.contains('░') && !bar.contains('╱'),
        "the bar is the total's own proportions: {bar:?}"
    );
    // The messages' share is of the total, not of a window.
    let total = app.context_usage.as_ref().unwrap().breakdown.total();
    let tools = tokens(&app, Category::SystemTools);
    let share = format!("{:.1}%", tools as f64 * 100.0 / total as f64);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("System tools") && t.contains(&share)),
        "System tools reads {share}: {texts:?}"
    );
}

// ===== usage and the cache =====

#[test]
fn the_usage_rows_report_the_providers_numbers() {
    let mut app = open_app();
    report(
        &mut app,
        TokenUsage {
            input: 38_100,
            output: 1_200,
            cached: 31_600,
            cache_write: 2_100,
            reasoning: 0,
        },
    );
    let lines = context_usage_lines(&app, 100, TALL);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    for expect in [
        "Last request",
        "38.1k in · 1.2k out",
        "Cache hit",
        "83%",
        "31.6k read · 2.1k written",
        "Since launch",
        "1 request · 38.1k in · 1.2k out · 83% cached",
    ] {
        assert!(has(&texts, expect), "{expect:?}: {texts:?}");
    }
    // The meter: its filled cells in the success green, about 83% of it.
    let meter = lines
        .iter()
        .find(|l| plain(l).contains("Cache hit"))
        .expect("the meter row");
    let filled: usize = meter
        .spans
        .iter()
        .filter(|s| s.style.fg == Some(context_usage_meter_color()))
        .map(|s| s.content.chars().filter(|&c| c == '█').count())
        .sum();
    let empty: usize = meter
        .spans
        .iter()
        .map(|s| s.content.chars().filter(|&c| c == '░').count())
        .sum();
    assert!(filled > 0 && empty > 0, "{:?}", plain(meter));
    let share = filled as f64 / (filled + empty) as f64;
    assert!((share - 0.83).abs() < 0.07, "{share}: {:?}", plain(meter));
}

#[test]
fn inside_an_agent_view_the_usage_rows_are_the_agents() {
    let mut app = open_app();
    app.close_context_usage();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs_for_view());
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Usage(TokenUsage {
            input: 5_000,
            output: 100,
            cached: 2_500,
            ..TokenUsage::default()
        }),
    );
    app.open_agent_view("a1");
    app.open_context_usage();
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "This agent"), "{texts:?}");
    assert!(has(&texts, "5k in · 100 out"), "{texts:?}");
    assert!(!has(&texts, "Since launch"), "{texts:?}");
}

/// One subagent, for the agent-view test.
fn agent_specs_for_view() -> Vec<crate::stream::AgentSpec> {
    vec![crate::stream::AgentSpec {
        id: "a1".to_string(),
        description: "Look around".to_string(),
        agent_type: crate::agents::GENERAL_PURPOSE.to_string(),
        prompt: "What is here?".to_string(),
        background: false,
        call_id: None,
        arguments: None,
    }]
}

// ===== a terminal too short for the page =====

#[test]
fn a_short_terminal_sheds_rows_and_keeps_the_legend_and_the_hint() {
    let mut app = open_app();
    report(
        &mut app,
        TokenUsage {
            input: 38_100,
            output: 1_200,
            cached: 31_600,
            ..TokenUsage::default()
        },
    );
    let whole = context_usage_lines(&app, 60, TALL).len() as u16;
    let mut last = whole;
    let mut shed = Vec::new();
    for height in (1..=whole).rev() {
        let page: Vec<String> = context_usage_lines(&app, 60, height)
            .iter()
            .map(plain)
            .collect();
        assert!(
            page.len() as u16 <= last,
            "a shorter terminal never builds a taller page"
        );
        last = page.len() as u16;
        if page.len() as u16 <= height {
            assert!(
                has(&page, "↑↓ select"),
                "height {height} keeps the hint: {page:?}"
            );
            assert!(
                has(&page, "System tools") && has(&page, "Messages"),
                "height {height} keeps the legend: {page:?}"
            );
        }
        for (what, needle) in [
            ("the detail", "· 7 tools"),
            ("since launch", "Since launch"),
            ("the meter", "Cache hit"),
            ("the title", "Context usage"),
        ] {
            if !has(&page, needle) && !shed.contains(&what) {
                shed.push(what);
            }
        }
    }
    assert_eq!(
        shed,
        ["the detail", "since launch", "the meter", "the title"],
        "what yields first"
    );
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_app();
    let lines = context_usage_lines(&app, 80, TALL).len() as u16;
    assert_eq!(context_usage_menu_rows(&app, 80, TALL), lines);
    assert_eq!(context_usage_height(&app, 80, 200), Some(lines));
    assert_eq!(context_usage_height(&app, 80, 6), Some(6));
}

#[test]
fn a_terminal_shorter_than_every_page_flows_its_top() {
    let app = open_app();
    let page = context_usage_lines(&app, 80, 4).len();
    let flow = crate::ui::view_flow(&app, 80, 4, 1000).expect("flows on a 4-row terminal");
    assert_eq!(flow.lines.len(), page - 4, "the skipped top flows");
}

#[test]
fn render_paints_the_page_and_the_cursor_hides_seated_on_the_marker() {
    let app = open_app();
    let height = context_usage_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_context_usage(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 80).starts_with('─'));
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    assert!(painted.iter().any(|r| r.contains("Context usage")));
    assert!(
        !cursor_visible(&app),
        "a read-only page has nothing for a cursor to point at"
    );
    let area = Rect::new(0, 0, 80, height);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the selected row wears the marker");
    assert_eq!(cursor_position(area, &app), (2, marker_row));
}

#[test]
fn a_short_page_keeps_the_fill_on_one_header_row() {
    // Past the bar, the title and the window's fill fold into one row
    // rather than going together: how full the window is outlasts the
    // picture of it.
    let app = open_app();
    let one_row = (1..=40u16)
        .map(|height| context_usage_lines(&app, 80, height))
        .find(|page| {
            let texts: Vec<String> = page.iter().map(plain).collect();
            has(&texts, "Context usage") && !texts.iter().any(|t| t.contains('░'))
        })
        .expect("a page with a title and no bar");
    let title = one_row
        .iter()
        .find(|line| plain(line).contains("Context usage"))
        .expect("the title row");
    assert!(
        plain(title).contains("of 200k tokens ("),
        "the fill rides the title row: {:?}",
        plain(title)
    );
    assert!(
        !one_row
            .iter()
            .any(|line| plain(line).contains("dummy_model_name")),
        "the subtitle row is gone"
    );
}

#[test]
fn a_narrow_ledger_row_drops_its_cache_clause_whole() {
    let mut app = open_app();
    report(
        &mut app,
        TokenUsage {
            input: 38_100,
            output: 1_200,
            cached: 31_600,
            ..TokenUsage::default()
        },
    );
    let texts = page_texts(&app, 60);
    let ledger = texts
        .iter()
        .find(|t| t.contains("Since launch"))
        .expect("the ledger row");
    assert!(
        ledger.ends_with("1.2k out"),
        "the clause that did not fit is left out, never cut: {ledger:?}"
    );
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "1.2k out · 83% cached"), "{texts:?}");
}

#[test]
fn a_docs_path_reads_by_the_sessions_path_rule() {
    // The breakdown keeps the path as the section names it; the screen
    // shows it the way every file tool's header does (`docs/tools.md` *Path
    // display*): relative under the cwd.
    let mut app = with_session();
    app.set_path_display(crate::app::PathDisplay::new(
        "/repo",
        Some(std::path::PathBuf::from("/home/me")),
    ));
    app.set_user_instructions(Some(format!(
        "{}\n\nContents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nUse TDD.",
        crate::project_doc::INSTRUCTIONS_PREAMBLE
    )));
    app.open_context_usage();
    select(&mut app, ContextUsageRow::Slice(Category::ProjectDocs));
    let texts = page_texts(&app, 100);
    assert!(has(&texts, "Project docs · 1 file"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("AGENTS.md")) && !has(&texts, "/repo/AGENTS.md"),
        "{texts:?}"
    );
}

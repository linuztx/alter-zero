//! The `/context` page (`docs/context-usage.md`).

use super::*;

use std::sync::Arc;

use crate::context_usage::{Breakdown, Category, ToolDefinition, UsageLedger};
use crate::stream::TokenUsage;

/// A tool definition whose schema is `size` bytes of `x`.
fn tool(name: &str, size: usize) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        schema: "x".repeat(size),
    }
}

/// An app with a system prompt, a tool set and a short conversation.
fn session() -> App {
    let mut app = App::new();
    app.set_system_prompt(Some("# System prompt\n\nBe terse.".to_string()));
    app.set_tool_definitions(Arc::from(vec![tool("bash", 4_000), tool("read", 2_000)]));
    app.record_user_message("hello there");
    app
}

/// The page's highlighted row.
fn selected(app: &App) -> ContextUsageRow {
    app.context_usage
        .as_ref()
        .expect("the page is open")
        .selected
}

/// The open page's tokens for `category` (0 when the window holds none).
fn tokens(app: &App, category: Category) -> usize {
    app.context_usage
        .as_ref()
        .expect("the page is open")
        .breakdown
        .slice(category)
        .map_or(0, |slice| slice.tokens)
}

// ===== the command =====

#[test]
fn the_palette_lists_context_right_after_compact() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let context = names
        .iter()
        .position(|n| *n == "context")
        .expect("/context is registered");
    assert_eq!(
        names[context - 1],
        "compact",
        "the window's two commands sit together"
    );
    assert_eq!(
        COMMANDS[context].description,
        "Show what fills the context window"
    );
    assert_eq!(COMMANDS[context].effect, CommandEffect::Context);
}

#[test]
fn slash_context_opens_the_page_idle_and_mid_turn() {
    for mid_turn in [false, true] {
        let mut app = session();
        if mid_turn {
            app.begin_stream();
        }
        app.shortcuts_open = true;
        type_str(&mut app, "/context");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::OpenContextUsage,
            "mid-turn {mid_turn}"
        );
        assert!(app.context_usage.is_some(), "mid-turn {mid_turn}");
        assert!(app.input.is_empty(), "the command token is consumed");
        assert!(app.command_menu.is_none());
        assert!(!app.shortcuts_open, "the bands sharing the composer close");
    }
}

#[test]
fn the_page_opens_on_the_largest_category() {
    // The first answer on screen is "what is using my context?".
    let mut app = session();
    app.open_context_usage();
    assert_eq!(
        selected(&app),
        ContextUsageRow::Slice(Category::SystemTools)
    );
    // A conversation that outweighs everything else is what it opens on.
    let mut app = App::new();
    app.record_user_message(&"word ".repeat(400));
    app.open_context_usage();
    assert_eq!(selected(&app), ContextUsageRow::Slice(Category::Messages));
}

#[test]
fn the_page_counts_the_window_the_session_sends() {
    let mut app = session();
    app.set_user_instructions(Some(format!(
        "{}\n\nContents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nUse TDD.",
        crate::project_doc::INSTRUCTIONS_PREAMBLE
    )));
    app.set_listings(Some(crate::skills::skill_section("- dataviz: Charts")));
    app.open_context_usage();
    let page = app.context_usage.as_ref().expect("open");
    let order: Vec<Category> = page.breakdown.slices.iter().map(|s| s.category).collect();
    assert_eq!(
        order,
        [
            Category::SystemPrompt,
            Category::SystemTools,
            Category::ProjectDocs,
            Category::Skills,
            Category::Messages,
        ]
    );
    // The tool schemas are counted as sent, the messages as derived.
    assert_eq!(
        tokens(&app, Category::SystemTools),
        count_tokens(&"x".repeat(4_000)) + count_tokens(&"x".repeat(2_000))
    );
    assert_eq!(
        tokens(&app, Category::Messages),
        count_tokens("hello there")
    );
}

// ===== the rows =====

#[test]
fn the_rows_are_the_slices_then_the_room_left() {
    let mut app = session();
    app.open_context_usage();
    let slices = vec![
        ContextUsageRow::Slice(Category::SystemPrompt),
        ContextUsageRow::Slice(Category::SystemTools),
        ContextUsageRow::Slice(Category::Messages),
    ];
    // No window known: nothing to say about the room left.
    assert_eq!(app.context_usage_rows(), slices);
    assert_eq!(app.context_usage_window(), None);
    assert_eq!(app.context_usage_threshold(), None);
    // A window: the free space, then the headroom auto-compact keeps.
    app.set_context_window(Some(200_000));
    let mut with_room = slices.clone();
    with_room.extend([ContextUsageRow::Free, ContextUsageRow::AutoCompact]);
    assert_eq!(app.context_usage_rows(), with_room);
    assert_eq!(app.context_usage_window(), Some(200_000));
    assert_eq!(app.context_usage_threshold(), Some(180_000));
    // Auto compact off: no headroom is kept, so there is no row for it.
    app.settings.auto_compact = false;
    let mut free_only = slices;
    free_only.push(ContextUsageRow::Free);
    assert_eq!(app.context_usage_rows(), free_only);
    assert_eq!(app.context_usage_threshold(), None);
}

// ===== keys =====

#[test]
fn arrows_move_the_highlight_wrapping_and_home_end_jump() {
    let mut app = session();
    app.set_context_window(Some(200_000));
    app.open_context_usage();
    let rows = app.context_usage_rows();
    app.context_usage.as_mut().expect("open").selected = rows[0];
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(selected(&app), rows[1]);
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        selected(&app),
        *rows.last().expect("rows"),
        "↑ wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), rows[0], "↓ wraps to the first");
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), *rows.last().expect("rows"));
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), rows[0]);
}

#[test]
fn esc_and_ctrl_c_close_the_page() {
    for close in [key(KeyCode::Esc), ctrl('c')] {
        let mut app = session();
        app.open_context_usage();
        assert_eq!(app.on_key(close), Action::CloseContextUsage);
        assert!(app.context_usage.is_none());
        assert_eq!(app.view, View::Conversation, "Ctrl+C never quits from here");
    }
}

#[test]
fn ctrl_d_opens_the_raw_context_over_the_open_page() {
    // The two read-only overlays show what the page measures, so they pass
    // through — the permission prompt's rule — and the page waits behind.
    let mut app = session();
    app.open_context_usage();
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
    assert!(app.context_usage.is_some(), "the page stays open behind it");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.view, View::Conversation);
    assert!(app.context_usage.is_some(), "and is there on the way back");
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput);
}

#[test]
fn every_other_key_is_swallowed() {
    let mut app = session();
    app.open_context_usage();
    let before = selected(&app);
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
    assert!(app.context_usage.is_some());
    assert_eq!(selected(&app), before);
}

#[test]
fn the_page_blanks_the_ctrl_b_hint_it_would_swallow() {
    let mut app = session();
    app.set_command_elapsed(Some(std::time::Duration::from_secs(9)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_context_usage();
    assert_eq!(app.background_hint_elapsed(), None);
}

// ===== live =====

#[test]
fn refresh_recounts_when_the_conversation_moves_and_only_then() {
    let mut app = session();
    app.open_context_usage();
    // A sentinel in place of the count: an unchanged window keeps it.
    let sentinel = Breakdown::default();
    app.context_usage.as_mut().expect("open").breakdown = sentinel.clone();
    app.refresh_context_usage();
    assert_eq!(
        app.context_usage.as_ref().expect("open").breakdown,
        sentinel,
        "nothing changed, nothing was recounted"
    );
    // A message lands: the next refresh counts it — as the derivation sends
    // it, merged into the user entry before it (the join costs a token).
    app.record_user_message("and another thing");
    app.refresh_context_usage();
    assert_eq!(
        tokens(&app, Category::Messages),
        count_tokens("hello there\n\nand another thing")
    );
}

#[test]
fn a_new_tool_set_recounts_and_the_same_one_does_not() {
    let mut app = session();
    app.open_context_usage();
    let sentinel = Breakdown::default();
    app.context_usage.as_mut().expect("open").breakdown = sentinel.clone();
    // The boundary pulls the set every loop bottom: the same content is no
    // change, whatever handle it arrives in.
    app.set_tool_definitions(Arc::from(vec![tool("bash", 4_000), tool("read", 2_000)]));
    app.refresh_context_usage();
    assert_eq!(
        app.context_usage.as_ref().expect("open").breakdown,
        sentinel
    );
    // An MCP server connected.
    app.set_tool_definitions(Arc::from(vec![
        tool("bash", 4_000),
        tool("read", 2_000),
        tool("mcp__deepwiki__ask_question", 600),
    ]));
    app.refresh_context_usage();
    assert_eq!(
        tokens(&app, Category::McpTools),
        count_tokens(&"x".repeat(600))
    );
}

#[test]
fn a_highlight_whose_row_is_gone_moves_to_the_largest() {
    let mut app = session();
    app.set_context_window(Some(200_000));
    app.open_context_usage();
    app.context_usage.as_mut().expect("open").selected = ContextUsageRow::AutoCompact;
    app.settings.auto_compact = false;
    app.refresh_context_usage();
    assert_eq!(
        selected(&app),
        ContextUsageRow::Slice(Category::SystemTools)
    );
}

#[test]
fn the_highlight_follows_the_largest_slice_until_the_user_moves_it() {
    // The pure core counts the page as it opens and the boundary hands the
    // tool set over right after: a highlight that stayed on what was largest
    // a moment before the tools were known opened on the system prompt
    // (smoke Phase 144).
    let mut app = App::new();
    app.set_system_prompt(Some("# System prompt\n\nBe terse.".to_string()));
    app.record_user_message("hello there");
    app.open_context_usage();
    assert_eq!(
        selected(&app),
        ContextUsageRow::Slice(Category::SystemPrompt)
    );
    app.set_tool_definitions(Arc::from(vec![tool("bash", 4_000)]));
    app.refresh_context_usage();
    assert_eq!(
        selected(&app),
        ContextUsageRow::Slice(Category::SystemTools)
    );
    // Once the user picks a row, a recount leaves the highlight on it.
    app.on_key(key(KeyCode::Up));
    let picked = selected(&app);
    assert_eq!(picked, ContextUsageRow::Slice(Category::SystemPrompt));
    app.set_tool_definitions(Arc::from(vec![tool("bash", 8_000)]));
    app.refresh_context_usage();
    assert_eq!(selected(&app), picked);
}

#[test]
fn a_clear_recounts_to_an_empty_conversation() {
    let mut app = session();
    app.open_context_usage();
    app.clear_conversation();
    app.refresh_context_usage();
    assert_eq!(tokens(&app, Category::Messages), 0);
    assert!(
        tokens(&app, Category::SystemTools) > 0,
        "the fixed costs ride the first request regardless"
    );
}

// ===== inside an agent session view =====

#[test]
fn inside_an_agent_view_the_page_is_the_agents_window() {
    let mut app = session();
    app.set_context_window(Some(200_000));
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.set_agent_system_prompt(Some("You are a subagent.".to_string()));
    app.set_agent_tool_definitions(Arc::from(vec![tool("read", 500)]));
    app.set_agent_context_window(Some(100_000));
    app.open_agent_view("a1");
    app.open_context_usage();
    // Its own prompt, its own tools, its own transcript (its launch prompt).
    assert_eq!(
        tokens(&app, Category::SystemPrompt),
        count_tokens("You are a subagent.")
    );
    assert_eq!(
        tokens(&app, Category::SystemTools),
        count_tokens(&"x".repeat(500))
    );
    assert_eq!(
        tokens(&app, Category::Messages),
        count_tokens("What is the weather in Warsaw?")
    );
    // Its own window — and no headroom, since a subagent never compacts.
    assert_eq!(app.context_usage_window(), Some(100_000));
    assert_eq!(app.context_usage_threshold(), None);
    assert!(
        !app.context_usage_rows()
            .contains(&ContextUsageRow::AutoCompact)
    );
    // Leaving the view, the next refresh counts the lead's window again.
    app.close_agent_view();
    app.refresh_context_usage();
    assert_eq!(
        tokens(&app, Category::SystemTools),
        count_tokens(&"x".repeat(4_000)) + count_tokens(&"x".repeat(2_000))
    );
}

// ===== the usage ledgers =====

#[test]
fn usage_frames_feed_the_last_request_and_the_ledger() {
    let mut app = App::new();
    assert_eq!(app.last_usage(), None);
    assert_eq!(app.usage_since_launch(), UsageLedger::default());
    app.begin_stream();
    let first = TokenUsage {
        input: 9_000,
        output: 300,
        cached: 8_000,
        cache_write: 500,
        reasoning: 0,
    };
    let second = TokenUsage {
        input: 10_000,
        output: 200,
        cached: 9_000,
        ..TokenUsage::default()
    };
    app.apply_usage(&first);
    app.apply_usage(&second);
    assert_eq!(app.last_usage(), Some(second));
    assert_eq!(
        app.usage_since_launch(),
        UsageLedger {
            requests: 2,
            input: 19_000,
            output: 500,
            cached: 17_000,
            cache_write: 500,
        }
    );
    // A `/clear` is a new conversation, not a refund: both stay.
    app.clear_conversation();
    assert_eq!(app.last_usage(), Some(second));
    assert_eq!(app.usage_since_launch().requests, 2);
}

#[test]
fn a_subagents_frames_feed_its_own_ledger_and_the_sessions() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    let frame = TokenUsage {
        input: 4_000,
        output: 100,
        cached: 1_000,
        ..TokenUsage::default()
    };
    app.apply_agent_event("a1", &StreamEvent::Usage(frame));
    let run = app.agent("a1").expect("a1 is on the roster");
    assert_eq!(run.last_usage, Some(frame));
    assert_eq!(run.usage.requests, 1);
    assert_eq!(run.usage.input, 4_000);
    assert_eq!(
        app.usage_since_launch().input,
        4_000,
        "since launch counts every request the session made"
    );
    assert_eq!(
        app.last_usage(),
        None,
        "the lead's own last request is its own"
    );
}

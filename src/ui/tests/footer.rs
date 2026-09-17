//! The footer row, the toast, and the queued-message rows
//! (`docs/footer.md`, `docs/toast.md`, `docs/queue.md`).

use super::*;
use crate::ui::footer::queued_builds;
use crate::ui::layout::live_layout;
use crate::ui::layout::strip_rows;
use crate::ui::theme::{
    FOOTER_INDENT, SEARCH_PROMPT, error_color, footer_color, footer_focus_bg, footer_focus_fg,
    search_query_color, shell_mode_color, toast_color, toast_error_color,
};
use crate::ui::wrap::cols;

// --- live-region layout: single source of truth ---

#[test]
fn live_layout_splits_the_area_into_the_strip_box_band_and_footer() {
    // The strip takes its rows (preview + gap while streaming, none when
    // idle); the band takes its fixed rows below the box; the footer the
    // very last row; the input box takes everything left — the four always
    // tile the whole area, at the minimum height and beyond, streaming or
    // not, band open or closed, footer shown or not.
    for streaming in [false, true] {
        // 0 = no preview; 1 = a single-row preview; 2 = a running backend
        // tool's multi-row cell (wrapped header + `⎿ Running…`).
        for preview_rows in [0u16, 1, 2] {
            for band_rows in [0, 3] {
                for footer_rows in [0, 1] {
                    // The smallest height still fits the tallest strip (a
                    // 2-row preview cell → 2 + gap + status + gap = 5) +
                    // the box (LIVE_MIN_HEIGHT) + band (3) + footer (1) = 12.
                    // Below that the split holds the box's floor back and the
                    // strip gives way instead — `live_layout_never_evicts_the_box`.
                    for h in [LIVE_MIN_HEIGHT + 9, 13, 24] {
                        let [strip, input, band, footer, _] = live_layout(
                            Rect::new(0, 0, 40, h),
                            streaming,
                            preview_rows,
                            0,
                            0,
                            0,
                            band_rows,
                            footer_rows,
                            0,
                        );
                        assert_eq!(
                            strip.height + input.height + band.height + footer.height,
                            h,
                            "sub-areas tile the area"
                        );
                        assert_eq!(strip.height, strip_rows(streaming, preview_rows, 0));
                        assert_eq!(band.height, band_rows);
                        assert_eq!(footer.height, footer_rows);
                        assert!(input.height >= LIVE_MIN_HEIGHT, "the box keeps its floor");
                    }
                }
            }
        }
    }
}

// --- the permission-mode segment (docs/permissions.md) ---

#[test]
fn the_footer_pins_the_permission_mode_at_the_right_edge() {
    // The mode gets its own zone flush at the row's right edge — the
    // transcript separator's right-aligned percentage, not another ` · `
    // segment — so however long the model/cwd/gauge chain grows, truncation
    // eats the left content and never the one segment with a safety meaning.
    // With permissions disabled (no mode injected) the row keeps its old
    // shape: nothing asks, so a mode would be a lie.
    let mut app = App::new();
    app.set_session_info("kimi-k3", "~/Codes/rust/project/alter-zero");
    let text = plain(&footer_line(&app, 120));
    assert!(
        !text.contains("manual"),
        "no mode injected → no segment: {text}"
    );
    app.set_permission_mode(Some(crate::permission::PermissionMode::Manual));
    let line = footer_line(&app, 120);
    let text = plain(&line);
    assert!(
        text.starts_with("  kimi-k3 · ~/Codes/rust/project/alter-zero"),
        "the left chain keeps its old shape: {text}"
    );
    assert!(text.ends_with("manual"), "{text}");
    assert_eq!(cols(&text), 120, "flush at the right edge: {text:?}");
    // Dim like every other segment (codex's no-theme-colours status line).
    let mode_span = line.spans.last().expect("the mode span");
    assert_eq!(mode_span.content, "manual");
    assert_eq!(mode_span.style.fg, Some(footer_color()));
    // Every mode of the Shift+Tab cycle renders its label there — auto and
    // master included (docs/permissions.md).
    for (mode, label) in [
        (crate::permission::PermissionMode::Edit, "edit"),
        (crate::permission::PermissionMode::Auto, "auto"),
        (crate::permission::PermissionMode::Master, "master"),
    ] {
        app.set_permission_mode(Some(mode));
        let text = plain(&footer_line(&app, 120));
        assert!(text.ends_with(label), "{text}");
        assert_eq!(cols(&text), 120);
    }
}

#[test]
fn a_narrow_footer_truncates_the_left_content_never_the_mode() {
    // The reservation comes off the left chain's budget, so the cwd gets the
    // `…` cut while the right-edge mode survives whole, a gap still between
    // them.
    let mut app = App::new();
    app.set_session_info(
        "a-rather-long-model-name",
        "~/a/deeply/nested/working/directory",
    );
    app.set_permission_mode(Some(crate::permission::PermissionMode::Edit));
    let text = plain(&footer_line(&app, 40));
    assert_eq!(cols(&text), 40, "{text:?}");
    assert!(text.ends_with(" edit"), "the mode survives whole: {text}");
    assert!(text.contains('…'), "the left content gave way: {text}");
}

// --- the footer context gauge (docs/compact.md) ---

#[test]
fn the_footer_shows_the_context_gauge_when_the_window_is_known() {
    let mut app = App::new();
    app.set_session_info("some-model", "~/x");
    app.set_context_window(Some(300_000));
    app.begin_stream();
    app.apply_usage(&crate::stream::TokenUsage {
        input: 17_900,
        output: 100,
        cached: 0,
        cache_write: 0,
        ..crate::stream::TokenUsage::default()
    });
    let text = plain(&footer_line(&app, 120));
    assert!(text.contains("18k/300k (6.0%)"), "{text}");
}

#[test]
fn the_footer_gauge_humanizes_the_used_tokens_beside_the_window() {
    // The numerator is the live context size, humanized like the window
    // (`1.3k/160k (0.8%)`) — the raw count the percentage alone hid.
    let mut app = App::new();
    app.set_session_info("deepseek-v3.2", "~/Codes/tmp");
    app.set_context_window(Some(160_000));
    app.begin_stream();
    app.apply_usage(&crate::stream::TokenUsage {
        input: 1_250,
        output: 50,
        cached: 0,
        cache_write: 0,
        ..crate::stream::TokenUsage::default()
    });
    let text = plain(&footer_line(&app, 120));
    assert!(text.contains("1.3k/160k (0.8%)"), "{text}");
}

#[test]
fn the_footer_gauge_shows_a_small_context_bare() {
    // Under a thousand the formatter stays bare (`842`), so a fresh
    // session reads `842/160k (0.5%)` rather than `0.8k/160k`.
    let mut app = App::new();
    app.set_session_info("deepseek-v3.2", "~/Codes/tmp");
    app.set_context_window(Some(160_000));
    app.begin_stream();
    app.apply_usage(&crate::stream::TokenUsage {
        input: 800,
        output: 42,
        cached: 0,
        cache_write: 0,
        ..crate::stream::TokenUsage::default()
    });
    let text = plain(&footer_line(&app, 120));
    assert!(text.contains("842/160k (0.5%)"), "{text}");
}

#[test]
fn the_footer_omits_the_gauge_without_a_window() {
    let mut app = App::new();
    app.set_session_info("some-model", "~/x");
    let text = plain(&footer_line(&app, 120));
    assert!(!text.contains('%'), "{text}");
}

#[test]
fn an_agent_session_view_gauges_that_agents_own_context() {
    // The reported bug (docs/agent-context-gauge.md): inside a subagent's
    // session view the footer read `23.7k/1M` — the LEAD's context — under a
    // roster row saying the agent itself was at 64.9k. The gauge follows the
    // screen: the viewed agent's own `input + output`, against its window.
    let mut app = App::new();
    app.set_session_info("kimi-k3", "~");
    app.set_context_window(Some(1_000_000));
    app.begin_stream();
    app.apply_usage(&crate::stream::TokenUsage {
        input: 23_000,
        output: 700,
        ..crate::stream::TokenUsage::default()
    });
    app.start_agent_group(
        false,
        &[spec("a1", "Look up linuztx GitHub profile", false)],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Usage(crate::stream::TokenUsage {
            input: 64_000,
            output: 900,
            ..crate::stream::TokenUsage::default()
        }),
    );
    let main = plain(&footer_line(&app, 120));
    assert!(main.contains("23.7k/1M (2.4%)"), "{main}");
    app.set_agent_context_window(Some(1_000_000));
    app.open_agent_view("a1");
    let view = plain(&footer_line(&app, 120));
    assert!(view.contains("64.9k/1M (6.5%)"), "the agent's own: {view}");
    assert!(!view.contains("23.7k"), "never the lead's: {view}");
    // Back in the main view the lead's gauge is back.
    app.close_agent_view();
    let back = plain(&footer_line(&app, 120));
    assert!(back.contains("23.7k/1M (2.4%)"), "{back}");
}

#[test]
fn an_agent_pinned_to_another_model_names_it_in_its_views_footer() {
    // A type whose definition pins `model:` runs on that model
    // (`docs/subagents.md`), so its view's footer names it; an inheriting
    // type keeps the session's name.
    // The session's thinking mode goes with the session's model: the launch
    // drops it beside the model it replaces, so the footer must not claim it.
    use crate::llm::{ReasoningEffort, ReasoningSupport, ThinkingMode};
    let mut app = App::new();
    app.set_session_info("kimi-k3", "~");
    app.set_thinking(Some((
        ReasoningSupport {
            efforts: vec![ReasoningEffort::Medium],
            can_disable: true,
            default_effort: None,
        },
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Review the diff", false)]);
    app.set_agent_model(Some("deepseek-v3.2".to_string()));
    app.open_agent_view("a1");
    let view = plain(&footer_line(&app, 120));
    assert!(view.starts_with("  deepseek-v3.2 · ~"), "{view}");
    assert!(
        !view.contains("medium"),
        "no mode beside a pinned model: {view}"
    );
    app.set_agent_model(None);
    let inherit = plain(&footer_line(&app, 120));
    assert!(inherit.starts_with("  kimi-k3 medium · ~"), "{inherit}");
}

// --- message queue (docs/queue.md) ---

#[test]
fn steered_messages_show_above_the_box_until_the_turn_takes_them() {
    // A message handed to the running turn reads exactly like a queued one —
    // it is pending either way; what differs is which turn it belongs to.
    let mut app = App::new();
    app.steered.push_back("also check the tests".to_string());
    assert_eq!(queued_rows(&app, 40), 1);
    assert!(
        plain(&queued_lines(&app, 40)[0]).contains("❯ also check the tests"),
        "the user-message style, inset"
    );
}

#[test]
fn steered_messages_lead_the_follow_up_turns() {
    // They belong to the turn already running, so they are what happens next
    // — a Tab follow-up comes after, divided by the usual blank row.
    let mut app = App::new();
    app.steered.push_back("now".to_string());
    app.queued.push_back(batch(&["later"]));
    let rows: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    assert_eq!(rows.len(), 3, "one each + the divider");
    assert!(rows[0].contains("❯ now"), "{rows:?}");
    assert!(rows[1].trim().is_empty(), "{rows:?}");
    assert!(rows[2].contains("❯ later"), "{rows:?}");
}

#[test]
fn an_agent_session_view_shows_that_agents_own_queue() {
    // The agent view shows the agent's world (docs/agent-tool.md): its own
    // pending messages, never the main session's follow-ups.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Manila weather", false)]);
    app.queued.push_back(batch(&["a main follow-up"]));
    app.queue_agent_chat("a1", "also check Manila");
    app.open_agent_view("a1");
    let rows: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("❯ also check Manila"), "{rows:?}");
    assert_eq!(queued_rows(&app, 40), 1);
}

#[test]
fn an_agent_session_view_shows_its_steered_rows_over_its_follow_ups() {
    // The main view's shape one level down: what the running loop reads next
    // leads, the Tab follow-ups come after, a blank row dividing each turn
    // (docs/queue.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Manila weather", false)]);
    app.queued.push_back(batch(&["a main follow-up"]));
    app.queue_agent_chat("a1", "now");
    app.open_agent_view("a1");
    app.queue_agent_followup("later");
    let rows: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    assert_eq!(rows.len(), 3, "one each + the divider: {rows:?}");
    assert!(rows[0].contains("\u{276f} now"), "{rows:?}");
    assert!(rows[1].trim().is_empty(), "{rows:?}");
    assert!(rows[2].contains("\u{276f} later"), "{rows:?}");
    assert_eq!(queued_rows(&app, 40), 3);
}

#[test]
fn an_agent_session_view_with_nothing_queued_reserves_no_rows() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Manila weather", false)]);
    app.queued.push_back(batch(&["a main follow-up"]));
    app.steered.push_back("and a steered one".to_string());
    app.open_agent_view("a1");
    assert_eq!(queued_rows(&app, 40), 0, "the main session's stay off it");
}

#[test]
fn the_pending_rows_are_built_once_per_frame_not_once_per_caller() {
    // `queued_lines` is reached six or seven times a draw — `live_height`,
    // `preview_budget`, `cursor_position`, `render_live`'s own layout and its
    // paint — and each build word-wraps and styles the WHOLE backlog. With
    // the frame chain re-arming every 32 ms that was ~200 full re-renders a
    // second for rows that never changed: "pressing Tab with a message again
    // and again lags the TUI" (`docs/queue.md`). The memo makes the repeats
    // free, and a real change still rebuilds.
    let mut app = App::new();
    app.begin_stream();
    for i in 0..20 {
        app.queued
            .push_back(batch(&[&format!("queued message number {i}")]));
    }
    let before = queued_builds();
    let rows = queued_rows(&app, 40);
    for _ in 0..6 {
        assert_eq!(queued_rows(&app, 40), rows);
    }
    let lines = queued_lines(&app, 40);
    assert_eq!(lines.len(), usize::from(rows), "rows and lines agree");
    assert_eq!(
        queued_builds() - before,
        1,
        "one build served every caller of the frame"
    );
    // A queued message is a change; so is a resize.
    app.queued.push_back(batch(&["one more"]));
    assert_eq!(
        queued_rows(&app, 40),
        rows + 2,
        "the new entry + its divider"
    );
    assert_eq!(queued_builds() - before, 2, "the change rebuilt");
    let _ = queued_lines(&app, 30);
    assert_eq!(queued_builds() - before, 3, "a new width rebuilt");
}

#[test]
fn the_memo_follows_the_agent_session_view() {
    // Same queues, a different conversation on screen: the memo must not
    // serve the main session's rows into an agent view (or the reverse).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Manila weather", false)]);
    app.queued.push_back(batch(&["a main follow-up"]));
    app.queue_agent_chat("a1", "an agent one");
    let main_rows: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    app.open_agent_view("a1");
    let agent_rows: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    app.close_agent_view();
    let back: Vec<String> = queued_lines(&app, 40).iter().map(plain).collect();
    assert!(main_rows[0].contains("a main follow-up"), "{main_rows:?}");
    assert!(agent_rows[0].contains("an agent one"), "{agent_rows:?}");
    assert_eq!(back, main_rows, "the return serves the main rows again");
}

#[test]
fn queued_rows_is_zero_empty_and_counts_the_queue() {
    let mut app = App::new();
    assert_eq!(queued_rows(&app, 40), 0);
    app.queued.push_back(batch(&["a", "b"]));
    assert_eq!(queued_rows(&app, 40), 2, "one short message per row");
}

#[test]
fn queued_rows_counts_the_blank_between_batches() {
    // Two single-message batches occupy three rows — a blank divides them.
    let mut app = App::new();
    app.queued.push_back(batch(&["a"]));
    app.queued.push_back(batch(&["b"]));
    assert_eq!(queued_rows(&app, 40), 3);
}

#[test]
fn queued_lines_separate_batches_with_a_blank_row() {
    // Tab-opened batches read as separate turns: a blank row sits between
    // each batch, grouping the follow-ups apart from the first queue.
    let mut app = App::new();
    app.queued.push_back(batch(&["first"]));
    app.queued.push_back(batch(&["later"]));
    let lines = queued_lines(&app, 40);
    assert_eq!(lines.len(), 3, "two single-message batches + one separator");
    assert!(
        plain(&lines[0]).contains("❯ first"),
        "{:?}",
        plain(&lines[0])
    );
    assert_eq!(
        plain(&lines[1]).trim(),
        "",
        "a blank separator row divides the batches"
    );
    assert!(
        plain(&lines[2]).contains("❯ later"),
        "{:?}",
        plain(&lines[2])
    );
}

#[test]
fn queued_lines_render_a_shell_entry_with_the_red_bang_prompt() {
    // A queued !command renders like the exec cell it becomes: the red `! `
    // Role::Shell header (not the ❯ user bullet), inset two columns.
    let mut app = App::new();
    app.queued.push_back(QueuedTurn::Shell("ls -la".into()));
    let lines = queued_lines(&app, 40);
    let expected = message_lines(Role::Shell, "ls -la", 38); // 40 minus the indent
    assert_eq!(lines.len(), expected.len());
    assert!(
        plain(&lines[0]).contains("! ls -la"),
        "the red shell prompt, not ❯: {:?}",
        plain(&lines[0])
    );
    assert_eq!(
        lines[0].spans[0].content.as_ref(),
        "  ",
        "inset two columns, the indent outside the dark block"
    );
}

#[test]
fn queued_lines_divide_a_text_batch_and_a_shell_entry() {
    // A text batch and a shell entry are separate turns: a blank row divides
    // them, the text keeping its ❯ bullet and the command its red `! `.
    let mut app = App::new();
    app.queued.push_back(batch(&["hello"]));
    app.queued.push_back(QueuedTurn::Shell("ls".into()));
    let lines = queued_lines(&app, 40);
    assert_eq!(lines.len(), 3, "message + blank divider + shell");
    assert!(
        plain(&lines[0]).contains("❯ hello"),
        "{:?}",
        plain(&lines[0])
    );
    assert_eq!(plain(&lines[1]).trim(), "", "a blank divider");
    assert!(plain(&lines[2]).contains("! ls"), "{:?}", plain(&lines[2]));
}

#[test]
fn queued_rows_count_wrapped_lines() {
    // A queued message wraps like a user message, so a long one is >1 row.
    let mut app = App::new();
    app.queued
        .push_back(batch(&["one two three four five six seven eight"]));
    assert!(queued_rows(&app, 16) >= 2, "a long queued message wraps");
}

#[test]
fn queued_rows_are_uncapped_every_message_counts() {
    // No display cap (codex shows the whole backlog): ten queued messages
    // are ten rows.
    let mut app = App::new();
    app.queued.push_back(QueuedTurn::Messages {
        texts: (0..10).map(|i| format!("m{i}")).collect(),
        images: Vec::new(),
    });
    assert_eq!(queued_rows(&app, 40), 10);
}

#[test]
fn queued_lines_indent_two_columns_and_keep_the_user_style() {
    // The queue is inset two columns from the strip's left edge; past the
    // indent each message is exactly a user message (❯ bullet, dark
    // background) wrapped to the remaining width — and the indent itself
    // stays *outside* the dark block.
    let mut app = App::new();
    app.queued.push_back(batch(&["world"]));
    let lines = queued_lines(&app, 40);
    let expected = message_lines(Role::User, "world", 38); // 40 minus the indent
    assert_eq!(lines.len(), expected.len());
    assert_eq!(plain(&lines[0]), format!("  {}", plain(&expected[0])));
    let indent = &lines[0].spans[0];
    assert_eq!(indent.content.as_ref(), "  ");
    assert_eq!(
        indent.style.bg, None,
        "the indent sits outside the dark block"
    );
    assert!(
        lines[0].spans[1..].iter().all(|s| s.style.bg.is_some()),
        "past the indent the user-message background holds: {lines:?}"
    );
}

#[test]
fn queued_lines_list_every_message_with_the_user_bullet() {
    let mut app = App::new();
    app.queued.push_back(batch(&["world", "again"]));
    let all: String = queued_lines(&app, 40)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("  ❯ world"), "{all:?}");
    assert!(all.contains("  ❯ again"), "{all:?}");
}

#[test]
fn queued_lines_wrap_a_long_message_across_rows() {
    let mut app = App::new();
    app.queued
        .push_back(batch(&["alpha beta gamma delta epsilon"]));
    let lines = queued_lines(&app, 18);
    assert!(lines.len() >= 2, "a long queued message wraps: {lines:?}");
    assert!(
        lines.iter().all(|l| plain(l).starts_with("  ")),
        "wrapped continuation rows carry the indent too: {lines:?}"
    );
}

#[test]
fn queued_lines_list_the_whole_backlog_uncapped() {
    let mut app = App::new();
    app.queued.push_back(QueuedTurn::Messages {
        texts: (0..10).map(|i| format!("m{i}")).collect(),
        images: Vec::new(),
    });
    let lines = queued_lines(&app, 40);
    assert_eq!(lines.len(), 10, "every queued message shows");
    assert!(plain(&lines[9]).contains("❯ m9"), "{:?}", plain(&lines[9]));
}

#[test]
fn footer_rows_is_zero_without_session_info_and_one_with_it() {
    assert_eq!(footer_rows(&App::new(), 0), 0, "no session info → no row");
    assert_eq!(footer_rows(&with_session(), 0), 1);
}

#[test]
fn footer_rows_yields_to_an_open_band() {
    // codex's popups / shortcut overlay take the footer's place; our
    // palette and `?` shortcuts band displace it the same way.
    assert_eq!(footer_rows(&with_session(), 3), 0);
}

#[test]
fn footer_line_shows_model_and_cwd_dim_behind_the_indent() {
    let line = footer_line(&with_session(), 60);
    assert_eq!(plain(&line), "  dummy_model_name · ~/alter-zero");
    // spans = [indent, model, separator, cwd] — every segment dim (codex's
    // no-theme-colours status line), the indent unstyled.
    assert_eq!(line.spans[0].style.fg, None);
    for span in &line.spans[1..] {
        assert_eq!(
            span.style.fg,
            Some(footer_color()),
            "dim: {:?}",
            span.content
        );
    }
}

#[test]
fn footer_line_truncates_with_an_ellipsis_when_narrow() {
    let line = footer_line(&with_session(), 20);
    let text = plain(&line);
    assert!(cols(&text) <= 20, "fits the width: {text:?}");
    assert!(text.ends_with('…'), "cut is visible: {text:?}");
    assert!(text.starts_with("  dummy_model"), "head kept: {text:?}");
}

#[test]
fn footer_line_shows_the_thinking_mode_beside_the_model() {
    // A reasoning-capable model carries its mode right after the model
    // name — `{model} {mode} · {cwd}` — so the current thinking level is
    // always visible (docs/reasoning.md).
    use crate::llm::{ReasoningEffort, ReasoningSupport, ThinkingMode};
    let mut app = with_session();
    app.set_thinking(Some((
        ReasoningSupport {
            efforts: vec![ReasoningEffort::Medium],
            can_disable: true,
            default_effort: None,
        },
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    let line = footer_line(&app, 60);
    assert_eq!(plain(&line), "  dummy_model_name medium · ~/alter-zero");
    for span in &line.spans[1..] {
        assert_eq!(
            span.style.fg,
            Some(footer_color()),
            "dim: {:?}",
            span.content
        );
    }
    // Off is a mode too — the user must see thinking is disabled.
    app.thinking.as_mut().unwrap().mode = ThinkingMode::Off;
    assert_eq!(
        plain(&footer_line(&app, 60)),
        "  dummy_model_name off · ~/alter-zero"
    );
}

#[test]
fn display_cwd_relativizes_home_to_a_tilde() {
    let home = Path::new("/home/user");
    assert_eq!(display_cwd(home, Some(home)), "~");
    assert_eq!(
        display_cwd(Path::new("/home/user/code/tui"), Some(home)),
        "~/code/tui"
    );
}

#[test]
fn display_cwd_outside_home_or_without_one_stays_absolute() {
    let home = Path::new("/home/user");
    assert_eq!(
        display_cwd(Path::new("/etc/nginx"), Some(home)),
        "/etc/nginx"
    );
    assert_eq!(display_cwd(Path::new("/srv/app"), None), "/srv/app");
    assert_eq!(
        display_cwd(Path::new("/home/username"), Some(home)),
        "/home/username",
        "component-wise, not a string prefix"
    );
}

#[test]
fn the_footer_stays_while_a_turn_streams() {
    let mut app = with_session();
    app.begin_stream();
    app.push_chunk("hello");
    let h = live_height(&app.input, 60, 24, true, 1, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, h - 1, 60).contains("dummy_model_name"),
        "the footer is ambient — present mid-turn too"
    );
}

// --- the transient toast (docs/toast.md) ---

#[test]
fn toast_rows_is_zero_without_a_toast_and_one_with_it() {
    let mut app = App::new();
    assert_eq!(toast_rows(&app), 0);
    app.show_toast("Copied last message to clipboard", ToastKind::Info);
    assert_eq!(toast_rows(&app), 1);
}

#[test]
fn toast_line_colors_info_dim_and_error_red() {
    let mut app = App::new();
    app.show_toast("ok", ToastKind::Info);
    assert_eq!(toast_line(&app, 40).spans[1].style.fg, Some(toast_color()));
    app.show_toast("bad", ToastKind::Error);
    assert_eq!(
        toast_line(&app, 40).spans[1].style.fg,
        Some(toast_error_color())
    );
}

#[test]
fn toast_line_truncates_with_an_ellipsis_when_narrow() {
    let mut app = App::new();
    app.show_toast(
        "a very long toast message that overflows the width",
        ToastKind::Info,
    );
    let text = plain(&toast_line(&app, 12));
    assert!(text.ends_with('…'), "truncated: {text:?}");
    assert!(cols(&text) <= 12, "fits the width: {text:?}");
}

#[test]
fn the_cursor_stays_put_when_the_footer_shows() {
    // The footer is reserved *below* the box, so injecting session info
    // must not move the cursor.
    let mut app = App::new();
    app.input = TextArea::from_text("hi");
    let bare_h = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 0, 0);
    let bare = cursor_position(Rect::new(0, 0, 40, bare_h), &app);
    app.set_session_info("dummy_model_name", "~/alter-zero");
    let footer_h = live_height(&app.input, 40, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let with_footer = cursor_position(Rect::new(0, 0, 40, footer_h), &app);
    assert_eq!(with_footer, bare, "cursor unchanged by the footer row");
}

#[test]
fn search_rows_take_the_footer_slot_even_without_session_info() {
    let app = searching(&[], "");
    assert_eq!(
        footer_rows(&app, 0),
        1,
        "the search line must show even when no session info is injected"
    );
}

#[test]
fn the_search_line_displaces_the_session_footer() {
    let mut app = searching(&["git status"], "git");
    app.set_session_info("dummy_model_name", "~/alter-zero");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let last = row(&buf, h - 1, 60);
    assert!(
        last.starts_with("  reverse-i-search: git"),
        "the query line sits in the footer slot: {last:?}"
    );
    assert!(
        !last.contains("dummy_model_name"),
        "the session footer is displaced: {last:?}"
    );
}

#[test]
fn the_search_line_shows_accept_hints_on_a_match() {
    let line = search_line(
        searching(&["git status"], "git")
            .history_search
            .as_ref()
            .unwrap(),
    );
    let text = plain(&line);
    assert_eq!(text, "  reverse-i-search: git  enter accept · esc cancel");
    // The query is cyan and the hint keys are cyan+bold, the rest dim
    // (codex's history_search_footer_line styling).
    let query = &line.spans[2];
    assert_eq!(query.content.as_ref(), "git");
    assert_eq!(query.style.fg, Some(search_query_color()));
    let enter = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "enter")
        .expect("enter key span");
    assert_eq!(enter.style.fg, Some(search_query_color()));
    assert!(enter.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn the_search_line_shows_no_match_in_red() {
    let line = search_line(
        searching(&["git status"], "zzz")
            .history_search
            .as_ref()
            .unwrap(),
    );
    assert_eq!(plain(&line), "  reverse-i-search: zzz  no match");
    let no_match = line.spans.last().unwrap();
    assert_eq!(no_match.style.fg, Some(error_color()));
}

#[test]
fn the_search_line_is_bare_while_idle() {
    let line = search_line(
        searching(&["git status"], "")
            .history_search
            .as_ref()
            .unwrap(),
    );
    assert_eq!(plain(&line), "  reverse-i-search: ");
}

#[test]
fn the_cursor_sits_at_the_end_of_the_query_in_the_search_line() {
    let app = searching(&["git status"], "git");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let area = Rect::new(0, 0, 60, h);
    let (x, y) = cursor_position(area, &app);
    assert_eq!(y, h - 1, "on the footer row, not in the textarea");
    let expected = cols(FOOTER_INDENT) + cols(SEARCH_PROMPT) + cols("git");
    assert_eq!(x as usize, expected);
}

#[test]
fn shell_mode_takes_the_footer_slot_even_without_session_info() {
    assert_eq!(
        footer_rows(&shelling("ls"), 0),
        1,
        "the Shell mode hint shows even with no session info"
    );
    assert_eq!(footer_rows(&App::new(), 0), 0, "not in shell mode");
}

#[test]
fn the_shell_mode_line_displaces_the_session_footer() {
    let mut app = shelling("ls -la");
    app.set_session_info("dummy_model_name", "~/alter-zero");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let last = row(&buf, h - 1, 60);
    assert!(last.starts_with("  Shell mode"), "the hint shows: {last:?}");
    assert!(
        !last.contains("dummy_model_name"),
        "the session footer is displaced: {last:?}"
    );
}

#[test]
fn the_shell_mode_line_is_red() {
    let line = shell_mode_line();
    assert_eq!(plain(&line), "  Shell mode");
    let label = line.spans.last().unwrap();
    assert_eq!(label.content.as_ref(), "Shell mode");
    assert_eq!(label.style.fg, Some(shell_mode_color()));
}

#[test]
fn shell_mode_swaps_the_composer_prompt_for_a_red_bang() {
    // The absorbed `!` renders back as the prompt: `! pwd`, not `❯ pwd`.
    let app = shelling("pwd");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(row(&buf, 1, 60).trim_end(), "! pwd");
    assert_eq!(
        buf[(0, 1)].fg,
        shell_mode_color(),
        "the bang prompt is red, the shell accent"
    );
}

#[test]
fn the_cursor_stays_in_the_box_in_shell_mode() {
    // Unlike the Ctrl+R search (which owns the footer cursor), shell mode
    // keeps the cursor on the composer's command line.
    let app = shelling("ls");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 1, 0);
    let area = Rect::new(0, 0, 60, h);
    let (_, y) = cursor_position(area, &app);
    assert!(y < h - 1, "cursor is in the box, not on the footer row");
}

#[test]
fn footer_rows_reserves_the_slot_while_primed() {
    // Like the Ctrl+R search line, the hint shows even with no session
    // info injected — the slot exists whenever the gesture is armed.
    let mut app = App::new();
    assert_eq!(footer_rows(&app, 0), 0);
    app.backtrack.primed = true;
    assert_eq!(footer_rows(&app, 0), 1);
}

#[test]
fn backtrack_hint_line_names_the_second_esc() {
    let line = backtrack_hint_line();
    assert_eq!(
        plain(&line),
        format!("{FOOTER_INDENT}esc again to edit previous message"),
    );
    // Spans: indent, the bold-cyan key, the dim label (the search-hint
    // styling).
    assert_eq!(line.spans[1].style.fg, Some(search_query_color()));
    assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(line.spans[2].style.fg, Some(footer_color()));
}

#[test]
fn the_footer_appends_the_running_shell_count() {
    let mut app = App::new();
    app.set_session_info("kimi-k2", "~/repo");
    assert_eq!(
        plain(&footer_line(&app, 80)).trim_end(),
        "  kimi-k2 · ~/repo"
    );
    app.bg_started("bash_1", "ping x.com", None, true, None);
    assert_eq!(
        plain(&footer_line(&app, 80)).trim_end(),
        "  kimi-k2 · ~/repo · 1 shell"
    );
    app.bg_started("bash_2", "ping y.com", None, true, None);
    assert_eq!(
        plain(&footer_line(&app, 80)).trim_end(),
        "  kimi-k2 · ~/repo · 2 shells"
    );
}

#[test]
fn the_focused_footer_shell_count_lights_up_on_cyan() {
    let mut app = App::new();
    app.set_session_info("kimi-k2", "~/repo");
    app.bg_started("bash_1", "ping x.com", None, true, None);
    let shell_span = |line: &Line<'static>| {
        line.spans
            .iter()
            .find(|s| s.content.contains("shell"))
            .expect("the footer carries a shell segment")
            .clone()
    };
    let idle = shell_span(&footer_line(&app, 80));
    assert_eq!(idle.style.bg, None, "unfocused it stays dim like the rest");
    assert_eq!(idle.style.fg, Some(footer_color()));
    // ↓ focuses the indicator: only that segment lights up — the model,
    // cwd and gauge segments keep their text and their dim styling.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let focused = footer_line(&app, 80);
    assert_eq!(
        plain(&focused).trim_end(),
        "  kimi-k2 · ~/repo · 1 shell",
        "the other footer segments stay put"
    );
    let lit = shell_span(&focused);
    assert_eq!(lit.style.bg, Some(footer_focus_bg()));
    assert_eq!(lit.style.fg, Some(footer_focus_fg()));
    assert!(
        focused
            .spans
            .iter()
            .filter(|s| !s.content.contains("shell"))
            .all(|s| s.style.bg.is_none()),
        "nothing else is highlighted"
    );
}

#[test]
fn the_roster_selection_marks_rows_and_swaps_the_footer_hint() {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            agent_type: "general-purpose".into(),
            prompt: "warsaw?".into(),
            background: false,
        }],
    );
    // ↓ opens the selection on `● main`.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
    assert!(texts[1].starts_with("❯ ● main"), "{}", texts[1]);
    assert_eq!(
        plain(&agent_hint_line(&app)),
        "  ↑/↓ to select · Enter to view"
    );
    assert_eq!(footer_rows(&app, 0), 1, "the hint takes the footer slot");
    // ↓ moves onto the agent row; the hint gains the stop key.
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
    assert!(texts[2].starts_with("❯ ◯ "), "{}", texts[2]);
    assert_eq!(plain(&agent_hint_line(&app)), "  Enter to view · x to stop");
    // Once that agent has settled (here: the user's own `x`), the same key
    // means something else — the red row is cleared, not stopped again.
    app.stop_agent("a1");
    assert_eq!(
        plain(&agent_hint_line(&app)),
        "  Enter to view · x to clear"
    );
}

// --- the `!` shell mode + its exec cell (docs/shell-command.md) ---

/// An app in shell mode with `command` typed (the bang absorbed into the
/// mode flag, codex-style — the textarea holds just the command).
fn shelling(command: &str) -> App {
    let mut app = App::new();
    app.shell_mode = true;
    app.input = TextArea::from_text(command);
    app
}

#[test]
fn the_queued_memo_rebuilds_when_the_theme_changes() {
    // The pending rows are styled user bubbles, so the memo's fingerprint
    // carries the active theme: a `/theme` switch repaints them rather than
    // serving rows in the old palette (`docs/theme.md`).
    use crate::app::Theme;
    use crate::ui::palette::{palette_of, with_theme};
    let mut app = App::new();
    app.queued.push_back(batch(&["also check the tests"]));
    // The bubble's ground rides the content spans (the indent stays bare).
    let ground = |app: &App| queued_lines(app, 40)[0].spans[1].style.bg;
    assert_eq!(ground(&app), Some(palette_of(Theme::Mocha).user_bg));
    assert_eq!(
        with_theme(Theme::Latte, || ground(&app)),
        Some(palette_of(Theme::Latte).user_bg),
        "the bubble repainted on Latte's pale ground"
    );
    assert_eq!(
        ground(&app),
        Some(palette_of(Theme::Mocha).user_bg),
        "…and back"
    );
}

#[test]
fn footer_line_shows_the_speed_tier_beside_the_thinking_mode() {
    // A selected speed tier wears its name right after the thinking mode —
    // `{model} {mode} {tier} · {cwd}` — so fast mode is always visible
    // (docs/fast-mode.md); standard shows nothing.
    use crate::llm::{ReasoningEffort, ReasoningSupport, ServiceTier, SpeedState, ThinkingMode};
    let mut app = with_session();
    app.set_thinking(Some((
        ReasoningSupport {
            efforts: vec![ReasoningEffort::Medium],
            can_disable: true,
            default_effort: None,
        },
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    let fast = ServiceTier::new("priority", "Fast", "1.5x speed, increased usage");
    app.set_speed(SpeedState::new(vec![fast], None));
    assert_eq!(
        plain(&footer_line(&app, 60)),
        "  dummy_model_name medium · ~/alter-zero",
        "standard: no tier word"
    );
    app.speed.as_mut().unwrap().toggle("priority");
    let line = footer_line(&app, 60);
    assert_eq!(
        plain(&line),
        "  dummy_model_name medium fast · ~/alter-zero"
    );
    for span in &line.spans[1..] {
        assert_eq!(
            span.style.fg,
            Some(footer_color()),
            "dim: {:?}",
            span.content
        );
    }
    // Without a thinking mode the tier still follows the model name.
    app.set_thinking(None);
    assert_eq!(
        plain(&footer_line(&app, 60)),
        "  dummy_model_name fast · ~/alter-zero"
    );
    // Whatever tier a record lists wears its own name there — the word is
    // the selection's label, so a new tier needs nothing of the footer.
    let ultrafast = ServiceTier::new("ultrafast", "Ultrafast", "The fastest available responses.");
    app.set_speed(SpeedState::new(
        vec![
            ServiceTier::new("priority", "Fast", "1.5x speed, increased usage"),
            ultrafast,
        ],
        Some("ultrafast".to_string()),
    ));
    assert_eq!(
        plain(&footer_line(&app, 60)),
        "  dummy_model_name ultrafast · ~/alter-zero"
    );
}

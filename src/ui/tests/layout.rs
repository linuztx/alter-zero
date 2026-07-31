//! Live-region geometry: height, re-pin, and the cursor seat.

use super::*;
use crate::ui::theme::{
    LOGIN_KEY_ROWS, MENU_MAX_ROWS, STREAM_PREVIEW_MIN_ROWS, STREAM_PREVIEW_RESERVED_ROWS,
};

#[test]
fn preview_rows_reports_the_injected_stream_preview_height() {
    // Only the boundary's StreamRender knows the multi-row preview's height,
    // so it injects the count (`set_stream_preview_rows`, the
    // set_status_times pattern) and `preview_rows` reserves it — 1 (the
    // single-row preview) until a draw injects otherwise, 0 when idle.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("| a | b |\n|---|---|\n| 1 | 2 |");
    assert_eq!(preview_rows(&app, 40), 1, "default: the single-row preview");
    app.set_stream_preview_rows(5);
    assert_eq!(preview_rows(&app, 40), 5, "the injected height is reserved");
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(
        preview_rows(&app, 40),
        0,
        "idle reserves nothing, however stale the injected count"
    );
}

#[test]
fn stream_preview_max_rows_leaves_room_for_the_chrome() {
    // The cap the boundary passes to `preview`: the screen minus the strip
    // gaps + status + box + footer chrome, floored so a tiny terminal still
    // previews a few rows.
    assert_eq!(
        stream_preview_max_rows(24),
        usize::from(24 - STREAM_PREVIEW_RESERVED_ROWS)
    );
    assert_eq!(stream_preview_max_rows(4), STREAM_PREVIEW_MIN_ROWS);
}

#[test]
fn cursor_sits_after_the_prompt_and_input() {
    // Idle (the only time the cursor shows): the box fills the area, so on a
    // 40x4 area the text row sits at row 1, flush-left (no side border).
    // Empty input → cursor right after "❯ ".
    let mut app = App::new();
    assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), &app), (2, 1));
    app.input = TextArea::from_text("hi");
    assert_eq!(cursor_position(Rect::new(0, 0, 40, 4), &app), (4, 1));
}

// --- growing input box: height + re-pin geometry ---

#[test]
fn live_height_is_minimal_for_short_input() {
    // Idle, empty or one-line input → a one-row box framed by two rules
    // (no preview strip) = LIVE_MIN_HEIGHT (3).
    assert_eq!(LIVE_MIN_HEIGHT, 3);
    assert_eq!(
        live_height(&TextArea::from_text(""), 40, 24, false, 0, 0, 0, 0, 0, 0),
        LIVE_MIN_HEIGHT
    );
    assert_eq!(
        live_height(&TextArea::from_text("hi"), 40, 24, false, 0, 0, 0, 0, 0, 0),
        LIVE_MIN_HEIGHT
    );
}

#[test]
fn live_height_adds_the_streaming_strip_above_the_box() {
    // While streaming, the live region gains a preview row, a blank gap row,
    // the live status row, and a blank gap below it (1 preview + GAP_ROWS
    // + STATUS_ROWS + STATUS_GAP_ROWS = 4) above whatever the idle box would be.
    for input in ["", "hi", "a\nb\nc"] {
        let ta = TextArea::from_text(input);
        assert_eq!(
            live_height(&ta, 40, 24, true, 1, 0, 0, 0, 0, 0),
            live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0) + 4,
            "streaming adds the preview + gap + status + gap rows for {input:?}"
        );
    }
}

#[test]
fn live_height_grows_one_row_per_wrapped_input_line() {
    // Idle, three explicit lines → the box has three text rows, so the live
    // region is 2 (two rules) + 3 = 5 rows tall.
    assert_eq!(
        live_height(
            &TextArea::from_text("a\nb\nc"),
            40,
            24,
            false,
            0,
            0,
            0,
            0,
            0,
            0
        ),
        5
    );
}

#[test]
fn live_height_grows_when_a_long_line_soft_wraps() {
    // No explicit newline: a line longer than the field width wraps and the
    // box still grows. field width = 10 - 2 = 8, so 16 columns fill two rows
    // exactly and the wrap reserves the sentinel row for the end-of-text
    // cursor (docs/textarea.md) → 3 rows → 5.
    assert_eq!(
        live_height(
            &TextArea::from_text("abcdefghijklmnop"),
            10,
            24,
            false,
            0,
            0,
            0,
            0,
            0,
            0
        ),
        5
    );
}

#[test]
fn live_height_is_clamped_to_the_terminal_height() {
    let many = TextArea::from_text(&"a\n".repeat(50));
    assert_eq!(
        live_height(&many, 40, 10, false, 0, 0, 0, 0, 0, 0),
        10,
        "never taller than the screen"
    );
}

#[test]
fn repin_keeps_the_box_top_anchored_growing_downward() {
    // Room below: grow in place, top fixed, no scroll, nothing to clear.
    assert_eq!(
        repin(3, 4, 6, 24),
        Repin {
            scroll_up: 0,
            top: 3,
            clear_below: 0
        }
    );
    // Unchanged height that already fits is a no-op.
    assert_eq!(
        repin(10, 5, 5, 24),
        Repin {
            scroll_up: 0,
            top: 10,
            clear_below: 0
        }
    );
}

#[test]
fn repin_clears_below_on_a_shrink_and_leaves_the_top_put() {
    // Shrinking pulls the bottom up; the two vacated rows below get blanked.
    assert_eq!(
        repin(3, 6, 4, 24),
        Repin {
            scroll_up: 0,
            top: 3,
            clear_below: 2
        }
    );
}

#[test]
fn repin_scrolls_up_only_when_the_box_overflows_the_bottom() {
    // At the bottom (top 20 + new height 6 = 26 > 24): scroll up 2 and pin.
    assert_eq!(
        repin(20, 4, 6, 24),
        Repin {
            scroll_up: 2,
            top: 18,
            clear_below: 0
        }
    );
}

#[test]
fn region_is_modal_only_while_a_permission_prompt_is_open() {
    // The one inline view that covers the conversation instead of scrolling
    // it away (docs/permissions.md) — every other band/picker is small enough
    // to grow the region the ordinary way.
    let mut app = App::new();
    assert!(!region_is_modal(&app));
    app.open_permission(crate::permission::PermissionRequest {
        id: "perm_0".to_string(),
        kind: crate::permission::PermissionKind::Write,
        target: "hello.py".to_string(),
        body: "1 print(\"hi\")".to_string(),
        detail: None,
        agent: None,
    });
    assert!(region_is_modal(&app));
}

#[test]
fn repin_modal_grows_upward_instead_of_scrolling_the_conversation_away() {
    // The composer sits flush at the bottom (20 + 4 = 24) when a permission
    // prompt makes the region 12 rows tall. The ordinary `repin` scrolls 8
    // rows of chat off the top into scrollback — gone from the screen for
    // good, so the collapse back to the composer leaves 8 blank rows under
    // the box. The modal **covers** them instead: no scroll, bottom put.
    assert_eq!(
        repin_modal(20, 4, 12, 24),
        Repin {
            scroll_up: 0,
            top: 12,
            clear_below: 0
        }
    );
}

#[test]
fn repin_modal_takes_the_free_rows_below_before_covering_anything() {
    // Early in a session the box sits high with free rows beneath it: the
    // prompt grows downward in place first (invariant 3), covering nothing.
    assert_eq!(
        repin_modal(3, 4, 12, 24),
        Repin {
            scroll_up: 0,
            top: 3,
            clear_below: 0
        }
    );
    // Taller than the room below (7 + 22 > 24): it takes all of it, then
    // covers just the one row it still needs.
    assert_eq!(
        repin_modal(3, 4, 22, 24),
        Repin {
            scroll_up: 0,
            top: 2,
            clear_below: 0
        }
    );
}

#[test]
fn repin_modal_fills_the_screen_without_scrolling() {
    // A prompt as tall as the terminal seats at row 0 — still no scroll, so
    // every row it covers is repaintable from history when it closes.
    assert_eq!(
        repin_modal(20, 4, 24, 24),
        Repin {
            scroll_up: 0,
            top: 0,
            clear_below: 0
        }
    );
}

#[test]
fn repin_modal_shrinks_like_any_other_region() {
    // Tab swapping the option rows for the amend field shortens the prompt:
    // the top stays put and the vacated rows below are blanked, exactly as
    // `repin` does. The modal only ever moves its top *up*, so nothing above
    // it is ever left stale.
    assert_eq!(
        repin_modal(12, 12, 10, 24),
        Repin {
            scroll_up: 0,
            top: 12,
            clear_below: 2
        }
    );
}

#[test]
fn a_modal_that_fits_below_the_conversation_keeps_its_own_height() {
    // Early session: the prompt fits between the committed rows above the
    // region (view_top of them) and the screen bottom — it covers nothing, so
    // there is nothing to replay and the region is exactly the prompt.
    assert_eq!(modal_region_height(20, 5, 40), 20);
    // Exactly flush against the bottom still fits without covering.
    assert_eq!(modal_region_height(35, 5, 40), 35);
}

#[test]
fn a_modal_that_would_cover_conversation_takes_the_whole_screen() {
    // The moment the prompt needs even one conversation row, the region spans
    // the terminal and the render replays the tail above the prompt — so the
    // newest messages stay visible instead of vanishing under the modal
    // (docs/permissions.md).
    assert_eq!(modal_region_height(36, 5, 40), 40);
    assert_eq!(modal_region_height(20, 30, 40), 40);
}

#[test]
fn a_modal_taller_than_the_screen_clamps_to_it() {
    assert_eq!(modal_region_height(60, 0, 40), 40);
    assert_eq!(modal_region_height(60, 30, 40), 40);
}

#[test]
fn restore_cursor_row_follows_the_box_down_the_screen() {
    // Box one row above the bottom → prompt on the last row, still no gap.
    assert_eq!(restore_cursor_row(20, 3, 24), Some(23));
}

#[test]
fn restore_cursor_row_is_none_when_the_box_occupies_the_last_row() {
    // No room below (top 21 + height 3 = 24): caller scrolls up one instead.
    assert_eq!(restore_cursor_row(21, 3, 24), None);
}

#[test]
fn cursor_row_sits_on_the_rendered_prompt_row() {
    // render_live and cursor_position both derive their geometry from
    // input_box, so the hardware cursor lands on exactly the row where the
    // prompt is drawn — they cannot drift apart.
    let area = Rect::new(0, 0, 40, LIVE_MIN_HEIGHT);
    let mut app = App::new();
    app.input = TextArea::from_text("x");
    let mut buf = Buffer::empty(area);
    render_live(area, &mut buf, &app);
    let (_, cy) = cursor_position(area, &app);
    let rendered: String = (0..area.width).map(|x| buf[(x, cy)].symbol()).collect();
    assert!(
        rendered.contains('❯'),
        "cursor row carries the prompt glyph"
    );
}

#[test]
fn cursor_row_sits_on_the_prompt_row_while_a_turn_streams() {
    // Codex keeps the composer focused while a task runs — typing mid-turn
    // edits the draft (Enter queues it), so the cursor must land on the
    // box's prompt row even with the streaming strip *and* a queued
    // message stacked above it, not on a strip row.
    let mut app = App::new();
    app.begin_stream();
    app.queued.push_back(batch(&["world"]));
    app.input = TextArea::from_text("x");
    let q = queued_rows(&app, 40);
    let h = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
    let area = Rect::new(0, 0, 40, h);
    let mut buf = buffer(40, h);
    render_live(area, &mut buf, &app);
    let (cx, cy) = cursor_position(area, &app);
    let rendered = row(&buf, cy, 40);
    assert!(
        rendered.starts_with("❯ x"),
        "cursor row carries the box prompt, not the strip: {rendered:?}"
    );
    assert_eq!(cx, 3, "right after the typed text");
}

#[test]
fn live_height_adds_the_command_menu_band() {
    let closed = live_height(&TextArea::from_text("hi"), 40, 24, false, 0, 0, 0, 0, 0, 0);
    let open = live_height(
        &TextArea::from_text("/"),
        40,
        24,
        false,
        0,
        0,
        0,
        MENU_MAX_ROWS,
        0,
        0,
    );
    assert_eq!(open, closed + MENU_MAX_ROWS, "the menu band adds its rows");
}

#[test]
fn live_height_grows_with_the_queue() {
    let mut app = App::new();
    app.begin_stream();
    let without = live_height(&app.input, 40, 24, true, 1, 0, 0, 0, 0, 0);
    app.queued.push_back(batch(&["world"]));
    let q = queued_rows(&app, 40);
    let with = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
    assert_eq!(with, without + q, "the queue grows the region by its rows");
    assert_eq!(q, 1, "one short queued message is one row");
}

#[test]
fn live_height_adds_the_footer_row() {
    let ta = TextArea::from_text("hi");
    assert_eq!(
        live_height(&ta, 40, 24, false, 0, 0, 0, 0, 1, 0),
        live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0) + 1,
        "the footer adds its row at the very bottom"
    );
}

#[test]
fn live_height_reserves_exactly_one_row_for_the_toast() {
    let mut app = App::new();
    let without = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 0);
    app.show_toast("hi", ToastKind::Info);
    let with = live_height(&app.input, 60, 24, false, 0, 0, toast_rows(&app), 0, 0, 0);
    assert_eq!(with, without + 1, "the toast adds exactly one row");
}

#[test]
fn render_pipeline_survives_extreme_terminal_sizes() {
    // codex clamps every wrap width (`.max(1)` and friends) so narrow or
    // short terminals degrade gracefully instead of panicking; this locks
    // the same property over our whole pure pipeline — the live region,
    // the cursor, the scrollback commits, the resize repaint tail, and the
    // Ctrl+O overlay — at every awkward size, wide CJK/emoji included.
    // (The terminal half of a real resize is smoke-covered: Phase 17.)
    let widths = [0u16, 1, 2, 3, 4, 5, 8, 13, 34, 80, 120];
    let heights = [1u16, 2, 3, 4, 5, 8, 12, 24, 48];
    for (state, app) in &size_sweep_apps() {
        for &w in &widths {
            for &h in &heights {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // mirror main.rs::draw
                    let band = band_rows(app);
                    let lh = live_height(
                        &app.input,
                        w,
                        h,
                        strip_has_status(app),
                        preview_rows(app, w),
                        queued_rows(app, w),
                        0,
                        band,
                        footer_rows(app, band),
                        0,
                    );
                    let area = Rect::new(0, 0, w, lh.clamp(1, h.max(1)));
                    let mut buf = Buffer::empty(area);
                    render_live(area, &mut buf, app);
                    let _ = cursor_position(area, app);
                    // mirror main.rs::repaint_conversation
                    let budget = repaint_budget(h, area.height);
                    let _ = repaint_lines(&app.history, w, budget);
                    // mirror the streaming commit path
                    if let Some(text) = app.streaming_text() {
                        let mut render = StreamRender::new();
                        let _ = render.commit(text, w);
                        let _ = render.finish(text, w);
                    }
                    // mirror the Ctrl+O overlay
                    let screen = Rect::new(0, 0, w, h);
                    let mut overlay = Buffer::empty(screen);
                    render_tool_view(screen, &mut overlay, app, &transcript_lines(app, w));
                    let _ = tool_view_max_scroll(app, w, h);
                }));
                assert!(
                    run.is_ok(),
                    "render pipeline panicked: state={state} width={w} height={h}"
                );
            }
        }
    }
}

#[test]
fn the_search_cursor_clamps_inside_a_narrow_terminal() {
    let app = searching(&["git status"], "a very very long query indeed");
    let h = live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 1, 0);
    let area = Rect::new(0, 0, 20, h);
    let (x, _) = cursor_position(area, &app);
    assert!(x < 20, "clamped inside the width (codex clamps the same)");
}

#[test]
fn all_failed_picker_height_covers_each_error_row() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(2);
    app.add_model_error("OpenRouter", "HTTP 500");
    app.add_model_error("Agent Zero API", "HTTP 401");
    // Collapsed chrome (6) + 2 error rows = 8.
    assert_eq!(model_picker_height(&app, 40), Some(8));
}

#[test]
fn model_picker_height_covers_the_chrome_plus_list() {
    let mut app = App::new();
    app.open_model_picker("a");
    app.set_models(three_models());
    // 9 chrome rows + 3 list rows.
    assert_eq!(model_picker_height(&app, 40), Some(12));
    // Clamped to the terminal height.
    assert_eq!(model_picker_height(&app, 8), Some(8));
    // A placeholder state (still loading — no models) drops the counter +
    // name detail rows: 6 collapsed chrome + 1 placeholder row = 7.
    app.open_model_picker("a");
    assert_eq!(model_picker_height(&app, 40), Some(7));
    // None when the picker is closed.
    app.close_model_picker();
    assert_eq!(model_picker_height(&app, 40), None);
}

#[test]
fn key_onboarding_height_covers_both_steps() {
    let mut app = login_app_provider();
    // Provider step: 9 chrome + 2 provider rows.
    assert_eq!(key_onboarding_height(&app, 40), Some(11));
    // Clamped to the terminal height.
    assert_eq!(key_onboarding_height(&app, 6), Some(6));
    // Key step: a fixed height.
    app.key_onboarding.as_mut().unwrap().step = KeyStep::Key;
    assert_eq!(key_onboarding_height(&app, 40), Some(LOGIN_KEY_ROWS));
    // None when closed.
    app.close_key_onboarding();
    assert_eq!(key_onboarding_height(&app, 40), None);
}

#[test]
fn live_height_saturates_instead_of_overflowing_u16() {
    // A recalled multi-megabyte paste (tens of thousands of wrapped rows)
    // plus the same text queued mid-turn used to overflow the u16 row sum
    // and panic in dev builds (overflow checks on).
    let input = TextArea::from_text(&"x".repeat(170_000));
    let h = live_height(&input, 10, 24, true, 0, 60_000, 0, 0, 1, 0);
    assert_eq!(h, 24, "clamped to the terminal height");
}

#[test]
fn the_manager_list_shows_title_count_rows_and_hints() {
    let mut app = App::new();
    for (i, cmd) in [
        "ping -c 100 x.com",
        "ping -c 100 facebook.com",
        "ping -c 100 google.com",
    ]
    .iter()
    .enumerate()
    {
        app.bg_started(&format!("bash_{}", i + 1), cmd, None, true);
    }
    app.open_background_view();
    let texts: Vec<String> = background_view_lines(&app, 74)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[0], "─".repeat(74), "top rule");
    assert_eq!(texts[1], "");
    assert_eq!(texts[2], "  Background");
    assert_eq!(texts[3], "  3 active shells");
    assert_eq!(texts[4], "");
    assert_eq!(texts[5], "  ❯ ping -c 100 x.com (running)");
    assert_eq!(texts[6], "    ping -c 100 facebook.com (running)");
    assert_eq!(texts[7], "    ping -c 100 google.com (running)");
    assert_eq!(texts[8], "");
    assert_eq!(
        texts[9],
        "  ↑/↓ to select · Enter to view · x to stop · Esc to close"
    );
    assert_eq!(texts[10], "");
    assert_eq!(texts[11], "─".repeat(74), "bottom rule");
    assert_eq!(texts.len(), 12);
    // The height helper reserves exactly the painted rows.
    assert_eq!(background_view_height(&app, 40), Some(12));
}

#[test]
fn the_manager_details_page_shows_fields_and_the_output_box() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping -c 120 x.com", None, true);
    for i in 1..=12 {
        app.bg_output("bash_1", &format!("64 bytes from x.com seq={i}\n"));
    }
    app.set_background_runtime("bash_1", Duration::from_secs(3));
    app.background_view = Some(BackgroundView::Details {
        id: "bash_1".to_string(),
    });
    let texts: Vec<String> = background_view_lines(&app, 74)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts[2], "  Shell details");
    assert_eq!(texts[4], "  Status:   running");
    assert_eq!(texts[5], "  Runtime:  3s");
    assert_eq!(texts[6], "  Command:  ping -c 120 x.com");
    assert_eq!(texts[8], "  Output:");
    assert!(texts[9].starts_with("  ╭") && texts[9].ends_with('╮'));
    // The box tails the LAST rows: seq=3..=12 fill its 10 interior rows.
    assert!(
        texts[10].contains("seq=3"),
        "tails the newest lines: {texts:?}"
    );
    assert!(texts[19].contains("seq=12"));
    assert!(texts[20].starts_with("  ╰") && texts[20].ends_with('╯'));
    assert_eq!(texts[21], "  Showing 10 lines");
    assert_eq!(texts[22], "");
    assert_eq!(
        texts[23],
        "  ← to go back · Esc/Enter/Space to close · x to stop"
    );
    assert_eq!(background_view_height(&app, 40), Some(texts.len() as u16));
}

// --- terminal-size sweep ---

/// Mixed-width stress text: prose, wide CJK, emoji, and an unbreakable
/// over-long token, so the sweep hits every wrap branch.
const SWEEP_TEXT: &str = "The quick brown fox 世界你好 mixes wide CJK with \
    emoji 🎉🎊 and averyveryverylongunbreakabletokenthatmusthardbreak too.";

/// One busy `App` per live-region feature, so the sweep exercises every
/// width-dependent render path on top of a shared finished history
/// (messages, a tool call, a summary, the session footer).
fn size_sweep_apps() -> Vec<(&'static str, App)> {
    let base = || {
        let mut app = App::new();
        app.set_session_info("dummy_model_name", "~/repo/some/longish/path");
        app.record_user_message("first message with CJK 世界 and emoji 🎉");
        app.record_system_message("help text\nwith a second line");
        app.begin_stream();
        app.push_chunk("text before the tool call. ");
        app.start_tool("read_file", "src/app.rs with a long argument string");
        app.end_tool(SWEEP_TEXT, true);
        app.push_chunk(SWEEP_TEXT);
        app.finish_stream();
        app.end_turn(3);
        app
    };
    let streaming = || {
        let mut app = base();
        app.begin_stream();
        app.push_chunk(SWEEP_TEXT);
        app.set_status_times(Duration::from_secs(7), None);
        app
    };
    let tool = {
        let mut app = base();
        app.begin_stream();
        app.push_chunk("before tool ");
        app.start_tool("write_file", SWEEP_TEXT);
        app.set_status_times(Duration::from_secs(7), Some(Duration::from_secs(2)));
        app
    };
    let queued = {
        let mut app = streaming();
        app.queued.push_back(batch(&["queued one with CJK 世界"]));
        app
    };
    let menu = {
        let mut app = base();
        app.input = TextArea::from_text("/");
        app.command_menu = Some(crate::app::CommandMenu { selected: 0 });
        app
    };
    let shortcuts = {
        let mut app = base();
        app.shortcuts_open = true;
        app
    };
    let draft = {
        let mut app = base();
        app.input = TextArea::from_text(&format!("{SWEEP_TEXT}\n{SWEEP_TEXT}"));
        app
    };
    let file_picker = {
        // An open `@` picker with match indices deep enough in a long
        // mixed-width path that tiny widths truncate past them, so
        // `file_menu_row`'s truncate + match-span grouping is swept too.
        let long = "src/some/deeply/nested/世界 with spaces/🎉emoji/averylongfilename.rs";
        let mut app = base();
        app.input = TextArea::from_text("@src");
        app.file_search = Some(FileSearch {
            selected: 0,
            query: "src".into(),
            matches: vec![
                FileMatch {
                    path: "src/app.rs".into(),
                    score: 10,
                    indices: vec![0, 1, 2],
                },
                FileMatch {
                    path: long.into(),
                    // Byte offsets of `s`, `r`, `c`, `世`, `界`, `🎉`, `a`,
                    // and `l` — the last five land past a narrow truncation.
                    score: 5,
                    indices: vec![0, 1, 2, 23, 26, 42, 52, 57],
                },
            ],
            waiting: false,
        });
        app
    };
    vec![
        ("idle", base()),
        ("streaming", streaming()),
        ("tool", tool),
        ("queued", queued),
        ("menu", menu),
        ("shortcuts", shortcuts),
        ("draft", draft),
        ("file_picker", file_picker),
    ]
}

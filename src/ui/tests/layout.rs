//! Live-region geometry: height, re-pin, and the cursor seat.

use super::*;

/// The `/login` key step's whole page: top rule, gap, prompt, gap, input,
/// gap, hint, gap, bottom rule — pinned here so a builder change that adds
/// or drops a row fails a height test (`key_onboarding_lines`).
const LOGIN_KEY_ROWS: u16 = 9;
use crate::ui::theme::{
    GAP_ROWS, MENU_MAX_ROWS, MODEL_SEARCH_ROW, STATUS_GAP_ROWS, STATUS_ROWS,
    STREAM_PREVIEW_MIN_ROWS, STREAM_PREVIEW_RESERVED_ROWS,
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
        live_height(&TextArea::from_text(""), 40, 24, false, 0, 0, 0, 0, 0, 0, 0),
        LIVE_MIN_HEIGHT
    );
    assert_eq!(
        live_height(
            &TextArea::from_text("hi"),
            40,
            24,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0
        ),
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
            live_height(&ta, 40, 24, true, 1, 0, 0, 0, 0, 0, 0),
            live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0, 0) + 4,
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
        live_height(&many, 40, 10, false, 0, 0, 0, 0, 0, 0, 0),
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
fn modal_rebuild_fires_when_a_pinned_prompt_would_seat_short_of_the_bottom() {
    // Back-to-back prompts of different heights: the tall body-capped prompt
    // (painted flush at the bottom, rows 4..40) was answered, its resolved
    // cell queued (14 pending rows), and a short prompt opened in its place —
    // the flush would seat the region at 4+14, ending at 4+14+16 = 34 of 40.
    // The scrolls that pinned it to the bottom are one-way, so that frame
    // strands the open prompt above a band of blank rows until it is
    // answered (the reported bug). The draw must purge-rebuild NOW, not wait
    // for the close.
    assert!(modal_needs_rebuild(true, false, 40, 4, 14, 16, 40));
    // …whether or not a one-way move was already noted, and equally with
    // nothing pending (the separate-frame ordering: the cell committed and
    // flushed before the next prompt opened — a pure repin shrink).
    assert!(modal_needs_rebuild(true, true, 40, 4, 14, 16, 40));
    assert!(modal_needs_rebuild(true, false, 40, 19, 0, 16, 40));
}

#[test]
fn modal_rebuild_waits_while_the_open_prompt_stays_seated_at_the_bottom() {
    // Same frame → the ordinary diff paint; taller → the ordinary repin,
    // whose scroll keeps the region flush at the bottom by itself.
    assert!(!modal_needs_rebuild(true, true, 40, 4, 0, 36, 40));
    assert!(!modal_needs_rebuild(true, true, 40, 4, 0, 39, 40));
    // A flush whose own scroll plan re-seats the region flush (4+14+25 ≥ 40)
    // needs no rebuild either — the paint lands it at the bottom.
    assert!(!modal_needs_rebuild(true, true, 40, 4, 14, 25, 40));
}

#[test]
fn modal_rebuild_skips_a_prompt_floating_above_the_bottom() {
    // A floating region (a short conversation — the painted frame never
    // reached the screen bottom) shrinks over rows that are already blank:
    // no visible gap, and skipping the rebuild keeps the user's own terminal
    // scrollback unpurged.
    assert!(!modal_needs_rebuild(true, false, 22, 2, 0, 12, 40));
    assert!(!modal_needs_rebuild(true, true, 22, 2, 0, 12, 40));
}

#[test]
fn modal_rebuild_on_close_still_follows_the_one_way_note() {
    // No prompt open: the note alone decides, exactly as before — a close
    // after a one-way move purges, an unmoved close shrinks in place.
    assert!(modal_needs_rebuild(false, true, 40, 4, 0, 8, 40));
    assert!(!modal_needs_rebuild(false, false, 22, 2, 0, 8, 40));
    // The geometry is irrelevant at the close: even a plan that reaches the
    // bottom rebuilds after the note (it stands for what already moved).
    assert!(modal_needs_rebuild(false, true, 40, 30, 0, 10, 40));
}

#[test]
fn region_is_modal_only_while_a_permission_prompt_is_open() {
    // The one inline view whose close needs the purge rebuild — its growth
    // scrolls chat into real scrollback one-way (docs/permissions.md); every
    // other band/picker shrinks back over rows it never scrolled.
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
    let h = live_height(&app.input, 40, 24, true, 1, 0, q, 0, 0, 0, 0);
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
    let closed = live_height(
        &TextArea::from_text("hi"),
        40,
        24,
        false,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    );
    let open = live_height(
        &TextArea::from_text("/"),
        40,
        24,
        false,
        0,
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
    let without = live_height(&app.input, 40, 24, true, 1, 0, 0, 0, 0, 0, 0);
    app.queued.push_back(batch(&["world"]));
    let q = queued_rows(&app, 40);
    let with = live_height(&app.input, 40, 24, true, 1, 0, q, 0, 0, 0, 0);
    assert_eq!(with, without + q, "the queue grows the region by its rows");
    assert_eq!(q, 1, "one short queued message is one row");
}

#[test]
fn live_height_adds_the_footer_row() {
    let ta = TextArea::from_text("hi");
    assert_eq!(
        live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 1, 0),
        live_height(&ta, 40, 24, false, 0, 0, 0, 0, 0, 0, 0) + 1,
        "the footer adds its row at the very bottom"
    );
}

#[test]
fn live_height_reserves_exactly_one_row_for_the_toast() {
    let mut app = App::new();
    let without = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 0, 0, 0);
    app.show_toast("hi", ToastKind::Info);
    let with = live_height(
        &app.input,
        60,
        24,
        false,
        0,
        0,
        0,
        toast_rows(&app),
        0,
        0,
        0,
    );
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
                    let band = band_rows(app, w);
                    let lh = live_height(
                        &app.input,
                        w,
                        h,
                        strip_has_status(app),
                        preview_rows(app, w),
                        0,
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
                    let _ = repaint_lines(&app.history, w, usize::from(h));
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
    let h = live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 0, 1, 0);
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
    assert_eq!(model_picker_height(&app, 74, 40), Some(8));
}

#[test]
fn model_picker_height_covers_the_chrome_plus_list() {
    let mut app = App::new();
    app.open_model_picker("a");
    app.set_models(three_models());
    // 9 chrome rows + 3 list rows.
    assert_eq!(model_picker_height(&app, 74, 40), Some(12));
    // Clamped to the terminal height.
    assert_eq!(model_picker_height(&app, 74, 8), Some(8));
    // A placeholder state (still loading — no models) drops the counter +
    // name detail rows: 6 collapsed chrome + 1 placeholder row = 7.
    app.open_model_picker("a");
    assert_eq!(model_picker_height(&app, 74, 40), Some(7));
    // None when the picker is closed.
    app.close_model_picker();
    assert_eq!(model_picker_height(&app, 74, 40), None);
}

#[test]
fn key_onboarding_height_covers_both_steps() {
    let mut app = login_app_provider();
    // Provider step: 9 chrome + 2 provider rows.
    assert_eq!(key_onboarding_height(&app, 74, 40), Some(11));
    // Clamped to the terminal height.
    assert_eq!(key_onboarding_height(&app, 74, 6), Some(6));
    // Key step: a fixed height.
    app.key_onboarding.as_mut().unwrap().step = KeyStep::Key;
    assert_eq!(key_onboarding_height(&app, 74, 40), Some(LOGIN_KEY_ROWS));
    // None when closed.
    app.close_key_onboarding();
    assert_eq!(key_onboarding_height(&app, 74, 40), None);
}

#[test]
fn the_model_picker_reserves_the_running_tool_strip_above_it() {
    // The user report: opening `/model` mid-turn hid the spinner status line
    // and the running tool's live cell — the picker took the *whole* region,
    // though it only ever replaces the composer. It now follows the ↓ manager
    // band's rule (docs/background.md): the strip keeps its rows above the
    // picker's own frame.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "for i in $(seq 1 100); do echo $i; sleep 1; done");
    app.open_model_picker("a");
    app.set_models(three_models());
    let preview = preview_rows(&app, 74);
    assert!(preview > 0, "the running tool previews mid-turn");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    // 9 chrome rows + 3 list rows, over the strip.
    assert_eq!(
        model_picker_height(&app, 74, 40),
        Some(strip + 12),
        "the picker keeps the streaming strip above its own frame"
    );
    // A queued follow-up and a toast ride the strip too — the same rows the
    // composer path reserves.
    app.queued.push_back(batch(&["and then this"]));
    app.show_toast("Switched model to kimi-k3", ToastKind::Info);
    let extra = queued_rows(&app, 74) + toast_rows(&app);
    assert!(extra > 0);
    assert_eq!(model_picker_height(&app, 74, 40), Some(strip + extra + 12));
    // Clamped to the terminal height like every region.
    assert_eq!(model_picker_height(&app, 74, 8), Some(8));
    // Idle again (turn over, tool resolved, toast/queue gone), the picker is
    // alone — the old geometry, no stray strip rows.
    app.queued.clear();
    app.clear_toast();
    app.end_tool("done", true);
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(model_picker_height(&app, 74, 40), Some(12));
}

#[test]
fn the_login_flow_reserves_the_strip_above_it() {
    // `/login` opens mid-turn for the same reason `/model` does, so it keeps
    // the same strip above itself — here a streaming reply's preview row.
    let mut app = login_app_provider();
    app.begin_stream();
    app.push_chunk("let me look that up");
    let preview = preview_rows(&app, 74);
    assert_eq!(preview, 1, "a streaming reply previews its last row");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    // Provider step: 9 chrome + 2 provider rows, over the strip.
    assert_eq!(key_onboarding_height(&app, 74, 40), Some(strip + 11));
    // The key step is a fixed height — over the same strip.
    app.key_onboarding.as_mut().unwrap().step = KeyStep::Key;
    assert_eq!(
        key_onboarding_height(&app, 74, 40),
        Some(strip + LOGIN_KEY_ROWS)
    );
    // Clamped to the terminal height like every region.
    assert_eq!(key_onboarding_height(&app, 74, 6), Some(6));
    // Idle, the flow is alone.
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(key_onboarding_height(&app, 74, 40), Some(LOGIN_KEY_ROWS));
}

#[test]
fn a_squeezed_region_keeps_the_picker_whole_and_drops_strip_rows() {
    // The band's rule for a terminal too short to fit both: the view the user
    // is typing into keeps its full height and the strip above it is squeezed
    // (the `Length`/`Min(0)` split in `view_split`) — never the other way
    // round, which would cut the picker's search line off the bottom.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 100");
    app.open_model_picker("a");
    app.set_models(three_models());
    let picker = 12; // 9 chrome + 3 list rows
    let term = 16; // shorter than the strip (4) + picker (12) would want…
    app.queued.push_back(batch(&["a queued follow-up"]));
    assert!(
        model_picker_height(&app, 60, 40).unwrap() > term,
        "the unclamped region really is taller than this terminal"
    );
    let h = model_picker_height(&app, 60, term).unwrap();
    assert_eq!(h, term, "clamped to the terminal");
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 60)).collect();
    assert!(
        rows[usize::from(h - picker)].starts_with('─'),
        "the picker's top rule sits {picker} rows up from the bottom: {rows:?}"
    );
    assert!(
        rows[usize::from(h - picker) + usize::from(MODEL_SEARCH_ROW)].contains('❯'),
        "…so its search line is on screen: {rows:?}"
    );
    assert!(
        rows.last().unwrap().starts_with('─'),
        "…and its bottom rule is the region's last row: {rows:?}"
    );
}

#[test]
fn live_height_saturates_instead_of_overflowing_u16() {
    // A recalled multi-megabyte paste (tens of thousands of wrapped rows)
    // plus the same text queued mid-turn used to overflow the u16 row sum
    // and panic in dev builds (overflow checks on).
    let input = TextArea::from_text(&"x".repeat(170_000));
    let h = live_height(&input, 10, 24, true, 0, 0, 60_000, 0, 0, 1, 0);
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
        app.bg_started(&format!("bash_{}", i + 1), cmd, None, true, None);
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
    assert_eq!(background_view_height(&app, 74, 40), Some(12));
}

#[test]
fn the_manager_details_page_shows_fields_and_the_output_box() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping -c 120 x.com", None, true, None);
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
    assert_eq!(
        background_view_height(&app, 74, 40),
        Some(texts.len() as u16)
    );
}

#[test]
fn the_details_page_humanizes_the_runtime_and_wraps_the_command() {
    // The user-report fixes (docs/background.md): a long-lived shell's
    // Runtime reads `2m 3s` (never a bare `123s`), and a long command WRAPS
    // under the value column instead of truncating away — the height helper
    // counting the wrapped rows at the real width.
    let mut app = App::new();
    let long_cmd = "for i in $(seq 1 100); do echo tick $i; sleep 1; done \
                    && echo all done at the end";
    app.bg_started("b1", long_cmd, None, true, None);
    app.set_background_runtime("b1", Duration::from_secs(123));
    app.background_view = Some(BackgroundView::Details {
        id: "b1".to_string(),
    });
    let texts: Vec<String> = background_view_lines(&app, 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t == "  Runtime:  2m 3s"), "{texts:?}");
    let cmd_row = texts
        .iter()
        .position(|t| t.starts_with("  Command:  for i in"))
        .expect("the command field leads its first row");
    assert!(
        texts[cmd_row + 1].starts_with("            "),
        "continuations align under the value column: {:?}",
        texts[cmd_row + 1]
    );
    let joined: String = texts.join(" ");
    assert!(
        joined.contains("all done at the end"),
        "the tail of the command survives — nothing truncated: {texts:?}"
    );
    assert_eq!(
        background_view_height(&app, 60, 40),
        Some(texts.len() as u16),
        "the reserved height counts the wrapped rows"
    );
}

#[test]
fn the_details_page_names_a_subagent_launcher() {
    // A shell a subagent launched shows where it came from — the `From:`
    // field (docs/agent-tool.md).
    let mut app = App::new();
    app.bg_started(
        "b1",
        "sleep 60",
        None,
        true,
        Some(crate::background::BgOrigin {
            agent_id: "a1".into(),
            agent_type: "general-purpose".into(),
        }),
    );
    app.background_view = Some(BackgroundView::Details {
        id: "b1".to_string(),
    });
    let texts: Vec<String> = background_view_lines(&app, 74)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t == "  From:     general-purpose agent"),
        "{texts:?}"
    );
}

#[test]
fn the_manager_band_reserves_the_running_tool_strip_above_it() {
    // The user report (docs/background.md): opening the ↓ manager while a
    // foreground tool ran swallowed its live cell — the band replaced the
    // whole region, streaming strip included. The reserved height now keeps
    // the strip's rows (the running cell + gap + status + gap) above the
    // band's own lines, exactly what the composer path reserves.
    let mut app = App::new();
    app.bg_started("bash_1", "sleep 100", None, true, None);
    app.begin_stream();
    app.start_tool("Bash", "for i in $(seq 1 100); do echo $i; sleep 1; done");
    app.open_background_view();
    let band = background_view_lines(&app, 74).len() as u16;
    let preview = preview_rows(&app, 74);
    assert!(preview > 0, "the running tool previews mid-turn");
    let strip = preview + GAP_ROWS + STATUS_ROWS + STATUS_GAP_ROWS;
    assert_eq!(
        background_view_height(&app, 74, 40),
        Some(strip + band),
        "the band keeps the streaming strip above its own lines"
    );
    // Clamped to the terminal height like every region.
    assert_eq!(background_view_height(&app, 74, 8), Some(8));
    // Idle again (turn over, tool resolved), the band is alone — the old
    // geometry, no stray strip rows.
    app.end_tool("done", true);
    app.finish_stream();
    app.end_turn(1);
    assert_eq!(background_view_height(&app, 74, 40), Some(band));
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

#[test]
fn the_menu_cursor_seat_follows_a_bottom_anchored_body() {
    // A `/trust` review taller than the region bottom-anchors
    // (docs/view-flow.md): the options — and the `❯` the hidden cursor seats
    // on — sit near the page's end, inside the painted tail. The seat must
    // subtract the same skipped rows the paint does, instead of falling back
    // to the far corner because the marker's *page* row is past the region.
    let mut app = App::new();
    app.open_trust_menu(crate::trust::TrustReview {
        root: "~/repo".into(),
        trusted: false,
        files: vec![crate::trust::TrustFileReview {
            label: "Hooks".into(),
            path: "~/repo/.alter-zero/hooks.json".into(),
            items: (0..40)
                .map(|i| format!("PreToolUse (bash): ./guard-{i}.sh"))
                .collect(),
            error: None,
            pending: true,
        }],
    });
    let (width, height) = (60u16, 20u16);
    let lines = crate::ui::trust_view_lines(&app, width);
    let skip = lines.len() - usize::from(height);
    assert!(skip > 0, "the review overflows the region");
    let marker_at = lines
        .iter()
        .position(|l| {
            l.spans
                .iter()
                .any(|s| s.content.as_ref() == crate::ui::theme::HOOKS_MARKER)
        })
        .expect("an option row carries the ❯");
    assert!(marker_at >= skip, "the marker sits inside the painted tail");
    let area = Rect::new(0, 0, width, height);
    let (x, y) = cursor_position(area, &app);
    assert_eq!(
        (x, y),
        (2, (marker_at - skip) as u16),
        "the seat lands on the painted ❯ row"
    );
}

#[test]
fn the_picker_cursor_seat_subtracts_the_bottom_anchor_skip() {
    // A /settings page one row taller than the region paints from its second
    // row (view_body_skip = 1), so the `❯` search line sits one row higher on
    // screen than its page row — and the hardware cursor must sit on the
    // painted line, not one below it (docs/view-flow.md).
    let mut app = App::new();
    app.open_settings();
    let width = 78u16;
    let page = crate::ui::settings_height(&app, width, 200).expect("open");
    let area = Rect::new(0, 0, width, page - 1);
    let (_, y) = cursor_position(area, &app);
    assert_eq!(
        y,
        crate::ui::theme::SETTINGS_SEARCH_ROW - 1,
        "the seat follows the anchored paint"
    );
}

// --- the overlay cursor seat (Ctrl+O / Ctrl+D, docs/tool-view-performance.md) ---

#[test]
fn overlay_cursor_seat_sits_just_after_the_last_glyph() {
    // The overlays never *show* the cursor, but a terminal with a cursor-move
    // animation still animates toward wherever it is seated — and a
    // full-screen cell paint used to leave it at the blank bottom-right
    // corner. The seat is the cell just past the frame's last non-blank
    // glyph, so the jump lands on text.
    let mut buf = buffer(20, 4);
    buf.set_string(0, 0, "title", Style::default());
    buf.set_string(0, 2, " q/esc to quit", Style::default());
    assert_eq!(overlay_cursor_seat(&buf), (14, 2));
}

#[test]
fn overlay_cursor_seat_clamps_inside_the_frame() {
    // A row that runs to the edge keeps the seat on its last column…
    let mut buf = buffer(6, 2);
    buf.set_string(0, 1, "abcdef", Style::default());
    assert_eq!(overlay_cursor_seat(&buf), (5, 1));
    // …and an empty frame parks it at the origin.
    assert_eq!(overlay_cursor_seat(&buffer(6, 2)), (0, 0));
}

#[test]
fn overlay_cursor_seat_steps_over_a_wide_glyph() {
    // A trailing emoji covers two columns; the seat lands after both, never
    // on the shadow cell.
    let mut buf = buffer(10, 1);
    buf.set_string(0, 0, "a🔥", Style::default());
    assert_eq!(overlay_cursor_seat(&buf), (3, 0));
}

#[test]
fn the_transcript_overlay_seats_the_cursor_after_its_quit_hint() {
    // The Ctrl+O pager's last text is the closing " q/esc/ctrl+o to quit"
    // hint — the seat sits right after "quit" (the user-visible fix: the
    // cursor animation jumps into that text, not to nowhere).
    let app = App::new();
    let area = Rect::new(0, 0, 60, 12);
    let mut buf = Buffer::empty(area);
    let lines = transcript_lines(&app, 60);
    render_tool_view(area, &mut buf, &app, &lines);
    let (x, y) = overlay_cursor_seat(&buf);
    let hint = " q/esc/ctrl+o to quit";
    assert!(
        row(&buf, y, 60).starts_with(hint),
        "the seat row is the closing hint: {:?}",
        row(&buf, y, 60)
    );
    assert_eq!(usize::from(x), hint.len(), "the seat sits right after it");
}

#[test]
fn the_context_overlay_seats_the_cursor_after_its_quit_hint() {
    // Ctrl+D is the pager's sibling: same chrome, its own closing hint.
    let app = App::new();
    let area = Rect::new(0, 0, 60, 12);
    let mut buf = Buffer::empty(area);
    let lines = context_lines(&app, 60);
    render_context_view(area, &mut buf, &app, &lines);
    let (x, y) = overlay_cursor_seat(&buf);
    let hint = " q/esc/ctrl+d to quit";
    assert!(
        row(&buf, y, 60).starts_with(hint),
        "the seat row is the closing hint: {:?}",
        row(&buf, y, 60)
    );
    assert_eq!(usize::from(x), hint.len(), "the seat sits right after it");
}

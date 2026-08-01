//! Painting the live region and its streaming strip.

use super::*;
use crate::ui::live::preview_tool_lines;
use crate::ui::theme::{
    FOOTER_FOCUS_BG, FOOTER_FOCUS_FG, INPUT_CHROME_ROWS, MODEL_SEARCH_ROW, STATUS_GAP_ROWS,
    STATUS_ROWS, TOOL_BACKGROUND_HINT, TOOL_BACKGROUND_HINT_DELAY, TOOL_PEEK_LINES,
    TOOL_PULSE_BRIGHT, TOOL_PULSE_DIM, TOOL_PULSE_PERIOD,
};
use crate::ui::wrap::cols;

#[test]
fn render_live_tails_a_running_bash_tool_with_its_streamed_output() {
    // End-to-end: streamed output accumulates on the running call and the
    // live strip tails it — the newest line and the `+N lines (Ns)` footer
    // both show (docs/tool-streaming.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ping -c 10 x");
    for i in 1..=9 {
        app.push_tool_output(&format!("line {i}\n"));
    }
    app.set_status_times(Duration::from_secs(9), None);
    let pv = preview_rows(&app, 60);
    let h = live_height(&app.input, 60, 24, true, pv, 0, 0, 0, 0, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 60))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("line 9"), "the newest line tails: {all:?}");
    assert!(!all.contains("line 4"), "older lines are hidden: {all:?}");
    assert!(all.contains("+5 lines (9s)"), "the footer shows: {all:?}");
}

#[test]
fn render_live_draws_the_status_row_while_streaming() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("hi");
    app.set_status_times(Duration::from_secs(3), None);
    let h = live_height(&app.input, 40, 24, true, 1, 0, 0, 0, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("Working…") && all.contains("3s"),
        "the live status line is drawn in the strip: {all:?}"
    );
}

#[test]
fn the_pre_stream_pause_shows_up_tokens_and_no_preview_bullet() {
    // After submit, before the first chunk arrives: the user's input is
    // counted into the tally (arrow ↑), the status spins, and the empty
    // reply buffer shows NO preview bullet (just the status line).
    let mut app = App::new();
    app.begin_stream();
    app.count_user_input("hello there"); // ↑ N tokens
    // No preview content yet → the strip is status + gap only (no preview
    // row, no preview gap): exactly the box + status + the two gaps.
    assert_eq!(preview_rows(&app, 60), 0);
    let h = live_height(&app.input, 60, 24, true, 0, 0, 0, 0, 0, 0);
    assert_eq!(
        h,
        STATUS_ROWS + STATUS_GAP_ROWS + INPUT_CHROME_ROWS + 1,
        "no preview row is reserved during the pause"
    );
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    // The status sits on the strip's top row (row 0) — no blank preview
    // line above it.
    assert!(
        row(&buf, 0, 60).contains('↑') && row(&buf, 0, 60).contains("tokens"),
        "the status with ↑ tokens is the strip's first row: {:?}",
        row(&buf, 0, 60)
    );
    // No row is an assistant preview line (`● ` at the start) — the only
    // `●` on screen is the comet spinner's head inside `(●•·   )`.
    for y in 0..h {
        assert!(
            !row(&buf, y, 60).starts_with("● "),
            "no reserved preview bullet row: {:?}",
            row(&buf, y, 60)
        );
    }
}

#[test]
fn render_live_grows_the_box_and_wraps_input_across_rows() {
    let mut app = App::new();
    app.input = TextArea::from_text("first\nsecond");
    let h = live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 0, 0);
    assert_eq!(h, 4, "two rules + two input rows (no strip when idle)");
    let mut buf = buffer(20, h);
    render_live(buf.area, &mut buf, &app);

    assert_eq!(buf[(0, 0)].symbol(), "─", "top rule");
    assert!(row(&buf, 1, 20).contains("❯ first"), "prompt on first line");
    assert!(
        row(&buf, 2, 20).contains("second") && !row(&buf, 2, 20).contains("❯"),
        "continuation line is indented, no prompt"
    );
    assert_eq!(
        buf[(0, 3)].symbol(),
        "─",
        "bottom rule moved down as box grew"
    );
}

#[test]
fn render_live_scrolls_input_to_keep_the_end_visible() {
    // Six input lines but a terminal that only fits four text rows: the box
    // shows the tail (so the cursor's line stays visible), not the head.
    let mut app = App::new();
    app.input = TextArea::from_text(
        &(0..6)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let term_h = 6; // live clamps to 6 → text rows = 6 - 2 = 4
    assert_eq!(
        live_height(&app.input, 20, term_h, false, 0, 0, 0, 0, 0, 0),
        6
    );
    let mut buf = buffer(20, 6);
    render_live(buf.area, &mut buf, &app);

    let text: String = (1..5).map(|y| row(&buf, y, 20)).collect();
    assert!(text.contains("line5"), "last line is visible: {text:?}");
    assert!(!text.contains("line0"), "first line scrolled off: {text:?}");
}

#[test]
fn render_live_strip_stacks_preview_gap_status_then_gap_above_the_box() {
    // While streaming the strip is four rows: the reply preview, a blank gap,
    // the live status line, then another blank gap — and only below them the
    // box's top rule, so the status never butts up against the box.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming reply");
    let mut buf = buffer(40, 7); // preview + gap + status + gap + (two rules + one input)
    render_live(buf.area, &mut buf, &app);

    assert!(
        row(&buf, 0, 40).contains("streaming reply"),
        "preview on row 0"
    );
    assert!(
        row(&buf, 1, 40).trim().is_empty(),
        "blank gap row below the preview"
    );
    assert!(
        row(&buf, 2, 40).contains("Working"),
        "status line on row 2: {:?}",
        row(&buf, 2, 40)
    );
    assert!(
        row(&buf, 3, 40).trim().is_empty(),
        "blank gap row below the status line"
    );
    assert_eq!(
        buf[(0, 4)].symbol(),
        "─",
        "top rule below the status gap, not touching the status"
    );
}

#[test]
fn render_live_has_no_strip_when_idle() {
    // Idle, the box sits directly under the chat — no preview/gap strip — so
    // the only separation is the committed blank after the last message.
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    let mut buf = buffer(40, 3); // just the box: two rules + one input row
    render_live(buf.area, &mut buf, &app);

    assert_eq!(
        buf[(0, 0)].symbol(),
        "─",
        "top rule on row 0 — no preview strip above it"
    );
    assert!(row(&buf, 1, 40).contains("❯ hello"), "input on row 1");
    assert_eq!(buf[(0, 2)].symbol(), "─", "bottom rule on row 2");
}

#[test]
fn render_live_shows_streaming_text_in_preview_row() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("Hi there");
    let mut buf = buffer(40, 4);
    render_live(buf.area, &mut buf, &app);

    let preview = row(&buf, 0, 40);
    assert!(preview.contains("●"), "preview shows assistant bullet");
    assert!(preview.contains("Hi there"), "preview shows streamed text");
}

#[test]
fn render_live_previews_a_running_tool_with_a_pulsing_bullet() {
    // While a tool runs, the strip's preview row shows its coloured header
    // instead of the assistant text, so the user sees what's executing — and
    // the bullet **breathes** at the injected frame phase rather than sitting
    // on a flat colour (`docs/tool-pulse.md`). This is the only place the
    // pulse reaches the screen, so it is the wiring this test pins.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Read", "src/main.rs");
    let mut buf = buffer(40, 5);
    render_live(buf.area, &mut buf, &app);

    let preview = row(&buf, 0, 40);
    assert!(
        preview.contains("Read(src/main.rs)"),
        "preview shows the running tool header: {preview:?}"
    );
    let dim = buf[(0, 0)].fg;
    assert_eq!(
        dim,
        Color::Rgb(TOOL_PULSE_DIM.0, TOOL_PULSE_DIM.1, TOOL_PULSE_DIM.2),
        "an un-injected clock renders the bottom of the breath"
    );
    // Half a period on, the same cell is at the bright end — the boundary's
    // per-frame `set_pulse` is what animates it.
    app.set_pulse(TOOL_PULSE_PERIOD / 2);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(
        buf[(0, 0)].fg,
        Color::Rgb(
            TOOL_PULSE_BRIGHT.0,
            TOOL_PULSE_BRIGHT.1,
            TOOL_PULSE_BRIGHT.2
        ),
        "the injected frame clock moves the bullet"
    );
    assert_ne!(dim, buf[(0, 0)].fg, "…so it visibly changes between frames");
}

#[test]
fn preview_shows_the_whole_parallel_batch_running_plus_waiting() {
    // A parallel batch previews every call: the running one + each `Waiting`
    // sibling, blank-separated. `preview_rows` counts them all, and
    // `render_live` paints the running cell alongside the `⎿ Waiting…`
    // siblings — so the batch is visible and clear. See
    // `docs/parallel-tools.md`.
    let mut app = App::new();
    app.begin_stream();
    let batch: Vec<crate::stream::ToolCallSummary> =
        ["ping google.com", "ping facebook.com", "ping x.com"]
            .iter()
            .map(|cmd| crate::stream::ToolCallSummary {
                name: "Bash".to_string(),
                args: (*cmd).to_string(),
            })
            .collect();
    app.start_tool_batch(&batch);
    app.start_tool("Bash", "ping google.com"); // the front call → Running
    // Past the hint delay so the running cell's Ctrl+B hint row shows.
    app.set_command_elapsed(Some(Duration::from_secs(3)));
    // Three 2-row cells (header + peek) with two blank separators, plus
    // the running cell's Ctrl+B hint row = 9 rows.
    assert_eq!(
        preview_rows(&app, 40),
        9,
        "the whole batch (3 cells + 2 gaps + the running cell's hint) is previewed"
    );
    let pv = preview_rows(&app, 40);
    let h = live_height(&app.input, 40, 30, true, pv, 0, 0, 0, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("ping google.com") && all.contains("Running…"),
        "the running call shows its header + Running…: {all:?}"
    );
    assert!(
        all.contains("ping facebook.com") && all.contains("ping x.com"),
        "the waiting siblings are shown: {all:?}"
    );
    assert_eq!(
        all.matches("Waiting…").count(),
        2,
        "exactly the two not-yet-run siblings show Waiting…: {all:?}"
    );
}

#[test]
fn render_live_previews_a_running_tool_with_its_running_row() {
    // While a backend tool runs the strip shows the whole cell — the header
    // *and* a `⎿ Running…` row beneath it — not just the header (req 2).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 1");
    let h = live_height(
        &app.input,
        40,
        24,
        true,
        preview_rows(&app, 40),
        0,
        0,
        0,
        0,
        0,
    );
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 0, 40).contains("Bash(sleep 1)"),
        "row 0 shows the header: {:?}",
        row(&buf, 0, 40)
    );
    let all: String = (0..h)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains('⎿') && all.contains("Running…"),
        "the strip shows the ⎿ Running… row: {all:?}"
    );
}

#[test]
fn render_live_draws_the_queue_above_the_box_as_a_user_message() {
    let mut app = App::new();
    app.begin_stream();
    app.queued.push_back(batch(&["world"]));
    let q = queued_rows(&app, 40);
    let h = live_height(&app.input, 40, 24, true, 1, q, 0, 0, 0, 0);
    let mut buf = buffer(40, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 40)).collect();
    // The queued "❯ world" sits *above* the box's (first) top rule, not below.
    let rule = rows
        .iter()
        .position(|r| r.contains('─'))
        .expect("a box rule");
    let world = rows
        .iter()
        .position(|r| r.contains("❯ world"))
        .expect("the queued message");
    assert!(
        world < rule,
        "the queued message is above the box: {rows:?}"
    );
    assert!(
        rows[world].starts_with("  ❯"),
        "the queued row is inset two columns: {:?}",
        rows[world]
    );
}

#[test]
fn render_live_paints_the_footer_on_the_last_row() {
    let app = with_session();
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let last = row(&buf, h - 1, 60);
    assert!(
        last.contains("dummy_model_name · ~/alter-zero"),
        "footer under the box: {last:?}"
    );
    assert!(last.starts_with("  dummy"), "two-column inset: {last:?}");
    assert_eq!(buf[(0, h - 2)].symbol(), "─", "bottom rule right above it");
}

#[test]
fn render_live_paints_the_toast_directly_above_the_box_when_idle() {
    let mut app = App::new();
    app.show_toast("Copied last message to clipboard", ToastKind::Info);
    let h = live_height(&app.input, 60, 24, false, 0, 0, toast_rows(&app), 0, 0, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    assert!(
        row(&buf, 0, 60).contains("Copied last message to clipboard"),
        "the toast is the region's first row: {:?}",
        row(&buf, 0, 60)
    );
    assert!(row(&buf, 0, 60).starts_with("  "), "two-column inset");
    assert_eq!(
        buf[(0, 1)].symbol(),
        "─",
        "the box's top rule sits directly below the toast"
    );
}

#[test]
fn render_live_paints_the_toast_below_the_status_while_streaming() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("hi");
    app.set_status_times(Duration::from_secs(1), None);
    app.show_toast(
        "/resume is disabled while a task is in progress",
        ToastKind::Info,
    );
    let h = live_height(&app.input, 60, 24, true, 1, 0, toast_rows(&app), 0, 0, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    // The toast sits on the strip's last row — directly above the box's top
    // rule, below the status line.
    let top_rule = (0..h)
        .find(|&y| row(&buf, y, 60).chars().all(|c| c == '─'))
        .expect("the box has a top rule");
    assert!(top_rule >= 1);
    assert!(
        row(&buf, top_rule - 1, 60).contains("/resume is disabled"),
        "the toast is the row just above the box: {:?}",
        row(&buf, top_rule - 1, 60)
    );
    let all: String = (0..h).map(|y| row(&buf, y, 60)).collect();
    assert!(all.contains("Working…"), "the status still shows above it");
}

#[test]
fn the_previewed_match_highlights_the_query_reversed() {
    let app = searching(&["git status"], "stat");
    assert_eq!(app.input.text(), "git status", "the match previews");
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    // The input row is "❯ git status" on the row inside the box frame:
    // "stat" starts at column 2 (prompt) + 4 ("git ").
    let y = 1;
    assert_eq!(row(&buf, y, 60).trim_end(), "❯ git status");
    for x in 6..10 {
        assert!(
            buf[(x, y)].modifier.contains(Modifier::REVERSED),
            "match cols reversed at x={x}"
        );
    }
    assert!(
        !buf[(2, y)].modifier.contains(Modifier::REVERSED),
        "outside the match stays plain"
    );
    assert!(
        !buf[(10, y)].modifier.contains(Modifier::REVERSED),
        "the highlight ends with the match"
    );
}

#[test]
fn a_long_shell_output_caps_the_preview_with_an_expand_hint() {
    // More than the cap → the first TOOL_PEEK_LINES lines, then a
    // `… +N lines (ctrl+o to expand)` row aligned with them.
    let output = (1..=6)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut t = tool("seq 6", "", ToolStatus::Ok, &output);
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(
        lines.len(),
        TOOL_PEEK_LINES + 1,
        "capped lines + the hint row"
    );
    assert_eq!(lines[0], "  ⎿  1");
    assert_eq!(lines[1], "     2", "continuation aligned, no corner");
    let hidden = 6 - TOOL_PEEK_LINES;
    assert_eq!(
        lines[TOOL_PEEK_LINES],
        format!("     … +{hidden} lines (ctrl+o to expand)")
    );
}

#[test]
fn the_running_shell_preview_is_the_flush_running_peek() {
    let mut app = App::new();
    app.begin_shell("sleep 5");
    app.set_status_times(Duration::from_secs(5), None);
    let q = queued_rows(&app, 60);
    // A shell turn hides the status line (has_status false), so the strip is
    // preview + gap only — sized exactly as main.rs::draw does.
    let h = live_height(
        &app.input,
        60,
        24,
        strip_has_status(&app),
        preview_rows(&app, 60),
        q,
        0,
        0,
        footer_rows(&app, 0),
        0,
    );
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    assert_eq!(
        row(&buf, 0, 60).trim_end(),
        "  ⎿  Running… (5s)",
        "the strip preview is the cell's running peek with its elapsed, flush \
         under the committed `! sleep 5` header just above the live region"
    );
}

#[test]
fn render_live_paints_the_hint_in_the_footer_slot() {
    let mut app = backtrack_app();
    app.set_session_info("model", "~/repo");
    app.backtrack.primed = true;
    let footer = footer_rows(&app, 0);
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, footer, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let last = row(&buf, h - 1, 60);
    assert!(
        last.contains("esc again to edit previous message"),
        "the hint takes the footer slot: {last:?}"
    );
    assert!(!last.contains("model"), "the session footer made way");
}

#[test]
fn render_live_shows_the_model_picker_when_open() {
    let mut app = App::new();
    app.open_model_picker("anthropic/claude-3-haiku");
    app.set_models(three_models());
    let mut buf = buffer(60, 14);
    render_live(buf.area, &mut buf, &app);
    // The picker stands in for the composer: top rule, then the `❯` search
    // line (headerless), then the model rows.
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, MODEL_SEARCH_ROW, 60).contains('❯'), "search line");
    assert!(
        row(&buf, 4, 60).contains("anthropic/claude-3-haiku"),
        "a model row"
    );
}

#[test]
fn cursor_position_stays_in_the_live_region_when_the_box_collapses() {
    // A tall queue on a short terminal leaves the input frame no rows at
    // all: Rect::inner returns Rect::ZERO (origin discarded) and the
    // hardware cursor used to teleport to the screen's top-left, sitting
    // over the scrollback.
    let mut app = App::new();
    for i in 0..20 {
        let text = format!("q{i}");
        app.queued.push_back(batch(&[text.as_str()]));
    }
    let area = Rect::new(0, 5, 30, 15);
    let (_, y) = cursor_position(area, &app);
    assert!(
        y >= area.y,
        "the cursor stays inside the live region (y={y}, region starts at {})",
        area.y
    );
}

#[test]
fn the_running_preview_hints_ctrl_b_but_the_committed_cell_does_not() {
    // The hint is live-only: the preview renderer appends it under the
    // running cell; the committed `tool_lines` never carry it. The hint
    // waits a few seconds (see below), so inject an elapsed past the delay.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ping google.com -c 50");
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    let preview: Vec<String> = preview_tool_lines(&app, 60).iter().map(plain).collect();
    assert!(
        preview
            .iter()
            .any(|l| l.trim() == "(ctrl+b to run in background)"),
        "the running preview hints Ctrl+B: {preview:?}"
    );
    let committed: Vec<String> = tool_lines(app.current_tool().unwrap(), 60)
        .iter()
        .map(plain)
        .collect();
    assert!(
        !committed.iter().any(|l| l.contains("ctrl+b")),
        "the commit-path cell never carries the hint: {committed:?}"
    );
}

#[test]
fn the_ctrl_b_hint_waits_a_few_seconds_before_showing() {
    // Like Claude Code: a command that finishes right away never shows the
    // Ctrl+B hint (it isn't needed) — the hint appears only once the
    // command has been running a few seconds. The boundary injects the
    // running command's own elapsed each frame (`set_command_elapsed`);
    // the preview gates the hint on it (docs/background.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "ping google.com -c 50");
    let shows_hint = |app: &App| {
        preview_tool_lines(app, 60)
            .iter()
            .map(plain)
            .any(|l| l.trim() == TOOL_BACKGROUND_HINT)
    };

    // Freshly started — no elapsed injected yet: no hint.
    assert!(!shows_hint(&app), "no hint the instant the command starts");
    // Under the delay: still no hint (a fast command stays clean).
    app.set_command_elapsed(Some(TOOL_BACKGROUND_HINT_DELAY - Duration::from_millis(1)));
    assert!(
        !shows_hint(&app),
        "no hint before the command has run a few seconds"
    );
    // At/past the delay: the hint appears.
    app.set_command_elapsed(Some(TOOL_BACKGROUND_HINT_DELAY));
    assert!(
        shows_hint(&app),
        "the hint shows once the command has run a few seconds"
    );
}

#[test]
fn a_running_shell_only_hints_ctrl_b_after_the_delay() {
    // The `!` shell run is the same: its own elapsed rides the status
    // (turn == command for a shell), so a quick `!` command never flashes
    // the hint, and a long one shows it after the delay.
    let mut app = App::new();
    app.begin_shell("sleep 30");
    let shows_hint = |app: &App| {
        preview_tool_lines(app, 60)
            .iter()
            .map(plain)
            .any(|l| l.trim() == TOOL_BACKGROUND_HINT)
    };
    app.set_command_elapsed(Some(Duration::from_secs(1)));
    assert!(!shows_hint(&app), "a fast `!` command shows no hint");
    app.set_command_elapsed(Some(Duration::from_secs(4)));
    assert!(
        shows_hint(&app),
        "a long `!` command hints Ctrl+B after the delay"
    );
}

#[test]
fn a_waiting_batch_sibling_gets_no_ctrl_b_hint() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool_batch(&[
        crate::stream::ToolCallSummary {
            name: "Bash".to_string(),
            args: "a".to_string(),
        },
        crate::stream::ToolCallSummary {
            name: "Bash".to_string(),
            args: "b".to_string(),
        },
    ]);
    app.start_tool("Bash", "a");
    // Past the hint delay so the running call shows its hint — the point
    // here is that the `⎿ Waiting…` sibling still gets none.
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    let preview: Vec<String> = preview_tool_lines(&app, 60).iter().map(plain).collect();
    let hints = preview.iter().filter(|l| l.contains("ctrl+b")).count();
    assert_eq!(hints, 1, "only the running call hints: {preview:?}");
}

#[test]
fn render_live_paints_the_focused_count_on_the_footer_row() {
    // The painted cells, not just the styled spans: the tint must cover
    // exactly the `1 shell` columns on the live region's last row — the
    // ` · ` separator before it stays untinted, so the highlight hugs the
    // indicator instead of bleeding across the footer.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new();
    app.set_session_info("kimi-k2", "~/repo");
    app.bg_started("bash_1", "ping x.com", None, true, None);
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let h = live_height(&app.input, 60, 24, false, 0, 0, 0, 0, 1, 0);
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let last = row(&buf, h - 1, 60);
    assert!(last.starts_with("  kimi-k2 · ~/repo · 1 shell"), "{last:?}");
    // Column, not byte offset — the ` · ` separators are multi-byte.
    let byte = last.find("1 shell").expect("the count");
    let start = u16::try_from(cols(&last[..byte])).unwrap();
    for x in start..start + u16::try_from(cols("1 shell")).unwrap() {
        assert_eq!(buf[(x, h - 1)].bg, FOOTER_FOCUS_BG, "tinted at column {x}");
        assert_eq!(buf[(x, h - 1)].fg, FOOTER_FOCUS_FG, "ink at column {x}");
    }
    assert_ne!(
        buf[(start - 1, h - 1)].bg,
        FOOTER_FOCUS_BG,
        "the separator before the count stays clean"
    );
}

#[test]
fn the_manager_band_replaces_the_composer_in_render_live() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping x.com", None, true, None);
    app.open_background_view();
    let h = background_view_height(&app, 60, 30).unwrap();
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h)
        .map(|y| row(&buf, y, 60))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("Background"), "the band renders: {all}");
    assert!(all.contains("1 active shell"));
    assert!(
        !all.contains('❯') || all.contains("❯ ping"),
        "no composer prompt — the band replaced it: {all}"
    );
}

#[test]
fn the_manager_band_keeps_the_running_tool_strip_above_it() {
    // The user report (docs/background.md): mid-turn, opening the ↓ manager
    // hid the running tool's live cell and the status line — the band
    // replaced the whole region. It replaces the composer only: the
    // streaming strip stays above it, the running cell tailing its output
    // and the spinner status still ticking.
    let mut app = App::new();
    app.bg_started("bash_1", "sleep 100", None, true, None);
    app.begin_stream();
    app.start_tool("Bash", "seq 1 100");
    app.push_tool_output("35\n36\n");
    app.open_background_view();
    let h = background_view_height(&app, 60, 40).unwrap();
    let mut buf = buffer(60, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 60)).collect();
    let all = rows.join("\n");
    assert!(
        all.contains("● Bash(seq 1 100)"),
        "the running cell stays visible: {all}"
    );
    assert!(all.contains("36"), "…tailing its streamed output: {all}");
    assert!(
        all.contains("esc to interrupt"),
        "the status line stays: {all}"
    );
    assert!(
        all.contains("1 active shell"),
        "the band renders too: {all}"
    );
    let cell = rows.iter().position(|r| r.contains("● Bash")).unwrap();
    let band = rows
        .iter()
        .position(|r| r.contains("1 active shell"))
        .unwrap();
    assert!(cell < band, "the strip sits above the band: {all}");
}

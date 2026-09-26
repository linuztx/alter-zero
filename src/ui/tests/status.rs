//! The live status line and the committed summary
//! (`docs/status-indicator.md`).

use super::*;
use crate::app::Spinner;
use crate::app::{STATUS_VERBS, VERB_ROTATION};
use crate::ui::message::compaction_full_lines;
use crate::ui::theme::{
    INDENT, SHIMMER_SWEEP, SPINNER_SPAN_COUNT, ai_color, header_gradient_end,
    header_gradient_start, shimmer_base, shimmer_highlight, spinner_bars_high,
    spinner_pulse_bright, spinner_tail_color, status_color, status_detail_color, status_done_color,
    status_retry_color, tool_diff_add_color, tool_diff_del_color, tool_dim_color, tool_pulse_dim,
};

#[test]
fn edit_cell_colours_the_summary_counts() {
    let output = "Updated a.rs (+6 -2)\n1 +x";
    let lines = tool_lines(
        &tool("Edit", "a.rs", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let summary = &lines[1];
    let plus = summary
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "+6")
        .unwrap();
    assert_eq!(plus.style.fg, Some(tool_diff_add_color()));
    let minus = summary
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "-2")
        .unwrap();
    assert_eq!(minus.style.fg, Some(tool_diff_del_color()));
}

#[test]
fn transcript_shows_no_stamp_on_assistant_tool_or_summary_items() {
    let mut app = App::new();
    app.history = vec![
        HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "hello".to_string(),
            timestamp: STAMP.to_string(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(ToolCall {
            name: "Read".to_string(),
            args: "f".to_string(),
            status: ToolStatus::Ok,
            output: "out".to_string(),
            timestamp: STAMP.to_string(),
            shell: false,
            truncated: false,
            context_output: None,
            arguments: None,
            approval_note: None,
            batch: None,
            call_id: None,
            position: None,
        }),
        HistoryItem::Summary(TurnSummary {
            verb: "Done",
            secs: 12,
            timestamp: STAMP.to_string(),
            shells: 0,
            tokens: 0,
            cached: 0,
            cache_write: 0,
        }),
    ];
    let all: String = transcript_lines(&app, 60)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !all.contains(STAMP),
        "only the user message shows a stamp: {all:?}"
    );
}

#[test]
fn status_line_just_submitted_shows_only_the_verb_and_seconds() {
    // Which clauses show is style-independent, and the comet's frame is the
    // readable one to spell out — so this names the style rather than
    // riding the default (`the_default_status_line_is_the_gravity_track`).
    let line = styled_status_line(
        &status(0, TokenArrow::Down, 0, None),
        None,
        Spinner::Comet,
        200,
    );
    let text = plain(&line);
    assert_eq!(
        text, "(●•·   ) Working… (0s · esc to interrupt)",
        "the bare just-submitted state, comet on the first frame"
    );
    assert!(
        !text.contains("tokens"),
        "no token clause while the tally is 0"
    );
    assert!(!text.contains("Thinking"), "no thinking clause");
}

#[test]
fn format_elapsed_is_bare_seconds_under_a_minute() {
    assert_eq!(format_elapsed(0), "0s");
    assert_eq!(format_elapsed(5), "5s");
    assert_eq!(format_elapsed(59), "59s");
}

#[test]
fn format_elapsed_combines_minutes_and_seconds() {
    // The live seconds keep ticking within the minute, so a "working…"
    // timer never looks frozen.
    assert_eq!(format_elapsed(60), "1m 0s");
    assert_eq!(format_elapsed(90), "1m 30s");
    assert_eq!(format_elapsed(600), "10m 0s");
    assert_eq!(format_elapsed(3_599), "59m 59s");
}

#[test]
fn format_elapsed_combines_hours_and_minutes_past_an_hour() {
    assert_eq!(format_elapsed(3_600), "1h 0m");
    assert_eq!(format_elapsed(3_661), "1h 1m");
    assert_eq!(format_elapsed(7_200), "2h 0m");
    assert_eq!(format_elapsed(7_380), "2h 3m");
    assert_eq!(format_elapsed(90_000), "25h 0m");
}

#[test]
fn format_timeout_drops_the_zero_parts_a_limit_has_no_use_for() {
    // The running `bash` cell's `(22s · wait 1m 50s)` clause names the
    // command's timeout beside its ticking elapsed (docs/tool-streaming.md).
    // A limit reads whole: `2m` for the tool's 120 000 ms default, never
    // `2m 0s` — the elapsed keeps its seconds because it moves.
    assert_eq!(format_timeout(120_000), "2m");
    assert_eq!(format_timeout(110_000), "1m 50s");
    assert_eq!(format_timeout(600_000), "10m");
    assert_eq!(format_timeout(30_000), "30s");
    assert_eq!(format_timeout(3_600_000), "1h");
    assert_eq!(format_timeout(3_661_000), "1h 1m 1s");
}

#[test]
fn format_timeout_keeps_a_sub_second_remainder() {
    // A model may send any millisecond count; a limit shown rounded is a
    // limit misreported, so the fraction stays, trailing zeros trimmed.
    assert_eq!(format_timeout(1_500), "1.5s");
    assert_eq!(format_timeout(250), "0.25s");
    assert_eq!(format_timeout(61_001), "1m 1.001s");
    assert_eq!(format_timeout(0), "0s");
}

#[test]
fn status_line_humanizes_a_long_elapsed_into_minutes_and_seconds() {
    let text = plain(&status_line(&status(100, TokenArrow::Down, 90, None), 200));
    assert!(
        text.contains("(1m 30s · ↓ 100 tokens"),
        "the elapsed reads m/s past a minute: {text:?}"
    );
    assert!(
        !text.contains("(90s"),
        "no bare-seconds form past a minute: {text:?}"
    );
}

#[test]
fn status_line_humanizes_the_thinking_clause_too() {
    // elapsed 200s → 3m 20s, thinking 75s → 1m 15s.
    let text = plain(&status_line(
        &status(150, TokenArrow::Down, 200, Some(75)),
        200,
    ));
    assert!(text.contains("(3m 20s "), "elapsed humanized: {text:?}");
    assert!(
        text.contains("Thinking for 1m 15s"),
        "thinking clause humanized: {text:?}"
    );
}

#[test]
fn summary_humanizes_a_long_turn() {
    let summary = TurnSummary {
        verb: "Done",
        secs: 3_661,
        timestamp: String::new(),
        shells: 0,
        tokens: 0,
        cached: 0,
        cache_write: 0,
    };
    assert_eq!(plain(&summary_lines(&summary, 80)[0]), "Done for 1h 1m");
}

#[test]
fn status_line_shows_the_token_tally_with_a_down_arrow() {
    let text = plain(&status_line(&status(100, TokenArrow::Down, 1, None), 200));
    assert!(
        text.ends_with("Working… (1s · ↓ 100 tokens · esc to interrupt)"),
        "{text:?}"
    );
}

#[test]
fn format_token_count_humanizes_thousands_and_millions() {
    // Real usage tallies run to six digits (the whole context re-billed
    // per agent round) — raw ints stop reading in a one-line status.
    assert_eq!(format_token_count(0), "0");
    assert_eq!(format_token_count(999), "999");
    assert_eq!(format_token_count(1_000), "1k");
    assert_eq!(format_token_count(8_063), "8.1k");
    assert_eq!(format_token_count(15_049), "15k");
    assert_eq!(format_token_count(154_302), "154.3k");
    assert_eq!(format_token_count(2_000_000), "2M");
    assert_eq!(format_token_count(1_234_567), "1.2M");
}

#[test]
fn status_line_humanizes_a_large_token_tally() {
    // Once the real usage snaps the tally past a thousand, the status
    // shows the compact form (the small-estimate case stays bare).
    let text = plain(&status_line(&status(8_063, TokenArrow::Down, 1, None), 200));
    assert!(
        text.contains("↓ 8.1k tokens"),
        "the tally reads compact: {text:?}"
    );
}

#[test]
fn summary_lines_append_the_real_token_usage() {
    // A turn whose backend reported usage commits it with the summary —
    // the cached share beside it as the visible proof caching worked.
    let mut summary = TurnSummary {
        verb: "Done",
        secs: 12,
        timestamp: String::new(),
        shells: 0,
        tokens: 8_203,
        cached: 8_063,
        cache_write: 0,
    };
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens (8.1k cached)"
    );
    // The turn that *primed* the cache says so too: an explicit-caching
    // provider bills that write at a premium, and a receipt reading just
    // `8.2k tokens` made the first turn look like caching did nothing.
    summary.cache_write = 1_204;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens (8.1k cached · 1.2k written)",
        "both halves when both apply"
    );
    summary.cached = 0;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens (1.2k written)",
        "a zero half is omitted"
    );
    summary.cache_write = 0;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens",
        "no parenthetical when nothing was cached or written"
    );
    summary.tokens = 0;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s",
        "a usage-less turn (the dummy) keeps the bare summary"
    );
}

#[test]
fn status_line_flips_to_an_up_arrow_after_a_tool() {
    let text = plain(&status_line(&status(200, TokenArrow::Up, 1, None), 200));
    assert!(
        text.contains("↑ 200 tokens"),
        "up arrow after a tool: {text:?}"
    );
    assert!(!text.contains('↓'), "not the down arrow: {text:?}");
}

#[test]
fn status_line_shows_thinking_only_while_thinking() {
    let thinking = plain(&status_line(
        &status(150, TokenArrow::Down, 1, Some(0)),
        200,
    ));
    assert!(
        thinking.ends_with("Working… (1s · ↓ 150 tokens · Thinking for 0s · esc to interrupt)"),
        "{thinking:?}"
    );
    let not = plain(&status_line(&status(150, TokenArrow::Down, 1, None), 200));
    assert!(
        !not.contains("Thinking"),
        "dropped once thinking ends: {not:?}"
    );
}

#[test]
fn status_line_shows_the_retry_count_between_tokens_and_the_hint() {
    let text = plain(&status_line(&status_retrying(2, 3, 42), 200));
    assert!(
        text.ends_with("Working… (5s · ↑ 42 tokens · retrying 2/3 · esc to interrupt)"),
        "the retry clause sits after the tokens and before the hint: {text:?}"
    );
}

#[test]
fn status_line_shows_the_retry_count_even_with_no_tokens_yet() {
    // A connection that fails before the first byte has only the input
    // counted; the clause still shows.
    let text = plain(&status_line(&status_retrying(1, 3, 0), 200));
    assert!(text.contains("· retrying 1/3 ·"), "{text:?}");
}

#[test]
fn status_line_has_no_retry_clause_when_not_retrying() {
    let text = plain(&status_line(&status(42, TokenArrow::Down, 5, None), 200));
    assert!(!text.contains("retrying"), "{text:?}");
}

#[test]
fn the_retry_clause_stands_out_in_its_own_colour() {
    let line = status_line(&status_retrying(1, 3, 0), 200);
    let retry = line
        .spans
        .iter()
        .find(|s| s.content.contains("retrying"))
        .expect("a span carrying the retry clause");
    assert_eq!(
        retry.style.fg,
        Some(status_retry_color()),
        "the retry clause uses the warning colour, not the dim metric grey"
    );
    assert_ne!(
        status_retry_color(),
        status_detail_color(),
        "the retry colour is distinct from the dim metrics"
    );
}

#[test]
fn status_line_always_ends_with_the_interrupt_hint() {
    // Codex's discoverability hint, the detail's final dim clause in
    // every phase (docs/interrupt.md).
    for status in [
        status(0, TokenArrow::Down, 0, None),
        status(100, TokenArrow::Down, 1, None),
        status(150, TokenArrow::Down, 1, Some(0)),
    ] {
        let text = plain(&status_line(&status, 200));
        assert!(text.ends_with(" · esc to interrupt)"), "{text:?}");
    }
}

#[test]
fn status_spinner_comet_sweeps_between_the_walls_and_its_tail_whips() {
    // The comet spinner advances one frame per SPINNER_INTERVAL (80 ms):
    // the head sweeps out to the right wall dragging its fading tail,
    // bounces (the tail whipping around behind it), sweeps back to the
    // left wall (flush against `(`, using the leftmost cell), and loops.
    let frame_at = |ms: u64| {
        let mut s = status(0, TokenArrow::Down, 0, None);
        s.elapsed = Duration::from_millis(ms);
        let text = plain(&styled_status_line(&s, None, Spinner::Comet, 200));
        text.chars().take_while(|&c| c != 'W').collect::<String>()
    };
    assert_eq!(
        frame_at(0).trim_end(),
        "(●•·   )",
        "head flush at the left wall, tail trailing right"
    );
    assert_eq!(
        frame_at(80).trim_end(),
        "(•●    )",
        "one frame later — moving right, the tail behind (its faint end under the head)"
    );
    assert_eq!(
        frame_at(160).trim_end(),
        "(·•●   )",
        "the full tail streams out behind the head"
    );
    assert_eq!(
        frame_at(400).trim_end(),
        "(   ·•●)",
        "head out at the right wall"
    );
    assert_eq!(
        frame_at(480).trim_end(),
        "(    ●•)",
        "bounced — the tail whips around to trail rightward"
    );
    assert_eq!(
        frame_at(720).trim_end(),
        "( ●•·  )",
        "sweeping back toward the left wall"
    );
    assert_eq!(
        frame_at(800),
        frame_at(0),
        "the sweep loops after a full 10-frame cycle"
    );
}

#[test]
fn status_line_has_a_white_comet_fading_tail_shimmering_verb_and_dim_metrics() {
    let line = styled_status_line(
        &status(0, TokenArrow::Down, 0, None),
        None,
        Spinner::Comet,
        200,
    );
    // The spinner: a white bold comet head dragging a tail that fades
    // through mid grey to the dim detail grey, between dim walls.
    let head = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "●")
        .expect("the comet's head span");
    assert_eq!(head.style.fg, Some(status_color()), "white head");
    assert!(
        head.style.add_modifier.contains(Modifier::BOLD),
        "the head is bold"
    );
    assert_eq!(
        status_color(),
        ai_color(),
        "the status white is the text white"
    );
    let mid = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "•")
        .expect("the tail's mid span");
    assert_eq!(
        mid.style.fg,
        Some(spinner_tail_color()),
        "mid-grey tail cell"
    );
    let faint = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "·")
        .expect("the tail's faint span");
    assert_eq!(
        faint.style.fg,
        Some(status_detail_color()),
        "the tail's end fades to the dim detail grey"
    );
    // The fade is monotonic: head brighter than mid, mid than the end.
    let grey = |c: Option<Color>| match c {
        Some(Color::Rgb(r, _, _)) => r,
        other => panic!("expected an RGB fg, got {other:?}"),
    };
    assert!(
        grey(head.style.fg) > grey(mid.style.fg) && grey(mid.style.fg) > grey(faint.style.fg),
        "the tail fades behind the head"
    );
    assert_eq!(line.spans[0].content.as_ref(), "(", "the left wall span");
    assert_eq!(
        line.spans[0].style.fg,
        Some(status_detail_color()),
        "dim left wall"
    );
    assert_eq!(
        line.spans[SPINNER_SPAN_COUNT - 1].content.as_ref(),
        ") ",
        "the right wall carries the separator space"
    );
    assert_eq!(
        line.spans[SPINNER_SPAN_COUNT - 1].style.fg,
        Some(status_detail_color()),
        "dim right wall"
    );
    // The verb renders one bold span per char (the shimmer), every char a
    // blend between the base and the bright highlight, channel by channel.
    let verb = "Working…";
    let verb_spans = &line.spans[VERB_START..VERB_START + verb.chars().count()];
    assert_eq!(
        verb_spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        verb,
        "per-char verb spans"
    );
    let base = rgb_of(shimmer_base());
    let highlight = rgb_of(shimmer_highlight());
    for span in verb_spans {
        let (r, g, b) = span_rgb(span);
        assert!(
            (base.0..=highlight.0).contains(&r)
                && (base.1..=highlight.1).contains(&g)
                && (base.2..=highlight.2).contains(&b),
            "between the base and the highlight, got ({r},{g},{b})"
        );
        assert!(
            span.style.add_modifier.contains(Modifier::BOLD),
            "verb chars are bold"
        );
    }
    assert_eq!(
        line.spans.last().unwrap().style.fg,
        Some(status_detail_color()),
        "dim metrics"
    );
}

#[test]
fn a_rotated_verb_arrives_with_the_shimmer_band_off_the_text() {
    // The rotation is a whole number of sweeps, so the first frame to wear
    // the next verb — on the boundary, or a frame's re-arm late — catches the
    // band before it reaches the word: the swap never cuts a crest in half
    // (docs/status-indicator.md).
    assert_eq!(VERB_ROTATION.as_millis() % SHIMMER_SWEEP.as_millis(), 0);
    let mut app = App::new();
    app.begin_stream();
    for late_ms in [0, 32] {
        app.set_status_times(VERB_ROTATION + Duration::from_millis(late_ms), None);
        let status = app.status().expect("a turn in flight");
        assert_eq!(status.verb, STATUS_VERBS[1].working, "the swap frame");
        let line = styled_status_line(status, None, Spinner::Comet, 200);
        let verb_cells = status.verb.chars().count() + 1; // + the `…`
        for span in &line.spans[VERB_START..VERB_START + verb_cells] {
            assert_eq!(
                span.style.fg,
                Some(shimmer_base()),
                "no crest on the fresh verb {late_ms} ms in: {span:?}"
            );
        }
    }
}

#[test]
fn status_verb_wave_peaks_where_the_band_is_and_moves_with_time() {
    // "Working…" is 8 chars → period = 8 + 2·10 = 28. The crest sits on
    // char 0 when pos = padding (10), i.e. elapsed = 10/28 · 2 s ≈ 715 ms:
    // char 0 must then be brighter than char 7 (7 chars away — outside the
    // band, so at the base colour).
    let mut at_crest = status(0, TokenArrow::Down, 0, None);
    at_crest.elapsed = Duration::from_millis(715);
    let crest_on_first = status_line(&at_crest, 200);
    let first = span_rgb(&crest_on_first.spans[VERB_START]);
    let last = span_rgb(&crest_on_first.spans[VERB_START + 7]);
    assert!(
        first.0 > last.0,
        "the band's crest is brighter than off-band chars: {first:?} vs {last:?}"
    );
    assert_eq!(
        last.0,
        rgb_of(shimmer_base()).0,
        "off-band chars sit at the base"
    );

    // Half a sweep later the band has moved on: char 0 is no longer the peak.
    let mut moved_on = status(0, TokenArrow::Down, 0, None);
    moved_on.elapsed = Duration::from_millis(715 + 1000);
    let first_later = span_rgb(&status_line(&moved_on, 200).spans[VERB_START]);
    assert!(
        first_later.0 < first.0,
        "the wave moved off char 0 as time advanced: {first_later:?} vs {first:?}"
    );
}

#[test]
fn summary_lines_is_a_single_dim_bulletless_line() {
    let summary = TurnSummary {
        verb: "Done",
        secs: 20,
        timestamp: String::new(),
        shells: 0,
        tokens: 0,
        cached: 0,
        cache_write: 0,
    };
    let lines = summary_lines(&summary, 80);
    assert_eq!(lines.len(), 1, "one line");
    assert_eq!(plain(&lines[0]), "Done for 20s");
    assert!(!plain(&lines[0]).contains('●'), "no bullet");
    assert_eq!(lines[0].spans[0].style.fg, Some(status_done_color()), "dim");
}

#[test]
fn conversation_lines_renders_a_committed_turn_summary() {
    let history = [
        msg(Role::Assistant, "all done"),
        HistoryItem::Summary(TurnSummary {
            verb: "Done",
            secs: 7,
            timestamp: String::new(),
            shells: 0,
            tokens: 0,
            cached: 0,
            cache_write: 0,
        }),
    ];
    let texts: Vec<String> = conversation_lines(&history, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t == "● all done"));
    assert!(
        texts.iter().any(|t| t == "Done for 7s"),
        "the summary flows inline: {texts:?}"
    );
}

#[test]
fn render_live_shell_run_shows_running_elapsed_and_hides_the_status_line() {
    // A `!` shell run suppresses the spinner status line entirely and shows
    // its elapsed in the `⎿ Running… (Ns)` preview (req 3): the strip is
    // the preview cell — the running row + the live-only Ctrl+B hint
    // (docs/background.md) — plus its gap, then the box's top rule — no
    // status row, no `esc to interrupt` hint. See docs/shell-command.md.
    let mut app = App::new();
    app.begin_shell("sleep 30");
    app.set_status_times(Duration::from_secs(3), None);
    // Past the hint delay so the Ctrl+B hint row shows (docs/background.md).
    app.set_command_elapsed(Some(Duration::from_secs(3)));
    let mut buf = buffer(40, 6); // preview (2) + gap + (two rules + one input)
    render_live(buf.area, &mut buf, &app);

    assert_eq!(
        row(&buf, 0, 40).trim_end(),
        "  ⎿  Running… (3s)",
        "the running preview carries the elapsed the status line would have"
    );
    assert_eq!(
        row(&buf, 1, 40).trim(),
        "(ctrl+b to run in background)",
        "the live cell hints the Ctrl+B background handoff"
    );
    assert!(
        row(&buf, 2, 40).trim().is_empty(),
        "blank gap row below the preview"
    );
    assert_eq!(
        buf[(0, 3)].symbol(),
        "─",
        "the box's top rule sits right under the preview gap — no status line between"
    );
    let all: String = (0..6).map(|y| row(&buf, y, 40)).collect();
    assert!(
        !all.contains("esc to interrupt"),
        "a shell run shows no status line (and so no interrupt hint): {all:?}"
    );
    assert_eq!(
        all.matches("Running…").count(),
        1,
        "`Running…` appears only in the preview, not also in a status line"
    );
}

#[test]
fn the_transcript_expands_the_compaction_summary_dim_below_the_marker() {
    let compaction = bare_compaction("kept the gist");
    let lines = compaction_full_lines(&compaction, 80);
    assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
    assert_eq!(plain(&lines[1]), format!("{INDENT}kept the gist"));
    assert_eq!(
        lines[1].spans[1].style.fg,
        Some(tool_dim_color()),
        "the summary body is dim"
    );
}

#[test]
fn an_empty_compaction_summary_expands_to_just_the_marker() {
    assert_eq!(compaction_full_lines(&bare_compaction(""), 80).len(), 1);
}

#[test]
fn summary_lines_append_the_still_running_shell_count() {
    let mut summary = TurnSummary {
        verb: "Done",
        secs: 22,
        timestamp: String::new(),
        shells: 3,
        tokens: 0,
        cached: 0,
        cache_write: 0,
    };
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 22s · 3 shells still running"
    );
    summary.shells = 1;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 22s · 1 shell still running",
        "singular for one shell"
    );
    summary.shells = 0;
    assert_eq!(plain(&summary_lines(&summary, 80)[0]), "Done for 22s");
}

/// The RGB triple of a span's foreground (panics on a non-RGB colour).
fn span_rgb(span: &Span) -> (u8, u8, u8) {
    match span.style.fg {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        other => panic!("expected an RGB fg, got {other:?}"),
    }
}

/// The spinner contributes the first [`SPINNER_SPAN_COUNT`] spans of the
/// status line; the shimmering verb's per-char spans start right after.
const VERB_START: usize = SPINNER_SPAN_COUNT;

/// A live status carrying a retry indicator (verb fixed to "Working").
fn status_retrying(attempt: u32, max: u32, tokens: usize) -> TurnStatus {
    let mut s = status(tokens, TokenArrow::Up, 5, None);
    s.retry = Some(RetryInfo { attempt, max });
    s
}

#[test]
fn the_status_line_clamps_to_the_width_with_an_ellipsis() {
    // The status line is one animated strip row by design (STATUS_ROWS is
    // fixed, and its shimmer/spinner spans must never commit) — so instead
    // of wrapping it degrades with a dim `…`, never a silent paint-clip
    // that swallowed the thinking clause and the esc hint at narrow widths.
    let wide = status_line(&status(12_345, TokenArrow::Down, 150, Some(75)), 200);
    assert!(
        plain(&wide).contains("esc to interrupt"),
        "a wide terminal keeps the whole line: {:?}",
        plain(&wide)
    );
    let narrow = status_line(&status(12_345, TokenArrow::Down, 150, Some(75)), 40);
    let text = plain(&narrow);
    assert!(
        crate::ui::wrap::cols(&text) <= 40,
        "never overflows the width: {text:?}"
    );
    assert!(text.ends_with('…'), "the cut says so: {text:?}");
    assert!(text.contains("Working"), "the verb survives: {text:?}");
}

#[test]
fn the_turn_summary_wraps_to_the_width() {
    // The committed summary is permanent scrollback — its token/cache/shell
    // clauses are the turn's receipts, so a narrow terminal wraps them whole
    // instead of losing the tail (the old `_width` was ignored).
    let summary = TurnSummary {
        verb: "Done",
        secs: 95,
        timestamp: String::new(),
        shells: 2,
        tokens: 1_500_000,
        cached: 1_200_000,
        cache_write: 0,
    };
    let lines = summary_lines(&summary, 30);
    assert!(lines.len() > 1, "the summary wrapped: {lines:?}");
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 30,
            "no row leaks past the width: {:?}",
            plain(line)
        );
        assert!(
            line.spans
                .iter()
                .all(|s| s.style.fg == Some(status_done_color())),
            "every row keeps the summary's dim dress: {line:?}"
        );
    }
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        all, "Done for 1m 35s · 1.5M tokens (1.2M cached) · 2 shells still running",
        "nothing lost, nothing reordered"
    );
}

// ===== the spinner styles (docs/spinner.md) =====

/// The frame `styled_status_line` opens with for `spinner` at `ms` — the text
/// before the verb, trailing separator trimmed.
fn styled_frame(spinner: Spinner, ms: u64) -> String {
    let mut s = status(0, TokenArrow::Down, 0, None);
    s.elapsed = Duration::from_millis(ms);
    let text = plain(&styled_status_line(&s, None, spinner, 200));
    text.chars()
        .take_while(|&c| c != 'W')
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The status line's first span (the spinner's glyph cell for a one-cell
/// style; the comet's left wall) for `spinner` at `ms`.
fn first_span(spinner: Spinner, ms: u64) -> Span<'static> {
    let mut s = status(0, TokenArrow::Down, 0, None);
    s.elapsed = Duration::from_millis(ms);
    styled_status_line(&s, None, spinner, 200).spans[0].clone()
}

#[test]
fn the_default_status_line_is_the_gravity_track() {
    // `status_line` / `status_line_with_verb` are the catalog's *default*
    // style, whatever it is (docs/spinner.md) — byte-for-byte, at every
    // phase of the animation.
    let mut s = status(42, TokenArrow::Down, 3, None);
    for ms in [0u64, 80, 400, 715] {
        s.elapsed = Duration::from_millis(ms);
        assert_eq!(
            status_line(&s, 200),
            styled_status_line(&s, None, Spinner::Gravity, 200),
            "at {ms} ms"
        );
        assert_eq!(
            status_line_with_verb(&s, Some("Testing"), 200),
            styled_status_line(&s, Some("Testing"), Spinner::Gravity, 200),
            "with a verb override at {ms} ms"
        );
    }
    // …and that is what a session with no chosen style actually opens with:
    // the ball on its floor against the left wall.
    s.elapsed = Duration::ZERO;
    assert!(
        plain(&status_line(&s, 200)).starts_with("⣤⣀⣀⣀⣀⣀⣀⣀ Working…"),
        "the default line wears the gravity track: {:?}",
        plain(&status_line(&s, 200))
    );
}

#[test]
fn every_style_animates_in_single_width_fixed_width_frames() {
    // A wide glyph would shear the verb and the metrics after it
    // (docs/table-streaming.md "Wide glyphs"), and a frame of a different
    // width would jitter them — so every frame of every style is the same
    // width as its first, every glyph one column.
    for spinner in Spinner::ALL {
        let first = styled_frame(spinner, 0);
        assert!(!first.is_empty(), "{}: empty frame", spinner.name());
        for ms in (0..2_400).step_by(10) {
            let frame = styled_frame(spinner, ms);
            assert_eq!(
                crate::ui::wrap::cols(&frame),
                frame.chars().count(),
                "{} at {ms} ms: wide glyph in {frame:?}",
                spinner.name()
            );
            assert_eq!(
                crate::ui::wrap::cols(&frame),
                crate::ui::wrap::cols(&first),
                "{} at {ms} ms: {frame:?} is not the width of {first:?}",
                spinner.name()
            );
        }
    }
}

#[test]
fn each_style_opens_the_line_with_its_own_first_frame() {
    let expect = [
        (Spinner::Comet, "(●•·   )"),
        // The ball on the floor against the left wall — its two dot-columns
        // fill the first cell's lower half, the floor runs under the rest.
        (Spinner::Gravity, "⣤⣀⣀⣀⣀⣀⣀⣀"),
        (Spinner::Sparkle, "·"),
        (Spinner::Dots, "⠋"),
        (Spinner::Blocks, "▙"),
        (Spinner::Pulse, "●"),
        (Spinner::Bars, "▁"),
        (Spinner::Line, "|"),
    ];
    for (spinner, frame) in expect {
        assert_eq!(styled_frame(spinner, 0), frame, "{}", spinner.name());
        let mut s = status(0, TokenArrow::Down, 0, None);
        s.elapsed = Duration::ZERO;
        let text = plain(&styled_status_line(&s, None, spinner, 200));
        assert!(
            text.starts_with(&format!("{frame} Working… (0s · esc to interrupt)")),
            "{}: one separator space between the spinner and the verb: {text:?}",
            spinner.name()
        );
    }
}

#[test]
fn a_one_cell_style_puts_the_glyph_and_its_separator_in_one_span() {
    // The comet spends eight spans (one per cell); a one-cell style spends
    // one — the glyph with the trailing separator — so the verb's shimmer
    // spans start at index 1, and the cell is the white bold head.
    let span = first_span(Spinner::Dots, 0);
    assert_eq!(span.content.as_ref(), "⠋ ");
    assert_eq!(span.style.fg, Some(status_color()), "the white head");
    assert!(
        span.style.add_modifier.contains(Modifier::BOLD),
        "bold head"
    );
    let mut s = status(0, TokenArrow::Down, 0, None);
    s.elapsed = Duration::ZERO;
    let line = styled_status_line(&s, None, Spinner::Dots, 200);
    assert_eq!(
        line.spans[1].content.as_ref(),
        "W",
        "the verb follows at once"
    );
}

#[test]
fn the_sparkle_blooms_into_a_star_and_back_in_the_banner_gradient() {
    // Ten frames at 120 ms: · ✢ ✳ ✶ ✻ ✽ ✻ ✶ ✳ ✢ — a spark opening into a
    // heavy star and closing again, its colour walking the header's cyan →
    // blue gradient with the bloom (docs/header.md), so the theme's accent
    // rides the status line.
    let frames: Vec<String> = (0..10)
        .map(|i| styled_frame(Spinner::Sparkle, i * 120))
        .collect();
    assert_eq!(
        frames,
        ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"],
        "the bloom and its fade"
    );
    assert_eq!(
        styled_frame(Spinner::Sparkle, 1200),
        "·",
        "loops after 1.2 s"
    );
    let spark = first_span(Spinner::Sparkle, 0);
    let star = first_span(Spinner::Sparkle, 600);
    let (r0, g0, b0) = rgb_of(header_gradient_start());
    let (r1, g1, b1) = rgb_of(header_gradient_end());
    assert_eq!(
        spark.style.fg,
        Some(Color::Rgb(r0, g0, b0)),
        "the spark is cyan"
    );
    assert_eq!(
        star.style.fg,
        Some(Color::Rgb(r1, g1, b1)),
        "the full star is blue"
    );
    assert!(spark.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn the_pulse_dot_breathes_dim_to_bright_without_moving() {
    // One glyph, coloured by a raised-cosine breath at the running bullet's
    // cadence (docs/spinner.md) — dim at the bottom of the breath, white at the
    // top, half a period later — so it swells rather than flicks.
    assert_eq!(styled_frame(Spinner::Pulse, 0), "●");
    assert_eq!(styled_frame(Spinner::Pulse, 500), "●");
    let (dr, dg, db) = rgb_of(tool_pulse_dim());
    assert_eq!(
        first_span(Spinner::Pulse, 0).style.fg,
        Some(Color::Rgb(dr, dg, db)),
        "the breath starts dim"
    );
    assert_eq!(
        first_span(Spinner::Pulse, 500).style.fg,
        Some(spinner_pulse_bright()),
        "half a breath later it is at the crest — the text colour"
    );
    let r = rgb_of(spinner_pulse_bright()).0;
    let quarter = span_rgb(&first_span(Spinner::Pulse, 250)).0;
    assert!(dr < quarter && quarter < r, "the swell is gradual");
}

#[test]
fn the_bars_rise_and_fall_brightening_with_height() {
    let frames: Vec<String> = (0..14)
        .map(|i| styled_frame(Spinner::Bars, i * 60))
        .collect();
    assert_eq!(frames.concat(), "▁▂▃▄▅▆▇█▇▆▅▄▃▂", "a level meter bouncing");
    let low = span_rgb(&first_span(Spinner::Bars, 0)).0;
    let high = span_rgb(&first_span(Spinner::Bars, 7 * 60)).0;
    assert!(low < high, "the full bar is the brightest: {low} vs {high}");
    assert_eq!(
        first_span(Spinner::Bars, 7 * 60).style.fg,
        Some(spinner_bars_high()),
        "…and it is the text colour"
    );
}

#[test]
fn the_blocks_turn_through_the_banner_gradient() {
    // The mascots' own quadrant glyphs (docs/mascot.md) turning clockwise,
    // the missing quadrant walking round, in the banner's gradient.
    let frames: Vec<String> = (0..4)
        .map(|i| styled_frame(Spinner::Blocks, i * 150))
        .collect();
    assert_eq!(frames, ["▙", "▛", "▜", "▟"]);
    let (r0, g0, b0) = rgb_of(header_gradient_start());
    let (r1, g1, b1) = rgb_of(header_gradient_end());
    assert_eq!(
        first_span(Spinner::Blocks, 0).style.fg,
        Some(Color::Rgb(r0, g0, b0))
    );
    assert_eq!(
        first_span(Spinner::Blocks, 450).style.fg,
        Some(Color::Rgb(r1, g1, b1))
    );
}

#[test]
fn the_classic_styles_step_one_glyph_per_interval() {
    let dots: Vec<String> = (0..10)
        .map(|i| styled_frame(Spinner::Dots, i * 80))
        .collect();
    assert_eq!(dots.concat(), "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
    let line: Vec<String> = (0..4)
        .map(|i| styled_frame(Spinner::Line, i * 100))
        .collect();
    assert_eq!(line.concat(), "|/-\\");
    assert_eq!(
        styled_frame(Spinner::Dots, 800),
        "⠋",
        "dots loop after 0.8 s"
    );
}

/// The dot count of a braille frame's `i`th cell.
fn braille_dots(frame: &str, i: usize) -> u32 {
    let c = frame.chars().nth(i).expect("a cell");
    assert!(
        ('\u{2800}'..='\u{28FF}').contains(&c),
        "{c:?} is not a braille cell"
    );
    (c as u32 - 0x2800).count_ones()
}

/// The spans of a track style's frame at `ms` — the status line's first
/// eight spans, one per cell.
fn track_spans(spinner: Spinner, ms: u64) -> Vec<Span<'static>> {
    let mut s = status(0, TokenArrow::Down, 0, None);
    s.elapsed = Duration::from_millis(ms);
    styled_status_line(&s, None, spinner, 200).spans[..8].to_vec()
}

#[test]
fn the_gravity_ball_hops_along_the_floor_and_touches_down_at_each_wall() {
    // An eight-cell braille track: a floor along the bottom dot row, a 2×2
    // dot ball ping-ponging along it at constant speed (one round trip per
    // SPINNER_GRAVITY_SWEEP) while it hops on a parabola (one hop per
    // SPINNER_GRAVITY_HOP) — four hops a round trip, so it lands exactly as
    // it meets each wall.
    assert_eq!(
        styled_frame(Spinner::Gravity, 0),
        "⣤⣀⣀⣀⣀⣀⣀⣀",
        "t=0: on the floor against the left wall"
    );
    assert_eq!(
        styled_frame(Spinner::Gravity, 1_200),
        "⣀⣀⣀⣀⣀⣀⣀⣤",
        "half a sweep: on the floor against the right wall"
    );
    assert_eq!(
        styled_frame(Spinner::Gravity, 2_400),
        "⣤⣀⣀⣀⣀⣀⣀⣀",
        "a full sweep later it is back where it started"
    );
    assert_eq!(
        styled_frame(Spinner::Gravity, 150),
        "⣘⣃⣀⣀⣀⣀⣀⣀",
        "a quarter hop in: the ball is up in the top two dot rows, one dot column in"
    );
    assert_eq!(
        styled_frame(Spinner::Gravity, 300),
        "⣀⣘⣃⣀⣀⣀⣀⣀",
        "the apex: the whole ball in the top two dot rows, straddling two cells, the floor intact under it"
    );
    // Every frame keeps the floor under every cell.
    for ms in (0..2_400).step_by(30) {
        let frame = styled_frame(Spinner::Gravity, ms);
        for (i, c) in frame.chars().enumerate() {
            let bits = c as u32 - 0x2800;
            assert_eq!(
                bits & 0xC0,
                0xC0,
                "at {ms} ms cell {i} lost its floor: {frame}"
            );
        }
    }
}

#[test]
fn the_gravity_ball_wears_the_gradient_over_a_dim_floor() {
    let (r0, g0, b0) = rgb_of(header_gradient_start());
    let (r1, g1, b1) = rgb_of(header_gradient_end());
    let at_left = track_spans(Spinner::Gravity, 0);
    assert_eq!(
        at_left[0].style.fg,
        Some(Color::Rgb(r0, g0, b0)),
        "the ball at the left wall is cyan"
    );
    for span in &at_left[1..] {
        assert_eq!(
            span.style.fg,
            Some(status_detail_color()),
            "the bare floor is dim: {:?}",
            span.content
        );
    }
    let at_right = track_spans(Spinner::Gravity, 1_200);
    assert_eq!(
        at_right[7].style.fg,
        Some(Color::Rgb(r1, g1, b1)),
        "the ball at the right wall is blue"
    );
    assert_eq!(
        at_right[7].content.as_ref(),
        "⣤ ",
        "the last cell carries the separator space"
    );
}

#[test]
fn the_wave_rolls_down_the_track_and_reflects_off_the_walls() {
    // One dot per dot column follows a sine across the eight cells (so every
    // cell holds exactly two dots), one wavelength spanning the track; the
    // phase ping-pongs, so the crawl reverses at each end of the sweep — and
    // because it travels a whole number of wavelengths each way, the frame
    // at the reversal is the frame it started from, with no seam.
    let start = styled_frame(Spinner::Wave, 0);
    for i in 0..8 {
        assert_eq!(braille_dots(&start, i), 2, "cell {i} of {start}");
    }
    let later = styled_frame(Spinner::Wave, 200);
    assert_ne!(start, later, "the wave moves");
    assert_eq!(
        styled_frame(Spinner::Wave, 1_700),
        start,
        "at the turn (3 wavelengths on) the wave is back in phase"
    );
    assert_eq!(
        styled_frame(Spinner::Wave, 1_700 - 200),
        styled_frame(Spinner::Wave, 1_700 + 200),
        "the crawl back retraces the crawl out"
    );
    assert_eq!(
        styled_frame(Spinner::Wave, 3_400),
        start,
        "a full sweep later it is back where it started"
    );
}

#[test]
fn the_wave_wears_the_gradient_across_the_track() {
    let (r0, g0, b0) = rgb_of(header_gradient_start());
    let (r1, g1, b1) = rgb_of(header_gradient_end());
    let spans = track_spans(Spinner::Wave, 0);
    assert_eq!(
        spans[0].style.fg,
        Some(Color::Rgb(r0, g0, b0)),
        "cyan at the left"
    );
    assert_eq!(
        spans[7].style.fg,
        Some(Color::Rgb(r1, g1, b1)),
        "blue at the right"
    );
    let reds: Vec<u8> = spans.iter().map(|s| span_rgb(s).0).collect();
    assert!(
        reds.windows(2).all(|w| w[0] <= w[1]),
        "the wash runs monotonically across the track: {reds:?}"
    );
}

#[test]
fn render_live_wears_the_session_spinner_on_the_status_row() {
    // The strip's status line is built for the session's chosen style — the
    // one thing `/spinner` exists to change — through the same renderer the
    // picker previews with.
    let mut app = App::new();
    app.set_spinner(Spinner::Dots);
    app.begin_stream();
    // 3.2 s: 40 dots frames on — back at the first (40 ≡ 0 mod 10).
    app.set_status_times(Duration::from_millis(3_200), None);
    let mut buf = buffer(60, 5); // status + gap + (two rules + one input)
    render_live(buf.area, &mut buf, &app);
    assert_eq!(
        row(&buf, 0, 60).trim_end(),
        "⠋ Working… (3s · esc to interrupt)",
        "the status row opens with the dots spinner"
    );
    assert_eq!(buf[(0, 0)].fg, status_color(), "the white head");
}

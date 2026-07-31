//! The live status line and the committed summary
//! (`docs/status-indicator.md`).

use super::*;
use crate::ui::message::compaction_full_lines;
use crate::ui::theme::{
    AI_COLOR, INDENT, SHIMMER_BASE, SPINNER_SPAN_COUNT, SPINNER_TAIL_COLOR, STATUS_COLOR,
    STATUS_DETAIL_COLOR, STATUS_DONE_COLOR, STATUS_RETRY_COLOR, TOOL_DIFF_ADD_COLOR,
    TOOL_DIFF_DEL_COLOR, TOOL_DIM_COLOR,
};

#[test]
fn edit_cell_colours_the_summary_counts() {
    let output = "Updated a.rs (+6 -2)\n1 +x";
    let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
    let summary = &lines[1];
    let plus = summary
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "+6")
        .unwrap();
    assert_eq!(plus.style.fg, Some(TOOL_DIFF_ADD_COLOR));
    let minus = summary
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "-2")
        .unwrap();
    assert_eq!(minus.style.fg, Some(TOOL_DIFF_DEL_COLOR));
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
        }),
        HistoryItem::Summary(TurnSummary {
            verb: "Done",
            secs: 12,
            timestamp: STAMP.to_string(),
            shells: 0,
            tokens: 0,
            cached: 0,
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
    let line = status_line(&status(0, TokenArrow::Down, 0, None));
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
fn status_line_humanizes_a_long_elapsed_into_minutes_and_seconds() {
    let text = plain(&status_line(&status(100, TokenArrow::Down, 90, None)));
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
    let text = plain(&status_line(&status(150, TokenArrow::Down, 200, Some(75))));
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
    };
    assert_eq!(plain(&summary_lines(&summary, 80)[0]), "Done for 1h 1m");
}

#[test]
fn status_line_shows_the_token_tally_with_a_down_arrow() {
    let text = plain(&status_line(&status(100, TokenArrow::Down, 1, None)));
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
    let text = plain(&status_line(&status(8_063, TokenArrow::Down, 1, None)));
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
    };
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens (8.1k cached)"
    );
    summary.cached = 0;
    assert_eq!(
        plain(&summary_lines(&summary, 80)[0]),
        "Done for 12s · 8.2k tokens",
        "no parenthetical when nothing was cached"
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
    let text = plain(&status_line(&status(200, TokenArrow::Up, 1, None)));
    assert!(
        text.contains("↑ 200 tokens"),
        "up arrow after a tool: {text:?}"
    );
    assert!(!text.contains('↓'), "not the down arrow: {text:?}");
}

#[test]
fn status_line_shows_thinking_only_while_thinking() {
    let thinking = plain(&status_line(&status(150, TokenArrow::Down, 1, Some(0))));
    assert!(
        thinking.ends_with("Working… (1s · ↓ 150 tokens · Thinking for 0s · esc to interrupt)"),
        "{thinking:?}"
    );
    let not = plain(&status_line(&status(150, TokenArrow::Down, 1, None)));
    assert!(
        !not.contains("Thinking"),
        "dropped once thinking ends: {not:?}"
    );
}

#[test]
fn status_line_shows_the_retry_count_between_tokens_and_the_hint() {
    let text = plain(&status_line(&status_retrying(2, 3, 42)));
    assert!(
        text.ends_with("Working… (5s · ↑ 42 tokens · retrying 2/3 · esc to interrupt)"),
        "the retry clause sits after the tokens and before the hint: {text:?}"
    );
}

#[test]
fn status_line_shows_the_retry_count_even_with_no_tokens_yet() {
    // A connection that fails before the first byte has only the input
    // counted; the clause still shows.
    let text = plain(&status_line(&status_retrying(1, 3, 0)));
    assert!(text.contains("· retrying 1/3 ·"), "{text:?}");
}

#[test]
fn status_line_has_no_retry_clause_when_not_retrying() {
    let text = plain(&status_line(&status(42, TokenArrow::Down, 5, None)));
    assert!(!text.contains("retrying"), "{text:?}");
}

#[test]
fn the_retry_clause_stands_out_in_its_own_colour() {
    let line = status_line(&status_retrying(1, 3, 0));
    let retry = line
        .spans
        .iter()
        .find(|s| s.content.contains("retrying"))
        .expect("a span carrying the retry clause");
    assert_eq!(
        retry.style.fg,
        Some(STATUS_RETRY_COLOR),
        "the retry clause uses the warning colour, not the dim metric grey"
    );
    assert_ne!(
        STATUS_RETRY_COLOR, STATUS_DETAIL_COLOR,
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
        let text = plain(&status_line(&status));
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
        let text = plain(&status_line(&s));
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
    let line = status_line(&status(0, TokenArrow::Down, 0, None));
    // The spinner: a white bold comet head dragging a tail that fades
    // through mid grey to the dim detail grey, between dim walls.
    let head = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "●")
        .expect("the comet's head span");
    assert_eq!(head.style.fg, Some(STATUS_COLOR), "white head");
    assert!(
        head.style.add_modifier.contains(Modifier::BOLD),
        "the head is bold"
    );
    assert_eq!(STATUS_COLOR, AI_COLOR, "the status white is the text white");
    let mid = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "•")
        .expect("the tail's mid span");
    assert_eq!(mid.style.fg, Some(SPINNER_TAIL_COLOR), "mid-grey tail cell");
    let faint = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "·")
        .expect("the tail's faint span");
    assert_eq!(
        faint.style.fg,
        Some(STATUS_DETAIL_COLOR),
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
        Some(STATUS_DETAIL_COLOR),
        "dim left wall"
    );
    assert_eq!(
        line.spans[SPINNER_SPAN_COUNT - 1].content.as_ref(),
        ") ",
        "the right wall carries the separator space"
    );
    assert_eq!(
        line.spans[SPINNER_SPAN_COUNT - 1].style.fg,
        Some(STATUS_DETAIL_COLOR),
        "dim right wall"
    );
    // The verb renders one bold span per char (the shimmer), every char a
    // greyscale white between the base and the bright highlight.
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
    for span in verb_spans {
        let (r, g, b) = span_rgb(span);
        assert!(r == g && g == b, "greyscale white, got ({r},{g},{b})");
        assert!(r >= SHIMMER_BASE.0, "never dimmer than the base");
        assert!(
            span.style.add_modifier.contains(Modifier::BOLD),
            "verb chars are bold"
        );
    }
    assert_eq!(
        line.spans.last().unwrap().style.fg,
        Some(STATUS_DETAIL_COLOR),
        "dim metrics"
    );
}

#[test]
fn status_verb_wave_peaks_where_the_band_is_and_moves_with_time() {
    // "Working…" is 8 chars → period = 8 + 2·10 = 28. The crest sits on
    // char 0 when pos = padding (10), i.e. elapsed = 10/28 · 2 s ≈ 715 ms:
    // char 0 must then be brighter than char 7 (7 chars away — outside the
    // band, so at the base colour).
    let mut at_crest = status(0, TokenArrow::Down, 0, None);
    at_crest.elapsed = Duration::from_millis(715);
    let crest_on_first = status_line(&at_crest);
    let first = span_rgb(&crest_on_first.spans[VERB_START]);
    let last = span_rgb(&crest_on_first.spans[VERB_START + 7]);
    assert!(
        first.0 > last.0,
        "the band's crest is brighter than off-band chars: {first:?} vs {last:?}"
    );
    assert_eq!(last.0, SHIMMER_BASE.0, "off-band chars sit at the base");

    // Half a sweep later the band has moved on: char 0 is no longer the peak.
    let mut moved_on = status(0, TokenArrow::Down, 0, None);
    moved_on.elapsed = Duration::from_millis(715 + 1000);
    let first_later = span_rgb(&status_line(&moved_on).spans[VERB_START]);
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
    };
    let lines = summary_lines(&summary, 80);
    assert_eq!(lines.len(), 1, "one line");
    assert_eq!(plain(&lines[0]), "Done for 20s");
    assert!(!plain(&lines[0]).contains('●'), "no bullet");
    assert_eq!(lines[0].spans[0].style.fg, Some(STATUS_DONE_COLOR), "dim");
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
        }),
    ];
    let texts: Vec<String> = conversation_lines(&history, 80)
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
        Some(TOOL_DIM_COLOR),
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

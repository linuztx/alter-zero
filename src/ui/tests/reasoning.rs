//! The thinking stream's three renderers: the live block in the strip, the
//! collapsed `Thought for …` line, and its Ctrl+O expansion.
//! See `docs/thinking-stream.md`.

use super::*;
use crate::app::Reasoning;
use crate::ui::live::preview_lines;
use crate::ui::reasoning::{live_reasoning_lines, reasoning_full_lines, reasoning_live_full_lines};
use crate::ui::theme::{
    SHIMMER_SWEEP, TOOL_PULSE_PERIOD, reasoning_label_color, reasoning_shimmer_base, shimmer_base,
    shimmer_highlight,
};
use crate::ui::wrap::cols;

fn thought(text: &str, secs: u64, tokens: usize) -> Reasoning {
    Reasoning {
        text: text.to_string(),
        secs,
        tokens,
        timestamp: String::new(),
    }
}

#[test]
fn the_collapsed_line_reads_thought_for_elapsed_tokens_and_the_expand_hint() {
    // The shape the feature was asked for, with the same humanizers the
    // status line and the turn summary use.
    let lines = reasoning_lines(&thought("…", 65, 1_500), 80);
    assert_eq!(lines.len(), 1, "one unwrapped line, like summary_lines");
    assert_eq!(
        plain(&lines[0]),
        "Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)"
    );
}

#[test]
fn the_collapsed_line_hides_an_unknown_token_count() {
    // A backend that reported no usage and estimated nothing (an old rollout)
    // keeps the bare shape rather than claiming `· 0 tokens`.
    let lines = reasoning_lines(&thought("…", 8, 0), 80);
    assert_eq!(plain(&lines[0]), "Thought for 8s (ctrl+o to expand)");
}

#[test]
fn the_settled_line_carries_no_bullet() {
    // The `● Thinking…` header meant *something is happening*; nothing is any
    // more, so what is left is a fact about the turn — `Done for Ns`'s shape.
    let lines = reasoning_lines(&thought("…", 3, 12), 80);
    assert!(
        plain(&lines[0]).starts_with("Thought for"),
        "no bullet, no indent: {:?}",
        plain(&lines[0])
    );
}

#[test]
fn the_settled_line_is_dim_throughout() {
    // `Done for Ns`'s exact dress. A finished thought is a footnote about work
    // already done, so it settles into the transcript instead of competing
    // with the reply it sits above — the weight belongs to the live block.
    let lines = reasoning_lines(&thought("…", 3, 12), 80);
    let [span] = lines[0].spans.as_slice() else {
        panic!("one tone across the whole row, got {:?}", lines[0].spans);
    };
    assert_eq!(
        span.content,
        "Thought for 3s · 12 tokens (ctrl+o to expand)"
    );
    assert_eq!(span.style.fg, Some(reasoning_label_color()));
    assert!(!span.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn the_transcript_expands_the_whole_chain_of_thought() {
    let reasoning = thought("first thought\n\nsecond thought", 40, 12);
    let lines = reasoning_full_lines(&reasoning, 40);
    let text: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        text[0], "Thought for 40s · 12 tokens",
        "the expansion drops the hint — this IS the expansion: {text:?}"
    );
    assert_eq!(
        text[1], "  ⎿  first thought",
        "the body hangs in the gutter"
    );
    assert!(
        text.iter().any(|l| l.contains("second thought")),
        "{text:?}"
    );
}

#[test]
fn the_expansion_drops_the_thoughts_blank_edges_but_keeps_its_paragraph_breaks() {
    // A model's reasoning often opens and closes on blank lines
    // (`<think>\n…\n\n</think>`). Those are not paragraph breaks: the opening
    // one pushed the `⎿` corner off the first row of thought, and the closing
    // ones stacked on the spacer the transcript puts after every item. The
    // blank line *between* two paragraphs stays.
    let reasoning = thought("\n\nfirst thought\n\nsecond thought\n\n \n", 40, 12);
    let text: Vec<String> = reasoning_full_lines(&reasoning, 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        text,
        [
            "Thought for 40s · 12 tokens",
            "  ⎿  first thought",
            "",
            "     second thought",
        ]
    );
}

#[test]
fn an_open_phase_in_the_transcript_drops_its_blank_edges_too() {
    // The live expansion shares the body: a phase whose newest delta closed a
    // paragraph shows no blank row under it until the next paragraph starts.
    let text: Vec<String> = reasoning_live_full_lines("\nweighing options\n\n", 60)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(text, ["● Thinking…", "  ⎿  weighing options"]);
}

#[test]
fn a_thought_closing_on_blank_lines_sits_one_blank_row_above_the_next_cell() {
    // The reported Ctrl+O screen: the thought ended on `\n\n`, and the pager
    // put three blank rows between its last line and the `● Bash(…)` under
    // it, where every other pair of cells has one.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(
        "I need to find the current NVDA stock price, so I'll try fetching it \
         from a finance data source to verify the number before answering.\n\n",
    );
    app.finish_reasoning(0);
    app.start_tool("Bash", "curl -s -m 20 https://example.com", None);
    app.end_tool("{}", true);
    let texts: Vec<String> = transcript_lines(&app, 87)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let bash = texts
        .iter()
        .position(|t| t.starts_with("● Bash("))
        .expect("the call is in the transcript");
    assert_eq!(texts[bash - 1], "", "one spacer above the call: {texts:?}");
    assert!(
        texts[bash - 2].ends_with("before answering."),
        "and the thought's last line right above it: {texts:?}"
    );
}

#[test]
fn the_transcript_label_is_the_same_dim_line_with_no_background() {
    // A thought looks like the same thing wherever you meet it: the settled
    // line wears exactly what the inline one wears, minus the expand hint.
    let lines = reasoning_full_lines(&thought("hm", 3, 12), 40);
    let [span] = lines[0].spans.as_slice() else {
        panic!("one label span, got {:?}", lines[0].spans);
    };
    assert_eq!(span.content, "Thought for 3s · 12 tokens");
    assert_eq!(span.style.fg, Some(reasoning_label_color()));
    assert!(!span.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(span.style.bg, None, "no band, no padding to the width");
}

#[test]
fn the_live_block_heads_with_a_bulleted_thinking_label_over_the_gutter() {
    // The tool cell's shape while it runs: `● Thinking…` over a `⎿` body.
    let lines = live_reasoning_lines("weighing options", Duration::ZERO, 60);
    assert_eq!(plain(&lines[0]), "● Thinking…");
    assert_eq!(plain(&lines[1]), "  ⎿  weighing options");
}

/// Twenty one-row lines of thought, `line 1` … `line 20`.
fn twenty_lines() -> String {
    (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_live_block_tails_the_newest_three_rows() {
    // A long think must not grow the live region without bound: the block
    // shows the last three rows — what just streamed — the way a running
    // `bash` cell tails its output.
    let lines = live_reasoning_lines(&twenty_lines(), Duration::ZERO, 60);
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(
        body,
        ["  ⎿  line 18", "     line 19", "     line 20"],
        "the corner opens the tail and later rows indent under it"
    );
}

#[test]
fn a_wrapping_think_shows_its_newest_three_display_rows() {
    // The reported screen, at its own 87 columns: three lines of thought that
    // wrap to five rows filled the whole window, so the block stood six rows
    // tall over the box. The window counts display rows, not source lines,
    // so it keeps the newest three wherever they wrap from — here both rows
    // of the line still streaming and the one line above them.
    let text = "- I should strictly use tools before answering and verify results \
                never claim unverified success\n\
                - I should keep answers concise and formatted\n\
                The user just said \"Hi\" - this is not a request for any specific work, \
                no tasks, no files to read, no commands to";
    let lines = live_reasoning_lines(text, Duration::ZERO, 87);
    let body: Vec<String> = lines[1..]
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect();
    assert_eq!(
        body,
        [
            "  ⎿  - I should keep answers concise and formatted",
            "     The user just said \"Hi\" - this is not a request for any specific work, no tasks,",
            "     no files to read, no commands to",
        ]
    );
}

#[test]
fn a_long_think_stands_four_rows_tall_in_the_strip() {
    // What the live region spends on it: the header and three rows of
    // thought, then the strip's gap — never a fourth row of the thought.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(&twenty_lines());
    let width = 60;
    let pv = preview_rows(&app, width);
    assert_eq!(pv, 4, "the header plus three rows of thought");
    let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0, 0);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, width).starts_with("● Thinking…"));
    assert!(row(&buf, 1, width).starts_with("  ⎿  line 18"));
    assert!(row(&buf, 3, width).starts_with("     line 20"));
    assert!(
        row(&buf, 4, width).trim().is_empty(),
        "the strip's gap follows the third row: {:?}",
        row(&buf, 4, width)
    );
}

#[test]
fn the_live_block_wraps_long_lines_instead_of_clipping() {
    // Three rows at this width, the whole window, so every word stays in view
    // and what this checks is the wrap, not the tail.
    let lines = live_reasoning_lines("alpha beta gamma", Duration::ZERO, 14);
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert!(body.len() > 1, "a long line wraps: {body:?}");
    for row in &body {
        assert!(cols(row) <= 14, "no row overflows the width: {row:?}");
    }
    let joined = body.join("").replace("  ", " ");
    assert!(joined.contains("alpha"), "{body:?}");
    assert!(joined.contains("gamma"), "no text is lost: {body:?}");
}

#[test]
fn the_live_block_skips_blank_rows_in_its_window() {
    // Reasoning is full of paragraph breaks; spending the small window on
    // them would show a third as much thought.
    let lines = live_reasoning_lines("one\n\n\ntwo\n\nthree", Duration::ZERO, 60);
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body, ["  ⎿  one", "     two", "     three"]);
}

#[test]
fn an_opening_phase_with_no_text_yet_is_just_the_header() {
    // ThinkingStart arrives before the first delta — the block says the model
    // is thinking rather than reserving an empty row.
    let lines = live_reasoning_lines("", Duration::ZERO, 60);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), "● Thinking…");
}

#[test]
fn the_live_label_shimmers_at_the_frame_pulse() {
    // The status line's own white sweep, on `Thinking…` — one span per char,
    // each a different point of the wave, and the wave moves between frames.
    let at = |pulse| {
        live_reasoning_lines("x", pulse, 60)[0]
            .spans
            .iter()
            .skip(1) // the bullet
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect::<Vec<_>>()
    };
    let early = at(Duration::ZERO);
    assert_eq!(
        early.len(),
        "Thinking…".chars().count(),
        "one shimmer span per char"
    );
    assert_eq!(
        early.iter().map(|(c, _)| c.as_str()).collect::<String>(),
        "Thinking…"
    );
    assert_ne!(
        early,
        at(SHIMMER_SWEEP / 3),
        "the sweep advances with the frame clock"
    );
}

#[test]
fn the_live_label_rests_at_bold_white_not_codexs_grey() {
    // Between crests — most of the sweep — the header must still read as a
    // header. Codex's grey base is right for the status metric below it and
    // wrong here: at rest it would be indistinguishable from the dim body.
    let resting: Vec<Color> = (0..40)
        .map(|i| {
            live_reasoning_lines("x", SHIMMER_SWEEP * i / 40, 60)[0].spans[1]
                .style
                .fg
        })
        .map(|fg| fg.expect("the label is coloured"))
        .collect();
    assert!(
        resting.contains(&reasoning_shimmer_base()),
        "the wave rests at the near-white floor: {resting:?}"
    );
    assert!(
        !resting.contains(&shimmer_base()),
        "and never at codex's grey: {resting:?}"
    );
    let brightest = resting
        .iter()
        .map(|fg| {
            let Color::Rgb(r, _, _) = fg else {
                panic!("expected an RGB colour, got {fg:?}")
            };
            assert!(
                *r >= rgb_of(reasoning_shimmer_base()).0,
                "never dimmer than the floor"
            );
            *r
        })
        .max()
        .expect("sampled the sweep");
    // The crest blends SHIMMER_MAX_BLEND (0.9) of the way from the floor to
    // the text colour — whatever the theme's text colour is.
    let floor = rgb_of(reasoning_shimmer_base()).0;
    let crest = rgb_of(shimmer_highlight()).0;
    let reach = floor + ((crest - floor) as f32 * 0.85) as u8;
    assert!(
        brightest >= reach,
        "and the crest does reach the text — a wave nobody sees is not a wave \
         (brightest sampled: {brightest:#04x}, expected at least {reach:#04x})"
    );
}

#[test]
fn the_transcript_header_never_shimmers() {
    // The overlay's cache signature is clock-free, so its header must render
    // at rest — one plain label span, not a per-char wave.
    let lines = reasoning_live_full_lines("x", 60);
    assert_eq!(lines[0].spans.len(), 2, "bullet + one plain label span");
    assert_eq!(lines[0].spans[1].content, "Thinking…");
}

#[test]
fn the_live_bullet_blinks_but_the_committed_one_never_does() {
    // The blink is live-only, like a running tool's (docs/tool-pulse.md): a
    // scrollback commit must never freeze a hidden frame of the animation.
    let shown = live_reasoning_lines("x", Duration::ZERO, 60)[0].spans[0]
        .content
        .to_string();
    let hidden = live_reasoning_lines("x", TOOL_PULSE_PERIOD / 2, 60)[0].spans[0]
        .content
        .to_string();
    assert_eq!(shown, "● ", "the live header shows its bullet");
    assert_eq!(hidden, "  ", "…and hides it half a period on");
    assert_eq!(
        reasoning_lines(&thought("x", 1, 1), 60)[0].spans[0]
            .style
            .fg,
        Some(reasoning_label_color()),
        "the committed line is a fixed colour — no frame of the pulse or the \
         shimmer can be frozen into scrollback"
    );
}

#[test]
fn a_live_phase_previews_in_the_strip() {
    // The strip's preview slot shows the block, and `preview_rows` sizes
    // exactly what `preview_lines` draws (the strip's debug_assert).
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("a\nb");
    let width = 60;
    let pv = preview_rows(&app, width);
    assert_eq!(pv, 3, "header + two rows");
    let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0, 0);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, width).starts_with("● Thinking…"));
    assert!(row(&buf, 1, width).starts_with("  ⎿  a"));
}

#[test]
fn a_running_tool_still_wins_the_preview_slot() {
    // Thinking always precedes the round's calls, so this never really
    // collides — but what is executing is what the user is waiting on.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("thought");
    app.start_tool("bash", "ls", None);
    let lines = preview_lines(&app, 60, None, preview_rows(&app, 60));
    assert!(
        plain(&lines[0]).contains("bash"),
        "the running call previews: {:?}",
        plain(&lines[0])
    );
}

#[test]
fn a_settled_thought_repaints_in_the_conversation_and_the_transcript() {
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking("the plan");
    app.finish_reasoning(7);
    let inline: Vec<String> = conversation_lines(&app.history, 60, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert!(
        inline
            .iter()
            .any(|l| l == "Thought for 7s · 2 tokens (ctrl+o to expand)"),
        "collapsed inline: {inline:?}"
    );
    assert!(
        !inline.iter().any(|l| l.contains("the plan")),
        "the chain-of-thought never commits to scrollback: {inline:?}"
    );
    let full: Vec<String> = transcript_lines(&app, 60).iter().map(plain).collect();
    assert!(
        full.iter().any(|l| l.contains("the plan")),
        "and expands in Ctrl+O: {full:?}"
    );
}

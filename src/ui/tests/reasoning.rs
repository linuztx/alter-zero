//! The thinking stream's three renderers: the live block in the strip, the
//! collapsed `Thought for …` line, and its Ctrl+O expansion.
//! See `docs/thinking-stream.md`.

use super::*;
use crate::app::Reasoning;
use crate::ui::live::preview_lines;
use crate::ui::reasoning::{live_reasoning_lines, reasoning_full_lines, reasoning_live_full_lines};
use crate::ui::theme::{
    REASONING_LABEL_COLOR, REASONING_PEEK_LINES, REASONING_SHIMMER_BASE, SHIMMER_BASE,
    SHIMMER_SWEEP, TOOL_PULSE_PERIOD,
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
    assert_eq!(span.style.fg, Some(REASONING_LABEL_COLOR));
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
fn the_transcript_label_is_the_same_dim_line_with_no_background() {
    // A thought looks like the same thing wherever you meet it: the settled
    // line wears exactly what the inline one wears, minus the expand hint.
    let lines = reasoning_full_lines(&thought("hm", 3, 12), 40);
    let [span] = lines[0].spans.as_slice() else {
        panic!("one label span, got {:?}", lines[0].spans);
    };
    assert_eq!(span.content, "Thought for 3s · 12 tokens");
    assert_eq!(span.style.fg, Some(REASONING_LABEL_COLOR));
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

#[test]
fn the_live_block_tails_the_newest_rows() {
    // A long think must not grow the live region without bound: the block
    // shows the last REASONING_PEEK_LINES rows — what just streamed — the way
    // a running `bash` cell tails its output.
    let text = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = live_reasoning_lines(&text, Duration::ZERO, 60);
    assert_eq!(lines.len(), 1 + REASONING_PEEK_LINES);
    assert_eq!(
        plain(&lines[1]),
        "  ⎿  line 16",
        "the corner opens the tail"
    );
    assert_eq!(
        plain(lines.last().unwrap()),
        "     line 20",
        "later rows indent under the corner"
    );
}

#[test]
fn the_live_block_wraps_long_lines_instead_of_clipping() {
    let lines = live_reasoning_lines("alpha beta gamma delta", Duration::ZERO, 14);
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert!(body.len() > 1, "a long line wraps: {body:?}");
    for row in &body {
        assert!(cols(row) <= 14, "no row overflows the width: {row:?}");
    }
    let joined = body.join("").replace("  ", " ");
    assert!(joined.contains("alpha"), "{body:?}");
    assert!(joined.contains("delta"), "no text is lost: {body:?}");
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
    let rgb = |(r, g, b)| Color::Rgb(r, g, b);
    let resting: Vec<Color> = (0..40)
        .map(|i| {
            live_reasoning_lines("x", SHIMMER_SWEEP * i / 40, 60)[0].spans[1]
                .style
                .fg
        })
        .map(|fg| fg.expect("the label is coloured"))
        .collect();
    assert!(
        resting.contains(&rgb(REASONING_SHIMMER_BASE)),
        "the wave rests at the near-white floor: {resting:?}"
    );
    assert!(
        !resting.contains(&rgb(SHIMMER_BASE)),
        "and never at codex's grey: {resting:?}"
    );
    let brightest = resting
        .iter()
        .map(|fg| {
            let Color::Rgb(r, _, _) = fg else {
                panic!("expected an RGB colour, got {fg:?}")
            };
            assert!(
                *r >= REASONING_SHIMMER_BASE.0,
                "never dimmer than the floor"
            );
            *r
        })
        .max()
        .expect("sampled the sweep");
    assert!(
        brightest >= 0xF0,
        "and the crest does reach the text — a wave nobody sees is not a wave \
         (brightest sampled: {brightest:#04x})"
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
fn the_live_bullet_breathes_but_the_committed_one_never_does() {
    // The pulse is live-only, like a running tool's (docs/tool-pulse.md): a
    // scrollback commit must never freeze a frame of the animation.
    let dim = live_reasoning_lines("x", Duration::ZERO, 60)[0].spans[0]
        .style
        .fg;
    let bright = live_reasoning_lines("x", TOOL_PULSE_PERIOD / 2, 60)[0].spans[0]
        .style
        .fg;
    assert_ne!(dim, bright, "the live header pulses");
    assert_eq!(
        reasoning_lines(&thought("x", 1, 1), 60)[0].spans[0]
            .style
            .fg,
        Some(REASONING_LABEL_COLOR),
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
    let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0);
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
    app.start_tool("bash", "ls");
    let lines = preview_lines(&app, 60, None);
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
    let inline: Vec<String> = conversation_lines(&app.history, 60)
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

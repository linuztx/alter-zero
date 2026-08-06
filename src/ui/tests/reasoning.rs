//! The thinking stream's three renderers: the live block in the strip, the
//! collapsed `Thought for …` line, and its Ctrl+O expansion.
//! See `docs/thinking-stream.md`.

use super::*;
use crate::app::Reasoning;
use crate::ui::live::preview_lines;
use crate::ui::reasoning::{live_reasoning_lines, reasoning_full_lines};
use crate::ui::theme::{REASONING_PEEK_LINES, TOOL_PULSE_PERIOD};
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
    assert_eq!(lines[0].spans.len(), 1, "one styled span, no bullet span");
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
        reasoning_lines(&thought("x", 1, 1), 60)[0].spans.len(),
        1,
        "the committed line has no bullet to freeze a frame of the pulse into"
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

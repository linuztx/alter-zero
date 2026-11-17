//! The thinking stream's three renderers: the live block in the strip, the
//! collapsed `Thought for …` line, and its Ctrl+O expansion.
//! See `docs/thinking-stream.md`.

use super::*;
use crate::app::{LiveThought, Reasoning};
use crate::ui::live::preview_lines;
use crate::ui::reasoning::{live_reasoning_lines, reasoning_full_lines, reasoning_live_full_lines};
use crate::ui::theme::{
    SHIMMER_SWEEP, TOOL_PULSE_PERIOD, reasoning_label_color, reasoning_shimmer_base,
    reasoning_text_color, shimmer_highlight,
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
    let lines = live_reasoning_lines(&"weighing options".into(), Duration::ZERO, 60);
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
    let lines = live_reasoning_lines(&twenty_lines().as_str().into(), Duration::ZERO, 60);
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
    let lines = live_reasoning_lines(&text.into(), Duration::ZERO, 87);
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

/// One long paragraph of thought — no line break in it, the way many
/// models think — of about `words` words.
fn paragraph(words: usize) -> String {
    (0..words)
        .map(|i| format!("step{i} weighs another option"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The strip's thinking block for `app` at `width`, as plain rows, and how
/// many bytes the wrapper walked to build it — sized, then drawn, as the
/// frame does.
fn block_and_scan(app: &App, width: u16) -> (Vec<String>, u64) {
    let before = crate::ui::wrap::scanned_bytes();
    let rows = preview_rows(app, width);
    let lines = preview_lines(app, width, None, rows)
        .iter()
        .map(plain)
        .collect();
    (lines, crate::ui::wrap::scanned_bytes() - before)
}

#[test]
fn the_live_block_wraps_only_what_arrived_since_the_last_frame() {
    // The block re-wrapped the newest paragraph whole every time it was
    // built — several times a frame — to show its last three rows, so a
    // model thinking in one long paragraph made every frame cost all of it:
    // at a thousand tokens a second a typed key's echo reached 40 ms after
    // twenty seconds of thought. Rows a later delta cannot change are kept;
    // a frame wraps what arrived since, and the row it may still rewrite.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(&paragraph(4_000));
    let (_, first) = block_and_scan(&app, 80);
    assert!(
        first >= 100_000,
        "the first frame wraps the paragraph: {first}"
    );
    for step in 0..10 {
        app.push_thinking(&format!(" then step{step} adds a few more words"));
        let (block, scanned) = block_and_scan(&app, 80);
        assert!(
            scanned <= 2_000,
            "frame {step} walked {scanned} bytes to show {block:?}"
        );
        let shown: String = block[1..]
            .iter()
            .map(|row| row.trim_start().trim_start_matches('⎿').trim_start())
            .collect();
        assert!(
            shown.ends_with(&format!("step{step} adds a few more words")),
            "the newest words close the block: {block:?}"
        );
    }
}

#[test]
fn the_live_block_wraps_long_lines_instead_of_clipping() {
    // Three rows at this width, the whole window, so every word stays in view
    // and what this checks is the wrap, not the tail.
    let lines = live_reasoning_lines(&"alpha beta gamma".into(), Duration::ZERO, 14);
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
    let lines = live_reasoning_lines(&"one\n\n\ntwo\n\nthree".into(), Duration::ZERO, 60);
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body, ["  ⎿  one", "     two", "     three"]);
}

#[test]
fn an_opening_phase_with_no_text_yet_is_just_the_header() {
    // ThinkingStart arrives before the first delta — the block says the model
    // is thinking rather than reserving an empty row.
    let lines = live_reasoning_lines(&"".into(), Duration::ZERO, 60);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), "● Thinking…");
}

#[test]
fn the_live_label_shimmers_at_the_frame_pulse() {
    // The status line's own white sweep, on `Thinking…` — one span per char,
    // each a different point of the wave, and the wave moves between frames.
    let at = |pulse| {
        live_reasoning_lines(&"x".into(), pulse, 60)[0]
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
            live_reasoning_lines(&"x".into(), SHIMMER_SWEEP * i / 40, 60)[0].spans[1]
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
        !resting.contains(&reasoning_text_color()),
        "and never at the dim the thought under it wears: {resting:?}"
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
    let shown = live_reasoning_lines(&"x".into(), Duration::ZERO, 60)[0].spans[0]
        .content
        .to_string();
    let hidden = live_reasoning_lines(&"x".into(), TOOL_PULSE_PERIOD / 2, 60)[0].spans[0]
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
    let inline: Vec<String> = conversation_lines(&app.history, 60, &PathDisplay::VERBATIM, true)
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

/// The block's thought rows as they were built before the tail was kept:
/// every line that is not blank wrapped whole, the last three rows — the
/// reference [`LiveThought::tail_rows`] must agree with row for row.
fn whole_wrap_tail(text: &str, width: u16) -> Vec<String> {
    let inner = width
        .saturating_sub(u16::try_from(cols(crate::ui::theme::TOOL_RESULT_PREFIX)).unwrap_or(0))
        .max(1);
    let rows: Vec<String> = text
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .flat_map(|line| crate::ui::wrap::wrap_output(line, inner))
        .collect();
    rows[rows
        .len()
        .saturating_sub(crate::ui::theme::REASONING_PEEK_LINES)..]
        .to_vec()
}

/// The thought rows of a built block, the gutter taken off.
fn block_rows(lines: &[Line<'static>]) -> Vec<String> {
    lines[1..]
        .iter()
        .map(|line| {
            line.spans[1..]
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn the_live_block_stays_inside_the_frame_budget_on_a_long_paragraph() {
    // The wall-clock half of the scan count above: a paragraph of ~1 MB —
    // a long think with no line break — and a frame that adds a few words
    // must cost those words, not the megabyte before them.
    let mut app = App::new();
    app.begin_stream();
    app.begin_reasoning();
    app.push_thinking(&paragraph(35_000));
    let _ = block_and_scan(&app, 80); // the first wrap
    let frames = 10u32;
    let mut total = Duration::ZERO;
    for step in 0..frames {
        app.push_thinking(&format!(" and step{step} weighs one more"));
        let start = std::time::Instant::now();
        let (block, _) = block_and_scan(&app, 80);
        total += start.elapsed();
        assert_eq!(block.len(), 4);
    }
    let per_build = total / frames;
    println!("the thinking block over a ~1 MB paragraph: {per_build:?} per build");
    assert!(
        per_build < Duration::from_millis(10),
        "one build of the thinking block took {per_build:?} — it is wrapping \
         the whole paragraph again"
    );
}

#[test]
fn the_kept_tail_agrees_with_wrapping_the_whole_thought_at_every_delta() {
    // The rows kept between frames must never change what the block shows:
    // hostile text — words wider than a row, runs of spaces at a row's end,
    // wide glyphs, a joined emoji, combining marks, tabs, blank and
    // all-space lines, a carriage return — streamed in pieces of every
    // size, at widths from one column up, with a resize between frames.
    let corpus = [
        "plain words that wrap across a few rows of the narrow block and keep going on",
        "a supercalifragilisticexpialidociousandthensomemorelettersuntilitisverylong word",
        "spaces    at     a    row's        end and   then     more     spaces",
        "日本語の文字列は二列ずつ幅を取るので折り返しが変わる、そして続く文章",
        "emoji 👩‍👩‍👧‍👦 family and e\u{301} combining and ✅ marks mid sentence ok",
        "tab\tseparated\tcolumns\tin\ta\tthought\tthat goes on",
        "line one\n\n   \nline after blanks\nanother line\n\nlast line still going",
        "carriage\rreturn inside and a https://example.com/a/very/long/path/that/never/ends/at/all",
        "ends in spaces   ",
        "x",
    ];
    let mut rng = Rng(0x5EED_7A11_0FBE_E555);
    for (case, doc) in corpus.iter().enumerate() {
        for &widths in &[
            [6u16, 6],
            [7, 7],
            [12, 12],
            [18, 18],
            [45, 45],
            [85, 85],
            [18, 45],
        ] {
            let mut thought = LiveThought::default();
            let mut at = 0usize;
            let mut frame = 0usize;
            while at < doc.len() {
                let mut end = (at + 1 + rng.below(12)).min(doc.len());
                while !doc.is_char_boundary(end) {
                    end += 1;
                }
                thought.push(&doc[at..end]);
                at = end;
                let width = widths[frame % 2];
                frame += 1;
                let warm = block_rows(&live_reasoning_lines(&thought, Duration::ZERO, width));
                assert_eq!(
                    warm,
                    whole_wrap_tail(&doc[..at], width),
                    "case {case}, {at} bytes, width {width}"
                );
            }
        }
    }
}

#[test]
fn the_kept_tail_agrees_with_wrapping_the_whole_thought_on_random_text() {
    // The same agreement over random documents long enough to keep many
    // rows: words of one to thirty columns, wide glyphs among them, runs of
    // spaces, and line breaks, streamed in random pieces.
    let pieces = [
        "a", "bb", "ccc", "日本", "語", "✅", "e\u{301}", "-", "/", "x",
    ];
    let mut rng = Rng(0x0DDB_A11C_0FFE_E000);
    for case in 0..40 {
        let mut doc = String::new();
        while doc.len() < 400 {
            match rng.below(12) {
                0 => doc.push('\n'),
                1 => doc.push_str(&" ".repeat(1 + rng.below(6))),
                _ => {
                    for _ in 0..1 + rng.below(10) {
                        doc.push_str(pieces[rng.below(pieces.len())]);
                    }
                    doc.push(' ');
                }
            }
        }
        for width in [6u16, 7, 10, 18, 30] {
            let mut thought = LiveThought::default();
            let mut at = 0usize;
            while at < doc.len() {
                let mut end = (at + 1 + rng.below(16)).min(doc.len());
                while !doc.is_char_boundary(end) {
                    end += 1;
                }
                thought.push(&doc[at..end]);
                at = end;
                assert_eq!(
                    block_rows(&live_reasoning_lines(&thought, Duration::ZERO, width)),
                    whole_wrap_tail(&doc[..at], width),
                    "case {case}, {at} bytes, width {width}: {doc:?}"
                );
            }
        }
    }
}

/// A tiny xorshift for the streamed pieces' sizes — deterministic, so a
/// failure names a reproducible case.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

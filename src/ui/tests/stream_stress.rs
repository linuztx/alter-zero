//! Stress tests for the **live-streaming render pipeline** — the surface a
//! real model's streamed reply exercises: hostile content (control characters,
//! CRLF line endings, ANSI escapes, zero-width/BiDi codepoints, emoji/CJK,
//! pathological markdown fragments), every chunk boundary, and volume.
//!
//! The differential harness mirrors `stream_render.rs`'s corpus test — the
//! committed rows must extend a stable prefix of the batch render at every
//! character prefix, and the preview must be a suffix of the batch render of
//! that prefix — but over *generated* fragment soup, so combinations no
//! hand-written corpus thought of are still machine-checked. See
//! `docs/markdown.md` and CLAUDE.md invariant 2.

use super::*;

/// xorshift64* — a deterministic, dependency-free PRNG so every failure
/// reproduces from the printed seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fragments a hostile stream could interleave: unclosed markers, fence and
/// table shards, control characters, ANSI colour escapes, zero-width and BiDi
/// codepoints, wide glyphs, and plain prose to glue them together.
const FRAGMENTS: &[&str] = &[
    "plain words here",
    "wrapping prose that runs long enough to cross a narrow width",
    "**bold",
    "**closed bold**",
    "*ital",
    "`tick",
    "`span`",
    "``double `nested` span``",
    "~~strike",
    "[link](http://e/x)",
    "[open link](http://e/",
    "![img](u)",
    "| a | b |",
    "|---|---|",
    "| cell | 🎮 wide |",
    "|:-:|--:|",
    "```rust",
    "```",
    "~~~",
    "fn call(",
    "let s = \"unterminated",
    "# Heading",
    "####",
    "####### seven",
    "- item",
    "1. numbered",
    "12345",
    "> quote",
    "---",
    "***",
    "___",
    "-",
    "--",
    "    indented code",
    "\t tab lead",
    "🎮🚀",
    "❤️ vs16",
    "👨\u{200D}👩\u{200D}👧\u{200D}👦 family",
    "世界你好",
    "ｶ\u{FF9E} dakuten",
    "e\u{301} combining",
    "مرحبا שלום",
    "\u{200B}zero\u{200B}width",
    "\u{202E}rlo\u{202C}",
    "\x1b[31mansi red\x1b[0m",
    "bell\x07 char",
    "nul\0 char",
    "carriage\rreturn",
    "supercalifragilisticexpialidocious_a_very_long_single_word_that_hard_breaks",
    "http://example.com/a/very/long/url/that/wraps/across/rows?with=query&and=more",
    "a | b pipe prose",
    "\\| escaped pipe",
];

/// Joiners between fragments — the newline shapes decide block boundaries.
const JOINERS: &[&str] = &[" ", "\n", "\n\n", "", "\r\n", "\n\n\n"];

/// One generated hostile document.
fn hostile_doc(rng: &mut Rng) -> String {
    let parts = 4 + rng.below(10);
    let mut doc = String::new();
    for i in 0..parts {
        if i > 0 {
            doc.push_str(JOINERS[rng.below(JOINERS.len())]);
        }
        doc.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
    }
    doc
}

fn styled(l: &Line) -> Vec<(String, Option<Color>, Modifier)> {
    l.spans
        .iter()
        .map(|s| (s.content.to_string(), s.style.fg, s.style.add_modifier))
        .collect()
}

/// Drive [`StreamRender`] over **every char prefix** of `full` at `width`,
/// asserting the three streaming invariants against the batch render:
/// committed rows extend a stable prefix of the final render, the preview is
/// a suffix of the batch render of the prefix, and commits + `finish`
/// reconstruct the whole reply. Panics name the seed/doc so failures repro.
fn assert_stream_matches_batch(full: &str, width: u16, ctx: &str) {
    let expected: Vec<Vec<(String, Option<Color>, Modifier)>> =
        message_lines(Role::Assistant, full, width)
            .iter()
            .map(styled)
            .collect();
    let mut render = StreamRender::new();
    let mut committed: Vec<Vec<(String, Option<Color>, Modifier)>> = Vec::new();
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let prefix = &full[..end];
        committed.extend(render.commit(prefix, width).iter().map(styled));
        assert!(
            committed.len() <= expected.len() && committed[..] == expected[..committed.len()],
            "{ctx}: a committed row diverged at {prefix:?} (w={width})\nfull doc: {full:?}\n got {committed:?}\nwant a prefix of {expected:?}"
        );
        let batch_prefix: Vec<Vec<(String, Option<Color>, Modifier)>> =
            message_lines(Role::Assistant, prefix, width)
                .iter()
                .map(styled)
                .collect();
        let got_preview: Vec<Vec<(String, Option<Color>, Modifier)>> = render
            .preview(prefix, width, usize::MAX)
            .iter()
            .map(styled)
            .collect();
        // Not merely a *suffix* of the batch render: the committed rows plus
        // the preview must be the batch render, exactly. A suffix check passes
        // whenever the preview holds the last row — including when rows
        // between scrollback and that row are on screen NOWHERE (the strip
        // showing one row of a source line `commit` withholds whole) or when
        // the preview repeats a row scrollback already has. Equality is what
        // pins the shared frontier (docs/markdown.md, *Scrollback and the
        // strip share one frontier*).
        let mut on_screen = committed.clone();
        on_screen.extend(got_preview.iter().cloned());
        assert_eq!(
            on_screen, batch_prefix,
            "{ctx}: scrollback + strip must be the reply so far at {prefix:?} (w={width}):\n             committed {committed:?}\npreview {got_preview:?}"
        );
    }
    committed.extend(render.finish(full, width).iter().map(styled));
    assert_eq!(
        committed, expected,
        "{ctx}: reconstruct {full:?} (w={width})"
    );
}

#[test]
fn fuzz_stream_render_agrees_with_batch_on_hostile_soup() {
    // 300 generated documents × 4 widths, every char prefix of each: the
    // streamed commits must never diverge from the batch render, whatever
    // fragment soup a hostile stream interleaves. Deterministic (seeded), so
    // a failure names the exact document. (One-off deeper sweeps — 2000 docs
    // × 8 widths at two seeds, fragments up to 27 per doc — passed too; this
    // committed size keeps the suite fast.)
    let mut rng = Rng(0x5EED_CAFE_F00D_0001);
    for case in 0..300 {
        let doc = hostile_doc(&mut rng);
        for width in [2u16, 3, 11, 38] {
            assert_stream_matches_batch(&doc, width, &format!("fuzz case {case}"));
        }
    }
}

#[test]
fn a_streaming_url_never_commits_a_plain_styled_prefix() {
    // The minimal reproduction of the autolink instability the fuzz found: a
    // bare URL streams in char by char. Detection flips on only once enough
    // of the scheme+body has arrived ("ht" is prose, "http://e" is a link),
    // restyling the whole word — so at a narrow width, a wrapped prefix row
    // committed as plain prose would later repaint blue+underlined
    // (immutable-scrollback corruption). The trailing line must be withheld
    // while a URL could still be forming at its end (docs/links.md).
    let full = "see\nhttp://example.com/path more";
    for width in [3u16, 6, 24] {
        assert_stream_matches_batch(full, width, "streaming url");
    }
    // The trailing-punctuation flip: the dot is prose while it ends the line
    // ("…e." trims to "…e") but joins the URL once "…e.com" arrives.
    assert_stream_matches_batch("go to http://e.com/x now", 4, "url dot flip");
}

#[test]
fn a_prose_line_growing_a_pipe_never_becomes_a_table_header() {
    // The fuzz-caught headerless-table flip: a prose line commits rows
    // progressively, then ` | b` streams in and the NEXT line is a table
    // delimiter — under GFM's optional-leading-pipe rule the whole line
    // retroactively becomes a table header rendered as a grid, orphaning the
    // committed prose rows. ANY growing prose line could flip this way, so
    // header candidacy requires the leading `|` (what models emit anyway):
    // the pipe-carrying prose line stays prose in batch and stream alike,
    // and the differential invariants hold at every prefix.
    let doc = "prose grows a pipe | later\n|---|---|\n| 1 | 2 |\nend";
    for width in [2u16, 8, 40] {
        assert_stream_matches_batch(doc, width, "headerless flip");
    }
    // And the line renders as PROSE — never a grid — in the final render.
    let rows: Vec<String> = message_lines(Role::Assistant, doc, 40)
        .iter()
        .map(plain)
        .collect();
    assert!(
        rows.iter()
            .any(|r| r.contains("prose grows a pipe | later")),
        "the pipe-carrying prose line stays literal prose: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains('┌')),
        "no grid is fabricated from a headerless candidate: {rows:?}"
    );
    // A leading-pipe table right after keeps working (the supported form).
    let table = "data:\n| a | b |\n|---|---|\n| 1 | 2 |\ndone";
    for width in [8u16, 40] {
        assert_stream_matches_batch(table, width, "leading-pipe table");
    }
}

#[test]
fn blank_code_rows_before_a_closing_fence_never_over_commit() {
    // The fuzz-caught trailing-blank flip: blank lines stream inside an open
    // fence (content — they used to commit at once), then the CLOSING fence
    // arrives with nothing after it, making them the message's trailing
    // blanks — which the batch render trims. Scrollback then held blank rows
    // a repaint drops. The frontier blanks must stay withheld until
    // non-blank code follows (interior — both keep them) or the reply ends
    // (`finish` keeps them only when the fence is still open, matching
    // `assistant_lines`).
    for doc in [
        "```\nx\n\n\n```",            // blanks turn trailing at the close → trimmed
        "```\nx\n\n\ny\n```",         // interior blanks → kept as code rows
        "intro\n```\ncode\n\n",       // reply ends inside the fence → kept
        "a\n\n```\n世界\n**b\n\n```", // the original fuzz shape
    ] {
        for width in [2u16, 10, 40] {
            assert_stream_matches_batch(doc, width, "fence blanks");
        }
    }
}

#[test]
fn crlf_reply_renders_like_its_lf_twin() {
    // Models echoing Windows files (or providers normalising to CRLF) stream
    // `\r\n` line endings. A CRLF reply must render **identically** to its LF
    // twin — the `\r` is part of the line ending, not content: fences still
    // open/close, `---` is still a rule, and no row text carries a stray CR
    // into scrollback, the Ctrl+O transcript, or a `/copy` cell.
    let lf_docs = [
        "a\n\n---\n\nb",
        "```rust\nlet x = 1;\n```\nafter",
        "| a | b |\n|---|---|\n| 1 | 2 |\ndone",
        "# Title\n- item one\n- item two\n> quoted",
        "intro\n\n    indented code\n\nback",
        "***\nmiddle\n___",
    ];
    for lf in lf_docs {
        let crlf = lf.replace('\n', "\r\n");
        for width in [12u16, 40] {
            let want: Vec<Vec<(String, Option<Color>, Modifier)>> =
                message_lines(Role::Assistant, lf, width)
                    .iter()
                    .map(styled)
                    .collect();
            let got: Vec<Vec<(String, Option<Color>, Modifier)>> =
                message_lines(Role::Assistant, &crlf, width)
                    .iter()
                    .map(styled)
                    .collect();
            assert_eq!(
                got, want,
                "CRLF twin diverged from LF for {lf:?} (w={width})"
            );
        }
    }
}

#[test]
fn crlf_reply_streams_prefix_stable_too() {
    // The differential invariants must hold while a CRLF reply streams —
    // including the moment a line's `\r` has arrived but its `\n` hasn't.
    for lf in [
        "a\n\n---\n\nb",
        "```python\ndef f():\n    return 1\n```\ndone",
        "| a | b |\n|---|---|\n| 1 | 2 |\nend",
    ] {
        let crlf = lf.replace('\n', "\r\n");
        for width in [4u16, 24] {
            assert_stream_matches_batch(&crlf, width, "crlf stream");
        }
    }
}

#[test]
fn hostile_control_chars_never_reach_painted_cells() {
    // The committed rows are rasterised exactly as `term::write_above` does —
    // `Paragraph::render` into a `Buffer` — and no cell may carry a C0
    // control, an ESC, or a BiDi override: a raw ESC/CR reaching the terminal
    // would corrupt the screen (colour bleed, cursor jumps), and an RLO can
    // visually spoof a row. ratatui's zero-width filter is what guarantees
    // this today; the test pins the property against any future custom paint.
    let hostile = "prose \x1b[31mred\x1b[0m mid\rcr bell\x07 nul\0 \u{202E}rlo\u{202C}\n\
                   ```sh\necho \x1b[1mbold\x1b[0m\r\ndone\r\n```\n\
                   tail \u{200B}zero\u{200B}";
    for width in [10u16, 40] {
        let lines = message_lines(Role::Assistant, hostile, width);
        let height = lines.len() as u16;
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        for cell in &buffer.content {
            let symbol = cell.symbol();
            assert!(
                !symbol.chars().any(|c| {
                    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
                }),
                "a control/BiDi char reached a painted cell: {symbol:?}"
            );
        }
    }
}

/// Build a large fenced-code reply: `lines` lines of plausible Rust-ish code.
fn big_code_doc(lines: usize) -> String {
    let mut doc = String::from("Here is the generated module:\n```rust\n");
    for i in 0..lines {
        doc.push_str(&format!(
            "fn generated_function_{i}(arg: usize) -> usize {{ arg + {i} }} // line {i}\n"
        ));
    }
    doc.push_str("```\nAll done.\n");
    doc
}

#[test]
fn huge_code_reply_streams_in_linear_time() {
    // A ~250 KB syntax-highlighted reply streamed in small chunks must stay
    // O(reply): the per-chunk commit may only touch the newly-arrived tail.
    // The generous ceiling is not a benchmark — it catches an accidental
    // return to the O(reply²) re-render (which takes minutes at this size).
    let doc = big_code_doc(3500);
    assert!(doc.len() > 250_000, "the fixture is large: {}", doc.len());
    let width = 80u16;
    let start = std::time::Instant::now();
    let mut render = StreamRender::new();
    let mut committed = 0usize;
    let mut end = 0usize;
    while end < doc.len() {
        end = (end + 96).min(doc.len());
        while !doc.is_char_boundary(end) {
            end += 1;
        }
        committed += render.commit(&doc[..end], width).len();
    }
    committed += render.finish(&doc, width).len();
    let elapsed = start.elapsed();
    println!(
        "streamed {} bytes → {committed} rows in {elapsed:?}",
        doc.len()
    );
    assert_eq!(
        committed,
        message_lines(Role::Assistant, &doc, width).len(),
        "the streamed rows reconstruct the reply"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "streaming a 250 KB code reply took {elapsed:?} — the incremental \
         renderer has gone quadratic"
    );
}

#[test]
fn huge_single_line_reply_stays_responsive() {
    // The pathological stream: one enormous source line (minified JSON, a
    // base64 dump) with **no newline** — nothing about it is "complete", so
    // every commit re-examines the whole trailing line. Per-chunk work must
    // stay bounded enough that the event loop keeps animating: the whole
    // ~130 KB / 4000-chunk stream must finish far under the ceiling, and no
    // single late commit may blow a frame budget wide open. This is the
    // guard against O(line) work per chunk turning a long single-line reply
    // into a frozen UI (pre-fix: ~39 s total, post-fix: ~0.2 s).
    // URLs included so the forming-URL withhold scan is part of the measured
    // per-chunk cost (it runs on every evaluated commit of the trailing line).
    let word = "{\"u\":\"http://e.com/a\",\"n\":12345}, ";
    let doc: String = word.repeat(3900); // ~130 KB, one line
    let width = 80u16;
    let mut render = StreamRender::new();
    let start = std::time::Instant::now();
    let mut worst = std::time::Duration::ZERO;
    let mut committed = 0usize;
    let mut end = 0usize;
    while end < doc.len() {
        end = (end + 32).min(doc.len());
        while !doc.is_char_boundary(end) {
            end += 1;
        }
        let t = std::time::Instant::now();
        committed += render.commit(&doc[..end], width).len();
        worst = worst.max(t.elapsed());
    }
    committed += render.finish(&doc, width).len();
    let elapsed = start.elapsed();
    println!(
        "single-line: {} bytes, {} commits → {committed} rows in {elapsed:?} (worst commit {worst:?})",
        doc.len(),
        doc.len() / 32,
    );
    assert_eq!(
        committed,
        message_lines(Role::Assistant, &doc, width).len(),
        "the streamed rows reconstruct the reply"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "streaming a 130 KB single-line reply took {elapsed:?} — per-chunk \
         work on the trailing line has gone quadratic"
    );
}

#[test]
fn preview_of_a_growing_code_line_is_bounded() {
    // The strip preview runs every animation frame (~30/s). While a huge code
    // line streams (in-code tails are withheld from commit but still fed to
    // the preview's clone), each preview costs a syntax-highlight of the
    // trailing line — it must stay comfortably inside a frame budget even for
    // an absurd 64 KB line.
    let mut doc = String::from("```json\n");
    doc.push_str(&"{\"k\":123,\"deep\":[1,2,3]},".repeat(2500)); // ~62 KB line
    let width = 80u16;
    let mut render = StreamRender::new();
    let _ = render.commit(&doc, width);
    let start = std::time::Instant::now();
    let frames = 10;
    for _ in 0..frames {
        let rows = render.preview(&doc, width, 6);
        assert!(!rows.is_empty());
    }
    let per_frame = start.elapsed() / frames;
    println!("preview of a 62 KB code line: {per_frame:?} per frame");
    assert!(
        per_frame < std::time::Duration::from_millis(150),
        "a single preview took {per_frame:?} — the strip redraw would starve \
         the event loop"
    );
}

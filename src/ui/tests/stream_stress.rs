//! Stress tests for the **live-streaming render pipeline** — the surface a
//! real model's streamed reply exercises: hostile content (control characters,
//! CRLF line endings, ANSI escapes, zero-width/BiDi codepoints, emoji/CJK,
//! pathological markdown fragments), every chunk boundary, and volume.
//!
//! The differential harness mirrors `stream_render.rs`'s corpus test — the
//! committed rows must extend a stable prefix of the batch render at every
//! character prefix, and scrollback plus the strip must together show the
//! whole reply (`committed ++ preview == assistant_lines(prefix)`, the
//! one-frontier contract) — but over *generated* fragment soup, so
//! combinations no hand-written corpus thought of are still machine-checked.
//! See `docs/markdown.md` and CLAUDE.md invariant 2.

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
    // --- classes the first corpus never generated ---
    "===",            // setext underline (ambiguous with prose)
    "Title text",     // its setext partner
    "\\*not bold\\*", // escaped emphasis stays literal
    "<http://e.com>", // angle autolink
    "[ref][label]",   // reference-style link (no inline target)
    "> > nested quote",
    ">", // bare quote marker
    "\t\tdeep tab indent",
    "999999999. huge ordinal",
    "~~~js",              // tilde fence with an info string
    "``` `tick` in info", // backtick in a backtick info string: NOT a fence
    "| `a|b` | c |",      // a pipe inside a code span in a cell
    "aaa\u{1F3AE}aaa",    // wide glyph mid-word (wrap-boundary straddle)
    "a\u{301}\u{302}\u{303} stacked marks",
    "   ",            // whitespace-only line
    "\t",             // tab-only line
    "[a [b] c](u)",   // nested brackets in link text
    "[![alt](i)](u)", // image inside a link
    "<div>html block</div>",
    "snake_case_word_here",
    "**bold**.", // emphasis closed against punctuation
    "1) paren ordinal",
    "        - eight space bullet",
    "||||",            // pipes only
    "|---|",           // a bare delimiter row with no header
    "text\u{200D}zwj", // a lone ZWJ
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
/// committed rows extend a stable prefix of the final render, **scrollback
/// and the strip together are the reply so far** (`committed ++ preview ==
/// assistant_lines(prefix)` — the shared-frontier contract, strictly stronger
/// than "the preview is a suffix", which a lone last row satisfies while rows
/// above it are on screen nowhere), and commits + `finish` reconstruct the
/// whole reply. Panics name the seed/doc so failures repro.
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
    // 300 generated documents × 4 widths, every char prefix of each: neither
    // scrollback nor the strip may diverge from the batch render, whatever
    // fragment soup a hostile stream interleaves. Deterministic (seeded), so
    // a failure names the exact document. (One-off deeper sweeps kept passing
    // as the corpus and the assertion grew: 2000 docs × 8 widths at two seeds
    // under the old suffix check, then 1500 × 8 under the whole-reply
    // equality, then 2500 × 9 with the setext/escape/autolink/reference-link/
    // nested-quote/ZWJ/stacked-mark classes added. This committed size keeps
    // the suite fast.)
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

#[test]
fn the_commit_amortizer_never_hides_rows_from_the_screen() {
    // The interaction between the two halves of this work. `commit`
    // amortizes its evaluation once the trailing line is huge (TAIL_EVAL_MIN),
    // so on a machine-dump line it deliberately commits NOTHING for a stretch
    // — and `preview` is what keeps those rows on screen, because it returns
    // the whole uncommitted tail rather than a row of its own. Under the old
    // single-row preview the amortizer would have blanked everything but the
    // last row of a growing 4 KB+ line; under the shared frontier the strip
    // shows exactly what scrollback is missing, so the reply stays whole.
    //
    // Checked with a REAL strip cap (not usize::MAX): past the cap the strip
    // tail-follows, so the contract is "scrollback + strip lose nothing the
    // cap did not deliberately scroll off", i.e. visible == batch whenever
    // the uncommitted tail fits, and the preview is always the batch render's
    // own tail otherwise.
    let width = 60u16;
    let cap = 12usize;
    let mut doc = String::from("intro line\n");
    // One prose line far past TAIL_EVAL_MIN (4 KB), streamed in chunks.
    let sentence = "the quick brown fox jumps over the lazy dog again and again ";
    doc.push_str(&sentence.repeat(120)); // ~7 KB, no newline
    let mut render = StreamRender::new();
    let mut committed: Vec<String> = Vec::new();
    let mut saw_amortized_gap = false;
    let mut end = 0usize;
    while end < doc.len() {
        end = (end + 64).min(doc.len());
        while !doc.is_char_boundary(end) {
            end += 1;
        }
        let prefix = &doc[..end];
        let new_rows = render.commit(prefix, width);
        let committed_before = committed.len();
        committed.extend(new_rows.iter().map(plain));
        let preview: Vec<String> = render
            .preview(prefix, width, cap)
            .iter()
            .map(plain)
            .collect();
        let batch: Vec<String> = message_lines(Role::Assistant, prefix, width)
            .iter()
            .map(plain)
            .collect();
        // The amortizer is really engaging (a chunk that committed nothing
        // while the render had grown past the committed frontier).
        if committed_before == committed.len() && committed.len() < batch.len() {
            saw_amortized_gap = true;
        }
        // The preview is always the batch render's own tail — nothing shown
        // that scrollback already has, nothing invented.
        assert!(
            preview.len() <= batch.len() && preview[..] == batch[batch.len() - preview.len()..],
            "the preview must be the batch render's tail at {} bytes: got {preview:?}",
            prefix.len()
        );
        // Nothing is lost between them: scrollback + strip cover the whole
        // render, except rows the strip's own cap deliberately scrolled off.
        let visible = committed.len() + preview.len();
        assert!(
            visible == batch.len() || preview.len() == cap,
            "rows vanished at {} bytes: {} committed + {} previewed vs {} rendered",
            prefix.len(),
            committed.len(),
            preview.len(),
            batch.len()
        );
    }
    assert!(
        saw_amortized_gap,
        "the fixture must actually exercise the amortizer's commit gap"
    );
    committed.extend(render.finish(&doc, width).iter().map(plain));
    assert_eq!(
        committed,
        message_lines(Role::Assistant, &doc, width)
            .iter()
            .map(plain)
            .collect::<Vec<_>>(),
        "the reply still reconstructs exactly"
    );
}

#[test]
fn a_purge_rebuild_plus_the_strip_still_shows_the_whole_reply() {
    // The resize path, which the on-screen frontier tests do not cover.
    // A purge rebuild (`/clear`, every resize, a history rewind) drops the
    // terminal's scrollback and re-renders it from `committed_rows` — the
    // rows that HAD been committed — while the strip repaints `preview`
    // beside it. So the rebuilt screen is `committed_rows ++ preview`, and it
    // must be the batch render just as the live screen is: a mismatch here
    // means a resize mid-reply silently loses or doubles rows, which the
    // live-streaming assertions cannot see (they never call `committed_rows`).
    //
    // It bites: grafted onto the pre-frontier renderer (b98de61) it fails at
    // case 0, the rebuilt screen short two blank rows the batch render keeps.
    let mut rng = Rng(0xABCD_0000_1234_9999);
    for case in 0..60 {
        let doc = hostile_doc(&mut rng);
        for width in [3u16, 9, 30] {
            let mut render = StreamRender::new();
            for end in 1..=doc.len() {
                if !doc.is_char_boundary(end) {
                    continue;
                }
                let prefix = &doc[..end];
                let _ = render.commit(prefix, width);
                let _ = render.preview(prefix, width, usize::MAX);
                // The rebuild, taken at this instant.
                let mut screen: Vec<String> = render
                    .committed_rows(prefix, width)
                    .iter()
                    .map(plain)
                    .collect();
                screen.extend(render.preview(prefix, width, usize::MAX).iter().map(plain));
                let batch: Vec<String> = message_lines(Role::Assistant, prefix, width)
                    .iter()
                    .map(plain)
                    .collect();
                assert_eq!(
                    screen, batch,
                    "case {case}: a rebuild at {prefix:?} (w={width}) does not \
                     reconstruct the reply"
                );
            }
        }
    }
}

#[test]
fn a_mid_stream_width_change_reconstructs_the_reply() {
    // A real resize while the reply streams: the cache is width-keyed, so the
    // first call at the new width rebuilds from scratch — `committed` resets
    // and `committed_rows` is empty, which is exactly why the boundary
    // re-commits the whole partial after a purge rebuild (`docs/flicker.md`).
    // What must hold either way: once the stream continues and finishes at
    // the new width, the rows that reached scrollback are the batch render at
    // that width — no fragment rendered at the old width survives.
    let doc = "intro paragraph that wraps\n\
               ```rust\n\
               fn wide_function_name(a: u32) -> u32 { a }\n\
               ```\n\
               | a | b |\n\
               |---|---|\n\
               | 1 | 2 |\n\
               tail line here";
    for (before, after) in [(60u16, 24u16), (24, 60), (80, 7)] {
        let split = doc.len() / 2;
        let mut split = split;
        while !doc.is_char_boundary(split) {
            split += 1;
        }
        let mut render = StreamRender::new();
        // Stream the first half at the old width.
        let mut end = 0usize;
        while end < split {
            end = (end + 7).min(split);
            while !doc.is_char_boundary(end) {
                end += 1;
            }
            let _ = render.commit(&doc[..end], before);
            let _ = render.preview(&doc[..end], before, 8);
        }
        // The resize lands: the cache rebuilds at the new width, so nothing
        // is considered committed any more and the boundary re-commits all
        // of it. Collect from here as the rebuild does.
        let mut committed: Vec<String> = render
            .committed_rows(&doc[..end], after)
            .iter()
            .map(plain)
            .collect();
        assert!(
            committed.is_empty(),
            "a width change invalidates the committed frontier ({before}→{after})"
        );
        // Stream the rest at the new width.
        while end < doc.len() {
            end = (end + 7).min(doc.len());
            while !doc.is_char_boundary(end) {
                end += 1;
            }
            committed.extend(render.commit(&doc[..end], after).iter().map(plain));
        }
        committed.extend(render.finish(doc, after).iter().map(plain));
        let batch: Vec<String> = message_lines(Role::Assistant, doc, after)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(
            committed, batch,
            "the reply must reconstruct at the new width ({before}→{after})"
        );
        // The fixture must really exercise both withheld constructs at the
        // new width — a fence and a *table* (a stray indent once turned the
        // table into indented code and the case silently stopped biting).
        assert!(
            batch.iter().any(|r| r.contains('\u{2502}')),
            "the fixture renders a table grid at width {after}: {batch:?}"
        );
    }
}

#[test]
fn a_degenerate_terminal_width_is_survivable() {
    // A tiling WM mid-animation can report a 0-column screen, and `term`
    // already guards a 0-row one. The render path must not panic there (a
    // panic in a draw kills the TUI and strands the terminal), and what it
    // does commit must still agree with the batch render — so the resize back
    // to a real width rebuilds from something coherent.
    let mut rng = Rng(0x1111_2222_3333_4444);
    for case in 0..40 {
        let doc = hostile_doc(&mut rng);
        for width in [0u16, 1] {
            let mut render = StreamRender::new();
            let mut committed: Vec<String> = Vec::new();
            for end in 1..=doc.len() {
                if !doc.is_char_boundary(end) {
                    continue;
                }
                let prefix = &doc[..end];
                committed.extend(render.commit(prefix, width).iter().map(plain));
                let _ = render.preview(prefix, width, 4);
            }
            committed.extend(render.finish(&doc, width).iter().map(plain));
            assert_eq!(
                committed,
                message_lines(Role::Assistant, &doc, width)
                    .iter()
                    .map(plain)
                    .collect::<Vec<_>>(),
                "case {case}: width {width} must still reconstruct the reply"
            );
        }
    }
}

#[test]
fn the_strip_redraw_stays_inside_a_frame_on_a_huge_prose_line() {
    // The other half of the huge-line story, which
    // `huge_single_line_reply_stays_responsive` does not measure: that test
    // drives only `commit`, whose evaluation is amortized. The draw tick also
    // calls `preview`, and its memo key carries the tail length — so every
    // frame that lands on a new chunk re-renders the whole trailing line.
    //
    // A huge **prose** line is the worst case: the 4 KB syntect cap bounds a
    // code line, but prose goes through `parse_inline`/`wrap_inline` over the
    // whole line, and that is O(line) with no cap. It cannot be cached
    // incrementally in general — an emphasis marker arriving later restyles
    // text already wrapped, which is exactly why `commit` withholds such a
    // line — so this is a **known linear bound**, pinned here rather than
    // engineered away: a 130 KB single-line reply costs a few ms per frame
    // against the ~32 ms animation cadence, and the ceilings below catch a
    // regression that would turn that into a visible freeze.
    let word = "the quick brown fox jumps over the lazy dog and keeps going ";
    let doc: String = word.repeat(2200); // ~130 KB, one prose line
    assert!(doc.len() > 120_000, "the fixture is huge: {}", doc.len());
    let width = 80u16;
    let mut render = StreamRender::new();
    let mut end = 0usize;
    let mut frames = 0u32;
    let mut total = std::time::Duration::ZERO;
    let mut worst = std::time::Duration::ZERO;
    while end < doc.len() {
        end = (end + 512).min(doc.len());
        while !doc.is_char_boundary(end) {
            end += 1;
        }
        let prefix = &doc[..end];
        let _ = render.commit(prefix, width);
        let t = std::time::Instant::now();
        let _ = render.preview(prefix, width, 8);
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
        frames += 1;
    }
    let avg = total / frames.max(1);
    println!("huge prose line: {frames} frames, avg {avg:?}, worst {worst:?}");
    assert!(
        avg < std::time::Duration::from_millis(25),
        "the average strip redraw on a 130 KB prose line is {avg:?} — the \
         per-frame preview has regressed badly"
    );
    assert!(
        worst < std::time::Duration::from_millis(60),
        "a single strip redraw took {worst:?} — the draw tick would visibly \
         stall the status animation"
    );
}

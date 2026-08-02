//! Incremental streaming to scrollback (`docs/markdown.md`, `docs/flicker.md`).

use super::*;

#[test]
fn table_commits_whole_and_previews_while_forming() {
    // The core behavior (docs/table-streaming.md): nothing of an open table
    // reaches scrollback (its widths need every row), while the strip
    // previews the ENTIRE forming grid — so `committed ++ preview` equals
    // the batch render of every prefix (the reply is always fully visible,
    // split between scrollback and the strip), and the whole grid commits
    // at the close sized to all rows.
    let full = "here:\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | a wider cell |\n| 5 | 6 |\ndone";
    let width = 40;
    let batch: Vec<String> = message_lines(Role::Assistant, full, width)
        .iter()
        .map(plain)
        .collect();
    let mut render = StreamRender::new();
    let mut committed: Vec<String> = Vec::new();
    let mut previewed_forming_grid = false;
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let prefix = &full[..end];
        committed.extend(render.commit(prefix, width).iter().map(plain));
        let preview: Vec<String> = render
            .preview(prefix, width, usize::MAX)
            .iter()
            .map(plain)
            .collect();
        // Scrollback never holds a fragment of the open table.
        if !prefix.contains("done") {
            assert!(
                !committed.iter().any(|r| r.contains('│')),
                "no table row commits before the close: {committed:?}"
            );
        }
        // Mid-table the strip shows the whole forming grid — borders,
        // header, and the rows seen so far (the just-arrived one included).
        if prefix.ends_with("| 3 | a wider cell |") {
            assert!(
                preview.iter().any(|r| r.contains('┌'))
                    && preview.iter().any(|r| r.contains("a wider cell")),
                "the forming grid previews whole: {preview:?}"
            );
            previewed_forming_grid = true;
        }
        // The full-visibility property: while the table is open, scrollback
        // plus the strip reconstruct the batch render of this prefix.
        if ends_in_open_table(prefix) {
            let batch_prefix: Vec<String> = message_lines(Role::Assistant, prefix, width)
                .iter()
                .map(plain)
                .collect();
            let mut visible = committed.clone();
            visible.extend(preview);
            assert_eq!(
                visible, batch_prefix,
                "scrollback + strip show the whole render at {prefix:?}"
            );
        }
    }
    committed.extend(render.finish(full, width).iter().map(plain));
    assert_eq!(committed, batch, "the whole grid commits at the close");
    assert!(previewed_forming_grid);
    // And the close sized the columns to the WIDEST row, not the first.
    assert!(
        committed.iter().any(|r| r.contains("│ a wider cell │")),
        "columns fit the widest row: {committed:?}"
    );
}

#[test]
fn stream_render_withholds_a_table_until_it_closes() {
    // A table is not prefix-stable, so nothing is committed while it is open;
    // `finish` flushes the whole block, matching the batch render exactly.
    let width = 40;
    let open = "| a | b |\n|---|---|\n| 1 | 2 |"; // header + delim + a row, unclosed
    let mut render = StreamRender::new();
    assert!(
        render.commit(open, width).is_empty(),
        "an open table commits nothing to scrollback"
    );
    let flushed = render.finish(open, width);
    assert_eq!(
        flushed,
        message_lines(Role::Assistant, open, width),
        "finish flushes the buffered table, matching the batch render"
    );
}

#[test]
fn streamed_code_never_recolours_a_committed_row() {
    // A code line LONGER than the code width wraps into several rows; the
    // highlighter's one-char lookahead (a call's `(`) must not recolour an
    // already-committed row when it finally streams in. Compare the streamed
    // commits' SPAN COLOURS (not just text) to the final render.
    let full = "```py\nsome_really_long_function_name_here()\n```";
    let width = 22; // content_width 20 → the long name wraps
    let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    };
    let expected: Vec<Vec<(String, Option<Color>)>> = message_lines(Role::Assistant, full, width)
        .iter()
        .map(styled)
        .collect();

    // Stream char-by-char so a commit boundary lands mid-identifier.
    let mut render = StreamRender::new();
    let mut got: Vec<Vec<(String, Option<Color>)>> = Vec::new();
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        got.extend(render.commit(&full[..end], width).iter().map(styled));
    }
    got.extend(render.finish(full, width).iter().map(styled));

    assert_eq!(got, expected, "a committed code row must never recolour");
}

#[test]
fn incremental_commits_reconstruct_a_fenced_code_reply() {
    // The critical prefix-stability integration test: stream a reply that
    // contains a fenced code block chunk-by-chunk; the committed lines plus
    // the final flush must exactly equal the fully-rendered message, so
    // scrollback never disagrees with a resize/Ctrl+O repaint.
    let full = "Here is code:\n```python\ndef f():\n    return 1\n\n    x = 2\n```\nDone.";
    let width = 24;
    let expected: Vec<String> = message_lines(Role::Assistant, full, width)
        .iter()
        .map(plain)
        .collect();

    let mut render = StreamRender::new();
    let mut got: Vec<String> = Vec::new();
    let mut acc = String::new();
    for chunk in crate::stream::chunks(full) {
        acc.push_str(&chunk);
        got.extend(render.commit(&acc, width).iter().map(plain));
    }
    got.extend(render.finish(&acc, width).iter().map(plain));

    assert_eq!(got, expected, "streamed commits reconstruct the code reply");
}

// --- streaming commit bookkeeping ---

/// Adversarial differential test: for a corpus of tricky replies at several
/// widths, drive [`StreamRender`] over **every character-prefix** and assert
/// that (a) the streamed commits + `finish` reconstruct the batch
/// [`message_lines`] render exactly (text, colour, **and modifiers** —
/// heading levels differ only by bold/italic, so an fg-only comparison is
/// blind to a level flip), (b) no committed row ever changes, and (c)
/// `preview(prefix)` equals the last row of the batch render of that
/// prefix. This is the guard against immutable-scrollback corruption — the
/// incremental renderer must never diverge from the batch one.
#[test]
fn stream_render_matches_batch_render_on_every_prefix() {
    let styled = |l: &Line| -> Vec<(String, Option<Color>, Modifier)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg, s.style.add_modifier))
            .collect()
    };
    let corpus = [
        // Prose that wraps several times.
        "the quick brown fox jumps over the lazy dog and keeps on running along",
        // Prose, a fenced code block, then prose.
        "Intro line here.\n```python\ndef f(x):\n    return x + 1\n```\nOutro line.",
        // A long code line whose call `(` lands past a wrap boundary — the
        // recolour trap (identifier turns blue only when the `(` arrives).
        "```rust\nfn some_really_long_function_name_that_wraps(argument: i32) -> i32 {\n    argument\n}\n```",
        // Python triple-quoted multi-line string (highlight carry across lines).
        "```python\ndoc = \"\"\"first\nsecond line\nthird\"\"\"\nx = f(1)\n```",
        // C block comment carried across lines.
        "```c\nint a; /* open\nstill comment\nclose */ int b;\n```",
        // Rust lifetimes (a `'a` must not open a string) + a char literal.
        "```rust\nimpl<'a> Foo<'a> {\n    let c = 'x';\n}\n```",
        // Tilde fence, indented fence, blank lines inside code.
        "~~~\nplain code\n\nmore\n~~~",
        "   ```\nindented fence body\n   ```",
        // ATX headings interleaved with prose and a bare code block.
        "# Title\nsome text under it\n## Sub heading here that is quite long and wraps\n```\ncode\n```",
        // Reply that is exactly a code block; unterminated fence at the end.
        "```go\npackage main\nfunc main() {}",
        // TAB-indented code (Go): tabs expand to spaces on the render path, so
        // the streamed commits must still match the batch render at every
        // prefix (the expansion is a pure per-line transform, prefix-stable).
        "```go\nfunc main() {\n\tif x {\n\t\tfmt.Println(\"hi\")\n\t}\n}\n```",
        // Consecutive fences (open immediately closed) and empty prose lines.
        "a\n\n```\n```\n\nb",
        // Multi-byte UTF-8: emoji + CJK in prose and inside a string, so the
        // per-prefix char-boundary handling is exercised.
        "greeting 🎮 hello 世界 more text to wrap around\n```python\nprint(\"🎮 世界!\")\n```\ndone 🚀",
        // Indented (4-space) code block: after a blank it renders verbatim
        // plain; the committed rows must stay stable as it streams in.
        "intro line\n\n    def f(x):\n        return x + 1\nback to prose",
        // A long indented-code line that hard-breaks at narrow widths, plus a
        // blank line inside the block (kept as code), then prose ends it.
        "note\n\n    a_really_long_indented_code_line_that_wraps_several_times = 42\n\n    tail\ndone",
        // Thematic breaks (`---` after a blank, and `***`) render as `———`.
        "one\n\n---\n\ntwo",
        "a\n\n***\nb",
        // Indented code followed by a thematic break and more prose.
        "lead\n\n    code_here()\n\n---\n\ntrailer",
        // A deep heading: while the trailing line is a bare `#` run its
        // LEVEL (→ style) is unsettled — another `#` deepens it, a 7th
        // flips it to prose — so at content-width 1 its wrapped rows must
        // be withheld (`markdown::is_partial_heading`), like a fence's.
        "lead\n###### deep heading level six",
        // The 7-hash flip: `#######` is prose, not a heading.
        "a\n####### not a heading",
        // Trailing paragraph break (a model's `…\n\n` before a tool call):
        // the trailing blank rows must be trimmed at every prefix, and a
        // committed row must never regress when they are.
        "building the thing now.\n\n",
        "first paragraph.\n\nsecond paragraph.\n\n",
        // A blank line *inside* a still-open fence at the end is content, not
        // a trailing blank — it must survive (the `!in_code` trim gate).
        "intro\n```\ncode\n\n",
        // --- GFM tables (docs/markdown.md): a table is buffered whole and
        // committed only when the block closes, so the streamed commits must
        // still match the batch render at every prefix and width (incl. the
        // column shrink at tiny widths). ---
        // A basic table between prose.
        "intro\n\n| Name | Type | Notes |\n|------|------|-------|\n| Alpha | String | Example row |\n| Beta | Number | Another row |\n\nafter",
        // A table the reply ends on (no closing line) — `finish` flushes it.
        "here is data:\n| a | b |\n|:--|--:|\n| 1 | 2 |",
        // Per-column alignment (left/center/right), then prose closes it.
        "| L | C | R |\n| :-- | :-: | --: |\n| x | yy | zzz |\ntail",
        // A pipe-carrying prose line that is NOT a table (no delimiter follows):
        // rendered as plain prose, buffered one line then flushed.
        "use a | b pipe here\nnext line of prose",
        // A candidate header whose delimiter column count mismatches → all prose.
        "| a | b | c |\n|---|---|\nnot a table",
        // A table immediately followed by a code fence (flush on CodeStart).
        "| a | b |\n|---|---|\n| 1 | 2 |\n```\ncode\n```",
        // A single-column table, then prose.
        "| Item |\n|------|\n| one |\n| two |\ndone",
        // A table with long cells that must WRAP into taller rows at the
        // narrow widths (img2): the progressive streamed commits + the final
        // flush must still equal the batch render at every prefix/width, so
        // the row-by-row streaming and the cell wrapping stay prefix-stable.
        "data:\n\n| Name | Email | Role |\n|------|-------|------|\n| John Doe | john.doe@example.com | Developer |\n| Jane Smith | jane@corp.io | Manager |\n\nend",
        // A table whose cells carry inline markdown (`` `code` ``, **bold**):
        // cells are inline-parsed like prose and the column sizes to the
        // rendered width, so the withheld-whole grid's streamed commits must
        // still match batch at every prefix/width (incl. the narrow shrink).
        "files:\n\n| Database | Modified |\n|----------|----------|\n| `core.db` | **Jul 13** |\n| plain.db | today |\n\nend",
        // A table whose cells are long enough that at the narrower sweep widths
        // the grid is too cramped to scan and flips to codex-style key/value
        // RECORDS (docs/table-streaming.md): the block (a `─` rule before each
        // non-first record) is buffered and rendered whole like the grid, so
        // the streamed commits + the final flush must still equal the batch
        // render at every prefix/width.
        "summary:\n\n| Component | Description of the thing |\n|-----------|--------------------------|\n| Parser | Reads and validates the input tokens |\n| Renderer | Draws styled cells into the terminal |\n\ndone",
        // HARD-WRAPPED rows (the model echoing terminal-wrapped source): the
        // pipe-carrying fragments (`96.4 ms |`, `15.0 ms |`) don't start
        // with `|` and re-join their rows (docs/table-streaming.md) — the
        // streamed commits + preview must match the batch render at every
        // prefix while the join forms (incl. mid-fragment prefixes, where
        // the pipe hasn't arrived yet and the tail still reads as prose).
        "pings:\n\n| Host | Loss | RTT |\n|------|------|-----|\n| google | 0% | 86.8 /\n96.4 ms |\n| fb | 0% |\n15.0 ms |\n\ndone",
        // --- Inline emphasis (docs/markdown.md): line-local, so a complete
        // line's styling is final (its frozen rows never restyle) while a
        // trailing line with an open marker is withheld (has_open_inline). The
        // streamed commits must match the batch render at every prefix/width,
        // including the marker-reveal flip and mid-word style changes. ---
        "first line **bold** here\nsecond *italic* line\nthird `code` end",
        // Emphasis closes early, then a long plain tail streams per row.
        "this **bold** part settles then a long plain tail that wraps across several rows here",
        // A bold phrase that itself wraps across rows (narrow widths hard-break it).
        "**wide bold phrase that wraps across multiple rows** then plain tail here",
        // Nested emphasis, a code span, a link and an image across a wrap.
        "nested **bold with _italic_ inside** and a `code span`\nsee [the docs](https://example.com/x) or ![pic](https://img/y.png) inline",
        // Non-emphasis markers stay literal: spaced `*`, snake_case `_`.
        "compute 2 * 3 and read foo_bar_baz then stop\ndone",
        // A strikethrough and inline code together, then a closing paragraph.
        "~~removed~~ and `kept` values differ\n\nsummary paragraph after a blank",
        // --- Lists & blockquotes (docs/markdown.md): line-local, so frozen
        // rows never restyle; the streamed commits must match batch at every
        // prefix/width, incl. the `-`-vs-partial-thematic-break handoff, the
        // hanging-indent wraps, and list text with inline emphasis. ---
        "- first item\n- second item\n  - nested item\ndone",
        "1. one\n2. two\n10. ten\ntail",
        // Deep nesting: a 4-space third level (and 6-space fourth) is a NESTED
        // item, not indented code — tight lists keep every level's indent.
        "- top\n  - mid\n    - deep\n      - deeper\nend",
        // A LOOSE nested list: the blank line used to hand the 4-space items
        // to the indented-code rule; list-marker lines are exempt from opening
        // an indented block, so they stay (styled) list items at every prefix.
        "- a\n\n    - loose nested\n\n      2. deep ordered\nend",
        // A nested ordered marker forming mid-stream: the prefix `    5` (a
        // bare digit run at 4+ spaces) must be withheld until the `.` settles
        // it — committing it as prose/code would diverge from the final list
        // render (is_partial_list_marker covers deep indents too).
        "quiz\n\n    5. five\n    6. six\nend",
        // The bullet/indented-code cliff: `    -` previews as an empty nested
        // bullet, but the completed `    --…` line is NOT a list item — after
        // a blank it opens an indented code block instead. Every prefix must
        // still match the batch render of that prefix.
        "x\n\n    -- weird\nend",
        "> a quoted line\n> continued quote\n\nafter the quote",
        "- a bullet with **bold** and `code` that wraps over rows\n- next item",
        "intro\n\n- [x] done task\n- [ ] pending task\n\nafter",
        "* star bullet\n+ plus bullet\ndone",
    ];

    for full in corpus {
        // Include pathologically narrow widths (content_width 1–2) where a
        // partial fence marker wraps into ≥2 rows — the invariant must hold
        // there too (`markdown::is_partial_fence`), not just at usable widths.
        for width in [3u16, 4, 5, 10, 16, 24, 40] {
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
                // Real streaming order: `commit` runs on the chunk's arrival,
                // the draw's `preview` after it.
                // (a)/(b) commit rows extend a stable prefix of the final render.
                committed.extend(render.commit(prefix, width).iter().map(styled));
                assert_eq!(
                    committed[..],
                    expected[..committed.len()],
                    "a committed row diverged while streaming {full:?} (w={width})"
                );
                // (c) the preview is a SUFFIX of the batch render of this
                // prefix — its last row outside a table, the whole
                // uncommitted tail (the forming block) while one is open
                // (docs/table-streaming.md).
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
                assert!(
                    got_preview.len() <= batch_prefix.len()
                        && got_preview[..]
                            == batch_prefix[batch_prefix.len() - got_preview.len()..],
                    "preview must be a suffix of the batch render at {prefix:?} (w={width}):\n got {got_preview:?}\nwant a tail of {batch_prefix:?}"
                );
                // (d) while a table is open, scrollback + strip together show
                // the WHOLE render — the table streams visibly even though
                // none of it has committed to scrollback yet.
                if ends_in_open_table(prefix) {
                    assert_eq!(
                        committed.len() + got_preview.len(),
                        batch_prefix.len(),
                        "open table: committed + preview must span the whole render at {prefix:?} (w={width})"
                    );
                }
            }
            committed.extend(render.finish(full, width).iter().map(styled));
            assert_eq!(committed, expected, "reconstruct {full:?} (w={width})");
        }
    }
}

#[test]
fn incremental_commits_reconstruct_the_whole_reply() {
    // Stream a reply word-by-word, committing stable lines as we go, and
    // confirm the committed lines (plus the final flush) exactly equal the
    // fully-rendered message — no gaps, no duplicates, no reordering.
    let full = "the quick brown fox jumps over the lazy dog and then \
                some extra words to force several wrapped lines here";
    let width = 20;
    let expected: Vec<String> = message_lines(Role::Assistant, full, width)
        .iter()
        .map(plain)
        .collect();

    let mut render = StreamRender::new();
    let mut got: Vec<String> = Vec::new();
    let mut acc = String::new();
    for chunk in crate::stream::chunks(full) {
        acc.push_str(&chunk);
        got.extend(render.commit(&acc, width).iter().map(plain));
    }
    got.extend(render.finish(&acc, width).iter().map(plain));

    assert_eq!(got, expected);
}

/// Exhaustively drive [`StreamRender`] over **every prefix** of a reply
/// (char-by-char) and prove two invariants that keep immutable scrollback
/// sound (CLAUDE.md invariant 2): a row, once committed, is **never**
/// re-emitted or changed, and committed rows + the final flush reconstruct
/// the whole rendered reply exactly. Runs a prose reply and a fenced-code
/// reply (the highlight-carry case).
#[test]
fn stream_render_is_prefix_stable_over_every_prefix() {
    // Compare full styled rows (text + fg colour of each span), so the scan
    // catches a recolour — e.g. a code identifier turning blue at its call
    // `(` after an earlier wrapped row was committed — not just a text change.
    let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    };
    for full in [
        "a short prose reply that wraps a few times across the width here",
        "intro line\n```python\ndef long_function_name_here(x):\n    s = \"\"\"multi\n    line\"\"\"\n    return s\n```\nend",
    ] {
        let width = 18;
        let expected: Vec<Vec<(String, Option<Color>)>> =
            message_lines(Role::Assistant, full, width)
                .iter()
                .map(styled)
                .collect();

        let mut render = StreamRender::new();
        let mut committed: Vec<Vec<(String, Option<Color>)>> = Vec::new();
        for end in 1..full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            committed.extend(render.commit(&full[..end], width).iter().map(styled));
            // Everything committed so far must be a stable prefix of the
            // final render — never a row that later changes text or colour.
            assert_eq!(
                committed[..],
                expected[..committed.len()],
                "a committed row changed while streaming {full:?}"
            );
        }
        committed.extend(render.finish(full, width).iter().map(styled));
        assert_eq!(committed, expected, "reconstruct {full:?}");
    }
}

#[test]
fn stream_render_withholds_the_last_line() {
    // "hi there" fits one line → nothing is stable yet.
    let mut render = StreamRender::new();
    assert!(render.commit("hi there", 80).is_empty());
}

#[test]
fn preview_never_shows_a_committed_row_while_streaming() {
    // Regression for the slow-stream duplicate-line bug: a chunk ending in a
    // newline completes a line, which `commit` must not flush to scrollback
    // while the strip still previews it — otherwise the line shows twice (once
    // committed, once previewed) until the next chunk arrives. Drive real
    // streaming order (commit before the draw's preview) over every prefix and
    // assert the preview is never a row already committed.
    let styled = |l: &Line| -> Vec<(String, Option<Color>)> {
        l.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect()
    };
    let width = 24;
    for full in [
        "first line here\nsecond line here\n\nthird paragraph line",
        "- bullet one\n- bullet two\n- bullet three\n",
        "some words that wrap a little here\n\nnext paragraph body\n",
        "## Heading Row\n\nbody text below it\n",
        // A streaming table: its rows commit progressively, so the preview
        // (the last streamed content row) must never duplicate a committed
        // one (docs/table-streaming.md).
        "here:\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\ndone\n",
    ] {
        let mut render = StreamRender::new();
        let mut committed: Vec<Vec<(String, Option<Color>)>> = Vec::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let prefix = &full[..end];
            committed.extend(render.commit(prefix, width).iter().map(styled));
            for p in render.preview(prefix, width, usize::MAX) {
                let p = styled(&p);
                // A non-blank preview row must not already sit in scrollback.
                if p.iter().any(|(t, _)| !t.trim().is_empty()) {
                    assert!(
                        !committed.contains(&p),
                        "preview {p:?} duplicates a committed row streaming {full:?} at {prefix:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn stream_render_preview_is_the_last_rendered_row() {
    // The strip preview must equal the last row of the full render — but
    // computed cheaply. Check it across a growing code reply.
    let full = "Here:\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
    let width = 30;
    let mut render = StreamRender::new();
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        let acc = &full[..end];
        let expected: Vec<String> = message_lines(Role::Assistant, acc, width)
            .pop()
            .map(|l| plain(&l))
            .into_iter()
            .collect();
        let got: Vec<String> = render
            .preview(acc, width, usize::MAX)
            .iter()
            .map(plain)
            .collect();
        assert_eq!(got, expected, "preview mismatch at {acc:?}");
        // Advancing the preview must not disturb a subsequent commit.
        let _ = render.commit(acc, width);
    }
}

#[test]
fn stream_render_rebuilds_on_a_width_change() {
    // A mid-stream width change (resize) must rebuild the cache from scratch
    // — no stale rows, no panic — matching the boundary's `reset` + re-commit.
    let text = "the quick brown fox jumps over the lazy dog";
    let mut render = StreamRender::new();
    let narrow = render.commit(text, 6);
    assert!(!narrow.is_empty(), "a narrow wrap commits several lines");

    // Re-wrapped wider: the cache rebuilds; committed + finish still equals
    // the whole reply rendered at the new width.
    let wide_commit = render.commit(text, 80);
    let wide_finish = render.finish(text, 80);
    let got: Vec<String> = wide_commit
        .iter()
        .chain(wide_finish.iter())
        .map(plain)
        .collect();
    let expected: Vec<String> = message_lines(Role::Assistant, text, 80)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(got, expected, "rebuilt at the new width");
}

#[test]
fn stream_render_trims_a_trailing_paragraph_break() {
    // Streaming "text.\n\n" char-by-char (as a real model emits before a tool
    // call) then finishing must commit no trailing blank rows — otherwise the
    // boundary's single spacer stacks into three (the reported bug).
    let full = "Building it.\n\n";
    let width = 80;
    let mut render = StreamRender::new();
    let mut got: Vec<String> = Vec::new();
    for end in 1..=full.len() {
        if !full.is_char_boundary(end) {
            continue;
        }
        got.extend(render.commit(&full[..end], width).iter().map(plain));
    }
    got.extend(render.finish(full, width).iter().map(plain));
    assert_eq!(
        got,
        vec!["● Building it.".to_string()],
        "no trailing blanks"
    );
}

#[test]
fn repaint_tail_repaints_committed_rows_of_a_still_open_line() {
    // The committed counter can point past `frozen` (wrapped rows of a
    // prose line that hasn't seen its newline yet) — those rows reached
    // scrollback too, so the repaint must reproduce them.
    let width = 18;
    let before = "a long prose line that wraps into a good number of rows here";
    let mut render = StreamRender::new();
    let committed: Vec<String> = render.commit(before, width).iter().map(plain).collect();
    assert!(
        committed.len() > 1,
        "several wrapped rows are stable: {committed:?}"
    );
    let full = format!("{before} and more");
    let tail: Vec<String> = repaint_tail(&[], Some(&full), &mut render, width, 100)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(tail, committed);
}

/// Whether `prefix` ends inside an OPEN GFM table — its last non-blank source
/// line is still a table row/delimiter/header, so no non-table line has
/// closed the block. Used by the differential test to skip the preview
/// equality check exactly where the streaming preview intentionally shows the
/// last content row rather than the batch's flushed bottom border.
fn ends_in_open_table(prefix: &str) -> bool {
    prefix
        .split('\n')
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(markdown::is_table_row)
}

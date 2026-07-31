//! Width math and the wrapping primitives.

use super::*;
use crate::ui::theme::{
    AI_BULLET, BULLET_WIDTH, ERROR_BULLET, INDENT, PROMPT, SHELL_MODE_COLOR, TOOL_ARGS_COLOR,
    TOOL_DIFF_ADD_BG, TOOL_DIFF_ADD_COLOR, TOOL_DIFF_DEL_COLOR, TOOL_HEADER_ELLIPSIS,
    TOOL_HEADER_MAX_ROWS, USER_BG_COLOR, USER_BULLET,
};
use crate::ui::tool::{running_command_lines, tool_full_lines};
use crate::ui::wrap::{cols, truncate_cols, wrap_output, wrap_verbatim};

#[test]
fn inline_emphasis_survives_a_wrap_boundary() {
    // A bold run that wraps keeps its style on every wrapped word.
    let lines = message_lines(Role::Assistant, "**alpha beta gamma delta**", 12);
    assert!(lines.len() >= 2, "wrapped into multiple rows");
    for line in &lines {
        for s in &line.spans {
            let t = s.content.as_ref();
            if t.trim().is_empty() || t == "● " {
                continue;
            }
            assert!(
                s.style.add_modifier.contains(Modifier::BOLD),
                "{t:?} should stay bold across the wrap"
            );
        }
    }
}

#[test]
fn list_item_wraps_with_a_hanging_indent() {
    // A long bullet wraps under the text, not back to the marker column.
    let rows: Vec<String> = message_lines(Role::Assistant, "- alpha beta gamma delta", 12)
        .iter()
        .map(plain)
        .collect();
    // content width 10: "- " marker leaves 8 for text.
    assert_eq!(
        rows,
        vec!["● - alpha", "    beta", "    gamma", "    delta"]
    );
}

#[test]
fn cols_measures_emoji_clusters_as_two_columns() {
    // The Claude-Code / string-width policy, delivered by unicode-width 0.2:
    // every emoji cluster — a VS16 presentation pair, a ZWJ sequence, a
    // skin-tone modifier, a flag, a keycap — is TWO columns, matching how
    // modern terminals draw them. All the table column math (natural widths,
    // allocation, cell padding) rests on this, so a dependency regression
    // here would shatter emoji grids again.
    for (cluster, what) in [
        ("✅", "EAW-wide check mark"),
        ("⚠\u{FE0F}", "VS16 emoji-presentation pair"),
        ("👍🏽", "skin-tone modifier sequence"),
        ("👨\u{200D}👩\u{200D}👧\u{200D}👦", "family ZWJ sequence"),
        ("🇵🇭", "regional-indicator flag pair"),
        ("1\u{FE0F}\u{20E3}", "keycap sequence"),
        ("❤\u{FE0F}\u{200D}🔥", "ZWJ sequence over a VS16 base"),
    ] {
        assert_eq!(cols(cluster), 2, "{what}: {cluster:?}");
    }
}

// --- wrap_text ---

#[test]
fn wrap_text_breaks_on_word_boundaries() {
    assert_eq!(wrap_text("hello world", 5), vec!["hello", "world"]);
}

#[test]
fn wrap_text_keeps_text_that_fits_on_one_line() {
    assert_eq!(wrap_text("hello world", 11), vec!["hello world"]);
}

#[test]
fn wrap_text_hard_breaks_words_longer_than_width() {
    assert_eq!(wrap_text("aaaaaa", 3), vec!["aaa", "aaa"]);
    assert_eq!(wrap_text("abcdefg", 3), vec!["abc", "def", "g"]);
}

#[test]
fn wrap_text_preserves_blank_lines() {
    assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
}

#[test]
fn wrap_text_of_empty_string_is_a_single_blank_line() {
    assert_eq!(wrap_text("", 10), vec![""]);
}

#[test]
fn wrap_text_with_zero_width_does_not_wrap() {
    assert_eq!(wrap_text("a b", 0), vec!["a b"]);
}

#[test]
fn wrap_text_hard_breaks_a_word_that_is_an_exact_multiple_of_width() {
    // A word whose length is an exact multiple of the width must not leave a
    // phantom trailing blank line or drop the final full chunk.
    assert_eq!(wrap_text("aaaaaaaaa", 3), vec!["aaa", "aaa", "aaa"]);
}

#[test]
fn wrap_text_packs_a_following_word_onto_a_hard_break_remainder() {
    // "abcdefg" (7) hard-breaks at width 5 into "abcde" + the remainder "fg".
    // The next token "h" was whitespace-separated in the input, so the space
    // in "fg h" is real — this is ordinary greedy packing (like `fold`), not
    // an invented word boundary. Locks the behaviour against regression.
    assert_eq!(wrap_text("abcdefg h", 5), vec!["abcde", "fg h"]);
}

#[test]
fn wrap_text_exact_multiple_remainder_does_not_absorb_the_next_word() {
    // "aaaaaa" is an exact multiple of 3 → two full lines, nothing buffered,
    // so the following word "bb" correctly starts its own line.
    assert_eq!(wrap_text("aaaaaa bb", 3), vec!["aaa", "aaa", "bb"]);
}

#[test]
fn wrap_text_measures_wide_chars_as_two_columns() {
    // CJK glyphs occupy 2 terminal columns each, so only two fit in width 4
    // — not four, as a naive char count would allow.
    assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
}

#[test]
fn wrap_text_treats_a_zero_width_combining_mark_as_zero_columns() {
    // "e" + combining acute is one column wide, so it fits a width-1 line
    // instead of being hard-broken onto two lines like a 2-char count implies.
    assert_eq!(wrap_text("e\u{0301}", 1), vec!["e\u{0301}"]);
}

#[test]
fn wrap_text_hard_break_keeps_zwj_emoji_clusters_whole() {
    // A family emoji is one grapheme cluster (four emoji joined by U+200D
    // zero-width joiners). The hard-break must split the over-long "word"
    // only on grapheme boundaries — never inside a cluster, which would
    // leave a bare ZWJ at a line end and render broken glyphs.
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    let text = FAMILY.repeat(50);
    let lines = wrap_text(&text, 78);
    assert!(lines.len() > 1, "the run is hard-broken across lines");
    for line in &lines {
        assert!(
            !line.ends_with('\u{200D}'),
            "no line ends mid-cluster on a joiner: {line:?}"
        );
        assert!(
            line.len() % FAMILY.len() == 0
                && line.matches(FAMILY).count() * FAMILY.len() == line.len(),
            "every line is whole clusters only: {line:?}"
        );
    }
    assert_eq!(lines.concat(), text, "no text lost by the break");
}

// --- wrap_output (word-boundary + whitespace-preserving, for tool output) ---

#[test]
fn wrap_output_breaks_at_word_boundaries_keeping_the_space() {
    let rows = wrap_output("sudo: a terminal is required", 12);
    assert!(rows.len() > 1, "the line wraps: {rows:?}");
    for r in &rows {
        assert!(cols(r) <= 12, "no row overflows: {rows:?}");
    }
    for r in &rows[1..] {
        assert!(
            !r.starts_with(' '),
            "continuations start at a word: {rows:?}"
        );
    }
    // The boundary space stays at the end of the current row, so
    // concatenating the rows reconstructs the line byte-exactly.
    assert_eq!(rows.concat(), "sudo: a terminal is required");
    for r in &rows[..rows.len() - 1] {
        assert!(
            r.ends_with(' '),
            "each break lands just past a space: {rows:?}"
        );
    }
}

#[test]
fn wrap_output_preserves_internal_space_runs_that_fit() {
    // Column-aligned output that fits is untouched — byte-exact.
    assert_eq!(
        wrap_output("-rw-r--r--  1 user   42 a.txt", 60),
        vec!["-rw-r--r--  1 user   42 a.txt"]
    );
}

#[test]
fn wrap_output_hard_breaks_an_overlong_word() {
    // A single word wider than the width can't break at a space — it
    // hard-breaks on grapheme boundaries like wrap_verbatim.
    let rows = wrap_output(&"x".repeat(25), 10);
    assert_eq!(rows, vec!["x".repeat(10), "x".repeat(10), "x".repeat(5)]);
}

#[test]
fn wrap_output_keeps_leading_indentation() {
    assert_eq!(
        wrap_output("    indented text", 40),
        vec!["    indented text"]
    );
}

#[test]
fn wrap_output_preserves_empty_lines() {
    assert_eq!(wrap_output("a\n\nb", 10), vec!["a", "", "b"]);
}

#[test]
fn wrap_output_zero_width_disables_wrapping() {
    assert_eq!(wrap_output("a b c", 0), vec!["a b c"]);
}

// --- wrap_verbatim (the whitespace-preserving wrap for tool output) ---

#[test]
fn wrap_verbatim_preserves_leading_indentation() {
    assert_eq!(
        wrap_verbatim("    fn main() {", 40),
        vec!["    fn main() {"]
    );
}

#[test]
fn wrap_verbatim_preserves_internal_space_runs() {
    // `ls -l` / `tree` output is column-aligned by space runs; every byte
    // of a line that fits must survive verbatim.
    assert_eq!(
        wrap_verbatim("-rw-r--r--  1 user   42 a.txt\n│   ├── b", 60),
        vec!["-rw-r--r--  1 user   42 a.txt", "│   ├── b"]
    );
}

#[test]
fn wrap_verbatim_preserves_empty_lines() {
    assert_eq!(wrap_verbatim("a\n\nb", 10), vec!["a", "", "b"]);
}

#[test]
fn wrap_verbatim_hard_breaks_wide_chars_on_column_boundaries() {
    // CJK glyphs are two columns each, so only two fit in width 4 — the
    // break is display-width-aware like wrap_text's.
    assert_eq!(wrap_verbatim("你好世界", 4), vec!["你好", "世界"]);
}

#[test]
fn wrap_verbatim_hard_breaks_on_grapheme_boundaries() {
    // A ZWJ family-emoji cluster is never severed mid-joiner.
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    let text = FAMILY.repeat(10);
    let lines = wrap_verbatim(&text, 8);
    assert!(lines.len() > 1, "hard-broken across lines");
    for line in &lines {
        assert!(
            !line.ends_with('\u{200D}'),
            "no line ends mid-cluster: {line:?}"
        );
    }
    assert_eq!(lines.concat(), text, "no text lost by the break");
}

#[test]
fn wrap_verbatim_keeps_spaces_at_a_hard_break() {
    // Unlike wrap_text, the break invents no word boundaries and drops no
    // whitespace: the wrapped pieces re-concatenate to the original line.
    let line = "  indented   with  runs  and  more  padding  ";
    let lines = wrap_verbatim(line, 10);
    assert!(lines.len() > 1);
    assert_eq!(lines.concat(), line);
}

#[test]
fn wrap_verbatim_with_zero_width_does_not_wrap() {
    assert_eq!(wrap_verbatim("a  b", 0), vec!["a  b"]);
}

#[test]
fn assistant_over_width_code_hard_breaks_keeping_whitespace() {
    // A code line wider than the code area hard-breaks (no word-collapse); the
    // first row keeps the leading indentation. There is no language label and
    // no gutter, so the rendered rows are the wrapped code alone (under the
    // bullet/indent).
    let src = "```\n        eight_spaces_then_a_very_long_token_here\n```";
    let lines = message_lines(Role::Assistant, src, 22);
    let code: Vec<String> = lines.iter().map(plain).collect();
    // At least two wrapped code rows (no label row now).
    assert!(code.len() >= 2, "hard-broke into rows: {code:?}");
    assert!(
        code.iter().any(|l| l.contains("        eight")),
        "leading spaces survive on the first code row: {code:?}"
    );
}

#[test]
fn tool_lines_wraps_a_long_command_output_line_instead_of_clipping() {
    // The FINISHED (committed) bash cell must wrap a long output line like
    // the Ctrl+O view does, not clip it at the width — the text used to
    // disappear past the terminal edge (the reported bug;
    // docs/tool-streaming.md).
    let long = "0123456789".repeat(6); // 60 cols
    let lines = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &long), 40);
    // width 40 − the 5-col `  ⎿  ` gutter = 35 content cols → 2 rows, and a
    // single source line → no hint.
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 2, "the 60-col line wraps to two rows: {body:?}");
    let joined: String = body
        .iter()
        .map(|l| l.chars().skip(5).collect::<String>()) // drop the 5-col gutter
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(joined, long, "every column is preserved, not clipped");
    for l in &lines {
        assert!(cols(&plain(l)) <= 40, "no row overflows the width: {l:?}");
    }
}

#[test]
fn a_finished_shell_output_wraps_a_long_line_showing_every_line() {
    // The reported bug, mechanism for mechanism: a `! sudo …` cell whose
    // first output line is longer than the terminal used to lose the text
    // past the edge. Both lines must show in full, wrapped, with no expand
    // hint (they fit the row budget).
    let out = "sudo: a terminal is required to read the password; either use the -S option\n\
               sudo: a password is required";
    let mut t = tool("sudo pacman -Rns steam", "", ToolStatus::Ok, out);
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 50).iter().map(plain).collect();
    let joined = lines.join("\n");
    assert!(
        joined.contains("either use the -S option"),
        "the full first line survives the wrap: {joined:?}"
    );
    assert!(
        joined.contains("a password is required"),
        "the second line still shows: {joined:?}"
    );
    assert!(
        !joined.contains("ctrl+o to expand"),
        "both lines fit the budget — no hint: {joined:?}"
    );
    for l in &lines {
        assert!(cols(l) <= 50, "no row overflows the width: {l:?}");
    }
}

#[test]
fn running_command_lines_wraps_a_long_tail_line_instead_of_clipping() {
    // The inline streaming tail must wrap like the Ctrl+O view does
    // (wrap_verbatim — docs/tool-streaming.md), not silently clip at the
    // width: every streamed column stays visible in the live cell.
    let long = "0123456789".repeat(6); // 60 cols
    let t = tool("Bash", "cat log", ToolStatus::Running, &long);
    let lines = running_command_lines(&t, Duration::from_secs(1), Duration::ZERO, 40);
    // width 40 − the 5-col `  ⎿  ` gutter = 35 content cols → 2 rows.
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 2, "the 60-col line wraps to two rows: {body:?}");
    // Strip the 5-char `  ⎿  ` gutter / continuation indent off each row.
    let joined: String = body
        .iter()
        .map(|l| l.chars().skip(5).collect::<String>())
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(joined, long, "no column is dropped: {body:?}");
}

#[test]
fn preview_rows_counts_a_wrapped_running_tail() {
    // Strip sizing and paint agree when the tail wraps: preview_rows
    // counts the wrapped rows (header + windowed tail + the Ctrl+B hint),
    // not one row per source line.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "cat log");
    // Past the hint delay so the Ctrl+B hint row is part of the preview.
    app.set_command_elapsed(Some(Duration::from_secs(3)));
    app.push_tool_output(&"y".repeat(70)); // 35 content cols → 2 rows
    assert_eq!(
        preview_rows(&app, 40),
        4,
        "header + 2 wrapped tail rows + the Ctrl+B hint"
    );
}

#[test]
fn tool_lines_wraps_a_long_header_aligned_under_the_open_paren() {
    // A long `Bash(…)` command must not run off the terminal edge: the args
    // word-wrap across continuation rows, each indented to align **under the
    // opening `(`** (Claude-Code's wrapped header) so the wrap reads clean and
    // no part of the command is lost.
    let cmd = "curl -s \"wttr.in/Warsaw?format=%C+%t+%w+%h\" 2>/dev/null \
               || echo \"wttr.in unavailable, trying alternative...\"";
    let lines = tool_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 80);
    let header: Vec<String> = lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .map(plain)
        .collect();
    // The header spans more than one row but stays within the inline cap.
    assert!(
        (2..=TOOL_HEADER_MAX_ROWS).contains(&header.len()),
        "long header wraps: {header:?}"
    );
    // No row exceeds the width — nothing is clipped.
    for l in &lines {
        assert!(
            cols(&plain(l)) <= 80,
            "row stays within the width: {:?}",
            plain(l)
        );
    }
    // Row 0 opens the header; the `(` sits at `cols("● Bash")`.
    assert!(header[0].starts_with("● Bash("), "row 0 opens the header");
    let paren_col = cols("● Bash");
    assert_eq!(
        header[0].chars().nth(paren_col),
        Some('('),
        "the open paren sits at cols(\"● Bash\")"
    );
    // The continuation is indented by exactly that many spaces, so its first
    // character lands directly under the `(` — not one column past it.
    assert!(
        header[1].starts_with(&" ".repeat(paren_col)),
        "continuation is indented to the open paren: {:?}",
        header[1]
    );
    assert_ne!(
        header[1].chars().nth(paren_col),
        Some(' '),
        "the continuation's content begins right under the (: {:?}",
        header[1]
    );
    // Nothing is lost — the command's start and end both survive.
    let joined: String = header
        .iter()
        .map(|r| r.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("curl -s"),
        "keeps the command head: {joined:?}"
    );
    assert!(
        joined.contains("alternative...\")"),
        "keeps the command tail and closing paren: {joined:?}"
    );
}

#[test]
fn tool_lines_truncates_a_very_long_header_with_an_ellipsis() {
    // A very long command is capped inline at TOOL_HEADER_MAX_ROWS wrapped
    // rows, the remainder replaced by `…)` (Claude-Code's truncated command);
    // the whole thing is still shown in the Ctrl+O view.
    let cmd = "for i in {1..5}; do echo \"=== Iteration $i ===\" \
               && echo \"Current time: $(date)\" \
               && echo \"System uptime: $(uptime)\" \
               && echo \"Memory usage: $(free -h | grep Mem)\"; done";
    let lines = tool_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 50);
    let header: Vec<String> = lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .map(plain)
        .collect();
    assert_eq!(
        header.len(),
        TOOL_HEADER_MAX_ROWS,
        "header caps at the row limit: {header:?}"
    );
    assert!(
        header
            .last()
            .unwrap()
            .trim_end()
            .ends_with(&format!("{TOOL_HEADER_ELLIPSIS})")),
        "the last shown row ends with the ellipsis + closing paren: {:?}",
        header.last().unwrap()
    );
    // The truncation `…` is the same bold white as the args, not dim grey.
    let ell_line = lines
        .iter()
        .find(|l| plain(l).contains(TOOL_HEADER_ELLIPSIS))
        .unwrap();
    let ell = ell_line
        .spans
        .iter()
        .find(|s| s.content.contains(TOOL_HEADER_ELLIPSIS))
        .unwrap();
    assert_eq!(
        ell.style.fg,
        Some(TOOL_ARGS_COLOR),
        "the truncation … matches the args colour, not grey"
    );
    // Still no clipping past the width.
    for l in &lines {
        assert!(
            cols(&plain(l)) <= 50,
            "row stays within the width: {:?}",
            plain(l)
        );
    }
    // The continuations still align under the `(`.
    let paren_col = cols("● Bash");
    assert!(header[1].starts_with(&" ".repeat(paren_col)));
}

#[test]
fn edit_full_view_colours_wrapped_continuation_rows_by_their_source_line() {
    // A `+` line longer than the width wraps into several rows in the Ctrl+O
    // view. Every wrapped row of an added line must stay green (and a removed
    // line red) — colouring each display row by ITS OWN first char would
    // leave the marker-less continuation rows dim (or mis-colour them).
    let added = format!("+{}", "x".repeat(60));
    let removed = format!("-{}", "y".repeat(60));
    let output = format!("Updated a.rs (+1 -1)\n{added}\n{removed}");
    let width = 24; // body width ~20 → the 61-char lines wrap into several rows
    let lines = tool_full_lines(&tool("Edit", "a.rs", ToolStatus::Ok, &output), width);
    let content_fg = |l: &Line| l.spans.last().unwrap().style.fg;
    let add_rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('x')).collect();
    let del_rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('y')).collect();
    assert!(
        add_rows.len() > 1,
        "the added line wrapped into multiple rows"
    );
    assert!(
        del_rows.len() > 1,
        "the removed line wrapped into multiple rows"
    );
    for row in add_rows {
        assert_eq!(
            content_fg(row),
            Some(TOOL_DIFF_ADD_COLOR),
            "every wrapped +row is green"
        );
    }
    for row in del_rows {
        assert_eq!(
            content_fg(row),
            Some(TOOL_DIFF_DEL_COLOR),
            "every wrapped -row is red"
        );
    }
}

#[test]
fn file_cell_wraps_long_rows_under_the_content_column() {
    // A long numbered row wraps (not truncates); continuations align under
    // the content column and keep the row's tint, and no row overflows.
    let output = format!("Updated a.rs (+1 -0)\n1 +{}", "x".repeat(60));
    let width = 30u16;
    let lines = tool_full_lines(&tool("Edit", "a.rs", ToolStatus::Ok, &output), width);
    let rows: Vec<_> = lines.iter().filter(|l| plain(l).contains('x')).collect();
    assert!(rows.len() > 1, "the 60-char row wrapped");
    let cont = plain(rows[1]);
    // 6 (numbered indent) + cols("1 +") = 9 blank columns, then the content.
    assert!(cont.starts_with("         x"), "got {cont:?}");
    for r in &rows {
        assert!(
            r.spans
                .iter()
                .skip(1)
                .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG)),
            "every wrapped row keeps the add tint"
        );
        assert!(cols(&plain(r)) <= width as usize);
    }
}

#[test]
fn tool_lines_wraps_a_peek_line_within_the_width_preserving_content() {
    // A peek line never overflows the terminal width (column-aware) — and,
    // when it's longer than the width, it **wraps** rather than clipping,
    // so no content is lost. A 50-col line at width 30 (25 content cols)
    // fits the row budget → two wrapped rows, no hint, every column kept.
    let long = "abcdefghij".repeat(5); // 50 cols
    let lines = tool_lines(&tool("Bash", "y", ToolStatus::Ok, &long), 30);
    for line in &lines {
        assert!(cols(&plain(line)) <= 30, "no line exceeds the width");
    }
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 2, "the 50-col line wraps to two rows: {body:?}");
    let joined: String = body
        .iter()
        .map(|l| l.chars().skip(5).collect::<String>()) // drop the 5-col gutter
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(joined, long, "every column is preserved by the wrap");
}

#[test]
fn a_shell_tools_full_output_keeps_its_whitespace_verbatim() {
    // `ls -l` columns / `tree` guides / indented code must survive the
    // expanded (Ctrl+O) view byte-for-byte: the collapsed inline peek shows
    // these lines verbatim (`truncate_cols`), so the "full output" view
    // must never be *less* faithful by collapsing the space runs.
    let mut t = tool(
        "tree",
        "",
        ToolStatus::Ok,
        "/home/me\n│   ├── a\n    indented   run",
    );
    t.shell = true;
    let lines: Vec<String> = tool_full_lines(&t, 80).iter().map(plain).collect();
    assert_eq!(
        lines,
        vec!["  ⎿  /home/me", "     │   ├── a", "         indented   run",],
        "every output line verbatim under the corner"
    );
}

#[test]
fn cursor_sits_on_the_last_wrapped_input_row() {
    // "ab\ncd" → two rows; the cursor follows the end onto the second text
    // row (y = 2) just after the indented "cd" (x = 2 + 2).
    let mut app = App::new();
    app.input = TextArea::from_text("ab\ncd");
    let area = Rect::new(
        0,
        0,
        20,
        live_height(&app.input, 20, 24, false, 0, 0, 0, 0, 0, 0),
    );
    assert_eq!(cursor_position(area, &app), (4, 2));
}

#[test]
fn bullet_prefixes_all_occupy_bullet_width_columns() {
    let bw = BULLET_WIDTH as usize;
    assert_eq!(cols(PROMPT), bw, "prompt width matches BULLET_WIDTH");
    assert_eq!(cols(USER_BULLET), bw, "user bullet matches BULLET_WIDTH");
    assert_eq!(cols(AI_BULLET), bw, "assistant bullet matches BULLET_WIDTH");
    assert_eq!(cols(ERROR_BULLET), bw, "error bullet matches BULLET_WIDTH");
    assert_eq!(cols(INDENT), bw, "continuation indent matches BULLET_WIDTH");
}

#[test]
fn header_falls_back_to_the_compact_wordmark_when_mid_width() {
    let width = 50;
    let lines = header_lines(&with_session(), width);
    let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    // The compact half-block wordmark uses `▀`, which the full block art
    // never does — so its presence proves the mid-width tier was chosen.
    assert!(text.contains('▀'), "compact half-block wordmark: {text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version kept: {text:?}"
    );
    assert!(text.contains("~/alter-zero"), "cwd kept: {text:?}");
    for line in &lines {
        assert!(
            cols(&plain(line)) <= width as usize,
            "fits {width}: {line:?}"
        );
    }
}

#[test]
fn header_never_exceeds_the_width() {
    let app = with_session();
    for width in [16u16, 20, 24, 39, 40, 50, 73, 75, 80, 120] {
        for line in header_lines(&app, width) {
            assert!(
                cols(&plain(&line)) <= width as usize,
                "width {width}: row {:?} overflows",
                plain(&line)
            );
        }
    }
}

#[test]
fn a_shell_header_message_renders_like_a_user_line_with_a_bang() {
    let lines = message_lines(Role::Shell, "pwd", 40);
    assert_eq!(plain(&lines[0]).trim_end(), "! pwd");
    // The bang is the shell accent; the row carries the dark user block,
    // padded to the full content width (the mock's "dark line wrap").
    assert_eq!(lines[0].spans[0].style.fg, Some(SHELL_MODE_COLOR));
    assert_eq!(lines[0].style.bg, Some(USER_BG_COLOR));
    assert_eq!(cols(&plain(&lines[0])), 40, "padded to the full width");
}

#[test]
fn a_truncated_shell_output_appends_an_ellipsis_marker_in_the_full_view() {
    // Over-cap output is cut to its head; the expanded (Ctrl+O) view appends
    // a dim `…` line after the last retained line so the user sees more was
    // dropped (it is not recoverable — nothing to expand to).
    let mut t = tool("tree ~/", "", ToolStatus::Ok, "/home/me\n├── a\n├── b");
    t.shell = true;
    t.truncated = true;
    let lines: Vec<String> = tool_full_lines(&t, 70)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(lines[0], "  ⎿  /home/me");
    assert_eq!(lines[1], "     ├── a");
    assert_eq!(lines[2], "     ├── b");
    assert_eq!(
        lines[3], "     …",
        "the truncation marker, aligned under the corner: {lines:?}"
    );
}

#[test]
fn a_complete_shell_output_has_no_truncation_marker() {
    // When the output was kept in full, no `…` marker is appended.
    let mut t = tool("ls", "", ToolStatus::Ok, "a\nb");
    t.shell = true;
    let lines: Vec<String> = tool_full_lines(&t, 70)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(
        lines,
        vec!["  ⎿  a", "     b"],
        "no trailing marker: {lines:?}"
    );
}

// ===== audited-defect regressions (2026-07 review) =====

#[test]
fn truncate_cols_measures_graphemes_not_chars() {
    // ❤️ (U+2764 U+FE0F) paints 2 columns (str-level width, what ratatui
    // uses); summing per-char widths counted 1 and let truncations
    // overflow their budget 2x.
    let hearts = "❤️".repeat(4); // 8 display columns
    assert_eq!(cols(&hearts), 8);
    let kept = truncate_cols(&hearts, 4);
    assert_eq!(cols(&kept), 4, "the kept prefix fits the budget as painted");
    assert_eq!(kept, "❤️".repeat(2));
}

#[test]
fn truncate_cols_never_splits_a_zwj_cluster() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"; // 👨‍👩‍👧, 2 cols
    let s = format!("{family} ok");
    assert_eq!(
        truncate_cols(&s, 3),
        format!("{family} "),
        "the whole cluster (2 cols) fits a 3-col budget"
    );
    assert_eq!(
        truncate_cols(&s, 1),
        "",
        "a cluster wider than the budget is dropped whole, never split"
    );
}

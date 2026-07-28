//! The assistant markdown renderer: fenced code, headings, lists, quotes
//! (`docs/markdown.md`).

use super::*;
use crate::ui::assistant::assistant_lines;
use crate::ui::theme::{
    AI_BULLET, AI_COLOR, BULLET_WIDTH, CODE_TAB_WIDTH, INLINE_CODE_COLOR, MENU_DIM_COLOR,
    MENU_SELECTED_COLOR, THEMATIC_BREAK,
};

#[test]
fn assistant_inline_emphasis_styles_spans() {
    let lines = message_lines(Role::Assistant, "a **b** _i_ ~~s~~ `c`", 80);
    let spans: Vec<(String, Modifier, Option<Color>)> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| (s.content.to_string(), s.style.add_modifier, s.style.fg))
        .collect();
    assert!(
        spans
            .iter()
            .any(|(t, m, _)| t == "b" && m.contains(Modifier::BOLD))
    );
    assert!(
        spans
            .iter()
            .any(|(t, m, _)| t == "i" && m.contains(Modifier::ITALIC))
    );
    assert!(
        spans
            .iter()
            .any(|(t, m, _)| t == "s" && m.contains(Modifier::CROSSED_OUT))
    );
    assert!(
        spans
            .iter()
            .any(|(t, _, fg)| t == "c" && *fg == Some(INLINE_CODE_COLOR))
    );
    // No raw markers leak through.
    let joined: String = spans.iter().map(|(t, _, _)| t.as_str()).collect();
    assert!(!joined.contains("**") && !joined.contains("~~") && !joined.contains('`'));
}

#[test]
fn assistant_inline_link_shows_text_then_url() {
    let joined: String = message_lines(Role::Assistant, "see [docs](https://x.com) ok", 80)
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.to_string())
        .collect();
    assert!(joined.contains("docs"), "link text kept: {joined:?}");
    assert!(joined.contains("(https://x.com)"), "url shown: {joined:?}");
    assert!(
        !joined.contains("[docs]"),
        "raw link syntax gone: {joined:?}"
    );
}

#[test]
fn assistant_renders_bullet_and_ordered_lists() {
    // Bullets keep the `-`, ordered items keep `N.`, and nesting indent
    // survives (the earlier wrap_text-collapses-whitespace bug).
    let text = "- first\n- second\n  - nested\n\n1. one\n2. two";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        rows,
        vec![
            "● - first",
            "  - second",
            "    - nested",
            "  ",
            "  1. one",
            "  2. two",
        ]
    );
}

#[test]
fn assistant_renders_blockquote() {
    let rows: Vec<String> = message_lines(Role::Assistant, "> quoted text", 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(rows, vec!["● > quoted text"]);
}

// --- assistant markdown: fenced code blocks + headings (docs/markdown.md) ---

#[test]
fn assistant_prose_is_byte_identical_to_the_plain_path() {
    // Fence/heading-free assistant text must render exactly as before — the
    // markdown path is transparent to ordinary prose (regression guard).
    let text = "the quick brown fox jumps over the lazy dog and then more";
    let width = 20u16;
    let expected = wrap_text(text, width - BULLET_WIDTH);
    let got: Vec<String> = message_lines(Role::Assistant, text, width)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(got.len(), expected.len());
    assert_eq!(got[0], format!("● {}", expected[0]));
    for (g, e) in got.iter().zip(expected.iter()).skip(1) {
        assert_eq!(g, &format!("  {e}"));
    }
}

#[test]
fn assistant_code_block_preserves_indentation() {
    // THE BUG: fenced code keeps its leading whitespace — not collapsed the
    // way prose word-wrap would.
    let src = "```py\ndef f():\n    return 1\n```";
    let joined: Vec<String> = message_lines(Role::Assistant, src, 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        joined.iter().any(|l| l.contains("    return 1")),
        "4-space indent kept: {joined:?}"
    );
    assert!(joined.iter().any(|l| l.contains("def f():")), "{joined:?}");
}

#[test]
fn assistant_code_hides_the_fences_and_the_language_label() {
    let joined: Vec<String> = message_lines(Role::Assistant, "```python\nx = 1\n```", 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        !joined.iter().any(|l| l.contains("```")),
        "fences hidden: {joined:?}"
    );
    assert!(
        !joined.iter().any(|l| l.contains("python")),
        "language label NOT shown: {joined:?}"
    );
    // The code itself still renders.
    assert!(
        joined.iter().any(|l| l.contains("x = 1")),
        "code shown: {joined:?}"
    );
}

#[test]
fn assistant_code_rows_have_no_gutter_and_use_the_code_colour() {
    let lines = message_lines(Role::Assistant, "```\nx=1\n```", 80);
    let code = lines
        .iter()
        .find(|l| plain(l).contains("x=1"))
        .expect("a code row");
    assert!(
        !plain(code).contains('▏'),
        "no gutter bar: {:?}",
        plain(code)
    );
    assert!(
        code.spans
            .iter()
            .any(|s| s.style.fg == highlight::plain_style().fg),
        "unhighlighted code text uses the plain (theme-default) code colour"
    );
}

#[test]
fn assistant_code_expands_tabs_so_indentation_survives() {
    // THE BUG: Go (and many langs) indent with TAB. A tab is zero display
    // width (unicode-width), so rendered verbatim it collapses and the code
    // loses all its indentation. Tabs must be expanded to spaces for display
    // (matching codex — a fixed CODE_TAB_WIDTH substitution, not tab stops).
    let src = "```go\nfunc main() {\n\tfmt.Println(\"hi\")\n\t\tnested()\n}\n```";
    let lines = message_lines(Role::Assistant, src, 80);
    let row = |needle: &str| -> String {
        plain(lines.iter().find(|l| plain(l).contains(needle)).unwrap())
    };
    let println = row("fmt.Println");
    assert!(!println.contains('\t'), "no raw tab survives: {println:?}");
    // The 2-col continuation indent, then one tab → CODE_TAB_WIDTH spaces.
    let one = "  ".to_string() + &" ".repeat(CODE_TAB_WIDTH);
    assert!(
        println.starts_with(&format!("{one}fmt.Println")),
        "one tab expands to {CODE_TAB_WIDTH} spaces of indent: {println:?}"
    );
    // Two tabs → twice the indent, so nesting reads as deeper.
    let nested = row("nested()");
    let two = "  ".to_string() + &" ".repeat(CODE_TAB_WIDTH * 2);
    assert!(
        nested.starts_with(&format!("{two}nested()")),
        "two tabs expand to {} spaces: {nested:?}",
        CODE_TAB_WIDTH * 2
    );
}

#[test]
fn assistant_headings_keep_the_hashes_and_style_per_level_like_codex() {
    // Codex keeps the `#` markers visible and styles the whole heading line
    // per level with text *modifiers only* (no foreground colour): h1
    // bold+underlined, h2 bold, h3 bold+italic, h4-6 italic. See
    // docs/markdown.md and codex-rs/tui/src/markdown_render.rs::start_heading.
    let h2 = message_lines(Role::Assistant, "## The Code", 80);
    assert_eq!(plain(&h2[0]), "● ## The Code", "hashes kept, not stripped");

    // The heading-text span carries the level's modifiers and no fg override.
    let style_of = |level: u8| {
        let src = format!("{} Heading", "#".repeat(level as usize));
        let lines = message_lines(Role::Assistant, &src, 80);
        lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Heading"))
            .expect("heading text span")
            .style
    };
    for level in 1..=6u8 {
        assert_eq!(
            style_of(level).fg,
            None,
            "codex headings carry no colour (h{level})"
        );
    }
    let m = |level: u8| style_of(level).add_modifier;
    assert!(
        m(1).contains(Modifier::BOLD) && m(1).contains(Modifier::UNDERLINED),
        "h1 bold+underlined"
    );
    assert!(
        m(2).contains(Modifier::BOLD)
            && !m(2).contains(Modifier::ITALIC)
            && !m(2).contains(Modifier::UNDERLINED),
        "h2 bold only"
    );
    assert!(
        m(3).contains(Modifier::BOLD) && m(3).contains(Modifier::ITALIC),
        "h3 bold+italic"
    );
    assert!(
        m(4).contains(Modifier::ITALIC) && !m(4).contains(Modifier::BOLD),
        "h4 italic only"
    );
    assert!(
        m(6).contains(Modifier::ITALIC) && !m(6).contains(Modifier::BOLD),
        "h6 italic"
    );
}

#[test]
fn assistant_thematic_break_renders_an_em_dash_rule_like_codex() {
    // `---`/`***`/`___` after a blank line render as codex's `———` (Event::Rule),
    // with the raw markers gone.
    for src in [
        "intro\n\n---\nmore",
        "intro\n\n***\nmore",
        "intro\n\n___\nmore",
    ] {
        let joined: Vec<String> = message_lines(Role::Assistant, src, 80)
            .iter()
            .map(plain)
            .collect();
        assert!(
            joined.iter().any(|l| l.contains(THEMATIC_BREAK)),
            "em-dash rule rendered for {src:?}: {joined:?}"
        );
        assert!(
            !joined
                .iter()
                .any(|l| l.contains("---") || l.contains("***") || l.contains("___")),
            "raw markers gone for {src:?}: {joined:?}"
        );
    }
}

#[test]
fn assistant_indented_code_renders_verbatim_like_codex() {
    // A 4-space-indented run after a blank is an indented code block: its
    // indentation is preserved (unlike prose, which collapses leading space).
    let joined: Vec<String> = message_lines(
        Role::Assistant,
        "intro\n\n    x = 1\n        deep()\nback",
        80,
    )
    .iter()
    .map(plain)
    .collect();
    assert!(
        joined.iter().any(|l| l.contains("    x = 1")),
        "4-space indent kept: {joined:?}"
    );
    assert!(
        joined.iter().any(|l| l.contains("        deep()")),
        "8-space indent kept: {joined:?}"
    );
}

#[test]
fn assistant_code_is_syntax_highlighted() {
    // We assert real, multi-colour highlighting without pinning the theme's
    // exact RGB (codex's own test style): each token class is coloured, and
    // the classes differ from one another and from plain prose.
    let lines = message_lines(
        Role::Assistant,
        "```python\ndef f():\n    x = \"hi\"  # note\n```",
        80,
    );
    let fg_of = |needle: &str| -> Option<Color> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(needle))
            .and_then(|s| s.style.fg)
    };
    let kw = fg_of("def");
    let func = fg_of("f");
    let string = fg_of("\"hi\"");
    let comment = fg_of("# note");
    for (name, c) in [
        ("def", kw),
        ("f", func),
        ("\"hi\"", string),
        ("# note", comment),
    ] {
        assert!(c.is_some(), "{name} should be coloured");
    }
    assert_ne!(kw, string, "keyword and string differ");
    assert_ne!(string, comment, "string and comment differ");
    assert_ne!(kw, comment, "keyword and comment differ");
}

#[test]
fn code_fences_stay_literal_for_non_assistant_roles() {
    // A user pasting triple-backticks must not be markdown-processed.
    let joined: Vec<String> = message_lines(Role::User, "```\ncode\n```", 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        joined.iter().any(|l| l.contains("```")),
        "user fences stay literal: {joined:?}"
    );
}

#[test]
fn a_fence_only_reply_renders_a_lone_bullet_in_batch_and_streaming() {
    // With fences hidden and no language label, a reply that is *only* a code
    // fence renders to zero body rows — but the role bullet still needs a
    // home, and the batch and streaming paths must agree on it (else the strip
    // preview would diverge from a scrollback repaint).
    let full = "```";
    let width = 40;
    let expected: Vec<String> = message_lines(Role::Assistant, full, width)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        expected,
        vec!["● ".to_string()],
        "batch: a lone bullet home"
    );

    let mut render = StreamRender::new();
    assert_eq!(
        render
            .preview(full, width, usize::MAX)
            .iter()
            .map(plain)
            .collect::<Vec<_>>(),
        vec!["● ".to_string()],
        "preview matches the batch bullet home"
    );
    let committed: Vec<String> = render.finish(full, width).iter().map(plain).collect();
    assert_eq!(committed, expected, "streamed finish matches batch");
}

#[test]
fn assistant_lines_trims_a_trailing_paragraph_break() {
    // A reply ending with a blank line (a model often emits "…\n\n" before a
    // tool call) must render no trailing blank rows: the caller adds exactly
    // one spacer, so trailing blanks would stack (the 3-newline bug).
    let with: Vec<String> = assistant_lines("Building it.\n\n", 80, AI_BULLET, AI_COLOR)
        .iter()
        .map(plain)
        .collect();
    let without: Vec<String> = assistant_lines("Building it.", 80, AI_BULLET, AI_COLOR)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(with, without, "trailing blank rows are trimmed");
}

#[test]
fn assistant_lines_keeps_interior_blank_lines() {
    // Only *trailing* blanks are trimmed — a paragraph break in the middle
    // stays (it separates two paragraphs).
    let rows = assistant_lines("One.\n\nTwo.", 80, AI_BULLET, AI_COLOR);
    assert_eq!(rows.len(), 3, "the interior blank is preserved");
    assert!(plain(&rows[0]).contains("One."));
    assert!(plain(&rows[1]).trim().is_empty(), "middle row is blank");
    assert!(plain(&rows[2]).contains("Two."));
}

#[test]
fn selecting_highlights_the_whole_row_in_one_consistent_colour() {
    // Two commands, the second highlighted. The selected row's name AND
    // description share the highlight colour (consistency); the other row
    // shares the dim colour. No caret/arrow.
    let lines = command_menu_lines(&palette("/", 1), 60);
    let name_fg = |l: &Line| l.spans[0].style.fg; // spans = [name, pad, desc]
    let desc_fg = |l: &Line| l.spans[2].style.fg;
    assert_eq!(
        name_fg(&lines[1]),
        Some(MENU_SELECTED_COLOR),
        "selected name"
    );
    assert_eq!(
        desc_fg(&lines[1]),
        name_fg(&lines[1]),
        "selected name matches its description colour"
    );
    assert_eq!(
        name_fg(&lines[0]),
        Some(MENU_DIM_COLOR),
        "other name dimmed"
    );
    assert_eq!(
        desc_fg(&lines[0]),
        name_fg(&lines[0]),
        "unselected name matches its description colour"
    );
    for line in &lines {
        assert!(!plain(line).contains('❯'), "no caret: {:?}", plain(line));
    }
}

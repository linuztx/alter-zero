//! The assistant markdown renderer: fenced code, headings, lists, quotes
//! (`docs/markdown.md`).

use super::*;
use crate::ui::assistant::assistant_lines;
use crate::ui::theme::{
    AI_BULLET, BULLET_WIDTH, CODE_TAB_WIDTH, THEMATIC_BREAK, ai_color, inline_code_color,
    menu_dim_color, menu_selected_color,
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
            .any(|(t, _, fg)| t == "c" && *fg == Some(inline_code_color()))
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
fn a_hard_wrapped_url_carries_its_whole_target_on_every_row() {
    // The reported bug (docs/links.md): in a narrow terminal the reply's URL
    // hard-breaks — `https://github.com/linuz` on one row, `tx.` on the next
    // — and clicking the first fragment opened the truncated URL. Every
    // rendered fragment must carry the FULL target through the whole
    // message pipeline (renderer, bullet/indent stamping, wrap).
    let url = "https://github.com/linuztx";
    let lines = message_lines(
        Role::Assistant,
        "created by linuztx · https://github.com/linuztx. That is the loop",
        26,
    );
    let fragments: Vec<(String, String)> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter_map(|s| {
            crate::links::style_link(&s.style).map(|u| (s.content.to_string(), u.to_string()))
        })
        .collect();
    assert!(
        fragments.len() >= 2,
        "the URL hard-breaks across rows at width 26: {lines:?}"
    );
    for (_, target) in &fragments {
        assert_eq!(target, url, "every fragment opens the whole URL");
    }
    let rejoined: String = fragments.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(rejoined, url, "the marked fragments are exactly the URL");
}

#[test]
fn a_wrapped_token_url_links_every_fragment_in_every_assistant_format() {
    // A synthetic token, never a real credential. The code/heading paths used
    // to bypass OSC 8, leaving only the terminal's first-row URL detection.
    let url = format!(
        "https://telemetry.example.test/?token={}",
        "0123456789abcdef".repeat(4)
    );
    for text in [
        format!("Dashboard:\n{url}"),
        format!("Dashboard:\n`{url}`"),
        format!("Dashboard:\n```text\n{url}\n```"),
        format!("Dashboard:\n```rust\nlet dashboard = \"{url}\";\n```"),
        format!("Dashboard:\n\n    {url}"),
        format!("## Dashboard {url}"),
        format!("- `{url}`"),
        format!("> `{url}`"),
        format!("| Dashboard |\n| --- |\n| `{url}` |"),
    ] {
        for width in [26, 80, 100] {
            let lines = message_lines(Role::Assistant, &text, width);
            let mut linked_rows = 0;
            let mut rejoined = String::new();
            let mut carrier = None;
            for line in &lines {
                let mut linked = false;
                for span in &line.spans {
                    if let Some(target) = crate::links::style_link(&span.style) {
                        assert_eq!(target.as_ref(), url, "full target: {text:?}");
                        if let Some(id) = carrier {
                            assert_eq!(span.style.underline_color, Some(id), "one hover group");
                        }
                        carrier = span.style.underline_color;
                        rejoined.push_str(&span.content);
                        linked = true;
                    }
                }
                linked_rows += usize::from(linked);
            }
            assert_eq!(
                rejoined, url,
                "every URL character links (w={width}): {text:?}"
            );
            assert!(
                linked_rows >= 2,
                "exercise a wrapped URL (w={width}): {text:?}"
            );
        }
    }
}

#[test]
fn quoted_code_urls_exclude_surrounding_syntax_and_keep_literal_punctuation() {
    let first = "https://one.example.test/?token=0123456789abcdef!";
    let second = "https://two.example.test/path?";
    for code in [
        format!("requests.get('{first}').json()"),
        format!("['{first}','{second}']"),
        format!("requests.get(\"{first}\").json()"),
        format!("[\"{first}\",\"{second}\"]"),
    ] {
        for text in [format!("`{code}`"), format!("```python\n{code}\n```")] {
            let rows = message_lines(Role::Assistant, &text, 26);
            let mut actual = Vec::<(String, String)>::new();
            for span in rows.iter().flat_map(|row| &row.spans) {
                if let Some(target) = crate::links::style_link(&span.style) {
                    if let Some((visible, previous)) = actual.last_mut()
                        && previous == target.as_ref()
                    {
                        visible.push_str(&span.content);
                    } else {
                        actual.push((span.content.to_string(), target.to_string()));
                    }
                }
            }
            let expected: Vec<_> = [first, second]
                .into_iter()
                .filter(|url| code.contains(url))
                .map(|url| (url.to_string(), url.to_string()))
                .collect();
            assert_eq!(actual, expected, "exact code URL targets: {text:?}");
        }
    }
}

#[test]
fn a_fenced_code_block_links_every_wrapped_url_line_separately() {
    let urls = (1..=12)
        .map(|n| {
            format!(
                "https://example.test/dashboard/{n}?token={:0>32}",
                n.to_string()
            )
        })
        .collect::<Vec<_>>();
    let text = format!("```text\n{}\n```", urls.join("\n"));
    let rows = message_lines(Role::Assistant, &text, 26);
    for url in &urls {
        let fragments: Vec<_> = rows
            .iter()
            .flat_map(|row| &row.spans)
            .filter_map(|span| {
                crate::links::style_link(&span.style)
                    .filter(|target| target.as_ref() == url)
                    .map(|_| span.content.as_ref())
            })
            .collect();
        assert_eq!(fragments.concat(), *url, "every character of {url} links");
        assert!(fragments.len() > 1, "the link wraps: {url}");
    }
    let linked: String = rows
        .iter()
        .flat_map(|row| &row.spans)
        .filter(|span| crate::links::style_link(&span.style).is_some())
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(linked, urls.concat(), "no syntax leaks into targets");
}

#[test]
fn heading_combining_marks_survive_the_painted_buffer() {
    use ratatui::widgets::{Paragraph, Widget};

    for text in [
        "## \u{0301}Hello",
        "## \u{0301}Hello https://example.test/x",
    ] {
        let lines = message_lines(Role::Assistant, text, 26);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 26, lines.len() as u16));
        Paragraph::new(lines).render(buffer.area, &mut buffer);
        assert_eq!(buffer[(4, 0)].symbol(), " \u{0301}", "{text:?}");
        assert!(buffer[(4, 0)].modifier.contains(Modifier::BOLD));
    }
}

// --- codex block layout: lists, quotes, block spacing (docs/markdown.md) ---

/// An assistant reply's rendered rows as plain strings.
fn rows(text: &str, width: u16) -> Vec<String> {
    message_lines(Role::Assistant, text, width)
        .iter()
        .map(plain)
        .collect()
}

#[test]
fn assistant_renders_bullet_and_ordered_lists() {
    // Every bullet renders codex's `•` whatever the source marker, ordered
    // items `N.`; a list of another type starts a new list, one blank line
    // apart (the old renderer echoed `- ` and kept the source indent).
    assert_eq!(
        rows("- first\n- second\n  - nested\n\n1. one\n2. two", 40),
        vec![
            "● • first",
            "  • second",
            "      • nested",
            "  ",
            "  1. one",
            "  2. two",
        ]
    );
    assert_eq!(rows("* star\n* star", 40), vec!["● • star", "  • star"]);
    assert_eq!(rows("+ plus", 40), vec!["● • plus"]);
}

#[test]
fn assistant_keeps_every_nesting_level_of_a_deep_list() {
    // Codex's geometry: four columns per level whatever the source indent,
    // so a third level (4+ spaces) still renders deeper than its parent.
    let expected = vec!["● • a", "      • b", "          • c", "              • d"];
    assert_eq!(rows("- a\n  - b\n    - c\n      - d", 60), expected);
    assert_eq!(
        rows("- a\n    - b\n        - c\n            - d", 60),
        expected,
        "four-space nesting lands on the same columns"
    );
}

#[test]
fn a_nested_item_after_a_blank_stays_a_list_not_indented_code() {
    // A loose list (blank between parent and children) nests at 4 spaces —
    // the indented-code rule must not swallow it, and a nested list hugs its
    // parent (no blank), as in codex. The ordered marker keeps its accent.
    let lines = message_lines(Role::Assistant, "- a\n\n    1. b", 60);
    let rendered: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(rendered, vec!["● • a", "      1. b"], "{rendered:?}");
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.as_ref() == "    1. "
                && s.style.fg == Some(crate::ui::theme::list_marker_color())),
        "the nested ordered marker keeps its list styling"
    );
}

#[test]
fn a_sub_list_indented_short_of_its_parents_text_still_nests() {
    // Models nest sub-bullets two spaces under `1. ` (two or three under
    // `10. `): short of the item's text column, so CommonMark — and codex —
    // flatten them into a separate list and break the numbering. Two columns
    // past the parent's marker is what the model meant: nest it.
    assert_eq!(
        rows(
            "1. **Step one**\n  - detail a\n  - detail b\n2. **Step two**\n  - detail c",
            40
        ),
        vec![
            "● 1. Step one",
            "      • detail a",
            "      • detail b",
            "  ",
            "  2. Step two",
            "      • detail c",
        ]
    );
    assert_eq!(
        rows(
            "9. Nine\n10. Ten\n   - sub of ten\n   - another\n11. Eleven",
            40
        ),
        vec![
            "● 9. Nine",
            "  10. Ten",
            "      • sub of ten",
            "      • another",
            "  ",
            "  11. Eleven",
        ]
    );
    assert_eq!(
        rows("9. Nine\n10. Ten\n  - two under ten\n11. Eleven", 40),
        vec![
            "● 9. Nine",
            "  10. Ten",
            "      • two under ten",
            "  ",
            "  11. Eleven",
        ]
    );
    // One column in is a sibling written sloppily, as CommonMark reads it:
    // nesting it would renumber the rest of the list.
    assert_eq!(
        rows("1. First\n 2. Second\n3. Third", 40),
        vec!["● 1. First", "  2. Second", "  3. Third"]
    );
    assert_eq!(
        rows("- parent\n - one space child\n- next", 40),
        vec!["● • parent", "  • one space child", "  • next"]
    );
}

#[test]
fn a_block_indented_short_of_an_items_text_stays_in_the_item() {
    // The same leniency for a fence, quote or heading opening between the
    // item's marker and its text — so `2. Run` stays item 2 of one list —
    // while a paragraph after a blank line must reach the text column.
    assert_eq!(
        rows("1. Install:\n  ```bash\n  npm i\n  ```\n2. Run", 40),
        vec!["● 1. Install:", "  ", "     npm i", "  ", "  2. Run"]
    );
    assert_eq!(
        rows("1. Item\n  > note\n2. Next", 40),
        vec!["● 1. Item", "     > note", "  ", "  2. Next"]
    );
    // The fence's code lines lose exactly the indent the fence line did, so
    // the code keeps its own indentation.
    assert_eq!(
        rows(
            "1. Install:\n  ```py\n  def f():\n      return 1\n  ```",
            40
        ),
        vec!["● 1. Install:", "  ", "     def f():", "         return 1"]
    );
    // An open table's later lines continue it at the table's indent, pipe
    // or none — a delimiter or row needn't open a block of its own.
    assert_eq!(
        rows("1. Compare:\n  | A | B |\n  ---|---\n  1 | 2\n2. Done", 40),
        vec![
            "● 1. Compare:",
            "     ┌───┬───┐",
            "     │ A │ B │",
            "     ├───┼───┤",
            "     │ 1 │ 2 │",
            "     └───┴───┘",
            "  ",
            "  2. Done",
        ]
    );
    assert_eq!(
        rows("1. Item\n\n  para", 40),
        vec!["● 1. Item", "  ", "  para"],
        "a paragraph after a blank needs the text column"
    );
}

#[test]
fn ordered_lists_renumber_from_their_start_like_codex() {
    assert_eq!(
        rows("1. a\n1. b\n1. c", 40),
        vec!["● 1. a", "  2. b", "  3. c"]
    );
    assert_eq!(rows("3. a\n4. b", 40), vec!["● 3. a", "  4. b"]);
    assert_eq!(rows("1) a\n2) b", 40), vec!["● 1. a", "  2. b"]);
    // A list nested under a bullet sits four columns in, its numbers
    // right-aligned there; the parent item spans three rows, so a blank
    // separates it from its next sibling (codex's AfterMultiline spacing).
    assert_eq!(
        rows("- a\n  1. x\n  2. y\n- b", 40),
        vec!["● • a", "      1. x", "      2. y", "  ", "  • b"]
    );
}

#[test]
fn task_list_items_render_codex_checkboxes() {
    assert_eq!(
        rows("- [ ] todo\n- [x] done\n- [X] caps\n- plain", 40),
        vec!["● ☐ todo", "  ☑ done", "  ☑ caps", "  • plain"]
    );
    assert_eq!(rows("1. [x] shipped", 40), vec!["● 1. ☑ shipped"]);
    assert_eq!(
        rows("- [x]", 40),
        vec!["● • [x]"],
        "a box with nothing after it is literal, as codex leaves it"
    );
    // What follows a box is the item's text, never another block marker.
    assert_eq!(rows("- [x] 10)", 40), vec!["● ☑ 10)"]);
    assert_eq!(
        rows("- [ ] # not a heading", 40),
        vec!["● ☐ # not a heading"]
    );
}

#[test]
fn a_wrapped_item_is_set_apart_from_its_next_sibling() {
    // Codex's AfterMultiline spacing: an item that renders on more than one
    // row is followed by a blank before its next sibling; one-row items stay
    // tight, and a source blank between items is dropped (loose lists are
    // compacted).
    assert_eq!(
        rows(
            "1. This item wraps onto another visible rendered line\n2. Next item",
            26
        ),
        vec![
            "● 1. This item wraps onto",
            "     another visible",
            "     rendered line",
            "  ",
            "  2. Next item",
        ]
    );
    assert_eq!(
        rows("- a\n\n- b\n\n- c", 40),
        vec!["● • a", "  • b", "  • c"]
    );
}

#[test]
fn blocks_are_separated_by_exactly_one_blank_line() {
    // Codex puts one blank line between blocks whatever the source had: a
    // heading's next block, a paragraph's list, a fence after prose — and
    // runs of blank lines collapse to one. A soft break stays a line break.
    assert_eq!(
        rows("## Title\nText", 40),
        vec!["● ## Title", "  ", "  Text"]
    );
    assert_eq!(rows("### H\n- a", 40), vec!["● ### H", "  ", "  • a"]);
    assert_eq!(
        rows("Intro:\n- a\n- b", 40),
        vec!["● Intro:", "  ", "  • a", "  • b"]
    );
    assert_eq!(rows("a\n\n\n\nb", 40), vec!["● a", "  ", "  b"]);
    assert_eq!(rows("a\nb", 40), vec!["● a", "  b"]);
    assert_eq!(
        rows("Run:\n```sh\nls\n```\nDone.", 40),
        vec!["● Run:", "  ", "  ls", "  ", "  Done."]
    );
    assert_eq!(
        rows("- a\n## Next", 40),
        vec!["● • a", "  ", "  ## Next"],
        "a heading ends the list"
    );
}

#[test]
fn a_dash_rule_right_after_a_paragraph_renders_as_a_rule() {
    // `text\n---` is a setext underline in CommonMark, but in a model's reply
    // it is a separator that lost its blank line: render the rule rather than
    // leave `---` showing.
    assert_eq!(
        rows("Text\n---\nMore", 40),
        vec!["● Text", "  ", "  ———", "  ", "  More"]
    );
}

#[test]
fn text_under_a_list_item_hangs_under_its_text() {
    // Continuation lines — indented, or lazy (unindented, no blank before) —
    // belong to the item and hang under its text, like codex's soft break.
    assert_eq!(
        rows("- item\n  more text", 40),
        vec!["● • item", "    more text"]
    );
    assert_eq!(rows("- item\nlazy", 40), vec!["● • item", "    lazy"]);
    // A second paragraph in the item keeps its blank; the item now spans
    // three rows, so its sibling is set apart too.
    assert_eq!(
        rows("- a\n\n  para\n- b", 40),
        vec!["● • a", "  ", "    para", "  ", "  • b"]
    );
}

#[test]
fn an_indented_paragraph_in_a_list_is_prose_not_code() {
    // Four spaces under `1. ` after a blank is the item's next paragraph —
    // the old top-level indented-code rule rendered it as code.
    let lines = message_lines(Role::Assistant, "1. **Step**\n\n    Explanation here.", 40);
    let rendered: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(rendered, vec!["● 1. Step", "  ", "     Explanation here."]);
    let span = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("Explanation"))
        .expect("the paragraph text");
    assert_eq!(span.style.fg, None, "prose, not the code style");
}

#[test]
fn a_fence_inside_a_list_item_renders_under_its_text() {
    // A fence nested four or more spaces deep (under a nested item) is still
    // a fence — the old renderer showed its backticks — and its code hangs
    // under the item's text.
    assert_eq!(
        rows("- a\n  - b\n\n    ```sh\n    ls\n    ```", 40),
        vec!["● • a", "      • b", "  ", "        ls"]
    );
    assert_eq!(
        rows(
            "1. Install:\n   ```bash\n   cargo install foo\n   ```\n2. Run",
            40
        ),
        vec![
            "● 1. Install:",
            "  ",
            "     cargo install foo",
            "  ",
            "  2. Run",
        ]
    );
}

#[test]
fn a_fence_strips_its_own_indent_from_its_code_lines() {
    // CommonMark: a fence indented N spaces takes up to N spaces off each
    // code line, so the code sits where the fence does — at the margin for
    // an indented fence on its own, at the item's text for one under a list
    // item (the code used to sit those N columns right of it).
    assert_eq!(
        rows("  ```python\n  def f():\n      return 1\n  ```", 40),
        vec!["● def f():", "      return 1"]
    );
    assert_eq!(
        rows(
            "- Install:\n    ```bash\n    npm install\n    ```\n- Next",
            40
        ),
        vec!["● • Install:", "  ", "    npm install", "  ", "  • Next"]
    );
    assert_eq!(
        rows("1. Build:\n    ```sh\n    cargo build\n    ```\n2. Run", 40),
        vec!["● 1. Build:", "  ", "     cargo build", "  ", "  2. Run"]
    );
    // A code line indented less than its fence keeps what it has.
    assert_eq!(rows("   ```\nflush\n   ```", 40), vec!["● flush"]);
}

#[test]
fn assistant_renders_blockquote() {
    assert_eq!(rows("> quoted text", 40), vec!["● > quoted text"]);
}

#[test]
fn blockquotes_hold_lists_headings_and_paragraphs() {
    // A quote's content is markdown too: `> - a` is a bullet inside the quote
    // (the old renderer showed `> - a`), a heading keeps its style, and a
    // blank `>` line separates paragraphs inside it.
    assert_eq!(rows("> - a\n> - b", 40), vec!["● > • a", "  > • b"]);
    assert_eq!(
        rows("> ## Note\n> body", 40),
        vec!["● > ## Note", "  > ", "  > body"]
    );
    assert_eq!(rows("> a\n>\n> b", 40), vec!["● > a", "  > ", "  > b"]);
    assert_eq!(rows("> q\n\nafter", 40), vec!["● > q", "  ", "  after"]);
    assert_eq!(
        rows("> a\nlazy", 40),
        vec!["● > a", "  > lazy"],
        "a lazy line continues the quoted paragraph"
    );
    assert_eq!(
        rows("- item\n  > quoted", 40),
        vec!["● • item", "    > quoted"],
        "a quote inside an item hangs under its text"
    );
}

#[test]
fn a_tight_item_keeps_a_quote_heading_or_table_against_its_text() {
    // Codex sets a block apart when a paragraph *ends*, and an item's own
    // text in a tight list never ends one — so a quote, heading or table
    // right under it hugs it (codex's `blockquote_list_then_nested_blockquote`)
    // while code and rules still sit a blank row off. A blank line between
    // makes the item loose and the gap comes back.
    assert_eq!(
        rows("- item one\n  > quoted under it\n- item two", 40),
        vec![
            "● • item one",
            "    > quoted under it",
            "  ",
            "  • item two"
        ]
    );
    assert_eq!(
        rows("> - parent\n>   > child", 40),
        vec!["● > • parent", "  >   > child"]
    );
    assert_eq!(
        rows("- item one\n  ### A heading inside\n- item two", 40),
        vec![
            "● • item one",
            "    ### A heading inside",
            "  ",
            "  • item two"
        ]
    );
    assert_eq!(
        rows(
            "1. Compare:\n   | A | B |\n   |---|---|\n   | 1 | 2 |\n2. Done",
            40
        ),
        vec![
            "● 1. Compare:",
            "     ┌───┬───┐",
            "     │ A │ B │",
            "     ├───┼───┤",
            "     │ 1 │ 2 │",
            "     └───┴───┘",
            "  ",
            "  2. Done",
        ]
    );
    assert_eq!(
        rows("- item\n\n  > quoted after a blank", 40),
        vec!["● • item", "  ", "    > quoted after a blank"],
        "a blank line makes the item loose"
    );
    assert_eq!(
        rows("- a\n  - b\n  > after the nested list", 40),
        vec!["● • a", "      • b", "  ", "    > after the nested list"],
        "only the item's own text hugs: a nested list ends a block"
    );
    assert_eq!(
        rows("- a\n  ---", 40),
        vec!["● • a", "  ", "    ———"],
        "a rule keeps its gap"
    );
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
            .any(|s| s.style.fg == highlight::plain_style(highlight::CodeTheme::default()).fg),
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
fn heading_inline_markdown_drops_its_markers_like_codex() {
    // Models wrap whole headings in emphasis (`## **Title**`). Codex parses
    // a heading's text as inline markdown, so the `**` is consumed and only
    // the `#`s stay — a heading that showed `## **Top 10…**` was the
    // reported bug. The level's dress still applies, composed with the
    // inline styles.
    let h2 = message_lines(Role::Assistant, "## **Top 10 PHP Frameworks in 2026**", 80);
    assert_eq!(plain(&h2[0]), "● ## Top 10 PHP Frameworks in 2026");

    let h3 = message_lines(Role::Assistant, "### **1. Laravel**", 80);
    assert_eq!(plain(&h3[0]), "● ### 1. Laravel");
    let laravel = h3[0]
        .spans
        .iter()
        .find(|s| s.content.contains("Laravel"))
        .expect("heading text span");
    assert!(
        laravel
            .style
            .add_modifier
            .contains(Modifier::BOLD | Modifier::ITALIC),
        "h3 keeps bold+italic under the consumed `**`"
    );

    let h4 = message_lines(Role::Assistant, "#### Run `cargo test` *first*", 80);
    assert_eq!(plain(&h4[0]), "● #### Run cargo test first");
    let code = h4[0]
        .spans
        .iter()
        .find(|s| s.content.contains("cargo"))
        .expect("code span");
    assert_eq!(
        code.style.fg,
        Some(inline_code_color()),
        "code keeps its colour"
    );
    assert!(
        code.style.add_modifier.contains(Modifier::ITALIC),
        "and the h4 dress"
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
    let with: Vec<String> = assistant_lines("Building it.\n\n", 80, AI_BULLET, ai_color())
        .iter()
        .map(plain)
        .collect();
    let without: Vec<String> = assistant_lines("Building it.", 80, AI_BULLET, ai_color())
        .iter()
        .map(plain)
        .collect();
    assert_eq!(with, without, "trailing blank rows are trimmed");
}

#[test]
fn assistant_lines_keeps_interior_blank_lines() {
    // Only *trailing* blanks are trimmed — a paragraph break in the middle
    // stays (it separates two paragraphs).
    let rows = assistant_lines("One.\n\nTwo.", 80, AI_BULLET, ai_color());
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
        Some(menu_selected_color()),
        "selected name"
    );
    assert_eq!(
        desc_fg(&lines[1]),
        name_fg(&lines[1]),
        "selected name matches its description colour"
    );
    assert_eq!(
        name_fg(&lines[0]),
        Some(menu_dim_color()),
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

#[test]
fn a_code_labelled_link_marks_only_the_two_names() {
    // The reported render: `` "[`AGENTS.md`](AGENTS.md)" `` shows
    // `"AGENTS.md (AGENTS.md)"` — the label keeps its inline-code colour, the
    // shown target the link dress, and the quotes and parens around them stay
    // plain prose. Both names carry the verbatim target; nothing else does,
    // at every width, the hard-breaking one included.
    let source = "\"[`AGENTS.md`](AGENTS.md)\"";
    for width in [8, 26, 80] {
        let lines = message_lines(Role::Assistant, source, width);
        let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
        let mut linked = String::new();
        let mut parens = String::new();
        for span in &spans {
            if let Some(target) = crate::links::style_link(&span.style) {
                assert_eq!(target.as_ref(), "AGENTS.md", "at width {width}");
                linked.push_str(&span.content);
            }
            if span.content.contains(['(', ')']) {
                parens.extend(span.content.chars().filter(|c| matches!(c, '(' | ')')));
                assert_eq!(span.style, Style::default(), "prose at {width}: {span:?}");
            }
        }
        assert_eq!(linked, "AGENTS.mdAGENTS.md", "both names link at {width}");
        assert_eq!(parens, "()", "exactly one pair of parens at {width}");
        // The label keeps its own inline-code dress — a hard break splits it
        // into fragments, so read the colour off the run, not one span.
        let labelled: String = spans
            .iter()
            .filter(|s| s.style.fg == Some(inline_code_color()))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(
            labelled, "AGENTS.md",
            "the code label at {width}: {spans:?}"
        );
    }
    // Wide enough not to hard-break, the row reads exactly as before the
    // parens lost the link dress — this fix moves no character.
    let joined: String = message_lines(Role::Assistant, source, 80)
        .iter()
        .flat_map(|line| &line.spans)
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        joined.ends_with("\"AGENTS.md (AGENTS.md)\""),
        "visible text unchanged: {joined:?}"
    );
}

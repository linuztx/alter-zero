//! Pure markdown structure for assistant replies (`docs/markdown.md`).
//!
//! The model streams markdown; the picker/scrollback renderer ([`crate::ui`])
//! needs to treat **fenced code blocks** differently from prose — code must keep
//! its indentation verbatim (no word-wrap, no whitespace collapse) while prose
//! word-wraps. This module is the pure, unit-tested split: [`parse_blocks`]
//! walks the text once into [`Block`]s (prose runs vs code blocks), and
//! [`heading_level`] classifies a single prose line as an ATX heading.
//!
//! **Prefix-stable by construction.** A line's block membership is a pure
//! function of the lines *before* it (a left-to-right scan of fence state, no
//! lookahead). Appending text never reclassifies an earlier line — which is what
//! lets [`crate::ui::stable_commit`] keep flushing completed lines to scrollback
//! as a reply streams (CLAUDE.md invariant 2). An **unterminated** fence still
//! renders as code, identically to a closed one, so nothing changes when the
//! closing fence finally arrives.

/// One structural block of an assistant reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A run of consecutive non-code source lines (may contain `'\n'`), wrapped
    /// as prose by the renderer.
    Prose(String),
    /// A fenced code block: the info-string language (first token, if any) and
    /// the verbatim source lines between the fences (the fences themselves are
    /// dropped).
    Code {
        /// The fence language (`` ```rust `` → `Some("rust")`, bare `` ``` `` →
        /// `None`).
        lang: Option<String>,
        /// The code's source lines, kept byte-for-byte (indentation intact).
        lines: Vec<String>,
    },
}

/// Split assistant `text` into prose and fenced-code [`Block`]s.
///
/// A line opens/closes a fence when — after ≤3 leading spaces — it begins with
/// 3+ `` ` `` or `~`. The closing fence must match the opening char, be at least
/// as long, and carry no info string. An unterminated fence at end-of-text still
/// yields a trailing [`Block::Code`] (see the module docs on prefix-stability).
#[must_use]
pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut code: Vec<String> = Vec::new();
    let mut lang: Option<String> = None;
    let mut open: Option<(char, usize)> = None; // (fence char, fence length)

    let flush_prose = |prose: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !prose.is_empty() {
            blocks.push(Block::Prose(prose.join("\n")));
            prose.clear();
        }
    };

    for line in text.split('\n') {
        match open {
            None => {
                if let Some((ch, len, info)) = fence_marker(line) {
                    flush_prose(&mut prose, &mut blocks);
                    open = Some((ch, len));
                    lang = fence_lang(info);
                } else {
                    prose.push(line);
                }
            }
            Some((ch, len)) => {
                // A closing fence matches the opening char, is at least as long,
                // and carries no info string; otherwise it's a code line.
                let closes = matches!(
                    fence_marker(line),
                    Some((c, l, info)) if c == ch && l >= len && info.trim().is_empty()
                );
                if closes {
                    blocks.push(Block::Code {
                        lang: lang.take(),
                        lines: std::mem::take(&mut code),
                    });
                    open = None;
                } else {
                    code.push(line.to_string());
                }
            }
        }
    }
    // End of text: flush whatever is open. An unterminated fence still renders as
    // a code block (prefix-stable — see the module docs).
    if open.is_some() {
        blocks.push(Block::Code {
            lang: lang.take(),
            lines: code,
        });
    } else {
        flush_prose(&mut prose, &mut blocks);
    }
    blocks
}

/// If `line` is a fence delimiter — after ≤3 leading spaces, 3+ of `` ` `` or
/// `~` — return `(marker char, run length, info string)`.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None; // 4+ spaces of indent is indented code, not a fence
    }
    for marker in ['`', '~'] {
        let len = trimmed.chars().take_while(|&c| c == marker).count();
        if len >= 3 {
            return Some((marker, len, &trimmed[len..]));
        }
    }
    None
}

/// The language of a fence info string: the first token before a comma/space/tab
/// (`"rust,no_run"` → `"rust"`, `"rust title=x"` → `"rust"`), or `None` when
/// empty.
#[must_use]
pub fn fence_lang(info: &str) -> Option<String> {
    info.trim()
        .split([',', ' ', '\t'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Classify a single (prose) line as an ATX heading: `Some((level, text))` where
/// `text` is the content with the leading `#`s and following spaces stripped, or
/// `None` when the line isn't a heading (no `#`, no space after the `#`s, >6
/// `#`s, or >3 leading spaces of indent).
#[must_use]
pub fn heading_level(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None; // 4+ spaces of indent is indented code
    }
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &trimmed[hashes..];
    // ATX headings need a space after the `#`s (a bare `###` is an empty heading).
    if after.is_empty() {
        return Some((hashes as u8, ""));
    }
    if !after.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, after.trim_start_matches(' ').trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(lang: Option<&str>, lines: &[&str]) -> Block {
        Block::Code {
            lang: lang.map(str::to_string),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn fence_free_text_is_one_prose_block() {
        assert_eq!(
            parse_blocks("hello world\nsecond line"),
            vec![Block::Prose("hello world\nsecond line".into())]
        );
    }

    #[test]
    fn a_fenced_block_splits_prose_around_it() {
        let text = "before\n```rust\nfn main() {}\n```\nafter";
        assert_eq!(
            parse_blocks(text),
            vec![
                Block::Prose("before".into()),
                code(Some("rust"), &["fn main() {}"]),
                Block::Prose("after".into()),
            ]
        );
    }

    #[test]
    fn code_indentation_is_preserved_byte_for_byte() {
        let text = "```py\ndef f():\n    return 1\n```";
        assert_eq!(
            parse_blocks(text),
            vec![code(Some("py"), &["def f():", "    return 1"])]
        );
    }

    #[test]
    fn blank_lines_inside_code_are_kept() {
        let text = "```\na\n\nb\n```";
        assert_eq!(parse_blocks(text), vec![code(None, &["a", "", "b"])]);
    }

    #[test]
    fn an_unterminated_fence_still_yields_a_code_block() {
        let text = "intro\n```python\nx = 1";
        assert_eq!(
            parse_blocks(text),
            vec![
                Block::Prose("intro".into()),
                code(Some("python"), &["x = 1"]),
            ]
        );
    }

    #[test]
    fn tilde_fences_and_indented_fences_are_recognized() {
        assert_eq!(parse_blocks("~~~\na\n~~~"), vec![code(None, &["a"])]);
        // Up to three leading spaces still open a fence.
        assert_eq!(parse_blocks("   ```\na\n   ```"), vec![code(None, &["a"])]);
    }

    #[test]
    fn a_backtick_line_inside_a_tilde_block_is_not_a_close() {
        // The closing fence must match the opening marker family.
        let text = "~~~\n```\ncode\n~~~";
        assert_eq!(parse_blocks(text), vec![code(None, &["```", "code"])]);
    }

    #[test]
    fn empty_prose_runs_are_omitted() {
        // Text that is exactly a code block yields only the Code block.
        assert_eq!(parse_blocks("```\na\n```"), vec![code(None, &["a"])]);
    }

    #[test]
    fn fence_lang_takes_the_first_token() {
        assert_eq!(fence_lang("rust"), Some("rust".into()));
        assert_eq!(fence_lang("rust,no_run"), Some("rust".into()));
        assert_eq!(fence_lang("rust title=demo"), Some("rust".into()));
        assert_eq!(fence_lang("python3"), Some("python3".into()));
        assert_eq!(fence_lang(""), None);
        assert_eq!(fence_lang("   "), None);
    }

    #[test]
    fn heading_levels_strip_the_markers() {
        assert_eq!(heading_level("# Title"), Some((1, "Title")));
        assert_eq!(heading_level("### The Code"), Some((3, "The Code")));
        assert_eq!(heading_level("###### Deep"), Some((6, "Deep")));
    }

    #[test]
    fn not_a_heading_without_a_space_or_too_many_hashes() {
        assert_eq!(heading_level("###no space"), None);
        assert_eq!(heading_level("####### too deep"), None);
        assert_eq!(heading_level("plain text"), None);
        assert_eq!(heading_level("`# in code`"), None);
    }

    #[test]
    fn heading_allows_up_to_three_leading_spaces() {
        assert_eq!(heading_level("  ## Indented"), Some((2, "Indented")));
        // Four leading spaces is indented code, not a heading.
        assert_eq!(heading_level("    # Not a heading"), None);
    }

    #[test]
    fn prefix_stability_no_committed_line_ever_changes() {
        // Faithfully model `ui::stable_commit`: as the reply streams, each render
        // commits all-but-the-last flattened line, tracking a monotonic
        // high-water `committed` count. This test proves a line, once committed
        // to scrollback, is NEVER rewritten — even as the opening fence, code
        // lines, and closing fence stream in one char at a time.
        let full = "intro\n```python\ndef f():\n    return 1\n```\ndone";
        // A block-flattened view of the reply — (is_code, line) — the proxy for
        // one output line each (headings/wrapping don't change the argument).
        let flatten = |t: &str| -> Vec<(bool, String)> {
            parse_blocks(t)
                .into_iter()
                .flat_map(|b| match b {
                    Block::Prose(s) => s
                        .split('\n')
                        .map(|l| (false, l.to_string()))
                        .collect::<Vec<_>>(),
                    Block::Code { lines, .. } => lines.into_iter().map(|l| (true, l)).collect(),
                })
                .collect()
        };
        let mut committed: Vec<(bool, String)> = Vec::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let lines = flatten(&full[..end]);
            let stable = lines.len().saturating_sub(1); // withhold the last line
            for (i, line) in lines.iter().enumerate().take(stable) {
                match committed.get(i) {
                    Some(frozen) => assert_eq!(frozen, line, "committed line {i} changed"),
                    None => committed.push(line.clone()),
                }
            }
        }
    }
}

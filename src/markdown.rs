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

/// Split assistant `text` into prose, fenced-code, and **indented-code**
/// [`Block`]s.
///
/// A line opens/closes a fence when — after ≤3 leading spaces — it begins with
/// 3+ `` ` `` or `~`. The closing fence must match the opening char, be at least
/// as long, and carry no info string. An unterminated fence at end-of-text still
/// yields a trailing [`Block::Code`] (see the module docs on prefix-stability).
///
/// A run of lines each indented ≥4 spaces (or a leading tab) is a CommonMark
/// **indented code block** (`lang: None`), but only when it does *not* interrupt
/// a paragraph — it must be preceded by a blank line or start-of-text
/// (`prev_blank`). Blank lines inside the run stay part of it; the first
/// non-blank, non-indented line ends it. This mirrors codex's
/// `CodeBlockKind::Indented` (which strips the 4-space marker then re-adds a
/// 4-space prefix — net: the source indentation, which we keep verbatim).
#[must_use]
pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut code: Vec<String> = Vec::new();
    let mut lang: Option<String> = None;
    let mut open: Option<(char, usize)> = None; // fenced: (fence char, fence length)
    let mut in_indented = false; // inside an indented (4-space) code block
    let mut prev_blank = true; // start-of-text is a blank boundary

    let flush_prose = |prose: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !prose.is_empty() {
            blocks.push(Block::Prose(prose.join("\n")));
            prose.clear();
        }
    };

    for line in text.split('\n') {
        // Inside a fenced block: everything is code until the matching close.
        if let Some((ch, len)) = open {
            if is_closing_fence(line, ch, len) {
                blocks.push(Block::Code {
                    lang: lang.take(),
                    lines: std::mem::take(&mut code),
                });
                open = None;
            } else {
                code.push(line.to_string());
            }
            prev_blank = false;
            continue;
        }
        let blank = line.trim().is_empty();
        // Inside an indented block: blank + still-indented lines stay code; the
        // first non-blank, non-indented line ends it (and is reclassified below).
        if in_indented {
            if blank || is_indented_code_line(line) {
                code.push(line.to_string());
                prev_blank = blank;
                continue;
            }
            blocks.push(Block::Code {
                lang: None,
                lines: std::mem::take(&mut code),
            });
            in_indented = false;
        }
        if let Some((ch, len, info)) = fence_marker(line) {
            flush_prose(&mut prose, &mut blocks);
            open = Some((ch, len));
            lang = fence_lang(info);
            prev_blank = false;
        } else if !blank && prev_blank && is_indented_code_line(line) {
            // An indented code block starts only after a blank/at start-of-text
            // (it cannot interrupt a paragraph).
            flush_prose(&mut prose, &mut blocks);
            in_indented = true;
            code.push(line.to_string());
            prev_blank = false;
        } else {
            prose.push(line);
            prev_blank = blank;
        }
    }
    // End of text: flush whatever is open. An unterminated fence — or a trailing
    // indented block — still renders as code (prefix-stable, see the module docs).
    if open.is_some() {
        blocks.push(Block::Code {
            lang: lang.take(),
            lines: code,
        });
    } else if in_indented {
        blocks.push(Block::Code {
            lang: None,
            lines: code,
        });
    } else {
        flush_prose(&mut prose, &mut blocks);
    }
    blocks
}

/// How the [`BlockScanner`] classifies one source line as a reply streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    /// A non-code line (prose or heading — the renderer decides which).
    Prose,
    /// An opening fence: the block's info-string language (first token, used
    /// only to prime syntax highlighting). The renderer emits no row — the fence
    /// line and its language label are hidden.
    CodeStart(Option<String>),
    /// A verbatim code line — inside an open fence, or inside an **indented**
    /// (4-space) code block (the renderer highlights fenced code by language and
    /// indented code as plain text).
    Code,
    /// A closing fence — rendered as nothing (the fence is hidden).
    CodeEnd,
}

/// The **incremental** counterpart of [`parse_blocks`]: a left-to-right fence
/// state machine that classifies one source line at a time. Feeding the same
/// lines in order yields exactly the prose/code split [`parse_blocks`] produces,
/// but without re-scanning the whole reply — which is what lets the streaming
/// renderer stay O(new line) per chunk (`docs/markdown.md`).
///
/// Prefix-stable: a line's [`LineKind`] depends only on the lines before it, so a
/// classification, once made, never changes.
///
/// `Clone` lets the streaming renderer peek the classification of an in-progress
/// line without advancing the fence state it will resume from.
#[derive(Debug, Clone)]
pub struct BlockScanner {
    /// The open fence's `(marker char, run length)`, or `None` outside a block.
    open: Option<(char, usize)>,
    /// Whether the scanner is inside an **indented** (4-space) code block.
    in_indented: bool,
    /// Whether the previous line was blank (or this is start-of-text) — the gate
    /// that lets an indented line *start* a code block without interrupting a
    /// paragraph. Mirrors [`parse_blocks`].
    prev_blank: bool,
}

impl Default for BlockScanner {
    fn default() -> Self {
        // `prev_blank` starts true: before the first line the scanner sits at a
        // block boundary, so a leading indented line opens an indented code block.
        Self {
            open: None,
            in_indented: false,
            prev_blank: true,
        }
    }
}

impl BlockScanner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify the next source `line`, advancing the fence/indent state. The
    /// verdict for a line depends only on the lines before it (a left-to-right
    /// scan), so it never changes once made — the property scrollback commits
    /// rely on. Agrees with [`parse_blocks`] line for line.
    pub fn classify(&mut self, line: &str) -> LineKind {
        // Inside a fenced block: code until the matching close.
        if let Some((ch, len)) = self.open {
            self.prev_blank = false;
            return if is_closing_fence(line, ch, len) {
                self.open = None;
                LineKind::CodeEnd
            } else {
                LineKind::Code
            };
        }
        let blank = line.trim().is_empty();
        // Inside an indented block: blank + still-indented lines stay code.
        if self.in_indented {
            if blank || is_indented_code_line(line) {
                self.prev_blank = blank;
                return LineKind::Code;
            }
            self.in_indented = false; // ended — reclassify this line below
        }
        if let Some((ch, len, info)) = fence_marker(line) {
            self.open = Some((ch, len));
            self.prev_blank = false;
            LineKind::CodeStart(fence_lang(info))
        } else if !blank && self.prev_blank && is_indented_code_line(line) {
            // Start an indented code block (its first line is code content, so
            // there is no separate `CodeStart` — the renderer highlights it plain).
            self.in_indented = true;
            self.prev_blank = false;
            LineKind::Code
        } else {
            self.prev_blank = blank;
            LineKind::Prose
        }
    }
}

/// Whether `line` closes an open fence of char `ch` and length `len` — the same
/// family, at least as long, and no info string.
fn is_closing_fence(line: &str, ch: char, len: usize) -> bool {
    matches!(
        fence_marker(line),
        Some((c, l, info)) if c == ch && l >= len && info.trim().is_empty()
    )
}

/// Whether `line` is deep enough to be a CommonMark **indented code** line — it
/// begins with 4 spaces or a leading tab (a tab counts as ≥4 columns). Whether it
/// actually *is* code also depends on context (not interrupting a paragraph); the
/// caller ([`parse_blocks`] / [`BlockScanner`]) applies that `prev_blank` gate.
fn is_indented_code_line(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

/// If `line` is a CommonMark **thematic break** (horizontal rule) — after ≤3
/// leading spaces, a run of ≥3 of the *same* marker among `-`, `*`, `_`,
/// optionally separated by spaces/tabs, and nothing else — return the marker
/// char. The marker matters to the caller: a `-` rule is ambiguous with a setext
/// `H2` underline, so the renderer only honours it after a blank line, whereas
/// `*`/`_` are unambiguous. Codex renders any of these as a `———` em-dash rule
/// (`markdown_render.rs`'s `Event::Rule`).
#[must_use]
pub fn thematic_break(line: &str) -> Option<char> {
    thematic_marker_run(line).and_then(|(marker, count)| (count >= 3).then_some(marker))
}

/// The single-family marker run of a would-be thematic break: after ≤3 leading
/// spaces (a leading tab is ≥4 columns — indented code, never a rule), a line
/// holding only one of `-`/`*`/`_` plus spaces/tabs — returned as
/// `(marker, count)`. `None` when any other char, a mixed family, or nothing
/// but whitespace appears. The one scan [`thematic_break`] (≥3 markers) and
/// [`is_partial_thematic_break`] (1–2 so far) both read.
fn thematic_marker_run(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 || trimmed.starts_with('\t') {
        return None;
    }
    let marker = trimmed.chars().find(|c| !matches!(c, ' ' | '\t'))?;
    if !matches!(marker, '-' | '*' | '_') {
        return None;
    }
    let mut count = 0usize;
    for c in trimmed.chars() {
        if c == marker {
            count += 1;
        } else if c != ' ' && c != '\t' {
            return None;
        }
    }
    Some((marker, count))
}

/// Whether `line` is a *partial* thematic-break run — after ≤3 leading spaces,
/// only marker chars of a single family (`-`/`*`/`_`) and spaces/tabs, with
/// **1–2** markers so far, so appending another marker could flip it to a `———`
/// rule. The streaming renderer withholds such a trailing line whole — exactly
/// like [`is_partial_fence`] — so a narrow-width wrap can't commit a prose row
/// the completed rule then rewrites (the differential test covers width 3 up).
/// Deliberately conservative: it also covers list-marker starts like `- ` (which
/// settle to prose the instant a non-marker char arrives), whose brief withholding
/// is harmless — [`crate::ui::StreamRender::finish`] flushes them.
#[must_use]
pub fn is_partial_thematic_break(line: &str) -> bool {
    // 1–2 markers and nothing else → a third marker would open a rule.
    thematic_marker_run(line).is_some_and(|(_, count)| (1..3).contains(&count))
}

/// Whether `line` is a *partial* ATX heading — after ≤3 leading spaces, a bare
/// run of 1–6 `#` and nothing else. Such a line is already a (level-N, empty)
/// heading, but its **style is not settled**: another `#` deepens the level (a
/// different modifier set) and a 7th flips it to prose — so at a width
/// narrower than the run, its wrapped rows must not reach scrollback yet
/// (exactly the [`is_partial_fence`] situation). A space or text after the
/// `#`s settles the level; any other leading char was never a heading.
#[must_use]
pub fn is_partial_heading(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false; // 4+ spaces of indent is indented code, never a heading
    }
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes) && hashes == trimmed.len()
}

/// If `line` is a fence delimiter — after ≤3 leading spaces, 3+ of `` ` `` or
/// `~` — return `(marker char, run length, info string)`. CommonMark: the info
/// string of a *backtick* fence may not contain a backtick (such a line is
/// inline code, not a fence — treating it as an opener would swallow the rest
/// of the reply as an unterminated block); a tilde fence's info string may.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None; // 4+ spaces of indent is indented code, not a fence
    }
    for marker in ['`', '~'] {
        let len = trimmed.chars().take_while(|&c| c == marker).count();
        if len >= 3 {
            let info = &trimmed[len..];
            if marker == '`' && info.contains('`') {
                return None;
            }
            return Some((marker, len, info));
        }
    }
    None
}

/// Whether `line` is a *partial* fence opener — after ≤3 leading spaces, a bare
/// run of **one or two** `` ` `` or `~` and nothing else — so streaming a third
/// marker would flip it from prose to a [`LineKind::CodeStart`], collapsing its
/// wrapped prose row(s) into a single dim label row.
///
/// This is the one non-code line whose block classification is *not yet settled*:
/// the streaming renderer must withhold such a trailing line whole (like an
/// in-code line), or at a narrow width — where the 1–2 marker chars wrap into ≥2
/// rows — it could commit a prose row that the third marker then rewrites,
/// breaking the immutable-scrollback invariant.
#[must_use]
pub fn is_partial_fence(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false; // 4+ spaces of indent is indented code, never a fence
    }
    for marker in ['`', '~'] {
        let run = trimmed.chars().take_while(|&c| c == marker).count();
        // 1–2 markers *and* nothing after them: a third marker would open a fence.
        if (1..3).contains(&run) && run == trimmed.chars().count() {
            return true;
        }
    }
    false
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

// --- GFM pipe tables (`docs/markdown.md`). A table is a header row, a
// **delimiter row** (`|---|:-:|`), and zero+ data rows — all pipe-delimited.
// Detection needs one line of lookahead (a header is only a header if the next
// line is a delimiter), and a new row can widen an already-emitted column, so
// tables are **not** prefix-stable: the renderer buffers a table block whole and
// commits it only once it closes ([`crate::ui::AssistantRenderer`]). These pure
// helpers do the row/delimiter parsing. ---

/// Per-column text alignment a table's delimiter row declares (GFM): `:--` left,
/// `:-:` center, `--:` right, `---` unspecified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// No colon — the renderer left-aligns.
    None,
    /// `:---` — explicitly left.
    Left,
    /// `:--:` — centered.
    Center,
    /// `---:` — right-aligned.
    Right,
}

/// The line with its ≤3 leading spaces stripped, or `None` when it is indented
/// ≥4 columns (a leading tab or 4 spaces is indented code, never a table) — the
/// shared indent gate for every table helper here, mirroring [`fence_marker`].
fn table_indent(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 || trimmed.starts_with('\t') {
        return None;
    }
    Some(trimmed.trim_end())
}

/// Whether `s` holds a `|` that is **not** backslash-escaped — the cell separator
/// that marks a candidate table row.
fn has_unescaped_pipe(s: &str) -> bool {
    let mut esc = false;
    for ch in s.chars() {
        if esc {
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == '|' {
            return true;
        }
    }
    false
}

/// Whether `line` is a *candidate* table row — after ≤3 leading spaces, it holds
/// an unescaped `|`. This is only a candidate: a header row becomes a real table
/// only when the **next** line is a [`table_delimiter`] (the one-line lookahead
/// the renderer resolves by buffering — see `docs/markdown.md`).
#[must_use]
pub fn is_table_row(line: &str) -> bool {
    table_indent(line).is_some_and(has_unescaped_pipe)
}

/// Split a table row into its **trimmed, unescaped** cell texts. Optional leading
/// and trailing pipes are dropped; interior empty cells (`a || b`) are kept; a
/// `\|` is an escaped literal pipe inside a cell, not a separator. Returns empty
/// for an indented-code line (never a table row).
#[must_use]
pub fn table_cells(line: &str) -> Vec<String> {
    match table_indent(line) {
        Some(body) => split_cells(body),
        None => Vec::new(),
    }
}

/// Split an already-indent-stripped table row body into trimmed, unescaped cells.
fn split_cells(body: &str) -> Vec<String> {
    let body = body.strip_prefix('|').unwrap_or(body);
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in body.chars() {
        if esc {
            cur.push(ch); // the escaped char, backslash dropped
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    // Push the final cell unless it is the empty remainder left by a trailing
    // pipe (the optional closing delimiter). A pipe-free single cell still counts.
    if !cur.trim().is_empty() || cells.is_empty() {
        cells.push(cur.trim().to_string());
    }
    cells
}

/// If `line` is a table **delimiter row** — after ≤3 leading spaces, an unescaped
/// pipe plus cells each matching `:?-+:?` (dashes with optional alignment colons)
/// — return the per-column [`Alignment`]s, else `None`. Requiring a pipe keeps a
/// bare `---` a [`thematic_break`], not a one-column delimiter (the renderer only
/// consults this after a candidate header, so the header supplies the column
/// count it must match).
#[must_use]
pub fn table_delimiter(line: &str) -> Option<Vec<Alignment>> {
    let body = table_indent(line)?;
    if !has_unescaped_pipe(body) {
        return None;
    }
    let cells = split_cells(body);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in &cells {
        let left = cell.starts_with(':');
        let right = cell.ends_with(':');
        let dashes = cell.strip_prefix(':').unwrap_or(cell);
        let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Alignment::Center,
            (true, false) => Alignment::Left,
            (false, true) => Alignment::Right,
            (false, false) => Alignment::None,
        });
    }
    Some(aligns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_delimiter_parses_per_column_alignment() {
        use Alignment::{Center, Left, None as NoAlign, Right};
        assert_eq!(table_delimiter("|---|---|"), Some(vec![NoAlign, NoAlign]));
        assert_eq!(
            table_delimiter("|:--|:-:|--:|"),
            Some(vec![Left, Center, Right])
        );
        // Surrounding pipes are optional.
        assert_eq!(table_delimiter("--- | ---"), Some(vec![NoAlign, NoAlign]));
        assert_eq!(table_delimiter(" | :---: | "), Some(vec![Center]));
        // Up to three leading spaces still count (block indent rule).
        assert_eq!(table_delimiter("   |---|"), Some(vec![NoAlign]));
    }

    #[test]
    fn table_delimiter_rejects_non_delimiter_rows() {
        assert_eq!(table_delimiter("| a | b |"), None, "has text");
        assert_eq!(table_delimiter("plain"), None, "no pipes");
        assert_eq!(table_delimiter("| -x- |"), None, "non-dash content");
        assert_eq!(table_delimiter("|::|"), None, "no dashes");
        assert_eq!(table_delimiter(""), None);
        assert_eq!(table_delimiter("    |---|"), None, "4-space indent is code");
    }

    #[test]
    fn table_cells_splits_trims_and_unescapes() {
        assert_eq!(table_cells("| a | b | c |"), vec!["a", "b", "c"]);
        assert_eq!(table_cells("a | b"), vec!["a", "b"], "no surrounding pipes");
        assert_eq!(table_cells("|  x  |"), vec!["x"], "cells are trimmed");
        assert_eq!(
            table_cells(r"| a \| b | c |"),
            vec!["a | b", "c"],
            "an escaped pipe stays inside its cell, unescaped"
        );
    }

    #[test]
    fn is_table_row_detects_pipe_lines() {
        assert!(is_table_row("| a | b |"));
        assert!(is_table_row("a | b"));
        assert!(!is_table_row("no pipes here"));
        assert!(!is_table_row(""));
        assert!(
            !is_table_row(r"escaped \| only"),
            "a lone escaped pipe is prose"
        );
        assert!(
            !is_table_row("    | a |"),
            "4-space indent is code, not a table"
        );
    }

    fn code(lang: Option<&str>, lines: &[&str]) -> Block {
        Block::Code {
            lang: lang.map(str::to_string),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn block_scanner_classifies_lines_like_parse_blocks() {
        use LineKind::{Code, CodeEnd, CodeStart, Prose};
        let text = "before\n```rust\nfn main() {}\n```\nafter";
        let mut sc = BlockScanner::new();
        let kinds: Vec<LineKind> = text.split('\n').map(|l| sc.classify(l)).collect();
        assert_eq!(
            kinds,
            vec![Prose, CodeStart(Some("rust".into())), Code, CodeEnd, Prose,]
        );
    }

    #[test]
    fn block_scanner_agrees_with_parse_blocks_on_code_membership() {
        // For every sample, the scanner's per-line verdict (is this line code
        // content?) must match what `parse_blocks` puts in a `Code` block — the
        // two fence state machines cannot disagree, or the renderer would commit
        // rows `parse_blocks`-based tests never expect.
        for text in [
            "plain only",
            "a\n```\ncode\n```\nb",
            "```python\nx = 1\ny = 2\n```",
            "~~~\n```\nnested\n~~~\ntail",
            "intro\n```py\nunterminated",
            "   ```\nindented fence\n   ```",
            // Indented (4-space) code blocks — the scanner and parse_blocks must
            // agree on their membership too (start-after-blank, blank-line
            // continuation, paragraph-interruption gate, trailing block).
            "intro\n\n    indented\n    code\nback",
            "    leading code\ndone",
            "para\n    lazy continuation not code",
            "    a\n\n    b\nx",
            "\ttab code after start\nprose",
        ] {
            // parse_blocks: flatten to (is_code, line) per source line.
            let expected: Vec<bool> = parse_blocks(text)
                .into_iter()
                .flat_map(|b| match b {
                    Block::Prose(s) => s.split('\n').map(|_| false).collect::<Vec<_>>(),
                    Block::Code { lines, .. } => lines.iter().map(|_| true).collect(),
                })
                .collect();
            // scanner: a line is "code content" iff it is Code (fence delimiters
            // are dropped by parse_blocks, so they have no flattened row).
            let mut sc = BlockScanner::new();
            let got: Vec<bool> = text
                .split('\n')
                .filter_map(|l| match sc.classify(l) {
                    LineKind::Code => Some(true),
                    LineKind::Prose => Some(false),
                    LineKind::CodeStart(_) | LineKind::CodeEnd => None,
                })
                .collect();
            assert_eq!(got, expected, "text={text:?}");
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
    fn a_four_space_indent_after_a_blank_is_an_indented_code_block() {
        // Preceded by a blank line, a ≥4-space run is a code block (lang None),
        // kept verbatim; the first non-indented line ends it.
        let text = "intro\n\n    let x = 1;\n    let y = 2;\nback to prose";
        assert_eq!(
            parse_blocks(text),
            vec![
                Block::Prose("intro\n".into()),
                code(None, &["    let x = 1;", "    let y = 2;"]),
                Block::Prose("back to prose".into()),
            ]
        );
    }

    #[test]
    fn a_leading_indented_line_is_code_at_start_of_text() {
        // Start-of-text counts as a blank boundary, so a leading indented run
        // opens an indented code block.
        assert_eq!(
            parse_blocks("    code here\ntail"),
            vec![code(None, &["    code here"]), Block::Prose("tail".into()),]
        );
    }

    #[test]
    fn an_indented_line_continuing_a_paragraph_is_not_code() {
        // No blank before the indented line → a lazy paragraph continuation, so
        // it stays prose (an indented code block cannot interrupt a paragraph).
        let text = "a paragraph\n    still the paragraph";
        assert_eq!(
            parse_blocks(text),
            vec![Block::Prose("a paragraph\n    still the paragraph".into())]
        );
    }

    #[test]
    fn a_tab_indent_after_a_blank_is_an_indented_code_block() {
        assert_eq!(
            parse_blocks("x\n\n\ttab_code()"),
            vec![Block::Prose("x\n".into()), code(None, &["\ttab_code()"]),]
        );
    }

    #[test]
    fn blank_lines_inside_an_indented_block_stay_code() {
        let text = "    a\n\n    b\nx";
        assert_eq!(
            parse_blocks(text),
            vec![
                code(None, &["    a", "", "    b"]),
                Block::Prose("x".into())
            ]
        );
    }

    #[test]
    fn thematic_break_detects_rules_and_rejects_non_rules() {
        assert_eq!(thematic_break("---"), Some('-'));
        assert_eq!(thematic_break("***"), Some('*'));
        assert_eq!(thematic_break("___"), Some('_'));
        assert_eq!(thematic_break("- - -"), Some('-'));
        assert_eq!(thematic_break("  ***  "), Some('*'));
        assert_eq!(thematic_break("-----"), Some('-'));
        // Not rules:
        assert_eq!(thematic_break("--"), None, "only two markers");
        assert_eq!(thematic_break("- item"), None, "list item, not a rule");
        assert_eq!(thematic_break("---x"), None, "trailing non-marker");
        assert_eq!(thematic_break("    ---"), None, "4-space indent is code");
        assert_eq!(thematic_break("| a | b |"), None, "table row");
        assert_eq!(thematic_break(""), None);
        assert_eq!(
            thematic_break("=="),
            None,
            "'=' is not a thematic-break marker"
        );
    }

    #[test]
    fn is_partial_thematic_break_detects_unsettled_rule_runs() {
        // 1–2 markers (optionally with spaces) can still grow into a ≥3 rule.
        assert!(is_partial_thematic_break("-"));
        assert!(is_partial_thematic_break("--"));
        assert!(is_partial_thematic_break("**"));
        assert!(is_partial_thematic_break("__"));
        assert!(is_partial_thematic_break("- "));
        assert!(is_partial_thematic_break("- -"));
        assert!(is_partial_thematic_break("  --"));
        // 3+ markers is already a settled rule, not a partial.
        assert!(!is_partial_thematic_break("---"));
        assert!(!is_partial_thematic_break("***"));
        // A non-marker char settles it as prose (e.g. a list item).
        assert!(!is_partial_thematic_break("- x"));
        assert!(!is_partial_thematic_break("-x"));
        assert!(!is_partial_thematic_break("hello"));
        assert!(!is_partial_thematic_break(""));
        // 4+ spaces of indent is indented code, never a rule.
        assert!(!is_partial_thematic_break("    --"));
    }

    #[test]
    fn is_partial_fence_detects_unsettled_marker_runs() {
        // 1–2 bare markers (optionally ≤3-space indented) can still become a fence.
        assert!(is_partial_fence("`"));
        assert!(is_partial_fence("``"));
        assert!(is_partial_fence("~"));
        assert!(is_partial_fence("~~"));
        assert!(is_partial_fence("  ``"));
        // 3+ markers is already a full fence opener (a settled CodeStart).
        assert!(!is_partial_fence("```"));
        assert!(!is_partial_fence("~~~"));
        // Anything after the markers freezes the run below 3 → settled prose.
        assert!(!is_partial_fence("``x"));
        assert!(!is_partial_fence("`` "));
        assert!(!is_partial_fence("x`"));
        // Plain prose and blank lines are settled.
        assert!(!is_partial_fence("hello"));
        assert!(!is_partial_fence(""));
        // 4+ spaces of indent is indented code, never a fence.
        assert!(!is_partial_fence("    ``"));
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
    fn backtick_fence_info_string_may_not_contain_backticks() {
        // CommonMark: the info string of a *backtick* fence cannot contain a
        // backtick (such a line is inline code, not a fence). Treating it as
        // an opener swallowed the rest of the reply as an unterminated block.
        let blocks = parse_blocks("```rust`inline`\nstill prose");
        assert!(
            blocks.iter().all(|b| matches!(b, Block::Prose(_))),
            "a backtick run with a backtick in its info string is prose: {blocks:?}"
        );
        // Tilde fences may carry backticks in the info string (spec).
        let blocks = parse_blocks("~~~py`x\ncode\n~~~");
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Code { .. })),
            "a tilde fence still opens: {blocks:?}"
        );
    }

    #[test]
    fn a_leading_tab_is_never_a_thematic_break() {
        // A leading tab is ≥4 columns of indent — indented code, not a rule.
        assert_eq!(thematic_break("\t---"), None);
        assert_eq!(thematic_break(" \t***"), None);
        assert!(!is_partial_thematic_break("\t--"));
        // Interior tabs between markers are still fine (spec).
        assert_eq!(thematic_break("- \t- \t-"), Some('-'));
    }

    #[test]
    fn is_partial_heading_holds_only_while_a_bare_hash_run() {
        // A trailing all-# line is a heading whose LEVEL is unsettled: another
        // '#' deepens it (different style), a 7th flips it to prose — the
        // renderer must withhold it like a partial fence.
        for run in ["#", "##", "######", "  ###"] {
            assert!(is_partial_heading(run), "{run:?} could still deepen");
        }
        // Settled: too deep for a heading, a space/text after the run, prose,
        // or 4+ spaces of indent (indented code, never a heading).
        for done in ["#######", "# title", "## ", "##x", "plain", "", "    #"] {
            assert!(!is_partial_heading(done), "{done:?} is settled");
        }
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

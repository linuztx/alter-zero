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
//! lets [`crate::ui::StreamRender::commit`] keep flushing completed lines to
//! scrollback as a reply streams (CLAUDE.md invariant 2). An **unterminated**
//! fence still renders as code, identically to a closed one, so nothing changes
//! when the closing fence finally arrives.

use std::borrow::Cow;

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
        } else if !blank && prev_blank && is_indented_code_line(line) && list_item(line).is_none() {
            // An indented code block starts only after a blank/at start-of-text
            // (it cannot interrupt a paragraph) — and never on a list-marker
            // line: a 4-space `- x` after a blank is a loose list's *nested
            // item*, which the top-level indent rule would otherwise swallow
            // (see [`list_item`]). A marker line inside an already-open block
            // still continues it above.
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

    /// Advance past a line the caller already knows is paragraph text — a task
    /// item's text, which no fence or indent can turn into code.
    pub fn note_prose(&mut self, line: &str) {
        self.in_indented = false;
        self.prev_blank = line.trim().is_empty();
    }

    /// Whether the scanner is inside an **indented** (4-space) code block — the
    /// state in which a blank or still-indented next line is code too.
    #[must_use]
    pub fn in_indented(&self) -> bool {
        self.in_indented
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
        } else if !blank
            && self.prev_blank
            && is_indented_code_line(line)
            && list_item(line).is_none()
        {
            // Start an indented code block (its first line is code content, so
            // there is no separate `CodeStart` — the renderer highlights it plain).
            // Never on a list-marker line — that is a loose list's nested item
            // (see [`list_item`]); a marker line inside an already-open block
            // still continues it above. Mirrors [`parse_blocks`].
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
#[must_use]
pub fn is_indented_code_line(line: &str) -> bool {
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

/// Whether `line` opens a fenced code block (a fence delimiter whose info
/// string a backtick fence allows) — the test a lazy paragraph continuation
/// must fail, since a fence interrupts a paragraph.
#[must_use]
pub fn is_fence_opener(line: &str) -> bool {
    fence_marker(line).is_some()
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
    // CommonMark's optional closing sequence: a run of `#`s that is the whole
    // content or follows a space is not content (`## Title ##` → `Title`).
    let content = after.trim_start_matches(' ').trim_end();
    let unclosed = content.trim_end_matches('#');
    let content = if unclosed.is_empty() {
        ""
    } else if unclosed.ends_with(' ') {
        unclosed.trim_end()
    } else {
        content
    };
    Some((hashes as u8, content))
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

/// Whether `line` could **open** a table block — a candidate header row: a
/// table row that additionally carries the **leading pipe** (after the ≤3
/// spaces of indent). GFM makes the leading pipe optional, but honouring the
/// headerless form is incompatible with streaming to immutable scrollback:
/// ANY growing prose line could turn into a table header the moment a later
/// `|` streamed in (`` `span`a `` → `` `span`a | b ``), retroactively
/// re-drawing rows already committed as a grid. A leading `|` decides
/// candidacy at the line's **first** character instead, so ordinary prose
/// commits progressively and can never be re-shaped — and it is the form
/// models actually emit. Data rows *inside* a confirmed table keep the loose
/// [`is_table_row`], including the hard-wrapped-tail re-join
/// (docs/table-streaming.md).
#[must_use]
pub fn is_table_header_candidate(line: &str) -> bool {
    table_indent(line).is_some_and(|t| t.starts_with('|'))
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
            // Only the pipe escape is the table's own (GFM): it keeps a pipe
            // inside its cell. Every other escape belongs to the inline parser
            // the cell meets next, so the pair passes through untouched —
            // `\*x\*` stays literal `*x*`, never italic.
            if ch != '|' {
                cur.push('\\');
            }
            cur.push(ch);
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
    if esc {
        cur.push('\\'); // a lone trailing backslash is literal
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

// --- Inline markdown (`docs/markdown.md`). A prose line's emphasis (`*`/`_`,
// `**`/`__`, and both at once), `~~strike~~`, `` `code` ``, `[text](url)` /
// `![alt](url)`, `<url>` autolinks, backslash escapes and HTML entities parse
// into a tree of styled spans; [`crate::ui`] maps each node to a `Style`.
// Emphasis follows CommonMark's **delimiter-run algorithm** — the flanking
// rules, the rule of three, a closing `***` split between the two openers it
// closes — the parse codex's pulldown-cmark pass makes, with one relaxation
// for CJK text ([`DelimRun::new`]). It is **line-local**: an unclosed marker
// stays literal, so a *complete* source line's styling is final (its frozen
// rows never restyle). A *trailing partial* line with an unsettled construct
// is withheld from scrollback ([`has_open_inline`], wired into
// [`crate::ui::StreamRender::commit`]) until it settles. ---

/// One inline node of a prose line. `Bold`/`Italic`/`Strike`/`Link` nest (their
/// content is itself parsed), so `**bold _and italic_**` is a `Bold` holding an
/// `Italic`; `Code`, `Image` alt text and `Url` text are literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Literal text (escapes and entities already resolved).
    Text(String),
    /// `**bold**` / `__bold__`.
    Bold(Vec<Inline>),
    /// `*italic*` / `_italic_`.
    Italic(Vec<Inline>),
    /// `~~strikethrough~~`.
    Strike(Vec<Inline>),
    /// `` `inline code` `` — rendered literally, in the code colour.
    Code(String),
    /// `[text](url)` — the text nests, the URL is shown after it.
    Link {
        /// The link's display text (parsed for nested emphasis).
        text: Vec<Inline>,
        /// The link target, rendered as ` (url)` after the text.
        url: String,
    },
    /// `![alt](url)` — rendered as its alt text (the URL is dropped).
    Image {
        /// The image's alt text.
        alt: String,
    },
    /// An autolink — `<https://…>` or `<me@example.com>` — shown as its bare
    /// text, the angle brackets dropped, and linked to `target` (the address
    /// gains `mailto:`).
    Url {
        /// What the reader sees: the URL or address as written.
        text: String,
        /// What a click opens.
        target: String,
    },
}

/// Parse a prose `line` into inline nodes (`docs/markdown.md`). Left-to-right,
/// line-local: an unclosed or non-flanking marker stays literal, so the result
/// of a **complete** line is final (prefix-stable once the line ends).
#[must_use]
pub fn parse_inline(text: &str) -> Vec<Inline> {
    InlineScan::scan(text).into_inlines()
}

/// Whether `text` (a still-growing trailing line) holds an **unsettled** inline
/// construct a later character could still resolve — and so restyle text
/// already emitted: an emphasis/strike opener with no closer yet, a code span
/// with no closing run, a link or image whose brackets or parens are open, an
/// autolink or entity still forming. The streaming committer withholds such a
/// line whole (like an in-code line) until it settles, keeping scrollback
/// immutable. A non-flanking `*` (`2 * 3`), an intraword `_` (`foo_bar`), an
/// escaped marker or a settled literal `[a]` is **not** open.
#[must_use]
pub fn has_open_inline(text: &str) -> bool {
    // A marker at the very END of the growing line is *undecided*, not
    // settled: an emphasis run's flanking — whether it opens at all — is
    // determined by the character that hasn't streamed in yet ("~~" + "s"
    // hides the tildes and strikes what follows; "~~" + " " keeps them
    // literal); a `!` becomes an image opener if a `[` lands next; a trailing
    // backtick may be a closing run the next char EXTENDS (`` `x` `` + `` ` ``
    // → `` `x`` `` — the exact-run match breaks and the settled-looking span
    // reverts to literal); a `\` escapes the next character, a `<` may open an
    // autolink and an `&` an entity. Either way the already-wrapped text would
    // restyle, so the committer must withhold the line. A complete line
    // settles these on its own (end-of-line means no follower), which is why
    // the check lives here and not in [`parse_inline`].
    if text
        .chars()
        .next_back()
        .is_some_and(|c| matches!(c, '*' | '_' | '~' | '!' | '`' | '\\' | '<' | '&'))
    {
        return true;
    }
    InlineScan::scan(text).open
}

/// One delimiter run (`*`, `_`, or a `~~` pair) and what CommonMark's flanking
/// rules let it do.
#[derive(Debug, Clone, Copy)]
struct DelimRun {
    /// The marker character.
    ch: char,
    /// Markers not yet consumed by a match.
    len: usize,
    /// The run's original length (the rule of three reads it).
    orig: usize,
    /// Whether the run may open emphasis.
    can_open: bool,
    /// Whether the run may close emphasis.
    can_close: bool,
}

impl DelimRun {
    /// The run of `ch` spanning `text[start..end]`, its abilities read off the
    /// characters around it (CommonMark's left/right-flanking rules; the line's
    /// ends count as whitespace). `_` additionally refuses to open or close
    /// inside a word, so `snake_case` stays literal.
    ///
    /// One relaxation: CJK text puts no spaces between words, so a run whose
    /// outer neighbour is a CJK character is not held to the punctuation clause
    /// on that side — `**标题：**内容` (a closer after fullwidth punctuation,
    /// before a CJK letter) and `这是**“引用”**` both still emphasize, where
    /// strict CommonMark leaves the markers showing.
    fn new(text: &str, start: usize, end: usize, ch: char) -> Self {
        let prev = text[..start].chars().next_back();
        let next = text[end..].chars().next();
        let prev_ws = prev.is_none_or(char::is_whitespace);
        let next_ws = next.is_none_or(char::is_whitespace);
        let prev_punct = prev.is_some_and(is_inline_punct);
        let next_punct = next.is_some_and(is_inline_punct);
        let prev_cjk = prev.is_some_and(is_cjk);
        let next_cjk = next.is_some_and(is_cjk);
        let left = !next_ws && (!next_punct || prev_ws || prev_punct || prev_cjk);
        let right = !prev_ws && (!prev_punct || next_ws || next_punct || next_cjk);
        let (can_open, can_close) = if ch == '_' {
            (
                left && (!right || prev_punct),
                right && (!left || next_punct),
            )
        } else {
            (left, right)
        };
        let len = end - start; // the markers are ASCII: bytes == chars
        Self {
            ch,
            len,
            orig: len,
            can_open,
            can_close,
        }
    }
}

/// CommonMark's punctuation class for flanking: ASCII punctuation, or any other
/// character that is neither alphanumeric nor whitespace (Unicode punctuation
/// and symbols — `“`, `：`, an emoji).
fn is_inline_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

/// Whether `c` is a CJK character — ideographs, kana, hangul, and the CJK and
/// fullwidth punctuation blocks — for [`DelimRun::new`]'s relaxation.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF
            | 0x2E80..=0x2FDF
            | 0x2FF0..=0x31FF
            | 0x3200..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7FF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x20000..=0x2FA1F
            | 0x30000..=0x3134F)
}

/// One piece of a line being parsed: text, a finished node, a delimiter run
/// still waiting for its match, or a slot emptied by one.
#[derive(Debug)]
enum Piece {
    Text(String),
    Node(Inline),
    Delim(DelimRun),
    Gone,
}

/// The inline parse of one line: its pieces as a linked list over an arena
/// (so wrapping a matched span never shifts indices), the delimiter stack of
/// CommonMark's algorithm, and whether anything is still unsettled.
struct InlineScan {
    pieces: Vec<Piece>,
    prev: Vec<Option<usize>>,
    next: Vec<Option<usize>>,
    head: Option<usize>,
    tail: Option<usize>,
    /// Arena ids of delimiter pieces in line order; `None` once removed.
    stack: Vec<Option<usize>>,
    /// An unsettled construct was seen (see [`has_open_inline`]).
    open: bool,
}

impl InlineScan {
    /// Tokenize `text` and resolve its emphasis.
    fn scan(text: &str) -> Self {
        let mut scan = Self {
            pieces: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
            head: None,
            tail: None,
            stack: Vec::new(),
            open: false,
        };
        let brackets = Brackets::new(text);
        let mut plain = String::new();
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let c = rest.chars().next().expect("i is a char boundary < len");
            match c {
                '\\' => {
                    // A backslash escapes ASCII punctuation: the char is literal
                    // and never a delimiter. Anything else keeps the backslash.
                    match rest[1..].chars().next() {
                        Some(n) if n.is_ascii_punctuation() => {
                            plain.push(n);
                            i += 1 + n.len_utf8();
                        }
                        _ => {
                            plain.push('\\');
                            i += 1;
                        }
                    }
                }
                '`' => {
                    if let Some((node, next)) = code_at(text, i) {
                        scan.push_text(&mut plain);
                        scan.push(Piece::Node(node));
                        i = next;
                    } else {
                        // No closing run of the same length: the run is literal
                        // for now, but one could still arrive.
                        let run = rest.bytes().take_while(|&b| b == b'`').count();
                        plain.push_str(&rest[..run]);
                        scan.open = true;
                        i += run;
                    }
                }
                '!' if rest.starts_with("![") => {
                    if let Some((node, next)) = image_at(text, i, &brackets) {
                        scan.push_text(&mut plain);
                        scan.push(Piece::Node(node));
                        i = next;
                    } else {
                        scan.open |= link_openable(text, i, &brackets);
                        plain.push('!');
                        i += 1;
                    }
                }
                '[' => {
                    if let Some((node, next)) = link_at(text, i, &brackets) {
                        scan.push_text(&mut plain);
                        scan.push(Piece::Node(node));
                        i = next;
                    } else {
                        scan.open |= link_openable(text, i, &brackets);
                        plain.push('[');
                        i += 1;
                    }
                }
                '<' => {
                    if let Some((node, next)) = autolink_at(text, i) {
                        scan.push_text(&mut plain);
                        scan.push(Piece::Node(node));
                        i = next;
                    } else {
                        scan.open |= autolink_forming(&rest[1..]);
                        plain.push('<');
                        i += 1;
                    }
                }
                '&' => {
                    if let Some((decoded, len)) = entity_at(rest) {
                        plain.push_str(&decoded);
                        i += len;
                    } else {
                        scan.open |= entity_forming(&rest[1..]);
                        plain.push('&');
                        i += 1;
                    }
                }
                '*' | '_' | '~' => {
                    let run = rest.bytes().take_while(|&b| b == c as u8).count();
                    // Strikethrough is a `~~` pair; any other tilde run is text.
                    let delim = (c != '~' || run == 2)
                        .then(|| DelimRun::new(text, i, i + run, c))
                        .filter(|d| d.can_open || d.can_close);
                    match delim {
                        Some(d) => {
                            scan.push_text(&mut plain);
                            let id = scan.push(Piece::Delim(d));
                            scan.stack.push(Some(id));
                        }
                        None => plain.push_str(&rest[..run]),
                    }
                    i += run;
                }
                _ => {
                    plain.push(c);
                    i += c.len_utf8();
                }
            }
        }
        scan.push_text(&mut plain);
        scan.process_emphasis();
        // A run that can still open, unmatched, waits on a closer that may
        // stream in later.
        scan.open |= scan
            .stack
            .iter()
            .flatten()
            .any(|&id| matches!(scan.pieces[id], Piece::Delim(d) if d.can_open && d.len > 0));
        scan
    }

    /// Append a piece at the end of the list, returning its arena id.
    fn push(&mut self, piece: Piece) -> usize {
        let id = self.pieces.len();
        self.pieces.push(piece);
        self.prev.push(self.tail);
        self.next.push(None);
        match self.tail {
            Some(t) => self.next[t] = Some(id),
            None => self.head = Some(id),
        }
        self.tail = Some(id);
        id
    }

    /// Flush the accumulated plain text as a piece (nothing when empty).
    fn push_text(&mut self, plain: &mut String) {
        if !plain.is_empty() {
            self.push(Piece::Text(std::mem::take(plain)));
        }
    }

    /// The delimiter run behind arena id `id`.
    fn delim(&self, id: usize) -> DelimRun {
        match self.pieces[id] {
            Piece::Delim(d) => d,
            _ => unreachable!("the stack holds only delimiter pieces"),
        }
    }

    /// CommonMark's `process_emphasis` over the whole delimiter stack: each
    /// closer, left to right, takes the nearest compatible opener; the pieces
    /// between them become one `Bold`/`Italic`/`Strike` node. `openers_bottom`
    /// remembers, per kind of closer, how far back a search already came up
    /// empty, which keeps a line full of unmatched markers linear.
    fn process_emphasis(&mut self) {
        let mut openers_bottom: Vec<((char, bool, usize), usize)> = Vec::new();
        let mut pos = 0;
        while pos < self.stack.len() {
            let Some(cid) = self.stack[pos] else {
                pos += 1;
                continue;
            };
            let closer = self.delim(cid);
            if !closer.can_close || closer.len == 0 {
                pos += 1;
                continue;
            }
            let key = (closer.ch, closer.can_open, closer.orig % 3);
            let bottom = openers_bottom
                .iter()
                .find(|(k, _)| *k == key)
                .map_or(0, |&(_, b)| b);
            let found = (bottom..pos).rev().find(|&p| {
                self.stack[p].is_some_and(|oid| {
                    let opener = self.delim(oid);
                    opener.ch == closer.ch
                        && opener.can_open
                        && opener.len > 0
                        && !((opener.can_close || closer.can_open)
                            && (opener.orig + closer.orig).is_multiple_of(3)
                            && !(opener.orig.is_multiple_of(3) && closer.orig.is_multiple_of(3)))
                })
            });
            let Some(op) = found else {
                match openers_bottom.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, b)) => *b = pos,
                    None => openers_bottom.push((key, pos)),
                }
                if !closer.can_open {
                    self.stack[pos] = None;
                }
                pos += 1;
                continue;
            };
            let oid = self.stack[op].expect("found on the stack");
            let opener = self.delim(oid);
            let used = if closer.ch == '~' || (opener.len >= 2 && closer.len >= 2) {
                2
            } else {
                1
            };
            let children = self.take_between(oid, cid);
            let node = match (closer.ch, used) {
                ('~', _) => Inline::Strike(children),
                (_, 2) => Inline::Bold(children),
                _ => Inline::Italic(children),
            };
            self.insert_between(oid, cid, Piece::Node(node));
            for slot in &mut self.stack[op + 1..pos] {
                *slot = None;
            }
            if let Piece::Delim(d) = &mut self.pieces[oid] {
                d.len -= used;
                if d.len == 0 {
                    self.stack[op] = None;
                }
            }
            if let Piece::Delim(d) = &mut self.pieces[cid] {
                d.len -= used;
                if d.len == 0 {
                    self.stack[pos] = None;
                    pos += 1;
                }
            }
        }
    }

    /// Detach every piece strictly between arena ids `from` and `to`,
    /// returning them as inline nodes (an unmatched run between them is
    /// literal text now).
    fn take_between(&mut self, from: usize, to: usize) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut cur = self.next[from];
        while let Some(id) = cur {
            if id == to {
                break;
            }
            cur = self.next[id];
            let piece = std::mem::replace(&mut self.pieces[id], Piece::Gone);
            push_piece_inline(&mut out, piece);
        }
        self.next[from] = Some(to);
        self.prev[to] = Some(from);
        out
    }

    /// Link a new piece between adjacent arena ids `from` and `to`.
    fn insert_between(&mut self, from: usize, to: usize, piece: Piece) {
        let id = self.pieces.len();
        self.pieces.push(piece);
        self.prev.push(Some(from));
        self.next.push(Some(to));
        self.next[from] = Some(id);
        self.prev[to] = Some(id);
    }

    /// The finished nodes in line order, adjacent text merged.
    fn into_inlines(mut self) -> Vec<Inline> {
        let mut out = Vec::new();
        let mut cur = self.head;
        while let Some(id) = cur {
            cur = self.next[id];
            let piece = std::mem::replace(&mut self.pieces[id], Piece::Gone);
            push_piece_inline(&mut out, piece);
        }
        out
    }
}

/// Append `piece` to `out` as inline nodes, merging text into a preceding text
/// node: a delimiter run's unconsumed markers are literal text by now.
fn push_piece_inline(out: &mut Vec<Inline>, piece: Piece) {
    let text = match piece {
        Piece::Text(t) => t,
        Piece::Delim(d) if d.len > 0 => d.ch.to_string().repeat(d.len),
        Piece::Delim(_) | Piece::Gone => return,
        Piece::Node(node) => {
            out.push(node);
            return;
        }
    };
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(&text);
    } else {
        out.push(Inline::Text(text));
    }
}

/// Parse a `` ` ``-delimited code span at byte `i`: a run of N backticks closed by
/// the next run of **exactly** N. One leading/trailing space is stripped (unless
/// the content is all spaces), per CommonMark.
fn code_at(text: &str, i: usize) -> Option<(Inline, usize)> {
    let run = text[i..].bytes().take_while(|&b| b == b'`').count();
    let open_end = i + run;
    let bytes = text.as_bytes();
    let mut j = open_end;
    while j < text.len() {
        if bytes[j] == b'`' {
            let rlen = text[j..].bytes().take_while(|&b| b == b'`').count();
            if rlen == run {
                return Some((Inline::Code(strip_code_span(&text[open_end..j])), j + rlen));
            }
            j += rlen;
        } else {
            j += 1;
        }
    }
    None
}

/// Strip one leading and trailing space from a code span's content unless it is
/// entirely spaces (CommonMark), so `` ` x ` `` → `x`.
fn strip_code_span(content: &str) -> String {
    if content.len() >= 2
        && content.starts_with(' ')
        && content.ends_with(' ')
        && !content.trim().is_empty()
    {
        content[1..content.len() - 1].to_string()
    } else {
        content.to_string()
    }
}

/// Every `[`…`]` and `(`…`)` pair of a line, matched in one pass — depth
/// tracked per kind, a backslash skipping the byte after it — which is the
/// pairing a forward scan from each opener would find. A line of unclosed
/// brackets used to rescan its rest for every `[` (twice: once to try the
/// link, once to ask whether it could still form), which made a 25 KB line
/// of `![` cost over a second per render.
struct Brackets {
    /// `(open, close)` byte offsets of matched `[`/`]` pairs, sorted by open.
    square: Vec<(usize, usize)>,
    /// The same for `(`/`)`.
    round: Vec<(usize, usize)>,
}

impl Brackets {
    fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let (mut square, mut round) = (Vec::new(), Vec::new());
        let (mut open_square, mut open_round) = (Vec::new(), Vec::new());
        let mut j = 0;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => {
                    j += 2; // skip the escaped byte
                    continue;
                }
                b'[' => open_square.push(j),
                b']' => {
                    if let Some(open) = open_square.pop() {
                        square.push((open, j));
                    }
                }
                b'(' => open_round.push(j),
                b')' => {
                    if let Some(open) = open_round.pop() {
                        round.push((open, j));
                    }
                }
                _ => {}
            }
            j += 1;
        }
        square.sort_unstable();
        round.sort_unstable();
        Self { square, round }
    }

    /// The content between a `[` at byte `i` and its matching `]`, plus the
    /// byte index just past the `]`.
    fn square<'a>(&self, text: &'a str, i: usize) -> Option<(&'a str, usize)> {
        Self::content(&self.square, text, i)
    }

    /// The content between a `(` at byte `i` and its matching `)`, plus the
    /// byte index just past the `)`.
    fn round<'a>(&self, text: &'a str, i: usize) -> Option<(&'a str, usize)> {
        Self::content(&self.round, text, i)
    }

    fn content<'a>(pairs: &[(usize, usize)], text: &'a str, i: usize) -> Option<(&'a str, usize)> {
        let k = pairs.binary_search_by_key(&i, |&(open, _)| open).ok()?;
        let close = pairs[k].1;
        Some((&text[i + 1..close], close + 1))
    }
}

/// Parse a `[text](url)` link at byte `i` (`text[i] == '['`).
fn link_at(text: &str, i: usize, brackets: &Brackets) -> Option<(Inline, usize)> {
    let (label, after_label) = brackets.square(text, i)?;
    let (url, next) = brackets.round(text, after_label)?;
    Some((
        Inline::Link {
            text: parse_inline(label),
            url: link_destination(url),
        },
        next,
    ))
}

/// Parse an `![alt](url)` image at byte `i` (`text[i..]` starts `![`).
fn image_at(text: &str, i: usize, brackets: &Brackets) -> Option<(Inline, usize)> {
    let (alt, after_alt) = brackets.square(text, i + 1)?;
    let (_url, next) = brackets.round(text, after_alt)?;
    Some((
        Inline::Image {
            alt: unescape_punct(alt),
        },
        next,
    ))
}

/// The destination inside a link's parens: a `<…>`-bracketed target, else
/// everything up to the first space — an optional `"title"` after it is
/// dropped, as codex's parser drops it — with escapes resolved.
fn link_destination(paren: &str) -> String {
    let paren = paren.trim();
    let dest = match paren.strip_prefix('<').and_then(|p| p.split_once('>')) {
        Some((inner, _)) => inner,
        None => paren.split_whitespace().next().unwrap_or(""),
    };
    unescape_punct(dest)
}

/// `s` with each backslash-escaped ASCII punctuation character unescaped — a
/// link target or alt text keeps none of the escapes its source needed.
fn unescape_punct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && n.is_ascii_punctuation()
        {
            out.push(n);
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a `[`/`![` at byte `i` could still grow into a link — its bracket is
/// unclosed, or closed but the `(url)` part is still incomplete (or could yet
/// begin). A `[a]` followed by a non-`(` char is settled literal, not openable.
fn link_openable(text: &str, i: usize, brackets: &Brackets) -> bool {
    let open = if text[i..].starts_with('!') { i + 1 } else { i };
    let Some((_, after_label)) = brackets.square(text, open) else {
        return true; // bracket still open
    };
    match text[after_label..].chars().next() {
        None => true, // `[label]` at end — `(` could still follow
        Some('(') => brackets.round(text, after_label).is_none(), // `(url` unclosed
        Some(_) => false, // `[label]x` — settled, not a link
    }
}

/// Parse an autolink at byte `i` (`text[i] == '<'`): a URL — a 2–32 character
/// scheme, a `:`, then no space, `<` or `>` — or an email address, closed by
/// `>`. Anything else (`<div>`, `a < b`) is not one.
fn autolink_at(text: &str, i: usize) -> Option<(Inline, usize)> {
    let rest = &text[i + 1..];
    // The body ends at the first character an autolink cannot hold, which
    // must be the closing `>` — a scan bounded by one word.
    let end = rest.find(|c: char| c == '>' || c == '<' || c.is_whitespace() || c.is_control())?;
    let body = &rest[..end];
    if !rest[end..].starts_with('>') || body.is_empty() {
        return None;
    }
    let node = if is_uri_autolink(body) {
        Inline::Url {
            text: body.to_string(),
            target: body.to_string(),
        }
    } else if is_email_autolink(body) {
        Inline::Url {
            text: body.to_string(),
            target: format!("mailto:{body}"),
        }
    } else {
        return None;
    };
    Some((node, i + 1 + end + 1))
}

/// Whether `body` is a CommonMark absolute URI: a scheme of 2–32 characters
/// (a letter, then letters, digits, `+`, `.` or `-`), a `:`, then the rest.
fn is_uri_autolink(body: &str) -> bool {
    let Some(colon) = body.find(':') else {
        return false;
    };
    let scheme = &body[..colon];
    (2..=32).contains(&scheme.len())
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Whether `c` may appear in an email autolink's local part.
fn is_email_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c)
}

/// Whether `body` is a CommonMark email autolink: a local part, `@`, then
/// dot-separated labels of letters, digits and inner hyphens.
fn is_email_autolink(body: &str) -> bool {
    let Some((local, domain)) = body.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.chars().all(is_email_local_char)
        && !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Whether `rest` — everything after a `<` up to the line's end — could still
/// grow into an autolink: no `>` yet, nothing an autolink can't hold, and
/// still on the way to a scheme or an address.
fn autolink_forming(rest: &str) -> bool {
    if rest
        .chars()
        .any(|c| c == '>' || c == '<' || c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let scheme_prefix = match rest.find(':') {
        Some(colon) => {
            let scheme = &rest[..colon];
            (2..=32).contains(&scheme.len())
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        }
        None => {
            rest.len() <= 32
                && rest.chars().next().is_none_or(|c| c.is_ascii_alphabetic())
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        }
    };
    let email_prefix = match rest.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && local.chars().all(is_email_local_char)
                && domain
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        }
        None => rest.chars().all(is_email_local_char),
    };
    scheme_prefix || email_prefix
}

/// The named HTML entities decoded in prose — the ones a model actually emits.
/// Anything else stays literal, as an unknown `&name;` does in CommonMark.
const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("copy", "©"),
    ("reg", "®"),
    ("trade", "™"),
    ("hellip", "…"),
    ("mdash", "—"),
    ("ndash", "–"),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("bull", "•"),
    ("middot", "·"),
    ("times", "×"),
    ("divide", "÷"),
    ("plusmn", "±"),
    ("minus", "−"),
    ("deg", "°"),
    ("micro", "µ"),
    ("para", "¶"),
    ("sect", "§"),
    ("cent", "¢"),
    ("pound", "£"),
    ("euro", "€"),
    ("yen", "¥"),
    ("larr", "←"),
    ("rarr", "→"),
    ("uarr", "↑"),
    ("darr", "↓"),
    ("harr", "↔"),
    ("lArr", "⇐"),
    ("rArr", "⇒"),
    ("hArr", "⇔"),
    ("le", "≤"),
    ("ge", "≥"),
    ("ne", "≠"),
    ("asymp", "≈"),
    ("infin", "∞"),
    ("check", "✓"),
    ("cross", "✗"),
    ("star", "☆"),
    ("hearts", "♥"),
    ("zwj", "\u{200d}"),
    ("zwnj", "\u{200c}"),
    ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"),
    ("thinsp", "\u{2009}"),
];

/// Decode an entity at the start of `rest` (`rest` starts with `&`): a named
/// one from [`ENTITIES`], or a decimal (`&#169;`) / hex (`&#x2014;`) character
/// reference — an invalid or zero code point becomes U+FFFD, as CommonMark
/// says. Returns the text and the entity's byte length.
fn entity_at(rest: &str) -> Option<(String, usize)> {
    // An entity name or number is short and ASCII: bound the look-ahead so a
    // line of `&`s stays linear.
    let body_len = rest[1..]
        .bytes()
        .take(33)
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'#')
        .count();
    if rest.as_bytes().get(1 + body_len) != Some(&b';') {
        return None;
    }
    let body = &rest[1..1 + body_len];
    let decoded = if let Some(num) = body.strip_prefix('#') {
        let (digits, radix) = match num.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (num, 10),
        };
        let max = if radix == 16 { 6 } else { 7 };
        if digits.is_empty() || digits.len() > max || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        let code = u32::from_str_radix(digits, radix).ok()?;
        let c = char::from_u32(code)
            .filter(|&c| c != '\0')
            .unwrap_or('\u{fffd}');
        c.to_string()
    } else {
        ENTITIES
            .iter()
            .find(|(name, _)| *name == body)
            .map(|(_, value)| (*value).to_string())?
    };
    Some((decoded, body_len + 2))
}

/// Whether `rest` — everything after an `&` up to the line's end — could still
/// become an entity once its `;` arrives.
fn entity_forming(rest: &str) -> bool {
    let body = rest
        .strip_prefix('#')
        .map_or(rest, |n| n.strip_prefix(['x', 'X']).unwrap_or(n));
    rest.len() <= 32 && body.chars().all(|c| c.is_ascii_alphanumeric())
}

// --- List items and blockquotes (`docs/markdown.md`). Both are **line-local**
// (classified by the line's own prefix), so a complete line's rendering is final
// — prefix-stable while streaming, no holdback needed. [`crate::ui`] styles the
// marker and hangs the wrapped continuation under the text. ---

/// A list item's marker: a `-`/`*`/`+` bullet, or an ordered `N.`/`N)` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    /// `-`, `*`, or `+` — all rendered as one bullet glyph, but the char is
    /// kept: CommonMark starts a new list when it changes.
    Bullet(char),
    /// `N.` or `N)` — the number and its `.`/`)` delimiter.
    Ordered(u64, char),
}

/// A parsed list item: its leading indent (nesting), marker, and the text after
/// the marker (leading spaces trimmed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem<'a> {
    /// Leading spaces before the marker — the nesting indent, kept on render.
    pub indent: usize,
    /// The bullet or ordered marker.
    pub marker: ListMarker,
    /// The item's content (inline-parsed and hang-wrapped by the renderer).
    pub content: &'a str,
}

/// If `line` is a list item — any run of leading spaces, then a `-`/`*`/`+`
/// bullet or an `N.`/`N)` ordered marker **followed by a space** (or nothing) —
/// return its parts. A marker with no following space (`-x`, `1.x`) is prose; a
/// bare `---` is a thematic break (its marker isn't space-separated); a **tab**
/// indent is code, never a list. The caller checks [`thematic_break`] first, so
/// `- - -` / `* * *` stay rules.
///
/// The indent is deliberately uncapped: CommonMark's "4+ spaces is indented
/// code" rule is a *top-level* rule — inside a list, a 4-space-indented marker
/// is a **nested item**, and a line-local parser can't see the enclosing list.
/// Models nest 2 or 4 spaces per level, so capping at 3 collapsed every third
/// level to column 0 (prose wrap swallowed the indent — a child bullet rendered
/// *outdented* below its own parent). The scanners keep genuine indented code
/// working by exempting only marker lines ([`parse_blocks`]/[`BlockScanner`]).
#[must_use]
pub fn list_item(line: &str) -> Option<ListItem<'_>> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    let first = trimmed.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        return list_content(&trimmed[1..]).map(|content| ListItem {
            indent,
            marker: ListMarker::Bullet(first),
            content,
        });
    }
    let digits: usize = trimmed.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let after = &trimmed[digits..];
        let delim = after.chars().next()?;
        if matches!(delim, '.' | ')') {
            let num = trimmed[..digits].parse().ok()?;
            return list_content(&after[1..]).map(|content| ListItem {
                indent,
                marker: ListMarker::Ordered(num, delim),
                content,
            });
        }
    }
    None
}

/// The content after a list marker: `Some(text)` when what follows is empty or
/// begins with a space (a real marker), `None` otherwise (`-x` is prose). Leading
/// spaces are trimmed off the content.
fn list_content(after_marker: &str) -> Option<&str> {
    if after_marker.is_empty() {
        return Some("");
    }
    after_marker
        .strip_prefix(' ')
        .map(|rest| rest.trim_start_matches(' '))
}

/// Whether `line` is a *partial* list marker — leading spaces, then a marker
/// with nothing after it yet: a bare run of 1–9 ASCII digits, the same run
/// closed by `.`/`)`, or a lone `-`/`*`/`+`. Whatever streams in next decides
/// what the line is: a space makes it a list item — a container that styles
/// the marker, sets the item apart from a multi-line sibling and indents the
/// lines after it — while anything else leaves it prose (`1.5`, `+x`), so
/// the streaming committer withholds it, exactly like [`is_partial_heading`].
/// The indent is uncapped to match [`list_item`]: a `    5` can settle into a
/// *nested* `    5. …`.
#[must_use]
pub fn is_partial_list_marker(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if matches!(trimmed, "-" | "*" | "+") {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (1..=9).contains(&digits) && matches!(&trimmed[digits..], "" | "." | ")")
}

/// `line` with the tabs in its **block prefix** — the indentation, quote
/// markers and list markers its containers are read from — expanded to
/// spaces at CommonMark's four-column tab stops, which codex's parser reads
/// too: `\t- child` under `- parent` nests like `    - child` instead of
/// staying text that shows its `-`. A tab past the prefix is content and
/// stays (code expands its own). Borrowed when the prefix holds no tab.
#[must_use]
pub fn expand_prefix_tabs(line: &str) -> Cow<'_, str> {
    let end = block_prefix_len(line);
    if !line.as_bytes()[..end].contains(&b'\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    // The prefix is ASCII (spaces, tabs, `>`, markers), so a byte is a column.
    for b in line[..end].bytes() {
        if b == b'\t' {
            let stop = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', stop));
            col += stop;
        } else {
            out.push(char::from(b));
            col += 1;
        }
    }
    out.push_str(&line[end..]);
    Cow::Owned(out)
}

/// The byte length of `line`'s block prefix: runs of spaces and tabs, `>`
/// quote markers, and list markers (`-`, `*`, `+`, `N.`, `N)`) followed by a
/// space, a tab or the line's end.
fn block_prefix_len(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut i = 0;
    loop {
        while bytes.get(i).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            i += 1;
        }
        let marker_end = match bytes.get(i) {
            Some(b'>') => {
                i += 1;
                continue;
            }
            Some(b'-' | b'*' | b'+') => i + 1,
            Some(_) => {
                let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
                if !(1..=9).contains(&digits) || !matches!(bytes.get(i + digits), Some(b'.' | b')'))
                {
                    return i;
                }
                i + digits + 1
            }
            None => return i,
        };
        if !matches!(bytes.get(marker_end), None | Some(b' ' | b'\t')) {
            return i;
        }
        i = marker_end;
    }
}

/// Whether the still-growing trailing `line` has a **block prefix** the next
/// character could still change — a partial list marker, fence, heading run or
/// thematic run — at the top level *or behind the quote markers and list
/// markers the line already holds* (`> -`, `- 1`, `> > ##`). The renderer
/// gives a line's container prefixes structure (a quote, an item it indents
/// under, the blank row that sets a block apart), so until the prefix
/// settles none of the line's rows may reach scrollback. Conservative by
/// design: a complete thematic run (`---`) is still unsettled while it trails,
/// since `--- x` would be prose, and over-withholding only delays a commit.
#[must_use]
pub fn has_unsettled_block_prefix(line: &str) -> bool {
    let line = expand_prefix_tabs(line);
    let mut rest: &str = &line;
    loop {
        let t = rest.trim_start_matches(' ');
        if is_partial_list_marker(t)
            || is_partial_fence(t)
            || is_partial_heading(t)
            || thematic_marker_run(t).is_some()
        {
            return true;
        }
        if let Some(inner) = t.strip_prefix('>') {
            rest = inner;
            continue;
        }
        match list_item(t) {
            // After a task box the line is paragraph text, whatever it says.
            Some(item) if !item.content.is_empty() && task_box(item.content).0.is_none() => {
                rest = item.content;
            }
            _ => return false,
        }
    }
}

/// Split a GFM task box off a list item's content: `[ ]`, `[x]` or `[X]`
/// followed by whitespace — returns whether it is ticked and the text after
/// it. A bare `[x]` with nothing after it is not a box (codex leaves it
/// literal). What follows a box is the item's paragraph text, never another
/// block marker.
#[must_use]
pub fn task_box(content: &str) -> (Option<bool>, &str) {
    let done = match content.get(..3) {
        Some("[ ]") => false,
        Some("[x]" | "[X]") => true,
        _ => return (None, content),
    };
    match content[3..].strip_prefix([' ', '\t']) {
        Some(rest) => (Some(done), rest.trim_start_matches([' ', '\t'])),
        None => (None, content),
    }
}

/// If `line` is a blockquote — after ≤3 leading spaces, a `>` — return its inner
/// text (one optional space after the `>` stripped). A nested `> >` yields the
/// inner `> …`, which the renderer shows as quoted text.
#[must_use]
pub fn block_quote(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_item_detects_bullets_and_ordered() {
        let b = list_item("- hello").unwrap();
        assert_eq!(b.indent, 0);
        assert_eq!(b.marker, ListMarker::Bullet('-'));
        assert_eq!(b.content, "hello");
        let n = list_item("  2) world").unwrap();
        assert_eq!(n.indent, 2);
        assert_eq!(n.marker, ListMarker::Ordered(2, ')'));
        assert_eq!(n.content, "world");
        assert_eq!(
            list_item("* star").unwrap().marker,
            ListMarker::Bullet('*'),
            "the bullet's char is kept: CommonMark starts a new list when it changes"
        );
        assert_eq!(
            list_item("10. ten").unwrap().marker,
            ListMarker::Ordered(10, '.')
        );
        // Task-list items are ordinary bullets whose `[x]` stays literal text.
        assert_eq!(list_item("- [x] done").unwrap().content, "[x] done");
    }

    #[test]
    fn list_item_rejects_non_lists() {
        assert!(list_item("---").is_none(), "thematic break, not a list");
        assert!(list_item("1.no space").is_none());
        assert!(list_item("-nospace").is_none());
        assert!(list_item("plain").is_none());
        assert!(
            list_item("\t- tab indented").is_none(),
            "tab indent is code"
        );
    }

    #[test]
    fn list_item_accepts_deep_nesting_indents() {
        // A third-level bullet indents 4+ spaces — a NESTED item, not indented
        // code (models nest 2 or 4 spaces per level; the old ≤3 cap collapsed
        // level three to column 0, outdented below its own parent).
        let deep = list_item("    - deep").expect("a 4-space bullet is a nested item");
        assert_eq!(deep.indent, 4);
        assert_eq!(deep.marker, ListMarker::Bullet('-'));
        assert_eq!(deep.content, "deep");
        let deeper = list_item("      3) deeper").expect("a 6-space ordered item");
        assert_eq!(deeper.indent, 6);
        assert_eq!(deeper.marker, ListMarker::Ordered(3, ')'));
        assert_eq!(deeper.content, "deeper");
    }

    #[test]
    fn is_partial_list_marker_flags_bare_digit_runs() {
        assert!(is_partial_list_marker("1"));
        assert!(is_partial_list_marker("10"));
        assert!(is_partial_list_marker("  3"));
        // A marker with nothing after it is undecided too: a space makes it an
        // item (a container the next lines indent under), anything else
        // prose (`1.5`, `+x`) — so the line can't commit yet.
        assert!(is_partial_list_marker("1."));
        assert!(is_partial_list_marker("12)"));
        assert!(is_partial_list_marker("+"));
        assert!(is_partial_list_marker("-"));
        assert!(is_partial_list_marker("      *"), "at any nesting depth");
        assert!(!is_partial_list_marker("1. x"), "a settled item");
        assert!(!is_partial_list_marker("1.5"), "prose");
        assert!(
            !is_partial_list_marker("--"),
            "a thematic run, not a marker"
        );
        assert!(!is_partial_list_marker("1 x"), "prose");
        assert!(!is_partial_list_marker(""));
        assert!(
            is_partial_list_marker("    5"),
            "a nested ordered marker forms at 4+ spaces too (list_item accepts them)"
        );
        assert!(!is_partial_list_marker("abc"));
    }

    #[test]
    fn a_nested_list_item_after_a_blank_is_not_indented_code() {
        // The indented-code rule (≥4 spaces after a blank) must not swallow a
        // nested list item in a loose list — the line-local list_item wins.
        let mut sc = BlockScanner::new();
        assert_eq!(sc.classify("- top"), LineKind::Prose);
        assert_eq!(sc.classify(""), LineKind::Prose);
        assert_eq!(
            sc.classify("    - nested"),
            LineKind::Prose,
            "a nested bullet, not code"
        );
        assert_eq!(
            sc.classify("      1. deeper"),
            LineKind::Prose,
            "a nested ordered item, not code"
        );
        // parse_blocks agrees: the whole thing is one prose block.
        let blocks = parse_blocks("- top\n\n    - nested\n      1. deeper");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::Prose(_)), "{blocks:?}");
        // A genuine code line after a blank still opens the indented block.
        let mut sc = BlockScanner::new();
        assert_eq!(sc.classify("text"), LineKind::Prose);
        assert_eq!(sc.classify(""), LineKind::Prose);
        assert_eq!(sc.classify("    let x = 1;"), LineKind::Code);
        assert_eq!(
            sc.classify("    - inside the open block"),
            LineKind::Code,
            "a bullet-looking line CONTINUES an open indented block"
        );
    }

    #[test]
    fn block_quote_strips_the_marker() {
        assert_eq!(block_quote("> quoted"), Some("quoted"));
        assert_eq!(block_quote(">no space"), Some("no space"));
        assert_eq!(block_quote(">"), Some(""));
        assert_eq!(block_quote("  > indented"), Some("indented"));
        assert_eq!(block_quote("not a quote"), None);
        assert_eq!(block_quote("    > code"), None, "4-space is code");
    }

    #[test]
    fn parse_inline_keeps_plain_text_as_one_node() {
        assert_eq!(
            parse_inline("just plain text"),
            vec![Inline::Text("just plain text".into())]
        );
        assert_eq!(parse_inline(""), vec![]);
    }

    #[test]
    fn parse_inline_splits_bold_italic_strike() {
        use Inline::{Bold, Italic, Strike, Text};
        assert_eq!(
            parse_inline("a **b** c"),
            vec![
                Text("a ".into()),
                Bold(vec![Text("b".into())]),
                Text(" c".into())
            ]
        );
        assert_eq!(
            parse_inline("*i* x"),
            vec![Italic(vec![Text("i".into())]), Text(" x".into())]
        );
        assert_eq!(parse_inline("~~s~~"), vec![Strike(vec![Text("s".into())])]);
        // Bold wins over italic on `**`.
        assert_eq!(parse_inline("__b__"), vec![Bold(vec![Text("b".into())])]);
    }

    #[test]
    fn parse_inline_handles_code_links_images() {
        use Inline::{Code, Image, Link, Text};
        assert_eq!(
            parse_inline("run `x = 1` now"),
            vec![
                Text("run ".into()),
                Code("x = 1".into()),
                Text(" now".into())
            ]
        );
        assert_eq!(
            parse_inline("[docs](https://x.com)"),
            vec![Link {
                text: vec![Text("docs".into())],
                url: "https://x.com".into()
            }]
        );
        assert_eq!(
            parse_inline("![alt text](https://img.png)"),
            vec![Image {
                alt: "alt text".into()
            }]
        );
    }

    #[test]
    fn parse_inline_leaves_unclosed_or_literal_markers_as_text() {
        // Unclosed markers stay literal.
        assert_eq!(parse_inline("a **b"), vec![Inline::Text("a **b".into())]);
        assert_eq!(parse_inline("`open"), vec![Inline::Text("`open".into())]);
        // Intraword underscores (snake_case) are not emphasis.
        assert_eq!(
            parse_inline("call foo_bar_baz()"),
            vec![Inline::Text("call foo_bar_baz()".into())]
        );
        // A `*` flanked by spaces (multiplication) is not emphasis.
        assert_eq!(
            parse_inline("2 * 3 = 6"),
            vec![Inline::Text("2 * 3 = 6".into())]
        );
    }

    #[test]
    fn parse_inline_nests_emphasis() {
        use Inline::{Bold, Italic, Text};
        assert_eq!(
            parse_inline("**bold _and italic_**"),
            vec![Bold(vec![
                Text("bold ".into()),
                Italic(vec![Text("and italic".into())]),
            ])]
        );
    }

    #[test]
    fn parse_inline_resolves_triple_and_nested_runs_like_commonmark() {
        use Inline::{Bold, Italic, Text};
        // `***x***` is emphasis wrapping strong (CommonMark / pulldown-cmark),
        // where the old longest-marker-first matcher showed `*x*` in bold.
        let bold_italic = vec![Italic(vec![Bold(vec![Text("x".into())])])];
        assert_eq!(parse_inline("***x***"), bold_italic);
        assert_eq!(parse_inline("___x___"), bold_italic);
        // A closing `***` run splits between the two openers it closes.
        assert_eq!(
            parse_inline("**x *y***"),
            vec![Bold(vec![
                Text("x ".into()),
                Italic(vec![Text("y".into())])
            ])]
        );
        assert_eq!(
            parse_inline("*x **y***"),
            vec![Italic(vec![
                Text("x ".into()),
                Bold(vec![Text("y".into())])
            ])]
        );
        // A leftover marker of a longer run stays literal.
        assert_eq!(
            parse_inline("**x*"),
            vec![Text("*".into()), Italic(vec![Text("x".into())])]
        );
    }

    #[test]
    fn parse_inline_follows_commonmark_flanking() {
        use Inline::{Bold, Text};
        assert_eq!(
            parse_inline("**Status:** Most popular"),
            vec![
                Bold(vec![Text("Status:".into())]),
                Text(" Most popular".into())
            ]
        );
        assert_eq!(
            parse_inline("a**b**c"),
            vec![
                Text("a".into()),
                Bold(vec![Text("b".into())]),
                Text("c".into())
            ],
            "intraword `*` emphasis"
        );
        assert_eq!(
            parse_inline("snake_case_name_"),
            vec![Text("snake_case_name_".into())],
            "intraword `_` is never emphasis"
        );
        assert_eq!(
            parse_inline("**foo:**bar"),
            vec![Text("**foo:**bar".into())],
            "a closer after punctuation needs a space or punctuation after it"
        );
        // CJK text has no spaces between words, so a closer after fullwidth
        // punctuation (or an opener before it) next to a CJK letter still
        // counts — the common `**标题：**内容` shape.
        assert_eq!(
            parse_inline("**注意：**这是"),
            vec![Bold(vec![Text("注意：".into())]), Text("这是".into())]
        );
        assert_eq!(
            parse_inline("这是**“引用”**吧"),
            vec![
                Text("这是".into()),
                Bold(vec![Text("“引用”".into())]),
                Text("吧".into())
            ]
        );
    }

    #[test]
    fn parse_inline_backslash_escapes_drop_the_backslash() {
        use Inline::{Code, Text};
        assert_eq!(
            parse_inline(r"\*not italic\*"),
            vec![Text("*not italic*".into())]
        );
        assert_eq!(parse_inline(r"1\. item"), vec![Text("1. item".into())]);
        assert_eq!(parse_inline(r"a\_b\_c"), vec![Text("a_b_c".into())]);
        assert_eq!(parse_inline(r"\\"), vec![Text(r"\".into())]);
        assert_eq!(
            parse_inline(r"C:\path\to"),
            vec![Text(r"C:\path\to".into())],
            "a backslash before a non-punctuation char is literal"
        );
        assert_eq!(
            parse_inline(r"\`not code\`"),
            vec![Text("`not code`".into())]
        );
        assert_eq!(
            parse_inline(r"`a\*b`"),
            vec![Code(r"a\*b".into())],
            "escapes are literal inside code"
        );
    }

    #[test]
    fn parse_inline_angle_autolinks_drop_the_brackets() {
        use Inline::{Text, Url};
        assert_eq!(
            parse_inline("see <https://x.com/a>."),
            vec![
                Text("see ".into()),
                Url {
                    text: "https://x.com/a".into(),
                    target: "https://x.com/a".into()
                },
                Text(".".into())
            ]
        );
        assert_eq!(
            parse_inline("<me@example.com>"),
            vec![Url {
                text: "me@example.com".into(),
                target: "mailto:me@example.com".into()
            }]
        );
        assert_eq!(parse_inline("<div>"), vec![Text("<div>".into())]);
        assert_eq!(parse_inline("a < b > c"), vec![Text("a < b > c".into())]);
        assert_eq!(
            parse_inline("<https://x.com/a b>"),
            vec![Text("<https://x.com/a b>".into())],
            "an autolink holds no space"
        );
    }

    #[test]
    fn parse_inline_decodes_html_entities() {
        use Inline::Text;
        assert_eq!(
            parse_inline("Tom &amp; Jerry"),
            vec![Text("Tom & Jerry".into())]
        );
        assert_eq!(parse_inline("&lt;tag&gt;"), vec![Text("<tag>".into())]);
        assert_eq!(parse_inline("&#169; &#x2014;"), vec![Text("© —".into())]);
        assert_eq!(parse_inline("a&nbsp;b"), vec![Text("a\u{a0}b".into())]);
        assert_eq!(
            parse_inline("&bogus; AT&T"),
            vec![Text("&bogus; AT&T".into())]
        );
        assert_eq!(parse_inline("&#0;"), vec![Text("\u{fffd}".into())]);
    }

    #[test]
    fn parse_inline_stays_linear_on_a_line_of_unmatched_markers() {
        // Every closer that finds no opener raises `openers_bottom`, so a huge
        // line of markers that never pair (a minified blob, a glob list) costs
        // one pass instead of a look-back per marker.
        let closers = "a* ".repeat(60_000);
        let mixed = "a*a ".repeat(60_000);
        let started = std::time::Instant::now();
        assert_eq!(parse_inline(&closers), vec![Inline::Text(closers.clone())]);
        assert_eq!(parse_inline(&mixed).len(), 60_000 + 1);
        assert!(!has_open_inline(&closers));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn parse_inline_stays_linear_on_a_line_of_unclosed_brackets() {
        // Every `[` that fails to open a link used to rescan the rest of the
        // line, twice (once to try the link, once to ask whether it could
        // still form) — a 25 KB line of `![` took over a second per render,
        // which the strip pays on every chunk of a streaming reply. The
        // brackets and parens are matched once per line now.
        let lines = [
            ("![a ".repeat(6_000), true),
            ("[a ".repeat(8_000), true),
            ("[a](b ".repeat(4_000), true),
            ("(a [b] ".repeat(4_000), false),
        ];
        let started = std::time::Instant::now();
        for (line, open) in &lines {
            assert_eq!(parse_inline(line), vec![Inline::Text(line.clone())]);
            assert_eq!(has_open_inline(line), *open, "{}", &line[..12]);
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn has_open_inline_tracks_the_new_inline_forms() {
        assert!(has_open_inline("a ***b"), "an unmatched triple opener");
        assert!(
            has_open_inline(r"trailing \"),
            "the next char may be escaped"
        );
        assert!(
            !has_open_inline(r"\*not open"),
            "an escaped marker opens nothing"
        );
        assert!(
            has_open_inline("see <https://x"),
            "an autolink still forming"
        );
        assert!(
            has_open_inline("mail <me@exa"),
            "an email autolink still forming"
        );
        assert!(!has_open_inline("a < b"), "a spaced `<` is settled");
        assert!(has_open_inline("Tom &amp"), "an entity still forming");
        assert!(has_open_inline("Tom &"), "a trailing `&` may start one");
        assert!(!has_open_inline("Tom & Jerry"));
        assert!(!has_open_inline("**Status:** done"));
    }

    #[test]
    fn has_open_inline_flags_unsettled_trailing_lines() {
        // An unclosed delimiter could still restyle earlier text → withhold.
        assert!(has_open_inline("a **b"));
        assert!(has_open_inline("run `code"));
        assert!(has_open_inline("see ~~strike"));
        assert!(has_open_inline("a [link"));
        assert!(has_open_inline("a [link](htt"));
        assert!(has_open_inline("start *em"));
        // A trailing marker run is UNDECIDED, not settled: its flanking —
        // whether it opens at all — is the next character's call ("a ~~" +
        // "strike~~" hides the tildes; "a ~~ b" keeps them literal), so the
        // still-growing line must be withheld while it ends on one. Same for
        // a trailing `!`, which a following `[` turns into an image opener.
        assert!(has_open_inline("a ~~"));
        assert!(has_open_inline("a ~"));
        assert!(has_open_inline("emoji🎮~~"));
        assert!(has_open_inline("a **"));
        assert!(has_open_inline("a *"));
        assert!(has_open_inline("word_"));
        assert!(has_open_inline("wow!"));
        // A trailing backtick can be a closing run the next char extends
        // (`` `x` `` + `` ` `` breaks the exact-run match → literal again).
        assert!(has_open_inline("run `x`"));
        assert!(has_open_inline("`tick *ital `"));
        // Settled lines (closed, or no real openers) are safe to stream per row.
        assert!(!has_open_inline("a **b** c"));
        assert!(!has_open_inline("plain prose here"));
        assert!(!has_open_inline("2 * 3 = 6"));
        assert!(!has_open_inline("foo_bar_baz"));
        assert!(!has_open_inline("[link](url) done"));
        assert!(!has_open_inline(""));
    }

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
        assert_eq!(
            table_cells(r"| \*x\* | \\| c |"),
            vec![r"\*x\*", r"\\", "c"],
            "other escapes are left whole for the inline parser"
        );
    }

    #[test]
    fn is_table_row_detects_pipe_lines() {
        assert!(is_table_row("| a | b |"));
        assert!(is_table_row("a | b"));
        assert!(!is_table_row("no pipes here"));
        assert!(!is_table_row(""));
        // Header candidacy is stricter: the leading pipe is required, so a
        // prose line can never retroactively become a table header while it
        // streams (see is_table_header_candidate).
        assert!(is_table_header_candidate("| a | b |"));
        assert!(is_table_header_candidate("  | indented |"));
        assert!(!is_table_header_candidate("a | b"));
        assert!(!is_table_header_candidate("prose grows a pipe | later"));
        assert!(!is_table_header_candidate("    | deep indent |"));
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
    fn heading_level_drops_a_closing_hash_sequence() {
        // CommonMark's optional closing `#`s (after a space) are not content.
        assert_eq!(heading_level("## Title ##"), Some((2, "Title")));
        assert_eq!(heading_level("# Title #####  "), Some((1, "Title")));
        assert_eq!(heading_level("### ###"), Some((3, "")));
        assert_eq!(heading_level("## C#"), Some((2, "C#")), "no space, content");
        assert_eq!(heading_level("## Issue #5"), Some((2, "Issue #5")));
    }

    #[test]
    fn task_box_needs_whitespace_after_it() {
        assert_eq!(task_box("[ ] todo"), (Some(false), "todo"));
        assert_eq!(task_box("[x] done"), (Some(true), "done"));
        assert_eq!(task_box("[X]  caps"), (Some(true), "caps"));
        assert_eq!(task_box("[x] "), (Some(true), ""));
        assert_eq!(task_box("[x]"), (None, "[x]"), "nothing after: literal");
        assert_eq!(task_box("[x](u)"), (None, "[x](u)"), "a link");
        assert_eq!(task_box("[y] no"), (None, "[y] no"));
        assert_eq!(task_box("世界"), (None, "世界"));
    }

    #[test]
    fn has_unsettled_block_prefix_looks_inside_quotes_and_items() {
        // A trailing line whose block structure the next character could
        // still change — at the top level, or behind the `>`s and list
        // markers it already holds.
        for line in [
            "-", "1.", "+", "  2)", "> -", "> > 1.", "- 1", "> ##", "  ##", "- **", "> ``", "---",
            "- ---", "* * *",
        ] {
            assert!(has_unsettled_block_prefix(line), "{line:?}");
        }
        for line in [
            "- x",
            "> text",
            "1. item",
            "> - item",
            "plain",
            "",
            "- [x] done",
            "- [x] 10",
        ] {
            assert!(!has_unsettled_block_prefix(line), "{line:?}");
        }
    }

    #[test]
    fn expand_prefix_tabs_counts_to_four_column_stops() {
        // A tab in the indentation, after a `>` or after a list marker
        // advances to the next multiple of four columns (CommonMark).
        for (line, expanded) in [
            ("\t- a", "    - a"),
            ("  \t- a", "    - a"),
            ("\t\t1. a", "        1. a"),
            (">\t- b", ">   - b"),
            ("> \t- b", ">   - b"),
            ("-\tfoo", "-   foo"),
            ("1.\tfoo", "1.  foo"),
            ("\tcode\twith a tab", "    code\twith a tab"),
        ] {
            assert_eq!(expand_prefix_tabs(line), expanded, "{line:?}");
        }
        // Past the prefix a tab is content, and a line with no tab in its
        // prefix comes back as it was.
        for line in ["a\tb", "plain", "- x\ty", "1.5\tx", ""] {
            assert!(
                matches!(expand_prefix_tabs(line), Cow::Borrowed(same) if same == line),
                "{line:?}"
            );
        }
    }

    #[test]
    fn has_unsettled_block_prefix_reads_tabs_as_indentation() {
        for line in ["\t-", "\t1.", ">\t-", "-\t1", "\t##"] {
            assert!(has_unsettled_block_prefix(line), "{line:?}");
        }
        assert!(!has_unsettled_block_prefix("\t- item"));
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

//! The live `write` cell: a call the model is still generating, its file
//! streaming into the strip as the `content` argument decodes. While the
//! string is open the cell shows its **tail** — the newest lines, the last
//! one still being typed — and once it closes, its **head**: the first lines
//! over the `… +N lines` hint, the shape the committed `Wrote N lines` cell
//! takes. See `docs/write-streaming.md`.
//!
//! Highlighting is incremental and memoized per draft (the `ui::footer`
//! queued-rows pattern): the strip is rebuilt every animation frame and
//! reaches this walk several times per frame, so each complete line is
//! highlighted once, by a highlighter that has seen every line before it — a
//! line inside a docstring opened above the window stays a string.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::assistant::expand_code_tabs;
use super::file_cell::{file_body_indent, file_cell_lang, numbered_row_lines};
use super::theme::*;
use super::tool::{more_hint_line, result_row, tool_header_lines};
use super::*;
use crate::app::{DraftCall, DraftCalls, Theme};
use crate::highlight::{Highlighter, Seg};
use crate::partial_json::Field;

/// Every `write` draft's live cell, blank-separated like a batch's cells —
/// the strip's rows for the calls the model is generating. `pulse` blinks the
/// headers' bullets (the strip passes its frame clock; `None` renders them at
/// rest).
pub(super) fn draft_cells_lines(
    drafts: &DraftCalls,
    pulse: Option<Duration>,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for draft in drafts.writes() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(draft_write_lines(draft, pulse, width, paths));
    }
    lines
}

/// One `write` draft's live cell: the `● Write({path})` header (a running
/// call's, blinking at `pulse`), the dim `⎿ Writing N lines…` corner, then
/// the tail while the content streams or the head once it has closed.
pub(super) fn draft_write_lines(
    draft: &DraftCall,
    pulse: Option<Duration>,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(draft, pulse, width, paths);
    lines.extend(body_lines(draft, width));
    lines
}

/// The Ctrl+O expansion of a draft: its header and corner over **every**
/// line decoded so far, numbered and coloured, with nothing clipped — the
/// transcript's live tail, beside the running tool and the thinking block.
/// Highlighted fresh: the transcript cache rebuilds its tail only when the
/// draft has moved.
pub(super) fn draft_full_lines(
    draft: &DraftCall,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let mut lines = header_lines(draft, None, width, paths);
    let Some(content) = draft.content() else {
        lines.push(result_row(0, writing_label(None)));
        return lines;
    };
    let total = content.lines();
    lines.push(result_row(0, writing_label(Some(total))));
    let mut hl = Highlighter::new(
        file_cell_lang(draft.path().unwrap_or_default()),
        super::palette::palette().code,
    );
    let digits = digits_of(total);
    for (i, line) in content.text.lines().enumerate() {
        let segs = hl.line(&display_line(line));
        lines.extend(numbered_line(i + 1, digits, &segs, width, None));
    }
    lines
}

/// The header a draft wears: the `write` tool's own, built by the same
/// [`tool_header_lines`] every cell uses, for a call that is running. The
/// path shows once its string has closed ([`DraftCall::path`]) — before that
/// the header is a bare `● Write`, so it never reflows a character at a time.
fn header_lines(
    draft: &DraftCall,
    pulse: Option<Duration>,
    width: u16,
    paths: &PathDisplay,
) -> Vec<Line<'static>> {
    let call = ToolCall {
        name: crate::llm::tools::display_name(draft.name().unwrap_or_default()),
        args: draft.path().unwrap_or_default().to_string(),
        status: ToolStatus::Running,
        output: String::new(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
        call_id: None,
        position: None,
    };
    tool_header_lines(&call, width, /*collapsed=*/ true, pulse, paths)
}

/// The corner row's text: `Writing…` before any content, then
/// `Writing N line(s)…` counted by [`str::lines`]' rule, the finished cell's
/// `Wrote N lines` in the present tense.
fn writing_label(lines: Option<usize>) -> String {
    match lines {
        None | Some(0) => format!("{DRAFT_WRITING}…"),
        Some(1) => format!("{DRAFT_WRITING} 1 line…"),
        Some(n) => format!("{DRAFT_WRITING} {n} lines…"),
    }
}

/// How wide the gutter's numbers are — the widest one's digits, the
/// executor's `render_numbered_content` rule.
fn digits_of(lines: usize) -> usize {
    lines.max(1).to_string().len()
}

/// One source line as the cell shows it: a trailing `\r` dropped (a CRLF
/// file's line end), tabs expanded — the finished cell's display rule.
fn display_line(line: &str) -> String {
    expand_code_tabs(line.strip_suffix('\r').unwrap_or(line)).into_owned()
}

/// One numbered line's display rows — the finished file cell's own row
/// builder, so a draft's rows and the committed cell's are the same rows.
fn numbered_line(
    number: usize,
    digits: usize,
    segs: &[Seg],
    width: u16,
    max_rows: Option<usize>,
) -> Vec<Line<'static>> {
    numbered_row_lines(
        &format!("{number:>digits$}"),
        None,
        segs,
        &[],
        file_body_indent(),
        width,
        max_rows,
    )
}

/// How many drafts' memos one thread keeps — more than a round generates at
/// once. Past it the oldest (lowest id) goes.
const DRAFT_MEMO_MAX: usize = 8;

/// One draft's highlighting, carried across frames.
struct DraftMemo {
    /// The draft ([`DraftCall::id`]).
    id: u64,
    /// The language and theme `hl` was opened under — a change (the path
    /// landing after the content began, a `/theme` switch) starts over.
    lang: Option<String>,
    theme: Theme,
    /// The lexer state after every complete line in `content[..consumed]`.
    hl: Highlighter,
    /// The bytes of content `hl` has consumed — always just past a `\n`.
    consumed: usize,
    /// How many complete lines that is.
    done: usize,
    /// The first [`FILE_PEEK_LINES`] complete lines' segments — the head.
    head: Vec<Vec<Seg>>,
    /// The last [`FILE_PEEK_LINES`] complete lines' segments — the tail.
    tail: VecDeque<Vec<Seg>>,
    /// The body rows last built, with the signature they were built for.
    body: Option<(u64, Vec<Line<'static>>)>,
}

impl DraftMemo {
    fn new(id: u64, lang: Option<String>, theme: Theme) -> Self {
        let hl = Highlighter::new(lang.as_deref(), super::palette::palette().code);
        Self {
            id,
            lang,
            theme,
            hl,
            consumed: 0,
            done: 0,
            head: Vec::new(),
            tail: VecDeque::new(),
            body: None,
        }
    }

    /// Highlight every line `text` has completed since the last call — each
    /// exactly once.
    fn catch_up(&mut self, text: &str) {
        while let Some(newline) = text[self.consumed..].find('\n') {
            let line = &text[self.consumed..self.consumed + newline];
            let segs = self.hl.line(&display_line(line));
            #[cfg(test)]
            LINES_HIGHLIGHTED.with(|n| n.set(n.get() + 1));
            self.done += 1;
            if self.head.len() < FILE_PEEK_LINES {
                self.head.push(segs.clone());
            }
            self.tail.push_back(segs);
            if self.tail.len() > FILE_PEEK_LINES {
                self.tail.pop_front();
            }
            self.consumed += newline + 1;
        }
    }

    /// The body rows for `content` at `width`: the corner, then the tail
    /// while the string is open, the head once it has closed.
    fn build(&self, content: &Field, width: u16) -> Vec<Line<'static>> {
        let total = content.lines();
        let digits = digits_of(total);
        // The line still being typed, coloured from a copy of the carried
        // state, so the state itself resumes untouched when the line ends.
        let partial = &content.text[self.consumed..];
        let partial = (!partial.is_empty()).then(|| self.hl.clone().line(&display_line(partial)));
        let mut rows = vec![result_row(0, writing_label(Some(total)))];
        let row = |number: usize, segs: &[Seg]| {
            numbered_line(number, digits, segs, width, Some(TOOL_LINE_MAX_ROWS))
        };
        if content.complete {
            // The head, folded like the finished cell's peek: whole lines up
            // to the row budget, the first one always.
            let head = self.head.iter().enumerate().map(|(i, segs)| (i + 1, segs));
            let last = partial
                .as_ref()
                .filter(|_| self.done < FILE_PEEK_LINES)
                .map(|segs| (self.done + 1, segs));
            let mut used = 0usize;
            let mut shown = 0usize;
            for (number, segs) in head.chain(last) {
                let display = row(number, segs);
                if used + display.len() > FILE_PEEK_LINES && used > 0 {
                    break;
                }
                used += display.len();
                shown += 1;
                rows.extend(display);
            }
            let hidden = total.saturating_sub(shown);
            if hidden > 0 {
                rows.push(more_hint_line(hidden));
            }
        } else {
            // The tail, walked newest-first so the window ends on the line
            // being typed and fills upward within the same row budget.
            let newest = partial.as_ref().map(|segs| (self.done + 1, segs));
            let older = self
                .tail
                .iter()
                .rev()
                .enumerate()
                .map(|(i, segs)| (self.done - i, segs));
            let mut window: Vec<Vec<Line<'static>>> = Vec::new();
            let mut used = 0usize;
            for (number, segs) in newest.into_iter().chain(older) {
                let display = row(number, segs);
                if used + display.len() > FILE_PEEK_LINES && used > 0 {
                    break;
                }
                used += display.len();
                window.push(display);
            }
            rows.extend(window.into_iter().rev().flatten());
        }
        rows
    }
}

thread_local! {
    /// One set of memos per thread: the loop draws on one, and each test
    /// thread gets its own.
    static DRAFT_MEMOS: RefCell<Vec<DraftMemo>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    /// How many complete lines the memos have highlighted on this thread —
    /// the memo's own test hook ([`draft_lines_highlighted`]).
    static LINES_HIGHLIGHTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many complete lines the draft memos have highlighted on this thread.
/// The property the memo exists for is a *cost*, and a cost is only pinned by
/// counting it.
#[cfg(test)]
pub(crate) fn draft_lines_highlighted() -> u64 {
    LINES_HIGHLIGHTED.with(std::cell::Cell::get)
}

/// A draft's body rows — the corner and its numbered window — answered from
/// its memo when nothing it reads has moved.
fn body_lines(draft: &DraftCall, width: u16) -> Vec<Line<'static>> {
    let Some(content) = draft.content() else {
        return vec![result_row(0, writing_label(None))];
    };
    let lang = file_cell_lang(draft.path().unwrap_or_default()).map(str::to_string);
    let theme = super::palette::active_theme();
    let signature = {
        let mut hasher = DefaultHasher::new();
        content.text.len().hash(&mut hasher);
        content.complete.hash(&mut hasher);
        width.hash(&mut hasher);
        hasher.finish()
    };
    DRAFT_MEMOS.with(|memos| {
        let mut memos = memos.borrow_mut();
        let at = match memos.iter().position(|memo| memo.id == draft.id()) {
            // A draft's content only grows; anything else — a new grammar, a
            // new theme — starts its highlighting over.
            Some(at)
                if memos[at].lang == lang
                    && memos[at].theme == theme
                    && memos[at].consumed <= content.text.len() =>
            {
                at
            }
            Some(at) => {
                memos[at] = DraftMemo::new(draft.id(), lang, theme);
                at
            }
            None => {
                if memos.len() >= DRAFT_MEMO_MAX
                    && let Some(oldest) = memos
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, memo)| memo.id)
                        .map(|(at, _)| at)
                {
                    memos.remove(oldest);
                }
                memos.push(DraftMemo::new(draft.id(), lang, theme));
                memos.len() - 1
            }
        };
        let memo = &mut memos[at];
        if let Some((built, rows)) = &memo.body
            && *built == signature
        {
            return rows.clone();
        }
        memo.catch_up(&content.text);
        let rows = memo.build(content, width);
        memo.body = Some((signature, rows.clone()));
        rows
    })
}

//! Character-level refinement of a diff cell's `-`/`+` line pairs — which
//! characters of a changed line actually changed (`docs/inline-diff.md`).
//!
//! Pure: this module knows nothing about terminals, styles or [`ToolCall`]s.
//! It maps a parsed diff body's `(sign, text)` rows to the **byte ranges** of
//! each row that actually changed, and `ui::file_cell` turns those into the
//! brighter tint. Keeping it here — rather than beside `llm::tools`' line-level
//! [`diff_lines`](crate::llm::tools::diff_lines) — is what keeps the model-facing
//! output byte-identical: the refinement is derived from the rendered body, so
//! it lights up on rollouts recorded before it existed.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use super::theme::{INLINE_DIFF_MAX_CELLS, INLINE_DIFF_MIN_COMMON_PCT};

/// One body row as the refinement sees it.
#[derive(Debug, Clone, Copy)]
pub(super) enum RefineRow<'a> {
    /// A numbered row: its `+`/`-`/space sign and its content.
    Line(char, &'a str),
    /// Anything that interrupts a change run — a `⋮` hunk gap, a `…` cap note,
    /// or a line that didn't parse. Pairing never spans one: the two sides of
    /// a gap are lines apart in the file, so comparing them would invent a
    /// relationship the diff never claimed.
    Break,
}

/// The changed byte ranges of one refined `-`/`+` pair, each side indexed into
/// its own line. Either side is empty when that side only lost or only gained
/// text (a pure insertion marks nothing on the removed line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InlineDiff {
    pub(super) removed: Vec<Range<usize>>,
    pub(super) added: Vec<Range<usize>>,
}

/// Split a line into diff units: **one per grapheme cluster**.
///
/// Character granularity is the point. Marking whole words would colour text
/// the edit never touched — `Rivera` → `Rivero` differs in one letter, and
/// painting all six says the surname changed when only its last letter did.
/// A grapheme rather than a `char` so a combining mark or an emoji-ZWJ
/// sequence stays one unit, and so every range lands on a boundary the
/// renderer can slice at.
fn tokens(line: &str) -> Vec<Range<usize>> {
    line.grapheme_indices(true)
        .map(|(at, g)| at..at + g.len())
        .collect()
}

/// One token's text — a borrowed slice, never a copy: the trim below compares
/// every token of a line that can be tens of thousands long, per frame.
fn tok<'a>(line: &'a str, r: &Range<usize>) -> &'a str {
    &line[r.start..r.end]
}

/// The display columns of `text` that aren't whitespace — the unit the
/// similarity guard counts in, so a shared indent never reads as similarity.
///
/// Allocation-free on purpose: the guard sums this over every trimmed token of
/// a line that can be tens of thousands long, on a cell that re-renders each
/// animation frame.
fn ink(text: &str) -> usize {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.width().unwrap_or(0))
        .sum()
}

/// Coalesce **adjacent** changed units into contiguous runs, so a changed
/// `z(1)` paints as one block rather than four separate flecks of colour.
///
/// Adjacent only: a run of unchanged text between two changes is deliberately
/// never swallowed, however short. Bridging it would colour characters the
/// edit did not touch, which is the one thing the marking exists not to do —
/// the `0`s of `8080` → `9090` stay plain, and so does the `and beta = ` of
/// two values changed at opposite ends of a line.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Refine one removed/added line pair into the byte ranges that changed, or
/// `None` when the two lines are too dissimilar for the answer to mean
/// anything — see `docs/inline-diff.md` for why a replacement is deliberately
/// left flat.
pub(super) fn refine_pair(old: &str, new: &str) -> Option<InlineDiff> {
    if old == new {
        return None;
    }
    let (a, b) = (tokens(old), tokens(new));

    // Trim the common prefix and suffix — the whole reason this is affordable
    // per frame: a typical edit's changed middle is a token or two, however
    // long the line (`llm::tools::diff_lines` trims for the same reason).
    let prefix = a
        .iter()
        .zip(&b)
        .take_while(|(x, y)| tok(old, x) == tok(new, y))
        .count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| tok(old, x) == tok(new, y))
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    // Common ink from the trimmed ends, which every branch below shares.
    let mut common = a[..prefix]
        .iter()
        .chain(&a[a.len() - suffix..])
        .map(|r| ink(&old[r.clone()]))
        .sum::<usize>();

    // The guard's cheap half, run *before* the O(n×m) table. The LCS can only
    // match tokens present on both sides, so the smaller middle's ink bounds
    // what it can still add to `common`; when even that upper bound fails the
    // threshold the pair is a replacement and the table is wasted work. Purely
    // an optimization — every pair it rejects here, the guard below rejects
    // too — but it is what keeps an `edit` over a file of long, heavily
    // rewritten lines (a minified bundle, a CSS file) from paying a quadratic
    // table per row on every commit, repaint and transcript build.
    let longest = ink(old).max(ink(new));
    let middle_bound = am
        .iter()
        .map(|r| ink(&old[r.clone()]))
        .sum::<usize>()
        .min(bm.iter().map(|r| ink(&new[r.clone()])).sum::<usize>());
    if (common + middle_bound) * 100 < longest * INLINE_DIFF_MIN_COMMON_PCT {
        return None;
    }

    let (mut del, mut add): (Vec<Range<usize>>, Vec<Range<usize>>) = (Vec::new(), Vec::new());
    if am.len().saturating_mul(bm.len()) > INLINE_DIFF_MAX_CELLS {
        // Past the bound: the middle is marked whole rather than searched.
        del.extend(am.iter().cloned());
        add.extend(bm.iter().cloned());
    } else {
        // Longest common subsequence over the changed middle only.
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![0usize; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if tok(old, &am[i]) == tok(new, &bm[j]) {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if tok(old, &am[i]) == tok(new, &bm[j]) {
                common += ink(&old[am[i].clone()]);
                i += 1;
                j += 1;
            } else if lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1] {
                del.push(am[i].clone());
                i += 1;
            } else {
                add.push(bm[j].clone());
                j += 1;
            }
        }
        del.extend(am[i..].iter().cloned());
        add.extend(bm[j..].iter().cloned());
    }

    // The guard: refine only what is an *edit* of a line, not a replacement
    // of one. Measured in non-whitespace columns against the longer side.
    if common == 0 || common * 100 < longest * INLINE_DIFF_MIN_COMMON_PCT {
        return None;
    }
    Some(InlineDiff {
        removed: merge(del),
        added: merge(add),
    })
}

/// The changed byte ranges of every row of a parsed diff body — one entry per
/// input row, empty where nothing is refined.
///
/// Pairs each maximal run of consecutive `-` rows with the run of `+` rows
/// immediately following it, positionally and up to `min(n, m)`. git's
/// `diff-highlight` refuses a block whose two runs differ in length; we refine
/// the overlap and let [`refine_pair`]'s guard decide, because a `-1 +2` change
/// is usually still an edit plus an insertion — and a mispairing is harmless,
/// falling back to the flat tint the row had before.
pub(super) fn refine_rows(rows: &[RefineRow]) -> Vec<Vec<Range<usize>>> {
    let mut out = vec![Vec::new(); rows.len()];
    let mut i = 0usize;
    while i < rows.len() {
        // A run of removed rows…
        let del_start = i;
        while matches!(rows.get(i), Some(RefineRow::Line('-', _))) {
            i += 1;
        }
        let del_end = i;
        // …immediately followed by a run of added rows.
        while matches!(rows.get(i), Some(RefineRow::Line('+', _))) {
            i += 1;
        }
        let add_end = i;
        for k in 0..(del_end - del_start).min(add_end - del_end) {
            let (RefineRow::Line(_, old), RefineRow::Line(_, new)) =
                (rows[del_start + k], rows[del_end + k])
            else {
                continue;
            };
            if let Some(d) = refine_pair(old, new) {
                out[del_start + k] = d.removed;
                out[del_end + k] = d.added;
            }
        }
        // Neither run advanced (a context row, a gap, an unpaired `+` run):
        // step past it so the scan always makes progress.
        if i == del_start {
            i += 1;
        }
    }
    out
}

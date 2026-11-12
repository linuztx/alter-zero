//! The hook-run cell — what the hooks at a turn's edges said, `● Ran 1 Stop
//! hook` collapsed inline and expanded in Ctrl+O (`docs/tools/hooks.md` *The
//! hook-run cell*).
//!
//! It wears a tool cell's dress: a bullet coloured by outcome, `⎿` rows under
//! it — each opening its own corner, its continuations hanging under its text
//! ([`gutter_row`]) — and the output fold (`docs/ui/long-lines.md`). What it says
//! is the [`HookReport`] the runner built; nothing here decides a word of it.

use std::borrow::Cow;

use crate::hooks::{HookReport, HookSaid};

use super::assistant::expand_code_tabs;
use super::file_cell::gutter_row;
use super::theme::*;
use super::wrap::{cols, wrap_output};
use super::*;

/// One row of the cell's body before it wraps: its words and the colour they
/// wear.
struct Row<'a> {
    text: Cow<'a, str>,
    color: Color,
}

impl<'a> Row<'a> {
    /// A hook's row: the feedback, a stop and a message read as output — they
    /// are what the hook said — context is dim, being the model's and not
    /// the user's, and a hook that broke is red.
    fn said(said: &'a HookSaid) -> Self {
        Self::reading(said, Cow::Borrowed(said.row()))
    }

    /// `said`'s row in other words — the ones it reads in under its own
    /// hook's line ([`crate::hooks::RanHook::row_under`]) — in its colour.
    fn reading(said: &HookSaid, text: Cow<'a, str>) -> Self {
        let color = match said {
            HookSaid::Stopped(_) | HookSaid::Feedback(_) | HookSaid::Message(_) => {
                tool_output_color()
            }
            HookSaid::Context { .. } => tool_dim_color(),
            HookSaid::Error(_) => tool_fail_color(),
        };
        Self { text, color }
    }

    /// A configuration problem's row — red, like a hook that broke.
    fn warning(text: &'a str) -> Self {
        Self {
            text: Cow::Borrowed(text),
            color: tool_fail_color(),
        }
    }

    /// The row's display rows at `width`: the first under its own `⎿`
    /// corner, the rest hanging under its text — tabs spaced out, as a
    /// tool's output is, since a tab paints as nothing.
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        wrap_output(&expand_code_tabs(&self.text), width)
            .into_iter()
            .enumerate()
            .map(|(i, text)| gutter_row(i, text, Some(self.color)))
            .collect()
    }
}

/// The width a row's text wraps to beside the `⎿` gutter.
fn body_width(width: u16) -> u16 {
    width
        .saturating_sub(u16::try_from(cols(TOOL_RESULT_PREFIX)).unwrap_or(u16::MAX))
        .max(1)
}

/// The cell's header: the bullet — green when every hook ran cleanly, a
/// block included, red when one broke — over `Ran {n} {event} hook(s)`, the
/// count bold, and the dim `(ctrl+o to expand)` hint on the collapsed cell,
/// which always hides something: the hooks themselves. On one row when it
/// fits beside the bullet; on a terminal too narrow for that it wraps under
/// the bullet instead, since a committed row wider than the terminal is cut
/// at its edge — the hint, still dim, after the words on the last row it
/// fits on, or on rows of its own.
fn header_lines(report: &HookReport, width: u16, collapsed: bool) -> Vec<Line<'static>> {
    let color = if report.failed() {
        tool_fail_color()
    } else {
        tool_ok_color()
    };
    let bullet = super::tool::bullet_span(color, None);
    let dim = Style::new().fg(tool_dim_color());
    let count = report.hooks.len();
    let noun = if count == 1 {
        HOOK_CELL_HOOK
    } else {
        HOOK_CELL_HOOKS
    };
    let rest = format!(" {}{noun}", report.event);
    let hint = if collapsed { EXPAND_HINT } else { "" };
    let room = width.saturating_sub(BULLET_WIDTH).max(1);
    let words = format!("{HOOK_CELL_RAN}{count}{rest}");
    if cols(&words) + cols(hint) <= usize::from(room) {
        let mut spans = vec![
            bullet,
            Span::raw(HOOK_CELL_RAN.to_string()),
            Span::styled(count.to_string(), Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(rest),
        ];
        if collapsed {
            spans.push(Span::styled(hint.to_string(), dim));
        }
        return vec![Line::from(spans)];
    }
    let mut rows: Vec<Vec<Span<'static>>> = wrap_output(&words, room)
        .into_iter()
        .map(|row| vec![Span::raw(row.trim_end().to_string())])
        .collect();
    if collapsed {
        let last = rows.last().map_or(0, |spans| {
            spans.iter().map(|span| cols(&span.content)).sum::<usize>()
        });
        match rows.last_mut() {
            Some(spans) if last + cols(hint) <= usize::from(room) => {
                spans.push(Span::styled(hint.to_string(), dim));
            }
            _ => rows.extend(
                wrap_output(hint.trim_start(), room)
                    .into_iter()
                    .map(|row| vec![Span::styled(row.trim_end().to_string(), dim)]),
            ),
        }
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, spans)| {
            let lead = if i == 0 {
                bullet.clone()
            } else {
                Span::raw(INDENT.to_string())
            };
            Line::from(std::iter::once(lead).chain(spans).collect::<Vec<_>>())
        })
        .collect()
}

/// The dim `… +N lines` row under a folded cell — the tool cell's fold row,
/// without its `(ctrl+o to expand)`: the header already says it.
fn fold_line(hidden: usize) -> Line<'static> {
    let dim = Style::new().fg(tool_dim_color());
    Line::from(vec![
        Span::styled(TOOL_MORE_PREFIX.to_string(), dim),
        Span::styled(format!("+{hidden} lines"), dim),
    ])
}

/// The **collapsed** cell, inline and committed to scrollback: the header,
/// then what took effect, most consequential first — the stop, the feedback
/// the agent keeps going on, context, messages, then what broke and the
/// configuration's problems ([`HookReport::collapsed`]). Each row folds like
/// a tool cell's output: past `TOOL_FOLD_ROWS + 1` display rows it keeps its
/// first `TOOL_FOLD_ROWS` and counts the rest on a `… +N lines` row. Row by
/// row, never the body as a whole, so a long row cannot fold the rows after
/// it out of sight — a red bullet over an error row the fold hid.
#[must_use]
pub fn hook_report_lines(report: &HookReport, width: u16) -> Vec<Line<'static>> {
    let mut lines = header_lines(report, width, true);
    let rows = report
        .collapsed()
        .into_iter()
        .map(Row::said)
        .chain(report.warnings.iter().map(|warning| Row::warning(warning)));
    for row in rows {
        let row = row.lines(body_width(width));
        if row.len() <= TOOL_FOLD_ROWS + 1 {
            lines.extend(row);
        } else {
            let hidden = row.len() - TOOL_FOLD_ROWS;
            lines.extend(row.into_iter().take(TOOL_FOLD_ROWS));
            lines.push(fold_line(hidden));
        }
    }
    lines
}

/// The **expanded** cell, in the Ctrl+O transcript: every hook that ran, in
/// order — its command and how long it ran, the `hooks.json` it is
/// configured in — then what that hook said, a context row followed by the
/// text it added and an error row without the name the line above gives
/// ([`crate::hooks::RanHook::row_under`]); then the configuration's
/// problems. Never folded.
pub(super) fn hook_report_full_lines(report: &HookReport, width: u16) -> Vec<Line<'static>> {
    let wrap = body_width(width);
    let dim = Some(tool_dim_color());
    let mut lines = header_lines(report, width, false);
    for hook in &report.hooks {
        let ran = format!("{} ({})", hook.command, hook_duration(hook.millis));
        let mut rows = wrap_output(&expand_code_tabs(&ran), wrap);
        if let Some(config) = &hook.config {
            rows.extend(wrap_output(&format!("{HOOK_CELL_CONFIG}{config}"), wrap));
        }
        lines.extend(
            rows.into_iter()
                .enumerate()
                .map(|(i, text)| gutter_row(i, text, dim)),
        );
        for said in &hook.said {
            lines.extend(Row::reading(said, hook.row_under(said)).lines(wrap));
            if let HookSaid::Context { text, .. } = said {
                lines.extend(
                    wrap_output(&expand_code_tabs(text), wrap)
                        .into_iter()
                        .map(|text| gutter_row(1, text, dim)),
                );
            }
        }
    }
    for warning in &report.warnings {
        lines.extend(Row::warning(warning).lines(wrap));
    }
    lines
}

/// How long a hook ran, in the unit that reads: `412ms` under a second,
/// tenths below a minute (`1.2s`, cut rather than rounded, so `59.9s` never
/// reads as `60.0s`), then the status line's `1m 5s`.
pub(super) fn hook_duration(millis: u64) -> String {
    if millis < 1_000 {
        format!("{millis}ms")
    } else if millis < 60_000 {
        format!("{}.{}s", millis / 1_000, millis % 1_000 / 100)
    } else {
        format_elapsed(millis / 1_000)
    }
}

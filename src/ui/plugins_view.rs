//! The inline `/plugins` page (`docs/plugins.md` *The `/plugins` page*).
//!
//! The **list** is a search-line picker under the ask modal's chip strip of
//! three tabs — the installed plugins, every plugin the marketplaces list,
//! the marketplaces themselves with the row that adds any other — shedding
//! rows down the picker ladder on a short terminal so the search line and
//! the highlighted row stay on screen (`docs/view-flow.md` *A page that
//! sheds rows before it flows*). A **plugin's** or a **marketplace's** page
//! is the `/mcp` server page's twin — fact rows over numbered actions,
//! bottom-anchored when taller than the terminal — the **add** page is one
//! field, and the **confirmation** is two actions under its question.
//!
//! **A job is said in the hint's own row** once it has run
//! `PLUGINS_JOB_DELAY`, and what went right is never said: the rows and
//! pages show it — so acting on the page never moves it (`docs/plugins.md`
//! *No flicker*).
//!
//! One builder, [`plugins_build`], makes every page *with its caret*, so the
//! reserved height, the paint, the cursor seat and the flow all read the
//! same page.

use std::collections::BTreeMap;

use super::inline::wrap_inline;
use super::layout::text_field_width;
use super::mcp_view::{
    action_rows, field_line_at, field_lines_at, linked_field_lines_at, state_field_line_at,
    url_field_lines_at,
};
use super::model_view::{
    Description, age_text, compact_page, compact_page_with_header, first_page_that_fits,
    list_counter_line, list_window, marked_row_head, model_description_rows, model_linked_rows,
    model_placeholder_row, model_rule, model_wrapped_rows, picker_page, placeholder_window,
    row_head_cols, search_line, view_prompt_row,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

use crate::app::{
    MarketplaceRow, PluginsConfirm, PluginsMenu, PluginsPage, PluginsTab, format_elapsed,
    marketplace_actions, plugin_actions,
};
use crate::plugins::{CatalogMarketplace, CatalogPlugin, SOURCE_FORMS};

/// The widest a list row's name column grows, so a long name never pushes
/// the descriptions off the row.
const NAME_COL_MAX: usize = 32;

/// The narrowest a name column is cut to on a narrow terminal — while the
/// status mark keeps its seat beside it.
const NAME_COL_MIN: usize = 12;

/// The narrowest a row's description is cut to: with less room than that
/// it is left out whole, since a stub like `clau…` says nothing.
const ROW_TEXT_MIN: usize = 8;

/// The room a row's description keeps beside a marketplace column. With
/// less, the rows leave the marketplace to the facts under the list, which
/// name it for the highlighted row.
const ROW_MARKET_TEXT_MIN: usize = 30;

/// A built page, and its caret on a page that is typed into — the list's
/// search line, the add page's field — as `(column, page row)`. `None` on a
/// page of actions, whose hidden cursor seats on its `❯` instead.
pub(super) struct PluginsBuild {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor: Option<(u16, usize)>,
}

/// One block of a framed page and, when the caret sits in it, its
/// `(column, row within the block)`.
type Block = (Vec<Line<'static>>, Option<(usize, usize)>);

/// `{n} {noun}`, the noun plural for anything but one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A version as the page shows it: `v1.2.0`, never `vv1.2.0`.
fn version_label(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

/// The version to show for `plugin`: the installed one, else the one its
/// marketplace offers.
fn shown_version(plugin: &CatalogPlugin) -> Option<&str> {
    plugin
        .installed
        .as_ref()
        .and_then(|state| state.version.as_deref())
        .or(plugin.version.as_deref())
}

/// `text` with its runs of whitespace folded to single spaces — a
/// description that wraps in its manifest still fits one row.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line a running job shows — `Installing deploy… (4s · esc to
/// cancel)` — the label in the accent, the clause dim: the status line's
/// grammar. Cut at the width, never wrapped.
fn job_line(label: &str, secs: u64, width: u16) -> Line<'static> {
    clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(label.to_string(), Style::new().fg(model_selected_color())),
            Span::styled(
                format!(" ({} · {PLUGINS_JOB_CANCEL})", format_elapsed(secs)),
                Style::new().fg(model_meta_color()),
            ),
        ],
        usize::from(width),
    )
}

/// `clauses` as one hint row that fits `width`: every clause while they
/// fit, else the ones that give way first (the highest rank; `0` never)
/// left out whole — a hint never ends on a cut word, and always keeps the
/// key that closes the page.
fn fitted_hint(clauses: &[(&str, u8)], width: u16) -> Line<'static> {
    let mut kept: Vec<(&str, u8)> = clauses.to_vec();
    let text = |kept: &[(&str, u8)]| {
        kept.iter()
            .map(|(clause, _)| *clause)
            .collect::<Vec<_>>()
            .join(FOOTER_SEPARATOR)
    };
    while cols(MODEL_INDENT) + cols(&text(&kept)) > usize::from(width) {
        let Some(drop) = kept
            .iter()
            .enumerate()
            .filter(|(_, (_, rank))| *rank > 0)
            .max_by_key(|(_, (_, rank))| *rank)
            .map(|(i, _)| i)
        else {
            break;
        };
        kept.remove(drop);
    }
    model_placeholder_row(&text(&kept), model_meta_color(), width)
}

/// The list's hint for its tab: a plugin tab's keys, or the Marketplaces
/// tab's, whose rows open, add or ask for a source.
fn list_hint(menu: &PluginsMenu, width: u16) -> Line<'static> {
    let clauses: &[(&str, u8)] = match menu.tab {
        PluginsTab::Marketplaces => &[PLUGINS_HINT_TABS, PLUGINS_HINT_SELECT, PLUGINS_HINT_CLOSE],
        PluginsTab::Installed | PluginsTab::Discover => &[
            PLUGINS_HINT_TABS,
            PLUGINS_HINT_OPEN,
            PLUGINS_HINT_TOGGLE,
            PLUGINS_HINT_CLOSE,
        ],
    };
    fitted_hint(clauses, width)
}

/// The page's one status row, in the hint's own seat so it never moves the
/// page: the job running, once it has run long enough to be said; else
/// `hint`. What a job did is never said here — the rows and pages show it.
fn status_row(menu: &PluginsMenu, hint: Line<'static>, width: u16) -> Line<'static> {
    match menu.job.as_ref().filter(|_| menu.job_shown()) {
        Some(job) => job_line(&job.label, menu.job_elapsed.as_secs(), width),
        None => hint,
    }
}

/// The last failure, in red, wrapped — it is the instruction.
fn error_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    menu.error
        .as_deref()
        .map(|error| model_wrapped_rows(error, toast_error_color(), width))
        .unwrap_or_default()
}

/// The status row for a page with no room for the failure's own block (the
/// compact rungs): the failure in its seat, cut to one row, above all but
/// a job that is being said.
fn compact_status_row(menu: &PluginsMenu, hint: Line<'static>, width: u16) -> Line<'static> {
    match &menu.error {
        Some(error) if !menu.job_shown() => {
            model_placeholder_row(error, toast_error_color(), width)
        }
        _ => status_row(menu, hint, width),
    }
}

// ===== the list =====

/// A tab's name in the strip.
const fn tab_label(tab: PluginsTab) -> &'static str {
    match tab {
        PluginsTab::Installed => PLUGINS_TAB_INSTALLED,
        PluginsTab::Discover => PLUGINS_TAB_DISCOVER,
        PluginsTab::Marketplaces => PLUGINS_TAB_MARKETPLACES,
    }
}

/// The tab strip: the three names on one quiet line, ` · ` between them —
/// the current one in the accent and bold, the others dim — with no count
/// and no chip, so the line is one width whatever is installed. One too
/// wide for the terminal shows a window around the current tab, a dim
/// `‹`/`›` saying tabs are hidden on that side, so the current tab is
/// always on screen.
fn tab_strip(menu: &PluginsMenu, width: u16) -> Line<'static> {
    let tabs = menu.tabs();
    let labels: Vec<&'static str> = tabs.iter().map(|tab| tab_label(*tab)).collect();
    let current = tabs.iter().position(|tab| *tab == menu.tab).unwrap_or(0);
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT));
    let gap = cols(FOOTER_SEPARATOR);
    let span = |lo: usize, hi: usize| {
        let names: usize = labels[lo..=hi].iter().map(|label| cols(label)).sum();
        let mut total = names + gap * (hi - lo);
        if lo > 0 {
            total += cols(PLUGINS_STRIP_MORE_LEFT) + 1;
        }
        if hi + 1 < labels.len() {
            total += 1 + cols(PLUGINS_STRIP_MORE_RIGHT);
        }
        total
    };
    let (mut lo, mut hi) = (current, current);
    loop {
        let mut grew = false;
        if hi + 1 < labels.len() && span(lo, hi + 1) <= room {
            hi += 1;
            grew = true;
        }
        if lo > 0 && span(lo - 1, hi) <= room {
            lo -= 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let lit = Style::new()
        .fg(model_selected_color())
        .add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(model_meta_color());
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    if lo > 0 {
        spans.push(Span::styled(format!("{PLUGINS_STRIP_MORE_LEFT} "), dim));
    }
    for (i, label) in labels.iter().enumerate().take(hi + 1).skip(lo) {
        if i > lo {
            spans.push(Span::styled(FOOTER_SEPARATOR, dim));
        }
        spans.push(Span::styled(*label, if i == current { lit } else { dim }));
    }
    if hi + 1 < labels.len() {
        spans.push(Span::styled(format!(" {PLUGINS_STRIP_MORE_RIGHT}"), dim));
    }
    clamp_spans(spans, usize::from(width))
}

/// A marketplace's facts in one line: how many of its plugins are
/// installed, what it skipped, how long ago it was read, where it comes
/// from.
fn market_facts(market: &CatalogMarketplace, now: i64) -> String {
    let mut parts = vec![format!("{} installed", market.installed)];
    match market.notes.len() {
        0 => {}
        1 => parts.push("1 entry skipped".to_string()),
        n => parts.push(format!("{n} entries skipped")),
    }
    if let Some(at) = market.updated_at.filter(|_| now > 0) {
        parts.push(format!("updated {}", age_text(now - at)));
    }
    parts.push(market.source.clone());
    parts.join(FOOTER_SEPARATOR)
}

/// What a marketplace says it is: its description, else its owner — else,
/// in a row, where it comes from.
fn market_blurb(market: &CatalogMarketplace) -> Option<String> {
    market
        .description
        .as_deref()
        .map(flat)
        .filter(|text| !text.is_empty())
        .or_else(|| market.owner.as_ref().map(|owner| format!("By {owner}")))
}

/// The most skipped-entry notes a marketplace's page lists before it
/// counts the rest.
const MARKET_NOTES_MAX: usize = 5;

/// The list's header: the title, the notes about the whole page (plugins
/// off for the session, a `plugins.json` that will not parse) — wrapped,
/// since their tails are the fix — and the tab strip under them, where a
/// subtitle that only repeated the tab's name used to be. The same rows on
/// every tab, so switching tabs never moves the search line.
fn header_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![super::header::page_title_line(PLUGINS_TITLE, width)];
    if !menu.session_enabled {
        lines.extend(model_wrapped_rows(
            PLUGINS_SESSION_OFF,
            toast_error_color(),
            width,
        ));
    }
    if let Some(error) = menu.catalog.as_ref().and_then(|c| c.error.as_deref()) {
        lines.extend(model_wrapped_rows(error, toast_error_color(), width));
    }
    lines.push(tab_strip(menu, width));
    lines
}

/// A plugin's status in the list: its mark and the mark's colour, its word
/// and the word's colour. Only the `✔` is green and only the `✘` red; the
/// word keeps the `/settings` two-tone, light for a plugin that is in use.
fn plugin_status(plugin: &CatalogPlugin) -> (&'static str, Color, &'static str, Color) {
    let lit = settings_value_color();
    let off = settings_value_off_color();
    match &plugin.installed {
        Some(state) if state.error.is_some() => {
            (PLUGINS_BROKEN_MARK, tool_fail_color(), PLUGINS_BROKEN, lit)
        }
        Some(state) if state.enabled => (
            PLUGINS_ENABLED_MARK,
            model_active_color(),
            PLUGINS_ENABLED,
            lit,
        ),
        Some(_) => (
            PLUGINS_DISABLED_MARK,
            model_meta_color(),
            PLUGINS_DISABLED,
            off,
        ),
        None if plugin.unavailable.is_some() => (
            PLUGINS_UNAVAILABLE_MARK,
            model_meta_color(),
            PLUGINS_UNAVAILABLE,
            off,
        ),
        None => (
            PLUGINS_AVAILABLE_MARK,
            model_meta_color(),
            PLUGINS_AVAILABLE,
            off,
        ),
    }
}

/// A status's whole width: the mark, the space, the word.
fn status_cols(plugin: &CatalogPlugin) -> usize {
    let (mark, _, word, _) = plugin_status(plugin);
    cols(mark) + 1 + cols(word)
}

/// The name each row shows: the plugin's own — or, where two listed rows
/// share one (the same plugin in two marketplaces), the whole
/// `name@marketplace`, so the two can be told apart.
fn row_names(rows: &[&CatalogPlugin]) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for plugin in rows {
        *seen.entry(plugin.id.name.as_str()).or_default() += 1;
    }
    rows.iter()
        .map(|plugin| {
            if seen[plugin.id.name.as_str()] > 1 {
                plugin.id.key()
            } else {
                plugin.id.name.clone()
            }
        })
        .collect()
}

/// A plugin list's columns, sized off **every** listed row so they hold
/// still as ↑/↓ scroll the window.
#[derive(Clone, Copy, Debug)]
struct RowColumns {
    name: usize,
    status: usize,
    /// The marketplace column: where the rows come from more than one
    /// marketplace, and only where the description keeps
    /// [`ROW_MARKET_TEXT_MIN`] beside it.
    market: Option<usize>,
}

/// The columns of the plugin `rows`, named `names`, on a `width`-column
/// page. The name column is the widest name, capped at two fifths of what
/// the row has past its status — the descriptions keep the larger share,
/// and a name past the cap is `…`-cut, whole in the facts under the list —
/// and never so wide that the status mark loses its seat.
fn row_columns(rows: &[&CatalogPlugin], names: &[String], width: u16) -> RowColumns {
    let width = usize::from(width);
    let gap = SETTINGS_VALUE_GAP;
    let status = rows.iter().map(|p| status_cols(p)).max().unwrap_or(0);
    let past_status = width.saturating_sub(row_head_cols() + gap + status + gap);
    let cap = (past_status * 2 / 5)
        .clamp(NAME_COL_MIN, NAME_COL_MAX)
        .min(width.saturating_sub(row_head_cols() + gap + 1))
        .max(1);
    let name = names.iter().map(|n| cols(n)).max().unwrap_or(0).min(cap);
    // One marketplace's plugins alone — a browse, or the one added — need
    // no column repeating its name.
    let mixed = rows.iter().any(|p| {
        rows.first()
            .is_some_and(|first| first.id.marketplace != p.id.marketplace)
    });
    let market = mixed
        .then(|| {
            rows.iter()
                .map(|p| cols(&p.id.marketplace))
                .max()
                .unwrap_or(0)
        })
        .filter(|market| past_status.saturating_sub(name) >= market + gap + ROW_MARKET_TEXT_MIN);
    RowColumns {
        name,
        status,
        market,
    }
}

/// One plugin row: `→ {name}  {mark} {status}  [{marketplace}  ]{description}`
/// — the name, the status and the marketplace each in its column, an
/// update opening the description on an amber `▲ update ·`. The mark is the
/// state, so it keeps its seat however narrow the row: the word gives way
/// first, and what follows it is left out whole rather than cut to a stub.
/// A row never ends on a column's padding.
fn plugin_row(
    plugin: &CatalogPlugin,
    name: &str,
    selected: bool,
    columns: RowColumns,
    width: u16,
) -> Line<'static> {
    let name = ellipsize(name, columns.name.max(1));
    let pad = columns.name.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
    let mut spans = marked_row_head(&name, selected);
    spans.push(Span::raw(" ".repeat(pad)));
    let (mark, mark_color, word, word_color) = plugin_status(plugin);
    spans.push(Span::styled(mark, Style::new().fg(mark_color)));
    let used = row_head_cols() + cols(&name) + pad + cols(mark);
    let mut room = usize::from(width).saturating_sub(used);
    if room < 2 {
        return Line::from(spans);
    }
    let word = ellipsize(word, room - 1);
    room -= 1 + cols(&word);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(word.clone(), Style::new().fg(word_color)));
    let dim = Style::new().fg(model_meta_color());
    // The padding before the next thing shown, pushed only once it is.
    let mut gap = columns.status.saturating_sub(cols(mark) + 1 + cols(&word)) + SETTINGS_VALUE_GAP;
    if let Some(market_width) = columns.market
        && room > gap
    {
        room -= gap;
        let market = ellipsize(&plugin.id.marketplace, market_width.min(room).max(1));
        room -= cols(&market);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(market.clone(), dim));
        gap = market_width.saturating_sub(cols(&market)) + SETTINGS_VALUE_GAP;
    }
    let mut lead = "";
    if plugin.installed.as_ref().is_some_and(|state| state.update) {
        let update = format!("{PLUGINS_UPDATE_MARK} {PLUGINS_UPDATE_WORD}");
        if room >= gap + cols(&update) {
            room -= gap + cols(&update);
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(update, Style::new().fg(ask_warning_color())));
            gap = 0;
            lead = FOOTER_SEPARATOR;
        }
    }
    if let Some(description) = plugin
        .description
        .as_deref()
        .map(flat)
        .filter(|text| !text.is_empty())
    {
        let avail = room.saturating_sub(gap + cols(lead));
        if room > gap + cols(lead) && (cols(&description) <= avail || avail >= ROW_TEXT_MIN) {
            spans.push(Span::raw(" ".repeat(gap)));
            if !lead.is_empty() {
                spans.push(Span::styled(lead, dim));
            }
            spans.push(Span::styled(ellipsize(&description, avail), dim));
        }
    }
    Line::from(spans)
}

/// The window of the plugin `rows` that keeps the selection in view —
/// `max` rows — as how many rows it hides above, its rows and how many it
/// hides below, every row on the columns of [`row_columns`].
fn plugin_window(
    rows: &[&CatalogPlugin],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let names = row_names(rows);
    let columns = row_columns(rows, &names, width);
    let lines = range
        .map(|i| plugin_row(rows[i], &names[i], i == selected, columns, width))
        .collect();
    (above, lines, below)
}

/// One row of the Marketplaces tab, before the window knows its columns:
/// the name, the status (none on the add row) and what it is.
struct MarketEntry {
    name: String,
    /// The mark and its colour, the words beside it and theirs.
    status: Option<(&'static str, Color, String, Color)>,
    description: String,
}

/// What `row` shows: an added marketplace as `✔ {n} plugins` (a red `✘ not
/// read` when its catalog could not be read) beside what it is; an official
/// one not added as `+ available`, under the name it will have; the add row
/// as what its field takes.
fn market_entry(menu: &PluginsMenu, row: &MarketplaceRow) -> MarketEntry {
    let lit = settings_value_color();
    match row {
        MarketplaceRow::Market(name) => {
            let Some(market) = menu.marketplace(name) else {
                return MarketEntry {
                    name: name.clone(),
                    status: None,
                    description: String::new(),
                };
            };
            let status = if market.error.is_some() {
                (
                    PLUGINS_BROKEN_MARK,
                    tool_fail_color(),
                    PLUGINS_MARKET_BROKEN.to_string(),
                    lit,
                )
            } else {
                (
                    PLUGINS_ENABLED_MARK,
                    model_active_color(),
                    counted(market.plugins, "plugin"),
                    lit,
                )
            };
            MarketEntry {
                name: name.clone(),
                status: Some(status),
                description: market_blurb(market).unwrap_or_else(|| market.source.clone()),
            }
        }
        MarketplaceRow::Suggestion(suggestion) => MarketEntry {
            name: suggestion.name.to_string(),
            status: Some((
                PLUGINS_AVAILABLE_MARK,
                model_meta_color(),
                PLUGINS_AVAILABLE.to_string(),
                settings_value_off_color(),
            )),
            description: suggestion.description.to_string(),
        },
        MarketplaceRow::Add => MarketEntry {
            name: PLUGINS_ADD_ROW.to_string(),
            status: None,
            description: PLUGINS_ADD_ROW_DESCRIPTION.to_string(),
        },
    }
}

/// One Marketplaces row: `→ {name}  {mark} {status}  {description}`, the
/// plugin rows' grammar on the columns `(name, status)` — the add row's
/// status column blank — and what follows the status left out whole rather
/// than cut to a stub.
fn market_row(
    entry: &MarketEntry,
    selected: bool,
    columns: (usize, usize),
    width: u16,
) -> Line<'static> {
    let (name_width, status_width) = columns;
    let name = ellipsize(&entry.name, name_width.max(1));
    let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
    let mut spans = marked_row_head(&name, selected);
    let mut room = usize::from(width).saturating_sub(row_head_cols() + cols(&name));
    // The padding before the next thing shown, pushed only once it is.
    let mut gap = pad;
    if let Some((mark, mark_color, word, word_color)) = &entry.status {
        if room < gap + cols(mark) {
            return Line::from(spans);
        }
        room -= gap + cols(mark);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(*mark, Style::new().fg(*mark_color)));
        if room < 2 {
            return Line::from(spans);
        }
        let word = ellipsize(word, room - 1);
        room -= 1 + cols(&word);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(word.clone(), Style::new().fg(*word_color)));
        gap = status_width.saturating_sub(cols(mark) + 1 + cols(&word)) + SETTINGS_VALUE_GAP;
    } else {
        gap += status_width + SETTINGS_VALUE_GAP;
    }
    let description = flat(&entry.description);
    let avail = room.saturating_sub(gap);
    if !description.is_empty()
        && room > gap
        && (cols(&description) <= avail || avail >= ROW_TEXT_MIN)
    {
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(
            ellipsize(&description, avail),
            Style::new().fg(model_meta_color()),
        ));
    }
    Line::from(spans)
}

/// The Marketplaces tab's window — `max` rows riding the selection — as how
/// many rows it hides above, its rows and how many it hides below, every
/// row on columns sized off them all. A marketplace's name is what its
/// plugins' ids end in and the rows are few, so — unlike a plugin list's
/// ([`row_columns`]) — the name column takes all it needs while a
/// description keeps [`ROW_TEXT_MIN`] beside it.
fn market_window(
    menu: &PluginsMenu,
    rows: &[MarketplaceRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let entries: Vec<MarketEntry> = rows.iter().map(|row| market_entry(menu, row)).collect();
    let status = entries
        .iter()
        .filter_map(|entry| entry.status.as_ref())
        .map(|(mark, _, word, _)| cols(mark) + 1 + cols(word))
        .max()
        .unwrap_or(0);
    let gap = SETTINGS_VALUE_GAP;
    let past_status = usize::from(width).saturating_sub(row_head_cols() + gap + status + gap);
    let cap = past_status
        .saturating_sub(ROW_TEXT_MIN)
        .clamp(NAME_COL_MIN, NAME_COL_MAX)
        .min(usize::from(width).saturating_sub(row_head_cols() + gap + 1))
        .max(1);
    let name = entries
        .iter()
        .map(|entry| cols(&entry.name))
        .max()
        .unwrap_or(0)
        .min(cap);
    let lines = range
        .map(|i| market_row(&entries[i], i == selected, (name, status), width))
        .collect();
    (above, lines, below)
}

/// A plugin's facts in one line: its id, version and what it is made of —
/// or, for one that cannot be installed, why.
fn plugin_facts(plugin: &CatalogPlugin) -> String {
    let mut parts = vec![plugin.id.key()];
    if let Some(version) = shown_version(plugin) {
        parts.push(version_label(version));
    }
    match (&plugin.unavailable, &plugin.components) {
        (Some(reason), _) if plugin.installed.is_none() => parts.push(reason.clone()),
        (_, Some(summary)) => parts.push(summary.line()),
        _ => {}
    }
    parts.join(FOOTER_SEPARATOR)
}

/// What the list says under itself about the highlighted row, in the same
/// number of rows at every selection — so ↑/↓ never resize the page: the
/// description cut to one row, when `describe` (`No description` for one
/// with none, rather than giving the row back), over the facts. On the
/// Marketplaces tab: what a marketplace is (or why it could not be read)
/// over its facts, where an official one comes from, how a source is
/// written.
fn detail_lines(
    menu: &PluginsMenu,
    selected: usize,
    describe: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let (description, facts) = if menu.tab == PluginsTab::Marketplaces {
        match menu.marketplace_rows().get(selected) {
            Some(MarketplaceRow::Market(name)) => match menu.marketplace(name) {
                Some(market) => (
                    market
                        .error
                        .as_ref()
                        .map(|error| format!("{PLUGINS_BROKEN_MARK} {error}"))
                        .or_else(|| market_blurb(market))
                        .unwrap_or_else(|| PLUGINS_NO_DESCRIPTION.to_string()),
                    market_facts(market, menu.now),
                ),
                None => return Vec::new(),
            },
            Some(MarketplaceRow::Suggestion(suggestion)) => (
                suggestion.description.to_string(),
                format!("github.com/{}", suggestion.source),
            ),
            Some(MarketplaceRow::Add) => {
                (format!("Use {SOURCE_FORMS}"), PLUGINS_ADD_PIN.to_string())
            }
            None => return Vec::new(),
        }
    } else {
        match menu.plugin_rows().get(selected) {
            Some(plugin) => (
                plugin
                    .description
                    .as_deref()
                    .map(flat)
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| PLUGINS_NO_DESCRIPTION.to_string()),
                plugin_facts(plugin),
            ),
            None => return Vec::new(),
        }
    };
    let mut lines = Vec::new();
    if describe {
        lines.extend(model_description_rows(
            &description,
            Description::OneRow,
            width,
        ));
    }
    lines.extend(model_description_rows(&facts, Description::OneRow, width));
    lines
}

/// What the list shows where it has no row: the first read once it has run
/// long enough to be said (the job line itself, so its clock is where the
/// rows will be) — nothing before — a read that did not answer, a search
/// that matched nothing, an empty tab naming where to go.
fn placeholder_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let dim = model_meta_color();
    match (&menu.catalog, &menu.job) {
        (None, Some(job)) if menu.job_shown() => {
            vec![job_line(&job.label, menu.job_elapsed.as_secs(), width)]
        }
        (None, Some(_)) => Vec::new(),
        (None, None) => vec![model_placeholder_row(PLUGINS_NOT_READ, dim, width)],
        (Some(_), _) if !menu.query.is_empty() => {
            vec![model_placeholder_row(PLUGINS_NO_MATCH, dim, width)]
        }
        (Some(_), _) if menu.tab == PluginsTab::Installed => {
            model_wrapped_rows(PLUGINS_NONE_INSTALLED, dim, width)
        }
        (Some(_), _) => model_wrapped_rows(PLUGINS_NONE_LISTED, dim, width),
    }
}

/// The list page, the first of its pages that fits `height` rows
/// ([`first_page_that_fits`]) — the picker ladder:
///
/// 1. the **whole** page: the header (title, notes, tabs), the `❯` search
///    line, five rows, the `(n/total)` counter,
///    the highlighted row's description and its facts — a row each, so the
///    page is one height at every selection ([`detail_lines`]) — the last
///    failure, the status row;
/// 2. the whole page with the description gone, its facts kept;
/// 3. the **compact** page with the header kept: three rows between the
///    reserved `↑`/`↓` slots ([`compact_page_with_header`]);
/// 4. the compact page under the **tab strip** alone — the tabs outlast
///    the title, since ←/→ still move between them;
/// 5. the header-less compact page and its rungs ([`compact_page`]), down
///    to the search line alone.
///
/// The status row is always one row in the hint's seat (`status_row`):
/// what runs and what happened never move the page.
fn list_page(menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let header = header_lines(menu, width);
    let tabs = [tab_strip(menu, width)];
    let search = search_line(&menu.query);
    let hint = list_hint(menu, width);
    // Before the first read, its job line is where the rows will be: the
    // status row keeps the hint rather than saying it twice.
    let reading = menu.catalog.is_none();
    let mut tail = error_lines(menu, width);
    if !tail.is_empty() {
        tail.push(Line::default());
    }
    tail.push(if reading {
        hint.clone()
    } else {
        status_row(menu, hint.clone(), width)
    });
    let compact_tail = [if reading {
        hint
    } else {
        compact_status_row(menu, hint, width)
    }];
    let page = |mut body: Vec<Line<'static>>| {
        body.extend(tail.iter().cloned());
        picker_page(&header, &search, body, width)
    };

    let len = menu.row_count();
    if len == 0 {
        let block = placeholder_lines(menu, width);
        let window = |max| placeholder_window(&block, max);
        return first_page_that_fits(
            height,
            &[
                &|| {
                    let mut body = vec![Line::default()];
                    if !block.is_empty() {
                        body.extend(block.iter().cloned());
                        body.push(Line::default());
                    }
                    page(body)
                },
                &|| compact_page_with_header(&tabs, &search, &window, &compact_tail, width),
                &|| compact_page(height, &search, &window, &compact_tail, width),
            ],
        );
    }
    let selected = menu.selected.min(len - 1);
    let plugins = menu.plugin_rows();
    let markets = menu.marketplace_rows();
    let window = |max: usize| {
        if menu.tab == PluginsTab::Marketplaces {
            market_window(menu, &markets, selected, max, width)
        } else {
            plugin_window(&plugins, selected, max, width)
        }
    };
    let whole = |describe: bool| {
        let (_, rows, _) = window(usize::from(SETTINGS_MENU_MAX_ROWS));
        let mut body = vec![Line::default()];
        body.extend(rows);
        body.push(list_counter_line(selected, len));
        body.push(Line::default());
        let detail = detail_lines(menu, selected, describe, width);
        if !detail.is_empty() {
            body.extend(detail);
            body.push(Line::default());
        }
        body
    };
    first_page_that_fits(
        height,
        &[
            &|| page(whole(true)),
            &|| page(whole(false)),
            &|| compact_page_with_header(&header, &search, &window, &compact_tail, width),
            &|| compact_page_with_header(&tabs, &search, &window, &compact_tail, width),
            &|| compact_page(height, &search, &window, &compact_tail, width),
        ],
    )
}

// ===== the detail pages =====

/// `blocks` framed: the top rule, the non-empty blocks one blank row apart,
/// the bottom rule — no blank row hugging either rule (`docs/view-flow.md`
/// *The frame hugs its content*) — with the caret of the block that has it.
fn framed(blocks: Vec<Block>, width: u16) -> PluginsBuild {
    let mut lines = vec![model_rule(width)];
    let mut cursor = None;
    for (i, (block, seat)) in blocks
        .into_iter()
        .filter(|(block, _)| !block.is_empty())
        .enumerate()
    {
        if i > 0 {
            lines.push(Line::default());
        }
        if let Some((col, row)) = seat {
            cursor = Some((u16::try_from(col).unwrap_or(u16::MAX), lines.len() + row));
        }
        lines.extend(block);
    }
    lines.push(model_rule(width));
    PluginsBuild { lines, cursor }
}

/// A page's closing blocks: why the last job failed, then the status row —
/// the job, what it did, or the key hint, in one seat, so acting on the
/// page never moves it. The hint is its `clauses`, fitted to the width.
fn closing_blocks(menu: &PluginsMenu, clauses: &[(&str, u8)], width: u16) -> [Block; 2] {
    let hint = fitted_hint(clauses, width);
    [
        (error_lines(menu, width), None),
        (vec![status_row(menu, hint, width)], None),
    ]
}

/// The `Status:` row of a plugin's page: the mark in its state's colour over
/// what the state means here.
fn plugin_state(plugin: &CatalogPlugin) -> (&'static str, Color, &'static str) {
    match &plugin.installed {
        Some(state) if state.error.is_some() => (
            PLUGINS_BROKEN_MARK,
            tool_fail_color(),
            "installed, but not loaded",
        ),
        Some(state) if state.enabled => (
            PLUGINS_ENABLED_MARK,
            model_active_color(),
            "enabled in this directory",
        ),
        Some(_) => (
            PLUGINS_DISABLED_MARK,
            model_meta_color(),
            "disabled in this directory",
        ),
        None if plugin.unavailable.is_some() => (
            PLUGINS_UNAVAILABLE_MARK,
            model_meta_color(),
            "cannot be installed",
        ),
        None => (PLUGINS_AVAILABLE_MARK, model_meta_color(), "not installed"),
    }
}

/// The gap between a fact's label and its value on a plugin's page.
const FACT_LABEL_GAP: usize = 2;

/// One fact row of a plugin's page, before the page knows its label
/// column — which is the widest label it shows, not `/mcp`'s.
enum Fact {
    /// A bounded value on one row.
    Line(&'static str, String, Style),
    /// A value read whole: wrapped under its label.
    Wrapped(&'static str, String, Style),
    /// A value with addresses in it, each a link to itself.
    Linked(&'static str, String, Style),
    /// An address, a link to itself.
    Url(&'static str, String, Style),
    /// A mark in its colour over words.
    State(&'static str, &'static str, Color, String),
}

impl Fact {
    fn label(&self) -> &'static str {
        match self {
            Self::Line(label, ..)
            | Self::Wrapped(label, ..)
            | Self::Linked(label, ..)
            | Self::Url(label, ..)
            | Self::State(label, ..) => label,
        }
    }

    /// The fact's rows, its value at column `col` past the inset.
    fn lines(self, col: usize, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::Line(label, value, style) => {
                vec![field_line_at(col, label, &value, style, width)]
            }
            Self::Wrapped(label, value, style) => field_lines_at(col, label, &value, style, width),
            Self::Linked(label, value, style) => {
                linked_field_lines_at(col, label, &value, style, width)
            }
            Self::Url(label, value, style) => url_field_lines_at(col, label, &value, style, width),
            Self::State(label, mark, color, words) => {
                vec![state_field_line_at(col, label, mark, color, &words, width)]
            }
        }
    }
}

/// `clauses` laid out ` · `-separated in rows of `width`, each row inset:
/// a clause that does not fit after the one before it starts the next row
/// whole — never `… · by` over `42Crunch` — and only a clause wider than a
/// row is broken across rows.
fn clause_lines(clauses: Vec<Vec<Span<'static>>>, width: u16) -> Vec<Line<'static>> {
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT)).max(1);
    let dim = Style::new().fg(model_meta_color());
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for clause in clauses {
        let size: usize = clause.iter().map(|span| cols(&span.content)).sum();
        if used > 0 && used + cols(FOOTER_SEPARATOR) + size <= room {
            row.push(Span::styled(FOOTER_SEPARATOR, dim));
            row.extend(clause);
            used += cols(FOOTER_SEPARATOR) + size;
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        if size <= room {
            used = size;
            row = clause;
        } else {
            let segments: Vec<(String, Style)> = clause
                .iter()
                .map(|span| (span.content.to_string(), span.style))
                .collect();
            let mut pieces = wrap_inline(&segments, u16::try_from(room).unwrap_or(u16::MAX));
            row = pieces.pop().unwrap_or_default();
            used = row.iter().map(|span| cols(&span.content)).sum();
            rows.extend(pieces);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.into_iter()
        .map(|row| {
            let mut spans = vec![Span::raw(MODEL_INDENT)];
            spans.extend(row);
            Line::from(spans)
        })
        .collect()
}

/// A plugin's page: its name, then its state leading the line under it —
/// beside its id, version and author, broken between those clauses — and
/// its description, its addresses links; the facts on a label column of
/// their own: an update, format, source, where it loads from, homepage,
/// category and each kind of component by name; why it cannot be installed
/// or was not loaded, the trust warning for one not installed that runs or
/// may run something, what reading it said; then the numbered actions its
/// state affords.
fn plugin_page(app: &App, menu: &PluginsMenu, plugin: &CatalogPlugin, width: u16) -> PluginsBuild {
    let value = Style::new().fg(mcp_detail_value_color());
    let names = Style::new().fg(mcp_detail_state_color());
    let dim = Style::new().fg(model_meta_color());

    let mut head = vec![super::header::page_title_line(plugin.title(), width)];
    let (mark, mark_color, state) = plugin_state(plugin);
    let mut clauses = vec![
        vec![
            Span::styled(mark, Style::new().fg(mark_color)),
            Span::raw(" "),
            Span::styled(state, names),
        ],
        vec![Span::styled(plugin.id.key(), dim)],
    ];
    if let Some(version) = shown_version(plugin) {
        clauses.push(vec![Span::styled(version_label(version), dim)]);
    }
    if let Some(author) = &plugin.author {
        clauses.push(vec![Span::styled(format!("by {author}"), dim)]);
    }
    head.extend(clause_lines(clauses, width));
    if let Some(description) = plugin
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        head.push(Line::default());
        head.extend(model_linked_rows(
            description,
            mcp_description_color(),
            width,
        ));
    }

    let mut facts = Vec::new();
    if plugin.installed.as_ref().is_some_and(|state| state.update) {
        let offered = plugin.version.as_deref().map_or_else(
            || "a newer copy is available".to_string(),
            |version| format!("{} is available", version_label(version)),
        );
        facts.push(Fact::State(
            "Update:",
            PLUGINS_UPDATE_MARK,
            ask_warning_color(),
            offered,
        ));
    }
    if let Some(format) = plugin.format {
        facts.push(Fact::Line("Format:", format.label().to_string(), value));
    }
    facts.push(Fact::Linked("Source:", plugin.source.clone(), value));
    // Loaded in place: its folder in its marketplace is the plugin.
    if plugin
        .installed
        .as_ref()
        .is_some_and(|state| state.in_place)
    {
        let folder = menu.marketplace(&plugin.id.marketplace).map_or_else(
            || std::path::PathBuf::from(&plugin.source),
            |market| market.root.join(plugin.source.trim_start_matches("./")),
        );
        let shown = app.path_display().display(&folder.display().to_string());
        facts.push(Fact::Wrapped("Loads from:", shown, value));
    }
    if let Some(homepage) = &plugin.homepage {
        facts.push(Fact::Url("Homepage:", homepage.clone(), value));
    }
    if let Some(category) = &plugin.category {
        facts.push(Fact::Line("Category:", category.clone(), value));
    }
    match &plugin.components {
        Some(summary) => {
            let kinds = [
                ("Skills:", &summary.skills),
                ("Commands:", &summary.commands),
                ("Agents:", &summary.agents),
                ("Hooks:", &summary.hooks),
                ("MCP servers:", &summary.servers),
            ];
            let mut any = false;
            for (label, list) in kinds {
                if !list.is_empty() {
                    any = true;
                    facts.push(Fact::Wrapped(label, list.join(", "), names));
                }
            }
            if !any {
                facts.push(Fact::Line("Contents:", summary.line(), value));
            }
            if !summary.unused.is_empty() {
                facts.push(Fact::Wrapped(
                    "Not used here:",
                    summary.unused.join(", "),
                    value,
                ));
            }
        }
        None => facts.push(Fact::Line(
            "Contents:",
            "read once it is installed".to_string(),
            value,
        )),
    }
    let col = facts
        .iter()
        .map(|fact| cols(fact.label()))
        .max()
        .unwrap_or(0)
        + FACT_LABEL_GAP;
    let facts: Vec<Line<'static>> = facts
        .into_iter()
        .flat_map(|fact| fact.lines(col, width))
        .collect();

    let mut notes = Vec::new();
    if let Some(error) = plugin.installed.as_ref().and_then(|s| s.error.as_deref()) {
        notes.extend(model_wrapped_rows(error, toast_error_color(), width));
    }
    if let Some(reason) = &plugin.unavailable {
        notes.extend(model_wrapped_rows(
            &super::model_view::capitalized(reason),
            model_meta_color(),
            width,
        ));
    }
    // Not installed, so not yet reviewed: one that runs something — or whose
    // contents are read only once it is installed — warns before Install.
    if plugin.installed.is_none()
        && plugin.unavailable.is_none()
        && plugin
            .components
            .as_ref()
            .is_none_or(crate::plugins::ComponentSummary::runs_code)
    {
        notes.extend(model_wrapped_rows(
            PLUGINS_TRUST_NOTE,
            model_meta_color(),
            width,
        ));
    }
    for note in &plugin.notes {
        notes.extend(model_wrapped_rows(note, model_meta_color(), width));
    }

    let labels: Vec<&'static str> = plugin_actions(plugin)
        .into_iter()
        .map(crate::app::PluginAction::label)
        .collect();
    let [status, hint] = closing_blocks(menu, &PLUGINS_PAGE_HINT, width);
    framed(
        vec![
            (head, None),
            (facts, None),
            (notes, None),
            (action_rows(&labels, menu.selected, width), None),
            status,
            hint,
        ],
        width,
    )
}

/// A marketplace's page: its name, then how many plugins it lists — or
/// that it could not be read — beside how many are installed and how long
/// ago it was read; its description, its addresses links; its facts —
/// source, revision, where its copy is kept; why it could not be read and
/// the entries it skipped; then the numbered actions it affords.
fn marketplace_page(
    app: &App,
    menu: &PluginsMenu,
    market: &CatalogMarketplace,
    width: u16,
) -> PluginsBuild {
    let value = Style::new().fg(mcp_detail_value_color());
    let names = Style::new().fg(mcp_detail_state_color());
    let dim = Style::new().fg(model_meta_color());

    let mut head = vec![super::header::page_title_line(&market.name, width)];
    let state = if market.error.is_some() {
        vec![
            Span::styled(PLUGINS_BROKEN_MARK, Style::new().fg(tool_fail_color())),
            Span::raw(" "),
            Span::styled(PLUGINS_MARKET_BROKEN, names),
        ]
    } else {
        vec![
            Span::styled(PLUGINS_ENABLED_MARK, Style::new().fg(model_active_color())),
            Span::raw(" "),
            Span::styled(counted(market.plugins, "plugin"), names),
        ]
    };
    let mut clauses = vec![state];
    clauses.push(vec![Span::styled(
        format!("{} installed", market.installed),
        dim,
    )]);
    if let Some(at) = market.updated_at.filter(|_| menu.now > 0) {
        clauses.push(vec![Span::styled(
            format!("updated {}", age_text(menu.now - at)),
            dim,
        )]);
    }
    head.extend(clause_lines(clauses, width));
    if let Some(blurb) = market_blurb(market) {
        head.push(Line::default());
        head.extend(model_linked_rows(&blurb, mcp_description_color(), width));
    }

    let mut facts = vec![Fact::Linked("Source:", market.source.clone(), value)];
    if let Some(revision) = &market.revision {
        facts.push(Fact::Line(
            "Revision:",
            revision.chars().take(12).collect(),
            value,
        ));
    }
    facts.push(Fact::Wrapped(
        "Location:",
        app.path_display()
            .display(&market.root.display().to_string()),
        value,
    ));
    let col = facts
        .iter()
        .map(|fact| cols(fact.label()))
        .max()
        .unwrap_or(0)
        + FACT_LABEL_GAP;
    let facts: Vec<Line<'static>> = facts
        .into_iter()
        .flat_map(|fact| fact.lines(col, width))
        .collect();

    let mut notes = Vec::new();
    if let Some(error) = &market.error {
        notes.extend(model_wrapped_rows(error, toast_error_color(), width));
    }
    for note in market.notes.iter().take(MARKET_NOTES_MAX) {
        notes.extend(model_wrapped_rows(note, ask_warning_color(), width));
    }
    if market.notes.len() > MARKET_NOTES_MAX {
        notes.push(model_placeholder_row(
            &format!("… {} more skipped", market.notes.len() - MARKET_NOTES_MAX),
            model_meta_color(),
            width,
        ));
    }

    let labels: Vec<&'static str> = marketplace_actions(market)
        .into_iter()
        .map(crate::app::MarketplaceAction::label)
        .collect();
    let [status, hint] = closing_blocks(menu, &PLUGINS_PAGE_HINT, width);
    framed(
        vec![
            (head, None),
            (facts, None),
            (notes, None),
            (action_rows(&labels, menu.selected, width), None),
            status,
            hint,
        ],
        width,
    )
}

/// The add page: what a marketplace is, the one field — its dim
/// placeholder until something is typed, then the text wrapped with the
/// caret in it — the forms a source takes and how to pin one, the failure
/// and the status row.
fn add_page(menu: &PluginsMenu, width: u16) -> PluginsBuild {
    let dim = model_meta_color();
    let mut head = vec![super::header::page_title_line(PLUGINS_ADD_TITLE, width)];
    head.extend(model_wrapped_rows(PLUGINS_ADD_BLURB, dim, width));
    let lead = cols(MODEL_INDENT) + cols(MODEL_PROMPT);
    let room = usize::from(width).saturating_sub(lead).max(1);
    let prompt = Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color()));
    let (field, caret) = if menu.input.is_empty() {
        (
            vec![Line::from(vec![
                Span::raw(MODEL_INDENT),
                prompt,
                Span::styled(
                    ellipsize(PLUGINS_ADD_PLACEHOLDER, room),
                    Style::new().fg(dim),
                ),
            ])],
            (lead, 0),
        )
    } else {
        let wrap = text_field_width(u16::try_from(room).unwrap_or(u16::MAX));
        let (row, col) = menu.input.cursor_row_col(wrap);
        let rows = menu
            .input
            .display_rows(wrap)
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let lead_span = if i == 0 {
                    prompt.clone()
                } else {
                    Span::raw(" ".repeat(cols(MODEL_PROMPT)))
                };
                Line::from(vec![
                    Span::raw(MODEL_INDENT),
                    lead_span,
                    Span::styled(text, Style::new().fg(model_id_color())),
                ])
            })
            .collect();
        (rows, (lead + col, row))
    };
    let mut forms = model_wrapped_rows(&format!("Use {SOURCE_FORMS}."), dim, width);
    forms.extend(model_wrapped_rows(PLUGINS_ADD_PIN, dim, width));
    let [status, hint] = closing_blocks(menu, &PLUGINS_ADD_HINT, width);
    framed(
        vec![
            (head, None),
            (field, Some(caret)),
            (forms, None),
            status,
            hint,
        ],
        width,
    )
}

/// The question before something that cannot be undone — the answer the
/// user just chose highlighted, so Enter confirms it and Esc goes back.
fn confirm_page(menu: &PluginsMenu, confirm: &PluginsConfirm, width: u16) -> PluginsBuild {
    let (title, text, yes) = match confirm {
        PluginsConfirm::Uninstall(id) => (
            format!("Uninstall {}?", id.name),
            "Its files and the data it kept are deleted. It can be installed again from its marketplace."
                .to_string(),
            "Uninstall",
        ),
        PluginsConfirm::RemoveMarketplace { name, plugins } => (
            format!("Remove the {name} marketplace?"),
            if plugins.is_empty() {
                "No plugin is installed from it.".to_string()
            } else {
                format!(
                    "It uninstalls the plugins installed from it: {}.",
                    plugins.join(", ")
                )
            },
            "Remove",
        ),
    };
    let mut head = vec![super::header::page_title_line(&title, width)];
    head.extend(model_wrapped_rows(&text, model_meta_color(), width));
    let [status, hint] = closing_blocks(menu, &PLUGINS_CONFIRM_HINT, width);
    framed(
        vec![
            (head, None),
            (action_rows(&[yes, "Cancel"], menu.selected, width), None),
            status,
            hint,
        ],
        width,
    )
}

// ===== the whole =====

/// The page as lines and its caret (see [`PluginsBuild`]): the list's first
/// page that fits `height` rows, or a detail page — those bottom-anchor when
/// taller than the terminal, their actions and status row staying on
/// screen. `height` is the terminal's, or the region already reserved for
/// the page, which builds the same page. Empty when the page is closed. A
/// page whose plugin or marketplace has just gone shows the list, as the
/// page will once the answer that removed it is taken.
pub(super) fn plugins_build(app: &App, width: u16, height: u16) -> PluginsBuild {
    let Some(menu) = app.plugins_menu.as_ref() else {
        return PluginsBuild {
            lines: Vec::new(),
            cursor: None,
        };
    };
    let list = || {
        let lines = list_page(menu, width, height);
        let cursor = view_prompt_row(&lines).map(|row| {
            let col = cols(MODEL_INDENT) + cols(MODEL_PROMPT) + cols(&menu.query);
            (u16::try_from(col).unwrap_or(u16::MAX), usize::from(row))
        });
        PluginsBuild { lines, cursor }
    };
    match &menu.page {
        PluginsPage::List => list(),
        PluginsPage::Add => add_page(menu, width),
        PluginsPage::Confirm(confirm) => confirm_page(menu, confirm, width),
        PluginsPage::Plugin(id) => match menu.plugin(id) {
            Some(plugin) => plugin_page(app, menu, plugin, width),
            None => list(),
        },
        PluginsPage::Marketplace(name) => match menu.marketplace(name) {
            Some(market) => marketplace_page(app, menu, market, width),
            None => list(),
        },
    }
}

/// The page as lines (`plugins_build`'s) — what [`render_plugins_menu`]
/// paints, what `plugins_menu_rows` counts and what flows
/// (`docs/view-flow.md`). Empty when the page is closed.
#[must_use]
pub fn plugins_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    plugins_build(app, width, height).lines
}

/// The rows the page's own frame occupies on a terminal `height` rows tall.
pub(super) fn plugins_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(plugins_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height while `/plugins` is open, or `None` when it
/// is not. Like every sibling page it replaces the composer — and only the
/// composer: the streaming strip keeps its rows above it. Clamped to the
/// terminal height.
#[must_use]
pub fn plugins_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.plugins_menu.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        plugins_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the inline `/plugins` page into the live region, in place of the
/// composer — bottom-anchored, so a detail page taller than the area keeps
/// its actions and hint on screen while its top flows. Pure: `render_live`
/// paints this. See `docs/plugins.md`.
pub fn render_plugins_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        plugins_view_lines(app, area.width, area.height),
    );
}

//! The inline `/plugins` page (`docs/plugins.md` *The `/plugins` page*) —
//! codex's `/plugins` in this app's frame.
//!
//! The **list** is a search-line picker under the ask modal's chip strip of
//! tabs — every plugin, the installed ones, a tab per marketplace (its own
//! header saying where it comes from and what it skipped), and the add tab,
//! whose field is the search line and whose rows are the official catalogs —
//! shedding rows down the picker ladder on a short terminal so the search
//! line and the highlighted row stay on screen (`docs/view-flow.md` *A page
//! that sheds rows before it flows*). A **plugin's** page is the `/mcp`
//! server page's twin — fact rows over numbered actions, bottom-anchored
//! when taller than the terminal — and the **confirmation** is two actions
//! under its question.
//!
//! **What runs and what happened are said in the hint's row** — a job once
//! it has run `PLUGINS_JOB_DELAY`, a success until the next key — so acting
//! on the page never moves it (`docs/plugins.md` *No flicker*).
//!
//! One builder, [`plugins_build`], makes every page *with its caret*, so the
//! reserved height, the paint, the cursor seat and the flow all read the
//! same page.

use std::collections::BTreeMap;

use super::mcp_view::{action_rows, field_line, field_lines, state_field_line, url_field_lines};
use super::model_view::{
    Description, age_text, compact_page, compact_page_with_header, first_page_that_fits,
    list_counter_line, list_window, marked_row_head, model_description_rows, model_placeholder_row,
    model_rule, model_wrapped_rows, picker_page, placeholder_window, row_head_cols, search_line,
    view_prompt_row,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

use crate::app::{
    AddRow, PluginsConfirm, PluginsMenu, PluginsPage, PluginsTab, format_elapsed, plugin_actions,
};
use crate::plugins::{CatalogMarketplace, CatalogPlugin, SOURCE_FORMS};

/// The widest a list row's name column grows, so a long name never pushes
/// the descriptions off the row.
const NAME_COL_MAX: usize = 32;

/// A built page, and its caret on a page that is typed into — the list's
/// search line, which is the add tab's field too — as `(column, page row)`.
/// `None` on a page of actions, whose hidden cursor seats on its `❯`
/// instead.
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

/// The list's hint for its tab: a marketplace's tab leads on its own keys
/// (Ctrl+U only for a git one, which is all an update can fetch), the add
/// tab says how to add.
fn list_hint(menu: &PluginsMenu, width: u16) -> Line<'static> {
    let mut clauses = Vec::new();
    match &menu.tab {
        PluginsTab::Add => {
            clauses.extend([
                PLUGINS_HINT_TABS,
                PLUGINS_HINT_TYPE,
                PLUGINS_HINT_ADD,
                PLUGINS_HINT_CLOSE,
            ]);
            return fitted_hint(&clauses, width);
        }
        PluginsTab::Marketplace(_) => {
            if menu.current_marketplace().is_some_and(|market| market.git) {
                clauses.push(PLUGINS_HINT_UPDATE);
            }
            clauses.push(PLUGINS_HINT_REMOVE);
        }
        PluginsTab::All | PluginsTab::Installed => {}
    }
    clauses.extend([
        PLUGINS_HINT_TABS,
        PLUGINS_HINT_OPEN,
        PLUGINS_HINT_TOGGLE,
        PLUGINS_HINT_CLOSE,
    ]);
    fitted_hint(&clauses, width)
}

/// The page's one status row, in the hint's own seat so it never moves the
/// page: the job running, once it has run long enough to be said; else what
/// the last one did, until the next key; else `hint`.
fn status_row(menu: &PluginsMenu, hint: Line<'static>, width: u16) -> Line<'static> {
    if let Some(job) = menu.job.as_ref().filter(|_| menu.job_shown()) {
        return job_line(&job.label, menu.job_elapsed.as_secs(), width);
    }
    if let Some(notice) = &menu.notice {
        return clamp_spans(
            vec![
                Span::raw(MODEL_INDENT),
                Span::styled(PLUGINS_ENABLED_MARK, Style::new().fg(model_active_color())),
                Span::raw(" "),
                Span::styled(notice.clone(), Style::new().fg(settings_value_color())),
            ],
            usize::from(width),
        );
    }
    hint
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

/// A tab's chip label: its name, and how many rows it would list once the
/// catalog is read.
fn tab_label(menu: &PluginsMenu, tab: &PluginsTab) -> String {
    let catalog = menu.catalog.as_ref();
    let (name, count) = match tab {
        PluginsTab::All => (
            PLUGINS_TAB_ALL.to_string(),
            catalog.map(|c| c.plugins.len()),
        ),
        PluginsTab::Installed => (
            PLUGINS_TAB_INSTALLED.to_string(),
            catalog.map(crate::plugins::PluginCatalog::installed_count),
        ),
        PluginsTab::Marketplace(name) => (
            name.clone(),
            catalog.map(|c| {
                c.plugins
                    .iter()
                    .filter(|p| p.id.marketplace == *name)
                    .count()
            }),
        ),
        PluginsTab::Add => (PLUGINS_TAB_ADD.to_string(), None),
    };
    match count {
        Some(count) => format!(" {name} ({count}) "),
        None => format!(" {name} "),
    }
}

/// The tab strip: a chip per tab, the current one lit in the accent — the
/// ask modal's strip. One too wide for the terminal shows a window around
/// the current tab, a dim `‹`/`›` saying tabs are hidden on that side
/// (codex's), so the current tab is always on screen.
fn tab_strip(menu: &PluginsMenu, width: u16) -> Line<'static> {
    let tabs = menu.tabs();
    let labels: Vec<String> = tabs.iter().map(|tab| tab_label(menu, tab)).collect();
    let current = tabs.iter().position(|tab| *tab == menu.tab).unwrap_or(0);
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT));
    let gap = cols(ASK_CHIP_GAP);
    let span = |lo: usize, hi: usize| {
        let chips: usize = labels[lo..=hi].iter().map(|label| cols(label)).sum();
        let mut total = chips + gap * (hi - lo);
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
        .fg(ask_chip_current_fg())
        .bg(ask_chip_current_bg());
    let idle = Style::new().fg(ask_chip_color());
    let dim = Style::new().fg(model_meta_color());
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    if lo > 0 {
        spans.push(Span::styled(format!("{PLUGINS_STRIP_MORE_LEFT} "), dim));
    }
    for (i, label) in labels.iter().enumerate().take(hi + 1).skip(lo) {
        if i > lo {
            spans.push(Span::raw(ASK_CHIP_GAP));
        }
        spans.push(Span::styled(
            label.clone(),
            if i == current { lit } else { idle },
        ));
    }
    if hi + 1 < labels.len() {
        spans.push(Span::styled(format!(" {PLUGINS_STRIP_MORE_RIGHT}"), dim));
    }
    clamp_spans(spans, usize::from(width))
}

/// A marketplace's counts in one line: how many plugins it lists, how many
/// are installed, how long ago it was read.
fn market_counts(market: &CatalogMarketplace, now: i64) -> String {
    let mut parts = vec![
        counted(market.plugins, "plugin"),
        format!("{} installed", market.installed),
    ];
    if let Some(at) = market.updated_at.filter(|_| now > 0) {
        parts.push(format!("updated {}", age_text(now - at)));
    }
    parts.join(FOOTER_SEPARATOR)
}

/// Where a marketplace comes from in one line: its source, and the commit
/// its checkout is at.
fn market_source(market: &CatalogMarketplace) -> String {
    match &market.revision {
        Some(revision) => format!(
            "{}{FOOTER_SEPARATOR}{}",
            market.source,
            revision.chars().take(7).collect::<String>()
        ),
        None => market.source.clone(),
    }
}

/// The most rows a marketplace's skipped-entry note takes in its header.
const MARKET_NOTE_MAX_ROWS: usize = 3;

/// The current tab's dim subtitle and its count line — and, for a
/// marketplace, why its catalog could not be read (red) or what it skipped.
fn tab_header(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let dim = model_meta_color();
    let row = |text: &str, color: Color| model_placeholder_row(&flat(text), color, width);
    let catalog = menu.catalog.as_ref();
    match &menu.tab {
        PluginsTab::All => {
            let count = catalog.map(|catalog| match catalog.marketplaces.len() {
                0 => "no marketplace added — add one under + Add".to_string(),
                n => format!(
                    "{} installed{FOOTER_SEPARATOR}{} in {}",
                    catalog.installed_count(),
                    counted(catalog.plugins.len(), "plugin"),
                    counted(n, "marketplace")
                ),
            });
            let mut lines = vec![row(PLUGINS_ALL_SUBTITLE, dim)];
            lines.extend(count.map(|count| row(&count, dim)));
            lines
        }
        PluginsTab::Installed => {
            let count = catalog.map(|catalog| {
                let on = catalog.plugins.iter().filter(|p| p.is_enabled()).count();
                format!(
                    "{} installed{FOOTER_SEPARATOR}{on} on in this directory",
                    catalog.installed_count()
                )
            });
            let mut lines = vec![row(PLUGINS_INSTALLED_SUBTITLE, dim)];
            lines.extend(count.map(|count| row(&count, dim)));
            lines
        }
        PluginsTab::Marketplace(_) => {
            let Some(market) = menu.current_marketplace() else {
                return Vec::new();
            };
            let subtitle = market
                .description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .map_or_else(
                    || match &market.owner {
                        Some(owner) => format!("By {owner}"),
                        None => format!("Plugins from {}", market.source),
                    },
                    str::to_string,
                );
            let mut lines = vec![row(&subtitle, dim)];
            match &market.error {
                Some(error) => lines.push(row(
                    &format!("{PLUGINS_BROKEN_MARK} {error}"),
                    toast_error_color(),
                )),
                None => lines.push(row(&market_counts(market, menu.now), dim)),
            }
            lines.push(row(&market_source(market), dim));
            if let Some(first) = market.notes.first() {
                let skipped = if market.notes.len() == 1 {
                    "1 entry skipped".to_string()
                } else {
                    format!("{} entries skipped", market.notes.len())
                };
                // The first one's reason, whole (it is the fix) but bounded,
                // without the parser's own `entry 4 skipped:` lead-in.
                let reason = first
                    .split_once(" skipped: ")
                    .filter(|(lead, _)| lead.starts_with("entry "))
                    .map_or(first.as_str(), |(_, reason)| reason);
                let mut note = model_wrapped_rows(
                    &format!("{skipped} — {reason}"),
                    ask_warning_color(),
                    width,
                );
                note.truncate(MARKET_NOTE_MAX_ROWS);
                lines.extend(note);
            }
            lines
        }
        PluginsTab::Add => vec![row(PLUGINS_ADD_SUBTITLE, dim), row(PLUGINS_ADD_COUNT, dim)],
    }
}

/// The list's header: the title, the tab's subtitle and counts, the notes
/// about the whole page (plugins off for the session, a `plugins.json` that
/// will not parse) — wrapped, since their tails are the fix — and the tab
/// strip under a gap.
fn header_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![super::header::page_title_line(PLUGINS_TITLE, width)];
    lines.extend(tab_header(menu, width));
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
    lines.push(Line::default());
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

/// The name column: the widest listed name, capped so the status always
/// keeps its seat and a long name never crowds out the descriptions.
fn name_column(names: &[String], status_width: usize, width: u16) -> usize {
    let cap = usize::from(width)
        .saturating_sub(row_head_cols() + SETTINGS_VALUE_GAP + status_width)
        .clamp(1, NAME_COL_MAX);
    names
        .iter()
        .map(|name| cols(name))
        .max()
        .unwrap_or(0)
        .min(cap)
}

/// One plugin row: `→ {name}  {mark} {status}  {description}` — the name
/// column and the status column aligned down the list, an update opening
/// the description on an amber `▲ update ·`, the marketplace naming itself
/// on the tabs that list every one (`show_market`), everything past the
/// status `…`-cut. The mark is the state, so it keeps its seat however narrow the
/// row; the word and the description give way first.
fn plugin_row(
    plugin: &CatalogPlugin,
    name: &str,
    selected: bool,
    columns: (usize, usize),
    show_market: bool,
    width: u16,
) -> Line<'static> {
    let (name_width, status_width) = columns;
    let name = ellipsize(name, name_width.max(1));
    let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
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
    let status_pad = status_width.saturating_sub(cols(mark) + 1 + cols(&word)) + SETTINGS_VALUE_GAP;
    if room <= status_pad {
        return Line::from(spans);
    }
    room -= status_pad;
    spans.push(Span::raw(" ".repeat(status_pad)));
    if plugin.installed.as_ref().is_some_and(|state| state.update) {
        let update = ellipsize(
            &format!("{PLUGINS_UPDATE_MARK} {PLUGINS_UPDATE_WORD}"),
            room,
        );
        room -= cols(&update);
        spans.push(Span::styled(update, Style::new().fg(ask_warning_color())));
        if room <= cols(FOOTER_SEPARATOR) {
            return Line::from(spans);
        }
        room -= cols(FOOTER_SEPARATOR);
        spans.push(Span::styled(
            FOOTER_SEPARATOR,
            Style::new().fg(model_meta_color()),
        ));
    }
    // Where every marketplace is listed, a row names its own.
    let mut text = if show_market {
        format!("{}{FOOTER_SEPARATOR}", plugin.id.marketplace)
    } else {
        String::new()
    };
    if let Some(description) = plugin.description.as_deref() {
        text.push_str(&flat(description));
    }
    let text = text.trim_end_matches(FOOTER_SEPARATOR).to_string();
    if !text.is_empty() {
        spans.push(Span::styled(
            ellipsize(&text, room),
            Style::new().fg(model_meta_color()),
        ));
    }
    Line::from(spans)
}

/// The window of the plugin `rows` that keeps the selection in view —
/// `max` rows — as how many rows it hides above, its rows and how many it
/// hides below. The columns are sized off **every** listed row, so they
/// hold still as ↑/↓ scroll the window.
fn plugin_window(
    rows: &[&CatalogPlugin],
    selected: usize,
    max: usize,
    show_market: bool,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let names = row_names(rows);
    let status_width = rows.iter().map(|p| status_cols(p)).max().unwrap_or(0);
    let name_width = name_column(&names, status_width, width);
    let lines = range
        .map(|i| {
            plugin_row(
                rows[i],
                &names[i],
                i == selected,
                (name_width, status_width),
                show_market,
                width,
            )
        })
        .collect();
    (above, lines, below)
}

/// The add tab's window: the typed source, then the official catalogs —
/// `→ {source}  {✔ added}  {what it is}`, the sources and the marks each in
/// their column.
fn add_window(
    menu: &PluginsMenu,
    rows: &[AddRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let source = |row: &AddRow| match row {
        AddRow::Typed(text) => text.clone(),
        AddRow::Suggestion(suggestion) => suggestion.source.to_string(),
    };
    let added = format!("{PLUGINS_ENABLED_MARK} {PLUGINS_ADDED}");
    let cap = usize::from(width)
        .saturating_sub(row_head_cols() + SETTINGS_VALUE_GAP)
        .clamp(1, 40);
    let name_width = rows
        .iter()
        .map(|row| cols(&source(row)))
        .max()
        .unwrap_or(0)
        .min(cap);
    let any_added = rows
        .iter()
        .any(|row| matches!(row, AddRow::Suggestion(s) if menu.is_added(s)));
    let lines = range
        .map(|i| {
            let row = &rows[i];
            let name = ellipsize(&source(row), name_width.max(1));
            let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
            let mut spans = marked_row_head(&name, i == selected);
            spans.push(Span::raw(" ".repeat(pad)));
            let mut used = row_head_cols() + cols(&name) + pad;
            let (is_added, description) = match row {
                AddRow::Typed(_) => (false, PLUGINS_ADD_TYPED),
                AddRow::Suggestion(suggestion) => {
                    (menu.is_added(suggestion), suggestion.description)
                }
            };
            if any_added {
                if is_added {
                    spans.push(Span::styled(
                        PLUGINS_ENABLED_MARK,
                        Style::new().fg(model_active_color()),
                    ));
                    spans.push(Span::styled(
                        format!(" {PLUGINS_ADDED}"),
                        Style::new().fg(settings_value_color()),
                    ));
                } else {
                    spans.push(Span::raw(" ".repeat(cols(&added))));
                }
                spans.push(Span::raw(" ".repeat(SETTINGS_VALUE_GAP)));
                used += cols(&added) + SETTINGS_VALUE_GAP;
            }
            let room = usize::from(width).saturating_sub(used);
            if room > 0 {
                spans.push(Span::styled(
                    ellipsize(description, room),
                    Style::new().fg(model_meta_color()),
                ));
            }
            Line::from(spans)
        })
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

/// What the list says under itself about the highlighted row: a plugin's
/// description and its facts; on the add tab, what the highlighted source
/// is. The description is laid out by `layout`; the facts are always one
/// row, so the page's one-row rung is one height at every selection.
fn detail_lines(
    menu: &PluginsMenu,
    selected: usize,
    layout: Description,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut describe = |description: Option<&str>, facts: String| {
        if layout == Description::Wrapped
            && let Some(text) = description.filter(|text| !text.trim().is_empty())
        {
            lines.extend(model_description_rows(text, layout, width));
        }
        lines.extend(model_description_rows(&facts, Description::OneRow, width));
    };
    if menu.tab == PluginsTab::Add {
        match menu.add_rows().get(selected) {
            Some(AddRow::Typed(_)) => describe(None, format!("Use {SOURCE_FORMS}")),
            Some(AddRow::Suggestion(suggestion)) => describe(
                Some(suggestion.description),
                format!(
                    "github.com/{}{FOOTER_SEPARATOR}its catalog is `{}`",
                    suggestion.source, suggestion.name
                ),
            ),
            None => {}
        }
    } else if let Some(plugin) = menu.plugin_rows().get(selected) {
        describe(plugin.description.as_deref(), plugin_facts(plugin));
    }
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
/// 1. the **whole** page: the header (title, the tab's subtitle and counts,
///    notes, tabs), the `❯` search line, five rows, the `(n/total)` counter,
///    the highlighted row's description wrapped and its facts, the last
///    failure, the status row;
/// 2. the whole page with the description gone — one height at every
///    selection, so a terminal at the edge never flips to the compact page
///    as ↑/↓ cross a long description;
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
    let adds = menu.add_rows();
    let show_market = matches!(menu.tab, PluginsTab::All | PluginsTab::Installed);
    let window = |max: usize| {
        if menu.tab == PluginsTab::Add {
            add_window(menu, &adds, selected, max, width)
        } else {
            plugin_window(&plugins, selected, max, show_market, width)
        }
    };
    let whole = |layout: Description| {
        let (_, rows, _) = window(usize::from(SETTINGS_MENU_MAX_ROWS));
        let mut body = vec![Line::default()];
        body.extend(rows);
        body.push(list_counter_line(selected, len));
        body.push(Line::default());
        let detail = detail_lines(menu, selected, layout, width);
        if !detail.is_empty() {
            body.extend(detail);
            body.push(Line::default());
        }
        body
    };
    first_page_that_fits(
        height,
        &[
            &|| page(whole(Description::Wrapped)),
            &|| page(whole(Description::OneRow)),
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
/// page never moves it.
fn closing_blocks(menu: &PluginsMenu, hint: &str, width: u16) -> [Block; 2] {
    let hint = model_placeholder_row(hint, model_meta_color(), width);
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

/// A plugin's page: its name over its id, version and author, and its
/// description; the fact rows — status, an update, format, source,
/// homepage, and each kind of component by name; why it cannot be installed
/// or read, the review note for one that brings something that runs, what
/// reading it said; then the numbered actions its state affords.
fn plugin_page(app: &App, menu: &PluginsMenu, plugin: &CatalogPlugin, width: u16) -> PluginsBuild {
    let value = Style::new().fg(mcp_detail_value_color());
    let names = Style::new().fg(mcp_detail_state_color());

    let mut head = vec![super::header::page_title_line(plugin.title(), width)];
    let mut ident = vec![plugin.id.key()];
    if let Some(version) = shown_version(plugin) {
        ident.push(version_label(version));
    }
    if let Some(author) = &plugin.author {
        ident.push(format!("by {author}"));
    }
    head.extend(model_wrapped_rows(
        &ident.join(FOOTER_SEPARATOR),
        model_meta_color(),
        width,
    ));
    if let Some(description) = plugin
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        head.push(Line::default());
        head.extend(model_wrapped_rows(
            description,
            mcp_description_color(),
            width,
        ));
    }

    let (mark, mark_color, words) = plugin_state(plugin);
    let mut facts = vec![state_field_line("Status:", mark, mark_color, words, width)];
    if plugin.installed.as_ref().is_some_and(|state| state.update) {
        let offered = plugin.version.as_deref().map_or_else(
            || "a newer copy is available".to_string(),
            |version| format!("{} is available", version_label(version)),
        );
        facts.push(state_field_line(
            "Update:",
            PLUGINS_UPDATE_MARK,
            ask_warning_color(),
            &offered,
            width,
        ));
    }
    if let Some(format) = plugin.format {
        facts.push(field_line("Format:", format.label(), value, width));
    }
    facts.extend(field_lines("Source:", &plugin.source, value, width));
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
        facts.extend(field_lines("Loads from:", &shown, value, width));
    }
    if let Some(homepage) = &plugin.homepage {
        facts.extend(url_field_lines("Homepage:", homepage, value, width));
    }
    if let Some(category) = &plugin.category {
        facts.push(field_line("Category:", category, value, width));
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
                    facts.extend(field_lines(label, &list.join(", "), names, width));
                }
            }
            if !any {
                facts.push(field_line("Contents:", &summary.line(), value, width));
            }
            if !summary.unused.is_empty() {
                facts.extend(field_lines(
                    "Not used here:",
                    &summary.unused.join(", "),
                    value,
                    width,
                ));
            }
        }
        None => facts.push(field_line(
            "Contents:",
            "read once it is installed",
            value,
            width,
        )),
    }

    let dim = model_meta_color();
    let mut notes = Vec::new();
    if let Some(error) = plugin.installed.as_ref().and_then(|s| s.error.as_deref()) {
        notes.extend(model_wrapped_rows(error, toast_error_color(), width));
    }
    if let Some(reason) = &plugin.unavailable {
        notes.extend(model_wrapped_rows(
            &super::model_view::capitalized(reason),
            dim,
            width,
        ));
    }
    if plugin.installed.is_none()
        && plugin.unavailable.is_none()
        && plugin
            .components
            .as_ref()
            .is_some_and(|summary| summary.runs_code())
    {
        notes.extend(model_wrapped_rows(PLUGINS_TRUST_NOTE, dim, width));
    }
    for note in &plugin.notes {
        notes.extend(model_wrapped_rows(note, dim, width));
    }

    let labels: Vec<&'static str> = plugin_actions(plugin)
        .into_iter()
        .map(crate::app::PluginAction::label)
        .collect();
    let [status, hint] = closing_blocks(menu, PLUGINS_PAGE_HINT, width);
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

/// The question before something that cannot be undone, the safe answer
/// highlighted when it opens.
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
    let [status, hint] = closing_blocks(menu, PLUGINS_CONFIRM_HINT, width);
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
/// plugin's page whose plugin has just gone shows the list, as the page will
/// once the answer that removed it is taken.
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
        PluginsPage::Confirm(confirm) => confirm_page(menu, confirm, width),
        PluginsPage::Plugin(id) => match menu.plugin(id) {
            Some(plugin) => plugin_page(app, menu, plugin, width),
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

//! The inline `/plugins` page (`docs/plugins.md` *The `/plugins` page*) —
//! codex's `/plugins` in this app's frame.
//!
//! The **list** is a search-line picker: the `/skills` menu's rows under the
//! ask modal's chip strip of tabs, shedding rows down the picker ladder on a
//! short terminal so the search line and the highlighted row stay on screen
//! (`docs/view-flow.md` *A page that sheds rows before it flows*). A
//! **plugin's** or a **marketplace's** page is the `/mcp` server page's twin
//! — fact rows over numbered actions, bottom-anchored when taller than the
//! terminal — the **confirmation** is two actions under its question, and
//! the **add** page is one field.
//!
//! One builder, [`plugins_build`], makes every page *with its caret*, so the
//! reserved height, the paint, the cursor seat and the flow all read the
//! same page.

use std::collections::BTreeMap;

use super::layout::text_field_width;
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
    MarketplaceRow, PluginsConfirm, PluginsMenu, PluginsPage, PluginsTab, format_elapsed,
    marketplace_actions, plugin_actions,
};
use crate::plugins::{CatalogMarketplace, CatalogPlugin, SOURCE_FORMS};

/// The widest a list row's name column grows, so a long name never pushes
/// the descriptions off the row.
const NAME_COL_MAX: usize = 32;

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

/// What the page says about its operations: the job running now, else the
/// last one's failure in red, wrapped (it is the instruction), else nothing.
/// The list's first read is shown by the placeholder instead, which says it
/// where the rows will be.
fn status_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    if let Some(job) = &menu.job {
        if shows_reading_placeholder(menu) {
            return Vec::new();
        }
        return vec![job_line(&job.label, menu.job_secs, width)];
    }
    menu.error
        .as_deref()
        .map(|error| model_wrapped_rows(error, toast_error_color(), width))
        .unwrap_or_default()
}

/// Is the list showing its no-catalog placeholder — a plugin tab before the
/// first read has answered?
fn shows_reading_placeholder(menu: &PluginsMenu) -> bool {
    menu.catalog.is_none() && menu.page == PluginsPage::List && menu.tab != PluginsTab::Marketplaces
}

// ===== the list =====

/// The tab strip: ` Installed (2) `, ` Discover (4) `, ` Marketplaces (2) `,
/// the current tab a chip lit in the accent — the ask modal's strip — each
/// count once the catalog is read. Cut at the width, never wrapped.
fn tab_strip(menu: &PluginsMenu, width: u16) -> Line<'static> {
    let counts = menu.catalog.as_ref().map(|catalog| {
        [
            catalog.installed_count(),
            catalog.plugins.len(),
            catalog.marketplaces.len(),
        ]
    });
    let current = Style::new()
        .fg(ask_chip_current_fg())
        .bg(ask_chip_current_bg());
    let idle = Style::new().fg(ask_chip_color());
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    for (i, (tab, name)) in PluginsTab::ALL.iter().zip(PLUGINS_TAB_NAMES).enumerate() {
        if i > 0 {
            spans.push(Span::raw(ASK_CHIP_GAP));
        }
        let label = match counts {
            Some(counts) => format!(" {name} ({}) ", counts[i]),
            None => format!(" {name} "),
        };
        spans.push(Span::styled(
            label,
            if *tab == menu.tab { current } else { idle },
        ));
    }
    clamp_spans(spans, usize::from(width))
}

/// The header's dim count line — `2 installed · 4 plugins in 2
/// marketplaces` — once the catalog is read.
fn count_line(menu: &PluginsMenu, width: u16) -> Option<Line<'static>> {
    let catalog = menu.catalog.as_ref()?;
    let listed = match catalog.marketplaces.len() {
        0 => "no marketplace added".to_string(),
        n => format!(
            "{} in {}",
            counted(catalog.plugins.len(), "plugin"),
            counted(n, "marketplace")
        ),
    };
    Some(model_placeholder_row(
        &format!(
            "{} installed{FOOTER_SEPARATOR}{listed}",
            catalog.installed_count()
        ),
        model_meta_color(),
        width,
    ))
}

/// The list's header: the title, the count, the notes about the whole page
/// (plugins off for the session, a `plugins.json` that will not parse) —
/// wrapped, since their tails are the fix — and the tab strip under a gap.
fn header_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![super::header::page_title_line(PLUGINS_TITLE, width)];
    lines.extend(count_line(menu, width));
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
/// the description on an amber `▲ update ·`, everything past the status
/// `…`-cut. The mark is the state, so it keeps its seat however narrow the
/// row; the word and the description give way first.
fn plugin_row(
    plugin: &CatalogPlugin,
    name: &str,
    selected: bool,
    columns: (usize, usize),
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
    if let Some(description) = plugin.description.as_deref() {
        spans.push(Span::styled(
            ellipsize(&flat(description), room),
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
                width,
            )
        })
        .collect();
    (above, lines, below)
}

/// One marketplace row: `→ {name}  {n} plugins · {k} installed · {source}`,
/// or its name over a red `✘` and why its catalog could not be read.
fn market_row(
    market: &CatalogMarketplace,
    selected: bool,
    name_width: usize,
    width: u16,
) -> Line<'static> {
    let name = ellipsize(&market.name, name_width.max(1));
    let pad = name_width.saturating_sub(cols(&name)) + SETTINGS_VALUE_GAP;
    let mut spans = marked_row_head(&name, selected);
    spans.push(Span::raw(" ".repeat(pad)));
    let room = usize::from(width).saturating_sub(row_head_cols() + cols(&name) + pad);
    let dim = Style::new().fg(model_meta_color());
    if let Some(error) = &market.error {
        spans.push(Span::styled(
            PLUGINS_BROKEN_MARK,
            Style::new().fg(tool_fail_color()),
        ));
        if room > cols(PLUGINS_BROKEN_MARK) + 1 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                ellipsize(&flat(error), room - cols(PLUGINS_BROKEN_MARK) - 1),
                dim,
            ));
        }
        return Line::from(spans);
    }
    let mut count = counted(market.plugins, "plugin");
    if market.installed > 0 {
        count.push_str(&format!("{FOOTER_SEPARATOR}{} installed", market.installed));
    }
    let count = ellipsize(&count, room);
    let rest = room.saturating_sub(cols(&count));
    spans.push(Span::styled(count, Style::new().fg(settings_value_color())));
    if rest > cols(FOOTER_SEPARATOR) {
        spans.push(Span::styled(
            format!(
                "{FOOTER_SEPARATOR}{}",
                ellipsize(&market.source, rest - cols(FOOTER_SEPARATOR))
            ),
            dim,
        ));
    }
    Line::from(spans)
}

/// The Marketplaces tab's window: its marketplace rows and the add row.
fn market_window(
    menu: &PluginsMenu,
    rows: &[MarketplaceRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let markets: &[CatalogMarketplace] = menu
        .catalog
        .as_ref()
        .map_or(&[], |catalog| catalog.marketplaces.as_slice());
    let (above, range, below) = list_window(rows.len(), selected, max);
    let cap = usize::from(width)
        .saturating_sub(row_head_cols() + SETTINGS_VALUE_GAP + 8)
        .clamp(1, NAME_COL_MAX);
    let name_width = rows
        .iter()
        .filter_map(|row| match row {
            MarketplaceRow::Market(i) => markets.get(*i).map(|m| cols(&m.name)),
            MarketplaceRow::Add => None,
        })
        .max()
        .unwrap_or(0)
        .min(cap);
    let lines = range
        .map(|i| match rows[i] {
            MarketplaceRow::Market(index) => markets.get(index).map_or_else(Line::default, |m| {
                market_row(m, i == selected, name_width, width)
            }),
            MarketplaceRow::Add => {
                let room = usize::from(width).saturating_sub(row_head_cols()).max(1);
                Line::from(marked_row_head(
                    &ellipsize(PLUGINS_ADD_ROW, room),
                    i == selected,
                ))
            }
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

/// A marketplace's facts in one line: its source, the commit its checkout
/// is at, and how long ago it was read.
fn market_facts(market: &CatalogMarketplace, now: i64) -> String {
    let mut parts = vec![market.source.clone()];
    if let Some(revision) = &market.revision {
        parts.push(revision.chars().take(7).collect());
    }
    if let Some(at) = market.updated_at.filter(|_| now > 0) {
        parts.push(format!("updated {}", age_text(now - at)));
    }
    parts.join(FOOTER_SEPARATOR)
}

/// What the list says under itself about the highlighted row: the plugin's
/// description and its facts, a marketplace's description and its facts,
/// what the add row takes. The description is laid out by `layout`; the
/// facts are always one row, so the page's one-row rung is one height at
/// every selection.
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
    if menu.tab == PluginsTab::Marketplaces {
        match menu.marketplace_rows().get(selected) {
            Some(MarketplaceRow::Market(index)) => {
                if let Some(market) = menu
                    .catalog
                    .as_ref()
                    .and_then(|c| c.marketplaces.get(*index))
                {
                    describe(
                        market.description.as_deref(),
                        market_facts(market, menu.now),
                    );
                }
            }
            Some(MarketplaceRow::Add) => describe(None, format!("Add {SOURCE_FORMS}")),
            None => {}
        }
    } else if let Some(plugin) = menu.plugin_rows().get(selected) {
        describe(plugin.description.as_deref(), plugin_facts(plugin));
    }
    lines
}

/// What the list shows where it has no row: the first read (the job line
/// itself, so its clock is where the rows will be), a read that did not
/// answer, a search that matched nothing, an empty tab naming where to go.
fn placeholder_lines(menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let dim = model_meta_color();
    match (&menu.catalog, &menu.job) {
        (None, Some(job)) => vec![job_line(&job.label, menu.job_secs, width)],
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
/// 1. the **whole** page: the header (title, count, notes, tabs), the `❯`
///    search line, five rows, the `(n/total)` counter, the highlighted
///    row's description wrapped and its facts, the job line or the last
///    failure, the key hint;
/// 2. the whole page with the description gone — one height at every
///    selection, so a terminal at the edge never flips to the compact page
///    as ↑/↓ cross a long description;
/// 3. the **compact** page with the header kept: three rows between the
///    reserved `↑`/`↓` slots ([`compact_page_with_header`]);
/// 4. the compact page under the **tab strip** alone — the tabs outlast
///    the title, since ←/→ still move between them;
/// 5. the header-less compact page and its rungs ([`compact_page`]), down
///    to the search line alone.
fn list_page(menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let header = header_lines(menu, width);
    let tabs = [tab_strip(menu, width)];
    let search = search_line(&menu.query);
    let hint = if menu.tab == PluginsTab::Marketplaces {
        PLUGINS_MARKETS_HINT
    } else {
        PLUGINS_LIST_HINT
    };
    let mut tail = status_lines(menu, width);
    if !tail.is_empty() {
        tail.push(Line::default());
    }
    tail.push(model_placeholder_row(hint, model_meta_color(), width));
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
                    body.extend(block.iter().cloned());
                    body.push(Line::default());
                    page(body)
                },
                &|| compact_page_with_header(&tabs, &search, &window, &tail, width),
                &|| compact_page(height, &search, &window, &tail, width),
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
            &|| compact_page_with_header(&header, &search, &window, &tail, width),
            &|| compact_page_with_header(&tabs, &search, &window, &tail, width),
            &|| compact_page(height, &search, &window, &tail, width),
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

/// A page's closing blocks: what its job is doing (or why the last one
/// failed), then the key hint.
fn closing_blocks(menu: &PluginsMenu, hint: &str, width: u16) -> [Block; 2] {
    [
        (status_lines(menu, width), None),
        (
            vec![model_placeholder_row(hint, model_meta_color(), width)],
            None,
        ),
    ]
}

/// The `Status:` row of a plugin's page: the mark in its state's colour over
/// what the state means here.
fn plugin_state(plugin: &CatalogPlugin) -> (&'static str, Color, &'static str) {
    match &plugin.installed {
        Some(state) if state.error.is_some() => (
            PLUGINS_BROKEN_MARK,
            tool_fail_color(),
            "installed, but its files cannot be read",
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
fn plugin_page(menu: &PluginsMenu, plugin: &CatalogPlugin, width: u16) -> PluginsBuild {
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

/// The most of a marketplace's skipped entries its page lists; the rest
/// are counted.
const MARKET_NOTES_MAX: usize = 5;

/// A marketplace's page: its name over its counts and owner, and its
/// description; its source, revision, age and where its files are; why it
/// could not be read and what it skipped; then its actions.
fn marketplace_page(
    app: &App,
    menu: &PluginsMenu,
    market: &CatalogMarketplace,
    width: u16,
) -> PluginsBuild {
    let value = Style::new().fg(mcp_detail_value_color());
    let dim = model_meta_color();
    let mut head = vec![super::header::page_title_line(&market.name, width)];
    let mut counts = vec![counted(market.plugins, "plugin")];
    if market.installed > 0 {
        counts.push(format!("{} installed", market.installed));
    }
    if let Some(owner) = &market.owner {
        counts.push(format!("by {owner}"));
    }
    head.extend(model_wrapped_rows(
        &counts.join(FOOTER_SEPARATOR),
        dim,
        width,
    ));
    if let Some(description) = market
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

    let mut facts = if market.git {
        url_field_lines("Source:", &market.source, value, width)
    } else {
        field_lines("Source:", &market.source, value, width)
    };
    if let Some(revision) = &market.revision {
        let short: String = revision.chars().take(12).collect();
        facts.push(field_line("Revision:", &short, value, width));
    }
    if let Some(at) = market.updated_at.filter(|_| menu.now > 0) {
        facts.push(field_line(
            "Updated:",
            &age_text(menu.now - at),
            value,
            width,
        ));
    }
    let location = app
        .path_display()
        .display(&market.root.display().to_string());
    facts.extend(field_lines("Location:", &location, value, width));

    let mut notes = Vec::new();
    if let Some(error) = &market.error {
        notes.extend(model_wrapped_rows(error, toast_error_color(), width));
    }
    for note in market.notes.iter().take(MARKET_NOTES_MAX) {
        notes.extend(model_wrapped_rows(note, dim, width));
    }
    if market.notes.len() > MARKET_NOTES_MAX {
        notes.push(model_placeholder_row(
            &format!("… {} more skipped", market.notes.len() - MARKET_NOTES_MAX),
            dim,
            width,
        ));
    }

    let labels: Vec<&'static str> = marketplace_actions(market)
        .into_iter()
        .map(crate::app::MarketplaceAction::label)
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

/// The add page: its title and what it takes, the `❯` field — wrapped,
/// the caret in it — what a source may be, the job or the failure, the
/// hint.
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
    let forms = model_wrapped_rows(&format!("Use {SOURCE_FORMS}."), dim, width);
    let [status, hint] = closing_blocks(menu, PLUGINS_ADD_HINT, width);
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

// ===== the whole =====

/// The page as lines and its caret (see [`PluginsBuild`]): the list's first
/// page that fits `height` rows, or a detail page — those bottom-anchor when
/// taller than the terminal, their actions and hint staying on screen.
/// `height` is the terminal's, or the region already reserved for the page,
/// which builds the same page. Empty when the page is closed. A detail page
/// whose plugin or marketplace has just gone shows the list, as the page
/// will once the answer that removed it is taken.
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
            Some(plugin) => plugin_page(menu, plugin, width),
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

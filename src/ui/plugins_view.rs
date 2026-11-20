//! The `/plugins` page. See `docs/plugins.md`.
//!
//! One composer-replacing page in four views — Browse, a plugin, a
//! marketplace, the Add field — dressed as the `/skills` and `/mcp` pages
//! are: a rule above and below, the title in the pages' title dress over a
//! dim line of what is true now, the `❯` search line with the `→` on the
//! highlighted row, the `(n/total)` counter, and the `/mcp` server page's
//! numbered `❯ 1.` actions on a detail page. What is its own:
//!
//! - **the tab strip** — `All`, `Installed`, one tab per marketplace with
//!   its count, `Marketplaces` — the active tab a chip in the ↓-focused
//!   footer segment's dress, a marketplace being fetched wearing the
//!   session's spinner where its count goes, a strip wider than the page
//!   keeping the active tab whole between `‹ ›` cut edges;
//! - **the status column** — one mark per row (`✔ ◯ ↑ ⊘`, blank for not
//!   installed), the same vocabulary as the detail page's status line;
//! - **live work** — a fetch or a job is a row with the spinner, git's own
//!   phase and percentage when it gave them, and the elapsed time on the
//!   page clock; its result is stated on the page it ran on.
//!
//! Every view is **height-aware** and never flows (`docs/view-flow.md` *A
//! page that sheds rows before it flows*): each builds the first of its
//! rungs that fits the rows it is given, so on a short terminal the search
//! line (or the highlighted action) and the hint stay on screen while the
//! description, the facts and the header yield. The rows are built only for
//! the window on screen: a column's width is measured off the tab's rows
//! without formatting them, so a catalog of hundreds costs a screenful.

use std::time::Duration;

use super::model_view::{
    Description, compact_page, first_page_that_fits, list_counter_line, list_window,
    marked_row_head, model_compact_list, model_description_rows, model_placeholder_row, model_rule,
    model_wrapped_rows, placeholder_window, search_line,
};
use super::status::spinner_spans;
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_text};
use super::*;

use crate::app::{
    BrowseRow, FetchState, Job, JobKind, MarketplaceAction, MarketplaceState, Notice, PluginAction,
    PluginsMenu, PluginsPage, PluginsTab, Spinner, format_elapsed,
};
use crate::plugins::catalog::{CatalogMarketplace, CatalogPlugin, PluginDetail, PluginStatus};
use crate::plugins::version_label;

// --- shared pieces ----------------------------------------------------------

/// `n` with its noun, singular for one: `1 plugin`, `382 plugins`.
fn counted(n: usize, (one, many): (&str, &str)) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Join `blocks` into a framed page: a rule, the non-empty blocks one blank
/// row apart, a rule — so no blank row ever hugs a rule
/// (`docs/view-flow.md` *The frame hugs its content*).
fn framed(blocks: Vec<Vec<Line<'static>>>, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width)];
    for block in blocks.into_iter().filter(|b| !b.is_empty()) {
        if lines.len() > 1 {
            lines.push(Line::default());
        }
        lines.extend(block);
    }
    lines.push(model_rule(width));
    lines
}

/// A one-row line of `spans` behind the pages' inset, `…`-cut to `width`.
fn inset_row(spans: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let mut row = vec![Span::raw(MODEL_INDENT)];
    row.extend(spans);
    clamp_spans(row, usize::from(width))
}

/// `text` word-wrapped behind `indent`, at most `max` rows — the last one
/// closing on `…` when the text goes on. Blank text is no rows at all, so a
/// plugin with no description leaves no blank row of its own between the
/// blank rows that part its sections.
fn capped_rows(
    indent: &str,
    text: &str,
    style: Style,
    width: u16,
    max: usize,
) -> Vec<Line<'static>> {
    let room = usize::from(width).saturating_sub(cols(indent)).max(1);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return Vec::new();
    }
    let rows = wrap_text(&flat, u16::try_from(room).unwrap_or(u16::MAX));
    let cut = rows.len() > max;
    rows.into_iter()
        .take(max)
        .enumerate()
        .map(|(i, row)| {
            let row = if cut && i + 1 == max {
                ellipsize(&format!("{row} {TOOL_HEADER_ELLIPSIS}"), room)
            } else {
                row
            };
            Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(row, style),
            ])
        })
        .collect()
}

/// `text` wrapped behind the inset with `lead` (a mark and its space) on
/// the first row and its width of blank under it on the others, so the text
/// hangs off the mark — a result or a failure, which is never cut.
fn hanging_rows(lead: Span<'static>, text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let lead_cols = cols(&lead.content);
    let room = usize::from(width)
        .saturating_sub(cols(MODEL_INDENT) + lead_cols)
        .max(1);
    let mut rows = Vec::new();
    for (i, row) in text
        .split('\n')
        .flat_map(|part| wrap_text(part, u16::try_from(room).unwrap_or(u16::MAX)))
        .enumerate()
    {
        let head = if i == 0 {
            lead.clone()
        } else {
            Span::raw(" ".repeat(lead_cols))
        };
        rows.push(Line::from(vec![
            Span::raw(MODEL_INDENT),
            head,
            Span::styled(row, style),
        ]));
    }
    rows
}

/// The spinner opening a live row or standing in a tab's count: one braille
/// cell — the `dots` style's frames, whatever the session's own style is —
/// at the page clock, ending in its separator space. A track style is eight
/// cells, which shoved the tab strip about and read as noise in a row.
fn spinner(_app: &App, menu: &PluginsMenu) -> Vec<Span<'static>> {
    spinner_spans(Spinner::Dots, menu.clock())
}

/// How long something that started at `since` on the page clock has run.
fn elapsed_text(menu: &PluginsMenu, since: Duration) -> String {
    format_elapsed(menu.clock().saturating_sub(since).as_secs())
}

/// `Fetching openai/plugins… 45% · 2s`, or with git's phase word,
/// `Fetching openai/plugins · receiving objects 45% · 2s`.
fn fetch_words(
    menu: &PluginsMenu,
    marketplace: &str,
    phase: Option<&str>,
    percent: Option<u8>,
    since: Duration,
) -> String {
    let percent = percent.map(|p| format!(" {p}%")).unwrap_or_default();
    let elapsed = elapsed_text(menu, since);
    match phase {
        Some(phase) => format!(
            "{PLUGINS_FETCH_VERB} {marketplace}{FOOTER_SEPARATOR}{}{percent}{FOOTER_SEPARATOR}{elapsed}",
            phase.to_lowercase()
        ),
        None => format!("{PLUGINS_FETCH_VERB} {marketplace}…{percent}{FOOTER_SEPARATOR}{elapsed}"),
    }
}

/// The rows of one fetch: a live row with the spinner while it runs; a
/// failure, red and wrapped whole, over the dim way to try again.
fn fetch_rows(app: &App, menu: &PluginsMenu, marketplace: &str, width: u16) -> Vec<Line<'static>> {
    let Some(fetch) = menu.fetch_for(marketplace) else {
        return Vec::new();
    };
    match &fetch.state {
        FetchState::Running {
            phase,
            percent,
            since,
        } => {
            let mut spans = spinner(app, menu);
            spans.push(Span::styled(
                fetch_words(menu, marketplace, phase.as_deref(), *percent, *since),
                Style::new().fg(model_meta_color()),
            ));
            vec![inset_row(spans, width)]
        }
        FetchState::Failed(reason) => {
            let mut rows = hanging_rows(
                Span::styled(
                    format!("{PLUGINS_FAILED_MARK} "),
                    Style::new().fg(tool_fail_color()),
                ),
                &format!("{PLUGINS_FETCH_FAILED} {marketplace} — {reason}"),
                Style::new().fg(tool_fail_color()),
                width,
            );
            rows.push(model_placeholder_row(
                PLUGINS_FETCH_RETRY,
                model_meta_color(),
                width,
            ));
            rows
        }
    }
}

/// A job's verb.
const fn job_verb(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Install => PLUGINS_JOB_INSTALLING,
        JobKind::Update | JobKind::UpdateMarketplace => PLUGINS_JOB_UPDATING,
        JobKind::Uninstall => PLUGINS_JOB_UNINSTALLING,
        JobKind::AddMarketplace => PLUGINS_JOB_ADDING,
        JobKind::RestoreMarketplace => PLUGINS_JOB_RESTORING,
        JobKind::RemoveMarketplace => PLUGINS_JOB_REMOVING,
    }
}

/// The running job's one live row: `⠹ Installing code-review · cloning
/// github.com/owner/repo · 62% · 3s`.
fn job_row(app: &App, menu: &PluginsMenu, job: &Job, width: u16) -> Line<'static> {
    let mut words = format!("{} {}", job_verb(job.kind), job.target);
    if let Some(step) = &job.step {
        words.push_str(FOOTER_SEPARATOR);
        words.push_str(step);
    }
    if let Some(percent) = job.percent {
        words.push_str(&format!("{FOOTER_SEPARATOR}{percent}%"));
    }
    words.push_str(FOOTER_SEPARATOR);
    words.push_str(&elapsed_text(menu, job.since));
    let mut spans = spinner(app, menu);
    spans.push(Span::styled(words, Style::new().fg(model_id_color())));
    inset_row(spans, width)
}

/// The last result: a green `✔` over text in the default ink, or a red `✗`
/// and red text — wrapped, never cut — and the dim second sentence under
/// it, hanging off the mark.
fn notice_rows(notice: &Notice, width: u16) -> Vec<Line<'static>> {
    let (mark, mark_color, text_style) = if notice.cancelled {
        (
            PLUGINS_FAILED_MARK,
            model_meta_color(),
            Style::new().fg(model_meta_color()),
        )
    } else if notice.ok {
        (PLUGINS_OK_MARK, model_active_color(), Style::default())
    } else {
        (
            PLUGINS_FAILED_MARK,
            tool_fail_color(),
            Style::new().fg(tool_fail_color()),
        )
    };
    let lead = Span::styled(format!("{mark} "), Style::new().fg(mark_color));
    let mut rows = hanging_rows(lead, &notice.text, text_style, width);
    if let Some(detail) = &notice.detail {
        let blank = Span::raw(" ".repeat(cols(mark) + 1));
        rows.extend(hanging_rows(
            blank,
            detail,
            Style::new().fg(model_meta_color()),
            width,
        ));
    }
    rows
}

/// The block between a page's content and its hint: the running job's row,
/// else the last result — or nothing.
fn status_block(app: &App, menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    if let Some(job) = &menu.job {
        return vec![job_row(app, menu, job, width)];
    }
    menu.notice
        .as_ref()
        .map(|notice| notice_rows(notice, width))
        .unwrap_or_default()
}

/// A hint row from its clauses, `…`-cut.
fn hint_row(clauses: &[&str], width: u16) -> Line<'static> {
    model_placeholder_row(&clauses.join(FOOTER_SEPARATOR), model_meta_color(), width)
}

/// A hint row that fits rather than cuts: while its clauses are wider than
/// the row, the ones in `optional` go, first to last — the keys the page
/// already shows (the caret on the search line, the tab strip) before the
/// ones it doesn't — and only then is it `…`-cut.
fn fitted_hint(clauses: &[&str], optional: &[&str], width: u16) -> Line<'static> {
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT));
    let mut kept: Vec<&str> = clauses.to_vec();
    for drop in optional {
        if cols(&kept.join(FOOTER_SEPARATOR)) <= room {
            break;
        }
        kept.retain(|clause| clause != drop);
    }
    hint_row(&kept, width)
}

/// The Plugin and Marketplace pages' hint, whole where it fits and without
/// its first clause where it does not.
fn detail_hint(width: u16) -> Line<'static> {
    fitted_hint(
        &[PLUGINS_HINT_CHOOSE, PLUGINS_HINT_CONFIRM, PLUGINS_HINT_BACK],
        &[PLUGINS_HINT_CHOOSE],
        width,
    )
}

/// While a job runs, every page's hint is the one key that does anything.
fn cancel_hint(width: u16) -> Line<'static> {
    hint_row(&[PLUGINS_HINT_CANCEL], width)
}

/// A detail page's title: `Plugins` in the title dress, the dim `›`, the
/// subject in bold — the subject `…`-cut to the row.
fn crumb_title(subject: &str, width: u16) -> Line<'static> {
    let mut spans = super::header::page_title_spans(PLUGINS_TITLE);
    spans.push(Span::styled(
        PLUGINS_CRUMB,
        Style::new().fg(model_meta_color()),
    ));
    let used = cols(MODEL_INDENT) + cols(PLUGINS_TITLE) + cols(PLUGINS_CRUMB);
    let room = usize::from(width).saturating_sub(used).max(1);
    spans.push(Span::styled(
        ellipsize(subject, room),
        Style::new().add_modifier(Modifier::BOLD),
    ));
    inset_row(spans, width)
}

/// One `{label}{value}` facts row: the label dim in its fixed column, the
/// value after it in `style`, `…`-cut to one row.
fn fact_row(
    label: &str,
    value: Vec<Span<'static>>,
    label_cols: usize,
    width: u16,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{label:<label_cols$}"),
        Style::new().fg(model_meta_color()),
    )];
    spans.extend(value);
    inset_row(spans, width)
}

/// A facts row whose value is read whole — a path, an address, a reason —
/// wrapped under its label's column rather than cut. `target`, when given,
/// makes every fragment a link to it (`docs/links.md`).
fn wrapped_fact(
    label: &str,
    value: &str,
    style: Style,
    target: Option<&str>,
    label_cols: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let lead = cols(MODEL_INDENT) + label_cols;
    let room = usize::from(width).saturating_sub(lead).max(1);
    let style = match target {
        Some(url) => crate::links::linked(style.add_modifier(Modifier::UNDERLINED), url),
        None => style,
    };
    let rows = super::inline::wrap_inline(
        &[(value.to_string(), style)],
        u16::try_from(room).unwrap_or(u16::MAX),
    );
    rows.into_iter()
        .enumerate()
        .map(|(i, spans)| {
            let head = if i == 0 {
                Span::styled(
                    format!("{label:<label_cols$}"),
                    Style::new().fg(model_meta_color()),
                )
            } else {
                Span::raw(" ".repeat(label_cols))
            };
            let mut row = vec![Span::raw(MODEL_INDENT), head];
            row.extend(spans);
            Line::from(row)
        })
        .collect()
}

/// The numbered actions of a detail page — the `/mcp` server page's
/// `❯ 1. Label` rows, the selection in the accent — with the highlighted
/// one's phrase dim after it, its column lined up past the longest label.
/// An armed confirmation replaces its row's label, in amber.
fn action_rows(
    actions: &[(String, Option<&'static str>)],
    selected: usize,
    armed: Option<&'static str>,
    width: u16,
) -> Vec<Line<'static>> {
    let label_cols = actions.iter().map(|(l, _)| cols(l)).max().unwrap_or(0);
    actions
        .iter()
        .enumerate()
        .map(|(i, (label, phrase))| {
            let chosen = i == selected;
            let marker = if chosen {
                Span::styled(HOOKS_MARKER, Style::new().fg(model_selected_color()))
            } else {
                Span::raw(" ".repeat(cols(HOOKS_MARKER)))
            };
            let style = Style::new().fg(if chosen {
                model_selected_color()
            } else {
                model_id_color()
            });
            let mut spans = vec![marker, Span::styled(format!("{}. ", i + 1), style)];
            match armed.filter(|_| chosen) {
                Some(confirm) => {
                    spans.push(Span::styled(confirm, Style::new().fg(ask_warning_color())));
                }
                None => {
                    spans.push(Span::styled(label.clone(), style));
                    if chosen && let Some(phrase) = phrase {
                        let pad = label_cols - cols(label) + PLUGINS_COLUMN_GAP;
                        spans.push(Span::raw(" ".repeat(pad)));
                        spans.push(Span::styled(*phrase, Style::new().fg(model_meta_color())));
                    }
                }
            }
            inset_row(spans, width)
        })
        .collect()
}

/// The small rungs every detail page sheds down to on a short terminal:
/// the title over the highlighted action alone between the `↑`/`↓` slots
/// (or the job's row) and the hint; then without the slots, the top rule,
/// the bottom rule, the hint and the title, one at a time — never the
/// action, which is what Enter would do.
fn small_rungs(
    height: u16,
    title: &Line<'static>,
    focus: (usize, Vec<Line<'static>>, usize),
    hint: &Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let (above, rows, below) = focus;
    let rule = model_rule(width);
    let mut slotted = vec![rule.clone(), title.clone()];
    slotted.extend(model_compact_list(above, rows.clone(), below, width));
    slotted.extend([hint.clone(), rule.clone()]);
    let with_rules = |top: bool, bottom: bool, hint_too: bool, title_too: bool| {
        let mut lines = Vec::new();
        if top {
            lines.push(rule.clone());
        }
        if title_too {
            lines.push(title.clone());
        }
        lines.extend(rows.iter().cloned());
        if hint_too {
            lines.push(hint.clone());
        }
        if bottom {
            lines.push(rule.clone());
        }
        lines
    };
    first_page_that_fits(
        height,
        &[
            &|| slotted.clone(),
            &|| with_rules(true, true, true, true),
            &|| with_rules(false, true, true, true),
            &|| with_rules(false, false, true, true),
            &|| with_rules(false, false, false, true),
            &|| with_rules(false, false, false, false),
        ],
    )
}

/// The highlighted action alone, as a window over `actions`.
fn action_focus(
    actions: &[(String, Option<&'static str>)],
    selected: usize,
    armed: Option<&'static str>,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let rows = action_rows(actions, selected, armed, width);
    let at = selected.min(rows.len().saturating_sub(1));
    let below = rows.len().saturating_sub(at + 1);
    (at, rows.into_iter().skip(at).take(1).collect(), below)
}

// --- Browse ------------------------------------------------------------------

/// A plugin's status mark and its colour; blank for one not installed. An
/// update waiting never takes the mark's place — it is its own `↑` beside
/// the name ([`update_waits`]), so the mark always says whether it is on.
fn status_mark(plugin_status: PluginStatus) -> (&'static str, Color) {
    match plugin_status {
        PluginStatus::Enabled => (PLUGINS_ENABLED_MARK, model_active_color()),
        PluginStatus::Disabled => (PLUGINS_DISABLED_MARK, model_meta_color()),
        PluginStatus::Unsupported => (PLUGINS_BLOCKED_MARK, model_meta_color()),
        PluginStatus::Available => (" ", model_meta_color()),
    }
}

/// The name a plugin row shows: its display name when it has one.
fn plugin_name(plugin: &CatalogPlugin) -> &str {
    plugin.display_name.as_deref().unwrap_or(&plugin.name)
}

/// Whether an installed plugin has an update waiting — its row wears the
/// amber `↑` after its name.
fn update_waits(plugin: &CatalogPlugin) -> bool {
    plugin.update_available && plugin.status.is_installed()
}

/// The columns the `↑` after an updatable plugin's name takes.
fn update_suffix_cols() -> usize {
    1 + cols(PLUGINS_UPDATE_MARK)
}

/// The version a plugin is at, as `v1.0.0` — or, while an update waits and
/// both are known, the installed one and the offered one: `v1.0.0 →
/// v2.0.0`.
fn version_text(installed: Option<&str>, offered: Option<&str>, update: bool) -> Option<String> {
    match (installed, offered) {
        (Some(from), Some(to)) if update && from != to => Some(format!(
            "{}{PLUGINS_VERSION_ARROW}{}",
            version_label(from),
            version_label(to)
        )),
        (installed, offered) => offered.or(installed).map(version_label),
    }
}

/// The columns a plugin row is laid out on, measured off every row the tab
/// lists — never off formatted rows.
struct PluginColumns {
    /// The name column's width.
    name: usize,
    /// The marketplace column's width; `0` when the row has none.
    marketplace: usize,
    /// Whether the row shows a description.
    description: bool,
}

/// The columns for `rows` at `width`: the name sized off every listed row
/// and capped ([`PLUGINS_NAME_MAX_COLS`]); the marketplace column on the
/// `All` and `Installed` tabs from [`PLUGINS_MARKETPLACE_MIN_WIDTH`]; the
/// description from [`PLUGINS_DESCRIPTION_MIN_WIDTH`].
fn plugin_columns(menu: &PluginsMenu, rows: &[BrowseRow], width: u16) -> PluginColumns {
    let Some(catalog) = menu.catalog.as_ref() else {
        return PluginColumns {
            name: 1,
            marketplace: 0,
            description: false,
        };
    };
    let plugins = || {
        rows.iter().filter_map(|row| match row {
            BrowseRow::Plugin(i) => catalog.plugins.get(*i),
            _ => None,
        })
    };
    let head = plugin_head_cols();
    let name = plugins()
        .map(|p| {
            cols(plugin_name(p))
                + if update_waits(p) {
                    update_suffix_cols()
                } else {
                    0
                }
        })
        .max()
        .unwrap_or(1)
        .clamp(1, PLUGINS_NAME_MAX_COLS)
        .min(usize::from(width).saturating_sub(head).max(1));
    let marketplace = if width >= PLUGINS_MARKETPLACE_MIN_WIDTH
        && matches!(menu.tab, PluginsTab::All | PluginsTab::Installed)
    {
        plugins()
            .filter_map(|p| catalog.marketplaces.get(p.marketplace))
            .map(|m| cols(&m.name))
            .max()
            .unwrap_or(0)
            .min(PLUGINS_MARKETPLACE_MAX_COLS)
    } else {
        0
    };
    PluginColumns {
        name,
        marketplace,
        description: width >= PLUGINS_DESCRIPTION_MIN_WIDTH,
    }
}

/// The columns a plugin row spends before its name: the inset, the marker,
/// the mark and its space.
fn plugin_head_cols() -> usize {
    cols(MODEL_INDENT) + cols(MODEL_MARKER) + 2
}

/// One plugin row: `→ ✔ code-review   Automated code review…   claude-plugins-official`.
fn plugin_row(
    plugin: &CatalogPlugin,
    marketplace: &str,
    selected: bool,
    columns: &PluginColumns,
    width: u16,
) -> Line<'static> {
    let suffix = if update_waits(plugin) {
        update_suffix_cols()
    } else {
        0
    };
    let name = ellipsize(
        plugin_name(plugin),
        columns.name.saturating_sub(suffix).max(1),
    );
    let pad = columns.name.saturating_sub(cols(&name) + suffix);
    let (mark, mark_color) = status_mark(plugin.status);
    let mut spans = marked_row_head(&name, selected);
    spans.insert(2, Span::styled(mark, Style::new().fg(mark_color)));
    spans.insert(3, Span::raw(" "));
    if suffix > 0 {
        spans.push(Span::styled(
            format!(" {PLUGINS_UPDATE_MARK}"),
            Style::new().fg(ask_warning_color()),
        ));
    }
    let used = plugin_head_cols() + columns.name;
    let tail = if columns.marketplace > 0 {
        PLUGINS_COLUMN_GAP + columns.marketplace
    } else {
        0
    };
    let room = usize::from(width).saturating_sub(used + PLUGINS_COLUMN_GAP + tail);
    if columns.description && room > 0 {
        let flat = plugin
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let description = ellipsize(&flat, room);
        let fill = room - cols(&description);
        spans.push(Span::raw(" ".repeat(pad + PLUGINS_COLUMN_GAP)));
        spans.push(Span::styled(
            description,
            Style::new().fg(model_meta_color()),
        ));
        if columns.marketplace > 0 {
            let shown = ellipsize(marketplace, columns.marketplace);
            spans.push(Span::raw(" ".repeat(fill + PLUGINS_COLUMN_GAP)));
            spans.push(Span::styled(shown, Style::new().fg(model_meta_color())));
        }
    }
    clamp_spans(spans, usize::from(width))
}

/// The columns of the Marketplaces tab: its name, its source, its facts.
struct MarketColumns {
    name: usize,
    source: usize,
    facts: usize,
}

/// The facts a marketplace row ends on: `317 plugins · updated 2h ago`,
/// `not fetched yet`, `local folder`, `fetching…`, or its failure.
fn market_facts(menu: &PluginsMenu, index: usize, market: &CatalogMarketplace) -> (String, bool) {
    if let Some(fetch) = menu.fetch_for(&market.name) {
        match &fetch.state {
            FetchState::Running { .. } => return (PLUGINS_FETCHING.to_string(), false),
            FetchState::Failed(reason) => {
                return (format!("{PLUGINS_FAILED_MARK} {reason}"), true);
            }
        }
    }
    if market.is_removed() {
        return (PLUGINS_REMOVED_FACT.to_string(), false);
    }
    if let Some(error) = &market.error {
        return (format!("{PLUGINS_FAILED_MARK} {error}"), true);
    }
    if !market.fetched {
        return (PLUGINS_NOT_FETCHED_FACT.to_string(), false);
    }
    let count = menu
        .catalog
        .as_ref()
        .map_or(0, |catalog| catalog.count_in(index));
    let mut facts = counted(count, PLUGINS_PLUGIN_NOUN);
    if !market.skipped.is_empty() {
        facts.push_str(&format!(
            "{FOOTER_SEPARATOR}{} {PLUGINS_SKIPPED_WORD}",
            market.skipped.len()
        ));
    }
    if market.is_local {
        facts.push_str(FOOTER_SEPARATOR);
        facts.push_str(PLUGINS_LOCAL_FACT);
    } else if let Some(updated) = market.updated {
        facts.push_str(&format!(
            "{FOOTER_SEPARATOR}{PLUGINS_UPDATED} {}",
            super::model_view::age_text(menu.age_of(updated))
        ));
    }
    (facts, false)
}

/// What a Marketplaces-tab row shows in its first two columns.
fn market_row_text(menu: &PluginsMenu, row: &BrowseRow) -> (String, String) {
    match row {
        BrowseRow::Marketplace(i) => menu
            .catalog
            .as_ref()
            .and_then(|c| c.marketplaces.get(*i))
            .map(|m| (m.name.clone(), m.source.clone()))
            .unwrap_or_default(),
        BrowseRow::AddMarketplace => (
            PLUGINS_ADD_ROW.to_string(),
            PLUGINS_ADD_ROW_NOTE.to_string(),
        ),
        BrowseRow::Restore(source) => (
            format!("{PLUGINS_RESTORE_ROW} {source}"),
            PLUGINS_RESTORE_ROW_NOTE.to_string(),
        ),
        BrowseRow::Plugin(_) => (String::new(), String::new()),
    }
}

/// The Marketplaces tab's columns, sized off every row it lists.
fn market_columns(menu: &PluginsMenu, rows: &[BrowseRow], width: u16) -> MarketColumns {
    let mut columns = MarketColumns {
        name: 1,
        source: 0,
        facts: 0,
    };
    for row in rows {
        let (name, source) = market_row_text(menu, row);
        columns.name = columns.name.max(cols(&name));
        if let BrowseRow::Marketplace(i) = row
            && let Some(market) = menu.catalog.as_ref().and_then(|c| c.marketplaces.get(*i))
        {
            columns.source = columns.source.max(cols(&source));
            columns.facts = columns.facts.max(cols(&market_facts(menu, *i, market).0));
        }
    }
    columns.name = columns
        .name
        .min(PLUGINS_MARKETPLACE_MAX_COLS)
        .min(usize::from(width).saturating_sub(row_head()).max(1));
    columns.source = columns.source.min(PLUGINS_SOURCE_COLS_MAX);
    columns.facts = columns.facts.min(PLUGINS_FACTS_COLS_MAX);
    columns
}

/// The columns a marketplace row spends before its name.
fn row_head() -> usize {
    cols(MODEL_INDENT) + cols(MODEL_MARKER)
}

/// One Marketplaces-tab row: the name, the source dim, the facts dim and
/// flush right (red for a failure) — on a narrow terminal the source is cut
/// with `…` first and then gives way, since the facts are what say a
/// marketplace failed or was removed; the facts are cut last. The `+` rows
/// put their note where the source goes and let it run on.
fn market_row(
    menu: &PluginsMenu,
    row: &BrowseRow,
    selected: bool,
    columns: &MarketColumns,
    width: u16,
) -> Line<'static> {
    let (name, source) = market_row_text(menu, row);
    let shown = ellipsize(&name, columns.name);
    let pad = columns.name - cols(&shown).min(columns.name);
    let mut spans = marked_row_head(&shown, selected);
    let rest = usize::from(width).saturating_sub(row_head() + columns.name + PLUGINS_COLUMN_GAP);
    let dim = Style::new().fg(model_meta_color());
    let BrowseRow::Marketplace(index) = row else {
        if rest > 0 {
            spans.push(Span::raw(" ".repeat(pad + PLUGINS_COLUMN_GAP)));
            spans.push(Span::styled(ellipsize(&source, rest), dim));
        }
        return clamp_spans(spans, usize::from(width));
    };
    let Some(market) = menu
        .catalog
        .as_ref()
        .and_then(|c| c.marketplaces.get(*index))
    else {
        return Line::from(spans);
    };
    let (facts, failed) = market_facts(menu, *index, market);
    let facts_style = if failed {
        Style::new().fg(tool_fail_color())
    } else {
        dim
    };
    // The source keeps its whole column while the facts fit beside it, then
    // is cut down to a few columns, then gives way: the facts outrank it.
    let (source_cols, facts_cols) = if rest >= columns.source + PLUGINS_COLUMN_GAP + columns.facts {
        (columns.source, columns.facts)
    } else if rest >= PLUGINS_SOURCE_COLS_MIN + PLUGINS_COLUMN_GAP + columns.facts {
        (rest - columns.facts - PLUGINS_COLUMN_GAP, columns.facts)
    } else {
        (0, rest)
    };
    if rest == 0 {
        return clamp_spans(spans, usize::from(width));
    }
    let source = if source_cols > 0 {
        ellipsize(&source, source_cols)
    } else {
        String::new()
    };
    let source_pad = source_cols.saturating_sub(cols(&source));
    spans.push(Span::raw(" ".repeat(pad + PLUGINS_COLUMN_GAP)));
    if source_cols > 0 {
        spans.push(Span::styled(source, dim));
    } else {
        // No source column: the facts start where it would have.
        spans.pop();
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if facts_cols > 0 {
        let facts = ellipsize(&facts, facts_cols);
        let right = facts_cols - cols(&facts);
        spans.push(Span::raw(
            " ".repeat(source_pad + PLUGINS_COLUMN_GAP + right),
        ));
        spans.push(Span::styled(facts, facts_style));
    }
    clamp_spans(spans, usize::from(width))
}

/// The window of the current tab's rows that keeps the highlight in view —
/// `max` rows riding it — as how many rows it hides above, its rows built
/// (only these), and how many below.
fn browse_window(
    menu: &PluginsMenu,
    rows: &[BrowseRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = list_window(rows.len(), selected, max);
    let lines = if menu.tab == PluginsTab::Marketplaces {
        let columns = market_columns(menu, rows, width);
        range
            .map(|i| market_row(menu, &rows[i], i == selected, &columns, width))
            .collect()
    } else {
        let columns = plugin_columns(menu, rows, width);
        let catalog = menu.catalog.as_ref();
        range
            .filter_map(|i| {
                let BrowseRow::Plugin(index) = rows[i] else {
                    return None;
                };
                let catalog = catalog?;
                let plugin = catalog.plugins.get(index)?;
                let marketplace = catalog
                    .marketplaces
                    .get(plugin.marketplace)
                    .map_or("", |m| m.name.as_str());
                Some(plugin_row(
                    plugin,
                    marketplace,
                    i == selected,
                    &columns,
                    width,
                ))
            })
            .collect()
    };
    (above, lines, below)
}

/// The subtitle: `2 marketplaces · 382 plugins · 3 installed`, then how
/// fresh it is — `updated 2h ago` (the oldest fetched marketplace),
/// `fetching…`, `refreshing…`, or nothing yet — and no line at all before
/// the catalog lands, when the list's place already says it is loading.
fn subtitle(menu: &PluginsMenu, width: u16) -> Option<Line<'static>> {
    let catalog = menu.catalog.as_ref()?;
    // The marketplaces the user has: not one removed (listed only for the
    // plugins it left installed) nor the folder installs' own listing.
    let marketplaces = catalog
        .marketplaces
        .iter()
        .filter(|m| !m.is_removed() && m.name != crate::plugins::LOCAL_MARKETPLACE)
        .count();
    let mut text = [
        counted(marketplaces, PLUGINS_MARKETPLACE_NOUN),
        counted(catalog.plugins.len(), PLUGINS_PLUGIN_NOUN),
        format!("{} {PLUGINS_INSTALLED_WORD}", catalog.installed_count()),
    ]
    .join(FOOTER_SEPARATOR);
    let running = |m: &CatalogMarketplace| {
        menu.fetch_for(&m.name)
            .is_some_and(|f| matches!(f.state, FetchState::Running { .. }))
    };
    let unlisted_running = menu.fetching()
        && menu.fetches.iter().any(|f| {
            matches!(f.state, FetchState::Running { .. })
                && !catalog
                    .marketplaces
                    .iter()
                    .any(|m| m.name == f.marketplace && m.fetched)
        });
    let freshness = if unlisted_running
        || catalog
            .marketplaces
            .iter()
            .any(|m| !m.fetched && running(m))
    {
        Some(PLUGINS_FETCHING.to_string())
    } else if menu.fetching() {
        Some(PLUGINS_REFRESHING.to_string())
    } else {
        catalog
            .marketplaces
            .iter()
            .filter(|m| m.fetched && !m.is_local)
            .filter_map(|m| m.updated)
            .min()
            .map(|oldest| {
                format!(
                    "{PLUGINS_UPDATED} {}",
                    super::model_view::age_text(menu.age_of(oldest))
                )
            })
    };
    if let Some(freshness) = freshness {
        text.push_str(FOOTER_SEPARATOR);
        text.push_str(&freshness);
    }
    Some(model_placeholder_row(&text, model_meta_color(), width))
}

/// One tab's label spans, without its padding: the name, and after it the
/// count — or, for a marketplace being fetched, the spinner in its place,
/// and for one that failed, a red `✗`.
fn tab_label(app: &App, menu: &PluginsMenu, tab: &PluginsTab) -> Vec<Span<'static>> {
    let dim = Style::new().fg(model_meta_color());
    match tab {
        PluginsTab::All => vec![Span::styled(PLUGINS_TAB_ALL, dim)],
        PluginsTab::Installed => {
            let count = menu.catalog.as_ref().map(|c| c.installed_count());
            vec![Span::styled(
                match count {
                    Some(n) => format!("{PLUGINS_TAB_INSTALLED} {n}"),
                    None => PLUGINS_TAB_INSTALLED.to_string(),
                },
                dim,
            )]
        }
        PluginsTab::Marketplaces => vec![Span::styled(PLUGINS_TAB_MARKETPLACES, dim)],
        PluginsTab::Marketplace(name) => {
            let mut spans = vec![Span::styled(format!("{name} "), dim)];
            match menu.fetch_for(name).map(|f| &f.state) {
                Some(FetchState::Running { .. }) => {
                    let mut frame = spinner(app, menu);
                    if let Some(last) = frame.last_mut() {
                        last.content = last.content.trim_end().to_string().into();
                    }
                    spans.extend(frame);
                }
                Some(FetchState::Failed(_)) => spans.push(Span::styled(
                    PLUGINS_FAILED_MARK,
                    Style::new().fg(tool_fail_color()),
                )),
                // A marketplace never fetched lists nothing yet: its tab is
                // its name alone, never a `0` that reads as an empty one.
                None => {
                    let unreadable = menu.marketplace_index(name).is_some_and(|i| {
                        menu.catalog.as_ref().is_some_and(|catalog| {
                            let market = &catalog.marketplaces[i];
                            market.error.is_some() && !market.is_removed()
                        })
                    });
                    if unreadable {
                        spans.push(Span::styled(
                            PLUGINS_FAILED_MARK,
                            Style::new().fg(tool_fail_color()),
                        ));
                        return spans;
                    }
                    let found = menu.marketplace_index(name).and_then(|i| {
                        let catalog = menu.catalog.as_ref()?;
                        let market = &catalog.marketplaces[i];
                        (market.fetched || market.is_removed()).then(|| catalog.count_in(i))
                    });
                    match found {
                        Some(count) => spans.push(Span::styled(count.to_string(), dim)),
                        None => {
                            if let Some(last) = spans.last_mut() {
                                last.content = last.content.trim_end().to_string().into();
                            }
                        }
                    }
                }
            }
            spans
        }
    }
}

/// One tab padded a space each side — the active one a chip in the
/// ↓-focused footer segment's dress, its whole width on the chip ground.
fn tab_cell(app: &App, menu: &PluginsMenu, tab: &PluginsTab, active: bool) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(PLUGINS_TAB_PAD)];
    spans.extend(tab_label(app, menu, tab));
    spans.push(Span::raw(PLUGINS_TAB_PAD));
    if active {
        let chip = Style::new().bg(footer_focus_bg()).fg(footer_focus_fg());
        spans = spans
            .into_iter()
            .map(|span| {
                let style = if span.style.fg == Some(model_meta_color()) || span.style.fg.is_none()
                {
                    chip
                } else {
                    span.style.bg(footer_focus_bg())
                };
                Span::styled(span.content, style)
            })
            .collect();
    }
    spans
}

/// The tab strip: every tab when they fit; else the active one whole with
/// as many neighbours as fit, a dim `‹` / `›` at a cut edge; and under
/// [`PLUGINS_TABS_MIN_WIDTH`] the active tab alone between the arrows, with
/// its place among the tabs after it.
fn tab_strip(app: &App, menu: &PluginsMenu, width: u16) -> Line<'static> {
    let tabs = menu.tabs();
    let active = menu.tab_index();
    let dim = Style::new().fg(model_meta_color());
    let cells: Vec<Vec<Span<'static>>> = tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| tab_cell(app, menu, tab, i == active))
        .collect();
    let widths: Vec<usize> = cells
        .iter()
        .map(|cell| cell.iter().map(|s| cols(&s.content)).sum())
        .collect();
    let mut spans = Vec::new();
    if width < PLUGINS_TABS_MIN_WIDTH {
        spans.push(Span::styled(PLUGINS_TAB_LEFT, dim));
        spans.extend(cells[active].clone());
        spans.push(Span::styled(PLUGINS_TAB_RIGHT, dim));
        spans.push(Span::styled(
            format!(" ({}/{})", active + 1, tabs.len()),
            dim,
        ));
        return inset_row(spans, width);
    }
    let room = usize::from(width).saturating_sub(cols(MODEL_INDENT));
    let gap = cols(PLUGINS_TAB_GAP);
    let span_of = |lo: usize, hi: usize| -> usize {
        widths[lo..=hi].iter().sum::<usize>()
            + gap * (hi - lo)
            + if lo > 0 { cols(PLUGINS_TAB_LEFT) } else { 0 }
            + if hi + 1 < tabs.len() {
                cols(PLUGINS_TAB_RIGHT)
            } else {
                0
            }
    };
    let (mut lo, mut hi) = (active, active);
    loop {
        let mut grew = false;
        if hi + 1 < tabs.len() && span_of(lo, hi + 1) <= room {
            hi += 1;
            grew = true;
        }
        if lo > 0 && span_of(lo - 1, hi) <= room {
            lo -= 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    if lo > 0 {
        spans.push(Span::styled(PLUGINS_TAB_LEFT, dim));
    }
    for (i, cell) in cells.iter().enumerate().take(hi + 1).skip(lo) {
        if i > lo {
            spans.push(Span::raw(PLUGINS_TAB_GAP));
        }
        spans.extend(cell.iter().cloned());
    }
    if hi + 1 < tabs.len() {
        spans.push(Span::styled(PLUGINS_TAB_RIGHT, dim));
    }
    inset_row(spans, width)
}

/// Whether the catalog would gain from Ctrl+R: a marketplace never fetched,
/// one whose fetch or read failed, or one older than a day — and nothing
/// being fetched already.
fn stale(menu: &PluginsMenu) -> bool {
    if menu.fetching() {
        return false;
    }
    let failed = menu
        .fetches
        .iter()
        .any(|f| matches!(f.state, FetchState::Failed(_)));
    failed
        || menu.catalog.as_ref().is_some_and(|catalog| {
            catalog.marketplaces.iter().any(|m| {
                !m.is_removed() && (!m.fetched || m.error.is_some())
                    || (!m.is_local
                        && m.updated
                            .is_some_and(|at| menu.age_of(at) > PLUGINS_STALE_SECS))
            })
        })
}

/// The Browse hint: what Enter and Space do to the highlighted row, the
/// tabs, the search, what Esc does, and Ctrl+R when it would help.
fn browse_hint(menu: &PluginsMenu, highlighted: Option<&BrowseRow>, width: u16) -> Line<'static> {
    if menu.job.is_some() {
        return cancel_hint(width);
    }
    let catalog = menu.catalog.as_ref();
    let mut clauses: Vec<&str> = match highlighted {
        Some(BrowseRow::Plugin(i)) => {
            match catalog.and_then(|c| c.plugins.get(*i)).map(|p| p.status) {
                Some(PluginStatus::Enabled) => vec![PLUGINS_HINT_DETAILS, PLUGINS_HINT_DISABLE],
                Some(PluginStatus::Disabled) => vec![PLUGINS_HINT_DETAILS, PLUGINS_HINT_ENABLE],
                Some(PluginStatus::Available) => vec![PLUGINS_HINT_INSTALL],
                _ => vec![PLUGINS_HINT_DETAILS],
            }
        }
        Some(BrowseRow::Marketplace(_)) => vec![PLUGINS_HINT_MANAGE],
        Some(BrowseRow::AddMarketplace) => vec![PLUGINS_HINT_ADD],
        Some(BrowseRow::Restore(_)) => vec![PLUGINS_HINT_RESTORE],
        None => Vec::new(),
    };
    clauses.push(PLUGINS_HINT_TABS);
    if menu.tab != PluginsTab::Marketplaces {
        clauses.push(PLUGINS_HINT_SEARCH);
    }
    clauses.push(if menu.query.is_empty() {
        PLUGINS_HINT_CLOSE
    } else {
        PLUGINS_HINT_CLEAR
    });
    if stale(menu) {
        clauses.push(PLUGINS_HINT_REFRESH);
    }
    fitted_hint(&clauses, &[PLUGINS_HINT_SEARCH, PLUGINS_HINT_TABS], width)
}

/// What the list's place says when the tab lists nothing: the catalog still
/// loading or unreadable, the fetches under way or failed that would fill
/// it, a search that matched nothing, an empty `Installed`, no marketplace
/// at all, or one not fetched.
fn empty_block(app: &App, menu: &PluginsMenu, width: u16) -> Vec<Line<'static>> {
    let Some(catalog) = menu.catalog.as_ref() else {
        if let Some(reason) = menu.load_error() {
            return hanging_rows(
                Span::styled(
                    format!("{PLUGINS_FAILED_MARK} "),
                    Style::new().fg(tool_fail_color()),
                ),
                &format!("{PLUGINS_LOAD_FAILED} — {reason}"),
                Style::new().fg(tool_fail_color()),
                width,
            );
        }
        return vec![model_placeholder_row(
            PLUGINS_LOADING,
            model_meta_color(),
            width,
        )];
    };
    let fetches: Vec<String> = match &menu.tab {
        PluginsTab::All => menu.fetches.iter().map(|f| f.marketplace.clone()).collect(),
        PluginsTab::Marketplace(name) => menu
            .fetch_for(name)
            .map(|f| vec![f.marketplace.clone()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if !fetches.is_empty() {
        return fetches
            .iter()
            .flat_map(|m| fetch_rows(app, menu, m, width))
            .collect();
    }
    let text = if !menu.query.is_empty() {
        format!("{PLUGINS_NO_MATCH} \"{}\"", menu.query)
    } else if menu.tab == PluginsTab::Installed {
        PLUGINS_NOTHING_INSTALLED.to_string()
    } else if catalog.marketplaces.is_empty() {
        PLUGINS_NO_MARKETPLACES.to_string()
    } else if let PluginsTab::Marketplace(name) = &menu.tab
        && let Some(market) = menu
            .marketplace_index(name)
            .map(|i| &catalog.marketplaces[i])
    {
        if market.is_removed() {
            return model_wrapped_rows(PLUGINS_REMOVED_ABOUT, model_meta_color(), width);
        }
        if let Some(error) = &market.error {
            return model_wrapped_rows(
                &format!("{PLUGINS_UNREADABLE} {name} — {error}"),
                tool_fail_color(),
                width,
            );
        }
        PLUGINS_NOT_FETCHED.to_string()
    } else {
        PLUGINS_NO_MARKETPLACES.to_string()
    };
    model_wrapped_rows(&text, model_meta_color(), width)
}

/// How much the Browse page says of the highlighted row under its list.
#[derive(Clone, Copy)]
enum About {
    /// Its description wrapped to at most this many rows, and the meta row.
    Wrapped(usize),
    /// Its description cut to one row.
    OneRow,
    /// Nothing: the rows the description took go to the list.
    Omitted,
}

/// What the description block under the list says of the highlighted row:
/// a plugin's description and meta row, or what a marketplace row is. The
/// meta row leads with the plugin's whole name when its row had to cut it
/// (`name_cols`, the name column's width).
fn about_highlighted(
    menu: &PluginsMenu,
    row: &BrowseRow,
    about: About,
    name_cols: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let dim = Style::new().fg(model_meta_color());
    let describe = |text: &str| match about {
        About::Wrapped(max) => capped_rows(MODEL_DETAIL_INDENT, text, dim, width, max),
        About::OneRow => model_description_rows(text, Description::OneRow, width),
        About::Omitted => Vec::new(),
    };
    let catalog = menu.catalog.as_ref();
    match row {
        BrowseRow::Plugin(i) => {
            let Some(plugin) = catalog.and_then(|c| c.plugins.get(*i)) else {
                return Vec::new();
            };
            let mut lines = if plugin.description.trim().is_empty() {
                Vec::new()
            } else {
                describe(&plugin.description)
            };
            if matches!(about, About::Wrapped(_)) {
                let name = plugin_name(plugin);
                let meta: Vec<String> = [
                    (cols(name)
                        + if update_waits(plugin) {
                            update_suffix_cols()
                        } else {
                            0
                        }
                        > name_cols)
                        .then(|| name.to_string()),
                    plugin.category.clone(),
                    plugin.author.clone(),
                    version_text(
                        plugin.installed_version.as_deref(),
                        plugin.version.as_deref(),
                        update_waits(plugin),
                    ),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !meta.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(MODEL_DETAIL_INDENT),
                        Span::styled(
                            ellipsize(
                                &meta.join(FOOTER_SEPARATOR),
                                usize::from(width).saturating_sub(cols(MODEL_DETAIL_INDENT)),
                            ),
                            dim,
                        ),
                    ]));
                }
            }
            lines
        }
        BrowseRow::Marketplace(i) => {
            let Some(market) = catalog.and_then(|c| c.marketplaces.get(*i)) else {
                return Vec::new();
            };
            let about = if market.is_removed() {
                PLUGINS_REMOVED_ABOUT
            } else if market.name == crate::plugins::LOCAL_MARKETPLACE {
                PLUGINS_FOLDER_INSTALLS_ABOUT
            } else if market.is_default {
                PLUGINS_DEFAULT_ABOUT
            } else if market.is_local {
                PLUGINS_LOCAL_ABOUT
            } else {
                PLUGINS_ADDED_ABOUT
            };
            let mut text = about.to_string();
            if market.title != market.name {
                text = format!("{} — {text}", market.title);
            }
            describe(&text)
        }
        BrowseRow::AddMarketplace => describe(PLUGINS_ADD_ABOUT),
        BrowseRow::Restore(_) => describe(PLUGINS_RESTORE_ABOUT),
    }
}

/// The Browse page — the first of its rungs that fits `height`:
///
/// 1. the whole page: the title over its subtitle; the tab strip over the
///    search line; eight rows over the counter; the highlighted row's
///    description (three rows at most) over its meta row; the last result
///    or the running job; the hint;
/// 2. five rows instead of eight;
/// 3. the description cut to one row and the meta row gone;
/// 4. eight rows over the counter and nothing said of the highlighted row,
///    then five — so a terminal of sixteen rows still lists more than the
///    compact page's three;
/// 5. the compact page with its header — the title and the tab strip over
///    the search line, three rows between the `↑`/`↓` slots, the hint;
/// 6. the compact page without the header, and below it the rungs every
///    search-line picker sheds to (`model_view::compact_page`), never the
///    search line.
///
/// A tab with nothing to list has its placeholder in the list's place and
/// the same shedding.
fn browse_lines(app: &App, menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let title = super::header::page_title_line(PLUGINS_TITLE, width);
    let header: Vec<Line<'static>> = std::iter::once(title.clone())
        .chain(subtitle(menu, width))
        .collect();
    let tabs = tab_strip(app, menu, width);
    let search = search_line(&plugins_field_text(&menu.query, width));
    let rows = menu.rows();
    let selected = menu.selected_in(rows.len());
    let highlighted = rows.get(selected);
    let hint = browse_hint(menu, highlighted, width);
    let status = status_block(app, menu, width);
    let compact_tail: Vec<Line<'static>> = menu
        .job
        .as_ref()
        .map(|job| job_row(app, menu, job, width))
        .into_iter()
        .chain(std::iter::once(hint.clone()))
        .collect();
    let compact_with_header = |window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize)| {
        let (above, list, below) = window(usize::from(COMPACT_LIST_ROWS));
        let mut lines = vec![
            model_rule(width),
            title.clone(),
            tabs.clone(),
            search.clone(),
        ];
        lines.extend(model_compact_list(above, list, below, width));
        lines.extend(compact_tail.iter().cloned());
        lines.push(model_rule(width));
        lines
    };
    if rows.is_empty() {
        let block = empty_block(app, menu, width);
        let placeholder = |max: usize| placeholder_window(&block, max);
        return first_page_that_fits(
            height,
            &[
                &|| {
                    framed(
                        vec![
                            header.clone(),
                            vec![tabs.clone(), search.clone()],
                            block.clone(),
                            status.clone(),
                            vec![hint.clone()],
                        ],
                        width,
                    )
                },
                &|| compact_with_header(&placeholder),
                &|| compact_page(height, &search, &placeholder, &compact_tail, width),
            ],
        );
    }
    let name_cols = plugin_columns(menu, &rows, width).name;
    let whole = |max: u16, about: About| {
        let (_, list, _) = browse_window(menu, &rows, selected, usize::from(max), width);
        let mut listed = list;
        listed.push(list_counter_line(selected, rows.len()));
        let about = highlighted
            .map(|row| about_highlighted(menu, row, about, name_cols, width))
            .unwrap_or_default();
        framed(
            vec![
                header.clone(),
                vec![tabs.clone(), search.clone()],
                listed,
                about,
                status.clone(),
                vec![hint.clone()],
            ],
            width,
        )
    };
    let window = |max: usize| browse_window(menu, &rows, selected, max, width);
    first_page_that_fits(
        height,
        &[
            &|| whole(PLUGINS_MENU_ROWS, About::Wrapped(PLUGINS_DESCRIPTION_ROWS)),
            &|| {
                whole(
                    PLUGINS_SHORT_MENU_ROWS,
                    About::Wrapped(PLUGINS_DESCRIPTION_ROWS),
                )
            },
            &|| whole(PLUGINS_SHORT_MENU_ROWS, About::OneRow),
            &|| whole(PLUGINS_MENU_ROWS, About::Omitted),
            &|| whole(PLUGINS_SHORT_MENU_ROWS, About::Omitted),
            &|| compact_with_header(&window),
            &|| compact_page(height, &search, &window, &compact_tail, width),
        ],
    )
}

// --- the Plugin page ---------------------------------------------------------

/// The status line under a plugin's title: its mark, then one phrase —
/// `✔ installed · enabled · v1.0.0 · claude-plugins-official`.
fn plugin_status_line(detail: &PluginDetail, width: u16) -> Line<'static> {
    let (mark, color) = status_mark(detail.status);
    let installed = detail.status.is_installed();
    let update = installed && detail.update_available;
    let mut words: Vec<String> = match detail.status {
        PluginStatus::Enabled | PluginStatus::Disabled => {
            let state = if detail.status == PluginStatus::Enabled {
                PLUGINS_STATUS_ENABLED
            } else {
                PLUGINS_STATUS_DISABLED
            };
            vec![PLUGINS_STATUS_INSTALLED.to_string(), state.to_string()]
        }
        PluginStatus::Available => vec![PLUGINS_STATUS_AVAILABLE.to_string()],
        PluginStatus::Unsupported => vec![PLUGINS_STATUS_BLOCKED.to_string()],
    };
    let dim = Style::new().fg(model_meta_color());
    let mut spans = Vec::new();
    if !mark.trim().is_empty() {
        spans.push(Span::styled(mark, Style::new().fg(color)));
        spans.push(Span::raw(" "));
    }
    if update {
        spans.push(Span::styled(
            format!("{}{FOOTER_SEPARATOR}", words.join(FOOTER_SEPARATOR)),
            dim,
        ));
        spans.push(Span::styled(
            format!("{PLUGINS_UPDATE_MARK} {PLUGINS_STATUS_UPDATE}"),
            Style::new().fg(ask_warning_color()),
        ));
        words.clear();
    }
    if detail.status != PluginStatus::Unsupported
        && let Some(version) = version_text(
            detail.installed_version.as_deref(),
            detail.version.as_deref(),
            update,
        )
    {
        words.push(version);
    }
    words.push(detail.id.marketplace.clone());
    let lead = if update { FOOTER_SEPARATOR } else { "" };
    spans.push(Span::styled(
        format!("{lead}{}", words.join(FOOTER_SEPARATOR)),
        dim,
    ));
    inset_row(spans, width)
}

/// A component row — `Skills  2 · code-review:lint, code-review:triage` —
/// with, for a plugin not installed, the dim note that its hooks or
/// servers will ask for review. Wrapped (`wrap`), every name is read under
/// the value's own column; on one row the count stays exact, the note keeps
/// its seat and the names are cut.
fn component_row(
    label: &str,
    names: &[String],
    review: bool,
    wrap: bool,
    label_cols: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let value = Style::new().fg(model_id_color());
    let dim = Style::new().fg(model_meta_color());
    let note = if review { PLUGINS_REVIEW_NOTE } else { "" };
    let text = format!("{}{FOOTER_SEPARATOR}{}", names.len(), names.join(", "));
    if wrap {
        let room = usize::from(width)
            .saturating_sub(cols(MODEL_INDENT) + label_cols)
            .max(1);
        let mut parts = vec![(text, value)];
        if review {
            parts.push((note.to_string(), dim));
        }
        let rows = super::inline::wrap_inline(&parts, u16::try_from(room).unwrap_or(u16::MAX));
        return rows
            .into_iter()
            .enumerate()
            .map(|(i, spans)| {
                let head = if i == 0 {
                    Span::styled(format!("{label:<label_cols$}"), dim)
                } else {
                    Span::raw(" ".repeat(label_cols))
                };
                let mut row = vec![Span::raw(MODEL_INDENT), head];
                row.extend(spans);
                Line::from(row)
            })
            .collect();
    }
    let room = usize::from(width)
        .saturating_sub(cols(MODEL_INDENT) + label_cols + cols(note))
        .max(1);
    let mut spans = vec![Span::styled(ellipsize(&text, room), value)];
    if review {
        spans.push(Span::styled(note, dim));
    }
    vec![fact_row(label, spans, label_cols, width)]
}

/// The component rows of a plugin's facts table, or the one `Contains` row
/// when they are not known until it is installed — and none for a plugin
/// that cannot be installed here, whose `Can't install` row says why.
fn component_rows(
    detail: &PluginDetail,
    wrap: bool,
    label_cols: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(components) = &detail.components else {
        if detail.blocked.is_some() || detail.status == PluginStatus::Unsupported {
            return Vec::new();
        }
        return wrapped_fact(
            PLUGINS_FACT_CONTAINS,
            PLUGINS_CONTAINS_LATER,
            Style::new().fg(model_meta_color()),
            None,
            label_cols,
            width,
        );
    };
    let review = detail.status == PluginStatus::Available;
    let mut rows = Vec::new();
    for (label, names, asks) in [
        (PLUGINS_FACT_SKILLS, &components.skills, false),
        (PLUGINS_FACT_COMMANDS, &components.commands, false),
        (PLUGINS_FACT_AGENTS, &components.agents, false),
        (PLUGINS_FACT_HOOKS, &components.hooks, review),
        (PLUGINS_FACT_MCP, &components.mcp_servers, review),
    ] {
        if !names.is_empty() {
            rows.extend(component_row(label, names, asks, wrap, label_cols, width));
        }
    }
    if !components.unsupported.is_empty() {
        rows.push(fact_row(
            PLUGINS_FACT_UNUSED,
            vec![Span::styled(
                components.unsupported.join(FOOTER_SEPARATOR),
                Style::new().fg(model_meta_color()),
            )],
            label_cols,
            width,
        ));
    }
    rows
}

/// A plugin's whole facts table: who made it, where it comes from, what it
/// carries (each list wrapped whole, or cut to a row: `wrap`), why it
/// cannot be installed or updated, what to watch for.
fn plugin_facts(
    menu: &PluginsMenu,
    detail: &PluginDetail,
    components_only: bool,
    wrap: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let value = Style::new().fg(model_id_color());
    // One label column for the whole table: the family's, widened only when
    // the longest label it shows needs it.
    let label_cols = if detail.blocked.is_some() {
        PLUGINS_FACT_COLS.max(cols(PLUGINS_FACT_BLOCKED) + PLUGINS_COLUMN_GAP)
    } else {
        PLUGINS_FACT_COLS
    };
    let mut rows = Vec::new();
    if !components_only {
        for (label, text) in [
            (PLUGINS_FACT_AUTHOR, &detail.author),
            (PLUGINS_FACT_CATEGORY, &detail.category),
            (PLUGINS_FACT_LICENSE, &detail.license),
        ] {
            if let Some(text) = text.as_ref().filter(|t| !t.trim().is_empty()) {
                rows.push(fact_row(
                    label,
                    vec![Span::styled(text.clone(), value)],
                    label_cols,
                    width,
                ));
            }
        }
        if !detail.source.trim().is_empty() {
            rows.extend(wrapped_fact(
                PLUGINS_FACT_SOURCE,
                &detail.source,
                value,
                detail.source_url.as_deref(),
                label_cols,
                width,
            ));
        }
        if let Some(homepage) = detail.homepage.as_ref().filter(|h| !h.trim().is_empty()) {
            let target = crate::links::is_whole_url(homepage).then_some(homepage.as_str());
            rows.extend(wrapped_fact(
                PLUGINS_FACT_HOMEPAGE,
                homepage,
                value,
                target,
                label_cols,
                width,
            ));
        }
    }
    rows.extend(component_rows(detail, wrap, label_cols, width));
    if detail.status.is_installed() && menu.marketplace_removed(&detail.id.marketplace) {
        rows.extend(wrapped_fact(
            PLUGINS_FACT_UPDATES,
            PLUGINS_NO_UPDATES_REMOVED,
            Style::new().fg(model_meta_color()),
            None,
            label_cols,
            width,
        ));
    }
    if let Some(blocked) = &detail.blocked {
        rows.extend(wrapped_fact(
            PLUGINS_FACT_BLOCKED,
            blocked,
            Style::new().fg(ask_warning_color()),
            None,
            label_cols,
            width,
        ));
    }
    if !components_only {
        for warning in detail.warnings.iter().take(PLUGINS_WARNINGS_MAX) {
            rows.push(inset_row(
                vec![Span::styled(
                    format!("{PLUGINS_WARNING_MARK} {warning}"),
                    Style::new().fg(model_meta_color()),
                )],
                width,
            ));
        }
    }
    rows
}

/// The Plugin page's actions as `(label, phrase)`.
fn plugin_action_labels(menu: &PluginsMenu) -> Vec<(String, Option<&'static str>)> {
    menu.plugin_actions()
        .into_iter()
        .map(|action| {
            let pair = |(label, phrase): (&str, &'static str)| (label.to_string(), Some(phrase));
            match action {
                PluginAction::Install => pair(PLUGINS_ACTION_INSTALL),
                PluginAction::Enable => pair(PLUGINS_ACTION_ENABLE),
                PluginAction::Disable => pair(PLUGINS_ACTION_DISABLE),
                PluginAction::CheckForUpdates => pair(PLUGINS_ACTION_CHECK),
                PluginAction::UpdateTo(version) => (
                    format!("{}{version}", PLUGINS_ACTION_UPDATE_TO.0),
                    Some(PLUGINS_ACTION_UPDATE_TO.1),
                ),
                PluginAction::Uninstall => pair(PLUGINS_ACTION_UNINSTALL),
                PluginAction::Back => (PLUGINS_ACTION_BACK.to_string(), None),
            }
        })
        .collect()
}

/// The one facts row a short Plugin page keeps: `Contains` over what the
/// plugin carries, summed up (`2 skills · 1 command · 5 agents`), or the row
/// saying that is known only after install — and nothing for a plugin that
/// carries nothing the session loads.
fn contains_row(detail: &PluginDetail, width: u16) -> Vec<Line<'static>> {
    match &detail.components {
        None => component_rows(detail, false, PLUGINS_FACT_COLS, width),
        Some(components) => {
            let summary = components.summary();
            if summary.is_empty() {
                return Vec::new();
            }
            vec![fact_row(
                PLUGINS_FACT_CONTAINS,
                vec![Span::styled(summary, Style::new().fg(model_id_color()))],
                PLUGINS_FACT_COLS,
                width,
            )]
        }
    }
}

/// The dim line under an enabled plugin's status saying what of it still
/// waits for the user, and where to go: `2 hooks wait for your review —
/// /hooks · 1 MCP server is off — /mcp` — or, for one the session has not
/// loaded yet, that the startup review asks at the next launch. Nothing
/// when everything it carries runs.
fn review_rows(menu: &PluginsMenu, detail: &PluginDetail, width: u16) -> Vec<Line<'static>> {
    if detail.status != PluginStatus::Enabled {
        return Vec::new();
    }
    let Some((hooks, servers, next_launch)) = menu.review_waiting() else {
        return Vec::new();
    };
    let pick =
        |n: usize, (one, many): (&'static str, &'static str)| if n == 1 { one } else { many };
    let text = if next_launch {
        let parts: Vec<String> = [
            (hooks, ("hook", "hooks")),
            (servers, ("MCP server", "MCP servers")),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, noun)| counted(n, noun))
        .collect();
        if parts.is_empty() {
            return Vec::new();
        }
        let one = parts.len() == 1 && hooks + servers == 1;
        format!(
            "{} {}",
            parts.join(" and "),
            if one {
                PLUGINS_WAIT_NEXT_LAUNCH.0
            } else {
                PLUGINS_WAIT_NEXT_LAUNCH.1
            }
        )
    } else {
        let mut parts = Vec::new();
        if hooks > 0 {
            parts.push(format!("{hooks} {}", pick(hooks, PLUGINS_HOOKS_WAIT)));
        }
        if servers > 0 {
            parts.push(format!("{servers} {}", pick(servers, PLUGINS_SERVERS_OFF)));
        }
        if parts.is_empty() {
            return Vec::new();
        }
        parts.join(FOOTER_SEPARATOR)
    };
    model_wrapped_rows(&text, model_meta_color(), width)
}

/// The confirmation the highlighted action wears when armed.
fn plugin_armed(menu: &PluginsMenu) -> Option<&'static str> {
    (menu.armed() && menu.plugin_actions().get(menu.action()) == Some(&PluginAction::Uninstall))
        .then_some(PLUGINS_UNINSTALL_ARMED)
}

/// The Plugin page — the first of its rungs that fits `height`: the whole
/// page, what it brings wrapped whole, then cut to a row a list; the
/// description cut to two rows and the facts to the components, wrapped,
/// then cut;
/// the title and the status line over one `Contains` row summing up what it
/// carries ([`contains_row`]), the result and the actions; the same without
/// that row; then the small
/// rungs ([`small_rungs`]). While its detail loads, or after it failed, the
/// title, `Loading…` or the reason in red, and `1. Back`.
fn plugin_lines(app: &App, menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let subject = menu
        .subject
        .as_ref()
        .map(|id| menu.plugin_label(id))
        .unwrap_or_default();
    let title = crumb_title(&subject, width);
    let actions = plugin_action_labels(menu);
    let armed = plugin_armed(menu);
    let (choices, hint) = match &menu.job {
        Some(job) => (vec![job_row(app, menu, job, width)], cancel_hint(width)),
        None => (
            action_rows(&actions, menu.action(), armed, width),
            detail_hint(width),
        ),
    };
    let result = menu
        .notice
        .as_ref()
        .filter(|_| menu.job.is_none())
        .map(|notice| notice_rows(notice, width))
        .unwrap_or_default();
    let focus = || match &menu.job {
        Some(_) => (0, choices.clone(), 0),
        None => action_focus(&actions, menu.action(), armed, width),
    };
    let contains = match &menu.detail {
        Some(Ok(detail)) => contains_row(detail, width),
        _ => Vec::new(),
    };
    // The facts in four sizes: every fact or the components alone, each
    // with what it brings wrapped whole or cut to a row.
    let (head, body, short_body, facts) = match &menu.detail {
        Some(Ok(detail)) => {
            let dim = Style::new().fg(model_id_color());
            let mut head = vec![title.clone(), plugin_status_line(detail, width)];
            head.extend(review_rows(menu, detail, width));
            (
                head,
                capped_rows(
                    MODEL_INDENT,
                    &detail.description,
                    dim,
                    width,
                    PLUGINS_DETAIL_DESCRIPTION_ROWS,
                ),
                capped_rows(
                    MODEL_INDENT,
                    &detail.description,
                    dim,
                    width,
                    PLUGINS_SHORT_DESCRIPTION_ROWS,
                ),
                [
                    plugin_facts(menu, detail, false, true, width),
                    plugin_facts(menu, detail, false, false, width),
                    plugin_facts(menu, detail, true, true, width),
                    plugin_facts(menu, detail, true, false, width),
                ],
            )
        }
        Some(Err(reason)) => {
            let failed = model_wrapped_rows(reason, tool_fail_color(), width);
            (
                vec![title.clone()],
                failed.clone(),
                failed,
                Default::default(),
            )
        }
        None => {
            let loading = vec![model_placeholder_row(
                PLUGINS_DETAIL_LOADING,
                model_meta_color(),
                width,
            )];
            (
                vec![title.clone()],
                loading.clone(),
                loading,
                Default::default(),
            )
        }
    };
    let page = |body: &Vec<Line<'static>>, facts: &Vec<Line<'static>>| {
        framed(
            vec![
                head.clone(),
                body.clone(),
                facts.clone(),
                result.clone(),
                choices.clone(),
                vec![hint.clone()],
            ],
            width,
        )
    };
    first_page_that_fits(
        height,
        &[
            &|| page(&body, &facts[0]),
            &|| page(&body, &facts[1]),
            &|| page(&short_body, &facts[2]),
            &|| page(&short_body, &facts[3]),
            &|| {
                framed(
                    vec![
                        head.clone(),
                        contains.clone(),
                        result.clone(),
                        choices.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| {
                framed(
                    vec![
                        head.clone(),
                        result.clone(),
                        choices.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| small_rungs(height, &title, focus(), &hint, width),
        ],
    )
}

// --- the Marketplace page ----------------------------------------------------

/// The Marketplace page's actions as `(label, phrase)`.
fn market_action_labels(menu: &PluginsMenu) -> Vec<(String, Option<&'static str>)> {
    menu.marketplace_actions()
        .into_iter()
        .map(|action| {
            let pair = |(label, phrase): (&str, &'static str)| (label.to_string(), Some(phrase));
            match action {
                MarketplaceAction::BrowsePlugins => pair(PLUGINS_ACTION_BROWSE),
                MarketplaceAction::FetchNow => pair(PLUGINS_ACTION_FETCH),
                MarketplaceAction::UpdateNow => pair(PLUGINS_ACTION_UPDATE),
                MarketplaceAction::ReadAgain => pair(PLUGINS_ACTION_READ),
                MarketplaceAction::Remove => pair(PLUGINS_ACTION_REMOVE),
                MarketplaceAction::Back => (PLUGINS_ACTION_BACK.to_string(), None),
            }
        })
        .collect()
}

/// The Marketplace page's line under its title: `317 plugins · 2 installed
/// · updated 2h ago · default marketplace`.
fn market_summary(
    menu: &PluginsMenu,
    index: usize,
    market: &CatalogMarketplace,
    width: u16,
) -> Line<'static> {
    let catalog = menu.catalog.as_ref();
    let mut words = Vec::new();
    if market.fetched {
        words.push(counted(
            catalog.map_or(0, |c| c.count_in(index)),
            PLUGINS_PLUGIN_NOUN,
        ));
        let installed = catalog.map_or(0, |c| {
            c.plugins
                .iter()
                .filter(|p| {
                    p.marketplace == index
                        && matches!(p.status, PluginStatus::Enabled | PluginStatus::Disabled)
                })
                .count()
        });
        words.push(format!("{installed} {PLUGINS_INSTALLED_WORD}"));
        if !market.skipped.is_empty() {
            words.push(format!("{} {PLUGINS_SKIPPED_WORD}", market.skipped.len()));
        }
        if market.is_local {
            words.push(PLUGINS_LOCAL_FACT.to_string());
        } else if let Some(updated) = market.updated {
            words.push(format!(
                "{PLUGINS_UPDATED} {}",
                super::model_view::age_text(menu.age_of(updated))
            ));
        }
    } else if market.is_removed() {
        words.push(PLUGINS_REMOVED_FACT.to_string());
    } else {
        words.push(PLUGINS_NOT_FETCHED_FACT.to_string());
    }
    if market.is_default {
        words.push(PLUGINS_DEFAULT_FACT.to_string());
    }
    model_placeholder_row(&words.join(FOOTER_SEPARATOR), model_meta_color(), width)
}

/// The Marketplace page's facts: its source (a link to its web page when
/// it has one), the revision fetched, and where its checkout is kept.
fn market_facts_rows(
    menu: &PluginsMenu,
    market: &CatalogMarketplace,
    width: u16,
) -> Vec<Line<'static>> {
    let value = Style::new().fg(model_id_color());
    let mut rows = wrapped_fact(
        PLUGINS_FACT_SOURCE,
        &market.source,
        value,
        market.web_url.as_deref(),
        PLUGINS_FACT_COLS,
        width,
    );
    if !market.is_local
        && let Some(revision) = &market.revision
    {
        let short: String = revision.chars().take(PLUGINS_REVISION_COLS).collect();
        rows.push(fact_row(
            PLUGINS_FACT_REVISION,
            vec![Span::styled(short, value)],
            PLUGINS_FACT_COLS,
            width,
        ));
    }
    if !market.is_local
        && market.fetched
        && let Some(home) = menu.home()
    {
        rows.extend(wrapped_fact(
            PLUGINS_FACT_KEPT,
            &format!("{home}/plugins/marketplaces/{}", market.name),
            value,
            None,
            PLUGINS_FACT_COLS,
            width,
        ));
    }
    rows
}

/// The entries a marketplace lists that could not be read, one sentence
/// each under a `Skipped` label, wrapped whole.
fn skipped_rows(market: &CatalogMarketplace, width: u16) -> Vec<Line<'static>> {
    market
        .skipped
        .iter()
        .enumerate()
        .flat_map(|(i, sentence)| {
            let label = if i == 0 { PLUGINS_FACT_SKIPPED } else { "" };
            wrapped_fact(
                label,
                sentence,
                Style::new().fg(ask_warning_color()),
                None,
                PLUGINS_FACT_COLS,
                width,
            )
        })
        .collect()
}

/// The Marketplace page — its whole page, then without the entries it
/// skipped, then without its facts, then the small rungs. A fetch of it
/// under way or failed shows above the actions, where a job's result goes.
fn market_lines(app: &App, menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let name = menu.marketplace.clone().unwrap_or_default();
    let title = crumb_title(&name, width);
    let found = menu
        .marketplace_index(&name)
        .and_then(|i| menu.catalog.as_ref().map(|c| (i, &c.marketplaces[i])));
    let actions = market_action_labels(menu);
    let armed = (menu.armed()
        && menu.marketplace_actions().get(menu.action()) == Some(&MarketplaceAction::Remove))
    .then_some(PLUGINS_REMOVE_ARMED);
    let (choices, hint) = match &menu.job {
        Some(job) => (vec![job_row(app, menu, job, width)], cancel_hint(width)),
        None => (
            action_rows(&actions, menu.action(), armed, width),
            detail_hint(width),
        ),
    };
    let mut result = fetch_rows(app, menu, &name, width);
    if menu.job.is_none()
        && let Some(notice) = &menu.notice
    {
        result.extend(notice_rows(notice, width));
    }
    let mut head = vec![title.clone()];
    let mut facts = Vec::new();
    let mut skipped = Vec::new();
    if let Some((index, market)) = found {
        skipped = skipped_rows(market, width);
        head.push(market_summary(menu, index, market, width));
        if let Some(error) = market.error.as_ref().filter(|_| !market.is_removed()) {
            head.extend(model_wrapped_rows(
                &format!("{PLUGINS_UNREADABLE} {name} — {error}"),
                tool_fail_color(),
                width,
            ));
        }
        facts = market_facts_rows(menu, market, width);
    }
    let focus = || match &menu.job {
        Some(_) => (0, choices.clone(), 0),
        None => action_focus(&actions, menu.action(), armed, width),
    };
    first_page_that_fits(
        height,
        &[
            &|| {
                framed(
                    vec![
                        head.clone(),
                        facts.clone(),
                        skipped.clone(),
                        result.clone(),
                        choices.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| {
                framed(
                    vec![
                        head.clone(),
                        facts.clone(),
                        result.clone(),
                        choices.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| {
                framed(
                    vec![
                        head.clone(),
                        result.clone(),
                        choices.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| small_rungs(height, &title, focus(), &hint, width),
        ],
    )
}

// --- the Add page ------------------------------------------------------------

/// The Add page's field as it is shown: what was typed, its head given up
/// to a `…` when it is wider than the row, so its end — where the caret
/// sits — stays in view.
pub(super) fn plugins_field_text(input: &str, width: u16) -> String {
    let room = usize::from(width)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_PROMPT) + 1)
        .max(1);
    if cols(input) <= room {
        return input.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = cols(TOOL_HEADER_ELLIPSIS);
    for c in input.chars().rev() {
        let w = cols(c.encode_utf8(&mut [0; 4]));
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    let tail: String = kept.into_iter().rev().collect();
    format!("{TOOL_HEADER_ELLIPSIS}{tail}")
}

/// The Add page — the title over what a marketplace is; the `❯` field; the
/// examples; the result or the job; the hint — then without the examples,
/// then the small rungs around the field.
fn add_lines(app: &App, menu: &PluginsMenu, width: u16, height: u16) -> Vec<Line<'static>> {
    let title = crumb_title(PLUGINS_ADD_TITLE, width);
    let field = Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
        Span::raw(plugins_field_text(&menu.input, width)),
    ]);
    let head = {
        let mut head = vec![title.clone()];
        head.extend(model_wrapped_rows(
            PLUGINS_ADD_SUBTITLE,
            model_meta_color(),
            width,
        ));
        head
    };
    let examples: Vec<Line<'static>> = PLUGINS_ADD_EXAMPLES
        .iter()
        .enumerate()
        .map(|(i, example)| {
            let label = if i == 0 { PLUGINS_FACT_EXAMPLES } else { "" };
            fact_row(
                label,
                vec![Span::styled(*example, Style::new().fg(model_id_color()))],
                PLUGINS_FACT_COLS,
                width,
            )
        })
        .collect();
    let result = status_block(app, menu, width);
    let hint = if menu.job.is_some() {
        cancel_hint(width)
    } else {
        hint_row(&[PLUGINS_ADD_HINT], width)
    };
    first_page_that_fits(
        height,
        &[
            &|| {
                framed(
                    vec![
                        head.clone(),
                        vec![field.clone()],
                        examples.clone(),
                        result.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| {
                framed(
                    vec![
                        head.clone(),
                        vec![field.clone()],
                        result.clone(),
                        vec![hint.clone()],
                    ],
                    width,
                )
            },
            &|| small_rungs(height, &title, (0, vec![field.clone()], 0), &hint, width),
        ],
    )
}

// --- the page -----------------------------------------------------------------

/// The open page as lines, built for `height` rows — the first of the
/// current view's rungs that fits, so it never flows. What
/// [`render_plugins_menu`] paints, [`plugins_menu_rows`] counts and the
/// cursor seat reads, so the three can never disagree. Empty when the page
/// is closed.
pub(super) fn plugins_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(menu) = app.plugins_menu.as_ref() else {
        return Vec::new();
    };
    match menu.page {
        PluginsPage::Browse => browse_lines(app, menu, width, height),
        PluginsPage::Plugin => plugin_lines(app, menu, width, height),
        PluginsPage::Marketplace => market_lines(app, menu, width, height),
        PluginsPage::AddMarketplace => add_lines(app, menu, width, height),
    }
}

/// The rows the page's own frame occupies on a terminal `height` rows tall.
pub(super) fn plugins_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(plugins_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/plugins` page is open, or
/// `None` when it isn't. Like every sibling the page replaces the composer
/// and only the composer: the streaming strip keeps its rows above it.
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

/// Render the `/plugins` page into the live region, in place of the
/// composer — the page built for the area's height, so it always fits and
/// nothing flows (`docs/view-flow.md`). Pure — `render_live` paints this.
/// See `docs/plugins.md`.
pub fn render_plugins_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        plugins_view_lines(app, area.width, area.height),
    );
}

//! The `/plugins` page's view (`docs/plugins-menu.md`) — codex's `/plugins`
//! in the host's picker frame, drawn through a [`PageKit`].
//!
//! The **list** is a search-line picker under the ask modal's chip strip of
//! tabs — every plugin, the installed ones, a tab per marketplace (its own
//! header saying where it comes from and what it skipped), and the add tab,
//! whose field is the search line and whose rows are the official catalogs
//! over the row that opens a field for any other source — shedding rows
//! down the picker ladder on a short terminal so the search line and the
//! highlighted row stay on screen. A **plugin's** page is the `/mcp` server
//! page's twin — fact rows over numbered actions, bottom-anchored when
//! taller than the terminal — the **add** page is one field, and the
//! **confirmation** is two actions under its question.
//!
//! **A job is said in the hint's own row** once it has run
//! [`PLUGINS_JOB_DELAY`](super::PLUGINS_JOB_DELAY), and what went right is
//! never said: the rows and pages show it — so acting on the page never
//! moves it (`docs/plugins-menu.md` *No flicker*).
//!
//! One builder, [`PluginsPage::build`], makes every page *with its caret*,
//! so the host's reserved height, paint, cursor seat and flow all read the
//! same page.
//!
//! **Why a kit.** This crate cannot depend on the main one, and the page must
//! look like every other picker: the family's rows, ladder and colours are
//! the main crate's. So the view asks for them through [`PageKit`], which
//! the main crate implements with thin delegations to its own helpers
//! (`src/ui/plugins_view.rs`) — the page's layout is here, the family's look
//! is there, and neither is copied.

use std::collections::BTreeMap;
use std::ops::Range;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::rows::PluginRef;
use super::state::{
    AddRow, MarketplaceRow, PluginAction, PluginContents, PluginsConfirm, PluginsPage,
    PluginsScreen, PluginsTab, plugin_actions,
};

/// The colours the page paints, by role — each one the main crate's theme
/// accessor of the same purpose, so `/theme` recolours the page with the
/// rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KitColor {
    /// Dim: counters, hints, descriptions, the status words of what is off.
    Meta,
    /// The accent: the selection, the `❯` prompt, a running job's label.
    Accent,
    /// A row's muted name; the add field's text.
    Muted,
    /// The one green: `✔ enabled`.
    Ok,
    /// The one red mark: `✘ not loaded`.
    Fail,
    /// An error's text: a failure, a session-off note.
    Error,
    /// The amber: `▲ update`, a marketplace's skipped entries.
    Warning,
    /// A status word that is live (`enabled`).
    ValueOn,
    /// A status word that is not (`disabled`, `available`).
    ValueOff,
    /// The current tab chip's ink and ground.
    ChipFg,
    ChipBg,
    /// An idle tab chip.
    ChipIdle,
    /// A plugin page's fact label, value, named state and description.
    Label,
    Value,
    State,
    Description,
}

/// What the page takes from its host's picker family — the frame, the rows,
/// the ladder, the measuring and the colours — so it reads as one of them.
/// The main crate implements it over its own helpers; a test may implement
/// it plainly. Every method is a plain one, so nothing generic is
/// instantiated across the crate boundary.
pub trait PageKit {
    /// Display columns of `text`.
    fn cols(&self, text: &str) -> usize;
    /// `text` cut to `max` columns, closed by `…` when cut.
    fn ellipsize(&self, text: &str, max: usize) -> String;
    /// `spans` as one line cut to `width` columns.
    fn clamp(&self, spans: Vec<Span<'static>>, width: usize) -> Line<'static>;
    /// Styled `segments` word-wrapped to rows of `width` columns.
    fn wrap_spans(&self, segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>>;
    /// `text` split into runs under `base`, each address a link to itself.
    fn linked_segments(&self, text: &str, base: Style) -> Vec<(String, Style)>;
    /// `text`, when it is one whole address, as the one run that links it.
    fn url_run(&self, text: &str, base: Style) -> Option<(String, Style)>;
    /// How long ago, in one unit: `just now`, `3m ago`, `2h ago`.
    fn age(&self, seconds: i64) -> String;
    /// A duration as the status line says it: `4s`, `1m 5s`.
    fn elapsed(&self, secs: u64) -> String;
    /// The text width of a typed field `cells` wide — one column kept for
    /// the caret.
    fn field_width(&self, cells: u16) -> u16;

    /// The pages' inset (`"  "`).
    fn indent(&self) -> &'static str;
    /// The search line's and fields' prompt (`"❯ "`).
    fn prompt(&self) -> &'static str;
    /// The highlighted action row's marker (`"❯ "`), where the hidden
    /// cursor seats.
    fn action_marker(&self) -> &'static str;
    /// The tab strip's gap between chips.
    fn chip_gap(&self) -> &'static str;
    /// The gap between a list's columns.
    fn value_gap(&self) -> usize;
    /// How many rows a whole list page shows where the terminal has room —
    /// the `/theme` picker's ten-row window.
    fn tall_list_rows(&self) -> usize;
    /// How many rows the next rung down shows — the picker family's five.
    fn list_rows(&self) -> usize;
    /// A colour by role.
    fn color(&self, role: KitColor) -> Color;

    /// A framed page's full-width rule.
    fn rule(&self, width: u16) -> Line<'static>;
    /// A page's title, in the pages' title dress.
    fn title(&self, text: &str, width: u16) -> Line<'static>;
    /// The `❯` search line carrying `query`.
    fn search_line(&self, query: &str) -> Line<'static>;
    /// A list row's head: the inset, the marker (or its width blank) and the
    /// name in the selection's or the muted dress.
    fn row_head(&self, name: &str, selected: bool) -> Vec<Span<'static>>;
    /// The columns [`row_head`](Self::row_head) spends before a name.
    fn row_head_cols(&self) -> usize;
    /// The `(n/total)` counter under a list.
    fn counter(&self, selected: usize, total: usize) -> Line<'static>;
    /// One inset row of `text` in `color`, cut to the width.
    fn placeholder_row(&self, text: &str, color: Color, width: u16) -> Line<'static>;
    /// `text` wrapped to inset rows in `color`.
    fn wrapped_rows(&self, text: &str, color: Color, width: u16) -> Vec<Line<'static>>;
    /// [`wrapped_rows`](Self::wrapped_rows) with every address a link.
    fn linked_rows(&self, text: &str, color: Color, width: u16) -> Vec<Line<'static>>;
    /// The highlighted row's description, one dim row hanging under the
    /// names.
    fn description_row(&self, text: &str, width: u16) -> Line<'static>;
    /// The row of `lines` carrying the `❯` prompt.
    fn prompt_row(&self, lines: &[Line<'static>]) -> Option<u16>;

    /// The window of a `len`-row list keeping `selected` in view: rows
    /// hidden above, the visible range, rows hidden below.
    fn list_window(&self, len: usize, selected: usize, max: usize) -> (usize, Range<usize>, usize);
    /// A search-line picker's page: rule, header and gap, search, body, rule.
    fn picker_page(
        &self,
        header: &[Line<'static>],
        search: &Line<'static>,
        body: Vec<Line<'static>>,
        width: u16,
    ) -> Vec<Line<'static>>;
    /// The compact page with its header kept.
    fn compact_page_with_header(
        &self,
        header: &[Line<'static>],
        search: &Line<'static>,
        window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
        tail: &[Line<'static>],
        width: u16,
    ) -> Vec<Line<'static>>;
    /// The compact page and its rungs, down to the search line alone.
    fn compact_page(
        &self,
        height: u16,
        search: &Line<'static>,
        window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
        tail: &[Line<'static>],
        width: u16,
    ) -> Vec<Line<'static>>;
    /// A placeholder block as a compact page's window.
    fn placeholder_window(
        &self,
        block: &[Line<'static>],
        max: usize,
    ) -> (usize, Vec<Line<'static>>, usize);
    /// The first of `pages` whose rows fit `height`.
    fn first_page_that_fits(
        &self,
        height: u16,
        pages: &[&dyn Fn() -> Vec<Line<'static>>],
    ) -> Vec<Line<'static>>;
}

/// A built page, and its caret on a page that is typed into — the list's
/// search line, which is the add tab's field too, and the add page's field —
/// as `(column, page row)`. `None` on a page of actions, whose hidden cursor
/// seats on its `❯` instead.
pub struct PageBuild {
    pub lines: Vec<Line<'static>>,
    pub cursor: Option<(u16, usize)>,
}

/// `text` with its first letter upper-cased.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

// --- the page's own vocabulary. Everything it shares with the picker
// family — the frame, the rows, the marks' colours, the chips' dress — comes
// through the kit; only the words and the page's own geometry live here. ---

/// The family's clause separator, ` · ` (the main crate's
/// `FOOTER_SEPARATOR`).
const SEP: &str = " · ";

/// The list page's title.
const PLUGINS_TITLE: &str = "Plugins";

/// The tab strip's fixed tabs (`app::PluginsTab`): every plugin, the
/// installed ones and — last, after a tab per marketplace — the add tab.
const PLUGINS_TAB_ALL: &str = "All";
const PLUGINS_TAB_INSTALLED: &str = "Installed";
const PLUGINS_TAB_ADD: &str = "+ Add";

/// The marks a strip too wide for the terminal shows where tabs are hidden
/// (codex's): `‹` at its left, `›` at its right.
const PLUGINS_STRIP_MORE_LEFT: &str = "‹";
const PLUGINS_STRIP_MORE_RIGHT: &str = "›";

/// Each tab's dim subtitle under the title. A marketplace's tab says its
/// own description instead, when it has one.
const PLUGINS_ALL_SUBTITLE: &str = "Every plugin your marketplaces list";
const PLUGINS_INSTALLED_SUBTITLE: &str = "The plugins installed on this machine";
const PLUGINS_ADD_SUBTITLE: &str = "Add a marketplace: owner/repo, a git URL, or a directory";
const PLUGINS_ADD_COUNT: &str = "Pick one below or type a source";

/// The forms a marketplace source takes — the add tab's and the add page's
/// one sentence on it.
const PLUGINS_SOURCE_FORMS: &str =
    "owner/repo, an https:// or git@ URL, or a directory (/abs, ./rel, ~/…)";

/// The add tab's last row while nothing is typed — it opens the add page —
/// and what it says the page's field takes.
const PLUGINS_ADD_ROW: &str = "Add a marketplace…";
const PLUGINS_ADD_ROW_DESCRIPTION: &str = "owner/repo, git URL or path";

/// The add page: its title, what a marketplace is, and the field's dim
/// placeholder.
const PLUGINS_ADD_TITLE: &str = "Add a marketplace";
const PLUGINS_ADD_BLURB: &str = "A git repository or a directory holding a plugin marketplace — or a single plugin, read as a marketplace of one.";
const PLUGINS_ADD_PLACEHOLDER: &str = "owner/repo";

/// The status column's marks: the `/skills` rows' `✔`/`◯` for an installed
/// plugin on or off here, `+` for one that can be installed, `–` for one
/// whose source this app cannot fetch, and `/mcp`'s `✘` for an installed copy
/// a session does not load. Only the `✔` is green and only the `✘` red: the
/// eye hunts down the column for what is on and what is broken.
const PLUGINS_ENABLED_MARK: &str = "✔";
const PLUGINS_DISABLED_MARK: &str = "◯";
const PLUGINS_AVAILABLE_MARK: &str = "+";
const PLUGINS_UNAVAILABLE_MARK: &str = "–";
const PLUGINS_BROKEN_MARK: &str = "✘";

/// The status column's words, beside those marks — and the add tab's for an
/// official catalog already added.
const PLUGINS_ENABLED: &str = "enabled";
const PLUGINS_DISABLED: &str = "disabled";
const PLUGINS_AVAILABLE: &str = "available";
const PLUGINS_UNAVAILABLE: &str = "unavailable";
const PLUGINS_BROKEN: &str = "not loaded";
const PLUGINS_ADDED: &str = "added";

/// The amber mark an installed plugin's description opens on when its
/// marketplace has a newer copy: `▲ update · Easily create hooks…`.
const PLUGINS_UPDATE_MARK: &str = "▲";
const PLUGINS_UPDATE_WORD: &str = "update";

/// The key hints' clauses, each with the order it gives way in on a narrow
/// terminal — highest first, `0` never — so a hint drops whole clauses
/// rather than ending on a cut word.
const PLUGINS_HINT_UPDATE: (&str, u8) = ("Ctrl+U update", 2);
const PLUGINS_HINT_REMOVE: (&str, u8) = ("Ctrl+R remove", 1);
const PLUGINS_HINT_TABS: (&str, u8) = ("←/→ tabs", 4);
const PLUGINS_HINT_OPEN: (&str, u8) = ("Enter to open", 3);
const PLUGINS_HINT_TOGGLE: (&str, u8) = ("Space to toggle", 5);
const PLUGINS_HINT_CLOSE: (&str, u8) = ("Esc to close", 0);
const PLUGINS_HINT_CLEAR: (&str, u8) = ("Esc to clear", 0);
const PLUGINS_HINT_TYPE: (&str, u8) = ("type a source", 5);
const PLUGINS_HINT_ADD: (&str, u8) = ("Enter to add", 0);

/// A detail page's hint and the confirmation's, clause by clause in the
/// order they give way (the list's rule): the way back is never left out.
const PLUGINS_PAGE_HINT: [(&str, u8); 3] = [
    ("↑/↓ to navigate", 2),
    ("Enter to select", 1),
    ("Esc to go back", 0),
];
const PLUGINS_CONFIRM_HINT: [(&str, u8); 2] = [("Enter to confirm", 1), ("Esc to go back", 0)];
const PLUGINS_ADD_HINT: [(&str, u8); 2] = [("Enter to add", 1), ("Esc to go back", 0)];

/// The list's placeholders: a search matching nothing, and the two empty
/// tabs — each naming where to go next.
const PLUGINS_NO_MATCH: &str = "No matching plugins";
/// Between the no-match row and where the search does match.
const SEP_DASH: &str = " — ";
const PLUGINS_NONE_INSTALLED: &str =
    "No plugins installed yet — find one under All, or add a marketplace under + Add.";
const PLUGINS_NONE_LISTED: &str = "No plugins listed yet — add a marketplace under + Add.";

/// The note under the count when the session loads no plugin
/// (`ALTER_ZERO_PLUGINS=0`): the page still installs and toggles, red like
/// the `/skills` and `/mcp` session-off notes.
const PLUGINS_SESSION_OFF: &str =
    "Plugins are off for this session (ALTER_ZERO_PLUGINS) — nothing installed is loaded";

/// What the add tab's typed row says.
const PLUGINS_ADD_TYPED: &str = "Press Enter to add it";
/// How to pin a source — `@ref` on the shorthand, `#ref` on any git source.
const PLUGINS_ADD_PIN: &str = "Pin a branch, tag or commit: owner/repo@ref, or #ref on a git URL";
/// The line under the list for a plugin whose manifest describes nothing.
const PLUGINS_NO_DESCRIPTION: &str = "No description";

/// A plugin page's `Contents:` while its contents are being read, for one
/// whose contents are read only once it is installed, and for a read that
/// failed (its message follows).
const PLUGINS_CONTENTS_READING: &str = "reading…";
const PLUGINS_CONTENTS_UNKNOWN: &str = "read once it is installed";
const PLUGINS_CONTENTS_UNREADABLE: &str = "could not be read";
/// The word a preview gives hooks and MCP servers it saw but could not
/// count — and the clause after them on every page: they are reviewed
/// before they run (`docs/config-review.md`).
const PLUGINS_CONTENTS_PRESENT: &str = "included";
const PLUGINS_REVIEWED: &str = "reviewed before they run";

/// A plugin page's warning for one not yet installed that brings something
/// that runs — or whose contents are read only once it is installed — the
/// risk in Claude Code's opening words, then this app's own guard.
const PLUGINS_TRUST_NOTE: &str = "Plugins can run code on your computer — install only plugins you trust. Any hooks or MCP servers it brings wait for your review before they run.";

/// The job line's closing clause: `Installing deploy… (4s · esc to cancel)`.
const PLUGINS_JOB_CANCEL: &str = "esc to cancel";

/// The widest a list row's name column grows, so a long name never pushes
/// the descriptions off the row; and the narrowest it is cut to.
const PLUGINS_NAME_COL_MAX: usize = 32;
const PLUGINS_NAME_COL_MIN: usize = 12;

/// The narrowest a row's description is cut to: with less room it is left
/// out whole, since a stub like `clau…` says nothing.
const PLUGINS_ROW_TEXT_MIN: usize = 8;

/// The room a row's description keeps beside a marketplace column. With
/// less, the rows leave the marketplace to the facts under the list.
const PLUGINS_ROW_MARKET_TEXT_MIN: usize = 30;

/// The most rows a marketplace's skipped-entry note takes in its header.
const PLUGINS_MARKET_NOTE_MAX_ROWS: usize = 3;

/// The gap between a fact's label and its value on a plugin's page, whose
/// label column is sized to the labels it shows.
const PLUGINS_FACT_LABEL_GAP: usize = 2;

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

/// `text` with its runs of whitespace folded to single spaces — a
/// description that wraps in its manifest still fits one row.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line a running job shows — `Installing deploy… fetching
/// openai/plugins (4s · esc to cancel)` — the label in the accent, the
/// boundary's step and the clock dim: the status line's grammar. Cut at the
/// width, never wrapped, so it never moves the page.
fn job_line(
    kit: &dyn PageKit,
    label: &str,
    step: Option<&str>,
    secs: u64,
    width: u16,
) -> Line<'static> {
    let dim = Style::new().fg(kit.color(KitColor::Meta));
    let mut spans = vec![
        Span::raw(kit.indent()),
        Span::styled(
            label.to_string(),
            Style::new().fg(kit.color(KitColor::Accent)),
        ),
    ];
    if let Some(step) = step.filter(|step| !step.trim().is_empty()) {
        spans.push(Span::styled(format!(" {}", flat(step)), dim));
    }
    spans.push(Span::styled(
        format!(" ({} · {PLUGINS_JOB_CANCEL})", kit.elapsed(secs)),
        dim,
    ));
    kit.clamp(spans, usize::from(width))
}

/// `clauses` as one hint row that fits `width`: every clause while they
/// fit, else the ones that give way first (the highest rank; `0` never)
/// left out whole — a hint never ends on a cut word, and always keeps the
/// key that closes the page.
fn fitted_hint(kit: &dyn PageKit, clauses: &[(&str, u8)], width: u16) -> Line<'static> {
    let mut kept: Vec<(&str, u8)> = clauses.to_vec();
    let text = |kept: &[(&str, u8)]| {
        kept.iter()
            .map(|(clause, _)| *clause)
            .collect::<Vec<_>>()
            .join(SEP)
    };
    while kit.cols(kit.indent()) + kit.cols(&text(&kept)) > usize::from(width) {
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
    kit.placeholder_row(&text(&kept), kit.color(KitColor::Meta), width)
}

/// The list's hint for its tab: a marketplace's tab leads on its own keys
/// (Ctrl+U only for a git one, which is all an update can fetch), the add
/// tab says how to add, and Space is offered only where it does something —
/// on an installed plugin's row. Esc says what it will do: clear the search,
/// or close.
fn list_hint(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> Line<'static> {
    let esc = if menu.query.is_empty() {
        PLUGINS_HINT_CLOSE
    } else {
        PLUGINS_HINT_CLEAR
    };
    let mut clauses = Vec::new();
    match &menu.tab {
        PluginsTab::Add => {
            clauses.extend([PLUGINS_HINT_TABS, PLUGINS_HINT_TYPE, PLUGINS_HINT_ADD, esc]);
            return fitted_hint(kit, &clauses, width);
        }
        PluginsTab::Marketplace(_) => {
            if menu.current_marketplace().is_some_and(|market| market.git) {
                clauses.push(PLUGINS_HINT_UPDATE);
            }
            clauses.push(PLUGINS_HINT_REMOVE);
        }
        PluginsTab::All | PluginsTab::Installed => {}
    }
    clauses.extend([PLUGINS_HINT_TABS, PLUGINS_HINT_OPEN]);
    if menu
        .highlighted_plugin()
        .is_some_and(|plugin| plugin.installed().is_some())
    {
        clauses.push(PLUGINS_HINT_TOGGLE);
    }
    clauses.push(esc);
    fitted_hint(kit, &clauses, width)
}

/// The page's one status row, in the hint's own seat so it never moves the
/// page: the job running, once it has run long enough to be said; else the
/// last failure, in red, cut to the row (the next key brings the hint
/// back); else `hint`. What a job did is never said here — the rows and
/// pages show it.
fn status_row(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    hint: Line<'static>,
    width: u16,
) -> Line<'static> {
    match (menu.job.as_ref().filter(|_| menu.job_shown()), &menu.error) {
        (Some(job), _) => job_line(
            kit,
            &job.label,
            job.step.as_deref(),
            menu.job_elapsed().as_secs(),
            width,
        ),
        (None, Some(error)) => kit.placeholder_row(&flat(error), kit.color(KitColor::Error), width),
        (None, None) => hint,
    }
}

// ===== the list =====

/// A tab's chip label: its name and how many rows it lists — the add tab
/// has no count.
fn tab_label(menu: &PluginsPage, tab: &PluginsTab) -> String {
    let plugins = &menu.snapshot.plugins;
    let (name, count) = match tab {
        PluginsTab::All => (PLUGINS_TAB_ALL.to_string(), Some(plugins.len())),
        PluginsTab::Installed => (
            PLUGINS_TAB_INSTALLED.to_string(),
            Some(menu.snapshot.installed_count()),
        ),
        PluginsTab::Marketplace(name) => (
            name.clone(),
            Some(plugins.iter().filter(|p| p.marketplace() == *name).count()),
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
fn tab_strip(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> Line<'static> {
    let tabs = menu.tabs();
    let labels: Vec<String> = tabs.iter().map(|tab| tab_label(menu, tab)).collect();
    let current = tabs.iter().position(|tab| *tab == menu.tab).unwrap_or(0);
    let room = usize::from(width).saturating_sub(kit.cols(kit.indent()));
    let gap = kit.cols(kit.chip_gap());
    let span = |lo: usize, hi: usize| {
        let chips: usize = labels[lo..=hi].iter().map(|label| kit.cols(label)).sum();
        let mut total = chips + gap * (hi - lo);
        if lo > 0 {
            total += kit.cols(PLUGINS_STRIP_MORE_LEFT) + 1;
        }
        if hi + 1 < labels.len() {
            total += 1 + kit.cols(PLUGINS_STRIP_MORE_RIGHT);
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
        .fg(kit.color(KitColor::ChipFg))
        .bg(kit.color(KitColor::ChipBg));
    let idle = Style::new().fg(kit.color(KitColor::ChipIdle));
    let dim = Style::new().fg(kit.color(KitColor::Meta));
    let mut spans = vec![Span::raw(kit.indent())];
    if lo > 0 {
        spans.push(Span::styled(format!("{PLUGINS_STRIP_MORE_LEFT} "), dim));
    }
    for (i, label) in labels.iter().enumerate().take(hi + 1).skip(lo) {
        if i > lo {
            spans.push(Span::raw(kit.chip_gap()));
        }
        spans.push(Span::styled(
            label.clone(),
            if i == current { lit } else { idle },
        ));
    }
    if hi + 1 < labels.len() {
        spans.push(Span::styled(format!(" {PLUGINS_STRIP_MORE_RIGHT}"), dim));
    }
    kit.clamp(spans, usize::from(width))
}

/// A marketplace's counts in one line: how many plugins it lists, how many
/// are installed, how long ago it was read.
fn market_counts(kit: &dyn PageKit, market: &MarketplaceRow, now: i64) -> String {
    let mut parts = vec![
        counted(market.plugins, "plugin"),
        format!("{} installed", market.installed),
    ];
    if let Some(at) = market.updated_at.filter(|_| now > 0) {
        parts.push(format!("updated {}", kit.age(now - at)));
    }
    parts.join(SEP)
}

/// Where a marketplace comes from in one line: its source, and the commit
/// its checkout is at.
fn market_source(market: &MarketplaceRow) -> String {
    match &market.revision {
        Some(revision) => format!(
            "{}{SEP}{}",
            market.source,
            revision.chars().take(7).collect::<String>()
        ),
        None => market.source.clone(),
    }
}

/// The current tab's dim subtitle and its count line — and, for a
/// marketplace, why its catalog could not be read (red) or what it skipped
/// (amber).
fn tab_header(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> Vec<Line<'static>> {
    let dim = kit.color(KitColor::Meta);
    let row = |text: &str, color: Color| kit.placeholder_row(&flat(text), color, width);
    let snapshot = &menu.snapshot;
    match &menu.tab {
        PluginsTab::All => {
            let count = match snapshot.marketplaces.len() {
                0 => "no marketplace added — add one under + Add".to_string(),
                n => format!(
                    "{} installed{SEP}{} in {}",
                    snapshot.installed_count(),
                    counted(snapshot.plugins.len(), "plugin"),
                    counted(n, "marketplace")
                ),
            };
            vec![row(PLUGINS_ALL_SUBTITLE, dim), row(&count, dim)]
        }
        PluginsTab::Installed => {
            let on = snapshot.plugins.iter().filter(|p| p.is_enabled()).count();
            let count = format!(
                "{} installed{SEP}{on} on in this directory",
                snapshot.installed_count()
            );
            vec![row(PLUGINS_INSTALLED_SUBTITLE, dim), row(&count, dim)]
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
                    kit.color(KitColor::Error),
                )),
                None => lines.push(row(&market_counts(kit, market, menu.now), dim)),
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
                let mut note = kit.wrapped_rows(
                    &format!("{skipped} — {reason}"),
                    kit.color(KitColor::Warning),
                    width,
                );
                note.truncate(PLUGINS_MARKET_NOTE_MAX_ROWS);
                lines.extend(note);
            }
            lines
        }
        PluginsTab::Add => vec![row(PLUGINS_ADD_SUBTITLE, dim), row(PLUGINS_ADD_COUNT, dim)],
    }
}

/// The list's header: the title, the tab's subtitle and counts, the notes
/// about the whole page (plugins off for the session, a state file that
/// will not parse) — wrapped, since their tails are the fix — and the tab
/// strip under a gap.
fn header_lines(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![kit.title(PLUGINS_TITLE, width)];
    lines.extend(tab_header(kit, menu, width));
    if !menu.snapshot.session_enabled {
        lines.extend(kit.wrapped_rows(PLUGINS_SESSION_OFF, kit.color(KitColor::Error), width));
    }
    if let Some(error) = menu.snapshot.error.as_deref() {
        lines.extend(kit.wrapped_rows(error, kit.color(KitColor::Error), width));
    }
    lines.push(Line::default());
    lines.push(tab_strip(kit, menu, width));
    lines
}

/// A plugin's status in the list: its mark and the mark's colour, its word
/// and the word's colour. Only the `✔` is green and only the `✘` red; the
/// word keeps the `/settings` two-tone, light for a plugin that is in use.
fn plugin_status(
    kit: &dyn PageKit,
    plugin: PluginRef<'_>,
) -> (&'static str, Color, &'static str, Color) {
    let lit = kit.color(KitColor::ValueOn);
    let off = kit.color(KitColor::ValueOff);
    match plugin.installed() {
        Some(state) if state.error.is_some() => (
            PLUGINS_BROKEN_MARK,
            kit.color(KitColor::Fail),
            PLUGINS_BROKEN,
            lit,
        ),
        Some(state) if state.enabled => (
            PLUGINS_ENABLED_MARK,
            kit.color(KitColor::Ok),
            PLUGINS_ENABLED,
            lit,
        ),
        Some(_) => (
            PLUGINS_DISABLED_MARK,
            kit.color(KitColor::Meta),
            PLUGINS_DISABLED,
            off,
        ),
        None if plugin.unavailable().is_some() => (
            PLUGINS_UNAVAILABLE_MARK,
            kit.color(KitColor::Meta),
            PLUGINS_UNAVAILABLE,
            off,
        ),
        None => (
            PLUGINS_AVAILABLE_MARK,
            kit.color(KitColor::Meta),
            PLUGINS_AVAILABLE,
            off,
        ),
    }
}

/// A status's whole width: the mark, the space, the word.
fn status_cols(kit: &dyn PageKit, plugin: PluginRef<'_>) -> usize {
    let (mark, _, word, _) = plugin_status(kit, plugin);
    kit.cols(mark) + 1 + kit.cols(word)
}

/// The name each row shows: the plugin's own — or, where two listed rows
/// share one (the same plugin in two marketplaces), the whole
/// `name@marketplace`, so the two can be told apart.
fn row_names(rows: &[PluginRef<'_>]) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for plugin in rows {
        *seen.entry(plugin.name()).or_default() += 1;
    }
    rows.iter()
        .map(|plugin| {
            if seen[plugin.name()] > 1 {
                plugin.id().to_string()
            } else {
                plugin.name().to_string()
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
    /// [`PLUGINS_ROW_MARKET_TEXT_MIN`] beside it.
    market: Option<usize>,
}

/// The columns of the plugin `rows`, named `names`, on a `width`-column
/// page. The name column is the widest name, capped at two fifths of what
/// the row has past its status — the descriptions keep the larger share,
/// and a name past the cap is `…`-cut, whole in the facts under the list —
/// and never so wide that the status mark loses its seat.
fn row_columns(
    kit: &dyn PageKit,
    rows: &[PluginRef<'_>],
    names: &[String],
    width: u16,
) -> RowColumns {
    let width = usize::from(width);
    let gap = kit.value_gap();
    let status = rows.iter().map(|p| status_cols(kit, *p)).max().unwrap_or(0);
    let past_status = width.saturating_sub(kit.row_head_cols() + gap + status + gap);
    let cap = (past_status * 2 / 5)
        .clamp(PLUGINS_NAME_COL_MIN, PLUGINS_NAME_COL_MAX)
        .min(width.saturating_sub(kit.row_head_cols() + gap + 1))
        .max(1);
    let name = names
        .iter()
        .map(|n| kit.cols(n))
        .max()
        .unwrap_or(0)
        .min(cap);
    // One marketplace's plugins alone — its own tab, or *All* with one
    // marketplace added — need no column repeating its name.
    let mixed = rows.iter().any(|p| {
        rows.first()
            .is_some_and(|first| first.marketplace() != p.marketplace())
    });
    let market = mixed
        .then(|| {
            rows.iter()
                .map(|p| kit.cols(p.marketplace()))
                .max()
                .unwrap_or(0)
        })
        .filter(|market| {
            past_status.saturating_sub(name) >= market + gap + PLUGINS_ROW_MARKET_TEXT_MIN
        });
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
    kit: &dyn PageKit,
    plugin: PluginRef<'_>,
    name: &str,
    selected: bool,
    columns: RowColumns,
    width: u16,
) -> Line<'static> {
    let name = kit.ellipsize(name, columns.name.max(1));
    let pad = columns.name.saturating_sub(kit.cols(&name)) + kit.value_gap();
    let mut spans = kit.row_head(&name, selected);
    spans.push(Span::raw(" ".repeat(pad)));
    let (mark, mark_color, word, word_color) = plugin_status(kit, plugin);
    spans.push(Span::styled(mark, Style::new().fg(mark_color)));
    let used = kit.row_head_cols() + kit.cols(&name) + pad + kit.cols(mark);
    let mut room = usize::from(width).saturating_sub(used);
    if room < 2 {
        return Line::from(spans);
    }
    let word = kit.ellipsize(word, room - 1);
    room -= 1 + kit.cols(&word);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(word.clone(), Style::new().fg(word_color)));
    let dim = Style::new().fg(kit.color(KitColor::Meta));
    // The padding before the next thing shown, pushed only once it is.
    let mut gap = columns
        .status
        .saturating_sub(kit.cols(mark) + 1 + kit.cols(&word))
        + kit.value_gap();
    if let Some(market_width) = columns.market
        && room > gap
    {
        room -= gap;
        let market = kit.ellipsize(plugin.marketplace(), market_width.min(room).max(1));
        room -= kit.cols(&market);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(market.clone(), dim));
        gap = market_width.saturating_sub(kit.cols(&market)) + kit.value_gap();
    }
    let mut lead = "";
    if plugin.installed().is_some_and(|state| state.update) {
        let update = format!("{PLUGINS_UPDATE_MARK} {PLUGINS_UPDATE_WORD}");
        if room >= gap + kit.cols(&update) {
            room -= gap + kit.cols(&update);
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(
                update,
                Style::new().fg(kit.color(KitColor::Warning)),
            ));
            gap = 0;
            lead = SEP;
        }
    }
    if let Some(description) = plugin
        .description()
        .map(flat)
        .filter(|text| !text.is_empty())
    {
        let avail = room.saturating_sub(gap + kit.cols(lead));
        if room > gap + kit.cols(lead)
            && (kit.cols(&description) <= avail || avail >= PLUGINS_ROW_TEXT_MIN)
        {
            spans.push(Span::raw(" ".repeat(gap)));
            if !lead.is_empty() {
                spans.push(Span::styled(lead, dim));
            }
            spans.push(Span::styled(kit.ellipsize(&description, avail), dim));
        }
    }
    Line::from(spans)
}

/// The window of the plugin `rows` that keeps the selection in view —
/// `max` rows — as how many rows it hides above, its rows and how many it
/// hides below, every row on the columns of [`row_columns`].
fn plugin_window(
    kit: &dyn PageKit,
    rows: &[PluginRef<'_>],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = kit.list_window(rows.len(), selected, max);
    let names = row_names(rows);
    let columns = row_columns(kit, rows, &names, width);
    let lines = range
        .map(|i| plugin_row(kit, rows[i], &names[i], i == selected, columns, width))
        .collect();
    (above, lines, below)
}

/// The add tab's window: the typed source, then the official catalogs, then
/// — while nothing is typed — the row for any other source:
/// `→ {source}  {✔ added}  {what it is}`, the sources and the marks each in
/// their column.
fn add_window(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    rows: &[AddRow],
    selected: usize,
    max: usize,
    width: u16,
) -> (usize, Vec<Line<'static>>, usize) {
    let (above, range, below) = kit.list_window(rows.len(), selected, max);
    let source = |row: &AddRow| match row {
        AddRow::Typed(text) => text.clone(),
        AddRow::Suggestion(index) => menu
            .suggestion(*index)
            .map_or_else(String::new, |s| s.source.clone()),
        AddRow::Other => PLUGINS_ADD_ROW.to_string(),
    };
    let added_of = |row: &AddRow| match row {
        AddRow::Suggestion(index) => menu.suggestion(*index).is_some_and(|s| menu.is_added(s)),
        _ => false,
    };
    let added = format!("{PLUGINS_ENABLED_MARK} {PLUGINS_ADDED}");
    let cap = usize::from(width)
        .saturating_sub(kit.row_head_cols() + kit.value_gap())
        .clamp(1, 40);
    let name_width = rows
        .iter()
        .map(|row| kit.cols(&source(row)))
        .max()
        .unwrap_or(0)
        .min(cap);
    let any_added = rows.iter().any(added_of);
    let lines = range
        .map(|i| {
            let row = &rows[i];
            let name = kit.ellipsize(&source(row), name_width.max(1));
            let pad = name_width.saturating_sub(kit.cols(&name)) + kit.value_gap();
            let mut spans = kit.row_head(&name, i == selected);
            spans.push(Span::raw(" ".repeat(pad)));
            let mut used = kit.row_head_cols() + kit.cols(&name) + pad;
            let description = match row {
                AddRow::Typed(_) => PLUGINS_ADD_TYPED.to_string(),
                AddRow::Other => PLUGINS_ADD_ROW_DESCRIPTION.to_string(),
                AddRow::Suggestion(index) => menu
                    .suggestion(*index)
                    .map_or_else(String::new, |s| s.description.clone()),
            };
            if any_added {
                if added_of(row) {
                    spans.push(Span::styled(
                        PLUGINS_ENABLED_MARK,
                        Style::new().fg(kit.color(KitColor::Ok)),
                    ));
                    spans.push(Span::styled(
                        format!(" {PLUGINS_ADDED}"),
                        Style::new().fg(kit.color(KitColor::ValueOn)),
                    ));
                } else {
                    spans.push(Span::raw(" ".repeat(kit.cols(&added))));
                }
                spans.push(Span::raw(" ".repeat(kit.value_gap())));
                used += kit.cols(&added) + kit.value_gap();
            }
            let room = usize::from(width).saturating_sub(used);
            if room > 0 {
                spans.push(Span::styled(
                    kit.ellipsize(&description, room),
                    Style::new().fg(kit.color(KitColor::Meta)),
                ));
            }
            // A row never ends on a column's padding.
            while spans.last().is_some_and(|s| s.content.trim().is_empty()) {
                spans.pop();
            }
            Line::from(spans)
        })
        .collect();
    (above, lines, below)
}

/// A plugin's facts in one line: its id, version and what it is made of —
/// or, for one that cannot be installed, why.
fn plugin_facts(plugin: PluginRef<'_>) -> String {
    let mut parts = vec![plugin.id().to_string()];
    if let Some(version) = plugin.shown_version() {
        parts.push(version_label(version));
    }
    match (plugin.unavailable(), plugin.summary()) {
        (Some(reason), _) if plugin.installed().is_none() => parts.push(flat(reason)),
        (_, Some(summary)) => parts.push(summary.line()),
        _ => {}
    }
    parts.join(SEP)
}

/// What the list says under itself about the highlighted row, in the same
/// number of rows at every selection — so ↑/↓ never resize the page: the
/// description cut to one row, when `describe` (`No description` for a
/// plugin with none, rather than giving the row back), over the facts. On
/// the add tab, what an official catalog is and where it comes from; for
/// the typed source and the row for any other, the forms a source takes and
/// how to pin one.
fn detail_lines(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    selected: usize,
    describe: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let (description, facts) = if menu.tab == PluginsTab::Add {
        match menu.add_rows().get(selected) {
            Some(AddRow::Suggestion(index)) => {
                let Some(suggestion) = menu.suggestion(*index) else {
                    return Vec::new();
                };
                (
                    suggestion.description.clone(),
                    format!("{}{SEP}adds the {} tab", suggestion.source, suggestion.name),
                )
            }
            Some(AddRow::Typed(_) | AddRow::Other) => (
                format!("Use {PLUGINS_SOURCE_FORMS}"),
                PLUGINS_ADD_PIN.to_string(),
            ),
            None => return Vec::new(),
        }
    } else {
        match menu.plugin_rows().get(selected) {
            Some(plugin) => (
                plugin
                    .description()
                    .map(flat)
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| PLUGINS_NO_DESCRIPTION.to_string()),
                plugin_facts(*plugin),
            ),
            None => return Vec::new(),
        }
    };
    let mut lines = Vec::new();
    if describe {
        lines.extend(vec![kit.description_row(&description, width)]);
    }
    lines.extend(vec![kit.description_row(&facts, width)]);
    lines
}

/// What the list shows where it has no row: a search that matched nothing,
/// an empty tab naming where to go.
fn placeholder_lines(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> Vec<Line<'static>> {
    let dim = kit.color(KitColor::Meta);
    if !menu.query.trim().is_empty() {
        // Nothing here, something elsewhere: say where, so ←/→ is the
        // obvious next key — never "there is no such plugin".
        let elsewhere = match menu.tab {
            PluginsTab::All | PluginsTab::Add => 0,
            _ => menu.matches_in_all(),
        };
        let text = if elsewhere > 0 {
            format!("{PLUGINS_NO_MATCH} here{SEP_DASH}{elsewhere} in {PLUGINS_TAB_ALL}")
        } else {
            PLUGINS_NO_MATCH.to_string()
        };
        vec![kit.placeholder_row(&text, dim, width)]
    } else if menu.tab == PluginsTab::Installed {
        kit.wrapped_rows(PLUGINS_NONE_INSTALLED, dim, width)
    } else {
        kit.wrapped_rows(PLUGINS_NONE_LISTED, dim, width)
    }
}

/// The list page, the first of its pages that fits `height` rows
/// ([`first_page_that_fits`]) — the picker ladder:
///
/// 1. the **whole** page: the header (title, the tab's subtitle and counts,
///    notes, tabs), the `❯` search line, a window of ten rows (the
///    `/theme` picker's), the `(n/total)` counter, the highlighted row's
///    description and its facts — a row each ([`detail_lines`]) — and the
///    status row;
/// 2. the same with a window of five rows;
/// 3. the five-row page with the description gone, its facts kept;
/// 4. the **compact** page with the header kept: three rows between the
///    reserved `↑`/`↓` slots ([`compact_page_with_header`]);
/// 5. the compact page under the **tab strip** alone — the tabs outlast
///    the title, since ←/→ still move between them;
/// 6. the header-less compact page and its rungs ([`compact_page`]), down
///    to the search line alone.
///
/// **One height while scrolling or typing.** A window holds as many rows
/// as the tab lists with no search, up to its size ([`PluginsPage::tab_total`]),
/// padded when the search matches fewer; a search that matches nothing
/// says so inside that same window, the counter's and the detail's rows
/// kept blank. So which rung fits never depends on the selection or the
/// search, and the page never changes height under a keystroke. The name
/// column is sized off the whole filtered list, not the window.
///
/// The status row is always one row in the hint's seat (`status_row`):
/// what runs and what happened never move the page.
fn list_page(kit: &dyn PageKit, menu: &PluginsPage, width: u16, height: u16) -> Vec<Line<'static>> {
    let header = header_lines(kit, menu, width);
    let tabs = [tab_strip(kit, menu, width)];
    let search = kit.search_line(&menu.query);
    let hint = list_hint(kit, menu, width);
    let tail = [status_row(kit, menu, hint, width)];
    let compact_tail = tail.clone();
    let page = |mut body: Vec<Line<'static>>| {
        body.extend(tail.iter().cloned());
        kit.picker_page(&header, &search, body, width)
    };

    let len = menu.row_count();
    let total = menu.tab_total();
    if total == 0 {
        // The tab lists nothing at all (nothing installed, nothing added):
        // its placeholder, which no keystroke on this tab changes.
        let block = placeholder_lines(kit, menu, width);
        let window = |max| kit.placeholder_window(&block, max);
        return kit.first_page_that_fits(
            height,
            &[
                &|| {
                    let mut body = vec![Line::default()];
                    body.extend(block.iter().cloned());
                    body.push(Line::default());
                    page(body)
                },
                &|| kit.compact_page_with_header(&tabs, &search, &window, &compact_tail, width),
                &|| kit.compact_page(height, &search, &window, &compact_tail, width),
            ],
        );
    }
    let selected = menu.selected.min(len.saturating_sub(1));
    let plugins = menu.plugin_rows();
    let adds = menu.add_rows();
    let placeholder = (len == 0).then(|| placeholder_lines(kit, menu, width));
    let window = |max: usize| {
        if let Some(block) = &placeholder {
            kit.placeholder_window(block, max)
        } else if menu.tab == PluginsTab::Add {
            add_window(kit, menu, &adds, selected, max, width)
        } else {
            plugin_window(kit, &plugins, selected, max, width)
        }
    };
    let whole = |size: usize, describe: bool| {
        let slots = size.min(total);
        let mut body = vec![Line::default()];
        let (_, mut rows, _) = match &placeholder {
            Some(block) => (0, block.iter().take(slots).cloned().collect(), 0),
            None => window(slots),
        };
        rows.resize(slots, Line::default());
        body.extend(rows);
        if placeholder.is_some() {
            // The counter's row and the detail's, kept, blank.
            body.push(Line::default());
            body.push(Line::default());
            body.extend((0..1 + usize::from(describe)).map(|_| Line::default()));
            body.push(Line::default());
            return body;
        }
        body.push(kit.counter(selected, len));
        body.push(Line::default());
        let detail = detail_lines(kit, menu, selected, describe, width);
        if !detail.is_empty() {
            body.extend(detail);
            body.push(Line::default());
        }
        body
    };
    let tall = kit.tall_list_rows();
    let short = kit.list_rows();
    kit.first_page_that_fits(
        height,
        &[
            &|| page(whole(tall, true)),
            &|| page(whole(short, true)),
            &|| page(whole(short, false)),
            &|| kit.compact_page_with_header(&header, &search, &window, &compact_tail, width),
            &|| kit.compact_page_with_header(&tabs, &search, &window, &compact_tail, width),
            &|| kit.compact_page(height, &search, &window, &compact_tail, width),
        ],
    )
}

// ===== the detail pages =====

/// `blocks` framed: the top rule, the non-empty blocks one blank row apart,
/// the bottom rule — no blank row hugging either rule (`docs/view-flow.md`
/// *The frame hugs its content*) — with the caret of the block that has it.
fn framed(kit: &dyn PageKit, blocks: Vec<Block>, width: u16) -> PageBuild {
    let mut lines = vec![kit.rule(width)];
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
    lines.push(kit.rule(width));
    PageBuild { lines, cursor }
}

/// A page's closing block: the status row — the job, the failure or the
/// key hint, in one seat, so acting on the page never moves it. The hint is
/// its `clauses`, fitted to the width.
fn closing_block(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    clauses: &[(&str, u8)],
    width: u16,
) -> Block {
    let hint = fitted_hint(kit, clauses, width);
    (vec![status_row(kit, menu, hint, width)], None)
}

/// The numbered action rows (`❯ 1. Install`) — `/mcp`'s: the accent `❯` and
/// label on the highlighted one, the others muted.
fn action_rows(
    kit: &dyn PageKit,
    labels: &[&str],
    selected: usize,
    width: u16,
) -> Vec<Line<'static>> {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let on = i == selected;
            let marker = if on {
                Span::styled(
                    kit.action_marker(),
                    Style::new().fg(kit.color(KitColor::Accent)),
                )
            } else {
                Span::raw(" ".repeat(kit.cols(kit.action_marker())))
            };
            let style = Style::new().fg(if on {
                kit.color(KitColor::Accent)
            } else {
                kit.color(KitColor::Muted)
            });
            let number = format!("{}. ", i + 1);
            let room = usize::from(width)
                .saturating_sub(
                    kit.cols(kit.indent()) + kit.cols(kit.action_marker()) + kit.cols(&number),
                )
                .max(1);
            Line::from(vec![
                Span::raw(kit.indent()),
                marker,
                Span::styled(number, style),
                Span::styled(kit.ellipsize(label, room), style),
            ])
        })
        .collect()
}

/// The state that leads the line under a plugin's name: the mark in its
/// state's colour over what the state means here.
fn plugin_state(kit: &dyn PageKit, plugin: PluginRef<'_>) -> (&'static str, Color, &'static str) {
    match plugin.installed() {
        Some(state) if state.error.is_some() => (
            PLUGINS_BROKEN_MARK,
            kit.color(KitColor::Fail),
            "installed, but not loaded",
        ),
        Some(state) if state.enabled => (
            PLUGINS_ENABLED_MARK,
            kit.color(KitColor::Ok),
            "enabled in this directory",
        ),
        Some(_) => (
            PLUGINS_DISABLED_MARK,
            kit.color(KitColor::Meta),
            "disabled in this directory",
        ),
        None if plugin.unavailable().is_some() => (
            PLUGINS_UNAVAILABLE_MARK,
            kit.color(KitColor::Meta),
            "cannot be installed",
        ),
        None => (
            PLUGINS_AVAILABLE_MARK,
            kit.color(KitColor::Meta),
            "not installed",
        ),
    }
}

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
    /// A value in one style, then a dim clause after it (`· reviewed before
    /// they run`), wrapped as one.
    Noted(&'static str, String, Style, &'static str),
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
            | Self::Noted(label, ..)
            | Self::State(label, ..) => label,
        }
    }

    /// The fact's rows, its value at column `col` past the inset.
    fn lines(self, kit: &dyn PageKit, col: usize, width: u16) -> Vec<Line<'static>> {
        let room = field_room(kit, col, width);
        match self {
            Self::Line(label, value, style) => field_rows(
                kit,
                col,
                label,
                vec![vec![Span::styled(
                    kit.ellipsize(&value, usize::from(room)),
                    style,
                )]],
            ),
            Self::Wrapped(label, value, style) => {
                field_rows(kit, col, label, kit.wrap_spans(&[(value, style)], room))
            }
            Self::Linked(label, value, style) => field_rows(
                kit,
                col,
                label,
                kit.wrap_spans(&kit.linked_segments(&value, style), room),
            ),
            Self::Url(label, value, style) => match kit.url_run(&value, style) {
                Some(run) => field_rows(kit, col, label, kit.wrap_spans(&[run], room)),
                None => field_rows(kit, col, label, kit.wrap_spans(&[(value, style)], room)),
            },
            Self::Noted(label, value, style, note) => {
                // The note rides the value's last row while it fits whole
                // there, else it takes a row of its own — never a `·` left
                // dangling at a row's end.
                let dim = Style::new().fg(kit.color(KitColor::Meta));
                let mut rows = kit.wrap_spans(&[(value, style)], room);
                let tail = format!("{SEP}{note}");
                let last_cols = rows
                    .last()
                    .map_or(0, |row| row.iter().map(|s| kit.cols(&s.content)).sum());
                match rows.last_mut() {
                    Some(last) if last_cols + kit.cols(&tail) <= usize::from(room) => {
                        last.push(Span::styled(tail, dim));
                    }
                    _ => rows.extend(kit.wrap_spans(&[(note.to_string(), dim)], room)),
                }
                field_rows(kit, col, label, rows)
            }
            Self::State(label, mark, color, words) => {
                let words_room = usize::from(room).saturating_sub(kit.cols(mark) + 1).max(1);
                field_rows(
                    kit,
                    col,
                    label,
                    vec![vec![
                        Span::styled(format!("{mark} "), Style::new().fg(color)),
                        Span::styled(
                            kit.ellipsize(&words, words_room),
                            Style::new().fg(kit.color(KitColor::State)),
                        ),
                    ]],
                )
            }
        }
    }
}

/// The room a fact's value wraps to: what is left of the row past the inset
/// and the label column `col`.
fn field_room(kit: &dyn PageKit, col: usize, width: u16) -> u16 {
    u16::try_from(
        usize::from(width)
            .saturating_sub(kit.cols(kit.indent()) + col)
            .max(1),
    )
    .unwrap_or(u16::MAX)
}

/// A fact's already-wrapped value rows laid out under its label: the first
/// row beside the label, the rest hanging at the field column `col`.
fn field_rows(
    kit: &dyn PageKit,
    col: usize,
    label: &str,
    rows: Vec<Vec<Span<'static>>>,
) -> Vec<Line<'static>> {
    let mut rows = rows.into_iter();
    let mut first = vec![
        Span::raw(kit.indent()),
        Span::styled(
            format!("{label:<col$}"),
            Style::new().fg(kit.color(KitColor::Label)),
        ),
    ];
    first.extend(rows.next().unwrap_or_default());
    let mut lines = vec![Line::from(first)];
    lines.extend(rows.map(|row| {
        let mut spans = vec![Span::raw(kit.indent()), Span::raw(" ".repeat(col))];
        spans.extend(row);
        Line::from(spans)
    }));
    lines
}

/// `clauses` laid out ` · `-separated in rows of `width`, each row inset:
/// a clause that does not fit after the one before it starts the next row
/// whole — never `… · by` over `42Crunch` — and only a clause wider than a
/// row is broken across rows.
fn clause_lines(
    kit: &dyn PageKit,
    clauses: Vec<Vec<Span<'static>>>,
    width: u16,
) -> Vec<Line<'static>> {
    let room = usize::from(width)
        .saturating_sub(kit.cols(kit.indent()))
        .max(1);
    let dim = Style::new().fg(kit.color(KitColor::Meta));
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for clause in clauses {
        let size: usize = clause.iter().map(|span| kit.cols(&span.content)).sum();
        if used > 0 && used + kit.cols(SEP) + size <= room {
            row.push(Span::styled(SEP, dim));
            row.extend(clause);
            used += kit.cols(SEP) + size;
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
            let mut pieces = kit.wrap_spans(&segments, u16::try_from(room).unwrap_or(u16::MAX));
            row = pieces.pop().unwrap_or_default();
            used = row.iter().map(|span| kit.cols(&span.content)).sum();
            rows.extend(pieces);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.into_iter()
        .map(|row| {
            let mut spans = vec![Span::raw(kit.indent())];
            spans.extend(row);
            Line::from(spans)
        })
        .collect()
}

/// The `Contents:` facts of a plugin's page: each kind of component by the
/// names a session knows it by — hooks and MCP servers saying they are
/// reviewed before they run — then what it carries that is not used here;
/// or one `Contents:` row while it is read, for one read only once it is
/// installed, for a read that failed, for one with nothing usable here.
fn contents_facts(kit: &dyn PageKit, plugin: PluginRef<'_>) -> Vec<Fact> {
    let value = Style::new().fg(kit.color(KitColor::Value));
    let names = Style::new().fg(kit.color(KitColor::State));
    let summary = match plugin.contents() {
        PluginContents::NotRead => {
            let reading = PLUGINS_CONTENTS_READING.to_string();
            return vec![Fact::Line("Contents:", reading, value)];
        }
        PluginContents::Unknown => {
            let unknown = PLUGINS_CONTENTS_UNKNOWN.to_string();
            return vec![Fact::Line("Contents:", unknown, value)];
        }
        PluginContents::Unreadable(error) => {
            let text = format!("{PLUGINS_CONTENTS_UNREADABLE} — {}", flat(error));
            return vec![Fact::Wrapped(
                "Contents:",
                text,
                Style::new().fg(kit.color(KitColor::Error)),
            )];
        }
        PluginContents::Known(summary) => summary,
    };
    let mut facts = Vec::new();
    for (label, list) in [
        ("Skills:", &summary.skills),
        ("Commands:", &summary.commands),
        ("Agents:", &summary.agents),
    ] {
        if !list.is_empty() {
            facts.push(Fact::Wrapped(label, list.join(", "), names));
        }
    }
    if summary.hooks_present() {
        let hooks = if summary.hooks.is_empty() {
            PLUGINS_CONTENTS_PRESENT.to_string()
        } else {
            summary.hooks.join(", ")
        };
        facts.push(Fact::Noted("Hooks:", hooks, names, PLUGINS_REVIEWED));
    }
    if summary.servers_present() {
        let servers = if summary.servers.is_empty() {
            PLUGINS_CONTENTS_PRESENT.to_string()
        } else {
            summary.servers.join(", ")
        };
        facts.push(Fact::Noted(
            "MCP servers:",
            servers,
            names,
            PLUGINS_REVIEWED,
        ));
    }
    if facts.is_empty() {
        facts.push(Fact::Line("Contents:", summary.line(), value));
    }
    if !summary.unused.is_empty() {
        facts.push(Fact::Wrapped(
            "Not used here:",
            summary.unused.join(", "),
            value,
        ));
    }
    facts
}

/// A plugin's page: its name, then its state leading the line under it —
/// beside its id, version and author, broken between those clauses — and
/// its description, its addresses links; the facts on a label column of
/// their own: an update, format, source, where it loads from, homepage,
/// category and each kind of component by name; why it cannot be installed
/// or was not loaded, the trust warning for one not installed that runs or
/// may run something, what reading it said; then the numbered actions its
/// state affords.
fn plugin_page(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    plugin: PluginRef<'_>,
    width: u16,
) -> PageBuild {
    let value = Style::new().fg(kit.color(KitColor::Value));
    let names = Style::new().fg(kit.color(KitColor::State));
    let dim = Style::new().fg(kit.color(KitColor::Meta));

    let mut head = vec![kit.title(plugin.title(), width)];
    let (mark, mark_color, state) = plugin_state(kit, plugin);
    let mut clauses = vec![
        vec![
            Span::styled(mark, Style::new().fg(mark_color)),
            Span::raw(" "),
            Span::styled(state, names),
        ],
        vec![Span::styled(plugin.id().to_string(), dim)],
    ];
    if let Some(version) = plugin.shown_version() {
        clauses.push(vec![Span::styled(version_label(version), dim)]);
    }
    if let Some(author) = plugin.author() {
        clauses.push(vec![Span::styled(format!("by {author}"), dim)]);
    }
    head.extend(clause_lines(kit, clauses, width));
    if let Some(description) = plugin.description().filter(|d| !d.trim().is_empty()) {
        head.push(Line::default());
        head.extend(kit.linked_rows(description, kit.color(KitColor::Description), width));
    }

    let mut facts = Vec::new();
    if plugin.installed().is_some_and(|state| state.update) {
        let offered = plugin.version().map_or_else(
            || "a newer copy is available".to_string(),
            |version| format!("{} is available", version_label(version)),
        );
        facts.push(Fact::State(
            "Update:",
            PLUGINS_UPDATE_MARK,
            kit.color(KitColor::Warning),
            offered,
        ));
    }
    if let Some(format) = plugin.format() {
        facts.push(Fact::Line("Format:", format.to_string(), value));
    }
    facts.push(Fact::Linked("Source:", plugin.source().to_string(), value));
    // Loaded in place: its folder in its marketplace is the plugin.
    if let Some(folder) = plugin
        .installed()
        .filter(|state| state.in_place)
        .and_then(|state| state.loads_from.clone())
    {
        facts.push(Fact::Wrapped("Loads from:", folder, value));
    }
    if let Some(homepage) = plugin.homepage() {
        facts.push(Fact::Url("Homepage:", homepage.to_string(), value));
    }
    if let Some(category) = plugin.category() {
        facts.push(Fact::Line("Category:", category.to_string(), value));
    }
    facts.extend(contents_facts(kit, plugin));
    let col = facts
        .iter()
        .map(|fact| kit.cols(fact.label()))
        .max()
        .unwrap_or(0)
        + PLUGINS_FACT_LABEL_GAP;
    let facts: Vec<Line<'static>> = facts
        .into_iter()
        .flat_map(|fact| fact.lines(kit, col, width))
        .collect();

    let mut notes = Vec::new();
    if let Some(error) = plugin.installed().and_then(|s| s.error.as_deref()) {
        notes.extend(kit.wrapped_rows(error, kit.color(KitColor::Error), width));
    }
    if let Some(reason) = plugin.unavailable() {
        notes.extend(kit.wrapped_rows(&capitalized(reason), kit.color(KitColor::Meta), width));
    }
    // Not installed, so not yet reviewed: one that runs something — or whose
    // contents are read only once it is installed, or could not be read —
    // warns before Install.
    let warns = match plugin.contents() {
        PluginContents::Known(summary) => summary.runs_code(),
        PluginContents::Unknown | PluginContents::Unreadable(_) => true,
        PluginContents::NotRead => false,
    };
    if plugin.installed().is_none() && plugin.unavailable().is_none() && warns {
        notes.extend(kit.wrapped_rows(PLUGINS_TRUST_NOTE, kit.color(KitColor::Meta), width));
    }
    for note in plugin.notes() {
        notes.extend(kit.wrapped_rows(note, kit.color(KitColor::Meta), width));
    }

    let labels: Vec<&'static str> = plugin_actions(plugin)
        .into_iter()
        .map(PluginAction::label)
        .collect();
    let status = closing_block(kit, menu, &PLUGINS_PAGE_HINT, width);
    framed(
        kit,
        vec![
            (head, None),
            (facts, None),
            (notes, None),
            (action_rows(kit, &labels, menu.selected, width), None),
            status,
        ],
        width,
    )
}

/// The add page: what a marketplace is, the one field — its dim
/// placeholder until something is typed, then the text wrapped with the
/// caret in it — the forms a source takes and how to pin one, the failure
/// and the status row.
fn add_page(kit: &dyn PageKit, menu: &PluginsPage, width: u16) -> PageBuild {
    let dim = kit.color(KitColor::Meta);
    let mut head = vec![kit.title(PLUGINS_ADD_TITLE, width)];
    head.extend(kit.wrapped_rows(PLUGINS_ADD_BLURB, dim, width));
    let lead = kit.cols(kit.indent()) + kit.cols(kit.prompt());
    let room = usize::from(width).saturating_sub(lead).max(1);
    let prompt = Span::styled(kit.prompt(), Style::new().fg(kit.color(KitColor::Accent)));
    let (field, caret) = if menu.input.is_empty() {
        (
            vec![Line::from(vec![
                Span::raw(kit.indent()),
                prompt,
                Span::styled(
                    kit.ellipsize(PLUGINS_ADD_PLACEHOLDER, room),
                    Style::new().fg(dim),
                ),
            ])],
            (lead, 0),
        )
    } else {
        let wrap = kit.field_width(u16::try_from(room).unwrap_or(u16::MAX));
        let (texts, (row, col)) = menu.input.rows(usize::from(wrap));
        let rows = texts
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let lead_span = if i == 0 {
                    prompt.clone()
                } else {
                    Span::raw(" ".repeat(kit.cols(kit.prompt())))
                };
                Line::from(vec![
                    Span::raw(kit.indent()),
                    lead_span,
                    Span::styled(text, Style::new().fg(kit.color(KitColor::Muted))),
                ])
            })
            .collect();
        (rows, (lead + col, row))
    };
    let mut forms = kit.wrapped_rows(&format!("Use {PLUGINS_SOURCE_FORMS}."), dim, width);
    forms.extend(kit.wrapped_rows(PLUGINS_ADD_PIN, dim, width));
    let status = closing_block(kit, menu, &PLUGINS_ADD_HINT, width);
    framed(
        kit,
        vec![(head, None), (field, Some(caret)), (forms, None), status],
        width,
    )
}

/// The question before something that cannot be undone — the answer the
/// user just chose highlighted, so Enter confirms it and Esc goes back.
fn confirm_page(
    kit: &dyn PageKit,
    menu: &PluginsPage,
    confirm: &PluginsConfirm,
    width: u16,
) -> PageBuild {
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
    let mut head = vec![kit.title(&title, width)];
    head.extend(kit.wrapped_rows(&text, kit.color(KitColor::Meta), width));
    let status = closing_block(kit, menu, &PLUGINS_CONFIRM_HINT, width);
    framed(
        kit,
        vec![
            (head, None),
            (
                action_rows(kit, &[yes, "Cancel"], menu.selected, width),
                None,
            ),
            status,
        ],
        width,
    )
}

// ===== the whole =====

impl PluginsPage {
    /// The page as lines and its caret (see [`PageBuild`]): the list's first
    /// page that fits `height` rows, or a detail page — those bottom-anchor
    /// when taller than the terminal, their actions and status row staying
    /// on screen. `height` is the terminal's, or the region already
    /// reserved for the page, which builds the same page. A plugin's page
    /// whose plugin has just gone shows the list, as the page will once the
    /// answer that removed it is taken.
    #[must_use]
    pub fn build(&self, kit: &dyn PageKit, width: u16, height: u16) -> PageBuild {
        let list = || {
            let lines = list_page(kit, self, width, height);
            let cursor = kit.prompt_row(&lines).map(|row| {
                let col = kit.cols(kit.indent()) + kit.cols(kit.prompt()) + kit.cols(&self.query);
                (u16::try_from(col).unwrap_or(u16::MAX), usize::from(row))
            });
            PageBuild { lines, cursor }
        };
        match &self.screen {
            PluginsScreen::List => list(),
            PluginsScreen::Add => add_page(kit, self, width),
            PluginsScreen::Confirm(confirm) => confirm_page(kit, self, confirm, width),
            PluginsScreen::Plugin(id) => match self.plugin(id) {
                Some(plugin) => plugin_page(kit, self, plugin, width),
                None => list(),
            },
        }
    }

    /// The page's lines alone ([`build`](Self::build)'s).
    #[must_use]
    pub fn lines(&self, kit: &dyn PageKit, width: u16, height: u16) -> Vec<Line<'static>> {
        self.build(kit, width, height).lines
    }
}

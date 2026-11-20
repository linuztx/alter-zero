//! The snapshot's plugin rows, packed (`docs/plugins-menu.md` *The
//! snapshot's shape*): one text buffer every row's strings are appended to,
//! and one `Vec` of fixed-size rows holding spans into it — so a snapshot of
//! four hundred plugins is a handful of allocations, not thousands. glibc
//! keeps what a heap of small interleaved allocations once held (the rule of
//! `docs/memory.md`); a few large ones it can hand back.
//!
//! The owned [`PluginRow`] stays the **input** form: a host builds a list by
//! pushing rows, each packed and dropped. The page reads a row through a
//! [`PluginRef`] — a `Copy` view over the list and an index, with methods
//! named after the owned row's fields. What a row owns beyond text — an
//! installed plugin's state, the contents read for its page, notes — sits
//! in one optional box, absent for the plain listed row.

use std::fmt;

use super::state::{ComponentSummary, InstalledState, PluginContents, PluginRow};
use crate::state::PluginId;

/// A piece of the text buffer; [`Span::NONE`] for a field that is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    at: u32,
    len: u32,
}

impl Span {
    const NONE: Self = Self {
        at: u32::MAX,
        len: 0,
    };
}

/// What a row owns beyond its text — few rows have any of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Extra {
    installed: Option<InstalledState>,
    contents: PluginContents,
    notes: Vec<String>,
}

impl Extra {
    fn is_empty(&self) -> bool {
        self.installed.is_none()
            && self.contents == PluginContents::NotRead
            && self.notes.is_empty()
    }
}

/// One row: spans into the text, and the box of what it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Packed {
    name: Span,
    marketplace: Span,
    display_name: Span,
    description: Span,
    version: Span,
    author: Span,
    category: Span,
    homepage: Span,
    /// Every keyword, `\n`-separated — they are only searched.
    keywords: Span,
    source: Span,
    unavailable: Span,
    format: Span,
    extra: Option<Box<Extra>>,
}

/// A row's contents when nothing has been read for it.
static NOT_READ: PluginContents = PluginContents::NotRead;

/// Every plugin a snapshot lists, packed.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PluginList {
    text: String,
    rows: Vec<Packed>,
}

/// How far a list had been built — what [`PluginList::truncate`] goes back
/// to when a catalog read part way turns out not to be valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListMark {
    rows: usize,
    text: usize,
}

impl PluginList {
    /// An empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            rows: Vec::new(),
        }
    }

    /// How many plugins it lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Does it list none?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn put(&mut self, text: &str) -> Span {
        let at = u32::try_from(self.text.len()).unwrap_or(u32::MAX - 1);
        self.text.push_str(text);
        Span {
            at,
            len: u32::try_from(text.len()).unwrap_or(0),
        }
    }

    fn put_opt(&mut self, text: Option<&str>) -> Span {
        text.map_or(Span::NONE, |text| self.put(text))
    }

    /// Add `row`, packed: its strings appended to the text, its owned parts
    /// boxed only when it has any.
    pub fn push(&mut self, row: PluginRow) {
        let PluginRow {
            id,
            display_name,
            description,
            version,
            author,
            category,
            homepage,
            keywords,
            source,
            unavailable,
            format,
            contents,
            installed,
            notes,
        } = row;
        let keywords_at = self.text.len();
        for (i, keyword) in keywords.iter().enumerate() {
            if i > 0 {
                self.text.push('\n');
            }
            self.text.push_str(keyword);
        }
        let keywords = Span {
            at: u32::try_from(keywords_at).unwrap_or(u32::MAX - 1),
            len: u32::try_from(self.text.len() - keywords_at).unwrap_or(0),
        };
        let extra = Extra {
            installed,
            contents,
            notes,
        };
        let packed = Packed {
            name: self.put(&id.name),
            marketplace: self.put(&id.marketplace),
            display_name: self.put_opt(display_name.as_deref()),
            description: self.put_opt(description.as_deref()),
            version: self.put_opt(version.as_deref()),
            author: self.put_opt(author.as_deref()),
            category: self.put_opt(category.as_deref()),
            homepage: self.put_opt(homepage.as_deref()),
            keywords,
            source: self.put(&source),
            unavailable: self.put_opt(unavailable.as_deref()),
            format: self.put_opt(format.as_deref()),
            extra: (!extra.is_empty()).then(|| Box::new(extra)),
        };
        self.rows.push(packed);
    }

    /// Where the list stands now.
    #[must_use]
    pub fn mark(&self) -> ListMark {
        ListMark {
            rows: self.rows.len(),
            text: self.text.len(),
        }
    }

    /// Drop every row pushed since `mark`.
    pub fn truncate(&mut self, mark: ListMark) {
        self.rows.truncate(mark.rows);
        self.text.truncate(mark.text);
    }

    /// The row at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<PluginRef<'_>> {
        (index < self.rows.len()).then_some(PluginRef { list: self, index })
    }

    /// Every row, in the order pushed.
    pub fn iter(&self) -> impl Iterator<Item = PluginRef<'_>> + '_ {
        (0..self.rows.len()).map(move |index| PluginRef { list: self, index })
    }

    /// Where the plugin `id` is.
    #[must_use]
    pub fn position(&self, id: &PluginId) -> Option<usize> {
        self.iter().position(|row| row.is(id))
    }

    /// The plugin `id`.
    #[must_use]
    pub fn find(&self, id: &PluginId) -> Option<PluginRef<'_>> {
        self.position(id).and_then(|index| self.get(index))
    }

    fn extra_mut(&mut self, index: usize) -> Option<&mut Extra> {
        let row = self.rows.get_mut(index)?;
        Some(row.extra.get_or_insert_with(Box::default))
    }

    /// Set what the row at `index` is known to contain.
    pub fn set_contents(&mut self, index: usize, contents: PluginContents) {
        if contents == PluginContents::NotRead
            && self.rows.get(index).is_some_and(|row| row.extra.is_none())
        {
            return;
        }
        if let Some(extra) = self.extra_mut(index) {
            extra.contents = contents;
        }
    }

    /// The installed state of the row at `index`, to change in place.
    pub fn installed_mut(&mut self, index: usize) -> Option<&mut InstalledState> {
        self.rows.get_mut(index)?.extra.as_mut()?.installed.as_mut()
    }

    /// The rows in their owned form — for tests and debugging.
    #[must_use]
    pub fn to_rows(&self) -> Vec<PluginRow> {
        self.iter().map(|row| row.to_row()).collect()
    }
}

impl From<Vec<PluginRow>> for PluginList {
    fn from(rows: Vec<PluginRow>) -> Self {
        rows.into_iter().collect()
    }
}

impl FromIterator<PluginRow> for PluginList {
    fn from_iter<I: IntoIterator<Item = PluginRow>>(rows: I) -> Self {
        let mut list = Self::new();
        for row in rows {
            list.push(row);
        }
        list
    }
}

impl fmt::Debug for PluginList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.iter().map(|row| row.to_row()))
            .finish()
    }
}

/// One row of a [`PluginList`], read in place.
#[derive(Clone, Copy)]
pub struct PluginRef<'a> {
    list: &'a PluginList,
    index: usize,
}

impl fmt::Debug for PluginRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_row().fmt(f)
    }
}

impl<'a> PluginRef<'a> {
    fn packed(&self) -> &'a Packed {
        &self.list.rows[self.index]
    }

    fn text(&self, span: Span) -> &'a str {
        let at = span.at as usize;
        self.list
            .text
            .get(at..at + span.len as usize)
            .unwrap_or_default()
    }

    fn opt(&self, span: Span) -> Option<&'a str> {
        (span != Span::NONE).then(|| self.text(span))
    }

    /// Its place in the list.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// The plugin's name — the first half of its id.
    #[must_use]
    pub fn name(&self) -> &'a str {
        self.text(self.packed().name)
    }

    /// Its marketplace — the second half of its id.
    #[must_use]
    pub fn marketplace(&self) -> &'a str {
        self.text(self.packed().marketplace)
    }

    /// Its id, owned.
    #[must_use]
    pub fn id(&self) -> PluginId {
        PluginId {
            name: self.name().to_string(),
            marketplace: self.marketplace().to_string(),
        }
    }

    /// Is it the plugin `id`?
    #[must_use]
    pub fn is(&self, id: &PluginId) -> bool {
        self.name() == id.name && self.marketplace() == id.marketplace
    }

    /// The manifest's display name, when it gives one.
    #[must_use]
    pub fn display_name(&self) -> Option<&'a str> {
        self.opt(self.packed().display_name)
    }

    #[must_use]
    pub fn description(&self) -> Option<&'a str> {
        self.opt(self.packed().description)
    }

    /// The version its marketplace offers.
    #[must_use]
    pub fn version(&self) -> Option<&'a str> {
        self.opt(self.packed().version)
    }

    #[must_use]
    pub fn author(&self) -> Option<&'a str> {
        self.opt(self.packed().author)
    }

    #[must_use]
    pub fn category(&self) -> Option<&'a str> {
        self.opt(self.packed().category)
    }

    #[must_use]
    pub fn homepage(&self) -> Option<&'a str> {
        self.opt(self.packed().homepage)
    }

    /// Its keywords — searched, never shown.
    pub fn keywords(&self) -> impl Iterator<Item = &'a str> + 'a {
        let text = self.text(self.packed().keywords);
        text.split('\n').filter(|keyword| !keyword.is_empty())
    }

    /// Where it is fetched from, in one line.
    #[must_use]
    pub fn source(&self) -> &'a str {
        self.text(self.packed().source)
    }

    /// Why it cannot be installed, when it cannot.
    #[must_use]
    pub fn unavailable(&self) -> Option<&'a str> {
        self.opt(self.packed().unavailable)
    }

    /// Its format, once its files have been read.
    #[must_use]
    pub fn format(&self) -> Option<&'a str> {
        self.opt(self.packed().format)
    }

    fn extra(&self) -> Option<&'a Extra> {
        self.packed().extra.as_deref()
    }

    /// What it is made of, as far as is known.
    #[must_use]
    pub fn contents(&self) -> &'a PluginContents {
        self.extra().map_or(&NOT_READ, |extra| &extra.contents)
    }

    /// Its installed state, when it is installed.
    #[must_use]
    pub fn installed(&self) -> Option<&'a InstalledState> {
        self.extra().and_then(|extra| extra.installed.as_ref())
    }

    /// What reading it said that did not cost it.
    #[must_use]
    pub fn notes(&self) -> &'a [String] {
        self.extra().map_or(&[], |extra| &extra.notes)
    }

    /// The name people see: the display name, else the plugin's own.
    #[must_use]
    pub fn title(&self) -> &'a str {
        self.display_name().unwrap_or_else(|| self.name())
    }

    /// Is it installed and on in this directory?
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.installed().is_some_and(|state| state.enabled)
    }

    /// The version to show: the installed one, else the one its marketplace
    /// offers.
    #[must_use]
    pub fn shown_version(&self) -> Option<&'a str> {
        self.installed()
            .and_then(|state| state.version.as_deref())
            .or_else(|| self.version())
    }

    /// What it is known to contain, when that is read.
    #[must_use]
    pub fn summary(&self) -> Option<&'a ComponentSummary> {
        match self.contents() {
            PluginContents::Known(summary) => Some(summary),
            _ => None,
        }
    }

    /// The row in its owned form.
    #[must_use]
    pub fn to_row(&self) -> PluginRow {
        let own = |text: Option<&str>| text.map(str::to_string);
        PluginRow {
            id: self.id(),
            display_name: own(self.display_name()),
            description: own(self.description()),
            version: own(self.version()),
            author: own(self.author()),
            category: own(self.category()),
            homepage: own(self.homepage()),
            keywords: self.keywords().map(str::to_string).collect(),
            source: self.source().to_string(),
            unavailable: own(self.unavailable()),
            format: own(self.format()),
            contents: self.contents().clone(),
            installed: self.installed().cloned(),
            notes: self.notes().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> PluginRow {
        PluginRow {
            id: PluginId::new(name, "team").unwrap(),
            description: Some(format!("What {name} does")),
            version: Some("1.0.0".into()),
            keywords: vec!["deploy".into(), "ship it".into()],
            source: format!("./plugins/{name}"),
            ..PluginRow::default()
        }
    }

    #[test]
    fn a_pushed_row_reads_back_field_for_field() {
        let mut full = row("hookify");
        full.display_name = Some("Hookify".into());
        full.author = Some("Anthropic".into());
        full.category = Some("dev".into());
        full.homepage = Some("https://h".into());
        full.unavailable = Some("npm".into());
        full.format = Some("Claude Code".into());
        full.notes = vec!["a note".into()];
        full.installed = Some(InstalledState {
            version: Some("0.9".into()),
            enabled: true,
            ..InstalledState::default()
        });
        full.contents = PluginContents::Unknown;
        let plain = row("lint");
        let empty_strings = PluginRow {
            id: PluginId::new("bare", "team").unwrap(),
            description: Some(String::new()),
            ..PluginRow::default()
        };
        let list = PluginList::from(vec![full.clone(), plain.clone(), empty_strings.clone()]);
        assert_eq!(list.to_rows(), [full, plain, empty_strings]);
        let hookify = list.get(0).unwrap();
        assert_eq!(hookify.title(), "Hookify");
        assert_eq!(hookify.shown_version(), Some("0.9"));
        assert!(hookify.is_enabled());
        assert_eq!(
            list.get(1).unwrap().keywords().collect::<Vec<_>>(),
            ["deploy", "ship it"]
        );
        assert_eq!(list.get(1).unwrap().title(), "lint");
        // Present but empty is not absent.
        assert_eq!(list.get(2).unwrap().description(), Some(""));
        assert_eq!(list.get(2).unwrap().version(), None);
    }

    #[test]
    fn a_list_is_a_handful_of_allocations_whatever_its_length() {
        let mut list = PluginList::new();
        for i in 0..400 {
            list.push(row(&format!("p{i}")));
        }
        assert_eq!(list.len(), 400);
        // A plain listed row owns nothing beyond its text.
        assert!(list.rows.iter().all(|row| row.extra.is_none()));
        assert!(list.find(&PluginId::new("p399", "team").unwrap()).is_some());
    }

    #[test]
    fn contents_and_state_change_in_place() {
        let mut installed = row("hookify");
        installed.installed = Some(InstalledState {
            enabled: true,
            ..InstalledState::default()
        });
        let mut list = PluginList::from(vec![row("lint"), installed]);
        assert_eq!(list.get(0).unwrap().contents(), &PluginContents::NotRead);
        list.set_contents(0, PluginContents::Unknown);
        assert_eq!(list.get(0).unwrap().contents(), &PluginContents::Unknown);
        assert!(list.installed_mut(0).is_none());
        list.installed_mut(1).unwrap().enabled = false;
        assert!(!list.get(1).unwrap().is_enabled());
    }

    #[test]
    fn truncate_goes_back_to_a_mark() {
        let mut list = PluginList::from(vec![row("a")]);
        let mark = list.mark();
        list.push(row("b"));
        list.push(row("c"));
        list.truncate(mark);
        assert_eq!(list.to_rows(), [row("a")]);
        list.push(row("d"));
        assert_eq!(list.to_rows(), [row("a"), row("d")]);
    }
}

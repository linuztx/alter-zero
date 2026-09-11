//! The status spinner style: the catalog of animations the live status line
//! can open with, the session's selected one, and the inline `/spinner`
//! picker that switches it. See `docs/spinner.md`.
//!
//! The picker is the `/mascot` picker's shape — same inline frame, same
//! own-every-key search grammar — over one row per style, and the page is
//! **live**: every row wears its own spinner and the highlighted style
//! previews as a whole sample status line, both animated off the frame clock
//! the boundary already injects ([`App::set_pulse`]). The `ui` side renders
//! them through the status line's own renderer, so what the picker shows and
//! what a turn shows can never disagree. The choice persists to
//! `{config_home}/spinner.json` at the boundary (`tui::config`) — **per
//! working directory**, `config.json`'s rule (`docs/per-directory-state.md`);
//! the pure format is the [`SpinnerFile`] instance of the [`LookFile`] the
//! two looks share.
//!
//! The catalog here is **identity only** — a name, a description, an order.
//! What each style *looks* like (its frames, cadence and colours) is styling,
//! and styling lives with every other styling decision in `ui::theme`
//! (`spinner_spans` maps a [`Spinner`] to its frames there).

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// How many rows PageUp/PageDown move the picker (the `/settings` page).
const SPINNER_PAGE: usize = TOOL_VIEW_PAGE;

/// One of the status line's spinner styles — the animation that opens the
/// `{spinner} {verb}… ({elapsed} · …)` line while a turn is in flight. The
/// catalog is fixed; [`Spinner::ALL`] lists it in picker order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Spinner {
    /// The comet — a Larson-scanner sweep between two dim walls. The status
    /// line's look before there was a catalog, and the list still opens with
    /// it; the default moved to [`Gravity`](Self::Gravity).
    Comet,
    /// A ball hopping along a braille track and bouncing off both walls —
    /// the comet's footprint with four dot rows of real vertical motion. The
    /// default: what a session with no `spinner.json` entry opens with.
    #[default]
    Gravity,
    /// A wave rolling down the same braille track and reflecting off the
    /// walls.
    Wave,
    /// A spark blooming into a star and back, in the banner's gradient.
    Sparkle,
    /// The classic braille dots.
    Dots,
    /// The mascots' own quadrant block glyphs, turning in the banner's
    /// gradient.
    Blocks,
    /// One dot breathing dim → bright — the running tool bullet's pulse.
    Pulse,
    /// A bar rising and falling like a level meter.
    Bars,
    /// The classic ASCII line, for a font with none of the glyphs above.
    Line,
}

impl Spinner {
    /// Every style, in the order the `/spinner` picker lists them.
    pub const ALL: [Self; 9] = [
        Self::Comet,
        Self::Gravity,
        Self::Wave,
        Self::Sparkle,
        Self::Dots,
        Self::Blocks,
        Self::Pulse,
        Self::Bars,
        Self::Line,
    ];

    /// The lowercase name the picker lists and `spinner.json` records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Comet => "comet",
            Self::Gravity => "gravity",
            Self::Wave => "wave",
            Self::Sparkle => "sparkle",
            Self::Dots => "dots",
            Self::Blocks => "blocks",
            Self::Pulse => "pulse",
            Self::Bars => "bars",
            Self::Line => "line",
        }
    }

    /// The one-line description shown under the picker's preview.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Comet => "A comet sweeping between two dim walls",
            Self::Gravity => "A ball hopping along the track, bouncing off both walls",
            Self::Wave => "A wave rolling down the track, reflecting off the walls",
            Self::Sparkle => "A spark blooming into a star in the banner's cyan",
            Self::Dots => "The classic braille dots, circling",
            Self::Blocks => "The mascot's own block glyphs, turning in its gradient",
            Self::Pulse => "One dot breathing dim to bright, like a running tool",
            Self::Bars => "A bar rising and falling like a level meter",
            Self::Line => "The classic spinning line, in any font",
        }
    }

    /// The style with this name (case-insensitive), if the catalog holds one
    /// — how `spinner.json` reads back ([`Look::from_name`]).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.name().eq_ignore_ascii_case(name))
    }
}

/// The `spinner.json` side of the catalog: the key the file records a choice
/// under, over the name mapping above (`docs/per-directory-state.md`).
impl Look for Spinner {
    const KEY: &'static str = "spinner";

    fn name(self) -> &'static str {
        Self::name(self)
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::from_name(name)
    }
}

/// One row of the `/spinner` picker: the style and whether it is the
/// session's current one (the ✓, the `/model` picker's active mark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpinnerRow {
    /// Which style this row is.
    pub spinner: Spinner,
    /// Whether this is the session's current style.
    pub active: bool,
}

/// The open `/spinner` picker's state (`None` on [`App`] when closed).
///
/// Like [`MascotPicker`] it **replaces the composer** in the bottom live
/// region and owns every key while open; the catalog is a const, so there is
/// nothing to load and no status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpinnerPicker {
    /// Index of the highlighted row within the current filtered rows.
    pub selected: usize,
    /// The type-to-search query — printable keys **except space** append
    /// (space selects, the `/settings` grammar), Backspace pops, Esc clears
    /// then closes.
    pub query: String,
    /// The frame clock ([`App::pulse`]) at the moment the picker opened. The
    /// page's preview counts its elapsed from here, so the sample status line
    /// reads `0s` when it appears and counts up while the user browses — a
    /// turn that just began, rather than a clock that has been running since
    /// the loop started.
    pub opened_at: Duration,
}

impl App {
    /// The session's status spinner style — what the live status line opens
    /// with while a turn runs.
    #[must_use]
    pub const fn spinner(&self) -> Spinner {
        self.spinner
    }

    /// Set the spinner style (the boundary's startup seed from
    /// `spinner.json`, and the picker's Enter). Live-only state — the status
    /// line is never committed — so a switch shows on the very next frame.
    pub const fn set_spinner(&mut self, spinner: Spinner) {
        self.spinner = spinner;
    }

    /// Open the inline `/spinner` picker, the highlight seated on the current
    /// style. Abandons any `?` band / palette / file picker (they share the
    /// composer the picker takes over) — the `/settings` open's rule.
    pub fn open_spinner_picker(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        let selected = Spinner::ALL
            .iter()
            .position(|&s| s == self.spinner)
            .unwrap_or(0);
        self.spinner_picker = Some(SpinnerPicker {
            selected,
            query: String::new(),
            opened_at: self.pulse,
        });
    }

    /// Dismiss the picker (Esc on an empty query, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_spinner_picker(&mut self) {
        self.spinner_picker = None;
    }

    /// The rows the picker shows: every style whose name or description
    /// contains the query (case-insensitive substring; all of them for an
    /// empty query), in catalog order — the `/settings` menu's filter.
    #[must_use]
    pub fn spinner_rows(&self) -> Vec<SpinnerRow> {
        let query = self
            .spinner_picker
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        Spinner::ALL
            .into_iter()
            .filter(|s| {
                query.is_empty()
                    || s.name().contains(&query)
                    || s.description().to_lowercase().contains(&query)
            })
            .map(|spinner| SpinnerRow {
                spinner,
                active: spinner == self.spinner,
            })
            .collect()
    }

    /// The highlighted style, if the picker is open and the query matches
    /// something.
    #[must_use]
    pub fn highlighted_spinner(&self) -> Option<Spinner> {
        let picker = self.spinner_picker.as_ref()?;
        let rows = self.spinner_rows();
        rows.get(picker.selected.min(rows.len().saturating_sub(1)))
            .map(|row| row.spinner)
    }

    /// How long the picker has been open on the frame clock — the elapsed the
    /// page's live preview (and every row's spinner) animates against, so the
    /// sample line counts up from `0s` like a turn that just began. Zero while
    /// the picker is closed. See [`SpinnerPicker::opened_at`].
    #[must_use]
    pub fn spinner_preview_elapsed(&self) -> Duration {
        self.spinner_picker
            .as_ref()
            .map_or(Duration::ZERO, |p| self.pulse.saturating_sub(p.opened_at))
    }

    /// Enter/Space: adopt the highlighted style and close. The pure state
    /// moves here; the loop persists it and confirms with a toast
    /// ([`Action::SelectSpinner`]).
    fn select_highlighted_spinner(&mut self) -> Action {
        let Some(spinner) = self.highlighted_spinner() else {
            return Action::None;
        };
        self.spinner = spinner;
        self.spinner_picker = None;
        Action::SelectSpinner(spinner)
    }

    /// Keys while the inline `/spinner` picker is open — the `/settings`
    /// menu's grammar (Enter and Space both select, so a space never reaches
    /// the search). Owns **every** key while open (routed at the top of
    /// [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_spinner_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the picker (never quits — the composer-clear/quit
        // rules don't apply while it owns the keys), like the `/settings` menu.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_spinner_picker();
            return Action::CloseSpinnerPicker;
        }
        let len = self.spinner_rows().len();
        let last = len.saturating_sub(1);
        let Some(picker) = self.spinner_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(SPINNER_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + SPINNER_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            // Enter and Space both choose — the preview already showed
            // exactly what lands, so choosing is the picker's one job.
            KeyCode::Enter | KeyCode::Char(' ') => return self.select_highlighted_spinner(),
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_spinner_picker();
                    return Action::CloseSpinnerPicker;
                }
                picker.query.clear();
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.query.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Action::None
    }
}

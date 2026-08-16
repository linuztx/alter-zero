//! The banner mascot: the catalog of block-character creatures the startup
//! header draws beside its metadata, the session's selected one, and the
//! inline `/mascot` picker that switches it. See `docs/mascot.md`.
//!
//! The picker is the `/settings` family's shape — same inline frame, same
//! own-every-key search grammar — over one row per mascot, with the page
//! previewing the highlighted mascot's *actual banner* (the `ui` side renders
//! it through the same builder the startup header uses, so the preview can
//! never disagree with what Enter produces). The choice persists to
//! `{config_home}/mascot.json` at the boundary (`tui::config`); the pure
//! format lives here ([`mascot_file_json`] / [`parse_mascot_file`]).

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// How many rows PageUp/PageDown move the picker (the `/settings` page).
const MASCOT_PAGE: usize = TOOL_VIEW_PAGE;

/// One of the banner mascots — a small block-character creature drawn
/// flush-left in the startup header, wearing the banner's cyan → blue
/// gradient. The catalog is fixed; [`Mascot::ALL`] lists it in picker order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mascot {
    /// The crested hatchling — the default.
    #[default]
    Crest,
    Bloom,
    Sprout,
    Twin,
    Skiter,
    Gem,
}

impl Mascot {
    /// Every mascot, in the order the `/mascot` picker lists them.
    pub const ALL: [Self; 6] = [
        Self::Crest,
        Self::Bloom,
        Self::Sprout,
        Self::Twin,
        Self::Skiter,
        Self::Gem,
    ];

    /// The lowercase name the picker lists and `mascot.json` records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Crest => "crest",
            Self::Bloom => "bloom",
            Self::Sprout => "sprout",
            Self::Twin => "twin",
            Self::Skiter => "skiter",
            Self::Gem => "gem",
        }
    }

    /// The one-line description shown under the picker's preview.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Crest => "A crested hatchling flaring its frill",
            Self::Bloom => "A round little creature in full bloom",
            Self::Sprout => "A seedling pushing up its first leaf",
            Self::Twin => "Two tufted ears over one shared grin",
            Self::Skiter => "A many-legged skitterer on the move",
            Self::Gem => "A cut gem catching the terminal light",
        }
    }

    /// The block-character art, one `&str` per row, drawn verbatim (leading
    /// spaces included — a `\`-continued literal would strip them). Every
    /// glyph is single-width (`docs/table-streaming.md` *Wide glyphs*).
    #[must_use]
    pub const fn art(self) -> &'static [&'static str] {
        match self {
            Self::Crest => &[" ▙▄▙▄▟▄▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"],
            Self::Bloom => &[" ▄█████▄", "▀█▄███▄█▀", " ▝▝   ▘▘"],
            Self::Sprout => &["    █", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"],
            Self::Twin => &[" ██▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"],
            Self::Skiter => &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"],
            Self::Gem => &["  ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘"],
        }
    }

    /// The widest art row in display columns — where the banner's metadata
    /// column starts (past the [`crate::ui`] gap). The art is pure ASCII-width
    /// block glyphs (one column each), so `chars().count()` *is* the display
    /// width; asserted by the catalog tests against the real width measure.
    #[must_use]
    pub fn art_width(self) -> usize {
        self.art()
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// The mascot with this name (case-insensitive), if the catalog holds one
    /// — how `mascot.json` reads back.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name))
    }
}

/// The `mascot.json` blob recording `mascot` — `{"mascot": "crest"}`. Names
/// are static lowercase ASCII, so the literal formatting needs no escaping.
#[must_use]
pub fn mascot_file_json(mascot: Mascot) -> String {
    format!("{{\n  \"mascot\": \"{}\"\n}}\n", mascot.name())
}

/// Read a `mascot.json` blob back. `None` for anything that doesn't parse to
/// a known mascot — the boundary then keeps the default (the
/// `load_settings` posture: a corrupt preference file must never block
/// startup).
#[must_use]
pub fn parse_mascot_file(text: &str) -> Option<Mascot> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    Mascot::from_name(value.get("mascot")?.as_str()?)
}

/// One row of the `/mascot` picker: the mascot and whether it is the
/// session's current one (the ✓, the `/model` picker's active mark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MascotRow {
    /// Which mascot this row is.
    pub mascot: Mascot,
    /// Whether this is the session's current mascot.
    pub active: bool,
}

/// The open `/mascot` picker's state (`None` on [`App`] when closed).
///
/// Like [`SettingsPicker`] it **replaces the composer** in the bottom live
/// region and owns every key while open; there is nothing to load, so there
/// is no status — the catalog is a const.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MascotPicker {
    /// Index of the highlighted row within the current filtered rows.
    pub selected: usize,
    /// The type-to-search query — printable keys **except space** append
    /// (space selects, the `/settings` grammar), Backspace pops, Esc clears
    /// then closes.
    pub query: String,
}

impl App {
    /// The session's banner mascot — what the startup header draws.
    #[must_use]
    pub const fn mascot(&self) -> Mascot {
        self.mascot
    }

    /// Set the banner mascot (the boundary's startup seed from `mascot.json`,
    /// and the picker's Enter).
    pub const fn set_mascot(&mut self, mascot: Mascot) {
        self.mascot = mascot;
    }

    /// Open the inline `/mascot` picker, the highlight seated on the current
    /// mascot. Abandons any `?` band / palette / file picker (they share the
    /// composer the picker takes over) — the `/settings` open's rule.
    pub fn open_mascot_picker(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        let selected = Mascot::ALL
            .iter()
            .position(|&m| m == self.mascot)
            .unwrap_or(0);
        self.mascot_picker = Some(MascotPicker {
            selected,
            query: String::new(),
        });
    }

    /// Dismiss the picker (Esc on an empty query, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_mascot_picker(&mut self) {
        self.mascot_picker = None;
    }

    /// The rows the picker shows: every mascot whose name or description
    /// contains the query (case-insensitive substring; all of them for an
    /// empty query), in catalog order — the `/settings` menu's filter.
    #[must_use]
    pub fn mascot_rows(&self) -> Vec<MascotRow> {
        let query = self
            .mascot_picker
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        Mascot::ALL
            .into_iter()
            .filter(|m| {
                query.is_empty()
                    || m.name().contains(&query)
                    || m.description().to_lowercase().contains(&query)
            })
            .map(|mascot| MascotRow {
                mascot,
                active: mascot == self.mascot,
            })
            .collect()
    }

    /// The highlighted mascot, if the picker is open and the query matches
    /// something.
    #[must_use]
    pub fn highlighted_mascot(&self) -> Option<Mascot> {
        let picker = self.mascot_picker.as_ref()?;
        let rows = self.mascot_rows();
        rows.get(picker.selected.min(rows.len().saturating_sub(1)))
            .map(|row| row.mascot)
    }

    /// Enter/Space: adopt the highlighted mascot and close. The pure state
    /// moves here; the loop persists it, repaints the banner, and confirms
    /// with a toast ([`Action::SelectMascot`]).
    fn select_highlighted_mascot(&mut self) -> Action {
        let Some(mascot) = self.highlighted_mascot() else {
            return Action::None;
        };
        self.mascot = mascot;
        self.mascot_picker = None;
        Action::SelectMascot(mascot)
    }

    /// Keys while the inline `/mascot` picker is open — the `/settings`
    /// menu's grammar (Enter and Space both select, so a space never reaches
    /// the search). Owns **every** key while open (routed at the top of
    /// [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_mascot_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the picker (never quits — the composer-clear/quit
        // rules don't apply while it owns the keys), like the `/settings` menu.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_mascot_picker();
            return Action::CloseMascotPicker;
        }
        let len = self.mascot_rows().len();
        let last = len.saturating_sub(1);
        let Some(picker) = self.mascot_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(MASCOT_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + MASCOT_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            // Enter and Space both choose — the preview already showed
            // exactly what lands, so choosing is the picker's one job.
            KeyCode::Enter | KeyCode::Char(' ') => return self.select_highlighted_mascot(),
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_mascot_picker();
                    return Action::CloseMascotPicker;
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

//! The colour theme: the catalog of palettes the whole TUI can wear — the
//! chrome's accent and semantic colours together with the code blocks'
//! syntax theme, one design system per entry — the session's selected one,
//! and the inline `/theme` picker that switches it. See `docs/theme.md`.
//!
//! The picker is the `/spinner` picker's shape — same inline frame, same
//! own-every-key search grammar — over one row per theme, and the page
//! previews the highlighted theme on **real cells**: a user bubble, an
//! `Edit` diff cell and an assistant reply with a code block, rendered by
//! the very builders the conversation uses, so what the picker shows and
//! what Enter produces can never disagree. The choice persists to
//! `{config_home}/theme.json` at the boundary (`tui::config`); the pure
//! format lives here ([`theme_file_json`] / [`parse_theme_file`]).
//!
//! The catalog here is **identity only** — a name, a title, a description,
//! an order. What each theme *looks* like (its palette and its syntect
//! theme) is styling, and styling lives with every other styling decision
//! in `ui::theme` (`palette_of` maps a [`Theme`] to its palette there), the
//! `/spinner` rule.

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// How many rows PageUp/PageDown move the picker (the `/settings` page).
const THEME_PAGE: usize = TOOL_VIEW_PAGE;

/// One of the colour themes — a palette for the chrome (the accent the
/// pickers select with, the success/error/warning hues, the dim greys, the
/// user bubble, the diff tints) paired with the matching syntax theme for
/// code blocks and file cells. The catalog is fixed; [`Theme::ALL`] lists it
/// in picker order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Catppuccin Mocha — the darkest flavour, and the default: the code
    /// blocks have worn it since the syntect port, so the chrome now matches.
    #[default]
    Mocha,
    /// Catppuccin Macchiato.
    Macchiato,
    /// Catppuccin Frappé.
    Frappe,
    /// Catppuccin Latte — the light flavour.
    Latte,
    /// One Dark — the TUI's original chrome, over Atom's One Dark code
    /// palette.
    OneDark,
    /// Dracula.
    Dracula,
    /// Nord.
    Nord,
    /// Gruvbox (dark).
    Gruvbox,
    /// Solarized (dark).
    Solarized,
    /// Monokai.
    Monokai,
    /// The terminal's own sixteen ANSI colours — the TUI follows whatever
    /// palette the terminal is configured with.
    Ansi,
}

impl Theme {
    /// Every theme, in the order the `/theme` picker lists them.
    pub const ALL: [Self; 11] = [
        Self::Mocha,
        Self::Macchiato,
        Self::Frappe,
        Self::Latte,
        Self::OneDark,
        Self::Dracula,
        Self::Nord,
        Self::Gruvbox,
        Self::Solarized,
        Self::Monokai,
        Self::Ansi,
    ];

    /// The lowercase name the picker lists and `theme.json` records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mocha => "mocha",
            Self::Macchiato => "macchiato",
            Self::Frappe => "frappe",
            Self::Latte => "latte",
            Self::OneDark => "onedark",
            Self::Dracula => "dracula",
            Self::Nord => "nord",
            Self::Gruvbox => "gruvbox",
            Self::Solarized => "solarized",
            Self::Monokai => "monokai",
            Self::Ansi => "ansi",
        }
    }

    /// The theme's proper name — what its description opens with, so the
    /// bare row name is explained right under the preview.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Mocha => "Catppuccin Mocha",
            Self::Macchiato => "Catppuccin Macchiato",
            Self::Frappe => "Catppuccin Frappé",
            Self::Latte => "Catppuccin Latte",
            Self::OneDark => "One Dark",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox Dark",
            Self::Solarized => "Solarized Dark",
            Self::Monokai => "Monokai",
            Self::Ansi => "Terminal",
        }
    }

    /// The one-line description shown under the picker's preview — the
    /// title, then what sets the theme apart.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Mocha => "Catppuccin Mocha — the darkest flavour, and the default",
            Self::Macchiato => "Catppuccin Macchiato — the medium-dark flavour",
            Self::Frappe => "Catppuccin Frappé — the lightest of the dark flavours",
            Self::Latte => "Catppuccin Latte — the light flavour, for a light terminal",
            Self::OneDark => "One Dark — Atom's classic palette, this TUI's original look",
            Self::Dracula => "Dracula — vivid purple, pink and cyan on a dark ground",
            Self::Nord => "Nord — arctic, bluish and calm",
            Self::Gruvbox => "Gruvbox Dark — retro groove, warm and earthy",
            Self::Solarized => "Solarized Dark — Ethan Schoonover's precision palette",
            Self::Monokai => "Monokai — the classic editor palette",
            Self::Ansi => {
                "Terminal — the terminal's own 16 ANSI colours, so the TUI follows its theme"
            }
        }
    }

    /// Whether the theme is meant for a **light** terminal background — its
    /// text is dark and its tints are pale, so on a dark terminal it reads
    /// washed out.
    #[must_use]
    pub const fn is_light(self) -> bool {
        matches!(self, Self::Latte)
    }

    /// The theme with this name (case-insensitive), if the catalog holds one
    /// — how `theme.json` reads back.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(name))
    }
}

/// The `theme.json` blob recording the theme — `{"theme": "mocha"}`. Names
/// are static lowercase ASCII, so the literal formatting needs no escaping
/// (the `spinner.json` shape, `docs/spinner.md`).
#[must_use]
pub fn theme_file_json(theme: Theme) -> String {
    format!("{{\n  \"theme\": \"{}\"\n}}\n", theme.name())
}

/// Read a `theme.json` blob back. `None` for anything that doesn't parse to
/// a known theme — the boundary then keeps the default (a corrupt preference
/// file must never block startup).
#[must_use]
pub fn parse_theme_file(text: &str) -> Option<Theme> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    Theme::from_name(value.get("theme")?.as_str()?)
}

/// One row of the `/theme` picker: the theme and whether it is the session's
/// current one (the ✓, the `/model` picker's active mark).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeRow {
    /// Which theme this row is.
    pub theme: Theme,
    /// Whether this is the session's current theme.
    pub active: bool,
}

/// The open `/theme` picker's state (`None` on [`App`] when closed).
///
/// Like [`SpinnerPicker`] it **replaces the composer** in the bottom live
/// region and owns every key while open; the catalog is a const, so there is
/// nothing to load and no status. Unlike the spinner's page it is still —
/// nothing on it ticks — so it carries no clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemePicker {
    /// Index of the highlighted row within the current filtered rows.
    pub selected: usize,
    /// The type-to-search query — printable keys **except space** append
    /// (space selects, the `/settings` grammar), Backspace pops, Esc clears
    /// then closes.
    pub query: String,
}

impl App {
    /// The session's colour theme — what every rendered row wears, once the
    /// boundary has activated it (`ui::activate_theme`; the pure state and
    /// the ambient palette are kept equal at the boundary, `docs/theme.md`).
    #[must_use]
    pub const fn theme(&self) -> Theme {
        self.theme
    }

    /// Set the theme (the boundary's startup seed from `theme.json`, and the
    /// picker's Enter). Pure state only: the palette the renderers read is
    /// activated at the boundary, which also purge-rebuilds the screen so
    /// every committed row changes colour at once.
    pub const fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Open the inline `/theme` picker, the highlight seated on the current
    /// theme. Abandons any `?` band / palette / file picker (they share the
    /// composer the picker takes over) — the `/settings` open's rule.
    pub fn open_theme_picker(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        let selected = Theme::ALL
            .iter()
            .position(|&t| t == self.theme)
            .unwrap_or(0);
        self.theme_picker = Some(ThemePicker {
            selected,
            query: String::new(),
        });
    }

    /// Dismiss the picker (Esc on an empty query, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_theme_picker(&mut self) {
        self.theme_picker = None;
    }

    /// The rows the picker shows: every theme whose name, title or
    /// description contains the query (case-insensitive substring; all of
    /// them for an empty query), in catalog order — the `/settings` menu's
    /// filter.
    #[must_use]
    pub fn theme_rows(&self) -> Vec<ThemeRow> {
        let query = self
            .theme_picker
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        Theme::ALL
            .into_iter()
            .filter(|t| {
                query.is_empty()
                    || t.name().contains(&query)
                    || t.title().to_lowercase().contains(&query)
                    || t.description().to_lowercase().contains(&query)
            })
            .map(|theme| ThemeRow {
                theme,
                active: theme == self.theme,
            })
            .collect()
    }

    /// The highlighted theme, if the picker is open and the query matches
    /// something.
    #[must_use]
    pub fn highlighted_theme(&self) -> Option<Theme> {
        let picker = self.theme_picker.as_ref()?;
        let rows = self.theme_rows();
        rows.get(picker.selected.min(rows.len().saturating_sub(1)))
            .map(|row| row.theme)
    }

    /// Enter/Space: adopt the highlighted theme and close. The pure state
    /// moves here; the loop activates the palette, persists the choice,
    /// purge-rebuilds and confirms with a toast ([`Action::SelectTheme`]).
    fn select_highlighted_theme(&mut self) -> Action {
        let Some(theme) = self.highlighted_theme() else {
            return Action::None;
        };
        self.theme = theme;
        self.theme_picker = None;
        Action::SelectTheme(theme)
    }

    /// Keys while the inline `/theme` picker is open — the `/settings`
    /// menu's grammar (Enter and Space both select, so a space never reaches
    /// the search). Owns **every** key while open (routed at the top of
    /// [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_theme_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the picker (never quits — the composer-clear/quit
        // rules don't apply while it owns the keys), like the `/settings` menu.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_theme_picker();
            return Action::CloseThemePicker;
        }
        let len = self.theme_rows().len();
        let last = len.saturating_sub(1);
        let Some(picker) = self.theme_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(THEME_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + THEME_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            // Enter and Space both choose — the preview already showed
            // exactly what lands, so choosing is the picker's one job.
            KeyCode::Enter | KeyCode::Char(' ') => return self.select_highlighted_theme(),
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_theme_picker();
                    return Action::CloseThemePicker;
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

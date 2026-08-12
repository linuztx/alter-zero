//! The inline `/settings` menu: the open picker's search + selection, the rows
//! it derives from the live session, and the cycle its Enter/Space performs.
//!
//! The sibling of [`super::model_picker`] — same inline shape, same
//! own-every-key rule — over the pure [`crate::settings`] model. Its rows are
//! **derived, never stored**, so the value column can't drift from what the
//! session is actually doing. See `docs/settings.md`.

use super::views::TOOL_VIEW_PAGE;
use super::*;

use crate::settings::{SettingAvailability, SettingKey};

/// How many rows PageUp/PageDown move the menu (the `/model` picker's page).
const SETTINGS_PAGE: usize = TOOL_VIEW_PAGE;

/// One rendered row of the menu, built on demand by [`App::setting_rows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    /// Which setting this row is.
    pub key: SettingKey,
    /// The left column — the setting's name.
    pub label: &'static str,
    /// The right column — its **effective** value, `(unavailable)`-suffixed
    /// when this session can't run the feature at all.
    pub value: String,
    /// The line shown under the list while this row is highlighted.
    pub description: &'static str,
    /// Whether Enter/Space actually cycles it.
    pub available: bool,
}

/// The open `/settings` menu's state (`None` on [`App`] when closed).
///
/// Like [`ModelPicker`] it **replaces the composer** in the bottom live region
/// and owns every key while open; unlike it there is nothing to load, so there
/// is no status — the rows exist the moment it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPicker {
    /// Index of the highlighted row within the current filtered rows.
    pub selected: usize,
    /// The type-to-search query — printable keys **except space** append
    /// (space is the cycle key), Backspace pops, Esc clears then closes.
    pub query: String,
}

impl App {
    /// The session's live settings — what the boundary reads to decide whether
    /// to show thinking, offer tools, snapshot the tree, and so on.
    #[must_use]
    pub const fn settings(&self) -> &crate::settings::SessionSettings {
        &self.settings
    }

    /// Mutable access for the boundary's startup seed (the file + the
    /// `ALTER_ZERO_*` overrides) — the pure key handling goes through the
    /// menu's own `cycle_selected_setting`.
    pub const fn settings_mut(&mut self) -> &mut crate::settings::SessionSettings {
        &mut self.settings
    }

    /// Inject what this host can actually run (the `set_clock` pattern): a
    /// feature the boundary can't serve shows `(unavailable)` and refuses to
    /// cycle. See `docs/settings.md`.
    pub const fn set_setting_availability(&mut self, availability: SettingAvailability) {
        self.settings.availability = availability;
    }

    /// Open the inline `/settings` menu. Abandons any `?` band / palette /
    /// file picker (they share the composer the menu takes over), but stays in
    /// [`View::Conversation`] — the menu is inline, not an overlay.
    pub fn open_settings(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.settings_picker = Some(SettingsPicker::default());
    }

    /// Dismiss the menu (Esc on an empty query, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_settings(&mut self) {
        self.settings_picker = None;
    }

    /// The rows the menu shows: every setting whose label or description
    /// contains the query (case-insensitive substring; all of them for an
    /// empty query), in registry order. Derived on each call from the live
    /// state — the `/model` picker's `matches` pattern.
    #[must_use]
    pub fn setting_rows(&self) -> Vec<SettingRow> {
        let query = self
            .settings_picker
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        let mode = self.permission_mode();
        SettingKey::ALL
            .iter()
            .filter(|key| {
                query.is_empty()
                    || key.label().to_lowercase().contains(&query)
                    || key.description().to_lowercase().contains(&query)
            })
            .map(|&key| SettingRow {
                key,
                label: key.label(),
                value: self.settings.value_text(key, mode),
                description: key.description(),
                available: self.settings.is_available(key, mode),
            })
            .collect()
    }

    /// The highlighted row, if the menu is open and the query matches
    /// something.
    #[must_use]
    pub fn highlighted_setting(&self) -> Option<SettingRow> {
        let picker = self.settings_picker.as_ref()?;
        self.setting_rows().into_iter().nth(picker.selected)
    }

    /// Enter/Space: advance the highlighted setting and tell the loop what to
    /// re-apply.
    ///
    /// Two knobs don't belong to the pure blob and route elsewhere:
    /// **Permission mode** returns the existing [`Action::SetPermissionMode`]
    /// (one state, two doors — Ctrl+A's path mirrors the gate, sweeps the
    /// covered requests and persists the project entry), and an *unavailable*
    /// setting explains itself with a toast rather than silently doing
    /// nothing. Everything else moves here and the boundary applies it
    /// ([`Action::SettingChanged`]).
    fn cycle_selected_setting(&mut self) -> Action {
        let Some(row) = self.highlighted_setting() else {
            return Action::None;
        };
        if row.key == SettingKey::PermissionMode {
            return self.toggle_permission_mode();
        }
        if !row.available {
            // Phrased around the label rather than after it, so it reads for a
            // singular row and a plural one alike ("Can't change Checkpoints…"
            // / "…Permission mode…").
            return Action::Toast(format!("Can't change {} in this session", row.label));
        }
        if self.settings.cycle(row.key) {
            Action::SettingChanged(row.key)
        } else {
            Action::None
        }
    }

    /// Keys while the inline `/settings` menu is open. The `/model` picker's
    /// grammar, plus **Space** as a second cycle key — which is why a plain
    /// space never reaches the search. Owns **every** key while open (routed
    /// at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_settings(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the menu (never quits — the composer-clear/quit rules
        // don't apply while it owns the keys), like the `/model` picker.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_settings();
            return Action::CloseSettings;
        }
        let last = self.setting_rows().len().saturating_sub(1);
        let Some(picker) = self.settings_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(last),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(SETTINGS_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + SETTINGS_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            // Enter and Space both cycle — the menu stays open so several
            // knobs can be set in one visit.
            KeyCode::Enter | KeyCode::Char(' ') => return self.cycle_selected_setting(),
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_settings();
                    return Action::CloseSettings;
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

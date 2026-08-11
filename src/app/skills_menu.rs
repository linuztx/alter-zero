//! The inline `/skills` menu: browse the discovered skills and turn any one
//! of them on or off.
//!
//! The fifth composer-replacing inline picker, and deliberately the
//! [`super::settings`] menu's twin rather than a new shape — same frame, same
//! `❯` type-to-search, same `{label}  {value}` column, same Enter/Space
//! toggle. What differs is only what the rows *are*: one per discovered
//! skill instead of one per knob, with the skill's own `description` as the
//! line under the list, so the picker doubles as the browser that answers
//! "what is this skill for?".
//!
//! The rows are **derived, never stored** ([`App::skill_menu_rows`]) from the
//! snapshot the boundary injects at open, so the value column can't drift
//! from what the session is actually offering the model. See `docs/skills.md`.

use super::views::TOOL_VIEW_PAGE;
use super::*;

use crate::skills::SkillMetadata;

/// How many rows PageUp/PageDown move the menu (the `/settings` menu's page).
const SKILLS_PAGE: usize = TOOL_VIEW_PAGE;

/// One rendered row of the menu, built on demand by [`App::skill_menu_rows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMenuRow {
    /// The skill's invocable name — the left column, and what a toggle names.
    pub name: String,
    /// The skill's own one-line description, shown under the list while this
    /// row is highlighted.
    pub description: String,
    /// Whether the model is currently offered this skill.
    pub enabled: bool,
}

/// The open `/skills` menu's state (`None` on [`App`] when closed).
///
/// Holds the skills themselves rather than reaching for a registry: the pure
/// core never touches the boundary's shared handles, so the snapshot is
/// injected whole at open — the `/hooks` menu's seam (`docs/hooks-menu.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillsMenu {
    /// Every discovered skill, in precedence order — the **disabled ones
    /// included**, since a skill you turned off is exactly the one you need
    /// to see to turn back on.
    pub skills: Vec<SkillMetadata>,
    /// The names currently turned off.
    pub disabled: std::collections::BTreeSet<String>,
    /// Whether skills are on for the session at all (the `/settings`
    /// **Skills** row, `ALTER_ZERO_SKILLS`). `false` shows a note rather than
    /// silently listing rows that do nothing — the `/hooks` menu's rule.
    pub session_enabled: bool,
    /// Where skills are looked for, `~`-relativized at the boundary. Shown
    /// when nothing was found, because "why is my skill not here?" is the
    /// only question an empty list ever raises.
    pub roots: Vec<String>,
    /// Index of the highlighted row within the current filtered rows.
    pub selected: usize,
    /// The type-to-search query — printable keys **except space** append
    /// (space is the toggle key), Backspace pops, Esc clears then closes.
    pub query: String,
}

impl App {
    /// Open the inline `/skills` menu over the boundary's snapshot. Abandons
    /// any `?` band / palette / file picker (they share the composer the menu
    /// takes over), but stays in [`View::Conversation`] — the menu is inline,
    /// not an overlay.
    pub fn open_skills_menu(
        &mut self,
        skills: Vec<SkillMetadata>,
        disabled: std::collections::BTreeSet<String>,
        session_enabled: bool,
        roots: Vec<String>,
    ) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.backtrack = Backtrack::default();
        self.skills_menu = Some(SkillsMenu {
            skills,
            disabled,
            session_enabled,
            roots,
            selected: 0,
            query: String::new(),
        });
    }

    /// Dismiss the menu (Esc on an empty query, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_skills_menu(&mut self) {
        self.skills_menu = None;
    }

    /// The rows the menu shows: every skill whose name or description
    /// contains the query (case-insensitive substring; all of them for an
    /// empty query), in discovery order. Derived on each call, the
    /// `/settings` menu's `setting_rows` pattern.
    #[must_use]
    pub fn skill_menu_rows(&self) -> Vec<SkillMenuRow> {
        let Some(menu) = self.skills_menu.as_ref() else {
            return Vec::new();
        };
        let query = menu.query.to_lowercase();
        menu.skills
            .iter()
            .filter(|skill| {
                query.is_empty()
                    || skill.name.to_lowercase().contains(&query)
                    || skill.description.to_lowercase().contains(&query)
            })
            .map(|skill| SkillMenuRow {
                name: skill.name.clone(),
                description: skill.description.clone(),
                enabled: !menu.disabled.contains(&skill.name),
            })
            .collect()
    }

    /// The highlighted row, if the menu is open and the query matches
    /// something.
    #[must_use]
    pub fn highlighted_skill(&self) -> Option<SkillMenuRow> {
        let menu = self.skills_menu.as_ref()?;
        self.skill_menu_rows().into_iter().nth(menu.selected)
    }

    /// Enter/Space: flip the highlighted skill and tell the loop to make it
    /// true of the session (the registry, the listing, the file).
    ///
    /// The menu's own copy of the disabled set moves first so the row's value
    /// column updates in the same frame — the loop then applies the same
    /// change to the shared registry. Both halves are the *same* decision, so
    /// they cannot disagree: the action carries the name and the new state.
    fn toggle_selected_skill(&mut self) -> Action {
        let Some(row) = self.highlighted_skill() else {
            return Action::None;
        };
        let Some(menu) = self.skills_menu.as_mut() else {
            return Action::None;
        };
        let enabled = if row.enabled {
            menu.disabled.insert(row.name.clone());
            false
        } else {
            menu.disabled.remove(&row.name);
            true
        };
        Action::SkillToggled {
            name: row.name,
            enabled,
        }
    }

    /// Keys while the inline `/skills` menu is open — the `/settings` menu's
    /// grammar exactly, including **Space** as a second toggle key (which is
    /// why a plain space never reaches the search). Owns **every** key while
    /// open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_skills(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the menu (never quits — the composer-clear/quit rules
        // don't apply while it owns the keys), like every sibling picker.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_skills_menu();
            return Action::CloseSkillsMenu;
        }
        let last = self.skill_menu_rows().len().saturating_sub(1);
        let Some(menu) = self.skills_menu.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Down => menu.selected = (menu.selected + 1).min(last),
            KeyCode::PageUp => menu.selected = menu.selected.saturating_sub(SKILLS_PAGE),
            KeyCode::PageDown => menu.selected = (menu.selected + SKILLS_PAGE).min(last),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = last,
            // Enter and Space both toggle — the menu stays open so several
            // skills can be set in one visit.
            KeyCode::Enter | KeyCode::Char(' ') => return self.toggle_selected_skill(),
            KeyCode::Esc => {
                if menu.query.is_empty() {
                    self.close_skills_menu();
                    return Action::CloseSkillsMenu;
                }
                menu.query.clear();
                menu.selected = 0;
            }
            KeyCode::Backspace => {
                menu.query.pop();
                menu.selected = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                menu.query.push(c);
                menu.selected = 0;
            }
            _ => {}
        }
        Action::None
    }
}

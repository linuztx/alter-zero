//! The read-only `/hooks` menu: the open browser's level stack and key map.
//!
//! The fourth composer-replacing inline picker, beside
//! [`super::model_picker`], [`super::login`] and [`super::settings`] — it
//! renders between the picker rules in place of the composer, owns every key
//! while open, and closes back to the composer. Unlike those three it has
//! **no text entry** (navigation is the whole grammar), so there is no query
//! and the hardware cursor parks in the frame's corner. The data it browses —
//! a [`HooksOverview`] digest of the runner's own parsed `hooks.json` — is
//! injected whole at open ([`App::open_hooks_menu`]), the `/resume` picker's
//! boundary seam. See `docs/hooks-menu.md`.

use super::*;

use crate::hooks::{HooksOverview, event_has_matchers};

/// The open `/hooks` menu (`None` on [`App`] when closed): the overview
/// snapshot it browses plus where the browse currently stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HooksMenu {
    /// The digested `hooks.json` — every modelled event, its matcher groups,
    /// their handlers ([`crate::hooks::HooksOverview`]).
    pub overview: HooksOverview,
    /// The display path of the file the `Source:` line names (`None` when no
    /// config home resolved).
    pub source: Option<String>,
    /// Whether the session actually runs these hooks — `false` shows the
    /// disabled note under the header (`/settings` off, `ALTER_ZERO_HOOKS=0`).
    pub enabled: bool,
    /// Where the browse stands — Claude Code's four-mode state machine.
    pub level: HooksLevel,
}

/// The menu's level stack, one frame per drill-down. Esc pops one frame;
/// indices reach into the overview (`events[event]`,
/// `events[event].matchers[matcher]`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HooksLevel {
    /// Level 1: the eleven events.
    Events { selected: usize },
    /// Level 2: one event's matcher rows (only for an event whose dispatch
    /// matches on something — [`crate::hooks::event_has_matchers`]).
    Matchers { event: usize, selected: usize },
    /// Level 3: the handlers under one matcher row (`matcher: Some`), or —
    /// for a matcher-less event — every handler of the event flattened
    /// (`matcher: None`).
    Hooks {
        event: usize,
        matcher: Option<usize>,
        selected: usize,
    },
    /// Level 4: one handler's read-only details. No selection — Esc is the
    /// only way out.
    Detail {
        event: usize,
        matcher: Option<usize>,
        hook: usize,
    },
}

impl HooksMenu {
    /// The selectable rows at the current level (0 on the detail page, and
    /// for an event with nothing configured — the empty state).
    #[must_use]
    pub fn row_count(&self) -> usize {
        match self.level {
            HooksLevel::Events { .. } => self.overview.events.len(),
            HooksLevel::Matchers { event, .. } => self
                .overview
                .events
                .get(event)
                .map_or(0, |e| e.matchers.len()),
            HooksLevel::Hooks { event, matcher, .. } => self
                .overview
                .events
                .get(event)
                .map_or(0, |e| e.hooks_at(matcher).len()),
            HooksLevel::Detail { .. } => 0,
        }
    }

    /// The highlighted row at the current level (`None` on the detail page).
    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        match self.level {
            HooksLevel::Events { selected }
            | HooksLevel::Matchers { selected, .. }
            | HooksLevel::Hooks { selected, .. } => Some(selected),
            HooksLevel::Detail { .. } => None,
        }
    }

    /// Move the highlight to `index`, clamped to the rows (a no-op on the
    /// detail page and in an empty state).
    fn select(&mut self, index: usize) {
        let last = match self.row_count() {
            0 => return,
            rows => rows - 1,
        };
        match &mut self.level {
            HooksLevel::Events { selected }
            | HooksLevel::Matchers { selected, .. }
            | HooksLevel::Hooks { selected, .. } => *selected = index.min(last),
            HooksLevel::Detail { .. } => {}
        }
    }

    /// Enter: descend one level from the highlighted row. An event whose
    /// dispatch matches on nothing skips the matcher level (Claude Code's
    /// rule); a row-less level (the empty state, the detail page) is a no-op.
    fn activate(&mut self) {
        match self.level {
            HooksLevel::Events { selected } => {
                let Some(event) = self.overview.events.get(selected) else {
                    return;
                };
                self.level = if event_has_matchers(event.event) {
                    HooksLevel::Matchers {
                        event: selected,
                        selected: 0,
                    }
                } else {
                    HooksLevel::Hooks {
                        event: selected,
                        matcher: None,
                        selected: 0,
                    }
                };
            }
            HooksLevel::Matchers { event, selected } => {
                if selected < self.row_count() {
                    self.level = HooksLevel::Hooks {
                        event,
                        matcher: Some(selected),
                        selected: 0,
                    };
                }
            }
            HooksLevel::Hooks {
                event,
                matcher,
                selected,
            } => {
                if selected < self.row_count() {
                    self.level = HooksLevel::Detail {
                        event,
                        matcher,
                        hook: selected,
                    };
                }
            }
            HooksLevel::Detail { .. } => {}
        }
    }

    /// Esc: pop one frame, restoring the parent's selection. `false` when
    /// already at the events level — the caller closes the menu.
    fn back(&mut self) -> bool {
        match self.level {
            HooksLevel::Events { .. } => false,
            HooksLevel::Matchers { event, .. } => {
                self.level = HooksLevel::Events { selected: event };
                true
            }
            HooksLevel::Hooks { event, matcher, .. } => {
                self.level = match matcher {
                    Some(selected) => HooksLevel::Matchers { event, selected },
                    // A matcher-less event descended straight from its row.
                    None => HooksLevel::Events { selected: event },
                };
                true
            }
            HooksLevel::Detail {
                event,
                matcher,
                hook,
            } => {
                self.level = HooksLevel::Hooks {
                    event,
                    matcher,
                    selected: hook,
                };
                true
            }
        }
    }
}

impl App {
    /// Open the `/hooks` menu over the boundary-supplied digest (the
    /// `/resume` picker's injection seam: the pure command returns the
    /// intent, the loop derives the data from the live `HookSetup`).
    /// Abandons the bands that share the composer, like every picker.
    pub fn open_hooks_menu(
        &mut self,
        overview: HooksOverview,
        source: Option<String>,
        enabled: bool,
    ) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.hooks_menu = Some(HooksMenu {
            overview,
            source,
            enabled,
            level: HooksLevel::Events { selected: 0 },
        });
    }

    /// Dismiss the menu (Esc from the events level, or Ctrl+C): the composer
    /// returns. No view change — it was never an overlay.
    pub fn close_hooks_menu(&mut self) {
        self.hooks_menu = None;
    }

    /// Keys while the `/hooks` menu is open. Owns **every** key (routed at
    /// the top of [`on_key`]): ↑/↓ move wrapping at the ends, Home/End jump,
    /// digits 1–9 jump-activate, Enter descends, Esc ascends (closing from
    /// the top), Ctrl+C closes.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_hooks(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the menu (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_hooks_menu();
            return Action::CloseHooksMenu;
        }
        let Some(menu) = self.hooks_menu.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => {
                let selected = menu.selected().unwrap_or(0);
                menu.select(wrap_step(selected, menu.row_count(), -1));
            }
            KeyCode::Down => {
                let selected = menu.selected().unwrap_or(0);
                menu.select(wrap_step(selected, menu.row_count(), 1));
            }
            KeyCode::Home => menu.select(0),
            KeyCode::End => menu.select(usize::MAX),
            KeyCode::Enter => menu.activate(),
            // Digits jump-activate their absolute row (the ask modal's rule
            // — Claude Code's Select selects on a number key). A digit past
            // the rows names nothing and is ignored.
            KeyCode::Char(c @ '1'..='9')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let index = (c as usize) - ('1' as usize);
                if index < menu.row_count() {
                    menu.select(index);
                    menu.activate();
                }
            }
            KeyCode::Esc => {
                if !menu.back() {
                    self.close_hooks_menu();
                    return Action::CloseHooksMenu;
                }
            }
            _ => {}
        }
        Action::None
    }
}

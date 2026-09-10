//! The `$` skill-mention picker's [`App`](super::App) state: the codex-style
//! popup that fuzzy-matches the discovered skills while the cursor sits in a
//! `$mention`, and the Tab/Enter accept that completes it in place.
//! See `docs/skill-mentions.md`.

use super::*;

use crate::skills::{MentionToken, SkillMatch, SkillMetadata, mention_token};

/// The open `$` skill picker (when the cursor is in a usable `$mention`);
/// `None` when closed. The palette's shape, not the file picker's: the
/// matches derive **synchronously** from `App::skills` and the live token
/// on demand ([`App::skill_matches`]) — skills are already discovered, so
/// there is no async round-trip to store — and only the highlight (plus the
/// query it was for, so a narrowed filter resets it) is state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillPicker {
    /// Index of the highlighted match within the current derived matches.
    pub selected: usize,
    /// The mention query the highlight is for (reset-on-change guard).
    pub query: String,
}

impl App {
    /// Inject the skills the composer may mention (called at the boundary
    /// beside the `<system-reminder>` listing render — `set_listings`'s
    /// sibling, so the picker and the listing can never disagree about what
    /// is offered). The **enabled** skills only: a disabled skill is one the
    /// registry's lookup refuses, so advertising it here would complete a
    /// mention that can only fail. Empty (no skills, skills off, tools off)
    /// leaves `$` an ordinary dollar sign.
    pub fn set_skills(&mut self, skills: Vec<SkillMetadata>) {
        self.skills = skills;
        if self.skills.is_empty() {
            self.skill_picker = None;
        }
    }

    /// The usable `$mention` under the cursor: the token itself
    /// ([`crate::skills::mention_token`]), unless the session has no skills
    /// to offer, the composer is in `!` shell mode (a `$VAR` there is real
    /// shell syntax), or the query is shell-flavored prose
    /// ([`crate::skills::shell_flavored_query`] — `$1`, `$PATH`).
    fn usable_mention(&self) -> Option<MentionToken> {
        if self.shell_mode || self.skills.is_empty() {
            return None;
        }
        let token = mention_token(self.input.text(), self.input.cursor())?;
        (!crate::skills::shell_flavored_query(&token.query)).then_some(token)
    }

    /// Is the cursor currently in a usable `$mention`? (The `had_mention`
    /// state the editing arms snapshot *before* a change, so
    /// [`refresh_skill_picker`]'s Esc-sticky logic mirrors the `@` picker's
    /// `had_token`.)
    ///
    /// [`refresh_skill_picker`]: App::refresh_skill_picker
    pub(super) fn in_skill_mention(&self) -> bool {
        self.usable_mention().is_some()
    }

    /// Re-derive the `$` picker after an edit — [`refresh_file_search`]'s
    /// logic over [`mention_token`]: open when the cursor *enters* a usable
    /// mention (the None→Some transition, so an Esc-dismiss stays dismissed
    /// within the same mention), reset the highlight when the query changes,
    /// close when the mention is gone (the terminator typed, the draft
    /// consumed, shell mode entered).
    ///
    /// [`refresh_file_search`]: App::refresh_file_search
    pub(super) fn refresh_skill_picker(&mut self, had_mention: bool) {
        match self.usable_mention() {
            None => self.skill_picker = None,
            Some(token) => match &mut self.skill_picker {
                Some(picker) if picker.query != token.query => {
                    picker.query = token.query;
                    picker.selected = 0;
                }
                Some(_) => {}
                // Just entered a usable mention → open at the top.
                None if !had_mention => {
                    self.skill_picker = Some(SkillPicker {
                        selected: 0,
                        query: token.query,
                    });
                }
                // Dismissed earlier and still in the same mention → stay closed.
                None => {}
            },
        }
    }

    /// Is the band actually **showing** — the picker open *and* the cursor
    /// still in a usable mention? The rows derive from the live token, so a
    /// plain cursor move out of the mention (no edit ran, the state
    /// lingers) already hides the band and releases the navigation keys.
    #[must_use]
    pub fn skill_band_active(&self) -> bool {
        self.skill_picker.is_some() && self.usable_mention().is_some()
    }

    /// The ranked matches for the live mention query — empty when the picker
    /// is closed or the cursor has left the mention. Derived on each call
    /// ([`crate::skills::rank_skills`] over the injected skills), the
    /// palette's `matching_commands` pattern.
    #[must_use]
    pub fn skill_matches(&self) -> Vec<SkillMatch> {
        if self.skill_picker.is_none() {
            return Vec::new();
        }
        self.usable_mention()
            .map(|token| crate::skills::rank_skills(&token.query, &self.skills))
            .unwrap_or_default()
    }

    /// The skill currently highlighted in the picker, if one is.
    #[must_use]
    pub fn highlighted_skill_match(&self) -> Option<SkillMatch> {
        let picker = self.skill_picker.as_ref()?;
        self.skill_matches().into_iter().nth(picker.selected)
    }

    /// Move the picker highlight one step over the current matches, wrapping
    /// at the ends (`wrap_step` — the palette's grammar).
    pub(super) fn move_skill_selection(&mut self, delta: isize) {
        let len = self.skill_matches().len();
        if let Some(picker) = &mut self.skill_picker {
            picker.selected = wrap_step(picker.selected, len, delta);
        }
    }

    /// Accept the highlighted skill: replace the `$mention` under the cursor
    /// with `$name` **keeping the sigil** (codex inserts the mention, not the
    /// bare name — the `$` is what marks it in the submitted text), plus a
    /// separating space. An accept mid-sentence **reuses** the space already
    /// following the mention instead of doubling it (codex's
    /// `advance_past_completion_separator`), the cursor landing past the
    /// separator either way, ready to keep typing. `Action::None` if nothing
    /// is highlighted.
    pub(super) fn accept_skill_selection(&mut self) -> Action {
        let Some(name) = self.highlighted_skill_match().map(|m| m.name) else {
            return Action::None;
        };
        if let Some(token) = self.usable_mention() {
            let mention = format!("{}{name}", crate::skills::SKILL_MENTION_PREFIX);
            if self.input.text()[token.range.end..].starts_with(' ') {
                self.input.replace_range(token.range, &mention);
                self.input.move_right(); // hop the existing separator
            } else {
                self.input
                    .replace_range(token.range, &format!("{mention} "));
            }
        }
        self.skill_picker = None;
        Action::None
    }
}

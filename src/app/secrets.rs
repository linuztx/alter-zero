//! The `/secrets` page: the user's secrets listed by placeholder, their
//! values masked, and the form that adds or edits one. See
//! `docs/secrets.md`.
//!
//! A composer-replacing page like `/donate`, but one that takes text: the
//! form's three fields are its own — never [`App::input`], which a modal
//! can stash and a large paste collapses — and the value lives in a
//! [`SecretValue`], whose `Debug` says `<redacted>`. What the page lists is
//! [`SecretMeta`]s injected by the boundary, so the pure core never holds a
//! stored value at all: editing a secret starts its value field empty.

use super::*;
use crate::secrets::{
    MAX_SECRET_CONTEXT_CHARS, MAX_SECRET_NAME_LEN, SecretDraft, SecretMeta, SecretValue,
    normalize_name, normalize_name_char, validate_draft,
};
use crate::textarea::TextArea;

/// Which field of the form has the focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SecretField {
    /// The name the placeholder carries.
    #[default]
    Name,
    /// The value — masked.
    Value,
    /// The line of context the agent is told.
    Context,
}

impl SecretField {
    /// The field after this one — Tab, ↓ and Enter's order, wrapping.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Value,
            Self::Value => Self::Context,
            Self::Context => Self::Name,
        }
    }

    /// The field before this one — Shift+Tab and ↑, wrapping.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Name => Self::Context,
            Self::Value => Self::Name,
            Self::Context => Self::Value,
        }
    }
}

/// The add/edit form.
#[derive(Debug, Clone, Default)]
pub struct SecretForm {
    /// The secret being edited — `None` adds a new one.
    pub original: Option<String>,
    /// The field keys go to.
    pub focus: SecretField,
    /// The name, normalized as it is typed.
    pub name: TextArea,
    /// The value typed so far — masked wherever it shows, and never loaded
    /// from a stored secret: an edit left empty keeps the stored value.
    pub value: SecretValue,
    /// The context line.
    pub context: TextArea,
    /// Why the last Enter was refused, shown in red under the fields.
    pub error: Option<String>,
}

impl SecretForm {
    /// The form over an existing secret: its name and context filled in,
    /// its value **not** — the page never holds a stored value.
    fn edit(meta: &SecretMeta) -> Self {
        Self {
            original: Some(meta.name.clone()),
            name: TextArea::from_text(&meta.name),
            context: TextArea::from_text(&meta.context),
            ..Self::default()
        }
    }

    /// The draft the form submits: an edit whose value was left empty keeps
    /// the stored one (`value: None`).
    #[must_use]
    pub fn draft(&self) -> SecretDraft {
        let keep = self.original.is_some() && self.value.is_empty();
        SecretDraft {
            original: self.original.clone(),
            name: self.name.text().to_string(),
            value: (!keep).then(|| self.value.clone()),
            context: self.context.text().trim().to_string(),
        }
    }

    /// Type `c` into the focused field — the name normalized
    /// ([`normalize_name_char`]) and capped, the context capped.
    fn type_char(&mut self, c: char) {
        match self.focus {
            SecretField::Name => {
                if let Some(c) = normalize_name_char(c)
                    && self.name.text().chars().count() < MAX_SECRET_NAME_LEN
                {
                    self.name.insert_char(c);
                }
            }
            SecretField::Value => self.value.push(c),
            SecretField::Context => {
                if self.context.text().chars().count() < MAX_SECRET_CONTEXT_CHARS {
                    self.context.insert_char(c);
                }
            }
        }
    }

    /// Paste `text` into the focused field: the name normalized, the value
    /// whole (trimmed when stored), the context flattened to one line.
    fn paste(&mut self, text: &str) {
        match self.focus {
            SecretField::Name => {
                let room = MAX_SECRET_NAME_LEN.saturating_sub(self.name.text().chars().count());
                let name: String = normalize_name(text).chars().take(room).collect();
                self.name.insert_str(&name);
            }
            SecretField::Value => self.value.push_str(text),
            SecretField::Context => {
                let room =
                    MAX_SECRET_CONTEXT_CHARS.saturating_sub(self.context.text().chars().count());
                let line: String = text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(room)
                    .collect();
                self.context.insert_str(&line);
            }
        }
    }

    /// Backspace in the focused field.
    fn backspace(&mut self) {
        match self.focus {
            SecretField::Name => self.name.delete_backward(),
            SecretField::Value => {
                self.value.pop();
            }
            SecretField::Context => self.context.delete_backward(),
        }
    }

    /// Ctrl+U: empty the focused field.
    fn clear_focused(&mut self) {
        match self.focus {
            SecretField::Name => self.name.clear(),
            SecretField::Value => self.value.clear(),
            SecretField::Context => self.context.clear(),
        }
    }

    /// The focused text field's editor — `None` on the value, which takes no
    /// cursor motion (a mask has nothing to move through).
    fn focused_text(&mut self) -> Option<&mut TextArea> {
        match self.focus {
            SecretField::Name => Some(&mut self.name),
            SecretField::Value => None,
            SecretField::Context => Some(&mut self.context),
        }
    }
}

/// The open page (`None` on [`App`] when closed).
#[derive(Debug, Clone, Default)]
pub struct SecretsPage {
    /// The highlighted row: a secret's index, or one past the last for the
    /// `+ Add a secret` row.
    pub selected: usize,
    /// The secret a first `d` asked to delete; a second `d` confirms.
    pub confirm_delete: Option<String>,
    /// The form, while adding or editing.
    pub form: Option<SecretForm>,
}

impl App {
    /// The secrets the page lists — names and context, never a value.
    #[must_use]
    pub fn secret_metas(&self) -> &[SecretMeta] {
        &self.secret_metas
    }

    /// Inject the store's secrets (from the boundary, at startup and after
    /// every change), keeping an open page's selection on a row and dropping
    /// a pending delete whose secret is gone.
    pub fn set_secret_metas(&mut self, metas: Vec<SecretMeta>) {
        self.secret_metas = metas;
        let rows = self.secret_metas.len() + 1;
        if let Some(page) = &mut self.secrets_page {
            page.selected = page.selected.min(rows - 1);
            if page
                .confirm_delete
                .as_ref()
                .is_some_and(|name| !self.secret_metas.iter().any(|meta| meta.name == *name))
            {
                page.confirm_delete = None;
            }
        }
    }

    /// Open the `/secrets` page on its first row — the first secret, or the
    /// add row when there is none. Abandons any `?` band / palette / file
    /// picker (they share the composer the page takes over) — every picker's
    /// open rule.
    pub fn open_secrets_page(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.secrets_page = Some(SecretsPage::default());
    }

    /// Close the page — and forget any draft with it.
    pub fn close_secrets_page(&mut self) {
        self.secrets_page = None;
    }

    /// The boundary saved `name`: the form closes onto the list, the saved
    /// row highlighted.
    pub fn secret_saved(&mut self, name: &str) {
        let index = self.secret_metas.iter().position(|meta| meta.name == name);
        if let Some(page) = &mut self.secrets_page {
            page.form = None;
            page.confirm_delete = None;
            if let Some(index) = index {
                page.selected = index;
            }
        }
    }

    /// The boundary could not save: the draft stays, with the reason.
    pub fn secret_save_failed(&mut self, reason: &str) {
        if let Some(form) = self
            .secrets_page
            .as_mut()
            .and_then(|page| page.form.as_mut())
        {
            form.error = Some(reason.to_string());
        }
    }

    /// Keys while the page is open — it owns **every** one (routed at the
    /// top of [`App::on_key`]), Ctrl+O included, so nothing reaches the
    /// composer underneath. Ctrl+C closes from anywhere (never quits, the
    /// picker family's rule); the rest is the list's or the form's.
    pub(super) fn on_key_secrets_page(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_secrets_page();
            return Action::CloseSecretsPage;
        }
        let in_form = self
            .secrets_page
            .as_ref()
            .is_some_and(|page| page.form.is_some());
        if in_form {
            self.on_key_secret_form(key)
        } else {
            self.on_key_secret_list(key)
        }
    }

    /// The list: ↑/↓ wrap, Home/End jump, Enter edits (or adds, on the last
    /// row), `n` adds, `c` copies the placeholder, `d` (or Delete) asks and
    /// a second `d` deletes — any other key takes the question back — and
    /// Esc closes.
    fn on_key_secret_list(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.close_secrets_page();
            return Action::CloseSecretsPage;
        }
        let rows = self.secret_metas.len() + 1;
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(page) = self.secrets_page.as_mut() else {
            return Action::None;
        };
        let highlighted = self.secret_metas.get(page.selected);
        let asked = page.confirm_delete.take();
        match key.code {
            KeyCode::Up => page.selected = wrap_step(page.selected, rows, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, rows, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = rows - 1,
            KeyCode::Enter => {
                page.form = Some(highlighted.map_or_else(SecretForm::default, SecretForm::edit));
            }
            KeyCode::Char('n' | 'N' | 'a' | 'A') if plain => {
                page.form = Some(SecretForm::default());
            }
            KeyCode::Char('d' | 'D') | KeyCode::Delete if plain => {
                if let Some(meta) = highlighted {
                    if asked.as_deref() == Some(meta.name.as_str()) {
                        return Action::DeleteSecret(meta.name.clone());
                    }
                    page.confirm_delete = Some(meta.name.clone());
                }
            }
            KeyCode::Char('c' | 'C') if plain => {
                if let Some(meta) = highlighted {
                    return Action::CopySecretPlaceholder(meta.name.clone());
                }
            }
            _ => {}
        }
        Action::None
    }

    /// The form: typing goes to the focused field (the name normalized, the
    /// value masked), Tab/↓ and Shift+Tab/↑ move, Enter moves on and saves
    /// from the last field, Ctrl+U empties the field, Esc returns to the
    /// list. A draft the store would refuse never leaves: the reason shows
    /// under the fields and the focus goes to the field at fault.
    fn on_key_secret_form(&mut self, key: KeyEvent) -> Action {
        let names: Vec<String> = self
            .secret_metas
            .iter()
            .map(|meta| meta.name.clone())
            .collect();
        let Some(page) = self.secrets_page.as_mut() else {
            return Action::None;
        };
        if key.code == KeyCode::Esc {
            page.form = None;
            return Action::None;
        }
        let Some(form) = page.form.as_mut() else {
            return Action::None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.focus = form.focus.next(),
            KeyCode::BackTab | KeyCode::Up => form.focus = form.focus.prev(),
            KeyCode::Enter if form.focus != SecretField::Context => {
                form.focus = form.focus.next();
            }
            KeyCode::Enter => {
                let draft = form.draft();
                return match validate_draft(&draft, &names) {
                    Ok(()) => Action::SaveSecret(draft),
                    Err(error) => {
                        form.focus = if error.is_about_name() {
                            SecretField::Name
                        } else {
                            SecretField::Value
                        };
                        form.error = Some(error.to_string());
                        Action::None
                    }
                };
            }
            KeyCode::Char('u') if ctrl => {
                form.clear_focused();
                form.error = None;
            }
            KeyCode::Char(c) if plain => {
                form.type_char(c);
                form.error = None;
            }
            KeyCode::Backspace => {
                form.backspace();
                form.error = None;
            }
            KeyCode::Left => form
                .focused_text()
                .into_iter()
                .for_each(TextArea::move_left),
            KeyCode::Right => form
                .focused_text()
                .into_iter()
                .for_each(TextArea::move_right),
            KeyCode::Home => form
                .focused_text()
                .into_iter()
                .for_each(TextArea::move_home),
            KeyCode::End => form.focused_text().into_iter().for_each(TextArea::move_end),
            KeyCode::Delete => {
                if let Some(text) = form.focused_text() {
                    text.delete_forward();
                    form.error = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    /// A bracketed paste while the page is open: into the focused field,
    /// never the composer underneath — a pasted value must not land in a
    /// message. On the list there is nothing to paste into.
    pub(super) fn paste_into_secrets_page(&mut self, pasted: &str) {
        if let Some(form) = self
            .secrets_page
            .as_mut()
            .and_then(|page| page.form.as_mut())
        {
            form.paste(pasted);
            form.error = None;
        }
    }
}

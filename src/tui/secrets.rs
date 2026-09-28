//! The `/secrets` page at the boundary (`docs/secrets.md`): the file under a
//! save or a delete, the reload when the page opens, the clipboard copy of a
//! placeholder, and the sync that keeps the page, the `<system-reminder>`
//! and the shared store agreeing.
//!
//! The pure side decided *what* — a validated [`SecretDraft`] rides
//! [`Action::SaveSecret`], a name rides the delete and the copy — and what
//! has to *happen* lives here. Every change is one read-modify-write of
//! `secrets.json` (`llm::keystore::update_secrets_file`: a process-wide
//! lock, owner-only, and a file that won't parse refused rather than
//! overwritten), and only a change the file took reaches the shared store —
//! so the executor, the reminder and the list never disagree with the disk.
//!
//! [`Action::SaveSecret`]: alter_zero::app::Action::SaveSecret

use alter_zero::app::ToastKind;
use alter_zero::clipboard;
use alter_zero::llm::keystore::{load_secrets_file, update_secrets_file};
use alter_zero::secrets::{SecretDraft, SecretStore, placeholder};

use super::{Session, config};

impl Session<'_> {
    /// A `secrets.json` that would not load at startup: a red toast, since
    /// the session then runs with no secrets — a placeholder the user
    /// expects to work would reach its tool unexpanded, never silently.
    pub(crate) fn report_secrets_error(&mut self, error: Option<String>) {
        if let Some(error) = error {
            self.toast(
                format!("{error} — no secrets this session"),
                ToastKind::Error,
            );
        }
    }

    /// Push what the page may know — names and context, never a value —
    /// into `App`, and re-render the reminder's listing sections, whose last
    /// section names the secrets (`Session::sync_listings`).
    pub(crate) fn sync_secrets(&mut self) {
        self.app.set_secret_metas(self.secrets.metas());
        self.sync_listings();
    }

    /// `/secrets` opened (the pure open already happened): reload the file,
    /// so a secret another session saved is listed and usable. A file that
    /// no longer parses keeps the store the session has, loudly.
    pub(crate) fn open_secrets_page(&mut self) {
        if let Some(path) = config::secrets_json_path() {
            match load_secrets_file(&path) {
                Ok(store) => self.secrets.replace(store),
                Err(error) => self.toast(error, ToastKind::Error),
            }
        }
        self.sync_secrets();
    }

    /// The form's Enter: apply the draft to the file, then to the shared
    /// store — the next tool call expands it, the next turn's reminder names
    /// it — and close the form onto the saved row. A refusal (the file won't
    /// parse, a name another session just took) keeps the draft open with
    /// the reason.
    pub(crate) fn save_secret(&mut self, draft: &SecretDraft) {
        let result = self.change_secrets(|store| store.apply(draft).map_err(|e| e.to_string()));
        match result {
            Ok(name) => {
                self.app.secret_saved(&name);
                self.toast(format!("Saved {}", placeholder(&name)), ToastKind::Info);
            }
            Err(error) => self.app.secret_save_failed(&error),
        }
    }

    /// A confirmed `d` on the list: remove the secret from the file and the
    /// store. A placeholder naming it is ordinary text from the next call on.
    pub(crate) fn delete_secret(&mut self, name: &str) {
        match self.change_secrets(|store| Ok(store.remove(name))) {
            Ok(_) => self.toast(format!("Deleted {}", placeholder(name)), ToastKind::Info),
            Err(error) => self.toast(error, ToastKind::Error),
        }
    }

    /// `c` on the list: the placeholder — never the value — to the
    /// clipboard, through `/copy`'s path (`docs/copy.md`), for pasting into
    /// a message. The page stays open.
    pub(crate) fn copy_secret_placeholder(&mut self, name: &str) {
        let text = placeholder(name);
        match clipboard::copy_to_clipboard(&text) {
            Ok(lease) => {
                self.clipboard_lease = lease;
                self.toast(format!("Copied {text} to clipboard"), ToastKind::Info);
            }
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }

    /// One change to the secrets: through the file when there is a config
    /// home (a read-modify-write, so two sessions never drop each other's
    /// secrets), in memory alone when there is none — then into the shared
    /// store, the page and the reminder. Nothing reaches the store unless
    /// the file took it.
    fn change_secrets<R>(
        &mut self,
        change: impl FnOnce(&mut SecretStore) -> Result<R, String>,
    ) -> Result<R, String> {
        let (store, result) = match config::secrets_json_path() {
            Some(path) => update_secrets_file(&path, change)?,
            None => {
                let mut store = self.secrets.snapshot();
                let result = change(&mut store)?;
                (store, result)
            }
        };
        self.secrets.replace(store);
        self.sync_secrets();
        Ok(result)
    }
}

//! The `/discord` page: the invite to the Alter Zero Discord server, and the
//! read-only inline page that shows it. See `docs/discord.md`.
//!
//! The `/sponsor` page's twin ([`super::sponsor`]) over one const invite:
//! **no text entry** and a single row, so copying and closing are the whole
//! grammar — the hardware cursor hides while its seat tracks the row's `❯`,
//! and every key is owned while the page is open. The invite is a const for
//! `/donate`'s reason: a page that read it from disk would be a page anyone
//! with write access to the config home could point at another server, and
//! a fake Discord invite is a phishing lure.

use super::*;

/// The invite to the Alter Zero Discord server: what the page's box shows,
/// what a click on it opens, and what Enter copies — verbatim. Bare ASCII
/// with no whitespace (the tests pin that).
pub const DISCORD_INVITE: &str = "https://discord.gg/EDBSpmq2fV";

/// The toast a copy confirms with. It names what was copied, like the
/// `/sponsor` page's (`Copied the sponsor link to clipboard`).
pub const DISCORD_COPIED_NOTICE: &str = "Copied the Discord invite link to clipboard";

/// The open `/discord` page's state (`None` on [`App`] when closed). Empty:
/// the invite is a const, and with one row there is no highlight to move
/// and no text to take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscordPage;

impl App {
    /// Open the `/discord` page. Abandons any `?` band / palette / file
    /// picker (they share the composer the page takes over) — every picker's
    /// open rule.
    pub fn open_discord_page(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.discord_page = Some(DiscordPage);
    }

    /// Dismiss the page (Esc, or Ctrl+C): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_discord_page(&mut self) {
        self.discord_page = None;
    }

    /// Keys while the `/discord` page is open — the `/sponsor` grammar over
    /// one row. Owns **every** key (routed at the top of [`on_key`]): Enter,
    /// `c` and the row's digit `1` copy the invite, Esc and Ctrl+C close,
    /// and the rest — ↑/↓ included, with no other row to reach — do
    /// nothing. Copying keeps the page open, so the toast lands with the
    /// invite still in view.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_discord_page(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_discord_page();
            return Action::CloseDiscordPage;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char('c' | 'C') => Action::CopyDiscordInvite,
            // The row is numbered like every row in the family, so its
            // digit copies it, as a digit does on `/sponsor`.
            KeyCode::Char('1')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Action::CopyDiscordInvite
            }
            KeyCode::Esc => {
                self.close_discord_page();
                Action::CloseDiscordPage
            }
            _ => Action::None,
        }
    }
}

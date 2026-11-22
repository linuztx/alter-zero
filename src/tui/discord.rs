//! The `/discord` page at the boundary: copying the invite
//! (`docs/discord.md`).
//!
//! The pure side already decided *that* to copy — Enter, `c` or `1` returns
//! [`Action::CopyDiscordInvite`], and there is only the one invite — and
//! what has to *happen* lives here: the clipboard write and the confirming
//! toast, the `/sponsor` page's `copy_sponsor_link` over `/copy`'s clipboard
//! path.
//!
//! [`Action::CopyDiscordInvite`]: alter_zero::app::Action::CopyDiscordInvite

use alter_zero::app::{DISCORD_COPIED_NOTICE, DISCORD_INVITE, ToastKind};
use alter_zero::clipboard;

use super::Session;

impl Session<'_> {
    /// Enter, `c`, or `1` on the `/discord` page: copy the invite, and say
    /// so. The page stays open either way — the toast lands above it with
    /// the invite still in view.
    pub(crate) fn copy_discord_invite(&mut self) {
        match clipboard::copy_to_clipboard(DISCORD_INVITE) {
            Ok(lease) => {
                self.clipboard_lease = lease;
                self.toast(DISCORD_COPIED_NOTICE, ToastKind::Info);
            }
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }
}

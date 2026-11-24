//! The `/sponsor` page at the boundary: copying a row's value
//! (`docs/sponsor.md`).
//!
//! The pure side already decided *what* to copy — the highlighted
//! [`SponsorLink`] rides [`Action::CopySponsorLink`] — and what has to
//! *happen* lives here: the clipboard write and the confirming toast, the
//! `/donate` page's `copy_donation_address` over `/copy`'s clipboard path.
//!
//! [`Action::CopySponsorLink`]: alter_zero::app::Action::CopySponsorLink

use alter_zero::app::{SponsorLink, ToastKind};

use super::Session;

impl Session<'_> {
    /// Enter, `c`, or a digit on the `/sponsor` page: copy the value, and
    /// say which one. The page stays open either way — the toast lands above
    /// it with the value still in view.
    pub(crate) fn copy_sponsor_link(&mut self, link: SponsorLink) {
        match self.copy_text(link.value) {
            Ok(()) => self.toast(
                format!("Copied the {} to clipboard", link.noun),
                ToastKind::Info,
            ),
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }
}

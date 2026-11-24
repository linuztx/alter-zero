//! The `/donate` page at the boundary: copying an address
//! (`docs/donate.md`).
//!
//! The pure side already decided *what* to copy — the highlighted
//! [`DonationAddress`] rides [`Action::CopyDonationAddress`] — and what has
//! to *happen* lives here: the clipboard write and the confirming toast, the
//! `/login` device page's `copy_device_code` pattern over `/copy`'s own
//! clipboard path.
//!
//! [`Action::CopyDonationAddress`]: alter_zero::app::Action::CopyDonationAddress

use alter_zero::app::{DonationAddress, ToastKind};

use super::Session;

impl Session<'_> {
    /// Enter, `c`, or a digit on the `/donate` page: copy the address, and
    /// say which one. The clipboard I/O is `/copy`'s (`Session::copy_text`,
    /// `docs/copy.md`); only the confirmation differs, because "Copied last
    /// message" would be a lie here. The page stays open either way — the
    /// toast lands above it with the address still in view.
    pub(crate) fn copy_donation_address(&mut self, address: DonationAddress) {
        match self.copy_text(address.address) {
            Ok(()) => self.toast(
                format!("Copied the {} address to clipboard", address.ticker),
                ToastKind::Info,
            ),
            Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
        }
    }
}

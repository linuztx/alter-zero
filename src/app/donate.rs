//! The `/donate` page: the project's crypto donation addresses, and the
//! read-only inline page that shows them. See `docs/donate.md`.
//!
//! The eleventh composer-replacing inline picker, and the `/hooks` browser's
//! sibling ([`super::hooks_menu`]): **no text entry** — navigation and copy
//! are the whole grammar — so there is no query, the hardware cursor hides
//! while its seat tracks the highlighted `❯` row, and every key is owned
//! while the page is open. The catalog is a const rather than a file: the
//! addresses are the project's, not the user's, and a page that read them
//! from disk would be a page anyone with write access to the config home
//! could redirect.

use super::*;

/// One donation address: the coin's ticker (`BTC`), its name (`Bitcoin`),
/// and the address itself — copied verbatim, so it is bare ASCII with no
/// whitespace (the catalog tests pin that). No network rides the entry:
/// every address is on its coin's own network, which the page's caution
/// says once for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DonationAddress {
    /// The ticker the row leads with and the copy toast names — one bare
    /// uppercase word.
    pub ticker: &'static str,
    /// The coin's full name, dim beside the ticker.
    pub coin: &'static str,
    /// The address, exactly as it is to be pasted into a wallet.
    pub address: &'static str,
}

/// The project's donation addresses, in the order the page lists them.
pub const DONATION_ADDRESSES: &[DonationAddress] = &[
    DonationAddress {
        ticker: "BTC",
        coin: "Bitcoin",
        address: "bc1q68v53mjj2uxg9qs5ke55qh4gv7un8esttwmvm9",
    },
    DonationAddress {
        ticker: "ETH",
        coin: "Ethereum",
        address: "0xaf7B6ac9BeeFDcfCd118701a00be960a592600CB",
    },
    DonationAddress {
        ticker: "SOL",
        coin: "Solana",
        address: "9hWaV4rTqNfF1c6mGDSnksMY1fqKuDU9iKymfbeSqXrA",
    },
];

/// The open `/donate` page's state (`None` on [`App`] when closed): which
/// address the `❯` sits on. Nothing else — the catalog is a const, and the
/// page takes no text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DonatePicker {
    /// Index of the highlighted address within [`DONATION_ADDRESSES`].
    pub selected: usize,
}

impl App {
    /// Open the `/donate` page, the highlight on the first address. Abandons
    /// any `?` band / palette / file picker (they share the composer the page
    /// takes over) — every picker's open rule.
    pub fn open_donate_picker(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.donate_picker = Some(DonatePicker::default());
    }

    /// Dismiss the page (Esc, or Ctrl+C): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_donate_picker(&mut self) {
        self.donate_picker = None;
    }

    /// The highlighted address, if the page is open.
    #[must_use]
    pub fn highlighted_donation(&self) -> Option<DonationAddress> {
        let picker = self.donate_picker.as_ref()?;
        DONATION_ADDRESSES.get(picker.selected).copied()
    }

    /// Keys while the `/donate` page is open — the `/hooks` grammar. Owns
    /// **every** key (routed at the top of [`on_key`]): ↑/↓ move wrapping at
    /// the ends, Home/End jump, digits 1–9 jump to their row **and copy it**,
    /// Enter and `c` copy the highlighted address, Esc and Ctrl+C close.
    /// Copying keeps the page open, so the confirming toast lands with the
    /// address still in view and a second address is one key away.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_donate_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_donate_picker();
            return Action::CloseDonatePicker;
        }
        let len = DONATION_ADDRESSES.len();
        let Some(picker) = self.donate_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = len.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('c' | 'C') => {
                if let Some(address) = DONATION_ADDRESSES.get(picker.selected) {
                    return Action::CopyDonationAddress(*address);
                }
            }
            // Digits jump to their absolute row and copy it (the ask modal's
            // and the `/hooks` menu's jump-activate rule). A digit past the
            // rows names nothing and is ignored.
            KeyCode::Char(c @ '1'..='9')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let index = (c as usize) - ('1' as usize);
                if let Some(address) = DONATION_ADDRESSES.get(index) {
                    picker.selected = index;
                    return Action::CopyDonationAddress(*address);
                }
            }
            KeyCode::Esc => {
                self.close_donate_picker();
                return Action::CloseDonatePicker;
            }
            _ => {}
        }
        Action::None
    }
}

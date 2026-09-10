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
/// the address itself — copied verbatim, so it is bare ASCII with no
/// whitespace (the catalog tests pin that) — and the **networks** it is
/// reachable on.
///
/// The networks are a list rather than a single name because one address
/// need not mean one chain: the EVM address is the same twenty bytes on
/// Ethereum, Linea, Base, Arbitrum, BNB Chain, OP and Polygon, and a page
/// that named only the first would leave a user guessing whether the other
/// six are safe — a guess whose wrong answer is unrecoverable. So the
/// entry states where its address may be sent and the page's caution says,
/// once, that nothing else may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DonationAddress {
    /// The ticker the row leads with and the copy toast names — one bare
    /// uppercase word.
    pub ticker: &'static str,
    /// The coin's full name, dim beside the ticker.
    pub coin: &'static str,
    /// The address, exactly as it is to be pasted into a wallet.
    pub address: &'static str,
    /// The networks this address is reachable on, in the order the page
    /// lists them — each a bare label as a wallet's own network picker
    /// spells it. Never empty (the catalog tests pin that): an address
    /// with no network named is one the caution cannot cover.
    pub networks: &'static [&'static str],
}

/// The project's donation addresses, in the order the page lists them.
pub const DONATION_ADDRESSES: &[DonationAddress] = &[
    DonationAddress {
        ticker: "BTC",
        coin: "Bitcoin",
        address: "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7",
        networks: &["Bitcoin (Native SegWit)"],
    },
    DonationAddress {
        ticker: "ETH",
        coin: "Ethereum",
        address: "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b",
        networks: &[
            "Ethereum",
            "Linea",
            "Base",
            "Arbitrum",
            "BNB Chain",
            "OP",
            "Polygon",
        ],
    },
    DonationAddress {
        ticker: "SOL",
        coin: "Solana",
        address: "Gwhv5c6uAa6aAz1MjwzV9QJpbm7CJWy2kuCeZ75mFc94",
        networks: &["Solana"],
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

//! The donation-address catalog and the `/donate` page (`docs/donate.md`).

use super::*;

use unicode_width::UnicodeWidthStr;

/// An app with the `/donate` page open.
fn donate_app() -> App {
    let mut app = App::new();
    app.open_donate_picker();
    app
}

/// The highlighted row's index.
fn selected(app: &App) -> usize {
    app.donate_picker
        .as_ref()
        .expect("the page is open")
        .selected
}

// ===== the catalog =====

#[test]
fn the_catalog_lists_btc_eth_then_sol_with_the_project_addresses() {
    let tickers: Vec<&str> = DONATION_ADDRESSES.iter().map(|a| a.ticker).collect();
    assert_eq!(tickers, ["BTC", "ETH", "SOL"]);
    let btc = DONATION_ADDRESSES[0];
    assert_eq!(btc.coin, "Bitcoin");
    assert_eq!(btc.address, "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7");
    let eth = DONATION_ADDRESSES[1];
    assert_eq!(eth.coin, "Ethereum");
    assert_eq!(eth.address, "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b");
    let sol = DONATION_ADDRESSES[2];
    assert_eq!(sol.coin, "Solana");
    assert_eq!(sol.address, "Gwhv5c6uAa6aAz1MjwzV9QJpbm7CJWy2kuCeZ75mFc94");
}

#[test]
fn every_entry_names_the_networks_its_address_is_reachable_on() {
    // The one question the page must answer beside the address itself:
    // *where* may this be sent? A wrong-network transfer is unrecoverable,
    // so an entry with no network named would be an entry the caution
    // cannot cover.
    let btc = DONATION_ADDRESSES[0];
    assert_eq!(btc.networks, ["Bitcoin (Native SegWit)"]);
    let eth = DONATION_ADDRESSES[1];
    assert_eq!(
        eth.networks,
        [
            "Ethereum",
            "Linea",
            "Base",
            "Arbitrum",
            "BNB Chain",
            "OP",
            "Polygon",
        ],
        "the one EVM address is reachable on every chain the wallet exposes"
    );
    let sol = DONATION_ADDRESSES[2];
    assert_eq!(sol.networks, ["Solana"]);
}

#[test]
fn every_network_name_is_a_bare_printable_label() {
    // The names are read off the page and typed into a wallet's network
    // picker: an empty one, a stray newline or a leading/trailing space
    // would render as a gap the reader has to guess at.
    for entry in DONATION_ADDRESSES {
        assert!(
            !entry.networks.is_empty(),
            "{}: an address with no network named",
            entry.ticker
        );
        for network in entry.networks {
            assert!(
                !network.trim().is_empty(),
                "{}: empty network name",
                entry.ticker
            );
            assert_eq!(
                *network,
                network.trim(),
                "{}: {network:?} carries padding",
                entry.ticker
            );
            assert!(
                !network.contains('\n'),
                "{}: {network:?} spans rows",
                entry.ticker
            );
        }
    }
}

#[test]
fn every_address_is_ascii_whitespace_free_and_single_width() {
    // An address is copied and transcribed verbatim: a stray space, a wide
    // glyph or a homoglyph would either break the box geometry or send the
    // funds nowhere. The tickers are the row labels — one bare uppercase
    // word each, distinct, so the toast can name what was copied.
    let mut seen = std::collections::HashSet::new();
    for entry in DONATION_ADDRESSES {
        assert!(!entry.address.is_empty(), "{}: empty address", entry.ticker);
        assert!(
            entry.address.is_ascii() && !entry.address.chars().any(char::is_whitespace),
            "{}: address must be bare ASCII: {:?}",
            entry.ticker,
            entry.address
        );
        assert_eq!(
            entry.address.width(),
            entry.address.chars().count(),
            "{}: single-width glyphs only",
            entry.ticker
        );
        assert!(
            !entry.ticker.is_empty() && entry.ticker.chars().all(|c| c.is_ascii_uppercase()),
            "{}: a ticker is one uppercase word",
            entry.ticker
        );
        assert!(
            seen.insert(entry.ticker),
            "{}: duplicate ticker",
            entry.ticker
        );
        assert!(
            !entry.coin.is_empty(),
            "{}: missing coin name",
            entry.ticker
        );
    }
}

// ===== the /donate command =====

#[test]
fn slash_donate_opens_the_page() {
    let mut app = App::new();
    type_chars(&mut app, "/donate");
    assert!(app.command_menu.is_some());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::OpenDonatePicker);
    assert!(app.donate_picker.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /donate token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
    assert_eq!(selected(&app), 0, "opens on the first address");
    assert_eq!(
        app.highlighted_donation(),
        Some(DONATION_ADDRESSES[0]),
        "the highlight is the first address"
    );
}

#[test]
fn the_palette_lists_donate_with_a_concise_description() {
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "donate")
        .expect("/donate is registered");
    assert_eq!(
        cmd.description,
        "Support the project with a crypto donation"
    );
    assert_eq!(cmd.effect, CommandEffect::Donate);
}

#[test]
fn slash_donate_works_mid_turn() {
    // Like every picker it only replaces the composer — the running turn
    // streams on its own thread and keeps its strip above the page.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/donate");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenDonatePicker);
    assert!(app.donate_picker.is_some());
    assert!(app.is_streaming(), "the turn was not touched");
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_donate_picker();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.donate_picker.is_some());
}

// ===== the key grammar (the /hooks family: navigation, no text entry) =====

#[test]
fn up_and_down_wrap_at_the_ends() {
    let mut app = donate_app();
    let last = DONATION_ADDRESSES.len() - 1;
    assert_eq!(selected(&app), 0);
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(selected(&app), 1);
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 2, "↓ reaches the third row");
    assert_eq!(selected(&app), last, "…which is the last one");
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 0, "↓ past the last wraps to the first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(selected(&app), last, "↑ from the first wraps to the last");
}

#[test]
fn home_and_end_jump() {
    let mut app = donate_app();
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), DONATION_ADDRESSES.len() - 1);
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), 0);
}

#[test]
fn enter_and_c_copy_the_highlighted_address_and_keep_the_page_open() {
    let mut app = donate_app();
    app.on_key(key(KeyCode::Down));
    let eth = DONATION_ADDRESSES[1];
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::CopyDonationAddress(eth)
    );
    assert!(app.donate_picker.is_some(), "copying keeps the page open");
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::CopyDonationAddress(eth)
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('C'))),
        Action::CopyDonationAddress(eth),
        "the shifted key copies too"
    );
    assert_eq!(selected(&app), 1, "the highlight stays put");
}

#[test]
fn a_digit_jumps_to_that_address_and_copies_it() {
    let mut app = donate_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::CopyDonationAddress(DONATION_ADDRESSES[1])
    );
    assert_eq!(selected(&app), 1, "the digit moved the highlight");
    assert_eq!(
        app.on_key(key(KeyCode::Char('1'))),
        Action::CopyDonationAddress(DONATION_ADDRESSES[0])
    );
    assert_eq!(selected(&app), 0);
    assert_eq!(
        app.on_key(key(KeyCode::Char('3'))),
        Action::CopyDonationAddress(DONATION_ADDRESSES[2]),
        "3 reaches the SOL row"
    );
    assert_eq!(selected(&app), 2);
    // A digit past the rows names nothing and is ignored.
    assert_eq!(app.on_key(key(KeyCode::Char('4'))), Action::None);
    assert_eq!(app.on_key(key(KeyCode::Char('9'))), Action::None);
    assert_eq!(selected(&app), 2);
}

#[test]
fn esc_and_ctrl_c_close_the_page_without_quitting() {
    let mut app = donate_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseDonatePicker);
    assert!(app.donate_picker.is_none());

    let mut app = donate_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseDonatePicker);
    assert!(
        app.donate_picker.is_none(),
        "Ctrl+C closes the page, never the app"
    );
}

#[test]
fn the_page_owns_every_other_key() {
    // No text entry: a printable key never reaches the composer draft
    // underneath, and the global Ctrl+O never opens the overlay over it.
    let mut app = donate_app();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char(' '),
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Left,
        KeyCode::Right,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
        assert!(app.donate_picker.is_some(), "{code:?} closed the page");
    }
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(app.view, View::Conversation, "Ctrl+O is owned while open");
    assert!(app.input.is_empty(), "nothing leaked into the composer");
    assert_eq!(selected(&app), 0);
}

#[test]
fn an_open_page_suppresses_the_ctrl_b_hint_clock_and_asks_no_animation() {
    // The running cell it keeps visible above itself must not advertise a
    // Ctrl+B the page would swallow (docs/background.md) — and the page is
    // still, so it never re-arms the animation chain on its own.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 100", None);
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_donate_picker();
    assert_eq!(
        app.background_hint_elapsed(),
        None,
        "the page swallows Ctrl+B"
    );

    let app = donate_app();
    assert!(
        !app.wants_animation_frames(),
        "a still page with no turn running asks for no frames"
    );
}

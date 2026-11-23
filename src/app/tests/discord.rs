//! The Discord invite and the `/discord` page (`docs/discord.md`).

use super::*;

/// An app with the `/discord` page open.
fn discord_app() -> App {
    let mut app = App::new();
    app.open_discord_page();
    app
}

// ===== the invite =====

#[test]
fn the_invite_is_the_servers_own_bare_url() {
    // A const the binary carries, never a file a stranger could edit: a
    // fake Discord invite is a phishing lure (docs/discord.md).
    assert_eq!(DISCORD_INVITE, "https://discord.gg/EDBSpmq2fV");
    // Copied verbatim and pasted into a browser or Discord itself: a stray
    // space or newline would break both.
    assert!(DISCORD_INVITE.is_ascii());
    assert!(!DISCORD_INVITE.chars().any(char::is_whitespace));
    assert!(DISCORD_INVITE.starts_with("https://discord.gg/"));
}

#[test]
fn the_copy_toast_says_what_was_copied() {
    assert_eq!(
        DISCORD_COPIED_NOTICE,
        "Copied the Discord invite link to clipboard"
    );
}

// ===== the /discord command =====

#[test]
fn slash_discord_opens_the_page() {
    let mut app = App::new();
    type_chars(&mut app, "/discord");
    assert!(app.command_menu.is_some());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenDiscordPage);
    assert!(app.discord_page.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /discord token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_palette_lists_discord_right_after_sponsor() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let sponsor = names
        .iter()
        .position(|n| *n == "sponsor")
        .expect("/sponsor");
    assert_eq!(names[sponsor + 1], "discord");
    let cmd = &COMMANDS[sponsor + 1];
    assert_eq!(cmd.description, "Join the community on Discord");
    assert_eq!(cmd.effect, CommandEffect::Discord);
}

#[test]
fn slash_discord_works_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/discord");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenDiscordPage);
    assert!(app.discord_page.is_some());
    assert!(app.is_streaming(), "the turn was not touched");
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_discord_page();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.skill_picker.is_none());
}

// ===== the key grammar (the /sponsor family over one row) =====

#[test]
fn enter_c_and_1_copy_the_invite_and_keep_the_page_open() {
    let mut app = discord_app();
    for code in [
        KeyCode::Enter,
        KeyCode::Char('c'),
        KeyCode::Char('C'),
        KeyCode::Char('1'),
    ] {
        assert_eq!(app.on_key(key(code)), Action::CopyDiscordInvite, "{code:?}");
        assert!(app.discord_page.is_some(), "{code:?} closed the page");
    }
}

#[test]
fn esc_and_ctrl_c_close_the_page_without_quitting() {
    let mut app = discord_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseDiscordPage);
    assert!(app.discord_page.is_none());

    let mut app = discord_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseDiscordPage);
    assert!(app.discord_page.is_none(), "Ctrl+C closes the page only");
}

#[test]
fn the_page_owns_every_other_key() {
    // One row: ↑/↓ have nowhere to go, and a digit past it names nothing.
    let mut app = discord_app();
    for code in [
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Char('2'),
        KeyCode::Char('x'),
        KeyCode::Char(' '),
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Left,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
        assert!(app.discord_page.is_some(), "{code:?} closed the page");
    }
    assert_eq!(app.on_key(alt(KeyCode::Char('1'))), Action::None);
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(app.view, View::Conversation, "Ctrl+O is owned while open");
    assert!(app.input.is_empty(), "nothing leaked into the composer");
}

#[test]
fn an_open_page_suppresses_the_ctrl_b_hint_clock_and_asks_no_animation() {
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 100", None);
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.background_hint_elapsed().is_some());
    app.open_discord_page();
    assert_eq!(app.background_hint_elapsed(), None);
    assert!(!discord_app().wants_animation_frames(), "a still page");
}

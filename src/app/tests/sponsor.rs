//! The sponsor catalog and the `/sponsor` page (`docs/sponsor.md`).

use super::*;

/// An app with the `/sponsor` page open.
fn sponsor_app() -> App {
    let mut app = App::new();
    app.open_sponsor_page();
    app
}

/// The highlighted row's index.
fn selected(app: &App) -> usize {
    app.sponsor_page
        .as_ref()
        .expect("the page is open")
        .selected
}

// ===== the catalog =====

#[test]
fn the_catalog_lists_the_sponsor_page_then_the_email() {
    let [github, email] = SPONSOR_LINKS else {
        panic!("two rows: {SPONSOR_LINKS:?}");
    };
    assert_eq!(github.label, "GitHub Sponsors");
    assert_eq!(github.value, "https://github.com/sponsors/linuztx");
    assert_eq!(github.target, github.value, "a URL opens itself");
    assert_eq!(email.label, "Email");
    assert_eq!(email.value, "linuztx@gmail.com");
    assert_eq!(
        email.target, "mailto:linuztx@gmail.com",
        "the address opens a message to itself"
    );
}

#[test]
fn every_value_is_bare_ascii_and_every_row_is_worded() {
    // A value is copied verbatim and pasted into a browser or a mail
    // client: a stray space or newline would break both.
    let mut nouns = std::collections::HashSet::new();
    for link in SPONSOR_LINKS {
        assert!(
            !link.value.is_empty()
                && link.value.is_ascii()
                && !link.value.chars().any(char::is_whitespace),
            "{}: {:?}",
            link.label,
            link.value
        );
        assert!(
            link.target.ends_with(link.value),
            "{}: the link opens what the box shows",
            link.label
        );
        assert!(!link.label.trim().is_empty() && !link.note.trim().is_empty());
        assert!(
            nouns.insert(link.noun),
            "{}: the toast must say which value was copied",
            link.label
        );
    }
}

// ===== the /sponsor command =====

#[test]
fn slash_sponsor_opens_the_page() {
    let mut app = App::new();
    type_chars(&mut app, "/sponsor");
    assert!(app.command_menu.is_some());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSponsorPage);
    assert!(app.sponsor_page.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /sponsor token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
    assert_eq!(app.highlighted_sponsor_link(), Some(SPONSOR_LINKS[0]));
}

#[test]
fn the_palette_lists_sponsor_right_after_donate() {
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name.as_ref()).collect();
    let donate = names.iter().position(|n| *n == "donate").expect("/donate");
    assert_eq!(names[donate + 1], "sponsor");
    let cmd = &COMMANDS[donate + 1];
    assert_eq!(cmd.description, "Sponsor the developer or get in touch");
    assert_eq!(cmd.effect, CommandEffect::Sponsor);
}

#[test]
fn slash_sponsor_works_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/sponsor");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSponsorPage);
    assert!(app.sponsor_page.is_some());
    assert!(app.is_streaming(), "the turn was not touched");
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_sponsor_page();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
}

// ===== the key grammar (the /donate family: navigation, no text entry) =====

#[test]
fn up_down_home_and_end_move_the_highlight_wrapping_at_the_ends() {
    let mut app = sponsor_app();
    let last = SPONSOR_LINKS.len() - 1;
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(selected(&app), 1);
    app.on_key(key(KeyCode::Down));
    assert_eq!(selected(&app), 0, "↓ past the last wraps to the first");
    app.on_key(key(KeyCode::Up));
    assert_eq!(selected(&app), last, "↑ from the first wraps to the last");
    app.on_key(key(KeyCode::Home));
    assert_eq!(selected(&app), 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(selected(&app), last);
}

#[test]
fn enter_and_c_copy_the_highlighted_value_and_keep_the_page_open() {
    let mut app = sponsor_app();
    let github = SPONSOR_LINKS[0];
    for code in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('C')] {
        assert_eq!(app.on_key(key(code)), Action::CopySponsorLink(github));
    }
    assert!(app.sponsor_page.is_some(), "copying keeps the page open");
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::CopySponsorLink(SPONSOR_LINKS[1])
    );
}

#[test]
fn a_digit_jumps_to_that_row_and_copies_it() {
    let mut app = sponsor_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('2'))),
        Action::CopySponsorLink(SPONSOR_LINKS[1])
    );
    assert_eq!(selected(&app), 1, "the digit moved the highlight");
    assert_eq!(
        app.on_key(key(KeyCode::Char('1'))),
        Action::CopySponsorLink(SPONSOR_LINKS[0])
    );
    // A digit past the rows names nothing and is ignored.
    assert_eq!(app.on_key(key(KeyCode::Char('3'))), Action::None);
    assert_eq!(selected(&app), 0);
}

#[test]
fn esc_and_ctrl_c_close_the_page_without_quitting() {
    let mut app = sponsor_app();
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseSponsorPage);
    assert!(app.sponsor_page.is_none());

    let mut app = sponsor_app();
    assert_eq!(app.on_key(ctrl('c')), Action::CloseSponsorPage);
    assert!(app.sponsor_page.is_none(), "Ctrl+C closes the page only");
}

#[test]
fn the_page_owns_every_other_key() {
    let mut app = sponsor_app();
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char(' '),
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Left,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
        assert!(app.sponsor_page.is_some(), "{code:?} closed the page");
    }
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
    app.open_sponsor_page();
    assert_eq!(app.background_hint_elapsed(), None);
    assert!(!sponsor_app().wants_animation_frames(), "a still page");
}

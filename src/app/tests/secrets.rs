//! The `/secrete` page: the list, the add/edit form and its key grammar
//! (`docs/secrets.md`).

use super::*;
use crate::secrets::{SecretDraft, SecretMeta, SecretValue};

fn meta(name: &str, context: &str) -> SecretMeta {
    SecretMeta {
        name: name.to_string(),
        context: context.to_string(),
    }
}

/// An app whose store holds `secrets`, the page open.
fn secrets_app(secrets: &[(&str, &str)]) -> App {
    let mut app = App::new();
    app.set_secret_metas(secrets.iter().map(|(n, c)| meta(n, c)).collect());
    app.open_secrets_page();
    app
}

fn page(app: &App) -> &SecretsPage {
    app.secrets_page.as_ref().expect("the page is open")
}

fn form(app: &App) -> &SecretForm {
    page(app).form.as_ref().expect("the form is open")
}

fn chars(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

// ===== the command =====

#[test]
fn slash_secrete_opens_the_page() {
    let mut app = App::new();
    type_chars(&mut app, "/secrete");
    assert!(app.command_menu.is_some());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSecretsPage);
    assert!(app.secrets_page.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /secrete token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_palette_lists_secrete_before_quit() {
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "secrete")
        .expect("/secrete is registered");
    assert_eq!(cmd.effect, CommandEffect::Secrets);
    assert!(cmd.description.len() <= 55, "{}", cmd.description);
    assert_eq!(COMMANDS.last().map(|c| c.name.as_ref()), Some("quit"));
}

#[test]
fn slash_secrete_works_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/secrete");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSecretsPage);
    assert!(app.secrets_page.is_some());
    assert!(app.is_streaming(), "the turn was not touched");
}

#[test]
fn opening_abandons_the_bands_that_share_the_composer() {
    let mut app = App::new();
    app.shortcuts_open = true;
    app.open_secrets_page();
    assert!(!app.shortcuts_open);
    assert!(app.command_menu.is_none());
    assert!(app.file_search.is_none());
    assert!(app.secrets_page.is_some());
}

// ===== the list =====

#[test]
fn the_list_is_every_secret_then_the_add_row_and_wraps() {
    let mut app = secrets_app(&[("A", ""), ("B", "")]);
    assert_eq!(page(&app).selected, 0);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(page(&app).selected, 2, "the add row closes the list");
    app.on_key(key(KeyCode::Down));
    assert_eq!(page(&app).selected, 0, "and wraps back to the top");
    app.on_key(key(KeyCode::Up));
    assert_eq!(page(&app).selected, 2);
    app.on_key(key(KeyCode::Home));
    assert_eq!(page(&app).selected, 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(page(&app).selected, 2);
}

#[test]
fn an_empty_store_opens_on_the_add_row() {
    let mut app = secrets_app(&[]);
    assert_eq!(page(&app).selected, 0);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let form = form(&app);
    assert_eq!(form.original, None);
    assert_eq!(form.focus, SecretField::Name);
}

#[test]
fn n_opens_a_new_secret_from_any_row() {
    let mut app = secrets_app(&[("A", "")]);
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(form(&app).original, None);
    assert!(form(&app).name.is_empty());
}

#[test]
fn a_shrinking_store_keeps_the_selection_on_the_page() {
    let mut app = secrets_app(&[("A", ""), ("B", ""), ("C", "")]);
    app.on_key(key(KeyCode::End));
    app.set_secret_metas(vec![meta("A", "")]);
    assert_eq!(page(&app).selected, 1, "clamped to the add row");
}

// ===== the form =====

#[test]
fn a_typed_name_normalizes_as_it_goes() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    chars(&mut app, "root password!");
    assert_eq!(form(&app).name.text(), "ROOT_PASSWORD");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).name.text(), "ROOT_PASSWOR");
}

#[test]
fn the_value_takes_every_character() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(form(&app).focus, SecretField::Value);
    chars(&mut app, "p4ss word!");
    assert_eq!(form(&app).value.expose(), "p4ss word!");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(form(&app).value.expose(), "p4ss word");
    app.on_key(ctrl('u'));
    assert!(form(&app).value.is_empty(), "Ctrl+U clears the field");
}

#[test]
fn tab_and_the_arrows_move_between_the_fields() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    assert_eq!(form(&app).focus, SecretField::Context);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(form(&app).focus, SecretField::Name, "Tab wraps");
    app.on_key(backtab());
    assert_eq!(form(&app).focus, SecretField::Context);
    app.on_key(key(KeyCode::Up));
    assert_eq!(form(&app).focus, SecretField::Value);
}

#[test]
fn enter_moves_on_then_saves_from_the_last_field() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    chars(&mut app, "TOKEN");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, SecretField::Value);
    chars(&mut app, "sk-live-1234");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(form(&app).focus, SecretField::Context);
    chars(&mut app, "The staging API key");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SaveSecret(SecretDraft {
            original: None,
            name: "TOKEN".into(),
            value: Some(SecretValue::new("sk-live-1234")),
            context: "The staging API key".into(),
        })
    );
    assert!(
        page(&app).form.is_some(),
        "the form waits for the boundary to confirm the save"
    );
}

#[test]
fn a_refused_draft_says_why_and_moves_to_the_field_at_fault() {
    let mut app = secrets_app(&[("TAKEN", "")]);
    app.on_key(key(KeyCode::Char('n')));
    chars(&mut app, "NEW");
    app.on_key(key(KeyCode::Tab));
    chars(&mut app, "abc");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        form(&app).error.as_deref(),
        Some("Values need at least 4 characters.")
    );
    assert_eq!(form(&app).focus, SecretField::Value);
    // Typing clears the complaint.
    chars(&mut app, "d");
    assert_eq!(form(&app).error, None);
    // A taken name sends the focus back to the name.
    app.on_key(key(KeyCode::BackTab));
    for _ in 0..3 {
        app.on_key(key(KeyCode::Backspace));
    }
    chars(&mut app, "TAKEN");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        form(&app).error.as_deref(),
        Some("<secrete:TAKEN> already exists.")
    );
    assert_eq!(form(&app).focus, SecretField::Name);
}

#[test]
fn editing_prefills_everything_but_the_value() {
    let mut app = secrets_app(&[("TOKEN", "The old context")]);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let form = form(&app);
    assert_eq!(form.original.as_deref(), Some("TOKEN"));
    assert_eq!(form.name.text(), "TOKEN");
    assert_eq!(form.context.text(), "The old context");
    assert!(form.value.is_empty(), "the value is never loaded back");
    // Saved untouched, the value is kept.
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SaveSecret(SecretDraft {
            original: Some("TOKEN".into()),
            name: "TOKEN".into(),
            value: None,
            context: "The old context".into(),
        })
    );
}

#[test]
fn a_confirmed_save_returns_to_the_list_on_the_saved_row() {
    let mut app = secrets_app(&[("A", "")]);
    app.on_key(key(KeyCode::Char('n')));
    app.set_secret_metas(vec![meta("A", ""), meta("B", "")]);
    app.secret_saved("B");
    assert!(page(&app).form.is_none());
    assert_eq!(page(&app).selected, 1);
}

#[test]
fn a_failed_save_keeps_the_draft_and_says_why() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    chars(&mut app, "TOKEN");
    app.secret_save_failed("secrets.json won't parse — fix or move it, then try again");
    assert_eq!(form(&app).name.text(), "TOKEN", "the draft survives");
    assert!(form(&app).error.as_deref().unwrap().contains("won't parse"));
}

#[test]
fn esc_backs_out_of_the_form_then_closes_the_page() {
    let mut app = secrets_app(&[("A", "")]);
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(page(&app).form.is_none(), "back to the list");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseSecretsPage);
    assert!(app.secrets_page.is_none());
}

#[test]
fn ctrl_c_closes_the_page_from_anywhere_and_never_quits() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.on_key(ctrl('c')), Action::CloseSecretsPage);
    assert!(app.secrets_page.is_none());
}

// ===== delete and copy =====

#[test]
fn d_asks_before_it_deletes() {
    let mut app = secrets_app(&[("TOKEN", "")]);
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), Action::None);
    assert_eq!(page(&app).confirm_delete.as_deref(), Some("TOKEN"));
    assert_eq!(
        app.on_key(key(KeyCode::Char('d'))),
        Action::DeleteSecret("TOKEN".into())
    );
    assert_eq!(page(&app).confirm_delete, None);
    // Any other key takes the question back.
    app.on_key(key(KeyCode::Delete));
    assert!(page(&app).confirm_delete.is_some(), "Delete asks too");
    app.on_key(key(KeyCode::Down));
    assert_eq!(page(&app).confirm_delete, None);
}

#[test]
fn nothing_to_delete_or_copy_on_the_add_row() {
    let mut app = secrets_app(&[]);
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), Action::None);
    assert_eq!(page(&app).confirm_delete, None);
    assert_eq!(app.on_key(key(KeyCode::Char('c'))), Action::None);
}

#[test]
fn c_copies_the_highlighted_placeholder() {
    let mut app = secrets_app(&[("TOKEN", "")]);
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Action::CopySecretPlaceholder("TOKEN".into())
    );
    assert!(app.secrets_page.is_some(), "the page stays open");
}

// ===== what the page owns =====

#[test]
fn the_page_owns_every_key_and_the_composer_is_untouched() {
    let mut app = App::new();
    type_str(&mut app, "draft in progress");
    app.open_secrets_page();
    for code in [KeyCode::Char('x'), KeyCode::Char('/'), KeyCode::Backspace] {
        app.on_key(key(code));
    }
    app.on_key(ctrl('o'));
    assert_eq!(
        app.view,
        View::Conversation,
        "Ctrl+O is the page's, not the overlay's"
    );
    app.on_key(key(KeyCode::Char('n')));
    chars(&mut app, "secret stuff");
    assert_eq!(app.input.text(), "draft in progress");
}

#[test]
fn a_paste_lands_in_the_focused_field_and_never_the_composer() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    app.on_paste("my api-key");
    assert_eq!(form(&app).name.text(), "MY_API_KEY");
    app.on_key(key(KeyCode::Tab));
    app.on_paste("sk-live-1234\n");
    assert_eq!(form(&app).value.expose(), "sk-live-1234\n");
    app.on_key(key(KeyCode::Tab));
    app.on_paste("line one\nline two");
    assert_eq!(form(&app).context.text(), "line one line two");
    assert!(app.input.is_empty(), "nothing reached the draft");
    assert!(app.command_menu.is_none());
}

#[test]
fn a_paste_on_the_list_goes_nowhere() {
    let mut app = secrets_app(&[("A", "")]);
    app.on_paste("sk-live-1234");
    assert!(app.input.is_empty());
    assert!(page(&app).form.is_none());
}

#[test]
fn the_value_never_reaches_a_debug_print() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Tab));
    chars(&mut app, "sk-live-1234");
    assert!(!format!("{:?}", app.secrets_page).contains("sk-live-1234"));
    app.on_key(key(KeyCode::Tab));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        !format!("{action:?}").contains("sk-live-1234"),
        "{action:?}"
    );
}

#[test]
fn closing_the_page_forgets_the_draft() {
    let mut app = secrets_app(&[]);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Tab));
    chars(&mut app, "sk-live-1234");
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc));
    app.open_secrets_page();
    assert!(page(&app).form.is_none());
}

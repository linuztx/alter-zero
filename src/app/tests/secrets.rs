//! The `/secrets` page: the list, the add/edit form and its key grammar —
//! and what the user types, which reaches the conversation with every stored
//! value replaced by its placeholder (`docs/secrets.md`).

use super::*;
use crate::ask::{AskDecision, AskOption, AskQuestion, AskRequest};
use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};
use crate::secrets::{SecretDraft, SecretMeta, SecretRegistry, SecretStore, SecretValue};

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
fn slash_secrets_opens_the_page() {
    let mut app = App::new();
    type_chars(&mut app, "/secrets");
    assert!(app.command_menu.is_some());
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenSecretsPage);
    assert!(app.secrets_page.is_some(), "the pure open happened");
    assert!(app.input.is_empty(), "the /secrets token was consumed");
    assert!(app.command_menu.is_none(), "the palette closed");
}

#[test]
fn the_palette_lists_secrets_before_quit() {
    let cmd = COMMANDS
        .iter()
        .find(|c| c.name == "secrets")
        .expect("/secrets is registered");
    assert_eq!(cmd.effect, CommandEffect::Secrets);
    assert!(cmd.description.len() <= 55, "{}", cmd.description);
    assert_eq!(COMMANDS.last().map(|c| c.name.as_ref()), Some("quit"));
}

#[test]
fn slash_secrets_works_mid_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("streaming…");
    type_chars(&mut app, "/secrets");
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
        Some("<secret:TAKEN> already exists.")
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

// ===== what the user types (docs/secrets.md *What the user types*) =====

/// `ROOT_PASSWORD`'s value — what the user types where they mean the secret.
const VALUE: &str = "hunter2-staging";

/// What the conversation keeps in its place.
const MASKED: &str = "<secret:ROOT_PASSWORD>";

/// An app the way the boundary leaves it: `secrets` stored, their redacting
/// half handed to `App` (`tui::bootstrap`'s `seed_app`).
fn masking_app(secrets: &[(&str, &str)]) -> App {
    let mut store = SecretStore::new();
    for (name, value) in secrets {
        store
            .apply(&SecretDraft {
                original: None,
                name: (*name).to_string(),
                value: Some(SecretValue::new(*value)),
                context: String::new(),
            })
            .expect("a valid secret");
    }
    let mut app = App::new();
    app.set_secret_redactor(SecretRegistry::new(store).redactor());
    app
}

/// [`masking_app`] storing `ROOT_PASSWORD` = [`VALUE`].
fn root_app() -> App {
    masking_app(&[("ROOT_PASSWORD", VALUE)])
}

#[test]
fn enter_sends_a_typed_value_as_its_placeholder() {
    let mut app = root_app();
    type_str(
        &mut app,
        &format!("log in as root, the password is {VALUE}"),
    );
    let sent = format!("log in as root, the password is {MASKED}");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit(sent.clone())
    );
    // The boundary's toast names what the send hid — once.
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
    assert!(app.take_masked_secrets().is_empty());
    // What history.jsonl keeps, and what ↑ brings back, is what was sent.
    assert_eq!(app.take_unpersisted_inputs(), std::slice::from_ref(&sent));
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), sent);
}

#[test]
fn a_placeholder_the_user_typed_goes_out_as_typed() {
    // `secret` is a common password, and a piece of every placeholder: the
    // one the user pasted from the page must survive it.
    let mut app = masking_app(&[("ROOT_PASSWORD", VALUE), ("DB_PASSWORD", "secret")]);
    let text = format!("use {MASKED} for root");
    type_str(&mut app, &text);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Submit(text));
    assert!(app.take_masked_secrets().is_empty(), "nothing was hidden");
}

#[test]
fn enter_mid_turn_steers_the_placeholder_into_the_running_turn() {
    let mut app = root_app();
    app.begin_stream();
    type_str(&mut app, &format!("sudo wants {VALUE}"));
    let sent = format!("sudo wants {MASKED}");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Steer(sent.clone()));
    assert_eq!(app.steered, [sent], "the row above the box shows it too");
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn tab_queues_the_placeholder_as_a_follow_up_turn() {
    let mut app = root_app();
    app.begin_stream();
    type_str(&mut app, &format!("then rotate {VALUE}"));
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    let queued = format!("then rotate {MASKED}");
    assert_eq!(app.queued.back(), Some(&batch(&[queued.as_str()])));
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn a_shell_command_is_recorded_with_the_placeholder() {
    // Its runner expands the placeholder back before the shell sees it
    // (`tui::shell`), so the command still runs as typed.
    let mut app = root_app();
    type_str(&mut app, &format!("!mysql -u root -p{VALUE}"));
    assert!(app.shell_mode);
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunShell(format!("mysql -u root -p{MASKED}"))
    );
    assert_eq!(
        app.take_unpersisted_inputs(),
        [format!("!mysql -u root -p{MASKED}")]
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
    // …and one queued behind a running `!` command the same way.
    app.begin_shell("sleep 5");
    type_str(&mut app, &format!("!echo {VALUE}"));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        app.queued.back(),
        Some(&QueuedTurn::Shell(format!("echo {MASKED}")))
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn a_subagents_chat_and_follow_up_carry_the_placeholder() {
    let mut app = root_app();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    type_str(&mut app, &format!("the key is {VALUE}"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::AgentChat {
            id: "a1".to_string(),
            text: format!("the key is {MASKED}"),
        }
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
    type_str(&mut app, &format!("then test {VALUE}"));
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(
        app.agent("a1").expect("the row").followups,
        [format!("then test {MASKED}")]
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn a_large_paste_is_spliced_back_before_it_is_redacted() {
    let mut app = root_app();
    let filler = "x".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD);
    app.on_paste(&format!("{filler}\nroot: {VALUE}"));
    assert!(
        app.input.text().starts_with("[Pasted Content"),
        "{}",
        app.input.text()
    );
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit(format!("{filler}\nroot: {MASKED}"))
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn a_cleared_draft_is_recalled_with_the_placeholder() {
    let mut app = root_app();
    type_str(&mut app, &format!("oops {VALUE}"));
    assert_eq!(app.on_key(ctrl('c')), Action::None);
    assert!(app.input.is_empty(), "the first Ctrl+C clears the draft");
    assert!(
        app.take_masked_secrets().is_empty(),
        "a cleared draft was never sent, so no toast says it was hidden"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input.text(), format!("oops {MASKED}"));
}

/// One single-select question; with `previews` its options carry the
/// preview that opens its notes field.
fn ask_request(previews: bool) -> AskRequest {
    let option = |label: &str| AskOption {
        label: label.to_string(),
        description: format!("{label} described"),
        preview: previews.then(|| format!("{label} preview")),
    };
    AskRequest {
        id: "ask_0".to_string(),
        questions: vec![AskQuestion {
            question: "Which login should I use?".to_string(),
            header: "Login".to_string(),
            options: vec![option("Key"), option("Agent"), option("None")],
            multi_select: false,
        }],
    }
}

/// The answers a resolving key hands the boundary.
fn submitted(action: Action) -> Vec<crate::ask::AskAnswer> {
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("expected the submission, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    answers
}

#[test]
fn an_answer_typed_into_a_question_carries_the_placeholder() {
    let mut app = root_app();
    app.open_ask(ask_request(false));
    app.on_key(key(KeyCode::Char('4'))); // the `Type something.` row
    assert!(app.ask().unwrap().editing());
    type_str(&mut app, &format!("use {VALUE}"));
    let answers = submitted(app.on_key(key(KeyCode::Enter)));
    assert_eq!(answers[0].labels, [format!("use {MASKED}")]);
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn a_note_typed_onto_a_question_carries_the_placeholder() {
    let mut app = root_app();
    app.open_ask(ask_request(true));
    app.on_key(key(KeyCode::Char('n')));
    type_str(&mut app, &format!("its passphrase is {VALUE}"));
    app.on_key(key(KeyCode::Esc));
    let note = format!("its passphrase is {MASKED}");
    assert_eq!(
        app.ask().unwrap().answers[0].notes,
        note,
        "kept, and shown again, redacted"
    );
    assert!(
        app.take_masked_secrets().is_empty(),
        "nothing is sent until the answer is"
    );
    let answers = submitted(app.on_key(key(KeyCode::Char('1'))));
    assert_eq!(answers[0].notes.as_deref(), Some(note.as_str()));
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn feedback_on_a_refused_call_carries_the_placeholder() {
    let mut app = root_app();
    let request = PermissionRequest {
        id: "p1".to_string(),
        kind: PermissionKind::Bash,
        target: "mysql -u root".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
        agent_id: None,
    };
    app.open_permission(request.clone());
    app.on_key(key(KeyCode::Tab));
    type_str(&mut app, &format!("pass -p{VALUE}"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ResolvePermission {
            request,
            decision: PermissionDecision::Deny(Some(format!("pass -p{MASKED}"))),
        }
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn text_from_outside_the_composer_is_redacted_the_same_way() {
    // The `[PROMPT]` given on the command line (`tui::turn`).
    let mut app = root_app();
    assert_eq!(
        app.redact_typed(format!("fix the login, it is {VALUE}")),
        format!("fix the login, it is {MASKED}")
    );
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn with_no_secrets_handed_over_what_the_user_types_is_untouched() {
    // The unit-test default, and a session that never stored a secret.
    let mut app = App::new();
    type_str(&mut app, &format!("the password is {VALUE}"));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit(format!("the password is {VALUE}"))
    );
    assert!(app.take_masked_secrets().is_empty());
}

#[test]
fn an_answer_kept_with_esc_is_announced_only_once_submitted() {
    // Esc keeps the typed answer without choosing it — nothing is sent, so
    // nothing is announced — yet the field already holds the placeholder,
    // the preview of what the agent will read.
    let mut app = root_app();
    app.open_ask(ask_request(false));
    app.on_key(key(KeyCode::Char('4')));
    type_str(&mut app, &format!("use {VALUE}"));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.ask().unwrap().answers[0].other, format!("use {MASKED}"));
    assert!(app.take_masked_secrets().is_empty());
    // Re-opened and accepted, the lone question resolves: now it is sent.
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.input.text(), format!("use {MASKED}"));
    let answers = submitted(app.on_key(key(KeyCode::Enter)));
    assert_eq!(answers[0].labels, [format!("use {MASKED}")]);
    assert_eq!(app.take_masked_secrets(), ["ROOT_PASSWORD"]);
}

#[test]
fn an_answer_edited_away_from_its_placeholder_announces_nothing() {
    let mut app = root_app();
    app.open_ask(ask_request(false));
    app.on_key(key(KeyCode::Char('4')));
    type_str(&mut app, &format!("use {VALUE}"));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('4')));
    app.on_key(ctrl('u'));
    type_str(&mut app, "use the agent");
    let answers = submitted(app.on_key(key(KeyCode::Enter)));
    assert_eq!(answers[0].labels, ["use the agent"]);
    assert!(
        app.take_masked_secrets().is_empty(),
        "nothing it sends was hidden"
    );
}

#[test]
fn a_declined_question_announces_nothing() {
    let mut app = root_app();
    app.open_ask(ask_request(false));
    app.on_key(key(KeyCode::Char('4')));
    type_str(&mut app, &format!("use {VALUE}"));
    app.on_key(key(KeyCode::Esc));
    let action = app.on_key(key(KeyCode::Esc));
    assert!(
        matches!(
            action,
            Action::ResolveAsk {
                decision: AskDecision::Declined,
                ..
            }
        ),
        "{action:?}"
    );
    assert!(app.take_masked_secrets().is_empty());
}

#[test]
fn a_question_that_times_out_announces_nothing() {
    // The user is away: an entry left open is no answer, and nobody is
    // there to read a toast.
    let mut app = root_app();
    app.open_ask(ask_request(false));
    app.on_key(key(KeyCode::Char('4')));
    type_str(&mut app, &format!("use {VALUE}"));
    let expired = app.expire_asks(std::time::Duration::from_secs(600));
    assert_eq!(expired.len(), 1);
    assert!(app.take_masked_secrets().is_empty());
}

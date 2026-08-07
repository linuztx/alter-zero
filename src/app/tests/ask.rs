//! The inline `AskUserQuestion` modal (`docs/ask.md`): opening stashes the
//! composer, the tab/row navigation, single- and multi-select answers, the
//! free-text Other entry, the notes field, the Submit page, and the queueing
//! against the permission prompt.

use super::*;
use crate::ask::{AskDecision, AskOption, AskQuestion, AskRequest};
use crate::permission::{PermissionKind, PermissionRequest};

fn option(label: &str) -> AskOption {
    AskOption {
        label: label.to_string(),
        description: format!("{label} described"),
        preview: None,
    }
}

fn question(text: &str, header: &str, labels: &[&str], multi: bool) -> AskQuestion {
    AskQuestion {
        question: text.to_string(),
        header: header.to_string(),
        options: labels.iter().map(|l| option(l)).collect(),
        multi_select: multi,
    }
}

fn coffee_question() -> AskQuestion {
    question(
        "What's your favorite way to drink coffee?",
        "Coffee style",
        &["Black", "Latte", "Cold brew"],
        false,
    )
}

fn topics_question() -> AskQuestion {
    question(
        "Which of these tool features would you like to see demoed next? (pick any number)",
        "Demo topics",
        &["Preview panel", "Custom 'Other' input", "4-option question"],
        true,
    )
}

fn preview_question() -> AskQuestion {
    AskQuestion {
        question: "Which code style do you prefer for a simple greeting function?".to_string(),
        header: "Code style".to_string(),
        options: vec![
            AskOption {
                label: "Arrow function".to_string(),
                description: "Modern and terse".to_string(),
                preview: Some("const greet = (name) => {}".to_string()),
            },
            AskOption {
                label: "Function declaration".to_string(),
                description: "Classic".to_string(),
                preview: Some("function greet(name) {}".to_string()),
            },
        ],
        multi_select: false,
    }
}

fn request(id: &str, questions: Vec<AskQuestion>) -> AskRequest {
    AskRequest {
        id: id.to_string(),
        questions,
    }
}

/// Type `text` into the app one key at a time (the real composer path).
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn a_request_replaces_the_composer_and_gives_the_draft_back_afterwards() {
    let mut app = App::new();
    type_text(&mut app, "half a thought");
    app.open_ask(request("ask_0", vec![coffee_question()]));
    assert!(app.ask().is_some());
    assert_eq!(
        app.input.text(),
        "",
        "the composer is cleared for the modal"
    );
    // Esc declines the whole call — the draft comes back with the resolution.
    let action = app.on_key(key(KeyCode::Esc));
    assert_eq!(
        action,
        Action::ResolveAsk {
            id: "ask_0".to_string(),
            decision: AskDecision::Declined,
        }
    );
    assert!(app.ask().is_none());
    assert_eq!(app.input.text(), "half a thought", "the draft came back");
}

#[test]
fn a_single_question_resolves_on_the_picked_option() {
    // A lone question has no Submit page: Enter on an option submits at once.
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    app.on_key(key(KeyCode::Down)); // ❯ on “Latte”
    let action = app.on_key(key(KeyCode::Enter));
    let Action::ResolveAsk { id, decision } = action else {
        panic!("expected a resolution, got {action:?}");
    };
    assert_eq!(id, "ask_0");
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers, got {decision:?}");
    };
    assert_eq!(answers.len(), 1);
    assert_eq!(
        answers[0].question,
        "What's your favorite way to drink coffee?"
    );
    assert_eq!(answers[0].labels, vec!["Latte".to_string()]);
}

#[test]
fn digits_jump_activate_the_numbered_rows() {
    // `1`..`3` are the options, `4` the Other row, `5` Chat about this.
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    let action = app.on_key(key(KeyCode::Char('1')));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("digit 1 picks the first option, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(answers[0].labels, vec!["Black".to_string()]);
    // `5` — Chat about this.
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    let action = app.on_key(key(KeyCode::Char('5')));
    assert_eq!(
        action,
        Action::ResolveAsk {
            id: "ask_0".to_string(),
            decision: AskDecision::Chat,
        }
    );
}

#[test]
fn answering_a_question_advances_to_the_next_tab_then_the_submit_page() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question(), topics_question()]));
    let prompt = app.ask().unwrap();
    assert_eq!(prompt.tab, 0);
    assert!(prompt.has_submit_tab());
    assert_eq!(prompt.tab_count(), 3);
    // Pick “Black” — the modal moves to the multi-select question.
    assert_eq!(app.on_key(key(KeyCode::Char('1'))), Action::None);
    assert_eq!(app.ask().unwrap().tab, 1);
    // Toggle two options; the page stays put.
    app.on_key(key(KeyCode::Char('1')));
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(app.ask().unwrap().tab, 1);
    let state = &app.ask().unwrap().answers[1];
    assert!(state.selected.contains(&0) && state.selected.contains(&1));
    // Toggling again clears one.
    app.on_key(key(KeyCode::Char('2')));
    assert!(!app.ask().unwrap().answers[1].selected.contains(&1));
    app.on_key(key(KeyCode::Char('2'))); // …and back on
    // The multi-select confirms via its own Submit row (unnumbered): ↑/↓ to
    // it and Enter. (The digit jumps above seated the `❯` on the toggled row,
    // so the walk starts from there.)
    let question = topics_question();
    let rows = ask_rows(&question);
    let confirm_at = rows.iter().position(|r| *r == AskRow::Confirm).unwrap();
    let at = app.ask().unwrap().row;
    for _ in 0..confirm_at.saturating_sub(at) {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    let prompt = app.ask().unwrap();
    assert!(prompt.on_submit_tab(), "the confirm advanced to Submit");
    // Submit page: Enter on “Submit answers” resolves with both questions.
    let action = app.on_key(key(KeyCode::Enter));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("expected the submission, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].labels, vec!["Black".to_string()]);
    assert_eq!(
        answers[1].labels,
        vec![
            "Preview panel".to_string(),
            "Custom 'Other' input".to_string()
        ]
    );
}

#[test]
fn arrows_and_tab_navigate_the_chip_strip() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question(), topics_question()]));
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.ask().unwrap().tab, 1);
    app.on_key(key(KeyCode::Right));
    assert!(app.ask().unwrap().on_submit_tab());
    app.on_key(key(KeyCode::Right)); // clamped at the end
    assert!(app.ask().unwrap().on_submit_tab());
    app.on_key(key(KeyCode::Left));
    assert_eq!(app.ask().unwrap().tab, 1);
    // Tab wraps forward; Shift+Tab (BackTab) steps back.
    app.on_key(key(KeyCode::Tab));
    assert!(app.ask().unwrap().on_submit_tab());
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.ask().unwrap().tab, 0, "Tab wraps past the last page");
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(
        app.ask().unwrap().tab,
        0,
        "BackTab clamps at the first page"
    );
}

#[test]
fn submitting_with_nothing_answered_walks_to_the_first_unanswered_question() {
    // “Handle if user submit but didn’t answer any question”: the Submit page
    // refuses an empty submission and moves to the question instead.
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question(), topics_question()]));
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    assert!(app.ask().unwrap().on_submit_tab());
    let action = app.on_key(key(KeyCode::Enter)); // “Submit answers”
    assert_eq!(action, Action::None, "nothing to submit yet");
    assert_eq!(app.ask().unwrap().tab, 0, "walked to the first unanswered");
    // A partial submission is fine: answer only the first question, then
    // submit from the review page — the answers carry just that one.
    app.on_key(key(KeyCode::Char('1')));
    app.on_key(key(KeyCode::Right)); // topics → Submit
    assert!(app.ask().unwrap().on_submit_tab());
    let action = app.on_key(key(KeyCode::Enter));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("expected the partial submission, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(answers.len(), 1, "only the answered question submits");
}

#[test]
fn the_submit_pages_cancel_row_declines() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question(), topics_question()]));
    app.on_key(key(KeyCode::Char('1'))); // answer, advance
    app.on_key(key(KeyCode::Right)); // → Submit page
    let action = app.on_key(key(KeyCode::Char('2'))); // Cancel
    assert_eq!(
        action,
        Action::ResolveAsk {
            id: "ask_0".to_string(),
            decision: AskDecision::Declined,
        }
    );
}

#[test]
fn the_other_row_takes_free_text_as_the_answer() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    // Row 4 = “Type something.” — opens the entry field.
    assert_eq!(app.on_key(key(KeyCode::Char('4'))), Action::None);
    assert!(app.ask().unwrap().editing());
    type_text(&mut app, "My answer");
    let action = app.on_key(key(KeyCode::Enter));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("a single question resolves on the accepted Other, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(answers[0].labels, vec!["My answer".to_string()]);
}

#[test]
fn escaping_the_other_entry_keeps_the_typed_text_without_choosing_it() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question(), topics_question()]));
    app.on_key(key(KeyCode::Char('4')));
    type_text(&mut app, "half typed");
    app.on_key(key(KeyCode::Esc));
    let prompt = app.ask().unwrap();
    assert!(!prompt.editing(), "Esc backs out to the option list");
    assert_eq!(prompt.answers[0].other, "half typed", "the text is kept");
    assert!(!prompt.answers[0].other_chosen, "…but not chosen");
    assert!(app.ask().is_some(), "the modal is still up");
    // Re-opening the field shows the kept draft.
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.input.text(), "half typed");
}

#[test]
fn a_multi_select_other_becomes_a_checked_entry() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![topics_question()]));
    app.on_key(key(KeyCode::Char('1'))); // toggle “Preview panel”
    app.on_key(key(KeyCode::Char('4'))); // the Other row
    type_text(&mut app, "My topic");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None, "stays open");
    let prompt = app.ask().unwrap();
    assert!(prompt.answers[0].other_chosen);
    // Confirm via the Submit row → the lone question resolves.
    let rows = ask_rows(&topics_question());
    let confirm_at = rows.iter().position(|r| *r == AskRow::Confirm).unwrap();
    let prompt = app.ask().unwrap();
    assert!(confirm_at >= prompt.row);
    for _ in 0..(confirm_at - prompt.row) {
        app.on_key(key(KeyCode::Down));
    }
    let action = app.on_key(key(KeyCode::Enter));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("expected the submission, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(
        answers[0].labels,
        vec!["Preview panel".to_string(), "My topic".to_string()]
    );
}

#[test]
fn notes_open_with_n_on_a_preview_question_and_ride_the_answer() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![preview_question()]));
    assert_eq!(app.on_key(key(KeyCode::Char('n'))), Action::None);
    assert!(app.ask().unwrap().editing(), "n opens the notes field");
    type_text(&mut app, "prefer terse");
    app.on_key(key(KeyCode::Esc)); // back to selecting, text kept
    assert_eq!(app.ask().unwrap().answers[0].notes, "prefer terse");
    let action = app.on_key(key(KeyCode::Char('1')));
    let Action::ResolveAsk { decision, .. } = action else {
        panic!("expected the submission, got {action:?}");
    };
    let AskDecision::Submitted(answers) = decision else {
        panic!("expected answers");
    };
    assert_eq!(answers[0].notes.as_deref(), Some("prefer terse"));
    assert_eq!(
        answers[0].preview.as_deref(),
        Some("const greet = (name) => {}"),
        "the picked option's preview rides the annotation"
    );
}

#[test]
fn n_is_inert_on_a_question_without_previews() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        !app.ask().unwrap().editing(),
        "no previews — no notes field"
    );
}

#[test]
fn an_ask_arriving_while_a_permission_prompt_is_open_queues_behind_it() {
    let mut app = App::new();
    app.open_permission(PermissionRequest {
        id: "perm_0".to_string(),
        kind: PermissionKind::Write,
        target: "a.py".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    });
    app.open_ask(request("ask_0", vec![coffee_question()]));
    assert!(app.permission().is_some(), "the prompt keeps the screen");
    assert!(app.ask().is_none(), "the ask waits its turn");
    // Answering the permission opens the queued ask.
    app.on_key(key(KeyCode::Char('1')));
    assert!(app.permission().is_none());
    assert!(app.ask().is_some(), "the queued ask opened");
}

#[test]
fn a_permission_arriving_while_the_ask_is_open_queues_behind_it() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    app.open_permission(PermissionRequest {
        id: "perm_0".to_string(),
        kind: PermissionKind::Write,
        target: "a.py".to_string(),
        body: String::new(),
        detail: None,
        agent: None,
    });
    assert!(app.ask().is_some(), "the modal keeps the screen");
    assert!(app.permission().is_none());
    // Resolving the ask opens the queued permission.
    app.on_key(key(KeyCode::Char('1')));
    assert!(app.ask().is_none());
    assert!(app.permission().is_some(), "the queued permission opened");
}

#[test]
fn clearing_abandons_the_open_and_queued_asks() {
    let mut app = App::new();
    app.open_ask(request("ask_0", vec![coffee_question()]));
    app.open_ask(request("ask_1", vec![coffee_question()])); // queues
    app.clear_conversation();
    assert!(app.ask().is_none());
    let abandoned = app.take_abandoned_asks();
    assert!(
        abandoned.contains(&"ask_0".to_string()),
        "got {abandoned:?}"
    );
    assert!(
        abandoned.contains(&"ask_1".to_string()),
        "got {abandoned:?}"
    );
    assert!(app.take_abandoned_asks().is_empty(), "the drain empties");
}

#[test]
fn ctrl_c_on_the_modal_declines_like_esc() {
    let mut app = App::new();
    type_text(&mut app, "draft");
    app.open_ask(request("ask_0", vec![coffee_question()]));
    let action = app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(
        action,
        Action::ResolveAsk {
            id: "ask_0".to_string(),
            decision: AskDecision::Declined,
        }
    );
    assert_eq!(app.input.text(), "draft", "the draft survives");
}

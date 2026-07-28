//! The `/resume` session picker (`docs/resume.md`).

use super::*;

// --- session info (the footer under the box; see docs/footer.md) ---

#[test]
fn session_info_is_unset_by_default() {
    // The unit-test default: no footer until the I/O boundary injects the
    // display strings (the set_clock pattern).
    assert!(App::new().session.is_none());
}

#[test]
fn set_session_info_stores_the_display_strings() {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/alter-zero");
    let session = app.session.as_ref().expect("session info stored");
    assert_eq!(session.model, "dummy_model_name");
    assert_eq!(session.cwd, "~/alter-zero");
}

#[test]
fn resume_at_seats_arrow_browsing_at_the_entry() {
    let mut history = history_of(&["oldest", "middle", "newest"]);
    history.resume_at(1);
    // Browsing resumes as if "middle" had just been recalled: ↑ steps to
    // the entry older than it (codex's shared history cursor on accept).
    assert!(history.should_navigate("middle", "middle".len()));
    assert_eq!(history.up(), Some("oldest".to_string()));
}

#[test]
fn slash_resume_runs_to_open_the_picker_when_idle() {
    let mut app = App::new();
    type_chars(&mut app, "/resume");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenResumePicker);
    assert!(app.input.is_empty(), "running a command clears the draft");
}

#[test]
fn open_resume_picker_enters_the_view_and_disarms_a_primed_backtrack() {
    let mut app = App::new();
    app.backtrack.primed = true;
    app.open_resume_picker(vec![summary("a.jsonl", "hello")], "/repo".into());
    assert_eq!(app.view, View::ResumePicker);
    assert!(!app.backtrack.primed, "any view swap disarms the gesture");
    let picker = app.resume_picker.as_ref().expect("picker state is open");
    assert_eq!(picker.selected, 0);
    assert!(picker.query.is_empty());
    // Codex's defaults: filter Cwd, sort Updated, focus on the Filter tab.
    assert_eq!(picker.filter, ResumeFilter::Cwd);
    assert_eq!(picker.sort, ResumeSort::Updated);
    assert_eq!(picker.focus, ResumeControl::Filter);
}

#[test]
fn the_cwd_filter_hides_other_directories_until_toggled_to_all() {
    // Codex's Filter: [Cwd] All — the default mode lists only sessions
    // recorded in the picker's own cwd; → on the focused Filter control
    // switches to All (and back), reseating the selection.
    let mut app = picker_app(&[("a", "here one"), ("b", "here two")]);
    {
        let picker = app.resume_picker.as_mut().unwrap();
        picker.sessions.push(crate::session::SessionSummary {
            path: PathBuf::from("c"),
            updated_secs: 10,
            created_secs: 10,
            cwd: "/elsewhere".into(),
            preview: "other repo".into(),
        });
    }
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.matches().len(), 2, "Cwd default hides /elsewhere");
    app.on_key(key(KeyCode::Down)); // move off the top
    app.on_key(key(KeyCode::Right)); // toggle the focused Filter control
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.filter, ResumeFilter::All);
    assert_eq!(picker.matches().len(), 3, "All shows every session");
    assert_eq!(picker.selected, 0, "a mode change reseats the selection");
    app.on_key(key(KeyCode::Left)); // toggle back
    assert_eq!(
        app.resume_picker.as_ref().unwrap().filter,
        ResumeFilter::Cwd
    );
}

#[test]
fn interrupt_undo_stops_at_a_resumed_sessions_trailing_user_message() {
    // A rollout can end with a user message (quit mid-turn before any
    // reply); resuming installs it as the history tail. A NEW submission
    // undone by an early Esc must pull back only its own message — not
    // merge the resumed conversation's tail into the composer and drop it
    // from history.
    let mut app = App::new();
    app.record_user_message("old");
    let items = std::mem::take(&mut app.history);
    app.load_session(items);
    app.record_user_message("new");
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "new");
    assert_eq!(roles(&app), vec![Role::User], "the resumed tail survives");
    assert_eq!(message_at(&app, 0).text, "old");
}

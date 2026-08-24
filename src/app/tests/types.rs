//! The conversation's value types and their small behaviours.

use super::*;

#[test]
fn record_user_message_appends_to_history() {
    let mut app = App::new();
    app.record_user_message("hello");
    assert_eq!(app.history.len(), 1);
    assert_eq!(message_at(&app, 0).role, Role::User);
    assert_eq!(message_at(&app, 0).text, "hello");
}

#[test]
fn show_toast_holds_the_text_and_kind_without_recording_history() {
    let mut app = App::new();
    app.show_toast("Copied last message to clipboard", ToastKind::Info);
    let toast = app.toast().expect("a toast is live");
    assert_eq!(toast.text, "Copied last message to clipboard");
    assert_eq!(toast.kind, ToastKind::Info);
    assert!(
        app.history.is_empty(),
        "a toast is UI, never a conversation entry"
    );
}

#[test]
fn show_toast_overwrites_the_previous_one() {
    let mut app = App::new();
    app.show_toast("first", ToastKind::Info);
    app.show_toast("second", ToastKind::Error);
    let toast = app.toast().expect("a toast is live");
    assert_eq!(toast.text, "second");
    assert_eq!(toast.kind, ToastKind::Error);
}

#[test]
fn clear_toast_removes_it() {
    let mut app = App::new();
    app.show_toast("hi", ToastKind::Info);
    app.clear_toast();
    assert!(app.toast().is_none());
}

#[test]
fn clear_conversation_drops_a_live_toast() {
    let mut app = App::new();
    app.show_toast("Copied", ToastKind::Info);
    app.clear_conversation();
    assert!(app.toast().is_none(), "a cleared slate shows nothing");
}

#[test]
fn record_system_message_appends_a_system_role_message() {
    let mut app = App::new();
    app.record_system_message("a notice");
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Message(Message {
            role: Role::System,
            text: "a notice".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }))
    );
}

#[test]
fn set_clock_stamps_every_recorded_message_and_tool() {
    let mut app = App::new();
    app.set_clock(|| STAMP.to_string());

    app.record_user_message("hi");
    app.begin_stream();
    app.push_chunk("answer");
    app.finish_stream();
    app.start_tool("Read", "f", None);
    app.end_tool("out", true);

    assert_eq!(app.history.len(), 3, "user, assistant, tool");
    for item in &app.history {
        let ts = match item {
            HistoryItem::Message(m) => &m.timestamp,
            HistoryItem::Tool(t) => &t.timestamp,
            HistoryItem::Summary(s) => &s.timestamp,
            HistoryItem::Background(n) => &n.timestamp,
            HistoryItem::AgentGroup(g) => &g.timestamp,
            HistoryItem::AgentNotice(n) => &n.timestamp,
            HistoryItem::Compaction(c) => &c.timestamp,
            HistoryItem::Reasoning(r) => &r.timestamp,
            HistoryItem::TaskCall(t) => &t.timestamp,
            HistoryItem::HookNote(n) => &n.timestamp,
        };
        assert_eq!(ts, STAMP, "every recorded item carries the clock's stamp");
    }
}

#[test]
fn without_a_clock_recorded_timestamps_are_empty() {
    // The pure default used by unit tests: no clock injected → empty stamp,
    // so existing equality assertions on Message/ToolCall still hold.
    let mut app = App::new();
    app.record_user_message("hi");
    assert_eq!(message_at(&app, 0).timestamp, "");
}

#[test]
fn count_tokens_is_zero_for_empty_and_grows_with_length() {
    assert_eq!(count_tokens(""), 0);
    assert!(count_tokens("a") >= 1);
    assert!(count_tokens("a much longer string") > count_tokens("a"));
}

#[test]
fn esc_clears_the_query_first_and_closes_second() {
    let mut app = picker_app(&[("a", "hello")]);
    type_chars(&mut app, "zzz");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.view, View::ResumePicker, "first Esc only clears");
    assert!(app.resume_picker.as_ref().unwrap().query.is_empty());
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseResumePicker);
    assert_eq!(app.view, View::Conversation);
    assert!(app.resume_picker.is_none());
}

#[test]
fn load_session_installs_history_and_returns_to_the_conversation() {
    let mut app = picker_app(&[("a", "hello")]);
    // Belt-and-braces: leftovers from a dead turn must not survive the
    // swap (a turn can't be *active* — /resume is rejected mid-task).
    app.begin_stream();
    app.push_chunk("partial");
    let items = vec![
        HistoryItem::Message(Message {
            role: Role::User,
            text: "hello".into(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Message(Message {
            role: Role::Assistant,
            text: "hi".into(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
    ];
    app.load_session(items.clone());
    assert_eq!(app.history, items);
    assert_eq!(app.view, View::Conversation);
    assert!(app.resume_picker.is_none());
    assert!(!app.is_streaming(), "no stream survives the swap");
    assert!(app.status().is_none(), "no status survives the swap");
}

// ===== wrap_step (the shared ↑/↓ selection step) =====

#[test]
fn wrap_step_steps_and_wraps_at_both_ends() {
    assert_eq!(wrap_step(0, 3, 1), 1, "an ordinary step");
    assert_eq!(
        wrap_step(2, 3, 1),
        0,
        "down from the last wraps to the first"
    );
    assert_eq!(
        wrap_step(0, 3, -1),
        2,
        "up from the first wraps to the last"
    );
    assert_eq!(wrap_step(0, 1, 1), 0, "a lone row wraps onto itself");
    assert_eq!(wrap_step(0, 1, -1), 0);
    assert_eq!(wrap_step(0, 0, 1), 0, "an empty list pins the selection");
    assert_eq!(wrap_step(3, 0, -1), 0);
    assert_eq!(
        wrap_step(9, 3, 1),
        0,
        "a stale index steps from the last real row"
    );
    assert_eq!(wrap_step(9, 3, -1), 1);
}

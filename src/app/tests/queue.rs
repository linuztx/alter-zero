//! The mid-turn message queue (`docs/queue.md`).

use super::*;

#[test]
fn record_queues_the_entry_for_persistence() {
    let mut history = InputHistory::default();
    history.record("git status");
    history.record("cargo test");
    assert_eq!(
        history.take_unpersisted(),
        ["git status", "cargo test"],
        "each genuine append queues its text for the boundary to flush"
    );
    assert!(
        history.take_unpersisted().is_empty(),
        "draining twice yields nothing the second time"
    );
}

#[test]
fn blank_and_duplicate_records_queue_nothing() {
    let mut history = InputHistory::default();
    history.record("");
    history.record("dup");
    history.record("dup"); // adjacent duplicate, collapsed
    assert_eq!(
        history.take_unpersisted(),
        ["dup"],
        "only the one genuine append is queued (blanks + dups are not)"
    );
}

#[test]
fn record_ephemeral_records_but_never_queues_for_disk() {
    let mut history = InputHistory::default();
    history.record_ephemeral("cleared draft");
    assert_eq!(
        history.up(),
        Some("cleared draft".to_string()),
        "an ephemerally-recorded draft still recalls this session"
    );
    assert!(
        history.take_unpersisted().is_empty(),
        "but it is never queued for the persistent file (codex parity)"
    );
}

#[test]
fn submitted_and_queued_inputs_are_persisted() {
    let mut app = App::new();
    submit(&mut app, "sent message");
    assert_eq!(
        app.take_unpersisted_inputs(),
        ["sent message"],
        "an idle submit persists its text"
    );
}

#[test]
fn interrupt_turn_keeps_the_notice_when_a_message_is_queued() {
    // With follow-ups queued the user wants them sent, so Esc interrupts
    // normally (notice committed) rather than undoing — even with no output.
    let mut app = App::new();
    app.record_user_message("Hi");
    app.begin_stream();
    app.queued.push_back(QueuedTurn::Shell("ls".to_string()));
    let outcome = app.interrupt_turn().expect("a turn was active");
    assert!(
        matches!(
            outcome,
            InterruptedTurn::Kept {
                notice: Some(_),
                ..
            }
        ),
        "a non-empty queue opts out of the undo"
    );
    assert!(app.input.text().is_empty(), "the composer is untouched");
    assert!(
        roles(&app).contains(&Role::Error),
        "the interrupt notice is recorded"
    );
}

#[test]
fn a_full_batch_runs_in_order_leaving_three_history_tools_and_an_empty_queue() {
    // Drive the whole batch: each call flips to Running then finishes, in
    // order, committing three tools; the live queue ends empty.
    let mut app = App::new();
    app.start_tool_batch(&ping_batch());
    for item in ping_batch() {
        app.start_tool(&item.name, &item.args, "");
        app.end_tool("done", true);
    }
    assert!(app.tool_queue().is_empty(), "the batch fully drained");
    let tool_args: Vec<&str> = app
        .history
        .iter()
        .filter_map(|i| match i {
            HistoryItem::Tool(t) => Some(t.args.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_args,
        vec!["ping google.com", "ping facebook.com", "ping x.com"],
        "all three committed, in request order"
    );
}

#[test]
fn enter_when_idle_submits_and_never_queues() {
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("hello".to_string())
    );
    assert!(
        app.queued.is_empty(),
        "an idle submit doesn't touch the queue"
    );
}

#[test]
fn drain_next_batch_takes_the_front_batch_in_order_and_empties() {
    // A batch (the Enters that share a turn) flushes whole as the next turn.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("a");
    app.on_key(key(KeyCode::Enter));
    app.input = TextArea::from_text("b");
    app.on_key(key(KeyCode::Enter));
    app.input = TextArea::from_text("c");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.drain_next_batch(),
        Some(batch(&["a", "b", "c"])),
        "the whole front batch, FIFO"
    );
    assert!(app.queued.is_empty(), "the queue is emptied");
    assert!(app.drain_next_batch().is_none(), "nothing left to flush");
}

#[test]
fn tab_mid_turn_opens_a_new_follow_up_batch() {
    // Enter accumulates into the current batch; Tab starts a *new* batch so
    // its message runs as its own follow-up turn after the first queue.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("first");
    app.on_key(key(KeyCode::Enter)); // batch 1
    app.input = TextArea::from_text("follow");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(app.input.text(), "", "Tab consumes the composer like Enter");
    assert_eq!(app.queued.len(), 2, "Tab opened a second batch");
    assert_eq!(app.queued[0], batch(&["first"]));
    assert_eq!(app.queued[1], batch(&["follow"]));
}

#[test]
fn tab_when_idle_does_not_queue() {
    // Tab only queues while a turn streams; idle it's a no-op (you queue
    // follow-ups against a *running* turn).
    let mut app = App::new();
    app.input = TextArea::from_text("hello");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert!(app.queued.is_empty(), "idle Tab queues nothing");
    assert_eq!(app.input.text(), "hello", "and leaves the draft intact");
}

#[test]
fn alt_up_pulls_only_the_last_batch_into_the_composer_for_editing() {
    // Alt+Up edits the *last* turn-batch (codex's edit_queued_message
    // pop_back), not the whole backlog: with an Enter batch then a Tab
    // batch, Alt+Up yanks back only the Tab batch — the earlier Enter batch
    // stays queued, untouched.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("hello");
    app.on_key(key(KeyCode::Enter)); // batch 1 = [hello]
    app.input = TextArea::from_text("world");
    app.on_key(key(KeyCode::Enter)); // batch 1 = [hello, world]
    app.input = TextArea::from_text("deploy");
    app.on_key(key(KeyCode::Tab)); // batch 2 = [deploy]
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(
        app.input.text(),
        "deploy",
        "only the last batch returns — not hello/world"
    );
    assert_eq!(
        app.input.cursor(),
        "deploy".len(),
        "the cursor lands at the end, ready to edit"
    );
    assert_eq!(app.queued.len(), 1, "the earlier batch stays queued");
    assert_eq!(
        app.queued[0],
        batch(&["hello", "world"]),
        "and is left untouched"
    );
}

#[test]
fn alt_up_concats_the_last_batchs_messages() {
    // The last batch can itself hold several messages (a Tab opened it, an
    // Enter extended it): Alt+Up returns them newline-joined, oldest first.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("deploy");
    app.on_key(key(KeyCode::Tab)); // batch 1 = [deploy]
    app.input = TextArea::from_text("rollback");
    app.on_key(key(KeyCode::Enter)); // batch 1 = [deploy, rollback]
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(
        app.input.text(),
        "deploy\nrollback",
        "the last batch's messages return newline-joined, oldest first"
    );
    assert_eq!(
        app.input.cursor(),
        "deploy\nrollback".len(),
        "the cursor lands at the end, ready to edit"
    );
    assert!(
        app.queued.is_empty(),
        "popping the only batch empties the queue"
    );
}

#[test]
fn alt_up_on_an_empty_queue_is_harmless() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "");
    assert!(app.queued.is_empty());
}

#[test]
fn clear_mid_turn_drops_the_queued_backlog() {
    // The backlog belonged to the conversation being wiped — flushing it
    // as the next turn would resurrect what `/clear` just removed.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("queued follow-up");
    app.on_key(key(KeyCode::Enter)); // a turn is in flight — this queues
    assert_eq!(app.queued.len(), 1, "the message queued");
    type_str(&mut app, "/clear");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert!(app.drain_next_batch().is_none(), "the backlog was wiped");
}

#[test]
fn a_bang_command_mid_turn_queues_as_a_standalone_shell_entry() {
    // codex parity (submit_queued_shell_prompt): a !command typed while a
    // turn streams queues as its own Shell entry — run locally when its turn
    // comes — NOT concatenated into a text batch and sent to the backend as
    // literal text (the old v1 limitation). The full `!command` is recorded
    // for ↑ recall, and submitting exits the mode.
    let mut app = App::new();
    app.begin_stream();
    type_query(&mut app, "!echo hi");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(!app.shell_mode, "submitting exits shell mode");
    assert_eq!(
        app.drain_next_batch(),
        Some(QueuedTurn::Shell("echo hi".to_string())),
        "queued as a local shell command, not literal text"
    );
    assert_eq!(
        app.input_history.up(),
        Some("!echo hi".to_string()),
        "the full !command is recorded for ↑ recall"
    );
}

#[test]
fn a_queued_shell_entry_is_never_merged_with_text() {
    // A Shell entry stands alone: an Enter-text after it opens a *fresh*
    // Messages batch (the back is a Shell, not a Messages), so the command
    // never gets concatenated into a text turn — codex's per-completion
    // shell dispatch ("cannot be added to").
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("hello");
    app.on_key(key(KeyCode::Enter)); // Messages(["hello"])
    type_query(&mut app, "!ls");
    app.on_key(key(KeyCode::Enter)); // Shell("ls") — standalone
    app.input = TextArea::from_text("world");
    app.on_key(key(KeyCode::Enter)); // a NEW Messages(["world"]), not merged
    assert_eq!(
        app.queued,
        VecDeque::from(vec![
            batch(&["hello"]),
            QueuedTurn::Shell("ls".to_string()),
            batch(&["world"]),
        ]),
        "the shell entry stands alone between the two text batches"
    );
}

#[test]
fn two_mid_turn_shell_commands_queue_as_separate_entries() {
    // Each !command is individual: two of them mid-turn become two Shell
    // entries (each its own local run), never one merged blob.
    let mut app = App::new();
    app.begin_stream();
    type_query(&mut app, "!one");
    app.on_key(key(KeyCode::Enter));
    type_query(&mut app, "!two");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.queued,
        VecDeque::from(vec![
            QueuedTurn::Shell("one".to_string()),
            QueuedTurn::Shell("two".to_string()),
        ])
    );
}

#[test]
fn alt_up_pulls_a_queued_shell_entry_back_into_shell_mode() {
    // The user chose: Alt+Up over a queued !command yanks it back into the
    // composer *re-entering shell mode* (the red `! ` prompt), ready to
    // edit/re-run — recalling `!command` re-absorbs the bang.
    let mut app = App::new();
    app.begin_stream();
    type_query(&mut app, "!deploy --prod");
    app.on_key(key(KeyCode::Enter)); // Shell("deploy --prod")
    assert_eq!(app.queued.len(), 1);
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert!(app.shell_mode, "Alt+Up re-enters shell mode");
    assert_eq!(
        app.input.text(),
        "deploy --prod",
        "the command returns with its bang absorbed into the mode"
    );
    assert!(
        app.queued.is_empty(),
        "the entry was pulled out of the queue"
    );
}

//! The mid-turn message queue (`docs/queue.md`).
//!
//! Two halves: what Enter hands to the **running** turn (steering — delivered
//! at its next round boundary) and what Tab / a `!` command leave for a
//! **follow-up** turn (the classic queue, drained at turn end).

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
        app.start_tool(&item.name, &item.args, None);
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
    // A batch flushes whole as the next turn. Three Enters against a turn
    // that ended before reading any of them is how a multi-message batch
    // forms now (the reclaim).
    let mut app = App::new();
    app.begin_stream();
    for text in ["a", "b", "c"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Enter));
    }
    app.reclaim_steered();
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
    // Tab is the *other* mid-turn intent (Enter steers into the running
    // turn): each Tab starts its own batch, so its message runs as a separate
    // follow-up turn after the ones already queued.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("first");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(app.input.text(), "", "Tab consumes the composer like Enter");
    app.input = TextArea::from_text("follow");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.queued.len(), 2, "each Tab opened its own batch");
    assert_eq!(app.queued[0], batch(&["first"]));
    assert_eq!(app.queued[1], batch(&["follow"]));
    assert!(app.steered.is_empty(), "Tab never steers the running turn");
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
    // pop_back), not the whole backlog: with a reclaimed batch then a Tab
    // batch, Alt+Up yanks back only the Tab batch — the earlier one stays
    // queued, untouched.
    let mut app = App::new();
    app.begin_stream();
    for text in ["hello", "world"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Enter));
    }
    app.reclaim_steered(); // batch 1 = [hello, world]
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
    // A batch can itself hold several messages (a turn ended before reading
    // the Enters steered into it): Alt+Up returns them newline-joined,
    // oldest first.
    let mut app = App::new();
    app.begin_stream();
    for text in ["deploy", "rollback"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Enter));
    }
    app.reclaim_steered(); // batch 1 = [deploy, rollback]
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
    app.on_key(key(KeyCode::Tab)); // a turn is in flight — this queues
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
    // A Shell entry stands alone: a Tab-text after it opens a *fresh*
    // Messages batch (the back is a Shell, not a Messages), so the command
    // never gets concatenated into a text turn — codex's per-completion
    // shell dispatch ("cannot be added to").
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("hello");
    app.on_key(key(KeyCode::Tab)); // Messages(["hello"])
    type_query(&mut app, "!ls");
    app.on_key(key(KeyCode::Enter)); // Shell("ls") — standalone
    app.input = TextArea::from_text("world");
    app.on_key(key(KeyCode::Tab)); // a NEW Messages(["world"]), not merged
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

// ===== steering: what Enter hands to the turn already running =====

#[test]
fn enter_mid_turn_hands_the_message_to_the_running_turn() {
    // The mid-turn queue's whole point: a message submitted while the model
    // works goes *into that turn* — the boundary pushes it onto the shared
    // queue the agent loop drains at its next round boundary — instead of
    // waiting for the turn to end.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("also check the tests");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Steer("also check the tests".to_string()),
        "the boundary is handed the text to steer"
    );
    assert_eq!(
        app.steered,
        ["also check the tests"],
        "and it shows above the box until the turn takes it"
    );
    assert!(
        app.queued.is_empty(),
        "it is not a follow-up turn — it belongs to the one running"
    );
    assert_eq!(app.input.text(), "", "the composer is consumed");
}

#[test]
fn consecutive_enters_steer_in_submission_order() {
    let mut app = App::new();
    app.begin_stream();
    for text in ["first", "second"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Enter));
    }
    assert_eq!(app.steered, ["first", "second"]);
}

#[test]
fn a_steered_message_is_recorded_for_recall() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("and deploy it");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.take_unpersisted_inputs(),
        ["and deploy it"],
        "↑ recalls a steered message like any other submit"
    );
}

#[test]
fn the_turn_taking_a_message_turns_it_into_a_user_bubble() {
    // `StreamEvent::Steered`: the model genuinely has the text now, so the
    // inset row above the box becomes a real user message in the conversation.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("also check the tests");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.deliver_steered("also check the tests"),
        "the row was ours"
    );
    assert!(app.steered.is_empty(), "it stops waiting above the box");
    assert!(
        matches!(
            app.history.last(),
            Some(HistoryItem::Message(m))
                if m.role == Role::User && m.text == "also check the tests"
        ),
        "and lands in the transcript where the model read it"
    );
}

#[test]
fn a_taken_message_finalises_the_reply_streamed_before_it() {
    // Invariant 4's flush-before-you-interleave: the run of assistant text
    // ahead of the user's message becomes its own history message, so the
    // bubble slots *after* it and a repaint keeps that order.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("Reading the file…");
    app.deliver_steered("wait, check the tests too");
    let roles: Vec<Role> = app
        .history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Message(m) => Some(m.role),
            _ => None,
        })
        .collect();
    assert_eq!(
        roles,
        vec![Role::Assistant, Role::User],
        "the streamed segment is finalised in front of the interleaved message"
    );
}

#[test]
fn enter_during_a_shell_turn_still_queues_a_follow_up() {
    // A `!` command has no model reading anything, so there is no round
    // boundary to steer into: the draft waits for the turn to end, exactly as
    // it always did.
    let mut app = App::new();
    app.begin_shell("sleep 5");
    app.input = TextArea::from_text("next please");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.steered.is_empty(), "nothing to steer into");
    assert_eq!(app.queued, [batch(&["next please"])]);
}

#[test]
fn a_draft_with_attachments_queues_instead_of_steering() {
    // A round-boundary injection is text-only (`ChatMessage::user`), so a
    // draft carrying Ctrl+V images keeps the follow-up path, where the typed
    // image channel can carry them (docs/image-paste.md).
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(std::path::PathBuf::from("/tmp/shot.png"));
    app.input = TextArea::from_text("what is in [Image #1]?");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(
        app.steered.is_empty(),
        "an attachment can't ride a round boundary"
    );
    assert_eq!(app.queued.len(), 1, "so it queues as a follow-up turn");
}

#[test]
fn an_undelivered_message_is_reclaimed_as_the_next_turn() {
    // The turn ended before its next round boundary (the model just
    // answered): what it never took becomes the next turn, batched.
    let mut app = App::new();
    app.begin_stream();
    for text in ["first", "second"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Enter));
    }
    app.reclaim_steered();
    assert!(app.steered.is_empty());
    assert_eq!(
        app.queued,
        [batch(&["first", "second"])],
        "one turn, the messages in order"
    );
}

#[test]
fn reclaimed_messages_lead_the_follow_ups_already_queued() {
    // They were submitted into the *current* turn, so they run before a Tab
    // follow-up that was always meant for later.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("later");
    app.on_key(key(KeyCode::Tab)); // an explicit follow-up turn
    app.input = TextArea::from_text("now");
    app.on_key(key(KeyCode::Enter)); // steered into the running turn
    app.reclaim_steered();
    assert_eq!(app.queued, [batch(&["now"]), batch(&["later"])]);
}

#[test]
fn reclaiming_nothing_leaves_the_queue_alone() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("later");
    app.on_key(key(KeyCode::Tab));
    app.reclaim_steered();
    assert_eq!(
        app.queued,
        [batch(&["later"])],
        "no empty batch is invented"
    );
}

#[test]
fn alt_up_over_a_steered_message_asks_the_boundary_for_it_back() {
    // Only the shared queue knows whether the turn has taken it yet, so the
    // pull-back is the boundary's call (docs/queue.md).
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("oops typo");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::ReclaimSteered);
}

#[test]
fn alt_up_edits_a_follow_up_turn_before_reaching_the_running_one() {
    // The follow-up queue is the deliberate backlog and the one Alt+Up has
    // always edited; a message already handed to the running turn is only
    // reachable once nothing is left there.
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("now");
    app.on_key(key(KeyCode::Enter)); // steered
    app.input = TextArea::from_text("later");
    app.on_key(key(KeyCode::Tab)); // a follow-up turn
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "later");
    assert_eq!(app.steered, ["now"], "the running turn keeps its message");
}

#[test]
fn recall_steered_puts_the_message_back_in_the_composer() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("oops typo");
    app.on_key(key(KeyCode::Enter));
    app.recall_steered("oops typo");
    assert_eq!(app.input.text(), "oops typo", "back as an editable draft");
    assert!(app.steered.is_empty(), "and off the strip");
}

#[test]
fn alt_up_falls_back_to_the_queue_with_nothing_in_flight() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("later");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "later");
    assert!(app.queued.is_empty());
}

#[test]
fn clear_mid_turn_drops_the_steered_messages_too() {
    let mut app = App::new();
    app.begin_stream();
    app.input = TextArea::from_text("still going");
    app.on_key(key(KeyCode::Enter));
    app.clear_conversation();
    assert!(app.steered.is_empty(), "/clear kills the whole turn");
}

#[test]
fn a_message_handed_to_the_turn_blocks_the_interrupt_undo() {
    // The undo rolls the submission back into the composer, which is only
    // right when nothing is waiting behind it. A steered message is waiting
    // behind it exactly like a queued follow-up, and the interrupt is what
    // sends it (docs/interrupt.md).
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.input = TextArea::from_text("wait, also this");
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.interrupt_turn(), Some(InterruptedTurn::Kept { .. })),
        "the turn is kept so the reclaim can send what it never read"
    );
    assert_eq!(app.input.text(), "", "the submission is not pulled back");
}

#[test]
fn a_compact_turn_takes_no_queued_messages() {
    // A summarization turn is not a conversation (docs/compact.md): its
    // request is the fixed handoff prompt, and a user message folded into it
    // would corrupt the summary. The draft queues as a follow-up turn, which
    // is what it was always going to be.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_compact(false);
    assert!(!app.turn_steerable(), "nothing to steer into");
    app.input = TextArea::from_text("meanwhile, check the tests");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.steered.is_empty());
    assert_eq!(app.queued, [batch(&["meanwhile, check the tests"])]);
}

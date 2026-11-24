//! The compactor driven with a fake summarizer: no thread, no network, no
//! clock a test waits on (`docs/compact.md` *Compacting while the agent
//! works*).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::*;
use crate::llm::ToolCallSpec;

/// About `tokens` estimated tokens of prose.
fn filler(tokens: usize) -> String {
    "lorem ".repeat(tokens)
}

/// One summarization the fake summarizer was asked for.
struct Spawned {
    job: SummaryJob,
    reply: Sender<SummaryResult>,
    cancel: CancelToken,
}

/// A turn's surroundings: the session's policy, the reply channel, and a
/// summarizer whose answers the test decides.
struct World {
    control: CompactionControl,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    rx: UnboundedReceiver<StreamEvent>,
    cancel: CancelToken,
    spawned: Rc<RefCell<Vec<Spawned>>>,
    /// What the next summarizations answer **at once**, in order. One asked
    /// for with this empty stays in flight until [`World::resolve`].
    ready: Rc<RefCell<VecDeque<SummaryResult>>>,
    /// The length of the messages at each swap.
    swaps: Rc<RefCell<Vec<usize>>>,
}

impl World {
    /// A session with auto compaction on and a `window`-token model.
    fn new(window: u64) -> Self {
        let (tx, rx) = unbounded_channel();
        let control = CompactionControl::new(true);
        control.set_window(Some(window));
        Self {
            control,
            tx,
            rx,
            cancel: CancelToken::new(),
            spawned: Rc::default(),
            ready: Rc::default(),
            swaps: Rc::default(),
        }
    }

    fn compactor(&self, seed: CompactionSeed) -> RollingCompactor<'static> {
        self.compactor_with(seed, LoadMeter::standalone(0), true)
    }

    fn compactor_with(
        &self,
        seed: CompactionSeed,
        load: LoadMeter,
        takes_requests: bool,
    ) -> RollingCompactor<'static> {
        let spawned = Rc::clone(&self.spawned);
        let ready = Rc::clone(&self.ready);
        let swaps = Rc::clone(&self.swaps);
        RollingCompactor::new(
            RollingSetup {
                control: self.control.clone(),
                inherits: true,
                takes_requests,
                load,
                seed,
                tx: self.tx.clone(),
                cancel: self.cancel.clone(),
            },
            move |job| {
                let (reply, result) = channel();
                let cancel = CancelToken::new();
                if let Some(answer) = ready.borrow_mut().pop_front() {
                    reply.send(answer).expect("the compactor is listening");
                }
                spawned.borrow_mut().push(Spawned {
                    job,
                    reply,
                    cancel: cancel.clone(),
                });
                SummaryHandle { result, cancel }
            },
            move |messages| swaps.borrow_mut().push(messages.len()),
        )
    }

    /// Answer the `index`th summarization.
    fn resolve(&self, index: usize, result: SummaryResult) {
        self.spawned.borrow()[index]
            .reply
            .send(result)
            .expect("the compactor is listening");
    }

    /// The next summarization answers `summary` at once.
    fn answer(&self, summary: &str) {
        self.ready.borrow_mut().push_back(summary_of(summary));
    }

    /// How many summarizations were asked for.
    fn asked(&self) -> usize {
        self.spawned.borrow().len()
    }

    /// The `index`th summarization's request.
    fn request(&self, index: usize) -> String {
        self.spawned.borrow()[index].job.request.clone()
    }

    /// Everything sent on the reply channel since the last look.
    fn events(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }
}

fn summary_of(text: &str) -> SummaryResult {
    SummaryResult::of(SummaryOutcome::Summary(text.to_string()))
}

fn seed(user_texts: &[&str]) -> CompactionSeed {
    CompactionSeed {
        leading: Some("<system-reminder>\nbe careful\n</system-reminder>".to_string()),
        user_texts: user_texts.iter().map(ToString::to_string).collect(),
    }
}

/// A conversation that opens on the system prompt and the user's request.
fn conversation(request: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system("You are a coding agent."),
        ChatMessage::user(request),
    ]
}

/// One tool round: the model reads `path`, the tool answers with about
/// `tokens` tokens.
fn round(messages: &mut Vec<ChatMessage>, id: &str, path: &str, tokens: usize) {
    messages.push(ChatMessage::assistant_tool_calls(
        "",
        vec![ToolCallSpec::function(
            id,
            "read",
            format!(r#"{{"path":"{path}"}}"#),
        )],
    ));
    messages.push(ChatMessage::tool_result(id, filler(tokens)));
}

fn text_of(message: &ChatMessage) -> &str {
    match &message.content {
        MessageContent::Text(text) => text,
        MessageContent::Parts(_) => panic!("a plain message: {message:?}"),
    }
}

// ----- the trigger -----

#[test]
fn the_filler_is_about_a_token_a_word() {
    // The tests size their conversations with it.
    let tokens = estimate(&filler(1_000));
    assert!((900..=1_200).contains(&tokens), "{tokens}");
}

#[test]
fn a_context_under_the_soft_threshold_is_left_alone() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 2_000);
    let before = messages.clone();

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert_eq!(messages, before);
    assert!(world.events().is_empty());
}

#[test]
fn a_summary_starts_in_the_background_past_the_soft_threshold() {
    // Eight tenths of the window: the summarizer starts, and the round goes
    // out as it is — the agent does not wait.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    let before = messages.clone();

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1, "one summarization, in flight");
    assert_eq!(messages, before, "nothing is swapped until it lands");
    assert_eq!(
        world.events(),
        vec![StreamEvent::CompactionStarted { auto: true }]
    );
    // The summarizer is sent the conversation's own system prompt, and the
    // conversation as a transcript under the prompt.
    let spawned = world.spawned.borrow();
    assert_eq!(
        spawned[0].job.system.as_deref(),
        Some("You are a coding agent.")
    );
    assert!(
        spawned[0]
            .job
            .request
            .starts_with("<conversation>\n[user]\nfix the parser")
    );
    assert!(
        spawned[0]
            .job
            .request
            .ends_with(SUMMARIZATION_PROMPT.trim_end())
    );
    assert!(spawned[0].job.auto);
}

#[test]
fn a_summary_in_flight_is_not_asked_for_twice() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    round(&mut messages, "call_2", "b.rs", 100);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1);
    assert_eq!(
        world.events(),
        vec![StreamEvent::CompactionStarted { auto: true }]
    );
}

#[test]
fn a_landed_summary_is_swapped_in_at_the_next_round_boundary() {
    // What the summarizer read becomes the user's own messages and the
    // bridge; what the agent did while it was being written stays as it
    // happened, behind them.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.events();

    // The agent works on while the summary is written.
    round(&mut messages, "call_2", "b.rs", 200);
    let tail = messages[4..].to_vec();
    world.resolve(0, summary_of("read a.rs; the parser drops the last token"));

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(messages.len(), 4, "{messages:?}");
    assert_eq!(messages[0], ChatMessage::system("You are a coding agent."));
    assert_eq!(
        text_of(&messages[1]),
        format!(
            "<system-reminder>\nbe careful\n</system-reminder>\n\nfix the parser\n\n{}",
            summary_bridge("read a.rs; the parser drops the last token", true)
        )
    );
    assert_eq!(&messages[2..], &tail[..], "the rounds since, untouched");
    assert_eq!(
        world.events(),
        vec![StreamEvent::Compacted {
            summary: "read a.rs; the parser drops the last token".to_string(),
            secs: 0,
            budget: 1_250,
        }]
    );
    assert_eq!(*world.swaps.borrow(), vec![4]);
}

#[test]
fn the_compacted_prefix_is_what_the_history_derives() {
    // The wire a compacted turn goes on with and the context every later
    // turn rebuilds must be one text: the same fragment, the same kept
    // messages under the same budget, the same bridge.
    use crate::app::{Compaction, HistoryItem, Message, Role};
    let user = |text: &str| {
        HistoryItem::Message(Message {
            role: Role::User,
            text: text.to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        })
    };
    let big = "x".repeat(4_000);
    let history = vec![
        user("the first request"),
        user(&big),
        user("and one more thing"),
        HistoryItem::Compaction(Compaction {
            summary: "so far so good".to_string(),
            resume: true,
            budget: 300,
            ..Compaction::default()
        }),
    ];
    let leading = "<system-reminder>\nbe careful\n</system-reminder>";
    let derived = crate::context::context_messages_behind(Some(leading), &history);
    let prefix = compacted_prefix(
        None,
        Some(leading),
        &[
            "the first request".to_string(),
            big.clone(),
            "and one more thing".to_string(),
        ],
        "so far so good",
        300,
    );
    assert_eq!(prefix.len(), 1);
    assert_eq!(derived.len(), 1);
    assert_eq!(text_of(&prefix[0]), derived[0].text);
    assert!(
        derived[0].text.contains("tokens truncated…"),
        "the budget bit"
    );
}

#[test]
fn a_message_sent_after_the_summary_started_stays_where_it_was_said() {
    // The kept messages are the ones the summarizer's part of the
    // conversation held. One the user sent while it was being written is in
    // the part kept verbatim already.
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    compactor.user_message("and keep the tests green");
    messages.push(ChatMessage::user("and keep the tests green"));
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);

    compactor.user_message("also the lexer");
    messages.push(ChatMessage::user("also the lexer"));
    world.resolve(0, summary_of("half way"));
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);

    let prefix = text_of(&messages[1]);
    assert!(
        prefix.contains("fix the parser\n\nand keep the tests green"),
        "{prefix}"
    );
    assert!(!prefix.contains("also the lexer"), "{prefix}");
    assert_eq!(messages[2], ChatMessage::user("also the lexer"));
}

#[test]
fn past_the_hard_threshold_the_round_waits_for_the_summary() {
    // Nine tenths: nothing more is sent until the context is smaller.
    let mut world = World::new(10_000);
    world.answer("everything so far");
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(
        messages.len(),
        2,
        "compacted before the request: {messages:?}"
    );
    let events = world.events();
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0], StreamEvent::CompactionStarted { auto: true });
    assert_eq!(events[1], StreamEvent::CompactionWaiting);
    assert!(
        matches!(&events[2], StreamEvent::Compacted { summary, .. } if summary == "everything so far")
    );
}

#[test]
fn a_summary_started_in_the_background_is_waited_for_once_the_context_is_past_hard() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.events();

    round(&mut messages, "call_2", "b.rs", 1_000);
    world.resolve(0, summary_of("read a.rs"));
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1, "the one already running");
    assert_eq!(messages.len(), 4, "the prefix and the round since");
    // It had landed by the time it was looked for: no wait to announce.
    assert!(matches!(
        world.events().as_slice(),
        [StreamEvent::Compacted { .. }]
    ));
}

#[test]
fn a_turn_compacts_as_often_as_it_needs() {
    // No count runs out: a turn working through a small window compacts
    // again and again, each summary read from the one before it.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["port the whole crate"]));
    let mut messages = conversation("port the whole crate");
    let mut compactions = 0;
    for step in 0..40 {
        round(&mut messages, &format!("call_{step}"), "f.rs", 1_500);
        world.answer(&format!("summary {}", world.asked() + 1));
        assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
        let landed = world
            .events()
            .iter()
            .filter(|event| matches!(event, StreamEvent::Compacted { .. }))
            .count();
        compactions += landed;
        // Whatever was just queued for a summarization nobody asked for.
        if world.asked() == compactions {
            world.ready.borrow_mut().clear();
        }
        assert!(
            compactor.load.reading(&messages) <= 9_000,
            "step {step}: every request fits"
        );
    }
    assert!(
        compactions >= 5,
        "{compactions} compactions over forty rounds"
    );
    assert_eq!(world.asked(), compactions);
    // Each summary after the first was written over the last.
    for index in 1..world.asked() {
        assert!(
            world.request(index).contains(&format!("summary {index}")),
            "summarization {} read summary {index}",
            index + 1
        );
    }
}

#[test]
fn rounds_too_large_to_keep_are_compacted_in_the_same_step() {
    // The agent ran on while the summary was written and what it added is
    // itself too much: the swap is followed by another, at once.
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    round(&mut messages, "call_2", "b.rs", 9_500);
    world.resolve(0, summary_of("read a.rs"));
    world.answer("read a.rs and b.rs");

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 2);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(text_of(&messages[1]).contains("read a.rs and b.rs"));
    assert!(
        world.request(1).contains("read a.rs"),
        "the second read the first's summary"
    );
}

#[test]
fn a_context_with_nothing_to_gain_is_not_summarized() {
    // The tools alone fill the window: a summary of two short messages
    // would cost a request and free nothing.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor_with(seed(&["hello"]), LoadMeter::standalone(9_500), true);
    let mut messages = conversation("hello");
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert!(world.events().is_empty());
}

#[test]
fn auto_compaction_turned_off_starts_nothing() {
    let mut world = World::new(10_000);
    world.control.set_auto(false);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert!(world.events().is_empty());
}

#[test]
fn a_window_nobody_knows_starts_nothing() {
    let world = World::new(10_000);
    world.control.set_window(None);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 50_000);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
}

#[test]
fn turning_auto_compaction_off_mid_turn_stops_the_next_one() {
    // The handle is live: the `/settings` row reaches a turn already
    // running.
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    world.control.set_auto(false);
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
}

// ----- /compact while the turn runs -----

#[test]
fn a_compact_typed_mid_turn_runs_at_the_next_step() {
    // The user asked: no threshold is consulted, and auto compaction being
    // off is beside the point.
    let mut world = World::new(100_000);
    world.control.set_auto(false);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 6_000);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);

    world.control.request();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1);
    assert!(
        !world.spawned.borrow()[0].job.auto,
        "the user's, not the gauge's"
    );
    assert_eq!(
        world.events(),
        vec![StreamEvent::CompactionStarted { auto: false }]
    );
    assert!(!world.control.requested(), "taken");
}

#[test]
fn a_compact_typed_right_after_a_compaction_has_nothing_to_compact() {
    // Nothing was said since the summary landed: summarizing a summary
    // would only cost a request.
    let mut world = World::new(100_000);
    world.answer("the gist");
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 6_000);
    world.control.request();
    assert_eq!(compactor.before_done(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1);
    world.events();

    world.control.request();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1, "no second summarization");
    assert_eq!(
        world.events(),
        vec![StreamEvent::CompactionFailed {
            reason: "Nothing to compact".to_string()
        }]
    );
    // One more round, and there is something again.
    round(&mut messages, "call_2", "b.rs", 100);
    world.control.request();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 2);
}

#[test]
fn a_compact_typed_over_an_empty_conversation_has_nothing_to_compact() {
    let mut world = World::new(100_000);
    let mut compactor = world.compactor(seed(&[]));
    let mut messages = vec![ChatMessage::system("You are a coding agent.")];
    world.control.request();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert_eq!(
        world.events(),
        vec![StreamEvent::CompactionFailed {
            reason: "Nothing to compact".to_string()
        }]
    );
}

#[test]
fn a_subagent_leaves_the_users_compact_for_the_lead() {
    let world = World::new(100_000);
    let mut compactor = world.compactor_with(seed(&["task"]), LoadMeter::standalone(0), false);
    let mut messages = conversation("task");
    round(&mut messages, "call_1", "a.rs", 6_000);
    world.control.request();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert!(world.control.requested(), "still the lead's to take");
}

// ----- a summary that does not come -----

#[test]
fn a_failed_summary_is_said_and_the_turn_goes_on() {
    let mut world = World::new(10_000);
    world
        .ready
        .borrow_mut()
        .push_back(SummaryResult::of(SummaryOutcome::Failed(
            "HTTP 500: upstream".to_string(),
        )));
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    let before = messages.clone();

    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(messages, before, "the context stands as it was");
    let events = world.events();
    assert_eq!(
        events.last(),
        Some(&StreamEvent::CompactionFailed {
            reason: "HTTP 500: upstream".to_string()
        })
    );
}

#[test]
fn a_failed_summary_is_not_asked_for_again_at_once() {
    // A provider's hiccup gets a few rounds to pass — and then the context,
    // still filling, is caught again.
    let world = World::new(10_000);
    world
        .ready
        .borrow_mut()
        .push_back(SummaryResult::of(SummaryOutcome::Failed(
            "boom".to_string(),
        )));
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    // In the background, then failed by the next boundary.
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 1);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(
        world.asked(),
        1,
        "the round right after a failure starts none"
    );
    let mut rounds = 0;
    while world.asked() == 1 {
        assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
        rounds += 1;
        assert!(rounds <= FAILURE_COOLDOWN_ROUNDS, "it never tried again");
    }
}

#[test]
fn summaries_that_keep_failing_stop_being_started() {
    // A provider that cannot summarize is not asked again at every step of
    // a long turn: a failure is followed by a few rounds of nothing, and the
    // third is the last one this turn asks for.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    for _ in 0..40 {
        if world.ready.borrow().is_empty() {
            // Whatever is asked for next fails at once.
            world
                .ready
                .borrow_mut()
                .push_back(SummaryResult::of(SummaryOutcome::Failed(
                    "boom".to_string(),
                )));
        }
        assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    }
    assert_eq!(world.asked() as u32, MAX_FAILURES);
    let said = world
        .events()
        .iter()
        .filter(|event| matches!(event, StreamEvent::CompactionFailed { .. }))
        .count();
    assert_eq!(said as u32, MAX_FAILURES, "each one is said once");
}

#[test]
fn a_summary_that_did_not_fit_is_asked_for_again_with_less_to_read() {
    // The request that makes room must not be the one that overflows: the
    // same conversation, half as much of it — and no failure said, since
    // nothing has failed yet.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    for index in 0..6 {
        round(&mut messages, &format!("call_{index}"), "a.rs", 1_400);
    }
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.events();
    world.resolve(
        0,
        SummaryResult {
            outcome: SummaryOutcome::Overflow("HTTP 400: prompt is too long".to_string()),
            usage: vec![TokenUsage {
                input: 7,
                ..TokenUsage::default()
            }],
            cells: Vec::new(),
        },
    );
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 2);
    assert!(
        world.request(1).len() < world.request(0).len() * 3 / 4,
        "{} then {}",
        world.request(0).len(),
        world.request(1).len()
    );
    assert!(world.events().is_empty(), "still being written");

    // What the overflowed attempt cost is billed with the one that landed.
    world.resolve(
        1,
        SummaryResult {
            outcome: SummaryOutcome::Summary("the gist".to_string()),
            usage: vec![TokenUsage {
                input: 3,
                ..TokenUsage::default()
            }],
            cells: Vec::new(),
        },
    );
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    let events = world.events();
    assert!(
        matches!(
            events.as_slice(),
            [
                StreamEvent::CompactionUsage(first),
                StreamEvent::CompactionUsage(second),
                StreamEvent::Compacted { .. },
            ] if first.input == 7 && second.input == 3
        ),
        "{events:?}"
    );
}

#[test]
fn a_summary_that_never_fits_fails_in_the_end() {
    let mut world = World::new(10_000);
    for _ in 0..=MAX_SHRINKS {
        world
            .ready
            .borrow_mut()
            .push_back(SummaryResult::of(SummaryOutcome::Overflow(
                "context_length_exceeded".to_string(),
            )));
    }
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert_eq!(world.asked() as u32, MAX_SHRINKS + 1);
    assert!(matches!(
        world.events().last(),
        Some(StreamEvent::CompactionFailed { .. })
    ));
}

// ----- what a summary costs and what its hooks said -----

#[test]
fn what_the_summary_cost_is_reported_ahead_of_its_landing() {
    let mut world = World::new(10_000);
    let usage = TokenUsage {
        input: 8_000,
        output: 400,
        ..TokenUsage::default()
    };
    let report = HookReport {
        event: "PreCompact".to_string(),
        hooks: Vec::new(),
        warnings: vec!["a warning".to_string()],
    };
    world.ready.borrow_mut().push_back(SummaryResult {
        outcome: SummaryOutcome::Summary("the gist".to_string()),
        usage: vec![usage],
        cells: vec![("PreCompact hook".to_string(), report.clone())],
    });
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);

    let events = world.events();
    assert_eq!(
        &events[2..4],
        &[
            StreamEvent::CompactionUsage(usage),
            // The compact hooks' cell, above the marker; it gave the model
            // nothing, so it carries no text.
            StreamEvent::HookNote {
                label: "PreCompact hook".to_string(),
                text: String::new(),
                report: Some(report),
            },
        ]
    );
    assert!(matches!(&events[4], StreamEvent::Compacted { .. }));
}

#[test]
fn a_summary_longer_than_its_share_of_the_window_is_cut() {
    let mut world = World::new(10_000);
    world.answer(&filler(5_000));
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    let events = world.events();
    let Some(StreamEvent::Compacted { summary, .. }) = events.last() else {
        panic!("it landed: {events:?}");
    };
    assert!(estimate(summary) <= 1_000, "{}", estimate(summary));
    assert!(summary.contains("tokens truncated…"));
}

// ----- the turn's end, a cancel, an overflow -----

#[test]
fn a_turn_does_not_end_under_a_summary_still_being_written() {
    // The summary is paid for: it is waited for and swapped in, so the next
    // turn opens on the compacted context.
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.events();
    world.resolve(0, summary_of("all of it"));

    assert_eq!(compactor.before_done(&mut messages), Flow::Continue);
    assert_eq!(messages.len(), 2);
    let events = world.events();
    assert_eq!(events[0], StreamEvent::CompactionWaiting);
    assert!(matches!(&events[1], StreamEvent::Compacted { .. }));
}

#[test]
fn a_turn_with_no_summary_in_flight_ends_at_once() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    // Past every threshold, and still: the end of a turn starts nothing.
    assert_eq!(compactor.before_done(&mut messages), Flow::Continue);
    assert_eq!(world.asked(), 0);
    assert!(world.events().is_empty());
}

#[test]
fn a_compact_typed_during_the_last_reply_is_made_before_the_turn_ends() {
    let mut world = World::new(100_000);
    world.answer("the whole turn");
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 6_000);
    world.control.request();
    assert_eq!(compactor.before_done(&mut messages), Flow::Continue);
    assert_eq!(messages.len(), 2);
    assert!(matches!(
        world.events().last(),
        Some(StreamEvent::Compacted { .. })
    ));
}

#[test]
fn a_cancel_while_waiting_stops_the_summary_and_the_turn() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 9_400);
    world.cancel.cancel();

    assert_eq!(compactor.before_round(&mut messages), Flow::Cancelled);
    assert!(world.spawned.borrow()[0].cancel.is_cancelled());
    assert!(
        !world
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::Compacted { .. }))
    );
}

#[test]
fn a_turn_that_ends_stops_the_summary_it_left_behind() {
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert!(!world.spawned.borrow()[0].cancel.is_cancelled());
    drop(compactor);
    assert!(world.spawned.borrow()[0].cancel.is_cancelled());
}

#[test]
fn a_summarizer_that_goes_away_has_failed() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.events();
    // Its thread died: the sending half is gone with it.
    world.spawned.borrow_mut().clear();
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    assert!(matches!(
        world.events().as_slice(),
        [StreamEvent::CompactionFailed { .. }]
    ));
}

#[test]
fn a_request_that_overflowed_is_given_room() {
    // No window is known, so nothing was started — until the provider said
    // the request does not fit.
    let mut world = World::new(10_000);
    world.control.set_window(None);
    world.answer("what was done");
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 50_000);

    assert_eq!(compactor.make_room(&mut messages), Room::Made);
    assert_eq!(messages.len(), 2);
    let events = world.events();
    assert_eq!(events[0], StreamEvent::CompactionStarted { auto: true });
    assert_eq!(events[1], StreamEvent::CompactionWaiting);
    assert!(matches!(&events[2], StreamEvent::Compacted { .. }));
}

#[test]
fn an_overflow_waits_for_the_summary_already_being_written() {
    let mut world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 8_300);
    assert_eq!(compactor.before_round(&mut messages), Flow::Continue);
    world.resolve(0, summary_of("read a.rs"));
    assert_eq!(compactor.make_room(&mut messages), Room::Made);
    assert_eq!(world.asked(), 1);
    assert!(matches!(
        world.events().last(),
        Some(StreamEvent::Compacted { .. })
    ));
}

#[test]
fn an_overflow_with_nothing_left_to_summarize_cannot_be_helped() {
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["hello"]));
    let mut messages = conversation("hello");
    assert_eq!(compactor.make_room(&mut messages), Room::Unable);
    assert_eq!(world.asked(), 0);
}

#[test]
fn an_overflow_is_left_alone_with_auto_compaction_off() {
    let world = World::new(10_000);
    world.control.set_auto(false);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 50_000);
    assert_eq!(compactor.make_room(&mut messages), Room::Unable);
    assert_eq!(world.asked(), 0);
}

#[test]
fn an_overflow_whose_summary_fails_cannot_be_helped() {
    let mut world = World::new(10_000);
    world
        .ready
        .borrow_mut()
        .push_back(SummaryResult::of(SummaryOutcome::Failed(
            "boom".to_string(),
        )));
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 50_000);
    assert_eq!(compactor.make_room(&mut messages), Room::Unable);
    assert!(matches!(
        world.events().last(),
        Some(StreamEvent::CompactionFailed { .. })
    ));
}

#[test]
fn an_overflow_cancelled_while_it_waits_says_so() {
    let world = World::new(10_000);
    let mut compactor = world.compactor(seed(&["fix the parser"]));
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_1", "a.rs", 50_000);
    world.cancel.cancel();
    assert_eq!(compactor.make_room(&mut messages), Room::Cancelled);
}

#[test]
fn the_seam_alone_does_nothing() {
    let mut messages = conversation("hello");
    let before = messages.clone();
    let mut none = NoCompaction;
    assert_eq!(none.before_round(&mut messages), Flow::Continue);
    assert_eq!(none.make_room(&mut messages), Room::Unable);
    assert_eq!(none.before_done(&mut messages), Flow::Continue);
    none.user_message("anything");
    assert_eq!(messages, before);
}

// ----- how full the context is -----

#[test]
fn before_any_usage_frame_the_load_is_the_estimate_and_the_tools() {
    let messages = conversation(&filler(1_000));
    let bare = LoadMeter::standalone(0).reading(&messages);
    assert_eq!(bare, wire_tokens(&messages));
    assert_eq!(LoadMeter::standalone(700).reading(&messages), bare + 700);
}

#[test]
fn a_usage_frame_is_the_base_what_joins_after_is_added_to() {
    // The provider counted the request at 12,000 tokens — its framing and
    // its own reading of the tool schemas among them. What the round then
    // added is estimated on top.
    let load = LoadMeter::standalone(0);
    let mut messages = conversation(&filler(1_000));
    load.measured(&messages, 12_000);
    assert_eq!(load.reading(&messages), 12_000);
    let before = wire_tokens(&messages);
    round(&mut messages, "call_1", "a.rs", 500);
    let added = wire_tokens(&messages) - before;
    assert_eq!(load.reading(&messages), 12_000 + added);
}

#[test]
fn a_compaction_moves_the_base_down_by_what_it_took_out() {
    let load = LoadMeter::standalone(0);
    let mut messages = conversation("fix it");
    round(&mut messages, "call_1", "a.rs", 6_000);
    load.measured(&messages, 9_000);
    let removed = wire_tokens(&messages[2..]);
    messages.truncate(2);
    load.rebase(&messages);
    assert_eq!(load.reading(&messages), 9_000 - removed);
    // …and what joins next is added to that.
    let before = wire_tokens(&messages);
    round(&mut messages, "call_2", "b.rs", 300);
    assert_eq!(
        load.reading(&messages),
        9_000 - removed + (wire_tokens(&messages) - before)
    );
}

#[test]
fn every_turn_of_a_backend_reads_the_same_meter() {
    let shared = Arc::new(Mutex::new(ContextMeter::default()));
    let messages = conversation(&filler(1_000));
    LoadMeter::new(Arc::clone(&shared), 0).measured(&messages, 5_000);
    assert_eq!(LoadMeter::new(shared, 0).reading(&messages), 5_000);
}

#[test]
fn a_picture_and_a_tool_call_are_counted_with_the_text() {
    let call = ChatMessage::assistant_tool_calls(
        "reading",
        vec![ToolCallSpec::function(
            "call_1",
            "read",
            r#"{"path":"a.rs"}"#,
        )],
    );
    assert_eq!(
        wire_tokens(std::slice::from_ref(&call)),
        (estimate("reading") + estimate("read") + estimate(r#"{"path":"a.rs"}"#)) as u64
    );
    let picture = ChatMessage::with_parts(
        "user",
        vec![
            ContentPart::text("look"),
            ContentPart::image("data:image/png;base64,AAAA"),
        ],
    );
    assert_eq!(
        wire_tokens(std::slice::from_ref(&picture)),
        (estimate("look") + crate::app::IMAGE_INPUT_TOKENS) as u64
    );
}

// ----- a subagent's seed -----

#[test]
fn a_subagents_seed_is_its_task_and_what_it_was_sent_since() {
    // A subagent has no history but its messages: its briefing leads, its
    // task is kept word for word, and so is what the user or the lead sent
    // it after — never the app's own notes, nor a picture's.
    let briefing = "<system-reminder>\nthe skills\n</system-reminder>";
    let messages = vec![
        ChatMessage::system("You are a subagent."),
        ChatMessage::user(briefing),
        ChatMessage::user("map the parser"),
        ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCallSpec::function(
                "call_1",
                "read",
                r#"{"path":"a.rs"}"#,
            )],
        ),
        ChatMessage::tool_result("call_1", "fn parse() {}"),
        ChatMessage::user("[system] Your last response was empty"),
        ChatMessage::user("also the lexer"),
        ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("the picture read"),
                ContentPart::image("data:image/png;base64,AAAA"),
            ],
        ),
    ];
    let seed = seed_from_messages(
        &messages,
        Some(briefing.to_string()),
        Some("map the parser"),
    );
    assert_eq!(seed.leading.as_deref(), Some(briefing));
    assert_eq!(seed.user_texts, vec!["map the parser", "also the lexer"]);
}

#[test]
fn a_continued_subagents_seed_passes_over_what_a_compaction_left() {
    // Its stored conversation opens on the merged prefix an earlier
    // compaction made: that is no message anyone sent. The task is known
    // apart from it, and the message that continued the run is the user's.
    let prefix = compacted_prefix(
        Some(ChatMessage::system("You are a subagent.")),
        None,
        &["map the parser".to_string()],
        "mapped half of it",
        20_000,
    );
    let mut messages = prefix;
    messages.push(ChatMessage::assistant("Mapped."));
    messages.push(ChatMessage::user("now the lexer"));
    let seed = seed_from_messages(&messages, None, Some("map the parser"));
    assert_eq!(seed.user_texts, vec!["map the parser", "now the lexer"]);
}

// ----- the transcript -----

#[test]
fn the_transcript_says_who_said_what() {
    let messages = vec![
        ChatMessage::system("You are a coding agent."),
        ChatMessage::user("fix the parser"),
        ChatMessage::assistant_tool_calls(
            "Reading it first.",
            vec![ToolCallSpec::function(
                "call_1",
                "read",
                r#"{"path":"a.rs"}"#,
            )],
        ),
        ChatMessage::tool_result("call_1", "fn parse() {}"),
        ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("the screenshot"),
                ContentPart::image("data:image/png;base64,AAAA"),
            ],
        ),
        ChatMessage::assistant("Done."),
    ];
    assert_eq!(
        render_transcript(&messages, 10_000),
        "[user]\nfix the parser\n\n\
         [assistant]\nReading it first.\n[tool call] read {\"path\":\"a.rs\"}\n\n\
         [tool result: read]\nfn parse() {}\n\n\
         [user]\nthe screenshot\n[image]\n\n\
         [assistant]\nDone."
    );
}

#[test]
fn a_conversation_with_no_system_prompt_is_rendered_from_its_first_message() {
    let messages = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];
    assert_eq!(
        render_transcript(&messages, 1_000),
        "[user]\nhello\n\n[assistant]\nhi"
    );
}

#[test]
fn a_long_tool_result_is_cut_in_its_middle() {
    // Nine old rounds and a big result among them: only that one is cut.
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_big", "big.rs", 9_000);
    for index in 0..RECENT_MESSAGES {
        messages.push(ChatMessage::assistant(format!("step {index}")));
    }
    let transcript = render_transcript(&messages, 100_000);
    assert!(transcript.contains(TRANSCRIPT_CUT), "{transcript}");
    assert!(estimate(&transcript) < 4_000, "{}", estimate(&transcript));
    assert!(transcript.contains("step 0") && transcript.contains("fix the parser"));
}

#[test]
fn a_cut_says_it_is_the_transcripts_own() {
    // A live summarizer read codex's `…N tokens truncated…` as the tool's own
    // output having been cut short, and its summary told the agent that the
    // files it had read whole were "truncated in the middle". The mark names
    // the transcript, sits on a line of its own, and never uses that word.
    let mut messages = conversation("fix the parser");
    round(&mut messages, "call_big", "big.rs", 9_000);
    for index in 0..RECENT_MESSAGES {
        messages.push(ChatMessage::assistant(format!("step {index}")));
    }
    let transcript = render_transcript(&messages, 100_000);
    let mark = transcript
        .lines()
        .find(|line| line.contains(TRANSCRIPT_CUT))
        .expect("a cut");
    assert!(mark.starts_with("[… ") && mark.ends_with(" …]"), "{mark}");
    assert!(mark.contains("tokens"), "it counts what is missing: {mark}");
    assert!(!transcript.contains("truncated"), "{transcript}");
}

#[test]
fn the_prompt_says_whose_the_cuts_are() {
    // The mark and the sentence explaining it are one contract.
    assert!(SUMMARIZATION_PROMPT.contains(TRANSCRIPT_CUT));
    assert!(SUMMARIZATION_PROMPT.contains("saw every message and result whole"));
}

#[test]
fn the_handoff_is_the_agents_own_notes() {
    // With the cuts explained, a live summarizer went on to describe its own
    // vantage point instead — "not visible in this transcript", "taken from
    // the earlier summary, not re-verified" — and the agent repeated the
    // caveat to the user. The prompt asks for notes about the work, the
    // bridge for the work to be carried on without a word about either.
    assert!(SUMMARIZATION_PROMPT.contains("never as a description of what you were shown"));
    assert!(SUMMARIZATION_PROMPT.contains("take what it says as what happened"));
    assert!(crate::context::SUMMARY_RESUME_NOTE.contains("no mention of this summary"));
}

#[test]
fn the_newest_messages_are_shown_with_twice_the_room() {
    // What was in progress is what the handoff needs whole.
    let mut messages = conversation("fix the parser");
    // Past an older message's share, inside a newest one's.
    round(&mut messages, "call_new", "new.rs", 2_500);
    let transcript = render_transcript(&messages, 100_000);
    assert!(!transcript.contains(TRANSCRIPT_CUT), "{transcript}");
}

#[test]
fn a_transcript_over_its_budget_is_cut_harder_until_it_fits() {
    let mut messages = conversation("fix the parser");
    for index in 0..30 {
        round(&mut messages, &format!("call_{index}"), "f.rs", 1_500);
    }
    for budget in [20_000, 8_000, 3_000] {
        let transcript = render_transcript(&messages, budget);
        assert!(
            estimate(&transcript) <= budget,
            "{} over {budget}",
            estimate(&transcript)
        );
        assert!(transcript.starts_with("[user]\nfix the parser"), "{budget}");
    }
}

#[test]
fn a_transcript_that_cannot_be_cut_enough_leaves_out_its_oldest_messages() {
    // The request everything answers and the newest work stay; the middle
    // is counted and left out.
    let mut messages = conversation("fix the parser");
    for index in 0..200 {
        round(
            &mut messages,
            &format!("call_{index}"),
            &format!("f{index}.rs"),
            400,
        );
    }
    let transcript = render_transcript(&messages, 2_000);
    assert!(estimate(&transcript) <= 2_000, "{}", estimate(&transcript));
    assert!(transcript.starts_with("[user]\nfix the parser"));
    assert!(transcript.contains("earlier messages left out of this transcript"));
    assert!(!transcript.contains("truncated"), "{transcript}");
    assert!(transcript.contains("f199.rs"), "the newest round is there");
    assert!(!transcript.contains("f5.rs\""), "an old one is not");
}

#[test]
fn a_summarization_is_the_conversation_then_the_prompt() {
    let messages = conversation("fix the parser");
    assert_eq!(
        summary_request(&messages, 1_000),
        format!(
            "<conversation>\n[user]\nfix the parser\n</conversation>\n\n{}",
            SUMMARIZATION_PROMPT.trim_end()
        )
    );
}

#[test]
fn an_idle_compaction_sends_the_conversation_as_one_transcript() {
    // What `/compact` at rest sends in place of the derived context: one
    // user message, the conversation rendered and the prompt under it — the
    // tool call and its result as text, a picture as a mark naming its file.
    use crate::context::ContextToolCall;
    let mut pictured = ContextMessage::new(ContextRole::User, "what is this?");
    pictured.images = vec![std::path::PathBuf::from("/tmp/shot.png")];
    let context = vec![
        ContextMessage::new(ContextRole::User, "fix the parser"),
        ContextMessage::assistant_tool_calls(
            "Reading it.",
            vec![ContextToolCall::new("call_1", "read", r#"{"path":"a.rs"}"#)],
        ),
        ContextMessage::tool_result("call_1", "fn parse() {}"),
        pictured,
    ];
    let sent = summary_context(&context, Some(50_000));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].role, ContextRole::User);
    assert!(sent[0].images.is_empty() && sent[0].tool_calls.is_empty());
    assert_eq!(
        sent[0].text,
        format!(
            "<conversation>\n\
             [user]\nfix the parser\n\n\
             [assistant]\nReading it.\n[tool call] read {{\"path\":\"a.rs\"}}\n\n\
             [tool result: read]\nfn parse() {{}}\n\n\
             [user]\nwhat is this?\n[image: /tmp/shot.png]\n\
             </conversation>\n\n{}",
            SUMMARIZATION_PROMPT.trim_end()
        )
    );
}

#[test]
fn an_idle_compaction_of_a_conversation_past_its_window_still_fits() {
    // The reported dead end: a context that had outgrown the window was
    // sent whole to be summarized, and refused for its size.
    let context: Vec<ContextMessage> = (0..40)
        .flat_map(|index| {
            [
                ContextMessage::assistant_tool_calls(
                    "",
                    vec![crate::context::ContextToolCall::new(
                        format!("call_{index}"),
                        "read",
                        r#"{"path":"f.rs"}"#,
                    )],
                ),
                ContextMessage::tool_result(format!("call_{index}"), filler(3_000)),
            ]
        })
        .collect();
    let sent = summary_context(&context, Some(50_000));
    assert!(
        estimate(&sent[0].text) < 32_000,
        "{} tokens for a 50k window",
        estimate(&sent[0].text)
    );
}

// ----- the summarizer's stream -----

fn collected(events: Vec<StreamEvent>) -> SummaryResult {
    let mut collector = SummaryCollector::default();
    for event in events {
        collector.fold(event);
    }
    collector.finish()
}

#[test]
fn a_summary_is_the_text_its_stream_carried() {
    let result = collected(vec![
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingChunk("what matters here".to_string()),
        StreamEvent::ThinkingEnd,
        StreamEvent::Chunk("The parser ".to_string()),
        StreamEvent::Chunk("is fixed.\n".to_string()),
        StreamEvent::Usage(TokenUsage {
            input: 900,
            output: 12,
            ..TokenUsage::default()
        }),
        StreamEvent::StreamDone,
    ]);
    assert_eq!(
        result.outcome,
        SummaryOutcome::Summary("The parser is fixed.".to_string())
    );
    assert_eq!(result.usage.len(), 1);
    assert_eq!(result.usage[0].input, 900);
}

#[test]
fn a_retried_request_takes_back_what_its_failed_attempt_wrote() {
    // The connection died under the reply; the request sent again writes
    // the summary from its start (docs/offline.md).
    let result = collected(vec![
        StreamEvent::Chunk("The pars".to_string()),
        StreamEvent::Retrying { attempt: 1, max: 3 },
        StreamEvent::Chunk("The parser is fixed.".to_string()),
        StreamEvent::StreamDone,
    ]);
    assert_eq!(
        result.outcome,
        SummaryOutcome::Summary("The parser is fixed.".to_string())
    );
}

#[test]
fn a_retry_keeps_what_the_rounds_before_it_wrote() {
    // A summary the output cap cut off goes on in another round; a retry of
    // that round takes back only its own text.
    let result = collected(vec![
        StreamEvent::Chunk("The parser ".to_string()),
        StreamEvent::HookNote {
            label: "Reply cut off".to_string(),
            text: "continue".to_string(),
            report: None,
        },
        StreamEvent::Chunk("is fi".to_string()),
        StreamEvent::Retrying { attempt: 1, max: 3 },
        StreamEvent::Chunk("is fixed.".to_string()),
        StreamEvent::StreamDone,
    ]);
    assert_eq!(
        result.outcome,
        SummaryOutcome::Summary("The parser is fixed.".to_string())
    );
    assert!(result.cells.is_empty(), "a note with no report is no cell");
}

#[test]
fn a_stream_that_failed_says_why() {
    let result = collected(vec![
        StreamEvent::Chunk("The".to_string()),
        StreamEvent::Error("HTTP 500: upstream".to_string()),
    ]);
    assert_eq!(
        result.outcome,
        SummaryOutcome::Failed("HTTP 500: upstream".to_string())
    );
}

#[test]
fn a_stream_refused_for_its_size_is_an_overflow() {
    let result = collected(vec![StreamEvent::Error(
        "HTTP 400: This model's maximum context length is 8192 tokens".to_string(),
    )]);
    assert!(matches!(result.outcome, SummaryOutcome::Overflow(_)));
}

#[test]
fn a_stream_that_never_ended_was_cancelled() {
    let result = collected(vec![StreamEvent::Chunk("The pars".to_string())]);
    assert_eq!(result.outcome, SummaryOutcome::Cancelled);
}

#[test]
fn the_compact_hooks_cells_ride_the_result() {
    let report = HookReport {
        event: "PostCompact".to_string(),
        hooks: Vec::new(),
        warnings: vec!["said something".to_string()],
    };
    let result = collected(vec![
        StreamEvent::Chunk("done".to_string()),
        StreamEvent::HookNote {
            label: "PostCompact hook".to_string(),
            text: String::new(),
            report: Some(report.clone()),
        },
        StreamEvent::StreamDone,
    ]);
    assert_eq!(result.cells, vec![("PostCompact hook".to_string(), report)]);
}

// ----- telling an overflow apart -----

#[test]
fn every_providers_words_for_an_overflow_are_known() {
    for body in [
        // OpenAI and everything shaped like it.
        r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130211 tokens. Please reduce the length of the messages.","code":"context_length_exceeded"}}"#,
        // The Responses API.
        "Your input exceeds the context window of this model. Please adjust your input and try again.",
        // Anthropic.
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 213462 tokens > 200000 maximum"}}"#,
        r#"{"error":{"message":"input length and `max_tokens` exceed context limit: 197202 + 8192 > 200000"}}"#,
        // Gemini.
        "The input token count (1204820) exceeds the maximum number of tokens allowed (1048575).",
        // OpenRouter.
        "This endpoint's maximum context length is 65536 tokens. However, you requested about 70021 tokens",
        // llama.cpp, LM Studio, the rewritten advice for it.
        "the request exceeds the available context size, try increasing it",
        "The number of tokens to keep from the initial prompt is greater than the context length.",
        "qwen/qwen3-8b is loaded with a context window too small for this conversation",
        // Bedrock.
        "Input is too long for requested model.",
    ] {
        assert!(names_context_overflow(body), "{body}");
        assert!(
            is_context_overflow(&LlmError::Api {
                status: 400,
                body: body.to_string()
            }),
            "{body}"
        );
    }
}

#[test]
fn any_other_refusal_is_no_overflow() {
    for body in [
        "",
        "Invalid API key",
        r#"{"error":{"message":"model_not_supported"}}"#,
        "Rate limit reached for requests",
        "The server had an error while processing your request",
        "tool_choice is not supported with this model",
    ] {
        assert!(!names_context_overflow(body), "{body}");
    }
}

#[test]
fn a_refusal_about_who_is_asking_is_never_an_overflow() {
    // A rate limit counted in tokens mentions them too.
    for status in [401, 403, 429] {
        assert!(!is_context_overflow(&LlmError::Api {
            status,
            body: "too many tokens per minute".to_string(),
        }));
    }
    assert!(!is_context_overflow(&LlmError::Http(
        "context length".to_string()
    )));
    assert!(!is_context_overflow(&LlmError::Cancelled));
}

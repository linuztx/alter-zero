//! The ask gate's backend half (`docs/ask.md`): turn an `askuserquestion`
//! call into the [`AskRequest`] the modal shows, and block the tool thread on
//! the user's decision — [`crate::llm::approval`]'s sibling, for questions
//! instead of permissions.
//!
//! Pure except for the blocking itself: parsing and every text the resolution
//! produces live in [`crate::ask`], so the whole round trip is unit-tested
//! with a plain thread and no terminal.

use tokio::sync::mpsc::UnboundedSender;

use super::tools::{ToolCallRequest, ToolOutcome};
use crate::ask::{
    AskDecision, AskGate, AskRequest, answered_display, answered_result, chat_display, chat_result,
    declined_display, declined_result, parse_questions,
};
use crate::stream::{CancelToken, StreamEvent};

/// The display recorded when the turn is cancelled out from under a waiting
/// question — the call never resolved, and the events go to an
/// already-swapped channel, so this is only ever a well-formed placeholder
/// (the approval seam's `CANCELLED_DISPLAY`).
const CANCELLED_DISPLAY: &str = "Interrupted by user";

/// Run one `askuserquestion` call: parse the questions, raise the modal
/// ([`StreamEvent::AskUser`]), **block this thread** on the gate until the
/// user decides (or the turn is cancelled), and map the decision onto the
/// split [`ToolOutcome`] — the displayed cell text in `output`, the
/// model-facing result in `context` — that `run_agent` surfaces as
/// `ToolAnswered` (submitted) or `ToolRejected` (declined / chat).
///
/// Unparseable arguments resolve as a recoverable error without raising
/// anything — the model reads the message and retries, like every other
/// tool's argument failure.
#[must_use]
pub fn ask_user(
    gate: &AskGate,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    call: &ToolCallRequest,
) -> ToolOutcome {
    let questions = match parse_questions(&call.arguments) {
        Ok(questions) => questions,
        Err(message) => return ToolOutcome::error(message),
    };
    let request = AskRequest {
        id: gate.next_id(),
        questions,
    };
    let _ = tx.send(StreamEvent::AskUser(request.clone()));
    match gate.wait(&request.id, &|| cancel.is_cancelled()) {
        Some(AskDecision::Submitted(answers)) => {
            ToolOutcome::ok(answered_display(&answers)).with_context(answered_result(&answers))
        }
        Some(AskDecision::Declined) => {
            ToolOutcome::error(declined_display(&request.questions)).with_context(declined_result())
        }
        Some(AskDecision::Chat) => {
            ToolOutcome::error(chat_display(&request.questions)).with_context(chat_result())
        }
        // The cancel that reaps a waiting thread (Esc, `/clear`, quit): the
        // turn is being torn down, so these texts only ever reach an
        // abandoned channel.
        None => ToolOutcome::error(CANCELLED_DISPLAY).with_context(declined_result()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::AskAnswer;

    fn call(args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: crate::llm::tools::ASK_TOOL_NAME.to_string(),
            arguments: args.to_string(),
        }
    }

    const VALID_ARGS: &str = r#"{"questions":[{
        "question":"Pick one?",
        "header":"Pick",
        "options":[
            {"label":"A","description":"first"},
            {"label":"B","description":"second"}
        ],
        "multiSelect":false
    }]}"#;

    #[test]
    fn bad_arguments_resolve_recoverably_without_raising_the_modal() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = AskGate::new();
        let outcome = ask_user(&gate, &tx, &CancelToken::new(), &call("not json"));
        assert!(!outcome.ok);
        assert!(outcome.output.contains("invalid tool arguments"));
        assert!(outcome.context.is_none(), "an argument error has one text");
        assert!(rx.try_recv().is_err(), "nothing was asked");
    }

    #[test]
    fn a_request_is_raised_and_the_submitted_answers_come_back_split() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = AskGate::new();
        let cancel = CancelToken::new();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || ask_user(&gate, &tx, &cancel, &call(VALID_ARGS)))
        };
        let request = loop {
            if let Ok(StreamEvent::AskUser(request)) = rx.try_recv() {
                break request;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(request.questions.len(), 1);
        gate.resolve(
            &request.id,
            AskDecision::Submitted(vec![AskAnswer {
                question: "Pick one?".to_string(),
                labels: vec!["A".to_string()],
                notes: None,
                preview: None,
            }]),
        );
        let outcome = waiter.join().unwrap();
        assert!(outcome.ok, "a submission is green");
        assert!(
            outcome.output.starts_with(crate::ask::ANSWERED_HEADLINE),
            "got {}",
            outcome.output
        );
        assert!(outcome.output.contains("· Pick one? → A"));
        let context = outcome.context.expect("the model reads the answers JSON");
        let value: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert_eq!(value["answers"]["Pick one?"], "A");
    }

    #[test]
    fn a_decline_and_a_chat_resolve_red_with_their_stop_and_wait_results() {
        for (decision, headline, needle) in [
            (
                AskDecision::Declined,
                crate::ask::DECLINED_HEADLINE,
                "declined",
            ),
            (
                AskDecision::Chat,
                crate::ask::CHAT_HEADLINE,
                "Chat about this",
            ),
        ] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let gate = AskGate::new();
            let cancel = CancelToken::new();
            let waiter = {
                let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
                std::thread::spawn(move || ask_user(&gate, &tx, &cancel, &call(VALID_ARGS)))
            };
            let request = loop {
                if let Ok(StreamEvent::AskUser(request)) = rx.try_recv() {
                    break request;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            gate.resolve(&request.id, decision);
            let outcome = waiter.join().unwrap();
            assert!(!outcome.ok, "nothing was answered");
            assert!(
                outcome.output.starts_with(headline),
                "got {}",
                outcome.output
            );
            assert!(
                outcome.output.contains("· Pick one? (A / B)"),
                "the cell names what went unanswered: {}",
                outcome.output
            );
            let context = outcome.context.expect("the model reads the instruction");
            assert!(context.contains(needle), "got {context}");
        }
    }

    #[test]
    fn a_cancelled_turn_releases_the_waiting_call_without_answers() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = AskGate::new();
        let cancel = CancelToken::new();
        let waiter = {
            let (gate, tx, cancel) = (gate.clone(), tx.clone(), cancel.clone());
            std::thread::spawn(move || ask_user(&gate, &tx, &cancel, &call(VALID_ARGS)))
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        cancel.cancel();
        let outcome = waiter.join().unwrap();
        assert!(!outcome.ok, "a reaped wait never counts as answered");
        assert_eq!(outcome.output, CANCELLED_DISPLAY);
    }
}

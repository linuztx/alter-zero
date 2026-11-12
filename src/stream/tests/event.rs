//! The reply protocol's own vocabulary ([`crate::stream::StreamEvent`]).

use super::*;
use crate::stream::{TokenUsage, ToolCallFragment, ToolCallSummary, ToolErrorKind, ToolRun};

#[test]
fn only_what_a_streaming_request_emits_is_attempt_output() {
    // What a retry voids (docs/providers/offline.md *A connection lost mid-reply*):
    // the reply text, a thinking phase, a call the model is still generating
    // — everything a request emits while it streams, and nothing that marks
    // where one stopped or the next began.
    let output = [
        StreamEvent::Chunk("Half a rep".into()),
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingChunk("weighing".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::ToolCallDelta(ToolCallFragment {
            index: 0,
            name: "write".into(),
            arguments: "{\"pa".into(),
        }),
    ];
    for event in output {
        assert!(event.is_attempt_output(), "{event:?} is attempt output");
    }
    let boundaries = [
        StreamEvent::Retrying { attempt: 1, max: 3 },
        StreamEvent::Offline {
            host: "api.venice.ai".into(),
            attempts: 1,
        },
        StreamEvent::Usage(TokenUsage::default()),
        StreamEvent::RoundCalls(Vec::new()),
        StreamEvent::ToolBatch(vec![ToolCallSummary {
            name: "Bash".into(),
            args: "ls".into(),
        }]),
        StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "ls".into(),
            detail: None,
            arguments: None,
        },
        StreamEvent::ToolEnd {
            output: "src".into(),
            ok: true,
            truncated: false,
            run: crate::stream::ToolRun::default(),
        },
        StreamEvent::Steered {
            text: "and the tests".into(),
        },
        StreamEvent::NoticeDelivered { seq: 1 },
        StreamEvent::HookNote {
            label: "Stop hook".into(),
            text: "keep going".into(),
            report: None,
        },
        StreamEvent::Error("boom".into()),
        StreamEvent::StreamDone,
    ];
    for event in boundaries {
        assert!(
            !event.is_attempt_output(),
            "{event:?} is not attempt output"
        );
    }
}

// ===== how a call ran (docs/state/rollout-records.md) =====

#[test]
fn every_error_kind_reads_back_from_its_rollout_name() {
    // One spelling per kind, codex's where codex has one, and each reads back
    // as the kind that wrote it.
    let names: Vec<&str> = ToolErrorKind::ALL.iter().map(|kind| kind.wire()).collect();
    assert_eq!(
        names,
        [
            "tool_error",
            "timed_out",
            "interrupted",
            "rejected",
            "classifier_denied",
            "hook_blocked",
            "approval_required",
            "tool_call_limit",
            "backend_error",
        ]
    );
    for kind in ToolErrorKind::ALL {
        assert_eq!(
            ToolErrorKind::from_wire(kind.wire()),
            Some(kind),
            "{kind:?}"
        );
    }
}

#[test]
fn a_name_this_build_does_not_know_is_no_kind() {
    // A newer build's kind reads as absent rather than as a wrong one.
    assert_eq!(ToolErrorKind::from_wire("sandbox_denied"), None);
    assert_eq!(ToolErrorKind::from_wire(""), None);
    assert_eq!(ToolErrorKind::from_wire("Tool_Error"), None);
}

#[test]
fn a_refused_call_names_why_and_never_ran() {
    // A call that never ran has no exit code and no duration — only the reason.
    assert_eq!(
        ToolRun::refused(ToolErrorKind::Rejected),
        ToolRun {
            exit_code: None,
            duration_ms: None,
            error_kind: Some(ToolErrorKind::Rejected),
        }
    );
    assert_eq!(ToolRun::default().error_kind, None);
}

#[test]
fn a_duration_is_whole_milliseconds() {
    use std::time::Duration;
    assert_eq!(ToolRun::millis(Duration::from_micros(48_213_999)), 48_213);
    assert_eq!(ToolRun::millis(Duration::ZERO), 0);
    assert_eq!(ToolRun::millis(Duration::MAX), u64::MAX);
}

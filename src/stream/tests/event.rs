//! The reply protocol's own vocabulary ([`crate::stream::StreamEvent`]).

use super::*;
use crate::stream::{TokenUsage, ToolCallFragment, ToolCallSummary};

#[test]
fn only_what_a_streaming_request_emits_is_attempt_output() {
    // What a retry voids (docs/offline.md *A connection lost mid-reply*):
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
        },
        StreamEvent::Steered {
            text: "and the tests".into(),
        },
        StreamEvent::NoticeDelivered { seq: 1 },
        StreamEvent::HookNote {
            label: "Stop hook".into(),
            text: "keep going".into(),
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

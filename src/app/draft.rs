//! The round's **draft calls**: the tool calls the model is still generating,
//! their arguments decoded as they stream, so a `write` cell can show its file
//! before the call is whole. Display state only — a draft is never history.
//! See `docs/write-streaming.md`.

use super::*;
use crate::partial_json::{Field, StringFields};
use crate::stream::ToolCallFragment;
use std::sync::atomic::{AtomicU64, Ordering};

/// The arguments a draft decodes as they stream — the two a `write` cell
/// shows. Every call decodes them (a call's name can arrive after its first
/// arguments), and only a `write` is ever drawn.
const DRAFT_FIELDS: &[&str] = &["path", "content"];

/// The wire name of the one tool whose draft renders.
const WRITE_TOOL: &str = "write";

/// Where the next draft's id comes from — process-wide, so no two drafts
/// share one, whichever conversation (or test thread) made them.
static NEXT_DRAFT_ID: AtomicU64 = AtomicU64::new(1);

/// One tool call the model is generating: its pieces folded in as they land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftCall {
    id: u64,
    /// The key its pieces share ([`ToolCallFragment::index`]).
    key: usize,
    name: Option<String>,
    fields: StringFields,
}

impl DraftCall {
    fn new(key: usize) -> Self {
        Self {
            id: NEXT_DRAFT_ID.fetch_add(1, Ordering::Relaxed),
            key,
            name: None,
            fields: StringFields::new(DRAFT_FIELDS),
        }
    }

    /// A process-unique id — what the renderer's memo keys its incremental
    /// highlighting on, so a new round's call never inherits an old one's.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// The call's wire tool name, once a piece carried it.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Is this a `write` — the one draft that renders?
    #[must_use]
    pub fn is_write(&self) -> bool {
        self.name.as_deref() == Some(WRITE_TOOL)
    }

    /// The `path` argument — only once its string has closed, so a header
    /// built from it never reflows character by character.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.fields
            .field("path")
            .filter(|field| field.complete)
            .map(|field| field.text.as_str())
    }

    /// The `content` argument as decoded so far, once its string has opened.
    #[must_use]
    pub fn content(&self) -> Option<&Field> {
        self.fields.field("content")
    }
}

/// The round's draft calls, in the order they began streaming.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftCalls {
    calls: Vec<DraftCall>,
}

impl DraftCalls {
    /// Fold one piece in. Returns whether a `write` draft became visible with
    /// it — the cue to finalise the run of reply text before the cell.
    pub fn push(&mut self, fragment: &ToolCallFragment) -> bool {
        let opens = self.would_open(fragment);
        let at = match self.calls.iter().position(|c| c.key == fragment.index) {
            Some(at) => at,
            None => {
                self.calls.push(DraftCall::new(fragment.index));
                self.calls.len() - 1
            }
        };
        let call = &mut self.calls[at];
        if let Some(name) = fragment.name.as_deref().filter(|name| !name.is_empty()) {
            call.name = Some(name.to_string());
        }
        call.fields.feed(&fragment.arguments);
        opens
    }

    /// Would folding `fragment` make a `write` draft visible? Asked before
    /// the fold by a boundary that must commit the text ahead of the cell.
    #[must_use]
    pub fn would_open(&self, fragment: &ToolCallFragment) -> bool {
        if fragment.name.as_deref() != Some(WRITE_TOOL) {
            return false;
        }
        !self
            .calls
            .iter()
            .any(|c| c.key == fragment.index && c.is_write())
    }

    /// Drop every draft — the round's calls are whole, or the turn is over.
    pub fn clear(&mut self) {
        self.calls.clear();
    }

    /// Is no call being generated?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The `write` drafts — the ones that render — in stream order.
    pub fn writes(&self) -> impl Iterator<Item = &DraftCall> {
        self.calls.iter().filter(|call| call.is_write())
    }

    /// Is any `write` draft showing?
    #[must_use]
    pub fn has_writes(&self) -> bool {
        self.calls.iter().any(DraftCall::is_write)
    }
}

/// What one piece of a call costs the live tally: its name and its
/// arguments, both model output. Shared with the subagent fold, so a
/// session view's tally ticks the same way (`docs/status-indicator.md`).
#[must_use]
pub(crate) fn fragment_tokens(fragment: &ToolCallFragment) -> usize {
    fragment.name.as_deref().map_or(0, count_tokens) + count_tokens(&fragment.arguments)
}

impl App {
    /// Fold one piece of a tool call the model is generating
    /// ([`crate::stream::StreamEvent::ToolCallDelta`]): count its name and its
    /// arguments into the live tally (arrow down — model output, the
    /// reasoning rule, `docs/status-indicator.md`) and decode it into the
    /// round's drafts. Returns whether a `write` draft became visible with it
    /// — the boundary's cue to finalise the text before the cell (invariant
    /// 4's flush-before-you-interleave). A no-op when no turn is in flight.
    pub fn push_tool_call_fragment(&mut self, fragment: &ToolCallFragment) -> bool {
        let Some(status) = self.status.as_mut() else {
            return false;
        };
        status.tokens += fragment_tokens(fragment);
        status.arrow = TokenArrow::Down;
        // Generating the call is content too — a retrying (or waiting)
        // request got through.
        status.recovered();
        self.drafts.push(fragment)
    }

    /// The round's draft calls — what the strip's live `write` cell renders.
    #[must_use]
    pub fn drafts(&self) -> &DraftCalls {
        &self.drafts
    }
}

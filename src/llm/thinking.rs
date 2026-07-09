//! Splits reasoning out of a streamed completion.
//!
//! Two provider conventions are normalised here into one `(response, reasoning)`
//! pair per fed chunk:
//!
//! - **Native reasoning** — the delta arrives on its own `reasoning` /
//!   `reasoning_content` field (DeepSeek, some OpenRouter models). Once any
//!   native reasoning is seen the splitter passes both fields straight through.
//! - **Inline tags** — the reasoning is wrapped in `<think>…</think>` (or
//!   `<reasoning>…</reasoning>`) inside the ordinary content stream (many local
//!   models). The splitter peels the tags out, buffering a chunk that ends
//!   mid-tag so a boundary split across two SSE frames is still recognised.
//!
//! Pure and unit-tested; the `openai` client feeds it and the `backend` bridge
//! turns the reasoning delta into the `ThinkingStart`/`ThinkingChunk`/
//! `ThinkingEnd` events that drive the `Thinking for Ns` status.

/// Final accumulated text after the stream completes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatStreamResult {
    pub response: String,
    pub reasoning: String,
}

/// The reasoning tag pairs recognised in the inline convention.
const PAIRS: &[(&str, &str)] = &[("<think>", "</think>"), ("<reasoning>", "</reasoning>")];

/// Peels `<think>`/`<reasoning>` tags (or native reasoning deltas) out of a
/// streamed response, exposing the reasoning separately so the TUI can drive its
/// thinking status without ever rendering the chain-of-thought.
#[derive(Debug, Default, Clone)]
pub struct ThinkingSplitter {
    response: String,
    reasoning: String,
    thinking: bool,
    closing_tag: &'static str,
    /// Accumulated bytes that may form a partial opening/closing tag.
    pending: String,
    native: bool,
    first_chunk: bool,
}

impl ThinkingSplitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            first_chunk: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn response(&self) -> &str {
        &self.response
    }

    #[must_use]
    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }

    /// Feed a raw chunk. Returns `(new_response_delta, new_reasoning_delta)` —
    /// the parts the caller should surface this round.
    pub fn feed(&mut self, response_delta: &str, reasoning_delta: &str) -> (String, String) {
        if !reasoning_delta.is_empty() {
            self.native = true;
        }

        let (mut resp_out, reason_out) = if self.native {
            (response_delta.to_string(), reasoning_delta.to_string())
        } else {
            self.process_thinking(response_delta)
        };

        // Trim leading whitespace on the first response text we surface, so a
        // model that opens with a blank line doesn't push the reply down. An
        // all-whitespace delta trims to nothing and must NOT burn the flag —
        // the next delta still opens the reply.
        if self.first_chunk && !resp_out.is_empty() {
            resp_out = resp_out.trim_start().to_string();
            if !resp_out.is_empty() {
                self.first_chunk = false;
            }
        }

        if !resp_out.is_empty() {
            self.response.push_str(&resp_out);
        }
        if !reason_out.is_empty() {
            self.reasoning.push_str(&reason_out);
        }
        (resp_out, reason_out)
    }

    fn process_thinking(&mut self, response_delta: &str) -> (String, String) {
        let mut response = std::mem::take(&mut self.pending);
        response.push_str(response_delta);
        let mut reasoning = String::new();

        if self.thinking {
            if let Some(idx) = response.find(self.closing_tag) {
                reasoning.push_str(&response[..idx]);
                let rest = response[idx + self.closing_tag.len()..].to_string();
                self.thinking = false;
                self.closing_tag = "";
                return (rest, reasoning);
            }
            if self.is_partial_closing(&response) {
                self.pending = response;
                return (String::new(), reasoning);
            }
            reasoning.push_str(&response);
            return (String::new(), reasoning);
        }

        // Opening tags are recognised only at the reply's START (`first_chunk`
        // — nothing visible surfaced yet). Mid-reply detection was
        // chunk-boundary-dependent: a chunk that *happened* to begin with
        // "<think>" — say, the model writing about the tag — silently hid real
        // reply text, while the same text split differently passed through.
        // The models that use the inline convention open with the tag.
        if !self.first_chunk {
            return (response, reasoning);
        }
        // Whitespace may precede the tag ("\n<think>" is common) — strip it
        // *before* matching, or the tag is never seen at the start and the
        // whole chain-of-thought leaks into the visible reply. The surfaced
        // text loses that whitespace to the first-chunk trim anyway.
        if response.starts_with(char::is_whitespace) {
            response = response.trim_start().to_string();
        }

        for (open, close) in PAIRS {
            if response.starts_with(open) {
                response.drain(..open.len());
                self.thinking = true;
                self.closing_tag = close;
                if let Some(idx) = response.find(close) {
                    reasoning.push_str(&response[..idx]);
                    let rest = response[idx + close.len()..].to_string();
                    self.thinking = false;
                    self.closing_tag = "";
                    return (rest, reasoning);
                }
                if self.is_partial_closing(&response) {
                    self.pending = response;
                    return (String::new(), reasoning);
                }
                reasoning.push_str(&response);
                return (String::new(), reasoning);
            }
            if response.len() < open.len() && is_partial_prefix(&response, open) {
                self.pending = response.clone();
                return (String::new(), reasoning);
            }
        }

        (response, reasoning)
    }

    fn is_partial_closing(&self, text: &str) -> bool {
        if self.closing_tag.is_empty() || text.is_empty() {
            return false;
        }
        let max = text.len().min(self.closing_tag.len() - 1);
        for i in 1..=max {
            if text.ends_with(&self.closing_tag[..i]) {
                return true;
            }
        }
        false
    }

    /// Surface whatever is still buffered mid-tag as a final delta, draining the
    /// pending buffer into the accumulators — so a stream that ends inside a
    /// partial `<think>`/`</think>` fragment doesn't drop its tail. Returns the
    /// `(response_delta, reasoning_delta)` the caller should emit (empty when
    /// nothing was buffered). Call this at end-of-stream *before* [`finish`];
    /// [`finish`] itself also drains the buffer, so a caller that skips `flush`
    /// still loses nothing (it just never surfaces the tail as a streamed delta).
    ///
    /// [`finish`]: ThinkingSplitter::finish
    pub fn flush(&mut self) -> (String, String) {
        if self.pending.is_empty() {
            return (String::new(), String::new());
        }
        let pending = std::mem::take(&mut self.pending);
        if self.thinking {
            self.reasoning.push_str(&pending);
            (String::new(), pending)
        } else {
            self.response.push_str(&pending);
            (pending, String::new())
        }
    }

    /// Flush whatever is still buffered as its current kind ([`flush`]),
    /// returning the final accumulated split.
    ///
    /// [`flush`]: ThinkingSplitter::flush
    #[must_use]
    pub fn finish(mut self) -> ChatStreamResult {
        let _ = self.flush();
        ChatStreamResult {
            response: self.response,
            reasoning: self.reasoning,
        }
    }
}

/// Is `text` a strict, non-empty prefix of `full`? (a chunk that could still
/// grow into the opening tag `full`).
fn is_partial_prefix(text: &str, full: &str) -> bool {
    if text.is_empty() || text.len() >= full.len() {
        return false;
    }
    full.starts_with(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_reasoning_passes_through() {
        let mut s = ThinkingSplitter::new();
        let (r, t) = s.feed("hello", "thinking");
        assert_eq!(r, "hello");
        assert_eq!(t, "thinking");
    }

    #[test]
    fn think_tags_are_split_out() {
        let mut s = ThinkingSplitter::new();
        let (_r, _t) = s.feed("<think>plan</think>actual", "");
        let res = s.finish();
        assert_eq!(res.reasoning, "plan");
        assert_eq!(res.response, "actual");
    }

    #[test]
    fn partial_open_tag_is_buffered_across_chunks() {
        let mut s = ThinkingSplitter::new();
        let (r, _t) = s.feed("<thi", "");
        assert!(r.is_empty(), "a chunk ending mid-open-tag surfaces nothing");
        let (r, _t) = s.feed("nk>x</think>y", "");
        assert_eq!(r, "y");
    }

    #[test]
    fn partial_closing_tag_is_buffered_across_chunks() {
        let mut s = ThinkingSplitter::new();
        let (_r, t) = s.feed("<think>plan", "");
        assert_eq!(t, "plan");
        // The closing tag is split across the frame boundary: a chunk whose tail
        // could be a partial "</think>" is buffered whole until the next frame,
        // so nothing leaks as response text meanwhile.
        let (r, _t) = s.feed("more</thi", "");
        assert!(
            r.is_empty(),
            "no response text leaks while the tag may be closing"
        );
        let (r, t) = s.feed("nk>done", "");
        assert_eq!(r, "done");
        assert_eq!(
            t, "more",
            "the buffered reasoning flushes when the tag resolves"
        );
        let res = s.finish();
        assert_eq!(res.reasoning, "planmore");
        assert_eq!(res.response, "done");
    }

    #[test]
    fn reasoning_tag_variant_is_recognised() {
        let mut s = ThinkingSplitter::new();
        s.feed("<reasoning>why</reasoning>because", "");
        let res = s.finish();
        assert_eq!(res.reasoning, "why");
        assert_eq!(res.response, "because");
    }

    #[test]
    fn first_chunk_strips_leading_whitespace() {
        let mut s = ThinkingSplitter::new();
        let (r, _) = s.feed("   hi", "");
        assert_eq!(r, "hi");
    }

    #[test]
    fn plain_text_without_tags_passes_through() {
        let mut s = ThinkingSplitter::new();
        let (r, t) = s.feed("just text", "");
        assert_eq!(r, "just text");
        assert!(t.is_empty());
    }

    #[test]
    fn finish_flushes_a_dangling_open_tag_as_reasoning() {
        // An unterminated <think> that ends the stream must not vanish.
        let mut s = ThinkingSplitter::new();
        s.feed("<think>still going", "");
        let res = s.finish();
        assert_eq!(res.reasoning, "still going");
        assert!(res.response.is_empty());
    }

    #[test]
    fn flush_surfaces_a_dangling_partial_open_tag() {
        // A stream that ends mid-open-tag buffered "<thi" and surfaced nothing;
        // flush must emit it as a final response delta so it isn't dropped.
        let mut s = ThinkingSplitter::new();
        let (r, _t) = s.feed("<thi", "");
        assert!(r.is_empty(), "buffered, nothing surfaced yet");
        let (r, t) = s.flush();
        assert_eq!(r, "<thi");
        assert!(t.is_empty());
        assert_eq!(s.response(), "<thi");
    }

    #[test]
    fn flush_of_an_empty_buffer_is_empty() {
        let mut s = ThinkingSplitter::new();
        s.feed("plain text", "");
        assert_eq!(s.flush(), (String::new(), String::new()));
    }

    #[test]
    fn flush_surfaces_buffered_reasoning_while_thinking() {
        // Inside a <think> block, a chunk ending on a partial "</thi" buffers as
        // reasoning; flush emits the reasoning delta, not response.
        let mut s = ThinkingSplitter::new();
        s.feed("<think>plan", "");
        s.feed("more</thi", ""); // tail could be a closing tag → buffered
        let (r, t) = s.flush();
        assert!(r.is_empty());
        assert_eq!(t, "more</thi");
    }

    #[test]
    fn accumulators_track_the_full_split() {
        let mut s = ThinkingSplitter::new();
        s.feed("<think>a", "");
        s.feed("b</think>c", "");
        assert_eq!(s.reasoning(), "ab");
        assert_eq!(s.response(), "c");
    }

    #[test]
    fn leading_whitespace_before_the_open_tag_still_splits() {
        // Models often open with "\n<think>" — the whitespace must not defeat
        // tag detection, or the whole chain-of-thought leaks into the visible
        // reply.
        let mut s = ThinkingSplitter::new();
        s.feed("\n<think>plan</think>hi", "");
        let res = s.finish();
        assert_eq!(res.reasoning, "plan");
        assert_eq!(res.response, "hi");
    }

    #[test]
    fn whitespace_then_partial_open_tag_across_chunks_still_splits() {
        // The same, split across SSE frames: "\n<thi" + "nk>plan</think>ok".
        let mut s = ThinkingSplitter::new();
        let (r, _t) = s.feed("\n<thi", "");
        assert!(r.is_empty(), "possible tag start is buffered, not surfaced");
        let (r, t) = s.feed("nk>plan</think>ok", "");
        assert_eq!(r, "ok");
        assert_eq!(t, "plan");
    }

    #[test]
    fn whitespace_only_first_delta_keeps_trimming_the_next() {
        // An all-whitespace first delta must not burn the one leading trim —
        // the *next* delta still opens the reply and gets trimmed.
        let mut s = ThinkingSplitter::new();
        let (r, _t) = s.feed("\n\n", "");
        assert_eq!(r, "");
        let (r, _t) = s.feed("  hi", "");
        assert_eq!(r, "hi");
        assert_eq!(s.response(), "hi");
    }

    #[test]
    fn open_tags_after_visible_text_are_plain_content() {
        // Tag detection is gated to the reply's start: once visible text has
        // streamed, a chunk that happens to begin with "<think>" is content
        // the model wrote, not a reasoning block — swallowing it (the old,
        // chunk-boundary-dependent behaviour) silently hid real reply text.
        let mut s = ThinkingSplitter::new();
        s.feed("literal tags: ", "");
        let (r, t) = s.feed("<think>is markup</think>", "");
        assert_eq!(r, "<think>is markup</think>");
        assert!(t.is_empty());
    }
}

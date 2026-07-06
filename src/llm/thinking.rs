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
            self.process_thinking(response_delta, reasoning_delta)
        };

        // Trim leading whitespace once, on the first response text we surface, so
        // a model that opens with a blank line doesn't push the reply down.
        if self.first_chunk && !resp_out.is_empty() {
            resp_out = resp_out.trim_start().to_string();
            self.first_chunk = false;
        }

        if !resp_out.is_empty() {
            self.response.push_str(&resp_out);
        }
        if !reason_out.is_empty() {
            self.reasoning.push_str(&reason_out);
        }
        (resp_out, reason_out)
    }

    fn process_thinking(
        &mut self,
        response_delta: &str,
        reasoning_delta: &str,
    ) -> (String, String) {
        let mut response = std::mem::take(&mut self.pending);
        response.push_str(response_delta);
        let mut reasoning = String::from(reasoning_delta);

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

    /// Flush whatever is still buffered as its current kind, returning the final
    /// accumulated split.
    #[must_use]
    pub fn finish(mut self) -> ChatStreamResult {
        if !self.pending.is_empty() {
            if self.thinking {
                let pending = std::mem::take(&mut self.pending);
                self.reasoning.push_str(&pending);
            } else {
                let pending = std::mem::take(&mut self.pending);
                self.response.push_str(&pending);
            }
        }
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
    fn accumulators_track_the_full_split() {
        let mut s = ThinkingSplitter::new();
        s.feed("<think>a", "");
        s.feed("b</think>c", "");
        assert_eq!(s.reasoning(), "ab");
        assert_eq!(s.response(), "c");
    }
}

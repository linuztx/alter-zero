//! A pure Server-Sent-Events frame parser (`docs/mcp.md`) — shared by the
//! streamable-HTTP response drain and the legacy HTTP+SSE stream, and tested
//! without a socket. Feed it lines (or raw chunks) as they arrive; it yields
//! complete events at each blank-line boundary.
//!
//! The subset of the SSE spec both MCP transports need: `event:` names,
//! multi-line `data:` accumulation (joined with `\n`), comment (`:`) and
//! unknown-field lines ignored, and the optional single leading space after
//! the colon stripped. `id:`/`retry:` are ignored — the MCP client never
//! resumes a stream by id.

/// One complete SSE event: its `event:` name (`message` when the stream
/// didn't name one — the SSE default) and the joined `data:` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// The incremental parser: push lines, collect events.
#[derive(Debug, Default)]
pub struct SseParser {
    event: Option<String>,
    data: Vec<String>,
    /// Carry for a chunk that ended mid-line ([`push_chunk`](Self::push_chunk)).
    partial: String,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one **line** (no trailing newline). Returns the completed event
    /// when this line was the blank separator ending one.
    pub fn push_line(&mut self, line: &str) -> Option<SseEvent> {
        // A CR left by a CRLF split is not content.
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.take_event();
        }
        if let Some(rest) = line.strip_prefix(':') {
            let _ = rest; // comment — ignored
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {} // id/retry/unknown — ignored
        }
        None
    }

    /// Feed a raw **chunk** (any split point). Returns every event completed
    /// within it; a trailing partial line is carried to the next push.
    pub fn push_chunk(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let mut buffer = std::mem::take(&mut self.partial);
        buffer.push_str(chunk);
        let mut start = 0;
        while let Some(nl) = buffer[start..].find('\n') {
            let line = &buffer[start..start + nl];
            if let Some(event) = self.push_line(line) {
                events.push(event);
            }
            start += nl + 1;
        }
        self.partial = buffer[start..].to_string();
        events
    }

    /// The pending event, if any data accumulated — the blank-line flush.
    fn take_event(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_none() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        let event = self.event.take().unwrap_or_else(|| "message".to_string());
        // A name with no data is dispatchable per spec, but neither MCP
        // transport sends one — treat it as noise rather than surfacing an
        // empty frame the JSON parse would then reject.
        if data.is_empty() {
            return None;
        }
        Some(SseEvent { event, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_line_and_a_blank_complete_an_event() {
        let mut p = SseParser::new();
        assert_eq!(p.push_line("data: {\"a\":1}"), None);
        assert_eq!(
            p.push_line(""),
            Some(SseEvent {
                event: "message".to_string(),
                data: "{\"a\":1}".to_string(),
            })
        );
    }

    #[test]
    fn named_events_and_multi_line_data_join() {
        let mut p = SseParser::new();
        p.push_line("event: endpoint");
        p.push_line("data: /messages?session=1");
        let ev = p.push_line("").unwrap();
        assert_eq!(ev.event, "endpoint");
        assert_eq!(ev.data, "/messages?session=1");

        p.push_line("data: line one");
        p.push_line("data: line two");
        assert_eq!(p.push_line("").unwrap().data, "line one\nline two");
    }

    #[test]
    fn comments_ids_and_crlf_are_tolerated() {
        let mut p = SseParser::new();
        p.push_line(": keep-alive");
        p.push_line("id: 42");
        p.push_line("retry: 100");
        p.push_line("data: x\r");
        let ev = p.push_line("\r").unwrap();
        assert_eq!(ev.data, "x");
        // The bare blank after an already-flushed event yields nothing.
        assert_eq!(p.push_line(""), None);
    }

    #[test]
    fn chunks_reassemble_across_arbitrary_splits() {
        let mut p = SseParser::new();
        let mut got = Vec::new();
        for chunk in ["data: hel", "lo\n\nda", "ta: world\n", "\n"] {
            got.extend(p.push_chunk(chunk));
        }
        assert_eq!(
            got.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(),
            ["hello", "world"]
        );
    }

    #[test]
    fn a_value_keeps_extra_colons_and_only_one_leading_space() {
        let mut p = SseParser::new();
        p.push_line("data: https://a/b?c=d");
        assert_eq!(p.push_line("").unwrap().data, "https://a/b?c=d");
        p.push_line("data:  two spaces");
        assert_eq!(p.push_line("").unwrap().data, " two spaces");
    }
}

//! The calls a model is still **generating**, previewed live: the incremental
//! decoder that reads a call's arguments as they stream, and the round's
//! drafts. See `docs/write-streaming.md`.
//!
//! A `write` call's `content` is the largest thing a model generates in a
//! turn, and it used to reach the screen only once the round had finished.
//! Everything here is pure — the pieces arrive as
//! [`crate::stream::StreamEvent::ToolCallDelta`]s, `App` (and each subagent's
//! [`crate::agents::AgentRun`]) folds them in, and `ui` renders what came out —
//! so a draft's whole life is unit-testable with no backend at all.

use std::cell::{Ref, RefCell};
use std::collections::VecDeque;

use crate::highlight::{CodeTheme, LineCache};
use crate::stream::ToolCallFragment;

/// One top-level string field of a streamed arguments object, decoded as far
/// as it has arrived ([`ArgsScanner::field`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArgField {
    /// The field's key, decoded.
    pub key: String,
    /// The value so far, decoded — escapes resolved. It only ever **grows**:
    /// every prefix of the stream decodes to a prefix of the whole value,
    /// however the pieces were split, which is what the render cache keys on.
    pub value: String,
    /// Whether the value's closing quote has arrived.
    pub closed: bool,
}

/// Where an [`ArgsScanner`] stands in the arguments object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Scan {
    /// Before the object's opening brace.
    #[default]
    Start,
    /// Inside the object, before a key or the closing brace.
    Key,
    /// Inside a key's string.
    KeyText,
    /// After a key, before its colon.
    Colon,
    /// After a colon, before the value.
    Value,
    /// Inside a top-level string value — the last field's.
    Text,
    /// Inside a nested object or array, which is skipped: `depth` brackets
    /// deep, inside one of its strings or not, just after a backslash or not.
    Nested {
        depth: u32,
        in_string: bool,
        escaped: bool,
    },
    /// Inside a number, `true`, `false` or `null`, which is skipped.
    Scalar,
    /// After a value, before the comma or the closing brace.
    AfterValue,
    /// The object closed — or the text is not an object at all. Whatever
    /// follows is ignored: the preview is cosmetic, and the call itself is
    /// still parsed whole by `serde_json` when its round ends.
    Done,
}

/// The escape a string is part way through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// Just after a backslash.
    Backslash,
    /// Inside `\uXXXX`: how many hex digits have been read, and their value.
    Unicode { digits: u8, value: u32 },
}

/// Decodes one JSON string body a character at a time — the escapes, a
/// `\uXXXX` split across pieces, and a surrogate pair split across two
/// escapes — appending what each character means to the caller's text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StringDecoder {
    escape: Escape,
    /// A high surrogate waiting for the low half that completes it.
    high: Option<u32>,
}

/// What a lone surrogate, an unknown escape or a bad `\u` digit decodes as.
const REPLACEMENT: char = '\u{fffd}';

impl StringDecoder {
    /// Feed one character of the string's body into `out`. Returns `true`
    /// when it was the closing quote.
    fn push(&mut self, c: char, out: &mut String) -> bool {
        match self.escape {
            Escape::None => match c {
                '"' => {
                    self.flush_high(out);
                    return true;
                }
                '\\' => self.escape = Escape::Backslash,
                _ => {
                    self.flush_high(out);
                    out.push(c);
                }
            },
            Escape::Backslash => {
                self.escape = Escape::None;
                let decoded = match c {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'u' => {
                        self.escape = Escape::Unicode {
                            digits: 0,
                            value: 0,
                        };
                        return false;
                    }
                    // `\"`, `\\`, `\/` — and an escape JSON does not have,
                    // which keeps its character rather than vanishing.
                    other => other,
                };
                self.flush_high(out);
                out.push(decoded);
            }
            Escape::Unicode { digits, value } => {
                let Some(digit) = c.to_digit(16) else {
                    // Not a hex digit: the escape was malformed. Mark it and
                    // read the character as itself.
                    self.escape = Escape::None;
                    self.flush_high(out);
                    out.push(REPLACEMENT);
                    return self.push(c, out);
                };
                let value = value * 16 + digit;
                if digits < 3 {
                    self.escape = Escape::Unicode {
                        digits: digits + 1,
                        value,
                    };
                } else {
                    self.escape = Escape::None;
                    self.unit(value, out);
                }
            }
        }
        false
    }

    /// One decoded UTF-16 code unit: a surrogate half waits for — or
    /// completes — its pair; anything else is a character.
    fn unit(&mut self, unit: u32, out: &mut String) {
        match unit {
            0xD800..=0xDBFF => {
                self.flush_high(out);
                self.high = Some(unit);
            }
            0xDC00..=0xDFFF => {
                let pair = self
                    .high
                    .take()
                    .map(|high| 0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00));
                out.push(pair.and_then(char::from_u32).unwrap_or(REPLACEMENT));
            }
            _ => {
                self.flush_high(out);
                out.push(char::from_u32(unit).unwrap_or(REPLACEMENT));
            }
        }
    }

    /// A high surrogate whose low half never came is a lone surrogate.
    fn flush_high(&mut self, out: &mut String) {
        if self.high.take().is_some() {
            out.push(REPLACEMENT);
        }
    }
}

/// Reads a tool call's JSON arguments **as they stream**: every top-level
/// string field, decoded as far as it has arrived, from pieces split
/// anywhere — mid-key, mid-escape, between the halves of a surrogate pair.
///
/// A character state machine rather than a re-parse of the accumulated text:
/// a 100 KB file costs one pass over its characters over the whole stream,
/// where parsing the arguments so far on every piece would cost quadratic
/// time — and `serde_json` cannot read an unfinished object anyway. Nested
/// objects, arrays and scalars are skipped; only string fields are kept,
/// which is all a preview needs (`docs/write-streaming.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArgsScanner {
    state: Scan,
    decoder: StringDecoder,
    /// The key being read, or the one whose value comes next.
    key: String,
    /// Every top-level string field met so far, in the order each opened.
    fields: Vec<ArgField>,
}

impl ArgsScanner {
    /// Read the next piece of the arguments text.
    pub fn feed(&mut self, text: &str) {
        for c in text.chars() {
            if self.state == Scan::Done {
                return;
            }
            self.step(c);
        }
    }

    /// The first top-level string field named `key`, as far as it has
    /// arrived — `None` until its value's opening quote. A key the model
    /// repeats is read where it **first** appears, so a value that has begun
    /// showing can never be swapped for another.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&ArgField> {
        self.fields.iter().find(|field| field.key == key)
    }

    /// Advance the machine by one character.
    fn step(&mut self, c: char) {
        self.state = match self.state {
            Scan::Start => match c {
                '{' => Scan::Key,
                c if c.is_whitespace() => Scan::Start,
                _ => Scan::Done,
            },
            Scan::Key => match c {
                '"' => {
                    self.key.clear();
                    self.decoder = StringDecoder::default();
                    Scan::KeyText
                }
                ',' => Scan::Key,
                c if c.is_whitespace() => Scan::Key,
                _ => Scan::Done,
            },
            Scan::KeyText => {
                if self.decoder.push(c, &mut self.key) {
                    Scan::Colon
                } else {
                    Scan::KeyText
                }
            }
            Scan::Colon => match c {
                ':' => Scan::Value,
                c if c.is_whitespace() => Scan::Colon,
                _ => Scan::Done,
            },
            Scan::Value => match c {
                '"' => {
                    self.fields.push(ArgField {
                        key: std::mem::take(&mut self.key),
                        value: String::new(),
                        closed: false,
                    });
                    self.decoder = StringDecoder::default();
                    Scan::Text
                }
                '{' | '[' => Scan::Nested {
                    depth: 1,
                    in_string: false,
                    escaped: false,
                },
                c if c.is_whitespace() => Scan::Value,
                _ => Scan::Scalar,
            },
            Scan::Text => {
                let Some(field) = self.fields.last_mut() else {
                    return;
                };
                if self.decoder.push(c, &mut field.value) {
                    field.closed = true;
                    Scan::AfterValue
                } else {
                    Scan::Text
                }
            }
            Scan::Nested {
                depth,
                in_string,
                escaped,
            } => nested_step(c, depth, in_string, escaped),
            Scan::Scalar => match c {
                ',' => Scan::Key,
                '}' => Scan::Done,
                c if c.is_whitespace() => Scan::AfterValue,
                _ => Scan::Scalar,
            },
            Scan::AfterValue => match c {
                ',' => Scan::Key,
                c if c.is_whitespace() => Scan::AfterValue,
                _ => Scan::Done,
            },
            Scan::Done => Scan::Done,
        };
    }
}

/// One character inside a skipped nested value: strings are skipped whole
/// (a bracket inside one is text), and the value ends when its outermost
/// bracket closes.
fn nested_step(c: char, depth: u32, in_string: bool, escaped: bool) -> Scan {
    let nested = |depth, in_string, escaped| Scan::Nested {
        depth,
        in_string,
        escaped,
    };
    if in_string {
        return match (escaped, c) {
            (true, _) => nested(depth, true, false),
            (false, '\\') => nested(depth, true, true),
            (false, '"') => nested(depth, false, false),
            _ => nested(depth, true, false),
        };
    }
    match c {
        '"' => nested(depth, true, false),
        '{' | '[' => nested(depth + 1, false, false),
        '}' | ']' if depth <= 1 => Scan::AfterValue,
        '}' | ']' => nested(depth - 1, false, false),
        _ => nested(depth, false, false),
    }
}

/// The wire tools whose generation previews live, each with the display name
/// its queued cell wears (`docs/write-streaming.md`). One entry today: a
/// `write`'s content is the long stream worth watching; every other call is
/// tracked so its pieces land in the right slot, and nothing of it is shown.
const PREVIEWED_TOOLS: &[(&str, &str)] = &[("write", "Write")];

/// Whether calls to the wire tool `name` preview as they stream.
fn previewed_wire(name: &str) -> bool {
    PREVIEWED_TOOLS.iter().any(|(wire, _)| *wire == name)
}

/// Whether a queued cell named `display` (a [`crate::app::ToolCall::name`])
/// is a previewed call — one that owns an adopted draft.
#[must_use]
pub fn previewed_display(display: &str) -> bool {
    PREVIEWED_TOOLS.iter().any(|(_, shown)| *shown == display)
}

/// One call of the round as it streams: its wire slot, its name once a piece
/// named it, and its arguments decoded as far as they have arrived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolDraft {
    /// The wire slot ([`ToolCallFragment::index`]) its pieces name.
    index: usize,
    /// The tool's wire name — empty until a piece names the call.
    name: String,
    /// The arguments, decoded as they stream.
    args: ArgsScanner,
    /// Nothing more of the call will stream: its round was announced.
    sealed: bool,
    /// The content's syntax colours, highlighted incrementally by the render
    /// path ([`highlighted`](Self::highlighted)).
    highlight: HighlightSlot,
}

/// A draft's render caches: its content highlighted incrementally
/// ([`LineCache`]), **filled by the render path** behind `RefCell`s — the
/// textarea's wrap-cache pattern — so the caches live exactly as long as the
/// draft and the state it describes stays plain data. Not part of the draft's
/// identity: two drafts that agree on everything else are equal whatever
/// their caches hold, and a clone carries the colours already worked out.
///
/// Two, because two kinds of view read the content. The strip's cell shows a
/// **peek** — its first and newest lines — and its cache keeps only those
/// ([`LineCache::windowed`]): a line's segments run ~50 times its text, so a
/// 2,000-line file coloured whole was ~1.9 MB held for the call's life, and
/// glibc keeps what a process has once used (`docs/memory.md`). The Ctrl+O
/// transcript and `/export` show **every** line; their cache starts empty and
/// is filled only when one of them asks.
#[derive(Default, Clone)]
struct HighlightSlot {
    peek: RefCell<LineCache>,
    whole: RefCell<LineCache>,
}

impl PartialEq for HighlightSlot {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for HighlightSlot {}

impl std::fmt::Debug for HighlightSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HighlightSlot(..)")
    }
}

impl ToolDraft {
    /// The tool's wire name (`write`) — empty until a piece names the call.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this call previews as it streams (a `write`).
    #[must_use]
    pub fn is_previewed(&self) -> bool {
        previewed_wire(&self.name)
    }

    /// The `path` argument, once its string has **closed** — a header that
    /// spelled `/home/us` for a frame would read as a bug.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.args
            .field("path")
            .filter(|field| field.closed)
            .map(|field| field.value.as_str())
    }

    /// The `content` argument as far as it has arrived — empty before its
    /// opening quote. It only ever grows ([`ArgField::value`]).
    #[must_use]
    pub fn content(&self) -> &str {
        self.args
            .field("content")
            .map_or("", |field| field.value.as_str())
    }

    /// Whether nothing more of the content will arrive: its closing quote
    /// came, or the round was announced (a call whose content never closed
    /// has stopped all the same). What flips the cell from the tail it
    /// follows while streaming to the head it settles on.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.sealed || self.args.field("content").is_some_and(|field| field.closed)
    }

    /// The content so far, syntax-highlighted as `lang` under the `code`
    /// theme with tabs expanded to `tab_width` spaces, keeping the colours of
    /// only its first and newest `lines` lines — what the strip's cell shows,
    /// from either end. Brought up to date on the way out, which costs the
    /// lines completed since the last call and nothing more
    /// ([`LineCache::update`]): the content only ever grows.
    #[must_use]
    pub fn highlighted_peek(
        &self,
        lang: Option<&str>,
        code: CodeTheme,
        tab_width: usize,
        lines: usize,
    ) -> Ref<'_, LineCache> {
        {
            let mut peek = self.highlight.peek.borrow_mut();
            if peek.window() != Some(lines) {
                *peek = LineCache::windowed(lines);
            }
            peek.update(self.content(), lang, code, tab_width);
        }
        self.highlight.peek.borrow()
    }

    /// The content so far highlighted like
    /// [`highlighted_peek`](Self::highlighted_peek), keeping **every** line —
    /// for the views that show the whole file (Ctrl+O, `/export`). Filled the
    /// first time one asks, brought up to date incrementally after.
    #[must_use]
    pub fn highlighted_whole(
        &self,
        lang: Option<&str>,
        code: CodeTheme,
        tab_width: usize,
    ) -> Ref<'_, LineCache> {
        self.highlight
            .whole
            .borrow_mut()
            .update(self.content(), lang, code, tab_width);
        self.highlight.whole.borrow()
    }
}

/// The calls of the round now being generated, and the previewed ones the
/// announced batch took over (`docs/write-streaming.md`).
///
/// One per conversation: `App` keeps the main session's and every
/// [`crate::agents::AgentRun`] its own. The lifecycle is the round's — pieces
/// [`push`](Self::push) in, the announcement [`seal`](Self::seal)s the round
/// and the batch [`adopt`](Self::adopt)s its previews, each queued call's
/// resolution [`resolve`](Self::resolve)s its own, and every turn edge
/// [`clear`](Self::clear)s it all. A half-generated call is never recorded:
/// its JSON is incomplete and the model never finished asking for it, so a
/// later request has nothing to replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolDrafts {
    /// The round's calls, one per wire slot, in the order each opened.
    generating: Vec<ToolDraft>,
    /// The round's generation is over: the next piece opens a new round
    /// rather than continuing a slot this one used.
    sealed: bool,
    /// The previewed drafts the announced batch took over, one per queued
    /// previewed call, front first — the k-th queued `Write` owns the k-th.
    queued: VecDeque<ToolDraft>,
    /// Bumped by every change — what the Ctrl+O transcript signs on.
    revision: u64,
}

impl ToolDrafts {
    /// Fold one streamed piece into its slot's draft, opening the draft when
    /// the slot is new. Returns whether the piece **opened a previewed
    /// call** — the moment the boundary flushes the streaming text before
    /// the cell goes up (`tui/stream.rs`' *flush before you interleave*).
    pub fn push(&mut self, fragment: &ToolCallFragment) -> bool {
        let opens = self.opens_preview(fragment);
        if self.sealed {
            self.generating.clear();
            self.sealed = false;
        }
        let at = match self
            .generating
            .iter()
            .position(|draft| draft.index == fragment.index)
        {
            Some(at) => at,
            None => {
                self.generating.push(ToolDraft {
                    index: fragment.index,
                    ..ToolDraft::default()
                });
                self.generating.len() - 1
            }
        };
        let draft = &mut self.generating[at];
        if !fragment.name.is_empty() {
            draft.name.clone_from(&fragment.name);
        }
        draft.args.feed(&fragment.arguments);
        self.revision += 1;
        opens
    }

    /// Whether [`push`](Self::push)ing `fragment` would open a previewed call
    /// — asked without folding it, for the agent session view, whose segment
    /// flush runs before its fold (`docs/write-streaming.md`).
    #[must_use]
    pub fn opens_preview(&self, fragment: &ToolCallFragment) -> bool {
        if !previewed_wire(&fragment.name) {
            return false;
        }
        if self.sealed {
            return true;
        }
        !self
            .generating
            .iter()
            .any(|draft| draft.index == fragment.index && draft.is_previewed())
    }

    /// The round's calls were announced (`RoundCalls`, an agent group, a task
    /// call): nothing more of them will stream. Every draft is finished, and
    /// the next piece opens a new round.
    pub fn seal(&mut self) {
        self.sealed = true;
        for draft in &mut self.generating {
            draft.sealed = true;
        }
        self.revision += 1;
    }

    /// The announced batch — its cells' display `names`, in queue order —
    /// takes the round's previews over: the queued previewed cells pair with
    /// the previewed drafts **in order**, but only when the two counts agree.
    /// A count that disagrees pairs nothing rather than guessing (the rule
    /// `App::start_tool_batch` applies to the round's call ids). Either way
    /// the round's generation is over.
    ///
    /// The adoptions **replace** whatever an earlier batch left, because the
    /// announcement *is* the queue — `App::start_tool_batch` replaces it
    /// whole — so pairing by order can never reach a draft another queue
    /// owned. A caller that adds a lone call to a queue already holding
    /// calls must not adopt at all (it clears instead): a wrong preview is
    /// worse than none.
    pub fn adopt<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        let wanted = names
            .into_iter()
            .filter(|name| previewed_display(name))
            .count();
        let previews: Vec<ToolDraft> = std::mem::take(&mut self.generating)
            .into_iter()
            .filter(ToolDraft::is_previewed)
            .collect();
        self.queued.clear();
        if previews.len() == wanted {
            self.queued.extend(previews.into_iter().map(|mut draft| {
                draft.sealed = true;
                draft
            }));
        }
        self.sealed = false;
        self.revision += 1;
    }

    /// A queued call named `display` left the queue — resolved, refused or
    /// interrupted. A previewed one gives up the front adopted draft, which
    /// is its own: both are front-first in queue order.
    pub fn resolve(&mut self, display: &str) {
        if previewed_display(display) && self.queued.pop_front().is_some() {
            self.revision += 1;
        }
    }

    /// Drop everything — a turn edge: its start, its end, an interrupt, an
    /// error, `/clear`.
    pub fn clear(&mut self) {
        self.generating.clear();
        self.queued.clear();
        self.sealed = false;
        self.revision += 1;
    }

    /// The previewed calls still being generated — not yet announced — in
    /// the order each opened.
    pub fn previews(&self) -> impl Iterator<Item = &ToolDraft> {
        self.generating.iter().filter(|draft| draft.is_previewed())
    }

    /// The previewed calls the queue holds, front first: the k-th queued
    /// previewed cell's draft is the k-th.
    pub fn queued(&self) -> impl Iterator<Item = &ToolDraft> {
        self.queued.iter()
    }

    /// Whether there is nothing to show — no preview streaming, none queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.previews().next().is_none() && self.queued.is_empty()
    }

    /// Bumped by every change.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

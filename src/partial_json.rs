//! Decoding a JSON object's top-level string fields **while it streams**.
//!
//! A tool call's arguments reach the app as fragments of one JSON text — a
//! provider streams them a few characters at a time — and a `write` call's
//! `content` is the whole file. [`StringFields`] reads the chosen keys' string
//! values out of those fragments as they land, so the live cell can show the
//! file being written before the call is whole (`docs/write-streaming.md`).
//!
//! Pure and allocation-light: one pass over each fragment, the selected values
//! decoded into their own buffers, everything else skipped without being kept.

/// One selected string field as decoded so far — **append-only**: a later
/// fragment only ever extends `text`, so whatever was drawn from it stays
/// true.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Field {
    /// The value decoded so far, escapes resolved.
    pub text: String,
    /// The closing quote has been read: `text` is the whole value.
    pub complete: bool,
    /// How many `\n` characters `text` holds — counted as they decode, so a
    /// line count stays O(1) however long the value grows.
    pub newlines: usize,
}

impl Field {
    /// How many lines `text` holds, by [`str::lines`]' rule — a trailing
    /// newline closes its line rather than opening an empty one — in O(1).
    #[must_use]
    pub fn lines(&self) -> usize {
        let open_tail = !self.text.is_empty() && !self.text.ends_with('\n');
        self.newlines + usize::from(open_tail)
    }

    /// Append one decoded character.
    fn push(&mut self, c: char) {
        if c == '\n' {
            self.newlines += 1;
        }
        self.text.push(c);
    }
}

/// The longest key [`StringFields`] keeps while reading one — far past any
/// key it is asked for, so a pathological key costs a bounded buffer.
const KEY_MAX_BYTES: usize = 256;

/// The Unicode replacement character, for a surrogate half with no partner —
/// what `String::from_utf16_lossy` makes of one too.
const REPLACEMENT: char = '\u{FFFD}';

/// Where the scan is in the JSON text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the object's opening `{`.
    Start,
    /// Inside the object, before a key (or the closing `}`).
    Key,
    /// Reading a key's string.
    KeyString,
    /// After a key, before its `:`.
    Colon,
    /// After the `:`, before the value begins.
    Value,
    /// Reading a string value — kept when `slot` names a chosen key.
    ValueString { slot: Option<usize> },
    /// Skipping a nested object or array value.
    Nested {
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    /// Skipping a scalar value (a number, `true`, `false`, `null`).
    Scalar,
    /// After a value, before the `,` or `}`.
    AfterValue,
    /// The object closed; whatever follows is not this object.
    Done,
    /// The text is not an object this scan can read — nothing more decodes.
    Broken,
}

/// What a string's next character means once its escapes are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decoded {
    /// A surrogate half turned out to have no partner: a replacement
    /// character comes first (what `String::from_utf16_lossy` makes of one).
    lone: bool,
    /// Then this.
    then: Then,
}

/// The part of a [`Decoded`] after any lone surrogate's replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Mid-escape: nothing to emit yet.
    Pending,
    /// One character of the string.
    Char(char),
    /// The closing quote.
    Close,
}

impl Decoded {
    /// `then`, with no lone surrogate before it.
    const fn plain(then: Then) -> Self {
        Self { lone: false, then }
    }
}

/// A JSON string's escape state, carried across fragments: a fragment may end
/// on a lone `\`, inside a `\u` escape's digits, or between the two halves of
/// a surrogate pair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Escape {
    /// Plain text.
    #[default]
    None,
    /// Just read a `\`.
    Backslash,
    /// Reading a `\u` escape's four hex digits — `high` is a decoded high
    /// surrogate waiting for this escape to be its low half.
    Unicode {
        value: u32,
        digits: u8,
        high: Option<u32>,
    },
    /// A high surrogate decoded; its low half's `\u` should come next.
    High(u32),
    /// A high surrogate decoded and a `\` read after it.
    HighBackslash(u32),
}

impl Escape {
    /// Resolve the string's next character `c`.
    fn step(&mut self, c: char) -> Decoded {
        match *self {
            Self::None => Decoded::plain(match c {
                '"' => Then::Close,
                '\\' => {
                    *self = Self::Backslash;
                    Then::Pending
                }
                // A raw control character is invalid JSON, but it is the text
                // the model meant — keep it (the module's leniency).
                c => Then::Char(c),
            }),
            Self::Backslash => self.after_backslash(c, None),
            Self::HighBackslash(high) => self.after_backslash(c, Some(high)),
            Self::Unicode {
                value,
                digits,
                high,
            } => {
                let Some(digit) = c.to_digit(16) else {
                    // A malformed escape: drop it and read `c` as if it had
                    // not been there — a quote still closes the string. A
                    // high half waiting on it was lone.
                    *self = Self::None;
                    let mut decoded = self.step(c);
                    decoded.lone |= high.is_some();
                    return decoded;
                };
                let value = value * 16 + digit;
                if digits < 3 {
                    *self = Self::Unicode {
                        value,
                        digits: digits + 1,
                        high,
                    };
                    return Decoded::plain(Then::Pending);
                }
                *self = Self::None;
                self.code_unit(value, high)
            }
            Self::High(high) => {
                if c == '\\' {
                    *self = Self::HighBackslash(high);
                    return Decoded::plain(Then::Pending);
                }
                // No escape followed, so no low half: the high one was lone.
                *self = Self::None;
                let mut decoded = self.step(c);
                decoded.lone = true;
                decoded
            }
        }
    }

    /// The character after a `\` — `high` the high surrogate still waiting
    /// for its low half, when one is.
    fn after_backslash(&mut self, c: char, high: Option<u32>) -> Decoded {
        let plain = match c {
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => {
                *self = Self::Unicode {
                    value: 0,
                    digits: 0,
                    high,
                };
                return Decoded::plain(Then::Pending);
            }
            // `\"`, `\\`, `\/` — and any escape JSON does not have, kept as
            // written.
            other => other,
        };
        *self = Self::None;
        Decoded {
            lone: high.is_some(),
            then: Then::Char(plain),
        }
    }

    /// One decoded `\uXXXX` code unit, `high` the high surrogate before it.
    fn code_unit(&mut self, unit: u32, high: Option<u32>) -> Decoded {
        let is_high = (0xD800..0xDC00).contains(&unit);
        let is_low = (0xDC00..0xE000).contains(&unit);
        let char_of = |unit: u32| char::from_u32(unit).unwrap_or(REPLACEMENT);
        match high {
            Some(high) if is_low => Decoded::plain(Then::Char(char_of(
                0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00),
            ))),
            // A second high half: the first was lone, this one waits.
            Some(_) if is_high => {
                *self = Self::High(unit);
                Decoded {
                    lone: true,
                    then: Then::Pending,
                }
            }
            Some(_) => Decoded {
                lone: true,
                then: Then::Char(char_of(unit)),
            },
            None if is_high => {
                *self = Self::High(unit);
                Decoded::plain(Then::Pending)
            }
            // A low half with no high one before it.
            None if is_low => Decoded::plain(Then::Char(REPLACEMENT)),
            None => Decoded::plain(Then::Char(char_of(unit))),
        }
    }
}

/// Decodes the string values of the chosen **top-level** keys of a JSON
/// object fed a fragment at a time. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringFields {
    /// The keys whose string values are decoded.
    keys: &'static [&'static str],
    /// One slot per key, `None` until its string value opens.
    fields: Vec<Option<Field>>,
    state: State,
    /// The key being read, or the one just read (its value comes next).
    key: String,
    /// The escape state of the string being read — a key's or a value's.
    escape: Escape,
}

impl StringFields {
    /// A decoder for the string values of `keys`.
    #[must_use]
    pub fn new(keys: &'static [&'static str]) -> Self {
        Self {
            keys,
            fields: vec![None; keys.len()],
            state: State::Start,
            key: String::new(),
            escape: Escape::None,
        }
    }

    /// Fold the next fragment of the JSON text in.
    pub fn feed(&mut self, fragment: &str) {
        for c in fragment.chars() {
            if matches!(self.state, State::Done | State::Broken) {
                return;
            }
            self.step(c);
        }
    }

    /// The field decoded for `key` — `None` until its string value opens (a
    /// key that is not selected is always `None`).
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&Field> {
        let slot = self.keys.iter().position(|k| *k == key)?;
        self.fields[slot].as_ref()
    }

    /// Advance the scan by one character.
    fn step(&mut self, c: char) {
        self.state = match self.state {
            State::Start => match c {
                '{' => State::Key,
                c if c.is_whitespace() => State::Start,
                _ => State::Broken,
            },
            State::Key => match c {
                '"' => {
                    self.key.clear();
                    self.escape = Escape::None;
                    State::KeyString
                }
                '}' => State::Done,
                // A stray comma is a model's typo, not a reason to stop.
                ',' => State::Key,
                c if c.is_whitespace() => State::Key,
                _ => State::Broken,
            },
            State::KeyString => self.key_char(c),
            State::Colon => match c {
                ':' => State::Value,
                c if c.is_whitespace() => State::Colon,
                _ => State::Broken,
            },
            State::Value => match c {
                '"' => {
                    self.escape = Escape::None;
                    let slot = self.keys.iter().position(|k| *k == self.key);
                    if let Some(slot) = slot {
                        // A repeated key starts over, as a parser's last-wins
                        // would read it.
                        self.fields[slot] = Some(Field::default());
                    }
                    State::ValueString { slot }
                }
                '{' | '[' => State::Nested {
                    depth: 1,
                    in_string: false,
                    escaped: false,
                },
                c if c.is_whitespace() => State::Value,
                ',' | '}' | ']' | ':' => State::Broken,
                _ => State::Scalar,
            },
            State::ValueString { slot } => self.value_char(c, slot),
            State::Nested {
                depth,
                in_string,
                escaped,
            } => Self::nested_char(c, depth, in_string, escaped),
            State::Scalar | State::AfterValue => match c {
                ',' => State::Key,
                '}' => State::Done,
                c if c.is_whitespace() => State::AfterValue,
                // Still inside the scalar (`-12.5e3`, `true`).
                _ if self.state == State::Scalar => State::Scalar,
                _ => State::Broken,
            },
            state @ (State::Done | State::Broken) => state,
        };
    }

    /// One character of a key's string.
    fn key_char(&mut self, c: char) -> State {
        let decoded = self.escape.step(c);
        if decoded.lone {
            self.push_key(REPLACEMENT);
        }
        match decoded.then {
            Then::Pending => State::KeyString,
            Then::Char(c) => {
                self.push_key(c);
                State::KeyString
            }
            Then::Close => State::Colon,
        }
    }

    /// Keep one character of the key being read, up to [`KEY_MAX_BYTES`].
    fn push_key(&mut self, c: char) {
        if self.key.len() < KEY_MAX_BYTES {
            self.key.push(c);
        }
    }

    /// One character of a value's string — kept when `slot` names a chosen
    /// key.
    fn value_char(&mut self, c: char, slot: Option<usize>) -> State {
        let decoded = self.escape.step(c);
        if let Some(field) = slot.and_then(|slot| self.fields[slot].as_mut()) {
            if decoded.lone {
                field.push(REPLACEMENT);
            }
            match decoded.then {
                Then::Pending => {}
                Then::Char(c) => field.push(c),
                Then::Close => field.complete = true,
            }
        }
        if decoded.then == Then::Close {
            State::AfterValue
        } else {
            State::ValueString { slot }
        }
    }

    /// One character of a skipped nested value: brackets counted, strings
    /// (and the brackets and quotes inside them) stepped over.
    fn nested_char(c: char, depth: usize, in_string: bool, escaped: bool) -> State {
        let (depth, in_string, escaped) = if in_string {
            match c {
                _ if escaped => (depth, true, false),
                '\\' => (depth, true, true),
                '"' => (depth, false, false),
                _ => (depth, true, false),
            }
        } else {
            match c {
                '"' => (depth, true, false),
                '{' | '[' => (depth + 1, false, false),
                '}' | ']' => (depth - 1, false, false),
                _ => (depth, false, false),
            }
        };
        if depth == 0 {
            State::AfterValue
        } else {
            State::Nested {
                depth,
                in_string,
                escaped,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &[&str] = &["path", "content"];

    fn decoded(fragments: &[&str]) -> StringFields {
        let mut fields = StringFields::new(KEYS);
        for fragment in fragments {
            fields.feed(fragment);
        }
        fields
    }

    fn text<'a>(fields: &'a StringFields, key: &str) -> Option<&'a str> {
        fields.field(key).map(|field| field.text.as_str())
    }

    #[test]
    fn a_whole_object_fed_at_once_decodes_every_selected_field() {
        let fields = decoded(&[r#"{"path":"a.py","content":"x\ny"}"#]);
        let path = fields.field("path").expect("path decoded");
        assert_eq!(path.text, "a.py");
        assert!(path.complete);
        let content = fields.field("content").expect("content decoded");
        assert_eq!(content.text, "x\ny");
        assert!(content.complete);
        assert_eq!(content.newlines, 1);
    }

    #[test]
    fn a_value_grows_as_its_fragments_arrive() {
        let mut fields = StringFields::new(KEYS);
        fields.feed(r#"{"content":"ab"#);
        let field = fields.field("content").expect("the string has opened");
        assert_eq!(field.text, "ab");
        assert!(!field.complete, "no closing quote yet");
        fields.feed(r#"c"}"#);
        let field = fields.field("content").expect("still there");
        assert_eq!(field.text, "abc");
        assert!(field.complete);
    }

    #[test]
    fn a_field_is_absent_until_its_string_value_opens() {
        let mut fields = StringFields::new(KEYS);
        fields.feed(r#"{"content""#);
        assert!(fields.field("content").is_none(), "only the key so far");
        fields.feed(": ");
        assert!(fields.field("content").is_none(), "the value has not begun");
        fields.feed("\"");
        assert_eq!(text(&fields, "content"), Some(""));
        assert!(!fields.field("content").expect("opened").complete);
    }

    #[test]
    fn a_key_that_was_not_chosen_is_never_decoded() {
        let fields = decoded(&[r#"{"old_string":"a","content":"b"}"#]);
        assert!(fields.field("old_string").is_none());
        assert_eq!(text(&fields, "content"), Some("b"));
    }

    #[test]
    fn an_escape_split_across_fragments_still_decodes() {
        // Real streams cut wherever the tokenizer did — between a backslash
        // and the letter it escapes is as likely as anywhere.
        let fields = decoded(&[r#"{"content":"a\"#, r#"nb\"#, r#"tc"}"#]);
        assert_eq!(text(&fields, "content"), Some("a\nb\tc"));
    }

    #[test]
    fn every_simple_escape_decodes() {
        let fields = decoded(&[r#"{"content":"q\" s\\ f\/ b\b f\f n\n r\r t\t"}"#]);
        assert_eq!(
            text(&fields, "content"),
            Some("q\" s\\ f/ b\u{8} f\u{c} n\n r\r t\t")
        );
    }

    #[test]
    fn a_unicode_escape_split_inside_its_digits_still_decodes() {
        let fields = decoded(&[r#"{"content":"caf\u0"#, r#"0e"#, r#"9!"}"#]);
        assert_eq!(text(&fields, "content"), Some("café!"));
    }

    #[test]
    fn a_surrogate_pair_decodes_to_one_character_wherever_it_is_split() {
        let json = r#"{"content":"\ud83d\ude00 ok"}"#;
        for at in 1..json.len() {
            let fields = decoded(&[&json[..at], &json[at..]]);
            assert_eq!(text(&fields, "content"), Some("😀 ok"), "split at {at}");
        }
    }

    #[test]
    fn a_lone_surrogate_decodes_to_the_replacement_character() {
        // A high half with no low half after it, a low half on its own, and a
        // high half followed by an escape that is not a low half.
        let fields = decoded(&[r#"{"content":"\ud83dx \ude00 \ud83d\u0041"}"#]);
        assert_eq!(
            text(&fields, "content"),
            Some("\u{FFFD}x \u{FFFD} \u{FFFD}A")
        );
    }

    #[test]
    fn an_escaped_quote_after_a_lone_surrogate_is_text_not_the_close() {
        let fields = decoded(&[r#"{"content":"\ud83d\"x"}"#]);
        let field = fields.field("content").expect("decoded");
        assert_eq!(field.text, "\u{FFFD}\"x");
        assert!(field.complete);
    }

    #[test]
    fn a_quote_cutting_a_unicode_escape_short_still_closes_the_string() {
        // `\u12` then the closing quote: the malformed escape is dropped and
        // the quote does what a quote does.
        let fields = decoded(&[r#"{"content":"a\u12","path":"p"}"#]);
        let content = fields.field("content").expect("decoded");
        assert_eq!(content.text, "a");
        assert!(content.complete);
        assert_eq!(text(&fields, "path"), Some("p"));
    }

    #[test]
    fn literal_non_ascii_text_passes_through() {
        let fields = decoded(&[r#"{"content":"日本"#, r#"語 ✓"}"#]);
        assert_eq!(text(&fields, "content"), Some("日本語 ✓"));
    }

    #[test]
    fn values_under_other_keys_are_skipped_nested_ones_included() {
        // A nested object holding a `content` key, an array holding the key's
        // name as a string and a `}` inside a string, scalars of every kind —
        // none of it is the top-level `content`.
        let fields = decoded(&[
            r#"{"a":{"content":"no","deep":[{"x":"}"}]},"b":[1,"content",{"y":"]\"}"}],"#,
            r#""n":-12.5e3,"t":true,"f":false,"z":null,"content":"yes"}"#,
        ]);
        assert_eq!(text(&fields, "content"), Some("yes"));
    }

    #[test]
    fn whitespace_between_tokens_is_allowed() {
        let fields = decoded(&["\n {\n  \"path\" :\t\"p\" ,\n  \"content\"\n:\n\"c\"\n}\n"]);
        assert_eq!(text(&fields, "path"), Some("p"));
        assert_eq!(text(&fields, "content"), Some("c"));
    }

    #[test]
    fn an_escaped_key_still_matches() {
        let fields = decoded(&[r#"{"con\u0074ent":"v"}"#]);
        assert_eq!(text(&fields, "content"), Some("v"));
    }

    #[test]
    fn a_repeated_key_starts_its_value_over() {
        let fields = decoded(&[r#"{"content":"first","content":"second"}"#]);
        assert_eq!(text(&fields, "content"), Some("second"));
    }

    #[test]
    fn a_raw_control_character_inside_a_string_is_kept() {
        // Invalid JSON — a model that forgot to escape a newline — but what a
        // display should show is the text the model meant.
        let fields = decoded(&["{\"content\":\"a\nb\"}"]);
        let field = fields.field("content").expect("decoded");
        assert_eq!(field.text, "a\nb");
        assert_eq!(field.newlines, 1);
    }

    #[test]
    fn text_that_is_not_an_object_decodes_nothing() {
        for json in [r#""{\"content\":\"x\"}""#, "[1,2]", "content", "null"] {
            let fields = decoded(&[json]);
            assert!(fields.field("content").is_none(), "{json}");
        }
    }

    #[test]
    fn whatever_follows_the_closed_object_is_ignored() {
        let fields = decoded(&[r#"{"content":"a"} {"content":"b"}"#]);
        assert_eq!(text(&fields, "content"), Some("a"));
    }

    #[test]
    fn newlines_are_counted_however_they_arrive() {
        let fields = decoded(&[r#"{"content":"1\n2\"#, r#"n3\r\n"#, "4\n\"}"]);
        assert_eq!(fields.field("content").expect("decoded").newlines, 4);
    }

    #[test]
    fn lines_counts_exactly_like_str_lines() {
        for sample in [
            "", "a", "a\n", "a\nb", "\n", "\n\n", "a\r\nb", "a\r\n", "\r", "x\n\ny",
        ] {
            let field = Field {
                text: sample.to_string(),
                complete: true,
                newlines: sample.matches('\n').count(),
            };
            assert_eq!(field.lines(), sample.lines().count(), "{sample:?}");
        }
    }

    /// Objects whose selected fields hold everything the decoder must get
    /// right — escapes of every kind, a surrogate pair, literal non-ASCII, a
    /// nested value and scalars under other keys.
    const SAMPLES: &[&str] = &[
        r#"{"path":"/tmp/a.py","content":"def f():\n    return \"x\\y\"\n"}"#,
        r#"{"content":"tab\there \u00e9 \ud83d\ude00 \/ \b\f\r","path":"b"}"#,
        r#"{"skip":{"content":"no","list":[1,{"z":"}"}]},"content":"ünïcödé ✓","n":3}"#,
        r#"  { "path" : "spaced name.txt" , "content" : "" }  "#,
    ];

    #[test]
    fn every_split_matches_what_serde_decodes() {
        for json in SAMPLES {
            let whole: serde_json::Value = serde_json::from_str(json).expect("a valid sample");
            let boundaries: Vec<usize> = (0..=json.len())
                .filter(|&at| json.is_char_boundary(at))
                .collect();
            for &at in &boundaries {
                let fields = decoded(&[&json[..at], &json[at..]]);
                for key in KEYS {
                    let want = whole.get(key).and_then(serde_json::Value::as_str);
                    let got = fields.field(key);
                    assert_eq!(got.map(|f| f.text.as_str()), want, "{json} split at {at}");
                    if want.is_some() {
                        assert!(got.is_some_and(|f| f.complete), "{json} split at {at}");
                    }
                }
            }
        }
    }

    #[test]
    fn fed_a_character_at_a_time_every_value_only_ever_grows() {
        for json in SAMPLES {
            let whole: serde_json::Value = serde_json::from_str(json).expect("a valid sample");
            let mut fields = StringFields::new(KEYS);
            for c in json.chars() {
                fields.feed(c.encode_utf8(&mut [0; 4]));
                for key in KEYS {
                    if let Some(field) = fields.field(key) {
                        let want = whole[*key].as_str().expect("a string sample");
                        assert!(
                            want.starts_with(&field.text),
                            "{json}: {key} decoded {:?}, not a prefix of {want:?}",
                            field.text
                        );
                    }
                }
            }
        }
    }
}

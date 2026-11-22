//! Reading an Images API answer **as it arrives**, never held whole.
//!
//! The ChatGPT backend answers an image request with one JSON object whose
//! `data[].b64_json` strings are the pictures, base64 — twenty-odd megabytes
//! of text for a 4K PNG. Parsed the ordinary way (the body read whole, a
//! string per image, the decoded bytes) that is three picture-sized blocks on
//! the worker's arena, which glibc keeps (`docs/memory.md`). So the body is
//! *scanned*: [`split_response`] walks it byte by byte, streams the first
//! image's base64 through a [`Base64Decoder`] straight into the caller's sink
//! (a staging file), and passes over every other byte, keeping only the few
//! bytes of the string it is in that could still be the `b64_json` key.
//! Kilobytes in memory, whatever the image weighs — and whatever else the
//! answer carries. Pure over `Read`/`Write`,
//! so every split and escape is unit-tested here (`docs/imagegen.md`).

use std::io::{self, Read, Write};

/// How much of the body is read per pass, and how often the cancel check
/// runs: once per chunk.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// How much decoded image the decoder holds before handing it to the sink.
const DECODE_BUF_BYTES: usize = 48 * 1024;

/// Why an answer could not be read into an image.
#[derive(Debug)]
pub(crate) enum ScanError {
    /// Reading the body failed — a reset, a stall past the deadline.
    Read(io::Error),
    /// The sink refused a write — the disk, or the image cap.
    Write(io::Error),
    /// A `b64_json` string that is not base64.
    Base64(String),
    /// The body is not JSON this scanner can read: it ends inside a string.
    Malformed(String),
    /// The turn was cancelled while the body streamed.
    Cancelled,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(f, "reading the response failed: {error}"),
            Self::Write(error) => write!(f, "saving the image failed: {error}"),
            Self::Base64(message) => write!(f, "the image data is not base64: {message}"),
            Self::Malformed(message) => write!(f, "the response could not be read: {message}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Standard base64 (RFC 4648: the `+/` alphabet, `=` padding) decoded as it
/// arrives — [`crate::clipboard::base64_encode`]'s inverse, fed a character
/// at a time and writing whole groups through a small buffer. Whitespace is
/// skipped; anything else outside the alphabet is refused.
pub(crate) struct Base64Decoder {
    quad: [u8; 4],
    filled: usize,
    padding: usize,
    out: Vec<u8>,
    written: u64,
}

impl Base64Decoder {
    pub(crate) fn new() -> Self {
        Self {
            quad: [0; 4],
            filled: 0,
            padding: 0,
            out: Vec::with_capacity(DECODE_BUF_BYTES),
            written: 0,
        }
    }

    /// Take one character of the encoding.
    pub(crate) fn push(&mut self, byte: u8, sink: &mut dyn Write) -> Result<(), ScanError> {
        if byte.is_ascii_whitespace() {
            return Ok(());
        }
        if byte == b'=' {
            // Padding stands only in a group's third or fourth place.
            if self.filled < 2 {
                return Err(ScanError::Base64(
                    "padding where a group cannot have it".into(),
                ));
            }
            self.quad[self.filled] = 0;
            self.filled += 1;
            self.padding += 1;
        } else {
            if self.padding > 0 {
                return Err(ScanError::Base64("data after the padding".into()));
            }
            let Some(value) = sextet(byte) else {
                return Err(ScanError::Base64(format!(
                    "{:?} is not a base64 character",
                    char::from(byte)
                )));
            };
            self.quad[self.filled] = value;
            self.filled += 1;
        }
        if self.filled == 4 {
            self.emit(4 - self.padding, sink)?;
            self.filled = 0;
        }
        Ok(())
    }

    /// Take a run of characters.
    #[cfg(test)]
    pub(crate) fn push_slice(
        &mut self,
        bytes: &[u8],
        sink: &mut dyn Write,
    ) -> Result<(), ScanError> {
        for &byte in bytes {
            self.push(byte, sink)?;
        }
        Ok(())
    }

    /// End the encoding: the last group is decoded, the buffer handed over,
    /// and the number of bytes the sink received comes back. A group left
    /// unpadded is read for what its characters carry — some encoders drop
    /// the padding — but a lone character carries no byte and is refused.
    pub(crate) fn finish(mut self, sink: &mut dyn Write) -> Result<u64, ScanError> {
        if self.filled > 0 {
            let data = self.filled - self.padding;
            if data < 2 {
                return Err(ScanError::Base64("the encoding ends mid-group".into()));
            }
            for slot in self.filled..4 {
                self.quad[slot] = 0;
            }
            self.emit(data, sink)?;
        }
        self.hand_over(sink)?;
        Ok(self.written)
    }

    /// Decode the group in `quad`, `data` of whose characters carry bits.
    fn emit(&mut self, data: usize, sink: &mut dyn Write) -> Result<(), ScanError> {
        let [a, b, c, d] = self.quad.map(u32::from);
        let group = (a << 18) | (b << 12) | (c << 6) | d;
        let bytes = [(group >> 16) as u8, (group >> 8) as u8, group as u8];
        // Two characters carry one byte, three carry two, four carry three.
        self.out.extend_from_slice(&bytes[..data - 1]);
        if self.out.len() >= DECODE_BUF_BYTES {
            self.hand_over(sink)?;
        }
        Ok(())
    }

    fn hand_over(&mut self, sink: &mut dyn Write) -> Result<(), ScanError> {
        if self.out.is_empty() {
            return Ok(());
        }
        sink.write_all(&self.out).map_err(ScanError::Write)?;
        self.written += self.out.len() as u64;
        self.out.clear();
        Ok(())
    }
}

/// The six bits a base64 character stands for, `None` outside the alphabet.
fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// What [`split_response`] found.
#[derive(Debug)]
pub(crate) struct Split {
    /// How many `b64_json` strings the answer held. Only the first reached
    /// the sink.
    pub images: usize,
    /// How many decoded bytes the sink received.
    pub decoded: u64,
}

/// Scan an Images API answer: the first `b64_json` string is decoded into
/// `sink` as it streams, and everything else is passed over. `cancelled` is
/// asked once per chunk read.
///
/// # Errors
/// A failed read or write, image data that is not base64, a body that ends
/// inside a string, or a cancel.
pub(crate) fn split_response(
    reader: &mut dyn Read,
    sink: &mut dyn Write,
    cancelled: &dyn Fn() -> bool,
) -> Result<Split, ScanError> {
    let mut scanner = Scanner::new();
    let mut buf = vec![0u8; READ_CHUNK_BYTES];
    loop {
        if cancelled() {
            return Err(ScanError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ScanError::Read(error)),
        };
        for &byte in &buf[..n] {
            scanner.take(byte, sink)?;
        }
    }
    scanner.end()
}

/// Where the scan is in the JSON.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between tokens.
    Outside,
    /// Inside a string that is not image data, `escaped` just after a
    /// backslash.
    Text { escaped: bool },
    /// Just past the string `"b64_json"`, before its `:`.
    AfterKey,
    /// Past `"b64_json":`, before its value.
    AfterColon,
    /// Inside a `b64_json` string.
    Image { escaped: bool },
}

/// The key whose string value is an image.
const IMAGE_KEY: &[u8] = b"b64_json";

struct Scanner {
    state: State,
    /// The string being read, while it could still be [`IMAGE_KEY`].
    key: Vec<u8>,
    key_possible: bool,
    images: usize,
    /// The first image's decoder, while its string is being read.
    decoder: Option<Base64Decoder>,
    decoded: u64,
}

impl Scanner {
    fn new() -> Self {
        Self {
            state: State::Outside,
            key: Vec::with_capacity(IMAGE_KEY.len()),
            key_possible: false,
            images: 0,
            decoder: None,
            decoded: 0,
        }
    }

    fn take(&mut self, byte: u8, sink: &mut dyn Write) -> Result<(), ScanError> {
        match self.state {
            State::Outside => {
                if byte == b'"' {
                    self.state = State::Text { escaped: false };
                    self.key.clear();
                    self.key_possible = true;
                }
                Ok(())
            }
            State::Text { escaped } => {
                if escaped {
                    // No escape is spelled in the key, so a string with one
                    // is not it.
                    self.key_possible = false;
                    self.state = State::Text { escaped: false };
                } else if byte == b'\\' {
                    self.state = State::Text { escaped: true };
                } else if byte == b'"' {
                    self.state = if self.key_possible && self.key == IMAGE_KEY {
                        State::AfterKey
                    } else {
                        State::Outside
                    };
                } else if self.key_possible {
                    if self.key.len() < IMAGE_KEY.len() {
                        self.key.push(byte);
                    } else {
                        self.key_possible = false;
                    }
                }
                Ok(())
            }
            State::AfterKey | State::AfterColon if byte.is_ascii_whitespace() => Ok(()),
            State::AfterKey if byte == b':' => {
                self.state = State::AfterColon;
                Ok(())
            }
            State::AfterColon if byte == b'"' => {
                self.images += 1;
                if self.images == 1 {
                    self.decoder = Some(Base64Decoder::new());
                }
                self.state = State::Image { escaped: false };
                Ok(())
            }
            // `"b64_json"` was a value, or its value is not a string: read
            // on as ordinary JSON.
            State::AfterKey | State::AfterColon => {
                self.state = State::Outside;
                self.take(byte, sink)
            }
            State::Image { escaped: true } => {
                self.state = State::Image { escaped: false };
                match byte {
                    // JSON may spell `/` as `\/`, and base64 is full of them.
                    b'/' => self.image_byte(b'/', sink),
                    // A wrapped encoding's line breaks carry nothing.
                    b'n' | b'r' | b't' => Ok(()),
                    other => Err(ScanError::Base64(format!(
                        "the escape \\{} in the image data",
                        char::from(other)
                    ))),
                }
            }
            State::Image { escaped: false } => match byte {
                b'\\' => {
                    self.state = State::Image { escaped: true };
                    Ok(())
                }
                b'"' => {
                    if let Some(decoder) = self.decoder.take() {
                        self.decoded = decoder.finish(sink)?;
                    }
                    self.state = State::Outside;
                    Ok(())
                }
                other => self.image_byte(other, sink),
            },
        }
    }

    /// One character of an image's base64: the first image's goes to the
    /// sink, a later one's is passed over.
    fn image_byte(&mut self, byte: u8, sink: &mut dyn Write) -> Result<(), ScanError> {
        match &mut self.decoder {
            Some(decoder) => decoder.push(byte, sink),
            None => Ok(()),
        }
    }

    fn end(self) -> Result<Split, ScanError> {
        if self.state != State::Outside {
            return Err(ScanError::Malformed("the response ended mid-value".into()));
        }
        Ok(Split {
            images: self.images,
            decoded: self.decoded,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::base64_encode;

    fn decode(text: &[u8]) -> Result<Vec<u8>, ScanError> {
        let mut out = Vec::new();
        let mut decoder = Base64Decoder::new();
        decoder.push_slice(text, &mut out)?;
        let written = decoder.finish(&mut out)?;
        assert_eq!(written, out.len() as u64, "finish counts what it wrote");
        Ok(out)
    }

    /// Every length 0..=40 of a byte ramp, so each padding case and each
    /// group boundary is crossed.
    fn samples() -> Vec<Vec<u8>> {
        (0..=40u8)
            .map(|len| {
                (0..len)
                    .map(|i| i.wrapping_mul(37).wrapping_add(251))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_decoder_inverts_the_crates_own_encoder() {
        for bytes in samples() {
            let text = base64_encode(&bytes);
            assert_eq!(decode(text.as_bytes()).unwrap(), bytes, "{text}");
        }
    }

    #[test]
    fn the_rfc_vectors_decode() {
        for (text, plain) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(decode(text.as_bytes()).unwrap(), plain.as_bytes(), "{text}");
        }
    }

    #[test]
    fn whitespace_inside_the_encoding_is_skipped() {
        assert_eq!(decode(b"Zm9v\nYmFy\r\n").unwrap(), b"foobar");
        assert_eq!(decode(b" Zm 9v ").unwrap(), b"foo");
    }

    #[test]
    fn a_character_outside_the_alphabet_is_refused() {
        assert!(matches!(decode(b"Zm9*"), Err(ScanError::Base64(_))));
        assert!(matches!(decode(b"Zm9v-_"), Err(ScanError::Base64(_))));
    }

    #[test]
    fn data_after_the_padding_is_refused() {
        assert!(matches!(decode(b"Zg==Zm9v"), Err(ScanError::Base64(_))));
        assert!(matches!(decode(b"Zm8=Z"), Err(ScanError::Base64(_))));
    }

    #[test]
    fn a_truncated_group_is_refused() {
        // One character of a group carries six bits: no byte at all.
        assert!(matches!(decode(b"Zm9vY"), Err(ScanError::Base64(_))));
        // Padding where a group cannot have it.
        assert!(matches!(decode(b"Z==="), Err(ScanError::Base64(_))));
        assert!(matches!(decode(b"=Zm9"), Err(ScanError::Base64(_))));
    }

    #[test]
    fn unpadded_input_still_decodes() {
        // Some encoders drop the padding; the group's length says how much
        // of it is data.
        assert_eq!(decode(b"Zg").unwrap(), b"f");
        assert_eq!(decode(b"Zm8").unwrap(), b"fo");
    }

    #[test]
    fn a_large_encoding_reaches_the_sink_in_buffered_pieces() {
        struct Counting {
            writes: usize,
            bytes: Vec<u8>,
        }
        impl Write for Counting {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let text = base64_encode(&bytes);
        let mut sink = Counting {
            writes: 0,
            bytes: Vec::new(),
        };
        let mut decoder = Base64Decoder::new();
        decoder.push_slice(text.as_bytes(), &mut sink).unwrap();
        decoder.finish(&mut sink).unwrap();
        assert_eq!(sink.bytes, bytes);
        // Buffered, never byte by byte, and never the whole thing at once.
        assert!(
            sink.writes > 1 && sink.writes < 20,
            "{} writes",
            sink.writes
        );
    }

    fn split(body: &[u8], chunk: usize) -> Result<(Split, Vec<u8>), ScanError> {
        let mut reader = Chunked { body, chunk };
        let mut sink = Vec::new();
        let split = split_response(&mut reader, &mut sink, &|| false)?;
        Ok((split, sink))
    }

    /// A reader that hands the body over `chunk` bytes at a time, so every
    /// split point — inside the key, the escape, the padding — is exercised.
    struct Chunked<'a> {
        body: &'a [u8],
        chunk: usize,
    }

    impl Read for Chunked<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.chunk.min(buf.len()).min(self.body.len());
            buf[..n].copy_from_slice(&self.body[..n]);
            self.body = &self.body[n..];
            Ok(n)
        }
    }

    fn answer(images: &[&[u8]]) -> String {
        let data: Vec<String> = images
            .iter()
            .map(|bytes| {
                format!(
                    r#"{{"b64_json": "{}", "revised_prompt": "a red fox"}}"#,
                    base64_encode(bytes)
                )
            })
            .collect();
        format!(
            r#"{{"created": 1778832973, "background": "opaque", "data": [{}], "output_format": "png", "quality": "medium", "size": "1024x1536", "usage": {{"total_tokens": 2846}}}}"#,
            data.join(", ")
        )
    }

    #[test]
    fn the_first_image_streams_into_the_sink_at_every_split() {
        let image: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let body = answer(&[&image]);
        for chunk in [1, 2, 3, 7, 64, 4096, body.len()] {
            let (split, sink) = split(body.as_bytes(), chunk).unwrap();
            assert_eq!(sink, image, "chunk {chunk}");
            assert_eq!(split.images, 1);
            assert_eq!(split.decoded, image.len() as u64);
        }
    }

    #[test]
    fn only_the_first_image_is_kept_and_the_rest_are_counted() {
        // Codex uses `data[0]` alone; a second image costs nothing but the
        // reading.
        let body = answer(&[b"first", b"second"]);
        let (split, sink) = split(body.as_bytes(), 3).unwrap();
        assert_eq!(sink, b"first");
        assert_eq!(split.images, 2);
    }

    #[test]
    fn an_escaped_slash_in_the_image_is_the_slash() {
        // JSON may spell `/` as `\/`; base64 is full of them.
        let bytes = [0xff_u8, 0xff, 0xfe, 0x3f, 0xff];
        let text = base64_encode(&bytes);
        assert!(text.contains('/'), "{text}");
        let escaped = text.replace('/', "\\/");
        let body = format!(r#"{{"data": [{{"b64_json": "{escaped}"}}]}}"#);
        for chunk in [1, 2, body.len()] {
            let (_, sink) = split(body.as_bytes(), chunk).unwrap();
            assert_eq!(sink, bytes, "chunk {chunk}");
        }
    }

    #[test]
    fn escaped_line_breaks_in_the_image_are_skipped() {
        let body = r#"{"data": [{"b64_json": "Zm9v\nYmFy\r\n"}]}"#;
        let (_, sink) = split(body.as_bytes(), 4).unwrap();
        assert_eq!(sink, b"foobar");
    }

    #[test]
    fn any_other_escape_in_the_image_is_refused() {
        let body = r#"{"data": [{"b64_json": "Zm9vA"}]}"#;
        assert!(matches!(
            split(body.as_bytes(), 8),
            Err(ScanError::Base64(_))
        ));
    }

    #[test]
    fn the_key_is_matched_only_as_a_key() {
        // A *value* spelled `b64_json` is just text, and a key of another
        // name is not the image.
        let body = r#"{"note": "b64_json", "data": [{"b64_jsonx": "Zm9v", "b64_json": "YmFy"}]}"#;
        let (split, sink) = split(body.as_bytes(), 2).unwrap();
        assert_eq!(sink, b"bar");
        assert_eq!(split.images, 1);
    }

    #[test]
    fn strings_with_escaped_quotes_do_not_derail_the_scan() {
        let body = r#"{"revised": "a \"quoted\" fox \\", "data": [{"b64_json": "Zm9v"}]}"#;
        let (split, sink) = split(body.as_bytes(), 1).unwrap();
        assert_eq!(sink, b"foo");
        assert_eq!(split.images, 1);
    }

    #[test]
    fn a_null_image_is_no_image() {
        let body = r#"{"data": [{"b64_json": null}]}"#;
        let (split, sink) = split(body.as_bytes(), 3).unwrap();
        assert_eq!(split.images, 0);
        assert!(sink.is_empty());
    }

    #[test]
    fn an_answer_with_no_image_scans_to_nothing() {
        let body = r#"{"created": 1, "data": []}"#;
        let (split, sink) = split(body.as_bytes(), 3).unwrap();
        assert_eq!(split.images, 0);
        assert!(sink.is_empty());
    }

    #[test]
    fn a_body_cut_off_inside_the_image_is_malformed() {
        let body = r#"{"data": [{"b64_json": "Zm9vYm"#;
        assert!(matches!(
            split(body.as_bytes(), 4),
            Err(ScanError::Malformed(_))
        ));
    }

    #[test]
    fn however_much_else_the_answer_carries_the_image_is_saved() {
        // The image was made — and paid for — before the answer arrived, so
        // nothing else in it is a reason to fail the call (Codex drops
        // metadata past its own cap rather than the image).
        let pixels = [7u8; 300];
        let body = format!(
            r#"{{"junk": "{}", "data": [{{"b64_json": "{}", "image_metadata": {{"x": "{}"}}}}]}}"#,
            "x".repeat(2 * 1024 * 1024),
            base64_encode(&pixels),
            "y".repeat(1024 * 1024)
        );
        let (split, sink) = split(body.as_bytes(), 4096).expect("the image is kept");
        assert_eq!(split.images, 1);
        assert_eq!(sink, pixels);
    }

    #[test]
    fn a_cancel_stops_the_scan() {
        let body = answer(&[b"pixels"]);
        let mut reader = Chunked {
            body: body.as_bytes(),
            chunk: 8,
        };
        let mut sink = Vec::new();
        let result = split_response(&mut reader, &mut sink, &|| true);
        assert!(matches!(result, Err(ScanError::Cancelled)));
    }

    #[test]
    fn a_refused_write_is_a_write_error() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let body = answer(&[&[7u8; 100]]);
        let mut reader = Chunked {
            body: body.as_bytes(),
            chunk: 4096,
        };
        let result = split_response(&mut reader, &mut Full, &|| false);
        assert!(matches!(result, Err(ScanError::Write(_))), "{result:?}");
    }
}

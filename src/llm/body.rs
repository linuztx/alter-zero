//! How a request leaves the process: **streamed**, never held whole.
//!
//! A request that carries a pasted picture is megabytes of base64, and the
//! conversation is re-sent every turn (`docs/context.md`). Serializing it
//! into a buffer — `reqwest`'s own `.json()`, a `Vec` grown by doubling —
//! made a picture-sized block per round that glibc's thread arenas kept:
//! the process stepped up by a body's worth whenever a turn landed on a new
//! arena, which read as "RAM grows every message" (`docs/memory.md`,
//! *Every turn re-sent the picture*). Even a buffer sized exactly once
//! measured a body's worth left per arena every few turns.
//!
//! So the body exists nowhere: a serializer thread writes the JSON into a
//! bounded pipe of [`BODY_CHUNK_BYTES`] chunks and the transport pumps the
//! [`BodyReader`] at the other end as it uploads, `Content-Length` known
//! from a counting pass that allocates nothing. What a wire hands over is a
//! [`BodySource`] — its request as owned data from which the JSON can be
//! written twice (count, then pipe), borrowing every picture's bytes from
//! the session's one shared encoding (`images::attachment`) rather than
//! copying them into a tree. Pure and unit-tested; the HTTP lives in
//! [`super::openai`].

use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use super::{LlmError, Result};

/// A request that can write itself as JSON — twice, since the byte count is
/// needed before the first byte is sent. Owned (`'static`) so it can move
/// to the serializer thread; what it writes may borrow from itself, which
/// is how a wire's typed request carries a picture by reference.
pub trait BodySource: Send + 'static {
    /// Write the request as JSON.
    ///
    /// # Errors
    /// The writer failing (the pipe's reader gone) or a value that can't be
    /// serialized.
    fn write_to(&self, out: &mut dyn Write) -> serde_json::Result<()>;
}

impl BodySource for serde_json::Value {
    fn write_to(&self, out: &mut dyn Write) -> serde_json::Result<()> {
        serde_json::to_writer(out, self)
    }
}

/// How many bytes `source` serializes to — a counting pass over the output
/// that allocates nothing, so the `Content-Length` of a streamed body is
/// known before a byte of it exists. A second walk over the text, a few
/// milliseconds on the largest body this app sends.
///
/// # Errors
/// A value that can't be serialized.
pub fn serialized_len(source: &impl BodySource) -> Result<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    source
        .write_to(&mut counter)
        .map_err(|e| LlmError::Http(format!("could not encode the request: {e}")))?;
    Ok(counter.0)
}

/// How much of a streamed body is in flight at once: the chunk the
/// serializer thread hands over, times the pipe's depth. Kilobytes,
/// whatever the body weighs.
pub const BODY_CHUNK_BYTES: usize = 64 * 1024;

/// How many chunks the body pipe holds before the serializer waits for the
/// transport to catch up.
const BODY_PIPE_DEPTH: usize = 4;

/// `source` serialized **as it is uploaded**: the JSON is written by its own
/// thread into a bounded pipe of [`BODY_CHUNK_BYTES`] chunks, and the
/// [`BodyReader`] the transport pumps takes them out the other end. The
/// whole body exists nowhere, so a request carrying a picture costs the
/// process kilobytes rather than the picture again per round
/// (`docs/memory.md`). The byte count comes from a counting pass first
/// ([`serialized_len`]), so the request is `Content-Length`-framed like the
/// buffered form was.
///
/// A reader dropped early — an Esc that ended the transport — breaks the
/// pipe, and the serializer thread ends on its next write.
///
/// # Errors
/// A value that can't be serialized (the counting pass finds out first).
pub fn streamed_request(source: impl BodySource) -> Result<(BodyReader, u64)> {
    let len = serialized_len(&source)?;
    let (tx, rx) = sync_channel(BODY_PIPE_DEPTH);
    std::thread::spawn(move || {
        let mut writer = PipeWriter {
            tx,
            buf: Vec::with_capacity(BODY_CHUNK_BYTES),
        };
        if source.write_to(&mut writer).is_ok() {
            let _ = writer.flush();
        }
    });
    Ok((
        BodyReader {
            rx,
            current: Vec::new(),
            pos: 0,
        },
        len as u64,
    ))
}

/// The serializer's end of the body pipe: fills one chunk and hands it over.
struct PipeWriter {
    tx: SyncSender<Vec<u8>>,
    buf: Vec<u8>,
}

impl PipeWriter {
    fn hand_over(&mut self) -> io::Result<()> {
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(BODY_CHUNK_BYTES));
        self.tx
            .send(chunk)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Write for PipeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Fill the chunk to its size and no further, so a chunk never grows
        // past `BODY_CHUNK_BYTES` however long one serialized string is.
        let room = BODY_CHUNK_BYTES - self.buf.len();
        let take = bytes.len().min(room);
        self.buf.extend_from_slice(&bytes[..take]);
        if self.buf.len() >= BODY_CHUNK_BYTES {
            self.hand_over()?;
        }
        Ok(take)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.hand_over()
    }
}

/// The transport's end of the body pipe — the `Read` a
/// `reqwest::blocking::Body::sized` pumps in its own small reads.
pub struct BodyReader {
    rx: Receiver<Vec<u8>>,
    current: Vec<u8>,
    pos: usize,
}

impl Read for BodyReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.current.len() {
            // The serializer dropping its end — done, or gone — is EOF.
            let Ok(next) = self.rx.recv() else {
                return Ok(0);
            };
            self.current = next;
            self.pos = 0;
        }
        let n = out.len().min(self.current.len() - self.pos);
        out[..n].copy_from_slice(&self.current[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Everything a stream yields, read the way the transport reads it — the
/// tests' and probes' drain.
pub fn drain(mut body: BodyReader) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match body.read(&mut buf) {
            Ok(0) | Err(_) => return out,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_body_pipe_hands_over_bounded_chunks_and_the_declared_length() {
        // A body several chunks long: what the reader yields is the exact
        // serialization, no read ever spans more than one chunk, and the
        // count the header carries matches.
        let value = json!({"a": [1, 2, 3], "picture": "x".repeat(3 * BODY_CHUNK_BYTES + 17)});
        let expected = serde_json::to_vec(&value).unwrap();
        assert_eq!(serialized_len(&value).unwrap(), expected.len());
        let (mut body, len) = streamed_request(value).expect("streams");
        assert_eq!(len as usize, expected.len());
        let mut out = Vec::new();
        let mut buf = vec![0u8; 4 * BODY_CHUNK_BYTES];
        loop {
            match body.read(&mut buf).unwrap() {
                0 => break,
                n => {
                    assert!(n <= BODY_CHUNK_BYTES, "a read of {n} bytes spans chunks");
                    out.extend_from_slice(&buf[..n]);
                }
            }
        }
        assert_eq!(out, expected);
    }

    #[test]
    fn a_reader_dropped_early_ends_the_serializer_quietly() {
        // An Esc that ends the transport drops its end of the pipe; the
        // serializer thread must finish rather than block forever on a full
        // pipe. Nothing to assert but that this returns: a wedged writer
        // would leave a thread parked, not a failure.
        let value = json!({"picture": "x".repeat(64 * BODY_CHUNK_BYTES)});
        let (body, _) = streamed_request(value).expect("streams");
        drop(body);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    #[test]
    fn drain_reads_everything_the_pipe_carries() {
        let value = json!({"k": "v".repeat(BODY_CHUNK_BYTES * 2 + 5)});
        let (body, len) = streamed_request(value.clone()).expect("streams");
        let bytes = drain(body);
        assert_eq!(bytes, serde_json::to_vec(&value).unwrap());
        assert_eq!(len as usize, bytes.len());
    }
}

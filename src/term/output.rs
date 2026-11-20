//! Where a viewport's escapes go, and how they get there: **a frame at a
//! time**.
//!
//! Every paint queues its escapes and ends on one flush, so that a frame is
//! one write — the terminal never reads half of one (`docs/flicker.md`).
//! Written to std's `Stdout` it was not: that is a line writer over a 1 KB
//! buffer, which writes whenever it holds a newline and whenever it fills.
//! A frame that committed a row to scrollback went out as three writes, a
//! screenful of them as one write a row, and a terminal without synchronized
//! updates could paint in between. [`FrameWriter`] holds the frame until the
//! flush and hands it over whole (`docs/input-latency.md`).

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsFd;

/// Past this many bytes a frame is no longer held whole: what is held goes
/// out, and the frame goes on behind it. A purge rebuild writes thousands of
/// rows — megabytes — and no terminal takes that in one read; holding it
/// would only make the buffer the largest block this process allocates.
pub(super) const FRAME_SPILL_BYTES: usize = 64 * 1024;

/// A writer that holds what it is given until [`flush`](Write::flush), then
/// writes it in one piece (see the module docs).
pub(super) struct FrameWriter<W: Write> {
    out: W,
    frame: Vec<u8>,
}

impl<W: Write> FrameWriter<W> {
    pub(super) const fn new(out: W) -> Self {
        Self {
            out,
            frame: Vec::new(),
        }
    }

    /// Write what is held, in one piece. The frame is forgotten whether or
    /// not the terminal took it: a failed write ends the loop, and what it
    /// lost must not lead whatever is written next.
    fn send(&mut self) -> io::Result<()> {
        if self.frame.is_empty() {
            return Ok(());
        }
        let sent = self.out.write_all(&self.frame);
        self.frame.clear();
        sent
    }
}

impl<W: Write> Write for FrameWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.frame.len() + buf.len() > FRAME_SPILL_BYTES {
            self.send()?;
        }
        if buf.len() > FRAME_SPILL_BYTES {
            // A piece that could never be held goes straight on, behind what
            // was.
            self.out.write_all(buf)?;
        } else {
            self.frame.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()?;
        self.out.flush()
    }
}

/// Where a viewport's escapes go: the terminal — or, for a headless run,
/// which keeps the session but shows nothing, nowhere (`docs/headless.md`).
/// The sink is defence in depth under `InlineViewport::headless`'s early
/// returns: stdout is a headless run's answer, and an escape that reached
/// it would corrupt the JSON a script parses.
pub(super) enum Output {
    Terminal(FrameWriter<File>),
    Nowhere,
}

impl Output {
    /// The terminal behind stdout, through a descriptor of this viewport's
    /// own: std's `Stdout` would put its line writer between a frame and
    /// the terminal (the module docs).
    pub(super) fn terminal() -> io::Result<Self> {
        let fd = io::stdout().as_fd().try_clone_to_owned()?;
        Ok(Self::Terminal(FrameWriter::new(File::from(fd))))
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Terminal(out) => out.write(buf),
            Self::Nowhere => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Terminal(out) => out.flush(),
            Self::Nowhere => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal that keeps each `write` it is given apart.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<Vec<u8>>,
        flushes: usize,
        /// The most one `write` takes — a terminal slow to read.
        takes: Option<usize>,
        /// The next write fails.
        broken: bool,
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.broken) {
                return Err(io::Error::other("the terminal went away"));
            }
            let taken = self.takes.map_or(buf.len(), |most| most.min(buf.len()));
            self.writes.push(buf[..taken].to_vec());
            Ok(taken)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn written(out: &FrameWriter<Recorder>) -> Vec<u8> {
        out.out.writes.concat()
    }

    #[test]
    fn a_frame_reaches_the_terminal_as_one_write() {
        // Escapes, rows that end in newlines, more than the kilobyte std's
        // stdout holds: whatever a frame is made of, the terminal gets it
        // whole.
        let mut out = FrameWriter::new(Recorder::default());
        let mut frame = Vec::new();
        for row in 0..60 {
            let piece =
                format!("\x1b[{row};1H row {row} of a frame that commits to scrollback\r\n");
            out.write_all(piece.as_bytes()).expect("held");
            frame.extend_from_slice(piece.as_bytes());
        }
        assert!(frame.len() > 2048);
        assert!(out.out.writes.is_empty(), "nothing leaves before the flush");
        out.flush().expect("written");
        assert_eq!(out.out.writes.len(), 1, "one write for the frame");
        assert_eq!(written(&out), frame);
        assert_eq!(out.out.flushes, 1, "and the terminal is flushed behind it");
    }

    #[test]
    fn a_flush_with_nothing_held_writes_nothing() {
        // A frame that changed nothing is silence on the wire
        // (`docs/overlay-repaint.md`) — not an empty write.
        let mut out = FrameWriter::new(Recorder::default());
        out.flush().expect("nothing to do");
        out.write_all(b"frame").expect("held");
        out.flush().expect("written");
        out.flush().expect("nothing to do");
        assert_eq!(out.out.writes, [b"frame".to_vec()]);
    }

    #[test]
    fn a_frame_past_the_cap_goes_out_in_order() {
        // A purge rebuild is megabytes: it is not held whole, and what
        // leaves early leaves in order.
        let mut out = FrameWriter::new(Recorder::default());
        let mut frame = Vec::new();
        for row in 0..4000u32 {
            let piece = format!("row {row:>6} of a rebuilt conversation, wrapped to the width\r\n");
            out.write_all(piece.as_bytes()).expect("held");
            frame.extend_from_slice(piece.as_bytes());
            assert!(
                out.frame.len() <= FRAME_SPILL_BYTES,
                "the held frame is bounded"
            );
        }
        out.flush().expect("written");
        assert_eq!(written(&out), frame);
        let pieces = out.out.writes.len();
        assert!(
            pieces <= frame.len() / (FRAME_SPILL_BYTES / 2) + 1,
            "{pieces} writes for {} bytes: large pieces, not one a row",
            frame.len()
        );
    }

    #[test]
    fn one_piece_larger_than_the_cap_is_passed_straight_through() {
        let mut out = FrameWriter::new(Recorder::default());
        out.write_all(b"before ").expect("held");
        let big = vec![b'x'; FRAME_SPILL_BYTES + 1];
        out.write_all(&big).expect("written");
        out.write_all(b" after").expect("held");
        out.flush().expect("written");
        let mut expected = b"before ".to_vec();
        expected.extend_from_slice(&big);
        expected.extend_from_slice(b" after");
        assert_eq!(written(&out), expected);
        assert!(out.frame.capacity() <= 2 * FRAME_SPILL_BYTES);
    }

    #[test]
    fn a_terminal_slow_to_read_still_gets_the_whole_frame() {
        let mut out = FrameWriter::new(Recorder {
            takes: Some(7),
            ..Recorder::default()
        });
        out.write_all(b"a frame the terminal takes seven bytes of at a time")
            .expect("held");
        out.flush().expect("written");
        assert_eq!(
            written(&out),
            b"a frame the terminal takes seven bytes of at a time"
        );
    }

    #[test]
    fn a_frame_that_could_not_be_written_is_not_sent_again() {
        // The error reaches the caller — it ends the loop — and the bytes of
        // the frame that failed are not left to lead the next one.
        let mut out = FrameWriter::new(Recorder {
            broken: true,
            ..Recorder::default()
        });
        out.write_all(b"lost").expect("held");
        assert!(out.flush().is_err());
        out.write_all(b"next").expect("held");
        out.flush().expect("written");
        assert_eq!(written(&out), b"next");
    }
}

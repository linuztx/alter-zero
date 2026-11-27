//! A GIF's frames, counted from its blocks — never a pixel decoded.
//!
//! A GIF goes to a model as a still PNG of its first frame, since providers
//! take only still GIFs (`docs/images.md` *A GIF goes up as a still*). What a
//! fact line still says about the animation is how many frames it has, and
//! that is a walk over the file's blocks: each frame is one image descriptor,
//! and everything else between the header and the trailer is passed over by
//! its own length bytes. Pure over any reader, so the `read` tool counts the
//! bytes it holds and the image tool the file it just saved, at a few
//! kilobytes of buffer either way.

use std::io::{self, BufReader, Read};

/// The block that opens an extension (a graphic control, a comment, an
/// application's — the NETSCAPE2.0 loop count among them).
const EXTENSION: u8 = 0x21;
/// The block that opens a frame: its image descriptor.
const IMAGE: u8 = 0x2C;
/// The block that ends the file.
const TRAILER: u8 = 0x3B;
/// A colour table's flag, in the packed byte of the screen or a frame.
const COLOUR_TABLE: u8 = 0x80;

/// How many frames the GIF `reader` holds: its image descriptors, counted
/// by walking its blocks. `None` when it is not a GIF or holds no whole
/// frame; a file cut short, or one that ends on a block this reader does not
/// know, counts the frames it got through.
#[must_use]
pub fn gif_frame_count(reader: impl Read) -> Option<u32> {
    let mut reader = BufReader::with_capacity(8 * 1024, reader);
    // The header and the logical screen descriptor: the signature, the
    // canvas size, then the packed byte saying whether a global colour
    // table follows, and how large it is.
    let mut screen = [0u8; 13];
    reader.read_exact(&mut screen).ok()?;
    if !matches!(&screen[..6], b"GIF87a" | b"GIF89a") {
        return None;
    }
    skip_colour_table(&mut reader, screen[10]).ok()?;
    let mut frames = 0u32;
    // Any read that fails ends the walk where it stands: what was counted
    // is every frame read through whole.
    let _ = walk(&mut reader, &mut frames);
    (frames > 0).then_some(frames)
}

/// Walk the blocks after the screen descriptor, counting each frame read
/// through whole, until the trailer, a block this reader does not know, or
/// the end of the bytes.
fn walk(reader: &mut impl Read, frames: &mut u32) -> io::Result<()> {
    loop {
        match byte(reader)? {
            EXTENSION => {
                // Its label, then its data as sub-blocks.
                byte(reader)?;
                skip_sub_blocks(reader)?;
            }
            IMAGE => {
                // Position and size, then the packed byte of its own colour
                // table, the LZW code size, and the image data.
                let mut descriptor = [0u8; 9];
                reader.read_exact(&mut descriptor)?;
                skip_colour_table(reader, descriptor[8])?;
                byte(reader)?;
                skip_sub_blocks(reader)?;
                *frames += 1;
            }
            TRAILER => return Ok(()),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

/// Pass over the colour table a packed byte says follows: three bytes for
/// each of its 2^(n+1) colours.
fn skip_colour_table(reader: &mut impl Read, packed: u8) -> io::Result<()> {
    if packed & COLOUR_TABLE == 0 {
        return Ok(());
    }
    skip(reader, 3u64 << (u32::from(packed & 0x07) + 1))
}

/// Pass over a run of sub-blocks: each a length byte and that many bytes,
/// ended by a zero length.
fn skip_sub_blocks(reader: &mut impl Read) -> io::Result<()> {
    loop {
        match byte(reader)? {
            0 => return Ok(()),
            len => skip(reader, u64::from(len))?,
        }
    }
}

/// Pass over exactly `len` bytes.
fn skip(reader: &mut impl Read, len: u64) -> io::Result<()> {
    let skipped = io::copy(&mut reader.by_ref().take(len), &mut io::sink())?;
    if skipped == len {
        Ok(())
    } else {
        Err(io::ErrorKind::UnexpectedEof.into())
    }
}

/// One byte.
fn byte(reader: &mut impl Read) -> io::Result<u8> {
    let mut one = [0u8; 1];
    reader.read_exact(&mut one)?;
    Ok(one[0])
}

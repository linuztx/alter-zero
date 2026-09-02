//! Linux: the clipboard's own encoded image, streamed straight into the
//! paste folder.
//!
//! A screenshot tool puts its picture on the clipboard **as a PNG** — under
//! X11 as the `image/png` target, under Wayland as the `image/png` MIME
//! offer — and that is the only shape arboard ever asks a Linux owner for.
//! What arboard then does with it is decode it to RGBA and hand that over,
//! and what [`super::read_clipboard_image`] used to do next was encode the
//! RGBA back into a PNG on disk. For a 1920×1080 screenshot that round trip
//! is an 8 MB decode buffer, an encoder's working set and a growing output
//! vector, all to reproduce bytes the owner had already sent. Measured, one
//! Ctrl+V spiked the process by ~24 MB — and the freed blocks raised glibc's
//! dynamic `mmap` threshold past their own size, which is what later turned
//! every same-sized buffer into a permanent heap residue (`docs/memory.md`).
//!
//! So on Linux the paste asks the owner for the encoded bytes itself and
//! **copies them into the paste folder as they arrive**: a Wayland offer is a pipe
//! (`wl-clipboard-rs`, the crate arboard's Wayland backend is built on), and
//! an X11 selection is fetched a bounded slice at a time — `INCR` segments
//! as the owner sends them, each property read in 1 MiB pieces — so the read
//! holds about a megabyte whatever the picture weighs. It asks for
//! `image/png` first and the other formats the backend accepts after it
//! ([`TARGETS`]), each landing under its own extension, and it runs **before
//! arboard is constructed**, since that construction is an X11 connection and
//! a serving thread of its own. No decode, no encode, and a paste that was
//! slower than the screenshot tool is now a file copy.
//!
//! Anything the direct read cannot settle — no encoded target offered, a
//! server it can't reach, a transfer that stalls — answers `Ok(None)` or
//! [`StreamError::Failed`], and the caller falls through to arboard's own
//! path, so the worst case is exactly what it was. The one exception is a
//! stream past [`super::CLIPBOARD_IMAGE_MAX_BYTES`], which is refused
//! outright ([`StreamError::TooLarge`]): handing it to a path that decodes
//! would be the very cost the cap exists to prevent. Boundary I/O:
//! `tests/clipboard_linux.rs` drives it against a real X server (both
//! transfer shapes, a second format, and a clipboard with nothing to offer)
//! under Xvfb, and `scripts/smoke.sh` covers the no-server failure.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, Property, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::StreamError;

/// The encoded targets worth asking for, best first, with the extension
/// each is saved under — the same accepted set the pasted-file
/// path copies verbatim (`super::accepted_image_extension`).
const TARGETS: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

/// How long the selection owner gets to answer a conversion request —
/// arboard's own budget, since an owner may render the picture on demand.
const FIRST_REPLY_TIMEOUT: Duration = Duration::from_secs(4);

/// How long one `INCR` segment may take to follow the previous one.
const SEGMENT_TIMEOUT: Duration = Duration::from_secs(2);

/// The slice a property is fetched in, in the protocol's 32-bit units: 1 MiB
/// per round trip, which is what bounds the read's memory whatever the
/// picture weighs. (arboard fetches a property whole.)
const PROPERTY_SLICE_LONGS: u32 = 256 * 1024;

/// Stream the clipboard's encoded image into the paste folder `dir` (its
/// next number, `super::store_image`), or `Ok(None)` when no display server
/// offered one — the caller then takes arboard's
/// path. Tries the compositor the environment names: Wayland when
/// `WAYLAND_DISPLAY` is set (arboard's own order), then X11 when `DISPLAY`
/// is — XWayland included, since a Wayland session with no data-control
/// protocol still serves its clipboard over X11.
pub(super) fn stream_image_into(dir: &Path) -> Result<Option<PathBuf>, StreamError> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if set("WAYLAND_DISPLAY") {
        match wayland_stream(dir) {
            Ok(found) => return Ok(found),
            // A compositor this can't talk to falls back to X11 — arboard's
            // rule, since WAYLAND_DISPLAY alone doesn't prove a data-control
            // protocol is there.
            Err(WaylandFailure::Unavailable) => {}
            Err(WaylandFailure::Stream(error)) => return Err(error),
        }
    }
    if !set("DISPLAY") {
        return Ok(None);
    }
    x11_stream(dir)
}

/// Why a Wayland read didn't deliver: no compositor to ask (try X11), or a
/// stream that failed or was refused (report it).
enum WaylandFailure {
    Unavailable,
    Stream(StreamError),
}

/// The Wayland half: an offer arrives on a pipe, and a pipe copies to a file
/// through a stack buffer.
fn wayland_stream(dir: &Path) -> Result<Option<PathBuf>, WaylandFailure> {
    use wl_clipboard_rs::paste::{ClipboardType, Error, MimeType, Seat, get_contents};
    for (mime, ext) in TARGETS {
        match get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific(mime),
        ) {
            Ok((mut pipe, _mime)) => {
                let stored =
                    super::store_image(dir, ext, super::CLIPBOARD_IMAGE_MAX_BYTES, |out| {
                        io::copy(&mut pipe, out)
                            .map(Some)
                            .map_err(|e| format!("could not read the clipboard: {e}"))
                    })
                    .map_err(WaylandFailure::Stream)?;
                if let Some(path) = stored {
                    return Ok(Some(path));
                }
            }
            // The owner has other targets but not this one — ask for the next.
            Err(Error::NoMimeType) => {}
            Err(Error::ClipboardEmpty) => return Ok(None),
            Err(
                Error::SocketOpenError(_)
                | Error::WaylandConnection(_)
                | Error::MissingProtocol { .. },
            ) => return Err(WaylandFailure::Unavailable),
            Err(e) => {
                return Err(WaylandFailure::Stream(StreamError::Failed(format!(
                    "Wayland: {e}"
                ))));
            }
        }
    }
    Ok(None)
}

fn x11_err(e: impl std::fmt::Display) -> String {
    format!("X11: {e}")
}

/// The X11 half: one connection, one hidden window, and a selection request
/// per candidate target — the property copied out whole, or in the `INCR`
/// segments a large picture arrives in.
fn x11_stream(dir: &Path) -> Result<Option<PathBuf>, StreamError> {
    let x11 = X11::connect().map_err(StreamError::Failed)?;
    let mut found = None;
    for (mime, ext) in TARGETS {
        let target = x11.atom(mime).map_err(StreamError::Failed)?;
        match super::store_image(dir, ext, super::CLIPBOARD_IMAGE_MAX_BYTES, |out| {
            x11.fetch(target, out)
        }) {
            Ok(Some(path)) => {
                found = Some(path);
                break;
            }
            // Not offered — ask for the next target.
            Ok(None) => {}
            Err(error) => {
                x11.close();
                return Err(error);
            }
        }
    }
    x11.close();
    Ok(found)
}

/// One X11 connection with the window the owner's answers land on.
struct X11 {
    conn: RustConnection,
    win: u32,
    clipboard: Atom,
    incr: Atom,
    property: Atom,
}

impl X11 {
    fn connect() -> Result<Self, String> {
        let (conn, screen) = RustConnection::connect(None).map_err(x11_err)?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or_else(|| x11_err("no screen"))?
            .root;
        let win = conn.generate_id().map_err(x11_err)?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            COPY_FROM_PARENT,
            // The owner's segments announce themselves as property changes on
            // our window; nothing else is wanted.
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(x11_err)?;
        let mut x11 = Self {
            conn,
            win,
            clipboard: NONE,
            incr: NONE,
            property: NONE,
        };
        x11.clipboard = x11.atom("CLIPBOARD")?;
        x11.incr = x11.atom("INCR")?;
        x11.property = x11.atom("ALTER_ZERO_CLIPBOARD")?;
        Ok(x11)
    }

    fn atom(&self, name: &str) -> Result<Atom, String> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?
            .atom)
    }

    /// Let the window go; the connection itself closes with the value.
    fn close(&self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }

    /// Ask the owner to convert `CLIPBOARD` to `target` into our property and
    /// copy the answer to `out`. `Ok(None)` when the owner declines the
    /// target — it has no such picture.
    fn fetch(&self, target: Atom, out: &mut dyn Write) -> Result<Option<u64>, String> {
        let conn = &self.conn;
        conn.delete_property(self.win, self.property)
            .map_err(x11_err)?;
        conn.convert_selection(
            self.win,
            self.clipboard,
            target,
            self.property,
            CURRENT_TIME,
        )
        .map_err(x11_err)?;
        conn.flush().map_err(x11_err)?;

        let mut deadline = Instant::now() + FIRST_REPLY_TIMEOUT;
        let mut incremental = false;
        let mut written = 0u64;
        while Instant::now() < deadline {
            let Some(event) = conn.poll_for_event().map_err(x11_err)? else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            match event {
                Event::SelectionNotify(ev)
                    if ev.requestor == self.win && ev.selection == self.clipboard =>
                {
                    // A `NONE` property is the owner declining the target: it
                    // has no such picture to give (ICCCM §2.2).
                    if ev.property == NONE || ev.target != target {
                        return Ok(None);
                    }
                    // Ask only for the property's type first: a zero-length
                    // read answers with the type and the size, nothing else.
                    let head = conn
                        .get_property(false, self.win, self.property, AtomEnum::ANY, 0, 0)
                        .map_err(x11_err)?
                        .reply()
                        .map_err(x11_err)?;
                    if head.type_ == self.incr {
                        // INCR: the property holds a size hint, and *deleting*
                        // it is what tells the owner to start sending segments.
                        conn.get_property(true, self.win, self.property, self.incr, 0, 1)
                            .map_err(x11_err)?
                            .reply()
                            .map_err(x11_err)?;
                        conn.flush().map_err(x11_err)?;
                        incremental = true;
                        deadline = Instant::now() + SEGMENT_TIMEOUT;
                    } else if head.type_ == target {
                        written += self.drain_property(target, out)?;
                        return Ok(Some(written));
                    } else {
                        return Err(x11_err(
                            "the clipboard owner answered with an unexpected type",
                        ));
                    }
                }
                Event::PropertyNotify(ev)
                    if incremental
                        && ev.window == self.win
                        && ev.atom == self.property
                        && ev.state == Property::NEW_VALUE =>
                {
                    // One segment; an empty one closes the transfer.
                    let segment = self.drain_property(target, out)?;
                    if segment == 0 {
                        return Ok(Some(written));
                    }
                    written += segment;
                    deadline = Instant::now() + SEGMENT_TIMEOUT;
                }
                _ => {}
            }
        }
        Err(x11_err("the clipboard owner did not answer in time"))
    }

    /// Copy our property to `out` a slice at a time and delete it — the
    /// delete happens on the last slice, which under `INCR` is the owner's
    /// cue for the next segment. Returns the bytes copied.
    fn drain_property(&self, type_: Atom, out: &mut dyn Write) -> Result<u64, String> {
        let mut offset = 0u32;
        let mut total = 0u64;
        loop {
            let reply = self
                .conn
                .get_property(
                    true,
                    self.win,
                    self.property,
                    type_,
                    offset,
                    PROPERTY_SLICE_LONGS,
                )
                .map_err(x11_err)?
                .reply()
                .map_err(x11_err)?;
            if reply.format == 0 && reply.bytes_after > 0 {
                return Err(x11_err("the clipboard property changed type mid-transfer"));
            }
            out.write_all(&reply.value)
                .map_err(|e| format!("could not write the image: {e}"))?;
            total += reply.value.len() as u64;
            if reply.bytes_after == 0 {
                return Ok(total);
            }
            offset = offset
                .checked_add(PROPERTY_SLICE_LONGS)
                .ok_or_else(|| x11_err("the clipboard image is too large"))?;
        }
    }
}

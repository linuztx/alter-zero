//! Linux: the clipboard's `image/png` bytes, streamed straight to the temp
//! file.
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
//! So on Linux the paste asks the owner for the PNG itself and **copies the
//! bytes to the temp file as they arrive**: a Wayland offer is a pipe
//! (`wl-clipboard-rs`, the crate arboard's Wayland backend is built on), and
//! an X11 selection is fetched a bounded slice at a time — `INCR` segments
//! as the owner sends them, each property read in 1 MiB pieces — so the read
//! holds about a megabyte whatever the picture weighs. No decode, no encode,
//! and a paste that was slower than the screenshot tool is now a file copy.
//!
//! Anything the direct read cannot settle — no `image/png` offered, a
//! server it can't reach, a transfer that stalls — answers `Ok(None)` or
//! `Err`, and the caller falls through to arboard's own path, so the worst
//! case is exactly what it was. Boundary I/O: `tests/clipboard_linux.rs`
//! drives it against a real X server (both transfer shapes) under Xvfb, and
//! `scripts/smoke.sh` covers the no-clipboard failure.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, Property, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

/// How long the selection owner gets to answer the conversion request —
/// arboard's own budget, since an owner may render the picture on demand.
const FIRST_REPLY_TIMEOUT: Duration = Duration::from_secs(4);

/// How long one `INCR` segment may take to follow the previous one.
const SEGMENT_TIMEOUT: Duration = Duration::from_secs(2);

/// The slice a property is fetched in, in the protocol's 32-bit units: 1 MiB
/// per round trip, which is what bounds the read's memory whatever the
/// picture weighs. (arboard fetches a property whole.)
const PROPERTY_SLICE_LONGS: u32 = 256 * 1024;

/// Stream the clipboard's PNG into a kept temp file, or `Ok(None)` when the
/// clipboard offers no PNG (or what it offers isn't one) — the caller then
/// takes arboard's path. `Err` is a read that started and failed.
pub(super) fn stream_png_to_temp() -> Result<Option<PathBuf>, String> {
    let tmp = tempfile::Builder::new()
        .prefix("alter-zero-clipboard-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("could not create a temp file: {e}"))?;
    let streamed = {
        let mut out = io::BufWriter::new(tmp.as_file());
        let streamed = stream_png(&mut out)?;
        out.flush()
            .map_err(|e| format!("could not write the image: {e}"))?;
        streamed
    };
    if streamed.is_none() || image::image_dimensions(tmp.path()).is_err() {
        // Nothing offered, or an owner that mislabelled its data: the temp
        // file goes with the guard, and arboard gets its turn.
        return Ok(None);
    }
    super::keep_temp(tmp).map(Some)
}

/// Copy the clipboard's `image/png` bytes to `out`: the Wayland offer when a
/// compositor is reachable, else the X11 selection (arboard's own fallback
/// order). `Ok(Some(bytes))` on a transfer, `Ok(None)` when no PNG is on
/// offer.
fn stream_png(out: &mut impl Write) -> Result<Option<u64>, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match wayland_stream_png(out) {
            Ok(done) => return Ok(done),
            // A compositor this can't talk to falls back to X11 — arboard's
            // rule, since WAYLAND_DISPLAY alone doesn't prove a data-control
            // protocol is there.
            Err(WaylandFailure::Unavailable) => {}
            Err(WaylandFailure::Read(e)) => return Err(e),
        }
    }
    x11_stream_png(out)
}

/// Why a Wayland read didn't deliver: no compositor to ask (try X11), or a
/// transfer that failed (report it).
enum WaylandFailure {
    Unavailable,
    Read(String),
}

/// The Wayland half: the offer arrives on a pipe, and a pipe copies to a file
/// in a stack buffer.
fn wayland_stream_png(out: &mut impl Write) -> Result<Option<u64>, WaylandFailure> {
    use wl_clipboard_rs::paste::{ClipboardType, Error, MimeType, Seat, get_contents};
    match get_contents(
        ClipboardType::Regular,
        Seat::Unspecified,
        MimeType::Specific("image/png"),
    ) {
        Ok((mut pipe, _mime)) => io::copy(&mut pipe, out)
            .map(Some)
            .map_err(|e| WaylandFailure::Read(format!("could not read the clipboard: {e}"))),
        Err(Error::ClipboardEmpty | Error::NoMimeType) => Ok(None),
        Err(
            Error::SocketOpenError(_) | Error::WaylandConnection(_) | Error::MissingProtocol { .. },
        ) => Err(WaylandFailure::Unavailable),
        Err(e) => Err(WaylandFailure::Read(format!("Wayland: {e}"))),
    }
}

fn x11_err(e: impl std::fmt::Display) -> String {
    format!("X11: {e}")
}

/// The X11 half: ask the owner to convert `CLIPBOARD` to `image/png` into a
/// property on a window of ours, then copy that property out — whole, or in
/// the `INCR` segments a large picture arrives in.
fn x11_stream_png(out: &mut impl Write) -> Result<Option<u64>, String> {
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
    let atom = |name: &str| -> Result<Atom, String> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?
            .atom)
    };
    let clipboard = atom("CLIPBOARD")?;
    let png = atom("image/png")?;
    let incr = atom("INCR")?;
    let property = atom("ALTER_ZERO_CLIPBOARD")?;
    conn.delete_property(win, property).map_err(x11_err)?;
    conn.convert_selection(win, clipboard, png, property, CURRENT_TIME)
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
            Event::SelectionNotify(ev) if ev.requestor == win && ev.selection == clipboard => {
                // A `NONE` property is the owner declining the target: it has
                // no PNG to give (ICCCM §2.2).
                if ev.property == NONE || ev.target != png {
                    return Ok(None);
                }
                // Ask only for the property's type first: a zero-length read
                // answers with the type and the size, and nothing else.
                let head = conn
                    .get_property(false, win, property, AtomEnum::ANY, 0, 0)
                    .map_err(x11_err)?
                    .reply()
                    .map_err(x11_err)?;
                if head.type_ == incr {
                    // INCR: the property holds a size hint, and *deleting* it
                    // is what tells the owner to start sending segments.
                    conn.get_property(true, win, property, incr, 0, 1)
                        .map_err(x11_err)?
                        .reply()
                        .map_err(x11_err)?;
                    conn.flush().map_err(x11_err)?;
                    incremental = true;
                    deadline = Instant::now() + SEGMENT_TIMEOUT;
                } else if head.type_ == png {
                    written += drain_property(&conn, win, property, png, out)?;
                    return Ok(Some(written));
                } else {
                    return Err(x11_err(
                        "the clipboard owner answered with an unexpected type",
                    ));
                }
            }
            Event::PropertyNotify(ev)
                if incremental
                    && ev.window == win
                    && ev.atom == property
                    && ev.state == Property::NEW_VALUE =>
            {
                // One segment; an empty one closes the transfer.
                let segment = drain_property(&conn, win, property, png, out)?;
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

/// Copy `property` to `out` a slice at a time and delete it — the delete
/// happens on the last slice, which under `INCR` is the owner's cue for the
/// next segment. Returns the bytes copied.
fn drain_property(
    conn: &RustConnection,
    win: u32,
    property: Atom,
    type_: Atom,
    out: &mut impl Write,
) -> Result<u64, String> {
    let mut offset = 0u32;
    let mut total = 0u64;
    loop {
        let reply = conn
            .get_property(true, win, property, type_, offset, PROPERTY_SLICE_LONGS)
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

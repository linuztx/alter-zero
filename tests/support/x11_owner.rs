//! A minimal X11 selection owner that serves one `image/png` payload the way
//! a screenshot tool does — whole in a single property, or in `INCR`
//! segments when the payload is bigger than the chunk it is asked to use.
//!
//! Test support only: shared between `tests/clipboard_linux.rs` (which proves
//! the Ctrl+V read streams the served bytes verbatim, on both transfer
//! shapes) and the `clipboard_owner` example (which serves a synthetic
//! screenshot for driving the paste by hand) via `#[path]`. It is what lets
//! the read be exercised under a virtual X server with no clipboard tool
//! installed. See `docs/image-paste.md`, *Measuring*.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

/// How long the owner waits for the requestor to take one `INCR` segment
/// before it gives the transfer up.
const SEGMENT_TIMEOUT: Duration = Duration::from_secs(5);

/// The selection, held until dropped.
pub struct Owner {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    png: Atom,
}

impl Owner {
    /// Take ownership of `CLIPBOARD` and serve `payload` as `image/png` —
    /// in `INCR` segments of `incr_chunk` bytes when it is larger than one,
    /// whole otherwise. `None` owns the selection but offers no picture at
    /// all (the "nothing to paste" case).
    pub fn serve(
        payload: Option<Arc<Vec<u8>>>,
        incr_chunk: Option<usize>,
    ) -> Result<Owner, String> {
        let (conn, screen) = RustConnection::connect(None).map_err(|e| format!("X11: {e}"))?;
        let root = conn.setup().roots.get(screen).ok_or("X11: no screen")?.root;
        let win = conn.generate_id().map_err(|e| e.to_string())?;
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
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| e.to_string())?;
        let atom = |name: &str| -> Result<Atom, String> {
            Ok(conn
                .intern_atom(false, name.as_bytes())
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
                .atom)
        };
        let atoms = Atoms {
            clipboard: atom("CLIPBOARD")?,
            targets: atom("TARGETS")?,
            incr: atom("INCR")?,
            png: atom("image/png")?,
        };
        conn.set_selection_owner(win, atoms.clipboard, CURRENT_TIME)
            .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        let owner = conn
            .get_selection_owner(atoms.clipboard)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .owner;
        if owner != win {
            return Err("could not take the CLIPBOARD selection".to_string());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match conn.poll_for_event() {
                    Ok(Some(Event::SelectionRequest(req))) => {
                        let _ = handle(&conn, &atoms, payload.as_deref(), incr_chunk, &req);
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => std::thread::sleep(Duration::from_millis(1)),
                    Err(_) => break,
                }
            }
        });
        Ok(Owner {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Answer one request: the target list, the picture (whole or `INCR`), or a
/// refusal.
fn handle(
    conn: &RustConnection,
    atoms: &Atoms,
    payload: Option<&Vec<u8>>,
    incr_chunk: Option<usize>,
    req: &SelectionRequestEvent,
) -> Result<(), String> {
    // A requestor that names no property gets the data under the target atom
    // (the ICCCM's allowance for pre-ICCCM clients).
    let property = if req.property == NONE {
        req.target
    } else {
        req.property
    };
    let e = |err: x11rb::errors::ConnectionError| err.to_string();
    if req.target == atoms.targets {
        let mut targets = vec![atoms.targets];
        if payload.is_some() {
            targets.push(atoms.png);
        }
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::ATOM,
            &targets,
        )
        .map_err(e)?;
        return notify(conn, req, property);
    }
    let Some(png) = payload.filter(|_| req.target == atoms.png) else {
        return notify(conn, req, NONE);
    };
    match incr_chunk {
        Some(chunk) if png.len() > chunk => {
            // INCR: announce the size, then hand over a segment each time the
            // requestor deletes the previous one, closing with an empty one.
            conn.change_window_attributes(
                req.requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(e)?;
            let size = u32::try_from(png.len()).unwrap_or(u32::MAX);
            conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                property,
                atoms.incr,
                &[size],
            )
            .map_err(e)?;
            notify(conn, req, property)?;
            let mut offset = 0;
            loop {
                wait_for_delete(conn, req.requestor, property)?;
                let end = (offset + chunk).min(png.len());
                conn.change_property8(
                    PropMode::REPLACE,
                    req.requestor,
                    property,
                    atoms.png,
                    &png[offset..end],
                )
                .map_err(e)?;
                conn.flush().map_err(e)?;
                if offset >= png.len() {
                    break; // that was the empty segment that ends the transfer
                }
                offset = end;
            }
            conn.change_window_attributes(
                req.requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            )
            .map_err(e)?;
            conn.flush().map_err(e)
        }
        _ => {
            conn.change_property8(PropMode::REPLACE, req.requestor, property, atoms.png, png)
                .map_err(e)?;
            notify(conn, req, property)
        }
    }
}

/// Block until the requestor deletes `property` on `window` — its signal that
/// it has taken the segment there.
fn wait_for_delete(conn: &RustConnection, window: u32, property: Atom) -> Result<(), String> {
    let deadline = Instant::now() + SEGMENT_TIMEOUT;
    while Instant::now() < deadline {
        match conn.poll_for_event().map_err(|e| e.to_string())? {
            Some(Event::PropertyNotify(ev))
                if ev.window == window && ev.atom == property && ev.state == Property::DELETE =>
            {
                return Ok(());
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Err("the requestor never took the INCR segment".to_string())
}

/// Tell the requestor its conversion is done (or refused, with `NONE`).
fn notify(
    conn: &RustConnection,
    req: &SelectionRequestEvent,
    property: Atom,
) -> Result<(), String> {
    let event = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, event)
        .map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())
}

//! The process-global render policy, the placement interner, and what the
//! boundary found on disk.
//!
//! Three pieces of state that pure `ui` code has to be able to *read* while it
//! builds lines, and that only the boundary ever *writes* — the same shape
//! [`crate::links`]' URL interner has, and for the same reason: threading
//! them through `conversation_lines`, `tool_commit_lines`,
//! `transcript_item_lines` and every caller would put an argument on a dozen
//! signatures to carry one session-wide fact.
//!
//! - The **policy** is the `/settings` rows plus what the terminal turned out
//!   to be able to do. It starts unavailable, so a host that never called
//!   [`set_policy`] (every unit test) reserves no rows at all and behaves
//!   exactly as it did before images existed.
//! - The **placements** are `(path, cols, rows)` triples interned to a small
//!   id, because the id is all that fits in the per-cell carrier. Interning
//!   on the *size* as well as the path is what makes a resize correct: a
//!   narrower terminal produces a different id, so the boundary encodes a
//!   fresh protocol instead of re-placing the old one at the wrong size.
//! - The **sightings** are what the boundary found at each picture's path: a
//!   paste's pixel size, which no fact line carries, or no picture at all —
//!   a file gone since the conversation showed it, which reserves no rows.
//!
//! See `docs/images.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use super::geometry::{
    DEFAULT_FONT_SIZE, DEFAULT_IMAGE_WIDTH, FontSize, IMAGE_ID_MAX, image_cells,
};

/// What the session is doing about images: the two `/settings` display rows,
/// the payload row, and the two facts only the terminal can supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImagePolicy {
    /// `/settings` **Show images** — render pictures inline at all.
    pub show: bool,
    /// `/settings` **Image width**, in columns (see
    /// [`IMAGE_WIDTH_CHOICES`](super::geometry::IMAGE_WIDTH_CHOICES)).
    pub max_cols: u16,
    /// `/settings` **Auto-resize images** — downscale a large picture before
    /// it is sent to the model. Nothing to do with the display.
    pub auto_resize: bool,
    /// The terminal's cell size in pixels, as the boundary detected it.
    pub font: FontSize,
    /// Whether this host can draw a picture at all — a picker was built.
    /// `false` before the boundary reports in, which is why a unit test that
    /// never sets a policy sees the pre-image behaviour.
    pub available: bool,
}

impl Default for ImagePolicy {
    fn default() -> Self {
        Self {
            show: true,
            max_cols: DEFAULT_IMAGE_WIDTH,
            auto_resize: true,
            font: DEFAULT_FONT_SIZE,
            available: false,
        }
    }
}

fn policy_cell() -> &'static RwLock<ImagePolicy> {
    static POLICY: OnceLock<RwLock<ImagePolicy>> = OnceLock::new();
    POLICY.get_or_init(|| RwLock::new(ImagePolicy::default()))
}

/// Publish the session's image policy — the boundary's only write.
pub fn set_policy(policy: ImagePolicy) {
    if let Ok(mut guard) = policy_cell().write() {
        *guard = policy;
    }
}

/// The session's image policy.
#[must_use]
pub fn policy() -> ImagePolicy {
    policy_cell().read().map(|guard| *guard).unwrap_or_default()
}

/// Whether a picture would actually be drawn: the **Show images** row *and*
/// a terminal that can draw one. The one place the two are combined
/// ([`crate::settings::SessionSettings::checkpoints_active`]'s pattern).
#[must_use]
pub fn showing() -> bool {
    let policy = policy();
    policy.show && policy.available
}

/// Whether a large image is downscaled before it is sent to the model.
#[must_use]
pub fn auto_resizing() -> bool {
    policy().auto_resize
}

/// One reserved block: which picture, at what size, under which id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The carrier id — what a reserved cell actually holds.
    pub id: u32,
    /// The file the picture is read from.
    pub path: String,
    /// The block's width in columns.
    pub cols: u16,
    /// The block's height in rows.
    pub rows: u16,
}

/// The placement interner: append-only, keyed on everything that decides what
/// the encoder must produce. `placements[id - 1]` is the placement behind `id`.
struct Placements {
    ids: HashMap<(String, u16, u16), u32>,
    entries: Vec<Placement>,
}

/// Whether **any** placement has ever been reserved in this process.
///
/// The paint boundary's stamp otherwise has to walk every cell of every frame
/// looking for a carrier, and the overwhelmingly common session never shows a
/// picture at all. An atomic set once on the first intern turns that walk
/// into a single relaxed load.
static ANY_PLACEMENTS: AtomicBool = AtomicBool::new(false);

/// Whether this process has ever reserved a block — the paint boundary's
/// early-out, so a session that shows no picture never walks a frame looking
/// for one.
#[must_use]
pub fn any_placements() -> bool {
    ANY_PLACEMENTS.load(Ordering::Relaxed)
}

fn placements() -> &'static Mutex<Placements> {
    static PLACEMENTS: OnceLock<Mutex<Placements>> = OnceLock::new();
    PLACEMENTS.get_or_init(|| {
        Mutex::new(Placements {
            ids: HashMap::new(),
            entries: Vec::new(),
        })
    })
}

/// Reserve a block for the image at `path` (whose pixel size is `px`) in a
/// region `avail_cols` wide, returning the placement its cells will carry.
///
/// `None` when images are off, the boundary found the picture's file gone
/// ([`remember_missing`]), the terminal is too narrow, or the interner is
/// full — in every case the caller simply reserves no rows and the cell above
/// stands alone, exactly as before.
#[must_use]
pub fn place(path: &str, px: (u32, u32), avail_cols: u16) -> Option<Placement> {
    if !showing() || known_missing(path) {
        return None;
    }
    let policy = policy();
    let (cols, rows) = image_cells(px, policy.font, policy.max_cols, avail_cols)?;
    let key = (path.to_string(), cols, rows);
    let mut guard = placements().lock().ok()?;
    if let Some(&id) = guard.ids.get(&key) {
        return guard.entries.get(id as usize - 1).cloned();
    }
    let id = u32::try_from(guard.entries.len()).ok()?.checked_add(1)?;
    if id > IMAGE_ID_MAX {
        return None;
    }
    let placement = Placement {
        id,
        path: path.to_string(),
        cols,
        rows,
    };
    guard.entries.push(placement.clone());
    guard.ids.insert(key, id);
    ANY_PLACEMENTS.store(true, Ordering::Relaxed);
    Some(placement)
}

/// The placement behind an interned id — [`place`]'s other half, and what the
/// paint boundary looks up when it meets a carrier cell.
#[must_use]
pub fn placement(id: u32) -> Option<Placement> {
    if id == 0 {
        return None;
    }
    let guard = placements().lock().ok()?;
    guard.entries.get(id as usize - 1).cloned()
}

// --- What the boundary found on disk ---

/// What the boundary last found at a picture's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sighting {
    /// A picture of this pixel size.
    Found((u32, u32)),
    /// No picture at all: the file is gone, or no longer an image.
    Missing,
}

/// What the boundary has found at each picture's path.
///
/// The `read` tool's own fact line carries the size of the picture it read
/// ([`super::read_image_size`]), so an image read needs no size from here. A
/// **Ctrl+V paste** has no such line — the composer shows `[Image #1]` and the
/// message records only a path — so the boundary reads the header once when
/// the paste lands and leaves the answer here for the line builders. A paste
/// with no size simply isn't drawn.
///
/// Either kind can also lose its file while the conversation that shows it
/// lives on: a screenshot the agent saved under `/tmp` is gone after a
/// reboot, and resuming the session reserved its rows from the fact line all
/// the same — a screenful of blanks, with nothing left to draw into them. So
/// the boundary looks again whenever it lays a conversation out from history
/// ([`recheck_pictures`]), and a path it found nothing at is
/// [`Missing`](Sighting::Missing): [`place`] reserves no rows for it. See
/// `docs/images.md`, *A picture whose file is gone*.
fn sightings() -> &'static Mutex<HashMap<String, Sighting>> {
    static SIGHTINGS: OnceLock<Mutex<HashMap<String, Sighting>>> = OnceLock::new();
    SIGHTINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record(path: &str, sighting: Sighting) {
    if let Ok(mut guard) = sightings().lock() {
        guard.insert(path.to_string(), sighting);
    }
}

fn sighting(path: &str) -> Option<Sighting> {
    sightings().lock().ok()?.get(path).copied()
}

/// Record the pixel size of the image at `path` — a paste landing, a `read`
/// that found a picture there, a recheck that did. Overrides an earlier
/// [`remember_missing`]: the picture is back.
pub fn remember_size(path: &str, px: (u32, u32)) {
    record(path, Sighting::Found(px));
}

/// The recorded pixel size of the image at `path`, if the boundary read one
/// and has not since found it gone.
#[must_use]
pub fn known_size(path: &str) -> Option<(u32, u32)> {
    match sighting(path)? {
        Sighting::Found(px) => Some(px),
        Sighting::Missing => None,
    }
}

/// Record that the boundary looked for the picture at `path` and found none,
/// so [`place`] reserves no rows for it until it is seen again.
pub fn remember_missing(path: &str) {
    record(path, Sighting::Missing);
}

/// Whether the boundary last found no picture at `path`. `false` for a path
/// it never looked at — a live `read`'s picture is there by definition.
#[must_use]
pub fn known_missing(path: &str) -> bool {
    sighting(path) == Some(Sighting::Missing)
}

/// Look again for the picture at each of `paths`: `probe` is the boundary's
/// header read, `Some` with the picture's pixel size or `None` when there is
/// no picture there. A size found is recorded ([`remember_size`]) — which is
/// how a loaded conversation's pastes get theirs back — and a picture not
/// found is marked [`remember_missing`].
///
/// The boundary runs it before every layout from history: a load and every
/// purge rebuild, which drops the encoded pictures with the scrollback, so a
/// picture whose file is gone could never be drawn into the rows it would
/// otherwise reserve.
pub fn recheck_pictures<S: AsRef<str>>(
    paths: impl IntoIterator<Item = S>,
    mut probe: impl FnMut(&str) -> Option<(u32, u32)>,
) {
    for path in paths {
        let path = path.as_ref();
        match probe(path) {
            Some(px) => remember_size(path, px),
            None => remember_missing(path),
        }
    }
}

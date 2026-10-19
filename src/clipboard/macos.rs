//! macOS: AppKit's own TIFF→PNG coercion, for the pictures arboard can't read.
//!
//! A macOS screenshot lands on the pasteboard as a **`public.tiff`**, and
//! `NSImage.tiffRepresentation` writes **16 bits per channel** by default
//! (`bitsPerSample = 16,16,16,16`, `sampleFormat = 3`, measured on this
//! machine). arboard's macOS backend asks the pasteboard for that TIFF and
//! hands it to the `image` crate, whose TIFF decoder refuses the shape
//! (`Unhandled TIFF sample format 3 for 16 bits`) — so
//! [`arboard::Clipboard::get_image`] answers `Err(ConversionFailure)` for the
//! most ordinary macOS screenshot there is. [`super::read_clipboard_image`]'s
//! raw-bytes fallback turns every error into `None`, which is how a full
//! clipboard came to report **`no image on the clipboard`**.
//!
//! Note what *does* already work, and why this module is only a fallback: a
//! `public.png` copy and an 8-bit TIFF both read fine through arboard, because
//! `NSPasteboard` **coerces** the type arboard asks for — it re-encodes a PNG
//! owner's picture to TIFF on demand. What it will not do is downgrade a
//! 16-bit TIFF to the 8-bit one `image` can decode; it hands the raw
//! 16-bit bytes over as-is. So the fast path stays exactly as it was and only
//! the refusal reaches here.
//!
//! **Why `osascript` and not AppKit directly.** The conversion needs
//! `NSBitmapImageRep::representationUsingType:properties:`, which
//! `objc2-app-kit` marks `unsafe` — and this crate sets
//! `unsafe_code = "forbid"` (`Cargo.toml`). `/usr/bin/osascript` is a
//! first-party macOS binary that already speaks AppKit, so the crate stays
//! unsafe-free and gains no dependency; the cost is one subprocess (~150 ms
//! measured for a screenshot) on the *failure* path only, already off the
//! event loop on a worker thread. Claude Code's `imagePaste.ts` takes the same
//! route (`the clipboard as «class PNGf»`).
//!
//! Anything that still yields no picture — a text copy, a file copy (arboard's
//! `file_list()` handles those), a denied automation permission — answers
//! `Ok(None)` / [`StreamError::Failed`] and the caller reports its own
//! refusal. Boundary I/O, so like the rest of this file's clipboard half it
//! is exercised against the real pasteboard rather than unit-tested; the
//! script's *shape* (see [`applescript`]) is pinned by a test.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::StreamError;

/// The AppleScript that asks AppKit for the pasteboard's picture **as PNG**,
/// whatever flavour the owner planted.
///
/// `the clipboard as «class PNGf»` is a *coercion*: AppKit loads the
/// pasteboard's image (a 16-bit TIFF, a PNG, a file URL it can open) and
/// re-encodes it as PNG, exactly as a ⌘V into Preview would. It fails with
/// `-1700` ("can't make … into the expected type") on a clipboard holding no
/// picture at all, which is what makes it a probe as well as a converter.
///
/// The destination comes in through `argv` (`item 1 of argv`), never
/// interpolated into the script: an apostrophe in a path must not end the
/// string literal. `set eof fp to 0` runs first so a longer file left by an
/// earlier attempt cannot leave bytes past the PNG's `IEND`, and the error
/// branch closes the handle before re-raising so a failed write cannot leak
/// it.
fn applescript() -> &'static str {
    "on run argv\n\
     set outPath to item 1 of argv\n\
     set png_data to (the clipboard as «class PNGf»)\n\
     set fp to open for access POSIX file outPath with write permission\n\
     try\n\
     set eof fp to 0\n\
     write png_data to fp\n\
     on error errMsg number errNum\n\
     close access fp\n\
     error errMsg number errNum\n\
     end try\n\
     close access fp\n\
     end run"
}

/// Ask AppKit for the pasteboard's picture as PNG, written into `path`.
///
/// `Ok(())` when it produced one. `Err("")` — an empty reason — when the
/// pasteboard simply held no picture (`-1700`): the ordinary answer for a text
/// or file copy, not worth a notice of its own. `Err(reason)` for a real
/// failure (osascript missing, automation denied, a wedged pasteboard
/// server), which the caller surfaces.
fn coerce_png(path: &Path) -> Result<(), String> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript())
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("could not run osascript: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr);
    let reason = reason.trim();
    // "Can't make … into the expected type": nothing picture-shaped on the
    // pasteboard. An empty reason is "not this path", never an error.
    if reason.contains("-1700") {
        return Err(String::new());
    }
    if reason.is_empty() {
        return Err("the clipboard image could not be converted to PNG".to_string());
    }
    Err(refusal_for(reason))
}

/// Convert whatever picture the pasteboard holds into the paste folder `dir`
/// as its next number, or `Ok(None)` when it holds none. See the module docs.
///
/// AppKit writes to a real path, so its PNG lands in a hidden **staging file
/// inside the folder** ([`super::staging_file`]) — and that file never takes
/// a number itself. **What the paste keeps is a copy with the metadata
/// stripped**, and that is a privacy requirement, not a preference:
/// `«class PNGf»` copies the pasteboard's metadata along with its pixels — a
/// TIFF's `ImageDescription`, `Artist` and `Software` tags land in the PNG's
/// `eXIf` chunk, and AppKit synthesises an `iTXt` XMP packet carrying them
/// again as `dc:description`, `dc:creator` and `xmp:CreatorTool` (measured —
/// `tests/clipboard_macos.rs` checks AppKit still does). A photo's camera and
/// location tags ride the same way, and the picture is then `base64`d into
/// **every later request** of the conversation (`docs/context.md`).
///
/// [`super::store_png_without_metadata`] makes that copy chunk by chunk,
/// keeping only the ones that draw the picture — never decoding it — through
/// the same `store_image` every other paste takes, so it is capped,
/// header-checked and numbered like them. AppKit's file goes when this
/// returns, whichever way the paste ended.
pub(super) fn coerce_into(dir: &Path) -> Result<Option<PathBuf>, StreamError> {
    let converted = super::staging_file(dir, "png").map_err(StreamError::Failed)?;
    match coerce_png(converted.path()) {
        Ok(()) => {}
        // `-1700`: no picture on the pasteboard.
        Err(reason) if reason.is_empty() => return Ok(None),
        Err(reason) => return Err(StreamError::Failed(reason)),
    }
    super::store_png_without_metadata(dir, converted.path(), super::CLIPBOARD_IMAGE_MAX_BYTES)
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The destination travels as `argv`, so nothing a path contains can
    /// change the script's shape, and the two properties the conversion
    /// depends on are spelled out.
    #[test]
    fn the_applescript_takes_its_path_as_an_argument() {
        let script = applescript();
        assert!(script.contains("item 1 of argv"), "{script}");
        assert!(script.contains("«class PNGf»"), "{script}");
        assert!(
            !script.contains("POSIX file \""),
            "no path may be interpolated into the script: {script}"
        );
        assert!(
            script.contains("set eof fp to 0"),
            "a shorter picture must not inherit a longer file's tail: {script}"
        );
    }

    /// One `-e` argument, balanced `try`/`end try` — a malformed script would
    /// fail on every paste, so the shape is pinned rather than left to the
    /// boundary test.
    #[test]
    fn the_applescript_is_balanced() {
        let script = applescript();
        assert_eq!(script.matches("\nend try").count(), 1, "{script}");
        assert_eq!(script.matches("\ntry\n").count(), 1, "{script}");
        assert_eq!(script.matches("open for access").count(), 1, "{script}");
        assert_eq!(script.matches("close access").count(), 2, "{script}");
    }
}

// ===== where a paste can run at all =====
//
// Linux answers "does this session name a display" from two environment
// variables (`super::require_display`). macOS has a different gate, and — as
// on Linux — the point of asking *first* is to say something useful instead
// of letting a backend report whatever it runs into.

/// Why a macOS paste is refused in a session with no window server, and what
/// to do instead: the same read-by-path escape hatch the Linux refusal gives,
/// because it works everywhere.
pub(super) const NO_GUI_SESSION_REASON: &str = "no desktop clipboard in this session (it is \
     not a GUI login, so `NSPasteboard` has nothing to read). Save the image file where this \
     session can reach it, then ask Alter Zero to read its path";

/// Why a macOS paste is refused when the automation permission is missing:
/// macOS gates one app reading another's clipboard behind **TCC**, and the
/// prompt only appears for a GUI session. Naming the exact pane matters —
/// "automation" is a sub-page most people never open.
pub(super) const AUTOMATION_DENIED_REASON: &str = "macOS has not allowed this terminal to read \
     the clipboard. Allow it in System Settings → Privacy & Security → Automation, then try \
     again — or save the image file where this session can reach it and ask Alter Zero to read \
     its path";

/// The `launchctl managername` value of a **window-server** session. `Aqua` is
/// a logged-in desktop; `Background` (a daemon, or an `ssh` login) and
/// `LoginWindow` (the lock screen) have no pasteboard to read.
const AQUA_SESSION: &str = "Aqua";

/// Whether a `launchctl managername` answer names a desktop session we can
/// read a pasteboard from. Pure over the string [`require_gui_session`] reads,
/// so the rule is unit-tested without touching the real session: anything but
/// `Aqua` — an empty answer included — is not one.
pub(super) fn is_desktop_session(managername: &str) -> bool {
    managername.trim() == AQUA_SESSION
}

/// Whether this process has a pasteboard at all, refused **before** arboard or
/// `osascript` is constructed — the macOS counterpart of
/// [`super::require_display`].
///
/// `launchctl managername` answers `Aqua` for a logged-in desktop and
/// `Background` for a daemon or an `ssh` login. `ssh` is worth noting: unlike
/// Linux, macOS *does* keep a display around for a terminal app, but a session
/// that never entered the window server cannot read the pasteboard, and
/// arboard's `generalPasteboard` answers NULL there. Asking first is what lets
/// the refusal say why instead of "clipboard unavailable: …".
pub(super) fn require_gui_session() -> Result<(), String> {
    // `None` is "could not ask" — `launchctl` is in `/bin` on every macOS, and
    // an answer we could not get must not refuse a paste that would work. The
    // read itself then answers with whatever it runs into.
    let name = session_name();
    judge_session(name.as_deref())
}

/// The rule [`require_gui_session`] applies, pure over `launchctl`'s answer so
/// it is unit-tested without touching the real session: `None` (could not ask)
/// allows the paste, `Some("Aqua")` allows it, anything else refuses.
pub(super) fn judge_session(name: Option<&str>) -> Result<(), String> {
    match name {
        None => Ok(()),
        Some(name) if is_desktop_session(name) => Ok(()),
        Some(_) => Err(NO_GUI_SESSION_REASON.to_string()),
    }
}

/// `launchctl managername`'s answer, trimmed, or `None` when it could not be
/// read.
fn session_name() -> Option<String> {
    let output = Command::new("/bin/launchctl")
        .arg("managername")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Turn an `osascript` failure into the reason the paste reports.
///
/// `-1743` is `errAEEventNotPermitted`: TCC refused the automation request.
/// It is the one failure a user can fix, so it names the pane and what to
/// switch on. Everything else keeps AppleScript's own wording, which is more
/// specific than anything this function could invent.
pub(super) fn refusal_for(reason: &str) -> String {
    if reason.contains("-1743") {
        AUTOMATION_DENIED_REASON.to_string()
    } else {
        format!("the clipboard image could not be converted to PNG: {reason}")
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn the_session_rule_allows_aqua_and_an_unaskable_host() {
        assert_eq!(judge_session(Some("Aqua")), Ok(()));
        assert_eq!(
            judge_session(None),
            Ok(()),
            "not being able to ask must not refuse a paste that would work"
        );
        for other in ["Background", "LoginWindow", ""] {
            assert_eq!(
                judge_session(Some(other)),
                Err(NO_GUI_SESSION_REASON.to_string()),
                "{other:?} has no pasteboard"
            );
        }
    }

    #[test]
    fn only_aqua_is_a_desktop_session() {
        assert!(is_desktop_session("Aqua"));
        assert!(
            is_desktop_session(" Aqua\n"),
            "launchctl answers with a newline"
        );
        for other in ["Background", "LoginWindow", "", "aqua", "StandardIO"] {
            assert!(!is_desktop_session(other), "{other:?} is not a desktop");
        }
    }

    #[test]
    fn both_refusals_name_the_way_around_them() {
        for reason in [NO_GUI_SESSION_REASON, AUTOMATION_DENIED_REASON] {
            assert!(reason.contains("read its path"), "{reason}");
        }
        assert!(
            AUTOMATION_DENIED_REASON.contains("Privacy & Security → Automation"),
            "the permission refusal names the exact pane: {AUTOMATION_DENIED_REASON}"
        );
        assert!(
            NO_GUI_SESSION_REASON.contains("NSPasteboard") || NO_GUI_SESSION_REASON.contains("GUI")
        );
    }

    #[test]
    fn a_tcc_refusal_is_explained_and_anything_else_keeps_its_cause() {
        let denied = refusal_for("execution error: Not authorized to send Apple events (-1743)");
        assert_eq!(denied, AUTOMATION_DENIED_REASON);
        let other = refusal_for("something odd happened");
        assert!(other.contains("something odd happened"), "{other}");
    }
}

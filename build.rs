//! The build script: the idle text layout (`docs/text-layout.md`).
//!
//! It writes one linker script and asks for it on the `alter-zero` binary's
//! link, and does nothing else. The script gathers the functions an idle
//! session runs (`layout/idle-functions.txt`) into a section of their own,
//! so the code a session keeps in memory is the code it runs: a third less
//! resident memory at rest, and a figure that stops moving by a megabyte
//! whenever an unrelated change shifts the default order.
//!
//! It applies to the builds people run — `release` and `dist`, on a Linux
//! target, where GNU ld or lld reads the script — and stands aside
//! otherwise. A `dev` build is linked as it always was (the script costs a
//! second or two per link there, for a binary nobody leaves idling);
//! another OS has another object format; a `-fuse-ld=` naming a linker that
//! takes no such script (gold, mold) is noticed in the flags and skipped
//! with a warning rather than left to fail the link.
//! `ALTER_ZERO_TEXT_LAYOUT=0` turns it off for any build and `=1` on for a
//! `dev` one. The binary runs the same either way; only where its functions
//! sit differs.

use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "layout/script.rs"]
mod script;

/// The list, relative to the package root (a build script's cwd).
const LIST: &str = "layout/idle-functions.txt";

/// The variable that turns the layout off for a build.
const SWITCH: &str = "ALTER_ZERO_TEXT_LAYOUT";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=layout/script.rs");
    println!("cargo::rerun-if-changed={LIST}");
    println!("cargo::rerun-if-env-changed={SWITCH}");
    if let Some(why) = stand_aside() {
        if !why.is_empty() {
            println!("cargo::warning=the idle text layout is off for this build: {why}");
        }
        return;
    }
    let list = fs::read_to_string(LIST).unwrap_or_else(|err| panic!("{LIST}: {err}"));
    let groups = script::groups(&list).unwrap_or_else(|err| panic!("{LIST}: {err}"));
    if groups.iter().all(|group| group.patterns.is_empty()) {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let path = out.join("idle-text.ld");
    fs::write(&path, script::linker_script(&groups))
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    // `-T {script}`, two arguments so no path can be mis-split: the form
    // gcc and clang take as drivers and ld takes directly. On the binary
    // only — tests and examples are not what idles in a terminal.
    println!("cargo::rustc-link-arg-bins=-T");
    println!("cargo::rustc-link-arg-bins={}", path.display());
    // What `tests/text_layout.rs` reads to know the binary was laid out.
    println!("cargo::rustc-env=ALTER_ZERO_TEXT_LAYOUT_ACTIVE=1");
}

/// Why this build is not laid out, when it is not: `Some("")` for a reason
/// that needs no saying (another OS, a `dev` build, the switch), `Some(why)`
/// for one worth a warning, `None` to go ahead.
fn stand_aside() -> Option<String> {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return Some(String::new());
    }
    let switch = env::var(SWITCH)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match switch.as_str() {
        "0" | "false" | "off" | "no" => return Some(String::new()),
        "1" | "true" | "on" | "yes" => {}
        // Unset (or anything else): the builds that are run, not the ones
        // that are tested. `PROFILE` is `release` for the release profile
        // and every profile inheriting it (`dist`), `debug` otherwise.
        _ if env::var("PROFILE").as_deref() != Ok("release") => return Some(String::new()),
        _ => {}
    }
    // gold and mold cannot read the script; when the flags name one, say so
    // and link as before rather than fail.
    let flags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    if let Some(linker) = script::unsupported_linker(&flags) {
        return Some(format!(
            "the flags select {linker}, which cannot read the linker script ({SWITCH}=0 says the same without this warning)"
        ));
    }
    None
}

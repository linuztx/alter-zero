//! Reading a marketplace manifest must cost what the catalog **keeps**, not
//! the document it reads (`docs/memory.md` *The rule*, `docs/plugins.md`).
//!
//! Claude's official marketplace is ~190 KB of JSON for 317 plugins, and the
//! `/plugins` page reads it every time it opens. A `serde_json::Value` tree of
//! it would cost several times its size, and glibc would keep those pages
//! after the tree was dropped — so the reader streams the file into typed
//! records, skipping every key it does not show and capping descriptions as
//! they are read. This gate holds both halves: what reading and dropping the
//! catalog leaves behind, and what holding it costs.
//!
//! An integration test of its own because the reading is process-wide: a
//! lone `#[test]` in its own binary has nothing running beside it.

use std::fmt::Write as _;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use alter_zero::plugins::{PluginSource, read_marketplace};

/// The process's resident set in bytes — `None` where `/proc` isn't
/// available (the probe then skips rather than asserting on noise).
fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: usize = field.trim().trim_end_matches(" kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// A marketplace in the official one's proportions: every source shape it
/// carries, a long description on one entry in eight, and the fields the
/// catalog never shows (`lspServers`, `keywords`, `strict`, `tags`, a skills
/// list) on a share of the rest.
fn marketplace(entries: usize) -> String {
    let short = "Code intelligence and review helpers for everyday development work. ";
    let long = "A long description that runs on well past what a catalog row shows. ".repeat(16);
    let mut text = String::from(
        r#"{"$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
"name": "synthetic-official", "description": "A synthetic marketplace",
"owner": {"name": "Synthetic", "email": "support@example.invalid"},
"renames": {"old-one": "plugin-1", "old-two": "plugin-2"},
"plugins": ["#,
    );
    for i in 0..entries {
        if i > 0 {
            text.push(',');
        }
        let description = if i % 8 == 0 { long.as_str() } else { short };
        let source = match i % 4 {
            0 => format!(r#""./plugins/plugin-{i}""#),
            1 => format!(
                r#"{{"source": "url", "url": "https://github.com/vendor-{i}/plugin-{i}.git",
"sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}}"#
            ),
            2 => format!(
                r#"{{"source": "git-subdir", "url": "https://github.com/vendor-{i}/monorepo.git",
"path": "plugins/plugin-{i}", "ref": "main", "sha": "faf5305385de8afed9468904e8639be737aff39e"}}"#
            ),
            _ => r#"{"source": "command", "command": "tool plugin path", "mode": "link"}"#.into(),
        };
        let extra = if i % 3 == 0 {
            r#", "strict": false, "keywords": ["one", "two", "three"], "tags": ["community-managed"],
"skills": ["./skills/a", "./skills/b"],
"lspServers": {"server": {"command": "server", "args": ["--stdio"],
"extensionToLanguage": {".c": "c", ".h": "c", ".cpp": "cpp"}}}"#
        } else {
            ""
        };
        write!(
            text,
            r#"
{{"name": "plugin-{i}", "description": "{description}", "category": "development",
"author": {{"name": "Vendor {i}", "email": "vendor-{i}@example.invalid"}},
"homepage": "https://example.invalid/plugin-{i}", "source": {source}{extra}}}"#
        )
        .expect("writes to a string");
    }
    text.push_str("]}\n");
    text
}

fn read(path: &Path) -> usize {
    let (_, entries) =
        read_marketplace(BufReader::new(File::open(path).expect("opens"))).expect("reads");
    entries.len()
}

#[test]
fn reading_a_marketplace_keeps_only_the_catalog() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let small = dir.path().join("small.json");
    let big = dir.path().join("big.json");
    std::fs::write(&small, marketplace(40)).expect("writes");
    let text = marketplace(400);
    let file_len = text.len();
    std::fs::write(&big, text).expect("writes");
    assert!(file_len > 150_000, "the synthetic file is {file_len} bytes");

    // Warm the read path (and the allocator's arena) on a small file, so the
    // readings below see this file's cost rather than first-touch faults.
    for _ in 0..3 {
        assert_eq!(read(&small), 40);
    }

    // Held: the page open must cost less than the file it read. This first
    // read of the big file is also the honest price of opening the page
    // once — the transient parts of the read included, since the arena
    // keeps what it was grown to.
    let before = rss_bytes().expect("rss");
    let (header, entries) =
        read_marketplace(BufReader::new(File::open(&big).expect("opens"))).expect("reads");
    let held = rss_bytes().expect("rss").saturating_sub(before);
    assert_eq!(header.name, "synthetic-official");
    assert_eq!(entries.len(), 400);
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e.source, PluginSource::Unsupported { .. }))
            .count(),
        100
    );
    assert!(
        held < file_len,
        "holding the catalog of a {file_len}-byte marketplace grew the resident set by {held} \
         bytes — the reader keeps more than the catalog shows"
    );
    drop((header, entries));

    // Read and dropped, again and again: closing and reopening the page must
    // not add to what one open cost. One more read first lets the arena
    // settle around a catalog's worth of blocks (measured: ~76 KB on the
    // second read, then flat to within a page or two over ten more).
    assert_eq!(read(&big), 400);
    let before = rss_bytes().expect("rss");
    for _ in 0..5 {
        assert_eq!(read(&big), 400);
    }
    let retained = rss_bytes().expect("rss").saturating_sub(before);
    eprintln!("file {file_len} B · held {held} B · five more reads retained {retained} B");
    assert!(
        retained < 64 * 1024,
        "reading a {file_len}-byte marketplace five more times grew the resident set by \
         {retained} bytes — something of the document outlives each read"
    );
}

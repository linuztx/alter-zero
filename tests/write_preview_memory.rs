//! A `write` streaming into its cell must cost memory in proportion to what
//! the cell **shows** — its first and newest ten lines — not to the file it
//! is writing (`docs/write-streaming.md`, `docs/memory.md`).
//!
//! The preview used to keep every line it had coloured: about 0.9 KB of
//! styled segments a line, ~19 times the file, for as long as the call was
//! live. glibc keeps what the process has once used, so one 2,000-line write
//! left the app ~2.3 MB bigger for the rest of the session, and a 5,000-line
//! one ~5.8 MB.
//!
//! An integration test for the reason `model_parse_memory.rs` is one: the
//! reading is process-wide, and a lone `#[test]` in its own binary has
//! nothing running beside it to pollute it.

use alter_zero::app::App;
use alter_zero::stream::ToolCallFragment;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// The process's resident set in bytes — `None` where `/proc` isn't available
/// (the probe then skips rather than asserting on noise).
fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: usize = field.trim().trim_end_matches(" kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// `lines` lines of ordinary Python — a few syntax tokens each, the shape a
/// model writes.
fn source(lines: usize) -> String {
    let mut out = String::new();
    for i in 1..=lines {
        out.push_str(&match i % 4 {
            1 => format!("def handler_{i}(alpha, beta, gamma=None):\n"),
            2 => format!("    value = compute(alpha, beta) + {i}  # step {i}\n"),
            3 => format!("    if value > {}: return \"over-{i}\"\n", i * 3),
            _ => format!("    return [value, {i}, 'item-{i}'] if gamma else value\n"),
        });
    }
    out
}

/// The `write` call's arguments, as the model sends them.
fn write_arguments(path: &str, content: &str) -> String {
    serde_json::to_string(&serde_json::json!({"path": path, "content": content}))
        .expect("the arguments encode")
}

/// Stream a `write` call's `arguments` eight bytes a piece, rendering the
/// live region every eighth piece as the frame loop would.
fn stream_write(app: &mut App, arguments: &str, area: Rect, buf: &mut Buffer) {
    for (k, piece) in arguments.as_bytes().chunks(8).enumerate() {
        app.push_tool_call_fragment(&ToolCallFragment {
            index: 0,
            name: if k == 0 { "write" } else { "" }.to_string(),
            arguments: String::from_utf8(piece.to_vec()).expect("ASCII"),
        });
        if k % 8 == 0 {
            buf.reset();
            alter_zero::ui::render_live(area, buf, app);
        }
    }
    buf.reset();
    alter_zero::ui::render_live(area, buf, app);
}

/// Everything the buffer shows, row by row.
fn screen(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn a_streaming_write_holds_what_its_cell_shows_not_the_whole_file() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let area = Rect::new(0, 0, 120, 40);
    let mut buf = Buffer::empty(area);
    let mut app = App::new();

    // A small write through the same path first: the tokenizer's tables and
    // the highlighter's grammars load once per process, on first use, and
    // every highlighted cell paid that before this preview existed.
    app.begin_stream();
    stream_write(
        &mut app,
        &write_arguments("warm.py", &source(20)),
        area,
        &mut buf,
    );
    let _ = app.finish_stream();
    app.begin_stream();

    // The model's side of the stream is built before the reading: only what
    // the app keeps of it is measured.
    let content = source(5_000);
    let arguments = write_arguments("big.py", &content);
    let before = rss_bytes().expect("rss");
    stream_write(&mut app, &arguments, area, &mut buf);
    let after = rss_bytes().expect("rss");

    // It really is previewing the file: the newest line is on screen.
    let shown = screen(&buf);
    assert!(
        shown.contains("5000 ") && shown.contains("Write(big.py)"),
        "the strip does not show the streaming file:\n{shown}"
    );

    // What the cell shows is twenty lines; what it has to keep beside them is
    // the decoded file (one copy) and the highlighter's state. Budget four
    // copies' worth — ample over that, a fifth of the ~19 copies the
    // every-line cache held.
    let growth = after.saturating_sub(before);
    let budget = 4 * content.len();
    assert!(
        growth < budget,
        "streaming a {}-byte write grew the resident set by {growth} bytes \
         (budget {budget}) — the preview is holding colours for lines its cell \
         cannot show",
        content.len()
    );
}

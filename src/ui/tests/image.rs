//! The rows a picture reserves in the conversation (`docs/images.md`).
//!
//! The policy is process-global — the module's own doc says why — so every
//! test here holds [`policy_lock`] and puts the default back on the way out.

use std::sync::{Mutex, MutexGuard, OnceLock};

use super::*;
use crate::app::{HistoryItem, Message, Role, ToolCall, ToolStatus};
use crate::images::{self, ImagePolicy};

fn policy_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A session whose terminal can draw, at a 10x20 cell.
fn drawing(max_cols: u16) -> MutexGuard<'static, ()> {
    let guard = policy_lock();
    images::set_policy(ImagePolicy {
        show: true,
        available: true,
        max_cols,
        auto_resize: true,
        font: (10, 20),
    });
    guard
}

fn restore() {
    images::set_policy(ImagePolicy::default());
}

/// A resolved image `read` of `path`, exactly as the executor records one.
fn image_read(path: &str, px: (u32, u32)) -> HistoryItem {
    HistoryItem::Tool(ToolCall {
        name: "read".to_string(),
        args: path.to_string(),
        status: ToolStatus::Ok,
        output: crate::llm::tools::format_read_image("PNG", px.0, px.1, 4096),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        approval_note: None,
        arguments: Some(format!("{{\"path\":\"{path}\"}}")),
        batch: None,
        call_id: None,
    })
}

fn marked_rows(lines: &[Line<'static>]) -> Vec<(u32, u16)> {
    lines
        .iter()
        .filter_map(|line| {
            let span = line.spans.first()?;
            images::carrier_parts(span.style.underline_color?)
        })
        .collect()
}

#[test]
fn an_image_read_reserves_a_blank_row_then_the_block() {
    let _guard = drawing(120);
    let item = image_read("/tmp/cat.png", (700, 689));
    let lines = image_block_lines(&item, 100);
    // Blank spacer first — the picture never butts against the `⎿` row.
    assert!(lines[0].spans.is_empty(), "a blank row leads");
    let marks = marked_rows(&lines);
    assert_eq!(lines.len(), marks.len() + 1, "every other row is reserved");
    // One id, rows numbered from 0 — which is how the paint boundary finds
    // the block's top-left corner.
    let id = marks[0].0;
    assert!(marks.iter().all(|&(other, _)| other == id));
    assert_eq!(
        marks.iter().map(|&(_, row)| row).collect::<Vec<_>>(),
        (0..u16::try_from(marks.len()).unwrap()).collect::<Vec<_>>()
    );
    // …and the block is exactly the footprint the geometry computed.
    let (cols, rows) = images::image_cells((700, 689), (10, 20), 120, 100).unwrap();
    assert_eq!(marks.len(), usize::from(rows));
    assert_eq!(lines[1].spans[0].content.chars().count(), usize::from(cols));
    restore();
}

#[test]
fn the_block_is_flush_left_not_indented_into_the_tool_gutter() {
    let _guard = drawing(120);
    let lines = image_block_lines(&image_read("/tmp/cat.png", (700, 689)), 100);
    // The very first span of the row is already the marked block: no leading
    // unmarked indent, so the picture starts at column 0 and gets the whole
    // width rather than hanging off the cell's `⎿` gutter.
    let first = &lines[1].spans[0];
    assert_eq!(lines[1].spans.len(), 1, "one span, the block itself");
    assert!(
        first
            .style
            .underline_color
            .and_then(images::carrier_parts)
            .is_some(),
        "column 0 is reserved, not indent"
    );
    restore();
}

#[test]
fn a_narrower_terminal_reserves_a_smaller_block() {
    let _guard = drawing(120);
    let item = image_read("/tmp/cat.png", (700, 689));
    let wide = image_block_lines(&item, 100);
    let narrow = image_block_lines(&item, 40);
    assert!(
        narrow.len() < wide.len(),
        "the same picture is fewer rows on a narrow terminal: {} vs {}",
        narrow.len(),
        wide.len()
    );
    restore();
}

#[test]
fn nothing_is_reserved_when_the_row_is_off_or_the_terminal_cannot_draw() {
    let _guard = policy_lock();
    let item = image_read("/tmp/cat.png", (700, 689));
    images::set_policy(ImagePolicy {
        show: false,
        available: true,
        ..ImagePolicy::default()
    });
    assert!(image_block_lines(&item, 100).is_empty(), "row off");
    images::set_policy(ImagePolicy {
        show: true,
        available: false,
        ..ImagePolicy::default()
    });
    assert!(image_block_lines(&item, 100).is_empty(), "no terminal");
    restore();
}

#[test]
fn a_read_that_was_not_an_image_reserves_nothing() {
    let _guard = drawing(120);
    let mut tool = match image_read("/tmp/notes.txt", (700, 689)) {
        HistoryItem::Tool(tool) => tool,
        _ => unreachable!(),
    };
    tool.output = "     1 alpha\n     2 beta".to_string();
    assert!(image_block_lines(&HistoryItem::Tool(tool), 100).is_empty());
    restore();
}

#[test]
fn the_path_comes_from_the_models_verbatim_arguments() {
    let _guard = drawing(120);
    // The header's one-line summary can be truncated; the recorded arguments
    // are the file the picture is actually read from.
    let mut tool = match image_read("/tmp/real.png", (64, 64)) {
        HistoryItem::Tool(tool) => tool,
        _ => unreachable!(),
    };
    tool.args = "…truncated…".to_string();
    let item = HistoryItem::Tool(tool);
    assert_eq!(
        item_images(&item),
        vec![("/tmp/real.png".to_string(), (64, 64))]
    );
    restore();
}

#[test]
fn a_pasted_attachment_draws_under_its_own_bubble() {
    let _guard = drawing(120);
    // The paste boundary records the size; without one there is nothing to
    // reserve, which is also the right answer for a resumed session whose
    // temp file is gone.
    let message = |path: &str| {
        HistoryItem::Message(Message {
            role: Role::User,
            text: "[Image #1] look".to_string(),
            timestamp: String::new(),
            images: vec![std::path::PathBuf::from(path)],
        })
    };
    assert!(
        image_block_lines(&message("/tmp/never-seen.png"), 100).is_empty(),
        "an unregistered path draws nothing"
    );
    images::remember_size("/tmp/pasted.png", (800, 600));
    let lines = image_block_lines(&message("/tmp/pasted.png"), 100);
    assert!(!lines.is_empty(), "a registered paste draws");
    assert_eq!(marked_rows(&lines).len(), lines.len() - 1);
    restore();
}

#[test]
fn the_conversation_puts_the_picture_between_the_cell_and_the_spacer() {
    let _guard = drawing(120);
    let history = vec![image_read("/tmp/cat.png", (700, 689))];
    let lines = conversation_lines(&history, 100, &PathDisplay::VERBATIM);
    let marks: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.spans
                .first()
                .and_then(|s| s.style.underline_color)
                .and_then(images::carrier_parts)
                .is_some()
        })
        .map(|(at, _)| at)
        .collect();
    let first = *marks.first().expect("the picture is in the conversation");
    let last = *marks.last().unwrap();
    assert!(
        lines[first - 1].spans.is_empty(),
        "exactly one blank row between the cell and the picture"
    );
    assert!(!lines[first - 2].spans.is_empty(), "and the cell above it");
    assert_eq!(last, lines.len() - 2, "then the item's own trailing spacer");
    assert!(lines[last + 1].spans.is_empty());
    restore();
}

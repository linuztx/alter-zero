//! The rows a picture reserves in the conversation.
//!
//! Pure, like every other line builder here: it reserves the cells and marks
//! them, and the paint boundary ([`crate::images::store::ImageStore::stamp`])
//! is what turns them into a picture. The split is what lets the *same* rows
//! reach the terminal's real scrollback, the live region and the Ctrl+O
//! transcript through the paths they already use — a scrollback commit is a
//! `Vec<Line>` and always has been.
//!
//! A block is `rows` lines of `cols` marked spaces, one blank line above it,
//! flush at the left margin — deliberately *not* indented into the tool
//! cell's `⎿` gutter, so a picture gets the full width. See `docs/images.md`.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::{HistoryItem, Message, Role, ToolCall};
use crate::images;

/// The `read` tool's name, as the model spells it — the one tool whose output
/// can be a picture.
const READ_TOOL: &str = "read";

/// Every picture a history item shows, as `(path, pixel size)` in the order
/// they should be drawn.
///
/// Two sources, and only two: a `read` whose result was an image (the size
/// comes from the tool's own fact line, so it survives a `/resume`), and a
/// user message carrying Ctrl+V attachments (the size comes from the registry
/// the paste boundary filled). Everything else has no picture.
#[must_use]
pub fn item_images(item: &HistoryItem) -> Vec<(String, (u32, u32))> {
    match item {
        HistoryItem::Tool(tool) => tool_image(tool).into_iter().collect(),
        HistoryItem::Message(message) => message_images(message),
        _ => Vec::new(),
    }
}

/// The picture a resolved `read` call shows, if its result was one.
fn tool_image(tool: &ToolCall) -> Option<(String, (u32, u32))> {
    if !tool.name.eq_ignore_ascii_case(READ_TOOL) || tool.shell {
        return None;
    }
    let px = images::read_image_size(&tool.output)?;
    Some((read_path(tool), px))
}

/// The file a `read` call names: the model's verbatim `path` argument when it
/// parses, else the one-line summary the header shows (a pre-`arguments`
/// rollout, where the summary *is* the path).
fn read_path(tool: &ToolCall) -> String {
    tool.arguments
        .as_deref()
        .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
        .and_then(|value| {
            value
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| tool.args.clone())
}

/// The Ctrl+V attachments a user message carries, for the paths the paste
/// boundary recorded a size for.
fn message_images(message: &Message) -> Vec<(String, (u32, u32))> {
    if message.role != Role::User {
        return Vec::new();
    }
    message
        .images
        .iter()
        .filter_map(|path| {
            let path = path.to_str()?;
            Some((path.to_string(), images::known_size(path)?))
        })
        .collect()
}

/// The rows `item`'s pictures reserve below its cell — a blank spacer then
/// the block, per picture. Empty when the item shows none, when `/settings`
/// **Show images** is off, or when the terminal can't draw one, which is what
/// keeps every existing line count exactly as it was.
#[must_use]
pub fn image_block_lines(item: &HistoryItem, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (path, px) in item_images(item) {
        lines.extend(image_lines(&path, px, width));
    }
    lines
}

/// One picture's rows: the blank spacer that separates it from the cell above
/// and the reserved block itself.
#[must_use]
pub fn image_lines(path: &str, px: (u32, u32), width: u16) -> Vec<Line<'static>> {
    let Some(place) = images::place(path, px, width) else {
        return Vec::new();
    };
    let mut lines = Vec::with_capacity(usize::from(place.rows) + 1);
    lines.push(Line::default());
    let row = " ".repeat(usize::from(place.cols));
    for index in 0..place.rows {
        lines.push(Line::from(Span::styled(
            row.clone(),
            Style::default().underline_color(images::carrier(place.id, index)),
        )));
    }
    lines
}

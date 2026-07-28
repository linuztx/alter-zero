//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`live_height`], [`repin`], [`cursor_position`]),
//! build ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, BackgroundShell, BackgroundView, HistoryItem, HistorySearch, KeyOnboarding, KeyStep,
    ModelLoad, ModelPicker, ProviderChoice, QueuedTurn, ResumeControl, ResumeFilter, ResumePicker,
    ResumeSort, Role, SearchState, SlashCommand, ToastKind, TokenArrow, ToolCall, ToolStatus,
    TurnStatus, TurnSummary, command_query, matching_commands,
};
use crate::file_search::FileMatch;
use crate::highlight;
use crate::llm::ModelEntry;
use crate::markdown;
use crate::textarea::TextArea;

mod agent;
mod assistant;
mod background_view;
mod context_view;
mod conversation;
mod file_cell;
mod footer;
mod header;
mod inline;
mod layout;
mod live;
mod login_view;
mod menu;
mod message;
mod model_view;
mod resume_view;
mod status;
mod stream_render;
mod table;
mod theme;
mod tool;
mod transcript;
mod wrap;

pub use self::agent::*;
pub use self::background_view::*;
pub use self::context_view::*;
pub use self::conversation::*;
pub use self::footer::*;
pub use self::header::*;
pub use self::layout::*;
pub use self::live::*;
pub use self::login_view::*;
pub use self::menu::*;
pub use self::message::*;
pub use self::model_view::*;
pub use self::resume_view::*;
pub use self::status::*;
pub use self::stream_render::*;
pub use self::theme::*;
pub use self::tool::*;
pub use self::transcript::*;
pub use self::wrap::*;

#[cfg(test)]
mod tests;

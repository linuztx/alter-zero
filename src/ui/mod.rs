//! Pure rendering helpers.
//!
//! These functions never touch the terminal directly — they either compute
//! plain data ([`wrap_text`], [`live_height`], [`repin`], [`cursor_position`]),
//! build ratatui [`Line`]s ([`message_lines`]), or render into a [`Buffer`]
//! ([`render_live`]). That keeps them unit-testable with a plain `Buffer` or
//! ratatui's `TestBackend`, with no real terminal involved.
//!
//! One module per area — see `docs/module-layout.md` for the map. Every styling
//! and geometry constant lives in `theme`, and all width math goes through
//! `cols` in `wrap`.

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
    App, BackgroundShell, BackgroundView, DeviceLogin, DeviceStatus, HistoryItem, HistorySearch,
    KeyKind, KeyOnboarding, KeyStep, ModelLoad, ModelPicker, PathDisplay, QueuedTurn,
    ResumeControl, ResumeFilter, ResumePicker, ResumeSort, Role, SearchState, SigninKind,
    SlashCommand, ToastKind, TokenArrow, ToolCall, ToolStatus, TurnStatus, TurnSummary,
    command_query, matching_commands,
};
use crate::file_search::FileMatch;
use crate::highlight;
use crate::llm::ModelEntry;
use crate::markdown;
use crate::textarea::TextArea;

mod agent;
mod ask_view;
mod assistant;
mod background_view;
mod classifier_view;
mod context_view;
mod conversation;
mod diff_view;
mod donate_view;
mod file_cell;
mod footer;
mod header;
mod hooks_view;
mod image;
mod inline;
mod inline_diff;
mod layout;
mod live;
mod login_view;
mod mascot_view;
mod mcp_view;
mod menu;
mod message;
mod model_view;
mod palette;
mod permission_view;
mod reasoning;
mod resume_view;
mod settings_view;
mod skills_view;
mod spinner_view;
mod status;
mod stream_render;
mod table;
mod tasks;
mod theme;
mod theme_view;
mod tool;
mod transcript;
mod trust_view;
mod view_flow;
mod wrap;

pub use self::agent::{
    agent_group_lines, agent_hint_line, agent_list_lines, agent_list_rows, agent_notice_lines,
    agent_view_status, live_agent_group_lines,
};
pub use self::ask_view::{ask_height, ask_lines, render_ask};
pub use self::background_view::{background_view_lines, render_background_view};
pub use self::classifier_view::classifier_lines;
pub use self::context_view::{ContextCache, context_lines, render_context_view};
pub use self::conversation::{
    banner_tail, committed_history, conversation_lines, repaint_lines, repaint_tail,
};
pub use self::diff_view::{diff_body_rows, render_diff_view};
pub use self::donate_view::{donate_picker_height, donate_view_lines, render_donate_picker};
pub use self::footer::{
    backtrack_hint_line, display_cwd, footer_line, footer_rows, queued_lines, queued_rows,
    search_line, shell_mode_line, toast_line, toast_rows,
};
pub use self::header::{
    header_lines, notice_card_lines, startup_notice_lines, startup_paragraph_lines,
    telemetry_notice_lines, update_notice_lines,
};
pub use self::hooks_view::{hooks_menu_height, hooks_view_lines, render_hooks_menu};
pub use self::image::{image_block_lines, image_lines, item_images};
pub use self::layout::{
    Repin, background_view_height, cursor_position, cursor_visible, fitted_preview_rows,
    key_onboarding_height, live_height, modal_needs_rebuild, model_picker_height,
    overlay_cursor_seat, permission_height, preview_budget, preview_rows, region_is_modal, repin,
    restore_cursor_row, stream_preview_max_rows, strip_has_status,
};
pub use self::live::{render_live, render_live_with_preview};
pub use self::login_view::render_key_onboarding;
pub use self::mascot_view::{mascot_picker_height, render_mascot_picker};
pub use self::mcp_view::{mcp_menu_height, mcp_view_lines, render_mcp_menu};
pub use self::menu::{
    band_rows, centered_window, command_menu_lines, file_menu_lines, file_menu_rows, menu_rows,
    menu_window, shortcuts_lines, shortcuts_rows, skill_menu_lines, skill_menu_rows,
};
pub use self::message::{compaction_lines, message_lines};
pub use self::model_view::render_model_picker;
pub use self::palette::{activate_theme, active_theme, with_theme};
pub use self::permission_view::{permission_lines, permission_remember_label, render_permission};
pub use self::reasoning::reasoning_lines;
pub use self::resume_view::render_resume_picker;
pub use self::settings_view::{render_settings, settings_height};
pub use self::skills_view::{render_skills_menu, skills_menu_height};
pub use self::spinner_view::{render_spinner_picker, spinner_picker_height};
pub use self::status::{
    background_notice_lines, format_elapsed, format_timeout, format_token_count, status_line,
    status_line_with_verb, styled_status_line, summary_lines,
};
pub use self::stream_render::StreamRender;
pub use self::tasks::{checklist_lines, idle_task_lines, task_rows};
pub use self::theme::{COMPACTED_NOTICE, LIVE_MIN_HEIGHT};
pub use self::theme_view::{render_theme_picker, theme_picker_height};
pub use self::tool::{held_run_len, tool_commit_lines, tool_lines};
pub use self::transcript::{
    TranscriptCache, agent_transcript_lines, backtrack_scroll, backtrack_scroll_for,
    render_tool_view, tool_view_body_rows, tool_view_max_scroll, tool_view_max_scroll_for,
    transcript_lines, transcript_selection,
};
pub use self::trust_view::{render_trust_menu, trust_menu_height, trust_view_lines};
pub use self::view_flow::{ViewFlow, view_body_skip, view_flow, view_flow_signature};
pub use self::wrap::wrap_text;

#[cfg(test)]
mod tests;

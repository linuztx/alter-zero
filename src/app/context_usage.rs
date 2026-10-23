//! The `/context` page: what fills the context window, by category, and
//! what the provider said about the requests that carried it. See
//! `docs/context-usage.md`.
//!
//! The `/donate` page's sibling — a read-only composer-replacing page with
//! **no text entry**, the hardware cursor hidden while its seat tracks the
//! highlighted `❯`, every key owned while open — over a breakdown counted
//! by the pure [`crate::context_usage`] core. The page is **live**: the
//! boundary refreshes it at the loop bottom while it is open
//! ([`App::refresh_context_usage`]), and the count is redone only when what
//! it was counted from has changed.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use super::compact::auto_compact_threshold;
use super::types::IMAGE_INPUT_TOKENS;
use super::*;
use crate::context_usage::{Breakdown, Category, CountMemo, Sources, ToolDefinition, UsageLedger};
use crate::stream::TokenUsage;

/// One row of the page's legend: a category the window holds, or one of the
/// two rows for the room that is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextUsageRow {
    /// A category of what the window holds.
    Slice(Category),
    /// The room left before the auto-compact threshold — or, with
    /// auto-compact off, before the window is full.
    Free,
    /// The headroom past the auto-compact threshold the session never fills.
    AutoCompact,
}

/// The open `/context` page's state (`None` on [`App`] when closed).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextUsagePage {
    /// The highlighted legend row — a row *kind* rather than an index, so a
    /// category appearing above it (an MCP server connecting) leaves the
    /// highlight where it was. It follows the largest category until the
    /// user moves it.
    pub selected: ContextUsageRow,
    /// The window, counted — redone by
    /// [`refresh_context_usage`](App::refresh_context_usage) whenever what it
    /// was counted from changes.
    pub breakdown: Breakdown,
    /// What [`breakdown`](Self::breakdown) was counted from.
    signature: u64,
    /// Has the user moved the highlight? Until then every recount puts it
    /// back on the largest category: the page is counted as it opens, and
    /// the tool set the boundary hands over right after can change which
    /// category that is.
    chosen: bool,
    /// Every text's count from the last pass, so a re-count tokenizes only
    /// what is new — a long conversation's whole window through the
    /// tokenizer again stalled a frame on each commit of a busy turn.
    memo: CountMemo,
}

impl ContextUsagePage {
    /// The row the page opens on, or falls back to: the largest category —
    /// the answer to "what is using my context?" — or the conversation when
    /// the window holds nothing at all.
    fn largest_row(&self) -> ContextUsageRow {
        ContextUsageRow::Slice(self.breakdown.largest().unwrap_or(Category::Messages))
    }
}

impl App {
    /// Open the `/context` page over the conversation on screen, counted now
    /// and highlighting its largest category. Abandons any `?` band / palette
    /// / picker band (they share the composer the page takes over) — every
    /// picker's open rule.
    pub fn open_context_usage(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        let mut memo = CountMemo::default();
        let mut page = ContextUsagePage {
            selected: ContextUsageRow::Slice(Category::Messages),
            breakdown: self.count_context_window(&mut memo),
            signature: self.context_usage_signature(),
            chosen: false,
            memo,
        };
        page.selected = page.largest_row();
        self.context_usage = Some(page);
    }

    /// Dismiss the page (Esc, Ctrl+C): the composer returns. No view change
    /// — it was never an overlay.
    pub fn close_context_usage(&mut self) {
        self.context_usage = None;
    }

    /// Re-count the open page when what it was counted from has changed —
    /// the conversation, the system prompt, the reminder's inputs, the tool
    /// set, or the conversation on screen — and keep its highlight on a row
    /// that still exists: the largest category until the user has moved it,
    /// the user's own row after. A few hashes and compares when nothing
    /// changed; the boundary calls it at every loop bottom while the page is
    /// open.
    pub fn refresh_context_usage(&mut self) {
        let Some(counted) = self.context_usage.as_ref().map(|page| page.signature) else {
            return;
        };
        let signature = self.context_usage_signature();
        if signature != counted {
            let mut memo = self
                .context_usage
                .as_mut()
                .map(|page| std::mem::take(&mut page.memo))
                .unwrap_or_default();
            let breakdown = self.count_context_window(&mut memo);
            if let Some(page) = self.context_usage.as_mut() {
                page.breakdown = breakdown;
                page.signature = signature;
                page.memo = memo;
            }
        }
        let rows = self.context_usage_rows();
        if let Some(page) = self.context_usage.as_mut()
            && (!page.chosen || !rows.contains(&page.selected))
        {
            page.selected = page.largest_row();
        }
    }

    /// The page's legend rows, top to bottom: every category the window
    /// holds, in request order, then — when the window is known — the free
    /// space and, while the conversation on screen compacts itself, the
    /// headroom it keeps. Empty while the page is closed.
    #[must_use]
    pub fn context_usage_rows(&self) -> Vec<ContextUsageRow> {
        let Some(page) = &self.context_usage else {
            return Vec::new();
        };
        let mut rows: Vec<ContextUsageRow> = page
            .breakdown
            .slices
            .iter()
            .map(|slice| ContextUsageRow::Slice(slice.category))
            .collect();
        if self.context_usage_window().is_some() {
            rows.push(ContextUsageRow::Free);
            if self.context_usage_threshold().is_some() {
                rows.push(ContextUsageRow::AutoCompact);
            }
        }
        rows
    }

    /// The window of the conversation on screen: the viewed subagent's
    /// model's inside its session view, else the session's — the footer
    /// gauge's denominator (`docs/agent-context-gauge.md`).
    #[must_use]
    pub fn context_usage_window(&self) -> Option<u64> {
        match self.viewed_agent() {
            Some(_) => self.agent_context_window,
            None => self.context_window,
        }
    }

    /// The token count at which the conversation on screen compacts itself,
    /// when it does: the main session's, while the `/settings` **Auto
    /// compact** knob is on and its window is known. A subagent never
    /// compacts (`docs/compact.md`).
    #[must_use]
    pub fn context_usage_threshold(&self) -> Option<u64> {
        if self.viewed_agent().is_some() || !self.settings.auto_compact {
            return None;
        }
        self.context_window.map(auto_compact_threshold)
    }

    /// Inject the tools the main session's next request offers (from
    /// `ReplySource::tool_definitions`, pulled at the loop bottom while the
    /// page is open). The same set in a new handle is no change, so the
    /// boundary can pull freely; a different one moves the page's signature.
    pub fn set_tool_definitions(&mut self, tools: Arc<[ToolDefinition]>) {
        if !Arc::ptr_eq(&self.tool_definitions, &tools) && *self.tool_definitions != *tools {
            self.tool_definitions = tools;
            self.tool_definitions_revision += 1;
        }
    }

    /// Inject the tools the **viewed** subagent's type is offered (from
    /// `ReplySource::agent_tool_definitions`) — its session view's page.
    pub fn set_agent_tool_definitions(&mut self, tools: Arc<[ToolDefinition]>) {
        if !Arc::ptr_eq(&self.agent_tool_definitions, &tools)
            && *self.agent_tool_definitions != *tools
        {
            self.agent_tool_definitions = tools;
            self.tool_definitions_revision += 1;
        }
    }

    /// The main session's newest usage frame — the page's **Last request**
    /// (a subagent's own is [`crate::agents::AgentRun::last_usage`]).
    #[must_use]
    pub const fn last_usage(&self) -> Option<TokenUsage> {
        self.last_usage
    }

    /// Every usage frame since launch — the main session's and every
    /// subagent's — the page's **Since launch** row.
    #[must_use]
    pub const fn usage_since_launch(&self) -> UsageLedger {
        self.usage_since_launch
    }

    /// Count the window of the conversation on screen
    /// ([`crate::context_usage::breakdown`]) — what the next request carries:
    /// the system prompt, the tools, the `<system-reminder>` the derived
    /// context leads with, and the derived conversation. Inside an agent
    /// session view, that agent's own: its prompt, its type's tools, its
    /// briefing and its transcript. Each text's count comes from `memo` when
    /// the last pass counted it, so only what is new is tokenized.
    fn count_context_window(&self, memo: &mut CountMemo) -> Breakdown {
        let memo = RefCell::new(memo);
        let count = |text: &str| memo.borrow_mut().count(text, &count_tokens);
        let breakdown = match self.viewed_agent() {
            Some(run) => {
                let (messages, origins) =
                    crate::context::context_messages_with_origins(&run.history);
                crate::context_usage::breakdown(
                    &Sources {
                        system_prompt: self.agent_system_prompt.as_deref(),
                        tools: &self.agent_tool_definitions,
                        reminder: self.agent_system_reminder.as_deref(),
                        instructions: None,
                        messages: &messages,
                        origins: &origins,
                        image_tokens: IMAGE_INPUT_TOKENS,
                    },
                    &count,
                )
            }
            None => {
                // The one block `context::context_messages_full` composes
                // from these two, in its order.
                let reminder = crate::reminder::reminder_message(&[
                    self.user_instructions.as_deref().unwrap_or_default(),
                    self.listings.as_deref().unwrap_or_default(),
                ]);
                let (messages, origins) =
                    crate::context::context_messages_with_origins(&self.history);
                crate::context_usage::breakdown(
                    &Sources {
                        system_prompt: self.system_prompt.as_deref(),
                        tools: &self.tool_definitions,
                        reminder: Some(&reminder),
                        instructions: self.user_instructions.as_deref(),
                        messages: &messages,
                        origins: &origins,
                        image_tokens: IMAGE_INPUT_TOKENS,
                    },
                    &count,
                )
            }
        };
        memo.borrow_mut().finish_pass();
        breakdown
    }

    /// What [`count_context_window`](Self::count_context_window) reads,
    /// hashed: the conversation's identity — `(generation, length)` for the
    /// main history (committed items are immutable and otherwise only
    /// appended), the agent's id and transcript length inside its view — the
    /// texts it counts around it, and the tool sets' revision.
    fn context_usage_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.tool_definitions_revision.hash(&mut hasher);
        match self.viewed_agent() {
            Some(run) => {
                run.id.hash(&mut hasher);
                run.history.len().hash(&mut hasher);
                self.agent_system_prompt.hash(&mut hasher);
                self.agent_system_reminder.hash(&mut hasher);
            }
            None => {
                self.history_generation.hash(&mut hasher);
                self.history.len().hash(&mut hasher);
                self.system_prompt.hash(&mut hasher);
                self.user_instructions.hash(&mut hasher);
                self.listings.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Keys while the `/context` page is open — the `/donate` grammar, minus
    /// anything to copy. Owns **every** key (routed at the top of
    /// [`on_key`]): ↑/↓ move the highlight wrapping at the ends, Home/End
    /// jump, Esc and Ctrl+C close. Ctrl+D and Ctrl+O pass through to the two
    /// read-only overlays — they show what the page measures, and the page
    /// waits behind them (the permission prompt's rule).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_context_usage(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_context_usage();
            return Action::CloseContextUsage;
        }
        if let Some(action) = self.on_key_overlay_toggle(key) {
            return action;
        }
        let rows = self.context_usage_rows();
        let Some(page) = self.context_usage.as_mut() else {
            return Action::None;
        };
        let current = rows
            .iter()
            .position(|row| *row == page.selected)
            .unwrap_or(0);
        let step = |delta| rows.get(wrap_step(current, rows.len(), delta)).copied();
        let moved = match key.code {
            KeyCode::Up => step(-1),
            KeyCode::Down => step(1),
            KeyCode::Home => rows.first().copied(),
            KeyCode::End => rows.last().copied(),
            KeyCode::Esc => {
                self.close_context_usage();
                return Action::CloseContextUsage;
            }
            _ => None,
        };
        if let Some(row) = moved {
            page.selected = row;
            page.chosen = true;
        }
        Action::None
    }
}

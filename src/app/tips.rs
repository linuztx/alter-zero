//! The usage tips under the status line (`docs/tips.md`): the catalog, the
//! walk through it, and the file that carries the walk across launches.
//!
//! Claude Code shows one tip under its spinner per turn, picked from a
//! catalog of things the user may not know yet, each tip resting for a few
//! sessions once shown so a launch never opens on the one before. This is
//! that, in the shape the status verbs already have (`docs/status-indicator.md`):
//! a **walk** through the catalog whose cursor is one past the last tip a
//! line showed — the next turn opens on the next tip, a relaunch opens after
//! the last one any session showed (`tips.json`), and a long turn moves on
//! to the next tip every [`TIP_ROTATION`]. Nothing shows until a turn is
//! [`TIP_DELAY`] old: a quick answer never shows one.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::*;

/// How far into a turn the first tip shows. A few seconds: a quick answer
/// keeps its strip to the status line (a tip under a spinner about to
/// vanish is a flash, not a hint), while any turn long enough to wait on
/// gets one. Claude Code's own tip lands about as soon as its relevance
/// checks resolve; the delay here is what makes "no tip, then a tip" the
/// shape of every turn rather than of the first.
pub const TIP_DELAY: Duration = Duration::from_secs(5);

/// How long a tip holds before the walk moves on to the next — for the long
/// agentic turn, where one sentence under the spinner for twenty minutes
/// says nothing new. Three minutes: read once and forgotten, not a row that
/// keeps changing under the reply streaming above it.
pub const TIP_ROTATION: Duration = Duration::from_secs(180);

/// The file's name under the config home: `{config_home}/tips.json` — the
/// walk's position, **per user**, like `telemetry.json` and `update.json`.
/// A tip is about the app, not the directory.
pub const TIPS_FILE_NAME: &str = "tips.json";

/// What a tip needs the session to have before it is worth showing —
/// Claude Code's `isRelevant`, reduced to the facts the pure core already
/// holds. A tip whose feature is absent is stepped over by the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipFeature {
    /// Every session.
    Always,
    /// The tool-permission gate is on (`App::permission_mode` is `Some`) —
    /// a Shift+Tab tip is dead advice under `ALTER_ZERO_PERMISSIONS=0`.
    Permissions,
    /// The active model reasons (`App::thinking` is `Some`) — Ctrl+T cycles
    /// nothing on a model that has no thinking mode.
    Thinking,
    /// At least one skill is usable in this session (the `$` picker's
    /// snapshot, `App::set_skills`).
    Skills,
}

/// One usage tip: a stable `id` (what `tips.json` records — the catalog may
/// be reordered or reworded without losing the walk's place), the sentence
/// the row shows after its `Tip: ` prefix, and the feature it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tip {
    /// The stable slug the persisted walk names.
    pub id: &'static str,
    /// The tip itself — one sentence, no closing period, at most seventy
    /// columns so it fits beside the `⎿  Tip: ` gutter on an 80-column
    /// terminal (pinned by a test).
    pub text: &'static str,
    /// What the session must have for the tip to apply.
    pub needs: TipFeature,
}

impl Tip {
    /// One [`TIPS`] row — a constructor so the table reads one tip per line.
    const fn new(id: &'static str, text: &'static str, needs: TipFeature) -> Self {
        Self { id, text, needs }
    }
}

/// The catalog, walked in order. Every entry names a feature of this app in
/// Claude Code's own register — what to press, what it does — and is true in
/// every session unless its [`TipFeature`] says otherwise. The order is the
/// order a new user meets them: the composer's keys first, then the views,
/// then the commands.
pub const TIPS: &[Tip] = &[
    Tip::new(
        "steer",
        "Hit Enter to send a message into the running turn, or Tab to queue it",
        TipFeature::Always,
    ),
    Tip::new(
        "backtrack",
        "Double-tap Esc to rewind the conversation to an earlier message",
        TipFeature::Always,
    ),
    Tip::new(
        "transcript",
        "Press Ctrl+O to open the full transcript, every tool output included",
        TipFeature::Always,
    ),
    Tip::new(
        "context",
        "Press Ctrl+D to see the exact context window the model receives",
        TipFeature::Always,
    ),
    Tip::new(
        "permission-mode",
        "Hit Shift+Tab to cycle the permission mode: manual, edit, auto, master",
        TipFeature::Permissions,
    ),
    Tip::new(
        "at-files",
        "Type @ to fuzzy-search a file and mention it in your message",
        TipFeature::Always,
    ),
    Tip::new(
        "skill-mentions",
        "Type $ to mention a skill by name and have the model load it",
        TipFeature::Skills,
    ),
    Tip::new(
        "history-search",
        "Press Ctrl+R to search everything you have typed, across sessions",
        TipFeature::Always,
    ),
    Tip::new(
        "shell-mode",
        "Start a message with ! to run a shell command right here",
        TipFeature::Always,
    ),
    Tip::new(
        "image-paste",
        "Press Ctrl+V to paste an image from your clipboard",
        TipFeature::Always,
    ),
    Tip::new(
        "newline",
        "Press Shift+Enter or Ctrl+J for a new line without sending",
        TipFeature::Always,
    ),
    Tip::new(
        "thinking-mode",
        "Press Ctrl+T to cycle the model's thinking effort",
        TipFeature::Thinking,
    ),
    Tip::new(
        "background",
        "Press Ctrl+B to move a long-running command to the background",
        TipFeature::Always,
    ),
    Tip::new(
        "manager",
        "Press ↓ in an empty composer to manage background shells and agents",
        TipFeature::Always,
    ),
    Tip::new(
        "edit-queue",
        "Press Alt+↑ to pull a queued message back into the composer",
        TipFeature::Always,
    ),
    Tip::new(
        "shortcuts",
        "Type ? in an empty composer to see every keyboard shortcut",
        TipFeature::Always,
    ),
    Tip::new(
        "compact",
        "Use /compact to summarize the conversation and free up context",
        TipFeature::Always,
    ),
    Tip::new(
        "resume",
        "Use /resume, or alter-zero --continue, to pick up a past conversation",
        TipFeature::Always,
    ),
    Tip::new(
        "copy",
        "Use /copy to copy the last reply to your clipboard",
        TipFeature::Always,
    ),
    Tip::new(
        "export",
        "Use /export to copy or save the whole conversation",
        TipFeature::Always,
    ),
    Tip::new(
        "init",
        "Run /init to write an AGENTS.md guide the model reads every turn",
        TipFeature::Always,
    ),
    Tip::new(
        "tasks",
        "Ask for a task list on big jobs: it ticks off live under the spinner",
        TipFeature::Always,
    ),
    Tip::new(
        "agents",
        "Ask for a subagent to explore or work on a side task in parallel",
        TipFeature::Always,
    ),
    Tip::new(
        "model",
        "Use /model to switch models, or /login to sign in to another provider",
        TipFeature::Always,
    ),
    Tip::new(
        "settings",
        "Use /settings to change the session's knobs without restarting",
        TipFeature::Always,
    ),
    Tip::new(
        "checkpoints",
        "Turn on Checkpoints in /settings so a rewind restores your files too",
        TipFeature::Always,
    ),
    Tip::new(
        "theme",
        "Use /theme to change the color theme",
        TipFeature::Always,
    ),
    Tip::new(
        "spinner",
        "Use /spinner to change the status line's spinner style",
        TipFeature::Always,
    ),
    Tip::new(
        "mascot",
        "Use /mascot to change the banner's mascot",
        TipFeature::Always,
    ),
    Tip::new(
        "skills-menu",
        "Use /skills to browse the skills the model can load, and turn them off",
        TipFeature::Skills,
    ),
    Tip::new(
        "mcp",
        "Use /mcp to add MCP servers, or alter-zero mcp add from your shell",
        TipFeature::Always,
    ),
    Tip::new(
        "hooks",
        "Add commands to ~/.alter-zero/hooks.json to run around every tool call",
        TipFeature::Always,
    ),
];

/// The tip at `index` in the walk through [`TIPS`], wrapping.
#[must_use]
pub fn tip_at(index: usize) -> &'static Tip {
    &TIPS[index % TIPS.len()]
}

/// The catalog index of the tip with `id`, if this build knows it.
fn index_of(id: &str) -> Option<usize> {
    TIPS.iter().position(|tip| tip.id == id)
}

/// The catalog index a turn that opened on `start` shows `elapsed` in, over
/// the tips `applies` accepts: `None` under [`TIP_DELAY`] and when no tip
/// applies; else the first applicable tip at or after `start` (wrapping)
/// for the first [`TIP_ROTATION`], the next applicable one for the next,
/// and so on around the applicable set. Pure — the boundary's clock comes
/// in as `elapsed`, the session's facts as `applies`.
fn walk(start: usize, elapsed: Duration, applies: impl Fn(&Tip) -> bool) -> Option<usize> {
    let since = elapsed.checked_sub(TIP_DELAY)?;
    let len = TIPS.len();
    let order: Vec<usize> = (0..len)
        .map(|offset| (start + offset) % len)
        .filter(|&index| applies(&TIPS[index]))
        .collect();
    if order.is_empty() {
        return None;
    }
    let steps = since.as_millis() / TIP_ROTATION.as_millis() % order.len() as u128;
    // `steps < order.len()`, so the narrowing is exact.
    Some(order[steps as usize])
}

/// What `tips.json` holds: the id of the last tip any session showed, so
/// the next launch opens the walk after it. Every field optional, so an
/// old, empty or future file still loads.
///
/// ```json
/// { "last": "compact" }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TipsFile {
    /// The last tip shown, by [`Tip::id`]; `None` when no session has shown
    /// one yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

impl TipsFile {
    /// Parse a `tips.json` body, best-effort: malformed, empty or wrongly
    /// typed JSON reads as nothing known rather than an error, so a corrupt
    /// file never blocks startup (`UpdateFile::parse`'s posture).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back to `tips.json`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

impl App {
    /// Seed the walk from `tips.json` (the boundary's startup injection, the
    /// `set_clock` pattern): the next tip shown is the one after `last`, so
    /// a relaunch never opens on the tip the previous session ended on. An
    /// id this build does not know — or none — starts at the catalog's
    /// first tip.
    pub fn seed_tips(&mut self, last: Option<&str>) {
        self.tip_cursor = last.and_then(index_of).map_or(0, |index| index + 1);
    }

    /// The tip the strip shows right now — `Some` while a turn's status line
    /// is up, [`TIP_DELAY`] has passed, the `/settings` **Tips** row is on,
    /// and some tip applies; walked by [`set_status_times`](App::set_status_times).
    /// `None` between turns, under a `!` shell turn (no status line), and
    /// with tips off, however far the turn's walk got before the switch.
    #[must_use]
    pub fn tip(&self) -> Option<&'static Tip> {
        if !self.settings.tips {
            return None;
        }
        self.status
            .as_ref()
            .and_then(|status| status.tip)
            .map(tip_at)
    }

    /// Whether `tip` applies to this session — its [`TipFeature`] against
    /// the session's own facts.
    #[must_use]
    pub fn tip_applies(&self, tip: &Tip) -> bool {
        match tip.needs {
            TipFeature::Always => true,
            TipFeature::Permissions => self.permission_mode.is_some(),
            TipFeature::Thinking => self.thinking.is_some(),
            TipFeature::Skills => !self.skills.is_empty(),
        }
    }

    /// The id of a tip that came up since the last take — the boundary
    /// records it in `tips.json` so the next launch opens after it. Each
    /// newly shown tip is reported exactly once; a frame that keeps the same
    /// tip reports nothing.
    pub fn take_tip_record(&mut self) -> Option<&'static str> {
        self.tip_record.take()
    }

    /// Move the turn's tip walk to where `elapsed` puts it — the
    /// [`set_status_times`](App::set_status_times) step for tips, run right
    /// after the verb rotation. A turn whose status carries no walk (a `!`
    /// shell), or a session with tips off, moves nothing: the cursor stays
    /// where it was, so turning tips back on resumes there.
    pub(super) fn walk_tip(&mut self, elapsed: Duration) {
        if !self.settings.tips {
            return;
        }
        let Some(start) = self.status.as_ref().and_then(|status| status.tips_from) else {
            return;
        };
        let next = walk(start, elapsed, |tip| self.tip_applies(tip));
        let Some(status) = self.status.as_mut() else {
            return;
        };
        if next == status.tip {
            return;
        }
        status.tip = next;
        if let Some(index) = next {
            // One past the tip shown, so the next turn opens on the next
            // tip — and the boundary learns which one to remember.
            self.tip_cursor = index + 1;
            self.tip_record = Some(tip_at(index).id);
        }
    }
}

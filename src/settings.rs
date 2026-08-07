//! The `/settings` menu's pure model: which knobs the session exposes, what
//! values each cycles through, and the `settings.json` format they persist in.
//!
//! Everything here is data — no environment reads, no filesystem. The boundary
//! (`tui::config`) reads the file and the `ALTER_ZERO_*` overrides and hands
//! the resulting [`SessionSettings`] to `App`; the picker
//! (`crate::app::SettingsPicker`) renders it and cycles it. See
//! `docs/settings.md`.

use serde::{Deserialize, Serialize};

use crate::permission::PermissionMode;

/// The retry counts **Error retry** cycles through — 0 (never retry) up to a
/// stubborn 10, with the historical [`crate::llm::retry::MAX_RETRIES`] default
/// in the middle.
pub const RETRY_CHOICES: &[u32] = &[0, 1, 2, 3, 5, 10];

/// The sampling temperatures **Temperature** cycles through. `None` is
/// `default` — send no `temperature` at all and leave it to the provider.
pub const TEMPERATURE_CHOICES: &[Option<f32>] =
    &[None, Some(0.0), Some(0.3), Some(0.5), Some(0.7), Some(1.0)];

/// The label a `default` (provider-chosen) temperature shows.
pub const TEMPERATURE_DEFAULT_LABEL: &str = "default";

/// The suffix a row wears when the boundary says the feature can't run at all
/// in this session (see [`SettingAvailability`]).
pub const UNAVAILABLE_SUFFIX: &str = " (unavailable)";

/// What the **Permission mode** row reads when the session has no gate at all
/// (`ALTER_ZERO_PERMISSIONS=0`) — the mode is not `manual`, it is absent.
pub const PERMISSIONS_DISABLED_LABEL: &str = "disabled";

/// One row of the `/settings` menu. The order here is the order they list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    /// Hide the model's streamed chain-of-thought (`docs/thinking-stream.md`).
    HideThinking,
    /// How many times a failed request is retried (`llm::retry`).
    ErrorRetry,
    /// Offer `bash`/`read`/`write`/`edit`/`agent` to the model (`docs/tools.md`).
    Tools,
    /// The session's permission posture — Ctrl+A's cycle (`docs/permissions.md`).
    PermissionMode,
    /// Per-turn working-directory snapshots (`docs/checkpoint.md`).
    Checkpoints,
    /// Auto-run `/compact` past the context threshold (`docs/compact.md`).
    AutoCompact,
    /// Re-read the project's `AGENTS.md` each turn (`docs/project-doc.md`).
    ProjectDocs,
    /// The sampling temperature every request carries.
    Temperature,
}

impl SettingKey {
    /// Every setting, in menu order.
    pub const ALL: &'static [Self] = &[
        Self::HideThinking,
        Self::ErrorRetry,
        Self::Tools,
        Self::PermissionMode,
        Self::Checkpoints,
        Self::AutoCompact,
        Self::ProjectDocs,
        Self::Temperature,
    ];

    /// The name shown in the menu's left column.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::HideThinking => "Hide thinking",
            Self::ErrorRetry => "Error retry",
            Self::Tools => "Tools",
            Self::PermissionMode => "Permission mode",
            Self::Checkpoints => "Checkpoints",
            Self::AutoCompact => "Auto compact",
            Self::ProjectDocs => "Project docs",
            Self::Temperature => "Temperature",
        }
    }

    /// The one-line explanation shown under the list for the highlighted row.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::HideThinking => {
                "Hide the model's chain-of-thought instead of streaming it above the composer"
            }
            Self::ErrorRetry => {
                "How many times a failed request is retried before the error is shown"
            }
            Self::Tools => "Offer the bash, read, write, edit and agent tools to the model",
            Self::PermissionMode => {
                "Which tool calls ask before running — the same posture ctrl+a cycles"
            }
            Self::Checkpoints => {
                "Snapshot the working directory each turn so a rewind restores the code"
            }
            Self::AutoCompact => {
                "Summarize the conversation on its own once the context window fills up"
            }
            Self::ProjectDocs => "Load the project's AGENTS.md instructions into every request",
            Self::Temperature => "The sampling temperature sent with every request",
        }
    }
}

/// Whether a feature can run in this session **at all** — the boundary's
/// verdict, injected at bootstrap (the `set_clock` pattern). A knob the host
/// can't honour (no `git` for checkpoints, no config home) shows `false
/// (unavailable)`, refuses to cycle, and is never persisted, so a session that
/// happened to run somewhere unsuitable doesn't teach the file a lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingAvailability {
    /// Whether the checkpoint store is capable (root present, `git` on PATH,
    /// cwd project-scoped — `checkpoint::CheckpointStore::is_enabled` at
    /// construction).
    pub checkpoints: bool,
}

impl Default for SettingAvailability {
    /// Everything available — the unit-test and pre-bootstrap default.
    fn default() -> Self {
        Self { checkpoints: true }
    }
}

/// The **Permission mode** row's availability isn't a host fact but a live
/// one — `App::permission_mode()` is `None` exactly while the gate is off — so
/// it rides in beside the blob rather than in [`SettingAvailability`]. That is
/// also why [`SessionSettings::value_text`] and
/// [`SessionSettings::is_available`] both take it: the posture deliberately
/// lives on `App`, and this module never keeps a second copy to drift from it.
type Mode = Option<PermissionMode>;

/// The live values behind the `/settings` menu.
///
/// Serialized to `~/.alter-zero/settings.json` with every field optional, so an
/// old, partial, or future file still loads and a value left at its default
/// stays off the wire. [`SettingAvailability`] is **not** persisted — it is a
/// fact about the host, re-derived every run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    /// Hide the streamed chain-of-thought (default `false` — it shows).
    #[serde(skip_serializing_if = "is_false")]
    pub hide_thinking: bool,
    /// Retries per failed request (default 3).
    #[serde(skip_serializing_if = "is_default_retry")]
    pub error_retry: u32,
    /// Offer tools to the model (default `true`).
    #[serde(skip_serializing_if = "is_true")]
    pub tools: bool,
    /// Per-turn checkpoints (default `true`).
    #[serde(skip_serializing_if = "is_true")]
    pub checkpoints: bool,
    /// Auto-compaction past the context threshold (default `true`).
    #[serde(skip_serializing_if = "is_true")]
    pub auto_compact: bool,
    /// Load `AGENTS.md` into the context (default `true`).
    #[serde(skip_serializing_if = "is_true")]
    pub project_docs: bool,
    /// The sampling temperature, or `None` for the provider's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// What this host can actually run — never persisted, never cycled.
    #[serde(skip)]
    pub availability: SettingAvailability,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            hide_thinking: false,
            error_retry: crate::llm::retry::MAX_RETRIES,
            tools: true,
            checkpoints: true,
            auto_compact: true,
            project_docs: true,
            temperature: None,
            availability: SettingAvailability::default(),
        }
    }
}

impl SessionSettings {
    /// Whether the model's thinking is **shown** — the inverse of the row, and
    /// the phrasing every consumer wants (`docs/thinking-stream.md`).
    #[must_use]
    pub const fn show_thinking(&self) -> bool {
        !self.hide_thinking
    }

    /// Whether per-turn checkpoints actually snapshot: the knob **and** the
    /// host's verdict. The one place the two are combined.
    #[must_use]
    pub const fn checkpoints_active(&self) -> bool {
        self.checkpoints && self.availability.checkpoints
    }

    /// Whether `key` can be cycled at all in this session: the host's verdict
    /// ([`SettingAvailability`]) plus, for the permission row, whether there is
    /// a gate to cycle.
    #[must_use]
    pub const fn is_available(&self, key: SettingKey, mode: Mode) -> bool {
        match key {
            SettingKey::Checkpoints => self.availability.checkpoints,
            SettingKey::PermissionMode => mode.is_some(),
            _ => true,
        }
    }

    /// The value column's text for `key` — the **effective** value, not
    /// necessarily the stored preference: a session that can't run checkpoints
    /// reads `false` however the file has it. An unavailable row wears
    /// [`UNAVAILABLE_SUFFIX`] so the menu never advertises a toggle that does
    /// nothing.
    #[must_use]
    pub fn value_text(&self, key: SettingKey, mode: Mode) -> String {
        let text = match key {
            SettingKey::HideThinking => bool_text(self.hide_thinking),
            SettingKey::ErrorRetry => self.error_retry.to_string(),
            SettingKey::Tools => bool_text(self.tools),
            SettingKey::PermissionMode => mode.map_or_else(
                || PERMISSIONS_DISABLED_LABEL.to_string(),
                |m| m.label().to_string(),
            ),
            SettingKey::Checkpoints => bool_text(self.checkpoints_active()),
            SettingKey::AutoCompact => bool_text(self.auto_compact),
            SettingKey::ProjectDocs => bool_text(self.project_docs),
            SettingKey::Temperature => temperature_text(self.temperature),
        };
        if self.is_available(key, mode) {
            text
        } else {
            format!("{text}{UNAVAILABLE_SUFFIX}")
        }
    }

    /// Advance `key` to its next value, returning `true` when something moved.
    /// [`SettingKey::PermissionMode`] is **not** handled here — it lives on
    /// `App` and the caller routes it through the existing Ctrl+A path — and an
    /// unavailable setting refuses.
    pub fn cycle(&mut self, key: SettingKey) -> bool {
        if !self.is_available(key, Some(PermissionMode::default())) {
            return false;
        }
        match key {
            SettingKey::HideThinking => self.hide_thinking = !self.hide_thinking,
            SettingKey::ErrorRetry => self.error_retry = next_in(RETRY_CHOICES, &self.error_retry),
            SettingKey::Tools => self.tools = !self.tools,
            SettingKey::PermissionMode => return false,
            SettingKey::Checkpoints => self.checkpoints = !self.checkpoints,
            SettingKey::AutoCompact => self.auto_compact = !self.auto_compact,
            SettingKey::ProjectDocs => self.project_docs = !self.project_docs,
            SettingKey::Temperature => {
                self.temperature = next_temperature(self.temperature);
            }
        }
        true
    }

    /// Copy **one** setting's value out of `live` — the read-modify-write the
    /// boundary saves through (`save_permissions`' pattern).
    ///
    /// This is what keeps an `ALTER_ZERO_*` override from *sticking*: the
    /// startup blob merges the environment over the file, so writing the whole
    /// merged blob back would silently persist an override the user set for
    /// one run. Instead the boundary keeps the file's own copy and moves
    /// across only the key the user actually cycled. Availability is a host
    /// fact and never travels.
    pub fn copy_value(&mut self, key: SettingKey, live: &Self) {
        match key {
            SettingKey::HideThinking => self.hide_thinking = live.hide_thinking,
            SettingKey::ErrorRetry => self.error_retry = live.error_retry,
            SettingKey::Tools => self.tools = live.tools,
            SettingKey::Checkpoints => self.checkpoints = live.checkpoints,
            SettingKey::AutoCompact => self.auto_compact = live.auto_compact,
            SettingKey::ProjectDocs => self.project_docs = live.project_docs,
            SettingKey::Temperature => self.temperature = live.temperature,
            // Not ours — the posture persists per project in permissions.json.
            SettingKey::PermissionMode => {}
        }
    }

    /// Parse a `settings.json` body, best-effort: malformed or empty JSON
    /// yields the defaults rather than an error, so a corrupt file never blocks
    /// startup (`llm::Settings::parse`'s posture).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back to `settings.json`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// `skip_serializing_if` predicates: a field still at its default stays off the
/// wire, so the file only ever records what the user actually changed.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde hands these a reference
fn is_false(v: &bool) -> bool {
    !*v
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(v: &bool) -> bool {
    *v
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_retry(v: &u32) -> bool {
    *v == crate::llm::retry::MAX_RETRIES
}

/// `true`/`false`, the value column's boolean spelling.
fn bool_text(on: bool) -> String {
    if on { "true" } else { "false" }.to_string()
}

/// A temperature's value text: one decimal place, or `default` for `None`.
fn temperature_text(t: Option<f32>) -> String {
    t.map_or_else(
        || TEMPERATURE_DEFAULT_LABEL.to_string(),
        |t| format!("{t:.1}"),
    )
}

/// The entry after `current` in `choices`, wrapping — and starting from the
/// front when `current` isn't one of them (a hand-edited file, or an
/// `ALTER_ZERO_*` override with a value the menu doesn't offer).
fn next_in<T: Copy + PartialEq>(choices: &[T], current: &T) -> T {
    let next = choices
        .iter()
        .position(|c| c == current)
        .map_or(0, |i| (i + 1) % choices.len());
    choices[next]
}

/// [`next_in`] for the temperature list, which can't derive `PartialEq`
/// meaningfully on floats: compare within a hair so `0.30000001` from a
/// round-tripped file still finds its slot.
fn next_temperature(current: Option<f32>) -> Option<f32> {
    let at = TEMPERATURE_CHOICES.iter().position(|c| match (c, current) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() < f32::EPSILON,
        _ => false,
    });
    TEMPERATURE_CHOICES[at.map_or(0, |i| (i + 1) % TEMPERATURE_CHOICES.len())]
}

#[cfg(test)]
mod tests;

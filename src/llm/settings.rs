//! The persisted `/model` selection — kept **per working directory**, so each
//! project runs the model that was picked *in* it, with the last choice made
//! anywhere seeding a directory launched in for the first time
//! (`docs/per-directory-state.md`). Written to `~/.alter-zero/config.json` by
//! the boundary (`tui::config`); the parse/serialize here is **pure and
//! unit-tested**, like the `.env` keystore's. See `docs/llm.md`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::reasoning::{ReasoningEffort, ReasoningSupport, ThinkingMode};

/// The persisted `config.json`.
///
/// The **top-level** fields are the *last* selection made anywhere — the seed
/// a directory launched in for the first time takes ([`Settings::adopt`]) —
/// and also exactly the pre-directory file, so a `config.json` written before
/// selections were per directory still loads and applies everywhere until a
/// directory chooses otherwise. [`projects`](Self::projects) holds each
/// directory's own. Every field is optional so an old, partial, or future file
/// still loads — a missing key just leaves that setting unset, and unknown
/// keys are ignored.
///
/// ```json
/// {
///   "provider": "openrouter",
///   "model": "openai/gpt-4o-mini",
///   "projects": {
///     "/home/user/project": { "provider": "a0_venice", "model": "llama-3.3-70b" }
///   }
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The provider id of the last selection made anywhere (e.g. `openrouter`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The model id of the last selection made anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The saved model's reasoning capability + chosen mode, when it supports
    /// thinking — restored at startup so the Ctrl+T cycle needs no refetch.
    /// See `docs/reasoning.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// The saved model's image-input support, when its `/v1/models` record
    /// said either way — restored at startup so the backend gates image
    /// attachments without a re-probe. Absent = unknown (a legacy file, or a
    /// provider whose records don't say) — the probe finds out. See
    /// `docs/tools.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    /// The saved model's context window in tokens, when its `/v1/models`
    /// record reported one — restored at startup so the footer's
    /// `{used}/{window} ({pct}%)` gauge (and the auto-compact trigger) work
    /// without a re-probe. Absent = unknown. See `docs/compact.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// One entry per working directory (its absolute path): the selection
    /// made *in* that directory, or the last one pinned there at its first
    /// launch. Omitted when empty, so a file with no entries is byte-for-byte
    /// the old shape. See `docs/per-directory-state.md`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ModelSelection>,
}

/// One saved `/model` selection: the provider/model pair plus what is known
/// about the model — its reasoning state, image-input support and context
/// window, each `None` until the picker's record or the startup probe says.
/// The shape of a [`Settings::projects`] entry, and of the top-level "last"
/// selection read back through [`Settings::last`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    /// The provider id (e.g. `openrouter`).
    pub provider: String,
    /// The model id.
    pub model: String,
    /// The model's reasoning capability + chosen mode (`docs/reasoning.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// The model's image-input support (`docs/tools.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    /// The model's context window in tokens (`docs/compact.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

impl ModelSelection {
    /// A bare selection — the pair, nothing yet known about the model.
    #[must_use]
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            thinking: None,
            vision: None,
            context: None,
        }
    }

    /// The same selection with the model's reasoning state attached (`None`
    /// = unknown; [`ThinkingSettings::unsupported`] for a known non-reasoner).
    #[must_use]
    pub fn with_thinking(mut self, thinking: Option<ThinkingSettings>) -> Self {
        self.thinking = thinking;
        self
    }

    /// The same selection with the model's image-input support attached.
    #[must_use]
    pub fn with_vision(mut self, vision: Option<bool>) -> Self {
        self.vision = vision;
        self
    }

    /// The same selection with the model's context window attached.
    #[must_use]
    pub fn with_context(mut self, context: Option<u64>) -> Self {
        self.context = context;
        self
    }

    /// The `(provider, model)` pair — what makes two selections the *same*
    /// model, whatever each knows about it.
    #[must_use]
    pub fn pair(&self) -> (&str, &str) {
        (&self.provider, &self.model)
    }
}

/// The persisted reasoning state, as plain labels so a hand-edited or
/// future-versioned file still loads ([`ThinkingSettings::to_state`] skips
/// what it can't parse). See `docs/reasoning.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingSettings {
    /// `Some(false)` records "this model has no thinking" so startup doesn't
    /// re-probe `/v1/models` for it; absent/`Some(true)` means the rest of the
    /// blob describes a real reasoning state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
    /// The chosen [`ThinkingMode`] label (`off`/`on`/an effort name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The model's accepted effort labels (empty = an on/off-only reasoner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    /// Whether the model's reasoning can be turned off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_disable: Option<bool>,
    /// The provider-reported default effort, when named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

impl ThinkingSettings {
    /// The blob recording a live reasoning state.
    #[must_use]
    pub fn from_state(support: &ReasoningSupport, mode: ThinkingMode) -> Self {
        Self {
            supported: Some(true),
            mode: Some(mode.label().to_string()),
            efforts: Some(
                support
                    .efforts
                    .iter()
                    .map(|e| e.as_str().to_string())
                    .collect(),
            ),
            can_disable: Some(support.can_disable),
            default_effort: support.default_effort.map(|e| e.as_str().to_string()),
        }
    }

    /// The marker blob for a model known to have no thinking at all — startup
    /// then skips the support probe entirely.
    #[must_use]
    pub fn unsupported() -> Self {
        Self {
            supported: Some(false),
            ..Self::default()
        }
    }

    /// What to seed [`crate::app::App::set_thinking`] with: `None` for the
    /// [`unsupported`](Self::unsupported) marker, else the restored state.
    #[must_use]
    pub fn to_seed(&self) -> Option<(ReasoningSupport, ThinkingMode)> {
        if self.supported == Some(false) {
            return None;
        }
        Some(self.to_state())
    }

    /// Restore the live state: unknown effort labels are skipped, and a mode
    /// the restored support doesn't offer (or that doesn't parse) falls back
    /// to its default — a stale file degrades, never wedges.
    #[must_use]
    pub fn to_state(&self) -> (ReasoningSupport, ThinkingMode) {
        let support = ReasoningSupport {
            efforts: self
                .efforts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|label| ReasoningEffort::parse(label))
                .collect(),
            can_disable: self.can_disable.unwrap_or(true),
            default_effort: self
                .default_effort
                .as_deref()
                .and_then(ReasoningEffort::parse),
        };
        let mode = self
            .mode
            .as_deref()
            .and_then(ThinkingMode::parse)
            .filter(|m| support.modes().contains(m))
            .unwrap_or_else(|| support.default_mode());
        (support, mode)
    }
}

impl Settings {
    /// Parse a `config.json` body, best-effort: malformed or empty JSON yields
    /// the default (all-unset) settings rather than an error, so a corrupt file
    /// never blocks startup.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back to `config.json`. A plain
    /// struct of strings can't really fail to serialize; an impossible failure
    /// falls back to `{}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The settings whose last selection is `provider`/`model`, with no
    /// directory entries yet.
    #[must_use]
    pub fn for_selection(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
            ..Self::default()
        }
    }

    /// The same settings with the saved model's reasoning state attached
    /// (`None` for a model with no thinking support).
    #[must_use]
    pub fn with_thinking(mut self, thinking: Option<ThinkingSettings>) -> Self {
        self.thinking = thinking;
        self
    }

    /// The same settings with the saved model's image-input support attached
    /// (`None` = the record didn't say).
    #[must_use]
    pub fn with_vision(mut self, vision: Option<bool>) -> Self {
        self.vision = vision;
        self
    }

    /// The same settings with the saved model's context window attached
    /// (`None` = the record didn't report one). See `docs/compact.md`.
    #[must_use]
    pub fn with_context(mut self, context: Option<u64>) -> Self {
        self.context = context;
        self
    }

    // ----- per-directory selections (docs/per-directory-state.md) -----

    /// The last selection made anywhere — the top-level fields — when both
    /// halves of the pair are there. A half-written file (a model with no
    /// provider) is not a selection.
    #[must_use]
    pub fn last(&self) -> Option<ModelSelection> {
        Some(ModelSelection {
            provider: self.provider.clone()?,
            model: self.model.clone()?,
            thinking: self.thinking.clone(),
            vision: self.vision,
            context: self.context,
        })
    }

    /// Make `selection` the last one made anywhere.
    fn set_last(&mut self, selection: &ModelSelection) {
        self.provider = Some(selection.provider.clone());
        self.model = Some(selection.model.clone());
        self.thinking = selection.thinking.clone();
        self.vision = selection.vision;
        self.context = selection.context;
    }

    /// The entry a working directory holds, if it has one of its own.
    #[must_use]
    pub fn project(&self, dir: &str) -> Option<&ModelSelection> {
        self.projects.get(dir)
    }

    /// What a session in `dir` runs: the directory's own entry, else the last
    /// selection made anywhere, else nothing (the dummy).
    #[must_use]
    pub fn selection_for(&self, dir: &str) -> Option<ModelSelection> {
        self.project(dir).cloned().or_else(|| self.last())
    }

    /// Pin the last selection as `dir`'s own entry when it has none — what a
    /// directory launched in for the first time does, so that a later `/model`
    /// switch elsewhere never moves it. Returns whether anything changed (the
    /// boundary writes the file only then); a directory that already has an
    /// entry, or a file with no last selection, is left alone.
    pub fn adopt(&mut self, dir: &str) -> bool {
        if self.projects.contains_key(dir) {
            return false;
        }
        match self.last() {
            Some(last) => {
                self.projects.insert(dir.to_string(), last);
                true
            }
            None => false,
        }
    }

    /// Record a `/model` choice made in `dir`: it becomes the directory's
    /// entry **and** the last selection made anywhere (the seed for the next
    /// new directory). Every other directory's entry is untouched — the caller
    /// re-reads the file first, so two sessions in two directories never
    /// clobber each other.
    pub fn record(&mut self, dir: &str, selection: &ModelSelection) {
        self.projects.insert(dir.to_string(), selection.clone());
        self.set_last(selection);
    }

    /// Attach what Ctrl+T or the startup probe learned about the model `dir`
    /// already records — its thinking state, vision, window — **without**
    /// changing which model that is: the entry is replaced only when it names
    /// the same pair, and the last selection moves with it only when it is
    /// that same pair too (so a new directory then seeds without a probe).
    /// Returns whether the entry was updated; a directory recording a
    /// different model, or none, refuses — an env-overridden selection never
    /// writes back through here (env always wins, never sticks).
    pub fn record_capabilities(&mut self, dir: &str, selection: &ModelSelection) -> bool {
        let Some(entry) = self.projects.get_mut(dir) else {
            return false;
        };
        if entry.pair() != selection.pair() {
            return false;
        }
        *entry = selection.clone();
        if self
            .last()
            .is_some_and(|last| last.pair() == selection.pair())
        {
            self.set_last(selection);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_provider_and_model() {
        let s =
            Settings::parse(r#"{"provider":"openrouter","model":"anthropic/claude-3.5-haiku"}"#);
        assert_eq!(s.provider.as_deref(), Some("openrouter"));
        assert_eq!(s.model.as_deref(), Some("anthropic/claude-3.5-haiku"));
    }

    #[test]
    fn parse_of_garbage_or_empty_is_the_default() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("not json at all"), Settings::default());
        assert_eq!(Settings::parse("{}"), Settings::default());
    }

    #[test]
    fn parse_tolerates_missing_and_unknown_fields() {
        // Only `model` set; an unknown key is ignored (forward compatibility).
        let s = Settings::parse(r#"{"model":"gpt-4o-mini","theme":"dark"}"#);
        assert_eq!(s.provider, None);
        assert_eq!(s.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn json_round_trips() {
        let s = Settings::for_selection("openrouter", "vendor/model-1");
        assert_eq!(Settings::parse(&s.to_json()), s);
    }

    #[test]
    fn the_context_window_round_trips_and_omits_when_absent() {
        // The saved model's context window (docs/compact.md) — restored at
        // startup so the footer gauge shows without a re-probe.
        let s = Settings::for_selection("p", "m").with_context(Some(128_000));
        assert_eq!(Settings::parse(&s.to_json()).context, Some(128_000));
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("context"),
            "absent stays off the wire (old-shape compat)"
        );
    }

    fn trio_support() -> ReasoningSupport {
        ReasoningSupport {
            efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            can_disable: true,
            default_effort: Some(ReasoningEffort::Medium),
        }
    }

    #[test]
    fn thinking_round_trips_through_json() {
        let s = Settings::for_selection("openrouter", "vendor/model-1").with_thinking(Some(
            ThinkingSettings::from_state(
                &trio_support(),
                ThinkingMode::Effort(ReasoningEffort::High),
            ),
        ));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored, s);
        let (support, mode) = restored.thinking.unwrap().to_state();
        assert_eq!(support, trio_support());
        assert_eq!(mode, ThinkingMode::Effort(ReasoningEffort::High));
    }

    #[test]
    fn an_effortless_reasoner_round_trips() {
        // efforts = [] (Venice's on/off-only shape) must survive the file —
        // it is a real state, distinct from "no thinking support at all".
        let support = ReasoningSupport {
            efforts: vec![],
            can_disable: true,
            default_effort: None,
        };
        let blob = ThinkingSettings::from_state(&support, ThinkingMode::On);
        let (restored, mode) = blob.to_state();
        assert_eq!(restored, support);
        assert_eq!(mode, ThinkingMode::On);
    }

    #[test]
    fn a_legacy_file_without_thinking_loads_with_none() {
        let s = Settings::parse(r#"{"provider":"openrouter","model":"m"}"#);
        assert_eq!(s.thinking, None);
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("thinking"),
            "unset thinking is skipped in the output"
        );
    }

    #[test]
    fn a_known_non_reasoner_is_recorded_and_seeds_nothing() {
        // "This model has no thinking" is knowledge worth keeping — without
        // it every startup re-probes /v1/models for nothing. The marker blob
        // seeds None; a real state seeds Some; and a legacy/absent blob stays
        // "unknown" at the caller (which probes).
        let marker = ThinkingSettings::unsupported();
        assert_eq!(marker.to_seed(), None);
        let s = Settings::for_selection("p", "m").with_thinking(Some(marker));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored.thinking.unwrap().to_seed(), None);

        let real = ThinkingSettings::from_state(&trio_support(), ThinkingMode::Off);
        assert_eq!(real.to_seed(), Some((trio_support(), ThinkingMode::Off)));
    }

    #[test]
    fn a_stale_or_garbled_thinking_blob_degrades_gracefully() {
        // Unknown effort labels are skipped; an unparseable mode falls back to
        // the support's default. A hand-edited file can never wedge startup.
        let s = Settings::parse(
            r#"{"thinking":{"mode":"turbo","efforts":["low","warp","high"],"can_disable":false}}"#,
        );
        let (support, mode) = s.thinking.unwrap().to_state();
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
        assert!(!support.can_disable);
        assert_eq!(mode, support.default_mode());
    }

    #[test]
    fn vision_round_trips_and_legacy_files_load_with_none() {
        // The saved model's image-input support is persisted beside the
        // thinking blob so startup needs no re-probe; a legacy file simply
        // has it unknown (the probe finds out). See docs/tools.md.
        let s =
            Settings::for_selection("openrouter", "openai/gpt-oss-120b").with_vision(Some(false));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored.vision, Some(false));
        let known_vision = Settings::for_selection("p", "m").with_vision(Some(true));
        assert_eq!(Settings::parse(&known_vision.to_json()).vision, Some(true));
        let legacy = Settings::parse(r#"{"provider":"p","model":"m"}"#);
        assert_eq!(legacy.vision, None);
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("vision"),
            "unset vision is skipped in the output"
        );
    }

    #[test]
    fn to_json_omits_unset_fields() {
        let json = Settings::default().to_json();
        assert!(
            !json.contains("provider"),
            "unset fields are skipped: {json}"
        );
        assert!(!json.contains("model"), "unset fields are skipped: {json}");
    }

    // ===== per-directory selections (docs/per-directory-state.md) =====

    fn own(provider: &str, model: &str) -> ModelSelection {
        ModelSelection::new(provider, model)
    }

    #[test]
    fn a_directory_with_its_own_entry_outranks_the_last_selection() {
        // The top-level fields are the LAST selection made anywhere; a
        // directory's own entry, when it has one, is what that directory runs.
        let mut s = Settings::for_selection("p", "last");
        s.record("/a", &own("p", "own"));
        assert_eq!(s.selection_for("/a"), Some(own("p", "own")));
        assert_eq!(
            s.selection_for("/b"),
            Some(own("p", "own")),
            "recording a choice makes it the last selection too"
        );
        assert!(s.project("/b").is_none(), "/b has no entry of its own");
        assert_eq!(Settings::default().selection_for("/a"), None);
    }

    #[test]
    fn adopt_pins_the_last_selection_for_a_new_directory_exactly_once() {
        // A directory launched in for the first time takes the last selection
        // AND keeps it as its own entry, so a later switch elsewhere never
        // moves it. Adopting again is a no-op the boundary can skip writing.
        let mut s = Settings::for_selection("p", "last").with_vision(Some(true));
        assert!(s.adopt("/b"), "the first launch pins the last selection");
        assert_eq!(
            s.project("/b"),
            Some(&own("p", "last").with_vision(Some(true)))
        );
        assert!(!s.adopt("/b"), "already pinned — nothing to write");
        // A switch elsewhere afterwards leaves /b where it was.
        s.record("/a", &own("p", "newer"));
        assert_eq!(
            s.selection_for("/b"),
            Some(own("p", "last").with_vision(Some(true)))
        );
        assert_eq!(s.selection_for("/c"), Some(own("p", "newer")));
    }

    #[test]
    fn adopt_has_nothing_to_pin_without_a_last_selection() {
        let mut empty = Settings::default();
        assert!(!empty.adopt("/a"));
        assert!(empty.projects.is_empty());
        // A half-written file (a model with no provider) is not a selection.
        let mut half = Settings::parse(r#"{"model":"m"}"#);
        assert!(!half.adopt("/a"));
        assert_eq!(half.last(), None);
    }

    #[test]
    fn record_replaces_the_directory_entry_and_moves_the_last_selection() {
        let mut s = Settings::for_selection("p", "last");
        s.record("/a", &own("p", "x"));
        s.record("/z", &own("q", "other"));
        s.record("/a", &own("p", "y").with_context(Some(8_000)));
        assert_eq!(
            s.project("/a"),
            Some(&own("p", "y").with_context(Some(8_000)))
        );
        assert_eq!(
            s.project("/z"),
            Some(&own("q", "other")),
            "other directories untouched"
        );
        assert_eq!(s.last(), Some(own("p", "y").with_context(Some(8_000))));
    }

    #[test]
    fn record_capabilities_touches_only_the_pair_the_directory_records() {
        // Ctrl+T and the startup probe attach what they learned to the model a
        // directory already records — never to a different model, and never
        // moving the last selection unless it IS that same pair.
        let learned = own("p", "m").with_thinking(Some(ThinkingSettings::unsupported()));
        let mut s = Settings::for_selection("p", "m");
        s.record("/a", &own("p", "m"));
        assert!(s.record_capabilities("/a", &learned));
        assert_eq!(s.project("/a"), Some(&learned));
        assert_eq!(
            s.last(),
            Some(learned.clone()),
            "the same pair: the seed learns too"
        );

        // The last selection is a different model now — it must not move.
        s.record("/b", &own("p", "other"));
        let refreshed = own("p", "m").with_vision(Some(false));
        assert!(s.record_capabilities("/a", &refreshed));
        assert_eq!(s.project("/a"), Some(&refreshed));
        assert_eq!(s.last(), Some(own("p", "other")));

        // A directory recording a different pair, or none, refuses.
        assert!(!s.record_capabilities("/b", &refreshed));
        assert_eq!(s.project("/b"), Some(&own("p", "other")));
        assert!(!s.record_capabilities("/nowhere", &refreshed));
        assert!(s.project("/nowhere").is_none());
    }

    #[test]
    fn a_pre_directory_file_loads_as_the_last_selection_alone() {
        // The old flat file is exactly the new file's top level, so an
        // existing config.json keeps applying everywhere until a directory
        // chooses otherwise — and a file with no entries writes no `projects`.
        let s = Settings::parse(r#"{"provider":"p","model":"m","vision":true}"#);
        assert_eq!(s.last(), Some(own("p", "m").with_vision(Some(true))));
        assert!(s.projects.is_empty());
        assert!(!s.to_json().contains("projects"), "{}", s.to_json());
    }

    #[test]
    fn directory_entries_round_trip_through_json() {
        let mut s = Settings::for_selection("p", "last");
        s.record(
            "/home/u/a",
            &own("p", "m")
                .with_thinking(Some(ThinkingSettings::from_state(
                    &trio_support(),
                    ThinkingMode::Effort(ReasoningEffort::Low),
                )))
                .with_vision(Some(false))
                .with_context(Some(32_000)),
        );
        let json = s.to_json();
        assert!(json.contains("\"projects\""), "{json}");
        assert!(json.contains("/home/u/a"), "{json}");
        assert_eq!(Settings::parse(&json), s);
    }
}

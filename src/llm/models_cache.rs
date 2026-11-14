//! The `/model` picker's kept lists (`docs/model-cache.md`): the
//! `model-cache.json` format, which kept list is still fresh, what an open or a
//! Ctrl+R does about each provider ([`plan`]), and the ledger of fetches still
//! out ([`FetchLedger`]).
//!
//! All of it is **pure**. The boundary (`tui::models`) reads and writes the
//! file, works out each provider's [`source_of`], runs the fetches, and does
//! what the plan says.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::config::ModelConfig;
use super::models::ModelEntry;
use super::reasoning::{ReasoningEffort, ReasoningSupport};
use super::service_tier::ServiceTier;

/// The kept lists' file in the config home.
pub const MODEL_CACHE_FILE_NAME: &str = "model-cache.json";

/// How long a kept list is **fresh** — shown without being fetched again — in
/// seconds. Model catalogs change over days, so this is not about staleness:
/// it is what keeps a reopened picker from refetching every provider. Past
/// it, a list is still shown at once and fetched again behind its rows.
pub const MODEL_CACHE_FRESH_SECS: i64 = 60 * 60;

/// The largest file read back. A cache is ours to rewrite, so anything bigger
/// than any real set of catalogs reads as empty rather than being parsed.
pub const MODEL_CACHE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// `model-cache.json`: one kept list per provider id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCache {
    #[serde(default)]
    providers: BTreeMap<String, KeptList>,
}

/// One provider's kept list on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KeptList {
    /// Where it was read from ([`source_of`]).
    source: String,
    /// When it was read, in Unix seconds.
    fetched_at: i64,
    #[serde(default)]
    models: Vec<KeptModel>,
}

/// One row of a kept list — a [`ModelEntry`] without its provider, which is
/// the list's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KeptModel {
    id: String,
    /// The friendly name, absent when it is the id itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning: Option<KeptReasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    service_tiers: Vec<ServiceTier>,
}

/// A [`ReasoningSupport`] as labels, so a label this build does not know is
/// skipped rather than failing the file (`ThinkingSettings`' rule).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct KeptReasoning {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    efforts: Vec<String>,
    #[serde(default)]
    can_disable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_effort: Option<String>,
}

/// One provider's kept list as the picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedListing {
    /// The provider id.
    pub provider: String,
    /// When the list was fetched, in Unix seconds.
    pub fetched_at: i64,
    /// Its rows.
    pub models: Vec<ModelEntry>,
}

impl ModelCache {
    /// Parse the file. Anything that isn't a cache this build can read is an
    /// empty one — the next fetch writes it again.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize for writing back — compact: nobody reads this file but us.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The list kept for `provider`, when it was read from `source`.
    #[must_use]
    pub fn lookup(&self, provider: &str, source: &str) -> Option<CachedListing> {
        let kept = self
            .providers
            .get(provider)
            .filter(|kept| kept.source == source)?;
        Some(CachedListing {
            provider: provider.to_string(),
            fetched_at: kept.fetched_at,
            models: kept.models.iter().map(|row| row.entry(provider)).collect(),
        })
    }

    /// Keep `models` as `provider`'s list, read from `source` at `fetched_at`,
    /// in place of whatever was kept for it.
    pub fn store(&mut self, provider: &str, source: &str, fetched_at: i64, models: &[ModelEntry]) {
        self.providers.insert(
            provider.to_string(),
            KeptList {
                source: source.to_string(),
                fetched_at,
                models: models.iter().map(KeptModel::of).collect(),
            },
        );
    }

    /// Drop `provider`'s kept list; whether there was one.
    pub fn forget(&mut self, provider: &str) -> bool {
        self.providers.remove(provider).is_some()
    }
}

impl KeptModel {
    /// The row a picker entry is kept as.
    fn of(entry: &ModelEntry) -> Self {
        Self {
            id: entry.id.clone(),
            name: (entry.display_name != entry.id).then(|| entry.display_name.clone()),
            reasoning: entry.reasoning.as_ref().map(|support| KeptReasoning {
                efforts: support
                    .efforts
                    .iter()
                    .map(|effort| effort.as_str().to_string())
                    .collect(),
                can_disable: support.can_disable,
                default_effort: support.default_effort.map(|e| e.as_str().to_string()),
            }),
            vision: entry.vision,
            context: entry.context,
            service_tiers: entry.service_tiers.clone(),
        }
    }

    /// The picker entry this row reads back as, under `provider`.
    fn entry(&self, provider: &str) -> ModelEntry {
        ModelEntry {
            id: self.id.clone(),
            provider: provider.to_string(),
            display_name: self.name.clone().unwrap_or_else(|| self.id.clone()),
            reasoning: self.reasoning.as_ref().map(|kept| ReasoningSupport {
                efforts: kept
                    .efforts
                    .iter()
                    .filter_map(|label| ReasoningEffort::parse(label))
                    .collect(),
                can_disable: kept.can_disable,
                default_effort: kept
                    .default_effort
                    .as_deref()
                    .and_then(ReasoningEffort::parse),
            }),
            vision: self.vision,
            context: self.context,
            service_tiers: self.service_tiers.clone(),
        }
    }
}

/// Whether a list fetched at `fetched_at` is still fresh at `now` (both Unix
/// seconds). A list stamped after `now` — a clock set back, a file from a
/// machine whose clock runs ahead — is not: its age can't be known.
#[must_use]
pub fn is_fresh(fetched_at: i64, now: i64) -> bool {
    now.checked_sub(fetched_at)
        .is_some_and(|age| (0..MODEL_CACHE_FRESH_SECS).contains(&age))
}

/// Where a provider's list is read from, as a fingerprint: the listing URL,
/// the chat base, the auth scheme, the wire format and the provider's own
/// headers. A kept list is shown only for the source it was read from, so a
/// provider pointed elsewhere never shows another server's models. Never the
/// credential: a rotating refresh token would miss after every rotation, and
/// a changed key is `/login`'s to report (`docs/model-cache.md`).
#[must_use]
pub fn source_of(cfg: &ModelConfig) -> String {
    let mut source = format!(
        "{}\n{}\n{:?}\n{:?}",
        super::models::models_endpoint(cfg),
        cfg.api_base,
        cfg.auth,
        cfg.wire_api
    );
    for (name, value) in &cfg.extra_headers {
        source.push_str(&format!("\n{name}: {value}"));
    }
    crate::trust::fingerprint(source.as_bytes())
}

/// What [`plan`] is told about one configured provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProvider {
    /// The provider id.
    pub provider: String,
    /// Where its list is read from now ([`source_of`]).
    pub source: String,
    /// A local model server's: never fresh, fetched behind its rows on every
    /// open.
    pub always_refresh: bool,
}

/// What to do about one provider's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// Nothing: its kept list is fresh.
    No,
    /// Start a fetch.
    Start,
    /// Wait on the fetch already out for it.
    Await,
}

/// What an open (or a Ctrl+R) does about one provider: the kept list to show
/// at once, if any, and whether to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPlan {
    /// The kept list to show at once.
    pub cached: Option<CachedListing>,
    /// Whether its list is fetched.
    pub fetch: Fetch,
}

/// Decide what to do about one configured provider's list at `now`: show what
/// was kept for its current source, and fetch it when nothing fresh was kept,
/// when it is a local model server, or when `force` (Ctrl+R) says so — unless
/// a fetch from that same source is already out, which is then waited on.
#[must_use]
pub fn plan(
    provider: &PlanProvider,
    cache: &ModelCache,
    ledger: &FetchLedger,
    now: i64,
    force: bool,
) -> ProviderPlan {
    let cached = cache.lookup(&provider.provider, &provider.source);
    let fresh = !force
        && !provider.always_refresh
        && cached
            .as_ref()
            .is_some_and(|kept| is_fresh(kept.fetched_at, now));
    let fetch = if fresh {
        Fetch::No
    } else if ledger.live_source(&provider.provider) == Some(provider.source.as_str()) {
        Fetch::Await
    } else {
        Fetch::Start
    };
    ProviderPlan { cached, fetch }
}

/// The fetches still out: one live ticket per provider. A list is kept, and
/// shown, only when it answers its provider's live ticket — so a fetch that
/// was superseded, or retired because the credential it was made with
/// changed, is dropped when it lands (`docs/model-cache.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchLedger {
    next: u64,
    live: BTreeMap<String, (u64, String)>,
}

impl FetchLedger {
    /// Open a ticket for a fetch of `provider`'s list from `source`,
    /// superseding any still out for it.
    pub fn issue(&mut self, provider: &str, source: &str) -> u64 {
        self.next += 1;
        self.live
            .insert(provider.to_string(), (self.next, source.to_string()));
        self.next
    }

    /// The source `provider`'s live fetch reads from, while one is out.
    #[must_use]
    pub fn live_source(&self, provider: &str) -> Option<&str> {
        self.live.get(provider).map(|(_, source)| source.as_str())
    }

    /// A fetch of `provider`'s list landed with `ticket`: the source it was
    /// read from when it answers the live ticket, which closes — `None` for a
    /// superseded or retired one.
    pub fn settle(&mut self, provider: &str, ticket: u64) -> Option<String> {
        if self.live.get(provider)?.0 != ticket {
            return None;
        }
        self.live.remove(provider).map(|(_, source)| source)
    }

    /// Retire whatever fetch is out for `provider`: its list is neither kept
    /// nor shown when it lands.
    pub fn retire(&mut self, provider: &str) {
        self.live.remove(provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{AuthScheme, WireApi};

    fn entry(id: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            provider: "openrouter".into(),
            display_name: name.into(),
            reasoning: None,
            vision: None,
            context: None,
            service_tiers: Vec::new(),
        }
    }

    /// A row with every field set.
    fn full_entry() -> ModelEntry {
        ModelEntry {
            reasoning: Some(ReasoningSupport {
                efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                can_disable: true,
                default_effort: Some(ReasoningEffort::Medium),
            }),
            vision: Some(true),
            context: Some(1_000_000),
            service_tiers: vec![ServiceTier::new("priority", "Fast", "1.5x speed")],
            ..entry("anthropic/claude-fable-5", "Anthropic: Claude Fable 5")
        }
    }

    const NOW: i64 = 1_763_110_000;

    // --- the file ---

    #[test]
    fn a_kept_list_round_trips_every_field_of_its_rows() {
        let rows = vec![full_entry(), entry("openai/gpt-oss-120b", "GPT OSS 120B")];
        let mut cache = ModelCache::default();
        cache.store("openrouter", "sha256:or", NOW, &rows);
        let read = ModelCache::parse(&cache.to_json());
        assert_eq!(
            read.lookup("openrouter", "sha256:or"),
            Some(CachedListing {
                provider: "openrouter".into(),
                fetched_at: NOW,
                models: rows,
            })
        );
    }

    #[test]
    fn a_row_named_by_its_id_keeps_no_name_and_reads_back_as_its_id() {
        let mut cache = ModelCache::default();
        cache.store("openrouter", "s", NOW, &[entry("bare/id", "bare/id")]);
        let json = cache.to_json();
        assert!(!json.contains("\"name\""), "{json}");
        let read = ModelCache::parse(&json).lookup("openrouter", "s").unwrap();
        assert_eq!(read.models[0].display_name, "bare/id");
    }

    #[test]
    fn a_kept_row_is_tagged_with_the_provider_it_was_kept_under() {
        let mut cache = ModelCache::default();
        cache.store("a0_venice", "s", NOW, &[entry("m", "M")]);
        let read = ModelCache::parse(&cache.to_json());
        assert_eq!(
            read.lookup("a0_venice", "s").unwrap().models[0].provider,
            "a0_venice"
        );
    }

    #[test]
    fn an_effort_this_build_does_not_know_is_skipped_not_fatal() {
        let text = r#"{"providers":{"p":{"source":"s","fetched_at":1,"models":[
            {"id":"a","reasoning":{"efforts":["low","warp","high"],"can_disable":false,
             "default_effort":"warp"}},
            {"id":"b"}
        ]}}}"#;
        let models = ModelCache::parse(text).lookup("p", "s").unwrap().models;
        assert_eq!(models.len(), 2, "the other row still reads");
        assert_eq!(
            models[0].reasoning,
            Some(ReasoningSupport {
                efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
                can_disable: false,
                default_effort: None,
            })
        );
    }

    #[test]
    fn a_file_that_does_not_parse_reads_as_empty() {
        for text in ["", "not json", "[]", r#"{"providers":{"p":{"source":1}}}"#] {
            assert_eq!(ModelCache::parse(text), ModelCache::default(), "{text:?}");
        }
    }

    #[test]
    fn a_kept_list_is_used_only_for_the_source_it_was_read_from() {
        let mut cache = ModelCache::default();
        cache.store("ollama", "sha256:here", NOW, &[entry("qwen3", "qwen3")]);
        assert!(cache.lookup("ollama", "sha256:here").is_some());
        assert_eq!(cache.lookup("ollama", "sha256:there"), None);
        assert_eq!(cache.lookup("lmstudio", "sha256:here"), None);
    }

    #[test]
    fn storing_a_list_replaces_the_one_kept_before() {
        let mut cache = ModelCache::default();
        cache.store("p", "s", NOW, &[entry("old", "Old")]);
        cache.store("p", "s", NOW + 5, &[entry("new", "New")]);
        let kept = cache.lookup("p", "s").unwrap();
        assert_eq!(kept.fetched_at, NOW + 5);
        let ids: Vec<&str> = kept.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["new"]);
    }

    #[test]
    fn forgetting_a_provider_drops_only_its_list() {
        let mut cache = ModelCache::default();
        cache.store("a", "s", NOW, &[entry("x", "X")]);
        cache.store("b", "s", NOW, &[entry("y", "Y")]);
        assert!(cache.forget("a"));
        assert!(!cache.forget("a"), "nothing left to forget");
        assert_eq!(cache.lookup("a", "s"), None);
        assert!(cache.lookup("b", "s").is_some());
    }

    // --- freshness ---

    #[test]
    fn a_kept_list_is_fresh_for_an_hour() {
        assert!(is_fresh(NOW, NOW));
        assert!(is_fresh(NOW, NOW + MODEL_CACHE_FRESH_SECS - 1));
        assert!(!is_fresh(NOW, NOW + MODEL_CACHE_FRESH_SECS));
        assert_eq!(MODEL_CACHE_FRESH_SECS, 3_600);
    }

    #[test]
    fn a_list_stamped_after_the_clock_now_is_not_fresh() {
        assert!(!is_fresh(NOW + 1, NOW));
        assert!(!is_fresh(i64::MAX, i64::MIN), "no overflow at the extremes");
        assert!(!is_fresh(i64::MIN, i64::MAX));
    }

    // --- the source ---

    fn cfg() -> ModelConfig {
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = "https://openrouter.ai/api/v1".into();
        cfg.api_model_base = "https://openrouter.ai/api/v1".into();
        cfg.api_key = Some("sk-one".into());
        cfg
    }

    #[test]
    fn the_source_moves_with_where_the_list_is_read_from() {
        let base = source_of(&cfg());
        assert!(base.starts_with("sha256:"), "{base}");
        let mut listing = cfg();
        listing.api_model_base = "http://127.0.0.1:11434".into();
        let mut chat = cfg();
        chat.api_base = "https://elsewhere.example/v1".into();
        let mut auth = cfg();
        auth.auth = AuthScheme::GithubCopilot;
        let mut wire = cfg();
        wire.wire_api = WireApi::Ollama;
        let mut headers = cfg();
        headers.extra_headers.push(("X-Org".into(), "acme".into()));
        for (what, moved) in [
            ("the listing base", listing),
            ("the chat base", chat),
            ("the auth scheme", auth),
            ("the wire format", wire),
            ("a header", headers),
        ] {
            assert_ne!(source_of(&moved), base, "{what} must move the source");
        }
    }

    #[test]
    fn the_source_never_carries_the_credential_or_the_model() {
        let base = source_of(&cfg());
        let mut key = cfg();
        key.api_key = Some("sk-rotated".into());
        let mut model = cfg();
        model.model = "another/model".into();
        assert_eq!(source_of(&key), base, "a rotated token keeps the list");
        assert_eq!(source_of(&model), base, "the listing is the provider's");
        assert!(!base.contains("openrouter"), "a fingerprint, never the URL");
    }

    // --- the plan ---

    fn provider(id: &str) -> PlanProvider {
        PlanProvider {
            provider: id.into(),
            source: "sha256:s".into(),
            always_refresh: false,
        }
    }

    fn cache_with(id: &str, fetched_at: i64) -> ModelCache {
        let mut cache = ModelCache::default();
        cache.store(id, "sha256:s", fetched_at, &[entry("m", "M")]);
        cache
    }

    #[test]
    fn a_fresh_kept_list_is_shown_and_not_fetched() {
        let cache = cache_with("p", NOW - 60);
        let decided = plan(&provider("p"), &cache, &FetchLedger::default(), NOW, false);
        assert_eq!(decided.fetch, Fetch::No);
        assert_eq!(decided.cached.map(|c| c.fetched_at), Some(NOW - 60));
    }

    #[test]
    fn a_stale_kept_list_is_shown_and_fetched_behind_it() {
        let cache = cache_with("p", NOW - MODEL_CACHE_FRESH_SECS);
        let decided = plan(&provider("p"), &cache, &FetchLedger::default(), NOW, false);
        assert_eq!(decided.fetch, Fetch::Start);
        assert!(decided.cached.is_some(), "the old rows show meanwhile");
    }

    #[test]
    fn a_provider_with_nothing_kept_is_fetched() {
        let decided = plan(
            &provider("p"),
            &ModelCache::default(),
            &FetchLedger::default(),
            NOW,
            false,
        );
        assert_eq!(
            decided,
            ProviderPlan {
                cached: None,
                fetch: Fetch::Start,
            }
        );
    }

    #[test]
    fn a_list_kept_from_another_source_is_neither_shown_nor_trusted() {
        let cache = cache_with("p", NOW);
        let moved = PlanProvider {
            source: "sha256:elsewhere".into(),
            ..provider("p")
        };
        let decided = plan(&moved, &cache, &FetchLedger::default(), NOW, false);
        assert_eq!(decided.cached, None);
        assert_eq!(decided.fetch, Fetch::Start);
    }

    #[test]
    fn a_local_model_server_is_fetched_on_every_open() {
        let cache = cache_with("ollama", NOW);
        let local = PlanProvider {
            always_refresh: true,
            ..provider("ollama")
        };
        let decided = plan(&local, &cache, &FetchLedger::default(), NOW, false);
        assert_eq!(decided.fetch, Fetch::Start);
        assert!(decided.cached.is_some(), "its kept rows still show at once");
    }

    #[test]
    fn a_refresh_fetches_a_fresh_list_too() {
        let cache = cache_with("p", NOW);
        let decided = plan(&provider("p"), &cache, &FetchLedger::default(), NOW, true);
        assert_eq!(decided.fetch, Fetch::Start);
    }

    #[test]
    fn a_fetch_already_out_is_waited_on_not_started_again() {
        let mut ledger = FetchLedger::default();
        ledger.issue("p", "sha256:s");
        for force in [false, true] {
            let decided = plan(&provider("p"), &ModelCache::default(), &ledger, NOW, force);
            assert_eq!(decided.fetch, Fetch::Await, "force: {force}");
        }
    }

    #[test]
    fn a_fetch_out_from_another_source_is_not_waited_on() {
        let mut ledger = FetchLedger::default();
        ledger.issue("p", "sha256:old");
        let decided = plan(&provider("p"), &ModelCache::default(), &ledger, NOW, false);
        assert_eq!(decided.fetch, Fetch::Start);
    }

    // --- the ledger ---

    #[test]
    fn a_landed_fetch_answers_only_its_live_ticket() {
        let mut ledger = FetchLedger::default();
        let first = ledger.issue("p", "sha256:a");
        let second = ledger.issue("p", "sha256:b");
        assert_ne!(first, second);
        assert_eq!(ledger.live_source("p"), Some("sha256:b"));
        assert_eq!(ledger.settle("p", first), None, "superseded");
        assert_eq!(ledger.settle("p", second), Some("sha256:b".into()));
        assert_eq!(ledger.live_source("p"), None, "settling closes the ticket");
        assert_eq!(ledger.settle("p", second), None, "a ticket answers once");
    }

    #[test]
    fn tickets_are_per_provider() {
        let mut ledger = FetchLedger::default();
        let a = ledger.issue("a", "s");
        let b = ledger.issue("b", "s");
        assert_eq!(ledger.settle("b", a), None, "a's ticket is not b's");
        assert_eq!(ledger.settle("a", a), Some("s".into()));
        assert_eq!(ledger.settle("b", b), Some("s".into()));
    }

    #[test]
    fn a_retired_fetch_is_never_kept() {
        let mut ledger = FetchLedger::default();
        let ticket = ledger.issue("p", "s");
        ledger.retire("p");
        assert_eq!(ledger.live_source("p"), None);
        assert_eq!(ledger.settle("p", ticket), None);
    }
}

//! The `/model` picker's list cache (`docs/model-cache.md`): each provider's
//! listing as it was last fetched, kept on disk so the picker opens on the
//! rows at once instead of on `Loading models…`.
//!
//! Pure — the file's format ([`CachedListing`]), what a listing is worth when
//! the picker opens ([`freshness`]), what it was fetched from
//! ([`listing_source`]) and the file it lives in ([`file_name`]). The reads
//! and the writes are the boundary's (`tui::config`), and so is the clock.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize, Serializer};

use super::config::ModelConfig;
use super::models::ModelEntry;
use super::reasoning::{ReasoningEffort, ReasoningSupport};
use super::service_tier::ServiceTier;
use crate::stream::CancelToken;

/// The cache file's format version. A file of any other version is no cache
/// at all: its provider is fetched as though nothing had been kept.
pub const CACHE_VERSION: u32 = 1;

/// How long a listing is used as it is, in seconds. Within it the picker
/// asks no provider anything; past it the kept rows still show at once and
/// the provider is asked again behind them. An hour: a catalog changes over
/// days, so a model listed this morning is there by lunch without a key
/// pressed, while reopening the picker to switch back and forth costs no
/// request at all. Ctrl+R asks at any age.
pub const MODEL_CACHE_TTL_SECS: i64 = 60 * 60;

/// The most bytes a cache file is read to — `MODELS_BODY_MAX_BYTES`, the
/// listing's own bound, since a file holds one listing's rows and no more.
pub const CACHE_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// One provider's listing, as the cache keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedListing {
    /// When it was fetched, in Unix seconds.
    pub fetched_at: i64,
    /// The version of the app that fetched it — a listing another version
    /// fetched may lack a fact this one reads, so it is shown and fetched
    /// again ([`Freshness::Stale`]).
    pub app_version: String,
    /// What it was fetched from: [`listing_source`] of the provider's
    /// config at the time.
    pub source: String,
    /// The rows, as the fetch returned them.
    pub models: Vec<ModelEntry>,
}

/// What a provider's cached listing is worth as the picker opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Shown as it is, and nothing is fetched.
    Fresh,
    /// Shown at once, and fetched again behind it.
    Stale,
    /// Not shown at all: it was fetched from somewhere else, so its rows are
    /// another server's. The provider is fetched as though nothing had been
    /// kept.
    Unusable,
}

impl CachedListing {
    /// Write the listing as its cache file: compact JSON, each row without
    /// its provider (the file is the provider's own) and without a name that
    /// only repeats its id. Serialized from the rows by reference, never
    /// copied into a tree first (`docs/memory.md`).
    ///
    /// # Errors
    /// Whatever `out` refuses.
    pub fn write_to(&self, out: impl Write) -> serde_json::Result<()> {
        serde_json::to_writer(
            out,
            &FileOut {
                version: CACHE_VERSION,
                app_version: &self.app_version,
                source: &self.source,
                fetched_at: self.fetched_at,
                models: Rows(&self.models),
            },
        )
    }

    /// Read a cache file back, its rows tagged with `provider`. `None` for a
    /// file of another [`CACHE_VERSION`], or one that does not parse — a
    /// cache that cannot be read is a cache that was never written. An
    /// effort label this version does not know is skipped, never fatal (the
    /// `config.json` thinking blob's rule).
    #[must_use]
    pub fn read_from(reader: impl Read, provider: &str) -> Option<Self> {
        let file: FileIn = serde_json::from_reader(reader).ok()?;
        if file.version != CACHE_VERSION {
            return None;
        }
        let models = file
            .models
            .into_iter()
            .map(|row| row.into_entry(provider))
            .collect();
        Some(Self {
            fetched_at: file.fetched_at,
            app_version: file.app_version,
            source: file.source,
            models,
        })
    }
}

/// What `listing` is worth for a provider whose config now has `source`,
/// read by `app_version` at `now` (Unix seconds): [`Freshness::Unusable`]
/// when it was fetched from somewhere else, [`Freshness::Stale`] when it is
/// [`MODEL_CACHE_TTL_SECS`] old or more, dated after `now` (a clock that
/// moved back), fetched by another version of the app, or `self_hosted`
/// ([`is_self_hosted`] — a server the user runs, whose listing changes
/// whenever a model is pulled) — and [`Freshness::Fresh`] otherwise.
#[must_use]
pub fn freshness(
    listing: &CachedListing,
    source: &str,
    app_version: &str,
    now: i64,
    self_hosted: bool,
) -> Freshness {
    if listing.source != source {
        return Freshness::Unusable;
    }
    let young = now
        .checked_sub(listing.fetched_at)
        .is_some_and(|age| (0..MODEL_CACHE_TTL_SECS).contains(&age));
    if self_hosted || listing.app_version != app_version || !young {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

/// What a provider's listing is fetched from, fingerprinted: its wire
/// format, how it authenticates, the two bases (the listing's, and the chat
/// base a server without a listing of its own is listed through) and the
/// headers its requests carry. A `providers.toml` edit or an address saved
/// through `/login` that moves any of them makes a kept listing another
/// server's ([`Freshness::Unusable`]). The model and the key are not part of
/// it: one names no listing, and the other is never written anywhere but
/// the key store — a signed-in account changing is the `/login` boundary's
/// to say, by dropping the provider's file. Fingerprinted rather than
/// written out, so a base that carries a password writes it nowhere new.
#[must_use]
pub fn listing_source(cfg: &ModelConfig) -> String {
    let mut identity = format!(
        "{:?}\n{:?}\n{}\n{}",
        cfg.wire_api,
        cfg.auth,
        cfg.api_model_base.trim_end_matches('/'),
        cfg.api_base.trim_end_matches('/'),
    );
    for (name, value) in &cfg.extra_headers {
        identity.push_str(&format!("\n{name}: {value}"));
    }
    crate::trust::fingerprint(identity.as_bytes())
}

/// Whether the provider is a server the user runs — Ollama, LM Studio, a
/// keyless server of their own: the providers whose key is optional
/// (`docs/local-models.md`). Its listing is the one a cache must never keep
/// from the user for an hour, since pulling a model is exactly how such a
/// list changes, and asking costs one request to a server they own. Read off
/// the kind of provider rather than its address: one on another machine of
/// theirs changes on every pull just the same, while a key-taking gateway on
/// localhost lists a catalog that moves no faster than any other.
#[must_use]
pub fn is_self_hosted(cfg: &ModelConfig) -> bool {
    cfg.auth.key_optional()
}

/// The file a provider's listing is kept in, inside the cache directory:
/// `{id}.json` for an id of ASCII letters, digits, `-` and `_` — every id
/// the shipped providers use — and the id's fingerprint otherwise, so an id
/// a user's `providers.toml` quoted (`"team/proxy"`, `".."`) can never name
/// a path outside the directory.
#[must_use]
pub fn file_name(provider: &str) -> String {
    let plain = !provider.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plain {
        format!("{provider}.json")
    } else {
        let digest = crate::trust::fingerprint(provider.as_bytes());
        format!("{}.json", digest.trim_start_matches("sha256:"))
    }
}

// ---------------------------------------------------------------------------
// The fetches still out
// ---------------------------------------------------------------------------

/// How long, in seconds, a list fetch may stay out before an open stops
/// waiting on it and asks again. A listing is one request (a few, for
/// Ollama's `/api/show` walk) and answers in seconds; one that has not in a
/// minute is a worker that will never report, and it must not hold its
/// provider for the rest of the session.
pub const FETCH_GIVE_UP_SECS: i64 = 60;

/// What an open (or a Ctrl+R) does about one provider's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchPlan {
    /// Nothing: the kept list is fresh.
    Keep,
    /// Wait on the fetch already out for it — the picker shows the kept rows
    /// meanwhile, and asking again would only be a second request for the
    /// same answer.
    Await,
    /// Start a fetch.
    Start,
}

/// What to do about one provider's list (`docs/model-cache.md`): wait on a
/// fetch already `out` for it from the same source, whatever else holds;
/// else keep a `Fresh` kept list as it is, unless `force` (Ctrl+R) says ask
/// anyway; else start a fetch. `kept` is what its kept listing is worth,
/// `None` when nothing was kept.
#[must_use]
pub fn plan(kept: Option<Freshness>, out: bool, force: bool) -> FetchPlan {
    if out {
        FetchPlan::Await
    } else if !force && kept == Some(Freshness::Fresh) {
        FetchPlan::Keep
    } else {
        FetchPlan::Start
    }
}

/// The list fetches still out: one live **ticket** per provider
/// (`docs/model-cache.md`). A fetch outlives the picker that started it —
/// what it reads is no less true for the picker having closed — so whether
/// its answer is kept and shown is decided when it lands, by whether its
/// ticket is still the provider's live one. A newer fetch for the provider
/// supersedes it, and a `/login` save of the provider's credential, or an
/// open that will not wait on it, retires it: either way it is cancelled,
/// and whatever it brings is neither kept nor shown.
#[derive(Debug, Default)]
pub struct FetchLedger {
    next: u64,
    live: BTreeMap<String, LiveFetch>,
}

/// One provider's fetch still out.
#[derive(Debug)]
struct LiveFetch {
    ticket: u64,
    /// What it reads from ([`listing_source`]).
    source: String,
    /// When it started, in Unix seconds.
    started_at: i64,
    /// Its own cancel flag, raised when it is superseded or retired.
    cancel: CancelToken,
}

impl FetchLedger {
    /// Open a ticket for a fetch of `provider`'s list from `source`, started
    /// at `now`, superseding — and cancelling — any fetch still out for it.
    /// The ticket names the fetch's answer; the token is the fetch's own.
    pub fn issue(&mut self, provider: &str, source: &str, now: i64) -> (u64, CancelToken) {
        self.next += 1;
        let cancel = CancelToken::new();
        let fetch = LiveFetch {
            ticket: self.next,
            source: source.to_string(),
            started_at: now,
            cancel: cancel.clone(),
        };
        if let Some(superseded) = self.live.insert(provider.to_string(), fetch) {
            superseded.cancel.cancel();
        }
        (self.next, cancel)
    }

    /// Whether a fetch of `provider`'s list from `source` is out and young
    /// enough at `now` to be waited on — under [`FETCH_GIVE_UP_SECS`] old,
    /// by a clock that has not moved back past its start.
    #[must_use]
    pub fn awaits(&self, provider: &str, source: &str, now: i64) -> bool {
        self.live.get(provider).is_some_and(|fetch| {
            fetch.source == source
                && now
                    .checked_sub(fetch.started_at)
                    .is_some_and(|age| (0..FETCH_GIVE_UP_SECS).contains(&age))
        })
    }

    /// What an open does about the fetch out for `provider`: waits on it —
    /// `true` — when [`awaits`](Self::awaits) says it may, and otherwise
    /// retires it. An answer nobody waits on could only land over the list
    /// the open kept instead: one from where the provider no longer points,
    /// or one given up on.
    pub fn await_or_retire(&mut self, provider: &str, source: &str, now: i64) -> bool {
        let waits = self.awaits(provider, source, now);
        if !waits {
            self.retire(provider);
        }
        waits
    }

    /// A fetch of `provider`'s list landed with `ticket`: the source it read
    /// when the ticket is the provider's live one, which closes — `None` for
    /// a superseded, retired or unknown ticket, whose answer nobody wants.
    pub fn settle(&mut self, provider: &str, ticket: u64) -> Option<String> {
        if self.live.get(provider)?.ticket != ticket {
            return None;
        }
        self.live.remove(provider).map(|fetch| fetch.source)
    }

    /// Retire the fetch out for `provider`, if any: it is cancelled, and
    /// whatever it brings is neither kept nor shown.
    pub fn retire(&mut self, provider: &str) {
        if let Some(fetch) = self.live.remove(provider) {
            fetch.cancel.cancel();
        }
    }

    /// Cancel every fetch still out — the app is quitting.
    pub fn cancel_all(&mut self) {
        for fetch in std::mem::take(&mut self.live).into_values() {
            fetch.cancel.cancel();
        }
    }
}

// ---------------------------------------------------------------------------
// The file's shape
// ---------------------------------------------------------------------------

/// The file as written: the listing's facts over its rows.
#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    app_version: &'a str,
    source: &'a str,
    fetched_at: i64,
    models: Rows<'a>,
}

/// The rows, serialized one at a time from the listing itself.
struct Rows<'a>(&'a [ModelEntry]);

impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(RowOut::of))
    }
}

/// One row as written: what [`ModelEntry`] carries, less its provider, each
/// fact left out when the record had none to say.
#[derive(Serialize)]
struct RowOut<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<u64>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    service_tiers: &'a [ServiceTier],
}

impl<'a> RowOut<'a> {
    fn of(entry: &'a ModelEntry) -> Self {
        Self {
            id: &entry.id,
            name: (entry.display_name != entry.id).then_some(entry.display_name.as_str()),
            reasoning: entry.reasoning.as_ref().map(ReasoningOut::of),
            vision: entry.vision,
            context: entry.context,
            service_tiers: &entry.service_tiers,
        }
    }
}

/// A row's reasoning ladder as plain labels — `config.json`'s thinking
/// blob's spelling.
#[derive(Serialize)]
struct ReasoningOut {
    efforts: Vec<&'static str>,
    can_disable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_effort: Option<&'static str>,
}

impl ReasoningOut {
    fn of(support: &ReasoningSupport) -> Self {
        Self {
            efforts: support.efforts.iter().map(|e| e.as_str()).collect(),
            can_disable: support.can_disable,
            default_effort: support.default_effort.map(ReasoningEffort::as_str),
        }
    }
}

/// The file as read.
#[derive(Deserialize)]
struct FileIn {
    version: u32,
    #[serde(default)]
    app_version: String,
    source: String,
    fetched_at: i64,
    models: Vec<RowIn>,
}

/// One row as read.
#[derive(Deserialize)]
struct RowIn {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reasoning: Option<ReasoningIn>,
    #[serde(default)]
    vision: Option<bool>,
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    service_tiers: Vec<ServiceTier>,
}

impl RowIn {
    fn into_entry(self, provider: &str) -> ModelEntry {
        ModelEntry {
            display_name: self.name.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            provider: provider.to_string(),
            reasoning: self.reasoning.map(ReasoningIn::into_support),
            vision: self.vision,
            context: self.context,
            service_tiers: self.service_tiers,
        }
    }
}

/// A row's reasoning ladder as read.
#[derive(Deserialize)]
struct ReasoningIn {
    #[serde(default)]
    efforts: Vec<String>,
    #[serde(default = "can_disable_by_default")]
    can_disable: bool,
    #[serde(default)]
    default_effort: Option<String>,
}

/// What a ladder that does not say reads as — the thinking blob's default.
const fn can_disable_by_default() -> bool {
    true
}

impl ReasoningIn {
    fn into_support(self) -> ReasoningSupport {
        ReasoningSupport {
            efforts: self
                .efforts
                .iter()
                .filter_map(|label| ReasoningEffort::parse(label))
                .collect(),
            can_disable: self.can_disable,
            default_effort: self
                .default_effort
                .as_deref()
                .and_then(ReasoningEffort::parse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::{AuthScheme, WireApi};

    const VERSION: &str = "1.2.3";
    const NOW: i64 = 1_763_100_000;

    fn entry(id: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            provider: "openrouter".to_string(),
            display_name: name.to_string(),
            reasoning: None,
            vision: None,
            context: None,
            service_tiers: Vec::new(),
        }
    }

    /// A row carrying every fact a listing can say.
    fn rich_entry() -> ModelEntry {
        ModelEntry {
            reasoning: Some(ReasoningSupport {
                efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                can_disable: false,
                default_effort: Some(ReasoningEffort::Medium),
            }),
            vision: Some(true),
            context: Some(400_000),
            service_tiers: vec![ServiceTier::new(
                "priority",
                "Fast",
                "1.5x speed, increased usage",
            )],
            ..entry("openai/gpt-5.5", "OpenAI: GPT-5.5")
        }
    }

    fn listing(models: Vec<ModelEntry>) -> CachedListing {
        CachedListing {
            fetched_at: NOW,
            app_version: VERSION.to_string(),
            source: "sha256:source".to_string(),
            models,
        }
    }

    fn written(listing: &CachedListing) -> String {
        let mut out = Vec::new();
        listing.write_to(&mut out).expect("a Vec takes every byte");
        String::from_utf8(out).expect("JSON is UTF-8")
    }

    // --- the file ---

    #[test]
    fn a_listing_reads_back_as_it_was_written() {
        let kept = listing(vec![
            rich_entry(),
            entry("meta/llama", "Meta: Llama"),
            // An on/off reasoner: an empty ladder that can be turned off.
            ModelEntry {
                reasoning: Some(ReasoningSupport {
                    efforts: Vec::new(),
                    can_disable: true,
                    default_effort: None,
                }),
                vision: Some(false),
                ..entry("venice/uncensored", "Venice Uncensored")
            },
        ]);
        let text = written(&kept);
        let read = CachedListing::read_from(text.as_bytes(), "openrouter");
        assert_eq!(read, Some(kept), "{text}");
    }

    #[test]
    fn rows_are_tagged_with_the_provider_the_file_belongs_to() {
        // The file is the provider's own, so its rows carry no provider —
        // the reader names it.
        let text = written(&listing(vec![entry("a", "A")]));
        assert!(!text.contains("openrouter"), "{text}");
        let read = CachedListing::read_from(text.as_bytes(), "acme").expect("it parses");
        assert_eq!(read.models[0].provider, "acme");
    }

    #[test]
    fn a_name_that_only_repeats_the_id_is_not_written() {
        let text = written(&listing(vec![entry("gpt-4o", "gpt-4o")]));
        assert!(!text.contains("\"name\""), "{text}");
        let read = CachedListing::read_from(text.as_bytes(), "openrouter").expect("it parses");
        assert_eq!(read.models[0].display_name, "gpt-4o");
    }

    #[test]
    fn a_file_of_another_version_is_no_cache() {
        let text =
            written(&listing(vec![entry("a", "A")])).replacen("\"version\":1", "\"version\":2", 1);
        assert!(text.contains("\"version\":2"), "{text}");
        assert_eq!(
            CachedListing::read_from(text.as_bytes(), "openrouter"),
            None
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_no_cache() {
        let whole = written(&listing(vec![entry("a", "A")]));
        // Cut short (a write another instance had not finished), not JSON at
        // all, and JSON of another shape.
        for text in [
            &whole[..whole.len() / 2],
            "not json",
            "{\"models\": []}",
            "",
        ] {
            assert_eq!(
                CachedListing::read_from(text.as_bytes(), "openrouter"),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn an_effort_label_this_version_does_not_know_is_skipped() {
        let text = format!(
            r#"{{"version":{CACHE_VERSION},"app_version":"{VERSION}","source":"s","fetched_at":{NOW},
            "models":[{{"id":"m","reasoning":{{"efforts":["low","warp","high"],"can_disable":false,"default_effort":"warp"}}}}]}}"#
        );
        let read = CachedListing::read_from(text.as_bytes(), "p").expect("it parses");
        let support = read.models[0]
            .reasoning
            .clone()
            .expect("the ladder survives");
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
        assert!(!support.can_disable);
        assert_eq!(support.default_effort, None);
    }

    // --- what a kept listing is worth ---

    #[test]
    fn a_listing_younger_than_the_ttl_is_fresh() {
        let mut kept = listing(Vec::new());
        for age in [0, 59, MODEL_CACHE_TTL_SECS - 1] {
            kept.fetched_at = NOW - age;
            assert_eq!(
                freshness(&kept, "sha256:source", VERSION, NOW, false),
                Freshness::Fresh,
                "{age}s old"
            );
        }
    }

    #[test]
    fn a_listing_the_ttl_old_or_older_is_stale() {
        let mut kept = listing(Vec::new());
        for age in [MODEL_CACHE_TTL_SECS, MODEL_CACHE_TTL_SECS * 30] {
            kept.fetched_at = NOW - age;
            assert_eq!(
                freshness(&kept, "sha256:source", VERSION, NOW, false),
                Freshness::Stale,
                "{age}s old"
            );
        }
    }

    #[test]
    fn a_listing_dated_after_now_is_stale() {
        // A clock that moved back, or a file another machine's clock dated.
        let mut kept = listing(Vec::new());
        kept.fetched_at = NOW + 60;
        assert_eq!(
            freshness(&kept, "sha256:source", VERSION, NOW, false),
            Freshness::Stale
        );
    }

    #[test]
    fn a_listing_another_version_fetched_is_stale() {
        let kept = listing(Vec::new());
        assert_eq!(
            freshness(&kept, "sha256:source", "1.2.4", NOW, false),
            Freshness::Stale
        );
    }

    #[test]
    fn a_self_hosted_servers_listing_is_always_asked_for_again() {
        let kept = listing(Vec::new());
        assert_eq!(
            freshness(&kept, "sha256:source", VERSION, NOW, true),
            Freshness::Stale
        );
    }

    #[test]
    fn a_listing_fetched_from_somewhere_else_is_unusable() {
        let kept = listing(Vec::new());
        // Whatever its age and version: it is another server's list.
        assert_eq!(
            freshness(&kept, "sha256:elsewhere", VERSION, NOW, false),
            Freshness::Unusable
        );
        assert_eq!(
            freshness(&kept, "sha256:elsewhere", VERSION, NOW, true),
            Freshness::Unusable
        );
    }

    #[test]
    fn a_listing_dated_at_the_edge_of_time_is_stale_not_a_panic() {
        // A hand-edited file can hold anything an i64 can; the age is
        // checked arithmetic, never a debug-build overflow.
        let mut kept = listing(Vec::new());
        for at in [i64::MIN, i64::MAX] {
            kept.fetched_at = at;
            assert_eq!(
                freshness(&kept, "sha256:source", VERSION, NOW, false),
                Freshness::Stale,
                "{at}"
            );
        }
    }

    // --- the fetches still out ---

    const SOURCE: &str = "sha256:source";

    #[test]
    fn a_fetch_answers_its_live_ticket_once() {
        let mut ledger = FetchLedger::default();
        let (ticket, cancel) = ledger.issue("openrouter", SOURCE, NOW);
        assert!(!cancel.is_cancelled());
        assert_eq!(ledger.settle("openrouter", ticket).as_deref(), Some(SOURCE));
        assert_eq!(
            ledger.settle("openrouter", ticket),
            None,
            "a ticket closes when it settles"
        );
    }

    #[test]
    fn a_newer_fetch_supersedes_and_cancels_the_one_out() {
        let mut ledger = FetchLedger::default();
        let (old, old_cancel) = ledger.issue("openrouter", SOURCE, NOW);
        let (new, new_cancel) = ledger.issue("openrouter", SOURCE, NOW + 1);
        assert_ne!(old, new);
        assert!(old_cancel.is_cancelled(), "the superseded fetch stops");
        assert!(!new_cancel.is_cancelled());
        assert_eq!(
            ledger.settle("openrouter", old),
            None,
            "its answer is nobody's"
        );
        assert_eq!(ledger.settle("openrouter", new).as_deref(), Some(SOURCE));
    }

    #[test]
    fn a_retired_fetch_is_cancelled_and_its_answer_dropped() {
        // A `/login` save retires the fetch out for every provider the saved
        // variable configures: what it brings was read with the old key.
        let mut ledger = FetchLedger::default();
        let (ticket, cancel) = ledger.issue("openrouter", SOURCE, NOW);
        ledger.retire("openrouter");
        assert!(cancel.is_cancelled());
        assert_eq!(ledger.settle("openrouter", ticket), None);
        ledger.retire("never_issued");
    }

    #[test]
    fn tickets_belong_to_their_provider() {
        let mut ledger = FetchLedger::default();
        let (a, a_cancel) = ledger.issue("openrouter", SOURCE, NOW);
        let (b, _) = ledger.issue("chatgpt_codex", SOURCE, NOW);
        assert!(
            !a_cancel.is_cancelled(),
            "another provider's fetch supersedes nothing"
        );
        assert_eq!(ledger.settle("chatgpt_codex", a), None);
        assert!(ledger.settle("openrouter", a).is_some());
        assert!(ledger.settle("chatgpt_codex", b).is_some());
    }

    #[test]
    fn an_open_waits_on_a_young_fetch_from_the_same_source() {
        let mut ledger = FetchLedger::default();
        ledger.issue("openrouter", SOURCE, NOW);
        assert!(ledger.awaits("openrouter", SOURCE, NOW));
        assert!(ledger.awaits("openrouter", SOURCE, NOW + FETCH_GIVE_UP_SECS - 1));
        assert!(
            !ledger.awaits("openrouter", "sha256:elsewhere", NOW),
            "another server's fetch is not this list"
        );
        assert!(!ledger.awaits("chatgpt_codex", SOURCE, NOW));
    }

    #[test]
    fn a_fetch_out_too_long_is_given_up_on() {
        // A worker that never answers must not hold its provider for the
        // rest of the session: past the limit an open asks again.
        let mut ledger = FetchLedger::default();
        ledger.issue("openrouter", SOURCE, NOW);
        assert!(!ledger.awaits("openrouter", SOURCE, NOW + FETCH_GIVE_UP_SECS));
        assert!(
            !ledger.awaits("openrouter", SOURCE, NOW - 5),
            "a clock moved back cannot vouch for it either"
        );
    }

    #[test]
    fn an_open_retires_a_fetch_it_will_not_wait_on() {
        // An open that does not wait on the fetch out may keep its list
        // instead (a fresh one), and an answer nobody waits on could then
        // only land over it: a list from where the provider no longer
        // points, or one given up on.
        let mut ledger = FetchLedger::default();
        let (moved, moved_cancel) = ledger.issue("openrouter", "sha256:elsewhere", NOW);
        assert!(!ledger.await_or_retire("openrouter", SOURCE, NOW));
        assert!(moved_cancel.is_cancelled());
        assert_eq!(ledger.settle("openrouter", moved), None);

        let (old, old_cancel) = ledger.issue("chatgpt_codex", SOURCE, NOW);
        assert!(!ledger.await_or_retire("chatgpt_codex", SOURCE, NOW + FETCH_GIVE_UP_SECS));
        assert!(old_cancel.is_cancelled());
        assert_eq!(ledger.settle("chatgpt_codex", old), None);
    }

    #[test]
    fn an_open_keeps_the_fetch_it_waits_on() {
        let mut ledger = FetchLedger::default();
        let (ticket, cancel) = ledger.issue("openrouter", SOURCE, NOW);
        assert!(ledger.await_or_retire("openrouter", SOURCE, NOW + 1));
        assert!(!cancel.is_cancelled());
        assert_eq!(
            ledger.settle("openrouter", ticket),
            Some(SOURCE.to_string())
        );
        assert!(
            !ledger.await_or_retire("openrouter", SOURCE, NOW + 2),
            "nothing is out once it settled"
        );
    }

    #[test]
    fn quitting_cancels_every_fetch_out() {
        let mut ledger = FetchLedger::default();
        let (_, a) = ledger.issue("openrouter", SOURCE, NOW);
        let (_, b) = ledger.issue("chatgpt_codex", SOURCE, NOW);
        ledger.cancel_all();
        assert!(a.is_cancelled() && b.is_cancelled());
    }

    // --- what an open does about each provider ---

    #[test]
    fn a_fresh_list_is_kept_without_a_fetch() {
        assert_eq!(plan(Some(Freshness::Fresh), false, false), FetchPlan::Keep);
    }

    #[test]
    fn anything_but_a_fresh_list_starts_a_fetch() {
        for kept in [None, Some(Freshness::Stale), Some(Freshness::Unusable)] {
            assert_eq!(plan(kept, false, false), FetchPlan::Start, "{kept:?}");
        }
        assert_eq!(
            plan(Some(Freshness::Fresh), false, true),
            FetchPlan::Start,
            "Ctrl+R asks whatever the cache says"
        );
    }

    #[test]
    fn a_fetch_already_out_is_waited_on_rather_than_asked_again() {
        for kept in [None, Some(Freshness::Fresh), Some(Freshness::Stale)] {
            for force in [false, true] {
                assert_eq!(
                    plan(kept, true, force),
                    FetchPlan::Await,
                    "{kept:?} force={force}"
                );
            }
        }
    }

    // --- what a listing is fetched from ---

    fn config() -> ModelConfig {
        let mut cfg = ModelConfig::fallback();
        cfg.provider_id = "openrouter".to_string();
        cfg.api_base = "https://openrouter.ai/api/v1".to_string();
        cfg.api_model_base = "https://openrouter.ai/api/v1".to_string();
        cfg
    }

    #[test]
    fn the_source_is_a_fingerprint_not_the_address() {
        let mut cfg = config();
        cfg.api_base = "https://user:hunter2@proxy.example/v1".to_string();
        let source = listing_source(&cfg);
        assert!(source.starts_with("sha256:"), "{source}");
        assert!(!source.contains("hunter2"), "{source}");
    }

    #[test]
    fn the_source_ignores_the_model_the_key_and_a_trailing_slash() {
        let base = listing_source(&config());
        let mut other = config();
        other.model = "anthropic/claude-sonnet-5".to_string();
        other.api_key = Some("sk-or-secret".to_string());
        other.api_model_base.push('/');
        other.api_base.push('/');
        assert_eq!(listing_source(&other), base);
    }

    #[test]
    fn the_source_moves_with_anything_the_listing_is_fetched_through() {
        let base = listing_source(&config());
        let changes: [fn(&mut ModelConfig); 5] = [
            |c| c.api_model_base = "https://elsewhere.example/v1".to_string(),
            |c| c.api_base = "https://elsewhere.example/v1".to_string(),
            |c| c.wire_api = WireApi::Responses,
            |c| c.auth = AuthScheme::GithubCopilot,
            |c| {
                c.extra_headers
                    .push(("OpenAI-Organization".to_string(), "org-1".to_string()));
            },
        ];
        for (i, change) in changes.iter().enumerate() {
            let mut cfg = config();
            change(&mut cfg);
            assert_ne!(listing_source(&cfg), base, "change {i} kept the source");
        }
    }

    #[test]
    fn a_server_the_user_runs_is_self_hosted_wherever_it_listens() {
        // What makes a list change on every `ollama pull` is the kind of
        // provider — a server the user runs, needing no key — not its
        // address: one on another machine changes just the same.
        for base in ["http://localhost:11434", "http://192.168.1.20:11434"] {
            let mut cfg = config();
            cfg.auth = AuthScheme::OptionalKey;
            cfg.api_base = base.to_string();
            cfg.api_model_base = base.to_string();
            assert!(is_self_hosted(&cfg), "{base}");
        }
    }

    #[test]
    fn a_keyed_gateway_on_this_machine_is_a_remote_catalog() {
        // A key-taking proxy on localhost (a LiteLLM, an OmniRoute) lists
        // what it routes to, which changes no faster than any other catalog.
        let mut gateway = config();
        gateway.api_base = "http://127.0.0.1:4000/v1".to_string();
        gateway.api_model_base = gateway.api_base.clone();
        assert!(!is_self_hosted(&gateway));
        assert!(!is_self_hosted(&config()));
    }

    // --- the file's name ---

    #[test]
    fn a_plain_provider_id_names_its_file() {
        for id in ["openrouter", "chatgpt_codex", "a0-venice", "Acme2"] {
            assert_eq!(file_name(id), format!("{id}.json"));
        }
    }

    #[test]
    fn an_id_that_could_name_a_path_is_fingerprinted() {
        for id in [
            "team/proxy",
            "..",
            "../../etc/passwd",
            "my provider",
            "",
            "ünïcode",
        ] {
            let name = file_name(id);
            let stem = name.strip_suffix(".json").expect("a .json file");
            assert_eq!(stem.len(), 64, "{id:?} → {name}");
            assert!(
                stem.chars().all(|c| c.is_ascii_hexdigit()),
                "{id:?} → {name}"
            );
        }
        assert_ne!(file_name("team/proxy"), file_name("team/other"));
    }
}

//! Live `/usage` verification, one test per subscription sign-in that is
//! read over the network (`docs/usage.md` — a Console sign-in is answered
//! without a request, so it has nothing to verify live): the provider's
//! config resolved through `ProvidersFile::model_config` exactly as the
//! boundary resolves it, then `llm::usage::fetch_usage` against the real
//! endpoint. Each test prints the report and asserts what every real answer
//! carries.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run on
//! purpose with real credentials, never committed — read from the
//! environment:
//!
//! ```sh
//! GITHUB_COPILOT_TOKEN=ghu_…   cargo test --test live_usage -- --ignored --nocapture live_copilot
//! CHATGPT_CODEX_REFRESH_TOKEN=rt.1.… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_usage -- --ignored --nocapture live_chatgpt
//! CLINE_ACCOUNT_REFRESH_TOKEN=… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_usage -- --ignored --nocapture live_clinepass
//! ```
//!
//! The two sign-ins whose refresh token **rotates** refuse to run without
//! `ALTER_ZERO_LIVE_TOKEN_STORE`, the `live_caching` rule: the mint may
//! retire the token it was given, and the rotated one is written back only
//! to a store path. After such a run, the value under that file's
//! `*_REFRESH_TOKEN` key is the one to keep.

use alter_zero::llm::usage::{MeterValue, UsageReport, fetch_usage};
use alter_zero::llm::{ModelConfig, ProvidersFile, Selection};
use alter_zero::stream::CancelToken;

/// The provider's config resolved the way `tui::config::model_config_for`
/// resolves it, through the built-in `providers.toml`.
fn config(provider_id: &str, key: String) -> ModelConfig {
    ProvidersFile::builtin()
        .model_config(&Selection {
            provider_id: provider_id.to_string(),
            model: String::new(),
            api_key: Some(key),
            temperature: None,
            thinking: None,
            vision: None,
            context: None,
            api_base: None,
            cache_key: None,
            service_tier: None,
        })
        .unwrap_or_else(|| panic!("{provider_id} is a built-in provider"))
}

/// The credential under `var`, read the way the boundary reads it — first
/// the process environment, then the token store a rotation writes to, so a
/// second run picks up the rotated token rather than the retired one.
fn credential(var: &str) -> String {
    if let Some(store) = std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
        && let Ok(text) = std::fs::read_to_string(store)
        && let Some(value) = text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{var}=")))
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    {
        return value;
    }
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to run this live test"))
}

fn token_store() -> std::path::PathBuf {
    std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
        .map(std::path::PathBuf::from)
        .expect(
            "set ALTER_ZERO_LIVE_TOKEN_STORE to a file the rotated refresh token can be written to",
        )
}

/// Seed the store with the environment's token when it holds none yet, so the
/// write-back has a chain to replace.
fn seed_store(var: &str) {
    let store = token_store();
    let text = std::fs::read_to_string(&store).unwrap_or_default();
    if !text
        .lines()
        .any(|line| line.starts_with(&format!("{var}=")))
    {
        let token =
            std::env::var(var).unwrap_or_else(|_| panic!("set {var} to run this live test"));
        let mut seeded = text;
        if !seeded.is_empty() && !seeded.ends_with('\n') {
            seeded.push('\n');
        }
        seeded.push_str(&format!("{var}={token}\n"));
        std::fs::write(&store, seeded).expect("write the token store");
    }
}

fn fetch(provider_id: &str, var: &str) -> UsageReport {
    let report = fetch_usage(&config(provider_id, credential(var)), &CancelToken::new())
        .unwrap_or_else(|e| panic!("{provider_id}: {e}"));
    println!("{provider_id}: {report:#?}");
    report
}

#[test]
#[ignore = "hits the network; needs GITHUB_COPILOT_TOKEN"]
fn live_copilot_reports_its_quota_snapshots() {
    let report = fetch("github_copilot", "GITHUB_COPILOT_TOKEN");
    assert!(report.plan.is_some(), "a Copilot seat names its plan");
    assert!(!report.meters.is_empty(), "every seat has some quota");
}

#[test]
#[ignore = "hits the network; needs CHATGPT_CODEX_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE"]
fn live_chatgpt_reports_its_plan_and_windows() {
    seed_store("CHATGPT_CODEX_REFRESH_TOKEN");
    alter_zero::llm::chatgpt::set_store_path(token_store());
    let report = fetch("chatgpt_codex", "CHATGPT_CODEX_REFRESH_TOKEN");
    assert!(report.plan.is_some(), "the answer names the plan");
    assert!(
        report
            .meters
            .iter()
            .any(|meter| matches!(meter.value, MeterValue::Limit(_))),
        "a ChatGPT seat has at least one rolling window"
    );
}

#[test]
#[ignore = "hits the network; needs CLINE_ACCOUNT_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE"]
fn live_clinepass_reports_its_windows() {
    seed_store("CLINE_ACCOUNT_REFRESH_TOKEN");
    alter_zero::llm::cline::set_store_path(token_store());
    let report = fetch("cline_pass", "CLINE_ACCOUNT_REFRESH_TOKEN");
    let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
    assert!(
        report.plan.is_none() || labels.contains(&"5-hour limit"),
        "a subscribed account reports its 5-hour window: {labels:?}"
    );
}

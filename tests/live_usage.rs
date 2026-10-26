//! Live checks of the `/usage` page's reads (`docs/usage.md`): each
//! subscription's own usage endpoint, through `llm::plan_usage::fetch` with
//! the provider's config resolved the way the boundary resolves it — the
//! built-in providers table, the stored credential — so a run proves the
//! request the page makes and the parse of what the provider answers today.
//! Each test prints the parsed answer, so a drifting backend is visible
//! before it is a bug.
//!
//! All `#[ignore]`d so `cargo test` stays offline and deterministic. Run on
//! purpose with real credentials, never committed — read from the
//! environment:
//!
//! ```sh
//! CHATGPT_CODEX_REFRESH_TOKEN=rt.1.… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_usage -- --ignored --nocapture live_chatgpt
//! CLINE_ACCOUNT_REFRESH_TOKEN=… ALTER_ZERO_LIVE_TOKEN_STORE=/tmp/live.env \
//!                              cargo test --test live_usage -- --ignored --nocapture live_clinepass
//! GITHUB_COPILOT_TOKEN=ghu_…   cargo test --test live_usage -- --ignored --nocapture live_copilot
//! ```
//!
//! The sign-ins whose refresh token **rotates** (ChatGPT, Cline —
//! `docs/chatgpt.md`, `docs/cline.md`) refuse to run without
//! `ALTER_ZERO_LIVE_TOKEN_STORE`: the mint may retire the token it was given,
//! and the rotated one is written back only to a store path, so a run without
//! one would strand the caller with a token the provider no longer honours.
//! After such a run, the value under that file's `*_REFRESH_TOKEN` key is the
//! one to keep. Copilot's GitHub token does not rotate, and its usage read
//! spends no exchange. The Anthropic Console sign-in makes no request at all.

use alter_zero::llm::plan_usage::{Allowance, PlanUsage, fetch};
use alter_zero::llm::{ModelConfig, ProvidersFile, Selection};
use alter_zero::stream::CancelToken;

/// A built-in provider's config with `key` as its stored credential.
fn config(provider_id: &str, key: String) -> ModelConfig {
    ProvidersFile::builtin()
        .model_config(&Selection {
            provider_id: provider_id.to_string(),
            model: "usage-probe".to_string(),
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

/// The credential a test runs on. Panics with a clear message when it is
/// unset — these tests only ever run on purpose.
fn credential(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to run this live test"))
}

/// The store a rotated refresh token is written back to. Required for the
/// rotating sign-ins — see the module docs.
fn token_store() -> std::path::PathBuf {
    std::env::var_os("ALTER_ZERO_LIVE_TOKEN_STORE")
        .map(std::path::PathBuf::from)
        .expect(
            "set ALTER_ZERO_LIVE_TOKEN_STORE to a file the rotated refresh token can be written to",
        )
}

/// Print what the page would show, row by row.
fn show(provider: &str, usage: &PlanUsage) {
    println!(
        "{provider}: plan {:?}, account {:?}",
        usage.plan, usage.account
    );
    for limit in &usage.limits {
        println!(
            "  {:?} {} → {:?}",
            limit.scope, limit.label, limit.allowance
        );
    }
    for balance in &usage.balances {
        println!(
            "  {} → {} {:?}",
            balance.label, balance.value, balance.detail
        );
    }
    for note in &usage.notes {
        println!("  note (warning: {}): {}", note.warning, note.text);
    }
}

#[test]
#[ignore = "hits the network; needs CHATGPT_CODEX_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE"]
fn live_chatgpt_usage_reads_the_plan_and_its_windows() {
    alter_zero::llm::chatgpt::set_store_path(token_store());
    let cfg = config("chatgpt_codex", credential("CHATGPT_CODEX_REFRESH_TOKEN"));
    let usage = fetch(&cfg, &CancelToken::new()).expect("ChatGPT's wham/usage");
    show("ChatGPT Codex", &usage);
    assert!(usage.plan.is_some(), "a signed-in seat names its plan");
    assert!(
        usage
            .limits
            .iter()
            .any(|limit| matches!(limit.allowance, Allowance::Metered(_))),
        "a ChatGPT seat is metered against at least one window"
    );
}

#[test]
#[ignore = "hits the network; needs CLINE_ACCOUNT_REFRESH_TOKEN + ALTER_ZERO_LIVE_TOKEN_STORE"]
fn live_clinepass_usage_reads_its_windows_or_says_there_is_no_plan() {
    alter_zero::llm::cline::set_store_path(token_store());
    let cfg = config("cline_pass", credential("CLINE_ACCOUNT_REFRESH_TOKEN"));
    let usage = fetch(&cfg, &CancelToken::new()).expect("ClinePass's usage limits");
    show("ClinePass", &usage);
    assert!(
        !usage.limits.is_empty() || !usage.notes.is_empty(),
        "a subscribed account has windows; an unsubscribed one, the sentence"
    );
}

#[test]
#[ignore = "hits the network; needs GITHUB_COPILOT_TOKEN"]
fn live_copilot_usage_reads_the_entitlements() {
    let cfg = config("github_copilot", credential("GITHUB_COPILOT_TOKEN"));
    let usage = fetch(&cfg, &CancelToken::new()).expect("GitHub's copilot_internal/user");
    show("GitHub Copilot", &usage);
    assert!(
        usage.plan.is_some() || !usage.notes.is_empty(),
        "a Copilot account names its plan, or says it has none"
    );
}

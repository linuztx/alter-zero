//! Memory probe for the `/model` picker's `/v1/models` fetch + parse (NOT part
//! of the app).
//!
//! Contrasts the OLD parse shape (the whole response materialized as a
//! `serde_json::Value` tree — one `Value` record per model, held all at once)
//! against the NEW per-record parse in [`alter_zero::llm::models::parse_models`]
//! (one record's tree alive at a time). The tree is what made `/model` grow
//! resident memory by megabytes: an aggregator list is hundreds of records of
//! ~1.7 KB JSON each, and a `Value` costs several times its JSON text in small
//! heap blocks — blocks glibc keeps in the arena after the parse drops them,
//! so the spike *stays* in RSS. What the picker actually keeps
//! ([`alter_zero::llm::ModelEntry`]) is a few hundred bytes per model.
//!
//! Part B drives a real OpenRouter fetch when `OPENROUTER_API_KEY` is set.
//!
//! Run:  cargo run --release --example mem_probe [captured-models.json]
//!
//! Linux-only reporting (reads `/proc/self/status`); elsewhere it prints n/a.

use std::fmt::Write as _;

use alter_zero::llm::models::parse_models;

/// A `/proc/self/status` field in KiB (`VmRSS` = resident now, `VmHWM` = the
/// resident high-water mark — what a transient spike leaves behind).
fn vm_kb(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn mb(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

fn report(label: &str, rss_before: u64, hwm_before: u64) {
    let rss = vm_kb("VmRSS:").unwrap_or(0);
    let hwm = vm_kb("VmHWM:").unwrap_or(0);
    println!(
        "{label:<44} RSS {:>6.1} MB (+{:>5.1})   peak {:>6.1} MB (+{:>5.1})",
        mb(rss),
        mb(rss.saturating_sub(rss_before)),
        mb(hwm),
        mb(hwm.saturating_sub(hwm_before)),
    );
}

/// A synthetic OpenRouter-shaped `/v1/models` body: `n` records with the real
/// list's heavy fields (description, pricing, architecture, top_provider,
/// supported_parameters), ~1.7 KB of JSON per record like the live catalog.
fn gen_body(n: usize) -> String {
    let mut body = String::from(r#"{"object":"list","data":["#);
    for i in 0..n {
        if i > 0 {
            body.push(',');
        }
        let desc = format!(
            "Model {i} is a large language model tuned for chat and tool use. \
             It supports long-context reasoning over code and prose, function \
             calling, and structured outputs. "
        )
        .repeat(4);
        let _ = write!(
            body,
            r#"{{"id":"vendor-{i}/model-{i}","canonical_slug":"vendor-{i}/model-{i}","hugging_face_id":"vendor-{i}/model-{i}","name":"Vendor {i}: Model {i}","created":1700000000,"description":{desc:?},"context_length":131072,"architecture":{{"modality":"text->text","input_modalities":["text"],"output_modalities":["text"],"tokenizer":"Other","instruct_type":null}},"pricing":{{"prompt":"0.0000005","completion":"0.0000015","request":"0","image":"0","web_search":"0","internal_reasoning":"0"}},"top_provider":{{"context_length":131072,"max_completion_tokens":16384,"is_moderated":false}},"per_request_limits":null,"supported_parameters":["max_tokens","temperature","top_p","tools","tool_choice","stop","frequency_penalty","presence_penalty","seed","response_format","structured_outputs"],"default_parameters":{{"temperature":null,"top_p":null,"frequency_penalty":null}}}}"#
        );
    }
    body.push_str("]}");
    body
}

fn part_a(body: &str) {
    println!(
        "=== Part A: parse a {} KB models body ===\n",
        body.len() / 1024
    );
    let rss0 = vm_kb("VmRSS:").unwrap_or(0);
    let hwm0 = vm_kb("VmHWM:").unwrap_or(0);
    if rss0 == 0 {
        println!("(/proc/self/status not readable — n/a on this platform)");
        return;
    }
    report("baseline", rss0, hwm0);

    // NEW first (so the OLD arm's freed arena can't subsidise its numbers):
    // the shipped parse — one record's tree alive at a time.
    let entries = parse_models(body, "probe").expect("parse");
    report(
        &format!("NEW parse_models — {} entries held", entries.len()),
        rss0,
        hwm0,
    );
    let retained: usize = entries
        .iter()
        .map(|e| e.id.len() + e.provider.len() + e.display_name.len() + 64)
        .sum();
    println!(
        "{:<44} ~{} KB of ModelEntry rows\n",
        "  (what the picker keeps)",
        retained / 1024
    );
    drop(entries);

    // OLD shape: the whole body as one serde_json::Value tree, every record
    // materialized at once — what `parse_models` used to hold internally.
    let tree: serde_json::Value = serde_json::from_str(body).expect("parse");
    report("OLD whole-list Value tree held", rss0, hwm0);
    drop(tree);
    report("OLD tree dropped (arena keeps the spike)", rss0, hwm0);
}

fn part_b() {
    use alter_zero::llm::ModelConfig;
    use alter_zero::llm::models::fetch_models;
    use alter_zero::stream::CancelToken;

    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        println!("\n=== Part B skipped (set OPENROUTER_API_KEY to run the live fetch) ===");
        return;
    };
    println!("\n=== Part B: live OpenRouter /v1/models fetch ===\n");
    let mut cfg = ModelConfig::fallback();
    cfg.provider_id = "openrouter".into();
    cfg.provider_name = "OpenRouter".into();
    cfg.api_base = "https://openrouter.ai/api/v1".into();
    cfg.api_model_base = "https://openrouter.ai/api/v1".into();
    cfg.api_key = Some(key);

    let rss0 = vm_kb("VmRSS:").unwrap_or(0);
    let hwm0 = vm_kb("VmHWM:").unwrap_or(0);
    report("baseline", rss0, hwm0);
    match fetch_models(&cfg, &CancelToken::new()) {
        Ok(entries) => {
            report(
                &format!("fetched + parsed — {} entries held", entries.len()),
                rss0,
                hwm0,
            );
            drop(entries);
            report("entries dropped", rss0, hwm0);
        }
        Err(e) => println!("fetch failed: {e}"),
    }
}

fn main() {
    let body = match std::env::args().nth(1) {
        Some(path) => {
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("can't read {path}: {e}"))
        }
        None => gen_body(420),
    };
    part_a(&body);
    part_b();
}

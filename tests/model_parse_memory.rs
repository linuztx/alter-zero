//! The `/model` list parse must stay memory-proportional to what it **keeps**,
//! not to the body it reads (`docs/memory.md`).
//!
//! An aggregator's `/v1/models` body is mostly fields this app never shows:
//! OpenRouter's 417-record list is 669 KB of JSON, of which `benchmarks`,
//! `description`, `pricing`, `top_provider`, `links` and `default_parameters`
//! alone are ~380 KB. Parsing that into a `serde_json::Value` DOM cost
//! **~6.5 MB resident** — ten times the body — and glibc returns none of it to
//! the OS afterwards (thousands of small interleaved allocations fragment the
//! arena), so opening `/model` once moved the app from 16.3 MB to 25 MB and it
//! stayed there.
//!
//! This is an integration test rather than a unit test because the measurement
//! is process-wide: a lone `#[test]` in its own binary has nothing running
//! beside it to pollute the reading.

/// The process's resident set in bytes — `None` where `/proc` isn't available
/// (the probe then skips rather than asserting on noise).
fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: usize = field.trim().trim_end_matches(" kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// A realistic aggregator body: every record carries the handful of fields the
/// picker reads plus the bulk it never shows, in OpenRouter's proportions
/// (~1.6 KB per record, most of it ignored).
fn aggregator_body(records: usize) -> String {
    let desc = "A capable general-purpose model with a long context window. ".repeat(6);
    let bench = "Scores are self-reported by the vendor and not independently verified. ".repeat(3);
    let mut body = String::from(r#"{"data":["#);
    for i in 0..records {
        if i > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            r#"{{"id":"vendor-{i}/model-{i}","canonical_slug":"vendor-{i}/model-{i}-v3",
"name":"Vendor {i}: Model {i}","created":174000000,"context_length":262144,
"hugging_face_id":"vendor-{i}/model-{i}","description":"{desc}",
"architecture":{{"modality":"text+image->text","input_modalities":["text","image"],
"output_modalities":["text"],"tokenizer":"Other","instruct_type":null}},
"pricing":{{"prompt":"0.0000004","completion":"0.0000016","request":"0","image":"0",
"web_search":"0","internal_reasoning":"0","input_cache_read":"0.0000001"}},
"top_provider":{{"context_length":262144,"max_completion_tokens":64000,"is_moderated":false}},
"supported_parameters":["max_tokens","temperature","top_p","reasoning","tools","tool_choice",
"stop","frequency_penalty","presence_penalty","seed","logit_bias","response_format"],
"reasoning":{{"mandatory":false,"supported_efforts":["low","medium","high"],"default_effort":"medium"}},
"default_parameters":{{"temperature":1.0,"top_p":1.0,"frequency_penalty":0.0}},
"benchmarks":{{"mmlu":0.87,"gpqa":0.55,"humaneval":0.91,"math":0.74,"notes":"{bench}"}},
"links":{{"website":"https://example.invalid/v{i}","docs":"https://example.invalid/d/{i}"}},
"per_request_limits":null,"knowledge_cutoff":"2025-01-01"}}"#
        ));
    }
    body.push_str("]}");
    body
}

#[test]
fn parsing_a_model_list_does_not_retain_the_fields_it_ignores() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }

    // Warm the parse path (and the allocator's arena) so the measurement below
    // sees this body's cost rather than one-time first-touch page faults.
    let warmup = aggregator_body(20);
    drop(alter_zero::llm::models::parse_models(&warmup, "openrouter").expect("parses"));
    drop(warmup);

    let body = aggregator_body(417);
    let body_len = body.len();
    let before = rss_bytes().expect("rss");

    let models = alter_zero::llm::models::parse_models(&body, "openrouter").expect("parses");
    let after = rss_bytes().expect("rss");

    assert_eq!(models.len(), 417, "every record is listed");
    assert_eq!(models[0].context, Some(262_144), "the kept fields are read");
    drop(models);

    // The rows the picker keeps are ids, names and a three-rung effort ladder —
    // a few percent of the body. The `serde_json::Value` DOM this replaced grew
    // the process by ~10x the body and gave none of it back, which is what made
    // one `/model` open cost 8.7 MB for good. Budget one body's worth: ample
    // headroom over what the per-record parse costs, an order of magnitude
    // under the DOM.
    let growth = after.saturating_sub(before);
    assert!(
        growth < body_len,
        "parsing a {body_len}-byte model list grew the resident set by {growth} bytes \
         — the ignored fields are being materialised on the heap"
    );
}

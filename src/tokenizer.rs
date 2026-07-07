//! Accurate token counting for the status line's live tally.
//!
//! The wire protocol carries no usage numbers, so the `(… · {arrow} {n} tokens
//! · …)` count in the status indicator has to be computed app-side. This used to
//! be a `chars / 4` heuristic; it is now a real byte-pair tokenizer —
//! `tiktoken`'s **`o200k_base`** encoding (the GPT-4o / GPT-5 / o-series era
//! vocabulary). That is *exact* for current OpenAI models and a close
//! approximation for the Llama / Claude / Qwen / DeepSeek models served through
//! the configured providers (tiktoken has no exact vocabulary for those, so no
//! single encoding is perfect — `o200k_base` is the pragmatic modern default).
//!
//! The BPE ranks are **embedded in the binary** by `tiktoken-rs` (no network at
//! runtime), parsed once into a [`CoreBPE`] behind a [`OnceLock`] and reused for
//! every count. [`count`] is a pure, deterministic function of its input — the
//! same seam `app::count_tokens` funnels every tally through, so input, reply,
//! reasoning, and tool output all count the same way.

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

/// The process-wide `o200k_base` tokenizer, built on first use from the ranks
/// `tiktoken-rs` bundles into the binary (no I/O), then shared for every count.
fn bpe() -> &'static CoreBPE {
    static BPE: OnceLock<CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled o200k_base ranks load"))
}

/// The number of `o200k_base` tokens in `text`.
///
/// Uses `encode_ordinary`, which treats any `<|special|>` markers as ordinary
/// text — so counting arbitrary user, model, or tool text can never error.
#[must_use]
pub fn count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    bpe().encode_ordinary(text).len()
}

/// Force the one-time BPE build (~125 ms of rank parsing) now, so the first
/// real [`count`] doesn't pay it. The boundary calls this on a background thread
/// at startup (`main.rs`) — long before the first turn — keeping the 125 ms off
/// the interactive path. Idempotent: the [`OnceLock`] means later calls are free.
pub fn warm() {
    let _ = bpe();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_zero_tokens() {
        assert_eq!(count(""), 0);
    }

    #[test]
    fn counts_match_o200k_base_golden_values() {
        // Golden values from the `o200k_base` encoding (verified against
        // tiktoken directly) — this is what "accurate" means here.
        assert_eq!(count("hello world"), 2);
        assert_eq!(count("The quick brown fox jumps over the lazy dog."), 10);
    }

    #[test]
    fn a_single_short_word_is_one_token() {
        assert_eq!(count("a"), 1);
    }

    #[test]
    fn count_grows_with_length() {
        assert!(count("a much longer string of several words") > count("a"));
    }

    #[test]
    fn count_is_deterministic() {
        let text = "def main():\n    print('hi')";
        assert_eq!(count(text), count(text));
    }

    #[test]
    fn warm_is_idempotent_and_leaves_counting_intact() {
        warm();
        warm();
        assert_eq!(count("hello world"), 2);
    }
}

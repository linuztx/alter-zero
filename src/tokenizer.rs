//! Accurate token counting for the status line's live tally.
//!
//! The wire protocol carries no usage numbers between usage frames, so the
//! `(… · {arrow} {n} tokens · …)` count in the status indicator has to be
//! computed app-side. It is a real byte-pair tokenizer — `tiktoken`'s
//! **`o200k_base`** encoding (the GPT-4o / GPT-5 / o-series era vocabulary).
//! That is *exact* for current OpenAI models and a close approximation for the
//! Llama / Claude / Qwen / DeepSeek models served through the configured
//! providers (tiktoken has no exact vocabulary for those, so no single encoding
//! is perfect — `o200k_base` is the pragmatic modern default).
//!
//! The implementation is the crate's own **compact, count-only BPE** over the
//! embedded vocabulary (`assets/o200k_base.cbpe`, docs/tokenizer.md) rather
//! than `tiktoken-rs`'s `CoreBPE`. The two count identically — the differential
//! tests below prove it rank-by-rank over the whole vocabulary and
//! count-by-count over a broad corpus — but `CoreBPE` holds three separate heap
//! copies of all ~200k token byte strings (encoder map, decoder map, and a
//! dead-code sorted list) plus 128 thread-local clones of its compiled split
//! regex: **45.5 MB resident, measured**, which was most of the app's startup
//! RSS. This store keeps one copy of the token bytes — the embedded asset
//! itself — plus a rank-offset array and one open-addressed hash table of rank
//! numbers: **~4 MB**, a tenth of the reference, with no per-token heap
//! allocations at load. Counting never materialises a token vector either
//! (`count` sums per-piece token counts), so tallying a huge tool output does
//! not spike memory the way `encode_ordinary`'s returned `Vec` did.
//!
//! The BPE merge is a faithful port of tiktoken's `byte_pair_merge` (MIT, from
//! `tiktoken-rs`'s vendored copy of OpenAI's reference implementation),
//! including its two-tier small/large piece strategy and its leftmost-first
//! tie-breaking — merge order can change *which* tokens equal-rank merges
//! produce, and downstream merges see those tokens, so tie-breaking is part of
//! the contract, not a detail. [`count`] is a pure, deterministic function of
//! its input — the same seam `app::count_tokens` funnels every tally through,
//! so input, reply, reasoning, and tool output all count the same way.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::OnceLock;

/// The o200k_base vocabulary: for each rank in ascending order, `[len: u8]`
/// then the token's bytes. Generated from tiktoken's canonical
/// `o200k_base.tiktoken` by `scripts/gen-o200k-asset.py` (provenance and
/// format documented there); verified against `tiktoken-rs` rank-by-rank by
/// the tests below.
const VOCAB: &[u8] = include_bytes!("../assets/o200k_base.cbpe");

/// How many ranks the ordinary o200k_base vocabulary holds. Ranks are dense —
/// `0..VOCAB_RANKS`, in exactly the asset's order — which is what lets the
/// loader use the rank as its only token id (asserted at load).
const VOCAB_RANKS: usize = 199_998;

/// tiktoken's o200k_base split pattern, verbatim (the pre-tokenizer that cuts
/// text into pieces the BPE merge then works within). The `(?i:…)` groups and
/// the `(?!\S)` lookahead are why this needs `fancy-regex` — same engine
/// `tiktoken-rs` uses for it.
const SPLIT_PATTERN: &str = concat!(
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    "|",
    r"\p{N}{1,3}",
    "|",
    r" ?[^\s\p{L}\p{N}]+[\r\n/]*",
    "|",
    r"\s*[\r\n]+",
    "|",
    r"\s+(?!\S)",
    "|",
    r"\s+"
);

/// "No rank": empty hash-table slot, and the merge algorithm's +∞ sentinel
/// (every real rank is `< VOCAB_RANKS`, far below it). tiktoken's `Rank::MAX`.
const EMPTY: u32 = u32::MAX;

/// Pieces at least this long take the heap-based merge; shorter ones the
/// cache-friendly quadratic one. tiktoken's own threshold, kept so both
/// implementations walk the same code shape on the same inputs.
const LARGE_PIECE_MIN: usize = 100;

/// The token bytes of `rank`: a slice straight into the embedded [`VOCAB`]
/// (the byte before each token's start is its length prefix). No copies — the
/// asset is the only place token bytes live.
fn token_bytes(starts: &[u32], rank: usize) -> &'static [u8] {
    let start = starts[rank] as usize;
    let len = VOCAB[start - 1] as usize;
    &VOCAB[start..start + len]
}

/// FNV-1a over token bytes — tokens are short (≤ 129 bytes, mostly single
/// digits of bytes), where FNV is competitive with heavier hashers and keeps
/// the table dependency-free.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// The process-wide compact o200k_base store.
struct Bpe {
    /// `starts[rank]` = offset of rank's token bytes in [`VOCAB`] (past the
    /// length prefix).
    starts: Vec<u32>,
    /// Open-addressed (linear-probe) hash table of ranks, power-of-two sized at
    /// ~2.6× the vocabulary so probes stay short; [`EMPTY`] marks a free slot.
    /// Keys are not stored — a hit is confirmed by comparing the candidate
    /// rank's [`token_bytes`] against the queried slice.
    table: Vec<u32>,
    /// `table.len() - 1`, for masking a hash into a slot index.
    mask: usize,
    /// The compiled [`SPLIT_PATTERN`]. One instance — `tiktoken-rs` clones its
    /// equivalent 128× for thread-local access; every [`count`] here runs on
    /// whichever thread asked, sharing this one.
    splitter: fancy_regex::Regex,
}

impl Bpe {
    fn load() -> Self {
        let mut starts = Vec::with_capacity(VOCAB_RANKS);
        let mut offset = 0;
        while offset < VOCAB.len() {
            let len = VOCAB[offset] as usize;
            offset += 1;
            starts.push(u32::try_from(offset).expect("vocabulary blob fits u32 offsets"));
            offset += len;
        }
        assert_eq!(
            starts.len(),
            VOCAB_RANKS,
            "embedded o200k_base vocabulary is corrupt (regenerate with scripts/gen-o200k-asset.py)"
        );

        let capacity = (VOCAB_RANKS * 2).next_power_of_two();
        let mask = capacity - 1;
        let mut table = vec![EMPTY; capacity];
        for rank in 0..VOCAB_RANKS {
            let mut slot = (fnv1a(token_bytes(&starts, rank)) as usize) & mask;
            while table[slot] != EMPTY {
                slot = (slot + 1) & mask;
            }
            table[slot] = u32::try_from(rank).expect("ranks fit u32");
        }

        let splitter =
            fancy_regex::Regex::new(SPLIT_PATTERN).expect("o200k_base split pattern compiles");
        Self {
            starts,
            table,
            mask,
            splitter,
        }
    }

    /// The rank of exactly these bytes, if they are a vocabulary token.
    fn rank_of(&self, bytes: &[u8]) -> Option<u32> {
        let mut slot = (fnv1a(bytes) as usize) & self.mask;
        loop {
            let candidate = self.table[slot];
            if candidate == EMPTY {
                return None;
            }
            if token_bytes(&self.starts, candidate as usize) == bytes {
                return Some(candidate);
            }
            slot = (slot + 1) & self.mask;
        }
    }

    /// How many tokens one split-pattern piece encodes to.
    fn count_piece(&self, piece: &[u8]) -> usize {
        // A whole-piece vocabulary hit is one token (tiktoken's fast path); so
        // is a single byte, which BPE cannot split further.
        if piece.len() == 1 || self.rank_of(piece).is_some() {
            return 1;
        }
        if piece.len() < LARGE_PIECE_MIN {
            self.count_merge_small(piece)
        } else {
            self.count_merge_large(piece)
        }
    }

    /// tiktoken's `_byte_pair_merge`, counting the parts instead of collecting
    /// them: `parts` holds `(start, rank-of-pair-starting-here)`, and each round
    /// merges the leftmost lowest-ranked adjacent pair. O(n²) in the piece —
    /// deliberately, as upstream notes: pieces this short favour the compact
    /// scan over a heap.
    fn count_merge_small(&self, piece: &[u8]) -> usize {
        let mut parts: Vec<(usize, u32)> = Vec::with_capacity(piece.len() + 1);
        let mut min_rank: (u32, usize) = (EMPTY, usize::MAX);
        for (i, pair) in piece.windows(2).enumerate() {
            let rank = self.rank_of(pair).unwrap_or(EMPTY);
            if rank < min_rank.0 {
                min_rank = (rank, i);
            }
            parts.push((i, rank));
        }
        parts.push((piece.len() - 1, EMPTY));
        parts.push((piece.len(), EMPTY));

        // The rank of the merged span `parts[i] .. parts[i + 3]` — +3 because
        // the entry between them is about to be removed but hasn't been yet.
        let get_rank = |parts: &[(usize, u32)], i: usize| {
            if i + 3 < parts.len() {
                self.rank_of(&piece[parts[i].0..parts[i + 3].0])
                    .unwrap_or(EMPTY)
            } else {
                EMPTY
            }
        };

        while min_rank.0 != EMPTY {
            let i = min_rank.1;
            if i > 0 {
                parts[i - 1].1 = get_rank(&parts, i - 1);
            }
            parts[i].1 = get_rank(&parts, i);
            parts.remove(i + 1);

            min_rank = (EMPTY, usize::MAX);
            for (i, &(_, rank)) in parts[..parts.len() - 1].iter().enumerate() {
                if rank < min_rank.0 {
                    min_rank = (rank, i);
                }
            }
        }
        parts.len() - 1
    }

    /// tiktoken's `_byte_pair_merge_large`, counting the final spans instead of
    /// resolving their ranks: a min-heap of candidate merges (ordered rank then
    /// start, so ties still merge leftmost-first, exactly like the small path)
    /// over a linked list of spans. O(m log n) — the shape that keeps a
    /// pathological piece (a minified bundle's symbol run, a `===…` banner)
    /// from going quadratic.
    fn count_merge_large(&self, piece: &[u8]) -> usize {
        /// One current span: where it ends, the span before it, where merging
        /// with the next span would end, and that merge's rank (the heap entry
        /// is only valid while it matches — stale entries are skipped on pop).
        struct Span {
            prev: usize,
            end: usize,
            next_end: usize,
            next_rank: u32,
        }

        #[derive(Eq, PartialEq, Clone, Copy)]
        struct Merge {
            start: usize,
            rank: u32,
        }
        impl Ord for Merge {
            fn cmp(&self, other: &Self) -> Ordering {
                // Reversed: BinaryHeap is a max-heap, this makes it pop the
                // smallest (rank, start) — lowest rank first, leftmost on ties.
                other
                    .rank
                    .cmp(&self.rank)
                    .then_with(|| other.start.cmp(&self.start))
            }
        }
        impl PartialOrd for Merge {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut spans = Vec::with_capacity(piece.len());
        spans.push(Span {
            prev: usize::MAX,
            end: 1,
            next_end: 2,
            next_rank: EMPTY,
        });
        let mut heap = BinaryHeap::with_capacity(piece.len());
        for i in 0..piece.len() - 1 {
            if let Some(rank) = self.rank_of(&piece[i..i + 2]) {
                heap.push(Merge { start: i, rank });
                spans[i].next_rank = rank;
            }
            spans.push(Span {
                prev: i,
                end: i + 2,
                next_end: i + 3,
                next_rank: EMPTY,
            });
        }

        let propose =
            |spans: &mut Vec<Span>, heap: &mut BinaryHeap<Merge>, start: usize, next_end: usize| {
                spans[start].next_end = next_end;
                spans[start].next_rank = EMPTY; // always invalidate the old merge
                if next_end <= piece.len()
                    && let Some(rank) = self.rank_of(&piece[start..next_end])
                {
                    heap.push(Merge { start, rank });
                    spans[start].next_rank = rank;
                }
            };

        while let Some(left) = heap.pop() {
            if left.rank != spans[left.start].next_rank {
                continue; // invalidated while queued
            }
            let left_start = left.start;
            let right_start = spans[left_start].end;
            let right_end = spans[left_start].next_end;
            debug_assert_eq!(right_end, spans[right_start].end);
            let right_next_end = spans[right_start].next_end;

            // Merge left and right into one span, then propose the merges the
            // new span enables on both sides.
            spans[left_start].end = right_end;
            propose(&mut spans, &mut heap, left_start, right_next_end);
            if right_end < spans.len() {
                spans[right_end].prev = left_start;
            }
            if left_start > 0 {
                let prev_start = spans[left_start].prev;
                propose(&mut spans, &mut heap, prev_start, right_end);
            }
            // The absorbed right span's queued merge is now stale.
            spans[right_start].next_rank = EMPTY;
        }

        let mut count = 0;
        let mut i = 0;
        while i < spans.len() {
            count += 1;
            i = spans[i].end;
        }
        count
    }

    /// The number of tokens in `text`: split with [`SPLIT_PATTERN`], sum each
    /// piece's count. Never materialises the tokens themselves.
    fn count_text(&self, text: &str) -> usize {
        let mut total = 0;
        let mut consumed = 0;
        for found in self.splitter.find_iter(text) {
            match found {
                Ok(piece) => {
                    total += self.count_piece(piece.as_str().as_bytes());
                    consumed = piece.end();
                }
                Err(_) => {
                    // fancy-regex's backtrack ceiling — unreachable for this
                    // pattern in practice (tiktoken unwraps here). Degrade to a
                    // coarse estimate of the unscanned tail rather than panic
                    // mid-tally.
                    total += text.len().saturating_sub(consumed).div_ceil(4);
                    break;
                }
            }
        }
        total
    }
}

/// The process-wide o200k_base store, built on first use from the embedded
/// vocabulary (no I/O), then shared for every count.
fn bpe() -> &'static Bpe {
    static BPE: OnceLock<Bpe> = OnceLock::new();
    BPE.get_or_init(Bpe::load)
}

/// The number of `o200k_base` tokens in `text`.
///
/// Splits with the ordinary-text pattern (any `<|special|>` markers count as
/// ordinary text, like tiktoken's `encode_ordinary`) — so counting arbitrary
/// user, model, or tool text can never error.
#[must_use]
pub fn count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    bpe().count_text(text)
}

/// Force the one-time vocabulary build (offset scan + hash table + split-regex
/// compile) now, so the first real [`count`] doesn't pay it. The boundary calls
/// this on a background thread at startup (`main.rs`) — long before the first
/// turn — keeping it off the interactive path. Idempotent: the [`OnceLock`]
/// means later calls are free.
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

    #[test]
    fn embedded_vocabulary_matches_tiktoken_rs_rank_by_rank() {
        // The strongest possible check on the committed asset: every rank's
        // bytes equal what the reference tokenizer decodes for that rank, so a
        // bad regeneration (or a corrupted asset) cannot land silently.
        let reference = tiktoken_rs::o200k_base().expect("reference tokenizer loads");
        let ours = bpe();
        for rank in 0..VOCAB_RANKS {
            let bytes = token_bytes(&ours.starts, rank);
            let reference_bytes = reference
                .decode_bytes(&[u32::try_from(rank).expect("ranks fit u32")])
                .expect("reference decodes every rank");
            assert_eq!(
                bytes,
                reference_bytes.as_slice(),
                "vocabulary diverged at rank {rank}"
            );
        }
    }

    #[test]
    fn every_rank_is_findable_through_the_hash_table() {
        // A duplicate token in the asset would shadow its twin in the table
        // and silently mis-rank lookups; round-tripping every rank rules the
        // whole class out.
        let ours = bpe();
        for rank in 0..VOCAB_RANKS {
            assert_eq!(
                ours.rank_of(token_bytes(&ours.starts, rank)),
                Some(u32::try_from(rank).expect("ranks fit u32")),
                "rank {rank} unreachable through the hash table"
            );
        }
    }

    /// Every corpus entry a differential test feeds both tokenizers: the shapes
    /// the app actually counts (prose, code, tool output) plus the encoder's
    /// known edge classes — contractions the split pattern special-cases,
    /// digit-run splitting, whitespace-lookahead boundaries, multi-byte
    /// scripts, and pieces long enough to take the large-merge path.
    fn corpus() -> Vec<String> {
        let mut texts: Vec<String> = [
            "hello world",
            "The quick brown fox jumps over the lazy dog.",
            "a",
            " ",
            "\n",
            "\r\n\r\n",
            "  \t \n\t\r\n   ",
            "trailing spaces   \nnext line",
            "word'll word's WORD'S can't I'M we'Re they'd o'clock",
            "1 22 333 4444 55555 123456789012",
            "var x1y2z3 = 42; // mixed alnum",
            "fn main() {\n    println!(\"hello, {}\", 42);\n}\n",
            "def f(xs):\n    return [x**2 for x in xs if x % 2 == 0]\n",
            "SELECT a, b FROM t WHERE x = 'y' ORDER BY a DESC;",
            "<div class=\"a\"><span id='b'>hi &amp; bye</span></div>",
            "#id .cls { color: #fff; margin: 0 auto; }",
            "{\"key\": [1, 2, {\"nested\": null, \"ok\": true}]}",
            "for f in *.txt; do echo \"$f\" >> out.log; done",
            "| col a | col b |\n|-------|-------|\n| 1     | 2     |\n",
            "# Title\n\nSome *emphasis* and `inline code` here.\n\n```rust\nlet x = 1;\n```\n",
            "path/to/file.rs:120:5: error[E0308]: mismatched types",
            "https://example.com/a/b?q=1&r=2#frag",
            "naïve façade coöperate résumé",
            "日本語のテキストを数える",
            "中文分词测试，标点。",
            "한국어 토큰 수 세기",
            "Привет, мир! Ελληνικά κείμενα.",
            "مرحبا بالعالم مع تشكيلٍ",
            "हिन्दी पाठ गिनती",
            "ไทยไม่มีช่องว่างระหว่างคำ",
            "😀😃😄 family: 👨‍👩‍👧‍👦 flags: 🇯🇵🇺🇸 tones: 👍🏽👍🏿",
            "e\u{301}e\u{301}e\u{301} combining marks a\u{300}\u{316}\u{35c}b",
            "tab\tseparated\tvalues\tline",
            "NUL byte \u{0} and controls \u{1}\u{2}\u{3} inside",
            "===>>><<<===!!!???***&&&|||~~~^^^%%%",
            "(((nested))) [[brackets]] {{braces}}",
            "-- comment ;; symbols //path\\back\\slashes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        // Every Latin-1 scalar in one string (multi-byte from U+0080 up).
        texts.push((0u32..=0xFF).map(|c| char::from_u32(c).unwrap()).collect());
        // Pieces past the 100-byte large-merge threshold: one unbroken word,
        // one symbol run, one repeated-emoji run, plus a big '=' banner.
        texts.push("supercalifragilisticexpialidocious".repeat(8));
        texts.push("#!$%".repeat(60));
        texts.push("🦀".repeat(80));
        texts.push("=".repeat(5000));
        texts.push(" ".repeat(300));
        texts.push("ab ".repeat(2000));
        texts
    }

    #[test]
    fn counts_match_tiktoken_rs_across_a_broad_corpus() {
        let reference = tiktoken_rs::o200k_base().expect("reference tokenizer loads");
        for text in corpus() {
            assert_eq!(
                count(&text),
                reference.encode_ordinary(&text).len(),
                "count diverged from tiktoken on {text:?}"
            );
        }
    }

    #[test]
    fn counts_match_tiktoken_rs_on_seeded_random_text() {
        // A fixed LCG (no rand dependency) drives deterministic fuzzing over an
        // alphabet that mixes ASCII, whitespace, punctuation, digits,
        // apostrophes, multi-byte scripts, and emoji — the mixes most likely to
        // shake out a split-pattern or merge divergence.
        let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\
             0123456789 \t\n\r'\"!?.,;:-_/\\()[]{}<>=+*&%$#@^~|`\
             éüßñçøæ日本語中文한국어русскийΩπλ😀🦀👍✨\u{301}\u{300}"
            .chars()
            .collect();
        let mut state: u64 = 0x243F_6A88_85A3_08D3;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        let reference = tiktoken_rs::o200k_base().expect("reference tokenizer loads");
        for case in 0..300 {
            let len = next() % 240;
            let text: String = (0..len)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect();
            assert_eq!(
                count(&text),
                reference.encode_ordinary(&text).len(),
                "count diverged from tiktoken on case {case}: {text:?}"
            );
        }
    }
}

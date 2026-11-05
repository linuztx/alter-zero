//! Provider-agnostic token estimates, and the calibration that corrects them
//! with what each provider reports (`docs/token-estimate.md`).
//!
//! Every token figure the app shows between a provider's usage frames — the
//! status line's live tally, the context gauge before a request's frame
//! lands, the `/context` page's breakdown, Ctrl+D's per-entry sizes — is an
//! *estimate*: the providers this app talks to tokenize with a dozen
//! different vocabularies (OpenAI's `o200k_base`, Anthropic's, Gemini's
//! SentencePiece, Llama's, Qwen's, …), none of which ships with a client, so
//! no local count is exact for more than one of them. [`estimate`] does not
//! pretend to be one of them. It models what every byte-pair tokenizer does —
//! a common word is one token with the space in front of it, a long or rare
//! word is several, digits go in threes, a run of indentation is one token,
//! punctuation clusters, a Han character is close to one token, a random
//! base64 run is one per one and a half characters — in one pass over the
//! text, with no vocabulary, no regex and no allocation. Fitted against
//! `o200k_base` and `cl100k_base` over code, prose, tool output and fourteen
//! languages, it lands within a few percent of both on English, code and
//! tool output, and between the two (nearer the newer, denser one) where they
//! disagree.
//!
//! What it cannot know is which tokenizer the next request will meet. That is
//! [`Calibration`]'s job: every request is estimated before it is sent, the
//! provider's usage frame says what it actually counted, and the ratio of the
//! two scales every estimate after it — so once a provider has spoken, the
//! app's figures are in that provider's tokens.

/// What one image attachment is estimated at. An image has no text to size
/// and every provider prices one differently (by tiles, by pixels, flat), so
/// this is a stand-in that [`Calibration`] corrects like any other share of
/// a request.
pub const IMAGE_TOKENS: usize = 256;

/// Estimates are summed in thousandths of a token, so a word part's quarter
/// tokens and a run's fractions add up before anything is rounded.
const TOKEN: u64 = 1000;
/// A word part's first token covers this many letters…
const WORD_FIRST_LETTERS: u64 = 6;
/// …and each letter past them costs a quarter token: rare and long words
/// split into several pieces.
const WORD_EXTRA_LETTER: u64 = 250;
/// An all-capitals part (`HTTP`, `MAX`) — about six letters a token.
const CAPS_LETTER: u64 = 167;
/// Digits split into groups of at most three.
const DIGITS_PER_TOKEN: u64 = 3;
/// A run of punctuation — `();`, `::`, `-->` — about five characters a
/// token, never less than one.
const PUNCT_CHAR: u64 = 200;
/// One punctuation character opening a word (`(self`, `"name`): it usually
/// joins the word's token…
const WORD_LEAD: u64 = 500;
/// …and almost always does when it is one of the characters paths, urls and
/// identifiers are made of ([`JOINING_LEADS`]).
const JOINING_LEAD: u64 = 150;
/// The punctuation that joins the word after it — `/home`, `.rs`, `_id`,
/// `-x`, `?q`, `&page`.
const JOINING_LEADS: &[u8] = b"/._-'@$#?&%\\<*!+=";
/// Two or more spaces (indentation, column alignment), or spaces ending the
/// text: one token for the run.
const SPACE_RUN: u64 = 1300;
/// A space before a number is a token of its own: no byte-pair
/// pre-tokenizer joins a space to the digits after it.
const SPACE_BEFORE_NUMBER: u64 = 1500;
/// A run of line breaks, with the spaces between them.
const LINE_BREAK: u64 = 750;
/// Each accented Latin letter, on top of its word: `ü`, `é`, `ñ` split words
/// the plain alphabet keeps whole.
const ACCENT: u64 = 400;
/// Each Cyrillic letter — the vocabularies hold Cyrillic words almost as
/// well as English ones.
const CYRILLIC: u64 = 300;
/// Each letter of Greek, Armenian, Georgian, Hebrew, Arabic and their
/// neighbours.
const ALPHABET: u64 = 500;
/// Each letter of the Indic scripts, Thai, Lao, Tibetan, Myanmar, Khmer and
/// Ethiopic — and of any script the table does not name.
const ABUGIDA: u64 = 550;
/// Each Han character, kana or Hangul syllable.
const CJK: u64 = 850;
/// Each emoji or pictograph.
const EMOJI: u64 = 2200;
/// Each zero-width joiner, variation selector or skin tone holding an emoji
/// sequence together.
const JOINER: u64 = 600;
/// Each character of a random-looking base64 run — about one and a half
/// characters a token, where words of the same length are four or five.
const BLOB_CHAR: u64 = 690;
/// How long a run must be before it can read as random…
const BLOB_MIN_LEN: usize = 20;
/// …and how often, per hundred characters, its letters must flip case or
/// turn into digits. Words, paths and identifiers flip a handful of times;
/// base64 flips at every other character.
const BLOB_MIN_FLIPS_PER_HUNDRED: usize = 35;

/// What a character contributes to an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Lower,
    Upper,
    Accent,
    Cyrillic,
    Alphabet,
    Abugida,
    Cjk,
    Digit,
    Space,
    LineBreak,
    Punct,
    Emoji,
    Joiner,
    Control,
}

impl Class {
    /// Whether this character can be part of a word.
    const fn in_word(self) -> bool {
        matches!(
            self,
            Self::Lower
                | Self::Upper
                | Self::Accent
                | Self::Cyrillic
                | Self::Alphabet
                | Self::Abugida
        )
    }
}

/// Every ASCII character's class, so the common case is a table read.
const ASCII: [Class; 128] = {
    let mut table = [Class::Control; 128];
    let mut byte = 0;
    while byte < 128 {
        table[byte] = match byte as u8 {
            b'a'..=b'z' => Class::Lower,
            b'A'..=b'Z' => Class::Upper,
            b'0'..=b'9' => Class::Digit,
            b' ' | b'\t' | 0x0B | 0x0C => Class::Space,
            b'\n' | b'\r' => Class::LineBreak,
            b'!'..=b'/' | b':'..=b'@' | b'['..=b'`' | b'{'..=b'~' => Class::Punct,
            _ => Class::Control,
        };
        byte += 1;
    }
    table
};

/// A non-ASCII character's class, by the block it sits in.
fn class_of(c: char) -> Class {
    let code = c as u32;
    match code {
        0x200D | 0xFE00..=0xFE0F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F => Class::Joiner,
        0x0085 | 0x2028 | 0x2029 => Class::LineBreak,
        0x00D7 | 0x00F7 => Class::Punct,
        0x00C0..=0x024F | 0x0300..=0x036F | 0x1E00..=0x1EFF => Class::Accent,
        0x0400..=0x052F => Class::Cyrillic,
        // Greek, Armenian, Hebrew, Arabic, Syriac, Thaana, NKo, Georgian —
        // with the Hebrew points and Arabic vowel marks riding their letters.
        0x0370..=0x03FF | 0x0530..=0x08FF | 0x10A0..=0x10FF => {
            if c.is_alphabetic()
                || (0x0591..=0x05C7).contains(&code)
                || (0x064B..=0x065F).contains(&code)
            {
                Class::Alphabet
            } else if c.is_numeric() {
                Class::Digit
            } else {
                Class::Punct
            }
        }
        // Indic, Sinhala, Thai, Lao, Tibetan, Myanmar, Ethiopic, Khmer.
        0x0900..=0x0FFF | 0x1000..=0x109F | 0x1200..=0x139F | 0x1780..=0x17FF => {
            if c.is_numeric() {
                Class::Digit
            } else {
                Class::Abugida
            }
        }
        0x1100..=0x11FF
        | 0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3100..=0x31FF
        | 0x3200..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => Class::Cjk,
        0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x1F000.. => Class::Emoji,
        _ if c.is_whitespace() => Class::Space,
        _ if c.is_alphabetic() => Class::Abugida,
        _ if c.is_numeric() => Class::Digit,
        _ if c.is_control() => Class::Control,
        _ => Class::Punct,
    }
}

/// One part of a word, cut where `camelCase` changes case.
#[derive(Default)]
struct Part {
    letters: u64,
    capitals: u64,
}

impl Part {
    fn cost(&self) -> u64 {
        match self.letters {
            0 => 0,
            letters if self.capitals == letters && letters > 1 => {
                (letters * CAPS_LETTER).max(TOKEN)
            }
            letters => TOKEN + letters.saturating_sub(WORD_FIRST_LETTERS) * WORD_EXTRA_LETTER,
        }
    }
}

/// A cursor over the text being estimated.
struct Scan<'a> {
    text: &'a str,
    at: usize,
    /// Where the last run that failed the blob test ended: nothing before it
    /// is tested again, which keeps the whole pass linear.
    blob_checked: usize,
}

impl Scan<'_> {
    /// The class and UTF-8 length of the character at byte `at`.
    fn class_at(&self, at: usize) -> Option<(Class, usize)> {
        let byte = *self.text.as_bytes().get(at)?;
        if byte < 0x80 {
            return Some((ASCII[usize::from(byte)], 1));
        }
        let c = self.text[at..].chars().next()?;
        Some((class_of(c), c.len_utf8()))
    }

    fn peek(&self) -> Option<(Class, usize)> {
        self.class_at(self.at)
    }

    /// Consume every character of `class` from here; how many there were.
    fn take_while(&mut self, class: Class) -> u64 {
        let mut count = 0;
        while let Some((next, len)) = self.peek() {
            if next != class {
                break;
            }
            self.at += len;
            count += 1;
        }
        count
    }

    /// A random-looking run of the base64 alphabet starting here, consumed
    /// and costed — or `None`, the run marked as tested, to be read as words
    /// and numbers instead.
    fn take_blob(&mut self) -> Option<u64> {
        let bytes = self.text.as_bytes();
        let start = self.at;
        let mut end = start;
        let (mut flips, mut last, mut seen) = (0, 0u8, 0u8);
        while let Some(&byte) = bytes.get(end) {
            let kind = match byte {
                b'a'..=b'z' => 1,
                b'A'..=b'Z' => 2,
                b'0'..=b'9' => 4,
                b'+' | b'/' | b'=' | b'_' | b'-' => 0,
                _ => break,
            };
            if kind != 0 {
                if last != 0 && kind != last {
                    flips += 1;
                }
                last = kind;
                seen |= kind;
            }
            end += 1;
        }
        let len = end - start;
        if len >= BLOB_MIN_LEN
            && seen & 0b11 == 0b11
            && flips * 100 >= len * BLOB_MIN_FLIPS_PER_HUNDRED
        {
            self.at = end;
            return Some(len as u64 * BLOB_CHAR);
        }
        self.blob_checked = end.max(start + 1);
        None
    }

    /// A word — letters of any script but the CJK ones — cut into parts
    /// where its case changes; never less than a token.
    fn take_word(&mut self) -> u64 {
        let mut cost = 0;
        let mut part = Part::default();
        let mut previous = Class::Space;
        while let Some((class, len)) = self.peek() {
            match class {
                Class::Lower => {
                    if previous == Class::Upper
                        && part.capitals >= 2
                        && part.capitals == part.letters
                    {
                        // `HTMLParser`: the last capital opens the next part.
                        cost += Part {
                            letters: part.letters - 1,
                            capitals: part.capitals - 1,
                        }
                        .cost();
                        part = Part {
                            letters: 1,
                            capitals: 1,
                        };
                    }
                    part.letters += 1;
                }
                Class::Upper => {
                    if previous == Class::Lower {
                        cost += part.cost();
                        part = Part::default();
                    }
                    part.letters += 1;
                    part.capitals += 1;
                }
                Class::Accent => {
                    part.letters += 1;
                    cost += ACCENT;
                }
                Class::Cyrillic => cost += CYRILLIC,
                Class::Alphabet => cost += ALPHABET,
                Class::Abugida => cost += ABUGIDA,
                _ => break,
            }
            previous = class;
            self.at += len;
        }
        (cost + part.cost()).max(TOKEN)
    }

    /// Spaces between pieces. One space rides the piece after it — except a
    /// number, which never takes one; a longer run is a token of its own;
    /// spaces ending a line belong to its line break.
    fn take_spaces(&mut self) -> u64 {
        let run = self.take_while(Class::Space);
        match self.peek() {
            None => SPACE_RUN,
            Some((Class::LineBreak, _)) => 0,
            Some((next, _)) => {
                let indent = if run >= 2 { SPACE_RUN } else { 0 };
                indent
                    + if next == Class::Digit {
                        SPACE_BEFORE_NUMBER
                    } else {
                        0
                    }
            }
        }
    }

    /// Line breaks, and the spaces between them, up to the run's last line
    /// break: one piece. The indentation after it is the next one.
    fn take_line_breaks(&mut self) -> u64 {
        let mut probe = self.at;
        while let Some((class, len)) = self.class_at(probe) {
            match class {
                Class::LineBreak => {
                    probe += len;
                    self.at = probe;
                }
                Class::Space => probe += len,
                _ => break,
            }
        }
        LINE_BREAK
    }

    /// A run of punctuation, with the line break straight after it, which
    /// joins its piece. A lone character opening a word mostly joins the
    /// word instead.
    fn take_punctuation(&mut self) -> u64 {
        let first = self.text.as_bytes()[self.at];
        let run = self.take_while(Class::Punct);
        let opens_word =
            matches!(self.peek(), Some((class, _)) if class.in_word() || class == Class::Cjk);
        self.take_while(Class::LineBreak);
        if run == 1 && opens_word {
            if JOINING_LEADS.contains(&first) {
                JOINING_LEAD
            } else {
                WORD_LEAD
            }
        } else {
            (run * PUNCT_CHAR).max(TOKEN)
        }
    }
}

/// About how many tokens `text` is, by any current model's tokenizer: a
/// provider-agnostic estimate (this module's doc), never less than one for
/// text that is not empty. One pass, no allocation.
#[must_use]
pub fn estimate(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut scan = Scan {
        text,
        at: 0,
        blob_checked: 0,
    };
    let mut thousandths: u64 = 0;
    while let Some((class, len)) = scan.peek() {
        thousandths += match class {
            Class::Lower | Class::Upper | Class::Digit
                if scan.at >= scan.blob_checked && text.as_bytes()[scan.at] < 0x80 =>
            {
                match scan.take_blob() {
                    Some(cost) => cost,
                    None => continue,
                }
            }
            class if class.in_word() => scan.take_word(),
            Class::Digit => scan.take_while(Class::Digit).div_ceil(DIGITS_PER_TOKEN) * TOKEN,
            Class::Space => scan.take_spaces(),
            Class::LineBreak => scan.take_line_breaks(),
            Class::Punct => scan.take_punctuation(),
            single => {
                scan.at += len;
                match single {
                    Class::Cjk => CJK,
                    Class::Emoji => EMOJI,
                    Class::Joiner => JOINER,
                    _ => TOKEN,
                }
            }
        };
    }
    usize::try_from((thousandths + TOKEN / 2) / TOKEN)
        .unwrap_or(usize::MAX)
        .max(1)
}

/// The smallest request whose report teaches anything: below it the
/// provider's per-message framing outweighs the text.
const MIN_ESTIMATE_TO_LEARN: u64 = 256;

/// The ratios a report may teach, per thousand: a provider's tokenizer runs
/// within a factor of four of the estimate either way, so a report outside
/// that is a usage frame gone wrong (a cached-only count, a session total)
/// and is ignored.
const PLAUSIBLE_PER_MILLE: std::ops::RangeInclusive<u64> = 250..=4000;

/// How far a provider's token counts run from [`estimate`]'s, learned from
/// the usage frames it reports (`docs/token-estimate.md`).
///
/// Before each request the backend estimates everything it is about to send;
/// the round's usage frame then says what the provider counted for it. Their
/// ratio is the provider's tokenizer — and its chat framing, and what it
/// charges for an image — relative to the estimate, and [`apply`] carries it
/// to every later estimate. The newest report replaces the last rather than
/// averaging with it: every request carries the whole conversation before
/// it, so the newest ratio is the one measured over the context as it is now,
/// and scaling the next request's estimate by it is exactly the last reported
/// count plus the estimated growth since, in the provider's tokens.
///
/// [`apply`]: Self::apply
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calibration {
    /// Provider tokens per thousand estimated tokens.
    per_mille: u64,
}

impl Calibration {
    /// No correction: estimates stand as they are.
    pub const NEUTRAL: Self = Self { per_mille: 1000 };

    /// Learn from one request: `estimated` is what [`estimate`] made of
    /// everything it carried, `reported` the input tokens the provider's
    /// usage frame counted for it. A request too small to say anything, or a
    /// report no tokenizer could produce, changes nothing.
    pub fn observe(&mut self, estimated: u64, reported: u64) {
        if estimated < MIN_ESTIMATE_TO_LEARN {
            return;
        }
        let per_mille = u128::from(reported) * 1000 / u128::from(estimated);
        if let Ok(per_mille) = u64::try_from(per_mille)
            && PLAUSIBLE_PER_MILLE.contains(&per_mille)
        {
            self.per_mille = per_mille;
        }
    }

    /// `estimated`, in the provider's tokens: scaled by the last report's
    /// ratio, unchanged before any.
    #[must_use]
    pub fn apply(&self, estimated: u64) -> u64 {
        let scaled = (u128::from(estimated) * u128::from(self.per_mille) + 500) / 1000;
        u64::try_from(scaled).unwrap_or(u64::MAX)
    }

    /// [`apply`](Self::apply) for a count — the views count in `usize`.
    #[must_use]
    pub fn apply_count(&self, estimated: usize) -> usize {
        let scaled = self.apply(u64::try_from(estimated).unwrap_or(u64::MAX));
        usize::try_from(scaled).unwrap_or(usize::MAX)
    }

    /// Provider tokens per thousand estimated ones — 1000 until a report
    /// says otherwise.
    #[must_use]
    pub const fn per_mille(&self) -> u64 {
        self.per_mille
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_zero_tokens() {
        assert_eq!(estimate(""), 0);
    }

    #[test]
    fn any_other_text_is_at_least_one_token() {
        for text in [" ", "\n", "a", "\t\t", "😀", ".", "\u{0}"] {
            assert!(estimate(text) >= 1, "{text:?}");
        }
    }

    #[test]
    fn common_words_are_a_token_each() {
        assert_eq!(estimate("hello world"), 2);
        assert_eq!(estimate("The quick brown fox jumps over the lazy dog."), 10);
    }

    #[test]
    fn a_space_rides_the_word_after_it() {
        assert_eq!(estimate(" world"), estimate("world"));
    }

    #[test]
    fn word_sized_pieces_add_up_to_the_whole() {
        // The live tally sums what each streamed piece estimates, and a
        // provider streams in token-sized pieces: they must add up to what
        // the whole text estimates, not to a piece's rounding apiece.
        let pieces = [
            "The", " quick", " brown", " fox", " jumps", " over", " the", " lazy", " dog",
        ];
        let whole: String = pieces.concat();
        let summed: usize = pieces.iter().map(|piece| estimate(piece)).sum();
        assert_eq!(summed, estimate(&whole));
    }

    #[test]
    fn long_words_cost_more_than_short_ones() {
        assert!(estimate("internationalization") > estimate("nation"));
        assert!(estimate("internationalization") >= 4);
    }

    #[test]
    fn identifiers_split_where_their_case_changes() {
        assert_eq!(estimate("hashmap"), 1);
        assert_eq!(estimate("HashMap"), 2);
        // An acronym ahead of a word: the last capital opens the word.
        assert_eq!(estimate("HTMLParser"), 2);
    }

    #[test]
    fn numbers_are_a_token_per_three_digits() {
        assert_eq!(estimate("1234567"), 3);
        assert_eq!(estimate("2026"), 2);
    }

    #[test]
    fn the_space_before_a_number_is_a_token_of_its_own() {
        // BPE pre-tokenizers never join a space to the digits after it.
        assert!(estimate("page 3") > estimate("page3"));
    }

    #[test]
    fn indentation_and_line_breaks_cost_a_token_a_run() {
        let flat = estimate("fn main() {}");
        let indented = estimate("fn main() {\n        body();\n}\n");
        assert!(indented > flat);
        // Eight spaces of indentation cost what four do: one run.
        assert_eq!(estimate("x\n        y"), estimate("x\n    y"),);
    }

    #[test]
    fn cjk_text_is_close_to_a_token_a_character() {
        let text = "人工智能正在改变软件开发的方式";
        let chars = text.chars().count();
        let tokens = estimate(text);
        assert!(
            tokens * 10 >= chars * 7 && tokens <= chars,
            "{tokens} for {chars}"
        );
    }

    #[test]
    fn a_random_base64_run_is_denser_than_words() {
        let blob = "aGVsbG8gd29ybGQgdGhpcyBpcyBiYXNlNjQgZGF0YQ==";
        // About a token per one and a half characters, where prose of the
        // same length is a handful of words.
        assert!(
            estimate(blob) * 3 >= blob.len() * 2 - 3,
            "{}",
            estimate(blob)
        );
        assert!(estimate("an ordinary sentence of the same length.") < estimate(blob) / 2);
    }

    #[test]
    fn a_path_or_identifier_is_not_mistaken_for_a_blob() {
        // Long runs of the base64 alphabet that read as words are words.
        assert!(estimate("/home/user/Projects2024/alter-zero/src") < 15);
        assert!(estimate("unwrap_or_default_for_every_value") < 15);
    }

    #[test]
    fn scripts_cost_what_their_tokenizers_charge() {
        // A Cyrillic letter is cheaper than an Arabic one, which is cheaper
        // than a Devanagari one — each script's place in the vocabularies.
        let cyrillic = estimate("Поэтому мы решили считать токены заранее");
        let arabic = estimate("لذلك نحتاج إلى طريقة سريعة لتقدير عدد");
        assert!(cyrillic < 20, "{cyrillic}");
        assert!(arabic > 10, "{arabic}");
        assert!(estimate("ข") >= 1 && estimate("नमस्ते") >= 2);
    }

    #[test]
    fn emoji_cost_more_than_letters() {
        assert!(estimate("🎉🎊🥳") >= 5);
        // A zero-width-joined family is several pictures, joined.
        assert!(estimate("👨\u{200D}👩\u{200D}👧") > estimate("👨"));
    }

    #[test]
    fn every_kind_of_text_estimates_without_panicking() {
        // Non-ASCII digits once sent the blob check round forever: it read
        // a zero-length run and never moved on.
        for text in [
            "٣٤٥",
            "²³¹ ½",
            "abc٣def",
            "\u{0300}\u{0301}",
            "e\u{0301}",
            "\r\n\r\n",
            "\u{FEFF}bom",
            "\u{1F3FB}",
            "Ⅻ ⅻ",
            "\u{7F}\u{1}\u{2}",
            "a\u{2028}b\u{2029}c\u{85}d",
            "ﾃｽﾄ　ＡＢＣ",
        ] {
            let _ = estimate(text);
        }
    }

    #[test]
    fn the_estimate_is_deterministic() {
        let text = "def main():\n    print('hi')";
        assert_eq!(estimate(text), estimate(text));
    }

    /// Samples of what fills an agent's context, each with its token count
    /// under tiktoken's `o200k_base` (GPT-4o onward) and `cl100k_base`
    /// (GPT-4, and the shape of the Llama 3 / Qwen vocabularies), measured
    /// when the estimator was fitted (`docs/token-estimate.md`).
    const REFERENCE: &[(&str, &str, usize, usize)] = &[
        (
            "english prose",
            "The model reads the whole conversation on every request, so the context window \
             fills as the session goes on. When it is nearly full, the app summarizes the older \
             turns and keeps the recent ones, which is cheaper than starting over and loses far \
             less.",
            50,
            50,
        ),
        (
            "rust code",
            "pub fn estimate_messages(messages: &[ChatMessage]) -> u64 {\n    messages\n        \
             .iter()\n        .map(|message| match &message.content {\n            \
             MessageContent::Text(text) => estimate(text) as u64,\n            \
             MessageContent::Parts(parts) => parts.len() as u64 * 4,\n        })\n        \
             .sum()\n}\n",
            72,
            72,
        ),
        (
            "python code",
            "def load_settings(path: str) -> dict:\n    \"\"\"Read the settings file, falling \
             back to the defaults.\"\"\"\n    try:\n        with open(path, encoding=\"utf-8\") \
             as handle:\n            return {**DEFAULTS, **json.load(handle)}\n    except \
             FileNotFoundError:\n        return dict(DEFAULTS)\n",
            65,
            62,
        ),
        (
            "pretty json",
            "{\n  \"results\": [\n    {\n      \"id\": 17,\n      \"name\": \"item-17\",\n      \
             \"tags\": [\"alpha\", \"prod\"],\n      \"price\": 412.5,\n      \"active\": true\n    \
             }\n  ],\n  \"next_page\": null\n}\n",
            61,
            61,
        ),
        (
            "git diff",
            "diff --git a/src/app/compact.rs b/src/app/compact.rs\nindex 3f1c2a7..9b0e4d2 \
             100644\n--- a/src/app/compact.rs\n+++ b/src/app/compact.rs\n@@ -160,7 +160,7 @@ \
             impl App {\n-        let total = self.system_prompt.as_deref().map_or(0, \
             count_tokens)\n+        let total = self.system_prompt.as_deref().map_or(0, \
             estimate_tokens)\n             + estimate_messages_tokens(&messages);\n",
            114,
            114,
        ),
        (
            "ls output",
            "drwxr-xr-x  5 root root    4096 Oct 31 01:17 assets\n-rw-r--r--  1 root root   \
             48211 Nov  5 09:42 Cargo.lock\n-rw-r--r--  1 root root    9133 Nov  5 09:42 \
             Cargo.toml\nlrwxrwxrwx  1 root root       9 Apr  8  2024 latest -> v0.18.0\n",
            108,
            108,
        ),
        (
            "log lines",
            "2026-11-05T09:42:17.204Z INFO  [worker-3] request 5f0c2a1e finished in 184ms \
             status=200\n2026-11-05T09:42:18.913Z WARN  [worker-7] retrying upstream call \
             (attempt 2/5) after 500ms\n2026-11-05T09:42:19.002Z ERROR [worker-7] upstream \
             refused: connection reset by peer\n",
            108,
            108,
        ),
        (
            "csv numbers",
            "id,timestamp,latency_ms,bytes,status\n1,1700000037,184,52311,200\n\
             2,1700000074,9921,1048576,500\n3,1700000111,12,880,404\n",
            54,
            53,
        ),
        (
            "base64 blob",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJ\
             RU5ErkJgggZm9vYmFyYmF6cXV4MTIzNDU2Nzg5MEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFla",
            90,
            99,
        ),
        (
            "hex digests",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  Cargo.lock\n\
             2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae  src/main.rs\n",
            80,
            80,
        ),
        (
            "urls and paths",
            "https://github.com/linuztx/alter-zero/blob/main/docs/compact.md?plain=1#L136\n\
             /home/user/alter-zero/src/llm/backend.rs:2022:fn stream_round(\n",
            44,
            44,
        ),
        (
            "markdown table",
            "| number | where it comes from |\n| --- | --- |\n| the gauge after a round | the \
             provider's own `input + output` |\n| the gauge before a frame | the calibrated \
             estimate of the request |\n",
            44,
            44,
        ),
        (
            "chinese",
            "人工智能正在改变软件开发的方式。过去，程序员需要手动编写每一行代码，而现在，智能助手\
             可以根据自然语言描述生成函数、修复错误，甚至解释复杂的系统架构。",
            47,
            70,
        ),
        (
            "japanese",
            "今日は朝から雨が降っていたので、家で本を読むことにしました。最近読んでいるのは、\
             プログラミングの歴史についての本です。",
            40,
            59,
        ),
        (
            "korean",
            "서울의 겨울은 생각보다 훨씬 춥습니다. 처음 이곳에 왔을 때는 두꺼운 외투도 없이 가벼운 \
             재킷 하나만 가지고 왔습니다.",
            44,
            68,
        ),
        (
            "russian",
            "Поэтому мы решили считать токены заранее, перед каждым запросом, а затем сверять \
             оценку с точными данными, которые возвращает провайдер.",
            33,
            54,
        ),
        (
            "arabic",
            "لذلك نحتاج إلى طريقة سريعة لتقدير عدد الرموز قبل إرسال الطلب، ثم تصحيح هذا التقدير \
             باستخدام الأرقام الحقيقية التي يعيدها مزود الخدمة.",
            38,
            94,
        ),
        (
            "hindi",
            "लेकिन इनका सही उपयोग करने के लिए यह समझना ज़रूरी है कि मॉडल कैसे काम करता है और \
             उसकी सीमाएँ क्या हैं।",
            28,
            100,
        ),
        (
            "thai",
            "หลังจากนั้นเรานั่งเรือชมวิถีชีวิตของชาวบ้านริมคลอง ซึ่งยังคงรักษาวัฒนธรรมดั้งเดิมไว้ได้อย่างดี",
            36,
            95,
        ),
        (
            "german",
            "Automatisierte Tests, kontinuierliche Integration und sorgfältige \
             Code-Überprüfungen sind dabei unverzichtbar.",
            23,
            32,
        ),
        (
            "emoji chat",
            "Party tonight 🎉🎊🥳? I'll bring 🍕🍺. 👨\u{200D}👩\u{200D}👧\u{200D}👦 family, \
             🏳\u{FE0F}\u{200D}🌈 flag, 👍🏽 thumbs, ❤\u{FE0F} heart. ✅ done ❌ failed ⚠\u{FE0F} \
             warning",
            55,
            74,
        ),
    ];

    /// Where a provider-agnostic estimate should land between the two
    /// references: nearer the newer, denser vocabulary, which is what most
    /// providers now serve (`docs/token-estimate.md`).
    fn midpoint(o200k: usize, cl100k: usize) -> f64 {
        (o200k as f64).powf(0.65) * (cl100k as f64).powf(0.35)
    }

    #[test]
    fn every_sample_lands_within_a_quarter_of_the_reference_tokenizers() {
        for (name, text, o200k, cl100k) in REFERENCE {
            let estimated = estimate(text) as f64;
            let ratio = estimated / midpoint(*o200k, *cl100k);
            assert!(
                (0.75..=1.25).contains(&ratio),
                "{name}: estimated {estimated} against o200k {o200k} / cl100k {cl100k} ({ratio:.2})"
            );
        }
    }

    #[test]
    fn english_code_and_tool_output_land_within_a_tenth_of_the_tokenizers() {
        // Where the vocabularies agree — English, code, logs, JSON, diffs —
        // the estimate agrees with both, in sum.
        let (mut estimated, mut reference) = (0, 0);
        for (_, text, o200k, _) in &REFERENCE[..12] {
            estimated += estimate(text);
            reference += o200k;
        }
        let ratio = estimated as f64 / reference as f64;
        assert!(
            (0.9..=1.1).contains(&ratio),
            "{estimated} against {reference} ({ratio:.3})"
        );
    }

    #[test]
    fn a_fresh_calibration_leaves_estimates_alone() {
        assert_eq!(Calibration::default().apply(1234), 1234);
        assert_eq!(Calibration::default().per_mille(), 1000);
    }

    #[test]
    fn a_report_teaches_the_providers_ratio() {
        let mut calibration = Calibration::default();
        calibration.observe(1_000, 1_250);
        assert_eq!(calibration.per_mille(), 1250);
        assert_eq!(calibration.apply(2_000), 2_500);
    }

    #[test]
    fn the_newest_report_replaces_the_last() {
        // Each request carries every one before it: the newest ratio is the
        // one that describes the conversation now.
        let mut calibration = Calibration::default();
        calibration.observe(1_000, 1_250);
        calibration.observe(1_000, 800);
        assert_eq!(calibration.apply(1_000), 800);
    }

    #[test]
    fn a_report_on_a_tiny_request_teaches_nothing() {
        // Framing tokens dominate a few-token request; its ratio says
        // nothing about the tokenizer.
        let mut calibration = Calibration::default();
        calibration.observe(10, 30);
        assert_eq!(calibration, Calibration::NEUTRAL);
    }

    #[test]
    fn an_implausible_report_is_ignored() {
        // A usage frame a shim got wrong (a cached-only count, a session
        // total) must not skew every estimate after it.
        let mut calibration = Calibration::default();
        calibration.observe(10_000, 1_250);
        calibration.observe(10_000, 1_000_000);
        calibration.observe(10_000, 0);
        assert_eq!(calibration, Calibration::NEUTRAL);
    }

    #[test]
    fn a_count_scales_like_a_total() {
        let mut calibration = Calibration::default();
        calibration.observe(1_000, 1_250);
        assert_eq!(calibration.apply_count(1_000), 1_250);
        assert_eq!(Calibration::NEUTRAL.apply_count(7), 7);
    }

    #[test]
    fn applying_saturates_rather_than_overflowing() {
        let mut calibration = Calibration::default();
        calibration.observe(1_000, 2_000);
        assert_eq!(calibration.apply(u64::MAX), u64::MAX);
    }
}

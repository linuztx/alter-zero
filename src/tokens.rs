//! Token estimation: how many tokens a text costs, without a tokenizer, and
//! the correction the provider's own usage reports teach it. See
//! `docs/token-estimate.md`.
//!
//! **[`estimate`]** is provider-agnostic and vocabulary-free. Every model
//! this app talks to splits text the same way before its vocabulary is ever
//! consulted — the GPT-style pre-tokenizer that OpenAI, Anthropic, Llama,
//! Qwen, DeepSeek and Mistral encoders all share: a word with the space or
//! single symbol in front of it is one piece, a case change starts another,
//! digits go three at a time, a run of whitespace is one piece, punctuation
//! runs merge in pairs. Counting those pieces gets almost all of the way to
//! a real count, because a vocabulary of 100k–200k entries holds nearly
//! every common piece whole; what is left is a little cost per letter for
//! the long and the rare. So one pass over the characters, classifying each
//! and pricing the pieces it closes, estimates within a few percent of a
//! real encoder on code, prose, JSON, diffs and shell output (measured
//! against `o200k_base`; `docs/token-estimate.md` has the numbers) — with no
//! tables to load, nothing resident, and nothing tied to one provider.
//!
//! What a heuristic cannot know — that this provider's tokenizer makes 15%
//! more tokens of the same text than the one it was fitted against, that the
//! request carries tool schemas the estimate never sees — the provider says
//! itself, once a round, in its usage report. **[`Calibration`]** learns the first from those
//! reports and **[`ContextMeter`]** keeps the second: the context gauge reads
//! the provider's own count of the last request, plus the calibrated
//! estimate of whatever changed since.

/// Costs are kept in hundredths of a token, so the fractional costs of many
/// small pieces add up exactly and only the total is rounded.
type Cost = u64;

/// One token, in [`Cost`] units.
const TOKEN: Cost = 100;

/// What one CJK ideograph, kana or hangul syllable costs: a little under a
/// token, since a vocabulary holds the common two-character words whole.
const WIDE_LETTER: Cost = 70;

/// A stretch of text between whitespace this long or longer may read as
/// random data (see [`Stretch`]).
const BLOB_MIN_CHARS: Cost = 16;

/// How often, in percent of its characters, a random stretch switches
/// between lower case, upper case and digits — base64 does on about half.
const BLOB_SWITCH_PERCENT: Cost = 30;

/// What one character of random data costs: no vocabulary compresses it,
/// so about a token per one and a half characters.
const BLOB_CHAR: Cost = 68;

/// The number of tokens `text` is estimated to cost — never zero for text
/// that is not empty, since a single character is a token. Deterministic,
/// one pass, no allocation. See the module docs for what it models.
#[must_use]
pub fn estimate(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut scanner = Scanner::new(text);
    while let Some((_, class)) = scanner.peek() {
        match class {
            Class::Space | Class::Newline => scanner.whitespace(),
            Class::Lower | Class::Upper => scanner.word(),
            Class::Digit => scanner.digits(),
            Class::Wide => scanner.letters(Class::Wide, WIDE_LETTER),
            kind @ Class::Script(per_letter) => scanner.letters(kind, per_letter),
            Class::Symbol => scanner.symbols(),
        }
    }
    scanner.finish()
}

/// What the scanner sees in a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A lowercase Latin letter, accented ones included.
    Lower,
    /// An uppercase Latin letter.
    Upper,
    /// A digit.
    Digit,
    /// Horizontal whitespace.
    Space,
    /// A line break.
    Newline,
    /// Punctuation, a symbol, an emoji.
    Symbol,
    /// A CJK ideograph, kana, hangul syllable or full-width form.
    Wide,
    /// A letter of a script vocabularies hold fewer whole words of, with
    /// what one of its letters costs.
    Script(Cost),
}

/// The class of an ASCII character.
const fn ascii_class(byte: u8) -> Class {
    match byte {
        b'a'..=b'z' => Class::Lower,
        b'A'..=b'Z' => Class::Upper,
        b'0'..=b'9' => Class::Digit,
        b'\n' | b'\r' => Class::Newline,
        b' ' | b'\t' | 0x0b | 0x0c => Class::Space,
        _ => Class::Symbol,
    }
}

/// [`ascii_class`] of every ASCII character, looked up rather than matched —
/// the scanner's hot path, since most of what a session carries is ASCII.
const ASCII_CLASS: [Class; 128] = {
    let mut table = [Class::Symbol; 128];
    let mut byte: u8 = 0;
    while byte < 128 {
        table[byte as usize] = ascii_class(byte);
        byte += 1;
    }
    table
};

/// The class of any character.
fn class(c: char) -> Class {
    if c.is_ascii() {
        return ASCII_CLASS[c as usize];
    }
    let code = u32::from(c);
    match code {
        0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3100..=0x31FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF
        | 0x2_0000..=0x3_FFFF => Class::Wide,
        0x85 | 0x2028 | 0x2029 => Class::Newline,
        _ if c.is_whitespace() => Class::Space,
        0x00C0..=0x024F | 0x1E00..=0x1EFF if c.is_alphabetic() => {
            if c.is_uppercase() {
                Class::Upper
            } else {
                Class::Lower
            }
        }
        _ if c.is_alphabetic() || is_combining_mark(code) => Class::Script(script_letter(code)),
        _ if c.is_numeric() => Class::Digit,
        _ => Class::Symbol,
    }
}

/// The combining marks that are not `Alphabetic` themselves but belong to
/// the letter before them — accents written as separate code points, vowel
/// points and signs.
const fn is_combining_mark(code: u32) -> bool {
    matches!(
        code,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x064B..=0x065F
            | 0x0900..=0x0903
            | 0x093A..=0x094F
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

/// What one letter of a non-Latin script costs. Greek and Cyrillic words
/// come out at a token per four letters or so, Arabic and Hebrew per three,
/// the Indic scripts a little denser, and the scripts written without
/// spaces (Thai, Lao, Khmer, Myanmar) densest of all.
const fn script_letter(code: u32) -> Cost {
    match code {
        0x0370..=0x058F => 24,
        0x0590..=0x08FF => 32,
        0x0900..=0x0DFF => 36,
        _ => 45,
    }
}

/// Whether `c` is a vowel, as far as telling a word from random letters
/// goes — an accented letter counts as one.
fn is_vowel(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y') || !c.is_ascii()
}

/// What a word piece follows, which decides how much of it a vocabulary
/// holds whole: ` function` is one token in every modern vocabulary,
/// `/function` and `.function` less often.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lead {
    /// Nothing — the start of the text, a line, or a piece of another kind.
    None,
    /// A space.
    Space,
    /// One symbol (`.method`, `/usr`, `_name`).
    Symbol,
}

/// What a word piece of `letters` letters costs. A common word is one token
/// however it is led; past a few letters each letter adds a little, more
/// after a symbol than after a space. A word with no vowel at all is an
/// abbreviation when short and random letters when long — nothing a
/// vocabulary learned whole (`lrwxrwxrwx` is six tokens).
fn word_cost(letters: Cost, vowels: bool, lead: Lead) -> Cost {
    let past = |n: Cost| letters.saturating_sub(n);
    if !vowels {
        return TOKEN + 35 * past(2) + 45 * past(5);
    }
    match lead {
        Lead::Space => TOKEN + 4 * past(4) + 4 * past(7),
        Lead::None => TOKEN + 6 * past(4) + 9 * past(7),
        Lead::Symbol => TOKEN + 10 * past(3) + 10 * past(8),
    }
}

/// What a run of symbols worth `units` costs (an ASCII symbol is one unit,
/// any other two): one or two merge into a single token (`();`, `=>`,
/// `":"`), longer mixes cost about a token per two.
fn symbol_cost(units: Cost) -> Cost {
    TOKEN + 20 * units.saturating_sub(2) + 25 * units.saturating_sub(4)
}

/// What one more copy of the symbol just before it costs, when repeating it
/// is cheap: the rules and banners vocabularies learned whole (twenty-four
/// `-` are two tokens) almost nothing, any other ASCII symbol a quarter of a
/// token. `None` for a symbol that does not compress when repeated (`│`,
/// `╌` — a token each).
const fn repeat_cost(c: char) -> Option<Cost> {
    match c {
        '-' | '=' | '_' | '*' | '#' | '~' | '.' | '/' | '+' | ':' | ';' | '!' | '?' | '^' | '<'
        | '>' | '%' | '@' | '─' | '━' | '═' | '…' => Some(8),
        _ if c.is_ascii() => Some(25),
        _ => None,
    }
}

/// A maximal stretch of text between whitespace, watched for one thing:
/// whether it reads as random data — a key, a hash in base64, an encoded
/// blob — which no vocabulary compresses, so it costs about
/// [`BLOB_CHAR`] per character whatever its pieces add up to. Random data
/// mixes lower case, upper case and digits and keeps switching between
/// them; an identifier, a path or a URL does not.
#[derive(Debug, Default)]
struct Stretch {
    /// What its pieces cost.
    cost: Cost,
    /// Its characters.
    chars: Cost,
    /// Which of lower case (1), upper case (2) and digits (4) it holds.
    seen: u8,
    /// How often it switched between them.
    switches: Cost,
    /// The kind of its last character, 0 for any other.
    last: u8,
}

impl Stretch {
    /// `count` more characters of `class` — a run, noted once it ends.
    fn extend(&mut self, class: Class, count: Cost) {
        if count == 0 {
            return;
        }
        let kind = match class {
            Class::Lower => 1,
            Class::Upper => 2,
            Class::Digit => 4,
            _ => 0,
        };
        self.seen |= kind;
        if kind != 0 && self.last != 0 && kind != self.last {
            self.switches += 1;
        }
        self.last = kind;
        self.chars += count;
    }

    /// The stretch's cost, now that it has ended — and a fresh stretch.
    fn settle(&mut self) -> Cost {
        let random = self.chars >= BLOB_MIN_CHARS
            && self.seen == 0b111
            && self.switches * 100 >= self.chars * BLOB_SWITCH_PERCENT;
        let cost = if random {
            self.cost.max(self.chars * BLOB_CHAR)
        } else {
            self.cost
        };
        *self = Self::default();
        cost
    }
}

/// [`estimate`]'s one pass: the position, the running total, and what the
/// piece about to start follows.
struct Scanner<'a> {
    text: &'a str,
    position: usize,
    total: Cost,
    stretch: Stretch,
    /// What the next word piece follows.
    lead: Lead,
    /// The last piece ended in a space a symbol run cannot share with the
    /// word after it (` (foo` is two pieces, `(foo` one).
    after_space: bool,
    /// The last piece was a symbol run, which line breaks straight after it
    /// ride with (`{` and its newline are one piece).
    after_symbols: bool,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            position: 0,
            total: 0,
            stretch: Stretch::default(),
            lead: Lead::None,
            after_space: false,
            after_symbols: false,
        }
    }

    /// The character at the position and its class, not consumed.
    fn peek(&self) -> Option<(char, Class)> {
        let byte = *self.text.as_bytes().get(self.position)?;
        if byte.is_ascii() {
            return Some((char::from(byte), ASCII_CLASS[usize::from(byte)]));
        }
        let c = self.text[self.position..].chars().next()?;
        Some((c, class(c)))
    }

    /// The character at the position, when it is of `kind` — consumed. The
    /// caller notes the run on the stretch once it ends.
    fn take(&mut self, kind: Class) -> Option<char> {
        let (c, _) = self.peek().filter(|&(_, class)| class == kind)?;
        self.position += c.len_utf8();
        Some(c)
    }

    /// A piece other than a word ended: nothing leads the next one.
    fn ended_piece(&mut self) {
        self.lead = Lead::None;
        self.after_space = false;
        self.after_symbols = false;
    }

    /// A run of whitespace. Line breaks are one piece (with any spaces
    /// before them), unless they directly follow a symbol run, which they
    /// ride with; the spaces after the last break are one more, less the
    /// one the next word or symbol run takes as its own first character —
    /// a number takes none, so the run splits in two before it.
    fn whitespace(&mut self) {
        self.total += self.stretch.settle();
        let mut line_break = false;
        let mut riding = self.after_symbols;
        let mut trailing: Cost = 0;
        while let Some((c, class)) = self.peek() {
            match class {
                Class::Newline => {
                    line_break |= !riding;
                    trailing = 0;
                }
                Class::Space => {
                    riding = false;
                    trailing += 1;
                }
                _ => break,
            }
            self.position += c.len_utf8();
        }
        if line_break {
            self.total += TOKEN;
        }
        self.total += match self.peek().map(|(_, class)| class) {
            None => TOKEN * trailing.min(1),
            Some(Class::Digit) => TOKEN * trailing.min(2),
            Some(_) => TOKEN * Cost::from(trailing >= 2),
        };
        self.after_space = trailing > 0;
        self.after_symbols = false;
        self.lead = if trailing > 0 {
            Lead::Space
        } else {
            Lead::None
        };
    }

    /// A word piece: capitals then lower case, so `HTTPServer` is one piece
    /// and `getUserById` four.
    fn word(&mut self) {
        let (mut letters, mut vowels) = (0, false);
        for kind in [Class::Upper, Class::Lower] {
            let mut run = 0;
            while let Some(c) = self.take(kind) {
                run += 1;
                vowels |= is_vowel(c);
            }
            self.stretch.extend(kind, run);
            letters += run;
        }
        self.stretch.cost += word_cost(letters, vowels, self.lead);
        self.ended_piece();
    }

    /// A run of digits: a token per three.
    fn digits(&mut self) {
        let mut digits: Cost = 0;
        while self.take(Class::Digit).is_some() {
            digits += 1;
        }
        self.stretch.extend(Class::Digit, digits);
        self.stretch.cost += TOKEN * digits.div_ceil(3);
        self.ended_piece();
    }

    /// A run of letters priced per letter — CJK, or a non-Latin script.
    fn letters(&mut self, kind: Class, per_letter: Cost) {
        let mut letters: Cost = 0;
        while self.take(kind).is_some() {
            letters += 1;
        }
        self.stretch.extend(kind, letters);
        self.stretch.cost += (per_letter * letters).max(TOKEN);
        self.ended_piece();
    }

    /// A run of symbols: priced by [`symbol_cost`], a cheap repeat by
    /// [`repeat_cost`], an emoji at two tokens and its joiners, modifiers
    /// and variation selectors at one. One ASCII symbol straight before a
    /// letter, with no space before it, is that word's first character and
    /// costs nothing of its own.
    fn symbols(&mut self) {
        let (mut chars, mut units, mut flat): (Cost, Cost, Cost) = (0, 0, 0);
        let mut previous = None;
        while let Some(c) = self.take(Class::Symbol) {
            match u32::from(c) {
                0x1_F3FB..=0x1_F3FF | 0x200D | 0xFE00..=0xFE0F => flat += TOKEN,
                0x1_0000.. => flat += 2 * TOKEN,
                _ => match repeat_cost(c).filter(|_| previous == Some(c)) {
                    Some(cost) => flat += cost,
                    None => units += if c.is_ascii() { 1 } else { 2 },
                },
            }
            previous = Some(c);
            chars += 1;
        }
        self.stretch.extend(Class::Symbol, chars);
        let before_letter = self.peek().is_some_and(|(_, class)| {
            matches!(
                class,
                Class::Lower | Class::Upper | Class::Wide | Class::Script(_)
            )
        });
        if chars == 1 && units == 1 && !self.after_space && before_letter {
            self.ended_piece();
            self.lead = Lead::Symbol;
            return;
        }
        self.stretch.cost += flat + if units > 0 { symbol_cost(units) } else { 0 };
        self.ended_piece();
        self.after_symbols = true;
    }

    fn finish(mut self) -> usize {
        self.total += self.stretch.settle();
        let tokens = ((self.total + TOKEN / 2) / TOKEN).max(1);
        usize::try_from(tokens).unwrap_or(usize::MAX)
    }
}

/// A change in the context smaller than this many estimated tokens teaches
/// the [`Calibration`] nothing: below it, the few tokens a chat template
/// frames each message with are a real share of the change.
const SAMPLE_MIN_ESTIMATE: u64 = 256;

/// The provider-to-estimate ratios, in thousandths, a single change may show
/// and still be taken as the tokenizer's: anything outside measured
/// something else — a tool set that connected, a system prompt that changed.
const SAMPLE_PERMILLE: std::ops::RangeInclusive<u64> = 400..=2_500;

/// The ratios, in thousandths, a [`Calibration`] corrects by at most — what
/// one modern tokenizer can differ from another by.
const RATIO_PERMILLE: std::ops::RangeInclusive<u64> = 500..=2_000;

/// The estimated tokens a [`Calibration`] weighs before it halves its older
/// samples away, so it follows what the session carries now.
const CALIBRATION_WINDOW: u64 = 65_536;

/// How the provider's tokens compare with [`estimate`]'s, learned from its
/// usage reports. A sample is one change in the context between two reports
/// — the provider's count of the change against the estimate of it — so
/// whatever both reports carried unchanged (the tool schemas, the system
/// prompt, a chat template's framing) cancels out, and what is left is the
/// tokenizer's own density on this session's text.
///
/// The ratio is the samples' sum over the estimates' — big changes weigh
/// more than small ones — with the older samples halved away once the
/// estimates pass 64k tokens, so it follows what the session carries now.
/// Until the first sample it is one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Calibration {
    /// The provider's tokens over the samples taken.
    provider: u64,
    /// The estimate of the same changes.
    estimated: u64,
}

impl Calibration {
    /// The provider's tokens per estimated token, in thousandths, held to
    /// 500–2000 (what one modern tokenizer can differ from another by) —
    /// 1000 until a sample has been taken.
    #[must_use]
    pub fn ratio_permille(&self) -> u64 {
        if self.estimated == 0 {
            return 1000;
        }
        (self.provider.saturating_mul(1000) / self.estimated)
            .clamp(*RATIO_PERMILLE.start(), *RATIO_PERMILLE.end())
    }

    /// Learn from one change the provider counted `provider` tokens for and
    /// the estimate `estimated` — unless it is under 256 estimated tokens,
    /// too small to measure, or its ratio is outside 0.4–2.5, which no
    /// tokenizer explains.
    pub fn learn(&mut self, provider: u64, estimated: u64) {
        if estimated < SAMPLE_MIN_ESTIMATE
            || !SAMPLE_PERMILLE.contains(&(provider.saturating_mul(1000) / estimated))
        {
            return;
        }
        self.provider = self.provider.saturating_add(provider);
        self.estimated = self.estimated.saturating_add(estimated);
        while self.estimated > CALIBRATION_WINDOW {
            self.provider /= 2;
            self.estimated /= 2;
        }
    }

    /// `estimate`, corrected into the provider's tokens.
    #[must_use]
    pub fn correct(&self, estimate: u64) -> u64 {
        (estimate.saturating_mul(self.ratio_permille()) + 500) / 1000
    }

    /// [`correct`](Self::correct) for a count kept as a `usize`.
    #[must_use]
    pub fn correct_count(&self, estimate: usize) -> usize {
        let corrected = self.correct(u64::try_from(estimate).unwrap_or(u64::MAX));
        usize::try_from(corrected).unwrap_or(usize::MAX)
    }
}

/// The context size of one conversation, kept in the provider's tokens: what
/// its last usage report counted for the request, plus the calibrated
/// estimate of whatever changed since. The context gauge reads it, the
/// auto-compact trigger acts on it, and a compaction records it.
///
/// A report sets the **base** — the provider's count beside the estimate of
/// the same context. Text that joins the conversation after it adds its
/// corrected estimate ([`reading_after`](Self::reading_after)); a history
/// that shrank or was replaced (a compaction, a rewind, `/clear`, a resumed
/// session) takes the corrected estimate of what left away
/// ([`reading`](Self::reading)), so the share of the count the estimate never
/// saw — tool schemas, the provider's own framing — stays in. Each report
/// also teaches the [`Calibration`], against the report before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextMeter {
    /// Where readings start from: a count, and the estimate of the same
    /// context. `None` until the first report or rebase.
    base: Option<Count>,
    /// The last report and the pictures its context held — what the next
    /// report learns against. `None` until one, and after a model switch.
    last_report: Option<(Count, usize)>,
    calibration: Calibration,
}

/// A context's size two ways: counted (by the provider, or carried over from
/// a count) and estimated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Count {
    tokens: u64,
    estimate: u64,
}

impl ContextMeter {
    /// A usage report: the provider counted `tokens` for a request whose
    /// context the estimate puts at `estimate`, holding `images` pictures.
    /// It becomes the base, and teaches the calibration the change since the
    /// last report — unless a picture came or went between them, since a
    /// picture is estimated at a flat rate the provider does not share.
    pub fn measured(&mut self, tokens: u64, estimate: u64, images: usize) {
        if let Some((last, last_images)) = self.last_report
            && last_images == images
            && tokens > last.tokens
            && estimate > last.estimate
        {
            self.calibration
                .learn(tokens - last.tokens, estimate - last.estimate);
        }
        let count = Count { tokens, estimate };
        self.last_report = Some((count, images));
        self.base = Some(count);
    }

    /// The context size of a conversation the estimate now puts at
    /// `estimate`: the base, moved by the corrected estimate of the change
    /// since — never below zero. With no base, the corrected estimate alone.
    #[must_use]
    pub fn reading(&self, estimate: u64) -> u64 {
        let Some(base) = self.base else {
            return self.calibration.correct(estimate);
        };
        if estimate >= base.estimate {
            base.tokens
                .saturating_add(self.calibration.correct(estimate - base.estimate))
        } else {
            base.tokens
                .saturating_sub(self.calibration.correct(base.estimate - estimate))
        }
    }

    /// The context size once `added` estimated tokens have joined the
    /// context the base measured.
    #[must_use]
    pub fn reading_after(&self, added: u64) -> u64 {
        self.base
            .map_or(0, |base| base.tokens)
            .saturating_add(self.calibration.correct(added))
    }

    /// The history moved under the base (a rewind, a compaction, a resumed
    /// session): take the reading for its new estimate as the base, so the
    /// text that joins next is added to it.
    pub fn rebase(&mut self, estimate: u64) {
        self.base = Some(Count {
            tokens: self.reading(estimate),
            estimate,
        });
    }

    /// The conversation moved to another model, whose tokenizer the ratio
    /// says nothing about — and whose first report is no sample against the
    /// last model's. Readings carry on from the base until it reports.
    pub fn model_changed(&mut self) {
        self.calibration = Calibration::default();
        self.last_report = None;
    }

    /// What the provider's reports have taught so far.
    #[must_use]
    pub const fn calibration(&self) -> Calibration {
        self.calibration
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
    fn any_text_costs_at_least_one_token() {
        for text in ["a", " ", "\n", ".", "\t\t"] {
            assert_eq!(estimate(text), 1, "{text:?}");
        }
    }

    #[test]
    fn common_words_cost_a_token_each() {
        // o200k_base counts, matched exactly: a space-led common word is one
        // token, and so is the punctuation after it.
        assert_eq!(estimate("hello world"), 2);
        assert_eq!(estimate("Hello, world!"), 4);
        assert_eq!(estimate("The quick brown fox jumps over the lazy dog."), 10);
    }

    #[test]
    fn identifiers_split_where_tokenizers_split_them() {
        // A case change and an underscore each start a new piece.
        assert_eq!(estimate("getUserById"), 4);
        assert_eq!(estimate("snake_case_name"), 3);
    }

    #[test]
    fn digits_cost_a_token_per_three() {
        assert_eq!(estimate("2026"), 2);
        assert_eq!(estimate("1234567"), 3);
    }

    #[test]
    fn a_run_of_spaces_is_one_token_however_long() {
        let text = format!("a{}b", " ".repeat(40));
        assert_eq!(estimate(&text), 3);
    }

    #[test]
    fn a_line_break_after_punctuation_rides_with_it() {
        // `{` + newline, the indentation, and `;` + newline are a piece each.
        assert_eq!(estimate("fn main() {\n    body();\n}"), 8);
    }

    #[test]
    fn a_repeated_rule_costs_a_few_tokens_however_long() {
        assert!(
            estimate(&"-".repeat(48)) <= 6,
            "{}",
            estimate(&"-".repeat(48))
        );
        assert!(
            estimate(&"─".repeat(32)) <= 6,
            "{}",
            estimate(&"─".repeat(32))
        );
        assert!(estimate(&"=".repeat(10_000)) < 1_000);
    }

    #[test]
    fn letters_with_no_vowel_cost_like_random_letters() {
        // A permissions string is no word any vocabulary learned whole.
        assert_eq!(estimate("lrwxrwxrwx"), 6);
    }

    #[test]
    fn a_random_blob_costs_about_a_token_per_one_and_a_half_characters() {
        let blob = "kJ8sdQx2Lm9PzR4tYw7VbN3cXf6GhK1q";
        let tokens = estimate(blob);
        assert!((21..=32).contains(&tokens), "{tokens}");
    }

    #[test]
    fn an_emoji_costs_two_tokens_and_cjk_about_three_for_four_characters() {
        assert_eq!(estimate("🚀"), 2);
        assert_eq!(estimate("数据压缩"), 3);
    }

    #[test]
    fn the_estimate_grows_with_the_text() {
        assert!(estimate("a much longer string of several words") > estimate("a"));
        let text = "def main():\n    print('hi')";
        assert_eq!(estimate(text), estimate(text), "deterministic");
    }

    /// Hand-written samples of what a session carries, each with the count
    /// the exact `o200k_base` byte-pair encoder gave it — measured with the
    /// BPE counter this module replaced, before it was removed. They hold
    /// the estimate to the encoding the heuristic was fitted against.
    fn reference_samples() -> Vec<(&'static str, usize, &'static str)> {
        vec![
            (
                "prose",
                87,
                r##"The status line counts tokens while a reply streams, and the footer shows how much of the context window the conversation already fills. Neither number has to be exact to be useful: the provider reports its own accounting at the end of every round, and the estimate only needs to be close enough to say whether a long paste or a large tool output just cost a lot. When the report arrives, the app trusts it over anything it guessed.
"##,
            ),
            (
                "rust",
                131,
                r##"/// Re-seat the gauge on the estimate when the history moved under it.
pub fn sync_context_used(&mut self) {
    let signature = (self.history_generation, self.history.len());
    if self.context_signature == Some(signature) {
        return;
    }
    let estimate = self.estimate_context_tokens();
    self.context_used = match estimate {
        0 => 0,
        tokens => self.context_meter.reading(tokens),
    };
    self.context_signature = Some(signature);
}

fn parse_port(text: &str) -> Result<u16, std::num::ParseIntError> {
    text.trim().parse::<u16>()
}
"##,
            ),
            (
                "python",
                121,
                r##"import json
from pathlib import Path


def load_settings(path: Path) -> dict:
    """Read the settings file, falling back to defaults when it is missing."""
    if not path.exists():
        return {"theme": "mocha", "tips": True, "retries": 3}
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    return {**DEFAULTS, **data}


for index, name in enumerate(sorted(Path(".").glob("*.toml"))):
    print(f"{index:>3}: {name}")
"##,
            ),
            (
                "compact json",
                87,
                r##"{"type":"function","function":{"name":"edit","description":"Replace one exact string in a file.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"The absolute path to the file to modify."},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean","default":false}},"required":["path","old_string","new_string"]}}}"##,
            ),
            (
                "pretty json",
                74,
                r##"{
  "projects": {
    "/home/user/alter-zero": {
      "allow_commands": [
        "cargo test *",
        "git status",
        "npm run"
      ],
      "mode": "edit"
    }
  },
  "version": 2,
  "enabled": true,
  "last_check_day": 20397
}
"##,
            ),
            (
                "ls -la",
                233,
                r##"total 1244
drwxr-xr-x 16 root root    4096 Nov  5 15:13 .
drwxr-xr-x  3 root root    4096 Oct 31 01:17 ..
-rw-r--r--  1 root root   18342 Oct 31 01:17 AGENTS.md
-rw-r--r--  1 root root  301442 Oct 31 01:17 CHANGELOG.md
-rw-r--r--  1 root root  125573 Nov  5 15:13 Cargo.lock
-rw-r--r--  1 root root   15204 Oct 31 01:17 Cargo.toml
drwxr-xr-x  2 root root    4096 Oct 31 01:17 assets
-rwxr-xr-x  1 root root   21934 Oct 31 01:17 install.sh
lrwxrwxrwx  1 root root      11 Oct 31 01:17 latest -> release/0.18
"##,
            ),
            (
                "diff",
                153,
                r##"diff --git a/src/app/compact.rs b/src/app/compact.rs
index 3f2a1b9..8c4d7e2 100644
--- a/src/app/compact.rs
+++ b/src/app/compact.rs
@@ -176,7 +176,9 @@ impl App {
     /// Re-seat the gauge on the estimate (the last usage frame no
     /// longer describes the derived context).
     pub(super) fn refresh_context_used(&mut self) {
-        self.context_used = self.estimate_context_tokens();
+        let estimate = self.estimate_context_tokens();
+        self.context_used = self.context_meter.reading(estimate);
+        self.context_signature = Some(self.history_signature());
     }
"##,
            ),
            (
                "markdown",
                111,
                r##"## Settings

| Row | Values | Default |
| --- | ------ | ------- |
| **Auto compact** | `true`, `false` | `true` |
| **Error retry** | `0`–`10` | `3` |
| **Ask timeout** | `5m`, `10m`, `never` | `10m` |

- Run `cargo test --lib app::` after editing `src/app/`.
- See [the design notes](docs/design.md#known-limitations) for the rest.
"##,
            ),
            (
                "base64",
                252,
                r##"IMyuySb915oZaZ2iK2zeTCvxqb+/wnSaylAvKwZOROXzVt4MfNIRepDg9C96KdWPYBx/RIvf+24L
ye6zhLy+wuaReKgNeXJvn1jVmCD8OhvZPge1nILN/fvQDri5xDOAEtlmS8BkCidyiLFAhM+s8+1F
U56u2tFBZkSkuRWi1f+fmbQzEhZuC+g9JBGUQXdSIG7/SwvU5hMkolf8AgtkBNVefm+4mF7rHb7E
qj7xf2eYEgKjIJOZQ+TLKNmq0Hb1z0+LtUZrQ1ivdGll9ag1UrtfFg0I1YAkbYTI5pCJ3UmfoXLF
hPSLdNZVBLU6hp9d+rKHTMVNudMjCkSd/NM/q1WW4TOj2aMA2x6YbDIu
"##,
            ),
            (
                "uuids",
                140,
                r##"0b72c1fa-61ef-44d1-8f44-f1f0552b6c79
d01a4449-3e2a-457c-b618-399e80f44108
5cb70dad-516b-4781-9005-db0c2e74d6c6
4382f5d1-7e18-482d-9461-306c460ac68c
6f89d864-4c27-473a-adc9-de63e8618951
9f674377-17ae-415b-b056-eb22d0401c48
"##,
            ),
            (
                "log",
                143,
                r##"2026-11-05T15:13:02.118Z INFO  alter_zero::llm::backend: request sent model=gpt-5.5 round=3 bytes=48213
2026-11-05T15:13:04.902Z WARN  alter_zero::llm::retry: 429 Too Many Requests, retrying in 2s (attempt 1/3)
2026-11-05T15:13:07.336Z INFO  alter_zero::llm::backend: usage input=38114 output=1207 cached=31604
error[E0308]: mismatched types --> src/app/status.rs:310:29
"##,
            ),
            (
                "cjk",
                43,
                r##"上下文窗口快满时，应用会自动压缩对话，把较早的内容总结成一段简短的说明。設定は作業ディレクトリごとに保存されます。
"##,
            ),
            (
                "cyrillic",
                27,
                r##"Когда окно контекста почти заполнено, приложение само сжимает разговор и заменяет старые сообщения кратким пересказом.
"##,
            ),
            (
                "emoji",
                41,
                r##"Shipped 🚀 the fix 🐛 — tests ✅, docs 📝, and one regression ⚠️ left. Thanks 🙏 everyone 👨‍👩‍👧‍👦!
"##,
            ),
        ]
    }

    #[test]
    fn every_reference_sample_lands_within_fifteen_percent() {
        for (name, reference, text) in reference_samples() {
            let tokens = estimate(text);
            let error = tokens.abs_diff(reference) as f64 / reference as f64;
            assert!(
                error <= 0.15,
                "{name}: estimated {tokens}, the encoder counted {reference}"
            );
        }
    }

    #[test]
    fn the_samples_together_land_within_three_percent() {
        let (mut estimated, mut reference) = (0, 0);
        for (_, counted, text) in reference_samples() {
            estimated += estimate(text);
            reference += counted;
        }
        let error = estimated.abs_diff(reference) as f64 / reference as f64;
        assert!(error <= 0.03, "estimated {estimated}, counted {reference}");
    }

    #[test]
    fn an_unlearned_calibration_corrects_nothing() {
        let calibration = Calibration::default();
        assert_eq!(calibration.ratio_permille(), 1000);
        assert_eq!(calibration.correct(1234), 1234);
    }

    #[test]
    fn a_provider_denser_than_the_estimate_scales_estimates_up() {
        let mut calibration = Calibration::default();
        calibration.learn(1_200, 1_000);
        assert_eq!(calibration.ratio_permille(), 1_200);
        assert_eq!(calibration.correct(1_000), 1_200);
        assert_eq!(calibration.correct(10), 12);
    }

    #[test]
    fn a_change_too_small_to_measure_teaches_nothing() {
        // A few hundred tokens are where a chat template's per-message
        // framing stops drowning the content (docs/token-estimate.md).
        let mut calibration = Calibration::default();
        calibration.learn(300, 100);
        assert_eq!(calibration.ratio_permille(), 1000);
    }

    #[test]
    fn a_change_no_tokenizer_explains_teaches_nothing() {
        // Ten times the estimate is a tool set connecting or a picture, not
        // a denser tokenizer.
        let mut calibration = Calibration::default();
        calibration.learn(10_000, 1_000);
        calibration.learn(100, 1_000);
        assert_eq!(calibration.ratio_permille(), 1000);
    }

    #[test]
    fn the_ratio_is_held_to_what_a_tokenizer_can_differ_by() {
        let mut calibration = Calibration::default();
        calibration.learn(2_400, 1_000);
        assert_eq!(calibration.ratio_permille(), 2_000);
        let mut calibration = Calibration::default();
        calibration.learn(450, 1_000);
        assert_eq!(calibration.ratio_permille(), 500);
    }

    #[test]
    fn the_ratio_follows_what_the_session_is_doing_now() {
        // Recent samples outweigh old ones: a session that moved from prose
        // to a language the tokenizer is denser on is soon measured by it.
        let mut calibration = Calibration::default();
        for _ in 0..20 {
            calibration.learn(10_000, 10_000);
        }
        for _ in 0..20 {
            calibration.learn(15_000, 10_000);
        }
        assert!(calibration.ratio_permille() >= 1_450, "{calibration:?}");
    }

    #[test]
    fn a_meter_with_no_frame_reads_the_estimate() {
        assert_eq!(ContextMeter::default().reading(500), 500);
    }

    #[test]
    fn a_frame_reads_exactly_what_the_provider_counted() {
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        assert_eq!(meter.reading(4_000), 5_000);
    }

    #[test]
    fn what_joins_after_a_frame_is_added_at_its_estimate() {
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        assert_eq!(meter.reading_after(100), 5_100);
        assert_eq!(meter.reading(4_100), 5_100);
    }

    #[test]
    fn a_shrink_takes_its_estimate_away_and_keeps_what_the_estimate_never_saw() {
        // 5k of the provider's count is what the estimate cannot see — tool
        // schemas, a template's framing — and still rides the next request
        // after a compaction or a rewind.
        let mut meter = ContextMeter::default();
        meter.measured(15_000, 10_000, 0);
        assert_eq!(meter.reading(2_000), 7_000);
    }

    #[test]
    fn a_reading_never_goes_below_zero() {
        let mut meter = ContextMeter::default();
        meter.measured(1_000, 5_000, 0);
        assert_eq!(meter.reading(100), 0);
    }

    #[test]
    fn consecutive_frames_teach_the_meter_its_providers_ratio() {
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        meter.measured(6_200, 5_000, 0);
        assert_eq!(meter.calibration().ratio_permille(), 1_200);
        assert_eq!(meter.reading_after(1_000), 7_400);
    }

    #[test]
    fn a_frame_carrying_a_new_picture_teaches_nothing() {
        // A picture is estimated at a flat rate the provider does not share,
        // so the change it rode in on measures the picture, not the text.
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        meter.measured(6_500, 5_000, 1);
        assert_eq!(meter.calibration().ratio_permille(), 1000);
        assert_eq!(meter.reading_after(1_000), 7_500);
    }

    #[test]
    fn a_model_switch_forgets_the_ratio_and_learns_nothing_across_it() {
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        meter.measured(6_200, 5_000, 0);
        meter.model_changed();
        assert_eq!(meter.calibration().ratio_permille(), 1000);
        assert_eq!(
            meter.reading_after(1_000),
            7_200,
            "still read from the last count"
        );
        // The next model counts the same conversation in another tokenizer:
        // its first frame against the last model's is no sample.
        meter.measured(9_000, 5_500, 0);
        assert_eq!(meter.calibration().ratio_permille(), 1000);
    }

    #[test]
    fn a_rebase_reads_on_from_where_the_history_moved_to() {
        let mut meter = ContextMeter::default();
        meter.measured(15_000, 10_000, 0);
        meter.rebase(2_000);
        assert_eq!(meter.reading(2_000), 7_000);
        assert_eq!(meter.reading_after(500), 7_500);
    }

    #[test]
    fn learning_compares_frames_whatever_was_rebased_between_them() {
        // A rewind between two frames moves the base, not the record of the
        // last count: the next frame still learns from the change since it.
        let mut meter = ContextMeter::default();
        meter.measured(5_000, 4_000, 0);
        meter.rebase(3_000);
        meter.measured(5_600, 4_500, 0);
        assert_eq!(meter.calibration().ratio_permille(), 1_200);
    }

    #[test]
    fn arbitrary_text_never_panics_and_never_reads_zero() {
        // A fixed LCG over an alphabet mixing every class the scanner tells
        // apart — ASCII, whitespace, symbols, accents, CJK, other scripts,
        // emoji with joiners and modifiers, lone combining marks.
        let alphabet: Vec<char> = "abcXYZ019 \t\n\r'\"!?.,;:-_/\\()[]{}<>=+*&%$#@^~|`\
             éüßñÉ日本語中文한국어русскийΩπλعربيहिन्दीไทย😀🦀👍🏽👨\u{200d}👩✨─═│\u{301}\u{fe0f}"
            .chars()
            .collect();
        let mut state: u64 = 0x243F_6A88_85A3_08D3;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        for _ in 0..500 {
            let len = 1 + next() % 120;
            let text: String = (0..len)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect();
            assert!(estimate(&text) >= 1, "{text:?}");
        }
    }
}

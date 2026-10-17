//! Grammar-accurate syntax highlighting for fenced code blocks
//! (`docs/markdown.md`), ported from codex's approach (`codex-rs/tui/src/render/
//! highlight.rs`).
//!
//! Codex colours code with [`syntect`] over the [`two_face`] grammar + theme
//! bundles (~250 TextMate grammars, embedded CSS/JS-in-HTML, etc.). This module
//! is the same stack — it **replaces** the earlier hand-rolled generic tokenizer
//! (which had no real HTML/CSS grammar and mis-parsed a CSS `#id` selector as a
//! `#` line comment). We use syntect's **oniguruma** regex engine, as codex does:
//! it's the one C build dependency in the crate, taken deliberately because the
//! pure-Rust `fancy-regex` alternative compiles each grammar into far heavier
//! state — syntect lazily compiles a grammar's regexes on first use and caches
//! them in the shared [`SyntaxSet`], and with fancy that reached 187 MB RSS
//! across 28 languages vs ~23 MB with onig (`Cargo.toml`).
//!
//! Unlike codex — which highlights a whole code block in one call
//! (`highlight_code_to_lines`) — this crate streams a reply line-by-line into
//! scrollback (CLAUDE.md invariant 2), so highlighting here is **incremental**:
//! [`Highlighter`] threads syntect's per-line [`ParseState`] + [`HighlightState`]
//! across [`Highlighter::line`] calls exactly as syntect's own `HighlightLines`
//! does internally. That keeps a growing code block O(new line) per chunk and,
//! because both states are `Clone`, lets the streaming renderer cheaply *peek*
//! an in-progress line without disturbing the state it resumes from.
//!
//! **Prefix-stable.** A completed line's segments are a pure function of the
//! lines fed before it (the carried parse/highlight state) plus the line's own
//! text — no lookahead past the line. Appending never recolours an
//! already-emitted line, so streaming-to-scrollback stays sound. (An
//! *in-progress* code line is withheld from scrollback entirely by the caller —
//! `ui::AssistantRenderer::in_code` — since a grammar parses the whole line at
//! once; only completed lines commit.)
//!
//! Styling lives with a **syntect theme**, not a `ui`-owned palette — the
//! tokenizer is no longer colour-agnostic, because a real grammar's scopes
//! carry far more distinction (tag vs attribute vs value) than a fixed
//! six-colour enum could. `Seg` therefore carries a resolved [`Style`]; `ui`
//! maps nothing. *Which* theme is a [`CodeTheme`] the caller names — one per
//! entry of the `/theme` catalog (`docs/theme.md`), each the syntect theme
//! that matches that entry's chrome palette, so the code in a reply and the
//! chrome around it come from one design system. The default is Catppuccin
//! Mocha, codex's dark default and this crate's since the syntect port.

use ratatui::style::{Color, Modifier, Style};
use std::collections::VecDeque;
use std::sync::{LazyLock, OnceLock};
use syntect::highlighting::{
    Color as SynColor, FontStyle, HighlightIterator, HighlightState, Highlighter as SynHighlighter,
    Style as SynStyle,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

/// One styled run of a code line (the grammar's scope resolved to a colour +
/// modifiers by the active theme).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    /// The run's text.
    pub text: String,
    /// Its resolved style (foreground colour, plus bold where the theme sets it;
    /// italic/underline are dropped — see `convert_style`).
    pub style: Style,
}

// -- Process-global singletons (built once, immutable) ------------------------

/// The ~250-language grammar database (newline-aware variants, required for the
/// per-line [`ParseState::parse_line`] here).
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// The syntax theme a code block or file cell is coloured with — one per
/// entry of the `/theme` catalog (`docs/theme.md`): each is the two-face
/// bundle's theme of the same family as the chrome palette it rides with,
/// so a reply's code and the chrome around it agree. [`CodeTheme::ALL`]
/// lists them in that catalog's order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CodeTheme {
    /// Catppuccin Mocha — the default (codex's dark default too).
    #[default]
    CatppuccinMocha,
    /// Catppuccin Macchiato.
    CatppuccinMacchiato,
    /// Catppuccin Frappé.
    CatppuccinFrappe,
    /// Catppuccin Latte — the light flavour.
    CatppuccinLatte,
    /// Atom's One Dark (bat's `TwoDark`).
    OneDark,
    /// Dracula.
    Dracula,
    /// Nord.
    Nord,
    /// Gruvbox (dark).
    GruvboxDark,
    /// Solarized (dark).
    SolarizedDark,
    /// Monokai (bat's `Monokai Extended`).
    Monokai,
    /// The terminal's own ANSI palette (bat's `ansi`): every scope resolves
    /// to a palette *index*, which `convert_syntect_color` decodes into
    /// ratatui's named colours — so the code follows the terminal's theme.
    Ansi,
}

impl CodeTheme {
    /// Every theme, in the `/theme` catalog's order.
    pub const ALL: [Self; 11] = [
        Self::CatppuccinMocha,
        Self::CatppuccinMacchiato,
        Self::CatppuccinFrappe,
        Self::CatppuccinLatte,
        Self::OneDark,
        Self::Dracula,
        Self::Nord,
        Self::GruvboxDark,
        Self::SolarizedDark,
        Self::Monokai,
        Self::Ansi,
    ];

    /// The two-face bundle entry this theme is.
    const fn embedded(self) -> EmbeddedThemeName {
        match self {
            Self::CatppuccinMocha => EmbeddedThemeName::CatppuccinMocha,
            Self::CatppuccinMacchiato => EmbeddedThemeName::CatppuccinMacchiato,
            Self::CatppuccinFrappe => EmbeddedThemeName::CatppuccinFrappe,
            Self::CatppuccinLatte => EmbeddedThemeName::CatppuccinLatte,
            Self::OneDark => EmbeddedThemeName::TwoDark,
            Self::Dracula => EmbeddedThemeName::Dracula,
            Self::Nord => EmbeddedThemeName::Nord,
            Self::GruvboxDark => EmbeddedThemeName::GruvboxDark,
            Self::SolarizedDark => EmbeddedThemeName::SolarizedDark,
            Self::Monokai => EmbeddedThemeName::MonokaiExtended,
            Self::Ansi => EmbeddedThemeName::Ansi,
        }
    }

    /// The bundle's own name for the theme (`Catppuccin Mocha`, `TwoDark`,
    /// `ansi`, …) — what a `bat --theme` would call it.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.embedded().as_name()
    }

    /// The theme's slot in the per-theme caches below.
    const fn slot(self) -> usize {
        self as usize
    }
}

/// The embedded theme bundle (~64 KB of serialized themes, parsed one theme
/// at a time on first use). Held whole rather than re-read per theme: a
/// `/theme` browse touches several, and the set is smaller than one parsed
/// grammar.
static THEME_SET: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

/// One theme-bound highlighter per [`CodeTheme`], built on first use: it
/// borrows its theme out of [`THEME_SET`] for the process's life, which is
/// what lets a [`Highlighter`]'s carried state hold a `'static` reference
/// to it.
static HIGHLIGHTERS: [OnceLock<SynHighlighter<'static>>; CodeTheme::ALL.len()] =
    [const { OnceLock::new() }; CodeTheme::ALL.len()];

/// The highlighter that resolves a scope stack to a style under `code`.
fn highlighter(code: CodeTheme) -> &'static SynHighlighter<'static> {
    HIGHLIGHTERS[code.slot()].get_or_init(|| SynHighlighter::new(THEME_SET.get(code.embedded())))
}

// Syntect/bat encode ANSI palette semantics in the colour's alpha channel:
// `a=0` => the RGB payload is an ANSI palette index, `a=1` => terminal default.
// Catppuccin (an RGB theme) never uses these, but we honour them for robustness
// in case the theme is ever swapped for an ANSI-family one — a direct port of
// codex's `convert_syntect_color`.
const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;
const OPAQUE_ALPHA: u8 = 0xFF;

/// A pathological single line (a minified bundle pasted into a fence) is left
/// unhighlighted rather than handed to the regex engine — bounds worst-case CPU
/// on one line without a whole-block guard (the block streams line-by-line).
///
/// The bound is a **frame budget**, not just a safety net: while such a line
/// is the trailing one of a streaming reply, the strip preview re-highlights
/// it on every frame that arrives with a new chunk, so one line must stay
/// well under the 32 ms animation cadence (a 62 KB minified JSON line
/// measured >150 ms per pass through oniguruma). Editors draw the same line —
/// tokenization is capped at a few KB and the rest renders plain.
const MAX_LINE_BYTES: usize = 4096;

/// The plain-code colour under `code`: the theme's default foreground, used
/// for unhighlighted text (an unknown/`text` language, or an indented code
/// block with no info string) so it matches the un-scoped tokens of a
/// *highlighted* block. A theme that names no foreground (or names the
/// terminal's default, as the ANSI theme does) yields an unstyled span.
#[must_use]
pub fn plain_style(code: CodeTheme) -> Style {
    static PLAIN: [OnceLock<Style>; CodeTheme::ALL.len()] =
        [const { OnceLock::new() }; CodeTheme::ALL.len()];
    *PLAIN[code.slot()].get_or_init(|| {
        let fg = THEME_SET
            .get(code.embedded())
            .settings
            .foreground
            .and_then(convert_syntect_color);
        fg.map_or_else(Style::default, |fg| Style::default().fg(fg))
    })
}

// -- Syntax lookup (ported from codex's `find_syntax`) ------------------------

/// Languages that should render **plain** (terminal output / prose fences), so a
/// ```` ```text ```` or ```` ```console ```` block isn't syntax-coloured.
fn is_plain_lang(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "" | "text" | "txt" | "plain" | "plaintext" | "console" | "output" | "log"
    )
}

/// Resolve a fenced block's info-string language to a grammar, patching the few
/// aliases two-face can't resolve on its own (codex parity). `None` for a
/// plain/terminal language or an unrecognised one — the caller then renders the
/// block plain.
fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    if is_plain_lang(lang) {
        return None;
    }
    let ss = &*SYNTAX_SET;
    let normalized = lang.to_ascii_lowercase();
    let patched = match normalized.as_str() {
        "csharp" | "c-sharp" => "c#",
        "cppm" | "cxxm" | "ixx" => "cpp",
        "golang" => "go",
        "python3" => "python",
        "shell" | "zsh" => "bash",
        "rs" => "rust",
        _ => lang,
    };
    if let Some(s) = ss.find_syntax_by_token(patched) {
        return Some(s);
    }
    if let Some(s) = ss.find_syntax_by_name(patched) {
        return Some(s);
    }
    let lower = patched.to_ascii_lowercase();
    if let Some(s) = ss
        .syntaxes()
        .iter()
        .find(|s| s.name.to_ascii_lowercase() == lower)
    {
        return Some(s);
    }
    ss.find_syntax_by_extension(lang)
}

// -- Style conversion (syntect -> ratatui), ported from codex -----------------

/// Decode a syntect foreground colour into a ratatui colour, honouring bat's
/// alpha-channel ANSI encoding. `None` ⇒ "use the terminal default".
fn convert_syntect_color(color: SynColor) -> Option<Color> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        OPAQUE_ALPHA => Some(Color::Rgb(color.r, color.g, color.b)),
        _ => Some(Color::Rgb(color.r, color.g, color.b)),
    }
}

/// Map an ANSI palette index to ratatui's named/indexed colours (codex parity).
fn ansi_palette_color(index: u8) -> Color {
    match index {
        0x00 => Color::Black,
        0x01 => Color::Red,
        0x02 => Color::Green,
        0x03 => Color::Yellow,
        0x04 => Color::Blue,
        0x05 => Color::Magenta,
        0x06 => Color::Cyan,
        0x07 => Color::Gray,
        n => Color::Indexed(n),
    }
}

/// Convert a syntect style to a ratatui style: foreground colour + bold only.
/// Background is skipped (the terminal's own bg shows through), and italic +
/// underline are dropped — many terminals render italic poorly and some themes
/// underline type scopes, both of which look wrong inline (codex parity).
fn convert_style(syn: SynStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = convert_syntect_color(syn.foreground) {
        style = style.fg(fg);
    }
    if syn.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

// -- Incremental highlighter --------------------------------------------------

/// Syntect's per-line lexer + highlighter state, carried across the lines of one
/// fenced block. `None` for a plain/unknown language (every line is one
/// [`plain_style`] segment).
#[derive(Clone)]
struct State {
    parse: ParseState,
    highlight: HighlightState,
    /// The theme-bound highlighter `highlight` was opened against — the
    /// state caches resolved styles, so it must keep resolving through the
    /// same theme however the ambient one changes mid-block.
    highlighter: &'static SynHighlighter<'static>,
}

/// An **incremental** syntax highlighter for one fenced code block: feed source
/// lines one at a time with [`Highlighter::line`], threading the multi-line
/// parse/highlight state across the calls. `Clone` lets the streaming renderer
/// *peek* an in-progress (not-yet-complete) line without disturbing the state it
/// resumes from once the line completes. See the module docs.
#[derive(Clone)]
pub struct Highlighter {
    /// `None` ⇒ plain passthrough (unknown/`text` language, or indented code).
    state: Option<State>,
    /// The theme the block is coloured with — fixed at the open, so a block
    /// that straddles a `/theme` switch stays one colour scheme until the
    /// purge rebuild re-renders it whole.
    code: CodeTheme,
}

impl Highlighter {
    /// A highlighter for `lang` (the fence info string) under the `code`
    /// theme. An unrecognised or plain-text language yields one
    /// [`plain_style`] segment per line.
    #[must_use]
    pub fn new(lang: Option<&str>, code: CodeTheme) -> Self {
        let state = lang.and_then(find_syntax).map(|syntax| {
            let highlighter = highlighter(code);
            State {
                parse: ParseState::new(syntax),
                highlight: HighlightState::new(highlighter, ScopeStack::new()),
                highlighter,
            }
        });
        Self { state, code }
    }

    /// Highlight the next source `line` (without a trailing newline), advancing
    /// the carried parse/highlight state.
    #[must_use]
    pub fn line(&mut self, line: &str) -> Vec<Seg> {
        let plain = plain_style(self.code);
        let Some(state) = self.state.as_mut() else {
            return vec![Seg {
                text: line.to_string(),
                style: plain,
            }];
        };
        if line.len() > MAX_LINE_BYTES {
            return vec![Seg {
                text: line.to_string(),
                style: plain,
            }];
        }
        // Grammars are line-based: many contexts anchor on `\n`, so syntect
        // parses lines *with* their newline (codex uses `LinesWithEndings`).
        // Our source lines arrive already split on `\n`, so re-append one, then
        // strip it back off the emitted spans.
        let with_nl = format!("{line}\n");
        let ops = match state.parse.parse_line(&with_nl, &SYNTAX_SET) {
            Ok(ops) => ops,
            Err(_) => {
                return vec![Seg {
                    text: line.to_string(),
                    style: plain,
                }];
            }
        };
        let iter = HighlightIterator::new(&mut state.highlight, &ops, &with_nl, state.highlighter);
        let mut out: Vec<Seg> = Vec::new();
        for (syn_style, text) in iter {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            push(&mut out, text, convert_style(syn_style));
        }
        if out.is_empty() {
            out.push(Seg {
                text: String::new(),
                style: plain,
            });
        }
        out
    }
}

/// Append `text` as a styled run, merging into the previous run when the style
/// matches so adjacent same-style scopes coalesce into one span.
fn push(out: &mut Vec<Seg>, text: &str, style: Style) {
    if let Some(last) = out.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
        return;
    }
    out.push(Seg {
        text: text.to_string(),
        style,
    });
}

/// Highlight each of `lines` (a fenced code block's source) into styled
/// segments under the `code` theme, threading the multi-line state
/// left-to-right. Every line concatenates back to the original text. Thin
/// batch wrapper over the incremental [`Highlighter`].
#[must_use]
pub fn highlight(lines: &[&str], lang: Option<&str>, code: CodeTheme) -> Vec<Vec<Seg>> {
    let mut h = Highlighter::new(lang, code);
    lines.iter().map(|line| h.line(line)).collect()
}

/// A **growing** text highlighted incrementally — what a `write` streaming
/// into its cell is coloured with (`docs/write-streaming.md`).
///
/// A grammar is a state machine, so line 40 can only be coloured by a
/// highlighter that has read lines 1 to 39: a docstring opened on line 3
/// makes line 40 a string. Re-highlighting the whole text on every frame
/// costs tens of microseconds a line, many times a frame. So the cache keeps
/// each **complete** line's segments, highlighted once, with the
/// highlighter's state after the last of them, and colours the unfinished
/// last line on a *clone* of that state — the peek `ui::StreamRender` takes
/// at a code block's withheld line — remembered by the text's length so the
/// several builds one frame makes are free.
///
/// Lines split like [`str::lines`]: a `\r` before a line break is dropped, and
/// a text ending at a line break has no unfinished line. Tabs are expanded to
/// `tab_width` spaces **before** highlighting, the order the committed file
/// cell uses, so the two colour a line alike.
///
/// What it **keeps** is its own choice: every line ([`LineCache::default`])
/// or, [`windowed`](LineCache::windowed), only the first and newest lines a
/// peek can show. A line's segments run about fifty times its text — a
/// `String` and a style per token — so a 2,000-line file coloured whole is
/// ~1.9 MB, held for as long as the call is live, and glibc keeps what a
/// process has once used. Every line is still highlighted once, in order:
/// the grammar's state has to pass through the middle to colour the newest
/// line right; only the segments in between are let go (`docs/memory.md`).
#[derive(Clone, Default)]
pub struct LineCache {
    /// What the cached lines were highlighted for: the language and the code
    /// theme. A change rebuilds the cache.
    key: Option<(Option<String>, CodeTheme, usize)>,
    /// The highlighter's state after the cached complete lines.
    state: Option<Highlighter>,
    /// How many complete lines it keeps at each end of the text — `None`
    /// keeps every line.
    window: Option<usize>,
    /// Complete lines' segments, in order: every one, or under a window the
    /// first `window`.
    lines: Vec<Vec<Seg>>,
    /// Under a window, the newest complete lines past the first `window`, at
    /// most `window` of them, oldest first.
    tail: VecDeque<Vec<Seg>>,
    /// How many complete lines the text has — kept or not.
    complete: usize,
    /// Bytes of the text the complete lines cover, their line breaks
    /// included.
    consumed: usize,
    /// The unfinished last line's segments, with the text length they were
    /// made at; `None` while the text ends at a line break.
    partial: Option<(usize, Vec<Seg>)>,
}

impl std::fmt::Debug for LineCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The highlighter's state is syntect's; the shape is what matters.
        f.debug_struct("LineCache")
            .field("key", &self.key)
            .field("window", &self.window)
            .field("complete", &self.complete)
            .field("retained", &self.retained())
            .field("consumed", &self.consumed)
            .field("partial", &self.partial.is_some())
            .finish_non_exhaustive()
    }
}

impl LineCache {
    /// A cache that keeps the segments of only the first and the newest
    /// `lines` complete lines — what a peek of `lines` rows can show, from
    /// either end — and of the unfinished last line.
    #[must_use]
    pub fn windowed(lines: usize) -> Self {
        Self {
            window: Some(lines),
            ..Self::default()
        }
    }

    /// Bring the cache up to `text`, highlighted as `lang` under `code` with
    /// tabs expanded to `tab_width` spaces. `text` is expected to **extend**
    /// the text the cache last saw — a streamed value only ever grows — so
    /// only the lines completed since are highlighted. A text that does not
    /// (shorter than the lines already read, or no longer breaking where they
    /// ended), or a different language, theme or tab width, rebuilds the
    /// cache from the start.
    pub fn update(&mut self, text: &str, lang: Option<&str>, code: CodeTheme, tab_width: usize) {
        let key = (lang.map(str::to_string), code, tab_width);
        let extends = self.consumed <= text.len()
            && (self.consumed == 0 || text.as_bytes()[self.consumed - 1] == b'\n');
        if self.key.as_ref() != Some(&key) || !extends {
            *self = Self {
                key: Some(key),
                state: Some(Highlighter::new(lang, code)),
                window: self.window,
                ..Self::default()
            };
        }
        let state = self
            .state
            .get_or_insert_with(|| Highlighter::new(lang, code));
        let expand = |line: &str| -> String {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.contains('\t') {
                line.replace('\t', &" ".repeat(tab_width))
            } else {
                line.to_string()
            }
        };
        while let Some(at) = text[self.consumed..].find('\n') {
            let line = &text[self.consumed..self.consumed + at];
            let segs = state.line(&expand(line));
            match self.window {
                Some(window) if self.lines.len() >= window => {
                    self.tail.push_back(segs);
                    if self.tail.len() > window {
                        self.tail.pop_front();
                    }
                }
                _ => self.lines.push(segs),
            }
            self.complete += 1;
            self.consumed += at + 1;
            self.partial = None;
        }
        let rest = &text[self.consumed..];
        if rest.is_empty() {
            self.partial = None;
        } else if self
            .partial
            .as_ref()
            .is_none_or(|(len, _)| *len != text.len())
        {
            self.partial = Some((text.len(), state.clone().line(&expand(rest))));
        }
    }

    /// Line `at`'s segments, counted from 0: a complete line the cache keeps,
    /// or the unfinished last line — `None` past the end, and for a line a
    /// window let go.
    #[must_use]
    pub fn line(&self, at: usize) -> Option<&[Seg]> {
        if let Some(line) = self.lines.get(at) {
            return Some(line);
        }
        if at < self.complete {
            let first = self.complete - self.tail.len();
            return at
                .checked_sub(first)
                .and_then(|i| self.tail.get(i))
                .map(Vec::as_slice);
        }
        if at == self.complete {
            return self.partial();
        }
        None
    }

    /// How many complete lines it keeps at each end of the text — `None`
    /// when it keeps every line.
    #[must_use]
    pub const fn window(&self) -> Option<usize> {
        self.window
    }

    /// How many complete lines' segments it holds — every line, or a
    /// window's first and newest. What it costs, in lines.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.lines.len() + self.tail.len()
    }

    /// The unfinished last line's segments — `None` when the text is empty or
    /// ends at a line break.
    #[must_use]
    pub fn partial(&self) -> Option<&[Seg]> {
        self.partial.as_ref().map(|(_, segs)| segs.as_slice())
    }

    /// How many lines the text has, counted as [`str::lines`] counts them:
    /// the complete ones and the unfinished one.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.complete + usize::from(self.partial.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The concatenated text of a line's segments.
    fn joined(segs: &[Seg]) -> String {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    /// The style covering the first segment whose text contains `needle`.
    fn style_of(line: &str, lang: &str, needle: &str) -> Style {
        let segs = highlight(&[line], Some(lang), CodeTheme::default())
            .pop()
            .unwrap();
        for s in &segs {
            if s.text.contains(needle) {
                return s.style;
            }
        }
        panic!("{needle:?} not found in {line:?} -> {segs:?}");
    }

    #[test]
    fn a_pathological_line_renders_plain_within_the_frame_budget() {
        // A machine-dump line (minified JSON) past MAX_LINE_BYTES must skip
        // the regex engine entirely: byte-identical text back, plain style,
        // and fast enough that the streaming preview can re-render it every
        // frame (>150 ms per pass through oniguruma at 62 KB — the stress
        // probe `preview_of_a_growing_code_line_is_bounded`).
        let line: String = "{\"k\":123,\"deep\":[1,2,3]},".repeat(2500);
        assert!(line.len() > MAX_LINE_BYTES);
        let start = std::time::Instant::now();
        let segs = highlight(&[&line], Some("json"), CodeTheme::default())
            .pop()
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(joined(&segs), line, "the text survives verbatim");
        assert!(
            segs.iter()
                .all(|s| s.style == plain_style(CodeTheme::default())),
            "an over-long line renders plain, never partially highlighted"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "the capped path must stay inside a frame budget, took {elapsed:?}"
        );
    }

    #[test]
    fn segments_concatenate_back_to_the_source_line() {
        for line in [
            "def guessing_game():",
            "    print(\"🎮 Welcome!\")  # hi",
            "x = random.randint(1, 100)",
            "return f\"got {n}\"",
        ] {
            let segs = highlight(&[line], Some("python"), CodeTheme::default())
                .pop()
                .unwrap();
            assert_eq!(joined(&segs), line, "round-trips");
        }
    }

    #[test]
    fn keywords_strings_and_functions_get_distinct_colours() {
        // We don't hardcode the theme's RGB (codex's test style): assert the
        // tokens are *styled* and that the classes differ from each other, which
        // is what proves real grammar-aware highlighting.
        let kw = style_of("def f():", "python", "def").fg;
        let func = style_of("def f():", "python", "f").fg;
        let string = style_of("x = \"hi\"", "python", "\"hi\"").fg;
        let number = style_of("x = 100", "python", "100").fg;
        for (name, c) in [
            ("keyword", kw),
            ("function", func),
            ("string", string),
            ("number", number),
        ] {
            assert!(c.is_some(), "{name} should be coloured");
        }
        assert_ne!(kw, string, "keyword and string must differ");
        assert_ne!(kw, number, "keyword and number must differ");
        assert_ne!(string, func, "string and function must differ");
    }

    #[test]
    fn python_hash_is_a_comment_but_a_css_id_selector_is_not() {
        // The regression the syntect port fixes: the old generic tokenizer
        // treated a leading `#` as a line comment in *every* language, so a CSS
        // `#id { … }` selector was dimmed as a comment. A real grammar knows the
        // difference.
        let py_comment = style_of("x = 1  # note", "python", "# note").fg;
        let css_selector = style_of("#message { color: blue; }", "css", "message").fg;
        assert!(py_comment.is_some(), "python # is a comment");
        assert!(
            css_selector.is_some(),
            "CSS #id is styled (a selector), not a comment"
        );
        assert_ne!(
            py_comment, css_selector,
            "a CSS id selector must not be coloured like a comment"
        );
    }

    #[test]
    fn html_tags_are_highlighted_including_embedded_css() {
        // Codex parity: HTML tags/attributes are coloured, and the CSS embedded
        // in a <style> block is highlighted by the embedded grammar — the whole
        // point of the port (the sample from the bug report).
        let lines = [
            "<!DOCTYPE html>",
            "<style>",
            "  #message { color: blue; }",
            "</style>",
        ];
        let out = highlight(&lines, Some("html"), CodeTheme::default());
        // The `html` tag name in <!DOCTYPE html> is coloured…
        assert!(
            out[0]
                .iter()
                .any(|s| s.text.contains("html") && s.style.fg.is_some()),
            "html tag name coloured: {:?}",
            out[0]
        );
        // …and inside <style>, the `#message` selector and `color` property are
        // coloured by the embedded CSS grammar, and NOT all as one comment run.
        let colours: Vec<_> = out[2].iter().filter_map(|s| s.style.fg).collect();
        assert!(
            colours.len() > 1,
            "embedded CSS is multi-coloured: {:?}",
            out[2]
        );
    }

    #[test]
    fn a_multiline_string_carries_across_lines() {
        // syntect's ParseState carries an open triple-quoted string, so interior
        // lines colour as string — same guarantee the old hand-rolled Carry gave.
        let lines = ["doc = \"\"\"", "multi", "line\"\"\"", "code = 1"];
        let out = highlight(&lines, Some("python"), CodeTheme::default());
        let str_fg = style_of("x = \"hi\"", "python", "\"hi\"").fg;
        assert!(
            out[1].iter().all(|s| s.style.fg == str_fg),
            "interior line is all string-coloured: {:?}",
            out[1]
        );
        assert_eq!(joined(&out[1]), "multi");
    }

    #[test]
    fn incremental_line_by_line_equals_batch_highlight() {
        // The incremental `Highlighter` (one `.line()` per source line, threading
        // its own state) must produce byte-identical output to the batch
        // `highlight()` — same multi-line carry, same styled segments.
        for (lang, lines) in [
            (
                Some("python"),
                &["x = \"\"\"", "multi", "line\"\"\"", "y = f(1)"][..],
            ),
            (Some("rust"), &["fn main() {", "    let c = 'x';", "}"][..]),
            (Some("html"), &["<div>", "  <span>hi</span>", "</div>"][..]),
            (None, &["def f():", "    return 1"][..]),
            (Some("text"), &["plain", "output"][..]),
        ] {
            let batch = highlight(lines, lang, CodeTheme::default());
            let mut h = Highlighter::new(lang, CodeTheme::default());
            let incremental: Vec<Vec<Seg>> = lines.iter().map(|l| h.line(l)).collect();
            assert_eq!(incremental, batch, "lang={lang:?}");
        }
    }

    #[test]
    fn unknown_and_plain_languages_are_not_highlighted() {
        // A `text` / unknown / no-language block renders as one plain segment per
        // line, in the plain (theme-default) colour.
        for lang in [Some("text"), Some("definitely-not-a-language"), None] {
            let segs = highlight(&["def f():"], lang, CodeTheme::default())
                .pop()
                .unwrap();
            assert_eq!(
                segs,
                vec![Seg {
                    text: "def f():".into(),
                    style: plain_style(CodeTheme::default())
                }],
                "lang={lang:?}"
            );
        }
    }

    #[test]
    fn a_committed_line_never_recolours_when_more_lines_arrive() {
        // Prefix stability: once a line is "complete" (a later line exists), its
        // segments are frozen — the property `stable_commit` relies on.
        let full = "x = \"\"\"\nhello\nworld\n\"\"\"\ny = f(1)";
        let rows_of = |t: &str| -> Vec<Vec<Seg>> {
            let lines: Vec<&str> = t.split('\n').collect();
            highlight(&lines, Some("python"), CodeTheme::default())
        };
        let mut committed: Vec<Vec<Seg>> = Vec::new();
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let rows = rows_of(&full[..end]);
            let stable = rows.len().saturating_sub(1); // withhold the last line
            for (i, row) in rows.iter().enumerate().take(stable) {
                match committed.get(i) {
                    Some(frozen) => assert_eq!(frozen, row, "code line {i} recoloured"),
                    None => committed.push(row.clone()),
                }
            }
        }
    }

    #[test]
    fn the_code_themes_follow_the_catalog_and_every_one_resolves() {
        // One syntect theme per `/theme` entry, all reachable in the two-face
        // bundle (a name the bundle lacks would panic at first use — on the
        // user's first code block, not at build time), and all distinct.
        let mut seen = std::collections::HashSet::new();
        for code in CodeTheme::ALL {
            assert!(seen.insert(code.name()), "{code:?}: duplicate bundle theme");
            let segs = highlight(&["def f():"], Some("python"), code)
                .pop()
                .unwrap();
            assert_eq!(joined(&segs), "def f():", "{code:?} round-trips");
            assert!(
                segs.iter().any(|s| s.style.fg.is_some()),
                "{code:?}: the keyword line is coloured"
            );
        }
        assert_eq!(CodeTheme::default(), CodeTheme::CatppuccinMocha);
        assert_eq!(CodeTheme::default().name(), "Catppuccin Mocha");
    }

    #[test]
    fn two_themes_colour_the_same_keyword_differently() {
        // The point of naming a theme: the same `def` resolves to Mocha's
        // mauve under one theme and Dracula's pink under the other.
        let under = |code: CodeTheme| {
            highlight(&["def f():"], Some("python"), code)
                .pop()
                .unwrap()
                .into_iter()
                .find(|s| s.text.contains("def"))
                .expect("the keyword segment")
                .style
                .fg
        };
        let mocha = under(CodeTheme::CatppuccinMocha);
        let dracula = under(CodeTheme::Dracula);
        assert!(mocha.is_some() && dracula.is_some());
        assert_ne!(mocha, dracula, "the two themes disagree on the keyword");
        assert_ne!(
            plain_style(CodeTheme::CatppuccinMocha),
            plain_style(CodeTheme::CatppuccinLatte),
            "the light flavour's plain text is dark, the dark's is light"
        );
    }

    #[test]
    fn the_ansi_theme_uses_the_terminal_palette_never_rgb() {
        // bat's `ansi` theme encodes palette indices in the colour's alpha
        // channel; decoded, every segment is a named/indexed terminal colour
        // — so the code follows whatever palette the terminal is configured
        // with — and the plain text is the terminal's default foreground.
        for line in ["def f():", "x = \"hi\"  # note", "return 100"] {
            for seg in highlight(&[line], Some("python"), CodeTheme::Ansi)
                .pop()
                .unwrap()
            {
                assert!(
                    !matches!(seg.style.fg, Some(Color::Rgb(..))),
                    "{line:?}: an ANSI segment wears a palette colour, got {:?}",
                    seg.style.fg
                );
            }
        }
        assert_eq!(
            plain_style(CodeTheme::Ansi).fg,
            None,
            "the ANSI theme's plain text is the terminal default"
        );
    }

    #[test]
    fn a_highlighter_keeps_the_theme_it_opened_with() {
        // The carried state resolves styles through the theme fixed at the
        // open — a block half-rendered when the theme switches stays one
        // scheme until the purge rebuild re-renders it whole.
        let mut mocha = Highlighter::new(Some("python"), CodeTheme::CatppuccinMocha);
        let first = mocha.line("def f():");
        let again = mocha.line("def g():");
        assert_eq!(
            first[0].style, again[0].style,
            "the same theme line after line"
        );
        assert_eq!(
            first,
            highlight(&["def f():"], Some("python"), CodeTheme::CatppuccinMocha).remove(0)
        );
    }

    // --- the incremental line cache (docs/write-streaming.md) ---

    /// What a fresh highlight of `text`'s complete lines gives, and of its
    /// unfinished last line resumed from them — the cache's whole contract.
    fn whole(text: &str, lang: Option<&str>) -> (Vec<Vec<Seg>>, Option<Vec<Seg>>) {
        let (done, partial) = match text.rfind('\n') {
            Some(at) => (&text[..at], &text[at + 1..]),
            None => ("", text),
        };
        let mut h = Highlighter::new(lang, CodeTheme::default());
        let lines: Vec<Vec<Seg>> = if text.contains('\n') {
            done.split('\n')
                .map(|line| h.line(&line.replace('\t', "    ")))
                .collect()
        } else {
            Vec::new()
        };
        let partial = (!partial.is_empty()).then(|| h.line(&partial.replace('\t', "    ")));
        (lines, partial)
    }

    /// Every complete line the cache keeps, by number — `None` where a
    /// window let it go.
    fn kept(cache: &LineCache) -> Vec<Option<Vec<Seg>>> {
        let complete = cache.line_count() - usize::from(cache.partial().is_some());
        (0..complete)
            .map(|at| cache.line(at).map(<[Seg]>::to_vec))
            .collect()
    }

    /// Every complete line of a cache that keeps them all.
    fn complete(cache: &LineCache) -> Vec<Vec<Seg>> {
        kept(cache)
            .into_iter()
            .map(|line| line.expect("a cache without a window keeps every line"))
            .collect()
    }

    #[test]
    fn a_windowed_line_cache_keeps_only_its_first_and_newest_lines() {
        // What a peek can show is its first and newest lines; every line in
        // between is highlighted once, to carry the grammar along, and let
        // go (docs/write-streaming.md).
        let text: String = (1..=100).map(|i| format!("x_{i} = {i}\n")).collect();
        let mut cache = LineCache::windowed(3);
        cache.update(&text, Some("python"), CodeTheme::default(), 4);
        assert_eq!(cache.line_count(), 100);
        let held: Vec<usize> = (0..100).filter(|&at| cache.line(at).is_some()).collect();
        assert_eq!(held, [0, 1, 2, 97, 98, 99]);
        assert_eq!(cache.retained(), 6, "nothing else is held");
        assert_eq!(cache.line(100), None, "the text ends at a line break");
        cache.update(
            &format!("{text}y = "),
            Some("python"),
            CodeTheme::default(),
            4,
        );
        assert_eq!(
            cache.line(100).map(joined).as_deref(),
            Some("y = "),
            "the unfinished line comes after the newest"
        );
        assert_eq!(cache.retained(), 6);
    }

    #[test]
    fn a_windowed_line_cache_colours_what_it_keeps_exactly_as_the_whole_text() {
        // A docstring opened in the first lines and closed in the newest: a
        // window that let the grammar's state go with the lines would colour
        // the newest as code.
        let mut text = String::from("def f():\n    \"\"\"Doc\n");
        for i in 0..30 {
            text.push_str(&format!("    still doc {i}\n"));
        }
        text.push_str("    \"\"\"\n\treturn 1  # done\n");
        let mut cache = LineCache::windowed(2);
        for (at, _) in text.char_indices().skip(1).chain([(text.len(), ' ')]) {
            let prefix = &text[..at];
            cache.update(prefix, Some("python"), CodeTheme::default(), 4);
            let (lines, partial) = whole(prefix, Some("python"));
            for (i, line) in lines.iter().enumerate() {
                let edge = i < 2 || i + 2 >= lines.len();
                assert_eq!(
                    cache.line(i).map(<[Seg]>::to_vec),
                    edge.then(|| line.clone()),
                    "line {i} of {prefix:?}"
                );
            }
            assert_eq!(
                cache.partial(),
                partial.as_deref(),
                "partial line of {prefix:?}"
            );
        }
    }

    #[test]
    fn a_windowed_line_cache_stays_a_window_through_a_rebuild() {
        // The path naming the language can arrive after the content: the
        // rebuild re-reads the whole text, and still keeps only the window.
        let text: String = (1..=50).map(|i| format!("v{i} = \"s\"\n")).collect();
        let mut cache = LineCache::windowed(2);
        cache.update(&text, None, CodeTheme::default(), 4);
        cache.update(&text, Some("python"), CodeTheme::default(), 4);
        assert_eq!(cache.retained(), 4);
        let (lines, _) = whole(&text, Some("python"));
        assert_eq!(cache.line(0).map(<[Seg]>::to_vec), Some(lines[0].clone()));
        assert_eq!(cache.line(49).map(<[Seg]>::to_vec), Some(lines[49].clone()));
    }

    #[test]
    fn a_line_cache_fed_a_growing_text_matches_highlighting_it_whole() {
        // A docstring spanning lines: a cache that restarted its highlighter
        // anywhere would colour the middle line as code.
        let text = "def f():\n    \"\"\"Doc\n    still doc\n    \"\"\"\n\treturn 1  # done\n";
        let mut cache = LineCache::default();
        for (at, _) in text.char_indices().skip(1).chain([(text.len(), ' ')]) {
            let prefix = &text[..at];
            cache.update(prefix, Some("python"), CodeTheme::default(), 4);
            let (lines, partial) = whole(prefix, Some("python"));
            assert_eq!(complete(&cache), lines, "complete lines of {prefix:?}");
            assert_eq!(
                cache.partial(),
                partial.as_deref(),
                "partial line of {prefix:?}"
            );
        }
    }

    #[test]
    fn a_line_cache_highlights_each_complete_line_once() {
        let mut cache = LineCache::default();
        cache.update("a = 1\nb = 2\n", Some("python"), CodeTheme::default(), 4);
        let first = complete(&cache)[0].clone();
        // Growing the text re-reads nothing it has already highlighted.
        cache.update(
            "a = 1\nb = 2\nc = ",
            Some("python"),
            CodeTheme::default(),
            4,
        );
        assert_eq!(complete(&cache).len(), 2);
        assert_eq!(complete(&cache)[0], first);
        assert_eq!(joined(cache.partial().expect("a partial line")), "c = ");
    }

    #[test]
    fn a_line_cache_rebuilds_for_a_new_language_a_new_theme_or_a_shorter_text() {
        let text = "x = \"s\"\n";
        let mut cache = LineCache::default();
        cache.update(text, None, CodeTheme::default(), 4);
        assert_eq!(complete(&cache), whole(text, None).0);
        cache.update(text, Some("python"), CodeTheme::default(), 4);
        assert_eq!(complete(&cache), whole(text, Some("python")).0);
        cache.update(text, Some("python"), CodeTheme::Dracula, 4);
        let mut dracula = Highlighter::new(Some("python"), CodeTheme::Dracula);
        assert_eq!(complete(&cache), [dracula.line("x = \"s\"")]);
        cache.update("y\n", Some("python"), CodeTheme::Dracula, 4);
        assert_eq!(complete(&cache).len(), 1);
        assert_eq!(joined(&complete(&cache)[0]), "y");
    }

    #[test]
    fn a_line_cache_expands_tabs_and_drops_carriage_returns() {
        let mut cache = LineCache::default();
        cache.update("\tone\r\ntwo\r", None, CodeTheme::default(), 4);
        assert_eq!(joined(&complete(&cache)[0]), "    one");
        assert_eq!(joined(cache.partial().expect("a partial line")), "two");
        assert_eq!(cache.line_count(), 2);
    }

    #[test]
    fn a_line_cache_counts_lines_the_way_str_lines_does() {
        for text in ["", "a", "a\n", "a\nb", "a\nb\n", "\n", "\n\n", "a\r\n"] {
            let mut cache = LineCache::default();
            cache.update(text, None, CodeTheme::default(), 4);
            assert_eq!(cache.line_count(), text.lines().count(), "{text:?}");
        }
    }
}

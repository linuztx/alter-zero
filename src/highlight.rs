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
//! Styling lives with the theme (Catppuccin Mocha, codex's dark default), not as
//! a `ui`-owned palette — the tokenizer is no longer colour-agnostic, because a
//! real grammar's scopes carry far more distinction (tag vs attribute vs value)
//! than a fixed six-colour enum could. `Seg` therefore carries a resolved
//! [`Style`]; `ui` maps nothing.

use ratatui::style::{Color, Modifier, Style};
use std::sync::LazyLock;
use syntect::highlighting::{
    Color as SynColor, FontStyle, HighlightIterator, HighlightState, Highlighter as SynHighlighter,
    Style as SynStyle, Theme,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::theme::EmbeddedThemeName;

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

/// The active colour theme — Catppuccin Mocha, codex's adaptive dark default.
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    two_face::theme::extra()
        .get(EmbeddedThemeName::CatppuccinMocha)
        .clone()
});

/// The theme-bound highlighter that resolves a scope stack to a style.
static HIGHLIGHTER: LazyLock<SynHighlighter<'static>> =
    LazyLock::new(|| SynHighlighter::new(&THEME));

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
const MAX_LINE_BYTES: usize = 100_000;

/// The plain-code colour: the theme's default foreground, used for unhighlighted
/// text (an unknown/`text` language, or an indented code block with no info
/// string) so it matches the un-scoped tokens of a *highlighted* block.
#[must_use]
pub fn plain_style() -> Style {
    static PLAIN: LazyLock<Style> = LazyLock::new(|| {
        let fg = THEME
            .settings
            .foreground
            .and_then(convert_syntect_color)
            // Catppuccin Mocha's default foreground, if the theme omits one.
            .unwrap_or(Color::Rgb(0xCD, 0xD6, 0xF4));
        Style::default().fg(fg)
    });
    *PLAIN
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
}

impl Highlighter {
    /// A highlighter for `lang` (the fence info string). An unrecognised or
    /// plain-text language yields one [`plain_style`] segment per line.
    #[must_use]
    pub fn new(lang: Option<&str>) -> Self {
        let state = lang.and_then(find_syntax).map(|syntax| State {
            parse: ParseState::new(syntax),
            highlight: HighlightState::new(&HIGHLIGHTER, ScopeStack::new()),
        });
        Self { state }
    }

    /// Highlight the next source `line` (without a trailing newline), advancing
    /// the carried parse/highlight state.
    #[must_use]
    pub fn line(&mut self, line: &str) -> Vec<Seg> {
        let Some(state) = self.state.as_mut() else {
            return vec![Seg {
                text: line.to_string(),
                style: plain_style(),
            }];
        };
        if line.len() > MAX_LINE_BYTES {
            return vec![Seg {
                text: line.to_string(),
                style: plain_style(),
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
                    style: plain_style(),
                }];
            }
        };
        let iter = HighlightIterator::new(&mut state.highlight, &ops, &with_nl, &HIGHLIGHTER);
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
                style: plain_style(),
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
/// segments, threading the multi-line state left-to-right. Every line
/// concatenates back to the original text. Thin batch wrapper over the
/// incremental [`Highlighter`].
#[must_use]
pub fn highlight(lines: &[&str], lang: Option<&str>) -> Vec<Vec<Seg>> {
    let mut h = Highlighter::new(lang);
    lines.iter().map(|line| h.line(line)).collect()
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
        let segs = highlight(&[line], Some(lang)).pop().unwrap();
        for s in &segs {
            if s.text.contains(needle) {
                return s.style;
            }
        }
        panic!("{needle:?} not found in {line:?} -> {segs:?}");
    }

    #[test]
    fn segments_concatenate_back_to_the_source_line() {
        for line in [
            "def guessing_game():",
            "    print(\"🎮 Welcome!\")  # hi",
            "x = random.randint(1, 100)",
            "return f\"got {n}\"",
        ] {
            let segs = highlight(&[line], Some("python")).pop().unwrap();
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
        let out = highlight(&lines, Some("html"));
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
        let out = highlight(&lines, Some("python"));
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
            let batch = highlight(lines, lang);
            let mut h = Highlighter::new(lang);
            let incremental: Vec<Vec<Seg>> = lines.iter().map(|l| h.line(l)).collect();
            assert_eq!(incremental, batch, "lang={lang:?}");
        }
    }

    #[test]
    fn unknown_and_plain_languages_are_not_highlighted() {
        // A `text` / unknown / no-language block renders as one plain segment per
        // line, in the plain (theme-default) colour.
        for lang in [Some("text"), Some("definitely-not-a-language"), None] {
            let segs = highlight(&["def f():"], lang).pop().unwrap();
            assert_eq!(
                segs,
                vec![Seg {
                    text: "def f():".into(),
                    style: plain_style()
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
            highlight(&lines, Some("python"))
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
}

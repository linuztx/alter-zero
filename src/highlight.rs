//! Lightweight, dependency-free syntax highlighting for fenced code blocks
//! (`docs/markdown.md`).
//!
//! Codex colours code with `syntect` (~250 TextMate grammars); this crate hand-
//! rolls everything and takes no such dependency, so this is a **generic**
//! tokenizer that classifies the token shapes common to mainstream languages:
//! comments, strings, numbers, control/declaration keywords, and function calls.
//! It is precise enough for the languages people actually paste (Python, JS/TS,
//! Rust, Go, C/C++, Java, Ruby, shell) and degrades to plain text for anything
//! it doesn't recognise. `ui` maps each [`Kind`] to a colour (the One Dark
//! palette), keeping all styling centralized there.
//!
//! **Prefix-stable.** [`highlight`] is a left-to-right scan: a line's colouring
//! is a pure function of the line's own text plus the multi-line [`Carry`] state
//! (open triple-string / block-comment) entering it — which depends only on the
//! lines *before* it. Appending text never recolours an already-finished line,
//! so streaming-to-scrollback stays sound (CLAUDE.md invariant 2).

/// The token class a run of code text falls into (mapped to a colour by `ui`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Identifiers, operators, punctuation — the default code colour.
    Plain,
    /// A language keyword (`if`, `def`, `fn`, `return`, `True`, …).
    Keyword,
    /// A string or character literal (single, double, triple, or backtick).
    Str,
    /// A comment (line or block).
    Comment,
    /// A numeric literal.
    Number,
    /// A name in call position — an identifier directly followed by `(`.
    Function,
}

/// One coloured run of a code line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    /// The run's text.
    pub text: String,
    /// Its token class.
    pub kind: Kind,
}

/// Multi-line lexer state carried between lines within a code block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carry {
    None,
    /// Inside a triple-quoted string opened with this quote char (`"` or `'`).
    Triple(char),
    /// Inside a `/* … */` block comment.
    Block,
}

/// Per-language lexical rules. Keywords/strings/numbers are language-agnostic;
/// only comment syntax and a couple of string flavours vary.
struct Syntax {
    /// `#` starts a line comment (Python, shell, Ruby, …).
    hash_comment: bool,
    /// `//` starts a line comment and `/* */` a block comment (C-like).
    slash_comment: bool,
    /// `--` starts a line comment (SQL, Lua, Haskell).
    dash_comment: bool,
    /// Triple-quoted strings `"""`/`'''` (Python).
    triple_strings: bool,
    /// Backtick strings (JS template literals, Go raw strings).
    backtick_strings: bool,
    /// Single quotes are **char literals** (`'x'`), not strings — so a bare `'a`
    /// is a Rust lifetime / label, not an unterminated string (C, Rust, Go, Java…).
    char_literals: bool,
}

impl Syntax {
    /// The rules for `lang`, or `None` when it shouldn't be highlighted (unknown
    /// info string, or a plain-text/console block).
    fn for_lang(lang: &str) -> Option<Self> {
        let lang = lang.trim().to_ascii_lowercase();
        // Non-code / terminal-output blocks render plain.
        if matches!(
            lang.as_str(),
            "" | "text" | "txt" | "plain" | "plaintext" | "console" | "output" | "log"
        ) {
            return None;
        }
        let hash = matches!(
            lang.as_str(),
            "python"
                | "py"
                | "python3"
                | "sh"
                | "bash"
                | "shell"
                | "zsh"
                | "fish"
                | "ruby"
                | "rb"
                | "perl"
                | "r"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "makefile"
                | "make"
                | "dockerfile"
                | "elixir"
                | "ex"
                | "exs"
                | "coffee"
                | "nim"
                | "julia"
                | "jl"
                | "php"
        );
        let slash = matches!(
            lang.as_str(),
            "c" | "h"
                | "cpp"
                | "c++"
                | "cc"
                | "hpp"
                | "cxx"
                | "java"
                | "js"
                | "javascript"
                | "jsx"
                | "ts"
                | "typescript"
                | "tsx"
                | "go"
                | "golang"
                | "rust"
                | "rs"
                | "swift"
                | "kotlin"
                | "kt"
                | "scala"
                | "csharp"
                | "cs"
                | "c#"
                | "php"
                | "dart"
                | "zig"
                | "json"
        );
        let dash = matches!(
            lang.as_str(),
            "sql" | "lua" | "haskell" | "hs" | "elm" | "ada"
        );
        let triple = matches!(lang.as_str(), "python" | "py" | "python3");
        let backtick = matches!(
            lang.as_str(),
            "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "go" | "golang"
        );
        // Languages where a single quote is a char literal (`'x'`) — so a bare
        // `'a` is a lifetime/label, not a string (Rust especially).
        let char_lit = matches!(
            lang.as_str(),
            "c" | "h"
                | "cpp"
                | "c++"
                | "cc"
                | "hpp"
                | "cxx"
                | "rust"
                | "rs"
                | "go"
                | "golang"
                | "java"
                | "csharp"
                | "cs"
                | "c#"
                | "swift"
                | "kotlin"
                | "kt"
                | "scala"
                | "zig"
                | "dart"
        );
        // A recognised language matches at least one rule; otherwise fall back to
        // a generic profile (hash + slash comments) so it still colours sensibly.
        if hash || slash || dash || triple || backtick {
            Some(Self {
                hash_comment: hash,
                slash_comment: slash,
                dash_comment: dash,
                triple_strings: triple,
                backtick_strings: backtick,
                char_literals: char_lit,
            })
        } else {
            Some(Self {
                hash_comment: true,
                slash_comment: true,
                dash_comment: false,
                triple_strings: false,
                backtick_strings: false,
                char_literals: false,
            })
        }
    }
}

/// Control-flow / declaration / constant keywords shared across mainstream
/// languages. Type and builtin names are deliberately excluded — `int(x)` should
/// read as a call (blue), not a keyword (magenta), matching the reference look.
const KEYWORDS: &[&str] = &[
    // control flow
    "if",
    "else",
    "elif",
    "elsif",
    "while",
    "for",
    "foreach",
    "do",
    "loop",
    "switch",
    "case",
    "match",
    "when",
    "unless",
    "until",
    "break",
    "continue",
    "return",
    "goto",
    "then",
    "end",
    "begin",
    "yield",
    "await",
    "async",
    "defer",
    "go",
    // declarations
    "def",
    "fn",
    "func",
    "function",
    "fun",
    "class",
    "struct",
    "enum",
    "trait",
    "impl",
    "interface",
    "module",
    "namespace",
    "package",
    "macro",
    "lambda",
    "let",
    "var",
    "val",
    "const",
    "mut",
    "pub",
    "static",
    "final",
    "public",
    "private",
    "protected",
    "abstract",
    "override",
    "extern",
    "inline",
    "type",
    "typedef",
    "using",
    // imports
    "import",
    "from",
    "use",
    "require",
    "include",
    // exceptions
    "try",
    "catch",
    "except",
    "finally",
    "throw",
    "throws",
    "raise",
    "rescue",
    "ensure",
    "panic",
    "with",
    // operators-as-words / misc
    "and",
    "or",
    "not",
    "in",
    "is",
    "as",
    "new",
    "delete",
    "del",
    "pass",
    "global",
    "nonlocal",
    "assert",
    "self",
    "this",
    "super",
    "sizeof",
    "typeof",
    "instanceof",
    // constants
    "true",
    "false",
    "none",
    "null",
    "nil",
    "True",
    "False",
    "None",
];

/// Highlight each of `lines` (a fenced code block's source) into coloured
/// segments, threading the multi-line [`Carry`] state left-to-right. Every line
/// concatenates back to the original text. `lang` selects the comment style; an
/// unrecognised or plain-text language yields one [`Kind::Plain`] segment per
/// line.
#[must_use]
pub fn highlight(lines: &[&str], lang: Option<&str>) -> Vec<Vec<Seg>> {
    let Some(syntax) = lang.and_then(Syntax::for_lang) else {
        return lines
            .iter()
            .map(|l| {
                vec![Seg {
                    text: (*l).to_string(),
                    kind: Kind::Plain,
                }]
            })
            .collect();
    };
    let mut carry = Carry::None;
    lines
        .iter()
        .map(|line| highlight_line(line, &syntax, &mut carry))
        .collect()
}

/// Append `text` as a `kind` run, merging into the previous run when the class
/// matches so adjacent same-colour text is one span.
fn push(out: &mut Vec<Seg>, text: &str, kind: Kind) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.kind == kind
    {
        last.text.push_str(text);
        return;
    }
    out.push(Seg {
        text: text.to_string(),
        kind,
    });
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Highlight one source line given the incoming [`Carry`] state; updates `carry`
/// for the next line.
fn highlight_line(line: &str, syntax: &Syntax, carry: &mut Carry) -> Vec<Seg> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Seg> = Vec::new();
    let mut i = 0usize;

    // Resume an open multi-line construct from the previous line.
    match *carry {
        Carry::Triple(q) => {
            let (end, closed) = scan_triple_body(&chars, 0, q);
            push(&mut out, &collect(&chars, 0, end), Kind::Str);
            if closed {
                *carry = Carry::None;
                i = end;
            } else {
                return out; // whole line is still inside the string
            }
        }
        Carry::Block => {
            let (end, closed) = scan_block_body(&chars, 0);
            push(&mut out, &collect(&chars, 0, end), Kind::Comment);
            if closed {
                *carry = Carry::None;
                i = end;
            } else {
                return out;
            }
        }
        Carry::None => {}
    }

    let n = chars.len();
    while i < n {
        let c = chars[i];

        // Line comments run to end-of-line.
        if is_line_comment(&chars, i, syntax) {
            push(&mut out, &collect(&chars, i, n), Kind::Comment);
            break;
        }
        // Block comment `/* … */` (may carry to the next line).
        if syntax.slash_comment && c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let (end, closed) = scan_block_body(&chars, i + 2);
            push(&mut out, &collect(&chars, i, end), Kind::Comment);
            i = end;
            if !closed {
                *carry = Carry::Block;
                break;
            }
            continue;
        }
        // Triple-quoted string (Python) — may carry.
        if syntax.triple_strings
            && (c == '"' || c == '\'')
            && i + 2 < n
            && chars[i + 1] == c
            && chars[i + 2] == c
        {
            let (end, closed) = scan_triple_body(&chars, i + 3, c);
            push(&mut out, &collect(&chars, i, end), Kind::Str);
            i = end;
            if !closed {
                *carry = Carry::Triple(c);
                break;
            }
            continue;
        }
        // Double-quote / backtick strings.
        if c == '"' || (c == '`' && syntax.backtick_strings) {
            let end = scan_string(&chars, i + 1, c);
            push(&mut out, &collect(&chars, i, end), Kind::Str);
            i = end;
            continue;
        }
        // Single quote: a string (Python/JS/shell) or a char literal (C/Rust/…),
        // where a bare `'a` is a lifetime/label — plain, not a runaway string.
        if c == '\'' {
            let is_string = if syntax.char_literals {
                // `'x'` or `'\x'` is a char literal; anything else is a lifetime.
                if chars.get(i + 1) == Some(&'\\') {
                    chars.get(i + 3) == Some(&'\'')
                } else {
                    chars.get(i + 2) == Some(&'\'')
                }
            } else {
                true
            };
            if is_string {
                let end = scan_string(&chars, i + 1, '\'');
                push(&mut out, &collect(&chars, i, end), Kind::Str);
                i = end;
            } else {
                push(&mut out, "'", Kind::Plain);
                i += 1;
            }
            continue;
        }
        // Number literal (not part of an identifier). A `.` is only consumed when
        // followed by a digit, so `1..100` (a range) and `3.method()` don't get
        // swallowed into the number.
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < n
                && (chars[j].is_ascii_alphanumeric()
                    || chars[j] == '_'
                    || (chars[j] == '.' && chars.get(j + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                j += 1;
            }
            push(&mut out, &collect(&chars, i, j), Kind::Number);
            i = j;
            continue;
        }
        // Identifier / keyword / function call.
        if is_ident_start(c) {
            let mut j = i + 1;
            while j < n && is_ident_continue(chars[j]) {
                j += 1;
            }
            let word = collect(&chars, i, j);
            let kind = if KEYWORDS.contains(&word.as_str()) {
                Kind::Keyword
            } else if j < n && chars[j] == '(' {
                Kind::Function
            } else {
                Kind::Plain
            };
            push(&mut out, &word, kind);
            i = j;
            continue;
        }
        // Anything else (whitespace, operators, punctuation) is plain.
        push(&mut out, &c.to_string(), Kind::Plain);
        i += 1;
    }
    out
}

/// Whether a line comment starts at `i` under `syntax`.
fn is_line_comment(chars: &[char], i: usize, syntax: &Syntax) -> bool {
    let c = chars[i];
    if syntax.hash_comment && c == '#' {
        return true;
    }
    if syntax.slash_comment && c == '/' && chars.get(i + 1) == Some(&'/') {
        return true;
    }
    if syntax.dash_comment && c == '-' && chars.get(i + 1) == Some(&'-') {
        return true;
    }
    false
}

/// Scan a single-line string body from `start` (just past the opening quote) to
/// just past the closing `quote`, honouring `\` escapes; an unterminated string
/// ends at end-of-line (single-line strings don't carry).
fn scan_string(chars: &[char], start: usize, quote: char) -> usize {
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2, // skip the escaped char
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Scan a triple-quoted body from `start` to just past the closing `q q q`;
/// returns `(end, closed)`.
fn scan_triple_body(chars: &[char], start: usize, q: char) -> (usize, bool) {
    let n = chars.len();
    let mut i = start;
    while i < n {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == q
            && i + 2 < n + 1
            && chars.get(i + 1) == Some(&q)
            && chars.get(i + 2) == Some(&q)
        {
            return (i + 3, true);
        }
        i += 1;
    }
    (n, false)
}

/// Scan a block-comment body from `start` to just past the closing `*/`; returns
/// `(end, closed)`.
fn scan_block_body(chars: &[char], start: usize) -> (usize, bool) {
    let n = chars.len();
    let mut i = start;
    while i < n {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
            return (i + 2, true);
        }
        i += 1;
    }
    (n, false)
}

/// Collect `chars[a..b]` into a `String`.
fn collect(chars: &[char], a: usize, b: usize) -> String {
    chars[a..b].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten one line's highlight into `(kind, text)` pairs for terse asserts.
    fn segs(line: &str, lang: &str) -> Vec<(Kind, String)> {
        highlight(&[line], Some(lang))
            .pop()
            .unwrap()
            .into_iter()
            .map(|s| (s.kind, s.text))
            .collect()
    }

    /// The kind covering the first occurrence of `needle` in `line`.
    fn kind_of(line: &str, lang: &str, needle: &str) -> Kind {
        for (kind, text) in segs(line, lang) {
            if text.contains(needle) {
                return kind;
            }
        }
        panic!("{needle:?} not found in {line:?}");
    }

    #[test]
    fn segments_concatenate_back_to_the_source_line() {
        for line in [
            "def guessing_game():",
            "    print(\"🎮 Welcome!\")  # hi",
            "x = random.randint(1, 100)",
            "return f\"got {n}\"",
        ] {
            let joined: String = segs(line, "python").into_iter().map(|(_, t)| t).collect();
            assert_eq!(joined, line, "round-trips");
        }
    }

    #[test]
    fn keywords_are_classified() {
        assert_eq!(kind_of("def f():", "python", "def"), Kind::Keyword);
        assert_eq!(kind_of("import random", "python", "import"), Kind::Keyword);
        assert_eq!(kind_of("while True:", "python", "while"), Kind::Keyword);
        assert_eq!(kind_of("while True:", "python", "True"), Kind::Keyword);
        assert_eq!(kind_of("return x", "python", "return"), Kind::Keyword);
    }

    #[test]
    fn calls_are_functions_but_keywords_win() {
        // A name directly before '(' is a function call...
        assert_eq!(kind_of("print(x)", "python", "print"), Kind::Function);
        assert_eq!(
            kind_of("guessing_game()", "python", "guessing_game"),
            Kind::Function
        );
        assert_eq!(kind_of("int(guess)", "python", "int"), Kind::Function);
        // ...but a keyword before '(' stays a keyword (e.g. `return(x)`).
        assert_eq!(kind_of("return(x)", "python", "return"), Kind::Keyword);
    }

    #[test]
    fn method_calls_after_a_dot_are_functions() {
        // `random` is a plain identifier (it merges with the following `.`);
        // `randint`, sitting before `(`, is a call.
        assert_eq!(
            kind_of("random.randint(1, 100)", "python", "random"),
            Kind::Plain
        );
        assert_eq!(
            kind_of("random.randint(1, 100)", "python", "randint"),
            Kind::Function
        );
    }

    #[test]
    fn strings_numbers_and_comments() {
        assert_eq!(kind_of("x = \"hello\"", "python", "\"hello\""), Kind::Str);
        assert_eq!(kind_of("x = 100", "python", "100"), Kind::Number);
        assert_eq!(kind_of("x = 3.14", "python", "3.14"), Kind::Number);
        assert_eq!(kind_of("x = 1  # note", "python", "# note"), Kind::Comment);
    }

    #[test]
    fn emoji_inside_a_string_stays_in_the_string() {
        assert_eq!(
            kind_of("print(\"🎮 Welcome!\")", "python", "🎮 Welcome!"),
            Kind::Str
        );
    }

    #[test]
    fn python_slash_slash_is_not_a_comment() {
        // `//` is floor division in Python, not a comment.
        let s = segs("x = a // b", "python");
        assert!(
            !s.iter().any(|(k, _)| *k == Kind::Comment),
            "no comment in {s:?}"
        );
    }

    #[test]
    fn c_like_uses_slash_comments_and_blocks() {
        assert_eq!(kind_of("int x; // note", "c", "// note"), Kind::Comment);
        let block = segs("a; /* mid */ b;", "c");
        assert!(
            block
                .iter()
                .any(|(k, t)| *k == Kind::Comment && t == "/* mid */")
        );
    }

    #[test]
    fn a_block_comment_carries_across_lines() {
        let lines = ["a /* start", "still comment", "end */ b"];
        let out = highlight(&lines, Some("c"));
        // Every segment of the middle line is a comment.
        assert!(out[1].iter().all(|s| s.kind == Kind::Comment));
        // The last line resumes code after `*/`.
        assert!(
            out[2]
                .iter()
                .any(|s| s.kind == Kind::Plain && s.text.contains('b'))
        );
    }

    #[test]
    fn a_triple_quoted_string_carries_across_lines() {
        let lines = ["doc = \"\"\"", "multi", "line\"\"\"", "code = 1"];
        let out = highlight(&lines, Some("python"));
        assert!(out[1].iter().all(|s| s.kind == Kind::Str), "{:?}", out[1]);
        assert!(out[2].iter().any(|s| s.kind == Kind::Str));
        // Code resumes after the closing triple quote.
        assert!(out[3].iter().any(|s| s.kind == Kind::Number));
    }

    #[test]
    fn number_stops_at_a_range_operator() {
        // `1..100` (a Rust range) must not be one giant Number.
        let s = segs("for i in 1..100 {", "rust");
        assert!(s.iter().any(|(k, t)| *k == Kind::Number && t == "1"));
        assert!(s.iter().any(|(k, t)| *k == Kind::Number && t == "100"));
        assert!(
            !s.iter()
                .any(|(k, t)| *k == Kind::Number && t.contains("..")),
            "the `..` is not part of the number: {s:?}"
        );
    }

    #[test]
    fn rust_lifetimes_are_not_strings() {
        let s = segs("impl<'a> Foo<'a> for Bar {", "rust");
        assert!(
            !s.iter().any(|(k, _)| *k == Kind::Str),
            "a lifetime must not open a string: {s:?}"
        );
        // A real Rust char literal still highlights.
        assert_eq!(kind_of("let c = 'x';", "rust", "'x'"), Kind::Str);
        assert_eq!(kind_of("let c = '\\n';", "rust", "'\\n'"), Kind::Str);
    }

    #[test]
    fn python_single_quoted_strings_still_work() {
        // Python has no char literals — `'hello'` is a full string.
        assert_eq!(
            kind_of("x = 'hello world'", "python", "'hello world'"),
            Kind::Str
        );
    }

    #[test]
    fn unknown_and_plain_languages_are_not_highlighted() {
        assert_eq!(
            highlight(&["def f():"], Some("text")),
            vec![vec![Seg {
                text: "def f():".into(),
                kind: Kind::Plain
            }]]
        );
        // No language at all → plain.
        assert_eq!(highlight(&["def f():"], None)[0][0].kind, Kind::Plain);
    }

    #[test]
    fn prefix_stability_a_committed_code_line_never_recolours() {
        // Stream a Python block char-by-char and confirm that once a line is
        // "complete" (a later line exists), its highlight is frozen — the model
        // for stable_commit's all-but-last flush.
        let full = "x = \"\"\"\nhello\nworld\n\"\"\"\ny = f(1)";
        let rows_of = |t: &str| -> Vec<Vec<(Kind, String)>> {
            let lines: Vec<&str> = t.split('\n').collect();
            highlight(&lines, Some("python"))
                .into_iter()
                .map(|segs| segs.into_iter().map(|s| (s.kind, s.text)).collect())
                .collect()
        };
        let mut committed: Vec<Vec<(Kind, String)>> = Vec::new();
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

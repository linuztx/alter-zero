//! A `bash` command line as the read-only checks see it: its simple commands,
//! each one's words with the quoting resolved, and every construct a verdict
//! about the words alone could not see through (`docs/tool-groups.md`).
//!
//! Deliberately small — bash's own grammar is not the goal. A shape this
//! lexer does not model is not guessed at: it raises a [`Hazards`] flag, and
//! [`super::is_read_only`] refuses any command that raised one, so the cost of
//! a gap here is a command sent to the classifier, never one run unasked.

/// One simple command — a pipeline stage or a list element — as its words,
/// quoting already resolved (`'ls'` and `ls` are the same word).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Segment {
    pub(super) words: Vec<String>,
}

impl Segment {
    /// The program word, when the segment has one.
    pub(super) fn program(&self) -> Option<&str> {
        self.words.first().map(String::as_str)
    }

    /// The words after the program.
    pub(super) fn args(&self) -> &[String] {
        self.words.get(1..).unwrap_or_default()
    }
}

/// The constructs a verdict about the words cannot see through. Every one of
/// them refuses [`super::is_read_only`]; the display classifier
/// ([`super::read_kind`]) refuses only the ones that can hide a *write* or a
/// second program — see [`Hazards::hides_a_write`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per construct, read by name
pub(super) struct Hazards {
    /// A `$` outside single quotes: a variable, `$'…'`, arithmetic — the word
    /// the check reads is not the word that runs.
    pub(super) expansion: bool,
    /// A backtick, `$(`, or a process substitution `<(` / `>(`: another
    /// program runs inside this one.
    pub(super) substitution: bool,
    /// A redirection other than to `/dev/null` or between the standard
    /// streams — a file written (or read from somewhere the words don't say).
    pub(super) redirect: bool,
    /// An unquoted `*`, `?`, `[` or `]`: the file names the glob expands to
    /// become words nobody has read (a file named `-delete`).
    pub(super) glob: bool,
    /// Brace expansion — an unquoted `{…}` holding a `,` or `..`: one word
    /// becomes several nobody has read.
    pub(super) brace: bool,
    /// A lone `&` or `|&`: something left running.
    pub(super) background: bool,
    /// An unquoted `(` / `)` or `;;`: a subshell or a case arm.
    pub(super) subshell: bool,
    /// An unquoted `#`: a comment, or a word the reader may split differently.
    pub(super) comment: bool,
    /// A backslash outside quotes, other than `\(` / `\)`.
    pub(super) escape: bool,
    /// A newline or carriage return anywhere.
    pub(super) newline: bool,
    /// Another control character, or a Unicode space bash does not split on.
    pub(super) control: bool,
    /// A separator with nothing before it (`ls && && ls`), or a trailing
    /// `|` / `&&` / `||`, or a redirection with no target.
    pub(super) syntax: bool,
}

impl Hazards {
    /// Whether any construct was met at all.
    pub(super) fn any(self) -> bool {
        self != Self::default()
    }

    /// The constructs that can hide a **write** or a second program from a
    /// reader of the words — what the display classifier refuses. A glob or a
    /// variable changes which files are read, never that the command reads.
    pub(super) fn hides_a_write(self) -> bool {
        self.substitution || self.redirect || self.background || self.subshell || self.syntax
    }
}

/// A command line split into its simple commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Parsed {
    pub(super) segments: Vec<Segment>,
    pub(super) hazards: Hazards,
}

/// Split `command` into its simple commands — `None` when a quote never
/// closes, which is a command no verdict should be reached about.
pub(super) fn parse(command: &str) -> Option<Parsed> {
    let tokens = tokenize(command)?;
    Some(assemble(tokens.tokens, tokens.hazards))
}

/// One lexed piece of the line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A word, quoting resolved. `bare_digits` marks an unquoted all-digit
    /// word — what a following `>` takes as its file descriptor.
    Word { text: String, bare_digits: bool },
    /// `&&`, `||`, `;`, `|` or a newline: the end of one simple command.
    /// `joins` for the ones that need a command on their right (`&&`, `||`,
    /// `|`); `newline` for a line break, which after a join is only a
    /// continuation.
    Separator { joins: bool, newline: bool },
    /// `&` or `|&`.
    Background,
    /// `(`, `)` or `;;`.
    Group,
    /// A redirection operator, with the file descriptor written before it.
    Redirect {
        fd: Option<String>,
        op: &'static str,
    },
}

struct Lexed {
    tokens: Vec<Token>,
    hazards: Hazards,
}

/// Unicode spaces bash does not split on, but a reader takes for a space.
fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200B}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// The word being built.
#[derive(Default)]
struct Word {
    text: String,
    /// The word exists — even an empty one, after `''`.
    open: bool,
    /// Some of it was quoted or escaped.
    quoted: bool,
    /// It holds an unquoted `{` or `}`.
    brace: bool,
}

impl Word {
    fn push(&mut self, c: char) {
        self.text.push(c);
        self.open = true;
    }

    /// Whether the word so far is an unquoted run of digits — the file
    /// descriptor a `>` written right after it redirects.
    fn is_fd(&self) -> bool {
        self.open
            && !self.quoted
            && !self.text.is_empty()
            && self.text.bytes().all(|b| b.is_ascii_digit())
    }

    /// Close the word into `tokens`. Braces are a hazard only as brace
    /// *expansion* — a comma or `..` between them, which turns one word into
    /// several nobody has read; a bare `{}` (`xargs -I {}`) is only a word.
    fn end(&mut self, tokens: &mut Vec<Token>, hazards: &mut Hazards) {
        let word = std::mem::take(self);
        if !word.open {
            return;
        }
        if word.brace && (word.text.contains(',') || word.text.contains("..")) {
            hazards.brace = true;
        }
        let bare_digits = word.is_fd();
        tokens.push(Token::Word {
            text: word.text,
            bare_digits,
        });
    }
}

/// The lexer proper: words with quoting resolved, operators, and hazards.
#[allow(clippy::too_many_lines)] // one state machine reads best in one place
fn tokenize(command: &str) -> Option<Lexed> {
    let mut tokens = Vec::new();
    let mut hazards = Hazards::default();
    let mut word = Word::default();
    let mut chars = command.chars().peekable();
    let separator = |joins: bool, newline: bool| Token::Separator { joins, newline };

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => word.end(&mut tokens, &mut hazards),
            '\n' | '\r' => {
                hazards.newline = true;
                word.end(&mut tokens, &mut hazards);
                tokens.push(separator(false, true));
            }
            '\'' => {
                word.open = true;
                word.quoted = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        ch => {
                            if ch == '\n' || ch == '\r' {
                                hazards.newline = true;
                            }
                            word.push(ch);
                        }
                    }
                }
            }
            '"' => {
                word.open = true;
                word.quoted = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        // Inside double quotes a backslash escapes only these;
                        // before anything else it is itself.
                        '\\' => match chars.peek() {
                            Some('$' | '`' | '"' | '\\') => word.push(chars.next()?),
                            Some('\n') => {
                                hazards.newline = true;
                                chars.next();
                            }
                            _ => word.push('\\'),
                        },
                        '$' => {
                            hazards.expansion = true;
                            if chars.peek() == Some(&'(') {
                                hazards.substitution = true;
                            }
                            word.push('$');
                        }
                        '`' => {
                            hazards.substitution = true;
                            word.push('`');
                        }
                        ch => {
                            if ch == '\n' || ch == '\r' {
                                hazards.newline = true;
                            }
                            word.push(ch);
                        }
                    }
                }
            }
            '\\' => match chars.next() {
                // `\(` / `\)` are `find`'s grouping parens: a literal paren
                // in a word, nothing more.
                Some(ch @ ('(' | ')')) => {
                    word.quoted = true;
                    word.push(ch);
                }
                Some('\n') => hazards.newline = true,
                Some(ch) => {
                    hazards.escape = true;
                    word.quoted = true;
                    word.push(ch);
                }
                None => hazards.escape = true,
            },
            '$' => {
                hazards.expansion = true;
                if chars.peek() == Some(&'(') {
                    hazards.substitution = true;
                }
                word.push('$');
            }
            '`' => {
                hazards.substitution = true;
                word.push('`');
            }
            '#' => {
                hazards.comment = true;
                if word.open {
                    word.push('#');
                } else {
                    // A comment runs to the end of the line.
                    while chars.peek().is_some_and(|&ch| ch != '\n') {
                        chars.next();
                    }
                }
            }
            '*' | '?' | '[' | ']' => {
                hazards.glob = true;
                word.push(c);
            }
            '{' | '}' => {
                word.brace = true;
                word.push(c);
            }
            '(' | ')' => {
                word.end(&mut tokens, &mut hazards);
                tokens.push(Token::Group);
            }
            ';' => {
                word.end(&mut tokens, &mut hazards);
                if chars.peek() == Some(&';') {
                    chars.next();
                    tokens.push(Token::Group);
                } else {
                    tokens.push(separator(false, false));
                }
            }
            '&' => {
                word.end(&mut tokens, &mut hazards);
                match chars.peek() {
                    Some('&') => {
                        chars.next();
                        tokens.push(separator(true, false));
                    }
                    Some('>') => {
                        chars.next();
                        let op = if chars.peek() == Some(&'>') {
                            chars.next();
                            "&>>"
                        } else {
                            "&>"
                        };
                        tokens.push(Token::Redirect { fd: None, op });
                    }
                    _ => tokens.push(Token::Background),
                }
            }
            '|' => {
                word.end(&mut tokens, &mut hazards);
                match chars.peek() {
                    Some('|') => {
                        chars.next();
                        tokens.push(separator(true, false));
                    }
                    Some('&') => {
                        chars.next();
                        tokens.push(Token::Background);
                    }
                    _ => tokens.push(separator(true, false)),
                }
            }
            '<' | '>' => {
                // `2>` — an unquoted all-digit word right before the operator
                // is the file descriptor it redirects, not an argument.
                let fd = if word.is_fd() {
                    Some(std::mem::take(&mut word).text)
                } else {
                    word.end(&mut tokens, &mut hazards);
                    None
                };
                let op = if c == '>' {
                    match chars.peek() {
                        Some('>') => {
                            chars.next();
                            ">>"
                        }
                        Some('|') => {
                            chars.next();
                            ">|"
                        }
                        Some('&') => {
                            chars.next();
                            ">&"
                        }
                        Some('(') => {
                            hazards.substitution = true;
                            ">"
                        }
                        _ => ">",
                    }
                } else {
                    match chars.peek() {
                        Some('<') => {
                            chars.next();
                            if chars.peek() == Some(&'<') {
                                chars.next();
                                "<<<"
                            } else {
                                "<<"
                            }
                        }
                        Some('&') => {
                            chars.next();
                            "<&"
                        }
                        Some('>') => {
                            chars.next();
                            "<>"
                        }
                        Some('(') => {
                            hazards.substitution = true;
                            "<"
                        }
                        _ => "<",
                    }
                };
                tokens.push(Token::Redirect { fd, op });
            }
            other => {
                if (other.is_control() && other != '\t') || is_unicode_space(other) {
                    hazards.control = true;
                }
                word.push(other);
            }
        }
    }
    word.end(&mut tokens, &mut hazards);
    Some(Lexed { tokens, hazards })
}

/// A redirection that writes nothing and reads nothing a reader would miss:
/// to or from `/dev/null`, or one standard stream duplicated onto another
/// (`2>&1`, `>&2`).
fn harmless_redirect(fd: Option<&str>, op: &str, target: &str) -> bool {
    let standard = |fd: Option<&str>| matches!(fd, None | Some("0" | "1" | "2"));
    match op {
        ">&" | "<&" => standard(fd) && matches!(target, "0" | "1" | "2" | "-"),
        ">" | ">>" | ">|" => standard(fd) && target == "/dev/null",
        "&>" | "&>>" => fd.is_none() && target == "/dev/null",
        "<" => matches!(fd, None | Some("0")) && target == "/dev/null",
        // Here-documents and here-strings, `<>`: never harmless.
        _ => false,
    }
}

/// Group the tokens into simple commands, settling every redirection and
/// raising the structural hazards.
fn assemble(tokens: Vec<Token>, mut hazards: Hazards) -> Parsed {
    let mut segments = Vec::new();
    let mut current = Segment::default();
    // A `|`, `&&` or `||` waiting for the command it joins.
    let mut dangling = false;
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        match token {
            Token::Word { text, .. } => {
                current.words.push(text);
                dangling = false;
            }
            Token::Separator { joins, newline } => {
                if current.words.is_empty() {
                    // A blank line — or a line break right after `&&`/`|`,
                    // which only continues the command — is nothing at all.
                    if newline {
                        continue;
                    }
                    // `ls && && ls`, a leading `|`, `ls && ;`: a join with
                    // nothing on its left, or nothing on its right.
                    if joins || dangling {
                        hazards.syntax = true;
                    }
                } else {
                    segments.push(std::mem::take(&mut current));
                }
                dangling = joins;
            }
            Token::Background => {
                hazards.background = true;
                if !current.words.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            Token::Group => hazards.subshell = true,
            Token::Redirect { fd, op } => match iter.next() {
                Some(Token::Word { text, .. }) => {
                    if !harmless_redirect(fd.as_deref(), op, &text) {
                        hazards.redirect = true;
                    }
                }
                _ => {
                    hazards.redirect = true;
                    hazards.syntax = true;
                }
            },
        }
    }
    if current.words.is_empty() {
        if dangling {
            hazards.syntax = true;
        }
    } else {
        segments.push(current);
    }
    Parsed { segments, hazards }
}

//! Read-only `bash` commands: which ones only look (`docs/tool-groups.md`).
//!
//! Two questions, one parser. [`is_read_only`] says whether a command can be
//! trusted to change nothing — every segment a known reader with arguments
//! that keep it one — which is what lets auto mode run it without asking the
//! classifier (`docs/permissions.md`). [`reading`] says what such a command
//! reads *as* — a search, a read, a listing — which is what the folded tool
//! view counts.
//!
//! The rules are Claude Code's, cut to what a model actually runs: a small
//! quote-aware lexer, then a contract per program. Anything the lexer cannot
//! account for — a substitution, an expansion, a redirect into a file, a
//! subshell, a wrapper, a program it does not know — answers **no**. A wrong
//! no costs one classifier round; a wrong yes runs something unreviewed, so
//! every doubt resolves to no.

/// What a read-only command does, as the folded tool view counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// Looks for something: `grep`, `rg`, `find`, `which`.
    Search,
    /// Reads a file's contents: `cat`, `head`, `wc`, `sed -n`.
    Read,
    /// Lists a directory: `ls`, `tree`, `du`.
    List,
}

/// A read-only command as the folded view sees it: its kind, and the files
/// or directories it names when that can be told from its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// What the command does.
    pub kind: ReadKind,
    /// The operands it reads or lists, in order — empty when it names none
    /// (`ls`, a filter reading standard input).
    pub targets: Vec<String>,
}

/// Whether `command` only reads: every segment of it a known reader whose
/// arguments keep it one, with nothing redirected into a file, nothing
/// substituted and nothing expanded. An empty command is not.
#[must_use]
pub fn is_read_only(command: &str) -> bool {
    verdicts(command).is_some()
}

/// What a read-only `command` reads as, or `None` when it is not read-only
/// or some part of it does nothing the folded view counts (`git status`).
///
/// Claude Code's rule: every part that says anything must search, read or
/// list — `echo`, `printf`, `true`, `false`, `:` and `cd` say nothing — and
/// the whole takes its strongest part's kind, a listing over a search over a
/// read (`cat a | grep b` is a search). The targets are that kind's parts'.
#[must_use]
pub fn reading(command: &str) -> Option<Reading> {
    let verdicts = verdicts(command)?;
    let mut kind: Option<ReadKind> = None;
    for verdict in &verdicts {
        match verdict {
            Verdict::Neutral => {}
            Verdict::Quiet => return None,
            Verdict::Reads(reading) => kind = Some(strongest(kind, reading.kind)),
        }
    }
    let kind = kind?;
    let targets = verdicts
        .into_iter()
        .filter_map(|verdict| match verdict {
            Verdict::Reads(reading) if reading.kind == kind => Some(reading.targets),
            _ => None,
        })
        .flatten()
        .collect();
    Some(Reading { kind, targets })
}

/// The stronger of two kinds — a listing over a search over a read.
fn strongest(current: Option<ReadKind>, next: ReadKind) -> ReadKind {
    let rank = |kind: ReadKind| match kind {
        ReadKind::List => 2,
        ReadKind::Search => 1,
        ReadKind::Read => 0,
    };
    match current {
        Some(current) if rank(current) >= rank(next) => current,
        _ => next,
    }
}

/// One read-only segment's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Says nothing: `echo`, `cd`.
    Neutral,
    /// Read-only, but neither searches, reads nor lists: `git status`, `pwd`.
    Quiet,
    /// Searches, reads or lists.
    Reads(Reading),
}

/// Every segment's verdict, or `None` when any segment is not read-only (or
/// there are none).
fn verdicts(command: &str) -> Option<Vec<Verdict>> {
    let tokens = lex(command)?;
    let segments = segments(tokens)?;
    if segments.is_empty() {
        return None;
    }
    segments
        .iter()
        .map(|words| segment_verdict(words))
        .collect()
}

// ===== the lexer =====

/// One shell word, quotes and escapes resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    /// The text with its quoting resolved.
    text: String,
    /// Typed with no quote or escape at all — what a program name must be.
    plain: bool,
    /// Holds an unquoted, unescaped `*`, `?` or `[`.
    glob: bool,
    /// Holds a live expansion: `$name`, `${…}`, `$(…)`, a backtick.
    expands: bool,
    /// Holds an unquoted `{` or `}` — a group or a brace expansion.
    brace: bool,
}

impl Word {
    const fn new() -> Self {
        Self {
            text: String::new(),
            plain: true,
            glob: false,
            expands: false,
            brace: false,
        }
    }
}

/// A shell operator the segment splitter or the redirect check reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `|`, `|&`, `||`, `&&`, `&`, `;` or a newline — between segments.
    Separator,
    /// `>`, `>>`, `>|`, `&>`, `&>>` — output into the next word.
    Out,
    /// `<` — input from the next word.
    In,
    /// `>&`, `<&` — onto the descriptor the next word names.
    Dup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(Word),
    Op(Op),
}

/// Lex `command` into words and operators, or `None` for anything a reader
/// never needs: an unclosed quote, a subshell or process substitution, a
/// heredoc, ANSI-C `$'…'` quoting, `;;`.
fn lex(command: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut word: Option<Word> = None;
    let mut chars = command.chars().peekable();
    // A word under way ends at the next separator or operator; an all-digit
    // one in front of a redirect is that redirect's descriptor, not a word.
    let finish = |tokens: &mut Vec<Token>, word: &mut Option<Word>| {
        if let Some(word) = word.take() {
            tokens.push(Token::Word(word));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => finish(&mut tokens, &mut word),
            '\n' => {
                finish(&mut tokens, &mut word);
                tokens.push(Token::Op(Op::Separator));
            }
            // A comment: the rest of the line says nothing.
            '#' if word.is_none() => {
                while chars.peek().is_some_and(|&c| c != '\n') {
                    chars.next();
                }
            }
            '\'' => {
                let current = word.get_or_insert_with(Word::new);
                current.plain = false;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => current.text.push(c),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(Word::new);
                current.plain = false;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('$' | '`' | '"' | '\\') => current.text.push(c),
                            '\n' => {}
                            c => {
                                current.text.push('\\');
                                current.text.push(c);
                            }
                        },
                        '`' => current.expands = true,
                        '$' => {
                            if expansion_follows(chars.peek().copied()) {
                                current.expands = true;
                            }
                            current.text.push('$');
                        }
                        c => current.text.push(c),
                    }
                }
            }
            '\\' => match chars.next() {
                // A line continuation joins the lines.
                Some('\n') => {}
                Some(c) => {
                    let current = word.get_or_insert_with(Word::new);
                    current.plain = false;
                    current.text.push(c);
                }
                None => return None,
            },
            '$' => {
                let next = chars.peek().copied();
                // `$'…'` and `$"…"` quote in ways a reader never needs.
                if matches!(next, Some('\'' | '"')) {
                    return None;
                }
                let current = word.get_or_insert_with(Word::new);
                if expansion_follows(next) {
                    current.expands = true;
                }
                current.text.push('$');
            }
            '`' => word.get_or_insert_with(Word::new).expands = true,
            '*' | '?' | '[' => {
                let current = word.get_or_insert_with(Word::new);
                current.glob = true;
                current.text.push(c);
            }
            '{' | '}' => {
                let current = word.get_or_insert_with(Word::new);
                current.brace = true;
                current.text.push(c);
            }
            '(' | ')' => return None,
            '|' => {
                finish(&mut tokens, &mut word);
                if matches!(chars.peek(), Some('|' | '&')) {
                    chars.next();
                }
                tokens.push(Token::Op(Op::Separator));
            }
            ';' => {
                finish(&mut tokens, &mut word);
                if chars.peek() == Some(&';') {
                    return None;
                }
                tokens.push(Token::Op(Op::Separator));
            }
            '&' => {
                finish(&mut tokens, &mut word);
                match chars.peek() {
                    Some('&') => {
                        chars.next();
                        tokens.push(Token::Op(Op::Separator));
                    }
                    Some('>') => {
                        chars.next();
                        if chars.peek() == Some(&'>') {
                            chars.next();
                        }
                        tokens.push(Token::Op(Op::Out));
                    }
                    _ => tokens.push(Token::Op(Op::Separator)),
                }
            }
            '>' | '<' => {
                // `2>` names the descriptor it redirects: the digits are part
                // of the operator, not a word of their own.
                if word.as_ref().is_some_and(|w| {
                    w.plain && !w.text.is_empty() && w.text.chars().all(|c| c.is_ascii_digit())
                }) {
                    word = None;
                }
                finish(&mut tokens, &mut word);
                let op = match (c, chars.peek().copied()) {
                    ('>', Some('>' | '|')) => {
                        chars.next();
                        Op::Out
                    }
                    (_, Some('&')) => {
                        chars.next();
                        Op::Dup
                    }
                    // A heredoc, a here-string, a process substitution, `<>`.
                    ('<', Some('<' | '(' | '>')) | ('>', Some('(')) => return None,
                    ('>', _) => Op::Out,
                    _ => Op::In,
                };
                tokens.push(Token::Op(op));
            }
            c => word.get_or_insert_with(Word::new).text.push(c),
        }
    }
    finish(&mut tokens, &mut word);
    Some(tokens)
}

/// Whether a `$` followed by `next` expands — a name, a positional or
/// special parameter, `${…}`, `$(…)` or `$[…]` — rather than standing for
/// itself (`'foo$'` and `"^end$"` anchor a regex).
fn expansion_follows(next: Option<char>) -> bool {
    next.is_some_and(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '_' | '{' | '(' | '[' | '@' | '*' | '#' | '?' | '!' | '$' | '-'
            )
    })
}

/// Split the tokens into segments of words, checking each redirect on the
/// way: only `/dev/null` may be written to or read from, and a descriptor may
/// be duplicated (`2>&1`). `None` for any other redirect, or an operator with
/// no word after it.
fn segments(tokens: Vec<Token>) -> Option<Vec<Vec<Word>>> {
    let mut segments = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    let mut tokens = tokens.into_iter();
    while let Some(token) = tokens.next() {
        match token {
            Token::Word(word) => current.push(word),
            Token::Op(Op::Separator) => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            Token::Op(op) => {
                let Some(Token::Word(target)) = tokens.next() else {
                    return None;
                };
                if target.expands || target.glob || target.brace {
                    return None;
                }
                let allowed = match op {
                    Op::Out | Op::In => target.text == "/dev/null",
                    Op::Dup => {
                        target.text == "-" || target.text.chars().all(|c| c.is_ascii_digit())
                    }
                    Op::Separator => unreachable!("matched above"),
                };
                if !allowed {
                    return None;
                }
            }
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    Some(segments)
}

// ===== the programs =====

/// One segment's verdict: its program's contract over its arguments.
fn segment_verdict(words: &[Word]) -> Option<Verdict> {
    let (program, args) = words.split_first()?;
    // The program is a bare name: no quoting, no path, no assignment.
    if !program.plain
        || program.glob
        || program.brace
        || program.expands
        || program.text.contains(['/', '='])
    {
        return None;
    }
    if args.iter().any(|arg| arg.expands || arg.brace) {
        return None;
    }
    let contract = contract(&program.text)?;
    if !contract.glob_safe && args.iter().any(|arg| arg.glob) {
        return None;
    }
    let texts: Vec<&str> = args.iter().map(|arg| arg.text.as_str()).collect();
    if forbidden(&texts, contract.forbid) {
        return None;
    }
    (contract.check)(&texts)
}

/// A program's read-only contract.
struct Contract {
    /// Flags that make the program write or run something.
    forbid: Forbid,
    /// Whether an unquoted glob may sit among its arguments — only where no
    /// flag it could expand into does anything but read.
    glob_safe: bool,
    /// The program's own rule over its arguments, and the verdict it gives.
    check: fn(&[&str]) -> Option<Verdict>,
}

/// The flags that disqualify a program.
#[derive(Clone, Copy)]
struct Forbid {
    /// Long flags, as `--name` or `--name=value`.
    long: &'static [&'static str],
    /// Short flags, by letter, anywhere in a bundle (`-uo` holds `o`).
    short: &'static [char],
    /// Whole single-dash words (`find`'s `-exec`).
    words: &'static [&'static str],
}

const NOTHING: Forbid = Forbid {
    long: &[],
    short: &[],
    words: &[],
};

/// Whether any of `args` (before a `--`) is a forbidden flag.
fn forbidden(args: &[&str], forbid: Forbid) -> bool {
    for arg in args {
        if *arg == "--" {
            return false;
        }
        if forbid.words.contains(arg) {
            return true;
        }
        if arg.starts_with("--") {
            if forbid.long.iter().any(|name| abbreviates(arg, name)) {
                return true;
            }
        } else if let Some(bundle) = arg.strip_prefix('-')
            && bundle
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && bundle.chars().any(|c| forbid.short.contains(&c))
        {
            return true;
        }
    }
    false
}

/// Whether the long flag `arg` (`--name`, `--name=value`) is `flag` or an
/// abbreviation of it: GNU `getopt_long` and git both take any unambiguous
/// prefix of a long option as the option itself, so `sort --out=x` *is*
/// `--output=x`. Matching every prefix also refuses an ambiguous one, which
/// the program would refuse too.
fn abbreviates(arg: &str, flag: &str) -> bool {
    let Some(long) = arg.strip_prefix("--") else {
        return false;
    };
    let name = long.split_once('=').map_or(long, |(name, _)| name);
    !name.is_empty()
        && flag
            .strip_prefix("--")
            .is_some_and(|flag| flag.starts_with(name))
}

/// The arguments split into operands, skipping flags and the values that
/// `takes_value` flags carry in the next word (`-n 20`); a short bundle
/// ending in one takes it too (`-fn 20`). Everything after `--` is an
/// operand; a lone `-` is standard input, not a name.
fn operands<'a>(args: &[&'a str], takes_value: &[&str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(&arg) = iter.next() {
        if arg == "--" {
            out.extend(iter.by_ref().copied());
            break;
        }
        if arg.starts_with("--") {
            if !arg.contains('=') && takes_value.contains(&arg) {
                iter.next();
            }
        } else if let Some(bundle) = arg.strip_prefix('-')
            && !bundle.is_empty()
        {
            let last = bundle
                .chars()
                .last()
                .filter(|_| bundle.chars().all(|c| c.is_ascii_alphabetic()));
            if last.is_some_and(|c| takes_value.contains(&format!("-{c}").as_str())) {
                iter.next();
            }
        } else if arg != "-" {
            out.push(arg);
        }
    }
    out
}

/// A reading of `kind` naming `targets`.
fn reads(kind: ReadKind, targets: Vec<&str>) -> Option<Verdict> {
    Some(Verdict::Reads(Reading {
        kind,
        targets: targets.into_iter().map(str::to_string).collect(),
    }))
}

/// The contract for `program`, or `None` for one this module does not trust.
fn contract(program: &str) -> Option<Contract> {
    let plain = |check: fn(&[&str]) -> Option<Verdict>| Contract {
        forbid: NOTHING,
        glob_safe: true,
        check,
    };
    let guarded = |forbid: Forbid, check: fn(&[&str]) -> Option<Verdict>| Contract {
        forbid,
        glob_safe: false,
        check,
    };
    Some(match program {
        // Say nothing at all.
        "echo" | "printf" | "true" | "false" | ":" => plain(|_| Some(Verdict::Neutral)),
        "cd" => plain(|args| (operands(args, &[]).len() <= 1).then_some(Verdict::Neutral)),
        // Listings.
        "ls" => plain(|args| {
            reads(
                ReadKind::List,
                operands(
                    args,
                    &[
                        "-I",
                        "-w",
                        "-T",
                        "--ignore",
                        "--hide",
                        "--width",
                        "--tabsize",
                        "--block-size",
                        "--format",
                        "--time-style",
                        "--quoting-style",
                        "--indicator-style",
                        "--sort",
                        "--time",
                    ],
                ),
            )
        }),
        "tree" => guarded(
            Forbid {
                long: &["--output"],
                short: &['o', 'R'],
                words: &[],
            },
            |args| {
                reads(
                    ReadKind::List,
                    operands(
                        args,
                        &[
                            "-L",
                            "-P",
                            "-I",
                            "-H",
                            "-T",
                            "--charset",
                            "--filelimit",
                            "--timefmt",
                            "--sort",
                        ],
                    ),
                )
            },
        ),
        "du" => plain(|args| {
            reads(
                ReadKind::List,
                operands(
                    args,
                    &[
                        "-d",
                        "-B",
                        "-t",
                        "-X",
                        "--max-depth",
                        "--block-size",
                        "--threshold",
                        "--exclude-from",
                        "--exclude",
                        "--time-style",
                        "--time",
                    ],
                ),
            )
        }),
        // Searches.
        "grep" | "egrep" | "fgrep" => plain(|_| reads(ReadKind::Search, Vec::new())),
        "rg" => guarded(
            Forbid {
                long: &["--pre", "--pre-glob"],
                short: &[],
                words: &[],
            },
            |_| reads(ReadKind::Search, Vec::new()),
        ),
        "find" => guarded(
            Forbid {
                long: &[],
                short: &[],
                words: &[
                    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0",
                    "-fprintf", "-fls",
                ],
            },
            |_| reads(ReadKind::Search, Vec::new()),
        ),
        "fd" | "fdfind" => guarded(
            Forbid {
                long: &["--exec", "--exec-batch"],
                short: &['x', 'X'],
                words: &[],
            },
            |_| reads(ReadKind::Search, Vec::new()),
        ),
        "which" | "whereis" => plain(|_| reads(ReadKind::Search, Vec::new())),
        // Reads.
        "cat" | "wc" => plain(|args| reads(ReadKind::Read, operands(args, &[]))),
        "tac" => plain(|args| reads(ReadKind::Read, operands(args, &["-s", "--separator"]))),
        "head" | "tail" => plain(|args| {
            reads(
                ReadKind::Read,
                operands(
                    args,
                    &[
                        "-n",
                        "-c",
                        "-s",
                        "--lines",
                        "--bytes",
                        "--sleep-interval",
                        "--pid",
                        "--max-unchanged-stats",
                    ],
                ),
            )
        }),
        "nl" => plain(|args| {
            reads(
                ReadKind::Read,
                operands(
                    args,
                    &["-b", "-d", "-f", "-h", "-i", "-l", "-n", "-s", "-v", "-w"],
                ),
            )
        }),
        "stat" => plain(|args| {
            reads(
                ReadKind::Read,
                operands(args, &["-c", "--format", "--printf"]),
            )
        }),
        "strings" => plain(|args| {
            reads(
                ReadKind::Read,
                operands(
                    args,
                    &[
                        "-n",
                        "-t",
                        "-e",
                        "-T",
                        "--bytes",
                        "--radix",
                        "--encoding",
                        "--target",
                    ],
                ),
            )
        }),
        "file" => guarded(
            Forbid {
                long: &["--compile"],
                short: &['C'],
                words: &[],
            },
            |args| {
                reads(
                    ReadKind::Read,
                    operands(
                        args,
                        &[
                            "-m",
                            "-f",
                            "-F",
                            "-e",
                            "-P",
                            "--magic-file",
                            "--files-from",
                            "--separator",
                            "--exclude",
                            "--exclude-quiet",
                            "--parameter",
                        ],
                    ),
                )
            },
        ),
        "cut" => plain(|args| {
            reads(
                ReadKind::Read,
                operands(
                    args,
                    &[
                        "-b",
                        "-c",
                        "-d",
                        "-f",
                        "--bytes",
                        "--characters",
                        "--delimiter",
                        "--fields",
                        "--output-delimiter",
                    ],
                ),
            )
        }),
        "tr" => plain(|_| reads(ReadKind::Read, Vec::new())),
        "sort" => guarded(
            Forbid {
                long: &["--output", "--temporary-directory", "--compress-program"],
                short: &['o', 'T'],
                words: &[],
            },
            |args| {
                reads(
                    ReadKind::Read,
                    operands(
                        args,
                        &[
                            "-k",
                            "-t",
                            "-S",
                            "--key",
                            "--field-separator",
                            "--buffer-size",
                            "--parallel",
                            "--batch-size",
                            "--sort",
                            "--random-source",
                        ],
                    ),
                )
            },
        ),
        // A second operand is uniq's *output* file.
        "uniq" => guarded(NOTHING, |args| {
            let files = operands(
                args,
                &[
                    "-f",
                    "-s",
                    "-w",
                    "--skip-fields",
                    "--skip-chars",
                    "--check-chars",
                ],
            );
            if files.len() > 1 {
                return None;
            }
            reads(ReadKind::Read, files)
        }),
        "sed" => guarded(NOTHING, sed),
        "jq" => guarded(
            Forbid {
                long: &[
                    "--from-file",
                    "--rawfile",
                    "--slurpfile",
                    "--run-tests",
                    "--library-path",
                ],
                short: &['f', 'L'],
                words: &[],
            },
            jq,
        ),
        // Read-only, but nothing the folded view counts — each shows its own
        // cell, since what it printed is the point.
        "od" | "hexdump" | "rev" | "paste" | "column" | "fold" | "fmt" | "expand" | "unexpand"
        | "comm" | "cmp" | "diff" | "md5sum" | "sha1sum" | "sha224sum" | "sha256sum"
        | "sha384sum" | "sha512sum" | "b2sum" | "cksum" | "readlink" | "realpath" | "basename"
        | "dirname" | "pwd" | "whoami" | "id" | "uname" | "uptime" | "nproc" | "arch"
        | "groups" | "df" | "free" | "seq" | "type" => plain(|_| Some(Verdict::Quiet)),
        "base64" => guarded(
            Forbid {
                long: &["--output"],
                short: &['o'],
                words: &[],
            },
            |_| Some(Verdict::Quiet),
        ),
        // `ps e` (BSD style) prints each process's environment.
        "ps" => guarded(NOTHING, |args| {
            args.iter()
                .all(|arg| arg.starts_with('-') || !arg.contains('e'))
                .then_some(Verdict::Quiet)
        }),
        // A positional sets the clock; a format starts with `+`.
        "date" => guarded(
            Forbid {
                long: &["--set"],
                short: &['s'],
                words: &[],
            },
            |args| {
                operands(args, &["-d", "-f", "-r", "--date", "--file", "--reference"])
                    .iter()
                    .all(|arg| arg.starts_with('+'))
                    .then_some(Verdict::Quiet)
            },
        ),
        // A positional sets the host name.
        "hostname" => guarded(
            Forbid {
                long: &["--file", "--boot"],
                short: &['F', 'b', 'y'],
                words: &[],
            },
            |args| operands(args, &[]).is_empty().then_some(Verdict::Quiet),
        ),
        "git" => guarded(NOTHING, git),
        _ => return None,
    })
}

/// `sed`, only as a printer: quiet (`-n`), every command a numeric or `$`
/// address before `p`, no flag that edits in place or reads a script.
fn sed(args: &[&str]) -> Option<Verdict> {
    let mut quiet = false;
    let mut scripts: Vec<&str> = Vec::new();
    let mut files: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(&arg) = iter.next() {
        if arg == "--" {
            files.extend(iter.by_ref().copied());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some(("expression", script)) => scripts.push(script),
                None if long == "expression" => scripts.push(iter.next()?),
                None if matches!(long, "quiet" | "silent") => quiet = true,
                None if matches!(
                    long,
                    "regexp-extended"
                        | "posix"
                        | "null-data"
                        | "separate"
                        | "unbuffered"
                        | "sandbox"
                        | "debug"
                ) => {}
                _ => return None,
            }
        } else if let Some(bundle) = arg.strip_prefix('-')
            && !bundle.is_empty()
        {
            for (at, flag) in bundle.char_indices() {
                match flag {
                    'n' => quiet = true,
                    'E' | 'r' | 'z' | 's' | 'u' => {}
                    'e' => {
                        let rest = &bundle[at + 1..];
                        scripts.push(if rest.is_empty() { iter.next()? } else { rest });
                        break;
                    }
                    _ => return None,
                }
            }
        } else {
            files.push(arg);
        }
    }
    if !quiet {
        return None;
    }
    if scripts.is_empty() {
        if files.is_empty() {
            return None;
        }
        scripts.push(files.remove(0));
    }
    scripts
        .iter()
        .flat_map(|script| script.split([';', '\n']))
        .all(prints_lines)
        .then(|| reads(ReadKind::Read, files))
        .flatten()
}

/// Whether one `sed` command only prints: `p` behind nothing, a line number,
/// `$`, or a range of those (`1,80p`, `100,$p`).
fn prints_lines(command: &str) -> bool {
    let Some(address) = command.trim().strip_suffix('p') else {
        return false;
    };
    let line =
        |part: &str| part == "$" || (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    let address = address.trim();
    address.is_empty()
        || match address.split_once(',') {
            Some((from, to)) => line(from.trim()) && line(to.trim()),
            None => line(address),
        }
}

/// `jq`: its first operand is the filter, the rest are files. A filter that
/// reaches the environment (`env`, `$ENV`) is refused — the variables hold
/// keys.
fn jq(args: &[&str]) -> Option<Verdict> {
    let mut words: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(&arg) = iter.next() {
        match arg {
            "--" => {
                words.extend(iter.by_ref().copied());
                break;
            }
            // Two values: a name and what it is bound to.
            "--arg" | "--argjson" => {
                iter.next()?;
                iter.next()?;
            }
            "--indent" => {
                iter.next()?;
            }
            flag if flag.starts_with('-') && flag != "-" => {}
            operand => words.push(operand),
        }
    }
    let (filter, files) = words.split_first()?;
    let reaches_env = filter.contains("$ENV")
        || filter
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == "env");
    if reaches_env {
        return None;
    }
    reads(ReadKind::Read, files.to_vec())
}

/// `git`, for the subcommands that only look: no global option but
/// `--no-pager`, and per subcommand the shapes that read rather than create,
/// delete, configure or contact a remote.
fn git(args: &[&str]) -> Option<Verdict> {
    let mut iter = args
        .iter()
        .copied()
        .skip_while(|arg| matches!(*arg, "--no-pager" | "-P"));
    let subcommand = iter.next()?;
    let rest: Vec<&str> = iter.collect();
    // A short flag matches exactly; a long one by any prefix git would take
    // for it (`--del` is `--delete`, [`abbreviates`]).
    let has = |flags: &[&str]| {
        rest.iter().any(|arg| {
            flags.iter().any(|flag| {
                if flag.starts_with("--") {
                    abbreviates(arg, flag)
                } else {
                    arg == flag
                }
            })
        })
    };
    let positionals = || rest.iter().filter(|arg| !arg.starts_with('-')).count();
    let quiet = Some(Verdict::Quiet);
    match subcommand {
        "status" | "blame" | "shortlog" | "rev-parse" | "rev-list" | "describe" | "ls-files"
        | "ls-tree" | "cat-file" | "merge-base" | "for-each-ref" | "show-ref" | "count-objects"
        | "name-rev" | "check-ignore" | "version" => quiet,
        // The history readers, minus the flags that write a file or run a
        // configured program.
        "log" | "show" | "diff" | "whatchanged" => {
            let writes = forbidden(
                &rest,
                Forbid {
                    long: &["--output", "--ext-diff", "--textconv"],
                    short: &[],
                    words: &[],
                },
            );
            (!writes).then_some(Verdict::Quiet)
        }
        // `-O` opens every match in a pager program of its own.
        "grep" => {
            let runs = forbidden(
                &rest,
                Forbid {
                    long: &["--open-files-in-pager"],
                    short: &['O'],
                    words: &[],
                },
            );
            (!runs)
                .then(|| reads(ReadKind::Search, Vec::new()))
                .flatten()
        }
        // Listing only: a name creates, and these flags delete, move, copy
        // or rewire.
        "branch" => {
            let edits = has(&[
                "-d",
                "-D",
                "-m",
                "-M",
                "-c",
                "-C",
                "-f",
                "-u",
                "-t",
                "--delete",
                "--move",
                "--copy",
                "--force",
                "--set-upstream-to",
                "--unset-upstream",
                "--edit-description",
                "--track",
                "--no-track",
            ]);
            let listing = has(&["-l", "--list"]);
            (!edits && (listing || positionals() == 0)).then_some(Verdict::Quiet)
        }
        "tag" => {
            let edits = has(&[
                "-d",
                "-a",
                "-s",
                "-m",
                "-f",
                "-u",
                "-F",
                "-e",
                "--delete",
                "--annotate",
                "--sign",
                "--force",
                "--message",
                "--file",
                "--edit",
                "--create-reflog",
            ]);
            let listing = has(&["-l", "--list"]);
            (!edits && (listing || rest.is_empty())).then_some(Verdict::Quiet)
        }
        "remote" => rest
            .iter()
            .all(|arg| matches!(*arg, "-v" | "--verbose"))
            .then_some(Verdict::Quiet),
        "config" => {
            let reads_config = rest.iter().any(|arg| {
                matches!(
                    *arg,
                    "--get" | "--get-all" | "--get-regexp" | "--get-urlmatch" | "--list" | "-l"
                )
            });
            let writes_config = has(&[
                "--add",
                "--unset",
                "--unset-all",
                "--replace-all",
                "--rename-section",
                "--remove-section",
                "--edit",
                "-e",
            ]);
            (reads_config && !writes_config).then_some(Verdict::Quiet)
        }
        "stash" => matches!(rest.first(), Some(&("list" | "show"))).then_some(Verdict::Quiet),
        "worktree" => (rest.first() == Some(&"list")).then_some(Verdict::Quiet),
        "reflog" => rest
            .iter()
            .find(|arg| !arg.starts_with('-'))
            .is_none_or(|first| *first == "show")
            .then_some(Verdict::Quiet),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

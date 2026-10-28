//! Read-only command recognition — the pure core behind two decisions a
//! `bash` call's command settles before it runs (`docs/compact-tools.md`,
//! `docs/permissions.md` *Read-only commands*):
//!
//! - **what it looks like it does** ([`kind`]): a directory listing, a read, a
//!   search — the classification the compact tool display groups consecutive
//!   calls by (`Listing 1 directory…`, `Searched for 2 patterns, read 1
//!   file`); and
//! - **whether it provably changes nothing** ([`is_read_only`]): the verdict
//!   auto mode acts on without consulting its classifier.
//!
//! Both read one [`lex`] of the command — `bash -c`'s own grammar as far as a
//! verdict needs it: single and double quotes with their escapes, `#`
//! comments, the list and pipeline operators, redirections with their file
//! descriptors, and the expansions a word carries (`$VAR`, a glob, a brace),
//! which make its text here not what the program will see. The lexer is also
//! what [`crate::permission::command_segments`] splits a command with, so the
//! prefix rules and the read-only verdict can never disagree about where a
//! command ends: a `;` hidden behind a backslash-escaped quote, or inside a
//! comment, is a `;` to bash and to both of them.
//!
//! The two verdicts share the program tables and the per-program flag rules
//! and differ in one thing: the display classification tolerates a `$VAR` or
//! a glob (`cat $FILE` *looks like* a read, and the group it joins is a
//! description), while the permission verdict refuses one (`find . $FLAGS`
//! may be `find . -delete` by the time the shell is done, and the verdict is
//! a promise). Every program not in the tables, every wrapper, every
//! substitution and every redirection into a file is "not read-only" — the
//! classifier and the prompt are still there for all of it.

/// The dim note appended to a `bash` cell auto mode ran unasked because its
/// command is provably read-only ([`crate::permission::CLASSIFIER_ALLOWED_NOTE`]'s
/// sibling, `docs/permissions.md` *Read-only commands*).
pub const READ_ONLY_ALLOWED_NOTE: &str = "Allowed as a read-only command";

/// One word of a command, quotes removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Word {
    /// The word as bash hands it to the program — quotes and escapes
    /// resolved. An expansion ([`dynamic`](Self::dynamic)) is left as written.
    pub text: String,
    /// The word carries something bash resolves at run time — `$VAR`, `${…}`,
    /// `$'…'`, a glob character (`*`, `?`, `[`), a brace — so `text` is not
    /// what the program will see.
    pub dynamic: bool,
    /// Some part of the word was quoted or escaped — so a `2` before a `>` is
    /// a word, not a file descriptor.
    pub quoted: bool,
}

/// A redirection, with its operator and its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// The operator with its descriptor: `>`, `>>`, `2>`, `&>`, `<`, `<<`,
    /// `<<<`, `2>&1`, `>&-`…
    pub op: String,
    /// The target word — `None` for a descriptor duplication (`2>&1`) and for
    /// a process substitution, which flags the segment instead.
    pub target: Option<Word>,
}

/// One list/pipeline segment of a command: what runs between two of `|`,
/// `||`, `&&`, `&`, `;` and newline.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Segment {
    /// The segment's own text — between its separators, comment dropped,
    /// trimmed; quotes and escapes as written.
    pub raw: String,
    /// Its words in order, quotes removed; redirection operators and their
    /// targets are not words.
    pub words: Vec<Word>,
    /// Its redirections, in order.
    pub redirects: Vec<Redirect>,
    /// A command substitution runs inside it: `$(…)`, backticks, `<(…)`,
    /// `>(…)`.
    pub substitution: bool,
    /// A subshell or group: `(`, `)`, or a `{` / `}` standing as a word.
    pub grouping: bool,
    /// Its first word is a `NAME=value` assignment — an environment the
    /// program runs under (`PAGER=… git log`).
    pub assignment: bool,
}

/// Lex `command` into its segments the way `bash -c` reads it. `None` for an
/// unterminated quote — a command bash itself refuses.
#[must_use]
pub fn lex(command: &str) -> Option<Vec<Segment>> {
    let mut lexer = Lexer {
        chars: command.chars().collect(),
        at: 0,
    };
    let mut segments = Vec::new();
    let mut segment = Segment::default();
    let mut first_word = true;
    while let Some(c) = lexer.peek(0) {
        match c {
            ' ' | '\t' => {
                segment.raw.push(c);
                lexer.at += 1;
            }
            // A backslash-newline continues the line: whitespace to bash.
            '\\' if lexer.peek(1) == Some('\n') => {
                segment.raw.push(' ');
                lexer.at += 2;
            }
            '\n' | ';' => {
                lexer.at += 1;
                if c == ';' && lexer.peek(0) == Some(';') {
                    lexer.at += 1;
                }
                finish(&mut segments, &mut segment);
                first_word = true;
            }
            '|' => {
                lexer.at += 1;
                if matches!(lexer.peek(0), Some('|' | '&')) {
                    lexer.at += 1;
                }
                finish(&mut segments, &mut segment);
                first_word = true;
            }
            '&' if lexer.peek(1) == Some('>') => {
                lexer.at += 1;
                segment.raw.push('&');
                lexer.redirect(&mut segment, String::from("&"))?;
            }
            '&' => {
                lexer.at += 1;
                if lexer.peek(0) == Some('&') {
                    lexer.at += 1;
                }
                finish(&mut segments, &mut segment);
                first_word = true;
            }
            // Between words, `#` opens a comment to the end of the line.
            '#' => {
                while lexer.peek(0).is_some_and(|c| c != '\n') {
                    lexer.at += 1;
                }
            }
            '(' | ')' => {
                segment.grouping = true;
                segment.raw.push(c);
                lexer.at += 1;
            }
            '<' | '>' => lexer.redirect(&mut segment, String::new())?,
            _ => {
                let (word, assigns) = lexer.word(&mut segment)?;
                // A bare number straight before a redirect operator is its
                // descriptor (`2>`), not a word.
                if matches!(lexer.peek(0), Some('<' | '>'))
                    && !word.quoted
                    && !word.text.is_empty()
                    && word.text.chars().all(|c| c.is_ascii_digit())
                {
                    lexer.redirect(&mut segment, word.text)?;
                    continue;
                }
                if first_word && assigns {
                    segment.assignment = true;
                }
                if !word.quoted && matches!(word.text.as_str(), "{" | "}") {
                    segment.grouping = true;
                }
                segment.words.push(word);
                first_word = false;
            }
        }
    }
    finish(&mut segments, &mut segment);
    Some(segments)
}

fn finish(segments: &mut Vec<Segment>, segment: &mut Segment) {
    let mut done = std::mem::take(segment);
    done.raw = done.raw.trim().to_string();
    if !done.raw.is_empty() {
        segments.push(done);
    }
}

struct Lexer {
    chars: Vec<char>,
    at: usize,
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    /// Consume the next character into `raw` and return it.
    fn take(&mut self, raw: &mut String) -> Option<char> {
        let c = self.peek(0)?;
        self.at += 1;
        raw.push(c);
        Some(c)
    }

    /// Lex one word from the cursor, which sits on its first character.
    /// Returns it and whether it is a `NAME=value` assignment.
    fn word(&mut self, segment: &mut Segment) -> Option<(Word, bool)> {
        let mut word = Word::default();
        let mut assigns = false;
        while let Some(c) = self.peek(0) {
            match c {
                '\'' => {
                    word.quoted = true;
                    self.take(&mut segment.raw);
                    self.single_quoted(&mut word, segment)?;
                }
                '"' => {
                    word.quoted = true;
                    self.take(&mut segment.raw);
                    self.double_quoted(&mut word, segment)?;
                }
                '\\' => {
                    self.take(&mut segment.raw);
                    match self.peek(0) {
                        // A continuation ends the word like whitespace.
                        Some('\n') => {
                            self.at += 1;
                            segment.raw.push(' ');
                            break;
                        }
                        Some(_) => {
                            word.quoted = true;
                            let escaped = self.take(&mut segment.raw)?;
                            word.text.push(escaped);
                        }
                        None => word.text.push('\\'),
                    }
                }
                '`' => {
                    segment.substitution = true;
                    word.text.push(self.take(&mut segment.raw)?);
                }
                '$' => {
                    self.take(&mut segment.raw);
                    match self.peek(0) {
                        Some('(') => segment.substitution = true,
                        // `$'…'` is ANSI-C quoting: its escapes resolve at
                        // run time, so the word is dynamic.
                        Some('\'') => {
                            word.quoted = true;
                            self.take(&mut segment.raw);
                            self.single_quoted(&mut word, segment)?;
                        }
                        // `$"…"` is locale quoting: a double-quoted word.
                        Some('"') => continue,
                        _ => {}
                    }
                    word.dynamic = true;
                    word.text.push('$');
                }
                '*' | '?' | '[' | '{' => {
                    word.dynamic = true;
                    word.text.push(self.take(&mut segment.raw)?);
                }
                '=' if !assigns && !word.quoted && is_name(&word.text) => {
                    assigns = true;
                    word.text.push(self.take(&mut segment.raw)?);
                }
                c if is_word_end(c) => break,
                _ => word.text.push(self.take(&mut segment.raw)?),
            }
        }
        // `{` alone opens a group, not a brace expansion; `[` alone is
        // `test`, not a glob.
        if !word.quoted && matches!(word.text.as_str(), "{" | "}" | "[") {
            word.dynamic = false;
        }
        Some((word, assigns))
    }

    /// The inside of `'…'`, the opening quote consumed: nothing escapes.
    fn single_quoted(&mut self, word: &mut Word, segment: &mut Segment) -> Option<()> {
        loop {
            let c = self.take(&mut segment.raw)?;
            if c == '\'' {
                return Some(());
            }
            word.text.push(c);
        }
    }

    /// The inside of `"…"`, the opening quote consumed: `\` escapes only `"`,
    /// `\`, `$`, a backtick and a newline, and expansions stay live.
    fn double_quoted(&mut self, word: &mut Word, segment: &mut Segment) -> Option<()> {
        loop {
            let c = self.take(&mut segment.raw)?;
            match c {
                '"' => return Some(()),
                '\\' => match self.peek(0) {
                    Some(escaped @ ('"' | '\\' | '$' | '`')) => {
                        self.take(&mut segment.raw);
                        word.text.push(escaped);
                    }
                    Some('\n') => {
                        self.take(&mut segment.raw);
                    }
                    _ => word.text.push('\\'),
                },
                '`' => {
                    segment.substitution = true;
                    word.text.push(c);
                }
                '$' => {
                    if self.peek(0) == Some('(') {
                        segment.substitution = true;
                    }
                    word.dynamic = true;
                    word.text.push(c);
                }
                _ => word.text.push(c),
            }
        }
    }

    /// A redirection at the cursor (on its `<` or `>`), `op` holding what came
    /// before the operator: a descriptor, or `&` for `&>`.
    fn redirect(&mut self, segment: &mut Segment, mut op: String) -> Option<()> {
        let c = self.take(&mut segment.raw)?;
        op.push(c);
        match (c, self.peek(0)) {
            ('>', Some('>' | '|')) | ('<', Some('>')) => {
                op.push(self.take(&mut segment.raw)?);
            }
            ('<', Some('<')) => {
                op.push(self.take(&mut segment.raw)?);
                if self.peek(0) == Some('<') {
                    op.push(self.take(&mut segment.raw)?);
                }
            }
            _ => {}
        }
        // A descriptor duplication — `2>&1`, `>&-` — names no file.
        if self.peek(0) == Some('&') {
            op.push(self.take(&mut segment.raw)?);
            let mut dup = String::new();
            while let Some(d) = self.peek(0) {
                if d.is_ascii_digit() || (d == '-' && dup.is_empty()) {
                    dup.push(self.take(&mut segment.raw)?);
                } else {
                    break;
                }
            }
            if !dup.is_empty() {
                op.push_str(&dup);
                segment.redirects.push(Redirect { op, target: None });
                return Some(());
            }
        }
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.take(&mut segment.raw);
        }
        // A process substitution (`<(…)`) runs a command; the subshell it
        // opens is lexed on as grouping.
        if self.peek(0) == Some('(') {
            segment.substitution = true;
            segment.redirects.push(Redirect { op, target: None });
            return Some(());
        }
        let target = match self.peek(0) {
            Some(c) if !is_word_end(c) => Some(self.word(segment)?.0),
            _ => None,
        };
        segment.redirects.push(Redirect { op, target });
        Some(())
    }
}

/// What ends a word outside quotes.
fn is_word_end(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')'
    )
}

/// A shell variable name — what may stand before the `=` of an assignment.
fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What a read-only command does, by the programs it runs — the counts the
/// compact tool display sums ([`kind`], `docs/compact-tools.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Kinds {
    /// A pattern is searched for: `grep`, `rg`, `find`, `which`…
    pub search: bool,
    /// A file is read: `cat`, `head`, `wc`, `jq`…
    pub read: bool,
    /// A directory is listed: `ls`, `tree`, `du`.
    pub list: bool,
}

impl Kinds {
    const NONE: Self = Self {
        search: false,
        read: false,
        list: false,
    };

    /// Nothing counted — every program was neutral (`echo`, `cd`, `pwd`…).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.search && !self.read && !self.list
    }
}

/// How a program counts in the compact display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Search,
    Read,
    List,
    /// Position-neutral: it counts nothing and breaks nothing — `ls a && echo
    /// --- && ls b` is still two listings.
    Neutral,
    /// Read-only, but shown as the command it is rather than folded into a
    /// group (`git status`, `node --version`).
    Plain,
}

/// What a program must satisfy, beyond the structural rules every read-only
/// command meets, for its command to be *provably* read-only.
#[derive(Debug, Clone, Copy)]
enum Safety {
    /// Its only effect is its output, whatever its flags and operands.
    Always,
    /// Never vouched for — a pager that takes keys, an interpreter.
    Never,
    /// Read-only unless one of these flags appears (a prefix match for the
    /// `--flag=value` and glued forms, checked on every word, `--` or not).
    Unless(&'static [&'static str]),
    /// A rule of its own over the words after the program.
    Rule(fn(&[Word]) -> bool),
}

/// The tables: what each known program is, and when it is read-only.
fn program(name: &str) -> Option<(Kind, Safety)> {
    use Kind::{List, Neutral, Plain, Read, Search};
    use Safety::{Always, Never, Rule, Unless};
    Some(match name {
        // Searching.
        "grep" | "egrep" | "fgrep" | "locate" | "which" | "whereis" | "pgrep" => (Search, Always),
        "find" => (
            Search,
            Unless(&[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf",
                "-fls",
            ]),
        ),
        "rg" => (Search, Unless(&["--pre", "--hostname-bin"])),
        "ag" | "ack" => (Search, Unless(&["--pager"])),
        "fd" | "fdfind" => (
            Search,
            Unless(&["-x", "--exec", "-X", "--exec-batch", "-l", "--list-details"]),
        ),
        // Reading.
        "cat" | "head" | "tail" | "wc" | "stat" | "strings" | "cut" | "tr" | "nl" | "tac"
        | "rev" | "od" | "hexdump" | "base64" | "column" | "fold" | "paste" | "comm" | "cmp"
        | "diff" | "sha1sum" | "sha256sum" | "md5sum" | "cksum" | "readlink" | "realpath" => {
            (Read, Always)
        }
        "file" => (Read, Unless(&["-C", "--compile"])),
        "sort" => (Read, Rule(sort_is_read_only)),
        "uniq" => (Read, Rule(uniq_is_read_only)),
        "jq" => (Read, Rule(jq_is_read_only)),
        // Pagers take keys; awk runs `system()`. Reads to look at, never to
        // vouch for.
        "less" | "more" | "awk" | "gawk" | "mawk" | "nawk" => (Read, Never),
        // Listing.
        "ls" | "du" | "exa" | "eza" | "lsd" => (List, Always),
        "tree" => (List, Unless(&["-o", "--output", "-R"])),
        // Neutral: output and status, a `cd` for the command's own shell.
        "echo" | "printf" | "true" | "false" | ":" | "pwd" | "test" | "[" | "whoami" | "id"
        | "uname" | "nproc" | "uptime" | "free" | "df" | "basename" | "dirname" | "seq"
        | "expr" | "type" | "getconf" | "locale" | "groups" | "cal" | "tsort" | "expand"
        | "unexpand" | "fmt" | "pr" | "numfmt" => (Neutral, Always),
        "cd" => (Neutral, Rule(|args| args.len() <= 1)),
        "hostname" => (
            Neutral,
            Rule(|args| args.iter().all(|a| a.text.starts_with('-'))),
        ),
        "date" => (
            Neutral,
            Rule(|args| {
                args.iter().all(|a| {
                    (a.text.starts_with('-')
                        && !a.text.starts_with("-s")
                        && !a.text.starts_with("--set"))
                        || a.text.starts_with('+')
                })
            }),
        ),
        // `command -v x` only answers where `x` is; `command x` runs it.
        "command" => (
            Neutral,
            Rule(|args| matches!(args.first().map(|a| a.text.as_str()), Some("-v" | "-V"))),
        ),
        // Plain: read-only, shown as what it is.
        "ps" => (
            Plain,
            Rule(|args| {
                args.iter()
                    .all(|a| a.text.starts_with('-') || !a.text.contains('e'))
            }),
        ),
        "git" => (Plain, Rule(git_is_read_only)),
        "node" | "npm" | "npx" | "pnpm" | "yarn" | "bun" | "deno" | "python" | "python3"
        | "pip" | "pip3" | "uv" | "cargo" | "rustc" | "rustup" | "rustfmt" | "gcc" | "g++"
        | "clang" | "make" | "cmake" | "ruby" | "gem" | "perl" | "php" | "dotnet" | "gh"
        | "docker" | "kubectl" | "terraform" | "tsc" => (
            Plain,
            Rule(|args| args.len() == 1 && args[0].text == "--version"),
        ),
        "go" => (
            Plain,
            Rule(|args| args.len() == 1 && args[0].text == "version"),
        ),
        "java" | "javac" => (
            Plain,
            Rule(|args| args.len() == 1 && args[0].text == "-version"),
        ),
        _ => return None,
    })
}

/// `sort` writes with `-o`/`--output` and runs `--compress-program`.
fn sort_is_read_only(args: &[Word]) -> bool {
    args.iter().all(|a| {
        let t = a.text.as_str();
        !(t.starts_with("--output")
            || t.starts_with("--compress-program")
            || (t.starts_with('-') && !t.starts_with("--") && t.contains('o')))
    })
}

/// `uniq input output` writes its second operand.
fn uniq_is_read_only(args: &[Word]) -> bool {
    args.iter().filter(|a| !a.text.starts_with('-')).count() < 2
}

/// `jq` reads programs and data from files it is pointed at, and its filter
/// can read the environment.
fn jq_is_read_only(args: &[Word]) -> bool {
    args.iter().all(|a| {
        let t = a.text.as_str();
        !(matches!(
            t,
            "-f" | "--from-file"
                | "--rawfile"
                | "--slurpfile"
                | "-L"
                | "--library-path"
                | "--run-tests"
        ) || t.contains("env"))
    })
}

/// The git subcommands that only read the repository, each with what would
/// make it write — Claude Code's own read-only git table, trimmed to what a
/// denylist can state.
fn git_is_read_only(args: &[Word]) -> bool {
    let mut args = args.iter();
    // Global options: `-C dir` and the pager switches are fine; `-c key=val`
    // sets configuration that can name a program to run (`core.fsmonitor`),
    // and `--exec-path` names where the subcommands come from.
    let subcommand = loop {
        let Some(arg) = args.next() else {
            return false;
        };
        match arg.text.as_str() {
            "-C" => {
                args.next();
            }
            "--no-pager" | "-P" | "--paginate" | "-p" | "--no-optional-locks" => {}
            t if t.starts_with("--git-dir=") || t.starts_with("--work-tree=") => {}
            t if t.starts_with('-') => return false,
            t => break t,
        }
    };
    let rest: Vec<&Word> = args.collect();
    let any = |flags: &[&str]| {
        rest.iter().any(|a| {
            flags
                .iter()
                .any(|f| a.text == *f || a.text.starts_with(&format!("{f}=")))
        })
    };
    let positionals = || rest.iter().filter(|a| !a.text.starts_with('-')).count();
    match subcommand {
        "status" | "blame" | "ls-files" | "ls-tree" | "cat-file" | "merge-base" | "rev-parse"
        | "describe" | "for-each-ref" | "grep" | "check-ignore" | "name-rev" | "count-objects"
        | "diff-tree" | "shortlog" | "rev-list" => !any(&["--batch"]),
        "log" | "diff" | "show" => !any(&["--output", "-o", "--ext-diff"]),
        "reflog" => !rest
            .first()
            .is_some_and(|a| matches!(a.text.as_str(), "expire" | "delete" | "exists")),
        "stash" => matches!(rest.first().map(|a| a.text.as_str()), Some("list" | "show")),
        "worktree" => rest.first().is_some_and(|a| a.text == "list"),
        "submodule" => rest.first().is_some_and(|a| a.text == "status"),
        "remote" => {
            positionals() == 0
                || rest
                    .first()
                    .is_some_and(|a| matches!(a.text.as_str(), "show" | "get-url"))
        }
        "config" => rest.iter().any(|a| {
            matches!(
                a.text.as_str(),
                "--get" | "--get-all" | "--get-regexp" | "-l" | "--list"
            )
        }),
        // Without a list flag, a positional creates.
        "tag" => {
            rest.iter().any(|a| {
                matches!(a.text.as_str(), "-l" | "--list")
                    || (a.text.starts_with('-')
                        && !a.text.starts_with("--")
                        && a.text.contains('l'))
            }) || positionals() == 0
        }
        "branch" => {
            !any(&[
                "-d",
                "-D",
                "-m",
                "-M",
                "-c",
                "-C",
                "--delete",
                "--move",
                "--copy",
                "-u",
                "--set-upstream-to",
                "--unset-upstream",
                "--edit-description",
                "-f",
                "--force",
                "--track",
                "--no-track",
            ]) && (positionals() == 0
                || rest.iter().any(|a| {
                    matches!(
                        a.text.as_str(),
                        "-l" | "--list"
                            | "--contains"
                            | "--no-contains"
                            | "--merged"
                            | "--no-merged"
                            | "--points-at"
                    )
                }))
        }
        _ => false,
    }
}

/// The program a segment names — its first word, unless that word is an
/// expansion (`$CMD`) or a path (`./script.sh`, `/usr/bin/ls`): a program this
/// table can know is named by its bare name.
fn program_of(segment: &Segment) -> Option<&Word> {
    let first = segment.words.first()?;
    (!first.dynamic && !first.text.contains('/')).then_some(first)
}

/// The redirections a read-only command may carry: input (`<`, `<<<`, a
/// heredoc), a descriptor duplication (`2>&1`), and output to `/dev/null`.
fn redirect_is_harmless(redirect: &Redirect) -> bool {
    let op = redirect.op.as_str();
    let input = op
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with('<')
        && !op.contains("<>");
    let duplication = op.contains('&') && !op.starts_with('&') && redirect.target.is_none();
    let to_null = redirect
        .target
        .as_ref()
        .is_some_and(|t| !t.dynamic && t.text == "/dev/null");
    input || duplication || to_null
}

/// How a segment's flags came out against its program's rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flags {
    /// None of the program's writing or executing flags.
    Ok,
    /// One of them is there (`find -delete`, `sort -o`).
    Banned,
    /// The program is never vouched for (a pager, awk).
    NeverVouched,
}

/// One segment judged: what its program is and what stands between it and
/// a read-only verdict. `None` for no program, or one the tables don't know.
struct Judgement {
    kind: Kind,
    /// No substitution, group or environment, and every redirection harmless.
    structural: bool,
    flags: Flags,
}

fn judge(segment: &Segment) -> Option<Judgement> {
    let named = program_of(segment)?;
    let (kind, safety) = program(&named.text)?;
    let args = &segment.words[1..];
    let structural = !segment.substitution
        && !segment.grouping
        && !segment.assignment
        && segment.redirects.iter().all(redirect_is_harmless);
    let flags = match safety {
        Safety::Always => Flags::Ok,
        Safety::Never => Flags::NeverVouched,
        Safety::Unless(banned) => {
            if args.iter().any(|a| {
                banned
                    .iter()
                    .any(|b| a.text == *b || a.text.starts_with(&format!("{b}=")))
            }) {
                Flags::Banned
            } else {
                Flags::Ok
            }
        }
        Safety::Rule(rule) => {
            if rule(args) {
                Flags::Ok
            } else {
                Flags::Banned
            }
        }
    };
    Some(Judgement {
        kind,
        structural,
        flags,
    })
}

/// What `command` looks like it does, for the compact tool display: `Some`
/// with the kinds it counts when every segment names a listing, reading or
/// searching program (or a neutral one) with none of its writing flags,
/// nothing is redirected into a file and nothing substituted — `None` for a
/// command that should stay a cell of its own. Lenient about expansions:
/// `cat $FILE` is a read, and a pager or awk is a read to look at.
#[must_use]
pub fn kind(command: &str) -> Option<Kinds> {
    let segments = lex(command)?;
    let mut kinds = Kinds::NONE;
    for segment in &segments {
        let judged = judge(segment)?;
        if !judged.structural || judged.flags == Flags::Banned {
            return None;
        }
        match judged.kind {
            Kind::Search => kinds.search = true,
            Kind::Read => kinds.read = true,
            Kind::List => kinds.list = true,
            Kind::Neutral => {}
            Kind::Plain => return None,
        }
    }
    (!kinds.is_empty()).then_some(kinds)
}

/// Is `command` provably read-only — every segment a known inspecting program
/// with none of its writing or executing flags, no expansion that could
/// become one, nothing substituted, no environment set, and no redirection
/// but input, a descriptor duplication or `/dev/null`?
#[must_use]
pub fn is_read_only(command: &str) -> bool {
    let Some(segments) = lex(command) else {
        return false;
    };
    !segments.is_empty()
        && segments.iter().all(|segment| {
            let dynamic = segment.words.iter().any(|w| w.dynamic)
                || segment
                    .redirects
                    .iter()
                    .any(|r| r.target.as_ref().is_some_and(|t| t.dynamic));
            !dynamic
                && judge(segment)
                    .is_some_and(|judged| judged.structural && judged.flags == Flags::Ok)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(segment: &Segment) -> Vec<&str> {
        segment.words.iter().map(|w| w.text.as_str()).collect()
    }

    fn raws(command: &str) -> Vec<String> {
        lex(command)
            .expect("lexes")
            .into_iter()
            .map(|s| s.raw)
            .collect()
    }

    // --- the lexer ---

    #[test]
    fn operators_split_segments_and_quotes_keep_them_whole() {
        assert_eq!(
            raws(r#"echo "a | b" && ls -la; pwd"#),
            vec![r#"echo "a | b""#, "ls -la", "pwd"]
        );
        assert_eq!(raws("a | b || c & d\ne"), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(raws("ls |& head"), vec!["ls", "head"]);
        assert_eq!(raws("  "), Vec::<String>::new());
    }

    #[test]
    fn words_lose_their_quotes_and_resolve_their_escapes() {
        let segments = lex(r#"grep -n 'a b' "c\"d" e\ f"#).expect("lexes");
        assert_eq!(
            words(&segments[0]),
            vec!["grep", "-n", "a b", "c\"d", "e f"]
        );
        assert!(segments[0].words[2].quoted);
        assert!(!segments[0].words[1].quoted);
    }

    #[test]
    fn an_escaped_quote_opens_nothing() {
        // The bug the lexer exists for: `\"` is a literal character to bash,
        // so the `;` after it is real and `rm -rf ~` runs.
        let segments = lex(r#"echo \"; rm -rf ~; echo \""#).expect("lexes");
        assert_eq!(segments.len(), 3, "{segments:#?}");
        assert_eq!(words(&segments[1]), vec!["rm", "-rf", "~"]);
        assert!(
            lex(r#"echo \' $(curl x|sh) \'"#)
                .expect("lexes")
                .iter()
                .any(|s| s.substitution),
            "a substitution behind an escaped quote is still a substitution"
        );
    }

    #[test]
    fn a_comment_hides_nothing() {
        let segments = lex("echo #'\nrm -rf ~\n#'").expect("lexes");
        assert_eq!(segments.len(), 2, "{segments:#?}");
        assert_eq!(words(&segments[1]), vec!["rm", "-rf", "~"]);
        // Only where a word would start: inside a word it is a character.
        let segments = lex("echo a#b # trailing").expect("lexes");
        assert_eq!(words(&segments[0]), vec!["echo", "a#b"]);
        assert_eq!(segments[0].raw, "echo a#b");
    }

    #[test]
    fn an_unterminated_quote_is_no_command() {
        assert_eq!(lex("ls \""), None);
        assert_eq!(lex("ls '"), None);
    }

    #[test]
    fn redirections_carry_their_descriptor_and_their_target() {
        let segments = lex("find / -name x 2>/dev/null >out.txt 2>&1 < in").expect("lexes");
        let ops: Vec<(&str, Option<&str>)> = segments[0]
            .redirects
            .iter()
            .map(|r| (r.op.as_str(), r.target.as_ref().map(|t| t.text.as_str())))
            .collect();
        assert_eq!(
            ops,
            vec![
                ("2>", Some("/dev/null")),
                (">", Some("out.txt")),
                ("2>&1", None),
                ("<", Some("in"))
            ]
        );
        assert_eq!(words(&segments[0]), vec!["find", "/", "-name", "x"]);
        assert_eq!(segments.len(), 1, "`2>&1` is no background `&`");
        let both = lex("ls &> log").expect("lexes");
        assert_eq!(both[0].redirects[0].op, "&>");
        assert_eq!(
            both[0].redirects[0]
                .target
                .as_ref()
                .map(|t| t.text.as_str()),
            Some("log")
        );
        let here = lex("grep x <<< \"$y\"").expect("lexes");
        assert_eq!(here[0].redirects[0].op, "<<<");
        assert!(
            here[0].redirects[0]
                .target
                .as_ref()
                .is_some_and(|t| t.dynamic)
        );
    }

    #[test]
    fn expansions_mark_a_word_dynamic_and_substitutions_the_segment() {
        let segments = lex("cat $FILE *.rs ?.md '[x]' \"$HOME\" {a,b} ~/x").expect("lexes");
        let dynamic: Vec<bool> = segments[0].words.iter().map(|w| w.dynamic).collect();
        assert_eq!(
            dynamic,
            vec![false, true, true, true, false, true, true, false]
        );
        assert!(!segments[0].substitution);
        for command in [
            "echo $(whoami)",
            "echo `whoami`",
            "echo \"$(whoami)\"",
            "diff <(ls a) <(ls b)",
        ] {
            assert!(
                lex(command).expect("lexes").iter().any(|s| s.substitution),
                "{command}"
            );
        }
        assert!(
            !lex("echo '$(whoami)'").expect("lexes")[0].substitution,
            "single quotes defuse a substitution"
        );
    }

    #[test]
    fn an_assignment_a_group_and_a_subshell_are_flagged() {
        assert!(lex("PAGER=evil git log").expect("lexes")[0].assignment);
        assert!(!lex("echo a=b").expect("lexes")[0].assignment);
        assert!(lex("(ls)").expect("lexes")[0].grouping);
        assert!(lex("{ ls; }").expect("lexes")[0].grouping);
        assert!(!lex("cat {a,b}").expect("lexes")[0].grouping);
    }

    // --- the display classification ---

    #[test]
    fn a_listing_a_read_and_a_search_are_told_apart() {
        assert_eq!(
            kind("ls -la"),
            Some(Kinds {
                list: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("cat src/main.rs"),
            Some(Kinds {
                read: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("grep -rn foo ."),
            Some(Kinds {
                search: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("rg 'fn main' src"),
            Some(Kinds {
                search: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("find . -name '*.rs'"),
            Some(Kinds {
                search: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("tree -L 2"),
            Some(Kinds {
                list: true,
                ..Kinds::NONE
            })
        );
    }

    #[test]
    fn a_pipeline_counts_every_kind_it_runs() {
        assert_eq!(
            kind("cat f | grep x | head"),
            Some(Kinds {
                search: true,
                read: true,
                list: false
            })
        );
        assert_eq!(
            kind("ls && echo --- && ls other"),
            Some(Kinds {
                list: true,
                ..Kinds::NONE
            }),
            "echo is neutral"
        );
        assert_eq!(
            kind("cd src && ls"),
            Some(Kinds {
                list: true,
                ..Kinds::NONE
            })
        );
    }

    #[test]
    fn a_command_that_is_not_an_inspection_stays_a_cell_of_its_own() {
        for command in [
            "echo hello",
            "pwd",
            "cargo test",
            "git status",
            "rm -rf x",
            "ls > out.txt",
            "cat f | tee g",
            "find . -delete",
            "find . -name x -exec rm {} \\;",
            "sort -o out in",
            "ls $(evil)",
            "sudo ls",
            "xargs cat",
            "ls \"",
            "",
        ] {
            assert_eq!(kind(command), None, "{command}");
        }
    }

    #[test]
    fn the_display_tolerates_an_expansion_and_a_null_redirect() {
        assert_eq!(
            kind("cat $FILE"),
            Some(Kinds {
                read: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("ls *.rs"),
            Some(Kinds {
                list: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("find / -name x 2>/dev/null"),
            Some(Kinds {
                search: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("awk '{print $1}' f"),
            Some(Kinds {
                read: true,
                ..Kinds::NONE
            })
        );
        assert_eq!(
            kind("less f"),
            Some(Kinds {
                read: true,
                ..Kinds::NONE
            })
        );
    }

    // --- the permission verdict ---

    #[test]
    fn plain_inspections_are_read_only() {
        for command in [
            "ls -la",
            "ls",
            "cat Cargo.toml",
            "head -n 20 src/main.rs",
            "grep -rn 'fn main' src/",
            "rg --no-heading foo",
            "find . -name '*.rs' -type f",
            "wc -l README.md",
            "tree -L 2 src",
            "du -sh .",
            "pwd",
            "echo hello world",
            "git status --short",
            "git log --oneline -n 20",
            "git diff HEAD~1 -- src",
            "git branch -a",
            "git -C /tmp/x status",
            "cat f | grep x | sort | uniq -c | head",
            "ls 2>/dev/null",
            "find / -name x 2>&1 | head",
            "cd src && ls",
            "test -f x",
            "node --version",
            "go version",
            "ps aux",
            "which cargo",
            "jq .name package.json",
            "sort -r f",
            "date +%s",
            "echo 'a; b'",
        ] {
            assert!(is_read_only(command), "{command}");
        }
    }

    #[test]
    fn anything_that_writes_runs_or_expands_is_not() {
        for command in [
            "",
            "   ",
            "rm -rf x",
            "ls > out.txt",
            "ls >> out.txt",
            "ls &> out.txt",
            "cat f | tee g",
            "find . -delete",
            "find . -name x -exec rm {} \\;",
            "find . -name x -execdir sh -c x \\;",
            "find . '-delete'",
            "find . -de\\lete",
            "sort -o out in",
            "sort -ro out in",
            "sort --output=out in",
            "uniq in out",
            "tree -o out",
            "rg --pre cat x",
            "fd -x rm",
            "jq -f prog.jq data",
            "jq 'env' data",
            "cat $FILE",
            "ls *.rs",
            "find . $FLAGS",
            "find . -name *.rs",
            "ls $(evil)",
            "echo `evil`",
            "cat <(evil)",
            "(ls)",
            "{ ls; }",
            "PAGER=evil git log",
            "sudo ls",
            "env ls",
            "sh -c ls",
            "xargs cat",
            "time ls",
            "ls; rm x",
            "ls && rm x",
            "awk 'BEGIN{system(\"x\")}'",
            "less f",
            "sed -n 1p f",
            "git push",
            "git branch -D x",
            "git branch new",
            "git tag v1",
            "git -c core.fsmonitor=evil status",
            "git log --output=f",
            "git stash",
            "git remote add x y",
            "git config x y",
            "git reflog expire",
            "cargo test",
            "node -e 'x'",
            "./script.sh --version",
            "/usr/bin/ls",
            "date -s now",
            "date 1231",
            "ps e",
            "cd a b",
            "command rm x",
            "ls \"",
            "cat <<EOF\nhi\nEOF",
            "echo \\\"; rm -rf ~; echo \\\"",
            "echo #'\nrm -rf ~\n#'",
        ] {
            assert!(!is_read_only(command), "{command}");
        }
    }

    #[test]
    fn git_reads_with_their_own_flags() {
        assert!(is_read_only("git tag -l 'v*'"));
        assert!(is_read_only("git tag"));
        assert!(is_read_only("git branch --merged main"));
        assert!(is_read_only("git remote -v"));
        assert!(is_read_only("git remote show origin"));
        assert!(is_read_only("git config --get user.name"));
        assert!(is_read_only("git stash list"));
        assert!(is_read_only("git worktree list"));
        assert!(is_read_only("git show HEAD --stat"));
        assert!(is_read_only("git --no-pager log -3"));
        assert!(!is_read_only("git worktree add x"));
        assert!(!is_read_only("git config --get-regexp x; git push"));
    }

    #[test]
    fn the_note_names_the_reason() {
        assert_eq!(READ_ONLY_ALLOWED_NOTE, "Allowed as a read-only command");
    }
}

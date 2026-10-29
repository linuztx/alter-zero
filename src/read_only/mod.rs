//! Recognising the `bash` commands that only **read** — two questions with two
//! answers, kept apart on purpose (`docs/tool-groups.md`):
//!
//! - [`read_kind`] — *what* a command reads: a search, a file read or a
//!   directory listing. What the collapsed tool cell counts ("Searched for 1
//!   pattern, read 2 files"), so it is cheap and generous: the program words
//!   decide, a glob or a variable is fine, and only what could hide a **write**
//!   from the reader (a redirect to a file, a substitution, a writing flag
//!   such as `find -delete` or `sort -o`) makes it no read at all — a collapsed
//!   line must never summarise a write as a read.
//! - [`is_read_only`] — whether a command **provably** only reads, so auto
//!   mode may run it without consulting the classifier
//!   (`docs/permissions.md`). Strict: every shell construct the words cannot
//!   vouch for refuses it (`lex::Hazards`), and every program must be on an
//!   allowlist with only the flags its table names (`flags`, `git`) —
//!   Claude Code's `checkReadOnlyConstraints`, ported. A miss costs a
//!   classifier round trip, never a write run unasked.
//!
//! Both are pure functions of the command text.

mod flags;
mod git;
mod lex;

use flags::Rules;
use lex::Segment;

/// What a command reads — the collapsed cell's three clauses, in the order
/// the reference names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadKind {
    /// A search: `grep`, `rg`, `find`, `fd`, `which`, …
    Search,
    /// A file read: `cat`, `head`, `tail`, `wc`, `jq`, `sed -n '1,40p'`, …
    Read,
    /// A directory listing: `ls`, `tree`, `du`.
    List,
}

/// Programs that list a directory.
const LIST_PROGRAMS: &[&str] = &["ls", "tree", "du", "eza", "exa"];

/// Programs that search.
const SEARCH_PROGRAMS: &[&str] = &[
    "find", "grep", "egrep", "fgrep", "rg", "ag", "ack", "locate", "which", "whereis", "fd",
    "fdfind",
];

/// Programs that read a file (or their stdin) and print what they find.
const READ_PROGRAMS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "wc", "stat", "file", "strings", "jq", "awk", "cut",
    "sort", "uniq", "tr", "nl", "bat", "sed",
];

/// Programs that neither read nor write anything worth a clause — they ride
/// along in a reading command without making one on their own. `cd` is here
/// (unlike in the reference) because `cd src && ls` lists as surely as `ls
/// src` does.
const NEUTRAL_PROGRAMS: &[&str] = &["echo", "printf", "true", "false", ":", "cd"];

/// What `command` reads, when reading is all it does — `None` for anything
/// that runs or changes something, and for a command of neutral words alone
/// (`echo hi`). A pipeline or list takes its strongest kind: a listing
/// wherever one appears, else a search, else a read (the reference's
/// precedence, so `ls | grep x` is a listing and `cat f | grep x` a search).
#[must_use]
pub fn read_kind(command: &str) -> Option<ReadKind> {
    let parsed = lex::parse(command)?;
    if parsed.hazards.hides_a_write() {
        return None;
    }
    let mut kinds = [false; 3];
    let mut reads = false;
    for segment in &parsed.segments {
        let program = segment.program()?;
        if NEUTRAL_PROGRAMS.contains(&program) {
            continue;
        }
        let kind = segment_read_kind(segment)?;
        reads = true;
        kinds[kind as usize] = true;
    }
    if !reads {
        return None;
    }
    let [search, read, list] = kinds;
    if list {
        Some(ReadKind::List)
    } else if search {
        Some(ReadKind::Search)
    } else if read {
        Some(ReadKind::Read)
    } else {
        None
    }
}

/// One simple command's kind, or `None` when it is no read — an unknown
/// program, or a reading program given a flag that writes.
fn segment_read_kind(segment: &Segment) -> Option<ReadKind> {
    let program = segment.program()?;
    let args = segment.args();
    // Any word giving one of these flags — as a short flag anywhere in a
    // bundle (`-aR`, `-xrm`), or as a long option however GNU getopt lets it
    // be shortened (`--out=` for `--output=`).
    let any = |short: &[char], long: &[&str]| {
        args.iter()
            .any(|word| bundles(word, short) || long.iter().any(|option| names_long(word, option)))
    };
    if LIST_PROGRAMS.contains(&program) {
        // `tree -o FILE` writes the listing; `-R` re-runs it in every
        // directory with `-o 00Tree.html`.
        let writes = program == "tree" && any(&['o', 'R'], &["--output"]);
        return (!writes).then_some(ReadKind::List);
    }
    if SEARCH_PROGRAMS.contains(&program) {
        let writes = match program {
            "find" => args.iter().any(|word| FIND_WRITES.contains(&word.as_str())),
            // A program run per match.
            "fd" | "fdfind" => any(&['x', 'X'], &["--exec", "--exec-batch"]),
            // A preprocessor, and a program asked for the hostname.
            "rg" => any(&[], &["--pre", "--hostname-bin"]),
            "ag" | "ack" => any(&[], &["--pager"]),
            _ => false,
        };
        return (!writes).then_some(ReadKind::Search);
    }
    if READ_PROGRAMS.contains(&program) {
        let writes = match program {
            "sort" => any(&['o'], &["--output", "--compress-program"]),
            // `-C` compiles a magic file into a `.mgc` beside it.
            "file" => any(&['C'], &["--compile"]),
            // A copy of what is shown, into a log file.
            "less" => any(&['o', 'O'], &["--log-file", "--LOG-FILE"]),
            // A pager of the caller's choosing; `bat cache` builds and clears
            // bat's own.
            "bat" => any(&[], &["--pager"]) || args.first().is_some_and(|word| word == "cache"),
            // `uniq INPUT OUTPUT` writes its second positional.
            "uniq" => uniq_positionals(args) > 1,
            "awk" => args.iter().any(|word| {
                ["system", "getline", ">", "|", "fflush", "close("]
                    .iter()
                    .any(|danger| word.contains(danger))
            }),
            "sed" => !flags::sed(args),
            _ => false,
        };
        return (!writes).then_some(ReadKind::Read);
    }
    None
}

/// Whether `word` is a bundle of short flags holding one of `letters` —
/// `-aR`, `-xrm` (an attached value reads as a bundle too, so `-exml` is
/// taken for an `x`: refusing a read costs a cell, never a hidden write).
fn bundles(word: &str, letters: &[char]) -> bool {
    word.len() > 1
        && word.starts_with('-')
        && !word.starts_with("--")
        && word[1..].chars().any(|letter| letters.contains(&letter))
}

/// Whether `word` gives the long option `option` — whole, with a value
/// (`--output=x`), or shortened the way GNU getopt allows (`--out=x`), which
/// a check of the full name alone walks past. An ambiguous prefix counts too:
/// the program refuses it, so nothing is lost.
fn names_long(word: &str, option: &str) -> bool {
    let name = word.split_once('=').map_or(word, |(name, _)| name);
    name.len() > 2 && name.starts_with("--") && option.starts_with(name)
}

/// `find` predicates that run a program, delete, or write a file.
const FIND_WRITES: &[&str] = &[
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf", "-fls",
];

/// How many of `uniq`'s words are files (the values of `-f`/`-s`/`-w`
/// excepted).
fn uniq_positionals(args: &[String]) -> usize {
    let mut count = 0;
    let mut skip = false;
    for word in args {
        if skip {
            skip = false;
        } else if matches!(word.as_str(), "-f" | "-s" | "-w") {
            skip = true;
        } else if !word.starts_with('-') || word == "-" {
            count += 1;
        }
    }
    count
}

/// The most simple commands one line may chain and still be judged.
const MAX_SEGMENTS: usize = 50;

/// Programs that are read-only with any words at all (Claude Code's
/// `makeRegexForSafeCommand` list): none of them has a flag that writes a
/// file or runs a program, and the shell constructs that could (a redirect,
/// a substitution) are refused before the words are read.
const ANY_ARGS: &[&str] = &[
    "cal", "uptime", "cat", "head", "tail", "wc", "stat", "strings", "hexdump", "od", "nl", "id",
    "uname", "free", "df", "du", "locale", "groups", "nproc", "basename", "dirname", "realpath",
    "cut", "paste", "tr", "column", "tac", "rev", "fold", "expand", "unexpand", "fmt", "comm",
    "cmp", "numfmt", "readlink", "diff", "true", "false", "sleep", "which", "type", "expr", "test",
    "getconf", "seq", "tsort", "pr", "ls", "echo",
];

/// Whether `command` provably only reads — every simple command an
/// allowlisted program with flags its table names, and no shell construct
/// the words cannot vouch for (a redirect to a file, a variable, a glob, a
/// substitution, a background job, a comment, a line break). The fast path
/// auto mode takes before its classifier (`docs/permissions.md`).
#[must_use]
pub fn is_read_only(command: &str) -> bool {
    let Some(parsed) = lex::parse(command) else {
        return false;
    };
    if parsed.hazards.any() || parsed.segments.is_empty() || parsed.segments.len() > MAX_SEGMENTS {
        return false;
    }
    // Another process's environment holds its secrets.
    if command.contains("/proc/") && command.contains("environ") {
        return false;
    }
    // A `cd` moves a `git` call into a repository the words do not name —
    // one whose config may run a program on any git command.
    let programs: Vec<&str> = parsed
        .segments
        .iter()
        .filter_map(Segment::program)
        .collect();
    if programs.contains(&"cd") && programs.contains(&"git") {
        return false;
    }
    parsed.segments.iter().all(segment_read_only)
}

/// One simple command's verdict.
fn segment_read_only(segment: &Segment) -> bool {
    let Some(program) = segment.program() else {
        return false;
    };
    let args = segment.args();
    let plain = Rules::default();
    match program {
        program if ANY_ARGS.contains(&program) => true,
        "pwd" | "whoami" => args.is_empty(),
        "cd" => args.len() <= 1,
        "node" => matches!(args, [only] if only == "-v" || only == "--version"),
        "python" | "python3" => matches!(args, [only] if only == "--version"),
        // `uniq INPUT OUTPUT` writes; read-only it filters its stdin.
        "uniq" => uniq_positionals(args) == 0,
        "find" => !args.iter().any(|word| FIND_WRITES.contains(&word.as_str())),
        "jq" => !args.iter().any(|word| {
            matches!(
                word.as_str(),
                "-f" | "--from-file"
                    | "--rawfile"
                    | "--slurpfile"
                    | "--run-tests"
                    | "-L"
                    | "--library-path"
            ) || word
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|token| token == "env")
                || word.contains("system(")
        }),
        "grep" | "egrep" | "fgrep" => flags::validate(
            args,
            flags::GREP,
            Rules {
                attached_numbers: flags::GREP_ATTACHED,
                ..plain
            },
        ),
        "rg" => flags::validate(
            args,
            flags::RG,
            Rules {
                attached_numbers: flags::GREP_ATTACHED,
                ..plain
            },
        ),
        "fd" | "fdfind" => flags::validate(args, flags::FD, plain),
        "tree" => flags::validate(args, flags::TREE, plain),
        "sort" => flags::validate(args, flags::SORT, plain),
        "file" => flags::validate(args, flags::FILE, plain),
        "sha256sum" | "sha1sum" | "md5sum" => flags::validate(args, flags::CHECKSUM, plain),
        // BSD `ps e` prints every process's environment.
        "ps" => {
            flags::validate(args, flags::PS, plain)
                && !args.iter().any(|word| {
                    !word.starts_with('-')
                        && word.chars().all(|c| c.is_ascii_alphabetic())
                        && word.contains('e')
                })
        }
        // A dashless word that is not `+FORMAT` sets the clock.
        "date" => flags::validate(args, flags::DATE, plain) && date_positionals_are_formats(args),
        "hostname" => {
            flags::validate(args, flags::HOSTNAME, plain)
                && args.iter().all(|word| word.starts_with('-'))
        }
        "base64" => flags::validate(
            args,
            flags::BASE64,
            Rules {
                double_dash_is_a_word: true,
                ..plain
            },
        ),
        "pgrep" => flags::validate(args, flags::PGREP, plain),
        "xargs" => flags::xargs(args),
        "sed" => flags::sed(args),
        "git" => git::read_only(args),
        _ => false,
    }
}

/// Every positional `date` word is a `+FORMAT` (the values of its
/// value-taking flags aside).
fn date_positionals_are_formats(args: &[String]) -> bool {
    let mut skip = false;
    for word in args {
        if skip {
            skip = false;
            continue;
        }
        if matches!(
            word.as_str(),
            "-d" | "--date" | "-r" | "--reference" | "--iso-8601" | "--rfc-3339"
        ) {
            skip = true;
            continue;
        }
        if !word.starts_with('-') && !word.starts_with('+') {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;

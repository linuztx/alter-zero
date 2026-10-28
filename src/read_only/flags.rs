//! The flag allowlists behind [`super::is_read_only`] — Claude Code's
//! `COMMAND_ALLOWLIST`, ported for the programs an agent inspects a project
//! with (`docs/tool-groups.md`). A program listed here is read-only only with
//! the flags its table names: an unknown flag, a value-taking flag with no
//! value, or a value-taking flag inside a bundle (`-rf patterns`) refuses the
//! command, so a writing flag nobody listed (`sort -o`, `tree -o`,
//! `rg --pre`) can never slip through as an unknown one.

/// What a flag takes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Arg {
    /// Nothing — a switch. Written with `=` it is refused.
    None,
    /// A whole number.
    Number,
    /// Any word that is not itself a flag.
    Str,
    /// One character.
    Char,
}

/// A flag table: groups of `(flag, what it takes)`, looked up in order.
pub(super) type Spec = &'static [&'static [(&'static str, Arg)]];

/// How a program's flags are read beyond its table.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Rules {
    /// `--` ends the flags (most programs); when `false` it is an ordinary
    /// word (`base64`).
    pub(super) double_dash_is_a_word: bool,
    /// Short flags that may carry their number attached — `grep -A20`,
    /// `git diff -U0`.
    pub(super) attached_numbers: &'static [char],
    /// `git log -5`: a dash and digits is `-n 5`.
    pub(super) dash_number: bool,
}

use Arg::{Char, None as Plain, Number, Str};

/// Look `flag` up in `spec`.
fn lookup(spec: Spec, flag: &str) -> Option<Arg> {
    spec.iter()
        .flat_map(|group| group.iter())
        .find(|(name, _)| *name == flag)
        .map(|(_, arg)| *arg)
}

/// Whether a value is acceptable for `arg`.
fn value_fits(arg: Arg, value: &str) -> bool {
    match arg {
        Plain => false,
        Number => !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
        // A value that is itself a flag is a flag the table never saw.
        Str => !value.starts_with('-'),
        Char => value.chars().count() == 1,
    }
}

/// Check every word of `args` against `spec` — Claude Code's
/// `validateFlags`, ported. Positional words are always accepted (what they
/// name is read, never run); every flag must be in the table with what it
/// takes.
pub(super) fn validate(args: &[String], spec: Spec, rules: Rules) -> bool {
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_str();
        if word == "--" && !rules.double_dash_is_a_word {
            // Everything after is a positional word.
            return true;
        }
        if let Some(long) = word.strip_prefix("--").filter(|rest| !rest.is_empty()) {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value)),
                None => (word.to_string(), None),
            };
            let Some(arg) = lookup(spec, &name) else {
                return false;
            };
            match (arg, value) {
                (Plain, None) => {}
                (Plain, Some(_)) => return false,
                (arg, Some(value)) => {
                    if !value_fits(arg, value) {
                        return false;
                    }
                }
                (arg, None) => {
                    i += 1;
                    match args.get(i) {
                        Some(value) if value_fits(arg, value) => {}
                        _ => return false,
                    }
                }
            }
        } else if word.len() > 1 && word.starts_with('-') {
            if let Some(arg) = lookup(spec, word) {
                if arg != Plain {
                    i += 1;
                    match args.get(i) {
                        Some(value) if value_fits(arg, value) => {}
                        _ => return false,
                    }
                }
            } else if !short_flag_fits(word, spec, rules) {
                return false;
            }
        }
        i += 1;
    }
    true
}

/// A short-flag word no table entry names whole: `-5` for `git log`, an
/// attached number (`-A20`), or a bundle of switches (`-rni`) — every one of
/// its letters a flag that takes nothing.
fn short_flag_fits(word: &str, spec: Spec, rules: Rules) -> bool {
    let rest = &word[1..];
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if rules.dash_number && all_digits(rest) {
        return true;
    }
    let mut chars = rest.chars();
    if let Some(first) = chars.next()
        && rules.attached_numbers.contains(&first)
        && all_digits(chars.as_str())
    {
        return true;
    }
    rest.chars()
        .all(|letter| lookup(spec, &format!("-{letter}")) == Some(Plain))
}

// ===== the tables =====

pub(super) const GREP: Spec = &[
    &[
        ("-e", Str),
        ("--regexp", Str),
        ("-f", Str),
        ("--file", Str),
        ("--color", Str),
        ("--colour", Str),
        ("--label", Str),
        ("--group-separator", Str),
        ("--binary-files", Str),
        ("-D", Str),
        ("--devices", Str),
        ("-d", Str),
        ("--directories", Str),
        ("--exclude", Str),
        ("--exclude-from", Str),
        ("--exclude-dir", Str),
        ("--include", Str),
    ],
    &[
        ("-m", Number),
        ("--max-count", Number),
        ("-A", Number),
        ("--after-context", Number),
        ("-B", Number),
        ("--before-context", Number),
        ("-C", Number),
        ("--context", Number),
    ],
    &[
        ("-F", Plain),
        ("--fixed-strings", Plain),
        ("-G", Plain),
        ("--basic-regexp", Plain),
        ("-E", Plain),
        ("--extended-regexp", Plain),
        ("-P", Plain),
        ("--perl-regexp", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
        ("--no-ignore-case", Plain),
        ("-v", Plain),
        ("--invert-match", Plain),
        ("-w", Plain),
        ("--word-regexp", Plain),
        ("-x", Plain),
        ("--line-regexp", Plain),
        ("-c", Plain),
        ("--count", Plain),
        ("-L", Plain),
        ("--files-without-match", Plain),
        ("-l", Plain),
        ("--files-with-matches", Plain),
        ("-o", Plain),
        ("--only-matching", Plain),
        ("-q", Plain),
        ("--quiet", Plain),
        ("--silent", Plain),
        ("-s", Plain),
        ("--no-messages", Plain),
        ("-b", Plain),
        ("--byte-offset", Plain),
        ("-H", Plain),
        ("--with-filename", Plain),
        ("-h", Plain),
        ("--no-filename", Plain),
        ("-n", Plain),
        ("--line-number", Plain),
        ("-T", Plain),
        ("--initial-tab", Plain),
        ("-u", Plain),
        ("-Z", Plain),
        ("--null", Plain),
        ("-z", Plain),
        ("--null-data", Plain),
        ("-a", Plain),
        ("--text", Plain),
        ("-r", Plain),
        ("--recursive", Plain),
        ("-R", Plain),
        ("--dereference-recursive", Plain),
        ("-U", Plain),
        ("--binary", Plain),
        ("--no-group-separator", Plain),
        ("--line-buffered", Plain),
        ("--help", Plain),
        ("-V", Plain),
        ("--version", Plain),
    ],
];

/// `grep`'s attached-number letters: `-A20`, `-m5`.
pub(super) const GREP_ATTACHED: &[char] = &['A', 'B', 'C', 'm'];

/// ripgrep. `--pre` runs a program on every file searched, so it is absent.
pub(super) const RG: Spec = &[
    &[
        ("-e", Str),
        ("--regexp", Str),
        ("-f", Str),
        ("--file", Str),
        ("-g", Str),
        ("--glob", Str),
        ("--iglob", Str),
        ("-t", Str),
        ("--type", Str),
        ("-T", Str),
        ("--type-not", Str),
        ("--color", Str),
        ("--sort", Str),
        ("--sortr", Str),
    ],
    &[
        ("-A", Number),
        ("--after-context", Number),
        ("-B", Number),
        ("--before-context", Number),
        ("-C", Number),
        ("--context", Number),
        ("-m", Number),
        ("--max-count", Number),
        ("-d", Number),
        ("--max-depth", Number),
        ("--maxdepth", Number),
    ],
    &[
        ("-i", Plain),
        ("--ignore-case", Plain),
        ("-S", Plain),
        ("--smart-case", Plain),
        ("-s", Plain),
        ("--case-sensitive", Plain),
        ("-F", Plain),
        ("--fixed-strings", Plain),
        ("-w", Plain),
        ("--word-regexp", Plain),
        ("-x", Plain),
        ("--line-regexp", Plain),
        ("-v", Plain),
        ("--invert-match", Plain),
        ("-c", Plain),
        ("--count", Plain),
        ("--count-matches", Plain),
        ("-l", Plain),
        ("--files-with-matches", Plain),
        ("--files-without-match", Plain),
        ("--files", Plain),
        ("-n", Plain),
        ("--line-number", Plain),
        ("-N", Plain),
        ("--no-line-number", Plain),
        ("-o", Plain),
        ("--only-matching", Plain),
        ("-H", Plain),
        ("--with-filename", Plain),
        ("-h", Plain),
        ("--no-filename", Plain),
        ("--heading", Plain),
        ("--no-heading", Plain),
        ("-q", Plain),
        ("--quiet", Plain),
        ("--column", Plain),
        ("--vimgrep", Plain),
        ("--trim", Plain),
        ("--type-list", Plain),
        ("--hidden", Plain),
        ("-.", Plain),
        ("--no-ignore", Plain),
        ("--no-ignore-vcs", Plain),
        ("-u", Plain),
        ("--unrestricted", Plain),
        ("-a", Plain),
        ("--text", Plain),
        ("-z", Plain),
        ("--search-zip", Plain),
        ("-L", Plain),
        ("--follow", Plain),
        ("-U", Plain),
        ("--multiline", Plain),
        ("--multiline-dotall", Plain),
        ("-P", Plain),
        ("--pcre2", Plain),
        ("--json", Plain),
        ("--stats", Plain),
        ("--no-messages", Plain),
        ("--debug", Plain),
        ("--help", Plain),
        ("-V", Plain),
        ("--version", Plain),
    ],
];

/// `fd` / `fdfind`. `-x`/`--exec`, `-X`/`--exec-batch` run a program per
/// result, and `-l` runs `ls` off `PATH`, so all are absent.
pub(super) const FD: Spec = &[
    &[
        ("-d", Number),
        ("--max-depth", Number),
        ("--min-depth", Number),
        ("--exact-depth", Number),
        ("-j", Number),
        ("--threads", Number),
        ("--max-results", Number),
    ],
    &[
        ("-t", Str),
        ("--type", Str),
        ("-e", Str),
        ("--extension", Str),
        ("-S", Str),
        ("--size", Str),
        ("-o", Str),
        ("--owner", Str),
        ("-E", Str),
        ("--exclude", Str),
        ("-c", Str),
        ("--color", Str),
        ("--changed-within", Str),
        ("--changed-before", Str),
        ("--ignore-file", Str),
        ("--search-path", Str),
        ("--base-directory", Str),
        ("--path-separator", Str),
        ("--and", Str),
    ],
    &[
        ("-H", Plain),
        ("--hidden", Plain),
        ("-I", Plain),
        ("--no-ignore", Plain),
        ("--no-ignore-vcs", Plain),
        ("--no-ignore-parent", Plain),
        ("-u", Plain),
        ("--unrestricted", Plain),
        ("-s", Plain),
        ("--case-sensitive", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
        ("-g", Plain),
        ("--glob", Plain),
        ("--regex", Plain),
        ("-F", Plain),
        ("--fixed-strings", Plain),
        ("-a", Plain),
        ("--absolute-path", Plain),
        ("-L", Plain),
        ("--follow", Plain),
        ("-p", Plain),
        ("--full-path", Plain),
        ("-0", Plain),
        ("--print0", Plain),
        ("--prune", Plain),
        ("-1", Plain),
        ("-q", Plain),
        ("--quiet", Plain),
        ("--show-errors", Plain),
        ("--one-file-system", Plain),
        ("--strip-cwd-prefix", Plain),
        ("-h", Plain),
        ("--help", Plain),
        ("-V", Plain),
        ("--version", Plain),
    ],
];

/// `tree`. `-o` writes its listing to a file and `-R` (with `-H`) writes an
/// HTML file into every directory, so both are absent.
pub(super) const TREE: Spec = &[
    &[("-L", Number), ("--filelimit", Number)],
    &[
        ("-P", Str),
        ("-I", Str),
        ("--gitfile", Str),
        ("--infofile", Str),
        ("--charset", Str),
        ("--timefmt", Str),
        ("--sort", Str),
        ("-H", Str),
        ("-T", Str),
    ],
    &[
        ("-a", Plain),
        ("-d", Plain),
        ("-l", Plain),
        ("-f", Plain),
        ("-x", Plain),
        ("-q", Plain),
        ("-N", Plain),
        ("-Q", Plain),
        ("-p", Plain),
        ("-u", Plain),
        ("-g", Plain),
        ("-s", Plain),
        ("-h", Plain),
        ("-D", Plain),
        ("-F", Plain),
        ("-v", Plain),
        ("-t", Plain),
        ("-c", Plain),
        ("-U", Plain),
        ("-r", Plain),
        ("-i", Plain),
        ("-A", Plain),
        ("-S", Plain),
        ("-n", Plain),
        ("-C", Plain),
        ("-X", Plain),
        ("-J", Plain),
        ("--gitignore", Plain),
        ("--ignore-case", Plain),
        ("--matchdirs", Plain),
        ("--metafirst", Plain),
        ("--prune", Plain),
        ("--info", Plain),
        ("--noreport", Plain),
        ("--si", Plain),
        ("--du", Plain),
        ("--inodes", Plain),
        ("--device", Plain),
        ("--dirsfirst", Plain),
        ("--filesfirst", Plain),
        ("--nolinks", Plain),
        ("--help", Plain),
        ("--version", Plain),
    ],
];

/// `sort`. `-o`/`--output` writes a file, and `-T`, `--compress-program` and
/// `--files0-from` reach beyond the words, so all are absent.
pub(super) const SORT: Spec = &[
    &[
        ("-b", Plain),
        ("--ignore-leading-blanks", Plain),
        ("-d", Plain),
        ("--dictionary-order", Plain),
        ("-f", Plain),
        ("--ignore-case", Plain),
        ("-g", Plain),
        ("--general-numeric-sort", Plain),
        ("-h", Plain),
        ("--human-numeric-sort", Plain),
        ("-i", Plain),
        ("--ignore-nonprinting", Plain),
        ("-M", Plain),
        ("--month-sort", Plain),
        ("-n", Plain),
        ("--numeric-sort", Plain),
        ("-R", Plain),
        ("--random-sort", Plain),
        ("-r", Plain),
        ("--reverse", Plain),
        ("-s", Plain),
        ("--stable", Plain),
        ("-u", Plain),
        ("--unique", Plain),
        ("-V", Plain),
        ("--version-sort", Plain),
        ("-z", Plain),
        ("--zero-terminated", Plain),
        ("-c", Plain),
        ("-C", Plain),
        ("--check", Plain),
        ("-m", Plain),
        ("--merge", Plain),
        ("--help", Plain),
        ("--version", Plain),
    ],
    &[
        ("-k", Str),
        ("--key", Str),
        ("-t", Str),
        ("--field-separator", Str),
        ("-S", Str),
        ("--buffer-size", Str),
        ("--sort", Str),
    ],
    &[("--parallel", Number), ("--batch-size", Number)],
];

/// `file`. `-C` compiles a magic file, so it is absent.
pub(super) const FILE: Spec = &[
    &[
        ("-b", Plain),
        ("--brief", Plain),
        ("-i", Plain),
        ("--mime", Plain),
        ("--mime-type", Plain),
        ("--mime-encoding", Plain),
        ("--apple", Plain),
        ("-c", Plain),
        ("--checking-printout", Plain),
        ("-0", Plain),
        ("--print0", Plain),
        ("-h", Plain),
        ("--no-dereference", Plain),
        ("-L", Plain),
        ("--dereference", Plain),
        ("-k", Plain),
        ("--keep-going", Plain),
        ("-l", Plain),
        ("--list", Plain),
        ("-n", Plain),
        ("--no-buffer", Plain),
        ("-N", Plain),
        ("--no-pad", Plain),
        ("-r", Plain),
        ("--raw", Plain),
        ("-s", Plain),
        ("--special-files", Plain),
        ("-z", Plain),
        ("--uncompress", Plain),
        ("-v", Plain),
        ("--version", Plain),
        ("--help", Plain),
    ],
    &[
        ("-e", Str),
        ("--exclude", Str),
        ("-f", Str),
        ("--files-from", Str),
        ("-F", Str),
        ("--separator", Str),
        ("-m", Str),
        ("--magic-file", Str),
    ],
];

/// `sha256sum`, `sha1sum`, `md5sum`.
pub(super) const CHECKSUM: Spec = &[&[
    ("-b", Plain),
    ("--binary", Plain),
    ("-t", Plain),
    ("--text", Plain),
    ("-c", Plain),
    ("--check", Plain),
    ("--ignore-missing", Plain),
    ("--quiet", Plain),
    ("--status", Plain),
    ("--strict", Plain),
    ("-w", Plain),
    ("--warn", Plain),
    ("--tag", Plain),
    ("-z", Plain),
    ("--zero", Plain),
    ("--help", Plain),
    ("--version", Plain),
]];

/// `ps`. BSD's dashless `e` shows every process's environment, which the
/// caller refuses on top of this table.
pub(super) const PS: Spec = &[
    &[
        ("-e", Plain),
        ("-A", Plain),
        ("-a", Plain),
        ("-d", Plain),
        ("-N", Plain),
        ("--deselect", Plain),
        ("-f", Plain),
        ("-F", Plain),
        ("-l", Plain),
        ("-j", Plain),
        ("-y", Plain),
        ("-w", Plain),
        ("-ww", Plain),
        ("-c", Plain),
        ("-H", Plain),
        ("--forest", Plain),
        ("--headers", Plain),
        ("--no-headers", Plain),
        ("-L", Plain),
        ("-T", Plain),
        ("-m", Plain),
        ("--help", Plain),
        ("--info", Plain),
        ("-V", Plain),
        ("--version", Plain),
    ],
    &[("--width", Number)],
    &[
        ("-o", Str),
        ("-O", Str),
        ("--format", Str),
        ("-n", Str),
        ("--sort", Str),
        ("-C", Str),
        ("-G", Str),
        ("-g", Str),
        ("-p", Str),
        ("--pid", Str),
        ("-q", Str),
        ("--quick-pid", Str),
        ("-s", Str),
        ("--sid", Str),
        ("-t", Str),
        ("--tty", Str),
        ("-U", Str),
        ("-u", Str),
        ("--user", Str),
    ],
];

/// `date`. Setting the clock is `-s` or a dashless positional, so the caller
/// also requires every positional to be a `+FORMAT`.
pub(super) const DATE: Spec = &[
    &[
        ("-d", Str),
        ("--date", Str),
        ("-r", Str),
        ("--reference", Str),
        ("--iso-8601", Str),
        ("--rfc-3339", Str),
    ],
    &[
        ("-u", Plain),
        ("--utc", Plain),
        ("--universal", Plain),
        ("-I", Plain),
        ("-R", Plain),
        ("--rfc-email", Plain),
        ("--debug", Plain),
        ("--help", Plain),
        ("--version", Plain),
    ],
];

/// `hostname` — switches only; a positional would set the name.
pub(super) const HOSTNAME: Spec = &[&[
    ("-f", Plain),
    ("--fqdn", Plain),
    ("--long", Plain),
    ("-s", Plain),
    ("--short", Plain),
    ("-i", Plain),
    ("--ip-address", Plain),
    ("-I", Plain),
    ("--all-ip-addresses", Plain),
    ("-a", Plain),
    ("--alias", Plain),
    ("-d", Plain),
    ("--domain", Plain),
    ("-A", Plain),
    ("--all-fqdns", Plain),
    ("-v", Plain),
    ("--verbose", Plain),
    ("-h", Plain),
    ("--help", Plain),
    ("-V", Plain),
    ("--version", Plain),
]];

/// `base64`. Its output goes to stdout; there is no flag that writes a file.
pub(super) const BASE64: Spec = &[
    &[
        ("-d", Plain),
        ("-D", Plain),
        ("--decode", Plain),
        ("--ignore-garbage", Plain),
        ("-h", Plain),
        ("--help", Plain),
        ("--version", Plain),
    ],
    &[
        ("-b", Number),
        ("--break", Number),
        ("-w", Number),
        ("--wrap", Number),
    ],
    &[("-i", Str), ("--input", Str)],
];

/// `pgrep` — it only names processes (`pkill` is another program).
pub(super) const PGREP: Spec = &[
    &[
        ("-d", Str),
        ("--delimiter", Str),
        ("-g", Str),
        ("--pgroup", Str),
        ("-G", Str),
        ("--group", Str),
        ("-O", Str),
        ("--older", Str),
        ("-P", Str),
        ("--parent", Str),
        ("-s", Str),
        ("--session", Str),
        ("-t", Str),
        ("--terminal", Str),
        ("-u", Str),
        ("--euid", Str),
        ("-U", Str),
        ("--uid", Str),
        ("-F", Str),
        ("--pidfile", Str),
        ("-r", Str),
        ("--runstates", Str),
        ("--ns", Str),
        ("--nslist", Str),
    ],
    &[
        ("-l", Plain),
        ("--list-name", Plain),
        ("-a", Plain),
        ("--list-full", Plain),
        ("-v", Plain),
        ("--inverse", Plain),
        ("-w", Plain),
        ("--lightweight", Plain),
        ("-c", Plain),
        ("--count", Plain),
        ("-f", Plain),
        ("--full", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
        ("-n", Plain),
        ("--newest", Plain),
        ("-o", Plain),
        ("--oldest", Plain),
        ("-x", Plain),
        ("--exact", Plain),
        ("-L", Plain),
        ("--logpidfile", Plain),
        ("--help", Plain),
        ("-V", Plain),
        ("--version", Plain),
    ],
];

/// `xargs`'s own flags. `-i` and `-e` take an *optional* attached value in
/// GNU xargs, which a reader of the words cannot tell from the next word, so
/// both are absent.
pub(super) const XARGS: Spec = &[
    &[
        ("-0", Plain),
        ("--null", Plain),
        ("-t", Plain),
        ("--verbose", Plain),
        ("-r", Plain),
        ("--no-run-if-empty", Plain),
        ("-x", Plain),
        ("--exit", Plain),
    ],
    &[
        ("-n", Number),
        ("--max-args", Number),
        ("-P", Number),
        ("--max-procs", Number),
        ("-L", Number),
        ("--max-lines", Number),
        ("-s", Number),
        ("--max-chars", Number),
    ],
    &[("-d", Char), ("--delimiter", Char)],
];

/// The programs `xargs` may hand its input to — each of them read-only with
/// any words appended (Claude Code's `SAFE_TARGET_COMMANDS_FOR_XARGS`).
pub(super) const XARGS_TARGETS: &[&str] = &["echo", "printf", "wc", "grep", "head", "tail"];

/// Validate an `xargs` segment's words: its own flags, then a target from
/// [`XARGS_TARGETS`] whose words are checked as that program's would be.
/// `-I {}` (a replacement string) is the one value-taking flag whose value is
/// fixed, and `-E EOF` the one end marker.
pub(super) fn xargs(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_str();
        match word {
            "-I" => {
                if args.get(i + 1).map(String::as_str) != Some("{}") {
                    return false;
                }
                i += 2;
            }
            "-E" => {
                if args.get(i + 1).map(String::as_str) != Some("EOF") {
                    return false;
                }
                i += 2;
            }
            "--" => {
                i += 1;
                break;
            }
            flag if flag.starts_with('-') => {
                // One flag (with its value, when it takes one) at a time.
                let take = match lookup(XARGS, flag) {
                    Some(Plain) => 1,
                    Some(arg) => {
                        if !args.get(i + 1).is_some_and(|value| value_fits(arg, value)) {
                            return false;
                        }
                        2
                    }
                    None => {
                        // A bundle of plain switches (`-0r`).
                        if !short_flag_fits(flag, XARGS, Rules::default()) {
                            return false;
                        }
                        1
                    }
                };
                i += take;
            }
            _ => break,
        }
    }
    let Some((target, rest)) = args.get(i..).and_then(<[String]>::split_first) else {
        // `xargs` alone runs `echo`.
        return true;
    };
    if !XARGS_TARGETS.contains(&target.as_str()) {
        return false;
    }
    match target.as_str() {
        "grep" => validate(
            rest,
            GREP,
            Rules {
                attached_numbers: GREP_ATTACHED,
                ..Rules::default()
            },
        ),
        _ => true,
    }
}

/// `sed` limited to the two shapes that provably only read (Claude Code's
/// `sedCommandIsAllowedByAllowlist`): printing line ranges under `-n`
/// (`sed -n '1,40p' file`), or one `s/…/…/` substitution filtering stdin
/// with no file to rewrite. Everything else — `-i`, `w`, `e`, a regex
/// address, several commands — is refused.
pub(super) fn sed(args: &[String]) -> bool {
    let mut quiet = false;
    // Flags pattern 2 (the substitution) does not allow.
    let mut print_only_flags = false;
    let mut scripts: Vec<&str> = Vec::new();
    let mut from_e = false;
    let mut files = 0usize;
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_str();
        match word {
            "-n" | "--quiet" | "--silent" => quiet = true,
            "-E" | "-r" | "--regexp-extended" | "--posix" => {}
            "-z" | "--zero-terminated" => print_only_flags = true,
            "-e" | "--expression" => {
                i += 1;
                let Some(script) = args.get(i) else {
                    return false;
                };
                scripts.push(script);
                from_e = true;
            }
            "--" => {
                files += args.len() - i - 1;
                if !from_e && scripts.is_empty() {
                    // `sed -- SCRIPT …`: the first word is still the script.
                    files = files.saturating_sub(1);
                    if let Some(script) = args.get(i + 1) {
                        scripts.push(script);
                    }
                }
                break;
            }
            flag if flag.starts_with("--expression=") => {
                scripts.push(&flag["--expression=".len()..]);
                from_e = true;
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                for letter in flag[1..].chars() {
                    match letter {
                        'n' => quiet = true,
                        'E' | 'r' => {}
                        'z' => print_only_flags = true,
                        _ => return false,
                    }
                }
            }
            positional => {
                if !from_e && scripts.is_empty() {
                    scripts.push(positional);
                } else {
                    files += 1;
                }
            }
        }
        i += 1;
    }
    if scripts.is_empty() || scripts.iter().any(|script| !script.is_ascii()) {
        return false;
    }
    // Pattern 1: print line ranges, files allowed.
    if quiet
        && scripts
            .iter()
            .flat_map(|script| script.split(';'))
            .all(|command| is_print_command(command.trim()))
    {
        return true;
    }
    // Pattern 2: one substitution over stdin.
    !quiet
        && !print_only_flags
        && files == 0
        && scripts.len() == 1
        && is_plain_substitution(scripts[0])
}

/// `p`, `5p`, `1,40p` — a print of nothing, one line or a line range.
fn is_print_command(command: &str) -> bool {
    let Some(address) = command.strip_suffix('p') else {
        return false;
    };
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match address.split_once(',') {
        _ if address.is_empty() => true,
        Some((from, to)) => number(from) && number(to),
        None => number(address),
    }
}

/// `s/pattern/replacement/flags` with `/` delimiters only and flags drawn
/// from `gpimIM` plus an occurrence number — none of which writes (`w`) or
/// executes (`e`).
fn is_plain_substitution(script: &str) -> bool {
    let Some(body) = script.strip_prefix("s/") else {
        return false;
    };
    // Split on unescaped `/`.
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in body.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            current.push(c);
            escaped = true;
        } else if c == '/' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    parts.push(current);
    let [_, _, flags] = parts.as_slice() else {
        return false;
    };
    if parts
        .iter()
        .any(|part| part.contains(['{', '}', '#', '!', '~']))
    {
        return false;
    }
    let digits = flags.chars().filter(char::is_ascii_digit).count();
    digits <= 1
        && flags
            .chars()
            .all(|c| matches!(c, 'g' | 'p' | 'i' | 'm' | 'I' | 'M') || c.is_ascii_digit())
        && !flags.contains('0')
}

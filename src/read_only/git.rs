//! The `git` subcommands that only read — Claude Code's
//! `GIT_READ_ONLY_COMMANDS`, ported for the inspection an agent does
//! (`docs/tool-groups.md`). The subcommand must be `git`'s very first word, so
//! a global option before it (`git -c core.pager=… log`, `git -C dir status`,
//! `--exec-path`) refuses the command: `-c` can switch on a program git runs.

use super::flags::{Arg, Rules, Spec, validate};

use Arg::{None as Plain, Number, Str};

const REF: &[(&str, Arg)] = &[
    ("--all", Plain),
    ("--branches", Plain),
    ("--tags", Plain),
    ("--remotes", Plain),
];
const DATE: &[(&str, Arg)] = &[
    ("--since", Str),
    ("--after", Str),
    ("--until", Str),
    ("--before", Str),
];
const LOG_DISPLAY: &[(&str, Arg)] = &[
    ("--oneline", Plain),
    ("--graph", Plain),
    ("--decorate", Plain),
    ("--no-decorate", Plain),
    ("--relative-date", Plain),
    ("--date", Str),
];
const COUNT: &[(&str, Arg)] = &[("--max-count", Number), ("-n", Number)];
const STAT: &[(&str, Arg)] = &[
    ("--stat", Plain),
    ("--numstat", Plain),
    ("--shortstat", Plain),
    ("--name-only", Plain),
    ("--name-status", Plain),
];
const COLOR: &[(&str, Arg)] = &[("--color", Plain), ("--no-color", Plain)];
const PATCH: &[(&str, Arg)] = &[
    ("--patch", Plain),
    ("-p", Plain),
    ("--no-patch", Plain),
    ("--no-ext-diff", Plain),
    ("-s", Plain),
];
const AUTHOR: &[(&str, Arg)] = &[("--author", Str), ("--committer", Str), ("--grep", Str)];
const UNIFIED: &[(&str, Arg)] = &[("-U", Number), ("--unified", Number)];

const DIFF: Spec = &[
    STAT,
    COLOR,
    UNIFIED,
    &[
        ("--dirstat", Plain),
        ("--summary", Plain),
        ("--patch-with-stat", Plain),
        ("--word-diff", Plain),
        ("--color-words", Plain),
        ("--no-renames", Plain),
        ("--no-ext-diff", Plain),
        ("--check", Plain),
        ("--full-index", Plain),
        ("--binary", Plain),
        ("--break-rewrites", Plain),
        ("--find-renames", Plain),
        ("--find-copies", Plain),
        ("--find-copies-harder", Plain),
        ("--irreversible-delete", Plain),
        ("--histogram", Plain),
        ("--patience", Plain),
        ("--minimal", Plain),
        ("--ignore-space-at-eol", Plain),
        ("--ignore-space-change", Plain),
        ("--ignore-all-space", Plain),
        ("--ignore-blank-lines", Plain),
        ("--function-context", Plain),
        ("--exit-code", Plain),
        ("--quiet", Plain),
        ("--cached", Plain),
        ("--staged", Plain),
        ("--pickaxe-regex", Plain),
        ("--pickaxe-all", Plain),
        ("--no-index", Plain),
        ("-p", Plain),
        ("-u", Plain),
        ("-s", Plain),
        ("-M", Plain),
        ("-C", Plain),
        ("-B", Plain),
        ("-D", Plain),
        ("-l", Plain),
        ("-R", Plain),
        ("-w", Plain),
        ("-b", Plain),
    ],
    // `-S`, `-G` and `-O` take a value, so `-S -- --output=…` cannot pass a
    // file write off as a pickaxe string.
    &[
        ("--word-diff-regex", Str),
        ("--ws-error-highlight", Str),
        ("--diff-algorithm", Str),
        ("--relative", Str),
        ("--diff-filter", Str),
        ("-S", Str),
        ("-G", Str),
        ("-O", Str),
    ],
    &[("--abbrev", Number), ("--inter-hunk-context", Number)],
];

const LOG: Spec = &[
    LOG_DISPLAY,
    REF,
    DATE,
    COUNT,
    STAT,
    COLOR,
    PATCH,
    AUTHOR,
    UNIFIED,
    &[
        ("--abbrev-commit", Plain),
        ("--full-history", Plain),
        ("--dense", Plain),
        ("--sparse", Plain),
        ("--simplify-merges", Plain),
        ("--ancestry-path", Plain),
        ("--source", Plain),
        ("--first-parent", Plain),
        ("--merges", Plain),
        ("--no-merges", Plain),
        ("--reverse", Plain),
        ("--walk-reflogs", Plain),
        ("--no-min-parents", Plain),
        ("--no-max-parents", Plain),
        ("--follow", Plain),
        ("--no-walk", Plain),
        ("--left-right", Plain),
        ("--cherry-mark", Plain),
        ("--cherry-pick", Plain),
        ("--boundary", Plain),
        ("--topo-order", Plain),
        ("--date-order", Plain),
        ("--author-date-order", Plain),
        ("--pickaxe-regex", Plain),
        ("--pickaxe-all", Plain),
    ],
    &[
        ("--skip", Number),
        ("--max-age", Number),
        ("--min-age", Number),
    ],
    &[
        ("--pretty", Str),
        ("--format", Str),
        ("--diff-filter", Str),
        ("-S", Str),
        ("-G", Str),
    ],
];

const SHOW: Spec = &[
    LOG_DISPLAY,
    STAT,
    COLOR,
    PATCH,
    UNIFIED,
    &[
        ("--abbrev-commit", Plain),
        ("--word-diff", Plain),
        ("--color-words", Plain),
        ("--first-parent", Plain),
        ("--raw", Plain),
        ("-m", Plain),
        ("--quiet", Plain),
    ],
    &[
        ("--word-diff-regex", Str),
        ("--pretty", Str),
        ("--format", Str),
        ("--diff-filter", Str),
    ],
];

const SHORTLOG: Spec = &[
    REF,
    DATE,
    &[
        ("-s", Plain),
        ("--summary", Plain),
        ("-n", Plain),
        ("--numbered", Plain),
        ("-e", Plain),
        ("--email", Plain),
        ("-c", Plain),
        ("--committer", Plain),
        ("--no-merges", Plain),
    ],
    &[("--group", Str), ("--format", Str), ("--author", Str)],
];

const REFLOG: Spec = &[LOG_DISPLAY, REF, DATE, COUNT, AUTHOR];

const STASH_LIST: Spec = &[LOG_DISPLAY, REF, COUNT];

const STASH_SHOW: Spec = &[
    STAT,
    COLOR,
    PATCH,
    &[("--word-diff", Plain)],
    &[("--word-diff-regex", Str), ("--diff-filter", Str)],
    &[("--abbrev", Number)],
];

const STATUS: Spec = &[
    &[
        ("--short", Plain),
        ("-s", Plain),
        ("--branch", Plain),
        ("-b", Plain),
        ("--porcelain", Plain),
        ("--long", Plain),
        ("--verbose", Plain),
        ("-v", Plain),
        ("--ignored", Plain),
        ("--column", Plain),
        ("--no-column", Plain),
        ("--ahead-behind", Plain),
        ("--no-ahead-behind", Plain),
        ("--renames", Plain),
        ("--no-renames", Plain),
    ],
    &[
        ("--untracked-files", Str),
        ("-u", Str),
        ("--ignore-submodules", Str),
        ("--find-renames", Str),
        ("-M", Str),
    ],
];

const BLAME: Spec = &[
    COLOR,
    &[
        ("--porcelain", Plain),
        ("-p", Plain),
        ("--line-porcelain", Plain),
        ("--incremental", Plain),
        ("--root", Plain),
        ("--show-stats", Plain),
        ("--show-name", Plain),
        ("--show-number", Plain),
        ("-n", Plain),
        ("--show-email", Plain),
        ("-e", Plain),
        ("-f", Plain),
        ("-w", Plain),
        ("-M", Plain),
        ("-C", Plain),
        ("--score-debug", Plain),
        ("-s", Plain),
        ("-l", Plain),
        ("-t", Plain),
    ],
    &[
        ("-L", Str),
        ("--date", Str),
        ("--ignore-rev", Str),
        ("--ignore-revs-file", Str),
    ],
    &[("--abbrev", Number)],
];

const LS_FILES: Spec = &[
    &[
        ("--cached", Plain),
        ("-c", Plain),
        ("--deleted", Plain),
        ("-d", Plain),
        ("--modified", Plain),
        ("-m", Plain),
        ("--others", Plain),
        ("-o", Plain),
        ("--ignored", Plain),
        ("-i", Plain),
        ("--stage", Plain),
        ("-s", Plain),
        ("--killed", Plain),
        ("-k", Plain),
        ("--unmerged", Plain),
        ("-u", Plain),
        ("--directory", Plain),
        ("--no-empty-directory", Plain),
        ("--eol", Plain),
        ("--full-name", Plain),
        ("--debug", Plain),
        ("-z", Plain),
        ("-t", Plain),
        ("-v", Plain),
        ("-f", Plain),
        ("--exclude-standard", Plain),
        ("--error-unmatch", Plain),
        ("--recurse-submodules", Plain),
    ],
    &[
        ("--exclude", Str),
        ("-x", Str),
        ("--exclude-from", Str),
        ("-X", Str),
        ("--exclude-per-directory", Str),
    ],
    &[("--abbrev", Number)],
];

const CONFIG_GET: Spec = &[
    &[
        ("--local", Plain),
        ("--global", Plain),
        ("--system", Plain),
        ("--worktree", Plain),
        ("--bool", Plain),
        ("--int", Plain),
        ("--bool-or-int", Plain),
        ("--path", Plain),
        ("--expiry-date", Plain),
        ("-z", Plain),
        ("--null", Plain),
        ("--name-only", Plain),
        ("--show-origin", Plain),
        ("--show-scope", Plain),
    ],
    &[("--default", Str), ("--type", Str)],
];

const MERGE_BASE: Spec = &[&[
    ("--is-ancestor", Plain),
    ("--fork-point", Plain),
    ("--octopus", Plain),
    ("--independent", Plain),
    ("--all", Plain),
]];

const REV_PARSE: Spec = &[
    &[
        ("--verify", Plain),
        ("-q", Plain),
        ("--quiet", Plain),
        ("--abbrev-ref", Plain),
        ("--symbolic", Plain),
        ("--symbolic-full-name", Plain),
        ("--show-toplevel", Plain),
        ("--show-cdup", Plain),
        ("--show-prefix", Plain),
        ("--git-dir", Plain),
        ("--git-common-dir", Plain),
        ("--absolute-git-dir", Plain),
        ("--show-superproject-working-tree", Plain),
        ("--is-inside-work-tree", Plain),
        ("--is-inside-git-dir", Plain),
        ("--is-bare-repository", Plain),
        ("--is-shallow-repository", Plain),
    ],
    &[("--short", Str), ("--path-prefix", Str)],
];

const REV_LIST: Spec = &[
    REF,
    DATE,
    COUNT,
    AUTHOR,
    &[
        ("--count", Plain),
        ("--reverse", Plain),
        ("--first-parent", Plain),
        ("--ancestry-path", Plain),
        ("--merges", Plain),
        ("--no-merges", Plain),
        ("--no-min-parents", Plain),
        ("--no-max-parents", Plain),
        ("--walk-reflogs", Plain),
        ("--oneline", Plain),
        ("--abbrev-commit", Plain),
        ("--full-history", Plain),
        ("--dense", Plain),
        ("--sparse", Plain),
        ("--source", Plain),
        ("--graph", Plain),
    ],
    &[
        ("--min-parents", Number),
        ("--max-parents", Number),
        ("--skip", Number),
        ("--max-age", Number),
        ("--min-age", Number),
        ("--abbrev", Number),
    ],
    &[("--pretty", Str), ("--format", Str)],
];

const DESCRIBE: Spec = &[
    &[
        ("--tags", Plain),
        ("--long", Plain),
        ("--always", Plain),
        ("--contains", Plain),
        ("--first-match", Plain),
        ("--exact-match", Plain),
        ("--dirty", Plain),
        ("--broken", Plain),
    ],
    &[("--match", Str), ("--exclude", Str)],
    &[("--abbrev", Number), ("--candidates", Number)],
];

const CAT_FILE: Spec = &[&[
    ("-t", Plain),
    ("-s", Plain),
    ("-p", Plain),
    ("-e", Plain),
    ("--batch-check", Plain),
    ("--allow-undetermined-type", Plain),
]];

const FOR_EACH_REF: Spec = &[
    &[
        ("--format", Str),
        ("--sort", Str),
        ("--contains", Str),
        ("--no-contains", Str),
        ("--merged", Str),
        ("--no-merged", Str),
        ("--points-at", Str),
    ],
    &[("--count", Number)],
];

/// `git grep`. `-O`/`--open-files-in-pager` runs a program, so it is absent.
const GREP: Spec = &[
    &[("-e", Str)],
    &[
        ("-A", Number),
        ("--after-context", Number),
        ("-B", Number),
        ("--before-context", Number),
        ("-C", Number),
        ("--context", Number),
        ("--max-depth", Number),
        ("--threads", Number),
    ],
    &[
        ("-E", Plain),
        ("--extended-regexp", Plain),
        ("-G", Plain),
        ("--basic-regexp", Plain),
        ("-F", Plain),
        ("--fixed-strings", Plain),
        ("-P", Plain),
        ("--perl-regexp", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
        ("-v", Plain),
        ("--invert-match", Plain),
        ("-w", Plain),
        ("--word-regexp", Plain),
        ("-n", Plain),
        ("--line-number", Plain),
        ("-c", Plain),
        ("--count", Plain),
        ("-l", Plain),
        ("--files-with-matches", Plain),
        ("-L", Plain),
        ("--files-without-match", Plain),
        ("-h", Plain),
        ("-H", Plain),
        ("-o", Plain),
        ("--only-matching", Plain),
        ("-q", Plain),
        ("--quiet", Plain),
        ("--heading", Plain),
        ("--break", Plain),
        ("--full-name", Plain),
        ("--color", Plain),
        ("--no-color", Plain),
        ("--and", Plain),
        ("--or", Plain),
        ("--not", Plain),
        ("--untracked", Plain),
        ("--no-index", Plain),
        ("--recurse-submodules", Plain),
        ("--cached", Plain),
    ],
];

const WORKTREE_LIST: Spec = &[
    &[("--porcelain", Plain), ("-v", Plain), ("--verbose", Plain)],
    &[("--expire", Str)],
];

const TAG: Spec = &[
    &[
        ("-l", Plain),
        ("--list", Plain),
        ("--column", Plain),
        ("--no-column", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
    ],
    &[("-n", Number)],
    &[
        ("--contains", Str),
        ("--no-contains", Str),
        ("--merged", Str),
        ("--no-merged", Str),
        ("--sort", Str),
        ("--format", Str),
        ("--points-at", Str),
    ],
];

const BRANCH: Spec = &[
    &[
        ("-l", Plain),
        ("--list", Plain),
        ("-a", Plain),
        ("--all", Plain),
        ("-r", Plain),
        ("--remotes", Plain),
        ("-v", Plain),
        ("-vv", Plain),
        ("--verbose", Plain),
        ("--color", Plain),
        ("--no-color", Plain),
        ("--column", Plain),
        ("--no-column", Plain),
        ("--no-abbrev", Plain),
        ("--merged", Plain),
        ("--no-merged", Plain),
        ("--show-current", Plain),
        ("-i", Plain),
        ("--ignore-case", Plain),
    ],
    &[("--abbrev", Number)],
    &[
        ("--contains", Str),
        ("--no-contains", Str),
        ("--points-at", Str),
        ("--sort", Str),
    ],
];

/// Whether `git {args}` only reads. `args[0]` is the subcommand.
pub(super) fn read_only(args: &[String]) -> bool {
    let Some((sub, rest)) = args.split_first() else {
        return false;
    };
    let plain = Rules::default();
    let numbered = Rules {
        attached_numbers: &['U', 'n'],
        dash_number: true,
        ..Rules::default()
    };
    match sub.as_str() {
        "--version" => rest.is_empty(),
        "status" => validate(rest, STATUS, plain),
        "log" => validate(rest, LOG, numbered),
        "diff" => validate(
            rest,
            DIFF,
            Rules {
                attached_numbers: &['U'],
                ..Rules::default()
            },
        ),
        "show" => validate(rest, SHOW, numbered),
        "shortlog" => validate(rest, SHORTLOG, plain),
        // `git reflog expire|delete` rewrite the reflog.
        "reflog" => {
            validate(rest, REFLOG, numbered)
                && !first_positional(rest)
                    .is_some_and(|word| matches!(word, "expire" | "delete" | "exists"))
        }
        "stash" => match rest.split_first() {
            Some((what, rest)) if what == "list" => validate(rest, STASH_LIST, numbered),
            Some((what, rest)) if what == "show" => validate(rest, STASH_SHOW, plain),
            _ => false,
        },
        "blame" => validate(rest, BLAME, plain),
        "ls-files" => validate(rest, LS_FILES, plain),
        // Only the literal `git config --get …` form reads.
        "config" => match rest.split_first() {
            Some((get, rest)) if get == "--get" => validate(rest, CONFIG_GET, plain),
            _ => false,
        },
        "remote" => remote(rest),
        "merge-base" => validate(rest, MERGE_BASE, plain),
        "rev-parse" => validate(rest, REV_PARSE, plain),
        "rev-list" => validate(rest, REV_LIST, numbered),
        "describe" => validate(rest, DESCRIBE, plain),
        "cat-file" => validate(rest, CAT_FILE, plain),
        "for-each-ref" => validate(rest, FOR_EACH_REF, plain),
        "grep" => validate(
            rest,
            GREP,
            Rules {
                attached_numbers: &['A', 'B', 'C'],
                ..Rules::default()
            },
        ),
        "worktree" => match rest.split_first() {
            Some((what, rest)) if what == "list" => validate(rest, WORKTREE_LIST, plain),
            _ => false,
        },
        // A positional without `-l` names a tag to create.
        "tag" => validate(rest, TAG, plain) && (positionals(rest).is_empty() || lists(rest)),
        "branch" => branch(rest),
        _ => false,
    }
}

/// `git branch` lists unless a positional names a branch to create — one
/// that neither `-l`/`--list` filters nor `--merged`/`--no-merged` takes as
/// its commit. `--abbrev` must carry its number with `=`: detached, the
/// number would be read as a branch to create.
fn branch(rest: &[String]) -> bool {
    if !validate(rest, BRANCH, Rules::default()) || rest.iter().any(|word| word == "--abbrev") {
        return false;
    }
    if lists(rest) {
        return true;
    }
    let mut previous: Option<&str> = None;
    for word in rest {
        if word == "--" {
            return false;
        }
        let is_value = matches!(
            previous,
            Some("--contains" | "--no-contains" | "--points-at" | "--sort")
        );
        let follows_merged = matches!(previous, Some("--merged" | "--no-merged"));
        if !word.starts_with('-') && !is_value && !follows_merged {
            return false;
        }
        previous = Some(word);
    }
    true
}

/// `git remote` with only `-v`, or `git remote show NAME` with only `-n`.
fn remote(rest: &[String]) -> bool {
    match rest.split_first() {
        None => true,
        Some((show, rest)) if show == "show" => {
            let names: Vec<&String> = rest.iter().filter(|word| *word != "-n").collect();
            names.len() == 1
                && names[0]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }
        Some(_) => rest.iter().all(|word| word == "-v" || word == "--verbose"),
    }
}

/// Whether the words ask for a listing: `-l`, `--list`, or a bundle with `l`.
fn lists(words: &[String]) -> bool {
    words.iter().any(|word| {
        word == "--list" || (word.starts_with('-') && !word.starts_with("--") && word.contains('l'))
    })
}

/// The words that are not flags — approximately: a value a flag takes is
/// counted too, which only ever makes a listing look like a creation.
fn positionals(words: &[String]) -> Vec<&String> {
    words.iter().filter(|word| !word.starts_with('-')).collect()
}

/// The first word that is not a flag.
fn first_positional(words: &[String]) -> Option<&str> {
    words
        .iter()
        .map(String::as_str)
        .find(|word| !word.starts_with('-'))
}

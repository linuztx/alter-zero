//! Read-only `bash` commands: what auto mode may run without asking, and what
//! the folded tool view counts each as. See `docs/tool-groups.md`.

use super::*;

/// Assert every command in `commands` is read-only.
#[track_caller]
fn all_read_only(commands: &[&str]) {
    for command in commands {
        assert!(is_read_only(command), "{command:?} should be read-only");
    }
}

/// Assert no command in `commands` is read-only.
#[track_caller]
fn none_read_only(commands: &[&str]) {
    for command in commands {
        assert!(!is_read_only(command), "{command:?} must not be read-only");
    }
}

/// The kind `command` reads as.
fn kind(command: &str) -> Option<ReadKind> {
    reading(command).map(|reading| reading.kind)
}

/// The operands `command` names.
fn targets(command: &str) -> Vec<String> {
    reading(command)
        .map(|reading| reading.targets)
        .unwrap_or_default()
}

// ===== what counts as read-only =====

#[test]
fn the_everyday_readers_are_read_only() {
    all_read_only(&[
        "ls -la",
        "ls",
        "tree -L 2",
        "du -sh .",
        "cat src/main.rs",
        "head -20 a.py",
        "head -n 20 a.py",
        "tail -f log.txt",
        "wc -l src/app.rs",
        "grep -rn \"fn main\" src",
        "rg -n TODO --type rust",
        "find . -name '*.rs'",
        "sed -n '1,80p' src/lib.rs",
        "stat Cargo.toml",
        "which cargo",
        "pwd",
        "echo hi",
    ]);
}

#[test]
fn commands_that_change_or_run_something_are_not() {
    none_read_only(&[
        "rm -rf target",
        "touch a",
        "mkdir build",
        "npm test",
        "python3 script.py",
        "cargo build",
        "curl https://example.com",
        "",
        "   ",
    ]);
}

#[test]
fn a_flag_that_writes_or_runs_disqualifies_a_reader() {
    none_read_only(&[
        "find . -delete",
        "find . -name x -exec rm {} \\;",
        "find . -execdir sh -c x \\;",
        "find . -fprint out",
        "sort -o out.txt in.txt",
        "sort -uo out.txt in.txt",
        "sort --output=out.txt in.txt",
        "rg --pre ./script foo",
        "rg --pre=./script foo",
        "fd -x rm",
        "fd --exec rm",
        "tree -o out.html",
        "sed -i 's/a/b/' f",
        "sed 's/a/b/' f",
        "sed -n 'w out' f",
        "file -C -m magic",
        "uniq in.txt out.txt",
        "date -s 2020-01-01",
        "date 0101000020",
        "hostname example",
        "jq -f filter.jq data.json",
        "jq --rawfile x /etc/passwd . data.json",
    ]);
}

#[test]
fn a_safe_reader_keeps_its_ordinary_flags() {
    all_read_only(&[
        "sort -u names.txt",
        "uniq -c sorted.txt",
        "date +%Y-%m-%d",
        "hostname -s",
        "jq '.dependencies' package.json",
        "sed -n -e '10,20p' -e '40p' notes.md",
        "find . -type f -name '*.md' -newer Cargo.toml",
        "tree -a -I target",
    ]);
}

#[test]
fn redirection_substitution_and_expansion_are_refused() {
    none_read_only(&[
        "ls > files.txt",
        "ls >> files.txt",
        "cat a.txt > b.txt",
        "cat < input.txt",
        "cat <<EOF\nx\nEOF",
        "echo $(rm -rf x)",
        "echo `rm -rf x`",
        "grep \"$(whoami)\" f",
        "cat $HOME/.bashrc",
        "cat \"$SECRET\"",
        "ls ${DIR}",
        "diff <(ls a) <(ls b)",
        "echo $'\\x41'",
    ]);
}

#[test]
fn a_redirect_into_nowhere_still_counts() {
    all_read_only(&[
        "grep -rn foo . 2>/dev/null",
        "find / -name x 2> /dev/null",
        "ls 2>&1",
        "cat a.txt >/dev/null",
        "ls &>/dev/null",
    ]);
}

#[test]
fn wrappers_env_assignments_and_groupings_are_refused() {
    none_read_only(&[
        "sudo ls",
        "env ls",
        "FOO=1 ls",
        "time ls",
        "timeout 5 cat f",
        "xargs cat",
        "nohup ls",
        "sh -c 'ls'",
        "bash -c ls",
        "(ls)",
        "{ ls; }",
        "/usr/bin/ls",
        "./ls",
        "less README.md",
        "awk '{print $1}' f",
    ]);
}

#[test]
fn a_compound_command_is_read_only_only_when_every_part_is() {
    all_read_only(&[
        "cd src && ls",
        "ls -la | head -20",
        "cat a.py | grep def",
        "ls; pwd",
        "git status && git log --oneline -5",
        "ls src\nls tests",
    ]);
    none_read_only(&[
        "ls; rm x",
        "cat a || rm b",
        "ls | tee out",
        "cd /tmp && rm -rf x",
        "ls\nrm x",
        "cd a b",
    ]);
}

#[test]
fn quotes_protect_what_they_hold() {
    all_read_only(&[
        "grep \"a > b\" f",
        "grep 'x|y' f",
        "echo \"one; two\"",
        "grep 'foo$' f",
        "grep \"^end$\" f",
        "rg '\\$HOME' docs",
        "grep -E 'a|b' f # a trailing comment",
    ]);
}

#[test]
fn globs_reach_only_a_program_with_no_dangerous_flag() {
    // A glob expands to whatever the directory holds, flags included — a
    // file named `-delete` makes `find *` delete. So it is allowed where no
    // expansion could turn into a write.
    all_read_only(&[
        "ls *.rs",
        "cat src/*.rs",
        "wc -l src/*.rs",
        "grep -l foo *.md",
    ]);
    none_read_only(&["find * -name x", "rg foo *", "sort *.txt", "sed -n 1p *"]);
    // A quoted or escaped glob is just text.
    all_read_only(&[
        "find . -name '*.rs'",
        "find . -name \\*.rs",
        "rg foo -g '*.rs'",
    ]);
}

#[test]
fn git_reads_are_read_only_and_its_writes_are_not() {
    all_read_only(&[
        "git status",
        "git status --short",
        "git log --oneline -10",
        "git --no-pager log -5",
        "git diff HEAD~1",
        "git diff --stat",
        "git show HEAD:src/main.rs",
        "git blame -L 10,20 src/main.rs",
        "git branch",
        "git branch -a",
        "git branch --show-current",
        "git branch -l 'feat/*'",
        "git tag",
        "git tag -l 'v*'",
        "git remote -v",
        "git rev-parse HEAD",
        "git ls-files",
        "git grep -n TODO",
        "git config --get user.name",
        "git config --list",
        "git stash list",
        "git describe --tags",
        "git merge-base HEAD main",
    ]);
    none_read_only(&[
        "git",
        "git push",
        "git commit -m x",
        "git checkout main",
        "git add .",
        "git stash",
        "git branch feature",
        "git branch -d old",
        "git tag v1.0",
        "git config user.name me",
        "git remote add origin x",
        "git -c core.pager=sh log",
        "git -C /tmp status",
        "git diff --output=out.patch",
        "git log --ext-diff",
        "git reflog expire --all",
    ]);
}

// ===== what the folded view counts each as =====

#[test]
fn each_reader_has_its_kind() {
    for (command, expected) in [
        ("ls -la", ReadKind::List),
        ("tree", ReadKind::List),
        ("du -sh src", ReadKind::List),
        ("grep -rn foo .", ReadKind::Search),
        ("rg foo", ReadKind::Search),
        ("find . -name '*.rs'", ReadKind::Search),
        ("which cargo", ReadKind::Search),
        ("git grep TODO", ReadKind::Search),
        ("cat a.py", ReadKind::Read),
        ("head -20 a", ReadKind::Read),
        ("wc -l x", ReadKind::Read),
        ("sed -n '1,5p' a.py", ReadKind::Read),
    ] {
        assert_eq!(kind(command), Some(expected), "{command:?}");
    }
}

#[test]
fn a_command_that_neither_searches_reads_nor_lists_has_no_kind() {
    // Read-only, so auto mode runs them unasked — but they show their own
    // cell: what `git status` printed is worth seeing.
    for command in ["git status", "git log --oneline", "pwd", "echo hi", "date"] {
        assert!(is_read_only(command), "{command:?}");
        assert_eq!(kind(command), None, "{command:?}");
    }
    // And nothing that is not read-only reads as anything.
    for command in ["rm x", "ls > out", "npm test", "awk '{print}' f"] {
        assert_eq!(kind(command), None, "{command:?}");
    }
}

#[test]
fn a_compound_reads_as_its_strongest_part() {
    // Claude Code's precedence: a listing outranks a search outranks a read.
    assert_eq!(kind("cat a.py | grep def"), Some(ReadKind::Search));
    assert_eq!(kind("ls && cat x"), Some(ReadKind::List));
    assert_eq!(kind("cd src && ls"), Some(ReadKind::List));
    assert_eq!(kind("ls | head -5"), Some(ReadKind::List));
    // `echo`, `printf`, `true`, `false`, `:` and `cd` say nothing.
    assert_eq!(kind("echo '---' && cat a"), Some(ReadKind::Read));
    // …but a part with no kind of its own leaves the whole without one.
    assert_eq!(kind("git status && ls"), None);
}

#[test]
fn a_read_names_the_files_it_reads() {
    assert_eq!(targets("cat a.py b.py"), ["a.py", "b.py"]);
    assert_eq!(targets("head -n 20 a.py"), ["a.py"]);
    assert_eq!(targets("head -20 a.py"), ["a.py"]);
    assert_eq!(targets("tail -n20 -f log.txt"), ["log.txt"]);
    assert_eq!(targets("wc -l src/app.rs"), ["src/app.rs"]);
    assert_eq!(targets("sed -n '1,5p' a.py"), ["a.py"]);
    assert_eq!(
        targets("sed -n -e '1p' -e '5p' a.py b.py"),
        ["a.py", "b.py"]
    );
    assert_eq!(targets("cat \"my file.txt\""), ["my file.txt"]);
    assert_eq!(targets("cut -d, -f2 data.csv"), ["data.csv"]);
    assert_eq!(targets("jq '.name' package.json"), ["package.json"]);
    // Reading standard input, it names no file.
    assert!(targets("sort -u").is_empty());
}

#[test]
fn a_listing_names_the_directories_it_lists() {
    assert_eq!(targets("ls -la src tests"), ["src", "tests"]);
    assert!(
        targets("ls -la").is_empty(),
        "the current directory, unnamed"
    );
    assert_eq!(targets("tree -L 2 docs"), ["docs"]);
    assert_eq!(targets("cd src && ls lib"), ["lib"]);
}

// ===== the shapes that must not slip through =====

#[test]
fn an_abbreviated_long_flag_is_the_flag_it_abbreviates() {
    // GNU getopt and git both accept any unambiguous prefix of a long option,
    // so `--out` *is* `--output` to sort.
    none_read_only(&[
        "sort --out=x.txt in.txt",
        "sort --outp x.txt in.txt",
        "git diff --outp=out.patch",
        "git log --ext",
        "file --comp -m magic",
        "rg --pr=./script foo",
        "git tag -l --del v1",
        "git branch -l --del old",
    ]);
    // …while a different flag that merely starts alike is still fine.
    all_read_only(&[
        "sort --stable in.txt",
        "rg --pretty foo",
        "git log --oneline",
    ]);
}

#[test]
fn quoting_a_forbidden_flag_does_not_hide_it() {
    none_read_only(&[
        r"find . -ex''ec rm \;",
        "find . \"-delete\"",
        "sort \"-o\" out in",
        "rg \"--pre=sh\" foo",
    ]);
}

#[test]
fn a_redirect_through_a_descriptor_or_device_is_still_a_redirect() {
    none_read_only(&[
        "ls >& out.txt",
        "printf x > /dev/tcp/example.com/80",
        "cat 0< secrets.txt",
        "ls 2>err.txt",
    ]);
    all_read_only(&["ls >&2", "ls 2>&-"]);
}

#[test]
fn a_comment_hides_nothing_and_a_mid_word_hash_is_text() {
    all_read_only(&["ls # rm -rf /", "ls;# rm -rf /"]);
    none_read_only(&["echo a#; rm -rf x"]);
}

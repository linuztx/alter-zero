use super::*;

// ===== read_kind: what a command reads, for the collapsed cell =====

#[test]
fn a_listing_reads_as_a_list() {
    for command in ["ls", "ls -la", "tree src", "du -sh .", "ls -la ~/projects"] {
        assert_eq!(read_kind(command), Some(ReadKind::List), "{command}");
    }
}

#[test]
fn a_search_reads_as_a_search() {
    for command in [
        "grep -rn 'fn main' src",
        "rg foo",
        "find . -name '*.rs' -type f",
        "which cargo",
        "fd main",
        "egrep -n 'a|b' notes.md",
        "locate bashrc",
    ] {
        assert_eq!(read_kind(command), Some(ReadKind::Search), "{command}");
    }
}

#[test]
fn a_file_read_reads_as_a_read() {
    for command in [
        "cat about.py",
        "head -n 20 src/main.rs",
        "tail -f server.log",
        "wc -l src/lib.rs",
        "sed -n '1,40p' src/app.rs",
        "jq .name package.json",
        "stat Cargo.toml",
    ] {
        assert_eq!(read_kind(command), Some(ReadKind::Read), "{command}");
    }
}

#[test]
fn a_pipeline_takes_the_strongest_kind_list_then_search_then_read() {
    // The reference's precedence: a listing piped into a search is still
    // the listing it started as, and a read filtered by a search is a search.
    assert_eq!(read_kind("ls | grep foo"), Some(ReadKind::List));
    assert_eq!(
        read_kind("cat notes.md | grep todo"),
        Some(ReadKind::Search)
    );
    assert_eq!(
        read_kind("grep -rn x src | head -5"),
        Some(ReadKind::Search)
    );
    assert_eq!(read_kind("cat notes.md | wc -l"), Some(ReadKind::Read));
    assert_eq!(read_kind("ls src && cat README.md"), Some(ReadKind::List));
}

#[test]
fn neutral_commands_ride_along_but_never_make_a_read_alone() {
    assert_eq!(read_kind("echo --- && ls"), Some(ReadKind::List));
    assert_eq!(read_kind("cd src && ls -la"), Some(ReadKind::List));
    assert_eq!(
        read_kind("cat a.txt; echo; cat b.txt"),
        Some(ReadKind::Read)
    );
    for command in ["echo hi", "true", "printf 'x'", "cd src", ":"] {
        assert_eq!(read_kind(command), None, "{command}");
    }
}

#[test]
fn anything_that_is_not_reading_is_no_read() {
    for command in [
        "rm -rf build",
        "ls && rm notes.md",
        "git status",
        "python3 about.py",
        "npm test",
        "cargo build",
        "FOO=1 ls",
        "sudo cat /etc/shadow",
        "/bin/ls",
        "",
        "   ",
    ] {
        assert_eq!(read_kind(command), None, "{command:?}");
    }
}

#[test]
fn a_quoted_program_name_is_still_the_program() {
    assert_eq!(read_kind("\"ls\" -la"), Some(ReadKind::List));
    assert_eq!(read_kind("'cat' notes.md"), Some(ReadKind::Read));
}

#[test]
fn a_redirect_to_a_file_is_a_write_but_to_dev_null_is_not() {
    assert_eq!(read_kind("cat notes.md > copy.md"), None);
    assert_eq!(read_kind("grep x a >> log.txt"), None);
    assert_eq!(
        read_kind("grep -rn foo src 2>/dev/null"),
        Some(ReadKind::Search)
    );
    assert_eq!(read_kind("ls missing 2>&1"), Some(ReadKind::List));
    assert_eq!(
        read_kind("find / -name x 2> /dev/null"),
        Some(ReadKind::Search)
    );
    assert_eq!(read_kind("ls &>/dev/null"), Some(ReadKind::List));
}

#[test]
fn structure_a_read_cannot_see_through_is_no_read() {
    for command in [
        "ls &",
        "(ls)",
        "cat $(git ls-files)",
        "cat `ls`",
        "cat <(ls)",
        "cat 'unterminated",
        "ls |",
        "ls && && ls",
    ] {
        assert_eq!(read_kind(command), None, "{command}");
    }
}

#[test]
fn display_only_shell_features_still_read() {
    // A variable or a glob changes *which* files are read, never that it is
    // reading — the cell may collapse; the permission check is another matter.
    assert_eq!(read_kind("grep \"$PATTERN\" src"), Some(ReadKind::Search));
    assert_eq!(read_kind("ls *.rs"), Some(ReadKind::List));
    assert_eq!(read_kind("cat src/{a,b}.rs"), Some(ReadKind::Read));
}

#[test]
fn a_reading_program_with_a_writing_flag_is_no_read() {
    for command in [
        "find . -delete",
        "find . -name '*.tmp' -exec rm {} +",
        "find . -fprint out.txt",
        "fd -e tmp -x rm",
        "rg --pre ./decode.sh secret",
        "sort -o sorted.txt names.txt",
        "sort -uo sorted.txt names.txt",
        "sed -i 's/a/b/' notes.md",
        "tree -o tree.txt",
        "uniq names.txt deduped.txt",
        "awk '{ system(\"rm x\") }' f",
        "awk '{ print > \"out\" }' f",
        // A program the caller names runs: a hostname helper, a pager.
        "rg --hostname-bin=./helper foo",
        "rg --hostname-bin ./helper foo",
        "ag --pager=./helper foo",
        "ack --pager ./helper foo",
        "bat --pager=./helper notes.md",
        // A compiled magic file, a pager's log, bat's own cache.
        "file -C -m magic",
        "file -Cm magic",
        "file --compile -m magic",
        "less -o copy.txt notes.md",
        "less -Ocopy.txt notes.md",
        "less --log-file=copy.txt notes.md",
        "bat cache --build",
        // The writing flag inside a bundle, or attached to its value.
        "tree -aR -L 2",
        "tree -ao tree.txt",
        "fd -Hx rm",
        "fd -xrm",
        "fd -Xrm",
        // GNU getopt takes any unambiguous prefix of a long option.
        "sort --out=sorted.txt names.txt",
        "sort --outp=sorted.txt names.txt",
        "sort --compress-prog=./helper names.txt",
        "tree --output tree.txt",
    ] {
        assert_eq!(read_kind(command), None, "{command}");
    }
}

#[test]
fn the_flags_beside_a_writing_one_still_read() {
    for (command, kind) in [
        ("rg --pre-glob '*.gz' foo", ReadKind::Search),
        ("fd -e rs", ReadKind::Search),
        ("fd -H -t f", ReadKind::Search),
        ("tree -L 2 -I node_modules", ReadKind::List),
        ("tree --dirsfirst", ReadKind::List),
        ("file -m magic f", ReadKind::Read),
        ("less -R notes.md", ReadKind::Read),
        ("bat --paging=never notes.md", ReadKind::Read),
        ("sort --check names.txt", ReadKind::Read),
        ("sort -rn names.txt", ReadKind::Read),
    ] {
        assert_eq!(read_kind(command), Some(kind), "{command}");
    }
}

#[test]
fn every_line_of_a_multi_line_command_must_read() {
    assert_eq!(read_kind("ls src\ncat README.md"), Some(ReadKind::List));
    assert_eq!(read_kind("ls src\nrm README.md"), None);
}

// ===== is_read_only: provably only reads, so auto mode needs no classifier =====

#[test]
fn everyday_inspection_commands_are_read_only() {
    for command in [
        "ls -la",
        "ls",
        "cat README.md",
        "head -n 50 src/main.rs",
        "tail -n 20 server.log",
        "wc -l src/lib.rs",
        "grep -rn \"fn main\" src",
        "grep -A20 -n render src/ui/live.rs",
        // Quoting is resolved, so a single-quoted `$` is a character the
        // pattern matches, never an expansion (the reference refuses any `$`
        // because its parser cannot tell).
        "grep -n 'end$' notes.md",
        "rg -n foo",
        "rg --files",
        "find . -name '*.rs' -type f",
        "fd -e rs",
        "tree -L 2",
        "du -sh .",
        "stat Cargo.toml",
        "file logo.png",
        "diff a.txt b.txt",
        "sed -n '1,40p' src/app.rs",
        "sed -n '5p' notes.md",
        "jq '.name' package.json",
        "which cargo",
        "pwd",
        "echo hello && ls",
        "cd src && ls",
        "ls 2>/dev/null",
        "grep foo notes.md 2>&1",
        "cat names.txt | sort | uniq -c",
        "find . -name '*.rs' | xargs grep -n todo",
        "ps aux",
        "date +%s",
        "sha256sum Cargo.lock",
        "sort -n -k 2 data.txt",
        "uniq -c",
        "base64 -d blob.txt",
        "pgrep -f node",
        "node --version",
        "python3 --version",
        "grep -rn foo src",
        "cat README.md | head -20",
        "git log --oneline",
    ] {
        assert!(is_read_only(command), "{command}");
    }
}

#[test]
fn git_inspection_subcommands_are_read_only() {
    for command in [
        "git status",
        "git status --short",
        "git log --oneline -10",
        "git log -n 5 --stat",
        "git diff",
        "git diff HEAD~1 --stat",
        "git diff --cached -U0",
        "git show HEAD",
        "git branch -a",
        "git branch --show-current",
        "git branch -vv",
        "git tag -l",
        "git rev-parse --show-toplevel",
        "git ls-files",
        "git blame -L 10,20 src/main.rs",
        "git grep -n todo",
        "git remote -v",
        "git stash list",
        "git describe --tags",
        "git config --get user.email",
        "git log --oneline | head -5",
    ] {
        assert!(is_read_only(command), "{command}");
    }
}

#[test]
fn a_command_that_changes_anything_is_not_read_only() {
    for command in [
        "rm -rf /tmp/scratch",
        "mkdir build",
        "touch notes.md",
        "cp a b",
        "mv a b",
        "python3 about.py",
        "curl https://example.com",
        "npm install",
        "git push",
        "git commit -m wip",
        "git checkout main",
        "git branch new-feature",
        "git branch -D old",
        "git tag v1.0",
        "git stash",
        "tee out.txt",
        "printf 'x\\n'",
        "awk '{print}' notes.md",
        "less notes.md",
        "xargs rm",
        "find . | xargs cat",
    ] {
        assert!(!is_read_only(command), "{command}");
    }
}

#[test]
fn a_reading_program_given_a_writing_flag_is_not_read_only() {
    for command in [
        "find . -delete",
        "find . -exec cat {} +",
        "find . -execdir ls ;",
        "find . -fprint out.txt",
        "sort -o out.txt names.txt",
        "sed -i 's/a/b/' notes.md",
        "sed 's/a/b/' notes.md",
        "sed -n '/fn/p' main.rs",
        "sed -n '1,5p;w out' main.rs",
        "tree -o tree.txt",
        "fd -x rm",
        "rg --pre ./decode.sh secret",
        "uniq names.txt out.txt",
        "jq -f prog.jq data.json",
        "jq 'env' data.json",
        "date 0101120030",
        "ps auxe",
        "git diff --output=patch.diff",
        "git log --format=%H --output=x",
        "git grep -O vim todo",
        "git branch --abbrev 5",
    ] {
        assert!(!is_read_only(command), "{command}");
    }
}

#[test]
fn shell_constructs_the_check_cannot_see_through_are_not_read_only() {
    for command in [
        "ls > out.txt",
        "cat notes.md >> log.txt",
        "cat < notes.md",
        "echo $HOME",
        "cat $(ls)",
        "cat `ls`",
        "grep \"end$HOME\" notes.md",
        "ls *.rs",
        "cat src/{a,b}.rs",
        "ls &",
        "(ls)",
        "ls; rm notes.md",
        "ls\nrm notes.md",
        "ls # just looking",
        "cat a\\;b",
        "FOO=1 ls",
        "sudo ls",
        "timeout 5 cat notes.md",
        "cat 'unterminated",
        "cd /repo && git status",
        "git -C other status",
        "git -c core.pager=less log",
        "cat /proc/1/environ",
        "cat <<EOF\nhi\nEOF",
        "",
        "git",
    ] {
        assert!(!is_read_only(command), "{command:?}");
    }
}

#[test]
fn a_flag_needing_a_value_must_get_one() {
    assert!(is_read_only("grep -e foo notes.md"));
    assert!(
        !is_read_only("grep -e"),
        "a trailing value flag has nothing to read"
    );
    assert!(
        !is_read_only("grep -m x notes.md"),
        "a number flag must get a number"
    );
    assert!(
        !is_read_only("grep -e -v notes.md"),
        "a value that is itself a flag is refused"
    );
    assert!(is_read_only("git log --since=yesterday"));
    assert!(
        !is_read_only("git status --porcelain=v2"),
        "a flag that takes no value refuses one"
    );
}

#[test]
fn a_bundle_of_short_flags_must_be_all_plain_flags() {
    assert!(is_read_only("grep -rni todo src"));
    assert!(is_read_only("ls -lahR"));
    assert!(
        !is_read_only("grep -rf patterns.txt src"),
        "a value-taking flag inside a bundle is refused"
    );
    assert!(
        !is_read_only("grep -rQ todo src"),
        "an unknown flag is refused"
    );
}

#[test]
fn a_double_dash_ends_the_flags() {
    assert!(is_read_only("grep -n -- -v notes.md"));
    assert!(is_read_only("git log -- src/main.rs"));
}

#[test]
fn a_quoted_flag_is_still_the_flag_it_spells() {
    assert!(!is_read_only("sort \"-o\" out.txt names.txt"));
    assert!(!is_read_only("find . ''-delete"));
}

#[test]
fn redirect_edges_are_judged_by_their_target() {
    for command in [
        "echo a2>out.txt",
        "ls 2>&1 >out.txt",
        "ls >/dev/nullo",
        "ls 3>/dev/null",
        "cat notes.md | tee copy.md",
        "ls >| out.txt",
        "cat <<< text",
    ] {
        assert!(!is_read_only(command), "{command}");
    }
    assert!(is_read_only("ls missing 2>/dev/null"));
    assert!(is_read_only("ls missing >/dev/null 2>&1"));
    assert!(is_read_only("ls missing &>/dev/null"));
}

#[test]
fn what_a_reader_cannot_see_is_refused() {
    for command in [
        "ls -la \\\n",
        "ls\u{00A0}-la",
        "ls\u{0007}",
        "\"ls -la\"",
        "ls\u{200B}; rm x",
    ] {
        assert!(!is_read_only(command), "{command:?}");
    }
}

#[test]
fn finds_grouping_parens_are_only_words() {
    assert!(is_read_only(
        "find . \\( -name '*.rs' -o -name '*.toml' \\) -type f"
    ));
    assert_eq!(
        read_kind("find . \\( -name '*.rs' -o -name '*.toml' \\)"),
        Some(ReadKind::Search)
    );
}

#[test]
fn git_edges_follow_the_subcommand_rules() {
    for command in [
        "git log --oneline -n5",
        "git diff -U3 HEAD",
        "git branch --merged main",
        "git branch --contains abc123",
        "git tag -l 'v*'",
        "git stash show -p",
        "git remote show origin",
        "git --version",
    ] {
        assert!(is_read_only(command), "{command}");
    }
    for command in [
        "git reflog expire --all",
        "git remote add origin https://example.com/x.git",
        "git stash pop",
        "git config user.name me",
        "git worktree add ../x",
        "git branch -m old new",
        "git log --exec-path=/tmp",
    ] {
        assert!(!is_read_only(command), "{command}");
    }
}

#[test]
fn xargs_hands_only_to_reading_targets() {
    assert!(is_read_only("find . -name '*.md' | xargs -0 grep -l todo"));
    assert!(is_read_only("ls | xargs -I {} echo {}"));
    assert!(is_read_only("ls | xargs wc -l"));
    for command in [
        "ls | xargs -I {} cat {}",
        "ls | xargs -I x echo x",
        "ls | xargs -i echo",
        "ls | xargs sh -c 'rm $0'",
        "ls | xargs grep --pre ./run.sh x",
    ] {
        assert!(!is_read_only(command), "{command}");
    }
}

#[test]
fn a_line_break_after_a_join_only_continues_the_command() {
    assert_eq!(
        read_kind("ls src &&\n  cat README.md"),
        Some(ReadKind::List)
    );
    assert!(
        !is_read_only("ls src &&\n  cat README.md"),
        "any line break refuses the strict check"
    );
}

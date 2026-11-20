//! The idle text layout (`docs/text-layout.md`): the linker script the build
//! script writes from `layout/idle-functions.txt`, and — on Linux, where it
//! is used — that the binary this suite runs was really linked with it.
//!
//! The generator is the build script's own module, included here by path so
//! its rules are unit-tested like any other pure code.

#[path = "../layout/script.rs"]
mod script;

use script::{Group, SECTION, groups, linker_script, unsupported_linker};

/// The checked-in list.
const LIST: &str = include_str!("../layout/idle-functions.txt");

/// The groups' names and patterns, for comparing.
fn shape<'a>(groups: &'a [Group<'a>]) -> Vec<(&'a str, &'a [&'a str])> {
    groups
        .iter()
        .map(|group| (group.name, group.patterns.as_slice()))
        .collect()
}

#[test]
fn a_list_is_groups_of_patterns_with_comments_and_blanks_skipped() {
    let list = "# what an idle session runs\n\n[a configured session]\nmain\n  _ZN4core3fmt5write17h*  \n\n[with plugins]\nload\n";
    let parsed = groups(list).unwrap();
    assert_eq!(
        shape(&parsed),
        [
            (
                "a configured session",
                ["main", "_ZN4core3fmt5write17h*"].as_slice()
            ),
            ("with plugins", ["load"].as_slice()),
        ]
    );
}

#[test]
fn a_pattern_belongs_to_a_group() {
    // The groups are the order the linker lays functions out in, so a
    // pattern outside every group has no place to go.
    let error = groups("main\n[a group]\nother\n").unwrap_err();
    assert_eq!(error, "line 1: main comes before the first [group]");
    for bad in ["[]", "[Upper]", "[a;b]", "[open", "[a] trailing"] {
        let error = groups(&format!("{bad}\nmain\n")).unwrap_err();
        assert!(error.starts_with("line 1: "), "{bad:?} gave {error:?}");
    }
}

#[test]
fn a_line_that_is_not_a_symbol_pattern_is_refused_with_its_line_number() {
    // A linker script is a language: a quote, a brace, a space or a glob
    // class in a name would end the pattern and start something else.
    for bad in [
        "a b", "a\"b", "a}b", "a;b", "a[bc]", "a?b", "a\\b", "(a)", "a,b", "a/b", "a@b",
    ] {
        let list = format!("[g]\nmain\n{bad}\n");
        let error = groups(&list).unwrap_err();
        assert!(error.starts_with("line 3: "), "{bad:?} gave {error:?}");
    }
    // The characters mangled names are made of are all taken.
    groups("[g]\n_ZN4core3ptr42drop_in_place$LT$alloc..string..String$GT$17h*\n").unwrap();
    groups("[g]\n_RNvCs1234_7mycrate3foo\n").unwrap();
}

#[test]
fn a_pattern_listed_twice_is_refused_whichever_groups_it_is_in() {
    let error = groups("[one]\nmain\nother\n[two]\nmain\n").unwrap_err();
    assert_eq!(error, "line 5: main is listed twice (first on line 2)");
}

#[test]
fn the_script_gathers_each_group_s_functions_in_order_before_text() {
    let list = "[a configured session]\nmain\n_ZN4core3fmt5write17h*\n[with plugins]\nload\n";
    let script = linker_script(&groups(list).unwrap());
    // One output section, inserted ahead of the default `.text`, so the
    // default script still lays everything else out as it always did.
    assert!(script.contains(&format!("  {SECTION} :\n")), "{script}");
    assert!(
        script.trim_end().ends_with("INSERT BEFORE .text;"),
        "{script}"
    );
    // rustc gives every function a section of its own, `.text.{symbol}` —
    // `.text.unlikely.{symbol}` for one marked cold, which an idle session
    // runs all the same (a vector's first growth).
    assert!(
        script.contains("      \".text.main\" \".text.unlikely.main\"\n"),
        "{script}"
    );
    assert!(
        script.contains(
            "      \".text._ZN4core3fmt5write17h*\" \".text.unlikely._ZN4core3fmt5write17h*\"\n"
        ),
        "{script}"
    );
    // One input statement a group, in the list's order: a linker places a
    // statement's sections together, so each group is one run of code —
    // what every session runs first, what only some run after it — and it
    // walks its sections once per statement, so a statement per function
    // made the link slower by as many passes.
    assert_eq!(script.matches("*(").count(), 2, "{script}");
    let configured = script
        .find("/* a configured session */")
        .expect("first group");
    let plugins = script.find("/* with plugins */").expect("second group");
    let load = script
        .find("\".text.load\"")
        .expect("the second group's pattern");
    assert!(configured < script.find("\".text.main\"").unwrap());
    assert!(script.find("\".text.main\"").unwrap() < plugins);
    assert!(plugins < load, "{script}");
    assert_eq!(script.matches('{').count(), script.matches('}').count());
    assert_eq!(script.matches('(').count(), script.matches(')').count());
    assert_eq!(script.matches("SECTIONS").count(), 1);
    // A group with nothing in it writes nothing.
    let script = linker_script(&groups("[empty]\n[full]\nmain\n").unwrap());
    assert_eq!(script.matches("*(").count(), 1, "{script}");
    assert!(!script.contains("/* empty */"), "{script}");
}

#[test]
fn a_linker_that_cannot_read_the_script_is_told_from_the_flags() {
    let flags = |list: &[&str]| list.join("\x1f");
    // By name or by path, in either option's spelling, wherever it sits.
    for (given, linker) in [
        (vec!["-Clink-arg=-fuse-ld=mold"], "mold"),
        (vec!["-C", "link-arg=-fuse-ld=gold"], "gold"),
        (
            vec!["-Copt-level=3", "-Clink-arg=-fuse-ld=/usr/local/bin/mold"],
            "mold",
        ),
        (vec!["-Clink-arg=--ld-path=/usr/bin/ld.gold"], "gold"),
        (vec!["-Clink-args=-Wl,-z,now -fuse-ld=mold"], "mold"),
    ] {
        assert_eq!(
            unsupported_linker(&flags(&given)),
            Some(linker),
            "{given:?}"
        );
    }
    // The two that read it, and no flags at all.
    for given in [
        vec![],
        vec!["-Clink-arg=-fuse-ld=lld"],
        vec!["-Clink-arg=-fuse-ld=bfd"],
        vec!["--cap-lints=warn"],
        // A path that merely has the word in a directory name.
        vec!["-Clink-arg=-fuse-ld=/opt/goldilocks/bin/ld.lld"],
    ] {
        assert_eq!(unsupported_linker(&flags(&given)), None, "{given:?}");
    }
}

#[test]
fn the_checked_in_list_is_valid_sorted_and_names_no_instance_by_its_hash() {
    let listed = groups(LIST).unwrap_or_else(|error| panic!("idle-functions.txt: {error}"));
    // What a configured session runs comes first and is the bulk of it.
    let first = listed.first().expect("the list has a group");
    assert_eq!(first.name, "a configured session");
    assert!(
        first.patterns.len() > 500,
        "only {} functions in the first group",
        first.patterns.len()
    );
    for group in &listed {
        let mut sorted = group.patterns.clone();
        sorted.sort_unstable();
        assert_eq!(group.patterns, sorted, "[{}] is kept sorted", group.name);
        // A legacy-mangled Rust name ends in `17h{16 hex}E`, a hash that
        // moves with the compiler, the crate's version and the crate an
        // instance was made in. The list keeps the path and leaves the hash
        // to a `*`, or the next release's build would match nothing.
        for pattern in &group.patterns {
            if let Some(at) = pattern.rfind("17h") {
                let tail = &pattern[at + 3..];
                assert!(
                    !(tail.len() >= 16 && tail[..16].bytes().all(|b| b.is_ascii_hexdigit())),
                    "{pattern} names one build's hash"
                );
            }
        }
    }
}

/// The binary under test, as its ELF section headers describe it: each
/// section's name, address, size and flags.
#[cfg(target_os = "linux")]
fn sections(bytes: &[u8]) -> Vec<(String, u64, u64, u64)> {
    let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    assert_eq!(&bytes[..4], b"\x7fELF", "not an ELF file");
    assert_eq!(bytes[4], 2, "a 64-bit ELF file is expected");
    assert_eq!(bytes[5], 1, "a little-endian ELF file is expected");
    let table = usize::try_from(u64_at(0x28)).unwrap();
    let entry = usize::from(u16_at(0x3a));
    let count = usize::from(u16_at(0x3c));
    let names = usize::from(u16_at(0x3e));
    let header = |index: usize| table + index * entry;
    let strings = usize::try_from(u64_at(header(names) + 0x18)).unwrap();
    (0..count)
        .map(|index| {
            let at = header(index);
            let name_at = strings + usize::try_from(u32_at(at)).unwrap();
            let end = bytes[name_at..].iter().position(|b| *b == 0).unwrap();
            let name = String::from_utf8_lossy(&bytes[name_at..name_at + end]).to_string();
            (
                name,
                u64_at(at + 0x10),
                u64_at(at + 0x20),
                u64_at(at + 0x08),
            )
        })
        .collect()
}

/// The layout is real in the binary this suite runs: the section is there,
/// it is executable, it sits ahead of the rest of the code, and it holds a
/// plausible share of it. A list gone stale — a toolchain that mangles
/// names another way, a linker that ignored the script — shows up here as a
/// missing or near-empty section, where it would otherwise only show up as
/// megabytes of idle memory nobody was looking at.
#[cfg(target_os = "linux")]
#[test]
fn the_binary_is_linked_with_the_idle_functions_together() {
    // The build script says whether it laid this build out (it does not
    // under `ALTER_ZERO_TEXT_LAYOUT=0`, or for a linker it knows cannot
    // take the script).
    if option_env!("ALTER_ZERO_TEXT_LAYOUT_ACTIVE") != Some("1") {
        eprintln!("the idle text layout is off in this build: nothing to check");
        return;
    }
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_alter-zero")).expect("the binary reads");
    let sections = sections(&bytes);
    const SHF_EXECINSTR: u64 = 0x4;
    let &(_, idle_at, idle_size, idle_flags) = sections
        .iter()
        .find(|(name, ..)| name == SECTION)
        .unwrap_or_else(|| panic!("the binary has no {SECTION} section"));
    let &(_, text_at, text_size, _) = sections
        .iter()
        .find(|(name, ..)| name == ".text")
        .expect("the binary has a .text section");
    assert_ne!(idle_flags & SHF_EXECINSTR, 0, "{SECTION} is executable");
    assert!(
        idle_at < text_at,
        "{SECTION} ({idle_at:#x}) sits ahead of .text ({text_at:#x})"
    );
    // What an idle session runs is on the order of a megabyte, and the hash
    // wildcards bring about as much again along with it. Far less means the
    // patterns stopped matching; far more means one started matching
    // everything.
    let kib = idle_size / 1024;
    assert!(kib >= 512, "{SECTION} holds only {kib} KiB");
    assert!(
        idle_size < (idle_size + text_size) / 2,
        "{SECTION} holds {kib} KiB, over half of the code"
    );
}

//! The linker script behind the idle text layout (`docs/text-layout.md`).
//!
//! An idle session runs about a megabyte of this binary's code, and the
//! linker's default order scatters that megabyte across all of it. Resident
//! memory is counted in pages, and the kernel maps the sixteen pages around
//! each one touched, so the scatter decides how much of the code stays in
//! memory: eight megabytes of it, give or take a megabyte for every
//! unrelated change that moved a hot function. The script names the
//! functions an idle session runs — `layout/idle-functions.txt`, written by
//! `scripts/text_layout.sh` from a profile of one — and has the linker put
//! them side by side in an output section of their own, ahead of the rest.
//!
//! Pure, and included by path from two places: `build.rs`, which writes the
//! script into `OUT_DIR` and hands it to the linker, and
//! `tests/text_layout.rs`, which holds these rules to their word.

/// The output section the listed functions are gathered in. A name under
/// `.text` so every tool that tells code by its section's name still does.
pub const SECTION: &str = ".text.idle";

/// One group of the list: functions the linker lays out as one run of code.
///
/// The groups are scenarios, most common first. What a configured session
/// runs at rest is the first and the bulk of it; what only a first run adds,
/// and what a session with plugins adds, follow as runs of their own — so a
/// session keeps resident the groups it runs and not the ones it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group<'a> {
    /// The header's text, `[like this]` without the brackets.
    pub name: &'a str,
    /// Its patterns, in file order.
    pub patterns: Vec<&'a str>,
}

/// The groups a list names: `[a group's name]` on a line of its own opens
/// one, every line after it is one of its patterns; `#` comments and blank
/// lines are skipped, surrounding space dropped.
///
/// A pattern is a symbol's name as the object files spell it, with `*` for
/// what moves from build to build (a legacy-mangled Rust name's hash). The
/// characters allowed are exactly the ones such names are made of — letters,
/// digits, `_`, `$`, `.` — and `*`: the pattern lands inside a quoted string
/// of a linker script, so anything else could end it and start a statement
/// of its own. A group's name lands in a comment there: lowercase letters,
/// digits, spaces and `-`.
///
/// # Errors
/// The first line that is no header and no such pattern, that comes before
/// any group, or that repeats an earlier pattern (in whichever group), as
/// `line {n}: {why}`.
pub fn groups(list: &str) -> Result<Vec<Group<'_>>, String> {
    let mut out: Vec<Group<'_>> = Vec::new();
    let mut seen = std::collections::HashMap::new();
    for (index, line) in list.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let name = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
                .filter(|name| {
                    !name.is_empty()
                        && name.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ' ' | '-')
                        })
                })
                .ok_or_else(|| format!("line {number}: {line:?} is not a [group] header"))?;
            out.push(Group {
                name,
                patterns: Vec::new(),
            });
            continue;
        }
        let pattern = line;
        if let Some(bad) = pattern
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '*')))
        {
            return Err(format!(
                "line {number}: {pattern:?} is not a symbol pattern ({bad:?} cannot be in one)"
            ));
        }
        let Some(group) = out.last_mut() else {
            return Err(format!(
                "line {number}: {pattern} comes before the first [group]"
            ));
        };
        if let Some(first) = seen.insert(pattern, number) {
            return Err(format!(
                "line {number}: {pattern} is listed twice (first on line {first})"
            ));
        }
        group.patterns.push(pattern);
    }
    Ok(out)
}

/// The linker script that gathers `groups`' functions in [`SECTION`], one
/// group after another.
///
/// rustc (and `cc` for the C this binary links) gives every function an
/// input section of its own, `.text.{symbol}`, or `.text.unlikely.{symbol}`
/// for one marked cold — which an idle session still runs (a vector's first
/// growth is one). Each pattern takes both. A group is one input statement:
/// a linker places a statement's sections together, so each group is one
/// run of code, and it walks its input sections once per statement, so the
/// patterns of a group share theirs. `INSERT BEFORE .text` makes the script
/// an addition to the linker's default one instead of a replacement:
/// everything not named here is laid out exactly as it was. GNU ld and lld
/// both read it; a pattern that matches nothing is not an error in either.
#[must_use]
pub fn linker_script(groups: &[Group<'_>]) -> String {
    let mut script = String::from(
        "/* Written by build.rs from layout/idle-functions.txt: the functions an\n   \
         idle session runs, side by side (docs/text-layout.md). */\n\
         SECTIONS\n{\n",
    );
    script.push_str("  ");
    script.push_str(SECTION);
    script.push_str(" :\n  {\n");
    for group in groups.iter().filter(|group| !group.patterns.is_empty()) {
        script.push_str("    /* ");
        script.push_str(group.name);
        script.push_str(" */\n    *(\n");
        for pattern in &group.patterns {
            script.push_str("      \".text.");
            script.push_str(pattern);
            script.push_str("\" \".text.unlikely.");
            script.push_str(pattern);
            script.push_str("\"\n");
        }
        script.push_str("    )\n");
    }
    script.push_str("  }\n}\nINSERT BEFORE .text;\n");
    script
}

/// The linker the flags select by name, when it is one that cannot read the
/// script: `Some("gold")` or `Some("mold")`. `flags` is cargo's
/// `CARGO_ENCODED_RUSTFLAGS` — every flag rustc is run with, the
/// environment's or a config file's, separated by `0x1f`.
///
/// gold has no `INSERT` and mold reads no `SECTIONS` at all, so either
/// would fail the link on the script. Both are chosen with `-fuse-ld=` (or
/// clang's `--ld-path=`), by name or by path, inside a `-C link-arg`.
#[must_use]
pub fn unsupported_linker(flags: &str) -> Option<&'static str> {
    for flag in flags.split('\x1f') {
        let Some((_, linker)) = flag
            .split_once("-fuse-ld=")
            .or_else(|| flag.split_once("--ld-path="))
        else {
            continue;
        };
        let name = linker.rsplit('/').next().unwrap_or(linker);
        for unsupported in ["gold", "mold"] {
            if name.contains(unsupported) {
                return Some(unsupported);
            }
        }
    }
    None
}

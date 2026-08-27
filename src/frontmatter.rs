//! The `---`-delimited YAML frontmatter parse two features share: a
//! `SKILL.md` (`docs/skills.md`) and an agent definition
//! (`docs/subagents.md`) are both a small scalar block over a markdown body,
//! and both must load a file authored for another tool without refusing it
//! over a key we don't model.
//!
//! Deliberately small — plain, quoted and block (`|`/`>`) scalars plus YAML's
//! indented plain-scalar continuation, which is how a long `description:` is
//! actually written. Nested maps and sequences fold to a string on their key:
//! harmless, since every key that carries one is a key we ignore.

/// A file that could not be read, parsed, or validated — a `SKILL.md`
/// ([`crate::skills::SkillError`]) or an agent definition
/// ([`crate::subagents::AgentFileError`]). Collected rather than thrown: one
/// bad file must not cost a session the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    pub path: std::path::PathBuf,
    pub message: String,
}

/// The errors in `current` whose file `reported` has not already raised — the
/// per-turn rescan's toast filter.
///
/// A file that stops parsing has to say so: silence is what makes "the model
/// ignores my skill" and "I typo'd the frontmatter" read as two unrelated
/// problems. But the walks re-run every turn and a broken file stays broken,
/// so the startup toast's shape alone would put the same red row on every
/// turn for the rest of the session.
///
/// Keyed on the **path**, and the caller re-seeds its set from each rescan's
/// errors: a file that is fixed is forgotten, so breaking it again reports
/// again.
#[must_use]
pub fn unreported(
    reported: &std::collections::BTreeSet<std::path::PathBuf>,
    current: &[FileError],
) -> Vec<FileError> {
    current
        .iter()
        .filter(|error| !reported.contains(&error.path))
        .cloned()
        .collect()
}

/// Split a `---`-delimited frontmatter block off the front of `contents`,
/// returning it and the remaining body. `None` when the block never opens or
/// never closes — the caller decides whether that is an error.
#[must_use]
pub fn split(contents: &str) -> Option<(String, String)> {
    // A leading BOM/blank line must not hide the fence.
    let contents = contents.trim_start_matches('\u{feff}');
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body.push(line);
        } else {
            frontmatter.push(line);
        }
    }
    if !closed || frontmatter.is_empty() {
        return None;
    }
    Some((frontmatter.join("\n"), body.join("\n")))
}

/// The top-level `key: value` scalars of a frontmatter block, in order.
/// Comment lines (`#`) and blanks are skipped, so a file can document its own
/// optional keys in place — which is exactly what the seeded agent files do.
#[must_use]
pub fn scalars(frontmatter: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut fields = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty()
            || line.starts_with([' ', '\t'])
            || line.trim_start().starts_with('#')
        {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        // Gather this key's continuation: every following line that is
        // indented (blank lines inside the block included).
        let start = index;
        while index < lines.len()
            && (lines[index].starts_with([' ', '\t']) || lines[index].trim().is_empty())
        {
            index += 1;
        }
        let continuation = &lines[start..index];
        fields.push((key, scalar_value(rest.trim(), continuation)));
    }
    fields
}

/// The value of `key` among parsed [`scalars`], empty values read as absent —
/// a `description:` with nothing after it is a missing description, not a
/// blank one.
#[must_use]
pub fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// One key's value: the same-line scalar folded with any continuation lines.
fn scalar_value(inline: &str, continuation: &[&str]) -> String {
    let literal = inline.starts_with('|');
    if literal || inline.starts_with('>') {
        // A block scalar: the marker line carries nothing but the style.
        let joined: Vec<&str> = continuation
            .iter()
            .map(|line| line.trim())
            .filter(|line| !literal || !line.is_empty())
            .collect();
        return if literal {
            joined.join("\n")
        } else {
            fold(&joined)
        };
    }
    let inline = unquote(inline);
    if continuation.is_empty() {
        return inline;
    }
    let mut parts: Vec<&str> = Vec::new();
    if !inline.is_empty() {
        parts.push(inline.as_str());
    }
    parts.extend(
        continuation
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty()),
    );
    unquote(&fold(&parts))
}

/// Join lines with single spaces, collapsing runs of whitespace — YAML's
/// folded style, and codex's `sanitize_single_line`.
fn fold(parts: &[&str]) -> String {
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip a matching pair of surrounding quotes, undoubling `''` inside a
/// single-quoted scalar. Prose like `description: Deploy: to ECS` is left
/// exactly as written.
fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['\'', '"'] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            let inner = &value[1..value.len() - 1];
            return if quote == '\'' {
                inner.replace("''", "'")
            } else {
                inner.to_string()
            };
        }
    }
    value.to_string()
}

/// `value` cut to `max` characters, the last replaced by `…` when it was —
/// the listing budget's trim, shared for the same reason the parse is.
#[must_use]
pub fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = value.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_block_from_the_body() {
        let (front, body) = split("---\nname: x\n---\n# Title\n\ntext\n").expect("frontmatter");
        assert_eq!(front, "name: x");
        assert_eq!(body, "# Title\n\ntext");
    }

    #[test]
    fn an_unclosed_or_absent_block_is_none() {
        assert!(split("name: x\nbody").is_none());
        assert!(split("---\nname: x\nbody").is_none());
        assert!(split("---\n---\nbody").is_none(), "an empty block is none");
    }

    #[test]
    fn a_leading_bom_does_not_hide_the_fence() {
        assert!(split("\u{feff}---\nname: x\n---\nbody").is_some());
    }

    #[test]
    fn reads_scalars_in_order_skipping_comments_and_blanks() {
        let fields = scalars("name: x\n\n# a comment: not a field\nmodel: inherit");
        assert_eq!(
            fields,
            vec![
                ("name".to_string(), "x".to_string()),
                ("model".to_string(), "inherit".to_string()),
            ]
        );
    }

    #[test]
    fn folds_an_indented_continuation_into_one_line() {
        let fields = scalars("description: first\n  second\n  third\nname: x");
        assert_eq!(field(&fields, "description"), Some("first second third"));
        assert_eq!(field(&fields, "name"), Some("x"));
    }

    #[test]
    fn reads_block_scalars_both_ways() {
        let literal = scalars("body: |\n  one\n  two");
        assert_eq!(field(&literal, "body"), Some("one\ntwo"));
        let folded = scalars("body: >\n  one\n  two");
        assert_eq!(field(&folded, "body"), Some("one two"));
    }

    #[test]
    fn unquotes_and_keeps_colons_in_prose() {
        let fields = scalars("a: 'it''s'\nb: \"quoted\"\nc: Deploy: to ECS");
        assert_eq!(field(&fields, "a"), Some("it's"));
        assert_eq!(field(&fields, "b"), Some("quoted"));
        assert_eq!(field(&fields, "c"), Some("Deploy: to ECS"));
    }

    #[test]
    fn an_empty_value_reads_as_absent() {
        let fields = scalars("description:\nname: x");
        assert_eq!(field(&fields, "description"), None);
        assert_eq!(field(&fields, "missing"), None);
    }

    #[test]
    fn unreported_filters_the_paths_already_raised() {
        let errors = vec![
            FileError {
                path: "a.md".into(),
                message: "bad".into(),
            },
            FileError {
                path: "b.md".into(),
                message: "worse".into(),
            },
        ];
        let reported = std::collections::BTreeSet::from(["a.md".into()]);
        let fresh = unreported(&reported, &errors);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].path, std::path::Path::new("b.md"));
    }

    #[test]
    fn truncate_marks_the_cut_and_leaves_short_values_alone() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "he…");
        assert_eq!(truncate_chars("hello", 0), "");
    }
}

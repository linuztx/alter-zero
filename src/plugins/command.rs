//! Plugin slash commands: a prompt template's frontmatter, its name, and its
//! argument expansion.
//!
//! A command is a markdown file under a plugin's `commands/` directory; typing
//! `/plugin:name args` submits its body as a user turn with the arguments put
//! where the body asks for them. The grammar is Claude Code's, kept so a
//! command written for it reads the same here. See `docs/plugins.md`.

use std::path::{Component, Path};

use crate::frontmatter;

/// How long a description taken from the body's first line may be.
const BODY_DESCRIPTION_MAX_CHARS: usize = 120;

/// What the palette shows for a command, read from its file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandMeta {
    /// The frontmatter `description`, else the body's first non-empty line.
    pub description: String,
    /// `argument-hint`, as written — `[project-name]` stays text, though YAML
    /// would read it as a list.
    pub argument_hint: Option<String>,
    /// The command expects arguments: it has a hint, or its body uses
    /// `$ARGUMENTS` / `$1`…`$9`.
    pub takes_arguments: bool,
}

/// Read a command file into what the palette shows and the body to expand.
/// A file with no frontmatter is fine — Codex ships several — and is all
/// body.
#[must_use]
pub fn parse_command(contents: &str) -> (CommandMeta, String) {
    let (fields, body) = match frontmatter::split(contents) {
        Some((block, body)) => (frontmatter::scalars(&block), body),
        None => (Vec::new(), contents.to_string()),
    };
    let description = frontmatter::field(&fields, "description")
        .map_or_else(|| body_description(&body), |text| text.trim().to_string());
    let argument_hint = frontmatter::field(&fields, "argument-hint")
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(str::to_string);
    let takes_arguments = argument_hint.is_some() || has_placeholder(&body);
    (
        CommandMeta {
            description,
            argument_hint,
            takes_arguments,
        },
        body,
    )
}

/// The body's first non-empty line, its heading marks stripped, capped.
fn body_description(body: &str) -> String {
    let line = body
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    frontmatter::truncate_chars(line, BODY_DESCRIPTION_MAX_CHARS)
}

/// Does `body` name any argument placeholder [`expand_arguments`] fills?
fn has_placeholder(body: &str) -> bool {
    scan(body, None).1
}

/// The command name for the file at `relative` below a plugin's commands
/// directory: `db/migrate.md` under plugin `p` is `p:db:migrate`. `None` for
/// a file that is not `.md` or a path segment outside `[a-z0-9_.-]` once
/// lowercased, since a name the palette cannot show whole is not one a user
/// can type.
#[must_use]
pub fn command_name(plugin: &str, relative: &Path) -> Option<String> {
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        segments.push(part.to_str()?.to_ascii_lowercase());
    }
    let last = segments.last_mut()?;
    let stem = last.strip_suffix(".md")?.to_string();
    *last = stem;
    let usable = |segment: &String| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    if !segments.iter().all(usable) {
        return None;
    }
    Some(format!("{plugin}:{}", segments.join(":")))
}

/// `body` with the user's argument text put in place.
///
/// - `$ARGUMENTS` is the whole text, trimmed.
/// - `$ARGUMENTS[N]` is word `N`, counted from 0; `$1`…`$9` are words 1 to 9,
///   counted from 1 — one digit only, so `$10` is `$1` then `0`. Words are
///   split the way a shell splits them (`"a b" c` is two words); a word that
///   is not there is empty.
/// - `\$` is a literal `$`, so `\$1` stays `$1`.
/// - A body with no placeholder that is given non-blank text gets
///   `\n\nARGUMENTS: {text}` appended, so the words are never lost.
#[must_use]
pub fn expand_arguments(body: &str, args: &str) -> String {
    let args = args.trim();
    let words = shell_words(args);
    let (mut out, placed) = scan(body, Some((args, &words)));
    if !placed && !args.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(args);
    }
    out
}

/// One pass over `body` with each placeholder replaced from `args` (the
/// whole text and its words), or by nothing when `args` is `None` — which
/// only asks whether there is a placeholder at all. The second value says
/// whether one was found.
fn scan(body: &str, args: Option<(&str, &[String])>) -> (String, bool) {
    const ARGUMENTS: &str = "$ARGUMENTS";
    let word = |index: usize| {
        args.and_then(|(_, words)| words.get(index))
            .map_or("", String::as_str)
    };
    let all = args.map_or("", |(all, _)| all);
    let mut out = String::with_capacity(body.len());
    let mut placed = false;
    let mut index = 0;
    while let Some(next) = body[index..].chars().next() {
        let rest = &body[index..];
        if rest.starts_with("\\$") {
            out.push('$');
            index += 2;
            continue;
        }
        if let Some(after) = rest.strip_prefix(ARGUMENTS) {
            if let Some((number, length)) = bracket_index(after) {
                out.push_str(word(number));
                placed = true;
                index += ARGUMENTS.len() + length;
                continue;
            }
            if !after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                out.push_str(all);
                placed = true;
                index += ARGUMENTS.len();
                continue;
            }
        }
        if let Some(digit) = rest
            .strip_prefix('$')
            .and_then(|after| after.chars().next())
            .and_then(|c| c.to_digit(10))
            .filter(|digit| *digit >= 1)
        {
            out.push_str(word(digit as usize - 1));
            placed = true;
            index += 2;
            continue;
        }
        out.push(next);
        index += next.len_utf8();
    }
    (out, placed)
}

/// `[N]` at the front of `text`: the index and how many bytes it spans.
fn bracket_index(text: &str) -> Option<(usize, usize)> {
    let inner = text.strip_prefix('[')?;
    let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !inner[digits..].starts_with(']') {
        return None;
    }
    let number = inner[..digits].parse().ok()?;
    Some((number, digits + 2))
}

/// `text` split into words the way `sh` would: whitespace separates,
/// single quotes keep everything literal, double quotes keep whitespace and
/// let a backslash escape `"`, `\`, `$` and `` ` ``, and a backslash
/// outside quotes escapes the next character. An unclosed quote runs to the
/// end — a half-typed argument is still the user's text.
fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    current.push(inner);
                }
            }
            '"' => {
                in_word = true;
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(escaped @ ('"' | '\\' | '$' | '`')) => current.push(escaped),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => current.push('\\'),
                        },
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            other => {
                in_word = true;
                current.push(other);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// The command a typed draft invokes and the argument text after it.
///
/// The draft must start with `/`; its first whitespace-delimited word, minus
/// the slash, names a command in `names` exactly (case-insensitively), or is
/// the last `:`-segment of exactly one of them — so `/review` reaches
/// `code-review:review` until a second plugin also ships a `review`.
#[must_use]
pub fn match_invocation<'a>(draft: &'a str, names: &[&str]) -> Option<(String, &'a str)> {
    let text = draft.trim_start().strip_prefix('/')?;
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let (word, rest) = text.split_at(end);
    if word.is_empty() {
        return None;
    }
    let rest = rest.trim_start();
    if let Some(name) = names.iter().find(|name| name.eq_ignore_ascii_case(word)) {
        return Some(((*name).to_string(), rest));
    }
    let mut by_segment = names.iter().filter(|name| {
        name.rsplit(':')
            .next()
            .is_some_and(|last| last.eq_ignore_ascii_case(word))
    });
    let only = by_segment.next()?;
    if by_segment.next().is_some() {
        return None;
    }
    Some(((*only).to_string(), rest))
}

/// Does palette `query` (lowercase, no slash) pick `name`? It does when it
/// starts the whole name or any of its `:`-segments, so `mig` finds
/// `p:db:migrate`.
#[must_use]
pub fn matches_query(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with(query) || name.split(':').any(|segment| segment.starts_with(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_command ----------------------------------------------------

    #[test]
    fn frontmatter_description_and_hint_are_read() {
        let (meta, body) = parse_command(
            "---\ndescription: Create and setup a new app\nargument-hint: [project-name]\n---\n\nMake $ARGUMENTS.\n",
        );
        assert_eq!(meta.description, "Create and setup a new app");
        assert_eq!(meta.argument_hint.as_deref(), Some("[project-name]"));
        assert!(meta.takes_arguments);
        assert_eq!(body.trim(), "Make $ARGUMENTS.");
    }

    #[test]
    fn a_folded_description_is_one_line() {
        let (meta, _) =
            parse_command("---\ndescription: >\n  Build the macOS app\n  and run it\n---\nBody\n");
        assert_eq!(meta.description, "Build the macOS app and run it");
        assert!(!meta.takes_arguments);
    }

    #[test]
    fn a_file_without_frontmatter_is_all_body() {
        let text = "# Test the macOS app\n\nRun the tests with $1.\n";
        let (meta, body) = parse_command(text);
        assert_eq!(meta.description, "Test the macOS app");
        assert_eq!(meta.argument_hint, None);
        assert!(meta.takes_arguments);
        assert_eq!(body, text);
    }

    #[test]
    fn the_body_description_is_capped() {
        let long = "x".repeat(300);
        let (meta, _) = parse_command(&format!("\n\n## {long}\n"));
        assert_eq!(meta.description.chars().count(), 120);
        assert!(meta.description.ends_with('…'));
    }

    #[test]
    fn an_escaped_placeholder_does_not_take_arguments() {
        let (meta, _) = parse_command("Costs \\$5 and $ARGUMENTSX.\n");
        assert!(!meta.takes_arguments);
    }

    // --- command_name -----------------------------------------------------

    #[test]
    fn nested_commands_join_with_colons() {
        assert_eq!(
            command_name("p", Path::new("db/migrate.md")).as_deref(),
            Some("p:db:migrate")
        );
        assert_eq!(
            command_name("code-review", Path::new("Review.md")).as_deref(),
            Some("code-review:review")
        );
        assert_eq!(
            command_name("vercel", Path::new("_conventions.md")).as_deref(),
            Some("vercel:_conventions")
        );
    }

    #[test]
    fn unusable_command_paths_have_no_name() {
        assert_eq!(command_name("p", Path::new("notes.txt")), None);
        assert_eq!(command_name("p", Path::new("has space.md")), None);
        assert_eq!(command_name("p", Path::new("db:x/go.md")), None);
        assert_eq!(command_name("p", Path::new("../go.md")), None);
        assert_eq!(command_name("p", Path::new("/abs/go.md")), None);
        assert_eq!(command_name("p", Path::new(".md")), None);
    }

    // --- expand_arguments -------------------------------------------------

    #[test]
    fn arguments_is_the_whole_trimmed_text() {
        assert_eq!(
            expand_arguments("Do $ARGUMENTS now", "  a \"b c\"  "),
            "Do a \"b c\" now"
        );
    }

    #[test]
    fn positionals_are_one_based_and_shell_split() {
        assert_eq!(
            expand_arguments("[$1] [$2] [$3]", "\"a b\" c"),
            "[a b] [c] []"
        );
        assert_eq!(expand_arguments("'$1'", "it\\'s 'x y'"), "'it's'");
        assert_eq!(expand_arguments("$2", "it\\'s 'x y'"), "x y");
    }

    #[test]
    fn the_bracket_form_is_zero_based() {
        assert_eq!(
            expand_arguments("$ARGUMENTS[0]/$ARGUMENTS[1]/$ARGUMENTS[5]", "a b"),
            "a/b/"
        );
    }

    #[test]
    fn an_escaped_dollar_stays_literal() {
        assert_eq!(expand_arguments("\\$1 and $1", "x"), "$1 and x");
    }

    #[test]
    fn ten_is_one_then_zero() {
        assert_eq!(expand_arguments("$10", "a b"), "a0");
    }

    #[test]
    fn arguments_followed_by_a_word_character_is_not_a_placeholder() {
        assert_eq!(
            expand_arguments("$ARGUMENTSX", "go"),
            "$ARGUMENTSX\n\nARGUMENTS: go"
        );
        assert_eq!(expand_arguments("$0 $", "go"), "$0 $\n\nARGUMENTS: go");
    }

    #[test]
    fn no_placeholder_appends_the_arguments() {
        assert_eq!(
            expand_arguments("Review the code.", " src/lib.rs "),
            "Review the code.\n\nARGUMENTS: src/lib.rs"
        );
    }

    #[test]
    fn no_placeholder_and_blank_arguments_leave_the_body() {
        assert_eq!(expand_arguments("Review.", "   "), "Review.");
        assert_eq!(expand_arguments("Review \\$1.", ""), "Review $1.");
    }

    // --- match_invocation -------------------------------------------------

    #[test]
    fn a_unique_last_segment_matches() {
        let names = ["code-review:review", "p:db:migrate"];
        assert_eq!(
            match_invocation("/review   src/ main", &names),
            Some(("code-review:review".to_string(), "src/ main"))
        );
        assert_eq!(
            match_invocation("/migrate", &names),
            Some(("p:db:migrate".to_string(), ""))
        );
    }

    #[test]
    fn an_ambiguous_last_segment_does_not_match() {
        let names = ["code-review:review", "pr-tools:review"];
        assert_eq!(match_invocation("/review x", &names), None);
        assert_eq!(
            match_invocation("/PR-Tools:Review x", &names),
            Some(("pr-tools:review".to_string(), "x"))
        );
    }

    #[test]
    fn an_exact_full_name_always_wins() {
        let names = ["review", "code-review:review"];
        assert_eq!(
            match_invocation("/review\tnow", &names),
            Some(("review".to_string(), "now"))
        );
    }

    #[test]
    fn a_draft_that_is_not_a_command_does_not_match() {
        let names = ["p:go"];
        assert_eq!(match_invocation("go", &names), None);
        assert_eq!(match_invocation("/", &names), None);
        assert_eq!(match_invocation("/gone", &names), None);
        assert_eq!(match_invocation("/p:go", &[]), None);
    }

    // --- matches_query ----------------------------------------------------

    #[test]
    fn a_query_matches_the_name_or_any_segment_prefix() {
        assert!(matches_query("p:db:migrate", ""));
        assert!(matches_query("p:db:migrate", "p:d"));
        assert!(matches_query("p:db:migrate", "db"));
        assert!(matches_query("p:db:migrate", "mig"));
        assert!(matches_query("Code-Review:Review", "rev"));
        assert!(!matches_query("p:db:migrate", "grate"));
        assert!(!matches_query("p:db:migrate", "x"));
    }
}

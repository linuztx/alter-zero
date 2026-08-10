//! Which handlers an event's match query selects (`docs/hooks.md`).
//!
//! Both references share one implementation and so does this: absent / empty /
//! `*` matches everything; a matcher whose every character is in
//! `[A-Za-z0-9_|]` takes the **fast path** — split on `|`, trim, compare for
//! exact equality, no regex compiled; anything else is a regex.
//!
//! The fast path is not an optimization we invented, it is the contract:
//! exact equality means `bash` deliberately does **not** match a hypothetical
//! `bashoutput`, which a substring or prefix rule would.

/// Does `matcher` select `query`?
///
/// `None` (the field absent), an empty string, and `*` all match everything.
/// A matcher that is meant as a regex but does not compile matches nothing —
/// [`invalid_regex`] is how a caller detects that case to warn about it.
#[must_use]
pub fn matches(matcher: Option<&str>, query: &str) -> bool {
    let Some(pattern) = wildcard_free(matcher) else {
        return true;
    };
    if is_fast_path(pattern) {
        // No trimming: the fast-path character class excludes whitespace, so
        // an alternative can never carry any. `"write | edit"` therefore is
        // *not* an alternation at all — it leaves the class and is compiled
        // as a regex, in both references and here.
        return pattern.split('|').any(|alternative| alternative == query);
    }
    regex::Regex::new(pattern).is_ok_and(|re| re.is_match(query))
}

/// Is `matcher` a regex that will not compile? Selection warns about these
/// rather than letting a typo silently disable a guard the user believes is
/// running.
#[must_use]
pub fn invalid_regex(matcher: Option<&str>) -> bool {
    wildcard_free(matcher)
        .is_some_and(|pattern| !is_fast_path(pattern) && regex::Regex::new(pattern).is_err())
}

/// The matcher text when it actually constrains anything — `None` for the
/// three spellings of "match everything" (absent, empty, `*`), so both
/// entry points agree on what a wildcard is.
fn wildcard_free(matcher: Option<&str>) -> Option<&str> {
    matcher.filter(|pattern| !pattern.is_empty() && *pattern != "*")
}

/// The non-regex fast path's character class, verbatim from both references
/// (`/^[a-zA-Z0-9_|]+$/`): a plain name, or `|`-separated plain names.
fn is_fast_path(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'|')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_empty_or_star_matcher_matches_everything() {
        for matcher in [None, Some(""), Some("*")] {
            assert!(matches(matcher, "bash"), "{matcher:?} should match bash");
            assert!(matches(matcher, "anything"), "{matcher:?} matches all");
        }
    }

    #[test]
    fn a_plain_name_matches_exactly_and_not_by_prefix() {
        assert!(matches(Some("bash"), "bash"));
        // The whole point of exact equality: a longer tool name is NOT caught.
        assert!(!matches(Some("bash"), "bashoutput"));
        assert!(!matches(Some("bash"), "ba"));
        assert!(!matches(Some("bash"), "write"));
    }

    #[test]
    fn pipe_alternation_matches_each_alternative_exactly() {
        let m = Some("write|edit");
        assert!(matches(m, "write"));
        assert!(matches(m, "edit"));
        assert!(!matches(m, "bash"));
        assert!(!matches(m, "writeedit"));
    }

    #[test]
    fn anything_outside_the_fast_path_class_compiles_as_a_regex() {
        // `^…$` is the common Claude Code spelling and must work.
        assert!(matches(Some("^bash$"), "bash"));
        assert!(!matches(Some("^bash$"), "xbash"));
        assert!(matches(Some("^(write|edit)$"), "write"));
        assert!(matches(Some("bash.*"), "bashoutput"));
    }

    #[test]
    fn a_matcher_with_a_space_is_a_regex_not_an_alternation() {
        // A space is outside the fast-path class, so this is a regex — and as
        // a regex it is an unanchored substring search.
        assert!(matches(Some("write edit"), "a write edit b"));
    }

    #[test]
    fn an_uncompilable_regex_matches_nothing_and_is_reported() {
        assert!(!matches(Some("["), "bash"));
        assert!(invalid_regex(Some("[")));
    }

    #[test]
    fn valid_patterns_are_never_reported_as_invalid() {
        for matcher in [None, Some(""), Some("*"), Some("bash"), Some("^bash$")] {
            assert!(!invalid_regex(matcher), "{matcher:?} is valid");
        }
    }
}

//! A minimal `.env` reader/writer for persisting provider API keys
//! (docs/llm.md).
//!
//! The `/login` onboarding flow collects an API key and saves it here so it
//! survives across runs; the boundary (`main.rs`) loads the file at startup and
//! consults it during key resolution (a real process env var still wins). This
//! module is **pure** and unit-tested — the actual file read/write is the
//! boundary's job.
//!
//! The grammar is the common `.env` subset: `KEY=VALUE` lines, `#` comments,
//! blank lines, an optional `export ` prefix, and optional surrounding quotes
//! (double-quoted values honour `\"`/`\\`/`\n`/`\t` escapes; single-quoted are
//! verbatim). [`upsert`](EnvFile::upsert) rewrites one key **in place**, leaving
//! every other line — comments and all — untouched, so hand-edited files
//! round-trip.

/// A parsed `.env` file: the `KEY=VALUE` pairs, in file order. Comments and
/// blank lines are dropped from the in-memory view (they're only preserved by
/// the text-level [`upsert`](EnvFile::upsert)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFile {
    entries: Vec<(String, String)>,
}

impl EnvFile {
    /// Parse `.env` text into its key/value pairs. Malformed lines (no `=`, an
    /// invalid key) are skipped rather than erroring — a partial file still
    /// yields whatever keys it can.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let entries = text
            .lines()
            .filter_map(|line| split_key_value(line).map(|(k, v)| (k.to_string(), unquote(v))))
            .collect();
        Self { entries }
    }

    /// The value for `key`, or `None` when it isn't set. The **first** definition
    /// wins (matching [`upsert`](Self::upsert), which rewrites the first
    /// occurrence), so a duplicated key reads consistently.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Is `key` present with a non-empty value?
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| !v.is_empty())
    }

    /// Rewrite `text` so `key` maps to `value`, returning the new file contents.
    /// If `key` already has a line it is replaced **in place** (its position,
    /// and every other line, preserved); otherwise a `KEY=VALUE` line is
    /// appended. The result always ends with a newline. A value needing it
    /// (whitespace, `#`, quotes, empty) is double-quoted and escaped; a plain
    /// API key is written bare.
    #[must_use]
    pub fn upsert(text: &str, key: &str, value: &str) -> String {
        let serialized = if needs_quote(value) {
            quote(value)
        } else {
            value.to_string()
        };
        let new_line = format!("{key}={serialized}");

        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let mut replaced = false;
        for line in &mut lines {
            if split_key_value(line).map(|(k, _)| k) == Some(key) {
                *line = new_line.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            lines.push(new_line);
        }

        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

/// Split one line into `(key, raw_value)` when it is a `KEY=…` assignment (with
/// an optional `export ` prefix), else `None` for comments, blanks, and lines
/// whose key isn't a valid env-var name. `raw_value` is the unparsed remainder
/// after the first `=` (still quoted / whitespace-padded).
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let (key, value) = trimmed.split_once('=')?;
    let key = key.trim();
    if is_valid_key(key) {
        Some((key, value))
    } else {
        None
    }
}

/// A valid env-var name: a leading letter or `_`, then letters/digits/`_`.
fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Strip matching surrounding quotes from a raw value (trimming outer
/// whitespace first). Double-quoted values honour `\"`/`\\`/`\n`/`\t`; single
/// quotes are verbatim; unquoted values are returned trimmed.
fn unquote(raw: &str) -> String {
    let v = raw.trim();
    let bytes = v.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if first == b'"' && last == b'"' {
            return unescape_double(&v[1..v.len() - 1]);
        }
        if first == b'\'' && last == b'\'' {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

/// Unescape the inside of a double-quoted value (`\"`, `\\`, `\n`, `\t`; an
/// unknown escape keeps its backslash).
fn unescape_double(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Does `value` need to be quoted to survive a round-trip? (empty, or holding
/// whitespace / `#` / a quote character). Ordinary API keys don't.
fn needs_quote(value: &str) -> bool {
    value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'')
}

/// Wrap `value` in double quotes, escaping backslashes and double quotes.
fn quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_pairs() {
        let env = EnvFile::parse("OPENROUTER_API_KEY=sk-abc123\nFOO=bar\n");
        assert_eq!(env.get("OPENROUTER_API_KEY"), Some("sk-abc123"));
        assert_eq!(env.get("FOO"), Some("bar"));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let env = EnvFile::parse("# a comment\n\n   \nKEY=value\n# trailing\n");
        assert_eq!(env.get("KEY"), Some("value"));
        // The comment lines produced no entries.
        assert!(env.get("# a comment").is_none());
    }

    #[test]
    fn strips_an_export_prefix() {
        let env = EnvFile::parse("export SAMBANOVA_API_KEY=xyz\n");
        assert_eq!(env.get("SAMBANOVA_API_KEY"), Some("xyz"));
    }

    #[test]
    fn strips_surrounding_double_quotes_and_unescapes() {
        let env = EnvFile::parse("KEY=\"a \\\"quoted\\\" val\"\n");
        assert_eq!(env.get("KEY"), Some("a \"quoted\" val"));
    }

    #[test]
    fn single_quotes_are_verbatim() {
        let env = EnvFile::parse("KEY='no \\n escape'\n");
        assert_eq!(env.get("KEY"), Some("no \\n escape"));
    }

    #[test]
    fn a_value_with_an_equals_sign_keeps_everything_after_the_first() {
        // Base64-ish keys can end with '='.
        let env = EnvFile::parse("KEY=abc==\n");
        assert_eq!(env.get("KEY"), Some("abc=="));
    }

    #[test]
    fn invalid_keys_are_skipped() {
        let env = EnvFile::parse("1BAD=x\nno-equals-here\nGOOD=y\n");
        assert_eq!(env.get("GOOD"), Some("y"));
        assert!(env.get("1BAD").is_none());
    }

    #[test]
    fn has_reports_non_empty() {
        let env = EnvFile::parse("SET=v\nEMPTY=\n");
        assert!(env.has("SET"));
        assert!(!env.has("EMPTY"));
        assert!(!env.has("MISSING"));
    }

    #[test]
    fn get_uses_the_first_occurrence() {
        let env = EnvFile::parse("K=first\nK=second\n");
        assert_eq!(env.get("K"), Some("first"));
    }

    #[test]
    fn upsert_appends_a_new_key_with_a_trailing_newline() {
        let out = EnvFile::upsert("EXISTING=1\n", "OPENROUTER_API_KEY", "sk-new");
        assert_eq!(out, "EXISTING=1\nOPENROUTER_API_KEY=sk-new\n");
    }

    #[test]
    fn upsert_into_empty_text_writes_one_line() {
        let out = EnvFile::upsert("", "K", "v");
        assert_eq!(out, "K=v\n");
    }

    #[test]
    fn upsert_replaces_a_key_in_place_preserving_other_lines() {
        let original = "# header comment\nA=1\nOPENROUTER_API_KEY=old\nB=2\n";
        let out = EnvFile::upsert(original, "OPENROUTER_API_KEY", "new");
        assert_eq!(out, "# header comment\nA=1\nOPENROUTER_API_KEY=new\nB=2\n");
    }

    #[test]
    fn upsert_preserves_blank_lines_and_comments_on_append() {
        let original = "A=1\n\n# note\n";
        let out = EnvFile::upsert(original, "B", "2");
        assert_eq!(out, "A=1\n\n# note\nB=2\n");
    }

    #[test]
    fn upsert_quotes_a_value_that_needs_it() {
        let out = EnvFile::upsert("", "K", "has space");
        assert_eq!(out, "K=\"has space\"\n");
    }

    #[test]
    fn upsert_is_idempotent_and_round_trips_through_parse() {
        let out = EnvFile::upsert("", "OPENROUTER_API_KEY", "sk-round-trip");
        let reparsed = EnvFile::parse(&out);
        assert_eq!(reparsed.get("OPENROUTER_API_KEY"), Some("sk-round-trip"));
        // A second upsert of the same value is a no-op on the file.
        let again = EnvFile::upsert(&out, "OPENROUTER_API_KEY", "sk-round-trip");
        assert_eq!(again, out);
    }

    #[test]
    fn upsert_recognises_an_exported_key_line() {
        let out = EnvFile::upsert("export K=old\n", "K", "new");
        // The existing (exported) line is replaced, not duplicated.
        assert_eq!(out, "K=new\n");
    }
}

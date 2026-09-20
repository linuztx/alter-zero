//! A minimal `.env` reader/writer for persisting provider API keys
//! (docs/llm.md).
//!
//! The `/login` onboarding flow collects an API key and saves it here so it
//! survives across runs; the boundary (`main.rs`) loads the file at startup and
//! consults it during key resolution (a real process env var still wins). This
//! parser is pure; [`EnvFile::update`] is the shared atomic file boundary
//! used by both sign-in and background refreshes.
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
    /// Update a persisted provider credential, returning the new file text.
    /// Read, merge and replacement are serialized within the process so
    /// simultaneous provider refreshes preserve each other's values. A
    /// private temporary file is synced before atomically replacing the store.
    ///
    /// # Errors
    /// Returns file-system errors without replacing an unreadable store or
    /// truncating the existing file when writing its replacement fails.
    pub fn update(path: &std::path::Path, key: &str, value: &str) -> std::io::Result<String> {
        use std::io::{ErrorKind, Write};

        static UPDATES: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _update = UPDATES.lock().unwrap_or_else(|error| error.into_inner());
        let current = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let updated = Self::upsert(&current, key, value);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        // NamedTempFile creates with 0600 on Unix, before any secret bytes
        // are written. Replacement also tightens an older store's mode.
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(updated.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(updated)
    }

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
                // An exported line stays exported — the rewrite preserves the
                // file's shape, like the untouched comments around it.
                let exported = line.trim_start().starts_with("export ");
                *line = if exported {
                    format!("export {new_line}")
                } else {
                    new_line.clone()
                };
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

/// Unescape the inside of a double-quoted value (`\"`, `\\`, `\n`, `\t`, `\r`;
/// an unknown escape keeps its backslash).
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
                Some('r') => out.push('\r'),
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

/// Wrap `value` in double quotes, escaping backslashes, double quotes, and the
/// control characters [`unescape_double`] reads back (`\n`/`\t`/`\r`) — a
/// literal newline inside the quotes would tear the file into two
/// unparseable lines.
fn quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_cache_regression_updates_do_not_replace_an_unreadable_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let existing = [0xff, 0xfe, 0xfd];
        std::fs::write(&path, existing).unwrap();
        assert!(EnvFile::update(&path, "TOKEN", "new").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), existing);
    }

    #[test]
    fn auth_cache_regression_parallel_credential_updates_preserve_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "# existing comment\nUNCHANGED=keep\n").unwrap();
        let barrier = std::sync::Barrier::new(32);
        std::thread::scope(|scope| {
            for index in 0..32 {
                let path = &path;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    let key = format!("PROVIDER_{index}");
                    for revision in 0..8 {
                        EnvFile::update(path, &key, &format!("revision-{revision}")).unwrap();
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# existing comment\n"));
        let parsed = EnvFile::parse(&text);
        assert_eq!(parsed.get("UNCHANGED"), Some("keep"));
        for index in 0..32 {
            assert_eq!(parsed.get(&format!("PROVIDER_{index}")), Some("revision-7"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn auth_cache_regression_updated_credentials_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "TOKEN=old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let updated = EnvFile::update(&path, "TOKEN", "new").unwrap();
        assert_eq!(updated, "TOKEN=new\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

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
        // The existing exported line is replaced in place — keeping its
        // `export ` prefix, like every other preserved detail of the file.
        let out = EnvFile::upsert("export K=old\n", "K", "new");
        assert_eq!(out, "export K=new\n");
    }

    #[test]
    fn upsert_of_a_value_with_newlines_round_trips() {
        // quote() must escape control characters — a literal newline inside
        // the quotes tears the file into two unparseable lines.
        let out = EnvFile::upsert("", "K", "a\nb\tc\r");
        assert_eq!(out.lines().count(), 1, "one line, not a torn file: {out:?}");
        let env = EnvFile::parse(&out);
        assert_eq!(env.get("K"), Some("a\nb\tc\r"));
    }
}

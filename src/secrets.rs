//! Secrets — the user's credentials, used by the agent through placeholders
//! it can write but never read (`docs/secrets.md`).
//!
//! A secret is a **name**, a **value** and an optional line of **context**.
//! The model is told the names and the context in the `<system-reminder>`
//! ([`secret_section`]) and writes `<secret:NAME>` wherever a tool call
//! needs the credential; the executor swaps the placeholder for the value
//! just before the tool runs ([`SecretStore::expand`], [`expand_arguments`])
//! and swaps the value back for the placeholder in everything the tool
//! reports ([`SecretStore::redact`], [`StreamRedactor`]). So the value lives
//! in three places only — the owner-only `secrets.json`, the shared
//! [`SecretRegistry`] in memory, and the argument of the one tool call that
//! needs it — never in a message, a cell, the rollout or a request.
//!
//! Everything here is pure: the file I/O and the wiring into the executor
//! are the boundary's (`src/tui/secrets.rs`, `src/llm/secret_exec.rs`).

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A placeholder's opening, up to the name: `<secret:NAME>`.
pub const PLACEHOLDER_OPEN: &str = "<secret:";

/// A placeholder's closing character.
pub const PLACEHOLDER_CLOSE: char = '>';

/// The longest name a secret may have, in characters.
pub const MAX_SECRET_NAME_LEN: usize = 64;

/// The shortest value a secret may have, in characters. Redaction replaces
/// **every** exact occurrence of a value in tool output, so a one- or
/// two-character value would shred ordinary output (every `a`, every `12`)
/// while hiding nothing a credential needs hidden. It is also the shortest
/// piece of a value [`SecretStore::redact_cut_tail`] masks.
pub const MIN_SECRET_VALUE_CHARS: usize = 4;

/// Values at least this long also match across **one line break** between
/// two of their characters — a terminal's screen wraps a long token at its
/// width (120 columns for a tool's session), and a JWT split over two rows
/// must still be hidden. A shorter value only matches exactly: `12\n34` is
/// ordinary output, not a wrapped PIN.
pub const WRAP_MATCH_MIN_CHARS: usize = 12;

/// How many characters of a secret's context the reminder listing carries —
/// a line of context, not a document, since it rides every request.
pub const MAX_SECRET_CONTEXT_CHARS: usize = 200;

/// The file the secrets persist in, under the config home.
pub const SECRETS_FILE_NAME: &str = "secrets.json";

/// The secrets section's header in the `<system-reminder>` — what the
/// placeholders are for and how to use them, in as few words as say it. The
/// listing follows it ([`secret_section`]).
pub const SECRET_LISTING_HEADER: &str = "The user's secrets, as placeholders: write one verbatim in any tool call argument (a command, file content, typed input) and the tool gets the real value, inserted as-is, so quote it in shell commands. Output shows the placeholder wherever the exact value appears, but not an encoded or hashed form, so pass placeholders straight to what needs them. Use them whenever a task needs these credentials; the values are hidden from you on purpose, so never ask for or try to reveal them.";

/// A secret's value. Never printed: its `Debug` says `<redacted>`, it has no
/// `Display`, and the one way to read it is [`expose`](Self::expose), whose
/// callers are the expansion and the file writer.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    /// Wrap `value`.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself — for the expansion and the file writer only.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many characters the value holds — what a mask may show.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }

    /// Type one character (the `/secrets` form's value field).
    pub fn push(&mut self, c: char) {
        self.0.push(c);
    }

    /// Paste `text` in.
    pub fn push_str(&mut self, text: &str) {
        self.0.push_str(text);
    }

    /// Erase the last character.
    pub fn pop(&mut self) -> Option<char> {
        self.0.pop()
    }

    /// Erase everything.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// One stored secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    /// The name the placeholder carries — `[A-Z_][A-Z0-9_]*`.
    pub name: String,
    /// The value the placeholder stands for.
    pub value: SecretValue,
    /// A line telling the model what the credential is for; may be empty.
    pub context: String,
}

/// What the interface may know about a secret: everything **but** its
/// value. The `/secrets` page lists these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    /// The secret's name.
    pub name: String,
    /// Its context line; may be empty.
    pub context: String,
}

/// An add or an edit, as the `/secrets` form submits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretDraft {
    /// The name of the secret being edited — `None` for a new one. A draft
    /// whose `name` differs from it renames the secret.
    pub original: Option<String>,
    /// The name to store it under.
    pub name: String,
    /// The value — `None` keeps an edited secret's current value, so an
    /// edit never has to load the value back into the form.
    pub value: Option<SecretValue>,
    /// The context line.
    pub context: String,
}

/// Why a draft (or a name, or a value) was refused. Its `Display` is the
/// sentence the form shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No name was given.
    NameEmpty,
    /// The name is longer than [`MAX_SECRET_NAME_LEN`].
    NameTooLong,
    /// The name is not `[A-Z_][A-Z0-9_]*`.
    NameInvalid,
    /// Another secret already has this name.
    NameTaken(String),
    /// No value was given for a new secret.
    ValueEmpty,
    /// The value is shorter than [`MIN_SECRET_VALUE_CHARS`].
    ValueTooShort,
    /// The secret being edited no longer exists.
    NotFound(String),
}

impl SecretError {
    /// Whether the name field is at fault — where the form moves the focus.
    #[must_use]
    pub fn is_about_name(&self) -> bool {
        matches!(
            self,
            Self::NameEmpty | Self::NameTooLong | Self::NameInvalid | Self::NameTaken(_)
        )
    }
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameEmpty => f.write_str("Enter a name."),
            Self::NameTooLong => write!(f, "Names are at most {MAX_SECRET_NAME_LEN} characters."),
            Self::NameInvalid => {
                f.write_str("Names use A-Z, 0-9 and _, and don't start with a digit.")
            }
            Self::NameTaken(name) => write!(f, "{} already exists.", placeholder(name)),
            Self::ValueEmpty => f.write_str("Enter a value."),
            Self::ValueTooShort => write!(
                f,
                "Values need at least {MIN_SECRET_VALUE_CHARS} characters."
            ),
            Self::NotFound(name) => write!(f, "{} no longer exists.", placeholder(name)),
        }
    }
}

/// The placeholder for `name`: `<secret:NAME>`.
#[must_use]
pub fn placeholder(name: &str) -> String {
    format!("{PLACEHOLDER_OPEN}{name}{PLACEHOLDER_CLOSE}")
}

/// One typed character of a name, normalized: letters upper-cased, a
/// space, `-` or `.` becoming `_`, digits and `_` kept, anything else
/// dropped (`None`). The form applies it keystroke by keystroke, so typing
/// `root password` fills in `ROOT_PASSWORD`.
#[must_use]
pub fn normalize_name_char(c: char) -> Option<char> {
    match c {
        'a'..='z' => Some(c.to_ascii_uppercase()),
        'A'..='Z' | '0'..='9' | '_' => Some(c),
        ' ' | '-' | '.' => Some('_'),
        _ => None,
    }
}

/// [`normalize_name_char`] over a whole string — a paste into the name.
#[must_use]
pub fn normalize_name(raw: &str) -> String {
    raw.chars().filter_map(normalize_name_char).collect()
}

/// Check a name: non-empty, at most [`MAX_SECRET_NAME_LEN`] characters, and
/// `[A-Z_][A-Z0-9_]*` — an environment variable's shape, which is what a
/// credential's name usually already is.
///
/// # Errors
/// The first rule the name breaks.
pub fn validate_name(name: &str) -> Result<(), SecretError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(SecretError::NameEmpty);
    };
    if name.chars().count() > MAX_SECRET_NAME_LEN {
        return Err(SecretError::NameTooLong);
    }
    let starts = first.is_ascii_uppercase() || first == '_';
    let continues = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if starts && continues {
        Ok(())
    } else {
        Err(SecretError::NameInvalid)
    }
}

/// Check a value, surrounding whitespace aside (a stored value is trimmed):
/// non-empty and at least [`MIN_SECRET_VALUE_CHARS`] long.
///
/// # Errors
/// The first rule the value breaks.
pub fn validate_value(value: &str) -> Result<(), SecretError> {
    let value = value.trim();
    if value.is_empty() {
        Err(SecretError::ValueEmpty)
    } else if value.chars().count() < MIN_SECRET_VALUE_CHARS {
        Err(SecretError::ValueTooShort)
    } else {
        Ok(())
    }
}

/// Check a draft against the names already stored (`names`): its name, the
/// name not taken by a *different* secret, and its value — required for a
/// new secret, optional (kept) for an edit. What the form checks before it
/// submits and the store checks again when it applies the draft.
///
/// # Errors
/// The first rule the draft breaks.
pub fn validate_draft(draft: &SecretDraft, names: &[String]) -> Result<(), SecretError> {
    validate_name(&draft.name)?;
    let editing = draft.original.as_deref();
    if names
        .iter()
        .any(|name| *name == draft.name && Some(name.as_str()) != editing)
    {
        return Err(SecretError::NameTaken(draft.name.clone()));
    }
    match (&draft.value, editing) {
        (Some(value), _) => validate_value(value.expose()),
        (None, None) => Err(SecretError::ValueEmpty),
        (None, Some(_)) => Ok(()),
    }
}

/// Whether a tool's arguments get their placeholders expanded. The tools
/// that **act** — the shell family, the file tools, every MCP tool — do;
/// the tools whose arguments are shown to a person or handed to another
/// model (`agent`, `askuserquestion`, `skill`, the task tools) never do,
/// since expanding there would put the value on the screen or in a context.
#[must_use]
pub fn expands_placeholders(tool: &str) -> bool {
    use crate::llm::tools::{BASH_SEND_TOOL, BASH_SESSION_TOOL_NAME};
    matches!(tool, "bash" | "read" | "write" | "edit")
        || tool == BASH_SEND_TOOL
        || tool == BASH_SESSION_TOOL_NAME
        || crate::mcp::is_mcp_tool(tool)
}

/// A placeholder in some text: its byte range and the name as written.
struct Found<'t> {
    start: usize,
    end: usize,
    name: &'t str,
}

/// Every well-formed placeholder in `text`, left to right — an opener, a
/// name of ASCII letters, digits and `_`, the close. A near miss is skipped
/// one character at a time, so it never swallows a real one after it.
fn placeholders(text: &str) -> impl Iterator<Item = Found<'_>> {
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(offset) = text[from..].find('<') {
            let at = from + offset;
            from = at + 1;
            if !text[at..].starts_with(PLACEHOLDER_OPEN) {
                continue;
            }
            let start = at + PLACEHOLDER_OPEN.len();
            let len = text[start..]
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            let close = start + len;
            if len > 0 && text[close..].starts_with(PLACEHOLDER_CLOSE) {
                from = close + PLACEHOLDER_CLOSE.len_utf8();
                return Some(Found {
                    start: at,
                    end: from,
                    name: &text[start..close],
                });
            }
        }
        None
    })
}

/// How a value lines up against the text at a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// The whole value is there, ending at this byte.
    Whole(usize),
    /// The text ends partway through what could still be the value.
    Partial,
    /// It is not there.
    No,
}

/// Match `value` against `text` from byte `at` — one line break allowed
/// between two of its characters when `wraps` ([`WRAP_MATCH_MIN_CHARS`]).
fn fit(text: &[u8], at: usize, value: &[u8], wraps: bool) -> Fit {
    let (mut i, mut j) = (at, 0);
    let mut broke = false;
    while j < value.len() {
        let Some(&byte) = text.get(i) else {
            return Fit::Partial;
        };
        if byte == value[j] {
            i += 1;
            j += 1;
            broke = false;
            continue;
        }
        if wraps && j > 0 && !broke {
            if byte == b'\n' {
                i += 1;
                broke = true;
                continue;
            }
            if byte == b'\r' {
                match text.get(i + 1) {
                    Some(b'\n') => {
                        i += 2;
                        broke = true;
                        continue;
                    }
                    None => return Fit::Partial,
                    Some(_) => {}
                }
            }
        }
        return Fit::No;
    }
    Fit::Whole(i)
}

/// One redaction pass: the text up to `decided`, redacted, and — when the
/// pass was asked to hold — the name of the value that could still be
/// completing at `decided`.
struct Scanned<'t> {
    text: Cow<'t, str>,
    decided: usize,
    pending: Option<String>,
}

/// The secrets, in the order they were added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretStore {
    secrets: Vec<Secret>,
}

impl SecretStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every secret, in order.
    #[must_use]
    pub fn secrets(&self) -> &[Secret] {
        &self.secrets
    }

    /// Whether there are no secrets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// How many secrets there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    /// Every name, in order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.secrets.iter().map(|s| s.name.clone()).collect()
    }

    /// Every secret without its value — what the interface may see.
    #[must_use]
    pub fn metas(&self) -> Vec<SecretMeta> {
        self.secrets
            .iter()
            .map(|s| SecretMeta {
                name: s.name.clone(),
                context: s.context.clone(),
            })
            .collect()
    }

    /// Whether a secret named `name` exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.secrets.iter().any(|s| s.name == name)
    }

    /// The secret a placeholder names — the name matched case-insensitively.
    fn lookup(&self, name: &str) -> Option<&Secret> {
        self.secrets
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Apply an add or an edit ([`validate_draft`] first): a new secret is
    /// appended, an edited one keeps its place — renamed, re-valued and
    /// re-described as the draft says. The value is trimmed (a pasted value
    /// usually carries its line break). Returns the stored name.
    ///
    /// # Errors
    /// The draft's first problem, or [`SecretError::NotFound`] when the
    /// secret being edited is gone.
    pub fn apply(&mut self, draft: &SecretDraft) -> Result<String, SecretError> {
        validate_draft(draft, &self.names())?;
        let value = draft
            .value
            .as_ref()
            .map(|value| SecretValue::new(value.expose().trim()));
        let context = draft.context.trim().to_string();
        match &draft.original {
            None => {
                let Some(value) = value else {
                    return Err(SecretError::ValueEmpty);
                };
                self.secrets.push(Secret {
                    name: draft.name.clone(),
                    value,
                    context,
                });
            }
            Some(original) => {
                let Some(secret) = self.secrets.iter_mut().find(|s| s.name == *original) else {
                    return Err(SecretError::NotFound(original.clone()));
                };
                secret.name.clone_from(&draft.name);
                if let Some(value) = value {
                    secret.value = value;
                }
                secret.context = context;
            }
        }
        Ok(draft.name.clone())
    }

    /// Remove the secret named `name`; whether one was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.secrets.len();
        self.secrets.retain(|s| s.name != name);
        self.secrets.len() != before
    }

    /// Replace every `<secret:NAME>` naming a stored secret with its value
    /// — the name matched case-insensitively. A placeholder naming nothing
    /// stored is left as it is (the callers that run something refuse it
    /// first, [`refusal_for`](Self::refusal_for)), and an inserted value is
    /// never scanned again.
    #[must_use]
    pub fn expand<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.secrets.is_empty() || !text.contains(PLACEHOLDER_OPEN) {
            return Cow::Borrowed(text);
        }
        let mut out: Option<String> = None;
        let mut copied = 0;
        for found in placeholders(text) {
            if let Some(secret) = self.lookup(found.name) {
                let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
                buf.push_str(&text[copied..found.start]);
                buf.push_str(secret.value.expose());
                copied = found.end;
            }
        }
        match out {
            None => Cow::Borrowed(text),
            Some(mut buf) => {
                buf.push_str(&text[copied..]);
                Cow::Owned(buf)
            }
        }
    }

    /// The names in the `<secret:NAME>` placeholders of `text` that no
    /// stored secret answers to — each once, as written, in order. Nothing
    /// stored is nothing to check.
    #[must_use]
    pub fn unknown_placeholders(&self, text: &str) -> Vec<String> {
        let mut unknown: Vec<String> = Vec::new();
        if self.secrets.is_empty() || !text.contains(PLACEHOLDER_OPEN) {
            return unknown;
        }
        for found in placeholders(text) {
            if self.lookup(found.name).is_none() && !unknown.iter().any(|name| name == found.name) {
                unknown.push(found.name.to_string());
            }
        }
        unknown
    }

    /// What a call naming `unknown` placeholders is told instead of running:
    /// the names it wrote, the ones stored — so one step corrects a slip —
    /// and where a missing secret comes from, so the model sends the user
    /// to the page rather than asking for a value in the chat.
    #[must_use]
    pub fn unknown_refusal(&self, unknown: &[String]) -> String {
        let written = unknown
            .iter()
            .map(|name| placeholder(name))
            .collect::<Vec<_>>()
            .join(", ");
        let stored = self
            .secrets
            .iter()
            .map(|secret| placeholder(&secret.name))
            .collect::<Vec<_>>()
            .join(", ");
        let verdict = if unknown.len() == 1 {
            "is not a stored secret"
        } else {
            "are not stored secrets"
        };
        format!(
            "Not run: {written} {verdict}. Stored: {stored}. The user adds secrets with /secrets."
        )
    }

    /// The refusal ([`unknown_refusal`](Self::unknown_refusal)) a call owes
    /// when its `texts` name secrets that are not stored — each name once,
    /// across all of them — or `None` when every placeholder answers to one.
    #[must_use]
    pub fn refusal_for<'t>(&self, texts: impl IntoIterator<Item = &'t str>) -> Option<String> {
        let mut unknown: Vec<String> = Vec::new();
        for text in texts {
            for name in self.unknown_placeholders(text) {
                if !unknown.contains(&name) {
                    unknown.push(name);
                }
            }
        }
        (!unknown.is_empty()).then(|| self.unknown_refusal(&unknown))
    }

    /// [`expand`](Self::expand), refusing a placeholder that names nothing
    /// stored ([`refusal_for`](Self::refusal_for)) — for a command the user
    /// typed, which runs with no other check.
    ///
    /// # Errors
    /// The refusal, when `text` names a secret that is not stored.
    pub fn expand_checked<'a>(&self, text: &'a str) -> Result<Cow<'a, str>, String> {
        match self.refusal_for([text]) {
            Some(refusal) => Err(refusal),
            None => Ok(self.expand(text)),
        }
    }

    /// Replace every exact occurrence of a stored value with its
    /// placeholder. Where two values could match at one position the longer
    /// wins, so a value that contains another is hidden whole; a long value
    /// is also caught across a line break ([`WRAP_MATCH_MIN_CHARS`]); text
    /// holding no value comes back borrowed.
    #[must_use]
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        self.scan(text, false).text
    }

    /// [`redact`](Self::redact) for text whose **end was cut** — a
    /// truncated output. A cut through a value leaves its first part behind,
    /// which no whole-value match catches, so a trailing piece of at least
    /// [`MIN_SECRET_VALUE_CHARS`] characters that could begin a value is
    /// masked as that value's placeholder too.
    #[must_use]
    pub fn redact_cut_tail<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let scanned = self.scan(text, true);
        let Some(name) = scanned.pending else {
            return scanned.text;
        };
        let tail = &text[scanned.decided..];
        let mut out = scanned.text.into_owned();
        if tail.chars().count() >= MIN_SECRET_VALUE_CHARS {
            out.push_str(&placeholder(&name));
        } else {
            out.push_str(tail);
        }
        Cow::Owned(out)
    }

    /// The one greedy left-to-right pass behind every redaction. At each
    /// position the longest value that fits wins; with `hold`, a position
    /// where a longer value could still be completing stops the pass there
    /// (the streaming and cut-tail cases), so everything before it is
    /// decided exactly as the whole text would decide it.
    fn scan<'t>(&self, text: &'t str, hold: bool) -> Scanned<'t> {
        let mut order: Vec<&Secret> = self
            .secrets
            .iter()
            .filter(|s| !s.value.is_empty())
            .collect();
        if order.is_empty() {
            return Scanned {
                text: Cow::Borrowed(text),
                decided: text.len(),
                pending: None,
            };
        }
        order.sort_by_key(|s| std::cmp::Reverse(s.value.expose().len()));
        let bytes = text.as_bytes();
        let mut out: Option<String> = None;
        let mut copied = 0;
        let mut i = 0;
        while i < text.len() {
            let mut hit = None;
            for secret in &order {
                let value = secret.value.expose();
                let wraps = value.chars().count() >= WRAP_MATCH_MIN_CHARS;
                match fit(bytes, i, value.as_bytes(), wraps) {
                    Fit::Whole(end) => {
                        hit = Some((*secret, end));
                        break;
                    }
                    Fit::Partial if hold => {
                        return Scanned {
                            text: finish_scan(out, text, copied, i),
                            decided: i,
                            pending: Some(secret.name.clone()),
                        };
                    }
                    Fit::Partial | Fit::No => {}
                }
            }
            if let Some((secret, end)) = hit {
                let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
                buf.push_str(&text[copied..i]);
                buf.push_str(&placeholder(&secret.name));
                i = end;
                copied = end;
                continue;
            }
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        Scanned {
            text: finish_scan(out, text, copied, text.len()),
            decided: text.len(),
            pending: None,
        }
    }

    /// The reminder's listing: one `- <secret:NAME>: context` row per
    /// secret (`- <secret:NAME>` with no context), the context's whitespace
    /// collapsed and capped at [`MAX_SECRET_CONTEXT_CHARS`].
    #[must_use]
    pub fn listing(&self) -> String {
        self.secrets
            .iter()
            .map(|secret| {
                let context = secret
                    .context
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if context.is_empty() {
                    format!("- {}", placeholder(&secret.name))
                } else {
                    format!(
                        "- {}: {}",
                        placeholder(&secret.name),
                        cap_chars(&context, MAX_SECRET_CONTEXT_CHARS)
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The redacted text up to `end`: borrowed when nothing was replaced.
fn finish_scan<'t>(out: Option<String>, text: &'t str, copied: usize, end: usize) -> Cow<'t, str> {
    match out {
        None => Cow::Borrowed(&text[..end]),
        Some(mut buf) => {
            buf.push_str(&text[copied..end]);
            Cow::Owned(buf)
        }
    }
}

/// `text` cut to `max` characters, the last one an ellipsis when cut.
fn cap_chars(text: &str, max: usize) -> Cow<'_, str> {
    if text.chars().count() <= max {
        return Cow::Borrowed(text);
    }
    let mut capped: String = text.chars().take(max.saturating_sub(1)).collect();
    capped.push('…');
    Cow::Owned(capped)
}

/// The secrets section of the `<system-reminder>` ([`crate::reminder`]):
/// [`SECRET_LISTING_HEADER`] over the listing. Empty in, empty out — a
/// session with no secrets says nothing about them.
#[must_use]
pub fn secret_section(listing: &str) -> String {
    let listing = listing.trim();
    if listing.is_empty() {
        return String::new();
    }
    format!("{SECRET_LISTING_HEADER}\n\n{listing}")
}

/// A tool call's JSON `arguments` with the placeholders in every string
/// expanded ([`SecretStore::expand`]) — `None` when nothing changed, so the
/// call runs on the model's own text. Keys are left alone, and arguments
/// that are not JSON are the executor's to refuse.
///
/// # Errors
/// The [`check_arguments`] refusal: a placeholder names a secret that is
/// not stored, and nothing is expanded.
pub fn expand_arguments(store: &SecretStore, arguments: &str) -> Result<Option<String>, String> {
    // `secret:` survives a JSON-escaped `<` (`\u003c`); the parse decides.
    if store.is_empty() || !arguments.contains("secret:") {
        return Ok(None);
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Ok(None);
    };
    if let Some(refusal) = store.refusal_for(json_strings(&value)) {
        return Err(refusal);
    }
    if !expand_json(store, &mut value) {
        return Ok(None);
    }
    Ok(serde_json::to_string(&value).ok())
}

/// Refuse a tool call whose arguments name a secret that is not stored
/// ([`SecretStore::unknown_placeholders`]): run as written, a misspelled
/// placeholder would reach the tool as its literal text — a wrong password
/// typed at a prompt, an API called with it, a config file that looks
/// written. Every JSON **string** is checked, keys left alone; arguments
/// that are not JSON are the executor's to refuse.
///
/// # Errors
/// [`SecretStore::unknown_refusal`] for the names nothing answers to.
pub fn check_arguments(store: &SecretStore, arguments: &str) -> Result<(), String> {
    if store.is_empty() || !arguments.contains("secret:") {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Ok(());
    };
    match store.refusal_for(json_strings(&value)) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// Every string inside `value` — the arguments a placeholder can sit in,
/// keys left out.
fn json_strings(value: &serde_json::Value) -> Vec<&str> {
    fn walk<'v>(value: &'v serde_json::Value, out: &mut Vec<&'v str>) {
        match value {
            serde_json::Value::String(text) => out.push(text),
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            serde_json::Value::Object(fields) => fields.values().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

/// Expand every string inside `value`; whether any changed.
fn expand_json(store: &SecretStore, value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => match store.expand(text) {
            Cow::Borrowed(_) => false,
            Cow::Owned(expanded) => {
                *text = expanded;
                true
            }
        },
        serde_json::Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| expand_json(store, item) || changed),
        serde_json::Value::Object(fields) => fields
            .values_mut()
            .fold(false, |changed, item| expand_json(store, item) || changed),
        _ => false,
    }
}

/// Redaction over text that arrives in pieces — a running command's
/// output. A value split across two pieces is still caught: the tail that
/// could still grow into a value is **held** until the next piece decides
/// it, and everything before it is redacted exactly as
/// [`SecretStore::redact`] would redact the whole text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRedactor {
    held: String,
}

impl StreamRedactor {
    /// Take the next piece: the decided text, redacted. What could still be
    /// the start of a value stays held.
    pub fn push(&mut self, store: &SecretStore, chunk: &str) -> String {
        self.held.push_str(chunk);
        let scanned = store.scan(&self.held, true);
        let decided = scanned.decided;
        let out = scanned.text.into_owned();
        self.held.drain(..decided);
        out
    }

    /// The text held back — undecided, never yet emitted.
    #[must_use]
    pub fn held(&self) -> &str {
        &self.held
    }

    /// The stream ended: the held text, redacted.
    pub fn finish(&mut self, store: &SecretStore) -> String {
        let held = std::mem::take(&mut self.held);
        store.redact(&held).into_owned()
    }
}

/// `secrets.json` on disk: one record per secret.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretsFileFormat {
    #[serde(default)]
    secrets: Vec<SecretRecord>,
}

/// One secret as the file writes it.
#[derive(Debug, Serialize, Deserialize)]
struct SecretRecord {
    name: String,
    value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    context: String,
}

/// Parse `secrets.json`. Blank text is an empty store; a record with an
/// invalid name, a too-short value or a name already seen is skipped.
///
/// # Errors
/// The parse error's message, for text that is not the file's JSON.
pub fn parse_secrets_file(text: &str) -> Result<SecretStore, String> {
    if text.trim().is_empty() {
        return Ok(SecretStore::new());
    }
    let file: SecretsFileFormat = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut store = SecretStore::new();
    for record in file.secrets {
        if validate_name(&record.name).is_err()
            || validate_value(&record.value).is_err()
            || store.contains(&record.name)
        {
            continue;
        }
        store.secrets.push(Secret {
            name: record.name,
            value: SecretValue::new(record.value.trim()),
            context: record.context.trim().to_string(),
        });
    }
    Ok(store)
}

/// Format `store` as `secrets.json`: pretty JSON, a closing newline.
#[must_use]
pub fn format_secrets_file(store: &SecretStore) -> String {
    let file = SecretsFileFormat {
        secrets: store
            .secrets
            .iter()
            .map(|secret| SecretRecord {
                name: secret.name.clone(),
                value: secret.value.expose().to_string(),
                context: secret.context.clone(),
            })
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&file).unwrap_or_default();
    text.push('\n');
    text
}

/// The secrets, shared between the boundary that loads and saves them, the
/// executor that expands and redacts with them, and the page that lists
/// them — the [`crate::skills::SkillRegistry`] pattern: one lock, cloned by
/// handle. Its `Debug` counts the secrets and names none.
#[derive(Clone, Default)]
pub struct SecretRegistry {
    state: Arc<Mutex<SecretStore>>,
}

impl fmt::Debug for SecretRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.with(SecretStore::len);
        f.debug_struct("SecretRegistry")
            .field("secrets", &count)
            .finish()
    }
}

impl SecretRegistry {
    /// A registry holding `store`.
    #[must_use]
    pub fn new(store: SecretStore) -> Self {
        Self {
            state: Arc::new(Mutex::new(store)),
        }
    }

    /// Read the store under the lock.
    pub fn with<R>(&self, read: impl FnOnce(&SecretStore) -> R) -> R {
        let store = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(&store)
    }

    /// Swap the whole store — a load, or a save's result.
    pub fn replace(&self, store: SecretStore) {
        let mut current = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *current = store;
    }

    /// A copy of the store, values included — for the boundary's save.
    #[must_use]
    pub fn snapshot(&self) -> SecretStore {
        self.with(SecretStore::clone)
    }

    /// Whether there are no secrets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.with(SecretStore::is_empty)
    }

    /// [`SecretStore::metas`].
    #[must_use]
    pub fn metas(&self) -> Vec<SecretMeta> {
        self.with(SecretStore::metas)
    }

    /// [`SecretStore::listing`].
    #[must_use]
    pub fn listing(&self) -> String {
        self.with(SecretStore::listing)
    }

    /// [`SecretStore::redact`], owned.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        self.with(|store| store.redact(text).into_owned())
    }

    /// [`SecretStore::redact_cut_tail`], owned.
    #[must_use]
    pub fn redact_cut_tail(&self, text: &str) -> String {
        self.with(|store| store.redact_cut_tail(text).into_owned())
    }

    /// [`SecretStore::expand`], owned.
    #[must_use]
    pub fn expand(&self, text: &str) -> String {
        self.with(|store| store.expand(text).into_owned())
    }

    /// [`expand_arguments`] over the shared store.
    ///
    /// # Errors
    /// The refusal, when the arguments name a secret that is not stored.
    pub fn expand_arguments(&self, arguments: &str) -> Result<Option<String>, String> {
        self.with(|store| expand_arguments(store, arguments))
    }

    /// [`check_arguments`] over the shared store.
    ///
    /// # Errors
    /// The refusal, when the arguments name a secret that is not stored.
    pub fn check_arguments(&self, arguments: &str) -> Result<(), String> {
        self.with(|store| check_arguments(store, arguments))
    }

    /// [`SecretStore::expand_checked`], owned.
    ///
    /// # Errors
    /// The refusal, when `text` names a secret that is not stored.
    pub fn expand_checked(&self, text: &str) -> Result<String, String> {
        self.with(|store| store.expand_checked(text).map(Cow::into_owned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(name: &str, value: &str, context: &str) -> Secret {
        Secret {
            name: name.to_string(),
            value: SecretValue::new(value),
            context: context.to_string(),
        }
    }

    fn store(secrets: &[(&str, &str)]) -> SecretStore {
        SecretStore {
            secrets: secrets
                .iter()
                .map(|(name, value)| secret(name, value, ""))
                .collect(),
        }
    }

    fn draft(
        original: Option<&str>,
        name: &str,
        value: Option<&str>,
        context: &str,
    ) -> SecretDraft {
        SecretDraft {
            original: original.map(str::to_string),
            name: name.to_string(),
            value: value.map(SecretValue::new),
            context: context.to_string(),
        }
    }

    #[test]
    fn a_placeholder_wraps_the_name() {
        assert_eq!(placeholder("ROOT_PASSWORD"), "<secret:ROOT_PASSWORD>");
    }

    #[test]
    fn typed_names_normalize_to_an_environment_variables_shape() {
        assert_eq!(normalize_name("root password"), "ROOT_PASSWORD");
        assert_eq!(normalize_name("api-key.v2"), "API_KEY_V2");
        assert_eq!(normalize_name("A0_VENICE_API_KEY"), "A0_VENICE_API_KEY");
        // Anything else is dropped rather than guessed at.
        assert_eq!(normalize_name("tök€n!"), "TKN");
        assert_eq!(normalize_name_char('a'), Some('A'));
        assert_eq!(normalize_name_char(' '), Some('_'));
        assert_eq!(normalize_name_char('7'), Some('7'));
        assert_eq!(normalize_name_char('$'), None);
    }

    #[test]
    fn names_are_checked_against_the_variable_grammar() {
        for good in ["A", "_X", "ROOT_PASSWORD", "A0_VENICE_API_KEY"] {
            assert_eq!(validate_name(good), Ok(()), "{good}");
        }
        assert_eq!(validate_name(""), Err(SecretError::NameEmpty));
        assert_eq!(validate_name("1ABC"), Err(SecretError::NameInvalid));
        assert_eq!(validate_name("lower"), Err(SecretError::NameInvalid));
        assert_eq!(validate_name("HAS SPACE"), Err(SecretError::NameInvalid));
        let long = "A".repeat(MAX_SECRET_NAME_LEN + 1);
        assert_eq!(validate_name(&long), Err(SecretError::NameTooLong));
        assert_eq!(validate_name(&long[1..]), Ok(()));
    }

    #[test]
    fn values_must_be_long_enough_to_redact_safely() {
        assert_eq!(validate_value(""), Err(SecretError::ValueEmpty));
        assert_eq!(validate_value("abc"), Err(SecretError::ValueTooShort));
        assert_eq!(validate_value("abcd"), Ok(()));
        // Characters, not bytes.
        assert_eq!(validate_value("ééé"), Err(SecretError::ValueTooShort));
    }

    #[test]
    fn every_error_reads_as_a_sentence() {
        let messages = [
            SecretError::NameEmpty,
            SecretError::NameTooLong,
            SecretError::NameInvalid,
            SecretError::NameTaken("ROOT_PASSWORD".into()),
            SecretError::ValueEmpty,
            SecretError::ValueTooShort,
            SecretError::NotFound("OLD".into()),
        ]
        .map(|error| error.to_string());
        for message in &messages {
            assert!(!message.is_empty());
            assert!(message.ends_with('.'), "{message}");
        }
        assert!(
            messages[3].contains("<secret:ROOT_PASSWORD>"),
            "{}",
            messages[3]
        );
        assert!(messages[5].contains(&MIN_SECRET_VALUE_CHARS.to_string()));
    }

    #[test]
    fn a_new_secret_needs_a_free_name_and_a_value() {
        let names = vec!["TAKEN".to_string()];
        assert_eq!(
            validate_draft(&draft(None, "NEW", Some("hunter22"), ""), &names),
            Ok(())
        );
        assert_eq!(
            validate_draft(&draft(None, "TAKEN", Some("hunter22"), ""), &names),
            Err(SecretError::NameTaken("TAKEN".into()))
        );
        assert_eq!(
            validate_draft(&draft(None, "NEW", None, ""), &names),
            Err(SecretError::ValueEmpty)
        );
        assert_eq!(
            validate_draft(&draft(None, "NEW", Some("  "), ""), &names),
            Err(SecretError::ValueEmpty)
        );
        assert_eq!(
            validate_draft(&draft(None, "", Some("hunter22"), ""), &names),
            Err(SecretError::NameEmpty)
        );
    }

    #[test]
    fn an_edit_may_keep_its_value_and_its_own_name() {
        let names = vec!["KEEP".to_string(), "OTHER".to_string()];
        assert_eq!(
            validate_draft(&draft(Some("KEEP"), "KEEP", None, "new context"), &names),
            Ok(())
        );
        assert_eq!(
            validate_draft(&draft(Some("KEEP"), "RENAMED", None, ""), &names),
            Ok(())
        );
        assert_eq!(
            validate_draft(&draft(Some("KEEP"), "OTHER", None, ""), &names),
            Err(SecretError::NameTaken("OTHER".into()))
        );
        assert_eq!(
            validate_draft(&draft(Some("KEEP"), "KEEP", Some("ab"), ""), &names),
            Err(SecretError::ValueTooShort)
        );
    }

    #[test]
    fn applying_drafts_adds_edits_renames_and_trims() {
        let mut secrets = SecretStore::new();
        assert_eq!(
            secrets.apply(&draft(None, "FIRST", Some("  one-value\n"), " ctx ")),
            Ok("FIRST".to_string())
        );
        secrets
            .apply(&draft(None, "SECOND", Some("two-value"), ""))
            .unwrap();
        assert_eq!(secrets.names(), ["FIRST", "SECOND"]);
        assert_eq!(secrets.secrets()[0].value.expose(), "one-value");
        assert_eq!(secrets.secrets()[0].context, "ctx");

        // An edit without a value keeps the value, and keeps its place.
        secrets
            .apply(&draft(Some("FIRST"), "RENAMED", None, "about it"))
            .unwrap();
        assert_eq!(secrets.names(), ["RENAMED", "SECOND"]);
        assert_eq!(secrets.secrets()[0].value.expose(), "one-value");
        assert_eq!(secrets.secrets()[0].context, "about it");

        // A new value replaces the old one.
        secrets
            .apply(&draft(Some("SECOND"), "SECOND", Some("fresh-value"), ""))
            .unwrap();
        assert_eq!(secrets.secrets()[1].value.expose(), "fresh-value");

        assert_eq!(
            secrets.apply(&draft(Some("GONE"), "GONE", None, "")),
            Err(SecretError::NotFound("GONE".into()))
        );
        assert_eq!(
            secrets.apply(&draft(None, "SECOND", Some("again!"), "")),
            Err(SecretError::NameTaken("SECOND".into()))
        );
        assert!(secrets.contains("RENAMED"));
        assert!(!secrets.contains("FIRST"));
    }

    #[test]
    fn removing_a_secret_forgets_it() {
        let mut secrets = store(&[("A", "aaaa"), ("B", "bbbb")]);
        assert!(secrets.remove("A"));
        assert!(!secrets.remove("A"));
        assert_eq!(secrets.names(), ["B"]);
        assert_eq!(secrets.len(), 1);
    }

    #[test]
    fn metas_carry_everything_but_the_value() {
        let secrets = SecretStore {
            secrets: vec![secret("KEY", "sk-live-1234", "the API key")],
        };
        assert_eq!(
            secrets.metas(),
            [SecretMeta {
                name: "KEY".into(),
                context: "the API key".into(),
            }]
        );
    }

    #[test]
    fn expansion_swaps_known_placeholders_for_their_values() {
        let secrets = store(&[("ROOT_PASSWORD", "hunter22"), ("TOKEN", "tok-9876")]);
        assert_eq!(
            secrets.expand("echo <secret:ROOT_PASSWORD> | sudo -S true"),
            "echo hunter22 | sudo -S true"
        );
        // Several, adjacent, a lower-cased name.
        assert_eq!(
            secrets.expand("<secret:ROOT_PASSWORD><secret:TOKEN> <secret:token>"),
            "hunter22tok-9876 tok-9876"
        );
    }

    #[test]
    fn expansion_leaves_anything_it_cannot_resolve_alone() {
        let secrets = store(&[("TOKEN", "tok-9876")]);
        for text in [
            "no placeholders here",
            "<secrets:TOKEN>",
            "<secret:UNKNOWN>",
            "<secret:TOKEN",
            "<secret:>",
            "<secret: TOKEN>",
            "<secret:TO KEN>",
            "a < b > c",
        ] {
            assert_eq!(secrets.expand(text), text, "{text}");
            assert!(matches!(secrets.expand(text), Cow::Borrowed(_)), "{text}");
        }
        // A near miss before a real one does not swallow it.
        assert_eq!(secrets.expand("<secret:<secret:TOKEN>"), "<secret:tok-9876");
    }

    #[test]
    fn an_inserted_value_is_never_expanded_again() {
        let secrets = store(&[("A", "<secret:B>"), ("B", "bbbb")]);
        assert_eq!(secrets.expand("<secret:A>"), "<secret:B>");
    }

    #[test]
    fn redaction_swaps_every_value_for_its_placeholder() {
        let secrets = store(&[("ROOT_PASSWORD", "hunter22"), ("TOKEN", "tok-9876")]);
        assert_eq!(
            secrets.redact("pw=hunter22 token=tok-9876 again hunter22"),
            "pw=<secret:ROOT_PASSWORD> token=<secret:TOKEN> again <secret:ROOT_PASSWORD>"
        );
        let clean = "nothing secret";
        assert!(matches!(secrets.redact(clean), Cow::Borrowed(_)));
        assert!(matches!(SecretStore::new().redact(clean), Cow::Borrowed(_)));
    }

    #[test]
    fn redaction_prefers_the_longer_value_at_a_position() {
        // Order in the store must not matter.
        for pairs in [
            [("SHORT", "abcd"), ("LONG", "abcdefgh")],
            [("LONG", "abcdefgh"), ("SHORT", "abcd")],
        ] {
            let secrets = store(&pairs);
            assert_eq!(
                secrets.redact("abcdefgh abcd abcdxyz"),
                "<secret:LONG> <secret:SHORT> <secret:SHORT>xyz"
            );
        }
    }

    #[test]
    fn a_placeholder_emitted_by_redaction_is_never_redacted_into() {
        // A value that is a piece of another secret's placeholder must not
        // rewrite the placeholder that was just emitted.
        let secrets = store(&[("LONGNAME", "value-one"), ("X", "LONGNAME")]);
        assert_eq!(secrets.redact("value-one"), "<secret:LONGNAME>");
    }

    #[test]
    fn a_long_value_is_caught_across_a_terminals_row_break() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.payload";
        let secrets = store(&[("JWT", jwt)]);
        assert_eq!(
            secrets.redact("token: eyJhbGciOiJI\nUzI1NiJ9.payload done"),
            "token: <secret:JWT> done"
        );
        assert_eq!(
            secrets.redact("eyJhbGciOiJI\r\nUzI1NiJ9.payload"),
            "<secret:JWT>"
        );
        // One break between two characters — not a run of blank lines, and
        // never before the first character.
        assert_eq!(
            secrets.redact("eyJhbGciOiJI\n\nUzI1NiJ9.payload"),
            "eyJhbGciOiJI\n\nUzI1NiJ9.payload"
        );
        assert_eq!(
            secrets.redact("\neyJhbGciOiJIUzI1NiJ9.payload"),
            "\n<secret:JWT>"
        );
    }

    #[test]
    fn a_short_value_only_matches_exactly() {
        // `12\n34` is ordinary output, not a wrapped PIN.
        let secrets = store(&[("PIN", "1234")]);
        assert_eq!(secrets.redact("12\n34"), "12\n34");
        assert_eq!(secrets.redact("pin 1234"), "pin <secret:PIN>");
    }

    #[test]
    fn a_cut_tail_masks_the_piece_of_a_value_left_behind() {
        let secrets = store(&[("KEY", "sk-live-123456789")]);
        assert_eq!(
            secrets.redact_cut_tail("output sk-live-12"),
            "output <secret:KEY>"
        );
        // A whole value is redacted as usual; a piece under four characters
        // gives nothing away and is left.
        assert_eq!(
            secrets.redact_cut_tail("full sk-live-123456789"),
            "full <secret:KEY>"
        );
        assert_eq!(secrets.redact_cut_tail("ends sk-"), "ends sk-");
        assert_eq!(secrets.redact_cut_tail("no secret"), "no secret");
    }

    #[test]
    fn expansion_then_redaction_round_trips() {
        let secrets = store(&[("ROOT_PASSWORD", "hunter22")]);
        let command = "printf '%s' <secret:ROOT_PASSWORD>";
        let expanded = secrets.expand(command);
        assert_eq!(expanded, "printf '%s' hunter22");
        assert_eq!(secrets.redact(&expanded), command);
    }

    #[test]
    fn arguments_expand_inside_every_json_string() {
        let secrets = store(&[("PW", "p\"w\\x9")]);
        let arguments = r#"{"command":"login <secret:PW>","nested":{"list":["<secret:PW>",1,true]},"<secret:PW>":"key stays"}"#;
        let expanded = expand_arguments(&secrets, arguments).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();
        assert_eq!(value["command"], "login p\"w\\x9");
        assert_eq!(value["nested"]["list"][0], "p\"w\\x9");
        assert_eq!(value["nested"]["list"][1], 1);
        // Keys are not arguments.
        assert_eq!(value["<secret:PW>"], "key stays");
    }

    #[test]
    fn arguments_with_nothing_to_expand_are_left_to_the_model() {
        let secrets = store(&[("PW", "hunter22")]);
        assert_eq!(expand_arguments(&secrets, r#"{"command":"ls"}"#), Ok(None));
        assert_eq!(expand_arguments(&secrets, "not json <secret:PW>"), Ok(None));
        // Nothing stored is nothing to check: the text goes through as written.
        assert_eq!(
            expand_arguments(&SecretStore::new(), r#"{"c":"<secret:PW>"}"#),
            Ok(None)
        );
    }

    #[test]
    fn a_placeholder_naming_nothing_stored_refuses_the_arguments() {
        // A misspelled or guessed name would otherwise reach the tool as the
        // literal text — a password typed wrong, a `.env` line that looks
        // written — so the call is refused before anything runs.
        let secrets = store(&[("PW", "hunter22"), ("TOKEN", "tok-9876")]);
        for arguments in [
            r#"{"command":"echo <secret:NOPE>"}"#,
            r#"{"command":"login <secret:PW> <secret:NOPE>"}"#,
            r#"{"a":{"list":[1,"<secret:NOPE>"]}}"#,
        ] {
            let refusal = expand_arguments(&secrets, arguments).unwrap_err();
            assert!(refusal.contains("<secret:NOPE>"), "{refusal}");
            assert_eq!(check_arguments(&secrets, arguments), Err(refusal));
        }
        assert_eq!(
            check_arguments(&secrets, r#"{"input":"<secret:pw>\n"}"#),
            Ok(())
        );
        assert_eq!(check_arguments(&secrets, "not json <secret:NOPE>"), Ok(()));
        assert_eq!(
            check_arguments(&SecretStore::new(), r#"{"c":"<secret:NOPE>"}"#),
            Ok(())
        );
    }

    #[test]
    fn unknown_placeholders_are_named_once_each_as_written() {
        let secrets = store(&[("ROOT_PASSWORD", "hunter22")]);
        assert_eq!(
            secrets.unknown_placeholders(
                "<secret:ROOT_PASWORD> <secret:root_password> <secret:TOKEN> \
                 <secret:TOKEN> <secret:OTHER>"
            ),
            ["ROOT_PASWORD", "TOKEN", "OTHER"]
        );
        assert!(
            secrets
                .unknown_placeholders("<secret:ROOT_PASSWORD> <secret:> <secret:A B>")
                .is_empty()
        );
        assert!(
            SecretStore::new()
                .unknown_placeholders("<secret:ANY>")
                .is_empty()
        );
    }

    #[test]
    fn the_refusal_names_the_mistake_the_stored_secrets_and_where_they_come_from() {
        let secrets = store(&[("PW", "hunter22"), ("TOKEN", "tok-9876")]);
        assert_eq!(
            secrets.unknown_refusal(&["NOPE".to_string()]),
            "Not run: <secret:NOPE> is not a stored secret. Stored: <secret:PW>, \
             <secret:TOKEN>. The user adds secrets with /secrets."
        );
        assert_eq!(
            secrets.unknown_refusal(&["A".to_string(), "B".to_string()]),
            "Not run: <secret:A>, <secret:B> are not stored secrets. Stored: \
             <secret:PW>, <secret:TOKEN>. The user adds secrets with /secrets."
        );
    }

    #[test]
    fn one_refusal_covers_every_text_of_a_call() {
        let secrets = store(&[("PW", "hunter22")]);
        assert_eq!(secrets.refusal_for(["<secret:PW>", "plain"]), None);
        assert_eq!(
            secrets.refusal_for(["<secret:A>", "<secret:PW> <secret:A>", "<secret:B>"]),
            Some(secrets.unknown_refusal(&["A".to_string(), "B".to_string()]))
        );
    }

    #[test]
    fn a_checked_expansion_refuses_what_a_plain_one_leaves_alone() {
        let secrets = store(&[("PW", "hunter22")]);
        assert_eq!(
            secrets.expand_checked("echo <secret:PW>").as_deref(),
            Ok("echo hunter22")
        );
        let refusal = secrets.expand_checked("echo <secret:NOPE>").unwrap_err();
        assert!(refusal.contains("<secret:NOPE>"), "{refusal}");
        assert_eq!(
            SecretStore::new()
                .expand_checked("<secret:NOPE>")
                .as_deref(),
            Ok("<secret:NOPE>")
        );
    }

    #[test]
    fn only_the_tools_that_act_expand_placeholders() {
        for acting in [
            "bash",
            "bashsend",
            "bash_session",
            "read",
            "write",
            "edit",
            "mcp__github__create_issue",
        ] {
            assert!(expands_placeholders(acting), "{acting}");
        }
        for shown in [
            "agent",
            "askuserquestion",
            "skill",
            "taskcreate",
            "taskupdate",
            "bashlist",
            "unknown",
        ] {
            assert!(!expands_placeholders(shown), "{shown}");
        }
    }

    #[test]
    fn the_listing_names_each_placeholder_with_its_context() {
        let secrets = SecretStore {
            secrets: vec![
                secret(
                    "ROOT_PASSWORD",
                    "hunter22",
                    "Root password for\n  the staging box",
                ),
                secret("TOKEN", "tok-9876", ""),
            ],
        };
        assert_eq!(
            secrets.listing(),
            "- <secret:ROOT_PASSWORD>: Root password for the staging box\n- <secret:TOKEN>"
        );
        assert_eq!(SecretStore::new().listing(), "");
    }

    #[test]
    fn a_long_context_is_capped_in_the_listing() {
        let context = "x".repeat(MAX_SECRET_CONTEXT_CHARS + 50);
        let secrets = SecretStore {
            secrets: vec![secret("KEY", "hunter22", &context)],
        };
        let listing = secrets.listing();
        let shown = listing.strip_prefix("- <secret:KEY>: ").unwrap();
        assert_eq!(shown.chars().count(), MAX_SECRET_CONTEXT_CHARS);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn the_section_puts_the_header_over_the_listing() {
        assert_eq!(
            secret_section("- <secret:KEY>"),
            format!("{SECRET_LISTING_HEADER}\n\n- <secret:KEY>")
        );
        assert_eq!(secret_section(""), "");
        assert_eq!(secret_section(" \n"), "");
    }

    #[test]
    fn the_header_says_what_the_placeholders_are_for_and_stays_short() {
        assert!(SECRET_LISTING_HEADER.contains("placeholder"));
        assert!(SECRET_LISTING_HEADER.contains("tool"));
        // Masking is exact text: a model told only that output shows the
        // placeholder prints a Basic-auth header or a base64 of the value
        // believing it hidden.
        assert!(
            SECRET_LISTING_HEADER.contains("encoded or hashed"),
            "{SECRET_LISTING_HEADER}"
        );
        // It rides every request.
        let words = SECRET_LISTING_HEADER.split_whitespace().count();
        assert!(words <= 85, "{words} words");
    }

    #[test]
    fn a_value_split_across_pieces_is_still_redacted() {
        let secrets = store(&[("KEY", "sk-live-1234")]);
        let mut stream = StreamRedactor::default();
        let mut out = stream.push(&secrets, "token: sk-li");
        assert_eq!(out, "token: ");
        assert_eq!(stream.held(), "sk-li");
        out.push_str(&stream.push(&secrets, "ve-1234 done\n"));
        out.push_str(&stream.finish(&secrets));
        assert_eq!(out, "token: <secret:KEY> done\n");
        assert_eq!(stream.held(), "");
    }

    #[test]
    fn a_value_ending_inside_the_held_tail_is_redacted_whole() {
        // `1234` is complete, but its tail `34` could start `34ZZ`: the tail
        // is held and `1234` must still be caught.
        let secrets = store(&[("V", "1234"), ("W", "34ZZ")]);
        let mut stream = StreamRedactor::default();
        let mut out = stream.push(&secrets, "x1234");
        out.push_str(&stream.push(&secrets, " ok"));
        out.push_str(&stream.finish(&secrets));
        assert_eq!(out, "x<secret:V> ok");
    }

    #[test]
    fn a_longer_value_still_arriving_wins_over_a_shorter_complete_one() {
        let secrets = store(&[("SHORT", "1234"), ("LONG", "1234567")]);
        let mut stream = StreamRedactor::default();
        let mut out = stream.push(&secrets, "id 1234");
        out.push_str(&stream.push(&secrets, "567 end"));
        out.push_str(&stream.finish(&secrets));
        assert_eq!(out, "id <secret:LONG> end");
        // …and the shorter one when the longer never completes.
        let mut stream = StreamRedactor::default();
        let mut out = stream.push(&secrets, "id 1234");
        out.push_str(&stream.push(&secrets, "5 end"));
        out.push_str(&stream.finish(&secrets));
        assert_eq!(out, "id <secret:SHORT>5 end");
    }

    #[test]
    fn streaming_matches_whole_text_redaction_at_every_split() {
        let secrets = store(&[
            ("A", "abcd"),
            ("B", "abcdefgh"),
            ("C", "fgh!"),
            ("WRAPPED", "0123456789abcdef"),
        ]);
        let text = "zabcdefgh!abcdabcdfgh!abcdefg 01234567\n89abcdef 0123";
        let whole = secrets.redact(text).into_owned();
        assert!(whole.contains("<secret:WRAPPED>"), "{whole}");
        for split in 0..=text.len() {
            let mut stream = StreamRedactor::default();
            let mut out = stream.push(&secrets, &text[..split]);
            out.push_str(&stream.push(&secrets, &text[split..]));
            out.push_str(&stream.finish(&secrets));
            assert_eq!(out, whole, "split at {split}");
        }
    }

    #[test]
    fn the_file_round_trips() {
        let secrets = SecretStore {
            secrets: vec![
                secret("ROOT_PASSWORD", "hunter22", "Root password"),
                secret("TOKEN", "tok-9876", ""),
            ],
        };
        let text = format_secrets_file(&secrets);
        assert!(text.ends_with('\n'));
        assert_eq!(parse_secrets_file(&text), Ok(secrets));
    }

    #[test]
    fn the_file_skips_records_it_cannot_honour() {
        let text = r#"{"secrets": [
            {"name": "GOOD", "value": "hunter22"},
            {"name": "bad name", "value": "hunter22"},
            {"name": "SHORT", "value": "abc"},
            {"name": "GOOD", "value": "duplicate"},
            {"name": "CTX", "value": "valuevalue", "context": "why", "extra": 1}
        ]}"#;
        let secrets = parse_secrets_file(text).unwrap();
        assert_eq!(secrets.names(), ["GOOD", "CTX"]);
        assert_eq!(secrets.secrets()[1].context, "why");
        assert_eq!(parse_secrets_file(""), Ok(SecretStore::new()));
        assert_eq!(parse_secrets_file(" \n"), Ok(SecretStore::new()));
        assert!(parse_secrets_file("{not json").is_err());
    }

    #[test]
    fn nothing_debug_prints_a_value() {
        let secrets = SecretStore {
            secrets: vec![secret("KEY", "sk-live-1234", "ctx")],
        };
        let registry = SecretRegistry::new(secrets.clone());
        for printed in [
            format!("{:?}", secrets.secrets()[0].value),
            format!("{:?}", secrets.secrets()[0]),
            format!("{secrets:?}"),
            format!("{registry:?}"),
            format!("{:?}", draft(None, "KEY", Some("sk-live-1234"), "")),
        ] {
            assert!(!printed.contains("sk-live-1234"), "{printed}");
        }
    }

    #[test]
    fn the_registry_is_one_store_shared_by_handle() {
        let registry = SecretRegistry::default();
        let clone = registry.clone();
        assert!(clone.is_empty());
        registry.replace(store(&[("KEY", "sk-live-1234")]));
        assert!(!clone.is_empty());
        assert_eq!(clone.metas()[0].name, "KEY");
        assert_eq!(clone.redact("x sk-live-1234"), "x <secret:KEY>");
        assert_eq!(clone.expand("<secret:KEY>"), "sk-live-1234");
        assert_eq!(
            clone.expand_arguments(r#"{"c":"<secret:KEY>"}"#),
            Ok(Some(r#"{"c":"sk-live-1234"}"#.to_string()))
        );
        assert_eq!(
            clone.expand_checked("<secret:KEY>").as_deref(),
            Ok("sk-live-1234")
        );
        assert!(clone.expand_checked("<secret:NOPE>").is_err());
        assert!(clone.check_arguments(r#"{"c":"<secret:NOPE>"}"#).is_err());
        assert_eq!(clone.listing(), "- <secret:KEY>");
        assert_eq!(clone.snapshot().names(), ["KEY"]);
    }
}

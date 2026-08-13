//! Per-project trust for the `.alter-zero` project config layer
//! (`docs/project-config.md`).
//!
//! A project can check in `.alter-zero/hooks.json` and `.alter-zero/mcp.json`
//! (plus the Claude-Code-compat `.mcp.json`) — files whose contents *execute*:
//! hooks run shell commands, MCP stdio entries spawn processes. Cloning a
//! repository must not earn it code execution, so the project layer is
//! default-deny: nothing runs until the user approves it in `/trust`, and the
//! approval is pinned to the **content hash** of each file (codex's
//! `trusted_hash` answer) so an edited file is untrusted again.
//!
//! This module is the pure half: the fingerprint, the `trust.json` format
//! (`{"projects": {"/abs/root": {"files": {"/abs/file": "sha256:…"}}}}`),
//! its read-modify-write cores, and the project-file path builders. The
//! file I/O lives in the binary boundary (`tui`), like every other config
//! store. Unlike `permissions.json`'s best-effort parse, a malformed
//! `trust.json` is an **error** (fail closed, loud): a guard file that
//! silently trusts nothing the user approved — or worse, could be made to
//! trust something they didn't — must say so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The `sha256:<hex>` fingerprint of a config file's exact bytes — what an
/// approval records and a later load compares against.
#[must_use]
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The project's hooks file: `{root}/.alter-zero/hooks.json`.
#[must_use]
pub fn project_hooks_file(root: &Path) -> PathBuf {
    root.join(".alter-zero").join("hooks.json")
}

/// The project's MCP files in precedence order: the app-specific
/// `.alter-zero/mcp.json` first (the skills convention — `.alter-zero`
/// outranks the compat name), then the Claude-Code-compat `.mcp.json`.
#[must_use]
pub fn project_mcp_files(root: &Path) -> [PathBuf; 2] {
    [
        root.join(".alter-zero").join("mcp.json"),
        root.join(".mcp.json"),
    ]
}

/// One project's trusted files: absolute file path → recorded fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectEntry {
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// The parsed `trust.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustFile {
    #[serde(default)]
    projects: BTreeMap<String, ProjectEntry>,
}

impl TrustFile {
    /// Parse a `trust.json` body. An empty/whitespace file is an empty store.
    ///
    /// # Errors
    /// The serde message when the text is not valid JSON of this exact shape
    /// — including unknown fields, because a guard file with keys we don't
    /// model is a guard file we don't understand. The caller fails closed
    /// (nothing trusted) and surfaces the error.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(text).map_err(|err| err.to_string())
    }

    /// The recorded file → fingerprint map for `project` (empty when the
    /// project was never trusted).
    #[must_use]
    pub fn trusted_files(&self, project: &str) -> BTreeMap<String, String> {
        self.projects
            .get(project)
            .map(|entry| entry.files.clone())
            .unwrap_or_default()
    }

    /// Is `file` (at `fingerprint`) trusted for `project`? Only an exact
    /// fingerprint match answers yes — an edited file is untrusted again.
    #[must_use]
    pub fn is_trusted(&self, project: &str, file: &str, fingerprint: &str) -> bool {
        self.projects
            .get(project)
            .and_then(|entry| entry.files.get(file))
            .is_some_and(|recorded| recorded == fingerprint)
    }

    /// Does `project` have any trust recorded at all (the `/trust` menu's
    /// trusted/not-trusted headline)?
    #[must_use]
    pub fn has_project(&self, project: &str) -> bool {
        self.projects.contains_key(project)
    }
}

/// One project config file digested for the `/trust` review: what it is,
/// where it is, and — verbatim — what approving it would let run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustFileReview {
    /// The section heading: `Hooks`, `MCP servers`.
    pub label: String,
    /// The file's display path.
    pub path: String,
    /// What runs, one line each — a hook's `{event} ({matcher}): {command}`,
    /// a server's `{name}: {target}`. What you approve is exactly what you
    /// read.
    pub items: Vec<String>,
    /// The parse failure, when the file wouldn't parse (`items` empty then).
    pub error: Option<String>,
    /// Present but not trusted at this content — what the startup toast and
    /// the approval are about.
    pub pending: bool,
}

/// The `/trust` menu's whole snapshot — boundary-built
/// (the `open_hooks_menu` injection seam), pure to render and decide over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustReview {
    /// The project root's display path.
    pub root: String,
    /// Whether `trust.json` has this project at all — the headline.
    pub trusted: bool,
    /// The project config files found, in display order.
    pub files: Vec<TrustFileReview>,
}

/// The two things the `/trust` menu can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustAction {
    /// Record the reviewed files' fingerprints and activate them.
    Approve,
    /// Drop the project's trust entry and deactivate its config.
    Revoke,
}

impl TrustReview {
    /// Is anything present-but-unapproved (the startup toast's question)?
    #[must_use]
    pub fn pending(&self) -> bool {
        self.files.iter().any(|file| file.pending)
    }

    /// The menu's option rows, in display order: `Approve` when something
    /// reviewable awaits approval — a file that wouldn't parse is *not*
    /// approvable, because recording a hash you couldn't review is trust
    /// sight-unseen — and `Revoke` whenever trust is recorded.
    #[must_use]
    pub fn options(&self) -> Vec<TrustAction> {
        let mut out = Vec::new();
        if self
            .files
            .iter()
            .any(|file| file.pending && file.error.is_none())
        {
            out.push(TrustAction::Approve);
        }
        if self.trusted {
            out.push(TrustAction::Revoke);
        }
        out
    }
}

/// The `/trust` review's hook lines — every **runnable** handler, verbatim:
/// `{event} ({matcher}): {command}`, the matcher part dropped when the group
/// matches everything. Non-command handlers don't run here, so they don't
/// appear — what you approve is exactly what can execute.
#[must_use]
pub fn hooks_review_items(file: &crate::hooks::HooksFile) -> Vec<String> {
    let mut out = Vec::new();
    for (event, groups) in &file.hooks {
        for group in groups {
            let matcher = group
                .matcher
                .as_deref()
                .filter(|m| !m.trim().is_empty() && m.trim() != "*");
            for handler in &group.hooks {
                if handler.kind != crate::hooks::HandlerKind::Command {
                    continue;
                }
                let Some(command) = handler.command.as_deref().filter(|c| !c.trim().is_empty())
                else {
                    continue;
                };
                out.push(match matcher {
                    Some(matcher) => format!("{event} ({matcher}): {command}"),
                    None => format!("{event}: {command}"),
                });
            }
        }
    }
    out
}

/// The `/trust` review's MCP lines — `{name}: {target}` per declared server
/// (the command line, or the URL).
#[must_use]
pub fn mcp_review_items(file: &crate::mcp::McpFile) -> Vec<String> {
    file.servers
        .iter()
        .map(|(name, config)| format!("{name}: {}", config.target()))
        .collect()
}

/// Record `files` as `project`'s trusted set, **replacing** the project's
/// entry wholesale (an approval records the complete current set, so a file
/// that no longer exists doesn't linger trusted) and passing every other
/// project through untouched. An empty `files` drops the entry — the
/// skills-file convention, keeping the store a diff from nothing-trusted.
/// A malformed `existing` starts fresh: the load already reported it, and
/// there is nothing recoverable to preserve.
#[must_use]
pub fn record_trust(existing: &str, project: &str, files: &[(String, String)]) -> String {
    let parsed = TrustFile::parse(existing).unwrap_or_default();
    let mut projects: BTreeMap<String, BTreeMap<String, String>> = parsed
        .projects
        .into_iter()
        .map(|(name, entry)| (name, entry.files))
        .collect();
    if files.is_empty() {
        projects.remove(project);
    } else {
        projects.insert(project.to_string(), files.iter().cloned().collect());
    }
    serialize(&projects)
}

/// Drop `project`'s trust entry (the `/trust` revoke), passing every other
/// project through untouched.
#[must_use]
pub fn revoke_trust(existing: &str, project: &str) -> String {
    record_trust(existing, project, &[])
}

fn serialize(projects: &BTreeMap<String, BTreeMap<String, String>>) -> String {
    let mut root = serde_json::Map::new();
    let mut out = serde_json::Map::new();
    for (name, files) in projects {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "files".to_string(),
            serde_json::Value::Object(
                files
                    .iter()
                    .map(|(path, fp)| (path.clone(), serde_json::Value::String(fp.clone())))
                    .collect(),
            ),
        );
        out.insert(name.clone(), serde_json::Value::Object(entry));
    }
    root.insert("projects".to_string(), serde_json::Value::Object(out));
    serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        let a = fingerprint(b"hello");
        let b = fingerprint(b"hello");
        let c = fingerprint(b"hello!");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("sha256:"), "{a}");
        // A well-known vector, so the encoding can never silently change.
        assert_eq!(
            fingerprint(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_project_paths_hang_off_the_alter_zero_dir() {
        let root = Path::new("/repo");
        assert_eq!(
            project_hooks_file(root),
            PathBuf::from("/repo/.alter-zero/hooks.json")
        );
        let [preferred, compat] = project_mcp_files(root);
        assert_eq!(preferred, PathBuf::from("/repo/.alter-zero/mcp.json"));
        assert_eq!(compat, PathBuf::from("/repo/.mcp.json"));
    }

    #[test]
    fn an_empty_file_trusts_nothing() {
        let file = TrustFile::parse("").expect("empty file parses");
        assert!(!file.is_trusted("/repo", "/repo/.mcp.json", "sha256:aa"));
        assert!(!file.has_project("/repo"));
        assert!(file.trusted_files("/repo").is_empty());
    }

    #[test]
    fn a_recorded_file_is_trusted_until_its_content_changes() {
        let fp = fingerprint(b"{}");
        let text = record_trust("", "/repo", &[("/repo/.mcp.json".to_string(), fp.clone())]);
        let file = TrustFile::parse(&text).expect("recorded file parses");
        assert!(file.is_trusted("/repo", "/repo/.mcp.json", &fp));
        assert!(file.has_project("/repo"));
        // Edited content — a different fingerprint — is untrusted again.
        assert!(!file.is_trusted("/repo", "/repo/.mcp.json", &fingerprint(b"{ }")));
        // A file the project never recorded is untrusted.
        assert!(!file.is_trusted("/repo", "/repo/.alter-zero/hooks.json", &fp));
        // Another project sharing the same path is untrusted.
        assert!(!file.is_trusted("/other", "/repo/.mcp.json", &fp));
    }

    #[test]
    fn record_replaces_the_projects_entry_wholesale() {
        let first = record_trust(
            "",
            "/repo",
            &[
                ("/repo/.mcp.json".to_string(), "sha256:aa".to_string()),
                (
                    "/repo/.alter-zero/hooks.json".to_string(),
                    "sha256:bb".to_string(),
                ),
            ],
        );
        let second = record_trust(
            &first,
            "/repo",
            &[("/repo/.mcp.json".to_string(), "sha256:cc".to_string())],
        );
        let file = TrustFile::parse(&second).expect("re-recorded file parses");
        assert!(file.is_trusted("/repo", "/repo/.mcp.json", "sha256:cc"));
        // The hooks file dropped out of the recorded set — no longer trusted.
        assert!(!file.is_trusted("/repo", "/repo/.alter-zero/hooks.json", "sha256:bb"));
    }

    #[test]
    fn revoke_drops_only_the_named_project() {
        let one = record_trust(
            "",
            "/repo",
            &[("/repo/.mcp.json".to_string(), "sha256:aa".to_string())],
        );
        let two = record_trust(
            &one,
            "/other",
            &[("/other/.mcp.json".to_string(), "sha256:bb".to_string())],
        );
        let revoked = revoke_trust(&two, "/repo");
        let file = TrustFile::parse(&revoked).expect("revoked file parses");
        assert!(!file.has_project("/repo"));
        assert!(file.is_trusted("/other", "/other/.mcp.json", "sha256:bb"));
    }

    #[test]
    fn an_empty_record_drops_the_entry() {
        let one = record_trust(
            "",
            "/repo",
            &[("/repo/.mcp.json".to_string(), "sha256:aa".to_string())],
        );
        let dropped = record_trust(&one, "/repo", &[]);
        let file = TrustFile::parse(&dropped).expect("emptied file parses");
        assert!(!file.has_project("/repo"));
    }

    #[test]
    fn a_malformed_trust_file_is_an_error_not_silent_trust() {
        assert!(TrustFile::parse("not json").is_err());
        // Unknown fields are malformed too — a guard file with keys we don't
        // model is a guard file we don't understand.
        assert!(TrustFile::parse(r#"{"projects": {}, "extra": 1}"#).is_err());
        assert!(TrustFile::parse(r#"{"projects": {"/r": {"files": {}, "x": 1}}}"#).is_err());
    }

    fn review_file(pending: bool, error: Option<&str>) -> TrustFileReview {
        TrustFileReview {
            label: "Hooks".to_string(),
            path: "~/repo/.alter-zero/hooks.json".to_string(),
            items: vec!["Stop: ./fmt.sh".to_string()],
            error: error.map(str::to_string),
            pending,
        }
    }

    #[test]
    fn hooks_review_items_name_every_runnable_handler_verbatim() {
        let file = crate::hooks::HooksFile::parse(
            r#"{"hooks":{
                "PreToolUse":[
                    {"matcher":"bash","hooks":[
                        {"type":"command","command":"./guard.sh"},
                        {"type":"prompt"}
                    ]},
                    {"hooks":[{"type":"command","command":"./always.sh"}]}
                ],
                "Stop":[{"hooks":[{"type":"command","command":"./fmt.sh"}]}]
            }}"#,
        )
        .expect("fixture parses");
        assert_eq!(
            hooks_review_items(&file),
            vec![
                "PreToolUse (bash): ./guard.sh",
                "PreToolUse: ./always.sh",
                "Stop: ./fmt.sh",
            ]
        );
    }

    #[test]
    fn mcp_review_items_name_every_server_and_its_target() {
        let file = crate::mcp::parse_mcp_file(
            r#"{"mcpServers":{
                "docs": {"command": "npx", "args": ["-y", "docs-mcp"]},
                "wiki": {"url": "https://wiki.example/mcp"}
            }}"#,
        );
        assert_eq!(
            mcp_review_items(&file),
            vec!["docs: npx -y docs-mcp", "wiki: https://wiki.example/mcp"]
        );
    }

    #[test]
    fn the_options_follow_the_review_state() {
        // Nothing found, nothing recorded: nothing to do.
        let review = TrustReview {
            root: "~/repo".to_string(),
            trusted: false,
            files: Vec::new(),
        };
        assert_eq!(review.options(), Vec::<TrustAction>::new());
        assert!(!review.pending());
        // A fresh project layer offers the approval.
        let review = TrustReview {
            root: "~/repo".to_string(),
            trusted: false,
            files: vec![review_file(true, None)],
        };
        assert_eq!(review.options(), vec![TrustAction::Approve]);
        assert!(review.pending());
        // Trusted and unchanged: only the revoke.
        let review = TrustReview {
            root: "~/repo".to_string(),
            trusted: true,
            files: vec![review_file(false, None)],
        };
        assert_eq!(review.options(), vec![TrustAction::Revoke]);
        // Trusted but a file changed: both.
        let review = TrustReview {
            root: "~/repo".to_string(),
            trusted: true,
            files: vec![review_file(true, None)],
        };
        assert_eq!(
            review.options(),
            vec![TrustAction::Approve, TrustAction::Revoke]
        );
        // A file that won't parse is pending but not approvable — approving
        // a config you couldn't review would record a hash sight-unseen.
        let review = TrustReview {
            root: "~/repo".to_string(),
            trusted: false,
            files: vec![review_file(true, Some("bad json"))],
        };
        assert_eq!(review.options(), Vec::<TrustAction>::new());
        assert!(review.pending());
    }

    #[test]
    fn record_over_a_malformed_file_starts_fresh() {
        let text = record_trust(
            "not json",
            "/repo",
            &[("/repo/.mcp.json".to_string(), "sha256:aa".to_string())],
        );
        let file = TrustFile::parse(&text).expect("fresh file parses");
        assert!(file.is_trusted("/repo", "/repo/.mcp.json", "sha256:aa"));
    }
}

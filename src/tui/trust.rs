//! The project-config trust gate's boundary half (`docs/project-config.md`):
//! reading the project's `.alter-zero` layer at bootstrap (each file's bytes
//! fingerprinted and checked against `trust.json`), opening `/trust` over
//! that **reviewed snapshot**, and applying its decision — the `trust.json`
//! read-modify-write plus the live (de)activation: the hooks re-merge and
//! the MCP release/re-hold.

use std::path::{Path, PathBuf};

use alter_zero::app::ToastKind;
use alter_zero::trust::{self, TrustAction, TrustFileReview, TrustReview};
use alter_zero::{hooks, mcp};

use super::Session;
use super::config;

/// One project config file as loaded at bootstrap: the parse (or its error),
/// the content fingerprint of exactly those bytes, and the trust verdict.
#[derive(Debug, Clone)]
pub(crate) struct ProjectFile<T> {
    pub(crate) path: PathBuf,
    pub(crate) parsed: Result<T, String>,
    pub(crate) fingerprint: String,
    pub(crate) trusted: bool,
}

impl<T> ProjectFile<T> {
    /// Present but not trusted at this content — the pending state.
    pub(crate) const fn pending(&self) -> bool {
        !self.trusted
    }
}

/// The project's `.alter-zero` layer, as loaded once at bootstrap — the
/// snapshot `/trust` reviews and an approval records/activates (never a
/// re-read: what you approved is exactly what runs, and a file edited on
/// disk afterwards shows up pending again at the next launch).
#[derive(Debug, Clone)]
pub(crate) struct ProjectLayer {
    /// The nearest-`.git` project root (cwd when there is none) — the trust
    /// store's key.
    pub(crate) root: PathBuf,
    /// Any trust recorded for this project at all (the `/trust` headline).
    pub(crate) trusted: bool,
    /// `{root}/.alter-zero/hooks.json`, when present.
    pub(crate) hooks: Option<ProjectFile<hooks::HooksFile>>,
    /// `{root}/.alter-zero/mcp.json`, when present.
    pub(crate) mcp_alter: Option<ProjectFile<mcp::McpFile>>,
    /// `{root}/.mcp.json` (the Claude-Code-compat name), when present.
    pub(crate) mcp_compat: Option<ProjectFile<mcp::McpFile>>,
}

impl ProjectLayer {
    /// The project hooks file's parse, when it is trusted — what merges into
    /// the session's hooks.
    pub(crate) fn trusted_hooks(&self) -> Option<&hooks::HooksFile> {
        self.hooks
            .as_ref()
            .filter(|file| file.trusted)
            .and_then(|file| file.parsed.as_ref().ok())
    }

    /// Is anything present but unapproved (the startup toast's question)?
    pub(crate) fn pending(&self) -> bool {
        self.hooks.as_ref().is_some_and(ProjectFile::pending)
            || self.mcp_alter.as_ref().is_some_and(ProjectFile::pending)
            || self.mcp_compat.as_ref().is_some_and(ProjectFile::pending)
    }

    /// The parseable files an approval records: `(path, fingerprint)` of the
    /// loaded snapshot. A file that wouldn't parse is not recordable — trust
    /// sight-unseen (`docs/project-config.md`).
    fn approvable(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(file) = &self.hooks
            && file.parsed.is_ok()
        {
            out.push((file.path.display().to_string(), file.fingerprint.clone()));
        }
        for file in [&self.mcp_alter, &self.mcp_compat].into_iter().flatten() {
            if file.parsed.is_ok() {
                out.push((file.path.display().to_string(), file.fingerprint.clone()));
            }
        }
        out
    }

    /// Flip every loaded file's trust verdict (an approval or a revoke).
    fn set_trusted(&mut self, trusted: bool) {
        self.trusted = trusted;
        if let Some(file) = &mut self.hooks {
            file.trusted = trusted && file.parsed.is_ok();
        }
        for file in [&mut self.mcp_alter, &mut self.mcp_compat]
            .into_iter()
            .flatten()
        {
            file.trusted = trusted && file.parsed.is_ok();
        }
    }
}

/// Read one project config file: bytes → fingerprint → parse. `None` when
/// absent (the normal case) or unreadable-as-text (surfaced as a parse
/// error would be — the loud posture).
fn load_file<T>(
    path: PathBuf,
    parse: impl Fn(&str) -> Result<T, String>,
    trust_file: &trust::TrustFile,
    project: &str,
) -> Option<ProjectFile<T>> {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            let fingerprint = String::new();
            return Some(ProjectFile {
                parsed: Err(err.to_string()),
                fingerprint,
                trusted: false,
                path,
            });
        }
    };
    let fingerprint = trust::fingerprint(text.as_bytes());
    let trusted = trust_file.is_trusted(project, &path.display().to_string(), &fingerprint);
    Some(ProjectFile {
        parsed: parse(&text),
        fingerprint,
        trusted,
        path,
    })
}

/// Gather the project layer at bootstrap: resolve the root, load
/// `trust.json` (fail-closed and loud on a malformed one), read +
/// fingerprint + verdict each project file. `None` when
/// `ALTER_ZERO_PROJECT_CONFIG` is off. The second return is the trust-store
/// error to toast, if any.
pub(crate) fn load_project_layer(cwd: &Path) -> (Option<ProjectLayer>, Option<String>) {
    if !config::project_config_enabled() {
        return (None, None);
    }
    let root = alter_zero::project_doc::find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let (trust_file, trust_error) = config::load_trust_file(config::trust_json_path().as_deref());
    let project = root.display().to_string();
    let hooks_path = trust::project_hooks_file(&root);
    let [mcp_alter_path, mcp_compat_path] = trust::project_mcp_files(&root);
    let parse_mcp = |text: &str| -> Result<mcp::McpFile, String> {
        let file = mcp::parse_mcp_file(text);
        // A document-level failure parses to a lone error and no servers —
        // that is a broken file, not an empty one; per-entry errors keep the
        // healthy entries (the mcp module's leniently-but-loudly posture).
        match (&file.servers.is_empty(), file.errors.first()) {
            (true, Some(error)) => Err(error.clone()),
            _ => Ok(file),
        }
    };
    let layer = ProjectLayer {
        trusted: trust_file.has_project(&project),
        hooks: load_file(hooks_path, hooks::HooksFile::parse, &trust_file, &project),
        mcp_alter: load_file(mcp_alter_path, parse_mcp, &trust_file, &project),
        mcp_compat: load_file(mcp_compat_path, parse_mcp, &trust_file, &project),
        root,
    };
    (Some(layer), trust_error)
}

/// The review rows for one loaded MCP file.
fn mcp_review(file: &ProjectFile<mcp::McpFile>, home: Option<&Path>) -> TrustFileReview {
    TrustFileReview {
        label: "MCP servers".to_string(),
        path: alter_zero::ui::display_cwd(&file.path, home),
        items: file
            .parsed
            .as_ref()
            .map(trust::mcp_review_items)
            .unwrap_or_default(),
        error: file.parsed.as_ref().err().cloned(),
        pending: file.pending(),
    }
}

impl Session<'_> {
    /// The startup toasts for the project layer, raised least-severe first
    /// so the one toast slot ends on what matters most: the pending-config
    /// pointer (info), then a project hooks file that wouldn't parse (red —
    /// the loud posture; MCP file errors ride `report_mcp_errors`' channel),
    /// then a malformed `trust.json` (red — the guard file limits
    /// everything).
    pub(crate) fn report_trust_state(&mut self, trust_error: Option<String>) {
        let Some(layer) = &self.project_layer else {
            return;
        };
        let pending = layer.pending();
        let hooks_error = layer.hooks.as_ref().and_then(|file| {
            file.parsed.as_ref().err().map(|err| {
                format!(
                    "{}: {err} — project hooks are off this session",
                    file.path.display()
                )
            })
        });
        if pending {
            self.toast(
                "Project .alter-zero config found — /trust to review",
                ToastKind::Info,
            );
        }
        if let Some(message) = hooks_error {
            self.toast(message, ToastKind::Error);
        }
        if let Some(error) = trust_error {
            self.toast(error, ToastKind::Error);
        }
    }

    /// `/trust`: open the review menu over the bootstrap-loaded project
    /// layer (the `open_hooks_menu` injection seam). Without the layer
    /// (`ALTER_ZERO_PROJECT_CONFIG` off) the command explains via toast.
    pub(crate) fn open_trust_menu(&mut self) {
        let Some(layer) = &self.project_layer else {
            self.toast(
                "Project config is disabled (ALTER_ZERO_PROJECT_CONFIG). Unset it to review.",
                ToastKind::Info,
            );
            return;
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut files = Vec::new();
        if let Some(file) = &layer.hooks {
            files.push(TrustFileReview {
                label: "Hooks".to_string(),
                path: alter_zero::ui::display_cwd(&file.path, home.as_deref()),
                items: file
                    .parsed
                    .as_ref()
                    .map(trust::hooks_review_items)
                    .unwrap_or_default(),
                error: file.parsed.as_ref().err().cloned(),
                pending: file.pending(),
            });
        }
        for file in [&layer.mcp_alter, &layer.mcp_compat].into_iter().flatten() {
            files.push(mcp_review(file, home.as_deref()));
        }
        let review = TrustReview {
            root: alter_zero::ui::display_cwd(&layer.root, home.as_deref()),
            trusted: layer.trusted,
            files,
        };
        self.app.open_trust_menu(review);
    }

    /// Apply the `/trust` decision: the `trust.json` read-modify-write, then
    /// the live swing — the hooks merge rebound into the backend and the
    /// project MCP servers released or re-held. See `docs/project-config.md`.
    pub(crate) fn apply_trust(&mut self, action: TrustAction) {
        let Some(layer) = self.project_layer.as_mut() else {
            return;
        };
        let project = layer.root.display().to_string();
        let (files, trusted) = match action {
            TrustAction::Approve => (layer.approvable(), true),
            TrustAction::Revoke => (Vec::new(), false),
        };
        // Persist first (read-modify-write, other projects untouched). A
        // failed write still activates for this session — the user decided —
        // but says so.
        let mut write_error = None;
        if let Some(path) = config::trust_json_path() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let updated = trust::record_trust(&existing, &project, &files);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&path, updated) {
                write_error = Some(format!("Could not save trust.json: {err}"));
            }
        }
        layer.set_trusted(trusted);
        // Hooks: rebind the merge into the running session (the next turn's
        // backend build carries it).
        let merged = match self
            .project_layer
            .as_ref()
            .and_then(ProjectLayer::trusted_hooks)
        {
            Some(project_hooks) => self.user_hooks_file.merged(project_hooks),
            None => self.user_hooks_file.clone(),
        };
        self.models.set_hooks_file(merged);
        self.sync_setting_availability();
        // MCP: release what the gate held, or re-hold every project-scope
        // server.
        if let Some(manager) = &self.mcp {
            if trusted {
                let held = manager.untrusted_names();
                if !held.is_empty() {
                    manager.set_trusted(&held, true);
                }
            } else {
                let project_servers: Vec<String> = manager
                    .snapshot()
                    .into_iter()
                    .filter(|server| server.scope == mcp::McpScope::Project)
                    .map(|server| server.name)
                    .collect();
                if !project_servers.is_empty() {
                    manager.set_trusted(&project_servers, false);
                }
            }
        }
        match (action, write_error) {
            (_, Some(error)) => self.toast(error, ToastKind::Error),
            (TrustAction::Approve, None) => {
                self.toast("Trusted this project's config", ToastKind::Info);
            }
            (TrustAction::Revoke, None) => {
                self.toast("Revoked this project's trust", ToastKind::Info);
            }
        }
        self.frame.schedule_frame();
    }
}

//! The startup review's boundary half (`docs/config-review.md`): reading what
//! it decides on — the user's hooks, the project's hooks and MCP servers —
//! and `trust.json`; building its pages; applying each answer live; and the
//! `/hooks` and `/mcp` toggles that change a decision later. Every decision
//! goes through the pure read-modify-write cores in [`alter_zero::trust`],
//! applied to the session's own copy of the store first, so a session with
//! nowhere to write still remembers what it was told.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use alter_zero::app::{
    ReviewAnswer, ReviewItem, ReviewKey, ReviewKind, ReviewPage, ReviewSection, ToastKind,
};
use alter_zero::trust::{self, HookIdentity, HookTrust, ServerReview, ServerTrust, TrustFile};
use alter_zero::{hooks, mcp};

use super::Session;
use super::config;

/// A project config file as read at bootstrap: where, and its parse.
#[derive(Debug, Clone)]
pub(crate) struct ProjectFile<T> {
    pub(crate) path: PathBuf,
    pub(crate) parsed: Result<T, String>,
}

/// The project's `.alter-zero` layer, read once at bootstrap
/// (`docs/project-config.md`).
#[derive(Debug, Clone)]
pub(crate) struct ProjectLayer {
    /// `{root}/.alter-zero/hooks.json`, when present.
    pub(crate) hooks: Option<ProjectFile<hooks::HooksFile>>,
    /// `{root}/.alter-zero/mcp.json`, when present.
    pub(crate) mcp_alter: Option<ProjectFile<mcp::McpFile>>,
    /// `{root}/.mcp.json` (the Claude-Code-compat name), when present.
    pub(crate) mcp_compat: Option<ProjectFile<mcp::McpFile>>,
}

impl ProjectLayer {
    /// The project's MCP files that parsed, in precedence order.
    fn mcp_files(&self) -> Vec<(&Path, &mcp::McpFile)> {
        [&self.mcp_alter, &self.mcp_compat]
            .into_iter()
            .flatten()
            .filter_map(|file| {
                file.parsed
                    .as_ref()
                    .ok()
                    .map(|parsed| (file.path.as_path(), parsed))
            })
            .collect()
    }
}

/// What the review decides over: `trust.json` and the files it is about.
#[derive(Debug, Default)]
pub(crate) struct TrustState {
    /// `trust.json` as the session knows it — empty (nothing trusted) when
    /// the file would not parse: the guard fails closed.
    store: TrustFile,
    /// The text every record builds on: the file's, or what the session
    /// last recorded.
    store_text: String,
    /// Where the store lives; `None` without a config home, when decisions
    /// hold for the session only.
    store_path: Option<PathBuf>,
    /// The user's hooks file: its path and parse (empty when absent or
    /// broken — the broken one's toast is `report_hooks_error`'s).
    user_hooks: Option<(PathBuf, hooks::HooksFile)>,
    /// The project layer; `None` when `ALTER_ZERO_PROJECT_CONFIG` is off or
    /// the cwd resolves to the home directory.
    pub(crate) project: Option<ProjectLayer>,
}

impl TrustState {
    /// Read `trust.json` and hold the user's hooks file and the project
    /// layer beside it. The second return is the store's error to toast, if
    /// any — a malformed store is loud.
    pub(crate) fn load(
        user_hooks: Option<(PathBuf, hooks::HooksFile)>,
        project: Option<ProjectLayer>,
    ) -> (Self, Option<String>) {
        let store_path = config::trust_json_path();
        let (store, store_text, error) = config::load_trust_file(store_path.as_deref());
        (
            Self {
                store,
                store_text,
                store_path,
                user_hooks,
                project,
            },
            error,
        )
    }

    /// Every hooks file that parsed, with its absolute path and scope word —
    /// the user's first, the project's after it (the merge's order).
    fn hook_layers(&self) -> Vec<(String, &hooks::HooksFile, &'static str)> {
        let mut out = Vec::new();
        if let Some((path, file)) = &self.user_hooks {
            out.push((path.display().to_string(), file, "User"));
        }
        if let Some(file) = self.project.as_ref().and_then(|p| p.hooks.as_ref())
            && let Ok(parsed) = &file.parsed
        {
            out.push((file.path.display().to_string(), parsed, "Project"));
        }
        out
    }

    /// Every configured hook, trusted or not: the user's file with the
    /// project's unioned in after it (`HooksFile::merged`) — what `/hooks`
    /// browses and the `/settings` row's availability asks about.
    pub(crate) fn configured_hooks(&self) -> hooks::HooksFile {
        self.hook_layers()
            .into_iter()
            .fold(hooks::HooksFile::default(), |merged, (_, file, _)| {
                merged.merged(file)
            })
    }

    /// The hooks the session runs: the configured ones the user trusts
    /// (`trust::trusted_only`), each layer filtered against its own file's
    /// record.
    pub(crate) fn runner_hooks(&self) -> hooks::HooksFile {
        self.hook_layers().into_iter().fold(
            hooks::HooksFile::default(),
            |merged, (source, file, _)| {
                merged.merged(&trust::trusted_only(file, &|identity| {
                    self.store.is_hook_trusted(&source, identity)
                }))
            },
        )
    }

    /// Is `identity` trusted in the hooks file `source`?
    pub(crate) fn is_hook_trusted(&self, source: &str, identity: &HookIdentity) -> bool {
        self.store.is_hook_trusted(source, identity)
    }

    /// The hooks page: every new or changed hook, under its file.
    pub(crate) fn hooks_page(&self, home: Option<&Path>) -> Option<ReviewPage> {
        let mut sections = Vec::new();
        for (source, file, scope) in self.hook_layers() {
            let items: Vec<ReviewItem> = self
                .store
                .review_hooks(&source, file)
                .into_iter()
                .filter_map(|review| {
                    let previous = match review.trust {
                        HookTrust::Trusted => return None,
                        HookTrust::New => None,
                        HookTrust::Changed { previous } => Some(previous),
                    };
                    Some(ReviewItem {
                        title: review.identity.label(),
                        detail: vec![review.identity.command.clone()],
                        previous,
                        checked: true,
                        key: ReviewKey::Hook {
                            source: source.clone(),
                            identity: review.identity,
                        },
                    })
                })
                .collect();
            if !items.is_empty() {
                sections.push(ReviewSection {
                    scope: Some(scope.to_string()),
                    path: alter_zero::ui::display_cwd(Path::new(&source), home),
                    items,
                });
            }
        }
        (!sections.is_empty()).then(|| ReviewPage::new(ReviewKind::Hooks, sections))
    }

    /// Every project server and where it stands (`trust::review_servers`).
    pub(crate) fn server_reviews(&self) -> Vec<ServerReview> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let files: Vec<(String, &mcp::McpFile)> = project
            .mcp_files()
            .into_iter()
            .map(|(path, file)| (path.display().to_string(), file))
            .collect();
        let borrowed: Vec<(&str, &mcp::McpFile)> = files
            .iter()
            .map(|(path, file)| (path.as_str(), *file))
            .collect();
        trust::review_servers(&self.store, &borrowed)
    }

    /// The servers page: the servers the manager holds for the review
    /// (`awaiting`), each under the file declaring it.
    pub(crate) fn mcp_page(&self, awaiting: &[String], home: Option<&Path>) -> Option<ReviewPage> {
        let mut sections: Vec<ReviewSection> = Vec::new();
        for review in self.server_reviews() {
            if !awaiting.contains(&review.name) {
                continue;
            }
            let previous = match &review.trust {
                ServerTrust::Changed { previous } => Some(previous.clone()),
                _ => None,
            };
            let item = ReviewItem {
                title: format!("{} · {}", review.name, review.config.transport_label()),
                detail: server_detail(&review.config),
                previous,
                checked: true,
                key: ReviewKey::Server {
                    source: review.source.clone(),
                    name: review.name.clone(),
                },
            };
            let path = alter_zero::ui::display_cwd(Path::new(&review.source), home);
            match sections.iter_mut().find(|section| section.path == path) {
                Some(section) => section.items.push(item),
                None => sections.push(ReviewSection {
                    scope: None,
                    path,
                    items: vec![item],
                }),
            }
        }
        (!sections.is_empty()).then(|| ReviewPage::new(ReviewKind::McpServers, sections))
    }

    /// Record a hooks decision for the file `source` — the session's copy
    /// first, then `trust.json`. `Err` names a failed write: the decision
    /// still holds for the session.
    fn record_hooks(
        &mut self,
        source: &str,
        trusted: &[HookIdentity],
        distrusted: &[HookIdentity],
    ) -> Result<(), String> {
        let Some(file) = self
            .hook_layers()
            .into_iter()
            .find(|(path, _, _)| path == source)
            .map(|(_, file, _)| file.clone())
        else {
            return Ok(());
        };
        let text = trust::record_hooks(&self.store_text, source, &file, trusted, distrusted);
        self.commit(text)
    }

    /// Record server decisions — `(file, name, enabled)` — the session's
    /// copy first, then `trust.json`.
    fn record_servers(&mut self, decisions: &[(String, String, bool)]) -> Result<(), String> {
        let sources: BTreeSet<&String> = decisions.iter().map(|(source, _, _)| source).collect();
        let mut text = self.store_text.clone();
        for source in sources {
            let Some(file) = self.project.as_ref().and_then(|project| {
                project
                    .mcp_files()
                    .into_iter()
                    .find(|(path, _)| path.display().to_string() == *source)
                    .map(|(_, file)| file.clone())
            }) else {
                continue;
            };
            let named: Vec<(String, bool)> = decisions
                .iter()
                .filter(|(from, _, _)| from == source)
                .map(|(_, name, enabled)| (name.clone(), *enabled))
                .collect();
            text = trust::record_servers(&text, source, &file, &named);
        }
        self.commit(text)
    }

    /// Take `text` as the store: the session's copy, and the file when there
    /// is one to write.
    fn commit(&mut self, text: String) -> Result<(), String> {
        self.store = TrustFile::parse(&text).unwrap_or_default();
        self.store_text = text;
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, &self.store_text)
            .map_err(|err| format!("Could not save trust.json: {err} — decided for this session"))
    }
}

/// A server's detail rows on the review: its command line or URL, then for
/// a local server its environment (`KEY=value` — what the program starts
/// with is part of what runs), and for a remote one the names of the
/// headers it sends (never their values, a token's usual home).
fn server_detail(config: &mcp::McpServerConfig) -> Vec<String> {
    let mut out = vec![config.target()];
    match config {
        mcp::McpServerConfig::Stdio { env, .. } if !env.is_empty() => {
            let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            out.push(format!("env  {}", pairs.join("  ")));
        }
        mcp::McpServerConfig::Http { headers, .. } | mcp::McpServerConfig::Sse { headers, .. }
            if !headers.is_empty() =>
        {
            let names: Vec<&str> = headers.keys().map(String::as_str).collect();
            out.push(format!("headers  {}", names.join(", ")));
        }
        _ => {}
    }
    out
}

/// Gather the project layer at bootstrap: resolve the root and read each
/// project file. `None` when `ALTER_ZERO_PROJECT_CONFIG` is off, or the
/// root is the home directory (never a project, `trust::is_project_root`).
pub(crate) fn load_project_layer(cwd: &Path) -> Option<ProjectLayer> {
    if !config::project_config_enabled() {
        return None;
    }
    let root = alter_zero::project_doc::find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if !trust::is_project_root(&root, home.as_deref()) {
        return None;
    }
    // A project path that IS the user layer's file (an
    // `ALTER_ZERO_CONFIG_DIR` pointed inside the project) is already loaded
    // as the user's — never read twice.
    let user_files = [
        alter_zero::llm::hooks::hooks_file_path(config::config_home().as_deref()),
        config::mcp_user_file_path(),
    ];
    let own = |path: &PathBuf| user_files.iter().flatten().any(|user| user == path);
    let guard = |path: PathBuf| (!own(&path)).then_some(path);
    let read = |path: PathBuf| -> Option<(PathBuf, Result<String, String>)> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Some((path, Ok(text))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => Some((path, Err(err.to_string()))),
        }
    };
    let parse_mcp = |text: &str| -> Result<mcp::McpFile, String> {
        let file = mcp::parse_mcp_file(text);
        // A document-level failure parses to a lone error and no servers —
        // a broken file, not an empty one; per-entry errors keep the healthy
        // entries (the mcp module's leniently-but-loudly posture).
        match (file.servers.is_empty(), file.errors.first()) {
            (true, Some(error)) => Err(error.clone()),
            _ => Ok(file),
        }
    };
    let mcp_file = |path: Option<PathBuf>| {
        let (path, text) = read(path?)?;
        Some(ProjectFile {
            parsed: text.and_then(|text| parse_mcp(&text)),
            path,
        })
    };
    let [mcp_alter, mcp_compat] = trust::project_mcp_files(&root).map(guard);
    let hooks = guard(trust::project_hooks_file(&root))
        .and_then(read)
        .map(|(path, text)| ProjectFile {
            // Every handler remembers the project's file, so a verdict can
            // name it beside the user's own.
            parsed: text.and_then(|text| {
                hooks::HooksFile::parse(&text).map(|file| file.with_source(&path))
            }),
            path,
        });
    Some(ProjectLayer {
        hooks,
        mcp_alter: mcp_file(mcp_alter),
        mcp_compat: mcp_file(mcp_compat),
    })
}

impl Session<'_> {
    /// The startup toasts for what the review reads, least severe first so
    /// the one toast slot ends on what matters most: a project hooks file
    /// that would not parse (red — its MCP siblings' errors ride
    /// `report_mcp_errors`), then a malformed `trust.json` (red — the guard
    /// file limits everything).
    pub(crate) fn report_trust_state(&mut self, trust_error: Option<String>) {
        let hooks_error = self
            .trust
            .project
            .as_ref()
            .and_then(|project| project.hooks.as_ref())
            .and_then(|file| {
                file.parsed.as_ref().err().map(|err| {
                    format!(
                        "{}: {err} — project hooks are off this session",
                        file.path.display()
                    )
                })
            });
        if let Some(message) = hooks_error {
            self.toast(message, ToastKind::Error);
        }
        if let Some(error) = trust_error {
            self.toast(error, ToastKind::Error);
        }
    }

    /// The review's pages for what is new or changed: the hooks page while
    /// the session runs hooks, the servers page for the servers the manager
    /// holds for it.
    fn review_pages(&self, hooks: bool, servers: bool) -> Vec<ReviewPage> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut pages = Vec::new();
        if hooks
            && self.models.hooks_enabled()
            && let Some(page) = self.trust.hooks_page(home.as_deref())
        {
            pages.push(page);
        }
        if servers
            && let Some(manager) = &self.mcp
            && let Some(page) = self
                .trust
                .mcp_page(&manager.awaiting_review(), home.as_deref())
        {
            pages.push(page);
        }
        pages
    }

    /// Open the startup review over whatever waits for a decision, holding
    /// `SessionStart` until it closes (`docs/config-review.md`). Whether it
    /// opened — the caller holds a `[PROMPT]` the same way.
    pub(crate) fn open_startup_review(&mut self) -> bool {
        let pages = self.review_pages(true, true);
        self.app.open_config_review(pages);
        let open = self.app.config_review.is_some();
        if open {
            self.models.hold_session_start();
        }
        open
    }

    /// Open the hooks page alone — the `/settings` **Hooks** row turned on
    /// over hooks that wait for trust.
    pub(crate) fn open_hooks_review(&mut self) {
        let pages = self.review_pages(true, false);
        self.app.open_config_review(pages);
    }

    /// Apply one page's answer (`docs/config-review.md`), then — once the
    /// review has closed — let what it held go.
    pub(crate) fn apply_review_answer(&mut self, answer: ReviewAnswer) {
        let result = match answer.kind {
            ReviewKind::Hooks => self.apply_hooks_answer(&answer),
            ReviewKind::McpServers => self.apply_servers_answer(&answer),
        };
        if let Err(error) = result {
            self.toast(error, ToastKind::Error);
        }
        if self.app.config_review.is_none() {
            self.finish_review();
        }
        self.frame.schedule_frame();
    }

    /// Hooks: a confirmed page records the selected hooks as trusted, and
    /// they run from now on; the rest stay untrusted, unrecorded, asked
    /// about again at the next launch — as is everything on a declined page.
    fn apply_hooks_answer(&mut self, answer: &ReviewAnswer) -> Result<(), String> {
        if !answer.confirmed {
            return Ok(());
        }
        let mut by_file: Vec<(String, Vec<HookIdentity>)> = Vec::new();
        for (key, checked) in &answer.choices {
            if let (ReviewKey::Hook { source, identity }, true) = (key, checked) {
                match by_file.iter_mut().find(|(file, _)| file == source) {
                    Some((_, list)) => list.push(identity.clone()),
                    None => by_file.push((source.clone(), vec![identity.clone()])),
                }
            }
        }
        let mut result = Ok(());
        for (source, trusted) in &by_file {
            if let Err(error) = self.trust.record_hooks(source, trusted, &[]) {
                result = Err(error);
            }
        }
        self.models.set_hooks_file(self.trust.runner_hooks());
        result
    }

    /// Servers: a confirmed page records each server enabled or disabled as
    /// it was selected; a declined one records every server disabled. The
    /// enabled ones connect at once.
    fn apply_servers_answer(&mut self, answer: &ReviewAnswer) -> Result<(), String> {
        let decisions: Vec<(String, String, bool)> = answer
            .choices
            .iter()
            .filter_map(|(key, checked)| match key {
                ReviewKey::Server { source, name } => {
                    Some((source.clone(), name.clone(), answer.confirmed && *checked))
                }
                ReviewKey::Hook { .. } => None,
            })
            .collect();
        let result = self.trust.record_servers(&decisions);
        if let Some(manager) = &self.mcp {
            for (_, name, enabled) in &decisions {
                manager.set_approved(name, *enabled);
            }
        }
        result
    }

    /// The review has closed: fire the held `SessionStart` with the hooks it
    /// trusted, and submit a held `[PROMPT]` so the first turn starts with
    /// the servers just enabled.
    fn finish_review(&mut self) {
        self.models.release_session_start();
        if let Some(prompt) = self.held_prompt.take() {
            self.submit_startup_prompt(prompt);
        }
    }

    /// `/hooks`: trust a hook, or stop trusting it — recorded, and the hooks
    /// the session runs rebound at once.
    pub(crate) fn set_hook_trust(&mut self, source: &str, identity: HookIdentity, trusted: bool) {
        let (trust, distrust) = if trusted {
            (vec![identity], Vec::new())
        } else {
            (Vec::new(), vec![identity])
        };
        let result = self.trust.record_hooks(source, &trust, &distrust);
        self.models.set_hooks_file(self.trust.runner_hooks());
        match result {
            Err(error) => self.toast(error, ToastKind::Error),
            Ok(()) if trusted => self.toast("Hook trusted — it runs from now on", ToastKind::Info),
            Ok(()) => self.toast("Hook no longer trusted — it does not run", ToastKind::Info),
        }
        self.frame.schedule_frame();
    }

    /// `/mcp` Enable or Disable on a **project** server: the decision is the
    /// review's, recorded in `trust.json`, never in the user's MCP file.
    /// `false` when `name` is not a project server the session knows, so
    /// the caller applies the user file's toggle instead.
    pub(crate) fn set_project_server(&mut self, name: &str, enabled: bool) -> bool {
        let Some(manager) = self.mcp.clone() else {
            return false;
        };
        let project = manager
            .snapshot()
            .iter()
            .any(|server| server.name == name && server.scope == mcp::McpScope::Project);
        let Some(review) = self.server_reviews_named(name).filter(|_| project) else {
            return false;
        };
        let result = self
            .trust
            .record_servers(&[(review.source, name.to_string(), enabled)]);
        manager.set_approved(name, enabled);
        // A project server turned off in `/mcp` before the review existed
        // sits in the user file's disabled set; enabling it here clears that
        // too, or it would stay off.
        if enabled && manager.is_user_disabled(name) {
            manager.set_disabled(name, false);
        }
        if let Err(error) = result {
            self.toast(error, ToastKind::Error);
        }
        true
    }

    fn server_reviews_named(&self, name: &str) -> Option<ServerReview> {
        self.trust
            .server_reviews()
            .into_iter()
            .find(|review| review.name == name)
    }
}

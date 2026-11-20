//! The startup review's boundary half (`docs/config-review.md`): reading what
//! it decides on — the user's hooks, the project's hooks and MCP servers —
//! and `trust.json`; building its pages; applying each answer live; and the
//! `/hooks` and `/mcp` toggles that change a decision later. Every decision
//! is applied to the session's own copy of the store first, so a session
//! with nowhere to write still remembers what it was told, then recorded
//! over `trust.json` **as it is on disk at that moment**
//! (`trust::update_store`) — never over the copy read at startup, which
//! would erase whatever another session recorded since — and never over a
//! file that will not parse.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use alter_zero::app::{
    ItemChange, ReviewAnswer, ReviewItem, ReviewKey, ReviewKind, ReviewPage, ReviewSection,
    ToastKind,
};
use alter_zero::trust::{
    self, HookIdentity, HookTrust, ServerChange, ServerReview, ServerTrust, TrustFile,
};
use alter_zero::{hooks, mcp};

use super::Session;
use super::config;

/// One MCP file's share of a set of server decisions: its absolute path,
/// its parse, and `(name, enabled)` for each server decided.
type FileDecisions = (String, mcp::McpFile, Vec<(String, bool)>);

/// A project config file as read at bootstrap: where, the fingerprint of
/// its bytes, and its parse.
#[derive(Debug, Clone)]
pub(crate) struct ProjectFile<T> {
    pub(crate) path: PathBuf,
    /// What the `/trust` era pinned an approval to (`trust::fingerprint` of
    /// the bytes) — what carrying one over compares against.
    pub(crate) fingerprint: String,
    pub(crate) parsed: Result<T, String>,
}

impl<T: Clone> ProjectFile<T> {
    /// `(path, fingerprint, parse)` — the file as the `/trust` era's
    /// approvals are carried over against — when it parsed.
    fn pinned(&self) -> Option<(String, String, T)> {
        let parsed = self.parsed.as_ref().ok()?.clone();
        Some((
            self.path.display().to_string(),
            self.fingerprint.clone(),
            parsed,
        ))
    }
}

/// The project's `.alter-zero` layer, read once at bootstrap
/// (`docs/project-config.md`).
#[derive(Debug, Clone)]
pub(crate) struct ProjectLayer {
    /// The project root — the `/trust` era's key for its approvals.
    pub(crate) root: PathBuf,
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
    /// `trust.json` as the session knows it: what it read at startup — empty
    /// (nothing trusted) when the file would not parse, the guard failing
    /// closed — plus every decision made since.
    store: TrustFile,
    /// Where the store lives; `None` without a config home, when decisions
    /// hold for the session only.
    store_path: Option<PathBuf>,
    /// The user's hooks file: its path and parse (empty when absent or
    /// broken — the broken one's toast is `report_hooks_error`'s).
    user_hooks: Option<(PathBuf, hooks::HooksFile)>,
    /// The project layer; `None` when `ALTER_ZERO_PROJECT_CONFIG` is off or
    /// the cwd resolves to the home directory.
    pub(crate) project: Option<ProjectLayer>,
    /// The enabled plugins' hooks, one layer per file, after the project's
    /// (`docs/plugins.md`) — untrusted until reviewed, like any other.
    plugin_hooks: Vec<hooks::PluginHooks>,
    /// The enabled plugins' MCP files — the third reviewed scope.
    plugin_mcp: Vec<mcp::PluginMcpFile>,
}

impl TrustState {
    /// Read `trust.json` and hold the user's hooks file and the project
    /// layer beside it, carrying the project's `/trust`-era approvals over
    /// first. The second return is the error to toast, if any — a
    /// malformed store is loud.
    pub(crate) fn load(
        user_hooks: Option<(PathBuf, hooks::HooksFile)>,
        project: Option<ProjectLayer>,
        plugin_hooks: Vec<hooks::PluginHooks>,
        plugin_mcp: Vec<mcp::PluginMcpFile>,
    ) -> (Self, Option<String>) {
        let store_path = config::trust_json_path();
        let (store, error) = config::load_trust_file(store_path.as_deref());
        let mut state = Self {
            store,
            store_path,
            user_hooks,
            project,
            plugin_hooks,
            plugin_mcp,
        };
        let migrated = state.migrate_legacy().err();
        (state, error.or(migrated))
    }

    /// Carry over what `/trust` approved in this project at exactly the
    /// files' current bytes (`TrustFile::migrate_legacy`), so an upgrade
    /// does not ask about it again; written back only when something was.
    fn migrate_legacy(&mut self) -> Result<(), String> {
        let Some(project) = &self.project else {
            return Ok(());
        };
        let root = project.root.display().to_string();
        let hooks = project.hooks.as_ref().and_then(ProjectFile::pinned);
        let mcp: Vec<(String, String, mcp::McpFile)> = [&project.mcp_alter, &project.mcp_compat]
            .into_iter()
            .flatten()
            .filter_map(ProjectFile::pinned)
            .collect();
        let migrate = |store: &mut TrustFile| {
            let hooks = hooks
                .as_ref()
                .map(|(path, fingerprint, file)| (path.as_str(), fingerprint.as_str(), file));
            let mcp: Vec<(&str, &str, &mcp::McpFile)> = mcp
                .iter()
                .map(|(path, fingerprint, file)| (path.as_str(), fingerprint.as_str(), file))
                .collect();
            store.migrate_legacy(&root, hooks, &mcp)
        };
        if !migrate(&mut self.store) {
            return Ok(());
        }
        self.persist(|store| {
            migrate(store);
        })
    }

    /// Replace the plugins' hook layers (an install, an update, a toggle).
    /// Whether they changed — the caller rebinds the runner then.
    pub(crate) fn set_plugin_hooks(&mut self, layers: Vec<hooks::PluginHooks>) -> bool {
        let changed = self.plugin_hooks != layers;
        self.plugin_hooks = layers;
        changed
    }

    /// Replace the plugins' MCP files. Whether they changed.
    pub(crate) fn set_plugin_mcp(&mut self, files: Vec<mcp::PluginMcpFile>) -> bool {
        let changed = self.plugin_mcp != files;
        self.plugin_mcp = files;
        changed
    }

    /// The plugins' MCP files.
    pub(crate) fn plugin_mcp(&self) -> &[mcp::PluginMcpFile] {
        &self.plugin_mcp
    }

    /// Every hooks file that parsed, with its absolute path and scope word —
    /// the user's first, the project's after it (the merge's order), then
    /// each plugin's, its scope naming the plugin.
    fn hook_layers(&self) -> Vec<(String, &hooks::HooksFile, String)> {
        let mut out = Vec::new();
        if let Some((path, file)) = &self.user_hooks {
            out.push((path.display().to_string(), file, "User".to_string()));
        }
        if let Some(file) = self.project.as_ref().and_then(|p| p.hooks.as_ref())
            && let Ok(parsed) = &file.parsed
        {
            out.push((
                file.path.display().to_string(),
                parsed,
                "Project".to_string(),
            ));
        }
        for layer in &self.plugin_hooks {
            out.push((
                layer.path.display().to_string(),
                &layer.file,
                format!("Plugin {}", layer.plugin.name),
            ));
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
                    let change = match review.trust {
                        HookTrust::Trusted => return None,
                        HookTrust::New => None,
                        HookTrust::Changed { previous } => Some(ItemChange::Was(previous)),
                    };
                    Some(ReviewItem {
                        title: review.identity.label(),
                        detail: vec![review.identity.command.clone()],
                        change,
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
                    scope: Some(scope),
                    path: alter_zero::ui::display_cwd(Path::new(&source), home),
                    items,
                });
            }
        }
        (!sections.is_empty()).then(|| ReviewPage::new(ReviewKind::Hooks, sections))
    }

    /// Every reviewed MCP file — the project's, then each plugin's — as
    /// `(absolute path, parse)`, in precedence order.
    fn reviewed_mcp_files(&self) -> Vec<(String, mcp::McpFile)> {
        let mut out: Vec<(String, mcp::McpFile)> = self
            .project
            .as_ref()
            .map(|project| {
                project
                    .mcp_files()
                    .into_iter()
                    .map(|(path, file)| (path.display().to_string(), file.clone()))
                    .collect()
            })
            .unwrap_or_default();
        out.extend(
            self.plugin_mcp
                .iter()
                .map(|file| (file.source.display().to_string(), file.as_mcp_file())),
        );
        out
    }

    /// Every project and plugin server and where it stands
    /// (`trust::review_servers`).
    pub(crate) fn server_reviews(&self) -> Vec<ServerReview> {
        // Without plugins, exactly the path every launch has always run —
        // nothing of the plugins' code is touched (`docs/plugins.md`
        // *Memory*).
        if !self.plugin_mcp.is_empty() {
            return self.server_reviews_with_plugins();
        }
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

    /// [`Self::server_reviews`] with the plugins' MCP files after the
    /// project's.
    fn server_reviews_with_plugins(&self) -> Vec<ServerReview> {
        let files = self.reviewed_mcp_files();
        let borrowed: Vec<(&str, &mcp::McpFile)> = files
            .iter()
            .map(|(path, file)| (path.as_str(), file))
            .collect();
        trust::review_servers(&self.store, &borrowed)
    }

    /// Are there plugin MCP files? The caller's guard, so a launch without
    /// plugins calls none of their code.
    #[inline]
    pub(crate) fn has_plugin_mcp(&self) -> bool {
        !self.plugin_mcp.is_empty()
    }

    /// The plugin declaring the MCP file `source`, when a plugin does.
    fn mcp_plugin(&self, source: &str) -> Option<&mcp::PluginMcpFile> {
        self.plugin_mcp
            .iter()
            .find(|file| file.source.display().to_string() == source)
    }

    /// The servers page: the servers the manager holds for the review
    /// (`awaiting`), each under the file declaring it.
    pub(crate) fn mcp_page(&self, awaiting: &[String], home: Option<&Path>) -> Option<ReviewPage> {
        let mut sections: Vec<ReviewSection> = Vec::new();
        for review in self.server_reviews() {
            if !awaiting.contains(&review.name) {
                continue;
            }
            let change = match &review.trust {
                ServerTrust::Changed { changes } => Some(ItemChange::Parts(
                    changes.iter().map(ServerChange::describe).collect(),
                )),
                _ => None,
            };
            let plugin = if self.plugin_mcp.is_empty() {
                None
            } else {
                self.mcp_plugin(&review.source)
            };
            let mut detail = server_detail(&review.config);
            // A plugin's local server runs in a directory of its plugin's
            // choosing: part of what runs, so the page says where.
            if let Some(cwd) = plugin
                .and_then(|file| file.servers.iter().find(|s| s.name == review.name))
                .and_then(|server| server.launch.cwd.as_ref())
                .filter(|_| !review.config.is_remote())
            {
                detail.push(format!("cwd  {}", cwd.display()));
            }
            let item = ReviewItem {
                title: format!("{} · {}", review.name, review.config.transport_label()),
                detail,
                change,
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
                    scope: plugin.map(|file| format!("Plugin {}", file.plugin)),
                    path,
                    items: vec![item],
                }),
            }
        }
        // Beside a plugin's file, a project's names its scope too.
        if sections.iter().any(|section| section.scope.is_some()) {
            for section in &mut sections {
                section.scope.get_or_insert_with(|| "Project".to_string());
            }
        }
        (!sections.is_empty()).then(|| ReviewPage::new(ReviewKind::McpServers, sections))
    }

    /// Record a hooks decision for the file `source`. `Err` names a write
    /// that did not happen: the decision still holds for the session.
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
        self.record(|store| store.record_hooks(source, &file, trusted, distrusted))
    }

    /// Record server decisions — `(file, name, enabled)`.
    fn record_servers(&mut self, decisions: &[(String, String, bool)]) -> Result<(), String> {
        let sources: BTreeSet<&String> = decisions.iter().map(|(source, _, _)| source).collect();
        let mut files: Vec<FileDecisions> = Vec::new();
        let reviewed = self.reviewed_mcp_files();
        for source in sources {
            let Some(file) = reviewed
                .iter()
                .find(|(path, _)| path == source)
                .map(|(_, file)| file.clone())
            else {
                continue;
            };
            let named: Vec<(String, bool)> = decisions
                .iter()
                .filter(|(from, _, _)| from == source)
                .map(|(_, name, enabled)| (name.clone(), *enabled))
                .collect();
            files.push((source.clone(), file, named));
        }
        self.record(|store| {
            for (source, file, named) in &files {
                store.record_servers(source, file, named);
            }
        })
    }

    /// Forget every decision about the files under `dir` — an installed
    /// plugin's cache directory — in the session's copy and in
    /// `trust.json` (`docs/plugins.md` *Installing*).
    pub(crate) fn forget_under(&mut self, dir: &Path) -> Result<(), String> {
        self.store.forget_under(dir);
        forget_plugin_trust(self.store_path.as_deref(), dir)
    }

    /// Apply `change` to the session's copy, then record it in `trust.json`.
    fn record(&mut self, change: impl Fn(&mut TrustFile)) -> Result<(), String> {
        change(&mut self.store);
        self.persist(change)
    }

    /// Record `change` over `trust.json` as it is on disk **now** — read
    /// afresh, so what another session recorded since startup survives — and
    /// never over a file that will not parse (`trust::update_store`): a typo
    /// costs this session's choices their persistence, not every earlier
    /// decision. `Ok` with nowhere to write (no config home).
    fn persist(&self, change: impl FnOnce(&mut TrustFile)) -> Result<(), String> {
        persist_at(self.store_path.as_deref(), change)
    }
}

/// `page` keeping only the sections a plugin's files make — what a change of
/// plugins asks about, never a decision the user already put off.
fn plugins_only(mut page: ReviewPage) -> Option<ReviewPage> {
    page.sections.retain(|section| {
        section
            .scope
            .as_deref()
            .is_some_and(|scope| scope.starts_with("Plugin "))
    });
    (!page.sections.is_empty()).then(|| ReviewPage::new(page.kind, page.sections))
}

/// Forget every hooks and MCP decision recorded for a file under `dir` (an
/// installed plugin's cache directory) in the `trust.json` at `store_path`
/// — the boundary half of `TrustFile::forget_under`, through the same
/// read-modify-write as every decision. Callable before the TUI boots
/// (`alter-zero plugin install`): `store_path` is
/// `config::trust_json_path()`. `Ok` with nowhere to write.
pub(crate) fn forget_plugin_trust(store_path: Option<&Path>, dir: &Path) -> Result<(), String> {
    persist_at(store_path, |store| {
        store.forget_under(dir);
    })
}

/// Record `change` over the `trust.json` at `path` as it is on disk now.
fn persist_at(path: Option<&Path>, change: impl FnOnce(&mut TrustFile)) -> Result<(), String> {
    {
        let Some(path) = path else {
            return Ok(());
        };
        let not_saved = |err: &dyn std::fmt::Display| {
            format!("trust.json: {err} — not saved, so this choice lasts this session only")
        };
        let existing = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(not_saved(&err)),
        };
        let text = trust::update_store(&existing, change).map_err(|err| not_saved(&err))?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, text).map_err(|err| {
            format!("Could not save trust.json: {err} — this choice lasts this session only")
        })
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
        config::user_hooks_path(),
        config::mcp_user_file_path().map(|path| std::path::absolute(&path).unwrap_or(path)),
    ];
    let own = |path: &PathBuf| user_files.iter().flatten().any(|user| user == path);
    let guard = |path: PathBuf| (!own(&path)).then_some(path);
    let read = |path: PathBuf| -> Option<(PathBuf, String, Result<String, String>)> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Some((path, trust::fingerprint(text.as_bytes()), Ok(text))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => Some((path, String::new(), Err(err.to_string()))),
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
        let (path, fingerprint, text) = read(path?)?;
        Some(ProjectFile {
            parsed: text.and_then(|text| parse_mcp(&text)),
            fingerprint,
            path,
        })
    };
    let [mcp_alter, mcp_compat] = trust::project_mcp_files(&root).map(guard);
    let hooks =
        guard(trust::project_hooks_file(&root))
            .and_then(read)
            .map(|(path, fingerprint, text)| ProjectFile {
                // Every handler remembers the project's file, so a verdict can
                // name it beside the user's own.
                parsed: text.and_then(|text| {
                    hooks::HooksFile::parse(&text).map(|file| file.with_source(&path))
                }),
                fingerprint,
                path,
            });
    Some(ProjectLayer {
        root,
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
    /// holds for it while MCP is on (the `/settings` **MCP** row).
    pub(crate) fn review_pages(&self, hooks: bool, servers: bool) -> Vec<ReviewPage> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut pages = Vec::new();
        if hooks
            && self.models.hooks_enabled()
            && let Some(page) = self.trust.hooks_page(home.as_deref())
        {
            pages.push(page);
        }
        if servers
            && self.mcp.is_active()
            && let Some(page) = self
                .trust
                .mcp_page(&self.mcp.awaiting_review(), home.as_deref())
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
        self.app.open_requested_review(pages);
    }

    /// Open the servers page alone — the `/settings` **MCP** row turned on
    /// over project servers that wait for the review.
    pub(crate) fn open_servers_review(&mut self) {
        let pages = self.review_pages(false, true);
        self.app.open_requested_review(pages);
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
        for (_, name, enabled) in &decisions {
            self.mcp.set_approved(name, *enabled);
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

    /// `/mcp` Enable or Disable on a **project** or **plugin** server: the
    /// decision is the review's, recorded in `trust.json`, never in the
    /// user's MCP file. `false` when `name` is not a reviewed server the
    /// session knows, so the caller applies the user file's toggle instead.
    pub(crate) fn set_reviewed_server(&mut self, name: &str, enabled: bool) -> bool {
        let manager = self.mcp.clone();
        let project = manager.snapshot().iter().any(|server| {
            server.name == name
                && matches!(server.scope, mcp::McpScope::Project | mcp::McpScope::Plugin)
        });
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
        let cleared = if enabled && manager.is_user_disabled(name) {
            manager.set_disabled(name, false)
        } else {
            Ok(())
        };
        if let Err(error) = result.and(cleared) {
            self.toast(error, ToastKind::Error);
        }
        true
    }

    /// Take in what the enabled plugins contribute now (`docs/plugins.md`
    /// *Loading into a session*): first forget every trust decision under
    /// the `forget` directories (a plugin just installed, updated or
    /// removed), then replace the plugins' hook layers — rebinding the
    /// hooks the session runs — and their MCP files — adding, holding and
    /// dropping servers in the running manager — and open the review on
    /// whatever of theirs now waits for a decision. Nothing changed, nothing
    /// done: the session without plugins pays nothing here.
    pub(crate) fn set_plugin_contributions(
        &mut self,
        plugin_hooks: Vec<hooks::PluginHooks>,
        plugin_mcp: Vec<mcp::PluginMcpFile>,
        forget: &[PathBuf],
    ) {
        let mut errors = Vec::new();
        for dir in forget {
            if let Err(error) = self.trust.forget_under(dir) {
                errors.push(error);
            }
        }
        let forgot = !forget.is_empty();
        let hooks_changed = self.trust.set_plugin_hooks(plugin_hooks) || forgot;
        let mcp_changed = self.trust.set_plugin_mcp(plugin_mcp) || forgot;
        if !hooks_changed && !mcp_changed && errors.is_empty() {
            return;
        }
        if hooks_changed {
            self.models.set_hooks_file(self.trust.runner_hooks());
        }
        if mcp_changed {
            errors.extend(super::mcp::sync_plugin_servers(&self.mcp, &self.trust));
        }
        for error in errors {
            self.toast(error, ToastKind::Error);
        }
        let pages = self
            .review_pages(hooks_changed, mcp_changed)
            .into_iter()
            .filter_map(plugins_only)
            .collect();
        self.app.open_requested_review(pages);
        self.frame.schedule_frame();
    }

    fn server_reviews_named(&self, name: &str) -> Option<ServerReview> {
        self.trust
            .server_reviews()
            .into_iter()
            .find(|review| review.name == name)
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    fn plugin_hooks(name: &str, root: &str) -> hooks::PluginHooks {
        hooks::PluginHooks::parse(
            hooks::HookPlugin {
                name: name.to_string(),
                root: PathBuf::from(root),
                data: PathBuf::from("/data"),
            },
            PathBuf::from(format!("{root}/hooks/hooks.json")),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"${CLAUDE_PLUGIN_ROOT}/run.sh"}]}]}}"#,
        )
        .expect("fixture parses")
    }

    fn stop_commands(file: &hooks::HooksFile) -> Vec<String> {
        file.select(hooks::HookEvent::Stop, None)
            .handlers
            .into_iter()
            .map(|hook| hook.command)
            .collect()
    }

    /// Nothing a plugin's hooks file holds reaches the runner before the
    /// review: untrusted, it is on the hooks page under its plugin's name;
    /// trusted, it runs; and forgetting the plugin's directory (an install
    /// or an update) puts it back to unreviewed.
    #[test]
    fn a_plugins_hooks_run_only_once_reviewed() {
        let mut state = TrustState::default();
        assert!(state.set_plugin_hooks(vec![
            plugin_hooks("one@m", "/cache/one"),
            plugin_hooks("two@m", "/cache/two"),
        ]));
        assert!(!state.set_plugin_hooks(vec![
            plugin_hooks("one@m", "/cache/one"),
            plugin_hooks("two@m", "/cache/two"),
        ]));
        assert!(stop_commands(&state.runner_hooks()).is_empty());
        assert_eq!(stop_commands(&state.configured_hooks()).len(), 2);
        let page = state.hooks_page(None).expect("waiting for review");
        let scopes: Vec<Option<&str>> = page
            .sections
            .iter()
            .map(|section| section.scope.as_deref())
            .collect();
        assert_eq!(scopes, [Some("Plugin one@m"), Some("Plugin two@m")]);
        let ReviewKey::Hook { source, identity } = page.sections[0].items[0].key.clone() else {
            panic!("a hook");
        };
        assert_eq!(identity.command, "/cache/one/run.sh");
        state.record_hooks(&source, &[identity], &[]).unwrap();
        assert_eq!(stop_commands(&state.runner_hooks()), ["/cache/one/run.sh"]);
        // An install or an update forgets it: asked about again.
        state.forget_under(Path::new("/cache/one")).unwrap();
        assert!(stop_commands(&state.runner_hooks()).is_empty());
        // Removed: gone from everything.
        assert!(state.set_plugin_hooks(Vec::new()));
        assert!(stop_commands(&state.configured_hooks()).is_empty());
        assert!(state.hooks_page(None).is_none());
    }

    fn plugin_mcp() -> mcp::PluginMcpFile {
        let root = Path::new("/cache/db");
        mcp::PluginMcpFile::parse_legacy(
            "db@m",
            "db",
            root.join(".mcp.json"),
            r#"{"main":{"command":"./bin/srv","cwd":"."}}"#,
            root,
            Path::new("/data/db"),
        )
    }

    /// A plugin's servers are reviewed like a project's: listed new until
    /// decided, on the servers page under the plugin's name with where they
    /// run, and recorded against the plugin's file.
    #[test]
    fn a_plugins_servers_are_reviewed_and_recorded() {
        let mut state = TrustState::default();
        assert!(state.server_reviews().is_empty());
        assert!(state.set_plugin_mcp(vec![plugin_mcp()]));
        let reviews = state.server_reviews();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].name, "plugin:db:main");
        assert!(reviews[0].trust.needs_review());
        let page = state
            .mcp_page(&["plugin:db:main".to_string()], None)
            .expect("waiting for review");
        assert_eq!(page.sections[0].scope.as_deref(), Some("Plugin db@m"));
        assert!(
            page.sections[0].items[0]
                .detail
                .contains(&"cwd  /cache/db/.".to_string()),
            "{:?}",
            page.sections[0].items[0].detail
        );
        state
            .record_servers(&[(
                "/cache/db/.mcp.json".to_string(),
                "plugin:db:main".to_string(),
                true,
            )])
            .unwrap();
        assert!(state.server_reviews()[0].trust.approved());
        state.forget_under(Path::new("/cache/db")).unwrap();
        assert!(state.server_reviews()[0].trust.needs_review());
    }

    /// The review a plugin change opens asks only about the plugins' files.
    #[test]
    fn a_plugin_change_reviews_only_the_plugins_files() {
        let page = ReviewPage::new(
            ReviewKind::Hooks,
            vec![
                ReviewSection {
                    scope: Some("User".to_string()),
                    path: "~/.alter-zero/hooks.json".to_string(),
                    items: Vec::new(),
                },
                ReviewSection {
                    scope: Some("Plugin one@m".to_string()),
                    path: "~/x".to_string(),
                    items: Vec::new(),
                },
            ],
        );
        let kept = plugins_only(page).expect("a plugin section");
        assert_eq!(kept.sections.len(), 1);
        assert!(plugins_only(ReviewPage::new(ReviewKind::Hooks, Vec::new())).is_none());
    }
}

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
use alter_zero::plugins::{EnabledPlugin, PluginId};
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

/// One file an enabled plugin's install generated
/// (`.alter-zero-plugin/hooks.json` or `mcp.json`), read once at bootstrap.
#[derive(Debug, Clone)]
pub(crate) struct PluginFile<T> {
    pub(crate) path: PathBuf,
    pub(crate) parsed: Result<T, String>,
}

/// One enabled plugin's share of the review: its id and whichever of its
/// two generated files its record names.
#[derive(Debug, Clone)]
pub(crate) struct PluginFiles {
    pub(crate) id: PluginId,
    pub(crate) hooks: Option<PluginFile<hooks::HooksFile>>,
    pub(crate) mcp: Option<PluginFile<mcp::McpFile>>,
}

/// The installed plugins' hooks and MCP servers, read once at bootstrap —
/// the project layer's sibling, held for the same review
/// (`docs/plugins.md`).
#[derive(Debug, Clone)]
pub(crate) struct PluginLayer {
    /// `{config_home}/plugins` — what names a generated file's plugin
    /// (`plugins::plugin_of_generated`).
    pub(crate) root: PathBuf,
    /// The enabled plugins with a hooks or an MCP file, in id order.
    pub(crate) files: Vec<PluginFiles>,
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
    /// The plugin layer; `None` when no enabled plugin has a hooks or an MCP
    /// file — the no-plugin session's whole cost.
    pub(crate) plugins: Option<PluginLayer>,
}

impl TrustState {
    /// Read `trust.json` and hold the user's hooks file and the project
    /// layer beside it, carrying the project's `/trust`-era approvals over
    /// first. The second return is the error to toast, if any — a
    /// malformed store is loud.
    pub(crate) fn load(
        user_hooks: Option<(PathBuf, hooks::HooksFile)>,
        project: Option<ProjectLayer>,
        plugins: Option<PluginLayer>,
    ) -> (Self, Option<String>) {
        let store_path = config::trust_json_path();
        let (store, error) = config::load_trust_file(store_path.as_deref());
        let mut state = Self {
            store,
            store_path,
            user_hooks,
            project,
            plugins,
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

    /// Every hooks file that parsed, with its absolute path and scope word —
    /// the user's first, the project's after it, then each plugin's in id
    /// order (the merge's order).
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
        for file in self
            .plugin_files()
            .filter_map(|plugin| plugin.hooks.as_ref())
        {
            if let Ok(parsed) = &file.parsed {
                out.push((file.path.display().to_string(), parsed, "Plugin"));
            }
        }
        out
    }

    /// The enabled plugins' files, in id order — nothing without a layer.
    fn plugin_files(&self) -> impl Iterator<Item = &PluginFiles> {
        self.plugins.iter().flat_map(|layer| layer.files.iter())
    }

    /// `plugin name@marketplace` for a file an enabled plugin's install
    /// generated — what the review, `/hooks` and `/mcp` title it — else
    /// `None` (`docs/plugins.md`).
    pub(crate) fn plugin_label(&self, source: &Path) -> Option<String> {
        let root = &self.plugins.as_ref()?.root;
        alter_zero::plugins::plugin_of_generated(source, root).map(|id| plugin_heading(&id))
    }

    /// The plugins' hooks files that would not parse, each worded for its
    /// toast: that plugin's hooks are off this session, nothing else.
    pub(crate) fn plugin_hooks_errors(&self) -> Vec<String> {
        self.plugin_files()
            .filter_map(|plugin| {
                let error = plugin.hooks.as_ref()?.parsed.as_ref().err()?;
                Some(format!(
                    "{}: {error} — its hooks are off this session",
                    plugin_heading(&plugin.id)
                ))
            })
            .collect()
    }

    /// Every MCP file the review decides over that parsed, in precedence
    /// order: the project's two, then each plugin's.
    fn reviewed_mcp_files(&self) -> Vec<(&Path, &mcp::McpFile)> {
        let mut out = self
            .project
            .as_ref()
            .map(ProjectLayer::mcp_files)
            .unwrap_or_default();
        for file in self.plugin_files().filter_map(|plugin| plugin.mcp.as_ref()) {
            if let Ok(parsed) = &file.parsed {
                out.push((file.path.as_path(), parsed));
            }
        }
        out
    }

    /// How a review page heads a file: `plugin name@marketplace` for a
    /// plugin's, else the path from the home.
    fn section_path(&self, source: &str, home: Option<&Path>) -> String {
        let path = Path::new(source);
        self.plugin_label(path)
            .unwrap_or_else(|| alter_zero::ui::display_cwd(path, home))
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
                // A plugin's heading names the plugin, which says the scope.
                let label = self.plugin_label(Path::new(&source));
                sections.push(ReviewSection {
                    scope: label.is_none().then(|| scope.to_string()),
                    path: label
                        .unwrap_or_else(|| alter_zero::ui::display_cwd(Path::new(&source), home)),
                    items,
                });
            }
        }
        (!sections.is_empty()).then(|| ReviewPage::new(ReviewKind::Hooks, sections))
    }

    /// Every project and plugin server and where it stands
    /// (`trust::review_servers`).
    pub(crate) fn server_reviews(&self) -> Vec<ServerReview> {
        let files: Vec<(String, &mcp::McpFile)> = self
            .reviewed_mcp_files()
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
            let change = match &review.trust {
                ServerTrust::Changed { changes } => Some(ItemChange::Parts(
                    changes.iter().map(ServerChange::describe).collect(),
                )),
                _ => None,
            };
            let item = ReviewItem {
                title: format!("{} · {}", review.name, review.config.transport_label()),
                detail: server_detail(&review.config),
                change,
                checked: true,
                key: ReviewKey::Server {
                    source: review.source.clone(),
                    name: review.name.clone(),
                },
            };
            let path = self.section_path(&review.source, home);
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
        for source in sources {
            let Some(file) = self
                .reviewed_mcp_files()
                .into_iter()
                .find(|(path, _)| path.display().to_string() == *source)
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
        let Some(path) = &self.store_path else {
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
    let mcp_file = |path: Option<PathBuf>| {
        let (path, fingerprint, text) = read(path?)?;
        Some(ProjectFile {
            parsed: text.and_then(|text| parse_mcp_document(&text)),
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

/// How the review, `/hooks` and `/mcp` name an installed plugin's file:
/// `plugin name@marketplace` (`docs/plugins.md`).
pub(crate) fn plugin_heading(id: &PluginId) -> String {
    format!("{}{id}", alter_zero::app::PLUGIN_SECTION_PREFIX)
}

/// An MCP file's parse, a document-level failure an `Err`: it parses to a
/// lone error and no servers — a broken file, not an empty one — while
/// per-entry errors keep the healthy entries (the mcp module's
/// leniently-but-loudly posture).
fn parse_mcp_document(text: &str) -> Result<mcp::McpFile, String> {
    let file = mcp::parse_mcp_file(text);
    match (file.servers.is_empty(), file.errors.first()) {
        (true, Some(error)) => Err(error.clone()),
        _ => Ok(file),
    }
}

/// The largest generated plugin file read — far past any real one; a file
/// past it is reported, never half-read.
const PLUGIN_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// The enabled plugins as the session launches with them
/// (`docs/plugins.md`): `plugins.json` read and its enabled records
/// resolved under `{config_home}/plugins`. Empty — after one failed open,
/// and nothing allocated — when there is no `plugins.json`, and empty
/// without reading anything under `ALTER_ZERO_PLUGINS=0` or with no config
/// home. A file that will not parse is empty too: its toast is the plugin
/// snapshot's.
pub(crate) fn plugins_at_launch(config_home: Option<&Path>) -> Vec<EnabledPlugin> {
    plugins_from(config_home, config::env_flag("ALTER_ZERO_PLUGINS"))
}

/// [`plugins_at_launch`] with the switch passed in.
fn plugins_from(config_home: Option<&Path>, enabled: bool) -> Vec<EnabledPlugin> {
    use std::io::Read;
    let Some(home) = config_home.filter(|_| enabled) else {
        return Vec::new();
    };
    let Ok(file) = std::fs::File::open(alter_zero::plugins::state_path(home)) else {
        return Vec::new();
    };
    let mut text = String::new();
    let read = file
        .take(PLUGIN_FILE_MAX_BYTES + 1)
        .read_to_string(&mut text);
    if read.is_err() || text.len() as u64 > PLUGIN_FILE_MAX_BYTES {
        return Vec::new();
    }
    match alter_zero::plugins::PluginsFile::parse(&text) {
        Ok(state) => {
            alter_zero::plugins::enabled_plugins(&state, &alter_zero::plugins::plugins_root(home))
        }
        Err(_) => Vec::new(),
    }
}

/// Read each enabled plugin's generated hooks and MCP files — once, at
/// bootstrap, like the project layer: a plugin installed, changed or removed
/// mid-session counts from the next launch (`docs/plugins.md`). `None` when
/// no plugin has either file, so a session without plugins holds nothing. A
/// hooks file that will not parse costs that plugin's hooks alone
/// ([`TrustState::plugin_hooks_errors`]); a broken MCP file, its servers.
pub(crate) fn load_plugin_layer(
    config_home: Option<&Path>,
    plugins: &[EnabledPlugin],
) -> Option<PluginLayer> {
    let home = config_home?;
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    let read = |path: &Path| alter_zero::plugins::read_capped(path, PLUGIN_FILE_MAX_BYTES);
    let files: Vec<PluginFiles> = plugins
        .iter()
        .filter(|plugin| plugin.hooks_file.is_some() || plugin.mcp_file.is_some())
        .map(|plugin| PluginFiles {
            id: plugin.id.clone(),
            hooks: plugin.hooks_file.as_ref().map(|path| PluginFile {
                // Every handler remembers its plugin's file, so the runner
                // gives it the plugin's variables and a verdict names it, and
                // its plugin, so `/hooks` does.
                parsed: read(path).and_then(|text| {
                    hooks::HooksFile::parse(&text).map(|file| {
                        file.with_source(path).with_origin(
                            &plugin_heading(&plugin.id),
                            &alter_zero::ui::display_cwd(path, user_home.as_deref()),
                        )
                    })
                }),
                path: path.clone(),
            }),
            mcp: plugin.mcp_file.as_ref().map(|path| PluginFile {
                parsed: read(path).and_then(|text| parse_mcp_document(&text)),
                path: path.clone(),
            }),
        })
        .collect();
    (!files.is_empty()).then(|| PluginLayer {
        root: alter_zero::plugins::plugins_root(home),
        files,
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
        for message in self.trust.plugin_hooks_errors() {
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
    /// user's MCP file. `false` when `name` is not such a server the session
    /// knows, so the caller applies the user file's toggle instead.
    pub(crate) fn set_project_server(&mut self, name: &str, enabled: bool) -> bool {
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

    fn server_reviews_named(&self, name: &str) -> Option<ServerReview> {
        self.trust
            .server_reviews()
            .into_iter()
            .find(|review| review.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alter_zero::plugins::{EnabledPlugin, GENERATED_HOOKS, GENERATED_MCP, PluginId};

    const PLUGINS_ROOT: &str = "/home/u/.alter-zero/plugins";

    fn plugin_path(name: &str, file: &str) -> PathBuf {
        Path::new(PLUGINS_ROOT)
            .join("cache/m")
            .join(name)
            .join(".alter-zero-plugin")
            .join(file)
    }

    fn hooks_at(path: &Path, command: &str) -> hooks::HooksFile {
        let json = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":{}}}]}}]}}}}"#,
            serde_json::to_string(command).unwrap()
        );
        hooks::HooksFile::parse(&json).unwrap().with_source(path)
    }

    fn plugin(name: &str, hooks: Option<&str>, mcp: Option<&str>) -> PluginFiles {
        PluginFiles {
            id: PluginId::new(name, "m").unwrap(),
            hooks: hooks.map(|command| {
                let path = plugin_path(name, GENERATED_HOOKS);
                PluginFile {
                    parsed: Ok(hooks_at(&path, command)),
                    path,
                }
            }),
            mcp: mcp.map(|json| PluginFile {
                path: plugin_path(name, GENERATED_MCP),
                parsed: Ok(mcp::parse_mcp_file(json)),
            }),
        }
    }

    fn state(user: Option<&str>, plugins: Vec<PluginFiles>) -> TrustState {
        let user_path = PathBuf::from("/home/u/.alter-zero/hooks.json");
        TrustState {
            user_hooks: user.map(|command| (user_path.clone(), hooks_at(&user_path, command))),
            plugins: Some(PluginLayer {
                root: PathBuf::from(PLUGINS_ROOT),
                files: plugins,
            }),
            ..TrustState::default()
        }
    }

    fn source(name: &str, file: &str) -> String {
        plugin_path(name, file).display().to_string()
    }

    #[test]
    fn plugin_hooks_are_a_layer_after_the_users_in_id_order() {
        let state = state(
            Some("./mine.sh"),
            vec![
                plugin("alpha", Some("./a.sh"), None),
                plugin("beta", None, Some(r#"{"mcpServers": {}}"#)),
                plugin("gamma", Some("./g.sh"), None),
            ],
        );
        let layers: Vec<(String, &str)> = state
            .hook_layers()
            .into_iter()
            .map(|(path, _, scope)| (path, scope))
            .collect();
        assert_eq!(
            layers,
            [
                ("/home/u/.alter-zero/hooks.json".to_string(), "User"),
                (source("alpha", GENERATED_HOOKS), "Plugin"),
                (source("gamma", GENERATED_HOOKS), "Plugin"),
            ]
        );
        // `/hooks` and the `/settings` row count every one of them.
        let configured = state.configured_hooks();
        assert_eq!(configured.hooks["PreToolUse"].len(), 3);
    }

    #[test]
    fn a_plugin_hook_runs_only_once_trusted() {
        let mut state = state(None, vec![plugin("review", Some("./check.sh"), None)]);
        assert!(state.runner_hooks().is_empty(), "untrusted: nothing runs");
        let source = source("review", GENERATED_HOOKS);
        let identity = trust::runnable_hooks(&state.hook_layers()[0].1.clone())[0].clone();
        state
            .record_hooks(&source, std::slice::from_ref(&identity), &[])
            .unwrap();
        let runs = state.runner_hooks();
        assert_eq!(runs.hooks["PreToolUse"].len(), 1);
        assert_eq!(
            runs.hooks["PreToolUse"][0].source.as_deref(),
            Some(plugin_path("review", GENERATED_HOOKS).as_path()),
            "the handler keeps its plugin file, so it runs with the plugin's variables"
        );
    }

    #[test]
    fn the_review_lists_a_new_plugin_hook_under_its_plugin() {
        let state = state(None, vec![plugin("review", Some("./check.sh --all"), None)]);
        let page = state
            .hooks_page(Some(Path::new("/home/u")))
            .expect("a page");
        assert_eq!(page.sections.len(), 1);
        let section = &page.sections[0];
        assert_eq!(section.path, "plugin review@m");
        assert_eq!(section.scope, None);
        assert_eq!(section.items[0].title, "PreToolUse · Bash");
        assert_eq!(section.items[0].detail, ["./check.sh --all"]);
        assert_eq!(section.items[0].change, None);
    }

    #[test]
    fn an_update_that_changes_a_plugin_hooks_command_shows_the_old_one() {
        let mut old = state(None, vec![plugin("review", Some("./check.sh"), None)]);
        let source = source("review", GENERATED_HOOKS);
        let identity = trust::runnable_hooks(&old.hook_layers()[0].1.clone())[0].clone();
        old.record_hooks(&source, &[identity], &[]).unwrap();
        // The same install path after an update: the store carries over.
        let mut updated = state(None, vec![plugin("review", Some("./check.sh --v2"), None)]);
        updated.store = old.store.clone();
        assert!(updated.runner_hooks().is_empty(), "a changed command waits");
        let page = updated.hooks_page(None).expect("a page");
        assert_eq!(
            page.sections[0].items[0].change,
            Some(ItemChange::Was("./check.sh".to_string()))
        );
        // An update that keeps the command asks nothing.
        let mut same = state(None, vec![plugin("review", Some("./check.sh"), None)]);
        same.store = old.store;
        assert!(same.hooks_page(None).is_none());
        assert!(!same.runner_hooks().is_empty());
    }

    #[test]
    fn plugin_servers_are_reviewed_under_their_plugin_and_recorded_by_path() {
        let mut state = state(
            None,
            vec![plugin(
                "review",
                None,
                Some(r#"{"mcpServers": {"plugin_review_lint": {"command": "lint"}}}"#),
            )],
        );
        let reviews = state.server_reviews();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].source, source("review", GENERATED_MCP));
        assert!(reviews[0].trust.needs_review());
        let page = state
            .mcp_page(&["plugin_review_lint".to_string()], None)
            .expect("a page");
        assert_eq!(page.sections[0].path, "plugin review@m");
        state
            .record_servers(&[(
                source("review", GENERATED_MCP),
                "plugin_review_lint".to_string(),
                true,
            )])
            .unwrap();
        assert!(state.server_reviews()[0].trust.approved());
    }

    #[test]
    fn a_layer_read_from_disk_names_each_hook_by_its_plugin_for_hooks() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("plugins.json"),
            r#"{"plugins": {"review@m": {"enabled": true, "hooks": true}}}"#,
        )
        .unwrap();
        let dir = alter_zero::plugins::plugins_root(home.path())
            .join("cache/m/review/.alter-zero-plugin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(GENERATED_HOOKS),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./p.sh"}]}]}}"#,
        )
        .unwrap();
        let plugins = plugins_from(Some(home.path()), true);
        let state = TrustState {
            plugins: load_plugin_layer(Some(home.path()), &plugins),
            ..TrustState::default()
        };
        let overview = hooks::HooksOverview::from_file(&state.configured_hooks());
        let stop = overview
            .events
            .iter()
            .find(|event| event.event == hooks::HookEvent::Stop)
            .unwrap();
        let (label, _) = stop.matchers[0].hooks[0]
            .origin
            .clone()
            .expect("a plugin's hook");
        assert_eq!(label, "plugin review@m");
    }

    #[test]
    fn no_plugins_is_no_layer_and_nothing_read() {
        let home = tempfile::tempdir().unwrap();
        assert!(plugins_from(Some(home.path()), true).is_empty());
        assert!(plugins_from(None, true).is_empty());
        assert!(load_plugin_layer(Some(home.path()), &[]).is_none());
        assert!(
            TrustState::default()
                .plugin_label(Path::new("/x"))
                .is_none()
        );
    }

    #[test]
    fn the_enabled_plugins_are_read_from_plugins_json_unless_switched_off() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("plugins.json"),
            r#"{"plugins": {"review@m": {"enabled": true, "hooks": true},
                "off@m": {"enabled": false, "hooks": true}}}"#,
        )
        .unwrap();
        let plugins: Vec<EnabledPlugin> = plugins_from(Some(home.path()), true);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id.to_string(), "review@m");
        assert!(plugins_from(Some(home.path()), false).is_empty());
        std::fs::write(home.path().join("plugins.json"), "{not json").unwrap();
        assert!(plugins_from(Some(home.path()), true).is_empty());
    }

    #[test]
    fn a_plugin_layer_reads_each_generated_file_and_reports_a_broken_one() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("plugins.json"),
            r#"{"plugins": {"good@m": {"enabled": true, "hooks": true, "mcp": true},
                "bad@m": {"enabled": true, "hooks": true}}}"#,
        )
        .unwrap();
        let plugins = plugins_from(Some(home.path()), true);
        let root = alter_zero::plugins::plugins_root(home.path());
        for (name, hooks) in [("good", r#"{"hooks": {}}"#), ("bad", "{broken")] {
            let dir = root.join("cache/m").join(name).join(".alter-zero-plugin");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(GENERATED_HOOKS), hooks).unwrap();
        }
        std::fs::write(
            root.join("cache/m/good/.alter-zero-plugin")
                .join(GENERATED_MCP),
            r#"{"mcpServers": {"plugin_good_db": {"command": "db"}}}"#,
        )
        .unwrap();
        let layer = load_plugin_layer(Some(home.path()), &plugins).expect("a layer");
        assert_eq!(layer.root, root);
        let ids: Vec<String> = layer.files.iter().map(|f| f.id.to_string()).collect();
        assert_eq!(ids, ["bad@m", "good@m"]);
        assert!(layer.files[0].hooks.as_ref().unwrap().parsed.is_err());
        assert!(layer.files[1].hooks.as_ref().unwrap().parsed.is_ok());
        let servers = &layer.files[1]
            .mcp
            .as_ref()
            .unwrap()
            .parsed
            .as_ref()
            .unwrap();
        assert_eq!(servers.servers[0].0, "plugin_good_db");
        let state = TrustState {
            plugins: Some(layer),
            ..TrustState::default()
        };
        let errors = state.plugin_hooks_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("plugin bad@m: "), "{}", errors[0]);
    }
}

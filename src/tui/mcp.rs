//! The `/mcp` manager's boundary half (`docs/tools/mcp.md`): building the
//! [`McpManager`] from the config files at bootstrap, opening the menu over
//! its snapshot, applying the menu's typed operations, and folding the MCP
//! event channel's reports back into the app.

use std::collections::BTreeSet;
use std::path::Path;

use alter_zero::app::{McpOp, ToastKind};
use alter_zero::llm::mcp::{McpEvent, McpHold, McpSources};
use alter_zero::mcp;

use super::Session;
use super::config;

/// Gather everything the manager is built from: the user file parsed here,
/// the project files taken from the bootstrap-read project layer, scopes
/// merged approval-aware (`docs/state/config-review.md`) — an enabled project
/// server shadows the user's, one waiting for the review or declined is
/// listed but held — plus this project's disabled set, the token-store path,
/// and the timeout knobs. Pure parse/merge over boundary reads. Without a
/// project layer (`ALTER_ZERO_PROJECT_CONFIG` off) no project file
/// contributes at all.
pub(crate) fn load_mcp_sources(cwd: &Path, trust: &super::trust::TrustState) -> McpSources {
    let project = cwd.display().to_string();
    let user_path = config::mcp_user_file_path();
    let user_contents = user_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let user_file = user_contents.as_deref().map(mcp::parse_mcp_file);
    let disabled = user_contents
        .as_deref()
        .map(|contents| mcp::parse_disabled(contents, &project))
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let user_display = user_path
        .as_deref()
        .map(|path| alter_zero::ui::display_cwd(path, home.as_deref()))
        .unwrap_or_else(|| "~/.alter-zero/mcp.json".to_string());
    // Where each project server stands with the review, and so which of
    // each file's servers may run.
    let reviews = trust.server_reviews();
    let approved = |path: &Path| -> BTreeSet<String> {
        let source = path.display().to_string();
        reviews
            .iter()
            .filter(|review| review.source == source && review.trust.approved())
            .map(|review| review.name.clone())
            .collect()
    };
    // The project files: `(parse, display path, enabled names)` per file
    // that parsed, plus its parse errors for the toast.
    let mut errors = Vec::new();
    let scoped = |file: Option<&super::trust::ProjectFile<mcp::McpFile>>,
                  errors: &mut Vec<String>| {
        let file = file?;
        let display = alter_zero::ui::display_cwd(&file.path, home.as_deref());
        match &file.parsed {
            Ok(parsed) => {
                errors.extend(
                    parsed
                        .errors
                        .iter()
                        .map(|error| format!("{display}: {error}")),
                );
                Some((parsed.clone(), display, approved(&file.path)))
            }
            Err(error) => {
                errors.push(format!("{display}: {error}"));
                None
            }
        }
    };
    let layer = trust.project.as_ref();
    let alter = scoped(layer.and_then(|l| l.mcp_alter.as_ref()), &mut errors);
    let compat = scoped(layer.and_then(|l| l.mcp_compat.as_ref()), &mut errors);
    if let Some(file) = user_file.as_ref() {
        errors.extend(
            file.errors
                .iter()
                .map(|error| format!("{user_display}: {error}")),
        );
    }
    let (entries, held_names) = mcp::merge_project_scopes(
        alter
            .as_ref()
            .map(|(file, display, enabled)| (file, display.as_str(), enabled)),
        compat
            .as_ref()
            .map(|(file, display, enabled)| (file, display.as_str(), enabled)),
        user_file.as_ref().map(|file| (file, user_display.as_str())),
    );
    // A held server waits for the review when it is new or changed, and is
    // disabled when the user said so.
    let held = held_names
        .into_iter()
        .map(|name| {
            let review = reviews
                .iter()
                .any(|r| r.name == name && r.trust.needs_review());
            let hold = if review {
                McpHold::Review
            } else {
                McpHold::Declined
            };
            (name, hold)
        })
        .collect();
    // The era cache is retired — a connect re-derives the verdict every
    // time now, so the file only ever misleads (`docs/tools/mcp.md`).
    config::remove_retired_mcp_era_cache();
    // Every provider's key variable — what a project's remote server may not
    // have expanded into its URL or headers.
    let (providers, _) = config::startup_providers(home.as_deref());
    let protected_env = providers
        .providers
        .keys()
        .map(|id| config::key_env_name(&providers, id))
        .collect();
    McpSources {
        entries,
        errors,
        disabled,
        held,
        project,
        user_file: user_path,
        auth_path: config::mcp_auth_path(),
        cwd: Some(cwd.to_path_buf()),
        // This binary, for the tier that detaches a stdio server where no
        // `setsid` binary exists (`docs/tools/tty-detach.md`) — the shells' helper.
        detach_helper: std::env::current_exe().ok(),
        startup_timeout: config::mcp_startup_timeout(),
        tool_timeout: config::mcp_tool_timeout(),
        open_browser: None,
        protected_env,
    }
}

impl Session<'_> {
    /// `/mcp`: open the manager over the live snapshot — the
    /// `open_skills_menu` injection seam. It opens with MCP off too (the
    /// `/settings` **MCP** row), the `/hooks` and `/skills` rule: the list
    /// says so, and each server's own switch still moves.
    pub(crate) fn open_mcp_menu(&mut self) {
        let snapshot = self.mcp.snapshot();
        self.app.open_mcp_menu(snapshot);
    }

    /// The `/settings` **MCP** row (`docs/tools/mcp.md`): connect every enabled
    /// server, or stop them all — each keeps its own `/mcp` switch either
    /// way. The offered tools and the servers' instructions follow at once
    /// (`refresh_mcp` rebuilds the next turn's backend when the set moved;
    /// servers that connect later rebuild it as their events land), and
    /// turned on over project servers that wait for the startup review, the
    /// review asks now — the **Hooks** row's rule (`docs/state/config-review.md`).
    pub(crate) fn apply_mcp_setting(&mut self, on: bool) {
        self.mcp.set_active(on);
        self.models.refresh_mcp();
        self.sync_listings();
        if on {
            self.open_servers_review();
        }
    }

    /// Apply one `/mcp` operation against the live manager. Connection work
    /// runs on the manager's worker threads — nothing here blocks the loop.
    pub(crate) fn apply_mcp_op(&mut self, op: McpOp) {
        let manager = self.mcp.clone();
        match op {
            McpOp::Reconnect { server } => manager.reconnect(&server),
            McpOp::SetDisabled { server, disabled } => {
                let state = if disabled { "disabled" } else { "enabled" };
                // A project server's on/off is the review's decision, kept in
                // trust.json (`docs/state/config-review.md`); a user server's is the
                // user file's per-directory set.
                if !self.set_project_server(&server, !disabled)
                    && let Err(error) = manager.set_disabled(&server, disabled)
                {
                    self.toast(
                        format!("{server} {state} for this session only — {error}"),
                        ToastKind::Error,
                    );
                    return;
                }
                self.toast(format!("{server} {state}"), ToastKind::Info);
            }
            McpOp::Authenticate { server } => manager.authenticate(&server),
            McpOp::ClearAuth { server } => match manager.clear_auth(&server) {
                Ok(()) => self.toast(
                    format!("Cleared authentication for {server}"),
                    ToastKind::Info,
                ),
                Err(error) => self.toast(
                    format!("Couldn't clear authentication for {server}: {error}"),
                    ToastKind::Error,
                ),
            },
            McpOp::CancelAuth => manager.cancel_auth(),
            McpOp::CopyAuthUrl { url } => {
                // The `/copy` seam: arboard with the OSC 52 fallback
                // (`docs/ui/copy.md`), so the copy works over SSH too.
                match alter_zero::clipboard::copy_to_clipboard(&url) {
                    Ok(lease) => {
                        if let Some(lease) = lease {
                            self.clipboard_lease = Some(lease);
                        }
                        self.toast("Copied authorization URL", ToastKind::Info);
                    }
                    Err(_) => self.toast("Copy failed", ToastKind::Error),
                }
            }
            McpOp::SubmitAuthUrl { text } => manager.submit_auth_paste(&text),
        }
    }

    /// One MCP event off the channel: re-inject the snapshot (the open menu
    /// re-clamps in place), fold auth progress into the auth page, and
    /// re-attach the backend when the offered tool set flipped.
    pub(crate) fn on_mcp_event(&mut self, event: McpEvent) {
        match event {
            McpEvent::Changed => {}
            McpEvent::AuthUrl { server, url } => {
                self.app.set_mcp_auth_url(&server, &url);
            }
            McpEvent::AuthPasteRejected { server, reason } => {
                self.app.reject_mcp_auth_paste(&server, &reason);
            }
            McpEvent::AuthDone { server, ok, detail } => {
                self.app.finish_mcp_auth(&server);
                let kind = if ok {
                    ToastKind::Info
                } else {
                    ToastKind::Error
                };
                let text = if ok {
                    detail
                } else {
                    format!("{server}: {detail}")
                };
                self.toast(text, kind);
            }
        }
        self.app.update_mcp_snapshot(self.mcp.snapshot());
        // The offered tool set may have flipped (a server connected, failed,
        // was disabled) — re-attach the backend only when it really did (the
        // `refresh_skills` pattern) — and with it the servers' instructions
        // the reminder carries.
        self.models.refresh_mcp();
        self.sync_listings();
        self.frame.schedule_frame();
    }

    /// Say that an `mcp.json` entry could not be parsed — at startup, and
    /// again when the `/settings` **MCP** row turns MCP on — the hooks-file
    /// rule: a server that vanishes must never do it quietly
    /// (`docs/tools/mcp.md`). Nothing while MCP is off, when no server is meant to
    /// run.
    pub(crate) fn report_mcp_errors(&mut self) {
        if !self.mcp.is_active() {
            return;
        }
        let errors = self.mcp.config_errors();
        if let Some(first) = errors.first() {
            let extra = errors.len().saturating_sub(1);
            let text = if extra > 0 {
                format!("MCP config: {first} (+{extra} more)")
            } else {
                format!("MCP config: {first}")
            };
            self.toast(text, ToastKind::Error);
        }
    }
}

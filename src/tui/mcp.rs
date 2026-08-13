//! The `/mcp` manager's boundary half (`docs/mcp.md`): building the
//! [`McpManager`] from the config files at bootstrap, opening the menu over
//! its snapshot, applying the menu's typed operations, and folding the MCP
//! event channel's reports back into the app.

use std::path::Path;

use alter_zero::app::{McpOp, ToastKind};
use alter_zero::llm::mcp::{McpEvent, McpSources};
use alter_zero::mcp;

use super::Session;
use super::config;

/// Gather everything the manager is built from: the user file parsed here,
/// the project files taken from the bootstrap-loaded [`ProjectLayer`]
/// snapshot (each already fingerprinted and trust-checked,
/// `docs/project-config.md`), scopes merged trust-aware — a trusted project
/// file shadows the user, an untrusted one is listed but held — plus this
/// project's disabled set, the token-store path, and the timeout knobs.
/// Pure parse/merge over boundary reads. Without a layer
/// (`ALTER_ZERO_PROJECT_CONFIG` off) no project file contributes at all.
pub(crate) fn load_mcp_sources(
    cwd: &Path,
    layer: Option<&super::trust::ProjectLayer>,
) -> McpSources {
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
    // The project files ride the trust layer's snapshot: `(parse, display
    // path, trusted)` per present file, plus its parse errors for the toast.
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
                Some((parsed.clone(), display, file.trusted))
            }
            Err(error) => {
                errors.push(format!("{display}: {error}"));
                None
            }
        }
    };
    let alter = scoped(layer.and_then(|l| l.mcp_alter.as_ref()), &mut errors);
    let compat = scoped(layer.and_then(|l| l.mcp_compat.as_ref()), &mut errors);
    if let Some(file) = user_file.as_ref() {
        errors.extend(
            file.errors
                .iter()
                .map(|error| format!("{user_display}: {error}")),
        );
    }
    let (entries, untrusted) = mcp::merge_project_scopes(
        alter
            .as_ref()
            .map(|(file, display, trusted)| (file, display.as_str(), *trusted)),
        compat
            .as_ref()
            .map(|(file, display, trusted)| (file, display.as_str(), *trusted)),
        user_file.as_ref().map(|file| (file, user_display.as_str())),
    );
    McpSources {
        entries,
        errors,
        disabled,
        untrusted,
        project,
        user_file: user_path,
        auth_path: config::mcp_auth_path(),
        cwd: Some(cwd.to_path_buf()),
        startup_timeout: config::mcp_startup_timeout(),
        tool_timeout: config::mcp_tool_timeout(),
    }
}

impl Session<'_> {
    /// `/mcp`: open the manager over the live snapshot — the
    /// `open_skills_menu` injection seam. Without a manager
    /// (`ALTER_ZERO_MCP` off) the command explains via toast instead.
    pub(crate) fn open_mcp_menu(&mut self) {
        let Some(manager) = &self.mcp else {
            self.toast(
                "MCP is disabled (ALTER_ZERO_MCP). Unset it to manage servers.",
                ToastKind::Info,
            );
            return;
        };
        let snapshot = manager.snapshot();
        self.app.open_mcp_menu(snapshot);
    }

    /// Apply one `/mcp` operation against the live manager. Connection work
    /// runs on the manager's worker threads — nothing here blocks the loop.
    pub(crate) fn apply_mcp_op(&mut self, op: McpOp) {
        let Some(manager) = self.mcp.clone() else {
            return;
        };
        match op {
            McpOp::Reconnect { server } => manager.reconnect(&server),
            McpOp::SetDisabled { server, disabled } => {
                manager.set_disabled(&server, disabled);
                let state = if disabled { "disabled" } else { "enabled" };
                self.toast(format!("{server} {state}"), ToastKind::Info);
            }
            McpOp::Authenticate { server } => manager.authenticate(&server),
            McpOp::ClearAuth { server } => {
                manager.clear_auth(&server);
                self.toast(
                    format!("Cleared authentication for {server}"),
                    ToastKind::Info,
                );
            }
            McpOp::CancelAuth => manager.cancel_auth(),
            McpOp::CopyAuthUrl { url } => {
                // The `/copy` seam: arboard with the OSC 52 fallback
                // (`docs/copy.md`), so the copy works over SSH too.
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
            McpEvent::AuthDone { server, ok, detail } => {
                self.app.finish_mcp_auth();
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
        if let Some(manager) = &self.mcp {
            self.app.update_mcp_snapshot(manager.snapshot());
        }
        // The offered tool set may have flipped (a server connected, failed,
        // was disabled) — re-attach the backend only when it really did (the
        // `refresh_skills` pattern).
        self.models.refresh_mcp();
        self.frame.schedule_frame();
    }

    /// Say once, at startup, that an `mcp.json` entry could not be parsed —
    /// the hooks-file rule: a server that vanishes must never do it quietly
    /// (`docs/mcp.md`).
    pub(crate) fn report_mcp_errors(&mut self) {
        let Some(manager) = &self.mcp else { return };
        let errors = manager.config_errors();
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

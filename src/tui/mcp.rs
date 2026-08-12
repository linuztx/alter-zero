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

/// Gather everything the manager is built from: both config files parsed,
/// scopes merged (project shadows user), this project's disabled set, the
/// token-store path, and the timeout knobs. Pure parse over boundary reads.
pub(crate) fn load_mcp_sources(cwd: &Path) -> McpSources {
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
    // The project file sits at the nearest-`.git` root (the `AGENTS.md`
    // walk-up), so launching in `repo/src` still finds the repo's servers.
    let project_root =
        alter_zero::project_doc::find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let project_path = project_root.join(".mcp.json");
    let project_contents = std::fs::read_to_string(&project_path).ok();
    let project_file = project_contents.as_deref().map(mcp::parse_mcp_file);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let user_display = user_path
        .as_deref()
        .map(|path| alter_zero::ui::display_cwd(path, home.as_deref()))
        .unwrap_or_else(|| "~/.alter-zero/mcp.json".to_string());
    let project_display = alter_zero::ui::display_cwd(&project_path, home.as_deref());
    let mut errors = Vec::new();
    for (file, origin) in [
        (project_file.as_ref(), project_display.as_str()),
        (user_file.as_ref(), user_display.as_str()),
    ] {
        if let Some(file) = file {
            errors.extend(file.errors.iter().map(|error| format!("{origin}: {error}")));
        }
    }
    let entries = mcp::merge_scopes(
        project_file
            .as_ref()
            .map(|file| (file, project_display.as_str())),
        user_file.as_ref().map(|file| (file, user_display.as_str())),
    );
    McpSources {
        entries,
        errors,
        disabled,
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

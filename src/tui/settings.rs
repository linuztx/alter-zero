//! The `/settings` menu at the boundary: opening it, and **applying** each
//! knob the pure core moved (`docs/settings.md`).
//!
//! The pure side ([`alter_zero::app::App::settings`]) only ever holds the
//! value; everything that has to *happen* when one changes lives here — the
//! backend rebuild, the checkpoint store's flip, the project-doc reload, the
//! file write, the confirming toast. `App` stays free of I/O, and every knob
//! has exactly one place its effect is spelled out.
//!
//! Two settings deliberately need nothing here: **Hide thinking** and **Auto
//! compact** are read straight off `App` where they are used
//! (`tui::stream`'s `ThinkingStart` arm and `App::should_auto_compact`), so
//! there is no second copy to keep in step.

use alter_zero::app::ToastKind;
use alter_zero::project_doc;
use alter_zero::settings::{SettingAvailability, SettingKey};

use super::{Session, config};

impl Session<'_> {
    /// `/settings`: open the inline menu. Nothing to fetch — the rows derive
    /// from state the loop already has — so this is only the open plus the
    /// availability the pure core can't know (whether this host can snapshot
    /// at all).
    pub(crate) fn open_settings(&mut self) {
        self.sync_setting_availability();
        self.app.open_settings();
    }

    /// `/hooks`: open the read-only hooks browser (`docs/hooks-menu.md`). The
    /// pure command returned the intent; this derives the data — the overview
    /// from the live `HookSetup` (the same parsed file the runner consults,
    /// so the browser and the dispatcher can never disagree), the `Source:`
    /// path display, and whether the session actually runs the hooks — and
    /// hands all three to `App`, the `/resume` picker's injection seam.
    pub(crate) fn open_hooks_menu(&mut self) {
        let (overview, enabled) = self.models.hooks_browse();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let source = alter_zero::llm::hooks::hooks_file_path(config::config_home().as_deref())
            .map(|path| alter_zero::ui::display_cwd(&path, home.as_deref()));
        self.app.open_hooks_menu(overview, source, enabled);
    }

    /// `/skills`: open the inline skills browser (`docs/skills.md`). The same
    /// injection seam as `/hooks`: the loop takes the registry's snapshot —
    /// **every** discovered skill plus the currently-off set, since a skill
    /// you turned off is the one you need to see to turn back on — along with
    /// whether the session-wide switch is on and where skills are looked for
    /// (the answer an empty list needs), and hands all four to `App`.
    pub(crate) fn open_skills_menu(&mut self) {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        // The same resolver the startup walk used, so the menu can only ever
        // name directories that were actually read.
        let roots = alter_zero::llm::skill::resolved_skill_roots(
            &self.cwd,
            config::config_home().as_deref(),
        )
        .iter()
        .map(|root| alter_zero::ui::display_cwd(root, home.as_deref()))
        .collect();
        self.app.open_skills_menu(
            self.skill_registry.snapshot(),
            self.skill_registry.disabled(),
            self.app.settings().skills_active(),
            roots,
        );
    }

    /// Apply the `/skills` menu's toggle: make it true of the running session
    /// and persist it for this project (`docs/skills.md`).
    ///
    /// Four things have to move together, which is why they are spelled out in
    /// one place: the shared registry (what the executor and the listing read),
    /// the re-rendered listing (what the next request carries), the backend
    /// (whether the `skill` tool is offered at all — turning the *last* skill
    /// off must withdraw it, not leave a tool that can only fail), and the
    /// file. The confirming toast is the `/settings` row's.
    pub(crate) fn apply_skill_toggle(&mut self, name: &str, enabled: bool) {
        // Set rather than flip: the menu already decided, and re-deriving the
        // state here is how the two copies would come to disagree.
        let mut disabled = self.skill_registry.disabled();
        if enabled {
            disabled.remove(name);
        } else {
            disabled.insert(name.to_string());
        }
        self.skill_registry.set_disabled(disabled.clone());
        self.sync_skill_listing();
        // The tool set is decided at attach time, so the rebuild is what
        // withdraws (or restores) the spec when the last skill goes off/on.
        self.models.refresh_skills();
        config::save_skills_file(
            config::skills_json_path().as_deref(),
            &self.cwd.display().to_string(),
            &disabled,
        );
        let state = if enabled { "enabled" } else { "disabled" };
        self.toast(format!("Skill {name}: {state}"), ToastKind::Info);
    }

    /// Push what this host can actually run into `App` (the `set_clock`
    /// pattern), so an unavailable row says so instead of offering a toggle
    /// that does nothing.
    pub(crate) fn sync_setting_availability(&mut self) {
        self.app.set_setting_availability(SettingAvailability {
            checkpoints: self.checkpoints.is_capable(),
            hooks: self.models.hooks_available(),
            // Nothing to toggle when no `SKILL.md` loaded (`docs/skills.md`).
            skills: !self.skill_registry.is_empty(),
        });
    }

    /// Apply a setting the menu just cycled: do whatever it takes to make the
    /// new value true of the running session, persist the file, and confirm
    /// with a transient toast.
    ///
    /// A rebuild here rebinds the **next** turn's backend — the running one
    /// streams on its own thread, untouched (the `/model` switch's rule).
    pub(crate) fn apply_setting(&mut self, key: SettingKey) {
        let settings = *self.app.settings();
        match key {
            // Offering (or withholding) the tools changes the request's shape,
            // so the backend is rebuilt around the new tool set.
            SettingKey::Tools => self.models.set_tools(settings.tools),
            // The retry budget rides every round of every turn — the main
            // one's and a subagent's.
            SettingKey::ErrorRetry => self.models.set_max_retries(settings.error_retry),
            SettingKey::Temperature => self.models.set_temperature(settings.temperature),
            // The tool-round ceiling a turn runs under (0 = none).
            SettingKey::MaxToolCalls => self.models.set_max_tool_calls(settings.max_tool_calls),
            // The store keeps its own enabled flag (it can only ever turn a
            // *capable* store on or off — docs/checkpoint.md).
            SettingKey::Checkpoints => {
                self.checkpoints.set_enabled(settings.checkpoints_active());
            }
            // Reload (or drop) the project's AGENTS.md right away, so Ctrl+D
            // shows the change before the next turn re-reads it.
            SettingKey::ProjectDocs => {
                let instructions = settings
                    .project_docs
                    .then(|| project_doc::load_user_instructions(&self.cwd))
                    .flatten();
                self.app.set_user_instructions(instructions);
            }
            // The hook sink is attached per backend build, so flipping the
            // row rebuilds — the next turn genuinely stops (or starts)
            // consulting them (docs/hooks.md).
            SettingKey::Hooks => self.models.set_hooks(settings.hooks_active()),
            // The tool rides the backend build and the listing rides the
            // context, so flipping the row does both — the next turn genuinely
            // stops (or starts) knowing about skills (`docs/skills.md`).
            SettingKey::Skills => {
                self.models.set_skills(settings.skills_active());
                self.sync_skill_listing();
            }
            // Read where they are used — nothing to rebuild.
            SettingKey::HideThinking | SettingKey::AutoCompact => {}
            // Ctrl+A's path owns this one; the menu never routes it here.
            SettingKey::PermissionMode => {}
        }
        // Persist as a read-modify-write over the blob the file itself holds,
        // moving across only the key the user cycled — so an `ALTER_ZERO_*`
        // override merged in at startup never sticks (`docs/settings.md`).
        self.saved_settings.copy_value(key, &settings);
        config::save_session_settings(self.settings_path.as_deref(), &self.saved_settings);
        let value = settings.value_text(key, self.app.permission_mode());
        self.toast(format!("{}: {value}", key.label()), ToastKind::Info);
    }
}

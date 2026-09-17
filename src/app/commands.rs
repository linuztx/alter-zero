//! The slash-command palette: the [`COMMANDS`] registry, the `/token` filter
//! behind it, and the [`App`](super::App) side of opening, scrolling, and
//! running the highlighted command.
//!
//! Each command's behaviour is documented where the feature lives —
//! `docs/copy.md`, `docs/init.md`, `docs/compact.md`, `docs/resume.md` — and a
//! mid-turn rejection surfaces as a toast rather than a scrollback bullet
//! (`docs/toast.md`).
//!
//! The registry is static, but the palette is not quite: the active model's
//! **speed tiers** each get a row of their own — `/fast`, `/ultrafast`,
//! whatever the listing names — spliced in after `/model` by
//! [`App::commands`], codex's `SlashCommandItem::ServiceTier`
//! (`docs/fast-mode.md`). Nothing about a tier is hardcoded here: the rows
//! are built from what the record said, so a tier the backend adds tomorrow
//! is a command the day it is listed.

use std::borrow::Cow;

use super::*;

/// The notice committed when `/copy` writes the last response to the clipboard —
/// codex's info event, verbatim. Recorded as a [`Role::System`] message. See
/// `docs/copy.md`.
pub const COPY_OK_NOTICE: &str = "Copied last message to clipboard";

/// The notice committed when `/copy` finds no assistant response to copy —
/// codex's error event, verbatim. Recorded as a [`Role::Error`] message. See
/// `docs/copy.md`.
pub const COPY_EMPTY_NOTICE: &str = "No agent response to copy";

/// The transient toast shown when `/resume` is run while a turn is active —
/// codex blocks the command mid-task (`slash_command_blocked_by_active_task`)
/// instead of racing the stream (it would swap the whole conversation). Shown as
/// an [`Action::Toast`] rather than a committed message — a soft rejection the
/// user needn't keep. See `docs/resume.md` / `docs/toast.md`.
pub const RESUME_BUSY_NOTICE: &str = "/resume is disabled while a task is in progress";

/// The transient toast shown when `/help` is run while a turn is active — its
/// multi-line command list would interleave with the streaming reply in
/// scrollback, so mid-turn it is rejected like `/resume` (idle it still commits
/// the full list). Shown as an [`Action::Toast`]. See `docs/toast.md`.
pub const HELP_BUSY_NOTICE: &str = "/help is disabled while a task is in progress";

/// Codex's `/init` prompt (`prompts/init.md`, its `prompt_for_init_command.md`
/// verbatim): generate an `AGENTS.md` contributor guide — never overwriting an
/// existing one. Submitted as a **regular user turn** ([`Action::Submit`]), so
/// the model's agentic tool loop does the exploring and writing; the prompt is
/// the whole feature. See `docs/init.md`.
pub const INIT_PROMPT: &str = include_str!("../../prompts/init.md");

/// The transient toast shown when `/init` is run while a turn is active —
/// codex disables it during a task (`available_during_task`); submitting would
/// race the running stream with a second turn. The `/compact` toast pattern.
/// See `docs/init.md` / `docs/toast.md`.
pub const INIT_BUSY_NOTICE: &str = "/init is disabled while a task is in progress";

/// The transient toast shown when `/compact` is run while a turn is active —
/// codex disables it during a task (`available_during_task`); ours rejects with
/// the `/help`/`/resume` toast pattern. See `docs/compact.md` / `docs/toast.md`.
pub const COMPACT_BUSY_NOTICE: &str = "/compact is disabled while a task is in progress";

/// The transient toast shown when `/compact` finds nothing to summarize — an
/// empty conversation would send the bare summarization prompt to the model
/// and "summarize" nothing. See `docs/compact.md`.
pub const COMPACT_EMPTY_NOTICE: &str = "Nothing to compact";

/// What running a slash command does. The palette dispatches one of these on
/// select; `App::run_selected_command` turns it into an [`Action`] for the loop.
/// Wiring a stub up later is just swapping its effect here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEffect {
    /// Clear the conversation history (`/clear`).
    Clear,
    /// Post the list of available commands as a system notice (`/help`) — or,
    /// while a turn is active, reject with a [`HELP_BUSY_NOTICE`] toast (its
    /// multi-line list would interleave with the streaming reply). See
    /// `docs/toast.md`.
    Help,
    /// Copy the last assistant response to the clipboard (`/copy`) — the
    /// confirmation surfaces as a toast, not a scrollback message. See
    /// `docs/copy.md` / `docs/toast.md`.
    Copy,
    /// Run codex's `/init`: submit the canned [`INIT_PROMPT`] as a regular
    /// user turn asking the model to generate an `AGENTS.md` contributor
    /// guide (`docs/init.md`) — or reject with an [`INIT_BUSY_NOTICE`] toast
    /// while a turn is active (codex's `available_during_task` is `false`).
    Init,
    /// Run codex's `/compact`: a summarization turn whose reply becomes the
    /// context bridge (`docs/compact.md`) — or reject with a
    /// [`COMPACT_BUSY_NOTICE`] toast while a turn is active (codex disables
    /// it mid-task) / a [`COMPACT_EMPTY_NOTICE`] toast when the derived
    /// context is empty.
    Compact,
    /// Open the `/resume` session picker — or reject with a
    /// [`RESUME_BUSY_NOTICE`] toast while a turn is active (codex blocks it
    /// mid-task; it swaps the whole conversation). See `docs/resume.md`.
    Resume,
    /// Open the inline `/model` picker. Works **mid-turn** — it only replaces the
    /// composer, never the running turn (which streams on its own thread); a
    /// switch only rebinds the *next* turn's backend. See `docs/llm.md` /
    /// `docs/toast.md`.
    Model,
    /// Toggle one of the active model's **speed tiers** — codex's per-tier
    /// commands, `/fast`, `/ultrafast`, … (`docs/fast-mode.md`). The row
    /// exists only while the model's record lists the tier, and running it
    /// selects the tier — or, when it is the selection already, standard —
    /// riding the *next* request as `service_tier`, so it works mid-turn
    /// exactly as Ctrl+T does. The tier rides the effect so the row is
    /// self-describing: it is the one effect no static registry entry
    /// carries, since the rows are built from the listing.
    ServiceTier(ServiceTier),
    /// Open the inline `/login` API-key onboarding flow. Works **mid-turn** like
    /// `/model` — saving a key never touches the running turn. See `docs/llm.md`
    /// / `docs/toast.md`.
    Login,
    /// Open the inline `/settings` menu — the session's togglable knobs.
    /// Works **mid-turn** like `/model`: it only replaces the composer, and a
    /// change that rebuilds the backend rebinds the *next* turn. See
    /// `docs/settings.md`.
    Settings,
    /// Open the read-only `/hooks` browser over the configured lifecycle
    /// hooks. Works **mid-turn** like `/model` — browsing touches nothing.
    /// See `docs/hooks-menu.md`.
    Hooks,
    /// Open the inline `/skills` browser: every discovered skill, each one
    /// enable/disable-able. Works **mid-turn** like `/hooks` — the menu only
    /// replaces the composer, and a toggle binds the *next* turn's request.
    /// See `docs/skills.md`.
    Skills,
    /// Open the inline `/theme` picker: the colour themes, previewed on real
    /// cells. Works **mid-turn** like `/settings` — it only replaces the
    /// composer. See `docs/theme.md`.
    Theme,
    /// Open the inline `/mascot` picker: the banner mascots, previewed live.
    /// Works **mid-turn** like `/settings` — it only replaces the composer.
    /// See `docs/mascot.md`.
    Mascot,
    /// Open the inline `/spinner` picker: the status line's spinner styles,
    /// previewed live. Works **mid-turn** like `/mascot` — it only replaces
    /// the composer. See `docs/spinner.md`.
    Spinner,
    /// Open the inline `/mcp` manager: every declared MCP server, its live
    /// status, tools, and the authenticate/reconnect/disable operations.
    /// Works **mid-turn** like `/hooks` — it only replaces the composer;
    /// connection work runs on worker threads either way. See `docs/mcp.md`.
    Mcp,
    /// Open the `/trust` review menu over the project's `.alter-zero`
    /// config layer. Works **mid-turn** like `/hooks` — it only replaces
    /// the composer; an approval rebinds the *next* turn's hooks and
    /// connects servers on worker threads. See `docs/project-config.md`.
    Trust,
    /// Open the read-only `/donate` page: the project's crypto donation
    /// addresses, each one copyable. Works **mid-turn** like `/hooks` — it
    /// only replaces the composer. See `docs/donate.md`.
    Donate,
    /// Exit the app (`/quit` — codex's `/quit`/`/exit`, "exit Codex").
    Quit,
}

/// One entry in the slash-command palette: how it shows (`name`/`description`)
/// and what it does (`effect`). The static registry ([`COMMANDS`]) holds the
/// **built-in** rows, borrowed for the program's life; a **tier** row
/// ([`SlashCommand::tier`]) is built per session from what the active
/// model's listing says and owns its strings — hence the `Cow`s, which let
/// one type serve both without the registry giving up being a `const`.
/// Adding a built-in command is a one-line addition to [`COMMANDS`]; the
/// palette, filtering, and scrolling don't change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// The command name **without** the leading slash (e.g. `"help"`), lowercase.
    pub name: Cow<'static, str>,
    /// A one-line description shown dimmed beside the name in the palette.
    pub description: Cow<'static, str>,
    /// What selecting it does.
    pub effect: CommandEffect,
}

impl SlashCommand {
    /// A built-in row — what the [`COMMANDS`] table is made of.
    #[must_use]
    pub const fn builtin(
        name: &'static str,
        description: &'static str,
        effect: CommandEffect,
    ) -> Self {
        Self {
            name: Cow::Borrowed(name),
            description: Cow::Borrowed(description),
            effect,
        }
    }

    /// The row for one speed tier the active model lists — codex's
    /// `ServiceTierCommand`: named by [`ServiceTier::command_name`] (`fast`,
    /// `ultrafast`), described by the backend's own cost statement for the
    /// tier (`1.5x speed, increased usage` — the price of the speed said
    /// where it is bought, which is the half a user cannot see coming) or,
    /// when the record gave none, `Toggle {label} mode`, and carrying the
    /// tier as its effect. `None` for a tier whose name leaves no palette
    /// token at all.
    #[must_use]
    pub fn tier(tier: &ServiceTier) -> Option<Self> {
        let name = tier.command_name();
        if name.is_empty() {
            return None;
        }
        let description = if tier.description.is_empty() {
            format!("Toggle {} mode", tier.label())
        } else {
            tier.description.clone()
        };
        Some(Self {
            name: Cow::Owned(name),
            description: Cow::Owned(description),
            effect: CommandEffect::ServiceTier(tier.clone()),
        })
    }
}

/// The built-in slash commands, in the order they list in the palette. Adding
/// a command is a one-line entry here plus an effect arm in
/// `run_selected_command`; the palette, filtering, and scrolling don't change.
/// The speed-tier rows are deliberately **not** here: they are what the
/// active model lists, spliced in after `/model` by [`App::commands`].
pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand::builtin("help", "List the available commands", CommandEffect::Help),
    SlashCommand::builtin("clear", "Clear the conversation", CommandEffect::Clear),
    SlashCommand::builtin(
        "copy",
        "Copy the last response to the clipboard",
        CommandEffect::Copy,
    ),
    // Codex says "…with instructions for Codex" — the palette keeps its
    // descriptions concise and product-name-free.
    SlashCommand::builtin(
        "init",
        "Create an AGENTS.md contributor guide",
        CommandEffect::Init,
    ),
    // Codex's wording ("summarize conversation to prevent hitting the
    // context limit") was the palette's longest row; this says the same
    // thing inside the standard 80-column description room.
    SlashCommand::builtin(
        "compact",
        "Summarize the conversation to free up context",
        CommandEffect::Compact,
    ),
    SlashCommand::builtin("resume", "Resume a saved chat", CommandEffect::Resume),
    // The active model's speed tiers list right after this row
    // (`App::commands`).
    SlashCommand::builtin("model", "Switch the active model", CommandEffect::Model),
    SlashCommand::builtin(
        "login",
        "Add or update a provider API key",
        CommandEffect::Login,
    ),
    SlashCommand::builtin("settings", "Open settings menu", CommandEffect::Settings),
    SlashCommand::builtin("theme", "Choose the colour theme", CommandEffect::Theme),
    SlashCommand::builtin("mascot", "Choose the banner mascot", CommandEffect::Mascot),
    SlashCommand::builtin(
        "spinner",
        "Choose the status spinner style",
        CommandEffect::Spinner,
    ),
    SlashCommand::builtin(
        "hooks",
        "Browse the configured lifecycle hooks",
        CommandEffect::Hooks,
    ),
    SlashCommand::builtin(
        "skills",
        "Browse skills and enable or disable each one",
        CommandEffect::Skills,
    ),
    SlashCommand::builtin("mcp", "Manage MCP servers", CommandEffect::Mcp),
    SlashCommand::builtin(
        "trust",
        "Review and approve this project's config",
        CommandEffect::Trust,
    ),
    SlashCommand::builtin(
        "donate",
        "Support the project with a crypto donation",
        CommandEffect::Donate,
    ),
    SlashCommand::builtin("quit", "Exit the app", CommandEffect::Quit),
];

/// The open slash-command palette: which match row is highlighted. The matches
/// themselves are derived from the input on demand ([`matching_commands`]); only
/// the highlight is stored. `None` on [`App`] means the palette is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandMenu {
    /// Index of the highlighted command within the current filtered matches.
    pub selected: usize,
}

/// The slash-command query in `input`, if it is a **bare command token**: a
/// leading `/` followed by no whitespace (so `/`, `/he`, `/help` qualify, but
/// `ask /help`, `/help me`, and `/a\nb` do not — a space or newline ends it).
/// `Some("")` for a lone `/` (lists everything).
#[must_use]
pub fn command_query(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        None
    } else {
        Some(rest)
    }
}

/// The rows of `commands` whose name starts with `query` (case-insensitive),
/// in their order — the palette's filter over [`App::commands`], the
/// session's rows with the tier commands spliced in. An empty query matches
/// everything.
#[must_use]
pub fn matching_commands<'a>(commands: &'a [SlashCommand], query: &str) -> Vec<&'a SlashCommand> {
    let q = query.to_lowercase();
    commands.iter().filter(|c| c.name.starts_with(&q)).collect()
}

/// The `/help` notice: a header followed by every command's `/name — description`.
fn help_text(commands: &[SlashCommand]) -> String {
    let mut text = String::from("Available commands:");
    for cmd in commands {
        text.push_str(&format!("\n/{} — {}", cmd.name, cmd.description));
    }
    text
}

impl App {
    /// The palette's rows for this session: [`COMMANDS`] with one row per
    /// speed tier the active model lists spliced in **right after `/model`**
    /// — where codex inserts its tier commands, a tier being a fact about
    /// the model just switched to (`docs/fast-mode.md`). Built from
    /// [`App::speed`] on demand rather than stored, so the rows can never
    /// disagree with the state the footer and the next request read; a
    /// built-in's clone is a borrowed pointer copy, so a keystroke's rebuild
    /// costs nothing. A tier whose name leaves no palette token, or whose
    /// name is a listed command's already — a built-in must never be
    /// shadowed, `/model` being the door to everything else — gets no row.
    #[must_use]
    pub fn commands(&self) -> Vec<SlashCommand> {
        let tiers = self
            .speed
            .as_ref()
            .map_or(&[][..], |speed| speed.tiers.as_slice());
        let mut commands: Vec<SlashCommand> = Vec::with_capacity(COMMANDS.len() + tiers.len());
        for command in COMMANDS {
            commands.push(command.clone());
            if command.effect != CommandEffect::Model {
                continue;
            }
            for row in tiers.iter().filter_map(SlashCommand::tier) {
                let taken = |c: &SlashCommand| c.name == row.name;
                if !COMMANDS.iter().any(taken) && !commands.iter().any(taken) {
                    commands.push(row);
                }
            }
        }
        commands
    }

    /// Re-derive the palette after an edit. Opens it when the input *becomes* a
    /// command token, clamps the highlight when the filter narrows, and closes it
    /// when the input stops being a command token. The `had_query` flag (the state
    /// *before* the edit) makes Esc sticky: once dismissed, editing within the same
    /// token won't reopen the palette — only leaving and re-entering command mode
    /// (a None→Some transition) does.
    pub(super) fn refresh_command_menu(&mut self, had_query: bool) {
        // In shell mode the whole draft is a literal command — a leading `/`
        // there (e.g. `!/usr/bin/env`) is a path, never the palette.
        if self.shell_mode {
            self.command_menu = None;
            return;
        }

        match command_query(self.input.text()) {
            None => self.command_menu = None,
            Some(query) => {
                let matches = matching_commands(&self.commands(), query).len();
                match &mut self.command_menu {
                    Some(menu) => menu.selected = menu.selected.min(matches.saturating_sub(1)),
                    // Just entered command mode → open at the top.
                    None if !had_query => self.command_menu = Some(CommandMenu { selected: 0 }),
                    // Dismissed earlier and still in the same token → stay closed.
                    None => {}
                }
            }
        }
    }

    /// Move the palette highlight one step over the current matches, wrapping
    /// at the ends (`wrap_step` — ↓ past the last command comes back to the
    /// first, ↑ from the first to the last).
    pub(super) fn move_command_selection(&mut self, delta: isize) {
        let Some(query) = command_query(self.input.text()) else {
            return;
        };
        let matches = matching_commands(&self.commands(), query).len();
        if let Some(menu) = &mut self.command_menu {
            menu.selected = wrap_step(menu.selected, matches, delta);
        }
    }

    /// The command currently highlighted in the palette, if one is (the palette is
    /// open and the query matches at least one command) — its own copy, since
    /// a tier row is built per call ([`App::commands`]).
    #[must_use]
    pub fn highlighted_command(&self) -> Option<SlashCommand> {
        let menu = self.command_menu.as_ref()?;
        let query = command_query(self.input.text())?;
        matching_commands(&self.commands(), query)
            .get(menu.selected)
            .map(|command| (*command).clone())
    }

    /// Run the highlighted command: consume the input, close the palette, and
    /// dispatch its effect as an [`Action`] for the loop. `Action::None` if no
    /// command is highlighted (an empty query).
    pub(super) fn run_selected_command(&mut self) -> Action {
        let Some(cmd) = self.highlighted_command() else {
            return Action::None;
        };
        let effect = cmd.effect;
        self.input.clear();
        self.command_menu = None;
        match effect {
            CommandEffect::Clear => {
                self.clear_conversation();
                Action::Clear
            }
            CommandEffect::Help => {
                // Idle, /help commits its multi-line command list. Mid-turn that
                // list would interleave with the streaming reply, so it's
                // rejected with a transient toast instead. See docs/toast.md.
                if self.turn_active() {
                    Action::Toast(HELP_BUSY_NOTICE.to_string())
                } else {
                    Action::Notice(help_text(&self.commands()))
                }
            }
            CommandEffect::Copy => Action::Copy(self.last_assistant_text()),
            CommandEffect::Init => {
                // Codex's /init is submit_user_message(INIT_PROMPT): the canned
                // prompt rides the normal Submit → start_turn path — echoed as
                // the user ❯ message, recorded, checkpointed — and the model's
                // tool loop generates AGENTS.md. Trimmed because the file's
                // final newline would wrap into an empty last line that
                // message_lines pads into a stray full-width dark row under
                // the ❯ cell (codex trims the same way at render time). Mid-turn
                // it is disabled like /compact (codex's available_during_task =
                // false); the ↑ recall history is untouched (the user typed
                // "/init", not the prompt). See docs/init.md.
                if self.turn_active() {
                    Action::Toast(INIT_BUSY_NOTICE.to_string())
                } else {
                    Action::Submit(INIT_PROMPT.trim_end().to_string())
                }
            }
            CommandEffect::Compact => {
                // Codex disables /compact while a task runs (the summarize
                // request would race the stream over the same history); the
                // rejection is a transient toast like /resume's. An empty
                // derived context has nothing to summarize. See
                // docs/compact.md / docs/toast.md.
                if self.turn_active() {
                    Action::Toast(COMPACT_BUSY_NOTICE.to_string())
                } else if !crate::context::derives_conversation(&self.history) {
                    Action::Toast(COMPACT_EMPTY_NOTICE.to_string())
                } else {
                    Action::Compact
                }
            }
            CommandEffect::Resume => {
                // Codex blocks /resume while a task runs (it swaps the whole
                // conversation, racing the stream); the rejection is a transient
                // toast. Idle, the *loop* scans the sessions dir and opens the
                // picker. See docs/resume.md / docs/toast.md.
                if self.turn_active() {
                    Action::Toast(RESUME_BUSY_NOTICE.to_string())
                } else {
                    Action::OpenResumePicker
                }
            }
            CommandEffect::Model => {
                // /model works mid-turn: it only replaces the composer with the
                // inline picker, never the running turn (which streams on its own
                // thread). Selecting rebinds only the *next* turn's backend. The
                // *loop* fetches the model list. See docs/llm.md / docs/toast.md.
                Action::OpenModelPicker
            }
            CommandEffect::ServiceTier(tier) => {
                // A tier's command works mid-turn like Ctrl+T: the pure state
                // toggles at once and the *loop* rebinds only the next turn's
                // backend, persists, and toasts (docs/fast-mode.md). The row
                // exists only while the model lists the tier, so there is no
                // "unsupported" case to explain.
                self.toggle_speed_tier(&tier)
            }
            CommandEffect::Login => {
                // /login works mid-turn like /model — saving a key never touches
                // the running turn. The *loop* builds the provider choices and
                // opens the onboarding inline. See docs/llm.md / docs/toast.md.
                Action::OpenKeyOnboarding
            }
            CommandEffect::Settings => {
                // /settings works mid-turn for the same reason: it replaces
                // only the composer, and a change that rebuilds the backend
                // rebinds the *next* turn (the running one streams on its own
                // thread). See docs/settings.md.
                Action::OpenSettings
            }
            CommandEffect::Hooks => {
                // /hooks works mid-turn too — a read-only browse of the
                // hooks config touches nothing running. The *loop* digests
                // the runner's parsed hooks.json into the overview and opens
                // the menu (the /resume data-injection seam). See
                // docs/hooks-menu.md.
                Action::OpenHooksMenu
            }
            CommandEffect::Trust => {
                // /trust works mid-turn too — reviewing touches nothing
                // running, and an approval rebinds the *next* turn. The
                // *loop* digests the project layer it loaded into the review
                // and opens the menu (the same injection seam). See
                // docs/project-config.md.
                Action::OpenTrustMenu
            }
            CommandEffect::Skills => {
                // /skills works mid-turn too — browsing and toggling touch
                // nothing running; a change binds the next turn's request.
                // The *loop* takes the registry's snapshot and opens the menu
                // over it, so the rows can never disagree with what the model
                // is offered. docs/skills.md.
                Action::OpenSkillsMenu
            }
            CommandEffect::Theme => {
                // /theme works mid-turn like /mascot: it only replaces the
                // composer, and a switch purge-rebuilds the screen in the new
                // palette without touching the running turn. The catalog is
                // a const — nothing to fetch, so the pure open happens right
                // here. docs/theme.md.
                self.open_theme_picker();
                Action::OpenThemePicker
            }
            CommandEffect::Mascot => {
                // /mascot works mid-turn like /settings: it only replaces the
                // composer, and a switch repaints the banner without touching
                // the running turn. The catalog is a const — nothing to
                // fetch, so the pure open happens right here. docs/mascot.md.
                self.open_mascot_picker();
                Action::OpenMascotPicker
            }
            CommandEffect::Spinner => {
                // /spinner works mid-turn like /mascot: it only replaces the
                // composer, and the switch reaches the running turn's status
                // line on the very next frame. The catalog is a const —
                // nothing to fetch, so the pure open happens right here.
                // docs/spinner.md.
                self.open_spinner_picker();
                Action::OpenSpinnerPicker
            }
            CommandEffect::Mcp => {
                // /mcp works mid-turn too — it only replaces the composer,
                // and every connection op runs on a worker thread. The *loop*
                // snapshots the live manager and opens the menu over it
                // (docs/mcp.md).
                Action::OpenMcpMenu
            }
            CommandEffect::Donate => {
                // /donate works mid-turn like /hooks: a read-only page that
                // only replaces the composer. The catalog is a const —
                // nothing to fetch, so the pure open happens right here.
                // docs/donate.md.
                self.open_donate_picker();
                Action::OpenDonatePicker
            }
            CommandEffect::Quit => Action::Quit,
        }
    }
}

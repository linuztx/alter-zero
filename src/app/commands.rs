//! The slash-command palette: the [`COMMANDS`] registry, the `/token` filter
//! behind it, and the [`App`](super::App) side of opening, scrolling, and
//! running the highlighted command.
//!
//! Each command's behaviour is documented where the feature lives —
//! `docs/copy.md`, `docs/init.md`, `docs/compact.md`, `docs/resume.md` — and a
//! mid-turn rejection surfaces as a toast rather than a scrollback bullet
//! (`docs/toast.md`).

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Exit the app (`/quit` — codex's `/quit`/`/exit`, "exit Codex").
    Quit,
}

/// One entry in the slash-command palette: how it shows (`name`/`description`)
/// and what it does (`effect`). Adding a command is a one-line addition to
/// [`COMMANDS`]; the palette, filtering, and scrolling don't change.
#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    /// The command name **without** the leading slash (e.g. `"help"`), lowercase.
    pub name: &'static str,
    /// A one-line description shown dimmed beside the name in the palette.
    pub description: &'static str,
    /// What selecting it does.
    pub effect: CommandEffect,
}

/// The available slash commands, in the order they list in the palette. Adding a
/// command is a one-line entry here plus an effect arm in `run_selected_command`;
/// the palette, filtering, and scrolling don't change.
pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        description: "List the available commands",
        effect: CommandEffect::Help,
    },
    SlashCommand {
        name: "clear",
        description: "Clear the conversation",
        effect: CommandEffect::Clear,
    },
    SlashCommand {
        name: "copy",
        description: "Copy the last response to the clipboard",
        effect: CommandEffect::Copy,
    },
    SlashCommand {
        name: "init",
        // Codex's description, its product name swapped for ours (the /quit
        // "Exit alter-zero" pattern).
        description: "create an AGENTS.md file with instructions for alter-zero",
        effect: CommandEffect::Init,
    },
    SlashCommand {
        name: "compact",
        // Codex's description, verbatim.
        description: "summarize conversation to prevent hitting the context limit",
        effect: CommandEffect::Compact,
    },
    SlashCommand {
        name: "resume",
        description: "Resume a saved chat",
        effect: CommandEffect::Resume,
    },
    SlashCommand {
        name: "model",
        description: "Switch the active model",
        effect: CommandEffect::Model,
    },
    SlashCommand {
        name: "login",
        description: "Add or update a provider API key",
        effect: CommandEffect::Login,
    },
    SlashCommand {
        name: "settings",
        description: "Open settings menu",
        effect: CommandEffect::Settings,
    },
    SlashCommand {
        name: "hooks",
        description: "Browse the configured lifecycle hooks",
        effect: CommandEffect::Hooks,
    },
    SlashCommand {
        name: "skills",
        description: "Browse skills and enable or disable each one",
        effect: CommandEffect::Skills,
    },
    SlashCommand {
        name: "mcp",
        description: "Manage MCP servers",
        effect: CommandEffect::Mcp,
    },
    SlashCommand {
        name: "trust",
        description: "Review and approve this project's .alter-zero config",
        effect: CommandEffect::Trust,
    },
    SlashCommand {
        name: "quit",
        description: "Exit alter-zero",
        effect: CommandEffect::Quit,
    },
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

/// The commands whose name starts with `query` (case-insensitive), in registry
/// order. An empty query matches everything.
#[must_use]
pub fn matching_commands(query: &str) -> Vec<&'static SlashCommand> {
    let q = query.to_lowercase();
    COMMANDS.iter().filter(|c| c.name.starts_with(&q)).collect()
}

/// The `/help` notice: a header followed by every command's `/name — description`.
fn help_text() -> String {
    let mut text = String::from("Available commands:");
    for cmd in COMMANDS {
        text.push_str(&format!("\n/{} — {}", cmd.name, cmd.description));
    }
    text
}

impl App {
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
                let matches = matching_commands(query).len();
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

    /// Move the palette highlight by `delta`, clamped to the current matches.
    pub(super) fn move_command_selection(&mut self, delta: isize) {
        let Some(query) = command_query(self.input.text()) else {
            return;
        };
        let matches = matching_commands(query).len();
        if let Some(menu) = &mut self.command_menu {
            let last = matches.saturating_sub(1) as isize;
            menu.selected = (menu.selected as isize + delta).clamp(0, last) as usize;
        }
    }

    /// The command currently highlighted in the palette, if one is (the palette is
    /// open and the query matches at least one command).
    #[must_use]
    pub fn highlighted_command(&self) -> Option<&'static SlashCommand> {
        let menu = self.command_menu.as_ref()?;
        let query = command_query(self.input.text())?;
        matching_commands(query).get(menu.selected).copied()
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
                    Action::Notice(help_text())
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
                } else if crate::context::context_messages(&self.history).is_empty() {
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
            CommandEffect::Mcp => {
                // /mcp works mid-turn too — it only replaces the composer,
                // and every connection op runs on a worker thread. The *loop*
                // snapshots the live manager and opens the menu over it
                // (docs/mcp.md).
                Action::OpenMcpMenu
            }
            CommandEffect::Quit => Action::Quit,
        }
    }
}

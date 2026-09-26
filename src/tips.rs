//! Spinner tips (`docs/tips.md`): the dim `⎿  Tip: …` row that hangs off the
//! status line once a turn has run a few seconds — Claude Code's spinner tip.
//!
//! This is the pure half: the catalog the tips are drawn from, when a turn's
//! tip appears and moves on ([`tip_slot`]), and the per-user `tips.json` that
//! remembers the **Show tips** switch and where the walk through the catalog
//! left off ([`TipsFile`]). The walk itself runs on `App`
//! (`App::set_status_times`); the row is `ui::tips`; the file I/O is the
//! boundary's.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a turn runs before its tip appears. A quick answer never flashes
/// one — the tip is for the user who is actually waiting — and three seconds
/// is the "a few seconds" the Ctrl+B hint already waits
/// (`TOOL_BACKGROUND_HINT_DELAY`, `docs/background.md`), so the strip's two
/// delayed hints keep one pace.
pub const TIP_DELAY: Duration = Duration::from_secs(3);

/// How long one tip stays before the next replaces it, counted from when the
/// tip appeared. Most turns end inside the first slot, so what the user
/// mostly sees is the next tip at the next turn; a long agentic turn gets a
/// fresh one every few minutes — slow enough never to pull the eye off the
/// reply streaming above it (the verb already moves every 30 s).
pub const TIP_ROTATION: Duration = Duration::from_secs(3 * 60);

/// The catalog, in the order it is walked (`docs/tips.md` *The catalog*).
///
/// One sentence each, no trailing full stop, every one naming a key, a
/// command or a command-line form the app really has — keys in the `?`
/// band's spelling (`ctrl+o`, `shift+tab`, `alt+↑`). The order interleaves
/// the kinds so neighbours differ, and opens on `?`, the tip that leads to
/// every other key. The tests hold it to one row at 80 columns and to
/// naming only `/commands` the palette lists.
pub const TIPS: &[&str] = &[
    "Press ? on an empty prompt to see the keyboard shortcuts",
    "Press ctrl+o to see the whole transcript and every tool's output",
    "Send a message while the model works to steer it in real time",
    "Run /resume to reopen a past conversation",
    "Type @ to insert a file path from this directory",
    "Press shift+tab to switch between manual, edit, auto and master",
    "Run /compact to summarize the conversation and free up context",
    "Start a message with ! to run a shell command yourself",
    "Press tab while the model works to queue a follow-up turn",
    "Run alter-zero --continue to resume the last session here",
    "Press ctrl+t to change how hard a reasoning model thinks",
    "Ask for a task list on bigger jobs to track progress right here",
    "Press ctrl+v to paste an image from the clipboard",
    "Run /init to write an AGENTS.md guide for this project",
    "Press ctrl+b to move a running command to the background",
    "Type $ to mention a skill and have the model load it",
    "Double-tap esc on an empty prompt to rewind and edit a message",
    "Run /diff to review this repository's Git changes",
    "Press shift+enter or ctrl+j to start a new line",
    "Turn on Checkpoints in /settings so a rewind restores files too",
    "Press ctrl+r to search the messages you have sent before",
    "Ask the model to write a skill for a workflow you repeat",
    "Press ctrl+d to see the exact context the model receives",
    "Run /copy to copy the last reply, or /export for the whole chat",
    "Press alt+↑ to pull your last queued message back to edit it",
    "Run /model to switch models, or /login to add a provider",
    "Start a session with a prompt: alter-zero \"fix the failing test\"",
    "Run /theme to change the colour theme",
    "Press ↑ on an empty prompt to recall a message you sent before",
    "Define your own subagent types in .alter-zero/agents/<name>.md",
    "Run /spinner to restyle the spinner above this tip",
    "Run /skills to turn individual skills on or off",
    "Run /mcp to manage MCP servers, or alter-zero mcp add to add one",
    "Run alter-zero update to install the latest release",
    "Run /mascot to change the banner mascot",
    "Run /settings to change retries, temperature, tools and more",
];

/// The file's name under the config home: `{config_home}/tips.json` — its
/// own file, per **user**, like `telemetry.json` and `update.json`
/// (`docs/per-directory-state.md`).
pub const TIPS_FILE_NAME: &str = "tips.json";

/// Seeds the **Show tips** row for a run (`0`/`false`/`no`/`off` = off, the
/// grammar every `ALTER_ZERO_*` flag uses) — never written back. The smoke
/// suite turns tips off with it, so a phase that holds a turn past the delay
/// sees the strip it asserts on.
pub const TIPS_ENV: &str = "ALTER_ZERO_TIPS";

/// Which tip slot a turn `elapsed` into is in: `None` before [`TIP_DELAY`],
/// then `0` for the first [`TIP_ROTATION`] after it, `1` for the next, and
/// so on. A turn shows one tip per slot (`App::set_status_times` draws it
/// the first frame its slot is on screen).
#[must_use]
pub fn tip_slot(elapsed: Duration) -> Option<u64> {
    let shown = elapsed.checked_sub(TIP_DELAY)?;
    let slot = shown.as_millis() / TIP_ROTATION.as_millis();
    Some(u64::try_from(slot).unwrap_or(u64::MAX))
}

/// The catalog entry at `index`, **wrapping** — so a cursor written against
/// a longer catalog (an older or newer `tips.json`) still lands on a tip.
#[must_use]
pub fn tip(index: usize) -> &'static str {
    TIPS[index % TIPS.len()]
}

/// What `tips.json` holds: the **Show tips** switch and the walk's position.
///
/// Every field is written on every save — a status file the user may open —
/// and every field is optional on the way in ([`Self::parse`] is lenient), so
/// a partial, hand-edited or corrupt file reads as the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TipsFile {
    /// The user's standing choice — `true` until turned off (`/settings` →
    /// Show tips). [`TIPS_ENV`] can override it for a run without moving it.
    pub enabled: bool,
    /// The catalog index the next tip shown opens on — the walk carries on
    /// across sessions, so every tip comes round before any repeats. Taken
    /// modulo the catalog ([`tip`]).
    pub next: usize,
}

impl Default for TipsFile {
    fn default() -> Self {
        Self {
            enabled: true,
            next: 0,
        }
    }
}

impl TipsFile {
    /// Parse a `tips.json` body, best-effort: malformed, empty or
    /// wrongly-typed JSON yields the defaults — tips on, from the first — so
    /// a bad file costs at most a repeated tip.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back — every field, always.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_tip_waits_a_few_seconds_and_stays_three_minutes() {
        // The two numbers the design names (docs/tips.md): the Ctrl+B hint's
        // "a few seconds", and the rotation the user asked for.
        assert_eq!(TIP_DELAY, Duration::from_secs(3));
        assert_eq!(TIP_ROTATION, Duration::from_secs(3 * 60));
    }

    #[test]
    fn no_slot_is_open_before_the_delay() {
        assert_eq!(tip_slot(Duration::ZERO), None);
        assert_eq!(tip_slot(TIP_DELAY - MS), None);
    }

    #[test]
    fn the_first_slot_opens_at_the_delay_and_the_next_one_rotation_later() {
        assert_eq!(tip_slot(TIP_DELAY), Some(0));
        assert_eq!(tip_slot(TIP_DELAY + TIP_ROTATION - MS), Some(0));
        assert_eq!(tip_slot(TIP_DELAY + TIP_ROTATION), Some(1));
        assert_eq!(tip_slot(TIP_DELAY + TIP_ROTATION * 2 + MS), Some(2));
    }

    #[test]
    fn a_cursor_past_the_end_of_the_catalog_wraps() {
        // A `tips.json` written against a longer catalog must still land on
        // a tip, never panic.
        assert_eq!(tip(0), TIPS[0]);
        assert_eq!(tip(TIPS.len()), TIPS[0]);
        assert_eq!(tip(TIPS.len() * 3 + 1), TIPS[1]);
    }

    #[test]
    fn the_walk_opens_on_the_tip_that_leads_to_the_others() {
        // `?` lists every key — the one tip worth showing a first-run user
        // before any other (docs/tips.md *The catalog*).
        assert!(TIPS[0].contains("?"), "{}", TIPS[0]);
    }

    #[test]
    fn the_catalog_has_no_blank_duplicate_or_full_stopped_tips() {
        assert!(TIPS.len() >= 20, "a catalog worth walking");
        for (i, tip) in TIPS.iter().enumerate() {
            assert_eq!(tip.trim(), *tip, "#{i} carries stray whitespace");
            assert!(!tip.is_empty(), "#{i} is empty");
            assert!(!tip.ends_with('.'), "#{i} ends in a full stop: {tip}");
            assert!(
                !TIPS[..i].contains(tip),
                "#{i} repeats an earlier tip: {tip}"
            );
        }
    }

    #[test]
    fn every_command_a_tip_names_is_in_the_palette() {
        // A renamed or retired command must not leave a tip pointing at
        // nothing: every whitespace-delimited `/word` a tip names is a
        // `COMMANDS` row (a path like `.alter-zero/agents` never starts a
        // token with `/`, so it is not mistaken for one).
        for tip in TIPS {
            for token in tip.split_whitespace() {
                let Some(name) = token.strip_prefix('/') else {
                    continue;
                };
                let name = name.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                assert!(
                    crate::app::COMMANDS.iter().any(|c| c.name == name),
                    "`/{name}` in {tip:?} is not a command"
                );
            }
        }
    }

    #[test]
    fn the_file_and_the_switch_have_their_documented_names() {
        assert_eq!(TIPS_FILE_NAME, "tips.json");
        assert_eq!(TIPS_ENV, "ALTER_ZERO_TIPS");
    }

    #[test]
    fn a_missing_or_corrupt_tips_file_reads_as_on_from_the_first_tip() {
        for text in ["", "not json", "[]", "{\"next\": \"seven\"}"] {
            let file = TipsFile::parse(text);
            assert!(file.enabled, "{text:?}");
            assert_eq!(file.next, 0, "{text:?}");
        }
    }

    #[test]
    fn a_partial_tips_file_keeps_the_defaults_it_does_not_name() {
        assert_eq!(
            TipsFile::parse(r#"{"next": 5}"#),
            TipsFile {
                enabled: true,
                next: 5
            }
        );
        assert_eq!(
            TipsFile::parse(r#"{"enabled": false}"#),
            TipsFile {
                enabled: false,
                next: 0
            }
        );
    }

    #[test]
    fn the_tips_file_writes_every_field_and_reads_back_the_same() {
        let file = TipsFile {
            enabled: false,
            next: 7,
        };
        let json = file.to_json();
        assert!(json.contains("\"enabled\": false"), "{json}");
        assert!(json.contains("\"next\": 7"), "{json}");
        assert_eq!(TipsFile::parse(&json), file);
    }
}

//! What one event's hooks did, as the conversation shows it — the record behind
//! the `● Ran 1 Stop hook` cell (`docs/hooks.md` *The hook-run cell*).
//!
//! Pure data. The runner fills it in from each handler's run and parsed answer
//! (`crate::llm::hooks::hook_report`), the rollout keeps it, and `ui` draws it
//! collapsed inline and expanded in Ctrl+O. Every row's words are composed
//! when the report is built and kept as they were shown, so a `/resume`
//! replays the record rather than today's wording — the refusal cell's rule.

/// One event's hooks, as their cell shows them. Built only when the hooks
/// said something: a silent run leaves no report and no cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReport {
    /// The event whose hooks ran — `Stop`, `SessionStart`, …
    pub event: String,
    /// Every handler that ran, in the order it ran — the silent ones too, so
    /// the header counts what ran and Ctrl+O lists it.
    pub hooks: Vec<RanHook>,
    /// What was wrong with the configuration on the way — a matcher that is
    /// no regex, a handler skipped — verbatim, rows of their own rather than
    /// any one hook's.
    pub warnings: Vec<String>,
}

/// One handler that ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RanHook {
    /// Its command as the transcript names it: the first line, cut — the
    /// label a refusal's `Hook:` line gives it.
    pub command: String,
    /// The `hooks.json` it is configured in, as the screen names it
    /// (`~`-relative under the user's home); `None` for a config read from
    /// no file.
    pub config: Option<String>,
    /// How long it ran, in milliseconds.
    pub millis: u64,
    /// What it said that the cell shows, in the order it said it.
    pub said: Vec<HookSaid>,
}

/// One thing a hook said that its cell shows, with its row's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSaid {
    /// `continue: false` from a `Stop`/`SubagentStop` hook — `Stopped by Stop
    /// hook: {reason}`.
    Stopped(String),
    /// The block the agent keeps going on — `Stop hook feedback: {reason}`,
    /// the first line of what the model reads.
    Feedback(String),
    /// Context the model reads: the row (`Context added by SessionStart
    /// hook`) and the text the hook added, which only Ctrl+O shows.
    Context { row: String, text: String },
    /// A `systemMessage` — a line the hook addressed to the user, verbatim.
    Message(String),
    /// The hook broke — `Stop hook error: {what broke}`.
    Error(String),
}

impl HookSaid {
    /// The row's words.
    #[must_use]
    pub fn row(&self) -> &str {
        match self {
            Self::Stopped(row)
            | Self::Feedback(row)
            | Self::Message(row)
            | Self::Error(row)
            | Self::Context { row, .. } => row,
        }
    }

    /// Whether this row reports a hook that broke — drawn red, and turning
    /// the cell's bullet red.
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// How consequential the row is — the collapsed cell lists the most
    /// consequential first: what changed the turn's course, then what the
    /// model was given, then what the user was told, then what broke.
    const fn rank(&self) -> u8 {
        match self {
            Self::Stopped(_) => 0,
            Self::Feedback(_) => 1,
            Self::Context { .. } => 2,
            Self::Message(_) => 3,
            Self::Error(_) => 4,
        }
    }
}

impl HookReport {
    /// The label a hook note carrying only this report goes under — `Stop
    /// hook`, the label every hook note uses.
    #[must_use]
    pub fn note_label(&self) -> String {
        format!("{} hook", self.event)
    }

    /// Did any hook break, or the configuration have a problem? The bullet
    /// is red then, green otherwise — a block is a hook working.
    #[must_use]
    pub fn failed(&self) -> bool {
        !self.warnings.is_empty()
            || self
                .hooks
                .iter()
                .any(|hook| hook.said.iter().any(HookSaid::is_error))
    }

    /// The collapsed cell's rows from the hooks, most consequential first —
    /// a stop, then the feedback the agent keeps going on, then context,
    /// messages and errors — each kind in the order its hooks ran. The
    /// configuration's [`warnings`](Self::warnings) follow them.
    #[must_use]
    pub fn collapsed(&self) -> Vec<&HookSaid> {
        let mut rows: Vec<&HookSaid> = self.hooks.iter().flat_map(|hook| &hook.said).collect();
        // Stable, so hooks of one kind keep the order they ran in.
        rows.sort_by_key(|said| said.rank());
        rows
    }

    /// Fold `other` — another dispatch of the same event, such as a
    /// `SessionStart` source still queued after the one fired as the session
    /// opened — into this report, after what it holds.
    pub fn absorb(&mut self, other: Self) {
        self.hooks.extend(other.hooks);
        self.warnings.extend(other.warnings);
    }

    /// [`absorb`](Self::absorb) over options: whichever reports there are,
    /// as one.
    #[must_use]
    pub fn joined(first: Option<Self>, second: Option<Self>) -> Option<Self> {
        match (first, second) {
            (Some(mut first), Some(second)) => {
                first.absorb(second);
                Some(first)
            }
            (first, second) => first.or(second),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(command: &str, said: Vec<HookSaid>) -> RanHook {
        RanHook {
            command: command.to_string(),
            config: Some("~/.alter-zero/hooks.json".to_string()),
            millis: 12,
            said,
        }
    }

    fn report(hooks: Vec<RanHook>) -> HookReport {
        HookReport {
            event: "Stop".to_string(),
            hooks,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_hook_note_carrying_a_report_is_labelled_by_its_event() {
        assert_eq!(report(Vec::new()).note_label(), "Stop hook");
    }

    #[test]
    fn every_row_reads_as_its_words() {
        let said = [
            HookSaid::Stopped("Stopped by Stop hook: done".into()),
            HookSaid::Feedback("Stop hook feedback: tests are red".into()),
            HookSaid::Context {
                row: "Context added by SessionStart hook".into(),
                text: "on branch main".into(),
            },
            HookSaid::Message("Formatted 3 files".into()),
            HookSaid::Error("Stop hook error: failed (exit 1): boom".into()),
        ];
        let rows: Vec<&str> = said.iter().map(HookSaid::row).collect();
        assert_eq!(
            rows,
            vec![
                "Stopped by Stop hook: done",
                "Stop hook feedback: tests are red",
                "Context added by SessionStart hook",
                "Formatted 3 files",
                "Stop hook error: failed (exit 1): boom",
            ]
        );
    }

    #[test]
    fn a_block_is_a_hook_working_only_an_error_or_a_config_problem_fails() {
        let blocked = report(vec![ran(
            "./check.sh",
            vec![HookSaid::Feedback(
                "Stop hook feedback: commit first".into(),
            )],
        )]);
        assert!(!blocked.failed(), "a block is the hook doing its job");

        let broke = report(vec![
            ran("./notify.sh", Vec::new()),
            ran(
                "./check.sh",
                vec![HookSaid::Error(
                    "Stop hook error: did not run: timed out after 60s".into(),
                )],
            ),
        ]);
        assert!(broke.failed(), "any hook that broke turns the cell red");

        let misconfigured = HookReport {
            warnings: vec!["hooks.json: Stop matcher \"[\" is not a valid regex — skipped".into()],
            ..report(Vec::new())
        };
        assert!(misconfigured.failed(), "a config problem is a failure too");
    }

    #[test]
    fn the_collapsed_rows_lead_with_what_changed_the_turn() {
        // The first hook only broke; the second blocked and spoke. Folded to
        // three rows, the cell must still show the feedback the agent acts
        // on, so the rows sort by consequence, not by which hook ran first.
        let cell = report(vec![
            ran(
                "./notify.sh",
                vec![HookSaid::Error(
                    "Stop hook error: failed (exit 1): offline".into(),
                )],
            ),
            ran(
                "./check.sh",
                vec![
                    HookSaid::Message("Checked 4 files".into()),
                    HookSaid::Feedback("Stop hook feedback: commit first".into()),
                ],
            ),
            ran(
                "./other.sh",
                vec![HookSaid::Error(
                    "Stop hook error: did not run: timed out after 60s".into(),
                )],
            ),
        ]);
        let rows: Vec<&str> = cell.collapsed().into_iter().map(HookSaid::row).collect();
        assert_eq!(
            rows,
            vec![
                "Stop hook feedback: commit first",
                "Checked 4 files",
                "Stop hook error: failed (exit 1): offline",
                "Stop hook error: did not run: timed out after 60s",
            ],
            "most consequential first, each kind in the order its hooks ran"
        );
    }

    #[test]
    fn a_second_dispatch_of_the_event_joins_the_first_report() {
        let fired = report(vec![ran("./startup.sh", Vec::new())]);
        let queued = HookReport {
            warnings: vec!["hooks.json: a warning".into()],
            ..report(vec![ran("./clear.sh", Vec::new())])
        };
        let joined = HookReport::joined(Some(fired.clone()), Some(queued.clone()))
            .expect("two reports make one");
        let commands: Vec<&str> = joined.hooks.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, vec!["./startup.sh", "./clear.sh"]);
        assert_eq!(joined.warnings, vec!["hooks.json: a warning".to_string()]);
        assert_eq!(HookReport::joined(None, Some(queued.clone())), Some(queued));
        assert_eq!(HookReport::joined(Some(fired.clone()), None), Some(fired));
        assert_eq!(HookReport::joined(None, None), None);
    }
}

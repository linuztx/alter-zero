//! The scenario registry: which demo the dummy plays for a given prompt.
//!
//! The dummy is the only backend `scripts/smoke.sh` can drive
//! deterministically, so every UI feature that would otherwise need a live
//! provider has a scripted turn here — a parallel tool batch, a subagent
//! group, a streaming markdown table, four permission round trips. Those used
//! to be selected by two hand-written `if`/`else` chains in two different
//! files: one inside `turn_events`, one inside `DummyAi::spawn`. Adding a demo
//! meant editing both, and nothing said which cues were already taken or
//! whether an entry was still reachable at all.
//!
//! They are one ordered table now, the shape `app::COMMANDS` uses for slash
//! commands: **adding a demo is one [`SCENARIOS`] entry plus its turn
//! function.** Registry order is match order, so a narrower cue must come
//! first ("staggered permission demo" also mentions "permission"), and
//! [`SCENARIOS`]'s last entry matches everything — [`select`] is total.
//!
//! The suite pairs every entry with a prompt that must select *it*, so an
//! entry shadowed by an earlier cue fails the tests instead of silently never
//! playing again — and a new entry with no example fails them too. See
//! `docs/dummy-backend.md`.

use tokio::sync::mpsc::UnboundedSender;

use crate::ask::AskGate;
use crate::permission::PermissionGate;

use super::super::{CancelToken, StreamEvent};
use super::{gated, turns};

/// The turn's inputs as a scenario reads them: the prompt, its lowercased
/// form (cue matching is case-insensitive) and how many Ctrl+V images rode
/// along. Built once per turn so a registry walk doesn't re-lowercase the
/// prompt for every entry it tries.
pub(in crate::stream) struct Cue {
    text: String,
    lower: String,
    images: usize,
}

impl Cue {
    /// The cue for one turn's `prompt` with `images` attachments.
    pub(in crate::stream) fn new(prompt: &str, images: usize) -> Self {
        Self {
            text: prompt.to_string(),
            lower: prompt.to_lowercase(),
            images,
        }
    }

    /// The prompt as it was typed — what a reply is composed from.
    pub(in crate::stream) fn text(&self) -> &str {
        &self.text
    }

    /// How many Ctrl+V images the turn carried (`docs/image-paste.md`).
    pub(in crate::stream) fn images(&self) -> usize {
        self.images
    }

    /// Does the prompt mention `word`? Case-insensitive — the user types
    /// these cues, so "Table" and "table" must select the same demo. `word`
    /// must be lowercase.
    pub(in crate::stream) fn mentions(&self, word: &str) -> bool {
        self.lower.contains(word)
    }

    /// Does the prompt *begin* with `marker`, exactly? Used for generated
    /// requests (`/compact`'s summarization prompt), where a fuzzy match
    /// would hijack a real question about the same subject.
    pub(in crate::stream) fn starts_with(&self, marker: &str) -> bool {
        self.text.starts_with(marker)
    }
}

/// What a **gated** scenario streams through: the shared permission gate it
/// blocks on, the event channel, and the turn's cancel flag. Bundled because
/// every gated demo needs all three and passes them to the same helpers.
pub(in crate::stream) struct Stage<'a> {
    pub(in crate::stream) gate: &'a PermissionGate,
    pub(in crate::stream) tx: &'a UnboundedSender<StreamEvent>,
    pub(in crate::stream) cancel: &'a CancelToken,
}

/// What an **asked** scenario streams through — the [`Stage`] twin for the
/// `AskUserQuestion` demo, blocking on the [`AskGate`] instead
/// (`docs/ask.md`).
pub(in crate::stream) struct AskStage<'a> {
    pub(in crate::stream) gate: &'a AskGate,
    pub(in crate::stream) tx: &'a UnboundedSender<StreamEvent>,
    pub(in crate::stream) cancel: &'a CancelToken,
}

/// How a selected scenario produces its events.
#[derive(Clone, Copy)]
pub(in crate::stream) enum Play {
    /// A **pure** turn: the whole event list up front, played back by
    /// [`DummyAi`](super::DummyAi) with the delays that make streaming
    /// visible. Unit-testable event by event.
    Script(fn(&Cue) -> Vec<StreamEvent>),
    /// A turn that needs the *user*: it streams on the channel itself and
    /// blocks on the permission gate between calls, exactly as a real
    /// backend's tool thread does (`docs/permissions.md`). Only ever selected
    /// when a gate is attached.
    Gated(fn(&Stage<'_>)),
    /// A turn that **asks the user questions**: it raises the inline modal
    /// and blocks on the ask gate exactly as the real tool does
    /// (`docs/ask.md`). Only ever selected when an ask gate is attached.
    Asked(fn(&AskStage<'_>)),
}

/// One offline demo the dummy can play.
pub(in crate::stream) struct Scenario {
    /// Test-only: the entry's stable id, so the suite can say *which*
    /// scenario a prompt selected. Nothing at runtime reads it — selection
    /// needs only the cue and the turn — and the docs name demos by their
    /// cue, so carrying it in the shipped binary would be dead weight.
    #[cfg(test)]
    pub(in crate::stream) name: &'static str,
    /// Does this prompt select the scenario? Tried in registry order.
    pub(in crate::stream) selects: fn(&Cue) -> bool,
    /// What it plays.
    pub(in crate::stream) play: Play,
}

/// Every demo, in match order: the gated approval round trips first (their
/// cues are the narrowest), then the scripted turns, then the default turn —
/// which matches anything, so selection never falls off the end.
pub(in crate::stream) const SCENARIOS: &[Scenario] = &[
    // The `AskUserQuestion` round trip: three questions through the modal
    // (docs/ask.md).
    Scenario {
        #[cfg(test)]
        name: "ask-questions",
        selects: |cue| cue.mentions("ask") && cue.mentions("question"),
        play: Play::Asked(gated::ask_questions_turn),
    },
    // Auto mode's classifier deciding a `bash` batch instead of the user.
    Scenario {
        #[cfg(test)]
        name: "permission-auto",
        selects: |cue| cue.mentions("permission") && cue.mentions("auto"),
        play: Play::Gated(gated::auto_permission_turn),
    },
    // Two gated `bash` calls: back-to-back prompts with no pause between.
    Scenario {
        #[cfg(test)]
        name: "permission-parallel",
        selects: |cue| cue.mentions("permission") && cue.mentions("parallel"),
        play: Play::Gated(gated::parallel_permission_turn),
    },
    // A screen-tall `write` prompt answered into a one-line one — the pinned
    // modal region's hardest shrink.
    Scenario {
        #[cfg(test)]
        name: "permission-staggered",
        selects: |cue| cue.mentions("permission") && cue.mentions("staggered"),
        play: Play::Gated(gated::staggered_permission_turn),
    },
    // The single `Write` approval: the options, Tab's amend, the draft stash.
    Scenario {
        #[cfg(test)]
        name: "permission-write",
        selects: |cue| cue.mentions("permission"),
        play: Play::Gated(gated::write_permission_turn),
    },
    // `/compact`'s summarization request: a text-only handoff summary.
    Scenario {
        #[cfg(test)]
        name: "compact",
        selects: |cue| cue.starts_with(turns::COMPACT_PROMPT_MARKER),
        play: Play::Script(turns::compact_turn),
    },
    // A UserPromptSubmit hook refusing the submission (docs/hooks.md) — the
    // more specific cue, so it outranks the general hooks demo below.
    Scenario {
        #[cfg(test)]
        name: "prompt-block",
        selects: |cue| cue.mentions("hook") && cue.mentions("block my prompt"),
        play: Play::Script(turns::prompt_block_turn),
    },
    // Lifecycle hooks (docs/hooks.md): a PreToolUse block, then a PostToolUse
    // note on a call that ran.
    Scenario {
        #[cfg(test)]
        name: "hooks",
        selects: |cue| cue.mentions("hook"),
        play: Play::Script(turns::hooks_turn),
    },
    // A streaming GFM table with wide emoji: the strip-collapse geometry.
    Scenario {
        #[cfg(test)]
        name: "table",
        selects: |cue| cue.mentions("table"),
        play: Play::Script(turns::table_turn),
    },
    // A two-subagent group, foreground or background (docs/agent-tool.md).
    Scenario {
        #[cfg(test)]
        name: "agents",
        // Never for `/init`, whose canned prompt names AGENTS.md.
        selects: |cue| cue.mentions("agents") && !cue.mentions("agents.md"),
        play: Play::Script(turns::agents_turn),
    },
    // Three parallel `Bash(ping …)` calls: the vivid `⎿ Waiting…` batch.
    Scenario {
        #[cfg(test)]
        name: "parallel-batch",
        selects: |cue| cue.mentions("parallel"),
        play: Play::Script(turns::parallel_turn),
    },
    // A `Write` then an `Edit`: the numbered file cell and its tinted diff.
    Scenario {
        #[cfg(test)]
        name: "files",
        // Never for `/init`, whose canned prompt says "do not overwrite" —
        // the same AGENTS.md guard the agent demo above needs.
        selects: |cue| {
            (cue.mentions("diff") || cue.mentions("edit") || cue.mentions("write"))
                && !cue.mentions("agents.md")
        },
        play: Play::Script(turns::files_turn),
    },
    // The todo demo taken all the way to done — the end state: the checklist
    // bows out with the turn and is swept for good (docs/task-tools.md).
    // Ahead of the broader todo cue, which it also matches.
    Scenario {
        #[cfg(test)]
        name: "tasks-finished",
        selects: |cue| (cue.mentions("todo") || cue.mentions("task")) && cue.mentions("finish"),
        play: Play::Script(turns::tasks_finished_turn),
    },
    // The task-tools lifecycle: the live checklist, dependencies, the
    // spinner override (docs/task-tools.md).
    Scenario {
        #[cfg(test)]
        name: "tasks",
        selects: |cue| cue.mentions("todo") || cue.mentions("task"),
        play: Play::Script(turns::tasks_turn),
    },
    // Loading an authored `SKILL.md` into the conversation — the one-line
    // cell over the whole body the model reads (docs/skills.md). The
    // `$dataviz` cue is the composer's `$` picker inserting a mention of the
    // demo skill (docs/skill-mentions.md): the word "skill" never appears in
    // such a prompt, and the demo answering it is how the offline dummy
    // plays the mention the way a live model would.
    Scenario {
        #[cfg(test)]
        name: "skills",
        selects: |cue| cue.mentions("skill") || cue.mentions("$dataviz"),
        play: Play::Script(turns::skills_turn),
    },
    // The default turn: think, then a compact `Read`+`Bash` batch.
    Scenario {
        #[cfg(test)]
        name: "tools",
        // The catch-all — keep it last, and keep it matching everything.
        selects: |_| true,
        play: Play::Script(turns::tools_turn),
    },
];

/// The scenario `cue` selects. `gate_attached` / `ask_attached` report which
/// gates the session handed the dummy: without the matching one a
/// gated/asked demo would block forever on an answer nobody can give, so it
/// is skipped and the prompt falls through to a scripted turn — which is why
/// `turn_events` can answer *any* prompt.
///
/// Total by construction: [`SCENARIOS`]'s last entry matches everything.
pub(in crate::stream) fn select(
    cue: &Cue,
    gate_attached: bool,
    ask_attached: bool,
) -> &'static Scenario {
    SCENARIOS
        .iter()
        .find(|scenario| {
            let playable = match scenario.play {
                Play::Script(_) => true,
                Play::Gated(_) => gate_attached,
                Play::Asked(_) => ask_attached,
            };
            playable && (scenario.selects)(cue)
        })
        .unwrap_or(&SCENARIOS[SCENARIOS.len() - 1])
}

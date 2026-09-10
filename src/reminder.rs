//! The `<system-reminder>` the derived context leads with (`docs/context.md`).
//!
//! One block, in front of every turn's conversation, carrying what the session
//! tells the model *about itself* that neither the system prompt nor the tool
//! schemas name: the project's `AGENTS.md` instructions
//! (`docs/project-doc.md`), the skills the `Skill` tool can load
//! (`docs/skills.md`) and the types the `Agent` tool can launch
//! (`docs/subagents.md`). Each contributor renders its own **section**
//! ([`crate::project_doc::instructions_section`],
//! [`crate::skills::skill_section`], [`crate::subagents::agent_section`]);
//! this module only knows how sections become the one reminder.
//!
//! One block rather than one fragment per contributor because they are one
//! kind of thing, and because every one of them is re-rendered per turn: a
//! second fragment would be a second place the prompt-cache prefix can shift
//! (`docs/prompt-caching.md`). The shape is the reference tool's — an opening
//! line saying what follows, the sections blank-line separated, the closing
//! tag — so a model trained on it reads ours the same way.

/// The opening tag.
pub const REMINDER_OPEN: &str = "<system-reminder>";

/// The closing tag.
pub const REMINDER_CLOSE: &str = "</system-reminder>";

/// The reminder's first line: what the block is, before any section says
/// what *it* is.
pub const REMINDER_PREAMBLE: &str = "Use the following contexts and instructions:";

/// The non-blank `sections`, each trimmed, joined by one blank line — the
/// body of a reminder, and the join a contributor with several parts
/// ([`crate::subagents::listing_sections`]) uses for its own. Empty when
/// nothing has anything to say.
#[must_use]
pub fn join_sections(sections: &[&str]) -> String {
    sections
        .iter()
        .map(|section| section.trim())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The reminder over `sections`: the preamble, then the sections
/// ([`join_sections`]), between the tags. Empty when no section has anything
/// to say — a session with nothing to remind the model of sends no block at
/// all, never a bare preamble.
#[must_use]
pub fn reminder_message(sections: &[&str]) -> String {
    let body = join_sections(sections);
    if body.is_empty() {
        return String::new();
    }
    format!("{REMINDER_OPEN}\n{REMINDER_PREAMBLE}\n\n{body}\n{REMINDER_CLOSE}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_sections_skips_blanks_and_separates_with_one_blank_line() {
        assert_eq!(join_sections(&["a", "", "  \n", "b"]), "a\n\nb");
        assert_eq!(join_sections(&[]), "");
        assert_eq!(join_sections(&["", "\n"]), "");
        // A section's own surrounding whitespace is trimmed, so a trailing
        // newline never doubles the separator.
        assert_eq!(join_sections(&["a\n", "\nb"]), "a\n\nb");
    }

    #[test]
    fn the_reminder_wraps_the_sections_under_the_preamble() {
        assert_eq!(
            reminder_message(&[
                "Contents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nUse TDD.",
                "The following skills are available for use with the Skill tool:\n\n- dataviz: Charts.",
            ]),
            "<system-reminder>\n\
             Use the following contexts and instructions:\n\n\
             Contents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\n\
             Use TDD.\n\n\
             The following skills are available for use with the Skill tool:\n\n\
             - dataviz: Charts.\n\
             </system-reminder>"
        );
    }

    #[test]
    fn a_reminder_with_nothing_to_say_is_empty() {
        // Not a bare preamble over nothing: a session with no AGENTS.md, no
        // skills and no agent types sends no reminder at all.
        assert_eq!(reminder_message(&[]), "");
        assert_eq!(reminder_message(&["", "  \n"]), "");
    }

    #[test]
    fn the_tags_and_the_preamble_are_the_references_wording() {
        assert_eq!(REMINDER_OPEN, "<system-reminder>");
        assert_eq!(REMINDER_CLOSE, "</system-reminder>");
        assert_eq!(
            REMINDER_PREAMBLE,
            "Use the following contexts and instructions:"
        );
    }
}

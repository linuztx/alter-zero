//! Subagent **definitions** — the `agents/*.md` files behind the `agent`
//! tool's `subagent_type` (`docs/subagents.md`).
//!
//! [`crate::agents`] is the *roster* (a launched agent's live state);
//! this is where a type comes from: YAML frontmatter naming it, describing
//! it, optionally pinning a model and an allowlist of tools, over an optional
//! markdown body that replaces the persona for that type.
//!
//! Everything here is pure — the parse ([`parse_agent`]), the tool allowlist
//! ([`AgentTools::allows`]), the budgeted listing the model chooses from
//! ([`agent_listing`], [`reminder_message`]), the system-prompt composition
//! ([`system_prompt_for`]) and the shared [`SubagentRegistry`] handle. The
//! filesystem walk and the seeding of the built-in defaults live in
//! [`crate::llm::subagent`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::frontmatter::{self, FileError};

/// The directory an agent definition lives in, under a root
/// (`~/.alter-zero/agents`, `{project}/.alter-zero/agents`).
pub const AGENTS_DIR_NAME: &str = "agents";

/// The extension an agent definition file carries.
pub const AGENT_FILE_EXT: &str = "md";

/// The longest name an agent type may carry — [`crate::skills`]' rule, for
/// the same reason: it has to be printable, matchable and typeable.
pub const MAX_AGENT_NAME_LEN: usize = 64;

/// The `model:` value meaning "run this type on the session's own model".
/// The seeded files write it rather than omitting the key: a key that is
/// *there* is a key you can edit without reading the docs first.
pub const INHERIT_MODEL: &str = "inherit";

/// The `tools:` value (and the listing's rendering) meaning every tool.
pub const ALL_TOOLS: &str = "*";

/// The listing's header inside the `<system-reminder>` — the skills header's
/// sibling ([`crate::skills::SKILL_LISTING_HEADER`]).
pub const AGENT_LISTING_HEADER: &str = "Available agent types for the Agent tool:";

/// A definition file that could not be read or parsed. Collected rather than
/// thrown: one bad agent file must not cost a session the rest.
pub type AgentFileError = FileError;

/// The errors in `current` whose file `reported` has not already raised — the
/// per-turn rescan's toast filter ([`crate::skills::unreported_errors`]'s
/// twin, and the same rule: loud once, then quiet until it changes).
#[must_use]
pub fn unreported_errors(
    reported: &std::collections::BTreeSet<PathBuf>,
    current: &[AgentFileError],
) -> Vec<AgentFileError> {
    frontmatter::unreported(reported, current)
}

/// Which model a subagent type runs on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentModel {
    /// The session's own model — `model: inherit`, or no `model:` at all.
    #[default]
    Inherit,
    /// A provider model id to run this type on instead.
    Named(String),
}

impl AgentModel {
    /// Read a `model:` value: `inherit` (or a blank) is [`Self::Inherit`].
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case(INHERIT_MODEL) {
            Self::Inherit
        } else {
            Self::Named(value.to_string())
        }
    }

    /// The model id to switch to, or `None` to inherit.
    #[must_use]
    pub fn named(&self) -> Option<&str> {
        match self {
            Self::Inherit => None,
            Self::Named(model) => Some(model),
        }
    }
}

/// Which tools a subagent type is offered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentTools {
    /// No `tools:` key (or a bare `*`) — every tool the session has.
    #[default]
    All,
    /// The allowlist as the file wrote it: built-in display names
    /// (`Bash`, `Read`, …), MCP wire names, and `*`-suffixed globs.
    Only(Vec<String>),
}

impl AgentTools {
    /// Read a `tools:` value. Tolerant of the three spellings a file may use
    /// — `Bash, Read`, `[Bash, Read]`, and a YAML block sequence (which the
    /// frontmatter scanner folds to `- Bash - Read`) — because the difference
    /// is the author's habit, not an intent.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let entries: Vec<String> = value
            .split([',', '\n'])
            .flat_map(|part| part.split_whitespace())
            .map(|entry| {
                // Brackets and quotes at either end are punctuation; a `-` is
                // only ever a YAML sequence dash, which is at the FRONT — a
                // trailing one belongs to the name (an MCP tool may well be
                // spelled with hyphens).
                entry
                    .trim_matches(['[', ']', '"', '\''])
                    .trim_start_matches('-')
                    .trim()
            })
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
        if entries.is_empty() || entries.iter().any(|entry| entry == ALL_TOOLS) {
            return Self::All;
        }
        Self::Only(entries)
    }

    /// Is the tool whose **wire** name is `name` (`bash`, `read`,
    /// `mcp__deepwiki__ask_question`) in this allowlist?
    ///
    /// Case-insensitive — the file writes `Bash` and the wire says `bash` —
    /// and a trailing `*` globs, so one `mcp__deepwiki__*` entry covers a
    /// whole server's tools.
    #[must_use]
    pub fn allows(&self, name: &str) -> bool {
        let Self::Only(entries) = self else {
            return true;
        };
        let name = name.trim().to_ascii_lowercase();
        entries.iter().any(|entry| {
            let entry = entry.trim().to_ascii_lowercase();
            match entry.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => entry == name,
            }
        })
    }

    /// How the listing renders this set: `*`, or the allowlist as written.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::All => ALL_TOOLS.to_string(),
            Self::Only(entries) => entries.join(", "),
        }
    }
}

/// One subagent type: what the model chooses from, and what its run is built
/// out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// The `subagent_type` the model passes — the frontmatter's `name`, else
    /// the file's stem.
    pub name: String,
    /// The one-line pitch in the listing (already capped).
    pub description: String,
    /// The model this type runs on ([`AgentModel::Inherit`] by default).
    pub model: AgentModel,
    /// The tools this type is offered ([`AgentTools::All`] by default).
    pub tools: AgentTools,
    /// The body: this type's own system prompt, replacing the persona.
    /// `None` when the file is frontmatter only.
    pub system_prompt: Option<String>,
    /// The file it was read from — what an error names, and what a later
    /// browser would open. Empty for a built-in fallback that never reached
    /// disk.
    pub path: PathBuf,
}

/// Why an agent file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentParseError {
    /// No `---`-delimited frontmatter block opened *and* closed the file.
    MissingFrontmatter,
    /// No `description` — the only thing the model sees before launching,
    /// so a type without one could never be chosen on purpose.
    MissingDescription,
    /// A name that could not be printed, matched, or passed as a
    /// `subagent_type`.
    InvalidName(String),
}

impl std::fmt::Display for AgentParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "missing YAML frontmatter delimited by ---"),
            Self::MissingDescription => write!(f, "missing field `description`"),
            Self::InvalidName(reason) => write!(f, "invalid name: {reason}"),
        }
    }
}

/// Is `name` usable as an agent type — safe to print, to match a
/// `subagent_type` argument against, and to name in the listing? Letters,
/// digits and underscores in `-`-joined segments, at most
/// [`MAX_AGENT_NAME_LEN`] long.
///
/// Slightly looser than [`crate::skills::validate_skill_name`]: uppercase is
/// allowed, because an ecosystem agent file may well be `Explore.md` and the
/// lookup folds case anyway.
///
/// # Errors
/// Returns the reason the name was refused.
pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty".to_string());
    }
    if name.chars().count() > MAX_AGENT_NAME_LEN {
        return Err(format!(
            "exceeds maximum length of {MAX_AGENT_NAME_LEN} characters"
        ));
    }
    if name.split('-').any(|segment| {
        segment.is_empty()
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    }) {
        return Err("expected letters, digits and underscores in `-`-joined segments".to_string());
    }
    Ok(())
}

/// Parse one agent definition file. `path` names it — its stem is the
/// default `name`, and an error reports it.
///
/// Unknown frontmatter keys are **ignored, not rejected** (the `SKILL.md`
/// rule): a file authored for another tool carries keys we do not model, and
/// refusing to load over one would lock the ecosystem out.
///
/// # Errors
/// [`AgentParseError`] when the frontmatter is absent, the description is
/// missing, or the resolved name is unusable.
pub fn parse_agent(
    contents: &str,
    path: &std::path::Path,
) -> Result<AgentDefinition, AgentParseError> {
    let (block, body) = frontmatter::split(contents).ok_or(AgentParseError::MissingFrontmatter)?;
    let fields = frontmatter::scalars(&block);

    let name = frontmatter::field(&fields, "name")
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string()
        });
    validate_agent_name(&name).map_err(AgentParseError::InvalidName)?;

    let description = frontmatter::field(&fields, "description")
        .map(str::to_string)
        .ok_or(AgentParseError::MissingDescription)?;

    let body = body.trim_matches(['\n', '\r']).trim_end().to_string();
    Ok(AgentDefinition {
        name,
        description: frontmatter::truncate_chars(
            &description,
            crate::skills::MAX_LISTING_DESC_CHARS,
        ),
        model: frontmatter::field(&fields, "model").map_or(AgentModel::Inherit, AgentModel::parse),
        tools: frontmatter::field(&fields, "tools").map_or(AgentTools::All, AgentTools::parse),
        system_prompt: (!body.is_empty()).then_some(body),
        path: path.to_path_buf(),
    })
}

/// The `- name: description (Tools: …)` rows the model chooses a type from,
/// within `budget` characters.
///
/// Over budget the descriptions are trimmed to an even share, then dropped
/// for names alone — but a *type* is never dropped, exactly as a skill isn't:
/// one the model cannot see is one it cannot launch. The `(Tools: …)` suffix
/// survives the trim: which tools a type has is the other half of choosing
/// it, and it is short.
#[must_use]
pub fn agent_listing(agents: &[AgentDefinition], budget: usize) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let suffixes: Vec<String> = agents
        .iter()
        .map(|agent| format!(" (Tools: {})", agent.tools.summary()))
        .collect();
    let row = |agent: &AgentDefinition, description: &str, suffix: &str| {
        if description.is_empty() {
            format!("- {}{suffix}", agent.name)
        } else {
            format!("- {}: {description}{suffix}", agent.name)
        }
    };
    let full: Vec<String> = agents
        .iter()
        .zip(&suffixes)
        .map(|(agent, suffix)| row(agent, &agent.description, suffix))
        .collect();
    let total: usize =
        full.iter().map(|r| r.chars().count()).sum::<usize>() + full.len().saturating_sub(1);
    if total <= budget {
        return full.join("\n");
    }
    // `- ` + `: ` is the four characters of overhead each name carries,
    // beside its own length and its tools suffix.
    let overhead: usize = agents
        .iter()
        .zip(&suffixes)
        .map(|(agent, suffix)| agent.name.chars().count() + suffix.chars().count() + 4)
        .sum::<usize>()
        + agents.len().saturating_sub(1);
    let max_desc = budget.saturating_sub(overhead) / agents.len();
    if max_desc < MIN_DESC_LEN {
        return agents
            .iter()
            .zip(&suffixes)
            .map(|(agent, suffix)| row(agent, "", suffix))
            .collect::<Vec<_>>()
            .join("\n");
    }
    agents
        .iter()
        .zip(&suffixes)
        .map(|(agent, suffix)| {
            row(
                agent,
                &frontmatter::truncate_chars(&agent.description, max_desc),
                suffix,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Below this, a trimmed description says nothing useful and the listing
/// degrades to names only instead ([`crate::skills`]' rule).
const MIN_DESC_LEN: usize = 20;

/// The `<system-reminder>` the derived context leads with: the skills the
/// `Skill` tool can load, then the types the `Agent` tool can launch — one
/// reminder, either section optional, empty when both are.
///
/// One fragment rather than two because they are one kind of thing (what this
/// session can reach that the tool schemas don't already name) and because
/// both are re-rendered per turn: a second fragment would be a second place
/// the prompt-cache prefix can shift (`docs/subagents.md`).
#[must_use]
pub fn reminder_message(skill_listing: &str, agent_listing: &str) -> String {
    let skills = skill_listing.trim();
    let agents = agent_listing.trim();
    let mut sections: Vec<String> = Vec::new();
    if !skills.is_empty() {
        sections.push(format!(
            "{}\n\n{skills}",
            crate::skills::SKILL_LISTING_HEADER
        ));
    }
    if !agents.is_empty() {
        sections.push(format!("{AGENT_LISTING_HEADER}\n\n{agents}"));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!(
        "<system-reminder>\n{}\n</system-reminder>",
        sections.join("\n\n")
    )
}

/// The system prompt a launched subagent of this type carries.
///
/// `base` is the session's own assembled prompt (persona → environment →
/// scratchpad) and `context` is the runtime half of it alone (environment →
/// scratchpad). A definition **with** a body replaces the persona and keeps
/// the context: the date, the os, the cwd and the scratchpad are facts about
/// this session, not personality, and an agent that doesn't know them writes
/// into `/tmp` and guesses the year. A definition without one is exactly what
/// every subagent got before this feature: the session's prompt, plus the
/// note.
///
/// `note` (`prompts/subagent.md`) always closes it — it is what tells the
/// agent its final message is the caller's result. `None` only when there is
/// nothing at all to say, which is the empty-`ALTER_ZERO_SYSTEM_PROMPT`
/// contract (`docs/context.md`).
#[must_use]
pub fn system_prompt_for(
    definition: Option<&AgentDefinition>,
    base: Option<&str>,
    context: Option<&str>,
    note: &str,
) -> Option<String> {
    let override_prompt = definition.and_then(|def| def.system_prompt.as_deref());
    let mut parts: Vec<&str> = Vec::new();
    match override_prompt {
        Some(body) => {
            parts.push(body);
            parts.extend(context);
        }
        None => parts.extend(base),
    }
    parts.push(note);
    let joined = parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.is_empty()).then_some(joined)
}

/// The recoverable error an `agent` call with an unknown `subagent_type`
/// resolves as: the model corrects itself in the same round instead of
/// silently getting a general-purpose agent it didn't ask for.
#[must_use]
pub fn unknown_agent_message(name: &str, available: &[String]) -> String {
    if available.is_empty() {
        return format!("Unknown agent type \"{name}\": no agent types are available.");
    }
    format!(
        "Unknown agent type \"{name}\". Available types: {}",
        available.join(", ")
    )
}

/// The recoverable error a call to a tool this type's `tools:` allowlist
/// withholds resolves as.
///
/// The allowlist is enforced where a call **runs**, not only where the specs
/// are offered: a model can name a tool it was never given — some providers
/// pass one straight through — and "the spec wasn't in the list" is not an
/// answer a `tools: Bash, Read` agent's `write` call should get. Recoverable
/// rather than fatal, so the agent picks another way in the same round.
#[must_use]
pub fn withheld_tool_message(tool: &str, agent_type: &str) -> String {
    format!(
        "The {tool} tool is not available to the {agent_type} agent.          Use one of the tools you were given, or report what you could not do."
    )
}

/// The discovered definitions, shared between the boundary that found them,
/// the launcher that builds a run out of one, and the loop that renders the
/// listing — [`crate::skills::SkillRegistry`]'s shape: one lock, cloned by
/// handle, so a per-turn rescan is a `replace` everyone already sees.
#[derive(Debug, Clone, Default)]
pub struct SubagentRegistry {
    state: Arc<Mutex<Vec<AgentDefinition>>>,
}

impl SubagentRegistry {
    /// A registry holding `agents`, in precedence order.
    #[must_use]
    pub fn new(agents: Vec<AgentDefinition>) -> Self {
        Self {
            state: Arc::new(Mutex::new(agents)),
        }
    }

    /// Every definition, in precedence order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<AgentDefinition> {
        self.lock().clone()
    }

    /// Swap the discovered set — a rescan.
    pub fn replace(&self, agents: Vec<AgentDefinition>) {
        *self.lock() = agents;
    }

    /// The definition `name` selects, tolerating case and stray whitespace:
    /// the model writes the type back from a listing it read a turn ago.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<AgentDefinition> {
        let wanted = name.trim().to_ascii_lowercase();
        self.lock()
            .iter()
            .find(|agent| agent.name.to_ascii_lowercase() == wanted)
            .cloned()
    }

    /// Every type's name, in order — what an unknown-type error lists.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.lock().iter().map(|agent| agent.name.clone()).collect()
    }

    /// Was nothing found at all?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The [`agent_listing`] within `budget`.
    #[must_use]
    pub fn listing(&self, budget: usize) -> String {
        agent_listing(&self.snapshot(), budget)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<AgentDefinition>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(contents: &str) -> AgentDefinition {
        parse_agent(contents, Path::new("/agents/explore.md")).expect("parses")
    }

    // ===== the file =====

    #[test]
    fn reads_the_name_description_model_and_tools_over_the_body() {
        let def = parse(
            "---\nname: reviewer\ndescription: Reviews a diff.\nmodel: kimi-k3\n\
             tools: Bash, Read, mcp__deepwiki__*\n---\n\nYou are a reviewer.\n",
        );
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.description, "Reviews a diff.");
        assert_eq!(def.model, AgentModel::Named("kimi-k3".to_string()));
        assert_eq!(
            def.tools,
            AgentTools::Only(vec![
                "Bash".to_string(),
                "Read".to_string(),
                "mcp__deepwiki__*".to_string()
            ])
        );
        assert_eq!(def.system_prompt.as_deref(), Some("You are a reviewer."));
        assert_eq!(def.path, Path::new("/agents/explore.md"));
    }

    #[test]
    fn the_name_defaults_to_the_file_stem() {
        let def = parse("---\ndescription: Explores.\n---\n");
        assert_eq!(def.name, "explore");
    }

    #[test]
    fn an_omitted_model_and_tools_inherit_everything() {
        let def = parse("---\ndescription: Does things.\n---\n");
        assert_eq!(def.model, AgentModel::Inherit);
        assert_eq!(def.tools, AgentTools::All);
        assert_eq!(
            def.system_prompt, None,
            "a frontmatter-only file overrides no prompt"
        );
    }

    #[test]
    fn inherit_is_spelled_out_rather_than_omitted() {
        assert_eq!(
            parse("---\ndescription: d\nmodel: inherit\n---\n").model,
            AgentModel::Inherit
        );
        assert_eq!(
            parse("---\ndescription: d\nmodel: INHERIT\n---\n").model,
            AgentModel::Inherit
        );
    }

    #[test]
    fn comments_document_the_optional_keys_without_becoming_fields() {
        // The seeded files carry `#` comments explaining `tools:`; the parse
        // must skip them — a comment that parsed as a field would show up in
        // the listing.
        let def = parse(
            "---\ndescription: d\n# tools: Bash, Read — omit for every tool\nname: explore\n---\n",
        );
        assert_eq!(def.tools, AgentTools::All);
        assert_eq!(def.name, "explore");
    }

    #[test]
    fn unknown_keys_are_ignored_not_rejected() {
        let def = parse("---\ndescription: d\ncolor: blue\nallowed-tools: Bash\n---\n");
        assert_eq!(def.description, "d");
    }

    #[test]
    fn a_file_without_frontmatter_or_description_is_refused() {
        assert_eq!(
            parse_agent("no frontmatter", Path::new("x.md")),
            Err(AgentParseError::MissingFrontmatter)
        );
        assert_eq!(
            parse_agent("---\nname: x\n---\nbody", Path::new("x.md")),
            Err(AgentParseError::MissingDescription)
        );
    }

    #[test]
    fn an_unusable_name_is_refused() {
        let err = parse_agent(
            "---\nname: not a name\ndescription: d\n---\n",
            Path::new("x.md"),
        );
        assert!(
            matches!(err, Err(AgentParseError::InvalidName(_))),
            "{err:?}"
        );
        assert!(validate_agent_name("general-purpose").is_ok());
        assert!(
            validate_agent_name("Explore").is_ok(),
            "uppercase is allowed"
        );
        assert!(validate_agent_name("-lead").is_err());
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn a_long_description_is_capped_for_the_listing() {
        let long = "x".repeat(crate::skills::MAX_LISTING_DESC_CHARS + 50);
        let def = parse(&format!("---\ndescription: {long}\n---\n"));
        assert_eq!(
            def.description.chars().count(),
            crate::skills::MAX_LISTING_DESC_CHARS
        );
        assert!(def.description.ends_with('…'));
    }

    // ===== the tools allowlist =====

    #[test]
    fn an_allowlist_admits_its_own_names_case_insensitively() {
        let tools = AgentTools::parse("Bash, Read");
        assert!(tools.allows("bash"));
        assert!(tools.allows("read"));
        assert!(!tools.allows("write"));
        assert!(!tools.allows("edit"));
        assert!(!tools.allows("skill"));
    }

    #[test]
    fn a_star_suffix_globs_a_whole_mcp_server() {
        let tools = AgentTools::parse("Read, mcp__deepwiki__*");
        assert!(tools.allows("mcp__deepwiki__ask_question"));
        assert!(tools.allows("mcp__deepwiki__read_wiki_structure"));
        assert!(!tools.allows("mcp__other__tool"));
        assert!(AgentTools::parse("mcp__*").allows("mcp__other__tool"));
    }

    #[test]
    fn an_omitted_or_starred_list_admits_everything() {
        assert!(AgentTools::All.allows("write"));
        assert_eq!(AgentTools::parse(""), AgentTools::All);
        assert_eq!(AgentTools::parse("*"), AgentTools::All);
        assert!(AgentTools::parse("*").allows("mcp__x__y"));
    }

    #[test]
    fn a_yaml_sequence_reads_like_the_comma_list() {
        // The frontmatter scanner folds a block sequence onto its key, and a
        // flow sequence arrives with brackets: both are the same intent.
        assert_eq!(
            AgentTools::parse("- Bash - Read"),
            AgentTools::parse("Bash, Read")
        );
        assert_eq!(
            AgentTools::parse("[Bash, Read]"),
            AgentTools::parse("Bash, Read")
        );
    }

    #[test]
    fn a_hyphenated_tool_name_survives_the_sequence_dash_strip() {
        let tools = AgentTools::parse("- mcp__srv__do-it - Read");
        assert!(tools.allows("mcp__srv__do-it"), "{tools:?}");
        assert!(tools.allows("read"));
    }

    #[test]
    fn the_summary_is_the_allowlist_as_written() {
        assert_eq!(AgentTools::All.summary(), "*");
        assert_eq!(AgentTools::parse("Bash, Read").summary(), "Bash, Read");
    }

    // ===== the listing =====

    fn defs() -> Vec<AgentDefinition> {
        vec![
            parse_agent(
                "---\nname: general-purpose\ndescription: Does anything.\n---\n",
                Path::new("general-purpose.md"),
            )
            .unwrap(),
            parse_agent(
                "---\nname: explore\ndescription: Searches.\ntools: Bash, Read\n---\n",
                Path::new("explore.md"),
            )
            .unwrap(),
        ]
    }

    #[test]
    fn the_listing_names_each_type_with_its_tools() {
        assert_eq!(
            agent_listing(&defs(), 8_000),
            "- general-purpose: Does anything. (Tools: *)\n\
             - explore: Searches. (Tools: Bash, Read)"
        );
        assert_eq!(agent_listing(&[], 8_000), "");
    }

    #[test]
    fn a_tight_budget_trims_descriptions_before_dropping_a_type() {
        let mut long = defs();
        long[0].description = "a".repeat(200);
        long[1].description = "b".repeat(200);
        let listing = agent_listing(&long, 200);
        assert_eq!(listing.lines().count(), 2, "every type still listed");
        assert!(listing.contains('…'), "descriptions trimmed: {listing}");
        assert!(
            listing.contains("(Tools: Bash, Read)"),
            "tools survive: {listing}"
        );
    }

    #[test]
    fn an_impossible_budget_degrades_to_names_and_tools() {
        let mut long = defs();
        long[0].description = "a".repeat(200);
        long[1].description = "b".repeat(200);
        let listing = agent_listing(&long, 90);
        assert_eq!(
            listing,
            "- general-purpose (Tools: *)\n- explore (Tools: Bash, Read)"
        );
    }

    // ===== the reminder =====

    #[test]
    fn the_reminder_carries_both_sections_in_one_block() {
        let rendered = reminder_message("- dataviz: Charts.", &agent_listing(&defs(), 8_000));
        assert_eq!(
            rendered,
            "<system-reminder>\n\
             The following skills are available for use with the Skill tool:\n\n\
             - dataviz: Charts.\n\n\
             Available agent types for the Agent tool:\n\n\
             - general-purpose: Does anything. (Tools: *)\n\
             - explore: Searches. (Tools: Bash, Read)\n\
             </system-reminder>"
        );
    }

    #[test]
    fn either_section_may_be_absent_and_both_absent_is_empty() {
        let agents_only = reminder_message("", "- explore: Searches. (Tools: Bash, Read)");
        assert!(!agents_only.contains("Skill tool"), "{agents_only}");
        assert!(agents_only.contains(AGENT_LISTING_HEADER));
        let skills_only = reminder_message("- dataviz: Charts.", "");
        assert!(!skills_only.contains(AGENT_LISTING_HEADER), "{skills_only}");
        assert_eq!(reminder_message("", ""), "");
        assert_eq!(reminder_message("   ", "\n"), "");
    }

    #[test]
    fn the_skills_only_reminder_is_still_the_one_subagents_get() {
        // `skills::listing_message` and a skills-only `reminder_message` are
        // the same bytes — a subagent has no `agent` tool, so it gets the
        // skills half alone and must not drift from the lead's wording.
        assert_eq!(
            crate::skills::listing_message("- dataviz: Charts."),
            reminder_message("- dataviz: Charts.", "")
        );
    }

    // ===== the system prompt =====

    #[test]
    fn a_body_replaces_the_persona_but_keeps_the_runtime_context() {
        let def = parse("---\ndescription: d\n---\nYou are the explorer.\n");
        let prompt = system_prompt_for(
            Some(&def),
            Some("PERSONA\n\n## Environment\ncwd: /repo"),
            Some("## Environment\ncwd: /repo"),
            "SUBAGENT NOTE",
        );
        assert_eq!(
            prompt.as_deref(),
            Some("You are the explorer.\n\n## Environment\ncwd: /repo\n\nSUBAGENT NOTE")
        );
    }

    #[test]
    fn no_body_keeps_the_sessions_own_prompt_and_the_note() {
        let def = parse("---\ndescription: d\n---\n");
        assert_eq!(
            system_prompt_for(Some(&def), Some("BASE"), Some("CTX"), "NOTE").as_deref(),
            Some("BASE\n\nNOTE")
        );
        assert_eq!(
            system_prompt_for(None, Some("BASE"), Some("CTX"), "NOTE").as_deref(),
            Some("BASE\n\nNOTE")
        );
    }

    #[test]
    fn a_session_with_no_system_prompt_sends_only_the_note() {
        assert_eq!(
            system_prompt_for(None, None, None, "NOTE").as_deref(),
            Some("NOTE")
        );
        assert_eq!(system_prompt_for(None, None, None, "  "), None);
    }

    // ===== the registry =====

    #[test]
    fn the_registry_finds_a_type_by_name_case_folded() {
        let registry = SubagentRegistry::new(defs());
        assert_eq!(
            registry.find("explore").map(|a| a.name),
            Some("explore".to_string())
        );
        assert_eq!(
            registry.find(" Explore ").map(|a| a.name),
            Some("explore".to_string())
        );
        assert!(registry.find("nope").is_none());
        assert_eq!(registry.names(), vec!["general-purpose", "explore"]);
        assert!(!registry.is_empty());
    }

    #[test]
    fn a_rescan_replaces_the_set_everyone_holds() {
        let registry = SubagentRegistry::new(defs());
        let handle = registry.clone();
        registry.replace(vec![]);
        assert!(handle.is_empty(), "the clone sees the rescan");
        assert_eq!(handle.listing(8_000), "");
    }

    #[test]
    fn a_withheld_tool_names_itself_and_the_type_that_lacks_it() {
        let message = withheld_tool_message("write", "explore");
        assert!(message.starts_with("The write tool is not available to the explore agent."));
    }

    #[test]
    fn an_unknown_type_lists_the_ones_that_exist() {
        let registry = SubagentRegistry::new(defs());
        assert_eq!(
            unknown_agent_message("reviewer", &registry.names()),
            "Unknown agent type \"reviewer\". Available types: general-purpose, explore"
        );
        assert_eq!(
            unknown_agent_message("x", &[]),
            "Unknown agent type \"x\": no agent types are available."
        );
    }
}

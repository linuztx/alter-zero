//! The `Skill` tool's pure model — see `docs/skills.md`.
//!
//! A skill is a directory holding a `SKILL.md`: YAML frontmatter naming and
//! describing it, over a markdown body the model pulls into the conversation
//! on demand. This module holds everything about them that isn't I/O:
//!
//! - the frontmatter **parse** ([`parse_skill`]) and name **validation**
//!   ([`validate_skill_name`]);
//! - the **listing** the model chooses from ([`skill_listing`],
//!   [`listing_message`]) and its character budget ([`listing_budget`]);
//! - the **body render** ([`render_skill_body`]) — the base-directory header,
//!   `$ARGUMENTS` substitution, `${…SKILL_DIR}` expansion, byte cap;
//! - the [`SkillRegistry`] handle the boundary and the loop share.
//!
//! The filesystem walk and the tool executor live in [`crate::llm::skill`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The wire name of the skill-loading tool, lowercase like every other tool
/// name here — which is also what the context replay's lowercasing fallback
/// reproduces from the display name, so a replayed call matches the offered
/// spec.
pub const SKILL_TOOL_NAME: &str = "skill";

/// The display name the cell header wears: `● Skill(dataviz)`.
pub const SKILL_TOOL_DISPLAY: &str = "Skill";

/// The resolved cell's whole `⎿` row. The model reads the skill body instead
/// (the [`crate::llm::tools::ToolOutcome::context`] split) — this is only
/// what the *user* sees, and Claude Code's exact wording.
pub const SKILL_LOADED_DISPLAY: &str = "Successfully loaded skill";

/// The file a skill directory must hold to be one.
pub const SKILL_FILE_NAME: &str = "SKILL.md";

/// The longest name a skill may carry — codex's `MAX_NAME_LEN`.
pub const MAX_SKILL_NAME_LEN: usize = 64;

/// The per-entry description cap in the listing. The listing is for discovery
/// only — the tool loads the full body on invoke, so a verbose description
/// buys no match rate and costs every turn's tokens (the reference's
/// `MAX_LISTING_DESC_CHARS`).
pub const MAX_LISTING_DESC_CHARS: usize = 250;

/// How much of a rendered skill body reaches the model before it is cut
/// (the reference's `maxResultSizeChars`).
pub const SKILL_BODY_MAX_BYTES: usize = 100 * 1024;

/// Appended to a body cut at [`SKILL_BODY_MAX_BYTES`].
pub const SKILL_TRUNCATION_MARKER: &str = "\n\n… (skill truncated)";

/// The listing's character budget when the model's context window is unknown
/// — the reference's `DEFAULT_CHAR_BUDGET` (1% of 200k tokens × 4 chars).
pub const DEFAULT_LISTING_BUDGET: usize = 8_000;

/// Approximate characters per token, for the budget maths.
const CHARS_PER_TOKEN: usize = 4;

/// The share of the context window the listing may take, in percent.
const LISTING_BUDGET_PERCENT: usize = 1;

/// Below this, a trimmed description says nothing useful and the listing
/// degrades to names only instead.
const MIN_DESC_LEN: usize = 20;

/// Is `name` the skill tool's wire name? The predicate the agent loop and the
/// permission seam route on.
#[must_use]
pub fn is_skill_tool(name: &str) -> bool {
    name == SKILL_TOOL_NAME
}

/// One skill discovered on disk: what the listing shows and where the body
/// is. The body itself is **not** held — it is re-read at invoke time, so
/// editing a `SKILL.md` mid-session takes effect on the next call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMetadata {
    /// The invocable name — the frontmatter's `name`, else the directory's.
    pub name: String,
    /// The one-line pitch the model chooses from (already capped).
    pub description: String,
    /// The skill's own directory: the body's `Base directory` header and the
    /// `${…SKILL_DIR}` expansion.
    pub dir: PathBuf,
    /// The `SKILL.md` itself.
    pub path: PathBuf,
}

/// A `SKILL.md` that could not be read, parsed, or validated. Collected
/// rather than thrown: one bad skill must not cost a session the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillError {
    pub path: PathBuf,
    pub message: String,
}

/// A validated `SKILL.md`: its frontmatter and the markdown body under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Why a `SKILL.md` was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillParseError {
    /// No `---`-delimited frontmatter block opened *and* closed the file.
    MissingFrontmatter,
    /// No `description` — the only thing the model sees before invoking, so
    /// a skill without one could never be chosen.
    MissingDescription,
    /// A name that could not be printed, matched, or typed after a `/`.
    InvalidName(String),
}

impl std::fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFrontmatter => {
                write!(f, "missing YAML frontmatter delimited by ---")
            }
            Self::MissingDescription => write!(f, "missing field `description`"),
            Self::InvalidName(reason) => write!(f, "invalid name: {reason}"),
        }
    }
}

/// Is `name` usable as a skill name — safe to print, to match against a tool
/// argument, and to type after a `/`? Lowercase alphanumerics and
/// underscores in `-`-joined segments, at most [`MAX_SKILL_NAME_LEN`] long.
///
/// # Errors
/// Returns the reason the name was refused.
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty".to_string());
    }
    if name.chars().count() > MAX_SKILL_NAME_LEN {
        return Err(format!(
            "exceeds maximum length of {MAX_SKILL_NAME_LEN} characters"
        ));
    }
    // Segments joined by single hyphens, each non-empty — which refuses a
    // leading/trailing hyphen and a `--` run in one rule.
    if name.split('-').any(|segment| {
        segment.is_empty()
            || !segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }) {
        return Err(
            "expected lowercase letters, digits and underscores in `-`-joined segments".to_string(),
        );
    }
    Ok(())
}

/// Parse a `SKILL.md`'s frontmatter and body. `default_name` (the containing
/// directory's name) is used when the frontmatter omits `name`.
///
/// Unknown frontmatter keys are **ignored, not rejected**: a skill authored
/// for Claude Code carries keys we do not implement (`allowed-tools`,
/// `model`, `version`, …), and refusing to parse over one would lock the
/// ecosystem out. `when_to_use` is the exception — the reference appends it
/// to the description, and so do we.
///
/// # Errors
/// [`SkillParseError`] when the frontmatter is absent, the description is
/// missing, or the resolved name is unusable.
pub fn parse_skill(contents: &str, default_name: &str) -> Result<ParsedSkill, SkillParseError> {
    let (frontmatter, body) =
        split_frontmatter(contents).ok_or(SkillParseError::MissingFrontmatter)?;
    let fields = parse_frontmatter_scalars(&frontmatter);

    let name = fields
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_name.to_string());
    validate_skill_name(&name).map_err(SkillParseError::InvalidName)?;

    let description = fields
        .iter()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .ok_or(SkillParseError::MissingDescription)?;
    // The reference's `getCommandDescription`: `description - whenToUse`.
    let when_to_use = fields
        .iter()
        .find(|(key, _)| key == "when_to_use" || key == "whenToUse")
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty());
    let description = match when_to_use {
        Some(when) => format!("{description} - {when}"),
        None => description,
    };

    Ok(ParsedSkill {
        name,
        description: truncate_chars(&description, MAX_LISTING_DESC_CHARS),
        body: body.trim_matches(['\n', '\r']).trim_end().to_string(),
    })
}

/// Split a `---`-delimited frontmatter block off the front of `contents`,
/// returning it and the remaining body. `None` when the block never opens or
/// never closes.
fn split_frontmatter(contents: &str) -> Option<(String, String)> {
    // A leading BOM/blank line must not hide the fence.
    let contents = contents.trim_start_matches('\u{feff}');
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body.push(line);
        } else {
            frontmatter.push(line);
        }
    }
    if !closed || frontmatter.is_empty() {
        return None;
    }
    Some((frontmatter.join("\n"), body.join("\n")))
}

/// The top-level `key: value` scalars of a frontmatter block, in order.
///
/// Deliberately small: plain, quoted and block (`|`/`>`) scalars plus YAML's
/// indented plain-scalar continuation, which is how long descriptions are
/// actually written. Nested maps and sequences parse as a folded string on
/// their key — harmless, since every key that carries one is a key we ignore.
fn parse_frontmatter_scalars(frontmatter: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut fields = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty()
            || line.starts_with([' ', '\t'])
            || line.trim_start().starts_with('#')
        {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        // Gather this key's continuation: every following line that is
        // indented (blank lines inside the block included).
        let start = index;
        while index < lines.len()
            && (lines[index].starts_with([' ', '\t']) || lines[index].trim().is_empty())
        {
            index += 1;
        }
        let continuation = &lines[start..index];
        fields.push((key, scalar_value(rest.trim(), continuation)));
    }
    fields
}

/// One key's value: the same-line scalar folded with any continuation lines.
fn scalar_value(inline: &str, continuation: &[&str]) -> String {
    let literal = inline.starts_with('|');
    if literal || inline.starts_with('>') {
        // A block scalar: the marker line carries nothing but the style.
        let joined: Vec<&str> = continuation
            .iter()
            .map(|line| line.trim())
            .filter(|line| !literal || !line.is_empty())
            .collect();
        return if literal {
            joined.join("\n")
        } else {
            fold(&joined)
        };
    }
    let inline = unquote(inline);
    if continuation.is_empty() {
        return inline;
    }
    let mut parts: Vec<&str> = Vec::new();
    if !inline.is_empty() {
        parts.push(inline.as_str());
    }
    parts.extend(
        continuation
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty()),
    );
    unquote(&fold(&parts))
}

/// Join lines with single spaces, collapsing runs of whitespace — YAML's
/// folded style, and codex's `sanitize_single_line`.
fn fold(parts: &[&str]) -> String {
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip a matching pair of surrounding quotes, undoubling `''` inside a
/// single-quoted scalar. Prose like `description: Deploy: to ECS` is left
/// exactly as written.
fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['\'', '"'] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            let inner = &value[1..value.len() - 1];
            return if quote == '\'' {
                inner.replace("''", "'")
            } else {
                inner.to_string()
            };
        }
    }
    value.to_string()
}

/// `value` cut to `max` characters, the last replaced by `…` when it was.
fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = value.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The listing's character budget for a model whose context window is
/// `context_window` tokens — the reference's
/// `SKILL_BUDGET_CONTEXT_PERCENT × CHARS_PER_TOKEN`, falling back to
/// [`DEFAULT_LISTING_BUDGET`] when the window is unknown.
#[must_use]
pub fn listing_budget(context_window: Option<usize>) -> usize {
    context_window.map_or(DEFAULT_LISTING_BUDGET, |window| {
        window * CHARS_PER_TOKEN * LISTING_BUDGET_PERCENT / 100
    })
}

/// The `- name: description` rows the model chooses from, within `budget`
/// characters.
///
/// Over budget, descriptions are **trimmed to an even share** and — past the
/// point where a share says anything — dropped for names only. Skills are
/// never dropped: one you cannot see is one you cannot invoke, while one with
/// a short description is merely a worse match.
#[must_use]
pub fn skill_listing(skills: &[SkillMetadata], budget: usize) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let full: Vec<String> = skills
        .iter()
        .map(|skill| format!("- {}: {}", skill.name, skill.description))
        .collect();
    let total: usize =
        full.iter().map(|row| row.chars().count()).sum::<usize>() + full.len().saturating_sub(1);
    if total <= budget {
        return full.join("\n");
    }
    // `- ` + `: ` is the four characters of overhead each name carries.
    let overhead: usize = skills
        .iter()
        .map(|skill| skill.name.chars().count() + 4)
        .sum::<usize>()
        + skills.len().saturating_sub(1);
    let max_desc = budget.saturating_sub(overhead) / skills.len();
    if max_desc < MIN_DESC_LEN {
        return skills
            .iter()
            .map(|skill| format!("- {}", skill.name))
            .collect::<Vec<_>>()
            .join("\n");
    }
    skills
        .iter()
        .map(|skill| {
            format!(
                "- {}: {}",
                skill.name,
                truncate_chars(&skill.description, max_desc)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The listing wrapped in the reference's `<system-reminder>` — the leading
/// context fragment `crate::context::context_messages_with` injects. Empty in,
/// empty out: with no skills there is nothing to say, and saying it anyway
/// would spend a turn's tokens telling the model about a tool it isn't
/// offered.
#[must_use]
pub fn listing_message(listing: &str) -> String {
    if listing.trim().is_empty() {
        return String::new();
    }
    format!(
        "<system-reminder>\nThe following skills are available for use with the \
         Skill tool:\n\n{listing}\n</system-reminder>"
    )
}

/// Substitute a skill invocation's `args` into its body: `$ARGUMENTS` for the
/// whole string and `$1`…`$9` for its whitespace-separated words (the
/// reference's `substituteArguments`).
///
/// A body with no placeholder and non-empty args gets them appended, so an
/// argument the model bothered to pass is never silently dropped.
#[must_use]
pub fn substitute_arguments(body: &str, args: &str) -> String {
    let args = args.trim();
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut out = body.to_string();
    let mut substituted = out.contains("$ARGUMENTS");
    out = out.replace("$ARGUMENTS", args);
    for slot in 1..=9usize {
        let placeholder = format!("${slot}");
        if out.contains(&placeholder) {
            substituted = true;
            out = out.replace(&placeholder, positional.get(slot - 1).unwrap_or(&""));
        }
    }
    if !substituted && !args.is_empty() {
        out.push_str("\n\nArguments: ");
        out.push_str(args);
    }
    out
}

/// The text a `skill` call returns to the model: the body with its arguments
/// substituted and its `${…SKILL_DIR}` placeholders expanded, under the
/// base-directory header that makes the skill's relative references
/// resolvable — capped at [`SKILL_BODY_MAX_BYTES`].
#[must_use]
pub fn render_skill_body(dir: &Path, body: &str, args: &str) -> String {
    let dir = dir.display().to_string();
    let text = substitute_arguments(body, args)
        .replace("${CLAUDE_SKILL_DIR}", &dir)
        .replace("${ALTER_ZERO_SKILL_DIR}", &dir);
    let rendered = format!("Base directory for this skill: {dir}\n\n{text}");
    if rendered.len() <= SKILL_BODY_MAX_BYTES {
        return rendered;
    }
    let mut cut = SKILL_BODY_MAX_BYTES;
    while cut > 0 && !rendered.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{SKILL_TRUNCATION_MARKER}", &rendered[..cut])
}

/// The discovered skills, shared between the boundary that found them, the
/// executor that loads one, and the loop that renders the listing — the
/// [`crate::tasks::TaskRegistry`] pattern: one lock, cloned by handle.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    state: Arc<Mutex<RegistryState>>,
}

/// What the registry holds behind its one lock: what was **found** and what
/// the user has since turned **off**. The two are deliberately separate —
/// disabling is a choice about a skill that still exists, so a rescan must
/// not silently re-enable it and an unavailable-here name must not be
/// forgotten (`docs/skills.md`).
#[derive(Debug, Default)]
struct RegistryState {
    skills: Vec<SkillMetadata>,
    disabled: BTreeSet<String>,
}

impl SkillRegistry {
    /// A registry holding `skills`, all enabled.
    #[must_use]
    pub fn new(skills: Vec<SkillMetadata>) -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState {
                skills,
                disabled: BTreeSet::new(),
            })),
        }
    }

    /// **Every** skill, in discovery (precedence) order — the disabled ones
    /// included. This is the `/skills` menu's list: a skill you turned off is
    /// exactly the one you need to see to turn back on.
    #[must_use]
    pub fn snapshot(&self) -> Vec<SkillMetadata> {
        self.lock().skills.clone()
    }

    /// The skills the model is actually offered — [`snapshot`](Self::snapshot)
    /// minus the disabled ones. What the listing and the lookup read.
    #[must_use]
    pub fn enabled(&self) -> Vec<SkillMetadata> {
        let state = self.lock();
        state
            .skills
            .iter()
            .filter(|skill| !state.disabled.contains(&skill.name))
            .cloned()
            .collect()
    }

    /// Swap the discovered set — a rescan. The disabled names are **kept**:
    /// re-finding a skill is not the user changing their mind about it.
    pub fn replace(&self, skills: Vec<SkillMetadata>) {
        self.lock().skills = skills;
    }

    /// Replace the disabled set wholesale — the boundary seeding this
    /// project's saved entry at startup.
    pub fn set_disabled(&self, disabled: BTreeSet<String>) {
        self.lock().disabled = disabled;
    }

    /// The names currently turned off, for the menu and the file.
    #[must_use]
    pub fn disabled(&self) -> BTreeSet<String> {
        self.lock().disabled.clone()
    }

    /// Is `name` currently offered to the model?
    #[must_use]
    pub fn is_enabled(&self, name: &str) -> bool {
        !self.lock().disabled.contains(name)
    }

    /// Flip `name` on or off, returning its **new** enabled state. A name
    /// that isn't installed here is left alone (and reported enabled) rather
    /// than recorded as a phantom entry — the menu can only name real rows,
    /// so this is belt-and-braces for a stale action.
    pub fn toggle(&self, name: &str) -> bool {
        let mut state = self.lock();
        if !state.skills.iter().any(|skill| skill.name == name) {
            return true;
        }
        if state.disabled.remove(name) {
            true
        } else {
            state.disabled.insert(name.to_string());
            false
        }
    }

    /// The skill `name` selects **among the enabled ones**, tolerating the
    /// leading `/` and stray whitespace a model writes about as often as the
    /// bare name. A disabled skill is not found — so a model that remembers
    /// the name from an earlier turn gets the recoverable "unknown skill"
    /// error rather than loading something the user turned off.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<SkillMetadata> {
        let wanted = name.trim().trim_start_matches('/').trim().to_lowercase();
        let state = self.lock();
        state
            .skills
            .iter()
            .find(|skill| {
                skill.name.to_lowercase() == wanted && !state.disabled.contains(&skill.name)
            })
            .cloned()
    }

    /// Every **enabled** skill's name, in order — the "did you mean" list an
    /// unknown-skill error carries, which must not advertise a skill the user
    /// turned off.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.enabled().into_iter().map(|skill| skill.name).collect()
    }

    /// Was **nothing found**? The `/settings` **Skills** row's availability:
    /// a session with no `SKILL.md` anywhere has nothing to toggle.
    /// Distinct from [`has_enabled`](Self::has_enabled) — turning every skill
    /// off is a choice, not an absence.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().skills.is_empty()
    }

    /// Is **anything on**? Whether the `skill` tool is worth offering at all.
    #[must_use]
    pub fn has_enabled(&self) -> bool {
        let state = self.lock();
        state
            .skills
            .iter()
            .any(|skill| !state.disabled.contains(&skill.name))
    }

    /// The **enabled** skills' [`skill_listing`] within `budget`.
    #[must_use]
    pub fn listing(&self, budget: usize) -> String {
        skill_listing(&self.enabled(), budget)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The `skills.json` format: which skills the user has turned **off**, per
/// project (`permissions.json`'s shape exactly — `docs/skills.md`).
///
/// Per project because skill relevance is project-specific: a `dataviz` skill
/// earns its listing tokens in an analytics repo and not in a kernel driver.
/// The session-wide switch is the `/settings` **Skills** row, so this file is
/// the finer scope rather than a second copy of the same decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillsFile {
    /// One entry per project directory (its absolute path). Omitted when
    /// empty, so a file with nothing turned off anywhere really is `{}` — the
    /// format stays a diff from "everything on" rather than accumulating
    /// husks of projects the user re-enabled.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub projects: std::collections::BTreeMap<String, ProjectSkills>,
}

/// One project's turned-off skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectSkills {
    /// The names the user turned off, sorted so the file is stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<String>,
}

impl SkillsFile {
    /// Parse a `skills.json` body, best-effort: malformed or empty JSON reads
    /// as nothing disabled (`PermissionsFile::parse`'s posture — a corrupt
    /// file must never cost a session its skills, nor block startup).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize back, pretty-printed like the sibling config files.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The names turned off for `project` — empty for one never recorded.
    #[must_use]
    pub fn disabled_for(&self, project: &str) -> BTreeSet<String> {
        self.projects
            .get(project)
            .map(|entry| entry.disabled.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Record `project`'s turned-off set, replacing whatever it held. An
    /// **empty** set drops the entry entirely, so re-enabling the last skill
    /// leaves no residue and the file stays a diff from "everything on".
    pub fn record(&mut self, project: &str, disabled: &BTreeSet<String>) {
        if disabled.is_empty() {
            self.projects.remove(project);
            return;
        }
        self.projects.insert(
            project.to_string(),
            ProjectSkills {
                disabled: disabled.iter().cloned().collect(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== frontmatter parse =====

    #[test]
    fn parse_reads_the_name_and_description_over_the_body() {
        let parsed = parse_skill(
            "---\nname: dataviz\ndescription: Charts and dashboards.\n---\n\n# Data viz\n\nBody.\n",
            "ignored",
        )
        .expect("parses");
        assert_eq!(parsed.name, "dataviz");
        assert_eq!(parsed.description, "Charts and dashboards.");
        assert_eq!(parsed.body, "# Data viz\n\nBody.");
    }

    #[test]
    fn a_missing_name_defaults_to_the_directory_name() {
        let parsed = parse_skill("---\ndescription: Does a thing.\n---\nBody.", "review-pr")
            .expect("parses");
        assert_eq!(parsed.name, "review-pr");
    }

    #[test]
    fn frontmatter_is_required() {
        assert!(matches!(
            parse_skill("# Just markdown\n", "x"),
            Err(SkillParseError::MissingFrontmatter)
        ));
        // An opening fence with no closing one is not frontmatter either.
        assert!(matches!(
            parse_skill("---\nname: x\n", "x"),
            Err(SkillParseError::MissingFrontmatter)
        ));
    }

    #[test]
    fn a_description_is_required() {
        // It is the only thing the model sees before invoking, so a skill
        // without one could never be chosen.
        assert!(matches!(
            parse_skill("---\nname: x\n---\nBody.", "x"),
            Err(SkillParseError::MissingDescription)
        ));
        assert!(matches!(
            parse_skill("---\nname: x\ndescription:   \n---\nBody.", "x"),
            Err(SkillParseError::MissingDescription)
        ));
    }

    #[test]
    fn a_name_that_could_not_be_typed_after_a_slash_is_refused() {
        assert!(matches!(
            parse_skill("---\nname: Not Valid!\ndescription: d\n---\nb", "x"),
            Err(SkillParseError::InvalidName(_))
        ));
        assert!(matches!(
            parse_skill(
                &format!("---\nname: {}\ndescription: d\n---\nb", "a".repeat(65)),
                "x"
            ),
            Err(SkillParseError::InvalidName(_))
        ));
    }

    #[test]
    fn validate_skill_name_accepts_the_reference_shape() {
        assert!(validate_skill_name("commit").is_ok());
        assert!(validate_skill_name("review-pr").is_ok());
        assert!(validate_skill_name("pdf2text").is_ok());
        // Underscores are safe to print, match and type, so they are allowed —
        // a `SKILL.md` in a `my_skill/` directory should not be unreachable.
        assert!(validate_skill_name("my_skill").is_ok());
        for bad in ["", "-lead", "trail-", "Upper", "has space", "a--b", "../x"] {
            assert!(validate_skill_name(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn quoted_and_folded_scalars_parse() {
        // Ecosystem skills quote descriptions containing a colon, and fold
        // long ones across continuation lines.
        let parsed = parse_skill(
            "---\nname: \"aws\"\ndescription: 'Deploy: to ECS'\n---\nb",
            "x",
        )
        .expect("parses");
        assert_eq!(parsed.description, "Deploy: to ECS");

        let folded = parse_skill(
            "---\nname: x\ndescription: >-\n  One line\n  and another\n---\nb",
            "x",
        )
        .expect("parses");
        assert_eq!(folded.description, "One line and another");
    }

    #[test]
    fn unknown_frontmatter_keys_are_ignored_not_rejected() {
        // A skill authored for the reference carries keys we do not implement;
        // refusing to parse over one would lock the ecosystem out.
        let parsed = parse_skill(
            "---\nname: pdf\ndescription: PDFs.\nallowed-tools: Bash(python3:*)\nmodel: opus\nversion: 2\n---\nb",
            "x",
        )
        .expect("parses");
        assert_eq!(parsed.name, "pdf");
        assert_eq!(parsed.description, "PDFs.");
    }

    #[test]
    fn when_to_use_is_appended_to_the_description() {
        // The reference's `getCommandDescription`: `description - whenToUse`.
        let parsed = parse_skill(
            "---\nname: x\ndescription: Review a PR.\nwhen_to_use: when asked to review changes\n---\nb",
            "x",
        )
        .expect("parses");
        assert_eq!(
            parsed.description,
            "Review a PR. - when asked to review changes"
        );
    }

    #[test]
    fn a_description_is_capped_for_the_listing() {
        let long = "d".repeat(MAX_LISTING_DESC_CHARS + 50);
        let parsed = parse_skill(&format!("---\nname: x\ndescription: {long}\n---\nb"), "x")
            .expect("parses");
        assert_eq!(parsed.description.chars().count(), MAX_LISTING_DESC_CHARS);
        assert!(parsed.description.ends_with('…'));
    }

    // ===== the listing =====

    fn meta(name: &str, description: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            dir: PathBuf::from("/skills").join(name),
            path: PathBuf::from("/skills").join(name).join(SKILL_FILE_NAME),
        }
    }

    #[test]
    fn the_listing_is_one_dashed_row_per_skill() {
        let skills = [meta("commit", "Create a git commit"), meta("pdf", "PDFs")];
        assert_eq!(
            skill_listing(&skills, DEFAULT_LISTING_BUDGET),
            "- commit: Create a git commit\n- pdf: PDFs"
        );
    }

    #[test]
    fn the_listing_message_wears_the_references_system_reminder() {
        let msg = listing_message("- commit: Create a git commit");
        assert!(msg.starts_with("<system-reminder>\n"), "got {msg}");
        assert!(msg.ends_with("\n</system-reminder>"), "got {msg}");
        assert!(msg.contains("available for use with the Skill tool"));
        assert!(msg.contains("- commit: Create a git commit"));
    }

    #[test]
    fn an_empty_listing_renders_nothing_at_all() {
        assert_eq!(skill_listing(&[], DEFAULT_LISTING_BUDGET), "");
        assert_eq!(listing_message(""), "");
    }

    #[test]
    fn an_over_budget_listing_trims_descriptions_rather_than_dropping_skills() {
        // A skill you cannot see is a skill you cannot invoke; a skill with a
        // short description is merely a worse match.
        let skills: Vec<SkillMetadata> = (0..10)
            .map(|i| meta(&format!("skill-{i}"), &"d".repeat(200)))
            .collect();
        let listing = skill_listing(&skills, 400);
        assert!(listing.chars().count() <= 400, "over budget: {listing}");
        for skill in &skills {
            assert!(
                listing.contains(&format!("- {}:", skill.name)),
                "{} was dropped",
                skill.name
            );
        }
    }

    #[test]
    fn a_hopeless_budget_degrades_to_names_only() {
        let skills: Vec<SkillMetadata> = (0..10)
            .map(|i| meta(&format!("skill-{i}"), &"d".repeat(200)))
            .collect();
        let listing = skill_listing(&skills, 120);
        assert_eq!(
            listing.lines().next(),
            Some("- skill-0"),
            "names only: {listing}"
        );
        assert_eq!(listing.lines().count(), 10, "every skill still listed");
    }

    #[test]
    fn the_budget_is_one_percent_of_the_context_window_in_characters() {
        // The reference's SKILL_BUDGET_CONTEXT_PERCENT × CHARS_PER_TOKEN.
        assert_eq!(listing_budget(Some(200_000)), 8_000);
        assert_eq!(listing_budget(None), DEFAULT_LISTING_BUDGET);
    }

    // ===== the body =====

    #[test]
    fn the_body_leads_with_its_base_directory() {
        // So relative references inside the skill resolve.
        let rendered = render_skill_body(Path::new("/skills/pdf"), "Read ./forms.md", "");
        assert_eq!(
            rendered,
            "Base directory for this skill: /skills/pdf\n\nRead ./forms.md"
        );
    }

    #[test]
    fn skill_dir_placeholders_expand_to_the_base_directory() {
        let rendered = render_skill_body(
            Path::new("/skills/pdf"),
            "run ${CLAUDE_SKILL_DIR}/go.py and ${ALTER_ZERO_SKILL_DIR}/x",
            "",
        );
        assert!(
            rendered.contains("run /skills/pdf/go.py and /skills/pdf/x"),
            "{rendered}"
        );
    }

    #[test]
    fn arguments_substitute_for_the_placeholder() {
        assert_eq!(
            substitute_arguments("Review PR $ARGUMENTS now", "123"),
            "Review PR 123 now"
        );
    }

    #[test]
    fn positional_arguments_split_on_whitespace() {
        assert_eq!(
            substitute_arguments("$1 then $2 then $3", "alpha beta"),
            "alpha then beta then "
        );
    }

    #[test]
    fn arguments_with_no_placeholder_are_appended_never_dropped() {
        assert_eq!(
            substitute_arguments("Do the thing.", "quarterly revenue"),
            "Do the thing.\n\nArguments: quarterly revenue"
        );
        // …and nothing is appended when there are none.
        assert_eq!(substitute_arguments("Do the thing.", "  "), "Do the thing.");
    }

    #[test]
    fn a_huge_body_is_truncated_with_a_marker_rather_than_refused() {
        let body = "x".repeat(SKILL_BODY_MAX_BYTES + 1_000);
        let rendered = render_skill_body(Path::new("/s"), &body, "");
        assert!(
            rendered.len() <= SKILL_BODY_MAX_BYTES + 200,
            "{}",
            rendered.len()
        );
        assert!(rendered.ends_with(SKILL_TRUNCATION_MARKER), "no marker");
    }

    #[test]
    fn truncation_never_splits_a_character() {
        let body = "é".repeat(SKILL_BODY_MAX_BYTES);
        let rendered = render_skill_body(Path::new("/s"), &body, "");
        assert!(rendered.ends_with(SKILL_TRUNCATION_MARKER));
    }

    // ===== the registry =====

    #[test]
    fn the_registry_finds_a_skill_by_name() {
        let registry = SkillRegistry::new(vec![meta("commit", "d"), meta("pdf", "d")]);
        assert_eq!(registry.find("pdf").expect("found").name, "pdf");
        assert!(registry.find("nope").is_none());
        assert_eq!(
            registry.names(),
            vec!["commit".to_string(), "pdf".to_string()]
        );
        assert!(!registry.is_empty());
        assert!(SkillRegistry::new(vec![]).is_empty());
    }

    #[test]
    fn a_leading_slash_and_stray_space_still_find_the_skill() {
        // The model writes `/commit` about as often as `commit`.
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        assert!(registry.find(" /commit ").is_some());
    }

    #[test]
    fn the_registry_is_a_shared_handle() {
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        let clone = registry.clone();
        registry.replace(vec![meta("pdf", "d")]);
        assert!(clone.find("pdf").is_some(), "the clone sees the swap");
    }

    // ===== enabling / disabling one skill (`/skills`, docs/skills.md) =====

    #[test]
    fn a_disabled_skill_leaves_the_listing_and_the_lookup() {
        // Both halves matter: dropping it from the listing stops the model
        // choosing it, and refusing it in the lookup stops a model that
        // remembers the name from an earlier turn loading it anyway.
        let registry = SkillRegistry::new(vec![meta("commit", "d"), meta("pdf", "d")]);
        registry.set_disabled(["pdf".to_string()].into_iter().collect());

        assert!(registry.find("pdf").is_none(), "the lookup refuses it");
        assert!(registry.find("commit").is_some());
        assert_eq!(registry.listing(DEFAULT_LISTING_BUDGET), "- commit: d");
        assert_eq!(registry.names(), vec!["commit".to_string()]);
    }

    #[test]
    fn the_menu_still_sees_every_skill_including_the_disabled_ones() {
        // `snapshot` is the picker's list — a skill you turned off must stay
        // visible there or you could never turn it back on.
        let registry = SkillRegistry::new(vec![meta("commit", "d"), meta("pdf", "d")]);
        registry.set_disabled(["pdf".to_string()].into_iter().collect());
        assert_eq!(registry.snapshot().len(), 2);
        assert!(registry.is_enabled("commit"));
        assert!(!registry.is_enabled("pdf"));
    }

    #[test]
    fn toggling_flips_one_skill_and_reports_its_new_state() {
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        assert!(!registry.toggle("commit"), "was on, now off");
        assert!(!registry.is_enabled("commit"));
        assert!(registry.toggle("commit"), "was off, now on");
        assert!(registry.is_enabled("commit"));
    }

    #[test]
    fn toggling_an_unknown_name_changes_nothing() {
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        assert!(registry.toggle("nope"));
        assert!(registry.disabled().is_empty(), "no phantom entry recorded");
    }

    #[test]
    fn discovery_and_selection_are_different_questions() {
        // `is_empty` asks whether anything was FOUND (the /settings row's
        // availability); `has_enabled` asks whether anything is currently ON
        // (whether the tool is worth offering). Turning everything off must
        // not read as "no skills installed".
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        assert!(!registry.is_empty() && registry.has_enabled());
        registry.set_disabled(["commit".to_string()].into_iter().collect());
        assert!(!registry.is_empty(), "one is still installed");
        assert!(!registry.has_enabled(), "…but none is on");
    }

    #[test]
    fn a_replace_keeps_the_disabled_set() {
        // A rescan must not silently re-enable what the user turned off.
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        registry.set_disabled(["commit".to_string()].into_iter().collect());
        registry.replace(vec![meta("commit", "d"), meta("pdf", "d")]);
        assert!(!registry.is_enabled("commit"));
        assert!(registry.is_enabled("pdf"));
    }

    // ===== the per-project file =====

    #[test]
    fn the_disabled_set_round_trips_through_the_file() {
        let mut file = SkillsFile::default();
        file.record("/work", &["pdf".to_string()].into_iter().collect());
        let reparsed = SkillsFile::parse(&file.to_json());
        assert_eq!(
            reparsed.disabled_for("/work"),
            ["pdf".to_string()].into_iter().collect()
        );
        assert!(reparsed.disabled_for("/elsewhere").is_empty());
    }

    #[test]
    fn recording_one_project_leaves_the_others_alone() {
        // The read-modify-write `permissions.json` does: two projects, two
        // entries, neither clobbering the other.
        let mut file = SkillsFile::default();
        file.record("/a", &["x".to_string()].into_iter().collect());
        file.record("/b", &["y".to_string()].into_iter().collect());
        assert_eq!(
            file.disabled_for("/a"),
            ["x".to_string()].into_iter().collect()
        );
        assert_eq!(
            file.disabled_for("/b"),
            ["y".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn a_project_with_nothing_disabled_is_dropped_from_the_file() {
        // Re-enabling the last skill should leave no entry behind, so the file
        // stays a diff from "everything on" rather than accumulating noise.
        let mut file = SkillsFile::default();
        file.record("/work", &["x".to_string()].into_iter().collect());
        file.record("/work", &std::collections::BTreeSet::new());
        assert_eq!(file.to_json().replace(char::is_whitespace, ""), "{}");
    }

    #[test]
    fn a_malformed_file_reads_as_nothing_disabled() {
        // `PermissionsFile::parse`'s posture — a corrupt file must never cost
        // a session its skills, and never block startup.
        assert!(
            SkillsFile::parse("not json")
                .disabled_for("/work")
                .is_empty()
        );
        assert!(SkillsFile::parse("").disabled_for("/work").is_empty());
    }

    #[test]
    fn a_disabled_name_that_is_not_installed_here_is_kept() {
        // The same file serves a project checked out on another machine, where
        // that skill exists. Pruning on load would silently re-enable it there.
        let file = SkillsFile::parse(r#"{"projects":{"/work":{"disabled":["gone"]}}}"#);
        let registry = SkillRegistry::new(vec![meta("commit", "d")]);
        registry.set_disabled(file.disabled_for("/work"));
        assert_eq!(
            registry.disabled(),
            ["gone".to_string()].into_iter().collect(),
            "kept verbatim"
        );
        assert!(registry.is_enabled("commit"));
    }

    #[test]
    fn is_skill_tool_matches_only_the_wire_name() {
        assert!(is_skill_tool(SKILL_TOOL_NAME));
        assert!(!is_skill_tool("skills"));
        assert!(!is_skill_tool("bash"));
    }
}

//! AGENTS.md discovery — codex's project doc, in the context window's
//! `<system-reminder>` (see `docs/project-doc.md`).
//!
//! `/init` generates an `AGENTS.md` contributor guide; this module reads it
//! (and any nested ones) back so every turn's context carries the project's
//! own instructions. The discovery is codex's
//! (`codex-rs/core/src/agents_md.rs`): find the project root by walking up
//! from the cwd to the nearest `.git` marker, collect every `AGENTS.md` from
//! the root *down to* the cwd, cap the total at 32 KiB. The rendering is the
//! reference tool's: each file under its own `Contents of {path} (project
//! instructions, checked into the codebase):` heading, behind a paragraph
//! saying the instructions override the defaults — the **instructions
//! section** of the one `<system-reminder>` the derived context leads with
//! ([`crate::reminder`], [`context::context_messages_full`]), where the
//! skills and agent-type listings follow it.
//!
//! The chain/budget/section logic is pure; [`find_project_root`] and
//! [`load_user_instructions`] are the small fs boundary (stat + read),
//! tempfile-tested in-module like the `checkpoint` store's I/O.
//!
//! [`context::context_messages_full`]: crate::context::context_messages_full

use std::path::{Path, PathBuf};

/// Codex's `project_doc_max_bytes` default: the total byte budget across
/// every discovered `AGENTS.md`. Overridable via
/// `ALTER_ZERO_PROJECT_DOC_MAX_BYTES` ([`doc_budget`] — `0` disables
/// loading entirely, codex's `max_total == 0` early out).
pub const PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;

/// The filename scanned for project instructions (codex's
/// `DEFAULT_AGENTS_MD_FILENAME`).
pub const PROJECT_DOC_FILENAME: &str = "AGENTS.md";

/// The git-ignorable local override (codex's `LOCAL_AGENTS_MD_FILENAME`):
/// per directory the first existing [`PROJECT_DOC_FILENAMES`] candidate
/// contributes, so a personal override replaces its directory's checked-in
/// guide without touching the repo.
pub const PROJECT_DOC_OVERRIDE_FILENAME: &str = "AGENTS.override.md";

/// Candidate filenames per directory, in priority order.
pub const PROJECT_DOC_FILENAMES: [&str; 2] = [PROJECT_DOC_OVERRIDE_FILENAME, PROJECT_DOC_FILENAME];

/// The byte budget from `ALTER_ZERO_PROJECT_DOC_MAX_BYTES`'s value: a parsed
/// number wins (`0` = off), anything else — unset, blank, garbage — is
/// codex's [`PROJECT_DOC_MAX_BYTES`] default. Pure (the boundary hands in the
/// env read), like `term`'s env predicate.
#[must_use]
pub fn doc_budget(env_value: Option<&str>) -> usize {
    env_value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(PROJECT_DOC_MAX_BYTES)
}

/// The directories searched for a project doc: the project root down to the
/// cwd, inclusive — codex's cursor walk (cwd up to `root`, then reversed so
/// the outermost guide comes first and an inner directory's refines it).
/// With no root the cwd alone is searched; a `root` that is not an ancestor
/// of `cwd` degrades the same way (codex cannot produce one — its root comes
/// from walking up from the cwd — so neither can our loader).
#[must_use]
pub fn doc_chain(root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    let Some(root) = root else {
        return vec![cwd.to_path_buf()];
    };
    let mut dirs = Vec::new();
    let mut cursor = cwd;
    loop {
        dirs.push(cursor.to_path_buf());
        if cursor == root {
            break;
        }
        let Some(parent) = cursor.parent() else {
            // Reached the filesystem root without meeting `root`: not an
            // ancestor — fall back to the cwd alone, like no root at all.
            return vec![cwd.to_path_buf()];
        };
        cursor = parent;
    }
    dirs.reverse();
    dirs
}

/// One discovered project doc: the file it was read from and what it said,
/// once [`budget_docs`] has cut it to the budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDoc {
    /// The file's path as the chain named it — what its heading shows
    /// ([`doc_heading`]), so the model knows which directory an instruction
    /// governs.
    pub path: PathBuf,
    /// The file's text: lossily decoded, budget-truncated.
    pub text: String,
}

/// Keep discovered docs under codex's byte budget: each doc is truncated to
/// the remaining budget (on a `char` boundary — codex truncates raw bytes and
/// lossy-decodes; our input is already `String`), blank docs are skipped and
/// cost nothing, and collection stops once the budget is spent. Empty when
/// nothing survives.
#[must_use]
pub fn budget_docs(docs: Vec<ProjectDoc>, max_bytes: usize) -> Vec<ProjectDoc> {
    let mut remaining = max_bytes;
    let mut kept = Vec::new();
    for ProjectDoc { path, mut text } in docs {
        if remaining == 0 {
            break;
        }
        text.truncate(fitted_len(&text, remaining));
        if text.trim().is_empty() {
            continue;
        }
        remaining -= text.len();
        kept.push(ProjectDoc { path, text });
    }
    kept
}

/// The length of the widest prefix of `text` within `max_bytes` that ends on
/// a `char` boundary (the whole text when it already fits).
fn fitted_len(text: &str, max_bytes: usize) -> usize {
    if text.len() <= max_bytes {
        return text.len();
    }
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    end
}

/// The paragraph the instructions section opens with — the reference tool's
/// own wording, which a model trained on it already reads as binding.
pub const INSTRUCTIONS_PREAMBLE: &str = "Codebase and user instructions are shown below. Be sure \
                                         to adhere to these instructions. IMPORTANT: These \
                                         instructions OVERRIDE any default behavior and you MUST \
                                         follow them exactly as written.";

/// The caption a checked-in guide's heading carries.
pub const DOC_CHECKED_IN_NOTE: &str = "project instructions, checked into the codebase";

/// The caption the git-ignorable override's heading carries
/// ([`PROJECT_DOC_OVERRIDE_FILENAME`]) — a private file is not "checked into
/// the codebase", and where an instruction came from is part of what it
/// means.
pub const DOC_OVERRIDE_NOTE: &str = "user's private project instructions, not checked in";

/// The heading a doc's contents sit under: `Contents of {path} ({note}):`,
/// the note saying whether the file is the checked-in guide or the private
/// override.
#[must_use]
pub fn doc_heading(path: &Path) -> String {
    let is_override =
        path.file_name().and_then(|name| name.to_str()) == Some(PROJECT_DOC_OVERRIDE_FILENAME);
    let note = if is_override {
        DOC_OVERRIDE_NOTE
    } else {
        DOC_CHECKED_IN_NOTE
    };
    format!("Contents of {} ({note}):", path.display())
}

/// Render the budgeted docs as the `<system-reminder>`'s instructions
/// section ([`crate::reminder`]): the [`INSTRUCTIONS_PREAMBLE`], then each
/// doc's [`doc_heading`] over its text, blank-line separated:
///
/// ```text
/// Codebase and user instructions are shown below. Be sure to adhere to …
///
/// Contents of /repo/AGENTS.md (project instructions, checked into the codebase):
///
/// {text}
///
/// Contents of /repo/app/AGENTS.md (project instructions, checked into the codebase):
///
/// {text}
/// ```
///
/// Empty with no docs — the section is then simply absent from the reminder.
#[must_use]
pub fn instructions_section(docs: &[ProjectDoc]) -> String {
    if docs.is_empty() {
        return String::new();
    }
    let mut parts = vec![INSTRUCTIONS_PREAMBLE.to_string()];
    parts.extend(
        docs.iter()
            .map(|doc| format!("{}\n\n{}", doc_heading(&doc.path), doc.text.trim())),
    );
    parts.join("\n\n")
}

/// The nearest ancestor-or-self of `cwd` containing a `.git` marker — a
/// directory *or* a worktree's gitfile (codex's default
/// `project_root_markers`). `None` when no ancestor carries one. Boundary:
/// one stat per ancestor.
#[must_use]
pub fn find_project_root(cwd: &Path) -> Option<PathBuf> {
    let mut cursor = cwd;
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor.to_path_buf());
        }
        cursor = cursor.parent()?;
    }
}

/// Load the project's AGENTS.md instructions for `cwd`, rendered as the
/// reminder's [`instructions_section`] — or `None` when no doc exists. The
/// budget comes from `ALTER_ZERO_PROJECT_DOC_MAX_BYTES` via [`doc_budget`]
/// (`0` disables loading entirely).
#[must_use]
pub fn load_user_instructions(cwd: &Path) -> Option<String> {
    load_user_instructions_with(
        cwd,
        doc_budget(
            std::env::var("ALTER_ZERO_PROJECT_DOC_MAX_BYTES")
                .ok()
                .as_deref(),
        ),
    )
}

/// [`load_user_instructions`] under an explicit budget. Boundary: composes
/// [`find_project_root`], [`doc_chain`], one `read_first_doc` per chain
/// directory, [`budget_docs`], [`instructions_section`]. A missing or
/// unreadable file is simply skipped (codex ignores NotFound); a zero budget
/// skips even the discovery.
#[must_use]
pub fn load_user_instructions_with(cwd: &Path, max_bytes: usize) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    let root = find_project_root(cwd);
    let docs: Vec<ProjectDoc> = doc_chain(root.as_deref(), cwd)
        .iter()
        .filter_map(|dir| read_first_doc(dir, max_bytes))
        .collect();
    let docs = budget_docs(docs, max_bytes);
    (!docs.is_empty()).then(|| instructions_section(&docs))
}

/// The first readable [`PROJECT_DOC_FILENAMES`] candidate in `dir`, read
/// under `cap` bytes and decoded lossily — codex reads bytes and
/// `from_utf8_lossy`s them, so one stray invalid byte can never silently
/// drop the whole guide (the `read_to_string` trap). A doc cut at the cap
/// may end in a replacement char; codex truncates raw bytes the same way.
fn read_first_doc(dir: &Path, cap: usize) -> Option<ProjectDoc> {
    PROJECT_DOC_FILENAMES.iter().find_map(|name| {
        let path = dir.join(name);
        let bytes = read_capped(&path, cap)?;
        Some(ProjectDoc {
            path,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        })
    })
}

/// At most `cap` bytes of `path`. The budget bounds the **I/O**, not just
/// the folded output: a huge file that merely happens to be named
/// `AGENTS.md` must never be slurped whole — this loader runs at startup
/// *and* at every turn start, so an unbounded read would block the raw-mode
/// terminal repeatedly (the `main.rs::read_capped` pattern, and the same
/// class of bug as the "hangs in `~`" checkpoint guard). `None` when the
/// file is missing or unreadable — the caller then tries the next candidate.
fn read_capped(path: &Path, cap: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.take(u64::try_from(cap).unwrap_or(u64::MAX))
        .read_to_end(&mut data)
        .ok()?;
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- doc_budget (the ALTER_ZERO_PROJECT_DOC_MAX_BYTES knob) ---

    #[test]
    fn doc_budget_defaults_and_parses_the_env_override() {
        // Unset / blank / garbage → codex's 32 KiB default; a number wins;
        // `0` is the documented off switch (it flows into the zero-budget
        // early return below).
        assert_eq!(doc_budget(None), PROJECT_DOC_MAX_BYTES);
        assert_eq!(doc_budget(Some("")), PROJECT_DOC_MAX_BYTES);
        assert_eq!(doc_budget(Some("  ")), PROJECT_DOC_MAX_BYTES);
        assert_eq!(doc_budget(Some("not a number")), PROJECT_DOC_MAX_BYTES);
        assert_eq!(doc_budget(Some("1234")), 1234);
        assert_eq!(doc_budget(Some(" 64 ")), 64);
        assert_eq!(doc_budget(Some("0")), 0);
    }

    // --- doc_chain ---

    #[test]
    fn doc_chain_runs_root_to_cwd_inclusive() {
        let chain = doc_chain(Some(Path::new("/repo")), Path::new("/repo/a/b"));
        assert_eq!(
            chain,
            vec![
                PathBuf::from("/repo"),
                PathBuf::from("/repo/a"),
                PathBuf::from("/repo/a/b"),
            ]
        );
    }

    #[test]
    fn doc_chain_with_root_at_cwd_is_just_the_cwd() {
        let chain = doc_chain(Some(Path::new("/repo")), Path::new("/repo"));
        assert_eq!(chain, vec![PathBuf::from("/repo")]);
    }

    #[test]
    fn doc_chain_without_a_root_is_just_the_cwd() {
        let chain = doc_chain(None, Path::new("/somewhere/deep"));
        assert_eq!(chain, vec![PathBuf::from("/somewhere/deep")]);
    }

    #[test]
    fn doc_chain_with_a_non_ancestor_root_degrades_to_the_cwd() {
        let chain = doc_chain(Some(Path::new("/elsewhere")), Path::new("/repo/a"));
        assert_eq!(chain, vec![PathBuf::from("/repo/a")]);
    }

    /// A discovered doc at `path` saying `text`.
    fn doc(path: &str, text: &str) -> ProjectDoc {
        ProjectDoc {
            path: PathBuf::from(path),
            text: text.to_string(),
        }
    }

    // --- budget_docs ---

    #[test]
    fn budget_docs_keeps_every_doc_in_order_under_the_budget() {
        let docs = vec![
            doc("/repo/AGENTS.md", "root guide"),
            doc("/repo/app/AGENTS.md", "inner guide"),
        ];
        assert_eq!(budget_docs(docs.clone(), 1024), docs);
    }

    #[test]
    fn budget_docs_skips_blank_docs_for_free() {
        let docs = vec![
            doc("/repo/AGENTS.md", "  \n\t"),
            doc("/repo/app/AGENTS.md", "real"),
        ];
        assert_eq!(
            budget_docs(docs, 1024),
            vec![doc("/repo/app/AGENTS.md", "real")]
        );
    }

    #[test]
    fn budget_docs_is_empty_when_nothing_survives() {
        assert!(budget_docs(vec![], 1024).is_empty());
        assert!(budget_docs(vec![doc("/repo/AGENTS.md", "   ")], 1024).is_empty());
        // A zero budget disables the whole feature, codex-style.
        assert!(budget_docs(vec![doc("/repo/AGENTS.md", "doc")], 0).is_empty());
    }

    #[test]
    fn budget_docs_truncates_the_overflowing_doc_and_stops() {
        let docs = vec![
            doc("/a/AGENTS.md", "12345678"),
            doc("/a/b/AGENTS.md", "abcdefgh"),
            doc("/a/b/c/AGENTS.md", "never"),
        ];
        // 8 bytes of doc one + 4 of doc two; doc three finds no budget left.
        assert_eq!(
            budget_docs(docs, 12),
            vec![
                doc("/a/AGENTS.md", "12345678"),
                doc("/a/b/AGENTS.md", "abcd"),
            ]
        );
    }

    #[test]
    fn budget_docs_truncates_on_a_char_boundary() {
        // 'é' is two bytes: a 3-byte budget keeps "aé", never splits it.
        assert_eq!(
            budget_docs(vec![doc("/a/AGENTS.md", "aéé")], 3),
            vec![doc("/a/AGENTS.md", "aé")]
        );
    }

    // --- the instructions section (docs/project-doc.md) ---

    #[test]
    fn doc_heading_names_the_file_and_whether_it_is_checked_in() {
        // The reference's own captions: a checked-in guide says so, and the
        // git-ignorable override says the opposite — a `Contents of` line
        // claiming a private file is in the codebase would be a lie about
        // where the instruction came from.
        assert_eq!(
            doc_heading(Path::new("/repo/AGENTS.md")),
            "Contents of /repo/AGENTS.md (project instructions, checked into the codebase):"
        );
        assert_eq!(
            doc_heading(Path::new("/repo/AGENTS.override.md")),
            "Contents of /repo/AGENTS.override.md (user's private project instructions, not checked in):"
        );
    }

    #[test]
    fn instructions_section_lists_each_doc_under_its_own_heading() {
        // The retired codex fragment folded every doc into one
        // `<INSTRUCTIONS>` body under the cwd's name; the section names each
        // file, so the model knows which directory an instruction governs.
        let docs = vec![
            doc("/repo/AGENTS.md", "Root guide.\n"),
            doc("/repo/app/AGENTS.md", "App guide."),
        ];
        assert_eq!(
            instructions_section(&docs),
            format!(
                "{INSTRUCTIONS_PREAMBLE}\n\n\
                 Contents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\n\
                 Root guide.\n\n\
                 Contents of /repo/app/AGENTS.md (project instructions, checked into the codebase):\n\n\
                 App guide."
            )
        );
    }

    #[test]
    fn the_section_carries_no_instructions_markers() {
        // The `<INSTRUCTIONS>` markers and the `# AGENTS.md instructions`
        // heading are gone with the fragment they framed.
        let rendered = instructions_section(&[doc("/repo/AGENTS.md", "Use TDD.")]);
        assert!(!rendered.contains("<INSTRUCTIONS>"), "{rendered}");
        assert!(!rendered.contains("</INSTRUCTIONS>"), "{rendered}");
        assert!(!rendered.contains("# AGENTS.md instructions"), "{rendered}");
    }

    #[test]
    fn instructions_section_is_empty_without_docs() {
        assert_eq!(instructions_section(&[]), "");
    }

    #[test]
    fn the_preamble_is_the_references_wording() {
        assert_eq!(
            INSTRUCTIONS_PREAMBLE,
            "Codebase and user instructions are shown below. Be sure to adhere to these \
             instructions. IMPORTANT: These instructions OVERRIDE any default behavior and \
             you MUST follow them exactly as written."
        );
    }

    // --- the fs boundary (tempfile-hermetic) ---

    #[test]
    fn find_project_root_walks_up_to_the_git_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(find_project_root(&deep), Some(root.clone()));
        assert_eq!(find_project_root(&root), Some(root));
    }

    #[test]
    fn find_project_root_accepts_a_worktree_gitfile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".git"), "gitdir: /elsewhere").unwrap();
        assert_eq!(find_project_root(&root), Some(root));
    }

    #[test]
    fn find_project_root_is_none_without_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).unwrap();
        // The tempdir's own ancestors (e.g. /tmp) carry no .git either.
        assert_eq!(find_project_root(&dir), None);
    }

    #[test]
    fn load_user_instructions_collects_root_to_cwd_and_renders_the_section() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let deep = root.join("crates/app");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Root guide.\n").unwrap();
        // The middle dir (crates/) has no doc — skipped, not an error.
        std::fs::write(deep.join("AGENTS.md"), "App guide.\n").unwrap();
        let rendered = load_user_instructions(&deep).unwrap();
        assert_eq!(
            rendered,
            format!(
                "{INSTRUCTIONS_PREAMBLE}\n\n\
                 Contents of {} (project instructions, checked into the codebase):\n\n\
                 Root guide.\n\n\
                 Contents of {} (project instructions, checked into the codebase):\n\n\
                 App guide.",
                root.join("AGENTS.md").display(),
                deep.join("AGENTS.md").display()
            )
        );
    }

    #[test]
    fn load_user_instructions_without_a_marker_reads_the_cwd_doc_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("loose");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "Loose guide.").unwrap();
        let rendered = load_user_instructions(&dir).unwrap();
        assert!(rendered.contains("Loose guide."), "{rendered}");
    }

    #[test]
    fn load_user_instructions_is_none_without_any_doc() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(load_user_instructions(&root), None);
    }

    #[test]
    fn an_override_doc_wins_over_agents_md_in_its_directory() {
        // Codex's LOCAL_AGENTS_MD_FILENAME: per directory the first existing
        // candidate contributes — the git-ignorable AGENTS.override.md beats
        // the checked-in guide; a chain dir with only AGENTS.md still counts.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let deep = root.join("app");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Checked-in root guide.").unwrap();
        std::fs::write(root.join("AGENTS.override.md"), "Local root override.").unwrap();
        std::fs::write(deep.join("AGENTS.md"), "App guide.").unwrap();
        let rendered = load_user_instructions(&deep).unwrap();
        assert!(rendered.contains("Local root override."), "{rendered}");
        assert!(
            !rendered.contains("Checked-in root guide."),
            "the override replaces its directory's AGENTS.md: {rendered}"
        );
        assert!(rendered.contains("App guide."), "{rendered}");
        // …and its heading says what it is: a private file, not checked in.
        assert!(
            rendered.contains(&format!(
                "Contents of {} (user's private project instructions, not checked in):",
                root.join("AGENTS.override.md").display()
            )),
            "{rendered}"
        );
    }

    #[test]
    fn load_user_instructions_survives_invalid_utf8() {
        // Codex reads bytes and from_utf8_lossy's them; a stray invalid byte
        // must not silently drop the whole guide (the read_to_string trap).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), b"Use TDD \xFF always.").unwrap();
        let rendered = load_user_instructions(&dir).unwrap();
        assert!(rendered.contains("Use TDD"), "{rendered}");
        assert!(rendered.contains("always."), "{rendered}");
        assert!(
            rendered.contains('\u{FFFD}'),
            "the invalid byte decodes to the replacement char: {rendered}"
        );
    }

    #[test]
    fn read_capped_reads_at_most_the_budget() {
        // The budget must bound the *I/O*, not just the folded output: a
        // giant file that merely happens to be named AGENTS.md cannot be
        // slurped whole into memory while the raw-mode terminal waits for
        // its first frame — and this loader re-runs at every turn start, so
        // an unbounded read would be paid over and over (the
        // `main.rs::read_capped` / "hangs in ~" class of bug).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, vec![b'x'; 100 * 1024]).unwrap();
        assert_eq!(read_capped(&path, 64).expect("readable").len(), 64);
        assert!(read_capped(&path, 0).expect("readable").is_empty());
        assert_eq!(
            read_capped(&tmp.path().join("missing.md"), 64),
            None,
            "an unreadable doc is skipped, not an error"
        );
    }

    #[test]
    fn a_huge_doc_folds_down_to_the_budget() {
        // The observable end of the same guarantee: an oversized guide still
        // loads, bounded — never the whole file.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), vec![b'x'; 100 * 1024]).unwrap();
        let rendered = load_user_instructions_with(&dir, 64).expect("loads");
        let heading = doc_heading(&dir.join("AGENTS.md"));
        let body = rendered
            .split_once(&format!("{heading}\n\n"))
            .map(|(_, body)| body)
            .expect("the section heads the body");
        assert_eq!(body.len(), 64, "the body is the budget, not the file");
    }

    #[test]
    fn a_zero_budget_disables_loading_entirely() {
        // The knob's off switch (ALTER_ZERO_PROJECT_DOC_MAX_BYTES=0): no
        // discovery, no reads, no fragment — codex's max_total == 0 early out.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "A real guide.").unwrap();
        assert_eq!(load_user_instructions_with(&dir, 0), None);
        // A tiny budget still loads (truncated) — 0 alone is the switch.
        assert!(load_user_instructions_with(&dir, 6).is_some());
    }
}

//! AGENTS.md discovery — codex's project doc in the context window (see
//! `docs/project-doc.md`).
//!
//! `/init` generates an `AGENTS.md` contributor guide; this module reads it
//! (and any nested ones) back so every turn's context carries the project's
//! own instructions, exactly as codex does (`codex-rs/core/src/agents_md.rs`):
//! find the project root by walking up from the cwd to the nearest `.git`
//! marker, collect every `AGENTS.md` from the root *down to* the cwd, cap the
//! total at 32 KiB, and render the result as a **user-role** instructions
//! fragment (`# AGENTS.md instructions … <INSTRUCTIONS>…</INSTRUCTIONS>`)
//! that [`context::context_messages_with`] injects at the front of the
//! derived conversation.
//!
//! The chain/budget/fragment logic is pure; [`find_project_root`] and
//! [`load_user_instructions`] are the small fs boundary (stat + read),
//! tempfile-tested in-module like the `checkpoint` store's I/O.
//!
//! [`context::context_messages_with`]: crate::context::context_messages_with

use std::path::{Path, PathBuf};

/// Codex's `project_doc_max_bytes` default: the total byte budget across
/// every discovered `AGENTS.md`.
pub const PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;

/// The filename scanned for project instructions (codex's
/// `DEFAULT_AGENTS_MD_FILENAME`).
pub const PROJECT_DOC_FILENAME: &str = "AGENTS.md";

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

/// Concatenate discovered docs under codex's byte budget: each doc is
/// truncated to the remaining budget (on a `char` boundary — codex truncates
/// raw bytes and lossy-decodes; our input is already `String`), blank docs
/// are skipped and cost nothing, and collection stops once the budget is
/// spent. Docs join with a blank line. `None` when nothing survives.
#[must_use]
pub fn combine_docs(docs: &[String], max_bytes: usize) -> Option<String> {
    let mut remaining = max_bytes;
    let mut kept: Vec<&str> = Vec::new();
    for doc in docs {
        if remaining == 0 {
            break;
        }
        let clipped = truncate_to_bytes(doc, remaining);
        if clipped.trim().is_empty() {
            continue;
        }
        kept.push(clipped);
        remaining -= clipped.len();
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join("\n\n"))
    }
}

/// The widest prefix of `text` within `max_bytes`, split on a `char`
/// boundary (the whole text when it already fits).
fn truncate_to_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &text[..end]
}

/// Render the combined docs as codex's user-instructions fragment
/// (`core/src/context/user_instructions.rs` — start marker, body, end
/// marker):
///
/// ```text
/// # AGENTS.md instructions for {directory}
///
/// <INSTRUCTIONS>
/// {text}
/// </INSTRUCTIONS>
/// ```
///
/// `directory: None` drops the ` for …` clause.
#[must_use]
pub fn instructions_message(text: &str, directory: Option<&str>) -> String {
    let directory = directory
        .map(|dir| format!(" for {dir}"))
        .unwrap_or_default();
    format!("# AGENTS.md instructions{directory}\n\n<INSTRUCTIONS>\n{text}\n</INSTRUCTIONS>")
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
/// context fragment [`instructions_message`] — or `None` when no doc exists.
/// Boundary: composes [`find_project_root`], [`doc_chain`], one read per
/// chain directory, [`combine_docs`] under [`PROJECT_DOC_MAX_BYTES`]. A
/// missing or unreadable file is simply skipped (codex ignores NotFound).
#[must_use]
pub fn load_user_instructions(cwd: &Path) -> Option<String> {
    let root = find_project_root(cwd);
    let docs: Vec<String> = doc_chain(root.as_deref(), cwd)
        .iter()
        .filter_map(|dir| std::fs::read_to_string(dir.join(PROJECT_DOC_FILENAME)).ok())
        .collect();
    let combined = combine_docs(&docs, PROJECT_DOC_MAX_BYTES)?;
    Some(instructions_message(
        &combined,
        Some(&cwd.display().to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- combine_docs ---

    #[test]
    fn combine_docs_joins_in_order_with_a_blank_line() {
        let docs = vec!["root guide".to_string(), "inner guide".to_string()];
        assert_eq!(
            combine_docs(&docs, 1024).as_deref(),
            Some("root guide\n\ninner guide")
        );
    }

    #[test]
    fn combine_docs_skips_blank_docs_for_free() {
        let docs = vec!["  \n\t".to_string(), "real".to_string()];
        assert_eq!(combine_docs(&docs, 1024).as_deref(), Some("real"));
    }

    #[test]
    fn combine_docs_is_none_when_nothing_survives() {
        assert_eq!(combine_docs(&[], 1024), None);
        assert_eq!(combine_docs(&["   ".to_string()], 1024), None);
        // A zero budget disables the whole feature, codex-style.
        assert_eq!(combine_docs(&["doc".to_string()], 0), None);
    }

    #[test]
    fn combine_docs_truncates_the_overflowing_doc_and_stops() {
        let docs = vec![
            "12345678".to_string(),
            "abcdefgh".to_string(),
            "never".to_string(),
        ];
        // 8 bytes of doc one + 4 of doc two; doc three finds no budget left.
        assert_eq!(combine_docs(&docs, 12).as_deref(), Some("12345678\n\nabcd"));
    }

    #[test]
    fn combine_docs_truncates_on_a_char_boundary() {
        // 'é' is two bytes: a 3-byte budget keeps "aé", never splits it.
        let docs = vec!["aéé".to_string()];
        assert_eq!(combine_docs(&docs, 3).as_deref(), Some("aé"));
    }

    // --- instructions_message ---

    #[test]
    fn instructions_message_renders_codexs_fragment() {
        assert_eq!(
            instructions_message("Use TDD.", Some("/repo")),
            "# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>\nUse TDD.\n</INSTRUCTIONS>"
        );
    }

    #[test]
    fn instructions_message_drops_the_for_clause_without_a_directory() {
        assert_eq!(
            instructions_message("Use TDD.", None),
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nUse TDD.\n</INSTRUCTIONS>"
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
    fn load_user_instructions_collects_root_to_cwd_and_renders_the_fragment() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let deep = root.join("crates/app");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Root guide.").unwrap();
        // The middle dir (crates/) has no doc — skipped, not an error.
        std::fs::write(deep.join("AGENTS.md"), "App guide.").unwrap();
        let rendered = load_user_instructions(&deep).unwrap();
        assert_eq!(
            rendered,
            format!(
                "# AGENTS.md instructions for {}\n\n<INSTRUCTIONS>\nRoot guide.\n\nApp guide.\n</INSTRUCTIONS>",
                deep.display()
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
}

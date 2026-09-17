//! Read-only Git review vocabulary and pure patch/status parsing.

use std::path::PathBuf;

/// The comparison a file belongs to; partially staged files appear twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSection {
    Unstaged,
    Staged,
    Untracked,
}

impl DiffSection {
    /// The name displayed in the review's file list.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
            Self::Untracked => "Untracked",
        }
    }
}

/// The visual meaning of one patch row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Hunk,
    Header,
    Notice,
}

/// One patch row. Content rows omit their leading patch marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

/// One file in one comparison, including metadata-only or binary changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub section: DiffSection,
    /// Git's single-letter status: M, A, D, R, C, T, U, or ?; ! is a notice.
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
}

/// A bounded snapshot of changes across the entire containing worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSnapshot {
    pub root: PathBuf,
    pub branch: String,
    pub files: Vec<DiffFile>,
}

/// Parse a unified patch without performing I/O. Metadata is preserved.
#[must_use]
pub fn parse_patch(patch: &str) -> Vec<DiffLine> {
    let mut hunk = None;
    patch
        .split_terminator('\n')
        .map(|text| {
            let mut line = DiffLine {
                kind: DiffLineKind::Header,
                old_line: None,
                new_line: None,
                text: text.to_owned(),
            };
            if text.starts_with("@@ ") {
                line.kind = DiffLineKind::Hunk;
                hunk = parse_hunk(text);
            } else if text.starts_with("\\ No newline")
                || text.starts_with("Binary files ")
                || text == "GIT binary patch"
                || text.starts_with("* Unmerged path ")
            {
                line.kind = DiffLineKind::Notice;
            } else if let Some((old, new, old_left, new_left)) = hunk.as_mut() {
                let marker = text.as_bytes().first().copied();
                let take_old = matches!(marker, Some(b' ' | b'-')) && *old_left > 0;
                let take_new = matches!(marker, Some(b' ' | b'+')) && *new_left > 0;
                if take_old || take_new {
                    line.kind = match marker {
                        Some(b'+') => DiffLineKind::Addition,
                        Some(b'-') => DiffLineKind::Deletion,
                        _ => DiffLineKind::Context,
                    };
                    line.text = text[1..].to_owned();
                    if take_old {
                        line.old_line = Some(*old);
                        *old = old.saturating_add(1);
                        *old_left -= 1;
                    }
                    if take_new {
                        line.new_line = Some(*new);
                        *new = new.saturating_add(1);
                        *new_left -= 1;
                    }
                } else {
                    hunk = None;
                }
            }
            line
        })
        .collect()
}

fn parse_hunk(text: &str) -> Option<(usize, usize, usize, usize)> {
    fn range(text: &str, prefix: char) -> Option<(usize, usize)> {
        let text = text.strip_prefix(prefix)?;
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    }
    let mut words = text.split_whitespace();
    if words.next()? != "@@" {
        return None;
    }
    let (old, old_count) = range(words.next()?, '-')?;
    let (new, new_count) = range(words.next()?, '+')?;
    (words.next()? == "@@").then_some((old, new, old_count, new_count))
}

/// Parse complete NUL-delimited porcelain-v1 records into file entries.
/// A trailing incomplete record is ignored, allowing bounded captures.
#[must_use]
pub fn parse_status(status: &[u8]) -> Vec<DiffFile> {
    let mut entries = status
        .split_inclusive(|byte| *byte == 0)
        .filter_map(|entry| entry.strip_suffix(&[0]));
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }
        let old_path = if matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C') {
            let Some(old) = entries.next().filter(|old| !old.is_empty()) else {
                break;
            };
            Some(path_from_bytes(old))
        } else {
            None
        };
        let path = path_from_bytes(&entry[3..]);
        let mut push = |section, status: u8| {
            files.push(DiffFile {
                path: path.clone(),
                old_path: old_path.clone(),
                section,
                status: char::from(status).to_string(),
                additions: 0,
                deletions: 0,
                lines: Vec::new(),
                truncated: false,
            });
        };
        if entry[..2] == *b"??" {
            push(DiffSection::Untracked, b'?');
        } else if entry[..2].contains(&b'U') || matches!(&entry[..2], b"AA" | b"DD") {
            push(DiffSection::Unstaged, b'U');
        } else {
            if matches!(entry[0], b'M' | b'A' | b'D' | b'R' | b'C' | b'T') {
                push(DiffSection::Staged, entry[0]);
            }
            if matches!(entry[1], b'M' | b'A' | b'D' | b'R' | b'C' | b'T') {
                push(DiffSection::Unstaged, entry[1]);
            }
        }
    }
    files
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes).into_owned().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_numbers_each_side_and_preserves_metadata() {
        let lines = parse_patch(
            "diff --git a/a b/a\nold mode 100644\nnew mode 100755\n--- a/a\n+++ b/a\n@@ -4,3 +4,3 @@ context\n keep\n-old\n+new\n last\n\\ No newline at end of file\n",
        );
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[1].kind, DiffLineKind::Header);
        assert_eq!(lines[5].kind, DiffLineKind::Hunk);
        assert_eq!(
            lines[6],
            DiffLine {
                kind: DiffLineKind::Context,
                old_line: Some(4),
                new_line: Some(4),
                text: "keep".into()
            }
        );
        assert_eq!(
            lines[7],
            DiffLine {
                kind: DiffLineKind::Deletion,
                old_line: Some(5),
                new_line: None,
                text: "old".into()
            }
        );
        assert_eq!(
            lines[8],
            DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some(5),
                text: "new".into()
            }
        );
        assert_eq!(lines[9].old_line, Some(6));
        assert_eq!(lines[9].new_line, Some(6));
        assert_eq!(lines[10].kind, DiffLineKind::Notice);
    }

    #[test]
    fn patch_hunk_ranges_reset_and_content_can_look_like_headers() {
        let lines =
            parse_patch("@@ -0,0 +1,2 @@\n+++content\n+\n@@ -8 +10 @@\n---content\n+replacement\n");
        assert_eq!(lines[1].kind, DiffLineKind::Addition);
        assert_eq!(lines[1].text, "++content");
        assert_eq!(lines[2].new_line, Some(2));
        assert_eq!(lines[4].kind, DiffLineKind::Deletion);
        assert_eq!(lines[4].old_line, Some(8));
        assert_eq!(lines[5].new_line, Some(10));
    }

    #[test]
    fn binary_and_bad_hunks_do_not_invent_line_numbers() {
        let lines = parse_patch("Binary files a/a and b/a differ\n@@ -oops +1 @@\n+unparsed\n");
        assert_eq!(lines[0].kind, DiffLineKind::Notice);
        assert!(
            lines
                .iter()
                .all(|line| line.old_line.is_none() && line.new_line.is_none())
        );
    }

    #[test]
    fn status_keeps_staged_unstaged_and_unusual_paths_separate() {
        let files = parse_status(
            b"MM space\nname\0?? :(glob)*.rs\0R  new\0old\0 U conflict\0UU both\0!! ignored\0",
        );
        assert_eq!(files.len(), 6);
        assert_eq!(files[0].path, PathBuf::from("space\nname"));
        assert_eq!(files[0].section, DiffSection::Staged);
        assert_eq!(files[1].section, DiffSection::Unstaged);
        assert_eq!(files[2].section, DiffSection::Untracked);
        assert_eq!(files[2].path, PathBuf::from(":(glob)*.rs"));
        assert_eq!(files[3].old_path, Some(PathBuf::from("old")));
        assert_eq!(files[3].status, "R");
        assert_eq!(files[4].status, "U");
        assert_eq!(files[5].status, "U");
    }

    #[test]
    fn status_ignores_incomplete_paths_and_rename_pairs() {
        assert!(parse_status(b" M missing terminator").is_empty());
        assert!(parse_status(b"R  new\0old").is_empty());
        assert!(parse_status(b"?? \0").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn status_preserves_non_utf8_filenames() {
        use std::os::unix::ffi::OsStrExt;
        let files = parse_status(b" M \xff.rs\0");
        assert_eq!(files[0].path.as_os_str().as_bytes(), b"\xff.rs");
    }
}

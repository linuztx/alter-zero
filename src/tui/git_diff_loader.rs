//! Read-only Git/file I/O for the full-screen diff review's worker.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use alter_zero::git_diff::{
    DiffFile, DiffLine, DiffLineKind, DiffSection, DiffSnapshot, parse_patch, parse_status,
};

const FILE_BYTES: usize = 256 * 1024;
const FILE_LINES: usize = 4_000;
const SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const SNAPSHOT_LINES: usize = 24_000;
const STATUS_BYTES: usize = 256 * 1024;
const MAX_FILES: usize = 4_096;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(15);

/// Check the containing worktree before the review takes over the terminal.
/// Failures stay concise because they are shown as ordinary info toasts.
pub(super) fn repository_root(cwd: &Path) -> Result<PathBuf, String> {
    let mut root_command = git(cwd);
    root_command.args(["rev-parse", "--show-toplevel"]);
    let root_output = capture(root_command, 16 * 1024).map_err(|error| {
        if error.contains("not a git repository") || error.contains("must be run in a work tree") {
            "Not a Git repository."
        } else if error.starts_with("Cannot run Git:") {
            "Git is unavailable."
        } else if error.contains("timed out") {
            "Git check timed out."
        } else {
            "Cannot inspect Git repository."
        }
        .to_string()
    })?;
    if root_output.truncated {
        return Err("Git's working-tree path is too long to display.".into());
    }
    let root_bytes = root_output
        .bytes
        .strip_suffix(b"\n")
        .unwrap_or(&root_output.bytes);
    Ok(path_from_bytes(root_bytes))
}

/// Called off the event-loop thread. Git is never allowed to refresh the
/// index, launch external diff/textconv/fsmonitor programs, or invoke a pager.
pub(super) fn load(cwd: &Path) -> Result<DiffSnapshot, String> {
    let started = Instant::now();
    let root = repository_root(cwd)?;
    let branch = branch_name(&root);
    let filters = disabled_filters(&root)?;
    let mut status_command = git(&root);
    status_command.args(&filters);
    status_command.args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
        "--renames",
    ]);
    let status = capture(status_command, STATUS_BYTES)?;
    let mut files = parse_status(&status.bytes);
    let omitted_files = files.len().saturating_sub(MAX_FILES);
    files.truncate(MAX_FILES);
    files.sort_by(|a, b| {
        section_order(a.section)
            .cmp(&section_order(b.section))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut bytes_left = SNAPSHOT_BYTES;
    let mut lines_left = SNAPSHOT_LINES;
    for file in &mut files {
        if bytes_left == 0 || lines_left == 0 || started.elapsed() >= SNAPSHOT_TIMEOUT {
            file.truncated = true;
            file.lines.push(notice(
                "Preview omitted: the review's size or time limit was reached. File remains changed.",
            ));
            continue;
        }
        let byte_limit = FILE_BYTES.min(bytes_left);
        let result = if file.section == DiffSection::Untracked {
            read_untracked(&root, file, byte_limit)
        } else {
            read_patch(&root, file, byte_limit, &filters)
        };
        match result {
            Ok(bytes) => bytes_left = bytes_left.saturating_sub(bytes),
            Err(error) => file.lines.push(notice(error)),
        }
        let line_limit = FILE_LINES.min(lines_left);
        if file.lines.len() > line_limit {
            file.lines.truncate(line_limit);
            file.truncated = true;
        }
        lines_left = lines_left.saturating_sub(file.lines.len());
        file.additions = file
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Addition)
            .count();
        file.deletions = file
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Deletion)
            .count();
        if file.truncated {
            file.lines.push(notice(
                "Preview truncated at the review limit; line totals cover the displayed changes only.",
            ));
        }
        if file.lines.is_empty() {
            file.lines.push(notice(
                "Metadata-only change, empty file, or file changed since the scan.",
            ));
        }
    }
    if status.truncated || omitted_files > 0 {
        files.push(DiffFile {
            path: PathBuf::from("[additional changes]"),
            old_path: None,
            section: DiffSection::Untracked,
            status: "!".into(),
            additions: 0,
            deletions: 0,
            lines: vec![notice(format!(
                "File listing truncated at {MAX_FILES} entries / 256 KiB of Git status. Additional changes exist; inspect them with git status."
            ))],
            truncated: true,
        });
    }
    Ok(DiffSnapshot {
        root,
        branch,
        files,
    })
}

fn section_order(section: DiffSection) -> u8 {
    match section {
        DiffSection::Unstaged => 0,
        DiffSection::Staged => 1,
        DiffSection::Untracked => 2,
    }
}

fn branch_name(root: &Path) -> String {
    let mut command = git(root);
    command.args(["symbolic-ref", "--quiet", "--short", "HEAD"]);
    if let Ok(output) = capture(command, 4096) {
        return String::from_utf8_lossy(&output.bytes).trim_end().to_owned();
    }
    let mut command = git(root);
    command.args(["rev-parse", "--short", "HEAD"]);
    match capture(command, 4096) {
        Ok(output) => format!(
            "detached · {}",
            String::from_utf8_lossy(&output.bytes).trim()
        ),
        Err(_) => "unborn branch".into(),
    }
}

fn read_patch(
    root: &Path,
    file: &mut DiffFile,
    limit: usize,
    filters: &[std::ffi::OsString],
) -> Result<usize, String> {
    let mut command = git(root);
    command.args(filters);
    command.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--find-renames=50%",
        "--unified=3",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--output-indicator-new=+",
        "--output-indicator-old=-",
        "--output-indicator-context= ",
        "--submodule=short",
        "--ignore-submodules=none",
    ]);
    if file.section == DiffSection::Staged {
        command.arg("--cached");
    } else if file.status == "U" {
        command.arg("--ours");
        file.lines.push(notice(
            "Unresolved merge conflict · comparing the working tree with our side.",
        ));
    }
    command.arg("--").arg(&file.path);
    if let Some(old_path) = &file.old_path {
        command.arg(old_path);
    }
    let mut output = capture(command, limit)?;
    file.truncated = output.truncated;
    // Bound row allocations as well as subprocess bytes: a patch consisting
    // mostly of blank lines must not allocate hundreds of thousands of rows.
    let max_lines = FILE_LINES.saturating_sub(file.lines.len());
    if let Some((end, _)) = output
        .bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(max_lines.saturating_sub(1))
        && end + 1 < output.bytes.len()
    {
        output.bytes.truncate(end + 1);
        file.truncated = true;
    }
    file.lines
        .extend(parse_patch(&String::from_utf8_lossy(&output.bytes)));
    Ok(output.bytes.len())
}

fn read_untracked(root: &Path, file: &mut DiffFile, limit: usize) -> Result<usize, String> {
    // Git paths are root-relative. Reject escaping paths defensively before
    // touching the filesystem, even though Git itself never emits them.
    if file.path.is_absolute()
        || file
            .path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("Cannot preview a path outside the Git working tree.".into());
    }
    let path = root.join(&file.path);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| format!("Cannot read file: {error}"))?;
    if metadata.file_type().is_symlink() {
        let target =
            std::fs::read_link(&path).map_err(|error| format!("Cannot read symlink: {error}"))?;
        let text = target.to_string_lossy().into_owned();
        let bytes = text.len();
        file.lines
            .push(notice("Symbolic link · target shown without following it."));
        file.lines.push(DiffLine {
            kind: DiffLineKind::Addition,
            old_line: None,
            new_line: Some(1),
            text,
        });
        return Ok(bytes);
    }
    if !metadata.is_file() {
        file.lines.push(notice(if metadata.is_dir() {
            "Untracked directory or nested Git repository · content preview unavailable."
        } else {
            "Special file · content preview unavailable."
        }));
        return Ok(0);
    }
    let reader = open_regular_file(&path)?;
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read file: {error}"))?;
    file.truncated = bytes.len() > limit;
    bytes.truncate(limit);
    // A byte cap may land inside the final UTF-8 codepoint; that does not
    // make an otherwise textual file binary.
    let text_bytes = match std::str::from_utf8(&bytes) {
        Err(error) if file.truncated && error.error_len().is_none() => {
            &bytes[..error.valid_up_to()]
        }
        _ => &bytes,
    };
    if text_bytes.contains(&0) || std::str::from_utf8(text_bytes).is_err() {
        file.lines.push(notice(format!(
            "Binary file · {} bytes (content preview unavailable).",
            metadata.len()
        )));
    } else if bytes.is_empty() {
        file.lines.push(notice("New empty file."));
    } else {
        // UTF-8 was validated above. Lossy conversion is only a non-panicking
        // bridge to the pure parser, and never changes these bytes.
        let text = String::from_utf8_lossy(text_bytes);
        let mut lines = text.split_terminator('\n').enumerate();
        file.lines.extend(
            lines
                .by_ref()
                .take(FILE_LINES)
                .map(|(index, text)| DiffLine {
                    kind: DiffLineKind::Addition,
                    old_line: None,
                    new_line: Some(index + 1),
                    text: text.to_owned(),
                }),
        );
        file.truncated |= lines.next().is_some();
        if !file.truncated && !bytes.ends_with(b"\n") {
            file.lines.push(notice("No newline at end of file."));
        }
    }
    Ok(bytes.len())
}

fn open_regular_file(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| format!("Cannot open file: {error}"))?;
        File::from(fd)
    };
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|error| format!("Cannot open file: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| format!("Cannot inspect file: {error}"))?
        .is_file()
    {
        return Err("File type changed during scan; refresh to inspect it.".into());
    }
    Ok(file)
}

fn notice(text: impl Into<String>) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Notice,
        old_line: None,
        new_line: None,
        text: text.into(),
    }
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

fn git(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "diff.renames=true",
            "-c",
            "diff.suppressBlankEmpty=false",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The application may have been launched by Git; these inherited values
    // must not redirect /diff to an unrelated worktree or alternate index.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_EXTERNAL_DIFF",
    ] {
        command.env_remove(variable);
    }
    command
}

/// Diff/status can invoke a configured clean filter even with --no-textconv.
/// Override only those program-bearing keys, retaining ordinary Git semantics
/// for line endings, renames, attributes and binary detection.
fn disabled_filters(root: &Path) -> Result<Vec<std::ffi::OsString>, String> {
    let mut command = git(root);
    command.args(["config", "--null", "--name-only", "--list"]);
    let output = capture(command, 64 * 1024)?;
    if output.truncated {
        return Err("Git configuration is too large to inspect safely.".into());
    }
    let mut arguments = Vec::new();
    for key in output.bytes.split(|byte| *byte == 0) {
        if !key.starts_with(b"filter.") {
            continue;
        }
        let Some(suffix) = key.rsplit(|byte| *byte == b'.').next() else {
            continue;
        };
        if matches!(suffix, b"clean" | b"smudge" | b"process" | b"required") {
            let mut setting = path_from_bytes(key).into_os_string();
            setting.push(if suffix == b"required" { "=false" } else { "=" });
            arguments.push("-c".into());
            arguments.push(setting);
        }
    }
    Ok(arguments)
}

struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Drain both pipes concurrently, retaining only the requested prefix. A
/// producer that exceeds the cap is killed, not allowed to keep filling RAM.
fn capture(mut command: Command, limit: usize) -> Result<Capture, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot run Git: {error}"))?;
    let stdout = child.stdout.take().ok_or("Git stdout is unavailable.")?;
    let stderr = child.stderr.take().ok_or("Git stderr is unavailable.")?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let output_limit = Arc::clone(&exceeded);
    let output_thread = thread::spawn(move || read_bounded(stdout, limit, &output_limit));
    let error_limit = Arc::clone(&exceeded);
    let error_thread = thread::spawn(move || read_bounded(stderr, 16 * 1024, &error_limit));
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if exceeded.load(Ordering::Relaxed) || started.elapsed() >= COMMAND_TIMEOUT {
            timed_out = started.elapsed() >= COMMAND_TIMEOUT;
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    let output = output_thread
        .join()
        .map_err(|_| "Git output reader stopped unexpectedly.")?
        .map_err(|error| format!("Cannot read Git output: {error}"))?;
    let errors = error_thread
        .join()
        .map_err(|_| "Git error reader stopped unexpectedly.")?
        .map_err(|error| format!("Cannot read Git errors: {error}"))?;
    if timed_out {
        return Err("Git inspection timed out; refresh to try again.".into());
    }
    let status = status.map_err(|error| format!("Cannot wait for Git: {error}"))?;
    if !status.success() && !output.truncated {
        let error = String::from_utf8_lossy(&errors.bytes);
        let error = error.trim();
        return Err(if error.is_empty() {
            "Git could not inspect these changes.".into()
        } else {
            error.chars().take(500).collect()
        });
    }
    Ok(output)
}

fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    exceeded: &AtomicBool,
) -> std::io::Result<Capture> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        if keep < count {
            truncated = true;
            exceeded.store(true, Ordering::Relaxed);
        }
    }
    Ok(Capture { bytes, truncated })
}

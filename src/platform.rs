//! Host platform paths and shell discovery (see `docs/windows.md`).

use std::ffi::OsString;
use std::path::PathBuf;

/// The user's home, including native Windows sessions with no HOME variable.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    home_from(cfg!(windows), |name| std::env::var_os(name))
}

fn home_from(windows: bool, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let value = |name: &str| var(name).filter(|value| !value.is_empty());
    if windows {
        if let Some(profile) = value("USERPROFILE") {
            return Some(profile.into());
        }
        if let (Some(mut drive), Some(path)) = (value("HOMEDRIVE"), value("HOMEPATH")) {
            drive.push(path);
            return Some(drive.into());
        }
    }
    value("HOME").map(PathBuf::from)
}

/// Git for Windows Bash. Never automatically select the legacy WSL launcher.
#[cfg(windows)]
#[must_use]
pub fn windows_shell() -> PathBuf {
    if let Some(shell) = std::env::var_os("ALTER_ZERO_BASH").filter(|s| !s.is_empty()) {
        let shell = PathBuf::from(shell);
        if shell.is_absolute() {
            return shell;
        }
        // An invalid override must not execute a project-local program.
        return PathBuf::from(r"C:\__alter_zero_invalid_bash_override__\bash.exe");
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from);
    if let Some(shell) =
        crate::subprocess::find_on_path("bash.exe", std::env::var_os("PATH").as_deref(), |path| {
            let parent = path.parent();
            let wsl = system.as_ref().is_some_and(|root| {
                ["System32", "Sysnative"].iter().any(|dir| {
                    parent.is_some_and(|parent| {
                        parent
                            .as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&root.join(dir).to_string_lossy())
                    })
                })
            });
            !wsl && path.is_file()
        })
    {
        return shell;
    }
    let mut candidates = Vec::new();
    for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key).filter(|s| !s.is_empty()) {
            candidates.push(PathBuf::from(root).join("Git/bin/bash.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(root).join("Programs/Git/bin/bash.exe"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"))
}

// Git Bash started from PowerShell inherits a Windows PATH, which need not
// contain its Unix utilities. Keep non-login shells while adding the chosen
// installation's own tool directories ahead of the inherited PATH.
#[cfg(any(windows, test))]
fn shell_dirs(
    shell: &std::path::Path,
    installed: impl Fn(&std::path::Path) -> bool,
) -> Vec<PathBuf> {
    let Some(bin) = shell.parent() else {
        return Vec::new();
    };
    let root = bin.parent().and_then(|parent| {
        if parent
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("usr"))
        {
            parent.parent()
        } else {
            Some(parent)
        }
    });
    let mut dirs = Vec::new();
    if let Some(root) = root {
        for relative in ["mingw64/bin", "clangarm64/bin", "usr/bin"] {
            let dir = root.join(relative);
            if installed(&dir) {
                dirs.push(dir);
            }
        }
    }
    if installed(bin) && !dirs.iter().any(|dir| dir == bin) {
        dirs.push(bin.to_path_buf());
    }
    dirs
}

#[cfg(windows)]
pub(crate) fn windows_shell_path(shell: &std::path::Path) -> Option<OsString> {
    let mut dirs = shell_dirs(shell, |path| path.is_dir());
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(dirs).ok()
}

/// Terminate a Windows command and its descendants without a shell.
#[cfg(windows)]
pub(crate) fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn windows_home_prefers_userprofile_and_ignores_empty_values() {
        let vars = |name: &str| match name {
            "HOME" => Some(OsString::from("/git-home")),
            "USERPROFILE" => Some(OsString::from("C:/Users/Ada Lovelace")),
            _ => None,
        };
        assert_eq!(home_from(true, vars), Some("C:/Users/Ada Lovelace".into()));
        assert_eq!(home_from(false, vars), Some("/git-home".into()));
        assert_eq!(home_from(true, |_| Some(OsString::new())), None);
    }

    #[test]
    fn windows_home_can_use_drive_and_path_or_home() {
        assert_eq!(
            home_from(true, |name| match name {
                "HOMEDRIVE" => Some("C:".into()),
                "HOMEPATH" => Some("/Users/Ada".into()),
                _ => None,
            }),
            Some("C:/Users/Ada".into())
        );
        assert_eq!(
            home_from(true, |name| (name == "HOME").then(|| "/home/ada".into())),
            Some("/home/ada".into())
        );
    }
    #[test]
    fn git_bash_utilities_are_available_without_a_login_profile() {
        let root = std::path::Path::new("/opt/git");
        let expected = [
            root.join("mingw64/bin"),
            root.join("usr/bin"),
            root.join("bin"),
        ];
        for shell in [root.join("bin/bash.exe"), root.join("usr/bin/bash.exe")] {
            let dirs = shell_dirs(&shell, |path| expected.contains(&path.to_path_buf()));
            assert!(dirs.contains(&root.join("mingw64/bin")));
            assert!(dirs.contains(&root.join("usr/bin")));
            assert!(!dirs.contains(&root.join("usr/usr/bin")));
        }
    }

    #[test]
    fn a_custom_bash_keeps_its_own_directory_without_inventing_utilities() {
        let parent = std::path::Path::new("/opt/custom");
        assert_eq!(
            shell_dirs(&parent.join("bash.exe"), |path| path == parent),
            vec![parent.to_path_buf()]
        );
    }
}

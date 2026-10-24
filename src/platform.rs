//! Shared native-platform path policy for the I/O boundary.
//!
//! Native Windows uses `USERPROFILE`, not a Git Bash `HOME`. Unix keeps its
//! existing `HOME` policy. The injected resolvers keep both branches testable
//! without changing the process environment.

use std::ffi::OsString;
use std::path::PathBuf;

/// Resolve the native user profile from an injected environment lookup.
/// Empty values count as unset; the other platform's variable is not a
/// fallback, since it can name an unrelated MSYS or inherited directory.
#[must_use]
pub fn profile_dir_with(windows: bool, env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    nonempty_path(env(if windows { "USERPROFILE" } else { "HOME" }))
}

/// The current process's native user profile.
#[must_use]
pub fn profile_dir() -> Option<PathBuf> {
    profile_dir_with(cfg!(windows), &|key| std::env::var_os(key))
}

/// Resolve the app's per-user state directory. A nonempty
/// `ALTER_ZERO_CONFIG_DIR` wins unchanged, including a relative override;
/// otherwise state lives under the native profile's `.alter-zero`.
#[must_use]
pub fn config_home_with(windows: bool, env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    nonempty_path(env("ALTER_ZERO_CONFIG_DIR"))
        .or_else(|| profile_dir_with(windows, env).map(|home| home.join(".alter-zero")))
}

/// The current process's app state directory, honoring `ALTER_ZERO_CONFIG_DIR`.
#[must_use]
pub fn config_home() -> Option<PathBuf> {
    config_home_with(cfg!(windows), &|key| std::env::var_os(key))
}

fn nonempty_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values: Vec<_> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), OsString::from(value)))
            .collect();
        move |key| {
            values
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn windows_uses_the_native_profile_even_when_git_bash_sets_home() {
        let vars = env(&[("HOME", "/c/msys/home/me"), ("USERPROFILE", r"C:\Users\Me")]);
        assert_eq!(
            profile_dir_with(true, &vars),
            Some(PathBuf::from(r"C:\Users\Me"))
        );
    }

    #[test]
    fn unix_keeps_home_and_does_not_use_a_windows_profile() {
        let vars = env(&[("HOME", "/home/me"), ("USERPROFILE", r"C:\Users\Me")]);
        assert_eq!(
            profile_dir_with(false, &vars),
            Some(PathBuf::from("/home/me"))
        );
        assert_eq!(
            profile_dir_with(false, &env(&[("USERPROFILE", r"C:\Users\Me")])),
            None
        );
    }

    #[test]
    fn missing_or_empty_native_profiles_do_not_become_relative_paths() {
        for windows in [false, true] {
            assert_eq!(profile_dir_with(windows, &env(&[])), None);
            assert_eq!(
                profile_dir_with(windows, &env(&[("HOME", ""), ("USERPROFILE", "")])),
                None
            );
        }
        assert_eq!(
            profile_dir_with(true, &env(&[("HOME", "/not/the/profile")])),
            None
        );
    }

    #[test]
    fn configuration_override_wins_without_changing_the_profile() {
        for windows in [false, true] {
            let vars = env(&[
                ("HOME", "/home/me"),
                ("USERPROFILE", r"C:\Users\Me"),
                ("ALTER_ZERO_CONFIG_DIR", "portable state"),
            ]);
            assert_eq!(
                config_home_with(windows, &vars),
                Some(PathBuf::from("portable state"))
            );
            assert_eq!(
                config_home_with(windows, &env(&[("ALTER_ZERO_CONFIG_DIR", "portable")])),
                Some(PathBuf::from("portable"))
            );
            assert_ne!(
                profile_dir_with(windows, &vars),
                config_home_with(windows, &vars)
            );
        }
    }

    #[test]
    fn empty_configuration_override_falls_back_to_the_native_profile() {
        for (windows, key, home) in [
            (false, "HOME", "/home/me"),
            (true, "USERPROFILE", r"C:\Users\Me"),
        ] {
            assert_eq!(
                config_home_with(windows, &env(&[(key, home), ("ALTER_ZERO_CONFIG_DIR", "")])),
                Some(PathBuf::from(home).join(".alter-zero"))
            );
        }
        assert_eq!(
            config_home_with(true, &env(&[("ALTER_ZERO_CONFIG_DIR", "")])),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_profile_keeps_non_utf8_native_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let home = OsString::from_vec(b"/home/me-\xff".to_vec());
        assert_eq!(
            profile_dir_with(false, &|key| (key == "HOME").then(|| home.clone())),
            Some(PathBuf::from(home))
        );
    }
}

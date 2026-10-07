//! Keep the app's own writes out of read-only locations (F30-AC05).
//!
//! Profiles, performance logs and screenshots are user data. The original
//! installation is read-only by contract, and the app's own directory (the
//! binary, its bundled assets) is replaced on every upgrade and may not be
//! writable at all. The default profile root already lives under the OS
//! user-data directory; this guard covers the *explicit* destinations
//! (`--profile-dir`, `--perf-log`, `--screenshot`), which can name any
//! path, including one inside the install being played.
//!
//! Containment is decided on resolved paths: symlinks in the existing part
//! of a path are followed and `..` is folded away, so neither a link into
//! the install nor `install/../install/x` slips past a textual prefix test.
//! A destination that does not exist yet is judged by its deepest existing
//! ancestor plus the remaining names.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// A directory the app must never write into, with the reason shown to the
/// user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protected {
    pub label: &'static str,
    pub dir: PathBuf,
}

/// A destination was refused: it sits inside a protected directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub what: &'static str,
    pub path: PathBuf,
    pub inside: Protected,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing to write {} to {}: it is inside the {} ({}); choose a user-writable location",
            self.what,
            self.path.display(),
            self.inside.label,
            self.inside.dir.display()
        )
    }
}

impl std::error::Error for Refused {}

/// `path` as an absolute path with `.`/`..` folded and every existing
/// component's symlinks resolved. Never touches the disk beyond
/// canonicalizing what exists.
pub fn resolve(path: &Path, cwd: Option<&Path>) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match cwd {
            Some(cwd) => cwd.join(path),
            None => path.to_path_buf(),
        }
    };
    let mut folded = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str()),
        }
    }
    // Canonicalize the deepest ancestor that exists, then re-append the
    // names that do not.
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = folded.as_path();
    loop {
        if let Ok(real) = probe.canonicalize() {
            let mut out = real;
            out.extend(tail.iter().rev());
            return out;
        }
        match (probe.parent(), probe.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                probe = parent;
            }
            _ => return folded,
        }
    }
}

/// Refuse `path` when it is, or lies under, any of `protected`.
pub fn check(
    what: &'static str,
    path: &Path,
    protected: &[Protected],
    cwd: Option<&Path>,
) -> Result<(), Refused> {
    let target = resolve(path, cwd);
    for guard in protected {
        if target.starts_with(resolve(&guard.dir, cwd)) {
            return Err(Refused {
                what,
                path: path.to_path_buf(),
                inside: guard.clone(),
            });
        }
    }
    Ok(())
}

/// The directories this process must not write into: the original
/// installation (when one is mounted), the app's bundled assets and the
/// directory holding the executable.
pub fn protected_dirs(
    install: Option<&Path>,
    app_assets: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Vec<Protected> {
    let mut out = Vec::new();
    if let Some(dir) = install {
        out.push(Protected {
            label: "original installation",
            dir: dir.to_path_buf(),
        });
    }
    if let Some(dir) = app_assets {
        out.push(Protected {
            label: "app's bundled assets",
            dir: dir.to_path_buf(),
        });
    }
    if let Some(dir) = exe_dir {
        out.push(Protected {
            label: "application directory",
            dir: dir.to_path_buf(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn guards(install: &Path) -> Vec<Protected> {
        protected_dirs(Some(install), None, None)
    }

    #[test]
    fn a_path_inside_the_install_is_refused_even_when_it_does_not_exist() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        fs::create_dir(&install).unwrap();
        let err = check(
            "the profile store",
            &install.join("profiles/new"),
            &guards(&install),
            None,
        )
        .unwrap_err();
        assert_eq!(err.inside.label, "original installation");
        assert!(err.to_string().contains("original installation"));
    }

    #[test]
    fn the_install_directory_itself_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        fs::create_dir(&install).unwrap();
        assert!(check("a log", &install, &guards(&install), None).is_err());
    }

    #[test]
    fn a_sibling_with_a_shared_name_prefix_is_allowed() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        let sibling = root.path().join("MM2-saves");
        fs::create_dir(&install).unwrap();
        fs::create_dir(&sibling).unwrap();
        assert!(check("the profile store", &sibling, &guards(&install), None).is_ok());
    }

    #[test]
    fn dot_dot_cannot_climb_back_into_the_install() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        fs::create_dir(&install).unwrap();
        let sneaky = root.path().join("elsewhere/../MM2/saves");
        assert!(check("the profile store", &sneaky, &guards(&install), None).is_err());
    }

    #[test]
    fn a_relative_path_is_judged_against_the_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        fs::create_dir(&install).unwrap();
        let guards = guards(&install);
        assert!(check("a log", Path::new("perf.csv"), &guards, Some(&install)).is_err());
        assert!(check("a log", Path::new("perf.csv"), &guards, Some(root.path())).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_the_install_does_not_launder_the_path() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("MM2");
        fs::create_dir(&install).unwrap();
        let link = root.path().join("saves");
        std::os::unix::fs::symlink(&install, &link).unwrap();
        assert!(
            check(
                "the profile store",
                &link.join("p"),
                &guards(&install),
                None
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_install_is_protected_under_either_name() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        // The install is named through the alias; the write goes to the real dir.
        assert!(check("a log", &real.join("x.csv"), &guards(&alias), None).is_err());
    }

    #[test]
    fn the_app_directory_and_bundled_assets_are_protected_too() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("bin");
        let assets = root.path().join("bundle/assets");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let all = protected_dirs(None, Some(&assets), Some(&exe_dir));
        assert_eq!(
            check("a screenshot", &exe_dir.join("a.png"), &all, None)
                .unwrap_err()
                .inside
                .label,
            "application directory"
        );
        assert_eq!(
            check("a screenshot", &assets.join("a.png"), &all, None)
                .unwrap_err()
                .inside
                .label,
            "app's bundled assets"
        );
        assert!(check("a screenshot", &root.path().join("a.png"), &all, None).is_ok());
    }

    #[test]
    fn a_non_ascii_install_path_is_protected() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("Midtown Madness 2 — Spel åäö");
        fs::create_dir(&install).unwrap();
        assert!(check("a log", &install.join("ö/log.csv"), &guards(&install), None).is_err());
        assert!(
            check(
                "a log",
                &root.path().join("ö/log.csv"),
                &guards(&install),
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn nothing_is_protected_without_an_install() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            check(
                "a log",
                &root.path().join("x"),
                &protected_dirs(None, None, None),
                None
            )
            .is_ok()
        );
    }
}

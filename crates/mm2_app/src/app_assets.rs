//! Where the app finds its own synthetic assets (F30-AC04).
//!
//! The shipped base assets (`assets/texture/dev_road.png`, …) are mounted
//! into the VFS above the install and below mods. They used to be found
//! through the *current directory*, so launching the binary from anywhere
//! but the repository root silently dropped them — the dev world lost its
//! ground texture. The binary's own location is the stable anchor, so the
//! lookup starts there and falls back to the working directory last.
//!
//! No path is baked into the binary at compile time (a packaged artifact
//! must not carry the build machine's directories); a dev build under
//! `target/<profile>/` finds the repository's `assets/` by walking up
//! from the executable.

use std::path::{Path, PathBuf};

/// A candidate directory only counts when it holds the app's own marker
/// asset, so an unrelated `assets/` folder (a project the shell happens
/// to be in, `/usr/assets`) is never mounted over the install.
pub const MARKER: &str = "texture/dev_road.png";

/// How far above the executable's directory the dev-tree search climbs:
/// `target/debug/mm2` and `target/debug/deps/<test>` both reach the
/// repository root within it.
const ANCESTOR_LEVELS: usize = 4;

fn has_marker(dir: &Path) -> bool {
    dir.join(MARKER).is_file()
}

/// The directory holding the app's base assets, or `None`.
///
/// Order — the first candidate carrying [`MARKER`] wins:
/// 1. `<exe_dir>/assets` — a flat portable bundle (Windows/Linux zip);
/// 2. `<exe_dir>/../Resources/assets` — a macOS `.app` bundle;
/// 3. `<exe_dir>/../share/rust-mm2/assets` — a Unix prefix install;
/// 4. `assets/` of an ancestor of the executable — a cargo dev tree;
/// 5. `<cwd>/assets` — the pre-F30 behaviour, now the last resort.
pub fn locate(exe_dir: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = exe_dir {
        candidates.push(exe_dir.join("assets"));
        candidates.push(exe_dir.join("../Resources/assets"));
        candidates.push(exe_dir.join("../share/rust-mm2/assets"));
        candidates.extend(
            exe_dir
                .ancestors()
                .skip(1)
                .take(ANCESTOR_LEVELS)
                .map(|dir| dir.join("assets")),
        );
    }
    if let Some(cwd) = cwd {
        candidates.push(cwd.join("assets"));
    }
    candidates.into_iter().find(|dir| has_marker(dir))
}

/// [`locate`] for the running process.
pub fn locate_for_process() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent);
    let cwd = std::env::current_dir().ok();
    locate(exe_dir, cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn seed(dir: &Path) -> PathBuf {
        let marker = dir.join(MARKER);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"png").unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn a_bundle_next_to_the_binary_is_found_from_any_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("bundle");
        let assets = seed(&exe_dir.join("assets"));
        let elsewhere = tempfile::tempdir().unwrap();
        assert_eq!(locate(Some(&exe_dir), Some(elsewhere.path())), Some(assets));
    }

    #[test]
    fn a_macos_app_bundle_resolves_through_resources() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("mm2.app/Contents/MacOS");
        fs::create_dir_all(&exe_dir).unwrap();
        seed(&root.path().join("mm2.app/Contents/Resources/assets"));
        let found = locate(Some(&exe_dir), None).expect("bundle assets");
        assert!(has_marker(&found));
        assert!(found.ends_with("Resources/assets"));
    }

    #[test]
    fn a_unix_prefix_install_resolves_through_share() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("prefix/bin");
        fs::create_dir_all(&exe_dir).unwrap();
        seed(&root.path().join("prefix/share/rust-mm2/assets"));
        let found = locate(Some(&exe_dir), None).expect("prefix assets");
        assert!(found.ends_with("share/rust-mm2/assets"));
    }

    #[test]
    fn a_cargo_dev_tree_is_found_by_climbing_from_the_binary() {
        let root = tempfile::tempdir().unwrap();
        let assets = seed(&root.path().join("assets"));
        let exe_dir = root.path().join("target/debug");
        fs::create_dir_all(&exe_dir).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let found = locate(Some(&exe_dir), Some(elsewhere.path())).expect("dev tree assets");
        assert!(has_marker(&found));
        assert_eq!(
            found.canonicalize().unwrap(),
            assets.canonicalize().unwrap()
        );
    }

    #[test]
    fn the_binary_wins_over_an_unrelated_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("bundle");
        let ours = seed(&exe_dir.join("assets"));
        let cwd = tempfile::tempdir().unwrap();
        seed(&cwd.path().join("assets"));
        assert_eq!(locate(Some(&exe_dir), Some(cwd.path())), Some(ours));
    }

    #[test]
    fn the_working_directory_is_the_last_resort() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("nowhere/bin");
        fs::create_dir_all(&exe_dir).unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let assets = seed(&cwd.path().join("assets"));
        assert_eq!(locate(Some(&exe_dir), Some(cwd.path())), Some(assets));
    }

    #[test]
    fn an_assets_folder_without_the_marker_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("bundle");
        fs::create_dir_all(exe_dir.join("assets/unrelated")).unwrap();
        fs::write(exe_dir.join("assets/unrelated/file.txt"), b"x").unwrap();
        let cwd = tempfile::tempdir().unwrap();
        fs::create_dir_all(cwd.path().join("assets")).unwrap();
        assert_eq!(locate(Some(&exe_dir), Some(cwd.path())), None);
    }

    #[test]
    fn a_marker_that_is_a_directory_does_not_count() {
        let root = tempfile::tempdir().unwrap();
        let exe_dir = root.path().join("bundle");
        fs::create_dir_all(exe_dir.join("assets").join(MARKER)).unwrap();
        assert_eq!(locate(Some(&exe_dir), None), None);
    }

    #[test]
    fn nothing_known_finds_nothing() {
        assert_eq!(locate(None, None), None);
    }

    #[test]
    fn the_checked_in_assets_carry_the_marker() {
        // The repository's own `assets/` is what a dev build must find;
        // renaming the marker asset without updating `MARKER` would turn
        // the lookup into a silent no-op.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        assert!(has_marker(&repo), "{} lacks {MARKER}", repo.display());
    }
}

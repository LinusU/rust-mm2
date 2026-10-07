//! Shared mounting policy for an MM2 installation plus mods.
//!
//! Both the game executable and `mm2-inspect` go through this module so the
//! source order, priorities, path normalization and diagnostics are
//! identical everywhere.
//!
//! ## Default archive order
//!
//! With [`InstallMount::default`] every `*.ar` file in the install directory
//! is mounted in **sorted filename order** at [`priority::ARCHIVE`]; later
//! mounts win ties, so `mm2tex.ar` (last alphabetically) wins conflicts
//! between archives. The precedence the original engine used between its
//! retail archives is not verified — this default is deterministic, not
//! authoritative. Supply an explicit list via
//! [`InstallMount::with_archives`] to control it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::manifest::ModManifest;
use crate::{AssetsError, Vfs, priority};

/// How to mount an MM2 installation directory.
#[derive(Debug, Clone)]
pub struct InstallMount {
    /// Explicit archive file names (relative to the install dir), in mount
    /// order — later entries win ties at equal priority. `None` mounts every
    /// `*.ar` found in the directory in sorted order.
    pub archives: Option<Vec<String>>,
    /// Whether to mount loose files in the install dir at
    /// [`priority::LOOSE`].
    pub loose_files: bool,
}

impl Default for InstallMount {
    fn default() -> Self {
        Self {
            archives: None,
            loose_files: true,
        }
    }
}

impl InstallMount {
    /// Use an explicit archive list (file names relative to the install
    /// directory, mounted in the given order).
    pub fn with_archives(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            archives: Some(names.into_iter().map(Into::into).collect()),
            loose_files: true,
        }
    }
}

/// Diagnostics produced while mounting an installation.
#[derive(Debug, Default)]
pub struct MountReport {
    /// Archives successfully mounted, in mount order.
    pub archives: Vec<PathBuf>,
    /// Archives skipped because they failed to parse, with the error.
    pub skipped: Vec<(PathBuf, String)>,
    /// Whether the loose-file directory was mounted.
    pub loose_files: bool,
    /// Loaded mod manifests, in mount order.
    pub mods: Vec<ModManifest>,
}

/// Mount an MM2 installation directory into `vfs` using `plan`.
///
/// Loose files (when enabled) are mounted above the archives. Archive parse
/// failures are reported in [`MountReport::skipped`] rather than aborting —
/// a corrupt optional archive should not hide an otherwise valid install —
/// while I/O errors on the directory itself are returned.
pub fn mount_install(
    vfs: &mut Vfs,
    dir: &Path,
    plan: &InstallMount,
) -> Result<MountReport, AssetsError> {
    if !dir.is_dir() {
        return Err(AssetsError::MissingDirectory(dir.to_path_buf()));
    }
    let mut report = MountReport::default();

    let archives: Vec<PathBuf> = match &plan.archives {
        Some(names) => names.iter().map(|n| dir.join(n)).collect(),
        None => {
            let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
                .map_err(AssetsError::io(dir))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .map(|e| e.eq_ignore_ascii_case("ar"))
                        .unwrap_or(false)
                })
                .collect();
            found.sort();
            found
        }
    };
    for ar in &archives {
        if !ar.is_file() {
            report
                .skipped
                .push((ar.clone(), "archive not found".to_string()));
            continue;
        }
        match vfs.mount_archive(ar, priority::ARCHIVE) {
            Ok(()) => report.archives.push(ar.clone()),
            Err(e) => report.skipped.push((ar.clone(), e.to_string())),
        }
    }

    if plan.loose_files {
        vfs.mount_dir(dir, priority::LOOSE)?;
        report.loose_files = true;
    }
    Ok(report)
}

/// Mount every mod under `mods_dir` (each a directory containing
/// `mod.toml`) at [`priority::MOD`], stacked deterministically.
///
/// A missing `mods_dir` is not an error — mounting no mods is valid.
pub fn mount_mods(vfs: &mut Vfs, mods_dir: &Path) -> Result<Vec<ModManifest>, AssetsError> {
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }
    let manifests = vfs.mount_mods_dir(mods_dir, priority::MOD)?;
    for o in override_summary(vfs) {
        if o.between_mods {
            tracing::warn!(
                winner = %o.winner,
                shadowed = %o.shadowed,
                paths = o.paths,
                example = %o.example,
                "mod conflict: the later-mounted mod wins"
            );
        } else if o.winner_is_mod {
            tracing::info!(
                winner = %o.winner,
                shadowed = %o.shadowed,
                paths = o.paths,
                example = %o.example,
                "mod overrides original content"
            );
        }
    }
    Ok(manifests)
}

/// Label used in an [`OverrideSummary`] for any non-mod source (archives,
/// loose install files, override directories).
pub const INSTALL_LABEL: &str = "install";

/// How many logical paths one source takes from another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideSummary {
    /// Mod id of the winning source, or [`INSTALL_LABEL`]. Display only: a
    /// mod may be named `install`, so test [`Self::winner_is_mod`] instead.
    pub winner: String,
    /// Mod id of the shadowed source, or [`INSTALL_LABEL`]; see
    /// [`Self::shadowed_is_mod`].
    pub shadowed: String,
    /// The winner is a mod (decided by source kind, not by `winner`).
    pub winner_is_mod: bool,
    /// The shadowed source is a mod.
    pub shadowed_is_mod: bool,
    /// Logical paths the winner takes from the shadowed source.
    pub paths: usize,
    /// First such path in sorted order.
    pub example: String,
    /// Both sides are mods (a conflict, not an intentional replacement of
    /// original content).
    pub between_mods: bool,
}

/// Group every conflict in `vfs` by (winner, shadowed) source pair, sorted
/// by pair. A path shadowing several sources counts once per shadowed
/// source. Shared by the game's mount log and `mm2-inspect conflicts`.
pub fn override_summary(vfs: &Vfs) -> Vec<OverrideSummary> {
    // Keyed on source kind plus label, so a mod whose id is "install" stays
    // distinct from the install itself.
    type Side = (String, bool);
    let mut groups: BTreeMap<(Side, Side), OverrideSummary> = BTreeMap::new();
    for ex in vfs.conflicts() {
        let side = |c: &crate::Candidate| -> Side {
            (
                c.source
                    .label
                    .clone()
                    .unwrap_or_else(|| INSTALL_LABEL.to_string()),
                c.source.is_mod(),
            )
        };
        let winner = side(&ex.candidates[0]);
        for shadowed in &ex.candidates[1..] {
            let shadowed_side = side(shadowed);
            if !ex.candidates[0].source.is_mod() && !shadowed.source.is_mod() {
                // Two non-mod sources (archives, loose files) are one
                // "install" side; that is original-content layering, not
                // something a mod author did.
                continue;
            }
            groups
                .entry((winner.clone(), shadowed_side.clone()))
                .and_modify(|o| o.paths += 1)
                .or_insert_with(|| OverrideSummary {
                    winner: winner.0.clone(),
                    shadowed: shadowed_side.0.clone(),
                    winner_is_mod: winner.1,
                    shadowed_is_mod: shadowed_side.1,
                    paths: 1,
                    example: ex.logical.clone(),
                    between_mods: ex.candidates[0].source.is_mod() && shadowed.source.is_mod(),
                });
        }
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn mod_dir(mods: &Path, id: &str, files: &[&str]) {
        let m = mods.join(id);
        write(&m, "mod.toml", format!("[mod]\nid = \"{id}\"\n").as_bytes());
        for rel in files {
            write(&m, rel, id.as_bytes());
        }
    }

    #[test]
    fn summary_separates_install_overrides_from_mod_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, mods) = (tmp.path().join("base"), tmp.path().join("mods"));
        write(&base, "texture/a.tex", b"base");
        write(&base, "texture/b.tex", b"base");
        write(&base, "tune/c.txt", b"base");
        mod_dir(&mods, "alpha", &["texture/a.tex", "texture/b.tex"]);
        mod_dir(&mods, "beta", &["texture/a.tex", "texture/new.tex"]);

        let mut vfs = Vfs::new();
        mount_install(&mut vfs, &base, &InstallMount::default()).unwrap();
        mount_mods(&mut vfs, &mods).unwrap();

        let got = override_summary(&vfs);
        let row = |w: &str, s: &str, paths, example: &str, between| OverrideSummary {
            winner: w.into(),
            shadowed: s.into(),
            winner_is_mod: w != INSTALL_LABEL,
            shadowed_is_mod: s != INSTALL_LABEL,
            paths,
            example: example.into(),
            between_mods: between,
        };
        assert_eq!(
            got,
            [
                // beta (mounted last) takes texture/a from both alpha and
                // the install; alpha keeps texture/b over the install.
                row("alpha", INSTALL_LABEL, 1, "texture/b.tex", false),
                row("beta", "alpha", 1, "texture/a.tex", true),
                row("beta", INSTALL_LABEL, 1, "texture/a.tex", false),
            ]
        );
    }

    #[test]
    fn a_mod_named_install_is_not_the_install() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, mods) = (tmp.path().join("base"), tmp.path().join("mods"));
        write(&base, "texture/a.tex", b"base");
        write(&base, "texture/b.tex", b"base");
        mod_dir(&mods, "install", &["texture/a.tex", "texture/b.tex"]);
        mod_dir(&mods, "other", &["texture/b.tex"]);

        let mut vfs = Vfs::new();
        mount_install(&mut vfs, &base, &InstallMount::default()).unwrap();
        mount_mods(&mut vfs, &mods).unwrap();

        let got: Vec<_> = override_summary(&vfs)
            .into_iter()
            .map(|o| (o.winner_is_mod, o.shadowed_is_mod, o.between_mods, o.paths))
            .collect();
        // `install` (the mod) takes a.tex from the real install; `other`
        // takes b.tex from both the mod `install` and the real install.
        // The two rows naming "install" as shadowed stay separate.
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(
            got.iter()
                .filter(|r| **r == (true, false, false, 1))
                .count(),
            2,
            "{got:?}"
        );
        assert!(got.contains(&(true, true, true, 1)), "{got:?}");
    }

    #[test]
    fn no_mods_means_no_summary() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "texture/a.tex", b"base");
        let mut vfs = Vfs::new();
        mount_install(&mut vfs, tmp.path(), &InstallMount::default()).unwrap();
        mount_mods(&mut vfs, &tmp.path().join("absent")).unwrap();
        assert!(override_summary(&vfs).is_empty());
    }
}

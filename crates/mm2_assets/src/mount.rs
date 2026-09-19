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
    vfs.mount_mods_dir(mods_dir, priority::MOD)
}

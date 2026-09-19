//! VFS source implementations: DAVE archives and plain directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mm2_formats::dave::{DaveArchive, DaveEntry, inflate_entry};

use crate::{AssetsError, normalize_path};

/// What kind of backing store a resolved asset came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An original `DAVE` archive (`.ar`).
    Archive,
    /// A plain directory (loose install files, override dirs, mods).
    Directory,
}

/// Where a resolved asset physically lives.
#[derive(Debug, Clone)]
pub struct ResolvedSource {
    /// Kind of backing store.
    pub kind: SourceKind,
    /// Filesystem path of the archive file or directory root.
    pub path: PathBuf,
    /// Byte offset inside the archive, when applicable.
    pub archive_offset: Option<usize>,
    /// Optional human-readable label (e.g. mod id).
    pub label: Option<String>,
}

/// A mounted source of assets.
pub(crate) trait Source: Send + Sync {
    /// List every logical path this source provides (normalized).
    fn list(&self) -> Vec<String>;
    /// Read the contents of `logical` (already normalized).
    fn read(&self, logical: &str) -> Result<Vec<u8>, AssetsError>;
    /// Describe where `logical` lives inside this source.
    fn provenance(&self, logical: &str) -> ResolvedSource;
}

/// A mounted `DAVE` archive.
pub(crate) struct ArchiveSource {
    data: Arc<Vec<u8>>,
    /// logical path -> entry
    index: HashMap<String, DaveEntry>,
    path: PathBuf,
}

impl ArchiveSource {
    pub fn open(path: &Path) -> Result<Self, AssetsError> {
        let data = Arc::new(std::fs::read(path).map_err(AssetsError::io(path))?);
        let archive = DaveArchive::parse(&data).map_err(|e| AssetsError::Archive {
            path: path.to_path_buf(),
            source: e,
        })?;
        let index = deterministic_index(archive.entries().iter().map(|entry| {
            (
                normalize_path(&entry.name),
                entry.name.clone(),
                entry.clone(),
            )
        }));
        Ok(Self {
            data,
            index,
            path: path.to_path_buf(),
        })
    }
}

/// Build a logical-path index where the winner of a normalized collision is
/// deterministic: entries are sorted by `(logical, original name)` and the
/// first wins. Collisions are reported, never resolved by enumeration order.
fn deterministic_index<T>(
    entries: impl Iterator<Item = (Option<String>, String, T)>,
) -> HashMap<String, T> {
    let mut rows: Vec<(String, String, T)> = entries
        .filter_map(|(logical, original, value)| logical.map(|l| (l, original, value)))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut index = HashMap::with_capacity(rows.len());
    for (logical, original, value) in rows {
        if index.insert(logical.clone(), value).is_some() {
            tracing::warn!(
                logical = %logical,
                loser = %original,
                "normalized path collision inside one source; deterministic winner kept"
            );
        }
    }
    index
}

impl Source for ArchiveSource {
    fn list(&self) -> Vec<String> {
        self.index.keys().cloned().collect()
    }

    fn read(&self, logical: &str) -> Result<Vec<u8>, AssetsError> {
        let entry = self
            .index
            .get(logical)
            .ok_or_else(|| AssetsError::NotFound(logical.to_string()))?;
        inflate_entry(&self.data, entry).map_err(|e| AssetsError::Decompression {
            logical: logical.to_string(),
            reason: e.to_string(),
        })
    }

    fn provenance(&self, logical: &str) -> ResolvedSource {
        ResolvedSource {
            kind: SourceKind::Archive,
            path: self.path.clone(),
            archive_offset: self.index.get(logical).map(|e| e.data_offset),
            label: None,
        }
    }
}

/// A mounted directory tree (loose files, override dirs, mod contents).
pub(crate) struct DirSource {
    root: PathBuf,
    /// normalized logical path -> actual relative path on disk
    index: HashMap<String, PathBuf>,
    label: Option<String>,
}

impl DirSource {
    /// Mount `root`, walking it eagerly. `skip` is called with each
    /// normalized relative path; entries returning `true` are not indexed
    /// (used to hide `mod.toml` manifests).
    ///
    /// Containment policy: **symlinks are never followed** — neither file
    /// links nor directory links — so a mounted tree cannot expose files
    /// outside `root` or recurse through link cycles.
    pub fn mount(
        root: &Path,
        label: Option<String>,
        skip: &dyn Fn(&str) -> bool,
    ) -> Result<Self, AssetsError> {
        if !root.is_dir() {
            return Err(AssetsError::MissingDirectory(root.to_path_buf()));
        }
        let mut rows: Vec<(Option<String>, String, PathBuf)> = Vec::new();
        walk(root, root, &mut rows, skip)?;
        let index = deterministic_index(rows.into_iter());
        Ok(Self {
            root: root.to_path_buf(),
            index,
            label,
        })
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    rows: &mut Vec<(Option<String>, String, PathBuf)>,
    skip: &dyn Fn(&str) -> bool,
) -> Result<(), AssetsError> {
    let entries = std::fs::read_dir(dir).map_err(AssetsError::io(dir))?;
    for entry in entries {
        let entry = entry.map_err(AssetsError::io(dir))?;
        // `file_type` does not traverse links: symlinks are skipped outright
        // so mounts stay contained in `root` and cannot cycle.
        let file_type = entry.file_type().map_err(AssetsError::io(dir))?;
        let path = entry.path();
        if file_type.is_symlink() {
            tracing::warn!(path = %path.display(), "skipping symlink in mounted directory");
            continue;
        }
        if file_type.is_dir() {
            walk(root, &path, rows, skip)?;
        } else if file_type.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy();
            let logical = normalize_path(&rel_str);
            if logical.as_deref().is_some_and(skip) {
                continue;
            }
            rows.push((logical, rel_str.into_owned(), rel.to_path_buf()));
        }
    }
    Ok(())
}

impl Source for DirSource {
    fn list(&self) -> Vec<String> {
        self.index.keys().cloned().collect()
    }

    fn read(&self, logical: &str) -> Result<Vec<u8>, AssetsError> {
        let rel = self
            .index
            .get(logical)
            .ok_or_else(|| AssetsError::NotFound(logical.to_string()))?;
        // The index was built from actual walk results, so `rel` is always
        // inside `root`. Never join an attacker-controlled path directly.
        let full = self.root.join(rel);
        // Re-check at read time: a real file swapped for a symlink after
        // mount must not escape the mount root.
        let meta = std::fs::symlink_metadata(&full).map_err(AssetsError::io(&full))?;
        if !meta.is_file() {
            return Err(AssetsError::NotFound(logical.to_string()));
        }
        std::fs::read(&full).map_err(AssetsError::io(&full))
    }

    fn provenance(&self, logical: &str) -> ResolvedSource {
        ResolvedSource {
            kind: SourceKind::Directory,
            path: self
                .index
                .get(logical)
                .map(|rel| self.root.join(rel))
                .unwrap_or_else(|| self.root.clone()),
            archive_offset: None,
            label: self.label.clone(),
        }
    }
}

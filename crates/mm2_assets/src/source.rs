//! VFS source implementations: DAVE archives and plain directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mm2_formats::dave::{DaveArchive, DaveEntry};

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
        let mut index = HashMap::with_capacity(archive.entries().len());
        for entry in archive.entries() {
            if let Some(logical) = normalize_path(&entry.name) {
                index.insert(logical, entry.clone());
            }
        }
        Ok(Self {
            data,
            index,
            path: path.to_path_buf(),
        })
    }
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
        if entry.is_compressed() {
            inflate(
                &self.data[entry.data_offset..entry.data_offset + entry.stored_size],
                entry.size,
                logical,
            )
        } else {
            Ok(self.data[entry.data_offset..entry.data_offset + entry.stored_size].to_vec())
        }
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
    pub fn mount(
        root: &Path,
        label: Option<String>,
        skip: &dyn Fn(&str) -> bool,
    ) -> Result<Self, AssetsError> {
        if !root.is_dir() {
            return Err(AssetsError::MissingDirectory(root.to_path_buf()));
        }
        let mut index = HashMap::new();
        walk(root, root, &mut index, skip)?;
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
    index: &mut HashMap<String, PathBuf>,
    skip: &dyn Fn(&str) -> bool,
) -> Result<(), AssetsError> {
    let entries = std::fs::read_dir(dir).map_err(AssetsError::io(dir))?;
    for entry in entries {
        let entry = entry.map_err(AssetsError::io(dir))?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, index, skip)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy();
            let Some(logical) = normalize_path(&rel_str) else {
                continue;
            };
            if skip(&logical) {
                continue;
            }
            index.insert(logical, rel.to_path_buf());
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

/// Inflate raw DEFLATE data, verifying the expected size.
fn inflate(raw: &[u8], expected: usize, logical: &str) -> Result<Vec<u8>, AssetsError> {
    use std::io::Read;
    let mut out = Vec::with_capacity(expected);
    flate2::read::DeflateDecoder::new(raw)
        .read_to_end(&mut out)
        .map_err(|e| AssetsError::Decompression {
            logical: logical.to_string(),
            reason: e.to_string(),
        })?;
    if out.len() != expected {
        return Err(AssetsError::Decompression {
            logical: logical.to_string(),
            reason: format!("decompressed {} bytes, expected {expected}", out.len()),
        });
    }
    Ok(out)
}

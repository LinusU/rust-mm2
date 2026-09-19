//! The virtual filesystem: an ordered set of mounted sources plus a
//! normalized logical-path index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::manifest::{MANIFEST_FILE, ModManifest};
use crate::source::{ArchiveSource, DirSource, ResolvedSource, Source};
use crate::{AssetsError, normalize_path};

/// A resolved logical path: which source serves it and where it lives.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Normalized logical path, e.g. `texture/foo.tex`.
    pub logical: String,
    /// Physical source description (archive path + offset, or file path).
    pub source: ResolvedSource,
}

/// A mounted source plus its ordering information.
struct Mounted {
    source: Box<dyn Source>,
    priority: i32,
    /// Mount sequence number; later mounts win ties.
    seq: usize,
}

/// The virtual filesystem.
///
/// Resolution rules:
///
/// - logical paths are normalized (`normalize_path`);
/// - the source with the highest priority wins;
/// - ties go to the most recently mounted source;
/// - `resolve_preferred` picks between alternative extensions by priority
///   first, then by the caller's extension preference order — so a mod's
///   `foo.png` beats the archive's `foo.tex`, while inside one source the
///   modern format is preferred.
#[derive(Default)]
pub struct Vfs {
    sources: Vec<Mounted>,
    index: HashMap<String, usize>,
}

impl Vfs {
    /// An empty VFS.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount a DAVE archive.
    pub fn mount_archive(&mut self, path: &Path, priority: i32) -> Result<(), AssetsError> {
        let source = ArchiveSource::open(path)?;
        tracing::info!(
            archive = %path.display(),
            entries = source.list().len(),
            priority,
            "mounted archive"
        );
        self.push(Box::new(source), priority);
        Ok(())
    }

    /// Mount all `.ar` archives found in `dir`, in the given order. Unknown
    /// archive precedence is deliberately caller-controlled.
    pub fn mount_archives(
        &mut self,
        dir: &Path,
        names: &[&str],
        priority: i32,
    ) -> Result<(), AssetsError> {
        for name in names {
            let path = dir.join(name);
            if path.is_file() {
                self.mount_archive(&path, priority)?;
            }
        }
        Ok(())
    }

    /// Mount a directory tree (loose install files or an override dir).
    pub fn mount_dir(&mut self, dir: &Path, priority: i32) -> Result<(), AssetsError> {
        let source = DirSource::mount(dir, None, &|_| false)?;
        tracing::info!(
            dir = %dir.display(),
            entries = source.list().len(),
            priority,
            "mounted directory"
        );
        self.push(Box::new(source), priority);
        Ok(())
    }

    /// Mount a mod directory. Reads `mod.toml` and indexes the whole tree
    /// (excluding the manifest itself).
    pub fn mount_mod(&mut self, dir: &Path, priority: i32) -> Result<ModManifest, AssetsError> {
        let manifest = ModManifest::load(dir)?;
        let source = DirSource::mount(dir, Some(manifest.id.clone()), &|logical| {
            logical == MANIFEST_FILE
        })?;
        tracing::info!(
            mod_id = %manifest.id,
            dir = %dir.display(),
            entries = source.list().len(),
            priority,
            "mounted mod"
        );
        self.push(Box::new(source), priority);
        Ok(manifest)
    }

    /// Mount every subdirectory of `mods_dir` that contains a `mod.toml`.
    /// Mods are mounted in sorted directory order so behaviour is
    /// deterministic; later mods win on conflict.
    pub fn mount_mods_dir(
        &mut self,
        mods_dir: &Path,
        base_priority: i32,
    ) -> Result<Vec<ModManifest>, AssetsError> {
        if !mods_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(mods_dir)
            .map_err(AssetsError::io(mods_dir))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join(MANIFEST_FILE).is_file())
            .collect();
        dirs.sort();
        let mut manifests = Vec::new();
        for (i, dir) in dirs.iter().enumerate() {
            manifests.push(self.mount_mod(dir, base_priority + i as i32)?);
        }
        Ok(manifests)
    }

    fn push(&mut self, source: Box<dyn Source>, priority: i32) {
        let seq = self.sources.len();
        let idx = self.sources.len();
        for logical in source.list() {
            match self.index.get(&logical) {
                Some(&winner) if !self.beats(idx, priority, seq, winner) => {}
                _ => {
                    self.index.insert(logical, idx);
                }
            }
        }
        self.sources.push(Mounted {
            source,
            priority,
            seq,
        });
    }

    /// Does candidate (idx, priority, seq) beat the current winner?
    /// Note: at call time the candidate isn't in `sources` yet.
    fn beats(&self, idx: usize, priority: i32, seq: usize, winner: usize) -> bool {
        let w = &self.sources[winner];
        let _ = idx;
        priority > w.priority || (priority == w.priority && seq > w.seq)
    }

    /// Resolve a logical path.
    pub fn resolve(&self, path: &str) -> Option<Resolved> {
        let logical = normalize_path(path)?;
        let &idx = self.index.get(&logical)?;
        Some(Resolved {
            logical: logical.clone(),
            source: self.sources[idx].source.provenance(&logical),
        })
    }

    /// Resolve `stem` trying each extension in `exts`. Returns the best
    /// candidate by `(priority, extension preference)`.
    pub fn resolve_preferred(&self, stem: &str, exts: &[&str]) -> Option<Resolved> {
        let stem = normalize_path(stem)?;
        let mut best: Option<(usize, usize, String)> = None;
        for (rank, ext) in exts.iter().enumerate() {
            let logical = format!("{stem}.{ext}");
            if let Some(&idx) = self.index.get(&logical) {
                let better = match &best {
                    None => true,
                    Some((w, brank, _)) => {
                        let (wp, _) = (self.sources[*w].priority, self.sources[*w].seq);
                        let cp = self.sources[idx].priority;
                        cp > wp || (cp == wp && rank < *brank)
                    }
                };
                if better {
                    best = Some((idx, rank, logical));
                }
            }
        }
        best.map(|(idx, _, logical)| Resolved {
            source: self.sources[idx].source.provenance(&logical),
            logical,
        })
    }

    /// Read the bytes of a resolved asset.
    pub fn read(&self, resolved: &Resolved) -> Result<Vec<u8>, AssetsError> {
        self.read_logical(&resolved.logical)
    }

    /// Read by logical path directly.
    pub fn read_logical(&self, logical: &str) -> Result<Vec<u8>, AssetsError> {
        let idx = self
            .index
            .get(logical)
            .ok_or_else(|| AssetsError::NotFound(logical.to_string()))?;
        self.sources[*idx].source.read(logical)
    }

    /// Read and resolve in one step, returning bytes + provenance.
    pub fn read_path(&self, path: &str) -> Result<(Vec<u8>, Resolved), AssetsError> {
        let resolved = self
            .resolve(path)
            .ok_or_else(|| AssetsError::NotFound(path.to_string()))?;
        let bytes = self.read(&resolved)?;
        Ok((bytes, resolved))
    }

    /// All logical paths currently resolvable, sorted.
    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.index.keys().cloned().collect();
        v.sort();
        v
    }

    /// Number of mounted sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::priority;
    use std::fs;

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn deterministic_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let low = tmp.path().join("low");
        let high = tmp.path().join("high");
        write(&low, "texture/foo.tex", b"low");
        write(&high, "texture/foo.tex", b"high");

        let mut vfs = Vfs::new();
        vfs.mount_dir(&low, 0).unwrap();
        vfs.mount_dir(&high, 100).unwrap();

        let (bytes, r) = vfs.read_path("texture/foo.tex").unwrap();
        assert_eq!(bytes, b"high");
        assert_eq!(r.source.path, high.join("texture/foo.tex"));
    }

    #[test]
    fn mod_overrides_original_and_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let mods = tmp.path().join("mods");
        write(&base, "texture/foo.tex", b"orig");
        write(&base, "texture/bar.tex", b"orig-bar");
        let m = mods.join("hd");
        write(&m, "mod.toml", b"[mod]\nid = \"hd\"\n");
        write(&m, "texture/foo.png", b"png-data");

        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, priority::LOOSE).unwrap();
        vfs.mount_mods_dir(&mods, priority::MOD).unwrap();

        // Logical texture "foo": mod's png beats base's tex.
        let r = vfs
            .resolve_preferred("texture/foo", &["png", "ktx2", "tex"])
            .unwrap();
        assert_eq!(r.logical, "texture/foo.png");
        assert_eq!(vfs.read(&r).unwrap(), b"png-data");

        // Fallback: bar only exists in base.
        let r = vfs
            .resolve_preferred("texture/bar", &["png", "tex"])
            .unwrap();
        assert_eq!(r.logical, "texture/bar.tex");
        assert_eq!(vfs.read(&r).unwrap(), b"orig-bar");
    }

    #[test]
    fn case_insensitive_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        write(&base, "Texture/Foo.TEX", b"x");
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        assert!(vfs.resolve("texture/foo.tex").is_some());
        assert!(vfs.resolve("TEXTURE\\FOO.TEX").is_some());
    }

    #[test]
    fn rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        write(&base, "ok.txt", b"x");
        fs::write(tmp.path().join("secret.txt"), b"s").unwrap();
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        assert!(vfs.resolve("../secret.txt").is_none());
        assert!(vfs.resolve("/etc/passwd").is_none());
        assert!(vfs.resolve("texture/../../secret.txt").is_none());
    }

    #[test]
    fn manifest_not_exposed_as_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        let m = mods.join("m1");
        write(&m, "mod.toml", b"[mod]\nid = \"m1\"\n");
        write(&m, "geometry/x.pkg", b"pkg");
        let mut vfs = Vfs::new();
        let manifests = vfs.mount_mods_dir(&mods, priority::MOD).unwrap();
        assert_eq!(manifests[0].id, "m1");
        assert!(vfs.resolve("mod.toml").is_none());
        assert!(vfs.resolve("geometry/x.pkg").is_some());
    }
}

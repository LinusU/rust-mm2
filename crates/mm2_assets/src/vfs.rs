//! The virtual filesystem: an ordered set of mounted sources plus a
//! normalized logical-path index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::manifest::{MANIFEST_FILE, ModManifest};
use crate::source::{ArchiveSource, DirSource, ResolvedSource, Source};
use crate::{AssetsError, normalize_path};

/// A resolved logical path: which source serves it and where it lives.
///
/// A `Resolved` pins its source identity: reading through it later returns
/// bytes from the same source even if higher-priority sources have been
/// mounted since.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Normalized logical path, e.g. `texture/foo.tex`.
    pub logical: String,
    /// Physical source description (archive path + offset, or file path).
    pub source: ResolvedSource,
    /// Index of the winning source inside `Vfs::sources`.
    source_index: usize,
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
/// - `resolve_preferred` picks between alternative extensions by source
///   first (priority, then mount sequence) and only then by the caller's
///   extension preference order *within the winning source* — so a mod's
///   `foo.png` beats the archive's `foo.tex`, and a later-mounted
///   same-priority source's `foo.tex` beats an earlier source's `foo.png`.
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
            return Err(AssetsError::MissingDirectory(mods_dir.to_path_buf()));
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

    fn resolved(&self, idx: usize, logical: String) -> Resolved {
        Resolved {
            source: self.sources[idx].source.provenance(&logical),
            logical,
            source_index: idx,
        }
    }

    /// Resolve a logical path.
    pub fn resolve(&self, path: &str) -> Option<Resolved> {
        let logical = normalize_path(path)?;
        let &idx = self.index.get(&logical)?;
        Some(self.resolved(idx, logical))
    }

    /// Resolve `stem` trying each extension in `exts`.
    ///
    /// Ordering: source priority first, then mount sequence; only within the
    /// winning source does the caller's extension preference apply. A mod's
    /// `foo.png` therefore beats the archive's `foo.tex`, but a later
    /// same-priority source's `foo.tex` beats an earlier source's `foo.png`,
    /// and a lower-priority source never wins on mount order alone.
    pub fn resolve_preferred(&self, stem: &str, exts: &[&str]) -> Option<Resolved> {
        let stem = normalize_path(stem)?;
        let mut best: Option<(usize, usize, String)> = None;
        for (rank, ext) in exts.iter().enumerate() {
            let logical = format!("{stem}.{ext}");
            if let Some(&idx) = self.index.get(&logical) {
                let better = match &best {
                    None => true,
                    Some((w, brank, _)) => {
                        let (w, brank) = (*w, *brank);
                        let (wcand, ccand) = (&self.sources[w], &self.sources[idx]);
                        ccand.priority > wcand.priority
                            || (ccand.priority == wcand.priority && ccand.seq > wcand.seq)
                            || (idx == w && rank < brank)
                    }
                };
                if better {
                    best = Some((idx, rank, logical));
                }
            }
        }
        best.map(|(idx, _, logical)| self.resolved(idx, logical))
    }

    /// Read the bytes of a resolved asset through the source that produced
    /// the resolution — never a newer mount that now wins the same logical
    /// path.
    pub fn read(&self, resolved: &Resolved) -> Result<Vec<u8>, AssetsError> {
        let mounted = self
            .sources
            .get(resolved.source_index)
            .ok_or_else(|| AssetsError::NotFound(resolved.logical.clone()))?;
        mounted.source.read(&resolved.logical)
    }

    /// Read by logical path directly (normalized like [`resolve`](Self::resolve)).
    pub fn read_logical(&self, logical: &str) -> Result<Vec<u8>, AssetsError> {
        let logical =
            normalize_path(logical).ok_or_else(|| AssetsError::InvalidPath(logical.to_string()))?;
        let idx = self
            .index
            .get(&logical)
            .ok_or(AssetsError::NotFound(logical.clone()))?;
        self.sources[*idx].source.read(&logical)
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
    fn source_priority_beats_extension_preference() {
        // A higher-priority source's .tex must beat a lower-priority source's
        // .png even though .png is the preferred extension and the source was
        // mounted later.
        let tmp = tempfile::tempdir().unwrap();
        let orig = tmp.path().join("orig");
        let low_mod = tmp.path().join("lowmod");
        write(&orig, "texture/foo.tex", b"orig-tex");
        write(&low_mod, "texture/foo.png", b"low-png");

        let mut vfs = Vfs::new();
        vfs.mount_dir(&orig, priority::ARCHIVE).unwrap();
        vfs.mount_dir(&low_mod, priority::ARCHIVE).unwrap(); // same tier, later seq

        // Same priority: later mount wins regardless of extension rank.
        let r = vfs
            .resolve_preferred("texture/foo", &["png", "tex"])
            .unwrap();
        assert_eq!(r.logical, "texture/foo.png");

        // Higher priority source wins even with the less-preferred extension.
        let mut vfs = Vfs::new();
        vfs.mount_dir(&low_mod, priority::ARCHIVE).unwrap();
        vfs.mount_dir(&orig, priority::LOOSE).unwrap();
        let r = vfs
            .resolve_preferred("texture/foo", &["png", "tex"])
            .unwrap();
        assert_eq!(r.logical, "texture/foo.tex");
        assert_eq!(vfs.read(&r).unwrap(), b"orig-tex");
    }

    #[test]
    fn same_source_prefers_listed_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("d");
        write(&dir, "texture/foo.tex", b"tex");
        write(&dir, "texture/foo.png", b"png");
        let mut vfs = Vfs::new();
        vfs.mount_dir(&dir, 0).unwrap();
        let r = vfs
            .resolve_preferred("texture/foo", &["png", "tex"])
            .unwrap();
        assert_eq!(r.logical, "texture/foo.png");
        let r = vfs
            .resolve_preferred("texture/foo", &["tex", "png"])
            .unwrap();
        assert_eq!(r.logical, "texture/foo.tex");
    }

    #[test]
    fn resolved_reads_keep_their_source() {
        // A resolution taken before a newer mount must keep reading the
        // originally selected source.
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write(&a, "texture/foo.tex", b"from-a");
        write(&b, "texture/foo.tex", b"from-b");

        let mut vfs = Vfs::new();
        vfs.mount_dir(&a, 0).unwrap();
        let stale = vfs.resolve("texture/foo.tex").unwrap();
        vfs.mount_dir(&b, 0).unwrap(); // same priority, later seq wins new lookups

        let fresh = vfs.resolve("texture/foo.tex").unwrap();
        assert_eq!(vfs.read(&fresh).unwrap(), b"from-b");
        assert_eq!(vfs.read(&stale).unwrap(), b"from-a");
        assert_eq!(stale.source.path, a.join("texture/foo.tex"));
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
    fn case_collision_is_deterministic() {
        // Two files that normalize to the same logical path inside one
        // source: the lexicographically smallest original wins, every time.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        write(&base, "texture/foo.tex", b"lower");
        write(&base, "texture/FOO.tex", b"upper");
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        assert_eq!(
            vfs.read_logical("texture/foo.tex").unwrap(),
            b"upper",
            "FOO.tex < foo.tex lexicographically"
        );
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
        assert!(vfs.read_logical("../secret.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let outside = tmp.path().join("outside");
        write(&base, "ok.txt", b"x");
        write(&outside, "secret.txt", b"s");
        write(&outside, "dir/in_dir.txt", b"d");
        std::os::unix::fs::symlink(outside.join("secret.txt"), base.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("linkdir")).unwrap();
        // A cyclic link must not hang the walk either.
        std::os::unix::fs::symlink(".", base.join("cycle")).unwrap();

        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        assert!(vfs.resolve("ok.txt").is_some());
        assert!(vfs.resolve("link.txt").is_none());
        assert!(vfs.resolve("linkdir/secret.txt").is_none());
        assert!(vfs.resolve("linkdir/in_dir.txt").is_none());
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

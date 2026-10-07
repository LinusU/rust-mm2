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
    /// Every source that provides a logical path, in mount order — the
    /// shadowed candidates behind each `index` winner, kept so a conflict
    /// can be explained rather than only resolved.
    providers: HashMap<String, Vec<usize>>,
    /// Mounted mods' manifest ids and directories: an id names one mod, so
    /// a second mount of it is refused rather than merged into the first.
    mod_ids: Vec<(String, PathBuf)>,
    /// Bumped every time the set of mounted sources changes (a mount or a
    /// rollback) and never reused, so a value read earlier names exactly
    /// one mount set. See [`Vfs::revision`].
    revision: u64,
}

/// Why the winning source of a logical path won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinReason {
    /// No other mounted source provides the path.
    OnlySource,
    /// The winner's priority tier is strictly higher than every other
    /// provider's.
    Priority,
    /// The winner ties the runner-up's priority and was mounted later.
    MountOrder,
}

impl WinReason {
    /// One-line explanation, shared by every tool that reports a conflict.
    pub fn describe(self) -> &'static str {
        match self {
            WinReason::OnlySource => "only source providing the path",
            WinReason::Priority => "higher priority tier than every other source",
            WinReason::MountOrder => "same priority as the runner-up; mounted later",
        }
    }
}

/// One source that provides a logical path.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Where the path lives inside this source.
    pub source: ResolvedSource,
    /// The source's priority tier.
    pub priority: i32,
    /// Mount sequence number (0 is the first mount).
    pub mount_seq: usize,
}

/// Every source providing one logical path, winner first.
///
/// Candidates are ordered exactly as resolution ranks them (priority, then
/// mount sequence, both descending), so `candidates[0]` is what
/// [`Vfs::resolve`] returns.
#[derive(Debug, Clone)]
pub struct Explanation {
    /// Normalized logical path.
    pub logical: String,
    /// Winner first, then each shadowed source in rank order.
    pub candidates: Vec<Candidate>,
    /// Why `candidates[0]` won.
    pub reason: WinReason,
}

impl Explanation {
    /// Whether more than one source provides the path.
    pub fn is_conflict(&self) -> bool {
        self.candidates.len() > 1
    }

    /// Multi-line report: the winner, each shadowed source in rank order,
    /// and why the winner won. The inspector prints it and the mount log
    /// summarises the same data, so every tool explains a conflict alike.
    pub fn render(&self) -> String {
        let mut out = format!("logical : {}\n", self.logical);
        for (i, c) in self.candidates.iter().enumerate() {
            let role = if i == 0 { "winner  " } else { "shadowed" };
            out.push_str(&format!(
                "{role}: {} [priority {}, mount #{}]\n",
                c.source.describe(),
                c.priority,
                c.mount_seq
            ));
        }
        out.push_str(&format!("reason  : {}\n", self.reason.describe()));
        out
    }

    /// Whether the winner and at least one shadowed source are both mods —
    /// a conflict between mods, not a mod replacing original content.
    pub fn is_mod_conflict(&self) -> bool {
        self.candidates.len() > 1
            && self.candidates.iter().filter(|c| c.source.is_mod()).count() > 1
            && self.candidates[0].source.is_mod()
    }
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
        if let Some((_, first)) = self.mod_ids.iter().find(|(id, _)| *id == manifest.id) {
            return Err(AssetsError::DuplicateModId {
                id: manifest.id,
                first: first.clone(),
                second: dir.to_path_buf(),
            });
        }
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
        self.mod_ids.push((manifest.id.clone(), dir.to_path_buf()));
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
        // All or nothing: a directory scan that fails part-way must not
        // leave its earlier mods mounted, or the caller (which reports "no
        // mods" on an error) and the VFS would disagree about the content.
        let (sources_before, mods_before) = (self.sources.len(), self.mod_ids.len());
        let mut manifests = Vec::new();
        for (i, dir) in dirs.iter().enumerate() {
            match self.mount_mod(dir, base_priority + i as i32) {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    self.truncate(sources_before, mods_before);
                    return Err(e);
                }
            }
        }
        Ok(manifests)
    }

    /// Drop every source mounted after the first `sources` (and the mod ids
    /// recorded after the first `mods`), then rebuild the path indexes so
    /// they describe exactly the sources that remain.
    fn truncate(&mut self, sources: usize, mods: usize) {
        self.sources.truncate(sources);
        self.mod_ids.truncate(mods);
        self.revision += 1;
        self.index.clear();
        self.providers.clear();
        for idx in 0..self.sources.len() {
            self.index_source(idx);
        }
    }

    fn push(&mut self, source: Box<dyn Source>, priority: i32) {
        let seq = self.sources.len();
        self.revision += 1;
        self.sources.push(Mounted {
            source,
            priority,
            seq,
        });
        self.index_source(seq);
    }

    /// Record source `idx`'s paths in the provider lists and the winner
    /// index. Sources must be indexed in mount order.
    fn index_source(&mut self, idx: usize) {
        let (priority, seq) = (self.sources[idx].priority, self.sources[idx].seq);
        for logical in self.sources[idx].source.list() {
            self.providers.entry(logical.clone()).or_default().push(idx);
            match self.index.get(&logical) {
                Some(&winner) if !self.beats(priority, seq, winner) => {}
                _ => {
                    self.index.insert(logical, idx);
                }
            }
        }
    }

    /// Does a candidate with this priority and mount sequence beat the
    /// current winner?
    fn beats(&self, priority: i32, seq: usize, winner: usize) -> bool {
        let w = &self.sources[winner];
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

    /// Every source that provides `path`, ranked the way resolution ranks
    /// them, with the reason the winner won. `None` when nothing provides
    /// the path.
    pub fn explain(&self, path: &str) -> Option<Explanation> {
        let logical = normalize_path(path)?;
        self.explanation(logical)
    }

    /// Every logical path provided by more than one source, sorted by path.
    /// This is the complete override map: original content a mod replaces
    /// and mod-against-mod conflicts alike.
    pub fn conflicts(&self) -> Vec<Explanation> {
        let mut paths: Vec<&String> = self
            .providers
            .iter()
            .filter(|(_, idxs)| idxs.len() > 1)
            .map(|(logical, _)| logical)
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|logical| self.explanation(logical.clone()))
            .collect()
    }

    fn explanation(&self, logical: String) -> Option<Explanation> {
        let mut idxs = self.providers.get(&logical)?.clone();
        // Rank exactly as `beats` does: priority, then mount sequence.
        idxs.sort_by_key(|&i| std::cmp::Reverse((self.sources[i].priority, self.sources[i].seq)));
        let reason = match idxs.as_slice() {
            [_] => WinReason::OnlySource,
            [a, b, ..] if self.sources[*a].priority > self.sources[*b].priority => {
                WinReason::Priority
            }
            _ => WinReason::MountOrder,
        };
        let candidates = idxs
            .into_iter()
            .map(|i| Candidate {
                source: self.sources[i].source.provenance(&logical),
                priority: self.sources[i].priority,
                mount_seq: self.sources[i].seq,
            })
            .collect();
        Some(Explanation {
            logical,
            candidates,
            reason,
        })
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

    /// Identity of the current mount set for anything that derives data from
    /// it without borrowing the VFS (a stem index, a decoded-asset cache).
    /// It changes on every mount and every rollback and never repeats, so
    /// a holder stamps the value it built from and treats any other value
    /// as stale: replacing content is a remount, and a remount must not
    /// leave an old index answering for the new layout. It identifies the
    /// *layout* of sources, not file bytes — [`fingerprint`](crate::fingerprint)
    /// is the content-level identity.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Manifest ids of the mounted mods, in mount order.
    pub fn mod_ids(&self) -> impl Iterator<Item = &str> {
        self.mod_ids.iter().map(|(id, _)| id.as_str())
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

    fn mod_dir(mods: &Path, id: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let m = mods.join(id);
        write(&m, "mod.toml", format!("[mod]\nid = \"{id}\"\n").as_bytes());
        for (rel, bytes) in files {
            write(&m, rel, bytes);
        }
        m
    }

    #[test]
    fn explain_ranks_every_provider_and_names_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let mods = tmp.path().join("mods");
        write(&base, "texture/shared.tex", b"base");
        write(&base, "texture/base_only.tex", b"base");
        write(&base, "tune/over.txt", b"base");
        mod_dir(
            &mods,
            "a_first",
            &[("texture/shared.tex", b"a"), ("tune/over.txt", b"a")],
        );
        mod_dir(&mods, "b_second", &[("texture/shared.tex", b"b")]);

        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, priority::LOOSE).unwrap();
        vfs.mount_mods_dir(&mods, priority::MOD).unwrap();

        // Three providers: the later-stacked mod outranks the earlier mod
        // (a higher priority tier), which outranks the install.
        let ex = vfs.explain("Texture\\SHARED.tex").unwrap();
        assert_eq!(ex.logical, "texture/shared.tex");
        let labels: Vec<_> = ex
            .candidates
            .iter()
            .map(|c| c.source.label.as_deref())
            .collect();
        assert_eq!(labels, [Some("b_second"), Some("a_first"), None]);
        assert_eq!(ex.reason, WinReason::Priority);
        assert!(ex.is_conflict() && ex.is_mod_conflict());
        // The explanation's winner is what resolution serves.
        let r = vfs.resolve("texture/shared.tex").unwrap();
        assert_eq!(ex.candidates[0].source.path, r.source.path);
        assert_eq!(vfs.read(&r).unwrap(), b"b");

        // A mod over the install only: a conflict, not a mod conflict.
        let ex = vfs.explain("tune/over.txt").unwrap();
        assert_eq!(ex.candidates.len(), 2);
        assert!(ex.is_conflict() && !ex.is_mod_conflict());

        // One provider, and no provider.
        let ex = vfs.explain("texture/base_only.tex").unwrap();
        assert_eq!(ex.reason, WinReason::OnlySource);
        assert!(!ex.is_conflict());
        assert!(vfs.explain("texture/nope.tex").is_none());
        assert!(vfs.explain("../escape").is_none());

        // `conflicts` lists exactly the multiply-provided paths, sorted.
        let paths: Vec<_> = vfs.conflicts().into_iter().map(|e| e.logical).collect();
        assert_eq!(paths, ["texture/shared.tex", "tune/over.txt"]);
    }

    #[test]
    fn explain_names_mount_order_when_priorities_tie() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        let first = mod_dir(&mods, "first", &[("texture/x.tex", b"1")]);
        let second = mod_dir(&mods, "second", &[("texture/x.tex", b"2")]);

        // Same tier, mounted in either order: the later mount wins and the
        // explanation says so; swapping the order swaps the winner.
        for (order, winner) in [([&first, &second], "second"), ([&second, &first], "first")] {
            let mut vfs = Vfs::new();
            for dir in order {
                vfs.mount_mod(dir, priority::MOD).unwrap();
            }
            let ex = vfs.explain("texture/x.tex").unwrap();
            assert_eq!(ex.reason, WinReason::MountOrder);
            assert_eq!(ex.candidates[0].source.label.as_deref(), Some(winner));
            assert!(ex.candidates[0].mount_seq > ex.candidates[1].mount_seq);
            let r = vfs.resolve("texture/x.tex").unwrap();
            assert_eq!(r.source.label.as_deref(), Some(winner));
        }
    }

    #[test]
    fn rendered_explanation_is_stable_and_names_both_sides() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let mods = tmp.path().join("mods");
        write(&base, "a.txt", b"base");
        mod_dir(&mods, "m", &[("a.txt", b"mod")]);
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, priority::LOOSE).unwrap();
        vfs.mount_mods_dir(&mods, priority::MOD).unwrap();

        let text = vfs.explain("a.txt").unwrap().render();
        let expected = format!(
            "logical : a.txt\nwinner  : mod `m` ({}) [priority 300, mount #1]\nshadowed: directory {} [priority 100, mount #0]\nreason  : higher priority tier than every other source\n",
            mods.join("m/a.txt").display(),
            base.join("a.txt").display(),
        );
        assert_eq!(text, expected);
        // Deterministic: a fresh mount of the same sources renders the same.
        let mut again = Vfs::new();
        again.mount_dir(&base, priority::LOOSE).unwrap();
        again.mount_mods_dir(&mods, priority::MOD).unwrap();
        assert_eq!(again.explain("a.txt").unwrap().render(), text);
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

    #[test]
    fn oversized_loose_file_is_refused_not_read() {
        use mm2_formats::dave::MAX_ENTRY_SIZE;
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        write(&base, "ok.bin", b"fine");
        // Sparse: no real disk or memory is needed for the oversize file.
        let big = fs::File::create(base.join("huge.bin")).unwrap();
        big.set_len(MAX_ENTRY_SIZE as u64 + 1).unwrap();
        let edge = fs::File::create(base.join("edge.bin")).unwrap();
        edge.set_len(MAX_ENTRY_SIZE as u64).unwrap();
        drop((big, edge));
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        // The file stays visible (provenance can name it) but reading it is
        // an explicit, typed refusal.
        assert!(vfs.resolve("huge.bin").is_some());
        match vfs.read_logical("huge.bin") {
            Err(AssetsError::TooLarge { size, limit, .. }) => {
                assert_eq!(size, MAX_ENTRY_SIZE as u64 + 1);
                assert_eq!(limit, MAX_ENTRY_SIZE as u64);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert_eq!(vfs.read_logical("ok.bin").unwrap(), b"fine");
    }

    #[test]
    fn oversized_or_linked_manifest_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        let big = mods.join("big");
        write(&big, "mod.toml", b"[mod]\nid = \"big\"\n");
        let f = fs::OpenOptions::new()
            .write(true)
            .open(big.join("mod.toml"))
            .unwrap();
        f.set_len(crate::manifest::MAX_MANIFEST_SIZE + 1).unwrap();
        drop(f);
        let mut vfs = Vfs::new();
        assert!(matches!(
            vfs.mount_mods_dir(&mods, priority::MOD),
            Err(AssetsError::TooLarge { .. })
        ));
        assert_eq!(vfs.source_count(), 0, "a rejected mod is not half-mounted");

        #[cfg(unix)]
        {
            let linked = tmp.path().join("linked");
            write(&linked, "x.txt", b"x");
            let real = tmp.path().join("real.toml");
            fs::write(&real, b"[mod]\nid = \"linked\"\n").unwrap();
            std::os::unix::fs::symlink(&real, linked.join("mod.toml")).unwrap();
            let mut vfs = Vfs::new();
            assert!(matches!(
                vfs.mount_mod(&linked, priority::MOD),
                Err(AssetsError::ModManifest { .. })
            ));
        }
    }

    #[test]
    fn a_mod_id_declared_twice_is_refused_not_merged() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        // Two directories, one manifest id: a renamed or copied mod folder.
        for dir in ["a_pack", "b_pack_copy"] {
            write(&mods.join(dir), "mod.toml", b"[mod]\nid = \"pack\"\n");
            write(&mods.join(dir), "texture/x.tex", dir.as_bytes());
        }
        write(
            &mods.join("c_other"),
            "mod.toml",
            b"[mod]\nid = \"other\"\n",
        );

        let base = tmp.path().join("base");
        write(&base, "texture/x.tex", b"base");
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        let err = vfs.mount_mods_dir(&mods, priority::MOD).unwrap_err();
        match &err {
            AssetsError::DuplicateModId { id, first, second } => {
                assert_eq!(id, "pack");
                assert!(first.ends_with("a_pack") && second.ends_with("b_pack_copy"));
            }
            other => panic!("expected DuplicateModId, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("a_pack") && msg.contains("b_pack_copy"),
            "{msg}"
        );
        // The directory scan is all or nothing: the refused copy and the mod
        // mounted before it are both gone, and the base mounted earlier
        // serves its own content again (indexes rebuilt, not left stale).
        assert_eq!(vfs.source_count(), 1);
        let r = vfs.resolve("texture/x.tex").unwrap();
        assert_eq!(vfs.read(&r).unwrap(), b"base");
        assert!(vfs.explain("texture/x.tex").unwrap().candidates.len() == 1);
        assert!(vfs.conflicts().is_empty());

        // Mounting one at a time keeps the first mount and still accepts a
        // distinct id afterwards.
        let mut vfs = Vfs::new();
        vfs.mount_mod(&mods.join("a_pack"), priority::MOD).unwrap();
        assert!(matches!(
            vfs.mount_mod(&mods.join("b_pack_copy"), priority::MOD),
            Err(AssetsError::DuplicateModId { .. })
        ));
        assert_eq!(vfs.source_count(), 1);
        let r = vfs.resolve("texture/x.tex").unwrap();
        assert_eq!(vfs.read(&r).unwrap(), b"a_pack");
        vfs.mount_mod(&mods.join("c_other"), priority::MOD + 1)
            .unwrap();
    }

    #[test]
    fn the_revision_names_one_mount_set_and_is_never_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        write(&mods.join("a_good"), "mod.toml", b"[mod]\nid = \"good\"\n");
        write(&mods.join("b_bad"), "mod.toml", b"[mod]\nid = \"bad\"\n");
        let f = fs::OpenOptions::new()
            .write(true)
            .open(mods.join("b_bad").join("mod.toml"))
            .unwrap();
        f.set_len(crate::manifest::MAX_MANIFEST_SIZE + 1).unwrap();
        drop(f);
        let base = tmp.path().join("base");
        write(&base, "texture/x.tex", b"base");

        let mut vfs = Vfs::new();
        let empty = vfs.revision();
        vfs.mount_dir(&base, 0).unwrap();
        let base_only = vfs.revision();
        assert_ne!(empty, base_only, "a mount changes the identity");
        // Reading never moves it.
        let _ = vfs.list();
        let _ = vfs.resolve("texture/x.tex");
        assert_eq!(vfs.revision(), base_only);

        // The scan mounts `a_good` then rolls it back: the layout is the
        // base-only one again, but the identity must not return to a value
        // a holder could have stamped before the detour.
        assert!(vfs.mount_mods_dir(&mods, priority::MOD).is_err());
        assert_eq!(vfs.source_count(), 1);
        assert!(
            vfs.revision() > base_only,
            "a rollback is a change, not a return"
        );
    }

    #[test]
    fn a_failed_mods_directory_scan_mounts_none_of_its_mods() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().join("mods");
        // Sorted order: `a_good` mounts (and shadows the base), then
        // `b_bad`'s oversize manifest fails the scan.
        write(&mods.join("a_good"), "mod.toml", b"[mod]\nid = \"good\"\n");
        write(&mods.join("a_good"), "texture/x.tex", b"good");
        write(&mods.join("b_bad"), "mod.toml", b"[mod]\nid = \"bad\"\n");
        let f = fs::OpenOptions::new()
            .write(true)
            .open(mods.join("b_bad").join("mod.toml"))
            .unwrap();
        f.set_len(crate::manifest::MAX_MANIFEST_SIZE + 1).unwrap();
        drop(f);
        let base = tmp.path().join("base");
        write(&base, "texture/x.tex", b"base");

        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        assert!(matches!(
            vfs.mount_mods_dir(&mods, priority::MOD),
            Err(AssetsError::TooLarge { .. })
        ));
        assert_eq!(vfs.source_count(), 1);
        let r = vfs.resolve("texture/x.tex").unwrap();
        assert_eq!(vfs.read(&r).unwrap(), b"base");
        assert!(vfs.conflicts().is_empty());

        // The rolled-back mod's id is free again: fixing the bad mod and
        // rescanning mounts both without a spurious duplicate.
        let f = fs::OpenOptions::new()
            .write(true)
            .open(mods.join("b_bad").join("mod.toml"))
            .unwrap();
        f.set_len(0).unwrap();
        drop(f);
        fs::write(
            mods.join("b_bad").join("mod.toml"),
            b"[mod]\nid = \"bad\"\n",
        )
        .unwrap();
        let manifests = vfs.mount_mods_dir(&mods, priority::MOD).unwrap();
        assert_eq!(manifests.len(), 2);
        let r = vfs.resolve("texture/x.tex").unwrap();
        assert_eq!(vfs.read(&r).unwrap(), b"good");
    }

    #[test]
    fn malformed_logical_paths_never_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        write(&base, "ok.txt", b"x");
        let mut vfs = Vfs::new();
        vfs.mount_dir(&base, 0).unwrap();
        for bad in [
            "",
            ".",
            "..",
            "\\..\\ok.txt",
            "C:\\ok.txt",
            "ok.txt\0.png",
            "a/../../ok.txt",
            "//ok.txt",
        ] {
            assert!(vfs.resolve(bad).is_none(), "{bad:?} must not resolve");
            assert!(vfs.read_logical(bad).is_err(), "{bad:?} must not read");
        }
    }
}

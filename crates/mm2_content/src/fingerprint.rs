//! Gameplay-relevant content fingerprint (F24-A; F29-AC05's split).
//!
//! [`gameplay`] hashes the *resolved bytes* of the logical paths that can
//! change the simulation or its rules — tuning, collision bounds, model
//! geometry (collision hulls, wheel origins), city data the world is
//! built from and every event record — so two peers fingerprint
//! identically only when their effective gameplay content is byte-equal.
//! Cosmetic families (`texture/`, `aud/`, menu art under `jpg/`, plus the
//! visual-only city records: skies, lighting and visibility sets) are
//! deliberately excluded: a paint or soundtrack mod must not block
//! joining (F24-AC02).
//!
//! The classification is an **implementation choice**, not an original
//! format claim: it names which families *this engine's* simulation
//! consumes. As new consumers land (pedestrians, audio cues with
//! gameplay triggers) their families join the set and the fingerprint's
//! meaning is versioned by [`PROTOCOL_VERSION`](mm2_net is the consumer
//! side; the classifier and the protocol version move together).

use mm2_assets::fingerprint::{self, FNV_OFFSET_BASIS, fnv};
use mm2_assets::{AssetsError, Vfs};

/// Top-level logical prefixes whose resolved content is gameplay data.
const GAMEPLAY_PREFIXES: &[&str] = &[
    // Vehicle/ambient tuning records (`vehCarSim`, `vehStuck`, …).
    "tune/",
    // `.bnd` collision bounds.
    "bound/",
    // `.pkg` meshes and `.mtx` part data: collision hulls and wheel
    // origins are derived from them.
    "geometry/",
    // Event definitions: aimap, opponent paths, waypoints, start grids,
    // crash-course tables, rewards.
    "race/",
    // Pedestrian rigs, animation and state records — currently
    // presentation-only, but consumed by the traffic/ped domains the
    // session will replicate; cheap to pin now.
    "anim/",
    // Player/roster records.
    "players/",
];

/// City extensions whose content is presentation-only: skies, lighting
/// (`ldef`/`lmap`/`ltNN`) and visibility sets (`cpvs`/`pvs`/`pvshist`)
/// drive culling and mood, never rules. Everything else under `city/` —
/// `psdl`, `bai`, `aimap`, `inst`, prop-rule CSVs, `pathset`, `water`,
/// and even unknowns — is hashed so an unseen gameplay family cannot
/// slip past the gate.
fn city_ext_is_visual(ext: &str) -> bool {
    if matches!(ext, "sky" | "ldef" | "lmap" | "cpvs" | "pvs" | "pvshist") {
        return true;
    }
    // `lt00`…`lt15` per-light records.
    ext.strip_prefix("lt")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether a normalized logical path feeds the simulation or its rules.
///
/// Unprefixed loose files (a stock install's executables, docs and
/// links) are outside the classified content set and never counted.
pub fn is_gameplay_path(logical: &str) -> bool {
    if let Some(rest) = logical.strip_prefix("city/") {
        let ext = rest.rsplit('.').next().unwrap_or(rest);
        return !city_ext_is_visual(ext);
    }
    GAMEPLAY_PREFIXES
        .iter()
        .any(|prefix| logical.starts_with(prefix))
}

/// The computed fingerprint plus the denominator it covered, so a
/// report can show how much content the hash represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameplayFingerprint {
    /// FNV-1a-64 over each included path, its length and its bytes.
    pub hash: u64,
    /// Number of resolved files hashed.
    pub files: usize,
    /// Total resolved bytes hashed.
    pub bytes: u64,
}

impl GameplayFingerprint {
    /// `fnv1a64:<hex>` presentation form, matching the catalog
    /// fingerprint's format.
    pub fn display(&self) -> String {
        fingerprint::display(self.hash)
    }
}

/// FNV-1a-64 over the resolved contents of every gameplay-relevant
/// logical path. Iterates the VFS's sorted listing, so the result is
/// deterministic for a given resolution map; each entry contributes
/// `path ‖ len ‖ bytes`, so a deletion, a retargeted override and a
/// byte-level edit all move the hash, while a cosmetic-only override
/// leaves it untouched.
///
/// A path that resolves but fails to read is an error rather than a
/// silent skip — a handshake must not run on a half-readable install.
pub fn gameplay(vfs: &Vfs) -> Result<GameplayFingerprint, AssetsError> {
    let mut out = GameplayFingerprint {
        hash: FNV_OFFSET_BASIS,
        files: 0,
        bytes: 0,
    };
    for logical in vfs.list() {
        if !is_gameplay_path(&logical) {
            continue;
        }
        let bytes = vfs.read_logical(&logical)?;
        out.hash = fnv(out.hash, &logical);
        out.hash = fnv(out.hash, (bytes.len() as u64).to_le_bytes());
        out.hash = fnv(out.hash, &bytes);
        out.files += 1;
        out.bytes += bytes.len() as u64;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    }

    #[test]
    fn gameplay_paths_classified() {
        for p in [
            "tune/vpbug.vehCarSim",
            "bound/vpbug.bnd",
            "geometry/vpbug.pkg",
            "race/sf/blitz0.aimap",
            "anim/man.mod",
            "players/foo.csv",
            "city/sf.psdl",
            "city/sf.bai",
            "city/props.csv",
            "city/sf.water",
        ] {
            assert!(is_gameplay_path(p), "{p} should be gameplay");
        }
        for p in [
            "texture/foo.tex",
            "aud/engine.wav",
            "jpg/bgframe.jpg",
            "city/amb_fa_l.ldef",
            "city/sf.cpvs",
            "city/sf.pvshist",
            "city/sf.lmap",
            "city/day.sky",
            "city/sf.lt07",
            "midtown2.exe",
            "readme.rtf",
        ] {
            assert!(!is_gameplay_path(p), "{p} should be cosmetic/ignored");
        }
    }

    #[test]
    fn cosmetic_only_change_does_not_move_the_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        for dir in [&a, &b] {
            write(dir, "tune/x.csv", b"tuning");
            write(dir, "city/sf.psdl", b"city");
            write(dir, "texture/sky.tex", b"old");
        }
        write(&b, "texture/sky.tex", b"new");
        write(&b, "city/sf.ldef", b"different lighting");

        let fa = gameplay(&vfs_of(&a)).unwrap();
        let fb = gameplay(&vfs_of(&b)).unwrap();
        assert_eq!(fa.hash, fb.hash, "cosmetic edits must not move it");
        assert_eq!(fa.files, 2);
        assert_eq!(fb.files, 2);
    }

    #[test]
    fn gameplay_changes_move_the_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let edited = tmp.path().join("edited");
        let deleted = tmp.path().join("deleted");
        for dir in [&base, &edited, &deleted] {
            write(dir, "tune/x.csv", b"tuning");
            write(dir, "bound/x.bnd", b"bounds");
        }
        write(&edited, "tune/x.csv", b"tuning+");
        fs::remove_file(deleted.join("bound/x.bnd")).unwrap();

        let f0 = gameplay(&vfs_of(&base)).unwrap();
        assert_ne!(gameplay(&vfs_of(&edited)).unwrap().hash, f0.hash);
        assert_ne!(gameplay(&vfs_of(&deleted)).unwrap().hash, f0.hash);
        assert_eq!(gameplay(&vfs_of(&base)).unwrap(), f0, "deterministic");
    }

    #[test]
    fn a_mod_override_moves_the_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let mods = tmp.path().join("mods");
        write(&base, "tune/x.csv", b"stock");
        write(&base, "texture/x.tex", b"stock-tex");
        let m = mods.join("m");
        write(&m, "mod.toml", b"[mod]\nid = \"m\"\n");
        write(&m, "tune/x.csv", b"modded");
        write(&m, "texture/x.tex", b"modded-tex");

        let stock = gameplay(&vfs_of(&base)).unwrap();
        let mut vfs = vfs_of(&base);
        vfs.mount_mods_dir(&mods, mm2_assets::priority::MOD)
            .unwrap();
        let modded = gameplay(&vfs).unwrap();
        assert_ne!(stock.hash, modded.hash);
        assert_eq!(modded.files, 1, "only the tune file is gameplay");
    }
}

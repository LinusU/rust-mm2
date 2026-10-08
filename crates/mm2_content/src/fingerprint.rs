//! Gameplay-relevant content fingerprint (F24-A; F29-AC05's split).
//!
//! [`gameplay`] hashes the *resolved bytes* of the logical paths that can
//! change the simulation or its rules — tuning, collision bounds, model
//! geometry (collision hulls, wheel origins), city data the world is
//! built from and every event record — so two peers fingerprint
//! identically only when their effective gameplay content is byte-equal.
//! Cosmetic families (`texture/`, `aud/`, menu art under `jpg/`, the
//! display-name records `tune/*.cinfo`, plus the visual-only city
//! records: skies, lighting and visibility sets) are
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
use mm2_assets::{AssetsError, DeclaredEffect, Vfs};

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
    if is_city_info(logical) {
        return false;
    }
    GAMEPLAY_PREFIXES
        .iter()
        .any(|prefix| logical.starts_with(prefix))
}

/// `tune/<city>.cinfo`: the localized city name and race names. The only
/// consumer is the menu's labels (`CityInfo`), so a translation mod must
/// not cost multiplayer compatibility or records. The file also carries
/// keys no consumer reads yet (`MustPlace`, `UnlockGroup`, …); the day
/// one is consumed, that key's family moves into the gameplay set.
fn is_city_info(logical: &str) -> bool {
    logical.starts_with("tune/") && logical.ends_with(".cinfo")
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

/// What one mounted mod does to the session, by the files it actually
/// wins (F29 req 5). Derived from the same [`is_gameplay_path`] split the
/// [`gameplay`] fingerprint hashes, so "cosmetic-only" means exactly "this
/// mod cannot move the fingerprint".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModReport {
    /// The manifest id.
    pub id: String,
    /// Logical paths this mod wins that feed the simulation or its rules.
    pub gameplay: usize,
    /// Logical paths this mod wins that the fingerprint ignores (textures,
    /// audio, menu art, sky/lighting, and unclassified loose files).
    pub cosmetic: usize,
    /// Paths this mod provides but another source wins — no effect.
    pub shadowed: usize,
    /// First gameplay path the mod wins, in sorted order.
    pub example: Option<String>,
    /// What the mod's manifest claims, if it claims anything. The verdict
    /// is [`is_cosmetic_only`](Self::is_cosmetic_only) either way.
    pub declared: Option<DeclaredEffect>,
}

impl ModReport {
    /// Whether the mod wins no gameplay path. A mod that wins nothing at
    /// all (empty, or fully shadowed) is cosmetic-only too: it changes
    /// nothing.
    pub fn is_cosmetic_only(&self) -> bool {
        self.gameplay == 0
    }

    /// Why the manifest's claim disagrees with the files the mod wins, or
    /// `None` when there is no claim or it holds. The claim never changes
    /// the verdict; this only tells the author their `effect` is wrong.
    pub fn contradiction(&self) -> Option<String> {
        match (self.declared?, self.is_cosmetic_only()) {
            (DeclaredEffect::Cosmetic, false) => Some(format!(
                "declares effect = \"cosmetic\" but wins {} gameplay path(s), first {}",
                self.gameplay,
                self.example.as_deref().unwrap_or("?"),
            )),
            (DeclaredEffect::Gameplay, true) => Some(format!(
                "declares effect = \"gameplay\" but wins no gameplay path ({} shadowed by other sources)",
                self.shadowed,
            )),
            _ => None,
        }
    }
}

/// One report per mounted mod, in mount order. Only winning files count:
/// a gameplay file a later mod replaces is credited to the later mod.
/// Unlike [`gameplay`] this reads no file contents — it is a path
/// classification, so a mod that rewrites a gameplay file with identical
/// bytes is still reported as gameplay (conservative).
pub fn mod_reports(vfs: &Vfs) -> Vec<ModReport> {
    let mut reports: Vec<ModReport> = vfs
        .mod_ids()
        .map(|id| ModReport {
            id: id.to_string(),
            gameplay: 0,
            cosmetic: 0,
            shadowed: 0,
            example: None,
            declared: vfs.declared_effect(id),
        })
        .collect();
    if reports.is_empty() {
        return reports;
    }
    let slot = |reports: &[ModReport], label: &str| reports.iter().position(|r| r.id == label);
    for logical in vfs.list() {
        let Some(label) = vfs.resolve(&logical).and_then(|r| r.source.label) else {
            continue;
        };
        let Some(i) = slot(&reports, &label) else {
            continue;
        };
        if is_gameplay_path(&logical) {
            reports[i].gameplay += 1;
            reports[i].example.get_or_insert(logical);
        } else {
            reports[i].cosmetic += 1;
        }
    }
    for ex in vfs.conflicts() {
        for loser in &ex.candidates[1..] {
            if let Some(i) = loser
                .source
                .label
                .as_deref()
                .and_then(|l| slot(&reports, l))
            {
                reports[i].shadowed += 1;
            }
        }
    }
    reports
}

/// Whether every mounted mod is cosmetic-only (vacuously true with no
/// mods). The session's record-eligibility gate reads this: a result
/// under a texture or soundtrack mod is comparable to stock, one under a
/// tuning or world mod is not.
pub fn mods_cosmetic_only(vfs: &Vfs) -> bool {
    mod_reports(vfs).iter().all(ModReport::is_cosmetic_only)
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
            // A backup copy is not the display-name record.
            "tune/sf.cinfo.bak",
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
            "tune/sf.cinfo",
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

    /// A mod's `(relative path, bytes)` files.
    type Files<'a> = &'a [(&'a str, &'a [u8])];

    /// Mount `mods` (id → files) over a stock tree of one tuning file, one
    /// texture and one city file.
    fn modded(tmp: &Path, mods: &[(&str, Files)]) -> Vfs {
        let base = tmp.join("base");
        write(&base, "tune/x.csv", b"stock");
        write(&base, "texture/x.tex", b"stock-tex");
        write(&base, "city/sf.psdl", b"city");
        let dir = tmp.join("mods");
        for (id, files) in mods {
            let m = dir.join(id);
            write(&m, "mod.toml", format!("[mod]\nid = \"{id}\"\n").as_bytes());
            for (rel, bytes) in *files {
                write(&m, rel, bytes);
            }
        }
        let mut vfs = vfs_of(&base);
        if !mods.is_empty() {
            vfs.mount_mods_dir(&dir, mm2_assets::priority::MOD).unwrap();
        }
        vfs
    }

    fn stock(tmp: &Path) -> GameplayFingerprint {
        gameplay(&modded(tmp, &[])).unwrap()
    }

    #[test]
    fn no_mods_report_nothing_and_count_as_cosmetic_only() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = modded(tmp.path(), &[]);
        assert!(mod_reports(&vfs).is_empty());
        assert!(mods_cosmetic_only(&vfs));
    }

    #[test]
    fn a_texture_audio_and_sky_mod_is_cosmetic_only_and_leaves_the_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = modded(
            tmp.path(),
            &[(
                "paint",
                &[
                    ("texture/x.tex", b"new-tex"),
                    ("aud/horn.wav", b"horn"),
                    ("city/day.sky", b"sky"),
                    ("notes.txt", b"unclassified"),
                ],
            )],
        );
        let r = &mod_reports(&vfs)[0];
        assert_eq!(
            (r.id.as_str(), r.gameplay, r.cosmetic, r.shadowed),
            ("paint", 0, 4, 0)
        );
        assert!(r.example.is_none());
        assert!(mods_cosmetic_only(&vfs));
        assert_eq!(
            gameplay(&vfs).unwrap(),
            stock(tmp.path()),
            "cosmetic-only moves nothing"
        );
    }

    #[test]
    fn a_tuning_or_world_mod_is_gameplay_and_moves_the_fingerprint() {
        for (rel, what) in [
            ("tune/x.csv", "replaces"),
            ("tune/new.csv", "adds"),
            ("city/sf.bai", "adds"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let vfs = modded(
                tmp.path(),
                &[("tuning", &[("texture/x.tex", b"t"), (rel, b"edited")])],
            );
            let r = &mod_reports(&vfs)[0];
            assert_eq!((r.gameplay, r.cosmetic), (1, 1), "{what} {rel}");
            assert_eq!(r.example.as_deref(), Some(rel));
            assert!(!r.is_cosmetic_only() && !mods_cosmetic_only(&vfs));
            assert_ne!(
                gameplay(&vfs).unwrap().hash,
                stock(tmp.path()).hash,
                "{what} {rel}"
            );
        }
    }

    #[test]
    fn only_winning_files_count_and_a_later_mod_takes_the_credit() {
        let tmp = tempfile::tempdir().unwrap();
        // Mounted in sorted directory order: `a-tuning` first, `b-paint` wins ties.
        let vfs = modded(
            tmp.path(),
            &[
                (
                    "a-tuning",
                    &[("tune/x.csv", b"first"), ("texture/x.tex", b"first-tex")],
                ),
                (
                    "b-paint",
                    &[("tune/x.csv", b"second"), ("texture/x.tex", b"second-tex")],
                ),
            ],
        );
        let reports = mod_reports(&vfs);
        let ids: Vec<_> = reports.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a-tuning", "b-paint"], "mount order");
        // `b-paint` (despite the name) wins both files; `a-tuning` is wholly shadowed.
        assert_eq!(
            (
                reports[0].gameplay,
                reports[0].cosmetic,
                reports[0].shadowed
            ),
            (0, 0, 2)
        );
        assert_eq!(
            (
                reports[1].gameplay,
                reports[1].cosmetic,
                reports[1].shadowed
            ),
            (1, 1, 0)
        );
        assert!(!mods_cosmetic_only(&vfs));
    }

    #[test]
    fn one_gameplay_mod_among_cosmetic_ones_makes_the_set_gameplay() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = modded(
            tmp.path(),
            &[
                ("a-skin", &[("texture/x.tex", b"skin")]),
                ("b-bounds", &[("bound/x.bnd", b"hull")]),
            ],
        );
        let reports = mod_reports(&vfs);
        assert!(reports[0].is_cosmetic_only());
        assert!(!reports[1].is_cosmetic_only());
        assert!(!mods_cosmetic_only(&vfs));
    }

    #[test]
    fn an_empty_mod_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = modded(tmp.path(), &[("empty", &[])]);
        let r = &mod_reports(&vfs)[0];
        assert_eq!((r.gameplay, r.cosmetic, r.shadowed), (0, 0, 0));
        assert!(mods_cosmetic_only(&vfs));
    }

    /// Mount one mod per `(id, declared effect, files)` over the stock tree.
    fn declared(tmp: &Path, mods: &[(&str, Option<&str>, Files)]) -> Vfs {
        let base = tmp.join("base");
        write(&base, "tune/x.csv", b"stock");
        let dir = tmp.join("mods");
        for (id, effect, files) in mods {
            let m = dir.join(id);
            let claim = effect.map_or(String::new(), |e| format!("effect = \"{e}\"\n"));
            write(
                &m,
                "mod.toml",
                format!("[mod]\nid = \"{id}\"\n{claim}").as_bytes(),
            );
            for (rel, bytes) in *files {
                write(&m, rel, bytes);
            }
        }
        let mut vfs = vfs_of(&base);
        vfs.mount_mods_dir(&dir, mm2_assets::priority::MOD).unwrap();
        vfs
    }

    #[test]
    fn a_declared_effect_is_checked_against_the_files_but_never_replaces_them() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = declared(
            tmp.path(),
            &[
                (
                    "a-honest-skin",
                    Some("cosmetic"),
                    &[("texture/x.tex", b"t")],
                ),
                ("b-honest-tuning", Some("gameplay"), &[("tune/x.csv", b"t")]),
                ("c-silent", None, &[("tune/y.csv", b"t")]),
                ("d-lying-tuning", Some("cosmetic"), &[("tune/z.csv", b"t")]),
                ("e-lying-skin", Some("gameplay"), &[("texture/y.tex", b"t")]),
            ],
        );
        let reports = mod_reports(&vfs);
        let claims: Vec<_> = reports.iter().map(|r| r.declared).collect();
        assert_eq!(
            claims,
            [
                Some(DeclaredEffect::Cosmetic),
                Some(DeclaredEffect::Gameplay),
                None,
                Some(DeclaredEffect::Cosmetic),
                Some(DeclaredEffect::Gameplay),
            ]
        );
        let contradictions: Vec<_> = reports.iter().map(ModReport::contradiction).collect();
        assert_eq!(
            contradictions[..3],
            [None, None, None],
            "true or absent claims"
        );
        assert!(
            contradictions[3]
                .as_ref()
                .is_some_and(|c| c.contains("tune/z.csv"))
        );
        assert!(
            contradictions[4]
                .as_ref()
                .is_some_and(|c| c.contains("no gameplay path"))
        );
        // The lie changes nothing: the verdict is the files'.
        assert!(
            !reports[3].is_cosmetic_only(),
            "a cosmetic claim cannot launder tuning"
        );
        assert!(
            reports[4].is_cosmetic_only(),
            "a gameplay claim does not make a skin gameplay"
        );
        assert!(!mods_cosmetic_only(&vfs));
    }

    #[test]
    fn a_gameplay_claim_on_a_fully_shadowed_mod_is_reported_with_its_shadowing() {
        let tmp = tempfile::tempdir().unwrap();
        let vfs = declared(
            tmp.path(),
            &[
                ("a-early", Some("gameplay"), &[("tune/x.csv", b"early")]),
                ("b-late", None, &[("tune/x.csv", b"late")]),
            ],
        );
        let early = &mod_reports(&vfs)[0];
        assert_eq!((early.gameplay, early.shadowed), (0, 1));
        let why = early.contradiction().unwrap();
        assert!(why.contains("1 shadowed"), "{why}");
    }
}

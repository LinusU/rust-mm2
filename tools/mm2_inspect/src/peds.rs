//! `mm2-inspect peds` — pedestrian rig audit (F19-A.1).
//!
//! Censuses every `anim/**` file through the VFS and deep-parses the
//! definition side of the pedestrian corpus:
//!
//! - `pedmodel_*.skel` bone hierarchies ([`PedSkel`]),
//! - `pedmodel_*.csv` animation state models ([`PedStates`]),
//! - `pedmodel_*.remap` bone remaps ([`PedRemap`]),
//! - `pedmodel_*.rays` records + integer grids ([`PedRays`]),
//! - `pedanim_*.anim` binary clips ([`PedAnim`]),
//! - `pedmodel_*.shaders` via the shared PKG shader grammar
//!   ([`PkgShaders`]),
//! - `pedmodel_*.mod` ASCII skinned meshes ([`PedMod`]) — both the
//!   flat adjunct-list dialect and the packet dialect.
//!
//! Cross-checks: skeleton bone count vs `.rays` row count, clip
//! channel width and `.mod` matrix count, `.mod` material count vs
//! `.shaders` per-paint-job count, state-model clip references vs
//! discovered `pedanim_*` files, authored frame windows vs clip
//! length, and the expected archetype roster ([`EXPECTED_PEDS`]).
//!
//! `--strict` exits nonzero on parse failures, issues and missing
//! expected archetypes. Authored quirks — the `pedmodel_wolf` partial
//! archetype, the ASCII scene lists masquerading as content, and
//! off-by-one frame windows — are reported but do not fail strict.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EXPECTED_PEDS, PED_REQUIRED_EXTS};
use mm2_formats::ped::{PedAnim, PedMod, PedModDialect, PedRays, PedRemap, PedSkel, PedStates};
use mm2_formats::pkg::PkgShaders;

/// Per-archetype audit row.
#[derive(Debug, Default)]
pub struct Archetype {
    /// `pedmodel_*` stem.
    pub stem: String,
    /// Whether the stem is in [`EXPECTED_PEDS`].
    pub expected: bool,
    /// Parsed bone count (skeleton present and valid).
    pub bones: Option<usize>,
    /// `NumBones` header value.
    pub declared_bones: Option<i64>,
    /// Parsed state count.
    pub states: Option<usize>,
    /// `.rays` row count (parsed records, not the declared field).
    pub rays: Option<usize>,
    /// `.rays` integer-grid row count.
    pub grid_rows: Option<usize>,
    /// `.remap` index count.
    pub remap: Option<usize>,
    /// `.shaders` paint-job count.
    pub paint_jobs: Option<u32>,
    /// `.shaders` materials per paint job.
    pub shaders_per_paint_job: Option<u32>,
    /// `.mod` mesh present.
    pub has_mod: bool,
    /// `.mod` vertex count (parsed).
    pub mod_verts: Option<usize>,
    /// `.mod` primitive count (parsed).
    pub mod_primitives: Option<usize>,
    /// `.mod` material-group count.
    pub mod_materials: Option<usize>,
    /// `.mod` geometry dialect.
    pub mod_dialect: Option<PedModDialect>,
    /// Required companion extensions not discovered.
    pub missing: Vec<&'static str>,
}

/// Result of [`audit`].
#[derive(Debug, Default)]
pub struct PedsReport {
    /// `anim/` files examined (content records).
    pub files: usize,
    /// Extensionless archive index/directory entries.
    pub dirs: usize,
    /// `pedanim_*.anim` clips that parsed.
    pub clips: usize,
    /// Total frames across parsed clips.
    pub clip_frames: u64,
    /// Window frames the [`mm2_game::ped::PedRig`] sampler evaluated
    /// cleanly (first/mid/clamped-last per authored state window).
    pub poses_sampled: u64,
    /// Per-archetype rows, sorted by stem.
    pub archetypes: Vec<Archetype>,
    /// Parsed clips no state model references.
    pub unreferenced: Vec<String>,
    /// [`EXPECTED_PEDS`] stems with no files at all.
    pub missing_expected: Vec<String>,
    /// Non-rig records under `anim/` by extension/kind.
    pub extras: BTreeMap<String, usize>,
    /// Authored oddities that are not defects.
    pub quirks: Vec<String>,
    /// Cross-check and validation issues.
    pub issues: Vec<String>,
    /// Parse/read failures `(path, error)`.
    pub failures: Vec<(String, String)>,
}

/// Census and cross-check `anim/` through the production VFS.
pub fn audit(vfs: &Vfs) -> PedsReport {
    let mut r = PedsReport::default();
    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.starts_with("anim/"))
        .collect();
    logicals.sort();

    // Pedmodel members (stem -> ext -> bytes) and parsed clips.
    let mut members: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    let mut clips: BTreeMap<String, PedAnim> = BTreeMap::new();
    // Every pedanim_* stem discovered, parseable or not.
    let mut clip_stems: BTreeSet<String> = BTreeSet::new();

    for logical in &logicals {
        let rest = &logical["anim/".len()..];
        if rest.contains('/') {
            *r.extras.entry("nested".into()).or_default() += 1;
            continue;
        }
        let bytes = match vfs.read_logical(logical) {
            Ok(b) => b,
            Err(e) => {
                r.failures.push((logical.clone(), e.to_string()));
                continue;
            }
        };
        if bytes.is_empty() {
            r.dirs += 1; // zero-length archive entries occur in retail
            continue;
        }
        r.files += 1;
        match rest.rsplit_once('.') {
            None => r.quirks.push(format!(
                "{logical}: extensionless file ({} bytes), not a rig member",
                bytes.len()
            )),
            Some((stem, ext)) if stem.starts_with("pedmodel_") => {
                members
                    .entry(stem.to_string())
                    .or_default()
                    .insert(ext.to_string(), bytes);
            }
            // Any top-level `.anim` is a clip candidate — state models
            // name clips by stem, not by a fixed prefix.
            Some((stem, "anim")) => {
                clip_stems.insert(stem.to_string());
                match PedAnim::parse(&bytes) {
                    Ok(a) => {
                        r.clips += 1;
                        r.clip_frames += a.frames as u64;
                        for i in a.validate() {
                            r.issues.push(format!("{logical}: {i}"));
                        }
                        clips.insert(stem.to_string(), a);
                    }
                    Err(e) if looks_like_text(&bytes) => r
                        .quirks
                        .push(format!("{logical}: ASCII text, not a binary clip ({e})")),
                    Err(e) => r.failures.push((logical.clone(), e.to_string())),
                }
            }
            Some((_, ext)) => *r.extras.entry(ext.to_string()).or_default() += 1,
        }
    }

    // Deep-parse each archetype's members.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut skels: BTreeMap<String, PedSkel> = BTreeMap::new();
    let mut state_models: BTreeMap<String, PedStates> = BTreeMap::new();
    for (stem, files) in &members {
        let expected = EXPECTED_PEDS.contains(&stem.as_str());
        let mut a = Archetype {
            stem: stem.clone(),
            expected,
            ..Archetype::default()
        };
        // `.csv` is added to the inventory's required set: the state
        // model is the rig's only behavioral record. `.remap` stays
        // optional (retail ships it on one archetype only).
        for ext in PED_REQUIRED_EXTS.iter().copied().chain(["csv"]) {
            if !files.contains_key(ext) {
                a.missing.push(ext);
            }
        }
        if !a.missing.is_empty() {
            let msg = format!(
                "anim/{stem}: partial archetype, missing {}",
                a.missing
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if expected {
                r.issues.push(msg);
            } else {
                // e.g. pedmodel_wolf ships a lone .skel on retail.
                r.quirks.push(msg);
            }
        }

        if let Some(b) = files.get("skel") {
            match std::str::from_utf8(b)
                .map_err(|e| format!("not UTF-8 text: {e}"))
                .and_then(|t| PedSkel::parse(t).map_err(|e| e.to_string()))
            {
                Ok(s) => {
                    a.declared_bones = Some(s.declared_bones);
                    a.bones = Some(s.bone_count());
                    for d in &s.diagnostics {
                        r.issues.push(format!("anim/{stem}.skel: {d}"));
                    }
                    for d in s.validate() {
                        r.issues.push(format!("anim/{stem}.skel: {d}"));
                    }
                    skels.insert(stem.clone(), s);
                }
                Err(e) => r.failures.push((format!("anim/{stem}.skel"), e)),
            }
        }
        if let Some(b) = files.get("csv") {
            match std::str::from_utf8(b)
                .map_err(|e| e.to_string())
                .and_then(|t| PedStates::parse(t).map_err(|e| e.to_string()))
            {
                Ok(s) => {
                    a.states = Some(s.states.len());
                    for d in &s.diagnostics {
                        r.issues.push(format!("anim/{stem}.csv: {d}"));
                    }
                    for d in s.validate() {
                        r.issues.push(format!("anim/{stem}.csv: {d}"));
                    }
                    state_models.insert(stem.clone(), s);
                }
                Err(e) => r.failures.push((format!("anim/{stem}.csv"), e)),
            }
        }
        if let Some(b) = files.get("rays") {
            match std::str::from_utf8(b)
                .map_err(|e| e.to_string())
                .and_then(|t| PedRays::parse(t).map_err(|e| e.to_string()))
            {
                Ok(s) => {
                    a.rays = Some(s.rays.len());
                    a.grid_rows = Some(s.grid.len());
                    for d in &s.diagnostics {
                        r.issues.push(format!("anim/{stem}.rays: {d}"));
                    }
                    for d in s.validate() {
                        r.issues.push(format!("anim/{stem}.rays: {d}"));
                    }
                    if let Some(bones) = a.bones
                        && s.declared != bones as i64
                    {
                        r.issues.push(format!(
                            "anim/{stem}.rays: declared {} rows but the skeleton has {bones} bones",
                            s.declared
                        ));
                    }
                }
                Err(e) => r.failures.push((format!("anim/{stem}.rays"), e)),
            }
        }
        if let Some(b) = files.get("remap") {
            match std::str::from_utf8(b)
                .map_err(|e| e.to_string())
                .and_then(|t| PedRemap::parse(t).map_err(|e| e.to_string()))
            {
                Ok(s) => {
                    a.remap = Some(s.indices.len());
                    for d in &s.diagnostics {
                        r.issues.push(format!("anim/{stem}.remap: {d}"));
                    }
                    for d in s.validate() {
                        r.issues.push(format!("anim/{stem}.remap: {d}"));
                    }
                }
                Err(e) => r.failures.push((format!("anim/{stem}.remap"), e)),
            }
        }
        if let Some(b) = files.get("shaders") {
            match PkgShaders::parse(b) {
                Ok(s) => {
                    a.paint_jobs = Some(s.paint_jobs);
                    a.shaders_per_paint_job = Some(s.shaders_per_paint_job);
                }
                Err(e) => r
                    .failures
                    .push((format!("anim/{stem}.shaders"), e.to_string())),
            }
        }
        if let Some(b) = files.get("mod") {
            match std::str::from_utf8(b)
                .map_err(|e| format!("not UTF-8 text: {e}"))
                .and_then(|t| PedMod::parse(t).map_err(|e| e.to_string()))
            {
                Ok(m) => {
                    a.mod_verts = Some(m.verts.len());
                    a.mod_primitives = Some(m.primitive_count());
                    a.mod_materials = Some(m.materials.len());
                    a.mod_dialect = Some(m.dialect());
                    for d in &m.diagnostics {
                        r.issues.push(format!("anim/{stem}.mod: {d}"));
                    }
                    for d in m.validate() {
                        r.issues.push(format!("anim/{stem}.mod: {d}"));
                    }
                    if let (Some(mtx), Some(bones)) = (m.declared.matrices, a.bones)
                        && mtx != bones as i64
                    {
                        r.issues.push(format!(
                            "anim/{stem}.mod: declares {mtx} matrices against a {bones}-bone skeleton"
                        ));
                    }
                    // R3: the material-group order must match the
                    // `.shaders` per-paint-job order — so the counts
                    // must agree.
                    if let Some(per) = a.shaders_per_paint_job
                        && per as usize != m.materials.len()
                    {
                        r.issues.push(format!(
                            "anim/{stem}.mod: {} material groups against {} shaders per paint job",
                            m.materials.len(),
                            per
                        ));
                    }
                }
                Err(e) => r.failures.push((format!("anim/{stem}.mod"), e)),
            }
        }
        a.has_mod = files.contains_key("mod");
        r.archetypes.push(a);
    }

    // State model → clip cross-checks.
    for (stem, model) in &state_models {
        let bones = skels.get(stem).map(|s| s.bone_count());
        // F19-A.3: exercise the runtime sampler over authored windows.
        let rig = skels.get(stem).map(mm2_game::ped::PedRig::from_skel);
        if let Some(Err(e)) = &rig {
            r.issues.push(format!("anim/{stem}.skel: {e}"));
        }
        for st in &model.states {
            let logical = format!("anim/{}.anim", st.anim);
            let Some(clip) = clips.get(&st.anim) else {
                if clip_stems.contains(&st.anim) {
                    // Present but unparseable — already reported.
                    referenced.insert(st.anim.clone());
                } else {
                    r.issues.push(format!(
                        "anim/{stem}.csv line {}: state {:?} references missing clip {logical}",
                        st.line, st.name
                    ));
                }
                continue;
            };
            referenced.insert(st.anim.clone());
            if st.first_frame < 1 || st.first_frame > clip.frames as i64 {
                r.issues.push(format!(
                    "anim/{stem}.csv line {}: state {:?} first frame {} outside 1..={}",
                    st.line, st.name, st.first_frame, clip.frames
                ));
            }
            if st.last_frame > clip.frames as i64 + 1 {
                r.issues.push(format!(
                    "anim/{stem}.csv line {}: state {:?} last frame {} exceeds clip frames {}",
                    st.line, st.name, st.last_frame, clip.frames
                ));
            } else if st.last_frame == clip.frames as i64 + 1 {
                // Authored on retail: the window's end is one past the
                // clip length on many transition states.
                r.quirks.push(format!(
                    "anim/{stem}.csv line {}: state {:?} window end {} = clip frames {} + 1",
                    st.line, st.name, st.last_frame, clip.frames
                ));
            }
            if let Some(bones) = bones
                && clip.floats_per_frame != (bones as u32 + 1) * 3
            {
                r.issues.push(format!(
                    "{}: {} floats/frame against a {bones}-bone rig (expected {})",
                    logical,
                    clip.floats_per_frame,
                    (bones + 1) * 3
                ));
            }
            // Sample the window's endpoints and midpoint (0-based,
            // clamped against the actual clip length — this also covers
            // the authored `frames + 1` overshoot rows).
            if let Some(Ok(rig)) = &rig {
                let hi = clip.frames as i64 - 1;
                let first = (st.first_frame - 1).clamp(0, hi);
                let last = (st.last_frame - 1).clamp(first, hi);
                for f in [first, (first + last) / 2, last] {
                    match rig.sample(clip, f as f32) {
                        Ok(p) if p.is_finite() => r.poses_sampled += 1,
                        Ok(_) => r
                            .issues
                            .push(format!("{logical}: non-finite pose at frame {}", f + 1)),
                        Err(e) => r.issues.push(format!("{logical}: frame {}: {e}", f + 1)),
                    }
                }
            }
        }
    }

    r.unreferenced = clips
        .keys()
        .filter(|s| !referenced.contains(*s))
        .cloned()
        .collect();
    for want in EXPECTED_PEDS {
        if !members.contains_key(*want) {
            r.missing_expected.push(format!("anim/{want}.*"));
        }
    }
    r
}

/// True when every byte is printable ASCII or whitespace — retail ships
/// two scratch scene lists (`pedanim_manantrnch.anim`, the extensionless
/// `anim/pedmodel_woman`) that are plain text, not binary clips.
fn looks_like_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|b| b.is_ascii_graphic() || b.is_ascii_whitespace())
}

/// Print the report.
pub fn print_report(r: &PedsReport) {
    println!(
        "anim/ census: {} files, {} index/dir entries",
        r.files, r.dirs
    );
    println!(
        "clips: {} parsed ({} frames total), {} unreferenced, {} pose samples",
        r.clips,
        r.clip_frames,
        r.unreferenced.len(),
        r.poses_sampled
    );
    println!("\narchetypes:");
    for a in &r.archetypes {
        let tag = if a.expected { "" } else { " (extra)" };
        let mod_desc = match (a.has_mod, a.mod_verts) {
            (true, Some(v)) => format!(
                "{}v/{}p/{}m ({})",
                v,
                a.mod_primitives.unwrap_or(0),
                a.mod_materials.unwrap_or(0),
                match a.mod_dialect {
                    Some(PedModDialect::Packets) => "packets",
                    Some(PedModDialect::Flat) => "flat",
                    _ => "mixed",
                }
            ),
            (true, None) => "present (failed)".to_string(),
            _ => "absent".to_string(),
        };
        println!(
            "  {}{}: {} bones (declared {:?}), {} states, {} rays ({}-row grid), \
             remap {}, {} paint jobs x {} shaders, .mod {}{}",
            a.stem,
            tag,
            a.bones.map(|b| b.to_string()).unwrap_or("-".into()),
            a.declared_bones
                .map(|d| d.to_string())
                .unwrap_or("-".into()),
            a.states.map(|s| s.to_string()).unwrap_or("-".into()),
            a.rays.map(|n| n.to_string()).unwrap_or("-".into()),
            a.grid_rows.map(|n| n.to_string()).unwrap_or("-".into()),
            a.remap.map(|n| n.to_string()).unwrap_or("absent".into()),
            a.paint_jobs.map(|n| n.to_string()).unwrap_or("-".into()),
            a.shaders_per_paint_job
                .map(|n| n.to_string())
                .unwrap_or("-".into()),
            mod_desc,
            if a.missing.is_empty() {
                String::new()
            } else {
                format!(
                    ", MISSING {}",
                    a.missing
                        .iter()
                        .map(|e| format!(".{e}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            },
        );
    }
    if !r.unreferenced.is_empty() {
        println!("\nunreferenced clips ({}):", r.unreferenced.len());
        for s in &r.unreferenced {
            println!("  anim/{s}.anim");
        }
    }
    if !r.extras.is_empty() {
        println!("\nnon-rig records:");
        for (k, n) in &r.extras {
            println!("  {k}: {n}");
        }
    }
    for (header, list) in [
        ("quirks", &r.quirks),
        ("issues", &r.issues),
        ("missing expected", &r.missing_expected),
    ] {
        if !list.is_empty() {
            println!("\n{header}:");
            for item in list {
                println!("  {item}");
            }
        }
    }
    if !r.failures.is_empty() {
        println!("\nfailures:");
        for (path, err) in &r.failures {
            println!("  {path}: {err}");
        }
    }
}

/// `mm2-inspect peds <dir> [--strict]`.
pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = crate::build_vfs(dir, mods)?;
    let report = audit(&vfs);
    if report.files == 0 && report.dirs == 0 {
        return Err("pedestrian audit: no anim/ files discovered".into());
    }
    print_report(&report);
    if strict
        && !(report.failures.is_empty()
            && report.issues.is_empty()
            && report.missing_expected.is_empty())
    {
        return Err(format!(
            "strict pedestrian audit: {} failures, {} issues, {} missing expected",
            report.failures.len(),
            report.issues.len(),
            report.missing_expected.len()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_assets::InstallMount;

    fn write(dir: &Path, logical: &str, bytes: &[u8]) {
        let p = dir.join(logical);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        mm2_assets::mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
        vfs
    }

    const SKEL: &str = "NumBones 3\nbone root {\n\toffset 0 1 0\n\tbone a {\n\t\toffset 0 0 0\n\t}\n\tbone b {\n\t\toffset 0 0 0\n\t}\n}\n";
    const CSV: &str = "# header\nSTAND,xstand,1,2,0,0,0,0,STAND\nWALK,xwalk,1,3,0,1,0,0,WALK\n";
    const RAYS: &str = "3\n0 0 0 1 2\n0 0 0 0 0\n0 0 0 0 0\n1 2 3\n";

    fn clip(frames: u32, fpf: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&frames.to_le_bytes());
        b.extend_from_slice(&fpf.to_le_bytes());
        b.extend_from_slice(&0f32.to_le_bytes());
        b.push(1);
        for _ in 0..frames * fpf {
            b.extend_from_slice(&0f32.to_le_bytes());
        }
        b
    }

    fn shaders() -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(&1u32.to_le_bytes()); // 1 paint job, float
        s.extend_from_slice(&2u32.to_le_bytes()); // 2 shaders
        for _ in 0..2 {
            s.push(0);
            for c in [0.5f32; 16] {
                s.extend_from_slice(&c.to_le_bytes());
            }
            s.extend_from_slice(&4f32.to_le_bytes());
        }
        s
    }

    /// A minimal packet-dialect `.mod`: 3 verts/3 normals, 2 materials
    /// (matching the 2-shader fixture above), 2 packets, 3 matrices.
    const MOD: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 3
primitives: 2
matrices: 3

v	0.0	0.0	0.0
v	1.0	0.0	0.0
v	0.0	1.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl Test1:SKIN {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl Test1:HAIR {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

packet 2 1 2 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	1
	tri	0	1	0
	mtx 0 1
}

packet 1 1 1 {
	adj	2	2	0	0	0	0
	tri	0	0	0
	mtx 2
}

mtxv 1 1 1
mtxn 1 1 1
";

    /// A complete synthetic archetype: 3 bones, fpf = (3+1)*3 = 12.
    fn write_arch(dir: &Path, stem: &str) {
        write(dir, &format!("anim/{stem}.skel"), SKEL.as_bytes());
        write(dir, &format!("anim/{stem}.csv"), CSV.as_bytes());
        write(dir, &format!("anim/{stem}.rays"), RAYS.as_bytes());
        write(dir, &format!("anim/{stem}.shaders"), &shaders());
        write(dir, &format!("anim/{stem}.mod"), MOD.as_bytes());
        write(dir, "anim/xstand.anim", &clip(2, 12));
        write(dir, "anim/xwalk.anim", &clip(3, 12));
    }

    #[test]
    fn audit_a_complete_archetype() {
        let tmp = tempfile::tempdir().unwrap();
        write_arch(tmp.path(), "pedmodel_man");
        // The other expected archetypes are absent — reported, not fatal here.
        let r = audit(&vfs_of(tmp.path()));
        assert_eq!(r.clips, 2);
        assert_eq!(r.archetypes.len(), 1);
        let a = &r.archetypes[0];
        assert_eq!(a.bones, Some(3));
        assert_eq!(a.states, Some(2));
        assert_eq!(a.rays, Some(3));
        assert_eq!(a.paint_jobs, Some(1));
        assert!(a.has_mod);
        assert!(a.missing.is_empty());
        assert!(r.issues.is_empty());
        assert!(r.failures.is_empty());
        assert_eq!(r.poses_sampled, 6); // 2 states × 3 sampled frames
        assert_eq!(r.missing_expected.len(), EXPECTED_PEDS.len() - 1);
    }

    #[test]
    fn audit_reports_mod_and_flags_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write_arch(d, "pedmodel_man");
        let r = audit(&vfs_of(d));
        let a = &r.archetypes[0];
        assert_eq!(a.mod_verts, Some(3));
        assert_eq!(a.mod_primitives, Some(2));
        assert_eq!(a.mod_materials, Some(2));
        assert_eq!(a.mod_dialect, Some(PedModDialect::Packets));
        assert!(r.issues.is_empty(), "{:?}", r.issues);

        // Skeleton/matrix disagreement and a material/shader-count gap.
        write(
            d,
            "anim/pedmodel_man.mod",
            MOD.replace("matrices: 3", "matrices: 9").as_bytes(),
        );
        let r = audit(&vfs_of(d));
        assert!(
            r.issues
                .iter()
                .any(|i| i.contains("9 matrices") && i.contains("3-bone"))
        );

        // An unparseable mesh is a failure.
        write(d, "anim/pedmodel_man.mod", b"\xFF\xFE not text");
        let r = audit(&vfs_of(d));
        assert!(r.failures.iter().any(|(p, _)| p.ends_with(".mod")));

        // Material groups ≠ shaders-per-paint-job is an issue.
        write(
            d,
            "anim/pedmodel_man.mod",
            MOD.replace("materials: 2", "materials: 1")
                .replace(
                    "mtl Test1:HAIR {\n\tpackets:\t1\n\tprimitives:\t1\n\ttextures:\t0\n\tillum: diffuse\n\tambient:\t0.1 0.0 0.0\n\tdiffuse:\t0.4 0.1 0.1\n\tspecular:\t0.6 0.4 0.4\n}\n\n",
                    "",
                )
                .as_bytes(),
        );
        let r = audit(&vfs_of(d));
        assert!(
            r.issues
                .iter()
                .any(|i| i.contains("material groups") && i.contains("shaders"))
        );
    }

    #[test]
    fn audit_flags_broken_and_unreferenced_content() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write_arch(d, "pedmodel_man");
        // State references a clip that does not exist.
        write(
            d,
            "anim/pedmodel_man.csv",
            b"S,xstand,1,2,0,0,0,0,S\nT,xmissing,1,9,0,0,0,0,T\n",
        );
        // Off-by-one authored window is a quirk, not an issue.
        write(d, "anim/pedmodel_man.csv", b"S,xstand,1,3,0,0,0,0,S\n");
        // An orphaned clip and an ASCII misfit.
        write(d, "anim/pedanim_orphan.anim", &clip(1, 12));
        write(d, "anim/pedanim_scene.anim", b"4\n\"Sun\" 0 1 2\n");
        // A nested non-content record and a dev script.
        write(d, "anim/cvs/entries", b"/x/1.1//\n");
        write(d, "anim/grog.bat", b"echo off\n");
        // A partial extra archetype (wolf shape).
        write(d, "anim/pedmodel_wolf.skel", SKEL.as_bytes());

        let r = audit(&vfs_of(d));
        // The second csv write wins: one state referencing xstand with
        // last frame 3 == frames+1 → quirk.
        assert!(r.quirks.iter().any(|q| q.contains("+ 1")));
        assert!(r.quirks.iter().any(|q| q.contains("pedanim_scene")));
        assert!(r.quirks.iter().any(|q| q.contains("pedmodel_wolf")));
        assert!(r.unreferenced.contains(&"pedanim_orphan".to_string()));
        assert_eq!(r.extras["nested"], 1);
        assert_eq!(r.extras["bat"], 1);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn audit_flags_missing_clip_and_channel_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write_arch(d, "pedmodel_man");
        write(
            d,
            "anim/pedmodel_man.csv",
            b"S,xstand,1,2,0,0,0,0,S\nT,xgone,1,9,0,0,0,0,T\nW,xwalk,1,99,0,0,0,0,W\n",
        );
        // Clip with wrong channel width for the 3-bone rig.
        write(d, "anim/xwalk.anim", &clip(3, 15));
        // A NaN channel value must surface as an issue, not a panic.
        let mut bad = clip(2, 12);
        bad[17..21].copy_from_slice(&f32::NAN.to_le_bytes());
        write(d, "anim/xstand.anim", &bad);
        let r = audit(&vfs_of(d));
        assert!(r.issues.iter().any(|i| i.contains("missing clip")));
        assert!(r.issues.iter().any(|i| i.contains("exceeds clip frames")));
        assert!(
            r.issues
                .iter()
                .any(|i| i.contains("floats/frame") && i.contains("xwalk"))
        );
        assert!(r.issues.iter().any(|i| i.contains("non-finite pose")));
    }

    #[test]
    fn audit_flags_rays_skel_mismatch_and_parse_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write_arch(d, "pedmodel_man");
        // rays declares 5 rows against a 3-bone skeleton.
        write(d, "anim/pedmodel_man.rays", b"5\n0 0 0 1 2\n");
        // A corrupt clip (truncated mid-samples).
        write(d, "anim/xwalk.anim", &clip(3, 12)[..40]);
        let r = audit(&vfs_of(d));
        assert!(
            r.issues
                .iter()
                .any(|i| i.contains("rays") && i.contains("3 bones"))
        );
        assert!(r.failures.iter().any(|(p, _)| p.contains("xwalk")));
    }
}

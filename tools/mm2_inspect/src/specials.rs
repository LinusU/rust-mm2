//! `mm2-inspect specials` — city special-content coverage (F28-A,
//! AC01/AC06).
//!
//! The moving and interactive things a generic static city and ambient
//! car traffic do not cover, inventoried per city *from the installation*
//! rather than from a hard-coded list of landmarks:
//!
//! - the per-city object pathsets (`race/<city>/<city>_<object>[_<event
//!   stem>].pathset` for `bridge`, `sailboat`, `ferry`, `train`,
//!   `parkedcar`) — every file found, parsed with the production
//!   parser, its model references resolved the way the managers resolve
//!   them, and whether any catalogued event can select it;
//! - `city/<city>.water` — the water/recovery room record;
//! - the cable car (`va_cablecar_f`): the model assets it ships, the
//!   rail curves the city's `.bai` authors, and the executable evidence
//!   that the original creates cable cars. No runtime consumes it yet,
//!   so it is reported **unresolved** rather than counted.
//!
//! The expected default files come from
//! [`mm2_content::EXPECTED_SPECIAL_PATHSETS`]. A city that lacks one
//! (San Francisco's Underground) is authored that way and is listed as
//! absent, never invented. `--strict` fails on a missing expected file,
//! a parse failure, a model that resolves neither by name nor through
//! the family default, or an unreadable water/BAI record; unresolved
//! actors are listed (and counted) but are an open implementation item,
//! not a data failure.

use std::collections::BTreeSet;
use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{EXPECTED_SPECIAL_PATHSETS, EventCatalog, race_cities};
use mm2_formats::bai::Bai;
use mm2_formats::pathset::Pathset;
use mm2_formats::water::WaterDef;
use mm2_game::movers::{DRAWBRIDGE_LEAF_MODEL, MoverFamily};

use crate::build_vfs;

/// How a family names the model one of its paths shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelRule {
    /// The lower-cased path name when `geometry/<name>.pkg` exists, else
    /// the family default (the sailboat/ferry/train managers).
    PathName,
    /// The path's asset name (`OPEN:`-style decorations stripped, `PATHnn`
    /// labels refused), else the family default (the drawbridge manager).
    AssetName,
    /// The family rolls its own models (parked cars); nothing to resolve.
    None,
}

/// One per-city object pathset family.
#[derive(Debug, Clone, Copy)]
struct Family {
    /// The object name in the file name.
    object: &'static str,
    /// What it is.
    label: &'static str,
    default_model: Option<&'static str>,
    /// The mover manager whose shared rule names the model, if any.
    mover: Option<MoverFamily>,
    rule: ModelRule,
    /// The runtime consumer, or why none is wired.
    consumer: &'static str,
    /// Where the family's loading rule was recovered.
    evidence: &'static str,
}

fn families() -> [Family; 5] {
    let mover = |m: MoverFamily, label, consumer| Family {
        object: m.object(),
        label,
        default_model: Some(m.default_model()),
        mover: Some(m),
        rule: ModelRule::PathName,
        consumer,
        evidence: "docs/research/movers.md",
    };
    [
        Family {
            object: "bridge",
            label: "drawbridge leaves",
            default_model: Some(DRAWBRIDGE_LEAF_MODEL),
            mover: None,
            rule: ModelRule::AssetName,
            consumer: "mm2_app::drawbridge",
            evidence: "docs/research/drawbridge.md (0x415410)",
        },
        mover(
            MoverFamily::Sailboat,
            "tugs, water taxis, sailboards, ducks",
            "mm2_app::movers",
        ),
        mover(MoverFamily::Ferry, "car ferries", "mm2_app::movers"),
        mover(
            MoverFamily::Train,
            "Underground cars (3 per path)",
            "mm2_app::movers",
        ),
        Family {
            object: "parkedcar",
            label: "parked-car strips",
            default_model: None,
            mover: None,
            rule: ModelRule::None,
            consumer: "mm2_app::city::spawn_parked_cars",
            evidence: "docs/research/parked.md",
        },
    ]
}

/// Which role a discovered file plays for its family.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    /// `<city>_<object>.pathset`.
    Default,
    /// `<city>_<object>_<stem>.pathset`; `reachable` when a catalogued
    /// event of the city has that stem.
    Event { stem: String, reachable: bool },
    /// Named like the family's files but not loadable as one (backup
    /// copies such as `.pathset.bak`).
    Variant,
}

/// What loading one file measured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Loaded {
    paths: usize,
    points: usize,
    /// Validator findings (authored quirks, reported not failed).
    issues: usize,
    /// Paths whose own model resolves.
    named: usize,
    /// Paths that fall back to the family default.
    default: usize,
    /// Paths where neither resolves.
    unresolved: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileRow {
    logical: String,
    role: Role,
    /// `None` for a [`Role::Variant`] (never loaded).
    loaded: Option<Result<Loaded, String>>,
}

#[derive(Debug, Clone)]
struct FamilyRow {
    family: Family,
    /// This city's default file is in the expected table.
    expected: bool,
    files: Vec<FileRow>,
}

impl FamilyRow {
    fn default_file(&self) -> Option<&FileRow> {
        self.files.iter().find(|f| f.role == Role::Default)
    }
}

/// The city's water record.
#[derive(Debug, Clone, PartialEq)]
enum Water {
    Absent,
    Unreadable(String),
    Loaded {
        level: f32,
        rooms: usize,
        issues: usize,
    },
}

/// Rail curves the city's ambient-navigation file authors.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rails {
    Absent,
    Unreadable(String),
    Loaded {
        roads: usize,
        roads_with_rails: usize,
        tram_curves: usize,
        train_curves: usize,
    },
}

/// Everything one city's audit measured.
#[derive(Debug, Clone)]
struct CityAudit {
    city: String,
    events: usize,
    families: Vec<FamilyRow>,
    water: Water,
    rails: Rails,
}

/// The cable car's assets (shared by both cities — `va_cablecar_f` is
/// one record in the shared ambient pool).
const CABLE_CAR_ID: &str = "va_cablecar_f";

fn cable_car_assets() -> [String; 3] {
    [
        format!("geometry/{CABLE_CAR_ID}.pkg"),
        format!("bound/{CABLE_CAR_ID}_bound.bnd"),
        format!("tune/vehicle/{CABLE_CAR_ID}.aivehicledata"),
    ]
}

/// Parse one family file and resolve its model references.
fn load_file(vfs: &Vfs, family: &Family, logical: &str) -> Result<Loaded, String> {
    let (bytes, _) = vfs.read_path(logical).map_err(|e| e.to_string())?;
    let ps = Pathset::parse(&bytes).map_err(|e| e.to_string())?;
    let mut out = Loaded {
        paths: ps.paths.len(),
        points: ps.paths.iter().map(|p| p.points.len()).sum(),
        issues: ps.validate().len(),
        ..Loaded::default()
    };
    let resolves = |name: &str| vfs.resolve(&format!("geometry/{name}.pkg")).is_some();
    for path in &ps.paths {
        match (family.rule, family.mover) {
            (ModelRule::PathName, Some(mover)) => {
                // The managers' own rule: a path that names no geometry
                // gets the family default.
                let model = mover.model_for(&path.name, resolves);
                if !resolves(&model) {
                    out.unresolved += 1;
                } else if model == path.name.to_ascii_lowercase() {
                    out.named += 1;
                } else {
                    out.default += 1;
                }
            }
            (ModelRule::AssetName, _) => {
                let own = path.asset_name().map(str::to_ascii_lowercase);
                if own.as_deref().is_some_and(resolves) {
                    out.named += 1;
                } else if family.default_model.is_some_and(resolves) {
                    out.default += 1;
                } else {
                    out.unresolved += 1;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn audit_family(
    vfs: &Vfs,
    city: &str,
    family: Family,
    event_stems: &BTreeSet<String>,
    all: &[String],
) -> FamilyRow {
    let prefix = format!("race/{city}/{city}_{}", family.object);
    let default = format!("{prefix}.pathset");
    let overlay = format!("{prefix}_");
    let mut files = Vec::new();
    for logical in all {
        let role = if *logical == default {
            Role::Default
        } else if let Some(stem) = logical
            .strip_prefix(&overlay)
            .and_then(|r| r.strip_suffix(".pathset"))
        {
            Role::Event {
                stem: stem.to_string(),
                reachable: event_stems.contains(stem),
            }
        } else if logical.starts_with(&prefix) {
            Role::Variant
        } else {
            continue;
        };
        let loaded = (role != Role::Variant).then(|| load_file(vfs, &family, logical));
        files.push(FileRow {
            logical: logical.clone(),
            role,
            loaded,
        });
    }
    FamilyRow {
        family,
        expected: EXPECTED_SPECIAL_PATHSETS.contains(&(city, family.object)),
        files,
    }
}

fn audit_water(vfs: &Vfs, city: &str) -> Water {
    let logical = format!("city/{city}.water");
    let Ok((bytes, _)) = vfs.read_path(&logical) else {
        return Water::Absent;
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => return Water::Unreadable(e.to_string()),
    };
    match WaterDef::parse(&text) {
        Ok(def) => Water::Loaded {
            level: def.level,
            rooms: def.refs.len(),
            issues: def.validate().len(),
        },
        Err(e) => Water::Unreadable(e.to_string()),
    }
}

fn audit_rails(vfs: &Vfs, city: &str) -> Rails {
    let Ok((bytes, _)) = vfs.read_path(&format!("city/{city}.bai")) else {
        return Rails::Absent;
    };
    let bai = match Bai::parse(&bytes) {
        Ok(b) => b,
        Err(e) => return Rails::Unreadable(e.to_string()),
    };
    let (mut with, mut tram, mut train) = (0, 0, 0);
    for road in &bai.roads {
        let (t, r) = [&road.left, &road.right].iter().fold((0, 0), |(t, r), s| {
            (t + s.tram_count as usize, r + s.train_count as usize)
        });
        tram += t;
        train += r;
        with += usize::from(t + r > 0);
    }
    Rails::Loaded {
        roads: bai.roads.len(),
        roads_with_rails: with,
        tram_curves: tram,
        train_curves: train,
    }
}

fn audit_city(vfs: &Vfs, city: &str) -> CityAudit {
    let events = EventCatalog::scan(vfs, city).events;
    let stems: BTreeSet<String> = events.iter().map(|e| e.stem.clone()).collect();
    let all: Vec<String> = vfs.list();
    CityAudit {
        city: city.to_string(),
        events: events.len(),
        families: families()
            .into_iter()
            .map(|f| audit_family(vfs, city, f, &stems, &all))
            .collect(),
        water: audit_water(vfs, city),
        rails: audit_rails(vfs, city),
    }
}

impl CityAudit {
    /// Data failures — the lines `--strict` exits on.
    fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for row in &self.families {
            let object = row.family.object;
            if row.expected && row.default_file().is_none() {
                out.push(format!(
                    "{}: expected race/{0}/{0}_{object}.pathset is missing",
                    self.city
                ));
            }
            for file in &row.files {
                // A file nothing can select (an unreachable overlay) is an
                // authored leftover — london_bridge_blitz10 is truncated
                // and London's blitz table ends at blitz9 — so it is shown
                // but cannot fail the audit.
                let selectable = match &file.role {
                    Role::Default => true,
                    Role::Event { reachable, .. } => *reachable,
                    Role::Variant => false,
                };
                if !selectable {
                    continue;
                }
                match &file.loaded {
                    Some(Err(e)) => out.push(format!("{}: {e}", file.logical)),
                    Some(Ok(l)) if l.unresolved > 0 => out.push(format!(
                        "{}: {} path(s) name a model that resolves neither by name nor as the family default",
                        file.logical, l.unresolved
                    )),
                    _ => {}
                }
            }
        }
        match &self.water {
            Water::Absent => out.push(format!("{0}: city/{0}.water is missing", self.city)),
            Water::Unreadable(e) => out.push(format!("{0}: city/{0}.water: {e}", self.city)),
            Water::Loaded { .. } => {}
        }
        if let Rails::Unreadable(e) = &self.rails {
            out.push(format!("{0}: city/{0}.bai: {e}", self.city));
        }
        out
    }
}

/// The unresolved-actor lines (shared by both cities).
fn unresolved(vfs: &Vfs) -> Vec<String> {
    let assets: Vec<String> = cable_car_assets()
        .iter()
        .map(|a| {
            format!(
                "{a} {}",
                if vfs.resolve(a).is_some() {
                    "ok"
                } else {
                    "MISSING"
                }
            )
        })
        .collect();
    vec![format!(
        "cable car ({CABLE_CAR_ID}): UNRESOLVED — no runtime. Assets: {}. Evidence: \
         Midtown2.exe creates cable cars in the AI map init (string \"AIMAP.Init: Create the \
         cable cars.\" at 0x5d66d0, referenced from 0x5358cf; \"Returning a NULL CableCar\" at \
         0x5d64bc) and ships `cablecar*`/`streetcable` audio; no per-city cable-car pathset \
         exists. Which authored data routes it (San Francisco's BAI tram-rail curves, listed above, are the candidate) is \
         not recovered, so no behaviour is claimed.",
        assets.join(", ")
    )]
}

fn render(audit: &CityAudit) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "== {} == ({} catalogued events)",
        audit.city, audit.events
    );
    for row in &audit.families {
        let f = &row.family;
        let present = row.default_file().is_some();
        let note = match (row.expected, present) {
            (true, true) => "expected, present",
            (true, false) => "EXPECTED, MISSING",
            (false, true) => "not in the expected table, present",
            (false, false) => "none in this installation (not expected)",
        };
        let _ = writeln!(
            s,
            "  {:<9} {} — {note}\n            consumer {}; rule: {}",
            f.object, f.label, f.consumer, f.evidence
        );
        for file in &row.files {
            let role = match &file.role {
                Role::Default => "default".to_string(),
                Role::Event { stem, reachable } => format!(
                    "event {stem}{}",
                    if *reachable {
                        ""
                    } else {
                        " (no catalogued event selects it)"
                    }
                ),
                Role::Variant => "variant (never loaded)".to_string(),
            };
            let detail = match &file.loaded {
                None => String::new(),
                Some(Err(e)) => format!("PARSE FAILED: {e}"),
                Some(Ok(l)) if row.family.rule == ModelRule::None => format!(
                    "{} path(s), {} point(s), {} issue(s); models rolled by the manager",
                    l.paths, l.points, l.issues
                ),
                Some(Ok(l)) => format!(
                    "{} path(s), {} point(s), {} issue(s); models named {} / default {} / unresolved {}",
                    l.paths, l.points, l.issues, l.named, l.default, l.unresolved
                ),
            };
            let _ = writeln!(s, "    {} [{role}] {detail}", file.logical);
        }
    }
    let _ = match &audit.water {
        Water::Absent => writeln!(s, "  water     city/{}.water ABSENT", audit.city),
        Water::Unreadable(e) => {
            writeln!(s, "  water     city/{}.water UNREADABLE: {e}", audit.city)
        }
        Water::Loaded {
            level,
            rooms,
            issues,
        } => writeln!(
            s,
            "  water     city/{}.water level {level}, {rooms} room ref(s), {issues} issue(s); consumer mm2_app::water + mm2_app::recovery",
            audit.city
        ),
    };
    let _ = match &audit.rails {
        Rails::Absent => writeln!(s, "  rails     city/{}.bai ABSENT", audit.city),
        Rails::Unreadable(e) => writeln!(s, "  rails     city/{}.bai UNREADABLE: {e}", audit.city),
        Rails::Loaded {
            roads,
            roads_with_rails,
            tram_curves,
            train_curves,
        } => writeln!(
            s,
            "  rails     city/{}.bai: {roads_with_rails}/{roads} road(s) carry rails — {tram_curves} tram curve(s), {train_curves} train curve(s)",
            audit.city
        ),
    };
    s
}

/// Cities to audit: an explicit one, else every stock city plus any
/// discovered `race/<city>/` directory.
fn cities(vfs: &Vfs, only: Option<&str>) -> Vec<String> {
    match only {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => race_cities(vfs),
    }
}

pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let audits: Vec<CityAudit> = cities(&vfs, city)
        .iter()
        .map(|c| audit_city(&vfs, c))
        .collect();
    let mut failures = Vec::new();
    let (mut expected, mut found, mut files, mut loaded) = (0, 0, 0, 0);
    for audit in &audits {
        print!("{}", render(audit));
        failures.extend(audit.failures());
        for row in &audit.families {
            expected += usize::from(row.expected);
            found += usize::from(row.expected && row.default_file().is_some());
            for file in row.files.iter().filter(|f| f.loaded.is_some()) {
                files += 1;
                loaded += usize::from(matches!(file.loaded, Some(Ok(_))));
            }
        }
    }
    let open = unresolved(&vfs);
    println!("unresolved special actors:");
    for line in &open {
        println!("  {line}");
    }
    for f in &failures {
        println!("FAIL {f}");
    }
    println!(
        "specials: {} city(ies); expected default pathsets {found}/{expected}; pathset files loaded {loaded}/{files}; \
         {} failure(s); {} unresolved actor(s)",
        audits.len(),
        failures.len(),
        open.len()
    );
    if strict && audits.is_empty() {
        return Err("strict specials audit: no city to audit".into());
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict specials audit: {} failure(s)", failures.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// A `PTH1` file of `(name, points)` line-strip paths.
    fn pth1(paths: &[(&str, &[[f32; 3]])]) -> Vec<u8> {
        let mut d = b"PTH1".to_vec();
        d.extend_from_slice(&(paths.len() as u32).to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        for (name, points) in paths {
            let mut n = vec![0u8; 32];
            n[..name.len()].copy_from_slice(name.as_bytes());
            d.extend_from_slice(&n);
            d.extend_from_slice(&(points.len() as u32).to_le_bytes());
            d.extend_from_slice(&0u32.to_le_bytes());
            for p in *points {
                d.extend_from_slice(&0u32.to_le_bytes());
                for c in p {
                    d.extend_from_slice(&c.to_le_bytes());
                }
            }
            d.extend_from_slice(&[2, 0, 0, 0]);
        }
        d
    }

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    }

    fn row<'a>(a: &'a CityAudit, object: &str) -> &'a FamilyRow {
        a.families
            .iter()
            .find(|r| r.family.object == object)
            .unwrap()
    }

    const POINTS: &[[f32; 3]] = &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]];

    /// A tiny install: a London ferry with one named and one default-model
    /// path, an unreachable overlay, a backup variant, and the water file.
    fn install(dir: &Path) {
        write(dir, "geometry/giz_carferry01_f.pkg", b"x");
        write(dir, "geometry/giz_tug01_l.pkg", b"x");
        write(
            dir,
            "race/london/london_ferry.pathset",
            &pth1(&[("giz_tug01_l", POINTS), ("PATH01", POINTS)]),
        );
        write(
            dir,
            "race/london/london_ferry_crash9.pathset",
            &pth1(&[("anything", POINTS)]),
        );
        write(dir, "race/london/london_ferry.pathset.bak", b"junk");
        write(dir, "city/london.water", b"-1.5\n3\n4\n");
    }

    #[test]
    fn files_are_classified_and_their_models_resolved_like_the_manager_does() {
        let d = tempfile::tempdir().unwrap();
        install(d.path());
        let a = audit_city(&vfs_of(d.path()), "london");
        let ferry = row(&a, "ferry");
        assert!(ferry.expected);
        assert_eq!(ferry.files.len(), 3);
        let by = |suffix: &str| {
            ferry
                .files
                .iter()
                .find(|f| f.logical.ends_with(suffix))
                .unwrap()
        };
        let default = by("london_ferry.pathset");
        assert_eq!(default.role, Role::Default);
        let l = default.loaded.clone().unwrap().unwrap();
        assert_eq!((l.paths, l.points), (2, 6));
        // The tug resolves by its own name; `PATH01` falls back to the
        // ferry default, which resolves.
        assert_eq!((l.named, l.default, l.unresolved), (1, 1, 0));
        // No event table in the fixture, so the crash9 overlay is
        // selected by nothing.
        assert_eq!(
            by("crash9.pathset").role,
            Role::Event {
                stem: "crash9".into(),
                reachable: false
            }
        );
        assert_eq!(by(".bak").role, Role::Variant);
        assert!(by(".bak").loaded.is_none());
    }

    #[test]
    fn an_unreachable_garbled_overlay_is_shown_but_does_not_fail() {
        let d = tempfile::tempdir().unwrap();
        install(d.path());
        write(
            d.path(),
            "race/london/london_ferry_blitz10.pathset",
            b"PTH1\x01",
        );
        let a = audit_city(&vfs_of(d.path()), "london");
        let ferry = row(&a, "ferry");
        let f = ferry
            .files
            .iter()
            .find(|f| f.logical.ends_with("blitz10.pathset"))
            .unwrap();
        assert!(matches!(f.loaded, Some(Err(_))));
        assert!(!a.failures().iter().any(|m| m.contains("blitz10")));
    }

    #[test]
    fn a_model_that_resolves_nowhere_is_a_strict_failure() {
        let d = tempfile::tempdir().unwrap();
        install(d.path());
        fs::remove_file(d.path().join("geometry/giz_carferry01_f.pkg")).unwrap();
        let a = audit_city(&vfs_of(d.path()), "london");
        let fails = a.failures();
        assert!(
            fails
                .iter()
                .any(|f| f.contains("london_ferry") && f.contains("neither by name")),
            "{fails:?}"
        );
    }

    #[test]
    fn a_missing_expected_default_and_a_garbled_file_both_fail() {
        let d = tempfile::tempdir().unwrap();
        install(d.path());
        write(
            d.path(),
            "race/london/london_sailboat.pathset",
            b"not a pathset",
        );
        let a = audit_city(&vfs_of(d.path()), "london");
        let fails = a.failures().join("\n");
        assert!(
            fails.contains("london_bridge.pathset is missing"),
            "{fails}"
        );
        assert!(fails.contains("london_train.pathset is missing"), "{fails}");
        assert!(fails.contains("london_sailboat.pathset"), "{fails}");
        assert!(matches!(
            row(&a, "sailboat").default_file().unwrap().loaded,
            Some(Err(_))
        ));
    }

    #[test]
    fn a_city_without_an_expected_family_is_absent_not_invented() {
        // San Francisco authors no Underground: no `train` expectation,
        // so its absence is not a failure and no row is fabricated.
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "city/sf.water", b"0\n1\n");
        let a = audit_city(&vfs_of(d.path()), "sf");
        let train = row(&a, "train");
        assert!(!train.expected && train.files.is_empty());
        assert!(!a.failures().iter().any(|f| f.contains("train")));
        // ... but its bridge is expected and reported missing.
        assert!(a.failures().iter().any(|f| f.contains("sf_bridge")));
    }

    #[test]
    fn water_and_rails_are_reported_and_absence_fails() {
        let d = tempfile::tempdir().unwrap();
        install(d.path());
        let a = audit_city(&vfs_of(d.path()), "london");
        assert_eq!(
            a.water,
            Water::Loaded {
                level: -1.5,
                rooms: 2,
                issues: 0
            }
        );
        assert_eq!(a.rails, Rails::Absent);
        let empty = tempfile::tempdir().unwrap();
        let b = audit_city(&vfs_of(empty.path()), "london");
        assert_eq!(b.water, Water::Absent);
        assert!(b.failures().iter().any(|f| f.contains("london.water")));
    }

    #[test]
    fn the_cable_car_is_listed_unresolved_with_its_asset_state() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "geometry/va_cablecar_f.pkg", b"x");
        let lines = unresolved(&vfs_of(d.path()));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("UNRESOLVED"));
        assert!(lines[0].contains("geometry/va_cablecar_f.pkg ok"));
        assert!(lines[0].contains("bound/va_cablecar_f_bound.bnd MISSING"));
    }

    #[test]
    fn the_stock_cities_are_audited_even_when_the_install_has_none() {
        let d = tempfile::tempdir().unwrap();
        let vfs = vfs_of(d.path());
        let cs = cities(&vfs, None);
        assert!(cs.contains(&"sf".to_string()) && cs.contains(&"london".to_string()));
        assert_eq!(cities(&vfs, Some("SF")), vec!["sf".to_string()]);
    }
}

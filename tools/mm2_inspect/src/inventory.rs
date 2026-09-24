//! `mm2-inspect inventory`: the versioned content inventory (F00-B.1).
//!
//! Composes the same VFS, mount policy, `VehicleCatalog` and format
//! parsers the game uses — no second loader. Reports are versioned by
//! engine commit (embedded at build time) and a deterministic catalog
//! fingerprint over resolved-path provenance.
//!
//! ## Count semantics (per family)
//!
//! - `expected`: entries in the authored stock denominator
//!   ([`mm2_content::expect`] tables), independent of import success.
//! - `discovered`: expected entries found plus extras found in the VFS.
//! - `accepted`: entries passing every check that exists for the family —
//!   PSDL parse for cities, dependency resolution for vehicles, INST
//!   parse for placement, presence of the authored primary record for
//!   events, full file set for pedestrian archetypes.
//! - `rejected`: entries that failed a check, expected entries missing,
//!   or records rejected as non-content junk — each with a reason.
//! - `unverified`: discovered records no existing check can judge
//!   (unparsed formats). Orthogonal to accepted/rejected by design.
//! - `extras`: discovered records beyond the stock table.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mm2_assets::{MountReport, Vfs};
use mm2_content::{
    EXPECTED_AUDIO_FAMILIES, EXPECTED_CITIES, EXPECTED_EVENT_TABLES, EXPECTED_PEDS,
    EXPECTED_RACE_CITIES, EXPECTED_STOCK_ROSTER, EntryStatus, PED_REQUIRED_EXTS, PrimaryRecord,
    VehicleCatalog, expected_events,
};
use mm2_formats::inst;
use mm2_formats::psdl::Psdl;
use mm2_formats::racedata::EventTable;
use mm2_formats::racefiles::{RaceFileKind as Kind, classify_race_file};
use serde_json::{Value, json};

/// Engine commit embedded by `build.rs`; `unknown` outside a git checkout.
const COMMIT: &str = env!("MM2_BUILD_COMMIT");

/// One rejected inventory record: what and why.
#[derive(Debug)]
pub struct Rejected {
    /// File path or entry id.
    pub entry: String,
    /// Why it was rejected (parse error, missing record, junk artifact).
    pub reason: String,
}

/// Inventory result for one content family.
#[derive(Debug, Default)]
pub struct Family {
    /// Family name, e.g. `cities`.
    pub name: &'static str,
    /// Authored stock denominator (0 = no authored table known).
    pub expected: usize,
    /// Expected-found plus extras found in the VFS.
    pub discovered: usize,
    /// Entries passing every existing check for this family.
    pub accepted: usize,
    /// Discovered records no check can judge (unparsed formats).
    pub unverified: usize,
    /// Discovered records beyond the stock table.
    pub extras: usize,
    /// Rejected records with reasons (failed checks, missing expected,
    /// non-content junk).
    pub rejected: Vec<Rejected>,
    /// Human-readable detail lines.
    pub notes: Vec<String>,
}

impl Family {
    fn new(name: &'static str) -> Self {
        Family {
            name,
            ..Family::default()
        }
    }
}

/// The full inventory report.
#[derive(Debug)]
pub struct Report {
    /// Engine commit that produced this report.
    pub commit: String,
    /// Installation directory inventoried.
    pub install: PathBuf,
    /// Mounted archives (mount order).
    pub archives: Vec<PathBuf>,
    /// Archives that failed to mount, with the error.
    pub skipped_archives: Vec<(PathBuf, String)>,
    /// Mounted mod ids (mount order).
    pub mods: Vec<String>,
    /// Total logical paths resolved.
    pub logical_paths: usize,
    /// Deterministic catalog fingerprint (`fnv1a64:<hex>`). Covers
    /// resolved-path provenance and source file sizes — not a content
    /// hash; identical catalog + provenance ⇒ identical fingerprint.
    pub fingerprint: String,
    /// Per-family results.
    pub families: Vec<Family>,
}

/// Build the inventory for an installation directory.
pub fn build(
    vfs: &Vfs,
    mount: &MountReport,
    dir: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    let paths = vfs.list();
    let mut families = Vec::new();
    families.push(cities(vfs, &paths));
    families.push(vehicles(vfs));
    let (races, lessons) = events(vfs, &paths);
    families.push(races);
    families.push(lessons);
    families.push(placement(vfs, &paths));
    families.push(audio(&paths));
    families.push(pedestrians(&paths));
    families.push(multiplayer(&paths));
    families.push(traffic(&paths));
    families.push(breakables(&paths));
    families.push(profiles(&paths));
    families.push(interface(&paths));

    Ok(Report {
        commit: COMMIT.to_string(),
        install: dir.to_path_buf(),
        archives: mount.archives.clone(),
        skipped_archives: mount.skipped.clone(),
        mods: mount.mods.iter().map(|m| m.id.clone()).collect(),
        logical_paths: paths.len(),
        fingerprint: fingerprint(vfs),
        families,
    })
}

/// `true` when the report is clean for strict validation: no family has
/// rejected entries and no family with a nonzero expected denominator is
/// empty (an empty catalog never passes).
pub fn strict_failures(report: &Report) -> Vec<String> {
    let mut out = Vec::new();
    for f in &report.families {
        if f.expected > 0 && f.discovered == 0 {
            out.push(format!(
                "{}: expected {} entries, discovered none",
                f.name, f.expected
            ));
        }
        for r in &f.rejected {
            out.push(format!("{}: {} — {}", f.name, r.entry, r.reason));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(h: u64, bytes: impl AsRef<[u8]>) -> u64 {
    bytes
        .as_ref()
        .iter()
        .fold(h, |h, b| h.wrapping_mul(FNV_PRIME) ^ u64::from(*b))
}

/// FNV-1a-64 over every resolved logical path, its provenance and the
/// winning source's file size. Deterministic for a given catalog; not a
/// content hash (bytes are not read).
fn fingerprint(vfs: &Vfs) -> String {
    let mut h = FNV_OFFSET;
    let mut size_cache: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for logical in vfs.list() {
        h = fnv(h, &logical);
        let Some(r) = vfs.resolve(&logical) else {
            continue;
        };
        h = fnv(h, r.source.path.to_string_lossy().as_bytes());
        if let Some(off) = r.source.archive_offset {
            h = fnv(h, (off as u64).to_le_bytes());
        }
        if let Some(label) = &r.source.label {
            h = fnv(h, label);
        }
        let size = size_cache.entry(r.source.path.clone()).or_insert_with(|| {
            std::fs::metadata(&r.source.path)
                .map(|m| m.len())
                .unwrap_or(0)
        });
        h = fnv(h, size.to_le_bytes());
    }
    format!("fnv1a64:{h:016x}")
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn basename(logical: &str) -> &str {
    logical.rsplit('/').next().unwrap_or(logical)
}

// ---------------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------------

/// `city/<id>.psdl` — the two shipped cities plus any extras.
fn cities(vfs: &Vfs, paths: &[String]) -> Family {
    let mut f = Family::new("cities");
    f.expected = EXPECTED_CITIES.len();

    let mut stems: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        if let Some(rest) = p.strip_prefix("city/")
            && let Some(stem) = rest.strip_suffix(".psdl")
            && !stem.contains('/')
        {
            stems.insert(stem.to_string());
        }
    }

    for stem in &stems {
        let parsed = vfs
            .read_logical(&format!("city/{stem}.psdl"))
            .map_err(|e| e.to_string())
            .and_then(|b| Psdl::parse(&b).map_err(|e| e.to_string()));
        match parsed {
            Ok(_) => f.accepted += 1,
            Err(e) => f.rejected.push(Rejected {
                entry: format!("city/{stem}.psdl"),
                reason: e,
            }),
        }
    }
    f.discovered = stems.len();
    for city in EXPECTED_CITIES {
        if !stems.contains(*city) {
            f.rejected.push(Rejected {
                entry: format!("city/{city}.psdl"),
                reason: "expected stock city not discovered".into(),
            });
        }
    }
    f.extras = stems
        .iter()
        .filter(|s| !EXPECTED_CITIES.contains(&s.as_str()))
        .count();
    if f.extras > 0 {
        f.notes.push(format!(
            "extra city datasets: {}",
            stems
                .iter()
                .filter(|s| !EXPECTED_CITIES.contains(&s.as_str()))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    f
}

/// Player vehicles via the existing catalog (metadata/tuning/model/bound
/// resolution). Deep load + paint validation is `validate-cars`' job and
/// stays unverified here.
fn vehicles(vfs: &Vfs) -> Family {
    let mut f = Family::new("player vehicles");
    f.expected = EXPECTED_STOCK_ROSTER.len();

    let catalog = VehicleCatalog::scan(vfs);
    f.discovered = catalog.entries.len();
    f.extras = catalog
        .entries
        .iter()
        .filter(|e| !EXPECTED_STOCK_ROSTER.contains(&e.id.as_str()))
        .count();

    let mut stock_paints = 0usize;
    for e in &catalog.entries {
        match &e.status {
            EntryStatus::Ready => {
                f.accepted += 1;
                if EXPECTED_STOCK_ROSTER.contains(&e.id.as_str()) {
                    stock_paints += e.paints.len();
                }
            }
            EntryStatus::Incomplete { missing } => f.rejected.push(Rejected {
                entry: e.id.clone(),
                reason: format!("incomplete: {}", missing.join(", ")),
            }),
        }
    }
    for id in EXPECTED_STOCK_ROSTER {
        if !catalog.entries.iter().any(|e| e.id == *id) {
            f.rejected.push(Rejected {
                entry: id.to_string(),
                reason: "expected stock vehicle not discovered".into(),
            });
        }
    }
    f.notes.push(format!(
        "{stock_paints} declared paint variants across ready stock entries; per-paint model/shader validation is `validate-cars`, not this command"
    ));
    f
}

// ---------------------------------------------------------------------------
// Race / lesson events
// ---------------------------------------------------------------------------

/// Does a discovered record satisfy an expected event's primary-record
/// requirement? The classification itself lives in
/// `mm2_formats::racefiles` — shared with the event catalog so audits
/// and the catalog never disagree about which record belongs to an
/// event.
fn satisfies(kind: Kind, primary: PrimaryRecord) -> bool {
    match primary {
        PrimaryRecord::Aimap => kind == Kind::Aimap,
        PrimaryRecord::CsvRecord => {
            matches!(
                kind,
                Kind::Csv | Kind::DataCsv | Kind::Waypoints | Kind::StartPoints
            )
        }
        PrimaryRecord::Pathset => kind == Kind::Pathset,
    }
}

/// Aggregated per-city counts that do not belong to either family.
#[derive(Default)]
struct RaceAux {
    aimap_p: usize,
    opp_a: usize,
    opp_p: usize,
    meta_tables: usize,
    aux_records: usize,
    variant_records: usize,
}

fn events(vfs: &Vfs, paths: &[String]) -> (Family, Family) {
    let mut races = Family::new("races");
    let mut lessons = Family::new("crash-course lessons");
    let mut aux_totals = RaceAux::default();

    for city in EXPECTED_RACE_CITIES {
        let prefix = format!("race/{city}/");
        // stem -> file kinds seen, plus non-event records bucketed aside.
        let mut stems: BTreeMap<String, BTreeSet<Kind>> = BTreeMap::new();
        let mut junk: Vec<Rejected> = Vec::new();
        let mut meta_tables: Vec<String> = Vec::new();
        for p in paths {
            if *p == format!("race/{city}") {
                aux_totals.aux_records += 1; // extensionless loose file
                continue;
            }
            let Some(name) = p.strip_prefix(&prefix) else {
                continue;
            };
            if name.contains('/') {
                aux_totals.aux_records += 1; // e.g. csvs/ subdirectory records
                continue;
            }
            let (kind, stem) = classify_race_file(name);
            match kind {
                Kind::Junk => junk.push(Rejected {
                    entry: p.clone(),
                    reason: "non-content artifact (backup/conflict/dev leftover)".into(),
                }),
                Kind::Meta => {
                    aux_totals.meta_tables += 1;
                    meta_tables.push(p.clone());
                }
                _ => {
                    if kind == Kind::AimapP {
                        aux_totals.aimap_p += 1;
                    }
                    if name.ends_with("_p.csv") {
                        aux_totals.variant_records += 1;
                    }
                    if name.contains("-p-") {
                        aux_totals.opp_p += 1;
                    } else if name.contains("-a-") {
                        aux_totals.opp_a += 1;
                    }
                    if let Some(stem) = stem {
                        stems.entry(stem).or_default().insert(kind);
                    }
                }
            }
        }

        let mut claimed: BTreeSet<String> = BTreeSet::new();
        for ev in expected_events(city) {
            let fam = if ev.lesson { &mut lessons } else { &mut races };
            fam.expected += 1;
            let matched: Vec<String> = stems
                .keys()
                .filter(|s| **s == ev.id || (ev.prefix && s.starts_with(&ev.id)))
                .cloned()
                .collect();
            if matched.is_empty() {
                fam.rejected.push(Rejected {
                    entry: format!("{prefix}{}", ev.id),
                    reason: "expected authored event not discovered".into(),
                });
                continue;
            }
            claimed.extend(matched.iter().cloned());
            fam.discovered += 1;
            if matched
                .iter()
                .any(|s| stems[s].iter().any(|k| satisfies(*k, ev.primary)))
            {
                fam.accepted += 1;
            } else {
                fam.rejected.push(Rejected {
                    entry: format!("{prefix}{}", ev.id),
                    reason: format!(
                        "partial: no {:?} record (has {})",
                        ev.primary,
                        matched
                            .iter()
                            .flat_map(|s| stems[s].iter().map(|k| format!("{k:?}")))
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }

        // Leftover stems: `<city>_*` records are ambient-path aux,
        // everything else is an extra event-like record.
        let mut extras: Vec<&String> = Vec::new();
        for stem in stems.keys() {
            if claimed.contains(stem) {
                continue;
            }
            if stem.starts_with(&format!("{city}_")) {
                aux_totals.aux_records += 1;
            } else {
                extras.push(stem);
            }
        }
        races.extras += extras.len();
        races.discovered += extras.len();
        races.notes.push(format!(
            "{city}: {} extra records ({}{})",
            extras.len(),
            extras
                .iter()
                .take(12)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if extras.len() > 12 { ", …" } else { "" }
        ));
        // Event-metadata tables: parse the authored rosters through the
        // VFS, not just their presence. A missing expected table or a
        // malformed row is a rejected record; row counts become the
        // ledger's reproducible evidence for the per-city event roster.
        let mut table_note: Vec<String> = Vec::new();
        for (table, lesson) in EXPECTED_EVENT_TABLES {
            let logical = format!("{prefix}{table}");
            let fam = if *lesson { &mut lessons } else { &mut races };
            if !meta_tables.contains(&logical) {
                fam.rejected.push(Rejected {
                    entry: logical,
                    reason: "expected event-metadata table not discovered".into(),
                });
                continue;
            }
            match vfs
                .read_logical(&logical)
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    EventTable::parse(&String::from_utf8_lossy(&b)).map_err(|e| e.to_string())
                }) {
                Ok(t) => {
                    table_note.push(format!("{table}={} rows", t.rows.len()));
                    for d in &t.diagnostics {
                        fam.rejected.push(Rejected {
                            entry: logical.clone(),
                            reason: d.to_string(),
                        });
                    }
                }
                Err(e) => fam.rejected.push(Rejected {
                    entry: logical,
                    reason: format!("malformed event-metadata table: {e}"),
                }),
            }
        }
        races.notes.push(format!(
            "{city} event-metadata rows: {}",
            table_note.join(", ")
        ));

        races.rejected.extend(junk);
    }

    // The mm*data.csv rosters now parse, but no race-format parser
    // exists for the events themselves: every discovered event record
    // is unverified at content level even when its authored files are
    // structurally complete.
    races.unverified = races.discovered;
    lessons.unverified = lessons.discovered;
    races.notes.push(format!(
        "{} .aimap_p variants, {} -a-N.opp / {} -p-N.opp opponent records, {} mm*data.csv metadata tables (parsed), {} aux records, {} *_p variant records — all other records unparsed",
        aux_totals.aimap_p,
        aux_totals.opp_a,
        aux_totals.opp_p,
        aux_totals.meta_tables,
        aux_totals.aux_records,
        aux_totals.variant_records,
    ));
    lessons
        .notes
        .push("exam/final/reverse180 match stems by prefix (exam1_2.csv → exam)".to_string());

    (races, lessons)
}

/// `city/*.inst` placement files plus the unparsed placement-adjacent
/// records (pathsets, prop rules, decals, lightmaps, …).
fn placement(vfs: &Vfs, paths: &[String]) -> Family {
    let mut f = Family::new("placement sources");
    let expected_inst: Vec<String> = EXPECTED_CITIES
        .iter()
        .map(|c| format!("city/{c}.inst"))
        .collect();
    f.expected = expected_inst.len();

    let mut inst_found: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        if p.starts_with("city/") && p.ends_with(".inst") {
            inst_found.insert(p.clone());
        }
    }
    for p in &inst_found {
        match vfs
            .read_logical(p)
            .map_err(|e| e.to_string())
            .and_then(|b| inst::parse(&b).map_err(|e| e.to_string()))
        {
            Ok(_) => f.accepted += 1,
            Err(e) => f.rejected.push(Rejected {
                entry: p.clone(),
                reason: e,
            }),
        }
    }
    f.discovered = inst_found.len();
    f.extras = inst_found
        .iter()
        .filter(|p| !expected_inst.contains(p))
        .count();
    for want in &expected_inst {
        if !inst_found.contains(want) {
            f.rejected.push(Rejected {
                entry: want.clone(),
                reason: "expected stock placement file not discovered".into(),
            });
        }
    }

    // Placement-adjacent authored records outside the .inst
    // denominator. Some have parsers with dedicated audits
    // (`mm2-inspect pathset`, `mm2-inspect proprules`); the rest stay
    // unverified until their formats are understood.
    let aux_exts = [
        "pathset", "cpvs", "ldef", "pvs", "pvshist", "lmap", "water", "rid", "reset", "ext",
        "extra", "grp", "mtl",
    ];
    let aux_csvs = [
        "props.csv",
        "propdefs.csv",
        "proprules.csv",
        "floors.csv",
        "facades.csv",
        "walls.csv",
        "lighting.csv",
        "lighting_ignore.csv",
    ];
    let mut aux = 0usize;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for p in paths {
        if !p.starts_with("city/") || p.ends_with(".inst") || p.ends_with(".psdl") {
            continue;
        }
        let name = basename(p);
        let is_aux = aux_exts.iter().any(|e| name.ends_with(&format!(".{e}")))
            || aux_csvs.contains(&name)
            || name.ends_with(".pathset");
        if is_aux {
            aux += 1;
            let key = name
                .rsplit_once('.')
                .map(|(_, e)| e.to_string())
                .unwrap_or_else(|| name.to_string());
            *by_kind.entry(key).or_default() += 1;
        }
    }
    f.unverified = aux;
    if aux > 0 {
        f.notes.push(format!(
            "{aux} unparsed placement-adjacent records: {}",
            by_kind
                .iter()
                .map(|(k, n)| format!("{k}×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    f
}

/// `aud/<family>/` — presence of the retail audio directories. Content
/// is verified by the dedicated `audio` audit (waves decode, cardata
/// tables parse); the inventory still counts per-file records as
/// unverified since no runtime consumer exists.
fn audio(paths: &[String]) -> Family {
    let mut f = Family::new("audio families");
    f.expected = EXPECTED_AUDIO_FAMILIES.len();

    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for p in paths {
        if let Some(rest) = p.strip_prefix("aud/")
            && let Some(dir) = rest.split('/').next()
        {
            *dirs.entry(dir.to_string()).or_default() += 1;
            f.unverified += 1;
        }
    }
    f.discovered = dirs.len();
    for want in EXPECTED_AUDIO_FAMILIES {
        match dirs.get(*want) {
            Some(_) => f.accepted += 1,
            None => f.rejected.push(Rejected {
                entry: format!("aud/{want}"),
                reason: "expected audio family not discovered".into(),
            }),
        }
    }
    f.extras = dirs
        .keys()
        .filter(|d| !EXPECTED_AUDIO_FAMILIES.contains(&d.as_str()))
        .count();
    f.notes.push(format!(
        "files per family: {}",
        dirs.iter()
            .map(|(d, n)| format!("{d}={n}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    f.notes.push(
        "audited by `mm2-inspect audio`: .wav files decode, aud/cardata + aud/ambient tables parse, DirectMusic containers classify by RIFF form; spchdata/creaturedata tables stay unparsed (F08) — no runtime consumer yet".into(),
    );
    f
}

/// `anim/pedmodel_*` archetypes requiring mod+skel+rays+shaders.
fn pedestrians(paths: &[String]) -> Family {
    let mut f = Family::new("pedestrian archetypes");
    f.expected = EXPECTED_PEDS.len();

    let mut archetypes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut clips = 0usize;
    for p in paths {
        let Some(rest) = p.strip_prefix("anim/") else {
            continue;
        };
        if rest.contains('/') {
            f.rejected.push(Rejected {
                entry: p.clone(),
                reason: "non-content record inside anim/ (e.g. CVS metadata)".into(),
            });
            continue;
        }
        if rest.starts_with("pedanim_") && rest.ends_with(".anim") {
            clips += 1;
        }
        if let Some((stem, ext)) = rest.rsplit_once('.')
            && stem.starts_with("pedmodel_")
        {
            archetypes
                .entry(stem.to_string())
                .or_default()
                .insert(ext.to_string());
        }
    }

    f.discovered = archetypes.len();
    for (stem, exts) in &archetypes {
        let missing: Vec<&&str> = PED_REQUIRED_EXTS
            .iter()
            .filter(|e| !exts.contains(**e))
            .collect();
        if missing.is_empty() {
            f.accepted += 1;
        } else {
            f.rejected.push(Rejected {
                entry: format!("anim/{stem}.*"),
                reason: format!(
                    "partial archetype: missing {}",
                    missing
                        .iter()
                        .map(|e| format!(".{e}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        if !EXPECTED_PEDS.contains(&stem.as_str()) {
            f.extras += 1;
        }
    }
    for want in EXPECTED_PEDS {
        if !archetypes.contains_key(*want) {
            f.rejected.push(Rejected {
                entry: format!("anim/{want}.*"),
                reason: "expected pedestrian archetype not discovered".into(),
            });
        }
    }
    f.unverified = archetypes.len() + clips;
    f.notes.push(format!(
        "{clips} pedanim_* animation clips discovered; no .anim/.mod/.skel parser exists — presence of the required file set is accepted, content unverified"
    ));
    f
}

/// Multiplayer-marked records. No authored MP roster is known — the
/// family reports what is discoverable, all unverified.
fn multiplayer(paths: &[String]) -> Family {
    let mut f = Family::new("multiplayer variants");
    f.notes.push(
        "no authored multiplayer roster known; these are discovered markers, not an expected set"
            .into(),
    );
    let mut aimap_p = 0usize;
    let mut named = Vec::new();
    let mut variants = 0usize;
    for p in paths {
        if !p.starts_with("race/") {
            continue;
        }
        let name = basename(p);
        if name.ends_with(".aimap_p") {
            aimap_p += 1;
        } else if name.contains("multicop") || name.contains("copchase") || name.contains("_multi")
        {
            named.push(p.clone());
        } else if name.ends_with("_p.csv") || name.ends_with("data_p.csv") {
            variants += 1;
        }
    }
    f.discovered = aimap_p + named.len() + variants;
    f.unverified = f.discovered;
    f.extras = f.discovered;
    f.notes.push(format!(
        "{aimap_p} .aimap_p records (role unverified), {} named multi records ({}), {variants} *_p variant records (difficulty vs multiplayer role unverified); no Cops & Robbers-specific data discovered",
        named.len(),
        named.join(", ")
    ));
    f
}

/// Ambient traffic vehicles (`va*`) — geometry, AI tuning and carsim
/// coverage. No authored roster table exists.
fn traffic(paths: &[String]) -> Family {
    let mut f = Family::new("ambient traffic vehicles");
    let mut stems: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for p in paths {
        if let Some(s) = p
            .strip_prefix("geometry/")
            .and_then(|s| s.strip_suffix(".pkg"))
            .filter(|s| s.starts_with("va") && !s.contains('/'))
        {
            stems.entry(s.to_string()).or_default().insert("model");
        }
        if let Some(rest) = p.strip_prefix("tune/vehicle/") {
            for (ext, tag) in [(".aivehicledata", "ai-data"), (".vehcarsim", "carsim")] {
                if let Some(s) = rest.strip_suffix(ext)
                    && s.starts_with("va")
                    && !s.contains('/')
                {
                    stems.entry(s.to_string()).or_default().insert(tag);
                }
            }
        }
    }
    f.discovered = stems.len();
    f.extras = f.discovered;
    f.unverified = f.discovered;
    let partial: Vec<String> = stems
        .iter()
        .filter(|(_, k)| !k.contains("model") || !k.contains("ai-data"))
        .map(|(s, _)| s.clone())
        .collect();
    for s in &partial {
        f.rejected.push(Rejected {
            entry: s.clone(),
            reason: "traffic vehicle lacks model or AI data".into(),
        });
    }
    f.notes.push(format!(
        "{} va* ids (model+AI+carsim coverage); no authored traffic roster table",
        stems.len()
    ));
    f
}

/// Breakable world props (`tune/banger/*.dgbangerdata`).
fn breakables(paths: &[String]) -> Family {
    let mut f = Family::new("breakable props");
    let mut files = 0usize;
    let mut stems: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        if let Some(rest) = p.strip_prefix("tune/banger/")
            && let Some(s) = rest.strip_suffix(".dgbangerdata")
        {
            files += 1;
            stems.insert(s.to_string());
        }
    }
    f.discovered = files;
    f.extras = files;
    f.unverified = files;
    f.notes.push(format!(
        "{files} banger records across {} prop ids; parsed and audited by `banger`/`banger-bind`, no runtime consumer yet",
        stems.len()
    ));
    f
}

/// `players/` — profile/save records shipped or created in the install.
fn profiles(paths: &[String]) -> Family {
    let mut f = Family::new("player profiles");
    f.discovered = paths.iter().filter(|p| p.starts_with("players/")).count();
    f.extras = f.discovered;
    f.unverified = f.discovered;
    f.notes.push(
        "sav/cfg/rec/dat records; format support unassessed, stock-install meaning unverified"
            .into(),
    );
    f
}

/// Menu/HUD/camera/effects data outside the spec families.
fn interface(paths: &[String]) -> Family {
    let mut f = Family::new("interface & effects data");
    let mut count = 0usize;
    for p in paths {
        let is = p.starts_with("tune/camera/")
            || p.starts_with("tune/effects/")
            || p.ends_with(".mmhudmap")
            || basename(p).starts_with("hudmap_")
            || matches!(basename(p), "menu.csv" | "widget.csv")
            || (p.ends_with(".cinfo") && p.starts_with("tune/"));
        if is {
            count += 1;
        }
    }
    f.discovered = count;
    f.extras = count;
    f.unverified = count;
    f.notes.push(
        "camera tracks, particle birth rules, hudmap geometry, menu/widget/cinfo records — all unparsed"
            .into(),
    );
    f
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Text table.
pub fn print(report: &Report) {
    println!("== content inventory ==");
    println!("engine commit : {}", report.commit);
    println!("install       : {}", report.install.display());
    println!(
        "sources       : {} archive(s){}, loose files, {} mod(s)",
        report.archives.len(),
        if report.skipped_archives.is_empty() {
            String::new()
        } else {
            format!(", {} skipped", report.skipped_archives.len())
        },
        report.mods.len()
    );
    for (p, e) in &report.skipped_archives {
        println!("  skipped archive {}: {e}", p.display());
    }
    println!("logical paths : {}", report.logical_paths);
    println!("fingerprint   : {}", report.fingerprint);
    println!();
    println!(
        "{:<24} {:>8} {:>10} {:>8} {:>8} {:>10} {:>6}",
        "family", "expected", "discovered", "accepted", "rejected", "unverified", "extras"
    );
    for f in &report.families {
        println!(
            "{:<24} {:>8} {:>10} {:>8} {:>8} {:>10} {:>6}",
            f.name,
            f.expected,
            f.discovered,
            f.accepted,
            f.rejected.len(),
            f.unverified,
            f.extras
        );
    }
    let rejected: Vec<&Family> = report
        .families
        .iter()
        .filter(|f| !f.rejected.is_empty())
        .collect();
    if !rejected.is_empty() {
        println!("\n== rejected / missing ==");
        for f in rejected {
            println!("{}:", f.name);
            for r in &f.rejected {
                println!("  {} — {}", r.entry, r.reason);
            }
        }
    }
    println!("\n== notes ==");
    for f in &report.families {
        for n in &f.notes {
            println!("  [{}] {n}", f.name);
        }
    }
}

/// JSON rendering for runner consumption.
pub fn to_json(report: &Report) -> Value {
    json!({
        "commit": report.commit,
        "install": report.install,
        "archives": report.archives,
        "skipped_archives": report.skipped_archives,
        "mods": report.mods,
        "logical_paths": report.logical_paths,
        "fingerprint": report.fingerprint,
        "families": report.families.iter().map(|f| json!({
            "name": f.name,
            "expected": f.expected,
            "discovered": f.discovered,
            "accepted": f.accepted,
            "rejected": f.rejected.iter().map(|r| json!({
                "entry": r.entry, "reason": r.reason,
            })).collect::<Vec<_>>(),
            "unverified": f.unverified,
            "extras": f.extras,
            "notes": f.notes,
        })).collect::<Vec<_>>(),
        "strict_failures": strict_failures(report),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_assets::InstallMount;
    use std::fs;

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    /// The smallest PSDL the parser accepts: one texture slot, one room
    /// (record 0 is implied), no geometry.
    fn minimal_psdl() -> Vec<u8> {
        let mut d = b"PSD0".to_vec();
        for v in [0u32, 0, 0, 1, 1, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0, 0]); // room flag + prop rule
        for _ in 0..10 {
            d.extend_from_slice(&0f32.to_le_bytes());
        }
        d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
        d
    }

    /// A dependency-complete fake vehicle id (presence checks only).
    fn write_vehicle(dir: &Path, id: &str) {
        write(
            dir,
            &format!("tune/{id}.info"),
            b"Description = Test\nColors = Red|Blue\n",
        );
        write(dir, &format!("tune/vehicle/{id}.vehcarsim"), b"");
        write(dir, &format!("geometry/{id}.pkg"), b"");
        write(dir, &format!("bound/{id}_bound.bnd"), b"");
        write(dir, &format!("geometry/{id}_whl0.mtx"), b"");
    }

    fn build_report(dir: &Path) -> Report {
        let mut vfs = Vfs::new();
        let mount = mm2_assets::mount_install(&mut vfs, dir, &InstallMount::default()).unwrap();
        build(&vfs, &mount, dir).unwrap()
    }

    fn family<'a>(report: &'a Report, name: &str) -> &'a Family {
        report
            .families
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("family {name} missing"))
    }

    #[test]
    fn race_file_classification() {
        assert_eq!(
            classify_race_file("blitz0.aimap"),
            (Kind::Aimap, Some("blitz0".into()))
        );
        assert_eq!(
            classify_race_file("roam.aimap_p"),
            (Kind::AimapP, Some("roam".into()))
        );
        assert_eq!(
            classify_race_file("circuit0-a-3.opp"),
            (Kind::Opp, Some("circuit0".into()))
        );
        assert_eq!(
            classify_race_file("circuit0-p-7.opp"),
            (Kind::Opp, Some("circuit0".into()))
        );
        assert_eq!(
            classify_race_file("exam1_2.opp"),
            (Kind::Opp, Some("exam1_2".into()))
        );
        assert_eq!(
            classify_race_file("follow-0.opp"),
            (Kind::Opp, Some("follow".into()))
        );
        assert_eq!(
            classify_race_file("final.opp"),
            (Kind::Opp, Some("final".into()))
        );
        assert_eq!(
            classify_race_file("crash0data_p.csv"),
            (Kind::DataCsv, Some("crash0".into()))
        );
        assert_eq!(
            classify_race_file("blitz0waypoints.csv"),
            (Kind::Waypoints, Some("blitz0".into()))
        );
        assert_eq!(
            classify_race_file("cir1_strtpnts"),
            (Kind::StartPoints, Some("cir1".into()))
        );
        assert_eq!(
            classify_race_file("london_bridge_multi.pathset"),
            (Kind::Pathset, Some("london_bridge_multi".into()))
        );
        assert_eq!(
            classify_race_file("reverse180_p.csv"),
            (Kind::Csv, Some("reverse180_p".into()))
        );
        assert_eq!(classify_race_file("mmracedata.csv").0, Kind::Meta);
        assert_eq!(classify_race_file(".#race6.opp.1.1").0, Kind::Junk);
        assert_eq!(classify_race_file("crash12data.csv.old").0, Kind::Junk);
        assert_eq!(classify_race_file("blitz12waypoints.csvs").0, Kind::Junk);
        assert_eq!(classify_race_file("dbugps2.ps2").0, Kind::Junk);
    }

    #[test]
    fn synthetic_install_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "city/london.psdl", &minimal_psdl());
        write(d, "city/sf.psdl", &minimal_psdl());
        write(d, "city/sfai.psdl", &minimal_psdl());
        write(d, "city/london.inst", b"");
        write(d, "city/sf.inst", b"");
        write(d, "city/sf/props.csv", b"");
        write_vehicle(d, "vpbug");
        write(d, "race/london/blitz0.aimap", b"");
        write(d, "race/london/blitz0waypoints.csv", b"");
        write(d, "race/london/crash0.aimap", b"");
        write(d, "race/london/crash0data.csv", b"");
        write(d, "race/london/exam1_1.csv", b"");
        write(d, "race/london/circuit11-a-0.opp", b"");
        write(d, "race/sf/multicopwaypoints.csv", b"");
        write(
            d,
            "race/london/mmracedata.csv",
            b"Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty\nnone,0,0,0,7,0,0.1,0.0,3,50,1,0,0,1,6,0,0.2,0.0,4,40,1\n",
        );
        write(d, "race/london/mmcrashdata.csv", b"not,a,table\n");
        write(d, "anim/pedmodel_man.mod", b"");
        write(d, "anim/pedmodel_man.skel", b"");
        write(d, "anim/pedmodel_man.rays", b"");
        write(d, "anim/pedmodel_man.shaders", b"");
        write(d, "anim/cvs/entries", b"");
        write(d, "aud/aud11/engine.wav", b"");

        let report = build_report(d);

        let cities = family(&report, "cities");
        assert_eq!(
            (
                cities.expected,
                cities.discovered,
                cities.accepted,
                cities.extras
            ),
            (2, 3, 3, 1)
        );
        assert!(cities.rejected.is_empty());

        let vehicles = family(&report, "player vehicles");
        assert_eq!(
            (vehicles.expected, vehicles.discovered, vehicles.accepted),
            (21, 1, 1)
        );
        // 20 stock cars missing.
        assert_eq!(vehicles.rejected.len(), 20);

        let races = family(&report, "races");
        assert_eq!(races.expected, 80);
        // london: blitz0 full, circuit11 partial; sf: nothing.
        assert_eq!(races.accepted, 1);
        assert!(
            races
                .rejected
                .iter()
                .any(|r| r.entry == "race/london/circuit11" && r.reason.starts_with("partial"))
        );
        assert!(
            races
                .rejected
                .iter()
                .any(|r| r.entry == "race/sf/blitz0" && r.reason.contains("not discovered"))
        );
        // Event-metadata tables: the well-formed one reports its row
        // count, missing tables reject as undiscovered expected data.
        assert!(
            races
                .notes
                .iter()
                .any(|n| n.contains("mmracedata.csv=1 rows"))
        );
        assert!(races.rejected.iter().any(
            |r| r.entry == "race/london/mmblitzdata.csv" && r.reason.contains("not discovered")
        ));

        let lessons = family(&report, "crash-course lessons");
        assert_eq!(lessons.expected, 42);
        assert_eq!(lessons.accepted, 2); // london crash0 + exam (prefix)
        assert!(
            lessons
                .rejected
                .iter()
                .all(|r| !r.reason.starts_with("partial"))
        );
        // The malformed Crash Course table rejects on the lessons family.
        assert!(
            lessons
                .rejected
                .iter()
                .any(|r| r.entry == "race/london/mmcrashdata.csv"
                    && r.reason.contains("malformed event-metadata table"))
        );

        let placement = family(&report, "placement sources");
        assert_eq!(
            (placement.expected, placement.discovered, placement.accepted),
            (2, 2, 2)
        );
        assert_eq!(placement.unverified, 1); // sf/props.csv

        let peds = family(&report, "pedestrian archetypes");
        assert_eq!((peds.expected, peds.discovered, peds.accepted), (4, 1, 1));
        assert!(
            peds.rejected
                .iter()
                .any(|r| r.entry.starts_with("anim/cvs"))
        );

        let audio = family(&report, "audio families");
        assert_eq!(
            (audio.expected, audio.discovered, audio.accepted),
            (7, 1, 1)
        );
        assert_eq!(audio.rejected.len(), 6);

        let mp = family(&report, "multiplayer variants");
        assert_eq!(mp.discovered, 1); // multicopwaypoints.csv
        assert_eq!(mp.unverified, 1);

        // Strict must fail: expected-but-missing entries exist.
        assert!(!strict_failures(&report).is_empty());
    }

    #[test]
    fn empty_install_never_passes_strict() {
        let tmp = tempfile::tempdir().unwrap();
        let report = build_report(tmp.path());
        let failures = strict_failures(&report);
        // Every expected denominator (cities, vehicles, races, lessons,
        // placement, audio, peds) reports an empty catalog.
        for name in [
            "cities",
            "player vehicles",
            "races",
            "crash-course lessons",
            "placement sources",
            "audio families",
            "pedestrian archetypes",
        ] {
            assert!(
                failures.iter().any(|f| f.starts_with(&format!("{name}:"))),
                "missing strict failure for {name}"
            );
        }
    }

    #[test]
    fn fingerprint_changes_with_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "city/london.psdl", &minimal_psdl());
        let fp1 = build_report(d).fingerprint;
        let fp2 = build_report(d).fingerprint;
        assert_eq!(fp1, fp2, "same catalog must fingerprint identically");
        write(d, "city/sf.psdl", &minimal_psdl());
        assert_ne!(build_report(d).fingerprint, fp1);
    }
}

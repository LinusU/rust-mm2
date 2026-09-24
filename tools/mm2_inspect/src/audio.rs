//! `audio` audit (F07-A.1): census every `aud/**` file, decode the waves
//! through the production RIFF/WAVE parser, parse the `aud/cardata` and
//! `aud/ambient` tables through the production cardata grammars, and
//! cross-check every referenced sample name against the discovered wave
//! stems.
//!
//! Families owned by other features stay in the denominator but are
//! only classified: `aud/spchdata`/`aud/creaturedata` (speech and
//! pedestrian voices, F08) are probed text-vs-binary, and the
//! `aud/dmusic` RIFF containers (`DMSG`/`DMST`/`DLS `/`DMBD`) are
//! classified by form word — DirectMusic playback is not decoded here.
//!
//! `--strict` exits nonzero on parse failures and validation issues.
//! Dead wave references, vehicle-coverage gaps and authored quirks like
//! the `engineparams*` binary name prefixes are reported findings —
//! retail ships them, so strict stays a regression gate, not a verdict
//! on authored data.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mm2_assets::Vfs;
use mm2_formats::cardata::{self, CardataBody, CardataIssue};
use mm2_formats::wav::{Wav, riff_form_type};

/// Everything the audit measures, kept separate from printing so tests
/// can exercise it on synthetic installs.
#[derive(Default)]
pub struct AudioReport {
    /// `aud/**` files examined (directory index entries excluded).
    pub files: usize,
    /// `aud/**` directory index entries skipped.
    pub dirs: usize,
    /// `.wav` files that decoded.
    pub waves: usize,
    /// PCM census: `(sample_rate, channels, bits)` → file count.
    pub wave_census: BTreeMap<(u32, u16, u16), usize>,
    /// Non-PCM or unusual waves: `tag` → file count.
    pub wave_tags: BTreeMap<u16, usize>,
    /// Waves per `aud/<family>` directory.
    pub wave_dirs: BTreeMap<String, usize>,
    /// Total decoded PCM payload.
    pub wave_bytes: u64,
    /// Total decoded playtime at authored rates.
    pub wave_seconds: f64,
    /// Non-WAVE RIFF containers by form word (`DMSG`, `DLS `, …).
    pub riff_forms: BTreeMap<String, usize>,
    /// RIFF containers whose form word disagrees with the extension.
    pub riff_mismatches: Vec<(String, String)>,
    /// One summary line per parsed cardata/object-audio table.
    pub tables: Vec<(String, String)>,
    /// Cardata files by grammar.
    pub cardata_kinds: BTreeMap<&'static str, usize>,
    /// Deferred `aud/**` CSVs that are plain text (F08 families).
    pub deferred_text: usize,
    /// Deferred text CSVs per `aud/<family>` directory.
    pub deferred_dirs: BTreeMap<String, usize>,
    /// Deferred `aud/**` CSVs carrying binary content — a `.csv`
    /// extension on non-text data is an authored quirk worth naming.
    pub deferred_binary: Vec<String>,
    /// Files whose extension has no audio interpretation (`.bat` work
    /// files, etc.).
    pub extras: BTreeMap<String, usize>,
    /// Files that failed to parse or read: `(path, error)`.
    pub failures: Vec<(String, String)>,
    /// Validation issues and parse diagnostics: `"path: issue"`.
    pub issues: Vec<String>,
    /// Authored quirks that are properties of the shipped data, not
    /// defects (e.g. binary name prefixes).
    pub quirks: Vec<String>,
    /// Findings: dead wave references, coverage gaps.
    pub findings: Vec<String>,
    /// Distinct sample names referenced by parsed tables.
    pub refs_total: usize,
    /// …of which resolve to a discovered wave stem.
    pub refs_resolved: usize,
    /// Unresolved references: name → referring tables.
    pub dead_refs: Vec<(String, Vec<String>)>,
}

/// The RIFF form word each DirectMusic extension should carry.
fn expected_riff_form(ext: &str) -> Option<&'static [u8; 4]> {
    match ext {
        "sgt" => Some(b"DMSG"),
        "sty" => Some(b"DMST"),
        "dls" => Some(b"DLS "),
        "bnd" => Some(b"DMBD"),
        _ => None,
    }
}

/// A wave's lookup stem: basename minus `.wav`, minus a `.<n>k` rate
/// suffix (`vwidle.22k.wav` → `vwidle`). Retail ships the same sample at
/// 11 kHz and 22 kHz under parallel directories; cardata references
/// carry neither suffix.
fn wave_stem(logical: &str) -> String {
    let base = logical.rsplit('/').next().unwrap_or(logical);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let stem = stem
        .rsplit_once('.')
        .and_then(|(s, sfx)| {
            (sfx.len() >= 2
                && sfx.ends_with(['k', 'K'])
                && sfx[..sfx.len() - 1].bytes().all(|b| b.is_ascii_digit()))
            .then_some(s)
        })
        .unwrap_or(stem);
    stem.to_ascii_lowercase()
}

fn kind_name(kind: cardata::CardataKind) -> &'static str {
    use cardata::CardataKind as K;
    match kind {
        K::CarAudio => "car audio",
        K::EngineParams => "engineparams",
        K::ImpactTable => "impacts",
        K::SurfaceTable => "surfaces",
        K::SirenProgram => "sirens",
        K::AmbientEngine => "ambient engine",
        K::AmbientHorn => "ambient horn",
        K::ObjectAudio => "object audio",
        K::AmbientContainer => "ambient container",
        K::SemiData => "semidata",
        K::VehTypes => "vehtypes",
        K::SuspensionAudio => "suspension",
        K::TireWobble => "tirewobble",
    }
}

/// One-line summary of a parsed cardata body for the per-file listing.
fn summarize(body: &CardataBody) -> String {
    match body {
        CardataBody::Car(b) => format!(
            "horn={}, clutch={}, {} engine sample(s) ({} declared)",
            b.horn.name,
            b.clutch.name,
            b.engine_samples.len(),
            b.declared_engine_samples
        ),
        CardataBody::EngineParams(b) => {
            let binary = b.rows.iter().filter(|r| r.name_text().is_none()).count();
            format!(
                "{} row(s), {binary} with binary name prefixes",
                b.rows.len()
            )
        }
        CardataBody::Impacts(b) => format!(
            "{} categories, {} samples",
            b.categories.len(),
            b.categories.iter().map(|c| c.samples.len()).sum::<usize>()
        ),
        CardataBody::Surfaces(b) => format!(
            "tunnel {}, {} surface(s)",
            b.tunnel_index
                .map(|i| i.to_string())
                .unwrap_or_else(|| "?".into()),
            b.surfaces.len()
        ),
        CardataBody::Sirens(b) => format!(
            "{} sequence(s), {} step(s){}",
            b.samples.len(),
            b.samples.iter().map(|s| s.steps.len()).sum::<usize>(),
            if b.explosion.is_some() {
                ", +explosion"
            } else {
                ""
            }
        ),
        CardataBody::AmbientEngine(b) => {
            format!("sample={}, {} band(s)", b.sample.name, b.ranges.len())
        }
        CardataBody::AmbientHorn(b) => {
            format!("horn={}, {} sequence(s)", b.horn.name, b.sequences.len())
        }
        CardataBody::Object(b) => format!(
            "{} clip(s), {} emitter point(s)",
            b.samples.len(),
            b.vector_points.len()
        ),
        CardataBody::Container(b) => format!("{} member(s): {}", b.files.len(), b.files.join(", ")),
        CardataBody::Semi(b) => format!("reverse={}, air={}", b.reverse_sample, b.air_blow_sample),
        CardataBody::VehTypes(b) => format!("{} group(s)", b.groups.len()),
        CardataBody::Bands(b) => format!("{} row(s)", b.rows.len()),
    }
}

/// Run the audit over a mounted install. Reads every `aud/**` file once.
pub fn audit(vfs: &Vfs) -> AudioReport {
    let mut r = AudioReport::default();
    // Wave stems discovered under aud/ (case-insensitive), name →
    // referring cardata tables, and vehicle-id coverage sets.
    let mut stems: BTreeSet<String> = BTreeSet::new();
    let mut refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut player_ids: BTreeSet<String> = BTreeSet::new();
    let mut opponent_ids: BTreeSet<String> = BTreeSet::new();

    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.starts_with("aud/"))
        .collect();
    logicals.sort();

    for logical in &logicals {
        let name = logical.rsplit('/').next().unwrap_or(logical);
        // Archive directory index entries carry no extension.
        if !name.contains('.') {
            r.dirs += 1;
            continue;
        }
        r.files += 1;
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let bytes = match vfs.read_logical(logical) {
            Ok(b) => b,
            Err(e) => {
                r.failures.push((logical.clone(), e.to_string()));
                continue;
            }
        };
        if bytes.is_empty() {
            continue; // zero-length archive entries occur in retail
        }
        match ext.as_str() {
            "wav" => {
                let dir = logical
                    .rsplit_once('/')
                    .map(|(d, _)| d)
                    .unwrap_or("")
                    .to_string();
                *r.wave_dirs.entry(dir).or_default() += 1;
                stems.insert(wave_stem(logical));
                match Wav::parse(&bytes) {
                    Ok(w) => {
                        r.waves += 1;
                        r.wave_bytes += w.pcm.len() as u64;
                        r.wave_seconds += w.duration_secs();
                        *r.wave_tags.entry(w.fmt.tag).or_default() += 1;
                        if w.fmt.tag == mm2_formats::wav::FORMAT_PCM {
                            *r.wave_census
                                .entry((w.fmt.sample_rate, w.fmt.channels, w.fmt.bits_per_sample))
                                .or_default() += 1;
                        }
                        for i in w.validate() {
                            r.issues.push(format!("{logical}: {i:?}"));
                        }
                    }
                    Err(e) => r.failures.push((logical.clone(), e.to_string())),
                }
            }
            "sgt" | "sty" | "dls" | "bnd" => match riff_form_type(&bytes) {
                Some(form) => {
                    let form_s = String::from_utf8_lossy(&form).into_owned();
                    *r.riff_forms.entry(form_s.clone()).or_default() += 1;
                    if let Some(want) = expected_riff_form(&ext)
                        && &form != want
                    {
                        r.riff_mismatches.push((
                            logical.clone(),
                            format!("expected {}, found {form_s}", String::from_utf8_lossy(want)),
                        ));
                    }
                }
                None => r
                    .failures
                    .push((logical.clone(), "not a RIFF container".into())),
            },
            "csv" => match cardata::classify(logical) {
                Some(kind) => match cardata::parse(logical, &bytes) {
                    Ok(f) => {
                        *r.cardata_kinds.entry(kind_name(kind)).or_default() += 1;
                        for d in f.body.diagnostics() {
                            r.issues
                                .push(format!("{logical}: line {}: {}", d.line, d.message));
                        }
                        for i in f.body.validate() {
                            match i {
                                // The binary name prefix is the authored
                                // engineparams shape, not a defect.
                                CardataIssue::BinaryNameField { .. } => {
                                    r.quirks.push(format!("{logical}: {i:?}"))
                                }
                                // Retail's declared row/sample counts
                                // drift from the parsed rows in several
                                // tables — an authored property, so a
                                // quirk rather than a strict failure.
                                CardataIssue::DeclaredVsParsed { .. } => {
                                    r.quirks.push(format!("{logical}: {i:?}"))
                                }
                                _ => r.issues.push(format!("{logical}: {i:?}")),
                            }
                        }
                        for name in f.body.wave_names() {
                            refs.entry(name.to_ascii_lowercase())
                                .or_default()
                                .push(logical.clone());
                        }
                        // Container member tables must exist as siblings.
                        if let CardataBody::Container(c) = &f.body {
                            for member in &c.files {
                                let target = format!("aud/ambient/{member}.csv");
                                if logicals.binary_search(&target).is_err() {
                                    r.issues.push(format!(
                                        "{logical}: container member {target} not found"
                                    ));
                                }
                            }
                        }
                        r.tables.push((logical.clone(), summarize(&f.body)));
                        // Vehicle-id coverage sets — `vp*.csv` only:
                        // `copy of`/`*.wrk` work files are extras.
                        if let Some(stem) = name.strip_suffix(".csv")
                            && stem.starts_with("vp")
                            && !stem.contains('.')
                            && !name.contains(' ')
                        {
                            if logical.starts_with("aud/cardata/player/") {
                                player_ids.insert(stem.to_string());
                            } else if logical.starts_with("aud/cardata/opponent/") {
                                opponent_ids.insert(stem.to_string());
                            }
                        }
                    }
                    Err(e) => r.failures.push((logical.clone(), e.to_string())),
                },
                // spchdata/creaturedata/dmusic csv_files: F08 families —
                // counted, probed text-vs-binary, not parsed here.
                None => match std::str::from_utf8(&bytes) {
                    Ok(_) => {
                        r.deferred_text += 1;
                        let fam = logical.split('/').nth(1).unwrap_or("").to_string();
                        *r.deferred_dirs.entry(fam).or_default() += 1;
                    }
                    Err(_) => r.deferred_binary.push(logical.clone()),
                },
            },
            _ => *r.extras.entry(ext).or_default() += 1,
        }
    }

    // Sample-name → wave-stem cross-check.
    r.refs_total = refs.len();
    for (name, users) in &refs {
        if stems.contains(name.as_str()) {
            r.refs_resolved += 1;
        } else {
            r.dead_refs.push((name.clone(), users.clone()));
        }
    }
    for (path, what) in &r.riff_mismatches {
        r.issues
            .push(format!("{path}: RIFF form mismatch ({what})"));
    }

    // Cardata vehicle-id coverage vs the tune roster.
    let tune_ids: BTreeSet<String> = vfs
        .list()
        .iter()
        .filter_map(|p| {
            p.strip_prefix("tune/")
                .and_then(|n| n.strip_suffix(".info"))
                .filter(|s| !s.contains('/'))
                .map(|s| s.to_string())
        })
        .collect();
    let uncovered_p: Vec<String> = tune_ids
        .iter()
        .filter(|id| id.starts_with("vp") && !player_ids.contains(*id))
        .cloned()
        .collect();
    let uncovered_o: Vec<String> = tune_ids
        .iter()
        .filter(|id| id.starts_with("vp") && !opponent_ids.contains(*id))
        .cloned()
        .collect();
    if !uncovered_p.is_empty() {
        r.findings.push(format!(
            "{} tune id(s) without player cardata: {}",
            uncovered_p.len(),
            uncovered_p.join(", ")
        ));
    }
    if !uncovered_o.is_empty() {
        r.findings.push(format!(
            "{} tune id(s) without opponent cardata: {}",
            uncovered_o.len(),
            uncovered_o.join(", ")
        ));
    }
    let orphans: Vec<String> = player_ids
        .union(&opponent_ids)
        .filter(|id| !tune_ids.contains(*id))
        .cloned()
        .collect();
    if !orphans.is_empty() {
        r.findings.push(format!(
            "{} cardata id(s) with no tune entry: {}",
            orphans.len(),
            orphans.join(", ")
        ));
    }
    r
}

/// Print the report; `strict` fails on failures and issues (authored
/// quirks and findings are reported but do not fail strict).
pub fn print_report(r: &AudioReport) {
    println!("== audio tables (aud/cardata/**, aud/ambient/**) ==");
    for (path, summary) in &r.tables {
        println!("  {path:<62} {summary}");
    }
    if !r.deferred_dirs.is_empty() {
        println!("  deferred text csv (F08 families):");
        for (dir, n) in &r.deferred_dirs {
            println!("    aud/{dir}: {n}");
        }
    }
    if !r.deferred_binary.is_empty() {
        println!("  binary .csv blobs (F08-deferred):");
        for p in &r.deferred_binary {
            println!("    {p}");
        }
    }

    println!("== waves ==");
    for ((rate, ch, bits), n) in &r.wave_census {
        println!("  PCM {rate} Hz {ch}ch {bits}bit: {n} file(s)");
    }
    for (tag, n) in &r.wave_tags {
        if *tag != mm2_formats::wav::FORMAT_PCM {
            println!("  format tag {tag:#06x} (non-PCM): {n} file(s)");
        }
    }
    for (dir, n) in &r.wave_dirs {
        println!("  {dir}: {n}");
    }
    println!(
        "  {} decoded, {} bytes PCM, {:.1}s total",
        r.waves, r.wave_bytes, r.wave_seconds
    );

    println!("== DirectMusic containers (recognized, not decoded) ==");
    for (form, n) in &r.riff_forms {
        println!("  RIFF form {form:?}: {n} file(s)");
    }
    if !r.extras.is_empty() {
        println!(
            "  extras (no audio interpretation): {}",
            r.extras
                .iter()
                .map(|(e, n)| format!("{e}={n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    println!("== cross-checks ==");
    println!(
        "  sample references: {}/{} resolved to discovered wave stems",
        r.refs_resolved, r.refs_total
    );
    for (name, users) in &r.dead_refs {
        println!(
            "    dead ref {name:?} ← {}",
            users.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
        );
    }

    if !r.failures.is_empty() {
        println!("== failures ==");
        for (p, e) in &r.failures {
            println!("  {p}: {e}");
        }
    }
    if !r.issues.is_empty() {
        println!("== issues ==");
        for i in &r.issues {
            println!("  {i}");
        }
    }
    if !r.quirks.is_empty() {
        println!("== authored quirks ==");
        for i in &r.quirks {
            println!("  {i}");
        }
    }
    if !r.findings.is_empty() {
        println!("== findings ==");
        for i in &r.findings {
            println!("  {i}");
        }
    }

    println!(
        "== summary: {} files ({} dir entries), {} waves, {} tables, {} deferred text csv, {} binary csv, {} extras, {} failures, {} issues, {} quirks, {} findings, {} dead refs",
        r.files,
        r.dirs,
        r.waves,
        r.tables.len(),
        r.deferred_text,
        r.deferred_binary.len(),
        r.extras.values().sum::<usize>(),
        r.failures.len(),
        r.issues.len(),
        r.quirks.len(),
        r.findings.len(),
        r.dead_refs.len(),
    );
}

/// `mm2-inspect audio <dir> [--strict]`.
pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = crate::build_vfs(dir, mods)?;
    let report = audit(&vfs);
    if report.files == 0 {
        return Err("audio audit: no aud/ files discovered".into());
    }
    print_report(&report);
    if strict && !(report.failures.is_empty() && report.issues.is_empty()) {
        return Err(format!(
            "strict audio audit: {} failures, {} issues",
            report.failures.len(),
            report.issues.len()
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

    fn pcm_wav(rate: u32, frames: usize) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * 2).to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let data = vec![0u8; frames * 2];
        let mut body = Vec::from(&b"WAVE"[..]);
        for (id, payload) in [(&b"fmt "[..], &fmt[..]), (&b"data"[..], &data[..])] {
            body.extend_from_slice(id);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        }
        let mut out = Vec::from(&b"RIFF"[..]);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    const CAR_CSV: &[u8] = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nTESTHORN,0.9,0,1,REVERSE,0.5\nEngine wave name,a,b,c,d,e,f,g,h,i,j\nTESTIDLE,0.1,0.2,1,2,3,4,0.5,1,0,9\n";

    #[test]
    fn stem_strips_rate_suffix() {
        assert_eq!(wave_stem("aud/aud22/vwidle.22k.wav"), "vwidle");
        assert_eq!(wave_stem("aud/aud11/vwidle.11k.wav"), "vwidle");
        assert_eq!(wave_stem("aud/aud11/tireskid1_ps1.wav"), "tireskid1_ps1");
        assert_eq!(wave_stem("aud/aud11/VWHORN.WAV"), "vwhorn");
        // A dotted name that is not a rate suffix keeps its stem.
        assert_eq!(wave_stem("aud/aud11/foo.bar.wav"), "foo.bar");
    }

    #[test]
    fn audit_counts_waves_tables_and_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/aud22/testhorn.wav", &pcm_wav(22050, 100));
        write(d, "aud/aud11/testidle.11k.wav", &pcm_wav(11025, 50));
        write(d, "aud/cardata/player/vpbug.csv", CAR_CSV);
        write(d, "tune/vpbug.info", b"Description = Test\n");
        // A dmusic segment and a deferred speech table.
        let mut sgt = Vec::from(&b"RIFF"[..]);
        sgt.extend_from_slice(&8u32.to_le_bytes());
        sgt.extend_from_slice(b"DMSG");
        write(d, "aud/dmusic/x.sgt", &sgt);
        write(d, "aud/spchdata/al1/blitz.csv", b"header\n");

        let r = audit(&vfs_of(d));
        assert_eq!(r.waves, 2);
        assert_eq!(r.wave_census[&(22050, 1, 16)], 1);
        assert_eq!(r.wave_census[&(11025, 1, 16)], 1);
        assert_eq!(r.riff_forms["DMSG"], 1);
        assert_eq!(r.deferred_text, 1);
        assert_eq!(r.tables.len(), 1);
        assert_eq!(r.cardata_kinds["car audio"], 1);
        // TESTHORN + REVERSE + TESTIDLE referenced; testhorn and
        // testidle resolve, REVERSE does not.
        assert_eq!(r.refs_total, 3);
        assert_eq!(r.refs_resolved, 2);
        assert_eq!(r.dead_refs.len(), 1);
        assert_eq!(r.dead_refs[0].0, "reverse");
        assert!(r.failures.is_empty());
    }

    #[test]
    fn audit_reports_failures_and_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/aud11/broken.wav", b"not a riff at all");
        // A .sty carrying a segment form word is a mismatch.
        let mut sty = Vec::from(&b"RIFF"[..]);
        sty.extend_from_slice(&8u32.to_le_bytes());
        sty.extend_from_slice(b"DMSG");
        write(d, "aud/dmusic/x.sty", &sty);
        // A binary blob masquerading as csv (creaturedata shape).
        write(d, "aud/creaturedata/voice.csv", &[0xf3, 0xcd, 0xcc, 0x53]);
        write(d, "aud/cardata/player/vpx.bat", b"echo hi\n");

        let r = audit(&vfs_of(d));
        assert_eq!(r.failures.len(), 1);
        assert!(r.failures[0].0.ends_with("broken.wav"));
        assert_eq!(r.riff_mismatches.len(), 1);
        assert_eq!(r.deferred_binary.len(), 1);
        assert_eq!(r.extras["bat"], 1);
    }

    #[test]
    fn coverage_reports_uncovered_and_orphan_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/cardata/player/vpbug.csv", CAR_CSV);
        write(d, "tune/vpbug.info", b"Description = a\n");
        write(d, "tune/vpford.info", b"Description = b\n");
        // Opponent cardata for an id with no tune entry.
        write(d, "aud/cardata/opponent/vpghost.csv", CAR_CSV);
        let r = audit(&vfs_of(d));
        assert!(
            r.findings
                .iter()
                .any(|f| f.contains("without player cardata") && f.contains("vpford"))
        );
        assert!(
            r.findings
                .iter()
                .any(|f| f.contains("without opponent cardata") && f.contains("vpbug"))
        );
        assert!(
            r.findings
                .iter()
                .any(|f| f.contains("no tune entry") && f.contains("vpghost"))
        );
    }
}

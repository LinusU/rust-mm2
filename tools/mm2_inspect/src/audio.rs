//! `audio` audit (F07-A.1): census every `aud/**` file, decode the waves
//! through the production RIFF/WAVE parser, parse the `aud/cardata` and
//! `aud/ambient` tables through the production cardata grammars, and
//! cross-check every referenced sample name against the discovered wave
//! stems.
//!
//! Families owned by other features stay in the denominator but are
//! only classified: `aud/spchdata` (speech, F08) is probed text-vs-binary,
//! `aud/creaturedata` (ped/ambient voices) is parsed
//! (`mm2_formats::creaturedata`) with supported/unsupported/missing
//! counts, and the
//! `aud/dmusic` RIFF containers (`DMSG`/`DMST`/`DLS `/`DMBD`) are
//! parsed structurally (`mm2_formats::dmus`: tracks, band instruments,
//! DLS waves, `DMRF` references resolved against the shipped files and
//! walked from the music-table stems) — DirectMusic playback is not
//! implemented.
//! The five `aud/dmusic/csv_files` music cue tables are parsed
//! (`mm2_formats::music`) and every segment/style/band stem they name
//! is cross-checked against the shipped containers, with the
//! containers no table names reported as a finding.
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
use mm2_formats::creaturedata::{self, CreatureKind, VoiceTable};
use mm2_formats::dmus::{DmContainer, DmKind};
use mm2_formats::music::{self, MusicTable};
use mm2_formats::spchdata::{AnnouncerIndex, CueTable};
use mm2_formats::wav::{Wav, lookup_stem, riff_form_type};

/// What the retail speech tables author under one section name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SpeechSection {
    /// Tables carrying the section.
    pub tables: usize,
    /// Cue rows authored under it.
    pub rows: usize,
    /// Distinct drawable wave stems.
    pub clips: BTreeSet<String>,
    /// …of which no shipped wave answers (filled after the wave scan).
    pub missing: usize,
}

/// Sections the runtime binds to a trigger (`mm2_app::audio`); every
/// other authored section is unbound and its trigger unknown (UNK-25).
fn speech_section_bound(name: &str) -> bool {
    matches!(
        name,
        "WEATHER" | "TIMEOFDAY" | "PRERACE" | "FINALCHECKPOINT"
    ) || name.starts_with("RESULTS")
}

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
    /// `aud/creaturedata` files parsed without diagnostics.
    pub creature_supported: usize,
    /// `aud/creaturedata` files that failed or carry diagnostics:
    /// `(path, why)`. Also listed in `failures`/`issues`.
    pub creature_unsupported: Vec<(String, String)>,
    /// Distinct creaturedata wave references with no shipped wave:
    /// `(stem, naming tables)`.
    pub creature_missing: Vec<(String, Vec<String>)>,
    /// `aud/spchdata` cue tables and registries that parsed cleanly.
    pub speech_supported: usize,
    /// `aud/spchdata` files that failed or carry diagnostics:
    /// `(path, why)`. A diagnostic is an authored quirk, so these are
    /// listed in `quirks` (a hard parse failure also lands in `failures`).
    pub speech_unsupported: Vec<(String, String)>,
    /// Distinct cue wave stems the speech tables can draw (every
    /// `add+1..=end` suffix of every usable row).
    pub speech_clips_total: usize,
    /// …of which resolve to a shipped wave.
    pub speech_clips_resolved: usize,
    /// Cue wave stems with no shipped wave: `(stem, naming table)`.
    pub speech_clips_missing: Vec<(String, String)>,
    /// Per authored cue section (the `<NAME> header` marker, upper-cased)
    /// across every speech table: tables, cue rows and drawable clips.
    pub speech_sections: BTreeMap<String, SpeechSection>,
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
    /// One summary line per parsed `aud/dmusic/csv_files` music table.
    pub music_tables: Vec<(String, String)>,
    /// Distinct `(stem, extension)` DirectMusic artifacts the music
    /// tables name.
    pub music_refs_total: usize,
    /// …of which resolve to a shipped `aud/dmusic/<stem>.<ext>`.
    pub music_refs_resolved: usize,
    /// Unresolved music references: `stem.ext` → naming tables.
    pub dead_music_refs: Vec<(String, Vec<String>)>,
    /// Shipped `.sgt`/`.sty`/`.bnd` files no music table names.
    pub unreferenced_music: Vec<String>,
    /// DirectMusic containers that parsed structurally.
    pub dm_parsed: usize,
    /// Segment track kinds across every parsed segment → track count.
    pub dm_tracks: BTreeMap<String, usize>,
    /// Segments missing one of the usual track kinds (`DM_USUAL_TRACKS`):
    /// track kind → segment paths. Informational, since a segment may
    /// legitimately omit one; it exists so a "every segment carries X"
    /// claim can be checked against the census.
    pub dm_segments_without: BTreeMap<&'static str, Vec<String>>,
    /// Band instruments across every container.
    pub dm_instruments: usize,
    /// `DMRF` references authored across all containers.
    pub dm_refs_total: usize,
    /// …of which name a shipped `aud/dmusic` file.
    pub dm_refs_resolved: usize,
    /// Unresolved `DMRF` references: `(owner path, referenced file)`.
    pub dead_dm_refs: Vec<(String, String)>,
    /// Shipped containers (any kind) that no music-table stem reaches,
    /// directly or through `DMRF` references.
    pub dm_unreached: Vec<String>,
    /// DLS banks parsed.
    pub dls_banks: usize,
    /// DLS instruments across all banks.
    pub dls_instruments: usize,
    /// DLS wave-pool entries across all banks.
    pub dls_waves: usize,
    /// DLS wave census: `(sample_rate, channels, bits)` of the waves that
    /// are uncompressed 16-bit PCM → count.
    pub dls_wave_census: BTreeMap<(u32, u16, u16), usize>,
    /// DLS waves that are not decodable PCM16.
    pub dls_unsupported_waves: usize,
    /// Total DLS PCM16 playtime at authored rates.
    pub dls_seconds: f64,
    /// Distinct sample names referenced by parsed tables.
    pub refs_total: usize,
    /// …of which resolve to a discovered wave stem.
    pub refs_resolved: usize,
    /// Unresolved references: name → referring tables.
    pub dead_refs: Vec<(String, Vec<String>)>,
}

/// One audio family's coverage line (F08-AC04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyRow {
    /// Family label.
    pub family: &'static str,
    /// Files in the family.
    pub files: usize,
    /// Files with sample decode (or parse) success *and* a runtime
    /// consumer that plays them.
    pub supported: usize,
    /// Files that parse or decode but have no playback semantics yet,
    /// or that carry parse diagnostics.
    pub unsupported: usize,
    /// References the family authors that name no shipped file.
    pub missing: usize,
    /// Decode/parse failures.
    pub failed: usize,
    /// What `unsupported` means for this family.
    pub note: &'static str,
}

impl AudioReport {
    /// The per-family coverage matrix, derived from the census.
    pub fn families(&self) -> Vec<FamilyRow> {
        let wave_failed = self
            .failures
            .iter()
            .filter(|(p, _)| p.ends_with(".wav"))
            .count();
        let non_pcm: usize = self
            .wave_tags
            .iter()
            .filter(|(t, _)| **t != mm2_formats::wav::FORMAT_PCM)
            .map(|(_, n)| n)
            .sum();
        let creature = self.creature_supported + self.creature_unsupported.len();
        let other_tables = self.tables.len().saturating_sub(creature);
        let other_missing = self
            .dead_refs
            .iter()
            .filter(|(_, users)| users.iter().any(|u| !u.starts_with("aud/creaturedata/")))
            .count();
        let table_failed = self
            .failures
            .iter()
            .filter(|(p, _)| p.ends_with(".csv") && !p.starts_with("aud/spchdata/"))
            .count();
        let speech_failed = self
            .failures
            .iter()
            .filter(|(p, _)| p.starts_with("aud/spchdata/"))
            .count();
        let dm_failed = self
            .failures
            .iter()
            .filter(|(p, _)| !p.ends_with(".wav") && !p.ends_with(".csv"))
            .count();
        let speech_files = self.speech_supported + self.speech_unsupported.len() + speech_failed;
        vec![
            FamilyRow {
                family: "waves (aud11/aud22)",
                files: self.waves + wave_failed,
                supported: self.waves - non_pcm.min(self.waves),
                unsupported: non_pcm,
                missing: 0,
                failed: wave_failed,
                note: "non-PCM waves",
            },
            FamilyRow {
                family: "vehicle + object tables (cardata/ambient)",
                files: other_tables + table_failed,
                supported: other_tables,
                unsupported: 0,
                missing: other_missing,
                failed: table_failed,
                note: "",
            },
            FamilyRow {
                family: "creature voices (creaturedata)",
                files: creature,
                supported: self.creature_supported,
                unsupported: self.creature_unsupported.len(),
                missing: self.creature_missing.len(),
                failed: 0,
                note: "files with diagnostics",
            },
            FamilyRow {
                family: "commentary cues (spchdata)",
                files: speech_files,
                supported: self.speech_supported,
                unsupported: self.speech_unsupported.len(),
                missing: self.speech_clips_missing.len(),
                failed: speech_failed,
                note: "tables with diagnostics",
            },
            FamilyRow {
                family: "music cue tables (dmusic/csv_files)",
                files: self.music_tables.len(),
                supported: 0,
                unsupported: self.music_tables.len(),
                missing: self.dead_music_refs.len(),
                failed: 0,
                note: "parsed; row choice and state triggers unrecovered (UNK-25)",
            },
            FamilyRow {
                family: "DirectMusic containers (dmusic)",
                files: self.dm_parsed + dm_failed,
                supported: 0,
                unsupported: self.dm_parsed,
                missing: self.dead_dm_refs.len(),
                failed: dm_failed,
                note: "structure parsed; no synthesizer, so no music plays",
            },
        ]
    }
}

/// Track kinds (`Track::kind_name`) nearly every retail segment carries.
const DM_USUAL_TRACKS: [&str; 4] = ["band", "command", "style", "tempo"];

impl AudioReport {
    /// Fold one parsed DirectMusic container into the census; returns the
    /// case-folded file names it references.
    fn absorb_dm(&mut self, logical: &str, c: &DmContainer) -> Vec<String> {
        self.dm_parsed += 1;
        for t in &c.content.tracks {
            *self.dm_tracks.entry(t.kind_name()).or_default() += 1;
        }
        if c.kind == DmKind::Segment {
            for kind in DM_USUAL_TRACKS {
                if !c.content.tracks.iter().any(|t| t.kind_name() == kind) {
                    self.dm_segments_without
                        .entry(kind)
                        .or_default()
                        .push(logical.to_string());
                }
            }
        }
        self.dm_instruments += c.content.instruments.len();
        for i in &c.issues {
            self.issues.push(format!("{logical}: {i:?}"));
        }
        if let Some(d) = &c.dls {
            self.dls_banks += 1;
            self.dls_instruments += d.instruments.len();
            self.dls_waves += d.waves.len();
            for w in &d.waves {
                let f = &w.format;
                if f.tag == mm2_formats::wav::FORMAT_PCM && f.bits_per_sample == 16 {
                    *self
                        .dls_wave_census
                        .entry((f.sample_rate, f.channels, f.bits_per_sample))
                        .or_default() += 1;
                    if let Some(frames) = w.frames()
                        && f.sample_rate != 0
                    {
                        self.dls_seconds += frames as f64 / f64::from(f.sample_rate);
                    }
                } else {
                    self.dls_unsupported_waves += 1;
                }
            }
        }
        debug_assert!(c.kind != DmKind::Dls || c.dls.is_some());
        c.references
            .iter()
            .filter_map(|r| r.file.as_deref())
            .filter(|f| !f.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    }
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
    // DirectMusic artifacts shipped (`stem.ext`, case-folded → logical
    // path) and the ones the music tables name (→ naming tables).
    let mut music_files: BTreeMap<String, String> = BTreeMap::new();
    let mut music_refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Every DirectMusic container by case-folded file name (DLS banks
    // included — `DMRF` references name them), and the references each
    // container authors: owner file name → referenced file names.
    let mut dm_files: BTreeMap<String, String> = BTreeMap::new();
    let mut dm_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Speech cue wave stem → naming table, collected while walking.
    let mut speech_cues: BTreeMap<String, String> = BTreeMap::new();

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
                stems.insert(lookup_stem(logical));
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
                    if ext != "dls" {
                        // Cues name a stem, not a path, so two containers
                        // sharing a file name make the reference ambiguous.
                        if let Some(prev) =
                            music_files.insert(name.to_ascii_lowercase(), logical.clone())
                        {
                            r.issues.push(format!(
                                "{logical}: same file name as {prev}; a music cue cannot tell them apart"
                            ));
                        }
                    }
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
                    dm_files.insert(name.to_ascii_lowercase(), logical.clone());
                    match DmContainer::parse(&bytes) {
                        Ok(c) => {
                            let edges = r.absorb_dm(logical, &c);
                            dm_edges.insert(name.to_ascii_lowercase(), edges);
                        }
                        Err(e) => r
                            .failures
                            .push((logical.clone(), format!("DirectMusic: {e}"))),
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
                // The music cue tables (F08-A): parsed, and every named
                // segment/style/band cross-checked against the shipped
                // containers below.
                None if music::is_music_table(logical) => {
                    match MusicTable::parse(&String::from_utf8_lossy(&bytes)) {
                        Ok(t) => {
                            for d in &t.diagnostics {
                                r.issues
                                    .push(format!("{logical}: line {}: {}", d.line, d.message));
                            }
                            for r_ in t.refs() {
                                music_refs
                                    .entry(format!(
                                        "{}.{}",
                                        r_.stem.to_ascii_lowercase(),
                                        r_.artifact.extension()
                                    ))
                                    .or_default()
                                    .push(logical.clone());
                            }
                            r.music_tables.push((
                                logical.clone(),
                                format!("{} row(s) × {} column(s)", t.rows.len(), t.columns.len()),
                            ));
                        }
                        Err(e) => r.failures.push((logical.clone(), e.to_string())),
                    }
                }
                // Ped/ambient voice tables (F08-B.4): grammar parsed,
                // runtime semantics unrecovered.
                None if creaturedata::classify(logical).is_some() => {
                    let text = String::from_utf8_lossy(&bytes);
                    let before = r.issues.len();
                    let summary = match creaturedata::classify(logical) {
                        Some(CreatureKind::Count) => match creaturedata::parse_count(&text) {
                            Ok(n) => Some(format!("count {n}")),
                            Err(e) => {
                                r.failures.push((logical.clone(), e.to_string()));
                                r.creature_unsupported
                                    .push((logical.clone(), e.to_string()));
                                None
                            }
                        },
                        _ => {
                            let t = VoiceTable::parse(&text);
                            for d in t.diagnostics.iter().chain(&t.validate()) {
                                r.issues
                                    .push(format!("{logical}: line {}: {}", d.line, d.message));
                            }
                            for s in t.samples() {
                                refs.entry(s.name.to_ascii_lowercase())
                                    .or_default()
                                    .push(logical.clone());
                            }
                            Some(format!(
                                "{} speed band(s), {} impact band(s), {} sample(s)",
                                t.speed_bands.len(),
                                t.impact_bands.len(),
                                t.samples().count()
                            ))
                        }
                    };
                    if let Some(summary) = summary {
                        if r.issues.len() == before {
                            r.creature_supported += 1;
                        } else {
                            r.creature_unsupported
                                .push((logical.clone(), r.issues[before..].join("; ")));
                        }
                        r.tables.push((logical.clone(), summary));
                    }
                }
                None if logical.starts_with("aud/spchdata/") => {
                    absorb_speech(&mut r, logical, &bytes, &mut speech_cues);
                }
                // Other deferred families — counted, probed text-vs-binary.
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
    for (name, users) in &r.dead_refs {
        let users: Vec<String> = users
            .iter()
            .filter(|u| u.starts_with("aud/creaturedata/"))
            .cloned()
            .collect();
        if !users.is_empty() {
            r.creature_missing.push((name.clone(), users));
        }
    }
    for sec in r.speech_sections.values_mut() {
        sec.missing = sec.clips.iter().filter(|c| !stems.contains(*c)).count();
    }
    r.speech_clips_total = speech_cues.len();
    for (stem, table) in &speech_cues {
        if stems.contains(stem) {
            r.speech_clips_resolved += 1;
        } else {
            r.speech_clips_missing.push((stem.clone(), table.clone()));
        }
    }
    // Music-table stem → shipped DirectMusic container cross-check.
    r.music_refs_total = music_refs.len();
    for (stem, users) in &music_refs {
        if music_files.contains_key(stem) {
            r.music_refs_resolved += 1;
        } else {
            let mut users = users.clone();
            users.dedup();
            r.dead_music_refs.push((stem.clone(), users));
        }
    }
    r.unreferenced_music = music_files
        .iter()
        .filter(|(name, _)| !music_refs.contains_key(*name))
        .map(|(_, logical)| logical.clone())
        .collect();
    // DMRF reference → shipped container cross-check, then reachability
    // from the stems the music tables name.
    for (owner, targets) in &dm_edges {
        for t in targets {
            r.dm_refs_total += 1;
            if dm_files.contains_key(t) {
                r.dm_refs_resolved += 1;
            } else {
                let owner = dm_files
                    .get(owner)
                    .cloned()
                    .unwrap_or_else(|| owner.clone());
                r.dead_dm_refs.push((owner, t.clone()));
            }
        }
    }
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    let mut frontier: Vec<&str> = music_refs
        .keys()
        .filter(|k| dm_files.contains_key(*k))
        .map(String::as_str)
        .collect();
    while let Some(n) = frontier.pop() {
        if !reached.insert(n) {
            continue;
        }
        for t in dm_edges.get(n).into_iter().flatten() {
            if dm_files.contains_key(t) {
                frontier.push(t);
            }
        }
    }
    r.dm_unreached = dm_files
        .iter()
        .filter(|(n, _)| !reached.contains(n.as_str()))
        .map(|(_, logical)| logical.clone())
        .collect();
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

/// Widest suffix draw the census enumerates per row; a wider authored
/// range is reported, not walked.
const MAX_CUE_DRAW: i64 = 1000;

/// Fold one `aud/spchdata/**` file into the speech census: a city
/// registry or a cue table, and every wave stem the usable rows can
/// draw (`cue_wave_stem`, the runtime's own naming).
fn absorb_speech(
    r: &mut AudioReport,
    logical: &str,
    bytes: &[u8],
    cues: &mut BTreeMap<String, String>,
) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        r.deferred_binary.push(logical.to_string());
        return;
    };
    let is_registry = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.trim().to_ascii_lowercase().starts_with("num announcers"));
    if is_registry {
        match AnnouncerIndex::parse(text) {
            Ok(i) if i.diagnostics.is_empty() => r.speech_supported += 1,
            Ok(i) => {
                let why = i
                    .diagnostics
                    .iter()
                    .map(|d| format!("line {}: {}", d.line, d.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                r.quirks.push(format!("{logical}: {why}"));
                r.speech_unsupported.push((logical.to_string(), why));
            }
            Err(e) => r.failures.push((logical.to_string(), e.to_string())),
        }
        return;
    }
    let table = match CueTable::parse(text) {
        Ok(t) => t,
        Err(e) => {
            r.failures.push((logical.to_string(), e.to_string()));
            return;
        }
    };
    let semantic = table.validate();
    let diags: Vec<_> = table.diagnostics.iter().chain(&semantic).collect();
    if diags.is_empty() {
        r.speech_supported += 1;
    } else {
        let why = diags
            .iter()
            .map(|d| format!("line {}: {}", d.line, d.message))
            .collect::<Vec<_>>()
            .join("; ");
        r.quirks.push(format!("{logical}: {why}"));
        r.speech_unsupported.push((logical.to_string(), why));
    }
    // `aud/spchdata/<speaker>/<table>.csv` draws `<speaker><prefix><NN>`;
    // a top-level table authors speaker-qualified prefixes.
    let speaker = logical
        .strip_prefix("aud/spchdata/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(dir, _)| dir)
        // Lesson tables author whole stems (`CCL01INTRO` →
        // `ccl01intro01`), so no speaker prefix applies.
        .filter(|dir| !matches!(*dir, "ccl" | "ccs"))
        .unwrap_or("");
    for section in &table.sections {
        let stats = r
            .speech_sections
            .entry(section.name.to_ascii_uppercase())
            .or_default();
        stats.tables += 1;
        stats.rows += section.rows.len();
        for row in &section.rows {
            if !row.problems().is_empty() || row.end - row.add > MAX_CUE_DRAW {
                continue;
            }
            for suffix in row.add + 1..=row.end {
                let stem = mm2_game::cue_wave_stem(speaker, &row.prefix, suffix);
                stats.clips.insert(stem.clone());
                cues.entry(stem).or_insert_with(|| logical.to_string());
            }
        }
    }
}

/// Print the report; `strict` fails on failures and issues (authored
/// quirks and findings are reported but do not fail strict).
pub fn print_report(r: &AudioReport) {
    println!("== audio tables (aud/cardata/**, aud/ambient/**) ==");
    for (path, summary) in &r.tables {
        println!("  {path:<62} {summary}");
    }
    println!(
        "  creaturedata (ped/ambient voices): {} supported, {} unsupported, {} missing wave ref(s)",
        r.creature_supported,
        r.creature_unsupported.len(),
        r.creature_missing.len()
    );
    for (p, why) in &r.creature_unsupported {
        println!("    unsupported {p}: {why}");
    }
    for (name, users) in &r.creature_missing {
        println!("    missing wave {name} (in {})", users.join(", "));
    }
    println!("== audio families (F08-AC04) ==");
    println!(
        "  {:<44} {:>6} {:>9} {:>11} {:>8} {:>7}",
        "family", "files", "supported", "unsupported", "missing", "failed"
    );
    for f in r.families() {
        println!(
            "  {:<44} {:>6} {:>9} {:>11} {:>8} {:>7}  {}",
            f.family, f.files, f.supported, f.unsupported, f.missing, f.failed, f.note
        );
    }
    println!(
        "  speech clips: {} drawable, {} resolve, {} missing; wave decode: {}/{} files",
        r.speech_clips_total,
        r.speech_clips_resolved,
        r.speech_clips_missing.len(),
        r.waves,
        r.waves
            + r.failures
                .iter()
                .filter(|(p, _)| p.ends_with(".wav"))
                .count()
    );
    println!("  speech sections (authored cue types; bound = runtime trigger exists):");
    println!(
        "    {:<20} {:>6} {:>5} {:>7} {:>8} {:>8}  bound",
        "section", "tables", "rows", "clips", "resolved", "missing"
    );
    for (name, s) in &r.speech_sections {
        println!(
            "    {:<20} {:>6} {:>5} {:>7} {:>8} {:>8}  {}",
            name,
            s.tables,
            s.rows,
            s.clips.len(),
            s.clips.len() - s.missing,
            s.missing,
            if speech_section_bound(name) {
                "yes"
            } else {
                "no"
            }
        );
    }
    for (stem, table) in &r.speech_clips_missing {
        println!("    missing speech wave {stem} (drawn by {table})");
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

    println!("== DirectMusic containers (structure parsed, nothing synthesized) ==");
    for (form, n) in &r.riff_forms {
        println!("  RIFF form {form:?}: {n} file(s)");
    }
    println!(
        "  {} parsed; segment tracks: {}; band instruments: {}",
        r.dm_parsed,
        r.dm_tracks
            .iter()
            .map(|(k, n)| format!("{k}={n}"))
            .collect::<Vec<_>>()
            .join(", "),
        r.dm_instruments
    );
    for (kind, segs) in &r.dm_segments_without {
        println!(
            "  {} segment(s) with no {kind} track: {}",
            segs.len(),
            segs.iter()
                .map(|p| p.strip_prefix("aud/dmusic/").unwrap_or(p))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "  DLS: {} bank(s), {} instrument(s), {} wave(s) ({} not PCM16), {:.1}s PCM16",
        r.dls_banks, r.dls_instruments, r.dls_waves, r.dls_unsupported_waves, r.dls_seconds
    );
    for ((rate, ch, bits), n) in &r.dls_wave_census {
        println!("    DLS PCM {rate} Hz {ch}ch {bits}bit: {n} wave(s)");
    }
    println!(
        "  DMRF references: {}/{} resolved to shipped containers",
        r.dm_refs_resolved, r.dm_refs_total
    );
    for (owner, target) in &r.dead_dm_refs {
        println!("    dead DMRF ref {target:?} ← {owner}");
    }
    if !r.dm_unreached.is_empty() {
        println!(
            "  {} container(s) no music-table stem reaches, even through DMRF references: {}",
            r.dm_unreached.len(),
            r.dm_unreached
                .iter()
                .map(|p| p.strip_prefix("aud/dmusic/").unwrap_or(p))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for (path, summary) in &r.music_tables {
        println!("  {path:<44} {summary}");
    }
    println!(
        "  music references: {}/{} resolved to shipped segment/style/band containers",
        r.music_refs_resolved, r.music_refs_total
    );
    for (stem, users) in &r.dead_music_refs {
        println!("    dead music ref {stem:?} ← {}", users.join(", "));
    }
    if !r.unreferenced_music.is_empty() {
        println!(
            "  {} container(s) no music table names (a segment may reach its style/band itself): {}",
            r.unreferenced_music.len(),
            r.unreferenced_music
                .iter()
                .map(|p| p.strip_prefix("aud/dmusic/").unwrap_or(p))
                .collect::<Vec<_>>()
                .join(", ")
        );
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
        assert_eq!(lookup_stem("aud/aud22/vwidle.22k.wav"), "vwidle");
        assert_eq!(lookup_stem("aud/aud11/vwidle.11k.wav"), "vwidle");
        assert_eq!(lookup_stem("aud/aud11/tireskid1_ps1.wav"), "tireskid1_ps1");
        assert_eq!(lookup_stem("aud/aud11/VWHORN.WAV"), "vwhorn");
        // A dotted name that is not a rate suffix keeps its stem.
        assert_eq!(lookup_stem("aud/aud11/foo.bar.wav"), "foo.bar");
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
        write(d, "aud/dmusic/x.sgt", &riff(b"DMSG"));
        write(
            d,
            "aud/spchdata/al1/blitz.csv",
            b"Name prefix/type header,end sufix value,sufix add value\nPRERACE header,,\nWEARAIN,2,0\n",
        );

        let r = audit(&vfs_of(d));
        assert_eq!(r.waves, 2);
        assert_eq!(r.wave_census[&(22050, 1, 16)], 1);
        assert_eq!(r.wave_census[&(11025, 1, 16)], 1);
        assert_eq!(r.riff_forms["DMSG"], 1);
        // The speech table is parsed, not deferred: two drawable clips,
        // neither shipped.
        assert_eq!(r.deferred_text, 0);
        assert_eq!(r.speech_supported, 1);
        assert_eq!(r.speech_clips_total, 2);
        assert_eq!(r.speech_clips_missing.len(), 2);
        assert_eq!(r.speech_clips_missing[0].0, "al1wearain01");
        let pre = &r.speech_sections["PRERACE"];
        assert_eq!(
            (pre.tables, pre.rows, pre.clips.len(), pre.missing),
            (1, 1, 2, 2)
        );
        assert!(speech_section_bound("RESULTSWIN") && !speech_section_bound("UNLOCKRACE"));
        let fam = r.families();
        let speech = fam.iter().find(|f| f.family.contains("spchdata")).unwrap();
        assert_eq!((speech.files, speech.supported, speech.missing), (1, 1, 2));
        let dm = fam
            .iter()
            .find(|f| f.family.contains("DirectMusic"))
            .unwrap();
        assert_eq!((dm.supported, dm.unsupported), (0, 1));
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

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn container(kind: &[u8; 4], form: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut body = form.to_vec();
        parts.iter().for_each(|p| body.extend_from_slice(p));
        chunk(kind, &body)
    }

    /// A `DMRF` reference list naming `file`.
    fn dmrf(file: &str) -> Vec<u8> {
        let utf16: Vec<u8> = file
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        container(b"LIST", b"DMRF", &[chunk(b"file", &utf16)])
    }

    /// A structurally complete segment: header, one command track and the
    /// given style/band references under a style track.
    fn segment_referencing(files: &[&str]) -> Vec<u8> {
        let mut hdr = vec![0u8; 24];
        hdr.extend_from_slice(b"cmnd\0\0\0\0");
        let refs: Vec<Vec<u8>> = files.iter().map(|f| dmrf(f)).collect();
        container(
            b"RIFF",
            b"DMSG",
            &[
                chunk(b"segh", &[0; 24]),
                container(
                    b"LIST",
                    b"trkl",
                    &[
                        container(b"RIFF", b"DMTK", &[chunk(b"trkh", &hdr)]),
                        container(
                            b"RIFF",
                            b"DMTK",
                            &[chunk(b"trkh", &hdr), container(b"LIST", b"strf", &refs)],
                        ),
                    ],
                ),
            ],
        )
    }

    /// The smallest container of `form` that parses without findings.
    fn riff(form: &[u8; 4]) -> Vec<u8> {
        match form {
            b"DMSG" => segment_referencing(&[]),
            b"DMST" => container(b"RIFF", b"DMST", &[chunk(b"styh", &[0; 12])]),
            other => container(b"RIFF", other, &[]),
        }
    }

    #[test]
    fn audit_resolves_music_table_stems_against_the_containers() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/dmusic/enemystart.sgt", &riff(b"DMSG"));
        write(d, "aud/dmusic/groovestyle.sty", &riff(b"DMST"));
        write(d, "aud/dmusic/bigair.bnd", &riff(b"DMBD"));
        write(d, "aud/dmusic/spare.sgt", &riff(b"DMSG"));
        write(d, "aud/dmusic/x.dls", &riff(b"DLS "));
        // `Missing` names no shipped segment; the style cell resolves as a
        // `.sty`, not the same-named `.sgt`.
        write(
            d,
            "aud/dmusic/csv_files/singleroam.csv",
            b"Start Music,Return Music,Big air Motif style,Big Air Motif Band\nEnemyStart,Missing,GrooveStyle,BigAir\n",
        );
        let r = audit(&vfs_of(d));
        assert_eq!(r.music_tables.len(), 1);
        assert_eq!(r.music_refs_total, 4);
        assert_eq!(r.music_refs_resolved, 3);
        assert_eq!(r.dead_music_refs.len(), 1);
        assert_eq!(r.dead_music_refs[0].0, "missing.sgt");
        // A table is not deferred text once parsed; the `.dls` bank is
        // not a segment/style/band so it is never "unreferenced".
        assert_eq!(r.deferred_text, 0);
        assert_eq!(r.unreferenced_music, ["aud/dmusic/spare.sgt"]);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn audit_flags_two_music_containers_sharing_a_file_name() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/dmusic/a.sgt", &riff(b"DMSG"));
        write(d, "aud/dmusic/sub/a.sgt", &riff(b"DMSG"));
        let r = audit(&vfs_of(d));
        assert_eq!(r.issues.len(), 1, "{:?}", r.issues);
        assert!(r.issues[0].contains("same file name"));
    }

    #[test]
    fn audit_follows_dmrf_references_and_reports_the_dead_and_the_unreached() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        // enemystart → enemy.sty + Gone.bnd (dead); enemy.sty is reached
        // through the segment, spare.sgt by nothing.
        write(
            d,
            "aud/dmusic/enemystart.sgt",
            &segment_referencing(&["Enemy.STY", "Gone.bnd"]),
        );
        write(d, "aud/dmusic/enemy.sty", &riff(b"DMST"));
        write(d, "aud/dmusic/spare.sgt", &riff(b"DMSG"));
        write(
            d,
            "aud/dmusic/csv_files/ui.csv",
            b"Music segment\nEnemyStart\n",
        );
        let r = audit(&vfs_of(d));
        assert!(r.failures.is_empty(), "{:?}", r.failures);
        assert!(r.issues.is_empty(), "{:?}", r.issues);
        assert_eq!(r.dm_parsed, 3);
        assert_eq!(r.dm_tracks["command"], 4);
        // The stub segments carry command tracks only: every one lacks
        // band/style/tempo, none lacks a command track.
        assert!(!r.dm_segments_without.contains_key("command"));
        assert_eq!(r.dm_segments_without["tempo"].len(), 2);
        assert!(
            r.dm_segments_without["tempo"]
                .iter()
                .all(|p| p.ends_with(".sgt"))
        );
        assert_eq!(r.dm_refs_total, 2);
        assert_eq!(r.dm_refs_resolved, 1);
        assert_eq!(
            r.dead_dm_refs,
            [(
                "aud/dmusic/enemystart.sgt".to_string(),
                "gone.bnd".to_string()
            )]
        );
        // The style is named by no table but reached through the segment.
        assert!(
            r.unreferenced_music
                .contains(&"aud/dmusic/enemy.sty".to_string())
        );
        assert_eq!(r.dm_unreached, ["aud/dmusic/spare.sgt"]);
    }

    #[test]
    fn audit_fails_a_container_whose_chunk_tree_is_damaged() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let mut sgt = segment_referencing(&["x.sty"]);
        // Inflate the style-track chunk past its parent.
        let at = sgt.windows(4).position(|w| w == b"strf").unwrap() - 8 + 4;
        sgt[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        write(d, "aud/dmusic/bad.sgt", &sgt);
        let r = audit(&vfs_of(d));
        assert_eq!(r.failures.len(), 1, "{:?}", r.failures);
        assert!(r.failures[0].1.starts_with("DirectMusic:"));
        assert_eq!(r.dm_parsed, 0);
    }

    #[test]
    fn audit_surfaces_a_containers_findings_as_issues() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        // A segment with neither header nor tracks.
        write(d, "aud/dmusic/empty.sgt", &container(b"RIFF", b"DMSG", &[]));
        let r = audit(&vfs_of(d));
        assert_eq!(r.issues.len(), 2, "{:?}", r.issues);
        assert!(r.issues.iter().any(|i| i.contains("MissingSegmentHeader")));
    }

    #[test]
    fn audit_censuses_dls_waves_and_flags_a_non_pcm_one() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let wave = |tag: u16, rate: u32, frames: usize| {
            let mut fmt = Vec::new();
            fmt.extend_from_slice(&tag.to_le_bytes());
            fmt.extend_from_slice(&1u16.to_le_bytes());
            fmt.extend_from_slice(&rate.to_le_bytes());
            fmt.extend_from_slice(&(rate * 2).to_le_bytes());
            fmt.extend_from_slice(&2u16.to_le_bytes());
            fmt.extend_from_slice(&16u16.to_le_bytes());
            container(
                b"LIST",
                b"wave",
                &[chunk(b"fmt ", &fmt), chunk(b"data", &vec![0u8; frames * 2])],
            )
        };
        let waves = [wave(1, 22050, 22050), wave(0x11, 22050, 8)];
        let off = waves[0].len() as u32;
        let mut ptbl = Vec::new();
        for w in [8u32, 2, 0, off] {
            ptbl.extend_from_slice(&w.to_le_bytes());
        }
        let dls = container(
            b"RIFF",
            b"DLS ",
            &[
                chunk(b"colh", &0u32.to_le_bytes()),
                chunk(b"ptbl", &ptbl),
                container(b"LIST", b"wvpl", &waves),
            ],
        );
        write(d, "aud/dmusic/bank.dls", &dls);
        let r = audit(&vfs_of(d));
        assert_eq!(r.dls_banks, 1);
        assert_eq!(r.dls_waves, 2);
        assert_eq!(r.dls_wave_census[&(22050, 1, 16)], 1);
        assert_eq!(r.dls_unsupported_waves, 1);
        assert!((r.dls_seconds - 1.0).abs() < 1e-9);
        assert_eq!(r.issues.len(), 1, "{:?}", r.issues);
        assert!(r.issues[0].contains("UnsupportedWave"));
    }

    #[test]
    fn audit_flags_a_ragged_music_row_as_an_issue() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/dmusic/csv_files/ui.csv", b"Music segment\nA,B\n");
        let r = audit(&vfs_of(d));
        assert_eq!(r.issues.len(), 1, "{:?}", r.issues);
        assert!(r.issues[0].contains("2 cells for 1 columns"));
    }

    #[test]
    fn audit_reports_failures_and_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/aud11/broken.wav", b"not a riff at all");
        // A .sty carrying a segment form word is a mismatch.
        write(d, "aud/dmusic/x.sty", &riff(b"DMSG"));
        // A binary blob masquerading as csv (spchdata shape).
        write(d, "aud/spchdata/voice.csv", &[0xf3, 0xcd, 0xcc, 0x53]);
        write(d, "aud/cardata/player/vpx.bat", b"echo hi\n");

        let r = audit(&vfs_of(d));
        assert_eq!(r.failures.len(), 1);
        assert!(r.failures[0].0.ends_with("broken.wav"));
        assert_eq!(r.riff_mismatches.len(), 1);
        assert_eq!(r.deferred_binary.len(), 1);
        assert_eq!(r.extras["bat"], 1);
    }

    #[test]
    fn creaturedata_is_supported_unsupported_or_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "aud/aud11/known.wav", &pcm_wav(11025, 4));
        write(
            d,
            "aud/creaturedata/ok.csv",
            b"Min speed,Max speed,min time in range,max time out of range\r\n0,5,1,0\r\nsample name,volume,,\r\nKNOWN,0.9,,\r\nGHOST,0.9,,\r\n",
        );
        write(d, "aud/creaturedata/bad.csv", b"junk row\n");
        write(d, "aud/creaturedata/numx.csv", b"Num files\r\n3\r\n");
        let r = audit(&vfs_of(d));
        assert_eq!(r.creature_supported, 2);
        assert_eq!(r.creature_unsupported.len(), 1);
        assert!(r.creature_unsupported[0].0.ends_with("bad.csv"));
        assert_eq!(r.creature_missing.len(), 1);
        assert_eq!(r.creature_missing[0].0, "ghost");
        assert_eq!(r.deferred_text, 0);
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

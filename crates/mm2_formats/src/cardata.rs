//! Parsers for `aud/cardata/**` and `aud/ambient/**` — the authored
//! audio metadata tables (F07-A).
//!
//! These are positional, label-framed CSV exports: each file is a
//! sequence of header rows naming the columns that follow, interleaved
//! with data rows. The parsers preserve authored quirks — the binary
//! "name" prefix in `engineparams*.csv`, sentinel cells like `NOSOUND`
//! and `ENDOFDATA`, the duplicate `copy of …` work files — and classify
//! anomalies as diagnostics rather than load failures, matching the
//! other table decoders.
//!
//! What the data does *not* document stays unverified: how a sample name
//! resolves to a wave file, how impact categories bind to banger
//! records, and the playback semantics of the numeric fields. See
//! `docs/research/audio.md`.

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// Sample-name cells that mean "no sample" rather than a wave lookup.
const SENTINELS: &[&str] = &["", "nosound", "endofdata", "false", "none", "nothing"];

/// Which `aud/cardata`/`aud/ambient` grammar a path carries, derived
/// from the logical name alone (same role as
/// `racefiles::classify_race_file`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardataKind {
    /// `player/`/`opponent/` `vp*.csv`, `default.csv` and their `copy
    /// of`/`*.wrk` work files: horn + RPM-faded engine samples.
    CarAudio,
    /// `engineparamsplay.csv`/`engineparamsopp.csv`: rows whose first
    /// field is a binary blob (MSVC uninitialized-fill on retail) ahead
    /// of nine positional floats.
    EngineParams,
    /// `*/default_impacts.csv`: banger-name → impact-sample tables.
    ImpactTable,
    /// `*/default_surface{dry,ice,wet}.csv`: per-surface loops plus skid
    /// slippage bands.
    SurfaceTable,
    /// `*policesiren.csv`/`policesiren.csv`: siren sequencing programs.
    SirenProgram,
    /// `ambient/*_engine.csv`: speed-ranged ambient engine loops.
    AmbientEngine,
    /// `ambient/*_horn.csv`: horn plus stuck-horn play/pause programs.
    AmbientHorn,
    /// `aud/ambient/*.csv` and `ambient/subwaycar.csv`: distance-ranged
    /// 3-D emitter definitions with optional `VECTORPOINTS`.
    ObjectAudio,
    /// `aud/ambient/*container*.csv`: a `file names` list of sibling
    /// `aud/ambient/<stem>.csv` tables forming one city's ambience.
    AmbientContainer,
    /// `shared/semidata.csv`: semi/bus reverse + air-blow samples.
    SemiData,
    /// `shared/vehtypes.csv`: vehicle-id lists per class label.
    VehTypes,
    /// `player/suspensionaudio.csv`: velocity-banded suspension hits.
    SuspensionAudio,
    /// `player/tirewobble.csv`: tire-wobble sample bands.
    TireWobble,
}

/// Classify a logical path under `aud/` to a cardata grammar, or `None`
/// when it is not one of these tables (other `aud/**` csv files belong
/// to the speech/creature families, F08).
pub fn classify(logical: &str) -> Option<CardataKind> {
    let name = logical.rsplit('/').next()?;
    if !name.ends_with(".csv") {
        return None;
    }
    if logical.starts_with("aud/ambient/") {
        return Some(if name.contains("container") {
            CardataKind::AmbientContainer
        } else {
            CardataKind::ObjectAudio
        });
    }
    if !logical.starts_with("aud/cardata/") {
        return None;
    }
    let kind = if name.starts_with("engineparams") {
        CardataKind::EngineParams
    } else if name.contains("impact") {
        CardataKind::ImpactTable
    } else if name.contains("surface") {
        CardataKind::SurfaceTable
    } else if name.contains("siren") {
        CardataKind::SirenProgram
    } else if name == "semidata.csv" {
        CardataKind::SemiData
    } else if name == "vehtypes.csv" {
        CardataKind::VehTypes
    } else if name.contains("suspensionaudio") {
        CardataKind::SuspensionAudio
    } else if name.contains("tirewobble") {
        CardataKind::TireWobble
    } else if logical.starts_with("aud/cardata/ambient/") {
        if name.ends_with("_engine.csv") {
            CardataKind::AmbientEngine
        } else if name.ends_with("_horn.csv") {
            CardataKind::AmbientHorn
        } else {
            CardataKind::ObjectAudio
        }
    } else {
        CardataKind::CarAudio
    };
    Some(kind)
}

/// A parsed cardata file: the grammar selected by path plus its body.
#[derive(Debug)]
pub struct CardataFile {
    /// The grammar the path classified as.
    pub kind: CardataKind,
    /// The parsed body.
    pub body: CardataBody,
}

/// Parse the cardata table at `logical` (the VFS path decides the
/// grammar). `Err` for unrecognized paths and structurally empty input;
/// malformed lines surface as diagnostics inside the body.
pub fn parse(logical: &str, data: &[u8]) -> Result<CardataFile, FormatError> {
    let kind = classify(logical).ok_or_else(|| {
        FormatError::parse(0, format!("not a recognized cardata path: {logical}"))
    })?;
    let body = match kind {
        CardataKind::CarAudio => CardataBody::Car(CarAudio::parse(data)?),
        CardataKind::EngineParams => CardataBody::EngineParams(EngineParams::parse(data)?),
        CardataKind::ImpactTable => CardataBody::Impacts(ImpactTable::parse(data)?),
        CardataKind::SurfaceTable => CardataBody::Surfaces(SurfaceTable::parse(data)?),
        CardataKind::SirenProgram => CardataBody::Sirens(SirenProgram::parse(data)?),
        CardataKind::AmbientEngine => CardataBody::AmbientEngine(AmbientEngine::parse(data)?),
        CardataKind::AmbientHorn => CardataBody::AmbientHorn(AmbientHorn::parse(data)?),
        CardataKind::ObjectAudio => CardataBody::Object(ObjectAudio::parse(data)?),
        CardataKind::AmbientContainer => CardataBody::Container(AmbientContainer::parse(data)?),
        CardataKind::SemiData => CardataBody::Semi(SemiData::parse(data)?),
        CardataKind::VehTypes => CardataBody::VehTypes(VehTypes::parse(data)?),
        CardataKind::SuspensionAudio | CardataKind::TireWobble => {
            CardataBody::Bands(BandTable::parse(data)?)
        }
    };
    Ok(CardataFile { kind, body })
}

/// The grammar bodies, one variant per [`CardataKind`].
#[derive(Debug)]
pub enum CardataBody {
    /// [`CardataKind::CarAudio`]
    Car(CarAudio),
    /// [`CardataKind::EngineParams`]
    EngineParams(EngineParams),
    /// [`CardataKind::ImpactTable`]
    Impacts(ImpactTable),
    /// [`CardataKind::SurfaceTable`]
    Surfaces(SurfaceTable),
    /// [`CardataKind::SirenProgram`]
    Sirens(SirenProgram),
    /// [`CardataKind::AmbientEngine`]
    AmbientEngine(AmbientEngine),
    /// [`CardataKind::AmbientHorn`]
    AmbientHorn(AmbientHorn),
    /// [`CardataKind::ObjectAudio`]
    Object(ObjectAudio),
    /// [`CardataKind::AmbientContainer`]
    Container(AmbientContainer),
    /// [`CardataKind::SemiData`]
    Semi(SemiData),
    /// [`CardataKind::VehTypes`]
    VehTypes(VehTypes),
    /// [`CardataKind::SuspensionAudio`] / [`CardataKind::TireWobble`]
    Bands(BandTable),
}

impl CardataBody {
    /// Recoverable line-level problems collected while parsing.
    pub fn diagnostics(&self) -> &[TableDiagnostic] {
        match self {
            CardataBody::Car(b) => &b.diagnostics,
            CardataBody::EngineParams(b) => &b.diagnostics,
            CardataBody::Impacts(b) => &b.diagnostics,
            CardataBody::Surfaces(b) => &b.diagnostics,
            CardataBody::Sirens(b) => &b.diagnostics,
            CardataBody::AmbientEngine(b) => &b.diagnostics,
            CardataBody::AmbientHorn(b) => &b.diagnostics,
            CardataBody::Object(b) => &b.diagnostics,
            CardataBody::Container(b) => &b.diagnostics,
            CardataBody::Semi(b) => &b.diagnostics,
            CardataBody::VehTypes(b) => &b.diagnostics,
            CardataBody::Bands(b) => &b.diagnostics,
        }
    }

    /// Semantic issues on the parsed body (declared-vs-parsed counts,
    /// degenerate ranges, non-finite values).
    pub fn validate(&self) -> Vec<CardataIssue> {
        match self {
            CardataBody::Car(b) => b.validate(),
            CardataBody::EngineParams(b) => b.validate(),
            CardataBody::Impacts(b) => b.validate(),
            CardataBody::Surfaces(b) => b.validate(),
            CardataBody::Sirens(b) => b.validate(),
            CardataBody::AmbientEngine(b) => b.validate(),
            CardataBody::AmbientHorn(b) => b.validate(),
            CardataBody::Object(b) => b.validate(),
            CardataBody::Container(b) => b.validate(),
            CardataBody::Semi(b) => b.validate(),
            CardataBody::VehTypes(b) => b.validate(),
            CardataBody::Bands(b) => b.validate(),
        }
    }

    /// Every wave/sample name the table references, lower-cased
    /// sentinel cells (`NOSOUND`, `ENDOFDATA`, `FALSE`, empty) dropped.
    /// Name→file resolution is a separate, unverified step — the caller
    /// decides how stems map to `aud/audNN/**.wav` paths.
    pub fn wave_names(&self) -> Vec<&str> {
        fn keep(name: &str) -> Option<&str> {
            let t = name.trim();
            (!SENTINELS.contains(&t.to_ascii_lowercase().as_str())).then_some(t)
        }
        let mut out: Vec<&str> = Vec::new();
        match self {
            CardataBody::Car(b) => {
                out.extend(keep(&b.horn.name));
                out.extend(keep(&b.clutch.name));
                out.extend(b.engine_samples.iter().filter_map(|s| keep(&s.name)));
            }
            CardataBody::Impacts(b) => out.extend(
                b.categories
                    .iter()
                    .flat_map(|c| c.samples.iter())
                    .filter_map(|s| keep(&s.name)),
            ),
            CardataBody::Surfaces(b) => out.extend(b.surfaces.iter().flat_map(|s| {
                keep(&s.name)
                    .into_iter()
                    .chain(s.skids.iter().filter_map(|k| keep(&k.name)))
            })),
            CardataBody::Sirens(b) => {
                out.extend(b.explosion.iter().filter_map(|s| keep(&s.name)));
                out.extend(b.samples.iter().filter_map(|s| keep(&s.name)));
            }
            CardataBody::AmbientEngine(b) => out.extend(keep(&b.sample.name)),
            CardataBody::AmbientHorn(b) => out.extend(keep(&b.horn.name)),
            CardataBody::Object(b) => out.extend(b.samples.iter().filter_map(|s| keep(&s.name))),
            CardataBody::Container(_) => {}
            CardataBody::Semi(b) => {
                out.extend(keep(&b.reverse_sample));
                out.extend(keep(&b.air_blow_sample));
            }
            CardataBody::Bands(b) => out.extend(b.rows.iter().filter_map(|r| keep(&r.name))),
            CardataBody::EngineParams(_) | CardataBody::VehTypes(_) => {}
        }
        out
    }
}

/// Semantic findings shared across the cardata grammars.
#[derive(Debug, Clone, PartialEq)]
pub enum CardataIssue {
    /// A declared count (`Num Engine Samples`, `Num samples`, `num skid
    /// samples`) does not match the rows actually present.
    DeclaredVsParsed {
        /// Which table/section.
        context: &'static str,
        /// The authored count.
        declared: usize,
        /// Rows actually parsed.
        parsed: usize,
    },
    /// A numeric field is NaN or infinite.
    NonFinite {
        /// Which table/section.
        context: &'static str,
        /// Which field.
        field: &'static str,
        /// 1-based source line.
        line: u32,
    },
    /// An authored min/max range is degenerate (`max < min`).
    DegenerateRange {
        /// Which table/section.
        context: &'static str,
        /// Which field pair.
        field: &'static str,
        /// 1-based source line.
        line: u32,
    },
    /// A name field carries non-text bytes (the `engineparams*`
    /// uninitialized-fill prefix). Preserved, never silently dropped.
    BinaryNameField {
        /// Which table/section.
        context: &'static str,
        /// Raw field length in bytes.
        len: usize,
        /// 1-based source line.
        line: u32,
    },
    /// Lines exist past the authored `ENDOFDATA` terminator.
    ContentAfterTerminator {
        /// Which table/section.
        context: &'static str,
        /// Lines following the terminator.
        lines: usize,
    },
}

impl std::fmt::Display for CardataIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeclaredVsParsed {
                context,
                declared,
                parsed,
            } => write!(
                f,
                "{context}: declared {declared} but {parsed} row(s) parsed"
            ),
            Self::NonFinite {
                context,
                field,
                line,
            } => write!(f, "{context}: non-finite {field} (line {line})"),
            Self::DegenerateRange {
                context,
                field,
                line,
            } => write!(f, "{context}: degenerate {field} range (line {line})"),
            Self::BinaryNameField { context, len, line } => {
                write!(f, "{context}: {len}-byte binary name field (line {line})")
            }
            Self::ContentAfterTerminator { context, lines } => {
                write!(f, "{context}: {lines} line(s) after ENDOFDATA")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CSV machinery: these files are positional, so lines become cell lists
// and labels are matched on the first cell, case-insensitively (authored
// casing varies — `Sample name` vs `sample name` in the same family).
// ---------------------------------------------------------------------------

/// Non-blank input lines as `(1-based line, trimmed cells)`.
fn rows(input: &[u8]) -> Vec<(u32, Vec<String>)> {
    input
        .split(|&b| b == b'\n')
        .enumerate()
        .map(|(i, raw)| {
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            (i as u32 + 1, raw)
        })
        .filter(|(_, raw)| !raw.iter().all(|b| b.is_ascii_whitespace()))
        .map(|(no, raw)| {
            let cells = raw
                .split(|&b| b == b',')
                .map(|c| String::from_utf8_lossy(c).trim().to_string())
                .collect();
            (no, cells)
        })
        .collect()
}

/// Whether the row's first cell is the authored label `want`.
fn is_label(cells: &[String], want: &str) -> bool {
    cells.first().is_some_and(|c| c.eq_ignore_ascii_case(want))
}

/// Whether the row's first cell starts with `prefix` (label families
/// like `horn play duration` repeat per group).
fn is_label_prefix(cells: &[String], prefix: &str) -> bool {
    cells
        .first()
        .is_some_and(|c| c.to_ascii_lowercase().starts_with(prefix))
}

/// Whether a cell parses as a finite-or-not float at all.
fn is_num(cell: &str) -> bool {
    !cell.is_empty() && cell.parse::<f32>().is_ok()
}

/// Whether the row is a section separator — authored as `***` on most
/// tables and quote-wrapped (`""""`) in the Excel-resaved `copy of`
/// work files.
fn is_separator(cells: &[String]) -> bool {
    cells
        .first()
        .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b == b'*' || b == b'"'))
}

/// Parse cell `i` as f32; on missing/non-numeric cells push a
/// diagnostic and return `None`.
fn f32_cell(
    cells: &[String],
    i: usize,
    line: u32,
    diags: &mut Vec<TableDiagnostic>,
) -> Option<f32> {
    match cells.get(i) {
        Some(c) if is_num(c) => c.parse::<f32>().ok(),
        _ => {
            diags.push(TableDiagnostic {
                line,
                message: format!("expected numeric field {i}, have {:?}", cells.get(i)),
            });
            None
        }
    }
}

/// Parse cell `i` as an integer (authored cells occasionally carry
/// `0.0`-style decimals — accepted via float round-trip).
fn int_cell(
    cells: &[String],
    i: usize,
    line: u32,
    diags: &mut Vec<TableDiagnostic>,
) -> Option<i64> {
    match cells.get(i) {
        Some(c) => match c
            .parse::<i64>()
            .or_else(|_| c.parse::<f64>().map(|f| f as i64))
        {
            Ok(v) => Some(v),
            Err(_) => {
                diags.push(TableDiagnostic {
                    line,
                    message: format!("expected integer field {i}, have {c:?}"),
                });
                None
            }
        },
        None => {
            diags.push(TableDiagnostic {
                line,
                message: format!("expected integer field {i}, row ends early"),
            });
            None
        }
    }
}

/// Record a [`CardataIssue::NonFinite`] when `v` is not finite.
fn check_finite(
    context: &'static str,
    issues: &mut Vec<CardataIssue>,
    field: &'static str,
    v: f32,
    line: u32,
) {
    if !v.is_finite() {
        issues.push(CardataIssue::NonFinite {
            context,
            field,
            line,
        });
    }
}

/// Record a [`CardataIssue::DegenerateRange`] when `hi < lo` (both
/// finite — NaN is reported by [`check_finite`] instead).
fn check_range(
    context: &'static str,
    issues: &mut Vec<CardataIssue>,
    field: &'static str,
    lo: f32,
    hi: f32,
    line: u32,
) {
    if lo.is_finite() && hi.is_finite() && hi < lo {
        issues.push(CardataIssue::DegenerateRange {
            context,
            field,
            line,
        });
    }
}

// ---------------------------------------------------------------------------
// CarAudio — player/opponent `vp*.csv`/`default.csv`
// ---------------------------------------------------------------------------

/// Horn plus clutch sample binding from the first data row.
#[derive(Debug, Clone)]
pub struct CarAudio {
    /// Horn sample name + volume.
    pub horn: NamedVolume,
    /// Authored `flags` word on the horn row (0 on retail).
    pub flags: i64,
    /// `Num Engine Samples` — the authored count for the second table.
    pub declared_engine_samples: usize,
    /// Clutch/reverse sample name + volume.
    pub clutch: NamedVolume,
    /// RPM-faded engine loop definitions, in authored order.
    pub engine_samples: Vec<EngineSample>,
    /// Recoverable problems (skipped malformed lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// A name + volume pair (horn, clutch, explosion, ambient samples).
#[derive(Debug, Clone)]
pub struct NamedVolume {
    /// Sample name (a wave stem on retail; resolution rule unverified).
    pub name: String,
    /// Authored volume.
    pub volume: f32,
    /// 1-based source line.
    pub line: u32,
}

/// One row of the `Engine wave name` table: an RPM-faded engine loop.
///
/// Three authored column layouts ship on retail — the 10-column
/// fade-window schema (`fade in start RPM` … `Pitch shift end RPM`), a
/// 9-column `Pitch divisor` variant (`copy of vpford.csv`), and the
/// 7-column `Volume divisor`/`vol inverse RPM` variant (`copy of
/// default.csv`, `copy of vpmustang99.csv`) — so values are stored
/// positionally and the header row is preserved verbatim.
#[derive(Debug, Clone)]
pub struct EngineSample {
    /// Sample name.
    pub name: String,
    /// The `Engine wave name` header cells verbatim — `columns[0]` is
    /// the label itself, so `values[i]` corresponds to `columns[i + 1]`.
    pub columns: Vec<String>,
    /// Positional values after the name, in authored column order.
    pub values: Vec<f32>,
    /// 1-based source line.
    pub line: u32,
}

impl EngineSample {
    /// The value under the column whose name contains `needle`
    /// (case- and whitespace-insensitive), when the column and the
    /// value both exist.
    pub fn column(&self, needle: &str) -> Option<f32> {
        let needle = collapse_ws(needle);
        self.columns
            .iter()
            .position(|c| collapse_ws(c).to_ascii_lowercase().contains(&needle))
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.values.get(i))
            .copied()
    }
}

/// Collapse whitespace runs — authored headers contain stray double
/// spaces (`fade in  start RPM`).
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl CarAudio {
    /// Parse a player/opponent car-audio table. Errors only when neither
    /// the horn row nor any engine sample parses.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut diags = Vec::new();
        let mut horn: Option<NamedVolume> = None;
        let mut flags = 0i64;
        let mut declared = 0usize;
        let mut clutch: Option<NamedVolume> = None;
        let mut samples = Vec::new();
        let mut engine_columns: Vec<String> = Vec::new();
        let mut in_engine_table = false;

        for (line, cells) in &rows {
            let line = *line;
            if is_label(cells, "Horn wave name") {
                continue;
            }
            if is_label(cells, "Engine wave name") {
                engine_columns = cells.clone();
                while engine_columns.last().is_some_and(|c| c.is_empty()) {
                    engine_columns.pop();
                }
                in_engine_table = true;
                continue;
            }
            if !in_engine_table && horn.is_none() {
                // The horn row: name,vol,flags,num,clutch-name,clutch-vol.
                if cells.len() < 6 || !is_num(&cells[1]) {
                    diags.push(TableDiagnostic {
                        line,
                        message: format!("skipping row: not the horn record ({cells:?})"),
                    });
                    continue;
                }
                horn = Some(NamedVolume {
                    name: cells[0].clone(),
                    volume: cells[1].parse().unwrap_or(f32::NAN),
                    line,
                });
                flags = int_cell(cells, 2, line, &mut diags).unwrap_or(0);
                declared = int_cell(cells, 3, line, &mut diags)
                    .and_then(|v| usize::try_from(v).ok())
                    .unwrap_or(0);
                clutch = Some(NamedVolume {
                    name: cells[4].clone(),
                    volume: f32_cell(cells, 5, line, &mut diags).unwrap_or(f32::NAN),
                    line,
                });
                continue;
            }
            // Engine-sample rows: a name plus an all-numeric tail in
            // whichever column layout the file's header declares.
            if cells.len() < 2 || !cells[1..].iter().all(|c| c.is_empty() || is_num(c)) {
                diags.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: not an engine sample ({cells:?})"),
                });
                continue;
            }
            let values: Vec<f32> = cells[1..]
                .iter()
                .filter(|c| !c.is_empty())
                .map(|c| c.parse().unwrap_or(f32::NAN))
                .collect();
            if !engine_columns.is_empty() && values.len() + 1 != engine_columns.len() {
                diags.push(TableDiagnostic {
                    line,
                    message: format!(
                        "engine row has {} values, header names {}",
                        values.len(),
                        engine_columns.len() - 1
                    ),
                });
            }
            samples.push(EngineSample {
                name: cells[0].clone(),
                columns: engine_columns.clone(),
                values,
                line,
            });
        }

        if horn.is_none() && samples.is_empty() {
            return Err(FormatError::parse(0, "no car-audio rows parsed"));
        }
        let empty = || NamedVolume {
            name: String::new(),
            volume: f32::NAN,
            line: 0,
        };
        let horn = horn.unwrap_or_else(empty);
        let clutch = clutch.unwrap_or_else(empty);
        Ok(CarAudio {
            horn,
            flags,
            declared_engine_samples: declared,
            clutch,
            engine_samples: samples,
            diagnostics: diags,
        })
    }

    /// Sanity issues: declared-vs-parsed sample count, non-finite
    /// values, degenerate fade bands.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        if self.declared_engine_samples != self.engine_samples.len() {
            issues.push(CardataIssue::DeclaredVsParsed {
                context: "engine samples",
                declared: self.declared_engine_samples,
                parsed: self.engine_samples.len(),
            });
        }
        check_finite(
            "car audio",
            &mut issues,
            "horn volume",
            self.horn.volume,
            self.horn.line,
        );
        check_finite(
            "car audio",
            &mut issues,
            "clutch volume",
            self.clutch.volume,
            self.clutch.line,
        );
        for s in &self.engine_samples {
            for v in &s.values {
                check_finite("car audio", &mut issues, "engine value", *v, s.line);
            }
            // Min/max pairs in whichever schema the file authored.
            for (lo, hi, field) in [
                ("min volume", "max volume", "engine volume"),
                ("min pitch", "max pitch", "pitch"),
                ("fade in start", "fade in end", "fade in rpm"),
                ("fade out start", "fade out end", "fade out rpm"),
                ("pitch shift start", "pitch shift end", "pitch shift rpm"),
            ] {
                if let (Some(a), Some(b)) = (s.column(lo), s.column(hi)) {
                    check_range("car audio", &mut issues, field, a, b, s.line);
                }
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// EngineParams — engineparams{play,opp}.csv
// ---------------------------------------------------------------------------

/// `engineparams*.csv`: rows whose first field is a binary blob on
/// retail (MSVC `0xCD`/`0xDD` debug fill plus garbage — an authored
/// quirk preserved verbatim) followed by nine positional floats whose
/// authored column names are not present in the file (semantics
/// unverified — they share the engine-sample column count minus one).
#[derive(Debug, Clone)]
pub struct EngineParams {
    /// Rows in authored order.
    pub rows: Vec<EngineParamsRow>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One engineparams row: the raw name field plus positional floats.
#[derive(Debug, Clone)]
pub struct EngineParamsRow {
    /// The first field verbatim — on retail it is non-ASCII binary fill,
    /// not a sample name.
    pub name_raw: Vec<u8>,
    /// The trailing positional values (9 on retail).
    pub values: Vec<f32>,
    /// 1-based source line.
    pub line: u32,
}

impl EngineParamsRow {
    /// The name field as text when it is printable ASCII (`None` on the
    /// retail binary fill).
    pub fn name_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.name_raw)
            .ok()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.bytes().all(|b| (0x20..=0x7e).contains(&b)))
    }
}

impl EngineParams {
    /// Parse an engineparams table. Each row's *last* nine
    /// comma-separated fields must be floats; everything before them is
    /// the raw name field — this tolerates commas inside the binary
    /// prefix, which a left-to-right split would not.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let mut out = Vec::new();
        let mut diags = Vec::new();
        for (i, raw) in input.split(|&b| b == b'\n').enumerate() {
            let line = i as u32 + 1;
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            if raw.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            let commas: Vec<usize> = raw
                .iter()
                .enumerate()
                .filter(|(_, b)| **b == b',')
                .map(|(i, _)| i)
                .collect();
            if commas.len() < 9 {
                diags.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: fewer than 9 comma fields ({})", commas.len()),
                });
                continue;
            }
            let split_at = commas[commas.len() - 9];
            let tail = &raw[split_at + 1..];
            let cells: Vec<&[u8]> = tail.split(|&b| b == b',').collect();
            let values: Option<Vec<f32>> = cells
                .iter()
                .map(|c| String::from_utf8_lossy(c).trim().parse::<f32>().ok())
                .collect();
            match values {
                Some(values) => out.push(EngineParamsRow {
                    name_raw: raw[..split_at].to_vec(),
                    values,
                    line,
                }),
                None => diags.push(TableDiagnostic {
                    line,
                    message: "skipping row: trailing fields are not all numeric".to_string(),
                }),
            }
        }
        if out.is_empty() {
            return Err(FormatError::parse(0, "no engineparams rows parsed"));
        }
        Ok(EngineParams {
            rows: out,
            diagnostics: diags,
        })
    }

    /// Sanity issues: binary name fields and non-finite values.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        for r in &self.rows {
            if r.name_text().is_none() && !r.name_raw.is_empty() {
                issues.push(CardataIssue::BinaryNameField {
                    context: "engineparams",
                    len: r.name_raw.len(),
                    line: r.line,
                });
            }
            for v in &r.values {
                if !v.is_finite() {
                    issues.push(CardataIssue::NonFinite {
                        context: "engineparams",
                        field: "value",
                        line: r.line,
                    });
                }
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// ImpactTable — */default_impacts.csv
// ---------------------------------------------------------------------------

/// A `default_impacts.csv` table: banger-name categories, each with an
/// authored numeric id and a force-ranged sample set. How categories
/// bind to `dgBangerData` records (whose `AudioId` is 0 on retail) is
/// unverified — the authored `ID` column is preserved but its runtime
/// role is unknown.
#[derive(Debug, Clone)]
pub struct ImpactTable {
    /// Categories in authored order, excluding the `ENDOFDATA`
    /// terminator.
    pub categories: Vec<ImpactCategory>,
    /// Lines found after the terminator (0 when the file ends cleanly).
    pub trailing_lines: usize,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One banger-name impact category.
#[derive(Debug, Clone)]
pub struct ImpactCategory {
    /// Authored name (`WALL`, `LIGHT`, `SIGN`, …).
    pub name: String,
    /// Authored numeric id (0-based dense on retail).
    pub id: i64,
    /// Authored `Num samples` count.
    pub declared_samples: usize,
    /// Force-ranged samples for this category.
    pub samples: Vec<ImpactSample>,
    /// 1-based source line.
    pub line: u32,
}

/// One impact sample binding.
#[derive(Debug, Clone)]
pub struct ImpactSample {
    /// Sample name.
    pub name: String,
    /// Minimum random volume.
    pub min_volume: f32,
    /// Maximum random volume.
    pub max_volume: f32,
    /// Impact-force band: softest hit this sample covers.
    pub min_force: f32,
    /// Impact-force band: hardest hit this sample covers.
    pub max_force: f32,
    /// Authored `frequency` weight (1.0 on most retail rows).
    pub frequency: f32,
    /// 1-based source line.
    pub line: u32,
}

impl ImpactTable {
    /// Parse an impact table: `***`-separated `Banger name` sections
    /// ending at `ENDOFDATA`.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut categories = Vec::new();
        let mut diags = Vec::new();
        let mut trailing = 0usize;
        let mut ended = false;
        let mut i = 0usize;
        while i < rows.len() {
            let (line, cells) = &rows[i];
            let line = *line;
            if ended {
                trailing += 1;
                i += 1;
                continue;
            }
            if is_separator(cells) || cells.iter().all(|c| c.is_empty()) {
                i += 1;
                continue;
            }
            if is_label(cells, "Banger name") {
                i += 1;
                let Some(&(cline, ref crow)) = rows.get(i) else {
                    diags.push(TableDiagnostic {
                        line,
                        message: "Banger name header at end of file".to_string(),
                    });
                    break;
                };
                let name = crow.first().cloned().unwrap_or_default();
                if name.eq_ignore_ascii_case("ENDOFDATA") {
                    ended = true;
                    i += 1;
                    continue;
                }
                let declared = int_cell(crow, 1, cline, &mut diags)
                    .and_then(|v| usize::try_from(v).ok())
                    .unwrap_or(0);
                let id = int_cell(crow, 2, cline, &mut diags).unwrap_or(0);
                let mut cat = ImpactCategory {
                    name,
                    id,
                    declared_samples: declared,
                    samples: Vec::new(),
                    line: cline,
                };
                i += 1;
                // Optional `sample name,…` header, then the sample rows
                // until the next `***`/label/terminator.
                if let Some((_, h)) = rows.get(i)
                    && is_label_prefix(h, "sample name")
                {
                    i += 1;
                }
                while i < rows.len() {
                    let (sline, scells) = &rows[i];
                    let sline = *sline;
                    let first = scells.first().map(|c| c.trim()).unwrap_or("");
                    if is_separator(scells)
                        || is_label(scells, "Banger name")
                        || first.eq_ignore_ascii_case("ENDOFDATA")
                    {
                        break;
                    }
                    if scells.len() < 6 || !scells[1..].iter().take(5).all(|c| is_num(c)) {
                        diags.push(TableDiagnostic {
                            line: sline,
                            message: format!("skipping impact row: {scells:?}"),
                        });
                        i += 1;
                        continue;
                    }
                    let f = |i: usize| scells[i].parse().unwrap_or(f32::NAN);
                    cat.samples.push(ImpactSample {
                        name: scells[0].clone(),
                        min_volume: f(1),
                        max_volume: f(2),
                        min_force: f(3),
                        max_force: f(4),
                        frequency: f(5),
                        line: sline,
                    });
                    i += 1;
                }
                categories.push(cat);
                continue;
            }
            diags.push(TableDiagnostic {
                line,
                message: format!("skipping line outside a banger section: {cells:?}"),
            });
            i += 1;
        }
        if categories.is_empty() {
            return Err(FormatError::parse(0, "no impact categories parsed"));
        }
        Ok(ImpactTable {
            categories,
            trailing_lines: trailing,
            diagnostics: diags,
        })
    }

    /// Sanity issues: declared-vs-parsed sample counts, non-finite or
    /// degenerate force/volume bands, content past `ENDOFDATA`.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        if self.trailing_lines > 0 {
            issues.push(CardataIssue::ContentAfterTerminator {
                context: "impacts",
                lines: self.trailing_lines,
            });
        }
        for c in &self.categories {
            if c.declared_samples != c.samples.len() {
                issues.push(CardataIssue::DeclaredVsParsed {
                    context: "impact samples",
                    declared: c.declared_samples,
                    parsed: c.samples.len(),
                });
            }
            for s in &c.samples {
                check_range(
                    "impacts",
                    &mut issues,
                    "impact volume",
                    s.min_volume,
                    s.max_volume,
                    s.line,
                );
                check_range(
                    "impacts",
                    &mut issues,
                    "impact force",
                    s.min_force,
                    s.max_force,
                    s.line,
                );
                check_finite(
                    "impacts",
                    &mut issues,
                    "impact volume",
                    s.min_volume,
                    s.line,
                );
                check_finite(
                    "impacts",
                    &mut issues,
                    "impact volume",
                    s.max_volume,
                    s.line,
                );
                check_finite("impacts", &mut issues, "impact force", s.min_force, s.line);
                check_finite("impacts", &mut issues, "impact force", s.max_force, s.line);
                check_finite(
                    "impacts",
                    &mut issues,
                    "impact frequency",
                    s.frequency,
                    s.line,
                );
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// SurfaceTable — */default_surface{dry,ice,wet}.csv
// ---------------------------------------------------------------------------

/// A `default_surface*.csv` table: a `tunnel sound index`, then one
/// entry per surface index (the positional row order — which index maps
/// to which material is unverified), each carrying a rolling loop and
/// banded skid samples.
///
/// Two authored column layouts exist and both are preserved verbatim:
/// the opponent files use a 9-column row (`max speed` … `num skid
/// samples`), the player files a 12-column row with `divisor` fields and
/// a `for tunnels` flag. Skid bands are `min slippage,max slippage` in
/// the opponent schema and `min speed,max speed` in the player schema —
/// values are stored positionally and the header row is kept so callers
/// see which unit the band claims.
#[derive(Debug, Clone)]
pub struct SurfaceTable {
    /// The authored `tunnel sound index`, when present (0 on dry/wet,
    /// 5 on ice — unverified meaning).
    pub tunnel_index: Option<i64>,
    /// Surface entries in authored order.
    pub surfaces: Vec<SurfaceEntry>,
    /// Lines found after the `ENDOFDATA` terminator (0 when absent or
    /// clean).
    pub trailing_lines: usize,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One surface row plus its skid bands. Numeric fields are positional —
/// consult [`SurfaceEntry::columns`] for the authored column names.
#[derive(Debug, Clone)]
pub struct SurfaceEntry {
    /// Rolling-surface sample name (`NOSOUND` where silent).
    pub name: String,
    /// The `surface wave` header's cells verbatim — `columns[0]` is the
    /// `surface wave` label itself, so `values[i]` corresponds to
    /// `columns[i + 1]`.
    pub columns: Vec<String>,
    /// Positional values after the name, in authored column order.
    pub values: Vec<f32>,
    /// The `skid wave` header's cells verbatim, when a skid table
    /// follows.
    pub skid_columns: Vec<String>,
    /// Banded skid samples.
    pub skids: Vec<SkidSample>,
    /// 1-based source line.
    pub line: u32,
}

impl SurfaceEntry {
    /// The value under the column whose name contains `needle` (e.g.
    /// `num skid samples`, `for tunnels`), if both the column and the
    /// value exist.
    pub fn column(&self, needle: &str) -> Option<f32> {
        self.columns
            .iter()
            .position(|c| c.to_ascii_lowercase().contains(needle))
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.values.get(i))
            .copied()
    }

    /// The authored `num skid samples` count, when the schema carries
    /// the column and the row covers it.
    pub fn declared_skids(&self) -> Option<usize> {
        self.column("num skid")
            .and_then(|v| (v >= 0.0 && v.is_finite()).then_some(v as usize))
    }
}

/// One skid sample covering a band (slippage or speed — see
/// [`SurfaceEntry::skid_columns`] for the authored unit).
#[derive(Debug, Clone)]
pub struct SkidSample {
    /// Sample name.
    pub name: String,
    /// Band start.
    pub min: f32,
    /// Band end.
    pub max: f32,
    /// 1-based source line.
    pub line: u32,
}

impl SurfaceTable {
    /// Parse a surface table.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut tunnel_index = None;
        let mut surfaces = Vec::new();
        let mut diags = Vec::new();
        let mut trailing = 0usize;
        let mut ended = false;
        let mut i = 0usize;
        while i < rows.len() {
            let (line, cells) = &rows[i];
            let line = *line;
            if ended {
                trailing += 1;
                i += 1;
                continue;
            }
            let first = cells.first().map(|c| c.trim()).unwrap_or("");
            if first.eq_ignore_ascii_case("endofdata") {
                ended = true;
                i += 1;
                continue;
            }
            if is_label_prefix(cells, "tunnel sound index") {
                i += 1;
                if let Some(&(vline, ref vcells)) = rows.get(i) {
                    tunnel_index = int_cell(vcells, 0, vline, &mut diags);
                    i += 1;
                }
                continue;
            }
            if is_label_prefix(cells, "surface wave") {
                let mut columns = cells.clone();
                while columns.last().is_some_and(|c| c.is_empty()) {
                    columns.pop();
                }
                i += 1;
                let Some(&(sline, ref scells)) = rows.get(i) else {
                    break;
                };
                if scells.len() < 2 || !scells[1..].iter().all(|c| c.is_empty() || is_num(c)) {
                    diags.push(TableDiagnostic {
                        line: sline,
                        message: format!("skipping surface row: {scells:?}"),
                    });
                    i += 1;
                    continue;
                }
                let values: Vec<f32> = scells[1..]
                    .iter()
                    .filter(|c| !c.is_empty())
                    .map(|c| c.parse().unwrap_or(f32::NAN))
                    .collect();
                if values.len() + 1 != columns.len() {
                    diags.push(TableDiagnostic {
                        line: sline,
                        message: format!(
                            "surface row has {} values, header names {}",
                            values.len(),
                            columns.len() - 1
                        ),
                    });
                }
                let mut entry = SurfaceEntry {
                    name: scells[0].clone(),
                    columns,
                    values,
                    skid_columns: Vec::new(),
                    skids: Vec::new(),
                    line: sline,
                };
                i += 1;
                if let Some((_, h)) = rows.get(i)
                    && is_label_prefix(h, "skid wave")
                {
                    entry.skid_columns = h.clone();
                    i += 1;
                }
                while i < rows.len() {
                    let (kline, kcells) = &rows[i];
                    let kline = *kline;
                    let kfirst = kcells.first().map(|c| c.trim()).unwrap_or("");
                    if kcells.len() < 3
                        || !is_num(&kcells[1])
                        || !is_num(&kcells[2])
                        || kfirst.eq_ignore_ascii_case("endofdata")
                    {
                        break;
                    }
                    entry.skids.push(SkidSample {
                        name: kcells[0].clone(),
                        min: kcells[1].parse().unwrap_or(f32::NAN),
                        max: kcells[2].parse().unwrap_or(f32::NAN),
                        line: kline,
                    });
                    i += 1;
                }
                surfaces.push(entry);
                continue;
            }
            diags.push(TableDiagnostic {
                line,
                message: format!("skipping line outside a surface section: {cells:?}"),
            });
            i += 1;
        }
        if surfaces.is_empty() && tunnel_index.is_none() {
            return Err(FormatError::parse(0, "no surface entries parsed"));
        }
        Ok(SurfaceTable {
            tunnel_index,
            surfaces,
            trailing_lines: trailing,
            diagnostics: diags,
        })
    }

    /// Sanity issues: declared-vs-parsed skid counts (when the schema
    /// carries the column), non-finite values, degenerate bands,
    /// content past `ENDOFDATA`.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        if self.trailing_lines > 0 {
            issues.push(CardataIssue::ContentAfterTerminator {
                context: "surfaces",
                lines: self.trailing_lines,
            });
        }
        for s in &self.surfaces {
            if let Some(declared) = s.declared_skids()
                && declared != s.skids.len()
            {
                issues.push(CardataIssue::DeclaredVsParsed {
                    context: "skid samples",
                    declared,
                    parsed: s.skids.len(),
                });
            }
            for v in &s.values {
                check_finite("surfaces", &mut issues, "value", *v, s.line);
            }
            for k in &s.skids {
                check_range("surfaces", &mut issues, "skid band", k.min, k.max, k.line);
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// SirenProgram — *policesiren.csv
// ---------------------------------------------------------------------------

/// A siren program: an optional explosion sample plus named sequences of
/// `(play time, next index)` steps forming a state machine. Step indices
/// are authored references into the same sample's step list (retail
/// tables index their own steps, not across samples — unverified beyond
/// the authored values).
#[derive(Debug, Clone)]
pub struct SirenProgram {
    /// `Explosion sample` binding when present (player-city sirens
    /// carry one; the opponent table does not).
    pub explosion: Option<NamedVolume>,
    /// Named siren sequences.
    pub samples: Vec<SirenSample>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One named siren sequence.
#[derive(Debug, Clone)]
pub struct SirenSample {
    /// Sample name.
    pub name: String,
    /// Authored volume when the row carries one (player-city files do;
    /// `opponent/policesiren.csv` leaves it empty).
    pub volume: Option<f32>,
    /// `(play time, next index)` steps.
    pub steps: Vec<SirenStep>,
    /// 1-based source line.
    pub line: u32,
}

/// One siren step.
#[derive(Debug, Clone)]
pub struct SirenStep {
    /// Seconds this step plays.
    pub play_time: f32,
    /// Authored next-step index.
    pub next_index: i64,
    /// 1-based source line.
    pub line: u32,
}

impl SirenProgram {
    /// Parse a siren program.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut explosion = None;
        let mut samples: Vec<SirenSample> = Vec::new();
        let mut diags = Vec::new();
        let mut i = 0usize;
        while i < rows.len() {
            let (line, cells) = &rows[i];
            let line = *line;
            if is_label_prefix(cells, "explosion sample") {
                i += 1;
                if let Some(&(vline, ref vcells)) = rows.get(i) {
                    explosion = Some(NamedVolume {
                        name: vcells.first().cloned().unwrap_or_default(),
                        volume: f32_cell(vcells, 1, vline, &mut diags).unwrap_or(f32::NAN),
                        line: vline,
                    });
                    i += 1;
                }
                continue;
            }
            if is_label_prefix(cells, "sample name") {
                i += 1;
                let Some(&(nline, ref ncells)) = rows.get(i) else {
                    break;
                };
                let mut sample = SirenSample {
                    name: ncells.first().cloned().unwrap_or_default(),
                    volume: ncells.get(1).and_then(|c| c.parse().ok()),
                    steps: Vec::new(),
                    line: nline,
                };
                i += 1;
                // Step rows: retail repeats the `play time,next index`
                // header before *every* step, so accept the label
                // interleaved or written once.
                while i < rows.len() {
                    let (sline, scells) = &rows[i];
                    let sline = *sline;
                    if is_label_prefix(scells, "play time") {
                        i += 1;
                        continue;
                    }
                    if !is_num(scells.first().map(|c| c.as_str()).unwrap_or("")) {
                        break;
                    }
                    let play_time = scells[0].parse().unwrap_or(f32::NAN);
                    let next_index = int_cell(scells, 1, sline, &mut diags).unwrap_or(0);
                    sample.steps.push(SirenStep {
                        play_time,
                        next_index,
                        line: sline,
                    });
                    i += 1;
                }
                samples.push(sample);
                continue;
            }
            if is_label_prefix(cells, "play time") {
                // A stray step header outside a sample block.
                i += 1;
                continue;
            }
            diags.push(TableDiagnostic {
                line,
                message: format!("skipping line outside a siren section: {cells:?}"),
            });
            i += 1;
        }
        if samples.is_empty() && explosion.is_none() {
            return Err(FormatError::parse(0, "no siren samples parsed"));
        }
        Ok(SirenProgram {
            explosion,
            samples,
            diagnostics: diags,
        })
    }

    /// Sanity issues: non-finite times and negative step indices. (No
    /// upper bound is enforced — whether authored indices index the
    /// owning sample's steps or a merged runtime table is unverified.)
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        if let Some(e) = &self.explosion {
            check_finite("sirens", &mut issues, "explosion volume", e.volume, e.line);
        }
        for s in &self.samples {
            for st in &s.steps {
                check_finite("sirens", &mut issues, "play time", st.play_time, st.line);
                if st.next_index < 0 {
                    issues.push(CardataIssue::DegenerateRange {
                        context: "sirens",
                        field: "next index",
                        line: st.line,
                    });
                }
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// AmbientEngine / AmbientHorn — aud/cardata/ambient/va_*.csv
// ---------------------------------------------------------------------------

/// An ambient-car engine table: one loop plus speed-ranged pitch bands.
#[derive(Debug, Clone)]
pub struct AmbientEngine {
    /// Engine sample name + volume.
    pub sample: NamedVolume,
    /// `(min speed, max speed, min pitch, max pitch)` bands — retail
    /// rows overlap deliberately (a catch-all 0–500 row follows the
    /// tight bands); order is preserved.
    pub ranges: Vec<SpeedBand>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// A speed-ranged pitch band.
#[derive(Debug, Clone)]
pub struct SpeedBand {
    /// Speed band start.
    pub min_speed: f32,
    /// Speed band end.
    pub max_speed: f32,
    /// Pitch at the band's low end.
    pub min_pitch: f32,
    /// Pitch at the band's high end.
    pub max_pitch: f32,
    /// 1-based source line.
    pub line: u32,
}

impl AmbientEngine {
    /// Parse an ambient engine table.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut sample = None;
        let mut ranges = Vec::new();
        let mut diags = Vec::new();
        let mut in_bands = false;
        for (line, cells) in &rows {
            let line = *line;
            if is_label_prefix(cells, "engine sample") {
                continue;
            }
            if is_label_prefix(cells, "min speed") {
                in_bands = true;
                continue;
            }
            if !in_bands {
                if sample.is_none() && cells.len() >= 2 && is_num(&cells[1]) {
                    sample = Some(NamedVolume {
                        name: cells[0].clone(),
                        volume: cells[1].parse().unwrap_or(f32::NAN),
                        line,
                    });
                } else {
                    diags.push(TableDiagnostic {
                        line,
                        message: format!("skipping row: {cells:?}"),
                    });
                }
                continue;
            }
            if cells.len() < 4 || !cells[..4].iter().all(|c| is_num(c)) {
                diags.push(TableDiagnostic {
                    line,
                    message: format!("skipping speed band: {cells:?}"),
                });
                continue;
            }
            let f = |i: usize| cells[i].parse().unwrap_or(f32::NAN);
            ranges.push(SpeedBand {
                min_speed: f(0),
                max_speed: f(1),
                min_pitch: f(2),
                max_pitch: f(3),
                line,
            });
        }
        let Some(sample) = sample else {
            return Err(FormatError::parse(0, "no ambient engine sample parsed"));
        };
        Ok(AmbientEngine {
            sample,
            ranges,
            diagnostics: diags,
        })
    }

    /// Sanity issues: degenerate speed bands and non-finite values.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        check_finite(
            "ambient engine",
            &mut issues,
            "sample volume",
            self.sample.volume,
            self.sample.line,
        );
        for b in &self.ranges {
            check_range(
                "ambient engine",
                &mut issues,
                "speed",
                b.min_speed,
                b.max_speed,
                b.line,
            );
            check_range(
                "ambient engine",
                &mut issues,
                "pitch",
                b.min_pitch,
                b.max_pitch,
                b.line,
            );
        }
        issues
    }
}

/// An ambient-car horn table: the horn sample plus repeated
/// `horn play duration,horn pause duration` groups — the authored
/// stuck-in-traffic honk patterns.
#[derive(Debug, Clone)]
pub struct AmbientHorn {
    /// Horn binding.
    pub horn: AmbientHornDef,
    /// Play/pause sequence groups, in authored order.
    pub sequences: Vec<Vec<HornBlip>>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// The horn sample row.
#[derive(Debug, Clone)]
pub struct AmbientHornDef {
    /// Sample name.
    pub name: String,
    /// Authored volume.
    pub volume: f32,
    /// Authored pitch.
    pub pitch: f32,
    /// `min stuck horn impact force` — the collision force that starts a
    /// stuck-honk sequence.
    pub min_stuck_impact: f32,
    /// 1-based source line.
    pub line: u32,
}

/// One play/pause pair inside a honk sequence.
#[derive(Debug, Clone)]
pub struct HornBlip {
    /// Seconds the horn sounds.
    pub play: f32,
    /// Seconds of silence after it.
    pub pause: f32,
    /// 1-based source line.
    pub line: u32,
}

impl AmbientHorn {
    /// Parse an ambient horn table.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut horn = None;
        let mut sequences: Vec<Vec<HornBlip>> = Vec::new();
        let mut diags = Vec::new();
        for (line, cells) in &rows {
            let line = *line;
            if is_label_prefix(cells, "horn sample") {
                continue;
            }
            if is_label_prefix(cells, "horn play duration") {
                sequences.push(Vec::new());
                continue;
            }
            if horn.is_none() {
                if cells.len() >= 4 && cells[1..].iter().take(3).all(|c| is_num(c)) {
                    horn = Some(AmbientHornDef {
                        name: cells[0].clone(),
                        volume: cells[1].parse().unwrap_or(f32::NAN),
                        pitch: cells[2].parse().unwrap_or(f32::NAN),
                        min_stuck_impact: cells[3].parse().unwrap_or(f32::NAN),
                        line,
                    });
                } else {
                    diags.push(TableDiagnostic {
                        line,
                        message: format!("skipping row: {cells:?}"),
                    });
                }
                continue;
            }
            if cells.len() < 2 || !is_num(&cells[0]) || !is_num(&cells[1]) {
                diags.push(TableDiagnostic {
                    line,
                    message: format!("skipping play/pause row: {cells:?}"),
                });
                continue;
            }
            let blip = HornBlip {
                play: cells[0].parse().unwrap_or(f32::NAN),
                pause: cells[1].parse().unwrap_or(f32::NAN),
                line,
            };
            match sequences.last_mut() {
                Some(seq) => seq.push(blip),
                None => sequences.push(vec![blip]),
            }
        }
        let Some(horn) = horn else {
            return Err(FormatError::parse(0, "no ambient horn row parsed"));
        };
        Ok(AmbientHorn {
            horn,
            sequences,
            diagnostics: diags,
        })
    }

    /// Sanity issues: non-finite values.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        check_finite(
            "ambient horn",
            &mut issues,
            "horn volume",
            self.horn.volume,
            self.horn.line,
        );
        check_finite(
            "ambient horn",
            &mut issues,
            "horn pitch",
            self.horn.pitch,
            self.horn.line,
        );
        check_finite(
            "ambient horn",
            &mut issues,
            "min stuck impact",
            self.horn.min_stuck_impact,
            self.horn.line,
        );
        for seq in &self.sequences {
            for b in seq {
                check_finite("ambient horn", &mut issues, "play duration", b.play, b.line);
                check_finite(
                    "ambient horn",
                    &mut issues,
                    "pause duration",
                    b.pause,
                    b.line,
                );
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// ObjectAudio — aud/ambient/*.csv, aud/cardata/ambient/subwaycar.csv
// ---------------------------------------------------------------------------

/// A distance-ranged ambient emitter table (the `aud3dobject`-style
/// files): a distance window and priority, a list of clips, and an
/// optional `VECTORPOINTS` block of emitter positions. Which rows are
/// one-shots vs loops is carried by the authored `sample type` field;
/// its value vocabulary is unverified beyond the observed 0/1.
#[derive(Debug, Clone)]
pub struct ObjectAudio {
    /// `Min distance` — closer than this, the emitter does not play.
    pub min_distance: f32,
    /// `Max distance` — farther than this, the emitter does not play.
    pub max_distance: f32,
    /// Authored `3D priority`.
    pub priority: f32,
    /// Authored `Audible area` when the column is present.
    pub audible_area: Option<f32>,
    /// Clip rows in authored order.
    pub samples: Vec<ObjectSample>,
    /// `VECTORPOINTS` emitter positions, when present.
    pub vector_points: Vec<[f32; 3]>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One ambient clip row.
#[derive(Debug, Clone)]
pub struct ObjectSample {
    /// Sample name.
    pub name: String,
    /// Authored volume.
    pub volume: f32,
    /// Authored `sample type` (0/1 observed on retail; vocabulary
    /// unverified).
    pub kind: f32,
    /// One-shot replay window: minimum seconds between plays.
    pub oneshot_low: f32,
    /// One-shot replay window: maximum seconds between plays.
    pub oneshot_high: f32,
    /// Authored `active` flag (1 observed).
    pub active: f32,
    /// Emitter-speed window start (moving emitters like the subway).
    pub min_speed: f32,
    /// Emitter-speed window end.
    pub max_speed: f32,
    /// Authored doppler flag.
    pub doppler: f32,
    /// 1-based source line.
    pub line: u32,
}

impl ObjectAudio {
    /// Parse an object-audio table.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut diags = Vec::new();
        let mut range: Option<(f32, f32, f32, Option<f32>)> = None;
        let mut samples = Vec::new();
        let mut points = Vec::new();
        enum Sec {
            Head,
            Samples,
            Vectors,
        }
        let mut sec = Sec::Head;
        for (line, cells) in &rows {
            let line = *line;
            if is_label_prefix(cells, "min distance") {
                continue;
            }
            if is_label_prefix(cells, "sample name") {
                sec = Sec::Samples;
                continue;
            }
            if is_label_prefix(cells, "vectorpoints")
                || (cells.first().is_some_and(|c| c == "x")
                    && cells.get(1).is_some_and(|c| c == "y"))
            {
                sec = Sec::Vectors;
                continue;
            }
            match sec {
                Sec::Head if range.is_none() => {
                    if cells.len() >= 3 && cells[..3].iter().all(|c| is_num(c)) {
                        range = Some((
                            cells[0].parse().unwrap_or(f32::NAN),
                            cells[1].parse().unwrap_or(f32::NAN),
                            cells[2].parse().unwrap_or(f32::NAN),
                            cells.get(3).and_then(|c| c.parse().ok()),
                        ));
                    } else {
                        diags.push(TableDiagnostic {
                            line,
                            message: format!("skipping distance row: {cells:?}"),
                        });
                    }
                }
                Sec::Samples => {
                    if cells.len() >= 9 && cells[1..].iter().take(8).all(|c| is_num(c)) {
                        let f = |i: usize| cells[i].parse().unwrap_or(f32::NAN);
                        samples.push(ObjectSample {
                            name: cells[0].clone(),
                            volume: f(1),
                            kind: f(2),
                            oneshot_low: f(3),
                            oneshot_high: f(4),
                            active: f(5),
                            min_speed: f(6),
                            max_speed: f(7),
                            doppler: f(8),
                            line,
                        });
                    } else {
                        diags.push(TableDiagnostic {
                            line,
                            message: format!("skipping sample row: {cells:?}"),
                        });
                    }
                }
                Sec::Vectors => {
                    if cells.len() >= 3 && cells[..3].iter().all(|c| is_num(c)) {
                        points.push([
                            cells[0].parse().unwrap_or(f32::NAN),
                            cells[1].parse().unwrap_or(f32::NAN),
                            cells[2].parse().unwrap_or(f32::NAN),
                        ]);
                    } else {
                        diags.push(TableDiagnostic {
                            line,
                            message: format!("skipping vector row: {cells:?}"),
                        });
                    }
                }
                Sec::Head => {
                    diags.push(TableDiagnostic {
                        line,
                        message: format!("skipping row: {cells:?}"),
                    });
                }
            }
        }
        let Some((min_distance, max_distance, priority, audible_area)) = range else {
            return Err(FormatError::parse(0, "no distance row parsed"));
        };
        Ok(ObjectAudio {
            min_distance,
            max_distance,
            priority,
            audible_area,
            samples,
            vector_points: points,
            diagnostics: diags,
        })
    }

    /// Sanity issues: degenerate distance/speed windows and non-finite
    /// values.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        check_range(
            "object audio",
            &mut issues,
            "distance",
            self.min_distance,
            self.max_distance,
            0,
        );
        for s in &self.samples {
            check_range(
                "object audio",
                &mut issues,
                "one-shot window",
                s.oneshot_low,
                s.oneshot_high,
                s.line,
            );
            check_range(
                "object audio",
                &mut issues,
                "speed window",
                s.min_speed,
                s.max_speed,
                s.line,
            );
            check_finite(
                "object audio",
                &mut issues,
                "sample volume",
                s.volume,
                s.line,
            );
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// AmbientContainer — aud/ambient/*container*.csv
// ---------------------------------------------------------------------------

/// An `aud/ambient/*container*.csv` file: a `file names` list of sibling
/// `aud/ambient/<stem>.csv` tables that together form a city's ambient
/// soundscape (retail: `londonambientcontainer` → `londonriver` +
/// `tubevoices`).
#[derive(Debug, Clone)]
pub struct AmbientContainer {
    /// Referenced sibling table stems, in authored order.
    pub files: Vec<String>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

impl AmbientContainer {
    /// Parse a container's `file names` list.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut files = Vec::new();
        let mut diags = Vec::new();
        for (line, cells) in &rows {
            let line = *line;
            if is_label_prefix(cells, "file names") {
                continue;
            }
            match cells.first() {
                Some(name) if !name.is_empty() => files.push(name.clone()),
                _ => diags.push(TableDiagnostic {
                    line,
                    message: format!("skipping container row: {cells:?}"),
                }),
            }
        }
        if files.is_empty() {
            return Err(FormatError::parse(0, "no container entries parsed"));
        }
        Ok(AmbientContainer {
            files,
            diagnostics: diags,
        })
    }

    /// Sanity issues: none defined — a name list carries no ranges.
    pub fn validate(&self) -> Vec<CardataIssue> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// SemiData / VehTypes — aud/cardata/shared/*.csv
// ---------------------------------------------------------------------------

/// `shared/semidata.csv`: the reverse-gear beeper and air-brake samples
/// shared by semi/bus vehicles.
#[derive(Debug, Clone)]
pub struct SemiData {
    /// Reverse-gear sample name.
    pub reverse_sample: String,
    /// Air-blow sample name.
    pub air_blow_sample: String,
    /// Reverse-gear volume.
    pub reverse_volume: f32,
    /// Air-blow volume.
    pub air_blow_volume: f32,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

impl SemiData {
    /// Parse the semidata table (one label row, one data row).
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut diags = Vec::new();
        for (line, cells) in &rows {
            let line = *line;
            if is_label_prefix(cells, "reverse sample") {
                continue;
            }
            if cells.len() >= 4 && is_num(&cells[2]) && is_num(&cells[3]) {
                return Ok(SemiData {
                    reverse_sample: cells[0].clone(),
                    air_blow_sample: cells[1].clone(),
                    reverse_volume: cells[2].parse().unwrap_or(f32::NAN),
                    air_blow_volume: cells[3].parse().unwrap_or(f32::NAN),
                    diagnostics: diags,
                });
            }
            diags.push(TableDiagnostic {
                line,
                message: format!("skipping row: {cells:?}"),
            });
        }
        Err(FormatError::parse(0, "no semidata row parsed"))
    }

    /// Sanity issues: non-finite volumes.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        check_finite(
            "semidata",
            &mut issues,
            "reverse volume",
            self.reverse_volume,
            0,
        );
        check_finite(
            "semidata",
            &mut issues,
            "air blow volume",
            self.air_blow_volume,
            0,
        );
        issues
    }
}

/// `shared/vehtypes.csv`: vehicle-class label → vehicle-id list rows
/// (`Semi or bus`, `Police Car`, `Always nitro`). Members are preserved
/// verbatim — terminator cells like `ENDOFDATA`/`FALSE` are authored
/// data, not structure.
#[derive(Debug, Clone)]
pub struct VehTypes {
    /// Label/member row pairs in authored order.
    pub groups: Vec<VehTypeGroup>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One vehicle-class row pair.
#[derive(Debug, Clone)]
pub struct VehTypeGroup {
    /// Authored class label (`Semi or bus`, `Police Car`, …).
    pub label: String,
    /// Member cells verbatim, trailing empties dropped.
    pub members: Vec<String>,
    /// 1-based source line of the label row.
    pub line: u32,
}

impl VehTypes {
    /// Parse the vehicle-types table: alternating label rows and
    /// comma-list member rows.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut groups = Vec::new();
        let mut diags = Vec::new();
        let mut pending: Option<(String, u32)> = None;
        for (line, cells) in &rows {
            let line = *line;
            match pending.take() {
                Some((label, lline)) => {
                    let mut members: Vec<String> = cells.clone();
                    while members.last().is_some_and(|c| c.is_empty()) {
                        members.pop();
                    }
                    groups.push(VehTypeGroup {
                        label,
                        members,
                        line: lline,
                    });
                }
                None => {
                    // A label row: first cell is text, the rest empty.
                    if cells.iter().skip(1).all(|c| c.is_empty()) && !cells[0].is_empty() {
                        pending = Some((cells[0].clone(), line));
                    } else {
                        diags.push(TableDiagnostic {
                            line,
                            message: format!("expected a label row, have {cells:?}"),
                        });
                    }
                }
            }
        }
        if groups.is_empty() {
            return Err(FormatError::parse(0, "no vehicle-type groups parsed"));
        }
        Ok(VehTypes {
            groups,
            diagnostics: diags,
        })
    }

    /// Sanity issues: groups with no members.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        for g in &self.groups {
            if g.members.is_empty() {
                issues.push(CardataIssue::DeclaredVsParsed {
                    context: "vehicle-type members",
                    declared: 1,
                    parsed: 0,
                });
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// BandTable — suspensionaudio.csv / tirewobble.csv
// ---------------------------------------------------------------------------

/// The small `player/` band tables (`suspensionaudio.csv`,
/// `tirewobble.csv`): a header naming the numeric columns, then rows of
/// `sample name` + that many floats.
#[derive(Debug, Clone)]
pub struct BandTable {
    /// Authored column names, verbatim.
    pub columns: Vec<String>,
    /// Data rows.
    pub rows: Vec<BandRow>,
    /// Recoverable problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One band-table row.
#[derive(Debug, Clone)]
pub struct BandRow {
    /// Sample name.
    pub name: String,
    /// Positional values matching `columns[1..]`.
    pub values: Vec<f32>,
    /// 1-based source line.
    pub line: u32,
}

impl BandTable {
    /// Parse a band table.
    pub fn parse(input: &[u8]) -> Result<Self, FormatError> {
        let rows = rows(input);
        let mut columns: Option<Vec<String>> = None;
        let mut out = Vec::new();
        let mut diags = Vec::new();
        for (line, cells) in &rows {
            let line = *line;
            match &columns {
                None => {
                    // The header row: a first text cell followed by more
                    // labels. A data row has a numeric second cell.
                    if cells.len() >= 2 && is_num(&cells[1]) {
                        columns = Some(Vec::new()); // headerless
                        let values: Option<Vec<f32>> = cells[1..]
                            .iter()
                            .filter(|c| !c.is_empty())
                            .map(|c| c.parse().ok())
                            .collect();
                        if let Some(values) = values {
                            out.push(BandRow {
                                name: cells[0].clone(),
                                values,
                                line,
                            });
                        }
                    } else {
                        columns = Some(cells.clone());
                    }
                }
                Some(cols) => {
                    let want = if cols.is_empty() {
                        cells.len().saturating_sub(1)
                    } else {
                        cols.len() - 1
                    };
                    let values: Option<Vec<f32>> = cells[1..]
                        .iter()
                        .filter(|c| !c.is_empty())
                        .map(|c| c.parse().ok())
                        .collect();
                    match values {
                        Some(values) => out.push(BandRow {
                            name: cells[0].clone(),
                            values: {
                                if values.len() != want {
                                    diags.push(TableDiagnostic {
                                        line,
                                        message: format!(
                                            "row has {} values, header names {want}",
                                            values.len()
                                        ),
                                    });
                                }
                                values
                            },
                            line,
                        }),
                        None => diags.push(TableDiagnostic {
                            line,
                            message: format!("skipping non-numeric row: {cells:?}"),
                        }),
                    }
                }
            }
        }
        if out.is_empty() {
            return Err(FormatError::parse(0, "no band rows parsed"));
        }
        Ok(BandTable {
            columns: columns.unwrap_or_default(),
            rows: out,
            diagnostics: diags,
        })
    }

    /// Sanity issues: non-finite values.
    pub fn validate(&self) -> Vec<CardataIssue> {
        let mut issues = Vec::new();
        for r in &self.rows {
            for v in &r.values {
                check_finite("band table", &mut issues, "value", *v, r.line);
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- CarAudio -----------------------------------------------------

    /// The real `aud/cardata/player/vpbug.csv`, verbatim.
    const VP_BUG: &[u8] = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume,,,,,\r\nVWHORN,0.95,0,4,REVERSE,0.93,,,,,\r\nEngine wave name,Min Volume,Max Volume,fade in  start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\r\nVWIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\r\nVWDRIVE,0.55,0.9,500,2800,6500,10000,0.6,2.5,500,12000\r\nVWMID,0.55,0.85,500,4000,7000,11000,1,2.25,500,11000\r\nVWHIGH,0.55,0.91,3000,8000,15000,15000,0.65,2.25,3000,12000\r\n";

    #[test]
    fn car_audio_parses_retail_vpbug() {
        let f = parse("aud/cardata/player/vpbug.csv", VP_BUG).unwrap();
        assert_eq!(f.kind, CardataKind::CarAudio);
        let CardataBody::Car(car) = &f.body else {
            panic!("expected car body");
        };
        assert_eq!(car.horn.name, "VWHORN");
        assert_eq!(car.horn.volume, 0.95);
        assert_eq!(car.flags, 0);
        assert_eq!(car.declared_engine_samples, 4);
        assert_eq!(car.clutch.name, "REVERSE");
        assert_eq!(car.engine_samples.len(), 4);
        let idle = &car.engine_samples[0];
        assert_eq!(idle.name, "VWIDLE");
        assert_eq!(idle.values.len(), 10);
        // Column lookup normalizes the header's stray double spaces.
        assert_eq!(idle.column("min volume"), Some(0.55));
        assert_eq!(idle.column("max volume"), Some(0.835));
        assert_eq!(idle.column("fade in start"), Some(1.0));
        assert_eq!(idle.column("fade in end"), Some(800.0));
        assert_eq!(idle.column("fade out start"), Some(2500.0));
        assert_eq!(idle.column("fade out end"), Some(7000.0));
        assert_eq!(idle.column("min pitch"), Some(0.85));
        assert_eq!(idle.column("max pitch"), Some(2.0));
        assert_eq!(idle.column("pitch shift start"), Some(1.0));
        assert_eq!(idle.column("pitch shift end"), Some(7000.0));
        assert!(car.diagnostics.is_empty());
        assert!(car.validate().is_empty());
        let names = f.body.wave_names();
        assert!(names.contains(&"VWHORN"));
        assert!(names.contains(&"VWIDLE"));
        assert!(names.contains(&"VWHIGH"));
    }

    #[test]
    fn car_audio_parses_the_compact_work_file_schema() {
        // `copy of default.csv` (a development work copy) uses the
        // 7-column divisor schema rather than fade windows.
        let data = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume,,\r\nRACECARHORN,0.95,0,2,REVERSE,0.93,,\r\nEngine wave name,Min Volume,Max Volume,Volume Divisor,Min Pitch,Max Pitch,Pitch Divisor,vol inverse RPM\r\nRACECARIDLE,0.100000,0.960000,1700.000000,0.800000,1.640000,1849.999955,0.000000\r\nRACECARDRIVE,0.100000,0.940000,2000.000000,0.650000,4.500000,6750.000146,1000000000.000000\r\n";
        let f = parse("aud/cardata/player/copy of default.csv", data).unwrap();
        let CardataBody::Car(car) = &f.body else {
            panic!("expected car body");
        };
        assert_eq!(car.engine_samples.len(), 2);
        let idle = &car.engine_samples[0];
        assert_eq!(idle.name, "RACECARIDLE");
        assert_eq!(idle.column("volume divisor"), Some(1700.0));
        assert_eq!(idle.column("pitch divisor"), Some(1849.999955f32));
        assert_eq!(idle.column("vol inverse rpm"), Some(0.0));
        // The fade-window columns simply don't exist in this schema.
        assert_eq!(idle.column("fade in start"), None);
        assert!(car.diagnostics.is_empty());
        assert!(car.validate().is_empty());
    }

    #[test]
    fn car_audio_count_mismatch_is_an_issue() {
        // Declared 4, only 2 rows present — a finding, not a failure.
        let data = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nH,0.9,0,4,C,0.5\nEngine wave name,a,b,c,d,e,f,g,h,i,j\nS1,0.1,0.2,1,2,3,4,0.5,1,0,9\nS2,0.1,0.2,1,2,3,4,0.5,1,0,9\n";
        let f = parse("aud/cardata/opponent/vpx.csv", data).unwrap();
        let CardataBody::Car(car) = &f.body else {
            panic!("expected car body");
        };
        assert_eq!(car.engine_samples.len(), 2);
        assert!(car.validate().contains(&CardataIssue::DeclaredVsParsed {
            context: "engine samples",
            declared: 4,
            parsed: 2,
        }));
    }

    #[test]
    fn car_audio_empty_is_an_error() {
        assert!(parse("aud/cardata/player/vpx.csv", b"\n\n").is_err());
    }

    // -- EngineParams ---------------------------------------------------

    /// The real `engineparamsplay.csv` head: 0xCD fill + junk + `,` +
    /// nine `%.6f` floats per row.
    fn engineparams_row(fill: u8, junk: &[u8], floats: &str) -> Vec<u8> {
        let mut v = vec![fill; 80];
        v.extend_from_slice(junk);
        v.push(b',');
        v.extend_from_slice(floats.as_bytes());
        v.extend_from_slice(b"\r\n");
        v
    }

    #[test]
    fn engineparams_preserves_binary_name_prefix() {
        let mut data = engineparams_row(
            0xCD,
            &[0xd4, 0x52, 0x88],
            "0.100000,0.950000,0.000000,0.000000,3000.000000,6000.000000,0.800000,4.500000,2500.000000",
        );
        data.extend_from_slice(&engineparams_row(
            0xDD,
            &[0xd1, 0x47, 0x72, 0x05, 0x14],
            "0.100000,0.900000,1000.000000,2000.000000,6000.000000,15000.000000,0.450000,4.500000,3999.999756",
        ));
        let f = parse("aud/cardata/engineparamsplay.csv", &data).unwrap();
        assert_eq!(f.kind, CardataKind::EngineParams);
        let CardataBody::EngineParams(p) = &f.body else {
            panic!("expected engineparams body");
        };
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].values.len(), 9);
        assert_eq!(p.rows[0].values[4], 3000.0);
        assert_eq!(p.rows[0].name_text(), None);
        // The name field is preserved raw — 80 fill + 3 junk bytes.
        assert_eq!(p.rows[0].name_raw.len(), 83);
        assert_eq!(p.rows[0].name_raw[0], 0xCD);
        assert_eq!(p.rows[1].name_raw[0], 0xDD);
        assert!(
            p.validate()
                .iter()
                .any(|i| matches!(i, CardataIssue::BinaryNameField { len: 83, .. }))
        );
    }

    #[test]
    fn engineparams_tolerates_commas_inside_name_blob() {
        // A `,` inside the binary prefix must not shift the float tail.
        let mut data = vec![0xCD; 40];
        data.extend_from_slice(&[b',', 0xCD, 0xCD]);
        data.push(b',');
        data.extend_from_slice(b"1,2,3,4,5,6,7,8,9\n");
        let f = parse("aud/cardata/engineparamsopp.csv", &data).unwrap();
        let CardataBody::EngineParams(p) = &f.body else {
            panic!("expected engineparams body");
        };
        assert_eq!(p.rows[0].values, vec![1., 2., 3., 4., 5., 6., 7., 8., 9.]);
        assert_eq!(p.rows[0].name_raw.len(), 43);
    }

    #[test]
    fn engineparams_ascii_names_still_parse() {
        let data = b"myengine,0.1,0.9,0,0,100,200,0.5,4.5,999\n";
        let f = parse("aud/cardata/engineparamsplay.csv", data).unwrap();
        let CardataBody::EngineParams(p) = &f.body else {
            panic!("expected engineparams body");
        };
        assert_eq!(p.rows[0].name_text(), Some("myengine"));
        assert!(p.validate().is_empty());
    }

    // -- ImpactTable ----------------------------------------------------

    /// Two sections carved from the real opponent `default_impacts.csv`.
    const IMPACTS: &[u8] = b"***,,,,,\r\nBanger name,Num samples,ID,,,\r\nWALL,3,0,,,\r\nsample name,min volume,max volume,min force,max force,frequency\r\nCarimpactsoft3,0.91,0.93,2,500,1\r\nCarimpactmed1,0.92,0.95,500,1500,1\r\nCarimpacthuge1,0.94,1,1500,999999,1\r\n***,,,,,\r\nBanger name,Num samples,ID,,,\r\nLIGHT,1,1,,,\r\nsample name,min volume,max volume,min force,max force,frequency\r\ntrafficlightimpact,0.95,0.98,0,999999,1\r\n***,,,,,\r\nBanger name,Num samples,ID,,,\r\nENDOFDATA,0,0,,,\r\n";

    #[test]
    fn impacts_parse_retail_sections() {
        let f = parse("aud/cardata/opponent/default_impacts.csv", IMPACTS).unwrap();
        let CardataBody::Impacts(t) = &f.body else {
            panic!("expected impacts body");
        };
        assert_eq!(t.categories.len(), 2);
        assert_eq!(t.categories[0].name, "WALL");
        assert_eq!(t.categories[0].id, 0);
        assert_eq!(t.categories[0].samples.len(), 3);
        assert_eq!(t.categories[1].name, "LIGHT");
        assert_eq!(t.categories[1].id, 1);
        assert_eq!(t.categories[1].samples[0].name, "trafficlightimpact");
        assert_eq!(t.categories[1].samples[0].max_force, 999999.0);
        assert!(t.diagnostics.is_empty());
        assert!(t.validate().is_empty());
        let names = f.body.wave_names();
        assert!(names.contains(&"Carimpactsoft3"));
    }

    #[test]
    fn impacts_missing_terminator_and_stray_content() {
        let data =
            b"Banger name,Num samples,ID\nWALL,1,0\nsample name,a,b,c,d,e\ns1,0.1,0.2,0,9,1\n";
        let f = parse("aud/cardata/player/default_impacts.csv", data).unwrap();
        let CardataBody::Impacts(t) = &f.body else {
            panic!("expected impacts body");
        };
        assert_eq!(t.categories.len(), 1);
        // Trailing junk after ENDOFDATA is counted.
        let mut data2 = IMPACTS.to_vec();
        data2.extend_from_slice(b"leftover,1,2\n");
        let f2 = parse("aud/cardata/player/default_impacts.csv", &data2).unwrap();
        let CardataBody::Impacts(t2) = &f2.body else {
            panic!("expected impacts body");
        };
        assert_eq!(t2.trailing_lines, 1);
        assert!(
            t2.validate()
                .contains(&CardataIssue::ContentAfterTerminator {
                    context: "impacts",
                    lines: 1,
                })
        );
    }

    #[test]
    fn impacts_quoted_separators_in_work_copies() {
        // `copy of default_impacts.csv` (an Excel resave) writes the
        // separator as `""""`.
        let data = b"\"\"\"\",,,,,\r\nbanger name,Num Samples,ID,,,\r\nWALL,1,0,,,\r\nsample name,min volume,max volume,min force,max force,frequency\r\nCarimpactsoft3,0.89,0.92,2,1000,1\r\n\"\"\"\",,,,,\r\nbanger name,Num Samples,ID,,,\r\nENDOFDATA,0,0,,,\r\n";
        let f = parse("aud/cardata/player/copy of default_impacts.csv", data).unwrap();
        let CardataBody::Impacts(t) = &f.body else {
            panic!("expected impacts body");
        };
        assert_eq!(t.categories.len(), 1);
        assert_eq!(t.categories[0].samples.len(), 1);
        assert!(t.diagnostics.is_empty());
    }

    // -- SurfaceTable ---------------------------------------------------

    /// The real opponent `default_surfacedry.csv` head.
    const SURFACES: &[u8] = b"Tunnel sound index\n0\nsurface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\nNOSOUND,125.000000,0.000000,0.000000,0.000000,0.000000,0.750000,0.960000,3\nskid wave,min slippage,max slippage\ntireskid1_ps1,0.250000,0.350000\ntireskid2_ps1,0.350000,0.550000\ntireskid3_ps1,0.550000,1.000000\nsurface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\nsurfacegrass,125.000000,0.350000,0.770000,0.850000,1.250000,0.750000,0.800000,1\nskid wave,min slippage,max slippage\nsurfacegrassskid,0.250000,1.000000\n";

    #[test]
    fn surfaces_parse_retail_blocks() {
        let f = parse("aud/cardata/opponent/default_surfacedry.csv", SURFACES).unwrap();
        let CardataBody::Surfaces(t) = &f.body else {
            panic!("expected surfaces body");
        };
        assert_eq!(t.tunnel_index, Some(0));
        assert_eq!(t.surfaces.len(), 2);
        let first = &t.surfaces[0];
        assert_eq!(first.name, "NOSOUND");
        assert_eq!(first.declared_skids(), Some(3));
        assert_eq!(first.skids.len(), 3);
        assert_eq!(first.skids[0].name, "tireskid1_ps1");
        assert_eq!(first.skids[0].max, 0.35);
        assert_eq!(first.column("max speed"), Some(125.0));
        assert_eq!(t.surfaces[1].name, "surfacegrass");
        assert_eq!(t.surfaces[1].skids.len(), 1);
        assert!(t.diagnostics.is_empty());
        assert!(t.validate().is_empty());
        // NOSOUND is a sentinel — dropped from wave refs.
        let names = f.body.wave_names();
        assert!(!names.contains(&"NOSOUND"));
        assert!(names.contains(&"surfacegrass"));
        assert!(names.contains(&"tireskid1_ps1"));
    }

    #[test]
    fn surfaces_parse_the_player_extended_schema() {
        // `player/default_surfaceice.csv`: 12 columns (divisors +
        // `for tunnels`), `min speed,max speed` skid bands, lowercase
        // `tunnel sound index`, and an `ENDOFDATA` terminator.
        let data = b"tunnel sound index,,,,,,,,,,,\r\n5,,,,,,,,,,,\r\nsurface wave,min surface volume,max surface volume,surface vol divisor,min surface pitch,max surface pitch,surface pitch divisor,min skid volume,max skid volume,skid vol divisor,num skid samples,for tunnels\r\nsurfacesnow,0.88,0.88,2,0.85,2,30,0.5,0.88,2,1,0\r\nskid wave ,min speed,max speed,,,,,,,,,\r\nsnowskid,0,1000,,,,,,,,,\r\nENDOFDATA,,,,,,,,,,,\r\n";
        let f = parse("aud/cardata/player/default_surfaceice.csv", data).unwrap();
        let CardataBody::Surfaces(t) = &f.body else {
            panic!("expected surfaces body");
        };
        assert_eq!(t.tunnel_index, Some(5));
        assert_eq!(t.surfaces.len(), 1);
        let s = &t.surfaces[0];
        assert_eq!(s.name, "surfacesnow");
        assert_eq!(s.values.len(), 11);
        assert_eq!(s.declared_skids(), Some(1));
        assert_eq!(s.column("for tunnels"), Some(0.0));
        assert_eq!(s.column("skid vol divisor"), Some(2.0));
        assert_eq!(s.skids.len(), 1);
        assert_eq!(s.skids[0].name, "snowskid");
        assert_eq!(s.skids[0].min, 0.0);
        assert_eq!(s.skids[0].max, 1000.0);
        assert_eq!(t.trailing_lines, 0);
        assert!(t.diagnostics.is_empty());
        assert!(t.validate().is_empty());
    }

    #[test]
    fn surfaces_report_short_rows_and_trailing_content() {
        let data = b"surface wave,a,b,c,d,e,f,g,h,num skid samples\ns1,1,2,3,4,5,6,7,8,9\nskid wave,a,b\nk1,0,1\nENDOFDATA\nextra,line\n";
        let f = parse("aud/cardata/player/default_surfacedry.csv", data).unwrap();
        let CardataBody::Surfaces(t) = &f.body else {
            panic!("expected surfaces body");
        };
        assert_eq!(t.trailing_lines, 1);
        assert!(
            t.validate()
                .contains(&CardataIssue::ContentAfterTerminator {
                    context: "surfaces",
                    lines: 1,
                })
        );
    }

    // -- SirenProgram ---------------------------------------------------

    /// The real opponent `policesiren.csv` head verbatim: no explosion
    /// row, no per-sample volume, and the `play time,next index` header
    /// repeated before *every* step.
    const SIREN: &[u8] = b"Sample name,\r\npolice1fastsirenloop,\r\nplay time,next index\r\n4.45,1\r\nplay time,next index\r\n2.65,2\r\nplay time,next index\r\n5.25,2\r\nplay time,next index\r\n4.15,1\r\nSample name,\r\npolice2hilowloop,\r\nplay time,next index\r\n2.31,2\r\nplay time,next index\r\n1.25,0\r\n";

    #[test]
    fn sirens_parse_retail_program() {
        let f = parse("aud/cardata/opponent/policesiren.csv", SIREN).unwrap();
        let CardataBody::Sirens(t) = &f.body else {
            panic!("expected sirens body");
        };
        assert!(t.explosion.is_none());
        assert_eq!(t.samples.len(), 2);
        assert_eq!(t.samples[0].name, "police1fastsirenloop");
        assert_eq!(t.samples[0].volume, None);
        assert_eq!(t.samples[0].steps.len(), 4);
        assert_eq!(t.samples[0].steps[0].play_time, 4.45);
        assert_eq!(t.samples[0].steps[0].next_index, 1);
        assert_eq!(t.samples[1].steps[1].next_index, 0);
        assert!(t.diagnostics.is_empty());
        assert!(t.validate().is_empty());
    }

    #[test]
    fn sirens_parse_explosion_variant() {
        // `player/sfpolicesiren.csv` carries an explosion row and a
        // per-sample volume column.
        let data = b"Explosion sample,volume\nexplosion,0.95\nSample name,\nsiren_london,0.95\nplay time,next index\n15,1\n";
        let f = parse("aud/cardata/player/sfpolicesiren.csv", data).unwrap();
        let CardataBody::Sirens(t) = &f.body else {
            panic!("expected sirens body");
        };
        assert_eq!(t.explosion.as_ref().unwrap().name, "explosion");
        assert_eq!(t.samples[0].volume, Some(0.95));
        assert!(f.body.wave_names().contains(&"explosion"));
    }

    // -- AmbientEngine / AmbientHorn -------------------------------------

    /// Real `aud/cardata/ambient/default_engine.csv`.
    const AMB_ENGINE: &[u8] = b"Engine sample,engine volume,,\r\nenginesedan1,0.98,,\r\nmin speed,max speed,engine min pitch,engine max pitch\r\n0,33,0.317,1\r\n33,500,1,1.25\r\n0,500,0.317,13.977\r\n";

    /// Real `aud/cardata/ambient/default_horn.csv`.
    const AMB_HORN: &[u8] = b"Horn sample,horn volume,horn pitch,min stuck horn impact force\r\nCARHORNAMB1,0.97,1,5500\r\nhorn play duration,horn pause duration,,\r\n0.5,0.2,,\r\n1.5,0,,\r\nhorn play duration,horn pause duration,,\r\n0.25,0.1,,\r\n0.15,0.1,,\r\n0.25,0.07,,\r\n1,0,,\r\nhorn play duration,horn pause duration,,\r\n0,0.15,,\r\n3,0,,\r\n";

    #[test]
    fn ambient_engine_parses_speed_bands() {
        let f = parse("aud/cardata/ambient/default_engine.csv", AMB_ENGINE).unwrap();
        let CardataBody::AmbientEngine(e) = &f.body else {
            panic!("expected ambient engine body");
        };
        assert_eq!(e.sample.name, "enginesedan1");
        assert_eq!(e.sample.volume, 0.98);
        assert_eq!(e.ranges.len(), 3);
        // The third row is the authored catch-all — preserved, not
        // deduplicated.
        assert_eq!(e.ranges[2].min_speed, 0.0);
        assert_eq!(e.ranges[2].max_pitch, 13.977);
        assert!(e.diagnostics.is_empty());
        assert!(e.validate().is_empty());
    }

    #[test]
    fn ambient_horn_parses_honk_sequences() {
        let f = parse("aud/cardata/ambient/default_horn.csv", AMB_HORN).unwrap();
        let CardataBody::AmbientHorn(h) = &f.body else {
            panic!("expected ambient horn body");
        };
        assert_eq!(h.horn.name, "CARHORNAMB1");
        assert_eq!(h.horn.min_stuck_impact, 5500.0);
        assert_eq!(h.sequences.len(), 3);
        assert_eq!(h.sequences[0].len(), 2);
        assert_eq!(h.sequences[0][0].play, 0.5);
        assert_eq!(h.sequences[0][0].pause, 0.2);
        assert_eq!(h.sequences[2][1].play, 3.0);
        assert!(h.diagnostics.is_empty());
        assert!(h.validate().is_empty());
    }

    // -- ObjectAudio ----------------------------------------------------

    /// Real `aud/cardata/ambient/subwaycar.csv`.
    const SUBWAY: &[u8] = b"Min distance,Max distance,3D priority,,,,,,\r\n0,150,12,,,,,,\r\nsample name,sample volume,sample type,oneshot time limit low,oneshot time limit high,active,min speed,max speed,doppler\r\nLondonTube,0.98,0,0,0,1,0,999999,1\r\n";

    /// `aud/ambient/birdies.csv` shape: extra `Audible area` column and
    /// a `VECTORPOINTS` tail.
    const BIRDIES: &[u8] = b"Min distance,Max distance,3D priority,Audible area,,,,,\n100,250,12,0,,,,,\nsample name,sample volume,sample type,oneshot time limit low,oneshot time limit high,active,min speed,max speed,Doppler\ngulls1,0.91,1,7,15,1,0,999999,0\ngulls2,0.87,1,5,15,1,0,999999,0\nVECTORPOINTS,,,,,,,,\nx,y,z,,,,,,\n-1560.94397,46.961636,-133.132431,,,,,,\n-1625.589355,48.832798,-212.562988,,,,,,\n";

    #[test]
    fn object_audio_parses_subway() {
        let f = parse("aud/cardata/ambient/subwaycar.csv", SUBWAY).unwrap();
        let CardataBody::Object(o) = &f.body else {
            panic!("expected object body");
        };
        assert_eq!(o.min_distance, 0.0);
        assert_eq!(o.max_distance, 150.0);
        assert_eq!(o.priority, 12.0);
        assert_eq!(o.audible_area, None);
        assert_eq!(o.samples.len(), 1);
        assert_eq!(o.samples[0].name, "LondonTube");
        assert_eq!(o.samples[0].doppler, 1.0);
        assert!(o.vector_points.is_empty());
        assert!(o.diagnostics.is_empty());
    }

    #[test]
    fn object_audio_parses_vectorpoints() {
        let f = parse("aud/ambient/birdies.csv", BIRDIES).unwrap();
        assert_eq!(f.kind, CardataKind::ObjectAudio);
        let CardataBody::Object(o) = &f.body else {
            panic!("expected object body");
        };
        assert_eq!(o.min_distance, 100.0);
        assert_eq!(o.audible_area, Some(0.0));
        assert_eq!(o.samples.len(), 2);
        assert_eq!(o.vector_points.len(), 2);
        assert_eq!(o.vector_points[0][0], -1_560.944);
        assert!(o.diagnostics.is_empty());
    }

    // -- SemiData / VehTypes / BandTable ---------------------------------

    #[test]
    fn semidata_parses() {
        let f = parse(
            "aud/cardata/shared/semidata.csv",
            b"reverse sample,air blow sample,reverse volume,air blow volume\nfreightreverse,freightairblow,0.87,0.9\n",
        )
        .unwrap();
        let CardataBody::Semi(s) = &f.body else {
            panic!("expected semi body");
        };
        assert_eq!(s.reverse_sample, "freightreverse");
        assert_eq!(s.air_blow_volume, 0.9);
        assert!(f.body.wave_names().contains(&"freightairblow"));
    }

    #[test]
    fn vehtypes_parses_groups() {
        let f = parse(
            "aud/cardata/shared/vehtypes.csv",
            b"Semi or bus,,,\nvpcentury,vpbus,vpddbus,ENDOFDATA\nPolice Car,,,\nvpcop,vpsemi,vpeagle,ENDOFDATA\nAlways nitro,,,\nFALSE,,,\n",
        )
        .unwrap();
        let CardataBody::VehTypes(v) = &f.body else {
            panic!("expected vehtypes body");
        };
        assert_eq!(v.groups.len(), 3);
        assert_eq!(v.groups[0].label, "Semi or bus");
        assert_eq!(
            v.groups[0].members,
            ["vpcentury", "vpbus", "vpddbus", "ENDOFDATA"]
        );
        assert_eq!(v.groups[2].members, ["FALSE"]);
    }

    #[test]
    fn band_tables_parse() {
        let f = parse(
            "aud/cardata/player/suspensionaudio.csv",
            b"Sample name,Min velocity,Max velocity,Min volume,Max volume,Volume Divisor\nSuspension3,2,3,0.85,0.9,3\n",
        )
        .unwrap();
        assert_eq!(f.kind, CardataKind::SuspensionAudio);
        let CardataBody::Bands(t) = &f.body else {
            panic!("expected band body");
        };
        assert_eq!(t.columns.len(), 6);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].name, "Suspension3");
        assert_eq!(t.rows[0].values, [2.0, 3.0, 0.85, 0.9, 3.0]);

        let w = parse(
            "aud/cardata/player/tirewobble.csv",
            b"sample name,min vol,max vol,min pitch,max pitch,pitch divisor\ntirewobble1,0.97,1,0.75,1.5,15\n",
        )
        .unwrap();
        assert_eq!(w.kind, CardataKind::TireWobble);
    }

    // -- AmbientContainer ---------------------------------------------------

    #[test]
    fn ambient_container_lists_siblings() {
        // The real `londonambientcontainer.csv` shape.
        let f = parse(
            "aud/ambient/londonambientcontainer.csv",
            b"file names\nlondonriver\ntubevoices\n",
        )
        .unwrap();
        let CardataBody::Container(c) = &f.body else {
            panic!("expected container body");
        };
        assert_eq!(c.files, ["londonriver", "tubevoices"]);
        assert!(c.diagnostics.is_empty());
    }

    // -- classification ---------------------------------------------------

    #[test]
    fn classify_covers_retail_paths() {
        assert_eq!(
            classify("aud/cardata/player/vpbug.csv"),
            Some(CardataKind::CarAudio)
        );
        assert_eq!(
            classify("aud/cardata/opponent/copy of vpford.csv"),
            Some(CardataKind::CarAudio)
        );
        assert_eq!(
            classify("aud/cardata/player/vpbullet.wrk.csv"),
            Some(CardataKind::CarAudio)
        );
        assert_eq!(
            classify("aud/cardata/engineparamsopp.csv"),
            Some(CardataKind::EngineParams)
        );
        assert_eq!(
            classify("aud/cardata/player/copy of default_impacts.csv"),
            Some(CardataKind::ImpactTable)
        );
        assert_eq!(
            classify("aud/cardata/player/default_surfacewet.csv"),
            Some(CardataKind::SurfaceTable)
        );
        assert_eq!(
            classify("aud/cardata/player/londonpolicesiren.csv"),
            Some(CardataKind::SirenProgram)
        );
        assert_eq!(
            classify("aud/cardata/ambient/va_bus_f_engine.csv"),
            Some(CardataKind::AmbientEngine)
        );
        assert_eq!(
            classify("aud/cardata/ambient/va_sedan_s_horn.csv"),
            Some(CardataKind::AmbientHorn)
        );
        assert_eq!(
            classify("aud/cardata/ambient/subwaycar.csv"),
            Some(CardataKind::ObjectAudio)
        );
        assert_eq!(
            classify("aud/ambient/drawbridge.csv"),
            Some(CardataKind::ObjectAudio)
        );
        assert_eq!(
            classify("aud/ambient/londonambientcontainer.csv"),
            Some(CardataKind::AmbientContainer)
        );
        assert_eq!(
            classify("aud/cardata/player/default_surfaceice.csv"),
            Some(CardataKind::SurfaceTable)
        );
        // Non-cardata csvs and non-csvs stay out.
        assert_eq!(classify("aud/spchdata/al1/blitz.csv"), None);
        assert_eq!(classify("aud/creaturedata/default_fpedvoice1.csv"), None);
        assert_eq!(classify("aud/cardata/opponent/renshit.bat"), None);
        assert_eq!(classify("aud/aud11/engine.wav"), None);
        assert_eq!(classify("aud/dmusic/csv_files/sfambience.csv"), None);
    }
}

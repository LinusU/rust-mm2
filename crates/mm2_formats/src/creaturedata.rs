//! Parser for the `aud/creaturedata/**` pedestrian and ambient-car
//! voice tables.
//!
//! Two plain-text schemas ship (measured on retail, 21 files):
//!
//! - **Voice tables** — `default_{f,m}pedvoice<N>.csv`,
//!   `default_ambcarvoice_{l,s}<N>.csv` and the per-vehicle
//!   `va_<vehicle>_ambcarvoice_{l,s}.csv`. A table is a sequence of
//!   blocks, each a column-header line, a numeric row, a
//!   `sample name,volume,…` header and sample rows:
//!   - `Min speed,Max speed,min time in range,max time out of range`
//!     followed by `<min>,<max>,<in>,<out>` opens a speed band;
//!   - `min impact force` followed by `<force>` opens an impact band
//!     whose sample header adds a `play after seconds` column.
//!
//!   Sample rows are `<wave stem>,<volume>[,<play after>]`; trailing
//!   empty cells are authored padding.
//!
//! - **Count files** — `num{male,female}pedvoicefiles.csv` and
//!   `numambcarvoicefiles_{l,s}.csv`: a `Num files` heading and one
//!   integer.
//!
//! Only the authored fields are read. What the speeds, times and force
//! are measured in, and how the runtime draws a sample, is unrecovered
//! from the executable and stays unknown.

use crate::racedata::TableDiagnostic;

/// One `sample name,volume[,play after seconds]` row.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSample {
    /// Wave stem as authored (case preserved).
    pub name: String,
    /// The authored volume (retail: `0.98`).
    pub volume: f32,
    /// The `play after seconds` column; only impact bands author it.
    pub play_after: Option<f32>,
    /// 1-based source line.
    pub line: u32,
}

/// A speed band: `Min speed,Max speed,min time in range,max time out
/// of range` and its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedBand {
    /// Authored `Min speed` (unit unrecovered).
    pub min_speed: f32,
    /// Authored `Max speed`; retail uses `999999` as "unbounded".
    pub max_speed: f32,
    /// Authored `min time in range` (seconds, unverified).
    pub min_time_in_range: f32,
    /// Authored `max time out of range` (seconds, unverified).
    pub max_time_out_of_range: f32,
    /// The band's samples in authored order.
    pub samples: Vec<VoiceSample>,
    /// 1-based line of the band's numeric row.
    pub line: u32,
}

/// An impact band: `min impact force` and its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactBand {
    /// Authored `min impact force` (unit unrecovered).
    pub min_force: f32,
    /// The band's samples in authored order.
    pub samples: Vec<VoiceSample>,
    /// 1-based line of the band's numeric row.
    pub line: u32,
}

/// A parsed voice table.
#[derive(Debug, Clone, Default)]
pub struct VoiceTable {
    /// Speed bands in authored order.
    pub speed_bands: Vec<SpeedBand>,
    /// Impact bands in authored order.
    pub impact_bands: Vec<ImpactBand>,
    /// Recoverable problems (malformed rows, stray lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// Which grammar a `aud/creaturedata` file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatureKind {
    /// A band/sample voice table.
    Voice,
    /// A `Num files` count file.
    Count,
}

/// Classify a logical path under `aud/creaturedata/`; `None` for
/// anything else.
pub fn classify(logical: &str) -> Option<CreatureKind> {
    let rest = logical.strip_prefix("aud/creaturedata/")?;
    if rest.contains('/') || !rest.to_ascii_lowercase().ends_with(".csv") {
        return None;
    }
    if rest.to_ascii_lowercase().starts_with("num") {
        Some(CreatureKind::Count)
    } else {
        Some(CreatureKind::Voice)
    }
}

enum Expect {
    /// Between blocks.
    Header,
    SpeedRow,
    ImpactRow,
    /// The `sample name,…` header of the current block.
    SampleHeader,
    Samples,
}

#[derive(PartialEq)]
enum Open {
    None,
    Speed,
    Impact,
}

fn cells(line: &str) -> Vec<&str> {
    line.split(',').map(str::trim).collect()
}

fn num(cell: &str) -> Option<f32> {
    cell.parse::<f32>().ok().filter(|v| v.is_finite())
}

impl VoiceTable {
    /// Parse a voice table. Never fails: problems are diagnostics.
    pub fn parse(input: &str) -> Self {
        let mut t = VoiceTable::default();
        let mut expect = Expect::Header;
        let mut open = Open::None;
        for (i, raw) in input.lines().enumerate() {
            let line = i as u32 + 1;
            let c = cells(raw);
            if c.iter().all(|s| s.is_empty()) {
                continue;
            }
            let first = c[0].to_ascii_lowercase();
            // A header line always starts a new block, whatever came
            // before it.
            if first == "min speed" {
                if c.len() < 4 {
                    t.diag(line, "speed header has fewer than 4 columns");
                }
                expect = Expect::SpeedRow;
                continue;
            }
            if first == "min impact force" {
                expect = Expect::ImpactRow;
                continue;
            }
            if first == "sample name" {
                if !matches!(expect, Expect::SampleHeader) {
                    t.diag(line, "sample header outside a band");
                }
                expect = Expect::Samples;
                continue;
            }
            match expect {
                Expect::SpeedRow => {
                    let v: Vec<Option<f32>> = c.iter().take(4).map(|s| num(s)).collect();
                    if v.len() == 4 && v.iter().all(Option::is_some) {
                        t.speed_bands.push(SpeedBand {
                            min_speed: v[0].unwrap_or_default(),
                            max_speed: v[1].unwrap_or_default(),
                            min_time_in_range: v[2].unwrap_or_default(),
                            max_time_out_of_range: v[3].unwrap_or_default(),
                            samples: Vec::new(),
                            line,
                        });
                        open = Open::Speed;
                        expect = Expect::SampleHeader;
                    } else {
                        t.diag(line, format!("malformed speed band row `{}`", raw.trim()));
                        open = Open::None;
                        expect = Expect::Header;
                    }
                }
                Expect::ImpactRow => match num(c[0]) {
                    Some(force) => {
                        t.impact_bands.push(ImpactBand {
                            min_force: force,
                            samples: Vec::new(),
                            line,
                        });
                        open = Open::Impact;
                        expect = Expect::SampleHeader;
                    }
                    None => {
                        t.diag(line, format!("malformed impact force row `{}`", raw.trim()));
                        open = Open::None;
                        expect = Expect::Header;
                    }
                },
                Expect::SampleHeader | Expect::Samples => {
                    if matches!(expect, Expect::SampleHeader) {
                        t.diag(line, "sample row before the `sample name` header");
                    }
                    t.sample(&open, &c, line);
                }
                Expect::Header => t.diag(line, format!("row outside any band `{}`", raw.trim())),
            }
        }
        t
    }

    fn diag(&mut self, line: u32, message: impl Into<String>) {
        self.diagnostics.push(TableDiagnostic {
            line,
            message: message.into(),
        });
    }

    fn sample(&mut self, open: &Open, c: &[&str], line: u32) {
        let volume = c.get(1).and_then(|s| num(s));
        let (Some(volume), false) = (volume, c[0].is_empty()) else {
            self.diag(line, "sample row needs a name and a numeric volume");
            return;
        };
        let play_after = match c.get(2).copied().filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => match num(s) {
                Some(v) => Some(v),
                None => {
                    self.diag(line, format!("non-numeric play-after `{s}`"));
                    return;
                }
            },
        };
        let s = VoiceSample {
            name: c[0].to_string(),
            volume,
            play_after,
            line,
        };
        match open {
            Open::Speed => {
                if let Some(b) = self.speed_bands.last_mut() {
                    b.samples.push(s);
                }
            }
            Open::Impact => {
                if let Some(b) = self.impact_bands.last_mut() {
                    b.samples.push(s);
                }
            }
            Open::None => self.diag(line, "sample row outside any band"),
        }
    }

    /// Every sample across all bands, authored order within a band.
    pub fn samples(&self) -> impl Iterator<Item = &VoiceSample> {
        self.speed_bands
            .iter()
            .flat_map(|b| b.samples.iter())
            .chain(self.impact_bands.iter().flat_map(|b| b.samples.iter()))
    }

    /// Structural problems on well-formed input: a band with no
    /// samples, an inverted speed range, a table with no bands.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        let mut push = |line, m: String| out.push(TableDiagnostic { line, message: m });
        if self.speed_bands.is_empty() && self.impact_bands.is_empty() {
            push(1, "table authors no bands".into());
        }
        for b in &self.speed_bands {
            if b.samples.is_empty() {
                push(b.line, "speed band has no samples".into());
            }
            if b.min_speed > b.max_speed {
                push(b.line, "min speed exceeds max speed".into());
            }
        }
        for b in &self.impact_bands {
            if b.samples.is_empty() {
                push(b.line, "impact band has no samples".into());
            }
        }
        out
    }
}

/// Parse a `Num files` count file: the heading then one non-negative
/// integer.
pub fn parse_count(input: &str) -> Result<u32, crate::FormatError> {
    let mut lines = input
        .lines()
        .map(|l| l.split(',').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty());
    match (lines.next(), lines.next(), lines.next()) {
        (Some(h), Some(v), None) if h.eq_ignore_ascii_case("num files") => v
            .parse::<u32>()
            .map_err(|_| crate::FormatError::parse(0, format!("count `{v}` is not an integer"))),
        _ => Err(crate::FormatError::parse(
            0,
            "expected a `Num files` heading and one count",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMB: &str = "Min speed,Max speed,min time in range,max time out of range\r\n\
0,20,5,0\r\nsample name,volume,,\r\nLDRIVER8,0.98,,\r\nLDRIVER7,0.98,,\r\n\
min impact force,,,\r\n5500,,,\r\nsample name,volume,play after seconds,\r\n\
LCOLLIDE2,0.98,1,\r\nLCOLLIDE4,0.98,1\r\n";

    #[test]
    fn parses_bands_and_samples() {
        let t = VoiceTable::parse(AMB);
        assert!(t.diagnostics.is_empty(), "{:?}", t.diagnostics);
        assert_eq!(t.speed_bands.len(), 1);
        assert_eq!(t.speed_bands[0].max_speed, 20.0);
        assert_eq!(t.speed_bands[0].min_time_in_range, 5.0);
        assert_eq!(t.speed_bands[0].samples.len(), 2);
        assert_eq!(t.impact_bands[0].min_force, 5500.0);
        assert_eq!(t.impact_bands[0].samples[1].play_after, Some(1.0));
        assert_eq!(t.samples().count(), 4);
        assert!(t.validate().is_empty());
    }

    #[test]
    fn malformed_rows_are_diagnostics() {
        let t = VoiceTable::parse(
            "Min speed,Max speed,min time in range,max time out of range\n0,x,1,0\nsample name,volume\nA,0.5\n",
        );
        assert!(t.speed_bands.is_empty());
        assert!(t.diagnostics.len() >= 2);
    }

    #[test]
    fn bad_volume_and_orphans_are_flagged() {
        let t = VoiceTable::parse("A,0.5\nmin impact force\n10\nsample name,volume\nB,loud\n");
        assert_eq!(t.diagnostics.len(), 2);
        assert_eq!(t.impact_bands.len(), 1);
    }

    #[test]
    fn validate_flags_empty_and_inverted() {
        let t = VoiceTable::parse(
            "Min speed,Max speed,min time in range,max time out of range\n9,1,0,0\nsample name,volume\n",
        );
        assert_eq!(t.validate().len(), 2);
        assert_eq!(VoiceTable::parse("").validate().len(), 1);
    }

    #[test]
    fn parses_count_files() {
        assert_eq!(parse_count("Num files\r\n3\r\n").unwrap(), 3);
        assert!(parse_count("Num files\n").is_err());
        assert!(parse_count("Num files\nx\n").is_err());
        assert!(parse_count("Num files\n1\n2\n").is_err());
    }

    #[test]
    fn classifies_paths() {
        assert_eq!(
            classify("aud/creaturedata/numfemalepedvoicefiles.csv"),
            Some(CreatureKind::Count)
        );
        assert_eq!(
            classify("aud/creaturedata/default_fpedvoice1.csv"),
            Some(CreatureKind::Voice)
        );
        assert_eq!(classify("aud/spchdata/al1/a.csv"), None);
        assert_eq!(classify("aud/creaturedata/x.bat"), None);
    }
}

//! Parser for `.aimap` / `.aimap_p` AI-map override files.
//!
//! Each city ships `city/<name>.aimap` with city-wide ambient settings,
//! and most `race/<city>/<stem>` events carry their own `.aimap` (plus a
//! `.aimap_p` variant — Amateur/Professional vs multiplayer role
//! unverified) overriding road speed limits, ambient density, police and
//! opponent spawns for that event.
//!
//! The grammar is INI-like and measured on all 209 retail files
//! (2026-09-20, see `docs/research/aimap.md`): `#` comment lines and
//! blank lines are ignored anywhere; `[Name]` starts a section; a
//! section body is either one scalar value line or a decimal row count
//! followed by that many whitespace-separated rows. CRLF line endings.
//! Sections may appear in any order and several are optional.
//!
//! Row shapes vary between retail files (police rows carry 8 or 5
//! numeric columns, opponent rows 10 or 1); numeric tails are preserved
//! raw rather than assigned invented names.

use crate::FormatError;
use crate::racedata::TableDiagnostic;
use std::fmt;

/// Section names observed on retail data. Matching is exact; anything
/// else is preserved in [`Aimap::unknown_sections`].
mod section {
    /// Ambient traffic density scalar.
    pub const DENSITY: &str = "Density";
    /// Default road speed limit — overrides the BAI `baseSpeed`.
    pub const SPEED_LIMIT: &str = "Speed Limit";
    /// Per-road density/speed overrides (counted).
    pub const EXCEPTIONS: &str = "Exceptions";
    /// Police spawns (counted).
    pub const POLICE: &str = "Police";
    /// Opponent spawns (counted).
    pub const OPPONENT: &str = "Opponent";
    /// Ambient vehicle roster with cumulative weights (counted).
    pub const AMBIENT_TYPES: &str = "Ambient Types/Density";
    /// Left-hand traffic flag.
    pub const DRIVE_ON_LEFT: &str = "Ambients Drive On The Left";
    /// Pedestrian model names per weather (counted).
    pub const PED_NAMES: &str = "GoodWeatherPedName / BadWeatherPedName";
    /// Police pursuit radius scalar.
    pub const COP_CHASE: &str = "CopChaseDistance";
    /// Lane-change enable flag.
    pub const LANE_CHANGES: &str = "AmbientLaneChanges";
    /// Traffic-light model names (free-form; one retail file).
    pub const TRAFFIC_LIGHTS: &str = "Traffic Lights";
    /// Hookmen spawns (counted; only a 0-count instance on retail).
    pub const HOOKMEN: &str = "Hookmen";
}

/// Hard cap on a declared section row count (retail maximum is 20).
const MAX_ROWS: u64 = 1 << 16;

/// One `[Exceptions]` row: per-road ambient overrides.
#[derive(Debug, Clone)]
pub struct RoadException {
    /// Road identifier. On retail SF these are `city/sf.bai` road ids
    /// (0-based); several London files reference ids beyond
    /// `city/london.bai`'s road space — reported by the audit, not
    /// repaired. See `docs/research/aimap.md`.
    pub road: u32,
    /// Density column (0.00 on every retail row — disables ambient
    /// traffic on the named road, semantics unverified).
    pub density: f32,
    /// Speed-limit column (0 on every retail row).
    pub speed_limit: f32,
    /// 1-based source line.
    pub line: u32,
}

/// One `[Police]` row: an authored cop spawn.
#[derive(Debug, Clone)]
pub struct PoliceRecord {
    /// Vehicle geo basename (`vpcop` on retail).
    pub geo: String,
    /// Authored spawn position.
    pub position: [f32; 3],
    /// Remaining numeric columns, preserved raw. Five values on most
    /// retail rows (first is a heading in degrees), two on
    /// `race/sf/evade0.aimap`. The file's own column comment names
    /// "StartLink, Start Dist, Start Mode, Start Lane, Patrol Route",
    /// which does not match the observed column count — undocumented.
    pub params: Vec<f32>,
    /// 1-based source line.
    pub line: u32,
}

/// One `[Opponent]` row: an authored opponent spawn.
#[derive(Debug, Clone)]
pub struct OpponentRecord {
    /// Vehicle geo basename (e.g. `vpcoop`, `vpford`).
    pub geo: String,
    /// Opponent-path reference — a `*.opp` basename on all but one
    /// retail row (`race/sf/stunt0.aimap` names `opp-c0.2`, which does
    /// not resolve in the VFS).
    pub waypoints: String,
    /// Remaining numeric columns, preserved raw: ten on retail rows
    /// (the first behaves like a 0–1 skill), one on the stunt0 row.
    /// Undocumented.
    pub params: Vec<f32>,
    /// 1-based source line.
    pub line: u32,
}

/// One `[Ambient Types/Density]` row: an ambient vehicle pick.
#[derive(Debug, Clone)]
pub struct AmbientTypeRow {
    /// Ambient vehicle geo basename (`va_*` on retail).
    pub name: String,
    /// Cumulative selection weight — non-decreasing across the list and
    /// closing at 1.0 on every retail file.
    pub weight: f32,
    /// Trailing flag column (0 on retail; absent on one roambak row).
    pub flag: i64,
    /// 1-based source line.
    pub line: u32,
}

/// One `[GoodWeatherPedName / BadWeatherPedName]` row.
#[derive(Debug, Clone)]
pub struct PedNames {
    /// Pedestrian model used in good weather.
    pub good_weather: String,
    /// Pedestrian model used in bad weather.
    pub bad_weather: String,
    /// 1-based source line.
    pub line: u32,
}

/// A section this parser does not interpret, preserved verbatim.
#[derive(Debug, Clone)]
pub struct UnknownSection {
    /// Section name as authored (inside the brackets).
    pub name: String,
    /// Non-comment, non-blank data lines.
    pub lines: Vec<String>,
}

/// A parsed `.aimap` file. Missing sections stay `None`/empty —
/// optional sections are genuinely absent on retail files, not errors.
#[derive(Debug, Clone, Default)]
pub struct Aimap {
    /// `[Density]` ambient traffic density.
    pub density: Option<f32>,
    /// `[Speed Limit]` default road speed limit.
    pub speed_limit: Option<f32>,
    /// `[Exceptions]` per-road overrides.
    pub exceptions: Vec<RoadException>,
    /// `[Police]` spawns.
    pub police: Vec<PoliceRecord>,
    /// `[Opponent]` spawns.
    pub opponents: Vec<OpponentRecord>,
    /// `[Ambient Types/Density]` roster.
    pub ambient_types: Vec<AmbientTypeRow>,
    /// `[Ambients Drive On The Left]` raw flag (0/1 on retail).
    pub drive_on_left: Option<i64>,
    /// `[GoodWeatherPedName / BadWeatherPedName]` pairs.
    pub ped_names: Vec<PedNames>,
    /// `[CopChaseDistance]` pursuit radius.
    pub cop_chase_distance: Option<f32>,
    /// `[AmbientLaneChanges]` raw flag (0/1 on retail).
    pub ambient_lane_changes: Option<i64>,
    /// `[Traffic Lights]` data lines, preserved raw — the single retail
    /// instance carries two model names on one line; shape unverified.
    pub traffic_lights: Vec<String>,
    /// `[Hookmen]` data rows, preserved raw — only a 0-count instance
    /// exists on retail, so the row shape is unknown.
    pub hookmen: Vec<String>,
    /// Uninterpreted sections, preserved verbatim.
    pub unknown_sections: Vec<UnknownSection>,
    /// Recoverable row-level problems (malformed rows, stray data).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// A consistency problem found by [`Aimap::validate`]. Parsing only
/// enforces the section grammar; value-level anomalies are reported
/// here so authored quirks still load.
#[derive(Debug, Clone, PartialEq)]
pub enum AimapIssue {
    /// Two `[Exceptions]` rows name the same road.
    DuplicateExceptionRoad(u32),
    /// An ambient weight outside [0, 1].
    AmbientWeightOutOfRange {
        /// Row index.
        index: usize,
        /// Raw weight.
        weight: f32,
    },
    /// Ambient weights decreased — they are cumulative on retail.
    NonMonotoneAmbientWeights {
        /// Row index.
        index: usize,
        /// This row's weight.
        weight: f32,
        /// Previous row's weight.
        previous: f32,
    },
    /// The last ambient weight is not 1.0 (retail always closes at 1.0).
    AmbientWeightsNotClosed {
        /// Last cumulative weight.
        last: f32,
    },
    /// A 0/1 flag section carries another value.
    FlagOutOfRange {
        /// Section name.
        section: &'static str,
        /// Raw value.
        value: i64,
    },
    /// A scalar that is a count/ratio/distance went negative.
    NegativeScalar {
        /// Section name.
        section: &'static str,
        /// Raw value.
        value: f32,
    },
    /// A section this parser does not interpret.
    UnknownSectionPresent {
        /// Section name.
        name: String,
    },
}

impl fmt::Display for AimapIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AimapIssue::DuplicateExceptionRoad(road) => {
                write!(f, "duplicate exception for road {road}")
            }
            AimapIssue::AmbientWeightOutOfRange { index, weight } => {
                write!(f, "ambient type {index}: weight {weight} outside [0,1]")
            }
            AimapIssue::NonMonotoneAmbientWeights {
                index,
                weight,
                previous,
            } => write!(
                f,
                "ambient type {index}: weight {weight} below previous {previous}"
            ),
            AimapIssue::AmbientWeightsNotClosed { last } => {
                write!(f, "ambient weights close at {last}, not 1.0")
            }
            AimapIssue::FlagOutOfRange { section, value } => {
                write!(f, "[{section}] flag value {value} is not 0/1")
            }
            AimapIssue::NegativeScalar { section, value } => {
                write!(f, "[{section}] negative value {value}")
            }
            AimapIssue::UnknownSectionPresent { name } => {
                write!(f, "uninterpreted section [{name}]")
            }
        }
    }
}

/// One raw `[Name]` + data-line group produced by the grouping pass.
struct RawSection<'a> {
    name: &'a str,
    /// `(1-based line number, line contents)` — comments and blanks
    /// already removed.
    lines: Vec<(u32, &'a str)>,
}

fn group_sections(text: &str) -> Result<Vec<RawSection<'_>>, FormatError> {
    let mut sections: Vec<RawSection<'_>> = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
                return Err(FormatError::parse(
                    idx,
                    format!("malformed section header {line:?}"),
                ));
            };
            if name.is_empty() {
                return Err(FormatError::parse(idx, "empty section name"));
            }
            sections.push(RawSection {
                name,
                lines: Vec::new(),
            });
            continue;
        }
        match sections.last_mut() {
            Some(sec) => sec.lines.push((line_no, line)),
            None => {
                return Err(FormatError::parse(
                    idx,
                    format!("data line {line:?} before any section header"),
                ));
            }
        }
    }
    Ok(sections)
}

/// Read the declared row count heading a counted section.
fn declared_count(sec: &RawSection<'_>) -> Result<u64, FormatError> {
    let Some(&(line_no, first)) = sec.lines.first() else {
        return Err(FormatError::parse(
            0,
            format!("[{}] has no row-count line", sec.name),
        ));
    };
    let count: u64 = first.parse().map_err(|_| {
        FormatError::parse(
            line_no as usize,
            format!(
                "[{}] row count {first:?} is not a non-negative integer",
                sec.name
            ),
        )
    })?;
    if count > MAX_ROWS {
        return Err(FormatError::InvalidValue {
            offset: line_no as usize,
            field: "row count",
            value: count,
            reason: "implausible .aimap section count",
        });
    }
    if sec.lines.len() - 1 != count as usize {
        return Err(FormatError::parse(
            line_no as usize,
            format!(
                "[{}] declares {count} row(s), found {}",
                sec.name,
                sec.lines.len() - 1
            ),
        ));
    }
    Ok(count)
}

/// Parse a single scalar-value section: first line is the value,
/// further lines are stray data recorded as diagnostics.
fn scalar<T: std::str::FromStr>(
    sec: &RawSection<'_>,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> Result<Option<T>, FormatError> {
    let Some(&(line_no, first)) = sec.lines.first() else {
        return Err(FormatError::parse(
            0,
            format!("[{}] has no value line", sec.name),
        ));
    };
    let value = first.parse::<T>().map_err(|_| {
        FormatError::parse(
            line_no as usize,
            format!("[{}] value {first:?} is not numeric", sec.name),
        )
    })?;
    for &(line_no, _) in &sec.lines[1..] {
        diagnostics.push(TableDiagnostic {
            line: line_no,
            message: format!("[{}] stray data after scalar value", sec.name),
        });
    }
    Ok(Some(value))
}

fn parse_num<T: std::str::FromStr>(
    tok: &str,
    line_no: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> Option<T> {
    tok.parse::<T>().ok().or_else(|| {
        diagnostics.push(TableDiagnostic {
            line: line_no,
            message: format!("non-numeric value {tok:?}"),
        });
        None
    })
}

impl Aimap {
    /// Parse a complete `.aimap`/`.aimap_p` file. Structural grammar
    /// violations (orphan data, bad counts, truncated sections) are
    /// [`FormatError`]; malformed rows inside a valid section are
    /// skipped and recorded in [`Aimap::diagnostics`].
    pub fn parse(text: &str) -> Result<Self, FormatError> {
        let mut aimap = Aimap::default();
        for sec in group_sections(text)? {
            match sec.name {
                section::DENSITY => {
                    aimap.density = scalar(&sec, &mut aimap.diagnostics)?;
                }
                section::SPEED_LIMIT => {
                    aimap.speed_limit = scalar(&sec, &mut aimap.diagnostics)?;
                }
                section::DRIVE_ON_LEFT => {
                    aimap.drive_on_left = scalar(&sec, &mut aimap.diagnostics)?;
                }
                section::COP_CHASE => {
                    aimap.cop_chase_distance = scalar(&sec, &mut aimap.diagnostics)?;
                }
                section::LANE_CHANGES => {
                    aimap.ambient_lane_changes = scalar(&sec, &mut aimap.diagnostics)?;
                }
                section::TRAFFIC_LIGHTS => {
                    aimap
                        .traffic_lights
                        .extend(sec.lines.iter().map(|&(_, l)| l.to_string()));
                }
                section::EXCEPTIONS => {
                    declared_count(&sec)?;
                    for &(line_no, line) in &sec.lines[1..] {
                        let toks: Vec<&str> = line.split_whitespace().collect();
                        if toks.len() != 3 {
                            aimap.diagnostics.push(TableDiagnostic {
                                line: line_no,
                                message: format!(
                                    "exception row: expected 3 fields, have {}",
                                    toks.len()
                                ),
                            });
                            continue;
                        }
                        let (road, density, speed) = (
                            parse_num(toks[0], line_no, &mut aimap.diagnostics),
                            parse_num(toks[1], line_no, &mut aimap.diagnostics),
                            parse_num(toks[2], line_no, &mut aimap.diagnostics),
                        );
                        if let (Some(road), Some(density), Some(speed_limit)) =
                            (road, density, speed)
                        {
                            aimap.exceptions.push(RoadException {
                                road,
                                density,
                                speed_limit,
                                line: line_no,
                            });
                        }
                    }
                }
                section::POLICE => {
                    declared_count(&sec)?;
                    for &(line_no, line) in &sec.lines[1..] {
                        let mut toks = line.split_whitespace();
                        let Some(geo) = toks.next() else { continue };
                        let nums: Vec<f32> = toks
                            .filter_map(|t| parse_num(t, line_no, &mut aimap.diagnostics))
                            .collect();
                        if nums.len() < 3 {
                            aimap.diagnostics.push(TableDiagnostic {
                                line: line_no,
                                message: format!(
                                    "police row: expected at least 3 coordinates, have {}",
                                    nums.len()
                                ),
                            });
                            continue;
                        }
                        aimap.police.push(PoliceRecord {
                            geo: geo.to_string(),
                            position: [nums[0], nums[1], nums[2]],
                            params: nums[3..].to_vec(),
                            line: line_no,
                        });
                    }
                }
                section::OPPONENT => {
                    declared_count(&sec)?;
                    for &(line_no, line) in &sec.lines[1..] {
                        let mut toks = line.split_whitespace();
                        let (Some(geo), Some(waypoints)) = (toks.next(), toks.next()) else {
                            aimap.diagnostics.push(TableDiagnostic {
                                line: line_no,
                                message: "opponent row: missing geo/waypoint fields".into(),
                            });
                            continue;
                        };
                        let params: Vec<f32> = toks
                            .filter_map(|t| parse_num(t, line_no, &mut aimap.diagnostics))
                            .collect();
                        aimap.opponents.push(OpponentRecord {
                            geo: geo.to_string(),
                            waypoints: waypoints.to_string(),
                            params,
                            line: line_no,
                        });
                    }
                }
                section::AMBIENT_TYPES => {
                    declared_count(&sec)?;
                    for &(line_no, line) in &sec.lines[1..] {
                        let toks: Vec<&str> = line.split_whitespace().collect();
                        if !(2..=3).contains(&toks.len()) {
                            aimap.diagnostics.push(TableDiagnostic {
                                line: line_no,
                                message: format!(
                                    "ambient type row: expected 2-3 fields, have {}",
                                    toks.len()
                                ),
                            });
                            continue;
                        }
                        let weight = parse_num(toks[1], line_no, &mut aimap.diagnostics);
                        let flag = match toks.get(2) {
                            Some(t) => parse_num(t, line_no, &mut aimap.diagnostics),
                            None => Some(0),
                        };
                        if let (Some(weight), Some(flag)) = (weight, flag) {
                            aimap.ambient_types.push(AmbientTypeRow {
                                name: toks[0].to_string(),
                                weight,
                                flag,
                                line: line_no,
                            });
                        }
                    }
                }
                section::PED_NAMES => {
                    declared_count(&sec)?;
                    for &(line_no, line) in &sec.lines[1..] {
                        let toks: Vec<&str> = line.split_whitespace().collect();
                        if toks.len() != 2 {
                            aimap.diagnostics.push(TableDiagnostic {
                                line: line_no,
                                message: format!(
                                    "ped-name row: expected 2 fields, have {}",
                                    toks.len()
                                ),
                            });
                            continue;
                        }
                        aimap.ped_names.push(PedNames {
                            good_weather: toks[0].to_string(),
                            bad_weather: toks[1].to_string(),
                            line: line_no,
                        });
                    }
                }
                section::HOOKMEN => {
                    declared_count(&sec)?;
                    aimap
                        .hookmen
                        .extend(sec.lines[1..].iter().map(|&(_, l)| l.to_string()));
                }
                _ => aimap.unknown_sections.push(UnknownSection {
                    name: sec.name.to_string(),
                    lines: sec.lines.iter().map(|&(_, l)| l.to_string()).collect(),
                }),
            }
        }
        Ok(aimap)
    }

    /// Check value-level integrity: exception duplicates, ambient-weight
    /// monotonicity and closure, flag ranges, negative scalars and
    /// uninterpreted sections. See [`AimapIssue`].
    pub fn validate(&self) -> Vec<AimapIssue> {
        let mut issues = Vec::new();

        let mut seen = std::collections::BTreeSet::new();
        for exc in &self.exceptions {
            if !seen.insert(exc.road) {
                issues.push(AimapIssue::DuplicateExceptionRoad(exc.road));
            }
        }

        let mut previous = 0f32;
        for (i, row) in self.ambient_types.iter().enumerate() {
            if !(0f32..=1f32).contains(&row.weight) {
                issues.push(AimapIssue::AmbientWeightOutOfRange {
                    index: i,
                    weight: row.weight,
                });
            }
            if i > 0 && row.weight < previous {
                issues.push(AimapIssue::NonMonotoneAmbientWeights {
                    index: i,
                    weight: row.weight,
                    previous,
                });
            }
            previous = row.weight;
        }
        if let Some(last) = self.ambient_types.last()
            && (last.weight - 1.0).abs() > f32::EPSILON
        {
            issues.push(AimapIssue::AmbientWeightsNotClosed { last: last.weight });
        }

        for (section, flag) in [
            (section::DRIVE_ON_LEFT, self.drive_on_left),
            (section::LANE_CHANGES, self.ambient_lane_changes),
        ] {
            if let Some(value) = flag
                && !(0..=1).contains(&value)
            {
                issues.push(AimapIssue::FlagOutOfRange { section, value });
            }
        }
        for (section, value) in [
            (section::DENSITY, self.density),
            (section::SPEED_LIMIT, self.speed_limit),
            (section::COP_CHASE, self.cop_chase_distance),
        ] {
            if let Some(value) = value
                && value < 0.0
            {
                issues.push(AimapIssue::NegativeScalar { section, value });
            }
        }
        for sec in &self.unknown_sections {
            issues.push(AimapIssue::UnknownSectionPresent {
                name: sec.name.clone(),
            });
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retail-shaped fixture covering every interpreted section.
    const FIXTURE: &str = "\
# Ambient Traffic Density\r\n\
[Density]\r\n\
.1\r\n\
\r\n\
# Default Road Speed Limit\r\n\
[Speed Limit]\r\n\
15\r\n\
# Ambient Traffic Exceptions\r\n\
# Rd Id, Density, Speed Limit\r\n\
[Exceptions]\r\n\
2\r\n\
376\t0.00\t0\r\n\
108\t0.50\t20\r\n\
# Police Init\r\n\
[Police]\r\n\
2\r\n\
vpcop\t-482.77 5.0 -885.89 125.0 0 15 0.5 50.0\r\n\
vpcop\t-9.58 5.0 -938.56 -1 0\r\n\
# Opponent Init\r\n\
[Opponent]\r\n\
2\r\n\
vpcoop race0-a-0.opp 1.00 0 50.0 0.7 0 0 0 0 0 1.0\r\n\
vpford opp-c0.2 1.00\r\n\
[Ambient Types/Density]\r\n\
3\r\n\
va_compact_s 0.07 0\r\n\
va_euro_l 0.73\r\n\
va_ddbus_l 1.0 0\r\n\
[Ambients Drive On The Left]\r\n\
1\r\n\
[GoodWeatherPedName / BadWeatherPedName]\r\n\
2\r\n\
pedmodel_man pedmodel_manw\r\n\
pedmodel_woman pedmodel_womanw\r\n\
[CopChaseDistance]\r\n\
150\r\n\
[AmbientLaneChanges]\r\n\
1\r\n\
[Traffic Lights]\r\n\
sp_traflitsingle_ped_l sp_traflitsingle_ped_l\r\n\
[Hookmen]\r\n\
0\r\n";

    #[test]
    fn parses_retail_shaped_file() {
        let a = Aimap::parse(FIXTURE).unwrap();
        assert!(a.diagnostics.is_empty());
        assert_eq!(a.density, Some(0.1));
        assert_eq!(a.speed_limit, Some(15.0));
        assert_eq!(a.exceptions.len(), 2);
        assert_eq!(a.exceptions[0].road, 376);
        assert_eq!(a.police.len(), 2);
        assert_eq!(a.police[0].params.len(), 5);
        assert_eq!(a.police[1].params.len(), 2);
        assert_eq!(a.opponents.len(), 2);
        assert_eq!(a.opponents[0].waypoints, "race0-a-0.opp");
        assert_eq!(a.opponents[0].params.len(), 10);
        assert_eq!(a.opponents[1].params.len(), 1);
        assert_eq!(a.ambient_types.len(), 3);
        assert_eq!(a.ambient_types[1].flag, 0); // 2-field row defaults
        assert_eq!(a.drive_on_left, Some(1));
        assert_eq!(a.ped_names.len(), 2);
        assert_eq!(a.cop_chase_distance, Some(150.0));
        assert_eq!(a.ambient_lane_changes, Some(1));
        assert_eq!(a.traffic_lights.len(), 1);
        assert!(a.hookmen.is_empty());
        assert!(a.unknown_sections.is_empty());
        assert!(a.validate().is_empty());
    }

    #[test]
    fn missing_sections_are_absent_not_errors() {
        let a = Aimap::parse("[Speed Limit]\n15\n").unwrap();
        assert_eq!(a.speed_limit, Some(15.0));
        assert!(a.density.is_none() && a.police.is_empty());
    }

    #[test]
    fn comments_and_blanks_are_ignored() {
        let a = Aimap::parse("# top\n\n[Density]\n# inner\n0.25\n\n").unwrap();
        assert_eq!(a.density, Some(0.25));
        assert!(a.diagnostics.is_empty());
    }

    #[test]
    fn data_before_first_section_is_an_error() {
        assert!(Aimap::parse("15\n[Speed Limit]\n15\n").is_err());
    }

    #[test]
    fn count_mismatch_is_an_error() {
        // Declared more rows than present.
        assert!(Aimap::parse("[Exceptions]\n2\n1 0.0 0\n").is_err());
        // Declared fewer rows than present.
        assert!(Aimap::parse("[Exceptions]\n1\n1 0.0 0\n2 0.0 0\n").is_err());
        // Non-numeric count.
        assert!(Aimap::parse("[Exceptions]\nabc\n").is_err());
        // Missing count entirely.
        assert!(Aimap::parse("[Exceptions]\n").is_err());
        // Implausible count is capped before allocation.
        assert!(matches!(
            Aimap::parse("[Exceptions]\n999999999999\n"),
            Err(FormatError::InvalidValue { .. }) | Err(FormatError::Parse { .. })
        ));
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = "[Exceptions]\n3\n1 0.0 0\nbad row\n2 0.0\n";
        let a = Aimap::parse(text).unwrap();
        assert_eq!(a.exceptions.len(), 1);
        assert_eq!(a.diagnostics.len(), 2);
    }

    #[test]
    fn unknown_sections_are_preserved_and_flagged() {
        let text = "[Speed Limit]\n15\n[SomeNew]\n2\nfoo bar\nbaz qux\n";
        let a = Aimap::parse(text).unwrap();
        assert_eq!(a.unknown_sections.len(), 1);
        assert_eq!(a.unknown_sections[0].lines.len(), 3); // count line preserved too
        let issues = a.validate();
        assert!(issues.iter().any(|i| matches!(
            i,
            AimapIssue::UnknownSectionPresent { name } if name == "SomeNew"
        )));
    }

    #[test]
    fn validate_reports_value_anomalies() {
        let text = "\
[Exceptions]\n2\n5 0.0 0\n5 0.0 0\n\
[Ambient Types/Density]\n3\nva_a 0.9 0\nva_b 0.5 0\nva_c 0.8 0\n\
[Ambients Drive On The Left]\n3\n\
[Density]\n-0.5\n";
        let a = Aimap::parse(text).unwrap();
        let issues = a.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, AimapIssue::DuplicateExceptionRoad(5)))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, AimapIssue::NonMonotoneAmbientWeights { .. }))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, AimapIssue::AmbientWeightsNotClosed { .. }))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, AimapIssue::FlagOutOfRange { value: 3, .. }))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, AimapIssue::NegativeScalar { .. }))
        );
    }

    #[test]
    fn malformed_section_header_is_an_error() {
        assert!(Aimap::parse("[Speed Limit\n15\n").is_err());
    }
}

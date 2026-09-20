//! Parsers for the roadside-prop rule tables: `propdefs.csv`,
//! `proprules.csv`, `props.csv` and the `geometry/props.csv` LOD table.
//!
//! These tables drive the per-room roadside dressing referenced by the
//! PSDL `prop_rule` byte: each room carries a rule number `N`, and the
//! `n{NN}left`/`n{NN}right` rows of `proprules.csv` list which
//! `propdefs.csv` prototypes may be stamped along that side of the
//! room's perimeter. A `propdefs.csv` row names the PKG variants and
//! carries the authored spacing fields. `props.csv` is a group→prop
//! membership list (retail uses a `Races` group); `geometry/props.csv`
//! is a different table entirely — per-PKG LOD triangle counts.
//!
//! Measured on the retail install (2026-09-20): headers are
//! `name,start,distance,maxUse,minLerp,maxLerp,file1..file4` and
//! `rulename,prop1..prop8` (trailing empty columns are common), rows
//! carry no quoting/escaping, `props.csv` is `Group,Name` except the
//! `city/phys/` dev copy which mislabels it `name,start` while still
//! writing `group,prop` rows. Field *semantics* (start offset along the
//! perimeter, spacing, use cap, lerp range) are inferred from the
//! values and the rule structure — see `docs/research/proprules.md`.

use std::fmt;

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// One `propdefs.csv` row: a named prop prototype.
#[derive(Debug, Clone)]
pub struct PropDef {
    /// Prototype name referenced by `proprules.csv`.
    pub name: String,
    /// `start` column — inferred perimeter offset before this prop may
    /// be placed (metres; 1-20 on retail).
    pub start: f32,
    /// `distance` column — inferred spacing between successive
    /// placements (metres; 1-90 on retail).
    pub distance: f32,
    /// `maxUse` column — inferred per-room placement cap (`9999` on
    /// retail means effectively unlimited).
    pub max_use: i64,
    /// `minLerp` column — semantics unverified (0.1-0.5 on retail).
    pub lerp_min: f32,
    /// `maxLerp` column — always equals `minLerp` on retail.
    pub lerp_max: f32,
    /// PKG basenames from the `file1`-`file4` columns (empty cells
    /// dropped). Choosing among several is inferred variant selection.
    pub files: Vec<String>,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `propdefs.csv` table.
#[derive(Debug, Clone)]
pub struct PropDefs {
    /// One entry per row, in authored order.
    pub defs: Vec<PropDef>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// Which side of a room a `proprules.csv` rule dresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropRuleSide {
    /// `n{NN}left` row.
    Left,
    /// `n{NN}right` row.
    Right,
}

impl fmt::Display for PropRuleSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropRuleSide::Left => write!(f, "left"),
            PropRuleSide::Right => write!(f, "right"),
        }
    }
}

/// One `proprules.csv` row: a rule name plus the propdef names it may
/// stamp, in authored priority order.
#[derive(Debug, Clone)]
pub struct PropRule {
    /// Rule name — `n{NN}left`/`n{NN}right` on every retail row.
    pub name: String,
    /// `propdefs.csv` names (empty cells dropped).
    pub props: Vec<String>,
    /// 1-based line number in the source file.
    pub line: u32,
}

impl PropRule {
    /// Split `n{NN}{side}` into the PSDL `prop_rule` byte value and the
    /// side. Returns `None` for names outside that convention.
    pub fn rule_key(&self) -> Option<(u8, PropRuleSide)> {
        let body = self.name.strip_prefix('n')?;
        let (num, side) = match body.strip_suffix("left") {
            Some(num) => (num, PropRuleSide::Left),
            None => (body.strip_suffix("right")?, PropRuleSide::Right),
        };
        if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        num.parse::<u8>().ok().map(|n| (n, side))
    }
}

/// A parsed `proprules.csv` table.
#[derive(Debug, Clone)]
pub struct PropRules {
    /// One entry per row, in authored order.
    pub rules: Vec<PropRule>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One `props.csv` row: a prop's membership in a named group.
#[derive(Debug, Clone)]
pub struct PropGroupEntry {
    /// Group label — only `Races` on the retail city tables (`group`
    /// on the `city/phys/` dev copy).
    pub group: String,
    /// Prop PKG basename.
    pub name: String,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `props.csv` group table.
#[derive(Debug, Clone)]
pub struct PropGroups {
    /// The header's first two labels (`Group`,`Name` on retail city
    /// tables; `name`,`start` on the `city/phys/` dev copy).
    pub header: [String; 2],
    /// One entry per row, in authored order.
    pub entries: Vec<PropGroupEntry>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One `geometry/props.csv` row: authored triangle counts per LOD for
/// a PKG. A different table than `city/**/props.csv` despite the name.
#[derive(Debug, Clone)]
pub struct PropLodStat {
    /// PKG filename including extension (e.g. `va_bus_f.pkg`).
    pub name: String,
    /// `H Tris`, `M Tris`, `L Tris`, `VL Tris` authored counts. The
    /// retail rows are not ordered by magnitude (e.g. 68/178/78/12) —
    /// column meaning is preserved, not reinterpreted.
    pub tris: [i64; 4],
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `geometry/props.csv` LOD table.
#[derive(Debug, Clone)]
pub struct PropLodStats {
    /// One entry per row, in authored order.
    pub stats: Vec<PropLodStat>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// A consistency problem in a parsed prop table.
#[derive(Debug, Clone, PartialEq)]
pub enum PropRuleIssue {
    /// Two `propdefs.csv` rows share a prototype name.
    DuplicateDef {
        /// Shared name.
        name: String,
        /// Line of the later row.
        line: u32,
    },
    /// A `propdefs.csv` row names no PKG files.
    DefWithoutFiles {
        /// Prototype name.
        name: String,
        /// Line number.
        line: u32,
    },
    /// A `propdefs.csv` row carries a nonpositive `distance`.
    NonPositiveDistance {
        /// Prototype name.
        name: String,
        /// Line number.
        line: u32,
        /// Raw distance value.
        distance: f32,
    },
    /// A `propdefs.csv` row's `start` is negative.
    NegativeStart {
        /// Prototype name.
        name: String,
        /// Line number.
        line: u32,
        /// Raw start value.
        start: f32,
    },
    /// A `propdefs.csv` row's `maxUse` is not positive.
    NonPositiveMaxUse {
        /// Prototype name.
        name: String,
        /// Line number.
        line: u32,
        /// Raw maxUse value.
        max_use: i64,
    },
    /// A `propdefs.csv` row's lerp range is inverted.
    InvertedLerpRange {
        /// Prototype name.
        name: String,
        /// Line number.
        line: u32,
    },
    /// Two `proprules.csv` rows share a rule name.
    DuplicateRule {
        /// Shared name.
        name: String,
        /// Line of the later row.
        line: u32,
    },
    /// A `proprules.csv` rule name does not fit `n{NN}{left,right}`.
    BadRuleName {
        /// Raw name.
        name: String,
        /// Line number.
        line: u32,
    },
    /// A `proprules.csv` rule lists no props.
    EmptyRule {
        /// Rule name.
        name: String,
        /// Line number.
        line: u32,
    },
    /// A `proprules.csv` rule repeats a propdef name.
    DuplicateRuleProp {
        /// Rule name.
        rule: String,
        /// Repeated propdef name.
        prop: String,
        /// Line number.
        line: u32,
    },
    /// Two `props.csv` rows repeat a (group, name) pair.
    DuplicateGroupEntry {
        /// Group label.
        group: String,
        /// Prop name.
        name: String,
        /// Line number.
        line: u32,
    },
    /// Two `geometry/props.csv` rows name the same PKG.
    DuplicateLodStat {
        /// PKG filename.
        name: String,
        /// Line number.
        line: u32,
    },
}

impl fmt::Display for PropRuleIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropRuleIssue::DuplicateDef { name, line } => {
                write!(f, "line {line}: duplicate propdef {name:?}")
            }
            PropRuleIssue::DefWithoutFiles { name, line } => {
                write!(f, "line {line}: propdef {name:?} names no PKG files")
            }
            PropRuleIssue::NonPositiveDistance {
                name,
                line,
                distance,
            } => write!(f, "line {line}: propdef {name:?} distance {distance} <= 0"),
            PropRuleIssue::NegativeStart { name, line, start } => {
                write!(f, "line {line}: propdef {name:?} start {start} < 0")
            }
            PropRuleIssue::NonPositiveMaxUse {
                name,
                line,
                max_use,
            } => write!(f, "line {line}: propdef {name:?} maxUse {max_use} <= 0"),
            PropRuleIssue::InvertedLerpRange { name, line } => {
                write!(f, "line {line}: propdef {name:?} minLerp > maxLerp")
            }
            PropRuleIssue::DuplicateRule { name, line } => {
                write!(f, "line {line}: duplicate rule {name:?}")
            }
            PropRuleIssue::BadRuleName { name, line } => {
                write!(
                    f,
                    "line {line}: rule name {name:?} is not nNN{{left,right}}"
                )
            }
            PropRuleIssue::EmptyRule { name, line } => {
                write!(f, "line {line}: rule {name:?} lists no props")
            }
            PropRuleIssue::DuplicateRuleProp { rule, prop, line } => {
                write!(f, "line {line}: rule {rule:?} repeats prop {prop:?}")
            }
            PropRuleIssue::DuplicateGroupEntry { group, name, line } => {
                write!(
                    f,
                    "line {line}: duplicate ({group:?}, {name:?}) group entry"
                )
            }
            PropRuleIssue::DuplicateLodStat { name, line } => {
                write!(f, "line {line}: duplicate LOD row for {name:?}")
            }
        }
    }
}

/// Split a CSV line into trimmed cells.
fn cells(line: &str) -> Vec<&str> {
    line.split(',').map(str::trim).collect()
}

/// First non-blank line of `input`, as trimmed cells.
fn header_cells(input: &str) -> Option<Vec<&str>> {
    input
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(cells)
}

fn parse_f32(cell: &str, what: &str, line: u32, diag: &mut Vec<TableDiagnostic>) -> Option<f32> {
    match cell.parse() {
        Ok(v) => Some(v),
        Err(_) => {
            diag.push(TableDiagnostic {
                line,
                message: format!("non-numeric {what} value {cell:?}"),
            });
            None
        }
    }
}

fn parse_i64(cell: &str, what: &str, line: u32, diag: &mut Vec<TableDiagnostic>) -> Option<i64> {
    match cell.parse() {
        Ok(v) => Some(v),
        Err(_) => {
            diag.push(TableDiagnostic {
                line,
                message: format!("non-numeric {what} value {cell:?}"),
            });
            None
        }
    }
}

impl PropDefs {
    /// Parse a `propdefs.csv` table. Returns `Err` when the header row
    /// is missing or its fixed columns do not match; malformed rows are
    /// skipped and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        const FIXED: [&str; 6] = ["name", "start", "distance", "maxUse", "minLerp", "maxLerp"];
        let Some(cols) = header_cells(input) else {
            return Err(FormatError::parse(0, "empty propdefs table"));
        };
        if cols.len() < FIXED.len() || cols[..FIXED.len()] != FIXED {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected propdefs header: expected first {} columns ({}), have {} ({})",
                    FIXED.len(),
                    FIXED.join(","),
                    cols.len(),
                    cols.join(","),
                ),
            ));
        }

        let mut defs = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_header = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true;
                continue;
            }
            let cells = cells(trimmed);
            if cells.len() < FIXED.len() + 1 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected at least {} fields, have {}",
                        FIXED.len() + 1,
                        cells.len()
                    ),
                });
                continue;
            }
            let mut d = Vec::new();
            let start = parse_f32(cells[1], "start", line, &mut d);
            let distance = parse_f32(cells[2], "distance", line, &mut d);
            let max_use = parse_i64(cells[3], "maxUse", line, &mut d);
            let lerp_min = parse_f32(cells[4], "minLerp", line, &mut d);
            let lerp_max = parse_f32(cells[5], "maxLerp", line, &mut d);
            diagnostics.append(&mut d);
            let (Some(start), Some(distance), Some(max_use), Some(lerp_min), Some(lerp_max)) =
                (start, distance, max_use, lerp_min, lerp_max)
            else {
                continue;
            };
            let files: Vec<String> = cells[6..]
                .iter()
                .filter(|c| !c.is_empty())
                .map(|c| c.to_string())
                .collect();
            defs.push(PropDef {
                name: cells[0].to_string(),
                start,
                distance,
                max_use,
                lerp_min,
                lerp_max,
                files,
                line,
            });
        }
        Ok(PropDefs { defs, diagnostics })
    }

    /// Internal consistency checks (cross-references to `proprules.csv`
    /// and the VFS belong to the audit layer).
    pub fn validate(&self) -> Vec<PropRuleIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for def in &self.defs {
            if !seen.insert(def.name.as_str()) {
                issues.push(PropRuleIssue::DuplicateDef {
                    name: def.name.clone(),
                    line: def.line,
                });
            }
            if def.files.is_empty() {
                issues.push(PropRuleIssue::DefWithoutFiles {
                    name: def.name.clone(),
                    line: def.line,
                });
            }
            if def.distance <= 0.0 {
                issues.push(PropRuleIssue::NonPositiveDistance {
                    name: def.name.clone(),
                    line: def.line,
                    distance: def.distance,
                });
            }
            if def.start < 0.0 {
                issues.push(PropRuleIssue::NegativeStart {
                    name: def.name.clone(),
                    line: def.line,
                    start: def.start,
                });
            }
            if def.max_use <= 0 {
                issues.push(PropRuleIssue::NonPositiveMaxUse {
                    name: def.name.clone(),
                    line: def.line,
                    max_use: def.max_use,
                });
            }
            if def.lerp_min > def.lerp_max {
                issues.push(PropRuleIssue::InvertedLerpRange {
                    name: def.name.clone(),
                    line: def.line,
                });
            }
        }
        issues
    }
}

impl PropRules {
    /// Parse a `proprules.csv` table. Returns `Err` when the header row
    /// is missing or does not start `rulename`; malformed rows are
    /// skipped and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let Some(cols) = header_cells(input) else {
            return Err(FormatError::parse(0, "empty proprules table"));
        };
        if cols.first() != Some(&"rulename") {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected proprules header: expected first column rulename, have ({})",
                    cols.join(","),
                ),
            ));
        }

        let mut rules = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_header = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true;
                continue;
            }
            let cells = cells(trimmed);
            if cells[0].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: "skipping row: empty rule name".into(),
                });
                continue;
            }
            rules.push(PropRule {
                name: cells[0].to_string(),
                props: cells[1..]
                    .iter()
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string())
                    .collect(),
                line,
            });
        }
        Ok(PropRules { rules, diagnostics })
    }

    /// Internal consistency checks (propdef-name and PSDL `prop_rule`
    /// cross-references belong to the audit layer).
    pub fn validate(&self) -> Vec<PropRuleIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for rule in &self.rules {
            if !seen.insert(rule.name.as_str()) {
                issues.push(PropRuleIssue::DuplicateRule {
                    name: rule.name.clone(),
                    line: rule.line,
                });
            }
            if rule.rule_key().is_none() {
                issues.push(PropRuleIssue::BadRuleName {
                    name: rule.name.clone(),
                    line: rule.line,
                });
            }
            if rule.props.is_empty() {
                issues.push(PropRuleIssue::EmptyRule {
                    name: rule.name.clone(),
                    line: rule.line,
                });
            }
            let mut props = std::collections::BTreeSet::new();
            for prop in &rule.props {
                if !props.insert(prop.as_str()) {
                    issues.push(PropRuleIssue::DuplicateRuleProp {
                        rule: rule.name.clone(),
                        prop: prop.clone(),
                        line: rule.line,
                    });
                }
            }
        }
        issues
    }
}

impl PropGroups {
    /// Parse a `props.csv` group table. The header is tolerated at any
    /// labelling — the `city/phys/` dev copy writes `name,start` over
    /// `group,prop` rows — but must have at least two columns; the first
    /// two labels are preserved on `header`. Malformed rows are skipped
    /// and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let Some(cols) = header_cells(input) else {
            return Err(FormatError::parse(0, "empty props group table"));
        };
        if cols.len() < 2 {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected props header: expected 2 columns, have ({})",
                    cols.join(","),
                ),
            ));
        }
        let header = [cols[0].to_string(), cols[1].to_string()];

        let mut entries = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_header = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true;
                continue;
            }
            let cells = cells(trimmed);
            if cells.len() < 2 || cells[0].is_empty() || cells[1].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected 2 non-empty fields, have {}",
                        cells.len()
                    ),
                });
                continue;
            }
            if cells[2..].iter().any(|c| !c.is_empty()) {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: "ignoring extra non-empty cells beyond name".into(),
                });
            }
            entries.push(PropGroupEntry {
                group: cells[0].to_string(),
                name: cells[1].to_string(),
                line,
            });
        }
        Ok(PropGroups {
            header,
            entries,
            diagnostics,
        })
    }

    /// Internal consistency checks.
    pub fn validate(&self) -> Vec<PropRuleIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entries {
            if !seen.insert((e.group.as_str(), e.name.as_str())) {
                issues.push(PropRuleIssue::DuplicateGroupEntry {
                    group: e.group.clone(),
                    name: e.name.clone(),
                    line: e.line,
                });
            }
        }
        issues
    }
}

impl PropLodStats {
    /// Parse the `geometry/props.csv` LOD table (`Name,H Tris,M Tris,L
    /// Tris,VL Tris`). Returns `Err` when the header row is missing or
    /// has fewer than five columns; malformed rows are skipped and
    /// recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let Some(cols) = header_cells(input) else {
            return Err(FormatError::parse(0, "empty prop LOD table"));
        };
        if cols.len() < 5 || cols[0] != "Name" {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected prop LOD header: expected Name + 4 columns, have ({})",
                    cols.join(","),
                ),
            ));
        }
        let labels: [String; 4] = [
            cols[1].to_string(),
            cols[2].to_string(),
            cols[3].to_string(),
            cols[4].to_string(),
        ];

        let mut stats = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_header = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true;
                continue;
            }
            let cells = cells(trimmed);
            if cells.len() < 5 || cells[0].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: expected 5 fields, have {}", cells.len()),
                });
                continue;
            }
            let mut d = Vec::new();
            let mut tris = [0i64; 4];
            let mut ok = true;
            for (i, label) in labels.iter().enumerate() {
                match parse_i64(cells[i + 1], label, line, &mut d) {
                    Some(v) => tris[i] = v,
                    None => ok = false,
                }
            }
            diagnostics.append(&mut d);
            if !ok {
                continue;
            }
            stats.push(PropLodStat {
                name: cells[0].to_string(),
                tris,
                line,
            });
        }
        Ok(PropLodStats { stats, diagnostics })
    }

    /// Internal consistency checks.
    pub fn validate(&self) -> Vec<PropRuleIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.stats {
            if !seen.insert(s.name.as_str()) {
                issues.push(PropRuleIssue::DuplicateLodStat {
                    name: s.name.clone(),
                    line: s.line,
                });
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFS_HEADER: &str =
        "name,start,distance,maxUse,minLerp,maxLerp,file1,file2,file3,file4,,,,,,,";
    const RULES_HEADER: &str = "rulename,prop1,prop2,prop3,prop4,prop5,prop6,prop7,prop8,,,,";

    #[test]
    fn parses_retail_shaped_propdefs() {
        let text = format!(
            "{DEFS_HEADER}\r\nstreetlight,9,29,9999,0.1,0.1,sp_lightstreet_l\nstreetree,14,50,2,0.1,0.1,sp_tree1_s,sp_tree1_s\n"
        );
        let defs = PropDefs::parse(&text).unwrap();
        assert!(defs.diagnostics.is_empty());
        assert_eq!(defs.defs.len(), 2);
        let d = &defs.defs[0];
        assert_eq!(d.name, "streetlight");
        assert_eq!(d.start, 9.0);
        assert_eq!(d.distance, 29.0);
        assert_eq!(d.max_use, 9999);
        assert_eq!(d.files, ["sp_lightstreet_l"]);
        assert_eq!(defs.defs[1].files.len(), 2);
        assert!(defs.validate().is_empty());
    }

    #[test]
    fn rejects_wrong_propdefs_header() {
        assert!(PropDefs::parse("a,b\n1,2\n").is_err());
        assert!(PropDefs::parse("").is_err());
    }

    #[test]
    fn propdefs_diagnostics_and_issues() {
        let text = format!(
            "{DEFS_HEADER}\nshort,1,2\ndup,0,10,1,0.1,0.1,f1\ndup,-1,0,0,0.5,0.1,f2\nnofile,3,10,5,0.1,0.1,\n"
        );
        let defs = PropDefs::parse(&text).unwrap();
        assert_eq!(defs.defs.len(), 3);
        assert_eq!(defs.diagnostics.len(), 1); // short row
        let issues = defs.validate();
        assert!(issues.contains(&PropRuleIssue::DuplicateDef {
            name: "dup".into(),
            line: 4
        }));
        assert!(issues.contains(&PropRuleIssue::NegativeStart {
            name: "dup".into(),
            line: 4,
            start: -1.0
        }));
        assert!(issues.contains(&PropRuleIssue::NonPositiveDistance {
            name: "dup".into(),
            line: 4,
            distance: 0.0
        }));
        assert!(issues.contains(&PropRuleIssue::NonPositiveMaxUse {
            name: "dup".into(),
            line: 4,
            max_use: 0
        }));
        assert!(issues.contains(&PropRuleIssue::InvertedLerpRange {
            name: "dup".into(),
            line: 4
        }));
        assert!(issues.contains(&PropRuleIssue::DefWithoutFiles {
            name: "nofile".into(),
            line: 5
        }));
    }

    #[test]
    fn parses_retail_shaped_proprules() {
        let text = format!(
            "{RULES_HEADER}\nn01left,streetlight02,streetree,trashcan\nn01right,streetlight02b,streetree,bench\n\nn02left,streetlight02,trashcan,phone,,,,\n"
        );
        let rules = PropRules::parse(&text).unwrap();
        assert!(rules.diagnostics.is_empty());
        assert_eq!(rules.rules.len(), 3);
        assert_eq!(rules.rules[0].rule_key(), Some((1, PropRuleSide::Left)));
        assert_eq!(rules.rules[1].rule_key(), Some((1, PropRuleSide::Right)));
        assert_eq!(rules.rules[2].props.len(), 3);
        assert!(rules.validate().is_empty());
    }

    #[test]
    fn rule_key_rejects_non_conforming_names() {
        for name in ["nleft", "n0xleft", "n300left", "n01", "left", "n1mid", ""] {
            let r = PropRule {
                name: name.into(),
                props: vec![],
                line: 1,
            };
            assert_eq!(r.rule_key(), None, "{name}");
        }
        let r = PropRule {
            name: "n16right".into(),
            props: vec![],
            line: 1,
        };
        assert_eq!(r.rule_key(), Some((16, PropRuleSide::Right)));
    }

    #[test]
    fn proprules_diagnostics_and_issues() {
        let text = format!("{RULES_HEADER}\nn01left,a,b\nn01left,a\nbadname,a\nempty,\ndup,a,a\n");
        let rules = PropRules::parse(&text).unwrap();
        assert_eq!(rules.rules.len(), 5);
        let issues = rules.validate();
        assert!(issues.contains(&PropRuleIssue::DuplicateRule {
            name: "n01left".into(),
            line: 3
        }));
        assert!(issues.contains(&PropRuleIssue::BadRuleName {
            name: "badname".into(),
            line: 4
        }));
        assert!(issues.contains(&PropRuleIssue::EmptyRule {
            name: "empty".into(),
            line: 5
        }));
        assert!(issues.contains(&PropRuleIssue::DuplicateRuleProp {
            rule: "dup".into(),
            prop: "a".into(),
            line: 6
        }));
    }

    #[test]
    fn prop_groups_tolerates_mislabeled_dev_header() {
        // city/phys/props.csv writes `name,start` over group,prop rows.
        let text = "name,start\ngroup,sp_benchwood_f\ngroup,sp_cone_f\nRaces,sp_cone_f\n";
        let groups = PropGroups::parse(text).unwrap();
        assert_eq!(groups.header, ["name", "start"]);
        assert_eq!(groups.entries.len(), 3);
        let issues = groups.validate();
        assert!(issues.is_empty()); // different groups, same name — not a dup
    }

    #[test]
    fn prop_groups_flags_duplicates_and_bad_rows() {
        let text = "Group,Name\nRaces,sp_a\nRaces,sp_a\nshort\nRaces,sp_b,extra\n";
        let groups = PropGroups::parse(text).unwrap();
        assert_eq!(groups.entries.len(), 3);
        assert_eq!(groups.diagnostics.len(), 2); // short row + extra cells
        let issues = groups.validate();
        assert_eq!(
            issues,
            vec![PropRuleIssue::DuplicateGroupEntry {
                group: "Races".into(),
                name: "sp_a".into(),
                line: 3
            }]
        );
    }

    #[test]
    fn prop_lod_stats_parse() {
        let text = "Name,H Tris,M Tris,L Tris,VL Tris\nva_bus_f.pkg,68,178,78,12\nva_taxi_f.pkg,256,144,62,10\nva_taxi_f.pkg,1,2,3,4\nbad,1,x,3,4\n";
        let stats = PropLodStats::parse(text).unwrap();
        assert_eq!(stats.stats.len(), 3);
        assert_eq!(stats.diagnostics.len(), 1); // non-numeric
        assert_eq!(stats.stats[0].tris, [68, 178, 78, 12]);
        let issues = stats.validate();
        assert_eq!(
            issues,
            vec![PropRuleIssue::DuplicateLodStat {
                name: "va_taxi_f.pkg".into(),
                line: 4
            }]
        );
        assert!(PropLodStats::parse("x\n").is_err());
    }
}

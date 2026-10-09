//! Parser for the `aud/spchdata/**` commentary cue tables.
//!
//! Two schemas ship under that tree (measured on retail 2026-09-27):
//!
//! - **Cue tables** — one per speaker directory (`al1`..`al6`,
//!   `as1`/`as2`/`as4`/`as5`) per cue family plus the per-city Cops &
//!   Robbers tables (`CNRSF.csv`/`CNRLONDON.csv`/`bullshit.csv`) and
//!   the crash-course lesson tables (`ccl/`/`ccs/`). The authored
//!   column header reads
//!   `Name prefix/type header,end sufix value,sufix add value`; rows
//!   then alternate `<NAME> header,,` section markers with
//!   `<prefix>,<end>,<add>[,extra…]` cue rows. A wave reference is
//!   `<prefix><NN>` — `NN` drawn inside `1..=end` (the "end sufix
//!   value"; `add` is an authored additive offset, unverified —
//!   retail uses it only on dead `RACELAPS01` rows and the C&R
//!   range rows). C&R rows carry a fourth numeric column; it is
//!   preserved verbatim in [`CueRow::extra`].
//! - **Announcer registries** — `aud/spchdata/<city>.csv` (`sf.csv`,
//!   `london.csv`): `Num announcers` + `prefix` pairs naming the
//!   speaker-directory count (`aud\spchdata\as%d`/`al%d` in the exe)
//!   and the directory/wave stem prefix (`AS`/`AL`).
//!
//! One file fits neither schema: `ccl/cc_cpoint_indexinfo.csv` is a
//! bare signed-integer list under a `SUFIX NUM FOR CPOINTS` heading.
//! Parsed as a cue table it yields diagnostics and no sections —
//! honest, not silently skipped.
//!
//! All semantics beyond the grammar (cue trigger, speaker-index draw,
//! suffix-draw shape) are unrecovered — this module only reads the
//! authored fields.

use crate::racedata::TableDiagnostic;

/// A parsed `*_prerace`/mode/city cue table.
#[derive(Debug, Clone)]
pub struct CueTable {
    /// The `header` sections in authored order.
    pub sections: Vec<CueSection>,
    /// Recoverable problems (malformed rows, stray lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One `<NAME> header,,` section and its cue rows.
#[derive(Debug, Clone)]
pub struct CueSection {
    /// The section name as authored (`WEATHER`, `PRERACE`, …),
    /// whitespace-trimmed.
    pub name: String,
    /// The section's cue rows in authored order.
    pub rows: Vec<CueRow>,
}

/// One `prefix,end,add[,extra…]` cue row.
#[derive(Debug, Clone)]
pub struct CueRow {
    /// The wave-name prefix as authored — `WEARAIN`, `PREBEETLE`,
    /// or a speaker-qualified C&R stem like `AL1\AL1ROBROB`.
    pub prefix: String,
    /// The "end sufix value" — the top of the suffix draw range.
    pub end: i64,
    /// The "sufix add value" — an authored additive offset on the
    /// drawn suffix (semantics unverified; zero on every live retail
    /// row except the C&R tables and the dead `RACELAPS01` rows).
    pub add: i64,
    /// Trailing numeric columns beyond the documented triple — the
    /// C&R tables author a fourth (`prefix,last,first,?`-shaped;
    /// semantics unverified). Preserved verbatim.
    pub extra: Vec<i64>,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A city's `aud/spchdata/<city>.csv` announcer registry: how many
/// speaker directories the tree ships (`aud\spchdata\<prefix>%d` in
/// the exe) and the stem prefix they share.
#[derive(Debug, Clone)]
pub struct AnnouncerIndex {
    /// The authored `Num announcers` count — the speaker-index draw
    /// domain (1-based: `as1`..`as5` for `sf`'s authored 5 — `as3`
    /// ships no tables or waves on retail, an authored gap the count
    /// still covers).
    pub announcers: u32,
    /// The authored `prefix` — `AS`/`AL` on retail; speaker dir and
    /// wave stems prepend it (`as1`, `as1wearain02`).
    pub prefix: String,
    /// Recoverable problems (missing/duplicated fields, stray lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

impl CueRow {
    /// The semantic problems that make this row unusable: an empty
    /// prefix, a non-positive `end`, a negative `add`, or an `add` at
    /// or past `end` (the draw lands inside `add + 1 ..= end`, so
    /// nothing is left to name). Empty means the row is usable; a
    /// consumer skips a row with any problem rather than coercing it.
    pub fn problems(&self) -> Vec<TableDiagnostic> {
        let row = self;
        let mut out = Vec::new();
        if row.prefix.is_empty() {
            out.push(TableDiagnostic {
                line: row.line,
                message: "cue row with an empty prefix".into(),
            });
        }
        if row.end <= 0 {
            out.push(TableDiagnostic {
                line: row.line,
                message: format!(
                    "cue row {} has a non-positive end sufix value {}",
                    row.prefix, row.end
                ),
            });
        }
        if row.add < 0 {
            out.push(TableDiagnostic {
                line: row.line,
                message: format!(
                    "cue row {} has a negative sufix add value {}",
                    row.prefix, row.add
                ),
            });
        }
        if row.end > 0 && row.add >= row.end {
            out.push(TableDiagnostic {
                line: row.line,
                message: format!(
                    "cue row {} names no wave: sufix add value {} is not below end sufix value {}",
                    row.prefix, row.add, row.end
                ),
            });
        }
        out
    }
}

impl CueTable {
    /// Parse a cue table. `Err` only on empty input; every malformed
    /// line degrades to a [`TableDiagnostic`] and the file's remaining
    /// rows still parse (the inventory keeps its denominator).
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(crate::FormatError::parse(0, "empty cue table"));
        };
        let mut diagnostics = Vec::new();
        let mut sections: Vec<CueSection> = Vec::new();
        // The authored column header — always `name prefix/…` on
        // retail cue tables. Anything else is still processed as a
        // data line rather than sinking the parse.
        if !header
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("name prefix/type header")
            && !header
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("name prefix")
        {
            diagnostics.push(TableDiagnostic {
                line: 1,
                message: format!("missing cue column header, found {header:?}"),
            });
            process_row(header, 1, &mut sections, &mut diagnostics);
        }
        for (idx, raw) in lines {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            process_row(trimmed, line, &mut sections, &mut diagnostics);
        }
        Ok(CueTable {
            sections,
            diagnostics,
        })
    }

    /// The `header` section named `name` (case-insensitive) — the
    /// exe's cue-type vocabulary (`WEATHER`, `TIMEOFDAY`, `PRERACE`,
    /// `FINALCHECKPOINT`, `RESULTSPOOR`, …).
    pub fn section(&self, name: &str) -> Option<&CueSection> {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Semantic checks beyond the parse grammar: duplicated section
    /// names, empty prefixes, non-positive draw ranges, negative
    /// offsets and `end`/`add` pairs whose draw window overflows
    /// `i64`. Purely advisory — the consumer decides what to skip.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        for (i, section) in self.sections.iter().enumerate() {
            if self.sections[..i]
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&section.name))
            {
                out.push(TableDiagnostic {
                    line: section.rows.first().map(|r| r.line).unwrap_or(0),
                    message: format!("duplicate {} header", section.name),
                });
            }
            for row in &section.rows {
                out.extend(row.problems());
            }
        }
        out
    }
}

/// Parse one data line: a `<NAME> header` marker opens a section,
/// anything else is a `prefix,end,add[,extra…]` cue row.
fn process_row(
    trimmed: &str,
    line: u32,
    sections: &mut Vec<CueSection>,
    diagnostics: &mut Vec<TableDiagnostic>,
) {
    let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
    let first = cells[0];
    // Section marker: `<NAME> header` — the literal marker the exe's
    // cue-type strings terminate with. A stray marker row whose name
    // strips to nothing is a diagnostic, not a section.
    if let Some(name) = strip_header(first) {
        if name.is_empty() {
            diagnostics.push(TableDiagnostic {
                line,
                message: format!("header marker with no name: {trimmed:?}"),
            });
        } else {
            sections.push(CueSection {
                name,
                rows: Vec::new(),
            });
        }
        return;
    }
    let Some(section) = sections.last_mut() else {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("cue row outside a header section: {trimmed:?}"),
        });
        return;
    };
    if cells.len() < 3 {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!(
                "cue row needs at least 3 fields (prefix,end,add), have {}: {trimmed:?}",
                cells.len()
            ),
        });
        return;
    }
    let mut nums = Vec::with_capacity(cells.len() - 1);
    let mut ok = true;
    for (i, cell) in cells.iter().enumerate().skip(1) {
        match cell.parse::<i64>() {
            Ok(v) => nums.push(v),
            Err(_) => {
                ok = false;
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("non-integral field {i} {cell:?} in cue row {first:?}"),
                });
            }
        }
    }
    if !ok {
        return;
    }
    section.rows.push(CueRow {
        prefix: first.to_string(),
        end: nums[0],
        add: nums[1],
        extra: nums.split_off(2),
        line,
    });
}

/// `<NAME> header` → `NAME` (trimmed), case-insensitive on the
/// marker word; `None` when the cell does not end with `header`.
fn strip_header(cell: &str) -> Option<String> {
    cell.to_ascii_lowercase().strip_suffix("header")?;
    Some(cell[..cell.len() - "header".len()].trim().to_string())
}

impl AnnouncerIndex {
    /// Parse a city registry (`aud/spchdata/<city>.csv`). The file is
    /// a `label → value` line pair sequence: `Num announcers` then
    /// the count, `prefix` then the stem prefix. `Err` on empty
    /// input; each missing/malformed field degrades to a diagnostic.
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        let lines: Vec<(u32, &str)> = input
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| ((i + 1) as u32, l.trim()))
            .collect();
        if lines.is_empty() {
            return Err(crate::FormatError::parse(0, "empty announcer registry"));
        }
        let mut diagnostics = Vec::new();
        let mut announcers = None;
        let mut prefix = None;
        let mut i = 0;
        while i < lines.len() {
            let (line, text) = lines[i];
            let label = text.to_ascii_lowercase();
            let value = lines.get(i + 1).map(|(_, l)| *l);
            if label.starts_with("num announcers") {
                match value.and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) => announcers = Some(n),
                    None => diagnostics.push(TableDiagnostic {
                        line,
                        message: format!(
                            "Num announcers without an integral count (have {value:?})"
                        ),
                    }),
                }
                i += 2;
            } else if label == "prefix" {
                match value.filter(|v| !v.is_empty()) {
                    Some(p) => prefix = Some(p.to_string()),
                    None => diagnostics.push(TableDiagnostic {
                        line,
                        message: "prefix label without a value".into(),
                    }),
                }
                i += 2;
            } else {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("unrecognized registry line: {text:?}"),
                });
                i += 1;
            }
        }
        if announcers.is_none() {
            diagnostics.push(TableDiagnostic {
                line: 0,
                message: "no Num announcers field".into(),
            });
        }
        if prefix.is_none() {
            diagnostics.push(TableDiagnostic {
                line: 0,
                message: "no prefix field".into(),
            });
        }
        Ok(AnnouncerIndex {
            announcers: announcers.unwrap_or(0),
            prefix: prefix.unwrap_or_default(),
            diagnostics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The as1-shaped mode table: several sections, varied counts.
    const CHECKPOINT: &str = "Name prefix/type header,end sufix value,sufix add value\r\nPRERACE header,,\r\nPRE,19,0\r\nCHECKGEN,4,0\r\nFINALCHECKPOINT header,,\r\nRACECHECK,4,0\r\nRESULTSPOOR header,,\r\nRESULTPOOR,19,0\r\nRESULTSWIN header,,\r\nRESULTWIN,19,0\r\nUNLOCKRACE header,,\r\nUNLOCKRACE,1,0\r\n";

    #[test]
    fn parses_retail_shaped_cue_table() {
        let table = CueTable::parse(CHECKPOINT).unwrap();
        assert!(table.diagnostics.is_empty(), "{:?}", table.diagnostics);
        assert_eq!(table.sections.len(), 5);
        let pre = table.section("prerace").unwrap();
        assert_eq!(pre.name, "PRERACE");
        assert_eq!(pre.rows.len(), 2);
        assert_eq!(pre.rows[0].prefix, "PRE");
        assert_eq!(pre.rows[0].end, 19);
        assert_eq!(pre.rows[0].add, 0);
        assert!(pre.rows[0].extra.is_empty());
        assert_eq!(table.section("UNLOCKRACE").unwrap().rows[0].end, 1);
        assert!(table.validate().is_empty());
    }

    #[test]
    fn parses_a_weather_prerace_table() {
        let text = "Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nWEARAIN,3,0\n";
        let table = CueTable::parse(text).unwrap();
        assert!(table.diagnostics.is_empty());
        let section = table.section("WEATHER").unwrap();
        assert_eq!(section.rows.len(), 1);
        assert_eq!(section.rows[0].prefix, "WEARAIN");
    }

    #[test]
    fn preserves_the_cops_and_robbers_extra_column() {
        // CNRLONDON.csv-shaped row: speaker-qualified prefix plus a
        // fourth numeric column — preserved verbatim, semantics open.
        let text = "Name prefix/type header,end sufix value,sufix add value\nBLUETEAMHASGOLD header,,,\nAL1\\AL1ROBROB ,1,0,1\n";
        let table = CueTable::parse(text).unwrap();
        assert!(table.diagnostics.is_empty(), "{:?}", table.diagnostics);
        let row = &table.section("blueteamhasgold").unwrap().rows[0];
        assert_eq!(row.prefix, "AL1\\AL1ROBROB");
        assert_eq!(row.end, 1);
        assert_eq!(row.add, 0);
        assert_eq!(row.extra, vec![1]);
    }

    #[test]
    fn tolerates_the_bare_index_table_as_diagnostics() {
        // ccl/cc_cpoint_indexinfo.csv is a different grammar — a bare
        // integer list. Parsed as a cue table it surfaces diagnostics
        // and no sections rather than vanishing.
        let text = "SUFIX NUM FOR CPOINTS\n-1\n15\n3\n";
        let table = CueTable::parse(text).unwrap();
        assert!(table.sections.is_empty());
        assert_eq!(table.diagnostics.len(), 5); // header + 4 orphan rows
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = "Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nWEARAIN,three,0\nWEAFOG,2,0\nORPHAN,1,0\ntoo,few\nPRE,1,0\n";
        // Two malformed rows inside WEATHER + `too,few` under the last
        // open section; nothing outside a section here.
        let table = CueTable::parse(text).unwrap();
        let weather = table.section("WEATHER").unwrap();
        assert_eq!(weather.rows.len(), 3); // WEAFOG + ORPHAN + too,few fails arity
        assert_eq!(weather.rows[0].prefix, "WEAFOG");
        assert!(!table.diagnostics.is_empty());
    }

    #[test]
    fn a_row_outside_any_section_is_flagged() {
        let text = "Name prefix/type header,end sufix value,sufix add value\nORPHAN,1,0\nWEATHER header,,\nWEARAIN,3,0\n";
        let table = CueTable::parse(text).unwrap();
        assert_eq!(table.sections.len(), 1);
        assert_eq!(table.diagnostics.len(), 1);
        assert!(table.diagnostics[0].message.contains("outside a header"));
    }

    #[test]
    fn a_missing_column_header_is_flagged_but_parsed() {
        let text = "WEATHER header,,\nWEARAIN,3,0\n";
        let table = CueTable::parse(text).unwrap();
        assert_eq!(table.sections.len(), 1);
        assert_eq!(table.diagnostics.len(), 1);
        assert!(table.diagnostics[0].message.contains("column header"));
    }

    #[test]
    fn validate_flags_bad_draw_ranges() {
        let text = "Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nZERO,0,0\nNEG,3,-1\n,3,0\nWEATHER header,,\nWEARAIN,3,0\n";
        let table = CueTable::parse(text).unwrap();
        let issues = table.validate();
        assert_eq!(issues.len(), 4); // zero end, negative add, empty prefix, dup section
    }

    #[test]
    fn validate_flags_a_row_that_names_no_wave() {
        // `add` at or past `end` leaves an empty `add + 1 ..= end`
        // window — an advisory diagnostic, like the other bad-range
        // legs. The retail C&R shape (`end` one past `add`) is fine,
        // and so is an `i64`-scale row whose window is non-empty.
        let text = "Name prefix/type header,end sufix value,sufix add value
WEATHER header,,
BIG,3,9223372036854775807
SAME,4,4
SINGLE,6,5
WIDE,9223372036854775807,1
";
        let table = CueTable::parse(text).unwrap();
        let issues = table.validate();
        assert_eq!(issues.len(), 2, "{issues:?}"); // BIG + SAME
        assert!(issues.iter().all(|i| i.message.contains("names no wave")));
    }

    #[test]
    fn parses_the_city_announcer_registry() {
        let text = "\nNum announcers\n6\nprefix\nAL\n";
        let index = AnnouncerIndex::parse(text).unwrap();
        assert!(index.diagnostics.is_empty(), "{:?}", index.diagnostics);
        assert_eq!(index.announcers, 6);
        assert_eq!(index.prefix, "AL");
    }

    #[test]
    fn registry_diagnostics_cover_missing_fields() {
        let index = AnnouncerIndex::parse("Num announcers\nfive\n").unwrap();
        assert_eq!(index.announcers, 0);
        assert!(index.prefix.is_empty());
        assert!(index.diagnostics.len() >= 2);
        assert!(AnnouncerIndex::parse("  \n").is_err());
    }
}

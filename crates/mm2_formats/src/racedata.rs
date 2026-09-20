//! Parser for the `race/<city>/mm*data.csv` event metadata tables.
//!
//! Each stock city ships four tables — `mmracedata.csv` (Checkpoint),
//! `mmblitzdata.csv` (Blitz), `mmcircuitdata.csv` (Circuit) and
//! `mmcrashdata.csv` (Crash Course). One row per selectable event
//! carries the same ten parameters twice: columns 2-11 and 12-21
//! repeat the header verbatim. The second parameter set is the harder
//! one on retail data (shorter time limits, denser traffic), matching
//! the documented Amateur/Professional difficulty split; that mapping
//! is *inferred* — the header does not label the halves.
//!
//! Format: `Description` plus two repeated parameter blocks per row,
//! comma-separated, CRLF or LF line endings, no quoting or escaping in
//! authored data.

use crate::FormatError;

/// The ten parameter column names, in authored order.
const PARAM_FIELDS: [&str; 10] = [
    "CarType",
    "TimeofDay",
    "Weather",
    "Opponents",
    "Cops",
    "Ambient",
    "Peds",
    "NumLaps",
    "TimeLimit",
    "Difficulty",
];

/// Expected columns per row: description + two parameter blocks.
const ROW_ARITY: usize = 1 + 2 * PARAM_FIELDS.len();

/// A parsed event-metadata table.
#[derive(Debug, Clone)]
pub struct EventTable {
    /// One entry per selectable event, in authored order.
    pub rows: Vec<EventRow>,
    /// Recoverable problems (malformed rows, stray lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One event row: a description tag plus the two parameter sets.
#[derive(Debug, Clone)]
pub struct EventRow {
    /// Authored tag — `none` for race events, lesson ids like
    /// `lesson1`/`midtrm1`/`final13` for Crash Course rows.
    pub description: String,
    /// First parameter block (easier conditions on retail data).
    pub amateur: RaceParams,
    /// Second parameter block (harder conditions on retail data).
    pub professional: RaceParams,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// One parameter block from an event row.
#[derive(Debug, Clone)]
pub struct RaceParams {
    /// Vehicle restriction/selection id authored for the event.
    pub car_type: i64,
    /// Time-of-day selector.
    pub time_of_day: i64,
    /// Weather selector.
    pub weather: i64,
    /// Number of computer opponents.
    pub opponents: i64,
    /// Number of police cars placed for the event.
    pub cops: i64,
    /// Ambient traffic density (0.0-1.0 on retail data).
    pub ambient: f32,
    /// Pedestrian density (0.0-1.0 on retail data).
    pub peds: f32,
    /// Lap count (meaningful for Circuit rows).
    pub num_laps: i64,
    /// Event time limit (unit unverified; 25-120 on retail data).
    pub time_limit: f32,
    /// Difficulty selector.
    pub difficulty: i64,
}

/// A non-fatal parse problem.
#[derive(Debug, Clone)]
pub struct TableDiagnostic {
    /// 1-based line number.
    pub line: u32,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for TableDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

fn parse_i64(cell: &str) -> Option<i64> {
    cell.parse().ok()
}

fn parse_f32(cell: &str) -> Option<f32> {
    cell.parse().ok()
}

fn parse_params(cells: &[&str], line: u32, diag: &mut Vec<TableDiagnostic>) -> Option<RaceParams> {
    debug_assert_eq!(cells.len(), PARAM_FIELDS.len());
    // Ambient, Peds and TimeLimit are decimals on retail data; the other
    // columns are integers.
    const FLOAT_COLS: [usize; 3] = [5, 6, 8];
    let mut ok = true;
    let mut ints = [0i64; PARAM_FIELDS.len()];
    let mut floats = [0f32; PARAM_FIELDS.len()];
    for (idx, cell) in cells.iter().enumerate() {
        let parsed = if FLOAT_COLS.contains(&idx) {
            parse_f32(cell).map(|v| floats[idx] = v)
        } else {
            parse_i64(cell).map(|v| ints[idx] = v)
        };
        if parsed.is_none() {
            ok = false;
            diag.push(TableDiagnostic {
                line,
                message: format!("non-numeric {} value {cell:?}", PARAM_FIELDS[idx]),
            });
        }
    }
    if !ok {
        return None;
    }
    Some(RaceParams {
        car_type: ints[0],
        time_of_day: ints[1],
        weather: ints[2],
        opponents: ints[3],
        cops: ints[4],
        ambient: floats[5],
        peds: floats[6],
        num_laps: ints[7],
        time_limit: floats[8],
        difficulty: ints[9],
    })
}

impl EventTable {
    /// Parse a `mm*data.csv` table. Returns `Err` when the header row is
    /// missing or does not match the authored column layout; individual
    /// malformed rows are skipped and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        // Skip blank lines before the header.
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(FormatError::parse(0, "empty event-metadata table"));
        };
        let cols: Vec<&str> = header.split(',').map(str::trim).collect();
        let mut expected = vec!["Description"];
        expected.extend(PARAM_FIELDS.iter().copied());
        expected.extend(PARAM_FIELDS.iter().copied());
        if cols != expected {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected header layout: expected {} columns ({}), have {} ({})",
                    expected.len(),
                    expected.join(","),
                    cols.len(),
                    cols.join(","),
                ),
            ));
        }

        let mut rows = Vec::new();
        let mut diagnostics = Vec::new();
        for (idx, raw) in lines {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
            if cells.len() != ROW_ARITY {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected {ROW_ARITY} fields, have {}",
                        cells.len()
                    ),
                });
                continue;
            }
            let mut row_diag = Vec::new();
            let amateur = parse_params(&cells[1..11], line, &mut row_diag);
            let professional = parse_params(&cells[11..21], line, &mut row_diag);
            diagnostics.append(&mut row_diag);
            let (Some(amateur), Some(professional)) = (amateur, professional) else {
                continue;
            };
            rows.push(EventRow {
                description: cells[0].to_string(),
                amateur,
                professional,
                line,
            });
        }
        Ok(EventTable { rows, diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";

    #[test]
    fn parses_retail_shaped_table() {
        let text = format!(
            "{HEADER}\r\nnone,0,0,0,7,0,0.1,0.0,3,50,1,0,0,1,6,0,0.2,0.0,4,40,1\r\nlesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"
        );
        let table = EventTable::parse(&text).unwrap();
        assert!(table.diagnostics.is_empty());
        assert_eq!(table.rows.len(), 2);
        let r = &table.rows[0];
        assert_eq!(r.description, "none");
        assert_eq!(r.amateur.opponents, 7);
        assert_eq!(r.amateur.time_limit, 50.0);
        assert_eq!(r.professional.opponents, 6);
        assert_eq!(r.professional.weather, 1);
        assert_eq!(r.professional.time_limit, 40.0);
        assert_eq!(table.rows[1].description, "lesson1");
    }

    #[test]
    fn rejects_wrong_header() {
        let err = EventTable::parse("a,b,c\n1,2,3\n").unwrap_err();
        assert!(err.to_string().contains("unexpected header layout"));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(EventTable::parse("  \n\n").is_err());
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = format!(
            "{HEADER}\nshort,0,0\nnone,0,0,0,7,0,0.1,0.0,3,50,1,0,0,1,6,0,0.2,0.0,4,40,1\nnone,0,0,0,7,0,0.1,0.0,3,oops,1,0,0,1,6,0,0.2,0.0,4,40,1\n"
        );
        let table = EventTable::parse(&text).unwrap();
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.diagnostics.len(), 2);
        assert!(table.diagnostics[0].message.contains("expected 21 fields"));
        assert!(table.diagnostics[1].message.contains("TimeLimit"));
    }
}

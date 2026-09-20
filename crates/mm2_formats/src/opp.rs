//! Parser for `race/<city>/<stem>[-a|-p]-N.opp` opponent path records.
//!
//! Each `.opp` file is one opponent's authored driving line: a CSV with
//! header `x,y,z,brake,forward offset,side offset,target speed,speed
//! start,side start` followed by one record per row. The `-a-`/`-p-`
//! infix is inferred to be the Amateur/Professional opponent set
//! (classified by [`crate::racefiles`]); rows are comma-separated with
//! no quoting in authored data.

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// Expected header cell names, in authored order.
const HEADER: [&str; 9] = [
    "x",
    "y",
    "z",
    "brake",
    "forward offset",
    "side offset",
    "target speed",
    "speed start",
    "side start",
];

/// One opponent-path record.
#[derive(Debug, Clone)]
pub struct OppPoint {
    /// Authored `x,y,z` position (MM2 world axes, unmirrored).
    pub position: [f32; 3],
    /// `brake` column — authored values are speeds on retail data; the
    /// exact semantics are unverified.
    pub brake: f32,
    /// `forward offset` column.
    pub forward_offset: f32,
    /// `side offset` column.
    pub side_offset: f32,
    /// `target speed` column.
    pub target_speed: f32,
    /// `speed start` column (0 on retail data).
    pub speed_start: f32,
    /// `side start` column (0 on retail data).
    pub side_start: f32,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `.opp` file.
#[derive(Debug, Clone)]
pub struct OppFile {
    /// One entry per data row, in authored order.
    pub rows: Vec<OppPoint>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

impl OppFile {
    /// Parse an `.opp` file. Returns `Err` when the header row is
    /// missing or does not match the authored layout; malformed rows
    /// are skipped and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(FormatError::parse(0, "empty opponent-path file"));
        };
        let cols: Vec<&str> = header.split(',').map(str::trim).collect();
        if cols != HEADER {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected opponent header layout: expected ({}), have ({})",
                    HEADER.join(","),
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
            if cells.len() != HEADER.len() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected {} fields, have {}",
                        HEADER.len(),
                        cells.len()
                    ),
                });
                continue;
            }
            let mut vals = [0f32; 9];
            let mut ok = true;
            for (i, cell) in cells.iter().enumerate() {
                match cell.parse() {
                    Ok(v) => vals[i] = v,
                    Err(_) => {
                        ok = false;
                        diagnostics.push(TableDiagnostic {
                            line,
                            message: format!("non-numeric {} value {cell:?}", HEADER[i]),
                        });
                    }
                }
            }
            if !ok {
                continue;
            }
            rows.push(OppPoint {
                position: [vals[0], vals[1], vals[2]],
                brake: vals[3],
                forward_offset: vals[4],
                side_offset: vals[5],
                target_speed: vals[6],
                speed_start: vals[7],
                side_start: vals[8],
                line,
            });
        }
        Ok(OppFile { rows, diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shaped_file() {
        let text = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\r\n\
                    -313.240814,-0.150068,-160.756409,175.000000,0,0,0,0,0\r\n\
                    -304.989258,-0.109091,-218.558746,0.000000,0,0,0,0,0\n";
        let f = OppFile::parse(text).unwrap();
        assert!(f.diagnostics.is_empty());
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.rows[0].brake, 175.0);
        assert_eq!(f.rows[1].position[2], -218.55875);
    }

    #[test]
    fn rejects_wrong_header() {
        assert!(OppFile::parse("x,y,z\n1,2,3\n").is_err());
        assert!(OppFile::parse("").is_err());
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n\
                    1,2,3\n1,2,3,4,5,6,7,8,bad\n1,2,3,4,5,6,7,8,9\n";
        let f = OppFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.diagnostics.len(), 2);
    }
}

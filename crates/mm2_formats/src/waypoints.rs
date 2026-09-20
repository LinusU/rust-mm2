//! Parsers for the `race/<city>/` waypoint-record CSVs.
//!
//! Two authored shapes share one module:
//!
//! - `<stem>waypoints.csv` and the free-standing `<stem>.csv` records
//!   carry a header `x,y,z,a,<width>,frame rate,state changes,texture
//!   changes,msg`. The fifth column is spelled `radius` on blitz files
//!   and `poly count` everywhere else on retail data (including the
//!   `frane rate` typo in that variant) — the label is preserved
//!   verbatim because the column's meaning outside blitz is unverified.
//! - `<stem>_strtpnts` start-point files carry the same record shape
//!   with one extra zero column and **no header**.
//!
//! Rows are comma-separated, CRLF or LF endings; no quoting or escaping
//! exists in authored data. The trailing `msg` column keeps whatever
//! text follows the eighth comma so a message containing commas is not
//! silently split.

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// One waypoint/start-point record.
#[derive(Debug, Clone)]
pub struct Waypoint {
    /// Authored `x,y,z` position (MM2 world axes, unmirrored).
    pub position: [f32; 3],
    /// The `a` column in degrees — a heading/orientation is inferred but
    /// unverified.
    pub angle_deg: f32,
    /// Fifth column — `radius` on blitz files, `poly count` elsewhere;
    /// see [`WaypointFile::width_label`].
    pub width: f32,
    /// `frame rate`/`frane rate` column (0 on retail data).
    pub frame_rate: i64,
    /// `state changes` column (0 on retail data).
    pub state_changes: i64,
    /// `texture changes` column (0 on retail data).
    pub texture_changes: i64,
    /// Trailing `msg` text (empty on retail data).
    pub msg: String,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed headered waypoint file (`*waypoints.csv`, `<stem>.csv`).
#[derive(Debug, Clone)]
pub struct WaypointFile {
    /// The authored label of the fifth column — `radius` or
    /// `poly count` on retail data. Its semantics outside blitz are
    /// unverified, so it is kept, not renamed.
    pub width_label: String,
    /// The authored label of the sixth column (`frame rate`, or the
    /// `frane rate` typo on most retail files).
    pub frame_rate_label: String,
    /// One entry per data row, in authored order.
    pub rows: Vec<Waypoint>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One row of a `_strtpnts` start-point file: a pose plus authored
/// auxiliary columns whose meaning is unverified.
#[derive(Debug, Clone)]
pub struct StartPoint {
    /// Authored `x,y,z` position.
    pub position: [f32; 3],
    /// Fourth column — the start heading in degrees (inferred from
    /// retail values; unverified).
    pub angle_deg: f32,
    /// Remaining authored columns (all 0 on retail data).
    pub aux: Vec<i64>,
    /// Trailing `msg` text (empty on retail data).
    pub msg: String,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed headerless `_strtpnts` file.
#[derive(Debug, Clone)]
pub struct StartPointsFile {
    /// One entry per data row, in authored order.
    pub rows: Vec<StartPoint>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
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

impl WaypointFile {
    /// Parse a headered waypoint CSV. Returns `Err` when the header row
    /// is missing or does not start with the authored `x,y,z,a` layout;
    /// malformed rows are skipped and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(FormatError::parse(0, "empty waypoint file"));
        };
        let cols: Vec<&str> = header.split(',').map(str::trim).collect();
        if cols.len() != 9
            || cols[0] != "x"
            || cols[1] != "y"
            || cols[2] != "z"
            || cols[3] != "a"
            || cols[8] != "msg"
        {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected waypoint header layout: expected 9 columns starting x,y,z,a and ending msg, have {} ({})",
                    cols.len(),
                    cols.join(","),
                ),
            ));
        }
        let width_label = cols[4].to_string();
        let frame_rate_label = cols[5].to_string();

        let mut rows = Vec::new();
        let mut diagnostics = Vec::new();
        for (idx, raw) in lines {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            // 8 data columns + msg: keep everything after the eighth
            // comma as the message so a comma inside it cannot shift
            // the record shape.
            let cells: Vec<&str> = trimmed.splitn(9, ',').map(str::trim).collect();
            if cells.len() != 9 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: expected 9 fields, have {}", cells.len()),
                });
                continue;
            }
            let mut d = Vec::new();
            let x = parse_f32(cells[0], "x", line, &mut d);
            let y = parse_f32(cells[1], "y", line, &mut d);
            let z = parse_f32(cells[2], "z", line, &mut d);
            let a = parse_f32(cells[3], "a", line, &mut d);
            let w = parse_f32(cells[4], &width_label, line, &mut d);
            let fr = parse_i64(cells[5], &frame_rate_label, line, &mut d);
            let sc = parse_i64(cells[6], "state changes", line, &mut d);
            let tc = parse_i64(cells[7], "texture changes", line, &mut d);
            diagnostics.append(&mut d);
            let (Some(x), Some(y), Some(z), Some(a), Some(w), Some(fr), Some(sc), Some(tc)) =
                (x, y, z, a, w, fr, sc, tc)
            else {
                continue;
            };
            rows.push(Waypoint {
                position: [x, y, z],
                angle_deg: a,
                width: w,
                frame_rate: fr,
                state_changes: sc,
                texture_changes: tc,
                msg: cells[8].to_string(),
                line,
            });
        }
        Ok(WaypointFile {
            width_label,
            frame_rate_label,
            rows,
            diagnostics,
        })
    }
}

impl StartPointsFile {
    /// Parse a headerless `_strtpnts` file. Returns `Err` when the file
    /// is empty or starts with a header row; malformed rows are skipped
    /// and recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut rows = Vec::new();
        let mut diagnostics = Vec::new();
        let mut saw_data = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            // 9 data columns + msg on retail files; accept any row with
            // at least x,y,z,a plus aux columns and a trailing msg.
            let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
            if !saw_data && cells[0].parse::<f32>().is_err() {
                return Err(FormatError::parse(
                    0,
                    format!(
                        "start-points file starts with non-numeric row {trimmed:?} (these files are headerless)"
                    ),
                ));
            }
            saw_data = true;
            if cells.len() < 5 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected at least 5 fields, have {}",
                        cells.len()
                    ),
                });
                continue;
            }
            let mut d = Vec::new();
            let x = parse_f32(cells[0], "x", line, &mut d);
            let y = parse_f32(cells[1], "y", line, &mut d);
            let z = parse_f32(cells[2], "z", line, &mut d);
            let a = parse_f32(cells[3], "a", line, &mut d);
            let mut aux = Vec::with_capacity(cells.len() - 5);
            let mut aux_ok = true;
            for cell in &cells[4..cells.len() - 1] {
                match parse_i64(cell, "aux", line, &mut d) {
                    Some(v) => aux.push(v),
                    None => aux_ok = false,
                }
            }
            diagnostics.append(&mut d);
            let (Some(x), Some(y), Some(z), Some(a)) = (x, y, z, a) else {
                continue;
            };
            if !aux_ok {
                continue;
            }
            rows.push(StartPoint {
                position: [x, y, z],
                angle_deg: a,
                aux,
                msg: cells[cells.len() - 1].to_string(),
                line,
            });
        }
        if rows.is_empty() && diagnostics.is_empty() {
            return Err(FormatError::parse(0, "empty start-points file"));
        }
        Ok(StartPointsFile { rows, diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blitz_radius_variant() {
        let text = "x,y,z,a,radius,frame rate,state changes,texture changes,msg\r\n\
                    252.172058,-0.149900,-295.174011,170.000000,15.000000,0,0,0,\r\n\
                    299.944000,-0.140000,150.008682,0.000000,11.000000,0,0,0,\n";
        let f = WaypointFile::parse(text).unwrap();
        assert_eq!(f.width_label, "radius");
        assert_eq!(f.frame_rate_label, "frame rate");
        assert!(f.diagnostics.is_empty());
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.rows[0].width, 15.0);
        assert_eq!(f.rows[0].angle_deg, 170.0);
        assert_eq!(f.rows[0].msg, "");
    }

    #[test]
    fn parses_poly_count_variant_with_typo() {
        let text = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n\
                    -447.994659,-0.150100,-187.253784,105.843811,15,0,0,0,\n";
        let f = WaypointFile::parse(text).unwrap();
        assert_eq!(f.width_label, "poly count");
        assert_eq!(f.frame_rate_label, "frane rate");
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.rows[0].width, 15.0);
    }

    #[test]
    fn msg_keeps_commas() {
        let text = "x,y,z,a,radius,frame rate,state changes,texture changes,msg\n\
                    1,2,3,4,5,0,0,0,go left, then right\n";
        let f = WaypointFile::parse(text).unwrap();
        assert_eq!(f.rows[0].msg, "go left, then right");
    }

    #[test]
    fn rejects_wrong_header() {
        assert!(WaypointFile::parse("a,b,c\n1,2,3\n").is_err());
        assert!(WaypointFile::parse("").is_err());
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = "x,y,z,a,radius,frame rate,state changes,texture changes,msg\n\
                    short,1,2\n1,2,3,oops,5,0,0,0,\n1,2,3,4,5,0,0,0,\n";
        let f = WaypointFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.diagnostics.len(), 2);
    }

    #[test]
    fn parses_headerless_start_points() {
        let text = "-489.250977,19.589998,-55.053009,92.360016,0,0,0,0,0,\r\n\
                    -499.991028,19.250000,-54.462997,-267.540039,0,0,0,0,0,\n";
        let f = StartPointsFile::parse(text).unwrap();
        assert!(f.diagnostics.is_empty());
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.rows[0].angle_deg, 92.360016);
        assert_eq!(f.rows[0].aux, vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn start_points_reject_headered_input() {
        let text =
            "x,y,z,a,radius,frame rate,state changes,texture changes,msg\n1,2,3,4,5,0,0,0,\n";
        assert!(StartPointsFile::parse(text).is_err());
    }
}

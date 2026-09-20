//! Parser for `race/<city>/crash<N>data.csv` Crash Course event tables.
//!
//! Each `mmcrashdata.csv` row selects a `crash<N>` lesson; the matching
//! `crash<N>data.csv` is that lesson's own table: header
//! `Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,
//! extra,etra,` followed by one row per sub-event. The header is
//! inconsistent across retail files: `AmbDenisty` is misspelled on
//! several, the tail column names vary (`Misc`, `cornerspeed`,
//! `chkflags`, `numopp`, `extra`), and london `crash8data.csv` omits
//! `Filename` entirely even though its rows still lead with one. Row
//! positions are fixed regardless: filename, event, checkpoints,
//! time limit, ambient density, then a raw integer tail. `Filename`
//! names the
//! waypoint CSV the sub-event runs on (`longjump` → `longjump.csv`,
//! `frogger0waypoints` → `frogger0waypoints.csv`) — it is a reference
//! the event catalog validates. `_p` variants (`crash<N>data_p.csv`)
//! carry the harder parameter set, consistent with the Amateur/
//! Professional split elsewhere — inferred, not documented.
//!
//! The header names only five of the numeric columns; retail rows carry
//! more tail values than named `extra` columns, so the tail is kept as
//! raw integers rather than guessed at.

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// One Crash Course sub-event row.
#[derive(Debug, Clone)]
pub struct CrashDataRow {
    /// Waypoint-CSV stem the sub-event references (`longjump`,
    /// `exam1_1`, `frogger0waypoints`); resolves to
    /// `race/<city>/<filename>.csv` through the VFS.
    pub filename: String,
    /// `Event` column — semantics unverified.
    pub event: i64,
    /// `Checkpoints` column — semantics unverified.
    pub checkpoints: i64,
    /// `TimeLimit` column (seconds inferred from the `mm*data.csv`
    /// time-limit convention; unverified).
    pub time_limit: f32,
    /// `AmbDensity` column — ambient traffic fraction (0.0-0.3 on
    /// retail data).
    pub amb_density: f32,
    /// Tail columns beyond the named five (all integers on retail
    /// data). The header under-names this tail, so values are kept raw.
    pub extra: Vec<i64>,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `crash<N>data.csv` file.
#[derive(Debug, Clone)]
pub struct CrashDataFile {
    /// One entry per sub-event row, in authored order.
    pub rows: Vec<CrashDataRow>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

impl CrashDataFile {
    /// Parse a Crash Course data table. Returns `Err` when the header
    /// row is missing or does not contain the authored
    /// `Event,Checkpoints,TimeLimit` sequence in the first five cells —
    /// the `Filename`/`AmbDensity` names themselves are unreliable on
    /// retail data (see module docs). Malformed rows are skipped and
    /// recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(FormatError::parse(0, "empty crash-course data table"));
        };
        let cols: Vec<&str> = header.split(',').map(str::trim).collect();
        // Positions are fixed in the rows; only the header names drift.
        // Require the identifiable trio Event/Checkpoints/TimeLimit in
        // order somewhere inside the first five cells.
        let trio = cols
            .windows(3)
            .take(4)
            .any(|w| w == ["Event", "Checkpoints", "TimeLimit"]);
        if !trio {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected crash-data header layout: expected Event,Checkpoints,TimeLimit in the first columns, have ({})",
                    cols.join(","),
                ),
            ));
        }
        let mut diagnostics = Vec::new();
        if cols.first() != Some(&"Filename") {
            diagnostics.push(TableDiagnostic {
                line: 1,
                message: "header omits the Filename column name (rows still lead with it)"
                    .to_string(),
            });
        }
        if cols.contains(&"AmbDenisty") {
            diagnostics.push(TableDiagnostic {
                line: 1,
                message: "header misspells AmbDensity as AmbDenisty".to_string(),
            });
        }

        let mut rows = Vec::new();
        for (idx, raw) in lines {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
            // Filename + Event + Checkpoints + TimeLimit + AmbDensity +
            // at least one extra tail value — positions are fixed even
            // when the header omits or misspells a name.
            if cells.len() < 6 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "skipping row: expected at least 6 fields, have {}",
                        cells.len()
                    ),
                });
                continue;
            }
            if cells[0].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: "skipping row: empty Filename".to_string(),
                });
                continue;
            }
            let mut d = Vec::new();
            let mut num = |what: &str, cell: &str| match cell.parse::<f32>() {
                Ok(v) => Some(v),
                Err(_) => {
                    d.push(TableDiagnostic {
                        line,
                        message: format!("non-numeric {what} value {cell:?}"),
                    });
                    None
                }
            };
            let event = num("Event", cells[1]);
            let checkpoints = num("Checkpoints", cells[2]);
            let time_limit = num("TimeLimit", cells[3]);
            let amb_density = num("AmbDensity", cells[4]);
            let mut extra = Vec::with_capacity(cells.len() - 5);
            let mut extra_ok = true;
            for cell in &cells[5..] {
                // Authored tail cells are integers; a trailing comma can
                // leave an empty final cell, which is skipped.
                if cell.is_empty() {
                    continue;
                }
                match cell.parse::<i64>() {
                    Ok(v) => extra.push(v),
                    Err(_) => {
                        extra_ok = false;
                        d.push(TableDiagnostic {
                            line,
                            message: format!("non-numeric extra value {cell:?}"),
                        });
                    }
                }
            }
            diagnostics.append(&mut d);
            let (Some(event), Some(checkpoints), Some(time_limit), Some(amb_density)) =
                (event, checkpoints, time_limit, amb_density)
            else {
                continue;
            };
            if !extra_ok {
                continue;
            }
            rows.push(CrashDataRow {
                filename: cells[0].to_string(),
                event: event as i64,
                checkpoints: checkpoints as i64,
                time_limit,
                amb_density,
                extra,
                line,
            });
        }
        Ok(CrashDataFile { rows, diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shaped_table() {
        let text = "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\r\n\
                    longjump,0,1,26,0,0,0,0,0,0,0\r\n\
                    final1,2,1,200,0,0,0,1,0,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert!(f.diagnostics.is_empty());
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.rows[0].filename, "longjump");
        assert_eq!(f.rows[0].event, 0);
        assert_eq!(f.rows[0].time_limit, 26.0);
        assert_eq!(f.rows[1].extra, vec![0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn retail_header_variants_parse() {
        // london crash8: header drops the Filename name; rows still
        // lead with it. london crash4/8: AmbDenisty typo.
        let text = "Event,Checkpoints,TimeLimit,AmbDenisty,extra,extra,extra,extra,etra,,\n\
                    reverse180,7,1,13,0,0,0,0,0,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.rows[0].filename, "reverse180");
        assert_eq!(f.rows[0].event, 7);
        assert_eq!(f.diagnostics.len(), 2); // both header notes recorded

        let text = "Filename,Event,Checkpoints,TimeLimit,AmbDenisty,extra,extra,extra,extra,etra,\n\
                    copchase,3,1,180,0,0,0,0,0,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.diagnostics.len(), 1);
    }

    #[test]
    fn named_extra_columns_parse() {
        // london crash1/crash6 name some tail columns.
        let text = "Filename,Event,Checkpoints,TimeLimit,AmbDensity,cornerspeed,chkflags,numopp,extra,etra,\n\
                    follow,2,1,36,0,0,0,1,0,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.rows[0].extra, vec![0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn fractional_density_parses() {
        let text = "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n\
                    copchase,3,1,180,0.2,0,0,0,0,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert_eq!(f.rows[0].amb_density, 0.2);
    }

    #[test]
    fn rejects_wrong_header() {
        assert!(CrashDataFile::parse("a,b,c\n1,2,3\n").is_err());
        assert!(CrashDataFile::parse("").is_err());
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n\
                    short,1\n,1,1,10,0,0\nok,1,1,10,0,oops\ngood,1,1,10,0,0\n";
        let f = CrashDataFile::parse(text).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.rows[0].filename, "good");
        assert_eq!(f.diagnostics.len(), 3);
    }
}

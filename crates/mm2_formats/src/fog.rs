//! Parser for `city/<stem>_fog.csv` — the authored per-preset fog tables.
//!
//! Each stock city ships one table: a header row
//! (`fog red,fog green,fog blue,fog start,fog end,description (ignored)`)
//! then sixteen rows of `r,g,b,start,end,label`:
//!
//! ```text
//! 250,230,200,650,1000,clear-morning
//! ```
//!
//! The row index is the same `tod*4 + weather` preset slot the `.ltNN`
//! grid uses (WLD-21). Verified on retail: both tables carry exactly 16
//! rows and every row's label equals the `.ltNN` block name at that slot
//! — `sf_fog.csv` row 6 says `foggy-noon`, `sf.lt06` is `foggy-noon`.
//! MM2Hook's recovered `lvlSky` holds `FogColors[16]`/`FogNearClip[16]`/
//! `FogFarClip[16]` arrays indexed by the same `TimeWeatherType`, which
//! is where these columns land (colour BGRA-packed, distances as short
//! ints). The header's own "description (ignored)" label confirms the
//! original reads positionally.
//!
//! Semantics: `fog start`/`fog end` are the near/far clip distances the
//! fog factor interpolates between — the fixed-function linear fog the
//! recovered field names describe (inferred curve, authored values).

use crate::racedata::TableDiagnostic;
use crate::{FormatError, lighting::LIGHTING_PRESET_COUNT};

/// Rows the measured 16-slot grid expects — one per `.ltNN` preset.
pub const FOG_TABLE_SLOTS: usize = LIGHTING_PRESET_COUNT;

/// A parsed `*_fog.csv` table.
#[derive(Debug, Clone)]
pub struct FogTable {
    /// Data rows in authored order; `rows[i]` is preset slot `i`.
    pub rows: Vec<FogRow>,
    /// Recoverable problems (skipped malformed lines).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One fog row: colour plus near/far distances.
#[derive(Debug, Clone)]
pub struct FogRow {
    /// Authored colour channels (0-255 on retail data).
    pub color: [f32; 3],
    /// Distance the fog starts at, in metres on retail data.
    pub start: f32,
    /// Distance the fog is fully opaque at, in metres on retail data.
    pub end: f32,
    /// Trailing label — declared ignored by the authored header; kept
    /// verbatim for auditing (on retail it mirrors the `.ltNN` name).
    pub description: String,
    /// 1-based source line.
    pub line: u32,
}

/// Semantic findings on a parsed [`FogTable`].
#[derive(Debug, Clone, PartialEq)]
pub enum FogIssue {
    /// The table's row count differs from the measured 16-slot grid —
    /// slot indexing is still positional, but coverage is incomplete
    /// or the file is not a per-preset table.
    RowCount {
        /// Rows present.
        have: usize,
    },
    /// A numeric field is NaN or infinite.
    NonFinite {
        /// 0-based row.
        row: usize,
        /// Which field (`red`/`green`/`blue`/`start`/`end`).
        field: &'static str,
    },
    /// A distance field is negative — meaningless for a clip range.
    NegativeDistance {
        /// 0-based row.
        row: usize,
        /// Which field (`start`/`end`).
        field: &'static str,
        /// Authored value.
        value: f32,
    },
    /// A colour channel is outside the authored 0-255 range.
    ColorOutOfRange {
        /// 0-based row.
        row: usize,
        /// Which channel (0/1/2).
        channel: usize,
        /// Authored value.
        value: f32,
    },
    /// `end <= start` — the band cannot interpolate.
    DegenerateBand {
        /// 0-based row.
        row: usize,
        /// Authored near distance.
        start: f32,
        /// Authored far distance.
        end: f32,
    },
}

impl FogTable {
    /// Parse a `*_fog.csv` table. The first non-blank line may be the
    /// authored header (non-numeric cells); a file whose first line is
    /// already numeric is treated as headerless data. Malformed lines
    /// past the header are skipped into `diagnostics`. Errors only when
    /// no data row parses at all.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut rows = Vec::new();
        let mut diagnostics = Vec::new();
        let mut header_seen = false;

        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
            // The header row: the first non-blank line whose first five
            // cells are not all numeric. Anything else is a data row —
            // malformed ones become diagnostics.
            let numeric = cells.len() >= 5 && cells[..5].iter().all(|c| c.parse::<f32>().is_ok());
            if !header_seen && !numeric {
                header_seen = true;
                if !trimmed.to_ascii_lowercase().contains("fog") {
                    diagnostics.push(TableDiagnostic {
                        line,
                        message: format!("non-fog header line skipped: {trimmed:?}"),
                    });
                }
                continue;
            }
            header_seen = true;
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
            if !numeric {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: "skipping row: non-numeric fog field".to_string(),
                });
                continue;
            }
            let mut f = [0.0f32; 5];
            for (i, c) in cells[..5].iter().enumerate() {
                f[i] = c.parse().unwrap_or(f32::NAN);
            }
            rows.push(FogRow {
                color: [f[0], f[1], f[2]],
                start: f[3],
                end: f[4],
                description: cells[5..].join(","),
                line,
            });
        }

        if rows.is_empty() {
            return Err(FormatError::parse(0, "no fog rows parsed"));
        }
        Ok(FogTable { rows, diagnostics })
    }

    /// The authored row for preset `slot` (`tod*4 + weather`), if the
    /// table covers it.
    pub fn row(&self, slot: usize) -> Option<&FogRow> {
        self.rows.get(slot)
    }

    /// Sanity issues worth reporting (wrong row count, non-finite or
    /// negative values, out-of-range colour, degenerate band). Authored
    /// anomalies are findings, not load failures.
    pub fn validate(&self) -> Vec<FogIssue> {
        let mut issues = Vec::new();
        if self.rows.len() != FOG_TABLE_SLOTS {
            issues.push(FogIssue::RowCount {
                have: self.rows.len(),
            });
        }
        const FIELDS: [&str; 5] = ["red", "green", "blue", "start", "end"];
        for (row, r) in self.rows.iter().enumerate() {
            let values = [r.color[0], r.color[1], r.color[2], r.start, r.end];
            for (i, v) in values.iter().enumerate() {
                if !v.is_finite() {
                    issues.push(FogIssue::NonFinite {
                        row,
                        field: FIELDS[i],
                    });
                }
            }
            for (i, c) in r.color.iter().enumerate() {
                if c.is_finite() && !(0.0..=255.0).contains(c) {
                    issues.push(FogIssue::ColorOutOfRange {
                        row,
                        channel: i,
                        value: *c,
                    });
                }
            }
            for (field, v) in [("start", r.start), ("end", r.end)] {
                if v.is_finite() && v < 0.0 {
                    issues.push(FogIssue::NegativeDistance {
                        row,
                        field,
                        value: v,
                    });
                }
            }
            if r.start.is_finite() && r.end.is_finite() && r.end <= r.start {
                issues.push(FogIssue::DegenerateBand {
                    row,
                    start: r.start,
                    end: r.end,
                });
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "fog red,fog green,fog blue,fog start,fog end,description (ignored)";

    fn table16() -> String {
        let mut s = String::from(HEADER);
        s.push('\n');
        for (i, name) in [
            "clear-morning",
            "cloudy-morning",
            "foggy-morning",
            "rainy-morning",
            "clear-noon",
            "cloudy-noon",
            "foggy-noon",
            "rainy-noon",
            "clear-evening",
            "cloudy-evening",
            "foggy-evening",
            "rainy-evening",
            "clear-night",
            "cloudy-night",
            "foggy-night",
            "rainy-night",
        ]
        .iter()
        .enumerate()
        {
            s.push_str(&format!(
                "{},{},{},{},{},{}\n",
                10 + i,
                20 + i,
                30 + i,
                100 + i,
                900 + i,
                name
            ));
        }
        s
    }

    #[test]
    fn parses_retail_shape() {
        // The real `sf_fog.csv` head, verbatim.
        let t = FogTable::parse(&format!(
            "{HEADER}\r\n250,230,200,650,1000,clear-morning\r\n139,102,105,600,1000,cloudy-morning\r\n"
        ))
        .unwrap();
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].color, [250.0, 230.0, 200.0]);
        assert_eq!(t.rows[0].start, 650.0);
        assert_eq!(t.rows[0].end, 1000.0);
        assert_eq!(t.rows[0].description, "clear-morning");
        assert_eq!(t.rows[0].line, 2);
        assert!(t.diagnostics.is_empty());
        // 2 rows is not the 16-slot grid — a finding, not an error.
        assert_eq!(t.validate(), vec![FogIssue::RowCount { have: 2 }]);
    }

    #[test]
    fn full_grid_indexes_positionally() {
        let t = FogTable::parse(&table16()).unwrap();
        assert_eq!(t.rows.len(), FOG_TABLE_SLOTS);
        assert!(t.validate().is_empty());
        let r6 = t.row(6).unwrap();
        assert_eq!(r6.description, "foggy-noon");
        assert_eq!(r6.color, [16.0, 26.0, 36.0]);
        assert!(t.row(16).is_none());
    }

    #[test]
    fn headerless_first_row_is_data() {
        let t = FogTable::parse("1,2,3,4,5,clear-morning\n").unwrap();
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].color, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn malformed_lines_are_diagnostics_not_panics() {
        let text =
            format!("{HEADER}\nshort,1,2\n250,230,200,650,1000,clear-morning\na,b,c,d,e,f\n");
        let t = FogTable::parse(&text).unwrap();
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.diagnostics.len(), 2);
        assert!(t.diagnostics[0].message.contains("at least 5 fields"));
        assert!(t.diagnostics[1].message.contains("non-numeric"));
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(FogTable::parse("  \n\n").is_err());
        assert!(FogTable::parse("garbage only\n").is_err());
    }

    #[test]
    fn validate_flags_bad_values() {
        let t =
            FogTable::parse("1,2,3,4,5,x\n300,-1,3,-5,2,y\n1,2,3,9,9,z\n1,2,nan,1,2,w\n").unwrap();
        let issues = t.validate();
        assert!(issues.contains(&FogIssue::RowCount { have: 4 }));
        assert!(issues.contains(&FogIssue::ColorOutOfRange {
            row: 1,
            channel: 0,
            value: 300.0
        }));
        assert!(issues.contains(&FogIssue::ColorOutOfRange {
            row: 1,
            channel: 1,
            value: -1.0
        }));
        assert!(issues.contains(&FogIssue::NegativeDistance {
            row: 1,
            field: "start",
            value: -5.0
        }));
        assert!(issues.contains(&FogIssue::DegenerateBand {
            row: 2,
            start: 9.0,
            end: 9.0
        }));
        assert!(issues.contains(&FogIssue::NonFinite {
            row: 3,
            field: "blue"
        }));
    }
}

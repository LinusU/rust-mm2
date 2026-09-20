//! Parser for `race/<city>/<city>_rewards.csv` progression reward links.
//!
//! One table per city links event families to unlocks: header
//! `RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),`
//! followed by rows like `blitz,half,vpcoop2k,0,<message>`. `RaceNum`
//! is `half`/`all` (milestone counts over the family) or a Crash Course
//! lesson index; `VariantNum` 0 unlocks the car itself, anything else a
//! paint job — the header itself documents that rule.
//!
//! Rows are comma-separated with no quoting in authored data; the
//! message keeps whatever follows the fourth comma so text containing
//! commas is preserved. A single trailing record-terminating comma is
//! stripped (authored rows are inconsistent about it).

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// The `RaceNum` column: a family milestone or a Crash Course index.
#[derive(Debug, Clone, PartialEq)]
pub enum RewardNum {
    /// `half` — beating half of the family's events.
    Half,
    /// `all` — beating every event in the family.
    All,
    /// A numeric event index (Crash Course lesson numbers on retail
    /// data).
    Index(i64),
    /// Any other authored token, kept raw.
    Other(String),
}

/// One reward link row.
#[derive(Debug, Clone)]
pub struct RewardRow {
    /// `RaceType` — `blitz`, `circuit`, `race`, `crash` on retail data.
    pub race_type: String,
    /// Which events in the family the reward covers.
    pub race_num: RewardNum,
    /// `CarName` — the vehicle catalog id the reward applies to.
    pub car: String,
    /// `VariantNum` — 0 unlocks the car, otherwise a paint index (per
    /// the authored header note).
    pub variant: i64,
    /// Authored unlock message.
    pub message: String,
    /// 1-based line number in the source file.
    pub line: u32,
}

/// A parsed `<city>_rewards.csv` table.
#[derive(Debug, Clone)]
pub struct RewardsFile {
    /// One entry per row, in authored order.
    pub rows: Vec<RewardRow>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

impl RewardsFile {
    /// Parse a rewards table. Returns `Err` when the header row is
    /// missing or does not start with the authored
    /// `RaceType,RaceNum,CarName` layout; malformed rows are skipped and
    /// recorded in `diagnostics`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut lines = input.lines().enumerate().peekable();
        while lines.next_if(|(_, l)| l.trim().is_empty()).is_some() {}
        let Some((_, header)) = lines.next() else {
            return Err(FormatError::parse(0, "empty rewards table"));
        };
        let cols: Vec<&str> = header.split(',').map(str::trim).collect();
        if cols.len() < 3 || cols[0] != "RaceType" || cols[1] != "RaceNum" || cols[2] != "CarName" {
            return Err(FormatError::parse(
                0,
                format!(
                    "unexpected rewards header layout: expected columns starting RaceType,RaceNum,CarName, have ({})",
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
            // RaceType,RaceNum,CarName,VariantNum then the message keeps
            // the rest verbatim so commas inside it cannot shift fields.
            let cells: Vec<&str> = trimmed.splitn(5, ',').map(str::trim).collect();
            if cells.len() < 5 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: expected 5 fields, have {}", cells.len()),
                });
                continue;
            }
            let variant = match cells[3].parse() {
                Ok(v) => v,
                Err(_) => {
                    diagnostics.push(TableDiagnostic {
                        line,
                        message: format!("non-numeric VariantNum value {:?}", cells[3]),
                    });
                    continue;
                }
            };
            let race_num = match cells[1] {
                "half" => RewardNum::Half,
                "all" => RewardNum::All,
                other => match other.parse::<i64>() {
                    Ok(n) => RewardNum::Index(n),
                    Err(_) => RewardNum::Other(other.to_string()),
                },
            };
            rows.push(RewardRow {
                race_type: cells[0].to_string(),
                race_num,
                car: cells[2].to_string(),
                variant,
                message: cells[4].trim_end_matches(',').to_string(),
                line,
            });
        }
        Ok(RewardsFile { rows, diagnostics })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "RaceType,RaceNum,CarName,VariantNum (zero if it unlocks a car),";

    #[test]
    fn parses_retail_shaped_table() {
        let text = format!(
            "{HEADER}\r\nblitz,half,vpcoop2k,0,Congratulations! You've unlocked the NEW MINI COOPER,\r\ncircuit,all,vpvwcup,4,Congrats!  New paint job for the New Beetle RSi!,\ncrash,12,vpdb7,0,Congratulations! You've unlocked the Aston Martin DB7 Vantage!,\n"
        );
        let f = RewardsFile::parse(&text).unwrap();
        assert!(f.diagnostics.is_empty());
        assert_eq!(f.rows.len(), 3);
        assert_eq!(f.rows[0].race_num, RewardNum::Half);
        assert_eq!(f.rows[1].race_num, RewardNum::All);
        assert_eq!(f.rows[2].race_num, RewardNum::Index(12));
        assert_eq!(f.rows[1].variant, 4);
        assert!(f.rows[0].message.ends_with("MINI COOPER"));
    }

    #[test]
    fn message_keeps_commas_and_drops_record_terminator() {
        let text = format!("{HEADER}\nrace,half,vpx,0,one, two, three,\n");
        let f = RewardsFile::parse(&text).unwrap();
        assert_eq!(f.rows[0].message, "one, two, three");
    }

    #[test]
    fn rejects_wrong_header() {
        assert!(RewardsFile::parse("a,b,c\n1,2,3\n").is_err());
        assert!(RewardsFile::parse("").is_err());
    }

    #[test]
    fn malformed_rows_are_diagnostics_not_panics() {
        let text = format!("{HEADER}\nshort,1\nblitz,half,vpx,oops,msg\n");
        let f = RewardsFile::parse(&text).unwrap();
        assert!(f.rows.is_empty());
        assert_eq!(f.diagnostics.len(), 2);
    }
}

//! Parser for MM2 `.water` "water of death" definitions.
//!
//! A `.water` file is text: the first line is a water height and the
//! remaining lines are integer references:
//!
//! ```text
//! -3.8
//! 345
//! 351
//! 356
//! ```
//!
//! Measured on `city/london.water` (`-3.8` + 3 ids) and `city/sf.water`
//! (`-1.9` + 3 ids). The mm2kiwi description ("the height of deadly water —
//! if a vehicle goes below this height it sleeps with the fishes. Possibly
//! a list of PSDL block ids can be listed for blocks that define deadly
//! water at any height") is consistent with the shape: the London level
//! (-3.8) sits just under the Thames surface where deepwater colliders put
//! a floating car (~-4.0), and the three ids are plausible room indices
//! (all < the city's room count). Runtime semantics are unverified — which
//! consumer reads the level, whether the ids are room numbers, and what
//! "deadly" does are not recovered. Values are preserved verbatim.

/// A parsed `.water` record.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterDef {
    /// Authored water level (world Y).
    pub level: f32,
    /// Trailing integer references verbatim (PSDL block ids per the
    /// community description; unverified).
    pub refs: Vec<i64>,
}

impl WaterDef {
    /// Parse a `.water` file. Fails on an empty file, a non-numeric level
    /// or a non-integer reference line.
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        let mut lines = input.lines().enumerate();
        let mut level = None;
        let mut refs = Vec::new();
        for (i, line) in &mut lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if level.is_none() {
                level = Some(line.parse::<f32>().map_err(|_| {
                    crate::FormatError::parse(
                        i + 1,
                        format!("water level is not numeric: {line:?}"),
                    )
                })?);
            } else {
                refs.push(line.parse::<i64>().map_err(|_| {
                    crate::FormatError::parse(
                        i + 1,
                        format!("water reference is not an integer: {line:?}"),
                    )
                })?);
            }
        }
        let Some(level) = level else {
            return Err(crate::FormatError::parse(0, "empty water file"));
        };
        Ok(WaterDef { level, refs })
    }

    /// Sanity findings: non-finite level, duplicate or negative refs.
    pub fn validate(&self) -> Vec<WaterIssue> {
        let mut issues = Vec::new();
        if !self.level.is_finite() {
            issues.push(WaterIssue::NonFiniteLevel);
        }
        let mut seen = std::collections::BTreeSet::new();
        for &r in &self.refs {
            if r < 0 {
                issues.push(WaterIssue::NegativeRef(r));
            }
            if !seen.insert(r) {
                issues.push(WaterIssue::DuplicateRef(r));
            }
        }
        issues
    }
}

/// Validation findings for a [`WaterDef`].
#[derive(Debug, Clone, PartialEq)]
pub enum WaterIssue {
    /// Level is NaN/infinite.
    NonFiniteLevel,
    /// A reference is negative.
    NegativeRef(i64),
    /// A reference appears twice.
    DuplicateRef(i64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shape() {
        let w = WaterDef::parse("-3.8\r\n345\r\n351\r\n356\r\n").unwrap();
        assert_eq!(w.level, -3.8);
        assert_eq!(w.refs, vec![345, 351, 356]);
        assert!(w.validate().is_empty());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(WaterDef::parse("").is_err());
        assert!(WaterDef::parse("deep").is_err());
        assert!(WaterDef::parse("-3.8\nroom12").is_err());
    }

    #[test]
    fn validate_flags_duplicates_and_negatives() {
        let w = WaterDef::parse("-1.9\n228\n-4\n228").unwrap();
        let issues = w.validate();
        assert!(issues.contains(&WaterIssue::NegativeRef(-4)));
        assert!(issues.contains(&WaterIssue::DuplicateRef(228)));
        let w = WaterDef::parse("NaN\n228").unwrap();
        assert_eq!(w.validate(), vec![WaterIssue::NonFiniteLevel]);
    }
}

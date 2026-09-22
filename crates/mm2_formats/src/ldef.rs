//! Parser for MM2 `.ldef` ambient-light definition files.
//!
//! A `.ldef` file is text: the first line is a development-machine texture
//! path (retail files all reference `\\taxi\projects\madness2\art\...tif`
//! bake sources), and the remaining lines are integer pairs:
//!
//! ```text
//! \\taxi\projects\madness2\art\san_francisco\ambient_lighting\amb_ca_f.tif
//! -2300 2600
//! 500 -1800
//! ```
//!
//! Measured on all 35 retail files: 32 `amb_<w><t>_<x>.ldef` covering the
//! same 4×4 weather × time-of-day grid as `.ltNN` (weather letters
//! `c`/`f`/`p`/`r`, time letters `a`/`d`/`m`/`n`, a `_f`/`_l` variant pair)
//! plus three named extras (`sf_clearmorn`, `sf_clearnoon`,
//! `london_clearmorn`). Only two distinct numeric tails exist, correlating
//! with the `_f`/`_l` suffix — the per-preset variation lives entirely in
//! the referenced texture.
//!
//! Semantics are unrecovered: the dev-art path and uniform numbers suggest
//! bake-time light definitions rather than runtime data. The integer pairs
//! are preserved verbatim; a milli-radian heading/pitch reading is
//! plausible but unverified.

/// A parsed `.ldef` record.
#[derive(Debug, Clone, PartialEq)]
pub struct Ldef {
    /// First line verbatim — a bake-source texture path on retail.
    pub source_ref: String,
    /// Integer rows following the first line, verbatim.
    pub rows: Vec<Vec<i64>>,
}

impl Ldef {
    /// Parse a `.ldef` file. `input` should already be decoded to text.
    /// Fails when the file is empty or a data row is not all integers.
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        let mut lines = input.lines();
        let Some(first) = lines.next() else {
            return Err(crate::FormatError::parse(0, "empty ldef file"));
        };
        let source_ref = first.trim_end_matches('\r').trim().to_string();
        let mut rows = Vec::new();
        for (line_no, line) in lines.enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut row = Vec::new();
            for tok in line.split_whitespace() {
                row.push(tok.parse::<i64>().map_err(|_| {
                    crate::FormatError::parse(
                        line_no + 2,
                        format!("ldef value is not an integer: {tok:?}"),
                    )
                })?);
            }
            rows.push(row);
        }
        Ok(Ldef { source_ref, rows })
    }

    /// Bake-source texture stem when `source_ref` looks like a path
    /// (`amb_ca_f.tif` → `amb_ca_f`); `None` when it has no file part.
    pub fn texture_stem(&self) -> Option<&str> {
        let file = self
            .source_ref
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(self.source_ref.as_str());
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        (!stem.is_empty()).then_some(stem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shape() {
        let l = Ldef::parse(
            "\\\\taxi\\projects\\madness2\\art\\san_francisco\\ambient_lighting\\amb_ca_f.tif\r\n-2300 2600\r\n500 -1800\r\n",
        )
        .unwrap();
        assert_eq!(l.texture_stem(), Some("amb_ca_f"));
        assert_eq!(l.rows, vec![vec![-2300, 2600], vec![500, -1800]]);
    }

    #[test]
    fn rejects_empty_and_non_integer() {
        assert!(Ldef::parse("").is_err());
        assert!(Ldef::parse("ref\n1 x").is_err());
    }

    #[test]
    fn single_ref_only_is_fine() {
        let l = Ldef::parse("bare.ldef\n").unwrap();
        assert!(l.rows.is_empty());
        assert_eq!(l.texture_stem(), Some("bare"));
    }
}

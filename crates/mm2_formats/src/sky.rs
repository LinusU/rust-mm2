//! Parser for MM2 `.sky` sky-dome definition files.
//!
//! A `.sky` file is a single text line naming the dome geometry followed by
//! three floats:
//!
//! ```text
//! sky_dome_l 0 0.95 0.005
//! ```
//!
//! Measured on `city/london.sky`, `city/sf.sky` and `city/phys/j01.sky` from
//! a retail installation — every file is exactly this shape.
//!
//! Field semantics are recovered, not documented: MM2Hook's `lvlSky` carries
//! `HatYOffset`, `YMultiplier` and `RotationRate` next to the dome model, and
//! the authored values (`0`, `0.95`, `0.005`) read plausibly under that
//! mapping — the dome squashed slightly in Y and rotating very slowly. The
//! older mm2kiwi text instead describes "the position for the center of this
//! dome relative to the PSDL". Both readings are recorded here; the fields
//! are preserved verbatim either way and the runtime mapping stays a
//! documented inference until observed.

/// A parsed `.sky` record.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyDef {
    /// Dome geometry name (resolved as `geometry/<model>.pkg` by callers).
    pub model: String,
    /// First float — recovered `lvlSky::HatYOffset` (see module docs).
    pub hat_y_offset: f32,
    /// Second float — recovered `lvlSky::YMultiplier`.
    pub y_multiplier: f32,
    /// Third float — recovered `lvlSky::RotationRate`.
    pub rotation_rate: f32,
}

impl SkyDef {
    /// Parse a `.sky` file. `input` should already be decoded to text.
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        let tokens: Vec<&str> = input.split_whitespace().collect();
        if tokens.len() != 4 {
            return Err(crate::FormatError::parse(
                0,
                format!(
                    "expected 4 tokens (model + 3 floats), found {}",
                    tokens.len()
                ),
            ));
        }
        let mut floats = [0.0f32; 3];
        for (i, tok) in tokens[1..].iter().enumerate() {
            floats[i] = tok.parse::<f32>().map_err(|_| {
                crate::FormatError::parse(0, format!("float {i} is not numeric: {tok:?}"))
            })?;
        }
        Ok(SkyDef {
            model: tokens[0].to_string(),
            hat_y_offset: floats[0],
            y_multiplier: floats[1],
            rotation_rate: floats[2],
        })
    }

    /// Sanity issues worth reporting (empty model name, non-finite fields).
    /// Sign/magnitude are deliberately unconstrained — the authored range is
    /// unknown beyond the three retail files.
    pub fn validate(&self) -> Vec<SkyIssue> {
        let mut issues = Vec::new();
        if self.model.is_empty() {
            issues.push(SkyIssue::EmptyModel);
        }
        if !self.hat_y_offset.is_finite() {
            issues.push(SkyIssue::NonFinite {
                field: "hat_y_offset",
            });
        }
        if !self.y_multiplier.is_finite() {
            issues.push(SkyIssue::NonFinite {
                field: "y_multiplier",
            });
        }
        if !self.rotation_rate.is_finite() {
            issues.push(SkyIssue::NonFinite {
                field: "rotation_rate",
            });
        }
        issues
    }
}

/// Validation findings for a [`SkyDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkyIssue {
    /// The dome model name is empty.
    EmptyModel,
    /// A float field is NaN or infinite.
    NonFinite {
        /// Which field.
        field: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_shape() {
        let s = SkyDef::parse("sky_dome_l 0 0.95 0.005\r\n").unwrap();
        assert_eq!(s.model, "sky_dome_l");
        assert_eq!(s.hat_y_offset, 0.0);
        assert_eq!(s.y_multiplier, 0.95);
        assert_eq!(s.rotation_rate, 0.005);
        assert!(s.validate().is_empty());
    }

    #[test]
    fn rejects_wrong_arity_and_non_numeric() {
        assert!(SkyDef::parse("sky_dome 0 0.95").is_err());
        assert!(SkyDef::parse("sky_dome 0 0.95 0.005 1").is_err());
        assert!(SkyDef::parse("sky_dome 0 x 0.005").is_err());
        assert!(SkyDef::parse("").is_err());
    }

    #[test]
    fn validate_flags_non_finite() {
        let s = SkyDef::parse("m 0 nan 0.005").unwrap();
        assert_eq!(
            s.validate(),
            vec![SkyIssue::NonFinite {
                field: "y_multiplier"
            }]
        );
    }
}

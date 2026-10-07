//! The shared police-roster contract (F20-A.1).
//!
//! One [`PoliceRoster`] is the difficulty-selected `[Police]` lineup an
//! event's (or Cruise's `roam`) `.aimap`/`.aimap_p` record wires: which
//! vehicle stands where. It is the runtime-facing counterpart of the
//! parser record (`mm2_formats::aimap::PoliceRecord`) — a future spawn
//! or pursuit system consumes this, never CSV text.
//!
//! Only what the data shows is interpreted. The first tail value is a
//! heading in degrees (ledger COP-5: finite on all 237 retail rows but
//! spanning −270…535 — an authored `535` is an unnormalised 175° — so
//! it is reduced modulo a turn, the raw value kept in `params`); the
//! remaining columns are kept raw
//! because the file's own header comment ("StartLink, Start Dist, Start
//! Mode, Start Lane, Patrol Route") does not match the observed column
//! counts (UNK-9). Nothing here says what a cop *does* — pursuit rules
//! stay unverified (COP-4). A problem that does not prevent building the
//! roster is a [`PoliceIssue`] on it: reported, never silently repaired.

use std::fmt;

use bevy::prelude::Vec3;

use crate::Difficulty;

/// One authored police spawn — a distilled `[Police]` row.
#[derive(Debug, Clone, PartialEq)]
pub struct PoliceSpec {
    /// Authored vehicle identity — a basename in the vehicle-catalog id
    /// space (`vpcop` on every retail row). Whether it resolves to a
    /// loadable vehicle is the audit's question, not the contract's.
    pub vehicle: String,
    /// Authored spawn position (MM2 world axes, unmirrored).
    pub position: Vec3,
    /// The first tail value as a vehicle-yaw heading in degrees,
    /// reduced to (−180, 180] (see [`normalize_heading`]). `None` when
    /// the row ships no tail at all, or its first value is not finite.
    pub heading_deg: Option<f32>,
    /// Every tail value after the position, raw and including the
    /// heading (five on most retail rows, two on `race/sf/evade0`).
    pub params: Vec<f32>,
    /// 1-based line in the source aimap, for diagnostics.
    pub line: u32,
}

/// Reduce a heading in degrees to (−180, 180]; `None` when not finite.
pub fn normalize_heading(deg: f32) -> Option<f32> {
    if !deg.is_finite() {
        return None;
    }
    let r = deg.rem_euclid(360.0);
    Some(if r > 180.0 { r - 360.0 } else { r })
}

impl PoliceSpec {
    /// Whether a spawner may place the row: finite coordinates, and a
    /// heading that is either absent (no tail) or a finite number.
    pub fn placeable(&self) -> bool {
        self.position.is_finite() && self.params.first().is_none_or(|h| h.is_finite())
    }
}

/// A non-fatal problem found while building a [`PoliceRoster`].
#[derive(Debug, Clone, PartialEq)]
pub enum PoliceIssue {
    /// The requested difficulty's aimap variant is absent; the other
    /// one's roster was used (the only authored lineup that exists).
    MissingVariant {
        /// Difficulty the caller asked for.
        wanted: Difficulty,
        /// Difficulty whose record was read instead.
        used: Difficulty,
    },
    /// The wired roster size disagrees with the table row's authored
    /// `Cops` count.
    CountMismatch {
        /// `[Police]` rows actually wired.
        wired: usize,
        /// The table row's authored count, verbatim.
        table: i64,
    },
    /// A row's position or heading is unusable (see
    /// [`PoliceSpec::placeable`]). The slot is kept so the lineup stays
    /// the authored one; a spawner must refuse it.
    Unplaceable {
        /// 1-based source line.
        line: u32,
    },
}

impl fmt::Display for PoliceIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVariant { wanted, used } => {
                write!(f, "no {wanted:?} aimap variant — using the {used:?} roster")
            }
            Self::CountMismatch { wired, table } => write!(
                f,
                "aimap wires {wired} police but the table row authors {table}"
            ),
            Self::Unplaceable { line } => {
                write!(
                    f,
                    "police row at line {line} has an unusable position or heading"
                )
            }
        }
    }
}

/// The difficulty-selected police lineup for one event or for Cruise.
#[derive(Debug, Clone, Default)]
pub struct PoliceRoster {
    /// One entry per authored `[Police]` row, in file order.
    pub entries: Vec<PoliceSpec>,
    /// `[CopChaseDistance]` when the aimap authors it (retail:
    /// `race/sf/crash5.aimap{,_p}` only). Raw; what the distance means
    /// is unverified.
    pub chase_distance: Option<f32>,
    /// Non-fatal problems found while building (row order).
    pub issues: Vec<PoliceIssue>,
}

impl PoliceRoster {
    /// Entries a spawner may place.
    pub fn placeable(&self) -> impl Iterator<Item = &PoliceSpec> {
        self.entries.iter().filter(|e| e.placeable())
    }

    /// Distinct authored vehicle ids, sorted.
    pub fn vehicles(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(|e| e.vehicle.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tail: &[f32], pos: Vec3) -> PoliceSpec {
        PoliceSpec {
            vehicle: "vpcop".into(),
            position: pos,
            heading_deg: tail.first().copied().and_then(normalize_heading),
            params: tail.to_vec(),
            line: 1,
        }
    }

    #[test]
    fn headings_reduce_modulo_a_turn_and_keep_retail_values() {
        // Retail authors −270…535 (e.g. -110, 169, 535); 535 is a 175° yaw.
        assert_eq!(normalize_heading(-110.0), Some(-110.0));
        assert_eq!(normalize_heading(169.0), Some(169.0));
        assert_eq!(normalize_heading(535.0), Some(175.0));
        assert_eq!(normalize_heading(-270.0), Some(90.0));
        assert_eq!(normalize_heading(-180.0), Some(180.0));
        assert_eq!(normalize_heading(360.0), Some(0.0));
        assert_eq!(normalize_heading(f32::NAN), None);
        assert_eq!(normalize_heading(f32::INFINITY), None);
    }

    #[test]
    fn placeable_needs_a_finite_position_and_a_finite_tail_heading() {
        assert!(spec(&[-110.0, 0.0], Vec3::new(1.0, 2.0, 3.0)).placeable());
        assert!(spec(&[535.0], Vec3::ZERO).placeable());
        assert!(spec(&[], Vec3::ZERO).placeable());
        assert!(!spec(&[f32::NAN], Vec3::ZERO).placeable());
        assert!(!spec(&[0.0], Vec3::new(f32::INFINITY, 0.0, 0.0)).placeable());
        assert_eq!(spec(&[], Vec3::ZERO).heading_deg, None);
    }

    #[test]
    fn vehicles_are_distinct_and_sorted() {
        let mut roster = PoliceRoster::default();
        for v in ["vpcop", "vpbus", "vpcop"] {
            let mut s = spec(&[0.0], Vec3::ZERO);
            s.vehicle = v.into();
            roster.entries.push(s);
        }
        assert_eq!(roster.vehicles(), ["vpbus", "vpcop"]);
        assert_eq!(roster.placeable().count(), 3);
    }
}

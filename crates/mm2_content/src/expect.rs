//! Expected stock-content tables: the authored denominators an inventory
//! audits against, independent of what the VFS successfully resolves.
//!
//! These tables are derived from a retail-install audit (enumerated
//! 2026-09-20; see `docs/ralph/PLAN.md` "Relevant discoveries").
//! Classification: *documented* — they name what a retail install ships.
//! They are **not** claims about which records the original engine
//! requires; partial entries (an event id with only some of the authored
//! files the rest of its family carries) are exactly what the inventory
//! exists to surface.

use std::ops::RangeInclusive;

/// Cities the stock game ships: London and San Francisco.
pub const EXPECTED_CITIES: &[&str] = &["london", "sf"];

/// Cities that have authored single-player race data under `race/<city>/`.
/// Same two ids as [`EXPECTED_CITIES`]; kept separate so a future city can
/// exist without an authored event roster.
pub const EXPECTED_RACE_CITIES: &[&str] = &["london", "sf"];

/// Pedestrian archetypes: `anim/<id>.{mod,skel,rays,shaders}`.
pub const EXPECTED_PEDS: &[&str] = &[
    "pedmodel_man",
    "pedmodel_manw",
    "pedmodel_woman",
    "pedmodel_womanw",
];

/// Files that make a pedestrian archetype complete.
pub const PED_REQUIRED_EXTS: &[&str] = &["mod", "skel", "rays", "shaders"];

/// Audio directories under `aud/` seen on a retail install.
pub const EXPECTED_AUDIO_FAMILIES: &[&str] = &[
    "ambient",
    "aud11",
    "aud22",
    "cardata",
    "creaturedata",
    "dmusic",
    "spchdata",
];

/// The record kind an expected event must carry to count as complete.
///
/// This encodes the authored convention observed on retail — e.g. every
/// stock blitz/checkpoint/circuit/crash/roam event ships an `.aimap` —
/// so an event missing its primary record is *incomplete relative to the
/// authored data*, not necessarily broken for the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryRecord {
    /// `<id>.aimap` — the AI map / event definition every race event and
    /// every numbered crash lesson carries.
    Aimap,
    /// Any csv-family record (`<id>.csv`, `<id>waypoints.csv`,
    /// `<id>data.csv`, `<id>_strtpnts`).
    CsvRecord,
    /// `<id>.pathset`.
    Pathset,
}

/// One expected event id under `race/<city>/`.
#[derive(Debug, Clone)]
pub struct ExpectedEvent {
    /// Event stem, e.g. `blitz3`, `race10`, `exam`.
    pub id: String,
    /// Crash-course / lesson event (`true`) vs race event (`false`).
    pub lesson: bool,
    /// Primary authored record kind required for "complete".
    pub primary: PrimaryRecord,
    /// Match any stem starting with `id` (`exam1_2.csv` → `exam`) rather
    /// than requiring an exact stem.
    pub prefix: bool,
}

fn numbered(
    out: &mut Vec<ExpectedEvent>,
    prefix: &str,
    range: RangeInclusive<u32>,
    lesson: bool,
    primary: PrimaryRecord,
) {
    out.extend(range.map(|n| ExpectedEvent {
        id: format!("{prefix}{n}"),
        lesson,
        primary,
        prefix: false,
    }));
}

fn single(
    out: &mut Vec<ExpectedEvent>,
    id: &str,
    lesson: bool,
    primary: PrimaryRecord,
    prefix: bool,
) {
    out.push(ExpectedEvent {
        id: id.to_string(),
        lesson,
        primary,
        prefix,
    });
}

/// The authored event roster for a stock city (`race/<city>/` records).
///
/// Numbered ranges come from the retail audit; unnumbered lesson entries
/// (`exam`, `final`, `reverse180`) match by stem prefix. Unknown-city
/// input yields an empty roster — the inventory reports that as an
/// unexpected discovered city rather than inventing expectations.
pub fn expected_events(city: &str) -> Vec<ExpectedEvent> {
    let mut v = Vec::new();
    match city {
        "london" => {
            numbered(&mut v, "blitz", 0..=12, false, PrimaryRecord::Aimap);
            numbered(&mut v, "race", 0..=13, false, PrimaryRecord::Aimap);
            numbered(&mut v, "circuit", 0..=11, false, PrimaryRecord::Aimap);
            single(&mut v, "roam", false, PrimaryRecord::Aimap, false);
            numbered(&mut v, "crash", 0..=12, true, PrimaryRecord::Aimap);
            single(&mut v, "exam", true, PrimaryRecord::CsvRecord, true);
            single(&mut v, "final", true, PrimaryRecord::CsvRecord, true);
            single(&mut v, "reverse180", true, PrimaryRecord::CsvRecord, true);
        }
        "sf" => {
            numbered(&mut v, "blitz", 0..=13, false, PrimaryRecord::Aimap);
            numbered(&mut v, "race", 0..=11, false, PrimaryRecord::Aimap);
            // `r0.csv` is a csv-only record audited alongside race0–11;
            // whether it is a standalone event is unverified.
            single(&mut v, "r0", false, PrimaryRecord::CsvRecord, false);
            numbered(&mut v, "circuit", 0..=11, false, PrimaryRecord::Aimap);
            single(&mut v, "roam", false, PrimaryRecord::Aimap, false);
            numbered(&mut v, "crash", 0..=12, true, PrimaryRecord::Aimap);
            // Stunt-course lessons. accel/corner/frogger/jump1/jump2/ramp
            // ship no .aimap on retail; that is authored data, not a gap.
            single(&mut v, "accel0", true, PrimaryRecord::CsvRecord, false);
            single(&mut v, "collide0", true, PrimaryRecord::Aimap, false);
            single(&mut v, "corner0", true, PrimaryRecord::CsvRecord, false);
            single(&mut v, "evade0", true, PrimaryRecord::Aimap, false);
            single(&mut v, "frogger0", true, PrimaryRecord::CsvRecord, false);
            single(&mut v, "jump0", true, PrimaryRecord::Aimap, false);
            single(&mut v, "jump1", true, PrimaryRecord::CsvRecord, false);
            single(&mut v, "jump2", true, PrimaryRecord::CsvRecord, false);
            single(&mut v, "ramp", true, PrimaryRecord::Pathset, false);
            single(&mut v, "stunt0", true, PrimaryRecord::Aimap, false);
            single(&mut v, "exam", true, PrimaryRecord::CsvRecord, true);
            single(&mut v, "final", true, PrimaryRecord::CsvRecord, true);
            single(&mut v, "reverse180", true, PrimaryRecord::CsvRecord, true);
        }
        _ => {}
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_sizes_match_retail_audit() {
        let london = expected_events("london");
        assert_eq!(london.iter().filter(|e| !e.lesson).count(), 40);
        assert_eq!(london.iter().filter(|e| e.lesson).count(), 16);
        let sf = expected_events("sf");
        assert_eq!(sf.iter().filter(|e| !e.lesson).count(), 40);
        assert_eq!(sf.iter().filter(|e| e.lesson).count(), 26);
        assert!(expected_events("tokyo").is_empty());
    }
}

//! `CatalogEvent → PoliceRoster` producer and roster audit (F20-A.1).
//!
//! An event's police are authored in the same `.aimap` (Amateur) /
//! `.aimap_p` (Professional) record that wires its opponents (RACE-11):
//! each `[Police]` row names a vehicle, a spawn position and a raw tail.
//! Free-roam Cruise has no table row — its cops are the `[Police]` rows
//! of `race/<city>/roam.aimap{,_p}` (the extra stem the catalog keeps
//! visible), resolved by [`cruise_police_roster`] with the same variant
//! preference. Both producers share [`police_roster_from_aimap`], and
//! the audit runs every cataloged event through the production path so
//! a spawner and the audit read one lineup.
//!
//! The producer interprets no behavior (COP-4 / UNK-9): it yields where
//! authored cops stand and which vehicle they are, and reports a table
//! `Cops` count that disagrees with the wired rows instead of padding or
//! trimming the lineup.

use std::collections::BTreeSet;

use bevy::prelude::Vec3;
use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_game::{
    Difficulty, EventRef, EventTableKind, PoliceIssue, PoliceRoster, PoliceSpec, normalize_heading,
};

use crate::catalog::VehicleCatalog;
use crate::events::{CatalogEvent, EventCatalog};
use crate::opponents::{EventAimap, RosterBuildError, event_aimap};

/// Build the difficulty-selected police roster for a catalog event.
///
/// The event must be `Ready`, like [`crate::opponent_roster`]: the
/// producer refuses incomplete content rather than fielding a partial
/// authored set.
pub fn police_roster(
    vfs: &Vfs,
    event: &CatalogEvent,
    difficulty: Difficulty,
) -> Result<PoliceRoster, RosterBuildError> {
    let (aimap, picked) = event_aimap(vfs, event, difficulty)?;
    police_roster_from_aimap(event, difficulty, &aimap, &picked)
}

/// [`police_roster`] with the aimap already resolved and parsed (the
/// event session setup shares one read between the opponent roster, the
/// ambient overrides and the police).
pub fn police_roster_from_aimap(
    event: &CatalogEvent,
    difficulty: Difficulty,
    aimap: &Aimap,
    picked: &EventAimap,
) -> Result<PoliceRoster, RosterBuildError> {
    if !event.status.is_ready() {
        return Err(RosterBuildError::NotReady(event.status.clone()));
    }
    if event.event_ref.table == EventTableKind::CrashCourse {
        return Err(RosterBuildError::CrashCourseUnsupported);
    }
    let mut roster = roster_of(aimap);
    if picked.used != difficulty {
        roster.issues.push(PoliceIssue::MissingVariant {
            wanted: difficulty,
            used: picked.used,
        });
    }
    // The table row's authored count is a claim about the lineup, not
    // the lineup.
    let table = event.race_params(difficulty).cops;
    if roster.entries.len() as i64 != table {
        roster.issues.push(PoliceIssue::CountMismatch {
            wired: roster.entries.len(),
            table,
        });
    }
    Ok(roster)
}

/// The police lineup of a city's free-roam (Cruise) record,
/// `race/<city>/roam.aimap` for Amateur and `roam.aimap_p` for
/// Professional, falling back to the one that ships (issue
/// [`PoliceIssue::MissingVariant`]). No table row exists, so no count
/// is cross-checked. Errors when neither record resolves or parses.
pub fn cruise_police_roster(
    vfs: &Vfs,
    city: &str,
    difficulty: Difficulty,
) -> Result<PoliceRoster, RosterBuildError> {
    let (first, second) = match difficulty {
        Difficulty::Amateur => ("aimap", "aimap_p"),
        Difficulty::Professional => ("aimap_p", "aimap"),
    };
    let path = |ext: &str| format!("race/{city}/roam.{ext}");
    let (logical, used) = if vfs.resolve(&path(first)).is_some() {
        (path(first), difficulty)
    } else if vfs.resolve(&path(second)).is_some() {
        let other = match difficulty {
            Difficulty::Amateur => Difficulty::Professional,
            Difficulty::Professional => Difficulty::Amateur,
        };
        (path(second), other)
    } else {
        return Err(RosterBuildError::NoAimapRecord);
    };
    let bytes = vfs
        .read_logical(&logical)
        .map_err(|e| RosterBuildError::AimapUnreadable {
            logical: logical.clone(),
            reason: e.to_string(),
        })?;
    let aimap = Aimap::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| {
        RosterBuildError::AimapParse {
            logical: logical.clone(),
            reason: e.to_string(),
        }
    })?;
    let mut roster = roster_of(&aimap);
    if used != difficulty {
        roster.issues.push(PoliceIssue::MissingVariant {
            wanted: difficulty,
            used,
        });
    }
    Ok(roster)
}

/// Distill an aimap's `[Police]` rows (the part both producers share).
fn roster_of(aimap: &Aimap) -> PoliceRoster {
    let mut roster = PoliceRoster {
        chase_distance: aimap.cop_chase_distance,
        ..PoliceRoster::default()
    };
    for row in &aimap.police {
        let spec = PoliceSpec {
            vehicle: row.geo.clone(),
            position: Vec3::from_array(row.position),
            heading_deg: row.params.first().copied().and_then(normalize_heading),
            params: row.params.clone(),
            line: row.line,
        };
        if !spec.placeable() {
            roster
                .issues
                .push(PoliceIssue::Unplaceable { line: row.line });
        }
        roster.entries.push(spec);
    }
    roster
}

/// Outcome of building one roster inside a [`PoliceReport`].
#[derive(Debug)]
pub enum PoliceBuild {
    /// The roster built (possibly carrying issues).
    Built(PoliceSummary),
    /// Crash Course — deferred to F21, counted not failed.
    Unsupported,
    /// The producer refused the event.
    Failed(RosterBuildError),
}

impl PoliceBuild {
    fn of(result: Result<PoliceRoster, RosterBuildError>, table: Option<i64>) -> Self {
        match result {
            Ok(r) => Self::Built(PoliceSummary {
                wired: r.entries.len(),
                table_cops: table,
                vehicles: r.vehicles(),
                chase_distance: r.chase_distance,
                issues: r.issues,
            }),
            Err(RosterBuildError::CrashCourseUnsupported) => Self::Unsupported,
            Err(e) => Self::Failed(e),
        }
    }
}

/// What the audit reports about a built roster.
#[derive(Debug)]
pub struct PoliceSummary {
    /// `[Police]` rows wired.
    pub wired: usize,
    /// The table row's authored `Cops` count (`None` for Cruise, which
    /// has no table row).
    pub table_cops: Option<i64>,
    /// Distinct authored vehicle ids, sorted.
    pub vehicles: Vec<String>,
    /// `[CopChaseDistance]` when authored.
    pub chase_distance: Option<f32>,
    /// Issues the build reported.
    pub issues: Vec<PoliceIssue>,
}

/// One catalog event's police audit at both difficulties.
#[derive(Debug)]
pub struct PoliceEntry {
    /// Stable identity — `(city, table, row)`.
    pub event_ref: EventRef,
    /// File stem (`race0`).
    pub stem: String,
    /// Amateur-parameter build (prefers `<stem>.aimap`).
    pub amateur: PoliceBuild,
    /// Professional-parameter build (prefers `<stem>.aimap_p`).
    pub professional: PoliceBuild,
}

/// A non-table, non-roam stem whose aimap still wires police — a
/// discovered lineup outside the selectable-event denominator.
#[derive(Debug)]
pub struct ExtraPolice {
    /// Aimap logical path.
    pub logical: String,
    /// `[Police]` rows wired.
    pub wired: usize,
}

/// Whole-city police audit: every cataloged event through the
/// production [`police_roster`] at both difficulties, the Cruise
/// lineup, extra stems that still wire police, and every wired vehicle
/// id checked against the vehicle catalog.
#[derive(Debug)]
pub struct PoliceReport {
    /// City stem audited.
    pub city: String,
    /// One entry per authored table row, in catalog order.
    pub entries: Vec<PoliceEntry>,
    /// The free-roam lineup at Amateur and Professional.
    pub cruise: [PoliceBuild; 2],
    /// Other stems carrying a wired lineup.
    pub extras: Vec<ExtraPolice>,
    /// Vehicle ids wired that resolve to no vehicle-catalog entry.
    pub unresolved_vehicles: Vec<String>,
}

impl PoliceReport {
    /// Scan `race/<city>/` through the VFS and audit every lineup.
    /// Never fails as a whole — partial installs report honestly.
    pub fn scan(vfs: &Vfs, city: &str) -> Self {
        let catalog = EventCatalog::scan(vfs, city);
        let vehicles = VehicleCatalog::scan(vfs);
        let ids: BTreeSet<&str> = vehicles.entries.iter().map(|e| e.id.as_str()).collect();
        let mut wired: BTreeSet<String> = BTreeSet::new();

        let mut note = |b: &PoliceBuild| {
            if let PoliceBuild::Built(s) = b {
                wired.extend(s.vehicles.iter().cloned());
            }
        };
        let entries: Vec<PoliceEntry> = catalog
            .events
            .iter()
            .map(|event| {
                let build = |d| {
                    PoliceBuild::of(
                        police_roster(vfs, event, d),
                        Some(event.race_params(d).cops),
                    )
                };
                let entry = PoliceEntry {
                    event_ref: event.event_ref.clone(),
                    stem: event.stem.clone(),
                    amateur: build(Difficulty::Amateur),
                    professional: build(Difficulty::Professional),
                };
                note(&entry.amateur);
                note(&entry.professional);
                entry
            })
            .collect();
        let cruise = [Difficulty::Amateur, Difficulty::Professional]
            .map(|d| PoliceBuild::of(cruise_police_roster(vfs, &catalog.city, d), None));
        cruise.iter().for_each(&mut note);

        // Extras stay in the denominator: a stem the tables never claim
        // can still wire cops (dev leftovers, mod events). `roam` is
        // the Cruise lineup above, not an extra.
        let mut extras = Vec::new();
        for extra in catalog.extras.iter().filter(|e| e.label != "roam") {
            for ext in ["aimap", "aimap_p"] {
                let logical = format!("race/{}/{}.{ext}", catalog.city, extra.label);
                let Ok(bytes) = vfs.read_logical(&logical) else {
                    continue;
                };
                let Ok(aimap) = Aimap::parse(&String::from_utf8_lossy(&bytes)) else {
                    continue;
                };
                if aimap.police.is_empty() {
                    continue;
                }
                wired.extend(aimap.police.iter().map(|r| r.geo.clone()));
                extras.push(ExtraPolice {
                    logical,
                    wired: aimap.police.len(),
                });
            }
        }

        let unresolved_vehicles = wired
            .iter()
            .filter(|id| !ids.contains(id.as_str()))
            .cloned()
            .collect();
        Self {
            city: catalog.city,
            entries,
            cruise,
            extras,
            unresolved_vehicles,
        }
    }

    /// Every build in the report (events at both difficulties + Cruise).
    pub fn builds(&self) -> impl Iterator<Item = &PoliceBuild> {
        self.entries
            .iter()
            .flat_map(|e| [&e.amateur, &e.professional])
            .chain(self.cruise.iter())
    }

    /// Builds that produced a roster.
    pub fn built(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, PoliceBuild::Built(_)))
            .count()
    }

    /// Builds on deliberately-deferred kinds (Crash Course).
    pub fn unsupported(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, PoliceBuild::Unsupported))
            .count()
    }

    /// Builds the producer rejected.
    pub fn failed(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, PoliceBuild::Failed(_)))
            .count()
    }

    /// Total police wired across all built rosters.
    pub fn wired(&self) -> usize {
        self.builds()
            .filter_map(|b| match b {
                PoliceBuild::Built(s) => Some(s.wired),
                _ => None,
            })
            .sum()
    }

    /// Total issues across all built rosters.
    pub fn issues(&self) -> usize {
        self.builds()
            .filter_map(|b| match b {
                PoliceBuild::Built(s) => Some(s.issues.len()),
                _ => None,
            })
            .sum()
    }
}

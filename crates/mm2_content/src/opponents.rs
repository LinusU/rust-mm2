//! `CatalogEvent → OpponentRoster` producer and roster audit (F15-A.1).
//!
//! A race event's opponent lineup is authored in its `.aimap` (Amateur)
//! or `.aimap_p` (Professional) `[Opponent]` section (RACE-11): each
//! row names a vehicle id and a `*.opp` route record, plus a numeric
//! tail kept raw. [`opponent_roster`] distills that into the shared
//! [`OpponentRoster`] contract — the same parsed catalog records the
//! race-definition producer consumes, so the audit and any future
//! spawn system resolve the lineup identically.
//!
//! The variant split is per-difficulty (RACE-11): Professional prefers
//! `<stem>.aimap_p` and Amateur `<stem>.aimap`. When an event ships only
//! one variant the other difficulty falls back to it — the only
//! authored lineup that exists — recorded as
//! [`OpponentIssue::MissingVariant`]. No event invents opponents: a
//! table `Opponents` count that disagrees with the wired rows is an
//! issue, not a reason to pad or trim the lineup.

use std::collections::BTreeSet;

use bevy::prelude::Vec3;
use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::opp::OppFile;
use mm2_formats::racefiles::RaceFileKind;
use mm2_game::{
    Difficulty, EventRef, EventTableKind, OpponentIssue, OpponentRoster, OpponentRoute,
    OpponentRoutePoint, OpponentSpec,
};
use thiserror::Error;

use crate::catalog::VehicleCatalog;
use crate::events::{CatalogEvent, EventCatalog, EventStatus, RecordContent};

/// Why an event's [`OpponentRoster`] cannot be built at all.
#[derive(Debug, Error)]
pub enum RosterBuildError {
    /// `CatalogEvent::status` is not `Ready` — required records are
    /// missing or failed; the catalog's `resolve` reports the detail.
    #[error("event is not ready: {0:?}")]
    NotReady(EventStatus),
    /// Crash Course rows describe lesson sub-events, not a rostered
    /// checkpoint race — deferred to the crash-course slice (F21).
    #[error("crash course events are not loadable yet")]
    CrashCourseUnsupported,
    /// A `Ready` event carries neither aimap variant for its own stem.
    /// Unreachable through the current catalog (a missing `.aimap`
    /// already makes the event `Incomplete`); defensive.
    #[error("event has no aimap record")]
    NoAimapRecord,
    /// The selected aimap record resolved at scan time but cannot be
    /// read back through the VFS.
    #[error("aimap record {logical} is unreadable: {reason}")]
    AimapUnreadable {
        /// Resolved logical path.
        logical: String,
        /// VFS error.
        reason: String,
    },
    /// The selected aimap record reads but does not parse (records are
    /// not parsed at scan time, so this is the first place a malformed
    /// file surfaces).
    #[error("aimap record {logical} failed to parse: {reason}")]
    AimapParse {
        /// Resolved logical path.
        logical: String,
        /// Parser error.
        reason: String,
    },
}

/// Which aimap record [`event_aimap`] resolved, beyond the parse.
#[derive(Debug, Clone)]
pub struct EventAimap {
    /// Difficulty tag of the record used (`'a'`/`'p'`) — `.opp` route
    /// records are checked against it.
    pub tag: char,
    /// Difficulty whose record was used — differs from the requested
    /// one when only the other variant shipped (see
    /// [`OpponentIssue::MissingVariant`]).
    pub used: Difficulty,
}

/// Resolve, read and parse an event's difficulty-selected aimap record
/// — the shared pick `opponent_roster` and the ambient-traffic session
/// setup both need: Professional prefers `<stem>.aimap_p`, Amateur
/// `<stem>.aimap`, and when only one variant ships the other difficulty
/// falls back to it (RACE-11).
pub fn event_aimap(
    vfs: &Vfs,
    event: &CatalogEvent,
    difficulty: Difficulty,
) -> Result<(Aimap, EventAimap), RosterBuildError> {
    let (preferred, fallback, preferred_tag) = match difficulty {
        Difficulty::Amateur => (RaceFileKind::Aimap, RaceFileKind::AimapP, 'a'),
        Difficulty::Professional => (RaceFileKind::AimapP, RaceFileKind::Aimap, 'p'),
    };
    // The event's *own* stem selects the record — crash-link attribution
    // can attach sibling-stem aimaps to an event, and a mod may share
    // stems; matching the basename keeps the pick unambiguous.
    let aimap_record = |kind: RaceFileKind| {
        event
            .records
            .iter()
            .find(|r| r.kind == kind && record_stem(r) == event.stem.as_str())
    };
    let (record, tag, used) = match aimap_record(preferred) {
        Some(r) => (r, preferred_tag, difficulty),
        None => match aimap_record(fallback) {
            Some(r) => (
                r,
                if preferred_tag == 'a' { 'p' } else { 'a' },
                match difficulty {
                    Difficulty::Amateur => Difficulty::Professional,
                    Difficulty::Professional => Difficulty::Amateur,
                },
            ),
            None => return Err(RosterBuildError::NoAimapRecord),
        },
    };

    let bytes =
        vfs.read_logical(&record.logical)
            .map_err(|e| RosterBuildError::AimapUnreadable {
                logical: record.logical.clone(),
                reason: e.to_string(),
            })?;
    let aimap = Aimap::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| {
        RosterBuildError::AimapParse {
            logical: record.logical.clone(),
            reason: e.to_string(),
        }
    })?;
    Ok((aimap, EventAimap { tag, used }))
}

/// Build the difficulty-selected opponent roster for a catalog event.
///
/// The event must be `Ready` — like [`crate::race_definition`], the
/// producer refuses incomplete content rather than racing a partial
/// authored set. A built roster can still carry [`OpponentIssue`]s;
/// those are authored-data problems the runtime should see, not
/// reasons to reject the event.
pub fn opponent_roster(
    vfs: &Vfs,
    event: &CatalogEvent,
    difficulty: Difficulty,
) -> Result<OpponentRoster, RosterBuildError> {
    let (aimap, picked) = event_aimap(vfs, event, difficulty)?;
    opponent_roster_from_aimap(event, difficulty, &aimap, &picked)
}

/// [`opponent_roster`] with the difficulty-selected aimap already
/// resolved and parsed — a caller that also needs the record itself
/// (the event session setup: the same aimap authors the ambient
/// overrides) shares one read+parse between both consumers instead of
/// paying it twice. The roster build itself reads only already-parsed
/// record content, so it takes no `vfs`.
pub fn opponent_roster_from_aimap(
    event: &CatalogEvent,
    difficulty: Difficulty,
    aimap: &Aimap,
    picked: &EventAimap,
) -> Result<OpponentRoster, RosterBuildError> {
    if !event.status.is_ready() {
        return Err(RosterBuildError::NotReady(event.status.clone()));
    }
    if event.event_ref.table == EventTableKind::CrashCourse {
        return Err(RosterBuildError::CrashCourseUnsupported);
    }

    let tag = picked.tag;

    let mut issues = Vec::new();
    if picked.used != difficulty {
        issues.push(OpponentIssue::MissingVariant {
            wanted: difficulty,
            used: picked.used,
        });
    }

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut entries = Vec::new();
    for row in &aimap.opponents {
        referenced.insert(row.waypoints.clone());
        let route = match event
            .records
            .iter()
            .find(|r| r.kind == RaceFileKind::Opp && record_basename(r) == row.waypoints)
        {
            None => {
                issues.push(OpponentIssue::UnresolvedRoute {
                    name: row.waypoints.clone(),
                });
                None
            }
            Some(rec) => match &rec.content {
                RecordContent::Opp(file) => {
                    if let Some(t) = rec.difficulty
                        && t != tag
                    {
                        issues.push(OpponentIssue::WrongDifficultyTag {
                            name: row.waypoints.clone(),
                            expected: tag,
                        });
                    }
                    Some(distill_route(file))
                }
                RecordContent::Failed(reason) => {
                    issues.push(OpponentIssue::RouteFailed {
                        name: row.waypoints.clone(),
                        reason: reason.clone(),
                    });
                    None
                }
                // `.opp` records always parse at scan time (into `Opp`
                // or `Failed`); any other content cannot occur.
                _ => {
                    issues.push(OpponentIssue::RouteFailed {
                        name: row.waypoints.clone(),
                        reason: "record is not parsed".into(),
                    });
                    None
                }
            },
        };
        entries.push(OpponentSpec {
            vehicle: row.geo.clone(),
            params: row.params.clone(),
            route,
        });
    }

    // The table row's authored count is a claim about the lineup, not
    // the lineup — a disagreement is reported, never padded or trimmed
    // (RACE-11's `sf/race0` anomaly is exactly this shape).
    let table = event.race_params(difficulty).opponents;
    if entries.len() as i64 != table {
        issues.push(OpponentIssue::CountMismatch {
            wired: entries.len(),
            table,
        });
    }

    // Route records this difficulty's roster never references — only
    // the selected variant's tag (or untagged records) count; the other
    // difficulty's `-a-`/`-p-` files are not this roster's business.
    for rec in event.records.iter().filter(|r| r.kind == RaceFileKind::Opp) {
        if rec.difficulty.is_some_and(|t| t != tag) {
            continue;
        }
        let name = record_basename(rec).to_string();
        if !referenced.contains(&name) {
            issues.push(OpponentIssue::UnreferencedRoute { name });
        }
    }

    Ok(OpponentRoster { entries, issues })
}

/// The basename of a record's logical path (`race/london/race0-a-0.opp`
/// → `race0-a-0.opp`).
fn record_basename(record: &crate::events::EventRecord) -> &str {
    record.logical.rsplit('/').next().unwrap_or(&record.logical)
}

/// The event stem a record's basename encodes (`race0-a-0.opp` →
/// `race0`, `circuit0.aimap_p` → `circuit0`) via the shared race-file
/// grammar.
fn record_stem(record: &crate::events::EventRecord) -> &str {
    let name = record_basename(record);
    // `.aimap_p` first: it also ends with `p`-suffixed `.aimap` text.
    for ext in [".aimap_p", ".aimap"] {
        if let Some(s) = name.strip_suffix(ext) {
            return s;
        }
    }
    name
}

/// Distill a parsed `.opp` file into the contract's driving line.
/// Columns stay verbatim — their semantics are unverified (UNK-11).
fn distill_route(file: &OppFile) -> OpponentRoute {
    OpponentRoute {
        points: file
            .rows
            .iter()
            .map(|p| OpponentRoutePoint {
                position: Vec3::new(p.position[0], p.position[1], p.position[2]),
                brake: p.brake,
                forward_offset: p.forward_offset,
                side_offset: p.side_offset,
                target_speed: p.target_speed,
                speed_start: p.speed_start,
                side_start: p.side_start,
            })
            .collect(),
    }
}

/// Outcome of building one event's [`OpponentRoster`] at one difficulty
/// inside an [`OpponentReport`].
#[derive(Debug)]
pub enum RosterBuild {
    /// The roster built (possibly carrying issues).
    Built(RosterSummary),
    /// Crash Course — deferred to F21, counted not failed.
    Unsupported,
    /// The producer refused the event.
    Failed(RosterBuildError),
}

/// What the audit reports about a built roster — enough to show the
/// event wires its own authored lineup rather than a shared template.
#[derive(Debug)]
pub struct RosterSummary {
    /// `[Opponent]` rows wired (roster size).
    pub wired: usize,
    /// Entries whose `.opp` route resolved.
    pub routes: usize,
    /// The table row's authored `Opponents` count, verbatim.
    pub table_opponents: i64,
    /// Distinct authored vehicle ids, sorted.
    pub vehicles: Vec<String>,
    /// Issues the build reported.
    pub issues: Vec<OpponentIssue>,
}

/// One catalog event's roster audit at both difficulties.
#[derive(Debug)]
pub struct RosterEntry {
    /// Stable identity — `(city, table, row)`.
    pub event_ref: EventRef,
    /// File stem (`race0`).
    pub stem: String,
    /// Amateur-parameter build (prefers `<stem>.aimap`).
    pub amateur: RosterBuild,
    /// Professional-parameter build (prefers `<stem>.aimap_p`).
    pub professional: RosterBuild,
}

/// A non-table stem whose aimap still wires `[Opponent]` rows — a
/// discovered roster outside the selectable-event denominator (dev
/// leftovers like `stunt0`, extras the tables never claim).
#[derive(Debug)]
pub struct ExtraRoster {
    /// Aimap logical path (`race/<city>/<stem>.aimap{,_p}`).
    pub logical: String,
    /// `[Opponent]` rows wired.
    pub wired: usize,
    /// Wired `.opp` names that resolve nowhere in the VFS.
    pub dead_refs: usize,
}

/// Whole-city roster audit: every cataloged event run through the
/// production [`opponent_roster`] producer at both difficulties, plus
/// every extra stem's aimap checked for wired lineups, plus the wired
/// vehicle ids cross-checked against the vehicle catalog.
#[derive(Debug)]
pub struct OpponentReport {
    /// City stem audited.
    pub city: String,
    /// One entry per authored table row, in catalog order.
    pub entries: Vec<RosterEntry>,
    /// Non-event stems carrying a wired lineup.
    pub extra_rosters: Vec<ExtraRoster>,
    /// Vehicle ids the rosters wire that resolve to no vehicle-catalog
    /// entry (sorted, deduplicated).
    pub unresolved_vehicles: Vec<String>,
}

impl OpponentReport {
    /// Scan `race/<city>/` through the VFS and audit every cataloged
    /// event's opponent lineup. Never fails as a whole — partial
    /// installs report honestly.
    pub fn scan(vfs: &Vfs, city: &str) -> Self {
        let catalog = EventCatalog::scan(vfs, city);
        let vehicles = VehicleCatalog::scan(vfs);
        let vehicle_ids: BTreeSet<&str> = vehicles.entries.iter().map(|e| e.id.as_str()).collect();

        let mut wired_vehicles: BTreeSet<String> = BTreeSet::new();
        let entries: Vec<RosterEntry> = catalog
            .events
            .iter()
            .map(|event| RosterEntry {
                event_ref: event.event_ref.clone(),
                stem: event.stem.clone(),
                amateur: audit_roster(vfs, event, Difficulty::Amateur, &mut wired_vehicles),
                professional: audit_roster(
                    vfs,
                    event,
                    Difficulty::Professional,
                    &mut wired_vehicles,
                ),
            })
            .collect();

        // Extras keep their aimaps in the denominator: a stem the
        // tables never claim can still wire a lineup (dev leftovers,
        // mod events) — discovered, counted, never filtered out.
        let mut extra_rosters = Vec::new();
        for extra in &catalog.extras {
            for (kind, ext) in [
                (RaceFileKind::Aimap, "aimap"),
                (RaceFileKind::AimapP, "aimap_p"),
            ] {
                if !extra.kinds.contains(&kind) {
                    continue;
                }
                let logical = format!(
                    "race/{}/{extra_stem}.{ext}",
                    catalog.city,
                    extra_stem = extra.label
                );
                let Ok(bytes) = vfs.read_logical(&logical) else {
                    continue;
                };
                let Ok(aimap) = Aimap::parse(&String::from_utf8_lossy(&bytes)) else {
                    continue;
                };
                if aimap.opponents.is_empty() {
                    continue;
                }
                let dead_refs = aimap
                    .opponents
                    .iter()
                    .filter(|r| {
                        vfs.resolve(&format!("race/{}/{}", catalog.city, r.waypoints))
                            .is_none()
                    })
                    .count();
                extra_rosters.push(ExtraRoster {
                    logical,
                    wired: aimap.opponents.len(),
                    dead_refs,
                });
            }
        }

        let unresolved_vehicles = wired_vehicles
            .iter()
            .filter(|id| !vehicle_ids.contains(id.as_str()))
            .cloned()
            .collect();

        Self {
            city: catalog.city,
            entries,
            extra_rosters,
            unresolved_vehicles,
        }
    }

    /// Builds that produced a roster (at most `2 × entries.len()`).
    pub fn built(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, RosterBuild::Built(_)))
            .count()
    }

    /// Builds on deliberately-deferred kinds (Crash Course).
    pub fn unsupported(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, RosterBuild::Unsupported))
            .count()
    }

    /// Builds the producer rejected.
    pub fn failed(&self) -> usize {
        self.builds()
            .filter(|b| matches!(b, RosterBuild::Failed(_)))
            .count()
    }

    /// Total opponents wired across all built rosters.
    pub fn wired(&self) -> usize {
        self.builds()
            .filter_map(|b| match b {
                RosterBuild::Built(s) => Some(s.wired),
                _ => None,
            })
            .sum()
    }

    /// Total issues across all built rosters.
    pub fn issues(&self) -> usize {
        self.builds()
            .filter_map(|b| match b {
                RosterBuild::Built(s) => Some(s.issues.len()),
                _ => None,
            })
            .sum()
    }

    fn builds(&self) -> impl Iterator<Item = &RosterBuild> {
        self.entries
            .iter()
            .flat_map(|e| [&e.amateur, &e.professional])
    }
}

/// Run one event through [`opponent_roster`] at one difficulty and
/// distill the outcome into a [`RosterBuild`], collecting the wired
/// vehicle ids into `wired_vehicles` for the catalog cross-check —
/// the per-event unit [`OpponentReport::scan`] and single-event
/// inspection both share.
pub fn audit_roster(
    vfs: &Vfs,
    event: &CatalogEvent,
    difficulty: Difficulty,
    wired_vehicles: &mut BTreeSet<String>,
) -> RosterBuild {
    match opponent_roster(vfs, event, difficulty) {
        Ok(roster) => {
            for e in &roster.entries {
                wired_vehicles.insert(e.vehicle.clone());
            }
            let mut vehicles: Vec<String> = roster
                .entries
                .iter()
                .map(|e| e.vehicle.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            vehicles.sort();
            RosterBuild::Built(RosterSummary {
                wired: roster.entries.len(),
                routes: roster.resolved_routes(),
                table_opponents: event.race_params(difficulty).opponents,
                vehicles,
                issues: roster.issues,
            })
        }
        Err(RosterBuildError::CrashCourseUnsupported) => RosterBuild::Unsupported,
        Err(e) => RosterBuild::Failed(e),
    }
}

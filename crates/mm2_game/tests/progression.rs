//! F16-B progression unit tests: `apply_result` records finishes,
//! evaluates `half`/`all` milestones and indexed rewards against the
//! persisted `beaten` flags, grants unlocks idempotently, and
//! `record_eligibility` gates dev/modded sessions out of records.

use bevy::prelude::*;
use mm2_game::*;

fn key(stem: &str) -> EventKey {
    EventKey {
        city: "sf".to_string(),
        table: EventTableKind::Checkpoint,
        stem: stem.to_string(),
    }
}

fn profile() -> PlayerProfile {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path()).unwrap();
    store
        .create("driver", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap()
}

fn milestone(req: RewardRequirement, car: &str, variant: i64) -> RewardRule {
    RewardRule {
        family: EventTableKind::Checkpoint,
        requirement: req,
        unlock: if variant == 0 {
            Unlock::Vehicle(car.to_string())
        } else {
            Unlock::Paint {
                car: car.to_string(),
                variant,
            }
        },
        message: "unlocked".to_string(),
        line: 2,
    }
}

fn finished(ticks: u64) -> SessionOutcome {
    SessionOutcome::Finished { race_ticks: ticks }
}

/// Top-3 at Amateur counts the event beaten; 4th does not. At
/// Professional only a win counts (RACE-3/CHK-3).
#[test]
fn place_criterion_is_rank_dependent() {
    assert_eq!(place_requirement(Difficulty::Amateur), 3);
    assert_eq!(place_requirement(Difficulty::Professional), 1);

    let mut p = profile();
    let table = RewardTable::default();
    apply_result(
        &mut p,
        &key("race0"),
        &finished(100),
        Some(3),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.event(&key("race0")).unwrap().is_beaten());

    apply_result(
        &mut p,
        &key("race1"),
        &finished(100),
        Some(4),
        Difficulty::Amateur,
        &table,
    );
    let r = p.event(&key("race1")).unwrap();
    assert_eq!(r.finishes, 1);
    assert!(!r.is_beaten(), "4th at Amateur is not a win");

    apply_result(
        &mut p,
        &key("race2"),
        &finished(100),
        Some(2),
        Difficulty::Professional,
        &table,
    );
    let r = p.event(&key("race2")).unwrap();
    assert!(!r.beaten_amateur && !r.beaten_professional);
    assert!(!r.is_beaten(), "2nd at Professional is not a win");

    apply_result(
        &mut p,
        &key("race2"),
        &finished(90),
        Some(1),
        Difficulty::Professional,
        &table,
    );
    let r = p.event(&key("race2")).unwrap();
    assert!(r.beaten_professional);
    assert_eq!(r.finishes, 2);
    assert_eq!(r.best_race_ticks, Some(90));
    assert_eq!(r.best_place, Some(1));
}

/// A `TimedOut` result records nothing and grants nothing (F16-AC03).
#[test]
fn timeout_records_nothing() {
    let mut p = profile();
    let mut table = RewardTable::default();
    table
        .milestones
        .push(milestone(RewardRequirement::Half, "vpx", 0));
    table.family_sizes.insert(EventTableKind::Checkpoint, 2);

    let out = apply_result(
        &mut p,
        &key("race0"),
        &SessionOutcome::TimedOut { race_ticks: 60 },
        None,
        Difficulty::Amateur,
        &table,
    );
    assert_eq!(out, ApplyOutcome::default());
    assert!(p.event(&key("race0")).is_none());
    assert!(p.progress.unlocks.is_empty());
}

/// `half` grants once at least half the authored family is beaten;
/// `all` waits for every event. The denominators are the catalog's
/// authored counts — finishes in another city or family never count.
#[test]
fn milestones_count_beaten_events_in_the_family() {
    let mut p = profile();
    let mut table = RewardTable::default();
    table
        .milestones
        .push(milestone(RewardRequirement::Half, "vphalf", 0));
    table
        .milestones
        .push(milestone(RewardRequirement::All, "vpall", 0));
    table.family_sizes.insert(EventTableKind::Checkpoint, 4);

    // One beaten event of four — neither milestone met.
    apply_result(
        &mut p,
        &key("race0"),
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.progress.unlocks.is_empty());

    // A finish that did not meet the place criterion does not count.
    apply_result(
        &mut p,
        &key("race1"),
        &finished(100),
        Some(5),
        Difficulty::Amateur,
        &table,
    );
    assert!(!p.progress.unlocks.contains("vehicle:vphalf"));

    // A record in another city or another family is not this family's.
    let other_city = EventKey {
        city: "london".to_string(),
        ..key("race9")
    };
    apply_result(
        &mut p,
        &other_city,
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    let other_family = EventKey {
        table: EventTableKind::Circuit,
        ..key("circuit0")
    };
    apply_result(
        &mut p,
        &other_family,
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(!p.progress.unlocks.contains("vehicle:vphalf"));

    // Second beaten event of four = half → the half grant, not all.
    apply_result(
        &mut p,
        &key("race1"),
        &finished(90),
        Some(2),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.progress.unlocks.contains("vehicle:vphalf"));
    assert!(!p.progress.unlocks.contains("vehicle:vpall"));

    for stem in ["race2", "race3"] {
        apply_result(
            &mut p,
            &key(stem),
            &finished(100),
            Some(1),
            Difficulty::Amateur,
            &table,
        );
    }
    assert!(p.progress.unlocks.contains("vehicle:vpall"));
}

/// Indexed rows (`crash,N` in the authored table) bind to one event
/// and grant on the event being beaten — a plain finish that missed
/// the place criterion grants nothing.
#[test]
fn indexed_rewards_attach_to_their_event() {
    let mut p = profile();
    let crash = |stem: &str| EventKey {
        city: "sf".to_string(),
        table: EventTableKind::CrashCourse,
        stem: stem.to_string(),
    };
    let mut table = RewardTable::default();
    table.per_event.push((
        crash("crash3"),
        RewardRule {
            family: EventTableKind::CrashCourse,
            requirement: RewardRequirement::Event(3),
            unlock: Unlock::Vehicle("vpvwcup".to_string()),
            message: "passed".to_string(),
            line: 5,
        },
    ));

    // A different event's finish does not touch it.
    apply_result(
        &mut p,
        &crash("crash2"),
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.progress.unlocks.is_empty());

    // A finish below the criterion does not grant.
    apply_result(
        &mut p,
        &crash("crash3"),
        &finished(100),
        Some(4),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.progress.unlocks.is_empty());

    // Meeting the criterion on the bound event grants the unlock.
    let out = apply_result(
        &mut p,
        &crash("crash3"),
        &finished(90),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert_eq!(out.granted.len(), 1);
    assert!(p.progress.unlocks.contains("vehicle:vpvwcup"));
}

/// Re-applying a result — or finishing the event again later — never
/// re-reports a grant; the unlock set makes it idempotent (F16-AC02).
#[test]
fn repeat_application_grants_once() {
    let mut p = profile();
    let mut table = RewardTable::default();
    table.per_event.push((
        key("race0"),
        RewardRule {
            family: EventTableKind::Checkpoint,
            requirement: RewardRequirement::Event(0),
            unlock: Unlock::Vehicle("vpx".to_string()),
            message: "unlocked".to_string(),
            line: 2,
        },
    ));

    let first = apply_result(
        &mut p,
        &key("race0"),
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert_eq!(first.granted.len(), 1);
    // Same logical result re-delivered: still recorded (each delivery
    // is a distinct finish) but never re-granted.
    let again = apply_result(
        &mut p,
        &key("race0"),
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(again.granted.is_empty());
    // A later finish of the same event also does not re-grant.
    let later = apply_result(
        &mut p,
        &key("race0"),
        &finished(80),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(later.granted.is_empty());
    assert_eq!(p.progress.unlocks.len(), 1);
}

/// `VariantNum` 0 is the vehicle itself; anything else is a paint —
/// the authored header documents that split (VEH-3/VEH-4).
#[test]
fn variant_zero_is_the_car_nonzero_a_paint() {
    let mut p = profile();
    let mut table = RewardTable::default();
    table.per_event.push((
        key("race0"),
        RewardRule {
            family: EventTableKind::Checkpoint,
            requirement: RewardRequirement::Event(0),
            unlock: Unlock::Paint {
                car: "vpauditt".to_string(),
                variant: 4,
            },
            message: "paint".to_string(),
            line: 3,
        },
    ));
    apply_result(
        &mut p,
        &key("race0"),
        &finished(100),
        Some(1),
        Difficulty::Amateur,
        &table,
    );
    assert!(p.progress.unlocks.contains("paint:vpauditt:4"));
    assert!(!p.progress.unlocks.contains("vehicle:vpauditt"));
}

/// The DRV-6 gate: dev-world rigs, the synthetic dev car, gameplay
/// overrides and modded sessions produce no records — stock city runs
/// are eligible.
#[test]
fn record_eligibility_gates_dev_and_modded_sessions() {
    let mut config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/sf.psdl".to_string(),
        },
        vehicle: VehicleSelection {
            id: Some("vpbug".to_string()),
            paint: 0,
        },
        ..SessionConfig::default()
    };
    assert_eq!(record_eligibility(&config), Ok(()));

    config.world = WorldMode::DevWorld;
    assert_eq!(record_eligibility(&config), Err(Ineligible::World));
    config.world = WorldMode::City {
        psdl: "city/sf.psdl".to_string(),
    };

    config.vehicle.id = None;
    assert_eq!(record_eligibility(&config), Err(Ineligible::Vehicle));
    config.vehicle.id = Some("vpbug".to_string());

    config.mods_active = true;
    assert_eq!(record_eligibility(&config), Err(Ineligible::ModContent));
    config.mods_active = false;

    config.dev.traction = Some(0.5);
    assert_eq!(
        record_eligibility(&config),
        Err(Ineligible::DevOverride("traction"))
    );
    config.dev.traction = None;
    config.dev.spawn = Some(SpawnPose {
        position: Vec3::ZERO,
        yaw: 0.0,
    });
    assert_eq!(
        record_eligibility(&config),
        Err(Ineligible::DevOverride("spawn"))
    );
}

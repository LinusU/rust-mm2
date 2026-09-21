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

/// A table shaped like the authored rules: six checkpoint rows gated
/// in sets of three (CHK-2/CHK-3) plus one always-open blitz row.
fn availability_table() -> AvailabilityTable {
    let row = |stem: &str, gate: EventGate| AvailabilityRow {
        key: key(stem),
        gate,
    };
    let race = |n: usize| key(&format!("race{n}"));
    let crash = |n: usize| EventKey {
        table: EventTableKind::CrashCourse,
        ..key(&format!("crash{n}"))
    };
    AvailabilityTable {
        rows: vec![
            row("race0", EventGate::Open),
            row("race1", EventGate::Open),
            row("race2", EventGate::Open),
            row(
                "race3",
                EventGate::AfterAll(vec![race(0), race(1), race(2)]),
            ),
            row(
                "race4",
                EventGate::AfterAll(vec![race(0), race(1), race(2)]),
            ),
            row("race5", EventGate::AfterAll(vec![race(3), race(4)])),
            AvailabilityRow {
                key: crash(3),
                gate: EventGate::AfterAll(vec![crash(0), crash(1), crash(2)]),
            },
            AvailabilityRow {
                key: EventKey {
                    table: EventTableKind::Blitz,
                    ..key("blitz0")
                },
                gate: EventGate::Open,
            },
        ],
        diagnostics: Vec::new(),
    }
}

fn beat(profile: &mut PlayerProfile, stem: &str) {
    profile
        .event_mut(key(stem))
        .record_finish(100, Some(1), Difficulty::Amateur);
}

/// CHK-2/CHK-3: a fresh profile sees the first set open and every
/// later set locked behind its predecessor's beaten flags; beating a
/// set unlocks exactly the next one.
#[test]
fn checkpoint_sets_gate_on_the_previous_set() {
    let mut p = profile();
    let table = availability_table();

    let eval = table.evaluate(&p);
    let race0 = &eval[0];
    let race3 = &eval[3];
    assert!(race0.unlocked && !race0.customizable && race0.blocked_by.is_empty());
    assert!(!race3.unlocked);
    assert_eq!(race3.blocked_by.len(), 3, "the whole first set blocks");
    // The blitz row is open with no prerequisites at all.
    assert!(eval[7].unlocked);

    for stem in ["race0", "race1"] {
        beat(&mut p, stem);
    }
    let av = table.of(&p, &key("race3")).unwrap();
    assert!(!av.unlocked, "one un-beaten set member still locks");
    assert_eq!(
        av.blocked_by
            .iter()
            .map(|k| k.stem.as_str())
            .collect::<Vec<_>>(),
        ["race2"]
    );

    beat(&mut p, "race2");
    let eval = table.evaluate(&p);
    assert!(eval[3].unlocked && eval[4].unlocked, "set one opens");
    assert!(!eval[5].unlocked, "set two waits on set one");
    assert_eq!(
        eval[5]
            .blocked_by
            .iter()
            .map(|k| k.stem.as_str())
            .collect::<Vec<_>>(),
        ["race3", "race4"]
    );
}

/// RACE-3: meeting an event's win criterion opens its conditions
/// options — the flag tracks the event's own beaten record, nothing
/// else.
#[test]
fn a_beaten_event_opens_its_customization() {
    let mut p = profile();
    let table = availability_table();

    assert!(!table.of(&p, &key("race0")).unwrap().customizable);
    beat(&mut p, "race0");
    let av = table.of(&p, &key("race0")).unwrap();
    assert!(av.customizable);
    // An unbeaten neighbour is unaffected.
    assert!(!table.of(&p, &key("race1")).unwrap().customizable);
}

/// A crash-course midterm gate evaluates like any other: locked while
/// its lesson group is unbeaten.
#[test]
fn a_midterm_reports_its_lesson_group() {
    let mut p = profile();
    let table = availability_table();
    let crash3 = EventKey {
        table: EventTableKind::CrashCourse,
        ..key("crash3")
    };
    assert!(!table.of(&p, &crash3).unwrap().unlocked);
    for n in 0..3 {
        let k = EventKey {
            table: EventTableKind::CrashCourse,
            ..key(&format!("crash{n}"))
        };
        p.event_mut(k)
            .record_finish(100, Some(1), Difficulty::Amateur);
    }
    assert!(table.of(&p, &crash3).unwrap().unlocked);
}

/// A sandbox identity sees the unrestricted view — spec req 5's
/// developer access — without touching its (never-written) progress.
#[test]
fn a_sandbox_profile_sees_everything_unlocked() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path()).unwrap();
    let p = store
        .create("dev", Difficulty::Amateur, ProfileKind::Sandbox)
        .unwrap();
    let table = availability_table();
    assert!(
        table
            .evaluate(&p)
            .iter()
            .all(|a| a.unlocked && a.customizable && a.blocked_by.is_empty())
    );
}

/// A key the table does not cover reports `None` — the catalog never
/// produced a row for it.
#[test]
fn an_uncatalogued_key_has_no_availability() {
    let p = profile();
    let table = availability_table();
    assert!(table.of(&p, &key("race99")).is_none());
}

/// A garage surface: one open car, one reward-gated car whose last
/// paint is also gated, and one unlisted entry (no canonical `.info`
/// — the vpmoonrover case).
fn garage_table() -> GarageTable {
    GarageTable {
        rows: vec![
            GarageRow {
                id: "vpbug".to_string(),
                listed: true,
                gate: VehicleGate::Open,
                paint_gates: vec![PaintGate::Open; 4],
                unlock_score: 0,
                unlock_flags: 0,
            },
            GarageRow {
                id: "vpvwcup".to_string(),
                listed: true,
                gate: VehicleGate::Reward,
                paint_gates: vec![
                    PaintGate::Open,
                    PaintGate::Open,
                    PaintGate::Open,
                    PaintGate::Reward,
                ],
                unlock_score: 0,
                unlock_flags: 0,
            },
            GarageRow {
                id: "vpmoonrover".to_string(),
                listed: false,
                gate: VehicleGate::Open,
                paint_gates: vec![PaintGate::Open],
                unlock_score: 0,
                unlock_flags: 0,
            },
        ],
        diagnostics: Vec::new(),
    }
}

/// VEH-3/VEH-4: a fresh standard profile may select open vehicles but
/// not the reward-gated one — and a locked vehicle reports every paint
/// locked, gated or not.
#[test]
fn a_fresh_profile_sees_gates() {
    let p = profile();
    let table = garage_table();

    let bug = table.of(&p, "vpbug").unwrap();
    assert!(bug.unlocked);
    assert_eq!(bug.paints, vec![true; 4]);

    let cup = table.of(&p, "vpvwcup").unwrap();
    assert!(!cup.unlocked);
    assert_eq!(cup.paints, vec![false; 4], "a locked car opens nothing");

    // Unlisted entries still evaluate — roster membership is a
    // GarageRow fact, not part of the availability answer.
    assert!(table.of(&p, "vpmoonrover").unwrap().unlocked);
    assert!(!table.row("vpmoonrover").unwrap().listed);
}

/// Granting `vehicle:<id>` — the id an authored VariantNum-0 row
/// produces — opens the vehicle and its open paints, but not its
/// reward-gated ones.
#[test]
fn a_vehicle_unlock_opens_the_car_not_gated_paints() {
    let mut p = profile();
    let table = garage_table();
    p.progress.unlocks.insert("vehicle:vpvwcup".to_string());

    let cup = table.of(&p, "vpvwcup").unwrap();
    assert!(cup.unlocked);
    assert_eq!(cup.paints, vec![true, true, true, false]);
}

/// VEH-4: `paint:<id>:<variant>` opens exactly that paint index — the
/// vehicle itself still needs its own grant.
#[test]
fn a_paint_unlock_opens_exactly_that_index() {
    let mut p = profile();
    let table = garage_table();
    p.progress.unlocks.insert("vehicle:vpvwcup".to_string());
    p.progress.unlocks.insert("paint:vpvwcup:3".to_string());

    let cup = table.of(&p, "vpvwcup").unwrap();
    assert!(cup.unlocked);
    assert_eq!(cup.paints, vec![true, true, true, true]);
    // The paint grant does not leak onto another vehicle.
    assert!(!table.of(&p, "vpvwcup").unwrap().paints[0..3].contains(&false));
    assert_eq!(table.of(&p, "vpbug").unwrap().paints, vec![true; 4]);
}

/// Unlock ids the table never produced are inert — a hand-edited or
/// foreign save cannot open anything.
#[test]
fn unknown_unlock_ids_change_nothing() {
    let mut p = profile();
    let table = garage_table();
    p.progress.unlocks.insert("vehicle:vpzzz".to_string());
    p.progress.unlocks.insert("paint:vpvwcup:99".to_string());
    p.progress.unlocks.insert("nonsense".to_string());

    assert!(!table.of(&p, "vpvwcup").unwrap().unlocked);
    assert_eq!(
        table.of(&p, "vpbug").unwrap(),
        table.of(&profile(), "vpbug").unwrap()
    );
}

/// A sandbox identity sees every vehicle and paint — spec req 5's
/// unrestricted view applies to the garage too.
#[test]
fn a_sandbox_profile_selects_everything() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path()).unwrap();
    let p = store
        .create("dev", Difficulty::Amateur, ProfileKind::Sandbox)
        .unwrap();
    let table = garage_table();
    assert!(
        table
            .evaluate(&p)
            .iter()
            .all(|a| a.unlocked && a.paints.iter().all(|p| *p))
    );
}

/// An id the catalog never produced has no availability — `of`
/// reports `None`, and `evaluate` cannot invent a row.
#[test]
fn an_uncatalogued_vehicle_has_no_garage_row() {
    let p = profile();
    let table = garage_table();
    assert!(table.of(&p, "vpzzz").is_none());
    assert_eq!(table.evaluate(&p).len(), table.rows.len());
}

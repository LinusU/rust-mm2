//! F11-B race contract tests: swept trigger geometry, checkpoint-rule
//! progress bookkeeping and race-state lifecycle — the pure core the
//! `mm2_app` driver feeds positions to.

use bevy::prelude::*;
use mm2_game::*;

fn checkpoint(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

fn any_order(cps: Vec<Checkpoint>, finish: Option<Checkpoint>) -> RaceDefinition {
    RaceDefinition {
        checkpoints: cps,
        finish,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: DEFAULT_COUNTDOWN_TICKS,
        start_slots: Vec::new(),
    }
}

fn ordered(cps: Vec<Checkpoint>, laps: u32) -> RaceDefinition {
    RaceDefinition {
        checkpoints: cps,
        finish: None,
        rule: CheckpointRule::Ordered,
        laps,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: DEFAULT_COUNTDOWN_TICKS,
        start_slots: Vec::new(),
    }
}

/// AC02: a segment longer than the trigger still counts — the test is
/// swept, so speed cannot skip a checkpoint between fixed steps.
#[test]
fn swept_crossing_cannot_be_skipped_by_speed() {
    let cp = checkpoint(0.0, 0.0);
    // 400 m in one step — far past the 15 m radius.
    assert!(cp.crossed(Vec3::new(-200.0, 0.0, 0.0), Vec3::new(200.0, 0.0, 0.0)));
    // A near miss at the same speed does not count.
    assert!(!cp.crossed(Vec3::new(-200.0, 0.0, 20.0), Vec3::new(200.0, 0.0, 20.0)));
    // The radius boundary itself counts.
    assert!(cp.crossed(Vec3::new(-20.0, 0.0, 15.0), Vec3::new(20.0, 0.0, 15.0)));
    // A segment that starts already inside counts.
    assert!(cp.crossed(Vec3::new(1.0, 0.0, 1.0), Vec3::new(200.0, 0.0, 0.0)));
    // Parked on the checkpoint counts (the car *is* in the trigger).
    assert!(cp.crossed(Vec3::ZERO, Vec3::ZERO));
}

/// AC02: the trigger is a cylinder — passing over or under it at the
/// wrong height does not count.
#[test]
fn crossing_rejects_wrong_height() {
    let cp = checkpoint(0.0, 0.0);
    let from = Vec3::new(-50.0, DEFAULT_CHECKPOINT_HEIGHT + 5.0, 0.0);
    let to = Vec3::new(50.0, DEFAULT_CHECKPOINT_HEIGHT + 5.0, 0.0);
    assert!(!cp.crossed(from, to));
    // The same path at road height counts.
    assert!(cp.crossed(Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)));
    // A segment that enters the band mid-way counts (descending into it).
    assert!(cp.crossed(Vec3::new(-50.0, 30.0, 0.0), Vec3::new(0.0, 0.0, 0.0)));
}

/// The opt-in direction check rejects a crossing travelled backwards —
/// kept off by default (designed; no verified rule needs it).
#[test]
fn direction_flag_rejects_reverse_crossings() {
    let mut cp = checkpoint(0.0, 0.0);
    cp.heading_deg = 0.0; // forward = +Z
    cp.require_direction = true;
    let forward = (Vec3::new(0.0, 0.0, -30.0), Vec3::new(0.0, 0.0, 30.0));
    let backward = (Vec3::new(0.0, 0.0, 30.0), Vec3::new(0.0, 0.0, -30.0));
    assert!(cp.crossed(forward.0, forward.1));
    assert!(!cp.crossed(backward.0, backward.1));
}

/// Documented Blitz/Checkpoint rule (BLZ-1/CHK-1): any order, each
/// checkpoint clears exactly once.
#[test]
fn any_order_clears_independently_and_once() {
    let def = any_order(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(100.0, 0.0),
            checkpoint(200.0, 0.0),
        ],
        None,
    );
    def.validate().unwrap();
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    // Anchor, then clear the *last* checkpoint first — order is free.
    p.advance(&def, Vec3::new(200.0, 0.0, -1.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 1.0)),
        ProgressOutcome::Racing
    );
    assert!(p.is_cleared(2));
    assert_eq!(p.cleared_count(), 1);
    // Re-crossing an already-cleared checkpoint does not double count.
    p.advance(&def, Vec3::new(200.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(200.0, 0.0, 1.0));
    assert_eq!(p.crossings, 1);
    // Clear the rest out of order; the last one finishes (no separate
    // finish trigger). Repositioning between far-apart checkpoints is an
    // explicit segment break so each hop tests one trigger.
    p.break_segment();
    p.advance(&def, Vec3::new(0.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(0.0, 0.0, 1.0));
    assert!(p.is_cleared(0));
    p.break_segment();
    p.advance(&def, Vec3::new(100.0, 0.0, -1.0));
    assert_eq!(
        p.advance(&def, Vec3::new(100.0, 0.0, 1.0)),
        ProgressOutcome::Finished
    );
}

/// RACE-7: a separate finish trigger stays inert until every
/// checkpoint is cleared — including on the segment that clears the
/// last one.
#[test]
fn finish_trigger_waits_for_full_clearance() {
    let finish = checkpoint(500.0, 0.0);
    let def = any_order(vec![checkpoint(0.0, 0.0)], Some(finish));
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    // Cross the finish trigger before clearing the checkpoint: inert.
    p.advance(&def, Vec3::new(499.0, 0.0, -10.0));
    assert_eq!(
        p.advance(&def, Vec3::new(499.0, 0.0, 10.0)),
        ProgressOutcome::Racing
    );
    // Clear the checkpoint (repositioning is an explicit segment
    // break), then cross the finish for real.
    p.break_segment();
    p.advance(&def, Vec3::new(0.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(0.0, 0.0, 1.0));
    assert!(p.is_cleared(0));
    p.break_segment();
    p.advance(&def, Vec3::new(499.0, 0.0, -10.0));
    assert_eq!(
        p.advance(&def, Vec3::new(499.0, 0.0, 10.0)),
        ProgressOutcome::Finished
    );
}

/// Documented Circuit rule (CIR-1): strict authored order — crossing a
/// later checkpoint while an earlier is outstanding clears nothing.
#[test]
fn ordered_requires_sequence_and_wraps_laps() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(100.0, 0.0)], 2);
    def.validate().unwrap();
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    // Cross checkpoint 1 first: nothing clears.
    p.advance(&def, Vec3::new(99.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(99.0, 0.0, 1.0));
    assert_eq!(p.cleared_count(), 0);
    // Clear 0 then 1 — lap 1 completes, progress resets for lap 2.
    // Repositioning between far-apart checkpoints is an explicit
    // segment break, like a reset would be.
    p.break_segment();
    p.advance(&def, Vec3::new(0.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(0.0, 0.0, 1.0));
    assert!(p.is_cleared(0));
    p.break_segment();
    p.advance(&def, Vec3::new(99.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(99.0, 0.0, 1.0));
    assert_eq!(p.lap, 1);
    assert_eq!(p.next, 0);
    assert_eq!(p.cleared_count(), 0, "the new lap starts un-cleared");
    // Lap 2: the finish comes from the last required crossing.
    p.break_segment();
    p.advance(&def, Vec3::new(0.0, 0.0, -1.0));
    p.advance(&def, Vec3::new(0.0, 0.0, 1.0));
    p.break_segment();
    p.advance(&def, Vec3::new(99.0, 0.0, -1.0));
    assert_eq!(
        p.advance(&def, Vec3::new(99.0, 0.0, 1.0)),
        ProgressOutcome::Finished
    );
    assert_eq!(p.lap, 2);
}

/// AC02's skipped-in-one-tick edge: a single segment that crosses two
/// ordered checkpoints consumes both.
#[test]
fn one_segment_consumes_every_ordered_checkpoint_it_crosses() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(50.0, 0.0)], 1);
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    p.advance(&def, Vec3::new(-10.0, 0.0, 0.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Finished,
        "the segment crossed both checkpoints and finished"
    );
    assert_eq!(p.crossings, 2);
}

/// CIR-1's flip side: the gate that closes a lap only counts once per
/// completed sequence. Repeated line crossings while earlier gates are
/// outstanding clear nothing, and crossings after the wrap still aim
/// at the first gate of the new lap.
#[test]
fn the_lap_line_only_closes_a_completed_sequence() {
    // Gates A/B plus the start-line copy at C — the producer's circuit
    // shape (rows[1..] + lifted row0 last).
    let def = ordered(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(100.0, 0.0),
            checkpoint(200.0, 0.0),
        ],
        2,
    );
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    // Oscillate on the closing gate before any sequence progress:
    // repeated finish-line hits without the required gates are inert.
    for z in [-10.0, 10.0, -10.0, 10.0] {
        p.advance(&def, Vec3::new(200.0, 0.0, z));
    }
    assert_eq!(p.cleared_count(), 0);
    assert_eq!(p.lap, 0);
    assert_eq!(p.crossings, 0);
    // A complete sequence closes lap 1 exactly once.
    for x in [0.0, 100.0, 200.0] {
        p.break_segment();
        p.advance(&def, Vec3::new(x, 0.0, -10.0));
        p.advance(&def, Vec3::new(x, 0.0, 10.0));
    }
    assert_eq!(p.lap, 1);
    assert_eq!(p.crossings, 3);
    // Bouncing on the line again clears nothing — the new lap's first
    // required gate is A, not the line.
    for z in [-10.0, 10.0, -10.0] {
        p.advance(&def, Vec3::new(200.0, 0.0, z));
    }
    assert_eq!(p.lap, 1);
    assert_eq!(p.crossings, 3);
    // Lap 2's sequence finishes the race on the line crossing.
    for x in [0.0, 100.0] {
        p.break_segment();
        p.advance(&def, Vec3::new(x, 0.0, -10.0));
        p.advance(&def, Vec3::new(x, 0.0, 10.0));
    }
    p.break_segment();
    p.advance(&def, Vec3::new(200.0, 0.0, -10.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 10.0)),
        ProgressOutcome::Finished
    );
    assert_eq!(p.lap, 2);
}

/// AC04's once-only rule is a contract property, not a caller
/// discipline: `advance` is inert outside `Racing`. An `AwaitingStart`
/// participant's steps only re-anchor the segment, and a `Finished`
/// participant fed more positions can never re-emit a finish or clear
/// another gate.
#[test]
fn a_participant_outside_racing_cannot_advance() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(50.0, 0.0)], 1);
    // AwaitingStart: a creep across both gates before release clears
    // nothing — and the re-anchor means the post-release step is not
    // treated as a giant sweep either.
    let mut p = RaceProgress::new(&def);
    p.advance(&def, Vec3::new(-10.0, 0.0, 0.0));
    assert_eq!(
        p.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Racing
    );
    assert_eq!(p.cleared_count(), 0);
    assert_eq!(p.crossings, 0);
    // Finished: repeated finish hits stay inert (F14-AC02's "repeated
    // finish hits do not accumulate laps").
    let mut q = RaceProgress::new(&def);
    q.state = ParticipantState::Finished {
        race_ticks: 10,
        result: ResultId {
            generation: 1,
            participant: PlayerId(0),
            event: None,
            sequence: 0,
        },
    };
    q.advance(&def, Vec3::new(-10.0, 0.0, 0.0));
    assert_eq!(
        q.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Racing,
        "a resolved participant cannot re-finish"
    );
    assert_eq!(q.cleared_count(), 0);
    assert_eq!(q.lap, 0);
}

/// Teleport/reset handling: `break_segment` re-anchors so the jump
/// cannot clear checkpoints the car skipped.
#[test]
fn break_segment_reanchors_after_teleport() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(50.0, 0.0)], 1);
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    p.advance(&def, Vec3::new(-100.0, 0.0, 0.0));
    // Teleport past both checkpoints *with* the segment broken: the
    // first step re-anchors, nothing is consumed.
    p.break_segment();
    p.advance(&def, Vec3::new(200.0, 0.0, 0.0));
    assert_eq!(p.cleared_count(), 0);
    // Sanity: without the break, that jump would have finished.
    let mut q = RaceProgress::new(&def);
    q.state = ParticipantState::Racing;
    q.advance(&def, Vec3::new(-100.0, 0.0, 0.0));
    assert_eq!(
        q.advance(&def, Vec3::new(200.0, 0.0, 0.0)),
        ProgressOutcome::Finished
    );
}

/// A race resource stamped by an older session is stale — teardown's
/// resource removal plus this guard keep an old timer out of a new
/// session (AC03).
#[test]
fn race_state_tracks_generation_and_lock() {
    let def = ordered(vec![checkpoint(0.0, 0.0)], 1);
    let race = RaceState::new(def, 7);
    assert!(race.input_locked());
    assert!(!race.is_stale(7));
    assert!(race.is_stale(8));
    let mut running = race;
    running.phase = RacePhase::Running;
    assert!(!running.input_locked());
}

/// Late join: `join` starts racing immediately when the race is already
/// running — no countdown replay for a joiner.
#[test]
fn late_join_starts_racing() {
    let def = ordered(vec![checkpoint(0.0, 0.0)], 1);
    let mut race = RaceState::new(def.clone(), 1);
    let waiting = RaceProgress::join(&def, &race);
    assert_eq!(waiting.state, ParticipantState::AwaitingStart);
    race.phase = RacePhase::Running;
    let joined = RaceProgress::join(&def, &race);
    assert_eq!(joined.state, ParticipantState::Racing);
}

/// The time limit counts down on the race clock — `None` for untimed
/// definitions, saturating at zero past the deadline.
#[test]
fn time_remaining_counts_down_and_saturates() {
    let mut def = any_order(vec![checkpoint(0.0, 0.0)], None);
    def.time_limit_ticks = Some(100);
    let mut race = RaceState::new(def, 1);
    assert_eq!(race.time_remaining(), Some(100));
    race.clock = 40;
    assert_eq!(race.time_remaining(), Some(60));
    race.clock = 100;
    assert_eq!(race.time_remaining(), Some(0));
    race.clock = 150;
    assert_eq!(race.time_remaining(), Some(0), "past expiry stays 0");
    let untimed = RaceState::new(any_order(vec![checkpoint(0.0, 0.0)], None), 1);
    assert_eq!(untimed.time_remaining(), None);
}

/// Validation rejects definitions that would misbehave at runtime.
#[test]
fn definition_validation_rejects_unusable_shapes() {
    assert_eq!(
        any_order(Vec::new(), None).validate(),
        Err(RaceError::NoCheckpoints)
    );
    assert_eq!(
        ordered(vec![checkpoint(0.0, 0.0)], 0).validate(),
        Err(RaceError::NoLaps)
    );
    let mut flat = checkpoint(0.0, 0.0);
    flat.radius = 0.0;
    assert_eq!(
        any_order(vec![flat], None).validate(),
        Err(RaceError::BadExtent)
    );
    let mut instant = any_order(vec![checkpoint(0.0, 0.0)], None);
    instant.time_limit_ticks = Some(0);
    assert_eq!(instant.validate(), Err(RaceError::BadTimeLimit));
}

/// `course_yaw` derives the course-facing a headless authored slot
/// (`yaw_deg == None`) falls back to: the first trigger far enough
/// away to define a direction, as a vehicle-yaw heading — the same
/// facing the producer's no-grid tangent fallback derives.
#[test]
fn course_yaw_faces_the_first_real_trigger() {
    // Course running +X, like the producer's row0→row1 tangent.
    let def = ordered(vec![checkpoint(110.0, 0.0), checkpoint(200.0, 0.0)], 1);
    let yaw = def.course_yaw(Vec3::new(60.0, 0.0, 0.0)).unwrap();
    let fwd = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    assert!(fwd.x > 0.98, "faces +X toward the first gate: {fwd:?}");

    // A trigger too close to define a direction is skipped — the
    // start-line copy sits on the slot, the next real gate decides.
    let def = ordered(
        vec![
            checkpoint(60.0, 0.0),
            checkpoint(60.0, 0.0),
            checkpoint(60.0, -100.0),
        ],
        1,
    );
    let yaw = def.course_yaw(Vec3::new(60.0, 0.0, 0.0)).unwrap();
    let fwd = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    assert!(fwd.z < -0.98, "skips the on-slot trigger: {fwd:?}");

    // Every trigger on the slot — degenerate authored data, no course.
    let def = ordered(vec![checkpoint(60.0, 0.0)], 1);
    assert_eq!(def.course_yaw(Vec3::new(60.0, 0.0, 0.0)), None);
}

/// RACE-6: with no pick the arrow tracks the nearest un-cleared gate —
/// XZ distance, not the lowest index.
#[test]
fn arrow_defaults_to_nearest_uncleared_gate() {
    let def = any_order(
        vec![
            checkpoint(0.0, 100.0),
            checkpoint(300.0, 0.0),
            checkpoint(0.0, -60.0),
        ],
        Some(checkpoint(0.0, 0.0)),
    );
    let p = RaceProgress::new(&def);
    assert_eq!(
        navigation_target(&def, &p, None, Vec3::ZERO),
        Some(NavTarget::Gate(2)),
        "gate 2 at (0,-60) is nearer than gate 0 at (0,100)"
    );
}

/// RACE-6: an explicit pick wins over the nearest gate; once that gate
/// is cleared the arrow falls back to the nearest remaining one.
#[test]
fn arrow_pick_wins_until_its_gate_is_cleared() {
    let def = any_order(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(100.0, 0.0),
            checkpoint(200.0, 0.0),
        ],
        None,
    );
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    let at = Vec3::new(-50.0, 0.0, 0.0);
    // Gate 0 is nearest but the pick aims at gate 2.
    assert_eq!(
        navigation_target(&def, &p, Some(2), at),
        Some(NavTarget::Gate(2))
    );
    // Clear the picked gate — the arrow must not keep aiming at it.
    p.advance(&def, Vec3::new(199.0, 0.0, -10.0));
    p.advance(&def, Vec3::new(199.0, 0.0, 10.0));
    assert!(p.is_cleared(2));
    assert_eq!(
        navigation_target(&def, &p, Some(2), at),
        Some(NavTarget::Gate(0)),
        "a cleared pick falls back to the nearest remaining gate"
    );
    // An out-of-range pick never aims at anything invalid either.
    assert_eq!(
        navigation_target(&def, &p, Some(9), at),
        Some(NavTarget::Gate(0))
    );
}

/// RACE-6 (inferred leg): once every gate is cleared the armed finish
/// is the remaining objective — the arrow points at it while the
/// definition has a finish trigger.
#[test]
fn arrow_tracks_the_armed_finish() {
    let finish = checkpoint(500.0, 0.0);
    let def = any_order(vec![checkpoint(0.0, 0.0)], Some(finish));
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    p.advance(&def, Vec3::new(-10.0, 0.0, 0.0));
    p.advance(&def, Vec3::new(10.0, 0.0, 0.0));
    assert!(p.is_cleared(0));
    let at = Vec3::new(300.0, 0.0, 0.0);
    assert_eq!(
        navigation_target(&def, &p, None, at),
        Some(NavTarget::Finish)
    );
    assert_eq!(
        NavTarget::Finish.position(&def),
        Some(Vec3::new(500.0, 0.0, 0.0)),
        "the finish target resolves to the trigger's position"
    );
    // Without a finish trigger there is nothing left to aim at.
    let no_finish = any_order(vec![checkpoint(0.0, 0.0)], None);
    let mut q = RaceProgress::new(&no_finish);
    q.state = ParticipantState::Racing;
    q.advance(&no_finish, Vec3::new(-10.0, 0.0, 0.0));
    q.advance(&no_finish, Vec3::new(10.0, 0.0, 0.0));
    assert_eq!(navigation_target(&no_finish, &q, None, at), None);
}

/// HUD-2 scopes the compass arrow to Blitz/Checkpoint — an `Ordered`
/// (Circuit) definition reports no target.
#[test]
fn ordered_definitions_have_no_arrow() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(100.0, 0.0)], 2);
    let p = RaceProgress::new(&def);
    assert_eq!(navigation_target(&def, &p, None, Vec3::ZERO), None);
}

/// RACE-6: cycling walks the remaining gates in authored order,
/// wrapping at both ends and skipping cleared gates.
#[test]
fn cycling_walks_remaining_gates_and_wraps() {
    let def = any_order(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(100.0, 0.0),
            checkpoint(200.0, 0.0),
        ],
        None,
    );
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    let at = Vec3::new(-50.0, 0.0, 0.0);
    // No pick yet: the first step moves off the nearest gate (0).
    let mut pick = cycle_target(&def, &p, None, at, 1);
    assert_eq!(pick, Some(1));
    pick = cycle_target(&def, &p, pick, at, 1);
    assert_eq!(pick, Some(2));
    pick = cycle_target(&def, &p, pick, at, 1);
    assert_eq!(pick, Some(0), "forward cycling wraps");
    pick = cycle_target(&def, &p, pick, at, -1);
    assert_eq!(pick, Some(2), "backward cycling wraps the other way");

    // A cleared gate is skipped in both directions.
    p.advance(&def, Vec3::new(99.0, 0.0, -10.0));
    p.advance(&def, Vec3::new(99.0, 0.0, 10.0));
    assert!(p.is_cleared(1));
    assert_eq!(cycle_target(&def, &p, Some(0), at, 1), Some(2));
    assert_eq!(cycle_target(&def, &p, Some(0), at, -1), Some(2));

    // Nothing left to aim at clears the pick.
    p.advance(&def, Vec3::new(-10.0, 0.0, 0.0));
    p.advance(&def, Vec3::new(10.0, 0.0, 0.0));
    p.advance(&def, Vec3::new(199.0, 0.0, -10.0));
    p.advance(&def, Vec3::new(199.0, 0.0, 10.0));
    assert_eq!(cycle_target(&def, &p, Some(0), at, 1), None);
}

/// The bearing is signed on the driver's frame: `+` right, `−` left,
/// `±π` behind — on the `Quat::from_rotation_y` yaw convention
/// (forward = −Z at yaw 0).
#[test]
fn relative_bearing_is_signed_on_the_driver_frame() {
    use std::f32::consts::{FRAC_PI_2, PI};
    let at = Vec3::ZERO;
    // Facing −Z (yaw 0): +X dead right, −X dead left.
    assert!((relative_bearing(0.0, at, Vec3::new(10.0, 0.0, 0.0)) - FRAC_PI_2).abs() < 1e-4);
    assert!((relative_bearing(0.0, at, Vec3::new(-10.0, 0.0, 0.0)) + FRAC_PI_2).abs() < 1e-4);
    // Ahead ≈ 0, behind ≈ ±π.
    assert!(relative_bearing(0.0, at, Vec3::new(0.0, 0.0, -10.0)).abs() < 1e-4);
    assert!((relative_bearing(0.0, at, Vec3::new(0.0, 0.0, 10.0)) - PI).abs() < 1e-4);
    // Rotated 90° (facing −X): a −Z target is dead right now.
    assert!((relative_bearing(FRAC_PI_2, at, Vec3::new(0.0, 0.0, -10.0)) - FRAC_PI_2).abs() < 1e-4);
    // Height does not rotate the needle — bearing is ground-plane.
    assert!(relative_bearing(0.0, at, Vec3::new(0.0, 50.0, -10.0)).abs() < 1e-4);
    // A coincident target reports 0, never NaN.
    assert_eq!(relative_bearing(0.0, at, at), 0.0);
}

// ---------- catch-up assist contract (F15-B.4, designed DSN-27) ----------

/// The leg scale is the course's own authored spacing — the mean of
/// consecutive gate centres — and `None` when nothing is measurable,
/// so the caller falls back to a designed reference.
#[test]
fn mean_gate_spacing_averages_consecutive_gates() {
    let def = any_order(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(50.0, 0.0),
            checkpoint(130.0, 0.0),
        ],
        None,
    );
    assert_eq!(mean_gate_spacing(&def), Some(65.0));

    let one = any_order(vec![checkpoint(0.0, 0.0)], None);
    assert_eq!(mean_gate_spacing(&one), None, "a lone gate has no leg");

    // A coincident gate pair contributes nothing measurable.
    let dup = any_order(vec![checkpoint(0.0, 0.0), checkpoint(0.0, 0.0)], None);
    assert_eq!(mean_gate_spacing(&dup), None);
}

/// `Ordered`: the score banks `lap × gates + next` plus the covered
/// fraction of the leg toward `checkpoints[next]` — so a follower
/// trails the leader by exactly the deficit the factor reads.
#[test]
fn course_progress_ordered_banks_laps_and_the_leg_fraction() {
    let def = ordered(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(50.0, 0.0),
            checkpoint(100.0, 0.0),
        ],
        2,
    );
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;

    // Mid-leg toward gate 1: banked 1 (gate 0 cleared → next=1)... set
    // the bookkeeping directly — the measure reads state, not motion.
    p.next = 1;
    let s = course_progress(&def, &p, Vec3::new(25.0, 0.0, 0.0), 50.0);
    assert!((s - 1.5).abs() < 1e-4, "1 banked + half the leg: {s}");
    // On the gate the whole leg is banked.
    let s = course_progress(&def, &p, Vec3::new(50.0, 0.0, 0.0), 50.0);
    assert!((s - 2.0).abs() < 1e-4, "on gate 1: {s}");

    // Lap boundary: lap 1 banks all three gates of lap 0; a car 10 m
    // short of the start-line copy adds 0.8 of the next leg.
    p.lap = 1;
    p.next = 0;
    let s = course_progress(&def, &p, Vec3::new(-10.0, 0.0, 0.0), 50.0);
    assert!((s - 3.8).abs() < 1e-4, "lap banked + 40/50 of the leg: {s}");

    // A second participant a leg and a fraction further on leads by
    // the deficit the assist ramps on: banked 3+2 + 0.8 = 5.8 vs 3.8.
    let mut q = p.clone();
    q.next = 2;
    let leader = course_progress(&def, &q, Vec3::new(90.0, 0.0, 0.0), 50.0);
    assert!((leader - s - 2.0).abs() < 1e-4, "leader − follower deficit");
}

/// `AnyOrder`: the score banks the cleared count plus the covered
/// fraction toward the `navigation_target` objective — the same
/// nearest remaining gate (then the armed finish) the arrow tracks.
#[test]
fn course_progress_any_order_banks_cleared_gates() {
    let def = any_order(
        vec![
            checkpoint(0.0, 0.0),
            checkpoint(50.0, 0.0),
            checkpoint(100.0, 0.0),
        ],
        Some(checkpoint(150.0, 0.0)),
    );
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    p.advance(&def, Vec3::new(-40.0, 0.0, 0.0));
    p.advance(&def, Vec3::new(10.0, 0.0, 0.0)); // sweeps gate 0
    assert_eq!(p.cleared_count(), 1);

    // Gate 1 at x=50 is the nearest remaining objective — 20 m away
    // banks 0.6 of a leg.
    let s = course_progress(&def, &p, Vec3::new(30.0, 0.0, 0.0), 50.0);
    assert!((s - 1.6).abs() < 1e-4, "1 cleared + 30/50 of the leg: {s}");

    // Every gate cleared → the finish is the objective; standing on
    // it banks the whole course.
    for to in [Vec3::new(60.0, 0.0, 0.0), Vec3::new(110.0, 0.0, 0.0)] {
        p.advance(&def, to);
    }
    assert_eq!(p.cleared_count(), 3);
    let s = course_progress(&def, &p, Vec3::new(140.0, 0.0, 0.0), 50.0);
    assert!(
        (s - 3.8).abs() < 1e-4,
        "3 cleared + 40/50 to the finish: {s}"
    );
}

/// The measure stays finite on every degenerate input — an
/// out-of-range `next`, a missing objective, a non-finite position or
/// a dead `leg_ref` contributes the banked count alone.
#[test]
fn course_progress_degenerate_inputs_stay_finite() {
    let def = ordered(vec![checkpoint(0.0, 0.0), checkpoint(50.0, 0.0)], 1);
    let mut p = RaceProgress::new(&def);
    p.state = ParticipantState::Racing;
    p.next = 9; // ran past the gate list — no current objective; the
    // banked count clamps to the definition's gate total.
    let s = course_progress(&def, &p, Vec3::ZERO, 50.0);
    assert_eq!(s, 2.0, "banked gates only: {s}");

    p.next = 0;
    for pos in [Vec3::NAN, Vec3::INFINITY] {
        let s = course_progress(&def, &p, pos, 50.0);
        assert_eq!(s, 0.0, "non-finite position → banked only: {s}");
    }
    for leg_ref in [0.0, -5.0, f32::NAN, f32::INFINITY] {
        let s = course_progress(&def, &p, Vec3::new(25.0, 0.0, 0.0), leg_ref);
        assert_eq!(s, 0.0, "dead leg_ref {leg_ref} → banked only: {s}");
    }

    // A definition with no checkpoints scores zero everywhere.
    let empty = ordered(Vec::new(), 1);
    assert_eq!(
        course_progress(&empty, &p, Vec3::ZERO, 50.0),
        0.0,
        "nothing to bank"
    );
}

/// The factor ramps from 0 at the lead to `assist_max` at
/// `deficit_full` and clamps — one-directional: an at-or-ahead driver
/// is never lifted and never slowed, and garbage in assists nothing.
#[test]
fn catch_up_factor_is_bounded_one_directional_and_safe() {
    let policy = CatchUpPolicy::default();
    assert_eq!(catch_up_factor(0.0, &policy), 0.0, "level with the lead");
    assert_eq!(catch_up_factor(-3.0, &policy), 0.0, "leading earns nothing");
    let half = catch_up_factor(1.0, &policy);
    assert!(
        (half - 0.125).abs() < 1e-4,
        "half the deficit, half the lift: {half}"
    );
    assert_eq!(
        catch_up_factor(policy.deficit_full, &policy),
        policy.assist_max,
        "saturates at the bound"
    );
    assert_eq!(
        catch_up_factor(99.0, &policy),
        policy.assist_max,
        "past the bound stays at the bound"
    );
    for deficit in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(catch_up_factor(deficit, &policy), 0.0);
    }
    // A dead policy cannot divide into NaN or lift anyone.
    let dead = CatchUpPolicy {
        deficit_full: 0.0,
        assist_max: 1.0,
    };
    assert_eq!(catch_up_factor(5.0, &dead), 0.0);
    let negative = CatchUpPolicy {
        deficit_full: -1.0,
        assist_max: 1.0,
    };
    assert_eq!(catch_up_factor(5.0, &negative), 0.0);
}

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
}

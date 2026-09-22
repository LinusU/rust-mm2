//! F05-B.5 recovery-contract tests: the designed water/OOB detector —
//! dry-grounded anchoring, the submersion dwell, the bounded fall leg
//! and the episode-clearing `recovered`.

use bevy::prelude::*;
use mm2_game::*;

const POLICY: RecoveryPolicy = RecoveryPolicy {
    water_min_drag: 0.3,
    submerge_dwell: 0.5,
    fall_margin: 8.0,
};
const DRY: Vec3 = Vec3::new(10.0, 0.6, -4.0);
const YAW: f32 = 0.4;
const DT: f32 = 1.0 / 120.0;

fn anchored() -> VehicleRecovery {
    VehicleRecovery::with_anchor(POLICY, DRY, YAW)
}

fn recover(v: &RecoveryVerdict) -> (RecoveryCause, Option<(Vec3, f32)>) {
    match v {
        RecoveryVerdict::Recover { cause, landing } => (*cause, *landing),
        RecoveryVerdict::Clear => panic!("expected a recovery, got Clear"),
    }
}

#[test]
fn a_dry_contact_anchors_the_car_and_stays_clear() {
    let mut det = VehicleRecovery::new(POLICY);
    assert_eq!(det.anchor(), None);
    for pos in [Vec3::new(0.0, 0.6, 0.0), Vec3::new(1.0, 0.6, 0.0)] {
        assert_eq!(
            det.observe(pos, 0.0, GroundContact::Dry, DT),
            RecoveryVerdict::Clear
        );
    }
    // The anchor tracks the *latest* dry pose — recovery lands where
    // the car last had purchase, not where it spawned.
    assert_eq!(det.anchor(), Some((Vec3::new(1.0, 0.6, 0.0), 0.0)));
}

#[test]
fn submersion_fires_once_per_dwell() {
    let mut det = anchored();
    // 0.5 s at 120 Hz = 60 steps; the fire edge tolerates the float
    // residue (same rule the stuck window uses), so collect the window
    // rather than pin an exact step.
    let verdicts: Vec<_> = (0..70)
        .map(|_| det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, DT))
        .collect();
    let fires: Vec<_> = verdicts
        .iter()
        .filter(|v| matches!(v, RecoveryVerdict::Recover { .. }))
        .collect();
    assert_eq!(fires.len(), 1, "exactly one fire in a 0.58 s window");
    let (cause, landing) = recover(fires[0]);
    assert_eq!(cause, RecoveryCause::Submerged);
    assert_eq!(landing, Some((DRY, YAW)), "the landing is the dry anchor");
    // One fire per full dwell — still submerged needs another ~60
    // steps, not a burst every step.
    let verdicts: Vec<_> = (0..70)
        .map(|_| det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, DT))
        .collect();
    assert_eq!(
        verdicts
            .iter()
            .filter(|v| matches!(v, RecoveryVerdict::Recover { .. }))
            .count(),
        1,
        "a fresh dwell produces exactly one more fire"
    );
}

#[test]
fn reaching_dry_ground_inside_the_dwell_escapes() {
    let mut det = anchored();
    for _ in 0..40 {
        det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, DT);
    }
    assert!(det.submerged_for() > 0.0);
    // A dry contact inside the window re-anchors and zeroes the dwell —
    // the car powered back onto the shore.
    let shore = DRY + Vec3::new(3.0, 0.0, 0.0);
    assert_eq!(
        det.observe(shore, YAW, GroundContact::Dry, DT),
        RecoveryVerdict::Clear
    );
    assert_eq!(det.anchor(), Some((shore, YAW)));
    assert_eq!(det.submerged_for(), 0.0);
    // The next submersion counts a full fresh dwell — 50 steps is
    // under the 0.5 s bound on any float residue.
    for _ in 0..50 {
        assert_eq!(
            det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, DT),
            RecoveryVerdict::Clear
        );
    }
}

#[test]
fn a_spawn_straight_onto_water_reports_no_landing() {
    // `--spawn` over water never touched dry ground: the detector
    // still fires but leaves the landing to the resolver's spawn
    // fallback.
    let mut det = VehicleRecovery::new(POLICY);
    let fired = (0..70)
        .map(|_| det.observe(DRY, YAW, GroundContact::Submerged, DT))
        .find(|v| matches!(v, RecoveryVerdict::Recover { .. }))
        .expect("the dwell must fire within 70 steps");
    let (cause, landing) = recover(&fired);
    assert_eq!(cause, RecoveryCause::Submerged);
    assert_eq!(landing, None);
}

#[test]
fn a_fall_past_the_margin_fires_once_until_grounded() {
    let mut det = anchored();
    // Inside the margin an airborne car is just jumping.
    assert_eq!(
        det.observe(DRY - Vec3::Y * 7.0, YAW, GroundContact::Airborne, DT),
        RecoveryVerdict::Clear
    );
    // Past it, one bounded fire — then the latch holds the stream at
    // one event while the car keeps falling and the reset travels.
    let (cause, landing) =
        recover(&det.observe(DRY - Vec3::Y * 9.0, YAW, GroundContact::Airborne, DT));
    assert_eq!(cause, RecoveryCause::OutOfBounds);
    assert_eq!(landing, Some((DRY, YAW)));
    assert_eq!(
        det.observe(DRY - Vec3::Y * 20.0, YAW, GroundContact::Airborne, DT),
        RecoveryVerdict::Clear,
        "latched: a continuing fall does not emit per step"
    );
    // Touching down — even on water — re-arms the leg.
    det.observe(DRY - Vec3::Y * 20.0, YAW, GroundContact::Submerged, DT);
    let (cause, _) = recover(&det.observe(DRY - Vec3::Y * 30.0, YAW, GroundContact::Airborne, DT));
    assert_eq!(cause, RecoveryCause::OutOfBounds);
}

#[test]
fn a_legitimate_drop_lands_before_the_margin() {
    // A real landing refreshes the anchor — the margin follows the
    // car down, so a big-but-real drop never fires.
    let mut det = anchored();
    det.observe(DRY - Vec3::Y * 3.0, YAW, GroundContact::Airborne, DT);
    let lower = DRY - Vec3::Y * 12.0;
    assert_eq!(
        det.observe(lower, YAW, GroundContact::Dry, DT),
        RecoveryVerdict::Clear
    );
    assert_eq!(det.anchor(), Some((lower, YAW)));
    // Falling further from the new anchor is measured against it.
    assert_eq!(
        det.observe(lower - Vec3::Y * 7.0, YAW, GroundContact::Airborne, DT),
        RecoveryVerdict::Clear
    );
}

#[test]
fn a_non_finite_pose_recovers_to_the_anchor_at_once() {
    let mut det = anchored();
    let (cause, landing) = recover(&det.observe(Vec3::NAN, YAW, GroundContact::Airborne, DT));
    assert_eq!(cause, RecoveryCause::OutOfBounds);
    assert_eq!(landing, Some((DRY, YAW)));
    // No latch window for NaN — while the pose stays broken it keeps
    // asking (the resolver's reset is what ends it).
    let (cause, _) = recover(&det.observe(Vec3::NAN, YAW, GroundContact::Dry, DT));
    assert_eq!(cause, RecoveryCause::OutOfBounds);
}

#[test]
fn recovered_clears_the_episode_and_reanchors() {
    let mut det = anchored();
    // Fire a submersion, then land it somewhere new.
    for _ in 0..70 {
        det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, DT);
    }
    let bank = Vec3::new(-6.0, 0.8, 3.0);
    det.recovered(bank, 1.2);
    assert_eq!(det.anchor(), Some((bank, 1.2)));
    assert_eq!(det.submerged_for(), 0.0);
    // A dry contact after the recovery is ordinary grounding.
    assert_eq!(
        det.observe(bank, 1.2, GroundContact::Dry, DT),
        RecoveryVerdict::Clear
    );
}

#[test]
fn garbage_dt_and_zero_dwell_edges() {
    let mut det = anchored();
    // Non-finite/negative dt never accrues.
    for dt in [f32::NAN, -1.0, 0.0] {
        assert_eq!(
            det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, dt),
            RecoveryVerdict::Clear
        );
    }
    assert_eq!(det.submerged_for(), 0.0);
    // A zero dwell fires on the first submerged step — degenerate
    // policy, immediate recovery.
    det.policy.submerge_dwell = 0.0;
    let (cause, _) = recover(&det.observe(DRY + Vec3::X, YAW, GroundContact::Submerged, 0.0));
    assert_eq!(cause, RecoveryCause::Submerged);
}

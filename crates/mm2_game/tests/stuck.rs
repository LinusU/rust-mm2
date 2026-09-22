//! F05-B.2 contract tests: the authored `vehstuck` detector — armed by
//! an impact, hysteresis on the pos/move bounds, the `turn` tumbling
//! leg, the `time_thresh` fire and the one-shot disarm.

use bevy::prelude::*;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::VehStuck;
use mm2_game::{StuckSpec, StuckVerdict, VehicleStuck};

/// A mid-roster retail-shaped spec (pos/move thresholds are uniform on
/// stock; time/turn vary).
const SPEC: StuckSpec = StuckSpec {
    time_thresh: 1.0,
    pos_thresh: 1.25,
    move_thresh: 1.75,
    turn: std::f32::consts::PI,
    rotation: 0.0,
    translation: 0.1,
};

fn tune_stuck(turn: f32, time: f32) -> VehStuck {
    let text = format!(
        "vehStuck {{\n Turn {turn}\n Rotation 0.0\n Translation 0.1\n TimeThresh {time}\n PosThresh 1.25\n MoveThresh 1.75\n}}"
    );
    let file = TuneFile::parse(&text).unwrap();
    VehStuck::from_tune(&file).unwrap()
}

#[test]
fn the_spec_carries_the_authored_fields_verbatim() {
    // vpford's authored turn bound and vpddbus's authored window.
    let s = tune_stuck(3.098593, 1.1714);
    let spec = StuckSpec::from(&s);
    assert_eq!(spec.turn, 3.098593);
    assert_eq!(spec.rotation, 0.0);
    assert_eq!(spec.translation, 0.1);
    assert_eq!(spec.time_thresh, 1.1714);
    assert_eq!(spec.pos_thresh, 1.25);
    assert_eq!(spec.move_thresh, 1.75);
}

#[test]
fn an_unarmed_detector_never_fires() {
    let mut stuck = VehicleStuck::new(SPEC);
    for _ in 0..1000 {
        assert_eq!(
            stuck.observe(Vec3::ZERO, Quat::IDENTITY, 1.0 / 120.0),
            StuckVerdict::Free
        );
    }
}

#[test]
fn a_parked_pose_inside_the_bound_fires_at_time_thresh() {
    let mut stuck = VehicleStuck::new(SPEC);
    let pos = Vec3::new(4.0, 0.5, -2.0);
    stuck.impact(pos, Quat::IDENTITY);
    assert!(stuck.armed());
    // 1.0 s at 120 Hz: the 120th step inside pos_thresh fires.
    for i in 0..119 {
        assert_eq!(
            stuck.observe(pos, Quat::IDENTITY, 1.0 / 120.0),
            StuckVerdict::Watching,
            "step {i}"
        );
    }
    assert_eq!(
        stuck.observe(pos, Quat::IDENTITY, 1.0 / 120.0),
        StuckVerdict::Stuck
    );
    // One-shot: the episode disarmed on fire — nothing else lands
    // until the next impact re-arms it.
    assert!(!stuck.armed());
    assert_eq!(
        stuck.observe(pos, Quat::IDENTITY, 1.0 / 120.0),
        StuckVerdict::Free
    );
}

#[test]
fn escaping_past_move_thresh_disarms() {
    let mut stuck = VehicleStuck::new(SPEC);
    let anchor = Vec3::ZERO;
    stuck.impact(anchor, Quat::IDENTITY);
    // Creep inside the bound — counting.
    assert_eq!(
        stuck.observe(anchor, Quat::IDENTITY, 1.0 / 120.0),
        StuckVerdict::Watching
    );
    // Then drive away: past move_thresh the detector gives the episode up.
    let away = Vec3::new(SPEC.move_thresh + 0.1, 0.0, 0.0);
    assert_eq!(
        stuck.observe(away, Quat::IDENTITY, 1.0 / 120.0),
        StuckVerdict::Free
    );
    assert!(!stuck.armed());
    // Coming back does not resurrect it — only a new impact re-arms.
    for _ in 0..240 {
        stuck.observe(anchor, Quat::IDENTITY, 1.0 / 120.0);
    }
    assert_eq!(
        stuck.observe(anchor, Quat::IDENTITY, 1.0 / 120.0),
        StuckVerdict::Free
    );
}

#[test]
fn the_hysteresis_band_holds_without_accruing() {
    let mut stuck = VehicleStuck::new(SPEC);
    let anchor = Vec3::ZERO;
    stuck.impact(anchor, Quat::IDENTITY);
    // Accrue half the window inside pos_thresh…
    for _ in 0..60 {
        stuck.observe(anchor, Quat::IDENTITY, 1.0 / 120.0);
    }
    let accrued = stuck.accrued();
    assert!(accrued > 0.0);
    // …sit in the band — neither free nor counting.
    let band = Vec3::new(1.5, 0.0, 0.0);
    for _ in 0..120 {
        assert_eq!(
            stuck.observe(band, Quat::IDENTITY, 1.0 / 120.0),
            StuckVerdict::Watching
        );
    }
    assert_eq!(stuck.accrued(), accrued, "the band must not accrue");
    assert!(stuck.armed(), "the band must not disarm either");
    // Drift back inside and the episode resumes where it held — the
    // 60th accruing step completes the time_thresh window.
    for i in 0..60 {
        let verdict = stuck.observe(anchor, Quat::IDENTITY, 1.0 / 120.0);
        assert_eq!(
            verdict,
            if i == 59 {
                StuckVerdict::Stuck
            } else {
                StuckVerdict::Watching
            },
            "step {i}"
        );
    }
}

#[test]
fn a_past_turn_rotation_restarts_the_settle_window() {
    let mut stuck = VehicleStuck::new(StuckSpec {
        // A low authored bound: rotating past ~34° counts as moving.
        turn: 0.6,
        ..SPEC
    });
    let pos = Vec3::ZERO;
    stuck.impact(pos, Quat::IDENTITY);
    // Landed on its side — 90° off the impact pose is still tumbling:
    // the window restarts on the pose it settles into.
    let side = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    assert_eq!(
        stuck.observe(pos, side, 1.0 / 120.0),
        StuckVerdict::Watching
    );
    assert_eq!(stuck.accrued(), 0.0);
    // Once settled the rotation leg goes quiet and the window counts
    // from the *new* orientation.
    for _ in 0..119 {
        assert_eq!(
            stuck.observe(pos, side, 1.0 / 120.0),
            StuckVerdict::Watching
        );
    }
    assert_eq!(stuck.observe(pos, side, 1.0 / 120.0), StuckVerdict::Stuck);
}

#[test]
fn a_pi_turn_never_exempts_a_settled_pose() {
    // turn = π (the most common authored value): even a fully inverted
    // pose is inside the bound — a roofed car counts like a wedged one.
    let mut stuck = VehicleStuck::new(SPEC);
    let pos = Vec3::ZERO;
    stuck.impact(pos, Quat::IDENTITY);
    let roof = Quat::from_rotation_x(std::f32::consts::PI);
    for _ in 0..119 {
        assert_eq!(
            stuck.observe(pos, roof, 1.0 / 120.0),
            StuckVerdict::Watching
        );
    }
    assert_eq!(stuck.observe(pos, roof, 1.0 / 120.0), StuckVerdict::Stuck);
}

#[test]
fn every_new_impact_re_anchors_the_episode() {
    let mut stuck = VehicleStuck::new(SPEC);
    stuck.impact(Vec3::ZERO, Quat::IDENTITY);
    for _ in 0..60 {
        stuck.observe(Vec3::ZERO, Quat::IDENTITY, 1.0 / 120.0);
    }
    assert!(stuck.accrued() > 0.0);
    // A second hit somewhere else restarts the window on the new pose.
    let elsewhere = Vec3::new(20.0, 0.5, 8.0);
    stuck.impact(elsewhere, Quat::from_rotation_y(1.0));
    assert_eq!(stuck.accrued(), 0.0);
    // The old anchor is gone: sitting at the new one counts.
    let yawed = Quat::from_rotation_y(1.0);
    for _ in 0..119 {
        stuck.observe(elsewhere, yawed, 1.0 / 120.0);
    }
    assert_eq!(
        stuck.observe(elsewhere, yawed, 1.0 / 120.0),
        StuckVerdict::Stuck
    );
}

#[test]
fn garbage_observations_never_fire_or_panic() {
    let mut stuck = VehicleStuck::new(SPEC);
    stuck.impact(Vec3::ZERO, Quat::IDENTITY);
    for dt in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
        assert_eq!(
            stuck.observe(Vec3::NAN, Quat::IDENTITY, dt),
            StuckVerdict::Watching
        );
        assert_eq!(stuck.accrued(), 0.0);
        assert!(stuck.armed());
    }
    // A zero/negative turn bound disables the rotation leg — position
    // alone decides, so an inverted pose still counts.
    let mut legless = VehicleStuck::new(StuckSpec { turn: 0.0, ..SPEC });
    let pos = Vec3::ZERO;
    legless.impact(pos, Quat::IDENTITY);
    let roof = Quat::from_rotation_x(std::f32::consts::PI);
    for _ in 0..119 {
        legless.observe(pos, roof, 1.0 / 120.0);
    }
    assert_eq!(legless.observe(pos, roof, 1.0 / 120.0), StuckVerdict::Stuck);
}

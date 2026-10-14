//! F19-B.3 pedestrian reactions, pure: which cars threaten a walker,
//! how the reaction machine moves through its phases, how far each
//! authored dive row carries a walker, and that every path terminates.

use bevy::prelude::Vec3;
use mm2_game::ped::PedAnimState;
use mm2_game::pedreact::{
    Alarm, Approacher, DiveSide, Order, Phase, ReactPolicy, Reaction, assess, dive_lateral,
    nearest_car, rejoin_step, sensed_threat,
};

const POLICY: ReactPolicy = ReactPolicy {
    sense_range: 35.0,
    min_speed: 2.0,
    alert_time: 3.0,
    dive_time: 1.2,
    clear_hold: 1.0,
    max_wary: 8.0,
    max_dive: 6.0,
    rejoin_radius: 0.25,
};

/// A 4.4 × 1.85 m car at the origin driving +X at `speed`, straight.
fn car(speed: f32) -> Approacher {
    Approacher {
        position: Vec3::ZERO,
        velocity: Vec3::new(speed, 0.0, 0.0),
        half_length: 2.2,
        half_width: 0.925,
        steering: 0.0,
    }
}

fn state(x_offset: f32, x_distance: f32, first: u32, last: u32) -> PedAnimState {
    PedAnimState {
        name: "S".into(),
        clip: "c".into(),
        first_frame: first,
        last_frame: last,
        y_offset: 0.0,
        y_distance: 0.0,
        x_offset,
        x_distance,
        next: None,
    }
}

#[test]
fn a_car_heading_for_a_walker_is_imminent_then_alert_by_distance() {
    // 10 m/s: the walker 20 m ahead is past the 20 m near corridor, so
    // the far one stops it — (20 - 2)/10 = 1.8 s.
    let t = assess(Vec3::new(20.0, 0.0, 0.2), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Alert);
    assert!((t.time_to_contact - 1.8).abs() < 1e-3);
    // 10 m ahead inside the near corridor: (10 - 2)/10 = 0.8 s => dive.
    let t = assess(Vec3::new(10.0, 0.0, 0.2), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Imminent);
    // Beyond the 35 m sense range nothing happens.
    assert!(assess(Vec3::new(40.0, 0.0, 0.0), &car(10.0), &POLICY).is_none());
}

#[test]
fn the_corridors_are_the_recovered_retail_windows() {
    // Behind the nose — 0.25 of the car's length = 0.5 half-lengths
    // ahead of its centre — is no threat.
    assert!(assess(Vec3::new(1.0, 0.0, 0.0), &car(10.0), &POLICY).is_none());
    assert!(assess(Vec3::new(1.2, 0.0, 0.0), &car(10.0), &POLICY).is_some());
    // Along the axis the near window ends at 20 m, the far one at 35 m.
    assert_eq!(
        assess(Vec3::new(20.0, 0.0, 0.0), &car(10.0), &POLICY)
            .unwrap()
            .alarm,
        Alarm::Alert
    );
    assert!(assess(Vec3::new(34.0, 0.0, 0.0), &car(60.0), &POLICY).is_some());
    assert!(assess(Vec3::new(35.0, 0.0, 0.0), &car(60.0), &POLICY).is_none());
    // Lateral: half-width + 2 for the near corridor, + 4 for the far.
    let c = car(10.0);
    assert!(assess(Vec3::new(10.0, 0.0, 0.925 + 2.5), &c, &POLICY).is_some());
    assert!(assess(Vec3::new(10.0, 0.0, 0.925 + 4.5), &c, &POLICY).is_none());
}

#[test]
fn a_car_passing_in_its_lane_or_already_past_is_no_threat() {
    // 5.5 m to the side: clear of both corridors (half-width + 4).
    assert!(assess(Vec3::new(10.0, 0.0, 5.5), &car(10.0), &POLICY).is_none());
    // 4 m out is inside the far corridor only: wary, never a dive.
    let t = assess(Vec3::new(10.0, 0.0, 4.0), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Alert);
    // Inside the near corridor, past the dive time: alert only.
    let t = assess(Vec3::new(14.0, 0.0, 2.0), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Alert);
    // The car's centre is past the walker.
    assert!(assess(Vec3::new(-1.0, 0.0, 0.0), &car(10.0), &POLICY).is_none());
}

#[test]
fn a_hard_steering_car_picks_the_dive_side() {
    // The walker's own right is +Z in the car's frame.
    let right = Vec3::Z;
    let right_of_line = Vec3::new(8.0, 0.0, 0.3);
    let left_of_line = Vec3::new(8.0, 0.0, -0.3);
    // A car past full right lock (`> 0.85`) sends the walker right
    // regardless of which side of the line it stands on (retail
    // `0x54dc3f` compares strictly).
    let mut c = car(10.0);
    c.steering = 0.86;
    assert_eq!(
        assess(left_of_line, &c, &POLICY).unwrap().dive_side(right),
        DiveSide::Right
    );
    // Exactly 0.85 is not past full lock: the walker falls back to its
    // own side of the line (left).
    c.steering = 0.85;
    assert_eq!(
        assess(left_of_line, &c, &POLICY).unwrap().dive_side(right),
        DiveSide::Left
    );
    c.steering = -0.86;
    assert_eq!(
        assess(right_of_line, &c, &POLICY).unwrap().dive_side(right),
        DiveSide::Left
    );
    // Below the gate each walker dives to its own side of the line.
    c.steering = 0.5;
    assert_eq!(
        assess(right_of_line, &c, &POLICY).unwrap().dive_side(right),
        DiveSide::Right
    );
    assert_eq!(
        assess(left_of_line, &c, &POLICY).unwrap().dive_side(right),
        DiveSide::Left
    );
}

#[test]
fn slow_stopped_and_hostile_cars_are_ignored() {
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &car(1.0), &POLICY).is_none());
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &car(0.0), &POLICY).is_none());
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &car(f32::NAN), &POLICY).is_none());
    assert!(assess(Vec3::new(f32::INFINITY, 0.0, 0.0), &car(10.0), &POLICY).is_none());
    let mut c = car(10.0);
    c.half_width = f32::NAN;
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &c, &POLICY).is_none());
    let mut c = car(10.0);
    c.steering = f32::NAN;
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &c, &POLICY).is_none());
    // Vertical speed alone is not approach.
    let mut up = car(0.0);
    up.velocity = Vec3::new(0.0, 30.0, 0.0);
    assert!(assess(Vec3::new(5.0, 0.0, 0.0), &up, &POLICY).is_none());
}

#[test]
fn the_dive_direction_is_across_the_cars_line_on_the_walkers_side() {
    let left = assess(Vec3::new(8.0, 0.0, -0.3), &car(10.0), &POLICY).unwrap();
    let right = assess(Vec3::new(8.0, 0.0, 0.3), &car(10.0), &POLICY).unwrap();
    assert!(left.away.dot(Vec3::new(0.0, 0.0, -1.0)) > 0.99);
    assert!(right.away.dot(Vec3::new(0.0, 0.0, 1.0)) > 0.99);
    // Dead on the line picks the same fixed side every time.
    let a = assess(Vec3::new(8.0, 0.0, 0.0), &car(10.0), &POLICY).unwrap();
    let b = assess(Vec3::new(8.0, 0.0, 0.0), &car(10.0), &POLICY).unwrap();
    assert_eq!(a.away, b.away);
    // `toward` points at the car.
    assert!(a.toward.dot(Vec3::NEG_X) > 0.99);
}

#[test]
fn only_the_nearest_car_is_assessed() {
    let ped = Vec3::ZERO;
    // 9 m back on the walker's line and about to strike.
    let strike = Approacher {
        position: Vec3::new(-9.0, 0.0, 0.0),
        ..car(10.0)
    };
    // Nearer (3 m to the side) but parked: no threat of its own.
    let parked = Approacher {
        position: Vec3::new(0.0, 0.0, 3.0),
        velocity: Vec3::ZERO,
        ..car(10.0)
    };
    // Nearer and moving, but its line clears the walker.
    let off_line = Approacher {
        position: Vec3::new(-4.0, 0.0, 6.0),
        ..car(10.0)
    };
    assert_eq!(
        sensed_threat(ped, [strike], &POLICY).unwrap().alarm,
        Alarm::Imminent
    );
    assert!(sensed_threat(ped, [strike, parked], &POLICY).is_none());
    assert!(sensed_threat(ped, [parked, strike], &POLICY).is_none());
    assert!(sensed_threat(ped, [strike, off_line], &POLICY).is_none());
    assert!(sensed_threat(ped, [], &POLICY).is_none());
}

#[test]
fn the_nearest_car_is_by_horizontal_distance_and_the_first_of_equals() {
    let ped = Vec3::new(0.0, 5.0, 0.0);
    let at = |x: f32, y: f32, z: f32| Approacher {
        position: Vec3::new(x, y, z),
        ..car(10.0)
    };
    // Height is ignored: the 20 m-high car at 2 m horizontal wins.
    let n = nearest_car(ped, [at(5.0, 0.0, 0.0), at(2.0, 20.0, 0.0)]).unwrap();
    assert_eq!(n.position, Vec3::new(2.0, 20.0, 0.0));
    // Equal distance keeps the first.
    let n = nearest_car(ped, [at(3.0, 0.0, 0.0), at(0.0, 0.0, 3.0)]).unwrap();
    assert_eq!(n.position, Vec3::new(3.0, 0.0, 0.0));
    // A non-finite position never wins.
    let n = nearest_car(ped, [at(f32::NAN, 0.0, 0.0), at(9.0, 0.0, 0.0)]).unwrap();
    assert_eq!(n.position, Vec3::new(9.0, 0.0, 0.0));
}

#[test]
fn a_dive_side_follows_the_walkers_own_frame() {
    // Facing -Z the walker's right is +X.
    assert_eq!(DiveSide::toward(Vec3::X, Vec3::X), DiveSide::Right);
    assert_eq!(DiveSide::toward(Vec3::X, Vec3::NEG_X), DiveSide::Left);
    assert_eq!(DiveSide::toward(Vec3::X, Vec3::Z), DiveSide::Right, "tie");
    assert_eq!(DiveSide::Left.from_antic(), "ANTIC_LDIVE");
    assert_eq!(DiveSide::Right.from_walk(), "WALK_RDIVE");
}

#[test]
fn the_authored_dive_rows_chain_their_lateral_travel() {
    // Retail man rows: ANTIC_LDIVE 0 -> +2.2 over frames 1..=10, then
    // LDIVE_GROUNDL 2.2 -> 4.38.
    let first = state(0.0, 2.2, 0, 9);
    assert_eq!(dive_lateral(&first, 0.0, 10), Some(0.0));
    assert!((dive_lateral(&first, 5.0, 10).unwrap() - 1.1).abs() < 1e-5);
    assert!((dive_lateral(&first, 10.0, 10).unwrap() - 2.2).abs() < 1e-5);
    let second = state(2.2, 2.18, 0, 9);
    assert!((dive_lateral(&second, 0.0, 10).unwrap() - 2.2).abs() < 1e-5);
    assert!((dive_lateral(&second, 10.0, 10).unwrap() - 4.38).abs() < 1e-5);
    // The right dive authors the mirror image.
    let right = state(0.0, -2.2, 0, 9);
    assert!((dive_lateral(&right, 10.0, 10).unwrap() + 2.2).abs() < 1e-5);
    // Ground recovery authors no lateral travel: the walker holds.
    assert_eq!(dive_lateral(&state(0.0, 0.0, 0, 43), 20.0, 44), None);
}

#[test]
fn dive_travel_clamps_to_the_window_and_survives_hostile_rows() {
    let row = state(0.0, 2.2, 0, 9);
    assert_eq!(dive_lateral(&row, -5.0, 10), Some(0.0));
    assert!((dive_lateral(&row, 99.0, 10).unwrap() - 2.2).abs() < 1e-5);
    // An authored `frames + 1` end clamps against a shorter clip.
    let over = state(0.0, 2.2, 0, 10);
    assert!((dive_lateral(&over, 10.0, 10).unwrap() - 2.2).abs() < 1e-5);
    assert_eq!(dive_lateral(&row, 3.0, 0), None);
    assert_eq!(dive_lateral(&state(0.0, f32::NAN, 0, 9), 3.0, 10), None);
    assert_eq!(
        dive_lateral(&state(f32::INFINITY, 2.2, 0, 9), 3.0, 10),
        None
    );
    // An empty window does not divide by zero.
    let empty = state(0.0, 2.2, 5, 2);
    assert!(dive_lateral(&empty, 5.0, 10).unwrap().is_finite());
}

fn threat(alarm: Alarm) -> mm2_game::pedreact::Threat {
    mm2_game::pedreact::Threat {
        alarm,
        time_to_contact: 1.0,
        toward: Vec3::X,
        away: Vec3::Z,
        steering: 0.0,
    }
}

#[test]
fn an_alert_stops_the_walker_and_it_walks_on_after_the_threat_clears() {
    let mut r = Reaction::new();
    let alert = threat(Alarm::Alert);
    let o = r.decide("WALK", Some(&alert), DiveSide::Right, 0.016, &POLICY);
    assert_eq!((o, r.phase), (Order::Alert, Phase::Wary));
    // Still threatened: it holds, and the clear timer does not run.
    for _ in 0..200 {
        let o = r.decide("ANTIC", Some(&alert), DiveSide::Right, 0.016, &POLICY);
        assert_eq!(o, Order::None);
    }
    // Clear, but still in the WALK_ANTIC transition: it waits for ANTIC.
    assert_eq!(
        r.decide("WALK_ANTIC", None, DiveSide::Right, 2.0, &POLICY),
        Order::None
    );
    assert_eq!(r.phase, Phase::Wary);
    let o = r.decide("ANTIC", None, DiveSide::Right, 0.016, &POLICY);
    assert_eq!((o, r.phase), (Order::Resume, Phase::Walking));
    assert!(r.is_walking());
}

#[test]
fn a_persistent_threat_cannot_hold_a_walker_forever() {
    let mut r = Reaction::new();
    let alert = threat(Alarm::Alert);
    r.decide("WALK", Some(&alert), DiveSide::Right, 0.1, &POLICY);
    let mut resumed_at = None;
    for i in 0..200 {
        if r.decide("ANTIC", Some(&alert), DiveSide::Right, 0.1, &POLICY) == Order::Resume {
            resumed_at = Some(i);
            break;
        }
    }
    let i = resumed_at.expect("max_wary releases the walker");
    assert!((i as f32 * 0.1 - POLICY.max_wary).abs() < 0.5, "at {i}");
}

#[test]
fn an_imminent_car_sends_a_walker_diving_from_either_stance() {
    let strike = threat(Alarm::Imminent);
    let mut walking = Reaction::new();
    let o = walking.decide("WALK", Some(&strike), DiveSide::Left, 0.016, &POLICY);
    assert_eq!(
        (o, walking.phase),
        (Order::Dive(DiveSide::Left), Phase::Diving)
    );

    let mut wary = Reaction::new();
    wary.decide(
        "WALK",
        Some(&threat(Alarm::Alert)),
        DiveSide::Right,
        0.016,
        &POLICY,
    );
    let o = wary.decide("ANTIC", Some(&strike), DiveSide::Right, 0.016, &POLICY);
    assert_eq!(
        (o, wary.phase),
        (Order::Dive(DiveSide::Right), Phase::Diving)
    );
}

#[test]
fn a_dive_ends_at_stand_and_the_walker_rejoins() {
    let mut r = Reaction::new();
    r.decide(
        "WALK",
        Some(&threat(Alarm::Imminent)),
        DiveSide::Left,
        0.016,
        &POLICY,
    );
    // The authored chain: nothing happens until it reaches STAND, and
    // fresh threats are ignored in flight.
    for s in ["ANTIC_LDIVE", "LDIVE_GROUNDL", "GROUND_STANDL"] {
        let o = r.decide(
            s,
            Some(&threat(Alarm::Imminent)),
            DiveSide::Left,
            0.1,
            &POLICY,
        );
        assert_eq!((o, r.phase), (Order::None, Phase::Diving), "{s}");
    }
    let o = r.decide("STAND", None, DiveSide::Left, 0.1, &POLICY);
    assert_eq!((o, r.phase), (Order::Resume, Phase::Rejoining));
    assert!(!r.is_walking());
    r.rejoined();
    assert!(r.is_walking());
}

#[test]
fn a_dive_whose_chain_never_reaches_stand_still_terminates() {
    let mut r = Reaction::new();
    r.decide(
        "WALK",
        Some(&threat(Alarm::Imminent)),
        DiveSide::Right,
        0.016,
        &POLICY,
    );
    let mut n = 0;
    while r.phase == Phase::Diving {
        r.decide("GROUND_STANDR", None, DiveSide::Right, 0.1, &POLICY);
        n += 1;
        assert!(n < 1000, "bounded by max_dive");
    }
    assert_eq!(r.phase, Phase::Rejoining);
}

#[test]
fn hostile_time_steps_advance_nothing() {
    let mut r = Reaction::new();
    r.decide(
        "WALK",
        Some(&threat(Alarm::Alert)),
        DiveSide::Right,
        0.0,
        &POLICY,
    );
    for dt in [f32::NAN, f32::INFINITY, -5.0] {
        assert_eq!(
            r.decide("ANTIC", None, DiveSide::Right, dt, &POLICY),
            Order::None
        );
    }
    assert_eq!(r.phase, Phase::Wary);
}

#[test]
fn the_walk_back_is_bounded_and_arrives() {
    let home = Vec3::new(8.5, 0.0, 10.0);
    let mut at = Vec3::new(8.5 + 4.4, 0.0, 10.0);
    let mut steps = 0;
    loop {
        let (next, arrived) = rejoin_step(at, home, 1.4 / 60.0, 0.25);
        assert!(next.distance(home) <= at.distance(home) + 1e-5, "closes");
        at = next;
        steps += 1;
        if arrived {
            break;
        }
        assert!(steps < 1000);
    }
    assert_eq!(at, home);
    assert!(steps > 100, "a 4 m walk takes a while: {steps}");
    // Hostile input holds still.
    assert_eq!(rejoin_step(at, Vec3::NAN, 1.0, 0.25), (at, false));
    assert_eq!(rejoin_step(at, home + Vec3::X, f32::NAN, 0.25), (at, false));
}

#[test]
fn a_car_beyond_the_sense_range_is_not_noticed() {
    let near = Vec3::new(30.0, 0.0, 0.0);
    let far = Vec3::new(36.0, 0.0, 0.0);
    let mut fast = car(60.0);
    fast.position = Vec3::ZERO;
    assert!(assess(near, &fast, &POLICY).is_some());
    assert!(assess(far, &fast, &POLICY).is_none());
}

#[test]
fn the_default_policy_carries_the_recovered_retail_thresholds() {
    let d = ReactPolicy::default();
    assert_eq!((d.sense_range, d.min_speed), (35.0, 1.0));
    assert_eq!((d.alert_time, d.dive_time), (2.3, 0.75));
}

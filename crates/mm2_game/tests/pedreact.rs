//! F19-B.3 pedestrian reactions, pure: which cars threaten a walker,
//! how the reaction machine moves through its phases, how far each
//! authored dive row carries a walker, and that every path terminates.

use bevy::prelude::Vec3;
use mm2_game::ped::PedAnimState;
use mm2_game::pedreact::{
    Alarm, Approacher, DiveSide, Order, Phase, ReactPolicy, Reaction, assess, dive_lateral,
    most_urgent, rejoin_step,
};

const POLICY: ReactPolicy = ReactPolicy {
    sense_range: 35.0,
    min_speed: 2.0,
    alert_time: 3.0,
    dive_time: 1.2,
    alert_margin: 1.5,
    dive_margin: 0.5,
    clear_hold: 1.0,
    max_wary: 8.0,
    max_dive: 6.0,
    rejoin_radius: 0.25,
};

/// A 4.4 × 1.85 m car at the origin driving +X at `speed`.
fn car(speed: f32) -> Approacher {
    Approacher {
        position: Vec3::ZERO,
        velocity: Vec3::new(speed, 0.0, 0.0),
        half_length: 2.2,
        half_width: 0.925,
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
    // 10 m/s: the nose is 20 m - 2.2 m away => 1.78 s, alert only.
    let t = assess(Vec3::new(20.0, 0.0, 0.2), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Alert);
    assert!((t.time_to_contact - 1.78).abs() < 1e-3);
    // 10 m ahead: 0.78 s => dive.
    let t = assess(Vec3::new(10.0, 0.0, 0.2), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Imminent);
    // Beyond three seconds nothing happens.
    assert!(assess(Vec3::new(40.0, 0.0, 0.0), &car(10.0), &POLICY).is_none());
}

#[test]
fn a_car_passing_in_its_lane_or_already_past_is_no_threat() {
    // 4 m to the side of a car in its lane: clear of the 0.925 + 1.5 band.
    assert!(assess(Vec3::new(10.0, 0.0, 4.0), &car(10.0), &POLICY).is_none());
    // Inside the alert band but outside the strike band.
    let t = assess(Vec3::new(10.0, 0.0, 2.0), &car(10.0), &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Alert);
    // The car's centre is past the walker.
    assert!(assess(Vec3::new(-1.0, 0.0, 0.0), &car(10.0), &POLICY).is_none());
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
fn the_most_urgent_car_wins() {
    let near_alert = Approacher {
        position: Vec3::new(-12.0, 0.0, 1.8),
        ..car(10.0)
    };
    let strike = Approacher {
        position: Vec3::new(-6.0, 0.0, 0.0),
        ..car(10.0)
    };
    let ped = Vec3::ZERO;
    let t = most_urgent(ped, [near_alert, strike], &POLICY).unwrap();
    assert_eq!(t.alarm, Alarm::Imminent);
    assert!(most_urgent(ped, [], &POLICY).is_none());
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

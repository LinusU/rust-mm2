//! F21-C.2 contract tests: the per-family Crash Course leg rules
//! recovered from the retail executable (`CC-8`/`CC-9`). Each family
//! has a positive case (the rule clears or fails the leg as the
//! handler does) and a negative one (the near miss that must not).
//! The observations are synthetic; the rules and constants are the
//! executable's.

use mm2_game::{
    CORNER_DEFAULT_MIN_MPH, CORNER_SPEED_GRACE_SECS, LeadObservation, LegFailure, LegObservation,
    LegReport, LegResult, LegRule, MPH_PER_MS, RuleError, RuleState, Verdict, WreckPolicy,
};

const DT: f32 = 1.0 / 60.0;

fn eval(rule: LegRule, state: &mut RuleState, obs: &LegObservation) -> Verdict {
    rule.evaluate(state, obs, DT).expect("a valid observation")
}

fn once(rule: LegRule, obs: &LegObservation) -> Verdict {
    eval(rule, &mut RuleState::default(), obs)
}

fn lead(distance_m: f32) -> LeadObservation {
    LeadObservation {
        distance_sq: distance_m * distance_m,
        route_finished: false,
        wrecked: false,
    }
}

fn with_lead(lead: LeadObservation) -> LegObservation {
    LegObservation {
        lead: Some(lead),
        ..LegObservation::IDLE
    }
}

// --- gate run (jump, cross-traffic, maneuver, map) -------------------

#[test]
fn gate_run_clears_on_course_complete() {
    let obs = LegObservation {
        course_complete: true,
        ..LegObservation::IDLE
    };
    assert_eq!(once(LegRule::GateRun, &obs), Verdict::Cleared);
}

#[test]
fn gate_run_stays_open_until_something_ends_it() {
    assert_eq!(
        once(LegRule::GateRun, &LegObservation::IDLE),
        Verdict::Pending
    );
}

#[test]
fn gate_run_times_out_and_a_wreck_fails_it() {
    let timed_out = LegObservation {
        time_expired: true,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(LegRule::GateRun, &timed_out),
        Verdict::Failed(LegFailure::TimedOut)
    );
    let wrecked = LegObservation {
        wrecked: true,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(LegRule::GateRun, &wrecked),
        Verdict::Failed(LegFailure::Disabled)
    );
}

#[test]
fn gate_run_completion_beats_a_simultaneous_expiry_or_wreck() {
    let obs = LegObservation {
        course_complete: true,
        time_expired: true,
        wrecked: true,
        ..LegObservation::IDLE
    };
    assert_eq!(once(LegRule::GateRun, &obs), Verdict::Cleared);
}

// --- follow ----------------------------------------------------------

#[test]
fn follow_clears_within_ten_metres_once_the_lead_has_arrived() {
    let obs = with_lead(LeadObservation {
        route_finished: true,
        ..lead(9.9)
    });
    assert_eq!(once(LegRule::Follow, &obs), Verdict::Cleared);
}

#[test]
fn follow_clears_on_course_complete_close_to_the_lead() {
    let obs = LegObservation {
        course_complete: true,
        ..with_lead(lead(5.0))
    };
    assert_eq!(once(LegRule::Follow, &obs), Verdict::Cleared);
}

#[test]
fn follow_does_not_clear_while_the_lead_is_still_driving() {
    // Close, but neither the lead's route nor the course is done.
    assert_eq!(
        once(LegRule::Follow, &with_lead(lead(3.0))),
        Verdict::Pending
    );
}

#[test]
fn follow_does_not_clear_at_ten_metres_or_more() {
    let obs = with_lead(LeadObservation {
        route_finished: true,
        ..lead(10.0)
    });
    assert_eq!(once(LegRule::Follow, &obs), Verdict::Pending);
}

#[test]
fn follow_fails_when_the_lead_escapes_past_a_hundred_metres() {
    assert_eq!(
        once(LegRule::Follow, &with_lead(lead(100.1))),
        Verdict::Failed(LegFailure::Objective)
    );
    // Exactly at the bound is still following.
    assert_eq!(
        once(LegRule::Follow, &with_lead(lead(100.0))),
        Verdict::Pending
    );
}

#[test]
fn follow_ignores_the_timer() {
    let obs = LegObservation {
        time_expired: true,
        ..with_lead(lead(30.0))
    };
    assert_eq!(once(LegRule::Follow, &obs), Verdict::Pending);
    assert!(LegRule::Follow.ignores_time_limit());
}

#[test]
fn follow_resets_a_wrecked_car_instead_of_failing() {
    let obs = LegObservation {
        wrecked: true,
        ..with_lead(lead(30.0))
    };
    assert_eq!(once(LegRule::Follow, &obs), Verdict::ResetCar);
    assert_eq!(LegRule::Follow.wreck_policy(), WreckPolicy::ResetCar);
}

#[test]
fn follow_without_a_lead_is_an_error_not_a_pass() {
    let obs = LegObservation {
        course_complete: true,
        ..LegObservation::IDLE
    };
    assert_eq!(
        LegRule::Follow.evaluate(&mut RuleState::default(), &obs, DT),
        Err(RuleError::MissingLead)
    );
}

// --- cop chase -------------------------------------------------------

#[test]
fn cop_chase_clears_at_the_finish_with_no_pursuer_in_sight() {
    let obs = LegObservation {
        course_complete: true,
        ..LegObservation::IDLE
    };
    assert_eq!(once(LegRule::CopChase, &obs), Verdict::Cleared);
}

#[test]
fn cop_chase_fails_at_the_finish_with_a_pursuer_in_sight() {
    let obs = LegObservation {
        course_complete: true,
        pursuer_in_sight: true,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(LegRule::CopChase, &obs),
        Verdict::Failed(LegFailure::Objective)
    );
}

#[test]
fn cop_chase_does_not_care_about_pursuers_before_the_finish() {
    let obs = LegObservation {
        pursuer_in_sight: true,
        ..LegObservation::IDLE
    };
    assert_eq!(once(LegRule::CopChase, &obs), Verdict::Pending);
}

#[test]
fn cop_chase_times_out_and_resets_a_wreck() {
    let timed_out = LegObservation {
        time_expired: true,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(LegRule::CopChase, &timed_out),
        Verdict::Failed(LegFailure::TimedOut)
    );
    let wrecked = LegObservation {
        wrecked: true,
        ..LegObservation::IDLE
    };
    assert_eq!(once(LegRule::CopChase, &wrecked), Verdict::ResetCar);
    assert_eq!(LegRule::CopChase.wreck_policy(), WreckPolicy::ResetCar);
}

// --- corner ----------------------------------------------------------

fn speed(mph: f32) -> LegObservation {
    LegObservation {
        speed_mph: mph,
        ..LegObservation::IDLE
    }
}

#[test]
fn corner_uses_the_authored_speed_or_the_fifty_mph_default() {
    assert_eq!(
        LegRule::corner(40.0),
        LegRule::Corner {
            min_speed_mph: 40.0
        }
    );
    assert_eq!(
        LegRule::corner(0.0),
        LegRule::Corner {
            min_speed_mph: CORNER_DEFAULT_MIN_MPH
        }
    );
    assert_eq!(CORNER_DEFAULT_MIN_MPH, 50.0);
    // Below one is "unset", not a one mph limit.
    assert_eq!(LegRule::corner(0.5), LegRule::corner(0.0));
}

#[test]
fn corner_clears_at_the_finish_after_holding_the_speed() {
    let rule = LegRule::corner(40.0);
    let mut state = RuleState::default();
    assert_eq!(eval(rule, &mut state, &speed(45.0)), Verdict::Pending);
    assert!(state.reached_speed());
    let done = LegObservation {
        course_complete: true,
        gates_passed: 5,
        speed_mph: 45.0,
        ..LegObservation::IDLE
    };
    assert_eq!(eval(rule, &mut state, &done), Verdict::Cleared);
}

#[test]
fn corner_fails_a_gate_hit_before_the_speed_was_reached() {
    let rule = LegRule::corner(40.0);
    let slow_at_a_gate = LegObservation {
        gates_passed: 1,
        speed_mph: 20.0,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(rule, &slow_at_a_gate),
        Verdict::Failed(LegFailure::Objective)
    );
    // Speed seen on the same tick as the first gate is too late: the
    // original tests the gate counter before it tests the speed.
    let fast_at_a_gate = LegObservation {
        gates_passed: 1,
        speed_mph: 41.0,
        ..LegObservation::IDLE
    };
    assert_eq!(
        once(rule, &fast_at_a_gate),
        Verdict::Failed(LegFailure::Objective)
    );
    // Reached on an earlier tick, the same gate is fine.
    let mut state = RuleState::default();
    assert_eq!(eval(rule, &mut state, &speed(41.0)), Verdict::Pending);
    assert_eq!(eval(rule, &mut state, &fast_at_a_gate), Verdict::Pending);
}

#[test]
fn corner_speed_must_be_strictly_above_the_minimum_to_count() {
    let rule = LegRule::corner(40.0);
    let mut state = RuleState::default();
    eval(rule, &mut state, &speed(40.0));
    assert!(!state.reached_speed());
    eval(rule, &mut state, &speed(40.1));
    assert!(state.reached_speed());
}

#[test]
fn corner_tolerates_a_dip_of_up_to_one_second() {
    let rule = LegRule::corner(40.0);
    let mut state = RuleState::default();
    eval(rule, &mut state, &speed(50.0));
    // 59 ticks (< 1 s) below the minimum.
    for _ in 0..59 {
        assert_eq!(eval(rule, &mut state, &speed(30.0)), Verdict::Pending);
    }
    assert!(state.below_secs() < CORNER_SPEED_GRACE_SECS);
    // Back above the minimum clears the clock.
    assert_eq!(eval(rule, &mut state, &speed(50.0)), Verdict::Pending);
    assert_eq!(state.below_secs(), 0.0);
    for _ in 0..59 {
        assert_eq!(eval(rule, &mut state, &speed(30.0)), Verdict::Pending);
    }
}

#[test]
fn corner_fails_after_more_than_a_second_below_the_minimum() {
    let rule = LegRule::corner(40.0);
    let mut state = RuleState::default();
    eval(rule, &mut state, &speed(50.0));
    let mut verdict = Verdict::Pending;
    for _ in 0..61 {
        verdict = eval(rule, &mut state, &speed(30.0));
        if verdict != Verdict::Pending {
            break;
        }
    }
    assert_eq!(verdict, Verdict::Failed(LegFailure::Objective));
    // The failure clears the reached flag (the original's `0x7968`).
    assert!(!state.reached_speed());
}

#[test]
fn corner_untimed_leg_never_times_out_but_a_timed_one_does() {
    let rule = LegRule::corner(40.0);
    assert_eq!(once(rule, &speed(10.0)), Verdict::Pending);
    let expired = LegObservation {
        time_expired: true,
        ..speed(10.0)
    };
    assert_eq!(once(rule, &expired), Verdict::Failed(LegFailure::TimedOut));
}

#[test]
fn corner_fails_the_finish_with_a_pursuer_in_sight_and_a_wreck_fails_the_leg() {
    let rule = LegRule::corner(40.0);
    let chased = LegObservation {
        course_complete: true,
        pursuer_in_sight: true,
        ..speed(60.0)
    };
    assert_eq!(once(rule, &chased), Verdict::Failed(LegFailure::Objective));
    let wrecked = LegObservation {
        wrecked: true,
        ..speed(0.0)
    };
    assert_eq!(once(rule, &wrecked), Verdict::Failed(LegFailure::Disabled));
    assert_eq!(rule.wreck_policy(), WreckPolicy::FailLeg);
}

#[test]
fn mph_conversion_is_the_executables() {
    // 22.352 m/s is 50 mph by the real factor; the game's constant
    // reads it as 49.98.
    let mph = 22.352 * MPH_PER_MS;
    assert!((mph - 49.98).abs() < 0.01, "{mph}");
}

// --- stop ------------------------------------------------------------

#[test]
fn stop_clears_when_the_lead_is_wrecked() {
    let obs = with_lead(LeadObservation {
        wrecked: true,
        ..lead(30.0)
    });
    assert_eq!(once(LegRule::Stop, &obs), Verdict::Cleared);
}

#[test]
fn stop_fails_when_the_lead_reaches_its_destination_intact() {
    let obs = with_lead(LeadObservation {
        route_finished: true,
        ..lead(30.0)
    });
    assert_eq!(
        once(LegRule::Stop, &obs),
        Verdict::Failed(LegFailure::Objective)
    );
}

#[test]
fn stop_wrecking_the_lead_on_arrival_still_counts() {
    let obs = with_lead(LeadObservation {
        route_finished: true,
        wrecked: true,
        ..lead(1.0)
    });
    assert_eq!(once(LegRule::Stop, &obs), Verdict::Cleared);
}

#[test]
fn stop_is_timed_and_resets_a_wreck() {
    let expired = LegObservation {
        time_expired: true,
        ..with_lead(lead(30.0))
    };
    assert_eq!(
        once(LegRule::Stop, &expired),
        Verdict::Failed(LegFailure::TimedOut)
    );
    let wrecked = LegObservation {
        wrecked: true,
        ..with_lead(lead(30.0))
    };
    assert_eq!(once(LegRule::Stop, &wrecked), Verdict::ResetCar);
    assert!(!LegRule::Stop.ignores_time_limit());
}

#[test]
fn stop_reaching_the_gate_is_not_a_pass() {
    // The stop handler never reads the course flag.
    let obs = LegObservation {
        course_complete: true,
        ..with_lead(lead(30.0))
    };
    assert_eq!(once(LegRule::Stop, &obs), Verdict::Pending);
}

// --- shared ----------------------------------------------------------

#[test]
fn non_finite_numbers_are_diagnosed_not_trusted() {
    let nan_speed = LegObservation {
        speed_mph: f32::NAN,
        ..LegObservation::IDLE
    };
    assert_eq!(
        LegRule::GateRun.evaluate(&mut RuleState::default(), &nan_speed, DT),
        Err(RuleError::NonFinite("speed"))
    );
    let bad_lead = with_lead(LeadObservation {
        distance_sq: f32::INFINITY,
        ..lead(1.0)
    });
    assert_eq!(
        LegRule::Follow.evaluate(&mut RuleState::default(), &bad_lead, DT),
        Err(RuleError::NonFinite("lead distance"))
    );
    assert_eq!(
        LegRule::GateRun.evaluate(&mut RuleState::default(), &LegObservation::IDLE, f32::NAN),
        Err(RuleError::NonFinite("tick length"))
    );
    assert_eq!(
        LegRule::Corner {
            min_speed_mph: f32::NAN
        }
        .evaluate(&mut RuleState::default(), &LegObservation::IDLE, DT),
        Err(RuleError::NonFinite("minimum speed"))
    );
}

#[test]
fn a_verdict_becomes_the_sequencers_report() {
    assert_eq!(
        LegReport::from_verdict(2, 1, 99, Verdict::Cleared),
        Some(LegReport {
            attempt: 2,
            leg: 1,
            result: LegResult::Cleared { ticks: 99 }
        })
    );
    assert_eq!(
        LegReport::from_verdict(1, 0, 5, Verdict::Failed(LegFailure::Objective)),
        Some(LegReport {
            attempt: 1,
            leg: 0,
            result: LegResult::Failed(LegFailure::Objective)
        })
    );
    // Undecided and a car reset do not end the leg.
    assert_eq!(LegReport::from_verdict(1, 0, 5, Verdict::Pending), None);
    assert_eq!(LegReport::from_verdict(1, 0, 5, Verdict::ResetCar), None);
}

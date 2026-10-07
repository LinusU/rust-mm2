//! F21-B.2 contract tests: the lesson leg sequencer — strict authored
//! order, stale/duplicate rejection, whole-lesson retry with cleared
//! counters, and a pass that is credited exactly once.

use mm2_game::{
    LegFailure, LegReport, LegResult, LessonPass, LessonPhase, LessonRun, NoLegs, ParticipantState,
    PlayerId, ReportEffect, ResultId, RetryError, StaleReason,
};

fn clear(attempt: u32, leg: u32, ticks: u64) -> LegReport {
    LegReport {
        attempt,
        leg,
        result: LegResult::Cleared { ticks },
    }
}

fn fail(attempt: u32, leg: u32, why: LegFailure) -> LegReport {
    LegReport {
        attempt,
        leg,
        result: LegResult::Failed(why),
    }
}

#[test]
fn a_lesson_with_no_legs_is_refused_rather_than_passed_vacuously() {
    assert_eq!(LessonRun::new(0).unwrap_err(), NoLegs);
}

#[test]
fn legs_clear_in_authored_order_and_the_last_one_passes() {
    let mut run = LessonRun::new(3).unwrap();
    assert_eq!(run.phase(), LessonPhase::Running { leg: 0 });
    assert_eq!(
        run.report(clear(1, 0, 100)),
        ReportEffect::Advanced { next_leg: 1 }
    );
    assert_eq!(
        run.report(clear(1, 1, 250)),
        ReportEffect::Advanced { next_leg: 2 }
    );
    assert_eq!(run.current_leg(), Some(2));
    assert_eq!(run.report(clear(1, 2, 30)), ReportEffect::Passed);
    assert_eq!(run.phase(), LessonPhase::Passed);
    assert_eq!(run.current_leg(), None);
    assert_eq!(run.cleared(), &[100, 250, 30]);
    assert_eq!(run.stale_reports(), 0);
}

#[test]
fn a_single_leg_lesson_passes_on_its_one_clear() {
    let mut run = LessonRun::new(1).unwrap();
    assert_eq!(run.report(clear(1, 0, 7)), ReportEffect::Passed);
    let pass = run.take_pass().unwrap();
    assert_eq!(pass.attempt, 1);
    assert_eq!(pass.total_ticks(), 7);
}

#[test]
fn an_out_of_order_or_duplicated_report_never_skips_or_repeats_a_leg() {
    let mut run = LessonRun::new(3).unwrap();
    // Leg 2 cannot clear before leg 0.
    assert_eq!(
        run.report(clear(1, 2, 1)),
        ReportEffect::Stale(StaleReason::Leg)
    );
    assert_eq!(run.current_leg(), Some(0));
    run.report(clear(1, 0, 100));
    // Leg 0's clear delivered again does not advance past leg 1.
    assert_eq!(
        run.report(clear(1, 0, 100)),
        ReportEffect::Stale(StaleReason::Leg)
    );
    assert_eq!(run.current_leg(), Some(1));
    assert_eq!(run.cleared(), &[100]);
    assert_eq!(run.stale_reports(), 2);
}

#[test]
fn a_failure_ends_the_attempt_and_later_reports_are_not_running() {
    let mut run = LessonRun::new(2).unwrap();
    run.report(clear(1, 0, 10));
    assert_eq!(
        run.report(fail(1, 1, LegFailure::TimedOut)),
        ReportEffect::Failed
    );
    assert_eq!(
        run.phase(),
        LessonPhase::Failed {
            leg: 1,
            failure: LegFailure::TimedOut
        }
    );
    // A late clear of the same leg cannot revive the failed attempt.
    assert_eq!(
        run.report(clear(1, 1, 5)),
        ReportEffect::Stale(StaleReason::NotRunning)
    );
    assert_eq!(run.take_pass(), None);
}

#[test]
fn retry_restarts_the_whole_lesson_with_every_counter_cleared() {
    let mut run = LessonRun::new(3).unwrap();
    run.report(clear(1, 0, 10));
    run.report(clear(1, 1, 20));
    run.report(fail(1, 2, LegFailure::Disabled));
    assert_eq!(run.cleared(), &[10, 20]);

    assert_eq!(run.retry(), Ok(2));
    assert_eq!(run.attempt(), 2);
    assert_eq!(run.phase(), LessonPhase::Running { leg: 0 });
    assert!(run.cleared().is_empty(), "no stale cleared legs survive");

    // Reports from the first attempt are stale on the second.
    assert_eq!(
        run.report(clear(1, 0, 10)),
        ReportEffect::Stale(StaleReason::Attempt)
    );
    assert_eq!(
        run.report(fail(1, 0, LegFailure::Objective)),
        ReportEffect::Stale(StaleReason::Attempt)
    );
    assert_eq!(run.phase(), LessonPhase::Running { leg: 0 });

    // The second attempt can pass; the pass names it and carries only
    // its own legs.
    run.report(clear(2, 0, 11));
    run.report(clear(2, 1, 21));
    assert_eq!(run.report(clear(2, 2, 31)), ReportEffect::Passed);
    assert_eq!(
        run.take_pass(),
        Some(LessonPass {
            attempt: 2,
            leg_ticks: vec![11, 21, 31]
        })
    );
}

#[test]
fn a_mid_lesson_restart_is_a_retry_too() {
    // DMG-2: a disabled Crash Course car restarts the lesson.
    let mut run = LessonRun::new(2).unwrap();
    run.report(clear(1, 0, 10));
    assert_eq!(run.retry(), Ok(2));
    assert_eq!(run.current_leg(), Some(0));
    assert!(run.cleared().is_empty());
    // The first attempt's leg-1 clear arriving late is stale.
    assert_eq!(
        run.report(clear(1, 1, 5)),
        ReportEffect::Stale(StaleReason::Attempt)
    );
}

#[test]
fn the_pass_is_credited_exactly_once() {
    let mut run = LessonRun::new(2).unwrap();
    assert_eq!(run.take_pass(), None, "nothing before the lesson passes");
    run.report(clear(1, 0, 4));
    assert_eq!(run.take_pass(), None, "nor mid-lesson");
    run.report(clear(1, 1, 6));
    let pass = run.take_pass().expect("first claim");
    assert_eq!(pass.leg_ticks, vec![4, 6]);
    assert_eq!(run.take_pass(), None, "second claim");
    // A re-delivered final clear changes nothing and credits nothing.
    assert_eq!(
        run.report(clear(1, 1, 6)),
        ReportEffect::Stale(StaleReason::NotRunning)
    );
    assert_eq!(run.take_pass(), None);
    assert_eq!(run.phase(), LessonPhase::Passed);
}

#[test]
fn a_passed_lesson_cannot_be_retried_or_abandoned_out_of_its_pass() {
    let mut run = LessonRun::new(1).unwrap();
    run.report(clear(1, 0, 4));
    assert_eq!(run.retry(), Err(RetryError::Passed));
    assert!(!run.abandon(), "quitting after the pass does not unpass it");
    assert_eq!(run.phase(), LessonPhase::Passed);
    assert!(run.take_pass().is_some(), "the unclaimed pass survives");
}

#[test]
fn quitting_never_yields_a_pass_and_cannot_be_retried() {
    let mut run = LessonRun::new(2).unwrap();
    run.report(clear(1, 0, 4));
    assert!(run.abandon());
    assert_eq!(run.phase(), LessonPhase::Abandoned);
    assert_eq!(run.take_pass(), None);
    assert_eq!(run.retry(), Err(RetryError::Abandoned));
    assert_eq!(
        run.report(clear(1, 1, 4)),
        ReportEffect::Stale(StaleReason::NotRunning)
    );
    assert_eq!(run.take_pass(), None);
    assert!(!run.abandon(), "already abandoned");
}

#[test]
fn quitting_from_a_failure_is_abandoned_not_passed() {
    let mut run = LessonRun::new(2).unwrap();
    run.report(fail(1, 0, LegFailure::Objective));
    assert!(run.abandon());
    assert_eq!(run.take_pass(), None);
}

#[test]
fn stale_reports_are_counted_across_attempts() {
    let mut run = LessonRun::new(2).unwrap();
    run.report(clear(1, 1, 1));
    run.report(fail(1, 0, LegFailure::TimedOut));
    run.retry().unwrap();
    run.report(clear(1, 0, 1));
    assert_eq!(run.stale_reports(), 2);
}

fn result_id() -> ResultId {
    ResultId {
        generation: 1,
        participant: PlayerId(0),
        event: None,
        sequence: 0,
    }
}

#[test]
fn a_gate_run_leg_clears_on_its_finish_and_fails_on_its_deadline() {
    let finished = ParticipantState::Finished {
        race_ticks: 480,
        result: result_id(),
    };
    assert_eq!(
        LegReport::from_gate_run(2, 1, &finished, false),
        Some(clear(2, 1, 480))
    );
    let expired = ParticipantState::TimedOut {
        race_ticks: 600,
        result: result_id(),
    };
    assert_eq!(
        LegReport::from_gate_run(2, 1, &expired, false),
        Some(fail(2, 1, LegFailure::TimedOut))
    );
}

#[test]
fn an_undecided_gate_run_reports_nothing_until_the_car_is_disabled() {
    for state in [ParticipantState::AwaitingStart, ParticipantState::Racing] {
        assert_eq!(LegReport::from_gate_run(1, 0, &state, false), None);
        assert_eq!(
            LegReport::from_gate_run(1, 0, &state, true),
            Some(fail(1, 0, LegFailure::Disabled)),
            "a disabled car fails the leg it is still racing"
        );
    }
}

#[test]
fn a_finish_is_not_undone_by_a_late_disable() {
    let finished = ParticipantState::Finished {
        race_ticks: 90,
        result: result_id(),
    };
    assert_eq!(
        LegReport::from_gate_run(1, 0, &finished, true),
        Some(clear(1, 0, 90))
    );
}

#[test]
fn gate_run_verdicts_drive_a_lesson_through_a_failed_attempt_and_a_pass() {
    let mut run = LessonRun::new(2).unwrap();
    let finish = |ticks| ParticipantState::Finished {
        race_ticks: ticks,
        result: result_id(),
    };
    let leg0 = LegReport::from_gate_run(run.attempt(), 0, &finish(100), false).unwrap();
    assert_eq!(run.report(leg0), ReportEffect::Advanced { next_leg: 1 });
    let wrecked =
        LegReport::from_gate_run(run.attempt(), 1, &ParticipantState::Racing, true).unwrap();
    assert_eq!(run.report(wrecked), ReportEffect::Failed);
    assert_eq!(run.take_pass(), None);
    let attempt = run.retry().unwrap();
    for (leg, ticks) in [(0, 110), (1, 200)] {
        let report = LegReport::from_gate_run(attempt, leg, &finish(ticks), false).unwrap();
        run.report(report);
    }
    assert_eq!(
        run.take_pass(),
        Some(LessonPass {
            attempt: 2,
            leg_ticks: vec![110, 200],
        })
    );
}

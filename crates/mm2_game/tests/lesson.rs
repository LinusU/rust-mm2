//! F21-B.2 contract tests: the lesson leg sequencer — strict authored
//! order, stale/duplicate rejection, whole-lesson retry with cleared
//! counters, and a pass that is credited exactly once.

use mm2_game::{
    LegFailure, LegReport, LegResult, LessonPass, LessonPhase, LessonRun, NoLegs, ReportEffect,
    RetryError, StaleReason,
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

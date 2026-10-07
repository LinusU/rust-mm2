//! Crash Course lesson sequencing (F21-B.2): which leg a lesson is on,
//! what ends it, and how it is retried — independent of how a leg's
//! pass/fail is decided.
//!
//! A lesson is a sequence of legs (`mm2_content::lesson_legs`; the exams
//! chain two or three). [`LessonRun`] is the pure state machine over
//! them. Whatever evaluates a leg — the shared gate-run race runtime
//! today, the per-family evaluators when UNK-35 is recovered — reports a
//! [`LegReport`]; the run decides what that means for the lesson:
//!
//! - **Order** (designed, `DSN-73`): legs clear strictly in authored
//!   order. A report for any other leg, or for a superseded attempt, is
//!   [`Stale`](ReportEffect::Stale) — counted, never applied — so a
//!   duplicated or late delivery cannot skip a leg, fail a fresh
//!   attempt or pass the lesson twice.
//! - **Retry** (designed): a failure ends the attempt, and
//!   [`LessonRun::retry`] restarts the *whole lesson* from leg 0 with
//!   every per-attempt counter cleared — the same "the lesson restarts
//!   from the beginning" reading [`DisabledOutcome::RestartEvent`]
//!   (`DMG-2`) already takes for a wrecked Crash Course car. Whether
//!   the original resumes at the failed exam leg is not recovered.
//! - **Credit once**: [`LessonRun::take_pass`] yields the pass exactly
//!   once. A failed, abandoned or still-running lesson yields nothing,
//!   so a quit or a duplicate delivery cannot reach the reward path.
//!   A passed lesson is terminal; replaying it (`CC-4`) is a new run.
//!
//! What a leg *requires* to count as cleared is not decided here —
//! that is the family evaluators' job and stays open (UNK-35).
//!
//! [`DisabledOutcome::RestartEvent`]: crate::DisabledOutcome::RestartEvent

use std::fmt;

use crate::ParticipantState;

/// Why a leg ended the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegFailure {
    /// The leg's time limit expired with the objective open.
    TimedOut,
    /// The vehicle was disabled (`DMG-2`: a Crash Course lesson
    /// restarts rather than waiting out a penalty).
    Disabled,
    /// The leg's own rule failed — a family evaluator's verdict.
    Objective,
}

/// A leg's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegResult {
    /// The leg was cleared in this many ticks on its own clock.
    Cleared {
        /// Ticks from the leg's start to its clear.
        ticks: u64,
    },
    /// The leg ended the attempt.
    Failed(LegFailure),
}

/// A verdict addressed to one leg of one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegReport {
    /// The attempt the leg ran in — see [`LessonRun::attempt`].
    pub attempt: u32,
    /// The leg's zero-based index in the lesson.
    pub leg: u32,
    /// What happened.
    pub result: LegResult,
}

impl LegReport {
    /// The gate-run baseline's verdict (`DSN-72`): read one leg's
    /// participant the way the shared race runtime ends it — reaching
    /// the last gate clears the leg on its race clock, the time limit
    /// expiring fails it, and a vehicle disabled while still racing
    /// fails it (`DMG-2`: a lesson restarts). `None` while the leg is
    /// still undecided.
    ///
    /// A terminal race state wins over `disabled`: a finish landing
    /// as the car is wrecked is still a finish (the same inclusive
    /// reading `DSN-7` gives the deadline). This is the *baseline*
    /// verdict, not a family evaluator's — what a cornering, follow,
    /// stop, jump or cop-chase leg additionally requires is open
    /// (`UNK-35`).
    pub fn from_gate_run(
        attempt: u32,
        leg: u32,
        state: &ParticipantState,
        disabled: bool,
    ) -> Option<Self> {
        let result = match state {
            ParticipantState::Finished { race_ticks, .. } => {
                LegResult::Cleared { ticks: *race_ticks }
            }
            ParticipantState::TimedOut { .. } => LegResult::Failed(LegFailure::TimedOut),
            ParticipantState::AwaitingStart | ParticipantState::Racing if disabled => {
                LegResult::Failed(LegFailure::Disabled)
            }
            ParticipantState::AwaitingStart | ParticipantState::Racing => return None,
        };
        Some(Self {
            attempt,
            leg,
            result,
        })
    }
}

/// Where a lesson is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonPhase {
    /// Running this leg.
    Running {
        /// Zero-based index of the leg in play.
        leg: u32,
    },
    /// Every leg cleared. Terminal.
    Passed,
    /// A leg ended the attempt; [`LessonRun::retry`] or
    /// [`LessonRun::abandon`] follow.
    Failed {
        /// The leg that ended it.
        leg: u32,
        /// Why.
        failure: LegFailure,
    },
    /// The player left the lesson. Terminal.
    Abandoned,
}

/// Why a report was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// Addressed to an earlier (or never-started) attempt.
    Attempt,
    /// Addressed to a leg that is not the one in play — already
    /// cleared, or not reached yet.
    Leg,
    /// The lesson is no longer running.
    NotRunning,
}

/// What a report did to the lesson.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportEffect {
    /// The leg cleared; the next one is in play.
    Advanced {
        /// The leg now in play.
        next_leg: u32,
    },
    /// The last leg cleared; the lesson is passed.
    Passed,
    /// The leg ended the attempt.
    Failed,
    /// Ignored — counted in [`LessonRun::stale_reports`].
    Stale(StaleReason),
}

/// Why a lesson cannot be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryError {
    /// A passed lesson is done; replaying it is a new run.
    Passed,
    /// The player left; a new run starts a new lesson.
    Abandoned,
}

impl fmt::Display for RetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Passed => "the lesson is already passed",
            Self::Abandoned => "the lesson was abandoned",
        })
    }
}

impl std::error::Error for RetryError {}

/// A lesson with no legs has nothing to pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoLegs;

impl fmt::Display for NoLegs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a lesson needs at least one leg")
    }
}

impl std::error::Error for NoLegs {}

/// A pass, handed out once by [`LessonRun::take_pass`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LessonPass {
    /// The attempt that passed (1 for a first-try pass).
    pub attempt: u32,
    /// Each leg's clear time, in authored order.
    pub leg_ticks: Vec<u64>,
}

impl LessonPass {
    /// The legs' clear times summed.
    pub fn total_ticks(&self) -> u64 {
        self.leg_ticks.iter().sum()
    }
}

/// The leg-sequencing state of one lesson run.
#[derive(Debug, Clone)]
pub struct LessonRun {
    legs: u32,
    attempt: u32,
    phase: LessonPhase,
    /// Clear times of the legs cleared this attempt, in order.
    cleared: Vec<u64>,
    stale: u32,
    credited: bool,
}

impl LessonRun {
    /// A run over `legs` legs, on its first attempt at leg 0.
    pub fn new(legs: usize) -> Result<Self, NoLegs> {
        let legs = u32::try_from(legs).map_err(|_| NoLegs)?;
        if legs == 0 {
            return Err(NoLegs);
        }
        Ok(Self {
            legs,
            attempt: 1,
            phase: LessonPhase::Running { leg: 0 },
            cleared: Vec::new(),
            stale: 0,
            credited: false,
        })
    }

    /// How many legs the lesson has.
    pub fn leg_count(&self) -> u32 {
        self.legs
    }

    /// The attempt in play, counting from 1; [`retry`](Self::retry)
    /// increments it. Reports must carry it.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Where the lesson is.
    pub fn phase(&self) -> LessonPhase {
        self.phase
    }

    /// The leg in play, while running.
    pub fn current_leg(&self) -> Option<u32> {
        match self.phase {
            LessonPhase::Running { leg } => Some(leg),
            _ => None,
        }
    }

    /// Clear times of the legs cleared this attempt, in order. Empty
    /// after a [`retry`](Self::retry).
    pub fn cleared(&self) -> &[u64] {
        &self.cleared
    }

    /// Reports ignored as stale so far — across attempts, so a
    /// duplicating transport stays visible rather than silent.
    pub fn stale_reports(&self) -> u32 {
        self.stale
    }

    /// Apply one leg's verdict.
    pub fn report(&mut self, report: LegReport) -> ReportEffect {
        let LessonPhase::Running { leg } = self.phase else {
            return self.ignore(StaleReason::NotRunning);
        };
        if report.attempt != self.attempt {
            return self.ignore(StaleReason::Attempt);
        }
        if report.leg != leg {
            return self.ignore(StaleReason::Leg);
        }
        match report.result {
            LegResult::Cleared { ticks } => {
                self.cleared.push(ticks);
                if leg + 1 == self.legs {
                    self.phase = LessonPhase::Passed;
                    ReportEffect::Passed
                } else {
                    self.phase = LessonPhase::Running { leg: leg + 1 };
                    ReportEffect::Advanced { next_leg: leg + 1 }
                }
            }
            LegResult::Failed(failure) => {
                self.phase = LessonPhase::Failed { leg, failure };
                ReportEffect::Failed
            }
        }
    }

    fn ignore(&mut self, reason: StaleReason) -> ReportEffect {
        self.stale = self.stale.saturating_add(1);
        ReportEffect::Stale(reason)
    }

    /// Restart the whole lesson from leg 0 as the next attempt, with
    /// the cleared legs forgotten. Allowed from a failure and from a
    /// running lesson (the player restarting mid-lesson, or `DMG-2`'s
    /// restart); returns the new attempt number. Reports still
    /// addressed to the old attempt go stale.
    pub fn retry(&mut self) -> Result<u32, RetryError> {
        match self.phase {
            LessonPhase::Passed => return Err(RetryError::Passed),
            LessonPhase::Abandoned => return Err(RetryError::Abandoned),
            LessonPhase::Running { .. } | LessonPhase::Failed { .. } => {}
        }
        self.attempt = self.attempt.saturating_add(1);
        self.cleared.clear();
        self.phase = LessonPhase::Running { leg: 0 };
        Ok(self.attempt)
    }

    /// Leave the lesson. A running or failed lesson becomes
    /// [`Abandoned`](LessonPhase::Abandoned); a passed one stays
    /// passed (its pass may still be unclaimed). Returns whether the
    /// phase changed.
    pub fn abandon(&mut self) -> bool {
        match self.phase {
            LessonPhase::Running { .. } | LessonPhase::Failed { .. } => {
                self.phase = LessonPhase::Abandoned;
                true
            }
            LessonPhase::Passed | LessonPhase::Abandoned => false,
        }
    }

    /// The pass, once: `Some` the first time it is asked for after the
    /// lesson passes, `None` before that and on every later call. This
    /// is the only way to a reward — failure, quit and duplicate
    /// reports never produce one.
    pub fn take_pass(&mut self) -> Option<LessonPass> {
        if self.phase != LessonPhase::Passed || self.credited {
            return None;
        }
        self.credited = true;
        Some(LessonPass {
            attempt: self.attempt,
            leg_ticks: self.cleared.clone(),
        })
    }
}

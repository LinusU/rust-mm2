//! The in-session driver for a Crash Course lesson (F21-B.4, `DSN-75`).
//!
//! A lesson is a sequence of legs, each a [`RaceDefinition`] the shared
//! race runtime already knows how to run. [`LessonDriver`] is the
//! session resource that holds the legs and the sequencer
//! ([`LessonRun`]); [`drive_lesson`] is the fixed-step system that
//! feeds each leg's outcome back into it and acts on the answer:
//!
//! - leg cleared, more to go: the next leg's definition replaces
//!   [`RaceState`] (fresh countdown, fresh clock), the participant's
//!   [`RaceProgress`] is rebuilt for its gates, the old gate markers are
//!   despawned and the new ones spawned, and the car is reseated on the
//!   new leg's start slot through the same [`ResetVehicle`] teleport
//!   every other reset takes (so the swept segment re-anchors rather
//!   than crossing a gate across the jump);
//! - last leg cleared: the pass is claimed once and stored on the
//!   driver ([`LessonDriver::pass`]); the race's own `Playing → Results`
//!   edge ends the session as for any event;
//! - a failure (the leg's time limit): nothing is replaced — the race
//!   runtime ends the session at `Results` like a failed event. The
//!   retry is the session's own restart, which tears the world down and
//!   rebuilds the driver on leg 0 with cleared counters (the `DMG-2`
//!   reading `DSN-73` encodes). A wrecked car already restarts the
//!   session through `resolve_disabled`'s `RestartEvent`, so this
//!   driver never sees a "disabled" verdict.
//!
//! [`advance_race`](crate::race::advance_race) consults
//! [`LessonDriver::holds_results`] so clearing a *non-final* leg does
//! not end the session; only the lesson's last clear or a failure does.
//!
//! The verdict is the gate-run baseline of [`LegReport::from_gate_run`]
//! — what a cornering, follow, stop, jump or cop-chase leg additionally
//! requires is `UNK-35`. The driver is session state: teardown removes it
//! with the [`RaceState`], and while it is present
//! `record_session_results` keeps every leg's ledger entry off the
//! profile (a leg clear is not an event finish) and credits the
//! lesson's own key once, from [`LessonDriver::pass`] alone (F21-B.9).

use bevy::prelude::*;
use mm2_game::{
    EventKey, LegFailure, LegReport, LessonPass, LessonPhase, LessonRun, ParticipantState, Player,
    PlayerControl, RaceProgress, RaceState, ReportEffect, Session, SessionEntity, SessionPhase,
    StaleReason,
};
use mm2_vehicle::ResetVehicle;

use crate::netdrive::seat_pose;
use crate::race::{CheckpointMarker, LessonSetup, spawn_checkpoint_markers};
use crate::session::SpawnPoint;

/// What one observation of the current leg did to the lesson.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegStep {
    /// The leg is undecided, or the lesson is no longer running.
    Hold,
    /// The leg cleared and `leg` is now in play.
    Next {
        /// The leg now in play.
        leg: u32,
    },
    /// The last leg cleared; [`LessonDriver::pass`] holds the pass.
    Passed,
    /// The leg ended the attempt.
    Failed {
        /// The leg that failed.
        leg: u32,
        /// Why.
        failure: LegFailure,
    },
    /// The sequencer refused the report (counted there, never applied).
    Stale(StaleReason),
}

/// The session resource carrying one lesson run: the legs the race
/// runtime steps through and the sequencer that decides when the
/// lesson is over. Inserted by whatever launches a lesson session;
/// removed with the session like [`RaceState`].
#[derive(Resource, Debug)]
pub struct LessonDriver {
    key: EventKey,
    legs: Vec<mm2_content::LessonLeg>,
    run: LessonRun,
    pass: Option<LessonPass>,
}

impl LessonDriver {
    /// A driver over `setup`'s legs, on leg 0 of the first attempt.
    pub fn new(setup: LessonSetup) -> Self {
        Self {
            key: setup.key,
            legs: setup.legs,
            run: setup.run,
            pass: None,
        }
    }

    /// The lesson's stable save identity.
    pub fn key(&self) -> &EventKey {
        &self.key
    }

    /// The sequencer, read-only — phase, attempt, cleared times.
    pub fn run(&self) -> &LessonRun {
        &self.run
    }

    /// The leg in play, while the lesson runs.
    pub fn current_leg(&self) -> Option<&mm2_content::LessonLeg> {
        self.run
            .current_leg()
            .and_then(|i| self.legs.get(i as usize))
    }

    /// The lesson's pass, once the last leg has cleared. Claimed from
    /// the sequencer exactly once; this keeps it for the results flow.
    pub fn pass(&self) -> Option<&LessonPass> {
        self.pass.as_ref()
    }

    /// The race for the leg in play, in countdown — the resource the
    /// session installs at load (leg 0) and the driver installs on
    /// every advance. `None` once the lesson is no longer running.
    pub fn race_state(&self, generation: u64) -> Option<RaceState> {
        self.current_leg()
            .map(|leg| RaceState::new(leg.definition.clone(), generation))
    }

    /// Whether a participant's `state` is a clear of a leg that is not
    /// the lesson's last — one the session must not end on.
    pub fn holds_results(&self, state: &ParticipantState) -> bool {
        matches!(state, ParticipantState::Finished { .. })
            && self
                .run
                .current_leg()
                .is_some_and(|leg| leg + 1 < self.run.leg_count())
    }

    /// Read the local participant's `state` for the leg in play and
    /// apply the verdict to the sequencer. Idempotent once the lesson
    /// stops running: a failed or passed lesson answers
    /// [`LegStep::Hold`] instead of counting every later tick as a
    /// stale report.
    pub fn observe(&mut self, state: &ParticipantState) -> LegStep {
        let Some(leg) = self.run.current_leg() else {
            return LegStep::Hold;
        };
        // `disabled` is always false here: a wrecked lesson car
        // restarts the whole session (`DisabledOutcome::RestartEvent`)
        // before it could be reported as a failure of this attempt.
        let Some(report) = LegReport::from_gate_run(self.run.attempt(), leg, state, false) else {
            return LegStep::Hold;
        };
        match self.run.report(report) {
            ReportEffect::Advanced { next_leg } => LegStep::Next { leg: next_leg },
            ReportEffect::Passed => {
                self.pass = self.run.take_pass();
                LegStep::Passed
            }
            ReportEffect::Failed => match self.run.phase() {
                LessonPhase::Failed { leg, failure } => LegStep::Failed { leg, failure },
                // `Failed` is only ever answered with that phase.
                _ => LegStep::Hold,
            },
            ReportEffect::Stale(reason) => LegStep::Stale(reason),
        }
    }
}

/// Fixed-step lesson driver — see module docs. Runs in `FixedLast`
/// right after [`advance_race`](crate::race::advance_race), so the
/// leg's terminal state is read on the tick it lands.
#[allow(clippy::too_many_arguments)] // Bevy system: the leg swap threads the race, the markers' asset handles and the reseat channel
pub fn drive_lesson(
    driver: Option<ResMut<LessonDriver>>,
    race: Option<ResMut<RaceState>>,
    session: Res<Session>,
    spawn: Option<Res<SpawnPoint>>,
    mut participants: Query<(Entity, &Player, &mut RaceProgress)>,
    markers: Query<Entity, With<CheckpointMarker>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (Some(mut driver), Some(mut race)) = (driver, race) else {
        return;
    };
    if race.is_stale(session.generation()) || !session.authority_role().is_authority() {
        return;
    }
    if !matches!(
        *session.phase(),
        SessionPhase::Playing | SessionPhase::Results
    ) {
        return;
    }
    let Some((entity, _, mut progress)) = participants
        .iter_mut()
        .find(|(_, player, _)| player.control == PlayerControl::Local)
    else {
        return;
    };
    match driver.observe(&progress.state) {
        LegStep::Next { leg } => {
            let Some(next) = driver.race_state(session.generation()) else {
                return;
            };
            info!(
                lesson = %driver.key.stem,
                leg,
                filename = %driver.legs[leg as usize].filename,
                "lesson leg cleared — next leg"
            );
            // The old leg's gates go; the new leg's gates spawn.
            for marker in &markers {
                commands.entity(marker).despawn();
            }
            spawn_checkpoint_markers(
                &mut commands,
                &mut meshes,
                &mut images,
                &mut materials,
                &next.definition,
                SessionEntity(session.generation()),
            );
            *progress = RaceProgress::new(&next.definition);
            // Reseat on the leg's own start slot — the roam base is
            // the fallback a slotless leg degrades to.
            let base = spawn
                .as_ref()
                .map_or((Vec3::ZERO, 0.0), |s| (s.origin, s.origin_yaw));
            let (position, yaw) = seat_pose(Some(&next.definition), base, 0);
            resets.write(ResetVehicle {
                entity: Some(entity),
                position,
                yaw,
            });
            *race = next;
        }
        LegStep::Passed => info!(lesson = %driver.key.stem, "lesson passed"),
        LegStep::Failed { leg, failure } => warn!(
            lesson = %driver.key.stem,
            leg,
            ?failure,
            "lesson failed — restart to retry from the first leg"
        ),
        LegStep::Hold | LegStep::Stale(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_content::{LessonLeg, LessonObjective};
    use mm2_game::{
        Checkpoint, CheckpointRule, EventTableKind, PlayerId, RaceDefinition, ResultId,
    };

    fn leg(name: &str, limit: Option<u32>) -> LessonLeg {
        LessonLeg {
            filename: name.into(),
            objective: LessonObjective::Maneuver,
            source: format!("crash0:{name}"),
            definition: RaceDefinition {
                checkpoints: vec![Checkpoint {
                    center: Vec3::ZERO,
                    radius: 10.0,
                    height: 5.0,
                    heading_deg: 0.0,
                    require_direction: false,
                }],
                finish: None,
                rule: CheckpointRule::Ordered,
                laps: 1,
                time_limit_ticks: limit,
                params: mm2_game::EventParams::default(),
                countdown_ticks: 3,
                start_slots: Vec::new(),
            },
        }
    }

    fn driver(legs: usize) -> LessonDriver {
        LessonDriver {
            key: EventKey {
                city: "london".into(),
                table: EventTableKind::CrashCourse,
                stem: "lesson1".into(),
            },
            legs: (0..legs).map(|i| leg(&format!("leg{i}"), None)).collect(),
            run: LessonRun::new(legs).unwrap(),
            pass: None,
        }
    }

    fn id() -> ResultId {
        ResultId {
            generation: 1,
            participant: PlayerId(0),
            event: None,
            sequence: 0,
        }
    }

    fn finished(ticks: u64) -> ParticipantState {
        ParticipantState::Finished {
            race_ticks: ticks,
            result: id(),
        }
    }

    #[test]
    fn an_undecided_leg_holds() {
        let mut d = driver(2);
        assert_eq!(d.observe(&ParticipantState::Racing), LegStep::Hold);
        assert_eq!(d.observe(&ParticipantState::AwaitingStart), LegStep::Hold);
        assert_eq!(d.run().stale_reports(), 0);
    }

    #[test]
    fn a_clear_advances_and_the_last_clear_yields_the_pass_once() {
        let mut d = driver(2);
        assert_eq!(d.observe(&finished(100)), LegStep::Next { leg: 1 });
        assert_eq!(d.current_leg().unwrap().filename, "leg1");
        assert_eq!(d.observe(&finished(250)), LegStep::Passed);
        assert_eq!(d.pass().unwrap().leg_ticks, vec![100, 250]);
        // Nothing runs afterwards — no stale-report noise per tick.
        assert_eq!(d.observe(&finished(250)), LegStep::Hold);
        assert_eq!(d.run().stale_reports(), 0);
        assert!(d.race_state(1).is_none());
    }

    #[test]
    fn a_timeout_fails_the_attempt_and_stays_failed() {
        let mut d = driver(3);
        let timed_out = ParticipantState::TimedOut {
            race_ticks: 60,
            result: id(),
        };
        assert_eq!(
            d.observe(&timed_out),
            LegStep::Failed {
                leg: 0,
                failure: LegFailure::TimedOut
            }
        );
        assert_eq!(d.observe(&timed_out), LegStep::Hold);
        assert!(d.pass().is_none());
        assert!(d.race_state(1).is_none());
    }

    #[test]
    fn only_a_non_final_clear_holds_the_results_screen() {
        let mut d = driver(2);
        assert!(d.holds_results(&finished(1)));
        assert!(!d.holds_results(&ParticipantState::Racing));
        assert!(!d.holds_results(&ParticipantState::TimedOut {
            race_ticks: 1,
            result: id()
        }));
        d.observe(&finished(1));
        // On the final leg a finish ends the session as usual.
        assert!(!d.holds_results(&finished(2)));
    }

    #[test]
    fn the_race_state_follows_the_leg_in_play() {
        let mut d = driver(2);
        d.legs[1].definition.time_limit_ticks = Some(500);
        let first = d.race_state(7).unwrap();
        assert_eq!(first.generation, 7);
        assert_eq!(first.definition.time_limit_ticks, None);
        d.observe(&finished(1));
        let second = d.race_state(7).unwrap();
        assert_eq!(second.definition.time_limit_ticks, Some(500));
    }
}

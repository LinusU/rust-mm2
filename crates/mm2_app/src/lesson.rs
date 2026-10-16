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
//!   reading `DSN-73` encodes). A wrecked car on a jump, cross-traffic,
//!   corner, maneuver or map leg restarts the session through
//!   `resolve_disabled`'s `RestartEvent`, so this driver never sees a
//!   "disabled" verdict; on a follow, cop-chase or stop leg
//!   ([`LessonDriver::wreck_policy`]) the car is instead reset in place
//!   after 3 s (`resolve_lesson_wreck`) and the leg goes on.
//!
//! [`advance_race`](crate::race::advance_race) consults
//! [`LessonDriver::holds_results`] so clearing a *non-final* leg does
//! not end the session; only the lesson's last clear or a failure does.
//!
//! For a leg whose family has a recovered rule ([`mm2_game::LegRule`],
//! `CC-9` — follow, cop chase, corner, stop) the driver evaluates that
//! rule each tick from a [`LegObservation`] the session builds
//! ([`drive_lesson`]) and reports its verdict through
//! [`LegReport::from_verdict`]; the gate-run baseline
//! ([`LegReport::from_gate_run`]) stays the verdict for jump,
//! cross-traffic, maneuver, map and any leg whose family is unresolved.
//! A rule that cannot be evaluated (a missing lead car, a non-finite
//! observation) is surfaced as [`LegStep::RuleError`], never a pass.
//!
//! The driver is session state: teardown removes it
//! with the [`RaceState`], and while it is present
//! `record_session_results` keeps every leg's ledger entry off the
//! profile (a leg clear is not an event finish) and credits the
//! lesson's own key once, from [`LessonDriver::pass`] alone (F21-B.9).

use avian3d::prelude::{Position, SpatialQuery};
use bevy::prelude::*;
use mm2_content::LessonObjective;
use mm2_content::lesson_text::LessonLine;
use mm2_game::{
    DamageTier, EventKey, LeadObservation, LegFailure, LegObservation, LegReport, LegRule,
    LessonPass, LessonPhase, LessonRun, MPH_PER_MS, PURSUER_SIGHT_RANGE_M, ParticipantState,
    Player, PlayerControl, RACE_TICK_HZ, RacePhase, RaceProgress, RaceState, ReportEffect,
    RuleError, RuleState, STOP_LEAD_MAX_DAMAGE, Session, SessionEntity, SessionPhase, StaleReason,
    VehicleDamage, WreckPolicy,
};
use mm2_vehicle::{ResetVehicle, VehicleState};

use crate::audio::{CommentaryAudio, EventCue};
use crate::netdrive::seat_pose;
use crate::opponents::{LeadCar, OpponentDriver, lead_route_finished};
use crate::police::{PoliceCar, cop_sight_clear};
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
    /// The leg's family rule could not be evaluated — a setup defect (a
    /// follow or stop leg with no lead car) or a non-finite observation.
    /// Surfaced, never a pass; the leg holds this tick.
    RuleError(RuleError),
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
    /// Memory a family rule keeps across the ticks of one leg (the
    /// corner rule's speed bookkeeping). Reset on every leg advance.
    rule_state: RuleState,
    /// Whether the stop leg in play has had its lesson damage bounds
    /// applied to the lead (the original sets `MaxDamage` once, when the
    /// race goes). Reset on every leg advance.
    stop_lead_armed: bool,
    /// The leg in play's cleared-gate mask at the last observation — the
    /// edge base for the per-gate location call-outs (F21-B). Reset on
    /// every leg advance, like the rule state.
    callout_mask: u64,
    /// Course rows a cleared gate has named, awaiting the call-out voice
    /// ([`lesson_callout_cue`]). Appended by [`drive_lesson`] on the
    /// clearing tick and drained the same frame.
    pending_callouts: Vec<usize>,
    /// Message lines a leg's verdict called for, awaiting the HUD
    /// ([`crate::lesson_text::drive_lesson_subtitles`]). Appended by
    /// [`drive_lesson`] on the verdict's tick and drained the same frame.
    pending_lines: Vec<(LessonObjective, LessonLine)>,
}

impl LessonDriver {
    /// A driver over `setup`'s legs, on leg 0 of the first attempt.
    pub fn new(setup: LessonSetup) -> Self {
        Self {
            key: setup.key,
            legs: setup.legs,
            run: setup.run,
            pass: None,
            rule_state: RuleState::default(),
            stop_lead_armed: false,
            callout_mask: 0,
            pending_callouts: Vec::new(),
            pending_lines: Vec::new(),
        }
    }

    /// Drain the message lines the legs' verdicts have called for.
    pub fn take_lines(&mut self) -> Vec<(LessonObjective, LessonLine)> {
        std::mem::take(&mut self.pending_lines)
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

    /// Leg `index` of the lesson, in authored order.
    pub fn leg(&self, index: usize) -> Option<&mm2_content::LessonLeg> {
        self.legs.get(index)
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

    /// What a wrecked player car does to the leg in play (`CC-9`):
    /// `None` once the lesson is no longer running. A leg whose family
    /// is unresolved runs the gate-run baseline, which fails on a wreck.
    pub fn wreck_policy(&self) -> Option<WreckPolicy> {
        let leg = self.run.current_leg()?;
        Some(
            self.rule_of(leg)
                .map_or(WreckPolicy::FailLeg, |rule| rule.wreck_policy()),
        )
    }

    /// The rule the leg in play runs, `None` for an unresolved family.
    fn rule_of(&self, leg: u32) -> Option<LegRule> {
        self.legs.get(leg as usize).and_then(|l| l.rule)
    }

    /// Whether a participant's `state` is a clear of a leg the session
    /// must not end on.
    ///
    /// A non-final clear always carries on into the next leg. On the
    /// *final* leg a gate-run clear ends the lesson as usual — but a
    /// leg whose family rule is not [`LegRule::GateRun`] is decided by
    /// that rule, so the gate run completing only supplies
    /// `course_complete` and the session keeps playing until the rule
    /// itself clears (then `current_leg` is `None`, so this is `false`).
    pub fn holds_results(&self, state: &ParticipantState) -> bool {
        if !matches!(state, ParticipantState::Finished { .. }) {
            return false;
        }
        match self.run.current_leg() {
            Some(leg) if leg + 1 < self.run.leg_count() => true,
            Some(leg) => matches!(self.rule_of(leg), Some(rule) if rule != LegRule::GateRun),
            None => false,
        }
    }

    /// Whether the lesson reached a terminal phase — passed or failed.
    /// A family rule can end a leg while the local car still races (a
    /// follow escape, a corner under-speed, a stop whose lead arrived),
    /// so the session's `Playing → Results` edge keys on this as well as
    /// on the participant resolving through the race runtime.
    pub fn is_over(&self) -> bool {
        matches!(
            self.run.phase(),
            LessonPhase::Passed | LessonPhase::Failed { .. }
        )
    }

    /// Whether a stop leg's lead still needs the lesson's damage bounds
    /// applied — the original sets the lead's `MaxDamage` once, when the
    /// race goes (`0x4198a3`, `CC-9`). Answers `true` once per stop leg,
    /// then latches until the leg advances; `false` for every other leg.
    pub fn arm_stop_lead(&mut self) -> bool {
        if self.stop_lead_armed {
            return false;
        }
        let running = self
            .run
            .current_leg()
            .is_some_and(|leg| matches!(self.rule_of(leg), Some(LegRule::Stop)));
        if running {
            self.stop_lead_armed = true;
        }
        running
    }

    /// Read the local participant's `state` and the leg's `obs` for the
    /// leg in play and apply the verdict to the sequencer. `ticks` is
    /// the leg's own clock, the clear time [`LegReport`] carries for a
    /// rule verdict.
    ///
    /// For a family with a recovered rule ([`LegRule`] other than
    /// [`LegRule::GateRun`]) the rule decides the leg; every other leg
    /// keeps the gate-run baseline. Idempotent once the lesson stops
    /// running: a failed or passed lesson answers [`LegStep::Hold`]
    /// instead of counting every later tick as a stale report.
    pub fn observe(
        &mut self,
        state: &ParticipantState,
        obs: &LegObservation,
        ticks: u64,
    ) -> LegStep {
        let Some(leg) = self.run.current_leg() else {
            return LegStep::Hold;
        };
        // A participant that has not been released yet has no verdict:
        // a follow leg's distances are meaningless mid-countdown.
        if matches!(state, ParticipantState::AwaitingStart) {
            return LegStep::Hold;
        }
        match self.rule_of(leg) {
            Some(rule) if rule != LegRule::GateRun => {
                let dt = 1.0 / RACE_TICK_HZ as f32;
                let verdict = match rule.evaluate(&mut self.rule_state, obs, dt) {
                    Ok(verdict) => verdict,
                    Err(error) => return LegStep::RuleError(error),
                };
                let Some(report) = LegReport::from_verdict(self.run.attempt(), leg, ticks, verdict)
                else {
                    // Pending, or the wreck-recovery policy that does not
                    // end the leg (`ResetCar`).
                    return LegStep::Hold;
                };
                self.apply(report)
            }
            // The gate-run baseline (`DSN-72`): jump, cross-traffic,
            // maneuver, map — and any leg whose family is unresolved.
            _ => {
                // `disabled` is always false here: a wrecked lesson car
                // restarts the whole session
                // (`DisabledOutcome::RestartEvent`) before it could be
                // reported as a failure of this attempt.
                let Some(report) = LegReport::from_gate_run(self.run.attempt(), leg, state, false)
                else {
                    return LegStep::Hold;
                };
                self.apply(report)
            }
        }
    }

    /// Feed one addressed report to the sequencer and translate its
    /// effect, resetting the per-leg rule state whenever a leg advances.
    fn apply(&mut self, report: LegReport) -> LegStep {
        match self.run.report(report) {
            ReportEffect::Advanced { next_leg } => {
                self.rule_state = RuleState::default();
                self.stop_lead_armed = false;
                self.callout_mask = 0;
                LegStep::Next { leg: next_leg }
            }
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
///
/// Each tick it builds the leg's [`LegObservation`] from the live
/// session — the local car's speed, gates, timer, wreck and position,
/// the lesson's lead car(s) (`F21-B.19`) and the fielded police — and
/// hands it to [`LessonDriver::observe`]. Which observation inputs a
/// leg's rule reads is [`mm2_game::LegRule`]'s business; the follow
/// leg's untimed definition means its `time_expired` is always false.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system: the leg swap threads the race, the markers' asset handles and the reseat channel; the participant query is the system's real input
pub fn drive_lesson(
    driver: Option<ResMut<LessonDriver>>,
    race: Option<ResMut<RaceState>>,
    session: Res<Session>,
    spatial: SpatialQuery,
    spawn: Option<Res<SpawnPoint>>,
    mut participants: Query<(
        Entity,
        &Player,
        &mut RaceProgress,
        &Position,
        &VehicleState,
        Option<&VehicleDamage>,
    )>,
    leads: Query<(&Position, &OpponentDriver, Option<&VehicleDamage>), With<LeadCar>>,
    police: Query<&Position, With<PoliceCar>>,
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
    let Some((entity, _, mut progress, position, vstate, damage)) = participants
        .iter_mut()
        .find(|(_, player, _, _, _, _)| player.control == PlayerControl::Local)
    else {
        return;
    };
    // "The lead" is the nearest marked lead car — retail lessons field
    // at most one (`F21-B.19`), so this is the lesson's lead; a lesson
    // with none (a follow or stop leg wired no opponent) leaves
    // `obs.lead` `None`, which the rule surfaces as a `MissingLead`.
    let mut lead = None;
    let mut nearest_sq = f32::INFINITY;
    for (lead_pos, lead_driver, lead_damage) in &leads {
        let distance_sq = position.0.distance_squared(lead_pos.0);
        if distance_sq < nearest_sq {
            nearest_sq = distance_sq;
            let route_finished = lead_driver
                .route
                .as_ref()
                .is_some_and(|route| lead_route_finished(route, lead_driver.next, lead_pos.0));
            let wrecked = lead_damage.is_some_and(|d| d.condition() == DamageTier::Disabled);
            lead = Some(LeadObservation {
                distance_sq,
                route_finished,
                wrecked,
            });
        }
    }
    let obs = LegObservation {
        course_complete: matches!(progress.state, ParticipantState::Finished { .. }),
        gates_passed: progress.cleared_count() as u32,
        time_expired: matches!(progress.state, ParticipantState::TimedOut { .. }),
        wrecked: damage.is_some_and(|d| d.condition() == DamageTier::Disabled),
        speed_mph: vstate.forward_speed.abs() * MPH_PER_MS,
        lead,
        pursuer_in_sight: police
            .iter()
            .any(|cop| cop_sight_clear(&spatial, cop.0, position.0, PURSUER_SIGHT_RANGE_M)),
    };
    // F21-B: a gate clearing is the edge a lesson's per-gate location
    // call-out rides. Diff the cleared mask against the last
    // observation; checkpoint `j` of a leg's definition is course row
    // `j + 1` (the definition's checkpoints are the waypoint rows after
    // the start row, and the original's `indexinfo` is keyed by course
    // row). Read before `observe`, which resets the base when the leg
    // advances.
    let mask = progress.cleared_mask();
    let mut newly = mask & !driver.callout_mask;
    driver.callout_mask = mask;
    while newly != 0 {
        let gate = newly.trailing_zeros() as usize;
        driver.pending_callouts.push(gate + 1);
        newly &= newly - 1;
    }
    let ticks = race.clock;
    let objective = driver.current_leg().map(|leg| leg.objective);
    let step = driver.observe(&progress.state, &obs, ticks);
    // The verdict's outcome line (F21 req 4): the leg that just ended,
    // read before the sequencer moved on.
    if let Some(objective) = objective {
        let line = match step {
            LegStep::Next { .. } | LegStep::Passed => Some(LessonLine::Cleared),
            LegStep::Failed { failure, .. } => failure_line(objective, failure),
            _ => None,
        };
        if let Some(line) = line {
            driver.pending_lines.push((objective, line));
        }
    }
    match step {
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
        LegStep::RuleError(error) => warn!(
            lesson = %driver.key.stem,
            %error,
            "lesson leg rule could not be evaluated — the leg holds"
        ),
        LegStep::Hold | LegStep::Stale(_) => {}
    }
}

/// The outcome line a failed leg of `objective` shows. A leg's own
/// rule failure is the family's own line: the cop chase's "a cop still
/// sees you", the follow and stop legs' lost-lead / lead-arrived lines;
/// the jump and maneuver families have no rule failure, so none.
fn failure_line(objective: LessonObjective, failure: LegFailure) -> Option<LessonLine> {
    match failure {
        LegFailure::TimedOut => Some(LessonLine::TimedOut),
        LegFailure::Disabled => Some(LessonLine::Wrecked),
        LegFailure::Objective => match objective {
            LessonObjective::CopChase => Some(LessonLine::CopsStillClose),
            LessonObjective::Follow | LessonObjective::Stop => Some(LessonLine::ObjectiveFailed),
            _ => None,
        },
    }
}

/// Apply a stop leg's lesson damage bounds to its lead car (F21-C.3,
/// `CC-9`): when the race goes the original sets the lead's `MaxDamage`
/// to [`STOP_LEAD_MAX_DAMAGE`] and `MedDamage` to half (`0x4198a3`), so
/// reaching the (otherwise far larger) authored bound is what "Destroy
/// that car!" asks. [`LessonDriver::arm_stop_lead`] latches it once per
/// stop leg.
///
/// Runs in `FixedLast` after [`advance_race`](crate::race::advance_race)
/// (the countdown it waits out releases there) and against the
/// authority only. Idle without a lesson, a stop leg, a fielded lead or
/// a released race.
pub fn arm_stop_lead_damage(
    driver: Option<ResMut<LessonDriver>>,
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    mut leads: Query<&mut VehicleDamage, With<LeadCar>>,
) {
    let (Some(mut driver), Some(race)) = (driver, race) else {
        return;
    };
    if race.is_stale(session.generation())
        || !session.authority_role().is_authority()
        || matches!(race.phase, RacePhase::Countdown { .. })
    {
        return;
    }
    if !driver.arm_stop_lead() {
        return;
    }
    for mut damage in &mut leads {
        damage.spec.max_damage = STOP_LEAD_MAX_DAMAGE;
        damage.spec.med_damage = STOP_LEAD_MAX_DAMAGE * 0.5;
    }
}

/// Ask the instructor for the lesson's verdict line (F21-B.16): the
/// sequencer's terminal phase — passed, or an attempt a leg ended —
/// requests the table's `RESULTSWIN` / `RESULTSPOOR` line once.
/// [`CommentaryAudio::request`](crate::audio::CommentaryAudio::request)
/// refuses a second ask, so this can read the phase every frame; a
/// retry rebuilds the driver and the commentary with the session.
pub fn lesson_verdict_cue(
    driver: Option<Res<LessonDriver>>,
    commentary: Option<ResMut<CommentaryAudio>>,
) {
    let (Some(driver), Some(mut commentary)) = (driver, commentary) else {
        return;
    };
    match driver.run().phase() {
        LessonPhase::Passed => {
            commentary.request(EventCue::LessonPass);
        }
        LessonPhase::Failed { .. } => {
            commentary.request(EventCue::LessonFail);
        }
        LessonPhase::Running { .. } | LessonPhase::Abandoned => {}
    }
}

/// Ask the instructor for the per-gate location call-out of every course
/// gate a leg cleared since the last frame (F21-B): [`drive_lesson`]
/// records the passed rows on the driver and this hands them to the
/// commentary right before the queue drains. A lesson whose row authors
/// no call-out tables (every retail row but London's map lesson)
/// declines silently — the tables are simply not bound. A retry rebuilds
/// the driver, so the base resets with the session.
pub fn lesson_callout_cue(
    driver: Option<ResMut<LessonDriver>>,
    commentary: Option<ResMut<CommentaryAudio>>,
) {
    let (Some(mut driver), Some(mut commentary)) = (driver, commentary) else {
        return;
    };
    if driver.pending_callouts.is_empty() {
        return;
    }
    for row in std::mem::take(&mut driver.pending_callouts) {
        commentary.request_callout(row);
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
            rule: Some(mm2_game::LegRule::GateRun),
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
            rule_state: mm2_game::RuleState::default(),
            stop_lead_armed: false,
            callout_mask: 0,
            pending_callouts: Vec::new(),
            pending_lines: Vec::new(),
        }
    }

    #[test]
    fn a_failed_legs_outcome_line_is_the_familys_own() {
        use LessonLine::*;
        assert_eq!(
            failure_line(LessonObjective::Jump, LegFailure::TimedOut),
            Some(TimedOut)
        );
        assert_eq!(
            failure_line(LessonObjective::Jump, LegFailure::Disabled),
            Some(Wrecked)
        );
        assert_eq!(
            failure_line(LessonObjective::CopChase, LegFailure::Objective),
            Some(CopsStillClose)
        );
        assert_eq!(
            failure_line(LessonObjective::Follow, LegFailure::Objective),
            Some(ObjectiveFailed)
        );
        assert_eq!(
            failure_line(LessonObjective::Stop, LegFailure::Objective),
            Some(ObjectiveFailed)
        );
        assert_eq!(
            failure_line(LessonObjective::Maneuver, LegFailure::Objective),
            None
        );
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

    /// Observe a leg with an idle observation — the gate-run baseline
    /// ignores the observation, so these driver-level tests keep
    /// driving it through `state` alone.
    fn observe(d: &mut LessonDriver, state: &ParticipantState) -> LegStep {
        d.observe(state, &LegObservation::IDLE, 0)
    }

    #[test]
    fn an_undecided_leg_holds() {
        let mut d = driver(2);
        assert_eq!(observe(&mut d, &ParticipantState::Racing), LegStep::Hold);
        assert_eq!(
            observe(&mut d, &ParticipantState::AwaitingStart),
            LegStep::Hold
        );
        assert_eq!(d.run().stale_reports(), 0);
    }

    #[test]
    fn a_clear_advances_and_the_last_clear_yields_the_pass_once() {
        let mut d = driver(2);
        assert_eq!(observe(&mut d, &finished(100)), LegStep::Next { leg: 1 });
        assert_eq!(d.current_leg().unwrap().filename, "leg1");
        assert_eq!(observe(&mut d, &finished(250)), LegStep::Passed);
        assert_eq!(d.pass().unwrap().leg_ticks, vec![100, 250]);
        // Nothing runs afterwards — no stale-report noise per tick.
        assert_eq!(observe(&mut d, &finished(250)), LegStep::Hold);
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
            observe(&mut d, &timed_out),
            LegStep::Failed {
                leg: 0,
                failure: LegFailure::TimedOut
            }
        );
        assert_eq!(observe(&mut d, &timed_out), LegStep::Hold);
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
        observe(&mut d, &finished(1));
        // On the final leg a finish ends the session as usual.
        assert!(!d.holds_results(&finished(2)));
    }

    #[test]
    fn a_family_final_leg_holds_the_results_until_its_rule_clears() {
        let mut d = driver(1);
        d.legs[0].rule = Some(LegRule::Follow);
        // The gate run completing under a family rule does not end the
        // session — `drive_lesson` must decide the leg.
        assert!(d.holds_results(&finished(2)));
        assert!(!d.is_over());
        // The lead finished its route right on top of the player: the
        // rule clears the leg, and then there is nothing left to hold.
        let obs = LegObservation {
            lead: Some(LeadObservation {
                distance_sq: 4.0,
                route_finished: true,
                wrecked: false,
            }),
            ..LegObservation::IDLE
        };
        assert_eq!(d.observe(&finished(2), &obs, 2), LegStep::Passed);
        assert!(d.is_over());
        assert!(!d.holds_results(&finished(2)));
    }

    #[test]
    fn the_race_state_follows_the_leg_in_play() {
        let mut d = driver(2);
        d.legs[1].definition.time_limit_ticks = Some(500);
        let first = d.race_state(7).unwrap();
        assert_eq!(first.generation, 7);
        assert_eq!(first.definition.time_limit_ticks, None);
        observe(&mut d, &finished(1));
        let second = d.race_state(7).unwrap();
        assert_eq!(second.definition.time_limit_ticks, Some(500));
    }
}

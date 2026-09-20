//! The app-side driver for the shared race runtime (F11-B).
//!
//! The contract types live in `mm2_game::race`; [`advance_race`] is the
//! producer that feeds them real positions, because only `mm2_app` may
//! see both the game contracts and Avian. It runs in `FixedLast`, after
//! the physics step, so each `Position` it reads is the step the
//! session clock names — the same swept-segment path tests drive and
//! gameplay uses (AC02).
//!
//! Per fixed step, while a [`RaceState`] resource exists and the
//! session authority simulates rules:
//!
//! - session `Countdown` (or `Playing` — a race resource that outlived
//!   its gate still counts down): tick the countdown, release exactly
//!   once — participants flip `AwaitingStart → Racing`, the session
//!   moves `Countdown → Playing`, one [`RaceStarted`] message goes out
//!   (AC03).
//! - session `Playing` + race `Running`: advance the race clock and
//!   every `Racing` participant's swept segment; a `Finished` outcome
//!   mints and records exactly one [`SessionResult`] per participant
//!   per generation into [`ResultLedger`] (AC04); all-finished marks
//!   the race `Complete`.
//! - anything else (`Paused`, `Unloading`, …): frozen — the race clock
//!   and every swept segment hold still, so pause/resume is
//!   deterministic and no timer runs during teardown.
//!
//! A stale `RaceState` (generation mismatch, e.g. the frames between a
//! restart's `begin` and teardown's resource removal) is never stepped.
//!
//! [`reanchor_teleported_participants`] runs chained before
//! [`advance_race`] in `FixedLast`: a `ResetVehicle` teleport is not
//! motion, so the swept segment must be re-anchored rather than counted
//! as a crossing (AC02).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    ParticipantState, Player, ProgressOutcome, RacePhase, RaceProgress, RaceStarted, RaceState,
    ResultLedger, Session, SessionOutcome, SessionPhase, SessionResult,
};
use mm2_vehicle::Teleported;
use tracing::warn;

/// Re-anchor swept segments on teleported participants.
///
/// `vehicle_reset` marks each entity it teleports with [`Teleported`]
/// in the same pass that writes the new `Position`; a participant's
/// swept segment must break on that jump rather than consume every
/// checkpoint between the two poses (AC02 — the reset-near-finish
/// edge). The marker is consumed here, before [`advance_race`] in
/// `FixedLast`, so it applies in every session phase — a reset while
/// paused or mid-countdown still lands. Markers on entities without
/// `RaceProgress` are inert and despawn with the entity.
pub fn reanchor_teleported_participants(
    mut commands: Commands,
    mut participants: Query<(Entity, &mut RaceProgress), With<Teleported>>,
) {
    for (entity, mut progress) in &mut participants {
        progress.break_segment();
        commands.entity(entity).remove::<Teleported>();
    }
}

/// Fixed-step race driver — see module docs.
pub fn advance_race(
    race: Option<ResMut<RaceState>>,
    mut session: ResMut<Session>,
    mut ledger: ResMut<ResultLedger>,
    mut started: MessageWriter<RaceStarted>,
    mut participants: Query<(&Player, &Position, &mut RaceProgress)>,
) {
    let Some(mut race) = race else {
        return;
    };
    if race.is_stale(session.generation()) || !session.authority_role().is_authority() {
        return;
    }
    match *session.phase() {
        // The race clock and its triggers only live while the session
        // runs them; Paused/Results/Unloading freeze everything.
        SessionPhase::Countdown | SessionPhase::Playing => {}
        _ => return,
    }
    match &mut race.phase {
        RacePhase::Countdown { remaining } => {
            if *remaining > 0 {
                *remaining -= 1;
            }
            if *remaining > 0 {
                return;
            }
            race.phase = RacePhase::Running;
            for (_, _, mut progress) in &mut participants {
                if progress.state == ParticipantState::AwaitingStart {
                    progress.state = ParticipantState::Racing;
                }
            }
            if *session.phase() == SessionPhase::Countdown {
                session
                    .transition(SessionPhase::Playing)
                    .expect("Countdown → Playing is a legal transition");
            }
            started.write(RaceStarted);
        }
        RacePhase::Running => {
            // A race released by another path while the session still
            // counts down waits for the session — the clock only runs
            // while Playing.
            if !session.is_playing() {
                return;
            }
            race.clock += 1;
            let mut pending = false;
            for (player, position, mut progress) in &mut participants {
                // AwaitingStart during Running counts as pending: the
                // race cannot complete with a participant that never
                // started. Finished participants are done.
                if progress.state == ParticipantState::AwaitingStart {
                    pending = true;
                    continue;
                }
                if !matches!(progress.state, ParticipantState::Racing) {
                    continue;
                }
                if progress.advance(&race.definition, position.0) == ProgressOutcome::Finished {
                    let id = session.mint_result_id(player.id);
                    let result = SessionResult {
                        id: id.clone(),
                        tick: session.tick(),
                        outcome: SessionOutcome::Finished {
                            race_ticks: race.clock,
                        },
                    };
                    match ledger.record(result) {
                        Ok(()) => {}
                        // Impossible through this path — the id is minted
                        // fresh — but a rejected record is never retried
                        // silently: the finish is still terminal so it
                        // cannot re-emit every step.
                        Err(dup) => warn!(duplicate = %dup, "race result rejected"),
                    }
                    progress.state = ParticipantState::Finished {
                        race_ticks: race.clock,
                        result: id,
                    };
                } else {
                    pending = true;
                }
            }
            if !pending && !participants.is_empty() {
                race.phase = RacePhase::Complete;
            }
        }
        RacePhase::Complete => {}
    }
}

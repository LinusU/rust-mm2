//! Result → progression consumption (F16-B).
//!
//! [`EventRewards`] is the running event's session-scoped reward
//! surface: the [`EventKey`] records save under plus the city's
//! normalized [`RewardTable`]. `load_session_world` inserts it when an
//! event setup succeeds; `drive_session`'s teardown removes it so a
//! later cruise or different event never consumes results against a
//! stale table.
//!
//! [`record_session_results`] is the write side of progression: it
//! drains the authoritative [`ResultLedger`] — the only source of
//! results, produced by `advance_race` under the session's authority —
//! and applies each of the *local* participant's results to the bound
//! profile through `mm2_game::apply_result`. Every gate the spec asks
//! for lives here or one layer down:
//!
//! - No bound profile → nothing to write into (profile-less runs stay
//!   profile-less; F16-AC01 isolation).
//! - `!records_progress()` (sandbox identities) → remembered
//!   selections still record at session start, but progress and
//!   unlocks never do (spec req 5, F16-AC03).
//! - `record_eligibility(config)` → dev-world rigs, the synthetic dev
//!   car, gameplay-affecting dev overrides and modded sessions are
//!   ineligible — records exist only for default-condition runs
//!   (DRV-6, F16-AC03). The reason is logged once per generation.
//! - `--bot` (`ScriptedDrive` mounted) → the scripted driver is an
//!   evidence tool, not the player; its finishes never grant.
//! - Only `Finished` results record or grant; `TimedOut` is inert
//!   (F16-AC03).
//! - Each `ResultId` applies exactly once per run of the app — the
//!   ledger already dedups by identity, and the `applied` set makes
//!   re-delivery across frames free; `unlocks` set membership makes
//!   repeat *earned* grants idempotent across sessions (F16-AC02).
//!
//! Every pass also maintains a [`SessionReport`] resource — what the
//! run earned or why it kept nothing — so the results screen (F17-B.2)
//! presents the real disposition instead of guessing.
//!
//! Places come from `standings_in`/`place_of_in` — generation-scoped,
//! because the ledger outlives one session and a restart's stale
//! results must not re-rank the live race.

use std::collections::HashSet;

use bevy::prelude::*;
use mm2_game::{
    EventKey, Player, PlayerControl, ResultId, ResultLedger, RewardTable, Session, SessionOutcome,
    SessionResult, apply_result, record_eligibility,
};
use tracing::{info, warn};

use crate::profile::ActiveProfile;
use crate::scripted::ScriptedDrive;

/// The running event's reward surface (F16-B). Session-scoped: inserted
/// by `load_session_world` only when the event setup succeeds, removed
/// with the session by `drive_session`'s `Unloading` pass.
#[derive(Resource)]
pub struct EventRewards {
    /// The event's stable save identity — the same key
    /// `selections.last_event` records.
    pub key: EventKey,
    /// The city's normalized reward rules and family denominators.
    pub table: RewardTable,
    /// The city's derived availability surface (CHK-2/CHK-3, CC-3) —
    /// which authored events the bound profile may select. Carried
    /// here so a session can report a locked launch; the menu flow
    /// that enforces it is F17.
    pub availability: mm2_game::AvailabilityTable,
}

/// What the session's recorded results actually produced — written by
/// [`record_session_results`] so the results screen (F17-B.2) presents
/// the run's real disposition rather than guessing. Session-scoped:
/// `drive_session`'s teardown removes it with the session it describes.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionReport {
    /// The session generation this report accounts for.
    pub generation: u64,
    /// A local result was recorded to the bound profile.
    pub recorded: bool,
    /// Authored unlock messages granted by this session's results
    /// (already deduped by `unlocks` set membership — a grant the
    /// profile already held does not reappear here).
    pub granted: Vec<String>,
    /// Why nothing was kept, when a gate refused the result or the
    /// outcome records nothing — the results screen's honesty line.
    pub note: Option<String>,
}

/// Per-app-run consumption state: which results already applied, and
/// the report being built for the current generation.
#[derive(Default)]
pub struct Consumption {
    generation: u64,
    applied: HashSet<ResultId>,
    report: SessionReport,
    dirty: bool,
}

/// Publish the accumulated report, once per change — `SessionReport`
/// is a whole-resource insert so readers see a consistent snapshot.
fn flush(state: &mut Consumption, commands: &mut Commands) {
    if state.dirty {
        commands.insert_resource(state.report.clone());
        state.dirty = false;
    }
}

/// Drain authoritative results into the bound profile and the
/// [`SessionReport`]. Runs every frame; the early-outs make the common
/// cases (nothing new, no local driver) free.
// One over the lint's limit — `commands` is the report's write
// channel; bundling the borrows would hide the handles every arm uses.
#[allow(clippy::too_many_arguments)]
pub fn record_session_results(
    session: Res<Session>,
    ledger: Res<ResultLedger>,
    rewards: Option<Res<EventRewards>>,
    scripted: Option<Res<ScriptedDrive>>,
    players: Query<&Player>,
    mut profile: Option<ResMut<ActiveProfile>>,
    mut commands: Commands,
    mut state: Local<Consumption>,
) {
    let generation = session.generation();
    if state.generation != generation {
        *state = Consumption {
            generation,
            report: SessionReport {
                generation,
                ..SessionReport::default()
            },
            ..Consumption::default()
        };
    }

    // The ledger is keyed by `PlayerId`, not by control — resolve the
    // local driver once so an AI opponent's result never lands on the
    // profile or the report.
    let Some(local) = players
        .iter()
        .find(|p| p.control == PlayerControl::Local)
        .map(|p| p.id)
    else {
        return;
    };
    // The local driver's un-accounted event results this generation —
    // `applied` makes re-delivery across frames free.
    let pending: Vec<SessionResult> = ledger
        .iter()
        .filter(|r| {
            r.id.generation == generation
                && r.id.participant == local
                && r.id.event.is_some()
                && !state.applied.contains(&r.id)
        })
        .cloned()
        .collect();
    if pending.is_empty() {
        return flush(&mut state, &mut commands);
    }
    let Some(config) = session.config() else {
        return;
    };
    // A local result exists to account for. If a gate refuses to keep
    // it, the refusal itself is the report — the results screen must
    // never claim a save that did not happen. The refusal is final for
    // the generation, so the ids are consumed the same either way.
    let blocked = match profile.as_ref() {
        None => Some("no driver profile - progress is not saved".to_string()),
        Some(p) if !p.profile.records_progress() => {
            Some("sandbox profile - records and rewards are not kept".to_string())
        }
        _ if scripted.is_some() => {
            Some("scripted driver - records and rewards are not kept".to_string())
        }
        _ => record_eligibility(config)
            .err()
            .map(|why| format!("records not kept: {why}")),
    };
    if let Some(reason) = blocked {
        if state.report.note.as_deref() != Some(reason.as_str()) {
            info!(reason = %reason, "session results ineligible for records");
            state.report.note = Some(reason);
            state.dirty = true;
        }
        state.applied.extend(pending.iter().map(|r| r.id.clone()));
        return flush(&mut state, &mut commands);
    }
    // Event results imply `EventRewards` was mounted at load; a brief
    // absence is teardown in progress — leave the ids unconsumed.
    let Some(rewards) = rewards else { return };
    let slot = &mut **profile.as_mut().expect("unblocked implies a bound profile");
    let mut earned = false;
    for result in pending {
        state.applied.insert(result.id.clone());
        let place = ledger.place_of_in(generation, local);
        let outcome = apply_result(
            &mut slot.profile,
            &rewards.key,
            &result.outcome,
            place,
            config.difficulty,
            &rewards.table,
        );
        if !outcome.recorded {
            // `TimedOut` records nothing (F16-AC03) — say so rather
            // than leaving the screen silent.
            if !state.report.recorded && state.report.note.is_none() {
                let note = match result.outcome {
                    SessionOutcome::TimedOut { .. } => "out of time - no record kept",
                    _ => "no record kept",
                };
                state.report.note = Some(note.to_string());
                state.dirty = true;
            }
            continue;
        }
        earned = true;
        state.report.recorded = true;
        for grant in outcome.granted {
            info!(unlock = %grant.unlock.id(), message = %grant.message, "reward unlocked");
            state.report.granted.push(grant.message);
        }
        state.dirty = true;
    }
    if earned {
        let id = slot.profile.id.clone();
        if let Err(e) = slot.store.save(&mut slot.profile) {
            warn!(profile = %id, error = %e, "profile save failed");
            if state.report.note.is_none() {
                state.report.note = Some(format!("profile save failed: {e}"));
                state.dirty = true;
            }
        }
    }
    flush(&mut state, &mut commands);
}

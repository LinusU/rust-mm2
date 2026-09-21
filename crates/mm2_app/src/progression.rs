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
//! Places come from `standings_in`/`place_of_in` — generation-scoped,
//! because the ledger outlives one session and a restart's stale
//! results must not re-rank the live race.

use std::collections::HashSet;

use bevy::prelude::*;
use mm2_game::{
    EventKey, Player, PlayerControl, ResultId, ResultLedger, RewardTable, Session, apply_result,
    record_eligibility,
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
}

/// Per-app-run consumption state: which results already applied, and
/// the generation an ineligibility note was last logged for.
#[derive(Default)]
pub struct Consumption {
    applied: HashSet<ResultId>,
    noted_ineligible: u64,
}

/// Drain authoritative results into the bound profile. Runs every
/// frame; the early-outs make the common cases (no profile, no event,
/// nothing new) free.
pub fn record_session_results(
    session: Res<Session>,
    ledger: Res<ResultLedger>,
    rewards: Option<Res<EventRewards>>,
    scripted: Option<Res<ScriptedDrive>>,
    players: Query<&Player>,
    mut profile: Option<ResMut<ActiveProfile>>,
    mut state: Local<Consumption>,
) {
    let Some(profile) = profile.as_mut() else {
        return;
    };
    // Sandbox identities keep their selections but never progress
    // (spec req 5) — checked before any ledger read so the gate is
    // unconditional, not per-result.
    if !profile.profile.records_progress() {
        return;
    }
    // The scripted driver is a testing tool (F11-C's `--bot`), not the
    // player — a bot-driven finish must not bank progress or unlocks.
    if scripted.is_some() {
        return;
    }
    let Some(rewards) = rewards else { return };
    let Some(config) = session.config() else {
        return;
    };
    let generation = session.generation();
    if let Err(why) = record_eligibility(config) {
        // One note per generation — the gate re-checks every frame but
        // the log should not.
        if state.noted_ineligible != generation {
            state.noted_ineligible = generation;
            info!(reason = %why, "session results ineligible for records");
        }
        return;
    }
    // The ledger is keyed by `PlayerId`, not by control — resolve the
    // local driver once so an AI opponent's result never lands on the
    // profile.
    let Some(local) = players
        .iter()
        .find(|p| p.control == PlayerControl::Local)
        .map(|p| p.id)
    else {
        return;
    };
    let difficulty = config.difficulty;
    let slot = &mut **profile;
    let mut earned = false;
    for result in ledger.iter() {
        // Only this session's results for the local driver, and only
        // results the session minted for an event — each id applies
        // exactly once.
        if result.id.generation != generation
            || result.id.participant != local
            || result.id.event.is_none()
            || !state.applied.insert(result.id.clone())
        {
            continue;
        }
        let place = ledger.place_of_in(generation, local);
        let outcome = apply_result(
            &mut slot.profile,
            &rewards.key,
            &result.outcome,
            place,
            difficulty,
            &rewards.table,
        );
        if !outcome.recorded {
            continue;
        }
        earned = true;
        for grant in &outcome.granted {
            info!(
                unlock = %grant.unlock.id(),
                message = %grant.message,
                "reward unlocked"
            );
        }
    }
    if earned {
        let id = slot.profile.id.clone();
        if let Err(e) = slot.store.save(&mut slot.profile) {
            warn!(profile = %id, error = %e, "profile save failed");
        }
    }
}

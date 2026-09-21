//! Session/race result identity and deduplication (F01-B contract).
//!
//! AC04: a result must have a stable unique identity so consumers —
//! progression, UI, future network peers — can deduplicate deliveries
//! instead of trusting message order or timing. [`ResultId`] is minted
//! through [`Session::mint_result_id`](crate::Session::mint_result_id)
//! so the session generation is part of the identity, and
//! [`ResultLedger`] is the deduplicating sink consumers record into.
//!
//! The shared race runtime (F11-B) produces results through this
//! contract; mode-specific outcomes extend [`SessionOutcome`] as the
//! modes that produce them land.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::config::EventRef;
use crate::ids::PlayerId;

/// Stable, unique identity of one session/race result (AC04). Equality
/// and hashing cover every field — two deliveries of the same result
/// compare equal and a replay across a session boundary cannot collide,
/// because `generation` namespaces it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResultId {
    /// Session generation the result was produced in.
    pub generation: u64,
    /// The participant the result belongs to.
    pub participant: PlayerId,
    /// The authored event the result is for, when the session is one.
    pub event: Option<EventRef>,
    /// Per-session sequence — distinguishes a retried event's second
    /// result from its first.
    pub sequence: u32,
}

/// What a session result records. The payload stays deliberately thin —
/// mode-specific fields (placement, score, DNF reasons) extend this
/// enum as the modes that produce them land; the shared race runtime
/// (F11-B) produces `Finished` and `TimedOut`.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOutcome {
    /// The event's required checkpoints were all cleared, and its
    /// finish trigger crossed where the definition has one.
    Finished {
        /// Ticks on the race clock from start to finish — the
        /// authoritative finish time progression compares.
        race_ticks: u64,
    },
    /// The event's time limit expired with objectives still open
    /// (BLZ-1 — the finish must come before time runs out).
    TimedOut {
        /// Ticks on the race clock when the deadline hit — the
        /// definition's limit in ticks.
        race_ticks: u64,
    },
}

impl SessionOutcome {
    /// Stable lowercase name for logs and smoke records.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Finished { .. } => "finished",
            Self::TimedOut { .. } => "timed-out",
        }
    }
}

/// A finished session's result record: stable identity plus the session
/// tick it was recorded at and what happened. Mode-specific fields
/// (placement, time, score) extend [`SessionOutcome`] when the modes
/// that produce them land.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResult {
    /// Stable identity — mint via
    /// [`Session::mint_result_id`](crate::Session::mint_result_id).
    pub id: ResultId,
    /// Fixed-step session tick the result was recorded on.
    pub tick: u64,
    /// What the result records.
    pub outcome: SessionOutcome,
}

/// A result whose [`ResultId`] was already recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateResult(pub ResultId);

impl std::fmt::Display for DuplicateResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "duplicate result: participant {:?} sequence {} in generation {}",
            self.0.participant, self.0.sequence, self.0.generation
        )
    }
}

impl std::error::Error for DuplicateResult {}

/// The deduplicating sink for results (AC04). `record` accepts a result
/// exactly once; redeliveries — retries, double sends, replayed
/// consumers — are rejected as [`DuplicateResult`]. Accepted records are
/// retained, so consumers (results presentation, progression, smoke
/// evidence) read what happened, not just that it did.
#[derive(Resource, Debug, Default)]
pub struct ResultLedger {
    recorded: HashMap<ResultId, SessionResult>,
}

impl ResultLedger {
    /// Record `result`; `Err(DuplicateResult)` if its id was seen before.
    pub fn record(&mut self, result: SessionResult) -> Result<(), DuplicateResult> {
        if self.recorded.contains_key(&result.id) {
            return Err(DuplicateResult(result.id));
        }
        self.recorded.insert(result.id.clone(), result);
        Ok(())
    }

    /// Whether a result id was already recorded.
    pub fn contains(&self, id: &ResultId) -> bool {
        self.recorded.contains_key(id)
    }

    /// The recorded result for `id`, if any.
    pub fn get(&self, id: &ResultId) -> Option<&SessionResult> {
        self.recorded.get(id)
    }

    /// Every recorded result, unordered.
    pub fn iter(&self) -> impl Iterator<Item = &SessionResult> {
        self.recorded.values()
    }

    /// Number of distinct results recorded.
    pub fn len(&self) -> usize {
        self.recorded.len()
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.recorded.is_empty()
    }

    /// The recorded results in finishing order — the authoritative
    /// standings a results screen or progression (F16) ranks by
    /// (F13-B).
    ///
    /// Ordering contract (**designed** — the ledger records no
    /// verified original placing rule, so this is an explicit policy,
    /// not an original-behavior claim):
    ///
    /// - `Finished` outranks `TimedOut`: a completed event beats a DNF
    ///   no matter the times.
    /// - `Finished` orders by `race_ticks` — the authoritative race
    ///   clock the finish was recorded against, so recording order
    ///   cannot scramble the standings.
    /// - Equal `race_ticks` (a same-tick finish, or two expiries on
    ///   the shared deadline) order by `participant` — a deterministic
    ///   tie-break that does not depend on query or recording order
    ///   (the spec's required explicit tie resolution).
    /// - A participant with no recorded result has no standing at
    ///   all: still-racing or quit participants are unplaced, not
    ///   ranked last.
    pub fn standings(&self) -> Vec<&SessionResult> {
        let mut ordered: Vec<&SessionResult> = self.recorded.values().collect();
        ordered.sort_by_key(|r| standing_key(r));
        ordered
    }

    /// `participant`'s 1-based place in [`standings`](Self::standings)
    /// — `None` when no result is recorded for them. A participant
    /// with several results (e.g. a retried event inside one session)
    /// places by the best one.
    pub fn place_of(&self, participant: PlayerId) -> Option<u32> {
        self.standings()
            .iter()
            .position(|r| r.id.participant == participant)
            .map(|i| i as u32 + 1)
    }

    /// [`standings`](Self::standings) scoped to one session generation.
    /// The ledger is not cleared between sessions — results carry their
    /// generation — so consumers ranking a *live* race must scope, or a
    /// finished restart's results pollute the new session's order.
    pub fn standings_in(&self, generation: u64) -> Vec<&SessionResult> {
        let mut ordered: Vec<&SessionResult> = self
            .recorded
            .values()
            .filter(|r| r.id.generation == generation)
            .collect();
        ordered.sort_by_key(|r| standing_key(r));
        ordered
    }

    /// `participant`'s 1-based place in
    /// [`standings_in`](Self::standings_in) — `None` when they recorded
    /// no result in `generation`.
    pub fn place_of_in(&self, generation: u64, participant: PlayerId) -> Option<u32> {
        self.standings_in(generation)
            .iter()
            .position(|r| r.id.participant == participant)
            .map(|i| i as u32 + 1)
    }
}

/// The standings sort key — see [`ResultLedger::standings`]' ordering
/// contract: finished before timed-out, then race ticks, then
/// participant id.
fn standing_key(result: &SessionResult) -> (u8, u64, PlayerId) {
    match result.outcome {
        SessionOutcome::Finished { race_ticks } => (0, race_ticks, result.id.participant),
        SessionOutcome::TimedOut { race_ticks } => (1, race_ticks, result.id.participant),
    }
}

/// English ordinal for a 1-based place: 1st, 2nd, 3rd… (the standings
/// formatter — shared by the HUD line and the results screen so the
/// two agree on a place's spelling).
pub fn ordinal(place: u32) -> String {
    let suffix = match (place % 100, place % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{place}{suffix}")
}

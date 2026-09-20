//! Session/race result identity and deduplication (F01-B contract).
//!
//! AC04: a result must have a stable unique identity so consumers —
//! progression, UI, future network peers — can deduplicate deliveries
//! instead of trusting message order or timing. [`ResultId`] is minted
//! through [`Session::mint_result_id`](crate::Session::mint_result_id)
//! so the session generation is part of the identity, and
//! [`ResultLedger`] is the deduplicating sink consumers record into.
//!
//! Nothing produces results yet — no race/mode logic exists (F12+).
//! This is the contract they will write against, not a claim of
//! implemented results.

use std::collections::HashSet;

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

/// A finished session's result record. The payload stays deliberately
/// thin until real modes exist: identity plus the session tick it was
/// recorded at. Mode-specific fields (placement, time, score) extend
/// this struct when the races that produce them land.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResult {
    /// Stable identity — mint via
    /// [`Session::mint_result_id`](crate::Session::mint_result_id).
    pub id: ResultId,
    /// Fixed-step session tick the result was recorded on.
    pub tick: u64,
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
/// consumers — are rejected as [`DuplicateResult`].
#[derive(Resource, Debug, Default)]
pub struct ResultLedger {
    recorded: HashSet<ResultId>,
}

impl ResultLedger {
    /// Record `result`; `Err(DuplicateResult)` if its id was seen before.
    pub fn record(&mut self, result: SessionResult) -> Result<(), DuplicateResult> {
        if self.recorded.insert(result.id.clone()) {
            Ok(())
        } else {
            Err(DuplicateResult(result.id))
        }
    }

    /// Whether a result id was already recorded.
    pub fn contains(&self, id: &ResultId) -> bool {
        self.recorded.contains(id)
    }

    /// Number of distinct results recorded.
    pub fn len(&self) -> usize {
        self.recorded.len()
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.recorded.is_empty()
    }
}

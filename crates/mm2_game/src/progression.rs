//! Progression: reward/unlock rules and authoritative-result
//! consumption (F16-B).
//!
//! The authored source is `race/<city>/<city>_rewards.csv` — one table
//! per city linking event families to unlocks (`blitz,half,vpcoop2k,0`
//! = beating half the Blitz events unlocks that car). `mm2_content`
//! normalizes the parsed rows into a [`RewardTable`]; this module owns
//! the domain semantics:
//!
//! - An event counts as *beaten* when an authoritative `Finished`
//!   result meets the documented place criterion
//!   ([`place_requirement`]: top-3 Amateur, 1st Professional — RACE-3,
//!   CHK-3, VEH-3/VEH-4 all state "top-3/1st"). Solo events (Blitz,
//!   Crash Course) finish at place 1, so both ranks reduce to
//!   "finish".
//! - `half`/`all` milestone rows count beaten events in the row's
//!   family against the city's authored family size; indexed
//!   (`crash,N`) rows attach to the one event they reward (CC-6's
//!   midterm/final links).
//! - Grants are idempotent — [`PlayerProfile::progress`]'s `unlocks`
//!   set makes re-delivered results and repeat finishes no-ops
//!   (F16-AC02). Progress is written only here, from authoritative
//!   results — never by UI navigation (spec req 3).
//! - [`record_eligibility`] is the DRV-6 default-conditions gate plus
//!   the spec req 5 separations: dev-world rigs, the synthetic dev
//!   car, gameplay-affecting [`DevOverrides`] and modded content never
//!   produce records. The profile-kind gate (`records_progress`) and
//!   the scripted-driver exclusion are the consumer's — they live on
//!   the profile resource / app schedule, not in the config.
//!
//! Event availability (CHK-2/CHK-3's set-of-three gating, CC-3's
//! lesson→midterm→final chain, RACE-3's per-race customization
//! unlocks) is *derived* state over the same `beaten` flags — its
//! consumer is F17's menu flow, so no availability query ships until
//! then. Pro points (DRV-4) stay unverified (UNK-8).

use std::collections::BTreeMap;

use crate::config::{Difficulty, EventTableKind, SessionConfig};
use crate::profile::{EventKey, PlayerProfile};
use crate::result::SessionOutcome;

/// What a reward row grants (VEH-3/VEH-4, CC-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unlock {
    /// `VariantNum` 0 — the vehicle itself becomes selectable.
    Vehicle(String),
    /// `VariantNum` ≠ 0 — a paint job for the named vehicle. The
    /// authored variant number is kept verbatim; whether it is a
    /// 0-based paint index or a 1-based count is unverified (the
    /// header only documents the 0 = car case).
    Paint {
        /// Vehicle catalog id the paint belongs to.
        car: String,
        /// Authored `VariantNum`.
        variant: i64,
    },
}

impl Unlock {
    /// The stable id stored in `ProfileProgress::unlocks` —
    /// `vehicle:<id>` / `paint:<id>:<variant>`.
    pub fn id(&self) -> String {
        match self {
            Self::Vehicle(car) => format!("vehicle:{car}"),
            Self::Paint { car, variant } => format!("paint:{car}:{variant}"),
        }
    }
}

/// How an authored reward row triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardRequirement {
    /// `half` — beat at least half the family's authored events. For
    /// an odd-sized family the bound is `size.div_ceil(2)` ("at least
    /// half" — every retail family is even, so the odd case is a
    /// designed choice, not an original rule).
    Half,
    /// `all` — beat every authored event in the family.
    All,
    /// An indexed `crash,N`-style row bound to the single event it
    /// rewards — the authored row index is kept for audit only; the
    /// binding is the [`EventKey`] the producer resolved.
    Event(i64),
}

/// One normalized reward rule.
#[derive(Debug, Clone)]
pub struct RewardRule {
    /// The event family the row measures (`race` = Checkpoint).
    pub family: EventTableKind,
    /// `half`/`all` milestone or an event-bound indexed row.
    pub requirement: RewardRequirement,
    /// What meeting it grants.
    pub unlock: Unlock,
    /// Authored unlock message (shown to the player by F17's UI).
    pub message: String,
    /// 1-based line in the source table — audit context.
    pub line: u32,
}

/// A city's normalized reward surface: the rules plus the denominators
/// `half`/`all` measure against. Produced once per session by
/// `mm2_content` from the event catalog — content stays a load-time
/// object, the table is the session-scoped runtime view.
#[derive(Debug, Clone, Default)]
pub struct RewardTable {
    /// Indexed rows bound to the event they reward (`crash,N` → the
    /// `crash<N>` event's key).
    pub per_event: Vec<(EventKey, RewardRule)>,
    /// `half`/`all` family milestone rows.
    pub milestones: Vec<RewardRule>,
    /// Authored event count per family — the milestone denominators.
    /// Counted from the catalog's table rows, so extras/discovered
    /// records never inflate a denominator.
    pub family_sizes: BTreeMap<EventTableKind, usize>,
    /// Rows that could not become a rule (unrecognized `RaceType`,
    /// unusable `RaceNum`, an index no event owns) — surfaced for the
    /// AC05 coverage audit, never silently dropped.
    pub diagnostics: Vec<String>,
}

/// The documented place a finish must reach to count the event as
/// beaten: top-3 at Amateur, 1st at Professional (RACE-3/CHK-3/VEH-3/
/// VEH-4 — every authored rule states "top-3/1st").
pub fn place_requirement(difficulty: Difficulty) -> u32 {
    match difficulty {
        Difficulty::Amateur => 3,
        Difficulty::Professional => 1,
    }
}

/// A newly granted unlock — reported once, on the result that earned
/// it (`unlocks` set membership makes every later delivery a no-op).
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    /// What was granted.
    pub unlock: Unlock,
    /// The authored unlock message.
    pub message: String,
}

/// What applying one authoritative result did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyOutcome {
    /// A `Finished` outcome was recorded into `progress.events`.
    pub recorded: bool,
    /// Unlocks this result granted for the first time.
    pub granted: Vec<Grant>,
}

/// Apply one authoritative local-participant result to `profile`'s
/// progress: record the finish and evaluate `table`'s rules.
///
/// Eligibility (profile kind, session conditions, scripted driver) is
/// the caller's — this function applies what it is handed. Only
/// `Finished` outcomes record; a `TimedOut` or quit run touches
/// nothing (F16-AC03). Re-applying an already-applied result is the
/// consumer's dedup concern — each distinct `ResultId` is a distinct
/// finish, and `unlocks` keeps the grants idempotent.
pub fn apply_result(
    profile: &mut PlayerProfile,
    key: &EventKey,
    outcome: &SessionOutcome,
    place: Option<u32>,
    difficulty: Difficulty,
    table: &RewardTable,
) -> ApplyOutcome {
    let &SessionOutcome::Finished { race_ticks } = outcome else {
        return ApplyOutcome::default();
    };
    let record = profile.event_mut(key.clone());
    record.record_finish(race_ticks, place, difficulty);

    let mut granted = Vec::new();
    // Indexed rules (CC-6's `crash,N` rows): the persisted beaten flag
    // is the "passed" state, so a repeat finish also re-grants an
    // unlock a hand-edited file lost — `insert` keeps it idempotent.
    if record.is_beaten() {
        for (k, rule) in &table.per_event {
            if k == key && profile.progress.unlocks.insert(rule.unlock.id()) {
                granted.push(Grant {
                    unlock: rule.unlock.clone(),
                    message: rule.message.clone(),
                });
            }
        }
    }
    // Family milestones: this finish can only move this family in this
    // city, so only its rules are re-evaluated.
    let size = table.family_sizes.get(&key.table).copied().unwrap_or(0);
    for rule in &table.milestones {
        if rule.family != key.table {
            continue;
        }
        let needed = match rule.requirement {
            RewardRequirement::Half => size.div_ceil(2),
            RewardRequirement::All => size,
            // A stray indexed row in milestones is a producer
            // diagnostic, never a rule — skip rather than guess.
            RewardRequirement::Event(_) => continue,
        };
        if needed == 0 {
            continue;
        }
        let beaten = profile
            .progress
            .events
            .iter()
            .filter(|r| r.key.city == key.city && r.key.table == rule.family && r.is_beaten())
            .count();
        if beaten >= needed && profile.progress.unlocks.insert(rule.unlock.id()) {
            granted.push(Grant {
                unlock: rule.unlock.clone(),
                message: rule.message.clone(),
            });
        }
    }
    ApplyOutcome {
        recorded: true,
        granted,
    }
}

/// Why a session's results are ineligible for records and unlocks
/// (F16-AC03, DRV-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    /// Not a real city session — the dev world is a test rig.
    World,
    /// No catalog vehicle drove — the synthetic dev car is not a
    /// stock ride.
    Vehicle,
    /// A gameplay-affecting developer override ran (which one). The
    /// presentation-only overrides — `--cam`, `--nav` — are not listed:
    /// they cannot change a run's outcome.
    DevOverride(&'static str),
    /// Mod content was mounted — a record under modded content is not
    /// comparable to stock (conservative policy until F29's per-mod
    /// classification).
    ModContent,
}

impl std::fmt::Display for Ineligible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::World => write!(f, "not a city session"),
            Self::Vehicle => write!(f, "no catalog vehicle"),
            Self::DevOverride(which) => write!(f, "dev override {which}"),
            Self::ModContent => write!(f, "mod content mounted"),
        }
    }
}

/// Whether `config` describes a record-eligible run (DRV-6: records
/// are kept only for races run under default conditions). The checks
/// are all things a config can decide before the first result exists —
/// the profile-kind and scripted-driver gates are separate because
/// they live outside the config.
pub fn record_eligibility(config: &SessionConfig) -> Result<(), Ineligible> {
    if !matches!(config.world, crate::WorldMode::City { .. }) {
        return Err(Ineligible::World);
    }
    if config.vehicle.id.is_none() {
        return Err(Ineligible::Vehicle);
    }
    if config.mods_active {
        return Err(Ineligible::ModContent);
    }
    let dev = &config.dev;
    if dev.vehicle_config.is_some() {
        Err(Ineligible::DevOverride("vehicle-config"))
    } else if dev.spawn.is_some() {
        Err(Ineligible::DevOverride("spawn"))
    } else if dev.banger_pool.is_some() {
        Err(Ineligible::DevOverride("banger-pool"))
    } else if dev.traction.is_some_and(|t| t != 1.0) {
        Err(Ineligible::DevOverride("traction"))
    } else {
        Ok(())
    }
}

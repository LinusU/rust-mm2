//! Damage and recovery rules contract (F05-A).
//!
//! [`DamageSignals`](crate::DamageSignals) (F01-B) accumulates the raw
//! impact input; this module is the interpretation layer F05-B's
//! runtime consumes. The *bounds* are authored data — `vehCarDamage`'s
//! `ImpactThreshold`/`MedDamage`/`MaxDamage`/`RegenerateRate`, decoded
//! by [`VehCarDamage`] — while the accumulation rule itself is a
//! disclosed designed policy: the original's exact severity→damage
//! conversion is unverified (UNK-13). What this module pins down is
//! everything the spec can state without that conversion:
//!
//! - impacts at or below `ImpactThreshold` never damage (F05-AC01:
//!   resting contact, curb taps and normal suspension loads must not
//!   accumulate — the authored floor is 1500 on 19 of 20 retail
//!   records, 100 on `vpcaddie`);
//! - damage is monotonic within a run except through `tick`
//!   (`RegenerateRate`, 0 on every retail record — the DMG-4 C&R
//!   healing mechanism's authored channel) or an explicit
//!   `repair`/`reset`, both of which are authority operations by the
//!   spec's "clients never grant themselves repairs" rule — who may
//!   call them is the session's authority check, not this state's;
//! - `MedDamage`/`MaxDamage` bound the damage tiers the DMG-1 meter
//!   and impairment rules read;
//! - a destroyed vehicle's consequence is per-mode, documented
//!   (RACE-5/DMG-2): restart in Blitz/Checkpoint, a time penalty plus
//!   reset in Circuit.
//!
//! Duplicate/ordering safety lives upstream: [`ImpactDedup`] already
//! suppresses repeated contact events before they reach `apply`, so a
//! re-delivered impulse cannot double-count (F05-AC06).

use bevy::prelude::*;
use mm2_formats::veh::VehCarDamage;

use crate::config::{EventTableKind, SessionMode};
use crate::ids::ObjectId;
use crate::impact::ImpactId;
use crate::race::RACE_TICK_HZ;

/// The authored damage bounds for one vehicle — the mechanical-damage
/// model decoded from `tune/vehicle/<id>.vehcardamage`.
///
/// The accumulating quantity is impulse-scale: retail `MaxDamage`
/// values (187.5k on `vpcoop`/`vpcoop2k`, 3.28M on `vpsemi`) track
/// vehicle mass, so the bound is an impulse integral, not a speed
/// count (inferred — the original conversion is unverified, UNK-13).
/// Callers feed the same impulse estimate the impact pipeline
/// reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageSpec {
    /// `ImpactThreshold` — severities at or below this are ignored.
    /// 1500 on 19 of 20 retail records; `vpcaddie` authors 100.
    pub impact_threshold: f32,
    /// `MedDamage` — the damaged-band bound (meter yellow).
    pub med_damage: f32,
    /// `MaxDamage` — the destruction bound (meter empty).
    pub max_damage: f32,
    /// `RegenerateRate` — damage healed per second. 0 on every retail
    /// record; the authored channel DMG-4's C&R healing would drive.
    pub regenerate_rate: f32,
}

impl From<&VehCarDamage> for DamageSpec {
    fn from(d: &VehCarDamage) -> Self {
        Self {
            impact_threshold: d.impact_threshold,
            med_damage: d.med_damage,
            max_damage: d.max_damage,
            regenerate_rate: d.regenerate_rate,
        }
    }
}

/// Which band of the damage meter an accumulated total sits in
/// (DMG-1: green → yellow → red/empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageTier {
    /// Below `MedDamage` — the healthy band.
    Intact,
    /// `MedDamage..MaxDamage` — visibly damaged; smoke/parts tier.
    Damaged,
    /// At `MaxDamage` — destroyed/disabled.
    Disabled,
}

/// What [`DamageState::apply`] did with one impact severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageVerdict {
    /// Non-finite, non-positive or at/below `ImpactThreshold` — the
    /// impact never reaches the accumulator (the F05-AC01 rule:
    /// resting contact and ordinary suspension loads cannot damage).
    Rejected,
    /// The impact identity was already applied (or arrived
    /// out-of-order behind a later one) — a re-delivered event cannot
    /// double-count (F05-AC06).
    Duplicate,
    /// Accumulated — the vehicle remains in the `Intact` band.
    Intact,
    /// Accumulated — the vehicle sits in the `Damaged` band.
    Damaged,
    /// Accumulated — the vehicle reached `MaxDamage` and is disabled.
    Disabled,
}

/// One vehicle's accumulated damage — authority-owned session state
/// (F05 req 6: clients never repair or shield themselves; the session
/// decides who may call `repair`/`reset`). The accumulator saturates
/// at `max_damage` — overkill hits do not grow it — so the tier read
/// off [`Self::condition`] is stable and `Disabled` is terminal until
/// an explicit authority repair.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageState {
    total: f32,
}

impl DamageState {
    /// The accumulated damage total (authored impulse units).
    pub fn total(&self) -> f32 {
        self.total
    }

    /// Damage remaining before destruction — the DMG-1 meter readout.
    /// `0.0` at `MaxDamage`; negative-space clamps at 0.
    pub fn remaining(&self, spec: &DamageSpec) -> f32 {
        (spec.max_damage - self.total).max(0.0)
    }

    /// Meter fraction in `0.0..=1.0` — 1.0 undamaged, 0.0 destroyed.
    /// `max_damage <= 0` degenerates to destroyed-at-rest.
    pub fn health_fraction(&self, spec: &DamageSpec) -> f32 {
        if spec.max_damage <= 0.0 {
            return 0.0;
        }
        (self.remaining(spec) / spec.max_damage).clamp(0.0, 1.0)
    }

    /// The tier the current total maps onto.
    pub fn condition(&self, spec: &DamageSpec) -> DamageTier {
        if self.total >= spec.max_damage {
            DamageTier::Disabled
        } else if self.total >= spec.med_damage {
            DamageTier::Damaged
        } else {
            DamageTier::Intact
        }
    }

    /// Accumulate one impact severity (the impulse estimate the impact
    /// pipeline reports — `approach_speed × striker_mass`, the same
    /// quantity banger activation gates on). Severities that are
    /// non-finite, non-positive or at/below `impact_threshold` are
    /// rejected outright — a resting car, a curb tap and normal
    /// suspension loads never reach the accumulator (F05-AC01).
    /// Returns the tier the vehicle sits in afterwards; callers
    /// detect tier transitions by comparing [`Self::condition`]
    /// across the call, so a re-delivered or duplicate impact cannot
    /// double-apply through the deduped pipeline.
    pub fn apply(&mut self, severity: f32, spec: &DamageSpec) -> DamageVerdict {
        if !severity.is_finite() || severity <= spec.impact_threshold.max(0.0) {
            return DamageVerdict::Rejected;
        }
        self.total = (self.total + severity).min(spec.max_damage.max(0.0));
        match self.condition(spec) {
            DamageTier::Intact => DamageVerdict::Intact,
            DamageTier::Damaged => DamageVerdict::Damaged,
            DamageTier::Disabled => DamageVerdict::Disabled,
        }
    }

    /// Advance the authored regeneration channel — `regenerate_rate`
    /// damage healed per second, never below 0. A no-op when the spec
    /// authors no regeneration (every retail record). Whether a
    /// session lets regeneration run at all is mode policy (DMG-4
    /// heals only non-carriers in C&R); this is the mechanism, not
    /// the rule.
    pub fn tick(&mut self, dt: f32, spec: &DamageSpec) {
        if dt.is_finite() && dt > 0.0 && spec.regenerate_rate > 0.0 {
            self.total = (self.total - spec.regenerate_rate * dt).max(0.0);
        }
    }

    /// Restore to full health — an authority-only operation by the
    /// spec's authority rule; the session's role check decides who may
    /// call it, never a remote client for itself.
    pub fn repair(&mut self) {
        self.total = 0.0;
    }

    /// [`Self::repair`] under a session-reset boundary — same
    /// semantics, separate name so call sites state which rule they
    /// implement (session teardown/restart vs an in-session heal).
    pub fn reset(&mut self) {
        self.repair();
    }
}

/// What destroying the vehicle costs the participant (RACE-5/DMG-2,
/// documented; the cruise leg is a designed extension — the help names
/// no free-roam consequence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledOutcome {
    /// Blitz/Checkpoint — the event restarts from the beginning.
    RestartEvent,
    /// Circuit — a time penalty and the vehicle resets in place.
    PenaltyReset,
    /// Cruise — a free reset/recovery (designed: no documented cost).
    FreeReset,
}

/// The consequence of reaching `MaxDamage` in `mode` (documented
/// RACE-5/DMG-2). Crash Course resolves to `RestartEvent` — a lesson
/// restart is the consistent reading, but its rules are F21's
/// unverified territory, so the mapping is marked designed there.
pub fn disabled_outcome(mode: &SessionMode) -> DisabledOutcome {
    match mode {
        SessionMode::Cruise => DisabledOutcome::FreeReset,
        SessionMode::Event(ev) => match ev.table {
            EventTableKind::Circuit => DisabledOutcome::PenaltyReset,
            EventTableKind::Blitz | EventTableKind::Checkpoint | EventTableKind::CrashCourse => {
                DisabledOutcome::RestartEvent
            }
        },
    }
}

/// The race-clock ticks a Circuit [`DisabledOutcome::PenaltyReset`]
/// costs — five seconds. RACE-5/DMG-2 documents "a time penalty" but
/// no source pins the magnitude (UNK-13), so this is a disclosed
/// designed value, not an original rule.
pub const DISABLED_PENALTY_TICKS: u64 = RACE_TICK_HZ as u64 * 5;

/// Designed impairment policy (DSN-25) — what the damaged tier costs
/// the engine.
///
/// MM2Hook's `mm2.ini` `PhysicalEngineDamage` option documents the
/// original coupling: damage affects engine torque, so "when the
/// engine spews smoke" the vehicle has "less acceleration and less
/// top speed". The implemented smoke gate is `MedDamage` (DSN-24), so
/// impairment keys on the same authored bound — the documented
/// coupling holds by construction. The ramp shape and floors are
/// unrecovered (the original `vehCarDamage::Update()` is a binary
/// thunk, UNK-13): these are disclosed designed values, not an
/// original-behavior claim. Scaling drive torque produces both named
/// symptoms — weaker acceleration and a lower drag-equilibrium top
/// speed — while brakes, engine braking and steering stay unaffected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpairmentPolicy {
    /// Fraction of rated engine torque delivered at `MedDamage` —
    /// designed. A step down from 1.0 at smoke onset, so the
    /// documented "when it spews smoke" coupling is literal.
    pub power_at_med: f32,
    /// Fraction delivered at `MaxDamage` — designed. Limps a
    /// near-dead engine without stalling it.
    pub power_at_max: f32,
}

impl Default for ImpairmentPolicy {
    fn default() -> Self {
        Self {
            power_at_med: 0.8,
            power_at_max: 0.4,
        }
    }
}

impl ImpairmentPolicy {
    /// Engine-output factor for a damage `total` under `spec`: 1.0
    /// below `MedDamage`, `power_at_med` on reaching it, then a
    /// linear ramp to `power_at_max` at `MaxDamage`. `factor < 1`
    /// exactly when the DSN-24 smoke gate emits — the documented
    /// smoke↔torque coupling. A degenerate `MedDamage >= MaxDamage`
    /// spec drops to `power_at_max` on the mid tier; non-finite
    /// input or a non-finite floor degrades to full output, never a
    /// stalled or over-driven car.
    pub fn factor(&self, total: f32, spec: &DamageSpec) -> f32 {
        let med = if self.power_at_med.is_finite() {
            self.power_at_med.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let max = if self.power_at_max.is_finite() {
            self.power_at_max.clamp(0.0, 1.0)
        } else {
            1.0
        };
        if !(total.is_finite()
            && spec.med_damage.is_finite()
            && spec.max_damage.is_finite()
            && total >= spec.med_damage)
        {
            return 1.0;
        }
        let span = spec.max_damage - spec.med_damage;
        if span <= 0.0 {
            return max;
        }
        let t = ((total - spec.med_damage) / span).clamp(0.0, 1.0);
        med + (max - med) * t
    }
}

/// A simulated vehicle's live damage: the authored [`DamageSpec`] it
/// decoded to plus the session's accumulated [`DamageState`].
///
/// This is a component on vehicle entities, spawned only when the
/// vehicle's `vehcardamage` record decoded — authored absence (retail's
/// `vpmoonrover`) means no component and no damage, never a fabricated
/// spec. Authority-owned like the state it wraps: a predicted client
/// receives totals by replication, it never `apply`s itself.
///
/// `last_impact` is a monotonic watermark over applied
/// [`ImpactId`]s — a re-delivered or out-of-order impact event reports
/// [`DamageVerdict::Duplicate`] and cannot double-apply (F05-AC06),
/// on top of the upstream [`crate::ImpactDedup`] pair window.
#[derive(Component, Debug, Clone, Copy)]
pub struct VehicleDamage {
    /// The authored bounds (`vehcardamage`).
    pub spec: DamageSpec,
    state: DamageState,
    last_impact: ImpactId,
}

impl VehicleDamage {
    /// A fresh accumulator bound by `spec`.
    pub fn new(spec: DamageSpec) -> Self {
        Self {
            spec,
            state: DamageState::default(),
            last_impact: ImpactId(0),
        }
    }

    /// The accumulated damage total (authored impulse units).
    pub fn total(&self) -> f32 {
        self.state.total()
    }

    /// The tier the current total maps onto — the DMG-1 meter band.
    pub fn condition(&self) -> DamageTier {
        self.state.condition(&self.spec)
    }

    /// Meter fraction in `0.0..=1.0` — 1.0 undamaged, 0.0 destroyed.
    pub fn health_fraction(&self) -> f32 {
        self.state.health_fraction(&self.spec)
    }

    /// Apply one delivered impact. `id` is the impact pipeline's
    /// monotonic identity: ids at or behind the watermark are
    /// duplicates — the delivery is counted and the severity is never
    /// accumulated twice. `severity` is the same impulse-scale quantity
    /// [`DamageState::apply`] takes.
    pub fn apply(&mut self, id: ImpactId, severity: f32) -> DamageVerdict {
        if id <= self.last_impact {
            return DamageVerdict::Duplicate;
        }
        self.last_impact = id;
        self.state.apply(severity, &self.spec)
    }

    /// Advance the authored regeneration channel (`regenerate_rate`
    /// per second). Whether regeneration runs at all is mode policy —
    /// DMG-4 heals only non-carriers in C&R.
    pub fn tick(&mut self, dt: f32) {
        self.state.tick(dt, &self.spec);
    }

    /// Restore to full health — authority-only (the session's role
    /// check decides who may repair, never a remote client for itself).
    /// The impact watermark survives: a repair is not a rewind, and a
    /// late duplicate of a pre-repair impact still cannot land.
    pub fn repair(&mut self) {
        self.state.repair();
    }

    /// [`Self::repair`] under a session-reset boundary — same
    /// semantics, separate name so call sites state which rule they
    /// implement (session teardown/restart vs an in-session heal).
    pub fn reset(&mut self) {
        self.state.reset();
    }
}

/// One accepted damage application — the event HUD, effects and future
/// replication consume. Only [`DamageVerdict`]s that accumulate emit:
/// `Rejected`/`Duplicate` deliveries are non-events by definition, so
/// the stream stays bounded by the impact pipeline's per-tick cap.
#[derive(Message, Debug, Clone, Copy)]
pub struct DamageEvent {
    /// The vehicle that took the damage.
    pub object: ObjectId,
    /// Session generation the impact belongs to.
    pub generation: u64,
    /// Fixed-step session tick the damage was applied on.
    pub tick: u64,
    /// The impact that caused it.
    pub impact: ImpactId,
    /// Impulse accumulated (kg·m/s — the delivered estimate).
    pub severity: f32,
    /// Damage total after this application.
    pub total: f32,
    /// Tier after this application — `Disabled` marks destruction and
    /// is what the session's [`disabled_outcome`] consumer reacts to.
    pub tier: DamageTier,
}

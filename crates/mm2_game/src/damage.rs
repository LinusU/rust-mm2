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
//!   accumulate — the authored 1500 sits well above them);
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

use mm2_formats::veh::VehCarDamage;

use crate::config::{EventTableKind, SessionMode};

/// The authored damage bounds for one vehicle — the mechanical-damage
/// model decoded from `tune/vehicle/<id>.vehcardamage`.
///
/// The accumulating quantity is impulse-scale: retail `MaxDamage`
/// values (238k on `vpauditt`, 3.28M on `vpsemi`) track vehicle mass,
/// so the bound is an impulse integral, not a speed count (inferred —
/// the original conversion is unverified, UNK-13). Callers feed the
/// same impulse estimate the impact pipeline reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageSpec {
    /// `ImpactThreshold` — severities at or below this are ignored.
    /// 1500 on every retail record.
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

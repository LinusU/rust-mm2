//! Vehicle breakaway-part contract (F05-B.3).
//!
//! Stock cars ship detachable panels: `BREAK<NN>` chunks inside the
//! vehicle's own PKG are the intact representation, and each is bound
//! to a `tune/banger/<id>_break<NN>.dgbangerdata` fragment record —
//! the same `<base>_break<N>` naming the prop-side audit resolves
//! (REC-1, WLD-15). The record carries fragment physicals (mass,
//! friction, elasticity) and an authored `ImpulseLimit2` — original
//! data — though the latter does not gate detachment (see below).
//!
//! A part carries the trigger that can take it off ([`BreakGate`]).
//! `BREAK<NN>` panels are **impact-driven**: an impact whose delivered
//! impulse reaches a manager-wide `10000` kg·m/s takes **one** part —
//! the attached panel whose anchor is nearest the contact point. A
//! part's authored `ImpulseLimit2` does not gate detachment at all; it
//! only governs the *spawned fragment's* later collisions through the
//! ordinary banger gate (OPR-1.3). The authored `ImpactThreshold` sits
//! below `10000` on every retail record (1500, or 100 on `vpcaddie`),
//! so the recovered leaf's `impulse > ImpactThreshold` precondition is
//! already satisfied whenever the manager gate trips. The impulse
//! compared is the same estimate [`crate::VehicleDamage`] accumulates —
//! approach speed × the other participant's mass (the struck car's own
//! mass for a wall hit) — which REC-3 measures as the original's unit,
//! kg·m/s, though a lower bound for the same wall hit.
//!
//! Wheels (`whl0..3`) are **damage-driven** (REC-2, a separate recovered
//! rule): once accumulated damage reaches `MaxDamage`, the original's
//! `vehCarDamage::Update` strips wheel parts from the car model's own
//! part manager, in random corner pairs selected by `SpeedMPH`
//! (`vehCarSim +0x24c`): `> 100` takes every wheel, `> 75` two random
//! pairs, `> 50` one, else nothing; the episode is one-shot (a flag the
//! model reset clears) and the fragment kick is `Speed × 1.3`. The
//! original's manager also holds `hubN`/`variantN` parts, but no retail
//! car authors their geometry, so a recovered "pair" here is one wheel.
//! [`VehicleBreaks::eject_at`] is that decision; the app spawns the
//! fragments and repairs re-attach on reset/repair. A part never
//! detaches on the other gate's trigger.
//!
//! Lifecycle per part:
//!
//! ```text
//!   Attached (rides the rig, intact chunk rendered)
//!     ── an impact reaching 10000, or damage reaching MaxDamage ──▶
//!   Detached (chunk hidden, fragment body spawned once)
//!     ── the authority repairs the rig ──▶
//!   Attached (fragment despawned, chunk shown)
//! ```
//!
//! [`VehicleBreaks`] is the authority-owned state: spawned only when
//! the vehicle authored at least one part carrying *both* sides of
//! the inventory (intact chunk + fragment record) — the same absence
//! policy [`VehicleDamage`](crate::VehicleDamage) follows. A part
//! whose record is missing or malformed stays bolted on rather than
//! borrowing a fabricated threshold.

use bevy::prelude::*;

use crate::banger::BangerDefinition;
use crate::ids::ObjectId;
use crate::nav::NavRng;

/// Which trigger can take a part off the rig (REC-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakGate {
    /// Impact-driven: one panel per impact whose delivered impulse
    /// reaches [`VehicleBreaks::DETACH_IMPULSE`] — the `break0..3`,
    /// `break01/12/23/03` panels.
    Impact,
    /// Damage-driven: once accumulated damage reaches `MaxDamage`, the
    /// wheels selected by [`VehicleBreaks::eject_at`] — the `whl0..3`
    /// corners.
    Damage,
}

/// One authored breakaway part: its model stem (`break01`) and the
/// distilled `<id>_<stem>` fragment record. The record's `CG`/`Size`
/// conventions differ from standalone props on retail (car-space
/// anchors vs bound-box centres — the two are not reconciled), so the
/// runtime consumes the record's physicals for the spawned fragment and
/// the fragment's pose from the detached geometry itself. The record's
/// `ImpulseLimit2` is *not* consulted for detachment — the recovered
/// gate is manager-wide (REC-2).
#[derive(Debug, Clone, PartialEq)]
pub struct BreakPartSpec {
    /// Model part stem, lowercase — matches `VehicleModel.parts`
    /// (`break0`, `break01`, ..., `whl0`).
    pub name: String,
    /// The part's own `tune/banger/<id>_<name>.dgbangerdata` record,
    /// distilled — authored threshold and fragment physicals.
    pub def: BangerDefinition,
    /// Which trigger can take this part off (REC-2).
    pub gate: BreakGate,
}

/// Runtime state of one breakaway part.
#[derive(Debug, Clone)]
pub struct BreakPartState {
    /// Authored spec.
    pub spec: BreakPartSpec,
    /// Whether the part still rides on the rig.
    pub attached: bool,
    /// The fragment body it spawned as, while detached (`None` when
    /// the part left the rig without spawning a body — e.g. the
    /// active-pool bound denied the slot).
    pub fragment: Option<Entity>,
}

/// The vehicle's breakaway rig: authored parts in model order. A
/// component, not a resource — each participant's rig is its own, and
/// the app attaches it only to vehicles that authored detach-capable
/// parts.
#[derive(Component, Debug)]
pub struct VehicleBreaks {
    /// Parts in model order; stable indices are the detach contract's
    /// part identity. Impact-gated panels and damage-gated wheels share
    /// one rig so the detached bitmask is a single replicated value
    /// (protocol v11, F25-B).
    pub parts: Vec<BreakPartState>,
    /// One-shot flag for the damage-driven wheel episode (REC-2,
    /// `vehCarModel +0xa4`): once accumulated damage reaches
    /// `MaxDamage` the episode fires once and never again until the
    /// model reset clears it ([`restore`](Self::restore)).
    pub ejected: bool,
}

impl VehicleBreaks {
    /// A rig where every part starts attached.
    pub fn new(specs: Vec<BreakPartSpec>) -> Self {
        Self {
            parts: specs
                .into_iter()
                .map(|spec| BreakPartState {
                    spec,
                    attached: true,
                    fragment: None,
                })
                .collect(),
            ejected: false,
        }
    }

    /// The recovered manager-wide detach gate (REC-2): an impact whose
    /// delivered impulse reaches this many kg·m/s detaches one part.
    /// The original's `vehCarDamage` break manager is constructed with a
    /// single `10000.0` (`0x4d8581`) and nothing rewrites it for a car,
    /// so it is a constant for the whole rig. A part's authored
    /// `ImpulseLimit2` never gates detachment. Authored `ImpactThreshold`
    /// is below it on every retail record (1500, or 100 on `vpcaddie`),
    /// so the recovered leaf's `impulse > ImpactThreshold` precondition
    /// is subsumed.
    pub const DETACH_IMPULSE: f32 = 10_000.0;

    /// `vehCarSim +0x24c` (`SpeedMPH`) above which the damage-driven
    /// wheel episode takes every wheel (`0x5af430` = 100).
    pub const EJECT_ALL_MPH: f32 = 100.0;
    /// `SpeedMPH` above which it takes two random wheel pairs
    /// (`0x5b1d1c` = 75).
    pub const EJECT_TWO_MPH: f32 = 75.0;
    /// `SpeedMPH` above which it takes one random wheel pair
    /// (`0x5b1d18` = 50).
    pub const EJECT_ONE_MPH: f32 = 50.0;
    /// Fragment kick scale: the original stores `Speed × 1.3` in the
    /// wheel manager's kick-speed slot (`0x5af3ec` = 1.3).
    pub const EJECT_KICK_SCALE: f32 = 1.3;

    /// Whether an impact of `impulse` kg·m/s — the same estimate
    /// [`crate::VehicleDamage`] accumulates, approach speed × the other
    /// participant's mass — is strong enough to take an impact-gated
    /// part and one is still attached. Non-finite or negative impulses
    /// are rejected: a junk delivery is never a detach.
    pub fn detaches_on(&self, impulse: f32) -> bool {
        impulse.is_finite()
            && impulse >= Self::DETACH_IMPULSE
            && self
                .parts
                .iter()
                .any(|p| p.spec.gate == BreakGate::Impact && p.attached)
    }

    /// The attached impact-gated part nearest `contact` — the recovered
    /// rule's smallest-squared-distance scan over attached panels
    /// (REC-2); the original detaches exactly this one part per
    /// qualifying impact. Damage-gated wheels are never candidates: the
    /// two gates are separate recovered rules. `anchors` yields
    /// `(part index, anchor)` in the same space as `contact`; a part
    /// with no anchor (no render node) or a non-finite one is simply
    /// absent and can never be chosen. `None` on a non-finite contact,
    /// on an out-of-range index or when no attached panel is a
    /// candidate; ties keep the lowest index.
    pub fn nearest_attached(
        &self,
        contact: Vec3,
        anchors: impl IntoIterator<Item = (usize, Vec3)>,
    ) -> Option<usize> {
        if !contact.is_finite() {
            return None;
        }
        anchors
            .into_iter()
            .filter(|(i, a)| {
                self.parts
                    .get(*i)
                    .is_some_and(|p| p.spec.gate == BreakGate::Impact && p.attached)
                    && a.is_finite()
            })
            .map(|(i, a)| (i, (a - contact).length_squared()))
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(i, _)| i)
    }

    /// The damage-driven wheel episode (REC-2, `0x4cdc90`): mark the
    /// one-shot episode fired and return the part indices to eject, by
    /// `speed_mph` (`vehCarSim +0x24c`): `> 100` every wheel, `> 75` two
    /// random corners, `> 50` one, else none. Draws come from `rng`
    /// (the caller seeds it per vehicle/episode) — the original draws a
    /// corner per pair and refuses an already-detached one, so a
    /// repeated corner in the two-pair band yields a single wheel; the
    /// returned indices may therefore repeat and the caller's
    /// [`detach`](Self::detach) drops the repeats. The episode is
    /// marked fired even when the speed band ejects nothing (the
    /// original sets the flag on its way out), and a second call before
    /// [`restore`](Self::restore) returns nothing.
    pub fn eject_at(&mut self, speed_mph: f32, rng: &mut NavRng) -> Vec<usize> {
        if self.ejected {
            return Vec::new();
        }
        self.ejected = true;
        let wheels: Vec<usize> = self
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.spec.gate == BreakGate::Damage)
            .map(|(i, _)| i)
            .collect();
        if wheels.is_empty() || !speed_mph.is_finite() {
            return Vec::new();
        }
        if speed_mph > Self::EJECT_ALL_MPH {
            return wheels;
        }
        let pairs = if speed_mph > Self::EJECT_TWO_MPH {
            2
        } else if speed_mph > Self::EJECT_ONE_MPH {
            1
        } else {
            0
        };
        let pick = |rng: &mut NavRng| wheels[(rng.next_u64() % wheels.len() as u64) as usize];
        (0..pairs).map(|_| pick(rng)).collect()
    }

    /// Mark a part detached and record the fragment body it spawned
    /// as (`None` when no body was spawned). Returns `false` for an
    /// already-detached part or a bad index — a part leaves the rig at
    /// most once per attachment, which is what bounds the
    /// [`PartDetached`] stream (F05-AC03/AC06).
    pub fn detach(&mut self, index: usize, fragment: Option<Entity>) -> bool {
        let Some(part) = self.parts.get_mut(index) else {
            return false;
        };
        if !part.attached {
            return false;
        }
        part.attached = false;
        part.fragment = fragment;
        true
    }

    /// Re-attach one part — the per-part half of [`restore`](Self::restore)
    /// a replicated-state consumer needs: the authority's detach mask
    /// can clear a single bit while others stay off (F25-B). Returns
    /// the fragment entity the part spawned as for the caller to
    /// despawn; `None` for an attached part or a bad index.
    pub fn attach(&mut self, index: usize) -> Option<Entity> {
        let part = self.parts.get_mut(index)?;
        if part.attached {
            return None;
        }
        part.attached = true;
        part.fragment.take()
    }

    /// Restore the whole rig (the repair side of the disabled
    /// outcome): every part re-attaches, the damage-driven episode flag
    /// clears (the original's model reset `0x4cdfe0` re-arms it), and
    /// the fragment entities it spawned are returned for the caller to
    /// despawn. Idempotent — an untouched rig returns empty.
    pub fn restore(&mut self) -> Vec<Entity> {
        self.ejected = false;
        let mut fragments = Vec::new();
        for part in &mut self.parts {
            part.attached = true;
            if let Some(e) = part.fragment.take() {
                fragments.push(e);
            }
        }
        fragments
    }

    /// Count of parts currently off the rig — evidence for the smoke
    /// record and the restore path's "was there anything to do" check.
    pub fn detached_count(&self) -> usize {
        self.parts.iter().filter(|p| !p.attached).count()
    }
}

/// One breakaway part left its rig — the semantic stream
/// audio/replication consumers read instead of watching entities.
/// Bounded by construction: [`VehicleBreaks::detach`] refuses a
/// second detach on the same part, so one event fires per part per
/// attachment (F05-AC06).
#[derive(Message, Debug, Clone)]
pub struct PartDetached {
    /// Stable identity of the vehicle the part came off.
    pub object: ObjectId,
    /// Session generation the detach belongs to.
    pub generation: u64,
    /// Fixed-step session tick it happened on.
    pub tick: u64,
    /// The part's model stem (`break01`).
    pub part: String,
    /// The fragment body's minted identity — `None` when the part
    /// detached without spawning a body (pool bound).
    pub fragment: Option<ObjectId>,
    /// The quantity that drove the detach: the impact's delivered
    /// impulse for an impact-gated panel (approach speed × the other
    /// participant's mass, kg·m/s — REC-2/REC-3), or the recovered
    /// `Speed × 1.3` fragment kick (m/s) for a damage-gated wheel.
    pub estimate: f32,
}

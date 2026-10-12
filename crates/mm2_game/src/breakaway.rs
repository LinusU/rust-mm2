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
//! Detachment is impact-driven, not damage-driven, and the gate is
//! recovered from the retail binary (REC-2): an impact whose delivered
//! impulse reaches a manager-wide `10000` kg·m/s takes **one** part —
//! the attached part whose anchor is nearest the contact point. A
//! part's authored `ImpulseLimit2` does not gate detachment at all; it
//! only governs the *spawned fragment's* later collisions through the
//! ordinary banger gate (OPR-1.3). The authored `ImpactThreshold` sits
//! below `10000` on every retail record (1500, or 100 on `vpcaddie`),
//! so the recovered leaf's `impulse > ImpactThreshold` precondition is
//! already satisfied whenever the manager gate trips. The impulse
//! compared is the same estimate [`crate::VehicleDamage`] accumulates —
//! approach speed × the other participant's mass (the struck car's own
//! mass for a wall hit) — which REC-3 measures as the original's unit,
//! kg·m/s, though a lower bound for the same wall hit. A part never
//! detaches on a damage-tier crossing. Repairing the rig (the disabled
//! outcome's reset) re-attaches every part and retires the spawned
//! fragments; a plain reset or stuck recovery does not repair, so
//! detached parts stay off.
//!
//! Lifecycle per part:
//!
//! ```text
//!   Attached (rides the rig, intact chunk rendered)
//!     ── delivered impulse reaches the manager-wide 10000 ──▶
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
    /// (`break0`, `break01`, ...).
    pub name: String,
    /// The part's own `tune/banger/<id>_<name>.dgbangerdata` record,
    /// distilled — authored threshold and fragment physicals.
    pub def: BangerDefinition,
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
    /// part identity.
    pub parts: Vec<BreakPartState>,
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

    /// Whether an impact of `impulse` kg·m/s — the same estimate
    /// [`crate::VehicleDamage`] accumulates, approach speed × the other
    /// participant's mass — is strong enough to take a part and one is
    /// still attached. Non-finite or negative impulses are rejected: a
    /// junk delivery is never a detach.
    pub fn detaches_on(&self, impulse: f32) -> bool {
        impulse.is_finite()
            && impulse >= Self::DETACH_IMPULSE
            && self.parts.iter().any(|p| p.attached)
    }

    /// The attached part nearest `contact` — the recovered rule's
    /// smallest-squared-distance scan over attached parts (REC-2); the
    /// original detaches exactly this one part per qualifying impact.
    /// `anchors` yields `(part index, anchor)` in the same space as
    /// `contact`; a part with no anchor (no render node) or a non-finite
    /// one is simply absent and can never be chosen. `None` on a
    /// non-finite contact, on an out-of-range index or when no attached
    /// part is a candidate; ties keep the lowest index.
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
            .filter(|(i, a)| self.parts.get(*i).is_some_and(|p| p.attached) && a.is_finite())
            .map(|(i, a)| (i, (a - contact).length_squared()))
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(i, _)| i)
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
    /// outcome): every part re-attaches and the fragment entities it
    /// spawned are returned for the caller to despawn. Idempotent —
    /// an untouched rig returns empty.
    pub fn restore(&mut self) -> Vec<Entity> {
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
    /// The impact's delivered impulse — approach speed × the other
    /// participant's mass, in kg·m/s — that reached the manager-wide
    /// gate (REC-2/REC-3). Not the part's `ImpulseLimit2`.
    pub estimate: f32,
}

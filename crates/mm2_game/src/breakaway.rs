//! Vehicle breakaway-part contract (F05-B.3).
//!
//! Stock cars ship detachable panels: `BREAK<NN>` chunks inside the
//! vehicle's own PKG are the intact representation, and each is bound
//! to a `tune/banger/<id>_break<NN>.dgbangerdata` fragment record —
//! the same `<base>_break<N>` naming the prop-side audit resolves
//! (REC-1, WLD-15). The record carries the part's authored detach
//! threshold (`ImpulseLimit2`) and fragment physicals (mass, friction,
//! elasticity) — original data.
//!
//! What the original compares `ImpulseLimit2` against is unverified
//! for vehicles as it is for props (UNK-22), and whether the original
//! detaches on impact severity or on accumulated damage is unrecovered
//! (UNK-13). The authored data pins the unit down hard, though: on
//! every retail vehicle fragment record `ImpulseLimit2` is `Mass ×
//! constant` — ≈31.25 m/s (70 mph) on ordinary panels, ≈2500 m/s
//! (effectively never) on heavy rigs like vpftruck's anchors. The
//! implemented reading is therefore the one the authored values are
//! shaped for (DSN-21): a part detaches when the impact's approach
//! speed delivers more than its authored limit measured against the
//! part's *own* mass — `severity × part_mass > ImpulseLimit2`, so
//! `limit / mass` reads directly as the authored detach speed. A part
//! never detaches on a damage-tier crossing. Repairing the rig (the
//! disabled outcome's reset) re-attaches every part and retires the
//! spawned fragments; a plain reset or stuck recovery does not repair,
//! so detached parts stay off.
//!
//! Lifecycle per part:
//!
//! ```text
//!   Attached (rides the rig, intact chunk rendered)
//!     ── delivered impulse > authored ImpulseLimit2 ──▶
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
/// runtime consumes the record's physicals and threshold only; the
/// fragment's pose comes from the detached geometry itself (DSN-21).
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

    /// Indices of attached parts whose authored `ImpulseLimit2` the
    /// approach speed exceeds — `approach_speed × part_mass` is the
    /// impulse the hit delivers to the part, and `limit / mass` is
    /// ≈31.25 m/s (or ≈2500, "never") on every retail record (DSN-21,
    /// UNK-22). Non-finite or non-positive speeds detach nothing: a
    /// resting contact is not a detach event.
    pub fn detachable(&self, approach_speed: f32) -> Vec<usize> {
        if !approach_speed.is_finite() || approach_speed <= 0.0 {
            return Vec::new();
        }
        self.parts
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.attached && p.spec.def.activates_on(approach_speed * p.spec.def.mass)
            })
            .map(|(i, _)| i)
            .collect()
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
    /// The impulse the hit delivered to the part — `approach_speed ×
    /// part_mass` — that exceeded the authored limit (the DSN-21
    /// reading of UNK-22's quantity).
    pub estimate: f32,
}

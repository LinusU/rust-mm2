//! NETPLAY-08C: force-affecting modifiers at the correction's tick.
//!
//! A replay restores the recorded capture of the compared tick, so the
//! body's engine impairment (damage), mass and inertia (carried gold) are
//! the *predicted* ones of that tick. The authority may have decided
//! otherwise by then and told us later, on another channel: the damage
//! byte of the snapshot, the Cops & Robbers frame. Those are state, not
//! impulses; this module turns them into the value the compared tick
//! should have had and the replay stamps into the restored capture, so the
//! rewritten history carries one consistent past. Nothing here applies an
//! impulse: a contact's own effect stays with the presentation ledger and
//! the replayed physics.
//!
//! - **Damage.** The snapshot's damage byte is the seat's total at the
//!   snapshot's host tick, the same instant as the assertion, so its
//!   impairment factor is exact for the compared tick.
//! - **Gold load.** The replica frame has no snapshot tick (see
//!   [`crate::cnr::authored_load_kg`]); the newest load is applied from the
//!   compared tick, the oldest the replay can rewrite. Designed bounded
//!   policy: the lag is the replica's own latency, and the first assertion
//!   after the frame corrects the history instead of waiting for a pose
//!   divergence to show it.
//! - **Trailer joints.** The hitch is part of every capture and restored
//!   with it ([`mm2_vehicle::physics_world::Hitch`]); the wire carries no
//!   joint change, so the tick's own recorded hitch is the authority.

use bevy::prelude::*;
use mm2_vehicle::capture::BodyModifiers;

/// Impairment factors closer than this are the same (the factor ramps
/// linearly; a thousandth of rated torque is below any observable force).
pub const IMPAIRMENT_EPSILON: f32 = 1e-3;
/// Masses closer than this (kg) are the same.
pub const MASS_EPSILON: f32 = 1e-2;

/// What the authority has said about one body's modifiers. A field the
/// authority has not spoken to stays `None` and the recorded value stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Authored {
    pub body: Entity,
    /// Engine impairment factor: `Some(None)` is "unimpaired".
    pub impairment: Option<Option<f32>>,
    /// Carried load in kg, with the body's unloaded mass and principal
    /// inertia to apply it to.
    pub load: Option<Load>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Load {
    pub added_kg: f32,
    pub base_mass: f32,
    pub base_inertia: Vec3,
}

/// An impairment factor as the component stores it: absent at full power.
pub fn impairment_component(factor: f32) -> Option<f32> {
    (factor.is_finite() && factor < 1.0).then(|| factor.clamp(0.0, 1.0))
}

impl Authored {
    /// The modifiers the compared tick should have had, given those it
    /// recorded.
    pub fn resolve(&self, recorded: &BodyModifiers) -> BodyModifiers {
        let mut out = *recorded;
        if let Some(impairment) = self.impairment {
            out.impairment = impairment;
        }
        if let Some(load) = self.load
            && load.base_mass.is_finite()
            && load.base_mass > 0.0
        {
            let total = load.base_mass + load.added_kg;
            out.mass = total;
            out.inertia_principal = (load.base_inertia * (total / load.base_mass)).to_array();
        }
        out
    }
}

/// Whether two modifier sets differ by more than the epsilons.
pub fn differ(a: &BodyModifiers, b: &BodyModifiers) -> bool {
    let impairment = match (a.impairment, b.impairment) {
        (None, None) => false,
        (Some(x), Some(y)) => (x - y).abs() > IMPAIRMENT_EPSILON,
        _ => true,
    };
    impairment || (a.mass - b.mass).abs() > MASS_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> BodyModifiers {
        BodyModifiers {
            impairment: None,
            mass: 1000.0,
            inertia_principal: [10.0, 20.0, 30.0],
        }
    }

    #[test]
    fn a_load_scales_mass_and_inertia_from_the_unloaded_base() {
        let a = Authored {
            body: Entity::PLACEHOLDER,
            impairment: None,
            load: Some(Load {
                added_kg: 250.0,
                base_mass: 1000.0,
                base_inertia: Vec3::new(10.0, 20.0, 30.0),
            }),
        };
        // Whatever the record held (already loaded or not), the result is
        // the base plus the load, never a stacked one.
        let loaded = a.resolve(&BodyModifiers {
            mass: 1250.0,
            ..mods()
        });
        assert_eq!(loaded.mass, 1250.0);
        assert_eq!(loaded.inertia_principal, [12.5, 25.0, 37.5]);
        assert_eq!(a.resolve(&mods()), loaded);
    }

    #[test]
    fn a_degenerate_base_leaves_the_record_alone() {
        let a = Authored {
            body: Entity::PLACEHOLDER,
            impairment: None,
            load: Some(Load {
                added_kg: 250.0,
                base_mass: 0.0,
                base_inertia: Vec3::ONE,
            }),
        };
        assert_eq!(a.resolve(&mods()), mods());
    }

    #[test]
    fn differences_inside_the_epsilons_are_the_same_state() {
        let a = mods();
        assert!(!differ(&a, &a));
        let b = BodyModifiers {
            impairment: Some(0.8),
            ..a
        };
        assert!(differ(&a, &b));
        let c = BodyModifiers {
            impairment: Some(0.8 + IMPAIRMENT_EPSILON / 2.0),
            ..b
        };
        assert!(!differ(&b, &c));
        assert!(differ(&a, &BodyModifiers { mass: 1250.0, ..a }));
    }

    #[test]
    fn an_impairment_component_exists_only_below_full_power() {
        assert_eq!(impairment_component(1.0), None);
        assert_eq!(impairment_component(0.4), Some(0.4));
        assert_eq!(impairment_component(-1.0), Some(0.0));
        assert_eq!(impairment_component(f32::NAN), None);
    }
}

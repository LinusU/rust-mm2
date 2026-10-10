//! The tire path's view of the driving surface (F06-B).
//!
//! `mm2_vehicle` cannot see the game's surface *identity* contract —
//! `SurfaceMaterial`/`SurfaceTables` live in `mm2_game`/`mm2_content`,
//! above this crate in the dependency direction — so the physics side
//! declares the two inputs it actually consumes and whoever spawns
//! colliders translates the authored material tables into them:
//!
//! - [`TireSurface`] — a component on collider entities carrying the
//!   normalized grip multiplier the tire model applies over that
//!   collider. A collider without one is the neutral reference surface.
//! - [`TireConditions`] — the session's environment traction modifier
//!   (wetness, ice, …), kept deliberately separate from the authored
//!   base material so a weather change never rewrites collider data.
//!
//! Both are plain data read once per grounded wheel inside the physics
//! step: the texture→csv→mtl lookup happened at import, so the hot loop
//! stays a component read (F06 spec req 6). `grip`/`drag` steer the
//! tire model only — the authored `elasticity` feeds Avian's own
//! `Restitution` on the collider (attached by the spawner, not this
//! component), and `sound`/`ptx*` still wait for the F07 consumers.

use bevy::prelude::*;

/// Physics-side surface properties a collider exposes to the tire path.
/// Attach to collider entities; wheel probes hitting an unmarked
/// collider treat it as the neutral reference surface.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct TireSurface {
    /// Grip multiplier applied to the tire's force limit over this
    /// collider: `1.0` is the reference surface the handling was tuned
    /// against, below `1.0` is slippery, above is grippier. The
    /// producer normalizes the authored material `friction` so the
    /// table's `_default` block lands exactly on `1.0` — authored
    /// differences scale delivered force, never commanded steering
    /// geometry. Must be finite and `>= 0`; the sim clamps negative or
    /// non-finite values to `0` rather than flipping a force's sign.
    pub grip: f32,
    /// Viscous resistance the surface applies to a wheel rolling over
    /// it — the authored `drag` field, which only water (`0.119`) and
    /// deepwater (`0.5`) carry nonzero on retail: it is the wading
    /// term, not rolling resistance. The sim applies a contact-plane
    /// force opposing the wheel's motion proportional to `drag ×
    /// load`, so a car bogs down crossing water instead of sliding
    /// over it. `0.0` is none — the neutral value every dry surface
    /// takes. Must be finite and `>= 0`; the sim clamps the value into
    /// `0..=1e4` so a bad coefficient can stall a car but never
    /// produce a non-finite force.
    pub drag: f32,
}

impl Default for TireSurface {
    /// The neutral reference surface: unmodified tire grip, no wading
    /// resistance.
    fn default() -> Self {
        Self {
            grip: 1.0,
            drag: 0.0,
        }
    }
}

/// Session-wide environment traction modifier — wetness, ice, packed
/// snow — applied on top of every contact's authored material grip
/// (F06 spec req 2: base material and environment modifier stay
/// separate terms, multiplied once into one effective coefficient).
///
/// `traction = 1.0` is *unmodified* and the default. Two writers exist
/// above this crate: the session's effective weather selector through a
/// designed wetness factor (F18-B.1 — `rainy` wets the road; the
/// original's scale is unrecovered, UNK-39), and a quarantined
/// dev/diagnostic pin that overrides it. Read-only inside the physics
/// loop; session-scoped like the other world inputs.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct TireConditions {
    /// Environment grip multiplier applied to every tire contact.
    /// Must be finite and `>= 0` (writers validate); the sim clamps a
    /// bad value to `0` rather than producing a negative force.
    pub traction: f32,
}

impl Default for TireConditions {
    /// No environment modification: `traction` = `1.0`.
    fn default() -> Self {
        Self { traction: 1.0 }
    }
}

/// Raw source material for a world collider's contact with a native hull.
/// Generic Avian bodies retain their existing collider coefficients.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct OriginalContactMaterial {
    pub friction: f32,
    pub elasticity: f32,
}

impl OriginalContactMaterial {
    /// Source material-pair products (research 01, `0x46cce0`).
    /// Invalid external components do not replace valid Avian coefficients.
    pub fn combined(self, other: Self) -> Option<(f32, f32)> {
        if [
            self.friction,
            self.elasticity,
            other.friction,
            other.elasticity,
        ]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        {
            return None;
        }
        let friction = self.friction * other.friction;
        friction
            .is_finite()
            .then_some((friction, (self.elasticity * other.elasticity).min(1.0)))
    }
}

#[cfg(test)]
mod original_material_tests {
    use super::*;

    #[test]
    fn original_pair_products_retain_raw_coefficients() {
        let rover = OriginalContactMaterial {
            friction: 0.9,
            elasticity: 0.5,
        };
        let ground = OriginalContactMaterial {
            friction: 1.0,
            elasticity: 0.5,
        };
        assert_eq!(rover.combined(ground), Some((0.9, 0.25)));
        assert_eq!(rover.combined(rover), Some((0.9 * 0.9, 0.25)));
        let high = OriginalContactMaterial {
            friction: 2.0,
            elasticity: 2.0,
        };
        assert_eq!(high.combined(high), Some((4.0, 1.0)));
        for invalid in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(
                rover
                    .combined(OriginalContactMaterial {
                        friction: invalid,
                        ..ground
                    })
                    .is_none()
            );
            assert!(
                rover
                    .combined(OriginalContactMaterial {
                        elasticity: invalid,
                        ..ground
                    })
                    .is_none()
            );
        }
    }
}

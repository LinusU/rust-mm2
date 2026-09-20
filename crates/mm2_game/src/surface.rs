//! Physical surface identity at contacts (F01-B contract).
//!
//! [`SurfaceMaterial`] is the *physical* identity of what a wheel or
//! chassis is touching — deliberately independent of the visual texture
//! so a reskin never changes traction. The authored material taxonomy is
//! not verified yet (F06 maps PSDL attributes to it), so the type
//! preserves authored codes without claiming what they mean, exactly
//! like the weather/time-of-day selectors in `config.rs` (UNK-1).

use bevy::prelude::*;

/// The physical material a collider is made of. Attach to collider
/// entities; contacts against unmarked colliders report
/// [`Unspecified`](Self::Unspecified).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SurfaceMaterial {
    /// No authored material was attached — or the source content does not
    /// author one. The honest value until F06 classifies materials.
    #[default]
    Unspecified,
    /// An authored material code preserved from source data whose
    /// semantics are unverified. Carried as data; nothing may interpret
    /// the number as a named surface yet.
    Authored(u16),
}

/// Effective surface state at a contact: the base material plus the
/// environment's traction modifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceState {
    /// Physical material identity.
    pub material: SurfaceMaterial,
    /// Traction multiplier the environment applies on top of the
    /// material's authored grip (wetness, ice, …). `1.0` means
    /// *unmodified* — no modifier is applied today; F06 wires authored
    /// materials and weather into this field.
    pub traction: f32,
}

impl Default for SurfaceState {
    fn default() -> Self {
        Self {
            material: SurfaceMaterial::Unspecified,
            traction: 1.0,
        }
    }
}

impl SurfaceState {
    /// A state over `material` with no environment modification.
    pub fn of(material: SurfaceMaterial) -> Self {
        Self {
            material,
            traction: 1.0,
        }
    }
}

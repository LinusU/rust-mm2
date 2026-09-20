//! Physical surface identity at contacts (F01-B contract).
//!
//! [`SurfaceMaterial`] is the *physical* identity of what a wheel or
//! chassis is touching — deliberately independent of the visual texture
//! so a reskin never changes traction. F06-A binds `Authored(i)` to the
//! session's loaded `materials.mtl` index space; what the material
//! fields *mean* to the original force path stays unverified (UNK-23),
//! so the code is carried, not interpreted, exactly like the
//! weather/time-of-day selectors in `config.rs` (UNK-1).

use bevy::prelude::*;

/// The physical material a collider is made of. Attach to collider
/// entities; contacts against unmarked colliders report
/// [`Unspecified`](Self::Unspecified).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SurfaceMaterial {
    /// No authored material was attached, the source content does not
    /// author one, or the surface tables could not classify it (`none`
    /// rows, blank slots, unmapped names all land here).
    #[default]
    Unspecified,
    /// The index of a material in the session's loaded
    /// `city/materials.mtl` table (`MaterialSet::defs`). Carried as
    /// data; nothing may interpret the index as a named surface yet
    /// (UNK-23).
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

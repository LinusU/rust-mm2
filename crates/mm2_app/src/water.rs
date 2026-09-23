//! F18-A.6 — the authored `city/<stem>.water` deadly-water record.
//!
//! The file shape is verified (`mm2_formats::water::WaterDef`): a
//! float level plus integer refs — London `-3.8` + `[345, 351, 356]`,
//! SF `-1.9` + `[228, 399, 401]`. Retail probing shows the refs are
//! 1-based PSDL room ids — the same space `PerimeterPoint.room`,
//! `road_rooms` and the `.cpvs` lists use: all six resolve to the flat
//! water-plane rooms sitting just under the authored level (the Thames
//! fans at y ≈ −4.0, the ocean at ≈ −2.0); the 0-based reading lands
//! SF's `401` on a road tunnel, which refutes it. The community
//! description (mm2kiwi) reads the level as the deadly-water height
//! and the ids as rooms whose water is deadly "at any height"; the
//! original runtime consumer is unrecovered, so the interpretation
//! here is a designed policy (DSN — the original rules stay unknown):
//!
//! - a point is **deadly-water exposed** when it lies inside a listed
//!   room's authored XZ perimeter *and* at/below that room's bound —
//!   `max(level, room-top)`. `max` keeps the authored level deadly
//!   inside a listed room even when the room's geometry dips below it,
//!   and honours "at any height" for a mod room whose water plane sits
//!   above the global level. [`SURFACE_SLACK`] covers contact-vs-plane
//!   float noise on that elevated bound (the retail bound is the level
//!   itself, which already sits ~0.2 m above the water plane);
//! - the level never applies outside the listed rooms: London carries
//!   real BAI lanes down to y ≈ −22 under the −3.8 level (below-grade
//!   roads), so a global below-level kill is refuted by the data;
//! - exposure feeds `crate::recovery::track_recovery`'s contact
//!   classification: a wheel contact inside a listed room at/below the
//!   bound is water whatever the collider's `drag` says, and a car
//!   under a listed room's bound with no contact at all — clipped
//!   through the plane — drowns rather than free-falls. A bridge deck
//!   above the bound inside the same XZ perimeter stays dry: the bound
//!   still applies vertically.
//!
//! A missing or unparseable `.water`, a non-finite level, or refs that
//! resolve to no rooms simply yield no resource — the F05-B.5
//! surface-`drag` classification still covers the ordinary water
//! materials, so the failure direction is "no authored escalation",
//! never a fabricated kill zone. The resource is session-scoped like
//! [`crate::pvs::CityPvs`]: inserted by `load_session_world` and
//! removed by `drive_session`'s teardown.

use bevy::prelude::*;
use mm2_formats::{psdl::Psdl, water::WaterDef};

use crate::city::{authored_z, point_in_poly, room_poly};

/// Vertical slack on a deadly room's bound, metres. The retail bound
/// is the authored level, already ~0.2 m above the water plane; the
/// slack exists for a mod room whose bound is its own surface, where a
/// resting wheel's raycast contact can report a hair above it.
const SURFACE_SLACK: f32 = 0.1;

/// One `.water` ref resolved to its PSDL room.
struct DeadlyRoom {
    /// The authored 1-based room id (the ref value).
    id: u32,
    /// XZ containment polygon in authored (x, z) space.
    poly: Vec<(f32, f32)>,
    /// World Y at/below which a contained point is deadly:
    /// `max(level, room-top)` — the authored level where the room's
    /// own geometry sits lower, the room's top where a mod lists
    /// water above the global level ("at any height").
    bound: f32,
}

/// The session's deadly-water volumes, built from the city's authored
/// `.water` record over the loaded PSDL. Absent on the dev world and
/// on cities whose install ships no usable record.
#[derive(Resource)]
pub struct CityWater {
    /// The authored level verbatim (world Y) — evidence for the smoke
    /// record and for consumers that want the raw datum.
    level: f32,
    /// Refs that resolved to a room.
    rooms: Vec<DeadlyRoom>,
    /// Refs skipped because they didn't resolve (`<= 0` or beyond the
    /// room count) — a mod authoring bug, logged at load.
    skipped: usize,
}

impl CityWater {
    /// Bind a parsed `.water` record to the loaded PSDL's rooms. Refs
    /// are 1-based room ids (verified on retail); a ref outside
    /// `1..=rooms.len()` is skipped, never reinterpreted.
    pub fn build(def: &WaterDef, psdl: &Psdl) -> Self {
        let mut rooms = Vec::new();
        let mut skipped = 0;
        for &r in &def.refs {
            let room = (r >= 1).then(|| psdl.rooms.get(r as usize - 1)).flatten();
            let Some(room) = room else {
                skipped += 1;
                continue;
            };
            let top = room
                .perimeter
                .iter()
                .filter_map(|p| psdl.vertices.get(p.vertex as usize))
                .map(|v| v[1])
                .fold(f32::NEG_INFINITY, f32::max);
            rooms.push(DeadlyRoom {
                id: r as u32,
                bound: if top.is_finite() {
                    def.level.max(top)
                } else {
                    def.level
                },
                poly: room_poly(psdl, room),
            });
        }
        CityWater {
            level: def.level,
            rooms,
            skipped,
        }
    }

    /// The authored water level (world Y).
    pub fn level(&self) -> f32 {
        self.level
    }

    /// How many refs resolved to rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// How many refs failed to resolve.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// The 1-based room ids that resolved — diagnostic order matches
    /// the file's ref order.
    pub fn room_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.rooms.iter().map(|r| r.id)
    }

    /// Whether `p` (world space) sits inside a listed room's XZ
    /// perimeter at/below that room's deadly bound.
    pub fn is_deadly(&self, p: Vec3) -> bool {
        let pt = (p.x, authored_z(p.z));
        self.rooms.iter().any(|r| {
            p.y <= r.bound + SURFACE_SLACK && r.poly.len() >= 3 && point_in_poly(pt, &r.poly)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::psdl::{PerimeterPoint, PsdlRoom};

    /// A two-room PSDL: room 1 is a flat water plane at `water_y`
    /// covering `x ∈ [0,8], z ∈ [0,8]`; room 2 is a flat "below-grade
    /// road" at `road_y` on the next tile — the London-tunnel shape
    /// that must never drown.
    fn psdl_with_water(water_y: f32, road_y: f32) -> Psdl {
        let room = |verts: &[[f32; 3]], base: u16| PsdlRoom {
            perimeter: (0..verts.len())
                .map(|k| PerimeterPoint {
                    vertex: base + k as u16,
                    room: 0,
                })
                .collect(),
            attributes: Vec::new(),
            unparsed_attributes: Vec::new(),
        };
        let water_verts = [
            [0.0, water_y, 0.0],
            [8.0, water_y, 0.0],
            [8.0, water_y, 8.0],
            [0.0, water_y, 8.0],
        ];
        let road_verts = [
            [10.0, road_y, 0.0],
            [18.0, road_y, 0.0],
            [18.0, road_y, 8.0],
            [10.0, road_y, 8.0],
        ];
        let rooms = vec![
            room(&water_verts, 0),
            room(&road_verts, water_verts.len() as u16),
        ];
        Psdl {
            target_size: 2,
            vertices: [water_verts, road_verts].concat(),
            heights: Vec::new(),
            textures: Vec::new(),
            rooms,
            room_flags: Vec::new(),
            prop_rules: Vec::new(),
            junction_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            bounds_center: [0.0; 3],
            bounds_radius: 0.0,
            paths: Vec::new(),
        }
    }

    /// The Bevy-space position over authored `(x, z)` at height `y`.
    fn world(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, if crate::city::MIRROR_Z { -z } else { z })
    }

    #[test]
    fn refs_are_one_based_room_ids() {
        let psdl = psdl_with_water(-4.0, -20.0);
        let def = WaterDef::parse("-3.8\n1\n2").unwrap();
        let water = CityWater::build(&def, &psdl);
        assert_eq!(water.level(), -3.8);
        assert_eq!(water.room_ids().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(water.skipped(), 0);
    }

    #[test]
    fn unresolvable_refs_are_skipped_not_reinterpreted() {
        let psdl = psdl_with_water(-4.0, -20.0);
        // 0, negative and past-the-end refs resolve to nothing.
        let def = WaterDef::parse("-3.8\n0\n-2\n1\n99").unwrap();
        let water = CityWater::build(&def, &psdl);
        assert_eq!(water.room_ids().collect::<Vec<_>>(), vec![1]);
        assert_eq!(water.skipped(), 3);
    }

    #[test]
    fn exposure_needs_the_room_and_the_bound() {
        let psdl = psdl_with_water(-4.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl);
        // Inside room 1's tile, below the level: deadly.
        assert!(water.is_deadly(world(4.0, -4.0, 4.0)));
        assert!(water.is_deadly(world(4.0, -10.0, 4.0)));
        // Inside room 1's tile but above the level — a bridge deck
        // over the water stays dry.
        assert!(!water.is_deadly(world(4.0, 5.0, 4.0)));
        // Below the level but outside every listed room — London's
        // below-grade roads (room 2 sits at −20) never drown.
        assert!(!water.is_deadly(world(14.0, -20.0, 4.0)));
        assert!(!water.is_deadly(world(40.0, -50.0, 4.0)));
    }

    #[test]
    fn an_elevated_room_binds_at_its_own_height() {
        // A mod room whose water plane sits above the authored level —
        // "deadly water at any height": the room's own top becomes the
        // bound, so the surface drowns while a deck above it is safe.
        let psdl = psdl_with_water(10.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl);
        assert!(water.is_deadly(world(4.0, 10.0, 4.0)));
        assert!(water.is_deadly(world(4.0, 6.0, 4.0)));
        assert!(!water.is_deadly(world(4.0, 12.0, 4.0)));
    }
}

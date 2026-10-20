//! F18-A.6/.7 — the authored `city/<stem>.water` deadly-water record
//! and the loader's SDL-side room marking.
//!
//! The file shape is verified (`mm2_formats::water::WaterDef`): a
//! float level plus integer refs — London `-3.8` + `[345, 351, 356]`,
//! SF `-1.9` + `[228, 399, 401]`. Retail probing shows the refs are
//! 1-based PSDL room ids — the same space `PerimeterPoint.room`,
//! `road_rooms` and the `.cpvs` lists use: all six resolve to the flat
//! water-plane rooms sitting just under the authored level (the Thames
//! fans at y ≈ −4.0, the ocean at ≈ −2.0); the 0-based reading lands
//! SF's `401` on a road tunnel, which refutes it.
//!
//! The marking semantics are verified from the retail exe
//! (Midtown2.exe, `cityLevel::Load`, 2026-09-23): rooms are flagged
//! deadly from two sources — each `.water` ref bounds-checks and sets
//! a room's water flag (`"Room %d has Water of Death(tm) [from .water
//! file]"`), and an SDL pass marks every room whose *first* attribute
//! is a `TextureRef` naming a drowning-class surface (`"[from SDL]"`).
//! The kill test is `flagged && pos.y < level` against the one global
//! level (`GetWaterLevel`); which surfaces carry the class flag is an
//! mtl-side derivation we haven't recovered, so
//! [`SurfaceTables::is_deadly_surface`] approximates it with the same
//! `water_min_drag` boundary the wheel classifier uses — retail's only
//! qualifying material is `deepwater` (drag 0.5; `water` at 0.119
//! stays wadeable).
//!
//! The exposure shape stays a designed policy (DSN-34 — the original
//! tests room occupancy; we test point-in-perimeter, which is the same
//! set for a point inside the room):
//!
//! - a point is **deadly-water exposed** when it lies inside a listed
//!   room's authored XZ perimeter *and* at/below that room's bound.
//!   `.water`-ref rooms bound at `max(level, room-top)` — `max` keeps
//!   the authored level deadly inside a listed room even when the
//!   room's geometry dips below it, and honours mm2kiwi's "deadly
//!   water at any height" for a mod room whose water plane sits above
//!   the global level. SDL-marked rooms bound at the authored `level`
//!   verbatim, matching `GetWaterLevel`'s single global value.
//!   [`SURFACE_SLACK`] covers contact-vs-plane float noise, and the
//!   column stops [`WATER_DEPTH`] below the bound so a tunnel under a
//!   river (London's Thames tunnel, found by the retail route audit
//!   in `tests/recovery.rs`) is not drowned;
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
//! A missing or unparseable `.water`, a non-finite level, or a build
//! resolving to no rooms simply yields no resource — `load_city` drops
//! an all-unresolvable record like a missing one, so a present
//! `CityWater` always lists at least one deadly room. No surface
//! tables (`materials.{mtl,csv}` absent) skips only the SDL pass — the
//! authored refs still apply. The F05-B.5 surface-`drag`
//! classification still covers the ordinary water materials, so the
//! failure direction is "no authored escalation", never a fabricated
//! kill zone. The resource is session-scoped like
//! [`crate::pvs::CityPvs`]: inserted by `load_session_world` and
//! removed by `drive_session`'s teardown.

use avian3d::prelude::{
    Forces, Gravity, PhysicsSchedule, Position, ReadRigidBodyForces, WriteRigidBodyForces,
};
use bevy::prelude::*;
use mm2_content::SurfaceTables;
use mm2_formats::{
    psdl::{AttributeType, Psdl, texture_ref_index},
    water::WaterDef,
};

use crate::city::{authored_z, point_in_poly, room_poly};

/// Vertical slack on a deadly room's bound, metres. The retail bound
/// is the authored level, already ~0.2 m above the water plane; the
/// slack exists for a mod room whose bound is its own surface, where a
/// resting wheel's raycast contact can report a hair above it.
const SURFACE_SLACK: f32 = 0.1;

/// How far below a deadly room's bound its water column reaches,
/// metres. The original tests room occupancy; a point-in-perimeter test
/// also catches whatever lies *under* the room's footprint, and London's
/// Thames tunnel (rooms 670-672, floors at y ≈ −13..−11) runs 7+ m
/// under rooms 347-349 (surface y ≈ −4.0) — an authored Blitz gate and
/// every car driving it would otherwise drown. Sunk deeper than this a
/// car is no longer in the water room: it reads as airborne and the
/// out-of-bounds legs own it.
const WATER_DEPTH: f32 = 5.0;

/// Nominal hull height the buoyancy fraction is measured over, metres.
/// Designed (#1715, DSN-23): the retail float behaviour is unrecovered.
const BODY_HEIGHT: f32 = 1.5;
/// Buoyant acceleration at full submersion, in multiples of gravity: 2
/// puts equilibrium with the centre of mass on the surface (half the
/// hull under). Designed.
const BUOYANCY_RATIO: f32 = 2.0;
/// Vertical velocity damping in water, 1/s. Under critical for the
/// spring `BUOYANCY_RATIO·g/BODY_HEIGHT`, so a car dropped in bobs a
/// few times and settles. Designed.
const VERTICAL_DAMPING: f32 = 4.0;
/// Horizontal velocity damping in water, 1/s. Designed.
const HORIZONTAL_DAMPING: f32 = 0.8;
/// Angular velocity damping in water, 1/s. Designed.
const ANGULAR_DAMPING: f32 = 2.0;

/// The buoyant (upward) acceleration and damping for a body at
/// `centre` moving at `velocity`/`angular` in water whose surface is at
/// `surface`; `gravity` is the vehicle's gravity magnitude. Returns
/// `(linear, angular)` accelerations.
pub fn water_accelerations(
    centre_y: f32,
    surface: f32,
    gravity: f32,
    velocity: Vec3,
    angular: Vec3,
) -> (Vec3, Vec3) {
    let submerged = (0.5 + (surface - centre_y) / BODY_HEIGHT).clamp(0.0, 1.0);
    let lift = Vec3::Y * BUOYANCY_RATIO * gravity * submerged;
    let damp = Vec3::new(
        -HORIZONTAL_DAMPING * velocity.x,
        -VERTICAL_DAMPING * velocity.y,
        -HORIZONTAL_DAMPING * velocity.z,
    ) * submerged;
    (lift + damp, -ANGULAR_DAMPING * submerged * angular)
}

/// Deep water is not ground (#1715): its collider is on the
/// collision-free `Water` layer, so a car sinks through the plane and
/// is carried here instead — buoyancy toward floating at the surface,
/// with damping, applied to the retail step's acceleration accumulator.
/// The submersion dwell in `track_recovery` still owns the rescue.
pub fn float_vehicles(
    water: Option<Res<CityWater>>,
    gravity: Res<Gravity>,
    mut vehicles: Query<
        (&mm2_vehicle::Vehicle, &Position, Forces),
        Without<mm2_vehicle::RemoteReplica>,
    >,
) {
    let Some(water) = water else {
        return;
    };
    for (vehicle, pos, mut forces) in &mut vehicles {
        let Some(surface) = water.surface_at(pos.0) else {
            continue;
        };
        let g = vehicle
            .config
            .original
            .as_ref()
            .map_or(gravity.0.length(), |o| o.gravity);
        let (lin, ang) = water_accelerations(
            pos.0.y,
            surface,
            g,
            forces.linear_velocity(),
            forces.angular_velocity(),
        );
        forces.apply_linear_acceleration(lin);
        forces.apply_angular_acceleration(ang);
    }
}

/// Registers [`float_vehicles`]; add it next to `VehiclePlugin`.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PhysicsSchedule,
            float_vehicles.in_set(mm2_vehicle::VehicleBodyForces),
        );
    }
}

/// Which of the exe's two deadly-water sources marked a room — the
/// authored `.water` refs or the SDL water-surface pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkSource {
    /// A `.water` ref — bound is `max(level, room-top)` (DSN-34).
    Ref,
    /// The SDL pass — bound is the authored level verbatim.
    Sdl,
}

/// One room resolved to its deadly-water volume.
struct DeadlyRoom {
    /// The 1-based room id (the `.water` ref value, or the SDL room).
    id: u32,
    /// XZ containment polygon in authored (x, z) space.
    poly: Vec<(f32, f32)>,
    /// World Y at/below which a contained point is deadly.
    bound: f32,
    /// Which source marked the room.
    source: MarkSource,
}

/// The session's deadly-water volumes, built from the city's authored
/// `.water` record plus the PSDL's SDL water-surface marks. Absent on
/// the dev world and on cities whose install ships no usable record.
#[derive(Resource)]
pub struct CityWater {
    /// The authored level verbatim (world Y) — evidence for the smoke
    /// record and for consumers that want the raw datum.
    level: f32,
    /// Rooms marked by either source.
    rooms: Vec<DeadlyRoom>,
    /// Refs skipped because they didn't resolve (`<= 0` or beyond the
    /// room count) — a mod authoring bug, logged at load.
    skipped: usize,
}

impl CityWater {
    /// Bind a parsed `.water` record to the loaded PSDL's rooms. Refs
    /// are 1-based room ids (verified on retail); a ref outside
    /// `1..=rooms.len()` is skipped, never reinterpreted. With
    /// `surfaces`, the SDL pass additionally marks every room whose
    /// first attribute is a `TextureRef` to a drowning-class surface —
    /// a room already ref'd keeps its authored bound.
    pub fn build(def: &WaterDef, psdl: &Psdl, surfaces: Option<&SurfaceTables>) -> Self {
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
                source: MarkSource::Ref,
            });
        }
        if let Some(tables) = surfaces {
            for (idx, room) in psdl.rooms.iter().enumerate() {
                let id = idx as u32 + 1;
                if rooms.iter().any(|r| r.id == id) {
                    continue;
                }
                let deadly = room
                    .attributes
                    .first()
                    .filter(|a| a.kind == AttributeType::TextureRef)
                    .and_then(texture_ref_index)
                    .and_then(|i| psdl.textures.get(i))
                    .is_some_and(|t| tables.is_deadly_surface(t));
                if deadly {
                    rooms.push(DeadlyRoom {
                        id,
                        bound: def.level,
                        poly: room_poly(psdl, room),
                        source: MarkSource::Sdl,
                    });
                }
            }
        }
        CityWater {
            level: def.level,
            rooms,
            skipped,
        }
    }

    /// Append a separately authored chunk without turning water into a global
    /// below-level kill plane. Each room keeps its own bound and polygon.
    pub(crate) fn append_chunk(&mut self, mut other: Self, room_offset: u32) {
        other.offset_room_ids(room_offset);
        self.rooms.extend(other.rooms);
        self.skipped += other.skipped;
    }

    pub(crate) fn offset_room_ids(&mut self, offset: u32) {
        for room in &mut self.rooms {
            room.id += offset;
        }
    }

    /// The authored water level (world Y).
    pub fn level(&self) -> f32 {
        self.level
    }

    /// How many rooms are marked deadly across both sources.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// How many rooms the SDL pass marked (water-surface texture) —
    /// the count the exe logs as `"[from SDL]"`.
    pub fn sdl_rooms(&self) -> usize {
        self.rooms
            .iter()
            .filter(|r| r.source == MarkSource::Sdl)
            .count()
    }

    /// How many refs failed to resolve.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// The 1-based room ids that resolved — diagnostic order matches
    /// the file's ref order, SDL marks appended in room order.
    pub fn room_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.rooms.iter().map(|r| r.id)
    }

    /// The surface height of the deadly water `p` is in: the bound of a
    /// listed room whose perimeter contains `p` and whose column
    /// reaches it (the same test as [`is_deadly`](Self::is_deadly)).
    pub fn surface_at(&self, p: Vec3) -> Option<f32> {
        let pt = (p.x, authored_z(p.z));
        self.rooms
            .iter()
            .filter(|r| {
                (r.bound - WATER_DEPTH..=r.bound + SURFACE_SLACK).contains(&p.y)
                    && r.poly.len() >= 3
                    && point_in_poly(pt, &r.poly)
            })
            .map(|r| r.bound)
            .reduce(f32::max)
    }

    /// Whether `p` (world space) sits inside a listed room's XZ
    /// perimeter at/below that room's deadly bound.
    pub fn is_deadly(&self, p: Vec3) -> bool {
        let pt = (p.x, authored_z(p.z));
        self.rooms.iter().any(|r| {
            (r.bound - WATER_DEPTH..=r.bound + SURFACE_SLACK).contains(&p.y)
                && r.poly.len() >= 3
                && point_in_poly(pt, &r.poly)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunk_water_retains_local_bounds_with_distinct_room_ids() {
        let mut first = CityWater::build(
            &WaterDef {
                level: 0.2,
                refs: vec![1],
            },
            &psdl_with_water(0.0, 2.0),
            None,
        );
        let mut second_psdl = psdl_with_water(4.0, 6.0);
        for point in &mut second_psdl.vertices {
            point[0] += 20.0;
        }
        let second = CityWater::build(
            &WaterDef {
                level: 4.2,
                refs: vec![1],
            },
            &second_psdl,
            None,
        );
        first.append_chunk(second, 2);
        assert_eq!(first.room_ids().collect::<Vec<_>>(), vec![1, 3]);
        assert!(first.is_deadly(Vec3::new(24.0, 4.0, 4.0)));
        assert!(!first.is_deadly(Vec3::new(4.0, 2.0, 4.0)));
        assert!(!first.is_deadly(Vec3::new(14.0, -5.0, 4.0)));
    }

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
        let water = CityWater::build(&def, &psdl, None);
        assert_eq!(water.level(), -3.8);
        assert_eq!(water.room_ids().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(water.skipped(), 0);
    }

    #[test]
    fn unresolvable_refs_are_skipped_not_reinterpreted() {
        let psdl = psdl_with_water(-4.0, -20.0);
        // 0, negative and past-the-end refs resolve to nothing.
        let def = WaterDef::parse("-3.8\n0\n-2\n1\n99").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        assert_eq!(water.room_ids().collect::<Vec<_>>(), vec![1]);
        assert_eq!(water.skipped(), 3);
    }

    #[test]
    fn exposure_needs_the_room_and_the_bound() {
        let psdl = psdl_with_water(-4.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        // Inside room 1's tile, below the level: deadly.
        assert!(water.is_deadly(world(4.0, -4.0, 4.0)));
        assert!(water.is_deadly(world(4.0, -8.0, 4.0)));
        // Inside room 1's tile but above the level — a bridge deck
        // over the water stays dry.
        assert!(!water.is_deadly(world(4.0, 5.0, 4.0)));
        // Below the level but outside every listed room — London's
        // below-grade roads (room 2 sits at −20) never drown.
        assert!(!water.is_deadly(world(14.0, -20.0, 4.0)));
        assert!(!water.is_deadly(world(40.0, -50.0, 4.0)));
    }

    /// London's Thames tunnel runs under the river's footprint with its
    /// floor ~9 m below the surface (rooms 670-672 under 347-349): the
    /// column stops `WATER_DEPTH` under the bound, so the tunnel is dry
    /// while a car sunk a few metres is still in the water.
    #[test]
    fn a_tunnel_under_the_footprint_is_not_water() {
        let psdl = psdl_with_water(-4.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        let bound = -3.8;
        assert!(water.is_deadly(world(4.0, bound - WATER_DEPTH + 0.1, 4.0)));
        assert!(!water.is_deadly(world(4.0, bound - WATER_DEPTH - 0.1, 4.0)));
        assert!(!water.is_deadly(world(4.0, -13.8, 4.0)));
    }

    #[test]
    fn an_elevated_room_binds_at_its_own_height() {
        // A mod room whose water plane sits above the authored level —
        // "deadly water at any height": the room's own top becomes the
        // bound, so the surface drowns while a deck above it is safe.
        let psdl = psdl_with_water(10.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        assert!(water.is_deadly(world(4.0, 10.0, 4.0)));
        assert!(water.is_deadly(world(4.0, 6.0, 4.0)));
        assert!(!water.is_deadly(world(4.0, 12.0, 4.0)));
    }

    /// Tables classifying `s_ocean` as drowning `deepwater` (drag 0.5)
    /// and `s_pond` as wadeable `water` (0.119) — the retail split.
    fn tables() -> SurfaceTables {
        let set = mm2_formats::materials::MaterialSet::parse(
            "mtl deepwater {\n    drag: 0.5\n}\nmtl water {\n    drag: 0.119\n}\n",
        )
        .unwrap();
        let map = mm2_formats::materials::MaterialMap::parse(
            "texture,physics\ns_ocean,deepwater\ns_pond,water\n",
        )
        .unwrap();
        SurfaceTables { set, map }
    }

    /// A `TextureRef` attribute selecting `psdl.textures[data0 - 1]`.
    fn texref(data0: u16) -> mm2_formats::psdl::RoomAttribute {
        mm2_formats::psdl::RoomAttribute {
            last: false,
            kind: AttributeType::TextureRef,
            subtype: 0,
            data: vec![data0],
        }
    }

    /// A `Fan` attribute standing in for "some other first attribute".
    fn other_attr() -> mm2_formats::psdl::RoomAttribute {
        mm2_formats::psdl::RoomAttribute {
            last: false,
            kind: AttributeType::Fan,
            subtype: 3,
            data: vec![0, 1, 2],
        }
    }

    #[test]
    fn sdl_mark_binds_through_the_first_texture_ref() {
        let mut psdl = psdl_with_water(-4.0, -20.0);
        // Room 1 leads with a TextureRef to `s_ocean` → deepwater.
        psdl.textures = vec![String::new(), "s_ocean".into()];
        psdl.rooms[0].attributes = vec![texref(2), other_attr()];
        let def = WaterDef::parse("-3.8").unwrap();
        let water = CityWater::build(&def, &psdl, Some(&tables()));
        assert_eq!(water.room_count(), 1);
        assert_eq!(water.sdl_rooms(), 1);
        // The SDL bound is the authored level itself: the plane at −4
        // drowns (−4 ≤ −3.8), the space above it does not.
        assert!(water.is_deadly(world(4.0, -4.0, 4.0)));
        assert!(!water.is_deadly(world(4.0, 0.0, 4.0)));
        // The plain room without a leading TextureRef is untouched.
        assert!(!water.is_deadly(world(14.0, -20.0, 4.0)));
    }

    #[test]
    fn sdl_mark_requires_a_drowning_class_surface() {
        let mut psdl = psdl_with_water(-4.0, -20.0);
        // `s_pond` → `water` (drag 0.119 < 0.3) stays wadeable.
        psdl.textures = vec![String::new(), "s_pond".into()];
        psdl.rooms[0].attributes = vec![texref(2)];
        let def = WaterDef::parse("-3.8").unwrap();
        let water = CityWater::build(&def, &psdl, Some(&tables()));
        assert_eq!(water.sdl_rooms(), 0);
        assert!(!water.is_deadly(world(4.0, -4.0, 4.0)));
    }

    #[test]
    fn sdl_mark_ignores_non_first_texture_refs() {
        let mut psdl = psdl_with_water(-4.0, -20.0);
        // The TextureRef is only the room's surface once it is the
        // first attribute — the exe reads word 0 of the attr stream.
        psdl.textures = vec![String::new(), "s_ocean".into()];
        psdl.rooms[0].attributes = vec![other_attr(), texref(2)];
        let def = WaterDef::parse("-3.8").unwrap();
        let water = CityWater::build(&def, &psdl, Some(&tables()));
        assert_eq!(water.sdl_rooms(), 0);
    }

    #[test]
    fn a_ref_room_keeps_its_elevated_bound_over_an_sdl_mark() {
        // Room 1 is both ref'd and SDL-marked — one entry, and the
        // ref's `max(level, room-top)` bound wins over the SDL level.
        let mut psdl = psdl_with_water(10.0, -20.0);
        psdl.textures = vec![String::new(), "s_ocean".into()];
        psdl.rooms[0].attributes = vec![texref(2)];
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl, Some(&tables()));
        assert_eq!(water.room_count(), 1);
        assert_eq!(water.sdl_rooms(), 0);
        assert!(water.is_deadly(world(4.0, 10.0, 4.0)));
        assert!(!water.is_deadly(world(4.0, 12.0, 4.0)));
    }

    #[test]
    fn no_surface_tables_skips_the_sdl_pass() {
        let mut psdl = psdl_with_water(-4.0, -20.0);
        psdl.textures = vec![String::new(), "s_ocean".into()];
        psdl.rooms[0].attributes = vec![texref(2)];
        let def = WaterDef::parse("-3.8").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        assert_eq!(water.sdl_rooms(), 0);
        assert!(!water.is_deadly(world(4.0, -4.0, 4.0)));
    }

    #[test]
    fn surface_is_the_bound_of_the_room_the_point_is_in() {
        let psdl = psdl_with_water(-4.0, -20.0);
        let def = WaterDef::parse("-3.8\n1").unwrap();
        let water = CityWater::build(&def, &psdl, None);
        assert_eq!(water.surface_at(world(4.0, -4.5, 4.0)), Some(-3.8));
        // Above the bound (a deck), outside the room, or sunk past the
        // column: no surface.
        assert_eq!(water.surface_at(world(4.0, 5.0, 4.0)), None);
        assert_eq!(water.surface_at(world(12.0, -4.5, 4.0)), None);
        assert_eq!(water.surface_at(world(4.0, -12.0, 4.0)), None);
    }

    #[test]
    fn buoyancy_is_nil_clear_of_the_water_and_balances_at_half_hull() {
        let g = 19.6;
        let (lin, ang) = water_accelerations(2.0, 0.0, g, Vec3::new(3.0, -1.0, 0.0), Vec3::ONE);
        assert_eq!((lin, ang), (Vec3::ZERO, Vec3::ZERO));
        // Centre of mass on the surface, at rest: lift exactly cancels gravity.
        let (lin, _) = water_accelerations(0.0, 0.0, g, Vec3::ZERO, Vec3::ZERO);
        assert!((lin.y - g).abs() < 1e-4, "{lin:?}");
        // Deeper pushes up harder, never more than the cap.
        let (deep, _) = water_accelerations(-5.0, 0.0, g, Vec3::ZERO, Vec3::ZERO);
        assert!((deep.y - BUOYANCY_RATIO * g).abs() < 1e-4);
    }

    /// A car dropped from 3 m above the surface at 12 m/s forward
    /// enters, is slowed, bobs and settles floating at the surface — it
    /// is not stopped dead by a plane and does not sink for ever.
    #[test]
    fn a_dropped_body_settles_floating_instead_of_sinking_or_bouncing() {
        let g = 19.6;
        let dt = 1.0 / 60.0;
        let (mut y, mut vy, mut vx) = (3.0_f32, 0.0_f32, 12.0_f32);
        let mut lowest = y;
        for _ in 0..(10.0 / dt) as usize {
            let (lin, _) = water_accelerations(y, 0.0, g, Vec3::new(vx, vy, 0.0), Vec3::ZERO);
            vy += (lin.y - g) * dt;
            vx += lin.x * dt;
            y += vy * dt;
            lowest = lowest.min(y);
        }
        assert!(lowest > -3.0, "sank {lowest} m");
        assert!(y.abs() < 0.05 && vy.abs() < 0.05, "y {y} vy {vy}");
        assert!(vx < 3.0, "horizontal speed is bled off: {vx}");
    }
}

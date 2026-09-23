//! F18-A.5 — authored room PVS culling.
//!
//! Retail `cityLevel` keeps a 512-byte `sm_PvsBuffer`: each frame the
//! source room's decompressed `.cpvs` list is cached there and
//! `DrawRooms` calls `IsRoomVisible(room)` — `code != 0` — per room
//! draw group (mm2hook `citylevel.cpp`). The source room comes from
//! `FindRoomId(position, previousRoom)`: the recovered signature takes
//! the view position plus the last room as a hint; the body is a thunk,
//! so its exact search order is unknown. `sm_EnablePVS` is the retail
//! global toggle — `--no-pvs` (`SessionConfig::dev.no_pvs`, read at
//! session load) is ours.
//!
//! The lookup here is a designed policy over the *verified* list
//! semantics (DSN, not an original-rules claim):
//!
//! - the source set is every room whose authored XZ perimeter contains
//!   the active camera position *or* the player vehicle's — retail
//!   `sdlPage16::PointInPerimeter` is the same 2-D containment test;
//!   `FindRoomId`'s `previousRoom` hint is only a search shortcut, so
//!   this rescan-per-frame produces the same answer without ordering
//!   dependence;
//! - where rooms overlap in plan (stacked decks) or the camera and the
//!   player sit in different rooms, their lists are *unioned* — an
//!   ambiguous pick can only leave extra rooms visible, never hide one
//!   the authored table shows. The player leg also guarantees the
//!   street under the car survives a camera that lagged across a room
//!   boundary;
//! - a position inside no room contributes nothing — with no sources
//!   at all, culling bypasses rather than hiding the city.
//!
//! Only the per-room render groups are tagged [`CityRoom`] — colliders
//! are physics (never culled) and props/decals are not room-scoped yet.

use avian3d::prelude::Position;
use bevy::prelude::*;
use mm2_formats::cpvs::Cpvs;
use mm2_formats::psdl::Psdl;
use mm2_game::PlayerVehicle;
use tracing::warn;

use crate::city::{authored_z, point_in_poly};

/// A per-room render entity's authored room id (`Psdl::rooms` index + 1 —
/// the same 1-based id the `.cpvs` lists and entity names use).
#[derive(Component, Debug, Clone, Copy)]
pub struct CityRoom(pub u32);

/// One room's containment shape: the authored perimeter polygon in
/// (x, authored-z) space.
struct RoomCell {
    poly: Vec<(f32, f32)>,
}

impl RoomCell {
    fn contains(&self, p: (f32, f32)) -> bool {
        self.poly.len() >= 3 && point_in_poly(p, &self.poly)
    }
}

/// The session's parsed PVS table plus the room-lookup state the frame
/// update maintains. Inserted by the session load when the city's
/// `<stem>.cpvs` resolves; absent on the dev world and on cities whose
/// install ships no table.
#[derive(Resource)]
pub struct CityPvs {
    cpvs: Cpvs,
    /// `cells[i]` is room id `i + 1`; rooms with degenerate perimeters
    /// carry an empty polygon and never contain a point.
    cells: Vec<RoomCell>,
    /// Source rooms for the current view, sorted ascending. Empty =
    /// unresolved → culling bypassed.
    sources: Vec<u32>,
    /// Decompressed visibility list per `sources` entry (corrupt lists
    /// are dropped — a dropped source under-culls, never over-culls).
    lists: Vec<Vec<u8>>,
    /// Whether culling applies (the `EnablePVS` counterpart).
    pub enabled: bool,
    /// Re-apply visibility to entities even when `sources` is unchanged
    /// (set when `enabled` flips).
    dirty: bool,
    /// Room entities hidden by the last apply.
    pub culled: usize,
    /// Room entities seen by the last apply.
    pub tagged: usize,
    /// Source-room resolutions performed (diagnostic: how often the
    /// full perimeter scan ran).
    pub lookups: u64,
    /// Resolutions that found no containing room.
    pub misses: u64,
    /// Decompressed lists dropped as corrupt.
    pub corrupt: usize,
}

impl CityPvs {
    /// Build the lookup over `psdl`'s rooms. A `.cpvs` whose list count
    /// doesn't cover the room set still works — lists past the table
    /// are missing sources, rooms past a list's length are implicitly
    /// invisible per the authored encoding.
    pub fn build(cpvs: Cpvs, psdl: &Psdl) -> Self {
        let cells = psdl
            .rooms
            .iter()
            .map(|room| RoomCell {
                poly: room
                    .perimeter
                    .iter()
                    .filter_map(|p| psdl.vertices.get(p.vertex as usize).map(|v| (v[0], v[2])))
                    .collect(),
            })
            .collect();
        CityPvs {
            cpvs,
            cells,
            sources: Vec::new(),
            lists: Vec::new(),
            enabled: true,
            dirty: true,
            culled: 0,
            tagged: 0,
            lookups: 0,
            misses: 0,
            corrupt: 0,
        }
    }

    /// Number of rooms the lookup covers (`Psdl::rooms` len).
    pub fn room_count(&self) -> usize {
        self.cells.len()
    }

    /// The lowest source room id — the record's `pvs=` field (0 when
    /// unresolved). With overlapping sources the union of their lists
    /// still applies; this names the primary cell only.
    pub fn source_room(&self) -> u32 {
        self.sources.first().copied().unwrap_or(0)
    }

    /// Flip the toggle; forces a re-apply on the next update.
    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled != on {
            self.enabled = on;
            self.dirty = true;
        }
    }

    /// Re-resolve the source room set for `positions` (world space) —
    /// the union of every room containing any of them. Returns `true`
    /// when the set changed and room visibility must be re-applied.
    /// The full perimeter scan is a few thousand point-in-polygon edge
    /// tests — cheap enough that the `previousRoom` hint's ordering
    /// complexity buys nothing.
    pub fn update_source(&mut self, positions: &[Vec3]) -> bool {
        self.lookups += 1;
        let mut sources: Vec<u32> = Vec::new();
        for pos in positions {
            let p = (pos.x, authored_z(pos.z));
            for i in 0..self.cells.len() as u32 {
                if self.cells[i as usize].contains(p) && !sources.contains(&(i + 1)) {
                    sources.push(i + 1);
                }
            }
        }
        sources.sort_unstable();
        if sources.is_empty() {
            self.misses += 1;
        }
        if sources == self.sources {
            return false;
        }
        self.sources = sources;
        let mut lists = Vec::with_capacity(self.sources.len());
        for &r in &self.sources {
            match self.cpvs.decompress(r as usize) {
                Ok(l) if !l.is_empty() => lists.push(l),
                Ok(_) => {}
                Err(e) => {
                    self.corrupt += 1;
                    warn!(room = r, error = %e, "cpvs list failed to decode; skipping source");
                }
            }
        }
        self.lists = lists;
        true
    }

    /// Whether `room` (1-based id) is shown this frame: union of the
    /// source lists' authored marks, `is_visible`'s nonzero-code rule.
    /// Bypassed (all visible) while disabled, unresolved, or when no
    /// source produced a list.
    pub fn is_room_visible(&self, room: u32) -> bool {
        if !self.enabled || self.lists.is_empty() {
            return true;
        }
        self.lists
            .iter()
            .any(|l| Cpvs::is_visible(l, room as usize))
    }
}

/// Per-frame PVS update: resolve the view's source room set and toggle
/// [`Visibility`] on every [`CityRoom`]-tagged render entity. The view
/// positions are the active 3-D camera's `Transform` — retail's
/// `Draw`/`FindRoomId` runs off the viewport, and the camera is a root
/// entity so `Transform` is the same-frame world pose `chase_follow`
/// just wrote — plus the player vehicle's physics `Position`, which
/// keeps the room under the car a source (and is the only position a
/// `--headless` run has). The query filters on [`Camera3d`] so a stray
/// active 2-D camera (the menu's `MenuCamera` survives a transition
/// frame) can never add a wrong source room — a wrong pick can only
/// over-show anyway, but the world camera is the honest source.
pub fn apply_city_pvs(
    pvs: Option<ResMut<CityPvs>>,
    views: Query<(&Camera, &Transform), With<Camera3d>>,
    player: Query<&Position, With<PlayerVehicle>>,
    mut rooms: Query<(&CityRoom, &mut Visibility)>,
    new_rooms: Query<(), Added<CityRoom>>,
) {
    let Some(mut pvs) = pvs else {
        return;
    };
    let mut positions: Vec<Vec3> = Vec::with_capacity(2);
    positions.extend(
        views
            .iter()
            .find(|(cam, _)| cam.is_active)
            .map(|(_, xf)| xf.translation),
    );
    // The player's room is a source too (see module docs): a chase
    // camera that lagged across a boundary can never cull the street
    // under the car, and a headless run (no camera) still resolves.
    positions.extend(player.iter().next().map(|p| p.0));
    if positions.is_empty() {
        return;
    }
    // Re-apply when the source set changed, the toggle flipped, or new
    // room entities arrived — the first update runs before the session
    // load's deferred spawns flush, so without the `Added` leg the
    // initial apply would latch on an empty entity set.
    let changed = pvs.update_source(&positions);
    if !changed && !pvs.dirty && new_rooms.is_empty() {
        return;
    }
    pvs.dirty = false;
    let (mut tagged, mut culled) = (0usize, 0usize);
    for (room, mut vis) in &mut rooms {
        tagged += 1;
        *vis = if pvs.is_room_visible(room.0) {
            Visibility::Inherited
        } else {
            culled += 1;
            Visibility::Hidden
        };
    }
    pvs.tagged = tagged;
    pvs.culled = culled;
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::psdl::{PerimeterPoint, PsdlRoom};

    /// A PSDL with `n` square rooms tiled along +X: room `i` covers
    /// `x ∈ [10i, 10i+8], z ∈ [0, 8]` in authored space (gaps between
    /// tiles are outside every room).
    fn grid_psdl(n: usize) -> Psdl {
        let mut vertices = Vec::new();
        let mut rooms = Vec::new();
        for i in 0..n {
            let x = 10.0 * i as f32;
            let base = vertices.len() as u16;
            vertices.extend_from_slice(&[
                [x, 0.0, 0.0],
                [x + 8.0, 0.0, 0.0],
                [x + 8.0, 0.0, 8.0],
                [x, 0.0, 8.0],
            ]);
            rooms.push(PsdlRoom {
                perimeter: (0..4)
                    .map(|k| PerimeterPoint {
                        vertex: base + k,
                        room: 0,
                    })
                    .collect(),
                attributes: Vec::new(),
                unparsed_attributes: Vec::new(),
            });
        }
        Psdl {
            target_size: 2,
            vertices,
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

    /// The Bevy-space position over authored `(x, z)` at height `y`
    /// (`MIRROR_Z` means authored z negates into world space).
    fn world(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, if crate::city::MIRROR_Z { -z } else { z })
    }

    /// Encode one cpvs list per room: list `i` marks room `i` visible.
    /// `build` mirrors the RLE in `cpvs.rs`'s own test helper.
    fn self_visible_cpvs(n: usize) -> Cpvs {
        let mut payload = Vec::new();
        let mut ends = Vec::new();
        for i in 0..=n {
            // list 0 stays empty (reserved); room r sees only itself.
            let list = if i == 0 {
                Vec::new()
            } else {
                let mut l = vec![0u8; i / 4 + 1];
                l[i / 4] = 3 << (2 * (i % 4));
                l
            };
            if !list.is_empty() {
                payload.push(0x80 + (list.len() as u8) - 1);
                payload.extend_from_slice(&list);
            }
            ends.push(payload.len() as u32);
        }
        let mut data = b"PVS0".to_vec();
        data.extend_from_slice(&((n + 2) as u32).to_le_bytes());
        for e in &ends {
            data.extend_from_slice(&e.to_le_bytes());
        }
        data.extend_from_slice(&payload);
        Cpvs::parse(&data).unwrap()
    }

    #[test]
    fn resolves_the_containing_room_and_culls_the_rest() {
        let psdl = grid_psdl(4);
        let mut pvs = CityPvs::build(self_visible_cpvs(4), &psdl);
        // Inside room 2's tile.
        assert!(pvs.update_source(&[world(14.0, 3.0, 4.0)]));
        assert_eq!(pvs.source_room(), 2);
        assert!(pvs.is_room_visible(2));
        assert!(!pvs.is_room_visible(1));
        assert!(!pvs.is_room_visible(3));
        // Staying inside keeps the set — no re-resolve.
        assert!(!pvs.update_source(&[world(15.0, 3.0, 4.0)]));
        // Moving to room 3's tile changes the set.
        assert!(pvs.update_source(&[world(24.0, 3.0, 4.0)]));
        assert_eq!(pvs.source_room(), 3);
        assert!(pvs.is_room_visible(3));
        assert!(!pvs.is_room_visible(2));
    }

    #[test]
    fn overlapping_rooms_union_their_lists() {
        // Two rooms on the same tile: room 2 overlaps room 1 exactly.
        let mut psdl = grid_psdl(2);
        let base = psdl.vertices.len() as u16;
        psdl.vertices.extend_from_slice(&[
            [0.0, 12.0, 0.0],
            [8.0, 12.0, 0.0],
            [8.0, 12.0, 8.0],
            [0.0, 12.0, 8.0],
        ]);
        psdl.rooms.push(PsdlRoom {
            perimeter: (0..4)
                .map(|k| PerimeterPoint {
                    vertex: base + k,
                    room: 0,
                })
                .collect(),
            attributes: Vec::new(),
            unparsed_attributes: Vec::new(),
        });
        // room1 sees {1}, room2 sees {2}, room3 sees {3}: a position in
        // the shared tile unions sources 1+3 → rooms 1 and 3 visible.
        let mut pvs = CityPvs::build(self_visible_cpvs(3), &psdl);
        assert!(pvs.update_source(&[world(4.0, 3.0, 4.0)]));
        assert_eq!(pvs.sources, vec![1, 3]);
        assert!(pvs.is_room_visible(1));
        assert!(pvs.is_room_visible(3));
        assert!(!pvs.is_room_visible(2));
    }

    #[test]
    fn unresolved_and_disabled_positions_show_everything() {
        let psdl = grid_psdl(2);
        let mut pvs = CityPvs::build(self_visible_cpvs(2), &psdl);
        // In the gap between tiles: no room contains the point — the
        // set stays empty (no change to report) but the miss counts.
        assert!(!pvs.update_source(&[world(9.0, 3.0, 4.0)]));
        assert_eq!(pvs.source_room(), 0);
        assert_eq!(pvs.misses, 1);
        assert!(pvs.is_room_visible(1));
        assert!(pvs.is_room_visible(2));
        // Resolved, then disabled → all visible again.
        assert!(pvs.update_source(&[world(4.0, 3.0, 4.0)]));
        assert!(!pvs.is_room_visible(2));
        pvs.set_enabled(false);
        assert!(pvs.is_room_visible(2));
        pvs.set_enabled(true);
        assert!(!pvs.is_room_visible(2));
    }

    /// The apply path end-to-end: tagged entities follow the source
    /// room's authored list, and a move re-applies on change only.
    #[test]
    fn system_hides_rooms_outside_the_source_list() {
        let mut app = App::new();
        let psdl = grid_psdl(3);
        app.insert_resource(CityPvs::build(self_visible_cpvs(3), &psdl));
        app.add_systems(Update, apply_city_pvs);
        let e: Vec<Entity> = (1..=3u32)
            .map(|r| {
                app.world_mut()
                    .spawn((CityRoom(r), Visibility::Inherited))
                    .id()
            })
            .collect();
        app.world_mut().spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            Transform::from_translation(world(4.0, 3.0, 4.0)),
        ));
        app.update();
        let vis = |app: &mut App, e: Entity| *app.world().get::<Visibility>(e).unwrap();
        assert_eq!(vis(&mut app, e[0]), Visibility::Inherited);
        assert_eq!(vis(&mut app, e[1]), Visibility::Hidden);
        assert_eq!(vis(&mut app, e[2]), Visibility::Hidden);
        let pvs = app.world().resource::<CityPvs>();
        assert_eq!((pvs.culled, pvs.tagged), (2, 3));
        // Move the camera into room 2's tile → the set re-applies.
        app.world_mut()
            .query::<&mut Transform>()
            .iter_mut(app.world_mut())
            .for_each(|mut t| t.translation = world(14.0, 3.0, 4.0));
        app.update();
        assert_eq!(vis(&mut app, e[1]), Visibility::Inherited);
        assert_eq!(vis(&mut app, e[0]), Visibility::Hidden);
    }

    /// A stray active 2-D camera (the menu's `MenuCamera` on a
    /// transition frame) is not a view source — only `Camera3d`
    /// contributes a room.
    #[test]
    fn system_ignores_2d_cameras() {
        let mut app = App::new();
        let psdl = grid_psdl(2);
        app.insert_resource(CityPvs::build(self_visible_cpvs(2), &psdl));
        app.add_systems(Update, apply_city_pvs);
        let room1 = app
            .world_mut()
            .spawn((CityRoom(1), Visibility::Inherited))
            .id();
        let room2 = app
            .world_mut()
            .spawn((CityRoom(2), Visibility::Inherited))
            .id();
        // Active Camera2d sitting inside room 1's tile; the player is
        // in room 2's. The 2-D camera must not union room 1 in.
        app.world_mut().spawn((
            Camera2d,
            Camera {
                is_active: true,
                ..default()
            },
            Transform::from_translation(world(4.0, 3.0, 4.0)),
        ));
        app.world_mut()
            .spawn((PlayerVehicle, Position(world(14.0, 3.0, 4.0))));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(room1).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            *app.world().get::<Visibility>(room2).unwrap(),
            Visibility::Inherited
        );
        assert_eq!(app.world().resource::<CityPvs>().source_room(), 2);
    }

    /// No camera at all falls back to the player vehicle's position —
    /// the headless record exercises the same code path.
    #[test]
    fn system_falls_back_to_the_player_vehicle() {
        let mut app = App::new();
        let psdl = grid_psdl(2);
        app.insert_resource(CityPvs::build(self_visible_cpvs(2), &psdl));
        app.add_systems(Update, apply_city_pvs);
        let room2 = app
            .world_mut()
            .spawn((CityRoom(2), Visibility::Inherited))
            .id();
        app.world_mut()
            .spawn((PlayerVehicle, Position(world(4.0, 3.0, 4.0))));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(room2).unwrap(),
            Visibility::Hidden
        );
    }
}

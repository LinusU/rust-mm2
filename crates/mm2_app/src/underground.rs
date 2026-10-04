//! Whether the listener is underground — the flag object sounds'
//! `audible area` reads.
//!
//! The original's per-frame camera update looks up the room the camera
//! stands in and copies that room's `Subterranean` PSDL flag (0x02)
//! into the audio manager (`0x4057c6`–`0x405809`, setting
//! `[mgr + 0x24]` through `0x50f9a0`/`0x50f9c0`). Tables with
//! `audible area` 1 play only while it is set (the Tube voices, the
//! Underground's rumble), 2 only while it is clear, 0 regardless.
//!
//! The room lookup is designed: `FindRoomId`'s body is unrecovered
//! (see `pvs.rs`), and London stacks tunnels under its streets, so a
//! plan-view containment test alone is ambiguous. Among the rooms whose
//! perimeter contains the listener in plan, the one taken is the
//! highest whose floor (lowest perimeter vertex) lies no more than
//! [`FLOOR_SLACK`] above the listener — the surface it stands over.

use bevy::audio::SpatialListener;
use bevy::prelude::*;
use mm2_formats::psdl::Psdl;

use crate::city::{authored_z, point_in_poly, room_poly};

/// The PSDL `Subterranean` room flag.
const SUBTERRANEAN: u8 = 0x02;

/// How far above the listener a room's floor may sit and still count
/// as the one it stands over — a camera dipping under the road surface
/// keeps its room. Designed.
pub const FLOOR_SLACK: f32 = 1.0;

/// One room's plan-view containment shape and floor height.
struct RoomFloor {
    poly: Vec<(f32, f32)>,
    floor: f32,
    subterranean: bool,
}

/// The city's rooms as the listener-room lookup sees them, plus the
/// current answer. Inserted by the session load on city worlds.
#[derive(Resource)]
pub struct ListenerRooms {
    rooms: Vec<RoomFloor>,
    /// Whether the listener stood in a subterranean room at the last
    /// update.
    pub underground: bool,
}

impl ListenerRooms {
    /// Build the lookup over `psdl`'s rooms; rooms with degenerate
    /// perimeters are dropped.
    pub fn build(psdl: &Psdl) -> Self {
        let rooms = psdl
            .rooms
            .iter()
            .enumerate()
            .filter_map(|(i, room)| {
                let poly = room_poly(psdl, room);
                let floor = room
                    .perimeter
                    .iter()
                    .filter_map(|p| psdl.vertices.get(p.vertex as usize).map(|v| v[1]))
                    .fold(f32::INFINITY, f32::min);
                (poly.len() >= 3 && floor.is_finite()).then(|| RoomFloor {
                    poly,
                    floor,
                    subterranean: psdl
                        .room_flags
                        .get(i)
                        .is_some_and(|f| f & SUBTERRANEAN != 0),
                })
            })
            .collect();
        Self {
            rooms,
            underground: false,
        }
    }

    /// Add a further city part's rooms.
    pub fn append(&mut self, other: Self) {
        self.rooms.extend(other.rooms);
    }

    /// Whether `at` (world space) stands in a subterranean room; a
    /// point inside no room is above ground.
    pub fn is_underground(&self, at: Vec3) -> bool {
        let p = (at.x, authored_z(at.z));
        let mut best: Option<&RoomFloor> = None;
        for room in self.rooms.iter().filter(|r| point_in_poly(p, &r.poly)) {
            let under = room.floor <= at.y + FLOOR_SLACK;
            best = match best {
                None => Some(room),
                Some(b) => {
                    let b_under = b.floor <= at.y + FLOOR_SLACK;
                    // Prefer a floor below the listener, then the
                    // highest such; with none below, the lowest.
                    let better = match (under, b_under) {
                        (true, false) => true,
                        (false, true) => false,
                        (true, true) => room.floor > b.floor,
                        (false, false) => room.floor < b.floor,
                    };
                    Some(if better { room } else { b })
                }
            };
        }
        best.is_some_and(|r| r.subterranean)
    }
}

/// Re-resolve [`ListenerRooms::underground`] from the spatial
/// listener's pose (the active camera, as in the original).
pub fn track_listener_room(
    rooms: Option<ResMut<ListenerRooms>>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
) {
    let Some(mut rooms) = rooms else {
        return;
    };
    let Some(ear) = listener.iter().next() else {
        return;
    };
    let underground = rooms.is_underground(ear.translation());
    if rooms.underground != underground {
        rooms.underground = underground;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(floor: f32, subterranean: bool) -> RoomFloor {
        RoomFloor {
            poly: vec![(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)],
            floor,
            subterranean,
        }
    }

    #[test]
    fn a_tunnel_under_a_street_is_told_apart_by_height() {
        let rooms = ListenerRooms {
            rooms: vec![square(5.0, false), square(-13.0, true)],
            underground: false,
        };
        assert!(
            !rooms.is_underground(Vec3::new(0.0, 8.0, 0.0)),
            "on the street"
        );
        assert!(
            rooms.is_underground(Vec3::new(0.0, -10.0, 0.0)),
            "in the tunnel"
        );
        assert!(
            rooms.is_underground(Vec3::new(0.0, -13.5, 0.0)),
            "dipped under its floor"
        );
        assert!(
            !rooms.is_underground(Vec3::new(50.0, -10.0, 0.0)),
            "outside every room"
        );
    }
}

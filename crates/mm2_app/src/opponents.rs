//! Race opponents (F15-A.2): the authored `[Opponent]` lineup spawned
//! as real simulated participants, each driving its own authored `.opp`
//! route through the same [`VehicleInput`] → physics → `advance_race`
//! path the player's controls feed.
//!
//! Two deliberate separations:
//!
//! - **Roster vs. runtime.** The lineup itself is `mm2_game`'s
//!   [`OpponentRoster`] contract, built by `mm2_content::opponents` from
//!   the event's `.aimap`/`.aimap_p` records. This module only consumes
//!   it — every authored entry keeps its slot even when its vehicle or
//!   route fails to resolve; a load failure is reported and the slot
//!   skipped, never repaired or re-numbered.
//! - **Route following vs. driving control.** [`route_target`] owns
//!   where the car is going (advance past reached points, close out or
//!   loop the polyline); [`crate::scripted::scripted_input`] owns how
//!   it gets there (proportional steering, corner braking, bounded
//!   reverse-and-turn stuck recovery — the same normalized-input
//!   control law the scripted evidence driver uses).
//!
//! Opponents are *not* clones of the player car: each roster entry
//! loads its own authored vehicle id through
//! [`mm2_content::load_opponent`], which prefers the retail
//! `<id>_opp.vehcarsim` opponent tuning when the VFS provides it, so
//! per-vehicle character (mass, power, size, grip) survives. They are
//! stamped `PlayerControl::Ai` — never counted as network clients —
//! and session-owned, so restart/teardown removes them and their
//! controller state with everything else.
//!
//! Slot assignment is provisional (UNK-17): authored `_strtpnts` slot
//! `index + 1` when the event ships a grid (slot 0 is the player by
//! convention), else the route's first point, else a designed stagger
//! behind the player. The `.opp` first row is a route anchor — verified
//! retail data puts it near, not on, the start line — so it is a spawn
//! fallback, not an asserted original grid slot. Facing always comes
//! from the route's first leg, since the `_strtpnts` `a` column's
//! convention is unverified (UNK-16: measured `cir1_strtpnts` headings
//! disagree with the waypoint `a` convention by ~180°).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    DamageSignals, ObjectIdentity, OpponentRoster, OpponentRoute, OpponentSpec, ParticipantState,
    Player, PlayerControl, RaceDefinition, RaceProgress, RaceState, Session, SessionEntity,
    relative_bearing,
};
use mm2_vehicle::{VehicleInput, VehicleState, vehicle_bundle};
use tracing::{info, warn};

use crate::car_visual;
use crate::scripted::{ScriptedBot, scripted_input};

/// XZ distance within which a route point counts as reached. `.opp`
/// points on retail routes sit 40-200 m apart; a generous radius keeps
/// the chase stable without needing to hit each anchor exactly.
const ROUTE_REACH: f32 = 14.0;

/// A route whose last point sits within this distance of its first is
/// treated as a loop — circuit routes on retail close back onto their
/// start (the lap structure lives in the race checkpoints; the route
/// just keeps supplying waypoints).
const ROUTE_LOOP: f32 = 40.0;

/// Spacing between designed-fallback slots behind the player when an
/// event has neither authored grid rows nor a resolvable route anchor.
const FALLBACK_SPACING: f32 = 6.0;

/// Spawn height above the authored slot position — the same settle
/// margin the player gets before its hull-clearance lift.
const SPAWN_LIFT: f32 = 0.25;

/// Per-opponent controller state — a component on the vehicle so
/// session teardown despawns it with everything else the session owns.
/// Carries the authored [`OpponentSpec`] verbatim: the vehicle id (for
/// diagnostics), the raw parameter tail (difficulty work, F15-B), and
/// the resolved route being chased.
#[derive(Component, Debug, Clone)]
pub struct OpponentDriver {
    /// The authored lineup entry this participant was spawned from.
    pub spec: OpponentSpec,
    /// Index of the route point currently being chased.
    pub next: usize,
    /// Bounded stuck-recovery state machine — the same three-point
    /// escape the scripted driver uses (F15-AC03's recovery leg).
    pub recovery: ScriptedBot,
}

/// Whether `pos` has reached route point `i`: inside [`ROUTE_REACH`],
/// or past its perpendicular plane along the direction of travel — a
/// car braked or shoved beyond an anchor chases forward instead of
/// U-turning back to it. The incoming leg supplies the direction for
/// interior points; point 0 has none, so its outgoing leg decides —
/// anything already ahead of the first anchor skips it.
fn point_reached(points: &[mm2_game::OpponentRoutePoint], i: usize, pos: Vec3) -> bool {
    let p = points[i].position;
    let dx = pos.x - p.x;
    let dz = pos.z - p.z;
    if dx * dx + dz * dz <= ROUTE_REACH * ROUTE_REACH {
        return true;
    }
    let dir = if i > 0 {
        points[i].position - points[i - 1].position
    } else {
        points.get(1).map(|n| n.position - p).unwrap_or(Vec3::ZERO)
    };
    let len2 = dir.x * dir.x + dir.z * dir.z;
    len2 > 0.0 && (pos.x - p.x) * dir.x + (pos.z - p.z) * dir.z > 0.0
}

/// Advance the chase index past reached points and return the point to
/// drive at. Returns the updated `next` and `Some(target)`; `None` when
/// the route is complete — an open polyline's end means the opponent
/// eases to a stop (its race progress is the checkpoints', not the
/// route's). A closed route (last point near the first — the retail
/// circuit pattern) rejoins at the first anchor so laps keep supplying
/// targets; the bounded retry keeps a degenerate all-in-reach route
/// from looping forever.
pub fn route_target(route: &OpponentRoute, mut next: usize, pos: Vec3) -> (usize, Option<Vec3>) {
    let points = &route.points;
    if points.is_empty() {
        return (next, None);
    }
    let closed = points.len() > 1 && {
        let last = points.last().unwrap().position;
        let first = points[0].position;
        (last.x - first.x).hypot(last.z - first.z) <= ROUTE_LOOP
    };
    for _ in 0..2 {
        while next < points.len() && point_reached(points, next, pos) {
            next += 1;
        }
        if next < points.len() {
            return (next, Some(points[next].position));
        }
        if !closed {
            return (next, None);
        }
        next = 0;
    }
    // Every anchor is in reach even after a wrap — a degenerate blob of
    // a route; hold on the first point rather than spin.
    (0, Some(points[0].position))
}

/// Where roster entry `index` spawns and which way it faces.
///
/// Position, provisional (UNK-17): authored `_strtpnts` slot `index + 1`
/// when the event ships a grid (slot 0 is the player), else the route's
/// first point, else a staggered line behind the player along its
/// reverse heading. Facing: toward the first route point at least a
/// car-length away — the route's first leg is verified course
/// direction, unlike the `a` columns whose conventions are open.
pub fn spawn_pose(
    definition: &RaceDefinition,
    index: usize,
    spec: &OpponentSpec,
    player_pos: Vec3,
    player_yaw: f32,
) -> (Vec3, f32) {
    let position = definition
        .start_slots
        .get(index + 1)
        .map(|s| s.position)
        .or_else(|| {
            spec.route
                .as_ref()
                .and_then(|r| r.points.first().map(|p| p.position))
        })
        .unwrap_or_else(|| {
            let back = Vec3::new(-player_yaw.sin(), 0.0, -player_yaw.cos());
            player_pos - back * (FALLBACK_SPACING * (index + 1) as f32)
        });
    let yaw = spec
        .route
        .as_ref()
        .and_then(|r| {
            r.points.iter().map(|p| p.position).find(|p| {
                let d = *p - position;
                d.x * d.x + d.z * d.z > 4.0
            })
        })
        .map(|target| {
            let f = target - position;
            (-f.x).atan2(-f.z)
        })
        .unwrap_or(player_yaw);
    (position, yaw)
}

/// Spawn every roster entry that loads as a real participant: its own
/// authored vehicle (opponent tuning preferred), a session-minted
/// `ObjectId`/`PlayerId`, `PlayerControl::Ai`, the session's authority
/// role, damage signals, `RaceProgress` on the shared definition and an
/// [`OpponentDriver`] — all stamped `owner` so teardown removes them
/// together. A vehicle that fails to load is warned and its authored
/// slot skipped; the roster's own issue list is untouched. Returns the
/// number spawned.
#[allow(clippy::too_many_arguments)]
pub fn spawn_opponents(
    commands: &mut Commands,
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    roster: &OpponentRoster,
    definition: &RaceDefinition,
    owner: SessionEntity,
    session: &mut Session,
    player_pos: Vec3,
    player_yaw: f32,
) -> usize {
    for issue in &roster.issues {
        warn!(issue = %issue, "opponent roster issue");
    }
    let role = session.authority_role();
    let mut spawned = 0;
    for (i, spec) in roster.entries.iter().enumerate() {
        let def = match mm2_content::load_opponent(vfs, &spec.vehicle, 0) {
            Ok(def) => def,
            Err(e) => {
                warn!(
                    vehicle = %spec.vehicle,
                    error = %e,
                    "opponent vehicle failed to load — authored slot skipped"
                );
                continue;
            }
        };
        let (mut pos, yaw) = spawn_pose(definition, i, spec, player_pos, player_yaw);
        // The same hull clearance the player spawn applies: the
        // collider's lowest point off the ground plus a settle margin.
        let hull_min_y = def
            .config
            .collider_points
            .as_ref()
            .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
            .unwrap_or(-def.config.chassis_size[1] * 0.5);
        pos.y += (SPAWN_LIFT - hull_min_y).max(0.35);

        let object = session.mint_object_id();
        let player_id = session.mint_player_id();
        let vehicle = commands
            .spawn((
                owner,
                ObjectIdentity(object),
                Player {
                    id: player_id,
                    control: PlayerControl::Ai,
                },
                role,
                DamageSignals::default(),
                RaceProgress::new(definition),
                OpponentDriver {
                    spec: spec.clone(),
                    next: 0,
                    recovery: ScriptedBot::default(),
                },
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                TransformInterpolation,
                // Parents of renderable children need the visibility chain.
                Visibility::Visible,
            ))
            .id();
        let missing = car_visual::spawn_vehicle_model(
            commands, vfs, &def.model, 0, meshes, images, materials, vehicle,
        );
        if !missing.is_empty() {
            warn!(car = %def.id, "opponent missing textures: {}", missing.join(", "));
        }
        if def.trailer.is_some() {
            // Trailer rigs are not wired for opponents yet — the roster
            // vehicle still spawns and races on its own (no retail
            // roster wires a hauler).
            info!(car = %def.id, "opponent vehicle has a trailer; spawned without it");
        }
        spawned += 1;
    }
    if spawned > 0 {
        info!(opponents = spawned, "opponent roster spawned");
    }
    spawned
}

/// Write [`VehicleInput`] for every AI opponent from its route target,
/// each frame. Scheduled in `Update` like the other input writers;
/// `vehicle_input` only touches `PlayerVehicle`, so there is no write
/// contention — this is the same normalized-input path, a different
/// driver behind it.
///
/// Honors the player input's gates: the session must be `Playing` and
/// the countdown's `input_locked` holds every car still. A participant
/// whose `RaceProgress` resolved — finished or timed out — gets a
/// zeroed input and coasts. A roster entry with no route (a dead `.opp`
/// reference the roster deliberately kept) holds still: the slot exists
/// because the authored lineup fields it, not because it can drive.
pub fn opponent_drive(
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut cars: Query<(
        &mut VehicleInput,
        &Position,
        &Rotation,
        &VehicleState,
        &RaceProgress,
        &mut OpponentDriver,
    )>,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    for (mut input, pos, rot, vstate, progress, mut driver) in &mut cars {
        let racing = matches!(
            progress.state,
            ParticipantState::AwaitingStart | ParticipantState::Racing
        );
        let target = if !session.is_playing() || locked || !racing {
            None
        } else if let Some(route) = &driver.spec.route {
            let (next, target) = route_target(route, driver.next, pos.0);
            driver.next = next;
            target
        } else {
            None
        };
        let Some(target) = target else {
            *input = VehicleInput::default();
            continue;
        };
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let bearing = relative_bearing(yaw, pos.0, target);
        *input = scripted_input(
            &mut driver.recovery,
            bearing,
            vstate.forward_speed,
            vstate.grounded,
        );
    }
}

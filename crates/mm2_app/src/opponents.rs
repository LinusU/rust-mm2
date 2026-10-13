//! Race opponents (F15): the authored `[Opponent]` lineup spawned as
//! real simulated participants, each driven along its own authored
//! `.opp` route by the neural driver ([`crate::neural`]) through the
//! same [`VehicleInput`] → physics → `advance_race` path the player's
//! controls feed.
//!
//! - **Roster vs. runtime.** The lineup itself is `mm2_game`'s
//!   [`OpponentRoster`] contract, built by `mm2_content::opponents` from
//!   the event's `.aimap`/`.aimap_p` records. This module only consumes
//!   it — every authored entry keeps its slot even when its vehicle or
//!   route fails to resolve; a load failure is reported and the slot
//!   skipped, never repaired or re-numbered.
//! - **The route.** An `.opp` record is read the way the original reads
//!   it (`docs/research/opponent-ai.md`): row 0 the start pose, the
//!   rows after it the course's intersections in order, the last row
//!   the finish position. [`driving_route`] turns that into the line
//!   actually driven — each leg laid along the road its two
//!   intersections share, a circuit reduced to its lap — and the
//!   driver's [`Guide`] is that line set on the road surface and
//!   smoothed.
//! - **The driving.** [`opponent_drive`] runs once per fixed step. It
//!   follows the car along its guide, feeds the route-bound progress
//!   ([`RouteGateLine`]) and hands the pedals to the policy for the
//!   session's difficulty ([`DrivingPolicies`]), capped at the row's
//!   authored `maxThrottle`. The policy sees the line ahead, the world
//!   around the car and the vehicles near it — the player, the other
//!   opponents, police and ambient traffic alike. A car that stalls
//!   against something is backed out ([`crate::neural::unstick`]).
//! - **Authored tuning.** Of the row's ten-value tail
//!   (`aiVehiclePhysics::RegisterRoute`, RACE-14) the driver consumes
//!   `maxThrottle` — the authored difficulty dial, 0.57–1.00 on retail.
//!   The corner-speed, braking-threshold, look-ahead and avoid-flag
//!   columns parametrised the original's own controller and have no
//!   counterpart in a learned one; they stay decoded on
//!   [`OpponentSpec`] and unconsumed.
//! - **Bounded re-anchor (F15-B.3).** A car nothing frees — penned,
//!   beached on its hull, knocked somewhere its route cannot be
//!   regained — gets a disclosed last resort after [`REANCHOR_FRAMES`]
//!   without [`REANCHOR_DIST`] of displacement: a `ResetVehicle`
//!   teleport back to where its guide last had it, walked back out of
//!   un-cleared checkpoint triggers and kept [`REANCHOR_CLEAR`] off
//!   every other car. `OpponentDriver::reanchors` counts each one and
//!   the smoke record reports the field total; designed policy
//!   (DSN-14), not an original rule — the original backs up and, when
//!   that fails, stays stuck.
//!
//! Opponents are *not* clones of the player car: each roster entry
//! loads its own authored vehicle id through
//! [`mm2_content::load_opponent`] — the same tuning the player gets for
//! that car, as in the original (RACE-13). They are stamped
//! `PlayerControl::Ai` — never counted as network clients — and
//! session-owned, so restart/teardown removes them and their controller
//! state with everything else.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    CheckpointRule, DamageSignals, DamageSpec, NavGraph, ObjectId, ObjectIdentity, OpponentRoster,
    OpponentRoute, OpponentSpec, ParticipantState, Player, PlayerControl, RaceDefinition,
    RaceProgress, RaceState, RecoveryPolicy, RouteGateLine, RouteOptions, Session, SessionEntity,
    SmokePolicy, SparkPolicy, StuckSpec, VehicleAudio, VehicleDamage, VehicleRecovery,
    VehicleSmoke, VehicleSparks, VehicleStuck,
};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState, vehicle_bundle};
use tracing::{info, warn};

use std::sync::Arc;

use crate::car_visual;
use crate::neural::drive::{DrivingPolicies, NeuralState};
use crate::neural::guide::Guide;
use crate::neural::observe::{Motion, Neighbour};
use crate::neural::policy::Scratch;
use crate::racing_line::RouteCursor;
use crate::recovery::seat_on_static_ground;

/// XZ distance within which a route point counts as reached. `.opp`
/// points on retail routes sit 40-200 m apart; a generous radius keeps
/// the chase stable without needing to hit each anchor exactly.
const ROUTE_REACH: f32 = 14.0;

/// A route whose last point sits within this distance of its first is
/// treated as a loop — circuit routes on retail close back onto their
/// start (the lap structure lives in the race checkpoints; the route
/// just keeps supplying waypoints).
const ROUTE_LOOP: f32 = 40.0;

/// Metres short of an open route's end at which a neural driver has
/// driven it out.
const ROUTE_END: f32 = 3.0;
/// ... and the distance (m, XZ) from the route's last point it must
/// also be within.
const ROUTE_END_RADIUS: f32 = 12.0;

/// Spacing between designed-fallback slots behind the player when an
/// event has neither authored grid rows nor a resolvable route anchor.
const FALLBACK_SPACING: f32 = 6.0;

/// Spawn height above the authored slot position — the same settle
/// margin the player gets before its hull-clearance lift. Shared with
/// the scripted bot's re-anchor (`crate::scripted`).
pub(crate) const SPAWN_LIFT: f32 = 0.25;

/// Bubble radius (m) around the episode anchor the stuck clock
/// tolerates: a car that never leaves it is not progressing even while
/// it rolls — the same displacement-over-speed test the pass stall
/// uses. Generous enough that a crawling queue keeps resetting.
/// Shared with the scripted bot's re-anchor (`crate::scripted`).
pub(crate) const REANCHOR_DIST: f32 = 8.0;
/// Frames (60 Hz updates) inside the bubble before the bounded last
/// resort fires — about 15 s, so the reverse-and-turn escapes (~3.5 s
/// a cycle) and the pass stall/ban cycle get their turns first.
pub const REANCHOR_FRAMES: u32 = 900;
/// Step back (m) along the route polyline the re-anchor takes —
/// re-approach the point it failed at instead of landing inside it —
/// and the stride each further step takes while the candidate still
/// sits inside an un-cleared trigger.
const REANCHOR_BACK: f32 = 4.0;
/// Total backward walk (m) the trigger-avoidance may consume — past
/// this the landing point stands wherever the walk reached and the
/// normal crossing rules apply (disclosed, not silently unbounded).
const REANCHOR_WALK: f32 = 60.0;
/// Hard bound on steps (leg traversals plus blocked retries) one
/// re-anchor walk may take. A legitimate walk spends one step per leg
/// traversed and one per rejected landing — tens, not thousands — but
/// a closed route of zero-length legs (every authored anchor on one
/// XZ point) never advances `walked`, and a non-finite anchor makes
/// every comparison false: both shapes would otherwise cycle the legs
/// forever (authored-numbers audit finding 1). The cap lands the car
/// on the degenerate pose the walk reached — bounded, and the roster
/// reports such a route at distillation instead of wiring it.
const REANCHOR_MAX_STEPS: usize = 16384;
/// XZ clearance (m) a re-anchor landing keeps from every participant
/// and from poses already claimed by a re-anchor this frame (F15-B.10,
/// the spec's competing-recovery-positions edge): two cars penned in
/// one pocket otherwise project to the same leg point and teleport
/// onto each other — a disclosed recovery that lands interpenetrating
/// solves out as a launch, not a recovery. Sized past a long vehicle's
/// half-length; the walk-back consumes the same [`REANCHOR_WALK`]
/// budget, so a route jammed solid still lands bounded (disclosed).
pub const REANCHOR_CLEAR: f32 = 8.0;
/// Vertical band (m) the clearance applies within — a car on the
/// street below an elevated leg does not block a landing on it.
const REANCHOR_CLEAR_Y: f32 = 4.0;

/// The occupancy leg of a re-anchor `blocked` test (F15-B.10): `p` is
/// occupied when it sits within [`REANCHOR_CLEAR`] in XZ and inside
/// the `REANCHOR_CLEAR_Y` band of any position in `occupied` — the
/// frame-start participant snapshot (the re-anchoring car's own spot
/// included, so a car beached on the line does not teleport back onto
/// itself) plus the poses earlier same-frame re-anchors claimed. Both
/// re-anchor call sites — `opponent_drive` and the scripted player's
/// — run this same predicate so the two bounded recoveries hold one
/// clearance contract; the check is positional and applies whatever
/// the authored avoid flags say.
pub fn reanchor_occupied(p: Vec3, occupied: impl IntoIterator<Item = Vec3>) -> bool {
    occupied.into_iter().any(|c| {
        (p.y - c.y).abs() < REANCHOR_CLEAR_Y && {
            let dx = p.x - c.x;
            let dz = p.z - c.z;
            dx * dx + dz * dz < REANCHOR_CLEAR * REANCHOR_CLEAR
        }
    })
}
/// Pure-pursuit aim distance (m) at a standstill, the extra per m/s of
/// speed, and the bounds — about 0.7 s of travel ahead, never so close
/// that steering hunts nor so far that it cuts the inside of a bend.
const AIM_BASE: f32 = 8.0;
const AIM_PER_SPEED: f32 = 0.7;
const AIM_RANGE: (f32, f32) = (12.0, 40.0);

/// How far down the route line the driver aims at `speed`.
pub fn aim_distance(speed: f32) -> f32 {
    (AIM_BASE + AIM_PER_SPEED * speed.max(0.0)).clamp(AIM_RANGE.0, AIM_RANGE.1)
}

/// Distance (m, XZ) from its place on its guide inside which a car
/// still earns route arc for the [`RouteGateLine`] high-water — an
/// overtake or a back-up stays inside it, a car punted onto a parallel
/// street does not (designed, DSN-45). Route credit cannot accumulate
/// off the line.
const ROUTE_ARC_LATERAL: f32 = crate::neural::guide::ON_LINE;

/// A Crash Course lesson's lead car (F21-B.19) — the `[Opponent]` slot a
/// follow or stop leg watches. Spawned by [`spawn_lead_cars`] on the
/// same AI driver as the race opponents but, unlike them, not a
/// participant: no `RaceProgress`, so the leg gates and the sequencer
/// never see it. The family rules [`crate::lesson::drive_lesson`] feeds
/// ([`mm2_game::LegRule`]) find "the lead" by this marker alone — retail
/// lessons field at most one, and the marker says which cars the lesson
/// rules may read.
#[derive(Component, Debug, Clone, Copy)]
pub struct LeadCar;

/// Per-opponent controller state — a component on the vehicle so
/// session teardown despawns it with everything else the session owns.
/// Carries the authored [`OpponentSpec`] verbatim (vehicle id, raw
/// parameter tail, authored route record) plus [`OpponentDriver::route`]
/// — the driving line actually chased.
#[derive(Component, Debug, Clone)]
pub struct OpponentDriver {
    /// This participant's index in the authored roster — the slot the
    /// lineup entry occupied, kept so evidence (the smoke record's
    /// `opps=` field) names the authored slot even when a neighbour's
    /// vehicle failed to load and its slot was skipped.
    pub index: usize,
    /// The authored lineup entry this participant was spawned from.
    pub spec: OpponentSpec,
    /// The route the driver follows — [`driving_route`]'s reading of
    /// the authored `.opp` record. `None` when the entry resolved no
    /// route at all.
    pub route: Option<OpponentRoute>,
    /// The most throttle the driver's pedal is given: the row's
    /// authored `maxThrottle` (full throttle when absent).
    pub throttle_cap: f32,
    /// Index of the route point the car is heading for, from its arc
    /// position on its guide.
    pub next: usize,
    /// Where the current stuck window started — progress is measured
    /// as displacement from here, not momentary speed: a penned or
    /// high-centred car can roll without ever leaving the bubble.
    pub stuck_pos: Vec3,
    /// Frames spent inside [`REANCHOR_DIST`] of `stuck_pos` while the
    /// car should be driving — at [`REANCHOR_FRAMES`] the bounded
    /// re-anchor fires (F15-B.3, AC03).
    pub stuck_frames: u32,
    /// The largest `stuck_frames` count ever reached — the longest
    /// continuous spell inside one displacement bubble (F15 req 6's
    /// stuck duration). A progressing car's window re-seats every
    /// [`REANCHOR_DIST`] of travel, so its peak stays a handful of
    /// frames; a penned one runs to [`REANCHOR_FRAMES`]. Unlike
    /// `stuck_frames` it survives the window reset and a re-anchor.
    pub stuck_peak: u32,
    /// Re-anchors this participant has taken this session — the
    /// observable record of the disclosed teleport assist (surfaced
    /// in the smoke record as `opp_rec=`).
    pub reanchors: u32,
    /// How far and how long this driver has actually driven — the
    /// denominator that turns back-up, re-anchor and impact counts
    /// into rates the smoke record's `opp_drv=` field can compare.
    pub stats: DriveStats,
    /// The neural driver's state on this car's route — `None` when the
    /// entry resolved no drivable route, and the car holds still.
    pub neural: Option<NeuralState>,
}

/// Distance and time an opponent spent driving its route — only frames
/// with a live target count, so the countdown, a finish and a dead
/// route add nothing. Re-anchor teleports are excluded from the
/// distance: the odometer measures driving, not the assist.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DriveStats {
    /// XZ distance driven (m).
    pub distance: f32,
    /// Seconds spent with a route target.
    pub seconds: f32,
    /// Where the previous driving frame left the car; `None` after a
    /// re-anchor or any frame without a target.
    pub last_pos: Option<Vec3>,
}

/// Per-frame XZ displacement (m) above which a step counts as a jump
/// rather than driving — well past any car's top speed at 60 Hz,
/// well below a re-anchor or recovery teleport.
const ODOMETER_JUMP: f32 = 5.0;

impl DriveStats {
    /// Accumulate one driving frame of `dt` seconds ending at `pos`.
    pub fn record(&mut self, pos: Vec3, dt: f32) {
        if let Some(last) = self.last_pos {
            let step = (pos.x - last.x).hypot(pos.z - last.z);
            if step < ODOMETER_JUMP {
                self.distance += step;
            }
        }
        self.seconds += dt;
        self.last_pos = Some(pos);
    }
}

/// Whether `pos` has reached route point `i`: inside [`ROUTE_REACH`],
/// or past its perpendicular plane along the direction of travel — a
/// car braked or shoved beyond an anchor chases forward instead of
/// U-turning back to it. The incoming leg supplies the direction for
/// interior points; point 0 has none, so its outgoing leg decides —
/// anything already ahead of the first anchor skips it.
pub(crate) fn point_reached(points: &[mm2_game::OpponentRoutePoint], i: usize, pos: Vec3) -> bool {
    point_reached_with_radius(points, i, pos, ROUTE_REACH)
}

/// Shared plane test with a bounded reach distance for densely sampled routes.
pub(crate) fn point_reached_with_radius(
    points: &[mm2_game::OpponentRoutePoint],
    i: usize,
    pos: Vec3,
    reach: f32,
) -> bool {
    let reach = reach.clamp(0.0, ROUTE_REACH);
    let p = points[i].position;
    let dx = pos.x - p.x;
    let dz = pos.z - p.z;
    if dx * dx + dz * dz <= reach * reach {
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

/// Local nonzero XZ sampling distance, independent of authored speed hints.
fn point_spacing(points: &[mm2_game::OpponentRoutePoint], next: usize) -> f32 {
    let n = points.len();
    let point = points[next].position;
    let mut spacing = f32::INFINITY;
    for neighbor in [next.checked_sub(1), (next + 1 < n).then_some(next + 1)]
        .into_iter()
        .flatten()
    {
        let delta = points[neighbor].position - point;
        let length = delta.x.hypot(delta.z);
        if length > 1e-3 {
            spacing = spacing.min(length);
        }
    }
    spacing
}

/// Dense authored lines need spatial progress rather than the sparse retail
/// anchor radius: accepting several unseen samples discards a pending bend.
/// Widely spaced routes retain the exact legacy 14 m reach and plane policy.
fn native_point_reached(route: &OpponentRoute, next: usize, pos: Vec3) -> bool {
    if point_spacing(&route.points, next) < ROUTE_REACH {
        spacing_point_reached(route, next, pos)
    } else {
        point_reached(&route.points, next, pos)
    }
}

/// Follow actual adjacent-leg progress on densely sampled routes. A small
/// spacing-tied reach cannot skip an unseen corner; the outgoing projection
/// still admits the normal rounded line inside a bend after the car turns.
pub(crate) fn spacing_point_reached(route: &OpponentRoute, next: usize, pos: Vec3) -> bool {
    let n = route.points.len();
    let spacing = point_spacing(&route.points, next);
    if point_reached_with_radius(&route.points, next, pos, (spacing * 0.25).max(0.5)) {
        return true;
    }
    let Some(incoming) = RouteCursor::locate(route, next, pos) else {
        return false;
    };
    let outgoing_next = if next + 1 < n {
        next + 1
    } else if route_is_closed(route) {
        0
    } else {
        return false;
    };
    let Some(outgoing) = RouteCursor::locate(route, outgoing_next, pos) else {
        return false;
    };
    outgoing.along > 0.0
        && outgoing.point_ahead(route, 0.0).distance_squared(pos)
            < incoming.point_ahead(route, 0.0).distance_squared(pos)
}

/// Whether a route's last point sits within [`ROUTE_LOOP`] of its
/// first — the retail circuit pattern, where the lap structure lives
/// in the race checkpoints and the route just keeps supplying
/// waypoints by rejoining at the first anchor.
pub fn route_is_closed(route: &OpponentRoute) -> bool {
    let points = &route.points;
    points.len() > 1 && {
        let last = points.last().unwrap().position;
        let first = points[0].position;
        (last.x - first.x).hypot(last.z - first.z) <= ROUTE_LOOP
    }
}

/// Distance (m, horizontal) within which a lead car on its last route
/// node counts as having reached the goal point — the original's
/// `0x53d6c0` constant `900` (squared), `CC-9`.
pub const LEAD_GOAL_RANGE_M: f32 = 30.0;

/// Distance (m, horizontal) inside which the original additionally
/// requires the lead to have crossed the goal plane (`0x53d6c0`,
/// constant `400` squared): within 30 m it has arrived, within 20 m it
/// must be *past* the plane. `CC-9`.
pub const LEAD_GOAL_PLANE_M: f32 = 20.0;

/// Whether a lesson's lead car has finished its authored route — the
/// original's `aiVehicleOpponent::finished` (`0x53d6c0`) that the follow
/// and stop handlers read (`CC-9`).
///
/// `next` is the [`OpponentDriver`]'s chase index and `pos` its
/// position. The car must be on (or past) its last route node, within
/// [`LEAD_GOAL_RANGE_M`] of the goal point — the route's final anchor —
/// and, once inside [`LEAD_GOAL_PLANE_M`], past the goal plane, a side
/// test along the goal direction. The goal direction was not traced in
/// the executable (`docs/research/crashcourse.md`, inferred): the final
/// leg's heading is used, and a final leg with no horizontal extent
/// (`length² < 1e-6`) has no orientable plane, so proximity alone
/// counts there.
pub fn lead_route_finished(route: &OpponentRoute, next: usize, pos: Vec3) -> bool {
    let points = &route.points;
    let Some(last) = points.last() else {
        return false;
    };
    // "On its last route node": chasing the final anchor or already
    // past it (an exhausted open route advances `next` to `len`).
    if next + 1 < points.len() {
        return false;
    }
    let goal = last.position;
    let (dx, dz) = (pos.x - goal.x, pos.z - goal.z);
    let dist_sq = dx * dx + dz * dz;
    if dist_sq >= LEAD_GOAL_RANGE_M * LEAD_GOAL_RANGE_M {
        return false;
    }
    if dist_sq < LEAD_GOAL_PLANE_M * LEAD_GOAL_PLANE_M
        && let Some(prev) = points.len().checked_sub(2).and_then(|i| points.get(i))
    {
        let dir = Vec3::new(goal.x - prev.position.x, 0.0, goal.z - prev.position.z);
        if dir.length_squared() > 1e-6 {
            let dir = dir.normalize();
            if dx * dir.x + dz * dir.z < 0.0 {
                return false;
            }
        }
    }
    true
}

/// Where roster entry `index` spawns and which way it faces.
///
/// An entry with a route stages on it — the route's row 0, as the
/// original places every opponent (`aiVehicleOpponent::Init`
/// `0x53d26f`; the shipped game reads no start grid at all, UNK-17).
/// An entry with no route takes authored `_strtpnts` slot `index + 1`
/// when the event carries a grid (slot 0 is the player), else a
/// staggered line behind the player along its reverse heading.
///
/// Facing follows the position source: the `.opp` row-0 staging
/// heading (the mislabelled `brake` column — measured as vehicle yaw
/// on 592/612 retail files), or the first-leg direction for a route
/// that authors none; a grid slot's own `yaw_deg`; and for a slot with
/// no authored heading (`cir6_strtpnts`' all-zero `a` column) or no
/// slot at all, the player's yaw.
pub fn spawn_pose(
    definition: &RaceDefinition,
    index: usize,
    spec: &OpponentSpec,
    player_pos: Vec3,
    player_yaw: f32,
) -> (Vec3, f32) {
    // An opponent with a route stages on it: the original places every
    // opponent at its own `.opp` row 0 and reads no start grid at all
    // (`aiVehicleOpponent::Init` `0x53d26f`, UNK-17). A grid slot only
    // seats an entry that has no route to stage on.
    let staged = spec.route.as_ref().is_some_and(|r| !r.points.is_empty());
    pose_at(
        definition.start_slots.get(index + 1).filter(|_| !staged),
        index,
        spec,
        player_pos,
        player_yaw,
    )
}

/// Where a lesson's lead car stands: its route's own staging pose
/// (row 0 and its heading), never a grid slot — a lesson leg's start
/// grid seats the player alone, and the lead car belongs to its route.
/// The designed stagger behind the player applies only to a dead route.
pub fn lead_car_pose(
    index: usize,
    spec: &OpponentSpec,
    player_pos: Vec3,
    player_yaw: f32,
) -> (Vec3, f32) {
    pose_at(None, index, spec, player_pos, player_yaw)
}

fn pose_at(
    slot: Option<&mm2_game::RaceStart>,
    index: usize,
    spec: &OpponentSpec,
    player_pos: Vec3,
    player_yaw: f32,
) -> (Vec3, f32) {
    let position = slot.map(|s| s.position).unwrap_or_else(|| {
        spec.route
            .as_ref()
            .and_then(|r| r.points.first().map(|p| p.position))
            .unwrap_or_else(|| {
                let back = Vec3::new(-player_yaw.sin(), 0.0, -player_yaw.cos());
                player_pos - back * (FALLBACK_SPACING * (index + 1) as f32)
            })
    });
    let yaw = slot
        .and_then(|s| s.yaw_deg)
        .map(f32::to_radians)
        .or_else(|| {
            spec.route
                .as_ref()
                .and_then(|r| r.start_heading_deg().map(f32::to_radians))
        })
        .or_else(|| {
            spec.route.as_ref().and_then(|r| {
                r.points
                    .iter()
                    .map(|p| p.position)
                    .find(|p| {
                        let d = *p - position;
                        d.x * d.x + d.z * d.z > 4.0
                    })
                    .map(|target| {
                        let f = target - position;
                        (-f.x).atan2(-f.z)
                    })
            })
        })
        .unwrap_or(player_yaw);
    (position, yaw)
}

/// The chase index a fresh spawn should start from: the first route
/// point that is both ahead of the spawn facing and not already
/// reached. An `.opp` line's early anchors can sit *behind* its
/// authored staging heading — the staged start joins the driving line
/// mid-leg (measured on retail `circuit1-a-0`: heading −X while row 1
/// sits +X behind it) — and chasing one U-turns the car off its
/// authored facing. Scanning is bounded to the authored order; a point
/// left behind is picked up on the next pass through a closed route.
pub fn initial_route_index(route: &OpponentRoute, pos: Vec3, yaw: f32) -> usize {
    let fwd = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let mut next = 0;
    while next < route.points.len() {
        let rel = route.points[next].position - pos;
        let ahead = rel.x * fwd.x + rel.z * fwd.z > 0.0;
        if ahead && !native_point_reached(route, next, pos) {
            break;
        }
        next += 1;
    }
    next
}

/// The pose a bounded re-anchor drops the car at (F15-B.3, AC03's
/// bounded recovery): its own position projected onto the route leg it
/// was chasing, then walked *backward* along the authored polyline —
/// [`REANCHOR_BACK`] for clearance, and further while the candidate
/// still sits inside a trigger the participant has not cleared
/// (`blocked`), bounded by [`REANCHOR_WALK`] and the route start.
/// Landing outside the un-cleared gates is what keeps the disclosed
/// teleport from banking a checkpoint the car never drove through:
/// the jump itself breaks the swept segment via `Teleported`, and the
/// walk-back keeps the anchored landing point out of pending triggers
/// (AC04 — a `ResetVehicle` inside a live cylinder still counts as a
/// crossing on the next step, so the pose must not start there).
///
/// Returns `(position, yaw)` — upright facing down-leg. The caller adds
/// the spawn's hull clearance to `y`; the polyline's authored heights
/// are interpolated along the walked legs. `blocked` reports whether a
/// candidate is disallowed — the caller builds it from the race
/// definition's un-cleared checkpoint cylinders for this participant
/// (XZ radius) plus any further landing constraints the recovery
/// requires — both callers add the participant-occupancy clearance
/// [`reanchor_occupied`] (F15-B.10/B.11); the scripted player adds a
/// ground probe too.
pub fn reanchor_pose(
    route: &OpponentRoute,
    next: usize,
    pos: Vec3,
    yaw: f32,
    blocked: impl Fn(Vec3) -> bool,
) -> (Vec3, f32) {
    let (position, yaw, _) = reanchor_pose_with_progress(route, next, pos, yaw, blocked);
    (position, yaw)
}

/// The height a re-anchored car's origin sits above its route sample:
/// the spawn's hull clearance.
pub(crate) fn reanchor_lift(config: &mm2_vehicle::VehicleConfig) -> f32 {
    let hull_min_y = config
        .collider_points
        .as_ref()
        .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
        .unwrap_or(-config.chassis_size[1] * 0.5);
    (SPAWN_LIFT - hull_min_y).max(0.35)
}

/// `car` set down level at the route sample `p` facing `yaw`, on the
/// static ground under its footprint — `None` when there is none, or
/// the ground is deadly water. The sample's height is only the probe's
/// starting hint; the ground found sets the landing height.
#[allow(clippy::too_many_arguments)] // one landing's full spec: the probe, what counts as ground, the car, where and how high
pub(crate) fn seat_reanchor(
    spatial: &SpatialQuery,
    is_static: &dyn Fn(Entity) -> bool,
    water: Option<&crate::water::CityWater>,
    car: Entity,
    config: &mm2_vehicle::VehicleConfig,
    p: Vec3,
    yaw: f32,
    lift: f32,
) -> Option<Vec3> {
    let seated = seat_on_static_ground(spatial, is_static, car, config, p + Vec3::Y * lift, yaw)?;
    (!water.is_some_and(|w| w.is_deadly(seated))).then_some(seated)
}

/// The same bounded landing walk, retaining its exact authored leg occurrence.
/// Revisited streets cannot be disambiguated by a fresh-spawn position search.
pub fn reanchor_pose_with_progress(
    route: &OpponentRoute,
    next: usize,
    pos: Vec3,
    yaw: f32,
    blocked: impl Fn(Vec3) -> bool,
) -> (Vec3, f32, usize) {
    let n = route.points.len();
    if n == 0 || !pos.is_finite() {
        return (pos, yaw, next.min(n.saturating_sub(1)));
    }
    if n == 1 {
        let p = route.points[0].position;
        return if p.is_finite() {
            (p, yaw, 0)
        } else {
            (pos, yaw, 0)
        };
    }
    let closed = route_is_closed(route);
    let leg_count = if closed { n } else { n - 1 };
    // Leg i runs points[i] → points[(i+1) % n]. The chased leg is
    // prev→next; `next == 0` means the car still approaches the first
    // anchor — an open route's approach line is leg 0, a closed
    // route's is the wrap leg back into point 0.
    let mut leg = match next {
        0 if closed => leg_count - 1,
        0 => 0,
        i => (i - 1).min(leg_count - 1),
    };
    let geom = |i: usize| -> (Vec3, Vec3) {
        (route.points[i].position, route.points[(i + 1) % n].position)
    };
    let len = |i: usize| -> f32 {
        let (a, b) = geom(i);
        (b.x - a.x).hypot(b.z - a.z)
    };
    let point_on = |i: usize, d: f32| -> Vec3 {
        let (a, b) = geom(i);
        let l = len(i);
        if l > 1e-3 { a + (b - a) * (d / l) } else { a }
    };
    let facing = |i: usize, fallback: f32| -> f32 {
        let (a, b) = geom(i);
        let (dx, dz) = (b.x - a.x, b.z - a.z);
        if dx.hypot(dz) > 1e-3 {
            (-dx).atan2(-dz)
        } else {
            fallback
        }
    };
    // Project onto the chased leg — distance along it from its start.
    let mut d = {
        let (a, b) = geom(leg);
        let l = len(leg);
        if l > 1e-3 {
            (((pos.x - a.x) * (b.x - a.x) + (pos.z - a.z) * (b.z - a.z)) / l).clamp(0.0, l)
        } else {
            0.0
        }
    };
    // Walk backward along the polyline: REANCHOR_BACK for clearance,
    // then the same stride while the candidate sits inside an
    // un-cleared trigger — bounded by REANCHOR_WALK and, for an open
    // route, the start point. The step cap ends the walk on degenerate
    // or non-finite input instead of cycling the legs forever; a
    // non-finite candidate is never handed back — the input pose stands
    // in as the documented "walk reached nowhere" landing.
    let mut remaining = REANCHOR_BACK;
    let mut walked = 0.0f32;
    let mut steps = 0usize;
    loop {
        while remaining > 0.0 && steps < REANCHOR_MAX_STEPS {
            steps += 1;
            if remaining <= d {
                d -= remaining;
                walked += remaining;
                remaining = 0.0;
            } else {
                remaining -= d;
                walked += d;
                if leg == 0 && !closed {
                    d = 0.0;
                    remaining = 0.0;
                } else {
                    leg = (leg + leg_count - 1) % leg_count;
                    d = len(leg);
                }
            }
            if walked >= REANCHOR_WALK {
                remaining = 0.0;
            }
        }
        let pose = point_on(leg, d);
        if !pose.is_finite() {
            return (pos, yaw, next.min(n.saturating_sub(1)));
        }
        if remaining > 0.0
            || steps >= REANCHOR_MAX_STEPS
            || !blocked(pose)
            || walked >= REANCHOR_WALK
            || (leg == 0 && d <= 0.0 && !closed)
        {
            return (pose, facing(leg, yaw), (leg + 1) % n);
        }
        remaining = REANCHOR_BACK;
    }
}

/// The authored route with its leading anchors that sit behind the
/// staged heading dropped — everything after the staging record
/// (row 0, which keeps the authored heading) starts at the first anchor
/// [`initial_route_index`] would chase. Retail `.opp` lines routinely
/// author row 1 a car-length or two *behind* the grid (`sf` checkpoint
/// 2: row 1 sits 40 m behind a car facing −Z). Left in, the nav
/// re-path joins the facing-forward car to that anchor by the nearest
/// legal road — a lap around the block — and every point of that lap
/// reads as "ahead", so the car drives it before racing. Dropping the
/// anchors first keeps the first leg pointing the way the car faces.
/// Routes that need no trimming come back unchanged.
pub fn trim_behind_staging(route: &OpponentRoute, pos: Vec3, yaw: f32) -> OpponentRoute {
    let first = initial_route_index(route, pos, yaw);
    if first <= 1 || first >= route.points.len() {
        return route.clone();
    }
    let points = std::iter::once(route.points[0].clone())
        .chain(route.points[first..].iter().cloned())
        .collect();
    OpponentRoute { points }
}

/// Distance (m, XZ) within which a circuit's staging row counts as
/// standing on its lap — a road's width and then some.
const STAGED_ON_LAP: f32 = 30.0;

/// A circuit's lap as the original drives it, or `None` when `route`
/// is not a circuit record.
///
/// An `.opp` file is a start pose (row 0), the intersections of the
/// course in order (rows 1 … n−2) and a finish position (row n−1); a
/// circuit, on reaching the end of the list, wraps to row 1
/// (`0x55c2d0`, `docs/research/opponent-ai.md`). The lap is therefore
/// the intersection rows alone. The start pose and the finish position
/// are places *on* the lap — measured on every retail circuit they sit
/// on the road between rows 1 and 2 — not corners of it: closing the
/// loop through them sends the car from the last intersection past its
/// own grid to the finish position, back to the start pose, back
/// further to row 1 (which is behind the grid) and round again — two
/// reversals a lap.
///
/// The lap is returned closed on its first intersection, so
/// [`route_is_closed`] holds however far apart the first and last
/// intersections are (they are one road apart on retail `sf circuit6`
/// and `london circuit3`, the same junction everywhere else).
pub fn circuit_lap(route: &OpponentRoute) -> Option<OpponentRoute> {
    let n = route.points.len();
    if n < 6 || !route_is_closed(route) {
        return None;
    }
    let lap = &route.points[1..n - 1];
    let start = route.points[0].position;
    // The staging row must stand on the lap: on the road out of the
    // first intersection, or the one back into it.
    let off = |a: Vec3, b: Vec3| {
        let (dx, dz) = (b.x - a.x, b.z - a.z);
        let len2 = dx * dx + dz * dz;
        let t = if len2 > 1e-6 {
            (((start.x - a.x) * dx + (start.z - a.z) * dz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (start.x - (a.x + dx * t)).hypot(start.z - (a.z + dz * t))
    };
    let first = lap[0].position;
    let on_lap = off(first, lap[1].position) <= STAGED_ON_LAP
        || off(lap[lap.len() - 1].position, first) <= STAGED_ON_LAP;
    if !on_lap || !lap.iter().all(|p| p.position.is_finite()) {
        return None;
    }
    let mut points = lap.to_vec();
    if points[points.len() - 1].position != first {
        points.push(lap[0].clone());
    }
    Some(OpponentRoute { points })
}

/// The driving line a participant chases: `route` verbatim when no
/// road graph was bound at session load, else
/// [`NavGraph::densify_route`]'s re-pathed copy (F15-B.6). Ambient
/// closures are deliberately *not* applied — an aimap `[Exceptions]`
/// row forbids ambient traffic on a road, it does not remove the road
/// from a race course the `.opp` anchors route through. The session
/// builds `nav` with [`NavGraph::build_for_routing`], so roads whose
/// ambient classes are disabled (London's Tower Bridge approaches) stay
/// routable here. `gates` are the race's checkpoint triggers: a re-path
/// that abandons a gate the authored leg crossed is rejected so the
/// course stays crossable.
///
/// `pos`/`yaw` is the staged pose the car starts from: anchors behind
/// it are dropped by [`trim_behind_staging`] before the re-path, so the
/// densified line cannot open with a detour back to them. A circuit
/// record is first reduced to its lap ([`circuit_lap`]).
pub fn driving_route(
    route: &OpponentRoute,
    nav: Option<&NavGraph>,
    gates: &[mm2_game::Checkpoint],
    pos: Vec3,
    yaw: f32,
) -> OpponentRoute {
    let route = circuit_lap(route).unwrap_or_else(|| trim_behind_staging(route, pos, yaw));
    match nav {
        Some(g) => g.densify_route(
            &route,
            route_is_closed(&route),
            gates,
            &RouteOptions::default(),
        ),
        None => route,
    }
}

/// Everything a spawned AI vehicle carries beyond its physics body, all
/// from the authored [`VehicleDef`]: damage bounds, smoke/spark rigs,
/// stuck thresholds, audio bindings, breakaway parts, water/out-of-bounds
/// recovery anchored at the spawn pose, and the visible model. Shared by
/// the racing opponents and the police (F20-A.2) so both field the same
/// authored car; the roster-specific components stay with the callers.
/// `pos` is the final (lifted) spawn position.
#[allow(clippy::too_many_arguments)]
pub fn equip_authored_vehicle(
    commands: &mut Commands,
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    vehicle: Entity,
    def: &mm2_content::VehicleDef,
    object: ObjectId,
    pos: Vec3,
    yaw: f32,
) {
    // Authored damage bounds when the vehicle ships them — the
    // same spec the player accumulates against (F05-B.1); no
    // authored record means undamageable, never a fabricated one.
    if let Some(d) = &def.damage {
        commands
            .entity(vehicle)
            .insert(VehicleDamage::new(DamageSpec::from(d)));
        // F05-B.6: the same authored smoke rig the player gets —
        // seeded from the opponent's object id.
        commands.entity(vehicle).insert(VehicleSmoke::new(
            d,
            SmokePolicy::default(),
            (object.generation << 32) | object.slot as u64,
        ));
        // F05-B.8: the authored record owns the impact-spark
        // renderer too (DSN-26) — same seed domain.
        commands.entity(vehicle).insert(VehicleSparks::new(
            SparkPolicy::default(),
            (object.generation << 32) | object.slot as u64,
        ));
    }
    // Same for the authored stuck thresholds (F05-B.2).
    if let Some(s) = &def.stuck {
        commands
            .entity(vehicle)
            .insert(VehicleStuck::new(StuckSpec::from(s)));
    }
    // And the authored audio bindings (F07-B.2): the opponent-side
    // cardata table — `engine_rigs` resolves its stems through the
    // session's `WaveBank` into spatial loop voices. Same absence
    // policy as the player: no record, no component.
    if let Some(a) = &def.audio {
        commands
            .entity(vehicle)
            .insert(VehicleAudio { spec: a.clone() });
    }
    // And the authored breakaway inventory (F05-B.3, wheels included):
    // impact-gated panels plus damage-gated corners, only
    // `dgbangerdata`-backed parts, so authoredless cars detach nothing.
    if let Some(rig) = crate::breakaway::vehicle_rig(def) {
        commands.entity(vehicle).insert(rig);
    }
    // Water/out-of-bounds recovery (F05-B.5) — the designed policy
    // rides on every participant, anchored at its spawn pose.
    commands
        .entity(vehicle)
        .insert(VehicleRecovery::with_anchor(
            RecoveryPolicy::default(),
            pos,
            yaw,
        ));
    let missing = car_visual::spawn_vehicle_model(
        commands,
        vfs,
        &def.model,
        0,
        meshes,
        images,
        materials,
        vehicle,
        // F05-B.9: the authored record gates the texel rig —
        // same seed domain as the opponent's smoke/sparks.
        def.damage
            .as_ref()
            .map(|d| (d, (object.generation << 32) | object.slot as u64)),
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
}

/// Spawn every roster entry that loads as a real participant: its own
/// authored vehicle (opponent tuning preferred), a session-minted
/// `ObjectId`/`PlayerId`, `PlayerControl::Ai`, the session's authority
/// role, damage signals, `RaceProgress` on the shared definition and an
/// [`OpponentDriver`] — all stamped `owner` so teardown removes them
/// together. A vehicle that fails to load is warned and its authored
/// slot skipped; the roster's own issue list is untouched. Returns the
/// number spawned.
///
/// Single-player only: MP-4 (documented) removes AI opponents from
/// networked races — the lobby's humans take the grid instead — so the
/// `load_session_world` caller gates this on `SessionAuthority::Local`.
/// The `index + 1` slot rule below is therefore always relative to a
/// sole human at slot 0.
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
    nav: Option<&NavGraph>,
) -> usize {
    spawn_roster(
        commands, vfs, meshes, images, materials, roster, definition, owner, session, player_pos,
        player_yaw, nav, true,
    )
}

/// Spawn a Crash Course lesson's `[Opponent]` lead cars (F21-B.19):
/// the same authored vehicles, `.opp` routes and AI driver as the race
/// opponents ([`spawn_opponents`]), but **not participants** — no
/// `RaceProgress`, so the lesson's leg gates, results and sequencer
/// never see them, and each stands at its route's own staging pose
/// ([`lead_car_pose`]) instead of a grid slot. The driver holds them
/// still through the countdown and drives them while the field races;
/// the lineup stands for the whole session (lessons swap legs under
/// it), and a retry refields it with the rest of the session-owned
/// world. Single-player only, like the opponents. Returns the number
/// spawned.
#[allow(clippy::too_many_arguments)]
pub fn spawn_lead_cars(
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
    nav: Option<&NavGraph>,
) -> usize {
    spawn_roster(
        commands, vfs, meshes, images, materials, roster, definition, owner, session, player_pos,
        player_yaw, nav, false,
    )
}

/// The body behind [`spawn_opponents`] (`participant`) and
/// [`spawn_lead_cars`] (not).
#[allow(clippy::too_many_arguments)]
fn spawn_roster(
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
    nav: Option<&NavGraph>,
    participant: bool,
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
        let (mut pos, yaw) = if participant {
            spawn_pose(definition, i, spec, player_pos, player_yaw)
        } else {
            lead_car_pose(i, spec, player_pos, player_yaw)
        };
        // The chased line is the authored route re-pathed through the
        // road graph where one is bound — `spec` itself stays verbatim.
        let route = spec
            .route
            .as_ref()
            // No gate constrains the re-path: an opponent earns its
            // gates by route position, so a leg is always better driven
            // along the road than straight through a block to keep a
            // checkpoint cylinder on the line.
            .map(|r| driving_route(r, nav, &[], pos, yaw));
        // First chase index along the authored facing — early anchors
        // behind the staged heading are left for the next pass, not
        // chased off the spawn line.
        let next = route
            .as_ref()
            .map(|r| initial_route_index(r, pos, yaw))
            .unwrap_or(0);
        let neural = route
            .as_ref()
            .and_then(|r| Guide::from_route_on(r, nav))
            .map(|guide| NeuralState::new(Arc::new(guide), &def.config, pos, next));
        // Progress for AI participants is route-bound (DSN-45,
        // UNK-11): each gate binds where the car's guide passes
        // closest to it, so a course whose `.opp` line never threads a
        // cylinder still earns by driving the line — the authored-miss
        // defect class the F14-A.2 matrix named. An `AnyOrder` line
        // binds the finish too: the original's opponents finish on
        // their own course and the finish line, never on checkpoint
        // triggers their lines miss (`0x53d6c0`). A route-less entry
        // binds nothing and stays trigger-bound.
        let route_line = neural.as_ref().filter(|_| participant).and_then(|n| {
            let gates: Vec<f32> = definition
                .checkpoints
                .iter()
                .map(|g| n.guide.closest_approach(g.center))
                .collect();
            let mut line =
                RouteGateLine::bind_arcs(&gates, n.arc, n.guide.length(), n.guide.is_closed())?;
            if definition.rule == CheckpointRule::AnyOrder {
                line.bind_finish_arc(
                    definition
                        .finish
                        .as_ref()
                        .map(|f| n.guide.closest_approach(f.center)),
                );
            }
            Some(line)
        });
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
                OpponentDriver {
                    index: i,
                    spec: spec.clone(),
                    route,
                    throttle_cap: spec
                        .drive_params()
                        .max_throttle
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0),
                    next,
                    stuck_pos: pos,
                    stuck_frames: 0,
                    stuck_peak: 0,
                    reanchors: 0,
                    stats: DriveStats::default(),
                    neural,
                },
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                TransformInterpolation,
                // Parents of renderable children need the visibility chain.
                Visibility::Visible,
            ))
            .id();
        equip_authored_vehicle(
            commands, vfs, meshes, images, materials, vehicle, &def, object, pos, yaw,
        );
        if participant {
            commands
                .entity(vehicle)
                .insert(RaceProgress::new(definition));
        } else {
            // Not a participant: the Crash Course leg rules (F21-C.3)
            // find a lesson's lead car by this marker alone.
            commands.entity(vehicle).insert(LeadCar);
        }
        // Route-bound Ordered progress (DSN-45) — only present on
        // Circuit definitions with a resolvable route.
        if let Some(line) = route_line {
            commands.entity(vehicle).insert(line);
        }
        spawned += 1;
    }
    if spawned > 0 {
        if participant {
            info!(opponents = spawned, "opponent roster spawned");
        } else {
            info!(lead_cars = spawned, "lesson lead cars spawned");
        }
    }
    spawned
}

/// Drive every AI opponent for one fixed step: follow it along its
/// guide, feed its route-bound progress, and write the [`VehicleInput`]
/// its policy chooses.
///
/// Honors the race's gates: the field must still race — `Playing`, or
/// `Results` while the opponents behind a finished local driver race on
/// (DSN-11) — and the countdown's `input_locked` holds every car still.
/// A participant whose `RaceProgress` resolved pulls up on the
/// handbrake, as the original's opponent stops at its finish point. A
/// roster entry with no route (a dead `.opp` reference the roster
/// deliberately kept) holds still: the slot exists because the authored
/// lineup fields it, not because it can drive.
///
/// The policy for the session's difficulty decides every third step and
/// holds its action between ([`NeuralState::drive`]). It is shown every
/// other vehicle — players, opponents, police and ambient traffic are
/// obstacles alike — and the world through rays that ignore them. A
/// session normally carries its own [`DrivingPolicies`]; a world built
/// without one drives on the built-in pair.
///
/// The bounded last resort (F15-B.3): backing up answers an ordinary
/// stall, but a car penned where every escape lands back in the same
/// pocket, beached on its hull, or knocked somewhere its route cannot
/// be regained would sit forever. [`REANCHOR_FRAMES`] without
/// [`REANCHOR_DIST`] of displacement therefore re-anchors it through
/// the production [`ResetVehicle`] path — the same disclosed teleport
/// the player's reset uses, so `Teleported` breaks the swept segment —
/// to where its guide last had it: never further along the route than
/// the car drove, walked back out of un-cleared gates
/// ([`reanchor_pose`], AC04) and clear of every other car. The assist
/// is explicit and observable: `OpponentDriver::reanchors` counts it
/// and the smoke record surfaces the field total as `opp_rec=`.
/// Authority only — a predicted client never teleports a participant.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)] // Bevy system: the re-anchor landing threads the physics and water handles beside the driver query
pub fn opponent_drive(
    session: Res<Session>,
    time: Res<Time>,
    spatial: Option<SpatialQuery>,
    bodies: Query<&RigidBody>,
    colliders: Query<&ColliderOf>,
    water: Option<Res<crate::water::CityWater>>,
    race: Option<Res<RaceState>>,
    policies: Option<Res<DrivingPolicies>>,
    mut scratch: Local<Scratch>,
    cars: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            Option<&Vehicle>,
        ),
        Or<(With<Vehicle>, With<crate::traffic::AmbientCar>)>,
    >,
    mut resets: MessageWriter<ResetVehicle>,
    mut drivers: Query<(
        Entity,
        &mut VehicleInput,
        &Position,
        &Rotation,
        (&LinearVelocity, &AngularVelocity),
        &Vehicle,
        &VehicleState,
        Option<&RaceProgress>,
        &mut OpponentDriver,
        Option<&mut RouteGateLine>,
        Has<mm2_vehicle::Teleported>,
    )>,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    let policies = policies
        .as_deref()
        .unwrap_or_else(|| DrivingPolicies::built_in());
    let policy = policies.for_difficulty(
        session
            .config()
            .map_or(mm2_game::Difficulty::Amateur, |c| c.difficulty),
    );
    // Every vehicle a driver may need to know about: simulated cars at
    // their own size, ambient traffic at a typical one.
    let field: Vec<(Entity, Neighbour)> = cars
        .iter()
        .map(|(e, pos, rot, vel, vehicle)| {
            let size = vehicle.map_or(AMBIENT_SIZE, |v| v.config.chassis_size);
            (
                e,
                Neighbour {
                    position: pos.0,
                    velocity: vel.0,
                    forward: rot.0 * Vec3::NEG_Z,
                    half_width: size[0] * 0.5,
                    half_length: size[2] * 0.5,
                },
            )
        })
        .collect();
    // Poses a re-anchor has already claimed this frame. `field` predates
    // the loop, so it cannot see a same-frame teleport — without this,
    // two cars whose windows expire together (a shared pen, a pileup)
    // both land on the same spot.
    let mut claimed: Vec<Vec3> = Vec::new();
    for (
        entity,
        mut input,
        pos,
        rot,
        (lin_vel, ang_vel),
        vehicle,
        vstate,
        progress,
        mut driver,
        mut line,
        teleported,
    ) in &mut drivers
    {
        let d = &mut *driver;
        // A lesson's lead car carries no progress: it drives while the
        // field races, with nothing of its own to finish.
        let racing = progress.is_none_or(|p| {
            matches!(
                p.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            )
        });
        let live = session.field_races() && !locked && racing;
        // Follow the car along its guide. A car that was moved rather
        // than drove — a recovery from the water, any reset — is found
        // again behind where it was. The route point it is heading for
        // follows from its arc position, and the route is driven out
        // when the car is *at* its end, not merely past it: one that
        // ran wide of the last bend still has the end to reach.
        let mut driving = false;
        if live && let Some(neural) = d.neural.as_mut() {
            if teleported {
                neural.reseat(pos.0);
            }
            let left = neural.track(pos.0);
            let next = neural.guide.chase_index(neural.arc);
            let end = neural.guide.point_at(neural.guide.length());
            driving = !(left.is_some_and(|m| m < ROUTE_END)
                && (pos.0.x - end.x).hypot(pos.0.z - end.z) < ROUTE_END_RADIUS);
            // Route-bound progress (DSN-45): what the car has driven
            // along its guide is the high-water mark `advance_route`
            // reads — while it is on the line. One punted off it keeps
            // its place but earns nothing until it is back.
            if let Some(line) = &mut line {
                let on_line = neural.guide.point_at(neural.arc);
                if (pos.0.x - on_line.x).hypot(pos.0.z - on_line.z) <= ROUTE_ARC_LATERAL {
                    line.arc_high = line.arc_high.max(neural.progress());
                }
            }
            d.next = next;
        }
        if !driving {
            d.stuck_frames = 0;
            d.stuck_pos = pos.0;
            d.stats.last_pos = None;
            // A car that has finished pulls up, as the original's does
            // at its finish point; one waiting for the start, out of
            // route or without one just holds no pedal.
            *input = VehicleInput {
                handbrake: if racing { 0.0 } else { 1.0 },
                ..default()
            };
            continue;
        }
        d.stats.record(pos.0, time.delta_secs());
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        // F15-B.3 — the bounded last resort. The stuck window measures
        // displacement, not speed: a penned or hull-beached car can
        // roll and still never leave the bubble.
        if session.authority_role().is_authority() && pos.0.is_finite() {
            if pos.0.distance(d.stuck_pos) >= REANCHOR_DIST {
                d.stuck_pos = pos.0;
                d.stuck_frames = 0;
            } else {
                d.stuck_frames += 1;
                d.stuck_peak = d.stuck_peak.max(d.stuck_frames);
            }
            if d.stuck_frames >= REANCHOR_FRAMES
                && let Some(route) = d.route.clone()
                && let Some(neural) = d.neural.as_mut()
            {
                let gates: Vec<mm2_game::Checkpoint> = race
                    .zip(progress)
                    .map(|(r, progress)| {
                        progress
                            .remaining()
                            .filter_map(|i| r.definition.checkpoints.get(i))
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();
                // The landing keeps REANCHOR_CLEAR of every car (the
                // stuck car's own spot included — beached on the line
                // it should not teleport back onto itself) and of poses
                // other re-anchors claimed this frame — the spec's
                // competing-recovery-positions edge (F15-B.10).
                let occupied = |p: Vec3| {
                    reanchor_occupied(
                        p,
                        field
                            .iter()
                            .map(|(_, n)| n.position)
                            .chain(claimed.iter().copied()),
                    )
                };
                let lift = reanchor_lift(&vehicle.config);
                // The route's heights are lane or anchor samples, not a
                // ground promise: a pose may sit inside a wall block or
                // under a raised deck with nothing to stand on. A
                // candidate counts only where static, dry ground lies
                // under the level car, and that ground sets the height.
                let is_static = |c: Entity| {
                    let body = colliders.get(c).map_or(c, |c| c.body);
                    !bodies
                        .get(body)
                        .is_ok_and(|b| b.is_dynamic() || b.is_kinematic())
                };
                let seat = |p: Vec3, y: f32| {
                    spatial.as_ref().and_then(|sq| {
                        seat_reanchor(
                            sq,
                            &is_static,
                            water.as_deref(),
                            entity,
                            &vehicle.config,
                            p,
                            y,
                            lift,
                        )
                    })
                };
                // Back to where the guide last had the car, not to
                // wherever it was knocked: the landing can then never
                // sit further along the route than the car drove.
                let from = neural.guide.point_at(neural.arc);
                let (pose, ryaw, resync_next) =
                    reanchor_pose_with_progress(&route, d.next, from, yaw, |p| {
                        gates.iter().any(|g| {
                            let dx = p.x - g.center.x;
                            let dz = p.z - g.center.z;
                            dx * dx + dz * dz < g.radius * g.radius
                        }) || occupied(p)
                            || (spatial.is_some() && seat(p, yaw).is_none())
                    });
                // Without a physics world (or when the walk found no
                // ground at all) the authored height plus clearance
                // stands.
                let pose = seat(pose, ryaw).unwrap_or(pose + Vec3::Y * lift);
                resets.write(ResetVehicle {
                    entity: Some(entity),
                    position: pose,
                    yaw: ryaw,
                });
                // The car's place on its guide goes back with it, so the
                // teleport banks no arc it did not drive.
                d.next = resync_next;
                neural.reseat(pose);
                d.stuck_frames = 0;
                d.stuck_pos = pose;
                d.reanchors += 1;
                d.stats.last_pos = None;
                claimed.push(pose);
                info!(
                    vehicle = %d.spec.vehicle,
                    reanchors = d.reanchors,
                    from = ?pos.0,
                    to = ?pose,
                    "opponent re-anchored onto its route after a bounded stuck"
                );
                *input = VehicleInput::default();
                continue;
            }
        } else {
            d.stuck_pos = pos.0;
            d.stuck_frames = 0;
        }
        let Some(neural) = d.neural.as_mut() else {
            continue;
        };
        let motion = Motion {
            position: pos.0,
            rotation: rot.0,
            linear_velocity: lin_vel.0,
            angular_velocity: ang_vel.0,
            steer_angle: vstate.steer_angle,
            grounded: vstate.wheels.iter().filter(|w| w.grounded).count() as f32
                / vstate.wheels.len().max(1) as f32,
            reversing: vstate.direction == mm2_vehicle::DriveDirection::Reverse,
        };
        let neighbours: Vec<Neighbour> = field
            .iter()
            .filter(|(e, _)| *e != entity)
            .map(|(_, n)| *n)
            .collect();
        let filter = SpatialQueryFilter::default();
        let water = water.as_deref();
        *input = neural.drive(
            policy,
            &mut scratch,
            &motion,
            &neighbours,
            d.throttle_cap,
            |origin, dir, reach| {
                let sq = spatial.as_ref()?;
                let unit = Dir3::new(dir).ok()?;
                let hit = sq.cast_ray_predicate(origin, unit, reach, true, &filter, &|e| {
                    !cars.contains(e)
                })?;
                // Deadly water is no ground to drive on.
                let point = origin + dir * hit.distance;
                (!water.is_some_and(|w| w.is_deadly(point))).then_some((hit.distance, hit.normal))
            },
        );
    }
}

/// The size (m: width, height, length) an ambient traffic car is given
/// as a neighbour — it carries no handling config to read one from.
const AMBIENT_SIZE: [f32; 3] = [2.0, 1.4, 4.6];

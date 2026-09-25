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
//!   loop the polyline); [`crate::scripted::scripted_input_tuned`] owns how
//!   it gets there (proportional steering, corner braking, bounded
//!   reverse-and-turn stuck recovery — the same normalized-input
//!   control law the scripted evidence driver uses).
//! - **Traffic (F15-B.1).** Each opponent also senses the other
//!   *participants* in a corridor ahead — the player included — from
//!   live physics state, never map data: a parked car and a moving one
//!   are the same obstacle until it moves. A blocker inside the
//!   corridor commits a pass side and the aim moves to a point down
//!   the offset lane, so the same steering law lane-changes around it;
//!   inside the closing-scaled follow gap the demand becomes a brake
//!   instead of a shove. The pass target is tracked until it falls
//!   fully behind — the corridor alone would release alongside and the
//!   merge would cut back across its nose. Static geometry (walls,
//!   props) is deliberately not sensed here; a hit the pass cannot
//!   make stays the recovery law's job.
//! - **Authored tuning (F15-B.2/B.8).** The `[Opponent]` row's
//!   ten-value parameter tail — the documented
//!   `aiVehiclePhysics::RegisterRoute` behavioral vocabulary (R3's
//!   published column table + mm2hook's recovered signature, R4;
//!   ledger RACE-14) — resolves at spawn into a per-driver
//!   [`ScriptedTuning`] and corridor sense mask: `maxThrottle`
//!   ceilings the car's throttle demand, the corner-speed multiplier
//!   scales the corner-brake engage speed, the look-ahead distance
//!   sets the corridor's reach, and the authored `avoidPlayers`/
//!   `avoidOpponents` flags gate whether the corridor senses human or
//!   AI participants at all (an unsensed class is fully transparent —
//!   59% of retail rows author `avoidOpponents=0`, so stock opponents
//!   genuinely do not dodge each other). `avoidTraffic`/`avoidProps`
//!   bind but stay inert — ambient cars are not `Player`
//!   participants and the corridor never sensed props; the remaining
//!   columns stay decoded-but-unconsumed pending verified semantics.
//! - **Bounded re-anchor (F15-B.3).** A car the escapes and pass
//!   machinery cannot free — penned, hull-beached with unloaded wheels
//!   (the stuck detector only counts grounded cars), or knocked off
//!   the route entirely — gets a disclosed last resort after
//!   [`REANCHOR_FRAMES`] without [`REANCHOR_DIST`] of displacement: a
//!   `ResetVehicle` teleport back onto the chased route leg, walked
//!   back out of un-cleared checkpoint triggers so the assist cannot
//!   bank a gate it never drove, and kept [`REANCHOR_CLEAR`] off every
//!   participant — including poses claimed by another re-anchor in the
//!   same frame — so competing recoveries cannot land interpenetrating
//!   (F15-B.10). `OpponentDriver::reanchors` counts each one and the
//!   smoke record reports the field total; designed policy (DSN-14),
//!   not a verified original rule.
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
//! convention), else the `.opp` row-0 staging position, else a designed
//! stagger behind the player. Facing is authored wherever the position
//! came from: the slot's `yaw_deg` on a grid, the `.opp` row-0 staging
//! heading on a route anchor — both measured as the vehicle-yaw
//! convention (the `_strtpnts` `a` column and the `.opp` `brake` field
//! agree; the waypoint `a` column is the opposite bearing — the UNK-16
//! split, now measured). A grid slot authoring `a = 0` carries no
//! heading (`yaw_deg == None` — `cir6_strtpnts` ships all-zero while
//! its routes stage ~180°) and falls through to the same staged →
//! first-leg chain rather than facing backward off the course. Which
//! of the two authored start sets the original consumes stays open
//! under UNK-17.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    BreakPartSpec, CheckpointRule, DamageSignals, DamageSpec, NavGraph, ObjectIdentity,
    OpponentRoster, OpponentRoute, OpponentSpec, ParticipantState, Player, PlayerControl,
    RaceDefinition, RaceProgress, RaceState, RecoveryPolicy, RouteGateLine, RouteOptions, Session,
    SessionEntity, SmokePolicy, SparkPolicy, StuckSpec, VehicleAudio, VehicleBreaks, VehicleDamage,
    VehicleRecovery, VehicleSmoke, VehicleSparks, VehicleStuck, relative_bearing,
};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState, vehicle_bundle};
use tracing::{info, warn};

use crate::car_visual;
use crate::scripted::{ScriptedBot, ScriptedTuning, scripted_input_tuned};

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
/// margin the player gets before its hull-clearance lift. Shared with
/// the scripted bot's re-anchor (`crate::scripted`).
pub(crate) const SPAWN_LIFT: f32 = 0.25;

/// Half-width of the brake corridor ahead (m) — about a car width:
/// a car sharing our line counts, one in the neighbouring lane does
/// not (test lanes sit 6 m apart; retail lanes are wider).
const BLOCK_HALF_WIDTH: f32 = 2.4;
/// Half-width (m) of the pass-room scan either side of the nose — the
/// far lane must be empty at commit, not just the corridor, so a
/// pass cannot aim into traffic the corridor never saw.
const PASS_SCAN: f32 = 7.0;
/// Obstacle-sensing reach (m) when the authored tail supplies no
/// look-ahead distance — `RegisterRoute`'s `someDistancePadding`
/// default (R4). Authored rows carry their own (retail 50–150).
const LOOK_AHEAD_DEFAULT: f32 = 75.0;
/// Lateral shift applied to the aim point to drive around a blocker —
/// just over a car width.
const PASS_OFFSET: f32 = 3.6;
/// How far ahead the offset-lane aim point sits while a pass is
/// committed (m) — far enough that the merge-in is a gentle arc, near
/// enough that the lane change starts immediately.
const PASS_LOOKAHEAD: f32 = 12.0;
/// |lateral| below which a blocker counts as dead-centre — the pass
/// side then falls to the route side, then a fixed default.
const CENTRED_LAT: f32 = 0.5;
/// How far behind (m, negative gap) a committed pass target must fall
/// before the pass releases — until then the offset holds so the merge
/// cannot cut back across its nose.
const PASS_BEHIND: f32 = 4.0;
/// Wider lateral bound (m) the committed pass target is held inside —
/// alongside it has left the brake corridor but the pass is not done.
const PASS_WIDE: f32 = 4.5;
/// Frames without any threat before the committed pass side releases —
/// long enough to finish the overtake, short enough to re-pick.
const PASS_RELEASE: u32 = 45;
/// Displacement (m) a held pass must cover inside its stall window —
/// speed alone oscillates through recovery bursts and resets a
/// velocity check; a car shuffling inside a few metres is not
/// progressing even when it briefly rolls.
const PASS_STALL_DIST: f32 = 8.0;
/// Frames a held pass may go without [`PASS_STALL_DIST`] of progress
/// before it is abandoned — an offset aim at an unyielding gap (wall
/// on the pass side, a wedged blocker, a parked player) cannot hold
/// forever: the bounded response is to give the gap up and drive the
/// route (AC03).
const PASS_STALL: u32 = 360;
/// Frames an abandoned blocker is barred from re-committing a pass —
/// long enough to push or slip past it on the route line, then the
/// clean pass gets another try.
const PASS_BAN: u32 = 300;
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
/// Base gap (m) the follower keeps behind a blocker.
const FOLLOW_GAP: f32 = 6.0;
/// Extra follow gap per m/s of *closing* speed (~0.5 s of travel).
const FOLLOW_TIME: f32 = 0.5;
/// Closing speed (m/s) below which the follow brake releases — at
/// crawl pace the steering, biased a car width off the blocker, is the
/// avoidance; braking there would stall the pass it is holding open.
const FOLLOW_RELEASE: f32 = 2.0;
/// Center-to-center gap (m) that brakes regardless of pace — bumpers
/// nearly touching; releases again the moment the car stops closing,
/// so a parked blocker cannot deadlock the follower.
const PANIC_GAP: f32 = 4.0;
/// Blocker speed (m/s) below which it counts as standing: a parked
/// car is steered around, a moving one is trailed at the comfort gap.
const CRAWL_SPEED: f32 = 3.0;

/// Distance-to-objective scale (m) the catch-up course measure falls
/// back to when a definition's own gate spacing cannot be measured —
/// designed (DSN-27), near a typical city block's length so a
/// participant a block behind banks roughly one gate unit of deficit.
const CATCH_UP_LEG_REF: f32 = 80.0;

/// Lateral corridor (m) around the chased route leg inside which a
/// pose still earns route arc for the [`RouteGateLine`] high-water —
/// a committed pass or a reverse-and-turn escape stays inside it, a
/// car punted onto a parallel street does not (designed, DSN-45).
/// Route credit cannot accumulate off the line.
const ROUTE_ARC_LATERAL: f32 = 25.0;

/// Per-opponent controller state — a component on the vehicle so
/// session teardown despawns it with everything else the session owns.
/// Carries the authored [`OpponentSpec`] verbatim (vehicle id, raw
/// parameter tail, authored route record) plus [`OpponentDriver::route`]
/// — the driving line actually chased, which session load may have
/// re-pathed through the road graph (F15-B.6).
#[derive(Component, Debug, Clone)]
pub struct OpponentDriver {
    /// This participant's index in the authored roster — the slot the
    /// lineup entry occupied, kept so evidence (the smoke record's
    /// `opps=` field) names the authored slot even when a neighbour's
    /// vehicle failed to load and its slot was skipped.
    pub index: usize,
    /// The authored lineup entry this participant was spawned from.
    pub spec: OpponentSpec,
    /// The route the driver chases — the authored `.opp` line verbatim
    /// when no road graph was bound at session load, else
    /// [`NavGraph::densify_route`]'s re-pathed copy: authored anchors
    /// kept verbatim with lane-sampled points spliced between them on
    /// legs whose straight line leaves the road corridor. `None` when
    /// the entry resolved no route at all.
    pub route: Option<OpponentRoute>,
    /// Control-law tuning resolved from the authored parameter tail at
    /// spawn (F15-B.2): `maxThrottle` → the throttle ceiling, the
    /// corner-speed multiplier → the corner-brake engage speed.
    /// `None` columns take the `RegisterRoute` defaults — full
    /// throttle, base corner speed — i.e. the pre-tail behavior.
    pub tuning: ScriptedTuning,
    /// Whether the corridor senses human participants — authored
    /// `avoidPlayers`, column 6 (default on, the `RegisterRoute`
    /// default). Retail authors 0 on 75% of rows — most stock
    /// opponents do not dodge the player.
    pub avoid_players: bool,
    /// Whether the corridor senses fellow AI opponents — authored
    /// `avoidOpponents`, column 7 (default on). 59% of retail rows
    /// author 0 — the field does not dodge itself on those rows
    /// (documented polarity, R3/R4).
    pub avoid_opponents: bool,
    /// The corridor's obstacle-sensing reach (m) — authored
    /// `Look Ahead Distance`, column 2 ([`LOOK_AHEAD_DEFAULT`] when
    /// the column is absent; a designed reading of the documented
    /// name, R3). Feeds both the brake corridor and the pass hold
    /// window.
    pub look_ahead: f32,
    /// Index of the route point currently being chased.
    pub next: usize,
    /// Bounded stuck-recovery state machine — the same three-point
    /// escape the scripted driver uses (F15-AC03's recovery leg).
    pub recovery: ScriptedBot,
    /// The participant currently being driven around, if any —
    /// tracked until it falls [`PASS_BEHIND`] metres behind, so the
    /// merge cannot cut back across its nose the moment it leaves the
    /// brake corridor.
    pub pass_entity: Option<Entity>,
    /// The committed pass side as ±1.0 (0 = uncommitted): which side
    /// of the route line the offset lane runs on. Held while a threat
    /// persists so the pick cannot flicker between sides, released
    /// [`PASS_RELEASE`] frames after the corridor clears. The lateral
    /// direction itself is re-derived from the route leg each frame,
    /// so the offset lane bends with the road instead of freezing a
    /// stale heading or orbiting with the car's nose.
    pub pass_side: f32,
    /// Frames since the corridor last held a threat.
    pub clear_frames: u32,
    /// Frames since the held pass last covered [`PASS_STALL_DIST`] —
    /// at [`PASS_STALL`] the commitment is abandoned rather than
    /// held forever at an unyielding gap (AC03's bounded response).
    pub stall_frames: u32,
    /// Where the current stall window started — progress is measured
    /// as displacement from here, not momentary speed, so recovery
    /// shuffles do not masquerade as a working pass.
    pub stall_pos: Vec3,
    /// A blocker whose abandoned pass is barred from re-committing,
    /// with the frames left on the ban — the car drives the route
    /// line past it (push or slip) before the clean pass retries.
    pub pass_ban: Option<(Entity, u32)>,
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
    /// The catch-up policy this driver runs under (designed, DSN-27 —
    /// see [`mm2_game::CatchUpPolicy`]). `pub` so tests and evidence
    /// runs can bind different bounds without touching authored data.
    pub catch_up_policy: mm2_game::CatchUpPolicy,
    /// The catch-up assist currently applied — the gates-behind factor
    /// in `0..=catch_up_policy.assist_max`, `0` while leading, level
    /// or not racing. The observable the smoke record's `cu=` field
    /// counts, kept distinct from `reanchors` on purpose: lifted
    /// demand is not a recovery.
    pub catch_up: f32,
}

impl OpponentDriver {
    /// Whether the authored avoid flags let the corridor sense `t` —
    /// human participants under `avoidPlayers`, AI under
    /// `avoidOpponents` (F15-B.2/B.8). An unsensed class is fully
    /// transparent: no pass, no brake, no ban — the authored lineup
    /// drives through it.
    pub fn senses(&self, t: &Traffic) -> bool {
        match t.control {
            PlayerControl::Ai => self.avoid_opponents,
            _ => self.avoid_players,
        }
    }
}

/// Whether `pos` has reached route point `i`: inside [`ROUTE_REACH`],
/// or past its perpendicular plane along the direction of travel — a
/// car braked or shoved beyond an anchor chases forward instead of
/// U-turning back to it. The incoming leg supplies the direction for
/// interior points; point 0 has none, so its outgoing leg decides —
/// anything already ahead of the first anchor skips it.
pub(crate) fn point_reached(points: &[mm2_game::OpponentRoutePoint], i: usize, pos: Vec3) -> bool {
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

/// Whether a route's last point sits within [`ROUTE_LOOP`] of its
/// first — the retail circuit pattern, where the lap structure lives
/// in the race checkpoints and the route just keeps supplying
/// waypoints by rejoining at the first anchor.
pub(crate) fn route_is_closed(route: &OpponentRoute) -> bool {
    let points = &route.points;
    points.len() > 1 && {
        let last = points.last().unwrap().position;
        let first = points[0].position;
        (last.x - first.x).hypot(last.z - first.z) <= ROUTE_LOOP
    }
}

/// Advance the chase index past reached points and return the point to
/// drive at. Returns the updated `next` and `Some(target)`; `None` when
/// the route is complete — an open polyline's end means the opponent
/// eases to a stop (its race progress is the checkpoints', not the
/// route's). A closed route rejoins at the first anchor so laps keep
/// supplying targets; the bounded retry keeps a degenerate all-in-reach
/// route from looping forever.
pub fn route_target(route: &OpponentRoute, mut next: usize, pos: Vec3) -> (usize, Option<Vec3>) {
    let points = &route.points;
    if points.is_empty() {
        return (next, None);
    }
    let closed = route_is_closed(route);
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
/// row-0 staging position, else a staggered line behind the player
/// along its reverse heading.
///
/// Facing follows the position source, all authored: a grid slot's own
/// `yaw_deg` (measured vehicle-yaw convention — the `a` column faces
/// the course, not away from it), else the `.opp` row-0 staging
/// heading (the mislabelled `brake` column — measured as the same yaw
/// convention on 592/612 retail files). Only a route with no authored
/// heading falls back to the first-leg direction it used before, and
/// a route-less entry inherits the player's yaw. A grid slot carrying
/// no authored heading (`yaw_deg == None` — `cir6_strtpnts`' all-zero
/// `a` column) takes the same staged-heading → first-leg chain: a
/// verbatim 0 would face the slot backward off the measured course
/// (the staged routes there run ~180°).
pub fn spawn_pose(
    definition: &RaceDefinition,
    index: usize,
    spec: &OpponentSpec,
    player_pos: Vec3,
    player_yaw: f32,
) -> (Vec3, f32) {
    let slot = definition.start_slots.get(index + 1);
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
        if ahead && !point_reached(&route.points, next, pos) {
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
    let n = route.points.len();
    if n == 0 {
        return (pos, yaw);
    }
    if n == 1 {
        return (route.points[0].position, yaw);
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
    // route, the start point.
    let mut remaining = REANCHOR_BACK;
    let mut walked = 0.0f32;
    loop {
        while remaining > 0.0 {
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
        if !blocked(pose) || walked >= REANCHOR_WALK || (leg == 0 && d <= 0.0 && !closed) {
            return (pose, facing(leg, yaw));
        }
        remaining = REANCHOR_BACK;
    }
}

/// The driving line a participant chases: `route` verbatim when no
/// road graph was bound at session load, else
/// [`NavGraph::densify_route`]'s re-pathed copy (F15-B.6). Ambient
/// closures are deliberately *not* applied — an aimap `[Exceptions]`
/// row forbids ambient traffic on a road, it does not remove the road
/// from a race course the `.opp` anchors route through. `gates` are
/// the race's checkpoint triggers: a re-path that abandons a gate the
/// authored leg crossed is rejected so the course stays crossable.
pub fn driving_route(
    route: &OpponentRoute,
    nav: Option<&NavGraph>,
    gates: &[mm2_game::Checkpoint],
) -> OpponentRoute {
    match nav {
        Some(g) => g.densify_route(
            route,
            route_is_closed(route),
            gates,
            &RouteOptions::default(),
        ),
        None => route.clone(),
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
        // The chased line is the authored route re-pathed through the
        // road graph where one is bound — `spec` itself stays verbatim.
        let route = spec
            .route
            .as_ref()
            .map(|r| driving_route(r, nav, &definition.checkpoints));
        // First chase index along the authored facing — early anchors
        // behind the staged heading are left for the next pass, not
        // chased off the spawn line.
        let next = route
            .as_ref()
            .map(|r| initial_route_index(r, pos, yaw))
            .unwrap_or(0);
        // Ordered progress for AI participants is route-bound
        // (DSN-45, UNK-11): each gate binds at the driven line's
        // closest-approach arc, so a course whose `.opp` line never
        // threads a cylinder still earns by driving the line — the
        // authored-miss defect class the F14-A.2 matrix named.
        // AnyOrder stays trigger-bound; a route-less entry binds
        // nothing.
        let route_line = (definition.rule == CheckpointRule::Ordered)
            .then(|| {
                route.as_ref().and_then(|r| {
                    RouteGateLine::bind(r, &definition.checkpoints, route_is_closed(r), pos, next)
                })
            })
            .flatten();
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
        let drive_params = spec.drive_params();
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
                    index: i,
                    spec: spec.clone(),
                    route,
                    tuning: ScriptedTuning {
                        throttle_cap: drive_params.max_throttle.unwrap_or(1.0).clamp(0.0, 1.0),
                        corner_speed: ScriptedTuning::DEFAULT.corner_speed
                            * drive_params.corner_speed_multiplier.unwrap_or(1.0).max(0.0),
                    },
                    avoid_players: drive_params.avoid_players.unwrap_or(true),
                    avoid_opponents: drive_params.avoid_opponents.unwrap_or(true),
                    look_ahead: drive_params
                        .distance_padding
                        .filter(|d| d.is_finite() && *d > 0.0)
                        .unwrap_or(LOOK_AHEAD_DEFAULT),
                    next,
                    recovery: ScriptedBot::default(),
                    pass_entity: None,
                    pass_side: 0.0,
                    clear_frames: 0,
                    stall_frames: 0,
                    stall_pos: Vec3::ZERO,
                    pass_ban: None,
                    stuck_pos: pos,
                    stuck_frames: 0,
                    stuck_peak: 0,
                    reanchors: 0,
                    catch_up_policy: mm2_game::CatchUpPolicy::default(),
                    catch_up: 0.0,
                },
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                TransformInterpolation,
                // Parents of renderable children need the visibility chain.
                Visibility::Visible,
            ))
            .id();
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
        // And the authored breakaway inventory (F05-B.3): only
        // `dgbangerdata`-backed BREAK chunks, so authoredless cars
        // detach nothing.
        if !def.breaks.is_empty() {
            commands.entity(vehicle).insert(VehicleBreaks::new(
                def.breaks
                    .iter()
                    .map(|b| BreakPartSpec {
                        name: b.name.clone(),
                        def: b.def.clone(),
                    })
                    .collect(),
            ));
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
        // Route-bound Ordered progress (DSN-45) — only present on
        // Circuit definitions with a resolvable route.
        if let Some(line) = route_line {
            commands.entity(vehicle).insert(line);
        }
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
        spawned += 1;
    }
    if spawned > 0 {
        info!(opponents = spawned, "opponent roster spawned");
    }
    spawned
}

/// Another participant on the road — the traffic the avoidance reads.
/// Positions, headings and speeds come from live physics state: the
/// local player and every other AI are obstacles exactly alike. The
/// authored avoid flags then decide which control classes each
/// opponent senses at all (F15-B.2).
#[derive(Debug, Clone, Copy)]
pub struct Traffic {
    /// The participant's entity — self is excluded by comparison.
    pub entity: Entity,
    /// Which input authority drives it — the authored avoid flags key
    /// on this class.
    pub control: PlayerControl,
    /// World position this step.
    pub pos: Vec3,
    /// Its nose direction (XZ-projected heading basis).
    pub fwd: Vec3,
    /// Signed speed along its own nose (m/s); 0 parked.
    pub speed: f32,
}

/// A participant resolved into our drive frame — the unit of both the
/// brake corridor and the pass hold window.
#[derive(Debug, Clone, Copy)]
pub struct Blocker {
    /// The traffic entry this describes.
    pub entity: Entity,
    /// Distance along our heading (m): ahead positive, behind negative.
    pub gap: f32,
    /// Lateral offset in our frame (m, + = our right).
    pub lat: f32,
    /// Its speed projected along our heading (m/s) — a parked or
    /// crossing car reads ≈0, a same-direction car its own pace.
    pub speed: f32,
}

/// Resolve one traffic entry into our drive frame. `fwd` is our
/// projected nose direction; right = `fwd × up`.
fn blocker_at(pos: Vec3, fwd: Vec3, t: &Traffic) -> Blocker {
    let rel = t.pos - pos;
    let gap = rel.x * fwd.x + rel.z * fwd.z;
    let lat = rel.x * -fwd.z + rel.z * fwd.x;
    let along = t.speed * (t.fwd.x * fwd.x + t.fwd.z * fwd.z);
    Blocker {
        entity: t.entity,
        gap,
        lat,
        speed: along,
    }
}

/// The nearest participant ahead inside the brake corridor — the
/// overtake/brake trigger. Only strictly-positive gaps count: a car
/// already alongside or behind is the hold window's question, not a
/// new blocker. `senses` is the driver's authored avoid mask — a
/// class it does not sense never registers as a blocker.
pub fn nearest_blocker(
    entity: Entity,
    pos: Vec3,
    fwd: Vec3,
    reach: f32,
    traffic: &[Traffic],
    senses: impl Fn(&Traffic) -> bool,
) -> Option<Blocker> {
    traffic
        .iter()
        .filter(|t| t.entity != entity && senses(t))
        .map(|t| blocker_at(pos, fwd, t))
        .filter(|b| b.gap > 0.0 && b.gap <= reach && b.lat.abs() < BLOCK_HALF_WIDTH)
        .min_by(|a, b| a.gap.total_cmp(&b.gap))
}

/// The committed pass target while it is still beside or ahead of us —
/// a wider window than the brake corridor, held until the obstacle
/// falls [`PASS_BEHIND`] metres behind. Without it the corridor would
/// release the moment we pull alongside and the merge would cut back
/// across the blocker's nose.
fn held_blocker(
    entity: Entity,
    pos: Vec3,
    fwd: Vec3,
    reach: f32,
    traffic: &[Traffic],
) -> Option<Blocker> {
    traffic
        .iter()
        .find(|t| t.entity == entity)
        .map(|t| blocker_at(pos, fwd, t))
        .filter(|b| b.gap > -PASS_BEHIND && b.gap < reach && b.lat.abs() < PASS_WIDE)
}

/// Which way around a blocker (±1.0 in the lateral frame): an offset
/// blocker is passed on its open side; a dead-centre one on the side
/// the route itself continues, then a fixed default. Commitment is
/// structural — the picked side lives in `OpponentDriver::pass_side`
/// until the corridor clears, so alternating blockers cannot flip it.
pub fn pick_pass_side(blocker_lat: f32, target_lat: f32) -> f32 {
    if blocker_lat.abs() >= CENTRED_LAT {
        -blocker_lat.signum()
    } else if target_lat.abs() >= CENTRED_LAT {
        target_lat.signum()
    } else {
        1.0
    }
}

/// Convert route demand into following distance. A *moving* blocker
/// inside the comfort gap brakes even at matched pace — a queue that
/// never trims its gaps rides bumpers into every corner. A *standing*
/// blocker brakes only on a meaningful approach: at crawl pace the
/// steering, biased a car width off it, is the avoidance, and braking
/// there is what deadlocked the follower behind parked cars (AC03).
/// Either way the brake's reach grows with *closing* speed — a parked
/// car brakes early from a fast approach — and inside [`PANIC_GAP`]
/// an active closure brakes hard regardless.
pub fn apply_gap_brake(input: &mut VehicleInput, blocker: &Blocker, speed: f32) {
    let closing = speed - blocker.speed;
    if blocker.gap < PANIC_GAP {
        if closing > 0.5 {
            input.throttle = 0.0;
            input.brake = input.brake.max(0.85);
        }
        return;
    }
    let need = FOLLOW_GAP + closing.max(0.0) * FOLLOW_TIME;
    if blocker.gap >= need {
        return;
    }
    let brake = if blocker.speed >= CRAWL_SPEED {
        // Adaptive-cruise deficit, softer than the panic stop.
        (0.25 + (need - blocker.gap) / need * 0.5).min(0.85)
    } else if closing > FOLLOW_RELEASE {
        (0.3 + (need - blocker.gap) / need * 0.55).min(0.85)
    } else {
        return;
    };
    input.throttle = 0.0;
    input.brake = input.brake.max(brake);
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
///
/// With a target, the corridor sense runs before the control law: a
/// participant ahead shifts the aim point one car width to the
/// committed pass side and the follow gap converts into braking, so a
/// slower or stopped car is driven around or trailed — never shoved
/// (F15-B.1, AC03's blocked-road leg).
///
/// The bounded last resort (F15-B.3): the reverse-and-turn escapes and
/// the pass stall/ban cycle answer ordinary blocks, but a car penned
/// where every escape lands back in the same pocket, beached on its
/// hull with the wheels unloaded, or knocked somewhere the route cannot
/// be regained would sit forever — the stuck detector itself never
/// counts an ungrounded car. [`REANCHOR_FRAMES`] without
/// [`REANCHOR_DIST`] of displacement therefore re-anchors it onto the
/// route it was chasing through the production [`ResetVehicle`] path —
/// the same disclosed teleport the player's reset uses, so `Teleported`
/// breaks the swept segment and [`reanchor_pose`]'s trigger walk-back
/// keeps the landing out of un-cleared gates (AC04). The assist is
/// explicit and observable: `OpponentDriver::reanchors` counts it and
/// the smoke record surfaces the field total as `opp_rec=`. Authority
/// only — a predicted client never teleports a participant.
#[allow(clippy::type_complexity)]
pub fn opponent_drive(
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut set: ParamSet<(
        Query<(
            Entity,
            &mut VehicleInput,
            &Position,
            &Rotation,
            &Vehicle,
            &VehicleState,
            &RaceProgress,
            &mut OpponentDriver,
            Option<&mut RouteGateLine>,
        )>,
        Query<(
            Entity,
            &Player,
            &Position,
            &Rotation,
            &VehicleState,
            Option<&RaceProgress>,
        )>,
    )>,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    let traffic: Vec<Traffic> = set
        .p1()
        .iter()
        .map(|(entity, player, pos, rot, vstate, _)| Traffic {
            entity,
            control: player.control,
            pos: pos.0,
            fwd: rot.0 * Vec3::NEG_Z,
            speed: vstate.forward_speed,
        })
        .collect();
    // F15-B.4 — the disclosed catch-up assist (designed, DSN-27). The
    // leader is the best continuous course position among every
    // progress-carrying participant — the human included, so a
    // trailing field's lift is measured against whoever is actually
    // ahead, not an AI-only shuffle. The leg scale is the course's
    // own mean gate spacing; a degenerate definition falls back to
    // the designed `CATCH_UP_LEG_REF`.
    let leg_ref = race
        .and_then(|r| mm2_game::mean_gate_spacing(&r.definition))
        .unwrap_or(CATCH_UP_LEG_REF);
    let leader = race.map(|r| {
        set.p1()
            .iter()
            .filter_map(|(_, _, pos, _, _, progress)| progress.map(|p| (pos.0, p)))
            .map(|(p_pos, progress)| {
                mm2_game::course_progress(&r.definition, progress, p_pos, leg_ref)
            })
            .fold(f32::NEG_INFINITY, f32::max)
    });
    // Poses a re-anchor has already claimed this frame. The `traffic`
    // snapshot predates the loop, so it cannot see a same-frame
    // teleport — without this, two cars whose windows expire together
    // (a shared pen, a pileup) both land on the same projected spot.
    let mut claimed: Vec<Vec3> = Vec::new();
    for (entity, mut input, pos, rot, vehicle, vstate, progress, mut driver, mut line) in
        &mut set.p0()
    {
        if let Some((_, left)) = &mut driver.pass_ban {
            *left = left.saturating_sub(1);
            if *left == 0 {
                driver.pass_ban = None;
            }
        }
        let racing = matches!(
            progress.state,
            ParticipantState::AwaitingStart | ParticipantState::Racing
        );
        let target = if !session.is_playing() || locked || !racing {
            None
        } else if let Some(route) = &driver.route {
            let (next, target) = route_target(route, driver.next, pos.0);
            if let Some(line) = &mut line {
                // Route-bound Ordered progress (DSN-45): a wrap of the
                // chase index completes a traversal, and the pose's
                // projection inside the chased leg's corridor grows
                // the high-water arc `advance_route` reads. The
                // lateral gate keeps punts off the line from earning.
                if line.is_closed() && next < driver.next {
                    line.wrap();
                }
                let (arc, lateral) = line.measure(route, next, pos.0);
                if lateral <= ROUTE_ARC_LATERAL {
                    line.arc_high = line.arc_high.max(arc);
                }
            }
            driver.next = next;
            target
        } else {
            None
        };
        let Some(mut target) = target else {
            driver.pass_entity = None;
            driver.pass_side = 0.0;
            driver.clear_frames = 0;
            driver.stall_frames = 0;
            driver.stuck_frames = 0;
            driver.stuck_pos = pos.0;
            driver.catch_up = 0.0;
            *input = VehicleInput::default();
            continue;
        };
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        // F15-B.3 — the bounded last resort. The stuck window measures
        // displacement, not speed: a penned or hull-beached car can
        // roll and still never leave the bubble, and an ungrounded one
        // never even reaches the recovery law's stuck counter. Once
        // the budget is spent the car teleports onto its chased route
        // through `ResetVehicle` — the same disclosed path the
        // player's reset uses, so the jump cannot sweep checkpoints
        // and the walk-back keeps the landing out of un-cleared
        // triggers. `reanchors` is the observable count.
        if session.authority_role().is_authority() && pos.0.is_finite() {
            if pos.0.distance(driver.stuck_pos) >= REANCHOR_DIST {
                driver.stuck_pos = pos.0;
                driver.stuck_frames = 0;
            } else {
                driver.stuck_frames += 1;
                driver.stuck_peak = driver.stuck_peak.max(driver.stuck_frames);
            }
            if driver.stuck_frames >= REANCHOR_FRAMES
                && let Some(route) = driver.route.clone()
            {
                let gates: Vec<mm2_game::Checkpoint> = race
                    .map(|r| {
                        progress
                            .remaining()
                            .filter_map(|i| r.definition.checkpoints.get(i))
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();
                // The landing also keeps REANCHOR_CLEAR of every
                // participant (the stuck car's own spot included —
                // beached on the line it should not teleport back onto
                // itself) and of poses other re-anchors claimed this
                // frame — the spec's competing-recovery-positions edge
                // (F15-B.10). The check is positional, not avoidance:
                // it applies whatever the authored avoid flags say.
                let occupied = |p: Vec3| {
                    reanchor_occupied(
                        p,
                        traffic.iter().map(|t| t.pos).chain(claimed.iter().copied()),
                    )
                };
                let (mut pose, ryaw) = reanchor_pose(&route, driver.next, pos.0, yaw, |p| {
                    gates.iter().any(|g| {
                        let dx = p.x - g.center.x;
                        let dz = p.z - g.center.z;
                        dx * dx + dz * dz < g.radius * g.radius
                    }) || occupied(p)
                });
                // The same hull clearance the spawn applies.
                let hull_min_y = vehicle
                    .config
                    .collider_points
                    .as_ref()
                    .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
                    .unwrap_or(-vehicle.config.chassis_size[1] * 0.5);
                pose.y += (SPAWN_LIFT - hull_min_y).max(0.35);
                resets.write(ResetVehicle {
                    entity: Some(entity),
                    position: pose,
                    yaw: ryaw,
                });
                let resync_next = initial_route_index(&route, pose, ryaw);
                // The walk-back can land a closed-route car across
                // the route boundary — resync the traversal count so
                // the teleport cannot bank arc it did not drive.
                if let Some(line) = &mut line {
                    line.reanchor(&route, driver.next, pos.0, resync_next, pose);
                }
                driver.next = resync_next;
                driver.recovery = ScriptedBot {
                    escapes: driver.recovery.escapes,
                    ..ScriptedBot::default()
                };
                driver.pass_entity = None;
                driver.pass_side = 0.0;
                driver.clear_frames = 0;
                driver.stall_frames = 0;
                driver.stuck_frames = 0;
                driver.stuck_pos = pose;
                driver.reanchors += 1;
                driver.catch_up = 0.0;
                claimed.push(pose);
                info!(
                    vehicle = %driver.spec.vehicle,
                    reanchors = driver.reanchors,
                    "opponent re-anchored onto its route after a bounded stuck"
                );
                *input = VehicleInput::default();
                continue;
            }
        } else {
            driver.stuck_pos = pos.0;
            driver.stuck_frames = 0;
        }
        // The corridor reach is the authored look-ahead distance
        // (F15-B.8) — how far ahead this driver senses obstacles, not
        // a speed-scaled sightline.
        let reach = driver.look_ahead;
        // A banned blocker — one whose abandoned pass is still on its
        // cooldown — is fully transparent: no aim, no brake. That is
        // what lets the fallback actually push or slip past instead
        // of brake-shuffling at the same unyielding gap.
        let banned = |e: Entity| driver.pass_ban.is_some_and(|(b, _)| b == e);
        let narrow = nearest_blocker(entity, pos.0, fwd, reach, &traffic, |t| driver.senses(t))
            .filter(|b| !banned(b.entity));
        let held = driver
            .pass_entity
            .and_then(|e| held_blocker(e, pos.0, fwd, reach, &traffic))
            .filter(|b| !banned(b.entity));
        // A pass commits only when the blocker is an obstruction. A
        // standing car anywhere in the corridor is one — swinging
        // wide early can clear a whole knot where creeping to
        // bumpers only joins it; a moving one needs a genuine
        // closure — a matched-pace car ahead is a queue to sit in,
        // not a reason to leave the route: with a field sharing
        // `.opp` lines an always-on offset aim weaves every car
        // off-line all race.
        let engaging = narrow.is_some_and(|b| {
            vstate.forward_speed - b.speed > FOLLOW_RELEASE || b.speed < CRAWL_SPEED
        });
        let mut threat = held.or(if engaging { narrow } else { None });
        // A pass that cannot make progress cannot hold the offset aim
        // forever: PASS_STALL frames without PASS_STALL_DIST of
        // displacement abandons the commitment and bans the blocker
        // from re-committing for PASS_BAN, so the route line — push
        // or slip past — gets its turn (AC03's bounded response, not
        // a parked shuffle).
        match &threat {
            Some(b) => {
                if driver.stall_frames == 0 {
                    driver.stall_pos = pos.0;
                }
                driver.stall_frames += 1;
                if pos.0.distance(driver.stall_pos) >= PASS_STALL_DIST {
                    driver.stall_frames = 0;
                    driver.stall_pos = pos.0;
                } else if driver.stall_frames >= PASS_STALL {
                    driver.pass_ban = Some((b.entity, PASS_BAN));
                    driver.pass_entity = None;
                    driver.pass_side = 0.0;
                    driver.stall_frames = 0;
                    threat = None;
                }
            }
            None => driver.stall_frames = 0,
        }
        if let Some(b) = threat {
            if driver.pass_side == 0.0 {
                let right = Vec3::new(-fwd.z, 0.0, fwd.x);
                let t_rel = target - pos.0;
                let mut side = pick_pass_side(b.lat, t_rel.x * right.x + t_rel.z * right.z);
                // Room check: weight every car ahead or alongside by
                // how far into either lane it sits. If the picked
                // side is the busier one the commit aims into traffic
                // the corridor never saw — take the emptier side.
                let mut room = [0.0f32; 2];
                for t in &traffic {
                    if t.entity == entity || !driver.senses(t) {
                        continue;
                    }
                    let o = blocker_at(pos.0, fwd, t);
                    if o.gap <= -PASS_BEHIND || o.gap >= reach || o.lat.abs() >= PASS_SCAN {
                        continue;
                    }
                    room[usize::from(o.lat >= 0.0)] += 1.0 - o.lat.abs() / PASS_SCAN;
                }
                if room[usize::from(side > 0.0)] > room[usize::from(side <= 0.0)] {
                    side = -side;
                }
                driver.pass_side = side;
            }
            driver.pass_entity = Some(b.entity);
            driver.clear_frames = 0;
            // Aim down the offset lane ahead — a lateral shift of the
            // route anchor itself is a few-degree wiggle at chase
            // distances, never a pass. The lane direction is the
            // route leg's lateral, re-derived each frame, so the
            // offset lane bends with the road: a world-fixed vector
            // aims across corners, a nose-relative one orbits.
            let rd = target - pos.0;
            let rd_len = (rd.x * rd.x + rd.z * rd.z).sqrt();
            let (rx, rz) = if rd_len > 0.5 {
                (rd.x / rd_len, rd.z / rd_len)
            } else {
                (fwd.x, fwd.z)
            };
            let lane = Vec3::new(-rz, 0.0, rx) * driver.pass_side;
            target = pos.0 + fwd * PASS_LOOKAHEAD + lane * PASS_OFFSET;
        } else {
            driver.pass_entity = None;
            driver.clear_frames += 1;
            if driver.clear_frames >= PASS_RELEASE {
                driver.pass_side = 0.0;
            }
        }
        let bearing = relative_bearing(yaw, pos.0, target);
        // F15-B.4 — disclosed catch-up (designed, DSN-27): a driver
        // trailing the leader lifts its demand ceiling by the bounded
        // factor — never the leader's, never the player's, and never
        // progress itself, which still has to be earned through the
        // same swept triggers. `driver.catch_up` is the observable
        // the `cu=` record counts; it is not a re-anchor and stays
        // out of `reanchors`.
        let assist = match (race, leader) {
            (Some(r), Some(l)) => mm2_game::catch_up_factor(
                l - mm2_game::course_progress(&r.definition, progress, pos.0, leg_ref),
                &driver.catch_up_policy,
            ),
            _ => 0.0,
        };
        driver.catch_up = assist;
        let mut tuning = driver.tuning;
        if assist > 0.0 {
            tuning.throttle_cap = (tuning.throttle_cap + assist).min(1.0);
            tuning.corner_speed *= 1.0 + assist;
        }
        *input = scripted_input_tuned(
            &mut driver.recovery,
            bearing,
            vstate.forward_speed,
            vstate.grounded,
            &tuning,
        );
        // Braking answers the narrow corridor only: the committed pass
        // target may sit outside it while we are alongside, and braking
        // for it there would stall the overtake it is holding open.
        if let Some(b) = &narrow {
            apply_gap_brake(&mut input, b, vstate.forward_speed);
        }
    }
}

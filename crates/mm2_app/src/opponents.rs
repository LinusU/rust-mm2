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

/// Half-width of the brake corridor ahead (m) — about a car width:
/// a car sharing our line counts, one in the neighbouring lane does
/// not (test lanes sit 6 m apart; retail lanes are wider).
const BLOCK_HALF_WIDTH: f32 = 2.4;
/// Half-width (m) of the pass-room scan either side of the nose — the
/// far lane must be empty at commit, not just the corridor, so a
/// pass cannot aim into traffic the corridor never saw.
const PASS_SCAN: f32 = 7.0;
/// Corridor reach at a standstill (m); grows with speed.
const BLOCK_NEAR: f32 = 14.0;
/// Added corridor reach per m/s — look ahead ~1.4 s of travel.
const BLOCK_LEAD: f32 = 1.4;
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
                    pass_entity: None,
                    pass_side: 0.0,
                    clear_frames: 0,
                    stall_frames: 0,
                    stall_pos: Vec3::ZERO,
                    pass_ban: None,
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

/// Another participant on the road — the traffic the avoidance reads.
/// Positions, headings and speeds come from live physics state: the
/// local player and every other AI are obstacles exactly alike.
#[derive(Debug, Clone, Copy)]
pub struct Traffic {
    /// The participant's entity — self is excluded by comparison.
    pub entity: Entity,
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
/// new blocker.
pub fn nearest_blocker(
    entity: Entity,
    pos: Vec3,
    fwd: Vec3,
    reach: f32,
    traffic: &[Traffic],
) -> Option<Blocker> {
    traffic
        .iter()
        .filter(|t| t.entity != entity)
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
#[allow(clippy::type_complexity)]
pub fn opponent_drive(
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut set: ParamSet<(
        Query<(
            Entity,
            &mut VehicleInput,
            &Position,
            &Rotation,
            &VehicleState,
            &RaceProgress,
            &mut OpponentDriver,
        )>,
        Query<(Entity, &Position, &Rotation, &VehicleState), With<Player>>,
    )>,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    let traffic: Vec<Traffic> = set
        .p1()
        .iter()
        .map(|(entity, pos, rot, vstate)| Traffic {
            entity,
            pos: pos.0,
            fwd: rot.0 * Vec3::NEG_Z,
            speed: vstate.forward_speed,
        })
        .collect();
    for (entity, mut input, pos, rot, vstate, progress, mut driver) in &mut set.p0() {
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
        } else if let Some(route) = &driver.spec.route {
            let (next, target) = route_target(route, driver.next, pos.0);
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
            *input = VehicleInput::default();
            continue;
        };
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let reach = BLOCK_NEAR + vstate.forward_speed.max(0.0) * BLOCK_LEAD;
        // A banned blocker — one whose abandoned pass is still on its
        // cooldown — is fully transparent: no aim, no brake. That is
        // what lets the fallback actually push or slip past instead
        // of brake-shuffling at the same unyielding gap.
        let banned = |e: Entity| driver.pass_ban.is_some_and(|(b, _)| b == e);
        let narrow =
            nearest_blocker(entity, pos.0, fwd, reach, &traffic).filter(|b| !banned(b.entity));
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
                    if t.entity == entity {
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
        *input = scripted_input(
            &mut driver.recovery,
            bearing,
            vstate.forward_speed,
            vstate.grounded,
        );
        // Braking answers the narrow corridor only: the committed pass
        // target may sit outside it while we are alongside, and braking
        // for it there would stall the overtake it is holding open.
        if let Some(b) = &narrow {
            apply_gap_brake(&mut input, b, vstate.forward_speed);
        }
    }
}

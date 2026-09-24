//! Scripted course-follower for evidence runs (F12-C).
//!
//! The `--bot` driver steers the player vehicle at the *live* race
//! objective through the same [`VehicleInput`] component the
//! keyboard/gamepad mapping writes — so a headless run exercises the
//! production checkpoint/finish/result path (`advance_race` +
//! `RaceProgress::advance`) instead of idling or running straight off
//! the course. It is an evidence driver, not a gameplay feature: it
//! borrows one wired `.opp` driving line as a guide between gates
//! ([`ScriptedRoute`], F15-B.5 — an implementation choice; retail
//! assigns routes to AI opponents, never to the player), with no
//! opponent AI beyond that, just enough control to finish authored
//! courses the throttle-hold driver cannot.
//!
//! The objective the bot chases is the race's own: under `AnyOrder`
//! the earliest un-cleared gate in authored order (the waypoint rows
//! encode the intended route — chasing the RACE-6 arrow's *nearest*
//! pick would send the pursuit across the map into buildings), then
//! the armed finish trigger (RACE-7); under `Ordered` the
//! participant's `next` required gate. A proportional steer aims the
//! nose at it, with a corner brake band and a reverse-and-turn escape
//! when the car is stopped against something — city courses have
//! walls the straight line to a gate does not see.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    CheckpointRule, ObjectId, ObjectIdentity, OpponentRoster, OpponentRoute, ParticipantState,
    Player, PlayerVehicle, RaceDefinition, RaceProgress, RaceState, RecoveryEvent, Session,
    relative_bearing,
};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState};
use tracing::info;

use crate::opponents::{
    REANCHOR_DIST, REANCHOR_FRAMES, SPAWN_LIFT, initial_route_index, point_reached,
    reanchor_occupied, reanchor_pose, route_is_closed,
};

/// Presence enables the scripted driver: `--bot` inserts it, and
/// [`scripted_drive`] owns the player vehicle's [`VehicleInput`] while
/// it exists. A resource (not a CLI argument threaded everywhere) so
/// both the windowed app and the headless smoke gate the same system.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ScriptedDrive;

/// Per-vehicle controller state — a component on the car so session
/// teardown despawns it with everything else the session owns: a
/// restart cannot inherit a half-finished recovery or a stale stuck
/// timer.
#[derive(Component, Debug, Clone, Copy)]
pub struct ScriptedBot {
    /// Consecutive frames demanding drive while grounded and barely
    /// moving — the stuck detector.
    pub stuck_frames: u32,
    /// Frames left in the reverse half of an escape.
    pub reverse_frames: u32,
    /// Frames left in the forward full-lock half of an escape — the
    /// car backs off the wall, then turns away from it.
    pub turn_frames: u32,
    /// Which way the current escape turns (±1). Alternates each
    /// recovery so a car that keeps failing tries the other side —
    /// the nav line can't see walls, so escapes are blind guesses.
    pub recovery_side: f32,
    /// Three-point escapes fired this session — the observable count
    /// of the bounded recovery the law attempts (F15 req 6's tracked
    /// recovery actions; the smoke record surfaces it per driver).
    pub escapes: u32,
}

impl Default for ScriptedBot {
    fn default() -> Self {
        Self {
            stuck_frames: 0,
            reverse_frames: 0,
            turn_frames: 0,
            recovery_side: 1.0,
            escapes: 0,
        }
    }
}

/// An authored `.opp` driving line bound to the scripted player
/// (F15-B.5). Gates are still banked exclusively by the race's own
/// swept triggers — the route only replaces the straight-line aim
/// *between* gates, which leaves the road wherever the course bends,
/// climbs or descends (the retail `sf circuit:0` evidence stall: the
/// gate 1→2 straight line cuts off the elevated road's edge and the
/// car lands 20 m under the gate it still needs). Chosen at session
/// load by [`pick_bot_route`] as the wired route whose staging point
/// is nearest the player spawn; dormant unless [`ScriptedDrive`] is
/// present. A component on the vehicle, so session teardown/restart
/// drops it with everything else the session owns.
#[derive(Component, Debug, Clone)]
pub struct ScriptedRoute {
    /// The authored driving line — one roster entry's resolved `.opp`.
    pub route: OpponentRoute,
    /// The route anchor currently being chased — the same convention
    /// as `OpponentDriver::next`.
    pub next: usize,
    /// Displacement-window anchor for the bounded re-anchor — the
    /// position the car must leave by [`REANCHOR_DIST`] or the window
    /// fills. The same displacement-over-speed test `OpponentDriver`
    /// uses: a penned or beached car can roll and still never escape
    /// the bubble.
    pub reanchor_pos: Vec3,
    /// Frames spent inside [`REANCHOR_DIST`] of `reanchor_pos` while a
    /// route-guided objective is live — at [`REANCHOR_FRAMES`] the
    /// bounded re-anchor teleports the car back onto its chased leg.
    pub reanchor_frames: u32,
    /// Frames spent further than [`OFF_ROUTE_DIST`] from the route
    /// polyline — the fall-loop arm the displacement window cannot
    /// see: a car that dropped off an elevated road keeps moving >8 m
    /// per cycle, so the bubble never fills, but it sits tens of
    /// metres *below* the line the whole time. At
    /// [`OFF_ROUTE_FRAMES`] the same re-anchor fires.
    pub offroute_frames: u32,
    /// This car's water/OOB recoveries since the last banked gate —
    /// the third re-anchor arm. The generic recovery lands at the last
    /// *dry-grounded* pose, which on a descent pocket is the lip the
    /// car keeps falling off: nothing in it can lift the car back onto
    /// the line. [`RECOVERY_REANCHOR`] falls without a banked gate
    /// (progress resets the count) while still off the line means the
    /// loop is unwinnable — re-anchor instead.
    pub recoveries: u32,
    /// `RaceProgress::crossings` at the last observation — a change
    /// means the car banked a gate, which clears `recoveries`.
    pub progress_stamp: u32,
    /// Bounded re-anchors this session — the observable count, the
    /// same disclosure `OpponentDriver::reanchors` gives AI drivers.
    pub reanchors: u32,
}

impl ScriptedRoute {
    /// Bind `route` starting at the first anchor ahead of `pos`/`yaw`
    /// that is not already reached — the same rule an opponent applies
    /// at its staged spawn ([`initial_route_index`]).
    pub fn new(route: OpponentRoute, pos: Vec3, yaw: f32) -> Self {
        let next = initial_route_index(&route, pos, yaw);
        Self {
            route,
            next,
            reanchor_pos: pos,
            reanchor_frames: 0,
            offroute_frames: 0,
            recoveries: 0,
            progress_stamp: 0,
            reanchors: 0,
        }
    }
}

/// Pick the scripted driver's driving line out of the wired roster:
/// the resolved route whose authored staging point (`.opp` row 0) is
/// nearest the player spawn — every wired route covers the course, so
/// the pick is only which opponent's line to borrow. An implementation
/// choice, not an original rule: retail assigns routes to AI
/// opponents, never to the player. `None` when no entry resolved a
/// route — the bot then aims gate-to-gate as before.
pub fn pick_bot_route(roster: &OpponentRoster, spawn: Vec3) -> Option<OpponentRoute> {
    roster
        .entries
        .iter()
        .filter_map(|e| e.route.as_ref())
        .filter(|r| !r.points.is_empty())
        .min_by(|a, b| {
            let da = a.points[0].position - spawn;
            let db = b.points[0].position - spawn;
            (da.x * da.x + da.z * da.z).total_cmp(&(db.x * db.x + db.z * db.z))
        })
        .cloned()
}

/// Advance the route chase one frame and return the aim point.
/// `next` is the chased anchor (same convention as
/// [`crate::opponents::route_target`]); `gate` is the live race
/// objective the swept triggers still own. The chase may never run
/// past the gate:
///
/// - `gate_idx` — the anchor nearest the gate in 3-D (the authored
///   line passes through every trigger it must bank; opponents clear
///   gates the same way). Anchors before it are chased; reaching it
///   hands the aim to the gate itself.
/// - open route: `gate_idx <= next` means the gate's anchor is at or
///   behind the chase — the car missed the gate (fell below the
///   route, got knocked wide) and drives at it directly.
/// - closed route: the same in lap arithmetic — a gate anchor more
///   than half the loop *behind* the chase counts as missed; within
///   half a loop ahead it is still chased toward.
///
/// [`point_reached`] advancement is *bounded* at `gate_idx`: the XZ
/// perpendicular-plane test can skip anchors while the car sits off
/// the road's height (a fall under an elevated route), and running
/// past the objective's anchor would chase the course forever while
/// the gate stays un-cleared.
///
/// While chasing (`next` ahead of the gate's anchor) the aim is not
/// the raw anchor but the point [`ROUTE_LOOKAHEAD`] metres down the
/// polyline ([`lookahead_aim`]) — `.opp` anchors sit 40–200 m apart,
/// so aiming at the next one would snap the bearing only *after* the
/// anchor's plane passed; the lookahead turns with the road's bends
/// while they are still ahead, which is what lets the corner-brake
/// band scrub speed before an edge instead of after it.
///
/// Returns the updated `next` and this frame's aim point.
pub fn route_aim(route: &OpponentRoute, mut next: usize, pos: Vec3, gate: Vec3) -> (usize, Vec3) {
    let n = route.points.len();
    if n == 0 {
        return (0, gate);
    }
    if next >= n {
        next = n - 1;
    }
    let gate_idx = (0..n)
        .min_by(|&i, &j| {
            route.points[i]
                .position
                .distance_squared(gate)
                .total_cmp(&route.points[j].position.distance_squared(gate))
        })
        .unwrap_or(0);
    let closed = route_is_closed(route);
    let behind = if closed {
        (gate_idx + n - next) % n > n / 2
    } else {
        gate_idx < next
    };
    if behind {
        return (next, gate);
    }
    while next != gate_idx && point_reached(&route.points, next, pos) {
        next = if closed { (next + 1) % n } else { next + 1 };
    }
    if next == gate_idx {
        return (next, gate);
    }
    (
        next,
        lookahead_aim(route, next, gate_idx, closed, pos, gate),
    )
}

/// Aim distance (m) along the route polyline — about a second of
/// travel at the soft speed cap. Implementation choice for the
/// evidence driver; no original analog is claimed.
const ROUTE_LOOKAHEAD: f32 = 40.0;

/// 3-D distance (m) from the route polyline that still counts as "on
/// the line" — wide enough for the road's other lane or a spawn-grid
/// offset, narrow enough that the street under an elevated route reads
/// as off-route. Measured in 3-D so a leg crossing overhead does not
/// hide a fall.
const OFF_ROUTE_DIST: f32 = 12.0;
/// Frames (60 Hz) sustained off the route before the bounded re-anchor
/// fires — a jump apex or a bounced landing clears the count; a fall
/// loop onto a lower street does not (~5 s).
const OFF_ROUTE_FRAMES: u32 = 300;
/// Recovery events (water/OOB) since the last banked gate that arm the
/// third re-anchor leg — repeated falls with no progress mean the
/// generic recovery's last-grounded anchor cannot regain the line.
const RECOVERY_REANCHOR: u32 = 3;
/// How far under a re-anchor candidate the ground probe reaches —
/// deep enough to accept real road under the line, shallow enough to
/// reject the rooftops/void the retail descent leg crosses.
const GROUND_PROBE: f32 = 10.0;

/// The car's shortest distance to the route polyline — the nearest
/// point over every leg (closed routes include the wrap leg), measured
/// in 3-D so a car parked under or over the line is not mistaken for
/// being on it. `f32::MAX` for an empty route.
pub fn route_distance(route: &OpponentRoute, pos: Vec3) -> f32 {
    let n = route.points.len();
    if n == 0 {
        return f32::MAX;
    }
    if n == 1 {
        return route.points[0].position.distance(pos);
    }
    let legs = if route_is_closed(route) { n } else { n - 1 };
    (0..legs)
        .map(|i| {
            let a = route.points[i].position;
            let b = route.points[(i + 1) % n].position;
            let ab = b - a;
            let t = ((pos - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t).distance(pos)
        })
        .fold(f32::MAX, f32::min)
}

/// The point on the route [`ROUTE_LOOKAHEAD`] metres of polyline ahead
/// of `pos`, walking from chased anchor `i` toward `gate_idx` — the
/// anchor itself when the leg to it is longer, an interpolated point
/// down-leg when the window spans several anchors, and the objective
/// `gate` when it lies inside the window (or `remain` metres toward
/// it when it sits just outside — the aim never passes the gate).
/// `i` starts strictly ahead of `gate_idx` in route order, so the
/// walk always reaches it; heights follow the polyline only through
/// the anchor `y`s the walk lands on.
fn lookahead_aim(
    route: &OpponentRoute,
    mut i: usize,
    gate_idx: usize,
    closed: bool,
    pos: Vec3,
    gate: Vec3,
) -> Vec3 {
    let n = route.points.len();
    let mut from = pos;
    let mut remain = ROUTE_LOOKAHEAD;
    loop {
        let at_gate_leg = i == gate_idx;
        let target = if at_gate_leg {
            gate
        } else {
            route.points[i].position
        };
        let dx = target.x - from.x;
        let dz = target.z - from.z;
        let d = dx.hypot(dz);
        if at_gate_leg || d <= remain {
            if at_gate_leg && d > remain {
                return from + Vec3::new(dx / d * remain, 0.0, dz / d * remain);
            }
            if at_gate_leg {
                return gate;
            }
            // The rest of this leg fits in the window — keep walking.
            remain -= d;
            from = route.points[i].position;
            i = if closed { (i + 1) % n } else { i + 1 };
            continue;
        }
        // The window ends inside this leg — interpolate onto it.
        return from + Vec3::new(dx / d * remain, 0.0, dz / d * remain);
    }
}

/// Radians of bearing → normalized steering. Positive bearing is to the
/// driver's right ([`relative_bearing`]) and positive steering is right
/// ([`VehicleInput::steering`]), so the signs line up directly.
const STEER_GAIN: f32 = 1.5;

/// Full throttle while the target is inside this |bearing| cone.
const STRAIGHT_RAD: f32 = 0.35;
/// Reduced throttle out to here; beyond it the crawl band applies.
const TURN_RAD: f32 = 1.0;
/// Corner brake: above this bearing *and* [`CORNER_SPEED`] the car
/// brakes instead of throttling, so it can rotate into the turn.
const CORNER_RAD: f32 = 1.1;
/// Speed (m/s) above which the corner brake engages.
const CORNER_SPEED: f32 = 16.0;
/// Soft speed cap (m/s): coast above it so the car does not out-run its
/// own steering on long straights. `vpbug` peaks ~37 m/s flat out.
const SPEED_CAP: f32 = 30.0;
/// |forward speed| below which the car counts as not moving (m/s).
const STUCK_SPEED: f32 = 1.5;
/// Frames (60 Hz updates) of grounded no-motion with throttle demanded
/// before a recovery starts — about 1.5 s.
const STUCK_FRAMES: u32 = 90;
/// Recovery reverse length in frames — about 1.2 s of backing off.
const REVERSE_FRAMES: u32 = 72;
/// Recovery forward-turn length in frames — about 0.7 s of full lock
/// after the reverse, turning the nose away from the wall.
const TURN_FRAMES: u32 = 45;

/// Per-driver tuning of the control law (F15-B.2). The player
/// evidence bot always uses [`ScriptedTuning::DEFAULT`]; opponents
/// resolve theirs from the authored `[Opponent]` parameter tail, so
/// authored skill shows up as throttle demand and carried corner
/// speed rather than a different car or a different law.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedTuning {
    /// Ceiling applied to every throttle demand — the authored
    /// `maxThrottle` column (default 1.0 = unchanged).
    pub throttle_cap: f32,
    /// Speed (m/s) above which a sharp bearing brakes instead of
    /// throttling — [`CORNER_SPEED`] scaled by the authored
    /// corner-speed multiplier (default = `CORNER_SPEED`).
    pub corner_speed: f32,
}

impl ScriptedTuning {
    /// The shared constants — what every driver used before authored
    /// tuning existed.
    pub const DEFAULT: Self = Self {
        throttle_cap: 1.0,
        corner_speed: CORNER_SPEED,
    };
}

impl Default for ScriptedTuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn steer_cmd(bearing: f32) -> f32 {
    (bearing * STEER_GAIN).clamp(-1.0, 1.0)
}

/// One frame of the control law under the driver's [`ScriptedTuning`]:
/// `corner_speed` sets the corner-brake engage speed and
/// `throttle_cap` ceilings every throttle demand, recovery included —
/// an authored `maxThrottle` of 0.8 limits the car everywhere, not
/// just on straights.
///
/// The stuck recovery is a blind three-point escape: reverse while
/// swinging the nose toward `recovery_side` (reversing pivots the nose
/// opposite the steer, hence `-recovery_side`), then drive forward at
/// full lock the same way. When the bearing already points somewhere
/// useful the reverse steers `-steer_cmd` instead — straight at the
/// target. `recovery_side` alternates between escapes.
pub fn scripted_input_tuned(
    bot: &mut ScriptedBot,
    bearing: f32,
    forward_speed: f32,
    grounded: bool,
    tuning: &ScriptedTuning,
) -> VehicleInput {
    if bot.reverse_frames > 0 {
        bot.reverse_frames -= 1;
        let steer = -steer_cmd(bearing);
        return VehicleInput {
            brake: 1.0,
            steering: if steer.abs() >= 0.5 {
                steer
            } else {
                -bot.recovery_side
            },
            ..default()
        };
    }
    if bot.turn_frames > 0 {
        bot.turn_frames -= 1;
        return VehicleInput {
            throttle: 0.5_f32.min(tuning.throttle_cap),
            steering: bot.recovery_side,
            ..default()
        };
    }
    let mut input = VehicleInput {
        steering: steer_cmd(bearing),
        ..default()
    };
    let b = bearing.abs();
    if b > CORNER_RAD && forward_speed > tuning.corner_speed {
        input.brake = 0.6;
    } else if forward_speed < SPEED_CAP {
        let band: f32 = if b <= STRAIGHT_RAD {
            1.0
        } else if b <= TURN_RAD {
            0.45
        } else {
            0.25
        };
        input.throttle = band.min(tuning.throttle_cap);
    }
    if grounded && input.throttle > 0.0 && forward_speed.abs() < STUCK_SPEED {
        bot.stuck_frames += 1;
        if bot.stuck_frames >= STUCK_FRAMES {
            bot.stuck_frames = 0;
            bot.reverse_frames = REVERSE_FRAMES;
            bot.turn_frames = TURN_FRAMES;
            bot.recovery_side = -bot.recovery_side;
            bot.escapes += 1;
        }
    } else {
        bot.stuck_frames = 0;
    }
    input
}

/// One frame of the control law at [`ScriptedTuning::DEFAULT`].
/// `bearing` is the signed angle to the target (positive = right),
/// `forward_speed` the signed speed along the nose (m/s), `grounded`
/// whether any wheel has contact.
pub fn scripted_input(
    bot: &mut ScriptedBot,
    bearing: f32,
    forward_speed: f32,
    grounded: bool,
) -> VehicleInput {
    scripted_input_tuned(
        bot,
        bearing,
        forward_speed,
        grounded,
        &ScriptedTuning::DEFAULT,
    )
}

/// The world position the bot drives at — the live objective, never a
/// memorised route. `AnyOrder` follows the *earliest* un-cleared gate
/// in authored order: the waypoint rows encode the intended course
/// sequence, so chasing the next row keeps the pursuit on the roads
/// between consecutive gates — the RACE-6 arrow's "nearest" pick is
/// the player's freedom, but a scripted route that ping-pongs across
/// the map just drives into buildings. Gates clipped out of order
/// still clear through the same swept path. Once every gate is
/// cleared the armed finish trigger (RACE-7) is the target. `Ordered`
/// aims at the next required gate in authored order. `None` when the
/// definition has no current objective — e.g. an `Ordered`
/// participant whose `next` ran past the gate list — which the caller
/// treats as "stop".
pub fn drive_target(definition: &RaceDefinition, progress: &RaceProgress) -> Option<Vec3> {
    match definition.rule {
        CheckpointRule::AnyOrder => progress.remaining().next().map_or_else(
            || definition.finish.as_ref().map(|c| c.center),
            |i| Some(definition.checkpoints[i].center),
        ),
        CheckpointRule::Ordered => definition.checkpoints.get(progress.next).map(|c| c.center),
    }
}

/// Write [`VehicleInput`] on the player vehicle from the live race
/// objective every frame. Scheduled `after` [`crate::input::vehicle_input`]
/// and gated on [`ScriptedDrive`], so while `--bot` is on the bot
/// deterministically owns the input — and while it is off this system
/// never runs.
///
/// Honors the same gates the keyboard path does: the session must be
/// `Playing` and the race countdown's `input_locked` holds the car
/// still (anything else writes a zeroed input so the car rolls to a
/// stop rather than holding stale controls). Once the participant
/// resolves — finished or timed out — the bot yields and the car
/// coasts. Cruise sessions have no race objective; the bot drives a
/// point far ahead of the nose, which is the throttle-hold driver's
/// straight line with the same recovery.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub fn scripted_drive(
    mut commands: Commands,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut recoveries_in: Option<MessageReader<RecoveryEvent>>,
    spatial: Option<SpatialQuery>,
    mut cars: Query<
        (
            Entity,
            &ObjectIdentity,
            &mut VehicleInput,
            &Position,
            &Rotation,
            &Vehicle,
            &VehicleState,
            Option<&RaceProgress>,
            Option<&mut ScriptedBot>,
            Option<&mut ScriptedRoute>,
        ),
        With<PlayerVehicle>,
    >,
    participants: Query<&Position, With<Player>>,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    // The frame-start participant snapshot the re-anchor's occupancy
    // leg reads — every `Player`-marked participant (the scripted car
    // itself included, so a beached one does not teleport back onto
    // itself), the same set `opponent_drive`'s `traffic` holds
    // (F15-B.11 parity).
    let occupants: Vec<Vec3> = participants.iter().map(|p| p.0).collect();
    // Poses a re-anchor already claimed this frame — the snapshot
    // predates the loop and cannot see a same-frame teleport.
    let mut claimed: Vec<Vec3> = Vec::new();
    // Drain once per frame — recovery events are rare and per-object.
    let mut recovery_hits: Vec<ObjectId> = Vec::new();
    if let Some(reader) = recoveries_in.as_mut() {
        for event in reader.read() {
            if event.generation == session.generation() {
                recovery_hits.push(event.object);
            }
        }
    }
    for (entity, id, mut input, pos, rot, vehicle, vstate, progress, bot, mut bot_route) in
        &mut cars
    {
        let racing = progress.is_some_and(|p| {
            matches!(
                p.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            )
        });
        // The live objective — a race gate — or, for a cruise session,
        // the straight-line point far ahead of the nose.
        let gate = if !session.is_playing() || locked {
            None
        } else if let Some(race) = race {
            if racing {
                progress.and_then(|p| drive_target(&race.definition, p))
            } else {
                None
            }
        } else {
            Some(pos.0 + rot.0 * Vec3::NEG_Z * 200.0)
        };
        let Some(gate) = gate else {
            *input = VehicleInput::default();
            continue;
        };
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let mut fresh = ScriptedBot::default();
        let bot = match bot {
            Some(b) => b.into_inner(),
            None => {
                commands.entity(entity).insert(ScriptedBot::default());
                &mut fresh
            }
        };
        let mut target = gate;
        if let Some(rs) = bot_route.as_mut() {
            // The bounded last resort — the same disclosed contract
            // `opponent_drive` holds (`reanchors`/`Teleported`/`blocked`
            // walk-back). The reverse-and-turn escapes answer ordinary
            // blocks, but a car that fell off an elevated route onto a
            // lower street can never climb back: its last-grounded
            // respawns stay below and the straight gate aim just walks
            // it off the same edge forever. Two windows share the one
            // [`ResetVehicle`] teleport: [`REANCHOR_FRAMES`] without
            // [`REANCHOR_DIST`] of displacement (penned/beached), or
            // [`OFF_ROUTE_FRAMES`] spent further than [`OFF_ROUTE_DIST`]
            // from the polyline (a fall loop — each cycle moves too far
            // for the bubble to fill, but the car is metres under the
            // line throughout). The swept segment breaks, so the jump
            // banks nothing, and the walk-back keeps the landing out of
            // un-cleared triggers, off occupied participant poses
            // (F15-B.11 parity with `opponent_drive`) and over ground
            // the probe can see.
            if session.authority_role().is_authority() && pos.0.is_finite() {
                if pos.0.distance(rs.reanchor_pos) >= REANCHOR_DIST {
                    rs.reanchor_pos = pos.0;
                    rs.reanchor_frames = 0;
                } else {
                    rs.reanchor_frames += 1;
                }
                if route_distance(&rs.route, pos.0) > OFF_ROUTE_DIST {
                    rs.offroute_frames += 1;
                } else {
                    rs.offroute_frames = 0;
                }
                // A banked gate is progress: it clears the fall count.
                let crossings = progress.map(|p| p.crossings).unwrap_or(0);
                if crossings != rs.progress_stamp {
                    rs.progress_stamp = crossings;
                    rs.recoveries = 0;
                }
                rs.recoveries += recovery_hits.iter().filter(|o| **o == id.0).count() as u32;
                if rs.reanchor_frames >= REANCHOR_FRAMES
                    || rs.offroute_frames >= OFF_ROUTE_FRAMES
                    || (rs.recoveries >= RECOVERY_REANCHOR
                        && route_distance(&rs.route, pos.0) > OFF_ROUTE_DIST)
                {
                    let gates: Vec<mm2_game::Checkpoint> = race
                        .zip(progress)
                        .map(|(r, p)| {
                            p.remaining()
                                .filter_map(|i| r.definition.checkpoints.get(i))
                                .copied()
                                .collect()
                        })
                        .unwrap_or_default();
                    // The walk-back also rejects poses with no collider
                    // within GROUND_PROBE below — the authored line is
                    // car-height sampling, not a ground promise, and on
                    // the retail descent it interpolates over a rooftop
                    // gap where a teleport lands the car in free fall.
                    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
                    let supported = |p: Vec3| {
                        spatial.as_ref().is_none_or(|sq| {
                            sq.cast_ray(p + Vec3::Y, Dir3::NEG_Y, GROUND_PROBE, true, &filter)
                                .is_some()
                        })
                    };
                    // The same occupancy leg `opponent_drive` holds
                    // (F15-B.11 parity with F15-B.10): the landing keeps
                    // REANCHOR_CLEAR of every participant — the scripted
                    // car's own stuck spot included — and of poses a
                    // re-anchor already claimed this frame.
                    let occupied = |p: Vec3| {
                        reanchor_occupied(
                            p,
                            occupants.iter().copied().chain(claimed.iter().copied()),
                        )
                    };
                    let (mut pose, ryaw) = reanchor_pose(&rs.route, rs.next, pos.0, yaw, |p| {
                        gates.iter().any(|g| {
                            let dx = p.x - g.center.x;
                            let dz = p.z - g.center.z;
                            dx * dx + dz * dz < g.radius * g.radius
                        }) || !supported(p)
                            || occupied(p)
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
                    rs.next = initial_route_index(&rs.route, pose, ryaw);
                    *bot = ScriptedBot {
                        escapes: bot.escapes,
                        ..ScriptedBot::default()
                    };
                    rs.reanchor_pos = pose;
                    rs.reanchor_frames = 0;
                    rs.offroute_frames = 0;
                    rs.recoveries = 0;
                    rs.reanchors += 1;
                    claimed.push(pose);
                    info!(
                        reanchors = rs.reanchors,
                        "scripted player re-anchored onto its route after a bounded stuck"
                    );
                    *input = VehicleInput::default();
                    continue;
                }
            } else {
                rs.reanchor_pos = pos.0;
                rs.reanchor_frames = 0;
                rs.offroute_frames = 0;
                rs.recoveries = 0;
            }
            let (next, aim) = route_aim(&rs.route, rs.next, pos.0, gate);
            rs.next = next;
            target = aim;
        }
        let bearing = relative_bearing(yaw, pos.0, target);
        *input = scripted_input(bot, bearing, vstate.forward_speed, vstate.grounded);
    }
}

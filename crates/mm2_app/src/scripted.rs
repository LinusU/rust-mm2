//! Scripted course-follower for evidence runs (F12-C).
//!
//! The `--bot` driver steers the player vehicle at the *live* race
//! objective through the same [`VehicleInput`] component the
//! keyboard/gamepad mapping writes — so a headless run exercises the
//! production checkpoint/finish/result path (`advance_race` +
//! `RaceProgress::advance`) instead of idling or running straight off
//! the course. It is an evidence driver, not a gameplay feature: it
//! uses an explicit diagnostic `.opp` guide, or borrows one wired
//! `.opp` driving line, as a guide between gates
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
use mm2_game::gold::{GoldState, GoldView};
use mm2_game::{Banger, BangerFragment};
use mm2_game::{
    CheckpointRule, Mm2Vfs, NavGraph, ObjectId, ObjectIdentity, OpponentRoster, OpponentRoute,
    OpponentRoutePoint, ParticipantState, Player, PlayerId, PlayerVehicle, RaceDefinition,
    RaceProgress, RaceState, RecoveryEvent, RouteOptions, Session, relative_bearing,
};
use mm2_vehicle::{ResetVehicle, Vehicle, VehicleInput, VehicleState};
use tracing::info;

use crate::cnr::{CnrHost, participant_id};
use crate::cnrhud::match_view;
use crate::cnrnet::CnrReplica;
use crate::netdrive::{NetPlayer, RemoteSnaps};
use crate::racing_line::{
    CORNER_BRAKE_DEFAULT, CarLimits, RouteCursor, SpeedPlan, corner_aim_distance, pace, plan_speed,
    sharp_corner_plan, steer_toward,
};
use mm2_game::cnr_options::DELIVERY_RADIUS_M;

use crate::opponents::{
    REANCHOR_DIST, REANCHOR_FRAMES, aim_distance, initial_route_index, point_reached,
    reanchor_lift, reanchor_occupied, reanchor_pose_with_progress, route_is_closed, seat_reanchor,
    spacing_point_reached,
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
    /// Frames to wait before the planner is asked again after it found
    /// no road line to the objective ([`NAV_REPLAN_FRAMES`]).
    pub plan_wait: u32,
    /// Escapes fired since the nav guide was last planned. A car that
    /// keeps failing them is wedged off the line (a ledge above the road,
    /// say), so [`GUIDE_REPLAN_ESCAPES`] of them drop the guide and the
    /// route is planned again from where the car really is.
    pub escapes_since_plan: u32,
    /// Consecutive frames spent over or under the objective — inside its
    /// radius on the ground plane, out of it in 3D. The pickup and the
    /// delivery are 3D triggers, so a car on the road above the gate is
    /// at the objective as far as the map is concerned and never takes
    /// it ([`OFF_LEVEL_FRAMES`]).
    pub off_level_frames: u32,
    /// Where the wedged-guide replan last fired and how many replans in
    /// a row started within [`SAME_POSE_DIST`] of it. The planner is
    /// deterministic, so a replan from the same pose draws the same
    /// line and the same wedge; [`SAME_POSE_REPLANS`] of them switch
    /// the guide off for [`UNGUIDED_FRAMES`].
    pub replan_at: Option<Vec3>,
    pub same_pose_replans: u32,
    /// Frames left with the nav guide off — the car aims straight at its
    /// objective and answers blocks with the ordinary escapes, which
    /// approach the obstacle from another angle than the failed line.
    pub unguided_frames: u32,
}

impl ScriptedBot {
    /// Record a wedged-guide replan from `pos`. Returns whether the
    /// replans have repeated from one pose, so the guide should be left
    /// off for a while instead of planned yet again.
    pub fn note_replan(&mut self, pos: Vec3) -> bool {
        if self
            .replan_at
            .is_some_and(|at| at.distance(pos) <= SAME_POSE_DIST)
        {
            self.same_pose_replans += 1;
        } else {
            self.same_pose_replans = 1;
        }
        self.replan_at = Some(pos);
        if self.same_pose_replans >= SAME_POSE_REPLANS {
            self.same_pose_replans = 0;
            self.unguided_frames = UNGUIDED_FRAMES;
            true
        } else {
            false
        }
    }
}

/// Distance within which two wedged-guide replans count as the same pose.
const SAME_POSE_DIST: f32 = 15.0;
/// Same-pose replans after which the guide is left off.
const SAME_POSE_REPLANS: u32 = 2;
/// Frames (60 Hz, ~10 s) the guide stays off after repeated same-pose
/// replans. Bounded: the guide returns from wherever the car got to.
const UNGUIDED_FRAMES: u32 = 600;

impl Default for ScriptedBot {
    fn default() -> Self {
        Self {
            stuck_frames: 0,
            reverse_frames: 0,
            turn_frames: 0,
            recovery_side: 1.0,
            escapes: 0,
            escapes_since_plan: 0,
            plan_wait: 0,
            off_level_frames: 0,
            replan_at: None,
            same_pose_replans: 0,
            unguided_frames: 0,
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
    /// Driving line from the explicit evidence guide or a resolved roster `.opp`.
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
    /// The objective a nav-planned route ([`plan_nav_route`]) was built
    /// for — a Cops & Robbers target; the driver re-plans when the
    /// objective moves. `None` on an authored/explicit guide.
    pub goal: Option<Vec3>,
    /// Handling-derived planner limits, cached for this session-owned car.
    limits: Option<CarLimits>,
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
            goal: None,
            limits: None,
        }
    }
}

/// The session city's routing graph, loaded once per city and kept in
/// the driver's local state — a networked Cops & Robbers session fields
/// no ambient traffic ([`crate::traffic::fields_ambient_traffic`]), so
/// the traffic resource's graph is not there to borrow. `None` for a
/// non-city world or a city whose aimap does not load (the driver then
/// aims straight, as the cruise bot does).
fn routing_graph<'a>(
    cache: &'a mut Option<(String, Option<NavGraph>)>,
    vfs: Option<&Mm2Vfs>,
    session: &Session,
) -> Option<&'a NavGraph> {
    let mm2_game::WorldMode::City { psdl } = &session.config()?.world else {
        return None;
    };
    let city = crate::net::city_stem(psdl)?;
    if cache.as_ref().is_none_or(|(c, _)| c != city) {
        let graph = vfs.and_then(|v| mm2_content::load_routing_nav_graph(&v.0, city).ok());
        *cache = Some((city.to_string(), graph.map(|b| b.graph)));
    }
    cache.as_ref().and_then(|(_, g)| g.as_ref())
}

/// Arc-length step, metres, between the samples of a nav-planned route.
const NAV_ROUTE_STEP: f32 = 6.0;

/// Frames a driver waits after the planner found no road line before it
/// asks again — the car has moved by then, so the endpoints may connect;
/// asking every frame would re-run the router for nothing.
const NAV_REPLAN_FRAMES: u32 = 120;

/// Whether a car's nav-planned guide needs (re)planning for `objective`:
/// it has none, or the one it has was built for an objective that has
/// since moved more than 2 m (a delivery, a drop, a new round).
fn guide_stale(guide: Option<&ScriptedRoute>, objective: Vec3) -> bool {
    guide.is_none_or(|rs| rs.goal.is_some_and(|g| g.distance(objective) > 2.0))
}

/// Failed escapes after which a nav-planned guide is dropped and planned
/// again from the car's current pose. The line cannot see walls, so a
/// wedged car otherwise chases the same `route_next` forever.
const GUIDE_REPLAN_ESCAPES: u32 = 4;

/// Frames a car may circle over or under its objective before its guide
/// is dropped and planned again from where it really is (~10 s: a car
/// crossing the objective's disc on the right level is gone in a few
/// seconds, and one on the wrong level is on a road that does not lead
/// down to it).
const OFF_LEVEL_FRAMES: u32 = 600;

/// Whether `pos` is over or under `objective`: inside its trigger radius
/// on the ground plane but out of it in 3D.
fn over_objective(pos: Vec3, objective: Vec3) -> bool {
    (pos.x - objective.x).hypot(pos.z - objective.z) <= DELIVERY_RADIUS_M
        && pos.distance(objective) > DELIVERY_RADIUS_M
}

/// What one frame of Cops & Robbers re-planning did.
enum NavReplan {
    /// A previous failure's wait is still running; the router was not asked.
    Waiting,
    /// The planner found a road line; bind it as the car's guide.
    Planned(ScriptedRoute),
    /// No road line; the wait is armed again.
    Failed,
}

/// One frame of re-planning for a car whose guide is stale: the planner
/// runs only once `wait` has run out, and a failure re-arms the wait to
/// [`NAV_REPLAN_FRAMES`] so a missing connection costs one router query
/// per window, not one per frame.
fn nav_replan(wait: &mut u32, plan: impl FnOnce() -> Option<ScriptedRoute>) -> NavReplan {
    if *wait > 0 {
        *wait -= 1;
        return NavReplan::Waiting;
    }
    match plan() {
        Some(route) => NavReplan::Planned(route),
        None => {
            *wait = NAV_REPLAN_FRAMES;
            NavReplan::Failed
        }
    }
}

/// A driving line over the city's shared road graph from `from` to
/// `to` — the F09 route query sampled into the same [`OpponentRoute`]
/// shape the authored guides use, so the bounded re-anchor and the
/// handling-derived pace apply unchanged. Only an evidence driver
/// plans this (a Cops & Robbers objective has no authored line; the
/// straight line to a gold site crosses buildings). `None` when the
/// graph cannot connect the endpoints on the objective's own level
/// ([`crate::cnr_bot_drive::GOAL_LEVEL`]: a Cops & Robbers pickup and
/// bank are 3D triggers, so a flyover lane above the bank leads the
/// car to a place it cannot deliver from) — the caller falls back to the
/// straight aim rather than inventing connectivity.
pub fn plan_nav_route(graph: &NavGraph, from: Vec3, to: Vec3) -> Option<OpponentRoute> {
    let line = graph
        .drive_line_at_level(
            from,
            to,
            &RouteOptions::default(),
            NAV_ROUTE_STEP,
            crate::cnr_bot_drive::GOAL_LEVEL,
        )
        .ok()?;
    (line.len() >= 2).then(|| OpponentRoute {
        points: line
            .into_iter()
            .map(|position| OpponentRoutePoint {
                position,
                brake: 0.0,
                forward_offset: 0.0,
                side_offset: 0.0,
                target_speed: 0.0,
                speed_start: 0.0,
                side_start: 0.0,
            })
            .collect(),
    })
}

/// Load an explicit diagnostic guide without wiring a fake opponent. Unlike
/// retail roster parsing, evidence input is strict: malformed rows cannot be
/// silently skipped, every field must be finite, and every XZ leg must advance.
pub fn load_bot_route(path: &std::path::Path) -> Result<OpponentRoute, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read bot route {}: {e}", path.display()))?;
    let file = mm2_formats::opp::OppFile::parse(&text)
        .map_err(|e| format!("invalid bot route {}: {e}", path.display()))?;
    if let Some(problem) = file.diagnostics.first() {
        return Err(format!(
            "invalid bot route {} at line {}: {}",
            path.display(),
            problem.line,
            problem.message
        ));
    }
    if file.rows.len() < 2 {
        return Err(format!(
            "invalid bot route {}: need at least two points",
            path.display()
        ));
    }
    let mut points = Vec::with_capacity(file.rows.len());
    for row in file.rows {
        if !row
            .position
            .iter()
            .chain([
                &row.brake,
                &row.forward_offset,
                &row.side_offset,
                &row.target_speed,
                &row.speed_start,
                &row.side_start,
            ])
            .all(|v| v.is_finite())
        {
            return Err(format!(
                "invalid bot route {} at line {}: non-finite field",
                path.display(),
                row.line
            ));
        }
        points.push(mm2_game::OpponentRoutePoint {
            position: Vec3::from_array(row.position),
            brake: row.brake,
            forward_offset: row.forward_offset,
            side_offset: row.side_offset,
            target_speed: row.target_speed,
            speed_start: row.speed_start,
            side_start: row.side_start,
        });
    }
    if points.windows(2).any(|pair| {
        let delta = pair[1].position - pair[0].position;
        let length = delta.x.hypot(delta.z);
        !length.is_finite() || length <= 0.0
    }) {
        return Err(format!(
            "invalid bot route {}: every consecutive XZ edge must have positive finite length",
            path.display()
        ));
    }
    Ok(OpponentRoute { points })
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
pub fn route_aim(route: &OpponentRoute, next: usize, pos: Vec3, gate: Vec3) -> (usize, Vec3) {
    let closed = route_is_closed(route);
    route_aim_at_distance(route, next, pos, gate, ROUTE_LOOKAHEAD, false, closed)
}

/// Guided evidence driving uses the AI planner's speed-scaled pursuit distance
/// and projection onto the route, while retaining the player's gate boundary.
pub fn planned_route_aim(
    route: &OpponentRoute,
    next: usize,
    pos: Vec3,
    gate: Vec3,
    speed: f32,
) -> (usize, Vec3) {
    let closed = route_is_closed(route);
    route_aim_at_distance(route, next, pos, gate, aim_distance(speed), true, closed)
}

/// [`planned_route_aim`] for a nav-planned line ([`plan_nav_route`]),
/// which is a one-way trip to its objective and never a circuit. Its
/// endpoints are not apart by construction: a road line that climbs a
/// ramp and comes back round can end within [`route_is_closed`]'s
/// 40 m of where it began (the sf upper deck, 25 m above its bank), and
/// reading that as a lap made the objective "behind" the car, which
/// then drove straight at it off the deck's edge.
pub fn planned_open_route_aim(
    route: &OpponentRoute,
    next: usize,
    pos: Vec3,
    gate: Vec3,
    speed: f32,
) -> (usize, Vec3) {
    route_aim_at_distance(route, next, pos, gate, aim_distance(speed), true, false)
}

fn route_aim_at_distance(
    route: &OpponentRoute,
    mut next: usize,
    pos: Vec3,
    gate: Vec3,
    distance: f32,
    project: bool,
    closed: bool,
) -> (usize, Vec3) {
    let n = route.points.len();
    if n == 0 {
        return (0, gate);
    }
    if next >= n {
        next = n - 1;
    }
    // A road can occur more than once in a legitimate itinerary. Equal
    // geometric matches must follow route progress, not the first visit.
    let progress_distance = |i: usize| {
        if i >= next {
            i - next
        } else if closed {
            n - next + i
        } else {
            n + next - i
        }
    };
    let gate_idx = (0..n)
        .min_by(|&i, &j| {
            route.points[i]
                .position
                .distance_squared(gate)
                .total_cmp(&route.points[j].position.distance_squared(gate))
                .then_with(|| progress_distance(i).cmp(&progress_distance(j)))
        })
        .unwrap_or(0);
    let behind = if closed {
        (gate_idx + n - next) % n > n / 2
    } else {
        gate_idx < next
    };
    if behind {
        return (next, gate);
    }
    while next != gate_idx
        && if project {
            spacing_point_reached(route, next, pos)
        } else {
            point_reached(&route.points, next, pos)
        }
    {
        next = if closed { (next + 1) % n } else { next + 1 };
    }
    if next == gate_idx {
        return (next, gate);
    }
    let distance = if project {
        RouteCursor::locate(route, next, pos).map_or(distance, |cursor| {
            corner_aim_distance(route, cursor, distance)
        })
    } else {
        distance
    };
    (
        next,
        lookahead_aim(
            route,
            next,
            gate_idx,
            closed,
            if project {
                RouteCursor::locate(route, next, pos).map_or(pos, |c| c.point_ahead(route, 0.0))
            } else {
                pos
            },
            gate,
            distance,
        ),
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
    distance: f32,
) -> Vec3 {
    let n = route.points.len();
    let mut from = pos;
    let mut remain = distance;
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

pub(crate) fn steer_cmd(bearing: f32) -> f32 {
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
    if let Some(input) = recovery_input(bot, bearing, tuning) {
        return input;
    }
    let mut input = VehicleInput {
        steering: steer_cmd(bearing),
        ..default()
    };
    let b = bearing.abs();
    if b > CORNER_RAD && forward_speed > tuning.corner_speed {
        input.brake = 0.6;
    } else if forward_speed < SPEED_CAP {
        input.throttle = bearing_throttle(bearing).min(tuning.throttle_cap);
    }
    watch_stuck(bot, &input, forward_speed, grounded);
    input
}

/// Throttle the bearing allows: full while the target is ahead, eased
/// as it swings wide so the car can rotate rather than push straight.
pub(crate) fn bearing_throttle(bearing: f32) -> f32 {
    let b = bearing.abs();
    if b <= STRAIGHT_RAD {
        1.0
    } else if b <= TURN_RAD {
        0.45
    } else {
        0.25
    }
}

/// The input of an escape in progress — reverse, then the forward turn
/// — or `None` when no escape is running and the driving law decides.
pub(crate) fn recovery_input(
    bot: &mut ScriptedBot,
    bearing: f32,
    tuning: &ScriptedTuning,
) -> Option<VehicleInput> {
    if bot.reverse_frames > 0 {
        bot.reverse_frames -= 1;
        let steer = -steer_cmd(bearing);
        return Some(VehicleInput {
            brake: 1.0,
            steering: if steer.abs() >= 0.5 {
                steer
            } else {
                -bot.recovery_side
            },
            ..default()
        });
    }
    if bot.turn_frames > 0 {
        bot.turn_frames -= 1;
        return Some(VehicleInput {
            throttle: 0.5_f32.min(tuning.throttle_cap),
            steering: bot.recovery_side,
            ..default()
        });
    }
    None
}

/// Start the reverse-and-turn manoeuvre on purpose — a driver whose
/// target is behind it and has no room or speed to arc round, as
/// opposed to one that [`watch_stuck`] found wedged. `toward` is the
/// signed bearing to the target: the reverse swings the nose toward it
/// and the forward turn that follows takes the same side. Not counted
/// as an escape (`bot.escapes` is stuck evidence).
pub(crate) fn begin_turnaround(bot: &mut ScriptedBot, toward: f32) {
    bot.reverse_frames = REVERSE_FRAMES;
    bot.turn_frames = TURN_FRAMES;
    bot.recovery_side = if toward < 0.0 { -1.0 } else { 1.0 };
}

/// Count grounded frames spent demanding throttle without moving and
/// arm the reverse-and-turn escape once [`STUCK_FRAMES`] accrue —
/// alternating its side each time.
pub(crate) fn watch_stuck(
    bot: &mut ScriptedBot,
    input: &VehicleInput,
    forward_speed: f32,
    grounded: bool,
) {
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

/// Where a Cops & Robbers match wants `me` to drive: the gold while it
/// lies free, the car's own side's delivery marker once `me` carries it
/// (F27 evidence driver). `None` when the match is decided, `me` is not
/// seated, or someone else carries the gold — the bot has no pursuit
/// rule, so it coasts rather than chase a carrier it cannot ram on
/// purpose. The same [`GoldView`] a HUD reads, so a client's bot follows
/// the host's replica exactly as its readout does.
pub fn cnr_target(view: &GoldView, me: PlayerId) -> Option<Vec3> {
    if view.outcome.is_some() {
        return None;
    }
    match view.state {
        GoldState::Carried { by } if by == me => {
            let side = view.standings.iter().find(|s| s.player == me)?.side;
            Some(view.sites.target(side.delivery_target()))
        }
        GoldState::Carried { .. } => None,
        GoldState::Resting { at } | GoldState::Dropped { at, .. } => {
            view.standings.iter().any(|s| s.player == me).then_some(at)
        }
    }
}

/// Where a `--bot-thief` driver wants to drive: the carrier while someone
/// else holds the gold (`carrier_at` is that car's pose), nowhere while
/// the gold lies untouched at its site — the thief lets the others go
/// first — and [`cnr_target`]'s answer otherwise (a dropped gold, or
/// its own carry to the delivery marker).
pub fn thief_target(view: &GoldView, me: PlayerId, carrier_at: Option<Vec3>) -> Option<Vec3> {
    match view.state {
        GoldState::Resting { .. } if view.outcome.is_none() => None,
        GoldState::Carried { by } if by != me && view.outcome.is_none() => carrier_at,
        _ => cnr_target(view, me),
    }
}

/// How close (m) a `--bot-thief` must be to its carrier to stop following
/// the road graph and drive straight through it. The road line ends at
/// the carrier, and a planned route brakes into its last point; a ram
/// has to arrive fast enough to cross the dislodge severity.
const RAM_RANGE: f32 = 40.0;

/// How far (m) past the carrier the thief aims, so the throttle stays
/// down through the contact instead of easing off as the target nears.
const RAM_OVERSHOOT: f32 = 25.0;

/// The point a thief within [`RAM_RANGE`] of the `carrier` drives at:
/// beyond it, on the line from the thief. `None` when the carrier is too
/// far for a straight run (the road-graph plan chases it instead).
fn ram_point(from: Vec3, carrier: Vec3) -> Option<Vec3> {
    let gap = Vec3::new(carrier.x - from.x, 0.0, carrier.z - from.z);
    (gap.length() < RAM_RANGE).then(|| carrier + gap.normalize_or_zero() * RAM_OVERSHOOT)
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
    bodies: Query<&RigidBody>,
    colliders: Query<&ColliderOf>,
    water: Option<Res<crate::water::CityWater>>,
    cnr_host: Option<Res<CnrHost>>,
    cnr_replica: Option<Res<CnrReplica>>,
    mut probe: WedgeProbe,
    vfs: Option<Res<Mm2Vfs>>,
    mut nav: Local<Option<(String, Option<NavGraph>)>>,
    mut cars: Query<
        (
            Entity,
            &ObjectIdentity,
            &Player,
            Option<&NetPlayer>,
            &mut VehicleInput,
            &Position,
            &Rotation,
            &LinearVelocity,
            &Vehicle,
            &VehicleState,
            Option<&RaceProgress>,
            Option<&mut ScriptedBot>,
            Option<&mut ScriptedRoute>,
        ),
        With<PlayerVehicle>,
    >,
    participants: Query<(&Player, Option<&NetPlayer>, &Position)>,
) {
    let thief = session.config().is_some_and(|c| c.dev.bot_thief);
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    // The frame-start participant snapshot the re-anchor's occupancy
    // leg reads — every `Player`-marked participant (the scripted car
    // itself included, so a beached one does not teleport back onto
    // itself), the same set `opponent_drive`'s `traffic` holds
    // (F15-B.11 parity).
    let occupants: Vec<Vec3> = participants.iter().map(|(_, _, p)| p.0).collect();
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
    // A Cops & Robbers match is the objective when no race runs: the
    // gold, then the delivery marker (F27 evidence driver).
    let cnr_view = match_view(cnr_host.as_deref(), cnr_replica.as_deref());
    for (
        entity,
        id,
        player,
        net,
        mut input,
        pos,
        rot,
        lin_vel,
        vehicle,
        vstate,
        progress,
        bot,
        mut bot_route,
    ) in &mut cars
    {
        let racing = progress.is_some_and(|p| {
            matches!(
                p.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            )
        });
        // The live objective — a race gate — or, for a cruise session,
        // the straight-line point far ahead of the nose.
        let mut ramming = false;
        let gate = if !session.is_playing() || locked {
            None
        } else if let Some(race) = race {
            if racing {
                progress.and_then(|p| drive_target(&race.definition, p))
            } else {
                None
            }
        } else if let Some(view) = &cnr_view {
            let me = participant_id(player, net);
            if thief {
                let carrier_at = match view.state {
                    GoldState::Carried { by } => participants
                        .iter()
                        .find(|(p, n, _)| participant_id(p, *n) == by)
                        .map(|(_, _, at)| at.0),
                    _ => None,
                };
                let aim = thief_target(view, me, carrier_at);
                match carrier_at.and_then(|c| ram_point(pos.0, c)) {
                    Some(through) if aim == carrier_at => {
                        ramming = true;
                        Some(through)
                    }
                    _ => aim,
                }
            } else {
                cnr_target(view, me)
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
        let new_bot = bot.is_none();
        let bot = match bot {
            Some(b) => b.into_inner(),
            None => {
                commands.entity(entity).insert(ScriptedBot::default());
                &mut fresh
            }
        };
        // A Cops & Robbers objective is planned over the road graph
        // and bound as the car's guide; the plan holds until the
        // objective moves (a delivery, a drop, a new round).
        if ramming && bot_route.is_some() {
            commands.entity(entity).remove::<ScriptedRoute>();
            bot_route = None;
        }
        if bot.escapes_since_plan >= GUIDE_REPLAN_ESCAPES
            && bot_route.as_deref().is_some_and(|rs| rs.goal.is_some())
        {
            commands.entity(entity).remove::<ScriptedRoute>();
            bot_route = None;
            bot.escapes_since_plan = 0;
            let unguided = bot.note_replan(pos.0);
            info!(
                tick = session.tick(),
                position = ?pos.0,
                gate = ?gate,
                unguided,
                "scripted player dropped its wedged nav guide to replan"
            );
        }
        let guide_off = bot.unguided_frames > 0;
        bot.unguided_frames = bot.unguided_frames.saturating_sub(1);
        bot.off_level_frames =
            if race.is_none() && cnr_view.is_some() && !ramming && over_objective(pos.0, gate) {
                bot.off_level_frames + 1
            } else {
                0
            };
        if bot.off_level_frames >= OFF_LEVEL_FRAMES
            && bot_route.as_deref().is_some_and(|rs| rs.goal.is_some())
        {
            commands.entity(entity).remove::<ScriptedRoute>();
            bot_route = None;
            bot.off_level_frames = 0;
            info!(
                tick = session.tick(),
                position = ?pos.0,
                gate = ?gate,
                "scripted player dropped its nav guide after circling over or under its objective"
            );
        }
        if race.is_none()
            && cnr_view.is_some()
            && !ramming
            && !guide_off
            && guide_stale(bot_route.as_deref(), gate)
        {
            let replan = nav_replan(&mut bot.plan_wait, || {
                let graph = routing_graph(&mut nav, vfs.as_deref(), &session)?;
                let route = plan_nav_route(graph, pos.0, gate)?;
                let mut guide = ScriptedRoute::new(route, pos.0, yaw);
                // The line starts at the car, so its early anchors are the
                // way out of wherever the car stands, not a staging
                // overshoot to skip: a nose facing away from them must
                // turn round to them, not chase the first anchor it
                // faces — which on the sf hill is the far side of a wall.
                guide.next = 0;
                guide.goal = Some(gate);
                Some(guide)
            });
            match replan {
                NavReplan::Waiting => {}
                NavReplan::Planned(guide) => {
                    commands.entity(entity).insert(guide);
                }
                NavReplan::Failed => {
                    // No road line: retry later, and stop chasing a guide
                    // built for an objective that is no longer the target
                    // (the car aims straight, as the cruise bot does).
                    if new_bot {
                        // The component is not on the car yet; the throwaway
                        // value the wait was armed on would be lost.
                        commands.entity(entity).insert(ScriptedBot {
                            plan_wait: bot.plan_wait,
                            ..default()
                        });
                    }
                    if bot_route.is_some() {
                        commands.entity(entity).remove::<ScriptedRoute>();
                        bot_route = None;
                    }
                }
            }
        }
        let mut target = gate;
        let mut planned = None;
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
            //
            // A nav-planned Cops & Robbers guide (`goal` set) never
            // re-anchors: the teleport lands on the route's chased
            // leg, which can be the objective itself — a delivery the
            // car did not drive. The bot only counts as evidence for a
            // match it drove, so a stuck car keeps the ordinary
            // reverse-and-turn escapes and the match stands or falls
            // on them.
            if rs.goal.is_none() && session.authority_role().is_authority() && pos.0.is_finite() {
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
                    // The walk-back also rejects poses with no static,
                    // dry ground under the level car — the authored line
                    // is car-height sampling, not a ground promise, and
                    // on the retail descent it interpolates over a
                    // rooftop gap where a teleport lands the car in free
                    // fall. The ground found sets the landing height.
                    let lift = reanchor_lift(&vehicle.config);
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
                    let (pose, ryaw, resync_next) =
                        reanchor_pose_with_progress(&rs.route, rs.next, pos.0, yaw, |p| {
                            gates.iter().any(|g| {
                                let dx = p.x - g.center.x;
                                let dz = p.z - g.center.z;
                                dx * dx + dz * dz < g.radius * g.radius
                            }) || occupied(p)
                                || (spatial.is_some() && seat(p, yaw).is_none())
                        });
                    // Without a physics world the authored height plus
                    // clearance stands.
                    let pose = seat(pose, ryaw).unwrap_or(pose + Vec3::Y * lift);
                    resets.write(ResetVehicle {
                        entity: Some(entity),
                        position: pose,
                        yaw: ryaw,
                    });
                    rs.next = resync_next;
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
                        tick = session.tick(),
                        reanchors = rs.reanchors,
                        position = ?pos.0,
                        speed = vstate.forward_speed,
                        heading_rad = yaw,
                        route_next = rs.next,
                        gate = ?gate,
                        landing = ?pose,
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
            let speed = vstate.forward_speed;
            let (next, aim) = if rs.goal.is_some() {
                planned_open_route_aim(&rs.route, rs.next, pos.0, gate, speed)
            } else {
                planned_route_aim(&rs.route, rs.next, pos.0, gate, speed)
            };
            rs.next = next;
            target = aim;
            let limits = *rs
                .limits
                .get_or_insert_with(|| CarLimits::of(&vehicle.config, None));
            let plan = RouteCursor::locate(&rs.route, next, pos.0).map_or(
                SpeedPlan {
                    limit: f32::INFINITY,
                    demand: 0.0,
                },
                |cursor| {
                    let mut plan = plan_speed(&rs.route, cursor, speed, &limits);
                    let sharp = sharp_corner_plan(&rs.route, cursor, speed, &limits);
                    plan.limit = plan.limit.min(sharp.limit);
                    plan.demand = plan.demand.max(sharp.demand);
                    plan
                },
            );
            planned = Some((limits, plan));
        }
        let bearing = relative_bearing(yaw, pos.0, target);
        let escapes_before = bot.escapes;
        *input = if let Some((limits, plan)) = planned {
            planned_input(
                bot,
                bearing,
                (target.x - pos.0.x).hypot(target.z - pos.0.z),
                vstate.forward_speed,
                vstate.grounded,
                &limits,
                plan,
            )
        } else {
            scripted_input(bot, bearing, vstate.forward_speed, vstate.grounded)
        };
        if bot.escapes > escapes_before {
            bot.escapes_since_plan += bot.escapes - escapes_before;
            info!(
                tick = session.tick(),
                escapes = bot.escapes,
                position = ?pos.0,
                speed = vstate.forward_speed,
                heading_rad = yaw,
                bearing_rad = bearing,
                route_next = ?bot_route.as_ref().map(|rs| rs.next),
                gate = ?gate,
                aim = ?target,
                "scripted player started a bounded escape after sustained low-speed throttle"
            );
            if bot.escapes % WEDGE_DUMP_EVERY == 0 && *probe.dumps < WEDGE_DUMPS_MAX {
                *probe.dumps += 1;
                let host = if cnr_host.is_some() {
                    "host"
                } else if cnr_replica.is_some() {
                    "client"
                } else {
                    "offline"
                };
                let host_copy = probe
                    .snaps
                    .as_deref()
                    .and_then(|s| s.own_host_copy())
                    .filter(|_| net.is_some() || cnr_replica.is_some());
                dump_wedge(
                    spatial.as_ref(),
                    &probe.aabbs,
                    &probe.props,
                    &probe.nearby,
                    &colliders,
                    entity,
                    host,
                    (pos.0, rot.0, lin_vel.0),
                    vehicle,
                    vstate,
                    host_copy,
                    session.tick(),
                    bot.escapes,
                );
            }
        }
        if let Some(limit) = session.config().and_then(|c| c.dev.bot_speed) {
            limit_evidence_speed(&mut input, vstate.forward_speed, limit);
        }
    }
}

/// A prop's phase, fragment marker and name, as the wedge dump reads it.
type PropData = (
    &'static Banger,
    Option<&'static BangerFragment>,
    Option<&'static Name>,
);

/// [`PropData`] plus where the prop is and whether it has a collider.
type NearbyPropData = (
    &'static Banger,
    Option<&'static BangerFragment>,
    Option<&'static Name>,
    &'static GlobalTransform,
    Has<Collider>,
);

/// What the wedge dump reads beyond the driver's own queries.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WedgeProbe<'w, 's> {
    snaps: Option<Res<'w, RemoteSnaps>>,
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    props: Query<'w, 's, PropData>,
    nearby: Query<'w, 's, NearbyPropData>,
    dumps: Local<'s, u32>,
}

/// Every this-many bounded escapes of one seat fire the wedge dump.
const WEDGE_DUMP_EVERY: u32 = 3;

/// Wedge dumps one process prints, so a long wedge cannot flood the log.
const WEDGE_DUMPS_MAX: u32 = 8;

/// Metres of margin around the chassis box in which the wedge dump
/// lists colliders.
const WEDGE_REACH: f32 = 2.0;

/// Metres from the car (or the host's copy of it) within which the wedge
/// dump lists every prop, collider or not.
const PROP_REACH: f32 = 8.0;

/// One-shot diagnostic (#2231) for a seat that keeps escaping at one
/// spot: what the car overlaps within [`WEDGE_REACH`], its velocity and
/// wheel contacts, which side of the link this process is, and for a
/// client the pose the host last asserted for the seat. Fires only at
/// the 3rd, 6th... escape, so it is free on the ordinary path.
#[allow(clippy::too_many_arguments)]
fn dump_wedge(
    spatial: Option<&SpatialQuery>,
    aabbs: &Query<&ColliderAabb>,
    props: &Query<PropData>,
    nearby: &Query<NearbyPropData>,
    colliders: &Query<&ColliderOf>,
    entity: Entity,
    side: &str,
    (pos, rot, vel): (Vec3, Quat, Vec3),
    vehicle: &Vehicle,
    vstate: &VehicleState,
    host_copy: Option<(u64, Vec3, Vec3)>,
    tick: u64,
    escapes: u32,
) {
    let half = Vec3::from(vehicle.config.chassis_size) * 0.5 + Vec3::splat(WEDGE_REACH);
    let overlaps: Vec<String> = spatial
        .map(|sq| {
            let filter = SpatialQueryFilter::from_excluded_entities([entity]);
            sq.shape_intersections(
                &Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                pos,
                rot,
                &filter,
            )
            .into_iter()
            .map(|hit| {
                let body = colliders.get(hit).map_or(hit, |c| c.body);
                let (min, max) = aabbs
                    .get(hit)
                    .map_or((Vec3::NAN, Vec3::NAN), |a| (a.min, a.max));
                let prop = props.get(body).map_or_else(
                    |_| String::new(),
                    |(banger, fragment, name)| {
                        format!(
                            " prop={:?} phase={:?} fragment={}",
                            name.map(Name::as_str),
                            banger.phase,
                            fragment.is_some()
                        )
                    },
                );
                format!("{hit:?} body={body:?} aabb={min:?}..{max:?}{prop}")
            })
            .collect()
        })
        .unwrap_or_default();
    let wheels: Vec<String> = vstate
        .wheels
        .iter()
        .map(|w| {
            format!(
                "{}@{:.2}/{:?}",
                if w.grounded { "G" } else { "-" },
                w.compression,
                w.contact_entity
            )
        })
        .collect();
    let delta = host_copy.map(|(t, p, v)| (t, p, v, pos.distance(p)));
    // Every prop within reach of this process's car or of the host's
    // copy of it, whether or not it has a collider: the host and the
    // client dumps of one wedge are compared line by line (#2242).
    let centres = [Some(pos), host_copy.map(|(_, p, _)| p)];
    let props_near: Vec<String> = nearby
        .iter()
        .filter(|(.., gt, _)| {
            let at = gt.translation();
            centres
                .iter()
                .flatten()
                .any(|c| c.distance(at) <= PROP_REACH)
        })
        .map(|(banger, fragment, name, gt, has_collider)| {
            format!(
                "{:?} phase={:?} fragment={} collider={} at={:?}",
                name.map(Name::as_str),
                banger.phase,
                fragment.is_some(),
                has_collider,
                gt.translation()
            )
        })
        .collect();
    info!(
        target: "wedge",
        tick,
        escapes,
        side,
        position = ?pos,
        velocity = ?vel,
        forward_speed = vstate.forward_speed,
        grounded = vstate.grounded,
        wheels = ?wheels,
        overlaps = overlaps.len(),
        colliders = ?overlaps,
        props_near = ?props_near,
        host_copy = ?delta,
        "wedge dump at a repeated bounded escape"
    );
}

/// Reuse the AI's handling-derived steering and curvature pace for a guided
/// player. Recovery remains the existing bounded scripted policy; this helper
/// cannot bank gates or reset a vehicle.
pub fn planned_input(
    bot: &mut ScriptedBot,
    bearing: f32,
    aim_dist: f32,
    speed: f32,
    grounded: bool,
    limits: &CarLimits,
    plan: SpeedPlan,
) -> VehicleInput {
    if let Some(input) = recovery_input(bot, bearing, &ScriptedTuning::DEFAULT) {
        return input;
    }
    let throttle = if speed < SPEED_CAP {
        bearing_throttle(bearing)
    } else {
        0.0
    };
    let (throttle, brake) = pace(speed, plan, throttle, CORNER_BRAKE_DEFAULT);
    let input = VehicleInput {
        steering: steer_toward(bearing, aim_dist, speed, limits),
        throttle,
        brake,
        ..default()
    };
    watch_stuck(bot, &input, speed, grounded);
    input
}

/// An explicit probe ceiling changes inputs only, preserving the shared handling.
fn limit_evidence_speed(input: &mut VehicleInput, speed: f32, limit: f32) {
    if speed > limit {
        input.throttle = 0.0;
        input.brake = input.brake.max(0.35);
    }
}

#[cfg(test)]
mod speed_limit_tests {
    use super::*;
    #[test]
    fn speed_ceiling_brakes_without_changing_steering() {
        let mut input = VehicleInput {
            throttle: 1.0,
            steering: -0.5,
            ..default()
        };
        limit_evidence_speed(&mut input, 12.0, 8.0);
        assert_eq!(input.throttle, 0.0);
        assert_eq!(input.brake, 0.35);
        assert_eq!(input.steering, -0.5);
    }
    #[test]
    fn reverse_recovery_is_preserved() {
        let mut input = VehicleInput {
            brake: 1.0,
            steering: 1.0,
            ..default()
        };
        limit_evidence_speed(&mut input, -5.0, 8.0);
        assert_eq!(input.brake, 1.0);
        assert_eq!(input.steering, 1.0);
    }
}

#[cfg(test)]
mod cnr_target_tests {
    use super::*;
    use mm2_game::gold::{CnrVariant, EndReason, EndRule, Outcome, Side, Sites, Standing, Winner};

    const ME: PlayerId = PlayerId(1);
    const OTHER: PlayerId = PlayerId(2);

    fn view(variant: CnrVariant, state: GoldState, me_side: Side) -> GoldView {
        let row = |player, side| Standing {
            player,
            side,
            score: 0,
            connected: true,
        };
        GoldView {
            generation: 1,
            variant,
            end: EndRule::None,
            round: 0,
            revision: 1,
            elapsed: 0,
            state,
            sites: Sites {
                gold: Vec3::new(1.0, 0.0, 0.0),
                hideout: Vec3::new(0.0, 0.0, 20.0),
                bank: Vec3::new(0.0, 0.0, 30.0),
            },
            outcome: None,
            standings: vec![row(ME, me_side), row(OTHER, Side::Cops)],
        }
    }

    /// Replans from one pose repeat the same line, so the second one
    /// switches the guide off; a replan from elsewhere starts over.
    #[test]
    fn repeated_same_pose_replans_leave_the_guide_off() {
        let mut bot = ScriptedBot::default();
        let wedge = Vec3::new(-1242.0, 85.5, 146.0);
        assert!(!bot.note_replan(wedge));
        assert!(!bot.note_replan(wedge + Vec3::new(100.0, 0.0, 0.0)));
        assert!(!bot.note_replan(wedge));
        assert!(bot.note_replan(wedge + Vec3::X * 3.0));
        assert_eq!(bot.unguided_frames, UNGUIDED_FRAMES);
        assert_eq!(bot.same_pose_replans, 0);
    }

    /// The thief lets the gold lie until someone takes it, chases the
    /// carrier, takes what falls, and stops once the match is decided.
    #[test]
    fn the_thief_waits_chases_and_recovers() {
        let at = Vec3::new(5.0, 1.0, 5.0);
        let carrier = Some(Vec3::new(40.0, 0.0, 9.0));
        let v = |state| view(CnrVariant::FreeForAll, state, Side::Solo);
        assert_eq!(
            thief_target(&v(GoldState::Resting { at }), ME, carrier),
            None
        );
        let carried = v(GoldState::Carried { by: OTHER });
        assert_eq!(thief_target(&carried, ME, carrier), carrier);
        assert_eq!(thief_target(&carried, ME, None), None);
        let dropped = v(GoldState::Dropped {
            at,
            by: Some(OTHER),
            free_at: 0,
        });
        assert_eq!(thief_target(&dropped, ME, None), Some(at));
        // Its own carry goes to its marker, as for any bot.
        let mine = v(GoldState::Carried { by: ME });
        assert_eq!(thief_target(&mine, ME, None), cnr_target(&mine, ME));
        let mut decided = v(GoldState::Carried { by: OTHER });
        decided.outcome = Some(Outcome {
            reason: EndReason::PointLimit,
            winner: Winner::Player(OTHER),
            at_tick: 9,
        });
        assert_eq!(thief_target(&decided, ME, carrier), None);
    }

    /// The straight run through a carrier starts inside [`RAM_RANGE`]
    /// and aims past it, level with the ground it drives on.
    #[test]
    fn the_ram_aims_past_a_near_carrier_only() {
        let from = Vec3::new(0.0, 5.0, 0.0);
        let near = Vec3::new(20.0, 9.0, 0.0);
        let through = ram_point(from, near).expect("inside the range");
        assert!(
            (through.x - (20.0 + RAM_OVERSHOOT)).abs() < 1e-4,
            "{through}"
        );
        assert_eq!(through.y, near.y);
        assert_eq!(ram_point(from, Vec3::new(RAM_RANGE + 1.0, 5.0, 0.0)), None);
        // A carrier on top of the thief has no direction to aim past.
        assert_eq!(ram_point(from, from), Some(from));
    }

    /// Free gold is the objective, whether it rests or was dropped.
    #[test]
    fn free_gold_is_the_target() {
        let at = Vec3::new(5.0, 1.0, 5.0);
        for state in [
            GoldState::Resting { at },
            GoldState::Dropped {
                at,
                by: Some(OTHER),
                free_at: 0,
            },
        ] {
            let v = view(CnrVariant::FreeForAll, state, Side::Solo);
            assert_eq!(cnr_target(&v, ME), Some(at));
        }
    }

    /// A carrier heads for its own side's marker: robbers the hideout,
    /// cops the bank.
    #[test]
    fn a_carrier_heads_for_its_sides_marker() {
        let carried = GoldState::Carried { by: ME };
        let robber = view(CnrVariant::CopsVsRobbers, carried, Side::Robbers);
        assert_eq!(cnr_target(&robber, ME), Some(robber.sites.hideout));
        let cop = view(CnrVariant::CopsVsRobbers, carried, Side::Cops);
        assert_eq!(cnr_target(&cop, ME), Some(cop.sites.bank));
    }

    /// The bot has no pursuit rule: gold in someone else's car, a
    /// decided match and an unseated car all leave it with no target.
    #[test]
    fn nothing_to_chase_leaves_no_target() {
        let other = view(
            CnrVariant::FreeForAll,
            GoldState::Carried { by: OTHER },
            Side::Solo,
        );
        assert_eq!(cnr_target(&other, ME), None);

        let mut decided = view(
            CnrVariant::FreeForAll,
            GoldState::Resting { at: Vec3::ZERO },
            Side::Solo,
        );
        decided.outcome = Some(Outcome {
            reason: EndReason::PointLimit,
            winner: Winner::Player(ME),
            at_tick: 9,
        });
        assert_eq!(cnr_target(&decided, ME), None);

        let resting = view(
            CnrVariant::FreeForAll,
            GoldState::Resting { at: Vec3::ZERO },
            Side::Solo,
        );
        assert_eq!(cnr_target(&resting, PlayerId(9)), None);
        let carried_by_stranger = view(
            CnrVariant::FreeForAll,
            GoldState::Carried { by: PlayerId(9) },
            Side::Solo,
        );
        assert_eq!(cnr_target(&carried_by_stranger, PlayerId(9)), None);
    }

    #[test]
    fn a_car_on_the_road_above_the_objective_is_over_it() {
        let gate = Vec3::new(-1850.0, 25.6, -331.0);
        assert!(over_objective(gate + Vec3::new(3.0, 22.0, 0.0), gate));
        assert!(!over_objective(gate + Vec3::new(3.0, 2.0, 0.0), gate));
        assert!(!over_objective(gate + Vec3::new(40.0, 22.0, 0.0), gate));
    }

    /// A guide is replanned when it is missing or its objective moved
    /// more than 2 m; an authored guide (no goal) is never replanned.
    #[test]
    fn a_guide_is_stale_only_when_its_objective_moved() {
        let objective = Vec3::new(10.0, 0.0, 10.0);
        let guide = |goal| {
            let mut g = ScriptedRoute::new(OpponentRoute::default(), Vec3::ZERO, 0.0);
            g.goal = goal;
            g
        };
        assert!(guide_stale(None, objective));
        assert!(!guide_stale(Some(&guide(Some(objective))), objective));
        assert!(!guide_stale(
            Some(&guide(Some(objective + Vec3::X))),
            objective
        ));
        assert!(guide_stale(
            Some(&guide(Some(objective + Vec3::X * 3.0))),
            objective
        ));
        assert!(!guide_stale(Some(&guide(None)), objective));
    }

    /// A failed plan costs one router query per window: the planner is not
    /// asked again until [`NAV_REPLAN_FRAMES`] frames have passed.
    #[test]
    fn a_failed_plan_waits_a_window_before_the_router_is_asked_again() {
        let mut wait = 0;
        let mut asked = 0;
        let mut frame = |wait: &mut u32, ok: bool| {
            nav_replan(wait, || {
                asked += 1;
                ok.then(|| ScriptedRoute::new(OpponentRoute::default(), Vec3::ZERO, 0.0))
            })
        };
        assert!(matches!(frame(&mut wait, false), NavReplan::Failed));
        assert_eq!(wait, NAV_REPLAN_FRAMES);
        for _ in 0..NAV_REPLAN_FRAMES {
            assert!(matches!(frame(&mut wait, true), NavReplan::Waiting));
        }
        assert_eq!(wait, 0);
        // Only the failing frame has asked so far; the next one asks again.
        assert!(matches!(frame(&mut wait, true), NavReplan::Planned(_)));
        assert_eq!(wait, 0);
        assert_eq!(asked, 2);
    }
}

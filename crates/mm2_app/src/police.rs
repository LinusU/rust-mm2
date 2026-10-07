//! Single-player police on the road (F20-A.2): the event's authored
//! `[Police]` lineup fielded as real, session-owned cars.
//!
//! [`spawn_police`] turns each [`PoliceSpec`] of the session's
//! [`PoliceRoster`] (the F20-A.1 content producer) into a physics car:
//! the authored vehicle through the same VFS → `load_opponent` →
//! [`equip_authored_vehicle`] path the racing opponents use (the
//! `aud/cardata/opponent` audio side, the authored damage/smoke/spark/
//! stuck/break records), stamped with the session's `SessionEntity`,
//! `ObjectIdentity` and authority role so teardown, replication-by-
//! authority and damage all treat it like any other simulated car.
//!
//! A cop is deliberately **not** a [`Player`](mm2_game::Player): it has
//! no `RaceProgress`, never appears in the standings, the opponent
//! indicator or the result ledger, and is not a network client.
//!
//! A fielded cop stands at its authored position and heading with the
//! handbrake held until [`police_pursuit`] (F20-A.3) sees a target: the
//! pure [`Pursuit`] machine (`mm2_game::police`) turns what the cop
//! can see into detect → engage → pursue → lost, and [`chase_input`]
//! drives the car toward the target (or, once it has lost contact, the
//! last place it saw it). The pursuit rules are an **enhanced policy**
//! — the retail ones are unverified (ledger COP-4 / UNK-9) — and the
//! drive is road-aware (F20-B.2, ledger COP-11): out of the target's
//! sight, or beyond [`CHASE_DIRECT_RANGE`](mm2_game::CHASE_DIRECT_RANGE),
//! the cop follows a line over the city's routing graph ([`PoliceNav`])
//! to the last place it saw the target, through each cop's
//! [`ChaseNav`]; with no graph, no line, or an unreachable goal it aims
//! straight at the goal as before, counted rather than hidden. While a
//! cop is pursuing it carries [`EmergencyLights`] (F20-B.1): its
//! light-bar flares flash and its siren program plays, both read off
//! that one component (ledger COP-10). The
//! heading's unit and
//! zero axis are inferred (the vehicle-yaw convention — forward
//! `(−sin h, −cos h)` — every other authored heading uses); a road-lane
//! comparison of the Cruise rows was inconclusive (ledger COP-8).
//!
//! Single-player only: like the AI opponents (MP-4) police are not
//! replicated, so a networked session fields none.

use avian3d::prelude::{Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    ChaseMode, ChaseNav, DamageSignals, EmergencyLights, NavGraph, ObjectIdentity,
    ParticipantState, Player, PlayerControl, PlayerVehicle, PoliceRoster, PoliceSpec, Pursuit,
    PursuitEvent, PursuitPhase, PursuitPolicy, RaceProgress, Session, SessionAuthority,
    SessionEntity, Sighting, relative_bearing,
};
use mm2_vehicle::{VehicleInput, VehicleState, vehicle_bundle};
use tracing::{info, warn};

use crate::opponents::{equip_authored_vehicle, reanchor_lift};
use crate::racing_line::{CarLimits, steer_toward};
use crate::scripted::{
    ScriptedBot, ScriptedTuning, bearing_throttle, begin_turnaround, recovery_input, watch_stuck,
};

/// One fielded police car — the authored row it came from.
#[derive(Component, Debug, Clone)]
pub struct PoliceCar {
    /// Index into the roster the car was spawned from (authored order).
    pub index: usize,
    /// The authored `[Police]` row, verbatim.
    pub spec: PoliceSpec,
}

/// What [`spawn_police`] did with the roster: the denominator and every
/// way a row can fail to become a car. Session-scoped (inserted at
/// load, removed at teardown) — the smoke record's `police=` reads it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct PoliceFleet {
    /// Authored `[Police]` rows in the roster.
    pub authored: usize,
    /// Cars spawned.
    pub spawned: usize,
    /// Rows refused because the position or heading is unusable
    /// ([`PoliceSpec::placeable`]).
    pub unplaceable: usize,
    /// Rows whose vehicle did not load; the authored slot is skipped.
    pub load_failed: usize,
}

impl PoliceFleet {
    /// Whether the session had any authored police at all — the smoke
    /// record stays silent for the (majority) no-cop sessions.
    pub fn any(&self) -> bool {
        self.authored > 0
    }

    /// The smoke record's `pol=` value: `<spawned>/<authored>`, then
    /// `,uns<N>` (unplaceable rows) and `,fail<N>` (vehicles that did
    /// not load) only when nonzero. A networked session reads `0/<n>`.
    pub fn smoke_detail(&self) -> String {
        let mut s = format!("{}/{}", self.spawned, self.authored);
        if self.unplaceable > 0 {
            s.push_str(&format!(",uns{}", self.unplaceable));
        }
        if self.load_failed > 0 {
            s.push_str(&format!(",fail{}", self.load_failed));
        }
        s
    }
}

/// The yaw a cop is staged with: the authored heading in degrees as a
/// vehicle yaw (forward `(−sin h, −cos h)`); a row with no heading
/// faces the world's default (0).
pub fn staging_yaw(spec: &PoliceSpec) -> f32 {
    spec.heading_deg.unwrap_or(0.0).to_radians()
}

/// Field the roster's police as session-owned cars; returns the fleet
/// report (also what the caller inserts as a resource). An
/// unplaceable row or an unloadable vehicle is skipped and counted —
/// the roster is never padded, and the authored order is kept so a
/// skipped row never renumbers the rest. Networked sessions field none
/// (see the module docs); the report still carries the authored count
/// so the omission is visible.
#[allow(clippy::too_many_arguments)]
pub fn spawn_police(
    commands: &mut Commands,
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    roster: &PoliceRoster,
    authority: SessionAuthority,
    owner: SessionEntity,
    session: &mut Session,
) -> PoliceFleet {
    let mut fleet = PoliceFleet {
        authored: roster.entries.len(),
        ..PoliceFleet::default()
    };
    if authority != SessionAuthority::Local {
        return fleet;
    }
    for issue in &roster.issues {
        warn!(issue = %issue, "police roster issue");
    }
    let role = session.authority_role();
    for (index, spec) in roster.entries.iter().enumerate() {
        if !spec.placeable() {
            fleet.unplaceable += 1;
            warn!(
                line = spec.line,
                "police row unplaceable — authored slot skipped"
            );
            continue;
        }
        let def = match mm2_content::load_opponent(vfs, &spec.vehicle, 0) {
            Ok(def) => def,
            Err(e) => {
                fleet.load_failed += 1;
                warn!(
                    vehicle = %spec.vehicle,
                    error = %e,
                    "police vehicle failed to load — authored slot skipped"
                );
                continue;
            }
        };
        let yaw = staging_yaw(spec);
        let mut pos = spec.position;
        pos.y += reanchor_lift(&def.config);
        let object = session.mint_object_id();
        let vehicle = commands
            .spawn((
                owner,
                ObjectIdentity(object),
                role,
                DamageSignals::default(),
                PoliceCar {
                    index,
                    spec: spec.clone(),
                },
                Pursuit::new(),
                ChaseNav::default(),
                PoliceDrive {
                    bot: ScriptedBot::default(),
                    limits: CarLimits::of(&def.config, None),
                    turnarounds: 0,
                },
                vehicle_bundle(&def.config),
                Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                avian3d::prelude::TransformInterpolation,
                Visibility::Visible,
            ))
            .id();
        // Held in place until `police_pursuit` releases it: a released
        // car would roll off its authored spot on any slope.
        commands.entity(vehicle).insert(VehicleInput {
            handbrake: 1.0,
            ..VehicleInput::default()
        });
        equip_authored_vehicle(
            commands, vfs, meshes, images, materials, vehicle, &def, object, pos, yaw,
        );
        fleet.spawned += 1;
    }
    if fleet.spawned > 0 {
        info!(
            police = fleet.spawned,
            authored = fleet.authored,
            "police roster fielded"
        );
    }
    fleet
}

/// Height (m) above a body origin the sight line is cast from/to — a
/// driver's eye / a roof, clear of kerbs and road crowns.
const EYE_HEIGHT: f32 = 1.3;
/// A sight line that reaches within this many metres of the target is
/// clear (the target's own collider is excluded, but its shadow on a
/// kerb is not a wall).
const SIGHT_SLACK: f32 = 1.5;

/// The session city's routing graph, as the police chase reads it
/// (F20-B.2) — the same [`NavGraph::build_for_routing`] graph the
/// opponents' road re-path uses, so roads whose ambient classes are
/// closed stay drivable for a cop. Inserted at load only when cops were
/// fielded and the graph loaded; removed at teardown. Its absence is the
/// honest "no road knowledge" case: the chase aims straight.
#[derive(Resource, Debug)]
pub struct PoliceNav(pub NavGraph);

/// Pursuit evidence for the session: how often the machine moved.
/// Session-scoped with the fleet; the smoke record's `pur=` reads it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct PursuitReport {
    /// Idle → Engaged transitions (a cop noticed the target).
    pub noticed: u32,
    /// Engaged → Idle without committing.
    pub dismissed: u32,
    /// Chases begun (Engaged → Pursuing).
    pub committed: u32,
    /// Chases the cop gave up (Pursuing → Lost).
    pub gave_up: u32,
    /// Cops pursuing right now.
    pub pursuing: u32,
    /// Most cops pursuing at once.
    pub peak: u32,
    /// Road lines planned (the router found one).
    pub planned: u32,
    /// Router queries that found no line — the cop aimed straight.
    pub unrouted: u32,
}

impl PursuitReport {
    /// The smoke record's `pur=` value: `<committed>/<gave_up>/<peak>`,
    /// then `,r<N>` (road lines planned) and `,nr<N>` (queries that found
    /// none) only when nonzero, so a chase that never left the cop's
    /// direct range keeps the earlier shape.
    pub fn smoke_detail(&self) -> String {
        let mut s = format!("{}/{}/{}", self.committed, self.gave_up, self.peak);
        if self.planned > 0 {
            s.push_str(&format!(",r{}", self.planned));
        }
        if self.unrouted > 0 {
            s.push_str(&format!(",nr{}", self.unrouted));
        }
        s
    }

    fn note(&mut self, event: PursuitEvent) {
        match event {
            PursuitEvent::Noticed => self.noticed += 1,
            PursuitEvent::Dismissed => self.dismissed += 1,
            PursuitEvent::Committed => self.committed += 1,
            PursuitEvent::GaveUp => self.gave_up += 1,
            PursuitEvent::Rearmed | PursuitEvent::Stood => {}
        }
    }
}

/// Distance (m) from the goal inside which a pursuing cop stops
/// closing — it shadows the target instead of ramming through it
/// (what a catch does is unverified, COP-4, so none is invented).
const SHADOW_RANGE: f32 = 8.0;

/// A cop's driving state: the shared AI-driver stuck/escape machine
/// plus this car's own steering/brake limits (what `steer_toward` needs
/// to turn a bearing into the lock the car really has). Attached at
/// spawn; session-owned with the car.
#[derive(Component, Debug, Clone)]
pub struct PoliceDrive {
    /// The opponents' bounded reverse-and-turn escape — shared with
    /// them so a wedged cop recovers by the same limited law.
    pub bot: ScriptedBot,
    /// Steering geometry/limits for this vehicle.
    pub limits: CarLimits,
    /// Deliberate turn-arounds begun (a target behind a slow car) —
    /// separate from `bot.escapes`, which counts being wedged.
    pub turnarounds: u32,
}

/// A target this far off the nose (rad, ~100°) with the car this slow
/// (m/s) is turned round to by reversing, not by pushing forward on full
/// lock into whatever the nose points at — a cop released from its post
/// facing away from the road line.
const TURNAROUND_RAD: f32 = 1.75;
const TURNAROUND_SPEED: f32 = 4.0;

/// One frame of pursuit driving: steer at `aim` from a car at `pos`
/// facing `yaw` doing `speed` (m/s along its nose), with the
/// opponents' pure-pursuit steering law and stuck escape. `aim` is the
/// point on the road line the cop follows ([`ChaseNav::aim`]), or the
/// `goal` itself when there is no line; `goal` is where the chase ends.
///
/// The cop closes on the goal at a pace the remaining distance can
/// stop from, brakes to a halt within [`SHADOW_RANGE`] of it, and
/// slows for a bend: the speed at which its cornering grip holds the arc
/// to `aim` caps its pace.
pub fn chase_input(
    drive: &mut PoliceDrive,
    pos: Vec3,
    yaw: f32,
    speed: f32,
    grounded: bool,
    aim: Vec3,
    goal: Vec3,
) -> VehicleInput {
    let dist = (goal.x - pos.x).hypot(goal.z - pos.z);
    if !dist.is_finite() || !yaw.is_finite() || !aim.is_finite() || dist < SHADOW_RANGE {
        // At the goal (or nothing usable to chase): stop, and do not
        // read a deliberate halt as being wedged.
        drive.bot.stuck_frames = 0;
        return VehicleInput {
            brake: 1.0,
            ..VehicleInput::default()
        };
    }
    let aim_dist = (aim.x - pos.x).hypot(aim.z - pos.z);
    let bearing = relative_bearing(yaw, pos, aim);
    if bearing.abs() > TURNAROUND_RAD
        && speed.abs() < TURNAROUND_SPEED
        && grounded
        && drive.bot.reverse_frames == 0
        && drive.bot.turn_frames == 0
    {
        begin_turnaround(&mut drive.bot, bearing);
        drive.turnarounds += 1;
    }
    if let Some(escape) = recovery_input(&mut drive.bot, bearing, &ScriptedTuning::DEFAULT) {
        return escape;
    }
    // Close at a speed the remaining distance can brake down from, and
    // no faster than the arc to the aim point can be held.
    let arrive = (2.0 * drive.limits.brake_accel * (dist - SHADOW_RANGE)).sqrt();
    let curvature = 2.0 * bearing.sin().abs() / aim_dist.max(1.0);
    let bend = if curvature > 1e-3 {
        (drive.limits.corner_accel / curvature).sqrt()
    } else {
        f32::INFINITY
    };
    let pace = arrive.min(bend);
    let over = speed > pace;
    let input = VehicleInput {
        steering: steer_toward(bearing, aim_dist, speed, &drive.limits),
        throttle: if over { 0.0 } else { bearing_throttle(bearing) },
        brake: if speed > arrive || speed > bend * BEND_BRAKE_MARGIN {
            1.0
        } else {
            0.0
        },
        ..VehicleInput::default()
    };
    watch_stuck(&mut drive.bot, &input, speed, grounded);
    input
}

/// How far over a bend's pace a cop must be before it brakes rather than
/// merely lifting — a small excess is shed by coasting. Arriving at the
/// goal always brakes: nothing there is to coast into.
const BEND_BRAKE_MARGIN: f32 = 1.15;

/// Whether the static city geometry leaves the line between `from` and
/// `to` open. Only [`GameLayer::World`](crate::layers::GameLayer) blocks
/// sight — another car never hides the target from a cop.
fn line_clear(spatial: &SpatialQuery, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    let Ok(dir) = Dir3::new(d) else { return true };
    let filter = SpatialQueryFilter::from_mask(crate::layers::GameLayer::World);
    spatial
        .cast_ray(from, dir, len, false, &filter)
        .is_none_or(|hit| hit.distance >= len - SIGHT_SLACK)
}

/// Advance every fielded cop's [`Pursuit`] and drive it (F20-A.3).
///
/// A cop's target is the nearest eligible local human — a
/// `PlayerVehicle` that is `Racing` (or has no race progress). Nobody
/// eligible (countdown, a finished race, an AI/remote seat) stands the
/// cops down at their posts: a result ends a chase, never a bust.
/// The pursuer cap is shared across the fleet, filled in authored
/// order so a run is deterministic. A pursuing cop gains
/// [`EmergencyLights`] (and keeps its flash clock running); every
/// other phase removes them. Its aim comes from its [`ChaseNav`] over
/// the session's [`PoliceNav`] graph (F20-B.2).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn police_pursuit(
    mut commands: Commands,
    time: Res<Time>,
    policy: Option<Res<PursuitPolicy>>,
    report: Option<ResMut<PursuitReport>>,
    nav: Option<Res<PoliceNav>>,
    spatial: SpatialQuery,
    targets: Query<(&Player, &Position, Option<&RaceProgress>), With<PlayerVehicle>>,
    mut cops: Query<(
        Entity,
        &PoliceCar,
        &mut Pursuit,
        &mut PoliceDrive,
        &mut ChaseNav,
        &mut VehicleInput,
        &Position,
        &Rotation,
        &VehicleState,
        Option<&mut EmergencyLights>,
    )>,
) {
    let (Some(policy), Some(mut report)) = (policy, report) else {
        return;
    };
    let dt = time.delta_secs();
    let targets: Vec<Vec3> = targets
        .iter()
        .filter(|(player, _, progress)| {
            player.control == PlayerControl::Local
                && progress.is_none_or(|p| p.state == ParticipantState::Racing)
        })
        .map(|(_, pos, _)| pos.0)
        .collect();
    let mut order: Vec<(usize, Entity)> = cops.iter().map(|c| (c.1.index, c.0)).collect();
    order.sort_unstable();
    let mut pursuing = cops.iter().filter(|c| c.2.is_pursuing()).count();
    for (_, entity) in order {
        let Ok((_, _, mut pursuit, mut drive, mut chase, mut input, pos, rot, vstate, mut lights)) =
            cops.get_mut(entity)
        else {
            continue;
        };
        let eye = pos.0 + Vec3::Y * EYE_HEIGHT;
        let nearest = targets
            .iter()
            .map(|t| (*t, t.distance(pos.0)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let sighting = nearest.map(|(position, distance)| Sighting {
            position,
            distance,
            clear: distance <= policy.contact_range
                && line_clear(&spatial, eye, position + Vec3::Y * EYE_HEIGHT),
        });
        let was_pursuing = pursuit.is_pursuing();
        let event = if sighting.is_none() {
            pursuit.stand_down()
        } else {
            pursuit.step(
                dt,
                sighting.as_ref(),
                pursuing < policy.max_pursuers,
                &policy,
            )
        };
        if let Some(event) = event {
            report.note(event);
        }
        match (was_pursuing, pursuit.is_pursuing()) {
            (false, true) => pursuing += 1,
            (true, false) => pursuing = pursuing.saturating_sub(1),
            _ => {}
        }
        report.pursuing = pursuing as u32;
        report.peak = report.peak.max(report.pursuing);
        // Signals ride the chase exactly: on while pursuing, off the
        // moment the cop reacts-only, gives up or stands down (COP-10).
        match (pursuit.is_pursuing(), lights.as_deref_mut()) {
            (true, Some(lights)) => lights.advance(dt),
            (true, None) => {
                commands.entity(entity).insert(EmergencyLights::default());
            }
            (false, Some(_)) => {
                commands.entity(entity).remove::<EmergencyLights>();
            }
            (false, None) => {}
        }
        // A chase that ended leaves no stale road line for the next.
        if !pursuit.is_pursuing() && chase.mode != ChaseMode::Idle {
            chase.clear();
        }
        let speed = vstate.forward_speed;
        *input = match (pursuit.phase, pursuit.last_seen) {
            (PursuitPhase::Pursuing(_), Some(goal)) => {
                let fwd = rot.0 * Vec3::NEG_Z;
                let yaw = (-fwd.x).atan2(-fwd.z);
                let in_view = sighting.is_some_and(|s| s.clear);
                let aim = chase.aim(nav.as_deref().map(|n| &n.0), dt, pos.0, goal, in_view);
                report.planned += u32::from(aim.planned);
                report.unrouted += u32::from(aim.failed);
                chase_input(
                    &mut drive,
                    pos.0,
                    yaw,
                    speed,
                    vstate.grounded,
                    aim.point,
                    goal,
                )
            }
            // Gave up: roll to a stop where it is.
            (PursuitPhase::Lost(_), _) => VehicleInput {
                brake: 1.0,
                handbrake: if speed.abs() < 1.0 { 1.0 } else { 0.0 },
                ..VehicleInput::default()
            },
            // At its post, watching or reacting.
            _ => VehicleInput {
                handbrake: 1.0,
                ..VehicleInput::default()
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive() -> PoliceDrive {
        PoliceDrive {
            bot: ScriptedBot::default(),
            limits: CarLimits::of(&mm2_vehicle::VehicleConfig::default(), None),
            turnarounds: 0,
        }
    }

    /// A chase with no road line: the aim is the goal.
    fn chase_goal(
        d: &mut PoliceDrive,
        pos: Vec3,
        yaw: f32,
        speed: f32,
        grounded: bool,
        goal: Vec3,
    ) -> VehicleInput {
        chase_input(d, pos, yaw, speed, grounded, goal, goal)
    }

    #[test]
    fn steering_turns_toward_the_goal() {
        // Facing −Z (yaw 0): a goal ahead and to the right (+x) steers
        // right (positive), to the left steers left.
        let mut d = drive();
        let r = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            5.0,
            true,
            Vec3::new(20.0, 0.0, -60.0),
        );
        assert!(r.steering > 0.0, "{r:?}");
        let l = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            5.0,
            true,
            Vec3::new(-20.0, 0.0, -60.0),
        );
        assert!(l.steering < 0.0, "{l:?}");
        let straight = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            5.0,
            true,
            Vec3::new(0.0, 0.0, -80.0),
        );
        assert!(straight.steering.abs() < 1e-3, "{straight:?}");
        assert_eq!((straight.throttle, straight.brake), (1.0, 0.0));
        assert_eq!(straight.handbrake, 0.0);
        // A goal dead behind takes full lock toward it.
        let behind = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            2.0,
            true,
            Vec3::new(5.0, 0.0, 80.0),
        );
        assert_eq!(behind.steering.abs(), 1.0);
    }

    #[test]
    fn it_brakes_onto_a_near_goal_and_for_a_speed_the_distance_cannot_stop() {
        let mut d = drive();
        // Inside the shadow range: stop, no throttle.
        let near = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            20.0,
            true,
            Vec3::new(0.0, 0.0, -3.0),
        );
        assert_eq!((near.throttle, near.brake), (0.0, 1.0));
        // Fast and close (but outside it): brake rather than drive on.
        let fast = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            40.0,
            true,
            Vec3::new(0.0, 0.0, -20.0),
        );
        assert_eq!((fast.throttle, fast.brake), (0.0, 1.0));
        // Slow and far: drive.
        let go = chase_goal(
            &mut d,
            Vec3::ZERO,
            0.0,
            3.0,
            true,
            Vec3::new(0.0, 0.0, -90.0),
        );
        assert_eq!((go.throttle, go.brake), (1.0, 0.0));
    }

    #[test]
    fn it_steers_for_the_aim_but_stops_for_the_goal() {
        let mut d = drive();
        // The goal is dead ahead 90 m, the aim 20 m to the right: the
        // wheel follows the aim.
        let goal = Vec3::new(0.0, 0.0, -90.0);
        let aim = Vec3::new(15.0, 0.0, -13.0);
        let i = chase_input(&mut d, Vec3::ZERO, 0.0, 3.0, true, aim, goal);
        assert!(i.steering > 0.0, "{i:?}");
        // Arriving is judged against the goal: close to the *aim* (5 m)
        // but far from the goal is not a stop.
        let near_aim = Vec3::new(0.0, 0.0, -5.0);
        let i = chase_input(&mut d, Vec3::ZERO, 0.0, 3.0, true, near_aim, goal);
        assert_eq!((i.throttle, i.brake), (1.0, 0.0), "{i:?}");
        // Within the shadow range of the goal it stops, whatever the aim.
        let i = chase_input(
            &mut d,
            Vec3::ZERO,
            0.0,
            3.0,
            true,
            aim,
            Vec3::new(0.0, 0.0, -3.0),
        );
        assert_eq!((i.throttle, i.brake), (0.0, 1.0));
        // A non-finite aim is not driven at.
        let i = chase_input(&mut d, Vec3::ZERO, 0.0, 3.0, true, Vec3::NAN, goal);
        assert_eq!((i.throttle, i.brake), (0.0, 1.0));
    }

    #[test]
    fn it_slows_for_a_bend_the_arc_cannot_be_held_at() {
        let mut d = drive();
        let goal = Vec3::new(300.0, 0.0, -300.0);
        // A sharp turn 20 m ahead (bearing ≈ 63°): at speed the cop lifts,
        // far over the pace it brakes, and slow enough it keeps driving.
        let aim = Vec3::new(18.0, 0.0, -9.0);
        let slow = chase_input(&mut d, Vec3::ZERO, 0.0, 4.0, true, aim, goal);
        assert!(slow.throttle > 0.0 && slow.brake == 0.0, "{slow:?}");
        let fast = chase_input(&mut d, Vec3::ZERO, 0.0, 30.0, true, aim, goal);
        assert_eq!((fast.throttle, fast.brake), (0.0, 1.0), "{fast:?}");
        // The same speed with the aim straight ahead is no bend at all.
        let ahead = chase_input(
            &mut d,
            Vec3::ZERO,
            0.0,
            30.0,
            true,
            Vec3::new(0.0, 0.0, -20.0),
            goal,
        );
        assert!(ahead.throttle > 0.0 && ahead.brake == 0.0, "{ahead:?}");
    }

    #[test]
    fn a_slow_car_with_the_aim_behind_it_reverses_round_instead_of_pushing_on() {
        let mut d = drive();
        // The aim is behind and to the right; the car is at rest. It
        // backs up swinging the nose toward the aim, then drives the
        // turn forward on that side — the same escape machinery, begun
        // on purpose and not counted as being wedged.
        let aim = Vec3::new(10.0, 0.0, 30.0);
        let goal = Vec3::new(100.0, 0.0, 100.0);
        let first = chase_input(&mut d, Vec3::ZERO, 0.0, 0.0, true, aim, goal);
        assert_eq!((first.throttle, first.brake), (0.0, 1.0), "{first:?}");
        assert_eq!(
            first.steering.signum(),
            -1.0,
            "reversing swings the nose right: {first:?}"
        );
        assert_eq!((d.turnarounds, d.bot.escapes), (1, 0));
        // It stays in the manoeuvre without re-arming each frame.
        for _ in 0..40 {
            chase_input(&mut d, Vec3::ZERO, 0.0, -1.0, true, aim, goal);
        }
        assert_eq!(d.turnarounds, 1);
        // A fast car, or one airborne, is never put into reverse for it.
        let mut d = drive();
        let fast = chase_input(&mut d, Vec3::ZERO, 0.0, 12.0, true, aim, goal);
        assert_eq!((fast.brake, d.turnarounds), (0.0, 0));
        let air = chase_input(&mut d, Vec3::ZERO, 0.0, 0.0, false, aim, goal);
        assert_eq!((air.brake, d.turnarounds), (0.0, 0));
        // And a target merely off to the side is not a turn-around.
        let side = chase_input(
            &mut d,
            Vec3::ZERO,
            0.0,
            0.0,
            true,
            Vec3::new(30.0, 0.0, 0.0),
            goal,
        );
        assert_eq!(side.brake, 0.0);
    }

    #[test]
    fn a_degenerate_goal_or_pose_brakes_instead_of_driving() {
        let mut d = drive();
        for (yaw, goal) in [(0.0, Vec3::NAN), (f32::NAN, Vec3::new(0.0, 0.0, -50.0))] {
            let i = chase_goal(&mut d, Vec3::ZERO, yaw, 5.0, true, goal);
            assert_eq!((i.throttle, i.brake), (0.0, 1.0));
        }
    }

    #[test]
    fn a_wedged_chaser_arms_the_bounded_escape_and_a_halt_is_not_wedged() {
        let mut d = drive();
        let goal = Vec3::new(0.0, 0.0, -90.0);
        let mut escaped = false;
        for _ in 0..400 {
            let i = chase_goal(&mut d, Vec3::ZERO, 0.0, 0.0, true, goal);
            if i.brake == 1.0 && i.throttle == 0.0 {
                escaped = true; // the reverse half of the escape
                break;
            }
        }
        assert!(escaped && d.bot.escapes == 1, "{:?}", d.bot);
        // Standing at the goal braking is a deliberate stop, never counted.
        let mut d = drive();
        for _ in 0..400 {
            chase_goal(
                &mut d,
                Vec3::ZERO,
                0.0,
                0.0,
                true,
                Vec3::new(0.0, 0.0, -2.0),
            );
        }
        assert_eq!(d.bot.escapes, 0);
    }

    #[test]
    fn the_report_counts_only_phase_changes_it_names() {
        let mut r = PursuitReport::default();
        for e in [
            PursuitEvent::Noticed,
            PursuitEvent::Committed,
            PursuitEvent::GaveUp,
            PursuitEvent::Rearmed,
            PursuitEvent::Stood,
        ] {
            r.note(e);
        }
        r.peak = 2;
        assert_eq!(r.smoke_detail(), "1/1/2");
        // Road-line counters join only when something happened.
        r.planned = 3;
        assert_eq!(r.smoke_detail(), "1/1/2,r3");
        r.unrouted = 1;
        assert_eq!(r.smoke_detail(), "1/1/2,r3,nr1");
        assert_eq!((r.noticed, r.dismissed), (1, 0));
    }
}

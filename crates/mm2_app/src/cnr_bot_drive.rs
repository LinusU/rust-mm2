//! CNRBOT-C — the driver behind a local Cops & Robbers bot: it turns the
//! planner's objective ([`mm2_game::cnr_tactics`]) into a line to drive
//! and the existing neural policy into pedals.
//!
//! A designed single-player enhancement, **not** the retail AI. Nothing
//! here is a new way of driving: the line is a [`Guide`] handed to the
//! same [`NeuralState`] race opponents use, its output is the bot car's
//! own [`VehicleInput`], and the car is the production rigid body. A bot
//! never moves to the gold, never writes the match, a score or another
//! car's input; a pickup, a steal and a delivery happen only because the
//! hull entered the trigger or struck the carrier, and
//! [`crate::cnr::cnr_host_step`] decides it.
//!
//! **Ordering.** [`cnr_bot_drive`] runs in `FixedUpdate`, ahead of the
//! physics step; the gold step runs in `FixedLast`. A bot therefore
//! always plans from the match state the previous step left, and its
//! pedals act on the next.
//!
//! **Lines.** The line from the car to the objective is the directed
//! street route over the city's [`NavGraph`] ([`NavGraph::drive_line`]).
//! Where there is none (no lane near either end, no connection, the
//! expansion bound) the bot aims straight at the objective — the
//! documented fallback, counted in [`BotStats::straight`] — and its rays
//! and neighbour sensing still steer it round what stands in the way.
//! Within [`DIRECT_RANGE`] of the objective, and with the way open to a
//! car, it also aims straight: a plaza or a lawn is no street, and a
//! hull that must enter a trigger need not follow the road into it.
//! Every line runs [`OVERSHOOT`] past the objective so the car arrives
//! at speed rather than slowing for the end of its guide.
//!
//! **Replanning** is bounded so a moving carrier cannot make a bot
//! thrash: a goal switch re-plans at once; otherwise a line is replaced
//! only after [`REPLAN_FRAMES`] (fewer, [`DIRECT_REPLAN_FRAMES`], on a
//! straight line) and when the target has moved [`GOAL_DRIFT`] from
//! where the line was planned, the line has run out, or the car is
//! [`OFF_ROUTE`] from it. A failed plan waits [`FAILED_WAIT`], and the
//! field spends at most [`PLANS_PER_STEP`] router queries a step.
//!
//! **Recovery.** The policy's own back-up answers an ordinary stall. A
//! bot that stays inside [`REANCHOR_DIST`] for [`BOT_REANCHOR_FRAMES`]
//! is put back on its line through the production `ResetVehicle` path,
//! counted in [`CnrBotDriver::reanchors`] and logged. Stuck again at the
//! same place, it is put down a little *ahead* on the line, past what
//! stopped it ([`REANCHOR_PAST`]). A re-anchor can never score: the
//! landing keeps
//! [`TRIGGER_KEEP_OUT`] from the gold and from both delivery markers,
//! and is refused — counted in [`CnrBotDriver::refused`] — when no such
//! place exists.

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::cnr_tactics::{CarPose, Objective, Planner, Snapshot, TacticsConfig};
use mm2_game::{
    DamageTier, NavGraph, Player, PlayerControl, PlayerId, RouteOptions, Session, VehicleDamage,
};
use mm2_vehicle::{ResetVehicle, Teleported, Vehicle, VehicleInput, VehicleState};
use tracing::{info, warn};

use crate::cnr::{CnrHost, participant_id};
use crate::cnr_bots::CnrBot;
use crate::netdrive::NetPlayer;
use crate::neural::drive::{DrivingPolicies, HEAVY_PROP, NeuralState};
use crate::neural::guide::{Guide, way_open};
use crate::neural::observe::{Motion, Neighbour};
use crate::neural::policy::Scratch;
use crate::opponents::{AMBIENT_SIZE, REANCHOR_DIST, reanchor_occupied, seat_reanchor};

/// Metres between samples of a street line.
pub const ROUTE_STEP: f32 = 6.0;
/// Metres a line runs past its objective.
pub const OVERSHOOT: f32 = 12.0;
/// Distance (m, XZ) to the objective within which a bot aims straight
/// at it, when the way is open.
pub const DIRECT_RANGE: f32 = 30.0;
/// Frames before a street line may be replaced without a goal switch.
pub const REPLAN_FRAMES: u32 = 90;
/// ... and before a straight line may.
pub const DIRECT_REPLAN_FRAMES: u32 = 20;
/// Metres the target may move from where a street line was planned
/// before the line is replaced.
pub const GOAL_DRIFT: f32 = 20.0;
/// ... and a straight line.
pub const DIRECT_DRIFT: f32 = 4.0;
/// Metres off its line (XZ) at which a bot is astray and re-plans.
pub const OFF_ROUTE: f32 = 35.0;
/// ... and off a straight line to a near objective: the car is then
/// re-aimed from where it is, so a hull that drifted wide of a trigger
/// swings back onto it.
pub const DIRECT_OFF_ROUTE: f32 = 5.0;
/// Metres from a still trigger (gold to take, marker to deliver to, a
/// guard post) within which a bot caps its throttle at
/// [`APPROACH_THROTTLE`]: a pickup is 5 m wide, and a car at full speed
/// misses it by more than that. Chases and intercepts are not capped —
/// the harder the hit the better.
pub const APPROACH_RANGE: f32 = 45.0;
/// The throttle cap on approach to a still trigger.
pub const APPROACH_THROTTLE: f32 = 0.5;
/// Frames a failed plan waits before the router is asked again.
pub const FAILED_WAIT: u32 = 180;
/// Router queries the whole field may spend in one fixed step.
pub const PLANS_PER_STEP: u32 = 1;
/// Frames inside [`REANCHOR_DIST`] before a bot is put back on its line
/// — 9 s, the opponents' window: the policy's back-ups get their turns.
pub const BOT_REANCHOR_FRAMES: u32 = 540;
/// Metres a re-anchor landing keeps from the gold and from both
/// delivery markers: a recovery must never be a pickup or a delivery.
pub const TRIGGER_KEEP_OUT: f32 = 25.0;
/// Metres back along its line a re-anchor looks for a landing, nearest
/// first.
const REANCHOR_BACK: [f32; 6] = [0.0, 8.0, 16.0, 28.0, 44.0, 64.0];
/// Metres ahead on its line a second re-anchor at the same place may
/// land, nearest first: past a wreck, not past a corner.
const REANCHOR_PAST: [f32; 5] = [12.0, 18.0, 25.0, 35.0, 50.0];
/// Metres of line left at which it counts as run out.
const LINE_END: f32 = 3.0;

/// Whether the car's pedals are held back for the match's state.
fn holds(session: &Session, host: &CnrHost) -> bool {
    !session.is_playing()
        || !session.authority_role().is_authority()
        || host.game.outcome().is_some()
}

/// The session's bot brain: the planner and the street graph. Inserted
/// with the roster and removed with the session.
#[derive(Resource)]
pub struct CnrBotBrain {
    planner: Planner,
    graph: Option<Arc<NavGraph>>,
    /// What the field has done so far.
    pub stats: BotStats,
}

impl CnrBotBrain {
    /// A brain over the city's street graph (`None`: every line is the
    /// straight fallback).
    pub fn new(graph: Option<Arc<NavGraph>>) -> Self {
        Self {
            planner: Planner::new(TacticsConfig::default()),
            graph,
            stats: BotStats::default(),
        }
    }
}

/// Field-wide counters — the evidence that replanning stays bounded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BotStats {
    /// Lines planned over the street graph.
    pub streets: u64,
    /// Lines that were straight because the router had none.
    pub straight: u64,
    /// Lines that were straight because the objective was near.
    pub direct: u64,
    /// Router queries that failed.
    pub failures: u64,
    /// Goal switches that replaced a line.
    pub switches: u64,
}

/// How a line was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Over the street graph.
    Street,
    /// Straight, the router having none.
    Fallback,
    /// Straight, the objective being near and the way open.
    Direct,
}

/// What a bot is after, ignoring where it is: the objective's kind and
/// the car it concerns.
type Goal = (std::mem::Discriminant<Objective>, Option<PlayerId>);

fn goal_of(o: &Objective) -> Goal {
    let car = match *o {
        Objective::ChaseEnemyCarrier { target, .. }
        | Objective::SupportFriendlyCarrier { target, .. }
        | Objective::Intercept { target, .. } => Some(target),
        _ => None,
    };
    (std::mem::discriminant(o), car)
}

/// One bot's driver state. Rides on the [`CnrBot`] car, so teardown
/// takes it with the car; race opponents never carry one.
#[derive(Component, Default)]
pub struct CnrBotDriver {
    neural: Option<NeuralState>,
    goal: Option<Goal>,
    /// Where the objective was when the line was planned.
    planned_for: Vec3,
    kind: Option<LineKind>,
    since_plan: u32,
    fail_wait: u32,
    stuck_pos: Vec3,
    stuck_frames: u32,
    /// Lines this bot has planned.
    pub plans: u32,
    /// Times the bot was put back on its line.
    pub reanchors: u32,
    /// Re-anchors refused for want of a safe landing.
    pub refused: u32,
}

impl CnrBotDriver {
    /// How the current line was made.
    pub fn line_kind(&self) -> Option<LineKind> {
        self.kind
    }

    /// The current line's remaining length, if there is one.
    pub fn line_remaining(&self) -> Option<f32> {
        let n = self.neural.as_ref()?;
        n.guide.remaining(n.arc)
    }
}

/// A line from `from` to `to` over `graph`, or the straight one, with
/// the overshoot. `near_open` says the objective is near and the way
/// to it open to a car. Pure but for the router.
pub fn plan_line(
    graph: Option<&NavGraph>,
    from: Vec3,
    to: Vec3,
    near_open: bool,
) -> (Vec<Vec3>, LineKind) {
    let (mut line, kind) = if near_open {
        (vec![from, to], LineKind::Direct)
    } else {
        match graph.map(|g| g.drive_line(from, to, &RouteOptions::default(), ROUTE_STEP)) {
            Some(Ok(line)) if line.len() >= 2 => (line, LineKind::Street),
            _ => (vec![from, to], LineKind::Fallback),
        }
    };
    let n = line.len();
    let tail = line[n - 1] - line[n - 2];
    let flat = Vec3::new(tail.x, 0.0, tail.z).normalize_or_zero();
    if flat != Vec3::ZERO {
        let end = line[n - 1];
        line.push(end + flat * OVERSHOOT);
    }
    (line, kind)
}

/// Whether an existing line must be replaced, and why not otherwise.
pub fn needs_replan(d: &CnrBotDriver, goal: Goal, target: Vec3, pos: Vec3) -> Option<&'static str> {
    let Some(neural) = d.neural.as_ref() else {
        return (d.fail_wait == 0).then_some("first line");
    };
    if d.goal != Some(goal) {
        return Some("goal switch");
    }
    let direct = d.kind == Some(LineKind::Direct);
    let wait = if direct {
        DIRECT_REPLAN_FRAMES
    } else {
        REPLAN_FRAMES
    };
    if d.since_plan < wait {
        return None;
    }
    let drift = if direct { DIRECT_DRIFT } else { GOAL_DRIFT };
    if (target.x - d.planned_for.x).hypot(target.z - d.planned_for.z) > drift {
        return Some("target moved");
    }
    if neural
        .guide
        .remaining(neural.arc)
        .is_some_and(|m| m < LINE_END)
    {
        return Some("line run out");
    }
    let on = neural.guide.point_at(neural.arc);
    let limit = if direct { DIRECT_OFF_ROUTE } else { OFF_ROUTE };
    ((pos.x - on.x).hypot(pos.z - on.z) > limit).then_some("astray")
}

/// What `cnr_bot_drive` reads of every car it must know about.
type Field<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Position,
        &'static Rotation,
        &'static LinearVelocity,
        Option<&'static Vehicle>,
    ),
    Or<(With<Vehicle>, With<crate::traffic::AmbientCar>)>,
>;

/// The bots' fixed-step leg: plan, line, drive.
///
/// Idle without a [`CnrHost`] and a [`CnrBotBrain`] (so outside a
/// local Cops & Robbers session it touches nothing), holds every bot's
/// pedals while the session is paused or the match is decided, and
/// runs on the authority only.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)] // Bevy system: the field, the physics handles and the writers
pub fn cnr_bot_drive(
    session: Res<Session>,
    host: Option<Res<CnrHost>>,
    brain: Option<ResMut<CnrBotBrain>>,
    spatial: Option<SpatialQuery>,
    bodies: Query<&RigidBody>,
    colliders: Query<&ColliderOf>,
    bangers: Query<&mm2_game::Banger>,
    water: Option<Res<crate::water::CityWater>>,
    policies: Option<Res<DrivingPolicies>>,
    mut scratch: Local<Scratch>,
    field: Field,
    people: Query<(&Player, Option<&NetPlayer>, Has<CnrBot>, &Position)>,
    mut resets: MessageWriter<ResetVehicle>,
    mut bots: Query<
        (
            Entity,
            &mut VehicleInput,
            &Position,
            &Rotation,
            (&LinearVelocity, &AngularVelocity),
            &Vehicle,
            &VehicleState,
            Option<&VehicleDamage>,
            &Player,
            &mut CnrBotDriver,
            Has<Teleported>,
        ),
        With<CnrBot>,
    >,
) {
    let (Some(host), Some(mut brain)) = (host, brain) else {
        return;
    };
    if holds(&session, &host) {
        for (_, mut input, _, _, _, _, _, _, _, _, _) in &mut bots {
            *input = VehicleInput {
                handbrake: 1.0,
                ..default()
            };
        }
        return;
    }
    let brain = &mut *brain;
    let policies = policies
        .as_deref()
        .unwrap_or_else(|| DrivingPolicies::built_in());
    let policy = policies.for_difficulty(
        session
            .config()
            .map_or(mm2_game::Difficulty::Amateur, |c| c.difficulty),
    );

    // The match as the planner sees it: the participants' cars.
    let view = host.game.view();
    let cars: Vec<CarPose> = people
        .iter()
        .filter(|(p, _, bot, _)| p.control != PlayerControl::Ai || *bot)
        .map(|(p, net, _, pos)| CarPose {
            player: participant_id(p, net),
            position: pos.0,
        })
        .collect();
    let ids: Vec<PlayerId> = bots
        .iter()
        .map(|b| participant_id(b.8, None))
        .collect::<Vec<_>>();
    let plan = brain.planner.plan(
        &Snapshot {
            view: &view,
            cars: &cars,
            bounds: None,
        },
        &ids,
    );

    let neighbours: Vec<(Entity, Neighbour)> = field
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
    let filter = SpatialQueryFilter::default();
    let in_field = |c: Entity| {
        let body = colliders.get(c).map_or(c, |c| c.body);
        neighbours.iter().any(|(e, _)| *e == body)
    };
    // In the way is what a car could not push aside: the world and its
    // heavy props.
    let solid = |c: Entity| {
        let body = colliders.get(c).map_or(c, |c| c.body);
        !in_field(c) && bangers.get(body).map_or(true, |b| b.def.mass >= HEAVY_PROP)
    };
    let ground = |c: Entity| {
        let body = colliders.get(c).map_or(c, |c| c.body);
        !in_field(c) && !bangers.contains(body) && bodies.get(body).is_ok_and(|b| !b.is_dynamic())
    };
    let rules = host.game.rules();
    let sites = host.game.sites();
    let keep_out: Vec<Vec3> = [
        Some(sites.hideout),
        Some(sites.bank),
        host.game.gold_position(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let keep_out_radius =
        TRIGGER_KEEP_OUT.max(rules.pickup_radius.max(rules.delivery_radius) * 2.0);

    let mut plans_left = PLANS_PER_STEP;
    let mut laid = false;
    let mut claimed: Vec<Vec3> = Vec::new();
    for (
        entity,
        mut input,
        pos,
        rot,
        (lin_vel, ang_vel),
        vehicle,
        vstate,
        damage,
        player,
        mut driver,
        teleported,
    ) in &mut bots
    {
        let d = &mut *driver;
        let me = participant_id(player, None);
        *input = VehicleInput::default();
        if !pos.0.is_finite() {
            continue;
        }
        // A wreck coasts; the session's disabled-vehicle policy owns it.
        if damage.is_some_and(|x| x.condition() == DamageTier::Disabled) {
            input.handbrake = 1.0;
            continue;
        }
        d.fail_wait = d.fail_wait.saturating_sub(1);
        d.since_plan = d.since_plan.saturating_add(1);
        let Some(&(_, objective)) = plan.iter().find(|(p, _)| *p == me) else {
            // Not in the match (yet): stand.
            input.handbrake = 1.0;
            continue;
        };
        let target = objective.position();
        let goal = goal_of(&objective);

        if teleported && let Some(n) = d.neural.as_mut() {
            n.reseat(pos.0);
        }

        if let Some(why) = needs_replan(d, goal, target, pos.0)
            && plans_left > 0
            && target.is_finite()
        {
            plans_left -= 1;
            let flat = (target.x - pos.0.x).hypot(target.z - pos.0.z);
            let near_open = flat <= DIRECT_RANGE
                && spatial.as_ref().is_some_and(|sq| {
                    way_open(
                        pos.0,
                        target,
                        false,
                        1.3,
                        |from, dir, reach| {
                            sq.cast_ray_predicate(
                                from,
                                Dir3::new(dir).ok()?,
                                reach,
                                true,
                                &filter,
                                &solid,
                            )
                            .map(|hit| (hit.distance, hit.normal))
                        },
                        |_| None,
                    )
                });
            let (line, kind) = plan_line(brain.graph.as_deref(), pos.0, target, near_open);
            match kind {
                LineKind::Street => brain.stats.streets += 1,
                LineKind::Fallback => {
                    brain.stats.straight += 1;
                    if brain.graph.is_some() {
                        brain.stats.failures += 1;
                        d.fail_wait = FAILED_WAIT;
                    }
                }
                LineKind::Direct => brain.stats.direct += 1,
            }
            if why == "goal switch" {
                brain.stats.switches += 1;
            }
            if let Some(guide) = Guide::new(line, false) {
                d.neural = Some(NeuralState::new(Arc::new(guide), &vehicle.config, pos.0, 1));
                d.goal = Some(goal);
                d.planned_for = target;
                d.kind = Some(kind);
                d.since_plan = 0;
                d.plans += 1;
                d.stuck_pos = pos.0;
                d.stuck_frames = 0;
            } else {
                warn!(?target, "bot line collapsed to a point");
                d.fail_wait = FAILED_WAIT;
            }
        }
        let Some(neural) = d.neural.as_mut() else {
            continue;
        };

        // Lay the new line on the world, one a step.
        if !laid && let Some(sq) = spatial.as_ref() {
            laid = neural.ground(
                pos.0,
                |from, reach| {
                    sq.cast_ray_predicate(from, Dir3::NEG_Y, reach, false, &filter, &ground)
                        .map(|hit| {
                            let body = colliders.get(hit.entity).map_or(hit.entity, |c| c.body);
                            (hit.distance, bodies.get(body).is_ok_and(|b| b.is_static()))
                        })
                },
                |from, dir, reach| {
                    sq.cast_ray_predicate(from, Dir3::new(dir).ok()?, reach, true, &filter, &solid)
                        .map(|hit| (hit.distance, hit.normal))
                },
            );
        }
        neural.track(pos.0);

        // The bounded last resort.
        if pos.0.distance(d.stuck_pos) >= REANCHOR_DIST {
            d.stuck_pos = pos.0;
            d.stuck_frames = 0;
        } else {
            d.stuck_frames += 1;
        }
        if d.stuck_frames >= BOT_REANCHOR_FRAMES {
            d.stuck_frames = 0;
            d.stuck_pos = pos.0;
            let lift = crate::opponents::reanchor_lift(&vehicle.config);
            let is_static = |c: Entity| {
                let body = colliders.get(c).map_or(c, |c| c.body);
                !bodies
                    .get(body)
                    .is_ok_and(|b| b.is_dynamic() || b.is_kinematic())
            };
            // Re-anchored here a moment ago and stuck again: what stopped
            // the car stands on its line and will stop it every time it
            // is put down behind it, so this once the landing is past it.
            let again = neural.reanchored_here_before();
            let offsets: Vec<f32> = if again {
                REANCHOR_PAST.to_vec()
            } else {
                REANCHOR_BACK.iter().map(|b| -b).collect()
            };
            let landing = offsets.into_iter().find_map(|offset| {
                let ahead = offset > 0.0;
                if ahead
                    && neural
                        .guide
                        .remaining(neural.arc)
                        .is_some_and(|left| left < offset + 5.0)
                {
                    return None;
                }
                let arc = (neural.arc + offset).max(0.0);
                let p = neural.guide.point_at(arc);
                let t = neural.guide.tangent_at(arc);
                let yaw = (-t.x).atan2(-t.z);
                let near_trigger = keep_out
                    .iter()
                    .any(|k| (p.x - k.x).hypot(p.z - k.z) < keep_out_radius);
                let occupied = reanchor_occupied(
                    p,
                    neighbours
                        .iter()
                        .map(|(_, n)| n.position)
                        .chain(claimed.iter().copied()),
                );
                if near_trigger || occupied {
                    return None;
                }
                // Past an obstruction the next few metres must be open.
                if ahead
                    && let Some(sq) = spatial.as_ref()
                    && !way_open(
                        p - t * 3.0,
                        p + t * 10.0,
                        false,
                        1.3,
                        |from, dir, reach| {
                            sq.cast_ray_predicate(
                                from,
                                Dir3::new(dir).ok()?,
                                reach,
                                true,
                                &filter,
                                &solid,
                            )
                            .map(|hit| (hit.distance, hit.normal))
                        },
                        |_| None,
                    )
                {
                    return None;
                }
                let seated = match spatial.as_ref() {
                    Some(sq) => seat_reanchor(
                        sq,
                        &is_static,
                        water.as_deref(),
                        entity,
                        &vehicle.config,
                        p,
                        yaw,
                        lift,
                    )?,
                    None => p + Vec3::Y * lift,
                };
                Some((seated, yaw, arc, ahead))
            });
            match landing {
                Some((pose, yaw, arc, ahead)) => {
                    resets.write(ResetVehicle {
                        entity: Some(entity),
                        position: pose,
                        yaw,
                    });
                    if ahead {
                        neural.pass_to(arc, pose);
                    } else {
                        neural.reseat(pose);
                    }
                    neural.restart_idle();
                    d.stuck_pos = pose;
                    d.reanchors += 1;
                    claimed.push(pose);
                    info!(
                        bot = ?me,
                        reanchors = d.reanchors,
                        from = ?pos.0,
                        to = ?pose,
                        "cops and robbers bot re-anchored onto its line after a bounded stuck"
                    );
                    continue;
                }
                None => {
                    d.refused += 1;
                    warn!(
                        bot = ?me,
                        refused = d.refused,
                        at = ?pos.0,
                        "cops and robbers bot re-anchor refused: no landing clear of the triggers"
                    );
                }
            }
        }

        let still = matches!(
            objective,
            Objective::AcquireLooseGold { .. }
                | Objective::DeliverOwnGold { .. }
                | Objective::Guard { .. }
        );
        let cap = if still && (target.x - pos.0.x).hypot(target.z - pos.0.z) < APPROACH_RANGE {
            APPROACH_THROTTLE
        } else {
            1.0
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
        let others: Vec<Neighbour> = neighbours
            .iter()
            .filter(|(e, _)| *e != entity)
            .map(|(_, n)| *n)
            .collect();
        let water = water.as_deref();
        *input = neural.drive(
            policy,
            &mut scratch,
            &motion,
            &others,
            cap,
            |origin, dir, reach| {
                let sq = spatial.as_ref()?;
                let unit = Dir3::new(dir).ok()?;
                let hit = sq.cast_ray_predicate(origin, unit, reach, true, &filter, &|e| {
                    !field.contains(e)
                })?;
                let point = origin + dir * hit.distance;
                (!water.is_some_and(|w| w.is_deadly(point))).then_some((hit.distance, hit.normal))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::cnr_tactics::Objective;

    fn driver_on(line: &[Vec3], goal: Goal, planned_for: Vec3, kind: LineKind) -> CnrBotDriver {
        let guide = Arc::new(Guide::new(line.iter().copied(), false).unwrap());
        let config = mm2_vehicle::VehicleConfig::default();
        CnrBotDriver {
            neural: Some(NeuralState::new(guide, &config, line[0], 1)),
            goal: Some(goal),
            planned_for,
            kind: Some(kind),
            since_plan: 1000,
            ..default()
        }
    }

    fn gold_at(at: Vec3) -> Objective {
        Objective::AcquireLooseGold { at }
    }

    #[test]
    fn a_line_without_a_graph_is_straight_and_runs_past_its_objective() {
        let (line, kind) = plan_line(None, Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), false);
        assert_eq!(kind, LineKind::Fallback);
        assert_eq!(line.len(), 3);
        assert_eq!(
            *line.last().unwrap(),
            Vec3::new(100.0 + OVERSHOOT, 0.0, 0.0)
        );
        let (_, kind) = plan_line(None, Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), true);
        assert_eq!(kind, LineKind::Direct);
    }

    #[test]
    fn a_still_target_on_a_good_line_is_not_replanned() {
        let line = [Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0)];
        let target = Vec3::new(200.0, 0.0, 0.0);
        let goal = goal_of(&gold_at(target));
        let mut d = driver_on(&line, goal, target, LineKind::Street);
        let here = Vec3::new(0.0, 0.0, 3.0);
        d.neural.as_mut().unwrap().track(here);
        assert_eq!(needs_replan(&d, goal, target, here), None);
    }

    #[test]
    fn replanning_is_rate_limited_and_follows_the_triggers() {
        let line = [Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0)];
        let target = Vec3::new(200.0, 0.0, 0.0);
        let goal = goal_of(&gold_at(target));
        let mut d = driver_on(&line, goal, target, LineKind::Street);
        let moved = target + Vec3::new(0.0, 0.0, GOAL_DRIFT + 1.0);
        assert_eq!(
            needs_replan(&d, goal, moved, Vec3::ZERO),
            Some("target moved")
        );
        assert_eq!(
            needs_replan(&d, goal, target, Vec3::new(50.0, 0.0, OFF_ROUTE + 1.0)),
            Some("astray")
        );
        // Too soon after the last plan: nothing but a goal switch.
        d.since_plan = 5;
        assert_eq!(needs_replan(&d, goal, moved, Vec3::ZERO), None);
        let other = goal_of(&Objective::DeliverOwnGold { at: target });
        assert_eq!(
            needs_replan(&d, other, target, Vec3::ZERO),
            Some("goal switch")
        );
        // A chase of a different car is a different goal.
        let chase = |t| Objective::ChaseEnemyCarrier {
            target: PlayerId(t),
            at: target,
        };
        assert_ne!(goal_of(&chase(1)), goal_of(&chase(2)));
        assert_eq!(goal_of(&chase(1)), goal_of(&chase(1)));
    }

    #[test]
    fn a_failed_plan_waits_before_the_router_is_asked_again() {
        let mut d = CnrBotDriver::default();
        let target = Vec3::X * 50.0;
        let goal = goal_of(&gold_at(target));
        assert_eq!(
            needs_replan(&d, goal, target, Vec3::ZERO),
            Some("first line")
        );
        d.fail_wait = FAILED_WAIT;
        assert_eq!(needs_replan(&d, goal, target, Vec3::ZERO), None);
    }
}

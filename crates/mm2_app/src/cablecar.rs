//! San Francisco's cable cars: the one special actor the retail
//! executable builds from the AI map rather than a pathset.
//!
//! The session reads the city's `.bai`, finds the tram-line termini
//! ([`Bai::tram_termini`]), and puts one `va_cablecar_f` at each, on the
//! circuit the original's next-road rule gives it
//! ([`Bai::tram_circuit`]): out along the line, round at the far
//! terminus and home again. [`mm2_game::cablecar`] holds the route and
//! the speed controller; this module loads the model, spawns the
//! kinematic bodies and drives them each fixed step, gating each car's
//! road end through the ambient-traffic junction controller when the
//! session has one. Rules and constants: `docs/research/specials.md`.
//!
//! Obstacles: other cable cars on the circuit, and every participant
//! ([`Player`]: the driver and AI opponents) and ambient car standing in
//! the strip of track ahead ([`CableRoute::blocker_gap`]) are the
//! controller's obstacle, so the car brakes to 2.5 m behind them instead
//! of shoving them. Which bodies the original's sensor counts is not
//! recovered (UNK-44): this is the same rule applied to every car body.
//! A car already inside the junction between two roads does not brake
//! (the original's controller does not either). Ambient cars queue
//! behind a cable car the same way ([`RoadObstacle`]).
//!
//! Networked sessions ([`fields_cable_cars`]): the cars' stops follow the
//! host's signal clock and the cars on its rails, so only the host runs
//! them, and only in free-roam Cruise — the sessions that replicate the
//! host's ambient traffic. A client holds kinematic copies posed from the
//! traffic frame (`worldtraffic`, [`CAR_CABLE`](crate::worldtraffic::CAR_CABLE)
//! rows); it never drives one. Whether the original runs them in
//! multiplayer is unrecovered (UNK-44), so this is an enhanced policy.
//!
//! Not yet reproduced (stated, not hidden): the object audio, and the
//! original's init gate, which is unrecovered — eligible sessions always
//! spawn the cars.

use avian3d::prelude::{Position, SimpleCollider};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::bai::{Bai, TramCircuit, TramLeg, VehicleRule};
use mm2_game::cablecar::{
    CABLE_CAR_MODEL, CABLE_LOOKAHEAD, CABLE_OBSTACLE_RANGE, CableCorridor, CableMotion, CableRoute,
    CableSense,
};
use mm2_game::movers::mover_rotation;
use mm2_game::parked::ParkedRng;
use mm2_game::{
    JunctionGate, Player, SessionAuthority, SessionConfig, SessionEntity, SessionPhase,
};

use crate::city::{MovableModels, v3};
use crate::movers::{BodyQuery, pose_body, spawn_body};
use crate::traffic::{AmbientCar, AmbientTraffic, RoadObstacle, fields_ambient_traffic};

/// Whether this process runs the session's cable cars: a local session
/// does; a `Host` does in the free-roam Cruise that replicates its
/// ambient traffic ([`fields_ambient_traffic`]); a `Remote` client never
/// does — its cars are copies of the host's.
pub fn fields_cable_cars(config: &SessionConfig) -> bool {
    match config.authority {
        SessionAuthority::Local => true,
        SessionAuthority::Host => fields_ambient_traffic(config),
        SessionAuthority::Remote => false,
    }
}

/// The junction a leg's road ends at, as the ambient-traffic controller
/// names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CableJunction {
    /// Intersection index.
    pub intersection: u16,
    /// Road index (the controller's member id).
    pub road: u16,
    /// The authored rule at that road end.
    pub rule: Option<VehicleRule>,
}

/// One closed circuit and the junction each of its legs ends at.
#[derive(Debug, Clone)]
pub struct CableCircuit {
    /// The arc-length table the cars on this circuit drive.
    pub route: CableRoute,
    /// The tram legs, in driving order — parallel to `route.legs()`.
    pub legs: Vec<TramLeg>,
    /// Per leg, the junction it ends at (`None` for a road end the map
    /// does not wire to an intersection the controller knows).
    pub junctions: Vec<Option<CableJunction>>,
}

/// The session's circuits; cars index into it.
#[derive(Resource, Debug, Default, Clone)]
pub struct CableCircuits(pub Vec<CableCircuit>);

/// One cable car.
#[derive(Component, Debug, Clone)]
pub struct CableCar {
    /// Index into [`CableCircuits`].
    pub circuit: usize,
    /// Its speed controller and place on the circuit.
    pub motion: CableMotion,
    /// Height added to the curve so the bound's base rests on it
    /// ([`rest_lift`]).
    pub lift: f32,
    /// How far ahead of the model's origin its front is, m.
    pub nose: f32,
    /// How far behind the model's origin its tail is, m.
    pub tail: f32,
    /// Half the model's width, m — the track strip it blocks.
    pub half_width: f32,
}

/// What the session's cable-car init produced.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct CableReport {
    /// Whether the session was eligible (a local city session).
    pub eligible: bool,
    /// Tram-line termini the map has — the original creates one car at
    /// each.
    pub termini: usize,
    /// Termini with no drivable start (no tram curve in the direction).
    pub no_start: usize,
    /// Termini whose line never closes (a junction the original has no
    /// next road at), so no circuit is built.
    pub stranded: usize,
    /// Distinct circuits built.
    pub circuits: usize,
    /// Cars spawned.
    pub cars: usize,
    /// The map file failed to parse.
    pub bai_unreadable: bool,
    /// `va_cablecar_f` could not be loaded.
    pub model_missing: bool,
    /// Texture stems the model could not resolve.
    pub missing_textures: usize,
}

/// A junction the controller can gate: the road end's intersection, the
/// road, and its rule.
fn junction_of(bai: &Bai, leg: TramLeg) -> Option<CableJunction> {
    let road = bai.roads.get(leg.road)?;
    let end = if leg.forward { &road.end } else { &road.start };
    Some(CableJunction {
        intersection: u16::try_from(end.intersection).ok()?,
        road: u16::try_from(leg.road).ok()?,
        rule: end.vehicle_rule(),
    })
}

/// The route of a circuit: every leg's tram curve, in the world's frame.
fn circuit_route(bai: &Bai, circuit: &TramCircuit) -> Option<CableRoute> {
    let legs: Option<Vec<Vec<Vec3>>> = circuit
        .legs
        .iter()
        .map(|&leg| bai.tram_curve(leg).map(|c| c.into_iter().map(v3).collect()))
        .collect();
    CableRoute::new(&legs?)
}

/// The circuits one map's cable cars run, and where each car starts
/// (circuit index, leg index) — without touching the world, so the
/// coverage report and the tests share the spawn's grouping
/// ([`Bai::tram_plan`]). A circuit whose route cannot be tabulated
/// strands its cars.
pub fn plan_cable_cars(
    bai: &Bai,
    report: &mut CableReport,
) -> (Vec<CableCircuit>, Vec<(usize, usize)>) {
    let plan = bai.tram_plan();
    report.termini = plan.termini;
    report.no_start = plan.no_start;
    report.stranded = plan.stranded;
    let mut circuits = Vec::new();
    let mut placed = Vec::with_capacity(plan.circuits.len());
    for circuit in plan.circuits {
        let Some(route) = circuit_route(bai, &circuit) else {
            placed.push(None);
            continue;
        };
        placed.push(Some(circuits.len()));
        let junctions = circuit.legs.iter().map(|&l| junction_of(bai, l)).collect();
        circuits.push(CableCircuit {
            route,
            legs: circuit.legs,
            junctions,
        });
    }
    let mut starts = Vec::with_capacity(plan.cars.len());
    for (ci, leg) in plan.cars {
        match placed[ci] {
            Some(placed) => starts.push((placed, leg)),
            None => report.stranded += 1,
        }
    }
    report.circuits = circuits.len();
    (circuits, starts)
}

/// How far above its curve point the car's origin rides so the base of
/// its bound rests on the rail.
///
/// The tram curve is the road-surface datum (measured on retail: the
/// median height of the curve over the carriageway under it is 0.0 m,
/// docs/research/specials.md) and `va_cablecar_f` is authored with its
/// origin at the floor — mesh and bound both span y 0..3.3 — so the
/// body rides at the curve with no offset. The bound record's `CG.y`
/// (1.645, half the height) is the centre of mass, not an offset: lifting
/// by it held the car 1.6 m above the rails. `base_y` is the bound's
/// lowest point in model space; a model with no bound rides at its origin.
fn rest_lift(base_y: Option<f32>) -> f32 {
    base_y
        .filter(|y| y.is_finite())
        .map_or(0.0, |y| (-y).clamp(-REST_LIFT_LIMIT, REST_LIFT_LIMIT))
}

/// The most the bound's base may sit off the model origin before the
/// number is treated as garbage and ignored, m.
const REST_LIFT_LIMIT: f32 = 3.0;

/// Spawn the city's cable cars, session-owned. `eligible` is
/// [`fields_cable_cars`] (see the module notes).
#[allow(clippy::too_many_arguments)] // Bevy asset stores have to be threaded separately
pub fn spawn_cable_cars(
    commands: &mut Commands,
    vfs: &Vfs,
    city: &str,
    eligible: bool,
    seed: u64,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> CableReport {
    let mut report = CableReport {
        eligible,
        ..CableReport::default()
    };
    commands.insert_resource(CableCircuits::default());
    if !eligible {
        return report;
    }
    let Ok((bytes, _)) = vfs.read_path(&format!("city/{city}.bai")) else {
        return report;
    };
    let bai = match Bai::parse(&bytes) {
        Ok(b) => b,
        Err(e) => {
            warn!(city, error = %e, "cable cars: map unreadable");
            report.bai_unreadable = true;
            return report;
        }
    };
    let (circuits, starts) = plan_cable_cars(&bai, &mut report);
    if starts.is_empty() {
        return report;
    }
    let mut models = MovableModels::new(vfs, meshes, images, materials);
    let Some(model) = models.load(CABLE_CAR_MODEL, Vec3::ZERO) else {
        warn!(
            model = CABLE_CAR_MODEL,
            "cable car model unresolved; none spawned"
        );
        report.model_missing = true;
        report.missing_textures = models.finish(commands, owner).len();
        return report;
    };
    let lift = rest_lift(
        model
            .collider
            .as_ref()
            .map(|c| c.aabb(Vec3::ZERO, Quat::IDENTITY).min.y),
    );
    let nose = model
        .collider
        .as_ref()
        .map(|c| c.aabb(Vec3::ZERO, Quat::IDENTITY).max.z)
        .filter(|z| z.is_finite())
        .unwrap_or(4.0)
        .clamp(1.0, 12.0);
    let tail = model
        .collider
        .as_ref()
        .map(|c| -c.aabb(Vec3::ZERO, Quat::IDENTITY).min.z)
        .filter(|z| z.is_finite())
        .unwrap_or(nose)
        .clamp(1.0, 12.0);
    let half_width = model
        .collider
        .as_ref()
        .map(|c| c.aabb(Vec3::ZERO, Quat::IDENTITY).max.x)
        .filter(|x| x.is_finite())
        .unwrap_or(1.5)
        .clamp(0.5, 4.0);
    let mut rng = ParkedRng::new(seed ^ 0x6361_626c);
    for (i, &(ci, leg)) in starts.iter().enumerate() {
        let route = &circuits[ci].route;
        let draw = rng.next_roll() as f32 / 32768.0;
        let motion = CableMotion::new(route, leg, draw);
        let (pos, dir) = route.pose(motion.s);
        let transform =
            Transform::from_translation(pos + Vec3::Y * lift).with_rotation(mover_rotation(dir));
        let body = spawn_body(commands, &model, transform, owner, format!("cablecar-{i}"));
        commands.entity(body).insert(CableCar {
            circuit: ci,
            motion,
            lift,
            nose,
            tail,
            half_width,
        });
        commands.entity(body).insert(RoadObstacle { nose, tail });
        report.cars += 1;
    }
    report.missing_textures = models.finish(commands, owner).len();
    commands.insert_resource(CableCircuits(circuits));
    info!(
        termini = report.termini,
        circuits = report.circuits,
        cars = report.cars,
        "cable cars spawned"
    );
    report
}

/// Advance every cable car one fixed step: sense (the junction gate on
/// its road end, the car ahead on its circuit), run the controller, and
/// pose the body where it now is, leaving the velocity that reaches the
/// next step's pose.
#[allow(clippy::type_complexity)] // Bevy system: the queries are the system's signature
pub fn drive_cable_cars(
    session: Res<mm2_game::Session>,
    time: Res<Time<Fixed>>,
    circuits: Res<CableCircuits>,
    mut traffic: Option<ResMut<AmbientTraffic>>,
    mut cars: Query<(Entity, &mut CableCar, BodyQuery)>,
    players: Query<&Position, (With<Player>, Without<CableCar>)>,
    ambient: Query<&Position, (With<AmbientCar>, Without<CableCar>)>,
) {
    let running = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    );
    let dt = if running { time.delta_secs() } else { 0.0 };
    // Where every car stands, so each can find the one ahead of it.
    let others: Vec<(Entity, usize, f32, f32)> = cars
        .iter()
        .map(|(e, car, _)| (e, car.circuit, car.motion.s, car.nose))
        .collect();
    // Cars the tram must not run into: every participant and every
    // ambient car. Collected once; each tram only looks at the few near it.
    let blockers: Vec<Vec3> = players
        .iter()
        .chain(ambient.iter())
        .map(|p| p.0)
        .filter(|p| p.is_finite())
        .collect();
    let mut near: Vec<Vec3> = Vec::new();
    for (entity, mut car, (mut pos, mut rot, mut lin, mut ang)) in &mut cars {
        let Some(circuit) = circuits.0.get(car.circuit) else {
            continue;
        };
        let route = &circuit.route;
        let lift = Vec3::Y * car.lift;
        let (p0, d0) = route.pose(car.motion.s);
        if dt > 0.0 {
            let nose = car.nose;
            let to_line = car.motion.distance_to_line(route, nose);
            let gate_open = match (
                to_line.filter(|&d| d < CABLE_LOOKAHEAD),
                circuit.junctions.get(car.motion.leg()).copied().flatten(),
                traffic.as_deref_mut(),
            ) {
                (Some(d), Some(j), Some(traffic)) => {
                    let (junctions, graph) = traffic.junctions_and_graph();
                    junctions.gate_approach(
                        graph,
                        (j.intersection, j.road, j.rule),
                        entity,
                        d <= 1.0,
                        car.motion.speed < 0.05,
                        false,
                    ) == JunctionGate::Open
                }
                // No controller (no ambient traffic this session) or an
                // unwired road end: nothing to wait for.
                _ => true,
            };
            // The nearest car ahead on this circuit, front to tail.
            let ahead_car = others
                .iter()
                .filter(|&&(e, c, ..)| e != entity && c == car.circuit)
                .map(|&(_, _, s, n)| route.gap_ahead(car.motion.s, s) - nose - n)
                .filter(|g| (0.0..CABLE_OBSTACLE_RANGE).contains(g))
                .min_by(f32::total_cmp);
            // The nearest participant or ambient car on the rails ahead.
            let reach = nose + CABLE_OBSTACLE_RANGE;
            near.clear();
            near.extend(
                blockers
                    .iter()
                    .copied()
                    .filter(|b| b.distance_squared(pos.0) <= (reach + 10.0).powi(2)),
            );
            let corridor = CableCorridor::for_car(car.half_width, car.lift);
            let ahead_body =
                route.blocker_gap(car.motion.s + nose, CABLE_OBSTACLE_RANGE, &corridor, &near);
            let obstacle = match (ahead_car, ahead_body) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let leg_before = car.motion.leg();
            car.motion.step(
                route,
                dt,
                nose,
                CableSense {
                    gate_open,
                    obstacle,
                },
            );
            if car.motion.leg() != leg_before
                && let Some(traffic) = traffic.as_deref_mut()
            {
                traffic.junctions.depart(entity);
            }
        }
        let (p1, d1) = route.pose(car.motion.s);
        pose_body(
            (&mut pos, &mut rot, &mut lin, &mut ang),
            (p0 + lift, mover_rotation(d0)),
            (p1 + lift, mover_rotation(d1)),
            dt,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_car_rides_so_the_base_of_its_bound_rests_on_the_rail() {
        // The retail tram: bound from y 0.0014 up — origin at the floor.
        assert!(rest_lift(Some(0.0013837814)).abs() < 0.01);
        // A bound centred on its origin would need lifting by half its height.
        assert_eq!(rest_lift(Some(-1.5)), 1.5);
        // No bound, or a nonsense one: ride at the origin / ignore the offset.
        assert_eq!(rest_lift(None), 0.0);
        assert_eq!(rest_lift(Some(f32::NAN)), 0.0);
        assert_eq!(rest_lift(Some(1e9)), -REST_LIFT_LIMIT);
    }
}

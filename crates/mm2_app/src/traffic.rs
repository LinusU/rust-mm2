//! F10-A.2 ambient traffic runtime — the consumer of the seeded spawn
//! plan F10-A.1 landed.
//!
//! `load_session_world` calls [`load_ambient_traffic`] once per city
//! session: it merges the city's `city/<stem>.aimap` ambient config
//! with the event aimap's overrides (`mm2_content::ambient_setup`),
//! builds the navigation graph, runs `plan_ambient` under the session
//! seed and spawns the planned cars as session-owned entities on real
//! `va_*` assets resolved through the VFS. `drive_ambient` then walks
//! each car's [`LaneCursor`] along the authored network at its target
//! speed — seeded legal-exit choices at intersections, rank-preserving
//! lane transfers — and `maintain_ambient` recycles cars that run out
//! of road or leave the player's bubble, respawning to the density
//! target.
//!
//! Deliberately partial scope, per the F10 spec: ambient cars are
//! **kinematic** lane followers — `AiVehicleData` authors no drivetrain
//! (its own doc says it is not `vehCarSim` input), so the car is a
//! rigid hull moved by lane sampling, which also keeps every pose
//! finite and on-road by construction. The kinematic collider blocks
//! the player coherently, and F10-B.1 adds the obstruction response: a
//! forward corridor senses every `Player` participant and other
//! ambient cars, and the follow law brakes to a bounded stop behind
//! the nearest blocker — queueing, never shoving — and resumes when
//! it clears. Lane-change passing and stuck recovery beyond the
//! wait-and-recycle bound stay open (F10-B/F10-C remainder), and
//! dynamic car-vs-player crash fidelity (AC03) is not claimed — the
//! follower stops short, nothing more.
//!
//! F10-B.2 adds the junction controller: the authored `vehicleRule`
//! on each road end (BAI) gates the lane transfer — `NeverStop`
//! approaches flow, `TrafficLight` approaches wait for their road's
//! phase in a deterministic per-junction signal cycle, `StopSign`
//! approaches queue first-come-first-served through a registered
//! dwell, and `AlwaysStop` never opens (the two last are unused on
//! retail data). "At the stop line" is a small designed tolerance —
//! the brake ramp decays the remaining distance geometrically, so an
//! f32 cursor asymptotes a hair short of an exact zero — keying the
//! FCFS registration, the held diagnostic and the last-step clamp.
//! A transfer that would land inside a live car or
//! participant reverts to the lane end and retries — cars never
//! materialise inside a junction queue. Signal timing, dwells,
//! stop-line inset and entry clearance are designed values (the
//! original's are unverified, UNK-12).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::veh::AiVehicleData;
use mm2_game::{
    AmbientRoster, AmbientSpec, AuthorityRole, FollowPolicy, JunctionGate, Junctions, LaneAdvance,
    LaneCursor, LaneId, NavGraph, NavOverrides, NavRng, ObjectIdentity, Player, Session,
    SessionConfig, SessionEntity, SessionPhase, SpawnDirective, SpawnDraw, SpawnPolicy, WorldMode,
    advance_lane_cursor, corridor_gap, draw_spawn, eligible_lanes, follow_speed, junction_speed,
    plan_ambient,
};
use tracing::{info, warn};

use crate::car_visual::spawn_vehicle_model;

/// A `va_*` class's runtime assets: render model plus the collider the
/// bound (or, failing that, the tuning's authored `Size`) describes.
struct AmbientClass {
    model: mm2_content::VehicleModel,
    collider: Collider,
}

/// Session-scoped ambient-traffic state: the merged roster, the nav
/// graph the cars follow, the seeded draw stream the initial plan and
/// the respawner share, and the accounting the smoke record reports.
/// Inserted by `load_session_world` for city sessions that resolve a
/// roster; removed by session teardown so a restart replans cleanly.
#[derive(Resource)]
pub struct AmbientTraffic {
    graph: NavGraph,
    overrides: NavOverrides,
    roster: AmbientRoster,
    eligible: Vec<LaneId>,
    rng: NavRng,
    policy: SpawnPolicy,
    /// Simultaneous population bound for this session's density
    /// (`density × policy.max_active`, the plan's `target`).
    pub target: usize,
    /// Per-class assets, loaded lazily on first successful draw —
    /// `None` caches a failed load so it is not retried every tick.
    classes: HashMap<usize, Option<Arc<AmbientClass>>>,
    /// Cars ever spawned (initial plan + respawns).
    pub spawned: usize,
    /// Cars despawned past `policy.recycle_distance`.
    pub recycled: usize,
    /// Cars that ran out of road (no legal open exit).
    pub dead_ends: usize,
    /// Draws that selected an unspawnable class or fell off an open
    /// weight table — the authored band stood.
    pub unspawnable: usize,
    /// Initial-plan directives dropped outside the spawn annulus.
    pub dropped: usize,
    /// Cars currently held at a stop behind a corridor blocker —
    /// refreshed every `drive_ambient` tick.
    pub queued: usize,
    /// Cars standing at a closed junction gate's stop line (signal
    /// red, stop-sign dwell/queue, `AlwaysStop`) — refreshed every
    /// `drive_ambient` tick.
    pub junction_held: usize,
    /// The per-junction right-of-way/signal controller (F10-B.2) —
    /// session-scoped like the plan it polices.
    pub junctions: Junctions,
    /// Planner/setup problems, reported honestly.
    pub issues: Vec<String>,
}

impl AmbientTraffic {
    /// The navigation graph the cars follow — exposed for diagnostics
    /// and tests that resolve a [`LaneCursor`] to a world pose.
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }
}

/// One ambient car on the network.
#[derive(Component)]
pub struct AmbientCar {
    /// Roster index the class draw selected.
    pub class: usize,
    /// Position on the authored network (travel-direction distance).
    pub cursor: LaneCursor,
    /// The current road's effective speed — refreshed on every turn so
    /// per-road exception limits apply.
    pub target_speed: f32,
    /// The kinematic speed the car actually travels this tick —
    /// `target_speed` on a clear corridor, braked down to a bounded
    /// stop by the obstruction sense (F10-B.1).
    pub speed: f32,
}

/// Load the ambient setup for this session and spawn the initial plan.
/// Returns the resource the caller inserts — `None` when the session
/// is not authoritative, the world is not a city, or no layer authors
/// a roster. All failures log and degrade to *no* ambient traffic;
/// ambient cars never sink an otherwise loadable session.
///
/// `authored_density` is the event-table `Ambient` dial for event
/// sessions (`RaceDefinition::params.densities.traffic`); cruise
/// passes `None`. The density chain is most-specific-authored-first:
/// the event aimap's `[Density]`, then the authored table dial, then
/// the city aimap's `[Density]`, then `SessionConfig::densities`
/// (implementation choice — the original layering is unverified,
/// UNK-12).
#[allow(clippy::too_many_arguments)] // session-load call site: assets + vfs + session all live here
pub fn load_ambient_traffic(
    commands: &mut Commands,
    vfs: &Vfs,
    config: &SessionConfig,
    event_aimap: Option<&Aimap>,
    authored_density: Option<f32>,
    owner: SessionEntity,
    session: &mut Session,
    player_at: Vec3,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<AmbientTraffic> {
    // Ambient traffic is authority state — a predicted remote session
    // mirrors it; it never simulates its own.
    if !session.authority_role().is_authority() {
        return None;
    }
    let WorldMode::City { psdl } = &config.world else {
        return None;
    };
    let stem = Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());

    let setup = match mm2_content::ambient_setup(vfs, stem, event_aimap) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "ambient: city aimap failed — no ambient traffic");
            return None;
        }
    };
    let setup = setup?;
    for d in &setup.diagnostics {
        warn!(diagnostic = %d, "ambient: roster diagnostic");
    }
    let build = match mm2_content::load_nav_graph(vfs, stem) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "ambient: nav graph failed — no ambient traffic");
            return None;
        }
    };

    let density = setup
        .event_density
        .or(authored_density)
        .or(setup.city_density)
        .unwrap_or(config.densities.traffic);
    let policy = SpawnPolicy::default();
    let plan = plan_ambient(
        &build.graph,
        &setup.overrides,
        &setup.roster,
        config.seed,
        density,
        player_at.to_array(),
        &policy,
    );

    let role = session.authority_role();
    let mut traffic = AmbientTraffic {
        eligible: eligible_lanes(&build.graph, &setup.overrides),
        graph: build.graph,
        // A second seeded stream for runtime draws (turn choices and
        // respawn placements) — same seed, never interleaved with the
        // plan's stream (implementation choice).
        rng: NavRng::new(config.seed.wrapping_add(0x4d4d_325f)),
        overrides: setup.overrides,
        roster: setup.roster,
        policy,
        target: plan.target,
        classes: HashMap::new(),
        spawned: 0,
        recycled: 0,
        dead_ends: 0,
        unspawnable: plan.unspawnable,
        dropped: plan.dropped,
        queued: 0,
        junction_held: 0,
        junctions: Junctions::default(),
        issues: plan
            .issues
            .iter()
            .map(|i| i.to_string())
            .chain(build.issues.iter().map(|i| i.to_string()))
            .collect(),
    };
    for directive in &plan.spawns {
        if spawn_ambient_car(
            commands,
            vfs,
            &mut traffic,
            directive,
            owner,
            role,
            session,
            meshes,
            images,
            materials,
        )
        .is_some()
        {
            traffic.spawned += 1;
        }
    }
    for i in &traffic.issues {
        warn!(issue = %i, "ambient issue");
    }
    info!(
        density,
        target = traffic.target,
        spawned = traffic.spawned,
        eligible = traffic.eligible.len(),
        issues = traffic.issues.len(),
        "ambient traffic loaded"
    );
    Some(traffic)
}

/// Resolve a class's assets once and cache the outcome. A class whose
/// model or collider cannot be produced caches `None` — it counts
/// unspawnable like a tuning-less row rather than retrying every tick.
fn class_assets(vfs: &Vfs, spec: &AmbientSpec) -> Option<AmbientClass> {
    let loaded = mm2_content::ambient_vehicle(vfs, &spec.id).ok()?;
    let collider = loaded
        .bound
        .as_ref()
        .and_then(|b| {
            Collider::convex_hull(b.verts.iter().map(|v| Vec3::from(*v)).collect::<Vec<_>>())
        })
        .unwrap_or_else(|| size_collider(spec.tuning.as_ref()));
    Some(AmbientClass {
        model: loaded.model,
        collider,
    })
}

/// Fallback hull over the tuning's authored `Size` centred on its `CG`
/// (the bound-centre offset) — used when `<id>_bound.bnd` never
/// resolves or does not parse. The corners bake the offset into the
/// hull, matching how the authored bound's verts carry it.
fn size_collider(tuning: Option<&AiVehicleData>) -> Collider {
    let (size, cg) = tuning.map_or_else(
        || (Vec3::new(2.0, 1.5, 5.0), Vec3::new(0.0, 0.75, 0.0)),
        |t| {
            (
                Vec3::from(t.size),
                Vec3::from(t.cg.unwrap_or([0.0, t.size[1] * 0.5, 0.0])),
            )
        },
    );
    let h = size * 0.5;
    let corners: Vec<Vec3> = [-1.0f32, 1.0]
        .iter()
        .flat_map(|&x| {
            [-1.0f32, 1.0].iter().flat_map(move |&y| {
                [-1.0f32, 1.0]
                    .iter()
                    .map(move |&z| cg + Vec3::new(h.x * x, h.y * y, h.z * z))
            })
        })
        .collect();
    Collider::convex_hull(corners).unwrap_or_else(|| Collider::cuboid(size.x, size.y, size.z))
}

/// Spawn one planned car as a session-owned kinematic rigid body —
/// real collider, real render model — on its authored lane pose. The
/// face direction comes from the lane tangent (travel direction), so
/// backward-arc lanes orient correctly.
#[allow(clippy::too_many_arguments)] // spawn call site threads the same stores the session load does
fn spawn_ambient_car(
    commands: &mut Commands,
    vfs: &Vfs,
    traffic: &mut AmbientTraffic,
    directive: &SpawnDirective,
    owner: SessionEntity,
    role: AuthorityRole,
    session: &mut Session,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<Entity> {
    let spec = traffic.roster.entries.get(directive.class)?;
    let tuning = spec.tuning.as_ref()?;
    let class = traffic
        .classes
        .entry(directive.class)
        .or_insert_with(|| class_assets(vfs, spec).map(Arc::new))
        .clone()?;
    let pos = Vec3::from(directive.sample.position);
    if !pos.is_finite() {
        warn!(class = %spec.id, "ambient: non-finite spawn pose — skipped");
        return None;
    }
    let tangent = Vec3::from(directive.sample.tangent).normalize_or(Vec3::NEG_Z);
    let yaw = (-tangent.x).atan2(-tangent.z);
    let pitch = tangent.y.clamp(-1.0, 1.0).asin();
    let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    let entity = commands
        .spawn((
            owner,
            ObjectIdentity(session.mint_object_id()),
            role,
            AmbientCar {
                class: directive.class,
                cursor: LaneCursor {
                    lane: directive.lane,
                    along: directive.along,
                },
                target_speed: directive.target_speed,
                speed: directive.target_speed.max(0.0),
            },
            RigidBody::Kinematic,
            class.collider.clone(),
            Friction::new(tuning.friction),
            Restitution::new(tuning.elasticity),
            Position(pos),
            Rotation(rot),
            LinearVelocity(tangent * directive.target_speed.max(0.0)),
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
            TransformInterpolation,
            Visibility::Visible,
        ))
        .id();
    let missing = spawn_vehicle_model(
        commands,
        vfs,
        &class.model,
        0,
        meshes,
        images,
        materials,
        entity,
    );
    if !missing.is_empty() {
        warn!(class = %spec.id, "ambient: missing textures: {}", missing.join(", "));
    }
    Some(entity)
}

/// Advance every ambient car along its lane by `speed × dt`,
/// re-posing it from the sampled lane each fixed tick. Runs in
/// `FixedLast` — after the physics step consumed the previous pose —
/// so the written `Position`/`Rotation` is what the next solver step
/// and the renderer both see, and `LinearVelocity` reports the surface
/// velocity contacts resolve against.
///
/// F10-B.1 obstruction response: before advancing, each car senses a
/// forward corridor (`corridor_gap`) against every `Player`
/// participant — the local driver and AI opponents alike — and every
/// other ambient car, then the `follow_speed` law sets its kinematic
/// `speed`: road limit on a clear corridor, a bounded brake to
/// `follow_gap` behind a blocker, an outright stop inside
/// `panic_gap`, and a `turn_speed` cap across intersections. A queued
/// car waits — a kinematic body that kept moving would shove whatever
/// blocks it — and resumes when the corridor clears. `traffic.queued`
/// reports the held count each tick.
///
/// A car that runs out of road despawns; `maintain_ambient` decides
/// whether a replacement spawns.
#[allow(clippy::type_complexity)] // Bevy system: the two queries are the system's actual signature
pub fn drive_ambient(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    traffic: Option<ResMut<AmbientTraffic>>,
    mut cars: Query<
        (
            Entity,
            &mut AmbientCar,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut Transform,
        ),
        Without<Player>,
    >,
    players: Query<(Entity, &Position), (With<Player>, Without<AmbientCar>)>,
    mut commands: Commands,
) {
    let Some(mut traffic) = traffic else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        return;
    }
    let traffic = &mut *traffic;
    let dt = time.delta_secs();
    // Corridor blockers: every participant plus every ambient car —
    // the follower queues behind whatever sits on its lane without
    // distinguishing who it is.
    let mut blockers: Vec<(Entity, Vec3)> = players.iter().map(|(e, p)| (e, p.0)).collect();
    blockers.extend(cars.iter().map(|(e, _, p, _, _, _)| (e, p.0)));
    let follow = FollowPolicy::default();
    let jpolicy = traffic.junctions.policy;
    // The controller's clock ticks with the driver, then sheds queue
    // entries whose cars the recycler collected since last tick.
    traffic.junctions.advance_tick();
    {
        let live: BTreeSet<Entity> = cars.iter().map(|(e, ..)| e).collect();
        traffic.junctions.retain(&live);
    }
    let mut queued = 0usize;
    let mut junction_held = 0usize;
    for (entity, mut car, mut position, mut rotation, mut velocity, mut transform) in
        cars.iter_mut()
    {
        let fwd = rotation.0 * Vec3::NEG_Z;
        let reach = follow.near + follow.lead * car.speed.max(0.0);
        let gap = corridor_gap(
            position.0.to_array(),
            fwd.to_array(),
            follow.half_width,
            follow.max_rise,
            reach,
            blockers
                .iter()
                .filter(|(e, _)| *e != entity)
                .map(|(_, p)| p.to_array()),
        );
        car.speed = follow_speed(car.speed, car.target_speed, gap, dt, &follow);
        if gap.is_some() && car.speed <= follow.held_speed {
            queued += 1;
        }
        // Junction gate (F10-B.2): the authored `vehicleRule` at the
        // arc's downstream end decides whether the car may pass the
        // lane end this tick — signal red, stop-sign queue/dwell and
        // `AlwaysStop` close it, everything else opens it. A closed
        // gate brakes to the stop line and never lets the cursor past
        // it; a stopped stop-sign car registers in the FCFS queue.
        let dist_to_stop = traffic
            .graph
            .lane(car.cursor.lane)
            .map(|l| l.length - jpolicy.stop_inset - car.cursor.along)
            .unwrap_or(f32::MAX);
        // "At the line" is the policy tolerance, never an exact zero:
        // the brake ramp decays `dist_to_stop` geometrically, so the
        // f32 cursor asymptotes a hair short of the line and would
        // otherwise never register, never open a stop-sign queue, and
        // never report held.
        let at_line = dist_to_stop <= jpolicy.stop_line_tolerance;
        let gate = traffic.junctions.gate(
            &traffic.graph,
            car.cursor.lane,
            entity,
            at_line,
            car.speed <= follow.held_speed,
        );
        car.speed = junction_speed(car.speed, dist_to_stop, gate, dt, &jpolicy);
        if gate == JunctionGate::Closed && at_line && car.speed <= follow.held_speed {
            junction_held += 1;
        }
        let mut ds = car.speed.max(0.0) * dt;
        if gate == JunctionGate::Closed {
            // Inside the tolerance the residual is below the ramp's
            // f32 resolution — close it outright so the car stands on
            // the line; outside it, never step past the line.
            ds = if at_line {
                dist_to_stop.max(0.0)
            } else {
                ds.min(dist_to_stop)
            };
        }
        let previous = car.cursor;
        let step = if ds > 0.0 {
            advance_lane_cursor(
                &traffic.graph,
                &traffic.overrides,
                &mut car.cursor,
                ds,
                &mut traffic.rng,
            )
        } else {
            LaneAdvance::Along
        };
        if step == LaneAdvance::DeadEnd {
            traffic.dead_ends += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        }
        if step == LaneAdvance::Turned {
            // Occupied-transfer check (F10-AC04's junction leg): a
            // landing inside `enter_clearance` of a live blocker would
            // materialise the car inside a junction queue — revert to
            // the lane end and retry next tick.
            let landing_occupied = traffic
                .graph
                .sample_lane(car.cursor.lane, car.cursor.along)
                .is_some_and(|s| {
                    let p = Vec3::from(s.position);
                    blockers
                        .iter()
                        .any(|(e, b)| *e != entity && b.distance(p) < jpolicy.enter_clearance)
                });
            if landing_occupied {
                car.cursor = previous;
                car.speed = 0.0;
            } else {
                traffic.junctions.depart(entity);
                if let Some(road) = traffic.graph.road(car.cursor.lane.road) {
                    car.target_speed = traffic.overrides.effective_speed(road);
                }
                // Corner braking stand-in: an intersection turn is
                // never taken at full road speed.
                car.speed = car.speed.min(follow.turn_speed);
            }
        }
        let Some(sample) = traffic.graph.sample_lane(car.cursor.lane, car.cursor.along) else {
            traffic.dead_ends += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        };
        let pos = Vec3::from(sample.position);
        if !pos.is_finite() {
            traffic.dead_ends += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        }
        let tangent = Vec3::from(sample.tangent).normalize_or(Vec3::NEG_Z);
        let yaw = (-tangent.x).atan2(-tangent.z);
        let pitch = tangent.y.clamp(-1.0, 1.0).asin();
        let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        // The *intended* surface velocity, never the measured delta:
        // Avian integrates kinematic bodies from `LinearVelocity`, so
        // a delta measured across the physics step feeds back on
        // itself and diverges — and a contact resolving against it
        // would see a phantom ~km/s impactor.
        velocity.0 = tangent * car.speed.max(0.0);
        position.0 = pos;
        rotation.0 = rot;
        *transform = Transform::from_translation(pos).with_rotation(rot);
    }
    traffic.queued = queued;
    traffic.junction_held = junction_held;
}

/// Keep the population at the plan's target: despawn cars that left
/// the player's bubble past `recycle_distance`, then draw fresh
/// placements through the same [`draw_spawn`] the planner used —
/// bounded per tick so a drained city cannot stall the frame.
#[allow(clippy::too_many_arguments)] // Bevy system: respawning threads the same asset stores the session load does
pub fn maintain_ambient(
    mut commands: Commands,
    mut session: ResMut<Session>,
    traffic: Option<ResMut<AmbientTraffic>>,
    vfs: Option<Res<mm2_game::Mm2Vfs>>,
    mut cars: Query<(Entity, &AmbientCar, &Position)>,
    player: Query<&Position, With<mm2_game::PlayerVehicle>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (Some(mut traffic), Some(vfs)) = (traffic, vfs) else {
        return;
    };
    // Same live-phase gate `drive_ambient` runs under — a paused or
    // resolved session freezes its population instead of churning
    // respawns behind the overlay.
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        return;
    }
    let Some(player_at) = player.iter().next().map(|p| p.0) else {
        return;
    };
    let traffic = &mut *traffic;

    let recycle2 = traffic.policy.recycle_distance * traffic.policy.recycle_distance;
    let mut active = 0usize;
    for (entity, _, pos) in &mut cars {
        if pos.0.distance_squared(player_at) > recycle2 {
            traffic.recycled += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
        } else {
            active += 1;
        }
    }
    if active >= traffic.target || traffic.eligible.is_empty() {
        return;
    }
    // Recycled slots refill through the same draw — the respawner
    // never out-attempts the planner's own bound.
    let role = session.authority_role();
    let owner = SessionEntity(session.generation());
    for _ in 0..traffic.policy.placement_attempts {
        if active >= traffic.target {
            break;
        }
        match draw_spawn(
            &traffic.graph,
            &traffic.overrides,
            &traffic.eligible,
            &traffic.roster,
            &mut traffic.rng,
            player_at.to_array(),
            &traffic.policy,
        ) {
            SpawnDraw::OutOfBand => continue,
            SpawnDraw::Unspawnable(_) => {
                traffic.unspawnable += 1;
                break;
            }
            SpawnDraw::Placed(directive) => {
                if spawn_ambient_car(
                    &mut commands,
                    &vfs.0,
                    traffic,
                    &directive,
                    owner,
                    role,
                    session.as_mut(),
                    &mut meshes,
                    &mut images,
                    &mut materials,
                )
                .is_some()
                {
                    traffic.spawned += 1;
                    active += 1;
                } else {
                    traffic.unspawnable += 1;
                }
            }
        }
    }
}

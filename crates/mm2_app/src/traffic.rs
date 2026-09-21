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
//! the player coherently, but there is no intersection controller,
//! signal or right-of-way handling, obstruction response, queueing or
//! stuck recovery yet (F10-B/F10-C), and car-vs-player contact
//! fidelity (AC03) is not claimed — the hull blocks, nothing more.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::veh::AiVehicleData;
use mm2_game::{
    AmbientRoster, AmbientSpec, AuthorityRole, LaneAdvance, LaneCursor, LaneId, NavGraph,
    NavOverrides, NavRng, ObjectIdentity, Session, SessionConfig, SessionEntity, SessionPhase,
    SpawnDirective, SpawnDraw, SpawnPolicy, WorldMode, advance_lane_cursor, draw_spawn,
    eligible_lanes, plan_ambient,
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
    /// Initial-plan directives dropped inside the player bubble.
    pub dropped: usize,
    /// Planner/setup problems, reported honestly.
    pub issues: Vec<String>,
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

/// Advance every ambient car along its lane by `target_speed × dt`,
/// re-posing it from the sampled lane each fixed tick. Runs in
/// `FixedLast` — after the physics step consumed the previous pose —
/// so the written `Position`/`Rotation` is what the next solver step
/// and the renderer both see, and `LinearVelocity` reports the surface
/// velocity contacts resolve against. A car that runs out of road
/// despawns; `maintain_ambient` decides whether a replacement spawns.
pub fn drive_ambient(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    traffic: Option<ResMut<AmbientTraffic>>,
    mut cars: Query<(
        Entity,
        &mut AmbientCar,
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut Transform,
    )>,
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
    for (entity, mut car, mut position, mut rotation, mut velocity, mut transform) in &mut cars {
        let ds = car.target_speed.max(0.0) * dt;
        let step = advance_lane_cursor(
            &traffic.graph,
            &traffic.overrides,
            &mut car.cursor,
            ds,
            &mut traffic.rng,
        );
        if step == LaneAdvance::DeadEnd {
            traffic.dead_ends += 1;
            commands.entity(entity).despawn();
            continue;
        }
        if step == LaneAdvance::Turned
            && let Some(road) = traffic.graph.road(car.cursor.lane.road)
        {
            car.target_speed = traffic.overrides.effective_speed(road);
        }
        let Some(sample) = traffic.graph.sample_lane(car.cursor.lane, car.cursor.along) else {
            traffic.dead_ends += 1;
            commands.entity(entity).despawn();
            continue;
        };
        let pos = Vec3::from(sample.position);
        if !pos.is_finite() {
            traffic.dead_ends += 1;
            commands.entity(entity).despawn();
            continue;
        }
        let tangent = Vec3::from(sample.tangent).normalize_or(Vec3::NEG_Z);
        let yaw = (-tangent.x).atan2(-tangent.z);
        let pitch = tangent.y.clamp(-1.0, 1.0).asin();
        let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        velocity.0 = (pos - position.0) / dt;
        position.0 = pos;
        rotation.0 = rot;
        *transform = Transform::from_translation(pos).with_rotation(rot);
    }
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
    if !session.authority_role().is_authority() {
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
            traffic.policy.min_player_distance,
        ) {
            SpawnDraw::InsideBubble => continue,
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

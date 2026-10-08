//! Moving scenery: the retail sailboat (tugs, water taxis, ducks),
//! ferry and Underground managers.
//!
//! Each session loads `race/<city>/<city>_{sailboat,ferry,train}[_<event
//! stem>].pathset` through the shared object lookup and spawns one
//! kinematic body per path (three cars per train path), driven along
//! the path by [`mm2_game::movers`]. The original creates the ferries
//! offline only; sailboats and trains always. Rules and constants are
//! recovered from the retail executable — `docs/research/movers.md`.

use std::collections::BTreeSet;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::pathset::Pathset;
pub use mm2_game::movers::MoverFamily;
use mm2_game::movers::{
    FERRY_SPEED, PathFollower, TRAIN_CARS, TrainMotion, mover_rotation, sailboat_speed,
};
use mm2_game::parked::ParkedRng;
use mm2_game::{CityEntity, Session, SessionEntity, SessionPhase, object_pathset_candidates};
use tracing::{info, warn};

use crate::banger::BangerDefs;
use crate::city::{MovableModel, MovableModels, v3};
use crate::layers::GameLayer;
use crate::object_sound::{ObjectSound, load_object_audio};

/// A boat or ferry following its path.
#[derive(Component, Debug, Clone)]
pub struct Mover {
    /// Which manager owns it.
    pub family: MoverFamily,
    /// Its spline follower.
    pub follower: PathFollower,
    /// Height added to the curve (`CG.y` of the ferry's bound record;
    /// sailboats ride the curve as authored).
    pub lift: f32,
}

/// One Underground train: its shuttle state and car bodies.
#[derive(Component, Debug, Clone)]
pub struct Train {
    /// The shuttle state.
    pub motion: TrainMotion,
    /// The car bodies, in [`TrainMotion::cars`] order.
    pub cars: Vec<Entity>,
    /// `CG.y` of the car's bound record.
    pub lift: f32,
}

/// Marker on a train car body.
#[derive(Component, Debug, Clone, Copy)]
pub struct TrainCar;

/// What the session's mover files produced.
#[derive(Resource, Debug, Default, Clone)]
pub struct MoverReport {
    /// Files loaded, one per family at most.
    pub files: Vec<String>,
    /// Candidate files that resolved but failed to parse.
    pub failed_files: Vec<String>,
    /// Boats spawned.
    pub sailboats: usize,
    /// Ferries spawned (moored two-point ones included).
    pub ferries: usize,
    /// Trains spawned.
    pub trains: usize,
    /// Paths whose model failed to load.
    pub missing_models: Vec<String>,
    /// Texture stems the models could not resolve.
    pub missing_textures: BTreeSet<String>,
}

fn load_pathset(
    vfs: &Vfs,
    city: &str,
    family: MoverFamily,
    event_stem: Option<&str>,
    report: &mut MoverReport,
) -> Option<Pathset> {
    for logical in object_pathset_candidates(city, family.object(), event_stem) {
        let Ok((bytes, resolved)) = vfs.read_path(&logical) else {
            continue;
        };
        match Pathset::parse(&bytes) {
            Ok(ps) => {
                report.files.push(resolved.logical.clone());
                return Some(ps);
            }
            Err(e) => {
                warn!(path = %resolved.logical, error = %e, "mover pathset parse failed; falling back");
                report.failed_files.push(resolved.logical.clone());
            }
        }
    }
    None
}

/// The model a path names: its name when `geometry/<name>.pkg`
/// resolves, else the family default (the managers' check).
fn model_name(vfs: &Vfs, family: MoverFamily, path_name: &str) -> String {
    family.model_for(path_name, |name| {
        vfs.resolve(&format!("geometry/{name}.pkg")).is_some()
    })
}

/// Spawn one kinematic body for `model` at `transform`; its origin is
/// the curve point, so rotation integrates about it.
pub(crate) fn spawn_body(
    commands: &mut Commands,
    model: &MovableModel,
    transform: Transform,
    owner: SessionEntity,
    name: String,
) -> Entity {
    let mut body = commands.spawn((
        CityEntity,
        owner,
        Name::new(name),
        transform,
        Visibility::default(),
        RigidBody::Kinematic,
        CenterOfMass(Vec3::ZERO),
        NoAutoCenterOfMass,
    ));
    if let Some(collider) = &model.collider {
        // Kinematic scenery never touches the static city geometry — see
        // `layers` for why that pair is worth skipping.
        body.insert((collider.clone(), GameLayer::scenery()));
    }
    let root = body.id();
    for (mesh, material) in &model.parts {
        let part = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
            ))
            .id();
        commands.entity(root).add_child(part);
    }
    root
}

/// Load the session's sailboat, ferry and train files and spawn their
/// objects, session-owned. `networked` sessions get no ferries (the
/// original creates that manager offline only).
#[allow(clippy::too_many_arguments)] // Bevy asset stores have to be threaded separately
pub fn spawn_movers(
    commands: &mut Commands,
    vfs: &Vfs,
    city: &str,
    event_stem: Option<&str>,
    networked: bool,
    seed: u64,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> MoverReport {
    let mut report = MoverReport::default();
    let mut models = MovableModels::new(vfs, meshes, images, materials);
    let mut bangers = BangerDefs::new(vfs);
    let mut rng = ParkedRng::new(seed ^ 0x6d6f_7665);
    // Ferries carry `ferry` (engine loop, horn every 5–10 s); a train
    // carries `subwaycar` on its middle car.
    let ferry_sound = load_object_audio(vfs, "ferry");
    let train_sound = load_object_audio(vfs, "subwaycar");
    let mut families = vec![MoverFamily::Sailboat, MoverFamily::Train];
    if !networked {
        families.insert(1, MoverFamily::Ferry);
    }
    for family in families {
        let Some(ps) = load_pathset(vfs, city, family, event_stem, &mut report) else {
            continue;
        };
        for (pi, path) in ps.paths.iter().enumerate() {
            let points: Vec<Vec3> = path.points.iter().map(|p| v3(p.position)).collect();
            if points.len() < 2 || points.iter().any(|p| !p.is_finite()) {
                continue;
            }
            let name = model_name(vfs, family, &path.name);
            let Some(model) = models.load(&name, Vec3::ZERO) else {
                warn!(path = %path.name, model = %name, "mover model unresolved; skipped");
                report.missing_models.push(name);
                continue;
            };
            let cg_y = bangers.get(&name).map(|d| d.cg[1]).unwrap_or(0.0);
            let label = format!("{}-{name}-{pi}", family.object());
            match family {
                MoverFamily::Sailboat | MoverFamily::Ferry => {
                    let (speed, lift) = if family == MoverFamily::Ferry {
                        (FERRY_SPEED, cg_y)
                    } else {
                        let draw = rng.next_roll() as f32 / 32767.0;
                        (sailboat_speed(path.spacing_metres(), draw), 0.0)
                    };
                    let Some(follower) = PathFollower::new(points, speed) else {
                        continue;
                    };
                    let (pos, dir) = follower.pose();
                    let transform = Transform::from_translation(pos + Vec3::Y * lift)
                        .with_rotation(mover_rotation(dir));
                    let body = spawn_body(commands, &model, transform, owner, label);
                    commands.entity(body).insert(Mover {
                        family,
                        follower,
                        lift,
                    });
                    if family == MoverFamily::Ferry
                        && let Some(spec) = &ferry_sound
                    {
                        commands
                            .entity(body)
                            .insert(ObjectSound::new(spec.clone(), pi as u32 + 1));
                    }
                    match family {
                        MoverFamily::Ferry => report.ferries += 1,
                        _ => report.sailboats += 1,
                    }
                }
                MoverFamily::Train => {
                    let Some(motion) = TrainMotion::new(&points) else {
                        continue;
                    };
                    let cars: Vec<Entity> = (0..TRAIN_CARS)
                        .map(|ci| {
                            let (pos, dir) = motion.car_pose(ci);
                            let transform = Transform::from_translation(pos + Vec3::Y * cg_y)
                                .with_rotation(mover_rotation(dir));
                            let car = spawn_body(
                                commands,
                                &model,
                                transform,
                                owner,
                                format!("{label}-car{ci}"),
                            );
                            commands.entity(car).insert(TrainCar);
                            car
                        })
                        .collect();
                    if let Some(spec) = &train_sound {
                        commands
                            .entity(cars[TRAIN_CARS / 2])
                            .insert(ObjectSound::new(spec.clone(), pi as u32 + 1));
                    }
                    commands.spawn((
                        CityEntity,
                        owner,
                        Name::new(label),
                        Train {
                            motion,
                            cars,
                            lift: cg_y,
                        },
                    ));
                    report.trains += 1;
                }
            }
        }
    }
    report.missing_textures = models.finish(commands, owner);
    info!(
        files = ?report.files,
        sailboats = report.sailboats,
        ferries = report.ferries,
        trains = report.trains,
        missing_models = report.missing_models.len(),
        "movers spawned"
    );
    report
}

/// Pose a kinematic body at `(p0, q0)` with the velocities that carry
/// it to `(p1, q1)` over `dt` — the solver then moves it exactly there,
/// and anything resting on it rides along.
pub(crate) fn pose_body(
    (pos, rot, lin, ang): (
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
    ),
    (p0, q0): (Vec3, Quat),
    (p1, q1): (Vec3, Quat),
    dt: f32,
) {
    pos.0 = p0;
    rot.0 = q0;
    if dt > 0.0 {
        lin.0 = (p1 - p0) / dt;
        ang.0 = (q1 * q0.inverse()).to_scaled_axis() / dt;
    } else {
        lin.0 = Vec3::ZERO;
        ang.0 = Vec3::ZERO;
    }
}

pub(crate) type BodyQuery<'a> = (
    &'a mut Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
);

/// Advance every boat, ferry and train one fixed step. Like the
/// drawbridges, each body is posed where its path puts it now and
/// left the velocity that reaches the next step's pose. Runs on every
/// peer — the motion is deterministic from session start.
#[allow(clippy::type_complexity)] // Bevy system: the queries are the system's signature
pub fn drive_movers(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    mut movers: Query<(&mut Mover, BodyQuery), Without<TrainCar>>,
    mut trains: Query<&mut Train>,
    mut cars: Query<BodyQuery, (With<TrainCar>, Without<Mover>)>,
    mut sounds: Query<&mut ObjectSound, With<TrainCar>>,
) {
    let running = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    );
    let dt = if running { time.delta_secs() } else { 0.0 };
    for (mut mover, (mut pos, mut rot, mut lin, mut ang)) in &mut movers {
        let lift = Vec3::Y * mover.lift;
        let (p0, d0) = mover.follower.pose();
        mover.follower.advance(dt);
        let (p1, d1) = mover.follower.pose();
        pose_body(
            (&mut pos, &mut rot, &mut lin, &mut ang),
            (p0 + lift, mover_rotation(d0)),
            (p1 + lift, mover_rotation(d1)),
            dt,
        );
    }
    for mut train in &mut trains {
        let lift = Vec3::Y * train.lift;
        let before: Vec<(Vec3, Vec3)> = (0..TRAIN_CARS).map(|i| train.motion.car_pose(i)).collect();
        train.motion.step(dt);
        // The Underground's sound switches rows by speed: the
        // `LondonTube` rumble (row 0) while the train moves, the
        // silent `NOTHING` row while it waits.
        if let Ok(mut sound) = sounds.get_mut(train.cars[TRAIN_CARS / 2]) {
            let moving = train.motion.moving();
            sound.state.set_active(Some(0), moving);
            sound.state.set_active(Some(1), !moving);
        }
        for (i, &car) in train.cars.iter().enumerate() {
            let Ok((mut pos, mut rot, mut lin, mut ang)) = cars.get_mut(car) else {
                continue;
            };
            let (p0, d0) = before[i];
            let (p1, d1) = train.motion.car_pose(i);
            pose_body(
                (&mut pos, &mut rot, &mut lin, &mut ang),
                (p0 + lift, mover_rotation(d0)),
                (p1 + lift, mover_rotation(d1)),
                dt,
            );
        }
    }
}

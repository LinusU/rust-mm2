//! F28-AC04 (#1468, #1505): a physics drive through London's Thames tunnel and
//! over its bridges against the city's real `CityWater`, asserting the
//! production recovery detector never classifies the car as drowned or
//! out of bounds.
//!
//! Evidence level: original-content validation — the real `load_city`
//! colliders and `CityWater`, the production vehicle step and recovery
//! systems, driven by a pursuit controller along authored BAI lanes. It
//! runs headless (no window or GPU); it does not judge how the drive
//! looks. The deadly-water *exposure* is the DSN-34 point-in-perimeter
//! test, so this is the check that its `WATER_DEPTH` column leaves the
//! tunnel dry under the Thames rooms and the bridges dry above them.
//!
//! Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's install,
//! and says so.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::load_city;
use mm2_app::contracts::ImpactFilter;
use mm2_app::drawbridge::DrawbridgeLeaf;
use mm2_app::recovery::{self, RecoveryReport};
use mm2_app::session::{SessionControl, SpawnPoint};
use mm2_assets::Vfs;
use mm2_game::drawbridge::{CLOSED_WAIT, DrawbridgeMode, LeafMotion};
use mm2_game::{
    Player, PlayerControl, RecoveryEvent, RecoveryPolicy, Session, SessionConfig, SessionEntity,
    SessionPhase, VehicleRecovery, advance_session_tick,
};
use mm2_vehicle::{TireConditions, VehicleConfig, VehicleInput, VehiclePlugin, vehicle_bundle};

#[derive(Resource)]
struct VfsRes(Vfs);

/// Road driven either side of the stretch over the water footprint,
/// metres, so the car crosses it at speed.
const LEAD: f32 = 40.0;
/// Cruise speed the pursuit controller holds, m/s.
const CRUISE: f32 = 9.0;
/// Pursuit look-ahead along the lane, metres.
const LOOKAHEAD: f32 = 7.0;

/// The loaded London with the production recovery stack and a player
/// car at `start`, facing `yaw`.
fn london_drive_app(vfs: Vfs, start: Vec3, yaw: f32) -> (App, Entity) {
    london_drive_app_with(vfs, start, yaw, Leaves::None)
}

/// What London's drawbridge leaves do in a drive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Leaves {
    /// No leaves are spawned (the PSDL rooms under them have no deck).
    None,
    /// Spawned and held closed for the whole drive.
    Closed,
    /// Spawned and run by the production `drive_drawbridges` timers,
    /// every leaf force-started in the opening phase.
    Opening,
}

fn london_drive_app_with(vfs: Vfs, start: Vec3, yaw: f32, leaves: Leaves) -> (App, Entity) {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let object = session.mint_object_id();
    let player = session.mint_player_id();
    let role = session.authority_role();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .insert_resource(TireConditions::default())
        .insert_resource(SpawnPoint::new(start, yaw))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_message::<mm2_game::ImpactEvent>()
        .add_message::<mm2_game::DamageEvent>()
        .add_message::<mm2_game::StuckEvent>()
        .add_message::<RecoveryEvent>()
        .insert_resource(ImpactFilter::default())
        .init_resource::<RecoveryReport>()
        .init_resource::<SessionControl>()
        .insert_resource(VfsRes(vfs))
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (recovery::track_recovery, recovery::resolve_recovery).chain(),
        );
    if leaves == Leaves::Opening {
        app.add_systems(FixedUpdate, mm2_app::drawbridge::drive_drawbridges);
    }
    app.finish();
    app.cleanup();

    let loaded = app
        .world_mut()
        .run_system_once(
            |mut commands: Commands,
             vfs: Res<VfsRes>,
             mut meshes: ResMut<Assets<Mesh>>,
             mut images: ResMut<Assets<Image>>,
             mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut session = Session::new();
                load_city(
                    &mut commands,
                    &vfs.0,
                    "city/london.psdl",
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                    &mut session,
                )
                .expect("retail London loads")
            },
        )
        .expect("load_city system runs");
    let water = loaded.water.expect("London ships a usable .water record");
    app.insert_resource(water);
    if let Some(floor) = loaded.floor {
        app.insert_resource(floor);
    }

    if leaves != Leaves::None {
        let report = app
            .world_mut()
            .run_system_once(
                |mut commands: Commands,
                 vfs: Res<VfsRes>,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut images: ResMut<Assets<Image>>,
                 mut materials: ResMut<Assets<StandardMaterial>>| {
                    mm2_app::drawbridge::spawn_drawbridges(
                        &mut commands,
                        &vfs.0,
                        "london",
                        None,
                        &mut meshes,
                        &mut images,
                        &mut materials,
                        SessionEntity(1),
                    )
                },
            )
            .expect("spawn_drawbridges system runs");
        assert!(
            report.leaves() > 0 && report.missing_models.is_empty(),
            "London's drawbridge leaves did not all spawn: {report:?}"
        );
        let mut q = app.world_mut().query::<&mut DrawbridgeLeaf>();
        for mut leaf in q.iter_mut(app.world_mut()) {
            leaf.motion = match leaves {
                // Whatever the authored mode, the leaf stays down.
                Leaves::Closed => LeafMotion::new(DrawbridgeMode::Inactive),
                // Every leaf lifts from the first step, as a timed leaf
                // does once its closed wait is over.
                _ => {
                    let mut m = LeafMotion::new(DrawbridgeMode::Timed);
                    m.timer = CLOSED_WAIT;
                    m
                }
            };
        }
    }

    let rot = Quat::from_rotation_y(yaw);
    let car = app
        .world_mut()
        .spawn((
            mm2_game::PlayerVehicle,
            mm2_game::ObjectIdentity(object),
            Player {
                id: player,
                control: PlayerControl::Local,
            },
            role,
            mm2_game::DamageSignals::default(),
            VehicleRecovery::with_anchor(RecoveryPolicy::default(), start, yaw),
            vehicle_bundle(&VehicleConfig::default()),
            Position(start),
            Rotation(rot),
            Transform::from_translation(start).with_rotation(rot),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    (app, car)
}

/// One lane to drive: its densified polyline, the number of vertices
/// over the water footprint.
struct Stretch {
    name: String,
    path: Vec<Vec3>,
    over: usize,
}

/// The lane's section vertices joined by cubic Hermite arcs on the
/// road's authored section tangents, one point per ~2 m. The sections
/// sit up to 200 m apart on the Thames tunnel, and the straight chord
/// between them leaves the road.
fn densify(pts: &[Vec3], road: &mm2_formats::bai::Road) -> Vec<Vec3> {
    if pts.len() != road.sections.len() {
        return pts.to_vec();
    }
    // A lane may run against the section order's travel direction.
    let sign = road
        .sections
        .iter()
        .zip(pts.windows(2))
        .map(|(sec, w)| Vec3::from(sec.tangent).dot(w[1] - w[0]))
        .sum::<f32>()
        .signum();
    let mut out = Vec::new();
    for (i, w) in pts.windows(2).enumerate() {
        let len = w[0].distance(w[1]);
        let m0 = Vec3::from(road.sections[i].tangent).normalize_or_zero() * sign * len;
        let m1 = Vec3::from(road.sections[i + 1].tangent).normalize_or_zero() * sign * len;
        let steps = (len / 2.0).ceil().max(1.0) as usize;
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                w[0] * (2.0 * t3 - 3.0 * t2 + 1.0)
                    + m0 * (t3 - 2.0 * t2 + t)
                    + w[1] * (-2.0 * t3 + 3.0 * t2)
                    + m1 * (t3 - t2),
            );
        }
    }
    out.push(*pts.last().unwrap());
    out
}

/// Every BAI lane's stretch over a deadly room's footprint, split by
/// whether the lane runs under the water column (a tunnel) or above the
/// surface (a bridge). The footprint test is the one the audit's frame
/// control uses: the vertex dropped to a metre under the level.
fn stretches(vfs: &Vfs, water: &mm2_app::water::CityWater) -> (Vec<Stretch>, Vec<Stretch>) {
    let (bytes, _) = vfs.read_path("city/london.bai").unwrap();
    let bai = mm2_formats::bai::Bai::parse(&bytes).unwrap();
    let (mut tunnels, mut bridges) = (Vec::new(), Vec::new());
    for road in &bai.roads {
        for (side_name, side) in [("right", &road.right), ("left", &road.left)] {
            for (l, curve) in side
                .lane_vertices
                .iter()
                .take(side.lane_count as usize)
                .enumerate()
            {
                let pts: Vec<Vec3> = curve.iter().map(|v| Vec3::from(*v)).collect();
                let over = |p: &Vec3| water.is_deadly(Vec3::new(p.x, water.level() - 1.0, p.z));
                let dense = densify(&pts, road);
                let (Some(first), Some(last)) =
                    (dense.iter().position(over), dense.iter().rposition(over))
                else {
                    continue;
                };
                // The footprint stretch plus the road either side, so
                // the car crosses it at speed.
                let lead = (LEAD / 2.0) as usize;
                let from = first.saturating_sub(lead);
                let to = (last + lead).min(dense.len() - 1);
                let span = &dense[first..=last];
                let min_y = span.iter().map(|p| p.y).fold(f32::MAX, f32::min);
                let max_y = span.iter().map(|p| p.y).fold(f32::MIN, f32::max);
                let stretch = Stretch {
                    name: format!("road {} {side_name} lane {l}", road.id),
                    path: dense[from..=to].to_vec(),
                    over: span.len(),
                };
                // Under the surface by more than the car is tall: a
                // tunnel. Clear above it: a bridge. Anything between is
                // wading or a ramp, not what this drive is about.
                if max_y < water.level() - 3.0 {
                    tunnels.push(stretch);
                } else if min_y > water.level() + 1.0 {
                    bridges.push(stretch);
                }
            }
        }
    }
    (tunnels, bridges)
}

fn retail_vfs(what: &str) -> Option<Vfs> {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; {what} was NOT checked");
        return None;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    Some(vfs)
}

fn heading(from: Vec3, to: Vec3) -> f32 {
    f32::atan2(-(to.x - from.x), -(to.z - from.z))
}

/// What a drive saw.
struct Drive {
    reached: bool,
    frames: usize,
    over_frames: usize,
    min_y: f32,
    max_y: f32,
    deadly_frames: usize,
    recovered: u64,
    submerged: u64,
    out_of_bounds: u64,
}

/// Drive `path` end to end with a pursuit controller on the production
/// vehicle step, watching the car against the real `CityWater`.
fn drive(vfs: Vfs, path: &[Vec3]) -> Drive {
    drive_with(vfs, path, Leaves::None)
}

fn drive_with(vfs: Vfs, path: &[Vec3], leaves: Leaves) -> Drive {
    let start = path[0] + Vec3::Y * 0.8;
    let (mut app, car) = london_drive_app_with(vfs, start, heading(path[0], path[1]), leaves);
    let length: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let budget = (length / (CRUISE * 0.3) * 60.0) as usize + 600;
    let end = *path.last().unwrap();
    let mut progress = 0usize;
    let mut out = Drive {
        reached: false,
        frames: 0,
        over_frames: 0,
        min_y: f32::MAX,
        max_y: f32::MIN,
        deadly_frames: 0,
        recovered: 0,
        submerged: 0,
        out_of_bounds: 0,
    };
    while out.frames < budget && !out.reached {
        out.frames += 1;
        let (pos, rot, speed) = {
            let w = app.world();
            (
                w.get::<Position>(car).unwrap().0,
                w.get::<Rotation>(car).unwrap().0,
                w.get::<LinearVelocity>(car).unwrap().0.length(),
            )
        };
        // Progress: the nearest vertex at or ahead of the last one.
        while progress + 1 < path.len()
            && path[progress + 1].distance(pos) < path[progress].distance(pos)
        {
            progress += 1;
        }
        // Target: the lane point a look-ahead further on (the dense
        // path is ~2 m apart).
        let target = path[(progress + (LOOKAHEAD / 2.0) as usize).min(path.len() - 1)];
        let to = (target - pos) * Vec3::new(1.0, 0.0, 1.0);
        let right = rot * Vec3::X;
        let steering = (to.normalize_or_zero().dot(right) * 2.0).clamp(-1.0, 1.0);
        app.world_mut()
            .get_mut::<VehicleInput>(car)
            .unwrap()
            .clone_from(&VehicleInput {
                throttle: if speed < CRUISE { 1.0 } else { 0.0 },
                steering,
                ..default()
            });
        app.update();

        let water = app.world().resource::<mm2_app::water::CityWater>();
        let pos = app.world().get::<Position>(car).unwrap().0;
        out.min_y = out.min_y.min(pos.y);
        out.max_y = out.max_y.max(pos.y);
        out.deadly_frames += usize::from(water.is_deadly(pos));
        out.over_frames +=
            usize::from(water.is_deadly(Vec3::new(pos.x, water.level() - 1.0, pos.z)));
        out.reached = progress + 1 >= path.len() || pos.distance(end) < 4.0;
    }
    let report = app.world().resource::<RecoveryReport>();
    out.recovered = report.recovered;
    out.submerged = report.submerged;
    out.out_of_bounds = report.out_of_bounds;
    out
}

fn assert_clean(what: &str, d: &Drive) {
    assert_safe(what, d);
    assert!(d.reached, "{what}: the car never finished the lane");
}

/// The drowning and recovery claims alone: they hold for a drive that
/// did not finish its lane as well.
fn assert_safe(what: &str, d: &Drive) {
    eprintln!(
        "{what}: reached={} in {} frames, {} frames over the footprint, y {:.1}..{:.1}, \
         {} frames in deadly water, recovered {} (submerged {}, out of bounds {})",
        d.reached,
        d.frames,
        d.over_frames,
        d.min_y,
        d.max_y,
        d.deadly_frames,
        d.recovered,
        d.submerged,
        d.out_of_bounds
    );
    assert_eq!(d.deadly_frames, 0, "{what}: the car stood in deadly water");
    assert_eq!(
        (d.recovered, d.submerged, d.out_of_bounds),
        (0, 0, 0),
        "{what}: the car was recovered"
    );
}

fn london_stretches() -> Option<(Vec<Stretch>, Vec<Stretch>, Vec<Stretch>)> {
    let vfs = retail_vfs("the Thames drives")?;
    let (mut app, _car) = london_drive_app(vfs, Vec3::new(0.0, 50.0, 0.0), 0.0);
    let water = app.world().resource::<mm2_app::water::CityWater>();
    let vfs = &app.world().resource::<VfsRes>().0;
    let (tunnels, bridges) = stretches(vfs, water);
    // A bridge lane with no static collider within 5 m under some
    // vertex — Tower Bridge's and Waterloo's drawbridge leaves are not
    // PSDL geometry (`spawn_drawbridges` places them, timer-driven) —
    // has nothing to drive on without the leaves; those lanes are
    // returned separately and driven with the leaves spawned.
    // A lane that merely clips the footprint's edge proves nothing.
    let mut bridges = Some(
        bridges
            .into_iter()
            .filter(|b| b.over >= 5)
            .collect::<Vec<_>>(),
    );
    let (driven, leaves): (Vec<_>, Vec<_>) = app
        .world_mut()
        .run_system_once(move |spatial: SpatialQuery| {
            bridges.take().unwrap().into_iter().partition(|b| {
                b.path.iter().all(|p| {
                    spatial
                        .cast_ray(
                            *p + Vec3::Y * 1.0,
                            Dir3::NEG_Y,
                            6.0,
                            true,
                            &SpatialQueryFilter::default(),
                        )
                        .is_some()
                })
            })
        })
        .unwrap();
    eprintln!(
        "bridges driven: {:?}; no static deck under the lane, not driven: {:?}",
        driven.iter().map(|b| &b.name).collect::<Vec<_>>(),
        leaves.iter().map(|b| &b.name).collect::<Vec<_>>()
    );
    Some((tunnels, driven, leaves))
}

/// The Thames tunnel (its lane runs ~10 m under the −3.8 river
/// surface, beneath rooms that the deadly overlay covers) is driven end
/// to end in both directions: every frame of the car stays out of the
/// deadly volume and the recovery detector never fires.
#[test]
fn a_car_drives_the_thames_tunnel_both_ways_without_drowning() {
    let Some((tunnels, ..)) = london_stretches() else {
        return;
    };
    assert!(!tunnels.is_empty(), "London lost its Thames tunnel lane");
    for tunnel in tunnels {
        let level = -3.8;
        for reverse in [false, true] {
            let mut path = tunnel.path.clone();
            if reverse {
                path.reverse();
            }
            let what = format!(
                "{} {}",
                tunnel.name,
                if reverse { "reversed" } else { "forward" }
            );
            let d = drive(retail_vfs(&what).unwrap(), &path);
            assert_clean(&what, &d);
            assert!(
                d.over_frames > 100,
                "{what}: the car never crossed the footprint"
            );
            assert!(
                d.max_y < level - 3.0,
                "{what}: the car came up out of the tunnel (y {:.1}) while crossing",
                d.max_y
            );
        }
    }
}

/// Every London bridge lane over a deadly room that has a static deck
/// is driven: the car rides the deck above the surface and the
/// overlay's vertical bound keeps it dry.
#[test]
fn a_car_drives_each_london_bridge_over_the_river_without_drowning() {
    let Some((_, bridges, _)) = london_stretches() else {
        return;
    };
    let mut crossed = Vec::new();
    for bridge in bridges {
        let d = drive(retail_vfs(&bridge.name).unwrap(), &bridge.path);
        // Safety holds for every drive, crossing or not.
        assert_clean(&bridge.name, &d);
        assert!(
            d.min_y > -3.8 + 0.5,
            "{}: the car sank to y {:.1}",
            bridge.name,
            d.min_y
        );
        // A lane the car drifted off (road 496 runs along a room wall
        // the pursuit controller does not clear) proves nothing about
        // the deck: count only the drives that were over the water.
        if d.over_frames > 0 {
            crossed.push(bridge.name);
        }
    }
    eprintln!("crossed the river on: {crossed:?}");
    assert!(
        crossed.len() >= 4,
        "bridge lanes driven over the river: {crossed:?}"
    );
}

/// Control: the same app and detector do fire for a car dropped into
/// the river beside a bridge, so the clean drives above are not an
/// unarmed detector.
#[test]
fn a_car_dropped_into_the_river_under_a_bridge_still_drowns() {
    let Some((_, bridges, _)) = london_stretches() else {
        return;
    };
    let bridge = bridges.first().expect("a London bridge");
    let vfs = retail_vfs("the drowning control").unwrap();
    let water_level = -3.8;
    let (app0, _) = london_drive_app(vfs, Vec3::new(0.0, 50.0, 0.0), 0.0);
    let water = app0.world().resource::<mm2_app::water::CityWater>();
    let spot = bridge
        .path
        .iter()
        .copied()
        .find(|p| water.is_deadly(Vec3::new(p.x, water_level - 1.0, p.z)))
        .expect("a bridge vertex over the river");
    drop(app0);
    let (mut app, car) = london_drive_app(
        retail_vfs("the drowning control").unwrap(),
        Vec3::new(spot.x, water_level + 0.8, spot.z),
        0.0,
    );
    for _ in 0..480 {
        app.update();
    }
    let report = app.world().resource::<RecoveryReport>();
    let pos = app.world().get::<Position>(car).unwrap().0;
    assert!(
        report.submerged >= 1 && report.recovered >= 1,
        "dropped into the Thames at {spot:?} and never recovered: {report:?}, car at {pos:?}"
    );
}

/// The drawbridge-leaf lanes (no static deck: roads 407, 415, 483, 508)
/// are driven with the retail leaves spawned by `spawn_drawbridges` and
/// held closed: the leaf is the deck, so the car rides it over the river
/// and the detector stays quiet.
#[test]
fn a_car_drives_each_closed_drawbridge_leaf_over_the_river_without_drowning() {
    let Some((_, _, lanes)) = london_stretches() else {
        return;
    };
    assert!(!lanes.is_empty(), "London lost its drawbridge-leaf lanes");
    let mut crossed = Vec::new();
    for lane in lanes {
        let d = drive_with(retail_vfs(&lane.name).unwrap(), &lane.path, Leaves::Closed);
        // Safety holds for every drive; road 415's lane is left
        // unfinished by the pursuit controller before it reaches the
        // river (0 frames over the footprint), which says nothing
        // about the leaf, so only the drives that crossed count.
        assert_safe(&lane.name, &d);
        assert!(d.min_y > -3.8 + 0.5, "{}: the car sank", lane.name);
        if d.over_frames > 0 {
            crossed.push(lane.name);
        }
    }
    eprintln!("crossed the river on a closed leaf: {crossed:?}");
    assert!(
        !crossed.is_empty(),
        "no leaf lane was driven over the river"
    );
}

/// Leaves lifting under a car that is already on them: the car is never
/// reported in deadly water while it is carried up, and nothing is
/// recovered. (The leaf rises 0.05 rad/s, so the lane is driven within
/// the opening phase.)
#[test]
fn a_car_on_an_opening_drawbridge_leaf_is_never_in_deadly_water() {
    let Some((_, _, lanes)) = london_stretches() else {
        return;
    };
    for lane in lanes {
        let d = drive_with(retail_vfs(&lane.name).unwrap(), &lane.path, Leaves::Opening);
        eprintln!(
            "{} (opening): reached={}, deadly {}",
            lane.name, d.reached, d.deadly_frames
        );
        assert_eq!(d.deadly_frames, 0, "{}: stood in deadly water", lane.name);
        assert_eq!(
            (d.recovered, d.submerged, d.out_of_bounds),
            (0, 0, 0),
            "{}: recovered",
            lane.name
        );
    }
}
